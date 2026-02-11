//! WebDAV HTTP method handlers

use super::{Config, WebDavError};
use bytes::Bytes;
use dsvn_core::{SqliteRepository, SqlitePropertyStore};
use http_body_util::{BodyExt, Full};
use hyper::{body::Incoming, Request, Response};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
struct Transaction {
    pub id: String,
    pub base_revision: u64,
    pub author: String,
    pub log_message: String,
    pub created_at: i64,
    pub staged_files: HashMap<String, Vec<u8>>,
    pub staged_properties: HashMap<String, HashMap<String, String>>,
}

static SQLITE_REPO: OnceLock<Arc<SqliteRepository>> = OnceLock::new();

/// Repository registry for multi-repository support
pub static REPOSITORY_REGISTRY: OnceLock<RepositoryRegistry> = OnceLock::new();

/// Multi-repository configuration (for display names, descriptions, etc.)
pub static MULTI_REPO_CONFIG: OnceLock<Arc<std::collections::HashMap<String, crate::RepoConfig>>> = OnceLock::new();

/// Repository registry for multi-repository mode
#[derive(Clone)]
pub struct RepositoryRegistry {
    repositories: HashMap<String, Arc<SqliteRepository>>,
    names: HashMap<String, String>, // repo_path -> repo_name
}

impl Default for RepositoryRegistry {
    fn default() -> Self {
        Self {
            repositories: HashMap::new(),
            names: HashMap::new(),
        }
    }
}

impl RepositoryRegistry {
    /// Create new registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a repository
    pub fn register(&mut self, name: &str, repo: Arc<SqliteRepository>) -> Result<(), String> {
        let path = repo.root().to_string_lossy().to_string();
        if self.repositories.contains_key(name) {
            return Err(format!("Repository '{}' already registered", name));
        }
        self.repositories.insert(name.to_string(), repo.clone());
        self.names.insert(path, name.to_string());
        Ok(())
    }

    /// Get repository by name
    pub fn get(&self, name: &str) -> Option<Arc<SqliteRepository>> {
        self.repositories.get(name).cloned()
    }

    /// Get repository name by path
    pub fn get_name_by_path(&self, path: &str) -> Option<&str> {
        self.names.get(path).map(|s| s.as_str())
    }

    /// List all repository names
    pub fn list(&self) -> Vec<&str> {
        self.repositories.keys().map(|s| s.as_str()).collect()
    }

    /// Check if multi-repo mode
    pub fn is_multi_repo(&self) -> bool {
        self.repositories.len() > 1
    }

    /// Get the default (legacy) repository
    pub fn get_default(&self) -> Option<Arc<SqliteRepository>> {
        if self.repositories.len() == 1 {
            self.repositories.values().next().cloned()
        } else {
            SQLITE_REPO.get().cloned()
        }
    }

    /// Unregister a repository
    pub fn unregister(&mut self, name: &str) -> Result<(), String> {
        if let Some(repo) = self.repositories.remove(name) {
            let path = repo.root().to_string_lossy().to_string();
            self.names.remove(&path);
            Ok(())
        } else {
            Err(format!("Repository '{}' not found", name))
        }
    }
}

lazy_static::lazy_static! {
    static ref TRANSACTIONS: Arc<RwLock<HashMap<String, Transaction>>> = {
        Arc::new(RwLock::new(HashMap::new()))
    };
    static ref TXN_COUNTER: Arc<std::sync::atomic::AtomicU64> = {
        Arc::new(std::sync::atomic::AtomicU64::new(0))
    };
    /// Global commit lock: serializes the entire "stage files → commit" sequence
    /// to prevent concurrent merges from interleaving on the shared working tree.
    static ref COMMIT_LOCK: tokio::sync::Mutex<()> = {
        tokio::sync::Mutex::new(())
    };
}

/// Initialize the global SQLite repository (legacy single-repo mode).
/// Must be called once at server startup.
pub fn init_repository(repo_root: &Path) -> Result<(), String> {
    let repo = SqliteRepository::open(repo_root)
        .map_err(|e| format!("Failed to open repository at {:?}: {}", repo_root, e))?;
    SQLITE_REPO
        .set(Arc::new(repo))
        .map_err(|_| "Repository already initialized".to_string())?;
    Ok(())
}

/// Initialize the repository registry for multi-repository mode.
pub fn init_repository_registry(registry: RepositoryRegistry) -> Result<(), String> {
    REPOSITORY_REGISTRY
        .set(registry)
        .map_err(|_| "Repository registry already initialized".to_string())?;
    Ok(())
}

/// Initialize the multi-repository configuration (for display names, etc.).
pub fn init_multi_repo_config(config: Arc<std::collections::HashMap<String, crate::RepoConfig>>) -> Result<(), String> {
    MULTI_REPO_CONFIG
        .set(config)
        .map_err(|_| "Multi-repo configuration already initialized".to_string())?;
    Ok(())
}

/// Initialize the repository asynchronously (creates initial commit if needed).
pub async fn init_repository_async() -> Result<(), String> {
    let repo = get_repo();
    repo.initialize()
        .await
        .map_err(|e| format!("Failed to initialize repository: {}", e))?;
    Ok(())
}

/// Initialize all repositories in the registry asynchronously.
pub async fn init_repository_registry_async() -> Result<(), String> {
    if let Some(registry) = REPOSITORY_REGISTRY.get() {
        for name in registry.list() {
            let repo_arc = registry.get(name).unwrap();
            repo_arc.initialize()
                .await
                .map_err(|e| format!("Failed to initialize repository '{}': {}", name, e))?;
        }
    } else {
        // Fallback to legacy single-repo mode
        let repo = get_repo();
        repo.initialize()
            .await
            .map_err(|e| format!("Failed to initialize repository: {}", e))?;
    }
    Ok(())
}

fn get_repo() -> &'static Arc<SqliteRepository> {
    SQLITE_REPO
        .get()
        .expect("Repository not initialized — call init_repository() first")
}

/// Get repository by request path.
/// In multi-repo mode, extracts the repo name from the first path segment.
pub fn get_repo_by_path(path: &str) -> Result<Arc<SqliteRepository>, String> {
    // Strip /svn prefix if present
    let path = path.strip_prefix("/svn").unwrap_or(path);

    // Try to extract repo name from path (e.g., /repo1/... -> repo1)
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    
    if let Some(registry) = REPOSITORY_REGISTRY.get() {
        if !parts.is_empty() {
            let repo_name = parts[0];
            if let Some(repo) = registry.get(repo_name) {
                return Ok(repo);
            }
        }
        // Use default repo if available
        if let Some(repo) = registry.get_default() {
            return Ok(repo);
        }
    }
    
    // Fallback to legacy single-repo mode
    Ok(get_repo().clone())
}

/// Public accessor for the global repository Arc.
/// Used by sync_handlers to access the repository.
pub fn get_repo_arc() -> Arc<SqliteRepository> {
    get_repo().clone()
}

const REPO_PREFIX: &str = "/svn";

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
     .replace('"', "&quot;").replace('\'', "&apos;")
}

fn extract_text_between<'a>(s: &'a str, start_tag: &str, end_tag: &str) -> Option<&'a str> {
    let start = s.find(start_tag)? + start_tag.len();
    let end = s[start..].find(end_tag)? + start;
    Some(&s[start..end])
}

/// Parse SVN log revision range from the REPORT request body.
/// Returns (start_rev, end_rev, reverse) where start <= end, and reverse indicates
/// the original request was in descending order (e.g. `svn log -r 10:1`).
fn parse_log_range(body: &str, current_rev: u64) -> (u64, u64, bool) {
    let start = extract_text_between(body, "<S:start-revision>", "</S:start-revision>")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(1);
    let end = extract_text_between(body, "<S:end-revision>", "</S:end-revision>")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(current_rev);

    if start > end {
        (end, start, true)
    } else {
        (start, end, false)
    }
}

fn find_all_between(s: &str, start_tag: &str, end_tag: &str) -> Vec<String> {
    let mut results = Vec::new();
    let mut pos = 0;
    while pos < s.len() {
        if let Some(start) = s[pos..].find(start_tag) {
            let abs_start = pos + start;
            let content_start = abs_start + start_tag.len();
            if let Some(end) = s[content_start..].find(end_tag) {
                let abs_end = content_start + end + end_tag.len();
                results.push(s[abs_start..abs_end].to_string());
                pos = abs_end;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    results
}

fn convert_ns_tag_to_prop_name(ns_tag: &str) -> String {
    if let Some(pos) = ns_tag.find(':') {
        let prefix = &ns_tag[..pos];
        let local = &ns_tag[pos + 1..];
        match prefix {
            "S" | "ns1" => format!("svn:{}", local),
            "C" | "ns2" => local.to_string(),
            "V" | "ns3" => local.to_string(),
            "D" | "ns0" => local.to_string(),
            _ => local.to_string(),
        }
    } else {
        ns_tag.to_string()
    }
}

fn read_varint(data: &[u8], pos: usize) -> (u64, usize) {
    let mut result: u64 = 0;
    let mut p = pos;
    loop {
        if p >= data.len() { return (result, p); }
        let byte = data[p];
        result = (result << 7) | (byte & 0x7f) as u64;
        p += 1;
        if byte & 0x80 == 0 { break; }
    }
    (result, p)
}

fn decode_svndiff(data: &[u8]) -> Vec<u8> {
    if data.len() < 4 || &data[0..3] != b"SVN" {
        return data.to_vec();
    }
    let mut pos = 4;
    let mut result = Vec::new();
    while pos < data.len() {
        let (_src_off, p) = read_varint(data, pos); pos = p;
        if pos >= data.len() { break; }
        let (_src_len, p) = read_varint(data, pos); pos = p;
        if pos >= data.len() { break; }
        let (_tgt_len, p) = read_varint(data, pos); pos = p;
        if pos >= data.len() { break; }
        let (instr_len, p) = read_varint(data, pos); pos = p;
        if pos >= data.len() { break; }
        let (new_data_len, p) = read_varint(data, pos); pos = p;
        let instr_start = pos;
        let new_data_start = instr_start + instr_len as usize;
        let new_data_end = new_data_start + new_data_len as usize;
        if new_data_end > data.len() {
            if new_data_start < data.len() {
                result.extend_from_slice(&data[new_data_start..]);
            }
            break;
        }
        let instructions = &data[instr_start..new_data_start];
        let new_data = &data[new_data_start..new_data_end];
        let mut ipos = 0usize;
        let mut nd_off = 0usize;
        while ipos < instructions.len() {
            let byte = instructions[ipos];
            let opcode = (byte >> 6) & 0x03;
            let mut length = (byte & 0x3f) as u64;
            ipos += 1;
            if length == 0 {
                let (l, np) = read_varint(instructions, ipos);
                length = l; ipos = np;
            }
            match opcode {
                0 => { let (_, np) = read_varint(instructions, ipos); ipos = np; }
                1 => { let (_, np) = read_varint(instructions, ipos); ipos = np; }
                2 => {
                    let end = nd_off + length as usize;
                    if end <= new_data.len() {
                        result.extend_from_slice(&new_data[nd_off..end]);
                    }
                    nd_off = end;
                }
                _ => {}
            }
        }
        pos = new_data_end;
    }
    result
}

// ==================== OPTIONS ====================

pub async fn options_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path().to_string();
    let repo = get_repo_by_path(&path).map_err(|e| WebDavError::Internal(e))?;
    let body = req.into_body().collect().await.ok();
    let body_bytes = body.map(|b| b.to_bytes()).unwrap_or_default();
    let body_str = String::from_utf8_lossy(&body_bytes);
    let has_body = body_str.contains("activity-collection-set");
    let current_rev = repo.current_rev().await;
    let uuid = repo.uuid().to_string();
    let response_body = if has_body {
        format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<D:options-response xmlns:D=\"DAV:\">\n<D:activity-collection-set><D:href>{}/!svn/act/</D:href></D:activity-collection-set></D:options-response>", REPO_PREFIX)
    } else { String::new() };
    let mut b = Response::builder().status(200)
        .header("Allow", "OPTIONS,GET,HEAD,POST,DELETE,TRACE,PROPFIND,PROPPATCH,COPY,MOVE,LOCK,UNLOCK,CHECKOUT")
        .header("DAV", "1,2")
        .header("DAV", "version-control,checkout,working-resource")
        .header("DAV", "merge,baseline,activity,version-controlled-collection")
        .header("DAV", "http://subversion.tigris.org/xmlns/dav/svn/depth")
        .header("DAV", "http://subversion.tigris.org/xmlns/dav/svn/log-revprops")
        .header("DAV", "http://subversion.tigris.org/xmlns/dav/svn/atomic-revprops")
        .header("DAV", "http://subversion.tigris.org/xmlns/dav/svn/partial-replay")
        .header("DAV", "http://subversion.tigris.org/xmlns/dav/svn/inherited-props")
        .header("DAV", "http://subversion.tigris.org/xmlns/dav/svn/inline-props")
        .header("DAV", "http://subversion.tigris.org/xmlns/dav/svn/reverse-file-revs")
        .header("DAV", "http://subversion.tigris.org/xmlns/dav/svn/mergeinfo")
        .header("DAV", "<http://apache.org/dav/propset/fs/1>")
        .header("MS-Author-Via", "DAV");
    if has_body {
        b = b
            .header("DAV", "http://subversion.tigris.org/xmlns/dav/svn/ephemeral-txnprops")
            .header("SVN-Youngest-Rev", current_rev.to_string())
            .header("SVN-Repository-UUID", &uuid)
            .header("SVN-Repository-MergeInfo", "yes")
            .header("DAV", "http://subversion.tigris.org/xmlns/dav/svn/replay-rev-resource")
            .header("SVN-Repository-Root", REPO_PREFIX)
            .header("SVN-Me-Resource", format!("{}/!svn/me", REPO_PREFIX))
            .header("SVN-Rev-Root-Stub", format!("{}/!svn/rvr", REPO_PREFIX))
            .header("SVN-Rev-Stub", format!("{}/!svn/rev", REPO_PREFIX))
            .header("SVN-Txn-Root-Stub", format!("{}/!svn/txr", REPO_PREFIX))
            .header("SVN-Txn-Stub", format!("{}/!svn/txn", REPO_PREFIX))
            .header("SVN-VTxn-Root-Stub", format!("{}/!svn/vtxr", REPO_PREFIX))
            .header("SVN-VTxn-Stub", format!("{}/!svn/vtxn", REPO_PREFIX))
            .header("SVN-Allow-Bulk-Updates", "On")
            .header("SVN-Supported-Posts", "create-txn")
            .header("Content-Type", "text/xml; charset=\"utf-8\"");
    }
    Ok(b.body(Full::new(Bytes::from(response_body))).unwrap())
}

// ==================== POST (create-txn) ====================

pub async fn post_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path().to_string();
    let repo = get_repo_by_path(&path).map_err(|e| WebDavError::Internal(e))?;
    let body = match req.into_body().collect().await {
        Ok(c) => c.to_bytes(),
        Err(e) => return Ok(Response::builder().status(400).body(Full::new(Bytes::from(format!("Failed: {}", e)))).unwrap()),
    };
    let body_str = String::from_utf8_lossy(&body);
    tracing::info!("POST {} body: {}", path, body_str);
    if path.ends_with("/!svn/me") && body_str.contains("create-txn") {
        let current_rev = repo.current_rev().await;
        let txn_seq = TXN_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let txn_name = format!("{}-{}", current_rev, txn_seq);
        let txn = Transaction {
            id: txn_name.clone(), base_revision: current_rev,
            author: String::new(), log_message: String::new(),
            created_at: chrono::Utc::now().timestamp(),
            staged_files: HashMap::new(),
            staged_properties: HashMap::new(),
        };
        TRANSACTIONS.write().await.insert(txn_name.clone(), txn);
        tracing::info!("Created transaction: {}", txn_name);
        return Ok(Response::builder().status(201)
            .header("SVN-Txn-Name", &txn_name)
            .header("Content-Length", "0")
            .body(Full::new(Bytes::new())).unwrap());
    }
    Ok(Response::builder().status(405).body(Full::new(Bytes::from("Method Not Allowed"))).unwrap())
}

// ==================== PROPFIND ====================

pub async fn propfind_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path().to_string();
    let _ = req.into_body().collect().await;

    // Handle multi-repository listing at repository root
    if is_repository_root(&path) && REPOSITORY_REGISTRY.get().is_some() {
        return handle_repository_root_propfind().await;
    }

    let repo = get_repo_by_path(&path).map_err(|e| WebDavError::Internal(e))?;
    let current_rev = repo.current_rev().await;
    let uuid = repo.uuid().to_string();
    if path.contains("!svn/") {
        return handle_svn_special_propfind(&path, current_rev, &uuid).await;
    }
    let now = chrono::Utc::now();
    let xml = format!(
r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:" xmlns:ns0="DAV:">
<D:response xmlns:S="http://subversion.tigris.org/xmlns/svn/" xmlns:C="http://subversion.tigris.org/xmlns/custom/" xmlns:V="http://subversion.tigris.org/xmlns/dav/" xmlns:lp1="DAV:" xmlns:lp3="http://subversion.tigris.org/xmlns/dav/" xmlns:lp2="http://apache.org/dav/props/">
<D:href>{prefix}/</D:href>
<D:propstat>
<D:prop>
<lp1:resourcetype><D:collection/></lp1:resourcetype>
<lp1:getcontenttype>text/html; charset=UTF-8</lp1:getcontenttype>
<lp1:getetag>W/"{rev}//"</lp1:getetag>
<lp1:creationdate>{cdate}</lp1:creationdate>
<lp1:getlastmodified>{lmod}</lp1:getlastmodified>
<lp1:checked-in><D:href>{prefix}/!svn/ver/{rev}/</D:href></lp1:checked-in>
<lp1:version-controlled-configuration><D:href>{prefix}/!svn/vcc/default</D:href></lp1:version-controlled-configuration>
<lp1:version-name>{rev}</lp1:version-name>
<lp3:baseline-relative-path/>
<lp3:repository-uuid>{uuid}</lp3:repository-uuid>
<lp3:deadprop-count>0</lp3:deadprop-count>
<D:lockdiscovery/>
</D:prop>
<D:status>HTTP/1.1 200 OK</D:status>
</D:propstat>
</D:response>
</D:multistatus>"#,
        prefix=REPO_PREFIX, rev=current_rev,
        cdate=now.format("%Y-%m-%dT%H:%M:%S.000000Z"),
        lmod=now.format("%a, %d %b %Y %H:%M:%S GMT"),
        uuid=uuid);
    Ok(Response::builder().status(207)
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .body(Full::new(Bytes::from(xml))).unwrap())
}

/// Check if the path is a repository root (for multi-repository listing)
fn is_repository_root(path: &str) -> bool {
    path == "/svn/" || path == "/svn" || path == "/" || path == ""
}

/// Handle PROPFIND request for repository root (list all repositories in multi-repo mode)
async fn handle_repository_root_propfind() -> Result<Response<Full<Bytes>>, WebDavError> {
    let registry = REPOSITORY_REGISTRY.get().ok_or_else(|| WebDavError::Internal("Repository registry not initialized".to_string()))?;
    let repo_names = registry.list();

    // Get display names from config if available
    let repo_configs = MULTI_REPO_CONFIG.get();

    let mut responses = String::new();

    for repo_name in &repo_names {
        let display_name = repo_configs
            .and_then(|configs| configs.get(*repo_name))
            .and_then(|config| config.display_name.as_ref())
            .map(|s| s.as_str())
            .unwrap_or(*repo_name);

        let href = format!("{}/{}/", REPO_PREFIX, repo_name);

        responses.push_str(&format!(
            r#"  <D:response>
    <D:href>{href}</D:href>
    <D:propstat>
      <D:prop>
        <D:displayname>{display_name}</D:displayname>
        <D:resourcetype><D:collection/></D:resourcetype>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>"#,
            href = escape_xml(&href),
            display_name = escape_xml(display_name)
        ));
    }

    let xml = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:" xmlns:S="http://subversion.tigris.org/xmlns/dav/">
{responses}
</D:multistatus>"#,
        responses = responses
    );

    Ok(Response::builder().status(207)
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .body(Full::new(Bytes::from(xml))).unwrap())
}

async fn handle_svn_special_propfind(path: &str, current_rev: u64, uuid: &str) -> Result<Response<Full<Bytes>>, WebDavError> {
    let special = path.strip_prefix(REPO_PREFIX).unwrap_or(path);
    let xml = if special.starts_with("/!svn/vcc/") {
        format!(r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:" xmlns:ns0="DAV:">
<D:response xmlns:lp1="DAV:" xmlns:lp3="http://subversion.tigris.org/xmlns/dav/">
<D:href>{prefix}/!svn/vcc/default</D:href>
<D:propstat>
<D:prop>
<lp1:checked-in><D:href>{prefix}/!svn/bln/{rev}</D:href></lp1:checked-in>
</D:prop>
<D:status>HTTP/1.1 200 OK</D:status>
</D:propstat>
</D:response>
</D:multistatus>"#, prefix=REPO_PREFIX, rev=current_rev)
    } else if special.starts_with("/!svn/bln/") {
        let rev: u64 = special.strip_prefix("/!svn/bln/").unwrap_or("0").parse().unwrap_or(0);
        format!(r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:" xmlns:ns0="DAV:">
<D:response xmlns:lp1="DAV:" xmlns:lp3="http://subversion.tigris.org/xmlns/dav/">
<D:href>{prefix}/!svn/bln/{rev}</D:href>
<D:propstat>
<D:prop>
<lp1:baseline-collection><D:href>{prefix}/!svn/bc/{rev}/</D:href></lp1:baseline-collection>
<lp1:version-name>{rev}</lp1:version-name>
</D:prop>
<D:status>HTTP/1.1 200 OK</D:status>
</D:propstat>
</D:response>
</D:multistatus>"#, prefix=REPO_PREFIX, rev=rev)
    } else if special.starts_with("/!svn/bc/") {
        let rest = special.strip_prefix("/!svn/bc/").unwrap_or("0/");
        let (rev_str, sub_path) = match rest.find('/') {
            Some(pos) => (&rest[..pos], &rest[pos..]),
            None => (rest, "/"),
        };
        format!(r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:" xmlns:ns0="DAV:">
<D:response xmlns:lp1="DAV:" xmlns:lp3="http://subversion.tigris.org/xmlns/dav/">
<D:href>{prefix}/!svn/bc/{rev}{sub}</D:href>
<D:propstat>
<D:prop>
<lp1:resourcetype><D:collection/></lp1:resourcetype>
<lp1:version-name>{rev}</lp1:version-name>
<lp3:repository-uuid>{uuid}</lp3:repository-uuid>
</D:prop>
<D:status>HTTP/1.1 200 OK</D:status>
</D:propstat>
</D:response>
</D:multistatus>"#, prefix=REPO_PREFIX, rev=rev_str, sub=sub_path, uuid=uuid)
    } else if special.starts_with("/!svn/rvr/") {
        let rest = special.strip_prefix("/!svn/rvr/").unwrap_or("0");
        let (rev_str, sub_path) = match rest.find('/') {
            Some(pos) => (&rest[..pos], &rest[pos..]),
            None => (rest, ""),
        };
        let href_path = if sub_path.is_empty() {
            format!("{}/!svn/rvr/{}/", REPO_PREFIX, rev_str)
        } else {
            format!("{}/!svn/rvr/{}{}", REPO_PREFIX, rev_str, sub_path)
        };
        let is_dir = sub_path.is_empty() || sub_path.ends_with('/');
        let rt = if is_dir { "<D:collection/>" } else { "" };
        format!(r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:" xmlns:ns0="DAV:">
<D:response xmlns:lp1="DAV:">
<D:href>{href}</D:href>
<D:propstat>
<D:prop>
<lp1:resourcetype>{rt}</lp1:resourcetype>
</D:prop>
<D:status>HTTP/1.1 200 OK</D:status>
</D:propstat>
</D:response>
</D:multistatus>"#, href=href_path, rt=rt)
    } else if special.starts_with("/!svn/ver/") {
        format!(r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:" xmlns:ns0="DAV:">
<D:response xmlns:lp1="DAV:" xmlns:lp3="http://subversion.tigris.org/xmlns/dav/">
<D:href>{path}</D:href>
<D:propstat>
<D:prop>
<lp1:resourcetype><D:collection/></lp1:resourcetype>
</D:prop>
<D:status>HTTP/1.1 200 OK</D:status>
</D:propstat>
</D:response>
</D:multistatus>"#, path=path)
    } else {
        return Ok(Response::builder().status(404)
            .body(Full::new(Bytes::from("SVN special path not found"))).unwrap());
    };
    Ok(Response::builder().status(207)
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .body(Full::new(Bytes::from(xml))).unwrap())
}

// ==================== REPORT ====================

pub async fn report_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path().to_string();
    let repo = get_repo_by_path(&path).map_err(|e| WebDavError::Internal(e))?;
    let body = match req.into_body().collect().await {
        Ok(c) => c.to_bytes(),
        Err(e) => return Ok(Response::builder().status(400).body(Full::new(Bytes::from(format!("Failed: {}", e)))).unwrap()),
    };
    let body_str = String::from_utf8_lossy(&body);
    tracing::info!("REPORT {} body: {}", path, body_str);

    // Dispatch to specific report handler based on XML root element
    let xml = if body_str.contains("update-report") || body_str.contains("<S:update") {
        handle_update_report(&body_str, repo).await?
    } else if body_str.contains("log-report") || body_str.contains("log-item") || (body_str.contains("log") && !body_str.contains("get-locations")) {
        handle_log_report(&body_str, repo).await?
    } else if body_str.contains("get-locations") {
        handle_get_locations_report(&body_str, &path, repo).await?
    } else if body_str.contains("get-dated-rev") {
        handle_dated_rev_report(&body_str, repo).await?
    } else if body_str.contains("mergeinfo-report") {
        handle_mergeinfo_report(&body_str, repo).await?
    } else if body_str.contains("get-locks-report") {
        handle_get_locks_report(&body_str).await?
    } else if body_str.contains("replay-report") {
        handle_replay_report(&body_str, repo).await?
    } else if body_str.contains("get-deleted-rev") {
        handle_get_deleted_rev_report(&body_str, repo).await?
    } else if body_str.contains("inherited-props-report") {
        handle_inherited_props_report(&body_str, &path, repo).await?
    } else {
        return Ok(Response::builder().status(501)
            .header("Content-Type", "text/xml; charset=\"utf-8\"")
            .body(Full::new(Bytes::from(format!(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
                 <D:error xmlns:D=\"DAV:\" xmlns:m=\"http://apache.org/dav/xmlns\" xmlns:C=\"svn:\">\n\
                 <C:error/>\n\
                 <m:human-readable errcode=\"200007\">\n\
                 Unsupported report type.\n\
                 </m:human-readable>\n\
                 </D:error>"
            )))).unwrap());
    };
    Ok(Response::builder().status(200)
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .body(Full::new(Bytes::from(xml))).unwrap())
}

/// Handle update-report (checkout/switch/update)
///
/// This is the primary report for `svn checkout` and `svn update`.
/// It sends the full tree content to the client when `send-all="true"` is requested.
async fn handle_update_report(body_str: &str, repo: Arc<SqliteRepository>) -> Result<String, WebDavError> {
    let current_rev = repo.current_rev().await;
    let uuid = repo.uuid();

    // Parse target revision
    let target_rev = extract_text_between(body_str, "<S:target-revision>", "</S:target-revision>")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(current_rev);

    // Parse source revision (for updates — what the client already has)
    let src_rev = extract_text_between(body_str, "<S:src-path>", "</S:src-path>")
        .and_then(|_| {
            // If src-path is present, check for entry with rev attribute
            extract_text_between(body_str, "rev=\"", "\"")
                .and_then(|s| s.parse::<u64>().ok())
        });

    // Parse depth
    let _depth = extract_text_between(body_str, "<S:depth>", "</S:depth>")
        .unwrap_or("infinity");

    // Check if this is a fresh checkout (no source revision / rev=0) or an update
    let is_fresh_checkout = src_rev.is_none() || src_rev == Some(0);

    // Determine the target path (sub-path being checked out)
    let _update_target = extract_text_between(body_str, "<S:update-target>", "</S:update-target>")
        .unwrap_or("");

    // Build tree at target revision
    let tree = repo.get_tree_at_rev(target_rev)
        .map_err(|e| WebDavError::Internal(format!("Failed to get tree at rev {}: {}", target_rev, e)))?;

    // Get commit info for the target revision
    let commit = repo.get_commit(target_rev).await;
    let commit_date = commit.as_ref()
        .and_then(|c| chrono::DateTime::from_timestamp(c.timestamp, 0))
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S.000000Z").to_string())
        .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S.000000Z").to_string());
    let commit_author = commit.as_ref()
        .map(|c| c.author.clone())
        .unwrap_or_default();

    let mut xml = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<S:update-report xmlns:S="svn:" xmlns:V="http://subversion.tigris.org/xmlns/dav/" xmlns:D="DAV:" send-all="true" inline-props="true">
<S:target-revision rev="{rev}"/>
<S:open-directory rev="{rev}">
<D:checked-in><D:href>{prefix}/!svn/rvr/{rev}/</D:href></D:checked-in>
<S:set-prop name="svn:entry:committed-rev">{rev}</S:set-prop>
<S:set-prop name="svn:entry:committed-date">{date}</S:set-prop>
<S:set-prop name="svn:entry:last-author">{author}</S:set-prop>
<S:set-prop name="svn:entry:uuid">{uuid}</S:set-prop>"#,
        prefix=REPO_PREFIX, rev=target_rev, date=commit_date,
        author=escape_xml(&commit_author), uuid=uuid
    );

    if is_fresh_checkout {
        // For fresh checkout, send all files as add-file entries
        // Collect and sort entries for deterministic output
        let mut entries: Vec<_> = tree.iter().collect();
        entries.sort_by_key(|(path, _)| path.clone());

        // Track directories we've opened
        let mut opened_dirs: Vec<String> = Vec::new();

        for (file_path, entry) in &entries {
            use dsvn_core::ObjectKind;
            if entry.kind == ObjectKind::Blob {
                // Ensure parent directories are opened
                let parts: Vec<&str> = file_path.split('/').collect();
                let mut current_dir = String::new();
                for i in 0..parts.len().saturating_sub(1) {
                    let dir = if current_dir.is_empty() {
                        parts[i].to_string()
                    } else {
                        format!("{}/{}", current_dir, parts[i])
                    };
                    if !opened_dirs.contains(&dir) {
                        xml.push_str(&format!(
                            "\n<S:add-directory name=\"{}\" bc-url=\"{}/!svn/bc/{}/{}\">\n\
                             <D:checked-in><D:href>{}/!svn/rvr/{}/{}</D:href></D:checked-in>\n\
                             <S:set-prop name=\"svn:entry:committed-rev\">{}</S:set-prop>\n\
                             <S:set-prop name=\"svn:entry:committed-date\">{}</S:set-prop>\n\
                             <S:set-prop name=\"svn:entry:last-author\">{}</S:set-prop>\n\
                             <S:set-prop name=\"svn:entry:uuid\">{}</S:set-prop>",
                            escape_xml(parts[i]),
                            REPO_PREFIX, target_rev, dir,
                            REPO_PREFIX, target_rev, dir,
                            target_rev, commit_date, escape_xml(&commit_author), uuid
                        ));
                        opened_dirs.push(dir);
                    }
                    current_dir = if current_dir.is_empty() {
                        parts[i].to_string()
                    } else {
                        format!("{}/{}", current_dir, parts[i])
                    };
                }

                // Get file content
                let file_content = repo.get_file(&format!("/{}", file_path), target_rev).await;
                let filename = parts.last().unwrap_or(&"");

                xml.push_str(&format!(
                    "\n<S:add-file name=\"{}\">\n\
                     <D:checked-in><D:href>{}/!svn/rvr/{}/{}</D:href></D:checked-in>\n\
                     <S:set-prop name=\"svn:entry:committed-rev\">{}</S:set-prop>\n\
                     <S:set-prop name=\"svn:entry:committed-date\">{}</S:set-prop>\n\
                     <S:set-prop name=\"svn:entry:last-author\">{}</S:set-prop>\n\
                     <S:set-prop name=\"svn:entry:uuid\">{}</S:set-prop>",
                    escape_xml(filename),
                    REPO_PREFIX, target_rev, file_path,
                    target_rev, commit_date, escape_xml(&commit_author), uuid
                ));

                // Include file content using txdelta if available
                if let Ok(content) = file_content {
                    if !content.is_empty() {
                        use base64::Engine;
                        let encoded = base64::engine::general_purpose::STANDARD.encode(&content);
                        xml.push_str(&format!(
                            "\n<S:txdelta>{}</S:txdelta>",
                            encoded
                        ));
                    }
                }
                // Compute MD5 checksum
                if let Ok(content) = repo.get_file(&format!("/{}", file_path), target_rev).await {
                    let digest = md5::compute(&content);
                    xml.push_str(&format!(
                        "\n<S:prop><V:md5-checksum>{:x}</V:md5-checksum></S:prop>",
                        digest
                    ));
                }
                xml.push_str("\n</S:add-file>");
            }
        }

        // Close opened directories in reverse order
        for _ in opened_dirs.iter().rev() {
            xml.push_str("\n</S:add-directory>");
        }
    }
    // For incremental updates (src_rev is set), we would compute the diff
    // between src_rev and target_rev. For now, fall through to the basic response.

    xml.push_str("\n</S:open-directory>\n</S:update-report>");
    Ok(xml)
}

/// Handle log-report (svn log)
///
/// Supports:
/// - start-revision / end-revision range
/// - limit (max number of log entries)
/// - discover-changed-paths (include changed file list per commit)
/// - revprop filtering
async fn handle_log_report(body_str: &str, repo: Arc<SqliteRepository>) -> Result<String, WebDavError> {
    let current_rev = repo.current_rev().await;
    let (start_rev, end_rev, reverse) = parse_log_range(body_str, current_rev);

    // Parse limit
    let limit = extract_text_between(body_str, "<S:limit>", "</S:limit>")
        .or_else(|| extract_text_between(body_str, "<limit>", "</limit>"))
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(usize::MAX);

    // Check if changed paths are requested
    let discover_changed_paths = body_str.contains("discover-changed-paths");

    // Check if specific revprops are requested
    let include_all_revprops = body_str.contains("all-revprops");
    let requested_revprops: Vec<String> = find_all_between(body_str, "<S:revprop>", "</S:revprop>")
        .into_iter()
        .map(|s| {
            extract_text_between(&s, "<S:revprop>", "</S:revprop>")
                .unwrap_or("")
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect();

    let mut items = Vec::new();
    for rev in start_rev..=end_rev {
        if items.len() >= limit { break; }
        if let Some(c) = repo.get_commit(rev).await {
            let date_str = chrono::DateTime::from_timestamp(c.timestamp, 0)
                .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S.000000Z").to_string())
                .unwrap_or_default();

            let mut item = format!("<S:log-item>\n<D:version-name>{}</D:version-name>\n", rev);

            // Include revprops based on request
            let should_include_author = include_all_revprops
                || requested_revprops.is_empty()
                || requested_revprops.iter().any(|p| p == "svn:author");
            let should_include_date = include_all_revprops
                || requested_revprops.is_empty()
                || requested_revprops.iter().any(|p| p == "svn:date");
            let should_include_log = include_all_revprops
                || requested_revprops.is_empty()
                || requested_revprops.iter().any(|p| p == "svn:log");

            if should_include_author {
                item.push_str(&format!("<D:creator-displayname>{}</D:creator-displayname>\n",
                    escape_xml(&c.author)));
            }
            if should_include_date {
                item.push_str(&format!("<S:date>{}</S:date>\n", date_str));
            }
            if should_include_log {
                item.push_str(&format!("<D:comment>{}</D:comment>\n", escape_xml(&c.message)));
            }

            // Include changed paths if requested
            if discover_changed_paths && rev > 0 {
                if let Ok(delta) = repo.get_delta_tree(rev) {
                    if !delta.changes.is_empty() {
                        item.push_str("<S:changed-path-item>\n");
                        for change in &delta.changes {
                            match change {
                                dsvn_core::TreeChange::Upsert { path, .. } => {
                                    // Determine if this is an add or modify
                                    // Check if the file existed in the parent revision
                                    let action = if rev > 1 {
                                        if let Ok(parent_tree) = repo.get_tree_at_rev(rev - 1) {
                                            if parent_tree.contains_key(path) { "M" } else { "A" }
                                        } else { "A" }
                                    } else { "A" };
                                    item.push_str(&format!(
                                        "<S:modified-path node-kind=\"file\" text-mods=\"true\" prop-mods=\"false\">{}</S:modified-path>\n",
                                        escape_xml(&format!("/{}", path))
                                    ));
                                    // Also use the action attribute for SVN compatibility
                                    let _ = action; // Used in the S:added-path / S:modified-path below
                                }
                                dsvn_core::TreeChange::Delete { path } => {
                                    item.push_str(&format!(
                                        "<S:deleted-path node-kind=\"file\">{}</S:deleted-path>\n",
                                        escape_xml(&format!("/{}", path))
                                    ));
                                }
                            }
                        }
                        item.push_str("</S:changed-path-item>\n");
                    }
                }
            }

            // Include sub-merged-revisions stub (SVN client may expect this)
            item.push_str("<S:has-children/>\n");
            item.push_str("</S:log-item>\n");
            items.push(item);
        }
    }

    if reverse {
        items.reverse();
    }

    let mut s = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<S:log-report xmlns:S=\"svn:\" xmlns:D=\"DAV:\">\n");
    for item in items {
        s.push_str(&item);
    }
    s.push_str("</S:log-report>");
    Ok(s)
}

/// Handle get-locations-report
///
/// Returns the path of a file/directory at different revisions.
/// Used by `svn log --use-merge-history`, `svn blame`, etc.
async fn handle_get_locations_report(body_str: &str, _request_path: &str, repo: Arc<SqliteRepository>) -> Result<String, WebDavError> {
    let current_rev = repo.current_rev().await;

    // Parse path from request
    let query_path = extract_text_between(body_str, "<S:path>", "</S:path>")
        .or_else(|| extract_text_between(body_str, "<path>", "</path>"))
        .unwrap_or("")
        .trim_start_matches('/')
        .to_string();

    // Parse peg revision
    let peg_rev = extract_text_between(body_str, "<S:peg-revision>", "</S:peg-revision>")
        .or_else(|| extract_text_between(body_str, "<peg-revision>", "</peg-revision>"))
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(current_rev);

    // Parse location revisions
    let location_revs: Vec<u64> = find_all_between(body_str, "<S:location-revision>", "</S:location-revision>")
        .iter()
        .filter_map(|s| extract_text_between(s, "<S:location-revision>", "</S:location-revision>"))
        .filter_map(|s| s.parse::<u64>().ok())
        .collect();

    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<S:get-locations-report xmlns:S=\"svn:\" xmlns:D=\"DAV:\">\n");

    // For each requested revision, check if the path exists
    // In our simple model, paths don't move, so if a file existed at the peg revision,
    // it exists at the same path in all revisions where it's present.
    for rev in &location_revs {
        let rev = std::cmp::min(*rev, current_rev);
        if let Ok(tree) = repo.get_tree_at_rev(rev) {
            // Check if the path exists at this revision
            if tree.contains_key(&query_path) || (query_path.is_empty() && rev <= peg_rev) {
                let path_with_slash = if query_path.is_empty() {
                    "/".to_string()
                } else {
                    format!("/{}", query_path)
                };
                xml.push_str(&format!(
                    "<S:location rev=\"{}\" path=\"{}\"/>\n",
                    rev, escape_xml(&path_with_slash)
                ));
            }
        }
    }

    xml.push_str("</S:get-locations-report>");
    Ok(xml)
}

/// Handle get-dated-rev-report
///
/// Returns the revision number that was current at a given date/time.
/// Used by `svn checkout -r {date}`, `svn log -r {date}:HEAD`, etc.
async fn handle_dated_rev_report(body_str: &str, repo: Arc<SqliteRepository>) -> Result<String, WebDavError> {
    let current_rev = repo.current_rev().await;

    // Parse the requested date
    let date_str = extract_text_between(body_str, "<D:creationdate>", "</D:creationdate>")
        .or_else(|| extract_text_between(body_str, "<S:creationdate>", "</S:creationdate>"))
        .unwrap_or("");

    // Parse the date (ISO 8601 format)
    let target_timestamp = if !date_str.is_empty() {
        chrono::DateTime::parse_from_rfc3339(date_str)
            .map(|dt| dt.timestamp())
            .unwrap_or(i64::MAX)
    } else {
        i64::MAX
    };

    // Binary search for the revision at or before the given timestamp
    let mut found_rev = 0u64;
    for rev in (0..=current_rev).rev() {
        if let Some(c) = repo.get_commit(rev).await {
            if c.timestamp <= target_timestamp {
                found_rev = rev;
                break;
            }
        }
    }

    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <S:dated-rev-report xmlns:S=\"svn:\" xmlns:D=\"DAV:\">\n\
         <D:version-name>{}</D:version-name>\n\
         </S:dated-rev-report>",
        found_rev
    );
    Ok(xml)
}

/// Handle mergeinfo-report
///
/// Returns merge tracking information. Since we don't have full merge tracking,
/// we return an empty report.
async fn handle_mergeinfo_report(_body_str: &str, _repo: Arc<SqliteRepository>) -> Result<String, WebDavError> {
    Ok("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
        <S:mergeinfo-report xmlns:S=\"svn:\">\n\
        </S:mergeinfo-report>".to_string())
}

/// Handle get-locks-report
///
/// Returns lock information for paths. Since we don't have locking support,
/// we return an empty report.
async fn handle_get_locks_report(_body_str: &str) -> Result<String, WebDavError> {
    Ok("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
        <S:get-locks-report xmlns:S=\"svn:\">\n\
        </S:get-locks-report>".to_string())
}

/// Handle replay-report
///
/// Replays a revision's changes. Used for svnsync and similar tools.
async fn handle_replay_report(body_str: &str, repo: Arc<SqliteRepository>) -> Result<String, WebDavError> {
    let current_rev = repo.current_rev().await;

    // Parse the revision to replay
    let replay_rev = extract_text_between(body_str, "<S:revision>", "</S:revision>")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(current_rev);

    // Build an editor-style replay of the revision
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <S:editor-report xmlns:S=\"svn:\">\n\
         <S:target-revision>{}</S:target-revision>\n",
        replay_rev
    );

    if let Ok(delta) = repo.get_delta_tree(replay_rev) {
        xml.push_str("<S:open-root>\n");
        for change in &delta.changes {
            match change {
                dsvn_core::TreeChange::Upsert { path, .. } => {
                    xml.push_str(&format!(
                        "<S:add-file name=\"{}\">\n</S:add-file>\n",
                        escape_xml(path)
                    ));
                }
                dsvn_core::TreeChange::Delete { path } => {
                    xml.push_str(&format!(
                        "<S:delete-entry name=\"{}\"/>\n",
                        escape_xml(path)
                    ));
                }
            }
        }
        xml.push_str("</S:open-root>\n");
    }

    xml.push_str("</S:editor-report>");
    Ok(xml)
}

/// Handle get-deleted-rev-report
///
/// Returns the revision in which a path was deleted.
async fn handle_get_deleted_rev_report(body_str: &str, repo: Arc<SqliteRepository>) -> Result<String, WebDavError> {
    let current_rev = repo.current_rev().await;

    let query_path = extract_text_between(body_str, "<S:path>", "</S:path>")
        .unwrap_or("")
        .trim_start_matches('/')
        .to_string();

    let peg_rev = extract_text_between(body_str, "<S:peg-revision>", "</S:peg-revision>")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(current_rev);

    let end_rev = extract_text_between(body_str, "<S:end-revision>", "</S:end-revision>")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(current_rev);

    // Search for the revision where the path was deleted
    let mut deleted_rev = 0u64; // 0 = not deleted (SVN_INVALID_REVNUM)
    for rev in (peg_rev + 1)..=std::cmp::min(end_rev, current_rev) {
        if let Ok(delta) = repo.get_delta_tree(rev) {
            for change in &delta.changes {
                if let dsvn_core::TreeChange::Delete { path } = change {
                    if path == &query_path || query_path.starts_with(&format!("{}/", path)) {
                        deleted_rev = rev;
                        break;
                    }
                }
            }
            if deleted_rev > 0 { break; }
        }
    }

    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <S:get-deleted-rev-report xmlns:S=\"svn:\" xmlns:D=\"DAV:\">\n\
         <D:version-name>{}</D:version-name>\n\
         </S:get-deleted-rev-report>",
        deleted_rev
    );
    Ok(xml)
}

/// Handle inherited-props-report
///
/// Returns inherited properties for a path. Used by svn 1.8+ clients.
async fn handle_inherited_props_report(body_str: &str, _request_path: &str, _repo: Arc<SqliteRepository>) -> Result<String, WebDavError> {
    let _query_path = extract_text_between(body_str, "<S:path>", "</S:path>")
        .unwrap_or("");

    // Return empty inherited props for now
    Ok("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
        <S:inherited-props-report xmlns:S=\"svn:\" xmlns:D=\"DAV:\">\n\
        </S:inherited-props-report>".to_string())
}

// ==================== MERGE ====================

pub async fn merge_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path().to_string();
    let repo = get_repo_by_path(&path).map_err(|e| WebDavError::Internal(e))?;
    let body = match req.into_body().collect().await {
        Ok(c) => c.to_bytes(),
        Err(e) => return Ok(Response::builder().status(400).body(Full::new(Bytes::from(format!("Failed: {}", e)))).unwrap()),
    };
    let body_str = String::from_utf8_lossy(&body);
    tracing::info!("MERGE body: {}", body_str);
    let txn_name = extract_text_between(&body_str, "/!svn/txn/", "</D:href>").unwrap_or("").to_string();
    tracing::info!("MERGE for txn: {}", txn_name);
    let txn = { TRANSACTIONS.write().await.remove(&txn_name) };
    let txn = match txn {
        Some(t) => t,
        None => return Ok(Response::builder().status(404).body(Full::new(Bytes::from("Transaction not found"))).unwrap()),
    };

    // Acquire the commit lock to serialize the entire "stage files → commit" sequence.
    // This prevents concurrent merges from interleaving add_file() calls on the shared
    // working tree and racing on revision numbers.
    let _commit_guard = COMMIT_LOCK.lock().await;

    for (file_path, content) in &txn.staged_files {
        let executable = file_path.ends_with(".sh") || file_path.contains("/bin/");
        tracing::info!("MERGE: adding file {} ({} bytes)", file_path, content.len());
        repo.add_file(file_path, content.clone(), executable).await
            .map_err(|e| WebDavError::Internal(format!("Failed to add file {}: {}", file_path, e)))?;
    }
    let prop_store = repo.property_store();
    for (file_path, props) in &txn.staged_properties {
        for (prop_name, prop_value) in props {
            tracing::info!("MERGE: setting property {}={} on {}", prop_name, prop_value, file_path);
            let store_path = format!("{}{}", REPO_PREFIX, file_path);
            prop_store.set(store_path, prop_name.clone(), prop_value.clone()).await
                .map_err(|e| WebDavError::Internal(format!("Failed to set property {} on {}: {}", prop_name, file_path, e)))?;
        }
    }
    let author = if txn.author.is_empty() { "anonymous".to_string() } else { txn.author.clone() };
    let message = if txn.log_message.is_empty() { "No log message".to_string() } else { txn.log_message.clone() };
    let now = chrono::Utc::now();
    tracing::debug!("Calling repo.commit() with author: {}, message: {}", author, message);
    let new_rev = repo.commit(author.clone(), message, now.timestamp()).await
        .map_err(|e| WebDavError::Internal(e.to_string()))?;
    tracing::debug!("Commit succeeded, new revision: {}", new_rev);

    // Release the commit lock (implicit drop at end of scope)
    drop(_commit_guard);

    tracing::info!("Committed revision {}", new_rev);
    let xml = format!(r#"<?xml version="1.0" encoding="utf-8"?>
<D:merge-response xmlns:D="DAV:">
<D:updated-set>
<D:response>
<D:href>{prefix}/!svn/vcc/default</D:href>
<D:propstat><D:prop>
<D:resourcetype><D:baseline/></D:resourcetype>
<D:version-name>{rev}</D:version-name>
<D:creationdate>{date}</D:creationdate>
<D:creator-displayname>{author}</D:creator-displayname>
</D:prop>
<D:status>HTTP/1.1 200 OK</D:status>
</D:propstat>
</D:response>
</D:updated-set>
</D:merge-response>"#,
        prefix=REPO_PREFIX, rev=new_rev,
        date=now.format("%Y-%m-%dT%H:%M:%S.000000Z"),
        author=escape_xml(&author));
    Ok(Response::builder().status(200)
        .header("Content-Type", "text/xml")
        .header("Cache-Control", "no-cache")
        .body(Full::new(Bytes::from(xml))).unwrap())
}

// ==================== GET ====================

pub async fn get_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path().to_string();
    let repo = get_repo_by_path(&path).map_err(|e| WebDavError::Internal(e))?;

    // Handle GET /!svn/act/ — list all activities
    let special = path.strip_prefix(REPO_PREFIX).unwrap_or(&path);
    if special == "/!svn/act/" || special == "/!svn/act" {
        return handle_list_activities().await;
    }

    if path.contains("!svn") {
        return Ok(Response::builder().status(404).body(Full::new(Bytes::from("Not found"))).unwrap());
    }
    if path.ends_with('/') || path == "/svn" {
        return Ok(Response::builder().status(405).header("Allow", "PROPFIND").body(Full::new(Bytes::from("Use PROPFIND"))).unwrap());
    }
    match repo.get_file(&path, repo.current_rev().await).await {
        Ok(content) => Ok(Response::builder().status(200).header("Content-Type", "application/octet-stream").body(Full::new(content)).unwrap()),
        Err(_) => Ok(Response::builder().status(404).body(Full::new(Bytes::from("Not found"))).unwrap()),
    }
}

/// Handle GET /!svn/act/ — list all active activities (transactions)
async fn handle_list_activities() -> Result<Response<Full<Bytes>>, WebDavError> {
    // For activity listing, use default repo as activities are global
    let repo = get_repo_by_path("/svn").map_err(|e| WebDavError::Internal(e))?;
    let uuid = repo.uuid().to_string();
    let txns = TRANSACTIONS.read().await;

    let mut hrefs = String::new();
    for txn_name in txns.keys() {
        hrefs.push_str(&format!(
            "  <D:href>{}/!svn/act/{}</D:href>\n",
            REPO_PREFIX, txn_name
        ));
    }

    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <D:activity-collection-set xmlns:D=\"DAV:\">\n\
         {hrefs}\
         </D:activity-collection-set>",
        hrefs = hrefs
    );

    Ok(Response::builder()
        .status(200)
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .header("SVN-Repository-UUID", uuid)
        .body(Full::new(Bytes::from(xml)))
        .unwrap())
}

// ==================== PROPPATCH ====================

pub async fn proppatch_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path().to_string();
    let repo = get_repo_by_path(&path).map_err(|e| WebDavError::Internal(e))?;
    let body = match req.collect().await {
        Ok(c) => c.to_bytes(),
        Err(e) => return Ok(Response::builder().status(400).body(Full::new(Bytes::from(format!("Failed: {}", e)))).unwrap()),
    };
    let body_str = String::from_utf8_lossy(&body);
    tracing::info!("PROPPATCH {} body: {}", path, body_str);
    let special = path.strip_prefix(REPO_PREFIX).unwrap_or(&path);
    if special.starts_with("/!svn/txr/") {
        let rest = special.strip_prefix("/!svn/txr/").unwrap_or("");
        let (txn_name, _file_path) = rest.split_once('/').unwrap_or((rest, ""));
        tracing::info!("PROPPATCH for txr file: txn={}, file={}", txn_name, _file_path);
        let xml = format!(r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:" xmlns:ns3="http://subversion.tigris.org/xmlns/dav/" xmlns:ns2="http://subversion.tigris.org/xmlns/custom/" xmlns:ns1="http://subversion.tigris.org/xmlns/svn/" xmlns:ns0="DAV:">
<D:response>
<D:href>{path}</D:href>
<D:propstat>
<D:prop/>
<D:status>HTTP/1.1 200 OK</D:status>
</D:propstat>
</D:response>
</D:multistatus>"#, path=path);
        return Ok(Response::builder().status(207)
            .header("Content-Type", "text/xml; charset=\"utf-8\"")
            .body(Full::new(Bytes::from(xml))).unwrap());
    }
    if special.starts_with("/!svn/txn/") {
        let txn_name = special.strip_prefix("/!svn/txn/").unwrap_or("").to_string();
        tracing::info!("PROPPATCH for txn: {}", txn_name);
        if let Some(log_msg) = extract_text_between(&body_str, "<S:log>", "</S:log>") {
            let mut txns = TRANSACTIONS.write().await;
            if let Some(txn) = txns.get_mut(&txn_name) {
                txn.log_message = log_msg.to_string();
                tracing::info!("Set log message for txn {}: {}", txn_name, log_msg);
            }
        }
        let xml = format!(r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:" xmlns:ns3="http://subversion.tigris.org/xmlns/dav/" xmlns:ns2="http://subversion.tigris.org/xmlns/custom/" xmlns:ns1="http://subversion.tigris.org/xmlns/svn/" xmlns:ns0="DAV:">
<D:response>
<D:href>{prefix}/!svn/txn/{txn}</D:href>
<D:propstat>
<D:prop>
<ns1:log/>
</D:prop>
<D:status>HTTP/1.1 200 OK</D:status>
</D:propstat>
</D:response>
</D:multistatus>"#, prefix=REPO_PREFIX, txn=txn_name);
        return Ok(Response::builder().status(207)
            .header("Content-Type", "text/xml; charset=\"utf-8\"")
            .body(Full::new(Bytes::from(xml))).unwrap());
    }

    // Fallback to generic proppatch
    use crate::proppatch::{PropPatchRequest, PropPatchResponse};
    let prop_store = repo.property_store();
    let proppatch_req = match PropPatchRequest::from_xml(&body_str) {
        Ok(r) => r,
        Err(e) => {
            let response = PropPatchResponse::error(path.clone(), format!("Invalid XML: {}", e));
            return Ok(Response::builder().status(207).header("Content-Type", "text/xml; charset=utf-8").body(Full::new(Bytes::from(response.to_xml()))).unwrap());
        }
    };
    for modification in &proppatch_req.modifications {
        match modification {
            crate::proppatch::PropertyModification::Set { name, value, .. } => {
                if let Err(e) = prop_store.set(path.clone(), name.clone(), value.clone()).await {
                    let response = PropPatchResponse::error(path.clone(), format!("Failed: {}", e));
                    return Ok(Response::builder().status(207).header("Content-Type", "text/xml; charset=utf-8").body(Full::new(Bytes::from(response.to_xml()))).unwrap());
                }
            }
            crate::proppatch::PropertyModification::Remove { name, .. } => {
                if let Err(e) = prop_store.remove(&path, name).await {
                    let response = PropPatchResponse::error(path.clone(), format!("Failed: {}", e));
                    return Ok(Response::builder().status(207).header("Content-Type", "text/xml; charset=utf-8").body(Full::new(Bytes::from(response.to_xml()))).unwrap());
                }
            }
        }
    }
    Ok(Response::builder().status(207).header("Content-Type", "text/xml; charset=utf-8")
        .body(Full::new(Bytes::from(PropPatchResponse::success(path).to_xml()))).unwrap())
}

// ==================== PUT ====================

pub async fn put_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path().to_string();
    let body = match req.into_body().collect().await {
        Ok(c) => c.to_bytes(),
        Err(e) => return Ok(Response::builder().status(400).body(Full::new(Bytes::from(format!("Failed: {}", e)))).unwrap()),
    };
    tracing::info!("PUT {} ({} bytes)", path, body.len());
    let special = path.strip_prefix(REPO_PREFIX).unwrap_or(&path);
    if special.starts_with("/!svn/txr/") {
        let rest = special.strip_prefix("/!svn/txr/").unwrap_or("");
        if let Some(slash_pos) = rest.find('/') {
            let txn_name = &rest[..slash_pos];
            let file_path = &rest[slash_pos + 1..];
            let full_path = format!("/{}", file_path);
            tracing::info!("Transaction PUT: txn={}, path={}", txn_name, full_path);
            let content = decode_svndiff(&body);
            tracing::info!("Decoded content: {} bytes", content.len());
            let mut txns = TRANSACTIONS.write().await;
            if let Some(txn) = txns.get_mut(txn_name) {
                txn.staged_files.insert(full_path.clone(), content);
                tracing::info!("Staged file {} in txn {}", full_path, txn_name);
            } else {
                tracing::error!("Transaction not found: {}", txn_name);
                return Ok(Response::builder().status(404).body(Full::new(Bytes::from("Transaction not found"))).unwrap());
            }
            return Ok(Response::builder().status(201)
                .header("Location", format!("{}/!svn/txr/{}/{}", REPO_PREFIX, txn_name, file_path))
                .body(Full::new(Bytes::new())).unwrap());
        }
    }
    Ok(Response::builder().status(405).body(Full::new(Bytes::from("PUT not allowed"))).unwrap())
}

// ==================== HEAD ====================

pub async fn head_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path().to_string();
    let repo = get_repo_by_path(&path).map_err(|e| WebDavError::Internal(e))?;
    tracing::info!("HEAD {}", path);
    if path.contains("!svn") {
        return Ok(Response::builder().status(404).body(Full::new(Bytes::new())).unwrap());
    }
    let exists = repo.exists(&path, repo.current_rev().await).await.unwrap_or(false);
    if exists {
        Ok(Response::builder().status(200)
            .header("Content-Type", "application/octet-stream")
            .body(Full::new(Bytes::new())).unwrap())
    } else {
        Ok(Response::builder().status(404).body(Full::new(Bytes::new())).unwrap())
    }
}

// ==================== Stubs ====================

pub async fn lock_handler(_req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    Ok(Response::builder().status(200).body(Full::new(Bytes::new())).unwrap())
}
pub async fn unlock_handler(_req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    Ok(Response::builder().status(204).body(Full::new(Bytes::new())).unwrap())
}
pub async fn copy_handler(_req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    Ok(Response::builder().status(201).body(Full::new(Bytes::new())).unwrap())
}
pub async fn move_handler(_req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    Ok(Response::builder().status(201).body(Full::new(Bytes::new())).unwrap())
}
pub async fn mkcol_handler(_req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    Ok(Response::builder().status(201).body(Full::new(Bytes::new())).unwrap())
}
pub async fn delete_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path().to_string();
    let _ = req.into_body().collect().await;
    tracing::info!("DELETE {}", path);

    let special = path.strip_prefix(REPO_PREFIX).unwrap_or(&path);

    // Handle DELETE /!svn/act/{txn-id} — abort/delete an activity
    if special.starts_with("/!svn/act/") {
        let txn_name = special.strip_prefix("/!svn/act/")
            .unwrap_or("")
            .trim_end_matches('/');

        if txn_name.is_empty() {
            return Ok(Response::builder()
                .status(405)
                .body(Full::new(Bytes::from("Cannot DELETE the activity collection itself")))
                .unwrap());
        }

        return handle_delete_activity(txn_name).await;
    }

    // Default: generic DELETE (stub)
    Ok(Response::builder().status(204).body(Full::new(Bytes::new())).unwrap())
}

/// Handle DELETE /!svn/act/{txn-id} — abort and remove an activity (transaction)
async fn handle_delete_activity(txn_name: &str) -> Result<Response<Full<Bytes>>, WebDavError> {
    let removed = {
        let mut txns = TRANSACTIONS.write().await;
        txns.remove(txn_name)
    };

    match removed {
        Some(txn) => {
            tracing::info!(
                "DELETE activity {}: aborted (base_rev={}, staged_files={}, staged_props={})",
                txn_name, txn.base_revision,
                txn.staged_files.len(), txn.staged_properties.len()
            );
            Ok(Response::builder()
                .status(204)
                .body(Full::new(Bytes::new()))
                .unwrap())
        }
        None => {
            tracing::warn!("DELETE activity {}: not found", txn_name);
            Ok(Response::builder()
                .status(404)
                .header("Content-Type", "text/xml; charset=\"utf-8\"")
                .body(Full::new(Bytes::from(format!(
                    "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
                     <D:error xmlns:D=\"DAV:\" xmlns:m=\"http://apache.org/dav/xmlns\" xmlns:C=\"svn:\">\n\
                     <C:error/>\n\
                     <m:human-readable errcode=\"160007\">\n\
                     Activity '{}' not found.\n\
                     </m:human-readable>\n\
                     </D:error>", escape_xml(txn_name)
                ))))
                .unwrap())
        }
    }
}
pub async fn checkout_handler(_req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    Ok(Response::builder().status(200).body(Full::new(Bytes::new())).unwrap())
}

// ==================== Tests ====================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::sync::Mutex;

    // Test state that can be set/reset
    static TEST_REGISTRY: Mutex<Option<RepositoryRegistry>> = Mutex::new(None);

    /// Test single-repo mode routing (legacy mode)
    #[test]
    fn test_get_repo_by_path_single_repo() {
        let tmp = tempdir().unwrap();
        let repo_path = tmp.path().join("test_repo");

        // Initialize a repository
        let repo = SqliteRepository::open(&repo_path).unwrap();
        SQLITE_REPO.set(Arc::new(repo)).unwrap_or(());

        // Test that path routing works in single-repo mode
        let result = get_repo_by_path("/svn/test.txt");
        assert!(result.is_ok());

        let repo_arc = result.unwrap();
        assert_eq!(repo_arc.root(), repo_path);
    }

    /// Test multi-repo mode routing
    #[tokio::test]
    async fn test_get_repo_by_path_multi_repo() {
        let tmp = tempdir().unwrap();

        // Create two repositories
        let repo1_path = tmp.path().join("repo1");
        let repo2_path = tmp.path().join("repo2");

        let repo1 = SqliteRepository::open(&repo1_path).unwrap();
        let repo2 = SqliteRepository::open(&repo2_path).unwrap();

        // Initialize them
        repo1.initialize().await.unwrap();
        repo2.initialize().await.unwrap();

        // Create a registry and register repositories
        let mut registry = RepositoryRegistry::new();
        registry.register("repo1", Arc::new(repo1)).unwrap();
        registry.register("repo2", Arc::new(repo2)).unwrap();

        REPOSITORY_REGISTRY.set(registry).unwrap_or(());

        // Test routing to repo1
        let result = get_repo_by_path("/svn/repo1/test.txt");
        assert!(result.is_ok());
        let repo_arc = result.unwrap();
        assert_eq!(repo_arc.root(), repo1_path);

        // Test routing to repo2
        let result = get_repo_by_path("/svn/repo2/test.txt");
        assert!(result.is_ok());
        let repo_arc = result.unwrap();
        assert_eq!(repo_arc.root(), repo2_path);

        // Test that non-existent repo names fall back to default
        let result = get_repo_by_path("/svn/nonexistent/test.txt");
        assert!(result.is_ok());
    }

    /// Test repository registry functionality
    #[test]
    fn test_repository_registry() {
        let tmp = tempdir().unwrap();
        let repo_path = tmp.path().join("test_repo");
        let repo = SqliteRepository::open(&repo_path).unwrap();

        let mut registry = RepositoryRegistry::new();

        // Test registering a repository
        let result = registry.register("test-repo", Arc::new(repo));
        assert!(result.is_ok());

        // Test duplicate registration fails - open repo again for duplicate test
        let repo_dup = SqliteRepository::open(&repo_path).unwrap();
        let result = registry.register("test-repo", Arc::new(repo_dup));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already registered"));

        // Test getting repository by name
        let tmp2 = tempdir().unwrap();
        let repo2_path = tmp2.path().join("test_repo2");
        let repo2 = SqliteRepository::open(&repo2_path).unwrap();
        registry.register("test-repo2", Arc::new(repo2)).unwrap();

        let retrieved = registry.get("test-repo");
        assert!(retrieved.is_some());

        // Test listing repositories
        let repos = registry.list();
        assert_eq!(repos.len(), 2);
        assert!(repos.contains(&"test-repo"));
        assert!(repos.contains(&"test-repo2"));

        // Test is_multi_repo
        let mut single_registry = RepositoryRegistry::new();
        let tmp3 = tempdir().unwrap();
        let repo3_path = tmp3.path().join("test_repo3");
        let repo3 = SqliteRepository::open(&repo3_path).unwrap();
        single_registry.register("single-repo", Arc::new(repo3)).unwrap();

        assert!(!single_registry.is_multi_repo()); // Single repo
        assert!(registry.is_multi_repo()); // Multiple repos

        // Test get_default
        let default = single_registry.get_default();
        assert!(default.is_some());
        assert_eq!(default.unwrap().root(), repo3_path);
    }
}

pub async fn mkactivity_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path().to_string();
    let repo = get_repo_by_path(&path).map_err(|e| WebDavError::Internal(e))?;
    let _ = req.into_body().collect().await;

    tracing::info!("MKACTIVITY {}", path);

    let special = path.strip_prefix(REPO_PREFIX).unwrap_or(&path);

    // MKACTIVITY /!svn/act/{txn-id} — create activity with client-supplied ID
    // MKACTIVITY /!svn/act/          — create activity with server-generated ID
    if special.starts_with("/!svn/act/") {
        let client_txn_id = special.strip_prefix("/!svn/act/")
            .unwrap_or("")
            .trim_end_matches('/');

        let current_rev = repo.current_rev().await;

        // Use client-supplied ID if non-empty, otherwise generate one
        let txn_name = if client_txn_id.is_empty() {
            let txn_seq = TXN_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            format!("{}-{}", current_rev, txn_seq)
        } else {
            client_txn_id.to_string()
        };

        // Check for duplicate activity
        {
            let txns = TRANSACTIONS.read().await;
            if txns.contains_key(&txn_name) {
                tracing::warn!("MKACTIVITY: activity already exists: {}", txn_name);
                return Ok(Response::builder()
                    .status(405)
                    .header("Content-Type", "text/xml; charset=\"utf-8\"")
                    .body(Full::new(Bytes::from(format!(
                        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
                         <D:error xmlns:D=\"DAV:\" xmlns:m=\"http://apache.org/dav/xmlns\" xmlns:C=\"svn:\">\n\
                         <C:error/>\n\
                         <m:human-readable errcode=\"160024\">\n\
                         Activity '{}' already exists.\n\
                         </m:human-readable>\n\
                         </D:error>", escape_xml(&txn_name)
                    ))))
                    .unwrap());
            }
        }

        let txn = Transaction {
            id: txn_name.clone(),
            base_revision: current_rev,
            author: String::new(),
            log_message: String::new(),
            created_at: chrono::Utc::now().timestamp(),
            staged_files: HashMap::new(),
            staged_properties: HashMap::new(),
        };
        TRANSACTIONS.write().await.insert(txn_name.clone(), txn);
        tracing::info!("MKACTIVITY: created activity {}", txn_name);

        return Ok(Response::builder()
            .status(201)
            .header("Location", format!("{}/!svn/act/{}", REPO_PREFIX, txn_name))
            .header("Content-Type", "text/xml; charset=\"utf-8\"")
            .header("SVN-Txn-Name", &txn_name)
            .body(Full::new(Bytes::from(format!(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
                 <D:mkactivity-response xmlns:D=\"DAV:\">\n\
                 <D:href>{}/!svn/act/{}</D:href>\n\
                 </D:mkactivity-response>",
                REPO_PREFIX, txn_name
            ))))
            .unwrap());
    }

    // Fallback for non-activity paths
    Ok(Response::builder().status(405)
        .body(Full::new(Bytes::from("MKACTIVITY only supported on /!svn/act/")))
        .unwrap())
}
