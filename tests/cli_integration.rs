//! CLI 端到端集成测试（针对编译产物，CARGO_BIN_EXE_wan）

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_wan")
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join(name)
}

/// Windows 用 cmd fixture；其余平台用 sh fixture
fn platform_fixture(win: &str, unix: &str) -> PathBuf {
    if cfg!(windows) {
        fixture(win)
    } else {
        fixture(unix)
    }
}

#[test]
fn run_success_exit0() {
    let f = platform_fixture("hello-win.yml", "hello-unix.yml");
    let out = Command::new(bin()).arg("run").arg(&f).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("hello from"), "stdout: {stdout}");
}

#[test]
fn run_failure_exit1_and_skip_propagation() {
    let f = platform_fixture("fail-win.yml", "fail-unix.yml");
    let out = Command::new(bin()).arg("run").arg(&f).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("SHOULD NOT RUN"), "stdout: {stdout}");
    assert!(stdout.contains("FAIL"), "stdout: {stdout}");
}

#[test]
fn json_events_shape() {
    let f = platform_fixture("hello-win.yml", "hello-unix.yml");
    let out = Command::new(bin()).arg("run").arg("--json").arg(&f).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(!lines.is_empty(), "no events in stdout");
    let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first["type"], "run_start");
    assert_eq!(first["workflow"], "hello-win");
    let last: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    assert_eq!(last["type"], "run_end");
    assert_eq!(last["code"], 0);
}

#[test]
fn validate_ok_and_bad() {
    let good = platform_fixture("hello-win.yml", "hello-unix.yml");
    let out = Command::new(bin()).arg("validate").arg(&good).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("OK"), "stdout: {stdout}");

    let bad = fixture("dag-cycle.yml");
    let out = Command::new(bin()).arg("validate").arg(&bad).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("环"), "stderr: {stderr}");
}

#[test]
fn graph_mermaid() {
    let f = platform_fixture("hello-win.yml", "hello-unix.yml");
    let out = Command::new(bin()).arg("graph").arg(&f).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("flowchart TD"), "stdout: {stdout}");
    assert!(stdout.contains("hello"), "stdout: {stdout}");
}

#[test]
fn output_passthrough() {
    let f = fixture("output-win.yml");
    if !cfg!(windows) {
        return;
    }
    let out = Command::new(bin()).arg("run").arg(&f).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("got: from-step-1"), "stdout: {stdout}");
}

#[test]
fn list_command() {
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures");
    let out = Command::new(bin()).arg("list").arg("-C").arg(&fixtures_dir).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("hello-win"), "stdout: {stdout}");
    assert!(stdout.contains("fail-win"), "stdout: {stdout}");
}

#[test]
fn retry_recovers() {
    if !cfg!(windows) {
        return;
    }
    let marker = std::env::temp_dir().join("wan-retry-marker.txt");
    let _ = std::fs::remove_file(&marker);
    let f = fixture("retry-win.yml");
    let out = Command::new(bin()).arg("run").arg(&f).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("重试"), "stderr: {stderr}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("OK"), "stdout: {stdout}");
}

fn has_pwsh() -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|d| d.join(if cfg!(windows) { "pwsh.exe" } else { "pwsh" }))
        .any(|p| p.is_file())
}

#[test]
fn pwsh_smoke() {
    if !has_pwsh() {
        eprintln!("skipping: pwsh not on PATH");
        return;
    }
    let f = fixture("pwsh-win.yml");
    let out = Command::new(bin()).arg("run").arg(&f).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("got ver="), "stdout: {stdout}");
    assert!(stdout.contains("second try ok"), "stdout: {stdout}");
}

#[test]
fn version_and_help() {
    let out = Command::new(bin()).arg("--version").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("wan "));

    let out = Command::new(bin()).arg("--help").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("用法"));
}

#[test]
fn short_name_resolution() {
    let tmp = std::env::temp_dir().join(format!("wan-int-{}", std::process::id()));
    let wf_dir = tmp.join(".wan").join("workflows");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&wf_dir).unwrap();
    let content = |shell: &str, mark: &str| {
        format!(
            "version: 1\njobs:\n  a:\n    steps:\n      - name: x\n        shell: {shell}\n        run: echo {mark}\n"
        )
    };
    let (plat_shell, unix_shell) = if cfg!(windows) { ("cmd", "sh") } else { ("sh", "sh") };
    std::fs::write(wf_dir.join("short-win.yml"), content("cmd", "win-ran")).unwrap();
    std::fs::write(wf_dir.join("short-unix.yml"), content("sh", "unix-ran")).unwrap();
    std::fs::write(wf_dir.join("plain.yml"), content(plat_shell, "plain-ran")).unwrap();

    let run_in = |name: &str| {
        Command::new(bin()).arg("run").arg("-C").arg(&tmp).arg(name).output().unwrap()
    };

    // 平台后缀优先（Windows 取 short-win，Linux 取 short-unix）
    let out = run_in("short");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let expected_wf = if cfg!(windows) { "short-win" } else { "short-unix" };
    assert!(stdout.contains(&format!("workflow: {expected_wf}")), "stdout: {stdout}");

    // 无平台后缀时回退到同名文件
    let out = run_in("plain");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("workflow: plain"), "stdout: {}", String::from_utf8_lossy(&out.stdout));

    // 都不存在 → 退出码 2，stderr 列出候选
    let out = run_in("missing");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("未找到"), "stderr: {stderr}");

    let _ = std::fs::remove_dir_all(&tmp);
}
