//! Authentication providers for DSvn
//!
//! Supports multiple authentication backends including:
//! - Htpasswd file-based authentication
//! - (Future) LDAP
//! - (Future) OAuth2
//! - (Future) Custom user-defined providers

use async_trait::async_trait;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Authentication result
#[derive(Debug, Clone, PartialEq)]
pub enum AuthResult {
    /// Authentication successful with the username
    Success(String),
    /// Authentication failed
    Failed,
    /// Authentication error (e.g., file not found)
    Error(String),
}

/// Authentication provider trait
///
/// All authentication providers must implement this trait.
#[async_trait]
pub trait AuthProvider: Send + Sync {
    /// Authenticate a user with the given credentials
    async fn authenticate(&self, username: &str, password: &str) -> AuthResult;
}

/// Htpasswd-based authentication provider
///
/// Supports Apache-style htpasswd file formats:
/// - bcrypt ($2y$, $2b$, $2a$)
/// - SHA1 ({SHA})
/// - crypt (deprecated, not recommended)
///
/// Example htpasswd file:
/// ```text
/// admin:$2y$05$rGZz6f3q9W5v7xY8zM2mLe8wW0k9vY6gQ4hMxLkNnMzKqPpJrWQ6e
/// user1:$2y$05$rGZz6f3q9W5v7xY8zM2mLe8wW0k9vY6gQ4hMxLkNnMzKqPpJrWQ6e
/// ```
#[derive(Clone)]
pub struct HtpasswdAuthProvider {
    users: HashMap<String, String>, // username -> password_hash
}

impl HtpasswdAuthProvider {
    /// Create a new htpasswd provider from a file
    ///
    /// # Arguments
    /// * `path` - Path to the htpasswd file
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or parsed
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let content = fs::read_to_string(path.as_ref())
            .map_err(|e| format!("Failed to read htpasswd file: {}", e))?;

        Self::parse(&content)
    }

    /// Parse htpasswd content
    fn parse(content: &str) -> Result<Self, String> {
        let mut users = HashMap::new();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Format: username:hash
            let parts: Vec<&str> = line.splitn(2, ':').collect();
            if parts.len() != 2 {
                return Err(format!("Invalid htpasswd line: {}", line));
            }

            let username = parts[0].trim();
            let hash = parts[1].trim();

            if username.is_empty() || hash.is_empty() {
                return Err(format!("Invalid htpasswd line (empty username or hash): {}", line));
            }

            users.insert(username.to_string(), hash.to_string());
        }

        if users.is_empty() {
            return Err("No valid users found in htpasswd file".to_string());
        }

        Ok(Self { users })
    }

    /// Create a new htpasswd provider from raw content
    pub fn from_content(content: &str) -> Result<Self, String> {
        Self::parse(content)
    }

    /// Verify a bcrypt password hash
    fn verify_bcrypt(hash: &str, password: &str) -> Result<bool, String> {
        // Check hash prefix
        if !hash.starts_with("$2y$") && !hash.starts_with("$2b$") && !hash.starts_with("$2a$") {
            return Ok(false);
        }

        // Parse bcrypt hash
        // Format: $2a$05$salt...hash...
        // Note: Rust bcrypt crate requires specific format

        #[cfg(feature = "bcrypt")]
        {
            use bcrypt::{hash, verify, DEFAULT_COST};
            return verify(password, hash).map_err(|e| e.to_string());
        }

        #[cfg(not(feature = "bcrypt"))]
        {
            // Simple verification without bcrypt crate (for testing only)
            // In production, you should enable the "bcrypt" feature
            return Err("bcrypt verification requires the 'bcrypt' feature".to_string());
        }
    }

    /// Verify a SHA1 password hash
    fn verify_sha1(hash: &str, password: &str) -> Result<bool, String> {
        if !hash.starts_with("{SHA}") {
            return Ok(false);
        }

        use sha1::{Digest, Sha1};

        let expected = &hash[5..]; // Skip {SHA} prefix

        let mut hasher = Sha1::new();
        hasher.update(password.as_bytes());
        let result = hasher.finalize();

        let computed = base64::encode(&result);

        Ok(computed == expected)
    }

    /// Verify a password against the stored hash
    fn verify_password(&self, username: &str, password: &str) -> Result<bool, String> {
        let hash = self.users.get(username)
            .ok_or_else(|| format!("User not found: {}", username))?;

        // Try bcrypt first (most common)
        if hash.starts_with("$2") {
            return Self::verify_bcrypt(hash, password);
        }

        // Try SHA1
        if hash.starts_with("{SHA}") {
            return Self::verify_sha1(hash, password);
        }

        // Try crypt (deprecated, basic implementation)
        if !hash.starts_with('$') {
            return Self::verify_crypt(hash, password);
        }

        Err(format!("Unsupported password hash format for user: {}", username))
    }

    /// Verify a crypt-style password hash (deprecated)
    fn verify_crypt(_hash: &str, _password: &str) -> Result<bool, String> {
        // This is a placeholder for crypt support
        // In production, you would use a proper crypt implementation
        Err("crypt password hashes are deprecated and not supported".to_string())
    }
}

#[async_trait]
impl AuthProvider for HtpasswdAuthProvider {
    async fn authenticate(&self, username: &str, password: &str) -> AuthResult {
        if username.is_empty() || password.is_empty() {
            return AuthResult::Failed;
        }

        match self.verify_password(username, password) {
            Ok(true) => AuthResult::Success(username.to_string()),
            Ok(false) => AuthResult::Failed,
            Err(e) => AuthResult::Error(e),
        }
    }
}

/// No-op authentication provider (allows all requests)
///
/// This is useful for testing or when authentication is handled by another layer
/// (e.g., reverse proxy).
#[derive(Clone, Default)]
pub struct NoOpAuthProvider;

#[async_trait]
impl AuthProvider for NoOpAuthProvider {
    async fn authenticate(&self, _username: &str, _password: &str) -> AuthResult {
        // Always succeed with the provided username
        AuthResult::Success(_username.to_string())
    }
}

/// Mock authentication provider for testing
///
/// Accepts any username with password "test123"
#[derive(Clone)]
pub struct MockAuthProvider;

#[async_trait]
impl AuthProvider for MockAuthProvider {
    async fn authenticate(&self, username: &str, password: &str) -> AuthResult {
        if password == "test123" && !username.is_empty() {
            AuthResult::Success(username.to_string())
        } else {
            AuthResult::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_htpasswd() {
        let content = r#"
            admin:$2y$05$rGZz6f3q9W5v7xY8zM2mLe8wW0k9vY6gQ4hMxLkNnMzKqPpJrWQ6e
            user1:$2y$05$rGZz6f3q9W5v7xY8zM2mLe8wW0k9vY6gQ4hMxLkNnMzKqPpJrWQ6e
        "#;

        let provider = HtpasswdAuthProvider::from_content(content).unwrap();
        assert!(provider.users.contains_key("admin"));
        assert!(provider.users.contains_key("user1"));
    }

    #[test]
    fn test_parse_htpasswd_empty() {
        let content = "\n# Comment\n\n";

        let result = HtpasswdAuthProvider::from_content(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_htpasswd_invalid() {
        let content = "invalid_line_without_colon";

        let result = HtpasswdAuthProvider::from_content(content);
        assert!(result.is_err());
    }
}
