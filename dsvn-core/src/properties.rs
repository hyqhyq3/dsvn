//! SVN Property Storage
//!
//! Manages versioned properties for files and directories

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Property value
pub type PropertyValue = String;

/// Property store for a single path
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertySet {
    /// Properties for this path
    #[serde(default)]
    pub properties: HashMap<String, PropertyValue>,
}

impl PropertySet {
    /// Create a new empty property set
    pub fn new() -> Self {
        Self {
            properties: HashMap::new(),
        }
    }

    /// Get a property value
    pub fn get(&self, name: &str) -> Option<&PropertyValue> {
        self.properties.get(name)
    }

    /// Set a property value
    pub fn set(&mut self, name: String, value: PropertyValue) {
        self.properties.insert(name, value);
    }

    /// Remove a property
    pub fn remove(&mut self, name: &str) -> Option<PropertyValue> {
        self.properties.remove(name)
    }

    /// List all property names
    pub fn list(&self) -> Vec<String> {
        self.properties.keys().cloned().collect()
    }

    /// Check if property exists
    pub fn contains(&self, name: &str) -> bool {
        self.properties.contains_key(name)
    }
}

impl Default for PropertySet {
    fn default() -> Self {
        Self::new()
    }
}

/// Global property store
pub struct PropertyStore {
    /// Path -> PropertySet mapping
    #[allow(dead_code)]
    properties: Arc<RwLock<HashMap<String, PropertySet>>>,
}

impl PropertyStore {
    /// Create a new property store
    pub fn new() -> Self {
        Self {
            properties: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get properties for a path
    pub async fn get(&self, path: &str) -> PropertySet {
        let props = self.properties.read().await;
        props.get(path).cloned().unwrap_or_default()
    }

    /// Set a property on a path
    pub async fn set(&self, path: String, name: String, value: PropertyValue) -> Result<()> {
        let mut props = self.properties.write().await;
        let prop_set = props.entry(path).or_insert_with(PropertySet::new);
        prop_set.set(name, value);
        Ok(())
    }

    /// Remove a property from a path
    pub async fn remove(&self, path: &str, name: &str) -> Result<Option<PropertyValue>> {
        let mut props = self.properties.write().await;
        if let Some(prop_set) = props.get_mut(path) {
            Ok(prop_set.remove(name))
        } else {
            Ok(None)
        }
    }

    /// List all properties for a path
    pub async fn list(&self, path: &str) -> Vec<String> {
        let props = self.properties.read().await;
        props.get(path)
            .map(|p| p.list())
            .unwrap_or_default()
    }

    /// Check if a path has a specific property
    pub async fn contains(&self, path: &str, name: &str) -> bool {
        let props = self.properties.read().await;
        props.get(path)
            .map(|p| p.contains(name))
            .unwrap_or(false)
    }
}

/// SVN standard properties
pub mod svn_props {
    /// Executable flag
    pub const EXECUTABLE: &str = "svn:executable";

    /// MIME type
    pub const MIME_TYPE: &str = "svn:mime-type";

    /// Ignore patterns
    pub const IGNORE: &str = "svn:ignore";

    /// End-of-line style
    pub const EOL_STYLE: &str = "svn:eol-style";

    /// Keywords
    pub const KEYWORDS: &str = "svn:keywords";

    /// Needs lock
    pub const NEEDS_LOCK: &str = "svn:needs-lock";

    /// Special property (directory)
    pub const SPECIAL: &str = "svn:special";

    /// Externals
    pub const EXTERNALS: &str = "svn:externals";

    /// Merge info
    pub const MERGE_INFO: &str = "svn:mergeinfo";

    /// Value for svn:executable
    pub const EXECUTABLE_VALUE: &str = "*";

    /// Check if a property name is an SVN standard property
    pub fn is_svn_property(name: &str) -> bool {
        name.starts_with("svn:")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_property_set_basic_operations() {
        let mut prop_set = PropertySet::new();

        // Initially empty
        assert!(prop_set.get("test").is_none());
        assert!(!prop_set.contains("test"));

        // Set property
        prop_set.set("test".to_string(), "value".to_string());
        assert_eq!(prop_set.get("test"), Some(&"value".to_string()));
        assert!(prop_set.contains("test"));

        // Remove property
        let removed = prop_set.remove("test");
        assert_eq!(removed, Some("value".to_string()));
        assert!(!prop_set.contains("test"));
    }

    #[tokio::test]
    async fn test_property_set_list() {
        let mut prop_set = PropertySet::new();

        prop_set.set("prop1".to_string(), "value1".to_string());
        prop_set.set("prop2".to_string(), "value2".to_string());
        prop_set.set("prop3".to_string(), "value3".to_string());

        let names = prop_set.list();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"prop1".to_string()));
        assert!(names.contains(&"prop2".to_string()));
        assert!(names.contains(&"prop3".to_string()));
    }

    #[tokio::test]
    async fn test_property_store_get_nonexistent_path() {
        let store = PropertyStore::new();

        let props = store.get("/nonexistent/path").await;
        assert!(props.get("any").is_none());
        assert_eq!(props.list().len(), 0);
    }

    #[tokio::test]
    async fn test_property_store_set_and_get() {
        let store = PropertyStore::new();

        store.set(
            "/test/file.txt".to_string(),
            "myprop".to_string(),
            "myvalue".to_string(),
        ).await.unwrap();

        let props = store.get("/test/file.txt").await;
        assert_eq!(props.get("myprop"), Some(&"myvalue".to_string()));
    }

    #[tokio::test]
    async fn test_property_store_multiple_paths() {
        let store = PropertyStore::new();

        // Set different properties on different paths
        store.set("/file1.txt".to_string(), "prop1".to_string(), "value1".to_string()).await.unwrap();
        store.set("/file2.txt".to_string(), "prop2".to_string(), "value2".to_string()).await.unwrap();

        let props1 = store.get("/file1.txt").await;
        let props2 = store.get("/file2.txt").await;

        assert_eq!(props1.get("prop1"), Some(&"value1".to_string()));
        assert!(props1.get("prop2").is_none());

        assert_eq!(props2.get("prop2"), Some(&"value2".to_string()));
        assert!(props2.get("prop1").is_none());
    }

    #[tokio::test]
    async fn test_property_store_remove() {
        let store = PropertyStore::new();

        store.set(
            "/test.txt".to_string(),
            "toremove".to_string(),
            "value".to_string(),
        ).await.unwrap();

        // Verify exists
        assert!(store.contains("/test.txt", "toremove").await);

        // Remove
        let removed = store.remove("/test.txt", "toremove").await.unwrap();
        assert_eq!(removed, Some("value".to_string()));

        // Verify gone
        assert!(!store.contains("/test.txt", "toremove").await);
    }

    #[tokio::test]
    async fn test_svn_standard_properties() {
        use svn_props::*;

        // Test SVN property detection
        assert!(is_svn_property("svn:executable"));
        assert!(is_svn_property("svn:mime-type"));
        assert!(is_svn_property("svn:ignore"));
        assert!(!is_svn_property("custom:myprop"));
        assert!(!is_svn_property("user:comment"));

        // Test constants
        assert_eq!(EXECUTABLE, "svn:executable");
        assert_eq!(EXECUTABLE_VALUE, "*");
    }

    #[tokio::test]
    async fn test_property_overwrite() {
        let store = PropertyStore::new();

        // Set initial value
        store.set("/test.txt".to_string(), "prop".to_string(), "value1".to_string()).await.unwrap();
        let props = store.get("/test.txt").await;
        assert_eq!(props.get("prop"), Some(&"value1".to_string()));

        // Overwrite
        store.set("/test.txt".to_string(), "prop".to_string(), "value2".to_string()).await.unwrap();
        let props = store.get("/test.txt").await;
        assert_eq!(props.get("prop"), Some(&"value2".to_string()));
    }

    #[tokio::test]
    async fn test_empty_property_value() {
        let mut prop_set = PropertySet::new();

        // Set empty value (allowed in SVN)
        prop_set.set("empty".to_string(), "".to_string());

        assert_eq!(prop_set.get("empty"), Some(&"".to_string()));
        assert!(prop_set.contains("empty"));
    }

    #[tokio::test]
    async fn test_property_list_separates_paths() {
        let store = PropertyStore::new();

        store.set("/dir/file1.txt".to_string(), "prop".to_string(), "val1".to_string()).await.unwrap();
        store.set("/dir/file2.txt".to_string(), "prop".to_string(), "val2".to_string()).await.unwrap();

        // Each path should have independent properties
        let list1 = store.list("/dir/file1.txt").await;
        let list2 = store.list("/dir/file2.txt").await;

        assert_eq!(list1.len(), 1);
        assert_eq!(list2.len(), 1);
        assert_eq!(list1[0], "prop");
        assert_eq!(list2[0], "prop");
    }

    #[tokio::test]
    async fn test_property_store_concurrent_access() {
        let store = Arc::new(PropertyStore::new());
        let mut handles = vec![];

        // Spawn multiple concurrent writes
        for i in 0..10 {
            let store_clone = store.clone();
            let handle = tokio::spawn(async move {
                store_clone.set(
                    format!("/test{}.txt", i),
                    format!("prop{}", i),
                    format!("value{}", i),
                ).await
            });
            handles.push(handle);
        }

        // Wait for all writes
        for handle in handles {
            handle.await.unwrap().unwrap();
        }

        // Verify all writes succeeded
        for i in 0..10 {
            let props = store.get(&format!("/test{}.txt", i)).await;
            assert_eq!(props.get(&format!("prop{}", i)), Some(&format!("value{}", i)));
        }
    }
}
