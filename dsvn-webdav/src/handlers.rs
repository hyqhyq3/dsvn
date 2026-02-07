//! WebDAV HTTP method handlers

use super::{Config, WebDavError};
use bytes::Bytes;
use dsvn_core::{Repository, properties::PropertyStore};
use http_body_util::{BodyExt, Full};
use hyper::{body::Incoming, Request, Response};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
struct Transaction {
    pub id: String,
    pub base_revision: u64,
    pub author: String,
    pub log_message: String,
    pub created_at: i64,
    pub staged_files: HashMap<String, Vec<u8>>,
}

lazy_static::lazy_static! {
    static ref REPOSITORY: Arc<Repository> = {
        let repo = Repository::new();
        let rt = tokio::runtime::Handle::try_current();
        if let Ok(handle) = rt {
            tokio::task::block_in_place(|| {
                handle.block_on(repo.initialize())
            }).expect("Failed to initialize repository");
        }
        Arc::new(repo)
    };
    static ref TRANSACTIONS: Arc<RwLock<HashMap<String, Transaction>>> = {
        Arc::new(RwLock::new(HashMap::new()))
    };
    static ref PROPERTY_STORE: Arc<PropertyStore> = {
        Arc::new(PropertyStore::new())
    };
    static ref TXN_COUNTER: Arc<std::sync::atomic::AtomicU64> = {
        Arc::new(std::sync::atomic::AtomicU64::new(0))
    };
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
    let body = req.into_body().collect().await.ok();
    let body_bytes = body.map(|b| b.to_bytes()).unwrap_or_default();
    let body_str = String::from_utf8_lossy(&body_bytes);
    let has_body = body_str.contains("activity-collection-set");
    let current_rev = REPOSITORY.current_rev().await;
    let uuid = REPOSITORY.uuid().to_string();
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
    let body = match req.into_body().collect().await {
        Ok(c) => c.to_bytes(),
        Err(e) => return Ok(Response::builder().status(400).body(Full::new(Bytes::from(format!("Failed: {}", e)))).unwrap()),
    };
    let body_str = String::from_utf8_lossy(&body);
    tracing::info!("POST {} body: {}", path, body_str);
    if path.ends_with("/!svn/me") && body_str.contains("create-txn") {
        let current_rev = REPOSITORY.current_rev().await;
        let txn_seq = TXN_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let txn_name = format!("{}-{}", current_rev, txn_seq);
        let txn = Transaction {
            id: txn_name.clone(), base_revision: current_rev,
            author: String::new(), log_message: String::new(),
            created_at: chrono::Utc::now().timestamp(),
            staged_files: HashMap::new(),
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
    let current_rev = REPOSITORY.current_rev().await;
    let uuid = REPOSITORY.uuid().to_string();
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
    let body = match req.into_body().collect().await {
        Ok(c) => c.to_bytes(),
        Err(e) => return Ok(Response::builder().status(400).body(Full::new(Bytes::from(format!("Failed: {}", e)))).unwrap()),
    };
    let body_str = String::from_utf8_lossy(&body);
    tracing::info!("REPORT body: {}", body_str);
    let xml = if body_str.contains("update-report") || body_str.contains("<S:update") {
        let current_rev = REPOSITORY.current_rev().await;
        let uuid = REPOSITORY.uuid();
        let now = chrono::Utc::now();
        let target_rev = extract_text_between(&body_str, "<S:target-revision>", "</S:target-revision>")
            .and_then(|s| s.parse::<u64>().ok()).unwrap_or(current_rev);
        format!(r#"<?xml version="1.0" encoding="utf-8"?>
<S:update-report xmlns:S="svn:" xmlns:V="http://subversion.tigris.org/xmlns/dav/" xmlns:D="DAV:" send-all="true" inline-props="true">
<S:target-revision rev="{rev}"/>
<S:open-directory rev="{rev}">
<D:checked-in><D:href>{prefix}/!svn/rvr/{rev}/</D:href></D:checked-in>
<S:set-prop name="svn:entry:committed-rev">{rev}</S:set-prop>
<S:set-prop name="svn:entry:committed-date">{date}</S:set-prop>
<S:remove-prop name="svn:entry:last-author"/>
<S:set-prop name="svn:entry:uuid">{uuid}</S:set-prop>
</S:open-directory>
</S:update-report>"#,
            prefix=REPO_PREFIX, rev=target_rev,
            date=now.format("%Y-%m-%dT%H:%M:%S.000000Z"), uuid=uuid)
    } else if body_str.contains("log") {
        let current_rev = REPOSITORY.current_rev().await;
        let commits = REPOSITORY.log(current_rev, 100).await.unwrap_or_default();
        let mut s = String::from(r#"<?xml version="1.0" encoding="utf-8"?><S:log-report xmlns:S="svn:" xmlns:D="DAV:">"#);
        for (i, c) in commits.iter().enumerate() {
            let rev = current_rev - i as u64;
            s.push_str(&format!(
                r#"<S:log-item><D:version-name>{}</D:version-name><D:creator-displayname>{}</D:creator-displayname><S:date>{}</S:date><D:comment>{}</D:comment></S:log-item>"#,
                rev, escape_xml(&c.author),
                chrono::DateTime::from_timestamp(c.timestamp, 0)
                    .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S.000000Z").to_string())
                    .unwrap_or_default(),
                escape_xml(&c.message)));
        }
        s.push_str("</S:log-report>");
        s
    } else {
        return Ok(Response::builder().status(400).body(Full::new(Bytes::from("Unknown report"))).unwrap());
    };
    Ok(Response::builder().status(200)
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .body(Full::new(Bytes::from(xml))).unwrap())
}

// ==================== MERGE ====================

pub async fn merge_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
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
    for (file_path, content) in &txn.staged_files {
        let executable = file_path.ends_with(".sh") || file_path.contains("/bin/");
        tracing::info!("MERGE: adding file {} ({} bytes)", file_path, content.len());
        REPOSITORY.add_file(file_path, content.clone(), executable).await
            .map_err(|e| WebDavError::Internal(format!("Failed to add file {}: {}", file_path, e)))?;
    }
    let author = if txn.author.is_empty() { "anonymous".to_string() } else { txn.author.clone() };
    let message = if txn.log_message.is_empty() { "No log message".to_string() } else { txn.log_message.clone() };
    let now = chrono::Utc::now();
    let new_rev = REPOSITORY.commit(author.clone(), message, now.timestamp()).await
        .map_err(|e| WebDavError::Internal(e.to_string()))?;
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
    let path = req.uri().path();
    if path.contains("!svn") {
        return Ok(Response::builder().status(404).body(Full::new(Bytes::from("Not found"))).unwrap());
    }
    if path.ends_with('/') || path == "/svn" {
        return Ok(Response::builder().status(405).header("Allow", "PROPFIND").body(Full::new(Bytes::from("Use PROPFIND"))).unwrap());
    }
    match REPOSITORY.get_file(path, REPOSITORY.current_rev().await).await {
        Ok(content) => Ok(Response::builder().status(200).header("Content-Type", "application/octet-stream").body(Full::new(content)).unwrap()),
        Err(_) => Ok(Response::builder().status(404).body(Full::new(Bytes::from("Not found"))).unwrap()),
    }
}

// ==================== PROPPATCH ====================

pub async fn proppatch_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path().to_string();
    let body = match req.collect().await {
        Ok(c) => c.to_bytes(),
        Err(e) => return Ok(Response::builder().status(400).body(Full::new(Bytes::from(format!("Failed: {}", e)))).unwrap()),
    };
    let body_str = String::from_utf8_lossy(&body);
    tracing::info!("PROPPATCH {} body: {}", path, body_str);
    let special = path.strip_prefix(REPO_PREFIX).unwrap_or(&path);
    if special.starts_with("/!svn/txn/") {
        let txn_name = special.strip_prefix("/!svn/txn/").unwrap_or("").to_string();
        tracing::info!("PROPPATCH for txn: {}", txn_name);
        // Extract log message from body: <S:log>...</S:log>
        if let Some(log_msg) = extract_text_between(&body_str, "<S:log>", "</S:log>") {
            let mut txns = TRANSACTIONS.write().await;
            if let Some(txn) = txns.get_mut(&txn_name) {
                txn.log_message = log_msg.to_string();
                tracing::info!("Set log message for txn {}: {}", txn_name, log_msg);
            }
        }
        // Return success multistatus matching official server
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
                if let Err(e) = PROPERTY_STORE.set(path.clone(), name.clone(), value.clone()).await {
                    let response = PropPatchResponse::error(path.clone(), format!("Failed: {}", e));
                    return Ok(Response::builder().status(207).header("Content-Type", "text/xml; charset=utf-8").body(Full::new(Bytes::from(response.to_xml()))).unwrap());
                }
            }
            crate::proppatch::PropertyModification::Remove { name, .. } => {
                if let Err(e) = PROPERTY_STORE.remove(&path, name).await {
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
    // Check if this is a transaction PUT: /!svn/txr/<txn>/<path>
    let special = path.strip_prefix(REPO_PREFIX).unwrap_or(&path);
    if special.starts_with("/!svn/txr/") {
        let rest = special.strip_prefix("/!svn/txr/").unwrap_or("");
        // Parse txn name and file path
        if let Some(slash_pos) = rest.find('/') {
            let txn_name = &rest[..slash_pos];
            let file_path = &rest[slash_pos + 1..];
            let full_path = format!("/{}", file_path);
            tracing::info!("Transaction PUT: txn={}, path={}", txn_name, full_path);
            // Decode svndiff content
            let content = decode_svndiff(&body);
            tracing::info!("Decoded content: {} bytes", content.len());
            // Store in transaction
            let mut txns = TRANSACTIONS.write().await;
            if let Some(txn) = txns.get_mut(txn_name) {
                txn.staged_files.insert(full_path.clone(), content);
                tracing::info!("Staged file {} in txn {}", full_path, txn_name);
            } else {
                tracing::error!("Transaction not found: {}", txn_name);
                return Ok(Response::builder().status(404).body(Full::new(Bytes::from("Transaction not found"))).unwrap());
            }
            // Return 201 Created with Location header
            return Ok(Response::builder().status(201)
                .header("Location", format!("{}/!svn/txr/{}/{}", REPO_PREFIX, txn_name, file_path))
                .body(Full::new(Bytes::new())).unwrap());
        }
    }
    // Regular PUT (not implemented for MVP)
    Ok(Response::builder().status(405).body(Full::new(Bytes::from("PUT not allowed"))).unwrap())
}

// ==================== HEAD ====================

pub async fn head_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path();
    tracing::info!("HEAD {}", path);
    // For HEAD requests during commit, we need to return 404 for non-existent files
    // The SVN client checks if file exists before PUT
    if path.contains("!svn") {
        return Ok(Response::builder().status(404).body(Full::new(Bytes::new())).unwrap());
    }
    // Check if file exists in repository
    let exists = REPOSITORY.exists(path, REPOSITORY.current_rev().await).await.unwrap_or(false);
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
pub async fn delete_handler(_req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    Ok(Response::builder().status(204).body(Full::new(Bytes::new())).unwrap())
}
pub async fn checkout_handler(_req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    Ok(Response::builder().status(200).body(Full::new(Bytes::new())).unwrap())
}
pub async fn mkactivity_handler(_req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    Ok(Response::builder().status(201).body(Full::new(Bytes::new())).unwrap())
}
