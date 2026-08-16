//! CLI 端到端集成测试（针对编译产物，CARGO_BIN_EXE_wan）

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_wan")
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
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
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
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
    let out = Command::new(bin())
        .arg("run")
        .arg("--json")
        .arg(&f)
        .output()
        .unwrap();
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
fn output_passthrough() {
    let f = fixture("output-win.yml");
    if !cfg!(windows) {
        return;
    }
    let out = Command::new(bin()).arg("run").arg(&f).output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("got: from-step-1"), "stdout: {stdout}");
}

#[test]
fn list_command() {
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    let out = Command::new(bin())
        .arg("list")
        .arg("-C")
        .arg(&fixtures_dir)
        .output()
        .unwrap();
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
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
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
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
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
    let (plat_shell, _unix_shell) = if cfg!(windows) {
        ("cmd", "sh")
    } else {
        ("sh", "sh")
    };
    std::fs::write(wf_dir.join("short-win.yml"), content("cmd", "win-ran")).unwrap();
    std::fs::write(wf_dir.join("short-unix.yml"), content("sh", "unix-ran")).unwrap();
    std::fs::write(wf_dir.join("plain.yml"), content(plat_shell, "plain-ran")).unwrap();

    let run_in = |name: &str| {
        Command::new(bin())
            .arg("run")
            .arg("-C")
            .arg(&tmp)
            .arg(name)
            .output()
            .unwrap()
    };

    // 平台后缀优先（Windows 取 short-win，Linux 取 short-unix）
    let out = run_in("short");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let expected_wf = if cfg!(windows) {
        "short-win"
    } else {
        "short-unix"
    };
    assert!(
        stdout.contains(&format!("workflow: {expected_wf}")),
        "stdout: {stdout}"
    );

    // 无平台后缀时回退到同名文件
    let out = run_in("plain");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("workflow: plain"),
        "stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    // 都不存在 → 退出码 2，stderr 列出候选
    let out = run_in("missing");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("未找到"), "stderr: {stderr}");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// 生成两个并行 job 的 workflow（各自先延迟再输出多行，确保输出阶段已并行）
fn make_parallel_workflow(dir: &Path) -> PathBuf {
    let (shell, run_a, run_b) = if cfg!(windows) {
        (
            "cmd",
            "ping -n 2 127.0.0.1 >nul&&echo alpha-1&&echo alpha-2&&echo alpha-3",
            "ping -n 2 127.0.0.1 >nul&&echo beta-1&&echo beta-2&&echo beta-3",
        )
    } else {
        (
            "sh",
            "sleep 1; echo alpha-1; echo alpha-2; echo alpha-3",
            "sleep 1; echo beta-1; echo beta-2; echo beta-3",
        )
    };
    let yml = format!(
        "version: 1\njobs:\n  alpha:\n    steps:\n      - name: say\n        shell: {shell}\n        run: \"{run_a}\"\n  beta:\n    steps:\n      - name: say\n        shell: {shell}\n        run: \"{run_b}\"\n"
    );
    let f = dir.join("parallel.yml");
    std::fs::write(&f, yml).unwrap();
    f
}

/// 并行分组：同一 job 的输出行连续，不被另一 job 穿插
#[test]
fn parallel_jobs_grouped_output() {
    let tmp = std::env::temp_dir().join(format!("wan-par-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let f = make_parallel_workflow(&tmp);

    let out = Command::new(bin()).arg("run").arg(&f).output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();

    let pos = |pat: &str| -> Vec<usize> {
        lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.trim_start().starts_with(pat))
            .map(|(i, _)| i)
            .collect()
    };
    let a = pos("alpha-");
    let b = pos("beta-");
    assert_eq!(a.len(), 3, "alpha 行数不对: {stdout}");
    assert_eq!(b.len(), 3, "beta 行数不对: {stdout}");

    // 各自连续（3 行紧挨）且两块互不嵌套
    assert_eq!(a[2] - a[0], 2, "alpha 行被穿插: {stdout}");
    assert_eq!(b[2] - b[0], 2, "beta 行被穿插: {stdout}");
    assert!(
        a[2] < b[0] || b[2] < a[0],
        "alpha/beta 输出块交错: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// --no-group：恢复实时直通（无分组 banner）
#[test]
fn no_group_disables_banner() {
    let tmp = std::env::temp_dir().join(format!("wan-par-ng-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let f = make_parallel_workflow(&tmp);

    let out = Command::new(bin())
        .arg("run")
        .arg("--no-group")
        .arg(&f)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("── [job]"),
        "--no-group 下不应出现分组 banner: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
