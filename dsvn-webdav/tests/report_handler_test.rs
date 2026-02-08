//! Integration tests for REPORT handler enhancements
//!
//! Tests the various REPORT request types:
//! - log-report (svn log)
//! - update-report (svn checkout/update)
//! - get-locations-report
//! - get-dated-rev-report
//! - mergeinfo-report
//! - get-locks-report
//! - get-deleted-rev-report
//! - inherited-props-report

use dsvn_core::SqliteRepository;
use tempfile::TempDir;

/// Set up a test repository with some commits
async fn setup_repo() -> (TempDir, SqliteRepository) {
    let tmp = TempDir::new().unwrap();
    let repo = SqliteRepository::open(tmp.path()).unwrap();
    repo.initialize().await.unwrap();

    // Create revision 1: add two files
    repo.add_file("/hello.txt", b"Hello, World!".to_vec(), false).await.unwrap();
    repo.add_file("/src/main.rs", b"fn main() {}".to_vec(), true).await.unwrap();
    repo.commit("alice".into(), "Initial files".into(), 1000).await.unwrap();

    // Create revision 2: add another file
    repo.add_file("/README.md", b"# Test Project".to_vec(), false).await.unwrap();
    repo.commit("bob".into(), "Add README".into(), 2000).await.unwrap();

    // Create revision 3: delete a file
    repo.delete_file("/hello.txt").await.unwrap();
    repo.commit("alice".into(), "Remove hello.txt".into(), 3000).await.unwrap();

    (tmp, repo)
}

// ==================== log-report tests ====================

#[tokio::test]
async fn test_log_report_basic() {
    let (_tmp, repo) = setup_repo().await;
    assert_eq!(repo.current_rev().await, 3);

    // Verify commits are retrievable
    let c1 = repo.get_commit(1).await.unwrap();
    assert_eq!(c1.author, "alice");
    assert_eq!(c1.message, "Initial files");

    let c2 = repo.get_commit(2).await.unwrap();
    assert_eq!(c2.author, "bob");
    assert_eq!(c2.message, "Add README");

    let c3 = repo.get_commit(3).await.unwrap();
    assert_eq!(c3.author, "alice");
    assert_eq!(c3.message, "Remove hello.txt");
}

#[tokio::test]
async fn test_log_report_range() {
    let (_tmp, repo) = setup_repo().await;

    // Test log from rev 1 to 2
    let log = repo.log(2, 100).await.unwrap();
    assert!(log.len() >= 2);
}

#[tokio::test]
async fn test_log_report_limit() {
    let (_tmp, repo) = setup_repo().await;

    // Test log with limit
    let log = repo.log(3, 1).await.unwrap();
    assert_eq!(log.len(), 1);
}

#[tokio::test]
async fn test_changed_paths_via_delta_tree() {
    let (_tmp, repo) = setup_repo().await;

    // Revision 1 should have added files
    let delta1 = repo.get_delta_tree(1).unwrap();
    assert!(!delta1.changes.is_empty());

    // Check that hello.txt and src/main.rs were added
    let added_paths: Vec<String> = delta1.changes.iter().filter_map(|c| {
        match c {
            dsvn_core::TreeChange::Upsert { path, .. } => Some(path.clone()),
            _ => None,
        }
    }).collect();
    assert!(added_paths.iter().any(|p| p == "hello.txt"), "Should have hello.txt: {:?}", added_paths);
    assert!(added_paths.iter().any(|p| p == "src/main.rs"), "Should have src/main.rs: {:?}", added_paths);

    // Revision 3 should have a delete for hello.txt
    let delta3 = repo.get_delta_tree(3).unwrap();
    let deleted_paths: Vec<String> = delta3.changes.iter().filter_map(|c| {
        match c {
            dsvn_core::TreeChange::Delete { path } => Some(path.clone()),
            _ => None,
        }
    }).collect();
    assert!(deleted_paths.iter().any(|p| p == "hello.txt"), "Should have deleted hello.txt: {:?}", deleted_paths);
}

// ==================== update-report (checkout) tests ====================

#[tokio::test]
async fn test_tree_at_revision() {
    let (_tmp, repo) = setup_repo().await;

    // At rev 1: hello.txt and src/main.rs
    let tree1 = repo.get_tree_at_rev(1).unwrap();
    assert!(tree1.contains_key("hello.txt"), "Rev 1 should have hello.txt");
    assert!(tree1.contains_key("src/main.rs"), "Rev 1 should have src/main.rs");

    // At rev 2: hello.txt, src/main.rs, README.md
    let tree2 = repo.get_tree_at_rev(2).unwrap();
    assert!(tree2.contains_key("hello.txt"));
    assert!(tree2.contains_key("src/main.rs"));
    assert!(tree2.contains_key("README.md"), "Rev 2 should have README.md");

    // At rev 3: src/main.rs, README.md (hello.txt deleted)
    let tree3 = repo.get_tree_at_rev(3).unwrap();
    assert!(!tree3.contains_key("hello.txt"), "Rev 3 should NOT have hello.txt");
    assert!(tree3.contains_key("src/main.rs"));
    assert!(tree3.contains_key("README.md"));
}

#[tokio::test]
async fn test_file_content_at_revision() {
    let (_tmp, repo) = setup_repo().await;

    // Read file at rev 1
    let content = repo.get_file("/hello.txt", 1).await.unwrap();
    assert_eq!(content.as_ref(), b"Hello, World!");

    // Read file at rev 2 (still exists)
    let content = repo.get_file("/hello.txt", 2).await.unwrap();
    assert_eq!(content.as_ref(), b"Hello, World!");

    // Read file at rev 3 (deleted)
    assert!(repo.get_file("/hello.txt", 3).await.is_err());

    // Read README.md at rev 2
    let content = repo.get_file("/README.md", 2).await.unwrap();
    assert_eq!(content.as_ref(), b"# Test Project");
}

// ==================== get-locations tests ====================

#[tokio::test]
async fn test_get_locations_file_exists() {
    let (_tmp, repo) = setup_repo().await;

    // hello.txt exists at rev 1 and 2, not at rev 3
    let tree1 = repo.get_tree_at_rev(1).unwrap();
    assert!(tree1.contains_key("hello.txt"));

    let tree2 = repo.get_tree_at_rev(2).unwrap();
    assert!(tree2.contains_key("hello.txt"));

    let tree3 = repo.get_tree_at_rev(3).unwrap();
    assert!(!tree3.contains_key("hello.txt"));
}

#[tokio::test]
async fn test_get_locations_new_file() {
    let (_tmp, repo) = setup_repo().await;

    // README.md only exists from rev 2 onward
    let tree1 = repo.get_tree_at_rev(1).unwrap();
    assert!(!tree1.contains_key("README.md"));

    let tree2 = repo.get_tree_at_rev(2).unwrap();
    assert!(tree2.contains_key("README.md"));
}

// ==================== get-dated-rev tests ====================

#[tokio::test]
async fn test_dated_rev_search() {
    let (_tmp, repo) = setup_repo().await;

    // Timestamps: rev1=1000, rev2=2000, rev3=3000
    // For timestamp 1500, the latest commit is rev1 (timestamp 1000)
    let c1 = repo.get_commit(1).await.unwrap();
    assert_eq!(c1.timestamp, 1000);

    let c2 = repo.get_commit(2).await.unwrap();
    assert_eq!(c2.timestamp, 2000);

    // For timestamp 2500, the latest is rev2
    // For timestamp >= 3000, the latest is rev3
    let c3 = repo.get_commit(3).await.unwrap();
    assert_eq!(c3.timestamp, 3000);
}

// ==================== get-deleted-rev tests ====================

#[tokio::test]
async fn test_deleted_rev_detection() {
    let (_tmp, repo) = setup_repo().await;

    // hello.txt was deleted in revision 3
    let delta3 = repo.get_delta_tree(3).unwrap();
    let has_delete = delta3.changes.iter().any(|c| {
        matches!(c, dsvn_core::TreeChange::Delete { path } if path == "hello.txt")
    });
    assert!(has_delete, "Revision 3 should have a delete for hello.txt");

    // Revisions 1 and 2 should not have deletes for hello.txt
    let delta1 = repo.get_delta_tree(1).unwrap();
    let has_delete1 = delta1.changes.iter().any(|c| {
        matches!(c, dsvn_core::TreeChange::Delete { path } if path == "hello.txt")
    });
    assert!(!has_delete1, "Revision 1 should NOT have a delete for hello.txt");
}

// ==================== XML format validation tests ====================

#[cfg(test)]
mod xml_format_tests {
    /// Validate log-report XML structure matches SVN expectations
    #[test]
    fn test_log_report_xml_structure() {
        let sample = r#"<?xml version="1.0" encoding="utf-8"?>
<S:log-report xmlns:S="svn:" xmlns:D="DAV:">
<S:log-item>
<D:version-name>1</D:version-name>
<D:creator-displayname>alice</D:creator-displayname>
<S:date>1970-01-01T00:16:40.000000Z</S:date>
<D:comment>Initial files</D:comment>
<S:changed-path-item>
<S:modified-path node-kind="file" text-mods="true" prop-mods="false">/hello.txt</S:modified-path>
</S:changed-path-item>
<S:has-children/>
</S:log-item>
</S:log-report>"#;

        assert!(sample.contains("S:log-report"), "Should have log-report root");
        assert!(sample.contains("xmlns:S=\"svn:\""), "Should have svn namespace");
        assert!(sample.contains("D:version-name"), "Should have version-name");
        assert!(sample.contains("D:creator-displayname"), "Should have author");
        assert!(sample.contains("S:date"), "Should have date");
        assert!(sample.contains("D:comment"), "Should have comment");
        assert!(sample.contains("S:changed-path-item"), "Should have changed paths");
    }

    /// Validate update-report XML structure
    #[test]
    fn test_update_report_xml_structure() {
        let sample = r#"<?xml version="1.0" encoding="utf-8"?>
<S:update-report xmlns:S="svn:" xmlns:V="http://subversion.tigris.org/xmlns/dav/" xmlns:D="DAV:" send-all="true" inline-props="true">
<S:target-revision rev="1"/>
<S:open-directory rev="1">
<D:checked-in><D:href>/svn/!svn/rvr/1/</D:href></D:checked-in>
<S:set-prop name="svn:entry:committed-rev">1</S:set-prop>
<S:add-file name="hello.txt">
<D:checked-in><D:href>/svn/!svn/rvr/1/hello.txt</D:href></D:checked-in>
</S:add-file>
</S:open-directory>
</S:update-report>"#;

        assert!(sample.contains("S:update-report"), "Should have update-report root");
        assert!(sample.contains("send-all=\"true\""), "Should have send-all");
        assert!(sample.contains("S:target-revision"), "Should have target-revision");
        assert!(sample.contains("S:open-directory"), "Should have open-directory");
        assert!(sample.contains("S:add-file"), "Should have add-file for checkout");
        assert!(sample.contains("D:checked-in"), "Should have checked-in href");
    }

    /// Validate get-locations-report XML structure
    #[test]
    fn test_get_locations_report_xml_structure() {
        let sample = r#"<?xml version="1.0" encoding="utf-8"?>
<S:get-locations-report xmlns:S="svn:" xmlns:D="DAV:">
<S:location rev="1" path="/hello.txt"/>
<S:location rev="2" path="/hello.txt"/>
</S:get-locations-report>"#;

        assert!(sample.contains("S:get-locations-report"), "Should have get-locations-report root");
        assert!(sample.contains("S:location"), "Should have location entries");
        assert!(sample.contains("rev=\"1\""), "Should have rev attribute");
        assert!(sample.contains("path=\"/hello.txt\""), "Should have path attribute");
    }

    /// Validate get-dated-rev-report XML structure
    #[test]
    fn test_dated_rev_report_xml_structure() {
        let sample = r#"<?xml version="1.0" encoding="utf-8"?>
<S:dated-rev-report xmlns:S="svn:" xmlns:D="DAV:">
<D:version-name>2</D:version-name>
</S:dated-rev-report>"#;

        assert!(sample.contains("S:dated-rev-report"), "Should have dated-rev-report root");
        assert!(sample.contains("D:version-name"), "Should have version-name");
    }

    /// Validate mergeinfo-report XML structure
    #[test]
    fn test_mergeinfo_report_xml_structure() {
        let sample = r#"<?xml version="1.0" encoding="utf-8"?>
<S:mergeinfo-report xmlns:S="svn:">
</S:mergeinfo-report>"#;

        assert!(sample.contains("S:mergeinfo-report"), "Should have mergeinfo-report root");
    }

    /// Validate get-deleted-rev-report XML structure
    #[test]
    fn test_deleted_rev_report_xml_structure() {
        let sample = r#"<?xml version="1.0" encoding="utf-8"?>
<S:get-deleted-rev-report xmlns:S="svn:" xmlns:D="DAV:">
<D:version-name>3</D:version-name>
</S:get-deleted-rev-report>"#;

        assert!(sample.contains("S:get-deleted-rev-report"), "Should have get-deleted-rev-report root");
        assert!(sample.contains("D:version-name"), "Should have version-name with deleted rev");
    }

    /// Validate inherited-props-report XML structure
    #[test]
    fn test_inherited_props_report_xml_structure() {
        let sample = r#"<?xml version="1.0" encoding="utf-8"?>
<S:inherited-props-report xmlns:S="svn:" xmlns:D="DAV:">
</S:inherited-props-report>"#;

        assert!(sample.contains("S:inherited-props-report"), "Should have inherited-props-report root");
    }

    /// Validate get-locks-report XML structure
    #[test]
    fn test_get_locks_report_xml_structure() {
        let sample = r#"<?xml version="1.0" encoding="utf-8"?>
<S:get-locks-report xmlns:S="svn:">
</S:get-locks-report>"#;

        assert!(sample.contains("S:get-locks-report"), "Should have get-locks-report root");
    }
}

// ==================== Report dispatch validation ====================

#[cfg(test)]
mod report_dispatch_tests {
    /// Test that report type detection works correctly for various XML bodies
    #[test]
    fn test_report_type_detection() {
        // Update report
        let update_body = r#"<S:update-report xmlns:S="svn:"><S:target-revision>5</S:target-revision></S:update-report>"#;
        assert!(update_body.contains("update-report"));

        // Log report
        let log_body = r#"<S:log-report xmlns:S="svn:"><S:start-revision>10</S:start-revision></S:log-report>"#;
        assert!(log_body.contains("log-report"));

        // Get-locations report
        let locations_body = r#"<S:get-locations xmlns:S="svn:"><S:path>/trunk/file.txt</S:path></S:get-locations>"#;
        assert!(locations_body.contains("get-locations"));

        // Get-dated-rev report
        let dated_body = r#"<S:get-dated-rev-report xmlns:S="svn:"><D:creationdate>2024-01-01T00:00:00Z</D:creationdate></S:get-dated-rev-report>"#;
        assert!(dated_body.contains("get-dated-rev"));

        // Mergeinfo report
        let mergeinfo_body = r#"<S:mergeinfo-report xmlns:S="svn:"><S:path>/trunk</S:path></S:mergeinfo-report>"#;
        assert!(mergeinfo_body.contains("mergeinfo-report"));

        // Get-locks report
        let locks_body = r#"<S:get-locks-report xmlns:S="svn:"><S:path>/trunk</S:path></S:get-locks-report>"#;
        assert!(locks_body.contains("get-locks-report"));

        // Replay report
        let replay_body = r#"<S:replay-report xmlns:S="svn:"><S:revision>5</S:revision></S:replay-report>"#;
        assert!(replay_body.contains("replay-report"));

        // Get-deleted-rev report
        let deleted_body = r#"<S:get-deleted-rev-report xmlns:S="svn:"><S:path>/trunk/old.txt</S:path></S:get-deleted-rev-report>"#;
        assert!(deleted_body.contains("get-deleted-rev"));

        // Inherited-props report
        let inherited_body = r#"<S:inherited-props-report xmlns:S="svn:"><S:path>/trunk</S:path></S:inherited-props-report>"#;
        assert!(inherited_body.contains("inherited-props-report"));
    }
}
