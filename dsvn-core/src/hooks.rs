//! SVN hook support for DSvn
//!
//! Supports standard SVN hooks:
//! - pre-commit: runs before a commit is finalized (can reject)
//! - post-commit: runs after a commit is finalized (notification)
//! - pre-revprop-change: runs before a revision property is changed (can reject)
//! - post-revprop-change: runs after a revision property is changed (notification)
//!
//! Hook scripts receive data on stdin and must exit with code 0 to succeed.
//! For pre-hooks, non-zero exit rejects the operation and stderr/stdout is
//! returned as the error message.

use anyhow::{anyhow, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Manages hook scripts for a repository.
pub struct HookManager {
    hooks_dir: PathBuf,
}

impl HookManager {
    /// Create a new HookManager for the given repository root path.
    /// Hooks are expected in `<repo_root>/hooks/`.
    pub fn new(repo_path: PathBuf) -> Self {
        Self {
            hooks_dir: repo_path.join("hooks"),
        }
    }

    /// Return the path to a named hook script.
    pub fn hook_path(&self, name: &str) -> PathBuf {
        self.hooks_dir.join(name)
    }

    /// Check whether a named hook exists and is executable.
    fn hook_exists(&self, name: &str) -> Option<PathBuf> {
        let p = self.hook_path(name);
        if p.exists() {
            Some(p)
        } else {
            None
        }
    }

    /// Execute a hook script, piping `stdin_data` to its stdin.
    /// Returns `Ok(())` if the hook does not exist or exits 0.
    /// Returns an error with the hook's output if it exits non-zero.
    fn run_hook(&self, name: &str, stdin_data: &str) -> Result<()> {
        let hook_path = match self.hook_exists(name) {
            Some(p) => p,
            None => return Ok(()), // No hook installed — allow
        };

        let mut child = Command::new(&hook_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .env("DSVN_REPO", self.hooks_dir.parent().unwrap_or(Path::new(".")))
            .spawn()
            .map_err(|e| anyhow!("Failed to execute hook '{}': {}", name, e))?;

        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(stdin_data.as_bytes());
        }

        let output = child
            .wait_with_output()
            .map_err(|e| anyhow!("Failed to wait for hook '{}': {}", name, e))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let msg = if !stderr.is_empty() {
                stderr.to_string()
            } else if !stdout.is_empty() {
                stdout.to_string()
            } else {
                format!(
                    "Hook '{}' exited with code {}",
                    name,
                    output.status.code().unwrap_or(-1)
                )
            };
            Err(anyhow!("Hook '{}' rejected the operation: {}", name, msg.trim()))
        }
    }

    // ── Commit hooks ───────────────────────────────────────────

    /// Run the **pre-commit** hook.
    ///
    /// `files` is a list of `(action, path)` pairs where action is "add",
    /// "modify", "delete", etc.
    pub fn run_pre_commit(
        &self,
        rev: u64,
        author: &str,
        log: &str,
        date: &str,
        files: &[(String, String)],
    ) -> Result<()> {
        let mut data = String::new();
        data.push_str(&format!("REVISION: {}\n", rev));
        data.push_str(&format!("AUTHOR: {}\n", author));
        data.push_str(&format!("DATE: {}\n", date));
        data.push_str(&format!("LOG: {}\n", log));
        data.push_str("FILES:\n");
        for (action, path) in files {
            data.push_str(&format!("{} {}\n", action, path));
        }
        self.run_hook("pre-commit", &data)
    }

    /// Run the **post-commit** hook (fire-and-forget — errors are logged but
    /// do not fail the operation).
    pub fn run_post_commit(&self, rev: u64, author: &str, log: &str, date: &str) -> Result<()> {
        let mut data = String::new();
        data.push_str(&format!("REVISION: {}\n", rev));
        data.push_str(&format!("AUTHOR: {}\n", author));
        data.push_str(&format!("DATE: {}\n", date));
        data.push_str(&format!("LOG: {}\n", log));
        // post-commit errors are intentionally swallowed to avoid breaking
        // the caller — the commit has already been persisted.
        if let Err(e) = self.run_hook("post-commit", &data) {
            tracing::warn!("post-commit hook error (ignored): {}", e);
        }
        Ok(())
    }

    // ── Revision-property hooks ────────────────────────────────

    /// Run the **pre-revprop-change** hook.
    ///
    /// * `action` — one of "M" (modify), "A" (add), "D" (delete).
    /// * `prop_name` — the property being changed (e.g. `svn:log`).
    /// * `prop_value` — the new value (empty string for delete).
    pub fn run_pre_revprop_change(
        &self,
        rev: u64,
        author: &str,
        prop_name: &str,
        action: &str,
        prop_value: &str,
    ) -> Result<()> {
        let mut data = String::new();
        data.push_str(&format!("REVISION: {}\n", rev));
        data.push_str(&format!("AUTHOR: {}\n", author));
        data.push_str(&format!("PROPNAME: {}\n", prop_name));
        data.push_str(&format!("ACTION: {}\n", action));
        data.push_str(&format!("VALUE: {}\n", prop_value));
        self.run_hook("pre-revprop-change", &data)
    }

    /// Run the **post-revprop-change** hook (fire-and-forget).
    pub fn run_post_revprop_change(
        &self,
        rev: u64,
        author: &str,
        prop_name: &str,
        action: &str,
    ) -> Result<()> {
        let mut data = String::new();
        data.push_str(&format!("REVISION: {}\n", rev));
        data.push_str(&format!("AUTHOR: {}\n", author));
        data.push_str(&format!("PROPNAME: {}\n", prop_name));
        data.push_str(&format!("ACTION: {}\n", action));
        if let Err(e) = self.run_hook("post-revprop-change", &data) {
            tracing::warn!("post-revprop-change hook error (ignored): {}", e);
        }
        Ok(())
    }

    /// Ensure the hooks directory exists (creates it if missing).
    pub fn ensure_hooks_dir(&self) -> Result<()> {
        if !self.hooks_dir.exists() {
            std::fs::create_dir_all(&self.hooks_dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_hook(dir: &Path, name: &str, script: &str) {
        let hooks_dir = dir.join("hooks");
        fs::create_dir_all(&hooks_dir).unwrap();
        let hook_path = hooks_dir.join(name);
        fs::write(&hook_path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn test_no_hook_allows() {
        let tmp = TempDir::new().unwrap();
        let mgr = HookManager::new(tmp.path().to_path_buf());
        // No hook file — should succeed
        assert!(mgr.run_pre_commit(1, "alice", "test msg", "2026-01-01T00:00:00Z", &[]).is_ok());
    }

    #[test]
    fn test_pre_commit_allow() {
        let tmp = TempDir::new().unwrap();
        make_hook(tmp.path(), "pre-commit", "#!/bin/bash\nexit 0\n");
        let mgr = HookManager::new(tmp.path().to_path_buf());
        assert!(mgr
            .run_pre_commit(
                1,
                "alice",
                "good commit",
                "2026-01-01T00:00:00Z",
                &[("A".into(), "/foo.txt".into())]
            )
            .is_ok());
    }

    #[test]
    fn test_pre_commit_reject() {
        let tmp = TempDir::new().unwrap();
        make_hook(
            tmp.path(),
            "pre-commit",
            "#!/bin/bash\necho 'Rejected by policy' >&2\nexit 1\n",
        );
        let mgr = HookManager::new(tmp.path().to_path_buf());
        let res = mgr.run_pre_commit(1, "alice", "bad", "2026-01-01T00:00:00Z", &[]);
        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(err_msg.contains("Rejected by policy"), "got: {}", err_msg);
    }

    #[test]
    fn test_pre_commit_receives_stdin() {
        let tmp = TempDir::new().unwrap();
        // Hook that checks for the LOG line via stdin
        make_hook(
            tmp.path(),
            "pre-commit",
            r#"#!/bin/bash
while IFS= read -r line; do
    if [[ "$line" =~ ^LOG:\ (.*)$ ]]; then
        log="${BASH_REMATCH[1]}"
        if [ ${#log} -lt 5 ]; then
            echo "Commit message too short" >&2
            exit 1
        fi
    fi
done
exit 0
"#,
        );
        let mgr = HookManager::new(tmp.path().to_path_buf());

        // Short message — should be rejected
        let res = mgr.run_pre_commit(1, "alice", "hi", "2026-01-01T00:00:00Z", &[]);
        assert!(res.is_err());

        // Long enough message — should succeed
        let res = mgr.run_pre_commit(1, "alice", "a valid commit message", "2026-01-01T00:00:00Z", &[]);
        assert!(res.is_ok());
    }

    #[test]
    fn test_post_commit_always_ok() {
        let tmp = TempDir::new().unwrap();
        // post-commit that exits non-zero — should still return Ok
        make_hook(
            tmp.path(),
            "post-commit",
            "#!/bin/bash\necho 'oops' >&2\nexit 1\n",
        );
        let mgr = HookManager::new(tmp.path().to_path_buf());
        assert!(mgr.run_post_commit(1, "alice", "msg", "2026-01-01T00:00:00Z").is_ok());
    }

    #[test]
    fn test_pre_revprop_change_reject() {
        let tmp = TempDir::new().unwrap();
        make_hook(
            tmp.path(),
            "pre-revprop-change",
            "#!/bin/bash\necho 'Cannot change revprops' >&2\nexit 1\n",
        );
        let mgr = HookManager::new(tmp.path().to_path_buf());
        let res = mgr.run_pre_revprop_change(1, "alice", "svn:log", "M", "new log");
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("Cannot change revprops"));
    }

    #[test]
    fn test_pre_revprop_change_allow() {
        let tmp = TempDir::new().unwrap();
        make_hook(
            tmp.path(),
            "pre-revprop-change",
            "#!/bin/bash\nexit 0\n",
        );
        let mgr = HookManager::new(tmp.path().to_path_buf());
        assert!(mgr.run_pre_revprop_change(1, "alice", "svn:log", "M", "new log").is_ok());
    }

    #[test]
    fn test_post_revprop_change_always_ok() {
        let tmp = TempDir::new().unwrap();
        make_hook(
            tmp.path(),
            "post-revprop-change",
            "#!/bin/bash\nexit 1\n",
        );
        let mgr = HookManager::new(tmp.path().to_path_buf());
        assert!(mgr.run_post_revprop_change(1, "alice", "svn:log", "M").is_ok());
    }

    #[test]
    fn test_ensure_hooks_dir() {
        let tmp = TempDir::new().unwrap();
        let mgr = HookManager::new(tmp.path().to_path_buf());
        assert!(!tmp.path().join("hooks").exists());
        mgr.ensure_hooks_dir().unwrap();
        assert!(tmp.path().join("hooks").exists());
    }

    #[test]
    fn test_hook_receives_env_var() {
        let tmp = TempDir::new().unwrap();
        // Hook that checks DSVN_REPO env var
        make_hook(
            tmp.path(),
            "pre-commit",
            &format!(
                "#!/bin/bash\nif [ \"$DSVN_REPO\" != \"{}\" ]; then echo 'bad env' >&2; exit 1; fi\nexit 0\n",
                tmp.path().display()
            ),
        );
        let mgr = HookManager::new(tmp.path().to_path_buf());
        assert!(mgr
            .run_pre_commit(1, "alice", "test", "2026-01-01T00:00:00Z", &[])
            .is_ok());
    }
}
