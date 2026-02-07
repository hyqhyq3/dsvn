//! SQLite-backed repository implementation
//!
//! Stores objects on disk using content-addressed filesystem (like git objects).
//! The working tree (file path -> entry mapping) is stored in an SQLite database
//! with WAL mode for high write throughput.

use crate::object::{Blob, Commit, ObjectId, ObjectKind, Tree, TreeEntry};
use crate::properties::PropertySet;
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;

/// Mirror of sled's TreeEntryRecord for migration purposes only
#[derive(Serialize, Deserialize)]
struct SledTreeEntryRecord {
    object_id: ObjectId,
    kind: ObjectKind,
    mode: u32,
}

fn kind_to_i64(kind: ObjectKind) -> i64 {
    match kind {
        ObjectKind::Blob => 0,
        ObjectKind::Tree => 1,
        _ => 0,
    }
}

fn i64_to_kind(i: i64) -> ObjectKind {
    if i == 0 { ObjectKind::Blob } else { ObjectKind::Tree }
}

fn bytes_to_oid(bytes: &[u8]) -> ObjectId {
    let mut arr = [0u8; 32];
    if bytes.len() == 32 { arr.copy_from_slice(bytes); }
    ObjectId::new(arr)
}

fn open_tree_db(root: &Path) -> Result<Connection> {
    let db_path = root.join("tree.sqlite");
    let conn = Connection::open(&db_path)
        .with_context(|| format!("Failed to open SQLite database at {:?}", db_path))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "OFF")?;
    conn.pragma_update(None, "cache_size", "-64000")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "mmap_size", "268435456")?;
    conn.pragma_update(None, "page_size", "4096")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tree_entries (
            path TEXT PRIMARY KEY,
            object_id BLOB NOT NULL,
            kind INTEGER NOT NULL,
            mode INTEGER NOT NULL
        ) WITHOUT ROWID;"
    )?;
    Ok(conn)
}

pub struct SqlitePropertyStore {
    root: PathBuf,
    cache: RwLock<HashMap<String, PropertySet>>,
}

impl SqlitePropertyStore {
    fn new(root: PathBuf) -> Self {
        Self { root, cache: RwLock::new(HashMap::new()) }
    }
    fn props_dir(&self) -> PathBuf { self.root.join("props") }
    fn path_hash(path: &str) -> String {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(path.as_bytes()))
    }
    fn prop_file(&self, path: &str) -> PathBuf {
        self.props_dir().join(format!("{}.json", Self::path_hash(path)))
    }
    pub async fn get(&self, path: &str) -> PropertySet {
        { let c = self.cache.read().await; if let Some(ps) = c.get(path) { return ps.clone(); } }
        let fp = self.prop_file(path);
        if fp.exists() {
            if let Ok(data) = fs::read_to_string(&fp) {
                if let Ok(ps) = serde_json::from_str::<PropertySet>(&data) {
                    self.cache.write().await.insert(path.to_string(), ps.clone());
                    return ps;
                }
            }
        }
        PropertySet::new()
    }
    pub async fn set(&self, path: String, name: String, value: String) -> Result<()> {
        let mut cache = self.cache.write().await;
        let ps = cache.entry(path.clone()).or_insert_with(PropertySet::new);
        ps.set(name, value);
        let fp = self.prop_file(&path);
        fs::create_dir_all(fp.parent().unwrap())?;
        fs::write(&fp, serde_json::to_string(ps)?)?;
        Ok(())
    }
    pub async fn remove(&self, path: &str, name: &str) -> Result<Option<String>> {
        let mut cache = self.cache.write().await;
        if let Some(ps) = cache.get_mut(path) {
            let val = ps.remove(name);
            let fp = self.prop_file(path);
            fs::create_dir_all(fp.parent().unwrap())?;
            fs::write(&fp, serde_json::to_string(ps)?)?;
            Ok(val)
        } else { Ok(None) }
    }
    pub async fn list(&self, path: &str) -> Vec<String> { self.get(path).await.list() }
    pub async fn contains(&self, path: &str, name: &str) -> bool { self.get(path).await.contains(name) }
}

/// SQLite-backed disk repository
pub struct SqliteRepository {
    root: PathBuf,
    uuid: String,
    current_rev: Arc<RwLock<u64>>,
    tree_conn: Mutex<Connection>,
    property_store: Arc<SqlitePropertyStore>,
    batch_mode: std::sync::atomic::AtomicBool,
}

fn conn_tree_insert(conn: &Connection, path: &str, oid: &ObjectId, kind: ObjectKind, mode: u32) -> Result<()> {
    conn.execute(
        "INSERT INTO tree_entries (path,object_id,kind,mode) VALUES (?1,?2,?3,?4) \
         ON CONFLICT(path) DO UPDATE SET object_id=excluded.object_id,kind=excluded.kind,mode=excluded.mode",
        rusqlite::params![path, oid.as_bytes().as_slice(), kind_to_i64(kind), mode as i64],
    )?;
    Ok(())
}

fn conn_tree_remove(conn: &Connection, path: &str) -> Result<()> {
    conn.execute("DELETE FROM tree_entries WHERE path=?1", rusqlite::params![path])?;
    Ok(())
}

fn conn_build_tree(conn: &Connection) -> Result<Tree> {
    let mut tree = Tree::new();
    let mut stmt = conn.prepare_cached("SELECT path,object_id,kind,mode FROM tree_entries ORDER BY path")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let p: String = row.get(0)?;
        let ob: Vec<u8> = row.get(1)?;
        let ki: i64 = row.get(2)?;
        let mo: i64 = row.get(3)?;
        tree.insert(TreeEntry::new(p, bytes_to_oid(&ob), i64_to_kind(ki), mo as u32));
    }
    Ok(tree)
}

fn conn_populate(conn: &Connection, tree: &Tree) -> Result<()> {
    conn.execute("DELETE FROM tree_entries", [])?;
    for e in tree.iter() {
        conn.execute(
            "INSERT INTO tree_entries (path,object_id,kind,mode) VALUES (?1,?2,?3,?4)",
            rusqlite::params![e.name, e.id.as_bytes().as_slice(), kind_to_i64(e.kind), e.mode as i64],
        )?;
    }
    Ok(())
}

fn conn_count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM tree_entries", [], |r| r.get(0))?)
}

impl SqliteRepository {
    pub fn open(path: &Path) -> Result<Self> {
        let root = path.to_path_buf();
        fs::create_dir_all(root.join("objects"))?;
        fs::create_dir_all(root.join("commits"))?;
        fs::create_dir_all(root.join("trees"))?;
        fs::create_dir_all(root.join("props"))?;
        fs::create_dir_all(root.join("refs"))?;

        let uuid_path = root.join("uuid");
        let uuid = if uuid_path.exists() {
            fs::read_to_string(&uuid_path)?.trim().to_string()
        } else {
            let u = uuid::Uuid::new_v4().to_string();
            fs::write(&uuid_path, &u)?; u
        };

        let head_path = root.join("refs").join("head");
        let current_rev = if head_path.exists() {
            fs::read_to_string(&head_path)?.trim().parse::<u64>().unwrap_or(0)
        } else { 0 };

        let tree_conn = open_tree_db(&root)?;

        // Migration: sled -> SQLite
        let sled_db_path = root.join("tree.db");
        if sled_db_path.exists() {
            let count: i64 = tree_conn.query_row("SELECT COUNT(*) FROM tree_entries", [], |r| r.get(0))?;
            if count == 0 {
                if let Ok(sled_db) = sled::open(&sled_db_path) {
                    let mut migrated = 0u64;
                    tree_conn.execute_batch("BEGIN")?;
                    for item in sled_db.iter() {
                        if let Ok((key, value)) = item {
                            if let Ok(ps) = String::from_utf8(key.to_vec()) {
                                if let Ok(rec) = bincode::deserialize::<SledTreeEntryRecord>(&value) {
                                    let _ = tree_conn.execute(
                                        "INSERT OR REPLACE INTO tree_entries (path,object_id,kind,mode) VALUES (?1,?2,?3,?4)",
                                        rusqlite::params![ps, rec.object_id.as_bytes().as_slice(), kind_to_i64(rec.kind), rec.mode as i64],
                                    );
                                    migrated += 1;
                                }
                            }
                        }
                    }
                    tree_conn.execute_batch("COMMIT")?;
                    if migrated > 0 { tracing::info!("Migrated {} entries from sled to SQLite", migrated); }
                }
            }
        }

        // Migration: root_tree.bin -> SQLite
        let rtp = root.join("root_tree.bin");
        if rtp.exists() {
            let count: i64 = tree_conn.query_row("SELECT COUNT(*) FROM tree_entries", [], |r| r.get(0))?;
            if count == 0 {
                if let Ok(data) = fs::read(&rtp) {
                    if let Ok(old_tree) = bincode::deserialize::<Tree>(&data) {
                        tree_conn.execute_batch("BEGIN")?;
                        for e in old_tree.iter() {
                            let _ = tree_conn.execute(
                                "INSERT OR REPLACE INTO tree_entries (path,object_id,kind,mode) VALUES (?1,?2,?3,?4)",
                                rusqlite::params![e.name, e.id.as_bytes().as_slice(), kind_to_i64(e.kind), e.mode as i64],
                            );
                        }
                        tree_conn.execute_batch("COMMIT")?;
                        let _ = fs::remove_file(&rtp);
                        tracing::info!("Migrated root_tree.bin to SQLite ({} entries)", old_tree.entries.len());
                    }
                }
            }
        }

        let property_store = Arc::new(SqlitePropertyStore::new(root.clone()));

        Ok(Self {
            root, uuid,
            current_rev: Arc::new(RwLock::new(current_rev)),
            tree_conn: Mutex::new(tree_conn),
            property_store,
            batch_mode: std::sync::atomic::AtomicBool::new(false),
        })
    }

    pub fn uuid(&self) -> &str { &self.uuid }
    pub async fn current_rev(&self) -> u64 { *self.current_rev.read().await }
    pub fn property_store(&self) -> &Arc<SqlitePropertyStore> { &self.property_store }
    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> { self.tree_conn.lock().unwrap() }

    fn object_path(&self, id: &ObjectId) -> PathBuf {
        let hex = id.to_hex();
        self.root.join("objects").join(&hex[..2]).join(&hex[2..])
    }
    fn store_object(&self, id: &ObjectId, data: &[u8]) -> Result<()> {
        let p = self.object_path(id);
        if p.exists() { return Ok(()); }
        fs::create_dir_all(p.parent().unwrap())?;
        let tmp = p.with_extension("tmp");
        fs::write(&tmp, data)?;
        fs::rename(&tmp, &p)?;
        Ok(())
    }
    fn load_object(&self, id: &ObjectId) -> Result<Vec<u8>> {
        let p = self.object_path(id);
        fs::read(&p).with_context(|| format!("Object {} not found at {:?}", id, p))
    }

    fn commit_path(&self, rev: u64) -> PathBuf { self.root.join("commits").join(format!("{}.bin", rev)) }
    fn store_commit(&self, rev: u64, commit: &Commit) -> Result<()> {
        fs::write(self.commit_path(rev), bincode::serialize(commit)?)?; Ok(())
    }
    fn load_commit(&self, rev: u64) -> Result<Commit> {
        let p = self.commit_path(rev);
        Ok(bincode::deserialize(&fs::read(&p).with_context(|| format!("Commit r{} not found", rev))?)?)
    }

    fn tree_snapshot_path(&self, rev: u64) -> PathBuf { self.root.join("trees").join(format!("{}.bin", rev)) }
    fn store_tree_snapshot(&self, rev: u64, tree: &Tree) -> Result<()> {
        fs::write(self.tree_snapshot_path(rev), bincode::serialize(tree)?)?; Ok(())
    }
    fn load_tree_snapshot(&self, rev: u64) -> Result<Tree> {
        let p = self.tree_snapshot_path(rev);
        Ok(bincode::deserialize(&fs::read(&p).with_context(|| format!("Tree r{} not found", rev))?)?)
    }
    fn save_head(&self, rev: u64) -> Result<()> {
        fs::write(self.root.join("refs").join("head"), rev.to_string())?; Ok(())
    }

    // ==================== Public API ====================

    pub async fn initialize(&self) -> Result<()> {
        let head_path = self.root.join("refs").join("head");
        if head_path.exists() {
            let rev = fs::read_to_string(&head_path)?.trim().parse::<u64>().unwrap_or(0);
            *self.current_rev.write().await = rev;
            let c = self.conn();
            if conn_count(&c)? == 0 {
                if let Ok(tree) = self.load_tree_snapshot(rev) { conn_populate(&c, &tree)?; }
            }
            return Ok(());
        }
        let tree = Tree::new();
        let tree_id = tree.id();
        self.store_object(&tree_id, &tree.to_bytes()?)?;
        let commit = Commit::new(tree_id, vec![], "system".into(), "Initial commit".into(), chrono::Utc::now().timestamp(), 0);
        self.store_object(&commit.id(), &commit.to_bytes()?)?;
        self.store_commit(0, &commit)?;
        self.store_tree_snapshot(0, &tree)?;
        self.save_head(0)?;
        self.conn().execute("DELETE FROM tree_entries", [])?;
        Ok(())
    }

    pub async fn get_file(&self, path: &str, rev: u64) -> Result<Bytes> {
        let commit = self.load_commit(rev)?;
        let tree: Tree = bincode::deserialize(&self.load_object(&commit.tree_id)?)?;
        let full_path = path.trim_start_matches('/');
        if let Some(entry) = tree.get(full_path) {
            if entry.kind == ObjectKind::Blob {
                return Ok(Bytes::from(Blob::deserialize(&self.load_object(&entry.id)?)?.data));
            }
        }
        let parts: Vec<&str> = full_path.split('/').filter(|p| !p.is_empty()).collect();
        let mut ct = tree;
        for (i, part) in parts.iter().enumerate() {
            if let Some(entry) = ct.get(*part) {
                if i == parts.len() - 1 {
                    return Ok(Bytes::from(Blob::deserialize(&self.load_object(&entry.id)?)?.data));
                }
                ct = bincode::deserialize(&self.load_object(&entry.id)?)?;
            } else { return Err(anyhow!("Path not found: {}", path)); }
        }
        Err(anyhow!("Path not found: {}", path))
    }

    pub async fn list_dir(&self, path: &str, rev: u64) -> Result<Vec<String>> {
        let commit = self.load_commit(rev)?;
        let mut ct: Tree = bincode::deserialize(&self.load_object(&commit.tree_id)?)?;
        for part in path.trim_start_matches('/').split('/').filter(|p| !p.is_empty()) {
            if let Some(e) = ct.get(part) { ct = bincode::deserialize(&self.load_object(&e.id)?)?; }
            else { return Err(anyhow!("Directory not found: {}", path)); }
        }
        Ok(ct.iter().map(|e| e.name.clone()).collect())
    }

    pub async fn add_file(&self, path: &str, content: Vec<u8>, executable: bool) -> Result<ObjectId> {
        let blob = Blob::new(content, executable);
        let bid = blob.id();
        self.store_object(&bid, &blob.to_bytes()?)?;
        conn_tree_insert(&self.conn(), path.trim_start_matches('/'), &bid, ObjectKind::Blob, if executable { 0o755 } else { 0o644 })?;
        Ok(bid)
    }

    pub async fn mkdir(&self, path: &str) -> Result<ObjectId> {
        let t = Tree::new(); let tid = t.id();
        self.store_object(&tid, &t.to_bytes()?)?;
        conn_tree_insert(&self.conn(), path.trim_start_matches('/'), &tid, ObjectKind::Tree, 0o755)?;
        Ok(tid)
    }

    pub async fn delete_file(&self, path: &str) -> Result<()> {
        conn_tree_remove(&self.conn(), path.trim_start_matches('/'))?; Ok(())
    }

    pub async fn commit(&self, author: String, message: String, timestamp: i64) -> Result<u64> {
        let root_tree = conn_build_tree(&self.conn())?;
        let tree_id = root_tree.id();
        self.store_object(&tree_id, &root_tree.to_bytes()?)?;
        let cr = *self.current_rev.read().await;
        let parents = if cr > 0 || self.commit_path(cr).exists() {
            self.load_commit(cr).map(|c| vec![c.id()]).unwrap_or_default()
        } else { vec![] };
        let commit = Commit::new(tree_id, parents, author, message, timestamp, 0);
        self.store_object(&commit.id(), &commit.to_bytes()?)?;
        let nr = cr + 1;
        self.store_commit(nr, &commit)?;
        self.store_tree_snapshot(nr, &root_tree)?;
        self.save_head(nr)?;
        *self.current_rev.write().await = nr;
        Ok(nr)
    }

    // ==================== Batch Import API ====================

    pub fn begin_batch(&self) {
        self.batch_mode.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = self.conn().execute_batch("BEGIN");
    }
    pub fn end_batch(&self) {
        self.batch_mode.store(false, std::sync::atomic::Ordering::Relaxed);
        let _ = self.conn().execute_batch("COMMIT");
    }

    pub fn add_file_sync(&self, path: &str, content: Vec<u8>, executable: bool) -> Result<ObjectId> {
        let blob = Blob::new(content, executable);
        let bid = blob.id();
        self.store_object(&bid, &blob.to_bytes()?)?;
        conn_tree_insert(&self.conn(), path.trim_start_matches('/'), &bid, ObjectKind::Blob, if executable { 0o755 } else { 0o644 })?;
        Ok(bid)
    }

    pub fn mkdir_sync(&self, path: &str) -> Result<ObjectId> {
        let t = Tree::new(); let tid = t.id();
        self.store_object(&tid, &t.to_bytes()?)?;
        conn_tree_insert(&self.conn(), path.trim_start_matches('/'), &tid, ObjectKind::Tree, 0o755)?;
        Ok(tid)
    }

    pub fn delete_file_sync(&self, path: &str) -> Result<()> {
        conn_tree_remove(&self.conn(), path.trim_start_matches('/'))?; Ok(())
    }

    pub fn commit_sync(&self, author: String, message: String, timestamp: i64) -> Result<u64> {
        let root_tree = conn_build_tree(&self.conn())?;
        let tree_id = root_tree.id();
        self.store_object(&tree_id, &root_tree.to_bytes()?)?;
        let cr = *self.current_rev.blocking_read();
        let parents = if cr > 0 || self.commit_path(cr).exists() {
            self.load_commit(cr).map(|c| vec![c.id()]).unwrap_or_default()
        } else { vec![] };
        let commit = Commit::new(tree_id, parents, author, message, timestamp, 0);
        self.store_object(&commit.id(), &commit.to_bytes()?)?;
        let nr = cr + 1;
        self.store_commit(nr, &commit)?;
        self.store_tree_snapshot(nr, &root_tree)?;
        self.save_head(nr)?;
        *self.current_rev.blocking_write() = nr;
        Ok(nr)
    }

    pub async fn log(&self, start_rev: u64, limit: usize) -> Result<Vec<Commit>> {
        let current = *self.current_rev.read().await;
        let end = std::cmp::min(start_rev, current);
        let mut result = Vec::new();
        for rev in (0..=end).rev() {
            if result.len() >= limit { break; }
            if let Ok(c) = self.load_commit(rev) { result.push(c); }
        }
        Ok(result)
    }

    pub async fn exists(&self, path: &str, rev: u64) -> Result<bool> {
        Ok(self.get_file(path, rev).await.is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_sqlite_repo_create_and_init() {
        let tmp = TempDir::new().unwrap();
        let repo = SqliteRepository::open(tmp.path()).unwrap();
        repo.initialize().await.unwrap();
        assert_eq!(repo.current_rev().await, 0);
        assert_eq!(repo.uuid().len(), 36);
    }

    #[tokio::test]
    async fn test_sqlite_repo_add_file_and_commit() {
        let tmp = TempDir::new().unwrap();
        let repo = SqliteRepository::open(tmp.path()).unwrap();
        repo.initialize().await.unwrap();
        repo.add_file("/test.txt", b"Hello, World!".to_vec(), false).await.unwrap();
        let rev = repo.commit("tester".into(), "add test.txt".into(), 1000).await.unwrap();
        assert_eq!(rev, 1);
        let content = repo.get_file("/test.txt", 1).await.unwrap();
        assert_eq!(content.as_ref(), b"Hello, World!");
    }

    #[tokio::test]
    async fn test_sqlite_repo_persistence() {
        let tmp = TempDir::new().unwrap();
        {
            let repo = SqliteRepository::open(tmp.path()).unwrap();
            repo.initialize().await.unwrap();
            repo.add_file("/hello.txt", b"hello".to_vec(), false).await.unwrap();
            repo.commit("user".into(), "first".into(), 100).await.unwrap();
        }
        {
            let repo = SqliteRepository::open(tmp.path()).unwrap();
            repo.initialize().await.unwrap();
            assert_eq!(repo.current_rev().await, 1);
            let content = repo.get_file("/hello.txt", 1).await.unwrap();
            assert_eq!(content.as_ref(), b"hello");
        }
    }

    #[tokio::test]
    async fn test_sqlite_repo_batch_sync() {
        let tmp = TempDir::new().unwrap();
        let repo = SqliteRepository::open(tmp.path()).unwrap();
        repo.initialize().await.unwrap();
        repo.begin_batch();
        for i in 0..100 {
            repo.add_file_sync(&format!("file_{}.txt", i), format!("content {}", i).into_bytes(), false).unwrap();
        }
        let rev = repo.commit_sync("bot".into(), "batch".into(), 1000).unwrap();
        repo.end_batch();
        assert_eq!(rev, 1);
        let content = repo.get_file("/file_0.txt", 1).await.unwrap();
        assert_eq!(content.as_ref(), b"content 0");
    }

    #[tokio::test]
    async fn test_sqlite_repo_delete() {
        let tmp = TempDir::new().unwrap();
        let repo = SqliteRepository::open(tmp.path()).unwrap();
        repo.initialize().await.unwrap();
        repo.add_file("/to_delete.txt", b"data".to_vec(), false).await.unwrap();
        repo.commit("user".into(), "add".into(), 100).await.unwrap();
        repo.delete_file("/to_delete.txt").await.unwrap();
        repo.commit("user".into(), "delete".into(), 200).await.unwrap();
        assert!(repo.get_file("/to_delete.txt", 1).await.is_ok());
        assert!(repo.get_file("/to_delete.txt", 2).await.is_err());
    }

    #[tokio::test]
    async fn test_sqlite_repo_many_commits() {
        let tmp = TempDir::new().unwrap();
        let repo = SqliteRepository::open(tmp.path()).unwrap();
        repo.initialize().await.unwrap();
        for i in 0..50 {
            repo.add_file(&format!("/file_{}.txt", i), format!("content {}", i).into_bytes(), false).await.unwrap();
            repo.commit("bot".into(), format!("commit {}", i), i as i64).await.unwrap();
        }
        assert_eq!(repo.current_rev().await, 50);
    }
}
