# wan 项目代码审查报告

**日期**：2026-08-03
**审查范围**：src/ 全部模块 + tests/fixtures/smoke.yml
**对照文档**：spec.md、product-review-2026-08-02.md

---

## 一、总体结论

**代码质量优良，核心逻辑与 spec 对齐度高，28 个单元测试全部通过，release 二进制 553KB（远低于 6MB 限制）。**

发现 **3 个 Bug**、**4 个需改进项**、**11 个 clippy warning**。

---

## 二、Bug（需修复）

### Bug-1：workflow 级 `working-directory` 未传递给 executor — ✅ 已修复

**位置**：`engine.rs:53-54`、`executor.rs:233`

**修复**：`RunCtx` 新增 `workflow_wd: Option<PathBuf>` 字段，engine 传入 `workflow.working_directory`，executor 在 `run_job` 中按优先级合并。

### Bug-2：job 级 `working-directory` 未生效 — ✅ 已修复

**位置**：`executor.rs:228-230`

**修复**：`run_job` 中 base_dir 按优先级链合并：CLI `-C` > workflow.wd > job.wd > cwd。

**配套改动**：`RunOptions.working_dir` 从 `PathBuf` 改为 `Option<PathBuf>`（区分 CLI 是否显式指定 `-C`），cli.rs Flags.cwd 同步改为 `Option<PathBuf>`。

### Bug-3：`StepProcess::Drop` 未关闭 Job Object 句柄

**位置**：`platform/windows.rs:25-28`

`Drop for StepProcess` 的实现体是空注释。虽然 `OwnedHandle` 的 Drop 会关闭句柄，但注释暗示开发者意图手动关闭。经确认 `OwnedHandle` 的 Drop 会正确调用 `CloseHandle`，所以**功能上不是 bug**，但注释误导，建议清理。

---

## 三、需改进项

### Imp-1：`shell: bash/sh` 在 Windows 上不做存在性预检

**位置**：`shell.rs:23-30`

`supported_on_platform` 对 Windows 上 `Bash`/`Sh` 直接 `Ok(())`，但实际执行时若 bash 不在 PATH 会到 `spawn_and_wait` 才报错。虽然 `spawn_hint` 提示信息足够，但在 `validate` 阶段无法提前发现问题。

**建议**：spec 没有要求 validate 阶段检查 shell 可用性，当前行为可接受，但可在文档中说明。

### Imp-2：`find_operator` 不处理转义引号

**位置**：`expr.rs:53-68`

`parse_if` 中的 `find_operator` 对引号内的 `==`/`!=` 做了保护，但不处理转义引号（`\"`）。例如 `FOO == "a\"b"` 会在 `\"` 后的 `"` 处错误关闭引号。

**影响**：实际 if 表达式中带转义引号的场景极少，优先级低。

### Imp-3：scheduler 中 job 级 `if: always()` 语义偏差

**位置**：scheduler.rs:183-190

当 job 有 `if: always()` 且依赖失败时，`deps` 中有 `Outcome::Failure`，但 `EvalCtx::new` 计算 `all_ok = false`、`any_failed = true`。`Expr::Always` 返回 `true`，这是正确的。但当 job 无 `if_condition` 时，默认语义是 `deps.iter().any(|o| *o != Outcome::Success)` → 跳过。这与 spec §6.2 "默认 success()" 一致。

**确认**：逻辑正确，无问题。

### Imp-4：clippy 11 个 warning — ✅ 已修复

`cargo clippy --fix` 自动修复 10 个，手动修复 1 个 `ptr_arg`（`&mut Vec<Frame>` → `&mut [Frame]`）。另清理 output.rs 中 1 个 unused import。当前 clippy 零 warning。

---

## 四、逐模块审查

### 4.1 parser.rs — ✅ 优秀

- F-PARSE-1~10 全部覆盖：version 强制、必填字段、类型校验、行列号报错、未识别字段报错、runs-on/uses/matrix/container/schedule 专门报错、`${{ }}` 报错、needs 引用存在、job id 合法且唯一、step 至少一个
- saphyr 事件流 → DNode 树 → Workflow 两阶段解析，行列号贯穿全链路
- `check_dup` 防重复键、`check_unknown` 白名单校验
- 16 个测试覆盖正负路径
- **唯一缺口**：env 值只接受字符串（`env_strings_only` 测试确认），但 spec 未明确是否应支持数字/布尔（当前拒绝是安全选择）

### 4.2 expr.rs — ✅ 良好

- `${VAR}` 单遍非递归插值，未定义变量保留原样 + warning
- `parse_if` 支持 `success()/failure()/always()` + `var == literal` / `var != literal`
- `EvalCtx` 语义正确：`all_ok` = 全部依赖成功，`any_failed` = 任一失败
- `literal_string` 对 Null 返回空串，与 `Eq("EMPTY", Null)` 匹配空值语义一致
- 7 个测试覆盖插值、if 解析、求值语义
- **小问题**：`find_operator` 不处理转义引号（Imp-2）

### 4.3 scheduler.rs — ✅ 良好

- Kahn 拓扑排序 + 环检测 + 环路径打印（DFS 找环）
- `Semaphore` 全局 max_parallel 限流，Condvar 等待
- work-list 语义：job 线程等待 needs 全部结算后入队
- job 级 if 求值：默认 success() 语义（依赖非全成功则跳过）
- 跳过的 job 设置 `Outcome::Skipped`，不发射 JobStart/JobEnd
- 3 个测试覆盖无环、有环、自环
- **注意**：每个 job 一个线程，即使 max_parallel=1 也会创建 N 个线程（在信号量前阻塞）。对 v0.1 规模可接受

### 4.4 executor.rs — ✅ 良好（有 2 个 Bug）

- 临时脚本文件方案：`tmp_root/step-{job_idx}-{step_idx}.{ext}`，cmd 强制 `@chcp 65001` + CRLF
- 流式日志：pump 线程读 stdout/stderr → mpsc → Event::StepOutput
- 超时：step 超时 ∩ job 剩余时间，job 超时优先
- 重试：仅非 0 退出码重试，delay 期间可被中断，重试耗尽后 continue-on-error 生效
- $WAN_OUTPUT：仅 step 成功时读回，key=value 格式，# 注释忽略
- **Bug-1**：workflow.working_directory 未使用
- **Bug-2**：job.working_directory 未使用

### 4.5 platform/windows.rs — ✅ 优秀

- Job Object 方案：`CreateJobObjectW` + `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`
- `AssignProcessToJobObject` 在 spawn 后立即调用
- `TerminateJobObject` 用于 Ctrl+C / 超时
- Ctrl+C handler 设置 `INTERRUPTED` 标志，不调用默认 handler（由执行器清理后 exit 130）
- `SetConsoleOutputCP(65001)` + `SetConsoleCP(65001)` 强制 UTF-8
- **竞态说明**：代码注释了 assign 前子进程派生孙进程的竞态窗口，KILL_ON_JOB_CLOSE 兜底
- **Bug-3**：Drop 注释误导（功能正确）

### 4.6 platform/unix.rs — ✅ 优秀

- `process_group(0)` 让子进程自成进程组
- `killpg(SIGTERM)` → 等待 → `killpg(SIGKILL)` 两档终止
- `kill_graceful`/`kill_force` 检查 `child.is_some()` 防止 pgid 回收后误杀
- SIGINT/SIGTERM handler 设置中断标志

### 4.7 shell.rs — ✅ 优秀

- 显式指定 pwsh/cmd/bash/sh，不猜
- `build_command` 各 shell 参数正确：pwsh `-NoProfile -NonInteractive -File`、cmd `/d /s /c`、bash `--noprofile --norc -e -o pipefail`、sh `-e`
- `spawn_hint` 给出可操作提示
- `script_prelude` 对 cmd 强制 `@chcp 65001 >nul`

### 4.8 cli.rs — ✅ 良好

- 四命令 run/validate/list/graph + --version/--help
- 退出码：0 成功 / 1 失败 / 2 配置错误 / 130 中断
- `parse_common_flags` 复用 --json/--quiet/--no-color/-C/--max-parallel
- list 支持 `.wan/workflows/` 优先、回退当前目录
- graph 输出 mermaid `flowchart TD`

### 4.9 engine.rs — ✅ 良好

- `validate()` = check_acyclic + check_platform_shells
- `run()` = validate → tmp_dir → mpsc 通道 → schedule → RunEnd → join collector → cleanup
- 事件顺序保证：RunStart → (JobStart → StepStart → StepOutput → StepEnd → JobEnd)* → RunEnd
- 可重入：纯函数式，无全局状态（除 INTERRUPTED 原子标志）

### 4.10 output.rs — ✅ 良好

- HumanSink：彩色终端输出，JOB/STEP/OK/FAIL/WARN/DIM 6 种样式
- JsonSink：每行一个 JSON 事件，`#[serde(tag="type")]` 保证类型标签
- CapturingSink：测试辅助
- quiet 模式抑制 StepOutput

### 4.11 model.rs — ✅ 优秀

- `EnvMap = Vec<(String,String)>` 保序确定性，`merge_env` 多层合并
- `Outcome` 三态 Success/Failure/Skipped
- `Event` 枚举 `#[serde(tag="type")]` 7 种事件 + RFC3339 时间戳
- `Shell` 枚举带 `as_str()`
- `Retry{attempts,delay}`、`Expr`、`Literal` 完整

---

## 五、spec 对照清单

| 需求 | 状态 | 说明 |
|------|------|------|
| F-PARSE-1 version:1 强制 | ✅ | |
| F-PARSE-2 必填/类型/枚举校验 | ✅ | |
| F-PARSE-3 行列号报错 | ✅ | saphyr span 贯穿 |
| F-PARSE-4 未识别字段报错 | ✅ | 白名单 + 专门报错 |
| F-PARSE-5 runs-on 报错 | ✅ | |
| F-PARSE-6 ${{ }} 报错 | ✅ | |
| F-PARSE-7 needs 引用存在 | ✅ | |
| F-PARSE-8 job id 合法标识符 | ✅ | |
| F-PARSE-9 job id 唯一 | ✅ | |
| F-PARSE-10 step 至少一个 | ✅ | |
| F-SCHED-1 DAG 拓扑+环检测+路径 | ✅ | |
| F-SCHED-2 max-parallel | ✅ | |
| F-SCHED-3 退出码传播 | ✅ | 0/1/130 |
| F-SCHED-4 $WAN_OUTPUT 变量传递 | ✅ | 仅成功读回 |
| F-SCHED-5 引擎可重入 | ✅ | |
| F-REL-1 step 超时 | ✅ | |
| F-REL-2 job 超时优先 | ✅ | |
| F-REL-3 retry 次序 | ✅ | 先重试后 continue-on-error |
| F-REL-4 Ctrl+C 进程树终止 | ✅ | Job Object / setpgid |
| F-REL-5 实时流式日志 | ✅ | mpsc pump |
| F-OUT-1 人类可读输出 | ✅ | |
| F-OUT-2 --json 事件流 | ✅ | |
| F-OUT-3 UTF-8 code page | ✅ | chcp 65001 + SetConsoleCP |
| F-OUT-4 退出码 0/1/2/130 | ✅ | |
| CLI run/validate/list/graph | ✅ | |
| ${VAR} 单遍非递归插值 | ✅ | |
| if 受限子集 | ✅ | |
| retry 仅对非 0 退出码 | ✅ | |
| 体积 <6MB | ✅ | 553KB |
| workflow 级 working-directory | ✅ | 已修复：RunCtx.workflow_wd 传递 |
| job 级 working-directory | ✅ | 已修复：run_job 优先级合并 |

---

## 六、构建与测试

- `cargo build --release`：19s，566KB
- `cargo test`：28 tests passed, 0 failed
- `cargo clippy`：0 warnings
- smoke 测试：`wan validate tests/fixtures/smoke.yml` → OK
- smoke 测试：`wan graph tests/fixtures/smoke.yml` → 正确输出 mermaid
- smoke 测试：`wan run tests/fixtures/smoke.yml` → pwsh 不在 PATH 导致 FAIL（预期，机器未装 PowerShell 7+）

---

## 七、建议修复优先级

1. ~~**P0**：Bug-1 + Bug-2（working-directory 传递）~~ — ✅ 已修复
2. ~~**P1**：clippy 11 warnings~~ — ✅ 已修复
3. **P2**：Bug-3 注释清理
4. **P2**：Imp-2 转义引号处理（低频场景）
