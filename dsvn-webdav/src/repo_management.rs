//! Repository Management API
//!
//! HTTP endpoints for managing repositories in multi-repository mode.

use bytes::Bytes;
use dsvn_core::SqliteRepository;
use http_body_util::{BodyExt, Full};
use hyper::{Response};
use std::sync::Arc;

use crate::{
    handlers::RepositoryRegistry,
    WebDavError,
};

/// Request body for creating a repository
#[derive(Debug, serde::Deserialize)]
struct CreateRepoRequest {
    name: String,
    path: String,
    display_name: Option<String>,
    description: Option<String>,
}

/// Repository information for API responses
#[derive(Debug, serde::Serialize)]
struct RepositoryInfo {
    name: String,
    path: String,
    display_name: Option<String>,
    description: Option<String>,
    uuid: String,
}

/// Handle POST /svn/_api/repos - Create a new repository
pub async fn handle_create_repo(
    body: Vec<u8>,
    registry: &mut RepositoryRegistry,
) -> Result<Response<Full<Bytes>>, WebDavError> {
    // Parse request body
    let request: CreateRepoRequest = serde_json::from_slice(&body)
        .map_err(|e| WebDavError::InvalidRequest(format!("Invalid JSON: {}", e)))?;

    // Validate name
    let name = request.name.trim();
    if name.is_empty() {
        return Ok(Response::builder()
            .status(400)
            .header("Content-Type", "application/json")
            .body(Full::new(Bytes::from(
                r#"{"error": "Repository name cannot be empty"}"#
            )))
            .unwrap());
    }

    // Validate path
    let path = std::path::Path::new(&request.path);
    if !path.is_absolute() {
        return Ok(Response::builder()
            .status(400)
            .header("Content-Type", "application/json")
            .body(Full::new(Bytes::from(
                r#"{"error": "Repository path must be absolute"}"#
            )))
            .unwrap());
    }

    // Check if repository with this name already exists
    if registry.get(name).is_some() {
        return Ok(Response::builder()
            .status(409)
            .header("Content-Type", "application/json")
            .body(Full::new(Bytes::from(
                serde_json::json!({
                    "error": format!("Repository '{}' already exists", name)
                })
                .to_string()
            )))
            .unwrap());
    }

    // Create parent directory if needed
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| WebDavError::Internal(format!("Failed to create directory: {}", e)))?;
    }

    // Open repository
    let repo = SqliteRepository::open(path)
        .map_err(|e| WebDavError::Internal(format!("Failed to open repository: {}", e)))?;

    // Initialize repository
    let repo_arc = Arc::new(repo);
    repo_arc.initialize()
        .await
        .map_err(|e| WebDavError::Internal(format!("Failed to initialize repository: {}", e)))?;

    // Get UUID
    let uuid = repo_arc.uuid().to_string();

    // Register in the registry
    registry.register(name, repo_arc)
        .map_err(|e| WebDavError::Internal(format!("Failed to register repository: {}", e)))?;

    // Build response
    let response_info = RepositoryInfo {
        name: name.to_string(),
        path: path.to_string_lossy().to_string(),
        display_name: request.display_name,
        description: request.description,
        uuid,
    };

    let response_json = serde_json::to_string(&response_info)
        .map_err(|e| WebDavError::Internal(format!("Failed to serialize response: {}", e)))?;

    Ok(Response::builder()
        .status(201)
        .header("Content-Type", "application/json")
        .body(Full::new(Bytes::from(response_json)))
        .unwrap())
}

/// Handle DELETE /svn/_api/repos/{repo-name} - Delete a repository from the registry
pub async fn handle_delete_repo(
    repo_name: &str,
    registry: &mut RepositoryRegistry,
) -> Result<Response<Full<Bytes>>, WebDavError> {
    let name = repo_name.trim();

    if name.is_empty() {
        return Ok(Response::builder()
            .status(400)
            .header("Content-Type", "application/json")
            .body(Full::new(Bytes::from(
                r#"{"error": "Repository name cannot be empty"}"#
            )))
            .unwrap());
    }

    // Check if repository exists
    if registry.get(name).is_none() {
        return Ok(Response::builder()
            .status(404)
            .header("Content-Type", "application/json")
            .body(Full::new(Bytes::from(
                serde_json::json!({
                    "error": format!("Repository '{}' not found", name)
                })
                .to_string()
            )))
            .unwrap());
    }

    // Remove from registry (does not delete files on disk for safety)
    registry.unregister(name)
        .map_err(|e| WebDavError::Internal(format!("Failed to unregister repository: {}", e)))?;

    Ok(Response::builder()
        .status(204)
        .body(Full::new(Bytes::new()))
        .unwrap())
}

/// Handle GET /svn/_api/repos - List all repositories
pub async fn handle_list_repos(
    registry: &RepositoryRegistry,
) -> Result<Response<Full<Bytes>>, WebDavError> {
    let repo_names = registry.list();

    let mut repos = Vec::new();
    for name in repo_names {
        if let Some(repo) = registry.get(name) {
            let info = RepositoryInfo {
                name: name.to_string(),
                path: repo.root().to_string_lossy().to_string(),
                display_name: None,
                description: None,
                uuid: repo.uuid().to_string(),
            };
            repos.push(info);
        }
    }

    let response_json = serde_json::to_string(&repos)
        .map_err(|e| WebDavError::Internal(format!("Failed to serialize response: {}", e)))?;

    Ok(Response::builder()
        .status(200)
        .header("Content-Type", "application/json")
        .body(Full::new(Bytes::from(response_json)))
        .unwrap())
}
