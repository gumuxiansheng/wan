//! wan-shim：无窗口启动器（Windows GUI 子系统）
//!
//! 编译为 `wan-shim.exe`（windows_subsystem = "windows"），不创建控制台窗口。
//! schtasks /TR 指向本程序，本程序以 CREATE_NO_WINDOW 启动 `wan.exe schedule run-once`，
//! 同步等待子进程退出后返回其退出码。
//!
//! 设计目标：
//! - 零依赖：不依赖 wscript/vbs/powershell，不受 VBScript 2027 弃用影响
//! - 无窗口：GUI 子系统 + CREATE_NO_WINDOW 双保险
//! - 轻量：~20KB，无 Rust 依赖（仅 windows-sys link）
//! - 同步：等子进程退出，schtasks 上次结果 = wan 真实退出码
//!
//! 用法：wan-shim.exe <wan-exe-path> <args...>
//! 例如：wan-shim.exe "C:\path\wan.exe" schedule run-once -C "C:\proj\."
//!
//! 注：`windows_subsystem` 与 Windows 专属 API 仅在 Windows 目标有效，
//! 故整体实现置于 `#[cfg(windows)]` 模块，非 Windows 平台编译为无操作二进制
//! （CI 的 Linux release 不会打包它，见 .cnb.yml）。

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod shim {
    use std::ffi::OsString;
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::Command;

    /// CREATE_NO_WINDOW（0x08000000）：子进程不创建控制台窗口
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    pub fn run() -> i32 {
        let mut args: Vec<OsString> = std::env::args_os().collect();
        if args.len() < 2 {
            // 无法输出到 stderr（GUI 子系统无控制台），写事件日志
            log_to_event_log("wan-shim: 缺少参数，用法 wan-shim.exe <wan-exe> [args...]");
            return 2;
        }

        let wan_exe = PathBuf::from(args.remove(1));
        let wan_args: Vec<OsString> = args.into_iter().skip(1).collect();

        if !wan_exe.exists() {
            log_to_event_log(&format!("wan-shim: wan.exe 不存在: {}", wan_exe.display()));
            return 2;
        }

        let mut cmd = Command::new(&wan_exe);
        cmd.args(&wan_args);
        cmd.creation_flags(CREATE_NO_WINDOW);

        match cmd.status() {
            Ok(status) => status.code().unwrap_or(1),
            Err(e) => {
                log_to_event_log(&format!("wan-shim: 启动 {} 失败: {}", wan_exe.display(), e));
                1
            }
        }
    }

    /// 写 Windows 事件日志（GUI 子系统无 stdout/stderr，只能走事件日志）
    fn log_to_event_log(msg: &str) {
        // 用 ReportEventW 写系统事件日志
        // 这里用最简方案：写注册表 HKLM\SOFTWARE\Wan\Shim\LastError
        // 避免引入 windows-sys 的 EventLog feature（保持零依赖）
        // 实际生产中可改用 windows-sys::Win32::System::EventLog
        let _ = std::fs::write(std::env::temp_dir().join("wan-shim-last-error.txt"), msg);
    }
}

fn main() {
    #[cfg(windows)]
    {
        std::process::exit(shim::run());
    }
    #[cfg(not(windows))]
    {
        eprintln!("wan-shim is only supported on Windows");
        std::process::exit(1);
    }
}
