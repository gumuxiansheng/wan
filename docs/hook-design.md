# wan hook 子命令设计文档

**日期**：2026-08-03
**状态**：Draft（待评审后实现）
**目标版本**：v0.2.0

---

## 一、设计目标

让 wan 能够响应本地 git 事件（commit / push / merge 等），自动触发 workflow 执行。零常驻进程、零额外依赖、git 原生机制。

## 二、核心方案：Git Hook 自动安装

### 2.1 命令设计

```
wan hook install <hook-type> <workflow> [--no-color] [-C <dir>]
wan hook remove <hook-type> [-C <dir>]
wan hook list [-C <dir>]
```

**参数说明**：

| 参数 | 说明 |
|------|------|
| `<hook-type>` | git hook 类型，见下表 |
| `<workflow>` | workflow 短名或路径（同 `wan run` 的 `<file|name>` 语义） |
| `-C <dir>` | 目标 git 仓库根目录（默认当前目录） |

**支持的 hook 类型**：

| hook-type | 触发时机 | 典型用途 |
|-----------|----------|----------|
| `pre-commit` | `git commit` 执行前 | lint / format 检查，非 0 阻止提交 |
| `pre-push` | `git push` 执行前 | 测试 / 安全扫描，非 0 阻止推送 |
| `post-commit` | `git commit` 执行后 | 通知 / 日志 / 构建触发 |
| `post-merge` | `git merge` 完成后 | 依赖同步 / 重建索引 |
| `post-checkout` | `git checkout` 完成后 | 环境切换 / 依赖重建 |

**不支持**的 hook（与 wan 定位无关）：
- `commit-msg` / `prepare-commit-msg`（消息编辑场景）
- `pre-rebase` / `post-rewrite`（历史重写场景）
- `update` / `proc-receive`（服务端 hook）

### 2.2 安装行为

`wan hook install pre-commit lint` 执行后：

1. **定位 git 目录**：从 `-C` 目录向上查找 `.git/`（`git rev-parse --git-dir` 不可用——不依赖 git CLI，直接文件系统查找）
2. **生成 hook 脚本**：写入 `.git/hooks/pre-commit`，内容见下
3. **权限设置**：Unix 上 `chmod +x`，Windows 上无需设置
4. **幂等**：若文件已存在且包含 `# wan-managed` 标记，覆盖；若存在且非 wan-managed，报错不覆盖（需 `--force`）

### 2.3 生成的 hook 脚本

**Unix 版**（`.git/hooks/pre-commit`）：

```sh
#!/bin/sh
# wan-managed: pre-commit → lint
# 由 `wan hook install` 生成，请勿手动编辑
exec wan run lint --quiet "$@"
```

**Windows 版**（`.git/hooks/pre-commit`，git for Windows 用 bash 执行）：

```sh
#!/bin/sh
# wan-managed: pre-commit → lint
exec wan run lint --quiet "$@"
```

git for Windows 自带 bash，hook 脚本统一用 sh 语法即可，无需区分平台。

**关键设计**：
- `--quiet`：hook 场景默认安静模式，仅失败时输出
- `exec`：wan 进程替换当前 shell，退出码直接传播给 git
- 退出码语义：0 放行 / 1 阻止（git 按非 0 阻止 commit/push）/ 2 配置错误 / 130 中断

### 2.4 wan hook list 输出

```
$ wan hook list
pre-commit   → lint     (.git/hooks/pre-commit)
post-merge   → rebuild  (.git/hooks/post-merge)
```

扫描 `.git/hooks/` 下所有文件，匹配 `# wan-managed:` 标记行，解析 hook-type 和 workflow 名。

### 2.5 wan hook remove

```
$ wan hook remove pre-commit
removed: .git/hooks/pre-commit (was: lint)
```

删除指定 hook 文件。若非 wan-managed 报错不删（需 `--force`）。

## 三、Git 目录查找逻辑

不依赖 `git` CLI（wan 零依赖原则），从 `-C` 指定目录或当前目录开始向上查找：

```rust
fn find_git_dir(start: &Path) -> Option<PathBuf> {
    let mut current = start.canonicalize().ok()?;
    loop {
        let git = current.join(".git");
        if git.is_dir() {
            return Some(git);
        }
        if git.is_file() {
            // worktree 指针文件：.git 内容为 "gitdir: /path/to/.git"
            if let Ok(content) = std::fs::read_to_string(&git) {
                if let Some(line) = content.lines().find(|l| l.starts_with("gitdir:")) {
                    let p = line["gitdir:".len()..].trim();
                    let resolved = if Path::new(p).is_absolute() {
                        PathBuf::from(p)
                    } else {
                        current.join(p)
                    };
                    if resolved.is_dir() {
                        return Some(resolved);
                    }
                }
            }
        }
        current = current.parent()?.to_path_buf();
    }
}
```

找不到 `.git/` → 报错：`不在 git 仓库内（未找到 .git 目录）`

## 四、退出码传播

| 场景 | wan 退出码 | git 行为 |
|------|-----------|----------|
| workflow 全部成功 | 0 | commit/push 继续 |
| workflow 有失败 | 1 | commit/push 被阻止 |
| 配置错误 | 2 | commit/push 被阻止 |
| 用户 Ctrl+C | 130 | commit/push 被阻止 |

对 `pre-commit` / `pre-push`：非 0 阻止操作，符合预期。
对 `post-commit` / `post-merge` / `post-checkout`：退出码不影响 git 操作（git 忽略 post hook 退出码），但用户能看到失败信息。

## 五、与 v0.2 cron 的关系

| 维度 | hook | cron |
|------|------|------|
| 触发方式 | 事件驱动（git 操作） | 时间驱动（定时） |
| 进程模型 | git 进程的子进程 | wan 常驻调度器 |
| 适用场景 | 代码变更相关（lint/test/build） | 定时任务（每日报告/清理/同步） |
| 持久化 | 无（hook 脚本即配置） | 需要（crontab 文件） |

两者互补，不冲突。一个 workflow 可以同时被 hook 和 cron 触发。

## 六、实现计划

### 6.1 新增模块

```
src/
  hook.rs        — hook 安装/删除/列出/查找 git 目录
```

### 6.2 cli.rs 改动

USAGE 新增：
```
wan hook install <hook-type> <workflow> [-C <dir>] [--force]
wan hook remove <hook-type> [-C <dir>] [--force]
wan hook list [-C <dir>]
```

dispatch 新增 `"hook"` 分支，解析子命令 `install` / `remove` / `list`。

### 6.3 hook.rs 接口

```rust
pub enum HookType {
    PreCommit,
    PrePush,
    PostCommit,
    PostMerge,
    PostCheckout,
}

impl HookType {
    pub fn from_str(s: &str) -> Result<Self>;
    pub fn filename(&self) -> &'static str;  // "pre-commit" 等
    pub fn as_str(&self) -> &'static str;
}

pub fn install(git_dir: &Path, hook: HookType, workflow: &str, force: bool) -> Result<()>;
pub fn remove(git_dir: &Path, hook: HookType, force: bool) -> Result<()>;
pub fn list(git_dir: &Path) -> Vec<InstalledHook>;

pub struct InstalledHook {
    pub hook_type: HookType,
    pub workflow: String,
    pub path: PathBuf,
}

pub fn find_git_dir(start: &Path) -> Option<PathBuf>;
```

### 6.4 hook 脚本模板

```sh
#!/bin/sh
# wan-managed: {hook_type} → {workflow}
exec wan run {workflow} --quiet "$@"
```

三行，无多余内容。`$@` 透传 git 传入的参数（如 pre-push 接收 remote name 和 url），workflow 内可通过 `$WAN_HOOK_ARGS` 等环境变量读取（v0.2 扩展，v0.1 先不传参）。

### 6.5 幂等与安全

- 安装前检查目标文件是否已存在
- 已存在且首行包含 `# wan-managed` → 覆盖（打印 `replaced: ...`）
- 已存在且非 wan-managed → 报错 `refusing to overwrite existing hook (use --force)`；`--force` 时先备份为 `{hook}.bak`
- 删除前同样检查 wan-managed 标记
- `--force` 删除非 wan-managed hook 时先备份

### 6.6 测试计划

```
hook install pre-commit lint           → 文件存在 + 内容正确 + 标记行
hook install pre-commit lint (再次)     → 覆盖 + 打印 replaced
hook install pre-commit lint (非 wan)   → 报错 refusing to overwrite
hook install pre-commit lint --force    → 备份原文件 + 覆盖
hook remove pre-commit                  → 文件删除 + 打印 was: lint
hook remove pre-commit (不存在)         → 报错 not found
hook remove pre-commit (非 wan)         → 报错 refusing
hook list                               → 正确列出已安装 hook
find_git_dir 从子目录向上查找           → 找到 .git
find_git_dir worktree 指针文件          → 解析 gitdir 指向
```

## 七、不在本方案范围内

- **非 git 事件**（文件监听、定时触发）→ v0.2 cron
- **hook 参数传递**（pre-push 的 remote/url 等传入 workflow）→ v0.2 扩展
- **hook 模板自定义**（用户自定义脚本前缀/后缀）→ 视需求再加
- **多 workflow 绑定同一 hook** → 当前一对一，一个 hook 一个 workflow；若需多个可用一个 wrapper workflow 调 needs 串联

## 八、体积影响估算

hook.rs 约 150 行纯 Rust，无新依赖。Release 体积预计增加 <5KB。
