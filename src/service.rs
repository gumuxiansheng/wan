//! 系统服务集成（spec §4.7 F-CRON-5，§15.5）
//!
//! Windows: schtasks 定时任务（每分钟启动一次 `wan schedule run-once`）
//! Linux: systemd user unit（wan-schedule.service + wan-schedule.timer）
//!
//! 设计决策：
//! - 不实现 Windows Service（ServiceMain 入口）——wan 是 CLI 工具，schtasks 更合适
//! - schtasks 每分钟触发一次 `wan schedule run-once`，由 wan 内部判断哪些调度到点
//! - Linux 用 systemd user unit（不需要 root），timer 每分钟触发
//! - 任务名/unit 名含项目路径哈希（WanSchedule-<hash8> / wan-schedule-<hash8>），
//!   同机多项目分别 setup 时互不覆盖
//! - 旧版使用固定名 WanSchedule / wan-schedule，install/remove 时顺带清理
//! - Windows 下通过 `wan-shim.exe`（GUI 子系统，无控制台窗口）启动 wan.exe：
//!   schtasks /TR 指向 shim，shim 以 CREATE_NO_WINDOW 启动 wan.exe schedule run-once。
//!   不依赖 wscript/vbs，不受 VBScript 2027 弃用影响。
//!   旧版曾用 .vbs 隐藏窗口，install 时顺带清理旧 vbs/bat 文件。
//!   任务注册为 "仅登录时运行"（/IT）——不同用户会话下可见性一致，均不弹窗

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// wan 自身可执行文件路径
fn self_exe() -> Result<PathBuf> {
    std::env::current_exe().map_err(|e| Error::io(format!("无法获取自身路径：{e}")))
}

/// FNV-1a 64 位哈希（内联实现，避免为此引入依赖）
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 规范化项目根路径，使同一目录的不同写法得到相同哈希：
/// - 相对路径基于 cwd 转绝对
/// - components() 归一化分隔符、尾部分隔符与冗余 `.`
/// - Windows 路径大小写不敏感，统一转小写
///
/// 不用 canonicalize：它要求路径存在、会解析符号链接、且返回 `\\?\` 前缀，
/// 反而可能使同一项目的不同访问方式得到不同哈希。
fn normalized_base(base: &Path) -> PathBuf {
    let abs = if base.is_absolute() {
        base.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(base)
    };

    let mut normalized = PathBuf::new();
    for comp in abs.components() {
        normalized.push(comp.as_os_str());
    }

    #[cfg(windows)]
    {
        normalized = PathBuf::from(normalized.to_string_lossy().to_lowercase());
    }
    normalized
}

/// 项目路径哈希（FNV-1a 低 32 位，8 位 hex）
fn base_hash(base: &Path) -> String {
    let normalized = normalized_base(base);
    format!("{:08x}", fnv1a(normalized.to_string_lossy().as_bytes()) as u32)
}

/// Windows: schtasks 任务名（含项目哈希，多项目并存）
#[cfg(windows)]
pub fn task_name(base: &Path) -> String {
    format!("WanSchedule-{}", base_hash(base))
}

/// Linux: systemd unit 名称（含项目哈希，多项目并存）
#[cfg(unix)]
pub fn unit_name(base: &Path) -> String {
    format!("wan-schedule-{}", base_hash(base))
}

/// 安装系统服务
pub fn install(base: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        install_windows(base)
    }
    #[cfg(unix)]
    {
        install_linux(base)
    }
    #[cfg(not(any(windows, unix)))]
    {
        Err(Error::config("当前平台不支持服务集成"))
    }
}

/// 移除系统服务
pub fn remove(base: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        remove_windows(base)
    }
    #[cfg(unix)]
    {
        remove_linux(base)
    }
    #[cfg(not(any(windows, unix)))]
    {
        Err(Error::config("当前平台不支持服务集成"))
    }
}

/// 查看服务状态
pub fn status(base: &Path) -> Result<String> {
    #[cfg(windows)]
    {
        status_windows(base)
    }
    #[cfg(unix)]
    {
        status_linux(base)
    }
    #[cfg(not(any(windows, unix)))]
    {
        Err(Error::config("当前平台不支持服务集成"))
    }
}

// ── Windows: schtasks ──

/// 旧版固定任务名，install/remove 时顺带清理（避免残留任务继续触发旧 bat）
#[cfg(windows)]
const LEGACY_TASK_NAME: &str = "WanSchedule";

/// Windows: 查找同目录下的 wan-shim.exe（GUI 子系统，无控制台窗口）
/// schtasks /TR 指向 shim，shim 以 CREATE_NO_WINDOW 启动 wan.exe schedule run-once
#[cfg(windows)]
fn shim_path() -> Result<PathBuf> {
    let exe = self_exe()?;
    let dir = exe.parent().ok_or_else(|| Error::io("无法获取 exe 目录"))?;
    let shim = dir.join("wan-shim.exe");
    if !shim.exists() {
        return Err(Error::io(format!(
            "wan-shim.exe 不存在于 {}（请确认 gates-toolkit bin 目录完整）",
            dir.display()
        )));
    }
    Ok(shim)
}
fn install_windows(base: &Path) -> Result<()> {
    let exe = self_exe()?;
    let base_str = base.to_string_lossy();
    let dir = crate::schedule::schedules_dir(base);
    std::fs::create_dir_all(&dir)?;

    // 清理旧版遗留的 vbs/bat wrapper（若存在）
    for legacy in ["run-once.vbs", "run-once.bat"] {
        let p = dir.join(legacy);
        if p.exists() {
            let _ = std::fs::remove_file(&p);
        }
    }

    let task = task_name(base);

    // schtasks /TR 指向 wan-shim.exe（GUI 子系统，无控制台窗口）
    // shim 以 CREATE_NO_WINDOW 启动 wan.exe schedule run-once -C <base>
    // 构造命令行：wan-shim.exe "<wan-exe>" schedule run-once -C "<base>"
    let shim = shim_path()?;
    let tr = format!(
        "\"{}\" \"{}\" schedule run-once -C \"{}\"",
        shim.display(),
        exe.display(),
        base_str
    );

    //  - /IT：仅登录时运行，确保任务在当前登录用户会话内启动，与交互桌面一致。
    //  - 不做 /RU SYSTEM：无窗口 + 无需管理员权限。
    let output = std::process::Command::new("schtasks")
        .args([
            "/Create",
            "/TN",
            &task,
            "/TR",
            &tr,
            "/SC",
            "MINUTE",
            "/MO",
            "1",
            "/IT",
            "/F",
        ])
        .output()
        .map_err(|e| Error::io(format!("执行 schtasks 失败：{e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(Error::config(format!(
            "schtasks 注册失败：{stderr}{stdout}"
        )));
    }

    remove_legacy_windows();

    Ok(())
}

/// 删除旧版固定名任务（若存在）。注意：同机其他仍用旧版 wan 的项目会被一并停掉，
/// 属一次性迁移代价。
#[cfg(windows)]
fn remove_legacy_windows() {
    let _ = std::process::Command::new("schtasks")
        .args(["/Delete", "/TN", LEGACY_TASK_NAME, "/F"])
        .output();
}

#[cfg(windows)]
fn legacy_task_exists() -> bool {
    std::process::Command::new("schtasks")
        .args(["/Query", "/TN", LEGACY_TASK_NAME])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn remove_windows(base: &Path) -> Result<()> {
    let output = std::process::Command::new("schtasks")
        .args(["/Delete", "/TN", &task_name(base), "/F"])
        .output()
        .map_err(|e| Error::io(format!("执行 schtasks 失败：{e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // 任务不存在不算错误
        if stderr.contains("无法找到")
            || stderr.contains("does not exist")
            || stderr.contains("cannot find")
        {
            remove_legacy_windows();
            return Ok(());
        }
        return Err(Error::config(format!("schtasks 删除失败：{stderr}")));
    }

    remove_legacy_windows();

    Ok(())
}

#[cfg(windows)]
fn status_windows(base: &Path) -> Result<String> {
    let output = std::process::Command::new("schtasks")
        .args(["/Query", "/TN", &task_name(base), "/FO", "LIST"])
        .output()
        .map_err(|e| Error::io(format!("执行 schtasks 失败：{e}")))?;

    if !output.status.success() {
        if legacy_task_exists() {
            return Ok(format!(
                "未安装（未找到当前项目任务）。检测到旧版任务 {LEGACY_TASK_NAME}，\
                 重新执行 `wan schedule service install` 可迁移。"
            ));
        }
        return Ok("未安装（schtasks 未找到任务）".to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(stdout)
}

// ── Linux: systemd user unit ──

/// 旧版固定 unit 名，install/remove 时顺带清理
#[cfg(unix)]
const LEGACY_UNIT: &str = "wan-schedule";

#[cfg(unix)]
fn systemd_user_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").map_err(|_| Error::config("无法获取 HOME 环境变量"))?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("systemd")
        .join("user"))
}

#[cfg(unix)]
fn install_linux(base: &Path) -> Result<()> {
    let exe = self_exe()?;
    let exe_str = exe.to_string_lossy();
    let base_str = base.to_string_lossy();
    let dir = systemd_user_dir()?;
    std::fs::create_dir_all(&dir)?;

    let unit = unit_name(base);
    let service_path = dir.join(format!("{unit}.service"));
    let timer_path = dir.join(format!("{unit}.timer"));

    // service 文件
    let service_content = format!(
        "[Unit]\n\
         Description=wan schedule daemon ({unit})\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         ExecStart={exe_str} schedule run-once -C {base_str}\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    );

    // timer 文件：每分钟触发
    let timer_content = format!(
        "[Unit]\n\
         Description=wan schedule timer (every minute)\n\
         \n\
         [Timer]\n\
         OnCalendar=*:0/1\n\
         Persistent=true\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    );

    std::fs::write(&service_path, service_content)?;
    std::fs::write(&timer_path, timer_content)?;

    // systemctl --user daemon-reload
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .output();

    // systemctl --user enable --now <unit>.timer
    let output = std::process::Command::new("systemctl")
        .args(["--user", "enable", "--now", &format!("{unit}.timer")])
        .output()
        .map_err(|e| Error::io(format!("执行 systemctl 失败：{e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::config(format!(
            "systemctl enable 失败：{stderr}（请确认 systemd user session 可用）"
        )));
    }

    remove_legacy_linux();

    Ok(())
}

/// 删除旧版固定名 unit（若存在）。注意：同机其他仍用旧版 wan 的项目会被一并停掉，
/// 属一次性迁移代价。
#[cfg(unix)]
fn remove_legacy_linux() {
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "disable", "--now", &format!("{LEGACY_UNIT}.timer")])
        .output();
    if let Ok(dir) = systemd_user_dir() {
        let _ = std::fs::remove_file(dir.join(format!("{LEGACY_UNIT}.timer")));
        let _ = std::fs::remove_file(dir.join(format!("{LEGACY_UNIT}.service")));
    }
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .output();
}

#[cfg(unix)]
fn remove_linux(base: &Path) -> Result<()> {
    let unit = unit_name(base);
    let dir = systemd_user_dir()?;

    // systemctl --user disable --now <unit>.timer
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "disable", "--now", &format!("{unit}.timer")])
        .output();

    // 删除 unit 文件
    let timer_path = dir.join(format!("{unit}.timer"));
    let service_path = dir.join(format!("{unit}.service"));
    let _ = std::fs::remove_file(&timer_path);
    let _ = std::fs::remove_file(&service_path);

    // daemon-reload
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .output();

    remove_legacy_linux();

    Ok(())
}

#[cfg(unix)]
fn status_linux(base: &Path) -> Result<String> {
    let unit = unit_name(base);
    let output = std::process::Command::new("systemctl")
        .args(["--user", "status", &format!("{unit}.timer")])
        .output()
        .map_err(|e| Error::io(format!("执行 systemctl 失败：{e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if output.status.success() {
        Ok(stdout)
    } else if systemd_user_dir()
        .map(|d| d.join(format!("{LEGACY_UNIT}.timer")).exists())
        .unwrap_or(false)
    {
        Ok(format!(
            "未安装或未运行。检测到旧版 unit {LEGACY_UNIT}，\
             重新执行 `wan schedule service install` 可迁移。"
        ))
    } else {
        Ok(format!("未安装或未运行\n{stdout}{stderr}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_known_vectors() {
        // FNV-1a 64 标准测试向量
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x85944171f73967e8);
    }

    #[cfg(windows)]
    #[test]
    fn task_name_is_deterministic() {
        // 同一目录的不同写法（大小写/分隔符/尾分隔符）得到相同任务名
        assert_eq!(
            task_name(std::path::Path::new(r"C:\Dev\Projects\wan")),
            task_name(std::path::Path::new(r"c:/dev/projects/wan/"))
        );
        // 不同目录得到不同任务名
        assert_ne!(
            task_name(std::path::Path::new(r"C:\Dev\Projects\wan")),
            task_name(std::path::Path::new(r"C:\Dev\Projects\other"))
        );
        // 格式：WanSchedule-<8 位 hex>
        let name = task_name(std::path::Path::new(r"C:\Dev\Projects\wan"));
        assert!(name.starts_with("WanSchedule-"));
        assert!(name["WanSchedule-".len()..]
            .chars()
            .all(|c| c.is_ascii_hexdigit()));
        assert_eq!(name.len(), "WanSchedule-".len() + 8);
    }

    #[cfg(windows)]
    #[test]
    fn shim_path_finds_adjacent_shim() {
        // shim_path 应在 exe 同目录下查找 wan-shim.exe
        let exe = self_exe().unwrap();
        let dir = exe.parent().unwrap();
        let expected = dir.join("wan-shim.exe");
        match shim_path() {
            Ok(p) => assert_eq!(p, expected),
            Err(_) => assert!(!expected.exists(), "shim 存在但 shim_path 返回 Err"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn unit_name_is_deterministic() {
        // 尾分隔符不影响
        assert_eq!(
            unit_name(Path::new("/home/u/wan")),
            unit_name(Path::new("/home/u/wan/"))
        );
        // 不同目录得到不同 unit 名
        assert_ne!(
            unit_name(Path::new("/home/u/wan")),
            unit_name(Path::new("/home/u/other"))
        );
        // 格式：wan-schedule-<8 位 hex>
        let name = unit_name(Path::new("/home/u/wan"));
        assert!(name.starts_with("wan-schedule-"));
        assert!(name["wan-schedule-".len()..]
            .chars()
            .all(|c| c.is_ascii_hexdigit()));
        assert_eq!(name.len(), "wan-schedule-".len() + 8);
    }
}
