//! 引擎（spec §14.1）：可重入 `run(workflow, opts, sink) -> 退出码`
//! 校验 → 临时目录 → RunStart → mpsc 事件通道 → DAG 调度 → RunEnd

use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::Instant;

use crate::error::Result;
use crate::executor::{check_platform_shells, RunCtx};
use crate::model::{now_rfc3339, Event, EventSink, RunOptions, Workflow};
use crate::scheduler::{check_acyclic, RunStatus};

/// 校验（parse 之外的部分）：DAG 无环 + 平台 shell 支持
pub fn validate(workflow: &Workflow) -> Result<()> {
    check_acyclic(workflow)?;
    for job in &workflow.jobs {
        check_platform_shells(job)?;
    }
    Ok(())
}

/// workflow 文件主文件名（去扩展名，§7.3）
pub fn workflow_name(workflow: &Workflow) -> String {
    Path::new(&workflow.source)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "workflow".to_string())
}

/// 执行 workflow，返回退出码：0 成功 / 1 失败 / 130 中断（§7.2）
pub fn run(
    workflow: &Workflow,
    opts: &RunOptions,
    sink: Box<dyn EventSink + Send>,
) -> Result<i32> {
    validate(workflow)?;

    // 复位中断标志（库形态连续调用不残留）
    crate::platform::set_interrupted(false);

    let start = Instant::now();
    let tmp_root = make_tmp_dir()?;
    let ctx = RunCtx {
        opts: opts.clone(),
        tmp_root: tmp_root.clone(),
        workflow_wd: workflow.working_directory.clone(),
    };

    let (tx, rx) = mpsc::channel::<Event>();

    // 收集线程：通道事件 → EventSink（§14.3 mpsc 推送），排空后归还 sink
    let collector = std::thread::spawn(move || {
        let mut sink = sink;
        for ev in rx {
            sink.emit(ev);
        }
    });

    let _ = tx.send(Event::RunStart {
        workflow: workflow_name(workflow),
        ts: now_rfc3339(),
    });

    let status = {
        let wf = Arc::new(workflow.clone());
        let ctx_arc = Arc::new(ctx);
        crate::scheduler::schedule(&wf, &ctx_arc, &tx)
    };

    let code = match status {
        RunStatus::Success => 0,
        RunStatus::Failed => 1,
        RunStatus::Interrupted => 130,
    };

    // RunEnd 走同一通道，保证事件顺序（收集线程 FIFO 排空）
    let _ = tx.send(Event::RunEnd {
        code: code as u32,
        duration_ms: start.elapsed().as_millis() as u64,
        ts: now_rfc3339(),
    });
    drop(tx);

    let _ = collector.join();

    // 清理临时目录：成功时删除，失败时保留供排查
    if code == 0 {
        let _ = std::fs::remove_dir_all(&tmp_root);
    } else {
        eprintln!("临时脚本保留在：{}", tmp_root.display());
    }

    Ok(code)
}

fn make_tmp_dir() -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!(
        "wan-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
