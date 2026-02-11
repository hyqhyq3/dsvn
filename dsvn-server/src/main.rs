//! DSvn Server - High-Performance SVN-Compatible Server
//!
//! A Rust implementation of an SVN server optimized for:
//! - Billions of files
//! - Millions of commits
//! - High-throughput parallel operations
//! - Multi-repository support

use anyhow::{Result, anyhow};
use clap::{Parser, Subcommand};
use dsvn_webdav::{Config, MultiRepoConfig, RepoConfig, RepositoryRegistry, WebDavHandler};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use http_body_util::Full;
use bytes::Bytes;
use std::net::SocketAddr;
use std::sync::Arc;
use std::path::Path;
use tokio::net::TcpListener;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use dsvn_core::SqliteRepository;

/// DSvn Server Configuration
#[derive(Parser, Debug)]
#[command(name = "dsvn")]
#[command(author = "DSvn Contributors")]
#[command(version = "0.1.0")]
#[command(about = "High-performance SVN-compatible server", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start server
    Start {
        /// Listen address (e.g., 0.0.0.0:8080)
        #[arg(short, long, default_value = "0.0.0.0:8080")]
        addr: String,

        /// Repository root directory (legacy single-repo mode)
        #[arg(short, long, default_value = "./data/repo")]
        repo_root: String,

        /// Configuration file for multi-repository mode (TOML format)
        #[arg(short = 'c', long)]
        config: Option<String>,

        /// Enable debug logging
        #[arg(long)]
        debug: bool,
    },

    /// Initialize a new repository
    Init {
        /// Repository path
        path: String,
    },

    /// Create multi-repository configuration file
    #[command(name = "init-config")]
    InitConfig {
        /// Config file path
        #[arg(short, long, default_value = "dsvn.toml")]
        output: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start { addr, repo_root, config, debug } => {
            // Initialize tracing
            let env_filter = if debug {
                tracing_subscriber::EnvFilter::new("debug")
            } else {
                tracing_subscriber::EnvFilter::from_default_env()
                    .add_directive(tracing::Level::INFO.into())
            };

            tracing_subscriber::registry()
                .with(tracing_subscriber::fmt::layer())
                .with(env_filter)
                .init();

            info!("Starting DSvn server on {}", addr);

            // Load configuration
            let multi_repo_config = if let Some(config_path) = config {
                let path = Path::new(&config_path);
                if path.exists() {
                    info!("Loading multi-repository config from {}", config_path);
                    MultiRepoConfig::from_file(path)
                        .map_err(|e| anyhow!("Failed to load config: {}", e))?
                } else {
                    warn!("Config file not found: {}, using legacy single-repo mode", config_path);
                    MultiRepoConfig::default()
                }
            } else {
                MultiRepoConfig::default()
            };

            // Initialize repositories
            if multi_repo_config.multi_repo && !multi_repo_config.repositories.is_empty() {
                info!("Multi-repository mode enabled");
                let mut registry = RepositoryRegistry::new();

                for (name, repo_config) in &multi_repo_config.repositories {
                    let repo_path = Path::new(&repo_config.path);
                    info!("  Registering repository '{}': {}", name, repo_config.path);

                    let repo = SqliteRepository::open(repo_path)
                        .map_err(|e| anyhow!("Failed to open repository '{}' at {:?}: {}", name, repo_path, e))?;

                    registry.register(name, Arc::new(repo))
                        .map_err(|e| anyhow!("Failed to register repository '{}': {}", name, e))?;
                }

                dsvn_webdav::handlers::init_repository_registry(registry)
                    .map_err(|e| anyhow!("Failed to init repository registry: {}", e))?;

                // Initialize multi-repo configuration for display names
                let repo_config_map: std::collections::HashMap<String, RepoConfig> =
                    multi_repo_config.repositories.clone();
                dsvn_webdav::handlers::init_multi_repo_config(Arc::new(repo_config_map))
                    .map_err(|e| anyhow!("Failed to init multi-repo config: {}", e))?;

                info!("Initializing repositories...");
                dsvn_webdav::handlers::init_repository_registry_async().await
                    .map_err(|e| anyhow!("Failed to init repositories: {}", e))?;

                info!("Multi-repository server ready with {} repositories",
                       multi_repo_config.repositories.len());
            } else {
                info!("Single-repository mode");
                info!("Repository root: {}", repo_root);
                info!("Initializing SQLite-persistent repository");

                // Initialize SQLite repository
                let repo_path = Path::new(&repo_root);
                dsvn_webdav::init_repository(repo_path)
                    .expect("Failed to open SQLite repository");
                dsvn_webdav::init_repository_async().await
                    .expect("Failed to initialize SQLite repository");
                info!("SQLite repository initialized at {}", repo_root);
            }

            // Create WebDAV handler
            let config = Config {
                repo_root,
                multi_repo_config,
                max_body_size: 100 * 1024 * 1024,
                compression: true,
                debug,
            };

            let handler = Arc::new(WebDavHandler::with_config(config));

            // Start server
            let addr: SocketAddr = addr.parse()?;
            let listener: tokio::net::TcpListener = TcpListener::bind(addr).await?;

            info!("Server listening on {}", addr);
            info!("Ready to accept SVN client connections");

            // HTTP server (non-TLS for MVP)
            loop {
                let (stream, _) = listener.accept().await?;
                let handler = handler.clone();
                let io = TokioIo::new(stream);

                tokio::spawn(async move {
                    if let Err(e) = http1::Builder::new()
                        .serve_connection(io, service_fn(move |req| handle_request(req, handler.clone())))
                        .await
                    {
                        error!("Error serving connection: {:?}", e);
                    }
                });
            }
        }

        Commands::Init { path } => {
            println!("Initializing repository at {}", path);
            let repo_path = Path::new(&path);
            
            if repo_path.exists() {
                return Err(anyhow!("Repository already exists at {}", path));
            }
            
            let repo = SqliteRepository::open(repo_path)?;
            repo.initialize().await?;
            println!("Repository initialized successfully at {}", path);
            println!("UUID: {}", repo.uuid());
        }

        Commands::InitConfig { output } => {
            create_default_config(&output)?;
            println!("Configuration file created: {}", output);
            println!();
            println!("Edit the file to add your repositories, then start the server:");
            println!("  dsvn start --config {}", output);
        }
    }

    Ok(())
}

/// Handle incoming HTTP request
async fn handle_request(
    req: Request<hyper::body::Incoming>,
    handler: Arc<WebDavHandler>,
) -> Result<Response<Full<Bytes>>, hyper::Error> {
    // Log request details
    info!("Request: {} {}", req.method(), req.uri());
    if handler.config.debug {
        info!("Request Headers:");
        for (name, value) in req.headers().iter() {
            info!("  {}: {}", name, value.to_str().unwrap_or("<binary>"));
        }
    }

    // Handle request
    let response = match handler.handle(req).await {
        Ok(resp) => resp,
        Err(e) => {
            error!("Request error: {}", e);
            Response::builder()
                .status(500)
                .body(Full::new(Bytes::from(format!("Error: {}", e))))
                .unwrap()
        }
    };

    // Log response details
    info!("Response: {}", response.status());
    if handler.config.debug {
        info!("Response Headers:");
        for (name, value) in response.headers().iter() {
            info!("  {}: {}", name, value.to_str().unwrap_or("<binary>"));
        }
    }

    Ok(response)
}

/// Create a default multi-repository configuration file
fn create_default_config(path: &str) -> Result<()> {
    use std::fs;

    let config = dsvn_webdav::MultiRepoConfig {
        multi_repo: true,
        repositories: [
            ("repo1", RepoConfig::new("./data/repo1").with_display_name("Main Repository").with_description("Primary development repository")),
            ("repo2", RepoConfig::new("./data/repo2").with_display_name("Staging Repository").with_description("Staging repository for testing")),
        ].into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
    };

    config.to_file(Path::new(path))
        .map_err(|e| anyhow!("Failed to write config file: {}", e))?;

    Ok(())
}
