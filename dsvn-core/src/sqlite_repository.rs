//! SQLite-backed repository with incremental (delta) tree storage.
//!
//! Each commit stores only changes (DeltaTree) instead of full tree snapshots.
//! Trees are reconstructed on demand with LRU caching.
//! Full snapshots every SNAPSHOT_INTERVAL revisions bound reconstruction cost.

use crate::hooks::HookManager;
use crate::object::{Blob, Commit, DeltaTree, ObjectId, ObjectKind, Tree, TreeChange, TreeEntry};
use crate::properties::PropertySet;
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use lru::LruCache;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;

const SNAPSHOT_INTERVAL: u64 = 1000;
const TREE_CACHE_CAPACITY: usize = 64;

#[derive(Serialize, Deserialize)]
struct SledTreeEntryRecord { object_id: ObjectId, kind: ObjectKind, mode: u32 }

fn kind_to_i64(k: ObjectKind) -> i64 { match k { ObjectKind::Blob => 0, ObjectKind::Tree => 1, _ => 0 } }
fn i64_to_kind(i: i64) -> ObjectKind { if i == 0 { ObjectKind::Blob } else { ObjectKind::Tree } }
fn bytes_to_oid(b: &[u8]) -> ObjectId { let mut a = [0u8; 32]; if b.len() == 32 { a.copy_from_slice(b); } ObjectId::new(a) }

fn tree_to_map(tree: &Tree) -> HashMap<String, TreeEntry> {
    tree.entries.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}
fn map_to_tree(map: &HashMap<String, TreeEntry>) -> Tree {
    let mut t = Tree::new(); for e in map.values() { t.insert(e.clone()); } t
}

fn open_tree_db(root: &Path) -> Result<Connection> {
    let db_path = root.join("tree.sqlite");
    let conn = Connection::open(&db_path)
        .with_context(|| format!("Failed to open SQLite at {:?}", db_path))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "OFF")?;
    conn.pragma_update(None, "cache_size", "-64000")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "mmap_size", "268435456")?;
    conn.pragma_update(None, "page_size", "4096")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tree_entries (
            path TEXT PRIMARY KEY, object_id BLOB NOT NULL, kind INTEGER NOT NULL, mode INTEGER NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS pending_changes (
            path TEXT PRIMARY KEY, change_type INTEGER NOT NULL, object_id BLOB, kind INTEGER, mode INTEGER
        ) WITHOUT ROWID;"
    )?;
    Ok(conn)
}

fn conn_tree_insert(c: &Connection, path: &str, oid: &ObjectId, kind: ObjectKind, mode: u32) -> Result<()> {
    c.execute("INSERT INTO tree_entries (path,object_id,kind,mode) VALUES (?1,?2,?3,?4) ON CONFLICT(path) DO UPDATE SET object_id=excluded.object_id,kind=excluded.kind,mode=excluded.mode",
        rusqlite::params![path, oid.as_bytes().as_slice(), kind_to_i64(kind), mode as i64])?;
    c.execute("INSERT INTO pending_changes (path,change_type,object_id,kind,mode) VALUES (?1,0,?2,?3,?4) ON CONFLICT(path) DO UPDATE SET change_type=0,object_id=excluded.object_id,kind=excluded.kind,mode=excluded.mode",
        rusqlite::params![path, oid.as_bytes().as_slice(), kind_to_i64(kind), mode as i64])?;
    Ok(())
}

fn conn_tree_remove(c: &Connection, path: &str) -> Result<()> {
    c.execute("DELETE FROM tree_entries WHERE path=?1", rusqlite::params![path])?;
    c.execute("DELETE FROM tree_entries WHERE path LIKE ?1", rusqlite::params![format!("{}/%", path)])?;
    c.execute("INSERT INTO pending_changes (path,change_type,object_id,kind,mode) VALUES (?1,1,NULL,NULL,NULL) ON CONFLICT(path) DO UPDATE SET change_type=1,object_id=NULL,kind=NULL,mode=NULL",
        rusqlite::params![path])?;
    Ok(())
}

fn conn_build_tree(c: &Connection) -> Result<Tree> {
    let mut tree = Tree::new();
    let mut stmt = c.prepare_cached("SELECT path,object_id,kind,mode FROM tree_entries ORDER BY path")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        tree.insert(TreeEntry::new(row.get::<_,String>(0)?, bytes_to_oid(&row.get::<_,Vec<u8>>(1)?), i64_to_kind(row.get(2)?), row.get::<_,i64>(3)? as u32));
    }
    Ok(tree)
}

fn conn_collect_pending(c: &Connection) -> Result<(Vec<TreeChange>, usize)> {
    let mut changes = Vec::new();
    {
        let mut stmt = c.prepare_cached("SELECT path,change_type,object_id,kind,mode FROM pending_changes ORDER BY path")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let path: String = row.get(0)?;
            let ct: i64 = row.get(1)?;
            if ct == 1 {
                changes.push(TreeChange::Delete { path });
            } else {
                changes.push(TreeChange::Upsert {
                    path: path.clone(),
                    entry: TreeEntry::new(path, bytes_to_oid(&row.get::<_,Vec<u8>>(2)?), i64_to_kind(row.get(3)?), row.get::<_,i64>(4)? as u32),
                });
            }
        }
    }
    let total: i64 = c.query_row("SELECT COUNT(*) FROM tree_entries", [], |r| r.get(0))?;
    c.execute("DELETE FROM pending_changes", [])?;
    Ok((changes, total as usize))
}

fn conn_populate(c: &Connection, tree: &Tree) -> Result<()> {
    c.execute("DELETE FROM tree_entries", [])?;
    for e in tree.iter() {
        c.execute("INSERT INTO tree_entries (path,object_id,kind,mode) VALUES (?1,?2,?3,?4)",
            rusqlite::params![e.name, e.id.as_bytes().as_slice(), kind_to_i64(e.kind), e.mode as i64])?;
    }
    Ok(())
}

fn conn_count(c: &Connection) -> Result<i64> {
    Ok(c.query_row("SELECT COUNT(*) FROM tree_entries", [], |r| r.get(0))?)
}

pub struct SqlitePropertyStore {
    root: PathBuf,
    cache: RwLock<HashMap<String, PropertySet>>,
}

impl SqlitePropertyStore {
    fn new(root: PathBuf) -> Self { Self { root, cache: RwLock::new(HashMap::new()) } }
    fn props_dir(&self) -> PathBuf { self.root.join("props") }
    fn path_hash(path: &str) -> String { use sha2::{Digest, Sha256}; hex::encode(Sha256::digest(path.as_bytes())) }
    fn prop_file(&self, path: &str) -> PathBuf { self.props_dir().join(format!("{}.json", Self::path_hash(path))) }
    pub async fn get(&self, path: &str) -> PropertySet {
        { let c = self.cache.read().await; if let Some(ps) = c.get(path) { return ps.clone(); } }
        let fp = self.prop_file(path);
        if fp.exists() { if let Ok(data) = fs::read_to_string(&fp) { if let Ok(ps) = serde_json::from_str::<PropertySet>(&data) { self.cache.write().await.insert(path.to_string(), ps.clone()); return ps; } } }
        PropertySet::new()
    }
    pub async fn set(&self, path: String, name: String, value: String) -> Result<()> {
        let mut cache = self.cache.write().await;
        let ps = cache.entry(path.clone()).or_insert_with(PropertySet::new);
        ps.set(name, value);
        let fp = self.prop_file(&path); fs::create_dir_all(fp.parent().unwrap())?;
        fs::write(&fp, serde_json::to_string(ps)?)?; Ok(())
    }
    pub async fn remove(&self, path: &str, name: &str) -> Result<Option<String>> {
        let mut cache = self.cache.write().await;
        if let Some(ps) = cache.get_mut(path) { let v = ps.remove(name); let fp = self.prop_file(path); fs::create_dir_all(fp.parent().unwrap())?; fs::write(&fp, serde_json::to_string(ps)?)?; Ok(v) } else { Ok(None) }
    }
    pub async fn list(&self, path: &str) -> Vec<String> { self.get(path).await.list() }
    pub async fn contains(&self, path: &str, name: &str) -> bool { self.get(path).await.contains(name) }
}

pub struct SqliteRepository {
    root: PathBuf,
    uuid: String,
    current_rev: Arc<RwLock<u64>>,
    tree_conn: Mutex<Connection>,
    property_store: Arc<SqlitePropertyStore>,
    batch_mode: std::sync::atomic::AtomicBool,
    tree_cache: Mutex<LruCache<u64, HashMap<String, TreeEntry>>>,
    /// Commit lock: serializes commit operations to prevent concurrent revision conflicts.
    commit_lock: tokio::sync::Mutex<()>,
}

impl SqliteRepository {
    pub fn open(path: &Path) -> Result<Self> {
        let root = path.to_path_buf();
        for d in &["objects","commits","trees","tree_deltas","props","refs"] { fs::create_dir_all(root.join(d))?; }
        let uuid_path = root.join("uuid");
        let uuid = if uuid_path.exists() { fs::read_to_string(&uuid_path)?.trim().to_string() }
            else { let u = uuid::Uuid::new_v4().to_string(); fs::write(&uuid_path, &u)?; u };
        let head_path = root.join("refs").join("head");
        let current_rev = if head_path.exists() { fs::read_to_string(&head_path)?.trim().parse::<u64>().unwrap_or(0) } else { 0 };
        let tree_conn = open_tree_db(&root)?;

        // Migration: sled -> SQLite
        let sled_db_path = root.join("tree.db");
        if sled_db_path.exists() {
            let count: i64 = tree_conn.query_row("SELECT COUNT(*) FROM tree_entries", [], |r| r.get(0))?;
            if count == 0 {
                if let Ok(sled_db) = sled::open(&sled_db_path) {
                    tree_conn.execute_batch("BEGIN")?;
                    for item in sled_db.iter() {
                        if let Ok((key, value)) = item {
                            if let Ok(ps) = String::from_utf8(key.to_vec()) {
                                if let Ok(rec) = bincode::deserialize::<SledTreeEntryRecord>(&value) {
                                    let _ = tree_conn.execute("INSERT OR REPLACE INTO tree_entries (path,object_id,kind,mode) VALUES (?1,?2,?3,?4)",
                                        rusqlite::params![ps, rec.object_id.as_bytes().as_slice(), kind_to_i64(rec.kind), rec.mode as i64]);
                                }
                            }
                        }
                    }
                    tree_conn.execute_batch("COMMIT")?;
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
                            let _ = tree_conn.execute("INSERT OR REPLACE INTO tree_entries (path,object_id,kind,mode) VALUES (?1,?2,?3,?4)",
                                rusqlite::params![e.name, e.id.as_bytes().as_slice(), kind_to_i64(e.kind), e.mode as i64]);
                        }
                        tree_conn.execute_batch("COMMIT")?;
                        let _ = fs::remove_file(&rtp);
                    }
                }
            }
        }

        Ok(Self {
            root: root.clone(), uuid, current_rev: Arc::new(RwLock::new(current_rev)),
            tree_conn: Mutex::new(tree_conn),
            property_store: Arc::new(SqlitePropertyStore::new(root)),
            batch_mode: std::sync::atomic::AtomicBool::new(false),
            tree_cache: Mutex::new(LruCache::new(NonZeroUsize::new(TREE_CACHE_CAPACITY).unwrap())),
            commit_lock: tokio::sync::Mutex::new(()),
        })
    }

    pub fn uuid(&self) -> &str { &self.uuid }
    pub async fn current_rev(&self) -> u64 { *self.current_rev.read().await }
    pub fn property_store(&self) -> &Arc<SqlitePropertyStore> { &self.property_store }
    pub fn hook_manager(&self) -> HookManager { HookManager::new(self.root.clone()) }
    
    pub fn root(&self) -> &Path { &self.root }
    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> { self.tree_conn.lock().unwrap() }

    fn object_path(&self, id: &ObjectId) -> PathBuf { let h = id.to_hex(); self.root.join("objects").join(&h[..2]).join(&h[2..]) }
    fn store_object(&self, id: &ObjectId, data: &[u8]) -> Result<()> {
        let p = self.object_path(id); if p.exists() { return Ok(()); }
        fs::create_dir_all(p.parent().unwrap())?; let tmp = p.with_extension("tmp"); fs::write(&tmp, data)?; fs::rename(&tmp, &p)?; Ok(())
    }
    fn load_object(&self, id: &ObjectId) -> Result<Vec<u8>> { let p = self.object_path(id); fs::read(&p).with_context(|| format!("Object {} not found", id)) }

    fn commit_path(&self, rev: u64) -> PathBuf { self.root.join("commits").join(format!("{}.bin", rev)) }
    fn store_commit(&self, rev: u64, c: &Commit) -> Result<()> { fs::write(self.commit_path(rev), bincode::serialize(c)?)?; Ok(()) }
    fn load_commit(&self, rev: u64) -> Result<Commit> { Ok(bincode::deserialize(&fs::read(self.commit_path(rev)).with_context(|| format!("Commit r{} not found", rev))?)?) }

    fn tree_snapshot_path(&self, rev: u64) -> PathBuf { self.root.join("trees").join(format!("{}.bin", rev)) }
    fn store_tree_snapshot(&self, rev: u64, t: &Tree) -> Result<()> { fs::write(self.tree_snapshot_path(rev), bincode::serialize(t)?)?; Ok(()) }
    fn load_tree_snapshot(&self, rev: u64) -> Result<Tree> { Ok(bincode::deserialize(&fs::read(self.tree_snapshot_path(rev)).with_context(|| format!("Tree r{} not found", rev))?)?) }
    fn has_tree_snapshot(&self, rev: u64) -> bool { self.tree_snapshot_path(rev).exists() }

    fn delta_tree_path(&self, rev: u64) -> PathBuf { self.root.join("tree_deltas").join(format!("{}.bin", rev)) }
    fn store_delta_tree(&self, rev: u64, d: &DeltaTree) -> Result<()> { fs::write(self.delta_tree_path(rev), bincode::serialize(d)?)?; Ok(()) }
    fn load_delta_tree(&self, rev: u64) -> Result<DeltaTree> { Ok(bincode::deserialize(&fs::read(self.delta_tree_path(rev)).with_context(|| format!("Delta r{} not found", rev))?)?) }
    fn has_delta_tree(&self, rev: u64) -> bool { self.delta_tree_path(rev).exists() }

    fn save_head(&self, rev: u64) -> Result<()> { fs::write(self.root.join("refs").join("head"), rev.to_string())?; Ok(()) }

    /// Reconstruct the full tree at a given revision via delta chain + cache + snapshots.
    pub fn get_tree_at_rev(&self, rev: u64) -> Result<HashMap<String, TreeEntry>> {
        if rev == 0 { return Ok(HashMap::new()); }
        { let mut c = self.tree_cache.lock().unwrap(); if let Some(t) = c.get(&rev) { return Ok(t.clone()); } }
        if self.has_tree_snapshot(rev) {
            let m = tree_to_map(&self.load_tree_snapshot(rev)?);
            self.tree_cache.lock().unwrap().put(rev, m.clone()); return Ok(m);
        }
        // Collect delta chain
        let mut chain: Vec<(u64, DeltaTree)> = Vec::new();
        let mut cur = rev;
        loop {
            if cur == 0 { break; }
            { let mut c = self.tree_cache.lock().unwrap(); if c.get(&cur).is_some() { break; } }
            if self.has_tree_snapshot(cur) { break; }
            if self.has_delta_tree(cur) {
                let d = self.load_delta_tree(cur)?; let p = d.parent_rev; chain.push((cur, d)); cur = p;
            } else {
                // Legacy: full tree in objects
                return self.get_tree_at_rev_legacy(rev);
            }
        }
        // Base tree
        let mut tree = if cur == 0 { HashMap::new() } else {
            let cached = { self.tree_cache.lock().unwrap().get(&cur).cloned() };
            if let Some(t) = cached { t }
            else if self.has_tree_snapshot(cur) {
                let m = tree_to_map(&self.load_tree_snapshot(cur)?);
                self.tree_cache.lock().unwrap().put(cur, m.clone()); m
            } else { return Err(anyhow!("No base tree for rev {}", cur)); }
        };
        // Apply deltas oldest-first
        for (dr, delta) in chain.into_iter().rev() {
            for ch in &delta.changes {
                match ch {
                    TreeChange::Upsert { path, entry } => { tree.insert(path.clone(), entry.clone()); }
                    TreeChange::Delete { path } => { tree.remove(path); let pfx = format!("{}/", path); tree.retain(|k,_| !k.starts_with(&pfx)); }
                }
            }
            self.tree_cache.lock().unwrap().put(dr, tree.clone());
        }
        Ok(tree)
    }

    fn get_tree_at_rev_legacy(&self, rev: u64) -> Result<HashMap<String, TreeEntry>> {
        let c = self.load_commit(rev)?;
        let t: Tree = bincode::deserialize(&self.load_object(&c.tree_id)?)?;
        let m = tree_to_map(&t);
        self.tree_cache.lock().unwrap().put(rev, m.clone());
        Ok(m)
    }

    pub async fn initialize(&self) -> Result<()> {
        let head_path = self.root.join("refs").join("head");
        if head_path.exists() {
            let rev = fs::read_to_string(&head_path)?.trim().parse::<u64>().unwrap_or(0);
            *self.current_rev.write().await = rev;
            let c = self.conn();
            if conn_count(&c)? == 0 {
                drop(c);
                if let Ok(tm) = self.get_tree_at_rev(rev) { conn_populate(&self.conn(), &map_to_tree(&tm))?; }
                else if let Ok(t) = self.load_tree_snapshot(rev) { conn_populate(&self.conn(), &t)?; }
            }
            return Ok(());
        }
        let tree = Tree::new(); let tid = tree.id();
        self.store_object(&tid, &tree.to_bytes()?)?;
        let commit = Commit::new(tid, vec![], "system".into(), "Initial commit".into(), chrono::Utc::now().timestamp(), 0);
        self.store_object(&commit.id(), &commit.to_bytes()?)?;
        self.store_commit(0, &commit)?;
        self.store_tree_snapshot(0, &tree)?;
        self.store_delta_tree(0, &DeltaTree::new(0, vec![], 0))?;
        self.save_head(0)?;
        { let c = self.conn(); c.execute("DELETE FROM tree_entries", [])?; c.execute("DELETE FROM pending_changes", [])?; }
        Ok(())
    }

    pub async fn get_file(&self, path: &str, rev: u64) -> Result<Bytes> {
        let fp = path.trim_start_matches('/');
        let tm = self.get_tree_at_rev(rev)?;
        if let Some(e) = tm.get(fp) {
            if e.kind == ObjectKind::Blob {
                return Ok(Bytes::from(Blob::deserialize(&self.load_object(&e.id)?)?.data));
            }
        }
        Err(anyhow!("Path not found: {}", path))
    }

    pub async fn list_dir(&self, path: &str, rev: u64) -> Result<Vec<String>> {
        let tm = self.get_tree_at_rev(rev)?;
        let pfx = path.trim_start_matches('/').trim_end_matches('/');
        let dir_pfx = if pfx.is_empty() { String::new() } else { format!("{}/", pfx) };
        let mut names: Vec<String> = tm.keys()
            .filter_map(|k| {
                if pfx.is_empty() { k.split('/').next().map(|s| s.to_string()) }
                else { k.strip_prefix(&dir_pfx).and_then(|r| r.split('/').next()).map(|s| s.to_string()) }
            })
            .collect::<std::collections::HashSet<_>>().into_iter().collect();
        names.sort();
        if names.is_empty() && !pfx.is_empty() { return Err(anyhow!("Directory not found: {}", path)); }
        Ok(names)
    }

    pub async fn add_file(&self, path: &str, content: Vec<u8>, executable: bool) -> Result<ObjectId> {
        let blob = Blob::new(content, executable); let bid = blob.id();
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
        // Serialize commits to prevent concurrent revision conflicts
        let _lock = self.commit_lock.lock().await;

        let cr = *self.current_rev.read().await;
        let nr = cr + 1;
        let parent_rev = if nr > 1 { nr - 1 } else { 0 };

        // Scope the connection guard so it's dropped before any await
        let (delta, root_tree, hook_mgr, _files, date) = {
            let c = self.conn();

            // Collect pending changes -> DeltaTree
            let (changes, total_entries) = conn_collect_pending(&c)?;
            tracing::debug!("Collected {} changes for commit", changes.len());

            // Prepare hook data
            tracing::info!("Creating HookManager with root: {:?}", self.root);
            let hook_mgr = HookManager::new(self.root.clone());
            let files: Vec<(String, String)> = changes.iter().map(|ch| match ch {
                TreeChange::Upsert { path, .. } => ("A".into(), path.clone()),
                TreeChange::Delete { path } => ("D".into(), path.clone()),
            }).collect();
            let date = chrono::DateTime::from_timestamp(timestamp, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_else(|| timestamp.to_string());

            // Run pre-commit hook (can reject the commit)
            tracing::info!("Running pre-commit hook for revision {}", nr);
            tracing::info!("Hook files: {:?}", files);
            hook_mgr.run_pre_commit(nr, &author, &message, &date, &files)?;
            tracing::info!("Pre-commit hook passed for revision {}", nr);

            let delta = DeltaTree::new(parent_rev, changes, total_entries);

            // Still build full Tree for commit's tree_id (backward compat)
            let root_tree = conn_build_tree(&c)?;

            (delta, root_tree, hook_mgr, files, date)
        }; // c is dropped here

        let tree_id = root_tree.id();
        self.store_object(&tree_id, &root_tree.to_bytes()?)?;

        let parents = if cr > 0 || self.commit_path(cr).exists() {
            self.load_commit(cr).map(|cc| vec![cc.id()]).unwrap_or_default()
        } else { vec![] };
        let commit = Commit::new(tree_id, parents, author.clone(), message.clone(), timestamp, 0);
        self.store_object(&commit.id(), &commit.to_bytes()?)?;
        self.store_commit(nr, &commit)?;

        // Store delta tree
        self.store_delta_tree(nr, &delta)?;

        // Periodic full snapshot
        if nr % SNAPSHOT_INTERVAL == 0 {
            self.store_tree_snapshot(nr, &root_tree)?;
        }

        self.save_head(nr)?;
        *self.current_rev.write().await = nr;

        // Run post-commit hook (fire-and-forget)
        hook_mgr.run_post_commit(nr, &author, &message, &date)?;

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
        let blob = Blob::new(content, executable); let bid = blob.id();
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

    /// Optimized sync commit: stores DeltaTree instead of full tree snapshot.
    /// Only builds full Tree for periodic snapshots (every SNAPSHOT_INTERVAL revisions).
    pub fn commit_sync(&self, author: String, message: String, timestamp: i64) -> Result<u64> {
        // Serialize commits to prevent concurrent revision conflicts
        let _lock = self.commit_lock.blocking_lock();

        let cr = *self.current_rev.blocking_read();
        let nr = cr + 1;
        let parent_rev = if nr > 1 { nr - 1 } else { 0 };
        let c = self.conn();

        // Collect pending changes -> DeltaTree (O(changes) not O(total_files))
        let (changes, total_entries) = conn_collect_pending(&c)?;

        // Run pre-commit hook (can reject the commit)
        let hook_mgr = HookManager::new(self.root.clone());
        let files: Vec<(String, String)> = changes.iter().map(|ch| match ch {
            TreeChange::Upsert { path, .. } => ("A".into(), path.clone()),
            TreeChange::Delete { path } => ("D".into(), path.clone()),
        }).collect();
        let date = chrono::DateTime::from_timestamp(timestamp, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| timestamp.to_string());
        tracing::debug!("Running pre-commit hook (sync) for revision {}", nr);
        hook_mgr.run_pre_commit(nr, &author, &message, &date, &files)?;
        tracing::debug!("Pre-commit hook (sync) passed for revision {}", nr);

        let delta = DeltaTree::new(parent_rev, changes, total_entries);

        // Only build the full tree when we need a periodic snapshot
        let need_snapshot = nr % SNAPSHOT_INTERVAL == 0;

        let tree_id = if need_snapshot {
            let root_tree = conn_build_tree(&c)?;
            let tid = root_tree.id();
            self.store_object(&tid, &root_tree.to_bytes()?)?;
            self.store_tree_snapshot(nr, &root_tree)?;
            tid
        } else {
            // Compute a lightweight tree_id from the delta hash for the commit object.
            // We don't need the actual Tree object unless someone requests it via legacy path.
            delta.id()
        };

        let parents = if cr > 0 || self.commit_path(cr).exists() {
            self.load_commit(cr).map(|cc| vec![cc.id()]).unwrap_or_default()
        } else { vec![] };
        let commit = Commit::new(tree_id, parents, author.clone(), message.clone(), timestamp, 0);
        self.store_object(&commit.id(), &commit.to_bytes()?)?;
        self.store_commit(nr, &commit)?;

        // Store delta tree (always)
        self.store_delta_tree(nr, &delta)?;

        self.save_head(nr)?;
        *self.current_rev.blocking_write() = nr;

        // Run post-commit hook (fire-and-forget)
        hook_mgr.run_post_commit(nr, &author, &message, &date)?;

        Ok(nr)
    }

    /// Get a single commit by revision number
    pub async fn get_commit(&self, rev: u64) -> Option<Commit> {
        self.load_commit(rev).ok()
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

    // ==================== Sync Endpoint Helpers ====================

    /// Load raw object bytes by ObjectId. Returns the on-disk blob bytes.
    pub fn load_object_raw(&self, id: &ObjectId) -> Result<Vec<u8>> {
        self.load_object(id)
    }

    /// Check whether an object exists in the store.
    pub fn has_object(&self, id: &ObjectId) -> bool {
        self.object_path(id).exists()
    }

    /// Load a DeltaTree for a given revision (public accessor).
    pub fn get_delta_tree(&self, rev: u64) -> Result<DeltaTree> {
        self.load_delta_tree(rev)
    }

    /// Load a Commit for a given revision (public sync accessor, synchronous).
    pub fn get_commit_sync(&self, rev: u64) -> Result<Commit> {
        self.load_commit(rev)
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

    #[tokio::test]
    async fn test_delta_tree_reconstruction() {
        let tmp = TempDir::new().unwrap();
        let repo = SqliteRepository::open(tmp.path()).unwrap();
        repo.initialize().await.unwrap();

        // Create 3 commits with different files
        repo.add_file("/a.txt", b"aaa".to_vec(), false).await.unwrap();
        repo.commit("user".into(), "add a".into(), 100).await.unwrap();

        repo.add_file("/b.txt", b"bbb".to_vec(), false).await.unwrap();
        repo.commit("user".into(), "add b".into(), 200).await.unwrap();

        repo.add_file("/c.txt", b"ccc".to_vec(), false).await.unwrap();
        repo.commit("user".into(), "add c".into(), 300).await.unwrap();

        // Check tree at each revision
        let t1 = repo.get_tree_at_rev(1).unwrap();
        assert_eq!(t1.len(), 1);
        assert!(t1.contains_key("a.txt"));

        let t2 = repo.get_tree_at_rev(2).unwrap();
        assert_eq!(t2.len(), 2);
        assert!(t2.contains_key("a.txt"));
        assert!(t2.contains_key("b.txt"));

        let t3 = repo.get_tree_at_rev(3).unwrap();
        assert_eq!(t3.len(), 3);
        assert!(t3.contains_key("a.txt"));
        assert!(t3.contains_key("b.txt"));
        assert!(t3.contains_key("c.txt"));
    }

    #[tokio::test]
    async fn test_delta_tree_with_deletes() {
        let tmp = TempDir::new().unwrap();
        let repo = SqliteRepository::open(tmp.path()).unwrap();
        repo.initialize().await.unwrap();

        repo.add_file("/a.txt", b"aaa".to_vec(), false).await.unwrap();
        repo.add_file("/b.txt", b"bbb".to_vec(), false).await.unwrap();
        repo.commit("user".into(), "add a+b".into(), 100).await.unwrap();

        repo.delete_file("/a.txt").await.unwrap();
        repo.commit("user".into(), "del a".into(), 200).await.unwrap();

        let t1 = repo.get_tree_at_rev(1).unwrap();
        assert_eq!(t1.len(), 2);

        let t2 = repo.get_tree_at_rev(2).unwrap();
        assert_eq!(t2.len(), 1);
        assert!(t2.contains_key("b.txt"));
        assert!(!t2.contains_key("a.txt"));
    }

    #[tokio::test]
    async fn test_delta_tree_batch_commit_sync() {
        let tmp = TempDir::new().unwrap();
        let repo = SqliteRepository::open(tmp.path()).unwrap();
        repo.initialize().await.unwrap();

        // Simulate multi-commit batch import
        repo.begin_batch();
        repo.add_file_sync("file1.txt", b"content1".to_vec(), false).unwrap();
        repo.commit_sync("bot".into(), "commit 1".into(), 100).unwrap();

        repo.add_file_sync("file2.txt", b"content2".to_vec(), false).unwrap();
        repo.commit_sync("bot".into(), "commit 2".into(), 200).unwrap();

        repo.delete_file_sync("file1.txt").unwrap();
        repo.commit_sync("bot".into(), "commit 3".into(), 300).unwrap();
        repo.end_batch();

        // Verify delta reconstruction
        let t1 = repo.get_tree_at_rev(1).unwrap();
        assert_eq!(t1.len(), 1);
        assert!(t1.contains_key("file1.txt"));

        let t2 = repo.get_tree_at_rev(2).unwrap();
        assert_eq!(t2.len(), 2);

        let t3 = repo.get_tree_at_rev(3).unwrap();
        assert_eq!(t3.len(), 1);
        assert!(t3.contains_key("file2.txt"));
        assert!(!t3.contains_key("file1.txt"));

        // Verify no full tree snapshots were written (except rev 0)
        assert!(!repo.has_tree_snapshot(1));
        assert!(!repo.has_tree_snapshot(2));
        assert!(!repo.has_tree_snapshot(3));

        // Verify delta trees exist
        assert!(repo.has_delta_tree(1));
        assert!(repo.has_delta_tree(2));
        assert!(repo.has_delta_tree(3));
    }

    /// Test that concurrent commits produce distinct, sequential revisions
    /// and no data is lost or overwritten.
    #[tokio::test]
    async fn test_sqlite_repo_concurrent_commits() {
        let tmp = TempDir::new().unwrap();
        let repo = Arc::new(SqliteRepository::open(tmp.path()).unwrap());
        repo.initialize().await.unwrap();

        let num_clients = 5;
        let mut handles = Vec::new();

        for i in 0..num_clients {
            let repo_clone = Arc::clone(&repo);
            handles.push(tokio::spawn(async move {
                let path = format!("/client_{}.txt", i);
                let content = format!("data from client {}", i);
                repo_clone.add_file(&path, content.into_bytes(), false).await.unwrap();
                let rev = repo_clone.commit(
                    format!("client_{}", i),
                    format!("commit from client {}", i),
                    1000 + i as i64,
                ).await.unwrap();
                rev
            }));
        }

        let mut revisions = Vec::new();
        for handle in handles {
            revisions.push(handle.await.unwrap());
        }

        // All revisions must be unique
        revisions.sort();
        let unique: std::collections::HashSet<u64> = revisions.iter().copied().collect();
        assert_eq!(unique.len(), num_clients as usize, "All revisions must be unique, got: {:?}", revisions);

        // Revisions must be sequential: 1, 2, 3, 4, 5
        assert_eq!(revisions, vec![1, 2, 3, 4, 5], "Revisions should be sequential");

        // Final rev should be num_clients
        assert_eq!(repo.current_rev().await, num_clients as u64);

        // All client files should be readable at the final revision
        for i in 0..num_clients {
            let path = format!("/client_{}.txt", i);
            let content = repo.get_file(&path, num_clients as u64).await.unwrap();
            assert_eq!(
                content.as_ref(),
                format!("data from client {}", i).as_bytes(),
                "File {} should be readable at final revision",
                path
            );
        }
    }

    // ==================== Hook integration tests ====================

    fn create_hook(repo_path: &Path, name: &str, script: &str) {
        let hooks_dir = repo_path.join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        let hook_path = hooks_dir.join(name);
        std::fs::write(&hook_path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[tokio::test]
    async fn test_pre_commit_hook_rejects_async_commit() {
        let tmp = TempDir::new().unwrap();
        let repo = SqliteRepository::open(tmp.path()).unwrap();
        repo.initialize().await.unwrap();

        // Install a pre-commit hook that rejects
        create_hook(
            tmp.path(),
            "pre-commit",
            "#!/bin/bash\necho 'Blocked by hook' >&2\nexit 1\n",
        );

        repo.add_file("/test.txt", b"hello".to_vec(), false).await.unwrap();
        let result = repo.commit("user".into(), "test".into(), 1000).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Blocked by hook"), "got: {}", err);

        // Revision should still be 0 (commit was rejected)
        assert_eq!(repo.current_rev().await, 0);
    }

    #[tokio::test]
    async fn test_pre_commit_hook_allows_async_commit() {
        let tmp = TempDir::new().unwrap();
        let repo = SqliteRepository::open(tmp.path()).unwrap();
        repo.initialize().await.unwrap();

        create_hook(
            tmp.path(),
            "pre-commit",
            "#!/bin/bash\nexit 0\n",
        );

        repo.add_file("/test.txt", b"hello".to_vec(), false).await.unwrap();
        let rev = repo.commit("user".into(), "test commit".into(), 1000).await.unwrap();
        assert_eq!(rev, 1);
    }

    #[test]
    fn test_pre_commit_hook_rejects_sync_commit() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let repo = SqliteRepository::open(tmp.path()).unwrap();
        rt.block_on(repo.initialize()).unwrap();

        create_hook(
            tmp.path(),
            "pre-commit",
            "#!/bin/bash\necho 'Sync reject' >&2\nexit 1\n",
        );

        repo.add_file_sync("test.txt", b"data".to_vec(), false).unwrap();
        let result = repo.commit_sync("user".into(), "msg".into(), 1000);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Sync reject"));
    }

    #[tokio::test]
    async fn test_post_commit_hook_runs_after_commit() {
        let tmp = TempDir::new().unwrap();
        let repo = SqliteRepository::open(tmp.path()).unwrap();
        repo.initialize().await.unwrap();

        let marker = tmp.path().join("post_commit_ran");
        create_hook(
            tmp.path(),
            "post-commit",
            &format!(
                "#!/bin/bash\ntouch {}\nexit 0\n",
                marker.display()
            ),
        );

        repo.add_file("/test.txt", b"hello".to_vec(), false).await.unwrap();
        repo.commit("user".into(), "test".into(), 1000).await.unwrap();

        assert!(marker.exists(), "post-commit hook should have created marker file");
    }

    #[tokio::test]
    async fn test_no_hook_allows_commit() {
        let tmp = TempDir::new().unwrap();
        let repo = SqliteRepository::open(tmp.path()).unwrap();
        repo.initialize().await.unwrap();

        repo.add_file("/test.txt", b"hello".to_vec(), false).await.unwrap();
        let rev = repo.commit("user".into(), "test".into(), 1000).await.unwrap();
        assert_eq!(rev, 1);
    }
}