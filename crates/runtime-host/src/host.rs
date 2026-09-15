// SPDX-License-Identifier: AGPL-3.0-only

use crate::agent_dispatcher::{
    AgentDispatcherState, AgentResponsePreparation, AgentResumePreparation, AgentSessionPreparation,
};
use crate::approval::ApprovalProjection;
use crate::events::RuntimeEvents;
use crate::fact_store::InstanceFactStore;
use crate::ipc::{DEFAULT_RUNTIME_MAX_FRAME_BYTES, FrameRead, read_frame, write_frame};
use crate::monitor::{DueMonitorProbe, MonitorRegistry, MonitorUpdate};
use crate::owner::{OwnerGuard, OwnerStartup};
use crate::performance::{
    PerformanceMonitor, PerformanceSemanticEvent, PerformanceTick, PipelineEventObservation,
};
use crate::performance_control::{PerformanceBalanceController, PerformanceDispatchGate};
use crate::planning::collect_maintenance_evidence;
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
    EventSource, EventType, FactPayloadDraft, FactRecord, FrameId, InputAction,
    InputExecutionPlanEvent, InputExecutionPlanRecord, InputPayload, InputPayloadDraft,
    InstanceFactContext, InstanceFactSnapshot, InstanceId, IssuedActionId, IssuedFrameId,
    IssuedMonitorProbe, IssuedReadOnlyCaptureCapability, IssuedRecognitionId, IssuedRunId,
    IssuedTaskId, LeaseId, LeasePayloadDraft, LeaseQueuePolicy, LeaseToken,
    MAX_EFFECTIVE_CONFIGURATION_BYTES, MAX_GOVERNANCE_CAPABILITY_BYTES,
    MIN_GOVERNANCE_CAPABILITY_BYTES, MonitorPayloadDraft, MonitorRecoveryCoordinationReason,
    ObservedMicroseconds, OriginModule, OwnerResourceDisposition, PackageDebugLayout,
    PackageDebugRequest, PackageDebugSummary, PerformanceContext, PerformancePayloadDraft,
    PinnedFrameReason, PolicyDispatchEventData, PolicyExecutionEventData, PolicyExecutionOutcome,
    PolicyFailureClass, PolicyPayload, PolicyPayloadDraft, PolicyPlanningSignalEventData,
    PolicyReasonRecord, ProjectDecisionPageRequest, ProjectInterfaceRequest,
    ProjectedArtifactReference, ProjectionPayload, ProposalClass, ProposalPromotion,
    RUNTIME_INFO_FILE, ReadonlyObservation, RecognitionPayloadDraft, RecognitionVerdict,
    ReleasePayload, ReleasePayloadDraft, ReleaseTransitionKind, RequestId, ResourceAuthoringEvent,
    ResourceAuthoringPayloadDraft, ResourceAuthoringPhase, ResourceQuiescence, RetentionClass,
    RunId, RuntimeCaptureBackend, RuntimeContractError, RuntimeControlPlaneStatus,
    RuntimeDebugEvent, RuntimeDebugOperation, RuntimeDebugPhase, RuntimeErrorCode,
    RuntimeErrorProjection, RuntimeEventBatch, RuntimeEventQueryPageRequest,
    RuntimeEvidenceExportRequest, RuntimeEvidenceExportSummary, RuntimeEvidenceScreenshotCounts,
    RuntimeForwardProjectionRequest, RuntimeInfo, RuntimeInstanceStatus, RuntimeLifecyclePhase,
    RuntimeMaintenanceQuery, RuntimeMonitorPolicy, RuntimeOperation, RuntimePayloadDraft,
    RuntimePlanningDocument, RuntimePlanningDocumentKind, RuntimePolicyInputIdentity,
    RuntimeReceipt, RuntimeReceiptState, RuntimeReleaseSet, RuntimeRequest, RuntimeResult,
    RuntimeStrategicPlanResult, RuntimeSubscriptionRequest, SchedulerPayloadDraft,
    SchedulingDisposition, SchedulingEffectCondition, SchedulingEffectEvidence,
    SchedulingOutcomeDeclaration, SchedulingOutcomeIdentity, SchedulingOutcomeProjection,
    Sensitivity, StatePayload, StatePayloadDraft, TaskEntryRecognitionPhase,
    TaskEntryTargetDisposition, TaskId, TaskOutcome, TaskPayload, TaskPayloadDraft,
    TaskSemanticFact, TaskTimingBoundary, TaskTimingObservationState, TaskTimingResult,
    TerminalEvent, TimingObservationIssue, ValidatedRuntimeRequest,
};
use actingcommand_device::{CaptureBackendName, DeviceCloseAuthority, Frame, SegmentedSwipeEvent};
use actingcommand_execution_kernel::ExecutionKernelError;
use actingcommand_execution_kernel::{
    ContainedTaskEvaluationTiming, ContainedTaskOutcome, ContainedTaskRunError,
    ContainedTaskRuntime, ContainedTaskRuntimeErrorClass, ContainedTaskTimingContext,
    ContainedTaskTrace, ExecutionBackendProvenance, ExecutionBackendProvider, ExecutionKernel,
    ExternalExpectedSha256, PostAdmissionOcrObservation, PreparedContainedTask,
    PreparedInputAction, RecognitionVisionProvider, StabilityComparisonResult,
    StabilityTerminalReason, StabilityTerminationDeclaration, decide_monitor, page_anchor_matches,
};
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
    StrategicEvidencePointer, StrategicProjection, StrategicReport, assess_predictive_maintenance,
    project_forward, project_strategic_report,
};
use actingcommand_runtime_state::{ReleaseArtifactSources, RuntimeStateStore};
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
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::panic::{AssertUnwindSafe, catch_unwind};
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

mod agent_control;
mod client_events;
mod contained_task;
mod device_diagnostic;
mod evidence_export;
mod facts;
mod frame_retention;
mod governance;
mod input;
mod lab_operation;
mod lease;
mod lifecycle;
mod monitor_control;
mod observation;
mod online_observation;
mod package_debug;
mod performance;
mod planning;
mod policy_catalog;
mod policy_outcome;
mod read_events;
mod requests;
mod saved_artifact_ocr;
mod signatures;
mod state_control;
mod task_diagnostic;
mod task_timing;

use agent_control::append_agent_wake;
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
use lease::{QueueTerminalStore, QueuedRequestContext};
use lifecycle::{append_runtime_start_event, record_failure};
use monitor_control::monitor_probe_loop;
use observation::CompletedReadonlyObservation;
use performance::{CapacityUse, performance_monitor_loop};
use planning::planning_request_failure;
#[cfg(test)]
pub(crate) use policy_outcome::insert_authoritative_policy_outcome;
#[cfg(test)]
use policy_outcome::{PolicyOutcomeCacheUpdate, validate_policy_run_admission_request};
use policy_outcome::{
    completed_run_matches_outcome, reconcile_policy_dispatches,
    recover_authoritative_policy_outcomes, validate_completed_run_admission_request,
};

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
    Client {
        code: &'static str,
        operation: &'static str,
        fatal: bool,
        runtime_code: Option<RuntimeErrorCode>,
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

fn validate_static_fact_pool_authority(
    catalog: &actingcommand_policy::CompiledCatalog,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
    operation: &'static str,
) -> RuntimeHostResult<()> {
    for pool in &catalog.catalog().pools.pools {
        if pool.value_source.is_static() {
            continue;
        }
        let actingcommand_policy::ObservationRef::Fact { fact_key } = &pool.observation else {
            return Err(policy_admission_request(
                "policy_pool_binding_invalid",
                operation,
            ));
        };
        if resources.pools.iter().any(|value| value.pool_id == pool.id)
            || facts
                .facts
                .iter()
                .any(|fact| fact.scope == pool.scope && fact.fact_key == *fact_key)
        {
            return Err(policy_admission_request(
                "policy_pool_authority_conflict",
                operation,
            ));
        }
    }
    Ok(())
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
    performance_control: PerformanceControlConfig,
    agent_dispatcher: Option<AgentDispatcherConfig>,
    secret_fingerprint_salt: Vec<u8>,
    governance_capability_sha256: Option<[u8; 32]>,
    governance_capability_invalid: bool,
    clock: Arc<dyn RuntimeClock>,
    policy_inputs: Option<PolicyInputSnapshot>,
    procedure_manifest: Option<ProcedureManifest>,
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
            frame_retention_enabled: false,
            performance_control: PerformanceControlConfig::default(),
            agent_dispatcher: None,
            secret_fingerprint_salt: secret_fingerprint_salt.as_ref().to_vec(),
            governance_capability_sha256: None,
            governance_capability_invalid: false,
            clock: Arc::new(SystemRuntimeClock::new()),
            policy_inputs: None,
            procedure_manifest: None,
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

    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    fn validate(&self) -> RuntimeHostResult<()> {
        self.scheduler
            .validate()
            .map_err(|error| RuntimeHostError::scheduler("validate_runtime_config", &error))?;
        self.policy_cadence.validate()?;
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
                        true,
                        EventLinksDraft::default(),
                    ) {
                        return Err(device_diagnostic::summary_incomplete(
                            Some(original),
                            &error,
                        ));
                    }
                    return Err(original);
                }
            };
        let fatal = FatalState::default();
        let shared = Arc::new(HostShared {
            owner_epoch,
            shutdown_target: info.shutdown_target(),
            lifecycle_admission: RwLock::new(false),
            scheduler: Mutex::new(scheduler),
            policy: Mutex::new(policy),
            performance: Mutex::new(performance),
            performance_control: Mutex::new(performance_control),
            frame_retention: Mutex::new(
                config
                    .frame_retention_enabled
                    .then(frame_retention::FrameRetention::default),
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
            policy_inputs: Mutex::new(config.policy_inputs),
            authoritative_policy_outcomes: Mutex::new(authoritative_policy_outcomes),
            procedure_manifest: Mutex::new(config.procedure_manifest),
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
            failed_start_cleanup(shared, &info_path, None, None, None)?;
            return Err(original);
        }
        if let Err(original) = shared.expire_agent_sessions() {
            failed_start_cleanup(shared, &info_path, None, None, None)?;
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
                failed_start_cleanup(shared, &info_path, None, None, None)?;
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
                failed_start_cleanup(shared, &info_path, Some(sweep_thread), None, None)?;
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

    pub fn admit_policy_dispatch(
        &self,
        intent: &DispatchIntent,
        reason_chain: &DecisionReasonChain,
        context: &PolicyAdmissionContext,
    ) -> RuntimeHostResult<PolicyDispatchAdmission> {
        self.work_ref("admit_policy_dispatch")?
            .admit_policy_dispatch(intent, reason_chain, context, None)
    }

    /// Admits a contained policy run using its bounded request budget for the lease.
    pub fn admit_scheduled_policy_dispatch(
        &self,
        intent: &DispatchIntent,
        reason_chain: &DecisionReasonChain,
        context: &PolicyAdmissionContext,
        task_request: &ContainedTaskRequest,
    ) -> RuntimeHostResult<PolicyDispatchAdmission> {
        self.work_ref("admit_policy_dispatch")?
            .admit_policy_dispatch(intent, reason_chain, context, Some(task_request))
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
        record_failure(
            &mut failure,
            shared
                .append_lifecycle_observed(
                    RuntimeLifecyclePhase::ShutdownRequested,
                    EventLinksDraft::default(),
                )
                .map(|_| ()),
        );
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
#[derive(Clone)]
struct LeaseExpiryTestCheckpoint {
    token: LeaseToken,
    terminal: TerminalEvent,
}

#[cfg(test)]
fn lease_token_identity_match_count(left: &LeaseToken, right: &LeaseToken) -> usize {
    [
        left.owner_epoch() == right.owner_epoch(),
        left.lease_id() == right.lease_id(),
        left.instance_id() == right.instance_id(),
        left.holder_id() == right.holder_id(),
        left.expires_at_monotonic_ms() == right.expires_at_monotonic_ms(),
    ]
    .into_iter()
    .filter(|matches| *matches)
    .count()
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

#[derive(Clone)]
struct TrustedPolicyDispatch {
    intent: DispatchIntent,
    reason_chain: DecisionReasonChain,
    observed_monotonic_ms: u64,
}

#[derive(Default)]
struct TrustedPolicyDispatchStore {
    entries: BTreeMap<String, TrustedPolicyDispatch>,
    order: VecDeque<String>,
}

impl TrustedPolicyDispatchStore {
    fn record_cycle(
        &mut self,
        cycle: &PolicyCycle,
        observed_monotonic_ms: u64,
    ) -> RuntimeHostResult<()> {
        let Some(evaluation) = &cycle.evaluation else {
            return Ok(());
        };
        for intent in &cycle.pending_dispatch_intents {
            let reason_chain = evaluation
                .reason_chains
                .iter()
                .find(|reason| reason.id == intent.reason_chain_id)
                .ok_or_else(|| {
                    policy_admission_fatal(
                        "policy_reason_chain_missing",
                        "record_trusted_policy_dispatch",
                    )
                })?;
            let trusted = TrustedPolicyDispatch {
                intent: intent.clone(),
                reason_chain: reason_chain.clone(),
                observed_monotonic_ms,
            };
            if let Some(existing) = self.entries.get(&intent.decision_id) {
                if existing.intent != trusted.intent
                    || existing.reason_chain != trusted.reason_chain
                {
                    return Err(policy_admission_fatal(
                        "policy_decision_identity_conflict",
                        "record_trusted_policy_dispatch",
                    ));
                }
                continue;
            }
            self.order.push_back(intent.decision_id.clone());
            self.entries.insert(intent.decision_id.clone(), trusted);
        }
        while self.order.len() > MAX_TRUSTED_POLICY_DISPATCHES {
            if let Some(expired) = self.order.pop_front() {
                self.entries.remove(&expired);
            }
        }
        Ok(())
    }

    fn authorize(
        &self,
        intent: &DispatchIntent,
        reason_chain: &DecisionReasonChain,
    ) -> RuntimeHostResult<TrustedPolicyDispatch> {
        let trusted = self.entries.get(&intent.decision_id).ok_or_else(|| {
            policy_admission_request(
                "policy_decision_not_host_evaluated",
                "authorize_policy_dispatch",
            )
        })?;
        if trusted.intent != *intent || trusted.reason_chain != *reason_chain {
            return Err(policy_admission_request(
                "policy_trusted_context_mismatch",
                "authorize_policy_dispatch",
            ));
        }
        Ok(trusted.clone())
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
}

fn initial_registered_instances(
    provider: &dyn ExecutionBackendProvider,
) -> RuntimeHostResult<BTreeMap<InstanceId, RegisteredInstance>> {
    let aliases = provider.instance_aliases();
    if aliases.is_empty() {
        return Err(RuntimeHostError::fatal(
            "empty_execution_backend_registry",
            "initialize_runtime_instance_registry",
            RuntimeErrorCode::RuntimeFatal,
        ));
    }
    let mut seen_aliases = BTreeSet::new();
    let mut instances = BTreeMap::new();
    for instance_alias in aliases {
        if actingcommand_contract::validate_instance_alias(&instance_alias).is_err()
            || !seen_aliases.insert(instance_alias.clone())
        {
            return Err(RuntimeHostError::fatal(
                "invalid_execution_backend_registry",
                "initialize_runtime_instance_registry",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        let resolved = provider.resolve(&instance_alias).ok_or_else(|| {
            RuntimeHostError::fatal(
                "execution_backend_registry_incomplete",
                "initialize_runtime_instance_registry",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        if resolved.audit_endpoint().is_empty() {
            return Err(RuntimeHostError::fatal(
                "invalid_execution_backend_registry",
                "initialize_runtime_instance_registry",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        let registration = RegisteredInstance {
            instance_alias,
            instance_id: resolved.instance_id(),
            audit_endpoint: resolved.audit_endpoint().to_string(),
            provenance: resolved.provenance(),
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

fn reconcile_runtime_state(
    state: &Arc<RuntimeStateStore>,
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
) -> RuntimeHostResult<()> {
    let migrated = ledger
        .query(EventQuery {
            event_type: Some(EventType::StateMigrated),
            ..EventQuery::default()
        })
        .map_err(|_| ledger_error("query_state_migrations"))?
        .into_iter()
        .filter_map(|event| match event.payload() {
            EventPayload::State(StatePayload::Migrated(payload)) => {
                Some(payload.migration().migration_id().to_owned())
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    for migration in state
        .migrations()
        .map_err(|error| RuntimeHostError::state(&error))?
    {
        if migration.state_key() == actingcommand_runtime_state::RELEASE_BASELINE_STATE_KEY {
            continue;
        }
        if migration.state_key() == actingcommand_runtime_state::CATALOG_ACTIVE_STATE_KEY {
            if !migrated.contains(migration.migration_id()) {
                return Err(RuntimeHostError::fatal(
                    "catalog_migration_source_missing",
                    "reconcile_runtime_state",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            continue;
        }
        if !migrated.contains(migration.migration_id()) {
            append_runtime_state_event(
                ledger,
                events,
                StatePayloadDraft::migrated(migration, AuditInput::new()),
            )?;
        }
    }

    state_control::reconcile_release_state(state, ledger, events)
}

fn append_runtime_state_event(
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
    payload: impl Into<actingcommand_contract::EventPayloadDraft>,
) -> RuntimeHostResult<PersistedEvent> {
    let draft = events.draft(
        EventSeverity::Info,
        EventSource::Runtime,
        OriginModule::Runtime,
        EventActor::Runtime,
        events.system_links()?,
        payload,
    )?;
    let draft = events.sanitize(draft)?;
    ledger
        .append(draft)
        .map_err(|_| ledger_error("append_runtime_state_event"))
}

fn reconcile_agent_wakes(
    state: &mut AgentDispatcherState,
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
    instances: &BTreeMap<InstanceId, RegisteredInstance>,
    config: &AgentDispatcherConfig,
) -> RuntimeHostResult<()> {
    let sources = ledger
        .query(EventQuery {
            event_type: Some(EventType::PolicyPlanningSignalObserved),
            ..EventQuery::default()
        })
        .map_err(|_| ledger_error("query_agent_wake_sources"))?;
    for source in sources {
        if state.has_wake_for_trigger(source.event_id()) {
            continue;
        }
        let EventPayload::Policy(actingcommand_contract::PolicyPayload::PlanningSignalObserved(
            signal,
        )) = source.payload()
        else {
            return Err(RuntimeHostError::fatal(
                "agent_wake_source_invalid",
                "reconcile_agent_wakes",
                RuntimeErrorCode::RuntimeFatal,
            ));
        };
        let kind = match signal.kind() {
            actingcommand_contract::PolicyPlanningSignalKind::TimelineReached => {
                AgentWakeKind::TimelineReached
            }
            actingcommand_contract::PolicyPlanningSignalKind::DriftPredicted => {
                AgentWakeKind::DriftPredicted
            }
            _ => continue,
        };
        let instance_id = instances
            .values()
            .find(|instance| instance.instance_alias == signal.instance_id())
            .map(|instance| instance.instance_id)
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    "agent_wake_instance_unknown",
                    "reconcile_agent_wakes",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
        append_agent_wake(state, ledger, events, config, &source, instance_id, kind)?;
    }
    Ok(())
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
    scheduler: Mutex<SeedScheduler>,
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
    policy_inputs: Mutex<Option<PolicyInputSnapshot>>,
    // A bounded cache of exact GlobalLedger projections; it never computes or owns outcomes.
    authoritative_policy_outcomes:
        Mutex<BTreeMap<(String, String), AuthoritativeSchedulingOutcome>>,
    procedure_manifest: Mutex<Option<ProcedureManifest>>,
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

#[derive(Clone, Copy)]
struct RuntimeLeaseAcquisition<'request, 'payload> {
    request: &'request ValidatedRuntimeRequest<'payload>,
    request_id: RequestId,
    instance_alias: &'request str,
    holder_id: actingcommand_contract::HolderId,
    connection_id: ConnectionId,
    run_links: Option<RuntimeRunLinks>,
    lease_ttl_ms: Option<u64>,
}

impl RuntimeRunLinks {
    const fn new(task_id: IssuedTaskId, run_id: IssuedRunId) -> Self {
        Self { task_id, run_id }
    }

    fn apply(self, links: EventLinksDraft) -> EventLinksDraft {
        links.with_task_id(self.task_id).with_run_id(self.run_id)
    }
}

struct PolicyAdmissionAppender<'a> {
    ledger: &'a GlobalLedger,
    initial_fact_gate: RefCell<Option<MutexGuard<'a, ()>>>,
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

impl<'a> PolicyAdmissionAppender<'a> {
    fn new(ledger: &'a GlobalLedger, initial_fact_gate: MutexGuard<'a, ()>) -> Self {
        Self {
            ledger,
            initial_fact_gate: RefCell::new(Some(initial_fact_gate)),
        }
    }
}

impl EventAppender for PolicyAdmissionAppender<'_> {
    fn append_durable(
        &self,
        draft: actingcommand_contract::SanitizedEventDraft,
    ) -> actingcommand_ledger::GlobalLedgerResult<PersistedEvent> {
        let event = self.ledger.append(draft)?;
        self.initial_fact_gate.borrow_mut().take();
        Ok(event)
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

impl OperationSuccess {
    fn into_receipt(self, request: &RuntimeRequest) -> RuntimeHostResult<RuntimeReceipt> {
        match self.result {
            RuntimeResult::ContainedLabOperation { operation } => {
                RuntimeReceipt::contained_lab_operation(
                    request,
                    self.terminal.ok_or_else(receipt_error)?,
                    operation,
                )
            }
            result => RuntimeReceipt::success(request, self.state, self.terminal, result),
        }
        .map_err(|_| receipt_error())
    }
}

struct RequestFailure {
    state: RuntimeReceiptState,
    terminal: Option<TerminalEvent>,
    error: Box<RuntimeHostError>,
    poison_runtime: bool,
    task_failure: Option<TaskFailureEvidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TaskFailureEvidence {
    code: &'static str,
    severity: EventSeverity,
}

struct ActionFailure {
    error: RuntimeHostError,
    diagnostic: DiagnosticCode,
    effect: EffectDisposition,
    poison_runtime: bool,
    release_after: bool,
    destructive_started: bool,
    transfer_after: bool,
    task_failure: Option<Box<TaskFailureEvidence>>,
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

    fn evaluate_policy_cycle(&self, trigger: PolicyTrigger) -> RuntimeHostResult<PolicyCycle> {
        let sample = self.runtime_clock_sample()?;
        let time = EvaluationTime {
            unix_ms: sample.unix_ms,
            monotonic_ms: sample.monotonic_ms,
        };
        self.evaluate_policy_cycle_authoritative(time, None, trigger, sample.monotonic_ms)
    }

    #[cfg(test)]
    fn evaluate_policy_cycle_with_test_inputs(
        &self,
        facts: &EvaluationFacts,
        resources: &EvaluationResources,
        time: EvaluationTime,
        seed: u64,
        trigger: PolicyTrigger,
    ) -> RuntimeHostResult<PolicyCycle> {
        {
            let _gate = lock(&self.fact_write_gate, "set_test_policy_inputs")?;
            *lock(&self.policy_inputs, "set_test_policy_inputs")? =
                Some(PolicyInputSnapshot::new(facts.clone(), resources.clone()));
        }
        self.evaluate_policy_cycle_authoritative(time, Some(seed), trigger, self.monotonic_ms()?)
    }

    fn evaluate_policy_cycle_authoritative(
        &self,
        time: EvaluationTime,
        seed: Option<u64>,
        trigger: PolicyTrigger,
        observed_monotonic_ms: u64,
    ) -> RuntimeHostResult<PolicyCycle> {
        if trigger == PolicyTrigger::Reconciliation {
            self.reconcile_pending_policy_settlements()?;
        }
        let _detection_gate = lock(&self.detection_write_gate, "plan_policy_detection")?;
        let procedure_manifest = lock(&self.procedure_manifest, "read_procedure_manifest")?
            .clone()
            .ok_or_else(|| {
                policy_admission_request("procedure_manifest_unconfigured", "evaluate_policy_cycle")
            })?;
        let (outcome_keys, facts, resources) = {
            let _outcome_gate = lock(&self.policy_outcome_gate, "snapshot_policy_outcome_state")?;
            let outcome_keys =
                lock(&self.policy, "read_policy_outcome_keys")?.outcome_key_snapshot()?;
            let _gate = lock(&self.fact_write_gate, "project_policy_facts")?;
            let (facts, resources) = self.project_authoritative_policy_inputs_under_gate(
                "evaluate_policy_cycle",
                &outcome_keys,
                None,
            )?;
            (outcome_keys, facts, resources)
        };
        let workloads = lock(&self.policy, "read_policy_performance_workloads")?
            .active_performance_workloads()?;
        let mut controlled_resources = resources;
        lock(
            &self.performance_control,
            "apply_policy_performance_control",
        )?
        .apply_to_resources(&mut controlled_resources.hosts, &workloads)?;
        let seed = match seed {
            Some(seed) => seed,
            None => runtime_policy_seed(&facts.fact_snapshot_id, time, self.owner_epoch)?,
        };
        let cycle = {
            let mut policy = lock(&self.policy, "evaluate_policy_cycle")?;
            policy.validate_outcome_key_snapshot(&outcome_keys)?;
            policy.evaluate(
                &facts,
                &controlled_resources,
                PolicyEvaluationContext {
                    procedure_manifest: &procedure_manifest,
                    time,
                    seed,
                    trigger,
                    sampled_at_monotonic_ms: observed_monotonic_ms,
                },
            )?
        };
        for signal in &cycle.detection_planning_signals {
            self.record_policy_planning_signal(signal.clone())?;
        }
        lock(
            &self.trusted_policy_dispatches,
            "record_trusted_policy_dispatches",
        )?
        .record_cycle(&cycle, observed_monotonic_ms)?;
        Ok(cycle)
    }

    #[cfg(test)]
    fn replace_procedure_manifest_for_test(
        &self,
        procedure_manifest: ProcedureManifest,
    ) -> RuntimeHostResult<()> {
        let _gate = lock(&self.fact_write_gate, "replace_procedure_manifest_for_test")?;
        *lock(
            &self.procedure_manifest,
            "replace_procedure_manifest_for_test",
        )? = Some(procedure_manifest);
        Ok(())
    }

    fn project_authoritative_policy_inputs_under_gate(
        &self,
        operation: &'static str,
        outcome_keys: &PolicyOutcomeKeySnapshot,
        as_of_ledger_position: Option<u64>,
    ) -> RuntimeHostResult<(EvaluationFacts, EvaluationResources)> {
        self.synchronize_fact_store_under_gate()?;
        let inputs = lock(&self.policy_inputs, "read_policy_inputs")?
            .clone()
            .ok_or_else(|| policy_admission_request("policy_inputs_unconfigured", operation))?;
        self.validate_policy_input_authority(&inputs, operation)?;
        let latest_ledger_position = self
            .ledger
            .latest_sequence()
            .map_err(|_| ledger_error("read_policy_fact_position"))?;
        let ledger_position = match as_of_ledger_position {
            Some(position) if position == 0 || position > latest_ledger_position => {
                return Err(policy_admission_request(
                    "policy_input_position_unavailable",
                    operation,
                ));
            }
            Some(position) => position,
            None => latest_ledger_position,
        };
        #[cfg(test)]
        let ledger_position = if as_of_ledger_position.is_none() {
            match self
                .policy_outcome_projection_position_override
                .swap(0, Ordering::AcqRel)
            {
                0 => ledger_position,
                injected => injected,
            }
        } else {
            ledger_position
        };
        let mut base_facts = inputs.facts().clone();
        base_facts.tasks = lock(&self.policy, "project_policy_task_state")?
            .task_runtime_snapshots(ledger_position)?;
        base_facts.tasks.retain(|state| {
            base_facts
                .instances
                .iter()
                .any(|instance| instance.instance_id == state.instance_id)
        });
        let authoritative_outcomes = lock(
            &self.authoritative_policy_outcomes,
            "project_policy_scheduling_outcomes",
        )?;
        if base_facts
            .outcomes
            .iter()
            .any(|outcome| outcome_keys.keys.contains_key(&outcome.task_id))
        {
            return Err(policy_admission_request(
                "policy_outcome_authority_conflict",
                operation,
            ));
        }
        for (key, expected_run) in &outcome_keys.completed_runs {
            let Some(expected_keys) = outcome_keys.keys.get(&expected_run.catalog_task_id) else {
                continue;
            };
            if matches!(
                expected_run.execution_outcome,
                PolicyExecutionOutcome::Failed { .. }
            ) {
                if authoritative_outcomes.contains_key(key) {
                    return Err(RuntimeHostError::fatal(
                        "policy_outcome_failed_run_residual",
                        operation,
                        RuntimeErrorCode::RuntimeFatal,
                    ));
                }
                continue;
            }
            let outcome = authoritative_outcomes.get(key).ok_or_else(|| {
                RuntimeHostError::request(
                    "outcome_projection_not_ready",
                    operation,
                    RuntimeErrorCode::RuntimeUnavailable,
                )
            })?;
            let identity = outcome.identity();
            if !completed_run_matches_outcome(expected_run, outcome)
                || !expected_keys.contains(outcome.disposition().outcome_key())
            {
                return Err(RuntimeHostError::request(
                    "outcome_projection_not_ready",
                    operation,
                    RuntimeErrorCode::RuntimeUnavailable,
                ));
            }
            if !base_facts
                .instances
                .iter()
                .any(|instance| instance.instance_id == identity.instance_alias())
            {
                continue;
            }
            if ledger_position < identity.terminal_sequence() {
                return Err(RuntimeHostError::request(
                    "outcome_projection_not_ready",
                    operation,
                    RuntimeErrorCode::RuntimeUnavailable,
                ));
            }
            #[cfg(test)]
            if self
                .policy_outcome_projection_failures
                .swap(0, Ordering::AcqRel)
                != 0
            {
                return Err(RuntimeHostError::fatal(
                    "policy_outcome_projection_injected_failure",
                    operation,
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            let projected = self
                .ledger
                .project_scheduling_outcomes(identity.clone(), ledger_position)
                .map_err(|error| {
                    if error.code() == "outcome_projection_not_ready"
                        || error.code() == "outcome_projection_position_invalid"
                    {
                        RuntimeHostError::request(
                            "outcome_projection_not_ready",
                            operation,
                            RuntimeErrorCode::RuntimeUnavailable,
                        )
                    } else {
                        ledger_error("project_policy_scheduling_outcome")
                    }
                })?;
            if projected.outcome() != outcome {
                return Err(RuntimeHostError::fatal(
                    "policy_outcome_projection_mismatch",
                    operation,
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            validate_completed_run_admission_request(
                &self.ledger,
                expected_run,
                identity.terminal_sequence(),
            )?;
            base_facts.outcomes.push(ObservedOutcome {
                task_id: identity.catalog_task_id().to_owned(),
                instance_id: identity.instance_alias().to_owned(),
                outcome_key: outcome.disposition().outcome_key().to_owned(),
                value: PolicyFactValue::Boolean(true),
                observed_at_unix_ms: outcome.terminal_timestamp_unix_ms(),
                expires_at_unix_ms: None,
                activity_window_id: Some(expected_run.activity_window_id.clone()),
            });
        }
        let fact_store = lock(&self.facts, "project_policy_facts")?;
        let historical;
        let fact_projection = if ledger_position == latest_ledger_position {
            &*fact_store
        } else {
            historical = fact_store.at_position(&self.ledger, ledger_position)?;
            &historical
        };
        let catalog = lock(&self.policy, "project_fact_pool_catalog")?.active_loaded();
        if let Some(catalog) = &catalog {
            validate_static_fact_pool_authority(
                catalog.compiled(),
                inputs.facts(),
                inputs.resources(),
                operation,
            )?;
        }
        let facts = fact_projection.overlay_policy_facts(
            &base_facts,
            inputs.resources(),
            ledger_position,
        )?;
        let resources = if let Some(catalog) = catalog {
            fact_projection.validate_pool_sources(catalog.compiled(), |scope| {
                self.fact_scope_instances(scope)
            })?;
            actingcommand_policy::project_fact_pools(catalog.compiled(), &facts, inputs.resources())
        } else {
            inputs.resources().clone()
        };
        let facts =
            fact_projection.overlay_policy_facts(&base_facts, &resources, ledger_position)?;
        Ok((facts, resources))
    }

    fn validate_policy_input_authority(
        &self,
        inputs: &PolicyInputSnapshot,
        operation: &'static str,
    ) -> RuntimeHostResult<()> {
        let registered = lock(
            &self.registered_instances,
            "validate_policy_instance_metadata",
        )?;
        let registered_aliases = registered
            .values()
            .map(|instance| instance.instance_alias.as_str())
            .collect::<BTreeSet<_>>();
        let snapshot_aliases = inputs
            .facts()
            .instances
            .iter()
            .map(|instance| instance.instance_id.as_str())
            .collect::<BTreeSet<_>>();
        if registered_aliases != snapshot_aliases {
            return Err(policy_admission_request(
                "policy_instance_metadata_untrusted",
                operation,
            ));
        }
        let host_ids = inputs
            .resources()
            .hosts
            .iter()
            .map(|host| host.host_id.as_str())
            .collect::<BTreeSet<_>>();
        if inputs
            .facts()
            .instances
            .iter()
            .any(|instance| !host_ids.contains(instance.host_id.as_str()))
        {
            return Err(policy_admission_request(
                "policy_resource_metadata_untrusted",
                operation,
            ));
        }
        Ok(())
    }

    fn project_policy_forward(
        &self,
        facts: &EvaluationFacts,
        resources: &EvaluationResources,
        time: EvaluationTime,
        seed: u64,
        config: ForwardProjectionConfig,
    ) -> RuntimeHostResult<ForwardProjection> {
        let declared_facts = facts;
        let (facts, fact_projection) = {
            let mut fact_projection = lock(&self.facts, "project_forward_facts")?.clone();
            fact_projection.synchronize(&self.ledger)?;
            let ledger_position = self
                .ledger
                .latest_sequence()
                .map_err(|_| ledger_error("project_forward_fact_position"))?;
            let facts =
                fact_projection.overlay_external_policy_facts(facts, resources, ledger_position)?;
            (facts, fact_projection)
        };
        let (catalog, workloads) = {
            let policy = lock(&self.policy, "project_forward_catalog")?;
            let catalog = policy.active_loaded().ok_or_else(|| {
                RuntimeHostError::request(
                    "policy_catalog_unavailable",
                    "project_policy_forward",
                    RuntimeErrorCode::InvalidRequest,
                )
            })?;
            (catalog, policy.active_performance_workloads()?)
        };
        validate_static_fact_pool_authority(
            catalog.compiled(),
            declared_facts,
            resources,
            "project_policy_forward",
        )?;
        fact_projection
            .validate_pool_sources(catalog.compiled(), |scope| self.fact_scope_instances(scope))?;
        let mut resources =
            actingcommand_policy::project_fact_pools(catalog.compiled(), &facts, resources);
        lock(
            &self.performance_control,
            "apply_forward_performance_control",
        )?
        .apply_to_resources(&mut resources.hosts, &workloads)?;
        project_forward(catalog.compiled(), &facts, &resources, time, seed, config).map_err(
            |error| {
                RuntimeHostError::request(
                    error.code(),
                    "project_policy_forward",
                    RuntimeErrorCode::InvalidRequest,
                )
            },
        )
    }

    fn assess_and_publish_predictive_maintenance(
        &self,
        query: &MaintenanceLedgerQuery,
    ) -> RuntimeHostResult<MaintenanceAssessment> {
        let result: RuntimeHostResult<MaintenanceAssessment> = (|| {
            let evidence = collect_maintenance_evidence(&self.ledger, query)?;
            let assessment = assess_predictive_maintenance(&evidence, query.trend_policy())
                .map_err(|error| {
                    RuntimeHostError::request(
                        error.code(),
                        "assess_predictive_maintenance",
                        RuntimeErrorCode::InvalidRequest,
                    )
                })?;
            if assessment.recheck_suggested() {
                let observed_at_unix_ms = evidence
                    .durations
                    .iter()
                    .map(|sample| sample.observed_at_unix_ms)
                    .chain(
                        evidence
                            .confidences
                            .iter()
                            .map(|sample| sample.observed_at_unix_ms),
                    )
                    .max()
                    .ok_or_else(|| {
                        RuntimeHostError::fatal(
                            "maintenance_evidence_timestamp_missing",
                            "assess_predictive_maintenance",
                            RuntimeErrorCode::RuntimeFatal,
                        )
                    })?;
                self.record_policy_planning_signal(PolicyPlanningSignalEventData {
                    signal_id: format!("signal:{}", assessment.assessment_id),
                    instance_id: query.instance_id().to_owned(),
                    task_id: Some(query.task_id().to_owned()),
                    kind: actingcommand_contract::PolicyPlanningSignalKind::DriftPredicted,
                    fact_code: "maintenance_recheck_suggested".to_owned(),
                    observed_at_unix_ms,
                    detection_budget: None,
                })?;
            }
            Ok(assessment)
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    fn admit_policy_dispatch(
        &self,
        intent: &DispatchIntent,
        reason_chain: &DecisionReasonChain,
        context: &PolicyAdmissionContext,
        task_request: Option<&ContainedTaskRequest>,
    ) -> RuntimeHostResult<PolicyDispatchAdmission> {
        {
            let policy = lock(&self.policy, "validate_policy_dispatch")?;
            if let Some(replay) = policy.replay_admission(intent, reason_chain)? {
                return Ok(replay);
            }
        }
        let trusted = lock(
            &self.trusted_policy_dispatches,
            "authorize_trusted_policy_dispatch",
        )?
        .authorize(intent, reason_chain)?;
        if context.fact_ledger_position != trusted.intent.input_ledger_position
            || context.fact_snapshot_id != trusted.intent.fact_snapshot_id
            || context.fencing_owner_epoch != self.owner_epoch
        {
            return Err(policy_admission_request(
                "policy_admission_context_untrusted",
                "admit_policy_dispatch",
            ));
        }
        let elapsed_ms = self
            .monotonic_ms()?
            .checked_sub(trusted.observed_monotonic_ms)
            .ok_or_else(|| {
                policy_admission_fatal("policy_admission_clock_regressed", "admit_policy_dispatch")
            })?;
        let now_unix_ms = trusted
            .intent
            .prerequisites
            .evaluated_at_unix_ms
            .checked_add(elapsed_ms)
            .ok_or_else(|| {
                policy_admission_fatal("policy_admission_clock_overflow", "admit_policy_dispatch")
            })?;
        // Approval projection and dispatch admission share one order so a concurrent revocation
        // cannot appear in the ledger before a dispatch authorized by the superseded fact.
        let _governance_gate = lock(&self.governance_write_gate, "project_policy_approvals")?;
        let approval_fact_ids =
            match ApprovalProjection::recover(&self.ledger, Arc::clone(&self.state)) {
                Ok(projection) => projection.active_for_dispatch(intent),
                Err(error) => {
                    self.fatal.mark(error.clone())?;
                    return Err(error);
                }
            };
        let authoritative_context = PolicyAdmissionContext {
            fact_ledger_position: trusted.intent.input_ledger_position,
            fact_snapshot_id: trusted.intent.fact_snapshot_id.clone(),
            approval_fact_ids,
            fencing_owner_epoch: self.owner_epoch,
            now_unix_ms,
        };
        let context = &authoritative_context;
        let mut gate_error = match lock(
            &self.performance_control,
            "gate_policy_performance_dispatch",
        )?
        .gate_dispatch(
            &intent.instance_id,
            intent.prerequisites.urgency_milli,
            context.now_unix_ms,
        )? {
            PerformanceDispatchGate::Allowed => None,
            PerformanceDispatchGate::Deferred {
                reason,
                deadline_disposition,
                event,
            } => {
                if let Some(event) = event {
                    self.record_performance_events(&[PerformanceSemanticEvent::BalanceChanged(
                        event,
                    )])?;
                }
                let code = if deadline_disposition
                    == Some(actingcommand_contract::PerformanceDeadlineDisposition::CapacityFailure)
                {
                    "performance_capacity_deadline_conflict"
                } else {
                    reason
                };
                Some(RuntimeHostError::request(
                    code,
                    "admit_policy_dispatch",
                    RuntimeErrorCode::InvalidRequest,
                ))
            }
        };
        if gate_error.is_none()
            && let Err(error) = self.admit_capacity()
        {
            if error.is_fatal() {
                return Err(error);
            }
            gate_error = Some(error);
        }
        let resolved = self
            .resolve_instance(&intent.instance_id)
            .map_err(|failure| *failure.error)?;
        let request_id = self
            .events
            .issuer()
            .mint_request_id()
            .map_err(|_| policy_id_error("issue_policy_request_id"))?;
        let correlation_id = self
            .events
            .issuer()
            .mint_correlation_id()
            .map_err(|_| policy_id_error("issue_policy_correlation_id"))?;
        let holder = self
            .events
            .issuer()
            .mint_holder_id()
            .map_err(|_| policy_id_error("issue_policy_holder_id"))?;
        let task_id = self
            .events
            .issuer()
            .mint_task_id()
            .map_err(|_| policy_id_error("issue_policy_task_id"))?;
        let run_id = self
            .events
            .issuer()
            .mint_run_id()
            .map_err(|_| policy_id_error("issue_policy_run_id"))?;
        let run_links = RuntimeRunLinks::new(task_id, run_id);
        let holder_id = *holder.transport();
        let request = RuntimeRequest::new(
            request_id,
            correlation_id,
            None,
            EventActor::Agent,
            EventSource::Adapter,
            context.now_unix_ms,
            RuntimeOperation::acquire_lease(intent.instance_id.clone(), holder),
        )
        .map_err(|_| policy_contract_error("build_policy_runtime_request"))?;
        let validated = request
            .validate()
            .map_err(|_| policy_contract_error("validate_policy_runtime_request"))?;
        let connection_id = ConnectionId::new(POLICY_CONNECTION_VALUE)
            .map_err(|error| RuntimeHostError::scheduler("build_policy_connection", &error))?;
        let action_id = self.events.action_id()?;
        let links = run_links.apply(self.events.request_links(
            &validated,
            Some(resolved.instance_id()),
            None,
            Some(action_id),
        ));
        let data = policy_event_data(intent, reason_chain)?;
        let event = self.events.draft(
            EventSeverity::Info,
            EventSource::Scheduler,
            OriginModule::Policy,
            EventActor::Scheduler,
            links.clone(),
            PolicyPayloadDraft::dispatch_intent(data.clone(), AuditInput::new()),
        )?;
        let event = self.events.sanitize(event)?;
        let plan = CriticalEventPlan::new(CriticalOperation::PolicyDispatch, event)
            .map_err(|_| critical_plan_error())?;
        let (outcome_keys, current_facts, fact_gate) = {
            let _outcome_gate = lock(&self.policy_outcome_gate, "snapshot_policy_outcome_state")?;
            let outcome_keys =
                lock(&self.policy, "read_policy_outcome_keys")?.outcome_key_snapshot()?;
            if outcome_keys.generation.as_ref().is_none_or(|generation| {
                generation.catalog_hash() != intent.catalog_hash
                    || generation.catalog_version() != intent.catalog_version
            }) {
                return Err(policy_admission_request(
                    "catalog_active_generation_changed",
                    "admit_policy_dispatch",
                ));
            }
            let fact_gate = lock(&self.fact_write_gate, "validate_policy_fact_freshness")?;
            let (current_facts, _) = self.project_authoritative_policy_inputs_under_gate(
                "admit_policy_dispatch",
                &outcome_keys,
                None,
            )?;
            (outcome_keys, current_facts, fact_gate)
        };
        if current_facts.fact_snapshot_id != trusted.intent.fact_snapshot_id {
            return Err(policy_admission_request(
                "policy_facts_stale",
                "admit_policy_dispatch",
            ));
        }
        lock(&self.procedure_manifest, "validate_procedure_manifest")?
            .as_ref()
            .ok_or_else(|| {
                policy_admission_request("procedure_manifest_unconfigured", "admit_policy_dispatch")
            })?
            .validate_intent(intent, "admit_policy_dispatch")?;
        let appender = PolicyAdmissionAppender::new(&self.ledger, fact_gate);
        let success_links = links.clone();
        let failure_links = links;
        let success_data = data.clone();
        let failure_data = data;
        let result = execute_critical(
            &appender,
            self.events.fingerprinter(),
            plan,
            || {
                #[cfg(test)]
                policy_crash_test_barrier("after_policy_intent");
                if let Some(error) = gate_error.clone() {
                    return CriticalActionReport::Failed {
                        error: RequestFailure::request(error, RuntimeReceiptState::Denied, None),
                        effect: EffectDisposition::NotPerformed,
                    };
                }
                let ledger_high_watermark = match self.ledger.latest_sequence() {
                    Ok(position) => position,
                    Err(_) => {
                        return CriticalActionReport::Failed {
                            error: RequestFailure::poison_without_terminal(ledger_error(
                                "read_policy_ledger_position",
                            )),
                            effect: EffectDisposition::NotPerformed,
                        };
                    }
                };
                let mut policy = match lock(&self.policy, "validate_policy_dispatch") {
                    Ok(policy) => policy,
                    Err(error) => {
                        return CriticalActionReport::Failed {
                            error: RequestFailure::poison_without_terminal(error),
                            effect: EffectDisposition::NotPerformed,
                        };
                    }
                };
                if let Err(error) = policy.validate_outcome_key_snapshot(&outcome_keys) {
                    return CriticalActionReport::Failed {
                        error: RequestFailure::request(error, RuntimeReceiptState::Denied, None),
                        effect: EffectDisposition::NotPerformed,
                    };
                }
                let catalog = match policy.validate_dispatch(
                    intent,
                    reason_chain,
                    context,
                    self.owner_epoch,
                    ledger_high_watermark,
                ) {
                    Ok(catalog) => catalog,
                    Err(error) => {
                        let failure = if error.is_fatal() {
                            RequestFailure::poison_without_terminal(error)
                        } else {
                            RequestFailure::request(error, RuntimeReceiptState::Denied, None)
                        };
                        return CriticalActionReport::Failed {
                            error: failure,
                            effect: EffectDisposition::NotPerformed,
                        };
                    }
                };
                let admission_record = match policy.preview_admission(intent, context.now_unix_ms) {
                    Ok(record) => record,
                    Err(error) => {
                        let failure = if error.is_fatal() {
                            RequestFailure::poison_without_terminal(error)
                        } else {
                            RequestFailure::request(error, RuntimeReceiptState::Denied, None)
                        };
                        return CriticalActionReport::Failed {
                            error: failure,
                            effect: EffectDisposition::NotPerformed,
                        };
                    }
                };
                let lease_ttl_ms = task_request
                    .map(|task_request| {
                        task_request.validate().map_err(|_| {
                            RequestFailure::request(
                                policy_admission_request(
                                    "policy_task_request_invalid",
                                    "admit_policy_dispatch",
                                ),
                                RuntimeReceiptState::Denied,
                                None,
                            )
                        })?;
                        if intent.package_digest.as_ref() != Some(task_request.expected_sha256()) {
                            return Err(RequestFailure::request(
                                policy_admission_request(
                                    "procedure_package_digest_mismatch",
                                    "admit_policy_dispatch",
                                ),
                                RuntimeReceiptState::Denied,
                                None,
                            ));
                        }
                        self.contained_task_lease_ttl(task_request)
                    })
                    .transpose();
                let lease_ttl_ms = match lease_ttl_ms {
                    Ok(ttl) => ttl,
                    Err(error) => {
                        return CriticalActionReport::Failed {
                            error,
                            effect: EffectDisposition::NotPerformed,
                        };
                    }
                };
                let admission = self.acquire_lease(RuntimeLeaseAcquisition {
                    request: &validated,
                    request_id: request.request_id(),
                    instance_alias: &intent.instance_id,
                    holder_id,
                    connection_id,
                    run_links: Some(run_links),
                    lease_ttl_ms,
                });
                match admission {
                    Ok(success) => match success.result {
                        RuntimeResult::LeaseGranted { token } => {
                            #[cfg(test)]
                            policy_crash_test_barrier("after_lease_grant");
                            if let Err(error) = policy.commit_admission(intent, &admission_record) {
                                return CriticalActionReport::Failed {
                                    error: RequestFailure::poison_without_terminal(error),
                                    effect: EffectDisposition::Indeterminate,
                                };
                            }
                            #[cfg(test)]
                            policy_crash_test_barrier("after_budget_commit");
                            CriticalActionReport::Succeeded {
                                value: (token, catalog, admission_record),
                                effect: DefiniteEffectDisposition::Performed,
                            }
                        }
                        _ => CriticalActionReport::Failed {
                            error: RequestFailure::poison_without_terminal(
                                RuntimeHostError::fatal(
                                    "policy_lease_result_invalid",
                                    "admit_policy_dispatch",
                                    RuntimeErrorCode::RuntimeFatal,
                                ),
                            ),
                            effect: EffectDisposition::Indeterminate,
                        },
                    },
                    Err(error) => {
                        let effect = if error.poison_runtime {
                            EffectDisposition::Indeterminate
                        } else {
                            EffectDisposition::NotPerformed
                        };
                        CriticalActionReport::Failed { error, effect }
                    }
                }
            },
            |(_, _, admission), _| {
                self.events
                    .draft(
                        EventSeverity::Info,
                        EventSource::Scheduler,
                        OriginModule::Policy,
                        EventActor::Scheduler,
                        success_links,
                        PolicyPayloadDraft::dispatch_admitted(
                            success_data,
                            admission.clone(),
                            AuditInput::new(),
                        ),
                    )
                    .map_err(|_| actingcommand_contract::SanitizationError::fingerprinter_failure())
            },
            |failure, effect| {
                self.events
                    .draft(
                        EventSeverity::Error,
                        EventSource::Scheduler,
                        OriginModule::Policy,
                        EventActor::Scheduler,
                        failure_links,
                        PolicyPayloadDraft::dispatch_rejected_with_reason(
                            failure_data,
                            effect,
                            failure.error.policy_rejection(),
                            AuditInput::new(),
                        ),
                    )
                    .map_err(|_| actingcommand_contract::SanitizationError::fingerprinter_failure())
            },
        );
        self.refresh_policy_dispatches()?;
        match result {
            Ok(receipt) => {
                let started_at_monotonic_ms = self.monotonic_ms()?;
                let (token, catalog, admission) = receipt.into_value();
                let clock = PolicyDispatchClock::live(
                    admission.activity.admitted_at_unix_ms,
                    started_at_monotonic_ms,
                );
                if lock(&self.policy_dispatch_clocks, "record_policy_dispatch_start")?
                    .insert(intent.decision_id.clone(), clock)
                    .is_some()
                {
                    let error = policy_admission_fatal(
                        "policy_dispatch_clock_identity_conflict",
                        "record_policy_dispatch_start",
                    );
                    self.fatal.mark(error.clone())?;
                    return Err(error);
                }
                Ok(PolicyDispatchAdmission::Granted {
                    context: Box::new(PolicyRunContext::new(
                        request,
                        correlation_id,
                        run_id,
                        task_id,
                        catalog,
                        token,
                        admission,
                        intent.clone(),
                        reason_chain.clone(),
                    )?),
                })
            }
            Err(CriticalExecutionError::Action { error, outcome, .. }) => {
                if error.error.lifecycle.capacity.is_some() {
                    self.record_required_failure(
                        &error.error,
                        &outcome,
                        self.events.request_links(
                            &validated,
                            Some(resolved.instance_id()),
                            None,
                            None,
                        ),
                    )?;
                }
                if error.poison_runtime {
                    self.fatal.mark((*error.error).clone())?;
                }
                Err(*error.error)
            }
            Err(error) => {
                let error = critical_execution_error(&error);
                self.fatal.mark(error.clone())?;
                Err(error)
            }
        }
    }

    fn refresh_policy_dispatches(&self) -> RuntimeHostResult<()> {
        let result =
            lock(&self.policy, "recover_policy_dispatches")?.refresh_dispatches(&self.ledger);
        if let Err(error) = &result {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    fn pinned_policy_catalog(
        &self,
        decision_id: &str,
    ) -> RuntimeHostResult<Option<CatalogGeneration>> {
        Ok(lock(&self.policy, "read_pinned_policy_catalog")?.pinned_catalog(decision_id))
    }

    fn record_policy_planning_signal(
        &self,
        signal: PolicyPlanningSignalEventData,
    ) -> RuntimeHostResult<()> {
        let mut committed_signal = None;
        let result: RuntimeHostResult<()> = (|| {
            let mut policy = lock(&self.policy, "record_policy_planning_signal")?;
            policy.validate_planning_signal(&signal)?;
            if let Some(existing) = policy.planning_signal(&signal.signal_id)? {
                return if existing == signal {
                    Ok(())
                } else {
                    Err(RuntimeHostError::fatal(
                        "policy_planning_signal_identity_conflict",
                        "record_policy_planning_signal",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                };
            }
            let (work, staged_quota) = policy.prepare_planning_signal(&signal)?;
            let fact_gate = lock(&self.fact_write_gate, "append_planning_transaction")?;
            if self.lifecycle_append_failed.load(Ordering::Acquire) {
                return Err(ledger_error("append_planning_transaction"));
            }
            let links = self.events.system_links()?;
            let draft = self.events.draft(
                EventSeverity::Info,
                EventSource::Scheduler,
                OriginModule::Policy,
                EventActor::Scheduler,
                links.clone(),
                PolicyPayloadDraft::planning_signal_observed(signal.clone(), AuditInput::new()),
            )?;
            let draft = self.events.sanitize(draft)?;
            let attempt_event_id = *draft.event_id();
            let persisted = self
                .ledger
                .append_transaction(draft, Box::new(work))
                .map_err(|error| {
                    let error = crate::policy_host::planning_transaction_error(error);
                    if error.is_fatal() {
                        self.lifecycle_append_failed.store(true, Ordering::Release);
                    }
                    let context = error.clone().with_native_detail(format!(
                        "event_id={attempt_event_id:?}; attempted_sequence={:?}",
                        staged_quota.attempted_sequence()
                    ));
                    error.with_related_failure("planning_attempt", &context)
                })?;
            committed_signal = Some((*persisted.event_id(), persisted.sequence()));
            policy.publish_planning_signal(staged_quota);
            drop(policy);
            let observed = self
                .observe_device_diagnostics_under_fact_gate(&persisted, &links)
                .and_then(|()| self.synchronize_fact_store_under_gate());
            drop(fact_gate);
            observed.and_then(|()| self.observe_pipeline_event(&persisted))?;
            let Some(config) = &self.agent_dispatcher_config else {
                return Ok(());
            };
            let kind = match signal.kind {
                actingcommand_contract::PolicyPlanningSignalKind::TimelineReached => {
                    AgentWakeKind::TimelineReached
                }
                actingcommand_contract::PolicyPlanningSignalKind::DriftPredicted => {
                    AgentWakeKind::DriftPredicted
                }
                _ => return Ok(()),
            };
            let instance_id = lock(&self.registered_instances, "resolve_agent_wake_instance")?
                .values()
                .find(|instance| instance.instance_alias == signal.instance_id)
                .map(|instance| instance.instance_id)
                .ok_or_else(|| {
                    RuntimeHostError::fatal(
                        "agent_wake_instance_unknown",
                        "record_policy_planning_signal",
                        RuntimeErrorCode::RuntimeFatal,
                    )
                })?;
            let _gate = lock(&self.agent_write_gate, "record_agent_wake")?;
            let mut agent = lock(&self.agent_dispatcher, "record_agent_wake")?;
            if !agent.has_wake_for_trigger(persisted.event_id()) {
                append_agent_wake(
                    &mut agent,
                    &self.ledger,
                    &self.events,
                    config,
                    &persisted,
                    instance_id,
                    kind,
                )?;
            }
            Ok(())
        })()
        .map_err(|error| {
            if let Some((event_id, sequence)) = committed_signal {
                self.lifecycle_append_failed.store(true, Ordering::Release);
                let context = error.clone().with_native_detail(format!(
                    "planning_fact_committed=true; event_id={event_id:?}; sequence={sequence}"
                ));
                let error = error
                    .into_fatal()
                    .with_related_failure("committed_planning_fact", &context);
                let _ = error.lifecycle.recorded_event.set(event_id);
                error
            } else {
                error
            }
        });
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone()).map_err(|secondary| {
                let mut combined = error.clone().with_related_failure("fatal_mark", &secondary);
                combined.lifecycle.recorded_event = Arc::clone(&error.lifecycle.recorded_event);
                combined
            })?;
        }
        result
    }

    fn project_policy_input_identity(
        &self,
        as_of_ledger_position: u64,
    ) -> Result<OperationSuccess, RequestFailure> {
        let identity = (|| {
            if as_of_ledger_position == 0 {
                return Err(RuntimeHostError::request(
                    "policy_input_position_unavailable",
                    "project_policy_input_identity",
                    RuntimeErrorCode::InvalidRequest,
                ));
            }
            let _outcome_gate = lock(
                &self.policy_outcome_gate,
                "snapshot_policy_input_identity_outcome_state",
            )?;
            let outcome_keys = lock(&self.policy, "read_policy_input_identity_outcome_keys")?
                .outcome_key_snapshot()?;
            let _fact_gate = lock(&self.fact_write_gate, "project_policy_input_identity_facts")?;
            let (facts, _) = self.project_authoritative_policy_inputs_under_gate(
                "project_policy_input_identity",
                &outcome_keys,
                Some(as_of_ledger_position),
            )?;
            RuntimePolicyInputIdentity::new(facts.ledger_position, facts.fact_snapshot_id).map_err(
                |_| {
                    RuntimeHostError::fatal(
                        "policy_input_identity_invalid",
                        "project_policy_input_identity",
                        RuntimeErrorCode::RuntimeFatal,
                    )
                },
            )
        })()
        .map_err(planning_request_failure)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::PolicyInputIdentityProjected { identity },
        })
    }

    fn acquire_lease(
        &self,
        acquisition: RuntimeLeaseAcquisition<'_, '_>,
    ) -> Result<OperationSuccess, RequestFailure> {
        let RuntimeLeaseAcquisition {
            request,
            request_id,
            instance_alias,
            holder_id,
            connection_id,
            run_links,
            lease_ttl_ms,
        } = acquisition;
        let resolved = self.resolve_instance(instance_alias)?;
        let instance_guard = self.instance_guard(resolved.instance_id())?;
        let _admission = lock(&instance_guard, "lock_instance_admission")?;
        self.expire_instance_if_due(resolved.instance_id())?;
        let preparation = {
            let mut scheduler = lock(&self.scheduler, "prepare_lease")?;
            let now_monotonic_ms = self.monotonic_ms()?;
            match lease_ttl_ms {
                Some(lease_ttl_ms) => scheduler.prepare_acquire_with_ttl(
                    request_id,
                    resolved.instance_id(),
                    holder_id,
                    connection_id,
                    lease_ttl_ms,
                    now_monotonic_ms,
                ),
                None => scheduler.prepare_acquire(
                    request_id,
                    resolved.instance_id(),
                    holder_id,
                    connection_id,
                    now_monotonic_ms,
                ),
            }
        };
        let preparation = match preparation {
            Ok(preparation) => preparation,
            Err(error) => {
                self.append_lease_requested(request, &resolved)?;
                return Err(self.scheduler_denied(request, &resolved, None, error)?);
            }
        };
        self.grant_prepared_lease(request, request_id, &resolved, preparation, run_links)
    }

    fn grant_prepared_lease(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        request_id: RequestId,
        resolved: &RegisteredInstance,
        preparation: LeasePreparation,
        run_links: Option<RuntimeRunLinks>,
    ) -> Result<OperationSuccess, RequestFailure> {
        if preparation.is_existing() {
            let terminal = self.existing_lease_terminal(
                request_id,
                preparation.token().lease_id(),
                EventType::LeaseGranted,
            )?;
            return Ok(OperationSuccess {
                state: RuntimeReceiptState::Admitted,
                terminal: Some(terminal),
                result: RuntimeResult::LeaseGranted {
                    token: preparation.token().clone(),
                },
            });
        }
        self.append_lease_requested(request, resolved)?;
        self.append_scheduler_admitted(request, resolved, None)?;
        let token = preparation.token().clone();
        let action_id = self
            .events
            .action_id()
            .map_err(RequestFailure::poison_without_terminal)?;
        let mut links = self.events.request_links(
            request,
            Some(resolved.instance_id()),
            Some(token.lease_id()),
            Some(action_id),
        );
        if let Some(run_links) = run_links {
            links = run_links.apply(links);
        }
        self.grant_prepared_lease_with_links(resolved, preparation, links, CapacityUse::Business)
    }

    fn grant_prepared_lease_with_links(
        &self,
        resolved: &RegisteredInstance,
        preparation: LeasePreparation,
        links: EventLinksDraft,
        capacity_use: CapacityUse,
    ) -> Result<OperationSuccess, RequestFailure> {
        if matches!(capacity_use, CapacityUse::Business) && !preparation.is_existing() {
            self.require_business_capacity(links.clone())?;
        }
        let intent = self.lease_intent(
            EventAction::LeaseAcquire,
            links.clone(),
            resolved.audit_endpoint(),
        )?;
        let plan = CriticalEventPlan::new(
            CriticalOperation::LeaseTransition(LeaseTransitionTarget::Granted),
            intent,
        )
        .map_err(|_| RequestFailure::poison_without_terminal(critical_plan_error()))?;
        let endpoint = resolved.audit_endpoint().to_string();
        let outcome_links = links.clone();
        let failure_links = links;
        let result = execute_critical(
            &self.ledger,
            self.events.fingerprinter(),
            plan,
            || match self.commit_acquire(preparation) {
                Ok(token) => CriticalActionReport::Succeeded {
                    value: token,
                    effect: DefiniteEffectDisposition::Performed,
                },
                Err(error) => CriticalActionReport::Failed {
                    effect: error.effect,
                    error,
                },
            },
            |_, effect| {
                self.events
                    .draft(
                        EventSeverity::Info,
                        EventSource::Scheduler,
                        OriginModule::Scheduler,
                        EventActor::Scheduler,
                        outcome_links,
                        LeasePayloadDraft::granted(
                            EventAction::LeaseAcquire,
                            effect.into(),
                            audit_endpoint(&endpoint),
                        ),
                    )
                    .map_err(|_| actingcommand_contract::SanitizationError::fingerprinter_failure())
            },
            |error, effect| {
                self.events
                    .draft(
                        EventSeverity::Error,
                        EventSource::Scheduler,
                        OriginModule::Scheduler,
                        EventActor::Scheduler,
                        failure_links,
                        LeasePayloadDraft::transition_failed(
                            EventAction::LeaseAcquire,
                            error.diagnostic,
                            effect,
                            audit_endpoint(&endpoint),
                        ),
                    )
                    .map_err(|_| actingcommand_contract::SanitizationError::fingerprinter_failure())
            },
        );
        self.map_critical_lease_result(result, RuntimeReceiptState::Admitted, |token| {
            RuntimeResult::LeaseGranted { token }
        })
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

    fn existing_lease_terminal(
        &self,
        request_id: RequestId,
        lease_id: LeaseId,
        event_type: EventType,
    ) -> Result<TerminalEvent, RequestFailure> {
        let events = self
            .ledger
            .query(EventQuery {
                event_type: Some(event_type),
                request_id: Some(request_id),
                lease_id: Some(lease_id),
                ..EventQuery::default()
            })
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error("query_lease_terminal"))
            })?;
        match events.as_slice() {
            [event] => Ok(terminal(event)),
            [] => Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "lease_terminal_event_missing",
                    "recover_idempotent_lease_request",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            )),
            _ => Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "lease_terminal_event_duplicated",
                    "recover_idempotent_lease_request",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            )),
        }
    }

    fn existing_request_terminal(
        &self,
        request_id: RequestId,
        event_type: EventType,
    ) -> Result<TerminalEvent, RequestFailure> {
        self.query_single_terminal(request_id, None, event_type)?
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "request_terminal_event_missing",
                    "recover_idempotent_runtime_request",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })
    }

    fn query_single_terminal(
        &self,
        request_id: RequestId,
        lease_id: Option<LeaseId>,
        event_type: EventType,
    ) -> Result<Option<TerminalEvent>, RequestFailure> {
        let events = self
            .ledger
            .query(EventQuery {
                event_type: Some(event_type),
                request_id: Some(request_id),
                lease_id,
                ..EventQuery::default()
            })
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error("query_request_terminal"))
            })?;
        match events.as_slice() {
            [] => Ok(None),
            [event] => Ok(Some(terminal(event))),
            _ => Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "request_terminal_event_duplicated",
                    "recover_idempotent_runtime_request",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            )),
        }
    }

    fn renew_lease(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        request_id: RequestId,
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let replayed = lock(&self.scheduler, "replay_renew_lease").and_then(|scheduler| {
            scheduler
                .replayed_renew(request_id, token, connection_id)
                .map_err(|error| RuntimeHostError::scheduler("replay_renew_lease", &error))
        });
        let replayed = match replayed {
            Ok(replayed) => replayed,
            Err(error) => {
                return Err(self.scheduler_denied_error(
                    request,
                    Some(token.instance_id()),
                    Some(token.lease_id()),
                    "",
                    error,
                )?);
            }
        };
        if let Some(renewed) = replayed {
            let terminal = self.existing_lease_terminal(
                request_id,
                renewed.lease_id(),
                EventType::LeaseRenewed,
            )?;
            return Ok(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: Some(terminal),
                result: RuntimeResult::LeaseRenewed { token: renewed },
            });
        }
        let instance_guard = self.instance_guard(token.instance_id())?;
        let _admission = lock(&instance_guard, "lock_instance_admission")?;
        let resolved = self.validated_instance(request, token, connection_id)?;
        self.append_scheduler_admitted_for_token(request, token, resolved.audit_endpoint())?;
        let action_id = self
            .events
            .action_id()
            .map_err(RequestFailure::poison_without_terminal)?;
        let links = self.events.request_links(
            request,
            Some(token.instance_id()),
            Some(token.lease_id()),
            Some(action_id),
        );
        let intent = self.lease_intent(
            EventAction::LeaseRenew,
            links.clone(),
            resolved.audit_endpoint(),
        )?;
        let plan = CriticalEventPlan::new(
            CriticalOperation::LeaseTransition(LeaseTransitionTarget::Renewed),
            intent,
        )
        .map_err(|_| RequestFailure::poison_without_terminal(critical_plan_error()))?;
        let outcome_links = links.clone();
        let failure_links = links;
        let endpoint = resolved.audit_endpoint;
        let result = execute_critical(
            &self.ledger,
            self.events.fingerprinter(),
            plan,
            || {
                let renewed = lock(&self.scheduler, "renew_lease").and_then(|mut scheduler| {
                    scheduler
                        .renew(request_id, token, connection_id, self.monotonic_ms()?)
                        .map_err(|error| RuntimeHostError::scheduler("renew_lease", &error))
                });
                match renewed {
                    Ok(token) => CriticalActionReport::Succeeded {
                        value: token,
                        effect: DefiniteEffectDisposition::Performed,
                    },
                    Err(error) => CriticalActionReport::Failed {
                        error: ActionFailure::scheduler(error),
                        effect: EffectDisposition::NotPerformed,
                    },
                }
            },
            |_, effect| {
                self.lease_outcome_draft(
                    EventSeverity::Info,
                    outcome_links,
                    LeasePayloadDraft::renewed(
                        EventAction::LeaseRenew,
                        effect.into(),
                        audit_endpoint(&endpoint),
                    ),
                )
            },
            |error, effect| {
                self.lease_failure_draft(
                    failure_links,
                    EventAction::LeaseRenew,
                    error.diagnostic,
                    effect,
                    &endpoint,
                )
            },
        );
        self.map_critical_lease_result(result, RuntimeReceiptState::Completed, |token| {
            RuntimeResult::LeaseRenewed { token }
        })
    }

    fn release_lease(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        request_id: RequestId,
        token: &LeaseToken,
        connection_id: ConnectionId,
        run_links: Option<RuntimeRunLinks>,
    ) -> Result<OperationSuccess, RequestFailure> {
        let replayed = lock(&self.scheduler, "replay_release_lease").and_then(|scheduler| {
            scheduler
                .replayed_release(request_id, token, connection_id)
                .map_err(|error| RuntimeHostError::scheduler("replay_release_lease", &error))
        });
        let replayed = match replayed {
            Ok(replayed) => replayed,
            Err(error) => {
                return Err(self.scheduler_denied_error(
                    request,
                    Some(token.instance_id()),
                    Some(token.lease_id()),
                    "",
                    error,
                )?);
            }
        };
        if let Some(released) = replayed {
            let terminal = self.existing_lease_terminal(
                request_id,
                released.token.lease_id(),
                EventType::LeaseReleased,
            )?;
            return Ok(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: Some(terminal),
                result: RuntimeResult::LeaseReleased {
                    instance_id: released.token.instance_id(),
                    lease_id: released.token.lease_id(),
                },
            });
        }
        let resolved = self.validated_instance(request, token, connection_id)?;
        let instance_guard = self.instance_guard(token.instance_id())?;
        let _admission = lock(&instance_guard, "lock_instance_admission")?;
        self.expire_queued_for_instance(token.instance_id())?;
        self.close_instance_resources(token, connection_id, EventLinksDraft::default())?;
        let transfer = lock(&self.scheduler, "prepare_release_transfer")?
            .prepare_transfer(
                token,
                connection_id,
                LeaseTransferReason::ExplicitRelease,
                Some(request_id),
                self.monotonic_ms()?,
            )
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                    "prepare_release_transfer",
                    &error,
                ))
            })?;
        self.append_scheduler_admitted_for_token(request, token, resolved.audit_endpoint())?;
        match transfer {
            TransferPreparation::Ready(prepared) => {
                if self.capacity_allows_transfer(token)? {
                    return self
                        .release_via_transfer(request, token, &resolved, prepared, run_links);
                }
            }
            TransferPreparation::Deferred => {
                return Err(self.scheduler_denied_error(
                    request,
                    Some(token.instance_id()),
                    Some(token.lease_id()),
                    resolved.audit_endpoint(),
                    RuntimeHostError::scheduler(
                        "prepare_release_transfer",
                        &SchedulerError::TransferNotSafe,
                    ),
                )?);
            }
            TransferPreparation::NoCandidate => {}
        }
        let action_id = self
            .events
            .action_id()
            .map_err(RequestFailure::poison_without_terminal)?;
        let mut links = self.events.request_links(
            request,
            Some(token.instance_id()),
            Some(token.lease_id()),
            Some(action_id),
        );
        if let Some(run_links) = run_links {
            links = run_links.apply(links);
        }
        let intent = self.lease_intent(
            EventAction::LeaseRelease,
            links.clone(),
            resolved.audit_endpoint(),
        )?;
        let plan = CriticalEventPlan::new(
            CriticalOperation::LeaseTransition(LeaseTransitionTarget::Released),
            intent,
        )
        .map_err(|_| RequestFailure::poison_without_terminal(critical_plan_error()))?;
        let endpoint = resolved.audit_endpoint.clone();
        let outcome_links = links.clone();
        let failure_links = links;
        let result = execute_critical(
            &self.ledger,
            self.events.fingerprinter(),
            plan,
            || match self.complete_explicit_release(request_id, token, connection_id) {
                Ok(token) => CriticalActionReport::Succeeded {
                    value: token,
                    effect: DefiniteEffectDisposition::Performed,
                },
                Err(error) => CriticalActionReport::Failed {
                    effect: error.effect,
                    error,
                },
            },
            |_, effect| {
                self.lease_outcome_draft(
                    EventSeverity::Info,
                    outcome_links,
                    LeasePayloadDraft::released(
                        EventAction::LeaseRelease,
                        effect.into(),
                        audit_endpoint(&endpoint),
                    ),
                )
            },
            |error, effect| {
                self.lease_failure_draft(
                    failure_links,
                    EventAction::LeaseRelease,
                    error.diagnostic,
                    effect,
                    &endpoint,
                )
            },
        );
        self.map_critical_lease_result(result, RuntimeReceiptState::Completed, |token| {
            RuntimeResult::LeaseReleased {
                instance_id: token.instance_id(),
                lease_id: token.lease_id(),
            }
        })
    }

    fn append_request_lifecycle(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_id: InstanceId,
        action: EventAction,
        run_links: Option<RuntimeRunLinks>,
    ) -> Result<(), RequestFailure> {
        let links =
            self.append_client_command_intent(original, request, instance_id, action, run_links)?;
        self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            CommandPayloadDraft::validated(
                action,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            ),
        )?;
        Ok(())
    }

    fn append_scheduled_request_lifecycle(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_id: InstanceId,
        run_links: RuntimeRunLinks,
        execution_provenance: ExecutionBackendProvenance,
    ) -> Result<(), RequestFailure> {
        let expected_origin = scheduled_request_transport_origin(execution_provenance);
        if (original.actor(), original.source()) != expected_origin {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "policy_task_request_origin_mismatch",
                    "append_scheduled_request_lifecycle",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        if execution_provenance == ExecutionBackendProvenance::FixtureSimulation {
            return self.append_request_lifecycle(
                original,
                request,
                instance_id,
                EventAction::RuntimeTaskRun,
                Some(run_links),
            );
        }
        let links =
            run_links.apply(
                self.events
                    .request_links(request, Some(instance_id), None, None),
            );
        self.append_event(
            EventSeverity::Info,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links.clone(),
            CommandPayloadDraft::received(EventAction::RuntimeTaskRun, AuditInput::new()),
        )?;
        self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            CommandPayloadDraft::validated(
                EventAction::RuntimeTaskRun,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            ),
        )?;
        Ok(())
    }

    fn append_client_command_intent(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_id: InstanceId,
        action: EventAction,
        run_links: Option<RuntimeRunLinks>,
    ) -> Result<EventLinksDraft, RequestFailure> {
        self.validate_c4_client_source(original)?;
        let (source, module, payload) = match original.source() {
            EventSource::Cli => (
                EventSource::Cli,
                OriginModule::Actingctl,
                ClientPayloadDraft::cli_command(action, AuditInput::new()),
            ),
            EventSource::Lab => (
                EventSource::Lab,
                OriginModule::Actinglab,
                ClientPayloadDraft::lab_request(action, AuditInput::new()),
            ),
            EventSource::Ui => (
                EventSource::Ui,
                OriginModule::Runtime,
                ClientPayloadDraft::ui_action(action, AuditInput::new()),
            ),
            EventSource::Adapter
            | EventSource::Runtime
            | EventSource::Scheduler
            | EventSource::Device
            | EventSource::System => {
                return Err(RequestFailure::request(
                    RuntimeHostError::request(
                        "c4_client_source_unsupported",
                        "append_request_lifecycle",
                        RuntimeErrorCode::InvalidRequest,
                    ),
                    RuntimeReceiptState::Denied,
                    None,
                ));
            }
        };
        let mut links = self
            .events
            .request_links(request, Some(instance_id), None, None);
        if let Some(run_links) = run_links {
            links = run_links.apply(links);
        }
        self.append_event(
            EventSeverity::Info,
            source,
            module,
            original.actor(),
            links.clone(),
            payload,
        )?;
        self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            CommandPayloadDraft::received(action, AuditInput::new()),
        )?;
        Ok(links)
    }

    fn validate_c4_client_source(&self, request: &RuntimeRequest) -> Result<(), RequestFailure> {
        if matches!(
            request.source(),
            EventSource::Cli | EventSource::Lab | EventSource::Ui
        ) {
            return Ok(());
        }
        Err(RequestFailure::request(
            RuntimeHostError::request(
                "c4_client_source_unsupported",
                "append_request_lifecycle",
                RuntimeErrorCode::InvalidRequest,
            ),
            RuntimeReceiptState::Denied,
            None,
        ))
    }

    fn cleanup_composite_failure(
        &self,
        token: LeaseToken,
        connection_id: ConnectionId,
        failure: RequestFailure,
    ) -> RequestFailure {
        match self.retain_unconfirmed_resources(&failure.error, EventLinksDraft::default()) {
            Ok(true) => return failure,
            Ok(false) => {}
            Err(retain_failure) => return failure.replace_with_poison(retain_failure),
        }
        match self.cleanup_token(&token, connection_id, LeaseReleaseReason::BackendFailure) {
            Ok(()) => failure,
            Err(error) => failure.replace_with_poison(error),
        }
    }

    fn cleanup_composite_failure_with_run_links(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: LeaseToken,
        connection_id: ConnectionId,
        run_links: Option<RuntimeRunLinks>,
        failure: RequestFailure,
    ) -> RequestFailure {
        match self.retain_unconfirmed_resources(&failure.error, EventLinksDraft::default()) {
            Ok(true) => return failure,
            Ok(false) => {}
            Err(retain_failure) => return failure.replace_with_poison(retain_failure),
        }
        let cleanup = match run_links {
            Some(run_links) => self.cleanup_scheduled_failure_with_run_links(
                request,
                &token,
                connection_id,
                run_links,
            ),
            None => self.cleanup_token(&token, connection_id, LeaseReleaseReason::BackendFailure),
        };
        match cleanup {
            Ok(()) => failure,
            Err(error) => failure.replace_with_poison(error),
        }
    }

    fn validated_instance(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> Result<RegisteredInstance, RequestFailure> {
        let validation = lock(&self.scheduler, "validate_runtime_lease").and_then(|scheduler| {
            scheduler
                .validate_write(token, connection_id, self.monotonic_ms()?)
                .map_err(|error| RuntimeHostError::scheduler("validate_runtime_lease", &error))
        });
        if let Err(error) = validation {
            return Err(self.scheduler_denied_error(
                request,
                Some(token.instance_id()),
                Some(token.lease_id()),
                "",
                error,
            )?);
        }
        lock(&self.registered_instances, "read_instance_registry")?
            .get(&token.instance_id())
            .cloned()
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "active_lease_instance_missing",
                    "read_instance_registry",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })
    }

    fn finish_destructive_input(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> Result<(), RequestFailure> {
        lock(&self.scheduler, "finish_destructive_input")?
            .finish_destructive_step(token, connection_id)
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                    "finish_destructive_input",
                    &error,
                ))
            })
    }

    fn mark_resources_in_use(&self) -> RuntimeHostResult<MutexGuard<'_, OwnerGuard>> {
        let mut owner = lock(&self.owner, "mark_owner_resources_in_use")?;
        owner.set_resource_disposition(OwnerResourceDisposition::InUse)?;
        Ok(owner)
    }

    fn record_owner_resource_close(&self) -> RuntimeHostResult<OwnerResourceDisposition> {
        // The same owner lock spans InUse and session registration on every acquisition.
        let mut owner = lock(&self.owner, "record_owner_resource_close")?;
        if let Some(disposition) = owner.retained_resource_disposition()? {
            return Ok(disposition);
        }
        let has_sessions = self.execution.has_sessions().map_err(|error| {
            RuntimeHostError::execution("inspect_remaining_execution_sessions", &error)
        })?;
        let disposition = if has_sessions {
            OwnerResourceDisposition::InUse
        } else {
            OwnerResourceDisposition::ConfirmedClosed
        };
        owner.set_resource_disposition(disposition)?;
        Ok(disposition)
    }

    fn retain_unconfirmed_resources(
        &self,
        error: &RuntimeHostError,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<bool> {
        if error.lifecycle.resource_quiescence != Some(ResourceQuiescence::Unconfirmed) {
            return Ok(false);
        }
        let error = error.clone().into_fatal();
        let lifecycle_result = self.append_lifecycle_failure(
            RuntimeLifecycleFailureStage::SessionClose,
            RuntimeLifecycleFailure::Host(&error),
            links,
            None,
        );
        let retain_result = lock(&self.owner, "retain_unconfirmed_owner")
            .and_then(|mut owner| owner.retain_unconfirmed());
        let fatal_result = self.fatal.mark(error.clone());
        lifecycle_result?;
        retain_result?;
        fatal_result?;
        Ok(true)
    }

    fn close_instance_resources(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
        links: EventLinksDraft,
    ) -> Result<(), RequestFailure> {
        self.close_instance_resources_result(token, connection_id, links)?
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::execution(
                    "close_execution_session",
                    &error,
                ))
            })
    }

    fn close_instance_resources_result(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
        links: EventLinksDraft,
    ) -> Result<Result<(), ExecutionKernelError>, RequestFailure> {
        if let Some(error) = self
            .execution
            .unconfirmed_instance_close_error(token.instance_id())
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::execution(
                    "read_execution_close_result",
                    &error,
                ))
            })?
        {
            return Ok(Err(error));
        }
        let has_session = self
            .execution
            .has_owned_resources(token.instance_id())
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::execution(
                    "inspect_execution_session",
                    &error,
                ))
            })?;
        if !has_session {
            self.record_owner_resource_close()?;
            return Ok(Ok(()));
        }
        lock(&self.scheduler, "begin_destructive_resource_close")?
            .begin_resource_close(token, connection_id, self.monotonic_ms()?)
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                    "begin_destructive_resource_close",
                    &error,
                ))
            })?;

        match self
            .execution
            .close_instance(token.instance_id(), DeviceCloseAuthority::FencedDeviceWrite)
        {
            Ok(outcome) => {
                let owner_disposition = self.record_owner_resource_close()?;
                self.append_lifecycle_observed(
                    RuntimeLifecyclePhase::ResourceQuiescence {
                        instance_id: token.instance_id(),
                        resource_count: outcome.resource_count(),
                        quiescence: outcome.quiescence(),
                        owner_disposition,
                    },
                    links,
                )
                .map_err(RequestFailure::poison_without_terminal)?;
                lock(&self.scheduler, "finish_destructive_resource_close")?
                    .finish_destructive_step(token, connection_id)
                    .map_err(|error| {
                        RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                            "finish_destructive_resource_close",
                            &error,
                        ))
                    })?;
                Ok(Ok(()))
            }
            Err(execution_error) => {
                let error =
                    RuntimeHostError::execution("close_execution_session", &execution_error);
                let lifecycle_result = self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::SessionClose,
                    RuntimeLifecycleFailure::Host(&error),
                    links,
                    None,
                );
                if error.lifecycle.resource_quiescence == Some(ResourceQuiescence::Unconfirmed) {
                    let retain_result = lock(&self.owner, "retain_unconfirmed_owner")
                        .and_then(|mut owner| owner.retain_unconfirmed());
                    let fatal_result = self.fatal.mark(error.clone());
                    lifecycle_result.map_err(RequestFailure::poison_without_terminal)?;
                    retain_result.map_err(RequestFailure::poison_without_terminal)?;
                    fatal_result.map_err(RequestFailure::poison_without_terminal)?;
                    return Ok(Err(execution_error));
                }
                lifecycle_result.map_err(RequestFailure::poison_without_terminal)?;
                self.record_owner_resource_close()?;
                lock(&self.scheduler, "finish_destructive_resource_close")?
                    .finish_destructive_step(token, connection_id)
                    .map_err(|scheduler_error| {
                        RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                            "finish_destructive_resource_close",
                            &scheduler_error,
                        ))
                    })?;
                Ok(Err(execution_error))
            }
        }
    }

    /// The instance admission guard excludes capture registration and business native calls.
    fn close_retained_instance_while_guarded(
        &self,
        instance_id: InstanceId,
        links: EventLinksDraft,
        reuse_active_lease: bool,
        admission: &MutexGuard<'_, ()>,
    ) -> RuntimeHostResult<Result<(), ExecutionKernelError>> {
        if !self
            .execution
            .has_owned_resources(instance_id)
            .map_err(|error| RuntimeHostError::execution("inspect_retained_session", &error))?
        {
            return Ok(Ok(()));
        }
        let active = lock(&self.scheduler, "read_resource_close_lease")?
            .active_tokens()
            .into_iter()
            .find(|token| token.instance_id() == instance_id);
        let (token, connection_id, acquired) = if let Some(token) = active {
            if !reuse_active_lease {
                return Err(RuntimeHostError::scheduler(
                    "acquire_resource_close_lease",
                    &SchedulerError::Busy {
                        holder_id: token.holder_id(),
                        lease_id: token.lease_id(),
                        expires_at_monotonic_ms: token.expires_at_monotonic_ms(),
                    },
                ));
            }
            let connection_id = lock(&self.scheduler, "read_resource_close_connection")?
                .connection_for_token(&token)
                .map_err(|error| {
                    RuntimeHostError::scheduler("read_resource_close_connection", &error)
                })?;
            (token, connection_id, false)
        } else {
            let resolved = lock(&self.registered_instances, "read_resource_close_instance")?
                .get(&instance_id)
                .cloned()
                .ok_or_else(|| {
                    RuntimeHostError::fatal(
                        "resource_close_instance_missing",
                        "acquire_resource_close_lease",
                        RuntimeErrorCode::RuntimeFatal,
                    )
                })?;
            let request_id = self
                .events
                .issuer()
                .mint_request_id()
                .map_err(|_| runtime_identifier_error())?;
            let holder_id = self
                .events
                .issuer()
                .mint_holder_id()
                .map_err(|_| runtime_identifier_error())?;
            let connection_id =
                ConnectionId::new(RESOURCE_CLOSE_CONNECTION_VALUE).map_err(|error| {
                    RuntimeHostError::scheduler("build_resource_close_connection", &error)
                })?;
            let preparation = lock(&self.scheduler, "prepare_resource_close_lease")?
                .prepare_resource_close(
                    *request_id.transport(),
                    instance_id,
                    *holder_id.transport(),
                    connection_id,
                    self.monotonic_ms()?,
                )
                .map_err(|error| {
                    RuntimeHostError::scheduler("prepare_resource_close_lease", &error)
                })?;
            let token = preparation.token().clone();
            let grant_links = self
                .events
                .synthetic_links(&token, self.events.action_id()?)?
                .with_request_id(request_id);
            self.grant_prepared_lease_with_links(
                &resolved,
                preparation,
                grant_links,
                CapacityUse::Drain,
            )
            .map_err(|failure| *failure.error)?;
            (token, connection_id, true)
        };
        let result = self
            .close_instance_resources_result(&token, connection_id, links)
            .map_err(|failure| *failure.error)?;
        let confirmed = result
            .as_ref()
            .err()
            .is_none_or(|error| error.resource_quiescence() == Some(ResourceQuiescence::Confirmed));
        if acquired && confirmed {
            self.cleanup_token_inner(
                &token,
                connection_id,
                LeaseReleaseReason::HostShutdown,
                None,
                Some(admission),
            )?;
        }
        Ok(result)
    }

    fn finish_input_failure(
        &self,
        primary: ExecutionKernelError,
        token: &LeaseToken,
        connection_id: ConnectionId,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<ExecutionKernelError> {
        let close_result: RuntimeHostResult<Result<(), ExecutionKernelError>> = (|| {
            let instance_guard = self
                .instance_guard(token.instance_id())
                .map_err(|failure| *failure.error)?;
            let _admission = lock(&instance_guard, "lock_instance_admission")?;
            self.finish_destructive_input(token, connection_id)
                .map_err(|failure| *failure.error)?;
            self.close_instance_resources_result(token, connection_id, links.clone())
                .map_err(|failure| *failure.error)
        })();
        match close_result {
            Ok(Ok(())) => Ok(primary),
            Ok(Err(cleanup)) => Ok(ExecutionKernelError::merge_cleanup(primary, cleanup)),
            Err(cleanup) => {
                let primary = RuntimeHostError::execution("execute_input_backend", &primary);
                self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::SessionClose,
                    RuntimeLifecycleFailure::Host(&primary),
                    links.clone(),
                    None,
                )?;
                let cleanup = cleanup.into_fatal();
                self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::SessionClose,
                    RuntimeLifecycleFailure::Host(&cleanup),
                    links,
                    None,
                )?;
                lock(&self.owner, "retain_unconfirmed_owner")?.retain_unconfirmed()?;
                self.fatal.mark(cleanup.clone())?;
                Err(cleanup)
            }
        }
    }

    fn finish_capture_failure_while_guarded(
        &self,
        primary: ExecutionKernelError,
        links: EventLinksDraft,
        admission: &MutexGuard<'_, ()>,
    ) -> RuntimeHostResult<ExecutionKernelError> {
        if primary.code() == "execution_session_close_pending" {
            return Ok(primary);
        }
        let Some(instance_id) = primary.instance_id() else {
            return Ok(primary);
        };
        match self.close_retained_instance_while_guarded(
            instance_id,
            links.clone(),
            true,
            admission,
        ) {
            Ok(Ok(())) => Ok(primary),
            Ok(Err(cleanup)) => Ok(ExecutionKernelError::merge_cleanup(primary, cleanup)),
            Err(cleanup) => {
                let primary = RuntimeHostError::execution("finish_capture_failure", &primary);
                self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::SessionClose,
                    RuntimeLifecycleFailure::Host(&primary),
                    links.clone(),
                    None,
                )?;
                let cleanup = cleanup.into_fatal();
                self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::SessionClose,
                    RuntimeLifecycleFailure::Host(&cleanup),
                    links,
                    None,
                )?;
                lock(&self.owner, "retain_unconfirmed_owner")?.retain_unconfirmed()?;
                self.fatal.mark(cleanup.clone())?;
                Err(cleanup)
            }
        }
    }

    fn transfer_preempted_if_ready(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> Result<bool, RequestFailure> {
        let instance_guard = self.instance_guard(token.instance_id())?;
        let admission = lock(&instance_guard, "lock_instance_admission")?;
        self.transfer_preempted_while_guarded(token, connection_id, &admission)
    }

    fn transfer_preempted_while_guarded(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
        _admission: &MutexGuard<'_, ()>,
    ) -> Result<bool, RequestFailure> {
        self.expire_queued_for_instance(token.instance_id())?;
        let transfer = lock(&self.scheduler, "prepare_preempted_transfer")?
            .prepare_transfer(
                token,
                connection_id,
                LeaseTransferReason::Preempted,
                None,
                self.monotonic_ms()?,
            )
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                    "prepare_preempted_transfer",
                    &error,
                ))
            })?;
        match transfer {
            TransferPreparation::NoCandidate => Ok(false),
            TransferPreparation::Ready(prepared) => {
                if !self.capacity_allows_transfer(token)? {
                    self.cleanup_token_inner(
                        token,
                        connection_id,
                        LeaseReleaseReason::Preempted,
                        None,
                        Some(_admission),
                    )?;
                    return Ok(true);
                }
                self.perform_transfer(prepared).map(|_| true)
            }
            TransferPreparation::Deferred => Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "preempted_transfer_remained_destructive",
                    "prepare_preempted_transfer",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            )),
        }
    }

    fn commit_acquire(&self, preparation: LeasePreparation) -> Result<LeaseToken, ActionFailure> {
        let token = preparation.token().clone();
        let now = self.monotonic_ms().map_err(ActionFailure::poison)?;
        let mut scheduler = lock(&self.scheduler, "commit_lease").map_err(ActionFailure::poison)?;
        if let Err(error) = scheduler.commit_acquire(preparation, now) {
            return Err(ActionFailure::scheduler(RuntimeHostError::scheduler(
                "commit_lease",
                &error,
            )));
        }
        let protected = scheduler.protected_instance_ids(now);
        if let Err(error) = lock(&self.owner, "update_owner_file")
            .and_then(|mut owner| owner.set_active_instances(protected))
        {
            let rollback = scheduler.rollback_lease(&token).err();
            let rollback_error = rollback
                .map(|rollback| RuntimeHostError::scheduler("rollback_lease", &rollback))
                .unwrap_or(error);
            return Err(ActionFailure::poison(rollback_error));
        }
        Ok(token)
    }

    fn complete_explicit_release(
        &self,
        request_id: RequestId,
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> Result<LeaseToken, ActionFailure> {
        {
            let mut scheduler =
                lock(&self.scheduler, "release_lease").map_err(ActionFailure::poison)?;
            scheduler
                .release(
                    request_id,
                    token,
                    connection_id,
                    self.monotonic_ms().map_err(ActionFailure::poison)?,
                )
                .map_err(|error| {
                    ActionFailure::scheduler(RuntimeHostError::scheduler("release_lease", &error))
                })?;
        }
        self.persist_active_instances()
            .map_err(ActionFailure::poison)?;
        Ok(token.clone())
    }

    fn release_via_transfer(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        resolved: &RegisteredInstance,
        prepared: Box<PreparedLeaseTransfer>,
        run_links: Option<RuntimeRunLinks>,
    ) -> Result<OperationSuccess, RequestFailure> {
        let action_id = self
            .events
            .action_id()
            .map_err(RequestFailure::poison_without_terminal)?;
        let mut links = self.events.request_links(
            request,
            Some(token.instance_id()),
            Some(token.lease_id()),
            Some(action_id),
        );
        if let Some(run_links) = run_links {
            links = run_links.apply(links);
        }
        self.append_event(
            EventSeverity::Info,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links.clone(),
            LeasePayloadDraft::transition_intent(
                EventAction::LeaseRelease,
                audit_endpoint(resolved.audit_endpoint()),
            ),
        )?;
        self.perform_transfer(prepared)?;
        let released = self.append_event(
            EventSeverity::Info,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links,
            LeasePayloadDraft::released(
                EventAction::LeaseRelease,
                EffectDisposition::Performed,
                audit_endpoint(resolved.audit_endpoint()),
            ),
        )?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&released)),
            result: RuntimeResult::LeaseReleased {
                instance_id: token.instance_id(),
                lease_id: token.lease_id(),
            },
        })
    }

    fn cleanup_token(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
        reason: LeaseReleaseReason,
    ) -> RuntimeHostResult<()> {
        self.cleanup_token_inner(token, connection_id, reason, None, None)
    }

    /// The sole producer of a run-linked scheduled failure cleanup.
    ///
    /// Its fixed `BackendFailure` reason is intentionally not caller-selectable: the resulting
    /// full run chain is the bounded proof consumed by startup settlement recovery.
    fn cleanup_scheduled_failure_with_run_links(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        connection_id: ConnectionId,
        run_links: RuntimeRunLinks,
    ) -> RuntimeHostResult<()> {
        self.cleanup_token_inner(
            token,
            connection_id,
            LeaseReleaseReason::BackendFailure,
            Some((request, run_links)),
            None,
        )
    }

    fn cleanup_token_inner(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
        reason: LeaseReleaseReason,
        request_links: Option<(&ValidatedRuntimeRequest<'_>, RuntimeRunLinks)>,
        admission: Option<&MutexGuard<'_, ()>>,
    ) -> RuntimeHostResult<()> {
        let resolved = lock(&self.registered_instances, "read_instance_registry")?
            .get(&token.instance_id())
            .cloned();
        let Some(resolved) = resolved else {
            let active = lock(&self.scheduler, "check_cleanup_lease")?
                .active_tokens()
                .into_iter()
                .any(|active| active == *token);
            return if active {
                Err(RuntimeHostError::fatal(
                    "active_lease_instance_missing",
                    "cleanup_runtime_connection",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            } else {
                Ok(())
            };
        };
        let instance_guard = self
            .instance_guard(token.instance_id())
            .map_err(|failure| *failure.error)?;
        let _owned_admission = if admission.is_none() {
            Some(lock(&instance_guard, "lock_instance_admission")?)
        } else {
            None
        };
        self.expire_queued_for_instance(token.instance_id())
            .map_err(|failure| *failure.error)?;
        self.close_instance_resources(token, connection_id, EventLinksDraft::default())
            .map_err(|failure| *failure.error)?;
        let transfer_reason = match reason {
            LeaseReleaseReason::Disconnect => Some(LeaseTransferReason::Disconnect),
            LeaseReleaseReason::Expired => Some(LeaseTransferReason::Expired),
            LeaseReleaseReason::Explicit
            | LeaseReleaseReason::Preempted
            | LeaseReleaseReason::BackendFailure
            | LeaseReleaseReason::HostShutdown => None,
        };
        if let Some(transfer_reason) = transfer_reason {
            let transfer = lock(&self.scheduler, "prepare_cleanup_transfer")?
                .prepare_transfer(
                    token,
                    connection_id,
                    transfer_reason,
                    None,
                    self.monotonic_ms()?,
                )
                .map_err(|error| RuntimeHostError::scheduler("prepare_cleanup_transfer", &error))?;
            match transfer {
                TransferPreparation::Ready(prepared) => {
                    if self.capacity_allows_transfer(token)? {
                        self.cleanup_via_transfer(token, &resolved, reason, prepared)?;
                        return Ok(());
                    }
                }
                TransferPreparation::Deferred if reason == LeaseReleaseReason::Expired => {
                    return Ok(());
                }
                TransferPreparation::Deferred => {
                    return Err(RuntimeHostError::fatal(
                        "cleanup_transfer_remained_destructive",
                        "prepare_cleanup_transfer",
                        RuntimeErrorCode::RuntimeFatal,
                    ));
                }
                TransferPreparation::NoCandidate => {}
            }
        }
        if matches!(
            reason,
            LeaseReleaseReason::BackendFailure | LeaseReleaseReason::HostShutdown
        ) {
            self.cancel_instance_queue(
                token.instance_id(),
                if reason == LeaseReleaseReason::BackendFailure {
                    DiagnosticCode::BackendOperationFailed
                } else {
                    DiagnosticCode::LeaseQueueDisconnected
                },
            )
            .map_err(|failure| *failure.error)?;
        }
        let action_id = self.events.action_id()?;
        let links = match request_links {
            Some((request, run_links)) => run_links.apply(self.events.request_links(
                request,
                Some(token.instance_id()),
                Some(token.lease_id()),
                Some(action_id),
            )),
            None => self.events.synthetic_links(token, action_id)?,
        };
        let target = if reason == LeaseReleaseReason::Expired {
            LeaseTransitionTarget::Expired
        } else {
            LeaseTransitionTarget::Released
        };
        let action = if reason == LeaseReleaseReason::Expired {
            EventAction::LeaseExpire
        } else {
            EventAction::LeaseRelease
        };
        let intent = self
            .lease_intent(action, links.clone(), resolved.audit_endpoint())
            .map_err(|failure| *failure.error)?;
        let plan = CriticalEventPlan::new(CriticalOperation::LeaseTransition(target), intent)
            .map_err(|_| critical_plan_error())?;
        let endpoint = resolved.audit_endpoint;
        let outcome_links = links.clone();
        let failure_links = links;
        let result = execute_critical(
            &self.ledger,
            self.events.fingerprinter(),
            plan,
            || {
                let released = {
                    let mut scheduler = match lock(&self.scheduler, "cleanup_runtime_lease") {
                        Ok(scheduler) => scheduler,
                        Err(error) => {
                            return CriticalActionReport::Failed {
                                error: ActionFailure::poison(error),
                                effect: EffectDisposition::Indeterminate,
                            };
                        }
                    };
                    if reason == LeaseReleaseReason::Expired {
                        let now = match self.monotonic_ms() {
                            Ok(now) => now,
                            Err(error) => {
                                return CriticalActionReport::Failed {
                                    error: ActionFailure::poison(error),
                                    effect: EffectDisposition::Indeterminate,
                                };
                            }
                        };
                        scheduler.expire_token(token, now)
                    } else {
                        scheduler.release_owned(token, connection_id, reason)
                    }
                };
                match released {
                    Ok(_) => match self.persist_active_instances() {
                        Ok(()) => CriticalActionReport::Succeeded {
                            value: token.clone(),
                            effect: DefiniteEffectDisposition::Performed,
                        },
                        Err(error) => CriticalActionReport::Failed {
                            effect: EffectDisposition::Indeterminate,
                            error: ActionFailure::poison(error),
                        },
                    },
                    Err(SchedulerError::LeaseMissing | SchedulerError::LeaseMismatch) => {
                        let already_removed =
                            lock(&self.scheduler, "check_scheduler_cleanup").map(|scheduler| {
                                !scheduler
                                    .active_tokens()
                                    .into_iter()
                                    .any(|active| active == *token)
                            });
                        match already_removed {
                            Ok(true) => CriticalActionReport::Succeeded {
                                value: token.clone(),
                                effect: DefiniteEffectDisposition::NotPerformed,
                            },
                            Ok(false) => CriticalActionReport::Failed {
                                error: ActionFailure::poison(RuntimeHostError::fatal(
                                    "scheduler_cleanup_state_mismatch",
                                    "cleanup_runtime_lease",
                                    RuntimeErrorCode::RuntimeFatal,
                                )),
                                effect: EffectDisposition::Indeterminate,
                            },
                            Err(error) => CriticalActionReport::Failed {
                                error: ActionFailure::poison(error),
                                effect: EffectDisposition::Indeterminate,
                            },
                        }
                    }
                    Err(error) => CriticalActionReport::Failed {
                        error: ActionFailure::scheduler(RuntimeHostError::scheduler(
                            "cleanup_runtime_lease",
                            &error,
                        )),
                        effect: EffectDisposition::NotPerformed,
                    },
                }
            },
            |_, effect| {
                self.lease_outcome_draft(
                    EventSeverity::Info,
                    outcome_links,
                    if reason == LeaseReleaseReason::Expired {
                        LeasePayloadDraft::expired(action, effect.into(), audit_endpoint(&endpoint))
                    } else {
                        LeasePayloadDraft::released(
                            action,
                            effect.into(),
                            audit_endpoint(&endpoint),
                        )
                    },
                )
            },
            |error, effect| {
                self.lease_failure_draft(failure_links, action, error.diagnostic, effect, &endpoint)
            },
        );
        match result {
            Ok(_) => Ok(()),
            Err(CriticalExecutionError::Action { error, outcome, .. }) => {
                let _ = error
                    .error
                    .lifecycle
                    .recorded_event
                    .set(*outcome.event_id());
                if error.poison_runtime {
                    self.fatal.mark(error.error.clone())?;
                }
                Err(error.error)
            }
            Err(error) => {
                let error = critical_execution_error(&error);
                self.fatal.mark(error.clone())?;
                Err(error)
            }
        }
    }

    fn cleanup_via_transfer(
        &self,
        token: &LeaseToken,
        resolved: &RegisteredInstance,
        reason: LeaseReleaseReason,
        prepared: Box<PreparedLeaseTransfer>,
    ) -> RuntimeHostResult<()> {
        let action_id = self.events.action_id()?;
        let links = self.events.synthetic_links(token, action_id)?;
        let action = if reason == LeaseReleaseReason::Expired {
            EventAction::LeaseExpire
        } else {
            EventAction::LeaseRelease
        };
        self.append_event(
            EventSeverity::Info,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links.clone(),
            LeasePayloadDraft::transition_intent(action, audit_endpoint(resolved.audit_endpoint())),
        )
        .map_err(|failure| *failure.error)?;
        self.perform_transfer(prepared)
            .map_err(|failure| *failure.error)?;
        self.append_event(
            EventSeverity::Info,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links,
            if reason == LeaseReleaseReason::Expired {
                LeasePayloadDraft::expired(
                    action,
                    EffectDisposition::Performed,
                    audit_endpoint(resolved.audit_endpoint()),
                )
            } else {
                LeasePayloadDraft::released(
                    action,
                    EffectDisposition::Performed,
                    audit_endpoint(resolved.audit_endpoint()),
                )
            },
        )
        .map_err(|failure| *failure.error)?;
        Ok(())
    }

    #[cfg(test)]
    fn durable_lease_expiry_terminal_for_test(
        &self,
        token: &LeaseToken,
    ) -> RuntimeHostResult<Option<TerminalEvent>> {
        let through_sequence = self
            .ledger
            .latest_sequence()
            .map_err(|_| ledger_error("read_test_lease_expiry_position"))?;
        let mut selected_terminal = None;
        for event_type in [EventType::LeaseExpired, EventType::LeaseReleased] {
            let events = self
                .ledger
                .query_page(
                    EventQuery {
                        to_sequence: Some(through_sequence),
                        event_type: Some(event_type),
                        instance_id: Some(token.instance_id()),
                        lease_id: Some(token.lease_id()),
                        ..EventQuery::default()
                    },
                    0,
                    through_sequence,
                    2,
                )
                .map_err(|_| ledger_error("read_test_lease_expiry_terminal"))?;
            let event = match events.as_slice() {
                [] => None,
                [event] => Some(terminal(event)),
                _ => {
                    return Err(RuntimeHostError::fatal(
                        "test_lease_expiry_terminal_not_unique",
                        "expire_lease_once_for_test",
                        RuntimeErrorCode::RuntimeFatal,
                    ));
                }
            };
            // LeaseExpired is the exact scan result. LeaseReleased is the permitted fallback
            // for an already-cleaned token and must not replace a durable expiry on replay.
            selected_terminal = selected_terminal.or(event);
        }
        Ok(selected_terminal)
    }

    #[cfg(test)]
    fn active_lease_token_for_test(
        &self,
        token: &LeaseToken,
    ) -> RuntimeHostResult<Option<LeaseToken>> {
        lock(&self.scheduler, "read_test_lease_expiry_token").map(|scheduler| {
            scheduler.active_tokens().into_iter().find(|active| {
                active.instance_id() == token.instance_id() && active.lease_id() == token.lease_id()
            })
        })
    }

    #[cfg(test)]
    fn record_completed_lease_expiry_for_test(
        &self,
        token: &LeaseToken,
    ) -> RuntimeHostResult<TerminalEvent> {
        let terminal = self
            .durable_lease_expiry_terminal_for_test(token)?
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    "test_lease_expiry_terminal_missing",
                    "expire_lease_once_for_test",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
        if self.active_lease_token_for_test(token)?.is_some() {
            return Err(RuntimeHostError::fatal(
                "test_lease_expiry_token_cleanup_incomplete",
                "expire_lease_once_for_test",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        let mut checkpoints = lock(
            &self.lease_expiry_test_checkpoints,
            "record_test_lease_expiry_checkpoint",
        )?;
        if checkpoints
            .iter()
            .any(|checkpoint| checkpoint.token == *token)
        {
            return Err(RuntimeHostError::fatal(
                "test_lease_expiry_checkpoint_duplicate",
                "expire_lease_once_for_test",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        if checkpoints
            .iter()
            .any(|checkpoint| checkpoint.terminal == terminal)
        {
            return Err(RuntimeHostError::fatal(
                "test_lease_expiry_checkpoint_inconsistent",
                "expire_lease_once_for_test",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        checkpoints.push(LeaseExpiryTestCheckpoint {
            token: token.clone(),
            terminal,
        });
        Ok(terminal)
    }

    #[cfg(test)]
    fn replay_lease_expiry_checkpoint_for_test(
        &self,
        token: &LeaseToken,
    ) -> RuntimeHostResult<Option<TerminalEvent>> {
        let checkpoint = {
            let checkpoints = lock(
                &self.lease_expiry_test_checkpoints,
                "read_test_lease_expiry_checkpoint",
            )?;
            let mut candidates = checkpoints.iter().filter(|checkpoint| {
                lease_token_identity_match_count(&checkpoint.token, token) >= 4
            });
            let Some(checkpoint) = candidates.next() else {
                return Ok(None);
            };
            if candidates.next().is_some() {
                return Err(RuntimeHostError::fatal(
                    "test_lease_expiry_checkpoint_duplicate",
                    "expire_lease_once_for_test",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            if checkpoint.token != *token {
                return Err(RuntimeHostError::fatal(
                    "test_lease_expiry_token_identity_mismatch",
                    "expire_lease_once_for_test",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            checkpoint.clone()
        };
        let durable_terminal = self
            .durable_lease_expiry_terminal_for_test(&checkpoint.token)?
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    "test_lease_expiry_checkpoint_missing",
                    "expire_lease_once_for_test",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
        if durable_terminal != checkpoint.terminal {
            return Err(RuntimeHostError::fatal(
                "test_lease_expiry_checkpoint_inconsistent",
                "expire_lease_once_for_test",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        if self
            .active_lease_token_for_test(&checkpoint.token)?
            .is_some()
        {
            return Err(RuntimeHostError::fatal(
                "test_lease_expiry_terminal_token_still_active",
                "expire_lease_once_for_test",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        Ok(Some(checkpoint.terminal))
    }

    fn expire_due_leases(&self) -> RuntimeHostResult<()> {
        #[cfg(test)]
        let _scan = lock(
            &self.lease_expiry_scan_test_gate,
            "serialize_test_lease_expiry_scan",
        )?;
        self.expire_all_queued_runtime()?;
        let now = self.monotonic_ms()?;
        let (due, cooldowns_cleared) = {
            let mut scheduler = lock(&self.scheduler, "scan_expired_leases")?;
            let due = scheduler.due_tokens(now);
            let cooldowns_cleared = scheduler.clear_elapsed_cooldowns(now);
            (due, cooldowns_cleared)
        };
        if cooldowns_cleared {
            self.persist_active_instances()?;
        }
        for token in due {
            let connection_id = lock(&self.scheduler, "read_lease_connection")?
                .connection_for_token(&token)
                .map_err(|error| RuntimeHostError::scheduler("read_lease_connection", &error))?;
            self.cleanup_token(&token, connection_id, LeaseReleaseReason::Expired)?;
            #[cfg(test)]
            self.record_completed_lease_expiry_for_test(&token)?;
        }
        Ok(())
    }

    fn expire_instance_if_due(&self, instance_id: InstanceId) -> Result<(), RequestFailure> {
        let now = self
            .monotonic_ms()
            .map_err(RequestFailure::poison_without_terminal)?;
        let due = lock(&self.scheduler, "scan_instance_expiry")?
            .due_tokens(now)
            .into_iter()
            .find(|token| token.instance_id() == instance_id);
        if let Some(token) = due {
            let connection_id = lock(&self.scheduler, "read_lease_connection")?
                .connection_for_token(&token)
                .map_err(|error| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                        "read_lease_connection",
                        &error,
                    ))
                })?;
            self.cleanup_token(&token, connection_id, LeaseReleaseReason::Expired)
                .map_err(RequestFailure::poison_without_terminal)?;
        }
        Ok(())
    }

    fn cleanup_connection(
        &self,
        connection_id: ConnectionId,
        reason: LeaseReleaseReason,
    ) -> RuntimeHostResult<()> {
        lock(
            &self.governance_connections,
            "cleanup_governance_connection",
        )?
        .remove(&connection_id);
        let queued_instances = lock(&self.scheduler, "list_connection_queues")?
            .queued_instance_ids_for_connection(connection_id);
        for instance_id in queued_instances {
            let instance_guard = self
                .instance_guard(instance_id)
                .map_err(|failure| *failure.error)?;
            let _admission = lock(&instance_guard, "lock_instance_admission")?;
            let removed = lock(&self.scheduler, "cleanup_connection_queues")?
                .remove_queued_for_connection_on_instance(instance_id, connection_id)
                .map_err(|error| {
                    RuntimeHostError::scheduler("cleanup_connection_queues", &error)
                })?;
            for cancelled in removed {
                let context = self
                    .take_queued_context(&cancelled)
                    .map_err(|failure| *failure.error)?;
                self.append_queue_terminal(&context, DiagnosticCode::LeaseQueueDisconnected)
                    .map_err(|failure| *failure.error)?;
            }
        }
        let tokens =
            lock(&self.scheduler, "list_connection_leases")?.tokens_for_connection(connection_id);
        let mut failure = None;
        for token in tokens {
            if self.fatal.current()?.is_some() {
                break;
            }
            self.record_lifecycle_result(
                RuntimeLifecycleFailureStage::ConnectionCleanup,
                &mut failure,
                self.cleanup_token(&token, connection_id, reason),
            );
        }
        failure.map_or(Ok(()), Err)
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
                    if !failure.as_ref().is_some_and(|error| {
                        error.projection().code == RuntimeErrorCode::LedgerFailure
                    }) && let Err(append_error) = self.append_lifecycle_failure(
                        RuntimeLifecycleFailureStage::SessionClose,
                        RuntimeLifecycleFailure::Host(&session_error),
                        EventLinksDraft::default(),
                        None,
                    ) {
                        failure = Some(append_error);
                    }
                }
                if !failure
                    .as_ref()
                    .is_some_and(|error| error.projection().code == RuntimeErrorCode::LedgerFailure)
                {
                    // Session facts are separate; the returned error retains the kernel's reduction.
                    record_failure(
                        &mut failure,
                        Err(RuntimeHostError::execution(
                            "close_execution_kernel",
                            &error,
                        )),
                    );
                }
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

    fn resolve_instance(&self, instance_alias: &str) -> Result<RegisteredInstance, RequestFailure> {
        let registered = lock(&self.registered_instances, "read_instance_registry")?
            .values()
            .find(|instance| instance.instance_alias == instance_alias)
            .cloned()
            .ok_or_else(|| {
                RequestFailure::request(
                    RuntimeHostError::request(
                        "instance_unknown",
                        "resolve_runtime_instance",
                        RuntimeErrorCode::InstanceUnknown,
                    ),
                    RuntimeReceiptState::Denied,
                    None,
                )
            })?;
        self.resolve_registered_backend(&registered)?;
        Ok(registered)
    }

    fn resolve_registered_backend(
        &self,
        registered: &RegisteredInstance,
    ) -> Result<crate::ResolvedExecutionInstance, RequestFailure> {
        let resolved = self
            .execution
            .resolve(&registered.instance_alias)
            .map_err(|error| {
                if error.code() == "execution_instance_unknown" {
                    RequestFailure::request(
                        RuntimeHostError::request(
                            "instance_unknown",
                            "resolve_runtime_instance",
                            RuntimeErrorCode::InstanceUnknown,
                        ),
                        RuntimeReceiptState::Denied,
                        None,
                    )
                } else {
                    RequestFailure::poison_without_terminal(RuntimeHostError::execution(
                        "resolve_runtime_instance",
                        &error,
                    ))
                }
            })?;
        if resolved.instance_id() != registered.instance_id
            || resolved.audit_endpoint() != registered.audit_endpoint
            || resolved.provenance() != registered.provenance
        {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "runtime_instance_identity_mismatch",
                    "resolve_runtime_instance",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        Ok(resolved)
    }

    fn require_physical_instance_alias(&self, instance_alias: &str) -> Result<(), RequestFailure> {
        let instance = self.resolve_instance(instance_alias)?;
        self.require_physical_provenance(&instance)
    }

    fn require_physical_instance_id(&self, instance_id: InstanceId) -> Result<(), RequestFailure> {
        let instance = lock(&self.registered_instances, "read_instance_registry")?
            .get(&instance_id)
            .cloned()
            .ok_or_else(|| {
                RequestFailure::request(
                    RuntimeHostError::request(
                        "instance_unknown",
                        "require_physical_execution_backend",
                        RuntimeErrorCode::InstanceUnknown,
                    ),
                    RuntimeReceiptState::Denied,
                    None,
                )
            })?;
        self.require_physical_provenance(&instance)
    }

    fn require_physical_provenance(
        &self,
        instance: &RegisteredInstance,
    ) -> Result<(), RequestFailure> {
        if instance.provenance() == ExecutionBackendProvenance::PhysicalDevice {
            return Ok(());
        }
        Err(RequestFailure::request(
            RuntimeHostError::request(
                "fixture_execution_scope_forbidden",
                "require_physical_execution_backend",
                RuntimeErrorCode::InvalidRequest,
            ),
            RuntimeReceiptState::Denied,
            None,
        ))
    }

    fn instance_guard(&self, instance_id: InstanceId) -> Result<Arc<Mutex<()>>, RequestFailure> {
        let mut guards = lock(&self.admission_guards, "read_instance_admission")?;
        Ok(Arc::clone(
            guards
                .entry(instance_id)
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        ))
    }

    fn persist_active_instances(&self) -> RuntimeHostResult<()> {
        let now = self.monotonic_ms()?;
        let instances = lock(&self.scheduler, "read_active_instances")?.protected_instance_ids(now);
        lock(&self.owner, "update_owner_file")?.set_active_instances(instances)
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

    fn append_lease_requested(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        resolved: &RegisteredInstance,
    ) -> Result<PersistedEvent, RequestFailure> {
        let links = self
            .events
            .request_links(request, Some(resolved.instance_id()), None, None);
        self.append_event(
            EventSeverity::Info,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links,
            LeasePayloadDraft::requested(
                EventAction::LeaseAcquire,
                audit_endpoint(resolved.audit_endpoint()),
            ),
        )
    }

    fn append_scheduler_admitted(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        resolved: &RegisteredInstance,
        lease_id: Option<LeaseId>,
    ) -> Result<PersistedEvent, RequestFailure> {
        self.append_scheduler_admitted_for(
            request,
            resolved.instance_id(),
            lease_id,
            resolved.audit_endpoint(),
        )
    }

    fn append_scheduler_admitted_for_token(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        endpoint: &str,
    ) -> Result<PersistedEvent, RequestFailure> {
        self.append_scheduler_admitted_for(
            request,
            token.instance_id(),
            Some(token.lease_id()),
            endpoint,
        )
    }

    fn append_scheduler_admitted_for(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        instance_id: InstanceId,
        lease_id: Option<LeaseId>,
        endpoint: &str,
    ) -> Result<PersistedEvent, RequestFailure> {
        let links = self
            .events
            .request_links(request, Some(instance_id), lease_id, None);
        self.append_event(
            EventSeverity::Info,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links,
            SchedulerPayloadDraft::admitted(EventAction::ScheduleAdmit, audit_endpoint(endpoint)),
        )
    }

    fn scheduler_denied(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        resolved: &RegisteredInstance,
        lease_id: Option<LeaseId>,
        error: SchedulerError,
    ) -> RuntimeHostResult<RequestFailure> {
        self.scheduler_denied_error(
            request,
            Some(resolved.instance_id()),
            lease_id,
            resolved.audit_endpoint(),
            RuntimeHostError::scheduler("scheduler_admission", &error),
        )
    }

    fn scheduler_denied_error(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        instance_id: Option<InstanceId>,
        lease_id: Option<LeaseId>,
        endpoint: &str,
        error: RuntimeHostError,
    ) -> RuntimeHostResult<RequestFailure> {
        let links = self
            .events
            .request_links(request, instance_id, lease_id, None);
        let diagnostic = diagnostic_for_projection(error.projection());
        let event = self.append_event_raw(
            EventSeverity::Warning,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links,
            SchedulerPayloadDraft::denied(
                EventAction::ScheduleAdmit,
                diagnostic,
                audit_endpoint(endpoint),
            ),
        )?;
        Ok(RequestFailure {
            state: RuntimeReceiptState::Denied,
            terminal: Some(terminal(&event)),
            poison_runtime: error.is_fatal(),
            error: Box::new(error),
            task_failure: None,
        })
    }

    fn lease_intent(
        &self,
        action: EventAction,
        links: EventLinksDraft,
        endpoint: &str,
    ) -> Result<actingcommand_contract::SanitizedEventDraft, RequestFailure> {
        self.events
            .draft(
                EventSeverity::Info,
                EventSource::Scheduler,
                OriginModule::Scheduler,
                EventActor::Scheduler,
                links,
                LeasePayloadDraft::transition_intent(action, audit_endpoint(endpoint)),
            )
            .and_then(|draft| self.events.sanitize(draft))
            .map_err(RequestFailure::poison_without_terminal)
    }

    fn lease_outcome_draft(
        &self,
        severity: EventSeverity,
        links: EventLinksDraft,
        payload: LeasePayloadDraft,
    ) -> Result<actingcommand_contract::EventDraft, actingcommand_contract::SanitizationError> {
        self.events
            .draft(
                severity,
                EventSource::Scheduler,
                OriginModule::Scheduler,
                EventActor::Scheduler,
                links,
                payload,
            )
            .map_err(|_| actingcommand_contract::SanitizationError::fingerprinter_failure())
    }

    fn lease_failure_draft(
        &self,
        links: EventLinksDraft,
        action: EventAction,
        diagnostic: DiagnosticCode,
        effect: EffectDisposition,
        endpoint: &str,
    ) -> Result<actingcommand_contract::EventDraft, actingcommand_contract::SanitizationError> {
        self.lease_outcome_draft(
            EventSeverity::Error,
            links,
            LeasePayloadDraft::transition_failed(
                action,
                diagnostic,
                effect,
                audit_endpoint(endpoint),
            ),
        )
    }

    fn map_critical_lease_result<T>(
        &self,
        result: Result<
            actingcommand_ledger::critical::CriticalReceipt<LeaseToken>,
            CriticalExecutionError<ActionFailure>,
        >,
        state: RuntimeReceiptState,
        result_builder: T,
    ) -> Result<OperationSuccess, RequestFailure>
    where
        T: FnOnce(LeaseToken) -> RuntimeResult,
    {
        match result {
            Ok(receipt) => {
                let terminal = terminal(receipt.outcome());
                Ok(OperationSuccess {
                    state,
                    terminal: Some(terminal),
                    result: result_builder(receipt.into_value()),
                })
            }
            Err(CriticalExecutionError::Action { error, outcome, .. }) => Err(RequestFailure {
                state: RuntimeReceiptState::Failed,
                terminal: Some(terminal(&outcome)),
                poison_runtime: error.poison_runtime,
                error: Box::new(error.error),
                task_failure: None,
            }),
            Err(error) => Err(RequestFailure::poison_without_terminal(
                critical_execution_error(&error),
            )),
        }
    }
}

impl RequestFailure {
    fn request(
        error: RuntimeHostError,
        state: RuntimeReceiptState,
        terminal: Option<TerminalEvent>,
    ) -> Self {
        Self {
            state,
            terminal,
            error: Box::new(error),
            poison_runtime: false,
            task_failure: None,
        }
    }

    fn poison(error: RuntimeHostError, terminal: Option<TerminalEvent>) -> Self {
        Self {
            state: RuntimeReceiptState::Failed,
            terminal,
            error: Box::new(error.into_fatal()),
            poison_runtime: true,
            task_failure: None,
        }
    }

    fn poison_without_terminal(error: RuntimeHostError) -> Self {
        Self::poison(error, None)
    }

    fn replace_with_poison(self, error: RuntimeHostError) -> Self {
        let mut error = if self.error.lifecycle.capacity.is_some() {
            error.with_related_failure("prior_capacity_admission", &self.error)
        } else {
            error
        };
        if error.lifecycle.task_timing.is_none() {
            error.lifecycle.task_timing = self.error.lifecycle.task_timing.clone();
        }
        Self {
            state: RuntimeReceiptState::Failed,
            terminal: self.terminal,
            error: Box::new(error.into_fatal()),
            poison_runtime: true,
            task_failure: self.task_failure,
        }
    }
}

#[cfg(test)]
mod request_failure_tests {
    use super::*;

    #[test]
    fn cleanup_escalation_preserves_the_original_task_failure_classification() {
        let original = RequestFailure {
            state: RuntimeReceiptState::Failed,
            terminal: None,
            error: Box::new(RuntimeHostError::request(
                "capture_backend_operation_failed",
                "run_contained_task_capture",
                RuntimeErrorCode::CaptureFailed,
            )),
            poison_runtime: false,
            task_failure: Some(TaskFailureEvidence {
                code: "capture_backend_operation_failed",
                severity: EventSeverity::Warning,
            }),
        };

        let escalated = original.replace_with_poison(RuntimeHostError::fatal(
            "lease_cleanup_failed",
            "release_failed_policy_run",
            RuntimeErrorCode::RuntimeFatal,
        ));

        assert_eq!(escalated.error.code(), "lease_cleanup_failed");
        assert!(escalated.poison_runtime);
        assert_eq!(
            escalated.task_failure,
            Some(TaskFailureEvidence {
                code: "capture_backend_operation_failed",
                severity: EventSeverity::Warning,
            })
        );
    }
}

impl From<RuntimeHostError> for RequestFailure {
    fn from(error: RuntimeHostError) -> Self {
        Self::poison_without_terminal(error)
    }
}

impl ActionFailure {
    fn scheduler(error: RuntimeHostError) -> Self {
        Self {
            diagnostic: diagnostic_for_projection(error.projection()),
            effect: EffectDisposition::NotPerformed,
            poison_runtime: error.is_fatal(),
            release_after: false,
            destructive_started: false,
            transfer_after: error.code() == "lease_transfer_not_safe",
            task_failure: None,
            error,
        }
    }

    fn backend(error: RuntimeHostError) -> Self {
        let task_failure = Some(Box::new(TaskFailureEvidence {
            code: error.code(),
            severity: if error.is_fatal() {
                EventSeverity::Fatal
            } else {
                EventSeverity::Warning
            },
        }));
        Self {
            diagnostic: DiagnosticCode::BackendOperationFailed,
            effect: EffectDisposition::Indeterminate,
            poison_runtime: false,
            release_after: true,
            destructive_started: true,
            transfer_after: false,
            task_failure,
            error,
        }
    }

    fn poison(error: RuntimeHostError) -> Self {
        let error = error.into_fatal();
        Self {
            diagnostic: DiagnosticCode::RuntimeDiagnostic,
            effect: EffectDisposition::Indeterminate,
            poison_runtime: true,
            release_after: false,
            destructive_started: false,
            transfer_after: false,
            task_failure: None,
            error,
        }
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
        let result = connection.join().map_err(|_| {
            let error = RuntimeHostError::fatal(
                "runtime_connection_panicked",
                "join_runtime_connection",
                RuntimeErrorCode::RuntimeFatal,
            );
            shared
                .append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::ShutdownJoin,
                    RuntimeLifecycleFailure::Host(&error),
                    EventLinksDraft::default(),
                    None,
                )
                .err()
                .unwrap_or(error)
        })?;
        shared.record_lifecycle_result(
            RuntimeLifecycleFailureStage::ShutdownJoin,
            &mut failure,
            result,
        );
    }
    failure.map_or(Ok(()), Err)
}

#[derive(Clone, Copy)]
enum ConnectionFailureStage {
    RequestRead,
    RequestDecode,
    RequestCache,
    Dispatch,
    ReceiptBuild,
    ReceiptWrite,
    ConnectionPanic,
}

impl ConnectionFailureStage {
    const fn as_str(self) -> &'static str {
        match self {
            Self::RequestRead => "runtime.ipc.request_read",
            Self::RequestDecode => "runtime.ipc.request_decode",
            Self::RequestCache => "runtime.ipc.request_cache",
            Self::Dispatch => "runtime.ipc.dispatch",
            Self::ReceiptBuild => "runtime.ipc.receipt_build",
            Self::ReceiptWrite => "runtime.ipc.receipt_write",
            Self::ConnectionPanic => "runtime.ipc.connection_panic",
        }
    }

    const fn effect(self) -> EffectDisposition {
        match self {
            Self::RequestRead | Self::RequestDecode | Self::RequestCache => {
                EffectDisposition::NotPerformed
            }
            Self::Dispatch | Self::ReceiptBuild | Self::ReceiptWrite | Self::ConnectionPanic => {
                EffectDisposition::Indeterminate
            }
        }
    }
}

struct ConnectionFailureContext {
    connection_serial: u64,
    stage: Option<ConnectionFailureStage>,
    request_decoded: bool,
    links: EventLinksDraft,
    timing: ConnectionTiming,
}

#[derive(Default, serde::Serialize)]
struct ConnectionTiming {
    receive: ConnectionCallTiming,
    validated_dispatch: ConnectionCallTiming,
    policy_identity_projection: ConnectionCallTiming,
    receipt_write: ConnectionCallTiming,
}

#[derive(serde::Serialize)]
struct ConnectionCallTiming {
    status: TaskTimingObservationState,
    elapsed_us: Option<ObservedMicroseconds>,
    result: Option<TaskTimingResult>,
    #[serde(skip)]
    started: Option<Instant>,
}

impl Default for ConnectionCallTiming {
    fn default() -> Self {
        Self {
            status: TaskTimingObservationState::Unobserved,
            elapsed_us: None,
            result: None,
            started: None,
        }
    }
}

impl ConnectionCallTiming {
    fn begin(&mut self) {
        self.started = Some(Instant::now());
        self.status = TaskTimingObservationState::Incomplete {
            reason: TimingObservationIssue::CallIncomplete,
        };
    }

    fn finish(&mut self, succeeded: bool) {
        let ended = Instant::now();
        self.result = Some(if succeeded {
            TaskTimingResult::Ok
        } else {
            TaskTimingResult::Err
        });
        if let Some(started) = self.started {
            let elapsed = actingcommand_execution_kernel::observe_instant_span(started, ended);
            self.status = match elapsed {
                ObservedMicroseconds::Measured { .. } => TaskTimingObservationState::Observed,
                ObservedMicroseconds::Unavailable { reason } => {
                    TaskTimingObservationState::Incomplete { reason }
                }
            };
            self.elapsed_us = Some(elapsed);
        }
    }
}

fn connection_boundary(
    mut stream: TcpStream,
    shared: Arc<HostShared>,
    connection_id: ConnectionId,
    connection_serial: u64,
    maximum_frame_bytes: usize,
    io_timeout: Duration,
) -> RuntimeHostResult<()> {
    let mut context = ConnectionFailureContext {
        connection_serial,
        stage: None,
        request_decoded: false,
        links: EventLinksDraft::default(),
        timing: ConnectionTiming::default(),
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        connection_loop(
            &mut stream,
            &shared,
            connection_id,
            maximum_frame_bytes,
            io_timeout,
            &mut context,
        )
    }));
    let mut failure = match result {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error),
        Err(_) => {
            context.stage = Some(ConnectionFailureStage::ConnectionPanic);
            Some(RuntimeHostError::fatal(
                "runtime_connection_panicked",
                "serve_runtime_connection",
                RuntimeErrorCode::RuntimeFatal,
            ))
        }
    };
    if let (Some(error), Some(stage)) = (&failure, context.stage)
        && let Err(append_error) = shared.append_connection_failure(&context, stage, error)
    {
        failure = Some(append_error);
    }
    drop(stream);
    let reason = if shared.fatal.is_shutdown_requested() {
        LeaseReleaseReason::HostShutdown
    } else {
        LeaseReleaseReason::Disconnect
    };
    let cleanup = (|| {
        let _work = shared.work_guard()?;
        shared.cleanup_connection(connection_id, reason)
    })();
    #[cfg(feature = "test-observation")]
    crate::test_observation::emit_connection(
        crate::test_observation::HostTestObservationPoint::ConnectionCleanupResult,
        if cleanup.is_ok() {
            crate::test_observation::HostTestObservationOutcome::Success
        } else {
            crate::test_observation::HostTestObservationOutcome::Error
        },
    );
    shared.record_lifecycle_result(
        RuntimeLifecycleFailureStage::ConnectionCleanup,
        &mut failure,
        cleanup,
    );
    if let Some(error) = failure {
        if error.is_fatal() {
            shared.fatal.mark(error.clone())?;
            Err(error)
        } else {
            Ok(())
        }
    } else {
        Ok(())
    }
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

fn connection_loop(
    stream: &mut TcpStream,
    shared: &HostShared,
    connection_id: ConnectionId,
    maximum_frame_bytes: usize,
    io_timeout: Duration,
    context: &mut ConnectionFailureContext,
) -> RuntimeHostResult<()> {
    stream
        .set_read_timeout(Some(io_timeout))
        .map_err(|_| protocol_error("set_read_timeout"))?;
    stream
        .set_write_timeout(Some(io_timeout))
        .map_err(|_| protocol_error("set_write_timeout"))?;
    stream
        .set_nodelay(true)
        .map_err(|_| protocol_error("set_tcp_nodelay"))?;
    let mut cache = RequestCache::default();
    while !shared.fatal.is_shutdown_requested() {
        context.stage = Some(ConnectionFailureStage::RequestRead);
        context.request_decoded = false;
        context.links = EventLinksDraft::default();
        context.timing = ConnectionTiming::default();
        context.timing.receive.begin();
        let received = read_frame(stream, maximum_frame_bytes);
        context.timing.receive.finish(received.is_ok());
        let frame = match received {
            Ok(FrameRead::Data(frame)) => frame,
            Ok(FrameRead::Idle) => continue,
            Ok(FrameRead::Closed) => {
                #[cfg(feature = "test-observation")]
                crate::test_observation::emit_connection(
                    crate::test_observation::HostTestObservationPoint::ConnectionExit,
                    crate::test_observation::HostTestObservationOutcome::Closed,
                );
                return Ok(());
            }
            Err(error) => {
                #[cfg(feature = "test-observation")]
                crate::test_observation::emit_connection(
                    crate::test_observation::HostTestObservationPoint::ConnectionExit,
                    crate::test_observation::HostTestObservationOutcome::Error,
                );
                return Err(error);
            }
        };
        context.stage = Some(ConnectionFailureStage::RequestDecode);
        let request = match serde_json::from_slice::<RuntimeRequest>(&frame) {
            Ok(request) => request,
            Err(_) => {
                #[cfg(feature = "test-observation")]
                crate::test_observation::emit_connection(
                    crate::test_observation::HostTestObservationPoint::ConnectionExit,
                    crate::test_observation::HostTestObservationOutcome::Error,
                );
                return Err(protocol_error("runtime_request_decode_failed"));
            }
        };
        context.request_decoded = true;
        // Idle sockets hold no admission. A decoded request remains in flight through its reply.
        let _work = if matches!(
            request.operation(),
            RuntimeOperation::RequestShutdown { .. }
        ) {
            None
        } else {
            shared.begin_work()?
        };
        if let Ok(validated) = request.validate() {
            context.links = validated.event_links(None, None, None);
        }
        #[cfg(feature = "test-observation")]
        crate::test_observation::emit_request(
            crate::test_observation::HostTestObservationPoint::FrameReceived,
            crate::test_observation::HostTestObservationOutcome::Success,
            &request,
        );
        #[cfg(feature = "test-observation")]
        crate::test_observation::emit_request(
            crate::test_observation::HostTestObservationPoint::DispatchStart,
            crate::test_observation::HostTestObservationOutcome::Started,
            &request,
        );
        context.stage = Some(ConnectionFailureStage::RequestCache);
        let receipt = match cache.get(&request) {
            Ok(Some(receipt)) => Ok(receipt),
            Ok(None) => {
                context.stage = Some(ConnectionFailureStage::Dispatch);
                shared
                    .process_request_observed(&request, connection_id, Some(&mut context.timing))
                    .inspect(|receipt| {
                        cache.insert(request.clone(), receipt.clone());
                    })
            }
            Err(error) => Err(error),
        };
        let receipt = match receipt {
            Ok(receipt) => {
                #[cfg(feature = "test-observation")]
                crate::test_observation::emit_receipt(
                    crate::test_observation::HostTestObservationPoint::DispatchResult,
                    crate::test_observation::HostTestObservationOutcome::Success,
                    &request,
                    &receipt,
                );
                receipt
            }
            Err(error) => {
                if error.operation() == "build_runtime_receipt" {
                    context.stage = Some(ConnectionFailureStage::ReceiptBuild);
                }
                #[cfg(feature = "test-observation")]
                crate::test_observation::emit_request(
                    crate::test_observation::HostTestObservationPoint::DispatchResult,
                    crate::test_observation::HostTestObservationOutcome::Error,
                    &request,
                );
                #[cfg(feature = "test-observation")]
                crate::test_observation::emit_connection(
                    crate::test_observation::HostTestObservationPoint::ConnectionExit,
                    crate::test_observation::HostTestObservationOutcome::Error,
                );
                return Err(error);
            }
        };
        #[cfg(feature = "test-observation")]
        crate::test_observation::emit_receipt(
            crate::test_observation::HostTestObservationPoint::ReceiptWriteStart,
            crate::test_observation::HostTestObservationOutcome::Started,
            &request,
            &receipt,
        );
        context.stage = Some(ConnectionFailureStage::ReceiptWrite);
        context.timing.receipt_write.begin();
        let written = write_frame(stream, &receipt, maximum_frame_bytes);
        context.timing.receipt_write.finish(written.is_ok());
        match written {
            Ok(()) => {
                #[cfg(feature = "test-observation")]
                crate::test_observation::emit_receipt(
                    crate::test_observation::HostTestObservationPoint::ReceiptWriteResult,
                    crate::test_observation::HostTestObservationOutcome::Success,
                    &request,
                    &receipt,
                );
            }
            Err(error) => {
                #[cfg(feature = "test-observation")]
                crate::test_observation::emit_receipt(
                    crate::test_observation::HostTestObservationPoint::ReceiptWriteResult,
                    crate::test_observation::HostTestObservationOutcome::Error,
                    &request,
                    &receipt,
                );
                #[cfg(feature = "test-observation")]
                crate::test_observation::emit_connection(
                    crate::test_observation::HostTestObservationPoint::ConnectionExit,
                    crate::test_observation::HostTestObservationOutcome::Error,
                );
                return Err(error);
            }
        }
    }
    #[cfg(feature = "test-observation")]
    crate::test_observation::emit_connection(
        crate::test_observation::HostTestObservationPoint::ConnectionExit,
        crate::test_observation::HostTestObservationOutcome::Shutdown,
    );
    Ok(())
}

#[derive(Default)]
struct RequestCache {
    entries: BTreeMap<RequestId, (RuntimeRequest, RuntimeReceipt)>,
    order: VecDeque<RequestId>,
}

impl RequestCache {
    fn get(&self, request: &RuntimeRequest) -> RuntimeHostResult<Option<RuntimeReceipt>> {
        let Some((original, receipt)) = self.entries.get(&request.request_id()) else {
            return Ok(None);
        };
        if original != request {
            return Err(protocol_error("runtime_request_id_reused"));
        }
        Ok(Some(receipt.clone()))
    }

    fn insert(&mut self, request: RuntimeRequest, receipt: RuntimeReceipt) {
        let request_id = request.request_id();
        self.entries.insert(request_id, (request, receipt));
        self.order.push_back(request_id);
        while self.order.len() > MAX_REQUEST_CACHE_ENTRIES {
            if let Some(expired) = self.order.pop_front() {
                self.entries.remove(&expired);
            }
        }
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

fn runtime_error_receipt(
    request: &RuntimeRequest,
    state: RuntimeReceiptState,
    terminal: Option<TerminalEvent>,
    error: RuntimeErrorProjection,
) -> RuntimeHostResult<RuntimeReceipt> {
    RuntimeReceipt::error(request, state, terminal, error).map_err(|_| receipt_error())
}

fn terminal(event: &PersistedEvent) -> TerminalEvent {
    TerminalEvent {
        sequence: event.sequence(),
        event_id: *event.event_id(),
    }
}

fn diagnostic_for_projection(projection: &RuntimeErrorProjection) -> DiagnosticCode {
    match projection.code {
        RuntimeErrorCode::LeaseBusy => DiagnosticCode::LeaseBusy,
        RuntimeErrorCode::LeaseCooldown => DiagnosticCode::LeaseCooldown,
        RuntimeErrorCode::LeaseExpired => DiagnosticCode::LeaseExpired,
        RuntimeErrorCode::BackendOpenFailed => DiagnosticCode::BackendOpenFailed,
        RuntimeErrorCode::BackendOperationFailed => DiagnosticCode::BackendOperationFailed,
        _ => DiagnosticCode::LeaseFencingDenied,
    }
}

fn policy_event_data(
    intent: &DispatchIntent,
    reason_chain: &DecisionReasonChain,
) -> RuntimeHostResult<PolicyDispatchEventData> {
    let package_digest = intent.package_digest.clone().ok_or_else(|| {
        policy_admission_fatal(
            "procedure_package_digest_missing",
            "build_policy_dispatch_event",
        )
    })?;
    let procedure_binding_digest = intent.procedure_binding_digest.clone().ok_or_else(|| {
        policy_admission_fatal(
            "procedure_binding_digest_missing",
            "build_policy_dispatch_event",
        )
    })?;
    Ok(PolicyDispatchEventData {
        decision_id: intent.decision_id.clone(),
        task_id: intent.task_id.clone(),
        instance_id: intent.instance_id.clone(),
        operation_id: intent.operation_id.clone(),
        package_digest,
        procedure_binding_digest,
        reason_chain_id: reason_chain.id.clone(),
        reasons: reason_chain
            .reasons
            .iter()
            .map(|reason| PolicyReasonRecord {
                code: reason.code.clone(),
                detail: reason.detail.clone(),
            })
            .collect(),
        catalog_hash: intent.catalog_hash.clone(),
        catalog_version: intent.catalog_version,
        input_ledger_position: intent.input_ledger_position,
        fact_snapshot_id: intent.fact_snapshot_id.clone(),
        approval_fact_ids: intent.approval_refs.clone(),
        urgency_milli: intent.prerequisites.urgency_milli,
    })
}

fn policy_id_error(operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(
        "policy_identifier_issue_failed",
        operation,
        RuntimeErrorCode::RuntimeFatal,
    )
}

fn policy_contract_error(operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(
        "policy_runtime_contract_invalid",
        operation,
        RuntimeErrorCode::RuntimeFatal,
    )
}

fn runtime_policy_seed(
    fact_snapshot_id: &str,
    time: EvaluationTime,
    owner_epoch: actingcommand_contract::OwnerEpoch,
) -> RuntimeHostResult<u64> {
    let bytes = serde_json::to_vec(&(fact_snapshot_id, time, owner_epoch)).map_err(|_| {
        RuntimeHostError::fatal(
            "policy_seed_encode_failed",
            "derive_policy_seed",
            RuntimeErrorCode::RuntimeFatal,
        )
    })?;
    let digest = Sha256::digest(bytes);
    let mut seed = [0_u8; 8];
    seed.copy_from_slice(&digest[..8]);
    Ok(u64::from_be_bytes(seed))
}

fn policy_admission_request(code: &'static str, operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::request(code, operation, RuntimeErrorCode::InvalidRequest)
}

fn policy_admission_fatal(code: &'static str, operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(code, operation, RuntimeErrorCode::RuntimeFatal)
}

fn critical_execution_error<E>(error: &CriticalExecutionError<E>) -> RuntimeHostError {
    match error {
        CriticalExecutionError::IntentAppend(_) => ledger_error("append_critical_intent"),
        CriticalExecutionError::OutcomeUndurable { .. } => ledger_error("append_critical_outcome"),
        CriticalExecutionError::Action { .. } => RuntimeHostError::fatal(
            "critical_action_mapping_invalid",
            "map_critical_result",
            RuntimeErrorCode::RuntimeFatal,
        ),
    }
}

fn critical_plan_error() -> RuntimeHostError {
    RuntimeHostError::fatal(
        "critical_event_plan_invalid",
        "build_critical_event",
        RuntimeErrorCode::RuntimeFatal,
    )
}

fn client_fact_conflict(code: &'static str, operation: &'static str) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::request(code, operation, RuntimeErrorCode::InvalidRequest),
        RuntimeReceiptState::Denied,
        None,
    )
}

fn ledger_error(operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal("ledger_failure", operation, RuntimeErrorCode::LedgerFailure)
}

fn receipt_error() -> RuntimeHostError {
    RuntimeHostError::fatal(
        "runtime_receipt_invalid",
        "build_runtime_receipt",
        RuntimeErrorCode::RuntimeFatal,
    )
}

fn protocol_error(operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::request(
        "runtime_protocol_invalid",
        operation,
        RuntimeErrorCode::ProtocolInvalid,
    )
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
