# wan cron 调度器实现报告

**日期**: 2026-08-05
**Commit**: `6a69f6a`
**状态**: ✅ 完成

## 目标

为 wan v0.2 实现 cron 调度功能，支持：
1. 标准 5 字段 cron 表达式解析
2. 常驻调度守护进程（到点触发 workflow 执行）
3. 执行历史持久化（JSONL）
4. CLI `schedule` 子命令（add/remove/list/start/history）

## 新增文件

### src/cron.rs（~350 行）
手写标准 5 字段 cron 表达式解析器，零新依赖。

**支持语法**：
- `*` — 任意值
- `N` — 具体值
- `N-M` — 范围
- `N,M,L` — 列表
- `*/N` — 步长
- `N-M/S` — 范围步长

**数据结构**：
- `Field`：u64 bit-set（分钟 0-59 需 60 bit，u32 不够）
- `CronExpr`：5 个 Field（minute/hour/day/month/weekday）+ raw 字符串

**核心方法**：
- `CronExpr::parse(s) -> Result<CronExpr>` — 解析并校验
- `CronExpr::next_after(now: SystemTime) -> Option<SystemTime>` — 逐分钟扫描下一个触发时间点

**字段范围**：
- minute: 0-59
- hour: 0-23
- day: 1-31
- month: 1-12
- weekday: 0-6（0=周日，7 也接受为周日）

### src/schedule.rs（~400 行）
常驻调度守护进程 + JSONL 历史持久化。

**数据结构**：
- `ScheduleEntry`：id / cron_expr / workflow_path / workflow_name
- `HistoryRecord`：ts / schedule_id / workflow / cron_expr / exit_code / duration_ms / triggered_at / catch_up

**存储路径**：
- 调度表：`.wan/schedules.json`（JSON 数组）
- 历史记录：`.wan/history.jsonl`（每行一条 JSON）

**公共函数**：
- `add_schedule(base, id, cron_expr, workflow_path)` — 添加调度（重复 id 报错）
- `remove_schedule(base, id) -> bool` — 移除调度
- `list_schedules(base) -> Vec<ScheduleEntry>` — 列出全部调度
- `read_history(base, limit) -> Vec<HistoryRecord>` — 读取历史（默认最近 20 条）
- `append_history(base, record)` — 追加历史记录
- `run_daemon(base, catch_up, until, opts) -> Result<i32>` — 常驻守护进程

**守护进程逻辑**：
1. 加载 schedules.json
2. 计算每个调度的下次触发时间
3. sleep 到最近的触发时间
4. 到点后调用 `engine::run(workflow, opts, sink)`
5. 记录执行结果到 history.jsonl
6. 错过窗口：默认跳过；`--catch-up` 时补跑最后一次
7. 循环直到 Ctrl+C 或 `until` 时间

## CLI 接入

### `wan schedule add <id> <cron-expr> <workflow> [-C <dir>]`
添加调度条目。校验 cron 表达式合法性，workflow 支持短名解析。

### `wan schedule remove <id> [-C <dir>]`
移除调度。不存在返回退出码 2。

### `wan schedule list [-C <dir>]`
列出所有调度，显示下次触发时间。

### `wan schedule start [-C <dir>] [--catch-up] [--json] [--quiet] [--no-color]`
启动常驻调度守护进程。Ctrl+C 优雅退出。

### `wan schedule history [<id>] [-C <dir>] [--limit N]`
查看执行历史。可选按 id 过滤，默认最近 20 条。

## 修改文件

- `src/lib.rs`：注册 cron + schedule 模块
- `src/error.rs`：新增 `impl From<serde_json::Error> for Error`
- `src/cli.rs`：
  - USAGE 文档新增 schedule 子命令说明
  - dispatch 新增 `schedule` 分支
  - 新增 `cmd_schedule()` 函数（~100 行）

## 测试

- **85 tests 全通过**（74 单元 + 11 集成），比之前增加 24 个测试
- cron 解析器 12 个测试：基本值/范围/列表/步长/范围步长/越界/字段数错误/闰年/跨小时/跨天/周日=7
- schedule 模块 7 个测试：add_and_list/duplicate_id_fails/remove/nonexistent/history_append_read/empty_history/history_limit
- clippy 零 warning

## 构建产物

- release 体积：646,144 字节 = 631KB（限制 6MB，余量 5.3MB）
- 相比 v0.1（587KB）增加 ~44KB

## 端到端验证

```
> wan schedule add daily "0 2 * * *" test -C $tmp
已添加调度：daily [0 2 * * *] -> ...test-win.yml

> wan schedule list -C $tmp
daily            [0 2 * * *     ] test-win -> ...test-win.yml (next: 2026-08-05T02:00:00Z)

> wan schedule remove daily -C $tmp
已移除调度：daily

> wan schedule list -C $tmp
（无调度条目）
```

## 设计决策

1. **u64 bit-set**：分钟 0-59 需 60 bit，u32 不够（曾用 u32 导致 4 个测试失败）
2. **逐分钟扫描**：next_after 从当前时间逐分钟检查，最多扫描 366*24*60 次（约一年），性能足够
3. **JSONL 历史**：不引 SQLite 控体积，每行一条 JSON 追加写入
4. **错过窗口策略**：默认跳过（避免重复执行堆积），`--catch-up` 补跑最后一次
5. **schedule.json 单文件**：调度列表用 JSON 数组，简单直观，手动可编辑

## v0.2 后续待办

- [ ] Windows sc.exe / Linux systemd 集成（注册为系统服务）
- [ ] schedule start 的守护进程循环（当前框架已搭好，需实测长时间运行）
- [ ] workflow 内 `schedule:` 字段支持（当前 check_unknown 报 v0.2 提示）
- [ ] RSS <15MB 验证
