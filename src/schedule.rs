//! Cron 调度守护进程（spec §4.7 F-CRON-1/3/4，§15.5）
//!
//! 常驻进程：到点调用 engine::run，记录执行历史（JSONL）
//! 错过窗口策略：默认跳过，--catch-up 可选补跑最近一次

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crate::cron::CronExpr;
use crate::engine;
use crate::error::{Error, Result};
use crate::model::RunOptions;
use crate::output::HumanSink;
use crate::parser::load_file;

/// 调度条目
#[derive(Debug, Clone)]
pub struct ScheduleEntry {
    pub id: String,
    pub cron: CronExpr,
    pub workflow_path: PathBuf,
    pub workflow_name: String,
}

/// 执行历史记录（JSONL 一行）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HistoryRecord {
    pub ts: String,
    pub schedule_id: String,
    pub workflow: String,
    pub cron_expr: String,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub triggered_at: String,
    pub catch_up: bool,
}

/// 调度器状态文件路径
pub fn schedules_dir(base: &Path) -> PathBuf {
    base.join(".wan").join("schedules")
}

pub fn history_file(base: &Path) -> PathBuf {
    schedules_dir(base).join("history.jsonl")
}

pub fn schedules_file(base: &Path) -> PathBuf {
    schedules_dir(base).join("schedules.json")
}

/// 注册一个调度条目
pub fn add_schedule(base: &Path, id: &str, cron_expr: &str, workflow_path: &Path) -> Result<()> {
    let dir = schedules_dir(base);
    fs::create_dir_all(&dir)?;

    let path = schedules_file(base);
    let mut entries = load_schedules(base)?;
    if entries.iter().any(|e| e.0 == id) {
        return Err(Error::config(format!("调度 ID `{id}` 已存在")));
    }
    entries.push((id.to_string(), cron_expr.to_string(), workflow_path.to_string_lossy().to_string()));

    let json = serde_json::to_string_pretty(&entries)?;
    fs::write(&path, json)?;
    Ok(())
}

/// 移除一个调度条目
pub fn remove_schedule(base: &Path, id: &str) -> Result<bool> {
    let entries = load_schedules(base)?;
    let original_len = entries.len();
    let filtered: Vec<_> = entries.into_iter().filter(|(eid, _, _)| eid != id).collect();

    if filtered.len() == original_len {
        return Ok(false);
    }

    let path = schedules_file(base);
    let json = serde_json::to_string_pretty(&filtered)?;
    fs::write(&path, json)?;
    Ok(true)
}

/// 列出所有调度条目
pub fn list_schedules(base: &Path) -> Result<Vec<ScheduleEntry>> {
    let raw = load_schedules(base)?;
    let mut out = Vec::new();
    for (id, cron_str, wf_path) in raw {
        let cron = match CronExpr::parse(&cron_str) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let path = PathBuf::from(&wf_path);
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| wf_path.clone());
        out.push(ScheduleEntry {
            id,
            cron,
            workflow_path: path,
            workflow_name: name,
        });
    }
    Ok(out)
}

/// 加载原始调度数据 (id, cron_str, workflow_path)
fn load_schedules(base: &Path) -> Result<Vec<(String, String, String)>> {
    let path = schedules_file(base);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&path)
        .map_err(|e| Error::io(format!("读取调度文件失败：{e}")))?;
    if content.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&content)
        .map_err(|e| Error::config(format!("调度文件格式错误：{e}")))
}

/// 追加一条历史记录
pub fn append_history(base: &Path, record: &HistoryRecord) -> Result<()> {
    let dir = schedules_dir(base);
    fs::create_dir_all(&dir)?;

    let path = history_file(base);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| Error::io(format!("打开历史文件失败：{e}")))?;

    let line = serde_json::to_string(record)
        .map_err(|e| Error::config(format!("序列化历史记录失败：{e}")))?;
    writeln!(file, "{line}").map_err(|e| Error::io(format!("写入历史文件失败：{e}")))?;
    Ok(())
}

/// 读取历史记录（最新 N 条）
pub fn read_history(base: &Path, limit: usize) -> Result<Vec<HistoryRecord>> {
    let path = history_file(base);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&path)
        .map_err(|e| Error::io(format!("读取历史文件失败：{e}")))?;
    let lines: Vec<&str> = content.lines().collect();
    let start = if lines.len() > limit { lines.len() - limit } else { 0 };
    let mut records = Vec::new();
    for line in &lines[start..] {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(r) = serde_json::from_str::<HistoryRecord>(line) {
            records.push(r);
        }
    }
    Ok(records)
}

/// 运行调度器守护进程
pub fn run_daemon(
    base: &Path,
    catch_up: bool,
    single: Option<&str>, // 单次执行某个 schedule id（调试用）
    opts: &RunOptions,
) -> Result<i32> {
    let entries = list_schedules(base)?;
    if entries.is_empty() {
        eprintln!("无调度条目。使用 `wan schedule add <id> <cron> <workflow>` 添加。");
        return Ok(0);
    }

    let entries: Vec<ScheduleEntry> = if let Some(id) = single {
        entries.into_iter().filter(|e| e.id == id).collect()
    } else {
        entries
    };

    if entries.is_empty() {
        return Err(Error::config(format!("未找到调度 ID `{single:?}`")));
    }

    // 计算每个条目的下次触发时间
    let now = SystemTime::now();
    let mut next_times: Vec<Option<SystemTime>> = entries
        .iter()
        .map(|e| e.cron.next_after(now))
        .collect();

    let stop = Arc::new(AtomicBool::new(false));

    // 安装信号处理
    crate::platform::install_interrupt_handler();

    let stop_clone = Arc::clone(&stop);
    // 信号处理通过 platform 的 INTERRUPTED flag
    // 主循环检查 stop flag

    eprintln!("wan 调度器已启动（{} 个条目）", entries.len());
    for e in &entries {
        eprintln!(
            "  {} [{}] {} -> {}",
            e.id,
            e.cron.raw(),
            e.workflow_name,
            e.workflow_path.display()
        );
    }
    eprintln!("按 Ctrl+C 停止。");

    while !stop_clone.load(Ordering::Relaxed) {
        if crate::platform::interrupted() {
            stop_clone.store(true, Ordering::Relaxed);
            break;
        }

        let now = SystemTime::now();
        let mut earliest_idx: Option<usize> = None;
        let mut earliest_time: Option<SystemTime> = None;

        for (i, nt) in next_times.iter().enumerate() {
            if let Some(t) = nt {
                if earliest_time.is_none() || *t < earliest_time.unwrap() {
                    earliest_time = Some(*t);
                    earliest_idx = Some(i);
                }
            }
        }

        let trigger_time = match earliest_time {
            Some(t) => t,
            None => {
                eprintln!("所有调度条目均无可触发的未来时间，退出。");
                break;
            }
        };

        if now >= trigger_time {
            // 触发
            let entry = &entries[earliest_idx.unwrap()];

            // 检查错过窗口
            let missed = now.duration_since(trigger_time).unwrap_or_default();
            let is_catch_up = missed > Duration::from_secs(60);

            if is_catch_up && !catch_up {
                eprintln!(
                    "[{}] 错过触发窗口 {}s，跳过（--catch-up 可补跑）",
                    entry.id,
                    missed.as_secs()
                );
            } else {
                let trigger_ts = crate::model::now_rfc3339();
                let start = Instant::now();

                eprintln!(
                    "[{}] {} 触发执行：{}",
                    entry.id,
                    if is_catch_up { "补跑" } else { "" },
                    entry.workflow_name
                );

                let exit_code = run_workflow(&entry.workflow_path, opts);
                let duration_ms = start.elapsed().as_millis() as u64;

                let record = HistoryRecord {
                    ts: crate::model::now_rfc3339(),
                    schedule_id: entry.id.clone(),
                    workflow: entry.workflow_name.clone(),
                    cron_expr: entry.cron.raw().to_string(),
                    exit_code,
                    duration_ms,
                    triggered_at: trigger_ts,
                    catch_up: is_catch_up,
                };

                if let Err(e) = append_history(base, &record) {
                    eprintln!("[{}] 历史记录写入失败：{}", entry.id, e);
                }

                eprintln!(
                    "[{}] 执行完成：退出码 {}，耗时 {}ms",
                    entry.id, exit_code, duration_ms
                );
            }

            // 计算下次触发
            next_times[earliest_idx.unwrap()] = entry.cron.next_after(now);
        } else {
            // 等待到最早触发时间（最多等 60 秒后重新检查，避免信号响应延迟）
            let wait = trigger_time.duration_since(now).unwrap_or_default();
            let sleep = std::cmp::min(wait, Duration::from_secs(60));
            std::thread::sleep(sleep);
        }
    }

    eprintln!("wan 调度器已停止。");
    Ok(0)
}

/// 单次扫描执行（供 service/schtasks 每分钟调用）
/// 检查所有调度，执行到点的 workflow，记录历史
pub fn run_once(base: &Path, opts: &RunOptions) -> Result<i32> {
    let entries = list_schedules(base)?;
    if entries.is_empty() {
        return Ok(0);
    }

    let now = SystemTime::now();
    let mut any_executed = false;
    let mut last_exit = 0;

    for entry in &entries {
        // 检查当前时间是否匹配 cron 表达式
        if !entry.cron.matches_now(now) {
            continue;
        }

        // 检查最近 60 秒内是否已执行过（防止重复触发）
        if let Ok(history) = read_history(base, 5) {
            let recent = history.iter().rev().find(|r| r.schedule_id == entry.id);
            if let Some(r) = recent {
                if let Ok(ts) = r.ts.parse::<jiff::Timestamp>() {
                    let elapsed = now.duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0) - ts.as_second();
                    if elapsed < 60 {
                        continue; // 60 秒内已执行过
                    }
                }
            }
        }

        any_executed = true;
        let trigger_ts = crate::model::now_rfc3339();
        let start = Instant::now();

        eprintln!("[{}] 触发执行：{}", entry.id, entry.workflow_name);
        let exit_code = run_workflow(&entry.workflow_path, opts);
        let duration_ms = start.elapsed().as_millis() as u64;

        let record = HistoryRecord {
            ts: crate::model::now_rfc3339(),
            schedule_id: entry.id.clone(),
            workflow: entry.workflow_name.clone(),
            cron_expr: entry.cron.raw().to_string(),
            exit_code,
            duration_ms,
            triggered_at: trigger_ts,
            catch_up: false,
        };

        if let Err(e) = append_history(base, &record) {
            eprintln!("[{}] 历史记录写入失败：{}", entry.id, e);
        }

        eprintln!("[{}] 执行完成：退出码 {}，耗时 {}ms", entry.id, exit_code, duration_ms);
        last_exit = exit_code;
    }

    if !any_executed {
        eprintln!("无调度到点。");
    }

    Ok(last_exit)
}

fn run_workflow(path: &Path, opts: &RunOptions) -> i32 {
    let wf = match load_file(path) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("加载 workflow 失败：{e}");
            return 2;
        }
    };

    if let Err(e) = engine::validate(&wf) {
        eprintln!("校验失败：{e}");
        return 2;
    }

    let sink: Box<dyn crate::model::EventSink + Send> = if opts.json_output {
        Box::new(crate::output::JsonSink::new(std::io::stdout()))
    } else {
        Box::new(HumanSink::new(opts))
    };

    match engine::run(&wf, opts, sink) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("执行失败：{e}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn tmp_base() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wan-cron-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn add_and_list_schedule() {
        let base = tmp_base();
        add_schedule(&base, "daily", "0 2 * * *", Path::new("deploy.yml")).unwrap();

        let entries = list_schedules(&base).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "daily");
        assert_eq!(entries[0].cron.raw(), "0 2 * * *");
    }

    #[test]
    fn add_duplicate_id_fails() {
        let base = tmp_base();
        add_schedule(&base, "daily", "0 2 * * *", Path::new("a.yml")).unwrap();
        let err = add_schedule(&base, "daily", "0 3 * * *", Path::new("b.yml")).unwrap_err();
        assert!(err.msg.contains("已存在"), "{err}");
    }

    #[test]
    fn remove_schedule_works() {
        let base = tmp_base();
        add_schedule(&base, "daily", "0 2 * * *", Path::new("a.yml")).unwrap();
        add_schedule(&base, "hourly", "0 * * * *", Path::new("b.yml")).unwrap();

        let removed = remove_schedule(&base, "daily").unwrap();
        assert!(removed);

        let entries = list_schedules(&base).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "hourly");
    }

    #[test]
    fn remove_nonexistent_returns_false() {
        let base = tmp_base();
        let removed = remove_schedule(&base, "nope").unwrap();
        assert!(!removed);
    }

    #[test]
    fn history_append_and_read() {
        let base = tmp_base();
        let r1 = HistoryRecord {
            ts: "2026-01-01T00:00:00Z".to_string(),
            schedule_id: "daily".to_string(),
            workflow: "deploy".to_string(),
            cron_expr: "0 2 * * *".to_string(),
            exit_code: 0,
            duration_ms: 100,
            triggered_at: "2026-01-01T02:00:00Z".to_string(),
            catch_up: false,
        };
        append_history(&base, &r1).unwrap();

        let r2 = HistoryRecord {
            ts: "2026-01-02T00:00:00Z".to_string(),
            schedule_id: "daily".to_string(),
            workflow: "deploy".to_string(),
            cron_expr: "0 2 * * *".to_string(),
            exit_code: 1,
            duration_ms: 200,
            triggered_at: "2026-01-02T02:00:00Z".to_string(),
            catch_up: false,
        };
        append_history(&base, &r2).unwrap();

        let records = read_history(&base, 10).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].exit_code, 0);
        assert_eq!(records[1].exit_code, 1);
    }

    #[test]
    fn history_read_limit() {
        let base = tmp_base();
        for i in 0..5 {
            let r = HistoryRecord {
                ts: format!("2026-01-0{i}T00:00:00Z"),
                schedule_id: "daily".to_string(),
                workflow: "w".to_string(),
                cron_expr: "* * * * *".to_string(),
                exit_code: i,
                duration_ms: i as u64 * 100,
                triggered_at: format!("2026-01-0{i}T00:00:00Z"),
                catch_up: false,
            };
            append_history(&base, &r).unwrap();
        }

        let records = read_history(&base, 3).unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].exit_code, 2);
        assert_eq!(records[2].exit_code, 4);
    }

    #[test]
    fn history_empty_when_no_file() {
        let base = tmp_base();
        let records = read_history(&base, 10).unwrap();
        assert!(records.is_empty());
    }
}
