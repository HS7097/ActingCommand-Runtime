// SPDX-License-Identifier: AGPL-3.0-only

// one-off (to be reverted): #288 A1 evidence on CI. A content-directory digest mismatch reaches
// contained-task admission callers as its own code, with the expected/actual/count detail.

use actingcommand_contract::{ContentDirectory, ContentDirectoryVersion, PackageRef};
use actingcommand_execution_kernel::PreparedContainedTask;
use std::io::Write;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[test]
fn one_off_a1_contained_task_digest_mismatch_code() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("one-off-a1-{}-{unique}", std::process::id()));
    let pack = root.join("work");
    std::fs::create_dir_all(pack.join("resources")).expect("create directory");
    std::fs::write(pack.join("control.json"), b"{}").expect("write file");
    let expected = PackageRef::ContentDirectory(ContentDirectory {
        schema_version: ContentDirectoryVersion::V1,
        sha256: "0".repeat(64),
    });
    let result = PreparedContainedTask::load_path(
        "one-off-a1",
        &pack,
        &expected,
        None,
        Instant::now() + Duration::from_secs(60),
    );
    std::fs::remove_dir_all(&root).expect("remove one-off directory");
    let error = match result {
        Ok(_) => panic!("digest mismatch was admitted"),
        Err(error) => error,
    };
    let _ = writeln!(
        std::io::stderr().lock(),
        "ONE-OFF-A1 kernel code={} detail={:?}",
        error.code(),
        error.detail()
    );
    assert_eq!(error.code(), "content_directory_digest_mismatch");
    assert!(
        error
            .detail()
            .is_some_and(|detail| detail.contains("over 1 files"))
    );
}
