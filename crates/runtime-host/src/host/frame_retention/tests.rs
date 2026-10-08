// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #375 R5d: the cleaner's sweeps over a frame view, on real files in a temporary
//! state root.

use super::*;
use actingcommand_contract::{
    ArtifactProducer, ArtifactRedactionState, IdentifierIssuer, InstanceId, RetentionClass,
};
use actingcommand_ledger::FrameRetentionFrame;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

const T0: u64 = 1_791_753_302_117;
const DAY_MS: u64 = 86_400_000;
const NOW: u64 = T0 + 2 * DAY_MS;
const DATE: &str = "2026-10-12";
const ERROR_LEAF: &str = "node_a-151234-contained_task_page_unknown";
const LAB_LEAF: &str = "lab-node_a";

/// Frames of one instance in a temporary state root, and the view that classes them.
struct Root {
    _temp: tempfile::TempDir,
    path: PathBuf,
    ids: IdentifierIssuer,
    instance: InstanceId,
    frames: Vec<FrameRetentionFrame>,
}

impl Root {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("root");
        let path = temp.path().canonicalize().expect("canonical root");
        let ids = IdentifierIssuer::new().expect("issuer");
        let instance = *ids.mint_instance_id().expect("instance").transport();
        Self {
            _temp: temp,
            path,
            ids,
            instance,
            frames: Vec::new(),
        }
    }

    /// A frame of `class`; its file exists at its object key when `written`.
    fn frame(
        &mut self,
        class: FrameRetentionClass,
        due: Option<u64>,
        leaf: Option<&str>,
        written: bool,
    ) -> usize {
        let bytes = format!("frame {}", self.frames.len()).into_bytes();
        let artifact_id = *self.ids.mint_artifact_id().expect("artifact").transport();
        let frame_id = *self.ids.mint_frame_id().expect("frame").transport();
        let sha256 = format!("sha256:{:x}", Sha256::digest(&bytes));
        let object_file = format!(
            "{}.png",
            crate::failure_identity::identifier_text(&artifact_id)
        );
        let object_key = format!("artifacts/{}/{object_file}", &sha256[7..9]);
        let reference = ProjectedArtifactReference {
            artifact_id,
            kind: ArtifactKind::CaptureFrame,
            run_id: None,
            frame_id: Some(frame_id),
            correlation_id: None,
            object_key: Some(object_key.clone()),
            media_type: ArtifactKind::CaptureFrame.media_type(),
            byte_count: bytes.len() as u64,
            sha256,
            created_at_unix_ms: T0,
            producer: ArtifactProducer::CaptureStore,
            retention_class: RetentionClass::Adaptive,
            redaction_state: ArtifactRedactionState::NotRequired,
        };
        reference.validate().expect("valid reference");
        if written {
            let path = self.path.join(&object_key);
            std::fs::create_dir_all(path.parent().expect("shard")).expect("shard folder");
            std::fs::write(path, &bytes).expect("frame file");
        }
        let settled = class != FrameRetentionClass::Running;
        self.frames.push(FrameRetentionFrame {
            reference,
            instance_id: self.instance,
            run_id: None,
            class,
            entry_unix_ms: settled.then_some(T0),
            due_unix_ms: due,
            folder: leaf.map(|leaf| KeptFrameFolder {
                date: DATE.to_owned(),
                leaf: leaf.to_owned(),
                file_name: format!("061502-117_{object_file}"),
            }),
            windows: Vec::new(),
        });
        self.frames.len() - 1
    }

    fn view(&self) -> FrameRetentionView {
        FrameRetentionView {
            through_sequence: 1,
            evaluated_at_unix_ms: NOW,
            frames: self.frames.clone(),
            error_points: Vec::new(),
        }
    }

    fn object(&self, index: usize) -> PathBuf {
        self.path.join(
            self.frames[index]
                .reference
                .object_key()
                .expect("object key"),
        )
    }

    fn kept(&self, index: usize) -> PathBuf {
        let folder = self.frames[index].folder.as_ref().expect("kept folder");
        self.path
            .join(KEPT_DIRECTORY)
            .join(&folder.date)
            .join(&folder.leaf)
            .join(&folder.file_name)
    }

    fn bytes(&self, indexes: &[usize]) -> u64 {
        indexes
            .iter()
            .map(|index| self.frames[*index].reference.byte_count)
            .sum()
    }
}

fn cleaner() -> FrameRetention {
    FrameRetention::new(T0, FrameRetentionSwitches::default())
}

/// One whole sweep over `view`: its report, if printed, and its Warnings.
fn sweep(
    retention: &mut FrameRetention,
    root: &Path,
    view: &FrameRetentionView,
) -> (Option<SweepReport>, Vec<FrameRetentionWarning>) {
    retention.begin_sweep(view);
    let mut warnings = Vec::new();
    loop {
        let report = retention
            .round(root, &|| false, &mut |warning| {
                warnings.push(warning.clone());
                Ok(())
            })
            .expect("a round never fails on a file");
        if retention.sweep.is_none() {
            return (report, warnings);
        }
    }
}

#[test]
fn a_sweep_removes_due_frames_moves_kept_frames_and_leaves_the_rest() {
    let mut root = Root::new();
    let default_due = root.frame(FrameRetentionClass::Default, Some(T0 + DAY_MS), None, true);
    let default_later = root.frame(FrameRetentionClass::Default, Some(NOW + 1), None, true);
    let duplicate = root.frame(FrameRetentionClass::Duplicate, Some(T0), None, true);
    let error = root.frame(FrameRetentionClass::Error, None, Some(ERROR_LEAF), true);
    let lab = root.frame(FrameRetentionClass::Lab, None, Some(LAB_LEAF), true);
    let running = root.frame(FrameRetentionClass::Running, None, None, true);
    let absent = root.frame(FrameRetentionClass::Default, Some(T0), None, false);
    let reading = root.frame(
        FrameRetentionClass::Resource,
        Some(T0 + 7 * DAY_MS),
        None,
        true,
    );
    let mut retention = cleaner();
    let (report, warnings) = sweep(&mut retention, &root.path, &root.view());
    let report = report.expect("the first sweep prints its line");
    assert!(warnings.is_empty());
    assert_eq!(
        report,
        SweepReport {
            frames: 5,
            deleted: 2,
            deleted_bytes: root.bytes(&[default_due, duplicate]),
            moved: 2,
            moved_bytes: root.bytes(&[error, lab]),
            absent: 0,
            rescanned_absent: 1,
            busy: 0,
            failed: 0,
            kept_error: 1,
            kept_lab: 1,
            running: 1,
            rounds: 1,
            pass_ms: report.pass_ms,
        }
    );
    assert!(!root.object(default_due).exists());
    assert!(!root.object(duplicate).exists());
    assert!(!root.object(absent).exists());
    for untouched in [default_later, running, reading] {
        assert!(root.object(untouched).is_file(), "frame {untouched} stays");
    }
    for moved in [error, lab] {
        assert!(!root.object(moved).exists());
        assert!(root.kept(moved).is_file(), "frame {moved} is kept");
    }
    assert_eq!(
        std::fs::read(root.path.join(KEPT_DIRECTORY).join(".moves"))
            .expect("move counter")
            .len(),
        16
    );
    assert!(report.to_string().starts_with(
        "frames=5 deleted=2 deleted_bytes=14 moved=2 moved_bytes=14 absent=0 rescanned_absent=1 \
         busy=0 failed=0 kept_error=1 kept_lab=1 running=1 rounds=1 pass_ms="
    ));
    // Handled frames are never visited again: a later idle sweep prints nothing.
    let (again, warnings) = sweep(&mut retention, &root.path, &root.view());
    assert_eq!(again, None);
    assert!(warnings.is_empty());
}

#[test]
fn a_full_round_stops_at_sixteen_actions() {
    let mut root = Root::new();
    for _ in 0..20 {
        root.frame(FrameRetentionClass::Duplicate, Some(T0), None, true);
    }
    let mut retention = cleaner();
    retention.begin_sweep(&root.view());
    let first = retention
        .round(&root.path, &|| false, &mut |_| Ok(()))
        .expect("round");
    assert_eq!(first, None, "the sweep needs a second round");
    let report = retention
        .round(&root.path, &|| false, &mut |_| Ok(()))
        .expect("round")
        .expect("the sweep completes");
    assert_eq!((report.deleted, report.rounds), (20, 2));
}

#[cfg(windows)]
#[test]
fn a_frame_held_by_a_viewer_is_busy_and_removed_by_a_later_sweep() {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    let mut root = Root::new();
    let held = root.frame(FrameRetentionClass::Default, Some(T0), None, true);
    let viewer = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(root.object(held))
        .expect("viewer");
    let mut retention = cleaner();
    let (report, warnings) = sweep(&mut retention, &root.path, &root.view());
    let report = report.expect("first sweep");
    assert_eq!((report.busy, report.deleted, report.failed), (1, 0, 0));
    assert!(warnings.is_empty());
    assert!(root.object(held).is_file());
    drop(viewer);
    let (report, _) = sweep(&mut retention, &root.path, &root.view());
    assert_eq!(report.expect("retried").deleted, 1);
    assert!(!root.object(held).exists());
}

#[test]
fn a_failed_move_warns_once_and_leaves_the_frame() {
    let mut root = Root::new();
    let stuck = root.frame(FrameRetentionClass::Error, None, Some(ERROR_LEAF), true);
    std::fs::create_dir_all(root.path.join(KEPT_DIRECTORY)).expect("kept");
    std::fs::write(root.path.join(KEPT_DIRECTORY).join(DATE), b"a file").expect("stray");
    let mut retention = cleaner();
    let (report, warnings) = sweep(&mut retention, &root.path, &root.view());
    assert_eq!(report.expect("first sweep").failed, 1);
    assert_eq!(warnings.len(), 1);
    let warning = &warnings[0];
    assert_eq!(warning.code, HostCode::FrameRetentionMoveFailed);
    assert_eq!(
        warning.artifact_id,
        root.frames[stuck].reference.artifact_id
    );
    assert_eq!(
        warning.entry.as_deref(),
        Some(format!("kept/{DATE}/{ERROR_LEAF}").as_str())
    );
    assert!(warning.io_kind.is_some());
    assert_eq!(warning.stage(), "frame_retention.move");
    let message = warning.message();
    assert!(message.starts_with("host_code=frame_retention_move_failed artifact_id=artifact_"));
    assert!(message.split(' ').all(|token| {
        token
            .split_once('=')
            .is_some_and(|(key, value)| !key.is_empty() && !value.is_empty())
    }));
    assert!(root.object(stuck).is_file());
    // Left until a restart: the next sweep neither retries nor warns again.
    let (again, warnings) = sweep(&mut retention, &root.path, &root.view());
    assert_eq!(again, None);
    assert!(warnings.is_empty());
}

#[test]
fn a_kept_frame_stays_after_a_restart_with_another_class() {
    let mut root = Root::new();
    let kept = root.frame(FrameRetentionClass::Error, None, Some(ERROR_LEAF), true);
    let mut retention = cleaner();
    let (report, _) = sweep(&mut retention, &root.path, &root.view());
    assert_eq!(report.expect("first sweep").moved, 1);
    // After a restart a switch change classes the frame as a duplicate: the cleaner finds
    // nothing at the object key, and the kept file stays.
    root.frames[kept].class = FrameRetentionClass::Duplicate;
    root.frames[kept].due_unix_ms = Some(T0);
    root.frames[kept].folder = None;
    let mut restarted = cleaner();
    let (report, _) = sweep(&mut restarted, &root.path, &root.view());
    let report = report.expect("first sweep after the restart");
    assert_eq!((report.rescanned_absent, report.deleted), (1, 0));
    root.frames[kept].folder = Some(KeptFrameFolder {
        date: DATE.to_owned(),
        leaf: ERROR_LEAF.to_owned(),
        file_name: format!(
            "061502-117_{}.png",
            crate::failure_identity::identifier_text(&root.frames[kept].reference.artifact_id)
        ),
    });
    assert!(root.kept(kept).is_file());
}
