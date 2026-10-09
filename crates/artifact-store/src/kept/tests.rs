// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #375 R5d: the cleaner's file operations at the object key and the readers' kept
//! lookup, on real files in a temporary state root.

use super::*;
use crate::{
    ArtifactStore, FrameFileOutcome, move_frame_file, open_projected_stream, remove_frame_file,
    try_artifact_delete_guard,
};
use actingcommand_contract::{
    ArtifactKind, ArtifactProducer, ArtifactRedactionState, IdentifierIssuer,
    ProjectedArtifactReference, RetentionClass,
};

const DATE: &str = "2026-10-12";
const LEAF: &str = "node_a-151234-contained_task_page_unknown";

/// A capture frame published at its object key, as the store writes one.
fn frame(root: &Path, bytes: &[u8]) -> ProjectedArtifactReference {
    let reference = unpublished(bytes);
    let path = root.join(reference.object_key().expect("object key"));
    fs::create_dir_all(path.parent().expect("shard")).expect("shard folder");
    fs::write(path, bytes).expect("frame file");
    reference
}

/// A reference whose file was never written (a frame deleted by hand).
fn unpublished(bytes: &[u8]) -> ProjectedArtifactReference {
    let ids = IdentifierIssuer::new().expect("issuer");
    let artifact_id = *ids.mint_artifact_id().expect("artifact").transport();
    let frame_id = *ids.mint_frame_id().expect("frame").transport();
    let sha256 = crate::store::canonical_sha256(bytes);
    let id = match serde_json::to_value(artifact_id) {
        Ok(serde_json::Value::String(id)) => id,
        other => panic!("artifact id text: {other:?}"),
    };
    let reference = ProjectedArtifactReference {
        artifact_id,
        kind: ArtifactKind::CaptureFrame,
        run_id: None,
        frame_id: Some(frame_id),
        correlation_id: None,
        object_key: Some(format!("artifacts/{}/{id}.png", &sha256[7..9])),
        media_type: ArtifactKind::CaptureFrame.media_type(),
        byte_count: bytes.len() as u64,
        sha256,
        created_at_unix_ms: 1_791_753_302_117,
        producer: ArtifactProducer::CaptureStore,
        retention_class: RetentionClass::Adaptive,
        redaction_state: ArtifactRedactionState::NotRequired,
    };
    reference.validate().expect("valid reference");
    reference
}

fn object_file(reference: &ProjectedArtifactReference) -> String {
    Path::new(reference.object_key().expect("object key"))
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .expect("object file")
        .to_owned()
}

fn kept_name(reference: &ProjectedArtifactReference) -> String {
    format!("061502-117_{}", object_file(reference))
}

fn canonical_root() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("root");
    let root = temp.path().canonicalize().expect("canonical root");
    (temp, root)
}

fn read(root: &Path, reference: &ProjectedArtifactReference) -> ArtifactStoreResult<PathBuf> {
    let reader = open_projected_stream(root, reference)?;
    let relative = reader.resolved_relative_path().to_path_buf();
    reader.finish()?;
    Ok(relative)
}

fn move_into_leaf(root: &Path, reference: &ProjectedArtifactReference) -> FrameFileOutcome {
    move_frame_file(root, reference, DATE, LEAF, &kept_name(reference)).expect("move")
}

#[test]
fn a_moved_frame_reads_available_and_a_deleted_leaf_reads_missing() {
    let (_temp, root) = canonical_root();
    let moved = frame(&root, b"error frame");
    let removed = frame(&root, b"duplicate frame");
    assert_eq!(move_into_leaf(&root, &moved), FrameFileOutcome::Moved);
    assert_eq!(
        remove_frame_file(&root, &removed).expect("remove"),
        FrameFileOutcome::Removed
    );
    let kept = Path::new(KEPT_DIRECTORY)
        .join(DATE)
        .join(LEAF)
        .join(kept_name(&moved));
    assert!(root.join(&kept).is_file());
    assert!(!root.join(moved.object_key().expect("key")).exists());
    // A real `artifact_<hex>` name resolves to its id, and the read verifies the bytes.
    assert_eq!(read(&root, &moved).expect("moved frame"), kept);
    assert!(
        read(&root, &removed)
            .expect_err("removed frame")
            .is_material_missing()
    );
    // A leaf deleted by hand: the cached entry is dropped and the frame is missing.
    fs::remove_dir_all(root.join(KEPT_DIRECTORY).join(DATE).join(LEAF)).expect("hand deletion");
    assert!(
        read(&root, &moved)
            .expect_err("deleted leaf")
            .is_material_missing()
    );
}

#[test]
fn the_cleaner_acts_only_at_the_object_key() {
    let (_temp, root) = canonical_root();
    let moved = frame(&root, b"kept frame");
    assert_eq!(move_into_leaf(&root, &moved), FrameFileOutcome::Moved);
    // After a switch change the frame may be due for removal: only the object key counts, so
    // the kept file stays.
    assert_eq!(
        remove_frame_file(&root, &moved).expect("remove"),
        FrameFileOutcome::Absent
    );
    assert_eq!(move_into_leaf(&root, &moved), FrameFileOutcome::Absent);
    assert!(
        root.join(KEPT_DIRECTORY)
            .join(DATE)
            .join(LEAF)
            .join(kept_name(&moved))
            .is_file()
    );
    assert_eq!(
        remove_frame_file(&root, &unpublished(b"never written")).expect("absent"),
        FrameFileOutcome::Absent
    );
}

#[cfg(windows)]
#[test]
fn a_frame_held_with_read_sharing_only_is_busy_until_released() {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    let (_temp, root) = canonical_root();
    let held = frame(&root, b"frame in an image viewer");
    let path = root.join(held.object_key().expect("key"));
    let viewer = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&path)
        .expect("viewer");
    assert_eq!(
        remove_frame_file(&root, &held).expect("busy, not an error"),
        FrameFileOutcome::Busy
    );
    assert_eq!(move_into_leaf(&root, &held), FrameFileOutcome::Busy);
    // The startup recovery guard defers a held file too, without an error.
    assert!(
        try_artifact_delete_guard(&root, &held)
            .expect("busy, not an error")
            .is_none()
    );
    assert!(path.is_file());
    drop(viewer);
    assert_eq!(
        remove_frame_file(&root, &held).expect("remove"),
        FrameFileOutcome::Removed
    );
}

#[test]
fn a_leaf_deleted_before_the_rename_is_created_again() {
    let (_temp, root) = canonical_root();
    let moved = frame(&root, b"frame of a deleted leaf");
    let leaf = root.join(KEPT_DIRECTORY).join(DATE).join(LEAF);
    let name = kept_name(&moved);
    let outcome = crate::usage::move_frame_file_with(&root, &moved, [DATE, LEAF, &name], || {
        fs::remove_dir_all(&leaf).expect("leaf deleted by hand");
    })
    .expect("move");
    assert_eq!(outcome, FrameFileOutcome::Moved);
    assert!(leaf.join(name).is_file());
}

#[test]
fn a_failed_move_leaves_the_frame_and_no_map_entry() {
    let (_temp, root) = canonical_root();
    let stuck = frame(&root, b"frame whose folder is a file");
    fs::create_dir_all(root.join(KEPT_DIRECTORY)).expect("kept");
    fs::write(
        root.join(KEPT_DIRECTORY).join(DATE),
        b"a file, not a folder",
    )
    .expect("stray");
    let error = move_frame_file(&root, &stuck, DATE, LEAF, &kept_name(&stuck))
        .expect_err("the folder cannot be created");
    assert!(!error.is_fatal());
    assert!(error.io_error_kind().is_some());
    assert!(root.join(stuck.object_key().expect("key")).is_file());
    assert_eq!(
        read(&root, &stuck).expect("still at its key"),
        PathBuf::from(stuck.object_key().expect("key"))
    );
}

#[test]
fn another_process_move_is_found_once_the_counter_changes() {
    let (_temp, root) = canonical_root();
    let moved = frame(&root, b"frame moved by actingd");
    // This process walks once on its first miss.
    let missing = unpublished(b"deleted by hand");
    assert!(
        read(&root, &missing)
            .expect_err("miss")
            .is_material_missing()
    );
    assert_eq!(walks(&root), 1);
    // One second later the other process moves the frame and bumps the counter; nothing of
    // that reaches this process's map.
    let leaf = root.join(KEPT_DIRECTORY).join(DATE).join(LEAF);
    fs::create_dir_all(&leaf).expect("leaf");
    fs::rename(
        root.join(moved.object_key().expect("key")),
        leaf.join(kept_name(&moved)),
    )
    .expect("the other process's move");
    KeptMoves::new(1_791_753_302_117)
        .record_move(&root)
        .expect("counter");
    assert!(read(&root, &moved).is_ok(), "available after the walk");
    assert_eq!(walks(&root), 2);
}

#[test]
fn misses_of_deleted_frames_walk_once() {
    let (_temp, root) = canonical_root();
    for index in 0..1_000_u32 {
        let missing = unpublished(&index.to_le_bytes());
        assert!(
            read(&root, &missing)
                .expect_err("miss")
                .is_material_missing()
        );
    }
    assert_eq!(walks(&root), 1);
}

#[test]
fn a_walk_skips_stray_files_and_vanished_folders() {
    let (_temp, root) = canonical_root();
    let moved = frame(&root, b"frame beside stray files");
    let leaf = root.join(KEPT_DIRECTORY).join(DATE).join(LEAF);
    fs::create_dir_all(&leaf).expect("leaf");
    fs::write(root.join(KEPT_DIRECTORY).join(DATE).join("notes.txt"), b"x").expect("stray");
    fs::write(root.join(KEPT_DIRECTORY).join("desktop.ini"), b"x").expect("stray");
    fs::write(leaf.join("notes.txt"), b"x").expect("stray");
    fs::create_dir_all(leaf.join("deeper")).expect("deeper folder");
    fs::rename(
        root.join(moved.object_key().expect("key")),
        leaf.join(kept_name(&moved)),
    )
    .expect("moved by hand");
    let paths = walk(&root).expect("walk");
    assert_eq!(paths.len(), 1);
    let name = object_file(&moved);
    assert_eq!(
        paths.get(object_id(&name).expect("id")),
        Some(&leaf.join(kept_name(&moved)))
    );
    // A leaf that vanishes during the walk lists as empty.
    assert!(
        entries(&leaf.join("vanished"), false)
            .expect("vanished")
            .is_empty()
    );
    assert!(read(&root, &moved).is_ok());
}

#[test]
fn a_renamed_leaf_is_found_after_the_minute_walk() {
    let (_temp, root) = canonical_root();
    let moved = frame(&root, b"frame of a renamed leaf");
    assert_eq!(move_into_leaf(&root, &moved), FrameFileOutcome::Moved);
    let missing = unpublished(b"walks once");
    assert!(
        read(&root, &missing)
            .expect_err("miss")
            .is_material_missing()
    );
    let walked = walks(&root);
    let dated = root.join(KEPT_DIRECTORY).join(DATE);
    fs::rename(dated.join(LEAF), dated.join("renamed-by-hand")).expect("rename");
    // No counter changed and the last walk is recent: the frame reads missing for now.
    assert!(
        read(&root, &moved)
            .expect_err("stale for a minute")
            .is_material_missing()
    );
    assert_eq!(walks(&root), walked);
    age_walk(&root);
    assert!(read(&root, &moved).is_ok(), "found by the minute walk");
    assert_eq!(walks(&root), walked + 1);
}

#[test]
fn kept_and_object_file_names_resolve_to_their_artifact_id() {
    let hex = "18dc0123456789abcdef0123456789ab";
    let id = format!("artifact_{hex}");
    assert_eq!(object_id(&format!("{id}.png")), Some(id.as_str()));
    assert_eq!(kept_id(&format!("061502-117_{id}.png")), Some(id.as_str()));
    assert_eq!(kept_id("notes.txt"), None);
    assert_eq!(kept_id(&format!("061502-117_{id}.json")), None);
    assert_eq!(object_id(&format!("{id}.json")), None);
    assert_eq!(object_id("artifact_.png"), None);
}

#[cfg(windows)]
#[test]
fn a_read_while_the_cleaner_holds_the_frame_is_retried() {
    let (_temp, root) = canonical_root();
    let held = frame(&root, b"frame read during a move");
    let Ok(crate::usage::Held::Frame(hold)) = crate::usage::hold_frame(&root, &held) else {
        panic!("the cleaner holds the frame");
    };
    let reader = {
        let root = root.clone();
        let held = held.clone();
        std::thread::spawn(move || read(&root, &held))
    };
    std::thread::sleep(Duration::from_millis(30));
    drop(hold);
    assert!(
        reader.join().expect("reader").is_ok(),
        "retried, then available"
    );
}

#[test]
fn restore_writes_two_kept_frames_at_their_kept_paths() {
    let (_source_temp, source) = canonical_root();
    let (_target_temp, target_root) = canonical_root();
    let frames = [
        frame(&source, b"first kept frame"),
        frame(&source, b"second kept frame"),
    ];
    for reference in &frames {
        assert_eq!(move_into_leaf(&source, reference), FrameFileOutcome::Moved);
    }
    let target = ArtifactStore::open(&target_root).expect("target store");
    let deadline = Instant::now() + Duration::from_secs(30);
    for reference in &frames {
        target
            .restore_recovery_reference(&source, reference, 1 << 20, deadline)
            .expect("restored into kept");
    }
    for reference in &frames {
        let kept = Path::new(KEPT_DIRECTORY)
            .join(DATE)
            .join(LEAF)
            .join(kept_name(reference));
        assert!(target_root.join(&kept).is_file());
        assert!(
            !target_root
                .join(reference.object_key().expect("key"))
                .exists()
        );
        assert_eq!(read(&target_root, reference).expect("target read"), kept);
    }
}
