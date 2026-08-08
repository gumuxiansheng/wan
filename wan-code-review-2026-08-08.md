# wan 代码审查报告（2026-08-08）

范围：全部 19 个 `.rs` 源文件 + `Cargo.toml`。
结论：`cargo check` 与 `cargo clippy --all-targets` 均**零警告、零错误**通过，整体工程质量较高、测试覆盖也不错。
下面列出的是**逻辑缺陷**与**坏味道**，按严重程度排序。

---

## 🔴 严重（逻辑错误，会影响实际行为）

### 1. Cron 按 **UTC** 解释，而非本地时区 —— 调度会在错误的时间触发
`src/cron.rs:142` `secs_to_components()` 使用 `TimeZone::UTC`：

```rust
let zoned = ts.to_zoned(TimeZone::UTC);
```

`next_after()` 与 `matches_now()` 都依赖它。但本工具是“**本地**工作流执行器”，CLI 帮助里的示例（`0 2 * * *` = 每天 02:00）按用户直觉是**本地墙上时间**。
后果：非 UTC 时区的用户（如 UTC+8），`0 2 * * *` 实际在本地 **10:00** 触发；而且所有 `Event` 时间戳、历史记录也都是 UTC，和用户的本地预期不一致。

**建议**：用本地时区计算。`jiff` 可用 `jiff::tz::offset::utc_offset()` / 系统本地偏移（Windows 上用 `GetTimeZoneInformation`，Unix 上 `localtime`/`jiff::tz`）把 `Timestamp` 转到本地再取 components。或至少显式声明“cron 为 UTC”并在文档/帮助里写明，避免误导。

---

### 2. `next_after` 只扫描 366 天 → `2 月 29 日`等低频 cron 返回 `None`
`src/cron.rs:109`
```rust
const MAX_SCAN: u64 = 366 * 24 * 60; // 366 天的分钟数
```
- `0 0 29 2 *`（闰年 2/29）下一次触发可能远在 ~4 年后（如 2025-03 注册，下一个 2/29 是 2028）。366 天上限太小 → 返回 `None`。
- 更糟的是 `src/schedule.rs:237` 的守护进程逻辑：
```rust
let trigger_time = match earliest_time {
    Some(t) => t,
    None => { eprintln!("所有调度条目均无可触发的未来时间，退出。"); break; }
};
```
当某个条目 `next_after` 返回 `None`（且它是唯一/全部条目时），**整个调度守护进程直接退出**。即使不是唯一条目，该条目 `next_times[i]` 永远停留在 `None`、再也不会被重新求值，等于被静默丢弃。

**建议**：
- 把扫描上限提到 ~1462 天（4 年 + 1 天）以覆盖 2/29；或直接用“逐字段推进”算法替代分钟扫描，避免上限问题。
- 守护进程遇到 `None` 应**跳过该条目**继续其它条目，而不是 break 退出。

---

### 3. `run_once` 的“防重复触发”逻辑有两处缺陷
`src/schedule.rs:327`
```rust
if let Ok(history) = read_history(base, 5) {
    let recent = history.iter().rev().find(|r| r.schedule_id == entry.id);
    if let Some(r) = recent {
        if let Ok(ts) = r.ts.parse::<jiff::Timestamp>() {
            let elapsed = ... - ts.as_second();
            if elapsed < 60 { continue; }   // 60s 内已执行过 → 跳过
        }
    }
}
```

缺陷 A（高频 cron 误跳过）：去重窗口是“距上一条记录写入时间 60s”，而 `run_once` 由 OS 定时器**每分钟**调用一次。`* * * * *` 这种每分钟 cron：上一次运行耗时哪怕 1 秒，本次 `elapsed ≈ 59 < 60` → **被跳过**，导致每分钟任务隔次丢失。

缺陷 B（多调度误重复）：`read_history(base, 5)` 只取全局最近 5 条。当调度数量多、彼此交错时，某 schedule 自己的上一次记录可能已超出这 5 条窗口 → `recent = None` → **再次执行**，造成重复跑。

**建议**：按“触发的分钟”去重，而不是按“记录写入时间 + 固定 60s 窗口”。例如在执行前判断“该 cron 在当前这一分钟是否已跑过”（比较 `triggered_at` 的分钟，或与上次运行的 cron 匹配分钟是否相同），且不限制读取条数（或先按 `schedule_id` 过滤再取最近一条）。

---

### 4. `find_cycle_path` 永远从 `jobs[0]` 开始 DFS → 环路径退化成 `?`
`src/scheduler.rs:62`
```rust
dfs(workflow, 0, &mut visited, &mut path).unwrap_or_else(|| vec!["?".to_string()])
```
若依赖环存在于一个从 `jobs[0]` **不可达**的连通分量中（例如 `a`（无依赖）、`b→c→b`），`check_acyclic` 仍能正确判定“有环”（因为 Kahn 入度统计是全局的），但 `find_cycle_path` 从 `a` 出发 DFS 立刻返回 `None`，最终报错信息变成：
```
检测到依赖环（DAG 不合法）：?
```
丢失了真实环路径，调试时毫无帮助。

**建议**：循环所有 job 作为 DFS 起点，直到找到环为止；或直接在 Kahn 过程中记录环。

---

## 🟠 中等（健壮性与一致性）

### 5. 守护进程一处 `None` 即整体退出（与 #2 同源）
已在 #2 说明：`run_daemon` 在 `earliest_time == None` 时 `break` 退出整个进程。任意一个“未来无可触发时间”的条目都会拖垮全部调度。应改为跳过该条目。

### 6. “失败保留脚本供排查”实际并未生效
`src/executor.rs:371`
```rust
if final_code == 0 {
    let _ = std::fs::remove_file(&script);
}
```
失败时脚本被保留在 `tmp_root` 下——但 `src/engine.rs:84` 在 `run()` 结束时**无条件** `remove_dir_all(tmp_root)`，所以失败的临时脚本在进程退出时被一起删掉，“保留供排查”的注释是空话。

**建议**：失败时不删整个 tmp 目录，或把失败脚本/输出拷贝到稳定目录（如 `.wan/debug/`）后再清理；或至少在失败退出时打印脚本绝对路径让用户知道去哪看。

### 7. Windows `kill_graceful` 实为强制终止，命名误导
`src/platform/windows.rs:123`
```rust
pub fn kill_graceful(&self) {
    unsafe { TerminateJobObject(self.job.as_raw_handle() as HANDLE, 1) };
}
```
Job Object 没有“优雅退出”的等价物（无 WM_CLOSE / SIGTERM），`Graceful` 与 `Force` 在 Windows 上完全一样（代码里也注释了“Windows 上无第二档”）。把硬终止称为 `Graceful` 会误导调用方认为有优雅路径。建议重命名（如 `kill`）或在文档里明确“Windows 下无优雅终止档”。

### 8. Unix 信号处理器用 `libc::signal` 而非 `sigaction`
`src/platform/unix.rs:77`
```rust
libc::signal(libc::SIGINT, h);
libc::signal(libc::SIGTERM, h);
```
`signal()` 在不同 libc 上语义不一致，且中断处理里未设 `SA_RESTART`。虽然当前 handler 只置标志、且子进程在独立进程组（收不到 SIGINT），实际能工作，但为健壮应改用 `sigaction` 并显式设置标志位。

### 9. `interrupted()` 标志是进程级全局、永不复位
`src/platform/mod.rs:23` 的 `INTERRUPTED` 是静态 `AtomicBool`，被 Ctrl+C 置位后永不清除。
- 单次 `run`：没问题。
- 守护进程（同一进程长期运行）：一旦触发中断，进程通常已退出，无碍。
- 但若未来想让守护进程在“软信号”后继续、或在同一进程内连续 `run` 多次（如测试/库用法），该标志会残留导致后续所有 job 被当作中断跳过。**建议**：库形态调用时提供复位入口，或把中断状态做成每次 `run` 的局部状态。

---

## 🟡 坏味道 / 轻微问题

### 10. `HumanSink::StepOutput` 丢弃了 `job`/`step` 上下文
`src/output.rs:63`
```rust
self.print(format!("    {prefix}{line}"));
let _ = (job, step);   // 丢弃
```
嵌套/并行输出时，单看一行 stdout 不知道它属于哪个 job/step。建议至少在该 step 首行标注一次 job/step，或在每行前缀里带上（可选、受 `quiet` 控制）。

### 11. `run_daemon` 中 `stop`/`stop_clone` 是冗余死状态
`src/schedule.rs:197-221`：`stop` 这个 `Arc<AtomicBool>` 只被 `platform::interrupted()` 镜像，循环条件本可直接写 `while !crate::platform::interrupted()`。属多余状态，可删。

### 12. `interpolate` 对未闭合的 `${VAR` 静默保留且不告警
`src/expr.rs:28`：当 `after.find('}') == None` 时整段原样保留，但**没有任何 warning**（与“未定义变量告警”行为不一致）。建议至少 warn 一次“未闭合的 `${`”。

### 13. `parse_output_file` 每次成功 step 都尝试读不存在的 `out_file`
`src/executor.rs:392`：脚本若从未写 `$WAN_OUTPUT`，`read_to_string` 失败→返回空。无害，但每个成功 step 都多一次无意义 syscall。可先 `path.exists()` 判断。

### 14. `warn()` 在 `--json` / `--quiet` 下仍写 stderr
`src/executor.rs:72`：插值告警等直接 `eprintln!`，会混入 JSONL 运行时的 stderr。JSON 模式通常希望 stdout 纯净，建议 `quiet`/`json` 下把 warning 也走事件通道（或明确这是预期）。

### 15. `merge_env` 是 O(n·m) 多层嵌套
`src/model.rs:7`：每层每个 key 都线性查找。当前规模无碍；若 env 很大或嵌套深，可改用 `HashMap` 归并。属可扩展性提示，非缺陷。

### 16. `Field` 位集在 `weekday` 上残留 bit 7
`src/cron.rs:78`：周字段把 `7` 映射到 `0`，但 bit 7 未被清除（`matches_timestamp` 只查 0–6，所以无害）。代码注释也称“无害”，但属脏数据，建议 `weekday.clear_bit(7)` 或直接只 set(0)。

### 17. `classify_scalar` 把 `0123` 当十进制 123
`src/parser.rs:63`：YAML 1.1 中前导 0 是八进制，这里统一当十进制。属有意的简化，建议加一行注释说明，避免以后踩坑。

### 18. `load_file` 的 `source` 已只是文件名 stem，却又被 `workflow_name` 二次 `file_stem`
`src/parser.rs:551` 与 `src/engine.rs:23`：`source` 存的是 stem 字符串，`workflow_name` 再对它做一次 `Path::new(...).file_stem()`——冗余但无害。

### 19. job 线程“先全部 spawn 再限流”
`src/scheduler.rs:122`：`schedule()` 对每个 job 立即 `spawn` 一个 OS 线程，信号量只在“执行”阶段限流，不限制线程数量。超大 workflow（上千 job）会一次性创建上千线程。当前本地场景可接受，但属扩展性坏味道；可考虑用线程池 / 工作窃取。

---

## ✅ 做得好的地方（顺便肯定）
- 依赖环检测（Kahn）+ 平台 shell 校验在 `validate` 阶段前置，失败快速、错误信息带行列号。
- 子进程树终止：Windows Job Object（`KILL_ON_JOB_CLOSE`）+ Unix `setpgid`/`killpg`，思路正确。
- 事件走 mpsc 通道 + 收集线程转发 `EventSink`，运行期与输出解耦，JSON/Human 双 sink 干净。
- 解析层手写 schema 校验 + 显式拒绝 `runs-on`/`uses`/`matrix` 等不支持字段，错误信息友好且符合“显式失败 > 静默降级”原则。
- 测试覆盖较全（parser / scheduler / cron / hook / schedule / cli 均有单元测试）。

---

## 优先级建议
1. 先修 **#1（时区）**、**#2（366 天上限 + 守护进程退出）**、**#3（run_once 去重）**、**#4（环路径）**——这 4 个会直接导致功能“看起来在跑但结果不对/不跑”。
2. 其次 **#5/#6/#7/#8/#9** 提升健壮性。
3. 其余坏味道按需清理。
