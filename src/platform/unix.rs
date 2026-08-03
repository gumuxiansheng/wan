//! Unix 进程树终止：setpgid + killpg（spec §15.1）
//! - 子进程经 CommandExt::process_group(0) 自成进程组（pgid = pid）
//! - 中断/超时：killpg(SIGTERM) → 等待 → 残留则 killpg(SIGKILL)
//! - 子进程本身由 std::process::Child 回收，避免僵尸

use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use super::{set_interrupted, Spawned, WaitResult};

pub struct StepProcess {
    child: Option<Child>,
    pgid: i32,
    reaped: Option<u32>,
}

pub fn spawn(mut cmd: Command) -> io::Result<Spawned> {
    cmd.process_group(0);
    let mut child = cmd.spawn()?;
    let pgid = child.id() as i32;
    let stdout = child.stdout.take().map(|s| -> Box<dyn Read + Send> { Box::new(s) });
    let stderr = child.stderr.take().map(|s| -> Box<dyn Read + Send> { Box::new(s) });
    Ok(Spawned {
        stdout,
        stderr,
        process: StepProcess { child: Some(child), pgid, reaped: None },
    })
}

pub fn wait(proc: &mut StepProcess, timeout: Duration) -> io::Result<WaitResult> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(code) = proc.reaped {
            return Ok(WaitResult::Exited(code));
        }
        if let Some(child) = proc.child.as_mut() {
            match child.try_wait()? {
                Some(status) => {
                    let code = status.code().unwrap_or(1) as u32;
                    proc.reaped = Some(code);
                    proc.child = None;
                    return Ok(WaitResult::Exited(code));
                }
                None => {}
            }
        }
        if Instant::now() >= deadline {
            return Ok(WaitResult::TimedOut);
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

impl StepProcess {
    pub fn kill_graceful(&self) {
        // 仅当组长进程还活着才 killpg：防止 pgid 被回收后误杀其他进程组
        if self.child.is_some() {
            unsafe { libc::killpg(self.pgid, libc::SIGTERM) };
        }
    }

    pub fn kill_force(&self) {
        if self.child.is_some() {
            unsafe { libc::killpg(self.pgid, libc::SIGKILL) };
        }
    }
}

pub fn install_interrupt_handler() {
    unsafe extern "C" fn handler(_: libc::c_int) {
        set_interrupted();
    }
    let h = handler as *const () as libc::sighandler_t;
    unsafe {
        libc::signal(libc::SIGINT, h);
        libc::signal(libc::SIGTERM, h);
    }
}

pub fn setup_utf8_console() {}
