// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    CaptureBackendName, CaptureExtent, CaptureGeometryObservation, TaskGeometryConclusion,
    TaskGeometryFailure, TaskGeometryFrame, TaskGeometryObservation, TaskGeometryPhase,
    TaskGeometryRecheckTrigger,
};
use actingcommand_execution_kernel::CaptureGeometrySessionRef;

const MAX_CONTAINED_TASK_OCR_FAILURE_DETAIL_BYTES: usize = 64 * 1024;
const CONTAINED_TASK_POST_ADMISSION_OCR_FAILED: &str = "contained_task_post_admission_ocr_failed";
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ContainedTaskCheckpointIdentity {
    request_id: RequestId,
    instance_id: InstanceId,
    lease_id: LeaseId,
}

#[cfg(test)]
impl ContainedTaskCheckpointIdentity {
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

#[cfg(test)]
pub(super) struct ContainedTaskCheckpointTestHook {
    pub(super) request_id: RequestId,
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
            "read_contained_task_checkpoint_test_control",
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
    pub(super) failure_code: Option<&'static str>,
    pub(super) failure_severity: Option<EventSeverity>,
    pub(super) scheduling_outcome: Option<(String, SchedulingOutcomeDeclaration)>,
    pub(super) selected_scheduling_outcome: Option<String>,
    pub(super) capture_summary: Option<CapturePipelineSummary>,
    pub(super) task_timing: Option<Box<actingcommand_contract::TaskTimingObservations>>,
}

pub(super) struct ContainedRunControl {
    pub(super) request_id: RequestId,
    pub(super) instance_id: InstanceId,
    client_cancellable: bool,
    deadline_monotonic_ms: AtomicU64,
    cancellation_reason: AtomicU8,
}

impl ContainedRunControl {
    const NONE: u8 = 0;
    const CLIENT_REQUESTED: u8 = 1;
    const DEADLINE_EXCEEDED: u8 = 2;

    const fn new(request_id: RequestId, instance_id: InstanceId, client_cancellable: bool) -> Self {
        Self {
            request_id,
            instance_id,
            client_cancellable,
            deadline_monotonic_ms: AtomicU64::new(0),
            cancellation_reason: AtomicU8::new(Self::NONE),
        }
    }

    fn set_deadline(&self, deadline_monotonic_ms: u64) -> RuntimeHostResult<()> {
        if deadline_monotonic_ms == 0 {
            return Err(RuntimeHostError::fatal(
                "contained_task_deadline_state_invalid",
                "configure_contained_task_deadline",
                RuntimeErrorCode::RuntimeFatal,
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
                        "contained_task_deadline_state_invalid",
                        "configure_contained_task_deadline",
                        RuntimeErrorCode::RuntimeFatal,
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

    fn cancellation_reason(
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
            _ => None,
        }
    }

    fn deadline(&self) -> u64 {
        self.deadline_monotonic_ms.load(Ordering::Acquire)
    }
}

struct ActiveContainedRun<'a> {
    active: &'a Mutex<BTreeMap<RequestId, Arc<ContainedRunControl>>>,
    request_id: RequestId,
    control: Arc<ContainedRunControl>,
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
                "capture_summary_count_overflow",
                "record_contained_task_capture",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        self.counts.captured = self.counts.captured.checked_add(1).ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "capture_summary_count_overflow",
                "record_contained_task_capture",
                RuntimeErrorCode::RuntimeFatal,
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
                    "capture_summary_frame_conflict",
                    "record_contained_task_capture",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        self.counts.persisted = self.counts.persisted.checked_add(1).ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "capture_summary_count_overflow",
                "record_contained_task_capture",
                RuntimeErrorCode::RuntimeFatal,
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
                    "capture_summary_phase_conflict",
                    "record_contained_task_effect",
                    RuntimeErrorCode::RuntimeFatal,
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
                    "capture_summary_frame_missing",
                    "record_contained_task_pin",
                    RuntimeErrorCode::RuntimeFatal,
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
                    "capture_summary_pin_conflict",
                    "record_contained_task_pin",
                    RuntimeErrorCode::RuntimeFatal,
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
            for ((index, reason), original) in &self.pinned {
                if let Some(index) = index {
                    let material = pipeline
                        .pin_frame(*index, *reason, &mut sink)
                        .map_err(online_observation::observation_artifact_failure)?;
                    if original.as_ref() != Some(&material) {
                        return Err(online_observation::observation_integrity_failure(
                            "capture_pin_material_changed",
                        ));
                    }
                }
            }
            let summary = pipeline
                .finish(&mut sink)
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
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                error.code(),
                "finalize_contained_task_capture_summary",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })
    }
}

pub(super) struct RuntimeContainedTask<'a> {
    pub(super) host: &'a HostShared,
    pub(super) request: &'a ValidatedRuntimeRequest<'a>,
    pub(super) token: &'a LeaseToken,
    instance_alias: &'a str,
    connection_id: ConnectionId,
    pub(super) task_id: IssuedTaskId,
    pub(super) run_id: IssuedRunId,
    execution_provenance: ExecutionBackendProvenance,
    pub(super) control: Arc<ContainedRunControl>,
    pub(super) last_frame_id: Option<IssuedFrameId>,
    geometry_session: Option<CaptureGeometrySessionRef>,
    geometry_frame: Option<TaskGeometryFrame>,
    geometry_initial: Option<(CaptureExtent, CaptureGeometryObservation)>,
    geometry_deadline: Option<Instant>,
    geometry_rechecked: bool,
    input_step_action_id: Option<ActionId>,
    post_input_action_id: Option<ActionId>,
    last_capture_input_action_id: Option<ActionId>,
    expected_stability_declaration: Option<StabilityTerminationDeclaration>,
    stability: Option<RuntimeContainedTaskStability>,
    expects_post_admission_ocr: bool,
    post_admission_ocr_observations: u32,
    post_admission_ocr_comparison_recorded: bool,
    pub(super) current_recognition_id: Option<IssuedRecognitionId>,
    step_actions: BTreeMap<u32, (IssuedActionId, String)>,
    step_index_offset: u32,
    pub(super) executed_steps: Option<u32>,
    entry_preflight_recorded: bool,
    sampling_run_seed: Option<u64>,
    used_action_seeds: BTreeSet<u64>,
    finalizing: Option<TaskOutcome>,
    capture_evidence: CaptureEvidenceAccumulator,
    configuration_records: u8,
    configuration_capture_recorded: bool,
    configuration_input_recorded: bool,
    pub(super) diagnostic_stream: Option<actingcommand_artifact_store::ArtifactStream>,
    pub(super) diagnostic_records: u64,
    pub(super) task_timing: task_timing::TaskTimingObserver,
    pub(super) diagnostic_step: Option<task_diagnostic::DiagnosticStep>,
    pub(super) diagnostic_physical: Option<ActionId>,
}

struct EntryRecoveryRuntime<'a, 'host> {
    inner: &'a mut RuntimeContainedTask<'host>,
}

impl ContainedTaskRuntime for EntryRecoveryRuntime<'_, '_> {
    type Error = RequestFailure;

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
    fn record_guard_evaluation(
        &mut self,
        target: Option<&str>,
        result: Option<
            &actingcommand_recognition_pack::RecognitionPackResult<
                actingcommand_recognition_pack::TargetEvaluation,
            >,
        >,
        reason: &'static str,
    ) -> Result<(), Self::Error> {
        self.inner.diagnostic_guard(target, result, reason)
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

    fn capture(&mut self) -> Result<Frame, Self::Error> {
        self.inner.capture()
    }

    fn action_seed(
        &mut self,
        step_index: u32,
        operation_label: &str,
    ) -> Result<Option<u64>, Self::Error> {
        self.inner.action_seed(step_index, operation_label)
    }

    fn input(&mut self, action: InputAction) -> Result<(), Self::Error> {
        self.inner.input(action)
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
}

fn fail_contained_task_entry(
    runtime: &RuntimeContainedTask<'_>,
    code: &'static str,
) -> Result<ContainedTaskOutcome, ContainedTaskRunError<RequestFailure>> {
    runtime
        .record_entry_fact(TaskSemanticFact::EntryTargetDisposition {
            disposition: TaskEntryTargetDisposition::FailClosed,
            failure_code: Some(code.to_owned()),
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

fn task_geometry_error(code: &'static str) -> RuntimeHostError {
    RuntimeHostError::request(
        code,
        "observe_task_geometry",
        RuntimeErrorCode::CaptureFailed,
    )
}

fn task_geometry_failure_ref(error: &RuntimeHostError) -> TaskGeometryFailure {
    TaskGeometryFailure {
        code: error.code().to_owned(),
        event_id: error.lifecycle.recorded_event.get().copied(),
    }
}

fn task_geometry_conclusion_error(conclusion: TaskGeometryConclusion) -> RuntimeHostError {
    task_geometry_error(match conclusion {
        TaskGeometryConclusion::AspectMismatch { .. } => "contained_task_geometry_aspect_mismatch",
        TaskGeometryConclusion::Unknown { .. } => "contained_task_geometry_unknown",
        TaskGeometryConclusion::Unavailable => "contained_task_geometry_unavailable",
        TaskGeometryConclusion::Pass | TaskGeometryConclusion::FixtureNotApplicable => {
            "contained_task_geometry_producer_binding_mismatch"
        }
    })
}

fn task_geometry_request_failure(
    error: RuntimeHostError,
    event: Option<&PersistedEvent>,
) -> RequestFailure {
    if let Some(event) = event {
        let _ = error.lifecycle.recorded_event.set(*event.event_id());
    }
    RequestFailure {
        state: RuntimeReceiptState::Failed,
        terminal: event.map(terminal),
        poison_runtime: error.is_fatal(),
        task_failure: Some(TaskFailureEvidence {
            code: error.code(),
            severity: if error.is_fatal() {
                EventSeverity::Fatal
            } else {
                EventSeverity::Warning
            },
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
            .ok_or_else(|| task_geometry_error("contained_task_geometry_budget_unavailable"))?;
        let now = self.host.monotonic_ms()?;
        if Instant::now() >= deadline || self.control.cancellation_reason(now).is_some() {
            return Err(task_geometry_error(
                "contained_task_geometry_budget_unavailable",
            ));
        }
        let session = self
            .geometry_session
            .as_ref()
            .ok_or_else(|| task_geometry_error("contained_task_geometry_session_unavailable"))?;
        let frame = self
            .geometry_frame
            .as_ref()
            .ok_or_else(|| task_geometry_error("contained_task_geometry_frame_unavailable"))?;
        // This checks the original session; it cannot create or reopen a producer.
        self.host
            .execution
            .validate_capture_geometry_session(session, deadline)
            .map_err(|error| {
                RuntimeHostError::execution("validate_task_geometry_session", &error)
            })?;
        if frame.backend == CaptureBackendName::FixtureSimulation
            || reuse_frame && frame.backend == CaptureBackendName::NemuIpc
        {
            return Ok(frame.producer.clone());
        }
        self.host
            .execution
            .observe_capture_geometry(session, deadline)
            .map_err(|error| RuntimeHostError::execution("observe_task_geometry", &error))
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
            let retained = self
                .host
                .retain_unconfirmed_resources(error, self.links())
                .map_err(|failure| {
                    RequestFailure::poison_without_terminal(
                        failure.with_related_failure("geometry_observation", error),
                    )
                })?;
            if retained || error.is_fatal() {
                return Err(RequestFailure::poison_without_terminal(error.clone()));
            }
        }
        let conclusion = match result {
            Ok(observation) => TaskGeometryObservation::assess(
                self.geometry_frame.as_ref().ok_or_else(|| {
                    RequestFailure::poison_without_terminal(task_geometry_error(
                        "contained_task_geometry_frame_unavailable",
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
        let extent = CaptureExtent::new(frame.width, frame.height).ok_or_else(|| {
            RequestFailure::poison_without_terminal(task_geometry_error(
                "contained_task_geometry_frame_extent_invalid",
            ))
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
                task_geometry_error("contained_task_geometry_producer_binding_mismatch"),
                None,
            ));
        }
        if self.geometry_session.is_none() {
            self.geometry_session = Some(session);
        }
        if self.geometry_initial.is_some() {
            return Ok(());
        }
        let result = if frame.backend_name != CaptureBackendName::FixtureSimulation
            && u64::from(extent.width()) * 9 != u64::from(extent.height()) * 16
        {
            // The delivered frame already disproves the prerequisite; no display read is needed.
            Ok(frame.geometry.clone())
        } else {
            self.read_task_geometry(true)
        };
        let (conclusion, event) =
            self.record_task_geometry(TaskGeometryPhase::Initial, None, None, &result)?;
        match (conclusion, result) {
            (
                TaskGeometryConclusion::Pass | TaskGeometryConclusion::FixtureNotApplicable,
                Ok(value),
            ) => {
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
                task_geometry_error("contained_task_geometry_input_prerequisite_missing"),
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
            .execution
            .validate_capture_geometry_session(session, self.geometry_deadline.ok_or_else(failure)?)
            .map_err(|error| {
                task_geometry_request_failure(
                    RuntimeHostError::execution("validate_task_geometry_input", &error),
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
                let trigger = match error.code() {
                    "contained_task_recognition_failed" => {
                        TaskGeometryRecheckTrigger::RecognitionFailed
                    }
                    "contained_task_page_unknown" => TaskGeometryRecheckTrigger::PageUnknown,
                    _ => return,
                };
                let mut primary = RuntimeHostError::request(
                    error.code(),
                    "run_contained_task",
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
                    && matches!(
                        failure.error.code(),
                        "input_backend_operation_failed" | "input_backend_open_failed"
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
                preserved_primary
                    .lifecycle
                    .causes
                    .extend(observation_context.causes.iter().cloned());
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
        package_sha256: actingcommand_contract::PackageRef,
        primary: &RuntimeHostError,
    ) -> Result<(), RequestFailure> {
        // Preserve the two original recovery failure facts after a classified failure,
        // even when the operation has consumed its last remaining execution budget.
        for fact in [
            TaskSemanticFact::EntryRecoveryFailed {
                package_sha256,
                failure_code: primary.code().to_owned(),
            },
            TaskSemanticFact::EntryTargetDisposition {
                disposition: TaskEntryTargetDisposition::FailClosed,
                failure_code: Some(primary.code().to_owned()),
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

    fn record_initial_configuration(
        &mut self,
        request: &ContainedTaskRequest,
        prepared: &PreparedContainedTask,
    ) -> Result<(), RequestFailure> {
        let resolved = self
            .host
            .execution
            .resolve(self.instance_alias)
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::execution(
                    "resolve_contained_task_configuration",
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

    fn record_configuration(
        &mut self,
        facts: EffectiveConfigurationFacts,
        frame_id: Option<IssuedFrameId>,
        action_id: Option<ActionId>,
        source_sequence: Option<u64>,
    ) -> Result<(), RequestFailure> {
        if self.configuration_records >= 4 {
            return Err(RequestFailure::poison_without_terminal(
                artifact_store_error("effective_configuration_limit_exceeded"),
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
                "encode_effective_configuration",
            ))
        })?;
        if bytes.len() as u64 > MAX_EFFECTIVE_CONFIGURATION_BYTES {
            return Err(RequestFailure::poison_without_terminal(
                artifact_store_error("effective_configuration_too_large"),
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
                "contained_task_deadline_exceeded",
                RuntimeErrorCode::ContainedTaskDeadlineExceeded,
            ),
            ContainedTaskCancellationReason::ClientRequested => (
                "contained_task_cancelled",
                RuntimeErrorCode::ContainedTaskCancelled,
            ),
            ContainedTaskCancellationReason::RecoveredAfterRestart => {
                return Err(RequestFailure::poison_without_terminal(
                    RuntimeHostError::fatal(
                        "contained_task_cancellation_state_invalid",
                        "check_contained_task_deadline",
                        RuntimeErrorCode::RuntimeFatal,
                    ),
                ));
            }
        };
        Err(RequestFailure::request(
            RuntimeHostError::request(code, "run_contained_task", projection),
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
        loop {
            self.ensure_active()?;
            self.host.ledger.check_writer_health().map_err(|error| {
                RequestFailure::poison_without_terminal(
                    RuntimeHostError::fatal(
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
                .poll_pressure(&context, &mut sink)
                .map_err(online_observation::observation_artifact_failure)?;
            if !pipeline.is_paused() || self.finalizing.is_some() {
                return Ok(());
            }
            // The original run deadline/cancellation bounds the pause before any capture effect.
            if self.control.deadline() == 0 || self.host.fatal.is_shutdown_requested() {
                return Err(RequestFailure::request(
                    RuntimeHostError::request(
                        "capture_pressure_pause_stopped",
                        "admit_contained_task_capture",
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
                            reason: error.to_string().chars().take(256).collect(),
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
                    reason: error.to_string().chars().take(256).collect(),
                },
            };
            let mut sink = RuntimeArtifactEventSink {
                ledger: &self.host.ledger,
                events: &self.host.events,
            };
            if let Some(pipeline) = self.capture_evidence.pipeline.as_mut() {
                pipeline
                    .record_recognition(index, state, &mut sink)
                    .map_err(online_observation::observation_artifact_failure)?;
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

    fn record_entry_fact(&self, fact: TaskSemanticFact) -> Result<(), RequestFailure> {
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
        self.step_index_offset
            .checked_add(step_index)
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "contained_task_step_index_overflow",
                    "run_contained_task",
                    RuntimeErrorCode::RuntimeFatal,
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
                "contained_task_post_admission_ocr_failure_frame_missing",
                "run_contained_task",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        let event_links = self.links();
        let request_id = event_links.request_id().copied().ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "contained_task_post_admission_ocr_failure_request_missing",
                "run_contained_task",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        let correlation_id = event_links.correlation_id().copied().ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "contained_task_post_admission_ocr_failure_correlation_missing",
                "run_contained_task",
                RuntimeErrorCode::RuntimeFatal,
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
                "contained_task_post_admission_ocr_failure_evidence_encode_failed",
                "run_contained_task",
                RuntimeErrorCode::RuntimeFatal,
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
                artifact_store_error("persist_contained_task_post_admission_ocr_failure"),
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
                "contained_task_post_admission_ocr_frame_missing",
                "run_contained_task",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        if !self.expects_post_admission_ocr
            || self.post_admission_ocr_comparison_recorded
            || frame_index != self.post_admission_ocr_observations
        {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "contained_task_post_admission_ocr_state_invalid",
                    "run_contained_task",
                    RuntimeErrorCode::RuntimeFatal,
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
                    "contained_task_post_admission_ocr_frame_artifact_missing",
                    "run_contained_task",
                    RuntimeErrorCode::RuntimeFatal,
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
                "contained_task_post_admission_ocr_evidence_encode_failed",
                "run_contained_task",
                RuntimeErrorCode::RuntimeFatal,
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
                    "contained_task_post_admission_ocr_count_overflow",
                    "run_contained_task",
                    RuntimeErrorCode::RuntimeFatal,
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
                "contained_task_post_admission_ocr_frame_missing",
                "run_contained_task",
                RuntimeErrorCode::RuntimeFatal,
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
                    "contained_task_post_admission_ocr_comparison_invalid",
                    "run_contained_task",
                    RuntimeErrorCode::RuntimeFatal,
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
                "contained_task_post_admission_ocr_evidence_encode_failed",
                "run_contained_task",
                RuntimeErrorCode::RuntimeFatal,
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
                "contained_task_stability_baseline_invalid",
            ));
        }
        contained_task_step_action(&self.step_actions, step_index, &operation_label)?;
        let current_frame_id = self.last_frame_id.ok_or_else(|| {
            contained_task_stability_failure("contained_task_stability_frame_identity_missing")
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
            contained_task_stability_failure("contained_task_stability_baseline_missing")
        })?;
        let current_frame_id =
            contained_task_stability_current_frame(state.previous_frame_id, self.last_frame_id)?;
        let expected_step_index = state.last_step_index.checked_add(1).ok_or_else(|| {
            contained_task_stability_failure("contained_task_stability_step_overflow")
        })?;
        if state.terminal_recorded
            || state.pending_terminal.is_some()
            || state.declaration != declaration
            || step_index != expected_step_index
            || prior_consecutive_unchanged != state.consecutive_unchanged
        {
            return Err(contained_task_stability_failure(
                "contained_task_stability_comparison_invalid",
            ));
        }
        let expected_count = match result {
            StabilityComparisonResult::Changed => 0,
            StabilityComparisonResult::Unchanged => {
                prior_consecutive_unchanged.checked_add(1).ok_or_else(|| {
                    contained_task_stability_failure("contained_task_stability_count_overflow")
                })?
            }
        };
        if new_consecutive_unchanged != expected_count
            || new_consecutive_unchanged > declaration.consecutive_unchanged_threshold
        {
            return Err(contained_task_stability_failure(
                "contained_task_stability_count_invalid",
            ));
        }
        let completed_steps = step_index
            .checked_sub(self.step_index_offset)
            .and_then(|relative| relative.checked_add(1))
            .ok_or_else(|| {
                contained_task_stability_failure("contained_task_stability_step_overflow")
            })?;
        if completed_steps > declaration.max_steps {
            return Err(contained_task_stability_failure(
                "contained_task_stability_step_invalid",
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
                "contained_task_stability_terminal_reason_invalid",
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
            contained_task_stability_failure("contained_task_stability_evidence_encode_failed")
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
                artifact_store_error("persist_contained_task_stability_comparison"),
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
            contained_task_stability_failure("contained_task_stability_state_missing")
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
            contained_task_stability_failure("contained_task_stability_baseline_missing")
        })?;
        let pending = state.pending_terminal.as_ref().ok_or_else(|| {
            contained_task_stability_failure("contained_task_stability_terminal_missing")
        })?;
        if state.terminal_recorded
            || pending.step_index != step_index
            || pending.operation_label != operation_label
            || pending.reason != reason
        {
            return Err(contained_task_stability_failure(
                "contained_task_stability_terminal_mismatch",
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
            "contained_task_post_admission_ocr_failure_detail_too_large",
            "persist_contained_task_post_admission_ocr_failure",
            RuntimeErrorCode::RuntimeFatal,
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

fn contained_task_stability_failure(code: &'static str) -> RequestFailure {
    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
        code,
        "record_contained_task_stability",
        RuntimeErrorCode::RuntimeFatal,
    ))
}

fn contained_task_stability_current_frame(
    previous_frame_id: IssuedFrameId,
    current_frame_id: Option<IssuedFrameId>,
) -> Result<IssuedFrameId, RequestFailure> {
    let current_frame_id = current_frame_id.ok_or_else(|| {
        contained_task_stability_failure("contained_task_stability_frame_identity_missing")
    })?;
    if current_frame_id == previous_frame_id {
        return Err(contained_task_stability_failure(
            "contained_task_stability_frame_identity_reused",
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

    fn update_run_progress(&mut self, executed_steps: u32) {
        self.executed_steps = self.step_index_offset.checked_add(executed_steps);
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
        reason: &'static str,
    ) -> Result<(), Self::Error> {
        self.diagnostic_guard(target, result, reason)
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

    fn classify_error(error: &Self::Error) -> ContainedTaskRuntimeErrorClass {
        if error.error.is_fatal() {
            ContainedTaskRuntimeErrorClass::Fatal
        } else {
            ContainedTaskRuntimeErrorClass::Nonfatal
        }
    }

    fn capture(&mut self) -> Result<Frame, Self::Error> {
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
            let admission = lock(&instance_guard, "lock_instance_admission")?;
            let frame_id =
                self.host.events.issuer().mint_frame_id().map_err(|_| {
                    RequestFailure::poison_without_terminal(runtime_identifier_error())
                })?;
            let input_action_id = self.post_input_action_id.take();
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
                .execution
                .capture_retained_with_geometry_session_and_registration_guard(
                    self.instance_alias,
                    registration,
                );
            self.task_timing
                .finish_boundary(backend_started, captured.is_ok());
            match captured {
                Ok((frame, geometry_session)) => {
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
                        let persistence = (|| {
                            if self.capture_evidence.pipeline.is_none() {
                                self.capture_evidence.pipeline =
                                    Some(CapturePipeline::open_with_store(
                                        Arc::clone(&self.host.artifacts),
                                        frame_retention::spill_root(
                                            self.host.artifacts.root(),
                                            self.run_id.transport(),
                                        )?,
                                        CapturePipelineConfig {
                                            frame_store:
                                                frame_retention::capture_frame_store_config(),
                                            retention_class: RetentionClass::DebugFull,
                                            redaction_state: ArtifactRedactionState::NotRequired,
                                            ..CapturePipelineConfig::default()
                                        },
                                        write_context.clone(),
                                        &mut sink,
                                    )?);
                            }
                            let pipeline = self
                                .capture_evidence
                                .pipeline
                                .as_mut()
                                .expect("capture pipeline initialized");
                            let result = pipeline.record_frame(
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
                                    frame: frame.clone(),
                                },
                                write_context.clone(),
                                &mut sink,
                            )?;
                            if !result.frame.warnings.is_empty() {
                                return Err(ArtifactStoreError::fatal(
                                    "capture_spill_failed",
                                    "persist_contained_task_frame",
                                    result.frame.warnings.join("; "),
                                ));
                            }
                            let reference = pipeline.persist_frame(frame_index, &mut sink)?;
                            pipeline.poll_pressure(&write_context, &mut sink)?;
                            Ok(reference)
                        })();
                        let reference = match persistence {
                            Ok(reference) => reference,
                            Err(error) => {
                                self.capture_evidence
                                    .pipeline_failure
                                    .get_or_insert(error.clone());
                                return Err(online_observation::observation_artifact_failure(
                                    error,
                                ));
                            }
                        };
                        self.capture_evidence.persisted(frame_index, &reference)?;
                        self.last_frame_id = Some(frame_id);
                        self.last_capture_input_action_id = input_action_id;
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
                        Ok(frame)
                    })();
                    self.task_timing
                        .finish_boundary(material_started, material.is_ok());
                    let frame = material?;
                    self.retain_task_geometry(&frame, frame_id, geometry_session)?;
                    Ok(frame)
                }
                Err(error) => {
                    let error = self
                        .host
                        .finish_capture_failure_while_guarded(error, links.clone(), &admission)
                        .map_err(RequestFailure::poison_without_terminal)?;
                    let runtime_error =
                        RuntimeHostError::execution("run_contained_task_capture", &error);
                    if self
                        .host
                        .retain_unconfirmed_resources(&runtime_error, links.clone())?
                    {
                        return Err(RequestFailure::poison_without_terminal(runtime_error));
                    }
                    let payload = CapturePayloadDraft::failed_with_causes(
                        EventAction::CaptureObserve,
                        DiagnosticCode::CaptureFailed,
                        EffectDisposition::NotPerformed,
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
                            code: runtime_error.code(),
                            severity: if runtime_error.is_fatal() {
                                EventSeverity::Fatal
                            } else {
                                EventSeverity::Warning
                            },
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
                    "contained_task_sampling_seed_reused",
                    "derive_contained_task_action_seed",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        Ok(Some(action_seed))
    }

    fn input(&mut self, action: InputAction) -> Result<(), Self::Error> {
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
                    before_frame_id: self.last_frame_id.map(|frame| *frame.transport()),
                },
            )?;
            self.ensure_active()?;
            if let RuntimeResult::InputCommitted { action_id } = success.result {
                self.post_input_action_id = Some(action_id);
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
                        "contained_task_input_result_invalid",
                        "run_contained_task",
                        RuntimeErrorCode::RuntimeFatal,
                    ),
                ))
            }
        })();
        self.task_timing
            .finish_boundary(input_started, result.is_ok());
        result
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        if !matches!(&trace, ContainedTaskTrace::PackageAdmitted { .. }) {
            self.ensure_active()?;
        }
        let trace = self.offset_trace(trace)?;
        match trace {
            ContainedTaskTrace::PackageAdmitted {
                task_label,
                package_label,
                package_sha256,
            } => {
                #[cfg(test)]
                self.host
                    .consume_contained_task_checkpoint_for_test(ContainedTaskCheckpointIdentity {
                        request_id: self.control.request_id,
                        instance_id: self.control.instance_id,
                        lease_id: self.token.lease_id(),
                    })
                    .map_err(RequestFailure::poison_without_terminal)?;
                self.append_task(
                    EventSeverity::Info,
                    self.links(),
                    TaskPayloadDraft::semantic_with_lease_expiry(
                        TaskSemanticFact::PackageAdmitted {
                            package_label,
                            task_label,
                            package_sha256,
                            response_deadline_monotonic_ms: Some(self.control.deadline()),
                        },
                        self.token.expires_at_monotonic_ms(),
                        AuditInput::new(),
                    ),
                )
            }
            ContainedTaskTrace::RunStarted => self.append_task(
                EventSeverity::Info,
                self.links(),
                TaskPayloadDraft::semantic(TaskSemanticFact::RunStarted, AuditInput::new()),
            ),
            ContainedTaskTrace::EntryRecognition {
                required_page,
                matched,
            } => {
                self.record_entry_fact(TaskSemanticFact::EntryRecognition {
                    phase: TaskEntryRecognitionPhase::Initial,
                    required_page,
                    matched,
                })?;
                if self.entry_preflight_recorded {
                    return if matched {
                        Ok(())
                    } else {
                        self.record_entry_fact(TaskSemanticFact::EntryTargetDisposition {
                            disposition: TaskEntryTargetDisposition::FailClosed,
                            failure_code: Some("contained_task_home_entry_not_matched".to_owned()),
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
                    failure_code: (!matched)
                        .then(|| "contained_task_home_entry_not_matched".to_owned()),
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
                            "contained_task_frame_identity_missing",
                            "run_contained_task",
                            RuntimeErrorCode::RuntimeFatal,
                        ))
                    })?;
                    let input_action_id = self.last_capture_input_action_id.take();
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
                        CapturePayloadDraft::completed(
                            EventAction::CaptureObserve,
                            EffectDisposition::NotPerformed,
                            width,
                            height,
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
                    )
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
                                "contained_task_recognition_state_invalid",
                                "run_contained_task",
                                RuntimeErrorCode::RuntimeFatal,
                            ),
                        ));
                    }
                    self.capture_evidence
                        .pin_last(PinnedFrameReason::RecognitionEvidence)?;
                    let frame_id = self.last_frame_id.ok_or_else(|| {
                        RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                            "contained_task_frame_identity_missing",
                            "run_contained_task",
                            RuntimeErrorCode::RuntimeFatal,
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
            } => {
                let observed_frame_id = self.last_frame_id.map(|id| *id.transport());
                let observed_recognition_id = self.current_recognition_id.map(|id| *id.transport());
                let started = Instant::now();
                let budget_before = self.task_timing.budget_at(started);
                let result = (|| {
                    let frame_id = self.last_frame_id.ok_or_else(|| {
                        RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                            "contained_task_frame_identity_missing",
                            "run_contained_task",
                            RuntimeErrorCode::RuntimeFatal,
                        ))
                    })?;
                    let recognition_id = self.current_recognition_id.ok_or_else(|| {
                        RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                            "contained_task_recognition_identity_missing",
                            "run_contained_task",
                            RuntimeErrorCode::RuntimeFatal,
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
                                matched_page: page_label,
                                frame_width: width,
                                frame_height: height,
                            },
                            AuditInput::new(),
                        ),
                    );
                    let appended = appended.map(|_| ());
                    self.task_timing
                        .finish_append(append_started, appended.is_ok(), observation);
                    appended?;
                    self.current_recognition_id = None;
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
                if self
                    .step_actions
                    .insert(step_index, (action_id, operation_label.clone()))
                    .is_some()
                {
                    return Err(RequestFailure::poison_without_terminal(
                        RuntimeHostError::fatal(
                            "contained_task_step_identity_reused",
                            "run_contained_task",
                            RuntimeErrorCode::RuntimeFatal,
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
            ContainedTaskTrace::EffectIntent {
                step_index,
                operation_label,
                action,
                sampling,
                guard: _,
            } => {
                action.validate().map_err(|_| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        "contained_task_effect_invalid",
                        "run_contained_task",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?;
                let action_id =
                    contained_task_step_action(&self.step_actions, step_index, &operation_label)?;
                let fact = TaskSemanticFact::EffectIntent {
                    step_index,
                    operation_label,
                    action,
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
                        "contained_task_step_identity_missing",
                        "run_contained_task",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?;
                self.append_task(
                    EventSeverity::Info,
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
                    self.post_admission_ocr_observations == 0
                        || !self.post_admission_ocr_comparison_recorded
                } else {
                    self.post_admission_ocr_observations != 0
                        || self.post_admission_ocr_comparison_recorded
                };
                if self.finalizing.replace(outcome).is_some()
                    || !self.step_actions.is_empty()
                    || stability_finalization_invalid
                    || post_admission_ocr_finalization_invalid
                {
                    return Err(RequestFailure::poison_without_terminal(
                        RuntimeHostError::fatal(
                            "contained_task_finalizing_state_invalid",
                            "run_contained_task",
                            RuntimeErrorCode::RuntimeFatal,
                        ),
                    ));
                }
                self.append_task(
                    EventSeverity::Info,
                    self.links(),
                    TaskPayloadDraft::semantic(
                        TaskSemanticFact::Finalizing { outcome },
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
                "contained_task_step_identity_mismatch",
                "run_contained_task",
                RuntimeErrorCode::RuntimeFatal,
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

fn prepare_contained_task(
    instance_alias: &str,
    request: &ContainedTaskRequest,
    vision_provider: Option<Arc<dyn RecognitionVisionProvider>>,
    deadline: Instant,
) -> Result<PreparedContainedTask, RequestFailure> {
    let path = Path::new(request.package_path());
    if !path.is_absolute() {
        return Err(contained_task_package_failure(
            "contained_task_path_not_absolute",
        ));
    }
    if matches!(
        request.expected_sha256(),
        actingcommand_contract::PackageRef::GitSourceTree(_)
    ) {
        return PreparedContainedTask::load_path(
            instance_alias,
            path,
            request.expected_sha256(),
            vision_provider,
            deadline,
        )
        .map_err(|error| contained_task_declaration_failure(request, error));
    }
    let path = fs::canonicalize(path)
        .map_err(|_| contained_task_package_failure("contained_task_package_open_failed"))?;
    let metadata = fs::metadata(&path)
        .map_err(|_| contained_task_package_failure("contained_task_package_metadata_failed"))?;
    if !metadata.is_file() || metadata.len() > DEFAULT_MAX_COMPRESSED_BYTES {
        return Err(contained_task_package_failure(
            "contained_task_package_size_invalid",
        ));
    }
    let bytes = fs::read(&path)
        .map_err(|_| contained_task_package_failure("contained_task_package_read_failed"))?;
    let expected =
        ExternalExpectedSha256::parse_hex(request.expected_sha256().legacy_sha256().ok_or_else(
            || contained_task_package_failure("contained_task_package_hash_invalid"),
        )?)
        .map_err(|_| contained_task_package_failure("contained_task_package_hash_invalid"))?;
    match vision_provider {
        Some(provider) => PreparedContainedTask::load_with_vision_provider(
            instance_alias,
            &bytes,
            expected,
            provider,
        ),
        None => PreparedContainedTask::load(instance_alias, &bytes, expected),
    }
    .map_err(|error| contained_task_declaration_failure(request, error))
}

fn contained_task_declaration_failure(
    request: &ContainedTaskRequest,
    error: actingcommand_execution_kernel::ContainedTaskError,
) -> RequestFailure {
    let mut failure = contained_task_package_failure(error.code());
    if let Some(issue) = error.declaration_issue() {
        // Declaration parsing happens only after the source snapshot or ZIP identity was verified.
        failure.error.lifecycle.resource_declaration = Some(Box::new(
            actingcommand_contract::ResourceDeclarationRejection {
                declared_package: request.expected_sha256().clone(),
                verified_package: Some(request.expected_sha256().clone()),
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

fn contained_task_package_failure(code: &'static str) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::request(code, "run_contained_task", RuntimeErrorCode::PackageInvalid),
        RuntimeReceiptState::Denied,
        None,
    )
}

fn select_scheduling_disposition(
    events: &[PersistedEvent],
    outcome: TaskOutcome,
    final_page: Option<&str>,
    executed_steps: Option<u32>,
    contract: Option<&(String, SchedulingOutcomeDeclaration)>,
    selected_outcome_key: Option<&str>,
) -> Result<Option<SchedulingDisposition>, RequestFailure> {
    let Some((game, declaration)) = contract else {
        if selected_outcome_key.is_some() {
            return Err(scheduling_outcome_failure(
                "contained_task_outcome_declaration_missing",
            ));
        }
        return Ok(None);
    };
    if outcome != TaskOutcome::Success {
        return Err(scheduling_outcome_failure(
            "contained_task_outcome_requires_success",
        ));
    }
    let executed_steps = executed_steps
        .ok_or_else(|| scheduling_outcome_failure("contained_task_outcome_progress_missing"))?;
    let final_page = final_page.ok_or_else(|| {
        scheduling_outcome_failure("contained_task_outcome_terminal_page_missing")
    })?;
    let designated_effects = declaration
        .designated_operation()
        .map(|designated| {
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
                    (operation_label == designated)
                        .then_some((*step_index, operation_label.clone()))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if designated_effects
        .iter()
        .any(|(step_index, _)| *step_index >= executed_steps)
    {
        return Err(scheduling_outcome_failure(
            "contained_task_outcome_effect_outside_executed_steps",
        ));
    }
    if designated_effects.len() > 1 {
        return Err(scheduling_outcome_failure(
            "contained_task_outcome_designated_effect_duplicate",
        ));
    }
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
                "contained_task_outcome_final_observation_missing",
            ));
        }
    } else {
        let final_steps = events
            .iter()
            .filter_map(|event| {
                let EventPayload::Task(TaskPayload::Semantic(payload)) = event.payload() else {
                    return None;
                };
                let TaskSemanticFact::StepFinished {
                    step_index,
                    page_label,
                    ..
                } = payload.fact()
                else {
                    return None;
                };
                (*step_index == executed_steps - 1).then_some(page_label.as_str())
            })
            .collect::<Vec<_>>();
        let [observed_page] = final_steps.as_slice() else {
            return Err(scheduling_outcome_failure(
                "contained_task_outcome_final_step_not_unique",
            ));
        };
        if !page_anchor_matches(game, observed_page, final_page) {
            return Err(scheduling_outcome_failure(
                "contained_task_outcome_final_page_conflict",
            ));
        }
    }
    if let [(step_index, operation_label)] = designated_effects.as_slice() {
        let lifecycle_count = events
            .iter()
            .filter(|event| {
                let EventPayload::Task(TaskPayload::Semantic(payload)) = event.payload() else {
                    return false;
                };
                matches!(
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
                "contained_task_outcome_designated_step_not_unique",
            ));
        }
    }
    let effect = if let [(step_index, operation_label)] = designated_effects.as_slice() {
        SchedulingEffectEvidence::DesignatedEffectCompleted {
            step_index: *step_index,
            operation_label: operation_label.clone(),
        }
    } else {
        SchedulingEffectEvidence::NoDesignatedEffect
    };
    let condition = match &effect {
        SchedulingEffectEvidence::NoDesignatedEffect => {
            SchedulingEffectCondition::NoDesignatedEffect
        }
        SchedulingEffectEvidence::DesignatedEffectCompleted { .. } => {
            SchedulingEffectCondition::DesignatedEffectCompleted
        }
    };
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
            "contained_task_outcome_mapping_missing"
        } else {
            "contained_task_outcome_mapping_conflict"
        }));
    };
    if selected_outcome_key.is_some_and(|selected| selected != mapping.outcome_key()) {
        return Err(scheduling_outcome_failure(
            "contained_task_outcome_comparison_conflict",
        ));
    }
    SchedulingDisposition::new(mapping.outcome_key(), effect)
        .map(Some)
        .map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "contained_task_outcome_disposition_invalid",
                "append_contained_task_terminal",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })
}

fn scheduling_outcome_failure(code: &'static str) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::request(
            code,
            "append_contained_task_terminal",
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

fn contained_task_replay_denied(code: &'static str) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::request(
            code,
            "recover_contained_task",
            RuntimeErrorCode::ProtocolInvalid,
        ),
        RuntimeReceiptState::Denied,
        None,
    )
}

fn contained_task_sampling_seed<T: serde::Serialize>(value: &T) -> RuntimeHostResult<u64> {
    let bytes = serde_json::to_vec(value).map_err(|_| {
        RuntimeHostError::fatal(
            "contained_task_sampling_seed_encode_failed",
            "derive_contained_task_sampling_seed",
            RuntimeErrorCode::RuntimeFatal,
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
            "contained_task_sampling_seed_missing",
            "derive_contained_task_action_seed",
            RuntimeErrorCode::RuntimeFatal,
        )
    })
}

impl HostShared {
    #[cfg(test)]
    fn consume_contained_task_checkpoint_for_test(
        &self,
        identity: ContainedTaskCheckpointIdentity,
    ) -> RuntimeHostResult<()> {
        let hook = {
            let mut slot = lock(
                &self.contained_task_checkpoint_test_hook,
                "consume_contained_task_checkpoint_test_hook",
            )?;
            let should_consume = match slot.as_mut() {
                Some(hook)
                    if hook.request_id == identity.request_id
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
            "record_contained_task_checkpoint_test_identity",
        )? = Some(identity);
        (hook.action)(identity);
        hook.consumed.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    pub(super) fn cancel_contained_task(
        &self,
        task_request_id: RequestId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let active = lock(&self.contained_runs, "read_active_contained_run")?
            .get(&task_request_id)
            .cloned();
        if let Some(control) = &active {
            if !control.client_cancellable {
                return Err(RequestFailure::request(
                    RuntimeHostError::request(
                        "scheduled_contained_task_not_client_cancellable",
                        "cancel_contained_task",
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
                    "query_contained_task_cancellation",
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
                    "contained_task_active_identity_mismatch",
                    "cancel_contained_task",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        if events.is_empty() && active.is_none() {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    "contained_task_cancellation_identity_missing",
                    "cancel_contained_task",
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
                        outcome,
                        failure_code,
                        ..
                    } => Some((event, *outcome, failure_code.as_deref())),
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>();
        if terminals.len() > 1 {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "contained_task_terminal_state_inconsistent",
                    "cancel_contained_task",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        if let [(task_terminal, outcome, failure_code)] = terminals.as_slice() {
            let lease_id = task_terminal.links().lease_id().copied().ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "contained_task_identity_missing",
                    "cancel_contained_task",
                    RuntimeErrorCode::RuntimeFatal,
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
                        "query_contained_task_lease_terminal",
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
                        "contained_task_lease_terminal_state_inconsistent",
                        "cancel_contained_task",
                        RuntimeErrorCode::RuntimeFatal,
                    ),
                ));
            }
            if let [lease_terminal] = lease_terminals.as_slice() {
                let reason = (*outcome == TaskOutcome::Cancelled).then_some(match *failure_code {
                    Some("contained_task_deadline_exceeded") => {
                        ContainedTaskCancellationReason::DeadlineExceeded
                    }
                    Some("contained_task_cancelled") => {
                        ContainedTaskCancellationReason::ClientRequested
                    }
                    _ => ContainedTaskCancellationReason::RecoveredAfterRestart,
                });
                return Ok(OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: Some(terminal(lease_terminal)),
                    result: RuntimeResult::ContainedTaskCancellation {
                        task_request_id,
                        status: ContainedTaskCancellationStatus::Terminal {
                            deadline_monotonic_ms,
                            outcome: *outcome,
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
        if let Some(recovered) =
            self.recover_contained_task(original, request, resolved.instance_id(), task_request)?
        {
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
        active_run
            .control
            .set_deadline(
                self.monotonic_ms()
                    .and_then(|now| {
                        now.checked_add(task_request.response_deadline_ms())
                            .ok_or_else(|| {
                                RuntimeHostError::fatal(
                                    "contained_task_deadline_overflow",
                                    "derive_contained_task_deadline",
                                    RuntimeErrorCode::RuntimeFatal,
                                )
                            })
                    })
                    .map_err(RequestFailure::poison_without_terminal)?,
            )
            .map_err(RequestFailure::poison_without_terminal)?;
        let prepared = prepare_contained_task(
            instance_alias,
            task_request,
            self.execution.vision_provider(),
            self.package_material_deadline(active_run.control.deadline())?,
        )?;
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
        let acquired = self.acquire_lease(RuntimeLeaseAcquisition {
            request,
            request_id: original.request_id(),
            instance_alias,
            holder_id,
            connection_id,
            run_links: None,
            lease_ttl_ms: Some(lease_ttl_ms),
        })?;
        let RuntimeResult::LeaseGranted { token } = acquired.result else {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "contained_task_lease_result_invalid",
                    "run_contained_task",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        };
        let deadline_monotonic_ms = match self.contained_task_deadline(task_request, &token) {
            Ok(deadline) => deadline,
            Err(failure) => {
                return Err(self.cleanup_composite_failure(token, connection_id, failure));
            }
        };
        active_run
            .control
            .set_deadline(deadline_monotonic_ms)
            .map_err(RequestFailure::poison_without_terminal)?;
        self.execute_contained_task_with_lease(
            original,
            request,
            instance_alias,
            connection_id,
            prepared,
            task_request,
            token,
            task_id,
            run_id,
            ExecutionBackendProvenance::PhysicalDevice,
            None,
            active_run.control(),
            None,
        )
    }

    fn package_material_deadline(
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
            .ok_or_else(|| contained_task_package_failure("contained_task_deadline_overflow"))
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
                    "contained_task_deadline_overflow",
                    "derive_contained_task_deadline",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })?;
        let reserve = lock(&self.scheduler, "read_contained_task_deadline_config")?
            .config()
            .maximum_client_heartbeat_interval_ms;
        let lease_boundary = token
            .expires_at_monotonic_ms()
            .checked_sub(reserve)
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "contained_task_lease_deadline_invalid",
                    "derive_contained_task_deadline",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })?;
        let deadline = requested.min(lease_boundary);
        if deadline <= now {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    "contained_task_deadline_unavailable",
                    "derive_contained_task_deadline",
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
        let reserve = lock(&self.scheduler, "read_contained_task_lease_ttl_config")?
            .config()
            .maximum_client_heartbeat_interval_ms;
        request
            .response_deadline_ms()
            .checked_add(reserve)
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "contained_task_lease_ttl_overflow",
                    "derive_contained_task_lease_ttl",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })
    }

    pub(super) fn run_scheduled_contained_task(
        &self,
        context: &PolicyRunContext,
        task_request: &ContainedTaskRequest,
    ) -> Result<(RuntimeRequest, OperationSuccess), RequestFailure> {
        lock(&self.policy, "validate_policy_run_context")?
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
                    "policy_run_request_invalid",
                    "run_scheduled_contained_task",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        };
        context.package_digest().validate().map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "policy_run_package_digest_invalid",
                "run_scheduled_contained_task",
                RuntimeErrorCode::RuntimeFatal,
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
                    "policy_run_identity_mismatch",
                    "run_scheduled_contained_task",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        let resolved = self.resolve_instance(instance_alias)?;
        if resolved.instance_id() != token.instance_id() {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "policy_run_lease_instance_mismatch",
                    "run_scheduled_contained_task",
                    RuntimeErrorCode::RuntimeFatal,
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
                "policy_task_request_invalid",
                "run_scheduled_contained_task",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        let validated = task_request_message.validate().map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "policy_task_request_invalid",
                "run_scheduled_contained_task",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        let connection_id = ConnectionId::new(POLICY_CONNECTION_VALUE).map_err(|error| {
            RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                "build_policy_connection",
                &error,
            ))
        })?;
        self.require_business_capacity(self.events.request_links(
            &validated,
            Some(resolved.instance_id()),
            Some(token.lease_id()),
            None,
        ))?;
        let source_deadline = matches!(
            task_request.expected_sha256(),
            actingcommand_contract::PackageRef::GitSourceTree(_)
        )
        .then(|| self.contained_task_deadline(task_request, token))
        .transpose()?;
        let material_deadline = source_deadline
            .map(|deadline| self.package_material_deadline(deadline))
            .transpose()?
            .unwrap_or_else(Instant::now);
        let prepared = if execution_provenance == ExecutionBackendProvenance::PhysicalDevice {
            Some(prepare_contained_task(
                instance_alias,
                task_request,
                self.execution.vision_provider(),
                material_deadline,
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
                    "policy_run_execution_identity_mismatch",
                    "run_scheduled_contained_task",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
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
                                    "contained_task_deadline_overflow",
                                    "derive_contained_task_deadline",
                                    RuntimeErrorCode::RuntimeFatal,
                                )
                            })
                    })
                    .map_err(RequestFailure::poison_without_terminal)?,
            )
            .map_err(RequestFailure::poison_without_terminal)?;
        if let Some(deadline) = source_deadline {
            active_run
                .control
                .set_deadline(deadline)
                .map_err(RequestFailure::poison_without_terminal)?;
        }
        let prepared = match prepared {
            Some(prepared) => prepared,
            None => prepare_contained_task(
                instance_alias,
                task_request,
                self.execution.vision_provider(),
                material_deadline,
            )?,
        };
        let expected_outcome_keys = lock(&self.policy, "validate_policy_outcome_declaration")?
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
        if !expected_outcome_keys.is_empty() && expected_outcome_keys != declared_outcome_keys {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    "policy_run_outcome_declaration_mismatch",
                    "run_scheduled_contained_task",
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
        let success = self.execute_contained_task_with_lease(
            &task_request_message,
            &validated,
            instance_alias,
            connection_id,
            prepared,
            task_request,
            token.clone(),
            context.issued_task_id(),
            context.issued_run_id(),
            execution_provenance,
            Some(run_links),
            active_run.control(),
            Some(context.request().request_id()),
        )?;
        Ok((task_request_message, success))
    }

    #[allow(clippy::too_many_arguments, clippy::let_and_return)]
    fn execute_contained_task_with_lease(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        connection_id: ConnectionId,
        prepared: PreparedContainedTask,
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
        let scheduling_outcome = prepared
            .scheduling_outcome()
            .cloned()
            .map(|declaration| (prepared.game().to_owned(), declaration));
        let expected_stability_declaration = prepared.stability_termination().cloned();
        let expects_post_admission_ocr = prepared.has_post_admission_ocr();
        let sampling_run_seed = Some(
            contained_task_sampling_seed(&("xorshift64_uniform_rect_v1/run", run_id.transport()))
                .map_err(RequestFailure::poison_without_terminal)?,
        );
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
            last_frame_id: None,
            geometry_session: None,
            geometry_frame: None,
            geometry_initial: None,
            geometry_deadline: None,
            geometry_rechecked: false,
            input_step_action_id: None,
            post_input_action_id: None,
            last_capture_input_action_id: None,
            expected_stability_declaration,
            stability: None,
            expects_post_admission_ocr,
            post_admission_ocr_observations: 0,
            post_admission_ocr_comparison_recorded: false,
            current_recognition_id: None,
            step_actions: BTreeMap::new(),
            step_index_offset: 0,
            executed_steps: Some(0),
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
        };
        // Zero-input fields confirm the required entry in the interpreter's first capture.
        let mut execution = if let Err(failure) = runtime
            .begin_diagnostic()
            .and_then(|()| runtime.record_initial_configuration(task_request, &prepared))
        {
            Err(ContainedTaskRunError::Boundary(failure))
        } else if prepared.required_home_entry_page().is_some()
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
            execution
        };
        if let Err(ContainedTaskRunError::Task(error)) = &execution {
            runtime
                .task_timing
                .task_failure(error.timing(), error.timing_check_position());
        }
        runtime.recheck_task_geometry(&mut execution);
        runtime.task_timing.begin_finalization();
        let post_admission_ocr_failure_diagnostic = match &execution {
            Err(ContainedTaskRunError::Task(error)) => {
                runtime.record_post_admission_ocr_failure(error.code(), error.detail())
            }
            _ => Ok(()),
        };
        let fatal = post_admission_ocr_failure_diagnostic.is_err()
            || matches!(&execution,
                Err(ContainedTaskRunError::Boundary(failure) | ContainedTaskRunError::NonfatalOperation(failure))
                    if failure.poison_runtime || failure.error.is_fatal());
        let capacity_refused = matches!(&execution,
            Err(ContainedTaskRunError::Boundary(failure) | ContainedTaskRunError::NonfatalOperation(failure))
                if failure.error.code() == "capacity_admission_refused" && !failure.error.is_fatal());
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
                        &RuntimeHostError::request(
                            error.code(),
                            "run_contained_task",
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
                ) {
                    let reason = control
                        .cancellation_reason(
                            self.monotonic_ms()
                                .map_err(RequestFailure::poison_without_terminal)?,
                        )
                        .ok_or_else(|| {
                            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                                "contained_task_cancellation_state_missing",
                                "finalize_contained_task_cancellation",
                                RuntimeErrorCode::RuntimeFatal,
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
                            failure_code: Some(failure.error.code()),
                            failure_severity: scheduled.then_some(EventSeverity::Warning),
                            scheduling_outcome: None,
                            selected_scheduling_outcome: None,
                            capture_summary: Some(capture_summary),
                            task_timing: Some(task_timing.clone()),
                        },
                    )?;
                    if scheduled {
                        failure.terminal = Some(terminal(&task_terminal));
                        failure.task_failure = Some(TaskFailureEvidence {
                            code: failure.error.code(),
                            severity: EventSeverity::Warning,
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
                        run_links,
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
                            lease_terminal: ContainedTaskLeaseTerminal::Released,
                        },
                    });
                }
                let task_failure = scheduled.then_some(failure.task_failure).flatten();
                let failure_severity = task_failure.map(|evidence| evidence.severity);
                if failure_severity.is_some() || !scheduled && !failure.poison_runtime {
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
                            failure_code: Some(
                                task_failure
                                    .map(|evidence| evidence.code)
                                    .unwrap_or_else(|| failure.error.code()),
                            ),
                            failure_severity,
                            scheduling_outcome: None,
                            selected_scheduling_outcome: None,
                            capture_summary: Some(capture_summary),
                            task_timing: Some(task_timing.clone()),
                        },
                    )?;
                    failure.terminal = Some(terminal(&event));
                    if failure.error.lifecycle.capacity.is_none()
                        && task_failure.is_none_or(|evidence| evidence.code == failure.error.code())
                    {
                        let _ = failure
                            .error
                            .lifecycle
                            .recorded_event
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
                if matches!(
                    error.code(),
                    "contained_task_guard_refused"
                        | "contained_task_guard_evaluation_failed"
                        | "contained_task_guard_target_missing"
                        | "contained_task_guard_target_invalid"
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
                let failure_severity = scheduled.then_some(EventSeverity::Warning);
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
                        failure_code: Some(error.code()),
                        failure_severity,
                        scheduling_outcome: None,
                        selected_scheduling_outcome: None,
                        capture_summary: Some(capture_summary),
                        task_timing: Some(task_timing.clone()),
                    },
                )?;
                let mut failure = RequestFailure::request(
                    RuntimeHostError::request(
                        error.code(),
                        "run_contained_task",
                        RuntimeErrorCode::BackendOperationFailed,
                    ),
                    RuntimeReceiptState::Failed,
                    Some(terminal(&event)),
                );
                failure.task_failure = failure_severity.map(|severity| TaskFailureEvidence {
                    code: error.code(),
                    severity,
                });
                let _ = failure
                    .error
                    .lifecycle
                    .recorded_event
                    .set(*event.event_id());
                let failure = match post_admission_ocr_failure_diagnostic {
                    Ok(()) => failure,
                    Err(diagnostic_failure) => {
                        failure.replace_with_poison(*diagnostic_failure.error)
                    }
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
                    "contained_task_finalizing_state_invalid",
                    "run_contained_task",
                    RuntimeErrorCode::RuntimeFatal,
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
                failure_code: None,
                failure_severity: None,
                scheduling_outcome,
                selected_scheduling_outcome: outcome.selected_scheduling_outcome,
                capture_summary: Some(capture_summary),
                task_timing: Some(task_timing),
            },
        )?;
        match self.release_lease(
            request,
            original.request_id(),
            &token,
            connection_id,
            run_links,
        ) {
            Ok(_) => Ok(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: Some(terminal(&task_terminal)),
                result: RuntimeResult::ContainedTaskCompleted {
                    run_id: *run_id.transport(),
                    task_id: *task_id.transport(),
                    task_request_id: original.request_id(),
                    response_deadline_monotonic_ms: Some(control.deadline()),
                    outcome: outcome.outcome,
                    final_page: outcome.final_page,
                    executed_steps: outcome.executed_steps,
                },
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
        let Some(required_home) = prepared.required_home_entry_page().map(str::to_owned) else {
            return prepared.run(runtime);
        };

        let initial_home = prepared.recognize_required_home(runtime)?;
        runtime
            .record_entry_fact(TaskSemanticFact::EntryRecognition {
                phase: TaskEntryRecognitionPhase::Initial,
                required_page: required_home.clone(),
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
                    failure_code: None,
                })
                .map_err(ContainedTaskRunError::Boundary)?;
            return prepared.run(runtime);
        }

        let Some(binding) = task_request.recovery() else {
            return fail_contained_task_entry(
                runtime,
                "contained_task_home_recovery_binding_missing",
            );
        };
        let recovery_request =
            ContainedTaskRequest::new(binding.package_path(), binding.expected_sha256()).map_err(
                |_| ContainedTaskRunError::task("contained_task_home_recovery_binding_invalid"),
            )?;
        let recovery = match prepare_contained_task(
            instance_alias,
            &recovery_request,
            self.execution.vision_provider(),
            self.package_material_deadline(runtime.control.deadline())
                .map_err(ContainedTaskRunError::Boundary)?,
        ) {
            Ok(recovery) => recovery,
            Err(mut failure) => {
                let code = failure.error.code();
                runtime
                    .record_entry_fact(TaskSemanticFact::EntryRecoveryFailed {
                        package_sha256: binding.expected_sha256().to_owned(),
                        failure_code: code.to_owned(),
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
                            failure_code: Some(code.to_owned()),
                        })
                        .map_err(ContainedTaskRunError::Boundary)?;
                    failure.state = RuntimeReceiptState::Failed;
                    failure.task_failure = Some(TaskFailureEvidence {
                        code,
                        severity: EventSeverity::Warning,
                    });
                    return Err(ContainedTaskRunError::Boundary(failure));
                }
                return fail_contained_task_entry(runtime, code);
            }
        };
        if !recovery.is_entry_recovery_compatible() {
            return fail_contained_task_entry(
                runtime,
                "contained_task_home_recovery_package_incompatible",
            );
        }
        let recovery_sha256 = recovery.package_sha256().to_owned();
        runtime
            .record_entry_fact(TaskSemanticFact::EntryRecoveryPackageAdmitted {
                package_sha256: recovery_sha256.clone(),
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        let previous_timing = runtime.task_timing.context();
        let recovery_execution = {
            if runtime.configuration_records > 0 {
                runtime
                    .record_configuration(
                        EffectiveConfigurationFacts::EntryRecovery {
                            package_sha256: recovery.package_sha256().to_owned(),
                            timing: recovery.effective_timing(),
                        },
                        None,
                        None,
                        None,
                    )
                    .map_err(ContainedTaskRunError::Boundary)?;
            }
            let mut recovery_runtime = EntryRecoveryRuntime { inner: runtime };
            recovery.run_entry_recovery(&mut recovery_runtime)
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
                if matches!(
                    error.code(),
                    "contained_task_recognition_failed" | "contained_task_page_unknown"
                ) {
                    let mut primary = RuntimeHostError::request(
                        error.code(),
                        "run_contained_task",
                        RuntimeErrorCode::BackendOperationFailed,
                    );
                    if let Some(detail) = error.detail() {
                        primary = primary.with_native_detail(detail.to_owned());
                    }
                    runtime
                        .record_geometry_triggered_recovery_failure(recovery_sha256, &primary)
                        .map_err(ContainedTaskRunError::Boundary)?;
                    return Err(ContainedTaskRunError::Task(error));
                }
                runtime
                    .record_entry_fact(TaskSemanticFact::EntryRecoveryFailed {
                        package_sha256: recovery_sha256,
                        failure_code: error.code().to_owned(),
                    })
                    .map_err(ContainedTaskRunError::Boundary)?;
                return fail_contained_task_entry(runtime, error.code());
            }
            Err(
                ContainedTaskRunError::Boundary(failure)
                | ContainedTaskRunError::NonfatalOperation(failure),
            ) => {
                if nonfatal_operation
                    && !failure.poison_runtime
                    && !failure.error.is_fatal()
                    && matches!(
                        failure.error.code(),
                        "input_backend_operation_failed" | "input_backend_open_failed"
                    )
                {
                    runtime
                        .record_geometry_triggered_recovery_failure(recovery_sha256, &failure.error)
                        .map_err(ContainedTaskRunError::Boundary)?;
                    return Err(ContainedTaskRunError::NonfatalOperation(failure));
                }
                let code = failure.error.code();
                runtime
                    .record_entry_fact(TaskSemanticFact::EntryRecoveryFailed {
                        package_sha256: recovery_sha256,
                        failure_code: code.to_owned(),
                    })
                    .map_err(ContainedTaskRunError::Boundary)?;
                runtime
                    .record_entry_fact(TaskSemanticFact::EntryTargetDisposition {
                        disposition: TaskEntryTargetDisposition::FailClosed,
                        failure_code: Some(code.to_owned()),
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
                "contained_task_home_recovery_final_page_missing",
            );
        };
        runtime
            .record_entry_fact(TaskSemanticFact::EntryRecoveryCompleted {
                package_sha256: recovery_sha256,
                final_page: final_page.clone(),
                executed_steps: recovery_outcome.executed_steps,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        if !prepared.terminal_matches_required_home(&final_page) {
            return fail_contained_task_entry(
                runtime,
                "contained_task_home_recovery_terminal_non_home",
            );
        }

        let post_recovery_home = prepared.recognize_required_home(runtime)?;
        runtime
            .record_entry_fact(TaskSemanticFact::EntryRecognition {
                phase: TaskEntryRecognitionPhase::PostRecovery,
                required_page: required_home,
                matched: post_recovery_home,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        if !post_recovery_home {
            return fail_contained_task_entry(
                runtime,
                "contained_task_home_recovery_persistently_non_home",
            );
        }
        if recovery_outcome
            .executed_steps
            .checked_add(prepared.maximum_executed_steps())
            .is_none_or(|maximum| maximum > 1_000)
        {
            return fail_contained_task_entry(runtime, "contained_task_home_recovery_step_limit");
        }
        runtime.step_index_offset = recovery_outcome.executed_steps;
        runtime
            .record_entry_fact(TaskSemanticFact::EntryTargetDisposition {
                disposition: TaskEntryTargetDisposition::Started,
                failure_code: None,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        let mut outcome = prepared.run(runtime)?;
        outcome.executed_steps = outcome
            .executed_steps
            .checked_add(recovery_outcome.executed_steps)
            .ok_or_else(|| {
                ContainedTaskRunError::task("contained_task_home_recovery_step_limit")
            })?;
        Ok(outcome)
    }

    fn recover_contained_task(
        &self,
        request: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        instance_id: InstanceId,
        task_request: &ContainedTaskRequest,
    ) -> Result<Option<OperationSuccess>, RequestFailure> {
        let active = lock(&self.contained_runs, "read_active_contained_runs")?
            .contains_key(&request.request_id());
        let events = self
            .ledger
            .query(EventQuery {
                request_id: Some(request.request_id()),
                ..EventQuery::default()
            })
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error("recover_contained_task"))
            })?;
        if events.is_empty() {
            return if active {
                Err(contained_task_replay_denied(
                    "contained_task_already_running",
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
                "contained_task_request_identity_reused",
            ));
        }
        let semantic = events
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => Some((event, payload.fact())),
                _ => None,
            })
            .collect::<Vec<_>>();
        if semantic.is_empty() {
            return Err(contained_task_replay_denied(
                "contained_task_previous_attempt_incomplete",
            ));
        }
        let packages = semantic
            .iter()
            .filter_map(|(event, fact)| match fact {
                TaskSemanticFact::PackageAdmitted {
                    package_sha256,
                    response_deadline_monotonic_ms,
                    ..
                } => Some((*event, package_sha256, *response_deadline_monotonic_ms)),
                _ => None,
            })
            .collect::<Vec<_>>();
        if packages.len() != 1 || packages[0].1 != task_request.expected_sha256() {
            return Err(contained_task_replay_denied(
                "contained_task_request_package_reused",
            ));
        }
        let recovery_packages = semantic
            .iter()
            .filter_map(|(_, fact)| match fact {
                TaskSemanticFact::EntryRecoveryPackageAdmitted { package_sha256 } => {
                    Some(package_sha256)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if recovery_packages.len() > 1
            || recovery_packages.first().is_some_and(|recorded| {
                task_request
                    .recovery()
                    .map(|binding| binding.expected_sha256())
                    != Some(*recorded)
            })
        {
            return Err(contained_task_replay_denied(
                "contained_task_request_recovery_reused",
            ));
        }
        let package_event = packages[0].0;
        let deadline_monotonic_ms = packages[0].2;
        let task_id = package_event.links().task_id().copied().ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "contained_task_identity_missing",
                "recover_contained_task",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        let run_id = package_event.links().run_id().copied().ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "contained_task_identity_missing",
                "recover_contained_task",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        let lease_id = package_event.links().lease_id().copied().ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "contained_task_identity_missing",
                "recover_contained_task",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        let mut terminals = semantic
            .iter()
            .filter_map(|(event, fact)| match fact {
                TaskSemanticFact::TerminalCommitted {
                    outcome,
                    final_page,
                    executed_steps,
                    failure_code,
                    ..
                } => Some((
                    (*event).clone(),
                    *outcome,
                    final_page.clone(),
                    *executed_steps,
                    failure_code.clone(),
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        if terminals.len() > 1 {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "contained_task_terminal_state_inconsistent",
                    "recover_contained_task",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        if terminals.is_empty() {
            if active {
                return Err(contained_task_replay_denied(
                    "contained_task_already_running",
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
                Some("contained_task_recovered_after_restart".to_owned()),
            ));
        }
        let lease_events = self
            .ledger
            .query(EventQuery {
                lease_id: Some(lease_id),
                ..EventQuery::default()
            })
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error(
                    "query_recovered_contained_task_lease_terminal",
                ))
            })?;
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
                    "contained_task_lease_terminal_state_inconsistent",
                    "recover_contained_task",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        let lease_disposition = match lease_terminals.as_slice() {
            [event] if event.event_type() == EventType::LeaseExpired => {
                ContainedTaskLeaseTerminal::Expired
            }
            [_] => ContainedTaskLeaseTerminal::Released,
            [] if active => {
                return Err(contained_task_replay_denied(
                    "contained_task_already_running",
                ));
            }
            [] => {
                self.append_recovered_contained_task_release(
                    validated,
                    instance_id,
                    lease_id,
                    task_id,
                    run_id,
                )?;
                ContainedTaskLeaseTerminal::Released
            }
            _ => unreachable!("lease terminal cardinality checked above"),
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
                    outcome: *outcome,
                    final_page: final_page.clone(),
                    executed_steps: executed_steps.ok_or_else(|| {
                        RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                            "contained_task_terminal_state_inconsistent",
                            "recover_contained_task",
                            RuntimeErrorCode::RuntimeFatal,
                        ))
                    })?,
                },
            })),
            TaskOutcome::Cancelled => Ok(Some(OperationSuccess {
                state: RuntimeReceiptState::Cancelled,
                terminal: Some(terminal(terminal_event)),
                result: RuntimeResult::ContainedTaskCancelled {
                    run_id,
                    task_id,
                    task_request_id: request.request_id(),
                    response_deadline_monotonic_ms: deadline_monotonic_ms,
                    reason: match failure_code.as_deref() {
                        Some("contained_task_deadline_exceeded") => {
                            ContainedTaskCancellationReason::DeadlineExceeded
                        }
                        Some("contained_task_cancelled") => {
                            ContainedTaskCancellationReason::ClientRequested
                        }
                        _ => ContainedTaskCancellationReason::RecoveredAfterRestart,
                    },
                    lease_terminal: lease_disposition,
                },
            })),
            TaskOutcome::Failure => Err(RequestFailure::request(
                RuntimeHostError::request(
                    "contained_task_recovered_terminal_failure",
                    "recover_contained_task",
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
        lease_id: LeaseId,
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
        let gate = lock(&self.fact_write_gate, "recover_contained_task_terminal")
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
                        outcome: TaskOutcome::Cancelled,
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
                        outcome: TaskOutcome::Cancelled,
                        final_page: None,
                        executed_steps: None,
                        failure_code: Some("contained_task_recovered_after_restart".to_owned()),
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
        if lock(&self.scheduler, "verify_recovered_contained_task_lease")?
            .active_tokens()
            .iter()
            .any(|token| token.instance_id() == instance_id && token.lease_id() == lease_id)
        {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    "contained_task_recovery_lease_busy",
                    "recover_contained_task",
                    RuntimeErrorCode::ContainedTaskBusy,
                ),
                RuntimeReceiptState::Denied,
                None,
            ));
        }
        let resolved = lock(&self.registered_instances, "read_instance_registry")?
            .get(&instance_id)
            .cloned()
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "contained_task_recovery_instance_missing",
                    "recover_contained_task",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })?;
        let links = request.contained_task_recovery_event_links(
            instance_id,
            lease_id,
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

    fn begin_contained_run(
        &self,
        request_id: RequestId,
        instance_id: InstanceId,
        client_cancellable: bool,
    ) -> Result<ActiveContainedRun<'_>, RequestFailure> {
        let mut active = lock(&self.contained_runs, "begin_contained_run")?;
        if active.contains_key(&request_id) {
            return Err(contained_task_replay_denied(
                "contained_task_already_running",
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

    pub(super) fn append_contained_task_terminal(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        draft: ContainedTaskTerminalDraft,
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
            let connection_id = lock(&self.scheduler, "read_task_lease_connection")?
                .connection_for_token(token)
                .map_err(|error| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                        "read_task_lease_connection",
                        &error,
                    ))
                })?;
            self.close_instance_resources(token, connection_id, links.clone())?;
            let gate = lock(&self.fact_write_gate, "append_contained_task_terminal")
                .map_err(RequestFailure::poison_without_terminal)?;
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
                        "check_contained_task_terminal",
                    ))
                })?;
            let terminals = chain_events
                .iter()
                .filter_map(|event| match event.payload() {
                    EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                        TaskSemanticFact::TerminalCommitted { outcome, .. } => Some(*outcome),
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
                                reason: "terminal_already_committed".to_string(),
                            },
                            AuditInput::new(),
                        ),
                    )
                    .map_err(RequestFailure::poison_without_terminal)?;
                self.synchronize_fact_store_under_gate()
                    .map_err(RequestFailure::poison_without_terminal)?;
                drop(gate);
                self.observe_pipeline_event(&rejected)
                    .map_err(RequestFailure::poison_without_terminal)?;
                return Err(RequestFailure::request(
                    RuntimeHostError::request(
                        "contained_task_terminal_already_committed",
                        "append_contained_task_terminal",
                        RuntimeErrorCode::InvalidRequest,
                    ),
                    RuntimeReceiptState::Denied,
                    Some(terminal(&rejected)),
                ));
            }
            if terminals.len() > 1 {
                return Err(RequestFailure::poison_without_terminal(
                    RuntimeHostError::fatal(
                        "contained_task_terminal_state_inconsistent",
                        "append_contained_task_terminal",
                        RuntimeErrorCode::RuntimeFatal,
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
                        "capture_summary_state_inconsistent",
                        "append_contained_task_terminal",
                        RuntimeErrorCode::RuntimeFatal,
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
                                "capture_summary_state_conflict",
                                "append_contained_task_terminal",
                                RuntimeErrorCode::RuntimeFatal,
                            ),
                        ));
                    }
                    let EventPayload::Capture(CapturePayload::SummaryCommitted(payload)) =
                        event.payload()
                    else {
                        return Err(RequestFailure::poison_without_terminal(
                            RuntimeHostError::fatal(
                                "capture_summary_state_inconsistent",
                                "append_contained_task_terminal",
                                RuntimeErrorCode::RuntimeFatal,
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
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        error.code(),
                        "append_contained_task_terminal",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?;
            if let (Some(existing), Some(requested)) =
                (existing_summary, requested_summary.as_ref())
                && existing != requested
            {
                return Err(RequestFailure::poison_without_terminal(
                    RuntimeHostError::fatal(
                        "capture_summary_state_conflict",
                        "append_contained_task_terminal",
                        RuntimeErrorCode::RuntimeFatal,
                    ),
                ));
            }
            let scheduling_disposition = select_scheduling_disposition(
                &chain_events,
                draft.outcome,
                draft.final_page.as_deref(),
                draft.executed_steps,
                draft.scheduling_outcome.as_ref(),
                draft.selected_scheduling_outcome.as_deref(),
            )?;
            let mut appended = Vec::with_capacity(3);
            if existing_summary.is_none()
                && let Some(summary) = requested_summary
            {
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
                                outcome: draft.outcome,
                            },
                            AuditInput::new(),
                        ),
                    )
                    .map_err(RequestFailure::poison_without_terminal)?,
                );
            }
            let severity = match (draft.outcome, draft.failure_severity) {
                (TaskOutcome::Success, None) => EventSeverity::Info,
                (TaskOutcome::Failure, None) => EventSeverity::Error,
                (
                    TaskOutcome::Failure,
                    Some(severity @ (EventSeverity::Warning | EventSeverity::Fatal)),
                ) => severity,
                (TaskOutcome::Cancelled, None) => EventSeverity::Warning,
                _ => {
                    return Err(RequestFailure::poison_without_terminal(
                        RuntimeHostError::fatal(
                            "contained_task_terminal_severity_invalid",
                            "append_contained_task_terminal",
                            RuntimeErrorCode::RuntimeFatal,
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
                        "scheduling_terminal_append_injected_failure",
                        "append_contained_task_terminal",
                        RuntimeErrorCode::RuntimeFatal,
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
                            outcome: draft.outcome,
                            final_page: draft.final_page,
                            executed_steps: draft.executed_steps,
                            failure_code: draft.failure_code.map(str::to_string),
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
            Ok(terminal_event)
        })();
        result.map_err(|mut failure: RequestFailure| {
            if failure.error.lifecycle.task_timing.is_none() {
                failure.error.lifecycle.task_timing = task_timing;
            }
            failure
        })
    }
}
