use std::path::PathBuf;
use std::time::Duration;

pub type EnvMap = Vec<(String, String)>;

/// 多层 env 合并：后者覆盖前者（保序，确定性）
pub fn merge_env(layers: &[&EnvMap]) -> EnvMap {
    let mut out: EnvMap = Vec::new();
    for layer in layers {
        for (k, v) in *layer {
            match out.iter_mut().find(|(ek, _)| ek == k) {
                Some(slot) => slot.1 = v.clone(),
                None => out.push((k.clone(), v.clone())),
            }
        }
    }
    out
}

/// job/step 结算状态（spec §14.2 三态）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    Failure,
    Skipped,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Workflow {
    pub version: u32,
    pub env: EnvMap,
    pub working_directory: Option<PathBuf>,
    pub jobs: Vec<Job>,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub id: String,
    pub needs: Vec<String>,
    pub env: EnvMap,
    pub working_directory: Option<PathBuf>,
    pub timeout_minutes: Option<u32>,
    pub if_condition: Option<Expr>,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub name: Option<String>,
    pub run: String,
    pub shell: Shell,
    pub working_directory: Option<PathBuf>,
    pub env: EnvMap,
    pub if_condition: Option<Expr>,
    pub timeout_minutes: Option<u32>,
    pub continue_on_error: bool,
    pub retry: Option<Retry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Pwsh,
    Cmd,
    Bash,
    Sh,
}

impl Shell {
    pub fn as_str(&self) -> &'static str {
        match self {
            Shell::Pwsh => "pwsh",
            Shell::Cmd => "cmd",
            Shell::Bash => "bash",
            Shell::Sh => "sh",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retry {
    pub attempts: u32,
    pub delay: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Success,
    Failure,
    Always,
    Eq(String, Literal),
    Ne(String, Literal),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Str(String),
    Num(f64),
    Bool(bool),
    Null,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunOptions {
    pub max_parallel: Option<usize>,
    /// CLI `-C` 显式指定时为 Some；None 时用 workflow 级或 cwd
    pub working_dir: Option<PathBuf>,
    pub json_output: bool,
    pub quiet: bool,
    pub color: bool,
}

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    RunStart {
        workflow: String,
        ts: String,
    },
    JobStart {
        job: String,
        ts: String,
    },
    StepStart {
        job: String,
        step: String,
        ts: String,
    },
    StepOutput {
        job: String,
        step: String,
        stream: Stream,
        line: String,
    },
    StepEnd {
        job: String,
        step: String,
        code: u32,
        duration_ms: u64,
    },
    JobEnd {
        job: String,
        code: u32,
        duration_ms: u64,
    },
    RunEnd {
        code: u32,
        duration_ms: u64,
        ts: String,
    },
}

pub trait EventSink {
    fn emit(&mut self, event: Event);
}

pub fn now_rfc3339() -> String {
    jiff::Timestamp::now().to_string()
}
