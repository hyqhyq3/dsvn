//! WebDAV HTTP method handlers

use super::{Config, WebDavError};
use bytes::Bytes;
use dsvn_core::{Repository, properties::PropertyStore};
use http_body_util::{BodyExt, Full};
use hyper::{body::Incoming, Request, Response};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone)]
struct Transaction {
    pub id: String,
    pub base_revision: u64,
    pub author: String,
    pub created_at: i64,
    #[allow(dead_code)]
    pub state: String,
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
    } else {
        String::new()
    };

    let mut b = Response::builder()
        .status(200)
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
            .header("SVN-Supported-Posts", "create-txn-with-props")
            .header("Content-Type", "text/xml; charset=\"utf-8\"");
    }

    Ok(b.body(Full::new(Bytes::from(response_body))).unwrap())
}

// ==================== PROPFIND ====================

pub async fn propfind_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path().to_string();
    let depth = req.headers().get("Depth").and_then(|v| v.to_str().ok()).unwrap_or("1").to_string();
    let _ = req.into_body().collect().await; // consume body

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
    } else if special.starts_with("/!svn/ver/") || special.starts_with("/!svn/rvr/") {
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
<D:checked-in><D:href>{prefix}/!svn/ver/{rev}/</D:href></D:checked-in>
<S:set-prop name="svn:entry:committed-rev">{rev}</S:set-prop>
<S:set-prop name="svn:entry:committed-date">{date}</S:set-prop>
<S:set-prop name="svn:entry:uuid">{uuid}</S:set-prop>
</S:open-directory>
</S:update-report>"#,
            prefix=REPO_PREFIX, rev=target_rev,
            date=now.format("%Y-%m-%dT%H:%M:%S.000000Z"), uuid=uuid)
    } else if body_str.contains("log") {
        let current_rev = REPOSITORY.current_rev().await;
        let commits = REPOSITORY.log(current_rev, 100).await.unwrap_or_default();
        let mut s = String::from(r#"<?xml version="1.0" encoding="utf-8"?><S:log-report xmlns:S="svn:" xmlns:D="DAV:">"#);
        for c in commits {
            s.push_str(&format!(r#"<S:log-item><D:version-name>{}</D:version-name><D:creator-displayname>{}</D:creator-displayname><D:comment>{}</D:comment></S:log-item>"#, current_rev, c.author, c.message));
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

pub async fn merge_handler(_req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let new_rev = REPOSITORY.commit("user".into(), "Test commit".into(), chrono::Utc::now().timestamp()).await
        .map_err(|e| WebDavError::Internal(e.to_string()))?;
    Ok(Response::builder().status(200)
        .body(Full::new(Bytes::from(format!(r#"<?xml version="1.0"?><D:merge-response xmlns:D="DAV:"><D:version-name>{}</D:version-name></D:merge-response>"#, new_rev)))).unwrap())
}

// ==================== GET ====================

pub async fn get_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path();
    if path.contains("!svn") {
        if path.contains("/vcc/") {
            return Ok(Response::builder().status(200).header("Content-Type", "text/xml; charset=utf-8")
                .body(Full::new(Bytes::from(format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<D:href xmlns:D=\"DAV:\">{}/!svn/vcc/default</D:href>", REPO_PREFIX)))).unwrap());
        }
        return Ok(Response::builder().status(404).body(Full::new(Bytes::from("SVN special path not implemented"))).unwrap());
    }
    if path.ends_with("/") || path == "/svn" {
        return Ok(Response::builder().status(405).header("Allow", "PROPFIND").body(Full::new(Bytes::from("Use PROPFIND"))).unwrap());
    }
    match REPOSITORY.get_file(path, REPOSITORY.current_rev().await).await {
        Ok(content) => Ok(Response::builder().status(200).header("Content-Type", "application/octet-stream").body(Full::new(content)).unwrap()),
        Err(_) => Ok(Response::builder().status(404).body(Full::new(Bytes::from("Not found"))).unwrap()),
    }
}

// ==================== PROPPATCH ====================

pub async fn proppatch_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    use crate::proppatch::{PropPatchRequest, PropPatchResponse, PropertyModification};
    let path = req.uri().path().to_string();
    let body = match req.collect().await {
        Ok(c) => c.to_bytes(),
        Err(e) => return Ok(Response::builder().status(400).body(Full::new(Bytes::from(format!("Failed to read body: {}", e)))).unwrap()),
    };
    let body_str = String::from_utf8_lossy(&body);
    let proppatch_req = match PropPatchRequest::from_xml(&body_str) {
        Ok(r) => r,
        Err(e) => {
            let response = PropPatchResponse::error(path.clone(), format!("Invalid XML: {}", e));
            return Ok(Response::builder().status(207).header("Content-Type", "text/xml; charset=utf-8").body(Full::new(Bytes::from(response.to_xml()))).unwrap());
        }
    };
    for modification in &proppatch_req.modifications {
        match modification {
            PropertyModification::Set { name, value, .. } => {
                if let Err(e) = PROPERTY_STORE.set(path.clone(), name.clone(), value.clone()).await {
                    let response = PropPatchResponse::error(path.clone(), format!("Failed: {}", e));
                    return Ok(Response::builder().status(207).header("Content-Type", "text/xml; charset=utf-8").body(Full::new(Bytes::from(response.to_xml()))).unwrap());
                }
            }
            PropertyModification::Remove { name, .. } => {
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

// ==================== CHECKOUT ====================

pub async fn checkout_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path();
    let rev = REPOSITORY.current_rev().await;
    Ok(Response::builder().status(200).header("Content-Type", "text/xml; charset=utf-8").header("Cache-Control", "no-cache")
        .body(Full::new(Bytes::from(format!(r#"<?xml version="1.0" encoding="utf-8"?><D:checkout-response xmlns:D="DAV:"><D:href>{}</D:href><D:version-name>{}</D:version-name></D:checkout-response>"#, path, rev)))).unwrap())
}

// ==================== CHECKIN ====================

pub async fn checkin_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let author = req.headers().get("X-SVN-Author").and_then(|v| v.to_str().ok()).unwrap_or("anonymous").to_string();
    let log_msg = req.headers().get("X-SVN-Log-Message").and_then(|v| v.to_str().ok()).unwrap_or("Commit via CHECKIN").to_string();
    let new_rev = REPOSITORY.commit(author.clone(), log_msg.clone(), chrono::Utc::now().timestamp()).await
        .map_err(|e| WebDavError::Internal(e.to_string()))?;
    Ok(Response::builder().status(200).header("Content-Type", "text/xml; charset=utf-8")
        .body(Full::new(Bytes::from(format!(r#"<?xml version="1.0" encoding="utf-8"?><D:checkin-response xmlns:D="DAV:"><D:version-name>{}</D:version-name><D:creator-displayname>{}</D:creator-displayname><D:comment>{}</D:comment></D:checkin-response>"#, new_rev, author, log_msg)))).unwrap())
}

// ==================== MKACTIVITY ====================

pub async fn mkactivity_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let activity_id = Uuid::new_v4().to_string();
    let current_rev = REPOSITORY.current_rev().await;
    let author = req.headers().get("X-SVN-User").and_then(|v| v.to_str().ok()).unwrap_or("anonymous").to_string();
    let mut transactions = TRANSACTIONS.write().await;
    transactions.insert(activity_id.clone(), Transaction {
        id: activity_id.clone(), base_revision: current_rev, author,
        created_at: chrono::Utc::now().timestamp(), state: "active".to_string(),
    });
    Ok(Response::builder().status(201).header("Location", format!("{}/!svn/act/{}", REPO_PREFIX, activity_id))
        .body(Full::new(Bytes::new())).unwrap())
}

// ==================== MKCOL ====================

pub async fn mkcol_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path();
    if !path.ends_with('/') && path != "/svn" {
        return Ok(Response::builder().status(405).body(Full::new(Bytes::from("MKCOL can only create collections"))).unwrap());
    }
    let current_rev = REPOSITORY.current_rev().await;
    if REPOSITORY.exists(&path, current_rev).await.unwrap_or(false) {
        return Ok(Response::builder().status(405).body(Full::new(Bytes::from("Resource already exists"))).unwrap());
    }
    match REPOSITORY.mkdir(&path).await {
        Ok(_) => Ok(Response::builder().status(201).body(Full::new(Bytes::new())).unwrap()),
        Err(e) => Ok(Response::builder().status(500).body(Full::new(Bytes::from(format!("Failed: {}", e)))).unwrap()),
    }
}

// ==================== DELETE ====================

pub async fn delete_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path();
    if path == "/svn" || path == "/" {
        return Ok(Response::builder().status(403).body(Full::new(Bytes::from("Cannot delete root"))).unwrap());
    }
    let current_rev = REPOSITORY.current_rev().await;
    if !REPOSITORY.exists(&path, current_rev).await.unwrap_or(false) {
        return Ok(Response::builder().status(404).body(Full::new(Bytes::from("Not found"))).unwrap());
    }
    match REPOSITORY.delete_file(&path).await {
        Ok(_) => Ok(Response::builder().status(204).body(Full::new(Bytes::new())).unwrap()),
        Err(e) => Ok(Response::builder().status(500).body(Full::new(Bytes::from(format!("Failed: {}", e)))).unwrap()),
    }
}

// ==================== PUT ====================

pub async fn put_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path().to_string();
    if path.ends_with('/') || path == "/svn" {
        return Ok(Response::builder().status(400).body(Full::new(Bytes::from("Cannot PUT to directory"))).unwrap());
    }
    let body = match req.into_body().collect().await {
        Ok(c) => c.to_bytes(),
        Err(e) => return Ok(Response::builder().status(400).body(Full::new(Bytes::from(format!("Failed: {}", e)))).unwrap()),
    };
    let content = body.to_vec();
    let current_rev = REPOSITORY.current_rev().await;
    let file_exists = REPOSITORY.exists(&path, current_rev).await.unwrap_or(false);
    let executable = path.ends_with(".sh") || path.contains("/bin/") || path.contains("/scripts/");
    match REPOSITORY.add_file(&path, content, executable).await {
        Ok(_) => Ok(Response::builder().status(if file_exists { 200 } else { 201 }).body(Full::new(Bytes::new())).unwrap()),
        Err(e) => Ok(Response::builder().status(500).body(Full::new(Bytes::from(format!("Failed: {}", e)))).unwrap()),
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
