//! 系统服务集成（spec §4.7 F-CRON-5，§15.5）
//!
//! Windows: schtasks 定时任务（每分钟启动一次 `wan schedule run-once`）
//! Linux: systemd user unit（wan-schedule.service + wan-schedule.timer）
//!
//! 设计决策：
//! - 不实现 Windows Service（ServiceMain 入口）——wan 是 CLI 工具，schtasks 更合适
//! - schtasks 每分钟触发一次 `wan schedule run-once`，由 wan 内部判断哪些调度到点
//! - Linux 用 systemd user unit（不需要 root），timer 每分钟触发

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// wan 自身可执行文件路径
fn self_exe() -> Result<PathBuf> {
    std::env::current_exe().map_err(|e| Error::io(format!("无法获取自身路径：{e}")))
}

/// Windows: schtasks 任务名
fn task_name() -> String {
    "WanSchedule".to_string()
}

/// Linux: systemd unit 名称
#[allow(dead_code)]
fn unit_name() -> String {
    "wan-schedule".to_string()
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
pub fn remove() -> Result<()> {
    #[cfg(windows)]
    {
        remove_windows()
    }
    #[cfg(unix)]
    {
        remove_linux()
    }
    #[cfg(not(any(windows, unix)))]
    {
        Err(Error::config("当前平台不支持服务集成"))
    }
}

/// 查看服务状态
pub fn status() -> Result<String> {
    #[cfg(windows)]
    {
        status_windows()
    }
    #[cfg(unix)]
    {
        status_linux()
    }
    #[cfg(not(any(windows, unix)))]
    {
        Err(Error::config("当前平台不支持服务集成"))
    }
}

// ── Windows: schtasks ──

#[cfg(windows)]
fn install_windows(base: &Path) -> Result<()> {
    let exe = self_exe()?;
    let base_str = base.to_string_lossy();

    // 创建 wrapper bat 文件（schtasks /TR 对含空格和参数的命令处理不可靠）
    let bat_path = crate::schedule::schedules_dir(base).join("run-once.bat");
    let bat_content = format!(
        "@echo off\r\n\"{}\" schedule run-once -C \"{}\"\r\n",
        exe.display(),
        base_str
    );
    std::fs::create_dir_all(crate::schedule::schedules_dir(base))?;
    std::fs::write(&bat_path, bat_content)?;

    // schtasks /Create /TN "WanSchedule" /TR "<bat path>" /SC MINUTE /MO 1 /F
    let output = std::process::Command::new("schtasks")
        .args([
            "/Create",
            "/TN", &task_name(),
            "/TR", &bat_path.to_string_lossy(),
            "/SC", "MINUTE",
            "/MO", "1",
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

    Ok(())
}

#[cfg(windows)]
fn remove_windows() -> Result<()> {
    let output = std::process::Command::new("schtasks")
        .args(["/Delete", "/TN", &task_name(), "/F"])
        .output()
        .map_err(|e| Error::io(format!("执行 schtasks 失败：{e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // 任务不存在不算错误
        if stderr.contains("无法找到") || stderr.contains("does not exist") || stderr.contains("cannot find") {
            return Ok(());
        }
        return Err(Error::config(format!("schtasks 删除失败：{stderr}")));
    }

    Ok(())
}

#[cfg(windows)]
fn status_windows() -> Result<String> {
    let output = std::process::Command::new("schtasks")
        .args(["/Query", "/TN", &task_name(), "/FO", "LIST"])
        .output()
        .map_err(|e| Error::io(format!("执行 schtasks 失败：{e}")))?;

    if !output.status.success() {
        return Ok("未安装（schtasks 未找到任务）".to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(stdout)
}

// ── Linux: systemd user unit ──

#[cfg(unix)]
fn systemd_user_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .map_err(|_| Error::config("无法获取 HOME 环境变量"))?;
    Ok(PathBuf::from(home).join(".config").join("systemd").join("user"))
}

#[cfg(unix)]
fn install_linux(base: &Path) -> Result<()> {
    let exe = self_exe()?;
    let exe_str = exe.to_string_lossy();
    let base_str = base.to_string_lossy();
    let dir = systemd_user_dir()?;
    std::fs::create_dir_all(&dir)?;

    let unit = unit_name();
    let service_path = dir.join(format!("{unit}.service"));
    let timer_path = dir.join(format!("{unit}.timer"));

    // service 文件
    let service_content = format!(
        "[Unit]\n\
         Description=wan schedule daemon\n\
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

    // systemctl --user enable --now wan-schedule.timer
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

    Ok(())
}

#[cfg(unix)]
fn remove_linux() -> Result<()> {
    let unit = unit_name();
    let dir = systemd_user_dir()?;

    // systemctl --user disable --now wan-schedule.timer
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

    Ok(())
}

#[cfg(unix)]
fn status_linux() -> Result<String> {
    let unit = unit_name();
    let output = std::process::Command::new("systemctl")
        .args(["--user", "status", &format!("{unit}.timer")])
        .output()
        .map_err(|e| Error::io(format!("执行 systemctl 失败：{e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if output.status.success() {
        Ok(stdout)
    } else {
        Ok(format!("未安装或未运行\n{stdout}{stderr}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_name_is_stable() {
        assert_eq!(task_name(), "WanSchedule");
    }

    #[test]
    fn unit_name_is_stable() {
        assert_eq!(unit_name(), "wan-schedule");
    }
}
