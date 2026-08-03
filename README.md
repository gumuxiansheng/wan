# wan — 本地工作流执行器

`wan` 是一个在你自己机器上运行的可执行工作流引擎：YAML 描述依赖关系与执行步骤，`wan` 负责解析、校验、调度，并用系统自带 shell（PowerShell 7 / cmd / bash / sh）逐条执行。无需安装 Docker、无需联网、无需守护进程。

```console
$ wan run demo
==> 开始运行 workflow: demo
[job] build
  [step] 编译
    ...
    OK (41 ms)
[job] build OK (42 ms)
==> 结果: 成功 (42 ms)
```

`demo` 是短名：`wan` 只在 `.wan/workflows/` 目录下查找 workflow，并按当前平台自动选型（Windows 用 `demo-win.yml`，Linux 用 `demo-unix.yml`，没有平台文件就用 `demo.yml`）。

## 特性

- **零依赖运行时**：单文件二进制（release 约 0.6 MB），冷启动 ~60 ms（含子进程启动）
- **YAML 原生**：行列号报错，`validate` 命令不执行即可校验 schema 与 DAG
- **DAG 调度**：`needs` 声明依赖，就绪队列 + 全局 `max-parallel` 信号量，自动环检测
- **双平台**：Windows（PowerShell 7 / cmd / Git Bash）与 Linux（bash / sh）共用一套 schema
- **step 级数据传递**：`$WAN_OUTPUT` 输出、后续 step 自动注入环境变量
- **健壮性**：`retry` 重试、`timeout-minutes` 超时（Job Object / 进程组杀树）、失败跳过传播
- **结构化事件流**：`--json` 每行一个事件，可直接接入 CI 或自定义渲染器
- **GitHub Actions 手感**：`if: success()/failure()/always()`、`continue-on-error`、`working-directory` 三层继承

## 快速开始

### 1. 构建

```console
$ git clone <repo-url> wan
$ cd wan
$ cargo build --release          # 产物：target/release/wan(.exe)
```

要求 Rust 1.85+（stable）。

### 2. 写第一个 workflow

在 `.wan/workflows/` 目录下创建文件。Windows 机器命名为 `hello-win.yml`，Linux 机器命名为 `hello-unix.yml`（只跑单平台时也直接叫 `hello.yml`）：

```yaml
version: 1
jobs:
  hello:
    steps:
      - name: 打招呼
        shell: pwsh        # Windows 下也可用 cmd；Linux 用 bash / sh
        run: |
          Write-Output "hello from wan, $(whoami)"
```

### 3. 运行

```console
$ wan run hello             # 自动选中 hello-win.yml 或 hello-unix.yml
$ wan run hello --json      # 结构化事件流
$ wan validate hello        # 只校验，不执行
```

## 命令速查

| 命令 | 作用 |
|---|---|
| `wan run <name>` | 执行 workflow（退出码 0/1/2/130） |
| `wan validate <name>` | 校验 schema 与 DAG（环检测），不执行 |
| `wan list` | 列出 `.wan/workflows/` 下的 workflow |
| `wan graph <name>` | 输出 mermaid 流程图文本 |
| `wan --version` / `--help` | 版本 / 帮助 |

全局参数：`--json`、`--max-parallel N`、`--quiet`、`--no-color`、`-C <dir>`（step 工作目录）。

**短名查找**：`<name>` 只查 `.wan/workflows/`——Windows 优先匹配 `{name}-win.yml`，Linux 优先匹配 `{name}-unix.yml`，无平台后缀文件时回退 `{name}.yml/.yaml`，均无则报错（退出码 2）。所以 `wan run demo` 会自动选用 `demo-win.yml`（Windows）或 `demo-unix.yml`（Linux），同一仓库可放双平台版本。参数含路径分隔符（`/`、`\`）时按文件路径直接执行。

```console
$ wan run demo            # Windows → demo-win.yml；Linux → demo-unix.yml
$ wan run shared          # 无平台后缀 → shared.yml
```

## 语法一览

```yaml
version: 1
env:                     # 全局环境变量（可选）
  REGION: cn-north
working-directory: src   # 顶层工作目录（可选）

jobs:
  build:
    needs: []            # 依赖列表（可选，默认空）
    env:
      MODE: release
    working-directory: build
    steps:
      - name: 编译
        shell: pwsh
        run: |
          dotnet build -c $env:MODE
      - name: 测试
        shell: pwsh
        run: |
          dotnet test
        if: success()          # 也支持 failure() / always() / 表达式
        continue-on-error: true
      - name: 上传产物
        shell: pwsh
        run: |
          $WAN_OUTPUT = "url=..."     # 数据传递：写入文件
        retry:
          attempts: 3
          delay: 2s
  deploy:
    needs: [build]       # 等 build 完成后执行
    if: success()
    steps:
      - name: 部署
        shell: bash
        run: |
          deploy.sh
```

### 规则速记

- `shell` **必须显式指定**，无默认值；Windows 支持 `pwsh` / `cmd` / `bash`，Linux 支持 `bash` / `sh`
- 变量插值 `${VAR}`：单遍、不递归；未定义保留原样并警告；仅作用于 `env` 值、`run`、`working-directory`
- step 失败 → 同 job 后续 step 跳过；job 失败 → 依赖它的 job 跳过；`if: always()` 不被跳过
- `$WAN_OUTPUT`：写入 `key=value` 行，step 成功结束即注入后续 step 环境
- `retry.attempts` ≥ 1（默认 1），`retry.delay` 支持 `s` / `m` / `h` 单位
- `timeout-minutes`：step 与 job 级别；超时终止整个进程树（Windows Job Object / Unix 进程组），step 超时码 124
- 退出码：`0` 成功 / `1` 执行失败 / `2` 配置错误 / `130` 中断

完整语法与行为以 [docs/spec.md](docs/spec.md) 为准，日常使用见 [docs/USER_GUIDE.md](docs/USER_GUIDE.md)。

## JSON 事件流

`--json` 输出每行一个事件，供消费方流式解析：

```json
{"type":"run_start","workflow":"hello","ts":"2026-08-02T16:45:31.4598588Z"}
{"type":"job_start","job":"hello"}
{"type":"step_start","job":"hello","step":"打招呼"}
{"type":"step_output","job":"hello","step":"打招呼","stream":"stdout","line":"hello from wan"}
{"type":"step_end","job":"hello","step":"打招呼","code":0,"duration_ms":41}
{"type":"job_end","job":"hello","code":0,"duration_ms":42}
{"type":"run_end","code":0,"duration_ms":42,"ts":"2026-08-02T16:45:31.5019421Z"}
```

事件类型：`run_start` / `job_start` / `step_start` / `step_output` / `step_end` / `job_end` / `run_end`。

## 平台支持

| | Windows | Linux |
|---|---|---|
| 构建 | MSVC / GNU | GNU / musl 交叉编译可用 |
| shell | `pwsh`（PowerShell 7）、`cmd`、`bash`（Git Bash） | `bash`、`sh` |
| 超时杀树 | Job Object | 进程组 `SIGTERM` → `SIGKILL` |
| Ctrl-C | 事件驱动终止 → 130 | 同上 |

> 注意：Windows 上通过 WSL 提供的 `bash.exe` 不支持 `C:\...` 反斜杠路径，请使用 Git Bash 或原生 Linux。

## 开发

```console
$ cargo test              # 45 个测试：34 单元 + 11 集成（含 CLI 端到端）
$ cargo build --release   # 产物 target/release/wan.exe（约 0.6 MB）
$ cargo build --release --profile release-fast   # 快速调试用（未做极限优化）
```

### 源码布局

```
src/
  cli.rs        命令与参数解析（lexopt）
  parser.rs     YAML → Workflow 模型（saphyr-parser，行列号报错）
  scheduler.rs  Kahn 拓扑调度 + 环检测 + 并行信号量
  executor.rs   单 step 执行：临时脚本、管道泵送、超时/重试/数据传递
  engine.rs     校验入口 + 事件收集线程 + 临时目录管理
  output.rs     人类可读 / JSON 两种渲染器
  shell.rs      shell 探测与脚本生成（pwsh/cmd/bash/sh）
  platform/     Windows Job Object 与 Unix 信号处理
tests/          CLI 端到端集成测试与 fixtures
docs/spec.md    行为规范（SSOT，含验收标准）
```

## 已知限制（v0.1）

- `if` 支持子集：`success()` / `failure()` / `always()` / `==` / `!=`（链式 `&&` 不支持）
- 插值单遍、不递归；未定义变量保留原样并警告，不做静默替换
- 无 secrets 管理、无凭据存储；`env` 为明文
- 不与 GitHub Actions `.github/workflows/` 互认，工作流目录为 `.wan/workflows/`
- Windows 需要 PowerShell 7（`pwsh`）而非内置 Windows PowerShell 5.1

## 文档

- [用户手册](docs/USER_GUIDE.md) — 日常使用全指南
- [行为规范](docs/spec.md) — 设计决策与验收标准（SSOT）
