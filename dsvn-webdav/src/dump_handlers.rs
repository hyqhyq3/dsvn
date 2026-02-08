//! SVN dump/load HTTP handlers
//!
//! Implements svnrdump-compatible protocol endpoints:
//! - GET  with Accept: application/vnd.svn-dumpfile -> dump repository
//! - POST with Content-Type: application/vnd.svn-dumpfile -> load dumpfile
//!
//! Supports SVN dumpfile format version 2 and 3, full/ranged/incremental dumps.

use bytes::Bytes;
use dsvn_core::{ObjectKind, SqliteRepository, TreeChange};
use http_body_util::Full;
use hyper::Response;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::Arc;

pub const DUMPFILE_MIME: &str = "application/vnd.svn-dumpfile";

#[derive(Debug)]
pub struct DumpParams {
    pub start_rev: u64,
    pub end_rev: u64,
    pub incremental: bool,
    pub format_version: u32,
}

impl DumpParams {
    pub fn from_query(query: &str, head_rev: u64) -> Self {
        let mut start_rev = 0u64;
        let mut end_rev = head_rev;
        let mut incremental = false;
        let mut format_version = 3u32;
        for pair in query.split('&') {
            let mut kv = pair.splitn(2, '=');
            let key = kv.next().unwrap_or("");
            let val = kv.next().unwrap_or("");
            match key {
                "r" | "revision" | "rev" => {
                    if let Some((s, e)) = val.split_once(':') {
                        start_rev = s.parse().unwrap_or(0);
                        end_rev = if e.eq_ignore_ascii_case("head") { head_rev } else { e.parse().unwrap_or(head_rev) };
                    } else {
                        start_rev = val.parse().unwrap_or(0);
                    }
                }
                "incremental" => incremental = val == "true" || val == "1" || val.is_empty(),
                "format" | "dump-format" => format_version = val.parse().unwrap_or(3),
                "start" => start_rev = val.parse().unwrap_or(0),
                "end" => end_rev = if val.eq_ignore_ascii_case("head") { head_rev } else { val.parse().unwrap_or(head_rev) },
                _ => {}
            }
        }
        end_rev = end_rev.min(head_rev);
        if start_rev > end_rev { std::mem::swap(&mut start_rev, &mut end_rev); }
        DumpParams { start_rev, end_rev, incremental, format_version }
    }
}

fn build_rev_props(author: &str, message: &str, date: &str) -> Vec<u8> {
    let mut p = String::new();
    if !author.is_empty() { p.push_str(&format!("K 10\nsvn:author\nV {}\n{}\n", author.len(), author)); }
    if !message.is_empty() { p.push_str(&format!("K 7\nsvn:log\nV {}\n{}\n", message.len(), message)); }
    if !date.is_empty() { p.push_str(&format!("K 8\nsvn:date\nV {}\n{}\n", date.len(), date)); }
    p.push_str("PROPS-END\n");
    p.into_bytes()
}

fn build_empty_props() -> Vec<u8> { b"PROPS-END\n".to_vec() }

fn build_node_props(executable: bool) -> Vec<u8> {
    let mut p = String::new();
    if executable { p.push_str("K 14\nsvn:executable\nV 1\n*\n"); }
    p.push_str("PROPS-END\n");
    p.into_bytes()
}

fn md5_hex(data: &[u8]) -> String {
    let d = md5_compute(data);
    d.iter().map(|b| format!("{:02x}", b)).collect()
}

fn md5_compute(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [7,12,17,22,7,12,17,22,7,12,17,22,7,12,17,22,5,9,14,20,5,9,14,20,5,9,14,20,5,9,14,20,4,11,16,23,4,11,16,23,4,11,16,23,4,11,16,23,6,10,15,21,6,10,15,21,6,10,15,21,6,10,15,21];
    const K: [u32; 64] = [0xd76aa478,0xe8c7b756,0x242070db,0xc1bdceee,0xf57c0faf,0x4787c62a,0xa8304613,0xfd469501,0x698098d8,0x8b44f7af,0xffff5bb1,0x895cd7be,0x6b901122,0xfd987193,0xa679438e,0x49b40821,0xf61e2562,0xc040b340,0x265e5a51,0xe9b6c7aa,0xd62f105d,0x02441453,0xd8a1e681,0xe7d3fbc8,0x21e1cde6,0xc33707d6,0xf4d50d87,0x455a14ed,0xa9e3e905,0xfcefa3f8,0x676f02d9,0x8d2a4c8a,0xfffa3942,0x8771f681,0x6d9d6122,0xfde5380c,0xa4beea44,0x4bdecfa9,0xf6bb4b60,0xbebfbc70,0x289b7ec6,0xeaa127fa,0xd4ef3085,0x04881d05,0xd9d4d039,0xe6db99e5,0x1fa27cf8,0xc4ac5665,0xf4292244,0x432aff97,0xab9423a7,0xfc93a039,0x655b59c3,0x8f0ccc92,0xffeff47d,0x85845dd1,0x6fa87e4f,0xfe2ce6e0,0xa3014314,0x4e0811a1,0xf7537e82,0xbd3af235,0x2ad7d2bb,0xeb86d391];
    let mut a0: u32 = 0x67452301; let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe; let mut d0: u32 = 0x10325476;
    let orig_len_bits = (data.len() as u64) * 8;
    let mut msg = data.to_vec(); msg.push(0x80);
    while msg.len() % 64 != 56 { msg.push(0); }
    msg.extend_from_slice(&orig_len_bits.to_le_bytes());
    for chunk in msg.chunks(64) {
        let mut m = [0u32; 16];
        for i in 0..16 { m[i] = u32::from_le_bytes([chunk[4*i],chunk[4*i+1],chunk[4*i+2],chunk[4*i+3]]); }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b&c)|(!b&d), i),
                16..=31 => ((d&b)|(!d&c), (5*i+1)%16),
                32..=47 => (b^c^d, (3*i+5)%16),
                _ => (c^(b|!d), (7*i)%16),
            };
            let f = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(m[g]);
            a = d; d = c; c = b; b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a); b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c); d0 = d0.wrapping_add(d);
    }
    let mut r = [0u8; 16];
    r[0..4].copy_from_slice(&a0.to_le_bytes()); r[4..8].copy_from_slice(&b0.to_le_bytes());
    r[8..12].copy_from_slice(&c0.to_le_bytes()); r[12..16].copy_from_slice(&d0.to_le_bytes());
    r
}

pub async fn generate_dump(repo: &SqliteRepository, params: &DumpParams) -> Result<Vec<u8>, String> {
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    writeln!(buf, "SVN-fs-dump-format-version: {}", params.format_version).map_err(|e| e.to_string())?;
    writeln!(buf).map_err(|e| e.to_string())?;
    if !params.incremental {
        writeln!(buf, "UUID: {}", repo.uuid()).map_err(|e| e.to_string())?;
        writeln!(buf).map_err(|e| e.to_string())?;
    }
    for rev in params.start_rev..=params.end_rev {
        let commit_opt = repo.get_commit(rev).await;
        let props_bytes = if let Some(ref commit) = commit_opt {
            let date = chrono::DateTime::from_timestamp(commit.timestamp, 0)
                .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S.000000Z").to_string()).unwrap_or_default();
            build_rev_props(&commit.author, &commit.message, &date)
        } else { build_empty_props() };
        writeln!(buf, "Revision-number: {}", rev).map_err(|e| e.to_string())?;
        writeln!(buf, "Prop-content-length: {}", props_bytes.len()).map_err(|e| e.to_string())?;
        writeln!(buf, "Content-length: {}", props_bytes.len()).map_err(|e| e.to_string())?;
        writeln!(buf).map_err(|e| e.to_string())?;
        buf.write_all(&props_bytes).map_err(|e| e.to_string())?;
        writeln!(buf).map_err(|e| e.to_string())?;
        if rev > 0 {
            if let Ok(delta) = repo.get_delta_tree(rev) {
                for change in &delta.changes {
                    write_change_to_buf(&mut buf, repo, rev, change).await?;
                }
            }
        }
    }
    Ok(buf)
}

async fn write_change_to_buf(buf: &mut Vec<u8>, repo: &SqliteRepository, rev: u64, change: &TreeChange) -> Result<(), String> {
    match change {
        TreeChange::Upsert { path, entry } => {
            let kind_str = if entry.kind == ObjectKind::Blob { "file" } else { "dir" };
            let action = if rev > 1 {
                if repo.get_tree_at_rev(rev - 1).map(|t| t.contains_key(path)).unwrap_or(false) { "change" } else { "add" }
            } else { "add" };
            if entry.kind == ObjectKind::Blob {
                match repo.get_file(&format!("/{}", path), rev).await {
                    Ok(content) => {
                        let np = build_node_props(entry.mode & 0o111 != 0);
                        let md5 = md5_hex(&content);
                        writeln!(buf, "Node-path: {}", path).map_err(|e| e.to_string())?;
                        writeln!(buf, "Node-kind: {}", kind_str).map_err(|e| e.to_string())?;
                        writeln!(buf, "Node-action: {}", action).map_err(|e| e.to_string())?;
                        writeln!(buf, "Prop-content-length: {}", np.len()).map_err(|e| e.to_string())?;
                        writeln!(buf, "Text-content-length: {}", content.len()).map_err(|e| e.to_string())?;
                        writeln!(buf, "Text-content-md5: {}", md5).map_err(|e| e.to_string())?;
                        writeln!(buf, "Content-length: {}", np.len() + content.len()).map_err(|e| e.to_string())?;
                        writeln!(buf).map_err(|e| e.to_string())?;
                        buf.write_all(&np).map_err(|e| e.to_string())?;
                        buf.write_all(&content).map_err(|e| e.to_string())?;
                        writeln!(buf).map_err(|e| e.to_string())?;
                        writeln!(buf).map_err(|e| e.to_string())?;
                    }
                    Err(e) => tracing::warn!("Failed to get file {}: {}", path, e),
                }
            } else {
                let np = build_empty_props();
                writeln!(buf, "Node-path: {}", path).map_err(|e| e.to_string())?;
                writeln!(buf, "Node-kind: dir").map_err(|e| e.to_string())?;
                writeln!(buf, "Node-action: {}", action).map_err(|e| e.to_string())?;
                writeln!(buf, "Prop-content-length: {}", np.len()).map_err(|e| e.to_string())?;
                writeln!(buf, "Content-length: {}", np.len()).map_err(|e| e.to_string())?;
                writeln!(buf).map_err(|e| e.to_string())?;
                buf.write_all(&np).map_err(|e| e.to_string())?;
                writeln!(buf).map_err(|e| e.to_string())?;
                writeln!(buf).map_err(|e| e.to_string())?;
            }
        }
        TreeChange::Delete { path } => {
            writeln!(buf, "Node-path: {}", path).map_err(|e| e.to_string())?;
            writeln!(buf, "Node-action: delete").map_err(|e| e.to_string())?;
            writeln!(buf).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub async fn handle_dump(repo: Arc<SqliteRepository>, query: &str) -> Response<Full<Bytes>> {
    let head_rev = repo.current_rev().await;
    let params = DumpParams::from_query(query, head_rev);
    tracing::info!("Dump request: r={}:{}, incremental={}, format={}", params.start_rev, params.end_rev, params.incremental, params.format_version);
    match generate_dump(&repo, &params).await {
        Ok(data) => {
            Response::builder().status(200)
                .header("Content-Type", DUMPFILE_MIME)
                .header("Content-Length", data.len().to_string())
                .header("Content-Disposition", "attachment; filename=\"repository.dump\"")
                .header("X-SVN-Dump-Revision-Range", format!("{}:{}", params.start_rev, params.end_rev))
                .header("X-SVN-Dump-Format-Version", params.format_version.to_string())
                .header("X-SVN-Dump-Incremental", if params.incremental { "true" } else { "false" })
                .body(Full::new(Bytes::from(data))).unwrap()
        }
        Err(e) => {
            tracing::error!("Dump generation failed: {}", e);
            Response::builder().status(500).header("Content-Type", "text/plain")
                .body(Full::new(Bytes::from(format!("Dump generation failed: {}", e)))).unwrap()
        }
    }
}

// ---- Load ----

#[derive(Debug, Default, serde::Serialize)]
pub struct LoadStats {
    pub revisions_loaded: u64,
    pub nodes_processed: u64,
    pub files_added: u64,
    pub dirs_created: u64,
    pub files_deleted: u64,
    pub files_modified: u64,
    pub final_revision: u64,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeAction { Add, Delete, Change, Replace }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeKind { File, Dir }

struct ParsedNode { path: String, kind: Option<NodeKind>, action: Option<NodeAction>, content: Vec<u8> }
struct ParsedRevision { revision_number: u64, author: String, message: String, timestamp: i64, nodes: Vec<ParsedNode> }

fn hdr_val(line: &str) -> String { line.splitn(2, ": ").nth(1).unwrap_or("").trim().to_string() }

fn parse_props_block(data: &[u8]) -> HashMap<String, String> {
    let mut props = HashMap::new();
    let text = String::from_utf8_lossy(data);
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if lines[i] == "PROPS-END" { break; }
        if lines[i].starts_with("K ") {
            let kl: usize = lines[i][2..].parse().unwrap_or(0);
            i += 1; if i >= lines.len() { break; }
            let key = lines[i][..kl.min(lines[i].len())].to_string();
            i += 1; if i >= lines.len() { break; }
            if lines[i].starts_with("V ") {
                let vl: usize = lines[i][2..].parse().unwrap_or(0);
                i += 1; if i >= lines.len() { break; }
                let val = lines[i][..vl.min(lines[i].len())].to_string();
                props.insert(key, val);
            }
        }
        i += 1;
    }
    props
}

fn read_hdr_buf<R: BufRead>(r: &mut R) -> Result<(usize, usize), String> {
    let mut pl = 0usize; let mut cl = 0usize; let mut line = String::new();
    loop {
        line.clear();
        let n = r.read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 { break; } let t = line.trim();
        if t.is_empty() { break; }
        if t.starts_with("Prop-content-length:") { pl = hdr_val(t).parse().unwrap_or(0); }
        else if t.starts_with("Content-length:") { cl = hdr_val(t).parse().unwrap_or(0); }
    }
    Ok((pl, cl))
}

fn parse_dump_stream<R: BufRead, F: FnMut(ParsedRevision) -> Result<(), String>>(
    mut reader: R, mut on_rev: F
) -> Result<(), String> {
    let mut lb = String::new();
    let mut cur_rev: Option<ParsedRevision> = None;
    loop {
        lb.clear();
        let n = reader.read_line(&mut lb).map_err(|e| e.to_string())?;
        if n == 0 { break; }
        let tr = lb.trim().to_string();
        if tr.starts_with("SVN-fs-dump-format-version:") || tr.starts_with("UUID:") || tr.is_empty() { continue; }
        if tr.starts_with("Revision-number:") {
            if let Some(rev) = cur_rev.take() { on_rev(rev)?; }
            let rn: u64 = hdr_val(&tr).parse().unwrap_or(0);
            let (pl, cl) = read_hdr_buf(&mut reader)?;
            let mut props = HashMap::new();
            let total = if cl > 0 { cl } else { pl };
            if total > 0 {
                let mut buf = vec![0u8; total];
                reader.read_exact(&mut buf).map_err(|e| e.to_string())?;
                if pl > 0 { props = parse_props_block(&buf[..pl]); }
            }
            cur_rev = Some(ParsedRevision {
                revision_number: rn,
                author: props.get("svn:author").cloned().unwrap_or_else(|| "unknown".into()),
                message: props.get("svn:log").cloned().unwrap_or_default(),
                timestamp: props.get("svn:date")
                    .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
                    .map(|dt| dt.timestamp())
                    .unwrap_or_else(|| chrono::Utc::now().timestamp()),
                nodes: Vec::new(),
            });
            continue;
        }
        if tr.starts_with("Node-path:") {
            let mut nd = ParsedNode { path: hdr_val(&tr), kind: None, action: None, content: Vec::new() };
            parse_node_headers(&mut reader, &mut lb, &mut nd)?;
            if let Some(ref mut rev) = cur_rev { rev.nodes.push(nd); }
            continue;
        }
    }
    if let Some(rev) = cur_rev.take() { on_rev(rev)?; }
    Ok(())
}

fn parse_node_headers<R: BufRead>(reader: &mut R, lb: &mut String, nd: &mut ParsedNode) -> Result<(), String> {
    loop {
        lb.clear();
        let n = reader.read_line(lb).map_err(|e| e.to_string())?;
        if n == 0 { break; }
        let t = lb.trim().to_string();
        if t.is_empty() { break; }
        if t.starts_with("Node-kind:") {
            nd.kind = match hdr_val(&t).as_str() { "file" => Some(NodeKind::File), "dir" => Some(NodeKind::Dir), _ => None };
        } else if t.starts_with("Node-action:") {
            nd.action = match hdr_val(&t).as_str() {
                "add" => Some(NodeAction::Add), "delete" => Some(NodeAction::Delete),
                "change" => Some(NodeAction::Change), "replace" => Some(NodeAction::Replace), _ => None
            };
        } else if t.starts_with("Prop-content-length:") || t.starts_with("Text-content-length:") || t.starts_with("Content-length:") {
            // Re-parse from here to get all content headers
            let mut pl: usize = 0; let mut tl: usize = 0; let mut tcl: usize = 0;
            if t.starts_with("Prop-content-length:") { pl = hdr_val(&t).parse().unwrap_or(0); }
            else if t.starts_with("Text-content-length:") { tl = hdr_val(&t).parse().unwrap_or(0); }
            else if t.starts_with("Content-length:") { tcl = hdr_val(&t).parse().unwrap_or(0); }
            // Read remaining content headers
            loop {
                lb.clear();
                let n2 = reader.read_line(lb).map_err(|e| e.to_string())?;
                if n2 == 0 { break; }
                let t2 = lb.trim().to_string();
                if t2.is_empty() { break; }
                if t2.starts_with("Prop-content-length:") { pl = hdr_val(&t2).parse().unwrap_or(0); }
                else if t2.starts_with("Text-content-length:") { tl = hdr_val(&t2).parse().unwrap_or(0); }
                else if t2.starts_with("Content-length:") { tcl = hdr_val(&t2).parse().unwrap_or(0); }
            }
            let rl = if tcl > 0 { tcl } else { pl + tl };
            if rl > 0 {
                let mut buf = vec![0u8; rl];
                reader.read_exact(&mut buf).map_err(|e| e.to_string())?;
                if tl > 0 && pl + tl <= buf.len() {
                    nd.content = buf[pl..pl+tl].to_vec();
                } else if tl > 0 {
                    nd.content = buf[..tl.min(buf.len())].to_vec();
                }
            }
            // Consume trailing newline
            lb.clear(); let _ = reader.read_line(lb);
            break;
        }
        // Ignore Node-copyfrom-*, Text-content-md5, Text-content-sha1
    }
    Ok(())
}

pub async fn handle_load(repo: Arc<SqliteRepository>, body: Vec<u8>) -> Response<Full<Bytes>> {
    let start = std::time::Instant::now();
    let repo_clone = repo.clone();
    let result = tokio::task::spawn_blocking(move || {
        let reader = std::io::BufReader::new(&body[..]);
        let mut st = LoadStats::default();
        parse_dump_stream(reader, |revision| {
            if revision.revision_number == 0 || revision.nodes.is_empty() { return Ok(()); }
            repo_clone.begin_batch();
            let mut has_changes = false;
            for node in &revision.nodes {
                st.nodes_processed += 1;
                match node.action {
                    Some(NodeAction::Add) | Some(NodeAction::Replace) => {
                        match node.kind {
                            Some(NodeKind::File) => { repo_clone.add_file_sync(&node.path, node.content.clone(), false).map_err(|e| e.to_string())?; st.files_added += 1; has_changes = true; }
                            Some(NodeKind::Dir) => { repo_clone.mkdir_sync(&node.path).map_err(|e| e.to_string())?; st.dirs_created += 1; has_changes = true; }
                            None => { if !node.content.is_empty() { repo_clone.add_file_sync(&node.path, node.content.clone(), false).map_err(|e| e.to_string())?; st.files_added += 1; has_changes = true; } }
                        }
                    }
                    Some(NodeAction::Delete) => { repo_clone.delete_file_sync(&node.path).map_err(|e| e.to_string())?; st.files_deleted += 1; has_changes = true; }
                    Some(NodeAction::Change) => { if !node.content.is_empty() { repo_clone.add_file_sync(&node.path, node.content.clone(), false).map_err(|e| e.to_string())?; st.files_modified += 1; has_changes = true; } }
                    None => {}
                }
            }
            if has_changes {
                let rev = repo_clone.commit_sync(revision.author, revision.message, revision.timestamp).map_err(|e| e.to_string())?;
                st.final_revision = rev;
            }
            repo_clone.end_batch();
            st.revisions_loaded += 1;
            Ok(())
        })?;
        Ok::<LoadStats, String>(st)
    }).await;
    match result {
        Ok(Ok(mut st)) => {
            st.elapsed_ms = start.elapsed().as_millis() as u64;
            let json = serde_json::to_string_pretty(&st).unwrap_or_default();
            Response::builder().status(200)
                .header("Content-Type", "application/json")
                .header("X-SVN-Revisions-Loaded", st.revisions_loaded.to_string())
                .header("X-SVN-Final-Revision", st.final_revision.to_string())
                .body(Full::new(Bytes::from(json))).unwrap()
        }
        Ok(Err(e)) => {
            tracing::error!("Dump load failed: {}", e);
            Response::builder().status(500).header("Content-Type", "text/plain")
                .body(Full::new(Bytes::from(format!("Load failed: {}", e)))).unwrap()
        }
        Err(e) => {
            tracing::error!("Dump load task panicked: {:?}", e);
            Response::builder().status(500).header("Content-Type", "text/plain")
                .body(Full::new(Bytes::from(format!("Load task failed: {:?}", e)))).unwrap()
        }
    }
}

pub fn is_dump_request(accept: &str) -> bool {
    accept.contains(DUMPFILE_MIME) || accept.contains("svn-dumpfile")
}

pub fn is_load_request(content_type: &str) -> bool {
    content_type.contains(DUMPFILE_MIME) || content_type.contains("svn-dumpfile")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dump_params_default() {
        let p = DumpParams::from_query("", 10);
        assert_eq!(p.start_rev, 0);
        assert_eq!(p.end_rev, 10);
        assert!(!p.incremental);
        assert_eq!(p.format_version, 3);
    }

    #[test]
    fn test_dump_params_range() {
        let p = DumpParams::from_query("r=3:7", 10);
        assert_eq!(p.start_rev, 3);
        assert_eq!(p.end_rev, 7);
    }

    #[test]
    fn test_dump_params_head() {
        let p = DumpParams::from_query("r=5:HEAD", 20);
        assert_eq!(p.start_rev, 5);
        assert_eq!(p.end_rev, 20);
    }

    #[test]
    fn test_dump_params_incremental() {
        let p = DumpParams::from_query("r=3:5&incremental=true", 10);
        assert_eq!(p.start_rev, 3);
        assert_eq!(p.end_rev, 5);
        assert!(p.incremental);
    }

    #[test]
    fn test_dump_params_format() {
        let p = DumpParams::from_query("format=2", 10);
        assert_eq!(p.format_version, 2);
    }

    #[test]
    fn test_dump_params_clamp() {
        let p = DumpParams::from_query("r=0:100", 5);
        assert_eq!(p.end_rev, 5);
    }

    #[test]
    fn test_md5_hex_known() {
        // MD5("") = d41d8cd98f00b204e9800998ecf8427e
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        // MD5("hello") = 5d41402abc4b2a76b9719d911017c592
        assert_eq!(md5_hex(b"hello"), "5d41402abc4b2a76b9719d911017c592");
    }

    #[test]
    fn test_build_rev_props() {
        let props = build_rev_props("alice", "test commit", "2024-01-01T00:00:00.000000Z");
        let s = String::from_utf8(props).unwrap();
        assert!(s.contains("svn:author"));
        assert!(s.contains("alice"));
        assert!(s.contains("svn:log"));
        assert!(s.contains("test commit"));
        assert!(s.contains("svn:date"));
        assert!(s.contains("PROPS-END"));
    }

    #[test]
    fn test_is_dump_request() {
        assert!(is_dump_request("application/vnd.svn-dumpfile"));
        assert!(is_dump_request("text/html, application/vnd.svn-dumpfile"));
        assert!(!is_dump_request("text/html"));
    }

    #[test]
    fn test_is_load_request() {
        assert!(is_load_request("application/vnd.svn-dumpfile"));
        assert!(!is_load_request("application/json"));
    }

    #[test]
    fn test_parse_dump_roundtrip() {
        // Create a minimal dumpfile
        let dump = b"SVN-fs-dump-format-version: 3\n\
\n\
UUID: test-uuid-1234\n\
\n\
Revision-number: 0\n\
Prop-content-length: 10\n\
Content-length: 10\n\
\n\
PROPS-END\n\
\n\
Revision-number: 1\n\
Prop-content-length: 56\n\
Content-length: 56\n\
\n\
K 7\n\
svn:log\n\
V 4\n\
test\n\
K 10\n\
svn:author\n\
V 4\n\
user\n\
PROPS-END\n\
\n\
Node-path: hello.txt\n\
Node-kind: file\n\
Node-action: add\n\
Prop-content-length: 10\n\
Text-content-length: 5\n\
Content-length: 15\n\
\n\
PROPS-END\n\
hello\n";

        let reader = std::io::BufReader::new(&dump[..]);
        let mut revisions = Vec::new();
        parse_dump_stream(reader, |rev| {
            revisions.push((rev.revision_number, rev.author.clone(), rev.nodes.len()));
            Ok(())
        }).unwrap();

        assert_eq!(revisions.len(), 2);
        assert_eq!(revisions[0].0, 0); // rev 0
        assert_eq!(revisions[1].0, 1); // rev 1
        assert_eq!(revisions[1].1, "user");
        assert_eq!(revisions[1].2, 1); // 1 node
    }
}