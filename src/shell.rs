use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use crate::error::{Error, Result};
use crate::model::Shell;

/// 脚本路径参数。Windows 上 Git Bash 的 bash/sh 不识别反斜杠路径
/// （`\U`、`\A` 等被当作转义吞掉 → 路径损坏），需转为正斜杠形式
/// `C:/...`；MSYS 运行时负责还原为 Windows 路径。其余平台原样传递。
#[cfg(windows)]
fn script_arg(script: &Path) -> OsString {
    OsString::from(script.to_string_lossy().replace('\\', "/"))
}

#[cfg(not(windows))]
fn script_arg(script: &Path) -> OsString {
    script.as_os_str().to_owned()
}

#[cfg(windows)]
fn is_wsl_shim(dir: &Path) -> bool {
    dir.to_string_lossy()
        .eq_ignore_ascii_case(r"C:\Windows\System32")
}

/// 解析 bash/sh 可执行文件。
///
/// Windows 特有问题：CreateProcess 的搜索顺序是「应用程序目录 → 当前目录 →
/// system32 → Windows 目录 → PATH」。system32 排在 PATH 之前，裸 `bash`
/// 会先命中 WSL 的 shim（`C:\Windows\System32\bash.exe`），其 /bin/bash
/// 运行在 Linux 文件系统里，读不到任何 Windows 路径 → `wan run` 必然失败。
/// 因此显式解析 Git Bash；解析不到时回落裸名（spawn_hint 给出安装指引）。
#[cfg(windows)]
fn resolve_program(name: &str) -> OsString {
    let exe = format!("{name}.exe");
    for base in [r"C:\Program Files\Git", r"C:\Program Files (x86)\Git"] {
        for sub in ["bin", r"usr\bin"] {
            let p = Path::new(base).join(sub).join(&exe);
            if p.is_file() {
                return p.into_os_string();
            }
        }
    }
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let p = dir.join(&exe);
        if p.is_file() && !(name == "bash" && is_wsl_shim(&dir)) {
            return p.into_os_string();
        }
    }
    OsString::from(name)
}

#[cfg(not(windows))]
fn resolve_program(name: &str) -> OsString {
    OsString::from(name)
}

pub fn parse(s: &str) -> Result<Shell> {
    match s {
        "pwsh" => Ok(Shell::Pwsh),
        "cmd" => Ok(Shell::Cmd),
        "bash" => Ok(Shell::Bash),
        "sh" => Ok(Shell::Sh),
        other => Err(Error::config(format!(
            "shell 必须显式指定为 pwsh / cmd / bash / sh 之一，收到：`{other}`"
        ))),
    }
}

pub fn supported_on_platform(shell: Shell) -> Result<()> {
    match shell {
        Shell::Cmd if !cfg!(windows) => Err(Error::config(
            "shell: cmd 仅 Windows 支持（Windows 专用），当前平台不可用",
        )),
        Shell::Bash | Shell::Sh if cfg!(windows) => Ok(()),
        _ => Ok(()),
    }
}

pub fn script_extension(shell: Shell) -> &'static str {
    match shell {
        Shell::Pwsh => "ps1",
        Shell::Cmd => "cmd",
        Shell::Bash | Shell::Sh => "sh",
    }
}

pub fn script_prelude(shell: Shell) -> &'static str {
    match shell {
        // 强制 UTF-8 code page（F-OUT-3），cmd 默认 OEM/ANSI 会乱码
        Shell::Cmd => "@chcp 65001 >nul\r\n",
        _ => "",
    }
}

/// 构建 shell 进程命令。脚本统一走临时文件（spec §14.3）。
pub fn build_command(shell: Shell, script: &Path) -> Command {
    match shell {
        Shell::Pwsh => {
            let mut c = Command::new("pwsh");
            c.args(["-NoProfile", "-NonInteractive", "-File"])
                .arg(script);
            c
        }
        Shell::Cmd => {
            let mut c = Command::new("cmd");
            c.args(["/d", "/s", "/c"]).arg(script);
            c
        }
        Shell::Bash => {
            let mut c = Command::new(resolve_program("bash"));
            c.args(["--noprofile", "--norc", "-e", "-o", "pipefail"])
                .arg(script_arg(script));
            c
        }
        Shell::Sh => {
            let mut c = Command::new(resolve_program("sh"));
            c.args(["-e"]).arg(script_arg(script));
            c
        }
    }
}

/// spawn 失败时给出可操作的提示（spec §15.2：不静默回落）
pub fn spawn_hint(shell: Shell) -> &'static str {
    match shell {
        Shell::Pwsh => "未找到 pwsh：请安装 PowerShell 7+（winget install Microsoft.PowerShell）或确认其已在 PATH",
        Shell::Cmd => "未找到 cmd",
        Shell::Bash => "未找到 bash：Windows 上请安装 Git Bash，或使用 shell: pwsh",
        Shell::Sh => "未找到 sh",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_shells() {
        assert_eq!(parse("pwsh").unwrap(), Shell::Pwsh);
        assert_eq!(parse("cmd").unwrap(), Shell::Cmd);
        assert_eq!(parse("bash").unwrap(), Shell::Bash);
        assert_eq!(parse("sh").unwrap(), Shell::Sh);
        assert!(parse("powershell").is_err());
        assert!(parse("zsh").is_err());
    }

    #[test]
    fn extensions() {
        assert_eq!(script_extension(Shell::Pwsh), "ps1");
        assert_eq!(script_extension(Shell::Cmd), "cmd");
        assert_eq!(script_extension(Shell::Bash), "sh");
    }

    #[cfg(windows)]
    #[test]
    fn script_arg_windows() {
        // Git Bash 吞反斜杠回归测试：路径必须转为正斜杠
        let p = Path::new(r"C:\Users\win11\AppData\Local\Temp\wan-x\step-0-0.sh");
        assert_eq!(
            script_arg(p),
            r"C:/Users/win11/AppData/Local/Temp/wan-x/step-0-0.sh"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn script_arg_unix() {
        let p = Path::new("/tmp/wan-x/step-0-0.sh");
        assert_eq!(script_arg(p), "/tmp/wan-x/step-0-0.sh");
    }

    #[cfg(windows)]
    #[test]
    fn resolve_program_finds_git_bash() {
        // 必须解析到真实存在的 bash（CI/本机均有 Git Bash），且不得命中
        // system32 的 WSL shim —— 那是 CreateProcess 裸名解析的默认结果
        let binding = resolve_program("bash");
        let resolved = Path::new(&binding);
        assert!(resolved.is_file(), "resolved {resolved:?} must exist");
        assert!(
            !resolved
                .to_string_lossy()
                .to_ascii_lowercase()
                .contains(r"windows\system32"),
            "must not resolve to WSL shim, got {resolved:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn is_wsl_shim_detects_system32() {
        assert!(is_wsl_shim(Path::new(r"C:\Windows\System32")));
        assert!(is_wsl_shim(Path::new(r"C:\WINDOWS\system32")));
        assert!(!is_wsl_shim(Path::new(r"C:\Program Files\Git\bin")));
        assert!(!is_wsl_shim(Path::new(r"C:\Program Files\Git\usr\bin")));
    }
}
