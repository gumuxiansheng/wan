//! DAG 调度（spec §14.2）：Kahn 拓扑 + 环检测 + work-list 就绪队列 + 全局 max_parallel 信号量
//! 事件经 mpsc 通道推送（spec §14.3），由 engine 的收集线程转发到 EventSink

use std::collections::VecDeque;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};

use crate::error::{Error, Result};
use crate::executor::{self, RunCtx};
use crate::model::{Event, Job, Outcome, Workflow};

/// Kahn 拓扑排序；存在环时返回错误并打印环路径（F-SCHED-1）
pub fn check_acyclic(workflow: &Workflow) -> Result<()> {
    let n = workflow.jobs.len();
    // 入度 = needs 列表长度（parser 已保证引用合法，F-PARSE-8）
    let mut indeg: Vec<usize> = workflow.jobs.iter().map(|j| j.needs.len()).collect();
    let mut queue: VecDeque<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    let mut processed = 0usize;
    while let Some(i) = queue.pop_front() {
        processed += 1;
        for (k, job) in workflow.jobs.iter().enumerate() {
            if job.needs.iter().any(|nd| nd == &workflow.jobs[i].id) {
                indeg[k] -= 1;
                if indeg[k] == 0 {
                    queue.push_back(k);
                }
            }
        }
    }
    if processed == n {
        return Ok(());
    }
    let cycle = find_cycle_path(workflow);
    Err(Error::config(format!(
        "检测到依赖环（DAG 不合法）：{}",
        cycle.join(" -> ")
    )))
}

fn find_cycle_path(workflow: &Workflow) -> Vec<String> {
    let n = workflow.jobs.len();
    let mut visited = vec![false; n];
    let mut on_stack = vec![false; n];
    let mut path: Vec<String> = Vec::new();

    fn dfs(
        workflow: &Workflow,
        idx: usize,
        visited: &mut [bool],
        on_stack: &mut [bool],
        path: &mut Vec<String>,
    ) -> Option<Vec<String>> {
        visited[idx] = true;
        on_stack[idx] = true;
        path.push(workflow.jobs[idx].id.clone());

        for need in &workflow.jobs[idx].needs {
            if let Some(k) = workflow.jobs.iter().position(|j| &j.id == need) {
                if !visited[k] {
                    if let Some(c) = dfs(workflow, k, visited, on_stack, path) {
                        return Some(c);
                    }
                } else if on_stack[k] {
                    // 找到环：从 path 中环起点开始截取
                    let pos = path.iter().position(|p| p == &workflow.jobs[k].id)?;
                    let mut cycle = path[pos..].to_vec();
                    cycle.push(workflow.jobs[k].id.clone());
                    return Some(cycle);
                }
            }
        }

        path.pop();
        on_stack[idx] = false;
        None
    }

    // 遍历所有节点作为起点（覆盖从 jobs[0] 不可达的环）
    for i in 0..n {
        if !visited[i] {
            if let Some(c) = dfs(workflow, i, &mut visited, &mut on_stack, &mut path) {
                return c;
            }
        }
    }
    vec!["?".to_string()]
}

// ---------- 执行期 ----------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RunStatus {
    Success,
    Failed,
    Interrupted,
}

struct SchedulerShared {
    states: Mutex<Vec<Option<Outcome>>>,
    cv: Condvar,
}

/// 全局信号量（--max-parallel N，默认无上限；作用于整个 run，§14.2）
struct Semaphore {
    cap: usize,
    cur: Mutex<usize>,
    cv: Condvar,
}

impl Semaphore {
    fn new(cap: usize) -> Self {
        Semaphore { cap, cur: Mutex::new(0), cv: Condvar::new() }
    }

    /// 返回 false 表示被中断
    fn acquire(&self) -> bool {
        let mut cur = self.cur.lock().unwrap();
        loop {
            if crate::platform::interrupted() {
                return false;
            }
            if *cur < self.cap {
                *cur += 1;
                return true;
            }
            cur = self.cv.wait(cur).unwrap();
        }
    }

    fn release(&self) {
        let mut cur = self.cur.lock().unwrap();
        *cur -= 1;
        self.cv.notify_one();
    }
}

/// 就绪队列调度：job 在 needs 全部结算后入队；max_parallel 全局信号量限流
pub fn schedule(workflow: &Arc<Workflow>, ctx: &Arc<RunCtx>, tx: &Sender<Event>) -> RunStatus {
    let shared = Arc::new(SchedulerShared {
        states: Mutex::new(vec![None; workflow.jobs.len()]),
        cv: Condvar::new(),
    });
    let sem = Arc::new(Semaphore::new(ctx.opts.max_parallel.unwrap_or(usize::MAX)));

    let mut handles = Vec::new();
    for (idx, job) in workflow.jobs.iter().enumerate() {
        let tx = tx.clone();
        let shared = Arc::clone(&shared);
        let sem = Arc::clone(&sem);
        let wf = Arc::clone(workflow);
        let ctx = Arc::clone(ctx);
        let job = job.clone();
        let handle = std::thread::spawn(move || {
            run_one_job(&wf, idx, &job, &ctx, &shared, &sem, tx);
        });
        handles.push(handle);
    }
    for h in handles {
        let _ = h.join();
    }

    let states = shared.states.lock().unwrap();
    if crate::platform::interrupted() {
        RunStatus::Interrupted
    } else if states.contains(&Some(Outcome::Failure)) {
        RunStatus::Failed
    } else {
        RunStatus::Success
    }
}

fn run_one_job(
    workflow: &Arc<Workflow>,
    idx: usize,
    job: &Job,
    ctx: &Arc<RunCtx>,
    shared: &Arc<SchedulerShared>,
    sem: &Arc<Semaphore>,
    tx: Sender<Event>,
) {
    // 1) 等待 needs 全部结算（work-list 语义，§14.2）
    {
        let mut states = shared.states.lock().unwrap();
        loop {
            if crate::platform::interrupted() {
                *states.get_mut(idx).unwrap() = Some(Outcome::Skipped);
                shared.cv.notify_all();
                return;
            }
            let deps_settled = job.needs.iter().all(|need| {
                workflow
                    .jobs
                    .iter()
                    .position(|j| &j.id == need)
                    .map(|k| states[k].is_some())
                    .unwrap_or(true)
            });
            if deps_settled {
                break;
            }
            states = shared.cv.wait(states).unwrap();
        }
    }

    // 2) 全局信号量槽位
    if !sem.acquire() {
        let mut states = shared.states.lock().unwrap();
        *states.get_mut(idx).unwrap() = Some(Outcome::Skipped);
        shared.cv.notify_all();
        return;
    }

    // 3) 依赖结算 → job 级 if 求值（§6.2）；默认 success() 语义
    let deps: Vec<Outcome> = {
        let states = shared.states.lock().unwrap();
        job.needs
            .iter()
            .map(|need| {
                workflow
                    .jobs
                    .iter()
                    .position(|j| &j.id == need)
                    .and_then(|k| states[k])
                    .unwrap_or(Outcome::Success)
            })
            .collect()
    };
    let skip = match &job.if_condition {
        Some(e) => {
            let env_raw = crate::model::merge_env(&[&workflow.env, &job.env]);
            let eval_ctx = crate::expr::EvalCtx::new(&deps, &env_raw);
            !e.eval(&eval_ctx)
        }
        // 默认：全部依赖 success 才执行（跳过传播，§6.2）
        None => deps.iter().any(|o| *o != Outcome::Success),
    };

    let outcome;
    if skip {
        // 被跳过：不发射 JobStart/StepStart（§7.3）
        outcome = Outcome::Skipped;
    } else {
        let _ = tx.send(Event::JobStart { job: job.id.clone(), ts: crate::model::now_rfc3339() });
        let result = executor::run_job(job, &workflow.env, ctx, &tx, idx);
        outcome = result.outcome;
        let _ = tx.send(Event::JobEnd {
            job: job.id.clone(),
            code: result.code,
            duration_ms: result.duration_ms,
        });
        if result.interrupted {
            crate::platform::set_interrupted(true);
        }
    }

    {
        let mut states = shared.states.lock().unwrap();
        *states.get_mut(idx).unwrap() = Some(outcome);
        shared.cv.notify_all();
    }
    sem.release();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::load_from_str;

    fn wf(s: &str) -> Workflow {
        load_from_str(s, "test").unwrap()
    }

    #[test]
    fn acyclic_ok() {
        let w = wf(
            "version: 1\njobs:\n  a:\n    steps:\n      - run: x\n        shell: sh\n  b:\n    needs: [a]\n    steps:\n      - run: y\n        shell: sh\n",
        );
        assert!(check_acyclic(&w).is_ok());
    }

    #[test]
    fn cycle_detected_with_path() {
        let w = wf(
            "version: 1\njobs:\n  a:\n    needs: [b]\n    steps:\n      - run: x\n        shell: sh\n  b:\n    needs: [a]\n    steps:\n      - run: y\n        shell: sh\n",
        );
        let e = check_acyclic(&w).unwrap_err();
        assert!(e.msg.contains("环"), "{e}");
        assert!(e.msg.contains("a"), "{e}");
        assert!(e.msg.contains("b"), "{e}");
    }

    #[test]
    fn self_cycle() {
        let w = wf(
            "version: 1\njobs:\n  a:\n    needs: [a]\n    steps:\n      - run: x\n        shell: sh\n",
        );
        assert!(check_acyclic(&w).is_err());
    }
}
