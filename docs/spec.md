# 本地工作流执行器 · 需求规格与技术设计

> **文档代号**：`wan`（项目仓库目录名，同时也是 CLI 命令名）
> **文档版本**：v0.1（对应产品 v0.1 范围）
> **文档日期**：2026-08-02
> **上游依据**：[product-review-2026-08-02.md](file:///c:/Dev/Projects/wan/docs/product-review-2026-08-02.md)（选型评审，结论：⚠️ 有条件 Go）
> **状态**：待评审 → 评审通过后进入实现

---

## 0. 文档说明

### 0.1 文档目的
本文档是 v0.1 的**单一权威来源（SSOT）**，整合「需求规格（PRD）」与「技术设计（TDD）」两大部分，目标是：任何一位工程师仅凭本文档即可独立完成 v0.1 的实现与自测。

### 0.2 与上游评审报告的关系
- 评审报告回答「**要不要做、做什么定位**」，本文档回答「**具体做成什么样、怎么实现**」。
- 评审报告中的所有结论性决策（语言、schema 边界、依赖黑名单、Windows 四硬指标、v0.1 范围）在本文档中**默认生效**，不再重复论证，仅落地为可执行规格。
- 若本文档与评审报告冲突，以评审报告为准；若实现中发现决策需修订，回到评审报告更新后再同步本文档。

### 0.3 版本与稳定性约定
- 配置 schema 在 **v1.0 之前视为不稳定**，v0.x 保留破坏性变更权利。
- 顶层强制 `version: 1` 字段，为未来 schema 迁移预留通道。
- 本文档自身遵循语义化版本：v0.1 对应产品 v0.1 范围冻结。

### 0.4 术语
| 术语 | 含义 |
|---|---|
| workflow | 一个 YAML 配置文件，含若干 job |
| job | workflow 内一个独立执行单元，由若干 step 串行组成 |
| step | job 内的最小执行单元，对应一条 shell 命令 |
| DAG | job 之间的有向无环依赖图，由 `needs` 描述 |
| runner | 本工具自身（区别于 GitHub Actions 的 runner 概念，本项目**无 runner 概念**） |

---

# 第一部分 · 需求规格（PRD）

## 1. 项目概述

### 1.1 一句话定位
> **Windows 与 Linux 双平台原生一等公民的、带常驻 cron 调度的、零依赖、单文件 < 6MB 的本地流水线执行器。**
>
> 简称：「**在 Windows 与 Linux 上都真正能用的 dagu。**」

### 1.2 背景
用户拥有 GitHub Actions + GitLab CI + CNB 三套 CI，本地还维护成对的 `test-fullchain.sh` / `test-fullchain.ps1` 脚本。现有工具均无法同时满足「小体积 + 零依赖 + Windows 原生 + DAG 并行 + cron 调度 + 类 GH Actions 心智」这一组合需求：

- `dagu`：有 cron，但 47.8MB、GPL-3.0、自有 DSL、功能过载。
- `act`：强依赖 Docker，Windows 链路最重。
- `wrkflw`：5.2MB、MIT、吃 GH Actions YAML，但**无 cron**、`shell:` 仅 bash/sh、`runs-on: windows-*` 静默映射为容器。
- `just`：2.1MB 优秀，但无 DAG 并行、无 cron。
- `go-task`：~38MB 解压，无 cron。

**唯一空白格子**：cron 调度 + Windows/Linux 原生 + 小体积 + GH Actions 心智，四者同时满足。本项目即填此空白。

### 1.3 与竞品的关系
- **不做**「精简版 dagu」——那个定位说不出用户为何不用 wrkflw。
- **不做**「兼容 GitHub Actions 的实现」——那是 wrkflw/act 的赛道，且是无底洞。
- **做**「借鉴 GH Actions 心智模型（jobs/steps/needs/env）的、独立定义的小 schema」。
- **做**「Windows 与 Linux 双平台原生一等公民」——wrkflw 在 Linux 上可用但无 cron、Windows 上静默映射容器；本项目在两平台都做原生，且 v0.2 起提供 cron。

### 1.4 启动前置门禁
进入实现前**必须**先完成评审报告 §6 的 wrkflw 实测验证（1 周时间盒）。按三分判定：
- wrkflw 在 Windows 原生下 T1/T2/T4 全过 → **No-Go**，改用 wrkflw + `schtasks`。
- 缺 cron 但可贡献、维护者开放 → **Contribute**，提 PR 而非自研。
- T1 或 T4 失败、维护者对 Windows 不积极 → **Go 自研**，按本文档执行。

> 本文档假设已通过门禁、判定为 Go 自研。

---

## 2. 目标用户与使用场景

### 2.1 主要用户画像
- **自用为主**：作者本人（Windows 11 主力开发，有 Rust 工具链资产），第一个"绝望用户"。
- **扩展用户**：在 Windows 或 Linux 原生环境（无 Docker / WSL / Git Bash）下需要编排多步本地任务的工程师；CI 环境中需要快速拉取的轻量流水线工具；需要在 Linux 服务器上常驻定时跑本地脚本的工程师。

### 2.2 核心使用场景
| # | 场景 | 描述 |
|---|---|---|
| S1 | 本地全链路测试 | 取代成对的 `.sh`/`.ps1`，用一个 `.wan/workflows/*.yml` 描述全链路，本地一键跑通 |
| S2 | CI 复用 | 同一 workflow 文件在本地与 CI（GH Actions/GitLab/CNB）中复用，CI 仅负责拉取本工具 + 调用 `wan run` |
| S3 | 定时任务（v0.2） | 用 cron 表达式常驻调度本地流水线（如每日构建、定时健康检查） |
| S4 | 开发期快速校验 | `wan validate` / `wan graph` 在不执行的情况下校验配置与可视化 DAG |

### 2.3 非目标场景（明确排除）
- 团队级/分布式 workflow 编排（dagu 的分布式 worker 方向）。
- 跨机远程执行（SSH executor 等）。
- 需要容器隔离的执行。
- 复用 GitHub Actions 的 `uses:` action 生态。

---

## 3. 产品定位与差异化

### 3.1 唯一卖点
「**Windows 与 Linux 双平台原生 + cron 调度 + 零依赖 + 单文件 < 6MB**」四者同时成立。

### 3.2 双平台硬指标（卖点支点）
任一平台任意一条不达标 → 该平台失去存在理由 → 该平台 No-Go。

**Windows 硬指标 W1–W4：**

| # | 标准 | 验证方式 |
|---|---|---|
| **W1** | `shell:` 原生支持 `pwsh` / `cmd` / `bash`（Git Bash）三选一，**显式指定，不猜测** | 三种 shell 各写一个 workflow 跑通 |
| **W2** | 路径分隔符自动处理：runner 自身的路径操作一律经 `PathBuf`（`push`/`join`），禁止手写字符串拼接，不产生 `C:\foo/bar\baz` 这类混用路径；`working-directory` 中的 `\`/`/` 混用由 OS 原生解析；`env` 中的路径值按原样传给子进程，**不做猜测性转换**（env 值不一定是路径） | 混合分隔符路径用例：`working-directory` 正确生效，runner 输出路径无混用 |
| **W3** | **Ctrl+C 能通过 Win32 Job Object 完整终止整个子进程树**，无孤儿进程残留 | 启动嵌套子进程（`pwsh`→`node`→`java`）后 Ctrl+C，任务管理器验证零残留 |
| **W4** | **零外部依赖**：纯净 Windows Server Core 容器（无 Docker/WSL/Node/Python/Git Bash）能跑通 `shell: pwsh` 与 `shell: cmd` | 纯净容器验证 |

**Linux 硬指标 L1–L4：**

| # | 标准 | 验证方式 |
|---|---|---|
| **L1** | `shell:` 原生支持 `bash` / `sh` 二选一（**显式指定，不猜测**）；`pwsh` 作为可选支持（若已安装） | 两种 shell 各写一个 workflow 跑通 |
| **L2** | 路径使用 POSIX 风格，`working-directory`、`env` 中的路径用 `/` 分隔符，与 Windows 路径互不污染 | 跨平台同一 workflow 在两平台跑通 |
| **L3** | **Ctrl+C 能通过 `setpgid`+`killpg` 完整终止整个子进程树**，无孤儿/僵尸进程残留 | 启动嵌套子进程（`bash`→`node`→`java`）后 Ctrl+C，`ps`/`pgrep` 验证零残留 |
| **L4** | **零外部依赖**：纯净 Linux 容器（如 `alpine`/`debian:slim`，无 Docker/Node/Python/bash 之外的运行时）能跑通 `shell: bash` 与 `shell: sh` | 纯净容器验证 |

### 3.3 不做什么（边界，详见 §9）
- 不兼容 GitHub Actions（不复用 `.github/workflows/`，用 `.wan/workflows/`）。
- 不实现 `${{ }}` 表达式引擎（只做 `${VAR}` 插值）。
- 不做 `uses:` action 生态、`matrix`、`container/services`、artifact、TUI/Web UI、多后端密钥管理。

---

## 4. 功能性需求

### 4.1 配置解析与校验
| 需求 ID | 描述 |
|---|---|
| F-PARSE-1 | 支持 YAML 解析为内部模型，主选 `saphyr`（`serde_yaml` 已废弃；备选 `serde_norway`，见 §11.2/§15.7） |
| F-PARSE-2 | 顶层强制 `version: 1` 字段，缺失或非 `1` 报错 |
| F-PARSE-3 | Schema 校验：必填字段缺失、类型不匹配、枚举值非法均报错 |
| F-PARSE-4 | **错误信息必须带行号和列号**（DX 第一道门面） |
| F-PARSE-5 | **未识别字段直接报错，不静默忽略**（显式失败 > 静默降级） |
| F-PARSE-6 | `runs-on:` 字段解析时报错（永不支持，明示而非静默） |
| F-PARSE-7 | 遇到 `${{ ... }}` 输入直接报错并提示「不支持，请使用 `${VAR}`」（绝不静默处理） |
| F-PARSE-8 | `needs` 引用的 job id 必须存在，否则报错（带位置） |
| F-PARSE-9 | job id 必须唯一且符合标识符规则（`[A-Za-z_][A-Za-z0-9_-]*`），违者报错（带位置） |
| F-PARSE-10 | 每个 job 必须至少包含一个 step，否则报错（带位置） |

### 4.2 语法支持（v0.1 子集）
| 字段 | 层级 | 语义 |
|---|---|---|
| `version` | workflow | 必须为 `1` |
| `env` | workflow / job / step | 三层作用域，后者覆盖前者；值支持 `${VAR}` 插值 |
| `jobs.<id>` | workflow | job 字典，`<id>` 为标识符 |
| `jobs.<id>.needs` | job | 依赖列表，如 `[a, b]`，驱动 DAG |
| `jobs.<id>.if` | job | job 级条件（受限子集同 step，语义见 §6.2） |
| `jobs.<id>.working-directory` | job | job 级工作目录，覆盖 workflow 级 |
| `working-directory` | workflow | workflow 级工作目录 |
| `jobs.<id>.steps[]` | job | 串行 step 列表 |
| `steps[].name` | step | 可读名称 |
| `steps[].run` | step | 核心执行命令（必填） |
| `steps[].shell` | step | **显式必填**：`pwsh` / `cmd` / `bash` / `sh` |
| `steps[].working-directory` | step | 工作目录，覆盖 job/workflow |
| `steps[].env` | step | step 级环境变量 |
| `steps[].if` | step | 受限子集：`success()` / `failure()` / `always()` / `<var> == <literal>` / `<var> != <literal>` |
| `steps[].timeout-minutes` | step | step 超时 |
| `steps[].continue-on-error` | step | 该 step 失败不致 job 失败 |
| `steps[].retry` | step | **本项目扩展**：`{ attempts: N, delay: Ns }`，非 GH Actions 原生 |
| `jobs.<id>.timeout-minutes` | job | job 级超时 |

### 4.3 调度执行
| 需求 ID | 描述 |
|---|---|
| F-SCHED-1 | 对 `needs` 做 DAG 拓扑排序，**环检测**：检测到环时报错并打印环路径 |
| F-SCHED-2 | 无依赖关系的 job 用 `std::thread` 并行执行；`--max-parallel N` 限流（默认无上限） |
| F-SCHED-3 | job 内 steps 串行执行 |
| F-SCHED-4 | 正确的退出码传播：step 非 0 退出且非 `continue-on-error` → job 失败；job 失败 → 依赖它的 job 不执行（除非其 `if: failure()`/`always()`） |
| F-SCHED-5 | step 间变量传递：`$WAN_OUTPUT` 风格，step 写临时文件，runner 读回注入后续 step 的 env（**不做表达式引用**） |
| F-SCHED-6 | 执行引擎设计为**可重入的库**（不写在 `main()` 里），为 v0.2 cron 预留 |

### 4.4 可靠性机制
| 需求 ID | 描述 |
|---|---|
| F-REL-1 | step 超时：`timeout-minutes` 到点终止该 step 进程树 |
| F-REL-2 | job 超时：job 整体超时终止其所有 step |
| F-REL-3 | 失败重试：`retry.attempts` 次数内重试，`retry.delay` 为退避间隔 |
| F-REL-4 | **Ctrl+C → 进程树完整终止**：Windows 用 Job Object，Unix 用 `setpgid`+`killpg` |
| F-REL-5 | 实时流式日志：子进程 stdout/stderr 行级实时输出，不等 job 结束 |
| F-REL-6 | 信号处理：Ctrl+C 触发优雅关闭，等待子进程树清理后退出 |

### 4.5 输出与日志
| 需求 ID | 描述 |
|---|---|
| F-OUT-1 | 默认人类可读输出：带 job/step 前缀、颜色、耗时统计 |
| F-OUT-2 | `--json` 切换为结构化事件流（每行一个 JSON），事件类型：`run_start` / `job_start` / `step_start` / `step_output` / `step_end` / `job_end` / `run_end` |
| F-OUT-3 | 强制 UTF-8 输出，启动时设置控制台 code page（Windows）避免中文乱码 |
| F-OUT-4 | 退出码：成功 0；workflow 执行失败（任一 step 非 0 退出且未被 `continue-on-error` 吞掉）→ 1；配置错误 2；被中断 130 |

### 4.6 CLI 命令
| 命令 | 功能 |
|---|---|
| `wan run <file>` | 执行一个 workflow 文件 |
| `wan validate <file>` | 仅校验 schema 与 DAG，不执行 |
| `wan list` | 列出 `.wan/workflows/` 目录下所有 workflow（无 `.wan/workflows/` 时列当前目录 `.yml`/`.yaml`） |
| `wan graph <file>` | 输出 mermaid 文本（零体积成本的"可视化"） |
| 全局参数 | `--json` / `--max-parallel N` / `--quiet` / `--no-color` / `-C <dir>` |

`--quiet`：抑制 step 输出（`step_output` 事件与人类可读的 step 输出行），保留 job/step 起止与最终摘要；与 `--json` 同用时无效（事件流始终完整）。

### 4.7 v0.2 规划（架构预留，v0.1 不实现）
| 需求 ID | 描述 |
|---|---|
| F-CRON-1 | `wan schedule <file> --cron "..."` 常驻调度 |
| F-CRON-2 | cron 表达式解析（`cron` 或 `saffron` crate） |
| F-CRON-3 | 错过窗口的补偿策略（默认跳过，可选补跑） |
| F-CRON-4 | 执行历史持久化（本地 SQLite 或 JSONL，与 cron 一起做，无历史的调度器价值减半） |
| F-CRON-5 | Windows 服务/计划任务 + Linux systemd 集成 |

---

## 5. 非功能性需求

| 维度 | 指标 | 验证方式 |
|---|---|---|
| **体积** | Windows/Linux x64 解压后单文件 **< 6 MB** | CI 门禁自动检查，>6MB fail build |
| **冷启动** | runner 启动到派生首个 step 子进程 **< 50 ms**（不含 shell 加载；基准固定用 `cmd`/`sh` 空 step） | `hyperfine` 基准测试 |
| **运行时依赖** | **0**（无 Docker/Node/Python/bash） | 纯净 Windows Server Core + Linux 容器验证 |
| **常驻内存（v0.2）** | 调度模式 RSS **< 15 MB** | `Get-Process` |
| **分发** | **1 个 .exe**，无 DLL、无配套文件 | release 资产检查 |
| **平台** | Windows x64 与 Linux x64 双一等公民；macOS 尽力 | 三平台 CI（Windows/Linux 强制，macOS best-effort） |
| **可观测性** | `--json` 结构化事件流 + 退出码语义 | 自动化测试断言 JSON 事件 |
| **可维护性** | 依赖最小化；新增依赖需跑 `cargo bloat` 并记入决策日志 | PR 审查 |

---

## 6. 配置 Schema 规范（详细）

### 6.1 完整示例

```yaml
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
          Write-Host "building in $env:BUILD_DIR"
          go build -o ../out/app ./...
        timeout-minutes: 10
        retry:
          attempts: 3
          delay: 5s

  test:
    needs: [build]
    steps:
      - name: 单元测试
        shell: pwsh
        run: go test ./...
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
        run: Write-Host "done"
```

### 6.2 `if` 受限子集语法
```
if := func | comparison
func := "success()" | "failure()" | "always()"
comparison := var ("==" | "!=") literal
var := identifier
literal := quoted_string | number | "true" | "false" | "null"
```
- 多条件组合（`&&` / `||`）**v0.1 不支持**，每个 `if` 只能是一个原语。
- `if` 内**不隐含 `${{ }}`**，直接写字面表达式。
- 求值语义（v0.1 冻结）：
  - job 级（在 `needs` 依赖全部结算后求值）：`success()` = 所有依赖 job 成功（无依赖时为 true）；`failure()` = 任一依赖 job 失败；`always()` = 恒真。
  - step 级（在同 job 先前 step 结算后求值）：`success()` = 先前 step 全部成功；`failure()` = 先前任一 step 失败；`always()` = 恒真。
  - 比较式：左操作数 var 取自合并后的 env（含先前 step 注入的输出）；var 未定义或值为空 → 视为 `""`；literal 序列化为字符串后与 var 做**字符串相等**（`true` → `"true"`、数字按解析值序列化、`null` 等价于 `""`）。
  - `if` 不做 `${VAR}` 插值（`if` 直接引用变量名）。
  - 跳过传播：job 因 `if` 为假被跳过 → 结算为 `skipped`；依赖它的 job 默认同样跳过（不执行），`if: always()` 的下游仍执行，`failure()` 对 `skipped` 不触发。

### 6.3 `retry` 字段
```yaml
retry:
  attempts: 3      # 最大尝试次数（含首次），>=1
  delay: 5s        # 重试间隔，支持 s/m/h 后缀
```
- 仅对非 0 退出码生效；被超时杀死的不重试（超时是更严重的失败）。
- `retry.delay` 为**单单位**时间（`5s` / `2m` / `1h`），不支持组合（`1m30s`）。
- 重试与 `continue-on-error` 的次序：**先重试**（attempts 用尽仍失败），再应用 `continue-on-error` 吞错。
- job 超时优先于重试：job 超时到点立即终止当前 step 进程树，重试循环（含 delay 等待）一并取消。

### 6.4 变量插值规则
- `${VAR}`：从 env 三层作用域（workflow → job → step）合并后的**原始表**中解析，未定义时**保留原样并 warning**（不报错，避免动态环境导致流水线脆弱）。
- 插值适用于：`run` 脚本、`env` 值、`working-directory`；不适用于 `name` 与 `if`（`if` 直接引用变量名）。
- **单遍、非递归**：以合并后的原始 env 表为解析源，每个值只扫描一次；env 值中的 `${VAR}` 解析自原始表，不做嵌套展开——循环引用（`A: "${B}"` + `B: "${A}"`）因此自然退化为「未定义 + warning」，不会死循环。
- 不支持嵌套、不支持默认值、不支持命令替换（这些是 `${{ }}` 的领地）。
- `env` 值必须是字符串：YAML 数字/布尔/null 标量直接报错（带位置），不做隐式转换。

### 6.5 step 间变量传递
- step 写：`$WAN_OUTPUT` 指向一个临时文件路径，step 脚本以 `key=value` 行格式写入（`key` 为首个 `=` 前的内容；值内不允许换行；行尾 `\n`/`\r\n` 剥离；`#` 开头行忽略）。
- runner 读：**仅在 step 成功（退出码 0）时**读取该文件，将 `key` 注入本 job 后续 step 的 env（含其 `if` 求值与 `run` 插值）；失败/超时/`continue-on-error` 的 step 输出**不回读**。
- 内置变量优先级：runner 注入的 `$WAN_OUTPUT` 等内置变量**优先于**用户 env（同名覆盖）。
- **不做 `needs.<id>.outputs.*` 跨 job 引用**（v0.1 不做，复杂度高）。

---

## 7. CLI 接口规范

### 7.1 命令清单

```
wan run <file> [--json] [--max-parallel N] [--quiet] [--no-color] [-C <dir>]
wan validate <file> [-C <dir>]
wan list [-C <dir>]
wan graph <file> [-C <dir>]        # 输出 mermaid 文本（dot 不做，v0.1 范围）
wan --version
wan --help
```

### 7.2 退出码
| 码 | 含义 |
|---|---|
| 0 | 成功 |
| 1 | workflow 执行失败（有 step 非 0 退出） |
| 2 | 配置错误（解析/校验/DAG 环） |
| 130 | 被 Ctrl+C 中断 |

### 7.3 `--json` 事件格式
```json
{"type":"run_start","workflow":"build","ts":"2026-08-02T10:00:00Z"}
{"type":"job_start","job":"build","ts":"..."}
{"type":"step_start","job":"build","step":"编译","ts":"..."}
{"type":"step_output","job":"build","step":"编译","stream":"stdout","line":"building..."}
{"type":"step_end","job":"build","step":"编译","code":0,"duration_ms":1234}
{"type":"job_end","job":"build","code":0,"duration_ms":1300}
{"type":"run_end","code":0,"duration_ms":2000,"ts":"..."}
```
- `workflow` 字段 = workflow 文件主文件名（去扩展名）。
- 被跳过的 job/step 不发射 `job_start` / `step_start` 事件（事件流保持干净）。
- `--quiet` 与 `--json` 同用时无效（事件流始终完整输出）。

---

## 8. 验收标准

### 8.1 v0.1 主验收
> **用它跑通 Baafoo 项目的 `test-fullchain` 全链路，并删掉 `.sh` / `.ps1` 中的一份。**

连自己的项目都用不上，就不该发布。

### 8.2 量化成功指标（v1）
| 指标 | 目标值 |
|---|---|
| Windows/Linux x64 解压后单文件体积 | < 6 MB |
| 冷启动（runner 启动 → 派生首个 step） | < 50 ms |
| 运行时依赖数量 | 0 |
| 常驻调度模式内存（v0.2） | < 15 MB RSS |
| 单文件分发 | 1 个 .exe |

### 8.3 双平台硬指标
见 §3.2 的 W1–W4 与 L1–L4，全部通过为验收前提。

---

## 9. 范围边界（明确不做清单）

> 以下功能**明确不做**，须写进 README 首屏。遇到对应输入**显式报错**而非静默忽略。

| 砍掉的功能 | 理由 | 未来 |
|---|---|---|
| `uses:` 整个 action 生态 | 需 Node runtime / Docker，体积杀手 | **永不做** |
| 完整 `${{ }}` 表达式引擎 | 复杂度无底洞 | **v1 永不做**，v2 极度克制 |
| `matrix` | 组合语义复杂 | v0.3 考虑最简笛卡尔积 |
| `container:` / `services:` | 与零依赖定位冲突 | **永不做** |
| Docker/Podman/K8s executor | 同上 | **永不做** |
| TUI / Web UI | 体积；wrkflw 已有 TUI | v0.3 考虑 TUI |
| artifact 上传/下载 / cache | 本地执行，伪需求 | **永不做** |
| 密钥管理后端（Vault/AWS/Azure/GCP） | 需 HTTP+TLS 栈 | **永不做**，走环境变量 |
| 远程触发 / GitLab 集成 | 需网络栈 | **永不做** |
| 可复用 workflow | 递归展开复杂 | v0.4 考虑 |
| 执行历史持久化 | — | **v0.2 随 cron 一起做** |
| `runs-on:` | 单机无 runner 概念 | 解析时报错，永不支持 |
| 复用 `.github/workflows/` 目录 | 避免兼容性债务 | 用 `.wan/workflows/` |

---

## 10. 风险摘要（详见评审报告 §5）

| 级别 | 风险 | 对策 |
|---|---|---|
| 🔴 P0-1 | wrkflw 已占位 85% | 启动前强制实测验证（§1.4 门禁） |
| 🔴 P0-2 | 双平台兼容性（Windows 进程树/pwsh/路径/编码；Linux 进程组/编码） | Windows 用 Job Object、Linux 用 setpgid/killpg；强制 UTF-8；双平台 CI |
| 🔴 P0-3 | 表达式引擎失控 | v1 明确不支持 `${{ }}`，写进 README 首屏 |
| 🟡 P1-1 | 体积与功能冲突 | CI 体积门禁 >6MB fail |
| 🟡 P1-2 | schema 一旦公开难改 | `version: 1` + 「v1.0 前 unstable」声明 |
| 🟡 P1-3 | "像 GH Actions"心智误导 | 用 `.wan/workflows/`；未识别字段报错 |
| 🟡 P1-4 | 依赖隐性膨胀 | 新增依赖跑 `cargo bloat` + 决策日志 |
| 🟢 P2 | 烂尾 / GPL 传染 / YAML 生态断层 / bus factor | 自用优先；只读 dagu 行为；YAML 层薄封装 |

---

# 第二部分 · 技术框架（TDD）

## 11. 技术栈选型

### 11.1 语言与工具链
- **语言**：Rust（stable，复用作者 sql-guard 项目已固定的 rustc 工具链与 USTC 镜像）。
- **交叉编译**：Windows 走 `x86_64-pc-windows-gnu`（复用作者已打通链路）；Linux 走 `x86_64-unknown-linux-gnu`；macOS 走 `x86_64-apple-darwin`。
- **选 Rust 的理由**（非性能）：① 工具链零启动成本；② 唯一能同时满足 <6MB 与生态可用；③ 可与 sql-guard 共享 CI 发布流程。

### 11.2 关键依赖选型
| 用途 | 选型 | 体积估算 | 备注 |
|---|---|---|---|
| YAML 解析 | `saphyr` 0.0.11（主选） | ~400 KB | `serde_yaml` 已废弃；saphyr 维护活跃（2026-07 更新）且事件流带位置信息（利于行列号报错）；`serde_norway` 0.9.42 自 2024-12 后无更新，为备选；解析层薄封装（§15.7） |
| 并发 | `std::thread` + `std::sync::mpsc` | 0 | **不用 tokio**（编排几十子进程用 OS 线程足够，且避免 async 传染） |
| 进程执行 | `std::process::Command` | 0 | 标准库足够 |
| CLI 参数 | `lexopt` | ~10 KB | 体积敏感；若需 derive 可用 `clap` 但必须 `default-features=false` |
| 进程树终止 (Win) | `windows-sys` 0.61（仅启用 Job Object 相关 feature） | ~200 KB | 直接调平台 API，不用跨平台大包 |
| 进程树终止 (Unix) | `libc`（`setpgid`/`killpg`） | ~100 KB | — |
| cron 解析 (v0.2) | `cron` 或 `saffron` | ~150 KB | 都很轻 |
| 表达式求值 | **手写递归下降** | ~30 KB | 子集极小，不引 `evalexpr` |
| 时间 | `jiff` 0.2（`default-features = false` 裁剪 tzdb）或 `time` | ~150–400 KB | **不用 chrono** |
| 终端颜色 | `anstyle` 或手写 ANSI | ~20 KB | 不用 `colored`/`termcolor` 全家桶 |
| JSON 输出 | `serde_json` | ~300 KB | 已有 serde，边际成本低 |
| 序列化 | `serde` | ~300 KB | 基础设施 |

### 11.3 依赖黑名单
| 禁止 | 理由 |
|---|---|
| `tokio` | ~1–2MB，async 传染 |
| `reqwest` | 带 TLS 栈 ~3–4MB，v1 无网络需求 |
| `regex` | ~1.5MB，能用手写 matcher/`str::starts_with` 解决 |
| `chrono` | 比 `time`/`jiff` 大，时区数据库重 |
| `serde_yaml` | 已 archived 废弃 |
| 任何前端框架/静态资源嵌入 | dagu 的教训 |

---

## 12. 系统架构

### 12.1 分层架构

```
┌─────────────────────────────────────────────┐
│  CLI 层 (main.rs / lexopt 解析)              │  薄壳，仅参数解析与退出码
├─────────────────────────────────────────────┤
│  调度层 (scheduler)                          │  DAG 拓扑 + 并行调度 + 重试/超时
│   └─ 执行引擎 (engine) ── 纯函数式可重入 ──┐  │  v0.2 调度器从这里调用
├─────────────────────────────────────────────┤
│  执行层 (executor)                          │  step 进程派生 + 流式日志
│   ├─ shell 抽象 (pwsh/cmd/bash/sh)          │
│   ├─ 变量插值 (${VAR}) / if 求值            │
│   └─ 输出收集 ($WAN_OUTPUT 回读)             │
├─────────────────────────────────────────────┤
│  平台抽象层 (platform)                      │  进程树终止 / 编码 / 路径
│   ├─ windows (Job Object, code page)        │
│   └─ unix (setpgid/killpg)                  │
├─────────────────────────────────────────────┤
│  解析层 (parser) + Schema 模型 (model)      │  YAML → 内部模型 + 校验 + 错误定位
├─────────────────────────────────────────────┤
│  输出层 (output)                            │  人类可读 / --json 事件流
└─────────────────────────────────────────────┘
```

### 12.2 关键架构约束（可重入）
执行引擎必须从第一天起是**可重入的库**，不依赖全局状态、不写死在 `main()`：

```
[CLI 入口] ──┐
             ├──> [执行引擎 (纯函数式，可反复调用)] ──> [平台抽象]
[调度器 v0.2]┘
```

- 输入：`Workflow` 模型 + `RunOptions` + 事件回调（trait）。
- 输出：通过回调推送事件，返回最终退出码。
- v0.2 的 cron 调度器在同一进程内反复调用引擎，无需重启。

---

## 13. 核心数据模型

```rust
// 顶层 workflow
pub struct Workflow {
    pub version: u32,              // 必须为 1
    pub env: EnvMap,               // workflow 级 env
    pub working_directory: Option<PathBuf>,
    pub jobs: Vec<Job>,            // 保序，但执行由 DAG 决定
    pub source: SourceSpan,        // 用于错误定位
}

pub struct Job {
    pub id: String,                // job 标识符
    pub needs: Vec<String>,        // 依赖
    pub env: EnvMap,
    pub working_directory: Option<PathBuf>,
    pub timeout_minutes: Option<u32>,
    pub if_condition: Option<Expr>,
    pub steps: Vec<Step>,
}

pub struct Step {
    pub name: Option<String>,
    pub run: String,               // 必填
    pub shell: Shell,              // 显式必填枚举
    pub working_directory: Option<PathBuf>,
    pub env: EnvMap,
    pub if_condition: Option<Expr>,
    pub timeout_minutes: Option<u32>,
    pub continue_on_error: bool,
    pub retry: Option<Retry>,
}

pub enum Shell {
    Pwsh,
    Cmd,
    Bash,
    Sh,
}

pub struct Retry {
    pub attempts: u32,             // 含首次
    pub delay: Duration,
}

// if 受限子集 AST
pub enum Expr {
    Success,
    Failure,
    Always,
    Eq(String, Literal),
    Ne(String, Literal),
}

pub enum Literal {
    Str(String),
    Num(f64),
    Bool(bool),
    Null,
}

pub type EnvMap = Vec<(String, String)>; // 保序，避免 HashMap 的非确定性

// 运行期上下文（引擎输入）
pub struct RunOptions {
    pub max_parallel: Option<usize>,
    pub working_dir: PathBuf,
    pub json_output: bool,
    pub quiet: bool,
    pub color: bool,
}

// 事件回调 trait（可重入的关键）
pub trait EventSink {
    fn emit(&mut self, event: Event);
}

pub enum Event {
    RunStart { workflow: String, ts: Timestamp },
    JobStart { job: String, ts: Timestamp },
    StepStart { job: String, step: String, ts: Timestamp },
    StepOutput { job: String, step: String, stream: Stream, line: String },
    StepEnd { job: String, step: String, code: u32, duration_ms: u64 },
    JobEnd { job: String, code: u32, duration_ms: u64 },
    RunEnd { code: u32, duration_ms: u64, ts: Timestamp },
}
```

> **设计要点**：`EnvMap` 用 `Vec` 而非 `HashMap`，保证遍历顺序确定，便于测试断言与可复现构建。

---

## 14. 执行流程

### 14.1 总体流程
```
load file
  → parse YAML
  → deserialize to Workflow
  → validate (version/必填/枚举/未识别字段报错/行号)
  → build DAG (needs)
  → detect cycle (环则报错并打印环路径)
  → Kahn 拓扑排序 → 就绪队列（入度 0 的 job 入队）
  → 调度循环（就绪队列 + 全局 max_parallel 信号量）:
       任意 job 在其 needs 全部结算（success/failure/skipped）后入队
       spawn thread per job（受全局信号量限流）:
         → for each step in job:
              evaluate if (跳过 or 执行)
              interpolate ${VAR}
              spawn shell process (进程树绑定)
              stream stdout/stderr → EventSink
              wait with timeout
              on non-zero: retry or fail
              read $WAN_OUTPUT → 注入后续 env
         → 结算 job 状态（success/failure/skipped）→ 通知下游就绪检查
  → 全部 job 结算 → emit RunEnd
  → return exit code
```

### 14.2 DAG 调度算法
- **拓扑排序**：Kahn 算法（入度表 + 队列），仅用于环检测与初始就绪集合，**不做分层 barrier**。
- **环检测**：拓扑排序后若仍有节点未输出，则存在环；从残留节点中 DFS 找出环路径并打印。
- **就绪队列（work-list）调度**：job 在全部 `needs` 结算（success/failure/skipped）后进入就绪队列，调度线程取队派发。`max_parallel` 为**全局**信号量（`std::sync::Semaphore` 或自旋计数），作用于整个 run 而非单层，默认无上限。分层 barrier 会引入无关串行化（同层长 job 阻塞下一层短 job），v0.1 **不采用**。
- **状态结算与传播**：job 结算为三态 `success` / `failure` / `skipped`（三态均为"已结算"，下游不等待）。
  - 下游默认（无 `if`）与 `if: success()`：当且仅当所有依赖均为 `success` 才执行；否则跳过。
  - `if: failure()`：任一依赖为 `failure` 时执行（`skipped` 不触发）。
  - `if: always()`：恒执行。
  - 被跳过的 job 不发射 `job_start`/`step_start` 事件，不产生失败退出码。

### 14.3 进程执行细节
- 每个 step 派生一个子进程。**脚本统一写临时文件执行**，解决三类问题：`cmd /c` 内联引号规则、多行脚本、Windows 命令行 32K 长度上限：
  - `pwsh` → `pwsh -NoProfile -NonInteractive -File <tmp.ps1>`
  - `cmd` → `cmd /d /s /c "<tmp.cmd>"`
  - `bash` → `bash --noprofile --norc -e -o pipefail <tmp.sh>`
  - `sh` → `sh -e <tmp.sh>`
  - 临时文件在 step 结束后删除；step 失败则保留并在错误信息中附路径，便于排查。
- env 三层合并：workflow → job → step（后者覆盖前者），再注入 runner 内置变量（`$WAN_OUTPUT` 等，优先于用户 env）。
- 工作目录：step > job > workflow > CLI `-C` > 进程 CWD（一律 `PathBuf::join`，不手写字符串拼接）。
- stdin：子进程默认 `Stdio::null()`，避免 CI 管道/交互终端下子进程读 stdin 挂起或吞输入。
- stdout/stderr：行缓冲实时读取，通过 `mpsc` 推送到 `EventSink`。

---

## 15. 关键技术方案

### 15.1 进程树终止（W3/L3 关键）

**Windows 问题**：无 Unix 信号语义，`TerminateProcess` 只杀单进程，子进程会成孤儿。Ctrl+C 无法可靠传递到子进程树。

**Linux 问题**：`SIGTERM` 默认只发给直接子进程，孙进程可能残留；若不显式建进程组，无法一次性终止整树。

**Windows 方案**：Win32 **Job Object**。
1. 创建 Job Object，设 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`。
2. 每个子进程创建后立即 `AssignProcessToJobObject`。
3. Ctrl+C 时：runner 捕获 `Ctrl+C` 事件（`SetConsoleCtrlHandler`），调用 `TerminateJobObject`，整个进程树一次性终止。
4. runner 进程退出时 Job Object 句柄关闭，`KILL_ON_JOB_CLOSE` 兜底。

**Linux 方案**：`setpgid` + `killpg`。
1. 子进程 fork 后立即 `setpgid(0, 0)`，使其自成新进程组（gid = pid）。
2. Ctrl+C 时：runner 捕获 `SIGINT`，向子进程组 `killpg(pgid, SIGTERM)`。
3. 等待短超时后，若仍有残留，`killpg(pgid, SIGKILL)` 兜底。
4. `waitpid` 回收所有子进程，避免僵尸。

**依赖**：`windows-sys`（仅 `Win32_System_JobObjects` + `Win32_System_Threading` + `Win32_Foundation` + `Win32_System_Console` feature）；Unix 用 `libc` 的 `setpgid`/`killpg`/`waitpid`。两者均在平台抽象层 `cfg` 隔离，体积可控。

### 15.2 Shell 抽象与编码处理（W1/W4/L1/L4 关键）
- **显式 shell 枚举**：`shell` 字段必填，不推断（避免 GH Actions 那套"按平台猜 bash/pwsh"的隐式行为）。
- **跨平台 shell 矩阵**：

  | Shell | Windows | Linux | 说明 |
  |---|---|---|---|
  | `pwsh` | ✅ 原生 | ✅ 原生（若已装 PowerShell 7+） | 跨平台一致首选 |
  | `cmd` | ✅ 原生 | ❌ 报错（平台不支持） | Windows 专用 |
  | `bash` | ⚠️ 需 Git Bash | ✅ 原生（默认 shell） | Linux 首选 |
  | `sh` | ⚠️ 需 Git Bash | ✅ 原生 | POSIX 兜底 |

  平台不匹配时**直接报错**，不静默回落。
- **只支持 `pwsh` 不支持 `powershell` 5.1**：明确声明，避免 Desktop/Core 行为差异坑。
- **强制 UTF-8**：runner 启动时在 Windows 调 `SetConsoleOutputCP(CP_UTF8)`；Linux 默认已是 UTF-8，但启动时检测 `LANG`/`LC_ALL`，若非 UTF-8 则 warning；子进程 env 注入 `PYTHONIOENCODING=utf-8` 等常见编码变量。
- **`pwsh` 缺失检测**：若 `pwsh` 不在 PATH，报错并提示安装 PowerShell 7+，不静默回落。
- **`bash`/`sh` 在 Windows 缺失检测**：若 Git Bash 未装，报错提示安装，不静默回落。

### 15.3 路径分隔符处理（W2/L2 关键）
- `working-directory` 与 `env` 中的路径值：解析时规范化为 `PathBuf`，跨平台用 `PathBuf` 传递，由 OS 处理分隔符。
- 用户 YAML 中混用 `\`/`/`：Windows 上 `PathBuf::from` 原生接受两者，无需手写转换；Linux 上 `\` 是合法文件名字符，不当作分隔符，**Linux 不做 `\`→`/` 转换**。
- `env` 中的路径值**不做转换**，按原样传给子进程（env 值不一定是路径，猜测性转换会破坏 URL/参数等非路径值）；W2 的验收口径见 §3.2。
- **跨平台一致行为**：同一 workflow 文件在 Windows 与 Linux 上路径都能正确解析。
- **禁止拼接**：不手写字符串拼路径，一律 `PathBuf::push` / `join`，避免 `C:\foo/bar\baz` 或 `/foo\bar` 这类混用。

### 15.4 变量插值与 if 求值
- **插值**：手写 `${VAR}` 扫描器，从合并后的 `EnvMap` 解析；未定义保留原样 + warning。
- **if 求值**：手写递归下降解析器（仅 `==`/`!=`/四函数），~30KB，不引 `evalexpr`。
- **遇到 `${{ }}` 输入直接报错**，提示不支持。

### 15.5 cron 调度架构（v0.2 预留）
v0.1 不实现，但执行引擎已满足可重入约束。v0.2 增量：

```
[daemon 进程]
  ├─ cron 表达式解析 → 下次触发时间
  ├─ 定时器线程 → 到点调用 engine::run(workflow, opts, sink)
  ├─ 执行历史持久化 (JSONL 落盘，v0.2 不引 SQLite 避免体积)
  └─ 信号处理 (优雅关闭)
```

- 服务化：Windows 用 `sc.exe` 注册或 `schtasks` 包装；Linux 用 systemd unit；v0.2 评估。
- 错过窗口策略：默认跳过，`--catch-up` 可选补跑最近一次。

### 15.6 结构化日志输出
- `EventSink` trait 两个实现：
  - `HumanSink`：颜色 + 前缀 + 耗时（默认）。
  - `JsonSink`：每行一个 JSON 事件（`--json`）。
- 测试用 `CapturingSink` 收集事件，断言事件序列而非终端文本。

### 15.7 YAML 解析层薄封装
为应对 `serde_norway`/`saphyr` 生态断层，解析层定义中间 `Document` 类型，serde 只负责"YAML→Document"，schema 校验在 `Document→Workflow` 阶段手写。换 YAML 后端时只动一层。
- **位置能力（F-PARSE-4/5 的关键）**：`serde_norway` 不提供 Value 级 span，「未识别字段 + 行列号」在其上不可达；`saphyr` 的事件流带位置信息。**开工第一周 spike 验证**「未知字段报错带行列号」在所选后端可达；不可达则主选 saphyr，或在 Document 层手写 key 比对。
- 版本现状（2026-08-02 实测 crates.io）：`serde_norway` 0.9.42 自 2024-12 后无更新；`saphyr` 0.0.11 更新至 2026-07，维护活跃。**主选 saphyr**，`serde_norway` 为备选。

---

## 16. 项目结构

```
wan/
├── Cargo.toml
├── Cargo.lock
├── README.md                  # 首屏写"不做什么"清单
├── docs/
│   ├── product-review-2026-08-02.md   # 评审报告（已有）
│   └── spec.md                          # 本文档
├── src/
│   ├── main.rs                # CLI 入口（薄）
│   ├── cli.rs                 # lexopt 参数解析
│   ├── model.rs               # Workflow/Job/Step/Expr 数据模型
│   ├── parser.rs              # YAML → Document → Workflow + 校验 + 错误定位
│   ├── scheduler.rs           # DAG 拓扑 + 环检测 + 并行调度
│   ├── engine.rs              # 可重入执行引擎 (run 函数)
│   ├── executor.rs            # step 进程派生 + 流式日志 + 重试/超时
│   ├── shell.rs               # Shell 枚举 → 命令行构造
│   ├── expr.rs                # ${VAR} 插值 + if 受限求值
│   ├── output.rs              # EventSink / HumanSink / JsonSink
│   ├── platform/
│   │   ├── mod.rs             # 平台抽象 trait
│   │   ├── windows.rs         # Job Object / code page
│   │   └── unix.rs            # setpgid/killpg
│   └── error.rs               # 统一错误类型 (带行号)
├── tests/                     # 集成测试（端到端 workflow）
│   ├── parse_valid.yml
│   ├── parse_invalid.yml
│   ├── dag_cycle.yml
│   └── ...
└── .github/workflows/ci.yml   # CI: 体积门禁 + 三平台 + hyperfine
```

---

## 17. 依赖清单与体积预算

### 17.1 Cargo.toml 依赖（目标）
```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
saphyr = "0.0.11"                 # 或 serde_norway = "0.9"，二选一，薄封装；主选 saphyr（位置能力 + 维护活跃，见 §15.7）
lexopt = "0.3"
jiff = { version = "0.2", default-features = false }   # 裁剪 tzdb 时区数据库；或 time = "0.3"
anstyle = "1"

[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.61", features = [
    "Win32_System_JobObjects",
    "Win32_System_Threading",
    "Win32_Foundation",
    "Win32_System_Console",
]}

[target.'cfg(unix)'.dependencies]
libc = "0.2"
```

### 17.2 体积预算（经验估算，需 CI 实测校准）
| 项 | 预算 |
|---|---|
| Rust std + 基础 | ~800 KB |
| serde + serde_json | ~500 KB |
| saphyr / serde_norway (YAML) | ~400 KB |
| lexopt | ~10 KB |
| jiff/time（`default-features = false` 后） | ~150 KB |
| anstyle | ~20 KB |
| windows-sys (限定 feature) | ~200 KB |
| libc | ~100 KB |
| 手写代码 (expr/parser/scheduler/engine) | ~200 KB |
| **合计估算** | **~2.5 MB** |
| **门禁阈值** | **6 MB**（留 2.4x 余量） |

> 若实测超 4MB，触发 `cargo bloat --release --crates` 排查。

---

## 18. Cargo 构建配置

```toml
[package]
name = "wan"
version = "0.1.0"
edition = "2021"
rust-version = "1.96"          # 与 sql-guard 对齐

[profile.release]
opt-level = "z"                # 必须实测对比 "s"（官方文档：s 有时更小）
lto = true                     # 跨 crate 死代码消除，收益最大
codegen-units = 1              # 牺牲编译并行度换优化质量
panic = "abort"                # 移除 unwinding（注意改变 panic 行为）
strip = true                   # 移除符号表

[profile.release-fast]         # 开发期用的快速 release
inherits = "release"
opt-level = 2
lto = false
codegen-units = 16
panic = "unwind"
```

> **纪律**：`opt-level="z"` vs `"s"` 必须在 CI 用真实产物体积对比，选更小者，不照抄。

---

## 19. 测试策略

### 19.1 测试分层
| 层 | 范围 | 工具 |
|---|---|---|
| 单元测试 | expr 插值/求值、DAG 拓扑+环检测、env 合并、shell 命令构造 | `#[test]` |
| 解析测试 | 合法/非法 YAML、行号错误、未识别字段报错 | 内嵌 YAML 文件 |
| 集成测试 | 端到端跑一个 workflow，断言 `--json` 事件序列 | `tests/` + `CapturingSink` |
| 平台测试 | 进程树终止（嵌套子进程 + Ctrl+C）、编码、路径 | 手动 + CI 脚本 |
| 体积门禁 | release 产物 < 6MB | CI 脚本 |
| 性能基准 | 冷启动 < 50ms | `hyperfine` |

### 19.2 双平台硬指标测试矩阵

**Windows：**

| 指标 | 测试 |
|---|---|
| W1 | 三种 shell（`pwsh`/`cmd`/`bash`）各跑一个 workflow，断言退出码 |
| W2 | 混合分隔符路径 workflow，断言 working-directory 正确 |
| W3 | `pwsh`→`node`→嵌套子进程，Ctrl+C 后任务管理器验证零残留（CI 用进程数断言） |
| W4 | Windows Server Core 容器跑 `shell: pwsh` 与 `shell: cmd` |

**Linux：**

| 指标 | 测试 |
|---|---|
| L1 | 两种 shell（`bash`/`sh`）各跑一个 workflow，断言退出码；`pwsh`（若装）额外验证 |
| L2 | 跨平台同一 workflow 与 Windows 互跑，断言路径正确 |
| L3 | `bash`→`node`→嵌套子进程，Ctrl+C 后 `pgrep`/`ps` 验证零残留 |
| L4 | `alpine`/`debian:slim` 容器跑 `shell: bash` 与 `shell: sh` |

---

## 20. CI 与发布

### 20.1 CI 流程（`.github/workflows/ci.yml`）
1. **三平台矩阵**：`windows-latest` + `ubuntu-latest` + `macos-latest`（macOS best-effort，失败不阻断）。
2. **检查**：`cargo fmt --check` + `cargo clippy -- -D warnings`。
3. **测试**：`cargo test`。
4. **体积门禁**（双平台均需通过）：
   ```bash
   cargo build --release
   # 三平台各自取值，> 6291456 字节则 exit 1：
   # Linux (ubuntu-latest):     stat -c%s target/release/wan
   # macOS (macos-latest):      stat -f%z target/release/wan
   # Windows (windows-latest, PowerShell): (Get-Item target/release/wan.exe).Length
   ```
   **>6MB → build FAIL**（编译期硬约束）。
5. **性能基准**：`hyperfine --warmup 3 'wan run tests/minimal.yml'`，记录冷启动。基准 workflow 固定为单 step 空脚本 + `shell: cmd`（Windows）/ `shell: sh`（Linux），口径为「runner 启动 → 派生首个 step 子进程」，不含 shell 加载（见 §5）。
6. **体积报告**：`cargo bloat --release --crates` 上传为 artifact。

### 20.2 发布
- tag `v0.1.0` 触发 release workflow。
- 产物：`wan-vX.Y.Z-x86_64-pc-windows-gnu.zip` / `-msvc.zip` / `-x86_64-unknown-linux-gnu.tar.gz` / `-x86_64-apple-darwin.tar.gz`。gnu / Linux / macOS 产物走交叉编译（§11.1）；msvc 产物由 windows-latest runner 原生构建（target `x86_64-pc-windows-msvc`）。
- 每个产物仅含 1 个 .exe，无 DLL、无配套文件。
- 发布说明首屏放「不做什么」清单与「v1.0 前 schema unstable」声明。

---

## 21. 里程碑与路线图

| 版本 | 范围 | 验收 |
|---|---|---|
| **v0.1** | 解析/校验（含首周 spike：行列号可达性、依赖版本确认）/DAG/并行/三 shell/重试/超时/进程树终止/`--json`/CLI | 跑通 Baafoo `test-fullchain` + 删一份脚本 + W1–W4 与 L1–L4 全过 + <6MB |
| **v0.2** | cron 常驻调度 + 执行历史持久化（JSONL） + Windows 服务 + Linux systemd 集成 | 定时跑通一个 workflow + 历史 可查 + RSS <15MB |
| **v0.3** | TUI（评估） + matrix 最简笛卡尔积 | 评估后再定 |
| **v1.0** | schema 稳定承诺 + 文档完善 | 公开发布 |

---

## 附录 A · 决策记录索引

| 决策 | 出处 |
|---|---|
| 选 Rust（非 Go/Zig/C） | 评审报告 §2.1 |
| 自定义 schema（非 GH Actions 兼容） | 评审报告 §2.2 |
| `std::thread`（非 tokio） | 评审报告 §2.5 |
| 纯 CLI + `--json`（非 TUI/Web） | 评审报告 §2.4 |
| 用 `.wan/workflows/`（非 `.github/workflows/`） | 评审报告 §2.2 |
| 体积 CI 门禁 >6MB fail | 评审报告 §2.5 |
| v0.1 不做 cron 但架构预留 | 评审报告 §2.3 |
| 执行引擎可重入库设计 | 评审报告 §2.3 |
| CLI 命令名 `wan`（评审报告暂名 `flow`，以本文档为准） | 本文档 §4.6 |
| 不支持 `${{ }}`，只支持 `${VAR}` | 评审报告 §2.2 |
| 双平台进程树终止（Windows Job Object / Linux setpgid） | 评审报告 §5 / §3.3 |

---

*文档完成 · 2026-08-02 · 基于 product-review-2026-08-02.md 落地 · 首轮技术评审已修订（rev 1）· 待终审*
