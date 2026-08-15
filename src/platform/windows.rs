//! Windows 进程树终止：Win32 Job Object（spec §15.1）
//! - 每个 step 子进程创建后立即 AssignProcessToJobObject
//! - Job 设 JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE：句柄关闭即整树终止（兜底）
//! - Ctrl+C：SetConsoleCtrlHandler 置位中断标志；执行器调用 TerminateJobObject

use std::io::{self, Read};
use std::mem::size_of;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::process::Command;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Console::{
    SetConsoleCP, SetConsoleCtrlHandler, SetConsoleOutputCP,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
    PROCESS_TERMINATE,
};

use super::{set_interrupted, Spawned, WaitResult};

pub struct StepProcess {
    proc_handle: OwnedHandle,
    job: OwnedHandle,
}

impl Drop for StepProcess {
    fn drop(&mut self) {
        // 关闭 Job Object 句柄 → KILL_ON_JOB_CLOSE 兜底终止整树
    }
}

fn last_os_error() -> io::Error {
    io::Error::last_os_error()
}

unsafe fn create_job() -> io::Result<OwnedHandle> {
    let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
    if job.is_null() {
        return Err(last_os_error());
    }
    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let ok = SetInformationJobObject(
        job,
        JobObjectExtendedLimitInformation,
        &info as *const _ as *const _,
        size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
    );
    if ok == 0 {
        CloseHandle(job);
        return Err(last_os_error());
    }
    Ok(OwnedHandle::from_raw_handle(job as *mut _))
}

pub fn spawn(mut cmd: Command) -> io::Result<Spawned> {
    let job = unsafe { create_job() }?;
    let mut child = cmd.spawn()?;
    let pid = child.id();
    let stdout = child
        .stdout
        .take()
        .map(|s| -> Box<dyn Read + Send> { Box::new(s) });
    let stderr = child
        .stderr
        .take()
        .map(|s| -> Box<dyn Read + Send> { Box::new(s) });

    let proc_handle = unsafe {
        let h = OpenProcess(
            PROCESS_SET_QUOTA | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        );
        if h.is_null() {
            return Err(last_os_error());
        }
        let ok = AssignProcessToJobObject(job.as_raw_handle() as HANDLE, h);
        if ok == 0 {
            CloseHandle(h);
            return Err(last_os_error());
        }
        CloseHandle(h);
        let h2 = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h2.is_null() {
            return Err(last_os_error());
        }
        OwnedHandle::from_raw_handle(h2 as *mut _)
    };

    // 理论竞态：子进程在赋值完成前派生孙进程会逃逸（shell 初始化耗时毫秒级，窗口极小；
    // KILL_ON_JOB_CLOSE 兜底覆盖已赋值部分）。W3 实测如发现孤儿再升级为 CREATE_SUSPENDED。
    Ok(Spawned {
        stdout,
        stderr,
        process: StepProcess { proc_handle, job },
    })
}

pub fn wait(proc: &mut StepProcess, timeout: Duration) -> io::Result<WaitResult> {
    let deadline = Instant::now() + timeout;
    loop {
        let mut code: u32 = 0;
        let ok =
            unsafe { GetExitCodeProcess(proc.proc_handle.as_raw_handle() as HANDLE, &mut code) };
        if ok == 0 {
            return Err(last_os_error());
        }
        // 259 = STILL_ACTIVE（0.61 未导出该常量，用字面量）
        if code != 259 {
            return Ok(WaitResult::Exited(code));
        }
        if Instant::now() >= deadline {
            return Ok(WaitResult::TimedOut);
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

impl StepProcess {
    pub fn kill_terminate(&self) {
        unsafe { TerminateJobObject(self.job.as_raw_handle() as HANDLE, 1) };
    }

    pub fn kill_force(&self) {
        // Windows 上 Job Object 即整树终止，无第二档
        self.kill_terminate();
    }
}

pub fn install_interrupt_handler() {
    unsafe extern "system" fn ctrl_handler(_: u32) -> i32 {
        set_interrupted(true);
        1 // 已处理：抑制默认终止，由执行器优雅清理后退出 130
    }
    unsafe {
        SetConsoleCtrlHandler(Some(ctrl_handler), 1);
    }
}

pub fn setup_utf8_console() {
    unsafe {
        SetConsoleOutputCP(65001);
        SetConsoleCP(65001);
    }
}
