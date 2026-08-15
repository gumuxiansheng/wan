//! step 执行器：临时脚本派生 + 流式日志 + 超时 + 重试 + $WAN_OUTPUT（spec §14.3 / §6.5）

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::error::Result;
use crate::expr::interpolate;
use crate::model::{
    merge_env, now_rfc3339, EnvMap, Event, Job, Outcome, RunOptions, Shell, Stream,
};
use crate::platform::{self, KillKind, Spawned, WaitResult};
use crate::shell;

pub const CODE_TIMEOUT: u32 = 124;
pub const CODE_INTERRUPT: u32 = 130;

#[derive(Clone)]
pub struct RunCtx {
    pub opts: RunOptions,
    pub tmp_root: PathBuf,
    /// workflow 级 working-directory（§6.3），CLI -C 未指定时生效
    pub workflow_wd: Option<PathBuf>,
}

pub struct JobResult {
    pub outcome: Outcome,
    pub code: u32,
    pub duration_ms: u64,
    pub interrupted: bool,
}

fn step_name(step: &crate::model::Step, _idx: usize) -> String {
    match &step.name {
        Some(n) => n.clone(),
        None => step
            .run
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string()
            .chars()
            .take(40)
            .collect(),
    }
}

/// 解析 $WAN_OUTPUT 文件（§6.5）：key=value 行，\r\n 剥离，# 开头忽略
fn parse_output_file(path: &Path) -> Vec<(String, String)> {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line.split_once('=') {
            Some((k, v)) => {
                let k = k.trim();
                if !k.is_empty() {
                    out.push((k.to_string(), v.to_string()));
                }
            }
            None => warn(&format!("$WAN_OUTPUT 行格式非法，忽略：`{line}`")),
        }
    }
    out
}

fn warn(msg: &str) {
    eprintln!("warning: {msg}");
}

fn pump<R: std::io::BufRead + Send + 'static>(
    mut reader: R,
    tx: mpsc::Sender<(Stream, String)>,
    stream: Stream,
) {
    let mut buf = String::new();
    loop {
        buf.clear();
        match reader.read_line(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let line = buf.trim_end_matches(['\n', '\r']).to_string();
                if tx.send((stream, line)).is_err() {
                    break;
                }
            }
        }
    }
}

/// 睡眠并感知中断（重试 delay 期间 Ctrl+C 生效）
fn interruptible_sleep(dur: Duration) -> bool {
    let mut remain = dur.as_millis() as u64;
    while remain > 0 {
        if platform::interrupted() {
            return true;
        }
        let chunk = remain.min(100);
        std::thread::sleep(Duration::from_millis(chunk));
        remain -= chunk;
    }
    platform::interrupted()
}

fn write_script(path: &Path, shell_kind: Shell, run: &str) -> std::io::Result<()> {
    let mut content = String::new();
    content.push_str(shell::script_prelude(shell_kind));
    if shell_kind == Shell::Cmd {
        content.push_str(&run.replace('\n', "\r\n"));
    } else {
        content.push_str(run);
    }
    std::fs::write(path, content)
}

struct AttemptOutcome {
    code: u32,
    timed_out: bool,
    interrupted: bool,
}

struct StepCtx {
    job: String,
    name: String,
    tmp_root: PathBuf,
    job_idx: usize,
    step_idx: usize,
}

fn spawn_and_wait(
    cmd: Command,
    deadline: Option<Instant>,
    ctx: &StepCtx,
    ev_tx: &std::sync::mpsc::Sender<Event>,
) -> std::io::Result<AttemptOutcome> {
    let mut spawned: Spawned = platform::spawn(cmd)?;
    let (tx, rx) = mpsc::channel::<(Stream, String)>();
    if let Some(s) = spawned.stdout.take() {
        let tx2 = tx.clone();
        std::thread::spawn(move || pump(std::io::BufReader::new(s), tx2, Stream::Stdout));
    }
    if let Some(s) = spawned.stderr.take() {
        let tx3 = tx.clone();
        std::thread::spawn(move || pump(std::io::BufReader::new(s), tx3, Stream::Stderr));
    }
    drop(tx);

    let mut timed_out = false;
    let mut interrupted = false;
    let code = loop {
        if platform::interrupted() {
            interrupted = true;
            spawned.process.kill_tree(KillKind::Terminate);
            let _ = wait_exited(&mut spawned);
            break CODE_INTERRUPT;
        }
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok((stream, line)) => {
                let _ = ev_tx.send(Event::StepOutput {
                    job: ctx.job.clone(),
                    step: ctx.name.clone(),
                    stream,
                    line,
                });
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                match spawned.process.wait(Duration::from_millis(20)) {
                    Ok(WaitResult::Exited(code)) => break code,
                    Ok(WaitResult::TimedOut) => {
                        if let Some(dl) = deadline {
                            if Instant::now() >= dl {
                                timed_out = true;
                                spawned.process.kill_tree(KillKind::Terminate);
                                let _ = wait_exited(&mut spawned);
                                break CODE_TIMEOUT;
                            }
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                match spawned.process.wait(Duration::from_millis(20)) {
                    Ok(WaitResult::Exited(code)) => break code,
                    Ok(WaitResult::TimedOut) => {}
                    Err(e) => return Err(e),
                }
            }
        }
    };

    while let Ok((stream, line)) = rx.try_recv() {
        let _ = ev_tx.send(Event::StepOutput {
            job: ctx.job.clone(),
            step: ctx.name.clone(),
            stream,
            line,
        });
    }
    Ok(AttemptOutcome {
        code,
        timed_out,
        interrupted,
    })
}

fn wait_exited(spawned: &mut Spawned) -> std::io::Result<u32> {
    let mut grace = Duration::from_secs(5);
    loop {
        match spawned.process.wait(Duration::from_millis(50)) {
            Ok(WaitResult::Exited(code)) => return Ok(code),
            Ok(WaitResult::TimedOut) => {
                if grace.is_zero() {
                    spawned.process.kill_tree(KillKind::Kill);
                    grace = Duration::from_secs(2);
                } else {
                    grace = grace.saturating_sub(Duration::from_millis(50));
                }
            }
            Err(e) => return Err(e),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_step(
    step: &crate::model::Step,
    ctx: &StepCtx,
    env_raw: &EnvMap,
    base_dir: &Path,
    job_deadline: Option<Instant>,
    tx: &std::sync::mpsc::Sender<Event>,
    injected: &mut EnvMap,
    prev_outcomes: &[Outcome],
) -> (Outcome, u32) {
    // if 求值（§6.2）：无 if 恒执行；有 if 按先前 step 结算状态求值，为假则跳过
    if let Some(e) = &step.if_condition {
        let eval_ctx = crate::expr::EvalCtx::new(prev_outcomes, env_raw);
        if !e.eval(&eval_ctx) {
            return (Outcome::Skipped, 0);
        }
    }

    // 插值（§6.4）：env 值 / run / working-directory，单遍非递归
    let mut step_env = EnvMap::new();
    for (k, v) in env_raw {
        let (iv, ws) = interpolate(v, env_raw);
        for w in ws {
            warn(&w);
        }
        step_env.push((k.clone(), iv));
    }
    let (run, ws) = interpolate(&step.run, env_raw);
    for w in ws {
        warn(&w);
    }
    let working_directory = match &step.working_directory {
        Some(p) => {
            let (s, ws) = interpolate(&p.to_string_lossy(), env_raw);
            for w in ws {
                warn(&w);
            }
            let pb = PathBuf::from(s);
            if pb.is_absolute() {
                pb
            } else {
                base_dir.join(pb)
            }
        }
        None => base_dir.to_path_buf(),
    };

    // 时间盒：step 超时 ∩ job 剩余（job 超时优先，§6.3）
    let mut deadline: Option<Instant> = step
        .timeout_minutes
        .map(|t| Instant::now() + Duration::from_secs(t as u64 * 60));
    if let Some(jd) = job_deadline {
        if Instant::now() >= jd {
            let _ = tx.send(Event::StepEnd {
                job: ctx.job.clone(),
                step: ctx.name.clone(),
                code: CODE_TIMEOUT,
                duration_ms: 0,
            });
            return (Outcome::Failure, CODE_TIMEOUT);
        }
        let job_remain = jd.saturating_duration_since(Instant::now());
        deadline = Some(match deadline {
            Some(d) => d.min(Instant::now() + job_remain),
            None => Instant::now() + job_remain,
        });
    }

    // 临时脚本（§14.3：统一写文件，失败保留供排查）
    let ext = shell::script_extension(step.shell);
    let script = ctx
        .tmp_root
        .join(format!("step-{}-{}.{}", ctx.job_idx, ctx.step_idx, ext));
    if let Err(e) = write_script(&script, step.shell, &run) {
        warn(&format!("写入临时脚本失败：{e}"));
        return (Outcome::Failure, 1);
    }
    let out_file = ctx
        .tmp_root
        .join(format!("out-{}-{}.txt", ctx.job_idx, ctx.step_idx));

    let _ = tx.send(Event::StepStart {
        job: ctx.job.clone(),
        step: ctx.name.clone(),
        ts: now_rfc3339(),
    });

    let attempts = step.retry.map(|r| r.attempts).unwrap_or(1);
    let delay = step.retry.map(|r| r.delay).unwrap_or(Duration::ZERO);
    let mut final_code: u32 = 0;
    let mut timed_out = false;
    let mut interrupted = false;
    let start = Instant::now();

    for attempt in 0..attempts {
        if attempt > 0 {
            warn(&format!(
                "step `{}` 失败 (code {final_code})，重试 {}/{}，等待 {}s",
                ctx.name,
                attempt,
                attempts - 1,
                delay.as_secs_f64()
            ));
            if interruptible_sleep(delay) {
                interrupted = true;
                break;
            }
        }

        let mut cmd = shell::build_command(step.shell, &script);
        cmd.current_dir(&working_directory);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        // 内置变量优先于用户 env（§6.5）
        cmd.env("WAN_OUTPUT", &out_file);
        cmd.env("PYTHONUTF8", "1");
        cmd.env("PYTHONIOENCODING", "utf-8");
        for (k, v) in &step_env {
            cmd.env(k, v);
        }

        let outcome = match spawn_and_wait(cmd, deadline, ctx, tx) {
            Ok(o) => o,
            Err(e) => {
                // spawn 失败（shell 不在 PATH）→ 安装提示，不静默回落（§15.2）
                warn(&format!(
                    "启动 shell `{}` 失败：{e}。{}",
                    step.shell.as_str(),
                    shell::spawn_hint(step.shell)
                ));
                final_code = 1;
                interrupted = true;
                break;
            }
        };
        final_code = outcome.code;
        timed_out = outcome.timed_out;
        interrupted = outcome.interrupted;
        if final_code == 0 || timed_out || interrupted {
            break;
        }
    }

    if final_code == 0 {
        let _ = std::fs::remove_file(&script);
    }

    let _ = tx.send(Event::StepEnd {
        job: ctx.job.clone(),
        step: ctx.name.clone(),
        code: final_code,
        duration_ms: start.elapsed().as_millis() as u64,
    });

    if interrupted {
        return (Outcome::Failure, final_code);
    }
    if timed_out {
        return (Outcome::Failure, CODE_TIMEOUT);
    }
    if final_code != 0 {
        return (Outcome::Failure, final_code);
    }
    // 成功 → 读回 $WAN_OUTPUT 注入后续 step（§6.5，仅成功读回）
    for (k, v) in parse_output_file(&out_file) {
        match injected.iter_mut().find(|(ik, _)| ik == &k) {
            Some(slot) => slot.1 = v,
            None => injected.push((k, v)),
        }
    }
    (Outcome::Success, 0)
}

pub fn run_job(
    job: &Job,
    wf_env: &EnvMap,
    ctx: &RunCtx,
    tx: &std::sync::mpsc::Sender<Event>,
    job_uid: usize,
) -> JobResult {
    let start = Instant::now();
    let mut injected: EnvMap = Vec::new();
    let mut prev_outcomes: Vec<Outcome> = Vec::new();
    let job_deadline = job
        .timeout_minutes
        .map(|t| Instant::now() + Duration::from_secs(t as u64 * 60));
    // 继承叠加（§6.3）：-C > workflow.working-directory > job.working-directory，
    // 相对路径按层级相对解析（绝对路径直接覆盖）
    let mut base_dir = ctx
        .opts
        .working_dir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    if let Some(wd) = &ctx.workflow_wd {
        base_dir = if wd.is_absolute() {
            wd.clone()
        } else {
            base_dir.join(wd)
        };
    }
    if let Some(wd) = &job.working_directory {
        base_dir = if wd.is_absolute() {
            wd.clone()
        } else {
            base_dir.join(wd)
        };
    }

    let mut job_code: u32 = 0;
    let mut outcome = Outcome::Success;
    let mut interrupted = false;

    for (idx, step) in job.steps.iter().enumerate() {
        if platform::interrupted() {
            interrupted = true;
            break;
        }
        let name = step_name(step, idx);
        let step_ctx = StepCtx {
            job: job.id.clone(),
            name: name.clone(),
            tmp_root: ctx.tmp_root.clone(),
            job_idx: job_uid,
            step_idx: idx,
        };
        let env_raw = merge_env(&[wf_env, &job.env, &injected, &step.env]);
        let (step_outcome, step_code) = run_step(
            step,
            &step_ctx,
            &env_raw,
            &base_dir,
            job_deadline,
            tx,
            &mut injected,
            &prev_outcomes,
        );
        prev_outcomes.push(step_outcome);
        match step_outcome {
            Outcome::Skipped => {}
            Outcome::Success => {}
            Outcome::Failure => {
                if step.continue_on_error {
                    warn(&format!(
                        "step `{name}` 失败但 continue-on-error 已设置，job 继续"
                    ));
                } else {
                    job_code = step_code;
                    outcome = Outcome::Failure;
                    break;
                }
            }
        }
        if interrupted {
            break;
        }
    }

    JobResult {
        outcome,
        code: if outcome == Outcome::Failure {
            job_code
        } else {
            0
        },
        duration_ms: start.elapsed().as_millis() as u64,
        interrupted,
    }
}

/// 供 engine 使用：加载时校验平台 shell 支持（cmd 仅 Windows，§15.2）
pub fn check_platform_shells(job: &Job) -> Result<()> {
    for step in &job.steps {
        shell::supported_on_platform(step.shell)?;
    }
    Ok(())
}
