//! Authorization providers for DSvn
//!
//! Supports multiple authorization backends including:
//! - SVN-style authz configuration files
//! - (Future) Path-based ACL
//! - (Future) Group-based policies
//! - (Future) Role-based access control (RBAC)

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

/// Access level for repository paths
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccessLevel {
    /// No access
    None,
    /// Read-only access
    Read,
    /// Read/write access
    Write,
}

/// Authorization result
#[derive(Debug, Clone, PartialEq)]
pub enum AuthzResult {
    /// Access granted
    Granted,
    /// Access denied
    Denied,
    /// Authorization error
    Error(String),
}

/// Authorization provider trait
///
/// All authorization providers must implement this trait.
pub trait AuthzProvider: Send + Sync {
    /// Check if a user has access to a specific path with the required access level
    fn check_access(
        &self,
        username: Option<&str>,
        path: &str,
        required_level: AccessLevel,
    ) -> AuthzResult;

    /// Check if a user has read access to a path
    fn can_read(&self, username: Option<&str>, path: &str) -> bool {
        matches!(
            self.check_access(username, path, AccessLevel::Read),
            AuthzResult::Granted
        )
    }

    /// Check if a user has write access to a path
    fn can_write(&self, username: Option<&str>, path: &str) -> bool {
        matches!(
            self.check_access(username, path, AccessLevel::Write),
            AuthzResult::Granted
        )
    }
}

/// Group alias for user authorization
///
/// Groups can be defined in the authz file and used to grant access to multiple users.
pub type Group = String;

/// Access rule for a specific path and user/group
#[derive(Debug, Clone)]
struct AccessRule {
    /// The path this rule applies to
    path: String,
    /// The user this rule applies to (None for anonymous)
    user: Option<String>,
    /// The group this rule applies to (None if not a group rule)
    group: Option<String>,
    /// The access level granted
    level: AccessLevel,
}

/// SVN-style authorization provider
///
/// Supports Apache Subversion-style authz.conf files.
///
/// File format:
/// ```text
/// [groups]
/// developers = alice, bob
/// admins = charlie
///
/// [/]
/// * = r
/// @admins = rw
///
/// [/trunk/src]
/// @developers = rw
/// bob = r
///
/// [/private]
/// @admins = rw
/// * =
/// ```
#[derive(Clone)]
pub struct SvnAuthzProvider {
    /// Access rules indexed by path prefix
    rules: HashMap<String, Vec<AccessRule>>,
    /// Group definitions: group name -> set of users
    groups: HashMap<Group, HashSet<String>>,
    /// Default access level for paths with no specific rules
    default_access: AccessLevel,
}

impl SvnAuthzProvider {
    /// Create a new SVN authz provider from a file
    ///
    /// # Arguments
    /// * `path` - Path to the authz.conf file
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or parsed
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let content = fs::read_to_string(path.as_ref())
            .map_err(|e| format!("Failed to read authz file: {}", e))?;

        Self::parse(&content)
    }

    /// Parse SVN authz configuration content
    fn parse(content: &str) -> Result<Self, String> {
        let mut rules: HashMap<String, Vec<AccessRule>> = HashMap::new();
        let mut groups: HashMap<Group, HashSet<String>> = HashMap::new();

        let mut current_section: Option<String> = None;

        for line in content.lines() {
            let line = line.trim();

            // Skip empty lines and comments
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Parse section header: [section_name]
            if line.starts_with('[') && line.ends_with(']') {
                current_section = Some(line[1..line.len() - 1].to_string());
                continue;
            }

            // Parse group definition: group_name = user1, user2, user3
            if let Some(section) = &current_section {
                if section == "groups" {
                    Self::parse_group(line, &mut groups)?;
                    continue;
                }

                // Parse access rule: user_or_group = r|rw| (empty for no access)
                Self::parse_access_rule(section, line, &mut rules, &groups)?;
            }
        }

        Ok(Self {
            rules,
            groups,
            default_access: AccessLevel::None,
        })
    }

    /// Parse a group definition
    ///
    /// Format: group_name = user1, user2, user3
    fn parse_group(
        line: &str,
        groups: &mut HashMap<Group, HashSet<String>>,
    ) -> Result<(), String> {
        let parts: Vec<&str> = line.splitn(2, '=').collect();
        if parts.len() != 2 {
            return Err(format!("Invalid group definition: {}", line));
        }

        let group_name = parts[0].trim().to_string();
        let users_str = parts[1].trim();

        if group_name.is_empty() {
            return Err("Group name cannot be empty".to_string());
        }

        let users: HashSet<String> = users_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        if users.is_empty() {
            return Err(format!("No users in group: {}", group_name));
        }

        groups.insert(group_name, users);
        Ok(())
    }

    /// Parse an access rule
    ///
    /// Format: user_or_group = r|rw| (empty for no access)
    fn parse_access_rule(
        path: &str,
        line: &str,
        rules: &mut HashMap<String, Vec<AccessRule>>,
        groups: &HashMap<Group, HashSet<String>>,
    ) -> Result<(), String> {
        let parts: Vec<&str> = line.splitn(2, '=').collect();
        if parts.len() != 2 {
            return Err(format!("Invalid access rule: {}", line));
        }

        let user_or_group = parts[0].trim();
        let access_str = parts[1].trim().to_lowercase();

        // Check if this is a group reference
        let (user, group) = if let Some(group_name) = user_or_group.strip_prefix('@') {
            // Group reference: @group_name
            (None, Some(group_name.to_string()))
        } else if user_or_group == "*" {
            // Anonymous user: *
            (None, None)
        } else {
            // Regular user
            (Some(user_or_group.to_string()), None)
        };

        // Parse access level
        let level = match access_str.as_str() {
            "r" | "read" => AccessLevel::Read,
            "rw" | "read-write" => AccessLevel::Write,
            "w" | "write" => AccessLevel::Write,
            "" => AccessLevel::None,
            _ => {
                return Err(format!("Invalid access level: {}", access_str));
            }
        };

        // Validate group exists if referenced
        if let Some(group_name) = &group {
            if !groups.contains_key(group_name) {
                return Err(format!("Group not found: {}", group_name));
            }
        }

        let rule = AccessRule {
            path: path.to_string(),
            user,
            group,
            level,
        };

        rules.entry(path.to_string())
            .or_insert_with(Vec::new)
            .push(rule);

        Ok(())
    }

    /// Create a new SVN authz provider from raw content
    pub fn from_content(content: &str) -> Result<Self, String> {
        Self::parse(content)
    }

    /// Set the default access level for paths with no specific rules
    pub fn with_default_access(mut self, default: AccessLevel) -> Self {
        self.default_access = default;
        self
    }

    /// Get all rules that apply to a given path
    fn get_applicable_rules(&self, path: &str) -> Vec<&AccessRule> {
        let mut applicable = Vec::new();

        // Check for exact path match
        if let Some(rules) = self.rules.get(path) {
            for rule in rules {
                applicable.push(rule);
            }
        }

        // Check for parent path matches
        let mut parent_path = path;
        while let Some(pos) = parent_path.rfind('/') {
            parent_path = &parent_path[..pos];
            let search_path = if parent_path.is_empty() { "/" } else { parent_path };

            if let Some(rules) = self.rules.get(search_path) {
                for rule in rules {
                    applicable.push(rule);
                }
            }
        }

        // Check root path [/] if not already checked
        if !path.is_empty() && path != "/" {
            if let Some(rules) = self.rules.get("/") {
                for rule in rules {
                    applicable.push(rule);
                }
            }
        }

        applicable
    }

    /// Find the most specific access level for a user at a path
    fn find_access_level(&self, username: Option<&str>, path: &str) -> AccessLevel {
        let mut best_level = self.default_access;

        // Get all applicable rules
        let rules = self.get_applicable_rules(path);

        // Process rules, with more specific paths taking precedence
        let mut path_specific_rules: Vec<(&AccessRule, usize)> = rules
            .iter()
            .map(|rule| {
                let specificity = if rule.path == path {
                    1000 // Exact match
                } else if rule.path.ends_with("/*") || rule.path == "/" {
                    500 // Parent path
                } else {
                    0 // Other
                };
                (*rule, specificity)
            })
            .collect();

        // Sort by specificity (higher = more specific)
        path_specific_rules.sort_by(|a, b| b.1.cmp(&a.1));

        // Find the first matching rule for the user
        for (rule, _specificity) in path_specific_rules {
            // Check if this rule matches the user
            let matches_user = match (&rule.user, &rule.group) {
                (Some(user), None) => {
                    // Specific user
                    username.map(|u| u == user).unwrap_or(false)
                }
                (None, Some(group)) => {
                    // Group reference
                    if let Some(username) = username {
                        self.groups.get(group)
                            .map(|users| users.contains(username))
                            .unwrap_or(false)
                    } else {
                        false
                    }
                }
                (None, None) => {
                    // Anonymous (*)
                    username.is_none()
                }
                _ => false,
            };

            if matches_user {
                best_level = rule.level;
                break;
            }
        }

        best_level
    }
}

impl AuthzProvider for SvnAuthzProvider {
    fn check_access(
        &self,
        username: Option<&str>,
        path: &str,
        required_level: AccessLevel,
    ) -> AuthzResult {
        let granted_level = self.find_access_level(username, path);

        // Normalize path for comparison
        let normalized_path = if path.is_empty() {
            "/"
        } else {
            path
        };

        // Check if the granted level meets the required level
        let has_access = match (granted_level, required_level) {
            (AccessLevel::Write, _) => true, // Write access grants both read and write
            (AccessLevel::Read, AccessLevel::Read) => true,
            (AccessLevel::None, _) => false,
            _ => false,
        };

        if has_access {
            AuthzResult::Granted
        } else {
            AuthzResult::Denied
        }
    }
}

/// No-op authorization provider (allows all requests)
///
/// This is useful for testing or when authorization is handled by another layer
/// (e.g., reverse proxy).
#[derive(Clone, Default)]
pub struct NoOpAuthzProvider;

impl AuthzProvider for NoOpAuthzProvider {
    fn check_access(
        &self,
        _username: Option<&str>,
        _path: &str,
        _required_level: AccessLevel,
    ) -> AuthzResult {
        AuthzResult::Granted
    }
}

/// Deny-all authorization provider (denies all requests)
///
/// This is useful as a default safe configuration.
#[derive(Clone, Default)]
pub struct DenyAllAuthzProvider;

impl AuthzProvider for DenyAllAuthzProvider {
    fn check_access(
        &self,
        _username: Option<&str>,
        _path: &str,
        _required_level: AccessLevel,
    ) -> AuthzResult {
        AuthzResult::Denied
    }
}

/// Mock authorization provider for testing
///
/// Rules:
/// - admin: read/write to all paths
/// - user: read-only to all paths
/// - anonymous: read-only to public paths (starting with /public)
#[derive(Clone)]
pub struct MockAuthzProvider;

impl AuthzProvider for MockAuthzProvider {
    fn check_access(
        &self,
        username: Option<&str>,
        path: &str,
        required_level: AccessLevel,
    ) -> AuthzResult {
        match username {
            Some("admin") => AuthzResult::Granted, // Admin has full access
            Some("user") => {
                // Regular user can read, cannot write
                if required_level == AccessLevel::Read {
                    AuthzResult::Granted
                } else {
                    AuthzResult::Denied
                }
            }
            None => {
                // Anonymous user can only read public paths
                if path.starts_with("/public") && required_level == AccessLevel::Read {
                    AuthzResult::Granted
                } else {
                    AuthzResult::Denied
                }
            }
            _ => AuthzResult::Denied, // Other users denied
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_groups() {
        let content = r#"
            [groups]
            developers = alice, bob
            admins = charlie
        "#;

        let provider = SvnAuthzProvider::from_content(content).unwrap();
        assert!(provider.groups.contains_key("developers"));
        assert!(provider.groups.contains_key("admins"));

        let developers = provider.groups.get("developers").unwrap();
        assert!(developers.contains("alice"));
        assert!(developers.contains("bob"));
    }

    #[test]
    fn test_parse_access_rules() {
        let content = r#"
            [/]
            * = r
            @admins = rw

            [/private]
            @admins = rw
            * =
        "#;

        let provider = SvnAuthzProvider::from_content(content).unwrap();
        assert!(provider.rules.contains_key("/"));
        assert!(provider.rules.contains_key("/private"));
    }

    #[test]
    fn test_access_check_user() {
        let content = r#"
            [groups]
            admins = charlie

            [/]
            * = r
            @admins = rw

            [/private]
            @admins = rw
            * =
        "#;

        let provider = SvnAuthzProvider::from_content(content).unwrap();

        // Anonymous user can read root
        assert!(matches!(
            provider.check_access(None, "/", AccessLevel::Read),
            AuthzResult::Granted
        ));

        // Anonymous user cannot write root
        assert!(matches!(
            provider.check_access(None, "/", AccessLevel::Write),
            AuthzResult::Denied
        ));

        // Admin can read and write everywhere
        assert!(matches!(
            provider.check_access(Some("charlie"), "/", AccessLevel::Write),
            AuthzResult::Granted
        ));
        assert!(matches!(
            provider.check_access(Some("charlie"), "/private", AccessLevel::Write),
            AuthzResult::Granted
        ));

        // Anonymous user cannot access private
        assert!(matches!(
            provider.check_access(None, "/private", AccessLevel::Read),
            AuthzResult::Denied
        ));
    }

    #[test]
    fn test_access_check_specific_user() {
        let content = r#"
            [/]
            * = r
            bob = rw

            [/private]
            bob = r
        "#;

        let provider = SvnAuthzProvider::from_content(content).unwrap();

        // Bob can write to root
        assert!(matches!(
            provider.check_access(Some("bob"), "/", AccessLevel::Write),
            AuthzResult::Granted
        ));

        // Other users cannot write to root
        assert!(matches!(
            provider.check_access(Some("alice"), "/", AccessLevel::Write),
            AuthzResult::Denied
        ));

        // Bob can only read private
        assert!(matches!(
            provider.check_access(Some("bob"), "/private", AccessLevel::Write),
            AuthzResult::Denied
        ));
    }

    #[test]
    fn test_path_specific_rules() {
        let content = r#"
            [/]
            * = r

            [/trunk/src]
            @developers = rw
        "#;

        let provider = SvnAuthzProvider::from_content(content).unwrap();

        // All can read root
        assert!(matches!(
            provider.check_access(Some("alice"), "/", AccessLevel::Read),
            AuthzResult::Granted
        ));

        // Anonymous can read /trunk/src (inherited from [/])
        assert!(matches!(
            provider.check_access(None, "/trunk/src", AccessLevel::Read),
            AuthzResult::Granted
        ));
    }
}
