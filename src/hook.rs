//! Git Hook 管理（spec §16）：安装 / 删除 / 列出 git hook 脚本
//! 零依赖：不调用 git CLI，直接文件系统操作

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// wan 管理的 hook 脚本标记行前缀
const MARKER: &str = "# wan-managed:";

/// 支持的 git hook 类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookType {
    PreCommit,
    PrePush,
    PostCommit,
    PostMerge,
    PostCheckout,
}

impl HookType {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "pre-commit" => Ok(HookType::PreCommit),
            "pre-push" => Ok(HookType::PrePush),
            "post-commit" => Ok(HookType::PostCommit),
            "post-merge" => Ok(HookType::PostMerge),
            "post-checkout" => Ok(HookType::PostCheckout),
            other => Err(Error::config(format!(
                "不支持的 hook 类型 `{other}`\n支持的类型：pre-commit / pre-push / post-commit / post-merge / post-checkout"
            ))),
        }
    }

    pub fn filename(&self) -> &'static str {
        match self {
            HookType::PreCommit => "pre-commit",
            HookType::PrePush => "pre-push",
            HookType::PostCommit => "post-commit",
            HookType::PostMerge => "post-merge",
            HookType::PostCheckout => "post-checkout",
        }
    }

    pub fn as_str(&self) -> &'static str {
        self.filename()
    }
}

/// 已安装的 wan hook 信息
#[derive(Debug, Clone)]
pub struct InstalledHook {
    pub hook_type: HookType,
    pub workflow: String,
    pub path: PathBuf,
}

/// 从指定路径向上查找 .git 目录（不依赖 git CLI）
/// 支持 worktree 指针文件（`.git` 文件内容为 `gitdir: /path`）
pub fn find_git_dir(start: &Path) -> Option<PathBuf> {
    let current = start.canonicalize().ok()?;
    find_git_dir_inner(&current)
}

fn find_git_dir_inner(current: &Path) -> Option<PathBuf> {
    let git = current.join(".git");
    if git.is_dir() {
        return Some(git);
    }
    // worktree：.git 是文件，内容 "gitdir: /path/to/.git"
    if git.is_file() {
        if let Ok(content) = std::fs::read_to_string(&git) {
            if let Some(line) = content.lines().find(|l| l.trim().starts_with("gitdir:")) {
                let p = line.trim()["gitdir:".len()..].trim();
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
    current.parent().and_then(find_git_dir_inner)
}

/// 生成 hook 脚本内容
fn hook_script_content(hook: HookType, workflow: &str) -> String {
    format!(
        "#!/bin/sh\n{marker} {hook} -> {workflow}\nexec wan run {workflow} --quiet \"$@\"\n",
        marker = MARKER,
        hook = hook.as_str(),
        workflow = workflow,
    )
}

/// 从已有 hook 文件内容中解析 wan-managed 标记行
/// 返回 Some(workflow) 如果是 wan 管理的 hook
fn parse_marker(content: &str) -> Option<(String, String)> {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(MARKER) {
            // 格式：# wan-managed: pre-commit -> lint
            let rest = rest.trim();
            if let Some((hook_str, wf)) = rest.split_once("->") {
                return Some((hook_str.trim().to_string(), wf.trim().to_string()));
            }
        }
    }
    None
}

/// 安装 hook
pub fn install(git_dir: &Path, hook: HookType, workflow: &str, force: bool) -> Result<()> {
    let hooks_dir = git_dir.join("hooks");
    std::fs::create_dir_all(&hooks_dir)?;

    let hook_path = hooks_dir.join(hook.filename());

    // 检查已存在文件
    if hook_path.exists() {
        let existing = std::fs::read_to_string(&hook_path)
            .map_err(|e| Error::io(format!("读取已有 hook 失败：{e}")))?;

        if parse_marker(&existing).is_some() {
            // wan-managed → 覆盖
            std::fs::write(&hook_path, hook_script_content(hook, workflow))?;
            set_executable(&hook_path);
            println!("replaced: {} -> {}", hook.as_str(), workflow);
            return Ok(());
        }

        // 非 wan-managed
        if !force {
            return Err(Error::config(format!(
                "{} 已存在且非 wan 管理，拒绝覆盖（使用 --force 强制，原文件备份为 .bak）",
                hook_path.display()
            )));
        }

        // force：备份
        let bak = hook_path.with_extension("bak");
        std::fs::copy(&hook_path, &bak)?;
        std::fs::write(&hook_path, hook_script_content(hook, workflow))?;
        set_executable(&hook_path);
        println!(
            "installed: {} -> {} (backed up to {})",
            hook.as_str(),
            workflow,
            bak.display()
        );
        return Ok(());
    }

    // 不存在 → 直接写
    std::fs::write(&hook_path, hook_script_content(hook, workflow))?;
    set_executable(&hook_path);
    println!("installed: {} -> {}", hook.as_str(), workflow);
    Ok(())
}

/// 删除 hook
pub fn remove(git_dir: &Path, hook: HookType, force: bool) -> Result<()> {
    let hook_path = git_dir.join("hooks").join(hook.filename());

    if !hook_path.exists() {
        return Err(Error::config(format!("{} 不存在", hook_path.display())));
    }

    let existing = std::fs::read_to_string(&hook_path)
        .map_err(|e| Error::io(format!("读取 hook 失败：{e}")))?;

    if let Some((hook_str, wf)) = parse_marker(&existing) {
        std::fs::remove_file(&hook_path)?;
        println!("removed: {} (was: {})", hook_str, wf);
        return Ok(());
    }

    // 非 wan-managed
    if !force {
        return Err(Error::config(format!(
            "{} 非 wan 管理，拒绝删除（使用 --force 强制）",
            hook_path.display()
        )));
    }

    let bak = hook_path.with_extension("bak");
    std::fs::copy(&hook_path, &bak)?;
    std::fs::remove_file(&hook_path)?;
    println!(
        "removed: {} (backed up to {})",
        hook_path.display(),
        bak.display()
    );
    Ok(())
}

/// 列出所有 wan-managed hook
pub fn list(git_dir: &Path) -> Result<Vec<InstalledHook>> {
    let hooks_dir = git_dir.join("hooks");
    if !hooks_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut found = Vec::new();
    let entries = std::fs::read_dir(&hooks_dir)
        .map_err(|e| Error::io(format!("读取 hooks 目录失败：{e}")))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let filename = match path.file_name().and_then(|n| n.to_str()) {
            Some(s) => s,
            None => continue,
        };

        // 跳过 .bak 等后缀
        if filename.ends_with(".bak") || filename.ends_with(".sample") {
            continue;
        }

        // 文件名必须是已知 hook 类型
        let hook_type = match HookType::from_str(filename) {
            Ok(h) => h,
            Err(_) => continue,
        };

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        if let Some((_, workflow)) = parse_marker(&content) {
            found.push(InstalledHook {
                hook_type,
                workflow,
                path,
            });
        }
    }

    // 按文件名排序保证输出稳定
    found.sort_by_key(|h| h.hook_type.filename());
    Ok(found)
}

/// Unix 上设置可执行权限；Windows 上 no-op（git for Windows 不检查权限位）
#[cfg(unix)]
fn set_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perm = meta.permissions();
        perm.set_mode(0o755);
        let _ = std::fs::set_permissions(path, perm);
    }
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_tmp_git() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wan-hook-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let git = dir.join(".git");
        fs::create_dir_all(git.join("hooks")).unwrap();
        git
    }

    #[test]
    fn hook_type_parse() {
        assert_eq!(
            HookType::from_str("pre-commit").unwrap(),
            HookType::PreCommit
        );
        assert_eq!(HookType::from_str("pre-push").unwrap(), HookType::PrePush);
        assert_eq!(
            HookType::from_str("post-commit").unwrap(),
            HookType::PostCommit
        );
        assert_eq!(
            HookType::from_str("post-merge").unwrap(),
            HookType::PostMerge
        );
        assert_eq!(
            HookType::from_str("post-checkout").unwrap(),
            HookType::PostCheckout
        );
        assert!(HookType::from_str("commit-msg").is_err());
        assert!(HookType::from_str("pre-rebase").is_err());
    }

    #[test]
    fn install_creates_file() {
        let git = make_tmp_git();
        install(&git, HookType::PreCommit, "lint", false).unwrap();

        let hook_path = git.join("hooks").join("pre-commit");
        let content = fs::read_to_string(&hook_path).unwrap();
        assert!(content.contains("# wan-managed: pre-commit -> lint"));
        assert!(content.contains("exec wan run lint --quiet"));
    }

    #[test]
    fn install_overwrites_wan_managed() {
        let git = make_tmp_git();
        install(&git, HookType::PreCommit, "lint", false).unwrap();
        install(&git, HookType::PreCommit, "test", false).unwrap();

        let hook_path = git.join("hooks").join("pre-commit");
        let content = fs::read_to_string(&hook_path).unwrap();
        assert!(content.contains("# wan-managed: pre-commit -> test"));
        assert!(content.contains("exec wan run test --quiet"));
    }

    #[test]
    fn install_refuses_non_wan_hook() {
        let git = make_tmp_git();
        let hook_path = git.join("hooks").join("pre-commit");
        fs::write(&hook_path, "#!/bin/sh\necho custom\n").unwrap();

        let err = install(&git, HookType::PreCommit, "lint", false).unwrap_err();
        assert!(err.msg.contains("拒绝覆盖"), "{err}");
    }

    #[test]
    fn install_force_backs_up() {
        let git = make_tmp_git();
        let hook_path = git.join("hooks").join("pre-commit");
        fs::write(&hook_path, "#!/bin/sh\necho custom\n").unwrap();

        install(&git, HookType::PreCommit, "lint", true).unwrap();

        let bak = hook_path.with_extension("bak");
        assert!(bak.exists(), "backup file should exist");
        let bak_content = fs::read_to_string(&bak).unwrap();
        assert!(bak_content.contains("custom"));

        let content = fs::read_to_string(&hook_path).unwrap();
        assert!(content.contains("# wan-managed:"));
    }

    #[test]
    fn remove_wan_hook() {
        let git = make_tmp_git();
        install(&git, HookType::PostCommit, "build", false).unwrap();

        let hook_path = git.join("hooks").join("post-commit");
        assert!(hook_path.exists());

        remove(&git, HookType::PostCommit, false).unwrap();
        assert!(!hook_path.exists());
    }

    #[test]
    fn remove_refuses_non_wan() {
        let git = make_tmp_git();
        let hook_path = git.join("hooks").join("pre-push");
        fs::write(&hook_path, "#!/bin/sh\necho custom\n").unwrap();

        let err = remove(&git, HookType::PrePush, false).unwrap_err();
        assert!(err.msg.contains("拒绝删除"), "{err}");
    }

    #[test]
    fn remove_nonexistent() {
        let git = make_tmp_git();
        let err = remove(&git, HookType::PreCommit, false).unwrap_err();
        assert!(err.msg.contains("不存在"), "{err}");
    }

    #[test]
    fn list_finds_wan_hooks() {
        let git = make_tmp_git();
        install(&git, HookType::PreCommit, "lint", false).unwrap();
        install(&git, HookType::PostMerge, "rebuild", false).unwrap();

        // 非 wan hook 不列出
        let hook_path = git.join("hooks").join("pre-push");
        fs::write(&hook_path, "#!/bin/sh\necho custom\n").unwrap();

        let hooks = list(&git).unwrap();
        assert_eq!(hooks.len(), 2);
        // 按文件名字母序：post-merge < pre-commit
        assert_eq!(hooks[0].hook_type, HookType::PostMerge);
        assert_eq!(hooks[0].workflow, "rebuild");
        assert_eq!(hooks[1].hook_type, HookType::PreCommit);
        assert_eq!(hooks[1].workflow, "lint");
    }

    #[test]
    fn list_empty_when_no_hooks() {
        let git = make_tmp_git();
        let hooks = list(&git).unwrap();
        assert!(hooks.is_empty());
    }

    #[test]
    fn list_ignores_sample_files() {
        let git = make_tmp_git();
        let sample = git.join("hooks").join("pre-commit.sample");
        fs::write(&sample, "#!/bin/sh\n").unwrap();

        let hooks = list(&git).unwrap();
        assert!(hooks.is_empty());
    }

    #[test]
    fn find_git_dir_finds_dot_git() {
        let tmp = make_tmp_git();
        let parent = tmp.parent().unwrap();
        let git = find_git_dir(parent).unwrap();
        assert!(git.ends_with(".git"), "got: {}", git.display());
    }

    #[test]
    fn find_git_dir_from_subdirectory() {
        let tmp = make_tmp_git();
        let subdir = tmp.parent().unwrap().join("subdir").join("nested");
        fs::create_dir_all(&subdir).unwrap();

        let git = find_git_dir(&subdir).unwrap();
        assert!(git.ends_with(".git"), "got: {}", git.display());
    }

    #[test]
    fn find_git_dir_not_found() {
        let dir = std::env::temp_dir().join("wan-hook-nogit-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert!(find_git_dir(&dir).is_none());
    }

    #[test]
    fn parse_marker_correct() {
        let content =
            "#!/bin/sh\n# wan-managed: pre-commit -> lint\nexec wan run lint --quiet \"$@\"\n";
        let result = parse_marker(content);
        assert_eq!(result, Some(("pre-commit".to_string(), "lint".to_string())));
    }

    #[test]
    fn parse_marker_none_for_custom() {
        let content = "#!/bin/sh\necho custom\n";
        assert!(parse_marker(content).is_none());
    }
}
