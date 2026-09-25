// SPDX-License-Identifier: AGPL-3.0-only

use crate::agent_dispatcher::{
    AgentDispatcherState, AgentResponsePreparation, AgentResumePreparation, AgentSessionPreparation,
};
use crate::approval::ApprovalProjection;
use crate::events::RuntimeEvents;
use crate::fact_store::InstanceFactStore;
use crate::ipc::DEFAULT_RUNTIME_MAX_FRAME_BYTES;
use crate::monitor::{DueMonitorProbe, MonitorRegistry, MonitorUpdate};
use crate::owner::{OwnerGuard, OwnerStartup};
use crate::performance::{
    PerformanceMonitor, PerformanceSemanticEvent, PerformanceTick, PipelineEventObservation,
};
use crate::performance_control::{PerformanceBalanceController, PerformanceDispatchGate};
use crate::policy_host::{
    CompletedPolicyRunIdentity, LoadedCatalog, PolicyEvaluationContext, PolicyExecutionPreparation,
    PolicyHost, PolicyOutcomeKeySnapshot,
};
use crate::proposal::prepare_proposal;
use crate::strategy::{StrategicPlanPreparation, build_strategy_proposal};
use crate::time::{RuntimeClock, RuntimeClockSample, SystemRuntimeClock, unix_ms_now};
use crate::{
    AgentDispatcherConfig, CatalogGeneration, FatalState, MaintenanceLedgerQuery,
    PerformanceControlConfig, PerformanceControlDirective, PerformanceMonitorConfig,
    PipelinePerformanceSignal, PolicyAdmissionContext, PolicyCadence, PolicyCycle,
    PolicyDispatchAdmission, PolicyExecutionInput, PolicyRunContext, PolicyTrigger,
    ProcedureManifest, RuntimeHostError, RuntimeHostResult,
};
use actingcommand_artifact_store::{
    ArtifactEventSink, ArtifactStore, ArtifactStoreError, ArtifactStoreResult,
    ArtifactWriteContext, ArtifactWriteRequest, CapturePipeline, CapturePipelineConfig,
    CapturePipelineCounts, CapturePipelineSummary, EvidenceExportDocuments, EvidenceExportIdentity,
    EvidenceExportRequest, EvidenceExporter, EvidenceJsonDocument, EvidencePackage,
    FrameStoreFrameInput, PackageVerification, PersistedFrameEvidence, PinnedFrameEvidence,
    PreparedArtifact, RecognitionState, build_capture_pipeline_summary, capture_summary_record,
    read_projected_verified,
};
use actingcommand_contract::{
    ActionId, AgentPayloadDraft, AgentSessionContext, AgentSessionId, AgentSessionResponse,
    AgentSessionStatus, AgentWakeId, AgentWakeKind, AgentWakeTrigger, ApplicationLifecycleAction,
    ApplicationPayload, ApplicationPayloadDraft, ApprovalDecisionRecord, ApprovalPayload,
    ApprovalPayloadDraft, ArtifactIssuePolicy, ArtifactKind, ArtifactLinksDraft, ArtifactProducer,
    ArtifactRedactionState, ArtifactReference, AuditInput, AuthoritativeSchedulingOutcome,
    CapturePayload, CapturePayloadDraft, CaptureSequence, CaptureSequenceSpec, CatalogPayloadDraft,
    CatalogPromotionAuthorization, CatalogProposal, CatalogTransitionEventData, ClientActionRecord,
    ClientPayload, ClientPayloadDraft, CommandPayloadDraft, ContainedTaskCancellationReason,
    ContainedTaskCancellationStatus, ContainedTaskLeaseTerminal, ContainedTaskRequest,
    CorrelationId, DiagnosticCode, DiagnosticDetailDraft, EFFECTIVE_CONFIGURATION_SCHEMA,
    EffectDisposition, EffectiveCaptureSelection, EffectiveConfigurationFacts,
    EffectiveConfigurationRecord, EffectiveInputSelection, EffectiveMumuInstallation, EventAction,
    EventActor, EventDraft, EventId, EventLinksDraft, EventPayload, EventQuery, EventSeverity,
    EventSource, EventType, FactPayloadDraft, FactRecord, FencedWrite, FrameId, InputAction,
    InputExecutionPlanEvent, InputExecutionPlanRecord, InputPayload, InputPayloadDraft,
    InstanceBindingSource, InstanceFactContext, InstanceFactSnapshot, InstanceId, IssuedActionId,
    IssuedFrameId, IssuedMonitorProbe, IssuedReadOnlyCaptureCapability, IssuedRecognitionId,
    IssuedRunId, IssuedTaskId, LeaseId, LeasePayloadDraft, LeaseQueuePolicy, LeaseToken,
    MAX_EFFECTIVE_CONFIGURATION_BYTES, MAX_GOVERNANCE_CAPABILITY_BYTES, MAX_RUNTIME_FACTS,
    MIN_GOVERNANCE_CAPABILITY_BYTES, MonitorPayloadDraft, MonitorRecoveryCoordinationReason,
    ObservedMicroseconds, OriginModule, OwnerResourceDisposition, PackageDebugLayout,
    PackageDebugRequest, PackageDebugSummary, PerformanceContext, PerformancePayloadDraft,
    PinnedFrameReason, PolicyDispatchEventData, PolicyExecutionEventData, PolicyExecutionOutcome,
    PolicyFailureClass, PolicyPayload, PolicyPayloadDraft, PolicyPlanningSignalEventData,
    PolicyReasonRecord, ProjectDecisionPageRequest, ProjectInterfaceRequest,
    ProjectedArtifactReference, ProjectionPayload, ProposalClass, ProposalPromotion,
    RUNTIME_FACT_SNAPSHOT_INTERVAL_MS, RUNTIME_INFO_FILE, ReadonlyObservation,
    RecognitionPayloadDraft, RecognitionVerdict, ReleasePayload, ReleasePayloadDraft,
    ReleaseTransitionKind, RequestId, ResourceAuthoringEvent, ResourceAuthoringPayloadDraft,
    ResourceAuthoringPhase, ResourceQuiescence, RetentionClass, RunId, RuntimeCaptureBackend,
    RuntimeConfigManifest, RuntimeContractError, RuntimeControlPlaneStatus, RuntimeDebugEvent,
    RuntimeDebugOperation, RuntimeDebugPhase, RuntimeErrorCode, RuntimeErrorProjection,
    RuntimeEventBatch, RuntimeEventQueryPageRequest, RuntimeEvidenceExportRequest,
    RuntimeEvidenceExportSummary, RuntimeEvidenceScreenshotCounts, RuntimeFactInvalidation,
    RuntimeFactInvalidationReason, RuntimeFactRecord, RuntimeFactScope, RuntimeFactSnapshot,
    RuntimeForwardProjectionRequest, RuntimeInfo, RuntimeInstanceStatus, RuntimeLifecyclePhase,
    RuntimeMaintenanceQuery, RuntimeMonitorPolicy, RuntimeOperation, RuntimePayload,
    RuntimePayloadDraft, RuntimePlanningDocument, RuntimePlanningDocumentKind,
    RuntimePolicyInputIdentity, RuntimeReceipt, RuntimeReceiptState, RuntimeReleaseSet,
    RuntimeRequest, RuntimeResult, RuntimeStrategicPlanResult, RuntimeSubscriptionRequest,
    SchedulerPayloadDraft, SchedulingDisposition, SchedulingEffectCondition,
    SchedulingEffectEvidence, SchedulingOutcomeDeclaration, SchedulingOutcomeIdentity,
    SchedulingOutcomeProjection, Sensitivity, StartupPackageDisposition, TaskEntryRecognitionPhase,
    TaskEntryTargetDisposition, TaskId, TaskOutcome, TaskPayload, TaskPayloadDraft,
    TaskSemanticFact, TaskTimingBoundary, TaskTimingObservationState, TaskTimingResult,
    TerminalEvent, TimingObservationIssue, ValidatedRuntimeRequest,
};
use actingcommand_device::{CaptureBackendName, DeviceCloseAuthority, Frame, SegmentedSwipeEvent};
use actingcommand_execution_kernel::ExecutionKernelError;
use actingcommand_execution_kernel::{
    ContainedTaskEvaluationTiming, ContainedTaskOutcome, ContainedTaskRunError,
    ContainedTaskRuntime, ContainedTaskRuntimeErrorClass, ContainedTaskTimingContext,
    ContainedTaskTrace, DiscoveredInstanceBinding, ExecutionBackendProvenance,
    ExecutionBackendProvider, ExecutionKernel, ExternalExpectedSha256, PostAdmissionOcrObservation,
    PreparedContainedTask, PreparedInputAction, RecognitionVisionProvider, ResolvedAdbEndpoint,
    ResolvedInstanceEndpoint, StabilityComparisonResult, StabilityTerminalReason,
    StabilityTerminationDeclaration, decide_monitor, page_anchor_matches,
};
use actingcommand_execution_kernel::{InputFrameContext, ObservedFrame};
use actingcommand_ledger::critical::{
    CatalogTransitionTarget, CriticalActionReport, CriticalEventPlan, CriticalExecutionError,
    CriticalOperation, DefiniteEffectDisposition, EventAppender, LeaseTransitionTarget,
    ReleaseTransitionTarget, execute_critical,
};
use actingcommand_ledger::{GlobalLedger, PersistedEvent, project_subscription_event};
use actingcommand_pack_containment::{
    Containment, DEFAULT_MAX_COMPRESSED_BYTES, InstanceId as ContainmentInstanceId, PackageLayout,
    Sha256Hash,
};
use actingcommand_policy::{
    CatalogSources, DecisionReasonChain, DispatchIntent, EvaluationFacts, EvaluationResources,
    EvaluationTime, FactValue as PolicyFactValue, ForwardProjection, ForwardProjectionConfig,
    MaintenanceAssessment, MaintenanceTrendPolicy, ObservedOutcome, StrategicBand,
    StrategicEvidencePointer, StrategicProjection, StrategicReport, project_forward,
    project_strategic_report,
};
use actingcommand_runtime_state::{ReleaseArtifactSources, RuntimeStateStore};
use actingcommand_scheduler::facts::{RuntimeFactChange, RuntimeFactError, RuntimeFactStore};
use actingcommand_scheduler::{
    CancelledQueuedLease, ConnectionId, LeasePreparation, LeaseReleaseReason, LeaseTransferReason,
    PreparedLeaseTransfer, QueueAdmissionDecision, QueueLeaseRequest, QueuePoll, QueuedLease,
    SchedulerConfig, SchedulerError, SeedScheduler, TransferPreparation,
};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Barrier;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const DEFAULT_RUNTIME_IO_TIMEOUT: Duration = Duration::from_secs(5);
const LEASE_SWEEP_INTERVAL: Duration = Duration::from_millis(50);
const ACCEPT_IDLE_INTERVAL: Duration = Duration::from_millis(20);
const MAX_REQUEST_CACHE_ENTRIES: usize = 4096;
const MAX_TRUSTED_POLICY_DISPATCHES: usize = 16_384;
const MAX_AUTHORITATIVE_POLICY_OUTCOMES: usize = 16_384;
const POLICY_CONNECTION_VALUE: u64 = u64::MAX;
const RESOURCE_CLOSE_CONNECTION_VALUE: u64 = u64::MAX - 1;
/// Synthesized connection of the host's own startup-package scheduling thread (#316-B3).
const STARTUP_PACKAGE_CONNECTION_VALUE: u64 = u64::MAX - 2;

mod agent_control;
mod backend_open;
mod client_events;
mod contained_task;
mod device_diagnostic;
mod emulator_instance;
mod evidence_export;
mod facts;
mod foreground_gate;
mod frame_retention;
mod governance;
mod input;
mod instance_discovery;
mod lab_operation;
mod lease;
mod lifecycle;
mod material_read;
mod monitor_control;
mod nemu_input;
mod observation;
mod online_observation;
mod package_debug;
mod performance;
mod planning;
mod policy_catalog;
mod policy_dispatch;
mod policy_outcome;
mod ppocr_diagnostic;
mod read_events;
mod recovery_ladder;
mod requests;
mod resource_close;
mod runtime_facts;
mod saved_artifact_ocr;
use material_read::MaterialReadContext;
mod signatures;
mod startup_package;
mod state_control;
mod task_diagnostic;
mod task_timing;

use agent_control::reconcile_agent_wakes;
use contained_task::{
    ContainedRunControl, RuntimeArtifactEventSink, RuntimeContainedTask, task_outcome_event_type,
    task_outcome_severity,
};
#[cfg(test)]
pub(crate) use contained_task::{
    ContainedTaskCheckpointIdentity, ContainedTaskCheckpointTestControl,
    require_contained_task_sampling_run_seed,
};
#[cfg(test)]
use contained_task::{ContainedTaskCheckpointTestHook, ContainedTaskTerminalDraft};
use input::RuntimeInputContext;
#[cfg(test)]
use lease::{LeaseExpiryTestCheckpoint, lease_token_identity_match_count};
use lease::{
    QueueTerminalStore, QueuedRequestContext, RuntimeLeaseAcquisition, instance_not_running,
};
use lifecycle::{
    append_instance_binding_events, append_runtime_start_event, instance_bound_payload,
    record_failure,
};
use monitor_control::monitor_probe_loop;
use observation::CompletedReadonlyObservation;
use performance::{CapacityUse, performance_monitor_loop};
use planning::planning_request_failure;
use policy_dispatch::TrustedPolicyDispatchStore;
#[cfg(test)]
pub(crate) use policy_outcome::insert_authoritative_policy_outcome;
#[cfg(test)]
use policy_outcome::{PolicyOutcomeCacheUpdate, validate_policy_run_admission_request};
use policy_outcome::{
    completed_run_matches_outcome, reconcile_policy_dispatches,
    recover_authoritative_policy_outcomes, validate_completed_run_admission_request,
};
use recovery_ladder::{RecoveryLadderAdmission, with_recovery_ladder_staging};
use requests::{
    ActionFailure, ConnectionFailureContext, ConnectionFailureStage, RequestFailure,
    TaskFailureEvidence, client_fact_conflict, connection_boundary, critical_execution_error,
    critical_plan_error, diagnostic_for_projection, policy_admission_fatal,
    policy_admission_request, policy_contract_error, policy_id_error, protocol_error,
    receipt_error, terminal,
};
use state_control::reconcile_runtime_state;

#[derive(Clone, Copy)]
pub enum RuntimeLifecycleFailureStage {
    PolicyInitialization,
    PolicyMonitor,
    PolicyForward,
    StrategicReport,
    SessionClose,
    OperationCleanup,
    ConnectionCleanup,
    ShutdownJoin,
    InfoFileRemoval,
    RetainedReference,
    HostClose,
    PolicyDriver,
    PolicyControl,
    PolicyBootstrap,
}

pub enum RuntimeLifecycleFailure<'a> {
    Host(&'a RuntimeHostError),
    PolicyAdmission {
        error: &'a RuntimeHostError,
        decision_id: &'a str,
    },
    Client {
        code: &'static str,
        operation: &'static str,
        fatal: bool,
        runtime_code: Option<RuntimeErrorCode>,
        message: &'a str,
    },
    Process {
        code: &'static str,
    },
}

/// Runtime-owned policy inputs supplied by trusted host integrations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyInputSnapshot {
    facts: EvaluationFacts,
    resources: EvaluationResources,
}

impl PolicyInputSnapshot {
    pub fn new(facts: EvaluationFacts, resources: EvaluationResources) -> Self {
        Self { facts, resources }
    }

    pub fn facts(&self) -> &EvaluationFacts {
        &self.facts
    }

    pub fn resources(&self) -> &EvaluationResources {
        &self.resources
    }
}

#[cfg(test)]
fn policy_crash_test_barrier(point: &str) {
    if std::env::var("ACTINGCOMMAND_POLICY_CRASH_POINT").as_deref() != Ok(point) {
        return;
    }
    let marker =
        std::env::var_os("ACTINGCOMMAND_POLICY_CRASH_MARKER").expect("policy crash marker path");
    fs::write(marker, point.as_bytes()).expect("policy crash marker");
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

#[cfg(test)]
fn fail_policy_execution_append_for_test() -> RuntimeHostResult<()> {
    if std::env::var("ACTINGCOMMAND_POLICY_CRASH_POINT").as_deref()
        != Ok("fail_policy_execution_append")
    {
        return Ok(());
    }
    let marker =
        std::env::var_os("ACTINGCOMMAND_POLICY_CRASH_MARKER").expect("policy crash marker path");
    fs::write(marker, b"fail_policy_execution_append").expect("policy append failure marker");
    Err(ledger_error("append_policy_execution"))
}

#[derive(Clone)]
pub struct RuntimeHostConfig {
    state_root: PathBuf,
    device_diagnostic_mode: actingcommand_contract::DeviceDiagnosticMode,
    bind_address: SocketAddr,
    scheduler: SchedulerConfig,
    policy_cadence: PolicyCadence,
    maximum_frame_bytes: usize,
    io_timeout: Duration,
    performance_monitor: Option<PerformanceMonitorConfig>,
    capacity_thresholds: actingcommand_contract::CapacityThresholds,
    frame_retention_enabled: bool,
    failed_run_retention: actingcommand_contract::FailedRunRetentionPolicy,
    performance_control: PerformanceControlConfig,
    agent_dispatcher: Option<AgentDispatcherConfig>,
    secret_fingerprint_salt: Vec<u8>,
    governance_capability_sha256: Option<[u8; 32]>,
    governance_capability_invalid: bool,
    clock: Arc<dyn RuntimeClock>,
    policy_inputs: Option<PolicyInputSnapshot>,
    procedure_manifest: Option<ProcedureManifest>,
    config_manifest: Option<RuntimeConfigManifest>,
    /// Per instance alias: the contained task the host schedules by itself after a successful
    /// emulator `start` / `restart` (slice #316-B3). Same locator + digest semantics as
    /// `actingctl task-run --package / --expected-sha256`.
    startup_packages: BTreeMap<String, ContainedTaskRequest>,
    /// Per instance alias: the default resource package the daemon admitted from its
    /// configuration (slice #324-r1). Reported by instance status only; never opened here.
    resource_packages: BTreeMap<String, actingcommand_contract::InstanceResourcePackage>,
    /// Per instance alias: the stuck-recovery ladder settings (slice #316-B4); an instance
    /// without an entry uses the defaults (enabled, 600 s cool-down).
    stuck_recovery: BTreeMap<String, actingcommand_contract::InstanceStuckRecovery>,
}

impl RuntimeHostConfig {
    pub fn new(state_root: impl Into<PathBuf>, secret_fingerprint_salt: impl AsRef<[u8]>) -> Self {
        Self {
            state_root: state_root.into(),
            device_diagnostic_mode: actingcommand_contract::DeviceDiagnosticMode::default(),
            bind_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            scheduler: SchedulerConfig::default(),
            policy_cadence: PolicyCadence::default(),
            maximum_frame_bytes: DEFAULT_RUNTIME_MAX_FRAME_BYTES,
            io_timeout: DEFAULT_RUNTIME_IO_TIMEOUT,
            performance_monitor: None,
            capacity_thresholds: actingcommand_contract::CapacityThresholds::default(),
            frame_retention_enabled: true,
            failed_run_retention: actingcommand_contract::FailedRunRetentionPolicy::default(),
            performance_control: PerformanceControlConfig::default(),
            agent_dispatcher: None,
            secret_fingerprint_salt: secret_fingerprint_salt.as_ref().to_vec(),
            governance_capability_sha256: None,
            governance_capability_invalid: false,
            clock: Arc::new(SystemRuntimeClock::new()),
            policy_inputs: None,
            procedure_manifest: None,
            config_manifest: None,
            startup_packages: BTreeMap::new(),
            resource_packages: BTreeMap::new(),
            stuck_recovery: BTreeMap::new(),
        }
    }

    pub fn with_bind_address(mut self, bind_address: SocketAddr) -> Self {
        self.bind_address = bind_address;
        self
    }

    pub fn with_device_diagnostic_mode(
        mut self,
        mode: actingcommand_contract::DeviceDiagnosticMode,
    ) -> Self {
        self.device_diagnostic_mode = mode;
        self
    }

    pub fn with_scheduler(mut self, scheduler: SchedulerConfig) -> Self {
        self.scheduler = scheduler;
        self
    }

    pub fn with_policy_cadence(mut self, policy_cadence: PolicyCadence) -> Self {
        self.policy_cadence = policy_cadence;
        self
    }

    pub fn with_io_timeout(mut self, io_timeout: Duration) -> Self {
        self.io_timeout = io_timeout;
        self
    }

    pub fn with_maximum_frame_bytes(mut self, maximum_frame_bytes: usize) -> Self {
        self.maximum_frame_bytes = maximum_frame_bytes;
        self
    }

    pub fn with_performance_monitor(
        mut self,
        performance_monitor: PerformanceMonitorConfig,
    ) -> Self {
        self.performance_monitor = Some(performance_monitor);
        self
    }

    pub fn with_performance_control(
        mut self,
        performance_control: PerformanceControlConfig,
    ) -> Self {
        self.performance_control = performance_control;
        self
    }

    pub fn with_capacity_thresholds(
        mut self,
        thresholds: actingcommand_contract::CapacityThresholds,
    ) -> Self {
        self.capacity_thresholds = thresholds;
        self
    }

    pub fn with_frame_retention_enabled(mut self, enabled: bool) -> Self {
        self.frame_retention_enabled = enabled;
        self
    }

    pub fn with_failed_run_retention(
        mut self,
        policy: actingcommand_contract::FailedRunRetentionPolicy,
    ) -> Self {
        self.failed_run_retention = policy;
        self
    }

    pub fn with_agent_dispatcher(mut self, agent_dispatcher: AgentDispatcherConfig) -> Self {
        self.agent_dispatcher = Some(agent_dispatcher);
        self
    }

    pub fn with_governance_capability(mut self, capability: impl AsRef<[u8]>) -> Self {
        let capability = capability.as_ref();
        self.governance_capability_invalid = !(MIN_GOVERNANCE_CAPABILITY_BYTES
            ..=MAX_GOVERNANCE_CAPABILITY_BYTES)
            .contains(&capability.len())
            || capability.iter().any(u8::is_ascii_control);
        self.governance_capability_sha256 =
            (!self.governance_capability_invalid).then(|| Sha256::digest(capability).into());
        self
    }

    /// Overrides the Runtime-owned clock, primarily for deterministic boundary tests.
    pub fn with_runtime_clock(mut self, clock: Arc<dyn RuntimeClock>) -> Self {
        self.clock = clock;
        self
    }

    /// Installs the trusted policy snapshot used by Runtime-owned evaluation.
    pub fn with_policy_inputs(mut self, policy_inputs: PolicyInputSnapshot) -> Self {
        self.policy_inputs = Some(policy_inputs);
        self
    }

    /// Installs the Runtime-owned manifest that binds procedure aliases to package content.
    pub fn with_procedure_manifest(mut self, procedure_manifest: ProcedureManifest) -> Self {
        self.procedure_manifest = Some(procedure_manifest);
        self
    }

    /// Installs the in-memory runtime configuration manifest the host records as the two
    /// `config.*` runtime facts once per startup. A host without one records nothing.
    pub fn with_config_manifest(mut self, config_manifest: RuntimeConfigManifest) -> Self {
        self.config_manifest = Some(config_manifest);
        self
    }

    /// Installs the startup packages, keyed by instance alias (slice #316-B3). An alias that
    /// is not a registered physical instance fails startup with
    /// `startup_package_instance_unknown`; the package itself is admitted (hash-checked) only
    /// when it runs, exactly like `task-run`.
    pub fn with_startup_packages(
        mut self,
        startup_packages: BTreeMap<String, ContainedTaskRequest>,
    ) -> Self {
        self.startup_packages = startup_packages;
        self
    }

    /// The configured startup packages, keyed by instance alias.
    pub const fn startup_packages(&self) -> &BTreeMap<String, ContainedTaskRequest> {
        &self.startup_packages
    }

    /// Installs the admitted default resource packages, keyed by instance alias (slice
    /// #324-r1); instance status reports them.
    pub fn with_resource_packages(
        mut self,
        resource_packages: BTreeMap<String, actingcommand_contract::InstanceResourcePackage>,
    ) -> Self {
        self.resource_packages = resource_packages;
        self
    }

    /// Installs the stuck-recovery ladder settings, keyed by instance alias (slice #316-B4).
    /// An alias that is not a registered instance fails startup with
    /// `stuck_recovery_instance_unknown`.
    pub fn with_stuck_recovery(
        mut self,
        stuck_recovery: BTreeMap<String, actingcommand_contract::InstanceStuckRecovery>,
    ) -> Self {
        self.stuck_recovery = stuck_recovery;
        self
    }

    /// The configured stuck-recovery ladder settings, keyed by instance alias.
    pub const fn stuck_recovery(
        &self,
    ) -> &BTreeMap<String, actingcommand_contract::InstanceStuckRecovery> {
        &self.stuck_recovery
    }

    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    pub const fn bind_address(&self) -> SocketAddr {
        self.bind_address
    }

    pub const fn io_timeout(&self) -> Duration {
        self.io_timeout
    }

    pub const fn maximum_frame_bytes(&self) -> usize {
        self.maximum_frame_bytes
    }

    pub fn validate(&self) -> RuntimeHostResult<()> {
        self.scheduler
            .validate()
            .map_err(|error| RuntimeHostError::scheduler("validate_runtime_config", &error))?;
        self.policy_cadence.validate()?;
        self.failed_run_retention.validate().map_err(|_| {
            RuntimeHostError::fatal(
                "invalid_failed_run_retention_policy",
                "validate_runtime_config",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        self.capacity_thresholds.validate().map_err(|_| {
            RuntimeHostError::fatal(
                "invalid_capacity_thresholds",
                "validate_runtime_config",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        if let Some(performance_monitor) = &self.performance_monitor {
            performance_monitor.validate()?;
        }
        self.performance_control.validate()?;
        if let Some(config_manifest) = &self.config_manifest {
            config_manifest.validate().map_err(|error| {
                RuntimeHostError::fatal(
                    "invalid_runtime_config_manifest",
                    "validate_runtime_config",
                    RuntimeErrorCode::RuntimeFatal,
                )
                .with_native_detail(error.code().to_owned())
            })?;
        }
        for (alias, request) in &self.startup_packages {
            if actingcommand_contract::validate_instance_alias(alias).is_err()
                || request.validate().is_err()
            {
                return Err(RuntimeHostError::fatal(
                    "invalid_startup_package",
                    "validate_runtime_config",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
        }
        for (alias, settings) in &self.stuck_recovery {
            if actingcommand_contract::validate_instance_alias(alias).is_err()
                || settings.validate().is_err()
            {
                return Err(RuntimeHostError::fatal(
                    "invalid_stuck_recovery",
                    "validate_runtime_config",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
        }
        if self.state_root.as_os_str().is_empty()
            || !self.bind_address.ip().is_loopback()
            || self.io_timeout.is_zero()
            || self.maximum_frame_bytes == 0
            || self.maximum_frame_bytes > DEFAULT_RUNTIME_MAX_FRAME_BYTES
            || self.secret_fingerprint_salt.is_empty()
            || self.governance_capability_invalid
        {
            return Err(RuntimeHostError::fatal(
                "invalid_runtime_host_config",
                "validate_runtime_config",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        Ok(())
    }
}

impl std::fmt::Debug for RuntimeHostConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeHostConfig")
            .field("state_root", &"<redacted>")
            .field("bind_address", &self.bind_address)
            .field("device_diagnostic_mode", &self.device_diagnostic_mode)
            .field("scheduler", &self.scheduler)
            .field("policy_cadence", &self.policy_cadence)
            .field("maximum_frame_bytes", &self.maximum_frame_bytes)
            .field("io_timeout", &self.io_timeout)
            .field("performance_monitor", &self.performance_monitor)
            .field("capacity_thresholds", &self.capacity_thresholds)
            .field("frame_retention_enabled", &self.frame_retention_enabled)
            .field("failed_run_retention", &self.failed_run_retention)
            .field("performance_control", &self.performance_control)
            .field("agent_dispatcher", &self.agent_dispatcher)
            .field("secret_fingerprint_salt", &"<redacted>")
            .field(
                "governance_capability_sha256",
                &self
                    .governance_capability_sha256
                    .map(|_| "<redacted-digest>"),
            )
            .field("clock", &"<runtime-owned>")
            .field(
                "policy_inputs",
                &self.policy_inputs.as_ref().map(|_| "<runtime-owned>"),
            )
            .field(
                "procedure_manifest",
                &self.procedure_manifest.as_ref().map(|_| "<runtime-owned>"),
            )
            .field("config_manifest", &self.config_manifest)
            .field(
                "startup_packages",
                &self.startup_packages.keys().collect::<Vec<_>>(),
            )
            .field(
                "resource_packages",
                &self.resource_packages.keys().collect::<Vec<_>>(),
            )
            .field("stuck_recovery", &self.stuck_recovery)
            .finish()
    }
}

/// Handle for a resident Runtime process that remains alive independently of UI clients.
pub struct RuntimeHost {
    info: RuntimeInfo,
    info_path: PathBuf,
    shared: Option<Arc<HostShared>>,
    accept_thread: Option<JoinHandle<RuntimeHostResult<()>>>,
    sweep_thread: Option<JoinHandle<RuntimeHostResult<()>>>,
    monitor_thread: Option<JoinHandle<RuntimeHostResult<()>>>,
    startup_thread: Option<JoinHandle<RuntimeHostResult<()>>>,
    performance_thread: Option<JoinHandle<RuntimeHostResult<()>>>,
}

impl RuntimeHost {
    pub fn maintain_ledger(
        config: RuntimeHostConfig,
        request: crate::LedgerMaintenanceRequest,
    ) -> Result<crate::LedgerMaintenanceReceipt, crate::LedgerMaintenanceFailure> {
        crate::ledger_maintenance::run(
            &config.state_root,
            &config.secret_fingerprint_salt,
            config.clock,
            request,
        )
    }

    /// Offline `actingd unlock-owner`; see `contracts/actingd-unlock-owner.md`.
    pub fn unlock_owner(
        config: RuntimeHostConfig,
        actor: actingcommand_contract::OwnerUnlockActor,
        confirmed: bool,
    ) -> Result<crate::OwnerUnlockReceipt, crate::OwnerUnlockFailure> {
        crate::owner_unlock::run(
            &config.state_root,
            &config.secret_fingerprint_salt,
            config.clock,
            actor,
            confirmed,
        )
    }

    pub fn start(
        config: RuntimeHostConfig,
        provider: Arc<dyn ExecutionBackendProvider>,
    ) -> RuntimeHostResult<Self> {
        Self::start_with_provider(config, |_| Ok(provider))
    }

    pub fn start_with_provider(
        config: RuntimeHostConfig,
        assemble: impl FnOnce(
            &mut crate::ProviderStartup<'_>,
        ) -> RuntimeHostResult<Arc<dyn ExecutionBackendProvider>>,
    ) -> RuntimeHostResult<Self> {
        config.validate()?;
        fs::create_dir_all(&config.state_root).map_err(|_| {
            RuntimeHostError::fatal(
                "state_root_create_failed",
                "start_runtime_host",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        let events =
            RuntimeEvents::new(&config.secret_fingerprint_salt, Arc::clone(&config.clock))?;
        let clock_origin = config.clock.sample()?;
        let started_at_unix_ms = clock_origin.unix_ms;
        let OwnerStartup {
            guard: mut owner,
            owner_epoch,
            takeover_instances,
            takeover,
            journal,
        } = OwnerGuard::acquire(&config.state_root, events.issuer(), started_at_unix_ms)?;
        let mut fresh_storage = true;
        for material in [
            "runtime-state.sqlite",
            "runtime-state.key",
            "ledger",
            "release-blobs",
            "artifacts",
        ] {
            if config.state_root.join(material).try_exists().map_err(|_| {
                RuntimeHostError::fatal(
                    "state_root_inspect_failed",
                    "select_runtime_storage",
                    RuntimeErrorCode::LedgerFailure,
                )
            })? {
                fresh_storage = false;
            }
        }
        let scheduler = SeedScheduler::new(owner_epoch, config.scheduler, takeover_instances, 0)
            .map_err(|error| RuntimeHostError::scheduler("start_runtime_host", &error))?;
        let ledger_owner = format!("actingd-{}-{started_at_unix_ms}", std::process::id());
        let database = Arc::new(if fresh_storage {
            RuntimeStateStore::open_database(&config.state_root, &config.secret_fingerprint_salt)
                .map_err(|error| RuntimeHostError::state(&error))?
        } else {
            actingcommand_runtime_database::RuntimeDatabase::open_existing(
                &config.state_root,
                false,
            )
            .map_err(|error| {
                RuntimeHostError::fatal(
                    error.code(),
                    error.operation(),
                    RuntimeErrorCode::LedgerFailure,
                )
                .with_native_detail(format!("{error:?}"))
            })?
        });
        let state = Arc::new(
            RuntimeStateStore::from_database(Arc::clone(&database))
                .map_err(|error| RuntimeHostError::state(&error))?,
        );
        let artifacts =
            Arc::new(ArtifactStore::open(&config.state_root).map_err(RuntimeHostError::artifact)?);
        let limits = actingcommand_runtime_database::MaintenanceLimits::default();
        let maintenance = actingcommand_ledger::LedgerMaintenance::acquire(
            &config.state_root,
            fresh_storage,
            limits,
            limits.deadline().map_err(|error| {
                RuntimeHostError::fatal(
                    error.code(),
                    error.operation(),
                    RuntimeErrorCode::LedgerFailure,
                )
                .with_native_detail(format!("{error:?}"))
            })?,
        )
        .map_err(|error| {
            RuntimeHostError::fatal(
                error.code(),
                error.operation(),
                RuntimeErrorCode::LedgerFailure,
            )
            .with_native_detail(format!("{error:?}"))
        })?;
        if fresh_storage {
            maintenance.initialize_empty(&database).map_err(|error| {
                RuntimeHostError::fatal(
                    error.code(),
                    error.operation(),
                    RuntimeErrorCode::LedgerFailure,
                )
                .with_native_detail(format!("{error:?}"))
            })?;
        }
        match maintenance
            .status(&database, |reference| {
                artifacts.verify_recovery_reference(reference).ok()
            })
            .map_err(|error| {
                RuntimeHostError::fatal(
                    error.code(),
                    error.operation(),
                    RuntimeErrorCode::LedgerFailure,
                )
                .with_native_detail(format!("{error:?}"))
            })? {
            actingcommand_ledger::LedgerStorageStatus::Ready { .. } => {}
            _ => {
                return Err(RuntimeHostError::fatal(
                    "ledger_migration_required",
                    "select_runtime_storage",
                    RuntimeErrorCode::LedgerFailure,
                ));
            }
        }
        let ledger = maintenance
            .open_writer(Arc::clone(&database), ledger_owner, |reference| {
                artifacts.verify_recovery_reference(reference).ok()
            })
            .map_err(|error| {
                RuntimeHostError::fatal(
                    error.code(),
                    error.operation(),
                    RuntimeErrorCode::LedgerFailure,
                )
                .with_native_detail(format!("{error:?}"))
            })?;
        let recovery = limits
            .deadline()
            .map_err(|error| {
                RuntimeHostError::fatal(
                    error.code(),
                    error.operation(),
                    RuntimeErrorCode::LedgerFailure,
                )
                .with_native_detail(format!("{error:?}"))
            })
            .and_then(|deadline| {
                frame_retention::FrameRetention::recover(&ledger, &artifacts, deadline)
            });
        if let Err(mut original) = recovery {
            let ledger_closed = ledger.close().map_err(|error| {
                RuntimeHostError::fatal(
                    error.code(),
                    error.operation(),
                    RuntimeErrorCode::LedgerFailure,
                )
                .with_native_detail(format!("{error:?}"))
            });
            let owner_closed = config
                .clock
                .sample()
                .and_then(|now| owner.close(now.unix_ms));
            for result in [ledger_closed, owner_closed] {
                if let Err(secondary) = result {
                    original = original
                        .into_fatal()
                        .with_related_failure("retention_startup_cleanup", &secondary);
                }
            }
            return Err(original);
        }
        let performance = (|| {
            PerformanceMonitor::preflight_capacity(
                crate::performance::CapacityPreflightConfig {
                    performance: config.performance_monitor.clone(),
                    thresholds: config.capacity_thresholds,
                },
                crate::performance::CapacityRoots::new(
                    owner_epoch,
                    &config.state_root,
                    artifacts.root(),
                )?,
                &ledger,
                &events,
                &artifacts,
                Arc::clone(&config.clock),
            )
        })();
        let performance = match performance {
            Ok(performance) => performance,
            Err(mut original) => {
                let ledger_closed = ledger.close().map_err(|error| {
                    RuntimeHostError::fatal(
                        error.code(),
                        error.operation(),
                        RuntimeErrorCode::LedgerFailure,
                    )
                    .with_native_detail(format!("{error:?}"))
                });
                let owner_closed = config
                    .clock
                    .sample()
                    .and_then(|now| owner.close(now.unix_ms));
                for result in [ledger_closed, owner_closed] {
                    if let Err(secondary) = result {
                        original = original
                            .into_fatal()
                            .with_related_failure("capacity_startup_cleanup", &secondary);
                    }
                }
                return Err(original);
            }
        };
        let prior_epoch_recovery = limits
            .deadline()
            .map_err(|error| {
                RuntimeHostError::fatal(
                    error.code(),
                    error.operation(),
                    RuntimeErrorCode::LedgerFailure,
                )
            })
            .and_then(|deadline| {
                ledger
                    .reconcile_prior_epoch_closes(owner_epoch, journal, deadline)
                    .map_err(|error| {
                        RuntimeHostError::fatal(
                            error.code(),
                            error.operation(),
                            RuntimeErrorCode::LedgerFailure,
                        )
                        .with_native_detail(format!("{error:?}"))
                    })
            });
        if let Err(mut original) = prior_epoch_recovery {
            let ledger_closed = ledger.close().map_err(|error| {
                RuntimeHostError::fatal(
                    error.code(),
                    error.operation(),
                    RuntimeErrorCode::LedgerFailure,
                )
                .with_native_detail(format!("{error:?}"))
            });
            let owner_closed = config
                .clock
                .sample()
                .and_then(|now| owner.close(now.unix_ms));
            for result in [ledger_closed, owner_closed] {
                if let Err(secondary) = result {
                    original = original
                        .into_fatal()
                        .with_related_failure("prior_epoch_startup_cleanup", &secondary);
                }
            }
            return Err(original);
        }
        let provider = match assemble(&mut crate::ProviderStartup {
            ledger: &ledger,
            events: &events,
            owner_epoch,
            links: events.system_links()?,
        }) {
            Ok(provider) => provider,
            Err(original) => {
                // Assembly has not opened any device session. Native library caches keep
                // their existing process lifetime; this does not attest SDK shutdown.
                let ledger_closed = ledger.close();
                let owner_closed = owner.close(config.clock.sample()?.unix_ms);
                ledger_closed.map_err(|_| ledger_error("close_startup_ledger"))?;
                owner_closed?;
                return Err(original);
            }
        };
        let registered_instances = initial_registered_instances(provider.as_ref())?;
        let startup_packages = startup_package::resolve_startup_packages(
            &config.startup_packages,
            &registered_instances,
        )?;
        let stuck_recovery =
            recovery_ladder::resolve_stuck_recovery(&config.stuck_recovery, &registered_instances)?;
        let monitor_registry = MonitorRegistry::open(
            &config.state_root,
            registered_instances
                .values()
                .map(|instance| instance.instance_alias.clone()),
            owner_epoch,
            &ledger,
            &events,
        )?;
        let mut policy = PolicyHost::open(
            &config.state_root,
            Arc::clone(&state),
            &ledger,
            config.policy_cadence.clone(),
            &events,
        )?;
        reconcile_policy_dispatches(&mut policy, &ledger, &events)?;
        let authoritative_policy_outcomes =
            recover_authoritative_policy_outcomes(&policy, &ledger)?;
        let policy_dispatch_clocks = policy
            .recovered_dispatch_clocks()?
            .into_iter()
            .map(|(decision_id, admitted_at_unix_ms)| {
                (
                    decision_id,
                    PolicyDispatchClock::recovered(admitted_at_unix_ms),
                )
            })
            .collect();
        ApprovalProjection::recover(&ledger, Arc::clone(&state))?;
        reconcile_runtime_state(&state, &ledger, &events)?;
        let agent_instance_ids = registered_instances
            .values()
            .map(|instance| (instance.instance_alias.clone(), instance.instance_id))
            .collect::<BTreeMap<_, _>>();
        let mut agent_dispatcher = AgentDispatcherState::recover(&ledger, &agent_instance_ids)?;
        if let Some(agent_config) = &config.agent_dispatcher {
            reconcile_agent_wakes(
                &mut agent_dispatcher,
                &ledger,
                &events,
                &registered_instances,
                agent_config,
            )?;
        } else if agent_dispatcher.has_live_obligations() {
            return Err(RuntimeHostError::fatal(
                "agent_dispatcher_config_missing",
                "start_runtime_host",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        let listener = TcpListener::bind(config.bind_address).map_err(|_| {
            RuntimeHostError::fatal(
                "runtime_bind_failed",
                "start_runtime_host",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        listener.set_nonblocking(true).map_err(|_| {
            RuntimeHostError::fatal(
                "runtime_listener_config_failed",
                "start_runtime_host",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        let local_address = listener.local_addr().map_err(|_| {
            RuntimeHostError::fatal(
                "runtime_listener_address_failed",
                "start_runtime_host",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        append_runtime_start_event(
            &ledger,
            &events,
            &config.state_root,
            takeover,
            config.device_diagnostic_mode,
        )?;
        append_instance_binding_events(&ledger, &events, &registered_instances)?;
        let prepared = (|| {
            let facts = InstanceFactStore::recover(&ledger, Arc::clone(&state))?;
            let performance_interval = performance.sample_interval().or_else(|| {
                config
                    .frame_retention_enabled
                    .then_some(Duration::from_secs(2))
            });
            let performance_control =
                PerformanceBalanceController::new(config.performance_control.clone())?;
            let info = RuntimeInfo::new(
                std::process::id(),
                local_address.ip().to_string(),
                local_address.port(),
                owner_epoch,
                started_at_unix_ms,
            )
            .map_err(|_| {
                RuntimeHostError::fatal(
                    "runtime_info_invalid",
                    "start_runtime_host",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
            let info_path = config.state_root.join(RUNTIME_INFO_FILE);
            publish_runtime_info(&info_path, &info)?;
            Ok::<_, RuntimeHostError>((
                facts,
                performance,
                performance_interval,
                performance_control,
                info,
                info_path,
            ))
        })();
        let (facts, performance, performance_interval, performance_control, info, info_path) =
            match prepared {
                Ok(prepared) => prepared,
                Err(original) => {
                    if let Err(error) = device_diagnostic::append_device_diagnostic_record(
                        &ledger,
                        &events,
                        owner_epoch,
                        actingcommand_contract::DeviceDiagnosticBudgetRecord::new(
                            owner_epoch,
                            config.device_diagnostic_mode,
                        ),
                    ) {
                        return Err(device_diagnostic::summary_incomplete(
                            Some(original),
                            &error,
                        ));
                    }
                    return Err(original);
                }
            };
        let (runtime_facts, runtime_facts_dirty) = runtime_facts::recover_runtime_fact_store(
            &ledger,
            &events,
            takeover,
            config.clock.sample()?.unix_ms,
        )?;
        let fatal = FatalState::default();
        let shared = Arc::new(HostShared {
            owner_epoch,
            shutdown_target: info.shutdown_target(),
            lifecycle_admission: RwLock::new(false),
            scheduler: Arc::new(Mutex::new(scheduler)),
            policy: Mutex::new(policy),
            performance: Mutex::new(performance),
            performance_control: Mutex::new(performance_control),
            frame_retention: Mutex::new(
                config
                    .frame_retention_enabled
                    .then(|| frame_retention::FrameRetention::new(config.failed_run_retention)),
            ),
            governance_write_gate: Mutex::new(()),
            governance_capability_sha256: config.governance_capability_sha256,
            governance_connections: Mutex::new(BTreeSet::new()),
            fact_write_gate: Mutex::new(()),
            signature_write_gate: Mutex::new(()),
            device_diagnostics: Mutex::new(device_diagnostic::DeviceDiagnosticBudget::new(
                owner_epoch,
                config.device_diagnostic_mode,
            )),
            lifecycle_append_failed: AtomicBool::new(false),
            detection_write_gate: Mutex::new(()),
            state_write_gate: Mutex::new(()),
            agent_write_gate: Mutex::new(()),
            proposal_write_gate: Mutex::new(()),
            facts: Mutex::new(facts),
            runtime_facts: Mutex::new(runtime_facts),
            runtime_facts_dirty: AtomicBool::new(runtime_facts_dirty),
            policy_inputs: Mutex::new(config.policy_inputs),
            authoritative_policy_outcomes: Mutex::new(authoritative_policy_outcomes),
            procedure_manifest: Mutex::new(config.procedure_manifest),
            verified_materials: Mutex::new(material_read::VerifiedMaterialCache::default()),
            owner: Mutex::new(owner),
            ledger,
            artifacts,
            state,
            agent_dispatcher_config: config.agent_dispatcher,
            agent_dispatcher: Mutex::new(agent_dispatcher),
            events,
            execution: ExecutionKernel::new(provider),
            registered_instances: Mutex::new(registered_instances),
            monitor_registry: Mutex::new(monitor_registry),
            queued_requests: Mutex::new(BTreeMap::new()),
            queue_terminals: Mutex::new(QueueTerminalStore::default()),
            #[cfg(test)]
            queue_operation_test_hook: Mutex::new(None),
            #[cfg(test)]
            policy_outcome_transition_test_hook: Mutex::new(None),
            #[cfg(test)]
            scheduled_policy_checkpoint_test_hook: Mutex::new(None),
            #[cfg(test)]
            contained_task_checkpoint_test_hook: Mutex::new(None),
            #[cfg(test)]
            lease_expiry_scan_test_gate: Mutex::new(()),
            #[cfg(test)]
            lease_expiry_test_checkpoints: Mutex::new(Vec::new()),
            trusted_policy_dispatches: Mutex::new(TrustedPolicyDispatchStore::default()),
            policy_dispatch_clocks: Mutex::new(policy_dispatch_clocks),
            policy_outcome_gate: Mutex::new(()),
            admission_guards: Mutex::new(BTreeMap::new()),
            debug_runs: Mutex::new(BTreeMap::new()),
            contained_runs: Mutex::new(BTreeMap::new()),
            startup_packages,
            pending_host_work: Mutex::new(VecDeque::new()),
            resource_packages: config.resource_packages,
            stuck_recovery,
            recovery_ladders: Mutex::new(BTreeMap::new()),
            parked_recovery_ladders: Mutex::new(BTreeMap::new()),
            #[cfg(test)]
            scheduling_terminal_append_failures: AtomicU64::new(0),
            #[cfg(test)]
            contained_task_stability_persistence_failures: AtomicU64::new(0),
            #[cfg(test)]
            contained_task_ocr_failure_persistence_failures: AtomicU64::new(0),
            #[cfg(test)]
            policy_outcome_projection_failures: AtomicU64::new(0),
            #[cfg(test)]
            policy_outcome_projection_position_override: AtomicU64::new(0),
            next_connection_id: AtomicU64::new(1),
            clock: Arc::clone(&config.clock),
            clock_origin_monotonic_ms: clock_origin.monotonic_ms,
            fatal,
        });
        if let Err(original) = shared.synchronize_fact_store() {
            failed_start_cleanup(shared, &info_path, None, None, None, None)?;
            return Err(original);
        }
        if let Some(config_manifest) = &config.config_manifest
            && let Err(original) = shared.record_config_manifest(config_manifest)
        {
            failed_start_cleanup(shared, &info_path, None, None, None, None)?;
            return Err(original);
        }
        if let Err(original) = shared.expire_agent_sessions() {
            failed_start_cleanup(shared, &info_path, None, None, None, None)?;
            return Err(original);
        }
        let sweep_shared = Arc::clone(&shared);
        let sweep_thread = match thread::Builder::new()
            .name("actingcommand-runtime-sweeper".to_string())
            .spawn(move || lease_sweep_loop(sweep_shared))
        {
            Ok(thread) => thread,
            Err(_) => {
                let original = RuntimeHostError::fatal(
                    "runtime_sweeper_spawn_failed",
                    "start_runtime_host",
                    RuntimeErrorCode::RuntimeFatal,
                );
                failed_start_cleanup(shared, &info_path, None, None, None, None)?;
                return Err(original);
            }
        };
        let monitor_shared = Arc::clone(&shared);
        let monitor_thread = match thread::Builder::new()
            .name("actingcommand-runtime-monitor".to_string())
            .spawn(move || monitor_probe_loop(monitor_shared))
        {
            Ok(thread) => thread,
            Err(_) => {
                let original = RuntimeHostError::fatal(
                    "runtime_monitor_spawn_failed",
                    "start_runtime_host",
                    RuntimeErrorCode::RuntimeFatal,
                );
                failed_start_cleanup(shared, &info_path, Some(sweep_thread), None, None, None)?;
                return Err(original);
            }
        };
        // Slice #316-B3: the host's own scheduling point for startup packages, a peer of the
        // monitor thread; a start request never runs the package on its connection thread.
        let startup_shared = Arc::clone(&shared);
        let startup_thread = match thread::Builder::new()
            .name("actingcommand-runtime-startup".to_string())
            .spawn(move || startup_package::startup_package_loop(startup_shared))
        {
            Ok(thread) => thread,
            Err(_) => {
                let original = RuntimeHostError::fatal(
                    "runtime_startup_spawn_failed",
                    "start_runtime_host",
                    RuntimeErrorCode::RuntimeFatal,
                );
                failed_start_cleanup(
                    shared,
                    &info_path,
                    Some(sweep_thread),
                    Some(monitor_thread),
                    None,
                    None,
                )?;
                return Err(original);
            }
        };
        let performance_thread = if let Some(interval) = performance_interval {
            let performance_shared = Arc::clone(&shared);
            match thread::Builder::new()
                .name("actingcommand-runtime-performance".to_string())
                .spawn(move || performance_monitor_loop(performance_shared, interval))
            {
                Ok(thread) => Some(thread),
                Err(_) => {
                    let original = RuntimeHostError::fatal(
                        "runtime_performance_spawn_failed",
                        "start_runtime_host",
                        RuntimeErrorCode::RuntimeFatal,
                    );
                    failed_start_cleanup(
                        shared,
                        &info_path,
                        Some(sweep_thread),
                        Some(monitor_thread),
                        Some(startup_thread),
                        None,
                    )?;
                    return Err(original);
                }
            }
        } else {
            None
        };
        let accept_shared = Arc::clone(&shared);
        let maximum_frame_bytes = config.maximum_frame_bytes;
        let io_timeout = config.io_timeout;
        #[cfg(feature = "test-observation")]
        let accept_observation_owner = crate::test_observation::current_observation_owner();
        let accept_thread = match thread::Builder::new()
            .name("actingcommand-runtime-ipc".to_string())
            .spawn(move || {
                #[cfg(feature = "test-observation")]
                let _observation_owner =
                    crate::test_observation::enter_observation_owner(accept_observation_owner);
                accept_loop(listener, accept_shared, maximum_frame_bytes, io_timeout)
            }) {
            Ok(thread) => thread,
            Err(_) => {
                let original = RuntimeHostError::fatal(
                    "runtime_accept_spawn_failed",
                    "start_runtime_host",
                    RuntimeErrorCode::RuntimeFatal,
                );
                failed_start_cleanup(
                    shared,
                    &info_path,
                    Some(sweep_thread),
                    Some(monitor_thread),
                    Some(startup_thread),
                    performance_thread,
                )?;
                return Err(original);
            }
        };
        Ok(Self {
            info,
            info_path,
            shared: Some(shared),
            accept_thread: Some(accept_thread),
            sweep_thread: Some(sweep_thread),
            monitor_thread: Some(monitor_thread),
            startup_thread: Some(startup_thread),
            performance_thread,
        })
    }

    pub const fn runtime_info(&self) -> &RuntimeInfo {
        &self.info
    }

    /// The daemon checks fatal_error first, then returns through its owned close path.
    pub fn is_shutdown_requested(&self) -> RuntimeHostResult<bool> {
        Ok(self
            .shared_ref("read_runtime_shutdown")?
            .fatal
            .is_shutdown_requested())
    }

    /// Keeps a whole policy cycle inside the same admission boundary as IPC and native probes.
    pub fn begin_policy_work(&self) -> RuntimeHostResult<Option<RuntimePolicyWork<'_>>> {
        Ok(self
            .shared_ref("begin_policy_work")?
            .begin_work()?
            .map(|guard| RuntimePolicyWork { _guard: guard }))
    }

    pub fn fatal_error(&self) -> RuntimeHostResult<Option<RuntimeHostError>> {
        let shared = self.shared_ref("read_runtime_health")?;
        if let Err(error) = shared.ledger.check_writer_health() {
            shared.fatal.mark(RuntimeHostError::fatal(
                error.code(),
                error.operation(),
                RuntimeErrorCode::LedgerFailure,
            ))?;
        }
        shared.fatal.current()
    }

    pub fn active_policy_catalog(&self) -> RuntimeHostResult<Option<CatalogGeneration>> {
        self.shared_ref("read_active_policy_catalog")?
            .active_policy_catalog()
    }

    pub fn activate_policy_catalog(
        &self,
        sources: &CatalogSources,
    ) -> RuntimeHostResult<CatalogGeneration> {
        self.work_ref("activate_policy_catalog")?
            .activate_policy_catalog(sources)
    }

    #[cfg(test)]
    pub(crate) fn activate_policy_catalog_with_expected_for_test(
        &self,
        sources: &CatalogSources,
        expected: CatalogGeneration,
    ) -> RuntimeHostResult<(
        RuntimeHostResult<CatalogGeneration>,
        Option<actingcommand_runtime_state::StateDocument>,
    )> {
        let shared = self.shared_ref("activate_policy_catalog_with_expected_for_test")?;
        let catalog = lock(&shared.policy, "stage_policy_catalog_for_cas_test")?.stage(sources)?;
        let result = shared.switch_policy_catalog(
            catalog,
            Some(expected),
            EventAction::CatalogActivate,
            CatalogTransitionTarget::Activated,
            None,
        );
        let document = shared
            .state
            .read_json_document(actingcommand_runtime_state::CATALOG_ACTIVE_STATE_KEY)
            .map_err(|error| RuntimeHostError::state(&error))?;
        Ok((result, document))
    }

    pub fn rollback_policy_catalog(
        &self,
        catalog_hash: &str,
    ) -> RuntimeHostResult<CatalogGeneration> {
        self.work_ref("rollback_policy_catalog")?
            .rollback_policy_catalog(catalog_hash)
    }

    pub fn stage_release_set(
        &self,
        manifest: RuntimeReleaseSet,
        sources: &ReleaseArtifactSources,
    ) -> RuntimeHostResult<RuntimeReleaseSet> {
        self.work_ref("stage_release_set")?
            .stage_release_set(manifest, sources)
    }

    pub fn active_release_set(&self) -> RuntimeHostResult<Option<RuntimeReleaseSet>> {
        self.shared_ref("read_active_release_set")?
            .active_release_set()
    }

    pub fn activate_release_set(&self, release_id: &str) -> RuntimeHostResult<RuntimeReleaseSet> {
        self.work_ref("activate_release_set")?
            .switch_release_set(ReleaseTransitionKind::Activate, release_id)
    }

    pub fn rollback_release_set(&self, release_id: &str) -> RuntimeHostResult<RuntimeReleaseSet> {
        self.work_ref("rollback_release_set")?
            .switch_release_set(ReleaseTransitionKind::Rollback, release_id)
    }

    #[cfg(test)]
    pub(crate) fn store_test_report(
        &self,
        bytes: &[u8],
    ) -> RuntimeHostResult<ProjectedArtifactReference> {
        self.shared_ref("store_test_report")?
            .store_test_report(bytes)
    }

    pub fn prepare_strategic_report(
        &self,
        report: &StrategicReport,
        evidence: &[ProjectedArtifactReference],
    ) -> RuntimeHostResult<StrategicPlanPreparation> {
        self.work_ref("prepare_strategic_report")?
            .prepare_strategic_report(report, evidence)
    }

    /// Evaluates one policy cycle from Runtime-owned facts, resources, time, and seed.
    pub fn evaluate_policy_cycle(&self, trigger: PolicyTrigger) -> RuntimeHostResult<PolicyCycle> {
        self.work_ref("evaluate_policy_cycle")?
            .evaluate_policy_cycle(trigger)
    }

    #[cfg(test)]
    pub(crate) fn evaluate_policy_cycle_with_test_inputs(
        &self,
        facts: &EvaluationFacts,
        resources: &EvaluationResources,
        time: EvaluationTime,
        seed: u64,
        trigger: PolicyTrigger,
    ) -> RuntimeHostResult<PolicyCycle> {
        self.work_ref("evaluate_policy_cycle")?
            .evaluate_policy_cycle_with_test_inputs(facts, resources, time, seed, trigger)
    }

    #[cfg(test)]
    pub(crate) fn replace_procedure_manifest_for_test(
        &self,
        procedure_manifest: ProcedureManifest,
    ) -> RuntimeHostResult<()> {
        self.shared_ref("replace_procedure_manifest_for_test")?
            .replace_procedure_manifest_for_test(procedure_manifest)
    }

    /// Runs a bounded future dry-run through the same pure policy evaluator used for admission.
    pub fn project_policy_forward(
        &self,
        facts: &EvaluationFacts,
        resources: &EvaluationResources,
        time: EvaluationTime,
        seed: u64,
        config: ForwardProjectionConfig,
    ) -> RuntimeHostResult<ForwardProjection> {
        self.work_ref("project_policy_forward")?
            .project_policy_forward(facts, resources, time, seed, config)
    }

    /// Assesses ledger-pinned trends and publishes an idempotent recheck planning signal when due.
    pub fn assess_and_publish_predictive_maintenance(
        &self,
        query: &MaintenanceLedgerQuery,
    ) -> RuntimeHostResult<MaintenanceAssessment> {
        self.work_ref("assess_predictive_maintenance")?
            .assess_and_publish_predictive_maintenance(query)
    }

    /// Validates and durably publishes one Runtime-owned fact into the GlobalLedger.
    pub fn publish_fact(&self, record: FactRecord) -> RuntimeHostResult<EventId> {
        self.work_ref("publish_fact")?.publish_fact(record)
    }

    /// Returns an immutable ledger-pinned fact projection for one instance context.
    pub fn instance_fact_snapshot(
        &self,
        context: InstanceFactContext,
    ) -> RuntimeHostResult<InstanceFactSnapshot> {
        self.shared_ref("read_instance_fact_snapshot")?
            .instance_fact_snapshot(context)
    }

    /// Returns the sealed image of the Runtime's own fact store at the ledger's latest sequence.
    pub fn runtime_fact_snapshot(&self) -> RuntimeHostResult<RuntimeFactSnapshot> {
        self.shared_ref("read_runtime_fact_snapshot")?
            .runtime_fact_snapshot()
    }

    pub fn admit_policy_dispatch(
        &self,
        intent: &DispatchIntent,
        reason_chain: &DecisionReasonChain,
        context: &PolicyAdmissionContext,
    ) -> RuntimeHostResult<PolicyDispatchAdmission> {
        match self.work_ref("admit_policy_dispatch") {
            Ok(work) => work.admit_policy_dispatch(intent, reason_chain, context, None),
            Err(error) => self
                .shared_ref("record_policy_admission_failure")?
                .record_policy_admission_result(intent, Err(error), None),
        }
    }

    /// Admits a contained policy run using its bounded request budget for the lease.
    pub fn admit_scheduled_policy_dispatch(
        &self,
        intent: &DispatchIntent,
        reason_chain: &DecisionReasonChain,
        context: &PolicyAdmissionContext,
        task_request: &ContainedTaskRequest,
    ) -> RuntimeHostResult<PolicyDispatchAdmission> {
        match self.work_ref("admit_policy_dispatch") {
            Ok(work) => {
                work.admit_policy_dispatch(intent, reason_chain, context, Some(task_request))
            }
            Err(error) => self
                .shared_ref("record_policy_admission_failure")?
                .record_policy_admission_result(intent, Err(error), None),
        }
    }

    pub fn pinned_policy_catalog(
        &self,
        decision_id: &str,
    ) -> RuntimeHostResult<Option<CatalogGeneration>> {
        self.shared_ref("read_pinned_policy_catalog")?
            .pinned_policy_catalog(decision_id)
    }

    #[cfg(test)]
    pub(crate) fn complete_policy_dispatch(&self, decision_id: &str) -> RuntimeHostResult<()> {
        self.work_ref("complete_policy_dispatch")?
            .record_policy_dispatch_outcome(decision_id, &PolicyExecutionInput::Succeeded, None)
            .map(|_| ())
    }

    #[cfg(test)]
    pub(crate) fn record_policy_dispatch_outcome(
        &self,
        decision_id: &str,
        input: &PolicyExecutionInput,
    ) -> RuntimeHostResult<PolicyExecutionEventData> {
        self.work_ref("record_policy_dispatch_outcome")?
            .record_policy_dispatch_outcome(decision_id, input, None)
    }

    #[cfg(test)]
    pub(crate) fn complete_scheduled_policy_success_without_terminal_for_test(
        &self,
        context: &PolicyRunContext,
    ) -> RuntimeHostResult<PolicyExecutionEventData> {
        let shared =
            self.shared_ref("complete_scheduled_policy_success_without_terminal_for_test")?;
        shared.ensure_scheduled_policy_lease_released(context)?;
        shared.record_policy_dispatch_outcome(
            context.decision_id(),
            &PolicyExecutionInput::Succeeded,
            Some(context),
        )
    }

    #[cfg(test)]
    pub(crate) fn complete_policy_success_with_partial_links_for_test(
        &self,
        context: &PolicyRunContext,
    ) -> RuntimeHostResult<PolicyExecutionEventData> {
        let shared = self.shared_ref("complete_policy_success_with_partial_links_for_test")?;
        shared.ensure_scheduled_policy_lease_released(context)?;
        shared.record_policy_dispatch_outcome(
            context.decision_id(),
            &PolicyExecutionInput::Succeeded,
            None,
        )
    }

    #[cfg(test)]
    pub(crate) fn complete_scheduled_policy_failure_without_terminal_for_test(
        &self,
        context: &PolicyRunContext,
    ) -> RuntimeHostResult<PolicyExecutionEventData> {
        let shared =
            self.shared_ref("complete_scheduled_policy_failure_without_terminal_for_test")?;
        shared.ensure_scheduled_policy_lease_released(context)?;
        shared.record_policy_dispatch_outcome_with_cache_update(
            context.decision_id(),
            &PolicyExecutionInput::Failed {
                error_code: "injected.pre-terminal".to_owned(),
                class: PolicyFailureClass::Recoverable,
            },
            Some(context),
            PolicyOutcomeCacheUpdate::Clear(context),
        )
    }

    #[cfg(test)]
    pub(crate) fn validate_policy_admission_request_for_test(
        &self,
        context: &PolicyRunContext,
        admission_request_id: RequestId,
        through_sequence: u64,
    ) -> RuntimeHostResult<()> {
        let shared = self.shared_ref("validate_policy_admission_request_for_test")?;
        validate_policy_run_admission_request(
            &shared.ledger,
            context.lease_token().instance_id(),
            admission_request_id,
            context.correlation_id(),
            context.task_id(),
            context.run_id(),
            context.decision_id(),
            context.catalog_task_id(),
            context.instance_alias(),
            through_sequence,
        )
    }

    #[cfg(test)]
    pub(crate) fn policy_outcome_key_snapshot_for_test(
        &self,
    ) -> RuntimeHostResult<PolicyOutcomeKeySnapshot> {
        let shared = self.shared_ref("policy_outcome_key_snapshot_for_test")?;
        let _gate = lock(
            &shared.policy_outcome_gate,
            "policy_outcome_key_snapshot_for_test",
        )?;
        lock(&shared.policy, "policy_outcome_key_snapshot_for_test")?.outcome_key_snapshot()
    }

    #[cfg(test)]
    pub(crate) fn pause_policy_outcome_transition_for_test(
        &self,
    ) -> RuntimeHostResult<PolicyOutcomeTransitionTestControl> {
        let shared = self.shared_ref("install_policy_outcome_transition_test_hook")?;
        let completion_committed = Arc::new(Barrier::new(2));
        let resume = Arc::new(Barrier::new(2));
        let mut slot = lock(
            &shared.policy_outcome_transition_test_hook,
            "install_policy_outcome_transition_test_hook",
        )?;
        if slot.is_some() {
            return Err(RuntimeHostError::fatal(
                "policy_outcome_transition_test_hook_already_installed",
                "install_policy_outcome_transition_test_hook",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        *slot = Some(PolicyOutcomeTransitionTestHook {
            completion_committed: Arc::clone(&completion_committed),
            resume: Arc::clone(&resume),
        });
        Ok(PolicyOutcomeTransitionTestControl {
            completion_committed,
            resume,
        })
    }

    #[cfg(test)]
    pub(crate) fn count_scheduled_policy_checkpoint_for_test(
        &self,
        identity: ScheduledPolicyCheckpointIdentity,
    ) -> RuntimeHostResult<ScheduledPolicyCheckpointTestControl> {
        let consumed = Arc::new(AtomicU64::new(0));
        self.install_scheduled_policy_checkpoint_for_test(
            identity,
            ScheduledPolicyCheckpointTestAction::Count(Arc::clone(&consumed)),
        )?;
        Ok(ScheduledPolicyCheckpointTestControl { consumed })
    }

    #[cfg(test)]
    pub(crate) fn exit_at_scheduled_policy_checkpoint_for_test(
        &self,
        context: &PolicyRunContext,
        marker: PathBuf,
    ) -> RuntimeHostResult<()> {
        self.install_scheduled_policy_checkpoint_for_test(
            ScheduledPolicyCheckpointIdentity::for_context(context),
            ScheduledPolicyCheckpointTestAction::Exit { marker },
        )
    }

    #[cfg(test)]
    fn install_scheduled_policy_checkpoint_for_test(
        &self,
        identity: ScheduledPolicyCheckpointIdentity,
        action: ScheduledPolicyCheckpointTestAction,
    ) -> RuntimeHostResult<()> {
        let shared = self.shared_ref("install_scheduled_policy_checkpoint_test_hook")?;
        let mut slot = lock(
            &shared.scheduled_policy_checkpoint_test_hook,
            "install_scheduled_policy_checkpoint_test_hook",
        )?;
        if slot.is_some() {
            return Err(RuntimeHostError::fatal(
                "scheduled_policy_checkpoint_test_hook_already_installed",
                "install_scheduled_policy_checkpoint_test_hook",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        *slot = Some(ScheduledPolicyCheckpointTestHook {
            identity,
            execution_thread: thread::current().id(),
            action,
        });
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn run_at_contained_task_checkpoint_for_test<F>(
        &self,
        request_id: RequestId,
        instance_id: InstanceId,
        lease_id: Option<LeaseId>,
        action: F,
    ) -> RuntimeHostResult<ContainedTaskCheckpointTestControl>
    where
        F: FnOnce(ContainedTaskCheckpointIdentity) + Send + 'static,
    {
        let shared = self.shared_ref("install_contained_task_checkpoint_test_hook")?;
        let consumed = Arc::new(AtomicU64::new(0));
        let observed = Arc::new(Mutex::new(None));
        let mut slot = lock(
            &shared.contained_task_checkpoint_test_hook,
            "install_contained_task_checkpoint_test_hook",
        )?;
        if slot.is_some() {
            return Err(RuntimeHostError::fatal(
                "contained_task_checkpoint_test_hook_already_installed",
                "install_contained_task_checkpoint_test_hook",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        *slot = Some(ContainedTaskCheckpointTestHook {
            request_id,
            instance_id,
            lease_id,
            execution_thread: thread::current().id(),
            action: Box::new(action),
            consumed: Arc::clone(&consumed),
            observed: Arc::clone(&observed),
        });
        Ok(ContainedTaskCheckpointTestControl { consumed, observed })
    }

    /// Executes one admitted policy run through the contained-task boundary without reacquiring
    /// its scheduler lease.
    pub fn run_scheduled_contained_task(
        &self,
        context: &PolicyRunContext,
        request: &ContainedTaskRequest,
    ) -> RuntimeHostResult<RuntimeReceipt> {
        let shared = self.work_ref("run_scheduled_contained_task")?;
        let (request, success) = match shared.run_scheduled_contained_task(context, request) {
            Ok(success) => success,
            Err(failure) => {
                shared.record_scheduled_policy_failure(context, &failure.error)?;
                let settlement = shared.complete_scheduled_policy_failure(context, &failure);
                let error = *failure.error;
                if failure.poison_runtime {
                    shared.fatal.mark(error.clone())?;
                }
                if let Err(settlement_error) = settlement {
                    shared.record_scheduled_policy_failure(context, &settlement_error)?;
                    return Err(settlement_error);
                }
                return Err(error);
            }
        };
        RuntimeReceipt::success(&request, success.state, success.terminal, success.result)
            .map_err(|_| receipt_error())
    }

    /// Validates a same-run contained-task receipt and records the policy execution outcome.
    pub fn complete_scheduled_policy_run(
        &self,
        context: &PolicyRunContext,
        receipt: &RuntimeReceipt,
    ) -> RuntimeHostResult<(
        PolicyExecutionEventData,
        Option<SchedulingOutcomeProjection>,
    )> {
        let shared = self.work_ref("complete_scheduled_policy_run")?;
        let result = shared.complete_scheduled_policy_run(context, receipt);
        if let Err(error) = &result {
            shared.record_scheduled_policy_failure(context, error)?;
        }
        result
    }

    pub fn record_policy_planning_signal(
        &self,
        signal: PolicyPlanningSignalEventData,
    ) -> RuntimeHostResult<()> {
        if matches!(
            signal.kind,
            actingcommand_contract::PolicyPlanningSignalKind::DetectionReserved
                | actingcommand_contract::PolicyPlanningSignalKind::DetectionQuotaExhausted
        ) {
            return Err(RuntimeHostError::request(
                "policy_detection_signal_runtime_owned",
                "record_policy_planning_signal",
                RuntimeErrorCode::InvalidRequest,
            ));
        }
        self.work_ref("record_policy_planning_signal")?
            .record_policy_planning_signal(signal)
    }

    pub fn record_pipeline_performance(
        &self,
        signal: PipelinePerformanceSignal,
    ) -> RuntimeHostResult<()> {
        self.work_ref("record_pipeline_performance")?
            .record_pipeline_performance(signal)
    }

    pub fn performance_control_directive(
        &self,
        instance_id: &str,
    ) -> RuntimeHostResult<PerformanceControlDirective> {
        self.shared_ref("read_performance_control_directive")?
            .performance_control_directive(instance_id)
    }

    #[cfg(test)]
    pub(crate) fn process_request_for_test(
        &self,
        request: &RuntimeRequest,
        connection_id: ConnectionId,
    ) -> RuntimeHostResult<RuntimeReceipt> {
        self.shared
            .as_ref()
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    "runtime_host_closed",
                    "process_test_request",
                    RuntimeErrorCode::RuntimeUnavailable,
                )
            })?
            .process_request(request, connection_id)
    }

    #[cfg(test)]
    pub(crate) fn pause_queue_operation_after_snapshot_for_test(
        &self,
        operation: QueueOperationTestKind,
        request_id: RequestId,
    ) -> RuntimeHostResult<QueueOperationTestControl> {
        let shared = self.shared_ref("install_queue_operation_test_hook")?;
        let snapshot_reached = Arc::new(Barrier::new(2));
        let resume = Arc::new(Barrier::new(2));
        let mut slot = lock(
            &shared.queue_operation_test_hook,
            "install_queue_operation_test_hook",
        )?;
        if slot.is_some() {
            return Err(RuntimeHostError::fatal(
                "queue_operation_test_hook_already_installed",
                "install_queue_operation_test_hook",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        *slot = Some(QueueOperationTestHook {
            operation,
            request_id,
            snapshot_reached: Arc::clone(&snapshot_reached),
            resume: Arc::clone(&resume),
        });
        Ok(QueueOperationTestControl {
            snapshot_reached,
            resume,
        })
    }

    #[cfg(test)]
    pub(crate) fn expire_all_queued_for_test(&self) -> RuntimeHostResult<()> {
        self.shared_ref("expire_all_queued_for_test")?
            .expire_all_queued_runtime()
    }

    #[cfg(test)]
    pub(crate) fn expire_lease_once_for_test(
        &self,
        token: &LeaseToken,
    ) -> RuntimeHostResult<TerminalEvent> {
        let shared = self.shared_ref("expire_lease_once_for_test")?;
        let _scan = lock(
            &shared.lease_expiry_scan_test_gate,
            "serialize_test_lease_expiry_scan",
        )?;
        if let Some(terminal) = shared.replay_lease_expiry_checkpoint_for_test(token)? {
            return Ok(terminal);
        }
        if shared
            .durable_lease_expiry_terminal_for_test(token)?
            .is_some()
        {
            return Err(RuntimeHostError::fatal(
                "test_lease_expiry_checkpoint_missing",
                "expire_lease_once_for_test",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }

        let now = shared.monotonic_ms()?;
        let connection_id = {
            let scheduler = lock(&shared.scheduler, "scan_test_lease_expiry")?;
            let mut candidates = scheduler
                .active_tokens()
                .into_iter()
                .filter(|active| lease_token_identity_match_count(active, token) >= 4);
            let active = candidates.next().ok_or_else(|| {
                RuntimeHostError::fatal(
                    "test_lease_expiry_token_missing",
                    "expire_lease_once_for_test",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
            if candidates.next().is_some() {
                return Err(RuntimeHostError::fatal(
                    "test_lease_expiry_token_candidate_not_unique",
                    "expire_lease_once_for_test",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            if active != *token {
                return Err(RuntimeHostError::fatal(
                    "test_lease_expiry_token_identity_mismatch",
                    "expire_lease_once_for_test",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            if !scheduler
                .due_tokens(now)
                .into_iter()
                .any(|due| due == *token)
            {
                return Err(RuntimeHostError::fatal(
                    "test_lease_expiry_token_not_due",
                    "expire_lease_once_for_test",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            scheduler.connection_for_token(token).map_err(|error| {
                RuntimeHostError::scheduler("read_test_lease_expiry_connection", &error)
            })?
        };

        // This existing owner returns only after token cleanup, any queued transfer, persisted
        // scheduler state, and the durable lease terminal have all completed.
        shared.cleanup_token(token, connection_id, LeaseReleaseReason::Expired)?;
        shared.record_completed_lease_expiry_for_test(token)
    }

    #[cfg(test)]
    pub(crate) fn performance_context_for_test(
        &self,
        instance_id: &str,
        observed_at_unix_ms: u64,
    ) -> RuntimeHostResult<PerformanceContext> {
        self.shared_ref("read_test_performance_context")?
            .performance_context(instance_id, observed_at_unix_ms)
    }

    #[cfg(test)]
    pub(crate) fn replace_capacity_sampler_for_test(
        &self,
        sampler: Box<dyn actingcommand_host_metrics::HostSampler>,
    ) -> RuntimeHostResult<()> {
        let shared = self.shared_ref("replace_test_capacity_sampler")?;
        let mut performance = lock(&shared.performance, "replace_test_capacity_sampler")?;
        performance.replace_capacity_sampler_for_test(sampler);
        performance.sample_and_record_capacity(&shared.ledger, &shared.events)
    }

    #[cfg(test)]
    pub(crate) fn capacity_sampler_for_test(
        &self,
    ) -> RuntimeHostResult<Box<dyn Fn() -> RuntimeHostResult<()> + Send>> {
        let owner = Arc::downgrade(self.shared.as_ref().ok_or_else(|| {
            RuntimeHostError::fatal(
                "runtime_host_closed",
                "sample_test_capacity",
                RuntimeErrorCode::RuntimeUnavailable,
            )
        })?);
        Ok(Box::new(move || {
            let shared = owner.upgrade().ok_or_else(|| {
                RuntimeHostError::fatal(
                    "runtime_host_closed",
                    "sample_test_capacity",
                    RuntimeErrorCode::RuntimeUnavailable,
                )
            })?;
            lock(&shared.performance, "sample_test_capacity")?
                .sample_and_record_capacity(&shared.ledger, &shared.events)
        }))
    }

    #[cfg(test)]
    pub(crate) fn observe_performance_control_for_test(
        &self,
        observation: crate::PerformanceControlObservation,
    ) -> RuntimeHostResult<()> {
        self.shared_ref("observe_test_performance_control")?
            .reconcile_performance_control(observation)
    }

    #[cfg(test)]
    pub(crate) fn append_approval_event_for_test(
        &self,
        source: EventSource,
        actor: EventActor,
        decision: ApprovalDecisionRecord,
    ) -> RuntimeHostResult<()> {
        let shared = self.shared_ref("append_test_approval_event")?;
        shared.append_event_raw(
            EventSeverity::Info,
            source,
            OriginModule::Governance,
            actor,
            shared.events.system_links()?,
            ApprovalPayloadDraft::decision(decision, AuditInput::new()),
        )?;
        Ok(())
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn append_contained_task_terminal_for_test(
        &self,
        request: &RuntimeRequest,
        token: &LeaseToken,
        task_id: IssuedTaskId,
        run_id: IssuedRunId,
        outcome: TaskOutcome,
        intent_already_recorded: bool,
        final_page: Option<String>,
        executed_steps: u32,
        failure_code: Option<&'static str>,
    ) -> Result<TerminalEvent, (RuntimeReceiptState, RuntimeErrorCode, Option<TerminalEvent>)> {
        let shared = self.shared.as_ref().expect("test Runtime host is open");
        let validated = request.validate().expect("test Runtime request is valid");
        shared
            .append_contained_task_terminal(
                &validated,
                token,
                ContainedTaskTerminalDraft {
                    task_id,
                    run_id,
                    outcome,
                    intent_already_recorded,
                    final_page,
                    executed_steps: Some(executed_steps),
                    failure_code,
                    failure_severity: None,
                    projection_failure_severity: None,
                    scheduling_outcome: None,
                    selected_scheduling_outcome: None,
                    capture_summary: None,
                    task_timing: None,
                },
            )
            .map(|event| terminal(&event))
            .map_err(|failure| {
                (
                    failure.state,
                    failure.error.projection().code,
                    failure.terminal,
                )
            })
    }

    #[cfg(test)]
    pub(crate) fn append_contained_task_semantic_for_test(
        &self,
        request: &RuntimeRequest,
        token: &LeaseToken,
        task_id: IssuedTaskId,
        run_id: IssuedRunId,
        fact: TaskSemanticFact,
    ) -> RuntimeHostResult<()> {
        let shared = self.shared_ref("append_test_contained_task_semantic")?;
        let validated = request.validate().expect("test Runtime request is valid");
        let links = shared
            .events
            .request_links(
                &validated,
                Some(token.instance_id()),
                Some(token.lease_id()),
                None,
            )
            .with_task_id(task_id)
            .with_run_id(run_id);
        shared.append_event_raw(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            TaskPayloadDraft::semantic(fact, AuditInput::new()),
        )?;
        Ok(())
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn append_mapped_contained_task_terminal_for_test(
        &self,
        request: &RuntimeRequest,
        token: &LeaseToken,
        task_id: IssuedTaskId,
        run_id: IssuedRunId,
        final_page: Option<String>,
        game: String,
        declaration: SchedulingOutcomeDeclaration,
        selected_scheduling_outcome: Option<String>,
    ) -> Result<
        TerminalEvent,
        (
            String,
            RuntimeReceiptState,
            RuntimeErrorCode,
            Option<TerminalEvent>,
        ),
    > {
        let shared = self.shared.as_ref().expect("test Runtime host is open");
        let validated = request.validate().expect("test Runtime request is valid");
        shared
            .append_contained_task_terminal(
                &validated,
                token,
                ContainedTaskTerminalDraft {
                    task_id,
                    run_id,
                    outcome: TaskOutcome::Success,
                    intent_already_recorded: false,
                    final_page,
                    executed_steps: Some(0),
                    failure_code: None,
                    failure_severity: None,
                    projection_failure_severity: None,
                    scheduling_outcome: Some((game, declaration)),
                    selected_scheduling_outcome,
                    capture_summary: None,
                    task_timing: None,
                },
            )
            .map(|event| terminal(&event))
            .map_err(|failure| {
                (
                    failure.error.code().to_owned(),
                    failure.state,
                    failure.error.projection().code,
                    failure.terminal,
                )
            })
    }

    #[cfg(test)]
    pub(crate) fn fail_next_scheduling_terminal_append_for_test(&self) -> RuntimeHostResult<()> {
        self.shared_ref("inject_test_scheduling_terminal_append_failure")?
            .scheduling_terminal_append_failures
            .store(1, Ordering::Release);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn fail_next_contained_task_stability_persistence_for_test(
        &self,
    ) -> RuntimeHostResult<()> {
        self.shared_ref("inject_test_contained_task_stability_persistence_failure")?
            .contained_task_stability_persistence_failures
            .store(1, Ordering::Release);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn fail_next_contained_task_stability_event_append_for_test(
        &self,
    ) -> RuntimeHostResult<()> {
        self.shared_ref("inject_test_contained_task_stability_event_append_failure")?
            .contained_task_stability_persistence_failures
            .store(2, Ordering::Release);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn fail_next_contained_task_ocr_failure_persistence_for_test(
        &self,
    ) -> RuntimeHostResult<()> {
        self.shared_ref("inject_test_contained_task_ocr_failure_persistence")?
            .contained_task_ocr_failure_persistence_failures
            .store(1, Ordering::Release);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn fail_next_policy_outcome_projection_for_test(&self) -> RuntimeHostResult<()> {
        self.shared_ref("inject_test_policy_outcome_projection_failure")?
            .policy_outcome_projection_failures
            .store(1, Ordering::Release);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn query_persisted_events_for_test(
        &self,
        query: EventQuery,
    ) -> RuntimeHostResult<Vec<PersistedEvent>> {
        self.shared_ref("query_test_persisted_events")?
            .ledger
            .query(query)
            .map_err(|_| ledger_error("query_test_persisted_events"))
    }

    #[cfg(test)]
    pub(crate) fn override_next_policy_outcome_projection_position_for_test(
        &self,
        ledger_position: u64,
    ) -> RuntimeHostResult<()> {
        self.shared_ref("inject_test_policy_outcome_projection_position")?
            .policy_outcome_projection_position_override
            .store(ledger_position, Ordering::Release);
        Ok(())
    }

    pub fn close(mut self) -> RuntimeHostResult<()> {
        self.shutdown()
    }

    pub fn record_lifecycle_failure(
        &self,
        stage: RuntimeLifecycleFailureStage,
        failure: RuntimeLifecycleFailure<'_>,
    ) -> RuntimeHostResult<()> {
        self.shared_ref("record_runtime_lifecycle_failure")?
            .append_lifecycle_failure(stage, failure, EventLinksDraft::default(), None)
    }

    fn shared_ref(&self, operation: &'static str) -> RuntimeHostResult<&HostShared> {
        self.shared.as_deref().ok_or_else(|| {
            RuntimeHostError::fatal(
                "runtime_host_closed",
                operation,
                RuntimeErrorCode::RuntimeUnavailable,
            )
        })
    }

    fn work_ref(&self, operation: &'static str) -> RuntimeHostResult<HostWork<'_>> {
        let shared = self.shared_ref(operation)?;
        let guard = shared.begin_work()?.ok_or_else(|| {
            RuntimeHostError::request(
                "runtime_stopping",
                operation,
                RuntimeErrorCode::RuntimeUnavailable,
            )
        })?;
        Ok(HostWork {
            shared,
            _guard: guard,
        })
    }

    fn shutdown(&mut self) -> RuntimeHostResult<()> {
        let Some(shared) = self.shared.take() else {
            return Ok(());
        };
        shared.fatal.request_shutdown();
        let mut failure = match shared.fatal.current() {
            Ok(failure) => failure,
            Err(error) => Some(error),
        };
        if failure
            .as_ref()
            .is_some_and(|error| error.projection().code == RuntimeErrorCode::LedgerFailure)
        {
            shared
                .lifecycle_append_failed
                .store(true, Ordering::Release);
        } else if !shared.lifecycle_append_failed.load(Ordering::Acquire) {
            record_failure(
                &mut failure,
                shared
                    .append_lifecycle_observed(
                        RuntimeLifecyclePhase::ShutdownRequested,
                        EventLinksDraft::default(),
                    )
                    .map(|_| ())
                    .map_err(|error| {
                        error.with_failure_stage("runtime.lifecycle.shutdown_requested")
                    }),
            );
        }
        shared.record_lifecycle_result(
            RuntimeLifecycleFailureStage::ShutdownJoin,
            &mut failure,
            join_runtime_thread(self.accept_thread.take(), "join_runtime_accept"),
        );
        shared.record_lifecycle_result(
            RuntimeLifecycleFailureStage::ShutdownJoin,
            &mut failure,
            join_runtime_thread(self.sweep_thread.take(), "join_runtime_sweeper"),
        );
        shared.record_lifecycle_result(
            RuntimeLifecycleFailureStage::ShutdownJoin,
            &mut failure,
            join_runtime_thread(self.monitor_thread.take(), "join_runtime_monitor"),
        );
        shared.record_lifecycle_result(
            RuntimeLifecycleFailureStage::ShutdownJoin,
            &mut failure,
            join_runtime_thread(self.startup_thread.take(), "join_runtime_startup"),
        );
        shared.record_lifecycle_result(
            RuntimeLifecycleFailureStage::ShutdownJoin,
            &mut failure,
            join_runtime_thread(self.performance_thread.take(), "join_runtime_performance"),
        );
        if let Err(error) = fs::remove_file(&self.info_path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            shared.record_lifecycle_result(
                RuntimeLifecycleFailureStage::InfoFileRemoval,
                &mut failure,
                Err(RuntimeHostError::fatal(
                    "runtime_info_remove_failed",
                    "close_runtime_host",
                    RuntimeErrorCode::RuntimeFatal,
                )
                .with_native_detail(error.to_string())),
            );
        }
        match Arc::try_unwrap(shared) {
            Ok(shared) => device_diagnostic::record_host_close_result(&mut failure, shared.close()),
            Err(shared) => {
                shared.record_lifecycle_result(
                    RuntimeLifecycleFailureStage::RetainedReference,
                    &mut failure,
                    Err(RuntimeHostError::fatal(
                        "runtime_reference_leaked",
                        "close_runtime_host",
                        RuntimeErrorCode::RuntimeFatal,
                    )),
                );
                shared.finish_device_diagnostics(&mut failure);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

impl Drop for RuntimeHost {
    fn drop(&mut self) {
        if self.shared.is_none() || thread::panicking() {
            return;
        }
        if let Err(error) = self.shutdown() {
            panic!("{error}");
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct RegisteredInstance {
    instance_alias: String,
    instance_id: InstanceId,
    audit_endpoint: String,
    provenance: ExecutionBackendProvenance,
    /// Mirrors the registry: emulator control (re)binds a discovery-bound instance and
    /// refreshes this copy together with `audit_endpoint` under the registry lock.
    adb_endpoint: Option<ResolvedInstanceEndpoint>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QueueOperationTestKind {
    Poll,
    Cancel,
}

#[cfg(test)]
struct QueueOperationTestHook {
    operation: QueueOperationTestKind,
    request_id: RequestId,
    snapshot_reached: Arc<Barrier>,
    resume: Arc<Barrier>,
}

#[cfg(test)]
struct PolicyOutcomeTransitionTestHook {
    completion_committed: Arc<Barrier>,
    resume: Arc<Barrier>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ScheduledPolicyCheckpointIdentity {
    run_id: RunId,
    task_id: TaskId,
    correlation_id: CorrelationId,
    lease_id: LeaseId,
}

#[cfg(test)]
impl ScheduledPolicyCheckpointIdentity {
    pub(crate) fn for_context(context: &PolicyRunContext) -> Self {
        Self {
            run_id: context.run_id(),
            task_id: context.task_id(),
            correlation_id: context.correlation_id(),
            lease_id: context.lease_token().lease_id(),
        }
    }

    pub(crate) fn with_run_id(mut self, run_id: RunId) -> Self {
        self.run_id = run_id;
        self
    }

    pub(crate) fn with_task_id(mut self, task_id: TaskId) -> Self {
        self.task_id = task_id;
        self
    }

    pub(crate) fn with_correlation_id(mut self, correlation_id: CorrelationId) -> Self {
        self.correlation_id = correlation_id;
        self
    }

    pub(crate) fn with_lease_id(mut self, lease_id: LeaseId) -> Self {
        self.lease_id = lease_id;
        self
    }
}

#[cfg(test)]
enum ScheduledPolicyCheckpointTestAction {
    Count(Arc<AtomicU64>),
    Exit { marker: PathBuf },
}

#[cfg(test)]
struct ScheduledPolicyCheckpointTestHook {
    identity: ScheduledPolicyCheckpointIdentity,
    execution_thread: std::thread::ThreadId,
    action: ScheduledPolicyCheckpointTestAction,
}

#[cfg(test)]
pub(crate) struct ScheduledPolicyCheckpointTestControl {
    consumed: Arc<AtomicU64>,
}

#[cfg(test)]
impl ScheduledPolicyCheckpointTestControl {
    pub(crate) fn consumed(&self) -> u64 {
        self.consumed.load(Ordering::Acquire)
    }
}

#[cfg(test)]
pub(crate) struct PolicyOutcomeTransitionTestControl {
    completion_committed: Arc<Barrier>,
    resume: Arc<Barrier>,
}

#[cfg(test)]
impl PolicyOutcomeTransitionTestControl {
    pub(crate) fn wait_until_completion_committed(&self) {
        self.completion_committed.wait();
    }

    pub(crate) fn resume(self) {
        self.resume.wait();
    }
}

#[cfg(test)]
pub(crate) struct QueueOperationTestControl {
    snapshot_reached: Arc<Barrier>,
    resume: Arc<Barrier>,
}

#[cfg(test)]
impl QueueOperationTestControl {
    pub(crate) fn wait_until_paused(&self) {
        self.snapshot_reached.wait();
    }

    pub(crate) fn resume(self) {
        self.resume.wait();
    }
}

impl RegisteredInstance {
    const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn audit_endpoint(&self) -> &str {
        &self.audit_endpoint
    }

    const fn provenance(&self) -> ExecutionBackendProvenance {
        self.provenance
    }

    /// The bound HOST:PORT target, absent while a discovery binding is pending.
    fn bound_adb_endpoint(&self) -> Option<&ResolvedAdbEndpoint> {
        self.adb_endpoint
            .as_ref()
            .and_then(ResolvedInstanceEndpoint::bound)
    }

    fn endpoint_pending(&self) -> bool {
        self.adb_endpoint
            .as_ref()
            .is_some_and(ResolvedInstanceEndpoint::is_pending)
    }
}

/// Reads the provider's registered set as it is. Alias validity and uniqueness are the
/// registry's own admission rules (`ExecutionBackendRegistry::from_assembly`); nothing is
/// re-validated here.
fn initial_registered_instances(
    provider: &dyn ExecutionBackendProvider,
) -> RuntimeHostResult<BTreeMap<InstanceId, RegisteredInstance>> {
    let mut instances = BTreeMap::new();
    for instance_alias in provider.instance_aliases() {
        let resolved = provider.resolve(&instance_alias).ok_or_else(|| {
            RuntimeHostError::fatal(
                "execution_backend_registry_incomplete",
                "initialize_runtime_instance_registry",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        let registration = RegisteredInstance {
            instance_alias,
            instance_id: resolved.instance_id(),
            audit_endpoint: resolved.audit_endpoint().to_string(),
            provenance: resolved.provenance(),
            adb_endpoint: resolved.adb_endpoint().cloned(),
        };
        if instances
            .insert(registration.instance_id, registration)
            .is_some()
        {
            return Err(RuntimeHostError::fatal(
                "duplicate_runtime_instance_id",
                "initialize_runtime_instance_registry",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
    }
    Ok(instances)
}

/// A scoped Runtime-owned policy admission; dropping it does not stop or close the host.
pub struct RuntimePolicyWork<'a> {
    _guard: RwLockReadGuard<'a, bool>,
}

struct HostWork<'a> {
    shared: &'a HostShared,
    _guard: RwLockReadGuard<'a, bool>,
}

impl std::ops::Deref for HostWork<'_> {
    type Target = HostShared;

    fn deref(&self) -> &Self::Target {
        self.shared
    }
}

struct HostShared {
    owner_epoch: actingcommand_contract::OwnerEpoch,
    shutdown_target: actingcommand_contract::RuntimeShutdownTarget,
    // Concurrent work holds the read side; idle shutdown never waits for a busy writer slot.
    lifecycle_admission: RwLock<bool>,
    scheduler: Arc<Mutex<SeedScheduler>>,
    policy: Mutex<PolicyHost>,
    performance: Mutex<PerformanceMonitor>,
    performance_control: Mutex<PerformanceBalanceController>,
    frame_retention: Mutex<Option<frame_retention::FrameRetention>>,
    // Client facts and approval authority are projected and appended as one ordered transition.
    governance_write_gate: Mutex<()>,
    governance_capability_sha256: Option<[u8; 32]>,
    governance_connections: Mutex<BTreeSet<ConnectionId>>,
    // Ledger append and fact projection commit are one ordered Runtime-owned transition.
    fact_write_gate: Mutex<()>,
    // Serialize explicit catalog transitions; the catalog itself is rebuilt by Ledger.
    signature_write_gate: Mutex<()>,
    device_diagnostics: Mutex<device_diagnostic::DeviceDiagnosticBudget>,
    lifecycle_append_failed: AtomicBool,
    // Detection quota preview, ledger append, and replay-state commit are one ordered transition.
    detection_write_gate: Mutex<()>,
    // State and release pointer changes are serialized with their ledger facts.
    state_write_gate: Mutex<()>,
    // Agent session transitions are ledger-first and serialized across IPC and timeout sweeps.
    agent_write_gate: Mutex<()>,
    // Proposal recompilation, approval checks, and catalog activation form one ordered gate.
    proposal_write_gate: Mutex<()>,
    facts: Mutex<InstanceFactStore>,
    // The Runtime's own facts: ledger-first, memory-only, sealed periodically while dirty.
    runtime_facts: Mutex<RuntimeFactStore>,
    runtime_facts_dirty: AtomicBool,
    policy_inputs: Mutex<Option<PolicyInputSnapshot>>,
    // A bounded cache of exact GlobalLedger projections; it never computes or owns outcomes.
    authoritative_policy_outcomes:
        Mutex<BTreeMap<(String, String), AuthoritativeSchedulingOutcome>>,
    procedure_manifest: Mutex<Option<ProcedureManifest>>,
    // Whole verified material objects, so chunked reads verify each object once.
    verified_materials: Mutex<material_read::VerifiedMaterialCache>,
    ledger: GlobalLedger,
    artifacts: Arc<ArtifactStore>,
    state: Arc<RuntimeStateStore>,
    agent_dispatcher_config: Option<AgentDispatcherConfig>,
    agent_dispatcher: Mutex<AgentDispatcherState>,
    owner: Mutex<OwnerGuard>,
    events: RuntimeEvents,
    execution: ExecutionKernel,
    registered_instances: Mutex<BTreeMap<InstanceId, RegisteredInstance>>,
    monitor_registry: Mutex<MonitorRegistry>,
    queued_requests: Mutex<BTreeMap<RequestId, QueuedRequestContext>>,
    queue_terminals: Mutex<QueueTerminalStore>,
    #[cfg(test)]
    queue_operation_test_hook: Mutex<Option<QueueOperationTestHook>>,
    #[cfg(test)]
    policy_outcome_transition_test_hook: Mutex<Option<PolicyOutcomeTransitionTestHook>>,
    #[cfg(test)]
    scheduled_policy_checkpoint_test_hook: Mutex<Option<ScheduledPolicyCheckpointTestHook>>,
    #[cfg(test)]
    contained_task_checkpoint_test_hook: Mutex<Option<ContainedTaskCheckpointTestHook>>,
    #[cfg(test)]
    lease_expiry_scan_test_gate: Mutex<()>,
    #[cfg(test)]
    lease_expiry_test_checkpoints: Mutex<Vec<LeaseExpiryTestCheckpoint>>,
    trusted_policy_dispatches: Mutex<TrustedPolicyDispatchStore>,
    policy_dispatch_clocks: Mutex<BTreeMap<String, PolicyDispatchClock>>,
    // Outcome preparation and completion form one idempotent Runtime-owned transition.
    policy_outcome_gate: Mutex<()>,
    admission_guards: Mutex<BTreeMap<InstanceId, Arc<Mutex<()>>>>,
    debug_runs: Mutex<BTreeMap<CorrelationId, DebugRunContext>>,
    contained_runs: Mutex<BTreeMap<RequestId, Arc<ContainedRunControl>>>,
    // Slice #316-B3: startup packages by registered instance, and the work handed to the
    // host's own scheduling thread (startup packages; since #316-B4 also recovery ladders).
    startup_packages: BTreeMap<InstanceId, ContainedTaskRequest>,
    pending_host_work: Mutex<VecDeque<startup_package::PendingHostWork>>,
    // Slice #324-r1: the admitted default resource package by instance alias (status only).
    resource_packages: BTreeMap<String, actingcommand_contract::InstanceResourcePackage>,
    // Slice #316-B4: stuck-recovery settings by registered instance (absent = defaults), the
    // per-instance ladder window, and direct-run triggers waiting for their receipt write.
    stuck_recovery: BTreeMap<InstanceId, actingcommand_contract::InstanceStuckRecovery>,
    recovery_ladders: Mutex<BTreeMap<InstanceId, recovery_ladder::RecoveryLadderWindow>>,
    parked_recovery_ladders: Mutex<BTreeMap<RequestId, recovery_ladder::PendingRecoveryLadder>>,
    #[cfg(test)]
    scheduling_terminal_append_failures: AtomicU64,
    #[cfg(test)]
    contained_task_stability_persistence_failures: AtomicU64,
    #[cfg(test)]
    contained_task_ocr_failure_persistence_failures: AtomicU64,
    #[cfg(test)]
    policy_outcome_projection_failures: AtomicU64,
    #[cfg(test)]
    policy_outcome_projection_position_override: AtomicU64,
    next_connection_id: AtomicU64,
    clock: Arc<dyn RuntimeClock>,
    clock_origin_monotonic_ms: u64,
    fatal: FatalState,
}

#[derive(Debug, Clone, Copy)]
struct RuntimeRunLinks {
    task_id: IssuedTaskId,
    run_id: IssuedRunId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PolicyDispatchClock {
    admitted_at_unix_ms: u64,
    started_at_monotonic_ms: Option<u64>,
}

impl PolicyDispatchClock {
    const fn live(admitted_at_unix_ms: u64, started_at_monotonic_ms: u64) -> Self {
        Self {
            admitted_at_unix_ms,
            started_at_monotonic_ms: Some(started_at_monotonic_ms),
        }
    }

    const fn recovered(admitted_at_unix_ms: u64) -> Self {
        Self {
            admitted_at_unix_ms,
            started_at_monotonic_ms: None,
        }
    }
}

#[derive(Clone)]
struct CompletedEvidenceExport {
    request_output_path: String,
    task_outcome: TaskOutcome,
    response_terminal: TerminalEvent,
    summary: RuntimeEvidenceExportSummary,
}

#[derive(Clone)]
struct DebugRunContext {
    package: EvidencePackage,
    package_summary: PackageDebugSummary,
    run_id: IssuedRunId,
    task_id: IssuedTaskId,
    terminal_outcome: Option<TaskOutcome>,
    completed_export: Option<CompletedEvidenceExport>,
}

struct OperationSuccess {
    state: RuntimeReceiptState,
    terminal: Option<TerminalEvent>,
    result: RuntimeResult,
}

impl HostShared {
    fn work_guard(&self) -> RuntimeHostResult<RwLockReadGuard<'_, bool>> {
        self.lifecycle_admission.read().map_err(|_| {
            RuntimeHostError::fatal(
                "runtime_lifecycle_admission_poisoned",
                "admit_runtime_work",
                RuntimeErrorCode::RuntimeFatal,
            )
        })
    }

    fn begin_work(&self) -> RuntimeHostResult<Option<RwLockReadGuard<'_, bool>>> {
        let guard = self.work_guard()?;
        if *guard {
            return Ok(None);
        }
        Ok(Some(guard))
    }

    fn request_shutdown(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        target: actingcommand_contract::RuntimeShutdownTarget,
    ) -> Result<OperationSuccess, RequestFailure> {
        use actingcommand_contract::RuntimeShutdownDecision;
        let admission = match self.lifecycle_admission.try_write() {
            Ok(guard) => Some(guard),
            Err(TryLockError::WouldBlock) => None,
            Err(TryLockError::Poisoned(_)) => {
                return Err(RequestFailure::poison_without_terminal(
                    RuntimeHostError::fatal(
                        "runtime_lifecycle_admission_poisoned",
                        "request_runtime_shutdown",
                        RuntimeErrorCode::RuntimeFatal,
                    ),
                ));
            }
        };
        if let Some(error) = self
            .fatal
            .current()
            .map_err(RequestFailure::poison_without_terminal)?
        {
            return Err(RequestFailure::poison_without_terminal(error));
        }
        let decision = if target != self.shutdown_target {
            RuntimeShutdownDecision::OwnerMismatch
        } else if self.fatal.is_shutdown_requested() {
            RuntimeShutdownDecision::AlreadyStopping
        } else if admission.is_none() {
            RuntimeShutdownDecision::Busy
        } else {
            let scheduler = lock(&self.scheduler, "check_shutdown_leases")
                .map_err(RequestFailure::poison_without_terminal)?;
            let queued = lock(&self.queued_requests, "check_shutdown_queue")
                .map_err(RequestFailure::poison_without_terminal)?;
            if !scheduler.active_tokens().is_empty() || !queued.is_empty() {
                RuntimeShutdownDecision::Busy
            } else {
                RuntimeShutdownDecision::Accepted
            }
        };
        let event = self.append_event(
            if decision == RuntimeShutdownDecision::Accepted {
                EventSeverity::Info
            } else {
                EventSeverity::Warning
            },
            request.source(),
            OriginModule::Runtime,
            request.actor(),
            request.event_links(None, None, None),
            RuntimePayloadDraft::lifecycle_observed(
                self.owner_epoch,
                RuntimeLifecyclePhase::ShutdownRequest { target, decision },
                AuditInput::new(),
            ),
        )?;
        match decision {
            RuntimeShutdownDecision::Accepted => {
                // Persist the exact target before closing admission. The daemon owns close/join.
                let mut admission = admission.expect("acceptance requires exclusive admission");
                *admission = true;
                self.fatal.request_shutdown();
                Ok(OperationSuccess {
                    state: RuntimeReceiptState::Admitted,
                    terminal: Some(terminal(&event)),
                    result: RuntimeResult::ShutdownAccepted { target },
                })
            }
            denied => Err(RequestFailure::request(
                RuntimeHostError::request(
                    "runtime_shutdown_denied",
                    "request_runtime_shutdown",
                    match denied {
                        RuntimeShutdownDecision::Busy => RuntimeErrorCode::RuntimeBusy,
                        RuntimeShutdownDecision::OwnerMismatch => {
                            RuntimeErrorCode::RuntimeOwnerMismatch
                        }
                        RuntimeShutdownDecision::AlreadyStopping => {
                            RuntimeErrorCode::RuntimeUnavailable
                        }
                        RuntimeShutdownDecision::Accepted => unreachable!(),
                    },
                ),
                RuntimeReceiptState::Denied,
                Some(terminal(&event)),
            )),
        }
    }

    #[cfg(test)]
    fn consume_scheduled_policy_checkpoint_for_test(
        &self,
        context: Option<&PolicyRunContext>,
    ) -> RuntimeHostResult<()> {
        let Some(context) = context else {
            return Ok(());
        };
        let identity = ScheduledPolicyCheckpointIdentity::for_context(context);
        let hook = {
            let mut slot = lock(
                &self.scheduled_policy_checkpoint_test_hook,
                "consume_scheduled_policy_checkpoint_test_hook",
            )?;
            slot.as_ref()
                .is_some_and(|hook| {
                    hook.identity == identity && hook.execution_thread == thread::current().id()
                })
                .then(|| slot.take())
                .flatten()
        };
        let Some(hook) = hook else {
            return Ok(());
        };
        match hook.action {
            ScheduledPolicyCheckpointTestAction::Count(consumed) => {
                consumed.fetch_add(1, Ordering::AcqRel);
                Ok(())
            }
            ScheduledPolicyCheckpointTestAction::Exit { marker } => {
                fs::write(
                    marker,
                    b"after-durable-lease-release-before-policy-execution-recorded",
                )
                .map_err(|_| {
                    RuntimeHostError::fatal(
                        "scheduled_policy_checkpoint_marker_write_failed",
                        "consume_scheduled_policy_checkpoint_test_hook",
                        RuntimeErrorCode::RuntimeFatal,
                    )
                })?;
                std::process::exit(87);
            }
        }
    }

    fn close(self) -> RuntimeHostResult<()> {
        let mut failure = None;
        let tokens = match lock(&self.scheduler, "list_runtime_leases") {
            Ok(scheduler) => scheduler.active_tokens(),
            Err(error) => {
                self.record_lifecycle_result(
                    RuntimeLifecycleFailureStage::HostClose,
                    &mut failure,
                    Err(error),
                );
                Vec::new()
            }
        };
        for token in tokens {
            let connection_id =
                lock(&self.scheduler, "read_lease_connection").and_then(|scheduler| {
                    scheduler.connection_for_token(&token).map_err(|error| {
                        RuntimeHostError::scheduler("read_lease_connection", &error)
                    })
                });
            match connection_id {
                Ok(connection_id) => self.record_lifecycle_result(
                    RuntimeLifecycleFailureStage::ConnectionCleanup,
                    &mut failure,
                    self.cleanup_token(&token, connection_id, LeaseReleaseReason::HostShutdown),
                ),
                Err(error) => self.record_lifecycle_result(
                    RuntimeLifecycleFailureStage::ConnectionCleanup,
                    &mut failure,
                    Err(error),
                ),
            }
        }
        match self.execution.owned_instance_ids() {
            Ok(instances) => {
                for instance_id in instances {
                    let result = (|| {
                        // Cached Unconfirmed outcomes are reduced below without another close attempt.
                        if !self.execution.has_session(instance_id).map_err(|error| {
                            RuntimeHostError::execution("inspect_retained_session", &error)
                        })? {
                            return Ok(());
                        }
                        let instance_guard = self
                            .instance_guard(instance_id)
                            .map_err(|failure| *failure.error)?;
                        let admission = lock(&instance_guard, "lock_instance_admission")?;
                        self.close_retained_instance_while_guarded(
                            instance_id,
                            EventLinksDraft::default(),
                            false,
                            &admission,
                        )?
                        .map_err(|error| {
                            RuntimeHostError::execution("close_execution_session", &error)
                        })
                    })();
                    self.record_lifecycle_result(
                        RuntimeLifecycleFailureStage::SessionClose,
                        &mut failure,
                        result,
                    );
                }
            }
            Err(error) => self.record_lifecycle_result(
                RuntimeLifecycleFailureStage::SessionClose,
                &mut failure,
                Err(RuntimeHostError::execution(
                    "list_retained_sessions",
                    &error,
                )),
            ),
        }
        if let Err(mut error) = self.execution.close_after_resource_retirement() {
            let aggregate_error = RuntimeHostError::execution("close_execution_kernel", &error);
            let unconfirmed = error.resource_quiescence() == Some(ResourceQuiescence::Unconfirmed);
            let closed_sessions = error.take_closed_sessions();
            if closed_sessions.is_empty() {
                self.record_lifecycle_result(
                    RuntimeLifecycleFailureStage::SessionClose,
                    &mut failure,
                    Err(RuntimeHostError::execution(
                        "close_execution_kernel",
                        &error,
                    )),
                );
            } else {
                for (instance_id, session_error) in closed_sessions {
                    let mut session_error =
                        RuntimeHostError::execution("close_execution_kernel", &session_error);
                    session_error.lifecycle.instance_id = Some(instance_id);
                    self.record_lifecycle_result(
                        RuntimeLifecycleFailureStage::SessionClose,
                        &mut failure,
                        Err(session_error),
                    );
                }
                // Keep the kernel reduction after the original per-session errors.
                record_failure(
                    &mut failure,
                    Err(
                        RuntimeHostError::execution("close_execution_kernel", &error)
                            .with_failure_stage(
                                RuntimeLifecycleFailureStage::SessionClose.as_str(),
                            ),
                    ),
                );
            }
            if unconfirmed {
                self.record_lifecycle_result(
                    RuntimeLifecycleFailureStage::SessionClose,
                    &mut failure,
                    lock(&self.owner, "retain_unconfirmed_owner")
                        .and_then(|mut owner| owner.retain_unconfirmed()),
                );
                self.record_lifecycle_result(
                    RuntimeLifecycleFailureStage::HostClose,
                    &mut failure,
                    self.fatal.mark(aggregate_error),
                );
            }
        }
        match self.fatal.current() {
            Ok(Some(error)) => self.record_lifecycle_result(
                RuntimeLifecycleFailureStage::HostClose,
                &mut failure,
                Err(error),
            ),
            Ok(None) => {}
            Err(error) => {
                self.record_lifecycle_result(
                    RuntimeLifecycleFailureStage::HostClose,
                    &mut failure,
                    Err(error),
                );
            }
        }
        self.finish_device_diagnostics(&mut failure);
        let HostShared { owner, ledger, .. } = self;
        if ledger.close().is_err() {
            record_failure(&mut failure, Err(ledger_error("close_global_ledger")));
        }
        match owner.into_inner() {
            Ok(mut owner) => {
                record_failure(&mut failure, unix_ms_now().and_then(|now| owner.close(now)))
            }
            Err(_) => record_failure(&mut failure, Err(lock_poison_error("close_owner_file"))),
        }
        failure.map_or(Ok(()), Err)
    }

    fn monotonic_ms(&self) -> RuntimeHostResult<u64> {
        Ok(self.runtime_clock_sample()?.monotonic_ms)
    }

    fn runtime_clock_sample(&self) -> RuntimeHostResult<RuntimeClockSample> {
        let sample = self.clock.sample()?;
        let monotonic_ms = sample
            .monotonic_ms
            .checked_sub(self.clock_origin_monotonic_ms)
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    "monotonic_clock_regressed",
                    "read_runtime_clock",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
        Ok(RuntimeClockSample {
            unix_ms: sample.unix_ms,
            monotonic_ms,
        })
    }
}

fn accept_loop(
    listener: TcpListener,
    shared: Arc<HostShared>,
    maximum_frame_bytes: usize,
    io_timeout: Duration,
) -> RuntimeHostResult<()> {
    let mut connections = Vec::new();
    let mut failure = None;
    while !shared.fatal.is_shutdown_requested() {
        reap_finished_connections(&mut connections, &shared, &mut failure);
        if shared.fatal.is_shutdown_requested() {
            break;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let connection_serial = shared.next_connection_id.fetch_add(1, Ordering::Relaxed);
                let connection_id = match ConnectionId::new(connection_serial) {
                    Ok(connection_id) => connection_id,
                    Err(error) => {
                        let error =
                            RuntimeHostError::scheduler("accept_runtime_connection", &error);
                        record_failure(&mut failure, shared.fatal.mark(error.clone()));
                        record_failure(&mut failure, Err(error));
                        break;
                    }
                };
                let connection_shared = Arc::clone(&shared);
                #[cfg(feature = "test-observation")]
                let connection_observation_owner =
                    crate::test_observation::current_observation_owner();
                let thread = thread::Builder::new()
                    .name("actingcommand-runtime-client".to_string())
                    .spawn(move || {
                        #[cfg(feature = "test-observation")]
                        let _observation_owner = crate::test_observation::enter_observation_owner(
                            connection_observation_owner,
                        );
                        connection_boundary(
                            stream,
                            connection_shared,
                            connection_id,
                            connection_serial,
                            maximum_frame_bytes,
                            io_timeout,
                        )
                    });
                let thread = match thread {
                    Ok(thread) => thread,
                    Err(_) => {
                        let error = RuntimeHostError::fatal(
                            "runtime_connection_spawn_failed",
                            "accept_runtime_connection",
                            RuntimeErrorCode::RuntimeFatal,
                        );
                        record_failure(&mut failure, shared.fatal.mark(error.clone()));
                        record_failure(&mut failure, Err(error));
                        break;
                    }
                };
                connections.push(thread);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_IDLE_INTERVAL);
            }
            Err(_) => {
                let error = RuntimeHostError::fatal(
                    "runtime_accept_failed",
                    "accept_runtime_connection",
                    RuntimeErrorCode::RuntimeFatal,
                );
                record_failure(&mut failure, shared.fatal.mark(error.clone()));
                record_failure(&mut failure, Err(error));
                break;
            }
        }
    }
    for connection in connections {
        let result = match connection.join() {
            Ok(result) => result,
            Err(_) => {
                shared.record_lifecycle_result(
                    RuntimeLifecycleFailureStage::ShutdownJoin,
                    &mut failure,
                    Err(RuntimeHostError::fatal(
                        "runtime_connection_panicked",
                        "join_runtime_connection",
                        RuntimeErrorCode::RuntimeFatal,
                    )),
                );
                return failure.map_or(Ok(()), Err);
            }
        };
        shared.record_lifecycle_result(
            RuntimeLifecycleFailureStage::ShutdownJoin,
            &mut failure,
            result,
        );
    }
    failure.map_or(Ok(()), Err)
}

fn reap_finished_connections(
    connections: &mut Vec<JoinHandle<RuntimeHostResult<()>>>,
    shared: &HostShared,
    failure: &mut Option<RuntimeHostError>,
) {
    let mut index = 0;
    while index < connections.len() {
        if !connections[index].is_finished() {
            index += 1;
            continue;
        }
        let connection = connections.swap_remove(index);
        let result = connection.join().map_err(|_| {
            RuntimeHostError::fatal(
                "runtime_connection_panicked",
                "join_runtime_connection",
                RuntimeErrorCode::RuntimeFatal,
            )
        });
        let result = result.and_then(|result| result);
        if let Err(error) = &result
            && error.is_fatal()
        {
            shared.record_lifecycle_result(
                RuntimeLifecycleFailureStage::ShutdownJoin,
                failure,
                shared.fatal.mark(error.clone()),
            );
        }
        shared.record_lifecycle_result(RuntimeLifecycleFailureStage::ShutdownJoin, failure, result);
    }
}

fn lease_sweep_loop(shared: Arc<HostShared>) -> RuntimeHostResult<()> {
    while !shared.fatal.is_shutdown_requested() {
        thread::sleep(LEASE_SWEEP_INTERVAL);
        if shared.fatal.is_shutdown_requested() {
            break;
        }
        let Some(_work) = shared.begin_work()? else {
            break;
        };
        if let Err(error) = shared.expire_due_leases() {
            shared.fatal.mark(error.clone())?;
            return Err(error);
        }
        if let Err(error) = shared.expire_agent_sessions() {
            shared.fatal.mark(error.clone())?;
            return Err(error);
        }
    }
    Ok(())
}

fn artifact_store_error(operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(
        "artifact_store_failure",
        operation,
        RuntimeErrorCode::RuntimeFatal,
    )
}

fn publish_runtime_info(path: &Path, info: &RuntimeInfo) -> RuntimeHostResult<()> {
    let parent = path.parent().ok_or_else(|| {
        RuntimeHostError::fatal(
            "runtime_info_parent_missing",
            "publish_runtime_info",
            RuntimeErrorCode::RuntimeFatal,
        )
    })?;
    let temporary = parent.join(format!("{RUNTIME_INFO_FILE}.tmp-{}", std::process::id()));
    remove_temporary_runtime_info(&temporary)?;
    let encoded = serde_json::to_vec_pretty(info).map_err(|_| {
        RuntimeHostError::fatal(
            "runtime_info_encode_failed",
            "publish_runtime_info",
            RuntimeErrorCode::RuntimeFatal,
        )
    })?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|_| {
            RuntimeHostError::fatal(
                "runtime_info_create_failed",
                "publish_runtime_info",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
    let write_result = file
        .write_all(&encoded)
        .and_then(|()| file.sync_all())
        .map_err(|_| {
            RuntimeHostError::fatal(
                "runtime_info_write_failed",
                "publish_runtime_info",
                RuntimeErrorCode::RuntimeFatal,
            )
        });
    drop(file);
    if let Err(error) = write_result {
        remove_temporary_runtime_info(&temporary)?;
        return Err(error);
    }
    if let Err(error) = fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        remove_temporary_runtime_info(&temporary)?;
        return Err(RuntimeHostError::fatal(
            "runtime_info_replace_failed",
            "publish_runtime_info",
            RuntimeErrorCode::RuntimeFatal,
        ));
    }
    if fs::rename(&temporary, path).is_err() {
        remove_temporary_runtime_info(&temporary)?;
        return Err(RuntimeHostError::fatal(
            "runtime_info_publish_failed",
            "publish_runtime_info",
            RuntimeErrorCode::RuntimeFatal,
        ));
    }
    Ok(())
}

fn remove_temporary_runtime_info(path: &Path) -> RuntimeHostResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(RuntimeHostError::fatal(
            "runtime_info_temp_remove_failed",
            "publish_runtime_info",
            RuntimeErrorCode::RuntimeFatal,
        )),
    }
}

fn runtime_identifier_error() -> RuntimeHostError {
    RuntimeHostError::fatal(
        "runtime_identifier_issue_failed",
        "run_contained_task",
        RuntimeErrorCode::RuntimeFatal,
    )
}

const fn terminal_from_projected(event: &actingcommand_contract::ProjectedEvent) -> TerminalEvent {
    TerminalEvent {
        sequence: event.sequence,
        event_id: event.event_id,
    }
}

fn ledger_error(operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal("ledger_failure", operation, RuntimeErrorCode::LedgerFailure)
}

fn lock_poison_error(operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(
        "runtime_state_poisoned",
        operation,
        RuntimeErrorCode::RuntimeFatal,
    )
}

fn lock<'a, T>(
    mutex: &'a Mutex<T>,
    operation: &'static str,
) -> RuntimeHostResult<MutexGuard<'a, T>> {
    mutex.lock().map_err(|_| lock_poison_error(operation))
}

fn audit_endpoint(endpoint: &str) -> AuditInput {
    if endpoint.is_empty() {
        AuditInput::new()
    } else {
        AuditInput::new().with_device_endpoint(endpoint)
    }
}

const fn scheduled_request_transport_origin(
    provenance: ExecutionBackendProvenance,
) -> (EventActor, EventSource) {
    match provenance {
        ExecutionBackendProvenance::PhysicalDevice => (EventActor::Agent, EventSource::Adapter),
        ExecutionBackendProvenance::FixtureSimulation => (EventActor::Lab, EventSource::Lab),
    }
}

fn audit_path(path: &Path) -> AuditInput {
    AuditInput::new().with_machine_path(path.to_string_lossy())
}

fn join_runtime_thread(
    thread: Option<JoinHandle<RuntimeHostResult<()>>>,
    operation: &'static str,
) -> RuntimeHostResult<()> {
    let Some(thread) = thread else {
        return Ok(());
    };
    thread.join().map_err(|_| {
        RuntimeHostError::fatal(
            "runtime_thread_panicked",
            operation,
            RuntimeErrorCode::RuntimeFatal,
        )
    })?
}

fn failed_start_cleanup(
    shared: Arc<HostShared>,
    info_path: &Path,
    sweep_thread: Option<JoinHandle<RuntimeHostResult<()>>>,
    monitor_thread: Option<JoinHandle<RuntimeHostResult<()>>>,
    startup_thread: Option<JoinHandle<RuntimeHostResult<()>>>,
    performance_thread: Option<JoinHandle<RuntimeHostResult<()>>>,
) -> RuntimeHostResult<()> {
    shared.fatal.request_shutdown();
    let mut failure = None;
    shared.record_lifecycle_result(
        RuntimeLifecycleFailureStage::ShutdownJoin,
        &mut failure,
        join_runtime_thread(sweep_thread, "join_runtime_sweeper"),
    );
    shared.record_lifecycle_result(
        RuntimeLifecycleFailureStage::ShutdownJoin,
        &mut failure,
        join_runtime_thread(monitor_thread, "join_runtime_monitor"),
    );
    shared.record_lifecycle_result(
        RuntimeLifecycleFailureStage::ShutdownJoin,
        &mut failure,
        join_runtime_thread(startup_thread, "join_runtime_startup"),
    );
    shared.record_lifecycle_result(
        RuntimeLifecycleFailureStage::ShutdownJoin,
        &mut failure,
        join_runtime_thread(performance_thread, "join_runtime_performance"),
    );
    if let Err(error) = fs::remove_file(info_path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        shared.record_lifecycle_result(
            RuntimeLifecycleFailureStage::InfoFileRemoval,
            &mut failure,
            Err(RuntimeHostError::fatal(
                "runtime_info_remove_failed",
                "abort_runtime_start",
                RuntimeErrorCode::RuntimeFatal,
            )
            .with_native_detail(error.to_string())),
        );
    }
    match Arc::try_unwrap(shared) {
        Ok(shared) => device_diagnostic::record_host_close_result(&mut failure, shared.close()),
        Err(shared) => {
            shared.record_lifecycle_result(
                RuntimeLifecycleFailureStage::RetainedReference,
                &mut failure,
                Err(RuntimeHostError::fatal(
                    "runtime_reference_leaked",
                    "abort_runtime_start",
                    RuntimeErrorCode::RuntimeFatal,
                )),
            );
            shared.finish_device_diagnostics(&mut failure);
        }
    }
    failure.map_or(Ok(()), Err)
}
