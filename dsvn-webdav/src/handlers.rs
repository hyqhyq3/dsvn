//! WebDAV HTTP method handlers

use super::{Config, WebDavError};
use bytes::Bytes;
use dsvn_core::Repository;
use http_body_util::{BodyExt, Full};
use hyper::{body::Incoming, Request, Response};
use std::sync::Arc;

lazy_static::lazy_static! {
    static ref REPOSITORY: Arc<Repository> = {
        let repo = Repository::new();
        Arc::new(repo)
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

    if path.ends_with("/") || path == "/svn" {
        response.push_str(&format!(
            r#"  <D:response><D:href>{}</D:href><D:propstat><D:prop>
<D:resourcetype><D:collection/></D:resourcetype>
<D:version-controlled-configuration><D:href>/svn/!svn/vcc/default</D:href></D:version-controlled-configuration>
</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>
"#, path
        ));

        if depth != "0" {
            if let Ok(entries) = REPOSITORY.list_dir(path, current_rev).await {
                for entry in entries {
                    response.push_str(&format!(
                        r#"  <D:response><D:href>{}{}</D:href><D:propstat>
<D:prop><D:resourcetype></D:resourcetype></D:prop><D:status>HTTP/1.1 200 OK</D:status>
</D:propstat></D:response>
"#, path.trim_end_matches('/'), entry
                    ));
                }
            }
        }
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
    if path.ends_with("/") || path == "/svn" {
        return Ok(Response::builder().status(405).header("Allow", "PROPFIND").body(Full::new(Bytes::from("Use PROPFIND"))).unwrap());
    }
    match REPOSITORY.get_file(path, REPOSITORY.current_rev().await).await {
        Ok(content) => Ok(Response::builder().status(200).header("Content-Type", "application/octet-stream").body(Full::new(content)).unwrap()),
        Err(_) => Ok(Response::builder().status(404).body(Full::new(Bytes::from("Not found"))).unwrap()),
    }
}

// Stub implementations for other handlers
pub async fn proppatch_handler(_req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    Ok(Response::builder().status(207).body(Full::new(Bytes::from(r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:"></D:multistatus>"#))).unwrap())
}
pub async fn checkout_handler(_req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    Ok(Response::builder().status(200).body(Full::new(Bytes::new())).unwrap())
}
pub async fn mkactivity_handler(_req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    Ok(Response::builder().status(201).header("Location", format!("/svn/!svn/act/{}", uuid::Uuid::new_v4())).body(Full::new(Bytes::new())).unwrap())
}
pub async fn mkcol_handler(_req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    Ok(Response::builder().status(201).body(Full::new(Bytes::new())).unwrap())
}
pub async fn delete_handler(_req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    Ok(Response::builder().status(204).body(Full::new(Bytes::new())).unwrap())
}
pub async fn put_handler(_req: Request<Incoming>, _config: &Config) -> Result<Response<Full<Bytes>>, WebDavError> {
    Ok(Response::builder().status(201).body(Full::new(Bytes::new())).unwrap())
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
