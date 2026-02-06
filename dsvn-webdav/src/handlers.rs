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

/// Represents an active SVN transaction (activity)
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
        Arc::new(repo)
    };

    static ref TRANSACTIONS: Arc<RwLock<HashMap<String, Transaction>>> = {
        Arc::new(RwLock::new(HashMap::new()))
    };

    static ref PROPERTY_STORE: Arc<PropertyStore> = {
        let store = PropertyStore::new();
        Arc::new(store)
    };
}

pub async fn propfind_handler(
    req: Request<Incoming>,
    _config: &Config,
) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path();
    let depth = req.headers().get("Depth").and_then(|v| v.to_str().ok()).unwrap_or("1");
    let current_rev = REPOSITORY.current_rev().await;

    let mut response = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:" xmlns:svn="http://subversion.tigris.org/xmlns/dav/">
"#);

    // Helper function to escape XML special characters
    fn escape_xml(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    }

    // Helper function to get properties for a path as XML
    async fn get_properties_xml(path: &str) -> String {
        let props = PROPERTY_STORE.list(path).await;
        let prop_set = PROPERTY_STORE.get(path).await;
        let mut xml = String::new();

        for prop_name in props {
            if let Some(prop_value) = prop_set.get(&prop_name) {
                // Format: <svn:executable>*</svn:executable>
                xml.push_str(&format!("<svn:{}>{}</svn:{}>", prop_name, escape_xml(prop_value), prop_name));
            }
        }

        xml
    };

    if path.ends_with("/") || path == "/svn" {
        // Directory/collection
        let properties = get_properties_xml(&path).await;
        response.push_str(&format!(
            r#"  <D:response><D:href>{}</D:href><D:propstat><D:prop>
<D:resourcetype><D:collection/></D:resourcetype>
<D:version-controlled-configuration><D:href>/svn/!svn/vcc/default</D:href></D:version-controlled-configuration>
{}</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>
"#, path, properties
        ));

        if depth != "0" {
            if let Ok(entries) = REPOSITORY.list_dir(path, current_rev).await {
                for entry in entries {
                    let entry_path = format!("{}{}", path.trim_end_matches('/'), entry);
                    let entry_properties = get_properties_xml(&entry_path).await;
                    response.push_str(&format!(
                        r#"  <D:response><D:href>{}</D:href><D:propstat>
<D:prop><D:resourcetype></D:resourcetype>{}</D:prop><D:status>HTTP/1.1 200 OK</D:status>
</D:propstat></D:response>
"#, entry_path, entry_properties
                    ));
                }
            }
        }
    } else {
        // File
        let properties = get_properties_xml(&path).await;
        response.push_str(&format!(
            r#"  <D:response><D:href>{}</D:href><D:propstat>
<D:prop><D:resourcetype></D:resourcetype>{}</D:prop><D:status>HTTP/1.1 200 OK</D:status>
</D:propstat></D:response>
"#, path, properties
        ));
    }

    response.push_str("</D:multistatus>");

    Ok(Response::builder()
        .status(207)
        .header("Content-Type", "text/xml; charset=utf-8")
        .body(Full::new(Bytes::from(response)))
        .unwrap())
}

pub async fn report_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let body = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => return Ok(Response::builder().status(400).body(Full::new(Bytes::from(format!("Failed: {}", e)))).unwrap()),
    };

    let body_str = String::from_utf8_lossy(&body);
    let response = if body_str.contains("log") {
        let current_rev = REPOSITORY.current_rev().await;
        let commits = REPOSITORY.log(current_rev, 100).await.unwrap_or_default();
        let mut xml = String::from(r#"<?xml version="1.0" encoding="utf-8"?><S:log-report xmlns:S="svn:" xmlns:D="DAV:">"#);
        for commit in commits {
            xml.push_str(&format!(r#"<S:log-item><D:version-name>{}</D:version-name><D:creator-displayname>{}</D:creator-displayname><D:comment>{}</D:comment></S:log-item>"#, current_rev, commit.author, commit.message));
        }
        xml.push_str("</S:log-report>");
        xml
    } else if body_str.contains("update") {
        let current_rev = REPOSITORY.current_rev().await;
        let uuid = REPOSITORY.uuid();
        format!(r#"<?xml version="1.0"?><S:update-report xmlns:S="svn:"><S:target-revision><D:version-name>{}</D:version-name></S:target-revision><S:entry><D:uuid>{}</D:uuid></S:entry></S:update-report>"#, current_rev, uuid)
    } else {
        return Ok(Response::builder().status(400).body(Full::new(Bytes::from("Unknown report"))).unwrap());
    };

    Ok(Response::builder().status(200).header("Content-Type", "text/xml").body(Full::new(Bytes::from(response))).unwrap())
}

pub async fn merge_handler(_req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let new_rev = REPOSITORY.commit("user".into(), "Test commit".into(), chrono::Utc::now().timestamp()).await
        .map_err(|e| WebDavError::Internal(e.to_string()))?;
    Ok(Response::builder().status(200).body(Full::new(Bytes::from(format!(r#"<?xml version="1.0"?><D:merge-response xmlns:D="DAV:"><D:version-name>{}</D:version-name></D:merge-response>"#, new_rev)))).unwrap())
}

pub async fn get_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path();

    // Handle SVN special paths
    if path.contains("!svn") {
        // SVN protocol metadata paths
        // For now, return empty collection response for VCC
        if path.contains("/vcc/") {
            return Ok(Response::builder()
                .status(200)
                .header("Content-Type", "text/xml; charset=utf-8")
                .body(Full::new(Bytes::from(
                    r#"<?xml version="1.0" encoding="utf-8"?>
<D:href xmlns:D="DAV:">/svn/!svn/vcc/default</D:href>"#
                )))
                .unwrap());
        }
        // Return 404 for other special paths (they will be implemented as needed)
        return Ok(Response::builder()
            .status(404)
            .body(Full::new(Bytes::from("SVN special path not implemented")))
            .unwrap());
    }

    if path.ends_with("/") || path == "/svn" {
        return Ok(Response::builder().status(405).header("Allow", "PROPFIND").body(Full::new(Bytes::from("Use PROPFIND"))).unwrap());
    }

    match REPOSITORY.get_file(path, REPOSITORY.current_rev().await).await {
        Ok(content) => Ok(Response::builder().status(200).header("Content-Type", "application/octet-stream").body(Full::new(content)).unwrap()),
        Err(_) => Ok(Response::builder().status(404).body(Full::new(Bytes::from("Not found"))).unwrap()),
    }
}

// PROPPATCH handler - modify properties
pub async fn proppatch_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    use crate::proppatch::{PropPatchResponse, PropPatchRequest, PropertyModification};

    let path = req.uri().path().to_string();

    // Collect request body
    let body = match req.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            return Ok(Response::builder()
                .status(400)
                .body(Full::new(Bytes::from(format!("Failed to read body: {}", e))))
                .unwrap());
        }
    };

    let body_str = String::from_utf8_lossy(&body);

    // Parse XML body
    let proppatch_req = match PropPatchRequest::from_xml(&body_str) {
        Ok(req) => req,
        Err(e) => {
            tracing::error!("Failed to parse PROPPATCH request: {}", e);
            let response = PropPatchResponse::error(path.clone(), format!("Invalid XML: {}", e));
            return Ok(Response::builder()
                .status(207)
                .header("Content-Type", "text/xml; charset=utf-8")
                .body(Full::new(Bytes::from(response.to_xml())))
                .unwrap());
        }
    };

    // Apply property modifications
    for modification in &proppatch_req.modifications {
        match modification {
            PropertyModification::Set { name, value, .. } => {
                if let Err(e) = PROPERTY_STORE.set(path.clone(), name.clone(), value.clone()).await {
                    tracing::error!("Failed to set property {}: {}", name, e);
                    let response = PropPatchResponse::error(path.clone(), format!("Failed to set property: {}", e));
                    return Ok(Response::builder()
                        .status(207)
                        .header("Content-Type", "text/xml; charset=utf-8")
                        .body(Full::new(Bytes::from(response.to_xml())))
                        .unwrap());
                }
                tracing::debug!("Set property {} on path {}", name, path);
            }
            PropertyModification::Remove { name, .. } => {
                if let Err(e) = PROPERTY_STORE.remove(&path, name).await {
                    tracing::error!("Failed to remove property {}: {}", name, e);
                    let response = PropPatchResponse::error(path.clone(), format!("Failed to remove property: {}", e));
                    return Ok(Response::builder()
                        .status(207)
                        .header("Content-Type", "text/xml; charset=utf-8")
                        .body(Full::new(Bytes::from(response.to_xml())))
                        .unwrap());
                }
                tracing::debug!("Removed property {} from path {}", name, path);
            }
        }
    }

    // Return success response
    let response = PropPatchResponse::success(path);

    Ok(Response::builder()
        .status(207)
        .header("Content-Type", "text/xml; charset=utf-8")
        .body(Full::new(Bytes::from(response.to_xml())))
        .unwrap())
}
pub async fn checkout_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path();
    let current_rev = REPOSITORY.current_rev().await;

    let response = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<D:checkout-response xmlns:D="DAV:">
  <D:href>{}</D:href>
  <D:version-name>{}</D:version-name>
</D:checkout-response>"#,
        path, current_rev
    );

    Ok(Response::builder()
        .status(200)
        .header("Content-Type", "text/xml; charset=utf-8")
        .header("Cache-Control", "no-cache")
        .body(Full::new(Bytes::from(response)))
        .unwrap())
}

pub async fn checkin_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    // Extract author from headers
    let author = req
        .headers()
        .get("X-SVN-Author")
        .and_then(|v| v.to_str().ok())
        .or_else(|| req.headers().get("Authorization").and_then(|v| v.to_str().ok()))
        .unwrap_or("anonymous")
        .to_string();

    // Extract log message from headers
    let log_message = req
        .headers()
        .get("X-SVN-Log-Message")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("Commit via CHECKIN")
        .to_string();

    // Create new commit
    let new_rev = REPOSITORY
        .commit(
            author.clone(),
            log_message.clone(),
            chrono::Utc::now().timestamp(),
        )
        .await
        .map_err(|e| WebDavError::Internal(e.to_string()))?;

    let response = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<D:checkin-response xmlns:D="DAV:">
  <D:version-name>{}</D:version-name>
  <D:creator-displayname>{}</D:creator-displayname>
  <D:comment>{}</D:comment>
</D:checkin-response>"#,
        new_rev, author, log_message
    );

    Ok(Response::builder()
        .status(200)
        .header("Content-Type", "text/xml; charset=utf-8")
        .header("Cache-Control", "no-cache")
        .body(Full::new(Bytes::from(response)))
        .unwrap())
}
pub async fn mkactivity_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    // Generate unique activity ID
    let activity_id = Uuid::new_v4().to_string();

    // Get current revision as base for this transaction
    let current_rev = REPOSITORY.current_rev().await;

    // Extract author from X-SVN-User header (or use default)
    let author = req
        .headers()
        .get("X-SVN-User")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("anonymous")
        .to_string();

    // Create transaction metadata
    let transaction = Transaction {
        id: activity_id.clone(),
        base_revision: current_rev,
        author: author.clone(),
        created_at: chrono::Utc::now().timestamp(),
        state: "active".to_string(),
    };

    // Store transaction in global state
    let mut transactions = TRANSACTIONS.write().await;
    transactions.insert(activity_id.clone(), transaction);

    tracing::debug!(
        activity_id = %activity_id,
        base_revision = current_rev,
        author = %author,
        "Created new SVN activity"
    );

    // Return 201 Created with Location header
    let location = format!("/svn/!svn/act/{}", activity_id);
    Ok(Response::builder()
        .status(201)
        .header("Location", location)
        .body(Full::new(Bytes::new()))
        .unwrap())
}
pub async fn mkcol_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path();

    // Validate path - MKCOL should target directories (ending with /)
    if !path.ends_with('/') && path != "/svn" {
        return Ok(Response::builder()
            .status(405)
            .body(Full::new(Bytes::from("MKCOL can only create collections")))
            .unwrap());
    }

    let current_rev = REPOSITORY.current_rev().await;

    // Check if resource already exists
    if REPOSITORY.exists(&path, current_rev).await.unwrap_or(false) {
        return Ok(Response::builder()
            .status(405)
            .body(Full::new(Bytes::from("Resource already exists")))
            .unwrap());
    }

    // Create directory
    match REPOSITORY.mkdir(&path).await {
        Ok(_object_id) => Ok(Response::builder()
            .status(201)
            .body(Full::new(Bytes::new()))
            .unwrap()),
        Err(e) => Ok(Response::builder()
            .status(500)
            .body(Full::new(Bytes::from(format!("Failed to create collection: {}", e))))
            .unwrap()),
    }
}
pub async fn delete_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path();

    // Prevent deletion of repository root
    if path == "/svn" || path == "/" {
        return Ok(Response::builder()
            .status(403)
            .body(Full::new(Bytes::from("Cannot delete repository root")))
            .unwrap());
    }

    let current_rev = REPOSITORY.current_rev().await;

    // Check if resource exists
    if !REPOSITORY.exists(&path, current_rev).await.unwrap_or(false) {
        return Ok(Response::builder()
            .status(404)
            .body(Full::new(Bytes::from("Resource not found")))
            .unwrap());
    }

    // Delete the resource
    match REPOSITORY.delete_file(&path).await {
        Ok(_) => Ok(Response::builder()
            .status(204)
            .body(Full::new(Bytes::new()))
            .unwrap()),
        Err(e) => Ok(Response::builder()
            .status(500)
            .body(Full::new(Bytes::from(format!("Failed to delete resource: {}", e))))
            .unwrap()),
    }
}
pub async fn put_handler(req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    let path = req.uri().path().to_string();

    // Validate path - PUT should not target directories
    if path.ends_with('/') || path == "/svn" {
        return Ok(Response::builder()
            .status(400)
            .body(Full::new(Bytes::from("Cannot PUT to directory")))
            .unwrap());
    }

    // Collect request body
    let body = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            return Ok(Response::builder()
                .status(400)
                .body(Full::new(Bytes::from(format!("Failed to read body: {}", e))))
                .unwrap());
        }
    };

    let content = body.to_vec();
    let current_rev = REPOSITORY.current_rev().await;

    // Check if file already exists to determine correct status code
    let file_exists = REPOSITORY.exists(&path, current_rev).await.unwrap_or(false);

    // Determine if file should be executable based on common patterns
    // In a full implementation, this would come from the request or file properties
    let executable = path.ends_with(".sh") || path.contains("/bin/") || path.contains("/scripts/");

    // Add or update file
    match REPOSITORY.add_file(&path, content, executable).await {
        Ok(_object_id) => {
            let status = if file_exists { 200 } else { 201 };
            Ok(Response::builder()
                .status(status)
                .body(Full::new(Bytes::new()))
                .unwrap())
        }
        Err(e) => Ok(Response::builder()
            .status(500)
            .body(Full::new(Bytes::from(format!("Failed to save file: {}", e))))
            .unwrap()),
    }
}
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
