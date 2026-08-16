use std::collections::HashMap;
use std::io::Write;

use anstyle::{AnsiColor, Color, Style};

use crate::model::{Event, EventSink, RunOptions, Stream};

/// 人类可读输出（spec F-OUT-1 / F-REL-5）：
/// - 单 job 运行：行级实时流式输出；
/// - 多 job 并行：各 job 输出按归属缓冲，job 结束时整块输出（banner + 该 job 全部行），
///   避免并行输出互相穿插、截断多行报告（如 SqlGuard Report）。
///   `--no-group` 可恢复实时穿插的旧行为。
pub struct HumanSink {
    pub color: bool,
    pub quiet: bool,
    out: std::io::Stdout,
    /// --no-group 时恒为 false
    group_enabled: bool,
    /// 出现过 ≥2 job 并行后置位，整 run 保持分组（中途回退会再次穿插）
    grouped: bool,
    active_jobs: usize,
    /// 单 job 阶段短暂 pending 的 JobStart 行（等下一个事件判定是否并行）
    pending: Option<String>,
    /// 分组前已有实时输出的 job（flush 块标注「续」）
    live_jobs: Vec<String>,
    /// buffer 中 job 的出现顺序
    order: Vec<String>,
    buffers: HashMap<String, Vec<String>>,
}

impl HumanSink {
    pub fn new(opts: &RunOptions) -> Self {
        HumanSink {
            color: opts.color,
            quiet: opts.quiet,
            out: std::io::stdout(),
            group_enabled: !opts.no_group,
            grouped: false,
            active_jobs: 0,
            pending: None,
            live_jobs: Vec::new(),
            order: Vec::new(),
            buffers: HashMap::new(),
        }
    }

    fn paint(&self, s: &str, style: Style) -> String {
        if !self.color {
            return s.to_string();
        }
        format!("{}{}{}", style.render(), s, anstyle::Reset.render())
    }

    fn print(&mut self, line: String) {
        let mut out = self.out.lock();
        let _ = writeln!(out, "{line}");
    }

    fn buf_push(&mut self, job: &str, line: String) {
        if !self.buffers.contains_key(job) {
            self.order.push(job.to_string());
        }
        self.buffers.entry(job.to_string()).or_default().push(line);
    }

    /// 单 job 模式：先补出 pending 的 JobStart 行，再实时打印
    fn emit_live(&mut self, line: String) {
        if let Some(h) = self.pending.take() {
            self.print(h);
        }
        self.print(line);
    }

    fn banner(&self, job: &str) -> String {
        let cont = if self.live_jobs.iter().any(|j| j == job) {
            " (续)"
        } else {
            ""
        };
        let title = format!("── [job] {job}{cont} ");
        let pad = 60usize.saturating_sub(title.chars().count()).max(4);
        format!(
            "{}{}",
            self.paint(&title, JOB),
            self.paint(&"─".repeat(pad), DIM)
        )
    }

    /// 整块输出某 job 的全部缓冲行
    fn print_block(&mut self, job: &str, lines: Vec<String>) {
        if lines.is_empty() {
            return;
        }
        let banner = self.banner(job);
        self.print(banner);
        for l in lines {
            self.print(l);
        }
    }

    fn flush_job(&mut self, job: &str) {
        if let Some(lines) = self.buffers.remove(job) {
            self.order.retain(|j| j != job);
            self.print_block(job, lines);
        }
    }
}

const JOB: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
const STEP: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Blue)));
const OK: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Green)));
const FAIL: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Red)));
const WARN: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
const DIM: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::BrightBlack)));

impl EventSink for HumanSink {
    fn emit(&mut self, event: Event) {
        match event {
            Event::RunStart { workflow, .. } => {
                self.print(
                    self.paint(&format!("==> 开始运行 workflow: {workflow}"), JOB)
                        .to_string(),
                );
            }
            Event::JobStart { job, .. } => {
                self.active_jobs += 1;
                if self.active_jobs >= 2 && self.group_enabled {
                    self.grouped = true;
                }
                let header = self.paint(&format!("[job] {job}"), JOB).to_string();
                if self.grouped {
                    // banner 已含 job 名；pending 的 header 尚未打印，直接丢弃
                    self.pending = None;
                } else {
                    // 单 job：暂存 header，等下一个事件判定是否并行
                    if let Some(h) = self.pending.take() {
                        self.print(h);
                    }
                    self.pending = Some(header);
                }
            }
            Event::StepStart { job, step, .. } => {
                let line = format!(
                    "  {} {}",
                    self.paint(&format!("[{job}]"), JOB),
                    self.paint(&format!("[step] {step}"), STEP)
                );
                if self.grouped {
                    self.buf_push(&job, line);
                } else {
                    self.emit_live(line);
                }
            }
            Event::StepOutput {
                job,
                step,
                stream,
                line,
            } => {
                if self.quiet {
                    return;
                }
                let prefix = match stream {
                    Stream::Stdout => "",
                    Stream::Stderr => "err| ",
                };
                let line = format!("    {prefix}{line}");
                if self.grouped {
                    self.buf_push(&job, line);
                } else {
                    self.emit_live(line);
                }
                let _ = step;
            }
            Event::StepEnd {
                job,
                step,
                code,
                duration_ms,
                ..
            } => {
                let line = if code == 0 {
                    format!(
                        "    {} {} ({duration_ms} ms)",
                        self.paint(&format!("[{job}]"), JOB),
                        self.paint("OK", OK)
                    )
                } else {
                    format!(
                        "    {} {} (code {code}, {duration_ms} ms)",
                        self.paint(&format!("[{job}]"), JOB),
                        self.paint("FAIL", FAIL)
                    )
                };
                if self.grouped {
                    self.buf_push(&job, line);
                } else {
                    self.emit_live(line);
                }
                let _ = step;
            }
            Event::JobEnd {
                job,
                code,
                duration_ms,
                ..
            } => {
                self.active_jobs = self.active_jobs.saturating_sub(1);
                let status = if code == 0 {
                    self.paint("OK", OK)
                } else {
                    self.paint("FAIL", FAIL)
                };
                let line = format!("[job] {job} {status} ({duration_ms} ms)");
                if self.grouped {
                    self.buf_push(&job, line);
                    self.flush_job(&job);
                } else {
                    self.emit_live(line);
                }
            }
            Event::RunEnd {
                code, duration_ms, ..
            } => {
                // 防御：中断等路径残留的 job 缓冲一并整块输出
                let pending = self.pending.take();
                if let Some(h) = pending {
                    self.print(h);
                }
                for j in std::mem::take(&mut self.order) {
                    if let Some(lines) = self.buffers.remove(&j) {
                        self.print_block(&j, lines);
                    }
                }
                let status = if code == 0 {
                    self.paint("成功", OK)
                } else if code == 130 {
                    self.paint("被中断", WARN)
                } else {
                    self.paint("失败", FAIL)
                };
                self.print(format!(
                    "{} ({duration_ms} ms)",
                    self.paint(&format!("==> 结果: {status}"), DIM)
                ));
            }
        }
    }
}

pub struct JsonSink<W: Write> {
    out: W,
}

impl<W: Write> JsonSink<W> {
    pub fn new(out: W) -> Self {
        JsonSink { out }
    }
}

impl<W: Write> EventSink for JsonSink<W> {
    fn emit(&mut self, event: Event) {
        let _ = writeln!(
            self.out,
            "{}",
            serde_json::to_string(&event).unwrap_or_default()
        );
    }
}

/// 测试辅助：收集事件（集成测试用）
#[allow(dead_code)]
pub struct CapturingSink {
    pub events: Vec<Event>,
}

#[allow(dead_code)]
impl Default for CapturingSink {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)]
impl CapturingSink {
    pub fn new() -> CapturingSink {
        CapturingSink { events: Vec::new() }
    }
}

impl EventSink for CapturingSink {
    fn emit(&mut self, event: Event) {
        self.events.push(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Event, Stream};

    fn sink() -> HumanSink {
        HumanSink {
            color: false,
            quiet: false,
            out: std::io::stdout(),
            group_enabled: true,
            grouped: false,
            active_jobs: 0,
            pending: None,
            live_jobs: Vec::new(),
            order: Vec::new(),
            buffers: HashMap::new(),
        }
    }

    #[test]
    fn json_event_shape() {
        let ev = Event::StepOutput {
            job: "build".into(),
            step: "编译".into(),
            stream: Stream::Stdout,
            line: "hello".into(),
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert_eq!(
            s,
            r#"{"type":"step_output","job":"build","step":"编译","stream":"stdout","line":"hello"}"#
        );
    }

    /// 并行时输出按 job 整块归属：A 的行不被 B 的行穿插
    #[test]
    fn grouped_output_keeps_jobs_contiguous() {
        let mut s = sink();
        // 收集输出行：用缓冲换掉 stdout —— 通过拦截 buf_push/flush 顺序验证
        // 这里直接驱动事件序列（A/B 交替输出），验证 flush 顺序
        s.emit(Event::JobStart {
            job: "a".into(),
            ts: String::new(),
        });
        s.emit(Event::JobStart {
            job: "b".into(),
            ts: String::new(),
        });
        assert!(s.grouped, "两个 JobStart 后应进入分组模式");
        for line in ["a1", "a2", "a3"] {
            s.emit(Event::StepOutput {
                job: "a".into(),
                step: "s".into(),
                stream: Stream::Stdout,
                line: line.into(),
            });
        }
        for line in ["b1", "b2", "b3"] {
            s.emit(Event::StepOutput {
                job: "b".into(),
                step: "s".into(),
                stream: Stream::Stdout,
                line: line.into(),
            });
        }
        // A 先结束：A 的缓冲应被 flush，B 的仍留在缓冲
        s.emit(Event::JobEnd {
            job: "a".into(),
            code: 0,
            duration_ms: 1,
        });
        assert!(!s.buffers.contains_key("a"), "a 的缓冲应已 flush");
        assert!(s.buffers.contains_key("b"), "b 的缓冲应保留");
        assert_eq!(s.buffers["b"].len(), 3);
        // B 结束
        s.emit(Event::JobEnd {
            job: "b".into(),
            code: 1,
            duration_ms: 2,
        });
        assert!(s.buffers.is_empty());
    }

    /// 单 job：不进入分组，缓冲恒空
    #[test]
    fn single_job_stays_live() {
        let mut s = sink();
        s.emit(Event::JobStart {
            job: "only".into(),
            ts: String::new(),
        });
        assert!(!s.grouped);
        s.emit(Event::StepOutput {
            job: "only".into(),
            step: "s".into(),
            stream: Stream::Stdout,
            line: "hello".into(),
        });
        assert!(!s.grouped);
        assert!(s.buffers.is_empty(), "单 job 不应有缓冲");
    }

    /// --no-group：即使并行也不分组
    #[test]
    fn no_group_flag_disables_grouping() {
        let mut s = sink();
        s.group_enabled = false;
        s.emit(Event::JobStart {
            job: "a".into(),
            ts: String::new(),
        });
        s.emit(Event::JobStart {
            job: "b".into(),
            ts: String::new(),
        });
        assert!(!s.grouped, "no_group 下不应进入分组模式");
    }
}
