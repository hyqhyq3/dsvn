//! SVNSync compatibility layer for dsvn.
//!
//! Provides compatibility with SVN's `svnsync` tool by:
//! - Supporting `svn:sync-*` revision 0 properties
//! - Generating SVN-compatible replication log format
//! - Implementing pre-revprop-change hook auto-configuration
//! - Translating between dsvn's native format and SVN dump format

use anyhow::{anyhow, Result};
use dsvn_core::{
    DeltaTree, ObjectKind, SqliteRepository, TreeChange,
};
use std::collections::HashMap;
use std::fs;
use std::io::Write;

/// SVN sync property names (compatibility layer).
pub mod svn_props {
    pub const SYNC_FROM_URL: &str = "svn:sync-from-url";
    pub const SYNC_FROM_UUID: &str = "svn:sync-from-uuid";
    pub const SYNC_LAST_MERGED_REV: &str = "svn:sync-last-merged-rev";
    pub const SYNC_LOCK: &str = "svn:sync-lock";
    pub const SYNC_CURRENTLY_COPYING: &str = "svn:sync-currently-copying";
}

/// Manages SVN-compatible sync operations.
pub struct SvnSyncCompat<'a> {
    repo: &'a SqliteRepository,
}

impl<'a> SvnSyncCompat<'a> {
    pub fn new(repo: &'a SqliteRepository) -> Self {
        Self { repo }
    }

    /// Initialize a repository as a sync mirror (equivalent to `svnsync init`).
    ///
    /// Sets up the pre-revprop-change hook and sync properties on revision 0.
    pub fn init_mirror(
        &self,
        source_url: &str,
        source_uuid: &str,
    ) -> Result<()> {
        // 1. Install pre-revprop-change hook (required by svnsync)
        self.install_pre_revprop_change_hook()?;

        // 2. Set sync properties on revision 0
        self.set_revprop(0, svn_props::SYNC_FROM_URL, source_url)?;
        self.set_revprop(0, svn_props::SYNC_FROM_UUID, source_uuid)?;
        self.set_revprop(0, svn_props::SYNC_LAST_MERGED_REV, "0")?;

        Ok(())
    }

    /// Check if a repository is configured as a sync mirror.
    pub fn is_mirror(&self) -> Result<bool> {
        let url = self.get_revprop(0, svn_props::SYNC_FROM_URL)?;
        Ok(url.is_some())
    }

    /// Get the source URL of the mirror.
    pub fn get_source_url(&self) -> Result<Option<String>> {
        self.get_revprop(0, svn_props::SYNC_FROM_URL)
    }

    /// Get the source UUID of the mirror.
    pub fn get_source_uuid(&self) -> Result<Option<String>> {
        self.get_revprop(0, svn_props::SYNC_FROM_UUID)
    }

    /// Get the last merged revision.
    pub fn get_last_merged_rev(&self) -> Result<u64> {
        match self.get_revprop(0, svn_props::SYNC_LAST_MERGED_REV)? {
            Some(v) => Ok(v.trim().parse::<u64>().unwrap_or(0)),
            None => Ok(0),
        }
    }

    /// Update the last merged revision.
    pub fn set_last_merged_rev(&self, rev: u64) -> Result<()> {
        self.set_revprop(0, svn_props::SYNC_LAST_MERGED_REV, &rev.to_string())
    }

    /// Acquire a sync lock (prevents concurrent svnsync operations).
    pub fn acquire_lock(&self) -> Result<String> {
        // Check if already locked
        if let Some(existing) = self.get_revprop(0, svn_props::SYNC_LOCK)? {
            return Err(anyhow!(
                "Repository is already locked for sync by: {}",
                existing
            ));
        }

        let lock_token = format!(
            "{}:{}:{}",
            hostname(),
            std::process::id(),
            chrono::Utc::now().timestamp()
        );
        self.set_revprop(0, svn_props::SYNC_LOCK, &lock_token)?;
        Ok(lock_token)
    }

    /// Release the sync lock.
    pub fn release_lock(&self) -> Result<()> {
        self.del_revprop(0, svn_props::SYNC_LOCK)
    }

    /// Set the "currently copying" revision (for crash recovery).
    pub fn set_currently_copying(&self, rev: u64) -> Result<()> {
        self.set_revprop(0, svn_props::SYNC_CURRENTLY_COPYING, &rev.to_string())
    }

    /// Clear the "currently copying" marker.
    pub fn clear_currently_copying(&self) -> Result<()> {
        self.del_revprop(0, svn_props::SYNC_CURRENTLY_COPYING)
    }

    /// Get the "currently copying" revision.
    pub fn get_currently_copying(&self) -> Result<Option<u64>> {
        match self.get_revprop(0, svn_props::SYNC_CURRENTLY_COPYING)? {
            Some(v) => Ok(Some(v.trim().parse::<u64>().unwrap_or(0))),
            None => Ok(None),
        }
    }

    /// Install a permissive pre-revprop-change hook (required by svnsync).
    pub fn install_pre_revprop_change_hook(&self) -> Result<()> {
        let hooks_dir = self.repo.root().join("hooks");
        fs::create_dir_all(&hooks_dir)?;

        let hook_path = hooks_dir.join("pre-revprop-change");
        if !hook_path.exists() {
            let script = "#!/bin/sh\n# Auto-installed by dsvnsync for svnsync compatibility.\n# Allows all revision property changes.\nexit 0\n";
            fs::write(&hook_path, script)?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755))?;
            }
        }
        Ok(())
    }

    /// Generate SVN-compatible replication log output for a revision range.
    /// This produces output similar to what svnsync would consume.
    /// NOTE: Uses blocking I/O only — safe to call from any context.
    pub fn generate_repl_log(
        &self,
        from_rev: u64,
        to_rev: u64,
        writer: &mut dyn Write,
    ) -> Result<u64> {
        // Read HEAD revision directly from disk to avoid needing async runtime
        let head_path = self.repo.root().join("refs").join("head");
        let head: u64 = if head_path.exists() {
            fs::read_to_string(&head_path)?
                .trim()
                .parse()
                .unwrap_or(0)
        } else {
            0
        };
        let end = to_rev.min(head);
        let mut count = 0u64;

        for rev in from_rev..=end {
            // Load commit directly from disk (no async needed)
            let commit_path = self.repo.root().join("commits").join(format!("{}.bin", rev));
            if let Ok(data) = fs::read(&commit_path) {
                if let Ok(commit) = bincode::deserialize::<dsvn_core::Commit>(&data) {
                    let date = chrono::DateTime::from_timestamp(commit.timestamp, 0)
                        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S.000000Z").to_string())
                        .unwrap_or_default();

                    writeln!(writer, "---")?;
                    writeln!(writer, "Revision: {}", rev)?;
                    writeln!(writer, "Author: {}", commit.author)?;
                    writeln!(writer, "Date: {}", date)?;
                    writeln!(writer, "Log: {}", commit.message)?;

                    // Include changes if available
                    let delta_path = self.repo.root().join("tree_deltas").join(format!("{}.bin", rev));
                    if delta_path.exists() {
                        if let Ok(ddata) = fs::read(&delta_path) {
                            if let Ok(delta) = bincode::deserialize::<DeltaTree>(&ddata) {
                                writeln!(writer, "Changes: {}", delta.changes.len())?;
                                for change in &delta.changes {
                                    match change {
                                        TreeChange::Upsert { path, entry } => {
                                            let kind = if entry.kind == ObjectKind::Blob {
                                                "file"
                                            } else {
                                                "dir"
                                            };
                                            writeln!(writer, "  A {} ({})", path, kind)?;
                                        }
                                        TreeChange::Delete { path } => {
                                            writeln!(writer, "  D {}", path)?;
                                        }
                                    }
                                }
                            }
                        }
                    }

                    count += 1;
                }
            }
        }

        Ok(count)
    }

    // ── Internal helpers ──

    fn revprops_path(&self, rev: u64) -> std::path::PathBuf {
        self.repo.root().join("revprops").join(format!("{}.json", rev))
    }

    fn load_revprops(&self, rev: u64) -> HashMap<String, String> {
        let path = self.revprops_path(rev);
        if let Ok(data) = fs::read_to_string(&path) {
            if let Ok(props) = serde_json::from_str(&data) {
                return props;
            }
        }
        HashMap::new()
    }

    fn save_revprops(&self, rev: u64, props: &HashMap<String, String>) -> Result<()> {
        let dir = self.repo.root().join("revprops");
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.json", rev));
        if props.is_empty() {
            if path.exists() {
                fs::remove_file(&path)?;
            }
        } else {
            fs::write(&path, serde_json::to_string_pretty(props)?)?;
        }
        Ok(())
    }

    fn get_revprop(&self, rev: u64, name: &str) -> Result<Option<String>> {
        let props = self.load_revprops(rev);
        Ok(props.get(name).cloned())
    }

    fn set_revprop(&self, rev: u64, name: &str, value: &str) -> Result<()> {
        let mut props = self.load_revprops(rev);
        props.insert(name.to_string(), value.to_string());
        self.save_revprops(rev, &props)
    }

    fn del_revprop(&self, rev: u64, name: &str) -> Result<()> {
        let mut props = self.load_revprops(rev);
        props.remove(name);
        self.save_revprops(rev, &props)
    }
}

/// Get the hostname for lock tokens.
fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;

    fn make_repo(path: &Path) -> SqliteRepository {
        let repo = SqliteRepository::open(path).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(repo.initialize()).unwrap();
        repo
    }

    #[test]
    fn test_init_mirror() {
        let tmp = TempDir::new().unwrap();
        let repo = make_repo(tmp.path());
        let compat = SvnSyncCompat::new(&repo);

        compat
            .init_mirror("file:///source", "uuid-123")
            .unwrap();

        assert!(compat.is_mirror().unwrap());
        assert_eq!(
            compat.get_source_url().unwrap(),
            Some("file:///source".to_string())
        );
        assert_eq!(
            compat.get_source_uuid().unwrap(),
            Some("uuid-123".to_string())
        );
        assert_eq!(compat.get_last_merged_rev().unwrap(), 0);

        // Check hook was installed
        let hook = tmp.path().join("hooks").join("pre-revprop-change");
        assert!(hook.exists());
    }

    #[test]
    fn test_sync_lock() {
        let tmp = TempDir::new().unwrap();
        let repo = make_repo(tmp.path());
        let compat = SvnSyncCompat::new(&repo);

        let token = compat.acquire_lock().unwrap();
        assert!(!token.is_empty());

        // Second lock should fail
        assert!(compat.acquire_lock().is_err());

        // Release and re-acquire should work
        compat.release_lock().unwrap();
        let _token2 = compat.acquire_lock().unwrap();
    }

    #[test]
    fn test_currently_copying() {
        let tmp = TempDir::new().unwrap();
        let repo = make_repo(tmp.path());
        let compat = SvnSyncCompat::new(&repo);

        assert!(compat.get_currently_copying().unwrap().is_none());

        compat.set_currently_copying(42).unwrap();
        assert_eq!(compat.get_currently_copying().unwrap(), Some(42));

        compat.clear_currently_copying().unwrap();
        assert!(compat.get_currently_copying().unwrap().is_none());
    }

    #[test]
    fn test_last_merged_rev() {
        let tmp = TempDir::new().unwrap();
        let repo = make_repo(tmp.path());
        let compat = SvnSyncCompat::new(&repo);

        compat
            .init_mirror("file:///source", "uuid-123")
            .unwrap();

        assert_eq!(compat.get_last_merged_rev().unwrap(), 0);
        compat.set_last_merged_rev(50).unwrap();
        assert_eq!(compat.get_last_merged_rev().unwrap(), 50);
    }

    #[test]
    fn test_not_mirror() {
        let tmp = TempDir::new().unwrap();
        let repo = make_repo(tmp.path());
        let compat = SvnSyncCompat::new(&repo);

        assert!(!compat.is_mirror().unwrap());
        assert!(compat.get_source_url().unwrap().is_none());
    }

    #[test]
    fn test_generate_repl_log() {
        let tmp = TempDir::new().unwrap();
        let repo = make_repo(tmp.path());

        // Create some commits
        repo.add_file_sync("test.txt", b"hello".to_vec(), false)
            .unwrap();
        repo.commit_sync("alice".into(), "first commit".into(), 1000)
            .unwrap();

        repo.add_file_sync("test2.txt", b"world".to_vec(), false)
            .unwrap();
        repo.commit_sync("bob".into(), "second commit".into(), 2000)
            .unwrap();

        let compat = SvnSyncCompat::new(&repo);
        let mut output = Vec::new();
        let count = compat.generate_repl_log(1, 2, &mut output).unwrap();

        assert_eq!(count, 2);
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("alice"));
        assert!(output_str.contains("bob"));
        assert!(output_str.contains("first commit"));
        assert!(output_str.contains("second commit"));
    }
}
