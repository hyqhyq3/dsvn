//! Performance benchmark for dump/load

use dsvn_core::SqliteRepository;
use dsvn_webdav::dump_handlers::{self, DumpParams};
use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
async fn bench_dump_performance() {
    let tmp = TempDir::new().unwrap();
    let repo = SqliteRepository::open(tmp.path()).unwrap();
    repo.initialize().await.unwrap();

    // Create a repo with 100 commits, each adding a file
    let start = std::time::Instant::now();
    for i in 1..=100 {
        repo.add_file(
            &format!("/file_{:04}.txt", i),
            format!("Content of file {} with some padding data to simulate real files. Lorem ipsum dolor sit amet.", i).into_bytes(),
            false,
        ).await.unwrap();
        repo.commit(
            format!("user{}", i % 5),
            format!("Commit {}: add file_{:04}.txt", i, i),
            1700000000 + i as i64 * 100,
        ).await.unwrap();
    }
    let setup_time = start.elapsed();
    eprintln!("Setup: created 100 commits in {:.1}ms", setup_time.as_millis());

    // Full dump
    let start = std::time::Instant::now();
    let params = DumpParams { start_rev: 0, end_rev: 100, incremental: false, format_version: 3 };
    let data = dump_handlers::generate_dump(&repo, &params).await.unwrap();
    let dump_time = start.elapsed();
    eprintln!(
        "Full dump: {} bytes in {:.1}ms ({:.0} revs/sec, {:.1} MB/s)",
        data.len(),
        dump_time.as_millis(),
        100.0 / dump_time.as_secs_f64(),
        data.len() as f64 / 1024.0 / 1024.0 / dump_time.as_secs_f64()
    );

    // Incremental dump (last 10 revisions)
    let start = std::time::Instant::now();
    let params = DumpParams { start_rev: 91, end_rev: 100, incremental: true, format_version: 3 };
    let inc_data = dump_handlers::generate_dump(&repo, &params).await.unwrap();
    let inc_time = start.elapsed();
    eprintln!(
        "Incremental dump (10 revs): {} bytes in {:.1}ms",
        inc_data.len(),
        inc_time.as_millis()
    );

    // Load into new repo
    let dst_tmp = TempDir::new().unwrap();
    let dst_repo = Arc::new(SqliteRepository::open(dst_tmp.path()).unwrap());
    dst_repo.initialize().await.unwrap();

    let start = std::time::Instant::now();
    let response = dump_handlers::handle_load(dst_repo.clone(), data.clone()).await;
    let load_time = start.elapsed();
    assert_eq!(response.status(), 200);
    eprintln!(
        "Full load: {} bytes in {:.1}ms ({:.0} revs/sec)",
        data.len(),
        load_time.as_millis(),
        100.0 / load_time.as_secs_f64()
    );

    // Verify
    let dst_head = dst_repo.current_rev().await;
    assert_eq!(dst_head, 100);
    let content = dst_repo.get_file("/file_0050.txt", dst_head).await.unwrap();
    assert!(content.len() > 0);

    eprintln!("Performance test passed: dump/load cycle verified for 100 revisions");
}
