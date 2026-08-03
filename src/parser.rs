//! YAML 解析层薄封装（spec §15.7）：
//! saphyr 事件流 → 中间 Document（保序 Map + 行列号）→ Workflow + 手写 schema 校验
//! F-PARSE-4 行列号 / F-PARSE-5 未识别字段报错 / F-PARSE-7 ${{ }} 报错

use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::expr;
use crate::model::{EnvMap, Job, Retry, Step, Workflow};
use saphyr_parser::{Event, Parser, ScalarStyle};

#[derive(Debug, Clone, PartialEq)]
pub enum DValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Seq(Vec<DNode>),
    Map(Vec<(DNode, DNode)>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct DNode {
    pub value: DValue,
    pub line: usize,
    pub col: usize,
}

impl DNode {
    /// line/col 为 1 起始（saphyr Marker 已 1 起始；col 需 +1）
    fn at(value: DValue, line: usize, col: usize) -> Self {
        DNode { value, line, col }
    }

    fn err(&self, msg: impl Into<String>) -> Error {
        Error::config_at(msg, self.line, self.col)
    }
}

enum Frame {
    Seq(Vec<DNode>),
    Map(Vec<(DNode, DNode)>, Option<DNode>),
}

fn classify_scalar(s: &str, style: ScalarStyle) -> DValue {
    if style != ScalarStyle::Plain {
        return DValue::Str(s.to_string());
    }
    match s {
        "" => return DValue::Null,
        "~" | "null" | "Null" | "NULL" => return DValue::Null,
        "true" | "True" | "TRUE" => return DValue::Bool(true),
        "false" | "False" | "FALSE" => return DValue::Bool(false),
        _ => {}
    }
    let bytes = s.as_bytes();
    let (_neg, rest) = match bytes.first() {
        Some(b'-') => (true, &bytes[1..]),
        Some(b'+') => (false, &bytes[1..]),
        _ => (false, bytes),
    };
    if !rest.is_empty() && rest.iter().all(|b| b.is_ascii_digit()) {
        if let Ok(v) = s.parse::<i64>() {
            return DValue::Int(v);
        }
    }
    let looks_float = s.contains('.') || s.contains('e') || s.contains('E');
    if looks_float && !s.is_empty() && s.parse::<f64>().is_ok() {
        return DValue::Float(s.parse::<f64>().unwrap());
    }
    DValue::Str(s.to_string())
}

pub fn parse_document(content: &str) -> Result<DNode> {
    let mut parser = Parser::new_from_str(content);
    let mut stack: Vec<Frame> = Vec::new();
    let mut root: Option<DNode> = None;
    let mut doc_count = 0usize;

    loop {
        let (ev, span) = match parser.next_event() {
            Some(Ok(pair)) => pair,
            Some(Err(e)) => {
                let m = e.marker();
                return Err(Error::config_at(e.to_string(), m.line(), m.col() + 1));
            }
            None => break,
        };
        match ev {
            Event::DocumentStart(_) => {
                doc_count += 1;
                if doc_count > 1 {
                    return Err(Error::config("仅支持单个 YAML 文档"));
                }
            }
            Event::DocumentEnd | Event::StreamStart | Event::StreamEnd => {}
            Event::Alias(_) => {
                return Err(Error::config("不支持 YAML anchor/alias"));
            }
            Event::Scalar(v, style, _, _) => {
                let node =
                    DNode::at(classify_scalar(&v, style), span.start.line(), span.start.col() + 1);
                push_node(&mut stack, &mut root, node)?;
            }
            Event::SequenceStart(_, _) => {
                stack.push(Frame::Seq(Vec::new()));
            }
            Event::MappingStart(_, _) => {
                stack.push(Frame::Map(Vec::new(), None));
            }
            Event::SequenceEnd => {
                let Frame::Seq(items) = stack.pop().unwrap() else {
                    return Err(Error::config("YAML 结构错误：SequenceEnd 不匹配"));
                };
                push_node(
                    &mut stack,
                    &mut root,
                    DNode::at(DValue::Seq(items), span.start.line(), span.start.col() + 1),
                )?;
            }
            Event::MappingEnd => {
                let Frame::Map(pairs, _) = stack.pop().unwrap() else {
                    return Err(Error::config("YAML 结构错误：MappingEnd 不匹配"));
                };
                push_node(
                    &mut stack,
                    &mut root,
                    DNode::at(DValue::Map(pairs), span.start.line(), span.start.col() + 1),
                )?;
            }
            Event::Nothing => {}
        }
    }

    root.ok_or_else(|| Error::config("配置文件为空"))
}

fn push_node(stack: &mut [Frame], root: &mut Option<DNode>, node: DNode) -> Result<()> {
    match stack.last_mut() {
        None => {
            *root = Some(node);
        }
        Some(Frame::Seq(items)) => items.push(node),
        Some(Frame::Map(pairs, pending)) => match pending.take() {
            None => {
                if !matches!(node.value, DValue::Str(_)) {
                    return Err(node.err("不支持的复杂 YAML 键（仅支持标量键）"));
                }
                *pending = Some(node);
            }
            Some(key) => {
                pairs.push((key, node));
            }
        },
    }
    Ok(())
}

/// F-PARSE-7：任何字符串含 `${{ }}` 直接报错
fn check_dollar_brace(node: &DNode) -> Result<()> {
    match &node.value {
        DValue::Str(s) => {
            if s.contains("${{") {
                return Err(node.err("不支持 `${{ }}` 表达式，请使用 `${VAR}`"));
            }
        }
        DValue::Seq(items) => {
            for item in items {
                check_dollar_brace(item)?;
            }
        }
        DValue::Map(pairs) => {
            for (k, v) in pairs {
                check_dollar_brace(k)?;
                check_dollar_brace(v)?;
            }
        }
        _ => {}
    }
    Ok(())
}

// ---------- 类型取读辅助 ----------

fn key_str(k: &DNode, node: &DNode) -> Result<String> {
    match &k.value {
        DValue::Str(s) => Ok(s.clone()),
        _ => Err(node.err("不支持的键类型（仅支持字符串键）")),
    }
}

fn get_map<'a>(node: &'a DNode, what: &str) -> Result<&'a [(DNode, DNode)]> {
    match &node.value {
        DValue::Map(pairs) => Ok(pairs),
        _ => Err(node.err(format!("`{what}` 必须是映射（map）"))),
    }
}

fn get_seq<'a>(node: &'a DNode, what: &str) -> Result<&'a [DNode]> {
    match &node.value {
        DValue::Seq(items) => Ok(items),
        _ => Err(node.err(format!("`{what}` 必须是列表（seq）"))),
    }
}

fn get_str(node: &DNode, what: &str) -> Result<String> {
    match &node.value {
        DValue::Str(s) => Ok(s.clone()),
        _ => Err(node.err(format!("`{what}` 必须是字符串"))),
    }
}

fn get_int(node: &DNode, what: &str) -> Result<i64> {
    match &node.value {
        DValue::Int(v) => Ok(*v),
        _ => Err(node.err(format!("`{what}` 必须是整数"))),
    }
}

fn get_bool(node: &DNode, what: &str) -> Result<bool> {
    match &node.value {
        DValue::Bool(b) => Ok(*b),
        _ => Err(node.err(format!("`{what}` 必须是布尔值 true/false"))),
    }
}

fn find<'a>(pairs: &'a [(DNode, DNode)], key: &str) -> Option<&'a DNode> {
    pairs
        .iter()
        .find(|(k, _)| matches!(&k.value, DValue::Str(s) if s == key))
        .map(|(_, v)| v)
}

/// 校验未知字段；runs-on/uses 给专门报错（F-PARSE-6 / §9）
fn check_unknown(
    pairs: &[(DNode, DNode)],
    allowed: &[&str],
    scope: &str,
) -> Result<()> {
    for (k, v) in pairs {
        let key = key_str(k, v)?;
        if !allowed.contains(&key.as_str()) {
            let msg = match key.as_str() {
                "runs-on" => "`runs-on` 永不支持（本工具无 runner 概念，单机执行）",
                "uses" => "`uses:` action 生态永不支持（需 Node runtime / Docker）",
                "matrix" => "`matrix` v0.1 不支持",
                "container" | "services" => "`container:` / `services:` 永不支持（与零依赖定位冲突）",
                "concurrency" => "`concurrency:` 不支持（单机本地执行）",
                "schedule" => "`schedule:` 属 v0.2 cron 范围，v0.1 不支持",
                _ => "未识别字段",
            };
            return Err(v.err(format!(
                "{scope}：{msg}：`{key}`（显式失败 > 静默降级）"
            )));
        }
    }
    Ok(())
}

/// 检测重复键
fn check_dup(pairs: &[(DNode, DNode)], scope: &str) -> Result<()> {
    let mut seen: Vec<String> = Vec::new();
    for (k, v) in pairs {
        let key = key_str(k, v)?;
        if seen.contains(&key) {
            return Err(v.err(format!("{scope}：重复字段 `{key}`")));
        }
        seen.push(key);
    }
    Ok(())
}

// ---------- 字段解析 ----------

fn parse_env(node: &DNode, scope: &str) -> Result<EnvMap> {
    let pairs = get_map(node, scope)?;
    let mut out = EnvMap::new();
    for (k, v) in pairs {
        let key = key_str(k, v)?;
        let val = get_str(v, &format!("{scope}.{key}"))?;
        out.push((key, val));
    }
    Ok(out)
}

fn parse_retry(node: &DNode) -> Result<Retry> {
    let pairs = get_map(node, "retry")?;
    check_dup(pairs, "retry")?;
    check_unknown(pairs, &["attempts", "delay"], "retry")?;
    let attempts_node = find(pairs, "attempts").ok_or_else(|| node.err("retry 缺少必填字段 attempts"))?;
    let attempts = get_int(attempts_node, "retry.attempts")?;
    if attempts < 1 {
        return Err(attempts_node.err("retry.attempts 必须 >= 1（含首次）"));
    }
    let delay_node = find(pairs, "delay").ok_or_else(|| node.err("retry 缺少必填字段 delay"))?;
    let delay_str = get_str(delay_node, "retry.delay")?;
    let delay = parse_duration(&delay_str)
        .ok_or_else(|| delay_node.err(format!("retry.delay 格式非法：`{delay_str}`（支持 s/m/h 后缀，如 5s / 2m / 1h）")))?;
    Ok(Retry { attempts: attempts as u32, delay })
}

fn parse_duration(s: &str) -> Option<std::time::Duration> {
    let t = s.trim();
    let (num, mult) = match t.chars().last()? {
        's' => (&t[..t.len() - 1], 1.0),
        'm' => (&t[..t.len() - 1], 60.0),
        'h' => (&t[..t.len() - 1], 3600.0),
        _ => (t, 1.0),
    };
    let v: f64 = num.parse().ok()?;
    if v < 0.0 {
        return None;
    }
    Some(std::time::Duration::from_secs_f64(v * mult))
}

fn valid_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn parse_step(node: &DNode, job_id: &str) -> Result<Step> {
    let pairs = get_map(node, "step")?;
    check_dup(pairs, &format!("jobs.{job_id}.steps[]"))?;
    check_unknown(
        pairs,
        &[
            "name", "run", "shell", "working-directory", "env", "if", "timeout-minutes",
            "continue-on-error", "retry",
        ],
        &format!("jobs.{job_id}.steps[]"),
    )?;

    let run = find(pairs, "run")
        .ok_or_else(|| node.err(format!("jobs.{job_id}.steps[] 缺少必填字段 run")))?
        .clone();
    let run = get_str(&run, "step.run")?;

    let shell_node = find(pairs, "shell")
        .ok_or_else(|| node.err(format!("jobs.{job_id}.steps[] 缺少必填字段 shell（必须显式指定）")))?
        .clone();
    let shell = crate::shell::parse(&get_str(&shell_node, "step.shell")?)
        .map_err(|e| Error::config_at(e.msg, shell_node.line, shell_node.col))?;

    let name = match find(pairs, "name") {
        Some(n) => Some(get_str(n, "step.name")?),
        None => None,
    };
    let working_directory = match find(pairs, "working-directory") {
        Some(w) => Some(PathBuf::from(get_str(w, "step.working-directory")?)),
        None => None,
    };
    let env = match find(pairs, "env") {
        Some(e) => parse_env(e, &format!("jobs.{job_id}.steps[] env"))?,
        None => EnvMap::new(),
    };
    let if_condition = match find(pairs, "if") {
        Some(i) => {
            let s = get_str(i, "step.if")?;
            Some(expr::parse_if(&s).map_err(|e| Error::config_at(e.msg, i.line, i.col))?)
        }
        None => None,
    };
    let timeout_minutes = match find(pairs, "timeout-minutes") {
        Some(t) => {
            let v = get_int(t, "step.timeout-minutes")?;
            if v < 1 {
                return Err(t.err("step.timeout-minutes 必须 >= 1"));
            }
            Some(v as u32)
        }
        None => None,
    };
    let continue_on_error = match find(pairs, "continue-on-error") {
        Some(c) => get_bool(c, "step.continue-on-error")?,
        None => false,
    };
    let retry = match find(pairs, "retry") {
        Some(r) => Some(parse_retry(r)?),
        None => None,
    };

    Ok(Step {
        name,
        run,
        shell,
        working_directory,
        env,
        if_condition,
        timeout_minutes,
        continue_on_error,
        retry,
    })
}

fn parse_job(node: &DNode, id: &str) -> Result<Job> {
    let pairs = get_map(node, &format!("jobs.{id}"))?;
    check_dup(pairs, &format!("jobs.{id}"))?;
    check_unknown(
        pairs,
        &[
            "needs", "env", "working-directory", "timeout-minutes", "if", "steps",
        ],
        &format!("jobs.{id}"),
    )?;

    let needs = match find(pairs, "needs") {
        Some(n) => {
            let items = get_seq(n, &format!("jobs.{id}.needs"))?;
            let mut out = Vec::new();
            for item in items {
                out.push(get_str(item, "needs 元素")?);
            }
            out
        }
        None => Vec::new(),
    };
    let env = match find(pairs, "env") {
        Some(e) => parse_env(e, &format!("jobs.{id} env"))?,
        None => EnvMap::new(),
    };
    let working_directory = match find(pairs, "working-directory") {
        Some(w) => Some(PathBuf::from(get_str(w, &format!("jobs.{id}.working-directory"))?)),
        None => None,
    };
    let timeout_minutes = match find(pairs, "timeout-minutes") {
        Some(t) => {
            let v = get_int(t, &format!("jobs.{id}.timeout-minutes"))?;
            if v < 1 {
                return Err(t.err("jobs.timeout-minutes 必须 >= 1"));
            }
            Some(v as u32)
        }
        None => None,
    };
    let if_condition = match find(pairs, "if") {
        Some(i) => {
            let s = get_str(i, &format!("jobs.{id}.if"))?;
            Some(expr::parse_if(&s).map_err(|e| Error::config_at(e.msg, i.line, i.col))?)
        }
        None => None,
    };

    let steps_node = find(pairs, "steps")
        .ok_or_else(|| node.err(format!("jobs.{id} 缺少必填字段 steps")))?
        .clone();
    let step_items = get_seq(&steps_node, &format!("jobs.{id}.steps"))?;
    // F-PARSE-10
    if step_items.is_empty() {
        return Err(steps_node.err(format!("jobs.{id}.steps 必须至少包含一个 step")));
    }
    let mut steps = Vec::new();
    for item in step_items {
        steps.push(parse_step(item, id)?);
    }

    Ok(Job {
        id: id.to_string(),
        needs,
        env,
        working_directory,
        timeout_minutes,
        if_condition,
        steps,
    })
}

fn parse_workflow_doc(root: &DNode, source: String) -> Result<Workflow> {
    let pairs = get_map(root, "workflow")?;
    check_dup(pairs, "workflow")?;
    check_unknown(pairs, &["version", "env", "working-directory", "jobs"], "workflow")?;

    let version_node = find(pairs, "version")
        .ok_or_else(|| root.err("缺少顶层强制字段 version: 1"))?
        .clone();
    match version_node.value {
        DValue::Int(1) => {}
        DValue::Int(v) => {
            return Err(version_node.err(format!(
                "version 必须为 1，收到 `{v}`（v1.0 前 schema 不稳定）"
            )))
        }
        _ => return Err(version_node.err("version 必须为整数 1")),
    }

    let env = match find(pairs, "env") {
        Some(e) => parse_env(e, "workflow env")?,
        None => EnvMap::new(),
    };
    let working_directory = match find(pairs, "working-directory") {
        Some(w) => Some(PathBuf::from(get_str(w, "workflow.working-directory")?)),
        None => None,
    };

    let jobs_node = find(pairs, "jobs")
        .ok_or_else(|| root.err("缺少必填字段 jobs"))?
        .clone();
    let job_pairs = get_map(&jobs_node, "jobs")?;
    if job_pairs.is_empty() {
        return Err(jobs_node.err("jobs 必须至少包含一个 job"));
    }
    let mut jobs = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    for (k, v) in job_pairs {
        let id = key_str(k, v)?;
        // F-PARSE-9
        if !valid_ident(&id) {
            return Err(k.err(format!(
                "job id 非法：`{id}`（须为 [A-Za-z_][A-Za-z0-9_-]*）"
            )));
        }
        if ids.contains(&id) {
            return Err(k.err(format!("job id 重复：`{id}`")));
        }
        ids.push(id.clone());
        jobs.push(parse_job(v, &id)?);
    }

    // F-PARSE-8：needs 必须引用已定义 job
    for job in &jobs {
        for need in &job.needs {
            if !ids.contains(need) {
                return Err(Error::config(format!(
                    "jobs.{}.needs 引用了不存在的 job：`{need}`",
                    job.id
                )));
            }
        }
    }

    Ok(Workflow { version: 1, env, working_directory, jobs, source })
}

/// 从字符串加载 workflow（校验全部 F-PARSE 需求）
pub fn load_from_str(content: &str, source: impl Into<String>) -> Result<Workflow> {
    let doc = parse_document(content)?;
    check_dollar_brace(&doc)?;
    parse_workflow_doc(&doc, source.into())
}

/// 从文件加载 workflow
pub fn load_file(path: &std::path::Path) -> Result<Workflow> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        Error::io(format!("读取文件失败：{}：{e}", path.display()))
    })?;
    let source = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());
    load_from_str(&content, source)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(s: &str) -> Result<Workflow> {
        load_from_str(s, "test")
    }

    const MINIMAL: &str = r#"
version: 1
jobs:
  build:
    steps:
      - name: 编译
        shell: pwsh
        run: echo hi
"#;

    #[test]
    fn minimal_ok() {
        let wf = load(MINIMAL).unwrap();
        assert_eq!(wf.version, 1);
        assert_eq!(wf.jobs.len(), 1);
        assert_eq!(wf.jobs[0].id, "build");
        assert_eq!(wf.jobs[0].steps[0].name.as_deref(), Some("编译"));
    }

    #[test]
    fn missing_version() {
        let e = load("jobs:\n  a:\n    steps:\n      - run: x\n        shell: sh\n").unwrap_err();
        assert!(e.msg.contains("version"), "{e}");
        assert!(e.line.is_some());
    }

    #[test]
    fn wrong_version() {
        let e = load("version: 2\njobs: {}\n").unwrap_err();
        assert!(e.msg.contains("必须为 1"));
    }

    #[test]
    fn unknown_field_with_position() {
        let e = load("version: 1\nfoo: bar\njobs:\n  a:\n    steps: []\n").unwrap_err();
        assert!(e.msg.contains("未识别字段"), "{e}");
        assert_eq!(e.line, Some(2));
    }

    #[test]
    fn runs_on_errors() {
        let e = load("version: 1\nruns-on: windows\njobs:\n  a:\n    steps:\n      - run: x\n        shell: sh\n").unwrap_err();
        assert!(e.msg.contains("runs-on"), "{e}");
    }

    #[test]
    fn dollar_brace_errors() {
        let e = load("version: 1\njobs:\n  a:\n    steps:\n      - run: echo ${{ x }}\n        shell: sh\n").unwrap_err();
        assert!(e.msg.contains("${{"), "{e}");
    }

    #[test]
    fn needs_dangling() {
        let e = load("version: 1\njobs:\n  a:\n    needs: [nope]\n    steps:\n      - run: x\n        shell: sh\n").unwrap_err();
        assert!(e.msg.contains("不存在的 job"), "{e}");
    }

    #[test]
    fn missing_run_and_shell() {
        let e = load("version: 1\njobs:\n  a:\n    steps:\n      - name: x\n").unwrap_err();
        assert!(e.msg.contains("run"), "{e}");
        let e = load("version: 1\njobs:\n  a:\n    steps:\n      - run: x\n").unwrap_err();
        assert!(e.msg.contains("shell"), "{e}");
    }

    #[test]
    fn bad_shell_enum() {
        let e = load("version: 1\njobs:\n  a:\n    steps:\n      - run: x\n        shell: powershell\n").unwrap_err();
        assert!(e.msg.contains("shell"), "{e}");
    }

    #[test]
    fn empty_steps_rejected() {
        let e = load("version: 1\njobs:\n  a:\n    steps: []\n").unwrap_err();
        assert!(e.msg.contains("至少包含一个 step"), "{e}");
    }

    #[test]
    fn dup_job_id() {
        let e = load("version: 1\njobs:\n  a:\n    steps:\n      - run: x\n        shell: sh\n  a:\n    steps:\n      - run: y\n        shell: sh\n").unwrap_err();
        assert!(e.msg.contains("重复"), "{e}");
    }

    #[test]
    fn env_strings_only() {
        let e = load("version: 1\nenv:\n  N: 3\njobs:\n  a:\n    steps:\n      - run: x\n        shell: sh\n").unwrap_err();
        assert!(e.msg.contains("字符串"), "{e}");
    }

    #[test]
    fn full_example_ok() {
        let s = r#"
version: 1
env:
  GLOBAL_VAR: "shared-value"
jobs:
  build:
    env:
      BUILD_DIR: "${HOME}/build"
    steps:
      - name: 编译
        shell: pwsh
        working-directory: src
        run: |
          echo building
        timeout-minutes: 10
        retry:
          attempts: 3
          delay: 5s
  test:
    needs: [build]
    steps:
      - name: 单元测试
        shell: pwsh
        run: echo test
        if: success()
      - name: 失败通知
        shell: cmd
        run: echo test failed
        if: failure()
  report:
    needs: [test]
    if: always()
    steps:
      - name: 汇总
        shell: pwsh
        run: echo done
"#;
        let wf = load(s).unwrap();
        assert_eq!(wf.jobs.len(), 3);
        assert_eq!(wf.jobs[1].needs, vec!["build"]);
        assert!(wf.jobs[2].if_condition.is_some());
        assert!(wf.jobs[0].steps[0].retry.is_some());
    }

    #[test]
    fn retry_bad_attempts() {
        let e = load("version: 1\njobs:\n  a:\n    steps:\n      - run: x\n        shell: sh\n        retry:\n          attempts: 0\n          delay: 1s\n").unwrap_err();
        assert!(e.msg.contains(">= 1"), "{e}");
    }

    #[test]
    fn retry_bad_delay() {
        let e = load("version: 1\njobs:\n  a:\n    steps:\n      - run: x\n        shell: sh\n        retry:\n          attempts: 2\n          delay: 1m30s\n").unwrap_err();
        assert!(e.msg.contains("delay"), "{e}");
    }
}
