// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_artifact_store::{
    ArtifactStore, ArtifactStoreError, ArtifactStoreResult, FrameFileOutcome, KEPT_DIRECTORY,
    KeptMoves, move_frame_file, remove_frame_file, try_artifact_delete_guard,
};
use actingcommand_contract::{
    ArtifactId, ArtifactKind, ArtifactPayloadDraft, ArtifactPinReason, ArtifactPinRecord,
    ArtifactRetentionFact, ArtifactRetentionIdentity, AuditInput, DiagnosticCode,
    DiagnosticDetailDraft, EffectDisposition, EventActor, EventLinksDraft, EventSeverity,
    EventSource, EventType, FRAME_RETENTION_POLICY_VERSION, FailedRunRetentionPolicy, OriginModule,
    OwnerEpoch, ProjectedArtifactReference, RETENTION_ROUND_BYTES, RETENTION_ROUND_OBJECTS,
    RETENTION_ROUND_START_BUDGET_MS, RuntimeErrorCode, RuntimePayloadDraft, Sensitivity,
    TerminalEvent,
};
use actingcommand_ledger::{
    ArtifactEvictionAdmission, FrameRetentionClass, FrameRetentionFrame, FrameRetentionSwitches,
    FrameRetentionView, GlobalLedger, GlobalLedgerError, KeptFrameFolder, PersistedEvent,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::codes::HostCode;
use crate::{RuntimeHostError, RuntimeHostResult, events::RuntimeEvents};

pub(super) fn capture_frame_store_config() -> actingcommand_artifact_store::FrameStoreConfig {
    actingcommand_artifact_store::FrameStoreConfig::default().with_memory_source(
        actingcommand_artifact_store::MemorySampleSource::live(|| {
            let sample = actingcommand_host_metrics::sample_physical_memory()
                .map_err(|code| ArtifactStoreError::fatal(code, "sample_capture_memory", code))?;
            Ok(actingcommand_artifact_store::MemorySample {
                total_bytes: sample.total_bytes,
                available_bytes: sample.available_bytes,
            })
        }),
    )
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
            "open_capture_pipeline",
            error.to_string(),
        )
    })?;
    Ok(root
        .join("frame-spills")
        .join(format!("{:x}", Sha256::digest(canonical))))
}

impl super::HostShared {
    /// Workflow #375 R5d: one cleaner round. Its errors are ledger errors only, which are
    /// fatal; a frame that cannot be removed or moved is a Warning.
    pub(super) fn maintain_frame_retention(&self) -> RuntimeHostResult<bool> {
        let result = (|| {
            // Try-only, so a `clear-kept` that holds the lock makes the round skip.
            let mut retention = match self.frame_retention.try_lock() {
                Ok(retention) => retention,
                Err(std::sync::TryLockError::WouldBlock) => return Ok(true),
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    return Err(super::lock_poison_error("maintain_frame_retention"));
                }
            };
            let Some(retention) = retention.as_mut() else {
                return Ok(false);
            };
            // The Runtime's clock, which also dates the ledger's events.
            let now_unix_ms = self.clock.sample()?.unix_ms;
            let report = retention.maintain(
                &self.ledger,
                self.artifacts.root(),
                now_unix_ms,
                &|| self.fatal.is_shutdown_requested(),
                &mut |warning| self.record_frame_retention_warning(warning),
            )?;
            if let Some(report) = report {
                println!("actingd frame_retention pass {report}");
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
                self.fatal.mark(error.clone())?;
                Err(error)
            }
        }
    }

    /// One Warning `runtime.failed` with system links for a frame the cleaner could not remove
    /// or move; its detail carries only `key=value` tokens.
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
                DiagnosticDetailDraft::new(
                    "frame_retention",
                    warning.stage(),
                    "artifact_store",
                    "maintain_frame_retention",
                    warning.message(),
                    Sensitivity::Internal,
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
        return Err(pin_failure("artifact_pin_source_not_verified"));
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
                .ok_or_else(|| pin_failure("artifact_pin_instance_missing"))?,
            request_id: *links
                .request_id()
                .ok_or_else(|| pin_failure("artifact_pin_request_missing"))?,
            correlation_id: *links
                .correlation_id()
                .ok_or_else(|| pin_failure("artifact_pin_correlation_missing"))?,
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

/// Workflow #375 R5d: a sweep starts at most this often; the first one starts at the first
/// round after the start.
const SWEEP_INTERVAL: Duration = Duration::from_secs(600);

/// Workflow #375 R5d: the frame cleaner (`contracts/ledger-store.md`, "Frame cleaner"). Each
/// round of the performance loop acts on at most 16 settled frames within 1 s; a sweep is one
/// pass over every settled frame that is due and not yet handled, over as many rounds as it
/// needs. It removes and moves frames at their object keys only, never in `kept\`, records
/// nothing in the ledger, and takes no lease or queue position. Only a ledger error is fatal.
pub(super) struct FrameRetention {
    switches: FrameRetentionSwitches,
    moves: KeptMoves,
    /// Frames this process removed, moved or found absent: never touched again.
    handled: BTreeSet<ArtifactId>,
    /// Frames whose removal or move failed: warned once and left until a restart.
    failed: BTreeSet<ArtifactId>,
    /// One Warning for a move counter that could not be written, per process.
    counter_warned: bool,
    sweep: Option<Sweep>,
    last_sweep_start: Option<Instant>,
    completed_sweeps: u64,
}

struct Sweep {
    started: Instant,
    actions: VecDeque<FrameAction>,
    report: SweepReport,
}

struct FrameAction {
    reference: ProjectedArtifactReference,
    /// The kept folder of a frame to move; `None` for a frame to remove.
    folder: Option<KeptFrameFolder>,
}

/// The counts of one sweep, printed as one developer line when the sweep completes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct SweepReport {
    /// Settled frames that were due, not yet handled, when the sweep started.
    pub(super) frames: u64,
    pub(super) deleted: u64,
    pub(super) deleted_bytes: u64,
    pub(super) moved: u64,
    pub(super) moved_bytes: u64,
    /// No file at the object key, new since this process started.
    pub(super) absent: u64,
    /// No file at the object key, found by the first sweep after a start.
    pub(super) rescanned_absent: u64,
    pub(super) busy: u64,
    pub(super) failed: u64,
    /// Settled error and Lab frames in the view the sweep started from.
    pub(super) kept_error: u64,
    pub(super) kept_lab: u64,
    /// Frames not yet settled in that view.
    pub(super) running: u64,
    pub(super) rounds: u64,
    pub(super) pass_ms: u64,
}

impl std::fmt::Display for SweepReport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "frames={} deleted={} deleted_bytes={} moved={} moved_bytes={} absent={} \
             rescanned_absent={} busy={} failed={} kept_error={} kept_lab={} running={} \
             rounds={} pass_ms={}",
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
            self.pass_ms
        )
    }
}

/// One failed removal or move: a Warning `runtime.failed` once per object per process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FrameRetentionWarning {
    pub(super) code: HostCode,
    /// The frame that could not be removed or moved; none for the move counter.
    pub(super) artifact_id: Option<ArtifactId>,
    /// The kept folder of a failed move, relative to the state root.
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

    fn stage(&self) -> &'static str {
        match self.code {
            HostCode::FrameRetentionMoveFailed => "frame_retention.move",
            HostCode::FrameRetentionCounterFailed => "frame_retention.counter",
            _ => "frame_retention.remove",
        }
    }

    /// Only `key=value` tokens.
    fn message(&self) -> String {
        let mut message = format!("host_code={}", self.code.as_str());
        if let Some(artifact_id) = &self.artifact_id {
            message.push_str(&format!(
                " artifact_id={}",
                crate::failure_identity::identifier_text(artifact_id)
            ));
        }
        if let Some(entry) = &self.entry {
            message.push_str(&format!(" entry={entry}"));
        }
        if let Some(kind) = self.io_kind {
            message.push_str(&format!(
                " io_kind={}",
                actingcommand_contract::outcome::IoKind::from_error_kind(kind).as_str()
            ));
        }
        if let Some(code) = self.os_error {
            message.push_str(&format!(" os_error={code}"));
        }
        message
    }
}

/// The action a settled frame is due for at `now`: `Some(Some(folder))` moves a Lab or error
/// frame, `Some(None)` removes a duplicate, default or resource frame whose due time has come,
/// and `None` leaves the frame alone.
fn due_action(frame: &FrameRetentionFrame, now: u64) -> Option<Option<KeptFrameFolder>> {
    match frame.class {
        FrameRetentionClass::Lab | FrameRetentionClass::Error => frame.folder.clone().map(Some),
        FrameRetentionClass::Resource
        | FrameRetentionClass::Duplicate
        | FrameRetentionClass::Default => frame
            .due_unix_ms
            .is_some_and(|due| due <= now)
            .then_some(None),
        FrameRetentionClass::Running => None,
    }
}

/// `kept/<date>/<leaf>`, relative to the state root.
fn kept_entry(folder: &KeptFrameFolder) -> String {
    format!("{KEPT_DIRECTORY}/{}/{}", folder.date, folder.leaf)
}

impl FrameRetention {
    pub(super) fn new(started_unix_ms: u64, switches: FrameRetentionSwitches) -> Self {
        Self {
            switches,
            moves: KeptMoves::new(started_unix_ms),
            handled: BTreeSet::new(),
            failed: BTreeSet::new(),
            counter_warned: false,
            sweep: None,
            last_sweep_start: None,
            completed_sweeps: 0,
        }
    }

    /// Called by the performance loop after its sampling locks are released. Starts a sweep
    /// from the ledger's frame view when none runs and the last one started at least
    /// `SWEEP_INTERVAL` ago (model v4.3 note); while a sweep still has queued actions, each
    /// round re-reads the view, which the writer caches by head, and derives every queued action
    /// again, so no frame is acted on by a class it no longer has. Then it runs one round.
    /// Returns the report of a sweep it completed.
    pub(super) fn maintain(
        &mut self,
        ledger: &GlobalLedger,
        root: &Path,
        now_unix_ms: u64,
        stopping: &dyn Fn() -> bool,
        warn: &mut dyn FnMut(&FrameRetentionWarning) -> RuntimeHostResult<()>,
    ) -> RuntimeHostResult<Option<SweepReport>> {
        ledger.check_writer_health().map_err(ledger_failure)?;
        if self.sweep.is_none() {
            if self
                .last_sweep_start
                .is_some_and(|started| started.elapsed() < SWEEP_INTERVAL)
            {
                return Ok(None);
            }
            let view = ledger
                .frame_retention_view(now_unix_ms, self.switches)
                .map_err(ledger_failure)?;
            self.begin_sweep(&view);
        } else {
            let view = ledger
                .frame_retention_view(now_unix_ms, self.switches)
                .map_err(ledger_failure)?;
            self.rederive(&view);
        }
        self.round(root, stopping, warn)
    }

    /// Derives each queued action again from a newer view: a frame now kept (Lab or error) is
    /// moved to its current folder, a frame still due is removed, and any other frame (not due,
    /// not settled, or gone from the view) leaves the sweep untouched.
    fn rederive(&mut self, view: &FrameRetentionView) {
        let Some(sweep) = self.sweep.as_mut() else {
            return;
        };
        let now = view.evaluated_at_unix_ms;
        let current = view
            .frames
            .iter()
            .map(|frame| (frame.reference.artifact_id, frame))
            .collect::<BTreeMap<_, _>>();
        sweep.actions.retain_mut(|action| {
            match current
                .get(&action.reference.artifact_id)
                .and_then(|frame| due_action(frame, now))
            {
                Some(folder) => {
                    action.folder = folder;
                    true
                }
                None => false,
            }
        });
    }

    /// Lists the frames the view says are due, in settle order (entry time, then artifact id).
    fn begin_sweep(&mut self, view: &FrameRetentionView) {
        let now = view.evaluated_at_unix_ms;
        let mut report = SweepReport::default();
        let mut due = Vec::new();
        for frame in &view.frames {
            match frame.class {
                FrameRetentionClass::Running => {
                    report.running += 1;
                    continue;
                }
                FrameRetentionClass::Error => report.kept_error += 1,
                FrameRetentionClass::Lab => report.kept_lab += 1,
                FrameRetentionClass::Resource
                | FrameRetentionClass::Duplicate
                | FrameRetentionClass::Default => {}
            }
            let id = frame.reference.artifact_id;
            if self.handled.contains(&id) || self.failed.contains(&id) {
                continue;
            }
            let Some(folder) = due_action(frame, now) else {
                continue;
            };
            due.push((
                frame.entry_unix_ms.unwrap_or_default(),
                FrameAction {
                    reference: frame.reference.clone(),
                    folder,
                },
            ));
        }
        // The view lists frames in artifact id order; a stable sort keeps it within an entry.
        due.sort_by_key(|(entry, _)| *entry);
        report.frames = due.len() as u64;
        let started = Instant::now();
        self.last_sweep_start = Some(started);
        self.sweep = Some(Sweep {
            started,
            actions: due.into_iter().map(|(_, action)| action).collect(),
            report,
        });
    }

    /// At most 16 actions within 1 s. A frame that is absent costs a stat and no action.
    fn round(
        &mut self,
        root: &Path,
        stopping: &dyn Fn() -> bool,
        warn: &mut dyn FnMut(&FrameRetentionWarning) -> RuntimeHostResult<()>,
    ) -> RuntimeHostResult<Option<SweepReport>> {
        let Some(sweep) = self.sweep.as_mut() else {
            return Ok(None);
        };
        sweep.report.rounds += 1;
        let started = Instant::now();
        let budget = Duration::from_millis(RETENTION_ROUND_START_BUDGET_MS);
        let mut actions = 0;
        while actions < RETENTION_ROUND_OBJECTS && started.elapsed() < budget && !stopping() {
            let Some(action) = sweep.actions.pop_front() else {
                break;
            };
            let id = action.reference.artifact_id;
            let bytes = action.reference.byte_count;
            let report = &mut sweep.report;
            let result = match &action.folder {
                None => remove_frame_file(root, &action.reference),
                Some(folder) => move_frame_file(
                    root,
                    &action.reference,
                    &folder.date,
                    &folder.leaf,
                    &folder.file_name,
                ),
            };
            match result {
                Ok(FrameFileOutcome::Absent) => {
                    if self.completed_sweeps == 0 {
                        report.rescanned_absent += 1;
                    } else {
                        report.absent += 1;
                    }
                    self.handled.insert(id);
                }
                Ok(FrameFileOutcome::Removed) => {
                    actions += 1;
                    report.deleted += 1;
                    report.deleted_bytes = report.deleted_bytes.saturating_add(bytes);
                    self.handled.insert(id);
                }
                Ok(FrameFileOutcome::Moved) => {
                    actions += 1;
                    report.moved += 1;
                    report.moved_bytes = report.moved_bytes.saturating_add(bytes);
                    self.handled.insert(id);
                    if let Err(error) = self.moves.record_move(root)
                        && !self.counter_warned
                    {
                        // The frame moved; only the counter that tells other processes
                        // to look again is behind, until the next move writes it.
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
                    actions += 1;
                    report.busy += 1;
                }
                Err(error) => {
                    actions += 1;
                    report.failed += 1;
                    self.failed.insert(id);
                    let (code, entry) = match &action.folder {
                        None => (HostCode::FrameRetentionRemoveFailed, None),
                        Some(folder) => {
                            (HostCode::FrameRetentionMoveFailed, Some(kept_entry(folder)))
                        }
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
        }
        if !sweep.actions.is_empty() {
            return Ok(None);
        }
        let Some(sweep) = self.sweep.take() else {
            return Ok(None);
        };
        let mut report = sweep.report;
        report.pass_ms = u64::try_from(sweep.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.completed_sweeps += 1;
        // An idle sweep after the first prints nothing.
        Ok((self.completed_sweeps == 1 || report.frames > 0).then_some(report))
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
                return Err(failure("artifact_eviction_recovery_deadline"));
            }
            // Each round starts from the remaining original intents.
            recovery.after = None;
            let round = recovery.round(ledger, artifacts, &|| Instant::now() >= deadline)?;
            if !round.recovering {
                return Ok(());
            }
            if round.completed == 0 {
                return Err(failure("artifact_eviction_recovery_deferred"));
            }
        }
    }
}

/// The scan cursor of startup recovery; eligibility belongs to the GlobalLedger.
#[derive(Default)]
struct EvictionRecovery {
    after: Option<ArtifactId>,
    policy: FailedRunRetentionPolicy,
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
            .retention_candidates(self.after, self.policy)
            .map_err(ledger_failure)?;
        if candidates.references.len() > RETENTION_ROUND_OBJECTS {
            return Err(failure("artifact_retention_candidate_bound_exceeded"));
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
                .admit_artifact_eviction(guard, self.policy)
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
                        .ok_or_else(|| failure("artifact_retention_byte_count_overflow"))?;
                    round.completed += 1;
                }
            }
        }
        self.after = candidates.next_after;
        Ok(round)
    }
}

fn failure(code: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(
        code,
        "maintain_frame_retention",
        RuntimeErrorCode::LedgerFailure,
    )
}

fn ledger_failure(error: GlobalLedgerError) -> RuntimeHostError {
    RuntimeHostError::fatal(
        error.code(),
        error.operation(),
        RuntimeErrorCode::LedgerFailure,
    )
    .with_native_detail(format!("{error}; detail={:?}", error.detail()))
}
