//! DSvn WebDAV/HTTP Protocol Implementation
//!
//! Implements the WebDAV/DeltaV protocol used by SVN over HTTP

pub mod handlers;
pub mod proppatch;
pub mod sync_handlers;
pub mod dump_handlers;
pub mod xml;
pub mod repo_management;

pub use handlers::{
    report_handler, propfind_handler, options_handler,
    init_repository, init_repository_async, get_repo_arc,
    RepositoryRegistry, init_repository_registry, init_repository_registry_async,
    init_multi_repo_config,
    get_repo_by_path,
};

use hyper::{body::Incoming, Request, Response};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use std::sync::Arc;
use std::collections::HashMap;

use dsvn_core::SqliteRepository;

/// Multi-repository configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MultiRepoConfig {
    /// Enable multi-repository mode
    #[serde(default)]
    pub multi_repo: bool,
    /// Repository registry (name -> path)
    #[serde(default)]
    pub repositories: HashMap<String, RepoConfig>,
}

impl Default for MultiRepoConfig {
    fn default() -> Self {
        Self {
            multi_repo: false,
            repositories: HashMap::new(),
        }
    }
}

impl MultiRepoConfig {
    /// Load from TOML file
    pub fn from_file(path: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        toml::from_str(&content).map_err(Into::into)
    }

    /// Save to TOML file
    pub fn to_file(&self, path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

/// Single repository configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RepoConfig {
    /// Repository path on disk
    pub path: String,
    /// Repository display name
    #[serde(default)]
    pub display_name: Option<String>,
    /// Repository description
    #[serde(default)]
    pub description: Option<String>,
}

impl RepoConfig {
    /// Create new config
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            display_name: None,
            description: None,
        }
    }

    /// With display name
    pub fn with_display_name(mut self, name: impl Into<String>) -> Self {
        self.display_name = Some(name.into());
        self
    }

    /// With description
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }
}

/// WebDAV configuration
#[derive(Debug, Clone)]
pub struct Config {
    /// Repository root path (legacy single-repo mode)
    pub repo_root: String,
    /// Multi-repository configuration
    pub multi_repo_config: MultiRepoConfig,
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
            multi_repo_config: MultiRepoConfig::default(),
            max_body_size: 100 * 1024 * 1024, // 100 MB
            compression: true,
            debug: false,
        }
    }
}

/// WebDAV request handler
pub struct WebDavHandler {
    pub config: Config,
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

        // ── Repository Management API (/_api/repos) ──
        let path = uri.path();
        if path.starts_with("/svn/_api/repos") {
            use repo_management::{handle_create_repo, handle_delete_repo, handle_list_repos};

            // Get reference to registry
            let registry = match handlers::REPOSITORY_REGISTRY.get() {
                Some(r) => r.clone(),
                None => {
                    // Create a new registry if not initialized
                    handlers::RepositoryRegistry::new()
                }
            };

            // Match on path and method
            if path == "/svn/_api/repos" {
                match method.as_str() {
                    "GET" => {
                        return handle_list_repos(&registry).await;
                    }
                    "POST" => {
                        let body_bytes = match req.into_body().collect().await {
                            Ok(c) => c.to_bytes().to_vec(),
                            Err(e) => {
                                return Ok(Response::builder()
                                    .status(400)
                                    .body(Full::new(Bytes::from(format!("Bad request: {}", e))))
                                    .unwrap());
                            }
                        };

                        // Clone registry for mutation
                        let mut registry_mut = registry.clone();
                        let response = handle_create_repo(body_bytes, &mut registry_mut).await?;

                        // Note: Changes to registry_mut are not persisted back to REPOSITORY_REGISTRY
                        // This is a limitation of the current OnceLock-based design
                        // For production use, REPOSITORY_REGISTRY should be wrapped in a Mutex/RwLock
                        return Ok(response);
                    }
                    _ => {
                        return Ok(Response::builder()
                            .status(405)
                            .body(Full::new(Bytes::from("Method Not Allowed")))
                            .unwrap());
                    }
                }
            } else if path.starts_with("/svn/_api/repos/") {
                let repo_name = path.strip_prefix("/svn/_api/repos/").unwrap_or("");
                if !repo_name.is_empty() {
                    match method.as_str() {
                        "DELETE" => {
                            // Clone registry for mutation
                            let mut registry_mut = registry.clone();
                            let response = handle_delete_repo(repo_name, &mut registry_mut).await?;

                            // Note: Changes to registry_mut are not persisted back to REPOSITORY_REGISTRY
                            // This is a limitation of the current OnceLock-based design
                            // For production use, REPOSITORY_REGISTRY should be wrapped in a Mutex/RwLock
                            return Ok(response);
                        }
                        _ => {
                            return Ok(Response::builder()
                                .status(405)
                                .body(Full::new(Bytes::from("Method Not Allowed")))
                                .unwrap());
                        }
                    }
                }
            }
        }

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

        // ── Dump/Load endpoints (svnrdump protocol) ──
        let accept_header = req.headers().get("accept")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let content_type_header = req.headers().get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        if method.as_str() == "GET" && dump_handlers::is_dump_request(&accept_header) {
            let query = uri.query().unwrap_or("");
            let repo = handlers::get_repo_arc();
            let _ = req.into_body();
            return Ok(dump_handlers::handle_dump(repo, query).await);
        }

        if method.as_str() == "POST" && dump_handlers::is_load_request(&content_type_header) {
            let repo = handlers::get_repo_arc();
            let body_bytes = match req.into_body().collect().await {
                Ok(c) => c.to_bytes().to_vec(),
                Err(e) => {
                    return Ok(Response::builder()
                        .status(400)
                        .body(Full::new(Bytes::from(format!("Bad request: {}", e))))
                        .unwrap());
                }
            };
            return Ok(dump_handlers::handle_load(repo, body_bytes).await);
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
