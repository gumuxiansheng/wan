# wan 用户手册

> 版本:0.1 · 适用 CLI 自 `wan --help` 输出的用法
> 行为细节以 [spec.md](spec.md)(SSOT)为准,本文是面向日常使用的完整指南。

## 目录

1. [关于 wan](#1-关于-wan)
2. [安装](#2-安装)
3. [第一个 workflow](#3-第一个-workflow)
4. [workflow 文件参考](#4-workflow-文件参考)
5. [if 条件](#5-if-条件)
6. [变量与插值](#6-变量与插值)
7. [step 间传递数据](#7-step-间传递数据)
8. [重试与超时](#8-重试与超时)
9. [调度与并行](#9-调度与并行)
10. [输出与日志](#10-输出与日志)
11. [退出码](#11-退出码)
12. [命令行参考](#12-命令行参考)
13. [Git Hook 集成](#13-git-hook-集成)
14. [与 CI 集成](#14-与-ci-集成)
15. [故障排查](#15-故障排查)
16. [限制](#16-限制)

---

## 1. 关于 wan

`wan` 是本地工作流执行器:一个 YAML 文件描述多个 **job**(任务)及每个 job 内的 **step**(步骤),`wan` 负责:

- 解析 YAML 并**校验**(schema 错误带行列号)
- 按依赖关系(`needs`)构建 DAG 并**调度**(环检测、失败跳过传播)
- 为每个 step 生成**临时脚本**并用系统 shell 执行
- 输出人类可读日志或**结构化 JSON 事件流**

适用场景:

- 本地构建 / 测试 / 部署脚本的编排(替代手写一堆 `.bat`/`.sh`)
- 需要"失败就停、依赖跳过、可重试、超时保护"的日常自动化
- 想在本地复现 CI 流水线的一部分

设计目标:零守护进程、单文件、冷启动快(release 实测 ~60 ms/次含子进程启动)、双平台共用一套 schema。

## 2. 安装

### 2.1 系统要求

| 平台 | 要求 |
|---|---|
| Windows 10/11 | `pwsh`(PowerShell 7,[微软安装说明](https://learn.microsoft.com/powershell/scripting/install/installing-powershell-on-windows))、`cmd`(系统自带)、`bash`(Git Bash) |
| Linux | `bash`、`sh` |

- Windows 内置的 Windows PowerShell 5.1(`powershell.exe`)**不是** `pwsh`,仅 `cmd` 开箱即用。
- 你的 workflow 用到哪个 shell,机器上就必须有哪个;没有的 shell 会在 `run` 时报错并给出安装提示。

### 2.2 构建

```console
$ git clone <repo-url> wan
$ cd wan
$ cargo build --release
```

产物:`target/release/wan`(Windows 为 `wan.exe`,约 0.6 MB)。可把它复制到任意目录并加入 `PATH`:

```console
$ cp target/release/wan /usr/local/bin/wan      # Linux
$ copy target\release\wan.exe %USERPROFILE%\bin # Windows
```

验证:

```console
$ wan --version
$ wan --help
```

## 3. 第一个 workflow

### 3.1 目录约定

`wan list` 默认列出 `.wan/workflows/` 目录下的所有 `.yml`/`.yaml` 文件(目录不存在时改为列出当前目录)。

`run` / `validate` / `graph` 接受两种参数:

- **短名**(如 `demo`)→ 只在 `.wan/workflows/` 下查找,且自动匹配平台:
  1. 当前平台后缀:Windows 找 `{name}-win.yml`,Linux 找 `{name}-unix.yml`
  2. 无平台后缀文件时回退 `{name}.yml` / `{name}.yaml`
  3. 都没有 → 报错并列出全部候选(退出码 2)
- **含路径分隔符**(`/` 或 `\`)→ 视为文件路径,直接执行

```console
$ wan run demo            # Windows → demo-win.yml;Linux → demo-unix.yml
$ wan run shared          # 无平台后缀 → shared.yml
```

参数含路径分隔符(`/` 或 `\`)时视为文件路径直接执行,不受上述查找规则约束。

同一仓库可以同时放置 `deploy-win.yml` 与 `deploy-unix.yml`,在不同平台上用同一短名自动选型。

### 3.2 写文件

在 `.wan/workflows/` 下创建 `hello-win.yml`(Windows)或 `hello-unix.yml`(Linux;只跑单平台时也可直接叫 `hello.yml`):

```yaml
version: 1
jobs:
  hello:
    steps:
      - name: 打招呼
        shell: pwsh
        run: |
          Write-Output "hello from wan, $(whoami)"
          Write-Output "现在时间: $(Get-Date -Format 'HH:mm:ss')"
```

### 3.3 先校验

```console
$ wan validate hello
```

- 校验通过:无输出,退出码 0。
- 校验失败:退出码 2,stderr 给出**文件名:行:列**与原因。

试着把 `shell:` 删掉再校验,会得到"step 必须显式指定 shell"之类的错误。

### 3.4 执行

```console
$ wan run hello
==> 开始运行 workflow: hello
[job] hello
  [step] 打招呼
    Write-Output "hello from wan, win11"
    hello from wan, win11
    现在时间: 16:45:31
    OK (41 ms)
[job] hello OK (42 ms)
==> 结果: 成功 (42 ms)
```

- 每行输出带两个空格前缀,来自 step 的子进程输出
- `OK (41 ms)` 是 step 耗时(含 shell 加载)
- 退出码 0

### 3.5 可视化

```console
$ wan graph hello
```

输出 mermaid `flowchart` 文本,可直接粘贴到支持 mermaid 的 Markdown 或 [mermaid.live](https://mermaid.live) 渲染:

```
flowchart LR
  hello[hello]
```

## 4. workflow 文件参考

文件必须是合法 YAML(UTF-8)。顶层结构:

```yaml
version: 1        # 必填,当前仅支持 1
env: {...}        # 可选,全局环境变量
working-directory: <path>   # 可选,全局工作目录
jobs: {...}       # 必填,至少一个 job
```

### 4.1 顶层字段

| 字段 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `version` | int | 是 | 当前仅支持 `1` |
| `jobs` | map | 是 | job 名 → job 定义;job 名建议 `[a-zA-Z0-9_-]` |
| `env` | map | 否 | 全局环境变量,注入所有 job 的所有 step |
| `working-directory` | string | 否 | 全局工作目录,被 job / step 级覆盖 |

### 4.2 job 字段

```yaml
jobs:
  build:
    needs: []                  # 依赖的 job 名列表,可空
    if: success()              # 可选,job 级条件
    env:                       # 可选,job 级环境变量
      MODE: release
    working-directory: build   # 可选,job 级工作目录
    timeout-minutes: 30        # 可选,整个 job 的超时(分钟)
    steps: [...]               # 必填,≥ 1 个 step
```

| 字段 | 说明 |
|---|---|
| `needs` | 前置 job 列表。全部成功后本 job 才执行;任一失败则本 job 被**跳过**(除非 `if: always()`)。无依赖时视为就绪,可立即执行 |
| `if` | 执行条件,见[第 5 节](#5-if-条件)。`if: always()` 可抵消依赖失败的跳过 |
| `env` | 追加到全局 env(同名覆盖全局) |
| `working-directory` | 覆盖全局工作目录(见 4.4 的继承规则) |
| `timeout-minutes` | job 总时长上限。超时后终止该 job 全部进程(Windows 用 Job Object 杀树,Linux 用进程组),job 记为失败 |
| `steps` | 顺序执行的步骤列表 |

### 4.3 step 字段

```yaml
steps:
  - name: 构建            # 可选,不填则生成默认名
    shell: pwsh           # 必填:pwsh / cmd / bash / sh(无默认值)
    run: |                # 必填:要执行的脚本(多行)
      dotnet build
    if: success()         # 可选
    env:                  # 可选,仅本 step 可见
      KEY: value
    working-directory: src   # 可选
    continue-on-error: false # 可选,默认 false
    timeout-minutes: 10      # 可选,本 step 超时
    retry:                   # 可选
      attempts: 3
      delay: 2s
```

| 字段 | 说明 |
|---|---|
| `name` | 显示名,出现在日志与事件流中 |
| `shell` | 见 4.5。**必须显式指定**,这是与 GitHub Actions 最大的不同 |
| `run` | 脚本内容。多行用 YAML `\|`。脚本写入临时文件(UTF-8)后交给 shell 执行 |
| `if` | 条件,见[第 5 节](#5-if-条件) |
| `env` | 追加到全局 + job 环境(同名覆盖) |
| `working-directory` | step 执行目录,见 4.4 |
| `continue-on-error` | `true` 时本 step 失败不中断 job,继续执行后续 step;job 结果仍记为失败 |
| `timeout-minutes` | 本 step 超时(秒内精确到分钟粒度的保护),超时后终止整个进程树,退出码 124 |
| `retry` | 失败重试,见[第 8 节](#8-重试与超时) |

### 4.4 working-directory 继承

按 `-C` 参数 → 全局 → job → step 的顺序**逐层叠加**(相对路径相对上一层解析,绝对路径直接覆盖):

```yaml
version: 1
working-directory: proj        # 基准:<cwd>/proj
jobs:
  build:
    working-directory: build   # <cwd>/proj/build
    steps:
      - name: 深层
        working-directory: out # <cwd>/proj/build/out
        shell: cmd
        run: echo %CD%
```

没有 `-C` 时基准为运行 `wan` 时的当前目录。

### 4.5 shell 支持矩阵

| shell | Windows | Linux | 说明 |
|---|---|---|---|
| `pwsh` | ✅ PowerShell 7 | ✅ | 需自行安装 PowerShell 7;Windows 5.1 不算 |
| `cmd` | ✅ 系统自带 | ❌ | Windows 原生 |
| `bash` | ✅ Git Bash | ✅ | Windows 上请用 Git Bash;**WSL bash 不支持反斜杠路径** |
| `sh` | ❌ | ✅ | 通常是 bash 的 POSIX 兼容模式 |

- 同一份文件在双平台复用:脚本内容若用到平台特有语法,请分别写 shell,或只声明单平台 shell(另一平台校验阶段会直接报"不支持")。
- shell 探测失败时报错并提示安装方式,退出码 1(执行期)或 2(校验期)。

## 5. if 条件

`if` 可用于 job 与 step。支持表达式(不区分大小写):

| 表达式 | 含义 |
|---|---|
| `success()` | 前置依赖 / 前一步全部成功(默认行为) |
| `failure()` | 有依赖失败(通常配合 `if: always()` 使用) |
| `always()` | 无条件执行(依赖失败也会执行) |
| `==` / `!=` | 字符串比较,如 `if: 'always()' != 'failure()'` |

规则:

- step 失败后,同 job 后续 step 默认跳过(不发射 start 事件),job 结果失败
- job 失败后,`needs` 依赖它的 job 默认跳过
- `if: always()` 可以绕过上述跳过
- `continue-on-error: true` 的 step 失败不触发跳过,但 job 结果仍为失败
- 不支持的语法(如 `&&`、`${{ }}` 内插)会报配置错误(退出码 2)

示例:

```yaml
steps:
  - name: 部署
    shell: pwsh
    run: deploy.ps1
    if: always()          # 即使前面失败也尝试部署
  - name: 上报失败
    shell: pwsh
    run: notify-fail.ps1
    if: failure()         # 只在有失败时执行
```

## 6. 变量与插值

### 6.1 变量来源

每个 step 的环境变量按以下顺序合并(后者覆盖前者):

1. 系统环境变量(`wan` 进程继承的)
2. 全局 `env`
3. job `env`
4. step `env`
5. step 间传递的 `$WAN_OUTPUT` 值(见[第 7 节](#7-step-间传递数据))

### 6.2 插值语法与范围

`${NAME}` 形式。插值**只发生在以下字段**:

- `env` 的值(全局 / job / step)
- `run` 脚本内容
- `working-directory`

```yaml
env:
  REGION: cn-north
jobs:
  build:
    steps:
      - name: 打包
        shell: pwsh
        env:
          TARGET: ${REGION}-prod
        run: |
          Write-Output "目标: $env:TARGET"   # 输出 "目标: cn-north-prod"
```

规则:

- **单遍、不递归**:`env: { A: ${B}, B: x }` 中 `A` 不会继续展开
- 未定义变量**保留原样**并在 stderr 警告,不静默替换(防止拼写错误被掩盖)
- 插值在脚本写入前完成;脚本内部的 `$var` 由 shell 自己解释,互不干扰

## 7. step 间传递数据

### 7.1 输出

step 脚本里把 `key=value` 行写入环境变量 `WAN_OUTPUT` 指向的文件:

```yaml
steps:
  - name: 产生版本号
    shell: pwsh
    run: |
      $v = Get-Date -Format 'yyyy.MM.dd'
      "version=$v" | Out-File -FilePath $env:WAN_OUTPUT -Encoding utf8
```

各 shell 的写法:

| shell | 写法 |
|---|---|
| `pwsh` | `"k=v" \| Out-File $env:WAN_OUTPUT -Encoding utf8` |
| `cmd` | `echo k=v > %WAN_OUTPUT%` |
| `bash`/`sh` | `echo k=v > "$WAN_OUTPUT"` |

### 7.2 消费

**仅当 step 成功结束**时,输出文件才被读回并以 `key=value` 注入**后续 step** 的环境(同名覆盖):

```yaml
steps:
  - name: 消费版本号
    shell: pwsh
    run: |
      Write-Output "构建版本: $env:version"
```

- 失败或超时的 step,其输出被丢弃
- 注入只发生在 step 的启动环境,不会污染其他 job 或系统环境
- 输出文件每行最多一个 `k=v`;无 `=` 的行被忽略

## 8. 重试与超时

### 8.1 retry

```yaml
retry:
  attempts: 3     # ≥ 1,默认 1(即不重试)
  delay: 2s       # 单单位时长,支持 s / m / h 后缀,如 5s / 2m / 1h
```

- 仅在 step **失败**(非零退出码或超时)时重试;`continue-on-error` 的失败不重试
- 每次重试前等待 `delay`;每次重试都是全新的临时脚本执行
- 重试警告输出到 stderr;超过 `attempts` 后仍失败,按普通失败处理
- 配置错误:`attempts < 1` 或非法 `delay` → 校验失败(退出码 2)

### 8.2 timeout-minutes

| 级别 | 字段 | 行为 |
|---|---|---|
| step | `timeout-minutes` | 超时终止整个进程树,step 退出码记 124,job 失败 |
| job | `timeout-minutes` | job 总时长(含所有 step)超时,终止 job 全部进程,job 失败 |

终止机制:Windows 使用 Job Object(一次关闭整棵进程树);Linux 先 `SIGTERM` 进程组,宽限后 `SIGKILL`。

## 9. 调度与并行

- 依赖图:job 之间用 `needs`;没有 `needs`(或 `needs: []`)的 job 之间**可并行**(默认不设上限)
- `--max-parallel N` 限制全局同时运行的 job 数(信号量语义)
- 调度算法:就绪队列(work-list)+ 全局信号量;`validate` 时做环检测,发现环则报错并**打印环路径**,退出码 2
- 跳过传播:job 失败 → 所有(直接或间接)依赖它的 job 被跳过;跳过不发射 start 事件

```console
$ wan run wf --max-parallel 2    # 最多 2 个 job 同时跑
```

## 10. 输出与日志

### 10.1 人类可读(默认)

- step 脚本原文回显(缩进 4 空格)与子进程输出(缩进 2 空格)
- step 结果 `OK (41 ms)` / `FAIL (code 1, 274 ms)`;job 同理
- 摘要 `==> 结果: 成功 / 失败`
- `--no-color` 关闭 ANSI 颜色;`--quiet` 抑制 step 输出(只留 job/step 状态行与摘要)

### 10.2 JSON 事件流(`--json`)

每行一个事件(`step_output` 按行发射),类型:

| 类型 | 关键字段 |
|---|---|
| `run_start` | `workflow`(文件名主名)、`ts`(RFC3339) |
| `job_start` | `job` |
| `step_start` | `job`、`step` |
| `step_output` | `job`、`step`、`stream`(stdout/stderr)、`line` |
| `step_end` | `job`、`step`、`code`、`duration_ms` |
| `job_end` | `job`、`code`、`duration_ms` |
| `run_end` | `code`、`duration_ms`、`ts` |

事件顺序保证与真实发生顺序一致(单通道 mpsc)。适合:接入 CI 展示、写入日志文件、自定义渲染器。

```console
$ wan run wf --json > run.jsonl
$ wan run wf --json | tee run.jsonl
```

### 10.3 临时文件

每个 step 的脚本写在系统临时目录 `wan-<pid>-<nanos>/` 下,运行结束自动清理。脚本内容即 `run` 文本(shell 差异见 4.5)。

## 11. 退出码

| 码 | 含义 |
|---|---|
| `0` | 全部成功 |
| `1` | 有 job/step 执行失败(含 continue-on-error 累计失败) |
| `2` | 配置错误:schema 非法、DAG 成环、shell 不支持等(`validate` 与 `run` 前置校验共用) |
| `124` | step 超时(出现在 step 的事件里,不是进程退出码) |
| `130` | 用户中断(Ctrl-C) |

退出码取**第一个失败的最高优先级结果**(失败优先于中断)。脚本被 Ctrl-C 时:job 被标记中断、依赖它的 job 全部跳过、`run_end` 后进程退出 130。

## 12. 命令行参考

```
wan run <name> [--json] [--max-parallel N] [--quiet] [--no-color] [-C <dir>]
wan validate <name> [-C <dir>]
wan list [-C <dir>]
wan graph <name> [-C <dir>]
wan hook install <hook-type> <workflow> [--force] [-C <dir>]
wan hook remove <hook-type> [--force] [-C <dir>]
wan hook list [-C <dir>]
wan schedule add <id> <cron-expr> <workflow> [-C <dir>]
wan schedule remove <id> [-C <dir>]
wan schedule list [-C <dir>]
wan schedule start [-C <dir>] [--catch-up] [--json] [--quiet] [--no-color]
wan schedule run-once [-C <dir>] [--json] [--quiet] [--no-color]
wan schedule service install|remove|status [-C <dir>]
wan schedule history [<id>] [-C <dir>] [--limit N]
wan --version
wan --help
```

| 命令 | 说明 |
|---|---|
| `run <name>` | 校验 + 执行。`<name>` 含路径分隔符时按文件路径;否则按短名在 `.wan/workflows/` 下做平台后缀解析(见 3.1) |
| `validate <name>` | 只做 schema + DAG 校验(含环检测、平台 shell 检查),不执行。短名解析规则同 `run` |
| `list` | 列出 `.wan/workflows/` 下的 workflow;目录不存在时列当前目录的 `.yml/.yaml` |
| `graph <name>` | 输出 mermaid `flowchart` 文本。短名解析规则同 `run` |
| `hook install` | 安装 git hook,见[第 13 节](#13-git-hook-集成) |
| `hook remove` | 删除 git hook |
| `hook list` | 列出已安装的 wan-managed hook |
| `schedule add` | 添加 cron 调度条目,见[第 14 节](#14-cron-调度) |
| `schedule remove` | 移除调度条目 |
| `schedule list` | 列出所有调度及下次触发时间 |
| `schedule start` | 启动常驻调度守护进程 |
| `schedule run-once` | 单次扫描执行(供系统服务调用) |
| `schedule service` | 安装/移除/查看系统服务(Windows schtasks / Linux systemd) |
| `schedule history` | 查看执行历史 |
| `--version` / `--help` | 版本与帮助 |

| 参数 | 作用 |
|---|---|
| `--json` | JSON 事件流(与 `--quiet` 同用无效) |
| `--max-parallel N` | job 全局并行上限(默认无上限) |
| `--quiet` | 抑制 step 子进程输出 |
| `--no-color` | 禁用 ANSI 颜色 |
| `-C <dir>` | step 工作目录基准(working-directory 继承链的根),并作为相对路径解析基准 |

## 13. Git Hook 集成

`wan hook` 子命令将 workflow 绑定到 git 事件,实现 commit / push 等操作时自动触发流水线。

### 13.1 命令

```
wan hook install <hook-type> <workflow> [--force] [-C <dir>]
wan hook remove <hook-type> [--force] [-C <dir>]
wan hook list [-C <dir>]
```

### 13.2 支持的 hook 类型

| hook 类型 | 触发时机 | 典型用途 |
|---|---|---|
| `pre-commit` | `git commit` 之前 | 代码风格检查、静态分析 |
| `pre-push` | `git push` 之前 | 运行测试、阻止推送未通过的代码 |
| `post-commit` | `git commit` 之后 | 通知、生成 changelog |
| `post-merge` | `git merge` 之后 | 依赖更新、重建索引 |
| `post-checkout` | `git checkout` 之后 | 切换分支后自动配置环境 |

### 13.3 安装

```console
$ wan hook install pre-commit lint
installed: pre-commit -> lint
```

生成的 hook 脚本(位于 `.git/hooks/pre-commit`):

```sh
#!/bin/sh
# wan-managed: pre-commit -> lint
exec wan run lint --quiet "$@"
```

- wan 在 `.git/hooks/` 目录下生成三行脚本,直接调用 `wan run <workflow> --quiet`
- **退出码直接传播**:wan 失败 → hook 返回非零 → git 阻止操作
- `-C <dir>` 指定工作目录(默认当前目录),wan 从该目录向上查找 `.git`

### 13.4 幂等与安全

- **wan-managed hook**(含 `# wan-managed:` 标记行):`install` 直接覆盖,`remove` 直接删除
- **非 wan hook**(已存在但不带标记):
  - `install` 拒绝覆盖,报错退出(退出码 2)
  - 加 `--force` 强制覆盖,原文件自动备份为 `.bak`
  - `remove` 同理拒绝删除,需 `--force`

```console
$ wan hook install pre-commit lint
error: pre-commit 已存在且非 wan 管理,拒绝覆盖(使用 --force 强制,原文件备份为 .bak)

$ wan hook install pre-commit lint --force
installed: pre-commit -> lint (backed up to .git/hooks/pre-commit.bak)
```

### 13.5 列出已安装 hook

```console
$ wan hook list
pre-commit     -> lint
post-merge     -> rebuild
```

仅列出 wan 管理的 hook(非 wan 的 hook 不显示)。无 hook 时输出 `(无 wan-managed hook)`。

### 13.6 worktree 支持

`find_git_dir` 支持 git worktree:当 `.git` 是文件(而非目录)时,读取其中的 `gitdir:` 指针并解析到真实 hooks 目录。在 worktree 中安装的 hook 对主仓库生效。

### 13.7 与 cron 的关系

- **hook**:事件驱动(commit / push 等git 操作触发)
- **cron**(v0.2 计划):时间驱动(定时执行)
- 两者互补,可组合使用

---

## 14. Cron 调度

wan v0.2 起支持 cron 定时调度。调度条目存储在 `.wan/schedules/schedules.json`,执行历史存储在 `.wan/schedules/history.jsonl`。

### 14.1 添加调度

```bash
wan schedule add <id> <cron-expr> <workflow> [-C <dir>]
```

- `<id>`:调度条目标识符(唯一)
- `<cron-expr>`:标准 5 字段 cron 表达式(分 时 日 月 周)
- `<workflow>`:workflow 文件路径或短名

示例:

```bash
wan schedule add daily "0 2 * * *" deploy
wan schedule add hourly "0 * * * *" health-check
```

cron 表达式支持:`*` / `N` / `N-M` / `N,M,L` / `*/N` / `N-M/S`

### 14.2 列出调度

```bash
wan schedule list [-C <dir>]
```

显示每个调度的 ID、cron 表达式、workflow 路径和下次触发时间。

### 14.3 移除调度

```bash
wan schedule remove <id> [-C <dir>]
```

### 14.4 启动常驻守护进程

```bash
wan schedule start [-C <dir>] [--catch-up] [--json] [--quiet] [--no-color]
```

常驻进程会到点自动触发 workflow 执行。按 Ctrl+C 停止。

- `--catch-up`:错过窗口时补跑最近一次(默认跳过)

### 14.5 单次扫描执行

```bash
wan schedule run-once [-C <dir>] [--json] [--quiet] [--no-color]
```

检查所有调度,执行当前分钟到点的 workflow。供系统服务每分钟调用。

### 14.6 系统服务集成

```bash
wan schedule service install [-C <dir>]
wan schedule service remove [-C <dir>]
wan schedule service status [-C <dir>]
```

**Windows**:注册 schtasks 定时任务(每分钟触发 `wan schedule run-once`)。

**Linux**:安装 systemd user unit(`wan-schedule-<hash>.service` + `.timer`,每分钟触发)。

任务名/unit 名含项目路径哈希(如 `WanSchedule-a1b2c3d4`),同一台机器上多个项目可分别安装、互不覆盖。`remove`/`status` 需在对应项目目录(或用 `-C` 指定)执行,以定位到本项目的任务。旧版固定名任务(`WanSchedule` / `wan-schedule`)会在 install/remove 时自动清理。

### 14.7 查看执行历史

```bash
wan schedule history [<id>] [-C <dir>] [--limit N]
```

显示最近 N 条执行记录(默认 20)。可选按调度 ID 过滤。

### 14.8 存储路径

| 文件 | 说明 |
|---|---|
| `.wan/schedules/schedules.json` | 调度条目列表 |
| `.wan/schedules/history.jsonl` | 执行历史(每行一条 JSON) |
| `.wan/schedules/run-once.bat` | Windows wrapper 脚本(service install 生成) |

## 15. 与 CI 集成

`wan` 输出结构化事件与确定退出码,可嵌入任何 CI:

GitHub Actions:

```yaml
- name: 本地流水线
  shell: bash
  run: |
    wan run release --json > wan-events.jsonl
```

GitLab CI:

```yaml
pipeline:
  script:
    - wan run release --json | tee wan-events.jsonl
```

在流水线内嵌套 wan 时注意:

- 把 `wan` 放到 CI runner 的 `PATH`
- 仓库带上 `.wan/workflows/`,用短名即可按平台自动选型
- 使用 `wan run --json` 以便上游 CI 解析事件
- 需要 runner 上具备 workflow 用到的 shell(如 Git Bash)

## 16. 故障排查

| 现象 | 原因与处理 |
|---|---|
| 退出码 2,stderr 有 `文件:行:列` | schema / 语法错误。按提示修正;`validate` 可快速迭代 |
| `环检测: ...` 且列出 job 链 | `needs` 存在环。`wan graph <name>` 看结构,删除环上一条依赖 |
| `shell 未指定` | 每个 step 必须显式写 `shell:` |
| Windows 上 `pwsh` 报错 | 未安装 PowerShell 7。安装后重试;或改用 `shell: cmd` |
| `bash: ... No such file or directory` 且路径变成 `C:Users...` | 用的是 WSL bash(不认反斜杠)。改用 Git Bash |
| 变量显示为 `\${VAR}` 原样 + 警告 | 变量未定义或拼写错误。检查 `env` 是否注入该 step |
| step 一直不结束 | 有 `timeout-minutes` 会杀树;没有则等它自然结束(Ctrl-C 可中断并退出 130) |
| 中文输出乱码 | 脚本文件为 UTF-8;cmd 会话会自动设置代码页;确保终端支持 UTF-8 |
| `list` 显示为空 | 检查是否存在 `.wan/workflows/`;也可直接用 `run <路径>` |
| `wan run xx` 报"未找到 workflow"(退出码 2) | 短名只查 `.wan/workflows/`。stderr 会列出已尝试的全部候选(如 `xx-win.yml`、`xx.yml`);确认文件名或改用完整路径 |
| `wan hook install` 报"不在 git 仓库内" | 从当前目录向上未找到 `.git`。`-C <dir>` 指定仓库根目录,或 `cd` 到仓库内 |
| `wan hook install` 报“已存在且非 wan 管理” | hook 文件已存在但不带 wan 标记。确认后加 `--force` 覆盖（原文件备份为 `.bak`） |
| `wan schedule service install` 报 schtasks 失败 | 确认有管理员权限；检查 `schtasks /Create /?` 帮助；可能是任务名冲突 |
| `wan schedule start` 退出后历史为空 | 检查 cron 表达式是否正确（`wan schedule list` 看下次触发时间）；确认 workflow 文件可加载 |
| `wan schedule run-once` 无输出 | 当前时间不匹配任何调度；检查 cron 表达式和系统时区 |

## 17. 限制

- `if` 仅支持 `success()` / `failure()` / `always()` / `==` / `!=`,不支持 `&&`、`!`、函数链
- 变量插值单遍、不递归;未定义变量保留原样并警告
- 无 secrets / 凭据存储,`env` 明文写入子进程环境
- 与 GitHub Actions 语法不互认;工作流目录为 `.wan/workflows/`
- `bash` 在 Windows 上需要 Git Bash;WSL bash 不受支持(路径格式限制)
- `cron` 定时调度为 v0.2 计划;v0.1 的 `hook` 子命令仅覆盖 git 事件触发
- v0.2 支持 cron 调度、执行历史持久化、Windows schtasks / Linux systemd 服务集成

---

*文档版本 0.2 · 行为细节以 `docs/spec.md` 为准。*
