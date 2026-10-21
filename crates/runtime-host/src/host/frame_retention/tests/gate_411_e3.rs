// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #411 A29 (slice E3, test row T2): the use lock `artifact-use-locks\<name>.lock` that
//! a frame's publication leaves in the state root is deleted when the frame cleaner's removal or
//! move of that frame completes. A lock that cannot be deleted is reported with a registered
//! code, and the sweep is unaffected: its frames still count as removed. Every expected value
//! comes from the E3 brief (A29).

use super::*;
use actingcommand_contract::outcome::Category;

/// The use lock of frame `index`, beside the state root's objects.
fn use_lock(root: &Root, index: usize) -> PathBuf {
    root.path.join("artifact-use-locks").join(format!(
        "{}.lock",
        crate::failure_identity::identifier_text(&root.frames[index].reference.artifact_id)
    ))
}

/// The empty use lock a publication of frame `index` leaves behind.
fn published_lock(root: &Root, index: usize) -> PathBuf {
    let lock = use_lock(root, index);
    std::fs::create_dir_all(lock.parent().expect("lock folder")).expect("lock folder");
    std::fs::write(&lock, b"").expect("use lock");
    lock
}

// T2: a completed removal and a completed move each take their frame's use lock with them.
#[test]
fn e3_t2_a_removed_or_moved_frame_takes_its_use_lock_with_it() {
    let mut root = Root::new();
    let removed = root.frame(FrameRetentionClass::Default, Some(T0), None, true);
    let moved = root.frame(FrameRetentionClass::Error, None, Some(ERROR_LEAF), true);
    let removed_lock = published_lock(&root, removed);
    let moved_lock = published_lock(&root, moved);
    let mut retention = cleaner();
    let (report, warnings) = sweep(&mut retention, &root.path, &root.view());
    let report = report.expect("first sweep");
    assert_eq!((report.deleted, report.moved, report.failed), (1, 1, 0));
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(!root.object(removed).exists());
    assert!(root.kept(moved).is_file());
    assert!(
        !removed_lock.exists(),
        "the removed frame's use lock is gone"
    );
    assert!(!moved_lock.exists(), "the moved frame's use lock is gone");
}

// T2: a use lock another program holds open without delete sharing cannot be deleted: the
// cleaner reports it with its code, and the sweep goes on as if the lock had gone.
#[cfg(windows)]
#[test]
fn e3_t2_a_use_lock_that_cannot_be_deleted_is_reported_and_the_sweep_goes_on() {
    use std::os::windows::fs::OpenOptionsExt;
    /// `FILE_SHARE_READ | FILE_SHARE_WRITE`, without `FILE_SHARE_DELETE`.
    const SHARE_READ_WRITE: u32 = 0x0000_0001 | 0x0000_0002;
    let mut root = Root::new();
    let held = root.frame(FrameRetentionClass::Default, Some(T0), None, true);
    let next = root.frame(FrameRetentionClass::Duplicate, Some(T0), None, true);
    let held_lock = published_lock(&root, held);
    let next_lock = published_lock(&root, next);
    // A backup tool, say, has the first lock open.
    let holder = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(SHARE_READ_WRITE)
        .open(&held_lock)
        .expect("holder");
    let mut retention = cleaner();
    let (report, warnings) = sweep(&mut retention, &root.path, &root.view());
    // The sweep is unaffected: both frames are removed and nothing failed.
    let report = report.expect("first sweep");
    assert_eq!((report.frames, report.deleted, report.failed), (2, 2, 0));
    assert!(!root.object(held).exists());
    assert!(!root.object(next).exists());
    assert!(!next_lock.exists(), "the other frame's use lock is gone");
    // The lock that could not be deleted is reported with its registered code.
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let warning = &warnings[0];
    assert_eq!(warning.code, HostCode::FrameRetentionLockRemoveFailed);
    assert_eq!(
        warning.artifact_id,
        Some(root.frames[held].reference.artifact_id)
    );
    assert!(warning.os_error.is_some(), "{warning:?}");
    let outcome = warning.outcome();
    assert_eq!(
        outcome.code().as_str(),
        "frame_retention_lock_remove_failed"
    );
    assert_eq!(outcome.category(), Category::Warning);
    assert!(
        outcome
            .value("artifact_id")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|id| id.starts_with("artifact_"))
    );
    drop(holder);
}
