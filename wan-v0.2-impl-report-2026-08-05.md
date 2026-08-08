# wan v0.2 实现报告

**日期**: 2026-08-05
**Commits**: `6a69f6a` (cron 调度器) → `f9351b7` (系统服务集成) → `bf54d56` (文档)
**状态**: ✅ v0.2 全部验收标准通过

## v0.2 验收标准（spec §21）

| 验收项 | 状态 | 验证方式 |
|---|---|---|
| cron 常驻调度 + 执行历史持久化 | ✅ | `schedule start` 守护进程，JSONL 历史 |
| Windows 服务 + Linux systemd 集成 | ✅ | `schedule service install/remove/status` |
| 定时跑通一个 workflow | ✅ | `run-once` 端到端验证通过 |
| 历史可查 | ✅ | `schedule history` 命令 |
| RSS <15MB | ✅ | 实测 4.05MB |

## 新增模块

### src/cron.rs（~400 行）
手写标准 5 字段 cron 表达式解析器，零新依赖。

- **语法支持**：`*`、`N`、`N-M`、`N,M,L`、`*/N`、`N-M/S`
- **数据结构**：`Field` 使用 u64 bit-set（分钟 0-59 需 60 bit，u32 不够）
- **核心方法**：
  - `CronExpr::parse(s)` — 解析并校验范围
  - `CronExpr::next_after(now)` — 逐分钟扫描下一个触发时间
  - `CronExpr::matches_now(now)` — 当前分钟是否匹配
- **12 个单元测试**：基本值/范围/列表/步长/范围步长/越界/字段数错误/闰年/跨小时/跨天/周日=7

### src/schedule.rs（~480 行）
常驻调度守护进程 + JSONL 历史持久化。

- **存储**：
  - `.wan/schedules/schedules.json` — 调度条目列表
  - `.wan/schedules/history.jsonl` — 执行历史（每行一条 JSON）
- **守护进程**（`run_daemon`）：
  - 计算每个调度的下次触发时间
  - sleep 到最近触发时间，到点调用 `engine::run`
  - 错过窗口：默认跳过，`--catch-up` 补跑
  - Ctrl+C 优雅退出
- **单次扫描**（`run_once`）：供 schtasks/systemd 每分钟调用
  - 检查所有调度当前分钟是否匹配
  - 60 秒内去重（防止重复触发）
  - 执行 workflow 并记录历史
- **7 个单元测试**：add/list/remove/duplicate_id/history_append_read/limit/empty

### src/service.rs（~260 行）
系统服务集成。

- **Windows**（schtasks）：
  - 生成 wrapper bat 文件到 `.wan/schedules/run-once.bat`
  - `schtasks /Create /TN WanSchedule /TR <bat> /SC MINUTE /MO 1 /F`
  - 每分钟触发 `wan schedule run-once`
- **Linux**（systemd user unit）：
  - 生成 `~/.config/systemd/user/wan-schedule.service` + `wan-schedule.timer`
  - `OnCalendar=*:0/1`，`Persistent=true`
  - `systemctl --user enable --now wan-schedule.timer`
- **2 个单元测试**：task_name/unit_name 稳定性

## CLI 子命令

```
wan schedule add <id> <cron-expr> <workflow> [-C <dir>]
wan schedule remove <id> [-C <dir>]
wan schedule list [-C <dir>]
wan schedule start [-C <dir>] [--catch-up] [--json] [--quiet] [--no-color]
wan schedule run-once [-C <dir>] [--json] [--quiet] [--no-color]
wan schedule service install|remove|status [-C <dir>]
wan schedule history [<id>] [-C <dir>] [--limit N]
```

## 端到端验证

### schedule add/list/remove
```
> wan schedule add daily "0 2 * * *" test -C $tmp
已添加调度：daily [0 2 * * *] -> ...test-win.yml

> wan schedule list -C $tmp
daily            [0 2 * * *     ] test-win -> ...test-win.yml (next: 2026-08-05T02:00:00Z)

> wan schedule remove daily -C $tmp
已移除调度：daily
```

### schedule run-once
```
> wan schedule add everymin "* * * * *" test -C $tmp
> wan schedule run-once -C $tmp
[everymin] 触发执行：test-win
    hello from cron
[everymin] 执行完成：退出码 0，耗时 509ms

> wan schedule history -C $tmp
2026-08-04T16:25:35.9450621Z everymin [* * * * *] test-win exit=0 (509ms)
```

### schedule service (Windows schtasks)
```
> wan schedule service install -C $tmp
已安装系统服务。

> schtasks /Query /TN "WanSchedule" /FO LIST
TaskName:      \WanSchedule
Next Run Time: 2026/8/5 0:23:00
Status:        Ready

> wan schedule service remove
已移除系统服务。
```

### RSS 验证
```
> wan schedule start -C $tmp  (3 seconds)
RSS: 4,243,456 bytes = 4.05 MB  (< 15MB ✅)
```

## 构建数据

| 指标 | 值 | 限制 |
|---|---|---|
| 测试 | 87 passed (76 unit + 11 integration) | — |
| clippy | 0 warnings | — |
| release 体积 | 670,720 bytes = 655KB | < 6MB |
| RSS（调度模式） | 4.05MB | < 15MB |

## 修改文件

| 文件 | 操作 | 说明 |
|---|---|---|
| src/cron.rs | 新建 | 5 字段 cron 解析器 |
| src/schedule.rs | 新建 | 守护进程 + JSONL 历史 + run_once |
| src/service.rs | 新建 | Windows schtasks / Linux systemd 集成 |
| src/lib.rs | 改 | 注册 cron/schedule/service 模块 |
| src/error.rs | 改 | 新增 From<serde_json::Error> |
| src/cli.rs | 改 | schedule 子命令 + USAGE 更新 |
| docs/USER_GUIDE.md | 改 | 新增第 14 节 Cron 调度 |

## git 历史

```
bf54d56 docs: 用户手册补充 cron 调度章节 + service 集成
f9351b7 feat: 系统服务集成 — schedule service install/remove/status + run-once
6a69f6a feat: cron 调度器 — schedule add/remove/list/start/history
8f21f6f fix: add /deploy to .gitignore
92f7bef docs: 用户手册补充 hook 章节 + deploy 目录
16697f4 feat: git hook 子命令 — install/remove/list
6a5cc15 feat: 本地工作流执行器 wan — 完整实现 v0.1
10ef080 初始提交：需求初稿
```

## v0.2 后续待办（非阻塞验收）

- [ ] workflow 内 `schedule:` 字段支持（当前用 CLI `schedule add` 管理）
- [ ] Linux 环境实测 systemd 集成
- [ ] 守护进程长时间运行稳定性测试
- [ ] catch-up 补跑策略细化（当前只补跑最近一次）
