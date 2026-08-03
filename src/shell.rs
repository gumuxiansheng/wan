use std::path::Path;
use std::process::Command;

use crate::error::{Error, Result};
use crate::model::Shell;

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
            c.args(["-NoProfile", "-NonInteractive", "-File"]).arg(script);
            c
        }
        Shell::Cmd => {
            let mut c = Command::new("cmd");
            c.args(["/d", "/s", "/c"]).arg(script);
            c
        }
        Shell::Bash => {
            let mut c = Command::new("bash");
            c.args(["--noprofile", "--norc", "-e", "-o", "pipefail"]).arg(script);
            c
        }
        Shell::Sh => {
            let mut c = Command::new("sh");
            c.args(["-e"]).arg(script);
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
}
