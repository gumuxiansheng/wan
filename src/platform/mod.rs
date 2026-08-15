use std::io::{self, Read};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use self::unix as imp;
#[cfg(windows)]
use self::windows as imp;

pub use imp::StepProcess;

pub enum WaitResult {
    Exited(u32),
    TimedOut,
}

pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::Relaxed)
}

pub fn set_interrupted(value: bool) {
    INTERRUPTED.store(value, Ordering::Relaxed);
}

pub struct Spawned {
    pub stdout: Option<Box<dyn Read + Send>>,
    pub stderr: Option<Box<dyn Read + Send>>,
    pub process: StepProcess,
}

pub fn spawn(cmd: Command) -> io::Result<Spawned> {
    imp::spawn(cmd)
}

/// 安装 Ctrl+C / SIGINT / SIGTERM 处理器（幂等）
pub fn install_interrupt_handler() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(imp::install_interrupt_handler);
}

/// Windows：启动时设置控制台 code page 为 UTF-8（F-OUT-3）；其他平台 no-op
pub fn setup_utf8_console() {
    imp::setup_utf8_console();
}

pub enum KillKind {
    /// SIGTERM（Unix）/ TerminateJobObject（Windows）
    /// Windows 上无优雅终止原语，TerminateJobObject 即为最佳可用方案
    Terminate,
    /// SIGKILL 兜底（Unix）；Windows 上与 Terminate 相同
    Kill,
}

impl StepProcess {
    /// 终止整个子进程树
    pub fn kill_tree(&self, kind: KillKind) {
        match kind {
            KillKind::Terminate => self.kill_terminate(),
            KillKind::Kill => self.kill_force(),
        }
    }

    /// 等待进程退出（超时则返回 TimedOut；被 kill 后继续调用直至 Exited）
    pub fn wait(&mut self, timeout: Duration) -> io::Result<WaitResult> {
        imp::wait(self, timeout)
    }
}
