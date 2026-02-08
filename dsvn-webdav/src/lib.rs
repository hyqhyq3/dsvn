//! DSvn WebDAV/HTTP Protocol Implementation
//!
//! Implements the WebDAV/DeltaV protocol used by SVN over HTTP

pub mod handlers;
pub mod proppatch;
pub mod sync_handlers;
pub mod xml;

pub use handlers::{report_handler, propfind_handler, options_handler, init_repository, init_repository_async, get_repo_arc};

use hyper::{body::Incoming, Request, Response};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use std::sync::Arc;

/// WebDAV configuration
#[derive(Debug, Clone)]
pub struct Config {
    /// Repository root path
    pub repo_root: String,
    /// Maximum request body size (bytes)
    pub max_body_size: usize,
    /// Enable compression
    pub compression: bool,
    /// Enable debug logging
    pub debug: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            repo_root: "/svn".to_string(),
            max_body_size: 100 * 1024 * 1024, // 100 MB
            compression: true,
            debug: false,
        }
    }
}

/// WebDAV request handler
pub struct WebDavHandler {
    config: Config,
}

impl WebDavHandler {
    /// Create a new handler with default config
    pub fn new() -> Self {
        Self {
            config: Config::default(),
        }
    }

    /// Create a new handler with custom config
    pub fn with_config(config: Config) -> Self {
        Self { config }
    }

    /// Handle an incoming HTTP request
    pub async fn handle(&self, req: Request<hyper::body::Incoming>) -> Result<Response<Full<Bytes>>, WebDavError> {
        let method = req.method().clone();
        let uri = req.uri().clone();

        tracing::debug!("WebDAV request: {} {}", method, uri);

        // ── Sync endpoints (/sync/*) ──
        let path = uri.path();
        if path.starts_with("/sync/") || path == "/sync" {
            let sync_path = path.strip_prefix("/sync").unwrap_or("");
            let query = uri.query().unwrap_or("");
            let method_str = method.as_str();

            // Read body for POST
            let body_bytes = if method_str == "POST" {
                match req.into_body().collect().await {
                    Ok(c) => c.to_bytes().to_vec(),
                    Err(e) => {
                        return Ok(Response::builder()
                            .status(400)
                            .body(Full::new(Bytes::from(format!("Bad request: {}", e))))
                            .unwrap());
                    }
                }
            } else {
                let _ = req.into_body();
                vec![]
            };

            let repo = handlers::get_repo_arc();
            return Ok(sync_handlers::handle_sync_request(
                sync_path,
                method_str,
                &body_bytes,
                query,
                &repo,
            )
            .await);
        }

        // Route to appropriate handler
        match method.as_str() {
            "OPTIONS" => self.handle_options(req).await,
            "PROPFIND" => self.handle_propfind(req).await,
            "PROPPATCH" => self.handle_proppatch(req).await,
            "REPORT" => self.handle_report(req).await,
            "MERGE" => self.handle_merge(req).await,
            "CHECKOUT" => self.handle_checkout(req).await,
            "MKACTIVITY" => self.handle_mkactivity(req).await,
            "MKCOL" => self.handle_mkcol(req).await,
            "DELETE" => self.handle_delete(req).await,
            "PUT" => self.handle_put(req).await,
            "GET" => self.handle_get(req).await,
            "HEAD" => self.handle_head(req).await,
            "LOCK" => self.handle_lock(req).await,
            "UNLOCK" => self.handle_unlock(req).await,
            "COPY" => self.handle_copy(req).await,
            "MOVE" => self.handle_move(req).await,
            "POST" => self.handle_post(req).await,
            _ => Ok(Response::builder()
                .status(405)
                .body(Full::new(Bytes::from("Method Not Allowed")))
                .unwrap()),
        }
    }

    /// Handle PROPFIND requests (retrieve properties)
    async fn handle_propfind(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, WebDavError> {
        handlers::propfind_handler(req, &self.config).await
    }

    /// Handle PROPPATCH requests (modify properties)
    async fn handle_proppatch(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, WebDavError> {
        handlers::proppatch_handler(req, &self.config).await
    }

    /// Handle REPORT requests (SVN-specific logs, diffs, etc)
    async fn handle_report(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, WebDavError> {
        handlers::report_handler(req, &self.config).await
    }

    /// Handle MERGE requests (commits)
    async fn handle_merge(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, WebDavError> {
        handlers::merge_handler(req, &self.config).await
    }

    /// Handle CHECKOUT requests
    async fn handle_checkout(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, WebDavError> {
        handlers::checkout_handler(req, &self.config).await
    }

    /// Handle MKACTIVITY requests (create transaction)
    async fn handle_mkactivity(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, WebDavError> {
        handlers::mkactivity_handler(req, &self.config).await
    }

    /// Handle MKCOL requests (create collection/directory)
    async fn handle_mkcol(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, WebDavError> {
        handlers::mkcol_handler(req, &self.config).await
    }

    /// Handle DELETE requests
    async fn handle_delete(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, WebDavError> {
        handlers::delete_handler(req, &self.config).await
    }

    /// Handle PUT requests
    async fn handle_put(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, WebDavError> {
        handlers::put_handler(req, &self.config).await
    }

    /// Handle GET requests
    async fn handle_get(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, WebDavError> {
        handlers::get_handler(req, &self.config).await
    }

    /// Handle LOCK requests
    async fn handle_lock(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, WebDavError> {
        handlers::lock_handler(req, &self.config).await
    }

    /// Handle UNLOCK requests
    async fn handle_unlock(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, WebDavError> {
        handlers::unlock_handler(req, &self.config).await
    }

    /// Handle COPY requests
    async fn handle_copy(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, WebDavError> {
        handlers::copy_handler(req, &self.config).await
    }

    /// Handle MOVE requests
    async fn handle_move(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, WebDavError> {
        handlers::move_handler(req, &self.config).await
    }

    /// Handle OPTIONS requests (SVN capability discovery)
    async fn handle_options(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, WebDavError> {
        handlers::options_handler(req, &self.config).await
    }

    /// Handle POST requests (create-txn)
    async fn handle_post(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, WebDavError> {
        handlers::post_handler(req, &self.config).await
    }

    /// Handle HEAD requests
    async fn handle_head(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, WebDavError> {
        handlers::head_handler(req, &self.config).await
    }
}

/// WebDAV errors
#[derive(Debug, thiserror::Error)]
pub enum WebDavError {
    #[error("HTTP error: {0}")]
    Http(String),

    #[error("XML parsing error: {0}")]
    Xml(String),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Authentication required")]
    Unauthorized,

    #[error("Forbidden: {0}")]
    Forbidden(String),

    #[error("Internal server error: {0}")]
    Internal(String),
}

impl WebDavHandler {
    pub fn default() -> Self {
        Self::new()
    }
}
