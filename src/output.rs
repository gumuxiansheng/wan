use std::io::Write;

use anstyle::{AnsiColor, Color, Style};

use crate::model::{Event, EventSink, RunOptions, Stream};

pub struct HumanSink {
    pub color: bool,
    pub quiet: bool,
    out: std::io::Stdout,
}

impl HumanSink {
    pub fn new(opts: &RunOptions) -> Self {
        HumanSink {
            color: opts.color,
            quiet: opts.quiet,
            out: std::io::stdout(),
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
}

const JOB: Style = Style::new().bold().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
const STEP: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Blue)));
const OK: Style = Style::new().bold().fg_color(Some(Color::Ansi(AnsiColor::Green)));
const FAIL: Style = Style::new().bold().fg_color(Some(Color::Ansi(AnsiColor::Red)));
const WARN: Style = Style::new().bold().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
const DIM: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::BrightBlack)));

impl EventSink for HumanSink {
    fn emit(&mut self, event: Event) {
        match event {
            Event::RunStart { workflow, .. } => {
                self.print(self.paint(&format!("==> 开始运行 workflow: {workflow}"), JOB).to_string());
            }
            Event::JobStart { job, .. } => {
                self.print(self.paint(&format!("[job] {job}"), JOB).to_string());
            }
            Event::StepStart { job: _, step, .. } => {
                self.print(format!("  {}", self.paint(&format!("[step] {step}"), STEP)));
            }
            Event::StepOutput { job, step, stream, line } => {
                if self.quiet {
                    return;
                }
                let prefix = match stream {
                    Stream::Stdout => "",
                    Stream::Stderr => "err| ",
                };
                self.print(format!("    {prefix}{line}"));
                let _ = (job, step);
            }
            Event::StepEnd { step, code, duration_ms, .. } => {
                if code == 0 {
                    self.print(format!("    {} ({duration_ms} ms)", self.paint("OK", OK)));
                } else {
                    self.print(format!(
                        "    {} (code {code}, {duration_ms} ms)",
                        self.paint("FAIL", FAIL)
                    ));
                }
                let _ = step;
            }
            Event::JobEnd { job, code, duration_ms, .. } => {
                let status = if code == 0 { self.paint("OK", OK) } else { self.paint("FAIL", FAIL) };
                self.print(format!("[job] {job} {status} ({duration_ms} ms)"));
            }
            Event::RunEnd { code, duration_ms, .. } => {
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
        let _ = writeln!(self.out, "{}", serde_json::to_string(&event).unwrap_or_default());
    }
}

/// 测试辅助：收集事件（集成测试用）
#[allow(dead_code)]
pub struct CapturingSink {
    pub events: Vec<Event>,
}

#[allow(dead_code)]
impl CapturingSink {
    pub fn new() -> Self {
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
    #[test]
    fn json_event_shape() {
        use crate::model::{Event, Stream};
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
}
