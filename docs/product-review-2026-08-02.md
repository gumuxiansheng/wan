# 本地工作流执行器 · 方案评审

> **评审对象**：一个本地运行的任务/工作流执行器（参考 dagu，类 GitHub Actions YAML 配置，强调小体积）
> **评审日期**：2026-08-02
> **评审人**：gstack-product-reviewer
> **阶段**：选型评审（非实现方案）

---

## 数据来源与查证说明

本文所有竞品数据均于 **2026-08-02** 通过以下渠道实测查证：

| 来源 | 用途 |
|---|---|
| GitHub REST API `/repos/{owner}/{repo}` | star 数、语言、最近 push 时间、license、归档状态 |
| GitHub REST API `/repos/{owner}/{repo}/releases/latest` | release 资产文件的**实际字节数** |
| crates.io API `/api/v1/crates/{name}` | crate 版本、更新时间、下载量 |
| 项目 README / BREAKING_CHANGES.md 原文 | 功能支持范围、限制清单 |

**标注约定**：
- ✅ **已查证** —— 数字来自上述 API 实测
- ⚠️ **估算** —— 由实测值按经验倍率推导，未实测
- ❓ **未查证** —— 无可靠来源，仅为经验判断

---

## 结论先行

| 判断项 | 结论 |
|---|---|
| **总判断** | **⚠️ 有条件 Go —— 但不是原始需求描述的那个东西** |
| **致命发现** | 原需求描述的产品**已经存在且成熟**：`bahdotsh/wrkflw`（Rust / 5.2MB / 3,285★ / MIT / v0.8.0 活跃）。照原方案做 = 重复造轮子 |
| **唯一站得住的方向** | **Windows 原生一等公民 + 常驻 cron 调度 + 零容器依赖 + <6MB** |
| **一句话定位** | **"在 Windows 上真的能用的 dagu"** |
| **若真实痛点只是统一 .sh/.ps1** | **直接 No-Go**，装 `just`（2.1MB，35k★），一下午解决 |

---

# 1. 需求质疑

## 1.1 先摆事实：竞品实测数据

**表 1-1 · 竞品体积与维护状态（✅ 已查证，2026-08-02）**

| 工具 | 语言 | 最新版本 | Windows x64 压缩包 | 解压后 ⚠️估算 | Star | 最近 push | License |
|---|---|---|---|---|---|---|---|
| **dagu** | Go | v2.11.2 | **47.8 MB** | ~110–130 MB | 3,692 | 2026-08-02 | **GPL-3.0** |
| **act** | Go | v0.2.89 | 8.0 MB | ~20 MB | 71,281 | 2026-08-01 | MIT |
| **go-task** | Go | v3.52.0 | 15.1 MB (linux) | ~38 MB | 15,920 | 2026-08-02 | MIT |
| **just** | Rust | 1.57.0 | **2.1 MB** | ~5 MB | 35,072 | 2026-07-26 | CC0 |
| **wrkflw** | Rust | v0.8.0 | **5.2 MB** | ~14 MB | 3,285 | 2026-07-03 | **MIT** |
| **mise** | Rust | v2026.8.0 | ❓未查证 | — | 31,421 | 2026-08-02 | MIT |
| **earthly** | Go | — | — | — | 12,045 | **2025-10-23** | BSL |

> ⚠️ **解压后体积为估算**：按经验倍率 Go ≈ 2.5×、Rust ≈ 2.7× 从压缩包推导，**未实测解压**。若需精确数字请本地下载验证。

**表 1-2 · Rust CLI 体积基准线（✅ 已查证，作为"6MB 目标可达性"的证据）**

| 项目 | 版本 | Windows x64 压缩包 | 说明 |
|---|---|---|---|
| `fd` | v10.4.2 | **1.4 MB** (gnu) / 1.5 MB (msvc) | 文件查找，含 regex |
| `ripgrep` | 15.2.0 | **1.8 MB** (gnu) / 1.7 MB (msvc) | 含完整 regex 引擎 + SIMD |
| `just` | 1.57.0 | **2.1 MB** | 含表达式求值 + 脚本调度 |
| `watchexec` | 2.5.1 | **3.2 MB** | 含文件监听 + **进程树管理**（与本项目最接近） |

**结论**：`watchexec` 3.2MB 已包含"跨平台进程树管理"这个本项目最重的模块。**6MB 目标是宽松可达的，不是苛刻目标。**

## 1.2 两个必须知道的事实

### 事实一：dagu 仓库已迁移，且许可证是 GPL-3.0

- 原 `dagu-org/dagu` 的 API 查询返回 **404**（✅ 已查证）
- 现址为 **`dagucloud/dagu`**，license 字段为 **GPL-3.0**（✅ 已查证）

**影响**：如果打算"参考 dagu 的实现"，**只能看文档和行为，不能读代码后照写**。GPL-3.0 有传染性，一旦被认定为衍生作品，你的项目必须同样开源为 GPL。规避方式：只做黑盒行为观察，实现完全自写。

### 事实二：dagu 已经比传言胖 3 倍

网络上广泛流传的 "dagu 是 ~16MB 单二进制" 是 **v1.x 时代的旧数据**（在本次调研中，DietPi 社区的软件申请贴仍在引用这个数字）。

**v2.11.2 的实测数据（✅ 已查证）**：

| 平台 | 压缩包体积 |
|---|---|
| `dagu_2.11.2_windows_amd64.tar.gz` | **47.8 MB** |
| `dagu_2.11.2_linux_amd64.tar.gz` | 47.1 MB |
| `dagu_2.11.2_darwin_amd64.tar.gz` | 47.7 MB |

膨胀来源（据其 README 自述功能清单）：内嵌 TypeScript/React Web UI + 19 种 executor（Docker/SSH/K8s/postgres/sqlite/JQ/HTTP/S3…）+ 内置 AI Agent + MCP Server + OIDC 认证 + RBAC + Prometheus metrics + OpenTelemetry tracing。

**判定**：**用户对 dagu 体积的不满完全成立，而且实际情况比他以为的更严重。** 这一条支撑了"做一个精简替代"的合理性 —— 但不足以支撑"从零自研"，见下文。

## 1.3 现成方案逐个点评

### wrkflw —— **最关键的竞品，能覆盖约 85%**

| 维度 | 情况 |
|---|---|
| **能覆盖什么** | Rust 实现 / 5.2MB / 直接吃 GitHub Actions YAML / `needs` DAG 依赖解析与并行执行 / `${{ }}` 表达式求值（含 `toJSON`/`fromJSON`/`contains`/`startsWith`）/ matrix 完整支持（`include`/`exclude`/`max-parallel`/`fail-fast`）/ **emulation 模式无需容器** / secure-emulation 沙箱 / TUI（含 DAG 可视化）/ artifact + cache + `needs.<id>.outputs.*` / composite & JS & Docker action / 可复用 workflow / 密钥管理（env/file/Vault/AWS/Azure/GCP）/ watch 模式 / diff-aware 过滤 / **MIT 许可证** |
| **不能覆盖什么** | 见下文 §1.4 的 15% 缺口分析 |
| **判定** | **这是最大的风险源，也是最大的机会** —— MIT 许可意味着"提 PR"和"fork"都是合法且低成本的路径 |

### act —— 否

| 维度 | 情况 |
|---|---|
| **能覆盖什么** | GitHub Actions 兼容度最高（71,281★，事实标准）、`uses:` action 生态完整可用、行为最接近真实 GH runner |
| **不能覆盖什么** | **强依赖 Docker**，这是决定性障碍。在 Windows 上的链路是「Win11 → Docker Desktop → Linux 容器 → 执行脚本」，冷启动数秒到数十秒，且要求用户装 Docker Desktop（授权与资源占用问题）。体积 8.0MB 压缩 / ⚠️~20MB 解压，也不符合体积目标 |
| **判定** | 与"零依赖 + Windows 原生"目标根本冲突 |

### just —— **部分能覆盖，且很可能是用户真正需要的**

| 维度 | 情况 |
|---|---|
| **能覆盖什么** | 2.1MB（全场最小）、35,072★ 生态成熟、跨平台一致、`recipe` 机制天然替代「同一件事写 .sh 和 .ps1 两份」的痛点、支持参数/依赖/变量/条件、Windows 支持良好 |
| **不能覆盖什么** | 无 DAG 并行执行（recipe 的 dependency 是串行前置，非并行调度）、**无 cron 常驻调度**、非 GitHub Actions 语法（需重新学一套 DSL）、无重试/超时原语 |
| **判定** | **如果用户的真实痛点只是"统一那堆 test-fullchain.sh / .ps1"，just 就是答案，不需要写新工具。** 这是本次评审最重要的"止损选项" |

### go-task (Taskfile) —— 否

| 维度 | 情况 |
|---|---|
| **能覆盖什么** | YAML 配置（心智接近）、有 `deps` 支持并行、15,920★ 维护活跃、跨平台、有 `sources`/`generates` 增量执行 |
| **不能覆盖什么** | 体积 15.1MB 压缩 / ⚠️~38MB 解压 —— **比 dagu 好不了多少，完全不满足体积目标**；自有 DSL 而非 GH Actions 语法；无 cron 调度 |
| **判定** | 体积一票否决 |

### mise —— 否

| 维度 | 情况 |
|---|---|
| **能覆盖什么** | Rust 实现、31,421★、活跃（2026-08-02 push）、有 task runner 功能 |
| **不能覆盖什么** | **产品定位是运行时版本管理器**（asdf 替代品），task 是附属功能而非核心。为了 task 功能引入整个版本管理器，体积和心智都不划算；无 DAG 编排；无 cron |
| **判定** | 定位错配 |

### earthly —— 否（且应主动避开）

| 维度 | 情况 |
|---|---|
| **能覆盖什么** | 曾经是很强的可复现构建方案，12,045★ |
| **不能覆盖什么** | **最近一次 push 为 2025-10-23，距今约 9 个月无更新**（✅ 已查证）—— 事实上处于停滞状态。另外强依赖 Docker/BuildKit，体积巨大 |
| **判定** | 维护状态一票否决，不应作为参考对象 |

### make —— 否

| 维度 | 情况 |
|---|---|
| **能覆盖什么** | 无处不在、零安装成本（Linux/macOS） |
| **不能覆盖什么** | **Windows 原生体验灾难**（需 MSYS2/GnuWin32/WSL）、Tab 敏感语法、变量展开规则反直觉、无并行 DAG 的良好表达、无 cron、错误信息可读性差 |
| **判定** | 与 Windows-first 目标直接冲突 |

### dagu —— 否（但作为"要打败的对象"有参考价值）

| 维度 | 情况 |
|---|---|
| **能覆盖什么** | cron 常驻调度（**这是它相对所有 task runner 的核心差异**）、DAG 编排、Web UI、执行历史、重试、通知、19 种 executor、分布式 worker |
| **不能覆盖什么** | **47.8MB 压缩包**、自有 DSL（非 GH Actions 语法）、**GPL-3.0**、功能过载（AI Agent / MCP / OIDC / RBAC 对单机本地场景是纯负担） |
| **判定** | 功能是对的，包装是错的。**"cron 调度"这个特性值得继承，其余全部砍掉** |

### 竞品格局总览

**表 1-3 · 关键能力矩阵**

| 能力 | dagu | act | just | go-task | wrkflw | **目标产品** |
|---|---|---|---|---|---|---|
| 体积 < 6MB | ❌ 47.8MB | ❌ 8MB | ✅ 2.1MB | ❌ 15MB | ⚠️ 5.2MB | ✅ |
| 零运行时依赖（无 Docker） | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ |
| GH Actions 心智模型 | ❌ | ✅ | ❌ | ❌ | ✅ | ✅ |
| DAG 并行（`needs`） | ✅ | ✅ | ❌ | ⚠️ deps | ✅ | ✅ |
| **cron 常驻调度** | ✅ | ❌ | ❌ | ❌ | **❌** | ✅ |
| **Windows 原生一等公民** | ⚠️ | ❌ | ✅ | ✅ | **❌** | ✅ |

**空白格子只有一个：cron 调度 + Windows 原生 + 小体积 + GH Actions 心智，四者同时满足。** 这就是唯一的机会窗口。

## 1.4 wrkflw 缺的那 15% 具体是什么

这是本次评审中最需要精确回答的问题。以下缺口**全部来自 wrkflw 官方 README 与 BREAKING_CHANGES.md 原文**（✅ 已查证，2026-08-02）。

**表 1-4 · wrkflw 能力缺口清单**

| # | 缺口 | 证据（官方文档原文/事实） | 对本项目的意义 | 严重度 |
|---|---|---|---|---|
| **1** | **完全没有 cron / 常驻调度** | CLI 子命令仅有 `validate` / `run` / `watch` / `tui` / `list` / `trigger` / `trigger-gitlab`。`watch` 是**文件变更触发**（`--debounce` / `--ignore-dir`），不是时间调度。全文无 cron/schedule/daemon/service 概念 | **这是最大的、结构性的缺口**。dagu 的灵魂特性它完全没有 | 🔴 决定性 |
| **2** | **`shell:` 只实现了 bash 和 sh** | BREAKING_CHANGES.md 明文：「The `bash` shell now executes with `bash --noprofile --norc -e -o pipefail -c`, matching GitHub Actions behavior. The `sh` shell uses `sh -e -c`.」—— **通篇没有 cmd / pwsh / powershell 的处理说明** | Windows 上必须自带 bash（Git Bash/WSL）才能跑。**对 Windows-first 定位是致命的** | 🔴 决定性 |
| **3** | **`runs-on: windows-*` 被静默映射为容器** | README "Not yet supported" 原文：「`runs-on: windows-*` / `macos-*` is silently mapped to a container image (…Windows → a Windows container that won't run on Linux/macOS hosts). `${{ runner.os }}` reflects the host OS, not `runs-on`.」 | Windows 是明确的二等公民，且是**静默降级**（最糟的失败模式） | 🔴 决定性 |
| **4** | `services:` 解析但从不启动 | README 原文：「`services:` is parsed but never started, in any runtime」 | 半成品语义 —— 用户以为支持，实际静默跳过 | 🟡 中 |
| **5** | `concurrency:` 解析但不执行 | README 原文：「`concurrency:` groups and `cancel-in-progress` — parsed but not enforced」 | 同上，属于"假装支持"的技术债 | 🟡 中 |
| **6** | 远程 `uses:` 不支持私有仓库 | README 原文：「reusable workflows clone over unauthenticated HTTPS」 | 企业内网场景不可用 | 🟢 低（本项目本就不做 `uses:`） |
| **7** | 体积 5.2MB 压缩 / ⚠️~14MB 解压 | ✅ 实测 release 资产 | 比 6MB 目标大 2 倍以上，因为它扛着 16 个 crate（含 TUI/GitLab/GitHub API/6 种密钥后端/matrix/artifact） | 🟡 中 |
| **8** | 加密密钥存储格式刚发生破坏性变更 | BREAKING_CHANGES.md：`EncryptedSecretStore` 在 0.8.0 修复了 AES-GCM nonce 复用漏洞，**无自动迁移路径** | 说明该项目仍在快速演进期，API 稳定性有限 | 🟢 低（信息价值） |

### 15% 缺口的结构性总结

缺口不是"零散的小功能"，而是**两个成块的、互相独立的能力域**：

| 缺口域 | 内容 | 能否通过提 PR 解决 |
|---|---|---|
| **A. 调度域** | cron 表达式解析、常驻进程、Windows 服务/计划任务集成、执行历史持久化、错过窗口的补偿策略 | **理论可以，但改动量大** —— 等于给一个"一次性执行器"加上"长驻调度器"，涉及架构层变更，不是补丁级 PR |
| **B. Windows 原生域** | `shell: cmd` / `shell: pwsh` 实现、`runs-on: windows-*` 的真实本机执行（而非映射容器）、`${{ runner.os }}` 语义修正、路径分隔符、Job Object 进程树终止 | **理论可以，改动量中等** —— 但需要维护者认同"Windows 是一等公民"这个方向，是产品方向之争而非技术问题 |

**关键洞察**：wrkflw 是 **MIT 许可**（✅ 已查证）。这意味着 fork 和贡献都合法且成本可控。**"给 wrkflw 提 PR" 是一条必须先严肃评估的路径，其投入产出比大概率高于从零自研。**

## 1.5 "包体积小"是不是真需求

**诚实判断：部分是伪需求。**

现代机器不缺 50MB 磁盘。"体积小"本身没有用户价值，它的价值全部是**间接**的：

**表 1-5 · 体积的真实价值拆解**

| 表面诉求 | 背后的真实价值 | 是否成立 |
|---|---|---|
| "二进制小" | 冷启动延迟低（用户唯一能"感觉到"的） | ✅ 成立，但**冷启动时间才是该优化的指标，不是 MB 数** |
| "二进制小" | CI 中每次 pipeline 拉取工具的耗时 | ✅ 成立（用户有 GitHub Actions + GitLab CI + CNB 三套 CI，每次都要拉） |
| "二进制小" | 心理上"这工具很克制、值得信任" | ⚠️ 真实但主观，是营销价值不是工程价值 |
| "二进制小" | 节省磁盘 | ❌ 不成立，50MB 无人关心 |

**结论**：体积对本项目**是真需求**，但理由是「CI 拉取成本 + 冷启动 + 定位可信度」，**不是「省磁盘」**。因此成功标准不应只写 MB 数。

### 建议的可量化成功标准

**表 1-6 · v1 成功标准（替代模糊的"小"）**

| 指标 | 目标值 | 判定方式 | 理由 |
|---|---|---|---|
| Windows x64 解压后单文件体积 | **< 6 MB** | CI 门禁自动检查 | 需比 wrkflw（⚠️~14MB）小一倍以上才有说服力；`watchexec` 3.2MB 证明可达 |
| 冷启动到第一个 step 开始执行 | **< 50 ms** | `hyperfine` 基准测试 | 这才是用户能感知的指标 |
| 运行时依赖数量 | **0** | 在纯净 Windows Server Core 容器中验证 | 不需要 Docker / Node / Python / bash |
| 常驻调度模式内存占用 | **< 15 MB** RSS | 任务管理器 / `Get-Process` | 对照 dagu 的 ~20–50MB |
| 单文件分发 | **1 个 .exe** | — | 无 DLL、无配套文件 |

### 为了体积必须牺牲什么（明确写下来）

| 砍掉的东西 | 预估节省 ❓未查证（经验估算） |
|---|---|
| 内嵌 Web UI | ~30 MB+ |
| Docker / Podman / K8s executor | ~3–5 MB |
| `uses:` action 生态（需 Node runtime） | ~5 MB+ |
| 完整 `${{ }}` 表达式引擎 | ~0.5–1 MB |
| 多后端密钥管理（Vault/AWS/Azure/GCP，需 HTTP 客户端 + TLS） | ~3–4 MB |
| 远程触发 / GitLab 集成（需 reqwest + TLS） | ~2–3 MB |
| TUI（ratatui） | ~1.5–2 MB |

**这七样全砍，才可能进 6MB。这就是"精简"的真实代价 —— 它意味着产品能力只有 wrkflw 的一半。**

---

# 2. 关键设计决策矩阵

## 2.1 实现语言

**表 2-1 · 语言选型对照**

| 语言 | 空 CLI 基线体积 | 生态成熟度（进程/YAML/并发） | 交叉编译难度 | 与用户技能栈匹配 | 推荐 |
|---|---|---|---|---|---|
| **Rust** | ❓300 KB – 1 MB（`opt-level="z"` + `lto=true` + `strip=true` + `panic="abort"` + `codegen-units=1`，经验估算） | ✅ 齐全：`std::process`、成熟 YAML crate、`std::thread` | 中等（需装 target + linker），**但用户已打通** | **最高** | ✅ **推荐** |
| Go | ❓~2 MB 起步（`-ldflags="-s -w"` 后），实测难压到 5MB 以下 | ✅ 最成熟，`os/exec` + goroutine 天然契合 | ✅ 最简单（`GOOS=windows go build`） | 低（用户主力 Java 8，无 Go 经验） | ❌ |
| Zig | ❓~100 KB | ❌ **进程管理 / YAML 生态几乎为零**，需大量自写 | ✅ 最强（内置 cross-compilation） | 低 | ❌ 会掉进造轮子陷阱 |
| C | ❓~50 KB | ❌ 需手写一切；Windows 进程/Job Object API 是深坑 | ❌ 差 | 低 | ❌ |

> ⚠️ 上表"空 CLI 基线体积"为经验估算，**未实测**。可靠的是表 1-2 的真实项目实测值。

### 推荐：Rust

理由不是性能，是三点**已有资产复用**：

1. **工具链零启动成本** —— 用户在 `sql-guard` 项目上已固定 rustc 1.96.0 stable、配置了 USTC 镜像、正在打通 `x86_64-pc-windows-gnu` 交叉编译链路。这套基建可以直接复用。
2. **唯一能同时满足「<6MB」和「生态可用」的选项** —— Go 体积不达标，Zig/C 生态不达标。
3. **可与 sql-guard 共享 CI 发布流程** —— 同样的 `cargo build --release` + strip + GitHub Release 上传脚本。

### 推荐的 Cargo 配置

```toml
[profile.release]
opt-level = "z"       # 优化体积（也需实测对比 "s"，官方文档明确说 "s" 有时更小）
lto = true            # 跨 crate 死代码消除，收益最大的单项
codegen-units = 1     # 牺牲编译并行度换优化质量
panic = "abort"       # 移除 unwinding 机制（注意：会改变 panic 行为）
strip = true          # 移除符号表（Rust 1.59+ 原生支持）
```

> **注意**：`min-sized-rust` 官方文档明确提示 —— 「the "s" and "z" levels not being necessarily smaller」，**必须实测对比 `"s"` 与 `"z"`**，不要照抄。

## 2.2 配置格式：**自定义精简 schema，借鉴心智模型，绝不承诺兼容**

### 核心判断

**"兼容 GitHub Actions YAML 子集"是本项目最大的失控风险源。**

四个无底洞：

| 无底洞 | 为什么无底 |
|---|---|
| `${{ }}` 表达式引擎 | 需实现：函数库（`contains`/`startsWith`/`endsWith`/`format`/`join`/`toJSON`/`fromJSON`/`hashFiles`/`success`/`failure`/`always`/`cancelled`）+ context 对象（`github`/`env`/`vars`/`job`/`jobs`/`steps`/`runner`/`secrets`/`strategy`/`matrix`/`needs`/`inputs`）+ 隐式类型转换规则 + 运算符优先级。**这是一门完整的小语言** |
| `uses:` action 生态 | JS action 需要内嵌 Node runtime；Docker action 需要 Docker；composite action 需要递归展开 + 输入输出映射。**任何一条都足以让体积翻倍** |
| `matrix` 展开 | `include` / `exclude` / `max-parallel` / `fail-fast` 的交互语义极其微妙，`include` 还能新增维度。组合爆炸 |
| `if:` 真值规则 | GH Actions 的隐式真值转换（空字符串、`'false'` 字符串、数字 0）有一堆坑；且 `if:` 内部隐含 `${{ }}` 上下文 |

**wrkflw 已经把这些都做了 —— 代价就是 16 个 crate、⚠️~14MB 体积，以及一堆"parsed but not enforced"的半成品语义（`services:`、`concurrency:`）。这是活生生的前车之鉴。**

### 最小可用语法子集清单

**表 2-2 · v1 语法边界（保留 vs 砍掉）**

| ✅ 保留（v1 必须） | 语义说明 |
|---|---|
| `version: 1` | **顶层版本字段，为未来 schema 迁移预留**（关键设计） |
| `jobs.<id>.steps[].run` | 核心执行单元 |
| `jobs.<id>.steps[].name` | 可读性 |
| `needs: [a, b]` | DAG 依赖，驱动并行调度 |
| `env:` | 三层作用域：workflow → job → step，后者覆盖前者 |
| `${VAR}` | **简单变量插值，不是 `${{ }}`** |
| `if:` | **仅支持**：`success()` / `failure()` / `always()` / `<var> == <literal>` / `<var> != <literal>` |
| `shell:` | 显式指定：`pwsh` / `cmd` / `bash` / `sh`，**必须显式，不猜** |
| `working-directory:` | 每 step 可覆盖 |
| `timeout-minutes:` | job 级 + step 级 |
| `continue-on-error:` | step 级 |
| `retry:` | **非 GH Actions 原生**，本项目扩展：`{ attempts: N, delay: Ns }` |
| `schedule: cron: "..."` | **v0.2 加入**，但 v0.1 就要在 schema 中预留位置 |

| ❌ 砍掉（明确写进 README 首屏） | 砍掉理由 |
|---|---|
| `uses:` —— 整个 action 生态 | 需要 Node runtime / Docker，体积杀手 |
| 完整 `${{ }}` 表达式引擎 | 复杂度无底洞，见上 |
| `matrix:` | v1 完全不做；v2 只考虑最简笛卡尔积，不做 include/exclude |
| `container:` / `services:` | 与"零依赖"定位冲突 |
| `concurrency:` | 单机本地场景价值低 |
| `permissions:` / `secrets:` | v1 走环境变量即可 |
| `artifact` 上传/下载 | **本地执行，文件本来就在磁盘上，这是伪需求** |
| 可复用 workflow（`jobs.<id>.uses`） | 递归展开 + 输入输出映射，复杂度高 |
| `runs-on:` | 单机本地，无 runner 概念。**解析时报错而非静默忽略** |

### 关键决策：不要复用 `.github/workflows/` 目录

**必须用自己的目录**（如 `.flow/` 或 `flows/`）。

理由：一旦用户能把真实的 GitHub Actions 文件直接丢进来跑，你就背上了**无限的兼容性债务** —— 每个不支持的字段都会变成一个 issue。而且 wrkflw 的教训表明，"parsed but not enforced" 是最糟糕的失败模式。

**正确的定位表述**：
> "语法上借鉴 GitHub Actions 的心智模型（jobs / steps / needs / env），但这是一个明确定义的、更小的独立 schema，不是 GitHub Actions 的实现。"

**对未识别字段的处理策略：解析时直接报错，不静默忽略。** 显式失败 > 静默降级。

## 2.3 执行模型

**表 2-3 · 执行能力 v1 取舍**

| 能力 | v1 | 判定理由 |
|---|---|---|
| 串行 steps | ✅ 必须 | 基础 |
| `needs` DAG 拓扑排序 + 并行执行 | ✅ 必须 | **这是相对 `just` 的核心差异**，也是"执行器"名副其实的前提 |
| 环检测（cycle detection） | ✅ 必须 | 拓扑排序的必然副产品，成本极低，错误信息价值高 |
| step / job 超时 | ✅ 必须 | 实现廉价（`wait_timeout`），防止 CI 挂死 |
| 失败重试（`retry`） | ✅ 必须 | 实现极廉价（循环 + backoff），CI 场景刚需（网络抖动） |
| `continue-on-error` | ✅ 必须 | 廉价 |
| 实时日志流式输出 | ✅ 必须 | 不能等 job 结束才吐日志，否则长任务无反馈 |
| 正确的退出码传播 | ✅ 必须 | CI 集成的前提 |
| **Ctrl+C → 进程树完整终止** | ✅ **必须** | **Windows 上的头号坑，见 §5**。做不好这条，工具在 Windows 上不可用 |
| **cron 常驻调度** | ⚠️ **v0.2，但 v0.1 架构必须预留** | **这是相对 wrkflw 的唯一护城河**。v0.1 不做是为了守住 2–3 周工期，但执行引擎必须设计成"可被调度器反复调用"而非"一次性 main 函数" |
| step 间变量传递 | ⚠️ 最简版 | 只做 `$GITHUB_OUTPUT` 风格：step 写临时文件，runner 读回注入后续 step 的 env。**不做表达式引用** |
| artifact 传递 | ❌ 砍 | 本地执行，文件就在磁盘上 |
| 执行历史持久化 | ❌ v1 砍 | 随 cron 一起在 v0.2 考虑（无历史的调度器价值减半） |

### 关键架构约束

即便 v0.1 不做 cron，**执行引擎必须从第一天就设计成可重入的库**：

```
[CLI 入口] ──┐
             ├──> [执行引擎 (纯函数式，可反复调用)]
[调度器]  ──┘         └──> [进程执行层 (平台抽象)]
```

如果 v0.1 把逻辑全写在 `main()` 里，v0.2 加 cron 时就得重写。这是**唯一一处值得为未来付出的设计成本**（对应原则 *Choose completeness*）。

## 2.4 UI 策略：**纯 CLI + 结构化日志，v1 不做 TUI**

**表 2-4 · UI 方案对照**

| 方案 | 体积影响 ❓估算 | 判定 |
|---|---|---|
| **纯 CLI + 彩色输出 + `--json` 结构化日志** | +~50 KB | ✅ **v1 选这个** |
| CLI + TUI（`ratatui`） | +1.5–2 MB | ⚠️ v2 再评估（wrkflw 已有 TUI，做重复功能无意义） |
| 内嵌 Web UI | +30 MB+ | ❌ **绝对不做** —— 这正是 dagu 47.8MB 的主因 |

### `--json` 是关键设计，不是附属功能

输出机器可读的结构化事件流（每行一个 JSON 对象：`job_start` / `step_start` / `step_output` / `step_end` / `job_end` / `run_end`），带来三个杠杆：

1. **把 UI 变成生态问题而不是你的体积问题** —— 别人可以用 `jq`、外部 dashboard、CI 注解消费
2. **让工具可被其他工具编排** —— 符合 Unix 哲学
3. **自动化测试更容易** —— 断言 JSON 事件流比断言终端输出可靠得多

同时提供人类可读的默认输出（带颜色、缩进、耗时统计），通过 `--json` 切换。

## 2.5 依赖策略

**表 2-5 · 依赖选型与体积影响**

| 用途 | 推荐 | 体积影响 ❓估算 | 说明 |
|---|---|---|---|
| **YAML 解析** | **`serde_norway`** 0.9.42 或 **`saphyr`** 0.0.11 | ~400 KB | ⚠️ **`serde_yaml` 已废弃** —— 最新版本号字面写着 `0.9.34+deprecated`，最后更新 2024-03-25，上游仓库 `dtolnay/serde-yaml` **已 archived**（✅ 已查证）。`serde_norway` 是社区维护的直接 fork（2024-12-21 更新）；`saphyr` 更新更活跃（2026-07-11）但 API 更底层、版本号仍是 0.0.x |
| **并发模型** | **`std::thread` + `std::sync::mpsc`** | **省 1–2 MB** | ⚠️ **不要用 tokio**。本项目负载是「编排几十个子进程」，不是「海量 IO 并发」。OS 线程完全够用。tokio 是体积大头，且 async 会传染整个代码库、大幅提高心智负担 |
| **进程执行** | **`std::process::Command`** | 0 | 标准库足够 |
| **CLI 参数解析** | `clap` **必须 `default-features = false`**，或用 `lexopt` | `clap` 完整版 +~800 KB；`lexopt` +~10 KB | **最容易被忽略的膨胀点**。`clap` 的 `suggestions`（含模糊匹配）、`color`、`usage` feature 都很重 |
| **进程树终止** | Windows: Win32 **Job Object**（`windows-sys` crate，只启用需要的 feature）；Unix: `setpgid` + `killpg` | ~200 KB | **不要用跨平台大包**，直接调平台 API |
| **cron 表达式解析**（v0.2） | `cron` 或 `saffron` | ~150 KB | 都很轻 |
| **表达式求值** | **手写递归下降解析器，不引库** | ~30 KB | 子集极小（只有 `==` / `!=` / 三个函数），引 `evalexpr`（~300KB）不划算 |
| **时间处理** | `time` 或 `jiff` | ~200–400 KB | **不要用 `chrono`**（更大且历史包袱多） |
| **终端颜色** | `anstyle` / 手写 ANSI 转义 | ~20 KB | 不要用 `colored`/`termcolor` 全家桶 |
| **JSON 输出** | `serde_json` | ~300 KB | 已有 serde 依赖，边际成本低 |

### 明确的依赖黑名单

| 禁止引入 | 理由 |
|---|---|
| `tokio` | ❓~1–2 MB，且 async 传染 |
| `reqwest` | 带 TLS 栈，❓~3–4 MB。v1 无网络需求 |
| `regex` | ❓~1.5 MB。能用手写 matcher / `str::starts_with` 解决就不引 |
| `chrono` | 比 `time` 大，且时区数据库重 |
| 任何前端框架 / 静态资源嵌入 | dagu 的教训 |

### 体积门禁（工程纪律）

在 CI 中加一条硬检查：

```
构建产物 > 6 MB → build FAIL
```

**让体积成为编译期硬约束，而不是一个"希望保持"的愿望。** 这是唯一能防止体积失控的机制（对应原则 *Explicit over clever*）。

---

# 3. 差异化定位

## 3.1 唯一卖点

> ## **"在 Windows 上真的能用的 dagu。"**
>
> 完整表述：**Windows 原生一等公民的、带常驻 cron 调度的、零依赖、单文件 < 6MB 的本地流水线执行器。**

## 3.2 卖点的证据链

**表 3-1 · 为什么这个格子是空的**

| 竞品 | 缺的那一块 | 缺口性质 |
|---|---|---|
| **wrkflw** | **无 cron 调度**；`shell:` 只有 bash/sh；`runs-on: windows-*` 静默映射为容器 | 结构性（架构层） |
| **act** | 强依赖 Docker，Windows 链路最重 | 结构性（设计前提） |
| **dagu** | 有 cron，但 47.8MB + 自有 DSL + GPL-3.0 | 结构性（产品哲学） |
| **just** | 2.1MB 很优秀，但无 DAG 并行、无 cron | 功能性（可补但非其定位） |
| **go-task** | ⚠️~38MB 解压，无 cron | 结构性（Go 体积下限） |

## 3.3 "Windows 原生"必须是真做，不是宣称

这是整个差异化的支点。**必须同时满足以下四条硬指标，否则卖点不成立：**

**表 3-2 · Windows 原生的四条验收标准**

| # | 标准 | 验证方式 |
|---|---|---|
| **W1** | `shell:` 原生支持 `pwsh` / `cmd` / `bash`（Git Bash）三选一，**显式指定，不猜测** | 三种 shell 各写一个 workflow 跑通 |
| **W2** | 路径分隔符自动处理：`working-directory`、`env` 中的路径在 `\` 与 `/` 之间正确转换，不出现 `C:\foo/bar\baz` | 混合分隔符的路径测试用例 |
| **W3** | **Ctrl+C 能通过 Win32 Job Object 完整终止整个子进程树**，无孤儿进程残留 | 启动嵌套子进程（如 `pwsh` → `node` → `java`）后按 Ctrl+C，用任务管理器验证零残留 |
| **W4** | **零外部依赖**：在纯净 Windows Server Core 容器中（无 Docker / WSL / Node / Python / Git Bash）能跑通 `shell: pwsh` 和 `shell: cmd` 的 workflow | 纯净容器验证 |

> **⚠️ 硬性判据：这四条中任何一条做不到 → 项目失去存在理由 → 直接 No-Go**，转为「wrkflw + Windows 计划任务」的组合方案。

## 3.4 反向检验：如果说不出卖点会怎样

如果去掉「Windows 原生」和「cron 调度」这两条，剩下的就是「一个更小的 wrkflw」。

**"更小"本身不是卖点** —— 用户不会为了省 8MB 去换一个功能少一半、生态为零、只有一个作者的工具。**"小"只有在它带来"零依赖 + 快启动 + 在别人跑不了的环境里能跑"时，才转化为真实价值。**

---

# 4. MVP v0.1 范围

**约束**：2–3 周业余时间可完成。

## 4.1 必须做（v0.1 Scope In）

**表 4-1 · v0.1 功能清单**

| 模块 | 具体内容 |
|---|---|
| **解析** | YAML → 内部模型；schema 校验；**错误信息必须带行号和列号**（这是 DX 的第一道门面） |
| **语法支持** | `version` / `jobs` / `steps` / `run` / `name` / `needs` / `env`（三层作用域）/ `shell` / `working-directory` / `timeout-minutes` / `continue-on-error` / `retry` / `if`（受限子集） |
| **调度** | DAG 拓扑排序 + 环检测 + `std::thread` 并行执行独立 job；`--max-parallel N` 限流 |
| **执行** | `shell:` 三选一（`pwsh` / `cmd` / `bash`）；`${VAR}` 插值；`if:` 受限求值（`success()`/`failure()`/`always()`/单变量比较） |
| **可靠性** | step 超时；job 超时；重试 + backoff；**Ctrl+C → Job Object 进程树完整清理**；正确的退出码传播 |
| **输出** | 实时流式日志（带 job/step 前缀、耗时）；`--json` 结构化事件流 |
| **CLI** | `flow run <file>` / `flow validate <file>` / `flow list` / `flow graph <file>`（输出 mermaid 文本，零体积成本的"可视化"） |
| **架构** | 执行引擎设计为可重入的库（为 v0.2 的 cron 预留） |
| **工程** | CI 体积门禁（>6MB fail）；Windows + Linux 双平台 CI；`hyperfine` 冷启动基准 |

## 4.2 明确砍掉（v0.1 Scope Out）

**表 4-2 · 明确不做的清单（必须写进 README 首屏）**

| 砍掉的功能 | 砍掉理由 | 未来 |
|---|---|---|
| **`uses:` —— 整个 action 生态** | 需 Node runtime / Docker，体积杀手 | **永不做** |
| **完整 `${{ }}` 表达式引擎** | 复杂度无底洞 | **v1 永不做**，v2 也需极度克制 |
| `matrix` | 组合语义复杂 | v0.3 考虑最简笛卡尔积 |
| `container:` / `services:` | 与零依赖定位冲突 | **永不做** |
| Docker / Podman / K8s executor | 同上 | **永不做** |
| TUI / Web UI | 体积；且 wrkflw 已有 TUI | v0.3 考虑 TUI |
| artifact 上传/下载 / cache | 本地执行，伪需求 | **永不做** |
| 密钥管理后端（Vault/AWS/Azure/GCP） | 需 HTTP + TLS 栈 | **永不做**，走环境变量 |
| 远程触发 / GitLab 集成 | 需网络栈 | **永不做** |
| 可复用 workflow | 递归展开复杂 | v0.4 考虑 |
| 执行历史持久化 | — | **v0.2 随 cron 一起做** |
| **cron 调度本身** | 守住 2–3 周工期 | **v0.2 第一优先级**（架构已预留） |
| `runs-on:` | 单机无 runner 概念 | 解析时报错，永不支持 |

## 4.3 v0.1 验收标准

> **用它跑通 Baafoo 项目的 `test-fullchain` 全链路，并删掉 `.sh` / `.ps1` 中的一份。**

这是唯一有意义的成功判据。理由：**用户自己就是第一个"绝望用户"** —— 他有 GitHub Actions + GitLab CI + CNB 三套 CI 和成对的 shell/ps1 脚本，这是真实的、可观察的痛点，不是假想需求。

如果连自己的项目都用不上，就不该发布。

---

# 5. 风险清单（按严重度排序）

**表 5-1 · 风险登记册**

| 级别 | 风险 | 影响 | 缓解措施 |
|---|---|---|---|
| 🔴 **P0-1** | **wrkflw 已占位** —— 它已实现 85% 的目标功能，且是 MIT + 活跃维护（3,285★）。若它补上 cron 和 Windows 原生支持，护城河瞬间归零 | 项目失去存在理由 | **第一周必须完成 wrkflw 实测（见 §6）**。这一步不做完，不要写第一行代码。若判定"够用" → 立即 No-Go 止损 |
| 🔴 **P0-2** | **Windows 兼容性** —— ① Ctrl+C 无法传递给子进程树，产生孤儿进程；② `pwsh` (Core 7.x) 与 `powershell` (Desktop 5.1) 语法/行为差异；③ 路径分隔符混用；④ 退出码语义不一致；⑤ 控制台编码（GBK/UTF-8/BOM）导致中文日志乱码 | **在 Windows 上不可用 = 项目唯一卖点消失** | ① 从第一天以 Windows 为**主**开发平台（用户在 Win11，是优势）；② 进程树清理**必须**用 Win32 Job Object，**不要指望信号语义**；③ 显式声明只支持 `pwsh` 不支持 `powershell` 5.1；④ 强制 UTF-8 输出 + 启动时设置 code page |
| 🔴 **P0-3** | **表达式引擎复杂度失控** —— 一旦开口支持 `${{ }}`，用户会持续要求 `fromJSON` / `hashFiles` / `contains` / matrix context…，功能边界无法收敛 | 项目膨胀成第二个 dagu，2–3 周工期变成半年 | v1 **明确不支持 `${{ }}`**，只支持 `${VAR}`。**写进 README 第一屏**。对 `${{ }}` 的输入**直接报错并提示不支持**，绝不静默处理 |
| 🟡 **P1-1** | **体积目标与功能需求直接冲突** —— 加上 cron + 表达式 + TUI 后必然突破 6MB | 卖点稀释，变成"又一个中等大小的工具" | **CI 体积门禁：>6MB 直接 fail build**。让体积成为编译期硬约束。每次加依赖前先跑 `cargo bloat` |
| 🟡 **P1-2** | **YAML schema 一旦公开就难改** —— 早期设计失误会被用户配置文件锁死 | 长期技术债 | ① 配置顶层强制 `version: 1` 字段，预留迁移通道；② README 首屏明写 **"schema unstable until v1.0"**；③ v0.x 阶段保留破坏性变更的权利并明确声明 |
| 🟡 **P1-3** | **"像 GitHub Actions"的心智误导** —— 用户拿真实 workflow 文件来跑，然后为每个不支持的字段提 issue | 无限兼容性债务 + 口碑受损 | ① **不复用 `.github/workflows/` 目录**，用 `.flow/`；② README 首屏放"不支持清单"；③ 遇到未识别字段**报错退出**，不静默忽略（wrkflw 的 `services: parsed but never started` 是反面教材） |
| 🟡 **P1-4** | **依赖生态的隐性膨胀** —— 每个 crate 都"看起来很小"，累加后突破预算 | 体积失控 | 每次新增依赖必须：① 检查 `default-features = false` 是否可行；② 跑 `cargo bloat --release --crates` 记录增量；③ 记入依赖决策日志 |
| 🟢 **P2-1** | **业余时间不足导致烂尾** | 项目死亡 | v0.1 范围已压至最小；**先满足自用（跑通 Baafoo）再谈开源**。自用价值在第 1 周就能兑现，不依赖发布 |
| 🟢 **P2-2** | **dagu 的 GPL-3.0 传染** | 法律风险 | **只读文档和观察行为，绝不读其源码**。实现完全自写 |
| 🟢 **P2-3** | **`serde_yaml` 生态断层** —— 主流 YAML crate 已废弃，替代品（`serde_norway` 2024-12 更新 / `saphyr` 仍是 0.0.x）成熟度存疑 | 可能需中途更换 YAML 后端 | 在解析层做薄封装（自定义 `Document` 中间类型），使 YAML crate 可替换。成本很低，收益明确 |
| 🟢 **P2-4** | 单人项目的 bus factor | 无人接手 | 接受。定位为"自用工具 + 顺便开源"，不承诺 SLA |

---

# 6. wrkflw 实测验证清单（P0-1 的执行方案）

> **这是启动项目前的强制门禁。1 周时间盒，不得跳过。**

## 6.1 准备

```bash
cargo install wrkflw     # 或 brew install wrkflw
```

测试环境：**Windows 11 原生**（不要在 WSL 里测 —— WSL 里测出的结论对本项目无意义）。

## 6.2 必测场景与判定标准

**表 6-1 · 验证清单**

| # | 场景 | 具体操作 | ✅ "够用"的结果 | ❌ "不够用"的结果 | 权重 |
|---|---|---|---|---|---|
| **T1** | **Windows 原生 shell 执行** | 写一个 workflow，step 使用 `shell: pwsh` 和 `shell: cmd`，在 Win11 原生（**不装 Git Bash / 不开 WSL**）下用 `--runtime emulation` 运行 | 两种 shell 都能正确执行并返回正确退出码 | 报错、静默跳过、或强制回落到 bash | **决定性** |
| **T2** | **Baafoo 真实流水线** | 取 `test-fullchain.ps1` 的内容，拆成 3–5 个 job（带 `needs` 依赖），用 wrkflw 跑 | 全链路跑通，日志可读，失败时退出码正确 | 中途失败且原因不可诊断 | **决定性** |
| **T3** | **cron / 定时调度** | 查找是否有任何方式让 wrkflw 按 cron 表达式常驻定时执行 | 存在可用的调度机制 | **无**（已预判：CLI 只有 `validate`/`run`/`watch`/`tui`/`list`/`trigger`，`watch` 是文件变更触发） | **决定性** |
| **T4** | **进程树终止** | 启动一个嵌套子进程的 job（如 `pwsh` 启动 `java -jar`），执行中按 Ctrl+C，检查任务管理器 | 所有子进程被清理，零残留 | 有孤儿进程残留 | **决定性** |
| **T5** | **冷启动性能** | `hyperfine "wrkflw run minimal.yml"`（最小 workflow，单 step `echo`） | < 200 ms | > 500 ms | 高 |
| **T6** | **实际体积** | 解压 release 包，实测 .exe 字节数 | < 10 MB | > 10 MB（⚠️预估 ~14MB） | 中 |
| **T7** | **常驻内存** | 若 T3 找到调度方案，观察常驻 RSS | < 30 MB | > 50 MB | 中 |
| **T8** | **错误信息质量** | 故意写错 YAML（缩进错、字段名拼错、循环 `needs`） | 报错带行号，能直接定位 | 报错模糊（如 "invalid workflow"） | 中 |
| **T9** | **中文日志编码** | step 输出中文，检查终端显示 | 正常显示 | 乱码 | 中 |
| **T10** | **贡献可行性** | 阅读 CONTRIBUTING.md；在 issue 区搜索 "windows" / "cron" / "schedule"，看维护者对这两个方向的态度 | 维护者对 Windows/cron 支持持开放态度 | 明确拒绝或长期无响应 | 高 |

## 6.3 决策规则（可判定，无歧义）

**表 6-2 · 三分判定**

| 判定 | 触发条件 | 行动 |
|---|---|---|
| **🛑 No-Go（止损）** | **T1、T2、T4 全部通过**（即 Windows 原生可用、真实流水线跑通、进程清理干净），**且** T3 虽无 cron 但可用 Windows 任务计划程序（`schtasks`）+ `wrkflw run` 组合替代 | **停止自研**。直接用 wrkflw + `schtasks`。把省下的 3 周投入 sql-guard 或 Baafoo |
| **🔀 Contribute（次优，投入产出比最高）** | **T3 失败（无 cron）**，但 **T1、T2、T4 通过**，**且 T10 显示维护者开放** | **不自研，给 wrkflw 提 PR**。MIT 许可，改动可控。先提 issue 讨论方向，获得认同后再动手 |
| **✅ Go（自研）** | **T1 失败**（Windows 原生 shell 不可用）**或 T4 失败**（进程树残留），**且** T10 显示维护者对 Windows 方向不积极 | **启动自研**，严格按 §4 的 v0.1 范围执行，卖点锁定"Windows 原生 + cron" |

### 附加止损条款

**在任何时候，如果发现用户的真实痛点只是「统一 test-fullchain.sh / test-fullchain.ps1 两份脚本」** ——

> **直接 No-Go。** 装 `just`（2.1MB，35,072★，✅ 已查证），把两份脚本合并成一个 `justfile`，一下午完成。不要写新工具。

---

# 7. 最终判断

> ## ⚠️ 有条件 Go
>
> ### 条件（必须按顺序满足）
>
> **① 先完成 §6 的 wrkflw 实测（1 周时间盒，强制门禁）**
> 按表 6-2 的三分规则判定。**"够用" → 立即 No-Go 止损；"缺 cron 但可贡献" → 提 PR 而非自研。** 这一步不做完，不要写第一行代码。
>
> **② 若判定 Go，唯一定位锁定为「Windows 原生 + cron 调度 + <6MB」**
> **不是「精简版 dagu」** —— 那个定位说不出为什么用户要用你而不是 wrkflw。必须同时守住 §3.3 的四条 Windows 硬指标（W1–W4）。
>
> **③ v1 明确不支持 `${{ }}` 表达式和 `uses:`**
> 守不住这条线，项目必然膨胀成第二个 dagu。这条要写进 README 第一屏，并在代码中对这两类输入**显式报错**。
>
> ### 直接 No-Go 的情形
>
> - 真实痛点只是统一 `.sh` / `.ps1` → **装 `just`**，一下午解决
> - wrkflw 在 Windows 原生下 T1/T2/T4 全部通过 → **用 wrkflw + `schtasks`**
> - 无法同时满足 W1–W4 四条 Windows 硬指标 → 卖点不成立，不做

## 六原则应用记录

| 原则 | 应用位置 |
|---|---|
| **Boil lakes（煮湖）** | 只煮"Windows 原生 + cron"这个湖，明确不碰 action 生态、容器编排、表达式引擎这片海 |
| **Pragmatic（实用主义）** | 选 Rust 因为用户工具链已就绪（而非语言优越性）；`std::thread` 而非 tokio；mermaid 文本输出代替可视化 UI |
| **Explicit over clever（显式优于巧妙）** | 自定义 schema 而非猜测 GH 兼容语义；`shell:` 必须显式指定不做推断；未识别字段报错而非静默忽略；体积做成 CI 硬门禁而非愿望 |
| **Bias toward action（倾向行动）** | 第一周直接跑 wrkflw 实测而不是继续做选型分析；验收标准绑定"跑通 Baafoo 真实流水线"而非抽象指标 |
| **DRY（不重复）** | 识别出 wrkflw 已覆盖 85%，避免重复造轮子；复用 sql-guard 的 Rust 工具链与发布流程 |
| **Choose completeness（选完整性）** | 唯一为未来预留的设计：v0.1 就把执行引擎做成可重入的库（而非写死在 `main()`），以便 v0.2 接入调度器 |

---

*评审完成 · 2026-08-02 · 所有竞品数据经 GitHub API / crates.io API 实测查证，估算值与未查证项已在文中逐一标注*
