//! CLI 层（spec §7）：薄壳，仅参数解析与退出码

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use lexopt::prelude::*;

use crate::engine;
use crate::error::{Error, Result};
use crate::model::RunOptions;
use crate::output::{HumanSink, JsonSink};
use crate::parser::load_file;

const USAGE: &str = "\
wan — 本地工作流执行器

用法:
  wan run <file|name> [--json] [--max-parallel N] [--quiet] [--no-color] [--no-group] [-C <dir>]
  wan validate <file|name> [-C <dir>]
  wan list [-C <dir>]
  wan graph <file|name> [-C <dir>]
  wan hook install <hook-type> <workflow> [-C <dir>] [--force]
  wan hook remove <hook-type> [-C <dir>] [--force]
  wan hook list [-C <dir>]
  wan schedule add <id> <cron-expr> <workflow> [-C <dir>]
  wan schedule remove <id> [-C <dir>]
  wan schedule list [-C <dir>]
  wan schedule start [-C <dir>] [--catch-up] [--json] [--quiet] [--no-color] [--no-group]
  wan schedule run-once [-C <dir>] [--json] [--quiet] [--no-color] [--no-group]
  wan schedule service install|remove|status [-C <dir>]
  wan schedule history [<id>] [-C <dir>] [--limit N]
  wan --version
  wan --help

命令:
  run          执行一个 workflow 文件
  validate     仅校验 schema 与 DAG，不执行
  list         列出 .wan/workflows/ 目录下所有 workflow
  graph        输出 mermaid 文本
  hook         管理 git hook（安装/删除/列出）
  schedule     管理 cron 调度（添加/删除/列出/启动/历史）
  --version    打印版本
  --help       打印本帮助

<hook-type>: pre-commit / pre-push / post-commit / post-merge / post-checkout

<file|name>: 含路径分隔符按文件路径处理；否则按短名在 .wan/workflows/ 下查找，
             自动匹配平台后缀（Windows: {name}-win.yml；Linux: {name}-unix.yml），
             无平台后缀文件时回退 {name}.yml/.yaml，均无则报错。

<cron-expr>: 标准 5 字段 cron 表达式（分 时 日 月 周），例如：
             `0 2 * * *`    每天 02:00
             `*/30 * * * *` 每 30 分钟
             `0 0 * * 1`    每周一 00:00

全局参数:
  --json            结构化事件流（每行一个 JSON）
  --max-parallel N  job 并行上限（默认无上限）
  --quiet           抑制 step 输出（与 --json 同用时无效）
  --no-color        禁用颜色
  --no-group        禁用并行分组（并行 job 输出实时直通，允许穿插）
  -C <dir>          工作目录（默认当前目录）

退出码: 0 成功 / 1 执行失败 / 2 配置错误 / 130 中断
";

fn print_version() {
    println!("wan {}", env!("CARGO_PKG_VERSION"));
}

fn report_error(e: &Error) {
    match (e.line, e.col) {
        (Some(l), Some(c)) => eprintln!("error: {}:{}:{}", e.msg, l, c),
        _ => eprintln!("error: {e}"),
    }
}

struct Flags {
    json: bool,
    quiet: bool,
    color: bool,
    no_group: bool,
    max_parallel: Option<usize>,
    cwd: Option<PathBuf>,
}

impl Default for Flags {
    fn default() -> Self {
        Flags {
            json: false,
            quiet: false,
            color: true,
            no_group: false,
            max_parallel: None,
            cwd: None,
        }
    }
}

fn take_value(parser: &mut lexopt::Parser, flag: &str) -> Result<OsString> {
    parser
        .value()
        .map_err(|_| Error::config(format!("参数 `{flag}` 缺少值")))
}

pub fn run_main() -> i32 {
    crate::platform::install_interrupt_handler();
    crate::platform::setup_utf8_console();
    // 注意：lexopt from_iter 会把第一个元素当作程序名，需传入 argv[0]
    let args: Vec<OsString> = std::env::args_os().collect();
    match dispatch(args) {
        Ok(code) => code,
        Err(e) => {
            report_error(&e);
            e.exit_code()
        }
    }
}

fn dispatch(args: Vec<OsString>) -> Result<i32> {
    let mut parser = lexopt::Parser::from_iter(args);
    let cmd = match parser.next()? {
        Some(Long("help")) => return Ok(print_help()),
        Some(Short('h')) => return Ok(print_help()),
        Some(Long("version")) => {
            print_version();
            return Ok(0);
        }
        Some(Value(s)) => s.to_string_lossy().to_string(),
        Some(Short(_)) | Some(Long(_)) => {
            return Err(Error::config("未知选项，`wan --help` 查看用法"))
        }
        None => return Ok(print_help()),
    };

    match cmd.as_str() {
        "run" => cmd_run(parser),
        "validate" => cmd_validate(parser),
        "list" => cmd_list(parser),
        "graph" => cmd_graph(parser),
        "hook" => cmd_hook(parser),
        "schedule" => cmd_schedule(parser),
        "help" => Ok(print_help()),
        _ => Err(Error::config(format!(
            "未知命令 `{cmd}`，`wan --help` 查看用法"
        ))),
    }
}

fn print_help() -> i32 {
    print!("{USAGE}");
    0
}

/// 解析 <file> 参数：含路径分隔符视为路径；否则按短名在 .wan/workflows/
/// 下做平台后缀解析（§7.4）：`{name}-{win|unix}.{yml|yaml}` → `{name}.{yml|yaml}`
fn resolve_workflow_file(arg: &str, base: &Path) -> Result<PathBuf> {
    if arg.contains('/') || arg.contains('\\') {
        let p = PathBuf::from(arg);
        return if p.is_file() {
            Ok(p)
        } else {
            Err(Error::config(format!("找不到文件：{}", p.display())))
        };
    }

    let wf_dir = base.join(".wan").join("workflows");
    if !wf_dir.is_dir() {
        return Err(Error::config(format!(
            "目录 {} 不存在，无法按短名查找 workflow `{arg}`；请提供完整路径",
            wf_dir.display()
        )));
    }

    let stem = Path::new(arg)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| arg.to_string());
    let platform = if cfg!(windows) { "win" } else { "unix" };
    let mut candidates = Vec::new();
    for ext in ["yml", "yaml"] {
        candidates.push(format!("{stem}-{platform}.{ext}"));
    }
    for ext in ["yml", "yaml"] {
        candidates.push(format!("{stem}.{ext}"));
    }
    for c in &candidates {
        let p = wf_dir.join(c);
        if p.is_file() {
            return Ok(p);
        }
    }
    Err(Error::config(format!(
        "在 {} 下未找到 workflow `{stem}`（已尝试：{}）",
        wf_dir.display(),
        candidates.join("、")
    )))
}

fn parse_common_flags(parser: &mut lexopt::Parser, flags: &mut Flags) -> Result<Option<PathBuf>> {
    let mut file: Option<PathBuf> = None;
    while let Some(arg) = parser.next()? {
        match arg {
            Long("json") => flags.json = true,
            Long("quiet") => flags.quiet = true,
            Long("no-color") => flags.color = false,
            Long("no-group") => flags.no_group = true,
            Short('C') => {
                let v = take_value(parser, "-C")?;
                let p = PathBuf::from(v);
                flags.cwd = if p.is_absolute() {
                    Some(p)
                } else {
                    let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                    Some(base.join(p))
                };
            }
            Long("max-parallel") => {
                let v = take_value(parser, "--max-parallel")?;
                let n: usize = v
                    .to_string_lossy()
                    .parse()
                    .map_err(|_| Error::config("`--max-parallel` 需要正整数"))?;
                if n == 0 {
                    return Err(Error::config("`--max-parallel` 需要 >= 1"));
                }
                flags.max_parallel = Some(n);
            }
            Value(p) => {
                if file.is_some() {
                    return Err(Error::config("多余的位置参数"));
                }
                file = Some(PathBuf::from(p));
            }
            Short(_) | Long(_) => {
                return Err(Error::config("未知选项，`wan --help` 查看用法".to_string()))
            }
        }
    }
    Ok(file)
}

fn cmd_run(parser: lexopt::Parser) -> Result<i32> {
    let mut flags = Flags::default();
    let mut parser = parser;
    let file = parse_common_flags(&mut parser, &mut flags)?
        .ok_or_else(|| Error::config("`wan run` 需要 <file> 参数"))?;
    let base = flags
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let path = resolve_workflow_file(&file.to_string_lossy(), &base)?;

    let wf = load_file(&path)?;
    engine::validate(&wf)?;

    let opts = RunOptions {
        max_parallel: flags.max_parallel,
        working_dir: flags.cwd.clone(),
        json_output: flags.json,
        quiet: flags.quiet,
        color: flags.color,
        no_group: flags.no_group,
    };

    let sink: Box<dyn crate::model::EventSink + Send> = if flags.json {
        Box::new(JsonSink::new(std::io::stdout()))
    } else {
        Box::new(HumanSink::new(&opts))
    };

    engine::run(&wf, &opts, sink)
}

fn cmd_validate(parser: lexopt::Parser) -> Result<i32> {
    let mut flags = Flags::default();
    let mut parser = parser;
    let file = parse_common_flags(&mut parser, &mut flags)?
        .ok_or_else(|| Error::config("`wan validate` 需要 <file> 参数"))?;
    let base = flags
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let path = resolve_workflow_file(&file.to_string_lossy(), &base)?;

    let wf = load_file(&path)?;
    engine::validate(&wf)?;
    println!(
        "OK: {} 校验通过（{} job, DAG 无环）",
        path.display(),
        wf.jobs.len()
    );
    Ok(0)
}

fn cmd_list(parser: lexopt::Parser) -> Result<i32> {
    let mut flags = Flags::default();
    let mut parser = parser;
    let _file = parse_common_flags(&mut parser, &mut flags)?;

    let dir = flags
        .cwd
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let wf_dir = dir.join(".wan").join("workflows");
    let search_dir = if wf_dir.is_dir() { wf_dir } else { dir };

    let mut found: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&search_dir) {
        for e in entries.flatten() {
            let p = e.path();
            let is_wf = matches!(
                p.extension().and_then(|x| x.to_str()),
                Some("yml") | Some("yaml")
            );
            if p.is_file() && is_wf {
                if let Some(stem) = p.file_stem() {
                    found.push(stem.to_string_lossy().to_string());
                }
            }
        }
    }
    found.sort();
    for name in found {
        println!("{name}");
    }
    Ok(0)
}

fn cmd_graph(parser: lexopt::Parser) -> Result<i32> {
    let mut flags = Flags::default();
    let mut parser = parser;
    let file = parse_common_flags(&mut parser, &mut flags)?
        .ok_or_else(|| Error::config("`wan graph` 需要 <file> 参数"))?;
    let base = flags
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let path = resolve_workflow_file(&file.to_string_lossy(), &base)?;

    let wf = load_file(&path)?;
    engine::validate(&wf)?;

    println!("flowchart TD");
    for job in &wf.jobs {
        println!("  {}", job.id);
        for need in &job.needs {
            println!("  {need} --> {}", job.id);
        }
    }
    Ok(0)
}

/// wan hook install/remove/list
fn cmd_hook(parser: lexopt::Parser) -> Result<i32> {
    let mut parser = parser;
    let sub = match parser.next()? {
        Some(Value(s)) => s.to_string_lossy().to_string(),
        Some(Long("help")) | Some(Short('h')) => {
            println!("wan hook install <hook-type> <workflow> [-C <dir>] [--force]");
            println!("wan hook remove <hook-type> [-C <dir>] [--force]");
            println!("wan hook list [-C <dir>]");
            println!();
            println!(
                "<hook-type>: pre-commit / pre-push / post-commit / post-merge / post-checkout"
            );
            return Ok(0);
        }
        _ => {
            return Err(Error::config(
                "`wan hook` 需要子命令 install / remove / list",
            ))
        }
    };

    // 解析公共参数 -C / --force / 位置参数
    let mut cwd: Option<PathBuf> = None;
    let mut force = false;
    let mut positional: Vec<String> = Vec::new();

    while let Some(arg) = parser.next()? {
        match arg {
            Short('C') => {
                let v = take_value(&mut parser, "-C")?;
                let p = PathBuf::from(v);
                cwd = if p.is_absolute() {
                    Some(p)
                } else {
                    let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                    Some(base.join(p))
                };
            }
            Long("force") => force = true,
            Long("no-color") => { /* hook 不产生颜色输出，忽略 */ }
            Value(v) => positional.push(v.to_string_lossy().to_string()),
            Short(_) | Long(_) => {
                return Err(Error::config(
                    "未知选项，`wan hook --help` 查看用法".to_string(),
                ))
            }
        }
    }

    let search_dir =
        cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let git_dir = crate::hook::find_git_dir(&search_dir).ok_or_else(|| {
        Error::config(format!(
            "不在 git 仓库内（从 {} 未找到 .git 目录）",
            search_dir.display()
        ))
    })?;

    match sub.as_str() {
        "install" => {
            let hook_str = positional
                .first()
                .ok_or_else(|| Error::config("`wan hook install` 需要 <hook-type> 参数"))?;
            let workflow = positional
                .get(1)
                .ok_or_else(|| Error::config("`wan hook install` 需要 <workflow> 参数"))?;
            let hook_type = crate::hook::HookType::from_str(hook_str)?;
            crate::hook::install(&git_dir, hook_type, workflow, force)?;
            Ok(0)
        }
        "remove" => {
            let hook_str = positional
                .first()
                .ok_or_else(|| Error::config("`wan hook remove` 需要 <hook-type> 参数"))?;
            let hook_type = crate::hook::HookType::from_str(hook_str)?;
            crate::hook::remove(&git_dir, hook_type, force)?;
            Ok(0)
        }
        "list" => {
            let hooks = crate::hook::list(&git_dir)?;
            if hooks.is_empty() {
                println!("（无 wan-managed hook）");
            } else {
                for h in &hooks {
                    println!("{:<14} -> {}", h.hook_type.as_str(), h.workflow);
                }
            }
            Ok(0)
        }
        other => Err(Error::config(format!(
            "未知子命令 `wan hook {other}`，支持 install / remove / list"
        ))),
    }
}

/// wan schedule add/remove/list/start/run-once/service/history
fn cmd_schedule(parser: lexopt::Parser) -> Result<i32> {
    let mut parser = parser;
    let sub = match parser.next()? {
        Some(Value(s)) => s.to_string_lossy().to_string(),
        Some(Long("help")) | Some(Short('h')) => {
            println!("wan schedule add <id> <cron-expr> <workflow> [-C <dir>]");
            println!("wan schedule remove <id> [-C <dir>]");
            println!("wan schedule list [-C <dir>]");
            println!("wan schedule start [-C <dir>] [--catch-up] [--json] [--quiet] [--no-color]");
            println!("wan schedule run-once [-C <dir>] [--json] [--quiet] [--no-color]");
            println!("wan schedule service install|remove|status [-C <dir>]");
            println!("wan schedule history [<id>] [-C <dir>] [--limit N]");
            println!();
            println!("<cron-expr>: 分 时 日 月 周（如 `0 2 * * *` 每天 02:00）");
            return Ok(0);
        }
        _ => return Err(Error::config(
            "`wan schedule` 需要子命令 add / remove / list / start / run-once / service / history",
        )),
    };

    // 解析公共参数
    let mut cwd: Option<PathBuf> = None;
    let mut json_output = false;
    let mut quiet = false;
    let mut color = true;
    let mut no_group = false;
    let mut catch_up = false;
    let mut limit: usize = 20;
    let mut positional: Vec<String> = Vec::new();

    while let Some(arg) = parser.next()? {
        match arg {
            Short('C') => {
                let v = take_value(&mut parser, "-C")?;
                let p = PathBuf::from(v);
                cwd = if p.is_absolute() {
                    Some(p)
                } else {
                    let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                    Some(base.join(p))
                };
            }
            Long("json") => json_output = true,
            Long("quiet") => quiet = true,
            Long("no-color") => color = false,
            Long("no-group") => no_group = true,
            Long("catch-up") => catch_up = true,
            Long("limit") => {
                let v = take_value(&mut parser, "--limit")?;
                limit = v
                    .to_string_lossy()
                    .parse()
                    .map_err(|_| Error::config("`--limit` 需要正整数"))?;
            }
            Long("force") => { /* schedule 不用 force，但容忍 */ }
            Value(v) => positional.push(v.to_string_lossy().to_string()),
            Short(_) | Long(_) => {
                return Err(Error::config(
                    "未知选项，`wan schedule --help` 查看用法".to_string(),
                ))
            }
        }
    }

    let base =
        cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    match sub.as_str() {
        "add" => {
            let id = positional.first().ok_or_else(|| {
                Error::config("`wan schedule add` 需要 <id> 参数")
            })?;
            let cron_expr = positional.get(1).ok_or_else(|| {
                Error::config("`wan schedule add` 需要 <cron-expr> 参数")
            })?;
            let workflow = positional.get(2).ok_or_else(|| {
                Error::config("`wan schedule add` 需要 <workflow> 参数")
            })?;

            // 验证 cron 表达式
            let _ = crate::cron::CronExpr::parse(cron_expr)?;

            // 解析 workflow 路径（支持短名）
            let wf_path = resolve_workflow_file(workflow, &base)?;

            crate::schedule::add_schedule(&base, id, cron_expr, &wf_path)?;
            println!("已添加调度：{} [{}] -> {}", id, cron_expr, wf_path.display());
            Ok(0)
        }
        "remove" => {
            let id = positional.first().ok_or_else(|| {
                Error::config("`wan schedule remove` 需要 <id> 参数")
            })?;
            let removed = crate::schedule::remove_schedule(&base, id)?;
            if removed {
                println!("已移除调度：{}", id);
                Ok(0)
            } else {
                eprintln!("未找到调度：{}", id);
                Ok(2)
            }
        }
        "list" => {
            let entries = crate::schedule::list_schedules(&base)?;
            if entries.is_empty() {
                println!("（无调度条目）");
            } else {
                for e in &entries {
                    // 计算下次触发时间
                    let now = std::time::SystemTime::now();
                    let next = e.cron.next_after(now).map(|t| {
                        let secs = t.duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        let ts = jiff::Timestamp::from_second(secs as i64)
                            .unwrap_or_else(|_| jiff::Timestamp::now());
                        ts.to_string()
                    }).unwrap_or_else(|| "不可计算".to_string());

                    println!("{:<16} [{:<14}] {} -> {} (next: {})",
                        e.id, e.cron.raw(), e.workflow_name, e.workflow_path.display(), next);
                }
            }
            Ok(0)
        }
        "start" => {
            let opts = RunOptions {
                max_parallel: None,
                working_dir: Some(base.clone()),
                json_output,
                quiet,
                color,
                no_group,
            };
            crate::schedule::run_daemon(&base, catch_up, None, &opts)
        }
        "run-once" => {
            // 供 service/schtasks 每分钟调用
            let opts = RunOptions {
                max_parallel: None,
                working_dir: Some(base.clone()),
                json_output,
                quiet,
                color,
                no_group,
            };
            crate::schedule::run_once(&base, &opts)
        }
        "service" => {
            // wan schedule service install/remove/status
            let sub2 = positional.first().map(|s| s.as_str()).unwrap_or("");
            match sub2 {
                "install" => {
                    crate::service::install(&base)?;
                    println!("已安装系统服务。");
                    #[cfg(windows)]
                    println!(
                        "Windows schtasks 任务名：{}（每分钟触发）",
                        crate::service::task_name(&base)
                    );
                    #[cfg(unix)]
                    println!(
                        "systemd user unit：{}.timer（每分钟触发）",
                        crate::service::unit_name(&base)
                    );
                    Ok(0)
                }
                "remove" => {
                    crate::service::remove(&base)?;
                    println!("已移除系统服务。");
                    Ok(0)
                }
                "status" => {
                    let s = crate::service::status(&base)?;
                    println!("{s}");
                    Ok(0)
                }
                "" => {
                    eprintln!("用法: wan schedule service install|remove|status");
                    Ok(2)
                }
                other => Err(Error::config(format!(
                    "未知子命令 `wan schedule service {other}`，支持 install / remove / status"
                ))),
            }
        }
        "history" => {
            let filter_id = positional.first().map(|s| s.as_str());
            let records = crate::schedule::read_history(&base, limit)?;
            let filtered: Vec<_> = match filter_id {
                Some(id) => records.into_iter().filter(|r| r.schedule_id == id).collect(),
                None => records,
            };
            if filtered.is_empty() {
                println!("（无历史记录）");
            } else {
                for r in &filtered {
                    let catchup = if r.catch_up { " (catch-up)" } else { "" };
                    println!("{} {} [{}] {} exit={} ({}ms){}",
                        r.ts, r.schedule_id, r.cron_expr, r.workflow, r.exit_code, r.duration_ms, catchup);
                }
            }
            Ok(0)
        }
        other => Err(Error::config(format!(
            "未知子命令 `wan schedule {other}`，支持 add / remove / list / start / run-once / service / history"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_flags_parse() {
        let mut parser = lexopt::Parser::from_iter(vec![
            "wan".to_string(),
            "--json".to_string(),
            "-C".to_string(),
            ".".to_string(),
            "a.yml".to_string(),
        ]);
        let mut flags = Flags::default();
        let file = parse_common_flags(&mut parser, &mut flags).unwrap();
        assert!(flags.json, "json flag not set");
        let cwd = flags.cwd.expect("cwd should be set");
        assert!(
            cwd.to_string_lossy().ends_with('.'),
            "cwd: {}",
            cwd.display()
        );
        assert_eq!(file, Some(PathBuf::from("a.yml")));
    }

    #[test]
    fn no_group_flag_parse() {
        let mut parser = lexopt::Parser::from_iter(vec![
            "wan".to_string(),
            "--no-group".to_string(),
            "a.yml".to_string(),
        ]);
        let mut flags = Flags::default();
        let file = parse_common_flags(&mut parser, &mut flags).unwrap();
        assert!(flags.no_group, "no_group flag not set");
        assert_eq!(file, Some(PathBuf::from("a.yml")));
    }

    fn make_tmp_workflows() -> PathBuf {
        use std::sync::LazyLock;
        static DIR: LazyLock<PathBuf> = LazyLock::new(|| {
            let dir = std::env::temp_dir().join(format!("wan-cli-test-{}", std::process::id()));
            let wf = dir.join(".wan").join("workflows");
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&wf).unwrap();
            let win = "version: 1\njobs:\n  a:\n    steps:\n      - name: x\n        shell: cmd\n        run: echo win\n";
            let unix = "version: 1\njobs:\n  a:\n    steps:\n      - name: x\n        shell: sh\n        run: echo unix\n";
            let plain = "version: 1\njobs:\n  a:\n    steps:\n      - name: x\n        shell: sh\n        run: echo plain\n";
            std::fs::write(wf.join("hello-win.yml"), win).unwrap();
            std::fs::write(wf.join("hello-unix.yml"), unix).unwrap();
            std::fs::write(wf.join("plain.yml"), plain).unwrap();
            dir
        });
        DIR.clone()
    }

    #[test]
    fn resolve_platform_prefers_suffix() {
        let dir = make_tmp_workflows();
        let p = resolve_workflow_file("hello", &dir).unwrap();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let expected = if cfg!(windows) {
            "hello-win.yml"
        } else {
            "hello-unix.yml"
        };
        assert_eq!(name, expected, "平台后缀未优先");
    }

    #[test]
    fn resolve_falls_back_to_plain() {
        let dir = make_tmp_workflows();
        let p = resolve_workflow_file("plain", &dir).unwrap();
        assert_eq!(p.file_name().unwrap().to_string_lossy(), "plain.yml");
    }

    #[test]
    fn resolve_strips_extension() {
        let dir = make_tmp_workflows();
        let p = resolve_workflow_file("hello.yml", &dir).unwrap();
        assert!(
            p.to_string_lossy().contains("hello-"),
            "应解析为平台后缀文件: {}",
            p.display()
        );
    }

    #[test]
    fn resolve_path_argument() {
        let dir = make_tmp_workflows();
        let direct = dir.join(".wan").join("workflows").join("hello-win.yml");
        let p = resolve_workflow_file(&direct.to_string_lossy(), &dir).unwrap();
        assert_eq!(p, direct);
    }

    #[test]
    fn resolve_missing_name_reports_candidates() {
        let dir = make_tmp_workflows();
        let e = resolve_workflow_file("nope", &dir).unwrap_err();
        assert!(e.msg.contains("未找到"), "{e}");
        assert!(
            e.msg.contains("nope-win") || e.msg.contains("nope-unix"),
            "{e}"
        );
    }

    #[test]
    fn resolve_missing_dir_reports_hint() {
        let dir = make_tmp_workflows();
        let p = resolve_workflow_file("hello", &dir.join("elsewhere")).unwrap_err();
        assert!(p.msg.contains("workflows"), "{p}");
    }
}
