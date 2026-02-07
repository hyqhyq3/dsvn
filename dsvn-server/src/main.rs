//! DSvn Server - High-Performance SVN-Compatible Server
//!
//! A Rust implementation of an SVN server optimized for:
//! - Billions of files
//! - Millions of commits
//! - High-throughput parallel operations

use anyhow::Result;
use clap::{Parser, Subcommand};
use dsvn_webdav::{Config, WebDavHandler};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use http_body_util::Full;
use bytes::Bytes;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

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
    /// Start the server
    Start {
        /// Listen address (e.g., 0.0.0.0:8080)
        #[arg(short, long, default_value = "0.0.0.0:8080")]
        addr: String,

        /// Repository root directory (for future use)
        #[arg(short, long, default_value = "./data/repo")]
        repo_root: String,

        /// Enable debug logging
        #[arg(long)]
        debug: bool,
    },

    /// Initialize a new repository
    Init {
        /// Repository path (for future use)
        path: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start { addr, repo_root, debug } => {
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
            info!("Repository root: {}", repo_root);
            info!("Initializing disk-persistent repository");

            // Initialize the disk repository
            let repo_path = std::path::Path::new(&repo_root);
            dsvn_webdav::init_repository(repo_path)
                .expect("Failed to open disk repository");
            dsvn_webdav::init_repository_async().await
                .expect("Failed to initialize disk repository");
            info!("Disk repository initialized at {}", repo_root);

            // Create WebDAV handler
            let config = Config {
                repo_root: repo_root.clone(),
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
            println!("Note: MVP mode uses in-memory storage");
            println!("To initialize a persistent repository, run:");
            println!("  mkdir -p {}", path);
            println!("  dsvn start --repo-root {}", path);
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
    info!("Request Headers:");
    for (name, value) in req.headers().iter() {
        info!("  {}: {}", name, value.to_str().unwrap_or("<binary>"));
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
    info!("Response Headers:");
    for (name, value) in response.headers().iter() {
        info!("  {}: {}", name, value.to_str().unwrap_or("<binary>"));
    }

    Ok(response)
}
