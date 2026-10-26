// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #375 R5d: the cleaner's passes, on real files in a temporary state root. #422 LN-7c
//! Q: the cleaner decides by ledger query; here its answers come from a fake (`Fake`) that
//! applies the query's rules to frames whose class each test sets (adapted: these tests built the
//! writer's frame view, which Q removed, and passed it to the cleaner). The fake's statements
//! report the steps a test gives them, and the tests drive the cleaner's clock.
//! - T-pace: on a fake clock and step counter, a statement starts no earlier than the one before
//!   plus its steps at the rate in effect, so the steps since a pass began never exceed the rate by
//!   more than one statement; a stop ends the round, and a stopped statement ends the pass.
//! - T-schedule: a live frame is examined at its settle age, then at 1 day (`keep`) and at 7 days
//!   (`keep_resource`) after its ingestion time, a running frame at every pass; a late window
//!   point after the frame settled moves it at its 1-day examination; a class that changes between
//!   the queueing and the round is acted on by the round's answer.
//! - T-evidence (the cleaner's half): a kept evidence frame is removed by the round's answer
//!   (confirmed, or at its maximum age), with its empty leaf and date folder; a kept file this
//!   ledger has no record of is left and counted; the maximum age is the one in effect at the
//!   pass start, so a change takes effect at the next pass.

use super::*;
use actingcommand_contract::{
    ArtifactProducer, ArtifactRedactionState, IdentifierIssuer, InstanceId,
    ProjectedArtifactReference, RetentionClass,
};
use actingcommand_ledger::FrameRetentionClass;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

mod gate_411_e3;
mod gate_h5g13;
mod gate_h5g14;
mod gate_ln7;

const T0: u64 = 1_791_753_302_117;
const NOW: u64 = T0 + 2 * DAY_MS;
const DATE: &str = "2026-10-12";
const ERROR_LEAF: &str = "node_a-151234-contained_task_page_unknown";
const LAB_LEAF: &str = "lab-node_a";
/// The settle age of a frame (the view's and statement L's).
const SETTLE_MS: u64 = 60_000;
/// The settings of the tests' passes: the keys' defaults.
const SETTINGS: CleanerSettings = CleanerSettings {
    evidence_max_age_days: 15,
    steps_per_second: 500_000,
    chunk_frames: 256,
};

/// One frame as the fake answers for it: its class (which a test may change), its due time
/// (a default, resource or duplicate frame's), its kept folder (a Lab or error frame's), and
/// whether an agent or a person confirmed reading it.
#[derive(Clone)]
struct FakeFrame {
    frame: FrameQueryFrame,
    class: FrameRetentionClass,
    due: Option<u64>,
    folder: Option<KeptFrameFolder>,
    confirmed: bool,
}

/// The fake ledger query: statement L and K by the query's rules over the frames a test gives it,
/// each statement reporting `steps` steps; it records what each statement asked.
#[derive(Default)]
struct Fake {
    frames: BTreeMap<ArtifactId, FakeFrame>,
    steps: u64,
    /// The frames each statement L asked about, with the time it was asked at.
    asked_live: Vec<(u64, Vec<ArtifactId>)>,
    asked_kept: Vec<Vec<ArtifactId>>,
    /// Statement W answers `Incomplete`, or `Stopped`.
    incomplete: bool,
    stopped: bool,
}

impl Fake {
    fn answered<T>(&self, value: T) -> FrameQueryOutcome<T> {
        if self.incomplete {
            FrameQueryOutcome::Incomplete
        } else if self.stopped {
            FrameQueryOutcome::Stopped
        } else {
            FrameQueryOutcome::Answered {
                value,
                steps: self.steps,
            }
        }
    }

    /// Statement L's rules: no row before the settle age; a confirmed evidence frame is removed;
    /// a Lab or error frame moved; a resource, duplicate or default frame removed at its due time
    /// and kept before it.
    fn live_answer(&self, frame: &FakeFrame, now: u64) -> Option<LiveFrameAnswer> {
        let at = frame.frame.reference.created_at_unix_ms;
        if at + SETTLE_MS > now {
            return None;
        }
        let due = frame.due.is_some_and(|due| due <= now);
        let action = match frame.class {
            FrameRetentionClass::Running => FrameQueryAction::Running,
            FrameRetentionClass::Lab | FrameRetentionClass::Error if frame.confirmed => {
                FrameQueryAction::RemoveConfirmed
            }
            FrameRetentionClass::Lab => FrameQueryAction::MoveLab,
            FrameRetentionClass::Error => FrameQueryAction::MoveError,
            FrameRetentionClass::Resource if due => FrameQueryAction::Remove,
            FrameRetentionClass::Resource => FrameQueryAction::KeepResource,
            FrameRetentionClass::Duplicate => FrameQueryAction::RemoveDuplicate,
            FrameRetentionClass::Default if due => FrameQueryAction::Remove,
            FrameRetentionClass::Default => FrameQueryAction::Keep,
        };
        Some(LiveFrameAnswer {
            artifact_id: frame.frame.reference.artifact_id,
            created_at_unix_ms: at,
            class: frame.class,
            action,
        })
    }
}

impl FrameAnswers for Fake {
    fn answer_waits(&mut self) -> RuntimeHostResult<FrameQueryOutcome<FrameWaits>> {
        Ok(self.answered(FrameWaits::none()))
    }

    fn answer_live(
        &mut self,
        frames: &[ArtifactId],
        _waits: &FrameWaits,
        settings: &FrameQuerySettings,
    ) -> RuntimeHostResult<FrameQueryOutcome<Vec<LiveFrameAnswer>>> {
        self.asked_live
            .push((settings.now_unix_ms, frames.to_vec()));
        let answers = frames
            .iter()
            .filter_map(|id| self.frames.get(id))
            .filter_map(|frame| self.live_answer(frame, settings.now_unix_ms))
            .collect();
        Ok(self.answered(answers))
    }

    fn answer_kept(
        &mut self,
        frames: &[ArtifactId],
        settings: &FrameQuerySettings,
    ) -> RuntimeHostResult<FrameQueryOutcome<Vec<KeptFrameAnswer>>> {
        self.asked_kept.push(frames.to_vec());
        let now = settings.now_unix_ms;
        let answers = frames
            .iter()
            .filter_map(|id| self.frames.get(id))
            .map(|frame| {
                let at = frame.frame.reference.created_at_unix_ms;
                KeptFrameAnswer {
                    artifact_id: frame.frame.reference.artifact_id,
                    created_at_unix_ms: at,
                    run_id: None,
                    action: if at + settings.evidence_max_age_ms <= now {
                        FrameQueryAction::RemoveExpired
                    } else if frame.confirmed {
                        FrameQueryAction::RemoveConfirmed
                    } else {
                        FrameQueryAction::Keep
                    },
                }
            })
            .collect();
        Ok(self.answered(answers))
    }

    fn verified_frame(
        &mut self,
        artifact: ArtifactId,
    ) -> RuntimeHostResult<Option<FrameQueryFrame>> {
        Ok(self.frames.get(&artifact).map(|frame| frame.frame.clone()))
    }

    fn kept_folder(
        &mut self,
        frame: &FrameQueryFrame,
        _lab: bool,
    ) -> RuntimeHostResult<KeptFrameFolder> {
        Ok(self
            .frames
            .get(&frame.reference.artifact_id)
            .and_then(|known| known.folder.clone())
            .expect("a moved fake frame has its folder"))
    }
}

/// Frames of one instance in a temporary state root.
struct Root {
    _temp: tempfile::TempDir,
    path: PathBuf,
    ids: IdentifierIssuer,
    instance: InstanceId,
    frames: Vec<FakeFrame>,
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

    /// A frame of `class` captured at `T0`; its file exists at its object key when `written`.
    fn frame(
        &mut self,
        class: FrameRetentionClass,
        due: Option<u64>,
        leaf: Option<&str>,
        written: bool,
    ) -> usize {
        self.frame_at(class, T0, due, leaf, written)
    }

    fn frame_at(
        &mut self,
        class: FrameRetentionClass,
        captured: u64,
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
            created_at_unix_ms: captured,
            producer: ArtifactProducer::CaptureStore,
            retention_class: RetentionClass::Adaptive,
            redaction_state: ArtifactRedactionState::NotRequired,
        };
        reference.validate().expect("valid reference");
        if written {
            let path = self.path.join(object_key.as_str());
            std::fs::create_dir_all(path.parent().expect("shard")).expect("shard folder");
            std::fs::write(path, bytes.as_slice()).expect("frame file");
        }
        self.frames.push(FakeFrame {
            frame: FrameQueryFrame {
                reference,
                instance_id: self.instance,
                run_id: None,
                verified_sequence: 1,
            },
            class,
            due,
            folder: leaf.map(|leaf| KeptFrameFolder {
                date: DATE.to_owned(),
                leaf: leaf.to_owned(),
                file_name: format!("061502-117_{object_file}"),
            }),
            confirmed: false,
        });
        self.frames.len() - 1
    }

    /// The fake ledger query over the root's frames, as they are now.
    fn answers(&self) -> Fake {
        Fake {
            frames: self
                .frames
                .iter()
                .map(|frame| (frame.frame.reference.artifact_id, frame.clone()))
                .collect(),
            ..Fake::default()
        }
    }

    /// Every frame of the root, as the listing of the live frame store names them (a frame
    /// whose file is gone is listed too, so the cleaner finds it absent).
    fn live(&self) -> BTreeSet<ArtifactId> {
        self.frames
            .iter()
            .map(|frame| frame.frame.reference.artifact_id)
            .collect()
    }

    fn id(&self, index: usize) -> ArtifactId {
        self.frames[index].frame.reference.artifact_id
    }

    fn object(&self, index: usize) -> PathBuf {
        self.path.join(
            self.frames[index]
                .frame
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

    /// The Lab folder of frame `index`, as the query names it once the frame is Lab.
    fn lab_folder(&self, index: usize) -> KeptFrameFolder {
        KeptFrameFolder {
            date: DATE.to_owned(),
            leaf: LAB_LEAF.to_owned(),
            file_name: format!(
                "061502-117_{}.png",
                crate::failure_identity::identifier_text(&self.id(index))
            ),
        }
    }

    fn bytes(&self, indexes: &[usize]) -> u64 {
        indexes
            .iter()
            .map(|index| self.frames[*index].frame.reference.byte_count)
            .sum()
    }

    /// The kept listing of the root, as a pass start reads it.
    fn kept_listing(&self) -> Vec<KeptLeafListing> {
        list_kept_folders(&self.path).expect("the kept listing")
    }
}

fn cleaner() -> FrameRetention {
    FrameRetention::new(T0, FrameRetentionSwitches::default())
}

fn clock(unix_ms: u64, monotonic_ms: u64) -> CleanerClock {
    CleanerClock {
        unix_ms,
        monotonic_ms,
    }
}

/// One whole pass at `NOW` over `fake` (steps of zero: every statement may start at once): its
/// line, if printed, and its Warnings.
fn sweep(
    retention: &mut FrameRetention,
    root: &Root,
    fake: &mut Fake,
) -> (Option<SweepReport>, Vec<FrameRetentionWarning>) {
    sweep_with(retention, root, fake, Vec::new())
}

fn sweep_with(
    retention: &mut FrameRetention,
    root: &Root,
    fake: &mut Fake,
    kept: Vec<KeptLeafListing>,
) -> (Option<SweepReport>, Vec<FrameRetentionWarning>) {
    let started = retention
        .begin_pass(fake, root.live(), kept, clock(NOW, 0), SETTINGS)
        .expect("a pass begins");
    assert_eq!(started, PassStart::Started);
    let mut warnings = Vec::new();
    loop {
        let ended = retention
            .round(
                fake,
                &root.path,
                clock(NOW, 0),
                SETTINGS,
                &|| false,
                &mut |warning| {
                    warnings.push(warning.clone());
                    Ok(())
                },
            )
            .expect("a round never fails on a file");
        if retention.pass.is_none() {
            let report = match ended {
                Some(PassEnd::Completed(report)) => Some(report),
                Some(PassEnd::Skipped) => panic!("the fake covers the ledger"),
                None => None,
            };
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
    let (report, warnings) = sweep(&mut retention, &root, &mut root.answers());
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
            ..SweepReport::default()
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
    assert!(report.to_string().ends_with(
        " kept_examined=0 removed_confirmed=0 removed_expired=0 kept_unknown=0 query_steps=0"
    ));
    // Handled frames are never visited again: a later idle sweep prints nothing.
    let (again, warnings) = sweep(&mut retention, &root, &mut root.answers());
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
    let mut fake = root.answers();
    retention
        .begin_pass(&mut fake, root.live(), Vec::new(), clock(NOW, 0), SETTINGS)
        .expect("a pass begins");
    let first = retention
        .round(
            &mut fake,
            &root.path,
            clock(NOW, 0),
            SETTINGS,
            &|| false,
            &mut |_| Ok(()),
        )
        .expect("round");
    assert_eq!(first, None, "the sweep needs a second round");
    let Some(PassEnd::Completed(report)) = retention
        .round(
            &mut fake,
            &root.path,
            clock(NOW, 0),
            SETTINGS,
            &|| false,
            &mut |_| Ok(()),
        )
        .expect("round")
    else {
        panic!("the sweep completes");
    };
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
    let (report, warnings) = sweep(&mut retention, &root, &mut root.answers());
    let report = report.expect("first sweep");
    assert_eq!((report.busy, report.deleted, report.failed), (1, 0, 0));
    assert!(warnings.is_empty());
    assert!(root.object(held).is_file());
    drop(viewer);
    let (report, _) = sweep(&mut retention, &root, &mut root.answers());
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
    let (report, warnings) = sweep(&mut retention, &root, &mut root.answers());
    assert_eq!(report.expect("first sweep").failed, 1);
    assert_eq!(warnings.len(), 1);
    let warning = &warnings[0];
    assert_eq!(warning.code, HostCode::FrameRetentionMoveFailed);
    assert_eq!(warning.artifact_id, Some(root.id(stuck)));
    assert_eq!(
        warning.entry.as_deref(),
        Some(format!("kept/{DATE}/{ERROR_LEAF}").as_str())
    );
    assert!(warning.io_kind.is_some());
    assert_eq!(warning.stage().as_str(), "frame_retention_move");
    // Ledger revision 3: the warning's record carries its outcome, with typed values.
    let outcome = warning.outcome();
    assert_eq!(outcome.code().as_str(), "frame_retention_move_failed");
    assert!(
        outcome
            .value("artifact_id")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|id| id.starts_with("artifact_"))
    );
    assert_eq!(
        outcome.value("entry").and_then(serde_json::Value::as_str),
        Some(format!("kept/{DATE}/{ERROR_LEAF}").as_str())
    );
    assert!(outcome.value("io_kind").is_some());
    assert!(root.object(stuck).is_file());
    // Left until a restart: the next sweep neither retries nor warns again.
    let (again, warnings) = sweep(&mut retention, &root, &mut root.answers());
    assert_eq!(again, None);
    assert!(warnings.is_empty());
}

#[test]
fn a_kept_frame_stays_after_a_restart_with_another_class() {
    let mut root = Root::new();
    let kept = root.frame(FrameRetentionClass::Error, None, Some(ERROR_LEAF), true);
    let mut retention = cleaner();
    let (report, _) = sweep(&mut retention, &root, &mut root.answers());
    assert_eq!(report.expect("first sweep").moved, 1);
    // After a restart a switch change classes the frame as a duplicate: the cleaner finds
    // nothing at the object key, and the kept file stays (statement K keeps it: unconfirmed and
    // younger than its maximum age).
    let kept_path = root.kept(kept);
    root.frames[kept].class = FrameRetentionClass::Duplicate;
    root.frames[kept].due = Some(T0);
    root.frames[kept].folder = None;
    let mut restarted = cleaner();
    let (report, _) = sweep_with(
        &mut restarted,
        &root,
        &mut root.answers(),
        root.kept_listing(),
    );
    let report = report.expect("first sweep after the restart");
    assert_eq!(
        (
            report.rescanned_absent,
            report.deleted,
            report.kept_examined
        ),
        (1, 0, 1)
    );
    assert!(kept_path.is_file());
}

/// Model v4.3 note, and T-schedule's last case: a frame whose class changes between its queueing
/// and its round is acted on by the round's answer. Here a due default frame becomes Lab output
/// after the first round: the second round moves it into its Lab folder and never removes it.
#[test]
fn a_frame_that_becomes_lab_during_a_sweep_is_moved_not_removed() {
    let mut root = Root::new();
    let first_round = (0..RETENTION_ROUND_OBJECTS)
        .map(|_| root.frame(FrameRetentionClass::Default, Some(T0), None, true))
        .collect::<Vec<_>>();
    let protected = root.frame(FrameRetentionClass::Default, Some(T0), None, true);
    let mut retention = cleaner();
    let mut fake = root.answers();
    retention
        .begin_pass(&mut fake, root.live(), Vec::new(), clock(NOW, 0), SETTINGS)
        .expect("a pass begins");
    let first = retention
        .round(
            &mut fake,
            &root.path,
            clock(NOW, 0),
            SETTINGS,
            &|| false,
            &mut |_| Ok(()),
        )
        .expect("round");
    assert_eq!(first, None, "the sweep needs a second round");
    for index in &first_round {
        assert!(!root.object(*index).exists());
    }
    assert!(root.object(protected).is_file());
    // Between the rounds a later Lab use of its run makes the frame Lab output.
    let folder = root.lab_folder(protected);
    let changed = fake
        .frames
        .get_mut(&root.id(protected))
        .expect("the protected frame");
    changed.class = FrameRetentionClass::Lab;
    changed.due = None;
    changed.folder = Some(folder);
    root.frames[protected].folder = changed.folder.clone();
    let Some(PassEnd::Completed(report)) = retention
        .round(
            &mut fake,
            &root.path,
            clock(NOW, 0),
            SETTINGS,
            &|| false,
            &mut |_| Ok(()),
        )
        .expect("round")
    else {
        panic!("the sweep completes");
    };
    assert_eq!((report.deleted, report.moved), (16, 1));
    assert!(!root.object(protected).exists());
    assert!(root.kept(protected).is_file(), "moved, not removed");
}

/// Model v4.3 note: a failed write of the move counter after a successful move has its own
/// code; the frame counts as moved and nothing failed.
#[test]
fn a_counter_that_cannot_be_written_has_its_own_warning() {
    let mut root = Root::new();
    let kept = root.frame(FrameRetentionClass::Error, None, Some(ERROR_LEAF), true);
    // A folder where the counter file should be makes its write fail.
    std::fs::create_dir_all(root.path.join(KEPT_DIRECTORY).join(".moves")).expect("blocker");
    let mut retention = cleaner();
    let (report, warnings) = sweep(&mut retention, &root, &mut root.answers());
    let report = report.expect("first sweep");
    assert_eq!((report.moved, report.failed), (1, 0));
    assert!(root.kept(kept).is_file());
    assert_eq!(warnings.len(), 1);
    let warning = &warnings[0];
    assert_eq!(warning.code, HostCode::FrameRetentionCounterFailed);
    assert_eq!(warning.artifact_id, None);
    assert_eq!(warning.entry.as_deref(), Some("kept/.moves"));
    assert_eq!(warning.stage().as_str(), "frame_retention_counter");
    let outcome = warning.outcome();
    assert_eq!(outcome.code().as_str(), "frame_retention_counter_failed");
    assert_eq!(
        outcome.value("entry").and_then(serde_json::Value::as_str),
        Some("kept/.moves")
    );
}

// ---- #422 LN-7c Q

/// T-pace: the fake's statements report 1 000 000 steps each at 500 000 steps a second, so each
/// starts no earlier than 2 s after the one before; the clock jumps by uneven steps. At every
/// statement's start, the steps of the statements before it are within the rate since the pass
/// began.
#[test]
fn ln7c_q_t_pace_steps_never_exceed_the_rate_by_more_than_one_statement() {
    const STEPS: u64 = 1_000_000;
    let mut root = Root::new();
    for index in 0..10_u64 {
        root.frame_at(
            FrameRetentionClass::Default,
            T0 + index,
            Some(NOW + DAY_MS),
            None,
            true,
        );
    }
    let settings = CleanerSettings {
        chunk_frames: 2,
        ..SETTINGS
    };
    let mut fake = root.answers();
    fake.steps = STEPS;
    let mut retention = cleaner();
    retention
        .begin_pass(&mut fake, root.live(), Vec::new(), clock(NOW, 0), settings)
        .expect("a pass begins");
    // W was the first statement, at 0 ms.
    let mut starts = vec![0_u64];
    let mut now_ms = 0;
    let mut round = 0;
    while retention.pass.is_some() {
        round += 1;
        assert!(round < 1_000, "the pass ends");
        now_ms += [700, 1_300, 2_100, 450][round % 4];
        let asked = fake.asked_live.len();
        retention
            .round(
                &mut fake,
                &root.path,
                clock(NOW, now_ms),
                settings,
                &|| false,
                &mut |_| Ok(()),
            )
            .expect("round");
        for _ in asked..fake.asked_live.len() {
            starts.push(now_ms);
        }
    }
    // W, then five chunks of two frames (all kept, so nothing is re-checked).
    assert_eq!(starts.len(), 6, "{starts:?}");
    let rate = settings.steps_per_second;
    for (index, start) in starts.iter().enumerate() {
        let before = STEPS * index as u64;
        assert!(
            before * 1_000 <= rate * start,
            "statement {index} at {start} ms: {before} steps before it exceed the rate"
        );
        if index > 0 {
            assert!(
                start - starts[index - 1] >= STEPS * 1_000 / rate,
                "statement {index} started too soon after the one before"
            );
        }
    }
    // The rate in effect is read after each statement: W was paced at the rate of the pass's
    // start (2 s), the chunk after it at the doubled rate (1 s, not 2 s).
    let mut fast = root.answers();
    fast.steps = STEPS;
    let mut retention = cleaner();
    retention
        .begin_pass(&mut fast, root.live(), Vec::new(), clock(NOW, 0), settings)
        .expect("a pass begins");
    let quicker = CleanerSettings {
        steps_per_second: 2 * settings.steps_per_second,
        ..settings
    };
    retention
        .round(
            &mut fast,
            &root.path,
            clock(NOW, 1_000),
            quicker,
            &|| false,
            &mut |_| Ok(()),
        )
        .expect("round");
    assert!(
        fast.asked_live.is_empty(),
        "W was paced at the rate of its start"
    );
    retention
        .round(
            &mut fast,
            &root.path,
            clock(NOW, 2_000),
            quicker,
            &|| false,
            &mut |_| Ok(()),
        )
        .expect("round");
    assert_eq!(fast.asked_live.len(), 1);
    retention
        .round(
            &mut fast,
            &root.path,
            clock(NOW, 2_999),
            quicker,
            &|| false,
            &mut |_| Ok(()),
        )
        .expect("round");
    assert_eq!(fast.asked_live.len(), 1, "the next chunk waits 1 s");
    retention
        .round(
            &mut fast,
            &root.path,
            clock(NOW, 3_000),
            quicker,
            &|| false,
            &mut |_| Ok(()),
        )
        .expect("round");
    assert_eq!(
        fast.asked_live.len(),
        2,
        "the next chunk starts 1 s later at the doubled rate"
    );
}

/// T-pace: the stop. A raised stop ends the round before any statement; a statement the stop
/// interrupted ends the pass without acting, and nothing is printed.
#[test]
fn ln7c_q_t_pace_the_stop_interrupts() {
    let mut root = Root::new();
    let due = root.frame(FrameRetentionClass::Default, Some(T0), None, true);
    let mut fake = root.answers();
    let mut retention = cleaner();
    retention
        .begin_pass(&mut fake, root.live(), Vec::new(), clock(NOW, 0), SETTINGS)
        .expect("a pass begins");
    let ended = retention
        .round(
            &mut fake,
            &root.path,
            clock(NOW, 0),
            SETTINGS,
            &|| true,
            &mut |_| Ok(()),
        )
        .expect("round");
    assert_eq!(ended, None);
    assert!(fake.asked_live.is_empty(), "no statement after the stop");
    assert!(
        retention.pass.is_some(),
        "the pass waits for the next round"
    );
    // The query was interrupted: the pass ends without acting.
    fake.stopped = true;
    let ended = retention
        .round(
            &mut fake,
            &root.path,
            clock(NOW, 0),
            SETTINGS,
            &|| false,
            &mut |_| Ok(()),
        )
        .expect("round");
    assert_eq!(ended, None);
    assert!(retention.pass.is_none(), "the stopped pass ended");
    assert!(root.object(due).is_file(), "nothing was acted on");
    // A pass whose W is stopped does not begin.
    let mut stopped = root.answers();
    stopped.stopped = true;
    assert_eq!(
        cleaner()
            .begin_pass(
                &mut stopped,
                root.live(),
                Vec::new(),
                clock(NOW, 0),
                SETTINGS
            )
            .expect("a pass begins"),
        PassStart::Stopped
    );
    // An index that does not cover the ledger skips the pass: nothing is decided.
    let mut incomplete = root.answers();
    incomplete.incomplete = true;
    assert_eq!(
        cleaner()
            .begin_pass(
                &mut incomplete,
                root.live(),
                Vec::new(),
                clock(NOW, 0),
                SETTINGS
            )
            .expect("a pass begins"),
        PassStart::Skipped
    );
}

/// One pass at `now` (its rounds until it ends) over `fake`; the frames statement L asked about.
fn pass_at(
    retention: &mut FrameRetention,
    root: &Root,
    fake: &mut Fake,
    now: u64,
) -> BTreeSet<ArtifactId> {
    fake.asked_live.clear();
    retention
        .begin_pass(
            fake,
            root.live(),
            root.kept_listing(),
            clock(now, 0),
            SETTINGS,
        )
        .expect("a pass begins");
    for _ in 0..100 {
        if retention.pass.is_none() {
            break;
        }
        retention
            .round(
                fake,
                &root.path,
                clock(now, 0),
                SETTINGS,
                &|| false,
                &mut |_| Ok(()),
            )
            .expect("round");
    }
    assert!(retention.pass.is_none(), "the pass ends");
    // The examination's statements (a re-check asks again about frames it queued).
    fake.asked_live
        .iter()
        .take(1)
        .flat_map(|(_, frames)| frames.iter().copied())
        .collect()
}

/// T-schedule (§1.4): when each live frame is examined.
#[test]
fn ln7c_q_t_schedule_examined_at_settle_at_one_day_and_at_seven_days_running_every_pass() {
    let mut root = Root::new();
    let ordinary = root.frame_at(
        FrameRetentionClass::Default,
        T0,
        Some(T0 + DAY_MS),
        None,
        true,
    );
    let reading = root.frame_at(
        FrameRetentionClass::Resource,
        T0,
        Some(T0 + 7 * DAY_MS),
        None,
        true,
    );
    let running = root.frame_at(FrameRetentionClass::Running, T0, None, None, true);
    let late = root.frame_at(
        FrameRetentionClass::Default,
        T0,
        Some(T0 + DAY_MS),
        Some(ERROR_LEAF),
        true,
    );
    let mut fake = root.answers();
    let mut retention = cleaner();
    let ids = |indexes: &[usize]| {
        indexes
            .iter()
            .map(|index| root.id(*index))
            .collect::<BTreeSet<_>>()
    };

    // Before its settle age a frame has no row, and is asked about again at every pass.
    let asked = pass_at(&mut retention, &root, &mut fake, T0 + 30_000);
    assert_eq!(asked, ids(&[ordinary, reading, running, late]));
    // At settle each is examined; `keep` and `keep_resource` schedule the next examination.
    let asked = pass_at(&mut retention, &root, &mut fake, T0 + SETTLE_MS);
    assert_eq!(asked, ids(&[ordinary, reading, running, late]));
    // A late window point after the frame settled: the frame is error now, but it is not
    // examined before its 1-day age.
    fake.frames
        .get_mut(&root.id(late))
        .expect("the late frame")
        .class = FrameRetentionClass::Error;
    for now in [T0 + 10 * 60_000, T0 + DAY_MS / 2, T0 + DAY_MS - 1] {
        let asked = pass_at(&mut retention, &root, &mut fake, now);
        assert_eq!(asked, ids(&[running]), "only the running frame at {now}");
    }
    assert!(
        root.object(late).is_file(),
        "not moved before its 1-day examination"
    );
    // At 1 day: the default frame is removed, the late frame moved into its error folder, the
    // resource frame not examined yet.
    let asked = pass_at(&mut retention, &root, &mut fake, T0 + DAY_MS);
    assert_eq!(asked, ids(&[ordinary, running, late]));
    assert!(!root.object(ordinary).exists(), "removed at 1 day");
    assert!(!root.object(late).exists());
    assert!(
        root.kept(late).is_file(),
        "the late window point moves it at 1 day"
    );
    for now in [T0 + 2 * DAY_MS, T0 + 7 * DAY_MS - 1] {
        let asked = pass_at(&mut retention, &root, &mut fake, now);
        assert_eq!(asked, ids(&[running]), "only the running frame at {now}");
    }
    // At 7 days the resource frame is examined and removed; the running frame at every pass.
    let asked = pass_at(&mut retention, &root, &mut fake, T0 + 7 * DAY_MS);
    assert_eq!(asked, ids(&[reading, running]));
    assert!(!root.object(reading).exists(), "removed at 7 days");
    assert!(root.object(running).is_file(), "a running frame stays");
}

/// T-evidence (the cleaner's half): the kept frames' evidence rule, acted on by the round's
/// answer, with the empty folders; the maximum age in effect at each pass start.
#[test]
fn ln7c_q_t_evidence_kept_frames_go_when_confirmed_or_at_the_age_the_pass_starts_with() {
    let mut root = Root::new();
    let confirmed = root.frame(FrameRetentionClass::Error, None, Some(ERROR_LEAF), true);
    let older = root.frame_at(
        FrameRetentionClass::Lab,
        T0 - 12 * DAY_MS,
        None,
        Some(LAB_LEAF),
        true,
    );
    let young = root.frame(FrameRetentionClass::Lab, None, Some(LAB_LEAF), true);
    // Their first pass moves the three evidence frames into `kept\`.
    let mut retention = cleaner();
    let (report, _) = sweep(&mut retention, &root, &mut root.answers());
    assert_eq!(report.expect("the first pass").moved, 3);
    // A kept file this ledger has no record of.
    let stranger = root
        .path
        .join(KEPT_DIRECTORY)
        .join(DATE)
        .join(LAB_LEAF)
        .join("061502-117_artifact_000000000000000000000000000000aa.png");
    std::fs::write(&stranger, b"a frame of another ledger").expect("a stranger kept file");

    // An agent confirms reading the error frame: the next pass removes it, and its leaf with it.
    root.frames[confirmed].confirmed = true;
    let (report, warnings) = sweep_with(
        &mut retention,
        &root,
        &mut root.answers(),
        root.kept_listing(),
    );
    let report = report.expect("a pass that removes");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        (
            report.kept_examined,
            report.removed_confirmed,
            report.removed_expired,
            report.kept_unknown,
            report.deleted
        ),
        (4, 1, 0, 1, 1)
    );
    assert!(!root.kept(confirmed).exists());
    assert!(
        !root
            .path
            .join(KEPT_DIRECTORY)
            .join(DATE)
            .join(ERROR_LEAF)
            .exists(),
        "the emptied leaf is removed"
    );
    assert!(stranger.is_file(), "a kept file of no known frame stays");

    // The maximum age is the one in effect at a pass start: 15 days keeps the 12-day-old frame
    // two days after its move, 10 days removes it at the next pass.
    let shorter = CleanerSettings {
        evidence_max_age_days: 10,
        ..SETTINGS
    };
    let mut fake = root.answers();
    retention
        .begin_pass(
            &mut fake,
            root.live(),
            root.kept_listing(),
            clock(NOW, 0),
            SETTINGS,
        )
        .expect("a pass begins");
    // A change in the middle of a pass does not reach it.
    while retention.pass.is_some() {
        retention
            .round(
                &mut fake,
                &root.path,
                clock(NOW, 0),
                shorter,
                &|| false,
                &mut |_| Ok(()),
            )
            .expect("round");
    }
    assert!(root.kept(older).is_file(), "15 days at this pass's start");
    let started = retention
        .begin_pass(
            &mut fake,
            root.live(),
            root.kept_listing(),
            clock(NOW, 0),
            shorter,
        )
        .expect("a pass begins");
    assert_eq!(started, PassStart::Started);
    let mut ended = None;
    while retention.pass.is_some() {
        ended = retention
            .round(
                &mut fake,
                &root.path,
                clock(NOW, 0),
                shorter,
                &|| false,
                &mut |_| Ok(()),
            )
            .expect("round");
    }
    let Some(PassEnd::Completed(report)) = ended else {
        panic!("the pass that removes prints its line");
    };
    assert_eq!((report.removed_expired, report.removed_confirmed), (1, 0));
    assert!(
        !root.kept(older).exists(),
        "10 days at the next pass's start"
    );
    assert!(
        root.kept(young).is_file(),
        "the young unconfirmed frame stays"
    );
    assert!(
        root.path
            .join(KEPT_DIRECTORY)
            .join(DATE)
            .join(LAB_LEAF)
            .is_dir(),
        "a leaf that still holds files stays"
    );
}
