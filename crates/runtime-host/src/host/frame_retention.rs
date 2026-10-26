// SPDX-License-Identifier: AGPL-3.0-only

use crate::codes::HostLocation;
use actingcommand_artifact_store::{
    ArtifactStore, ArtifactStoreError, ArtifactStoreResult, FrameFileOutcome, KEPT_DIRECTORY,
    KeptLeafListing, KeptMoves, StoreLocation, list_kept_folders, move_frame_file,
    remove_frame_file, remove_kept_frame_file, try_artifact_delete_guard,
};
use actingcommand_contract::codes::ContractLocation;
use actingcommand_contract::outcome::Code;
use actingcommand_contract::{
    ArtifactId, ArtifactKind, ArtifactPayloadDraft, ArtifactPinReason, ArtifactPinRecord,
    ArtifactRetentionFact, ArtifactRetentionIdentity, AuditInput, DiagnosticCode,
    DiagnosticDetailDraft, EffectDisposition, EventActor, EventLinksDraft, EventSeverity,
    EventSource, EventType, FRAME_RETENTION_POLICY_VERSION, OriginModule, OwnerEpoch,
    RETENTION_ROUND_BYTES, RETENTION_ROUND_OBJECTS, RETENTION_ROUND_START_BUDGET_MS,
    RuntimeErrorCode, RuntimePayloadDraft, TerminalEvent,
};
use actingcommand_ledger::{
    ArtifactEvictionAdmission, FrameQuery, FrameQueryAction, FrameQueryFrame, FrameQueryOutcome,
    FrameQuerySettings, FrameQueryStop, FrameRetentionSwitches, FrameWaits, GlobalLedger,
    GlobalLedgerError, KeptFrameAnswer, KeptFrameFolder, LiveFrameAnswer, PersistedEvent,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::codes::HostCode;
use crate::{RuntimeHostError, RuntimeHostResult, events::RuntimeEvents};

impl super::HostShared {
    /// The configuration of every capture frame store the host opens: the host's physical memory
    /// sample, and (#411 A67-R) the host's one floor watch, so the budgets of all of them enter
    /// and leave the floor together.
    pub(super) fn capture_frame_store_config(
        &self,
    ) -> actingcommand_artifact_store::FrameStoreConfig {
        let config = actingcommand_artifact_store::FrameStoreConfig::default()
            .with_floor_watch(Arc::clone(&self.frame_store_floor));
        #[cfg(test)]
        if let Some(sample) = self.capture_memory_sample_for_test {
            return config.with_memory_source(
                actingcommand_artifact_store::MemorySampleSource::fixed(sample),
            );
        }
        config.with_memory_source(actingcommand_artifact_store::MemorySampleSource::live(
            || {
                let sample =
                    actingcommand_host_metrics::sample_physical_memory().map_err(|code| {
                        ArtifactStoreError::fatal(
                            code.as_str(),
                            "sample_capture_memory",
                            code.as_str(),
                        )
                    })?;
                Ok(actingcommand_artifact_store::MemorySample {
                    total_bytes: sample.total_bytes,
                    available_bytes: sample.available_bytes,
                })
            },
        ))
    }

    /// #411 A67-R (A67 R5): records each `frame_store_budget_floor_applied` the capture frame
    /// stores' budget entered since the last call, at the start's preparation or at a refresh:
    /// first to the daemon's log, then as one Warning `runtime.failed` with system links whose
    /// outcome is the warning, as the host's other warnings are. A budget that stays on the floor
    /// has nothing more to record; one that left it and came back is recorded again.
    pub(super) fn record_frame_store_floor_entries(&self) -> RuntimeHostResult<()> {
        let entered = self
            .frame_store_floor
            .take_entered()
            .map_err(RuntimeHostError::artifact)?;
        for warning in entered {
            self.fatal.report_warning(&warning);
            self.append_event_raw(
                // The envelope derives the record's severity from the warning's category.
                EventSeverity::Warning,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                self.events.system_links()?,
                RuntimePayloadDraft::failed(
                    DiagnosticCode::RuntimeDiagnostic,
                    EffectDisposition::NotPerformed,
                    warning,
                    DiagnosticDetailDraft::new(
                        StoreLocation::FrameStore.as_str(),
                        StoreLocation::FrameStore,
                        "artifact_store",
                        StoreLocation::FrameStore.as_str(),
                    ),
                    AuditInput::new(),
                ),
            )?;
        }
        Ok(())
    }
}

pub(super) fn capture_pin_reason(
    request: &actingcommand_contract::ValidatedRuntimeRequest<'_>,
) -> ArtifactPinReason {
    if request.actor() == EventActor::Lab && request.source() == EventSource::Lab {
        ArtifactPinReason::Lab
    } else {
        ArtifactPinReason::Explicit
    }
}

pub(super) fn spill_root(
    root: &std::path::Path,
    identity: &impl serde::Serialize,
) -> ArtifactStoreResult<std::path::PathBuf> {
    use sha2::{Digest, Sha256};
    let canonical = serde_json::to_vec(identity).map_err(|error| {
        ArtifactStoreError::fatal(
            "frame_spill_identity_invalid",
            ContractLocation::OpenCapturePipeline.as_str(),
            error.to_string(),
        )
    })?;
    Ok(root
        .join("frame-spills")
        .join(format!("{:x}", Sha256::digest(canonical))))
}

impl super::HostShared {
    /// Workflow #375 R5d: one cleaner round. Its errors are ledger errors only; a frame that
    /// cannot be removed or moved is a Warning. Workflow #383 AO-1 (`contracts/ledger-observer.md`
    /// §9.5): a ledger read that fails without a fatal failure (a reply that did not come in
    /// time, a query statement that failed) is recorded as the round's error and the next pass
    /// reads again; only a fatal failure stops the Runtime. #422 LN-7c Q: the cleaner decides by
    /// ledger query; its keys are read here (`frame_cleaner_settings`), and the shutdown
    /// interrupts its statement in progress.
    pub(super) fn maintain_frame_retention(&self) -> RuntimeHostResult<bool> {
        let result = (|| {
            // Try-only, so a `clear-kept` that holds the lock makes the round skip.
            let mut retention = match self.frame_retention.try_lock() {
                Ok(retention) => retention,
                Err(std::sync::TryLockError::WouldBlock) => return Ok(true),
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    return Err(super::lock_poison_error(
                        HostLocation::MaintainFrameRetention,
                    ));
                }
            };
            let Some(retention) = retention.as_mut() else {
                return Ok(false);
            };
            // The Runtime's clock, which also dates the ledger's events.
            let clock = CleanerClock {
                unix_ms: self.clock.sample()?.unix_ms,
                monotonic_ms: retention.monotonic_ms(),
            };
            let settings = self.frame_cleaner_settings()?;
            let fatal = self.fatal.clone();
            let stop: FrameQueryStop = Arc::new(move || fatal.is_shutdown_requested());
            let ended = retention.maintain(
                &self.ledger,
                self.artifacts.root(),
                clock,
                settings,
                stop,
                &|| self.fatal.is_shutdown_requested(),
                &mut |warning| self.record_frame_retention_warning(warning),
            )?;
            match ended {
                Some(PassEnd::Completed(report)) => {
                    println!("actingd frame_retention pass {report}");
                }
                Some(PassEnd::Skipped) => {
                    println!("actingd frame_retention pass skipped=frame_refs_incomplete");
                }
                None => {}
            }
            Ok(true)
        })();
        match result {
            Ok(enabled) => Ok(enabled),
            Err(error) => {
                let mut failure = None;
                self.record_lifecycle_result(
                    super::RuntimeLifecycleFailureStage::OperationCleanup,
                    &mut failure,
                    Err(error),
                );
                let error =
                    failure.expect("retention error remains explicit after lifecycle recording");
                if !error.is_fatal() {
                    return Ok(true);
                }
                self.fatal.mark(error.clone())?;
                Err(error)
            }
        }
    }

    /// One Warning `runtime.failed` with system links for a frame the cleaner could not remove
    /// or move, or whose use lock it could not delete (Workflow #411 A29); its outcome carries
    /// the frame, the kept folder and the OS error as values.
    fn record_frame_retention_warning(
        &self,
        warning: &FrameRetentionWarning,
    ) -> RuntimeHostResult<()> {
        self.append_event_raw(
            EventSeverity::Warning,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            self.events.system_links()?,
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::NotPerformed,
                warning.outcome(),
                DiagnosticDetailDraft::new(
                    "frame_retention",
                    warning.stage(),
                    "artifact_store",
                    HostLocation::MaintainFrameRetention.as_str(),
                ),
                AuditInput::new(),
            ),
        )?;
        Ok(())
    }
}

/// The publication sink calls this while the original material publication guard is held.
pub(super) fn pin_published_frames(
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
    owner_epoch: OwnerEpoch,
    verified: &PersistedEvent,
    reason: ArtifactPinReason,
) -> ArtifactStoreResult<()> {
    if verified.event_type() != EventType::ArtifactVerified {
        return Err(pin_failure(HostCode::ArtifactPinSourceNotVerified.as_str()));
    }
    let links = verified.links();
    for artifact in verified
        .artifacts()
        .iter()
        .filter(|artifact| artifact.kind() == ArtifactKind::CaptureFrame)
    {
        let identity = ArtifactRetentionIdentity {
            artifact: artifact.project(true),
            owner_epoch,
            instance_id: *links
                .instance_id()
                .ok_or_else(|| pin_failure(HostCode::ArtifactPinInstanceMissing.as_str()))?,
            request_id: *links
                .request_id()
                .ok_or_else(|| pin_failure(HostCode::ArtifactPinRequestMissing.as_str()))?,
            correlation_id: *links
                .correlation_id()
                .ok_or_else(|| pin_failure(HostCode::ArtifactPinCorrelationMissing.as_str()))?,
            run_id: links.run_id().copied(),
            lease_id: links.lease_id().copied(),
            policy_version: FRAME_RETENTION_POLICY_VERSION,
        };
        let draft = events
            .draft(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::ArtifactStore,
                EventActor::Runtime,
                EventLinksDraft::default(),
                ArtifactPayloadDraft::retention(
                    ArtifactRetentionFact::PinRecorded(ArtifactPinRecord {
                        identity,
                        reason,
                        trigger: TerminalEvent {
                            event_id: *verified.event_id(),
                            sequence: verified.sequence(),
                        },
                    }),
                    AuditInput::new(),
                ),
            )
            .and_then(|draft| events.sanitize(draft))
            .map_err(|error| {
                ArtifactStoreError::fatal(error.code(), "pin_published_frame", error.to_string())
            })?;
        let draft = links
            .artifact_retention_source()
            .apply_to(draft)
            .map_err(|error| {
                ArtifactStoreError::fatal(
                    "artifact_pin_source_conflict",
                    "pin_published_frame",
                    error.to_string(),
                )
            })?;
        ledger.append(draft).map_err(|error| {
            ArtifactStoreError::fatal(error.code(), "pin_published_frame", error.to_string())
        })?;
    }
    Ok(())
}

fn pin_failure(code: &'static str) -> ArtifactStoreError {
    ArtifactStoreError::fatal(
        code,
        "pin_published_frame",
        "verified frame source lacks its originating identity",
    )
}

#[cfg(test)]
mod tests;

/// Workflow #375 R5d: a pass starts at most this often; the first one starts at the first
/// round after the start.
const SWEEP_INTERVAL_MS: u64 = 600_000;
/// A day, and the age at which a resource reading frame is due (the view's own, LN-7b).
const DAY_MS: u64 = 86_400_000;
const READING_DAYS: u64 = 7;

/// #422 LN-7c Q: what the cleaner reads from the configuration in effect, at each round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CleanerSettings {
    /// `frame_retention_evidence_max_age_days`, taken at each pass start.
    pub(super) evidence_max_age_days: u64,
    /// `ledger_observer.frame_query_steps_per_second`, read after each statement.
    pub(super) steps_per_second: u64,
    /// `ledger_observer.frame_query_chunk_frames` (not settable: the value of the start).
    pub(super) chunk_frames: usize,
}

/// #422 LN-7c Q: the clock of one round: the Runtime's (which dates the ledger's records) and a
/// monotonic one the pace and the pass interval count on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CleanerClock {
    pub(super) unix_ms: u64,
    pub(super) monotonic_ms: u64,
}

/// #422 LN-7c Q: where a pass reads its answers (`final-query.md` §1.2): the ledger's frame query
/// in the Runtime, a test's fake in the cleaner's tests.
pub(super) trait FrameAnswers {
    /// Statement W, once per pass.
    fn answer_waits(&mut self) -> RuntimeHostResult<FrameQueryOutcome<FrameWaits>>;
    /// Statement L for one chunk of live frames.
    fn answer_live(
        &mut self,
        frames: &[ArtifactId],
        waits: &FrameWaits,
        settings: &FrameQuerySettings,
    ) -> RuntimeHostResult<FrameQueryOutcome<Vec<LiveFrameAnswer>>>;
    /// Statement K for one chunk of kept frames.
    fn answer_kept(
        &mut self,
        frames: &[ArtifactId],
        settings: &FrameQuerySettings,
    ) -> RuntimeHostResult<FrameQueryOutcome<Vec<KeptFrameAnswer>>>;
    /// The frame's own authenticated `artifact.verified` record, before it is acted on.
    fn verified_frame(
        &mut self,
        artifact: ArtifactId,
    ) -> RuntimeHostResult<Option<FrameQueryFrame>>;
    /// The kept folder of a frame to move, as Lab output (`lab`) or as an error frame.
    fn kept_folder(
        &mut self,
        frame: &FrameQueryFrame,
        lab: bool,
    ) -> RuntimeHostResult<KeptFrameFolder>;
}

impl FrameAnswers for FrameQuery {
    fn answer_waits(&mut self) -> RuntimeHostResult<FrameQueryOutcome<FrameWaits>> {
        self.waits().map_err(ledger_failure)
    }

    fn answer_live(
        &mut self,
        frames: &[ArtifactId],
        waits: &FrameWaits,
        settings: &FrameQuerySettings,
    ) -> RuntimeHostResult<FrameQueryOutcome<Vec<LiveFrameAnswer>>> {
        self.live(frames, waits, settings).map_err(ledger_failure)
    }

    fn answer_kept(
        &mut self,
        frames: &[ArtifactId],
        settings: &FrameQuerySettings,
    ) -> RuntimeHostResult<FrameQueryOutcome<Vec<KeptFrameAnswer>>> {
        self.kept(frames, settings).map_err(ledger_failure)
    }

    fn verified_frame(
        &mut self,
        artifact: ArtifactId,
    ) -> RuntimeHostResult<Option<FrameQueryFrame>> {
        self.frame(artifact).map_err(ledger_failure)
    }

    fn kept_folder(
        &mut self,
        frame: &FrameQueryFrame,
        lab: bool,
    ) -> RuntimeHostResult<KeptFrameFolder> {
        self.folder(frame, lab).map_err(ledger_failure)
    }
}

/// Workflow #375 R5d, #422 LN-7c Q: the frame cleaner (`contracts/ledger-store.md`, "Frame
/// cleaner"). Each pass, at most every `SWEEP_INTERVAL_MS`, lists the live frame store and
/// `kept\`, asks the ledger (statements W, L and K on the query's own read-only connection,
/// chunked and paced) for the action of every live frame due for examination and of every kept
/// frame, and queues the frames to remove or move. Each round re-checks at most 16 queued frames
/// on a fresh snapshot and acts on that answer: it removes and moves live frames at their object
/// keys, and removes kept evidence frames that were confirmed or reached
/// `frame_retention_evidence_max_age_days`. It records nothing in the ledger and takes no lease
/// or queue position. No retention state is kept: the examination schedule below is this
/// process's only memory, and it never decides a class.
pub(super) struct FrameRetention {
    switches: FrameRetentionSwitches,
    moves: KeptMoves,
    /// Live frames this process removed, moved or found absent: never touched again.
    handled: BTreeSet<ArtifactId>,
    /// Live frames whose removal or move failed: warned once and left until a restart.
    failed: BTreeSet<ArtifactId>,
    /// Kept frames whose removal failed: warned once and left until a restart.
    kept_failed: BTreeSet<ArtifactId>,
    /// One Warning for a move counter that could not be written, per process.
    counter_warned: bool,
    /// #422 LN-7c Q (§1.4): when a live frame is examined again, by its last answer: `keep` at 1
    /// day and `keep_resource` at 7 days after its ingestion time; a frame without an entry is
    /// examined at every pass (not settled, running, busy, or new to this process).
    schedule: BTreeMap<ArtifactId, u64>,
    pass: Option<Pass>,
    /// The Runtime's own frame query while a pass runs.
    query: Option<FrameQuery>,
    last_pass_start_ms: Option<u64>,
    completed_passes: u64,
    origin: Instant,
}

/// One pass: what it still examines, what it queued, its pace and its counts.
struct Pass {
    started_ms: u64,
    waits: FrameWaits,
    settings: FrameQuerySettings,
    /// Live frames due for examination, not yet asked about.
    live: VecDeque<ArtifactId>,
    /// Kept frames not yet asked about, and every kept file of the listing.
    kept: VecDeque<ArtifactId>,
    kept_files: BTreeMap<ArtifactId, KeptFile>,
    queue: VecDeque<Queued>,
    /// The monotonic time the next statement may start at (§1.5).
    due_ms: u64,
    report: SweepReport,
}

/// A kept frame file as the kept listing found it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct KeptFile {
    date: String,
    leaf: String,
    file_name: String,
    byte_count: u64,
}

/// A frame the pass's answer acts on, re-checked by the round that takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Queued {
    Live(ArtifactId),
    Kept(ArtifactId),
}

/// How a pass began.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PassStart {
    Started,
    /// `ledger_frame_refs` does not cover the ledger: nothing is decided this pass.
    Skipped,
    /// The stop was raised.
    Stopped,
}

/// How a pass ended, when its line is printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PassEnd {
    /// `actingd frame_retention pass <report>`.
    Completed(SweepReport),
    /// `actingd frame_retention pass skipped=frame_refs_incomplete`: nothing was decided.
    Skipped,
}

/// What one statement of a round gave.
enum Step {
    Continue,
    Incomplete,
    Stopped,
}

/// The counts of one pass, printed as one developer line when the pass completes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct SweepReport {
    /// Frames the pass queued to remove or move (live and kept).
    pub(super) frames: u64,
    /// Files removed (live and kept) and their bytes.
    pub(super) deleted: u64,
    pub(super) deleted_bytes: u64,
    pub(super) moved: u64,
    pub(super) moved_bytes: u64,
    /// No file at the object key (or in `kept\`), new since this process started.
    pub(super) absent: u64,
    /// No file at the object key, found by the first pass after a start.
    pub(super) rescanned_absent: u64,
    pub(super) busy: u64,
    pub(super) failed: u64,
    /// Live error and Lab frames the pass's answers move.
    pub(super) kept_error: u64,
    pub(super) kept_lab: u64,
    /// Live frames answered `running`.
    pub(super) running: u64,
    pub(super) rounds: u64,
    pub(super) pass_ms: u64,
    /// #422 LN-7c Q: kept frames statement K examined.
    pub(super) kept_examined: u64,
    /// Evidence frames removed because an agent or a person confirmed reading them (live and
    /// kept).
    pub(super) removed_confirmed: u64,
    /// Kept evidence frames removed at `frame_retention_evidence_max_age_days`.
    pub(super) removed_expired: u64,
    /// Kept frame files whose artifact this ledger has no record of: left alone.
    pub(super) kept_unknown: u64,
    /// The SQLite virtual-machine steps of the pass's statements.
    pub(super) query_steps: u64,
}

impl std::fmt::Display for SweepReport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "frames={} deleted={} deleted_bytes={} moved={} moved_bytes={} absent={} \
             rescanned_absent={} busy={} failed={} kept_error={} kept_lab={} running={} \
             rounds={} pass_ms={} kept_examined={} removed_confirmed={} removed_expired={} \
             kept_unknown={} query_steps={}",
            self.frames,
            self.deleted,
            self.deleted_bytes,
            self.moved,
            self.moved_bytes,
            self.absent,
            self.rescanned_absent,
            self.busy,
            self.failed,
            self.kept_error,
            self.kept_lab,
            self.running,
            self.rounds,
            self.pass_ms,
            self.kept_examined,
            self.removed_confirmed,
            self.removed_expired,
            self.kept_unknown,
            self.query_steps
        )
    }
}

/// One failed removal or move: a Warning `runtime.failed` once per object per process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FrameRetentionWarning {
    pub(super) code: HostCode,
    /// The frame that could not be removed or moved; none for the move counter.
    pub(super) artifact_id: Option<ArtifactId>,
    /// The kept folder of a failed move or removal, relative to the state root.
    pub(super) entry: Option<String>,
    pub(super) io_kind: Option<std::io::ErrorKind>,
    pub(super) os_error: Option<i32>,
}

impl FrameRetentionWarning {
    fn new(
        code: HostCode,
        artifact_id: Option<ArtifactId>,
        entry: Option<String>,
        io_kind: Option<std::io::ErrorKind>,
        os_error: Option<i32>,
    ) -> Self {
        Self {
            code,
            artifact_id,
            entry,
            io_kind,
            os_error,
        }
    }

    fn stage(&self) -> actingcommand_contract::outcome::Location {
        match self.code {
            HostCode::FrameRetentionMoveFailed => HostLocation::FrameRetentionMove,
            HostCode::FrameRetentionCounterFailed => HostLocation::FrameRetentionCounter,
            HostCode::FrameRetentionLockRemoveFailed => HostLocation::FrameRetentionLockRemove,
            _ => HostLocation::FrameRetentionRemove,
        }
        .location()
    }

    /// The warning as its record's outcome (ledger revision 3; was a message of `key=value`
    /// tokens): its code with the frame, the kept folder and the OS error as values.
    fn outcome(&self) -> actingcommand_contract::outcome::Outcome {
        let mut outcome = actingcommand_contract::outcome::Outcome::new(self.code)
            .operation(HostLocation::MaintainFrameRetention);
        if let Some(artifact_id) = &self.artifact_id {
            outcome = outcome.artifact_id(crate::failure_identity::identifier_text(artifact_id));
        }
        if let Some(entry) = &self.entry {
            outcome = outcome.entry(entry);
        }
        if let Some(kind) = self.io_kind {
            outcome = outcome.io_kind(actingcommand_contract::outcome::IoKind::from_error_kind(
                kind,
            ));
        }
        if let Some(code) = self.os_error {
            outcome = outcome.os_error(code);
        }
        outcome
    }
}

/// `kept/<date>/<leaf>`, relative to the state root.
fn kept_entry(date: &str, leaf: &str) -> String {
    format!("{KEPT_DIRECTORY}/{date}/{leaf}")
}

/// The artifact id of a kept file's object file `artifact_<hex>.png`.
fn kept_artifact(object_file: &str) -> Option<ArtifactId> {
    let id = object_file.strip_suffix(".png")?;
    serde_json::from_value(serde_json::Value::String(id.to_owned())).ok()
}

/// The milliseconds `steps` take at `steps_per_second` (§1.5).
fn paced_ms(steps: u64, steps_per_second: u64) -> u64 {
    steps
        .saturating_mul(1_000)
        .div_ceil(steps_per_second.max(1))
}

impl Pass {
    /// Whether the next statement may start at `now_ms`.
    fn ready(&self, now_ms: u64) -> bool {
        now_ms >= self.due_ms
    }

    /// A statement that started at `started_ms` took `steps`: the next starts no earlier than
    /// that plus `steps` at the rate in effect (read after each statement).
    fn paced(&mut self, started_ms: u64, steps: u64, steps_per_second: u64) {
        self.report.query_steps = self.report.query_steps.saturating_add(steps);
        self.due_ms = started_ms.saturating_add(paced_ms(steps, steps_per_second));
    }

    fn examining(&self) -> bool {
        !self.live.is_empty() || !self.kept.is_empty()
    }
}

impl FrameRetention {
    pub(super) fn new(started_unix_ms: u64, switches: FrameRetentionSwitches) -> Self {
        Self {
            switches,
            moves: KeptMoves::new(started_unix_ms),
            handled: BTreeSet::new(),
            failed: BTreeSet::new(),
            kept_failed: BTreeSet::new(),
            counter_warned: false,
            schedule: BTreeMap::new(),
            pass: None,
            query: None,
            last_pass_start_ms: None,
            completed_passes: 0,
            origin: Instant::now(),
        }
    }

    /// The process's monotonic milliseconds since the cleaner was made.
    pub(super) fn monotonic_ms(&self) -> u64 {
        u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Called by the performance loop after its sampling locks are released. Starts a pass when
    /// none runs and the last one started at least `SWEEP_INTERVAL_MS` ago: it lists the live
    /// frame store (and hands the listing to the writer, which keeps the objects of those frames
    /// only, §2) and `kept\`, opens the pass's frame query on its own connection, whose statements
    /// `stop` interrupts, and asks statement W. Then it runs one round.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn maintain(
        &mut self,
        ledger: &GlobalLedger,
        root: &Path,
        clock: CleanerClock,
        settings: CleanerSettings,
        stop: FrameQueryStop,
        stopping: &dyn Fn() -> bool,
        warn: &mut dyn FnMut(&FrameRetentionWarning) -> RuntimeHostResult<()>,
    ) -> RuntimeHostResult<Option<PassEnd>> {
        ledger.check_writer_health().map_err(ledger_failure)?;
        if self.pass.is_none() {
            if self.last_pass_start_ms.is_some_and(|started| {
                clock.monotonic_ms.saturating_sub(started) < SWEEP_INTERVAL_MS
            }) {
                return Ok(None);
            }
            self.last_pass_start_ms = Some(clock.monotonic_ms);
            self.query = None;
            let live = actingcommand_ledger::live_frame_ids(root).map_err(|error| {
                RuntimeHostError::relayed_request(
                    actingcommand_ledger::LedgerCode::LedgerIo.as_str(),
                    actingcommand_ledger::LedgerLocation::QueryFrameRetention
                        .location()
                        .as_str(),
                    RuntimeErrorCode::RuntimeUnavailable,
                )
                .with_native_detail(error.to_string())
            })?;
            ledger
                .retain_live_frames(live.clone())
                .map_err(ledger_failure)?;
            let kept = list_kept_folders(root).map_err(RuntimeHostError::artifact_read)?;
            let mut query = ledger.frame_query(stop).map_err(ledger_failure)?;
            match self.begin_pass(&mut query, live, kept, clock, settings)? {
                PassStart::Started => self.query = Some(query),
                PassStart::Skipped => return Ok(Some(PassEnd::Skipped)),
                PassStart::Stopped => return Ok(None),
            }
        }
        let Some(mut query) = self.query.take() else {
            return Ok(None);
        };
        let ended = self.round(&mut query, root, clock, settings, stopping, warn);
        if self.pass.is_some() {
            self.query = Some(query);
        }
        ended
    }

    /// Begins a pass over the live frames `live` (the store's listing) and the kept listing
    /// `kept`: statement W, then the frames due for examination (§1.4) and every kept frame.
    pub(super) fn begin_pass(
        &mut self,
        answers: &mut dyn FrameAnswers,
        live: BTreeSet<ArtifactId>,
        kept: Vec<KeptLeafListing>,
        clock: CleanerClock,
        settings: CleanerSettings,
    ) -> RuntimeHostResult<PassStart> {
        self.pass = None;
        let (waits, steps) = match answers.answer_waits()? {
            FrameQueryOutcome::Answered { value, steps } => (value, steps),
            FrameQueryOutcome::Incomplete => return Ok(PassStart::Skipped),
            FrameQueryOutcome::Stopped => return Ok(PassStart::Stopped),
        };
        // A frame that left the store is forgotten.
        self.handled.retain(|id| live.contains(id));
        self.failed.retain(|id| live.contains(id));
        self.schedule.retain(|id, _| live.contains(id));
        let now = clock.unix_ms;
        let examined = live
            .iter()
            .filter(|id| {
                !self.handled.contains(*id)
                    && !self.failed.contains(*id)
                    && self.schedule.get(*id).is_none_or(|due| *due <= now)
            })
            .copied()
            .collect();
        let mut kept_files = BTreeMap::new();
        for leaf in kept {
            for file in leaf.files {
                if let Some(id) = kept_artifact(&file.object_file) {
                    kept_files.entry(id).or_insert(KeptFile {
                        date: leaf.date.clone(),
                        leaf: leaf.leaf.clone(),
                        file_name: file.file_name,
                        byte_count: file.byte_count,
                    });
                }
            }
        }
        self.kept_failed.retain(|id| kept_files.contains_key(id));
        let kept = kept_files
            .keys()
            .filter(|id| !self.kept_failed.contains(*id))
            .copied()
            .collect();
        let mut pass = Pass {
            started_ms: clock.monotonic_ms,
            waits,
            settings: FrameQuerySettings {
                now_unix_ms: now,
                switches: self.switches,
                evidence_max_age_ms: settings.evidence_max_age_days.saturating_mul(DAY_MS),
            },
            live: examined,
            kept,
            kept_files,
            queue: VecDeque::new(),
            due_ms: clock.monotonic_ms,
            report: SweepReport::default(),
        };
        pass.paced(clock.monotonic_ms, steps, settings.steps_per_second);
        self.pass = Some(pass);
        Ok(PassStart::Started)
    }

    /// One round: while the pace lets a statement start, at most one re-check of up to 16 queued
    /// frames, acted on by its answer, and the examination of the next chunks. Returns how the
    /// pass ended, when it did and its line is printed (an idle pass after the first prints
    /// nothing). A failed statement ends the pass: the next one starts after the interval.
    pub(super) fn round(
        &mut self,
        answers: &mut dyn FrameAnswers,
        root: &Path,
        clock: CleanerClock,
        settings: CleanerSettings,
        stopping: &dyn Fn() -> bool,
        warn: &mut dyn FnMut(&FrameRetentionWarning) -> RuntimeHostResult<()>,
    ) -> RuntimeHostResult<Option<PassEnd>> {
        let Some(mut pass) = self.pass.take() else {
            return Ok(None);
        };
        pass.report.rounds += 1;
        pass.settings.now_unix_ms = clock.unix_ms;
        let mut acted = false;
        loop {
            if stopping() || !pass.ready(clock.monotonic_ms) {
                break;
            }
            let step = if !pass.queue.is_empty() {
                if acted {
                    break;
                }
                acted = true;
                self.act(&mut pass, answers, root, clock, settings, stopping, warn)?
            } else if pass.examining() {
                self.examine(&mut pass, answers, clock, settings)?
            } else {
                break;
            };
            match step {
                Step::Continue => {}
                Step::Incomplete => return Ok(Some(PassEnd::Skipped)),
                Step::Stopped => return Ok(None),
            }
        }
        if !pass.queue.is_empty() || pass.examining() {
            self.pass = Some(pass);
            return Ok(None);
        }
        let mut report = pass.report;
        report.pass_ms = clock.monotonic_ms.saturating_sub(pass.started_ms);
        self.completed_passes += 1;
        Ok((self.completed_passes == 1 || report.frames > 0).then_some(PassEnd::Completed(report)))
    }

    /// The next chunk of live frames (statement L), else of kept frames (statement K); queues the
    /// frames to act on and schedules the others (§1.4).
    fn examine(
        &mut self,
        pass: &mut Pass,
        answers: &mut dyn FrameAnswers,
        clock: CleanerClock,
        settings: CleanerSettings,
    ) -> RuntimeHostResult<Step> {
        let chunk = settings.chunk_frames.max(1);
        if !pass.live.is_empty() {
            let frames = pass
                .live
                .drain(..chunk.min(pass.live.len()))
                .collect::<Vec<_>>();
            let (answered, steps) =
                match answers.answer_live(&frames, &pass.waits, &pass.settings)? {
                    FrameQueryOutcome::Answered { value, steps } => (value, steps),
                    FrameQueryOutcome::Incomplete => return Ok(Step::Incomplete),
                    FrameQueryOutcome::Stopped => return Ok(Step::Stopped),
                };
            pass.paced(clock.monotonic_ms, steps, settings.steps_per_second);
            for answer in answered {
                self.decide_live(pass, &answer);
            }
            return Ok(Step::Continue);
        }
        let frames = pass
            .kept
            .drain(..chunk.min(pass.kept.len()))
            .collect::<Vec<_>>();
        let (answered, steps) = match answers.answer_kept(&frames, &pass.settings)? {
            FrameQueryOutcome::Answered { value, steps } => (value, steps),
            FrameQueryOutcome::Incomplete => return Ok(Step::Incomplete),
            FrameQueryOutcome::Stopped => return Ok(Step::Stopped),
        };
        pass.paced(clock.monotonic_ms, steps, settings.steps_per_second);
        pass.report.kept_examined += frames.len() as u64;
        let known = answered
            .iter()
            .map(|answer| answer.artifact_id)
            .collect::<BTreeSet<_>>();
        pass.report.kept_unknown += frames.iter().filter(|id| !known.contains(*id)).count() as u64;
        for answer in answered {
            if matches!(
                answer.action,
                FrameQueryAction::RemoveConfirmed | FrameQueryAction::RemoveExpired
            ) {
                pass.report.frames += 1;
                pass.queue.push_back(Queued::Kept(answer.artifact_id));
            }
        }
        Ok(Step::Continue)
    }

    /// A live frame's answer at its examination: queued when it is acted on, else examined again
    /// at its next decision age (`keep` at 1 day, `keep_resource` at 7 days) or at the next pass.
    fn decide_live(&mut self, pass: &mut Pass, answer: &LiveFrameAnswer) {
        let id = answer.artifact_id;
        let at = answer.created_at_unix_ms;
        match answer.action {
            FrameQueryAction::Running => {
                pass.report.running += 1;
                self.schedule.remove(&id);
            }
            FrameQueryAction::Keep => {
                self.schedule.insert(id, at.saturating_add(DAY_MS));
            }
            FrameQueryAction::KeepResource => {
                self.schedule
                    .insert(id, at.saturating_add(READING_DAYS * DAY_MS));
            }
            FrameQueryAction::RemoveExpired => {
                self.schedule.remove(&id);
            }
            action => {
                match action {
                    FrameQueryAction::MoveLab => pass.report.kept_lab += 1,
                    FrameQueryAction::MoveError => pass.report.kept_error += 1,
                    _ => {}
                }
                self.schedule.remove(&id);
                pass.report.frames += 1;
                pass.queue.push_back(Queued::Live(id));
            }
        }
    }

    /// At most 16 queued frames within 1 s, each re-checked on a fresh snapshot (statements L
    /// and K for those frames only, with the pass's W) and acted on by that answer (§1.2, "Never
    /// a stale class"). A frame that is absent costs a stat and no action.
    #[allow(clippy::too_many_arguments)]
    fn act(
        &mut self,
        pass: &mut Pass,
        answers: &mut dyn FrameAnswers,
        root: &Path,
        clock: CleanerClock,
        settings: CleanerSettings,
        stopping: &dyn Fn() -> bool,
        warn: &mut dyn FnMut(&FrameRetentionWarning) -> RuntimeHostResult<()>,
    ) -> RuntimeHostResult<Step> {
        let taken = pass
            .queue
            .drain(..RETENTION_ROUND_OBJECTS.min(pass.queue.len()))
            .collect::<Vec<_>>();
        let live = taken
            .iter()
            .filter_map(|queued| match queued {
                Queued::Live(id) => Some(*id),
                Queued::Kept(_) => None,
            })
            .collect::<Vec<_>>();
        let kept = taken
            .iter()
            .filter_map(|queued| match queued {
                Queued::Kept(id) => Some(*id),
                Queued::Live(_) => None,
            })
            .collect::<Vec<_>>();
        let mut steps = 0_u64;
        let mut live_answers = BTreeMap::new();
        if !live.is_empty() {
            match answers.answer_live(&live, &pass.waits, &pass.settings)? {
                FrameQueryOutcome::Answered { value, steps: used } => {
                    steps = steps.saturating_add(used);
                    live_answers
                        .extend(value.into_iter().map(|answer| (answer.artifact_id, answer)));
                }
                FrameQueryOutcome::Incomplete => return Ok(Step::Incomplete),
                FrameQueryOutcome::Stopped => return Ok(Step::Stopped),
            }
        }
        let mut kept_answers = BTreeMap::new();
        if !kept.is_empty() {
            match answers.answer_kept(&kept, &pass.settings)? {
                FrameQueryOutcome::Answered { value, steps: used } => {
                    steps = steps.saturating_add(used);
                    kept_answers
                        .extend(value.into_iter().map(|answer| (answer.artifact_id, answer)));
                }
                FrameQueryOutcome::Incomplete => return Ok(Step::Incomplete),
                FrameQueryOutcome::Stopped => return Ok(Step::Stopped),
            }
        }
        pass.paced(clock.monotonic_ms, steps, settings.steps_per_second);
        let started = Instant::now();
        let budget = Duration::from_millis(RETENTION_ROUND_START_BUDGET_MS);
        let mut left = taken.into_iter();
        for queued in left.by_ref() {
            // A frame not answered now (its pin or record changed) is examined at the next pass.
            match queued {
                Queued::Live(id) => {
                    if let Some(answer) = live_answers.get(&id) {
                        self.act_live(pass, answers, root, answer, warn)?;
                    }
                }
                Queued::Kept(id) => {
                    if let Some(answer) = kept_answers.get(&id) {
                        self.act_kept(pass, root, answer, warn)?;
                    }
                }
            }
            if stopping() || started.elapsed() >= budget {
                break;
            }
        }
        // What the round did not reach goes back to the front of the queue, re-checked again.
        for queued in left.rev() {
            pass.queue.push_front(queued);
        }
        Ok(Step::Continue)
    }

    /// Acts on a live frame by the round's answer: removes it or moves it into its kept folder,
    /// at its object key only; a frame now running or kept is examined again by its schedule.
    fn act_live(
        &mut self,
        pass: &mut Pass,
        answers: &mut dyn FrameAnswers,
        root: &Path,
        answer: &LiveFrameAnswer,
        warn: &mut dyn FnMut(&FrameRetentionWarning) -> RuntimeHostResult<()>,
    ) -> RuntimeHostResult<()> {
        let id = answer.artifact_id;
        let action = answer.action;
        if !action.removes() && !action.moves() {
            self.decide_live_again(answer);
            return Ok(());
        }
        // The frame's own verified record, authenticated, names its object.
        let Some(frame) = answers.verified_frame(id)? else {
            return Ok(());
        };
        let folder = match action {
            FrameQueryAction::MoveLab => Some(answers.kept_folder(&frame, true)?),
            FrameQueryAction::MoveError => Some(answers.kept_folder(&frame, false)?),
            _ => None,
        };
        let reference = &frame.reference;
        let bytes = reference.byte_count;
        let result = match &folder {
            None => remove_frame_file(root, reference),
            Some(folder) => move_frame_file(
                root,
                reference,
                &folder.date,
                &folder.leaf,
                &folder.file_name,
            ),
        };
        // Workflow #411 A29: a removed or moved frame whose use lock could not be deleted.
        let lock_left = result.as_ref().ok().and_then(|outcome| outcome.lock_left());
        let report = &mut pass.report;
        match result {
            Ok(FrameFileOutcome::Absent) => {
                if self.completed_passes == 0 {
                    report.rescanned_absent += 1;
                } else {
                    report.absent += 1;
                }
                self.handled.insert(id);
            }
            Ok(FrameFileOutcome::Removed | FrameFileOutcome::RemovedLockLeft(_)) => {
                report.deleted += 1;
                report.deleted_bytes = report.deleted_bytes.saturating_add(bytes);
                if action == FrameQueryAction::RemoveConfirmed {
                    report.removed_confirmed += 1;
                }
                self.handled.insert(id);
            }
            Ok(FrameFileOutcome::Moved | FrameFileOutcome::MovedLockLeft(_)) => {
                report.moved += 1;
                report.moved_bytes = report.moved_bytes.saturating_add(bytes);
                self.handled.insert(id);
                if let Err(error) = self.moves.record_move(root)
                    && !self.counter_warned
                {
                    // The frame moved; only the counter that tells other processes to look
                    // again is behind, until the next move writes it.
                    self.counter_warned = true;
                    warn(&FrameRetentionWarning::new(
                        HostCode::FrameRetentionCounterFailed,
                        None,
                        Some(format!("{KEPT_DIRECTORY}/.moves")),
                        Some(error.kind()),
                        error.raw_os_error(),
                    ))?;
                }
            }
            Ok(FrameFileOutcome::Busy) => {
                report.busy += 1;
            }
            Err(error) => {
                report.failed += 1;
                self.failed.insert(id);
                let (code, entry) = match &folder {
                    None => (HostCode::FrameRetentionRemoveFailed, None),
                    Some(folder) => (
                        HostCode::FrameRetentionMoveFailed,
                        Some(kept_entry(&folder.date, &folder.leaf)),
                    ),
                };
                warn(&FrameRetentionWarning::new(
                    code,
                    Some(id),
                    entry,
                    error.io_error_kind(),
                    error.raw_os_error(),
                ))?;
            }
        }
        // The frame counts as removed or moved; only its lock stays, reported once (the frame
        // is handled, so it is never visited again by this process).
        if let Some(left) = lock_left {
            warn(&FrameRetentionWarning::new(
                HostCode::FrameRetentionLockRemoveFailed,
                Some(id),
                None,
                Some(left.io_kind),
                left.os_error,
            ))?;
        }
        if self.handled.contains(&id) || self.failed.contains(&id) {
            self.schedule.remove(&id);
        }
        Ok(())
    }

    /// A queued live frame the round's answer no longer acts on: scheduled by that answer.
    fn decide_live_again(&mut self, answer: &LiveFrameAnswer) {
        let id = answer.artifact_id;
        match answer.action {
            FrameQueryAction::Keep => {
                self.schedule
                    .insert(id, answer.created_at_unix_ms.saturating_add(DAY_MS));
            }
            FrameQueryAction::KeepResource => {
                self.schedule.insert(
                    id,
                    answer
                        .created_at_unix_ms
                        .saturating_add(READING_DAYS * DAY_MS),
                );
            }
            _ => {
                self.schedule.remove(&id);
            }
        }
    }

    /// Removes a kept evidence frame the round's answer still removes (confirmed or expired),
    /// with its empty leaf and date folder; a kept frame now kept stays.
    fn act_kept(
        &mut self,
        pass: &mut Pass,
        root: &Path,
        answer: &KeptFrameAnswer,
        warn: &mut dyn FnMut(&FrameRetentionWarning) -> RuntimeHostResult<()>,
    ) -> RuntimeHostResult<()> {
        let id = answer.artifact_id;
        if !matches!(
            answer.action,
            FrameQueryAction::RemoveConfirmed | FrameQueryAction::RemoveExpired
        ) {
            return Ok(());
        }
        let Some(file) = pass.kept_files.get(&id).cloned() else {
            return Ok(());
        };
        let entry = kept_entry(&file.date, &file.leaf);
        let report = &mut pass.report;
        let removal = match remove_kept_frame_file(root, &file.date, &file.leaf, &file.file_name) {
            Ok(removal) => removal,
            Err(error) => {
                report.failed += 1;
                self.kept_failed.insert(id);
                return warn(&FrameRetentionWarning::new(
                    HostCode::FrameRetentionRemoveFailed,
                    Some(id),
                    Some(entry),
                    error.io_error_kind(),
                    error.raw_os_error(),
                ));
            }
        };
        match removal.outcome {
            FrameFileOutcome::Removed | FrameFileOutcome::RemovedLockLeft(_) => {
                report.deleted += 1;
                report.deleted_bytes = report.deleted_bytes.saturating_add(file.byte_count);
                if answer.action == FrameQueryAction::RemoveConfirmed {
                    report.removed_confirmed += 1;
                } else {
                    report.removed_expired += 1;
                }
            }
            FrameFileOutcome::Absent => report.absent += 1,
            FrameFileOutcome::Busy => report.busy += 1,
            FrameFileOutcome::Moved | FrameFileOutcome::MovedLockLeft(_) => {}
        }
        if let Some(left) = removal.outcome.lock_left() {
            warn(&FrameRetentionWarning::new(
                HostCode::FrameRetentionLockRemoveFailed,
                Some(id),
                Some(entry),
                Some(left.io_kind),
                left.os_error,
            ))?;
        }
        if let Some((folder, left)) = removal.folder_left {
            warn(&FrameRetentionWarning::new(
                HostCode::FrameRetentionRemoveFailed,
                Some(id),
                Some(folder),
                Some(left.io_kind),
                left.os_error,
            ))?;
        }
        Ok(())
    }

    /// Startup recovery of eviction intents left without their outcome (a v0.11.4 Runtime's
    /// hard stop between an intent and its unlink). It completes before provider assembly or
    /// normal work admission, and admits no new intent.
    pub(super) fn recover(
        ledger: &GlobalLedger,
        artifacts: &ArtifactStore,
        deadline: Instant,
    ) -> RuntimeHostResult<()> {
        let mut recovery = EvictionRecovery::default();
        loop {
            if Instant::now() >= deadline {
                return Err(failure(HostCode::ArtifactEvictionRecoveryDeadline));
            }
            // Each round starts from the remaining original intents.
            recovery.after = None;
            let round = recovery.round(ledger, artifacts, &|| Instant::now() >= deadline)?;
            if !round.recovering {
                return Ok(());
            }
            if round.completed == 0 {
                return Err(failure(HostCode::ArtifactEvictionRecoveryDeferred));
            }
        }
    }
}

/// The scan cursor of startup recovery; eligibility belongs to the GlobalLedger.
#[derive(Default)]
struct EvictionRecovery {
    after: Option<ArtifactId>,
}

struct RecoveryRound {
    recovering: bool,
    completed: usize,
}

impl EvictionRecovery {
    fn round(
        &mut self,
        ledger: &GlobalLedger,
        artifacts: &ArtifactStore,
        stopping: &impl Fn() -> bool,
    ) -> RuntimeHostResult<RecoveryRound> {
        ledger.check_writer_health().map_err(ledger_failure)?;
        let started = Instant::now();
        let candidates = ledger
            .retention_candidates(self.after)
            .map_err(ledger_failure)?;
        if candidates.references.len() > RETENTION_ROUND_OBJECTS {
            return Err(failure(HostCode::ArtifactRetentionCandidateBoundExceeded));
        }
        let mut round = RecoveryRound {
            recovering: candidates.recovery_pending,
            completed: 0,
        };
        if !round.recovering {
            return Ok(round);
        }
        let mut removed_bytes = 0_u64;
        for reference in &candidates.references {
            if stopping()
                || started.elapsed() >= Duration::from_millis(RETENTION_ROUND_START_BUDGET_MS)
            {
                return Ok(round);
            }
            self.after = Some(reference.artifact_id);
            if candidates.ineligible.contains(&reference.artifact_id) {
                continue;
            }
            if reference.byte_count > RETENTION_ROUND_BYTES.saturating_sub(removed_bytes) {
                continue;
            }
            // The writer never waits for this cross-process, try-only material guard.
            let Some(guard) = try_artifact_delete_guard(artifacts.root(), reference)
                .map_err(RuntimeHostError::artifact)?
            else {
                continue;
            };
            if stopping() {
                return Ok(round);
            }
            match ledger
                .admit_artifact_eviction(guard)
                .map_err(ledger_failure)?
            {
                ArtifactEvictionAdmission::Deferred => {}
                ArtifactEvictionAdmission::Committed(permit) => {
                    // Once intent is durable, shutdown waits for this outcome or its explicit error.
                    ledger
                        .finish_artifact_eviction(permit)
                        .map_err(ledger_failure)?;
                    removed_bytes = removed_bytes
                        .checked_add(reference.byte_count)
                        .ok_or_else(|| failure(HostCode::ArtifactRetentionByteCountOverflow))?;
                    round.completed += 1;
                }
            }
        }
        self.after = candidates.next_after;
        Ok(round)
    }
}

fn failure(code: impl Into<Code>) -> RuntimeHostError {
    RuntimeHostError::fatal(
        code,
        HostLocation::MaintainFrameRetention,
        RuntimeErrorCode::LedgerFailure,
    )
}

/// A ledger failure of the cleaner. Workflow #383 AO-1 (§9.5): a read the writer did not answer
/// in time is an error of the round (recorded, not fatal); every other ledger failure stays
/// fatal.
fn ledger_failure(error: GlobalLedgerError) -> RuntimeHostError {
    let detail = format!("{error}; detail={:?}", error.detail());
    let relayed = if error.is_fatal() {
        RuntimeHostError::relayed_fatal(
            error.code(),
            error.operation(),
            RuntimeErrorCode::LedgerFailure,
        )
    } else {
        RuntimeHostError::relayed_request(
            error.code(),
            error.operation(),
            RuntimeErrorCode::RuntimeUnavailable,
        )
    };
    relayed.with_native_detail(detail)
}
