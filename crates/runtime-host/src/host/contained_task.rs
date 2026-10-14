// SPDX-License-Identifier: AGPL-3.0-only

use super::facts::FactPublicationPurpose;
use super::lease::HostClaim;
use super::resource_fence::UnconfirmedResources;
use super::runtime_facts::{TASK_GAME_FACT_KEY, TASK_PAGE_FACT_KEY, TASK_SERVER_FACT_KEY};
use super::*;
use crate::codes::{HostCode, HostLocation};
use crate::error::spelled;
use crate::resource_adjustment::{
    AdjustmentPlan, ConfirmationInput, PlannedAdjustment, PlannedBase, PlannedPublication,
    PreRunRecord, ReadingFrameInput,
};
use actingcommand_contract::codes::{
    ContractLocation, DeviceSelfCheckFailure, RuntimeErrorCode, SharedCode,
};
use actingcommand_contract::outcome::{Code, CodeStr, Location, Outcome};
use actingcommand_contract::{
    CaptureBackendName, CaptureExtent, CaptureGeometryObservation, DesignatedEffectTally,
    SCHEDULING_PAUSE_CHECKPOINT_GRACE_MS, SchedulingDrainSummary, TaskGeometryConclusion,
    TaskGeometryFailure, TaskGeometryFrame, TaskGeometryObservation, TaskGeometryPhase,
    TaskGeometryRecheckTrigger, TaskRunIdentity,
};
use actingcommand_execution_kernel::{
    CaptureGeometrySessionRef, ContainedTaskGuardEvaluation, DesignatedEffectCap,
    ExecutionKernelCode, ResourceAdjustmentResult, ResourceReadingValue, StepPacing,
};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_CONTAINED_TASK_OCR_FAILURE_DETAIL_BYTES: usize = 64 * 1024;
/// How often an instance pause re-reads its instance's in-flight contained runs (and, in stage
/// (c), its lease and owed client resets).
pub(super) const SCHEDULING_PAUSE_DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(20);
const CONTAINED_TASK_POST_ADMISSION_OCR_FAILED: &str =
    SharedCode::ContainedTaskPostAdmissionOcrFailed.as_str();
const PREVIOUS_EPOCH_SETTLEMENT_OPERATION: HostLocation =
    HostLocation::SettlePreviousEpochContainedRuns;
/// Workflow #369 Q-2: the longest a direct task run (CLI, console, MCP `ac_run_pack`) waits in
/// its instance's queue before it is refused `lease_queue_expired`.
const DIRECT_TASK_RUN_CLAIM_WAIT_MS: u64 = 60_000;
/// Workflow #369 Q-2, W-3: the run time a direct task run keeps after its wait; its wait cap is
/// min(`DIRECT_TASK_RUN_CLAIM_WAIT_MS`, the run's remaining deadline minus this).
const DIRECT_TASK_RUN_MINIMUM_RUN_MS: u64 = 30_000;

/// Workflow #369 Q-2, W-3: a direct task run's wait cap at its enqueue: min(60 s, the receipt
/// deadline's remaining time − the 30 s run budget). A remainder of 0 or less is a cap of 0.
fn direct_task_run_claim_cap_ms(receipt_deadline_monotonic_ms: u64, now_monotonic_ms: u64) -> u64 {
    receipt_deadline_monotonic_ms
        .saturating_sub(now_monotonic_ms)
        .saturating_sub(DIRECT_TASK_RUN_MINIMUM_RUN_MS)
        .min(DIRECT_TASK_RUN_CLAIM_WAIT_MS)
}

/// Reads one original run identity without creating identifiers or producer authority.
fn contained_task_recovery_identity(
    events: &[PersistedEvent],
    operation: impl Into<Location>,
) -> Result<Option<(TaskRunIdentity, Option<LeaseId>)>, RequestFailure> {
    let operation: Location = operation.into();
    let invalid = || {
        RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
            HostCode::ContainedTaskRecoveryIdentityInvalid,
            operation,
            RuntimeErrorCode::RuntimeHalted,
        ))
    };
    let mut identity = None;
    let mut lease_id = None;
    for event in events {
        if matches!(
            event.payload(),
            EventPayload::Task(TaskPayload::Semantic(_))
        ) {
            let found = TaskRunIdentity::from_semantic_event(event.payload(), event.links())
                .ok_or_else(invalid)?;
            if identity.is_some_and(|previous| previous != found) {
                return Err(invalid());
            }
            identity = Some(found);
        }
        if let Some(found) = event.links().lease_id().copied() {
            if lease_id.is_some_and(|previous| previous != found) {
                return Err(invalid());
            }
            lease_id = Some(found);
        }
    }
    if let Some(identity) = identity {
        if events
            .iter()
            .any(|event| !identity.matches_links(event.links()))
        {
            return Err(invalid());
        }
        Ok(Some((identity, lease_id)))
    } else {
        Ok(None)
    }
}
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ContainedTaskCheckpointIdentity {
    request_id: RequestId,
    instance_id: InstanceId,
    lease_id: LeaseId,
}

#[cfg(test)]
impl ContainedTaskCheckpointIdentity {
    pub(super) const fn new(
        request_id: RequestId,
        instance_id: InstanceId,
        lease_id: LeaseId,
    ) -> Self {
        Self {
            request_id,
            instance_id,
            lease_id,
        }
    }

    pub(crate) const fn request_id(&self) -> RequestId {
        self.request_id
    }

    pub(crate) const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub(crate) const fn lease_id(&self) -> LeaseId {
        self.lease_id
    }
}

/// Where a contained-task checkpoint hook runs.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContainedTaskCheckpointPoint {
    /// Before the run's `PackageAdmitted` fact.
    PackageAdmitted,
    /// Workflow #369 E3 (#670 final review): a run's terminal, after the check that its lease
    /// is current and before the terminal is appended.
    BeforeTerminalAppend,
    /// Workflow #369 E3 (#670 final review M-1): a failed scheduled run's lease end, after its
    /// terminal and before the admission guard is taken.
    FailedRunLeaseEnd,
}

#[cfg(test)]
pub(super) struct ContainedTaskCheckpointTestHook {
    pub(super) point: ContainedTaskCheckpointPoint,
    /// `None`: the run on the hook's lease, whatever its request.
    pub(super) request_id: Option<RequestId>,
    pub(super) instance_id: InstanceId,
    pub(super) lease_id: Option<LeaseId>,
    pub(super) execution_thread: std::thread::ThreadId,
    pub(super) action: Box<dyn FnOnce(ContainedTaskCheckpointIdentity) + Send>,
    pub(super) consumed: Arc<AtomicU64>,
    pub(super) observed: Arc<Mutex<Option<ContainedTaskCheckpointIdentity>>>,
}

#[cfg(test)]
pub(crate) struct ContainedTaskCheckpointTestControl {
    pub(super) consumed: Arc<AtomicU64>,
    pub(super) observed: Arc<Mutex<Option<ContainedTaskCheckpointIdentity>>>,
}

#[cfg(test)]
impl ContainedTaskCheckpointTestControl {
    pub(crate) fn consumed(&self) -> u64 {
        self.consumed.load(Ordering::Acquire)
    }

    pub(crate) fn observed(&self) -> RuntimeHostResult<Option<ContainedTaskCheckpointIdentity>> {
        Ok(*lock(
            &self.observed,
            crate::codes::test_spellings::TestLocation::ReadContainedTaskCheckpointTestControl,
        )?)
    }
}

#[derive(Clone)]
pub(super) struct ContainedTaskTerminalDraft {
    pub(super) task_id: IssuedTaskId,
    pub(super) run_id: IssuedRunId,
    pub(super) outcome: TaskOutcome,
    pub(super) intent_already_recorded: bool,
    pub(super) final_page: Option<String>,
    pub(super) executed_steps: Option<u32>,
    /// A failed or cancelled run's failure: the terminal's outcome (ledger revision 3; the
    /// terminal's severity is its category, so no separate severity is kept).
    pub(super) failure: Option<Outcome>,
    /// The failure point's verdict that the failure must pause the task: the terminal's
    /// `failure_severe` (see [`terminal_failure_severe`]). Only a failed terminal sets it.
    pub(super) failure_severe: bool,
    pub(super) scheduling_outcome: Option<(String, SchedulingOutcomeDeclaration)>,
    pub(super) selected_scheduling_outcome: Option<String>,
    /// Workflow #390 OUT-2: the caps the run held its designated effect set to; empty without
    /// a set.
    pub(super) designated_effect_caps: Vec<DesignatedEffectCap>,
    pub(super) capture_summary: Option<CapturePipelineSummary>,
    pub(super) task_timing: Option<Box<actingcommand_contract::TaskTimingObservations>>,
    /// Readings a successful run took (Workflow #335 S5b, #384 D2-S2); in memory only.
    pub(super) resource_readings: Option<ContainedTaskResourceReadings>,
}

/// The failure point's verdict that a failed run must pause its task: the terminal's
/// `failure_severe` (Workflow #378 A3, model section 8.1; coordinator ruling 9, 2026-10-13).
/// `severe` is the verdict where the failure arose (the run poisoned the Runtime or its error
/// was fatal); a `linear_steps` main package's device backend failure that did not poison the
/// Runtime is rerun only and so not severe (R25-2). The policy class follows the verdict
/// (`actingcommand_contract::scheduled_failure_class`) in the live settlement, the startup
/// recovery and the ledger alike.
fn terminal_failure_severe(
    severe: bool,
    failure: &Outcome,
    poison_runtime: bool,
    linear_main: bool,
) -> bool {
    severe
        && !(linear_main
            && !poison_runtime
            && crate::failure_identity::RERUN_ONLY_BACKEND_FAILURES
                .contains(&failure.code().as_str()))
}

/// The resource readings a successful run took (Workflow #335 S5b; Workflow #384 D2-S2: on every
/// frame it handed to recognition), carried in memory from the kernel traces to the terminal,
/// which publishes each one as an instance fact. Never serialized.
#[derive(Clone)]
pub(super) struct ContainedTaskResourceReadings {
    /// The fact scope: the run's registered instance alias.
    pub(super) instance_alias: String,
    /// The admitted package's entry task id (`source_detector`).
    pub(super) task_label: String,
    /// The admitted package identity as SHA-256 hex (`resource_bundle_hash`).
    pub(super) resource_bundle_hash: String,
    /// The frames whose readings held, in hand-off order.
    pub(super) frames: Vec<JoinedReadingFrame>,
    /// Workflow #384 D2-S3: the frame each confirmation of an operation that declares
    /// adjustments was made on, in confirmation order.
    pub(super) confirmation_frames: Vec<IssuedFrameId>,
    /// Workflow #384 D2-S3: the plan of the run's confirmed adjustments, when it confirmed any.
    pub(super) adjustments: Option<AdjustmentPlan>,
}

/// Workflow #384 D2-S2: the readings that held on one frame handed to recognition.
#[derive(Clone)]
pub(super) struct JoinedReadingFrame {
    /// The frame's device capture time (`observed_at_unix_ms`).
    pub(super) captured_at: SystemTime,
    pub(super) frame_id: IssuedFrameId,
    /// In mark order.
    pub(super) readings: Vec<ResourceReadingValue>,
}

/// Workflow #335 S5b: the instance fact of one reading. The scope is the run's instance, the
/// observation time the terminal frame's capture time; the TTL is the declared `valid_for_ms`
/// from the detector contract. `None` for a reading the fact record cannot hold (an integer
/// beyond `i64`, an expiry beyond `u64`).
fn resource_reading_fact_record(
    readings: &ContainedTaskResourceReadings,
    reading: &ResourceReadingValue,
    observed_at_unix_ms: u64,
    run_text: &str,
    frame_text: &str,
) -> Option<FactRecord> {
    let declaration = &reading.declaration;
    Some(FactRecord {
        scope: FactScope::Instance {
            instance_id: readings.instance_alias.clone(),
        },
        key: declaration.fact_key.clone(),
        content: FactContent::Inline {
            value: ContractFactValue::Integer(i64::try_from(reading.value).ok()?),
        },
        observed_at_unix_ms,
        expires_at_unix_ms: Some(observed_at_unix_ms.checked_add(declaration.valid_for_ms)?),
        ttl_policy: Some(actingcommand_contract::FactTtlPolicy {
            minimum_ms: declaration.valid_for_ms,
            maximum_ms: declaration.valid_for_ms,
            source: actingcommand_contract::FactTtlSource::DetectorContract,
        }),
        confidence_milli: reading.confidence_milli,
        source_detector: format!(
            "{}{}/{}",
            actingcommand_contract::RESOURCE_READING_DETECTOR_PREFIX,
            readings.task_label,
            declaration.id
        ),
        source_snapshot_id: actingcommand_contract::resource_reading_snapshot_id(
            run_text,
            frame_text,
            &declaration.id,
        ),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: readings.resource_bundle_hash.clone(),
        invalidate_on: Vec::new(),
    })
}

/// Workflow #384 D2-S3 (D2 model §3.6): the instance fact of one adjustment write: the adjusted
/// value, observed when its operation was confirmed, until the adjusted record's expiry, with
/// the lower confidence of the two; its snapshot names the run, the confirmation frame and the
/// `fact.published` event of the adjusted record. `None` for a value or a lifetime the fact
/// record cannot hold.
fn resource_adjustment_fact_record(
    readings: &ContainedTaskResourceReadings,
    adjustment: &PlannedAdjustment,
    run_text: &str,
    frame_text: &str,
    base_event_text: &str,
) -> Option<FactRecord> {
    let ttl_policy = match adjustment.expires_at_unix_ms {
        None => None,
        Some(expires) => {
            let ttl_ms = expires.checked_sub(adjustment.observed_at_unix_ms)?;
            Some(actingcommand_contract::FactTtlPolicy {
                minimum_ms: ttl_ms,
                maximum_ms: ttl_ms,
                source: actingcommand_contract::FactTtlSource::DetectorContract,
            })
        }
    };
    Some(FactRecord {
        scope: FactScope::Instance {
            instance_id: readings.instance_alias.clone(),
        },
        key: adjustment.fact_key.clone(),
        content: FactContent::Inline {
            value: ContractFactValue::Integer(adjustment.value?),
        },
        observed_at_unix_ms: adjustment.observed_at_unix_ms,
        expires_at_unix_ms: adjustment.expires_at_unix_ms,
        ttl_policy,
        confidence_milli: adjustment.confidence_milli,
        source_detector: format!(
            "{}{}/{}",
            actingcommand_contract::RESOURCE_ADJUSTMENT_DETECTOR_PREFIX,
            readings.task_label,
            adjustment.adjustment_id
        ),
        source_snapshot_id: actingcommand_contract::resource_adjustment_snapshot_id(
            run_text,
            frame_text,
            &adjustment.adjustment_id,
            base_event_text,
        ),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: readings.resource_bundle_hash.clone(),
        invalidate_on: Vec::new(),
    })
}

/// Workflow #335 S5b: refuses a successful terminal whose record `id` (a reading, or a
/// Workflow #384 D2-S3 adjustment) the fact store refused with `refusal`:
/// `contained_task_resource_reading_rejected`, detail `<id>:<fact store code>`.
fn refuse_resource_reading(
    draft: &mut ContainedTaskTerminalDraft,
    id: &str,
    refusal: &str,
) -> RequestFailure {
    refuse_resource_readings(
        draft,
        HostCode::ContainedTaskResourceReadingRejected,
        format!("{id}:{refusal}"),
    )
}

/// Workflow #384 D2-S2: a frame's device capture time in Unix ms; a clock before the epoch or
/// beyond `u64` poisons the Runtime.
fn reading_unix_ms(at: SystemTime) -> Result<u64, RequestFailure> {
    at.duration_since(UNIX_EPOCH)
        .map_err(|_| SharedCode::SystemClockInvalid.code())
        .and_then(|elapsed| {
            u64::try_from(elapsed.as_millis()).map_err(|_| HostCode::SystemClockOverflow.code())
        })
        .map_err(|code| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                code,
                HostLocation::PublishResourceReadings,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })
}

/// Workflow #335 S5b, #336 PKG-S2 (option 丙, M-PKG D8): `resource_bundle_hash` of a reading:
/// the lowercase hex SHA-256 of `package:<package_id>`. A package changed in place keeps the
/// value; two packages never share one; no package content is hashed.
pub(crate) fn resource_reading_bundle_hash(package_id: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("package:{package_id}").as_bytes())
    )
}

/// The canonical text of a typed identifier (`run_<hex>`, `frame_<hex>`).
fn resource_reading_identifier_text(
    identifier: &impl serde::Serialize,
) -> Result<String, RequestFailure> {
    serde_json::to_value(identifier)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| RequestFailure::poison_without_terminal(runtime_identifier_error()))
}

/// Workflow #335 S5b: the `<id>:<reason>` of a declaration the kernel refused is the receipt's
/// native detail. A reading the kernel could not take no longer fails a run (Workflow #384
/// D2-S2).
fn resource_reading_failure_detail(code: &str, detail: Option<&str>) -> Option<String> {
    if spelled(
        code,
        &[ExecutionKernelCode::ContainedTaskResourceReadingInvalid.code()],
    ) {
        detail.map(str::to_owned)
    } else {
        None
    }
}

/// Workflow #335 S5b: rewrites a successful terminal whose readings were refused to the
/// failure `code` (as a refused settlement projection is) and returns the failure answered
/// after that terminal, with `detail` as native detail.
fn refuse_resource_readings(
    draft: &mut ContainedTaskTerminalDraft,
    code: impl Into<Code>,
    detail: String,
) -> RequestFailure {
    let error = RuntimeHostError::request(
        code,
        HostLocation::AppendContainedTaskTerminal,
        RuntimeErrorCode::BackendOperationFailed,
    );
    draft.outcome = TaskOutcome::Failure;
    draft.final_page = None;
    draft.failure = Some(error.outcome());
    draft.failure_severe = false;
    draft.scheduling_outcome = None;
    draft.selected_scheduling_outcome = None;
    RequestFailure::request(
        error.with_native_detail(detail),
        RuntimeReceiptState::Failed,
        None,
    )
}

fn resource_reading_poison(code: impl Into<Code>) -> RequestFailure {
    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
        code,
        ContractLocation::RunContainedTask,
        RuntimeErrorCode::RuntimeHalted,
    ))
}

/// The terminal checks under the fact write gate: an already committed terminal of the run
/// (its rejection is appended) or the open chain's summaries and settlement projection.
enum ContainedTaskTerminalCheck {
    AlreadyCommitted(Box<PersistedEvent>),
    Open {
        summary_committed: bool,
        requested_summary: Option<actingcommand_contract::CaptureSummaryRecord>,
        projection: Result<Option<SchedulingDisposition>, RequestFailure>,
    },
}

pub(super) struct ContainedRunControl {
    pub(super) request_id: RequestId,
    pub(super) instance_id: InstanceId,
    client_cancellable: bool,
    deadline_monotonic_ms: AtomicU64,
    cancellation_reason: AtomicU8,
    /// The committed `task.failed` terminal of this run (slice #316-B4 trigger input).
    failed_terminal: Mutex<Option<FailedTaskTerminal>>,
    /// Workflow #336 L2d: the prepared package's identity, set once the run executes; the
    /// stuck-recovery ladder takes the return-home package configured for its game and server.
    package: std::sync::OnceLock<PackageIdentity>,
    /// Workflow #369 H-1 (review M-1): set only by the policy-run path; its failed lease end
    /// hands the key to a ladder.
    ladder_source: std::sync::OnceLock<super::recovery_ladder::LadderSource>,
}

/// Workflow #336 L2d: a prepared package's `control.json` game, server and resolution, which a
/// return-home package from `return_home_packages` must share to stand in for it.
#[derive(Clone)]
pub(super) struct PackageIdentity {
    pub(super) game: String,
    pub(super) server: String,
    pub(super) resolution: (u32, u32),
}

impl PackageIdentity {
    pub(super) fn of(package: &PreparedContainedTask) -> Self {
        Self {
            game: package.game().to_owned(),
            server: package.server().to_owned(),
            resolution: package.resolution(),
        }
    }
}

/// A committed `task.failed` terminal: the run it ended and the code of its outcome.
#[derive(Clone)]
pub(super) struct FailedTaskTerminal {
    pub(super) task_id: IssuedTaskId,
    pub(super) run_id: IssuedRunId,
    pub(super) failure_code: CodeStr,
}

impl ContainedRunControl {
    const NONE: u8 = 0;
    const CLIENT_REQUESTED: u8 = 1;
    const DEADLINE_EXCEEDED: u8 = 2;
    const PAUSE_REQUESTED: u8 = 3;

    const fn new(request_id: RequestId, instance_id: InstanceId, client_cancellable: bool) -> Self {
        Self {
            request_id,
            instance_id,
            client_cancellable,
            deadline_monotonic_ms: AtomicU64::new(0),
            cancellation_reason: AtomicU8::new(Self::NONE),
            failed_terminal: Mutex::new(None),
            package: std::sync::OnceLock::new(),
            ladder_source: std::sync::OnceLock::new(),
        }
    }

    /// Workflow #369 H-1: marks this run as a policy run whose failed lease end may hand the
    /// key to a ladder; set once, before the run executes.
    pub(super) fn note_ladder_source(
        &self,
        source: super::recovery_ladder::LadderSource,
    ) -> RuntimeHostResult<()> {
        self.ladder_source.set(source).map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::ContainedTaskLadderSourceStateInvalid,
                HostLocation::NoteContainedTaskLadderSource,
                RuntimeErrorCode::RuntimeHalted,
            )
        })
    }

    /// Workflow #369 H-1: a policy run's ladder trigger, taken with its committed `task.failed`
    /// terminal; `None` for every other run and for a run that committed no failure.
    pub(super) fn ladder_trigger(
        &self,
    ) -> RuntimeHostResult<Option<super::recovery_ladder::LadderTrigger>> {
        let Some(source) = self.ladder_source.get() else {
            return Ok(None);
        };
        Ok(self.take_failed_terminal()?.map(|terminal| {
            super::recovery_ladder::LadderTrigger::new(terminal, source, self.package().cloned())
        }))
    }

    /// The run's committed `task.failed` terminal, taken once.
    pub(super) fn take_failed_terminal(&self) -> RuntimeHostResult<Option<FailedTaskTerminal>> {
        Ok(lock(&self.failed_terminal, HostLocation::TakeFailedTaskTerminal)?.take())
    }

    /// Workflow #336 L2d: notes the prepared package's identity, once per run.
    fn note_package(&self, package: &PreparedContainedTask) -> RuntimeHostResult<()> {
        self.package.set(PackageIdentity::of(package)).map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::ContainedTaskPackageStateInvalid,
                HostLocation::NoteContainedTaskPackage,
                RuntimeErrorCode::RuntimeHalted,
            )
        })
    }

    /// Workflow #336 L2d: the prepared package's identity, once the run executed.
    pub(super) fn package(&self) -> Option<&PackageIdentity> {
        self.package.get()
    }

    fn set_deadline(&self, deadline_monotonic_ms: u64) -> RuntimeHostResult<()> {
        if deadline_monotonic_ms == 0 {
            return Err(RuntimeHostError::fatal(
                HostCode::ContainedTaskDeadlineStateInvalid,
                HostLocation::ConfigureContainedTaskDeadline,
                RuntimeErrorCode::RuntimeHalted,
            ));
        }
        let current = self.deadline_monotonic_ms.load(Ordering::Acquire);
        if current == 0 {
            self.deadline_monotonic_ms
                .compare_exchange(
                    0,
                    deadline_monotonic_ms,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .map(|_| ())
                .map_err(|_| {
                    RuntimeHostError::fatal(
                        HostCode::ContainedTaskDeadlineStateInvalid,
                        HostLocation::ConfigureContainedTaskDeadline,
                        RuntimeErrorCode::RuntimeHalted,
                    )
                })
        } else {
            self.deadline_monotonic_ms
                .store(current.min(deadline_monotonic_ms), Ordering::Release);
            Ok(())
        }
    }

    fn request_cancel(&self) {
        let _ = self.cancellation_reason.compare_exchange(
            Self::NONE,
            Self::CLIENT_REQUESTED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// Workflow #191 ps1 (b): an instance pause whose drain timeout expired asks the run to
    /// stop at its next checkpoint. Returns whether this call set the reason; a run already
    /// ending for another reason keeps that reason.
    pub(super) fn request_pause_cancel(&self) -> bool {
        self.cancellation_reason
            .compare_exchange(
                Self::NONE,
                Self::PAUSE_REQUESTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    pub(super) fn cancellation_reason(
        &self,
        now_monotonic_ms: u64,
    ) -> Option<ContainedTaskCancellationReason> {
        let deadline = self.deadline_monotonic_ms.load(Ordering::Acquire);
        if deadline != 0 && now_monotonic_ms >= deadline {
            let _ = self.cancellation_reason.compare_exchange(
                Self::NONE,
                Self::DEADLINE_EXCEEDED,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
        match self.cancellation_reason.load(Ordering::Acquire) {
            Self::CLIENT_REQUESTED => Some(ContainedTaskCancellationReason::ClientRequested),
            Self::DEADLINE_EXCEEDED => Some(ContainedTaskCancellationReason::DeadlineExceeded),
            Self::PAUSE_REQUESTED => Some(ContainedTaskCancellationReason::PausedByOperator),
            _ => None,
        }
    }

    pub(super) fn deadline(&self) -> u64 {
        self.deadline_monotonic_ms.load(Ordering::Acquire)
    }
}

fn pause_drain_fatal(code: impl Into<Code>) -> RequestFailure {
    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
        code,
        HostLocation::DrainPausedInstance,
        RuntimeErrorCode::RuntimeHalted,
    ))
}

/// The two deadlines of one instance pause, fixed when it starts: runs still in flight at `drain`
/// are asked to stop, and the pause fails if its drain or its device hand-back (Workflow #191 ps2)
/// has not finished by `grace` (`drain` plus `SCHEDULING_PAUSE_CHECKPOINT_GRACE_MS`).
#[derive(Clone, Copy)]
pub(super) struct SchedulingPauseDeadlines {
    pub(super) drain: Instant,
    pub(super) grace: Instant,
}

impl SchedulingPauseDeadlines {
    pub(super) fn after(drain_timeout_ms: u64) -> Result<Self, RequestFailure> {
        let overflow = || pause_drain_fatal(HostCode::SchedulingPauseDrainDeadlineOverflow);
        let drain = Instant::now()
            .checked_add(Duration::from_millis(drain_timeout_ms))
            .ok_or_else(overflow)?;
        let grace = drain
            .checked_add(Duration::from_millis(SCHEDULING_PAUSE_CHECKPOINT_GRACE_MS))
            .ok_or_else(overflow)?;
        Ok(Self { drain, grace })
    }
}

struct ActiveContainedRun<'a> {
    active: &'a Mutex<BTreeMap<RequestId, Arc<ContainedRunControl>>>,
    request_id: RequestId,
    control: Arc<ContainedRunControl>,
}

/// The original request's identity, as a resubmission of it would arrive: only its request,
/// correlation and causation reach the recovery links of what a restart writes for it (Workflow
/// #338 R3; #369 E3). `None` when the identity does not form a request.
pub(super) fn recovery_request_identity(
    request_id: RequestId,
    correlation_id: CorrelationId,
    causation_id: Option<&actingcommand_contract::CausationId>,
    submitted_at_unix_ms: u64,
) -> Option<RuntimeRequest> {
    let mut identity = serde_json::Map::new();
    identity.insert(
        "schema_version".to_owned(),
        serde_json::Value::from(actingcommand_contract::RUNTIME_REQUEST_SCHEMA_VERSION),
    );
    identity.insert(
        "request_id".to_owned(),
        serde_json::to_value(request_id).ok()?,
    );
    identity.insert(
        "correlation_id".to_owned(),
        serde_json::to_value(correlation_id).ok()?,
    );
    if let Some(causation_id) = causation_id {
        identity.insert(
            "causation_id".to_owned(),
            serde_json::to_value(causation_id).ok()?,
        );
    }
    identity.insert(
        "actor".to_owned(),
        serde_json::to_value(EventActor::Cli).ok()?,
    );
    identity.insert(
        "source".to_owned(),
        serde_json::to_value(EventSource::Cli).ok()?,
    );
    identity.insert(
        "submitted_at_unix_ms".to_owned(),
        serde_json::Value::from(submitted_at_unix_ms),
    );
    identity.insert(
        "operation".to_owned(),
        serde_json::to_value(RuntimeOperation::Health).ok()?,
    );
    serde_json::from_value::<RuntimeRequest>(serde_json::Value::Object(identity)).ok()
}

/// Workflow #369 S3a: the lease a host package run executes under, which its caller holds.
pub(super) struct HeldPackageLease {
    pub(super) token: LeaseToken,
    pub(super) connection_id: ConnectionId,
}

/// Workflow #369 S3a: a host package run prepared before its lease (`prepare_package_run`).
pub(super) struct PreparedPackageRun<'a> {
    instance_id: InstanceId,
    instance_alias: String,
    task_request: ContainedTaskRequest,
    task_request_message: RuntimeRequest,
    prepared: PreparedContainedTask,
    prerequisites: Vec<PreparedContainedTask>,
    interrupts: Option<Box<super::standard_pack::RunInterrupts>>,
    task_id: IssuedTaskId,
    run_id: IssuedRunId,
    run_links: RuntimeRunLinks,
    execution_provenance: ExecutionBackendProvenance,
    active_run: ActiveContainedRun<'a>,
    control_request_id: RequestId,
}

impl ActiveContainedRun<'_> {
    fn control(&self) -> Arc<ContainedRunControl> {
        Arc::clone(&self.control)
    }
}

impl Drop for ActiveContainedRun<'_> {
    fn drop(&mut self) {
        let mut active = self
            .active
            .lock()
            .expect("active contained-run registry poisoned");
        assert!(
            active.remove(&self.request_id).is_some(),
            "active contained-run identity missing during cleanup"
        );
    }
}

#[derive(Default)]
struct CaptureEvidenceAccumulator {
    pipeline: Option<CapturePipeline>,
    frame_store: Option<actingcommand_artifact_store::FrameStore>,
    pipeline_failure: Option<ArtifactStoreError>,
    counts: CapturePipelineCounts,
    next_frame_index: usize,
    last_frame_index: Option<usize>,
    frames: Vec<PersistedFrameEvidence>,
    pinned: BTreeMap<(Option<usize>, PinnedFrameReason), Option<ArtifactReference>>,
    pending_post_input: bool,
}

impl CaptureEvidenceAccumulator {
    fn captured(&mut self) -> Result<usize, RequestFailure> {
        let frame_index = self.next_frame_index;
        self.next_frame_index = self.next_frame_index.checked_add(1).ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                SharedCode::CaptureSummaryCountOverflow,
                HostLocation::RecordContainedTaskCapture,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })?;
        self.counts.captured = self.counts.captured.checked_add(1).ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                SharedCode::CaptureSummaryCountOverflow,
                HostLocation::RecordContainedTaskCapture,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })?;
        Ok(frame_index)
    }

    fn persisted(
        &mut self,
        frame_index: usize,
        artifact: &ArtifactReference,
    ) -> Result<(), RequestFailure> {
        if self
            .frames
            .iter()
            .any(|frame| frame.frame_index == frame_index)
        {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    SharedCode::CaptureSummaryFrameConflict,
                    HostLocation::RecordContainedTaskCapture,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            ));
        }
        self.counts.persisted = self.counts.persisted.checked_add(1).ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                SharedCode::CaptureSummaryCountOverflow,
                HostLocation::RecordContainedTaskCapture,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })?;
        self.frames.push(PersistedFrameEvidence {
            frame_index,
            pinned_reason: None,
            artifact: artifact.clone(),
        });
        self.last_frame_index = Some(frame_index);
        if self.pending_post_input {
            self.pin_frame(frame_index, PinnedFrameReason::PostInput)?;
            self.pending_post_input = false;
        }
        Ok(())
    }

    fn effect_completed(&mut self) -> Result<(), RequestFailure> {
        if self.pending_post_input {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    HostCode::CaptureSummaryPhaseConflict,
                    HostLocation::RecordContainedTaskEffect,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            ));
        }
        self.pending_post_input = true;
        Ok(())
    }

    fn pin_last(&mut self, reason: PinnedFrameReason) -> Result<(), RequestFailure> {
        match self.last_frame_index {
            Some(frame_index) => self.pin_frame(frame_index, reason),
            None => self.pin(None, reason, None),
        }
    }

    fn pin_frame(
        &mut self,
        frame_index: usize,
        reason: PinnedFrameReason,
    ) -> Result<(), RequestFailure> {
        let artifact = self
            .frames
            .iter()
            .find(|frame| frame.frame_index == frame_index)
            .map(|frame| frame.artifact.clone())
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    HostCode::CaptureSummaryFrameMissing,
                    HostLocation::RecordContainedTaskPin,
                    RuntimeErrorCode::RuntimeHalted,
                ))
            })?;
        self.pin(Some(frame_index), reason, Some(artifact))
    }

    fn pin(
        &mut self,
        frame_index: Option<usize>,
        reason: PinnedFrameReason,
        artifact: Option<ArtifactReference>,
    ) -> Result<(), RequestFailure> {
        let key = (frame_index, reason);
        if let Some(existing) = self.pinned.get(&key) {
            if existing == &artifact {
                return Ok(());
            }
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    SharedCode::CaptureSummaryPinConflict,
                    HostLocation::RecordContainedTaskPin,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            ));
        }
        self.pinned.insert(key, artifact);
        Ok(())
    }

    fn finalize(
        mut self,
        outcome: TaskOutcome,
        host: &HostShared,
    ) -> Result<CapturePipelineSummary, RequestFailure> {
        if self.pending_post_input {
            self.pin(None, PinnedFrameReason::PostInput, None)?;
        }
        self.pin_last(PinnedFrameReason::Terminal)?;
        if outcome == TaskOutcome::Failure {
            self.pin_last(PinnedFrameReason::Failure)?;
        }
        if let Some(pipeline) = self.pipeline.as_mut() {
            if let Some(error) = self.pipeline_failure.take() {
                return Err(online_observation::observation_artifact_failure(error));
            }
            let mut sink = online_observation::ObservationArtifactSink {
                ledger: &host.ledger,
                events: &host.events,
                verified: None,
                frame_retention: Some((
                    host.owner_epoch,
                    actingcommand_contract::ArtifactPinReason::Explicit,
                )),
            };
            let mut events = RuntimeArtifactEventSink {
                ledger: &host.ledger,
                events: &host.events,
            };
            let mut publish = |store: &ArtifactStore, input: ArtifactWriteRequest<'_>| {
                sink.publish_frame(store, input, None)
            };
            for ((index, reason), original) in &self.pinned {
                if let Some(index) = index {
                    let material = pipeline
                        .pin_frame_with_publisher(*index, *reason, &mut events, Some(&mut publish))
                        .map_err(online_observation::observation_artifact_failure)?;
                    if original.as_ref() != Some(&material) {
                        return Err(online_observation::observation_integrity_failure(
                            HostCode::CapturePinMaterialChanged,
                        ));
                    }
                }
            }
            let summary = pipeline
                .finish_with_publisher(&mut events, Some(&mut publish))
                .map_err(online_observation::observation_artifact_failure)?;
            pipeline
                .cleanup_spills()
                .map_err(online_observation::observation_artifact_failure)?;
            self.counts = summary.counts;
            self.frames = summary.frames;
        }
        let pinned = self
            .pinned
            .into_iter()
            .map(|((frame_index, reason), artifact)| PinnedFrameEvidence {
                frame_index,
                reason,
                artifact,
            })
            .collect();
        build_capture_pipeline_summary(self.counts, pinned, self.frames).map_err(|error| {
            RequestFailure::poison_without_terminal(RuntimeHostError::relayed_fatal(
                error.code(),
                HostLocation::FinalizeContainedTaskCaptureSummary.as_str(),
                RuntimeErrorCode::RuntimeHalted,
            ))
        })
    }
}

pub(super) struct RuntimeContainedTask<'a> {
    pub(super) host: &'a HostShared,
    pub(super) request: &'a ValidatedRuntimeRequest<'a>,
    pub(super) token: &'a LeaseToken,
    pub(super) instance_alias: &'a str,
    connection_id: ConnectionId,
    pub(super) task_id: IssuedTaskId,
    pub(super) run_id: IssuedRunId,
    execution_provenance: ExecutionBackendProvenance,
    pub(super) control: Arc<ContainedRunControl>,
    /// The admitted package's `control.json` `game` / `server`, published as `task.game` /
    /// `task.server` once `task.package_admitted` is appended.
    declared_game_server: (String, String),
    pub(super) last_frame_id: Option<IssuedFrameId>,
    /// The device capture time of the frame `last_frame_id` names.
    last_frame_captured_at: Option<SystemTime>,
    /// Workflow #384 D2-S2: the frames the run handed to recognition, in hand-off order: each
    /// frame's device capture time and id.
    reading_frames: Vec<(SystemTime, IssuedFrameId)>,
    /// How many of `reading_frames` the run has joined, in order.
    joined_reading_frames: usize,
    /// The readings that held on the joined frames, accepted before `Finalizing` (Workflow #335
    /// S5b, #384 D2-S2).
    taken_resource_readings: Vec<JoinedReadingFrame>,
    /// Workflow #384 D2-S3: the admitted package's entry task id.
    entry_task_label: String,
    /// Workflow #384 D2-S3: the operation and the ledger sequence of the `task.step_finished` of
    /// the step finished last.
    last_step_finished: Option<(String, u64)>,
    /// Workflow #384 D2-S3: the confirmations of operations that declare adjustments, in order:
    /// the frame's device capture time and id and the sequence of the step's
    /// `task.step_finished`.
    adjustment_confirmations: Vec<(SystemTime, IssuedFrameId, u64)>,
    /// How many of `adjustment_confirmations` the run has joined, in order.
    joined_adjustment_confirmations: usize,
    /// The adjustments of the joined confirmations, in confirmation order.
    taken_adjustments: Vec<Vec<ResourceAdjustmentResult>>,
    /// The plan of the run's adjustments, made once its last confirmation is joined.
    adjustment_plan: Option<AdjustmentPlan>,
    /// Workflow #384 D2-S2: the frame the diagnostic rows now written belong to, when it is not
    /// the last captured one (a joined reading's evaluation and miss).
    pub(super) diagnostic_frame: Option<IssuedFrameId>,
    geometry_session: Option<CaptureGeometrySessionRef>,
    geometry_frame: Option<TaskGeometryFrame>,
    geometry_initial: Option<(CaptureExtent, CaptureGeometryObservation)>,
    geometry_deadline: Option<Instant>,
    geometry_rechecked: bool,
    input_step_action_id: Option<ActionId>,
    post_input_action_id: Option<ActionId>,
    /// #398 R1 §5.2 (RD-3i): when the last input completed; the next capture's floor. Unlike
    /// `post_input_action_id` it outlives the step, because the first frame after an input may
    /// be the next step's.
    pending_input_floor: Option<Instant>,
    /// #398 R1 §5.2 (RD-3i): the stream sequence of the frame delivered last (none for a
    /// snapshot backend); a capture without a new input must deliver a newer frame.
    last_frame_sequence: Option<u64>,
    last_capture_input_action_id: Option<ActionId>,
    /// Backend acquisition span of the last successful capture, consumed by its `CaptureCompleted` trace.
    last_capture_acquire_us: Option<u64>,
    expected_stability_declaration: Option<StabilityTerminationDeclaration>,
    stability: Option<RuntimeContainedTaskStability>,
    expects_post_admission_ocr: bool,
    /// Workflow #390 OUT-1 (FCM O-05): the package collects OCR fields, so a success that
    /// passed no field page ends with no OCR observation and no fields report.
    collects_ocr_fields: bool,
    post_admission_ocr_observations: u32,
    post_admission_ocr_comparison_recorded: bool,
    pub(super) current_recognition_id: Option<IssuedRecognitionId>,
    pub(super) step_actions: BTreeMap<u32, (IssuedActionId, String)>,
    pub(super) step_index_offset: u32,
    pub(super) executed_steps: Option<u32>,
    /// Workflow #373 NAV-6: the run's shared interrupt pages, read in its preparation; taken
    /// out while one is handled.
    pub(super) interrupts: Option<Box<super::standard_pack::RunInterrupts>>,
    /// Workflow #373 NAV-6: the steps the interrupt layers of the run executed.
    pub(super) interrupt_steps: u32,
    /// Workflow #373 NAV-6: the effective configuration records of interrupt layer packages,
    /// one per distinct package, beyond the seven of the run (M-373 §8.3).
    pub(super) interrupt_configurations: u8,
    /// Workflow #373 NAV-6: the most steps the run's own layers can execute (its prerequisite
    /// chain and package, or its entry recovery package and package); interrupt layers fit
    /// within 1000 beside them.
    pub(super) planned_steps: u32,
    /// Workflow #373 NAV-6: one more than the highest step index recorded so far.
    pub(super) next_free_step_index: u32,
    /// Workflow #373 NAV-6: the steps of interrupt layers since the running layer began, which
    /// its later steps follow.
    pub(super) step_shift: u32,
    /// Workflow #373 NAV-6: the running layer's step that an interrupt layer ran inside: its own
    /// index and the index it keeps.
    pub(super) pinned_step: Option<(u32, u32)>,
    /// Workflow #373 NAV-6: whether the traces now recorded are an interrupt layer's.
    pub(super) interrupt_layer: bool,
    entry_preflight_recorded: bool,
    sampling_run_seed: Option<u64>,
    used_action_seeds: BTreeSet<u64>,
    finalizing: Option<TaskOutcome>,
    capture_evidence: CaptureEvidenceAccumulator,
    pub(super) configuration_records: u8,
    configuration_capture_recorded: bool,
    configuration_input_recorded: bool,
    pub(super) diagnostic_stream: Option<actingcommand_artifact_store::ArtifactStream>,
    pub(super) diagnostic_records: u64,
    pub(super) task_timing: task_timing::TaskTimingObserver,
    pub(super) diagnostic_step: Option<task_diagnostic::DiagnosticStep>,
    pub(super) diagnostic_physical: Option<ActionId>,
    /// Workflow #308: a `run_ending` select record that could not be written. Its failure is
    /// recorded in the ledger when it happens; this keeps whether it poisons the runtime, and
    /// the failure itself when the ledger refused that record too, for the run to join to its
    /// own failure.
    unwritten_selection_record: Option<(bool, Option<RuntimeHostError>)>,
}

pub(super) struct EntryRecoveryRuntime<'a, 'host> {
    pub(super) inner: &'a mut RuntimeContainedTask<'host>,
}

impl ContainedTaskRuntime for EntryRecoveryRuntime<'_, '_> {
    type Error = RequestFailure;

    fn supports_application_effect(&self) -> bool {
        self.inner.supports_application_effect()
    }

    fn control_application(
        &mut self,
        action: ApplicationLifecycleAction,
    ) -> Result<actingcommand_execution_kernel::ApplicationEffectSupport, Self::Error> {
        self.inner.control_application(action)
    }

    fn update_run_progress(&mut self, executed_steps: u32) {
        self.inner.update_run_progress(executed_steps);
    }

    fn observe_task_timing(&mut self, context: ContainedTaskTimingContext) {
        self.inner.task_timing.replace_context(Some(context));
    }

    fn task_boundary_identity(
        &self,
        boundary: TaskTimingBoundary,
    ) -> task_timing::BoundaryIdentity {
        self.inner.timing_identity(boundary)
    }

    fn observe_task_boundary(
        &mut self,
        timing: actingcommand_execution_kernel::ContainedTaskBoundaryTiming,
    ) {
        self.inner.task_timing.kernel_boundary(timing);
    }

    fn record_page_evaluations(
        &mut self,
        phase: &'static str,
        results: &actingcommand_page_detector::PageBatchResult,
        timing: Option<ContainedTaskEvaluationTiming>,
    ) -> Result<(), Self::Error> {
        if let Some(timing) = timing {
            self.inner.task_timing.record_evaluation(
                timing,
                self.inner.last_frame_id.map(|id| *id.transport()),
                self.inner.current_recognition_id.map(|id| *id.transport()),
            );
        }
        self.inner.diagnostic_pages(phase, results)?;
        self.inner.record_capture_recognition(results)
    }
    /// Workflow #386 SR-1 (scheduling model §2.2): the Runtime samples.
    fn sampling_supported(&self) -> bool {
        true
    }

    /// Workflow #386 SR-1: a cancellation (deadline, client, operator pause) under its own code.
    fn candidate_sampling_checkpoint(&mut self) -> Result<(), Self::Error> {
        self.inner.ensure_active()
    }

    /// Workflow #386 SR-1 (scheduling model §2.1 item 4): the run's step pacing.
    fn step_pacing(&mut self) -> Result<StepPacing, Self::Error> {
        self.inner.step_pacing()
    }

    fn record_candidate_evaluation(
        &mut self,
        target_id: &str,
        result: &actingcommand_recognition_pack::RecognitionPackResult<
            actingcommand_recognition_pack::TargetEvaluation,
        >,
        timing: ContainedTaskEvaluationTiming,
    ) -> Result<(), Self::Error> {
        self.inner.task_timing.record_evaluation(timing, None, None);
        self.inner.diagnostic_guard(
            Some(target_id),
            Some(result),
            ContainedTaskGuardEvaluation::CandidateSample,
        )?;
        self.inner.ensure_active()
    }

    fn record_guard_evaluation(
        &mut self,
        target: Option<&str>,
        result: Option<
            &actingcommand_recognition_pack::RecognitionPackResult<
                actingcommand_recognition_pack::TargetEvaluation,
            >,
        >,
        evaluation: ContainedTaskGuardEvaluation,
    ) -> Result<(), Self::Error> {
        self.inner.diagnostic_guard(target, result, evaluation)
    }
    fn record_ocr_evaluation(
        &mut self,
        target: &str,
        result: &actingcommand_recognition_pack::RecognitionPackResult<
            actingcommand_recognition_pack::OcrObservationEvaluation,
        >,
    ) -> Result<(), Self::Error> {
        self.inner.diagnostic_ocr(target, result)
    }

    fn classify_error(error: &Self::Error) -> ContainedTaskRuntimeErrorClass {
        RuntimeContainedTask::classify_error(error)
    }

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        self.inner.capture()
    }

    fn committed_input_frame(
        &mut self,
        reference: actingcommand_contract::InputFrameReference,
    ) -> Result<Option<InputFrameContext>, Self::Error> {
        self.inner.committed_input_frame(reference)
    }

    fn action_seed(
        &mut self,
        step_index: u32,
        operation_label: &str,
    ) -> Result<Option<u64>, Self::Error> {
        self.inner.action_seed(step_index, operation_label)
    }

    fn selection_state(
        &mut self,
        request: actingcommand_execution_kernel::SelectionStateRequest,
    ) -> Result<actingcommand_execution_kernel::SelectionState, Self::Error> {
        self.inner.selection_state(request)
    }

    fn input(
        &mut self,
        action: InputAction,
        frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inner.input(action, frame)
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        if matches!(
            &trace,
            ContainedTaskTrace::PackageAdmitted { .. }
                | ContainedTaskTrace::RunStarted
                | ContainedTaskTrace::Finalizing { .. }
        ) {
            Ok(())
        } else {
            self.inner.record(trace)
        }
    }

    /// Workflow #373 NAV-6: a layer of the run looks at the run's shared interrupt pages too.
    fn shared_interrupt(
        &mut self,
        frame: actingcommand_execution_kernel::SharedInterruptFrame<'_>,
    ) -> Result<
        actingcommand_execution_kernel::SharedInterruptVerdict,
        ContainedTaskRunError<Self::Error>,
    > {
        self.inner.offered_shared_interrupt(frame)
    }
}

fn fail_contained_task_entry(
    runtime: &RuntimeContainedTask<'_>,
    code: &'static str,
) -> Result<ContainedTaskOutcome, ContainedTaskRunError<RequestFailure>> {
    runtime
        .record_entry_fact(TaskSemanticFact::EntryTargetDisposition {
            disposition: TaskEntryTargetDisposition::FailClosed,
            outcome: Some(Outcome::relayed(code)),
        })
        .map_err(ContainedTaskRunError::Boundary)?;
    Err(ContainedTaskRunError::task(code))
}

struct RuntimeContainedTaskStability {
    declaration: StabilityTerminationDeclaration,
    previous_frame_id: IssuedFrameId,
    last_step_index: u32,
    consecutive_unchanged: u32,
    pending_terminal: Option<RuntimeContainedTaskStabilityTerminal>,
    terminal_recorded: bool,
}

struct RuntimeContainedTaskStabilityTerminal {
    step_index: u32,
    operation_label: String,
    reason: StabilityTerminalReason,
}

struct RuntimeContainedTaskStabilityComparison {
    step_index: u32,
    operation_label: String,
    declaration: StabilityTerminationDeclaration,
    result: StabilityComparisonResult,
    prior_consecutive_unchanged: u32,
    new_consecutive_unchanged: u32,
    terminal_reason: Option<StabilityTerminalReason>,
}

#[derive(serde::Serialize)]
struct RuntimeContainedTaskStabilityDiagnostic<'a> {
    schema_version: &'static str,
    task_id: &'a TaskId,
    run_id: &'a RunId,
    action_id: &'a ActionId,
    step_index: u32,
    operation_label: &'a str,
    previous_frame_id: &'a FrameId,
    current_frame_id: &'a FrameId,
    region: &'a actingcommand_execution_kernel::StabilityRegion,
    comparison_mode: actingcommand_execution_kernel::StabilityComparisonMode,
    comparison_parameters: &'a actingcommand_execution_kernel::StabilityComparisonParameters,
    result: StabilityComparisonResult,
    prior_consecutive_unchanged: u32,
    new_consecutive_unchanged: u32,
    consecutive_unchanged_threshold: u32,
    max_steps: u32,
    terminal_reason: Option<StabilityTerminalReason>,
}

#[derive(serde::Serialize)]
struct RuntimeContainedTaskOcrObservationDiagnostic<'a> {
    schema_version: &'static str,
    task_id: &'a TaskId,
    run_id: &'a RunId,
    frame_id: &'a FrameId,
    frame_index: u32,
    frame_artifact: &'a actingcommand_contract::ArtifactReference,
    observation: &'a PostAdmissionOcrObservation,
}

#[derive(serde::Serialize)]
struct RuntimeContainedTaskOcrComparisonDiagnostic<'a, T: serde::Serialize> {
    schema_version: &'static str,
    task_id: &'a TaskId,
    run_id: &'a RunId,
    final_frame_id: &'a FrameId,
    report: &'a T,
}

#[derive(serde::Serialize)]
struct RuntimeContainedTaskOcrFailureDiagnostic<'a> {
    schema_version: &'static str,
    request_id: RequestId,
    correlation_id: CorrelationId,
    instance_id: InstanceId,
    lease_id: LeaseId,
    task_id: &'a TaskId,
    run_id: &'a RunId,
    frame_id: &'a FrameId,
    failure_code: &'static str,
    detail: &'a str,
    detail_utf8_bytes: usize,
    detail_sha256: String,
}

/// The registered code of a frame whose page recognition failed (Workflow #378 A2-P): the
/// recognition pack's code of the failed target's cause, else
/// `contained_task_recognition_failed`.
fn recognition_failure_code(error: &actingcommand_page_detector::PageDetectorError) -> Code {
    match &error.failed_target {
        Some(target) => target.cause.code().code(),
        None => SharedCode::ContainedTaskRecognitionFailed.code(),
    }
}

fn task_geometry_error(code: impl Into<Code>) -> RuntimeHostError {
    RuntimeHostError::request(
        code,
        HostLocation::ObserveTaskGeometry,
        RuntimeErrorCode::CaptureFailed,
    )
}

fn task_geometry_failure_ref(error: &RuntimeHostError) -> TaskGeometryFailure {
    TaskGeometryFailure {
        outcome: error.outcome(),
        event_id: error.diagnostics().recorded_event().get().copied(),
    }
}

fn task_geometry_conclusion_error(conclusion: TaskGeometryConclusion) -> RuntimeHostError {
    task_geometry_error(match conclusion {
        TaskGeometryConclusion::AspectMismatch { .. } => {
            HostCode::ContainedTaskGeometryFrameDisplayMismatch
        }
        TaskGeometryConclusion::Unknown { .. } => HostCode::ContainedTaskGeometryUnknown,
        TaskGeometryConclusion::Unavailable => HostCode::ContainedTaskGeometryUnavailable,
        TaskGeometryConclusion::Pass | TaskGeometryConclusion::FixtureNotApplicable => {
            HostCode::ContainedTaskGeometryProducerBindingMismatch
        }
    })
}

fn task_geometry_request_failure(
    error: RuntimeHostError,
    event: Option<&PersistedEvent>,
) -> RequestFailure {
    if let Some(event) = event {
        let _ = error.diagnostics().recorded_event().set(*event.event_id());
    }
    RequestFailure {
        state: RuntimeReceiptState::Failed,
        terminal: event.map(terminal),
        poison_runtime: error.is_fatal(),
        task_failure: Some(TaskFailureEvidence {
            outcome: error.outcome(),
            severe: error.is_fatal(),
        }),
        error: Box::new(error),
    }
}

impl RuntimeContainedTask<'_> {
    fn geometry_operation_deadline(&self) -> Result<Instant, RequestFailure> {
        let deadline = self
            .host
            .package_material_deadline(self.control.deadline())?;
        Ok(self
            .task_timing
            .context()
            .map_or(deadline, |context| deadline.min(context.deadline())))
    }

    fn read_task_geometry(
        &self,
        reuse_frame: bool,
    ) -> RuntimeHostResult<CaptureGeometryObservation> {
        let deadline = self
            .geometry_deadline
            .ok_or_else(|| task_geometry_error(HostCode::ContainedTaskGeometryBudgetUnavailable))?;
        let now = self.host.monotonic_ms()?;
        if Instant::now() >= deadline || self.control.cancellation_reason(now).is_some() {
            return Err(task_geometry_error(
                HostCode::ContainedTaskGeometryBudgetUnavailable,
            ));
        }
        let session = self.geometry_session.as_ref().ok_or_else(|| {
            task_geometry_error(HostCode::ContainedTaskGeometrySessionUnavailable)
        })?;
        let frame = self
            .geometry_frame
            .as_ref()
            .ok_or_else(|| task_geometry_error(HostCode::ContainedTaskGeometryFrameUnavailable))?;
        // This checks the original session; it cannot create or reopen a producer.
        self.host
            .execution()?
            .validate_capture_geometry_session(session, deadline)
            .map_err(|error| {
                RuntimeHostError::execution(HostLocation::ValidateTaskGeometrySession, &error)
            })?;
        if frame.backend == CaptureBackendName::FixtureSimulation
            || reuse_frame && frame.backend == CaptureBackendName::NemuIpc
        {
            return Ok(frame.producer.clone());
        }
        self.host
            .execution()?
            .observe_capture_geometry(session, deadline)
            .map_err(|error| RuntimeHostError::execution(HostLocation::ObserveTaskGeometry, &error))
    }

    fn record_task_geometry(
        &self,
        phase: TaskGeometryPhase,
        trigger: Option<TaskGeometryRecheckTrigger>,
        original_failure: Option<TaskGeometryFailure>,
        result: &RuntimeHostResult<CaptureGeometryObservation>,
    ) -> Result<(TaskGeometryConclusion, PersistedEvent), RequestFailure> {
        if let Err(error) = result {
            // Preserve the original fatal/Unconfirmed boundary before attempting another fact.
            if error.projection().code == RuntimeErrorCode::LedgerFailure {
                return Err(RequestFailure::poison_without_terminal(error.clone()));
            }
            let settled = self
                .host
                .settle_unconfirmed_resources(error, Some(self.token.instance_id()), self.links())
                .map_err(|failure| {
                    RequestFailure::poison_without_terminal(
                        failure.with_related_failure("geometry_observation", error),
                    )
                })?;
            match settled {
                // Workflow #403: the instance is fenced; the run fails non-fatally.
                UnconfirmedResources::Fenced(fence) => {
                    return Err(RequestFailure::fence_without_terminal(fence));
                }
                UnconfirmedResources::Retained => {
                    return Err(RequestFailure::poison_without_terminal(error.clone()));
                }
                UnconfirmedResources::Confirmed if error.is_fatal() => {
                    return Err(RequestFailure::poison_without_terminal(error.clone()));
                }
                UnconfirmedResources::Confirmed => {}
            }
        }
        let conclusion = match result {
            Ok(observation) => TaskGeometryObservation::assess(
                self.geometry_frame.as_ref().ok_or_else(|| {
                    RequestFailure::poison_without_terminal(task_geometry_error(
                        HostCode::ContainedTaskGeometryFrameUnavailable,
                    ))
                })?,
                observation,
            ),
            Err(_) => TaskGeometryConclusion::Unavailable,
        };
        let mut links = self.links();
        if let Some(frame) = self.last_frame_id {
            links = links.with_frame_id(frame);
        }
        // Failure finalization may record this observation after the execution deadline.
        // It deliberately does not pass through record_entry_fact / ensure_active.
        let event = self
            .host
            .append_event(
                if matches!(
                    conclusion,
                    TaskGeometryConclusion::Pass | TaskGeometryConclusion::FixtureNotApplicable
                ) {
                    EventSeverity::Info
                } else {
                    EventSeverity::Warning
                },
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                links.clone(),
                TaskPayloadDraft::semantic(
                    TaskSemanticFact::GeometryObserved {
                        observation: Box::new(TaskGeometryObservation {
                            phase,
                            frame: self.geometry_frame.clone(),
                            observation: result.as_ref().ok().cloned(),
                            conclusion,
                            trigger,
                            original_failure,
                            unavailable: result.as_ref().err().map(task_geometry_failure_ref),
                            display_map: None,
                        }),
                    },
                    AuditInput::new(),
                ),
            )
            .map_err(|mut failure| {
                if let Err(observation_error) = result {
                    failure.error = Box::new(
                        failure
                            .error
                            .as_ref()
                            .clone()
                            .with_related_failure("geometry_observation", observation_error),
                    );
                }
                failure
            })?;
        if let Err(error) = result {
            self.host
                .record_required_failure(error, &event, links)
                .map_err(|failure| {
                    RequestFailure::poison_without_terminal(
                        failure.with_related_failure("geometry_observation", error),
                    )
                })?;
        }
        Ok((conclusion, event))
    }

    fn retain_task_geometry(
        &mut self,
        frame: &Frame,
        frame_id: IssuedFrameId,
        session: CaptureGeometrySessionRef,
    ) -> Result<(), RequestFailure> {
        #[cfg(test)]
        let width = if self
            .host
            .frame_extent_fault_for_test
            .swap(false, Ordering::AcqRel)
        {
            0
        } else {
            frame.width
        };
        #[cfg(not(test))]
        let width = frame.width;
        // Workflow #387 D1c-G: a frame without an extent ends this run only, with its code, as
        // the other task geometry failures do (class 5, so it counts toward direction ①); the
        // Runtime and the other instances go on.
        let extent = CaptureExtent::new(width, frame.height).ok_or_else(|| {
            task_geometry_request_failure(
                task_geometry_error(HostCode::ContainedTaskGeometryFrameExtentInvalid),
                None,
            )
        })?;
        self.geometry_frame = Some(TaskGeometryFrame {
            frame_id: *frame_id.transport(),
            extent,
            backend: frame.backend_name,
            captured_at: frame.captured_at,
            producer: frame.geometry.clone(),
        });
        if session.instance_id() != self.token.instance_id()
            || self
                .geometry_session
                .as_ref()
                .is_some_and(|original| !original.same_session(&session))
            || (frame.backend_name == CaptureBackendName::FixtureSimulation)
                != (self.execution_provenance == ExecutionBackendProvenance::FixtureSimulation)
        {
            return Err(task_geometry_request_failure(
                task_geometry_error(HostCode::ContainedTaskGeometryProducerBindingMismatch),
                None,
            ));
        }
        if self.geometry_session.is_none() {
            self.geometry_session = Some(session);
        }
        if self.geometry_initial.is_some() {
            return Ok(());
        }
        // Workflow #392 DA-A: no frame is refused for its aspect ratio, so every first frame
        // reads its display's geometry.
        let result = self.read_task_geometry(true);
        let (conclusion, event) =
            self.record_task_geometry(TaskGeometryPhase::Initial, None, None, &result)?;
        match (conclusion, result) {
            (
                TaskGeometryConclusion::Pass | TaskGeometryConclusion::FixtureNotApplicable,
                Ok(value),
            ) => {
                if conclusion == TaskGeometryConclusion::Pass {
                    self.host
                        .publish_display_facts(
                            self.token.instance_id(),
                            self.instance_alias,
                            extent,
                            &value,
                        )
                        .map_err(|error| task_geometry_request_failure(error, Some(&event)))?;
                }
                // Only a confirmed GlobalLedger commit opens the input prerequisite.
                self.geometry_initial = Some((extent, value));
                Ok(())
            }
            (_, Err(error)) => Err(task_geometry_request_failure(error, Some(&event))),
            (conclusion, Ok(_)) => Err(task_geometry_request_failure(
                task_geometry_conclusion_error(conclusion),
                Some(&event),
            )),
        }
    }

    fn require_task_geometry_for_input(&self) -> Result<(), RequestFailure> {
        let failure = || {
            task_geometry_request_failure(
                task_geometry_error(HostCode::ContainedTaskGeometryInputPrerequisiteMissing),
                None,
            )
        };
        let (initial_extent, initial_observation) =
            self.geometry_initial.as_ref().ok_or_else(failure)?;
        let frame = self.geometry_frame.as_ref().ok_or_else(failure)?;
        let session = self.geometry_session.as_ref().ok_or_else(failure)?;
        if self.last_frame_id.map(|id| *id.transport()) != Some(frame.frame_id)
            || frame.extent != *initial_extent
        {
            return Err(failure());
        }
        let conclusion = TaskGeometryObservation::assess(frame, initial_observation);
        if !matches!(
            (self.execution_provenance, conclusion),
            (
                ExecutionBackendProvenance::PhysicalDevice,
                TaskGeometryConclusion::Pass
            ) | (
                ExecutionBackendProvenance::FixtureSimulation,
                TaskGeometryConclusion::FixtureNotApplicable
            )
        ) {
            return Err(task_geometry_request_failure(
                task_geometry_conclusion_error(conclusion),
                None,
            ));
        }
        self.host
            .execution()?
            .validate_capture_geometry_session(session, self.geometry_deadline.ok_or_else(failure)?)
            .map_err(|error| {
                task_geometry_request_failure(
                    RuntimeHostError::execution(HostLocation::ValidateTaskGeometryInput, &error),
                    None,
                )
            })
    }

    fn recheck_task_geometry(
        &mut self,
        execution: &mut Result<ContainedTaskOutcome, ContainedTaskRunError<RequestFailure>>,
    ) {
        if self.geometry_rechecked {
            return;
        }
        let (trigger, primary) = match execution {
            Err(ContainedTaskRunError::Task(error)) => {
                let trigger = match error.code().as_str() {
                    code if spelled(code, &[SharedCode::ContainedTaskRecognitionFailed.code()]) => {
                        TaskGeometryRecheckTrigger::RecognitionFailed
                    }
                    code if spelled(code, &[SharedCode::ContainedTaskPageUnknown.code()]) => {
                        TaskGeometryRecheckTrigger::PageUnknown
                    }
                    _ => return,
                };
                let mut primary = RuntimeHostError::relayed_request(
                    error.code().as_str(),
                    ContractLocation::RunContainedTask.as_str(),
                    RuntimeErrorCode::BackendOperationFailed,
                );
                if let Some(detail) = error.detail() {
                    primary = primary.with_native_detail(detail.to_owned());
                }
                (trigger, primary)
            }
            Err(ContainedTaskRunError::NonfatalOperation(failure))
                if !failure.poison_runtime
                    && !failure.error.is_fatal()
                    && spelled(
                        failure.error.code(),
                        &[
                            SharedCode::InputBackendOperationFailed.code(),
                            DeviceSelfCheckFailure::InputBackendOpenFailed.code(),
                        ],
                    ) =>
            {
                (
                    TaskGeometryRecheckTrigger::InputFailed,
                    failure.error.as_ref().clone(),
                )
            }
            // In particular, record/ledger errors and fatal exits authorize no new observation.
            _ => return,
        };
        self.geometry_rechecked = true;
        let result = self.read_task_geometry(false);
        if let Err(mut failure) = self.record_task_geometry(
            TaskGeometryPhase::Recheck,
            Some(trigger),
            Some(task_geometry_failure_ref(&primary)),
            &result,
        ) {
            let observation_context = failure.error.lifecycle.as_ref();
            let mut preserved_primary = primary.clone();
            // A newly unconfirmed producer still belongs to the original close owner.
            // Keep that lifecycle boundary when retaining the execution error as primary.
            if observation_context.resource_quiescence == Some(ResourceQuiescence::Unconfirmed) {
                preserved_primary.lifecycle.resource_quiescence =
                    observation_context.resource_quiescence;
                preserved_primary.lifecycle.instance_id = observation_context.instance_id;
                preserved_primary = preserved_primary.with_related_causes(&failure.error);
            }
            failure.error = Box::new(
                if failure.error.projection().code == RuntimeErrorCode::LedgerFailure {
                    failure
                        .error
                        .as_ref()
                        .clone()
                        .with_related_failure("prior_task", &primary)
                } else {
                    preserved_primary
                        .with_related_failure("geometry_recheck", &failure.error)
                        .into_fatal()
                },
            );
            *execution = Err(ContainedTaskRunError::Boundary(failure));
        }
    }

    fn record_geometry_triggered_recovery_failure(
        &self,
        package_label: String,
        primary: &RuntimeHostError,
    ) -> Result<(), RequestFailure> {
        // Preserve the two original recovery failure facts after a classified failure,
        // even when the operation has consumed its last remaining execution budget.
        for fact in [
            TaskSemanticFact::EntryRecoveryFailed {
                package_label: Some(package_label),
                outcome: primary.outcome(),
                role: None,
            },
            TaskSemanticFact::EntryTargetDisposition {
                disposition: TaskEntryTargetDisposition::FailClosed,
                outcome: Some(primary.outcome()),
            },
        ] {
            self.append_task(
                EventSeverity::Warning,
                self.links(),
                TaskPayloadDraft::semantic(fact, AuditInput::new()),
            )
            .map_err(|mut failure| {
                failure.error = Box::new(
                    failure
                        .error
                        .as_ref()
                        .clone()
                        .with_related_failure("prior_task", primary),
                );
                failure
            })?;
        }
        Ok(())
    }

    /// Workflow #336 L2b: the same facts for a prerequisite gate, one `EntryRecoveryFailed` per
    /// open prerequisite package (innermost first) and one `FailClosed`, written even when the
    /// operation consumed its last remaining execution budget.
    pub(super) fn record_geometry_triggered_prerequisite_failure(
        &self,
        open_packages: &[String],
        primary: &RuntimeHostError,
    ) -> Result<(), RequestFailure> {
        let outcome = primary.outcome();
        for fact in open_packages
            .iter()
            .rev()
            .map(|package_label| TaskSemanticFact::EntryRecoveryFailed {
                package_label: Some(package_label.clone()),
                outcome: outcome.clone(),
                role: None,
            })
            .chain([TaskSemanticFact::EntryTargetDisposition {
                disposition: TaskEntryTargetDisposition::FailClosed,
                outcome: Some(outcome.clone()),
            }])
        {
            self.append_task(
                EventSeverity::Warning,
                self.links(),
                TaskPayloadDraft::semantic(fact, AuditInput::new()),
            )
            .map_err(|mut failure| {
                failure.error = Box::new(
                    failure
                        .error
                        .as_ref()
                        .clone()
                        .with_related_failure("prior_task", primary),
                );
                failure
            })?;
        }
        Ok(())
    }

    fn record_initial_configuration(
        &mut self,
        request: &ContainedTaskRequest,
        prepared: &PreparedContainedTask,
    ) -> Result<(), RequestFailure> {
        let resolved = self
            .host
            .execution()?
            .resolve(self.instance_alias)
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::execution(
                    HostLocation::ResolveContainedTaskConfiguration,
                    &error,
                ))
            })?;
        let observed_at = self
            .host
            .monotonic_ms()
            .map_err(RequestFailure::poison_without_terminal)?;
        self.record_configuration(
            EffectiveConfigurationFacts::Initial {
                device: resolved.configuration().cloned(),
                timing: prepared.effective_timing(),
                request_timeout_ms: request.response_deadline_ms(),
                host_deadline_monotonic_ms: self.control.deadline(),
                observed_at_monotonic_ms: observed_at,
                host_remaining_ms: self.control.deadline().saturating_sub(observed_at),
                capture_observed: false,
                input_observed: false,
            },
            None,
            None,
            None,
        )
    }

    pub(super) fn record_configuration(
        &mut self,
        facts: EffectiveConfigurationFacts,
        frame_id: Option<IssuedFrameId>,
        action_id: Option<ActionId>,
        source_sequence: Option<u64>,
    ) -> Result<(), RequestFailure> {
        // Initial, at most one EntryRecovery per prerequisite package (three declared, Workflow
        // #336 L2b, and one return-home, L2c), Capture and Input; Workflow #373 NAV-6: and one
        // per distinct interrupt layer package.
        if self.configuration_records >= 7_u8.saturating_add(self.interrupt_configurations) {
            return Err(RequestFailure::poison_without_terminal(
                artifact_store_error(HostLocation::EffectiveConfigurationLimitExceeded),
            ));
        }
        let record = EffectiveConfigurationRecord {
            schema_version: EFFECTIVE_CONFIGURATION_SCHEMA.to_owned(),
            request_id: self.control.request_id,
            task_id: *self.task_id.transport(),
            run_id: *self.run_id.transport(),
            frame_id: frame_id.map(|id| *id.transport()),
            action_id,
            source_sequence,
            facts,
        };
        let bytes = serde_json::to_vec(&record).map_err(|_| {
            RequestFailure::poison_without_terminal(artifact_store_error(
                HostLocation::EncodeEffectiveConfiguration,
            ))
        })?;
        if bytes.len() as u64 > MAX_EFFECTIVE_CONFIGURATION_BYTES {
            return Err(RequestFailure::poison_without_terminal(
                artifact_store_error(HostLocation::EffectiveConfigurationSizeCheck),
            ));
        }
        let mut event_links = self
            .host
            .events
            .request_links(
                self.request,
                Some(self.token.instance_id()),
                Some(self.token.lease_id()),
                action_id,
            )
            .with_task_id(self.task_id)
            .with_run_id(self.run_id);
        let mut artifact_links = self.request.task_artifact_links(self.run_id);
        if let Some(frame_id) = frame_id {
            event_links = event_links.with_frame_id(frame_id);
            artifact_links = artifact_links.with_frame_id(frame_id);
        }
        let mut sink = RuntimeArtifactEventSink {
            ledger: &self.host.ledger,
            events: &self.host.events,
        };
        self.host
            .artifacts
            .put(
                ArtifactWriteRequest::new(
                    ArtifactKind::DiagnosticJson,
                    &bytes,
                    ArtifactWriteContext::new(
                        artifact_links,
                        event_links,
                        unix_ms_now().map_err(RequestFailure::poison_without_terminal)?,
                    ),
                    ArtifactIssuePolicy::new(
                        ArtifactProducer::ArtifactStore,
                        RetentionClass::DebugFull,
                        ArtifactRedactionState::NotRequired,
                    ),
                ),
                &mut sink,
            )
            .map_err(online_observation::observation_artifact_failure)?;
        self.configuration_records += 1;
        Ok(())
    }

    /// Workflow #384 D2-S3: the frame of the next confirmation to join, which must be the one
    /// captured at `captured_at`; the kernel joins confirmations in order.
    fn joined_adjustment_frame(
        &self,
        captured_at: SystemTime,
    ) -> Result<IssuedFrameId, RequestFailure> {
        match self
            .adjustment_confirmations
            .get(self.joined_adjustment_confirmations)
        {
            None => Err(resource_reading_poison(
                HostCode::ContainedTaskResourceReadingFrameMissing,
            )),
            Some((confirmed, _, _)) if *confirmed != captured_at => Err(resource_reading_poison(
                HostCode::ContainedTaskResourceReadingFrameMismatch,
            )),
            Some((_, frame_id, _)) => Ok(*frame_id),
        }
    }

    /// Workflow #384 D2-S3 (D2 model §3.6): once the run's last confirmation is joined, plans
    /// its adjustments from its readings, its adjustments and each adjusted key's current record
    /// (read under `fact_write_gate`), and writes each adjustment's outcome as a row of the
    /// task diagnostic on its confirmation frame.
    fn plan_resource_adjustments(&mut self) -> Result<(), RequestFailure> {
        let scope = FactScope::Instance {
            instance_id: self.instance_alias.to_owned(),
        };
        let keys = self
            .taken_adjustments
            .iter()
            .flatten()
            .map(|adjustment| adjustment.declaration.fact_key.clone())
            .collect::<BTreeSet<_>>();
        let mut pre_run = BTreeMap::new();
        for key in keys {
            if let Some((record, sequence, event_id)) = self
                .host
                .active_fact_revision(&scope, &key)
                .map_err(RequestFailure::poison_without_terminal)?
            {
                pre_run.insert(
                    key,
                    PreRunRecord {
                        record,
                        sequence,
                        event_id,
                    },
                );
            }
        }
        let now_unix_ms = self
            .host
            .clock
            .sample()
            .map_err(RequestFailure::poison_without_terminal)?
            .unix_ms;
        let plan = {
            let frames = self
                .taken_resource_readings
                .iter()
                .map(|frame| {
                    Ok(ReadingFrameInput {
                        captured_at: frame.captured_at,
                        observed_at_unix_ms: reading_unix_ms(frame.captured_at)?,
                        readings: &frame.readings,
                    })
                })
                .collect::<Result<Vec<_>, RequestFailure>>()?;
            let confirmations = self
                .adjustment_confirmations
                .iter()
                .zip(&self.taken_adjustments)
                .map(|((captured_at, _, step_sequence), adjustments)| {
                    Ok(ConfirmationInput {
                        captured_at: *captured_at,
                        observed_at_unix_ms: reading_unix_ms(*captured_at)?,
                        step_sequence: *step_sequence,
                        adjustments,
                    })
                })
                .collect::<Result<Vec<_>, RequestFailure>>()?;
            crate::resource_adjustment::plan_resource_adjustments(
                self.instance_alias,
                &self.entry_task_label,
                now_unix_ms,
                &frames,
                &confirmations,
                &pre_run,
            )
        };
        for planned in plan.outcomes.clone() {
            let frame_id = self
                .adjustment_confirmations
                .get(planned.confirmation)
                .map(|(_, frame_id, _)| *frame_id)
                .ok_or_else(|| {
                    resource_reading_poison(HostCode::ContainedTaskResourceReadingFrameMissing)
                })?;
            let recorded =
                self.diagnostic_adjustment(frame_id, planned.target_id.as_deref(), planned.outcome);
            self.diagnostic_frame = None;
            recorded?;
        }
        self.adjustment_plan = Some(plan);
        Ok(())
    }

    /// Workflow #384 D2-S2: the id of the next handed-off frame to join, which must be the one
    /// captured at `captured_at`; the kernel joins frames in hand-off order.
    fn joined_reading_frame(
        &self,
        captured_at: SystemTime,
    ) -> Result<IssuedFrameId, RequestFailure> {
        match self.reading_frames.get(self.joined_reading_frames) {
            None => Err(resource_reading_poison(
                HostCode::ContainedTaskResourceReadingFrameMissing,
            )),
            Some((handed_off, _)) if *handed_off != captured_at => Err(resource_reading_poison(
                HostCode::ContainedTaskResourceReadingFrameMismatch,
            )),
            Some((_, frame_id)) => Ok(*frame_id),
        }
    }

    fn ensure_active(&self) -> Result<(), RequestFailure> {
        let Some(reason) = self.control.cancellation_reason(
            self.host
                .monotonic_ms()
                .map_err(RequestFailure::poison_without_terminal)?,
        ) else {
            return Ok(());
        };
        let (code, projection) = match reason {
            ContainedTaskCancellationReason::DeadlineExceeded => (
                RuntimeErrorCode::ContainedTaskDeadlineExceeded.code(),
                RuntimeErrorCode::ContainedTaskDeadlineExceeded,
            ),
            ContainedTaskCancellationReason::ClientRequested => (
                RuntimeErrorCode::ContainedTaskCancelled.code(),
                RuntimeErrorCode::ContainedTaskCancelled,
            ),
            ContainedTaskCancellationReason::PausedByOperator => (
                RuntimeErrorCode::ContainedTaskPaused.code(),
                RuntimeErrorCode::ContainedTaskPaused,
            ),
            // No control asks for a blackout stop or a performance yield before SR-4b and P-k /
            // P-h write them (register S2, P7): a run's control holding one is invalid state.
            ContainedTaskCancellationReason::RecoveredAfterRestart
            | ContainedTaskCancellationReason::BlackoutStop
            | ContainedTaskCancellationReason::PerformanceYield => {
                return Err(RequestFailure::poison_without_terminal(
                    RuntimeHostError::fatal(
                        HostCode::ContainedTaskCancellationStateInvalid,
                        HostLocation::CheckContainedTaskDeadline,
                        RuntimeErrorCode::RuntimeHalted,
                    ),
                ));
            }
        };
        Err(RequestFailure::request(
            RuntimeHostError::request(code, ContractLocation::RunContainedTask, projection),
            RuntimeReceiptState::Failed,
            None,
        ))
    }

    fn poll_capture_pressure(&mut self) -> Result<(), RequestFailure> {
        if !self
            .capture_evidence
            .pipeline
            .as_ref()
            .is_some_and(CapturePipeline::is_paused)
        {
            return Ok(());
        }
        let mut sink = RuntimeArtifactEventSink {
            ledger: &self.host.ledger,
            events: &self.host.events,
        };
        let context = ArtifactWriteContext::new(
            self.request.task_artifact_links(self.run_id),
            self.links(),
            unix_ms_now().map_err(RequestFailure::poison_without_terminal)?,
        );
        let mut frame_sink = online_observation::ObservationArtifactSink {
            ledger: &self.host.ledger,
            events: &self.host.events,
            verified: None,
            frame_retention: Some((
                self.host.owner_epoch,
                frame_retention::capture_pin_reason(self.request),
            )),
        };
        let mut publish = |store: &ArtifactStore, input: ArtifactWriteRequest<'_>| {
            frame_sink.publish_frame(store, input, None)
        };
        loop {
            self.ensure_active()?;
            self.host.ledger.check_writer_health().map_err(|error| {
                RequestFailure::poison_without_terminal(
                    RuntimeHostError::relayed_fatal(
                        error.code(),
                        error.operation(),
                        RuntimeErrorCode::LedgerFailure,
                    )
                    .with_native_detail(error.to_string()),
                )
            })?;
            let pipeline = self
                .capture_evidence
                .pipeline
                .as_mut()
                .expect("paused pipeline exists");
            pipeline
                .poll_pressure_with_publisher(&context, &mut sink, Some(&mut publish))
                .map_err(|error| {
                    let links = pipeline
                        .failure_context(&error)
                        .unwrap_or(&context)
                        .event_links()
                        .clone();
                    self.host.capture_material_failure(error, links)
                })?;
            if !pipeline.is_paused() || self.finalizing.is_some() {
                return Ok(());
            }
            // The original run deadline/cancellation bounds the pause before any capture effect.
            if self.control.deadline() == 0 || self.host.fatal.is_shutdown_requested() {
                return Err(RequestFailure::request(
                    RuntimeHostError::request(
                        HostCode::CapturePressurePauseStopped,
                        HostLocation::AdmitContainedTaskCapture,
                        RuntimeErrorCode::CaptureFailed,
                    ),
                    RuntimeReceiptState::Denied,
                    None,
                ));
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn record_capture_recognition(
        &mut self,
        results: &actingcommand_page_detector::PageBatchResult,
    ) -> Result<(), RequestFailure> {
        let frame_id = self.last_frame_id.map(|id| *id.transport());
        let recognition_id = self.current_recognition_id.map(|id| *id.transport());
        let started = Instant::now();
        let budget_before = self.task_timing.budget_at(started);
        let result = (|| {
            let Some(index) = self.capture_evidence.last_frame_index else {
                return Ok(());
            };
            let state = match results {
                Ok(outcomes) => {
                    if let Some(error) = outcomes
                        .iter()
                        .find_map(|outcome| outcome.result.as_ref().err())
                    {
                        RecognitionState::Failed {
                            code: recognition_failure_code(error),
                        }
                    } else {
                        let mut matches = outcomes
                            .iter()
                            .filter_map(|outcome| outcome.result.as_ref().ok())
                            .filter(|evaluation| evaluation.matched);
                        let first = matches.next();
                        if matches.next().is_some() {
                            // No unique page relation is available to the frame cache.
                            return Ok(());
                        }
                        RecognitionState::from_matched_page(
                            first.map(|evaluation| evaluation.page_id.clone()),
                        )
                    }
                }
                Err(error) => RecognitionState::Failed {
                    code: recognition_failure_code(&error.cause),
                },
            };
            let mut sink = RuntimeArtifactEventSink {
                ledger: &self.host.ledger,
                events: &self.host.events,
            };
            if let Some(pipeline) = self.capture_evidence.pipeline.as_mut() {
                let mut frame_sink = online_observation::ObservationArtifactSink {
                    ledger: &self.host.ledger,
                    events: &self.host.events,
                    verified: None,
                    frame_retention: Some((
                        self.host.owner_epoch,
                        frame_retention::capture_pin_reason(self.request),
                    )),
                };
                let mut publish = |store: &ArtifactStore, input: ArtifactWriteRequest<'_>| {
                    frame_sink.publish_frame(store, input, None)
                };
                pipeline
                    .record_recognition_with_publisher(index, state, &mut sink, Some(&mut publish))
                    .map_err(|error| {
                        let context = pipeline
                            .failure_context(&error)
                            .or_else(|| pipeline.frame_context(index));
                        let Some(context) = context else {
                            return RequestFailure::poison_without_terminal(
                                RuntimeHostError::artifact(error).with_related_failure(
                                    "capture_frame_context",
                                    &RuntimeHostError::fatal(
                                        HostCode::ObservationFrameContextMissing,
                                        ContractLocation::RecordCaptureRecognition,
                                        RuntimeErrorCode::RuntimeHalted,
                                    ),
                                ),
                            );
                        };
                        self.host
                            .capture_material_failure(error, context.event_links().clone())
                    })?;
            }
            Ok(())
        })();
        let elapsed_us =
            actingcommand_execution_kernel::observe_instant_span(started, Instant::now());
        self.task_timing
            .capture_recognition(actingcommand_contract::TaskTimingSample {
                elapsed_us,
                budget_before,
                result: if result.is_ok() {
                    actingcommand_contract::TaskTimingResult::Ok
                } else {
                    actingcommand_contract::TaskTimingResult::Err
                },
                record_index: None,
                frame_id,
                recognition_id,
            });
        result
    }

    fn timing_identity(
        &self,
        boundary: actingcommand_contract::TaskTimingBoundary,
    ) -> task_timing::BoundaryIdentity {
        use actingcommand_contract::TaskTimingBoundary as Boundary;
        let before_capture = matches!(
            boundary,
            Boundary::Capture | Boundary::CapturePage | Boundary::CaptureActivePressure
        );
        let before_recognition = before_capture
            || matches!(
                boundary,
                Boundary::CaptureBackend
                    | Boundary::CaptureMaterial
                    | Boundary::CaptureCompletedRecord
                    | Boundary::RecognitionStartedRecord
            );
        task_timing::BoundaryIdentity {
            frame_id: (!before_capture)
                .then(|| self.last_frame_id.map(|id| *id.transport()))
                .flatten(),
            recognition_id: (!before_recognition)
                .then(|| self.current_recognition_id.map(|id| *id.transport()))
                .flatten(),
            step_index: self.diagnostic_step.as_ref().map(|step| step.index),
            action_id: if boundary == Boundary::Input {
                self.input_step_action_id
            } else {
                self.diagnostic_step.as_ref().map(|step| step.action_id)
            },
        }
    }

    const fn capture_origin(&self) -> (EventSource, OriginModule) {
        match self.execution_provenance {
            ExecutionBackendProvenance::PhysicalDevice => {
                (EventSource::Device, OriginModule::Capture)
            }
            ExecutionBackendProvenance::FixtureSimulation => {
                (EventSource::Lab, OriginModule::Actinglab)
            }
        }
    }

    pub(super) fn links(&self) -> EventLinksDraft {
        self.host
            .events
            .request_links(
                self.request,
                Some(self.token.instance_id()),
                Some(self.token.lease_id()),
                None,
            )
            .with_task_id(self.task_id)
            .with_run_id(self.run_id)
    }

    /// Workflow #308: the interpreter returns the run-ending capture error, not the failure of
    /// a `run_ending` select record, so that failure is recorded here as it happens: one
    /// `runtime.failed` lifecycle record (stage `runtime_lifecycle_selection_record`, the
    /// failure's own code and detail) linked to the run, before the run's terminal events and
    /// without touching the run's failure. When the ledger refuses that record as well, the
    /// failure is kept for the run to join to its own failure, which then poisons the runtime.
    fn record_unwritten_selection(&mut self, failure: &RequestFailure) {
        let error = failure
            .error
            .as_ref()
            .clone()
            .with_failure_stage(HostLocation::RuntimeLifecycleSelectionRecord);
        let unrecorded = self
            .host
            .append_lifecycle_failure(
                RuntimeLifecycleFailureStage::OperationCleanup,
                RuntimeLifecycleFailure::Host(&error),
                self.links(),
                None,
            )
            .err()
            .map(|ledger| error.with_related_failure("selection_record_ledger", &ledger));
        self.unwritten_selection_record = Some((failure.poison_runtime, unrecorded));
    }

    fn append_task(
        &self,
        severity: EventSeverity,
        links: EventLinksDraft,
        payload: TaskPayloadDraft,
    ) -> Result<(), RequestFailure> {
        self.host
            .append_event(
                severity,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                links,
                payload,
            )
            .map(|_| ())
    }

    pub(super) fn record_entry_fact(&self, fact: TaskSemanticFact) -> Result<(), RequestFailure> {
        self.ensure_active()?;
        let severity = if matches!(
            &fact,
            TaskSemanticFact::EntryRecoveryFailed { .. }
                | TaskSemanticFact::EntryTargetDisposition {
                    disposition: TaskEntryTargetDisposition::FailClosed,
                    ..
                }
        ) {
            EventSeverity::Warning
        } else {
            EventSeverity::Info
        };
        self.append_task(
            severity,
            self.links(),
            TaskPayloadDraft::semantic(fact, AuditInput::new()),
        )
    }

    fn absolute_step_index(&self, step_index: u32) -> Result<u32, RequestFailure> {
        self.layer_step_index(step_index).ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                HostCode::ContainedTaskStepIndexOverflow,
                ContractLocation::RunContainedTask,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })
    }

    fn offset_trace(
        &self,
        trace: ContainedTaskTrace,
    ) -> Result<ContainedTaskTrace, RequestFailure> {
        Ok(match trace {
            ContainedTaskTrace::StepStarted {
                step_index,
                operation_label,
                from_page,
                phase,
            } => ContainedTaskTrace::StepStarted {
                step_index: self.absolute_step_index(step_index)?,
                operation_label,
                from_page,
                phase,
            },
            ContainedTaskTrace::SelectionEvaluated {
                step_index,
                operation_label,
                selection,
                run_ending,
            } => ContainedTaskTrace::SelectionEvaluated {
                step_index: self.absolute_step_index(step_index)?,
                operation_label,
                selection,
                run_ending,
            },
            ContainedTaskTrace::EffectIntent {
                step_index,
                operation_label,
                action,
                sampling,
                guard,
            } => ContainedTaskTrace::EffectIntent {
                step_index: self.absolute_step_index(step_index)?,
                operation_label,
                action,
                sampling,
                guard,
            },
            ContainedTaskTrace::EffectCompleted {
                step_index,
                operation_label,
            } => ContainedTaskTrace::EffectCompleted {
                step_index: self.absolute_step_index(step_index)?,
                operation_label,
            },
            ContainedTaskTrace::StepFinished {
                step_index,
                operation_label,
                page_label,
                phase,
            } => ContainedTaskTrace::StepFinished {
                step_index: self.absolute_step_index(step_index)?,
                operation_label,
                page_label,
                phase,
            },
            ContainedTaskTrace::StabilityBaseline {
                step_index,
                operation_label,
                declaration,
            } => ContainedTaskTrace::StabilityBaseline {
                step_index: self.absolute_step_index(step_index)?,
                operation_label,
                declaration,
            },
            ContainedTaskTrace::StabilityComparison {
                step_index,
                operation_label,
                declaration,
                result,
                prior_consecutive_unchanged,
                new_consecutive_unchanged,
                terminal_reason,
            } => ContainedTaskTrace::StabilityComparison {
                step_index: self.absolute_step_index(step_index)?,
                operation_label,
                declaration,
                result,
                prior_consecutive_unchanged,
                new_consecutive_unchanged,
                terminal_reason,
            },
            ContainedTaskTrace::StabilityTerminal {
                step_index,
                operation_label,
                reason,
            } => ContainedTaskTrace::StabilityTerminal {
                step_index: self.absolute_step_index(step_index)?,
                operation_label,
                reason,
            },
            trace => trace,
        })
    }

    fn persist_post_admission_ocr_diagnostic(
        &self,
        frame_id: IssuedFrameId,
        bytes: &[u8],
        personal: bool,
        capacity_use: CapacityUse,
    ) -> Result<(), RequestFailure> {
        let event_links = self.links().with_frame_id(frame_id);
        let mut write_context = ArtifactWriteContext::new(
            self.request
                .task_artifact_links(self.run_id)
                .with_frame_id(frame_id),
            event_links,
            unix_ms_now().map_err(RequestFailure::poison_without_terminal)?,
        );
        if matches!(capacity_use, CapacityUse::Drain) {
            write_context = write_context.for_drain();
        }
        let mut sink = RuntimeArtifactEventSink {
            ledger: &self.host.ledger,
            events: &self.host.events,
        };
        self.host
            .artifacts
            .put(
                ArtifactWriteRequest::new(
                    ArtifactKind::DiagnosticJson,
                    bytes,
                    write_context,
                    ArtifactIssuePolicy::new(
                        ArtifactProducer::CapturePipeline,
                        RetentionClass::DebugFull,
                        if personal {
                            ArtifactRedactionState::Pending
                        } else {
                            ArtifactRedactionState::NotRequired
                        },
                    ),
                ),
                &mut sink,
            )
            .map(|_| ())
            .map_err(online_observation::observation_artifact_failure)
    }

    fn record_post_admission_ocr_failure(
        &self,
        failure_code: &'static str,
        detail: Option<&str>,
    ) -> Result<(), RequestFailure> {
        let Some(detail) = bounded_post_admission_ocr_failure_detail(failure_code, detail)
            .map_err(RequestFailure::poison_without_terminal)?
        else {
            return Ok(());
        };
        let frame_id = self.last_frame_id.ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                HostCode::ContainedTaskPostAdmissionOcrFailureFrameMissing,
                ContractLocation::RunContainedTask,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })?;
        let event_links = self.links();
        let request_id = event_links.request_id().copied().ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                HostCode::ContainedTaskPostAdmissionOcrFailureRequestMissing,
                ContractLocation::RunContainedTask,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })?;
        let correlation_id = event_links.correlation_id().copied().ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                HostCode::ContainedTaskPostAdmissionOcrFailureCorrelationMissing,
                ContractLocation::RunContainedTask,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })?;
        let diagnostic = RuntimeContainedTaskOcrFailureDiagnostic {
            schema_version: "actingcommand.runtime.post-admission-ocr-failure.v1",
            request_id,
            correlation_id,
            instance_id: self.token.instance_id(),
            lease_id: self.token.lease_id(),
            task_id: self.task_id.transport(),
            run_id: self.run_id.transport(),
            frame_id: frame_id.transport(),
            failure_code,
            detail,
            detail_utf8_bytes: detail.len(),
            detail_sha256: format!("{:x}", Sha256::digest(detail.as_bytes())),
        };
        let bytes = serde_json::to_vec(&diagnostic).map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                HostCode::ContainedTaskPostAdmissionOcrFailureEvidenceEncodeFailed,
                ContractLocation::RunContainedTask,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })?;
        #[cfg(test)]
        if self
            .host
            .contained_task_ocr_failure_persistence_failures
            .swap(0, Ordering::AcqRel)
            == 1
        {
            return Err(RequestFailure::poison_without_terminal(
                artifact_store_error(
                    crate::codes::HostLocation::PersistContainedTaskPostAdmissionOcrFailure,
                ),
            ));
        }
        self.persist_post_admission_ocr_diagnostic(frame_id, &bytes, false, CapacityUse::Drain)
    }

    fn record_post_admission_ocr_observation(
        &mut self,
        frame_index: u32,
        observation: PostAdmissionOcrObservation,
    ) -> Result<(), RequestFailure> {
        let frame_id = self.last_frame_id.ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                HostCode::ContainedTaskPostAdmissionOcrFrameMissing,
                ContractLocation::RunContainedTask,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })?;
        if !self.expects_post_admission_ocr
            || self.post_admission_ocr_comparison_recorded
            || frame_index != self.post_admission_ocr_observations
        {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    HostCode::ContainedTaskPostAdmissionOcrStateInvalid,
                    ContractLocation::RunContainedTask,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            ));
        }
        let frame_artifact = self
            .capture_evidence
            .frames
            .last()
            .filter(|frame| frame.artifact.frame_id() == Some(frame_id.transport()))
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    HostCode::ContainedTaskPostAdmissionOcrFrameArtifactMissing,
                    ContractLocation::RunContainedTask,
                    RuntimeErrorCode::RuntimeHalted,
                ))
            })?;
        let diagnostic = RuntimeContainedTaskOcrObservationDiagnostic {
            schema_version: "actingcommand.runtime.post-admission-ocr-observation.v1",
            task_id: self.task_id.transport(),
            run_id: self.run_id.transport(),
            frame_id: frame_id.transport(),
            frame_index,
            frame_artifact: &frame_artifact.artifact,
            observation: &observation,
        };
        let bytes = serde_json::to_vec(&diagnostic).map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                HostCode::ContainedTaskPostAdmissionOcrEvidenceEncodeFailed,
                ContractLocation::RunContainedTask,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })?;
        self.persist_post_admission_ocr_diagnostic(
            frame_id,
            &bytes,
            observation.contains_personal_fields(),
            CapacityUse::Business,
        )?;
        self.post_admission_ocr_observations = self
            .post_admission_ocr_observations
            .checked_add(1)
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    HostCode::ContainedTaskPostAdmissionOcrCountOverflow,
                    ContractLocation::RunContainedTask,
                    RuntimeErrorCode::RuntimeHalted,
                ))
            })?;
        Ok(())
    }

    fn record_post_admission_ocr_comparison<T: serde::Serialize>(
        &mut self,
        report: T,
        frames_collected: u32,
        outcome_key: &str,
        personal: bool,
    ) -> Result<(), RequestFailure> {
        let frame_id = self.last_frame_id.ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                HostCode::ContainedTaskPostAdmissionOcrFrameMissing,
                ContractLocation::RunContainedTask,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })?;
        if !self.expects_post_admission_ocr
            || self.post_admission_ocr_comparison_recorded
            || self.post_admission_ocr_observations == 0
            || frames_collected != self.post_admission_ocr_observations
            || outcome_key.trim().is_empty()
        {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    HostCode::ContainedTaskPostAdmissionOcrComparisonInvalid,
                    ContractLocation::RunContainedTask,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            ));
        }
        let diagnostic = RuntimeContainedTaskOcrComparisonDiagnostic {
            schema_version: "actingcommand.runtime.post-admission-ocr-comparison-envelope.v1",
            task_id: self.task_id.transport(),
            run_id: self.run_id.transport(),
            final_frame_id: frame_id.transport(),
            report: &report,
        };
        let bytes = serde_json::to_vec(&diagnostic).map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                HostCode::ContainedTaskPostAdmissionOcrEvidenceEncodeFailed,
                ContractLocation::RunContainedTask,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })?;
        self.persist_post_admission_ocr_diagnostic(
            frame_id,
            &bytes,
            personal,
            CapacityUse::Business,
        )?;
        self.post_admission_ocr_comparison_recorded = true;
        Ok(())
    }

    fn record_stability_baseline(
        &mut self,
        step_index: u32,
        operation_label: String,
        declaration: StabilityTerminationDeclaration,
    ) -> Result<(), RequestFailure> {
        if self.stability.is_some()
            || self.expected_stability_declaration.as_ref() != Some(&declaration)
            || step_index != self.step_index_offset
            || declaration.consecutive_unchanged_threshold == 0
            || declaration.consecutive_unchanged_threshold >= declaration.max_steps
            || declaration.region.width == 0
            || declaration.region.height == 0
            || declaration
                .region
                .x
                .checked_add(declaration.region.width)
                .is_none()
            || declaration
                .region
                .y
                .checked_add(declaration.region.height)
                .is_none()
        {
            return Err(contained_task_stability_failure(
                HostCode::ContainedTaskStabilityBaselineInvalid,
            ));
        }
        contained_task_step_action(&self.step_actions, step_index, &operation_label)?;
        let current_frame_id = self.last_frame_id.ok_or_else(|| {
            contained_task_stability_failure(HostCode::ContainedTaskStabilityFrameIdentityMissing)
        })?;
        self.stability = Some(RuntimeContainedTaskStability {
            declaration,
            previous_frame_id: current_frame_id,
            last_step_index: step_index,
            consecutive_unchanged: 0,
            pending_terminal: None,
            terminal_recorded: false,
        });
        Ok(())
    }

    fn record_stability_comparison(
        &mut self,
        comparison: RuntimeContainedTaskStabilityComparison,
    ) -> Result<(), RequestFailure> {
        let RuntimeContainedTaskStabilityComparison {
            step_index,
            operation_label,
            declaration,
            result,
            prior_consecutive_unchanged,
            new_consecutive_unchanged,
            terminal_reason,
        } = comparison;
        let action_id =
            contained_task_step_action(&self.step_actions, step_index, &operation_label)?;
        let state = self.stability.as_ref().ok_or_else(|| {
            contained_task_stability_failure(HostCode::ContainedTaskStabilityBaselineMissing)
        })?;
        let current_frame_id =
            contained_task_stability_current_frame(state.previous_frame_id, self.last_frame_id)?;
        let expected_step_index = state.last_step_index.checked_add(1).ok_or_else(|| {
            contained_task_stability_failure(HostCode::ContainedTaskStabilityStepOverflow)
        })?;
        if state.terminal_recorded
            || state.pending_terminal.is_some()
            || state.declaration != declaration
            || step_index != expected_step_index
            || prior_consecutive_unchanged != state.consecutive_unchanged
        {
            return Err(contained_task_stability_failure(
                HostCode::ContainedTaskStabilityComparisonInvalid,
            ));
        }
        let expected_count = match result {
            StabilityComparisonResult::Changed => 0,
            StabilityComparisonResult::Unchanged => {
                prior_consecutive_unchanged.checked_add(1).ok_or_else(|| {
                    contained_task_stability_failure(HostCode::ContainedTaskStabilityCountOverflow)
                })?
            }
        };
        if new_consecutive_unchanged != expected_count
            || new_consecutive_unchanged > declaration.consecutive_unchanged_threshold
        {
            return Err(contained_task_stability_failure(
                HostCode::ContainedTaskStabilityCountInvalid,
            ));
        }
        let completed_steps = step_index
            .checked_sub(self.step_index_offset)
            .and_then(|relative| relative.checked_add(1))
            .ok_or_else(|| {
                contained_task_stability_failure(HostCode::ContainedTaskStabilityStepOverflow)
            })?;
        if completed_steps > declaration.max_steps {
            return Err(contained_task_stability_failure(
                HostCode::ContainedTaskStabilityStepInvalid,
            ));
        }
        let expected_terminal_reason =
            if new_consecutive_unchanged == declaration.consecutive_unchanged_threshold {
                Some(StabilityTerminalReason::ConsecutiveUnchangedThresholdReached)
            } else if completed_steps == declaration.max_steps {
                Some(StabilityTerminalReason::MaxStepsReached)
            } else {
                None
            };
        if terminal_reason != expected_terminal_reason {
            return Err(contained_task_stability_failure(
                HostCode::ContainedTaskStabilityTerminalReasonInvalid,
            ));
        }

        let diagnostic = RuntimeContainedTaskStabilityDiagnostic {
            schema_version: "actingcommand.runtime.contained-task-stability-comparison.v1",
            task_id: self.task_id.transport(),
            run_id: self.run_id.transport(),
            action_id: action_id.transport(),
            step_index,
            operation_label: &operation_label,
            previous_frame_id: state.previous_frame_id.transport(),
            current_frame_id: current_frame_id.transport(),
            region: &declaration.region,
            comparison_mode: declaration.comparison.mode,
            comparison_parameters: &declaration.comparison.parameters,
            result,
            prior_consecutive_unchanged,
            new_consecutive_unchanged,
            consecutive_unchanged_threshold: declaration.consecutive_unchanged_threshold,
            max_steps: declaration.max_steps,
            terminal_reason,
        };
        let bytes = serde_json::to_vec(&diagnostic).map_err(|_| {
            contained_task_stability_failure(HostCode::ContainedTaskStabilityEvidenceEncodeFailed)
        })?;
        let event_links = self
            .links()
            .with_frame_id(current_frame_id)
            .with_action_id(action_id);
        let mut write_context = ArtifactWriteContext::new(
            self.request
                .task_artifact_links(self.run_id)
                .with_frame_id(current_frame_id),
            event_links,
            unix_ms_now().map_err(RequestFailure::poison_without_terminal)?,
        );
        if terminal_reason.is_some() {
            write_context = write_context.for_drain();
        }
        #[cfg(test)]
        let persistence_failure = self
            .host
            .contained_task_stability_persistence_failures
            .swap(0, Ordering::AcqRel);
        #[cfg(test)]
        if persistence_failure == 1 {
            return Err(RequestFailure::poison_without_terminal(
                artifact_store_error(crate::codes::test_spellings::TestLocation::PersistContainedTaskStabilityComparison),
            ));
        }
        let write_request = ArtifactWriteRequest::new(
            ArtifactKind::DiagnosticJson,
            &bytes,
            write_context,
            ArtifactIssuePolicy::new(
                ArtifactProducer::CapturePipeline,
                RetentionClass::DebugFull,
                ArtifactRedactionState::NotRequired,
            ),
        );
        #[cfg(test)]
        let stored = if persistence_failure == 2 {
            let mut sink = FailingContainedTaskStabilityEventSink;
            self.host.artifacts.put(write_request, &mut sink)
        } else {
            let mut sink = RuntimeArtifactEventSink {
                ledger: &self.host.ledger,
                events: &self.host.events,
            };
            self.host.artifacts.put(write_request, &mut sink)
        };
        #[cfg(not(test))]
        let stored = {
            let mut sink = RuntimeArtifactEventSink {
                ledger: &self.host.ledger,
                events: &self.host.events,
            };
            self.host.artifacts.put(write_request, &mut sink)
        };
        stored.map_err(online_observation::observation_artifact_failure)?;

        let state = self.stability.as_mut().ok_or_else(|| {
            contained_task_stability_failure(HostCode::ContainedTaskStabilityStateMissing)
        })?;
        state.previous_frame_id = current_frame_id;
        state.last_step_index = step_index;
        state.consecutive_unchanged = new_consecutive_unchanged;
        state.pending_terminal =
            terminal_reason.map(|reason| RuntimeContainedTaskStabilityTerminal {
                step_index,
                operation_label,
                reason,
            });
        Ok(())
    }

    fn record_stability_terminal(
        &mut self,
        step_index: u32,
        operation_label: String,
        reason: StabilityTerminalReason,
    ) -> Result<(), RequestFailure> {
        let state = self.stability.as_mut().ok_or_else(|| {
            contained_task_stability_failure(HostCode::ContainedTaskStabilityBaselineMissing)
        })?;
        let pending = state.pending_terminal.as_ref().ok_or_else(|| {
            contained_task_stability_failure(HostCode::ContainedTaskStabilityTerminalMissing)
        })?;
        if state.terminal_recorded
            || pending.step_index != step_index
            || pending.operation_label != operation_label
            || pending.reason != reason
        {
            return Err(contained_task_stability_failure(
                HostCode::ContainedTaskStabilityTerminalMismatch,
            ));
        }
        state.pending_terminal = None;
        state.terminal_recorded = true;
        Ok(())
    }
}

fn bounded_post_admission_ocr_failure_detail<'a>(
    failure_code: &str,
    detail: Option<&'a str>,
) -> RuntimeHostResult<Option<&'a str>> {
    if failure_code != CONTAINED_TASK_POST_ADMISSION_OCR_FAILED {
        return Ok(None);
    }
    let Some(detail) = detail else {
        return Ok(None);
    };
    if detail.len() > MAX_CONTAINED_TASK_OCR_FAILURE_DETAIL_BYTES {
        return Err(RuntimeHostError::fatal(
            HostCode::ContainedTaskPostAdmissionOcrFailureDetailTooLarge,
            HostLocation::PersistContainedTaskPostAdmissionOcrFailure,
            RuntimeErrorCode::RuntimeHalted,
        ));
    }
    Ok(Some(detail))
}

#[cfg(test)]
#[test]
fn post_admission_ocr_failure_detail_gate_is_exact_and_bounded() {
    assert_eq!(
        bounded_post_admission_ocr_failure_detail("another_task_error", Some("detail"))
            .expect("different error is ignored"),
        None
    );
    assert_eq!(
        bounded_post_admission_ocr_failure_detail(CONTAINED_TASK_POST_ADMISSION_OCR_FAILED, None,)
            .expect("missing detail is ignored"),
        None
    );
    assert_eq!(
        bounded_post_admission_ocr_failure_detail(
            CONTAINED_TASK_POST_ADMISSION_OCR_FAILED,
            Some("complete detail"),
        )
        .expect("bounded detail"),
        Some("complete detail")
    );

    let oversized = "x".repeat(MAX_CONTAINED_TASK_OCR_FAILURE_DETAIL_BYTES + 1);
    let error = bounded_post_admission_ocr_failure_detail(
        CONTAINED_TASK_POST_ADMISSION_OCR_FAILED,
        Some(&oversized),
    )
    .expect_err("oversized detail must fail closed");
    assert_eq!(
        error.code(),
        "contained_task_post_admission_ocr_failure_detail_too_large"
    );
    assert!(error.is_fatal());
}

fn contained_task_stability_failure(code: impl Into<Code>) -> RequestFailure {
    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
        code,
        HostLocation::RecordContainedTaskStability,
        RuntimeErrorCode::RuntimeHalted,
    ))
}

fn contained_task_stability_current_frame(
    previous_frame_id: IssuedFrameId,
    current_frame_id: Option<IssuedFrameId>,
) -> Result<IssuedFrameId, RequestFailure> {
    let current_frame_id = current_frame_id.ok_or_else(|| {
        contained_task_stability_failure(HostCode::ContainedTaskStabilityFrameIdentityMissing)
    })?;
    if current_frame_id == previous_frame_id {
        return Err(contained_task_stability_failure(
            HostCode::ContainedTaskStabilityFrameIdentityReused,
        ));
    }
    Ok(current_frame_id)
}

#[cfg(test)]
#[test]
fn contained_task_stability_frame_identity_failures_are_typed_and_closed() {
    let identifiers = actingcommand_contract::IdentifierIssuer::new().expect("identifier issuer");
    let previous = identifiers.mint_frame_id().expect("previous frame");

    for (current, expected_code) in [
        (None, "contained_task_stability_frame_identity_missing"),
        (
            Some(previous),
            "contained_task_stability_frame_identity_reused",
        ),
    ] {
        let failure = contained_task_stability_current_frame(previous, current)
            .expect_err("invalid formal frame binding must fail closed");
        assert!(failure.poison_runtime);
        assert_eq!(failure.error.code(), expected_code);
    }
}

impl ContainedTaskRuntime for RuntimeContainedTask<'_> {
    type Error = RequestFailure;

    /// Workflow #386 SR-1 (scheduling model §2.2): the Runtime samples.
    fn sampling_supported(&self) -> bool {
        true
    }

    /// Workflow #386 SR-1: a cancellation (deadline, client, operator pause) under its own code;
    /// a run never loses its permission while it holds its lease (scheduling model §2.2).
    fn candidate_sampling_checkpoint(&mut self) -> Result<(), Self::Error> {
        self.ensure_active()
    }

    /// Workflow #386 SR-1 (scheduling model §2.1 item 4): the configured base of observe-only
    /// retries (`scheduling.recognition.sample_timeout_retries`), read at each step; the pacing
    /// curve's extra retries join with performance slice P-k.
    fn step_pacing(&mut self) -> Result<StepPacing, Self::Error> {
        let retries = self
            .host
            .sample_timeout_retries()
            .map_err(RequestFailure::poison_without_terminal)?;
        Ok(StepPacing::new(retries))
    }

    /// Workflow #373 NAV-6: the run's shared interrupt pages (`host/standard_pack.rs`).
    fn shared_interrupt(
        &mut self,
        frame: actingcommand_execution_kernel::SharedInterruptFrame<'_>,
    ) -> Result<
        actingcommand_execution_kernel::SharedInterruptVerdict,
        ContainedTaskRunError<Self::Error>,
    > {
        self.offered_shared_interrupt(frame)
    }

    fn record_candidate_evaluation(
        &mut self,
        target_id: &str,
        result: &actingcommand_recognition_pack::RecognitionPackResult<
            actingcommand_recognition_pack::TargetEvaluation,
        >,
        timing: ContainedTaskEvaluationTiming,
    ) -> Result<(), Self::Error> {
        self.task_timing.record_evaluation(timing, None, None);
        self.diagnostic_guard(
            Some(target_id),
            Some(result),
            ContainedTaskGuardEvaluation::CandidateSample,
        )?;
        self.ensure_active()
    }

    fn update_run_progress(&mut self, executed_steps: u32) {
        // Workflow #373 NAV-6: beside the steps of the interrupt layers that ran.
        let shift = if self.interrupt_layer {
            0
        } else {
            self.step_shift
        };
        self.executed_steps = self
            .step_index_offset
            .checked_add(executed_steps)
            .and_then(|steps| steps.checked_add(shift));
    }

    fn observe_task_timing(&mut self, context: ContainedTaskTimingContext) {
        self.task_timing.begin_execution(context);
    }

    fn task_boundary_identity(
        &self,
        boundary: TaskTimingBoundary,
    ) -> task_timing::BoundaryIdentity {
        self.timing_identity(boundary)
    }

    fn observe_task_boundary(
        &mut self,
        timing: actingcommand_execution_kernel::ContainedTaskBoundaryTiming,
    ) {
        self.task_timing.kernel_boundary(timing);
    }

    fn record_page_evaluations(
        &mut self,
        phase: &'static str,
        results: &actingcommand_page_detector::PageBatchResult,
        timing: Option<ContainedTaskEvaluationTiming>,
    ) -> Result<(), Self::Error> {
        if let Some(timing) = timing {
            self.task_timing.record_evaluation(
                timing,
                self.last_frame_id.map(|id| *id.transport()),
                self.current_recognition_id.map(|id| *id.transport()),
            );
        }
        self.diagnostic_pages(phase, results)?;
        self.record_capture_recognition(results)
    }
    fn record_guard_evaluation(
        &mut self,
        target: Option<&str>,
        result: Option<
            &actingcommand_recognition_pack::RecognitionPackResult<
                actingcommand_recognition_pack::TargetEvaluation,
            >,
        >,
        evaluation: ContainedTaskGuardEvaluation,
    ) -> Result<(), Self::Error> {
        self.diagnostic_guard(target, result, evaluation)
    }
    fn record_ocr_evaluation(
        &mut self,
        target: &str,
        result: &actingcommand_recognition_pack::RecognitionPackResult<
            actingcommand_recognition_pack::OcrObservationEvaluation,
        >,
    ) -> Result<(), Self::Error> {
        self.diagnostic_ocr(target, result)
    }

    /// Workflow #384 D2-S2 (model §3.5): the configured join timeout
    /// (`scheduling.fact_refresh.recognition_join_timeout_ms`), read when the run joins.
    fn reading_join_timeout(&mut self) -> Result<Option<std::time::Duration>, Self::Error> {
        self.host
            .recognition_join_timeout()
            .map(Some)
            .map_err(RequestFailure::poison_without_terminal)
    }

    /// Workflow #384 D2-S2: a joined reading's evaluation, recorded against the frame it was
    /// read on.
    fn record_reading_evaluation(
        &mut self,
        captured_at: SystemTime,
        target_id: &str,
        result: &actingcommand_recognition_pack::RecognitionPackResult<
            actingcommand_recognition_pack::OcrObservationEvaluation,
        >,
    ) -> Result<(), Self::Error> {
        let frame_id = self.joined_reading_frame(captured_at)?;
        self.diagnostic_frame = Some(frame_id);
        let recorded = self.diagnostic_ocr(target_id, result);
        self.diagnostic_frame = None;
        recorded
    }

    /// Workflow #384 D2-S3: a joined adjustment amount's evaluation, recorded against the frame
    /// its operation was confirmed on.
    fn record_adjustment_evaluation(
        &mut self,
        captured_at: SystemTime,
        target_id: &str,
        result: &actingcommand_recognition_pack::RecognitionPackResult<
            actingcommand_recognition_pack::OcrObservationEvaluation,
        >,
    ) -> Result<(), Self::Error> {
        let frame_id = self.joined_adjustment_frame(captured_at)?;
        self.diagnostic_frame = Some(frame_id);
        let recorded = self.diagnostic_ocr(target_id, result);
        self.diagnostic_frame = None;
        recorded
    }

    fn classify_error(error: &Self::Error) -> ContainedTaskRuntimeErrorClass {
        if error.error.is_fatal() {
            ContainedTaskRuntimeErrorClass::Fatal
        } else {
            ContainedTaskRuntimeErrorClass::Nonfatal
        }
    }

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        use actingcommand_contract::TaskTimingBoundary as Boundary;
        let capture_started = self
            .task_timing
            .begin_boundary(Boundary::Capture, self.timing_identity(Boundary::Capture));
        let result = (|| {
            let active_started = self.task_timing.begin_boundary(
                Boundary::CaptureActivePressure,
                self.timing_identity(Boundary::CaptureActivePressure),
            );
            let active = (|| {
                self.ensure_active()?;
                self.poll_capture_pressure()
            })();
            self.task_timing
                .finish_boundary(active_started, active.is_ok());
            active?;
            self.geometry_deadline = Some(self.geometry_operation_deadline()?);
            let instance_guard = self.host.instance_guard(self.token.instance_id())?;
            let admission = lock(&instance_guard, HostLocation::LockInstanceAdmission)?;
            let frame_id =
                self.host.events.issuer().mint_frame_id().map_err(|_| {
                    RequestFailure::poison_without_terminal(runtime_identifier_error())
                })?;
            let input_action_id = self.post_input_action_id.take();
            // #398 R1 §5.2 (RD-3i): after an input the frame must be newer than its completion;
            // otherwise newer than the frame delivered last.
            let floor = match (self.pending_input_floor.take(), self.last_frame_sequence) {
                (None, None) => None,
                (after, after_sequence) => Some(actingcommand_device::CaptureFloor {
                    after,
                    after_sequence,
                }),
            };
            let links = RuntimeRunLinks::new(self.task_id, self.run_id)
                .apply(self.host.events.request_links(
                    self.request,
                    Some(self.token.instance_id()),
                    Some(self.token.lease_id()),
                    input_action_id,
                ))
                .with_frame_id(frame_id);
            let (source, module) = self.capture_origin();
            let requested = self.host.append_event(
                EventSeverity::Info,
                source,
                module,
                EventActor::Runtime,
                links.clone(),
                CapturePayloadDraft::requested(EventAction::CaptureObserve, AuditInput::new()),
            )?;
            if self.capture_evidence.pipeline.is_none()
                && self.capture_evidence.frame_store.is_none()
            {
                self.capture_evidence.frame_store = Some(
                    actingcommand_artifact_store::FrameStore::new(
                        frame_retention::spill_root(
                            self.host.artifacts.root(),
                            self.run_id.transport(),
                        )
                        .map_err(online_observation::observation_artifact_failure)?,
                        frame_retention::capture_frame_store_config(),
                    )
                    .map_err(online_observation::observation_artifact_failure)?,
                );
            }
            let memory = match &self.capture_evidence.pipeline {
                Some(pipeline) => pipeline.memory_budget(),
                None => self
                    .capture_evidence
                    .frame_store
                    .as_ref()
                    .expect("capture memory owner prepared")
                    .memory_budget(),
            };
            let registration = self.host.mark_resources_in_use()?;
            let identity = task_timing::BoundaryIdentity {
                frame_id: Some(*frame_id.transport()),
                ..self.timing_identity(Boundary::CaptureBackend)
            };
            let backend_started = self
                .task_timing
                .begin_boundary(Boundary::CaptureBackend, identity);
            let captured = self
                .host
                .execution()?
                .capture_frame_retained_with_geometry_session_and_registration_guard(
                    self.instance_alias,
                    Some(*frame_id.transport()),
                    floor,
                    registration,
                    memory,
                );
            self.task_timing
                .finish_boundary(backend_started, captured.is_ok());
            match captured {
                Ok((mut frame, geometry_session)) => {
                    self.host
                        .append_backend_open_observations(
                            &std::mem::take(&mut frame.backend_open_observations),
                            links.clone(),
                            source,
                            module,
                        )
                        .map_err(RequestFailure::poison_without_terminal)?;
                    let material_started = self
                        .task_timing
                        .begin_boundary(Boundary::CaptureMaterial, identity);
                    let material = (|| {
                        self.ensure_active()?;
                        let frame_index = self.capture_evidence.captured()?;
                        let write_context = ArtifactWriteContext::new(
                            self.request
                                .task_artifact_links(self.run_id)
                                .with_frame_id(frame_id),
                            links,
                            unix_ms_now().map_err(RequestFailure::poison_without_terminal)?,
                        );
                        let mut sink = online_observation::ObservationArtifactSink {
                            ledger: &self.host.ledger,
                            events: &self.host.events,
                            verified: None,
                            frame_retention: Some((
                                self.host.owner_epoch,
                                frame_retention::capture_pin_reason(self.request),
                            )),
                        };
                        let mut events = RuntimeArtifactEventSink {
                            ledger: &self.host.ledger,
                            events: &self.host.events,
                        };
                        let mut publish =
                            |store: &ArtifactStore, input: ArtifactWriteRequest<'_>| {
                                sink.publish_frame(store, input, Some(*frame_id.transport()))
                            };
                        let persistence: ArtifactStoreResult<_> = (|| {
                            if self.capture_evidence.pipeline.is_none() {
                                self.capture_evidence.pipeline =
                                    Some(CapturePipeline::open_with_frame_store(
                                        Arc::clone(&self.host.artifacts),
                                        self.capture_evidence
                                            .frame_store
                                            .take()
                                            .expect("capture memory owner prepared"),
                                        CapturePipelineConfig {
                                            frame_store:
                                                frame_retention::capture_frame_store_config(),
                                            retention_class: RetentionClass::DebugFull,
                                            redaction_state: ArtifactRedactionState::NotRequired,
                                            ..CapturePipelineConfig::default()
                                        },
                                        write_context.clone(),
                                        &mut events,
                                    )?);
                            }
                            let pipeline = self
                                .capture_evidence
                                .pipeline
                                .as_mut()
                                .expect("capture pipeline initialized");
                            pipeline.with_frame_copy(
                                &frame,
                                |pipeline, frame| -> ArtifactStoreResult<_> {
                                    let result = pipeline.record_frame_with_publisher(
                                        FrameStoreFrameInput {
                                            frame_index,
                                            file_name: format!("frame-{frame_index}.png"),
                                            label: if frame_index == 0 {
                                                "initial"
                                            } else if input_action_id.is_some() {
                                                "after-input"
                                            } else {
                                                "capture"
                                            }
                                            .to_owned(),
                                            recognition_state: RecognitionState::Pending,
                                            pinned_reason: self
                                                .finalizing
                                                .map(|_| PinnedFrameReason::Terminal),
                                            frame,
                                        },
                                        write_context.clone(),
                                        &mut events,
                                        Some(&mut publish),
                                    )?;
                                    let reference = pipeline.persist_frame_with_publisher(
                                        frame_index,
                                        &mut events,
                                        Some(&mut publish),
                                    )?;
                                    pipeline.poll_pressure_with_publisher(
                                        &write_context,
                                        &mut events,
                                        Some(&mut publish),
                                    )?;
                                    Ok((reference, result.frame.frame_failures))
                                },
                            )?
                        })();
                        let reference = match persistence {
                            Ok((reference, failures)) => {
                                self.host.record_frame_pressure_failures(
                                    self.capture_evidence
                                        .pipeline
                                        .as_ref()
                                        .expect("capture pipeline initialized"),
                                    &failures,
                                )?;
                                reference
                            }
                            Err(error) => {
                                self.capture_evidence
                                    .pipeline_failure
                                    .get_or_insert(error.clone());
                                let links = self
                                    .capture_evidence
                                    .pipeline
                                    .as_ref()
                                    .and_then(|pipeline| pipeline.failure_context(&error))
                                    .unwrap_or(&write_context)
                                    .event_links()
                                    .clone();
                                return Err(self.host.capture_material_failure(error, links));
                            }
                        };
                        self.capture_evidence.persisted(frame_index, &reference)?;
                        self.last_frame_id = Some(frame_id);
                        self.last_frame_captured_at = Some(frame.captured_at);
                        self.last_capture_input_action_id = input_action_id;
                        self.last_capture_acquire_us = frame.capture_acquire_us();
                        self.last_frame_sequence = frame.stream.map(|stamp| stamp.sequence);
                        if self.configuration_records > 0 && !self.configuration_capture_recorded {
                            let selection = frame.selection.as_ref().map(|selection| {
                                EffectiveCaptureSelection {
                                    requested_backend: selection.requested.as_str().to_owned(),
                                    configured_adb: selection.configured_adb.clone(),
                                    configured_serial: selection.configured_serial.clone(),
                                    resolved_adb: selection.resolved_adb.clone(),
                                    selected_serial: selection.selected_serial.clone(),
                                    mumu: selection.mumu.as_ref().map(|mumu| {
                                        EffectiveMumuInstallation {
                                            root: mumu.root.clone(),
                                            adb_path: mumu.adb_path.clone(),
                                            capture_dll_path: mumu.capture_dll_path.clone(),
                                            source: mumu.source.as_str().to_owned(),
                                        }
                                    }),
                                }
                            });
                            self.record_configuration(
                                EffectiveConfigurationFacts::Capture {
                                    backend: frame.backend_name.as_str().to_owned(),
                                    selection,
                                },
                                Some(frame_id),
                                None,
                                Some(requested.sequence()),
                            )?;
                            self.configuration_capture_recorded = true;
                        }
                        Ok(ObservedFrame {
                            input_reference: Some(actingcommand_contract::InputFrameReference {
                                frame_id: *frame_id.transport(),
                                width: frame.width,
                                height: frame.height,
                            }),
                            frame,
                        })
                    })();
                    self.task_timing
                        .finish_boundary(material_started, material.is_ok());
                    let frame = material?;
                    self.retain_task_geometry(&frame, frame_id, geometry_session)?;
                    Ok(frame)
                }
                Err(error) => {
                    self.host
                        .append_backend_open_failure_observations(
                            &error,
                            links.clone(),
                            source,
                            module,
                        )
                        .map_err(RequestFailure::poison_without_terminal)?;
                    let error = self
                        .host
                        .finish_capture_failure_while_guarded(error, links.clone(), &admission)
                        .map_err(RequestFailure::poison_without_terminal)?;
                    let mut runtime_error =
                        RuntimeHostError::execution(HostLocation::RunContainedTaskCapture, &error);
                    match self.host.settle_unconfirmed_resources(
                        &runtime_error,
                        Some(self.token.instance_id()),
                        links.clone(),
                    )? {
                        UnconfirmedResources::Confirmed => {}
                        // Workflow #403: the instance is fenced; the run fails non-fatally.
                        UnconfirmedResources::Fenced(fence) => {
                            return Err(RequestFailure::fence_without_terminal(fence));
                        }
                        UnconfirmedResources::Retained => {
                            return Err(RequestFailure::poison_without_terminal(runtime_error));
                        }
                    }
                    // #316-P4: a physical instance's first capture that fails while one ADB
                    // baseline probe (task deadline) also fails is the ADB baseline lost, not a
                    // host fault: both ADB facts are invalidated and the task fails nonfatal.
                    // An answering adbd keeps the capture failure fatal, except an operation
                    // failure on a session kept from an earlier lease (Workflow #191 H, below).
                    if self.last_frame_id.is_none()
                        && self.execution_provenance == ExecutionBackendProvenance::PhysicalDevice
                        && runtime_error.is_fatal()
                        && spelled(
                            error.code().as_str(),
                            &[
                                DeviceSelfCheckFailure::CaptureBackendOpenFailed.code(),
                                SharedCode::CaptureBackendOperationFailed.code(),
                            ],
                        )
                    {
                        let host = self.host;
                        let probe = host.execution()?.probe_adb_baseline_until(
                            self.instance_alias,
                            self.geometry_operation_deadline()?,
                            &|| host.fatal.is_shutdown_requested(),
                        );
                        match probe {
                            Err(probe)
                                if probe.resource_quiescence()
                                    != Some(ResourceQuiescence::Unconfirmed) =>
                            {
                                host.invalidate_adb_baseline(self.token.instance_id())?;
                                runtime_error = RuntimeHostError::adb_unreachable_capture(
                                    HostLocation::RunContainedTaskCapture,
                                    &error,
                                );
                            }
                            // Workflow #191 H: the first capture failed on a session kept from
                            // an earlier lease (no open in this capture) while adbd answers: the
                            // idle session went stale. It is closed above; the task fails
                            // nonfatal and the next capture opens a new session.
                            Ok(())
                                if error.code()
                                    == SharedCode::CaptureBackendOperationFailed.code()
                                    && error
                                        .failure_context()
                                        .backend_open_observations()
                                        .is_empty() =>
                            {
                                runtime_error = RuntimeHostError::reused_session_capture(
                                    HostLocation::RunContainedTaskCapture,
                                    &error,
                                );
                            }
                            _ => {}
                        }
                    }
                    let payload = CapturePayloadDraft::failed_with_causes(
                        EventAction::CaptureObserve,
                        DiagnosticCode::CaptureFailed,
                        if spelled(
                            runtime_error.code(),
                            &[
                                SharedCode::CaptureFrameInvalid.code(),
                                SharedCode::FrameWorkspaceUnavailable.code(),
                            ],
                        ) {
                            EffectDisposition::Indeterminate
                        } else {
                            EffectDisposition::NotPerformed
                        },
                        runtime_error.record_outcome(),
                        runtime_error.diagnostic_detail().cloned(),
                        runtime_error.cleanup_cause().cloned(),
                        AuditInput::new(),
                    );
                    let failed = self.host.append_event(
                        EventSeverity::Error,
                        source,
                        module,
                        EventActor::Runtime,
                        links.clone(),
                        payload,
                    )?;
                    self.host
                        .record_required_failure(&runtime_error, &failed, links)?;
                    Err(RequestFailure {
                        state: RuntimeReceiptState::Failed,
                        terminal: Some(terminal(&failed)),
                        poison_runtime: runtime_error.is_fatal(),
                        task_failure: Some(TaskFailureEvidence {
                            outcome: runtime_error.outcome(),
                            severe: runtime_error.is_fatal(),
                        }),
                        error: Box::new(runtime_error),
                    })
                }
            }
        })();
        self.task_timing
            .finish_boundary(capture_started, result.is_ok());
        result
    }

    fn action_seed(
        &mut self,
        step_index: u32,
        operation_label: &str,
    ) -> Result<Option<u64>, Self::Error> {
        let run_seed = require_contained_task_sampling_run_seed(self.sampling_run_seed)
            .map_err(RequestFailure::poison_without_terminal)?;
        self.ensure_active()?;
        let step_index = self.absolute_step_index(step_index)?;
        let action_id =
            contained_task_step_action(&self.step_actions, step_index, operation_label)?;
        let action_seed = contained_task_sampling_seed(&(
            "xorshift64_uniform_rect_v1/action",
            run_seed,
            action_id.transport(),
        ))
        .map_err(RequestFailure::poison_without_terminal)?;
        if !self.used_action_seeds.insert(action_seed) {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    HostCode::ContainedTaskSamplingSeedReused,
                    HostLocation::DeriveContainedTaskActionSeed,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            ));
        }
        Ok(Some(action_seed))
    }

    /// Workflow #308: the instance's fact snapshot and the host clock for one select step. The
    /// context is this instance's alias with the game and server of its configured policy
    /// identity, or of the package's `control.json` when none is configured; the interpreter
    /// refuses a context that differs from the package's. The policy inputs are read and
    /// released before the fact write gate is taken; no policy lock is held.
    fn selection_state(
        &mut self,
        _request: actingcommand_execution_kernel::SelectionStateRequest,
    ) -> Result<actingcommand_execution_kernel::SelectionState, Self::Error> {
        self.ensure_active()?;
        let configured = {
            let inputs = lock(&self.host.policy_inputs, HostLocation::ReadSelectionState)?;
            inputs.as_ref().and_then(|inputs| {
                inputs
                    .instance_identities()
                    .find(|identity| identity.instance_id == self.instance_alias)
                    .map(|identity| (identity.game_id.to_owned(), identity.server_id.to_owned()))
            })
        };
        let (game_id, server_id) = configured.unwrap_or_else(|| self.declared_game_server.clone());
        let failure = |error: RuntimeHostError| {
            if error.is_fatal() {
                RequestFailure::poison_without_terminal(error)
            } else {
                RequestFailure::request(error, RuntimeReceiptState::Failed, None)
            }
        };
        let snapshot = self
            .host
            .instance_fact_snapshot(InstanceFactContext {
                instance_id: self.instance_alias.to_owned(),
                server_id,
                game_id,
            })
            .map_err(failure)?;
        let now_unix_ms = self.host.clock.sample().map_err(failure)?.unix_ms;
        Ok(actingcommand_execution_kernel::SelectionState::Snapshot {
            snapshot,
            now_unix_ms,
        })
    }

    fn committed_input_frame(
        &mut self,
        reference: actingcommand_contract::InputFrameReference,
    ) -> Result<Option<InputFrameContext>, Self::Error> {
        self.host
            .execution()?
            .resolve_input_frame(self.instance_alias, reference)
            .map(Some)
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::execution(
                    HostLocation::ResolveCommittedInputFrame,
                    &error,
                ))
            })
    }

    fn input(
        &mut self,
        action: InputAction,
        frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        let input_started = self.task_timing.begin_boundary(
            actingcommand_contract::TaskTimingBoundary::Input,
            self.timing_identity(actingcommand_contract::TaskTimingBoundary::Input),
        );
        let result = (|| {
            self.ensure_active()?;
            self.geometry_deadline = Some(self.geometry_operation_deadline()?);
            self.require_task_geometry_for_input()?;
            let (success, selection) = self.host.input(
                self.request,
                self.token,
                &action,
                self.connection_id,
                self.execution_provenance,
                RuntimeInputContext {
                    run_links: Some(RuntimeRunLinks::new(self.task_id, self.run_id)),
                    source_step_action_id: self.input_step_action_id.take(),
                    before_frame_id: None,
                    input_frame: frame.map(|frame| frame.reference()),
                    input_control: Some(Arc::clone(&self.control)),
                },
            )?;
            self.ensure_active()?;
            if let RuntimeResult::InputCommitted { action_id } = success.result {
                self.post_input_action_id = Some(action_id);
                self.pending_input_floor = Some(Instant::now());
                self.diagnostic_physical = Some(action_id);
                if self.configuration_records > 0 && !self.configuration_input_recorded {
                    self.record_configuration(
                        EffectiveConfigurationFacts::Input {
                            selection: selection.map(|selection| EffectiveInputSelection {
                                backend: selection.backend.as_str().to_owned(),
                                serial: selection.serial,
                            }),
                        },
                        self.last_frame_id,
                        Some(action_id),
                        success.terminal.map(|terminal| terminal.sequence),
                    )?;
                    self.configuration_input_recorded = true;
                }
                Ok(())
            } else {
                Err(RequestFailure::poison_without_terminal(
                    RuntimeHostError::fatal(
                        HostCode::ContainedTaskInputResultInvalid,
                        ContractLocation::RunContainedTask,
                        RuntimeErrorCode::RuntimeHalted,
                    ),
                ))
            }
        })();
        self.task_timing
            .finish_boundary(input_started, result.is_ok());
        result
    }

    /// The `application` effect of a task step (slice #316-B3): the host's own application
    /// lifecycle path under the run's lease, with the task and run ids on every
    /// `application.*` event. A fixture run has no assigned application and reports
    /// `Unsupported`, which the interpreter turns into the typed task failure.
    fn supports_application_effect(&self) -> bool {
        self.execution_provenance == ExecutionBackendProvenance::PhysicalDevice
    }

    fn control_application(
        &mut self,
        action: ApplicationLifecycleAction,
    ) -> Result<actingcommand_execution_kernel::ApplicationEffectSupport, Self::Error> {
        use actingcommand_execution_kernel::ApplicationEffectSupport;
        self.ensure_active()?;
        if !self.supports_application_effect() {
            return Ok(ApplicationEffectSupport::Unsupported);
        }
        self.host.application_control(
            self.request,
            self.token,
            action,
            self.connection_id,
            Some(RuntimeRunLinks::new(self.task_id, self.run_id)),
        )?;
        self.ensure_active()?;
        Ok(ApplicationEffectSupport::Performed)
    }

    fn record(&mut self, mut trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        // Workflow #308: the interpreter does not return a `run_ending` select record's failure
        // (it returns the run-ending capture error); it is recorded as it happens.
        if let ContainedTaskTrace::SelectionEvaluated { run_ending, .. } = &mut trace
            && std::mem::take(run_ending)
        {
            let recorded = self.record(trace);
            if let Err(failure) = &recorded {
                self.record_unwritten_selection(failure);
            }
            return recorded;
        }
        if !matches!(&trace, ContainedTaskTrace::PackageAdmitted { .. }) {
            self.ensure_active()?;
        }
        let trace = self.offset_trace(trace)?;
        match trace {
            ContainedTaskTrace::PackageAdmitted {
                task_label,
                package_label,
            } => {
                #[cfg(test)]
                self.host
                    .consume_contained_task_checkpoint_for_test(
                        ContainedTaskCheckpointPoint::PackageAdmitted,
                        ContainedTaskCheckpointIdentity {
                            request_id: self.control.request_id,
                            instance_id: self.control.instance_id,
                            lease_id: self.token.lease_id(),
                        },
                    )
                    .map_err(RequestFailure::poison_without_terminal)?;
                self.append_task(
                    EventSeverity::Info,
                    self.links(),
                    TaskPayloadDraft::semantic_with_lease_expiry(
                        TaskSemanticFact::PackageAdmitted {
                            package_label,
                            task_label,
                            response_deadline_monotonic_ms: Some(self.control.deadline()),
                        },
                        self.token.expires_at_monotonic_ms(),
                        AuditInput::new(),
                    ),
                )?;
                let (game, server) = &self.declared_game_server;
                for (key, value) in [(TASK_GAME_FACT_KEY, game), (TASK_SERVER_FACT_KEY, server)] {
                    self.host
                        .record_changed_instance_string_fact(self.control.instance_id, key, value)
                        .map_err(RequestFailure::poison_without_terminal)?;
                }
                Ok(())
            }
            ContainedTaskTrace::RunStarted => self.append_task(
                EventSeverity::Info,
                self.links(),
                TaskPayloadDraft::semantic(TaskSemanticFact::RunStarted, AuditInput::new()),
            ),
            ContainedTaskTrace::EntryRecognition {
                required_pages,
                matched_page,
            } => {
                let matched = matched_page.is_some();
                self.record_entry_fact(TaskSemanticFact::EntryRecognition {
                    phase: TaskEntryRecognitionPhase::Initial,
                    required_pages,
                    matched_page,
                    matched,
                })?;
                if self.entry_preflight_recorded {
                    return if matched {
                        Ok(())
                    } else {
                        self.record_entry_fact(TaskSemanticFact::EntryTargetDisposition {
                            disposition: TaskEntryTargetDisposition::FailClosed,
                            outcome: Some(Outcome::new(
                                SharedCode::ContainedTaskHomeEntryNotMatched,
                            )),
                        })
                    };
                }
                self.record_entry_fact(TaskSemanticFact::EntryRecoveryDecision {
                    required: false,
                })?;
                self.record_entry_fact(TaskSemanticFact::EntryTargetDisposition {
                    disposition: if matched {
                        TaskEntryTargetDisposition::Started
                    } else {
                        TaskEntryTargetDisposition::FailClosed
                    },
                    outcome: (!matched)
                        .then(|| Outcome::new(SharedCode::ContainedTaskHomeEntryNotMatched)),
                })
            }
            ContainedTaskTrace::CaptureCompleted { width, height } => {
                let observed = self.task_timing.begin_boundary(
                    actingcommand_contract::TaskTimingBoundary::CaptureCompletedRecord,
                    self.timing_identity(
                        actingcommand_contract::TaskTimingBoundary::CaptureCompletedRecord,
                    ),
                );
                let result = (|| {
                    let frame_id = self.last_frame_id.ok_or_else(|| {
                        RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                            HostCode::ContainedTaskFrameIdentityMissing,
                            ContractLocation::RunContainedTask,
                            RuntimeErrorCode::RuntimeHalted,
                        ))
                    })?;
                    let input_action_id = self.last_capture_input_action_id.take();
                    let capture_acquire_us = self.last_capture_acquire_us.take();
                    let links = RuntimeRunLinks::new(self.task_id, self.run_id)
                        .apply(self.host.events.request_links(
                            self.request,
                            Some(self.token.instance_id()),
                            Some(self.token.lease_id()),
                            input_action_id,
                        ))
                        .with_frame_id(frame_id);
                    let (source, module) = self.capture_origin();
                    self.host.append_event(
                        EventSeverity::Info,
                        source,
                        module,
                        EventActor::Runtime,
                        links.clone(),
                        CapturePayloadDraft::completed_with_capture_acquire(
                            EventAction::CaptureObserve,
                            EffectDisposition::NotPerformed,
                            width,
                            height,
                            capture_acquire_us,
                            AuditInput::new(),
                        ),
                    )?;
                    self.append_task(
                        EventSeverity::Info,
                        links,
                        TaskPayloadDraft::semantic(
                            TaskSemanticFact::EvidenceIndexed {
                                frame_width: width,
                                frame_height: height,
                            },
                            AuditInput::new(),
                        ),
                    )?;
                    self.host
                        .execution()?
                        .commit_input_frame(
                            self.instance_alias,
                            actingcommand_contract::InputFrameReference {
                                frame_id: *frame_id.transport(),
                                width,
                                height,
                            },
                        )
                        .map(|_| ())
                        .map_err(|error| {
                            RequestFailure::poison_without_terminal(RuntimeHostError::execution(
                                HostLocation::CommitCaptureInputFrame,
                                &error,
                            ))
                        })
                })();
                self.task_timing.finish_boundary(observed, result.is_ok());
                result
            }
            ContainedTaskTrace::RecognitionStarted {
                candidate_pages,
                width,
                height,
            } => {
                let observed = self.task_timing.begin_boundary(
                    actingcommand_contract::TaskTimingBoundary::RecognitionStartedRecord,
                    self.timing_identity(
                        actingcommand_contract::TaskTimingBoundary::RecognitionStartedRecord,
                    ),
                );
                let result = (|| {
                    if self.current_recognition_id.is_some() {
                        return Err(RequestFailure::poison_without_terminal(
                            RuntimeHostError::fatal(
                                HostCode::ContainedTaskRecognitionStateInvalid,
                                ContractLocation::RunContainedTask,
                                RuntimeErrorCode::RuntimeHalted,
                            ),
                        ));
                    }
                    self.capture_evidence
                        .pin_last(PinnedFrameReason::RecognitionEvidence)?;
                    let frame_id = self.last_frame_id.ok_or_else(|| {
                        RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                            HostCode::ContainedTaskFrameIdentityMissing,
                            ContractLocation::RunContainedTask,
                            RuntimeErrorCode::RuntimeHalted,
                        ))
                    })?;
                    let recognition_id =
                        self.host
                            .events
                            .issuer()
                            .mint_recognition_id()
                            .map_err(|_| {
                                RequestFailure::poison_without_terminal(runtime_identifier_error())
                            })?;
                    let links = self
                        .links()
                        .with_frame_id(frame_id)
                        .with_recognition_id(recognition_id);
                    self.host.append_event(
                        EventSeverity::Info,
                        EventSource::Runtime,
                        OriginModule::Recognition,
                        EventActor::Runtime,
                        links.clone(),
                        RecognitionPayloadDraft::requested(
                            EventAction::RecognitionObserve,
                            AuditInput::new(),
                        ),
                    )?;
                    self.append_task(
                        EventSeverity::Info,
                        links,
                        TaskPayloadDraft::semantic(
                            TaskSemanticFact::RecognitionStarted {
                                candidate_pages,
                                frame_width: width,
                                frame_height: height,
                            },
                            AuditInput::new(),
                        ),
                    )?;
                    self.current_recognition_id = Some(recognition_id);
                    Ok(())
                })();
                self.task_timing.finish_boundary(observed, result.is_ok());
                result
            }
            ContainedTaskTrace::RecognitionCompleted {
                candidate_pages,
                page_label,
                width,
                height,
                targets,
            } => {
                let observed_frame_id = self.last_frame_id.map(|id| *id.transport());
                let observed_recognition_id = self.current_recognition_id.map(|id| *id.transport());
                let started = Instant::now();
                let budget_before = self.task_timing.budget_at(started);
                let result = (|| {
                    let frame_id = self.last_frame_id.ok_or_else(|| {
                        RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                            HostCode::ContainedTaskFrameIdentityMissing,
                            ContractLocation::RunContainedTask,
                            RuntimeErrorCode::RuntimeHalted,
                        ))
                    })?;
                    let recognition_id = self.current_recognition_id.ok_or_else(|| {
                        RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                            HostCode::ContainedTaskRecognitionIdentityMissing,
                            ContractLocation::RunContainedTask,
                            RuntimeErrorCode::RuntimeHalted,
                        ))
                    })?;
                    let links = self
                        .links()
                        .with_frame_id(frame_id)
                        .with_recognition_id(recognition_id);
                    let identity = self.timing_identity(
                        actingcommand_contract::TaskTimingBoundary::RecognitionPayloadAppend,
                    );
                    let append_started = self
                        .task_timing
                        .begin_append(task_timing::TaskAppend::RecognitionPayload, identity);
                    let (appended, observation) = self.host.append_event_observed(
                        EventSeverity::Info,
                        EventSource::Runtime,
                        OriginModule::Recognition,
                        EventActor::Runtime,
                        links.clone(),
                        RecognitionPayloadDraft::completed(
                            EventAction::RecognitionObserve,
                            EffectDisposition::NotPerformed,
                            width,
                            height,
                            if page_label.is_some() {
                                RecognitionVerdict::PageMatched
                            } else {
                                RecognitionVerdict::PageUnmatched
                            },
                            AuditInput::new(),
                        ),
                    );
                    self.task_timing
                        .finish_append(append_started, appended.is_ok(), observation);
                    appended?;
                    let append_started = self
                        .task_timing
                        .begin_append(task_timing::TaskAppend::RecognitionTask, identity);
                    let (appended, observation) = self.host.append_event_observed(
                        EventSeverity::Info,
                        EventSource::Runtime,
                        OriginModule::Runtime,
                        EventActor::Runtime,
                        links,
                        TaskPayloadDraft::semantic(
                            TaskSemanticFact::RecognitionCompleted {
                                candidate_pages,
                                matched_page: page_label.clone(),
                                frame_width: width,
                                frame_height: height,
                                targets,
                            },
                            AuditInput::new(),
                        ),
                    );
                    let appended = appended.map(|_| ());
                    self.task_timing
                        .finish_append(append_started, appended.is_ok(), observation);
                    appended?;
                    self.current_recognition_id = None;
                    if let Some(page) = &page_label {
                        self.host
                            .record_changed_instance_string_fact(
                                self.control.instance_id,
                                TASK_PAGE_FACT_KEY,
                                page,
                            )
                            .map_err(RequestFailure::poison_without_terminal)?;
                    }
                    Ok(())
                })();
                let elapsed_us =
                    actingcommand_execution_kernel::observe_instant_span(started, Instant::now());
                self.task_timing.recognition_completed_record(
                    actingcommand_contract::TaskTimingSample {
                        elapsed_us,
                        budget_before,
                        result: if result.is_ok() {
                            actingcommand_contract::TaskTimingResult::Ok
                        } else {
                            actingcommand_contract::TaskTimingResult::Err
                        },
                        record_index: None,
                        frame_id: observed_frame_id,
                        recognition_id: observed_recognition_id,
                    },
                );
                result
            }
            ContainedTaskTrace::StepStarted {
                step_index,
                operation_label,
                from_page,
                phase,
            } => {
                let diagnostic_started = self
                    .host
                    .monotonic_ms()
                    .map_err(RequestFailure::poison_without_terminal)?;
                self.capture_evidence
                    .pin_last(PinnedFrameReason::PreInput)?;
                let action_id = self.host.events.issuer().mint_action_id().map_err(|_| {
                    RequestFailure::poison_without_terminal(runtime_identifier_error())
                })?;
                // Workflow #373 NAV-6: an interrupt layer's steps follow every step so far.
                self.next_free_step_index =
                    self.next_free_step_index.max(step_index.saturating_add(1));
                if self
                    .step_actions
                    .insert(step_index, (action_id, operation_label.clone()))
                    .is_some()
                {
                    return Err(RequestFailure::poison_without_terminal(
                        RuntimeHostError::fatal(
                            HostCode::ContainedTaskStepIdentityReused,
                            ContractLocation::RunContainedTask,
                            RuntimeErrorCode::RuntimeHalted,
                        ),
                    ));
                }
                self.append_task(
                    EventSeverity::Info,
                    self.links().with_action_id(action_id),
                    TaskPayloadDraft::semantic(
                        TaskSemanticFact::StepStarted {
                            step_index,
                            operation_label,
                            from_page,
                            phase,
                        },
                        AuditInput::new(),
                    ),
                )?;
                self.begin_diagnostic_step(step_index, *action_id.transport(), diagnostic_started)
            }
            // Workflow #308: one select attempt's decision, appended before any input or
            // failure return, linked to its step and to the last frame it read.
            ContainedTaskTrace::SelectionEvaluated {
                step_index,
                operation_label,
                selection,
                run_ending: _,
            } => {
                // The interpreter validates a record it can still fail the task with; a
                // `run_ending` record reaches this check unvalidated.
                selection.validate_for_append().map_err(|error| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        error.code(),
                        HostLocation::RecordContainedTaskSelection,
                        RuntimeErrorCode::RuntimeHalted,
                    ))
                })?;
                let action_id =
                    contained_task_step_action(&self.step_actions, step_index, &operation_label)?;
                let mut links = self.links().with_action_id(action_id);
                if let Some(frame_id) = self.last_frame_id {
                    links = links.with_frame_id(frame_id);
                }
                self.append_task(
                    EventSeverity::Info,
                    links,
                    TaskPayloadDraft::semantic(
                        TaskSemanticFact::SelectionEvaluated {
                            step_index,
                            operation_label,
                            selection,
                        },
                        AuditInput::new(),
                    ),
                )
            }
            ContainedTaskTrace::EffectIntent {
                step_index,
                operation_label,
                action,
                sampling,
                guard: _,
            } => {
                action.validate().map_err(|_| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        HostCode::ContainedTaskEffectInvalid,
                        ContractLocation::RunContainedTask,
                        RuntimeErrorCode::RuntimeHalted,
                    ))
                })?;
                let action_id =
                    contained_task_step_action(&self.step_actions, step_index, &operation_label)?;
                let frame_extent = match (&self.geometry_frame, self.last_frame_id) {
                    (Some(frame), Some(frame_id)) => {
                        if frame.frame_id != *frame_id.transport() {
                            return Err(RequestFailure::poison_without_terminal(
                                RuntimeHostError::fatal(
                                    HostCode::ContainedTaskEffectFrameIdentityMismatch,
                                    ContractLocation::RunContainedTask,
                                    RuntimeErrorCode::RuntimeHalted,
                                ),
                            ));
                        }
                        Some(frame.extent)
                    }
                    _ => None,
                };
                let fact = TaskSemanticFact::EffectIntent {
                    step_index,
                    operation_label,
                    action,
                    frame_extent,
                };
                let payload = match sampling {
                    Some(sampling) => {
                        TaskPayloadDraft::semantic_with_sampling(fact, sampling, AuditInput::new())
                    }
                    None => TaskPayloadDraft::semantic(fact, AuditInput::new()),
                };
                let mut links = self.links().with_action_id(action_id);
                if let Some(frame_id) = self.last_frame_id {
                    links = links.with_frame_id(frame_id);
                }
                self.append_task(EventSeverity::Info, links, payload)?;
                self.input_step_action_id = Some(*action_id.transport());
                Ok(())
            }
            ContainedTaskTrace::EffectCompleted {
                step_index,
                operation_label,
            } => {
                let action_id =
                    contained_task_step_action(&self.step_actions, step_index, &operation_label)?;
                let identity = task_timing::BoundaryIdentity {
                    step_index: Some(step_index),
                    action_id: Some(*action_id.transport()),
                    ..self.timing_identity(TaskTimingBoundary::EffectCompletedAppend)
                };
                let append_started = self
                    .task_timing
                    .begin_append(task_timing::TaskAppend::EffectCompleted, identity);
                let (appended, observation) = self.host.append_event_observed(
                    EventSeverity::Info,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    self.links().with_action_id(action_id),
                    TaskPayloadDraft::semantic(
                        TaskSemanticFact::EffectCompleted {
                            step_index,
                            operation_label,
                        },
                        AuditInput::new(),
                    ),
                );
                let appended = appended.map(|_| ());
                self.task_timing
                    .finish_append(append_started, appended.is_ok(), observation);
                appended?;
                self.capture_evidence.effect_completed()
            }
            ContainedTaskTrace::StepFinished {
                step_index,
                operation_label,
                page_label,
                phase,
            } => {
                let diagnostic_ended = self
                    .host
                    .monotonic_ms()
                    .map_err(RequestFailure::poison_without_terminal)?;
                self.input_step_action_id = None;
                self.post_input_action_id = None;
                contained_task_step_action(&self.step_actions, step_index, &operation_label)?;
                let (action_id, _) = self.step_actions.remove(&step_index).ok_or_else(|| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        HostCode::ContainedTaskStepIdentityMissing,
                        ContractLocation::RunContainedTask,
                        RuntimeErrorCode::RuntimeHalted,
                    ))
                })?;
                let finished_operation = operation_label.clone();
                let finished = self.host.append_event(
                    EventSeverity::Info,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    self.links().with_action_id(action_id),
                    TaskPayloadDraft::semantic(
                        TaskSemanticFact::StepFinished {
                            step_index,
                            operation_label,
                            page_label,
                            phase,
                        },
                        AuditInput::new(),
                    ),
                )?;
                // Workflow #384 D2-S3: the step a confirmation of adjustments follows.
                self.last_step_finished = Some((finished_operation, finished.sequence()));
                self.end_diagnostic_step(diagnostic_ended, true)?;
                self.diagnostic_physical = None;
                Ok(())
            }
            ContainedTaskTrace::StabilityBaseline {
                step_index,
                operation_label,
                declaration,
            } => self.record_stability_baseline(step_index, operation_label, declaration),
            ContainedTaskTrace::StabilityComparison {
                step_index,
                operation_label,
                declaration,
                result,
                prior_consecutive_unchanged,
                new_consecutive_unchanged,
                terminal_reason,
            } => self.record_stability_comparison(RuntimeContainedTaskStabilityComparison {
                step_index,
                operation_label,
                declaration,
                result,
                prior_consecutive_unchanged,
                new_consecutive_unchanged,
                terminal_reason,
            }),
            ContainedTaskTrace::StabilityTerminal {
                step_index,
                operation_label,
                reason,
            } => self.record_stability_terminal(step_index, operation_label, reason),
            ContainedTaskTrace::PostAdmissionOcrObservation {
                frame_index,
                observation,
            } => self.record_post_admission_ocr_observation(frame_index, observation),
            ContainedTaskTrace::PostAdmissionOcrComparison { report } => {
                let frames = report.frames_collected();
                let outcome = report.outcome_key().to_string();
                self.record_post_admission_ocr_comparison(report, frames, &outcome, false)
            }
            ContainedTaskTrace::PostAdmissionOcrFields { report } => {
                let frames = report.frames_collected;
                let outcome = report.declaration.outcome_key.clone();
                let personal = report
                    .declaration
                    .fields
                    .iter()
                    .any(|f| f.privacy == actingcommand_contract::OcrFieldPrivacy::Personal);
                self.record_post_admission_ocr_comparison(report, frames, &outcome, personal)
            }
            // Workflow #384 D2-S2: a frame handed to recognition is the last captured frame,
            // handed off before Finalizing and before the run joins any.
            ContainedTaskTrace::ResourceReadingFrame { captured_at } => {
                if self.finalizing.is_some() || self.joined_reading_frames != 0 {
                    return Err(resource_reading_poison(
                        HostCode::ContainedTaskResourceReadingDuplicate,
                    ));
                }
                let (Some(frame_id), Some(frame_captured_at)) =
                    (self.last_frame_id, self.last_frame_captured_at)
                else {
                    return Err(resource_reading_poison(
                        HostCode::ContainedTaskResourceReadingFrameMissing,
                    ));
                };
                if frame_captured_at != captured_at {
                    return Err(resource_reading_poison(
                        HostCode::ContainedTaskResourceReadingFrameMismatch,
                    ));
                }
                self.reading_frames.push((captured_at, frame_id));
                Ok(())
            }
            // Workflow #335 S5b, #384 D2-S2: the readings of the next handed-off frame, joined
            // before Finalizing in hand-off order; the terminal publishes those that held. Each
            // mark that gave no reading is a warning row of the task diagnostic, on its frame.
            ContainedTaskTrace::ResourceReadings {
                captured_at,
                readings,
                unresolved,
            } => {
                if self.finalizing.is_some() {
                    return Err(resource_reading_poison(
                        HostCode::ContainedTaskResourceReadingDuplicate,
                    ));
                }
                let frame_id = self.joined_reading_frame(captured_at)?;
                self.joined_reading_frames += 1;
                let recorded = unresolved
                    .iter()
                    .try_for_each(|miss| self.diagnostic_reading_miss(frame_id, miss));
                self.diagnostic_frame = None;
                recorded?;
                if !readings.is_empty() {
                    self.taken_resource_readings.push(JoinedReadingFrame {
                        captured_at,
                        frame_id,
                        readings,
                    });
                }
                Ok(())
            }
            // Workflow #384 D2-S3: an operation that declares adjustments was confirmed on the
            // frame captured last, right after its step finished; before Finalizing and before
            // the run joins anything.
            ContainedTaskTrace::ResourceAdjustmentConfirmed {
                captured_at,
                operation_label,
            } => {
                if self.finalizing.is_some()
                    || self.joined_reading_frames != 0
                    || self.joined_adjustment_confirmations != 0
                {
                    return Err(resource_reading_poison(
                        HostCode::ContainedTaskResourceReadingDuplicate,
                    ));
                }
                let (Some(frame_id), Some(frame_captured_at)) =
                    (self.last_frame_id, self.last_frame_captured_at)
                else {
                    return Err(resource_reading_poison(
                        HostCode::ContainedTaskResourceReadingFrameMissing,
                    ));
                };
                let step_sequence = self
                    .last_step_finished
                    .as_ref()
                    .filter(|(operation, _)| *operation == operation_label)
                    .map(|(_, sequence)| *sequence);
                let Some(step_sequence) =
                    step_sequence.filter(|_| frame_captured_at == captured_at)
                else {
                    return Err(resource_reading_poison(
                        HostCode::ContainedTaskResourceReadingFrameMismatch,
                    ));
                };
                self.adjustment_confirmations
                    .push((captured_at, frame_id, step_sequence));
                Ok(())
            }
            // Workflow #384 D2-S3: the adjustments of the next confirmation, joined before
            // Finalizing in confirmation order, after every handed-off frame. An amount that gave
            // no reading is a warning row on the confirmation frame; once the last confirmation
            // is joined, the run's adjustments are planned and each one's outcome is a row.
            ContainedTaskTrace::ResourceAdjustments {
                captured_at,
                adjustments,
            } => {
                if self.finalizing.is_some()
                    || self.joined_reading_frames != self.reading_frames.len()
                {
                    return Err(resource_reading_poison(
                        HostCode::ContainedTaskResourceReadingDuplicate,
                    ));
                }
                let frame_id = self.joined_adjustment_frame(captured_at)?;
                self.joined_adjustment_confirmations += 1;
                let recorded = adjustments.iter().try_for_each(|adjustment| {
                    match (adjustment.amount_miss(), &adjustment.declaration.amount) {
                        (Some(miss), Some(amount)) => self.diagnostic_adjustment(
                            frame_id,
                            Some(amount.target_id.as_str()),
                            miss,
                        ),
                        _ => Ok(()),
                    }
                });
                self.diagnostic_frame = None;
                recorded?;
                self.taken_adjustments.push(adjustments);
                if self.joined_adjustment_confirmations == self.adjustment_confirmations.len() {
                    self.plan_resource_adjustments()?;
                }
                Ok(())
            }
            ContainedTaskTrace::Finalizing { outcome } => {
                let stability_finalization_invalid = match (
                    self.expected_stability_declaration.as_ref(),
                    self.stability.as_ref(),
                ) {
                    (None, None) => false,
                    (Some(expected), Some(state)) => {
                        &state.declaration != expected
                            || !state.terminal_recorded
                            || state.pending_terminal.is_some()
                    }
                    _ => true,
                };
                let post_admission_ocr_finalization_invalid = if self.expects_post_admission_ocr {
                    // Workflow #390 OUT-1 (FCM O-05): a fields run that passed no field page
                    // has neither an observation nor a report.
                    let no_field_page = self.collects_ocr_fields
                        && self.post_admission_ocr_observations == 0
                        && !self.post_admission_ocr_comparison_recorded;
                    !no_field_page
                        && (self.post_admission_ocr_observations == 0
                            || !self.post_admission_ocr_comparison_recorded)
                } else {
                    self.post_admission_ocr_observations != 0
                        || self.post_admission_ocr_comparison_recorded
                };
                // Workflow #384 D2-S2: a successful run joins every frame it handed off; D2-S3:
                // and every confirmation of adjustments.
                let readings_finalization_invalid = outcome == TaskOutcome::Success
                    && (self.joined_reading_frames != self.reading_frames.len()
                        || self.joined_adjustment_confirmations
                            != self.adjustment_confirmations.len());
                if self.finalizing.replace(outcome).is_some()
                    || !self.step_actions.is_empty()
                    || stability_finalization_invalid
                    || post_admission_ocr_finalization_invalid
                    || readings_finalization_invalid
                {
                    return Err(RequestFailure::poison_without_terminal(
                        RuntimeHostError::fatal(
                            HostCode::ContainedTaskFinalizingStateInvalid,
                            ContractLocation::RunContainedTask,
                            RuntimeErrorCode::RuntimeHalted,
                        ),
                    ));
                }
                self.append_task(
                    EventSeverity::Info,
                    self.links(),
                    TaskPayloadDraft::semantic(
                        TaskSemanticFact::Finalizing {
                            task_result: outcome,
                        },
                        AuditInput::new(),
                    ),
                )
            }
        }
    }
}

fn contained_task_step_action(
    steps: &BTreeMap<u32, (IssuedActionId, String)>,
    step_index: u32,
    operation_label: &str,
) -> Result<IssuedActionId, RequestFailure> {
    steps
        .get(&step_index)
        .filter(|(_, expected)| expected == operation_label)
        .map(|(action_id, _)| *action_id)
        .ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                HostCode::ContainedTaskStepIdentityMismatch,
                ContractLocation::RunContainedTask,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })
}

pub(super) struct RuntimeArtifactEventSink<'a> {
    pub(super) ledger: &'a GlobalLedger,
    pub(super) events: &'a RuntimeEvents,
}

#[cfg(test)]
struct FailingContainedTaskStabilityEventSink;

#[cfg(test)]
impl ArtifactEventSink for FailingContainedTaskStabilityEventSink {
    fn append(&mut self, _draft: EventDraft) -> ArtifactStoreResult<()> {
        Err(ArtifactStoreError::fatal(
            "contained_task_stability_event_append_injected_failure",
            "append_contained_task_stability_artifact_event",
            "injected test failure",
        ))
    }
}

impl ArtifactEventSink for RuntimeArtifactEventSink<'_> {
    fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
        let sanitized = self.events.sanitize(draft).map_err(|error| {
            ArtifactStoreError::fatal(
                "artifact_event_sanitize_failed",
                "append_runtime_artifact_event",
                error.to_string(),
            )
        })?;
        self.ledger.append(sanitized).map(|_| ()).map_err(|error| {
            ArtifactStoreError::fatal(
                "artifact_event_append_failed",
                "append_runtime_artifact_event",
                error.to_string(),
            )
        })
    }
}

pub(super) fn prepare_contained_task(
    instance_alias: &str,
    request: &ContainedTaskRequest,
    vision_provider: Option<Arc<dyn RecognitionVisionProvider>>,
    deadline: Instant,
) -> Result<PreparedContainedTask, RequestFailure> {
    let path = Path::new(request.package_path());
    if !path.is_absolute() {
        return Err(contained_task_package_failure(
            HostCode::ContainedTaskPathNotAbsolute,
        ));
    }
    // Workflow #336 PKG-S1 (option 丙): the path chooses the loader; the request's reference is
    // recorded as the caller gave it and compared with nothing.
    PreparedContainedTask::load_path(
        instance_alias,
        path,
        request.expected_sha256(),
        vision_provider,
        deadline,
    )
    .map_err(contained_task_declaration_failure)
}

fn contained_task_declaration_failure(
    error: actingcommand_execution_kernel::ContainedTaskError,
) -> RequestFailure {
    let mut refused = RuntimeHostError::relayed_request(
        error.code().as_str(),
        ContractLocation::RunContainedTask.as_str(),
        RuntimeErrorCode::PackageInvalid,
    );
    // Workflow #386 SR-1: `recognition_sample_budget_invalid` keeps its `required_ms`,
    // `limit_ms` and `pack_operation_id` on the refusal.
    if error.has_values() {
        let values = error.outcome();
        refused =
            refused.with_outcome_values(|_| values.operation(ContractLocation::RunContainedTask));
    }
    let mut failure = RequestFailure::request(refused, RuntimeReceiptState::Denied, None);
    if let Some(detail) = resource_reading_failure_detail(error.code().as_str(), error.detail()) {
        let refused = *failure.error;
        failure.error = Box::new(refused.with_native_detail(detail));
    }
    if let Some(issue) = error.declaration_issue() {
        // Declaration parsing happens only after the package was read. Workflow #336 PKG-S2
        // (option 丙): the package is named by the `package_id` its control declares, when
        // preparation read one; no package digest is recorded.
        failure.error.lifecycle.resource_declaration = Some(Box::new(
            actingcommand_contract::ResourceDeclarationRejection {
                package_label: error.declaration_package().map(str::to_owned),
                program_version: format!(
                    "actingcommand-runtime-host/{}",
                    env!("CARGO_PKG_VERSION")
                ),
                issue: issue.clone(),
            },
        ));
    }
    failure
}

fn contained_task_package_failure(code: impl Into<Code>) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::request(
            code,
            ContractLocation::RunContainedTask,
            RuntimeErrorCode::PackageInvalid,
        ),
        RuntimeReceiptState::Denied,
        None,
    )
}

/// Workflow #369 S3a: a host package run's own request did not validate (an invariant).
fn startup_package_request_invalid() -> RequestFailure {
    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
        HostCode::StartupPackageRequestInvalid,
        HostLocation::RunStartupPackage,
        RuntimeErrorCode::RuntimeHalted,
    ))
}

/// Types a startup package admission refusal (slice #316-B3): a locator that does not exist
/// is `startup_package_missing`, every other refusal `startup_package_admission_failed`; the
/// underlying admission code stays attached as the related failure. The resource declaration
/// rejection, when there is one, travels with it. Workflow #336 PKG-S1: every locator is read
/// by its path, so a missing one is containment's `content_directory_missing`.
fn startup_package_admission_failure(mut failure: RequestFailure) -> RequestFailure {
    if failure.poison_runtime || failure.error.is_fatal() {
        return failure;
    }
    let code = if failure.error.code()
        == actingcommand_pack_containment::PackContainmentCode::ContentDirectoryMissing.as_str()
    {
        HostCode::StartupPackageMissing
    } else {
        HostCode::StartupPackageAdmissionFailed
    };
    let mut error = RuntimeHostError::request(
        code,
        HostLocation::RunStartupPackage,
        RuntimeErrorCode::PackageInvalid,
    )
    .with_related_failure("package_admission", &failure.error);
    error.lifecycle.resource_declaration = failure.error.lifecycle.resource_declaration.take();
    failure.error = Box::new(error);
    failure
}

fn select_scheduling_disposition(
    events: &[PersistedEvent],
    outcome: TaskOutcome,
    final_page: Option<&str>,
    executed_steps: Option<u32>,
    contract: Option<&(String, SchedulingOutcomeDeclaration)>,
    selected_outcome_key: Option<&str>,
    designated_effect_caps: &[DesignatedEffectCap],
) -> Result<Option<SchedulingDisposition>, RequestFailure> {
    let Some((game, declaration)) = contract else {
        if selected_outcome_key.is_some() {
            return Err(scheduling_outcome_failure(
                HostCode::ContainedTaskOutcomeDeclarationMissing,
            ));
        }
        return Ok(None);
    };
    if outcome != TaskOutcome::Success {
        return Err(scheduling_outcome_failure(
            HostCode::ContainedTaskOutcomeRequiresSuccess,
        ));
    }
    let executed_steps = executed_steps
        .ok_or_else(|| scheduling_outcome_failure(HostCode::ContainedTaskOutcomeProgressMissing))?;
    let final_page = final_page.ok_or_else(|| {
        scheduling_outcome_failure(HostCode::ContainedTaskOutcomeTerminalPageMissing)
    })?;
    // The completed effects of every designated operation (Workflow #390 OUT-2: of each member
    // of a designated effect set), in ledger order.
    let effects_of = |designated: &str| {
        events
            .iter()
            .filter_map(|event| {
                let EventPayload::Task(TaskPayload::Semantic(payload)) = event.payload() else {
                    return None;
                };
                let TaskSemanticFact::EffectCompleted {
                    step_index,
                    operation_label,
                } = payload.fact()
                else {
                    return None;
                };
                (operation_label == designated).then_some((*step_index, operation_label.clone()))
            })
            .collect::<Vec<_>>()
    };
    let member_effects = declaration
        .designated_operations()
        .into_iter()
        .map(effects_of)
        .collect::<Vec<_>>();
    let designated_effects = member_effects.concat();
    if designated_effects
        .iter()
        .any(|(step_index, _)| *step_index >= executed_steps)
    {
        return Err(scheduling_outcome_failure(
            HostCode::ContainedTaskOutcomeEffectOutsideExecutedSteps,
        ));
    }
    // The one designated operation completes at most once; a member of a set at most once per
    // step and at most as often as the cap its run held it to.
    let tallies = match declaration.designated_effects() {
        None => {
            if designated_effects.len() > 1 {
                return Err(scheduling_outcome_failure(
                    HostCode::ContainedTaskOutcomeDesignatedEffectDuplicate,
                ));
            }
            None
        }
        Some(members) => {
            if designated_effect_caps.len() != members.len()
                || members
                    .iter()
                    .zip(designated_effect_caps)
                    .any(|(member, cap)| member.operation_id() != cap.operation_id)
            {
                return Err(scheduling_outcome_failure(
                    HostCode::ContainedTaskOutcomeProgressMissing,
                ));
            }
            let mut tallies = Vec::with_capacity(members.len());
            for (effects, cap) in member_effects.iter().zip(designated_effect_caps) {
                let steps = effects
                    .iter()
                    .map(|(step_index, _)| *step_index)
                    .collect::<BTreeSet<_>>();
                let count = u32::try_from(effects.len()).unwrap_or(u32::MAX);
                if steps.len() != effects.len() || count > cap.max_repeats {
                    return Err(scheduling_outcome_failure(
                        HostCode::ContainedTaskOutcomeDesignatedEffectDuplicate,
                    ));
                }
                tallies.push((cap.operation_id.clone(), count, cap.max_repeats));
            }
            Some(tallies)
        }
    };
    if executed_steps == 0 {
        let observed_page = events.iter().rev().find_map(|event| {
            let EventPayload::Task(TaskPayload::Semantic(payload)) = event.payload() else {
                return None;
            };
            let TaskSemanticFact::RecognitionCompleted {
                matched_page: Some(page),
                ..
            } = payload.fact()
            else {
                return None;
            };
            Some(page.as_str())
        });
        if observed_page.is_none_or(|page| !page_anchor_matches(game, page, final_page)) {
            return Err(scheduling_outcome_failure(
                HostCode::ContainedTaskOutcomeFinalObservationMissing,
            ));
        }
    } else {
        let final_step = executed_steps - 1;
        let semantic_facts = events.iter().filter_map(|event| {
            let EventPayload::Task(TaskPayload::Semantic(payload)) = event.payload() else {
                return None;
            };
            Some((event.sequence(), payload.fact()))
        });
        // A retried step records one StepFinished per attempt; judge only the last attempt.
        let last_attempt_started = semantic_facts
            .clone()
            .filter_map(|(sequence, fact)| {
                matches!(
                    fact,
                    TaskSemanticFact::StepStarted { step_index, .. } if *step_index == final_step
                )
                .then_some(sequence)
            })
            .max();
        let final_steps = semantic_facts
            .filter_map(|(sequence, fact)| {
                let TaskSemanticFact::StepFinished {
                    step_index,
                    page_label,
                    ..
                } = fact
                else {
                    return None;
                };
                (*step_index == final_step
                    && last_attempt_started.is_none_or(|started| sequence > started))
                .then_some(page_label.as_str())
            })
            .collect::<Vec<_>>();
        let [observed_page] = final_steps.as_slice() else {
            return Err(scheduling_outcome_failure(
                HostCode::ContainedTaskOutcomeFinalStepNotUnique,
            ));
        };
        if !page_anchor_matches(game, observed_page, final_page) {
            return Err(scheduling_outcome_failure(
                HostCode::ContainedTaskOutcomeFinalPageConflict,
            ));
        }
    }
    for (step_index, operation_label) in &designated_effects {
        // Same last-attempt rule as the final step: earlier attempts' StepFinished are ignored.
        let last_attempt_started = events
            .iter()
            .filter(|event| {
                let EventPayload::Task(TaskPayload::Semantic(payload)) = event.payload() else {
                    return false;
                };
                matches!(
                    payload.fact(),
                    TaskSemanticFact::StepStarted {
                        step_index: started_step,
                        ..
                    } if started_step == step_index
                )
            })
            .map(PersistedEvent::sequence)
            .max();
        let lifecycle_count = events
            .iter()
            .filter(|event| {
                let EventPayload::Task(TaskPayload::Semantic(payload)) = event.payload() else {
                    return false;
                };
                last_attempt_started.is_none_or(|started| event.sequence() > started)
                    && matches!(
                        payload.fact(),
                        TaskSemanticFact::StepFinished {
                            step_index: completed_step,
                            operation_label: completed_operation,
                            ..
                        } if completed_step == step_index && completed_operation == operation_label
                    )
            })
            .count();
        if lifecycle_count != 1 {
            return Err(scheduling_outcome_failure(
                HostCode::ContainedTaskOutcomeDesignatedStepNotUnique,
            ));
        }
    }
    let effect = match (tallies, designated_effects.as_slice()) {
        // Workflow #390 OUT-2: a set's run records every member's count and cap.
        (Some(tallies), _) => SchedulingEffectEvidence::DesignatedEffects {
            effects: tallies
                .into_iter()
                .map(|(operation, count, max_repeats)| {
                    DesignatedEffectTally::new(operation, count, max_repeats)
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        HostCode::ContainedTaskOutcomeDispositionInvalid,
                        HostLocation::AppendContainedTaskTerminal,
                        RuntimeErrorCode::RuntimeHalted,
                    ))
                })?,
        },
        (None, [(step_index, operation_label)]) => {
            SchedulingEffectEvidence::DesignatedEffectCompleted {
                step_index: *step_index,
                operation_label: operation_label.clone(),
            }
        }
        (None, _) => SchedulingEffectEvidence::NoDesignatedEffect,
    };
    let condition = effect.condition();
    let matching = declaration
        .mappings()
        .iter()
        .filter(|mapping| {
            mapping.effect() == condition
                && mapping
                    .terminal_pages()
                    .iter()
                    .any(|page| page_anchor_matches(game, final_page, page))
        })
        .collect::<Vec<_>>();
    let [mapping] = matching.as_slice() else {
        return Err(scheduling_outcome_failure(if matching.is_empty() {
            HostCode::ContainedTaskOutcomeMappingMissing
        } else {
            HostCode::ContainedTaskOutcomeMappingConflict
        }));
    };
    if selected_outcome_key.is_some_and(|selected| selected != mapping.outcome_key()) {
        return Err(scheduling_outcome_failure(
            HostCode::ContainedTaskOutcomeComparisonConflict,
        ));
    }
    SchedulingDisposition::new(mapping.outcome_key(), effect)
        .map(Some)
        .map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                HostCode::ContainedTaskOutcomeDispositionInvalid,
                HostLocation::AppendContainedTaskTerminal,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })
}

fn scheduling_outcome_failure(code: impl Into<Code>) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::request(
            code,
            HostLocation::AppendContainedTaskTerminal,
            RuntimeErrorCode::BackendOperationFailed,
        ),
        RuntimeReceiptState::Failed,
        None,
    )
}

pub(super) const fn task_outcome_severity(outcome: TaskOutcome) -> EventSeverity {
    match outcome {
        TaskOutcome::Success => EventSeverity::Info,
        TaskOutcome::Failure => EventSeverity::Error,
        TaskOutcome::Cancelled => EventSeverity::Warning,
    }
}

pub(super) const fn task_outcome_event_type(outcome: TaskOutcome) -> EventType {
    match outcome {
        TaskOutcome::Success => EventType::TaskCompleted,
        TaskOutcome::Failure => EventType::TaskFailed,
        TaskOutcome::Cancelled => EventType::TaskCancelled,
    }
}

fn contained_task_replay_denied(code: impl Into<Code>) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::request(
            code,
            HostLocation::RecoverContainedTask,
            RuntimeErrorCode::ProtocolInvalid,
        ),
        RuntimeReceiptState::Denied,
        None,
    )
}

fn contained_task_sampling_seed<T: serde::Serialize>(value: &T) -> RuntimeHostResult<u64> {
    let bytes = serde_json::to_vec(value).map_err(|_| {
        RuntimeHostError::fatal(
            HostCode::ContainedTaskSamplingSeedEncodeFailed,
            HostLocation::DeriveContainedTaskSamplingSeed,
            RuntimeErrorCode::RuntimeHalted,
        )
    })?;
    let digest = Sha256::digest(bytes);
    let mut seed = [0_u8; 8];
    seed.copy_from_slice(&digest[..8]);
    Ok(u64::from_be_bytes(seed))
}

pub(crate) fn require_contained_task_sampling_run_seed(
    run_seed: Option<u64>,
) -> RuntimeHostResult<u64> {
    run_seed.ok_or_else(|| {
        RuntimeHostError::fatal(
            HostCode::ContainedTaskSamplingSeedMissing,
            HostLocation::DeriveContainedTaskActionSeed,
            RuntimeErrorCode::RuntimeHalted,
        )
    })
}

impl HostShared {
    #[cfg(test)]
    pub(super) fn consume_contained_task_checkpoint_for_test(
        &self,
        point: ContainedTaskCheckpointPoint,
        identity: ContainedTaskCheckpointIdentity,
    ) -> RuntimeHostResult<()> {
        let hook = {
            let mut slot = lock(
                &self.contained_task_checkpoint_test_hook,
                crate::codes::test_spellings::TestLocation::ConsumeContainedTaskCheckpointTestHook,
            )?;
            let should_consume = match slot.as_mut() {
                Some(hook)
                    if hook.point == point
                        && hook
                            .request_id
                            .is_none_or(|request_id| request_id == identity.request_id)
                        && hook.instance_id == identity.instance_id
                        && hook.execution_thread == thread::current().id() =>
                {
                    let bound_lease_id = hook.lease_id.get_or_insert(identity.lease_id);
                    *bound_lease_id == identity.lease_id
                }
                _ => false,
            };
            should_consume.then(|| slot.take()).flatten()
        };
        let Some(hook) = hook else {
            return Ok(());
        };
        *lock(
            &hook.observed,
            crate::codes::test_spellings::TestLocation::RecordContainedTaskCheckpointTestIdentity,
        )? = Some(identity);
        (hook.action)(identity);
        hook.consumed.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    pub(super) fn cancel_contained_task(
        &self,
        task_request_id: RequestId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let active = lock(&self.contained_runs, HostLocation::ReadActiveContainedRun)?
            .get(&task_request_id)
            .cloned();
        if let Some(control) = &active {
            if !control.client_cancellable {
                return Err(RequestFailure::request(
                    RuntimeHostError::request(
                        SharedCode::ScheduledContainedTaskNotClientCancellable,
                        ContractLocation::CancelContainedTask,
                        RuntimeErrorCode::InvalidRequest,
                    ),
                    RuntimeReceiptState::Denied,
                    None,
                ));
            }
            control.request_cancel();
        }
        let events = self
            .ledger
            .query(EventQuery {
                request_id: Some(task_request_id),
                ..EventQuery::default()
            })
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error(
                    HostLocation::QueryContainedTaskCancellation,
                ))
            })?;
        if let Some(control) = &active
            && (control.request_id != task_request_id
                || events.iter().any(|event| {
                    event
                        .links()
                        .instance_id()
                        .is_some_and(|instance_id| instance_id != &control.instance_id)
                }))
        {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    HostCode::ContainedTaskActiveIdentityMismatch,
                    ContractLocation::CancelContainedTask,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            ));
        }
        if events.is_empty() && active.is_none() {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    HostCode::ContainedTaskCancellationIdentityMissing,
                    ContractLocation::CancelContainedTask,
                    RuntimeErrorCode::InvalidRequest,
                ),
                RuntimeReceiptState::Denied,
                None,
            ));
        }
        let deadline_monotonic_ms = events.iter().find_map(|event| match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                TaskSemanticFact::PackageAdmitted {
                    response_deadline_monotonic_ms,
                    ..
                } => *response_deadline_monotonic_ms,
                _ => None,
            },
            _ => None,
        });
        let terminals = events
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                    TaskSemanticFact::TerminalCommitted {
                        task_result,
                        outcome,
                        ..
                    } => Some((event, *task_result, outcome.as_ref().map(Outcome::code))),
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>();
        if terminals.len() > 1 {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    HostCode::ContainedTaskTerminalStateInconsistent,
                    ContractLocation::CancelContainedTask,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            ));
        }
        if let [(task_terminal, outcome, failure_code)] = terminals.as_slice() {
            let lease_id = task_terminal.links().lease_id().copied().ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    HostCode::ContainedTaskIdentityMissing,
                    ContractLocation::CancelContainedTask,
                    RuntimeErrorCode::RuntimeHalted,
                ))
            })?;
            let lease_events = self
                .ledger
                .query(EventQuery {
                    lease_id: Some(lease_id),
                    ..EventQuery::default()
                })
                .map_err(|_| {
                    RequestFailure::poison_without_terminal(ledger_error(
                        HostLocation::QueryContainedTaskLeaseTerminal,
                    ))
                })?;
            let lease_terminals = lease_events
                .iter()
                .filter(|event| {
                    matches!(
                        event.event_type(),
                        EventType::LeaseReleased | EventType::LeaseExpired
                    )
                })
                .collect::<Vec<_>>();
            if lease_terminals.len() > 1 {
                return Err(RequestFailure::poison_without_terminal(
                    RuntimeHostError::fatal(
                        HostCode::ContainedTaskLeaseTerminalStateInconsistent,
                        ContractLocation::CancelContainedTask,
                        RuntimeErrorCode::RuntimeHalted,
                    ),
                ));
            }
            if let [lease_terminal] = lease_terminals.as_slice() {
                let reason = (*outcome == TaskOutcome::Cancelled).then_some(
                    ContainedTaskCancellationReason::from_failure_code(*failure_code),
                );
                return Ok(OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: Some(terminal(lease_terminal)),
                    result: RuntimeResult::ContainedTaskCancellation {
                        task_request_id,
                        status: ContainedTaskCancellationStatus::Terminal {
                            deadline_monotonic_ms,
                            task_result: *outcome,
                            reason,
                            task_terminal: terminal(task_terminal),
                            lease_terminal: terminal(lease_terminal),
                            lease_disposition: if lease_terminal.event_type()
                                == EventType::LeaseExpired
                            {
                                ContainedTaskLeaseTerminal::Expired
                            } else {
                                ContainedTaskLeaseTerminal::Released
                            },
                        },
                    },
                    failure_outcome: None,
                });
            }
        }
        let status = match active {
            Some(control) => ContainedTaskCancellationStatus::Pending {
                deadline_monotonic_ms: control.deadline(),
            },
            None => ContainedTaskCancellationStatus::RecoveryRequired {
                deadline_monotonic_ms,
            },
        };
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::ContainedTaskCancellation {
                task_request_id,
                status,
            },
            failure_outcome: None,
        })
    }

    pub(super) fn run_contained_task(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        holder_id: actingcommand_contract::HolderId,
        task_request: &ContainedTaskRequest,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        if let Some(recovered) = self.recover_contained_task(
            original,
            request,
            instance_alias,
            resolved.instance_id(),
            task_request,
        )? {
            return Ok(recovered);
        }
        self.require_business_capacity(self.events.request_links(
            request,
            Some(resolved.instance_id()),
            None,
            None,
        ))?;
        let active_run =
            self.begin_contained_run(original.request_id(), resolved.instance_id(), true)?;
        // Workflow #387 D1c (B1): a direct run's failed end starts a ladder only under
        // `recovery_ladder.rules.direct_runs = trigger`, which `ladder_at_lease_end` reads.
        let ladder_correlation_id = self
            .events
            .issuer()
            .mint_correlation_id()
            .map_err(|_| RequestFailure::poison_without_terminal(runtime_identifier_error()))?;
        active_run
            .control
            .note_ladder_source(super::recovery_ladder::LadderSource {
                correlation_id: ladder_correlation_id,
                recovery: task_request.recovery().cloned(),
                direct: true,
            })
            .map_err(RequestFailure::poison_without_terminal)?;
        active_run
            .control
            .set_deadline(
                self.monotonic_ms()
                    .and_then(|now| {
                        now.checked_add(task_request.response_deadline_ms())
                            .ok_or_else(|| {
                                RuntimeHostError::fatal(
                                    HostCode::ContainedTaskDeadlineOverflow,
                                    HostLocation::DeriveContainedTaskDeadline,
                                    RuntimeErrorCode::RuntimeHalted,
                                )
                            })
                    })
                    .map_err(RequestFailure::poison_without_terminal)?,
            )
            .map_err(RequestFailure::poison_without_terminal)?;
        let prepared = prepare_contained_task(
            instance_alias,
            task_request,
            self.execution()?.vision_provider(),
            self.package_material_deadline(active_run.control.deadline())?,
        )?;
        let prerequisites = self.resolve_prerequisite_chain(instance_alias, &prepared, || {
            self.package_material_deadline(active_run.control.deadline())
        })?;
        // Workflow #373 NAV-6: the run's shared interrupt pages, read in its preparation.
        let interrupts = self.prepare_run_interrupts(&prepared)?;
        self.append_request_lifecycle(
            original,
            request,
            resolved.instance_id(),
            EventAction::RuntimeTaskRun,
            None,
        )?;
        let task_id = self
            .events
            .issuer()
            .mint_task_id()
            .map_err(|_| RequestFailure::poison_without_terminal(runtime_identifier_error()))?;
        let run_id = self
            .events
            .issuer()
            .mint_run_id()
            .map_err(|_| RequestFailure::poison_without_terminal(runtime_identifier_error()))?;
        let lease_ttl_ms = self.contained_task_lease_ttl(task_request)?;
        // Workflow #369 W-3: the run waits for its turn in the instance's queue; its deadline
        // stays anchored at receipt, so the wait is taken from it.
        let receipt_deadline_monotonic_ms = active_run.control.deadline();
        let cap_ms = direct_task_run_claim_cap_ms(
            receipt_deadline_monotonic_ms,
            self.monotonic_ms()
                .map_err(RequestFailure::poison_without_terminal)?,
        );
        let token = self.await_operator_claim(
            HostClaim {
                request: original,
                instance_alias,
                holder_id,
                connection_id,
                kind: ClaimKind::DirectTaskRun,
                priority: actingcommand_contract::LeasePriority::Normal,
                lease_ttl_ms,
            },
            cap_ms,
        )?;
        let deadline_monotonic_ms =
            match self.contained_task_deadline_until(receipt_deadline_monotonic_ms, &token) {
                Ok(deadline) => deadline,
                Err(failure) => {
                    return Err(self.cleanup_composite_failure(token, connection_id, failure));
                }
            };
        active_run
            .control
            .set_deadline(deadline_monotonic_ms)
            .map_err(RequestFailure::poison_without_terminal)?;
        // Workflow #387 D1c-H (B1): the run's records count for direction ① only under
        // `rules.direct_runs = trigger`; the counter sees its key held by the run's
        // `DirectTaskRun` claim, the fence's manual set.
        let executed = self.execute_contained_task_with_lease(
            original,
            request,
            instance_alias,
            connection_id,
            prepared,
            prerequisites,
            interrupts,
            task_request,
            token,
            task_id,
            run_id,
            ExecutionBackendProvenance::PhysicalDevice,
            None,
            active_run.control(),
            None,
        );
        // Workflow #191 ps2: the client of a cancelled run resets the instance next
        // (`SafeReset` on this connection); an instance pause hands the device back only after
        // that. Noted while the run is still registered, so a draining pause sees it.
        let awaited = match &executed {
            Ok(OperationSuccess {
                result: RuntimeResult::ContainedTaskCancelled { .. },
                ..
            }) => self.await_client_reset(instance_alias, connection_id),
            _ => Ok(()),
        };
        // Workflow #369-3 (coordinator ruling Q1): a direct run (CLI or console task-run, MCP
        // `ac_run_pack`) starts no stuck-recovery ladder by default; whoever ran it has the
        // receipt and decides. Workflow #387 D1c: `rules.direct_runs = trigger` lets its failed
        // end queue one (`ladder_after_direct_run`).
        with_recovery_ladder_staging(executed, awaited)
    }

    pub(super) fn package_material_deadline(
        &self,
        deadline_monotonic_ms: u64,
    ) -> Result<Instant, RequestFailure> {
        let now = self
            .monotonic_ms()
            .map_err(RequestFailure::poison_without_terminal)?;
        Instant::now()
            .checked_add(Duration::from_millis(
                deadline_monotonic_ms.saturating_sub(now),
            ))
            .ok_or_else(|| contained_task_package_failure(HostCode::ContainedTaskDeadlineOverflow))
    }

    fn contained_task_deadline(
        &self,
        request: &ContainedTaskRequest,
        token: &LeaseToken,
    ) -> Result<u64, RequestFailure> {
        let now = self
            .monotonic_ms()
            .map_err(RequestFailure::poison_without_terminal)?;
        let requested = now
            .checked_add(request.response_deadline_ms())
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    HostCode::ContainedTaskDeadlineOverflow,
                    HostLocation::DeriveContainedTaskDeadline,
                    RuntimeErrorCode::RuntimeHalted,
                ))
            })?;
        self.contained_task_deadline_until(requested, token)
    }

    /// The earlier of `requested` and the lease boundary; refused when it has passed. Workflow
    /// #369 W-3: a direct run passes its receipt deadline, so its deadline stays anchored at
    /// receipt however long it waited for its grant.
    fn contained_task_deadline_until(
        &self,
        requested: u64,
        token: &LeaseToken,
    ) -> Result<u64, RequestFailure> {
        let now = self
            .monotonic_ms()
            .map_err(RequestFailure::poison_without_terminal)?;
        let reserve = lock(
            &self.scheduler,
            HostLocation::ReadContainedTaskDeadlineConfig,
        )?
        .config()
        .maximum_client_heartbeat_interval_ms;
        let lease_boundary = token
            .expires_at_monotonic_ms()
            .checked_sub(reserve)
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    HostCode::ContainedTaskLeaseDeadlineInvalid,
                    HostLocation::DeriveContainedTaskDeadline,
                    RuntimeErrorCode::RuntimeHalted,
                ))
            })?;
        let deadline = requested.min(lease_boundary);
        if deadline <= now {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    HostCode::ContainedTaskDeadlineUnavailable,
                    HostLocation::DeriveContainedTaskDeadline,
                    RuntimeErrorCode::ContainedTaskDeadlineExceeded,
                ),
                RuntimeReceiptState::Denied,
                None,
            ));
        }
        Ok(deadline)
    }

    pub(super) fn contained_task_lease_ttl(
        &self,
        request: &ContainedTaskRequest,
    ) -> Result<u64, RequestFailure> {
        let reserve = lock(
            &self.scheduler,
            HostLocation::ReadContainedTaskLeaseTtlConfig,
        )?
        .config()
        .maximum_client_heartbeat_interval_ms;
        request
            .response_deadline_ms()
            .checked_add(reserve)
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    HostCode::ContainedTaskLeaseTtlOverflow,
                    HostLocation::DeriveContainedTaskLeaseTtl,
                    RuntimeErrorCode::RuntimeHalted,
                ))
            })
    }

    /// Workflow #369 S3a: everything a host package run does before its lease: identity,
    /// capacity, admission, prerequisite chain, entry checks and its command lifecycle. The run
    /// will hold `holder_id`'s lease; a refusal here takes no lease.
    pub(super) fn prepare_package_run(
        &self,
        pending: &startup_package::PendingStartupPackage,
        holder_id: actingcommand_contract::HolderId,
    ) -> Result<PreparedPackageRun<'_>, RequestFailure> {
        let instance_alias = pending.instance_alias.as_str();
        let task_request = &pending.request;
        let resolved = self.resolve_instance(instance_alias)?;
        if resolved.instance_id() != pending.instance_id {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    HostCode::StartupPackageInstanceMismatch,
                    HostLocation::RunStartupPackage,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            ));
        }
        let execution_provenance = resolved.provenance();
        let (task_actor, task_source) = scheduled_request_transport_origin(execution_provenance);
        let issuer = self.events.issuer();
        let identifier = || RequestFailure::poison_without_terminal(runtime_identifier_error());
        let request_id = issuer.mint_request_id().map_err(|_| identifier())?;
        let correlation_id = issuer.mint_correlation_id().map_err(|_| identifier())?;
        let task_id = issuer.mint_task_id().map_err(|_| identifier())?;
        let run_id = issuer.mint_run_id().map_err(|_| identifier())?;
        let task_request_message = RuntimeRequest::new(
            request_id,
            correlation_id,
            Some(pending.causation_id),
            task_actor,
            task_source,
            unix_ms_now().map_err(RequestFailure::poison_without_terminal)?,
            RuntimeOperation::RunContainedTask {
                instance_alias: instance_alias.to_owned(),
                holder_id,
                request: task_request.clone(),
            },
        )
        .map_err(|_| startup_package_request_invalid())?;
        let validated = task_request_message
            .validate()
            .map_err(|_| startup_package_request_invalid())?;
        self.require_business_capacity(self.events.request_links(
            &validated,
            Some(resolved.instance_id()),
            None,
            None,
        ))?;
        let active_run = self.begin_contained_run(
            task_request_message.request_id(),
            resolved.instance_id(),
            false,
        )?;
        active_run
            .control
            .set_deadline(
                self.monotonic_ms()
                    .and_then(|now| {
                        now.checked_add(task_request.response_deadline_ms())
                            .ok_or_else(|| {
                                RuntimeHostError::fatal(
                                    HostCode::ContainedTaskDeadlineOverflow,
                                    HostLocation::DeriveContainedTaskDeadline,
                                    RuntimeErrorCode::RuntimeHalted,
                                )
                            })
                    })
                    .map_err(RequestFailure::poison_without_terminal)?,
            )
            .map_err(RequestFailure::poison_without_terminal)?;
        let material_deadline = self.package_material_deadline(active_run.control.deadline())?;
        let prepared = prepare_contained_task(
            instance_alias,
            task_request,
            self.execution()?.vision_provider(),
            material_deadline,
        )
        .map_err(|failure| match pending.run {
            startup_package::HostPackageRun::StartupPackage => {
                startup_package_admission_failure(failure)
            }
            startup_package::HostPackageRun::ReturnHome => failure,
        })?;
        // Workflow #336 L2d: a configured return-home package on the ladder's first rung passes
        // the checks of a return-home chain layer against the failed package and declares no
        // prerequisite package, before its chain and any lease.
        if let Some(failed) = &pending.configured_return_home
            && let Some(detail) =
                prerequisite::configured_return_home_incompatibility(&prepared, failed)
        {
            return Err(prerequisite::configured_return_home_refusal(detail));
        }
        let prerequisites =
            self.resolve_prerequisite_chain(instance_alias, &prepared, || Ok(material_deadline))?;
        // Workflow #373 NAV-6: the run's shared interrupt pages, read in its preparation.
        let interrupts = self.prepare_run_interrupts(&prepared)?;
        if pending.recovery_rung {
            let (capture, input) = prepared.recovery_entry_channels();
            if let Some(code) = self
                .recovery_entry_unavailable(resolved.instance_id(), capture, input)
                .map_err(RequestFailure::poison_without_terminal)?
            {
                return Err(RequestFailure::request(
                    RuntimeHostError::request(
                        code,
                        HostLocation::RunStartupPackage,
                        RuntimeErrorCode::BackendOperationFailed,
                    ),
                    RuntimeReceiptState::Denied,
                    None,
                ));
            }
        }
        // An application action needs ADB independently of input/capture. Other recovery
        // entries use their configured providers and let those owners establish channels.
        if (!pending.recovery_rung || prepared.has_application_effect())
            && let Err(error) = self.execution()?.probe_adb_baseline_until(
                instance_alias,
                Instant::now() + Duration::from_secs(30),
                &|| self.fatal.is_shutdown_requested(),
            )
        {
            // Workflow #387 D1c-G: the probe's own adb child or pipe left unconfirmed fences the
            // instance (a startup run, a rung's startup run or a return-home run alike) and the
            // run fails with the fence; only where no fence may take it is it the Runtime's fatal.
            if error.resource_quiescence() == Some(ResourceQuiescence::Unconfirmed) {
                let links =
                    self.events
                        .request_links(&validated, Some(resolved.instance_id()), None, None);
                return Err(
                    match self.settle_unconfirmed_adb(
                        RuntimeHostError::execution(HostLocation::RunStartupPackage, &error),
                        resolved.instance_id(),
                        links,
                    ) {
                        Ok(fence) => RequestFailure::fence_without_terminal(fence),
                        Err(fatal) => RequestFailure::poison_without_terminal(fatal),
                    },
                );
            }
            let mut host_error = RuntimeHostError::request(
                pending.run.adb_not_ready_code(),
                HostLocation::RunStartupPackage,
                RuntimeErrorCode::BackendOperationFailed,
            )
            .with_native_detail(format!(
                "instance_alias={instance_alias}; adb_failed={error}"
            ));
            host_error.lifecycle.instance_id = Some(resolved.instance_id());
            return Err(RequestFailure::request(
                host_error,
                RuntimeReceiptState::Failed,
                None,
            ));
        }
        // Workflow #335 S5b: a startup or return-home package writes no instance facts; one
        // that declares resource readings is refused before any lease and any input.
        if prepared.startup_incompatibility().is_some() {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    HostCode::ContainedTaskResourceReadingRunKindUnsupported,
                    HostLocation::RunStartupPackage,
                    RuntimeErrorCode::PackageInvalid,
                ),
                RuntimeReceiptState::Denied,
                None,
            ));
        }
        let run_links = RuntimeRunLinks::new(task_id, run_id);
        self.append_scheduled_request_lifecycle(
            &task_request_message,
            &validated,
            resolved.instance_id(),
            run_links,
            execution_provenance,
        )?;
        Ok(PreparedPackageRun {
            instance_id: resolved.instance_id(),
            instance_alias: instance_alias.to_owned(),
            task_request: task_request.clone(),
            task_request_message,
            prepared,
            prerequisites,
            interrupts,
            task_id,
            run_id,
            run_links,
            execution_provenance,
            active_run,
            control_request_id: pending.control_request_id,
        })
    }

    /// Workflow #369 S3a: runs a prepared host package under `held`, a key its caller holds
    /// (the startup claim's, or the ladder's), so the run takes no lease of its own. The
    /// caller first renews `held` to the run's response deadline plus the heartbeat reserve
    /// (5 s; H-3), because the run's deadline is the earlier of its response deadline and the
    /// lease's expiry less that reserve. `held` must be the lease of the prepared request's
    /// instance and holder. The run releases the lease once, under its own run links (C9), as
    /// every contained run does, a refused deadline included (through the run-linked
    /// cleanup). Only a poison exit before the run (an invalid prepared request, a key of
    /// another instance or holder, an invalid deadline state) leaves the lease where it is:
    /// the Runtime is then poisoned, and the host's close releases it.
    pub(super) fn run_prepared_package(
        &self,
        run: PreparedPackageRun<'_>,
        held: HeldPackageLease,
    ) -> Result<OperationSuccess, RequestFailure> {
        let PreparedPackageRun {
            instance_id,
            instance_alias,
            task_request,
            task_request_message,
            prepared,
            prerequisites,
            interrupts,
            task_id,
            run_id,
            run_links,
            execution_provenance,
            active_run,
            control_request_id,
        } = run;
        let HeldPackageLease {
            token,
            connection_id,
        } = held;
        let validated = task_request_message
            .validate()
            .map_err(|_| startup_package_request_invalid())?;
        let holder_matches = matches!(
            task_request_message.operation(),
            RuntimeOperation::RunContainedTask { holder_id, .. } if *holder_id == token.holder_id()
        );
        if token.instance_id() != instance_id || !holder_matches {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    HostCode::HostPackageLeaseIdentityMismatch,
                    HostLocation::RunPreparedPackage,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            ));
        }
        let deadline_monotonic_ms = match self.contained_task_deadline(&task_request, &token) {
            Ok(deadline) => deadline,
            Err(failure) => {
                return Err(self.cleanup_composite_failure_with_run_links(
                    &validated,
                    token,
                    connection_id,
                    Some(run_links),
                    failure,
                ));
            }
        };
        active_run
            .control
            .set_deadline(deadline_monotonic_ms)
            .map_err(RequestFailure::poison_without_terminal)?;
        self.execute_contained_task_with_lease(
            &task_request_message,
            &validated,
            &instance_alias,
            connection_id,
            prepared,
            prerequisites,
            interrupts,
            &task_request,
            token,
            task_id,
            run_id,
            execution_provenance,
            Some(run_links),
            active_run.control(),
            Some(control_request_id),
        )
    }

    pub(super) fn run_scheduled_contained_task(
        &self,
        context: &PolicyRunContext,
        task_request: &ContainedTaskRequest,
    ) -> Result<(RuntimeRequest, OperationSuccess), RequestFailure> {
        // Workflow #336 L6: the settlement reads what this path resolves (§5.2.1 last item).
        self.register_scheduled_resolution(context.decision_id())?;
        lock(self.policy()?, HostLocation::ValidatePolicyRunContext)?
            .validate_run_context(context)
            .map_err(|error| {
                if error.is_fatal() {
                    RequestFailure::poison_without_terminal(error)
                } else {
                    RequestFailure::request(error, RuntimeReceiptState::Denied, None)
                }
            })?;
        let token = context.lease_token();
        let RuntimeOperation::AcquireLease {
            instance_alias,
            holder_id,
        } = context.request().operation()
        else {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    HostCode::PolicyRunRequestInvalid,
                    HostLocation::RunScheduledContainedTask,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            ));
        };
        context.package_digest().validate().map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                HostCode::PolicyRunPackageDigestInvalid,
                HostLocation::RunScheduledContainedTask,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })?;
        let expected_sha256 = context.package_digest();
        if context.correlation_id() != context.request().correlation_id()
            || context.instance_alias() != instance_alias
            || context.lease_token().owner_epoch() != self.owner_epoch
            || context.lease_token().holder_id() != *holder_id
            || task_request.expected_sha256() != expected_sha256
        {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    HostCode::PolicyRunIdentityMismatch,
                    HostLocation::RunScheduledContainedTask,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            ));
        }
        let resolved = self.resolve_instance(instance_alias)?;
        if resolved.instance_id() != token.instance_id() {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    HostCode::PolicyRunLeaseInstanceMismatch,
                    HostLocation::RunScheduledContainedTask,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            ));
        }
        let execution_provenance = resolved.provenance();
        let (task_actor, task_source) = scheduled_request_transport_origin(execution_provenance);
        let request_id = self
            .events
            .issuer()
            .mint_request_id()
            .map_err(|_| RequestFailure::poison_without_terminal(runtime_identifier_error()))?;
        let task_request_message = RuntimeRequest::new(
            request_id,
            context.issued_correlation_id(),
            None,
            task_actor,
            task_source,
            unix_ms_now().map_err(RequestFailure::poison_without_terminal)?,
            RuntimeOperation::RunContainedTask {
                instance_alias: instance_alias.clone(),
                holder_id: *holder_id,
                request: task_request.clone(),
            },
        )
        .map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                HostCode::PolicyTaskRequestInvalid,
                HostLocation::RunScheduledContainedTask,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })?;
        let validated = task_request_message.validate().map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                HostCode::PolicyTaskRequestInvalid,
                HostLocation::RunScheduledContainedTask,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })?;
        let connection_id = ConnectionId::new(POLICY_CONNECTION_VALUE).map_err(|error| {
            RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                HostLocation::BuildPolicyConnection,
                &error,
            ))
        })?;
        self.require_business_capacity(self.events.request_links(
            &validated,
            Some(resolved.instance_id()),
            Some(token.lease_id()),
            None,
        ))?;
        // Workflow #336 PKG-S1: every package is read by its path, under the run's deadline. That
        // deadline is bounded by the lease, so the lease is fenced first: a run whose lease has
        // expired is `lease_mismatch`, never a deadline that has passed (host gate key 29).
        self.validated_instance(&validated, token, connection_id)?;
        let source_deadline = self.contained_task_deadline(task_request, token)?;
        let material_deadline = self.package_material_deadline(source_deadline)?;
        // Workflow #336 L2b: a prerequisite package's admission deadline is the run's own,
        // derived when its layer is admitted.
        let prerequisite_deadline = || {
            self.contained_task_deadline(task_request, token)
                .and_then(|deadline| self.package_material_deadline(deadline))
        };
        let prepared = if execution_provenance == ExecutionBackendProvenance::PhysicalDevice {
            Some(self.prepare_scheduled_package(
                context.decision_id(),
                instance_alias,
                task_request,
                material_deadline,
                prerequisite_deadline,
            )?)
        } else {
            None
        };
        let admitted_instance = self.validated_instance(&validated, token, connection_id)?;
        if admitted_instance.instance_id() != resolved.instance_id()
            || admitted_instance.provenance() != execution_provenance
        {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    HostCode::PolicyRunExecutionIdentityMismatch,
                    HostLocation::RunScheduledContainedTask,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            ));
        }
        let active_run = self.begin_contained_run(
            task_request_message.request_id(),
            resolved.instance_id(),
            false,
        )?;
        // Workflow #369 H-1 (review M-1): only this path gives its failed lease end a ladder
        // trigger.
        active_run
            .control
            .note_ladder_source(super::recovery_ladder::LadderSource {
                correlation_id: context.issued_correlation_id(),
                recovery: task_request.recovery().cloned(),
                direct: false,
            })
            .map_err(RequestFailure::poison_without_terminal)?;
        active_run
            .control
            .set_deadline(
                self.monotonic_ms()
                    .and_then(|now| {
                        now.checked_add(task_request.response_deadline_ms())
                            .ok_or_else(|| {
                                RuntimeHostError::fatal(
                                    HostCode::ContainedTaskDeadlineOverflow,
                                    HostLocation::DeriveContainedTaskDeadline,
                                    RuntimeErrorCode::RuntimeHalted,
                                )
                            })
                    })
                    .map_err(RequestFailure::poison_without_terminal)?,
            )
            .map_err(RequestFailure::poison_without_terminal)?;
        active_run
            .control
            .set_deadline(source_deadline)
            .map_err(RequestFailure::poison_without_terminal)?;
        let (prepared, prerequisites) = match prepared {
            Some(prepared) => prepared,
            None => self.prepare_scheduled_package(
                context.decision_id(),
                instance_alias,
                task_request,
                material_deadline,
                prerequisite_deadline,
            )?,
        };
        // Workflow #373 NAV-6: the run's shared interrupt pages, read in its preparation.
        let interrupts = self.prepare_run_interrupts(&prepared)?;
        let expected_outcome_keys = lock(
            self.policy()?,
            HostLocation::ValidatePolicyOutcomeDeclaration,
        )?
        .referenced_outcome_keys(context)
        .map_err(|error| {
            if error.is_fatal() {
                RequestFailure::poison_without_terminal(error)
            } else {
                RequestFailure::request(error, RuntimeReceiptState::Denied, None)
            }
        })?;
        let declared_outcome_keys = prepared
            .scheduling_outcome()
            .into_iter()
            .flat_map(|declaration| declaration.mappings())
            .map(|mapping| mapping.outcome_key().to_owned())
            .collect::<BTreeSet<_>>();
        // Workflow #390 OUT-1 (FCM O-03): the catalog may reference a subset of the outcomes the
        // package declares; it is refused only for a key the package does not declare.
        if !expected_outcome_keys.is_subset(&declared_outcome_keys) {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    HostCode::PolicyRunOutcomeDeclarationMismatch,
                    HostLocation::RunScheduledContainedTask,
                    RuntimeErrorCode::InvalidRequest,
                ),
                RuntimeReceiptState::Denied,
                None,
            ));
        }
        let run_links = RuntimeRunLinks::new(context.issued_task_id(), context.issued_run_id());
        self.append_scheduled_request_lifecycle(
            &task_request_message,
            &validated,
            resolved.instance_id(),
            run_links,
            execution_provenance,
        )?;
        let deadline_monotonic_ms = match self.contained_task_deadline(task_request, token) {
            Ok(deadline) => deadline,
            Err(failure) => {
                return Err(self.cleanup_composite_failure_with_run_links(
                    &validated,
                    token.clone(),
                    connection_id,
                    Some(run_links),
                    failure,
                ));
            }
        };
        active_run
            .control
            .set_deadline(deadline_monotonic_ms)
            .map_err(RequestFailure::poison_without_terminal)?;
        let executed = self.execute_contained_task_with_lease(
            &task_request_message,
            &validated,
            instance_alias,
            connection_id,
            prepared,
            prerequisites,
            interrupts,
            task_request,
            token.clone(),
            context.issued_task_id(),
            context.issued_run_id(),
            execution_provenance,
            Some(run_links),
            active_run.control(),
            Some(context.request().request_id()),
        );
        // Slice #316-B4: a scheduled run has no client receipt; its failure returns to the
        // policy driver on this thread. Workflow #369 H-1: its ladder, if any, already holds
        // the instance: the failed lease end handed the key over.
        Ok((task_request_message, executed?))
    }

    #[allow(clippy::too_many_arguments, clippy::let_and_return)]
    fn execute_contained_task_with_lease(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        connection_id: ConnectionId,
        prepared: PreparedContainedTask,
        prerequisites: Vec<PreparedContainedTask>,
        interrupts: Option<Box<super::standard_pack::RunInterrupts>>,
        task_request: &ContainedTaskRequest,
        token: LeaseToken,
        task_id: IssuedTaskId,
        run_id: IssuedRunId,
        execution_provenance: ExecutionBackendProvenance,
        run_links: Option<RuntimeRunLinks>,
        control: Arc<ContainedRunControl>,
        admission_request_id: Option<RequestId>,
    ) -> Result<OperationSuccess, RequestFailure> {
        let scheduled = run_links.is_some();
        // Workflow #373 NAV-6: the steps the run's own layers can execute.
        let planned_steps = prerequisites
            .iter()
            .chain(std::iter::once(&prepared))
            .fold(0_u32, |planned, package| {
                planned.saturating_add(package.maximum_executed_steps())
            });
        // Workflow #332 H2f M1: a manual run's release also carries its task/run links, so
        // its run scope closes in its own epoch; `scheduled` keeps the original meaning.
        let release_links = run_links.or(Some(RuntimeRunLinks::new(task_id, run_id)));
        let scheduling_outcome = prepared
            .scheduling_outcome()
            .cloned()
            .map(|declaration| (prepared.game().to_owned(), declaration));
        let expected_stability_declaration = prepared.stability_termination().cloned();
        // R25-2: a `linear_steps` main package's non-poisoning device backend failure is rerun
        // only, so its terminal is not severe (`terminal_failure_severe`).
        let linear_main = prepared.execution_mode() == "linear_steps";
        let expects_post_admission_ocr = prepared.has_post_admission_ocr();
        let sampling_run_seed = Some(
            contained_task_sampling_seed(&("xorshift64_uniform_rect_v1/run", run_id.transport()))
                .map_err(RequestFailure::poison_without_terminal)?,
        );
        control
            .note_package(&prepared)
            .map_err(RequestFailure::poison_without_terminal)?;
        let mut runtime = RuntimeContainedTask {
            host: self,
            request,
            token: &token,
            instance_alias,
            connection_id,
            task_id,
            run_id,
            execution_provenance,
            control: Arc::clone(&control),
            declared_game_server: (prepared.game().to_owned(), prepared.server().to_owned()),
            last_frame_id: None,
            last_frame_captured_at: None,
            reading_frames: Vec::new(),
            joined_reading_frames: 0,
            taken_resource_readings: Vec::new(),
            entry_task_label: prepared.task_label().to_owned(),
            last_step_finished: None,
            adjustment_confirmations: Vec::new(),
            joined_adjustment_confirmations: 0,
            taken_adjustments: Vec::new(),
            adjustment_plan: None,
            diagnostic_frame: None,
            geometry_session: None,
            geometry_frame: None,
            geometry_initial: None,
            geometry_deadline: None,
            geometry_rechecked: false,
            input_step_action_id: None,
            post_input_action_id: None,
            pending_input_floor: None,
            last_frame_sequence: None,
            last_capture_input_action_id: None,
            last_capture_acquire_us: None,
            expected_stability_declaration,
            stability: None,
            expects_post_admission_ocr,
            collects_ocr_fields: prepared.has_ocr_fields(),
            post_admission_ocr_observations: 0,
            post_admission_ocr_comparison_recorded: false,
            current_recognition_id: None,
            step_actions: BTreeMap::new(),
            step_index_offset: 0,
            executed_steps: Some(0),
            interrupts,
            interrupt_steps: 0,
            interrupt_configurations: 0,
            planned_steps,
            next_free_step_index: 0,
            step_shift: 0,
            pinned_step: None,
            interrupt_layer: false,
            entry_preflight_recorded: false,
            sampling_run_seed,
            used_action_seeds: BTreeSet::new(),
            finalizing: None,
            capture_evidence: CaptureEvidenceAccumulator::default(),
            configuration_records: 0,
            configuration_capture_recorded: false,
            configuration_input_recorded: false,
            diagnostic_stream: None,
            diagnostic_records: 0,
            task_timing: task_timing::TaskTimingObserver::new(
                control.request_id,
                admission_request_id,
                request.correlation_id(),
                *task_id.transport(),
                *run_id.transport(),
            ),
            diagnostic_step: None,
            diagnostic_physical: None,
            unwritten_selection_record: None,
        };
        // Zero-input fields confirm the required entry in the interpreter's first capture.
        let mut execution = if let Err(failure) = runtime
            .begin_diagnostic()
            .and_then(|()| runtime.record_initial_configuration(task_request, &prepared))
        {
            Err(ContainedTaskRunError::Boundary(failure))
        } else if !prerequisites.is_empty() {
            // Workflow #336 L2b: only a `linear_steps` package has prerequisite packages, and
            // it has no required home entry page.
            prerequisite::run_linear_gated(&prepared, &prerequisites, &mut runtime)
        } else if prepared.required_entry_pages().is_some()
            && !(prepared.has_post_admission_ocr() && prepared.maximum_executed_steps() == 0)
        {
            self.run_preflighted_contained_task(
                instance_alias,
                task_request,
                &prepared,
                &mut runtime,
            )
        } else {
            let execution = prepared.run(&mut runtime);
            // Workflow #373 NAV-6: the run's steps include those of its interrupt layers.
            execution.and_then(|outcome| runtime.with_layer_end(outcome))
        };
        // Workflow #308: a select record that could not be written after the confirmation
        // capture ended the run is already in the ledger as its own failure; the run's failure
        // keeps its code, carries its poison, and joins it only when the ledger refused it.
        if let Some((poison, unrecorded)) = runtime.unwritten_selection_record.take() {
            match &mut execution {
                Err(
                    ContainedTaskRunError::Boundary(failure)
                    | ContainedTaskRunError::NonfatalOperation(failure),
                ) => {
                    if let Some(record_error) = &unrecorded {
                        *failure.error = failure
                            .error
                            .as_ref()
                            .clone()
                            .with_related_failure("selection_record", record_error);
                    }
                    failure.poison_runtime |= poison || unrecorded.is_some();
                }
                // The interpreter returns a run-ending error whenever it leaves a select record
                // to the runtime; anything else is an invariant breach and ends the runtime.
                _ => {
                    execution = Err(ContainedTaskRunError::Boundary(
                        RequestFailure::poison_without_terminal(unrecorded.unwrap_or_else(|| {
                            RuntimeHostError::fatal(
                                HostCode::ContainedTaskSelectionRecordUnreturned,
                                ContractLocation::RunContainedTask,
                                RuntimeErrorCode::RuntimeHalted,
                            )
                        })),
                    ));
                }
            }
        }
        if let Err(ContainedTaskRunError::Task(error)) = &execution {
            runtime
                .task_timing
                .task_failure(error.timing(), error.timing_check_position());
        }
        runtime.recheck_task_geometry(&mut execution);
        runtime.task_timing.begin_finalization();
        let post_admission_ocr_failure_diagnostic = match &execution {
            Err(ContainedTaskRunError::Task(error)) => {
                runtime.record_post_admission_ocr_failure(error.code().as_str(), error.detail())
            }
            _ => Ok(()),
        };
        let fatal = post_admission_ocr_failure_diagnostic.is_err()
            || matches!(&execution,
                Err(ContainedTaskRunError::Boundary(failure) | ContainedTaskRunError::NonfatalOperation(failure))
                    if failure.poison_runtime || failure.error.is_fatal());
        let capacity_refused = matches!(&execution,
            Err(ContainedTaskRunError::Boundary(failure) | ContainedTaskRunError::NonfatalOperation(failure))
                if failure.error.code() == SharedCode::CapacityAdmissionRefused.as_str()
                    && !failure.error.is_fatal());
        // The refusal already carries its Ledger fact reference. Abort the unpublished
        // diagnostic if its new bytes were refused; task terminal/settlement still run.
        let diagnostic_result = if fatal || capacity_refused {
            runtime.abort_diagnostic()
        } else {
            runtime.finish_diagnostic(&execution)
        };
        if let Err(mut failure) = diagnostic_result {
            failure.error = Box::new(match &execution {
                Err(ContainedTaskRunError::Task(error)) => {
                    let primary = match &post_admission_ocr_failure_diagnostic {
                        Err(prior) => prior
                            .error
                            .as_ref()
                            .clone()
                            .with_related_failure("diagnostic_cleanup", &failure.error),
                        Ok(()) => failure.error.as_ref().clone(),
                    };
                    primary.with_related_failure(
                        "prior_task",
                        &RuntimeHostError::relayed_request(
                            error.code().as_str(),
                            ContractLocation::RunContainedTask.as_str(),
                            RuntimeErrorCode::BackendOperationFailed,
                        ),
                    )
                }
                Err(
                    ContainedTaskRunError::Boundary(error)
                    | ContainedTaskRunError::NonfatalOperation(error),
                ) if error.error.is_fatal() => error
                    .error
                    .as_ref()
                    .clone()
                    .with_related_failure("diagnostic_cleanup", &failure.error)
                    .into_fatal(),
                Err(
                    ContainedTaskRunError::Boundary(error)
                    | ContainedTaskRunError::NonfatalOperation(error),
                ) => failure
                    .error
                    .as_ref()
                    .clone()
                    .with_related_failure("prior_task", &error.error),
                Ok(_) => *failure.error,
            });
            if let Err(cleanup) = runtime.abort_diagnostic() {
                failure.error = Box::new(
                    failure
                        .error
                        .as_ref()
                        .clone()
                        .with_related_failure("diagnostic_cleanup", &cleanup.error),
                );
            }
            execution = Err(ContainedTaskRunError::Boundary(failure));
        }
        let task_timing = runtime.task_timing.snapshot();
        if let Err(
            ContainedTaskRunError::Boundary(failure)
            | ContainedTaskRunError::NonfatalOperation(failure),
        ) = &mut execution
        {
            failure.error.lifecycle.task_timing = Some(task_timing.clone());
        }
        let finalizing = runtime.finalizing;
        let executed_steps = runtime.executed_steps;
        let taken_resource_readings = std::mem::take(&mut runtime.taken_resource_readings);
        // Workflow #384 D2-S3: the plan of the run's adjustments and their confirmation frames.
        let adjustment_plan = runtime.adjustment_plan.take();
        let confirmation_frames = runtime
            .adjustment_confirmations
            .iter()
            .map(|(_, frame_id, _)| *frame_id)
            .collect::<Vec<_>>();
        let mut capture_evidence = std::mem::take(&mut runtime.capture_evidence);
        drop(runtime);
        let outcome = match execution {
            Ok(outcome) => outcome,
            Err(
                ContainedTaskRunError::Boundary(mut failure)
                | ContainedTaskRunError::NonfatalOperation(mut failure),
            ) => {
                if matches!(
                    failure.error.projection().code,
                    RuntimeErrorCode::ContainedTaskDeadlineExceeded
                        | RuntimeErrorCode::ContainedTaskCancelled
                        | RuntimeErrorCode::ContainedTaskPaused
                ) {
                    let reason = control
                        .cancellation_reason(
                            self.monotonic_ms()
                                .map_err(RequestFailure::poison_without_terminal)?,
                        )
                        .ok_or_else(|| {
                            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                                HostCode::ContainedTaskCancellationStateMissing,
                                HostLocation::FinalizeContainedTaskCancellation,
                                RuntimeErrorCode::RuntimeHalted,
                            ))
                        })?;
                    let terminal_outcome = if scheduled {
                        TaskOutcome::Failure
                    } else {
                        TaskOutcome::Cancelled
                    };
                    let capture_summary = capture_evidence
                        .finalize(terminal_outcome, self)
                        .map_err(|summary_failure| {
                            self.cleanup_composite_failure_with_run_links(
                                request,
                                token.clone(),
                                connection_id,
                                run_links,
                                summary_failure,
                            )
                        })?;
                    let task_terminal = self.append_contained_task_terminal(
                        request,
                        &token,
                        ContainedTaskTerminalDraft {
                            task_id,
                            run_id,
                            outcome: terminal_outcome,
                            intent_already_recorded: finalizing.is_some(),
                            final_page: None,
                            executed_steps,
                            failure: Some(failure.error.outcome()),
                            failure_severe: false,
                            scheduling_outcome: None,
                            selected_scheduling_outcome: None,
                            designated_effect_caps: Vec::new(),
                            capture_summary: Some(capture_summary),
                            task_timing: Some(task_timing.clone()),
                            resource_readings: None,
                        },
                    )?;
                    if scheduled {
                        failure.terminal = Some(terminal(&task_terminal));
                        failure.task_failure = Some(TaskFailureEvidence {
                            outcome: failure.error.outcome(),
                            severe: false,
                        });
                        return Err(self.cleanup_composite_failure_with_run_links(
                            request,
                            token,
                            connection_id,
                            run_links,
                            failure,
                        ));
                    }
                    if let Err(release_failure) = self.release_lease(
                        request,
                        original.request_id(),
                        &token,
                        connection_id,
                        release_links,
                    ) {
                        return Err(self.cleanup_composite_failure_with_run_links(
                            request,
                            token,
                            connection_id,
                            run_links,
                            release_failure,
                        ));
                    }
                    return Ok(OperationSuccess {
                        state: RuntimeReceiptState::Cancelled,
                        terminal: Some(terminal(&task_terminal)),
                        result: RuntimeResult::ContainedTaskCancelled {
                            run_id: *run_id.transport(),
                            task_id: *task_id.transport(),
                            task_request_id: original.request_id(),
                            response_deadline_monotonic_ms: Some(control.deadline()),
                            reason,
                            lease_terminal: Some(ContainedTaskLeaseTerminal::Released),
                        },
                        failure_outcome: None,
                    });
                }
                let task_failure = if scheduled {
                    failure.task_failure.clone()
                } else {
                    None
                };
                let (task_outcome, severe) = match &task_failure {
                    Some(evidence) => (evidence.outcome.clone(), evidence.severe),
                    None => (
                        failure.error.outcome(),
                        failure.poison_runtime || failure.error.is_fatal(),
                    ),
                };
                let failure_severe = terminal_failure_severe(
                    severe,
                    &task_outcome,
                    failure.poison_runtime,
                    linear_main,
                );
                if task_failure.is_some() || !scheduled && !failure.poison_runtime {
                    let capture_summary =
                        match capture_evidence.finalize(TaskOutcome::Failure, self) {
                            Ok(summary) => summary,
                            Err(summary_failure) => {
                                return Err(self.cleanup_composite_failure_with_run_links(
                                    request,
                                    token,
                                    connection_id,
                                    run_links,
                                    summary_failure,
                                ));
                            }
                        };
                    let event = self.append_contained_task_terminal(
                        request,
                        &token,
                        ContainedTaskTerminalDraft {
                            task_id,
                            run_id,
                            outcome: TaskOutcome::Failure,
                            intent_already_recorded: finalizing.is_some(),
                            final_page: None,
                            executed_steps,
                            failure: Some(task_outcome),
                            failure_severe,
                            scheduling_outcome: None,
                            selected_scheduling_outcome: None,
                            designated_effect_caps: Vec::new(),
                            capture_summary: Some(capture_summary),
                            task_timing: Some(task_timing.clone()),
                            resource_readings: None,
                        },
                    )?;
                    failure.terminal = Some(terminal(&event));
                    if failure.error.lifecycle.capacity.is_none()
                        && task_failure.as_ref().is_none_or(|evidence| {
                            evidence.outcome.code().as_str() == failure.error.code()
                        })
                    {
                        let _ = failure
                            .error
                            .diagnostics()
                            .recorded_event()
                            .set(*event.event_id());
                    }
                }
                return Err(self.cleanup_composite_failure_with_run_links(
                    request,
                    token,
                    connection_id,
                    run_links,
                    failure,
                ));
            }
            Err(ContainedTaskRunError::Task(error)) => {
                if spelled(
                    error.code().as_str(),
                    &[
                        ExecutionKernelCode::ContainedTaskGuardRefused.code(),
                        ExecutionKernelCode::ContainedTaskGuardTargetMissing.code(),
                        ExecutionKernelCode::ContainedTaskGuardTargetInvalid.code(),
                        SharedCode::ContainedTaskGuardEvaluationFailed.code(),
                    ],
                ) && let Err(pin_failure) =
                    capture_evidence.pin_last(PinnedFrameReason::GuardRejection)
                {
                    return Err(self.cleanup_composite_failure_with_run_links(
                        request,
                        token,
                        connection_id,
                        run_links,
                        pin_failure,
                    ));
                }
                let capture_summary = match capture_evidence.finalize(TaskOutcome::Failure, self) {
                    Ok(summary) => summary,
                    Err(summary_failure) => {
                        return Err(self.cleanup_composite_failure_with_run_links(
                            request,
                            token,
                            connection_id,
                            run_links,
                            summary_failure,
                        ));
                    }
                };
                let mut task_error = RuntimeHostError::relayed_request(
                    error.code().as_str(),
                    ContractLocation::RunContainedTask.as_str(),
                    if error.code().as_str()
                        == SharedCode::ApplicationEffectRequiresAssignedApplication.as_str()
                    {
                        RuntimeErrorCode::InvalidRequest
                    } else {
                        RuntimeErrorCode::BackendOperationFailed
                    },
                );
                // Workflow #386 SR-1: a kernel failure that carries the values its code declares (a
                // sampling timeout's `attempts`, `timeout_ms` and `stage`; a sampling budget's
                // `required_ms` and `remaining_ms`) keeps them on the terminal and the receipt.
                if error.has_values() {
                    let values = error.outcome();
                    task_error = task_error.with_outcome_values(|_| {
                        values.operation(ContractLocation::RunContainedTask)
                    });
                }
                // Ledger revision 3: the terminal carries the task failure as its outcome.
                let task_outcome = task_error.outcome();
                let event = self.append_contained_task_terminal(
                    request,
                    &token,
                    ContainedTaskTerminalDraft {
                        task_id,
                        run_id,
                        outcome: TaskOutcome::Failure,
                        intent_already_recorded: finalizing.is_some(),
                        final_page: None,
                        executed_steps,
                        failure: Some(task_outcome.clone()),
                        failure_severe: false,
                        scheduling_outcome: None,
                        selected_scheduling_outcome: None,
                        designated_effect_caps: Vec::new(),
                        capture_summary: Some(capture_summary),
                        task_timing: Some(task_timing.clone()),
                        resource_readings: None,
                    },
                )?;
                // Workflow #336 L2d: the page-graph home entry's refusal of a configured
                // return-home package carries its detail as a linear failure does. Workflow
                // #371-2: so does a page-graph step whose confirmation failed.
                let detailed = prepared.execution_mode() == "linear_steps"
                    || error.code().as_str() == prerequisite::INCOMPATIBLE
                    || spelled(
                        error.code().as_str(),
                        &[
                            ExecutionKernelCode::ContainedTaskStepUnconfirmed.code(),
                            ExecutionKernelCode::ContainedTaskErrorPageReached.code(),
                            ExecutionKernelCode::PageConfirmationFailed.code(),
                        ],
                    );
                if let Some(detail) =
                    resource_reading_failure_detail(error.code().as_str(), error.detail())
                {
                    task_error = task_error.with_native_detail(detail);
                } else if detailed && let Some(detail) = error.detail() {
                    // Workflow #336 L2c: a `linear_steps` package's failure always carries the
                    // kernel's detail.
                    task_error = task_error.with_native_detail(detail.to_owned());
                }
                let mut failure = RequestFailure::request(
                    task_error,
                    if error.code().as_str()
                        == SharedCode::ApplicationEffectRequiresAssignedApplication.as_str()
                    {
                        RuntimeReceiptState::Denied
                    } else {
                        RuntimeReceiptState::Failed
                    },
                    Some(terminal(&event)),
                );
                failure.task_failure = scheduled.then_some(TaskFailureEvidence {
                    outcome: task_outcome,
                    severe: false,
                });
                // Workflow #336 L2c: a linear failure with a detail also gets the run's runtime
                // lifecycle failure record, which carries the detail and names the terminal, as
                // an outcome with an extra native detail does, at the terminal's severity;
                // otherwise the terminal alone records the failure.
                let recorded = if detailed {
                    self.record_required_failure_with_severity(
                        &failure.error,
                        &event,
                        self.events
                            .request_links(
                                request,
                                Some(token.instance_id()),
                                Some(token.lease_id()),
                                None,
                            )
                            .with_task_id(task_id)
                            .with_run_id(run_id),
                        Some(event.severity()),
                    )
                } else {
                    let _ = failure
                        .error
                        .diagnostics()
                        .recorded_event()
                        .set(*event.event_id());
                    Ok(())
                };
                let failure = match post_admission_ocr_failure_diagnostic {
                    Ok(()) => failure,
                    Err(diagnostic_failure) => {
                        failure.replace_with_poison(*diagnostic_failure.error)
                    }
                };
                let failure = match recorded {
                    Ok(()) => failure,
                    Err(record_error) => failure.replace_with_poison(record_error),
                };
                return Err(self.cleanup_composite_failure_with_run_links(
                    request,
                    token,
                    connection_id,
                    run_links,
                    failure,
                ));
            }
        };
        if finalizing != Some(outcome.outcome) {
            return Err(self.cleanup_composite_failure_with_run_links(
                request,
                token,
                connection_id,
                run_links,
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    HostCode::ContainedTaskFinalizingStateInvalid,
                    ContractLocation::RunContainedTask,
                    RuntimeErrorCode::RuntimeHalted,
                )),
            ));
        }
        let capture_summary = match capture_evidence.finalize(outcome.outcome, self) {
            Ok(summary) => summary,
            Err(failure) => {
                return Err(self.cleanup_composite_failure_with_run_links(
                    request,
                    token,
                    connection_id,
                    run_links,
                    failure,
                ));
            }
        };
        let task_terminal = self.append_contained_task_terminal(
            request,
            &token,
            ContainedTaskTerminalDraft {
                task_id,
                run_id,
                outcome: outcome.outcome,
                intent_already_recorded: true,
                final_page: outcome.final_page.clone(),
                executed_steps: Some(outcome.executed_steps),
                failure: None,
                failure_severe: false,
                scheduling_outcome,
                selected_scheduling_outcome: outcome.selected_scheduling_outcome,
                designated_effect_caps: outcome.designated_effect_caps,
                capture_summary: Some(capture_summary),
                task_timing: Some(task_timing),
                resource_readings: (!taken_resource_readings.is_empty()
                    || adjustment_plan.is_some())
                .then(|| ContainedTaskResourceReadings {
                    instance_alias: instance_alias.to_owned(),
                    task_label: prepared.task_label().to_owned(),
                    resource_bundle_hash: resource_reading_bundle_hash(prepared.package_label()),
                    frames: taken_resource_readings,
                    confirmation_frames,
                    adjustments: adjustment_plan,
                }),
            },
        )?;
        match self.release_lease(
            request,
            original.request_id(),
            &token,
            connection_id,
            release_links,
        ) {
            Ok(_) => Ok(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: Some(terminal(&task_terminal)),
                result: RuntimeResult::ContainedTaskCompleted {
                    run_id: *run_id.transport(),
                    task_id: *task_id.transport(),
                    task_request_id: original.request_id(),
                    response_deadline_monotonic_ms: Some(control.deadline()),
                    task_result: outcome.outcome,
                    final_page: outcome.final_page,
                    executed_steps: outcome.executed_steps,
                },
                failure_outcome: None,
            }),
            Err(failure) => Err(self.cleanup_composite_failure_with_run_links(
                request,
                token,
                connection_id,
                run_links,
                failure,
            )),
        }
    }

    fn run_preflighted_contained_task(
        &self,
        instance_alias: &str,
        task_request: &ContainedTaskRequest,
        prepared: &PreparedContainedTask,
        runtime: &mut RuntimeContainedTask<'_>,
    ) -> Result<ContainedTaskOutcome, ContainedTaskRunError<RequestFailure>> {
        // Workflow #373 NAV-3b: the required entry is one page or an entry page set.
        let Some(required_pages) = prepared.required_entry_pages().map(<[String]>::to_vec) else {
            return prepared
                .run(runtime)
                .and_then(|outcome| runtime.with_layer_end(outcome));
        };

        let initial_page = prepared.recognize_required_entry(runtime)?;
        let initial_home = initial_page.is_some();
        runtime
            .record_entry_fact(TaskSemanticFact::EntryRecognition {
                phase: TaskEntryRecognitionPhase::Initial,
                required_pages: required_pages.clone(),
                matched_page: initial_page,
                matched: initial_home,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        runtime
            .record_entry_fact(TaskSemanticFact::EntryRecoveryDecision {
                required: !initial_home,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        runtime.entry_preflight_recorded = true;
        if initial_home {
            runtime
                .record_entry_fact(TaskSemanticFact::EntryTargetDisposition {
                    disposition: TaskEntryTargetDisposition::Started,
                    outcome: None,
                })
                .map_err(ContainedTaskRunError::Boundary)?;
            return prepared
                .run(runtime)
                .and_then(|outcome| runtime.with_layer_end(outcome));
        }

        // Workflow #336 L2d (R23): a request that binds no recovery package takes the
        // return-home package actingd configures for the package's game and server.
        let (binding, configured) = match task_request.recovery() {
            Some(binding) => (binding, false),
            None => match self
                .configured_return_home(prepared.game(), prepared.server())
                .map_err(RequestFailure::poison_without_terminal)
                .map_err(ContainedTaskRunError::Boundary)?
            {
                Some(binding) => (binding, true),
                None => {
                    return fail_contained_task_entry(
                        runtime,
                        HostCode::ContainedTaskHomeRecoveryBindingMissing.as_str(),
                    );
                }
            },
        };
        let recovery_request = ContainedTaskRequest::new(
            binding.package_path(),
            binding.expected_sha256(),
        )
        .map_err(|_| {
            ContainedTaskRunError::task(HostCode::ContainedTaskHomeRecoveryBindingInvalid.as_str())
        })?;
        let recovery = match prepare_contained_task(
            instance_alias,
            &recovery_request,
            self.execution()
                .map_err(|error| ContainedTaskRunError::Boundary(RequestFailure::from(error)))?
                .vision_provider(),
            self.package_material_deadline(runtime.control.deadline())
                .map_err(ContainedTaskRunError::Boundary)?,
        ) {
            Ok(recovery) => recovery,
            Err(mut failure) => {
                let code = failure.error.code();
                let outcome = failure.error.outcome();
                // Workflow #336 PKG-S2 (option 丙): the refused package's `package_id`, when it
                // is known: the configured return-home package's, or the one its refused
                // declaration read. A request's own binding names only a path.
                let package_label = if configured {
                    self.return_home_packages
                        .get(&(prepared.game().to_owned(), prepared.server().to_owned()))
                        .cloned()
                } else {
                    failure
                        .error
                        .resource_declaration()
                        .and_then(|rejection| rejection.package_label.clone())
                };
                runtime
                    .record_entry_fact(TaskSemanticFact::EntryRecoveryFailed {
                        package_label,
                        outcome: outcome.clone(),
                        role: None,
                    })
                    .map_err(ContainedTaskRunError::Boundary)?;
                if let Some(rejection) = failure.error.resource_declaration().cloned() {
                    let links = runtime.links();
                    let event = self
                        .append_event(
                            EventSeverity::Warning,
                            EventSource::Runtime,
                            OriginModule::Runtime,
                            EventActor::Runtime,
                            links.clone(),
                            RuntimePayloadDraft::resource_declaration_rejected(rejection),
                        )
                        .map_err(ContainedTaskRunError::Boundary)?;
                    self.record_required_failure(&failure.error, &event, links)
                        .map_err(RequestFailure::poison_without_terminal)
                        .map_err(ContainedTaskRunError::Boundary)?;
                    failure.error.lifecycle.resource_declaration_event = Some(terminal(&event));
                    runtime
                        .record_entry_fact(TaskSemanticFact::EntryTargetDisposition {
                            disposition: TaskEntryTargetDisposition::FailClosed,
                            outcome: Some(outcome.clone()),
                        })
                        .map_err(ContainedTaskRunError::Boundary)?;
                    failure.state = RuntimeReceiptState::Failed;
                    failure.task_failure = Some(TaskFailureEvidence {
                        outcome,
                        severe: false,
                    });
                    return Err(ContainedTaskRunError::Boundary(failure));
                }
                return fail_contained_task_entry(runtime, code);
            }
        };
        if configured {
            // A configured package passes the checks of a return-home chain layer (its
            // `scheduling_outcome` without a designated operation allowed and ignored) and
            // declares no prerequisite package; a refusal, like the chain's, starts no ladder.
            if let Some(detail) = prerequisite::configured_return_home_incompatibility(
                &recovery,
                &PackageIdentity::of(prepared),
            ) {
                runtime
                    .record_entry_fact(TaskSemanticFact::EntryTargetDisposition {
                        disposition: TaskEntryTargetDisposition::FailClosed,
                        outcome: Some(Outcome::new(
                            SharedCode::ContainedTaskPrerequisiteIncompatible,
                        )),
                    })
                    .map_err(ContainedTaskRunError::Boundary)?;
                return Err(ContainedTaskRunError::Task(
                    actingcommand_execution_kernel::ContainedTaskError::with_detail(
                        actingcommand_contract::codes::SharedCode::ContainedTaskPrerequisiteIncompatible,
                        detail,
                    ),
                ));
            }
        } else if !recovery.is_entry_recovery_compatible() {
            return fail_contained_task_entry(
                runtime,
                SharedCode::ContainedTaskHomeRecoveryPackageIncompatible.as_str(),
            );
        }
        let recovery_label = recovery.package_label().to_owned();
        runtime
            .record_entry_fact(TaskSemanticFact::EntryRecoveryPackageAdmitted {
                package_label: recovery_label.clone(),
                role: None,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        // Workflow #373 NAV-6: the recovery package's steps follow those of the interrupt
        // layers that ran so far, and the run's own steps are now its and the package's.
        runtime.planned_steps = recovery
            .maximum_executed_steps()
            .saturating_add(prepared.maximum_executed_steps());
        runtime.begin_layer(0).ok_or_else(|| {
            ContainedTaskRunError::task(HostCode::ContainedTaskHomeRecoveryStepLimit.as_str())
        })?;
        let previous_timing = runtime.task_timing.context();
        let recovery_execution = {
            if runtime.configuration_records > 0 {
                runtime
                    .record_configuration(
                        EffectiveConfigurationFacts::EntryRecovery {
                            package_label: recovery_label.clone(),
                            timing: recovery.effective_timing(),
                        },
                        None,
                        None,
                        None,
                    )
                    .map_err(ContainedTaskRunError::Boundary)?;
            }
            let mut recovery_runtime = EntryRecoveryRuntime { inner: runtime };
            if configured {
                recovery.run_as_prerequisite(&mut recovery_runtime)
            } else {
                recovery.run_entry_recovery(&mut recovery_runtime)
            }
        };
        if let Err(ContainedTaskRunError::Task(error)) = &recovery_execution {
            runtime
                .task_timing
                .task_failure(error.timing(), error.timing_check_position());
        }
        runtime.task_timing.replace_context(previous_timing);
        let nonfatal_operation = matches!(
            &recovery_execution,
            Err(ContainedTaskRunError::NonfatalOperation(_))
        );
        let recovery_outcome = match recovery_execution {
            Ok(outcome) => outcome,
            Err(ContainedTaskRunError::Task(error)) => {
                if spelled(
                    error.code().as_str(),
                    &[
                        SharedCode::ContainedTaskRecognitionFailed.code(),
                        SharedCode::ContainedTaskPageUnknown.code(),
                    ],
                ) {
                    let mut primary = RuntimeHostError::relayed_request(
                        error.code().as_str(),
                        ContractLocation::RunContainedTask.as_str(),
                        RuntimeErrorCode::BackendOperationFailed,
                    );
                    if let Some(detail) = error.detail() {
                        primary = primary.with_native_detail(detail.to_owned());
                    }
                    runtime
                        .record_geometry_triggered_recovery_failure(recovery_label, &primary)
                        .map_err(ContainedTaskRunError::Boundary)?;
                    return Err(ContainedTaskRunError::Task(error));
                }
                runtime
                    .record_entry_fact(TaskSemanticFact::EntryRecoveryFailed {
                        package_label: Some(recovery_label),
                        outcome: Outcome::relayed(error.code().as_str()),
                        role: None,
                    })
                    .map_err(ContainedTaskRunError::Boundary)?;
                return fail_contained_task_entry(runtime, error.code().as_str());
            }
            Err(
                ContainedTaskRunError::Boundary(failure)
                | ContainedTaskRunError::NonfatalOperation(failure),
            ) => {
                if nonfatal_operation
                    && !failure.poison_runtime
                    && !failure.error.is_fatal()
                    && spelled(
                        failure.error.code(),
                        &[
                            SharedCode::InputBackendOperationFailed.code(),
                            DeviceSelfCheckFailure::InputBackendOpenFailed.code(),
                        ],
                    )
                {
                    runtime
                        .record_geometry_triggered_recovery_failure(recovery_label, &failure.error)
                        .map_err(ContainedTaskRunError::Boundary)?;
                    return Err(ContainedTaskRunError::NonfatalOperation(failure));
                }
                let outcome = failure.error.outcome();
                runtime
                    .record_entry_fact(TaskSemanticFact::EntryRecoveryFailed {
                        package_label: Some(recovery_label),
                        outcome: outcome.clone(),
                        role: None,
                    })
                    .map_err(ContainedTaskRunError::Boundary)?;
                runtime
                    .record_entry_fact(TaskSemanticFact::EntryTargetDisposition {
                        disposition: TaskEntryTargetDisposition::FailClosed,
                        outcome: Some(outcome),
                    })
                    .map_err(ContainedTaskRunError::Boundary)?;
                return Err(if nonfatal_operation {
                    ContainedTaskRunError::NonfatalOperation(failure)
                } else {
                    ContainedTaskRunError::Boundary(failure)
                });
            }
        };
        let Some(final_page) = recovery_outcome.final_page.clone() else {
            return fail_contained_task_entry(
                runtime,
                HostCode::ContainedTaskHomeRecoveryFinalPageMissing.as_str(),
            );
        };
        runtime
            .record_entry_fact(TaskSemanticFact::EntryRecoveryCompleted {
                package_label: recovery_label,
                final_page: final_page.clone(),
                executed_steps: recovery_outcome.executed_steps,
                role: None,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        // Workflow #336 L2d (R24): a configured `linear_steps` package ends on a Lab page id
        // (`<game>/step_<n>_home`), never the literal home; only the recheck below decides.
        let configured_linear = configured && recovery.execution_mode() == "linear_steps";
        // Workflow #373 NAV-3b: the recovery package must end on a page of the required entry.
        if !configured_linear && !prepared.terminal_in_required_entry(&final_page) {
            return fail_contained_task_entry(
                runtime,
                HostCode::ContainedTaskHomeRecoveryTerminalNonHome.as_str(),
            );
        }

        let post_recovery_page = prepared.recognize_required_entry(runtime)?;
        let post_recovery_home = post_recovery_page.is_some();
        runtime
            .record_entry_fact(TaskSemanticFact::EntryRecognition {
                phase: TaskEntryRecognitionPhase::PostRecovery,
                required_pages,
                matched_page: post_recovery_page,
                matched: post_recovery_home,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        if !post_recovery_home {
            return fail_contained_task_entry(
                runtime,
                HostCode::ContainedTaskHomeRecoveryPersistentlyNonHome.as_str(),
            );
        }
        if recovery_outcome
            .executed_steps
            .checked_add(prepared.maximum_executed_steps())
            .and_then(|maximum| maximum.checked_add(runtime.interrupt_steps))
            .is_none_or(|maximum| maximum > 1_000)
        {
            return fail_contained_task_entry(
                runtime,
                HostCode::ContainedTaskHomeRecoveryStepLimit.as_str(),
            );
        }
        runtime
            .begin_layer(recovery_outcome.executed_steps)
            .ok_or_else(|| {
                ContainedTaskRunError::task(HostCode::ContainedTaskHomeRecoveryStepLimit.as_str())
            })?;
        runtime
            .record_entry_fact(TaskSemanticFact::EntryTargetDisposition {
                disposition: TaskEntryTargetDisposition::Started,
                outcome: None,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        let mut outcome = prepared.run(runtime)?;
        outcome.executed_steps = runtime.layer_end(outcome.executed_steps).ok_or_else(|| {
            ContainedTaskRunError::task(HostCode::ContainedTaskHomeRecoveryStepLimit.as_str())
        })?;
        Ok(outcome)
    }

    /// Workflow #336 PKG-S2 (option 丙): the `package_id` of the package `request` names,
    /// admitted again as a run admits it, for a replay that compares it with the label its run
    /// recorded. Nothing is hashed; the package may have changed in place since.
    fn replayed_package_label(
        &self,
        instance_alias: &str,
        request: &ContainedTaskRequest,
        deadline: Instant,
    ) -> Result<String, RequestFailure> {
        prepare_contained_task(
            instance_alias,
            request,
            self.execution()?.vision_provider(),
            deadline,
        )
        .map(|prepared| prepared.package_label().to_owned())
    }

    fn recover_contained_task(
        &self,
        request: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        instance_id: InstanceId,
        task_request: &ContainedTaskRequest,
    ) -> Result<Option<OperationSuccess>, RequestFailure> {
        let active = lock(&self.contained_runs, HostLocation::ReadActiveContainedRuns)?
            .contains_key(&request.request_id());
        let events = self
            .ledger
            .query(EventQuery {
                request_id: Some(request.request_id()),
                ..EventQuery::default()
            })
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error(
                    HostLocation::RecoverContainedTask,
                ))
            })?;
        if events.is_empty() {
            return if active {
                Err(contained_task_replay_denied(
                    HostCode::ContainedTaskAlreadyRunning,
                ))
            } else {
                Ok(None)
            };
        }
        if events.iter().any(|event| {
            event.links().correlation_id() != Some(&request.correlation_id())
                || event
                    .links()
                    .instance_id()
                    .is_some_and(|actual| actual != &instance_id)
        }) {
            return Err(contained_task_replay_denied(
                HostCode::ContainedTaskRequestIdentityReused,
            ));
        }
        let semantic = events
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => Some((event, payload.fact())),
                _ => None,
            })
            .collect::<Vec<_>>();
        let Some((identity, lease_id)) =
            contained_task_recovery_identity(&events, HostLocation::RecoverContainedTask)?
        else {
            return Err(contained_task_replay_denied(
                HostCode::ContainedTaskPreviousAttemptIncomplete,
            ));
        };
        let packages = semantic
            .iter()
            .filter_map(|(event, fact)| match fact {
                TaskSemanticFact::PackageAdmitted {
                    package_label,
                    response_deadline_monotonic_ms,
                    ..
                } => Some((*event, package_label, *response_deadline_monotonic_ms)),
                _ => None,
            })
            .collect::<Vec<_>>();
        if packages.len() > 1 {
            return Err(contained_task_replay_denied(
                HostCode::ContainedTaskRequestPackageReused,
            ));
        }
        // Workflow #336 PKG-S2 (option 丙): a replay names the same package when the request's
        // package, admitted again, has the `package_id` the run recorded; its content may have
        // changed in place since. A replay is admitted under the request's own deadline.
        let replay_deadline = Instant::now()
            .checked_add(Duration::from_millis(task_request.response_deadline_ms()))
            .ok_or_else(|| {
                contained_task_package_failure(HostCode::ContainedTaskDeadlineOverflow)
            })?;
        if let Some((_, recorded, _)) = packages.first()
            && self.replayed_package_label(instance_alias, task_request, replay_deadline)?
                != **recorded
        {
            return Err(contained_task_replay_denied(
                HostCode::ContainedTaskRequestPackageReused,
            ));
        }
        let recovery_packages = semantic
            .iter()
            .filter_map(|(_, fact)| match fact {
                // Workflow #373 NAV-6: an interrupt layer is no gate package.
                TaskSemanticFact::EntryRecoveryPackageAdmitted {
                    package_label,
                    role: None,
                } => Some(package_label),
                _ => None,
            })
            .collect::<Vec<_>>();
        // Workflow #336 L2b: a prerequisite gate opens at most four distinct packages (three
        // declared and one return-home, L2c), each the request's binding or one of the host's
        // prerequisite packages. Workflow #336 PKG-S2: by `package_id`, a configured package's
        // by its configuration key, the request's binding by its package admitted again.
        if recovery_packages.len() > prerequisite::MAX_PREREQUISITE_DEPTH + 1
            || recovery_packages
                .iter()
                .enumerate()
                .any(|(index, recorded)| recovery_packages[..index].contains(recorded))
        {
            return Err(contained_task_replay_denied(
                HostCode::ContainedTaskRequestRecoveryReused,
            ));
        }
        let mut binding_label = None;
        for recorded in &recovery_packages {
            if self.prerequisite_packages.contains_key(recorded.as_str()) {
                continue;
            }
            let Some(binding) = task_request.recovery() else {
                return Err(contained_task_replay_denied(
                    HostCode::ContainedTaskRequestRecoveryReused,
                ));
            };
            if binding_label.is_none() {
                let binding_request =
                    ContainedTaskRequest::new(binding.package_path(), binding.expected_sha256())
                        .map_err(|_| {
                            contained_task_replay_denied(
                                HostCode::ContainedTaskRequestRecoveryReused,
                            )
                        })?;
                binding_label = Some(self.replayed_package_label(
                    instance_alias,
                    &binding_request,
                    replay_deadline,
                )?);
            }
            if binding_label.as_deref() != Some(recorded.as_str()) {
                return Err(contained_task_replay_denied(
                    HostCode::ContainedTaskRequestRecoveryReused,
                ));
            }
        }
        let deadline_monotonic_ms = packages.first().and_then(|(_, _, deadline)| *deadline);
        let task_id = identity.task_id;
        let run_id = identity.run_id;
        let mut terminals = semantic
            .iter()
            .filter_map(|(event, fact)| match fact {
                TaskSemanticFact::TerminalCommitted {
                    task_result,
                    final_page,
                    executed_steps,
                    outcome,
                    ..
                } => Some((
                    (*event).clone(),
                    *task_result,
                    final_page.clone(),
                    *executed_steps,
                    outcome.as_ref().map(|outcome| outcome.code().clone()),
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        if terminals.len() > 1 {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    HostCode::ContainedTaskTerminalStateInconsistent,
                    HostLocation::RecoverContainedTask,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            ));
        }
        if terminals.is_empty() {
            if active {
                return Err(contained_task_replay_denied(
                    HostCode::ContainedTaskAlreadyRunning,
                ));
            }
            if packages.is_empty() {
                return Err(contained_task_replay_denied(
                    HostCode::ContainedTaskPreviousAttemptIncomplete,
                ));
            }
            let terminal_event = self.append_recovered_contained_task_terminal(
                validated,
                instance_id,
                lease_id,
                task_id,
                run_id,
            )?;
            terminals.push((
                terminal_event,
                TaskOutcome::Cancelled,
                None,
                None,
                Some(CodeStr::from(
                    HostCode::ContainedTaskRecoveredAfterRestart.code(),
                )),
            ));
        }
        let lease_disposition = if let Some(lease_id) = lease_id {
            let lease_events = self
                .ledger
                .query(EventQuery {
                    lease_id: Some(lease_id),
                    ..EventQuery::default()
                })
                .map_err(|_| {
                    RequestFailure::poison_without_terminal(ledger_error(
                        HostLocation::QueryRecoveredContainedTaskLeaseTerminal,
                    ))
                })?;
            let granted = lease_events.iter().any(|event| {
                event.event_type() == EventType::LeaseGranted
                    && event.links().instance_id() == Some(&identity.instance_id)
                    && identity.matches_links(event.links())
            });
            let lease_terminals = lease_events
                .iter()
                .filter(|event| {
                    matches!(
                        event.event_type(),
                        EventType::LeaseReleased | EventType::LeaseExpired
                    ) && event.links().lease_id() == Some(&lease_id)
                })
                .collect::<Vec<_>>();
            if lease_terminals.len() > 1 {
                return Err(RequestFailure::poison_without_terminal(
                    RuntimeHostError::fatal(
                        HostCode::ContainedTaskLeaseTerminalStateInconsistent,
                        HostLocation::RecoverContainedTask,
                        RuntimeErrorCode::RuntimeHalted,
                    ),
                ));
            }
            match lease_terminals.as_slice() {
                [event] if event.event_type() == EventType::LeaseExpired => {
                    Some(ContainedTaskLeaseTerminal::Expired)
                }
                [_] => Some(ContainedTaskLeaseTerminal::Released),
                [] if active => {
                    return Err(contained_task_replay_denied(
                        HostCode::ContainedTaskAlreadyRunning,
                    ));
                }
                [] if granted => {
                    self.append_recovered_contained_task_release(
                        validated,
                        instance_id,
                        lease_id,
                        task_id,
                        run_id,
                    )?;
                    Some(ContainedTaskLeaseTerminal::Released)
                }
                [] => None,
                _ => unreachable!("lease terminal cardinality checked above"),
            }
        } else {
            None
        };
        let [(terminal_event, outcome, final_page, executed_steps, failure_code)] =
            terminals.as_slice()
        else {
            unreachable!("terminal cardinality checked above")
        };
        match outcome {
            TaskOutcome::Success => Ok(Some(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: Some(terminal(terminal_event)),
                result: RuntimeResult::ContainedTaskCompleted {
                    run_id,
                    task_id,
                    task_request_id: request.request_id(),
                    response_deadline_monotonic_ms: deadline_monotonic_ms,
                    task_result: *outcome,
                    final_page: final_page.clone(),
                    executed_steps: executed_steps.ok_or_else(|| {
                        RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                            HostCode::ContainedTaskTerminalStateInconsistent,
                            HostLocation::RecoverContainedTask,
                            RuntimeErrorCode::RuntimeHalted,
                        ))
                    })?,
                },
                failure_outcome: None,
            })),
            TaskOutcome::Cancelled => Ok(Some(OperationSuccess {
                state: RuntimeReceiptState::Cancelled,
                terminal: Some(terminal(terminal_event)),
                result: RuntimeResult::ContainedTaskCancelled {
                    run_id,
                    task_id,
                    task_request_id: request.request_id(),
                    response_deadline_monotonic_ms: deadline_monotonic_ms,
                    reason: ContainedTaskCancellationReason::from_failure_code(
                        failure_code.as_ref(),
                    ),
                    lease_terminal: lease_disposition,
                },
                failure_outcome: None,
            })),
            TaskOutcome::Failure => Err(RequestFailure::request(
                RuntimeHostError::request(
                    HostCode::ContainedTaskRecoveredTerminalFailure,
                    HostLocation::RecoverContainedTask,
                    RuntimeErrorCode::BackendOperationFailed,
                ),
                RuntimeReceiptState::Failed,
                Some(terminal(terminal_event)),
            )),
        }
    }

    fn append_recovered_contained_task_terminal(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        instance_id: InstanceId,
        lease_id: Option<LeaseId>,
        task_id: TaskId,
        run_id: RunId,
    ) -> Result<PersistedEvent, RequestFailure> {
        let links = request.contained_task_recovery_event_links(
            instance_id,
            lease_id,
            task_id,
            run_id,
            None,
        );
        let gate = lock(
            &self.fact_write_gate,
            HostLocation::RecoverContainedTaskTerminal,
        )
        .map_err(RequestFailure::poison_without_terminal)?;
        let mut appended = Vec::with_capacity(2);
        appended.push(
            self.append_event_under_fact_gate(
                EventSeverity::Warning,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                links.clone(),
                TaskPayloadDraft::semantic(
                    TaskSemanticFact::Finalizing {
                        task_result: TaskOutcome::Cancelled,
                    },
                    AuditInput::new(),
                ),
            )
            .map_err(RequestFailure::poison_without_terminal)?,
        );
        let terminal_event = self
            .append_event_under_fact_gate(
                EventSeverity::Warning,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                links,
                TaskPayloadDraft::semantic(
                    TaskSemanticFact::TerminalCommitted {
                        task_result: TaskOutcome::Cancelled,
                        final_page: None,
                        executed_steps: None,
                        outcome: Some(
                            Outcome::new(HostCode::ContainedTaskRecoveredAfterRestart)
                                .operation(HostLocation::RecoverContainedTaskTerminal),
                        ),
                        failure_severe: false,
                        scheduling_disposition: None,
                        task_timing: None,
                    },
                    AuditInput::new(),
                ),
            )
            .map_err(RequestFailure::poison_without_terminal)?;
        appended.push(terminal_event.clone());
        self.synchronize_fact_store_under_gate()
            .map_err(RequestFailure::poison_without_terminal)?;
        drop(gate);
        for event in &appended {
            self.observe_pipeline_event(event)
                .map_err(RequestFailure::poison_without_terminal)?;
        }
        Ok(terminal_event)
    }

    fn append_recovered_contained_task_release(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        instance_id: InstanceId,
        lease_id: LeaseId,
        task_id: TaskId,
        run_id: RunId,
    ) -> Result<PersistedEvent, RequestFailure> {
        if lock(
            &self.scheduler,
            HostLocation::VerifyRecoveredContainedTaskLease,
        )?
        .active_tokens()
        .iter()
        .any(|token| token.instance_id() == instance_id && token.lease_id() == lease_id)
        {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    HostCode::ContainedTaskRecoveryLeaseBusy,
                    HostLocation::RecoverContainedTask,
                    RuntimeErrorCode::ContainedTaskBusy,
                ),
                RuntimeReceiptState::Denied,
                None,
            ));
        }
        let resolved = lock(
            &self.registered_instances,
            HostLocation::ReadInstanceRegistry,
        )?
        .get(&instance_id)
        .cloned()
        .ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                HostCode::ContainedTaskRecoveryInstanceMissing,
                HostLocation::RecoverContainedTask,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })?;
        let links = request.contained_task_recovery_event_links(
            instance_id,
            Some(lease_id),
            task_id,
            run_id,
            Some(
                self.events
                    .action_id()
                    .map_err(RequestFailure::poison_without_terminal)?,
            ),
        );
        self.append_event(
            EventSeverity::Info,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links,
            LeasePayloadDraft::released(
                EventAction::LeaseRelease,
                EffectDisposition::NotPerformed,
                audit_endpoint(resolved.audit_endpoint()),
            ),
        )
    }

    /// Workflow #338 R3 (#343-1): at start, settles the direct contained runs the previous
    /// owner epoch first recorded and never terminated: `task.terminal_intent` and
    /// `task.cancelled` (`contained_task_recovered_after_restart`) on the run's original links,
    /// then a lease release when an actual grant has no terminal. The scan is bounded to the previous
    /// epoch, the events between the last two `runtime.started` / `runtime.takeover` events
    /// (the later one is this start's); older epochs are not reached. A run of the scheduler
    /// chain (one with a `policy.dispatch_intent` on its run or request) keeps the policy dispatch
    /// reconciliation, and a run of an instance this start did not register stays open. A run
    /// settled once has its terminal and lies before the next start's previous epoch, so no
    /// restart settles it again. Any failure fails the start.
    pub(super) fn settle_previous_epoch_contained_runs(&self) -> RuntimeHostResult<()> {
        let mut starts = Vec::new();
        for event_type in [EventType::RuntimeStarted, EventType::RuntimeTakeover] {
            starts.extend(
                self.ledger
                    .query(EventQuery {
                        event_type: Some(event_type),
                        ..EventQuery::default()
                    })
                    .map_err(|_| ledger_error(PREVIOUS_EPOCH_SETTLEMENT_OPERATION))?
                    .iter()
                    .map(PersistedEvent::sequence),
            );
        }
        starts.sort_unstable();
        let [.., previous, current] = starts.as_slice() else {
            return Ok(());
        };
        if current.saturating_sub(*previous) < 2 {
            return Ok(());
        }
        let events = self
            .ledger
            .query(EventQuery {
                from_sequence: Some(previous + 1),
                to_sequence: Some(current - 1),
                ..EventQuery::default()
            })
            .map_err(|_| ledger_error(PREVIOUS_EPOCH_SETTLEMENT_OPERATION))?;
        let mut requests = BTreeSet::new();
        for event in &events {
            if !matches!(
                event.payload(),
                EventPayload::Task(TaskPayload::Semantic(_))
            ) {
                continue;
            }
            let identity = TaskRunIdentity::from_semantic_event(event.payload(), event.links())
                .ok_or_else(|| {
                    RuntimeHostError::fatal(
                        HostCode::ContainedTaskIdentityMissing,
                        PREVIOUS_EPOCH_SETTLEMENT_OPERATION,
                        RuntimeErrorCode::RuntimeHalted,
                    )
                })?;
            if !requests.insert(identity.request_id) {
                continue;
            }
            self.settle_previous_epoch_contained_run(event)
                .map_err(|failure| *failure.error)?;
        }
        Ok(())
    }

    fn settle_previous_epoch_contained_run(
        &self,
        first_fact: &PersistedEvent,
    ) -> Result<(), RequestFailure> {
        let identity_missing = || {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                HostCode::ContainedTaskIdentityMissing,
                PREVIOUS_EPOCH_SETTLEMENT_OPERATION,
                RuntimeErrorCode::RuntimeHalted,
            ))
        };
        let links = first_fact.links();
        let request_id = *links.request_id().ok_or_else(identity_missing)?;
        let correlation_id = *links.correlation_id().ok_or_else(identity_missing)?;
        let instance_id = *links.instance_id().ok_or_else(identity_missing)?;
        let task_id = *links.task_id().ok_or_else(identity_missing)?;
        let run_id = *links.run_id().ok_or_else(identity_missing)?;
        let events = self
            .ledger
            .query(EventQuery {
                request_id: Some(request_id),
                ..EventQuery::default()
            })
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error(
                    PREVIOUS_EPOCH_SETTLEMENT_OPERATION,
                ))
            })?;
        // The scheduler chain: the run of a policy dispatch keeps `reconcile_policy_dispatches`.
        let policy_intents = self
            .ledger
            .query(EventQuery {
                run_id: Some(run_id),
                event_type: Some(EventType::PolicyDispatchIntent),
                ..EventQuery::default()
            })
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error(
                    PREVIOUS_EPOCH_SETTLEMENT_OPERATION,
                ))
            })?;
        if !policy_intents.is_empty()
            || events
                .iter()
                .any(|event| event.event_type() == EventType::PolicyDispatchIntent)
        {
            return Ok(());
        }
        let (run_identity, lease_id) =
            contained_task_recovery_identity(&events, PREVIOUS_EPOCH_SETTLEMENT_OPERATION)?
                .ok_or_else(identity_missing)?;
        // Candidate discovery covers only the previous epoch. A later fact cannot move an
        // older run's first fact into that epoch.
        if events.iter().any(|event| {
            event.sequence() < first_fact.sequence()
                && TaskRunIdentity::from_semantic_event(event.payload(), event.links()).is_some()
        }) {
            return Ok(());
        }
        let packages = events
            .iter()
            .filter(|event| {
                matches!(event.payload(), EventPayload::Task(TaskPayload::Semantic(payload))
                if matches!(payload.fact(), TaskSemanticFact::PackageAdmitted { .. }))
            })
            .count();
        if packages > 1 {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    HostCode::ContainedTaskRequestPackageReused,
                    PREVIOUS_EPOCH_SETTLEMENT_OPERATION,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            ));
        }
        let terminals = events
            .iter()
            .filter(|event| {
                matches!(
                    event.payload(),
                    EventPayload::Task(TaskPayload::Semantic(payload))
                        if matches!(payload.fact(), TaskSemanticFact::TerminalCommitted { .. })
                )
            })
            .count();
        if terminals > 1 {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    HostCode::ContainedTaskTerminalStateInconsistent,
                    PREVIOUS_EPOCH_SETTLEMENT_OPERATION,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            ));
        }
        if terminals == 1
            || !lock(
                &self.registered_instances,
                HostLocation::ReadInstanceRegistry,
            )?
            .contains_key(&instance_id)
        {
            return Ok(());
        }
        // The original request's identity, as a resubmission of it would arrive: only its
        // request, correlation and causation reach the recovery links.
        let identity_invalid = || {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                HostCode::ContainedTaskRecoveryIdentityInvalid,
                PREVIOUS_EPOCH_SETTLEMENT_OPERATION,
                RuntimeErrorCode::RuntimeHalted,
            ))
        };
        let submitted_at_unix_ms = self
            .runtime_clock_sample()
            .map_err(RequestFailure::poison_without_terminal)?
            .unix_ms;
        let original = recovery_request_identity(
            request_id,
            correlation_id,
            links.causation_id(),
            submitted_at_unix_ms,
        )
        .ok_or_else(identity_invalid)?;
        let validated = original.validate().map_err(|_| identity_invalid())?;
        self.append_recovered_contained_task_terminal(
            &validated,
            instance_id,
            lease_id,
            task_id,
            run_id,
        )?;
        let Some(lease_id) = lease_id else {
            return Ok(());
        };
        let lease_events = self
            .ledger
            .query(EventQuery {
                lease_id: Some(lease_id),
                ..EventQuery::default()
            })
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error(
                    PREVIOUS_EPOCH_SETTLEMENT_OPERATION,
                ))
            })?;
        let granted = lease_events.iter().any(|event| {
            event.event_type() == EventType::LeaseGranted
                && event.links().instance_id() == Some(&run_identity.instance_id)
                && run_identity.matches_links(event.links())
        });
        let lease_terminals = lease_events
            .iter()
            .filter(|event| {
                matches!(
                    event.event_type(),
                    EventType::LeaseReleased | EventType::LeaseExpired
                ) && event.links().lease_id() == Some(&lease_id)
            })
            .count();
        match lease_terminals {
            0 if granted => {
                self.append_recovered_contained_task_release(
                    &validated,
                    instance_id,
                    lease_id,
                    task_id,
                    run_id,
                )?;
                Ok(())
            }
            0 | 1 => Ok(()),
            _ => Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    HostCode::ContainedTaskLeaseTerminalStateInconsistent,
                    PREVIOUS_EPOCH_SETTLEMENT_OPERATION,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            )),
        }
    }

    fn begin_contained_run(
        &self,
        request_id: RequestId,
        instance_id: InstanceId,
        client_cancellable: bool,
    ) -> Result<ActiveContainedRun<'_>, RequestFailure> {
        let mut active = lock(&self.contained_runs, HostLocation::BeginContainedRun)?;
        if active.contains_key(&request_id) {
            return Err(contained_task_replay_denied(
                HostCode::ContainedTaskAlreadyRunning,
            ));
        }
        let control = Arc::new(ContainedRunControl::new(
            request_id,
            instance_id,
            client_cancellable,
        ));
        active.insert(request_id, Arc::clone(&control));
        drop(active);
        Ok(ActiveContainedRun {
            active: &self.contained_runs,
            request_id,
            control,
        })
    }

    /// Workflow #191 ps1 (b): waits until no contained run of `instance_id` is in flight. A
    /// run still in flight at the drain deadline is asked to stop at its next checkpoint
    /// (`contained_task_paused`); no lease is preempted or reclaimed. Runs asked to stop that have
    /// not ended by the grace deadline fail the drain. Stage (c) (ps2) drains again with the same
    /// deadlines when a run started after (b).
    pub(super) fn drain_contained_runs_for_pause(
        &self,
        instance_id: InstanceId,
        deadlines: SchedulingPauseDeadlines,
    ) -> Result<SchedulingDrainSummary, RequestFailure> {
        // Request identity -> (the run's control, whether this drain asked it to stop).
        let mut tracked = BTreeMap::<RequestId, (Arc<ContainedRunControl>, bool)>::new();
        let mut summary = SchedulingDrainSummary {
            finished: 0,
            cancelled: 0,
        };
        loop {
            if let Some(error) = self
                .fatal
                .current()
                .map_err(RequestFailure::poison_without_terminal)?
            {
                return Err(RequestFailure::poison_without_terminal(error));
            }
            if self.fatal.is_shutdown_requested() {
                return Err(RequestFailure::request(
                    RuntimeHostError::request(
                        HostCode::SchedulingPauseDrainInterrupted,
                        HostLocation::DrainPausedInstance,
                        RuntimeErrorCode::RuntimeUnavailable,
                    ),
                    RuntimeReceiptState::Failed,
                    None,
                ));
            }
            let active = lock(&self.contained_runs, HostLocation::ReadPausedInstanceRuns)?
                .iter()
                .filter(|(_, control)| control.instance_id == instance_id)
                .map(|(request_id, control)| (*request_id, Arc::clone(control)))
                .collect::<BTreeMap<_, _>>();
            let ended = tracked
                .iter()
                .filter(|(request_id, (control, _))| {
                    active
                        .get(request_id)
                        .is_none_or(|current| !Arc::ptr_eq(current, control))
                })
                .map(|(request_id, (_, cancelled))| (*request_id, *cancelled))
                .collect::<Vec<_>>();
            for (request_id, cancelled) in ended {
                tracked.remove(&request_id);
                let count = if cancelled {
                    &mut summary.cancelled
                } else {
                    &mut summary.finished
                };
                *count = count.checked_add(1).ok_or_else(|| {
                    pause_drain_fatal(HostCode::SchedulingPauseDrainCountOverflow)
                })?;
            }
            for (request_id, control) in active {
                tracked.entry(request_id).or_insert((control, false));
            }
            if tracked.is_empty() {
                return Ok(summary);
            }
            let now = Instant::now();
            if now >= deadlines.drain {
                for (control, cancelled) in tracked.values_mut() {
                    if !*cancelled && control.request_pause_cancel() {
                        *cancelled = true;
                    }
                }
                if now >= deadlines.grace {
                    return Err(RequestFailure::request(
                        RuntimeHostError::request(
                            HostCode::SchedulingPauseDrainIncomplete,
                            HostLocation::DrainPausedInstance,
                            RuntimeErrorCode::ContainedTaskBusy,
                        ),
                        RuntimeReceiptState::Failed,
                        None,
                    ));
                }
            }
            thread::sleep(SCHEDULING_PAUSE_DRAIN_POLL_INTERVAL);
        }
    }

    /// Keeps a committed `task.failed` on the control of the run that is still active under
    /// `request_id`; a terminal appended for no active run (replay recovery) keeps nothing.
    fn note_failed_task_terminal(
        &self,
        request_id: RequestId,
        terminal: FailedTaskTerminal,
    ) -> RuntimeHostResult<()> {
        let control = lock(&self.contained_runs, HostLocation::NoteFailedTaskTerminal)?
            .get(&request_id)
            .cloned();
        if let Some(control) = control {
            *lock(
                &control.failed_terminal,
                HostLocation::NoteFailedTaskTerminal,
            )? = Some(terminal);
        }
        Ok(())
    }

    pub(super) fn append_contained_task_terminal(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        mut draft: ContainedTaskTerminalDraft,
    ) -> Result<PersistedEvent, RequestFailure> {
        let task_timing = draft.task_timing.clone();
        let result = (|| {
            let links = self
                .events
                .request_links(
                    request,
                    Some(token.instance_id()),
                    Some(token.lease_id()),
                    None,
                )
                .with_task_id(draft.task_id)
                .with_run_id(draft.run_id);
            // The task's lease is still current when its terminal is written. Workflow #191 H: the
            // device session stays with the instance; the lease end closes one that must close.
            let _task_connection = lock(&self.scheduler, HostLocation::ReadTaskLeaseConnection)?
                .connection_for_token(token)
                .map_err(|error| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                        HostLocation::ReadTaskLeaseConnection,
                        &error,
                    ))
                })?;
            #[cfg(test)]
            self.consume_contained_task_checkpoint_for_test(
                ContainedTaskCheckpointPoint::BeforeTerminalAppend,
                ContainedTaskCheckpointIdentity::new(
                    request.request_id(),
                    token.instance_id(),
                    token.lease_id(),
                ),
            )
            .map_err(RequestFailure::poison_without_terminal)?;
            // Workflow #335 S5b: a successful terminal that took resource readings first runs the
            // live pool check, the terminal checks and the publication (stages 0-2); then this
            // original flow runs again in full (stage 3). Every other terminal runs it alone.
            let reading_failure = match draft.resource_readings.take() {
                Some(readings) if draft.outcome == TaskOutcome::Success => self
                    .publish_contained_task_resource_readings(
                        request, token, &links, &mut draft, readings,
                    )?,
                _ => None,
            };
            let gate = lock(
                &self.fact_write_gate,
                HostLocation::AppendContainedTaskTerminal,
            )
            .map_err(RequestFailure::poison_without_terminal)?;
            let (summary_committed, requested_summary, projection) =
                match self.check_contained_task_terminal(request, token, &draft, &links)? {
                    ContainedTaskTerminalCheck::AlreadyCommitted(rejected) => {
                        drop(gate);
                        return Err(self.contained_task_terminal_already_committed(&rejected));
                    }
                    ContainedTaskTerminalCheck::Open {
                        summary_committed,
                        requested_summary,
                        projection,
                    } => (summary_committed, requested_summary, projection),
                };
            let (scheduling_disposition, projection_failure) = match projection {
                Ok(disposition) => (disposition, None),
                // A rejected projection still owes the run a terminal fact: commit it as failed.
                Err(failure) if failure.error.code().starts_with("contained_task_outcome_") => {
                    draft.outcome = TaskOutcome::Failure;
                    draft.final_page = None;
                    draft.failure = Some(failure.error.outcome());
                    draft.failure_severe = failure.poison_runtime;
                    (None, Some(failure))
                }
                Err(failure) => return Err(failure),
            };
            let mut appended = Vec::with_capacity(3);
            if !summary_committed && let Some(summary) = requested_summary {
                appended.push(
                    self.append_event_under_fact_gate(
                        EventSeverity::Info,
                        EventSource::Runtime,
                        OriginModule::CapturePipeline,
                        EventActor::Runtime,
                        links.clone(),
                        CapturePayloadDraft::summary_committed(summary, AuditInput::new()),
                    )
                    .map_err(RequestFailure::poison_without_terminal)?,
                );
            }
            if !draft.intent_already_recorded {
                appended.push(
                    self.append_event_under_fact_gate(
                        EventSeverity::Info,
                        EventSource::Runtime,
                        OriginModule::Runtime,
                        EventActor::Runtime,
                        links.clone(),
                        TaskPayloadDraft::semantic(
                            TaskSemanticFact::Finalizing {
                                task_result: draft.outcome,
                            },
                            AuditInput::new(),
                        ),
                    )
                    .map_err(RequestFailure::poison_without_terminal)?,
                );
            }
            // Ledger revision 3 (model N2): a terminal's severity is its outcome's category; a
            // successful terminal carries no outcome, a failed or cancelled one must.
            let severity = match (draft.outcome, &draft.failure) {
                (TaskOutcome::Success, None) => EventSeverity::Info,
                (TaskOutcome::Failure | TaskOutcome::Cancelled, Some(failure)) => {
                    EventSeverity::from_category(failure.category())
                }
                _ => {
                    return Err(RequestFailure::poison_without_terminal(
                        RuntimeHostError::fatal(
                            HostCode::ContainedTaskTerminalSeverityInvalid,
                            HostLocation::AppendContainedTaskTerminal,
                            RuntimeErrorCode::RuntimeHalted,
                        ),
                    ));
                }
            };
            #[cfg(test)]
            if draft.scheduling_outcome.is_some()
                && self
                    .scheduling_terminal_append_failures
                    .swap(0, Ordering::AcqRel)
                    != 0
            {
                return Err(RequestFailure::poison_without_terminal(
                    RuntimeHostError::fatal(
                        crate::codes::test_spellings::TestCode::SchedulingTerminalAppendInjectedFailure,
                        crate::codes::HostLocation::AppendContainedTaskTerminal,
                        RuntimeErrorCode::RuntimeHalted,
                    ),
                ));
            }
            let terminal_event = self
                .append_event_under_fact_gate(
                    severity,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    links,
                    TaskPayloadDraft::semantic(
                        TaskSemanticFact::TerminalCommitted {
                            task_result: draft.outcome,
                            final_page: draft.final_page,
                            executed_steps: draft.executed_steps,
                            outcome: draft.failure.clone(),
                            failure_severe: draft.failure_severe,
                            scheduling_disposition,
                            task_timing: draft.task_timing,
                        },
                        AuditInput::new(),
                    ),
                )
                .map_err(RequestFailure::poison_without_terminal)?;
            appended.push(terminal_event.clone());
            self.synchronize_fact_store_under_gate()
                .map_err(RequestFailure::poison_without_terminal)?;
            drop(gate);
            for event in &appended {
                self.observe_pipeline_event(event)
                    .map_err(RequestFailure::poison_without_terminal)?;
            }
            if draft.outcome == TaskOutcome::Failure
                && let Some(failure) = &draft.failure
            {
                self.note_failed_task_terminal(
                    request.request_id(),
                    FailedTaskTerminal {
                        task_id: draft.task_id,
                        run_id: draft.run_id,
                        // Coordinator ruling 7 (acceptance H-L6): an unregistered relayed
                        // spelling triggers the ladder by its `raw_code`.
                        failure_code: actingcommand_contract::scheduled_failure_code_str(failure),
                    },
                )
                .map_err(RequestFailure::poison_without_terminal)?;
            }
            if let Some(mut failure) = projection_failure {
                failure.terminal = Some(terminal(&terminal_event));
                return Err(failure);
            }
            if let Some(mut failure) = reading_failure {
                failure.terminal = Some(terminal(&terminal_event));
                return Err(failure);
            }
            Ok(terminal_event)
        })();
        result.map_err(|mut failure: RequestFailure| {
            if failure.error.lifecycle.task_timing.is_none() {
                failure.error.lifecycle.task_timing = task_timing;
            }
            failure
        })
    }

    /// The terminal checks, under the fact write gate the caller holds: the run's chain, an
    /// already committed terminal (its rejection is appended here), the capture summary and
    /// the settlement projection.
    fn check_contained_task_terminal(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        draft: &ContainedTaskTerminalDraft,
        links: &EventLinksDraft,
    ) -> Result<ContainedTaskTerminalCheck, RequestFailure> {
        let chain_events = self
            .ledger
            .query(EventQuery {
                instance_id: Some(token.instance_id()),
                correlation_id: Some(request.correlation_id()),
                task_id: Some(*draft.task_id.transport()),
                run_id: Some(*draft.run_id.transport()),
                lease_id: Some(token.lease_id()),
                ..EventQuery::default()
            })
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error(
                    HostLocation::CheckContainedTaskTerminal,
                ))
            })?;
        let terminals = chain_events
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                    TaskSemanticFact::TerminalCommitted { task_result, .. } => Some(*task_result),
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>();
        if let [committed_outcome] = terminals.as_slice() {
            let rejected = self
                .append_event_under_fact_gate(
                    EventSeverity::Error,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    links.clone(),
                    TaskPayloadDraft::semantic(
                        TaskSemanticFact::TerminalRejected {
                            committed_outcome: *committed_outcome,
                            attempted_outcome: draft.outcome,
                            outcome: Outcome::new(HostCode::TerminalAlreadyCommitted)
                                .operation(HostLocation::AppendContainedTaskTerminal),
                        },
                        AuditInput::new(),
                    ),
                )
                .map_err(RequestFailure::poison_without_terminal)?;
            self.synchronize_fact_store_under_gate()
                .map_err(RequestFailure::poison_without_terminal)?;
            return Ok(ContainedTaskTerminalCheck::AlreadyCommitted(Box::new(
                rejected,
            )));
        }
        if terminals.len() > 1 {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    HostCode::ContainedTaskTerminalStateInconsistent,
                    HostLocation::AppendContainedTaskTerminal,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            ));
        }
        let existing_summary_events = chain_events
            .iter()
            .filter(|event| event.event_type() == EventType::CaptureSummaryCommitted)
            .collect::<Vec<_>>();
        if existing_summary_events.len() > 1 {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    HostCode::CaptureSummaryStateInconsistent,
                    HostLocation::AppendContainedTaskTerminal,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            ));
        }
        let existing_summary = existing_summary_events
            .first()
            .map(|event| {
                if event.origin().source() != EventSource::Runtime
                    || event.origin().module() != OriginModule::CapturePipeline
                    || event.origin().actor() != EventActor::Runtime
                {
                    return Err(RequestFailure::poison_without_terminal(
                        RuntimeHostError::fatal(
                            HostCode::CaptureSummaryStateConflict,
                            HostLocation::AppendContainedTaskTerminal,
                            RuntimeErrorCode::RuntimeHalted,
                        ),
                    ));
                }
                let EventPayload::Capture(CapturePayload::SummaryCommitted(payload)) =
                    event.payload()
                else {
                    return Err(RequestFailure::poison_without_terminal(
                        RuntimeHostError::fatal(
                            HostCode::CaptureSummaryStateInconsistent,
                            HostLocation::AppendContainedTaskTerminal,
                            RuntimeErrorCode::RuntimeHalted,
                        ),
                    ));
                };
                Ok(payload.summary())
            })
            .transpose()?;
        let requested_summary = draft
            .capture_summary
            .as_ref()
            .map(capture_summary_record)
            .transpose()
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::relayed_fatal(
                    error.code(),
                    HostLocation::AppendContainedTaskTerminal.as_str(),
                    RuntimeErrorCode::RuntimeHalted,
                ))
            })?;
        if let (Some(existing), Some(requested)) = (existing_summary, requested_summary.as_ref())
            && existing != requested
        {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    HostCode::CaptureSummaryStateConflict,
                    HostLocation::AppendContainedTaskTerminal,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            ));
        }
        let projection = select_scheduling_disposition(
            &chain_events,
            draft.outcome,
            draft.final_page.as_deref(),
            draft.executed_steps,
            draft.scheduling_outcome.as_ref(),
            draft.selected_scheduling_outcome.as_deref(),
            &draft.designated_effect_caps,
        );
        Ok(ContainedTaskTerminalCheck::Open {
            summary_committed: existing_summary.is_some(),
            requested_summary,
            projection,
        })
    }

    /// Answers a terminal attempt after its run's terminal was committed, once the fact write
    /// gate is released: the appended rejection is observed and the attempt is denied.
    fn contained_task_terminal_already_committed(
        &self,
        rejected: &PersistedEvent,
    ) -> RequestFailure {
        if let Err(error) = self.observe_pipeline_event(rejected) {
            return RequestFailure::poison_without_terminal(error);
        }
        RequestFailure::request(
            RuntimeHostError::request(
                HostCode::ContainedTaskTerminalAlreadyCommitted,
                HostLocation::AppendContainedTaskTerminal,
                RuntimeErrorCode::InvalidRequest,
            ),
            RuntimeReceiptState::Denied,
            Some(terminal(rejected)),
        )
    }

    /// Workflow #335 S5b, stages 0-2 of a successful terminal that took resource readings; the
    /// caller then runs the original terminal flow (stage 3). Workflow #384 D2-S3: the run's
    /// adjustment plan, when it has one, decides which records it publishes and which records
    /// from before the run it drops.
    ///
    /// - Stage 0, no lock held while the pools are read: the active catalog read as the policy
    ///   forward projection reads it. A `ledger_fact` pool observing the key of a record to
    ///   publish in the run's instance scope refuses the readings
    ///   (`contained_task_resource_reading_live_pool_unsupported`, detail `<id>:<pool>`).
    /// - Stage 1, under the fact write gate, then released: the terminal checks. An already
    ///   committed terminal is answered as before and a refused settlement projection is left
    ///   to the original rewrite; neither publishes.
    /// - Stage 2, no lock held: first each planned drop of a record from before the run, one
    ///   `fact.invalidated` each (`HostShared::invalidate_adjusted_fact`); then each record,
    ///   its own observation through `publish_facts` (Runtime origin, system links), in
    ///   publication order: the readings with the Ordinary purpose, an adjustment write with
    ///   the `ResourceAdjustment` purpose, which refuses it when its base was replaced. A
    ///   non-fatal refusal stops there (earlier records stay published) and refuses the
    ///   readings (`contained_task_resource_reading_rejected`, detail `<id>:<fact store code>`);
    ///   a fatal one, already marked, poisons the Runtime.
    ///
    /// A refusal rewrites the draft to that failure and returns it, to be answered after the
    /// failure terminal.
    fn publish_contained_task_resource_readings(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        links: &EventLinksDraft,
        draft: &mut ContainedTaskTerminalDraft,
        readings: ContainedTaskResourceReadings,
    ) -> Result<Option<RequestFailure>, RequestFailure> {
        // The records to publish: the plan's, or every reading in the order of its frame's
        // capture time (Workflow #384 D2-S2).
        let publications = match &readings.adjustments {
            Some(plan) => plan.publications.clone(),
            None => {
                let mut frames = (0..readings.frames.len()).collect::<Vec<_>>();
                frames.sort_by_key(|frame| readings.frames[*frame].captured_at);
                frames
                    .into_iter()
                    .flat_map(|frame| {
                        (0..readings.frames[frame].readings.len())
                            .map(move |reading| PlannedPublication::Reading { frame, reading })
                    })
                    .collect()
            }
        };
        let scope = actingcommand_policy::ScopeSelector::Instance {
            instance_id: readings.instance_alias.clone(),
        };
        let catalog = lock(self.policy()?, HostLocation::CheckResourceReadingPools)
            .map_err(RequestFailure::poison_without_terminal)?
            .active_loaded();
        let live_pool = catalog.as_ref().and_then(|catalog| {
            publications
                .iter()
                .filter_map(|publication| match publication {
                    PlannedPublication::Reading { frame, reading } => readings
                        .frames
                        .get(*frame)
                        .and_then(|joined| joined.readings.get(*reading))
                        .map(|value| {
                            (
                                value.declaration.id.as_str(),
                                value.declaration.fact_key.as_str(),
                            )
                        }),
                    PlannedPublication::Adjustment(adjustment) => Some((
                        adjustment.adjustment_id.as_str(),
                        adjustment.fact_key.as_str(),
                    )),
                })
                .find_map(|(id, fact_key)| {
                    catalog
                        .compiled()
                        .catalog()
                        .pools
                        .pools
                        .iter()
                        .find(|pool| {
                            matches!(
                                pool.value_source,
                                actingcommand_policy::PoolValueSource::LedgerFact { .. }
                            ) && pool.scope == scope
                                && matches!(
                                    &pool.observation,
                                    actingcommand_policy::ObservationRef::Fact { fact_key: observed }
                                        if observed == fact_key
                                )
                        })
                        .map(|pool| format!("{id}:{}", pool.id))
                })
        });
        if let Some(detail) = live_pool {
            return Ok(Some(refuse_resource_readings(
                draft,
                HostCode::ContainedTaskResourceReadingLivePoolUnsupported,
                detail,
            )));
        }
        let gate = lock(
            &self.fact_write_gate,
            HostLocation::AppendContainedTaskTerminal,
        )
        .map_err(RequestFailure::poison_without_terminal)?;
        match self.check_contained_task_terminal(request, token, draft, links)? {
            ContainedTaskTerminalCheck::AlreadyCommitted(rejected) => {
                drop(gate);
                return Err(self.contained_task_terminal_already_committed(&rejected));
            }
            ContainedTaskTerminalCheck::Open {
                projection: Ok(_), ..
            } => {}
            ContainedTaskTerminalCheck::Open {
                projection: Err(failure),
                ..
            } => {
                return if failure.error.code().starts_with("contained_task_outcome_") {
                    Ok(None)
                } else {
                    Err(failure)
                };
            }
        }
        drop(gate);
        let run_text = resource_reading_identifier_text(draft.run_id.transport())?;
        // Every record must fit a fact record before any is published: each reading's record,
        // and each adjustment write's value and lifetime (its snapshot names its base, which a
        // run record before it receives only when published).
        let mut reading_records = Vec::with_capacity(publications.len());
        for publication in &publications {
            match publication {
                PlannedPublication::Reading { frame, reading } => {
                    let (Some(joined), Some(value)) = (
                        readings.frames.get(*frame),
                        readings
                            .frames
                            .get(*frame)
                            .and_then(|joined| joined.readings.get(*reading)),
                    ) else {
                        return Err(resource_reading_poison(
                            HostCode::ContainedTaskResourceReadingFrameMissing,
                        ));
                    };
                    let frame_text = resource_reading_identifier_text(joined.frame_id.transport())?;
                    let Some(record) = resource_reading_fact_record(
                        &readings,
                        value,
                        reading_unix_ms(joined.captured_at)?,
                        &run_text,
                        &frame_text,
                    ) else {
                        return Ok(Some(refuse_resource_reading(
                            draft,
                            &value.declaration.id,
                            HostCode::FactObservationInvalid.as_str(),
                        )));
                    };
                    reading_records.push(Some((value.declaration.id.as_str(), record)));
                }
                PlannedPublication::Adjustment(adjustment) => {
                    if resource_adjustment_fact_record(&readings, adjustment, "", "", "").is_none()
                    {
                        return Ok(Some(refuse_resource_reading(
                            draft,
                            &adjustment.adjustment_id,
                            HostCode::FactObservationInvalid.as_str(),
                        )));
                    }
                    reading_records.push(None);
                }
            }
        }
        let fact_scope = FactScope::Instance {
            instance_id: readings.instance_alias.clone(),
        };
        // Workflow #384 D2-S3 (D2 model §3.6): the records from before the run that a confirmed
        // change, read by nothing, made stale.
        for invalidation in readings
            .adjustments
            .iter()
            .flat_map(|plan| &plan.invalidations)
        {
            match self.invalidate_adjusted_fact(
                &fact_scope,
                &invalidation.fact_key,
                invalidation.base_sequence,
                invalidation.step_sequence,
            ) {
                Ok(()) => {}
                Err(error) if error.is_fatal() => {
                    return Err(RequestFailure::poison_without_terminal(error));
                }
                Err(error) => {
                    return Ok(Some(refuse_resource_reading(
                        draft,
                        &invalidation.adjustment_id,
                        error.code(),
                    )));
                }
            }
        }
        // Workflow #384 D2-S2 (model §3.5): each reading is observed when its frame was captured;
        // D2-S3: an adjustment write when its operation was confirmed, adjusting the record it
        // names, which must still be the key's record.
        let mut published: Vec<Option<(u64, EventId)>> = Vec::with_capacity(publications.len());
        for (publication, reading_record) in publications.iter().zip(reading_records) {
            let (id, record, purpose) = match (publication, reading_record) {
                (_, Some((id, record))) => (id, Some(record), FactPublicationPurpose::Ordinary),
                (PlannedPublication::Adjustment(adjustment), None) => {
                    let Some(frame_id) = readings.confirmation_frames.get(adjustment.confirmation)
                    else {
                        return Err(resource_reading_poison(
                            HostCode::ContainedTaskResourceReadingFrameMissing,
                        ));
                    };
                    let base = match adjustment.base {
                        PlannedBase::PreRun { sequence, event_id } => Some((sequence, event_id)),
                        PlannedBase::Run(position) => published.get(position).copied().flatten(),
                    };
                    let Some((base_sequence, base_event)) = base else {
                        return Ok(Some(refuse_resource_reading(
                            draft,
                            &adjustment.adjustment_id,
                            HostCode::FactObservationVersionConflict.as_str(),
                        )));
                    };
                    let frame_text = resource_reading_identifier_text(frame_id.transport())?;
                    let base_text = resource_reading_identifier_text(&base_event)?;
                    (
                        adjustment.adjustment_id.as_str(),
                        resource_adjustment_fact_record(
                            &readings,
                            adjustment,
                            &run_text,
                            &frame_text,
                            &base_text,
                        ),
                        FactPublicationPurpose::ResourceAdjustment { base_sequence },
                    )
                }
                (PlannedPublication::Reading { .. }, None) => {
                    return Err(resource_reading_poison(
                        HostCode::ContainedTaskResourceReadingFrameMissing,
                    ));
                }
            };
            let Some(record) = record else {
                return Ok(Some(refuse_resource_reading(
                    draft,
                    id,
                    HostCode::FactObservationInvalid.as_str(),
                )));
            };
            match self.publish_facts(
                actingcommand_contract::FactObservation {
                    records: vec![record],
                },
                None,
                purpose,
            ) {
                Ok(fact) => published.push(Some((fact.sequence, fact.event_id))),
                Err(error) if error.is_fatal() => {
                    return Err(RequestFailure::poison_without_terminal(error));
                }
                Err(error) => {
                    return Ok(Some(refuse_resource_reading(draft, id, error.code())));
                }
            }
        }
        Ok(None)
    }
}
