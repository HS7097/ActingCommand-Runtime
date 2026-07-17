// SPDX-License-Identifier: AGPL-3.0-only

use crate::agent_dispatcher::{
    AgentDispatcherState, AgentResponsePreparation, AgentSessionPreparation,
};
use crate::approval::ApprovalProjection;
use crate::events::RuntimeEvents;
use crate::fact_store::InstanceFactStore;
use crate::ipc::{DEFAULT_RUNTIME_MAX_FRAME_BYTES, FrameRead, read_frame, write_frame};
use crate::monitor::{DueMonitorProbe, MonitorRegistry, MonitorUpdate};
use crate::owner::{OwnerGuard, OwnerStartup};
use crate::performance::{
    PerformanceMonitor, PerformanceSemanticEvent, PerformanceTick, PipelineEventObservation,
    system_performance_sampler,
};
use crate::performance_control::{PerformanceBalanceController, PerformanceDispatchGate};
use crate::planning::collect_maintenance_evidence;
use crate::policy_host::{LoadedCatalog, PolicyExecutionPreparation, PolicyHost};
use crate::project_interface::{
    ProjectDiagnosticProjection, ProjectInterfaceProjection, retain_recent_diagnostics,
};
use crate::proposal::prepare_proposal;
use crate::strategy::{StrategicPlanPreparation, build_strategy_proposal};
use crate::time::{RuntimeClock, RuntimeClockSample, SystemRuntimeClock, unix_ms_now};
use crate::{
    AgentDispatcherConfig, CatalogGeneration, FatalState, MaintenanceLedgerQuery,
    PerformanceControlConfig, PerformanceControlDirective, PerformanceMonitorConfig,
    PipelinePerformanceSignal, PolicyAdmissionContext, PolicyCadence, PolicyCycle,
    PolicyDispatchAdmission, PolicyExecutionInput, PolicyTrigger, RuntimeHostError,
    RuntimeHostResult,
};
use actingcommand_artifact_store::{
    ArtifactEventSink, ArtifactStore, ArtifactStoreError, ArtifactStoreResult,
    ArtifactWriteContext, ArtifactWriteRequest, CapturePipelineCounts, CapturePipelineSummary,
    EvidenceExportDocuments, EvidenceExportIdentity, EvidenceExportRequest, EvidenceExporter,
    EvidenceJsonDocument, EvidencePackage, PackageVerification, PersistedFrameEvidence,
    read_projected_verified,
};
use actingcommand_contract::{
    AgentPayloadDraft, AgentSessionContext, AgentSessionId, AgentSessionResponse,
    AgentSessionStatus, AgentWakeId, AgentWakeKind, AgentWakeTrigger, ApplicationLifecycleAction,
    ApplicationPayload, ApplicationPayloadDraft, ApprovalDecisionRecord, ApprovalPayload,
    ApprovalPayloadDraft, ArtifactIssuePolicy, ArtifactKind, ArtifactLinksDraft, ArtifactProducer,
    ArtifactRedactionState, AuditInput, CapturePayloadDraft, CaptureSequence, CaptureSequenceSpec,
    CatalogPayloadDraft, CatalogPromotionAuthorization, CatalogProposal,
    CatalogTransitionEventData, ClientActionRecord, ClientPayload, ClientPayloadDraft,
    CommandPayloadDraft, ContainedTaskRequest, CorrelationId, DiagnosticCode, EffectDisposition,
    EventAction, EventActor, EventDraft, EventId, EventLinksDraft, EventPayload, EventQuery,
    EventSeverity, EventSource, EventType, EvidenceCompleteness, FactPayloadDraft, FactRecord,
    InputAction, InputPayload, InputPayloadDraft, InstanceFactContext, InstanceFactSnapshot,
    InstanceId, IssuedActionId, IssuedFrameId, IssuedMonitorProbe, IssuedReadOnlyCaptureCapability,
    IssuedRecognitionId, IssuedRunId, IssuedTaskId, LeaseId, LeasePayloadDraft, LeaseQueuePolicy,
    LeaseToken, MAX_GOVERNANCE_CAPABILITY_BYTES, MAX_INSTANCE_ALIAS_BYTES,
    MIN_GOVERNANCE_CAPABILITY_BYTES, MonitorPayloadDraft, MonitorRecoveryCoordinationReason,
    OriginModule, PackageDebugLayout, PackageDebugRequest, PackageDebugSummary, PerformanceContext,
    PerformancePayloadDraft, PolicyDispatchEventData, PolicyExecutionEventData, PolicyPayload,
    PolicyPayloadDraft, PolicyPlanningSignalEventData, PolicyReasonRecord,
    ProjectDecisionPageRequest, ProjectInterfaceRequest, ProjectedArtifactReference, ProposalClass,
    ProposalPromotion, RUNTIME_INFO_FILE, ReadonlyObservation, RecognitionPayloadDraft,
    RecognitionVerdict, ReleasePayload, ReleasePayloadDraft, ReleaseTransitionKind, RequestId,
    ResourceAuthoringEvent, ResourceAuthoringPayloadDraft, ResourceAuthoringPhase, RetentionClass,
    RuntimeCaptureBackend, RuntimeControlPlaneStatus, RuntimeDebugEvent, RuntimeDebugOperation,
    RuntimeDebugPhase, RuntimeErrorCode, RuntimeErrorProjection, RuntimeEventBatch,
    RuntimeEvidenceExportRequest, RuntimeEvidenceExportSummary, RuntimeEvidenceScreenshotCounts,
    RuntimeInfo, RuntimeInstanceStatus, RuntimeMonitorPolicy, RuntimeOperation,
    RuntimePayloadDraft, RuntimeReceipt, RuntimeReceiptState, RuntimeReleaseSet, RuntimeRequest,
    RuntimeResult, RuntimeSubscriptionRequest, SchedulerPayloadDraft, StatePayload,
    StatePayloadDraft, TaskOutcome, TaskPayload, TaskPayloadDraft, TaskSemanticFact, TerminalEvent,
    ValidatedRuntimeRequest,
};
use actingcommand_device::{CaptureBackendName, Frame};
use actingcommand_execution_kernel::{
    ContainedTaskRunError, ContainedTaskRuntime, ContainedTaskTrace, ExecutionBackendProvider,
    ExecutionKernel, ExternalExpectedSha256, PreparedContainedTask, decide_monitor,
};
use actingcommand_ledger::critical::{
    CatalogTransitionTarget, CriticalActionReport, CriticalEventPlan, CriticalExecutionError,
    CriticalOperation, DefiniteEffectDisposition, EventAppender, LeaseTransitionTarget,
    ReleaseTransitionTarget, execute_critical,
};
use actingcommand_ledger::{
    GlobalLedger, GlobalLedgerConfig, PersistedEvent, project_subscription_event,
};
use actingcommand_pack_containment::{
    Containment, DEFAULT_MAX_COMPRESSED_BYTES, InstanceId as ContainmentInstanceId, PackageLayout,
    Sha256Hash,
};
use actingcommand_policy::{
    CatalogSources, DecisionReasonChain, DispatchIntent, EvaluationFacts, EvaluationResources,
    EvaluationTime, ForwardProjection, ForwardProjectionConfig, MaintenanceAssessment,
    StrategicEvidencePointer, StrategicReport, assess_predictive_maintenance, project_forward,
    project_strategic_report,
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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const DEFAULT_RUNTIME_IO_TIMEOUT: Duration = Duration::from_secs(5);
const LEASE_SWEEP_INTERVAL: Duration = Duration::from_millis(50);
const MONITOR_POLL_INTERVAL: Duration = Duration::from_millis(25);
const ACCEPT_IDLE_INTERVAL: Duration = Duration::from_millis(20);
const MAX_REQUEST_CACHE_ENTRIES: usize = 4096;
const MAX_TRUSTED_POLICY_DISPATCHES: usize = 16_384;
const MAX_MONITOR_PROBES_PER_TICK: usize = 16;
const POLICY_CONNECTION_VALUE: u64 = u64::MAX;

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

#[derive(Clone)]
pub struct RuntimeHostConfig {
    state_root: PathBuf,
    bind_address: SocketAddr,
    scheduler: SchedulerConfig,
    policy_cadence: PolicyCadence,
    maximum_frame_bytes: usize,
    io_timeout: Duration,
    performance_monitor: Option<PerformanceMonitorConfig>,
    performance_control: PerformanceControlConfig,
    agent_dispatcher: Option<AgentDispatcherConfig>,
    secret_fingerprint_salt: Vec<u8>,
    governance_capability_sha256: Option<[u8; 32]>,
    governance_capability_invalid: bool,
    clock: Arc<dyn RuntimeClock>,
    policy_inputs: Option<PolicyInputSnapshot>,
}

impl RuntimeHostConfig {
    pub fn new(state_root: impl Into<PathBuf>, secret_fingerprint_salt: impl AsRef<[u8]>) -> Self {
        Self {
            state_root: state_root.into(),
            bind_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            scheduler: SchedulerConfig::default(),
            policy_cadence: PolicyCadence::default(),
            maximum_frame_bytes: DEFAULT_RUNTIME_MAX_FRAME_BYTES,
            io_timeout: DEFAULT_RUNTIME_IO_TIMEOUT,
            performance_monitor: None,
            performance_control: PerformanceControlConfig::default(),
            agent_dispatcher: None,
            secret_fingerprint_salt: secret_fingerprint_salt.as_ref().to_vec(),
            governance_capability_sha256: None,
            governance_capability_invalid: false,
            clock: Arc::new(SystemRuntimeClock::new()),
            policy_inputs: None,
        }
    }

    pub fn with_bind_address(mut self, bind_address: SocketAddr) -> Self {
        self.bind_address = bind_address;
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

    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    fn validate(&self) -> RuntimeHostResult<()> {
        self.scheduler
            .validate()
            .map_err(|error| RuntimeHostError::scheduler("validate_runtime_config", &error))?;
        self.policy_cadence.validate()?;
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
            .field("scheduler", &self.scheduler)
            .field("policy_cadence", &self.policy_cadence)
            .field("maximum_frame_bytes", &self.maximum_frame_bytes)
            .field("io_timeout", &self.io_timeout)
            .field("performance_monitor", &self.performance_monitor)
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
    pub fn start(
        config: RuntimeHostConfig,
        provider: Arc<dyn ExecutionBackendProvider>,
    ) -> RuntimeHostResult<Self> {
        config.validate()?;
        let registered_instances = initial_registered_instances(provider.as_ref())?;
        fs::create_dir_all(&config.state_root).map_err(|_| {
            RuntimeHostError::fatal(
                "state_root_create_failed",
                "start_runtime_host",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        let events = RuntimeEvents::new(&config.secret_fingerprint_salt)?;
        let clock_origin = config.clock.sample()?;
        let started_at_unix_ms = clock_origin.unix_ms;
        let OwnerStartup {
            guard: owner,
            owner_epoch,
            takeover_instances,
            takeover,
        } = OwnerGuard::acquire(&config.state_root, events.issuer(), started_at_unix_ms)?;
        let monitor_registry = MonitorRegistry::open(
            &config.state_root,
            registered_instances
                .values()
                .map(|instance| instance.instance_alias.clone()),
        )?;
        let scheduler = SeedScheduler::new(owner_epoch, config.scheduler, takeover_instances, 0)
            .map_err(|error| RuntimeHostError::scheduler("start_runtime_host", &error))?;
        let ledger_owner = format!("actingd-{}-{started_at_unix_ms}", std::process::id());
        let artifacts = ArtifactStore::open(&config.state_root)
            .map_err(|_| artifact_store_error("open_artifact_store"))?;
        let ledger = GlobalLedger::open_with_artifact_verifier(
            GlobalLedgerConfig::new(config.state_root.join("ledger"), ledger_owner),
            |reference| artifacts.verify_recovery_reference(reference).ok(),
        )
        .map_err(|_| ledger_error("open_global_ledger"))?;
        let state = Arc::new(
            RuntimeStateStore::open(&config.state_root, &config.secret_fingerprint_salt)
                .map_err(|error| RuntimeHostError::state(&error))?,
        );
        let mut policy = PolicyHost::open(
            &config.state_root,
            Arc::clone(&state),
            &ledger,
            config.policy_cadence.clone(),
        )?;
        reconcile_policy_dispatches(&mut policy, &ledger, &events)?;
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
        } else if !agent_dispatcher.is_empty() {
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
        append_runtime_start_event(&ledger, &events, &config.state_root, takeover)?;
        let facts = InstanceFactStore::recover(&ledger, Arc::clone(&state))?;
        let performance = match config.performance_monitor.clone() {
            Some(performance_config) => {
                PerformanceMonitor::enabled(performance_config, system_performance_sampler())?
            }
            None => PerformanceMonitor::disabled(),
        };
        let performance_interval = performance.sample_interval();
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
        let fatal = FatalState::default();
        let shared = Arc::new(HostShared {
            owner_epoch,
            scheduler: Mutex::new(scheduler),
            policy: Mutex::new(policy),
            performance: Mutex::new(performance),
            performance_control: Mutex::new(performance_control),
            governance_write_gate: Mutex::new(()),
            governance_capability_sha256: config.governance_capability_sha256,
            governance_connections: Mutex::new(BTreeSet::new()),
            fact_write_gate: Mutex::new(()),
            detection_write_gate: Mutex::new(()),
            state_write_gate: Mutex::new(()),
            agent_write_gate: Mutex::new(()),
            proposal_write_gate: Mutex::new(()),
            facts: Mutex::new(facts),
            policy_inputs: Mutex::new(config.policy_inputs),
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
            trusted_policy_dispatches: Mutex::new(TrustedPolicyDispatchStore::default()),
            policy_dispatch_started_at: Mutex::new(BTreeMap::new()),
            policy_outcome_gate: Mutex::new(()),
            admission_guards: Mutex::new(BTreeMap::new()),
            debug_runs: Mutex::new(BTreeMap::new()),
            contained_runs: Mutex::new(BTreeSet::new()),
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
        let accept_thread = match thread::Builder::new()
            .name("actingcommand-runtime-ipc".to_string())
            .spawn(move || accept_loop(listener, accept_shared, maximum_frame_bytes, io_timeout))
        {
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

    pub fn fatal_error(&self) -> RuntimeHostResult<Option<RuntimeHostError>> {
        self.shared_ref("read_runtime_health")?.fatal.current()
    }

    pub fn active_policy_catalog(&self) -> RuntimeHostResult<Option<CatalogGeneration>> {
        self.shared_ref("read_active_policy_catalog")?
            .active_policy_catalog()
    }

    pub fn activate_policy_catalog(
        &self,
        sources: &CatalogSources,
    ) -> RuntimeHostResult<CatalogGeneration> {
        self.shared_ref("activate_policy_catalog")?
            .activate_policy_catalog(sources)
    }

    #[cfg(test)]
    pub(crate) fn activate_policy_catalog_with_expected_for_test(
        &self,
        sources: &CatalogSources,
        expected: CatalogGeneration,
    ) -> RuntimeHostResult<CatalogGeneration> {
        let shared = self.shared_ref("activate_policy_catalog_with_expected_for_test")?;
        let catalog = lock(&shared.policy, "stage_policy_catalog_for_cas_test")?.stage(sources)?;
        shared.switch_policy_catalog(
            catalog,
            Some(expected),
            EventAction::CatalogActivate,
            CatalogTransitionTarget::Activated,
            None,
        )
    }

    pub fn rollback_policy_catalog(
        &self,
        catalog_hash: &str,
    ) -> RuntimeHostResult<CatalogGeneration> {
        self.shared_ref("rollback_policy_catalog")?
            .rollback_policy_catalog(catalog_hash)
    }

    pub fn stage_release_set(
        &self,
        manifest: RuntimeReleaseSet,
        sources: &ReleaseArtifactSources,
    ) -> RuntimeHostResult<RuntimeReleaseSet> {
        self.shared_ref("stage_release_set")?
            .stage_release_set(manifest, sources)
    }

    pub fn active_release_set(&self) -> RuntimeHostResult<Option<RuntimeReleaseSet>> {
        self.shared_ref("read_active_release_set")?
            .active_release_set()
    }

    pub fn activate_release_set(&self, release_id: &str) -> RuntimeHostResult<RuntimeReleaseSet> {
        self.shared_ref("activate_release_set")?
            .switch_release_set(ReleaseTransitionKind::Activate, release_id)
    }

    pub fn rollback_release_set(&self, release_id: &str) -> RuntimeHostResult<RuntimeReleaseSet> {
        self.shared_ref("rollback_release_set")?
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
        self.shared_ref("prepare_strategic_report")?
            .prepare_strategic_report(report, evidence)
    }

    /// Evaluates one policy cycle from Runtime-owned facts, resources, time, and seed.
    pub fn evaluate_policy_cycle(&self, trigger: PolicyTrigger) -> RuntimeHostResult<PolicyCycle> {
        self.shared_ref("evaluate_policy_cycle")?
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
        self.shared_ref("evaluate_policy_cycle")?
            .evaluate_policy_cycle_with_test_inputs(facts, resources, time, seed, trigger)
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
        self.shared_ref("project_policy_forward")?
            .project_policy_forward(facts, resources, time, seed, config)
    }

    /// Assesses ledger-pinned trends and publishes an idempotent recheck planning signal when due.
    pub fn assess_and_publish_predictive_maintenance(
        &self,
        query: &MaintenanceLedgerQuery,
    ) -> RuntimeHostResult<MaintenanceAssessment> {
        self.shared_ref("assess_predictive_maintenance")?
            .assess_and_publish_predictive_maintenance(query)
    }

    /// Validates and durably publishes one Runtime-owned fact into the GlobalLedger.
    pub fn publish_fact(&self, record: FactRecord) -> RuntimeHostResult<EventId> {
        self.shared_ref("publish_fact")?.publish_fact(record)
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
        self.shared_ref("admit_policy_dispatch")?
            .admit_policy_dispatch(intent, reason_chain, context)
    }

    pub fn pinned_policy_catalog(
        &self,
        decision_id: &str,
    ) -> RuntimeHostResult<Option<CatalogGeneration>> {
        self.shared_ref("read_pinned_policy_catalog")?
            .pinned_policy_catalog(decision_id)
    }

    pub fn complete_policy_dispatch(&self, decision_id: &str) -> RuntimeHostResult<()> {
        self.shared_ref("complete_policy_dispatch")?
            .record_policy_dispatch_outcome(decision_id, &PolicyExecutionInput::Succeeded)
            .map(|_| ())
    }

    pub fn record_policy_dispatch_outcome(
        &self,
        decision_id: &str,
        input: &PolicyExecutionInput,
    ) -> RuntimeHostResult<PolicyExecutionEventData> {
        self.shared_ref("record_policy_dispatch_outcome")?
            .record_policy_dispatch_outcome(decision_id, input)
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
        self.shared_ref("record_policy_planning_signal")?
            .record_policy_planning_signal(signal)
    }

    pub fn record_pipeline_performance(
        &self,
        signal: PipelinePerformanceSignal,
    ) -> RuntimeHostResult<()> {
        self.shared_ref("record_pipeline_performance")?
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
    pub(crate) fn performance_context_for_test(
        &self,
        instance_id: &str,
        observed_at_unix_ms: u64,
    ) -> RuntimeHostResult<PerformanceContext> {
        self.shared_ref("read_test_performance_context")?
            .performance_context(instance_id, observed_at_unix_ms)
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
                    executed_steps,
                    failure_code,
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

    pub fn close(mut self) -> RuntimeHostResult<()> {
        self.shutdown()
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

    fn shutdown(&mut self) -> RuntimeHostResult<()> {
        let Some(shared) = self.shared.take() else {
            return Ok(());
        };
        shared.fatal.request_shutdown();
        let mut failure =
            join_runtime_thread(self.accept_thread.take(), "join_runtime_accept").err();
        record_failure(
            &mut failure,
            join_runtime_thread(self.sweep_thread.take(), "join_runtime_sweeper"),
        );
        record_failure(
            &mut failure,
            join_runtime_thread(self.monitor_thread.take(), "join_runtime_monitor"),
        );
        record_failure(
            &mut failure,
            join_runtime_thread(self.performance_thread.take(), "join_runtime_performance"),
        );
        if let Err(error) = fs::remove_file(&self.info_path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            record_failure(
                &mut failure,
                Err(RuntimeHostError::fatal(
                    "runtime_info_remove_failed",
                    "close_runtime_host",
                    RuntimeErrorCode::RuntimeFatal,
                )),
            );
        }
        match Arc::try_unwrap(shared) {
            Ok(shared) => record_failure(&mut failure, shared.close()),
            Err(_) => record_failure(
                &mut failure,
                Err(RuntimeHostError::fatal(
                    "runtime_reference_leaked",
                    "close_runtime_host",
                    RuntimeErrorCode::RuntimeFatal,
                )),
            ),
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
}

#[derive(Clone)]
struct QueuedRequestContext {
    request: RuntimeRequest,
    instance: RegisteredInstance,
    connection_id: ConnectionId,
}

#[derive(Clone, Copy)]
struct QueueTerminalRecord {
    connection_id: ConnectionId,
    terminal: TerminalEvent,
}

struct MonitorRecoveryAdmission {
    reason: MonitorRecoveryCoordinationReason,
    lease_id: Option<LeaseId>,
}

struct CompletedReadonlyObservation {
    observation: ReadonlyObservation,
    terminal: PersistedEvent,
}

impl MonitorRecoveryAdmission {
    fn admitted(&self) -> bool {
        self.reason == MonitorRecoveryCoordinationReason::SchedulerAvailable
    }
}

#[derive(Default)]
struct QueueTerminalStore {
    entries: BTreeMap<RequestId, QueueTerminalRecord>,
    order: VecDeque<RequestId>,
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

impl QueueTerminalStore {
    fn insert(&mut self, request_id: RequestId, record: QueueTerminalRecord) {
        if self.entries.insert(request_id, record).is_none() {
            self.order.push_back(request_id);
        }
        while self.order.len() > MAX_REQUEST_CACHE_ENTRIES {
            if let Some(expired) = self.order.pop_front() {
                self.entries.remove(&expired);
            }
        }
    }
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
        if instance_alias.is_empty()
            || instance_alias.len() > MAX_INSTANCE_ALIAS_BYTES
            || instance_alias.chars().any(char::is_control)
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
    state: &RuntimeStateStore,
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
        if !migrated.contains(migration.migration_id()) {
            append_runtime_state_event(
                ledger,
                events,
                StatePayloadDraft::migrated(migration, AuditInput::new()),
            )?;
        }
    }

    let staged =
        release_event_identities(ledger, EventType::ReleaseStaged, |payload| match payload {
            ReleasePayload::Staged(value) => Some(value.manifest().release_id().to_owned()),
            _ => None,
        })?;
    for manifest in state
        .release_generations()
        .map_err(|error| RuntimeHostError::state(&error))?
    {
        if !staged.contains(manifest.release_id()) {
            append_runtime_state_event(
                ledger,
                events,
                ReleasePayloadDraft::staged(manifest, AuditInput::new()),
            )?;
        }
    }

    let mut completed = release_transition_identities(ledger, EventType::ReleaseActivated)?;
    completed.extend(release_transition_identities(
        ledger,
        EventType::ReleaseRolledBack,
    )?);
    for transition in state
        .release_transitions()
        .map_err(|error| RuntimeHostError::state(&error))?
    {
        if completed.contains(transition.transition_id()) {
            continue;
        }
        let recovered = transition.recovered_for_ledger();
        let payload = match recovered.kind() {
            ReleaseTransitionKind::Activate => {
                ReleasePayloadDraft::activated(recovered, AuditInput::new())
            }
            ReleaseTransitionKind::Rollback => {
                ReleasePayloadDraft::rolled_back(recovered, AuditInput::new())
            }
        };
        append_runtime_state_event(ledger, events, payload)?;
    }
    Ok(())
}

fn reconcile_policy_dispatches(
    policy: &mut PolicyHost,
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
) -> RuntimeHostResult<()> {
    let pending = policy.pending_dispatches();
    if pending.is_empty() {
        return Ok(());
    }
    let persisted = ledger
        .query(EventQuery::default())
        .map_err(|_| ledger_error("reconcile_policy_dispatches"))?;
    for dispatch in pending {
        let intent = persisted
            .iter()
            .find(|event| {
                event.event_type() == EventType::PolicyDispatchIntent
                    && matches!(
                        event.payload(),
                        EventPayload::Policy(PolicyPayload::DispatchIntent(payload))
                            if payload.decision_id() == dispatch.data.decision_id.as_str()
                    )
            })
            .ok_or_else(|| {
                policy_admission_fatal(
                    "policy_dispatch_intent_missing",
                    "reconcile_policy_dispatches",
                )
            })?;
        let lease_granted = persisted.iter().any(|event| {
            event.sequence() > intent.sequence()
                && event.event_type() == EventType::LeaseGranted
                && event.links().request_id() == intent.links().request_id()
                && event.links().correlation_id() == intent.links().correlation_id()
                && event.links().instance_id() == intent.links().instance_id()
        });
        let effect = if lease_granted {
            EffectDisposition::Indeterminate
        } else {
            EffectDisposition::NotPerformed
        };
        let draft = events.draft(
            EventSeverity::Error,
            EventSource::Scheduler,
            OriginModule::Policy,
            EventActor::Scheduler,
            events.system_links()?,
            PolicyPayloadDraft::dispatch_rejected(dispatch.data, effect, AuditInput::new()),
        )?;
        let draft = events.sanitize(draft)?;
        ledger
            .append(draft)
            .map_err(|_| ledger_error("reconcile_policy_dispatches"))?;
    }
    policy.refresh_dispatches(ledger)
}

fn ledger_has_release_stage(ledger: &GlobalLedger, release_id: &str) -> RuntimeHostResult<bool> {
    Ok(
        release_event_identities(ledger, EventType::ReleaseStaged, |payload| match payload {
            ReleasePayload::Staged(value) => Some(value.manifest().release_id().to_owned()),
            _ => None,
        })?
        .contains(release_id),
    )
}

fn release_event_identities(
    ledger: &GlobalLedger,
    event_type: EventType,
    identity: impl Fn(&ReleasePayload) -> Option<String>,
) -> RuntimeHostResult<BTreeSet<String>> {
    Ok(ledger
        .query(EventQuery {
            event_type: Some(event_type),
            ..EventQuery::default()
        })
        .map_err(|_| ledger_error("query_release_events"))?
        .into_iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Release(payload) => identity(payload),
            _ => None,
        })
        .collect::<BTreeSet<_>>())
}

fn release_transition_identities(
    ledger: &GlobalLedger,
    event_type: EventType,
) -> RuntimeHostResult<BTreeSet<String>> {
    release_event_identities(ledger, event_type, |payload| match payload {
        ReleasePayload::Activated(value) | ReleasePayload::RolledBack(value) => {
            Some(value.transition().transition_id().to_owned())
        }
        _ => None,
    })
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

fn append_agent_wake(
    state: &mut AgentDispatcherState,
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
    config: &AgentDispatcherConfig,
    source: &PersistedEvent,
    instance_id: InstanceId,
    kind: AgentWakeKind,
) -> RuntimeHostResult<PersistedEvent> {
    let issued = events
        .issuer()
        .issue_agent_wake(
            AgentWakeTrigger::new(
                instance_id,
                kind,
                *source.event_id(),
                source.sequence(),
                source.timestamp_unix_ms(),
            )
            .map_err(|_| {
                RuntimeHostError::fatal(
                    "agent_wake_trigger_invalid",
                    "append_agent_wake",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?,
            config.budget(),
            config.capabilities().clone(),
        )
        .map_err(|_| {
            RuntimeHostError::fatal(
                "agent_wake_issue_failed",
                "append_agent_wake",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
    let draft = events.draft(
        EventSeverity::Info,
        EventSource::Scheduler,
        OriginModule::AgentDispatcher,
        EventActor::Runtime,
        issued.event_links(),
        AgentPayloadDraft::wake_requested(issued.data().clone(), AuditInput::new()),
    )?;
    let persisted = ledger
        .append(events.sanitize(draft)?)
        .map_err(|_| ledger_error("append_agent_wake"))?;
    state.apply_event(&persisted, None)?;
    Ok(persisted)
}

struct HostShared {
    owner_epoch: actingcommand_contract::OwnerEpoch,
    scheduler: Mutex<SeedScheduler>,
    policy: Mutex<PolicyHost>,
    performance: Mutex<PerformanceMonitor>,
    performance_control: Mutex<PerformanceBalanceController>,
    // Client facts and approval authority are projected and appended as one ordered transition.
    governance_write_gate: Mutex<()>,
    governance_capability_sha256: Option<[u8; 32]>,
    governance_connections: Mutex<BTreeSet<ConnectionId>>,
    // Ledger append and fact projection commit are one ordered Runtime-owned transition.
    fact_write_gate: Mutex<()>,
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
    ledger: GlobalLedger,
    artifacts: ArtifactStore,
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
    trusted_policy_dispatches: Mutex<TrustedPolicyDispatchStore>,
    policy_dispatch_started_at: Mutex<BTreeMap<String, u64>>,
    // Outcome preparation and completion form one idempotent Runtime-owned transition.
    policy_outcome_gate: Mutex<()>,
    admission_guards: Mutex<BTreeMap<InstanceId, Arc<Mutex<()>>>>,
    debug_runs: Mutex<BTreeMap<CorrelationId, DebugRunContext>>,
    contained_runs: Mutex<BTreeSet<RequestId>>,
    next_connection_id: AtomicU64,
    clock: Arc<dyn RuntimeClock>,
    clock_origin_monotonic_ms: u64,
    fatal: FatalState,
}

struct PolicyAdmissionAppender<'a> {
    ledger: &'a GlobalLedger,
    initial_fact_gate: RefCell<Option<MutexGuard<'a, ()>>>,
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

struct RequestFailure {
    state: RuntimeReceiptState,
    terminal: Option<TerminalEvent>,
    error: Box<RuntimeHostError>,
    poison_runtime: bool,
}

struct ActionFailure {
    error: RuntimeHostError,
    diagnostic: DiagnosticCode,
    effect: EffectDisposition,
    poison_runtime: bool,
    release_after: bool,
    destructive_started: bool,
    transfer_after: bool,
}

impl HostShared {
    fn expire_agent_sessions(&self) -> RuntimeHostResult<()> {
        if self.agent_dispatcher_config.is_none() {
            return Ok(());
        }
        let now_unix_ms = unix_ms_now()?;
        let _gate = lock(&self.agent_write_gate, "expire_agent_sessions")?;
        let expired =
            lock(&self.agent_dispatcher, "expire_agent_sessions")?.expired_sessions(now_unix_ms)?;
        for data in expired {
            let links = self
                .events
                .issuer()
                .issue_agent_session_links(data.status().instance_id())
                .map_err(|_| runtime_identifier_error())?;
            let persisted = self.append_event_raw(
                EventSeverity::Warning,
                EventSource::System,
                OriginModule::AgentDispatcher,
                EventActor::Runtime,
                links.event_links(),
                AgentPayloadDraft::session_escalated(data, AuditInput::new()),
            )?;
            lock(&self.agent_dispatcher, "commit_agent_timeout")?.apply_event(&persisted, None)?;
        }
        Ok(())
    }

    fn sample_performance(&self, observed_at_unix_ms: u64) -> RuntimeHostResult<bool> {
        let (tick, control_observation) = {
            let mut performance = lock(&self.performance, "sample_performance")?;
            let tick = performance.tick(observed_at_unix_ms)?;
            let observation = performance.control_observation(observed_at_unix_ms)?;
            (tick, observation)
        };
        let PerformanceTick {
            events,
            stop_sampling,
        } = tick;
        self.record_performance_events(&events)?;
        if let Some(observation) = control_observation {
            self.reconcile_performance_control(observation)?;
        }
        Ok(stop_sampling)
    }

    fn reconcile_performance_control(
        &self,
        observation: crate::PerformanceControlObservation,
    ) -> RuntimeHostResult<()> {
        let workloads =
            lock(&self.policy, "read_performance_workloads")?.active_performance_workloads()?;
        let control_events = lock(&self.performance_control, "reconcile_performance_control")?
            .observe(observation, &workloads)?
            .into_iter()
            .map(PerformanceSemanticEvent::BalanceChanged)
            .collect::<Vec<_>>();
        self.record_performance_events(&control_events)
    }

    fn record_pipeline_performance(
        &self,
        signal: PipelinePerformanceSignal,
    ) -> RuntimeHostResult<()> {
        let events = lock(&self.performance, "record_pipeline_performance")?
            .record_pipeline_signal(signal)?;
        self.record_performance_events(&events)
    }

    fn performance_context(
        &self,
        instance_id: &str,
        observed_at_unix_ms: u64,
    ) -> RuntimeHostResult<PerformanceContext> {
        lock(&self.performance, "read_performance_context")?
            .context(instance_id, observed_at_unix_ms)
    }

    fn performance_control_directive(
        &self,
        instance_id: &str,
    ) -> RuntimeHostResult<PerformanceControlDirective> {
        lock(
            &self.performance_control,
            "read_performance_control_directive",
        )?
        .directive(instance_id)
    }

    fn record_performance_events(
        &self,
        events: &[PerformanceSemanticEvent],
    ) -> RuntimeHostResult<()> {
        for event in events {
            let payload = match event {
                PerformanceSemanticEvent::PressureStarted(data) => {
                    PerformancePayloadDraft::pressure_started(data.clone(), AuditInput::new())
                }
                PerformanceSemanticEvent::PressureEnded(data) => {
                    PerformancePayloadDraft::pressure_ended(data.clone(), AuditInput::new())
                }
                PerformanceSemanticEvent::StutterDetected(data) => {
                    PerformancePayloadDraft::stutter_detected(data.clone(), AuditInput::new())
                }
                PerformanceSemanticEvent::Summary(data) => {
                    PerformancePayloadDraft::summary(data.as_ref().clone(), AuditInput::new())
                }
                PerformanceSemanticEvent::MonitorDegraded(data) => {
                    PerformancePayloadDraft::monitor_degraded(data.clone(), AuditInput::new())
                }
                PerformanceSemanticEvent::MonitorRecovered(data) => {
                    PerformancePayloadDraft::monitor_recovered(data.clone(), AuditInput::new())
                }
                PerformanceSemanticEvent::BalanceChanged(data) => {
                    PerformancePayloadDraft::balance_changed(data.clone(), AuditInput::new())
                }
            };
            let persisted = self.append_event_raw(
                event.severity(),
                EventSource::Runtime,
                OriginModule::PerformanceMonitor,
                EventActor::Runtime,
                self.events.system_links()?,
                payload,
            )?;
            let mut performance = lock(&self.performance, "record_performance_event_reference")?;
            if !matches!(event, PerformanceSemanticEvent::BalanceChanged(_))
                || performance.sample_interval().is_some()
            {
                performance.record_event_reference(event, *persisted.event_id())?;
            }
        }
        Ok(())
    }

    fn observe_pipeline_event(&self, event: &PersistedEvent) -> RuntimeHostResult<()> {
        if !is_pipeline_event(event.event_type())
            || !lock(&self.performance, "read_performance_monitor_state")?.accepts_pipeline_events()
        {
            return Ok(());
        }
        let result: RuntimeHostResult<Vec<PerformanceSemanticEvent>> = (|| {
            let instance_id = event.links().instance_id().ok_or_else(|| {
                RuntimeHostError::fatal(
                    "performance_pipeline_instance_missing",
                    "observe_performance_pipeline_event",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
            let instance_alias = lock(
                &self.registered_instances,
                "resolve_performance_pipeline_instance",
            )?
            .get(instance_id)
            .map(|instance| instance.instance_alias.clone())
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    "performance_pipeline_instance_unknown",
                    "observe_performance_pipeline_event",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
            let observation = PipelineEventObservation {
                event_type: event.event_type(),
                instance_id: instance_alias,
                observed_at_unix_ms: event.timestamp_unix_ms(),
                frame_id: event.links().frame_id().copied(),
                recognition_id: event.links().recognition_id().copied(),
                action_id: event.links().action_id().copied(),
            };
            lock(&self.performance, "observe_performance_pipeline_event")?
                .observe_pipeline_event(observation)
        })();
        let semantic_events = match result {
            Ok(mut events) => {
                let mut recovered =
                    lock(&self.performance, "recover_performance_pipeline_monitor")?
                        .record_pipeline_success(event.timestamp_unix_ms())?;
                events.append(&mut recovered);
                events
            }
            Err(error) => {
                let tick = lock(&self.performance, "degrade_performance_pipeline_monitor")?
                    .record_monitor_failure(
                        event.timestamp_unix_ms(),
                        error.code(),
                        Some(*event.event_id()),
                    )?;
                tick.events
            }
        };
        self.record_performance_events(&semantic_events)
    }

    fn active_policy_catalog(&self) -> RuntimeHostResult<Option<CatalogGeneration>> {
        Ok(lock(&self.policy, "read_active_policy_catalog")?.active_generation())
    }

    fn stage_release_set(
        &self,
        manifest: RuntimeReleaseSet,
        sources: &ReleaseArtifactSources,
    ) -> RuntimeHostResult<RuntimeReleaseSet> {
        let result: RuntimeHostResult<RuntimeReleaseSet> = (|| {
            let _gate = lock(&self.state_write_gate, "stage_release_set")?;
            let staged = self
                .state
                .stage_release(manifest, sources)
                .map_err(|error| RuntimeHostError::state(&error))?;
            let manifest = staged.manifest().clone();
            if !ledger_has_release_stage(&self.ledger, manifest.release_id())? {
                self.append_event_raw(
                    EventSeverity::Info,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    self.events.system_links()?,
                    ReleasePayloadDraft::staged(manifest.clone(), AuditInput::new()),
                )?;
            }
            Ok(manifest)
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    fn active_release_set(&self) -> RuntimeHostResult<Option<RuntimeReleaseSet>> {
        self.state
            .active_release()
            .map(|active| active.map(|release| release.manifest().clone()))
            .map_err(|error| RuntimeHostError::state(&error))
    }

    fn switch_release_set(
        &self,
        kind: ReleaseTransitionKind,
        release_id: &str,
    ) -> RuntimeHostResult<RuntimeReleaseSet> {
        let _gate = lock(&self.state_write_gate, "switch_release_set")?;
        let preview = self
            .state
            .preview_release_transition(kind, release_id)
            .map_err(|error| RuntimeHostError::state(&error))?;
        let transition = preview.data().clone();
        let links = self.events.system_links()?;
        let intent = self.events.draft(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            ReleasePayloadDraft::transition_intent(transition.clone(), AuditInput::new()),
        )?;
        let intent = self.events.sanitize(intent)?;
        let target = match kind {
            ReleaseTransitionKind::Activate => ReleaseTransitionTarget::Activated,
            ReleaseTransitionKind::Rollback => ReleaseTransitionTarget::RolledBack,
        };
        let plan = CriticalEventPlan::new(CriticalOperation::ReleaseTransition(target), intent)
            .map_err(|_| critical_plan_error())?;
        let success_links = links.clone();
        let failure_links = links;
        let success_transition = transition.clone();
        let failure_transition = transition;
        let result = execute_critical(
            &self.ledger,
            self.events.fingerprinter(),
            plan,
            || match self
                .state
                .commit_release_transition(&preview)
                .map_err(|error| RuntimeHostError::state(&error))
            {
                Ok(active) => CriticalActionReport::Succeeded {
                    value: active,
                    effect: DefiniteEffectDisposition::Performed,
                },
                Err(error) => CriticalActionReport::Failed {
                    effect: if error.is_fatal() {
                        EffectDisposition::Indeterminate
                    } else {
                        EffectDisposition::NotPerformed
                    },
                    error,
                },
            },
            |_, _| {
                self.events
                    .draft(
                        EventSeverity::Info,
                        EventSource::Runtime,
                        OriginModule::Runtime,
                        EventActor::Runtime,
                        success_links,
                        match target {
                            ReleaseTransitionTarget::Activated => ReleasePayloadDraft::activated(
                                success_transition,
                                AuditInput::new(),
                            ),
                            ReleaseTransitionTarget::RolledBack => {
                                ReleasePayloadDraft::rolled_back(
                                    success_transition,
                                    AuditInput::new(),
                                )
                            }
                        },
                    )
                    .map_err(|_| actingcommand_contract::SanitizationError::fingerprinter_failure())
            },
            |_, effect| {
                self.events
                    .draft(
                        EventSeverity::Error,
                        EventSource::Runtime,
                        OriginModule::Runtime,
                        EventActor::Runtime,
                        failure_links,
                        ReleasePayloadDraft::transition_failed(
                            failure_transition,
                            effect,
                            AuditInput::new(),
                        ),
                    )
                    .map_err(|_| actingcommand_contract::SanitizationError::fingerprinter_failure())
            },
        );
        match result {
            Ok(receipt) => Ok(receipt.into_value().manifest().clone()),
            Err(CriticalExecutionError::Action { error, .. }) => {
                if error.is_fatal() {
                    self.fatal.mark(error.clone())?;
                }
                Err(error)
            }
            Err(error) => {
                let error = critical_execution_error(&error);
                self.fatal.mark(error.clone())?;
                Err(error)
            }
        }
    }

    fn activate_policy_catalog(
        &self,
        sources: &CatalogSources,
    ) -> RuntimeHostResult<CatalogGeneration> {
        self.activate_policy_catalog_with_authorization(sources, None)
    }

    fn activate_policy_catalog_with_authorization(
        &self,
        sources: &CatalogSources,
        promotion: Option<CatalogPromotionAuthorization>,
    ) -> RuntimeHostResult<CatalogGeneration> {
        let (catalog, previous) = {
            let policy = lock(&self.policy, "stage_policy_catalog")?;
            let catalog = policy.stage(sources)?;
            let previous = policy.active_generation();
            (catalog, previous)
        };
        if previous
            .as_ref()
            .is_some_and(|current| current.catalog_hash() == catalog.generation().catalog_hash())
        {
            return Ok(catalog.generation().clone());
        }
        if let Some(current) = &previous
            && (current.catalog_id() != catalog.generation().catalog_id()
                || catalog.generation().catalog_version() <= current.catalog_version())
        {
            return Err(RuntimeHostError::request(
                "catalog_activation_not_newer",
                "activate_policy_catalog",
                RuntimeErrorCode::InvalidRequest,
            ));
        }
        self.switch_policy_catalog(
            catalog,
            previous,
            EventAction::CatalogActivate,
            CatalogTransitionTarget::Activated,
            promotion,
        )
    }

    fn rollback_policy_catalog(&self, catalog_hash: &str) -> RuntimeHostResult<CatalogGeneration> {
        let (catalog, previous) = {
            let policy = lock(&self.policy, "load_policy_catalog_rollback")?;
            let previous = policy.active_generation().ok_or_else(|| {
                RuntimeHostError::request(
                    "policy_catalog_unavailable",
                    "rollback_policy_catalog",
                    RuntimeErrorCode::InvalidRequest,
                )
            })?;
            let catalog = policy.load_generation(catalog_hash)?;
            (catalog, previous)
        };
        if catalog.generation().catalog_hash() == previous.catalog_hash() {
            return Ok(previous);
        }
        if catalog.generation().catalog_id() != previous.catalog_id()
            || catalog.generation().catalog_version() >= previous.catalog_version()
        {
            return Err(RuntimeHostError::request(
                "catalog_rollback_not_older",
                "rollback_policy_catalog",
                RuntimeErrorCode::InvalidRequest,
            ));
        }
        self.switch_policy_catalog(
            catalog,
            Some(previous),
            EventAction::CatalogRollback,
            CatalogTransitionTarget::RolledBack,
            None,
        )
    }

    fn switch_policy_catalog(
        &self,
        catalog: LoadedCatalog,
        previous: Option<CatalogGeneration>,
        action: EventAction,
        target: CatalogTransitionTarget,
        promotion: Option<CatalogPromotionAuthorization>,
    ) -> RuntimeHostResult<CatalogGeneration> {
        let generation = catalog.generation().clone();
        let expected_active_hash = previous
            .as_ref()
            .map(|value| value.catalog_hash().to_owned());
        let data = CatalogTransitionEventData {
            catalog_id: generation.catalog_id().to_owned(),
            catalog_version: generation.catalog_version(),
            catalog_hash: generation.catalog_hash().to_owned(),
            previous_catalog_hash: previous
                .as_ref()
                .map(|value| value.catalog_hash().to_owned()),
            promotion,
        };
        let links = self.events.system_links()?;
        let intent = self.events.draft(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Policy,
            EventActor::Runtime,
            links.clone(),
            CatalogPayloadDraft::transition_intent(action, data.clone(), AuditInput::new()),
        )?;
        let intent = self.events.sanitize(intent)?;
        let plan = CriticalEventPlan::new(CriticalOperation::CatalogTransition(target), intent)
            .map_err(|_| critical_plan_error())?;
        let success_links = links.clone();
        let failure_links = links;
        let success_data = data.clone();
        let failure_data = data;
        let result = execute_critical(
            &self.ledger,
            self.events.fingerprinter(),
            plan,
            || match lock(&self.policy, "switch_active_policy_catalog").and_then(|mut policy| {
                policy.switch_active(catalog, expected_active_hash.as_deref())
            }) {
                Ok(()) => CriticalActionReport::Succeeded {
                    value: generation.clone(),
                    effect: DefiniteEffectDisposition::Performed,
                },
                Err(error) => CriticalActionReport::Failed {
                    effect: if error.is_fatal() {
                        EffectDisposition::Indeterminate
                    } else {
                        EffectDisposition::NotPerformed
                    },
                    error,
                },
            },
            |_, _| {
                self.events
                    .draft(
                        EventSeverity::Info,
                        EventSource::Runtime,
                        OriginModule::Policy,
                        EventActor::Runtime,
                        success_links,
                        match target {
                            CatalogTransitionTarget::Activated => {
                                CatalogPayloadDraft::activated(success_data, AuditInput::new())
                            }
                            CatalogTransitionTarget::RolledBack => {
                                CatalogPayloadDraft::rolled_back(success_data, AuditInput::new())
                            }
                        },
                    )
                    .map_err(|_| actingcommand_contract::SanitizationError::fingerprinter_failure())
            },
            |_, effect| {
                self.events
                    .draft(
                        EventSeverity::Error,
                        EventSource::Runtime,
                        OriginModule::Policy,
                        EventActor::Runtime,
                        failure_links,
                        CatalogPayloadDraft::transition_failed(
                            action,
                            failure_data,
                            effect,
                            AuditInput::new(),
                        ),
                    )
                    .map_err(|_| actingcommand_contract::SanitizationError::fingerprinter_failure())
            },
        );
        match result {
            Ok(receipt) => Ok(receipt.into_value()),
            Err(CriticalExecutionError::Action { error, .. }) => {
                if error.is_fatal() {
                    self.fatal.mark(error.clone())?;
                }
                Err(error)
            }
            Err(error) => {
                let error = critical_execution_error(&error);
                self.fatal.mark(error.clone())?;
                Err(error)
            }
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
        let _detection_gate = lock(&self.detection_write_gate, "plan_policy_detection")?;
        let (facts, resources) = {
            let _gate = lock(&self.fact_write_gate, "project_policy_facts")?;
            self.project_authoritative_policy_inputs_under_gate("evaluate_policy_cycle")?
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
        let cycle = lock(&self.policy, "evaluate_policy_cycle")?.evaluate(
            &facts,
            &controlled_resources,
            time,
            seed,
            trigger,
            observed_monotonic_ms,
        )?;
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

    fn project_authoritative_policy_inputs_under_gate(
        &self,
        operation: &'static str,
    ) -> RuntimeHostResult<(EvaluationFacts, EvaluationResources)> {
        self.synchronize_fact_store_under_gate()?;
        let inputs = lock(&self.policy_inputs, "read_policy_inputs")?
            .clone()
            .ok_or_else(|| policy_admission_request("policy_inputs_unconfigured", operation))?;
        self.validate_policy_input_authority(&inputs, operation)?;
        let ledger_position = self
            .ledger
            .latest_sequence()
            .map_err(|_| ledger_error("read_policy_fact_position"))?;
        let facts = lock(&self.facts, "project_policy_facts")?.overlay_policy_facts(
            inputs.facts(),
            inputs.resources(),
            ledger_position,
        )?;
        Ok((facts, inputs.resources().clone()))
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
        let facts = {
            let mut fact_projection = lock(&self.facts, "project_forward_facts")?.clone();
            fact_projection.synchronize(&self.ledger)?;
            let ledger_position = self
                .ledger
                .latest_sequence()
                .map_err(|_| ledger_error("project_forward_fact_position"))?;
            fact_projection.overlay_policy_facts(facts, resources, ledger_position)?
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
        let mut resources = resources.clone();
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

    fn publish_fact(&self, record: FactRecord) -> RuntimeHostResult<EventId> {
        let result: RuntimeHostResult<EventId> = (|| {
            let _gate = lock(&self.fact_write_gate, "publish_fact")?;
            self.synchronize_fact_store_under_gate()?;
            if let Some(event_id) = lock(&self.facts, "publish_fact")?.preview_publish(&record)? {
                return Ok(event_id);
            }
            let event = self.append_event_under_fact_gate(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::FactStore,
                EventActor::Runtime,
                self.events.system_links()?,
                FactPayloadDraft::published(record.clone(), AuditInput::new()),
            )?;
            self.synchronize_fact_store_under_gate()?;
            Ok(*event.event_id())
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    fn instance_fact_snapshot(
        &self,
        context: InstanceFactContext,
    ) -> RuntimeHostResult<InstanceFactSnapshot> {
        let _gate = lock(&self.fact_write_gate, "read_instance_fact_snapshot")?;
        self.synchronize_fact_store_under_gate()?;
        let ledger_position = self
            .ledger
            .latest_sequence()
            .map_err(|_| ledger_error("read_instance_fact_position"))?;
        lock(&self.facts, "read_instance_fact_snapshot")?.snapshot(context, ledger_position)
    }

    fn admit_policy_dispatch(
        &self,
        intent: &DispatchIntent,
        reason_chain: &DecisionReasonChain,
        context: &PolicyAdmissionContext,
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
        let gate_error = match lock(
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
        let links = self.events.request_links(
            &validated,
            Some(resolved.instance_id()),
            None,
            Some(action_id),
        );
        let data = policy_event_data(intent, reason_chain);
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
        let fact_gate = lock(&self.fact_write_gate, "validate_policy_fact_freshness")?;
        let (current_facts, _) =
            self.project_authoritative_policy_inputs_under_gate("admit_policy_dispatch")?;
        if current_facts.fact_snapshot_id != trusted.intent.fact_snapshot_id {
            return Err(policy_admission_request(
                "policy_facts_stale",
                "admit_policy_dispatch",
            ));
        }
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
                let admission = self.acquire_lease(
                    &validated,
                    request.request_id(),
                    &intent.instance_id,
                    holder_id,
                    connection_id,
                );
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
            |_, effect| {
                self.events
                    .draft(
                        EventSeverity::Error,
                        EventSource::Scheduler,
                        OriginModule::Policy,
                        EventActor::Scheduler,
                        failure_links,
                        PolicyPayloadDraft::dispatch_rejected(
                            failure_data,
                            effect,
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
                let mut starts = lock(
                    &self.policy_dispatch_started_at,
                    "record_policy_dispatch_start",
                )?;
                if starts
                    .insert(intent.decision_id.clone(), started_at_monotonic_ms)
                    .is_some()
                {
                    let error = policy_admission_fatal(
                        "policy_dispatch_clock_identity_conflict",
                        "record_policy_dispatch_start",
                    );
                    self.fatal.mark(error.clone())?;
                    return Err(error);
                }
                let (token, catalog, admission) = receipt.into_value();
                Ok(PolicyDispatchAdmission::Granted {
                    decision_id: intent.decision_id.clone(),
                    catalog,
                    token,
                    admission: Box::new(admission),
                })
            }
            Err(CriticalExecutionError::Action { error, .. }) => {
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

    fn record_policy_dispatch_outcome(
        &self,
        decision_id: &str,
        input: &PolicyExecutionInput,
    ) -> RuntimeHostResult<PolicyExecutionEventData> {
        let result: RuntimeHostResult<PolicyExecutionEventData> = (|| {
            let _gate = lock(&self.policy_outcome_gate, "record_policy_dispatch_outcome")?;
            let (instance_id, admitted_at_unix_ms) = {
                let policy = lock(&self.policy, "read_policy_dispatch_instance")?;
                if let Some(existing) = policy.replay_execution(decision_id, input)? {
                    return Ok(existing);
                }
                (
                    policy.execution_instance_id(decision_id)?.to_owned(),
                    policy.admitted_at(decision_id)?,
                )
            };
            let sample = self.runtime_clock_sample()?;
            let started_at_monotonic_ms = lock(
                &self.policy_dispatch_started_at,
                "read_policy_dispatch_start",
            )?
            .get(decision_id)
            .copied()
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    "policy_dispatch_clock_missing",
                    "record_policy_dispatch_outcome",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
            let runtime_ms = sample
                .monotonic_ms
                .checked_sub(started_at_monotonic_ms)
                .ok_or_else(|| {
                    RuntimeHostError::fatal(
                        "policy_dispatch_clock_regressed",
                        "record_policy_dispatch_outcome",
                        RuntimeErrorCode::RuntimeFatal,
                    )
                })?;
            let observed_at_unix_ms =
                admitted_at_unix_ms.checked_add(runtime_ms).ok_or_else(|| {
                    RuntimeHostError::fatal(
                        "policy_execution_time_overflow",
                        "record_policy_dispatch_outcome",
                        RuntimeErrorCode::RuntimeFatal,
                    )
                })?;
            let perf_context = self.performance_context(&instance_id, observed_at_unix_ms)?;
            let mut policy = lock(&self.policy, "record_policy_dispatch_outcome")?;
            let data = match policy.prepare_execution(
                decision_id,
                observed_at_unix_ms,
                runtime_ms,
                input,
                &perf_context,
            )? {
                PolicyExecutionPreparation::New(data) => {
                    let links = self.events.system_links()?;
                    self.append_event_raw(
                        policy_execution_severity(&data),
                        EventSource::Scheduler,
                        OriginModule::Policy,
                        EventActor::Scheduler,
                        links,
                        PolicyPayloadDraft::execution_recorded(data.clone(), AuditInput::new()),
                    )?;
                    policy.commit_execution(&data)?;
                    data
                }
                PolicyExecutionPreparation::Replay(data) => data,
            };
            if policy.dispatch_needs_completion(decision_id)? {
                let (dispatch, admission) = policy.completion_data(decision_id)?;
                let links = self.events.system_links()?;
                self.append_event_raw(
                    EventSeverity::Info,
                    EventSource::Scheduler,
                    OriginModule::Policy,
                    EventActor::Scheduler,
                    links,
                    PolicyPayloadDraft::dispatch_completed(dispatch, admission, AuditInput::new()),
                )?;
                policy.complete_dispatch(decision_id)?;
            }
            lock(
                &self.policy_dispatch_started_at,
                "clear_policy_dispatch_start",
            )?
            .remove(decision_id);
            Ok(data)
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    fn record_policy_planning_signal(
        &self,
        signal: PolicyPlanningSignalEventData,
    ) -> RuntimeHostResult<()> {
        let result: RuntimeHostResult<()> = (|| {
            let mut policy = lock(&self.policy, "record_policy_planning_signal")?;
            policy.validate_planning_signal(&signal)?;
            if let Some(existing) = policy.planning_signal(&signal.signal_id) {
                return if existing == &signal {
                    Ok(())
                } else {
                    Err(RuntimeHostError::fatal(
                        "policy_planning_signal_identity_conflict",
                        "record_policy_planning_signal",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                };
            }
            let links = self.events.system_links()?;
            let persisted = self.append_event_raw(
                EventSeverity::Info,
                EventSource::Scheduler,
                OriginModule::Policy,
                EventActor::Scheduler,
                links,
                PolicyPayloadDraft::planning_signal_observed(signal.clone(), AuditInput::new()),
            )?;
            policy.commit_planning_signal(signal.clone())?;
            drop(policy);
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
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    fn process_request(
        &self,
        request: &RuntimeRequest,
        connection_id: ConnectionId,
    ) -> RuntimeHostResult<RuntimeReceipt> {
        if let Some(error) = self.fatal.current()? {
            return runtime_error_receipt(
                request,
                RuntimeReceiptState::Failed,
                None,
                error.projection().clone(),
            );
        }
        let validated = match request.validate() {
            Ok(validated) => validated,
            Err(_) => {
                return runtime_error_receipt(
                    request,
                    RuntimeReceiptState::Denied,
                    None,
                    RuntimeErrorProjection::new(RuntimeErrorCode::InvalidRequest, false),
                );
            }
        };
        match self.process_validated(request, &validated, connection_id) {
            Ok(success) => {
                RuntimeReceipt::success(request, success.state, success.terminal, success.result)
                    .map_err(|_| receipt_error())
            }
            Err(failure) => {
                if failure.poison_runtime {
                    self.fatal.mark((*failure.error).clone())?;
                }
                runtime_error_receipt(
                    request,
                    failure.state,
                    failure.terminal,
                    failure.error.projection().clone(),
                )
            }
        }
    }

    fn process_validated(
        &self,
        request: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        match request.operation() {
            RuntimeOperation::Health => Ok(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: None,
                result: RuntimeResult::Health {
                    owner_epoch: self.owner_epoch,
                },
            }),
            RuntimeOperation::Status => self.control_plane_status(),
            RuntimeOperation::ProjectInterface { request } => self.project_interface(request),
            RuntimeOperation::MonitorStatus => self.monitor_status(),
            RuntimeOperation::ConfigureMonitor {
                instance_alias,
                policy,
            } => self.configure_monitor(request, validated, instance_alias, policy.clone()),
            RuntimeOperation::ClearMonitor { instance_alias } => {
                self.clear_monitor(request, validated, instance_alias)
            }
            RuntimeOperation::AcquireLease {
                instance_alias,
                holder_id,
            } => self.acquire_lease(
                validated,
                request.request_id(),
                instance_alias,
                *holder_id,
                connection_id,
            ),
            RuntimeOperation::QueueLease {
                instance_alias,
                holder_id,
                policy,
            } => self.queue_lease(
                request,
                validated,
                instance_alias,
                *holder_id,
                *policy,
                connection_id,
            ),
            RuntimeOperation::PollQueuedLease { queued_request_id } => {
                self.poll_queued_lease(validated, *queued_request_id, connection_id)
            }
            RuntimeOperation::CancelQueuedLease { queued_request_id } => {
                self.cancel_queued_lease(validated, *queued_request_id, connection_id)
            }
            RuntimeOperation::RenewLease { token } => {
                self.renew_lease(validated, request.request_id(), token, connection_id)
            }
            RuntimeOperation::ReleaseLease { token } => {
                self.release_lease(validated, request.request_id(), token, connection_id)
            }
            RuntimeOperation::ObserveReadonly { instance_alias } => {
                self.observe_readonly(request, validated, instance_alias)
            }
            RuntimeOperation::CaptureSequence {
                instance_alias,
                spec,
            } => self.capture_sequence(request, validated, instance_alias, *spec),
            RuntimeOperation::SafeReset {
                instance_alias,
                holder_id,
            } => self.safe_reset(
                request,
                validated,
                instance_alias,
                *holder_id,
                connection_id,
            ),
            RuntimeOperation::ApplicationLifecycle {
                instance_alias,
                holder_id,
                action,
            } => self.application_lifecycle(
                request,
                validated,
                instance_alias,
                *holder_id,
                *action,
                connection_id,
            ),
            RuntimeOperation::RunContainedTask {
                instance_alias,
                holder_id,
                request: task_request,
            } => self.run_contained_task(
                request,
                validated,
                instance_alias,
                *holder_id,
                task_request,
                connection_id,
            ),
            RuntimeOperation::Input { token, action } => {
                self.input(validated, token, action, connection_id)
            }
            RuntimeOperation::QueryEvents { query, profile } => self
                .ledger
                .project(query.clone(), *profile)
                .map(|events| OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: None,
                    result: RuntimeResult::Events { events },
                })
                .map_err(|_| RequestFailure::poison(ledger_error("query_runtime_events"), None)),
            RuntimeOperation::SubscribeEvents { request } => self.subscribe_events(request),
            RuntimeOperation::DebugPackage { request } => self.debug_package(validated, request),
            RuntimeOperation::ExportEvidence { request } => {
                self.export_evidence(validated, request)
            }
            RuntimeOperation::RecordAuthoringEvent { event } => {
                self.record_authoring_event(validated, event)
            }
            RuntimeOperation::RecordDebugEvent { event } => {
                self.record_debug_event(validated, event)
            }
            RuntimeOperation::RecordClientAction { action } => {
                self.record_client_action(request, validated, action)
            }
            RuntimeOperation::AuthenticateGovernance { capability } => {
                self.authenticate_governance(request, connection_id, capability)
            }
            RuntimeOperation::RecordApprovalDecision { decision } => {
                self.record_approval_decision(request, validated, decision, connection_id)
            }
            RuntimeOperation::StartAgentSession { wake_id } => {
                self.start_agent_session(request, validated, *wake_id)
            }
            RuntimeOperation::ResumeAgentSession { session_id } => {
                self.resume_agent_session(request, validated, *session_id)
            }
            RuntimeOperation::AgentSessionStatus { session_id } => {
                self.agent_session_status(*session_id)
            }
            RuntimeOperation::RecordAgentResponse { response } => {
                self.record_agent_response(request, validated, response)
            }
            RuntimeOperation::CompileProposal { proposal } => self.compile_proposal(proposal),
            RuntimeOperation::PromoteProposal { proposal } => self.promote_proposal(proposal),
        }
    }

    fn subscribe_events(
        &self,
        request: &RuntimeSubscriptionRequest,
    ) -> Result<OperationSuccess, RequestFailure> {
        let mut subscription = self
            .ledger
            .subscribe(request.cursor())
            .map_err(|_| RequestFailure::poison(ledger_error("subscribe_runtime_events"), None))?;
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(request.wait_ms()))
            .ok_or_else(|| {
                RequestFailure::poison(protocol_error("subscribe_runtime_events"), None)
            })?;
        let mut events = Vec::with_capacity(usize::from(request.max_events()));
        let mut first_receive = true;
        let mut post_match_receive_budget = usize::from(request.max_events());
        let timed_out = loop {
            if events.len() == usize::from(request.max_events()) {
                break false;
            }
            let now = Instant::now();
            if events.is_empty() && !first_receive && now >= deadline {
                break true;
            }
            if !events.is_empty() {
                if post_match_receive_budget == 0 {
                    break false;
                }
                post_match_receive_budget -= 1;
            }
            let timeout = if events.is_empty() {
                deadline.saturating_duration_since(now)
            } else {
                Duration::ZERO
            };
            first_receive = false;
            match subscription.recv_timeout(timeout) {
                Ok(event) => {
                    if let Some(projected) =
                        project_subscription_event(&event, request.query(), request.profile())
                    {
                        events.push(projected);
                    }
                }
                Err(error) if error.code() == "subscription_timeout" => {
                    break events.is_empty();
                }
                Err(_) => {
                    return Err(RequestFailure::poison(
                        ledger_error("receive_runtime_subscription"),
                        None,
                    ));
                }
            }
        };
        let batch = RuntimeEventBatch::new(events, subscription.resume_cursor(), timed_out)
            .map_err(|_| {
                RequestFailure::poison(protocol_error("build_runtime_event_batch"), None)
            })?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::EventBatch { batch },
        })
    }

    fn debug_package(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        request: &PackageDebugRequest,
    ) -> Result<OperationSuccess, RequestFailure> {
        let links = validated.event_links(None, None, None);
        self.append_event(
            EventSeverity::Info,
            EventSource::Lab,
            OriginModule::Actinglab,
            EventActor::Lab,
            links.clone(),
            CommandPayloadDraft::received(EventAction::RuntimeDebugPackage, AuditInput::new()),
        )?;

        let summary = match inspect_debug_package(request) {
            Ok(summary) => summary,
            Err(error) => {
                let event = self.append_event(
                    EventSeverity::Error,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    links,
                    CommandPayloadDraft::rejected(
                        EventAction::RuntimeDebugPackage,
                        DiagnosticCode::CommandRejected,
                        EffectDisposition::NotPerformed,
                        AuditInput::new(),
                    ),
                )?;
                return Err(RequestFailure::request(
                    error,
                    RuntimeReceiptState::Failed,
                    Some(terminal(&event)),
                ));
            }
        };
        let package_name = Path::new(request.package_path())
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                RequestFailure::request(
                    debug_package_error("debug_package_name_invalid"),
                    RuntimeReceiptState::Failed,
                    None,
                )
            })?;
        let package = EvidencePackage::new(
            package_name,
            summary.verified_sha256(),
            PackageVerification::Passed,
        )
        .map_err(|error| {
            RequestFailure::request(
                debug_package_error(error.code()),
                RuntimeReceiptState::Failed,
                None,
            )
        })?;
        let mut debug_runs = lock(&self.debug_runs, "lock_runtime_debug_runs")?;
        let existing = debug_runs.get(&validated.correlation_id()).cloned();
        let context = match existing {
            Some(existing)
                if existing.package == package && existing.package_summary == summary =>
            {
                existing
            }
            Some(_) => {
                let event = self.append_event(
                    EventSeverity::Error,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    links,
                    CommandPayloadDraft::rejected(
                        EventAction::RuntimeDebugPackage,
                        DiagnosticCode::CommandRejected,
                        EffectDisposition::NotPerformed,
                        AuditInput::new(),
                    ),
                )?;
                return Err(RequestFailure::request(
                    RuntimeHostError::request(
                        "runtime_debug_context_conflict",
                        "debug_package",
                        RuntimeErrorCode::PackageInvalid,
                    ),
                    RuntimeReceiptState::Denied,
                    Some(terminal(&event)),
                ));
            }
            None => {
                let run_id = self.events.issuer().mint_run_id().map_err(|_| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        "run_id_issue_failed",
                        "debug_package",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?;
                let task_id = self.events.issuer().mint_task_id().map_err(|_| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        "task_id_issue_failed",
                        "debug_package",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?;
                let task_links = validated.task_event_links(task_id, run_id);
                self.append_event(
                    EventSeverity::Info,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    task_links.clone(),
                    TaskPayloadDraft::requested(
                        EventAction::RuntimeDebugPackage,
                        AuditInput::new(),
                    ),
                )?;
                self.append_event(
                    EventSeverity::Info,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    task_links,
                    TaskPayloadDraft::started(EventAction::RuntimeDebugPackage, AuditInput::new()),
                )?;
                let context = DebugRunContext {
                    package,
                    package_summary: summary.clone(),
                    run_id,
                    task_id,
                    terminal_outcome: None,
                    completed_export: None,
                };
                debug_runs.insert(validated.correlation_id(), context.clone());
                context
            }
        };
        drop(debug_runs);
        let event = self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            CommandPayloadDraft::validated(
                EventAction::RuntimeDebugPackage,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            ),
        )?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&event)),
            result: RuntimeResult::PackageDebugCompleted {
                summary: context.package_summary,
            },
        })
    }

    fn export_evidence(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        request: &RuntimeEvidenceExportRequest,
    ) -> Result<OperationSuccess, RequestFailure> {
        let mut debug_runs = lock(&self.debug_runs, "lock_runtime_debug_runs")?;
        let context = debug_runs
            .get_mut(&validated.correlation_id())
            .ok_or_else(|| {
                RequestFailure::request(
                    RuntimeHostError::request(
                        "runtime_debug_context_missing",
                        "export_evidence",
                        RuntimeErrorCode::EvidenceExportFailed,
                    ),
                    RuntimeReceiptState::Denied,
                    None,
                )
            })?;
        if let Some(completed) = &context.completed_export {
            if completed.request_output_path == request.output_path()
                && completed.task_outcome == request.task_outcome()
            {
                return Ok(OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: Some(completed.response_terminal),
                    result: RuntimeResult::EvidenceExportCompleted {
                        summary: Box::new(completed.summary.clone()),
                    },
                });
            }
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    "runtime_evidence_export_conflict",
                    "export_evidence",
                    RuntimeErrorCode::EvidenceExportFailed,
                ),
                RuntimeReceiptState::Denied,
                None,
            ));
        }
        if context
            .terminal_outcome
            .is_some_and(|outcome| outcome != request.task_outcome())
        {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    "runtime_task_outcome_conflict",
                    "export_evidence",
                    RuntimeErrorCode::EvidenceExportFailed,
                ),
                RuntimeReceiptState::Denied,
                None,
            ));
        }

        let task_links = validated.task_event_links(context.task_id, context.run_id);
        if context.terminal_outcome.is_none() {
            self.append_event(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                task_links.clone(),
                TaskPayloadDraft::terminal_intent(EventAction::ArtifactExport, AuditInput::new()),
            )?;
            self.append_event(
                task_outcome_severity(request.task_outcome()),
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                task_links.clone(),
                task_outcome_payload(request.task_outcome()),
            )?;
            context.terminal_outcome = Some(request.task_outcome());
        }

        let events = self
            .ledger
            .project(
                EventQuery {
                    correlation_id: Some(validated.correlation_id()),
                    ..EventQuery::default()
                },
                actingcommand_contract::ProjectionProfile::Forensic,
            )
            .map_err(|_| RequestFailure::poison(ledger_error("project_evidence_events"), None))?;
        let terminal_receipt = events
            .iter()
            .rev()
            .find(|event| {
                event.links.run_id() == Some(context.run_id.transport())
                    && event.event_type == task_outcome_event_type(request.task_outcome())
            })
            .cloned()
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "runtime_task_terminal_missing",
                    "export_evidence",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })?;
        let pipeline = self.capture_pipeline_summary(
            &events,
            validated.correlation_id(),
            *context.run_id.transport(),
        )?;
        let documents = runtime_evidence_documents(
            context.run_id,
            context.task_id,
            request.task_outcome(),
            &terminal_receipt,
            &events,
        )?;
        let archive_context = ArtifactWriteContext::new(
            validated.task_artifact_links(context.run_id),
            task_links,
            unix_ms_now().map_err(RequestFailure::poison_without_terminal)?,
        );
        let export_request = EvidenceExportRequest {
            output_path: PathBuf::from(request.output_path()),
            identity: EvidenceExportIdentity {
                run_id: *context.run_id.transport(),
                correlation_id: validated.correlation_id(),
                package: context.package.clone(),
                task_outcome: request.task_outcome(),
                terminal_receipt: terminal_receipt.clone(),
                projection_profile: actingcommand_contract::ProjectionProfile::Forensic,
                retention_class: RetentionClass::DebugFull,
                archive_redaction_state: ArtifactRedactionState::NotRequired,
            },
            events,
            pipeline,
            documents,
            archive_context,
        };
        let mut exporter = EvidenceExporter::open(self.artifacts.root()).map_err(|error| {
            RequestFailure::request(
                evidence_request_error(error.code()),
                RuntimeReceiptState::Failed,
                Some(terminal_from_projected(&terminal_receipt)),
            )
        })?;
        let mut sink = RuntimeArtifactEventSink {
            ledger: &self.ledger,
            events: &self.events,
        };
        let receipt = match exporter.export(export_request, &mut sink) {
            Ok(receipt) => receipt,
            Err(error) => {
                let failure_terminal = self.latest_evidence_export_terminal(
                    validated.correlation_id(),
                    EventType::ArtifactExportFailed,
                )?;
                return Err(RequestFailure::request(
                    evidence_request_error(error.code()),
                    RuntimeReceiptState::Failed,
                    failure_terminal.or_else(|| Some(terminal_from_projected(&terminal_receipt))),
                ));
            }
        };
        let response_terminal = self
            .latest_evidence_export_terminal(
                validated.correlation_id(),
                EventType::ArtifactExportCompleted,
            )?
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "evidence_export_terminal_missing",
                    "export_evidence",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })?;
        let output_path = receipt.output_path().to_str().ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "evidence_output_path_invalid",
                "export_evidence",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        let manifest = receipt.manifest();
        let summary = RuntimeEvidenceExportSummary::new(
            validated.correlation_id(),
            *context.run_id.transport(),
            request.task_outcome(),
            manifest.evidence_completeness,
            output_path,
            receipt.zip_byte_count(),
            receipt.zip_sha256(),
            receipt.manifest_sha256(),
            receipt.archive().project(true),
            RuntimeEvidenceScreenshotCounts {
                captured: manifest.screenshot_counts.captured,
                deduplicated: manifest.screenshot_counts.deduplicated,
                dropped: manifest.screenshot_counts.dropped,
                persisted: manifest.screenshot_counts.persisted,
            },
            terminal_receipt,
        )
        .map_err(|error| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                error.code(),
                "export_evidence",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        context.completed_export = Some(CompletedEvidenceExport {
            request_output_path: request.output_path().to_string(),
            task_outcome: request.task_outcome(),
            response_terminal,
            summary: summary.clone(),
        });
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(response_terminal),
            result: RuntimeResult::EvidenceExportCompleted {
                summary: Box::new(summary),
            },
        })
    }

    fn capture_pipeline_summary(
        &self,
        events: &[actingcommand_contract::ProjectedEvent],
        correlation_id: CorrelationId,
        run_id: actingcommand_contract::RunId,
    ) -> Result<CapturePipelineSummary, RequestFailure> {
        let mut seen = BTreeSet::new();
        let mut frames = Vec::new();
        for projected in events
            .iter()
            .flat_map(|event| &event.artifacts)
            .filter(|artifact| artifact.kind() == ArtifactKind::CaptureFrame)
        {
            if !seen.insert(projected.artifact_id) {
                continue;
            }
            if projected.correlation_id != Some(correlation_id)
                || projected.run_id.is_some_and(|actual| actual != run_id)
            {
                return Err(RequestFailure::request(
                    evidence_request_error("evidence_capture_identity_mismatch"),
                    RuntimeReceiptState::Failed,
                    None,
                ));
            }
            let verified = self
                .artifacts
                .verify_recovery_reference(projected)
                .map_err(|error| {
                    RequestFailure::request(
                        evidence_request_error(error.code()),
                        RuntimeReceiptState::Failed,
                        None,
                    )
                })?;
            frames.push(PersistedFrameEvidence {
                frame_index: frames.len(),
                pinned_reason: None,
                artifact: verified.into_reference(),
            });
        }
        let persisted = u64::try_from(frames.len()).map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "evidence_frame_count_overflow",
                "export_evidence",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        Ok(CapturePipelineSummary {
            counts: CapturePipelineCounts {
                captured: persisted,
                deduplicated: 0,
                dropped: 0,
                persisted,
            },
            evidence_completeness: EvidenceCompleteness::Complete,
            pinned: Vec::new(),
            frames,
        })
    }

    fn latest_evidence_export_terminal(
        &self,
        correlation_id: CorrelationId,
        event_type: EventType,
    ) -> Result<Option<TerminalEvent>, RequestFailure> {
        self.ledger
            .project(
                EventQuery {
                    correlation_id: Some(correlation_id),
                    event_type: Some(event_type),
                    ..EventQuery::default()
                },
                actingcommand_contract::ProjectionProfile::Forensic,
            )
            .map(|events| events.last().map(terminal_from_projected))
            .map_err(|_| RequestFailure::poison(ledger_error("query_evidence_terminal"), None))
    }

    fn record_authoring_event(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        event: &ResourceAuthoringEvent,
    ) -> Result<OperationSuccess, RequestFailure> {
        let severity = if event.phase() == ResourceAuthoringPhase::PromoteFailed {
            EventSeverity::Error
        } else {
            EventSeverity::Info
        };
        let persisted = self.append_event(
            severity,
            EventSource::Lab,
            OriginModule::ResourceTooling,
            EventActor::Lab,
            validated.event_links(None, None, None),
            ResourceAuthoringPayloadDraft::event(
                event.phase(),
                event.draft_id(),
                event.target_label(),
                event.target_fingerprint(),
                event.changed_paths().to_vec(),
                event.failure_code().map(str::to_owned),
                AuditInput::new(),
            ),
        )?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&persisted)),
            result: RuntimeResult::AuthoringEventRecorded {
                phase: event.phase(),
            },
        })
    }

    fn record_debug_event(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        event: &RuntimeDebugEvent,
    ) -> Result<OperationSuccess, RequestFailure> {
        let context = lock(&self.debug_runs, "read_runtime_debug_event_context")?
            .get(&validated.correlation_id())
            .cloned();
        if event.operation() == RuntimeDebugOperation::LabRun && context.is_none() {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    "runtime_debug_context_missing",
                    "record_runtime_debug_event",
                    RuntimeErrorCode::InvalidRequest,
                ),
                RuntimeReceiptState::Denied,
                None,
            ));
        }
        let links = context.as_ref().map_or_else(
            || validated.event_links(None, None, None),
            |context| validated.task_event_links(context.task_id, context.run_id),
        );
        let action = event.operation().event_action();
        let persisted = match (event.operation(), event.phase()) {
            (_, RuntimeDebugPhase::Requested) => self.append_event(
                EventSeverity::Info,
                EventSource::Lab,
                OriginModule::Actinglab,
                EventActor::Lab,
                links,
                ClientPayloadDraft::lab_request(action, AuditInput::new()),
            )?,
            (RuntimeDebugOperation::LabRun, RuntimeDebugPhase::Progress) => self.append_event(
                EventSeverity::Info,
                EventSource::Lab,
                OriginModule::Actinglab,
                EventActor::Lab,
                links,
                TaskPayloadDraft::step_finished(action, AuditInput::new()),
            )?,
            (RuntimeDebugOperation::LabRun, RuntimeDebugPhase::Completed) => self.append_event(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                links,
                TaskPayloadDraft::completed(action, event.effect_disposition(), AuditInput::new()),
            )?,
            (RuntimeDebugOperation::LabRun, RuntimeDebugPhase::Failed) => self.append_event(
                EventSeverity::Error,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                links,
                TaskPayloadDraft::failed(
                    action,
                    DiagnosticCode::RuntimeDiagnostic,
                    event.effect_disposition(),
                    AuditInput::new(),
                ),
            )?,
            (_, RuntimeDebugPhase::Completed) => self.append_event(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                links,
                CommandPayloadDraft::validated(
                    action,
                    event.effect_disposition(),
                    AuditInput::new(),
                ),
            )?,
            (_, RuntimeDebugPhase::Failed) => self.append_event(
                EventSeverity::Error,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                links,
                CommandPayloadDraft::rejected(
                    action,
                    DiagnosticCode::CommandRejected,
                    event.effect_disposition(),
                    AuditInput::new(),
                ),
            )?,
            (_, RuntimeDebugPhase::Progress) => {
                return Err(RequestFailure::request(
                    RuntimeHostError::request(
                        "runtime_debug_event_invalid",
                        "record_runtime_debug_event",
                        RuntimeErrorCode::InvalidRequest,
                    ),
                    RuntimeReceiptState::Denied,
                    None,
                ));
            }
        };
        if event.operation() == RuntimeDebugOperation::LabRun {
            let outcome = match event.phase() {
                RuntimeDebugPhase::Completed => Some(TaskOutcome::Success),
                RuntimeDebugPhase::Failed => Some(TaskOutcome::Failure),
                _ => None,
            };
            if let Some(outcome) = outcome {
                let mut debug_runs = lock(&self.debug_runs, "update_runtime_debug_run_outcome")?;
                let context = debug_runs
                    .get_mut(&validated.correlation_id())
                    .ok_or_else(|| {
                        RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                            "runtime_debug_context_missing_after_terminal",
                            "record_runtime_debug_event",
                            RuntimeErrorCode::RuntimeFatal,
                        ))
                    })?;
                context.terminal_outcome = Some(outcome);
            }
        }
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&persisted)),
            result: RuntimeResult::DebugEventRecorded {
                phase: event.phase(),
            },
        })
    }

    fn record_client_action(
        &self,
        request: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        action: &ClientActionRecord,
    ) -> Result<OperationSuccess, RequestFailure> {
        let _gate = lock(&self.governance_write_gate, "record_client_action")
            .map_err(RequestFailure::poison_without_terminal)?;
        if let Some(existing) = self.client_fact_replay(request, EventType::ClientAction)? {
            let EventPayload::Client(ClientPayload::Action(payload)) = existing.payload() else {
                return Err(RequestFailure::poison_without_terminal(
                    RuntimeHostError::fatal(
                        "client_action_replay_payload_mismatch",
                        "record_client_action",
                        RuntimeErrorCode::RuntimeFatal,
                    ),
                ));
            };
            if payload.record() != action {
                return Err(client_fact_conflict(
                    "client_action_replay_conflict",
                    "record_client_action",
                ));
            }
            return Ok(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: Some(terminal(&existing)),
                result: RuntimeResult::ClientActionRecorded,
            });
        }
        let instance_id = match action.instance_alias() {
            Some(alias) => Some(self.registered_instance_id(alias)?),
            None => None,
        };
        let persisted = self.append_event(
            EventSeverity::Info,
            request.source(),
            OriginModule::Governance,
            request.actor(),
            validated.event_links(instance_id, None, None),
            ClientPayloadDraft::action(action.clone(), AuditInput::new()),
        )?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&persisted)),
            result: RuntimeResult::ClientActionRecorded,
        })
    }

    fn record_approval_decision(
        &self,
        request: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        decision: &ApprovalDecisionRecord,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        if request.actor() != EventActor::User
            || request.source() != EventSource::Ui
            || !lock(
                &self.governance_connections,
                "authorize_approval_connection",
            )?
            .contains(&connection_id)
        {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    "governance_authority_required",
                    "record_approval_decision",
                    RuntimeErrorCode::InvalidRequest,
                ),
                RuntimeReceiptState::Denied,
                None,
            ));
        }
        let _gate = lock(&self.governance_write_gate, "record_approval_decision")
            .map_err(RequestFailure::poison_without_terminal)?;
        let approvals = ApprovalProjection::recover(&self.ledger, Arc::clone(&self.state))
            .map_err(RequestFailure::poison_without_terminal)?;
        if let Some(existing) = self.client_fact_replay(request, EventType::ApprovalDecision)? {
            let EventPayload::Approval(ApprovalPayload::Decision(payload)) = existing.payload()
            else {
                return Err(RequestFailure::poison_without_terminal(
                    RuntimeHostError::fatal(
                        "approval_replay_payload_mismatch",
                        "record_approval_decision",
                        RuntimeErrorCode::RuntimeFatal,
                    ),
                ));
            };
            if payload.decision() != decision {
                return Err(client_fact_conflict(
                    "approval_replay_conflict",
                    "record_approval_decision",
                ));
            }
            return Ok(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: Some(terminal(&existing)),
                result: RuntimeResult::ApprovalDecisionRecorded {
                    approval_id: decision.approval_id().to_owned(),
                    disposition: decision.disposition(),
                },
            });
        }
        approvals
            .validate_transition(decision)
            .map_err(|error| RequestFailure::request(error, RuntimeReceiptState::Denied, None))?;
        let persisted = self.append_event(
            EventSeverity::Info,
            EventSource::Ui,
            OriginModule::Governance,
            EventActor::User,
            validated.event_links(None, None, None),
            ApprovalPayloadDraft::decision(decision.clone(), AuditInput::new()),
        )?;
        ApprovalProjection::recover(&self.ledger, Arc::clone(&self.state))
            .map_err(RequestFailure::poison_without_terminal)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&persisted)),
            result: RuntimeResult::ApprovalDecisionRecorded {
                approval_id: decision.approval_id().to_owned(),
                disposition: decision.disposition(),
            },
        })
    }

    fn authenticate_governance(
        &self,
        request: &RuntimeRequest,
        connection_id: ConnectionId,
        capability: &str,
    ) -> Result<OperationSuccess, RequestFailure> {
        if request.actor() != EventActor::User || request.source() != EventSource::Ui {
            return Err(governance_authentication_denied(
                "governance_origin_untrusted",
            ));
        }
        let expected = self.governance_capability_sha256.ok_or_else(|| {
            governance_authentication_denied("governance_authentication_unavailable")
        })?;
        let actual: [u8; 32] = Sha256::digest(capability.as_bytes()).into();
        if !constant_time_digest_eq(&expected, &actual) {
            return Err(governance_authentication_denied(
                "governance_authentication_failed",
            ));
        }
        lock(
            &self.governance_connections,
            "authenticate_governance_connection",
        )?
        .insert(connection_id);
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::GovernanceAuthenticated,
        })
    }

    fn start_agent_session(
        &self,
        request: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        wake_id: AgentWakeId,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.require_agent_dispatcher("start_agent_session")?;
        let _gate = lock(&self.agent_write_gate, "start_agent_session")
            .map_err(RequestFailure::poison_without_terminal)?;
        let session_id = self
            .events
            .issuer()
            .mint_agent_session_id()
            .map(|issued| *issued.transport())
            .map_err(|_| RequestFailure::poison_without_terminal(runtime_identifier_error()))?;
        let started_at_unix_ms = unix_ms_now().map_err(RequestFailure::poison_without_terminal)?;
        let preparation = lock(&self.agent_dispatcher, "start_agent_session")?
            .prepare_start(wake_id, session_id, started_at_unix_ms)
            .map_err(agent_request_failure)?;
        let (status, terminal) = match preparation {
            AgentSessionPreparation::Replay(status) => (status, None),
            AgentSessionPreparation::New(data) => {
                let persisted = self.append_event(
                    EventSeverity::Info,
                    request.source(),
                    OriginModule::AgentDispatcher,
                    request.actor(),
                    validated.event_links(Some(data.status().instance_id()), None, None),
                    AgentPayloadDraft::session_started(data.clone(), AuditInput::new()),
                )?;
                lock(&self.agent_dispatcher, "commit_agent_session_start")?
                    .apply_event(&persisted, None)
                    .map_err(RequestFailure::poison_without_terminal)?;
                (data.status().clone(), Some(terminal(&persisted)))
            }
        };
        let context = self.agent_session_context(status)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal,
            result: RuntimeResult::AgentSessionOpened {
                context: Box::new(context),
            },
        })
    }

    fn resume_agent_session(
        &self,
        request: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        session_id: AgentSessionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.require_agent_dispatcher("resume_agent_session")?;
        let _gate = lock(&self.agent_write_gate, "resume_agent_session")
            .map_err(RequestFailure::poison_without_terminal)?;
        let observed_at_unix_ms = unix_ms_now().map_err(RequestFailure::poison_without_terminal)?;
        let data = lock(&self.agent_dispatcher, "resume_agent_session")?
            .prepare_resume(session_id, observed_at_unix_ms)
            .map_err(agent_request_failure)?;
        let persisted = self.append_event(
            EventSeverity::Info,
            request.source(),
            OriginModule::AgentDispatcher,
            request.actor(),
            validated.event_links(Some(data.status().instance_id()), None, None),
            AgentPayloadDraft::session_resumed(data.clone(), AuditInput::new()),
        )?;
        lock(&self.agent_dispatcher, "commit_agent_session_resume")?
            .apply_event(&persisted, None)
            .map_err(RequestFailure::poison_without_terminal)?;
        let context = self.agent_session_context(data.status().clone())?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&persisted)),
            result: RuntimeResult::AgentSessionObserved {
                context: Box::new(context),
            },
        })
    }

    fn agent_session_status(
        &self,
        session_id: AgentSessionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.require_agent_dispatcher("read_agent_session")?;
        let status = lock(&self.agent_dispatcher, "read_agent_session")?
            .session(session_id)
            .map_err(agent_request_failure)?
            .clone();
        let context = self.agent_session_context(status)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::AgentSessionObserved {
                context: Box::new(context),
            },
        })
    }

    fn record_agent_response(
        &self,
        request: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        response: &AgentSessionResponse,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.require_agent_dispatcher("record_agent_response")?;
        let _gate = lock(&self.agent_write_gate, "record_agent_response")
            .map_err(RequestFailure::poison_without_terminal)?;
        let observed_at_unix_ms = unix_ms_now().map_err(RequestFailure::poison_without_terminal)?;
        let preparation = lock(&self.agent_dispatcher, "record_agent_response")?
            .prepare_response(request.request_id(), response, observed_at_unix_ms)
            .map_err(agent_request_failure)?;
        let (data, payload, severity) = match preparation {
            AgentResponsePreparation::Replay(status) => {
                return Ok(OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: None,
                    result: RuntimeResult::AgentResponseRecorded { status },
                });
            }
            AgentResponsePreparation::Retry(data) => {
                let payload = AgentPayloadDraft::response_recorded(data.clone(), AuditInput::new());
                (data, payload, EventSeverity::Warning)
            }
            AgentResponsePreparation::Complete(data) => {
                let payload = AgentPayloadDraft::session_completed(data.clone(), AuditInput::new());
                (data, payload, EventSeverity::Info)
            }
            AgentResponsePreparation::Escalate(data) => {
                let payload = AgentPayloadDraft::session_escalated(data.clone(), AuditInput::new());
                (data, payload, EventSeverity::Warning)
            }
        };
        let persisted = self.append_event(
            severity,
            request.source(),
            OriginModule::AgentDispatcher,
            request.actor(),
            validated.event_links(Some(data.status().instance_id()), None, None),
            payload,
        )?;
        lock(&self.agent_dispatcher, "commit_agent_response")?
            .apply_event(&persisted, None)
            .map_err(RequestFailure::poison_without_terminal)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&persisted)),
            result: RuntimeResult::AgentResponseRecorded {
                status: data.status().clone(),
            },
        })
    }

    fn agent_session_context(
        &self,
        status: AgentSessionStatus,
    ) -> Result<AgentSessionContext, RequestFailure> {
        lock(&self.agent_dispatcher, "project_agent_session")?
            .context(&self.ledger, status)
            .map_err(agent_request_failure)
    }

    fn compile_proposal(
        &self,
        proposal: &CatalogProposal,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.verify_proposal_reports(proposal)?;
        let prepared = {
            let policy = lock(&self.policy, "compile_proposal")?;
            let generation = policy.active_generation().ok_or_else(|| {
                proposal_request_failure(RuntimeHostError::request(
                    "proposal_base_catalog_unavailable",
                    "compile_proposal",
                    RuntimeErrorCode::InvalidRequest,
                ))
            })?;
            let sources = policy.active_sources().ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "proposal_base_sources_unavailable",
                    "compile_proposal",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })?;
            prepare_proposal(&generation, &sources, proposal).map_err(proposal_request_failure)?
        };
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::ProposalEvaluated {
                preview: prepared.preview().clone(),
            },
        })
    }

    fn promote_proposal(
        &self,
        proposal: &CatalogProposal,
    ) -> Result<OperationSuccess, RequestFailure> {
        let _gate = lock(&self.proposal_write_gate, "promote_proposal")
            .map_err(RequestFailure::poison_without_terminal)?;
        self.verify_proposal_reports(proposal)?;
        let (prepared, current) = {
            let policy = lock(&self.policy, "prepare_proposal_promotion")?;
            let base = policy
                .load_generation(proposal.base_catalog_hash())
                .map_err(proposal_request_failure)?;
            let prepared = prepare_proposal(base.generation(), base.sources(), proposal)
                .map_err(proposal_request_failure)?;
            (prepared, policy.active_generation())
        };
        let (preview, sources) = prepared.into_ready().map_err(proposal_request_failure)?;
        let target_hash = preview.target_catalog_hash().ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "proposal_target_hash_missing",
                "promote_proposal",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        let current = current.ok_or_else(|| {
            proposal_request_failure(RuntimeHostError::request(
                "proposal_active_catalog_unavailable",
                "promote_proposal",
                RuntimeErrorCode::InvalidRequest,
            ))
        })?;
        if current.catalog_hash() != proposal.base_catalog_hash()
            && current.catalog_hash() != target_hash
        {
            return Err(proposal_request_failure(RuntimeHostError::request(
                "proposal_active_catalog_changed",
                "promote_proposal",
                RuntimeErrorCode::InvalidRequest,
            )));
        }
        let approvals = ApprovalProjection::recover(&self.ledger, Arc::clone(&self.state))
            .map_err(RequestFailure::poison_without_terminal)?;
        let mut approval_fact_ids = approvals.active_for_plan(
            preview.proposal_id(),
            target_hash,
            preview.target_catalog_version(),
        );
        if approval_fact_ids.is_empty() {
            return Err(proposal_request_failure(RuntimeHostError::request(
                "proposal_approval_missing",
                "promote_proposal",
                RuntimeErrorCode::InvalidRequest,
            )));
        }
        if preview.class() == ProposalClass::A {
            let template_approvals = approvals.active_for_catalog(
                proposal.base_catalog_hash(),
                proposal.base_catalog_version(),
            );
            if template_approvals.is_empty() {
                return Err(proposal_request_failure(RuntimeHostError::request(
                    "proposal_template_approval_missing",
                    "promote_proposal",
                    RuntimeErrorCode::InvalidRequest,
                )));
            }
            approval_fact_ids.extend(template_approvals);
        }
        let promotion =
            ProposalPromotion::new(preview.clone(), approval_fact_ids.into_iter().collect())
                .map_err(|_| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        "proposal_promotion_invalid",
                        "promote_proposal",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?;
        let authorization =
            CatalogPromotionAuthorization::new(proposal, &promotion).map_err(|_| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "proposal_authorization_invalid",
                    "promote_proposal",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })?;
        if current.catalog_hash() != target_hash {
            let generation = self
                .activate_policy_catalog_with_authorization(&sources, Some(authorization))
                .map_err(proposal_request_failure)?;
            if generation.catalog_hash() != target_hash
                || generation.catalog_version() != preview.target_catalog_version()
            {
                return Err(RequestFailure::poison_without_terminal(
                    RuntimeHostError::fatal(
                        "proposal_activation_mismatch",
                        "promote_proposal",
                        RuntimeErrorCode::RuntimeFatal,
                    ),
                ));
            }
        }
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::ProposalPromoted { promotion },
        })
    }

    fn verify_proposal_reports(&self, proposal: &CatalogProposal) -> Result<(), RequestFailure> {
        let verified_events = self
            .ledger
            .query(EventQuery {
                event_type: Some(EventType::ArtifactVerified),
                ..EventQuery::default()
            })
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error("verify_proposal_reports"))
            })?;
        for reference in proposal.report_refs() {
            if !proposal_report_is_verified(&verified_events, reference) {
                return Err(proposal_request_failure(RuntimeHostError::request(
                    "proposal_report_unverified",
                    "verify_proposal_reports",
                    RuntimeErrorCode::InvalidRequest,
                )));
            }
            read_projected_verified(self.artifacts.root(), reference).map_err(|_| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "proposal_report_unavailable",
                    "verify_proposal_reports",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })?;
        }
        Ok(())
    }

    fn prepare_strategic_report(
        &self,
        report: &StrategicReport,
        evidence: &[ProjectedArtifactReference],
    ) -> RuntimeHostResult<StrategicPlanPreparation> {
        let result = (|| {
            let _gate = lock(&self.proposal_write_gate, "prepare_strategic_report")?;
            report.validate().map_err(|_| {
                RuntimeHostError::request(
                    "strategic_report_invalid",
                    "prepare_strategic_report",
                    RuntimeErrorCode::InvalidRequest,
                )
            })?;
            self.verify_strategic_evidence(report, evidence)?;
            let ledger_position = self
                .ledger
                .latest_sequence()
                .map_err(|_| ledger_error("prepare_strategic_report"))?;
            if report.as_of_ledger_position() > ledger_position {
                return Err(RuntimeHostError::request(
                    "strategic_report_position_unavailable",
                    "prepare_strategic_report",
                    RuntimeErrorCode::InvalidRequest,
                ));
            }
            let loaded = lock(&self.policy, "prepare_strategic_report")?
                .active_loaded()
                .ok_or_else(|| {
                    RuntimeHostError::request(
                        "strategic_catalog_unavailable",
                        "prepare_strategic_report",
                        RuntimeErrorCode::InvalidRequest,
                    )
                })?;
            let projection =
                project_strategic_report(loaded.compiled(), report).map_err(|error| {
                    RuntimeHostError::request(
                        error.code(),
                        "prepare_strategic_report",
                        RuntimeErrorCode::InvalidRequest,
                    )
                })?;
            let bytes = report.canonical_bytes().map_err(|_| {
                RuntimeHostError::fatal(
                    "strategic_report_encode_failed",
                    "prepare_strategic_report",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
            let report_reference = self.store_or_reuse_strategic_report(&bytes)?;
            let proposal = build_strategy_proposal(&projection, report_reference.clone())?;
            let preview = proposal
                .as_ref()
                .map(|proposal| {
                    prepare_proposal(loaded.generation(), loaded.sources(), proposal)
                        .and_then(|prepared| prepared.into_ready().map(|(preview, _)| preview))
                })
                .transpose()?;
            StrategicPlanPreparation::new(report_reference, projection, proposal, preview)
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    fn verify_strategic_evidence(
        &self,
        report: &StrategicReport,
        evidence: &[ProjectedArtifactReference],
    ) -> RuntimeHostResult<()> {
        let verified_events = self
            .ledger
            .query(EventQuery {
                event_type: Some(EventType::ArtifactVerified),
                ..EventQuery::default()
            })
            .map_err(|_| ledger_error("verify_strategic_evidence"))?;
        let mut pointers = Vec::with_capacity(evidence.len());
        for reference in evidence {
            reference.validate().map_err(|_| {
                RuntimeHostError::request(
                    "strategic_evidence_invalid",
                    "verify_strategic_evidence",
                    RuntimeErrorCode::InvalidRequest,
                )
            })?;
            let verified_sequence = proposal_report_verified_sequence(&verified_events, reference);
            if reference.object_key().is_none()
                || reference.redaction_state() == ArtifactRedactionState::Pending
                || verified_sequence.is_none()
                || verified_sequence
                    .is_some_and(|sequence| sequence > report.as_of_ledger_position())
            {
                return Err(RuntimeHostError::request(
                    "strategic_evidence_unverified",
                    "verify_strategic_evidence",
                    RuntimeErrorCode::InvalidRequest,
                ));
            }
            read_projected_verified(self.artifacts.root(), reference).map_err(|_| {
                RuntimeHostError::fatal(
                    "strategic_evidence_unavailable",
                    "verify_strategic_evidence",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
            pointers.push(strategic_evidence_pointer(reference)?);
        }
        pointers.sort();
        if pointers != report.evidence() {
            return Err(RuntimeHostError::request(
                "strategic_evidence_mismatch",
                "verify_strategic_evidence",
                RuntimeErrorCode::InvalidRequest,
            ));
        }
        Ok(())
    }

    fn store_or_reuse_strategic_report(
        &self,
        bytes: &[u8],
    ) -> RuntimeHostResult<ProjectedArtifactReference> {
        let sha256 = format!("sha256:{:x}", Sha256::digest(bytes));
        let events = self
            .ledger
            .query(EventQuery {
                event_type: Some(EventType::ArtifactVerified),
                ..EventQuery::default()
            })
            .map_err(|_| ledger_error("find_strategic_report"))?;
        let mut existing = Vec::new();
        for reference in events
            .iter()
            .flat_map(PersistedEvent::artifacts)
            .filter(|reference| {
                reference.kind() == ArtifactKind::StrategyReport && reference.sha256() == sha256
            })
        {
            let reference = reference.project(true);
            existing.push((artifact_id_text(&reference)?, reference));
        }
        existing.sort_by(|left, right| left.0.cmp(&right.0));
        if let Some((_, reference)) = existing.into_iter().next() {
            let stored = read_projected_verified(self.artifacts.root(), &reference)
                .map_err(|_| artifact_store_error("read_strategic_report"))?;
            if stored != bytes {
                return Err(RuntimeHostError::fatal(
                    "strategic_report_identity_conflict",
                    "read_strategic_report",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            return Ok(reference);
        }
        let context = ArtifactWriteContext::new(
            ArtifactLinksDraft::default(),
            self.events.system_links()?,
            unix_ms_now()?,
        );
        let mut sink = RuntimeArtifactEventSink {
            ledger: &self.ledger,
            events: &self.events,
        };
        self.artifacts
            .put(
                ArtifactWriteRequest::new(
                    ArtifactKind::StrategyReport,
                    bytes,
                    context,
                    ArtifactIssuePolicy::new(
                        ArtifactProducer::ArtifactStore,
                        RetentionClass::Adaptive,
                        ArtifactRedactionState::Applied,
                    ),
                ),
                &mut sink,
            )
            .map(|stored| stored.reference().project(true))
            .map_err(|_| artifact_store_error("store_strategic_report"))
    }

    #[cfg(test)]
    fn store_test_report(&self, bytes: &[u8]) -> RuntimeHostResult<ProjectedArtifactReference> {
        let context = ArtifactWriteContext::new(
            ArtifactLinksDraft::default(),
            self.events.system_links()?,
            unix_ms_now()?,
        );
        let mut sink = RuntimeArtifactEventSink {
            ledger: &self.ledger,
            events: &self.events,
        };
        self.artifacts
            .put(
                ArtifactWriteRequest::new(
                    ArtifactKind::TextReport,
                    bytes,
                    context,
                    ArtifactIssuePolicy::new(
                        ArtifactProducer::ArtifactStore,
                        RetentionClass::Adaptive,
                        ArtifactRedactionState::NotRequired,
                    ),
                ),
                &mut sink,
            )
            .map(|stored| stored.reference().project(true))
            .map_err(|_| artifact_store_error("store_test_report"))
    }

    fn client_fact_replay(
        &self,
        request: &RuntimeRequest,
        event_type: EventType,
    ) -> Result<Option<PersistedEvent>, RequestFailure> {
        let mut events = self
            .ledger
            .query(EventQuery {
                request_id: Some(request.request_id()),
                ..EventQuery::default()
            })
            .map_err(|_| RequestFailure::poison(ledger_error("query_client_fact_replay"), None))?;
        if events.len() > 1 {
            if events.iter().all(|event| {
                event.event_type() == event_type
                    && event.origin().module() == OriginModule::Governance
            }) {
                return Err(RequestFailure::poison_without_terminal(
                    RuntimeHostError::fatal(
                        "client_fact_replay_ambiguous",
                        "query_client_fact_replay",
                        RuntimeErrorCode::RuntimeFatal,
                    ),
                ));
            }
            return Err(client_fact_conflict(
                "client_fact_request_id_conflict",
                "query_client_fact_replay",
            ));
        }
        let Some(event) = events.pop() else {
            return Ok(None);
        };
        if event.event_type() != event_type
            || event.origin().source() != request.source()
            || event.origin().actor() != request.actor()
            || event.origin().module() != OriginModule::Governance
            || event.links().correlation_id() != Some(&request.correlation_id())
        {
            return Err(client_fact_conflict(
                "client_fact_replay_origin_conflict",
                "query_client_fact_replay",
            ));
        }
        Ok(Some(event))
    }

    fn registered_instance_id(&self, instance_alias: &str) -> Result<InstanceId, RequestFailure> {
        lock(&self.registered_instances, "resolve_client_action_instance")?
            .values()
            .find(|instance| instance.instance_alias == instance_alias)
            .map(RegisteredInstance::instance_id)
            .ok_or_else(|| {
                RequestFailure::request(
                    RuntimeHostError::request(
                        "instance_unknown",
                        "resolve_client_action_instance",
                        RuntimeErrorCode::InstanceUnknown,
                    ),
                    RuntimeReceiptState::Denied,
                    None,
                )
            })
    }

    fn project_interface(
        &self,
        request: &ProjectInterfaceRequest,
    ) -> Result<OperationSuccess, RequestFailure> {
        request.negotiate().map_err(|error| {
            project_interface_failure(RuntimeHostError::request(
                error.code(),
                "negotiate_project_interface",
                RuntimeErrorCode::ProtocolInvalid,
            ))
        })?;
        let status = self.control_plane_status_projection()?;
        let facts = {
            let _gate = lock(&self.fact_write_gate, "project_runtime_facts")?;
            self.synchronize_fact_store_under_gate()
                .map_err(RequestFailure::poison_without_terminal)?;
            lock(&self.facts, "project_runtime_facts")?.active_records()
        };
        let approvals = ApprovalProjection::recover(&self.ledger, Arc::clone(&self.state))
            .map_err(RequestFailure::poison_without_terminal)?
            .records();
        let ledger_position = self.ledger.latest_sequence().map_err(|_| {
            RequestFailure::poison_without_terminal(ledger_error("project_runtime_position"))
        })?;
        let decision_page = request
            .decision_page()
            .cloned()
            .unwrap_or_else(ProjectDecisionPageRequest::default);
        let (catalog, decisions) = {
            let policy = lock(&self.policy, "project_runtime_policy")?;
            (
                policy.active_loaded(),
                policy.project_dispatches(ledger_position, &decision_page)?,
            )
        };
        let diagnostics = self
            .ledger
            .query(EventQuery {
                to_sequence: Some(ledger_position),
                minimum_severity: Some(EventSeverity::Warning),
                ..EventQuery::default()
            })
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error("project_runtime_diagnostics"))
            })?
            .into_iter()
            .map(|event| ProjectDiagnosticProjection {
                sequence: event.sequence(),
                timestamp_unix_ms: event.timestamp_unix_ms(),
                severity: event.severity(),
                event_type: event.event_type(),
            })
            .collect();
        let fatal = self
            .fatal
            .current()
            .map_err(RequestFailure::poison_without_terminal)?
            .is_some();
        let response = ProjectInterfaceProjection {
            ledger_position,
            catalog,
            instances: status,
            facts,
            decisions,
            approvals,
            diagnostics: retain_recent_diagnostics(diagnostics),
            fatal,
        }
        .into_response(request)
        .map_err(project_interface_failure)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::ProjectInterface {
                response: Box::new(response),
            },
        })
    }

    fn control_plane_status(&self) -> Result<OperationSuccess, RequestFailure> {
        let status = self.control_plane_status_projection()?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::Status { status },
        })
    }

    fn control_plane_status_projection(&self) -> Result<RuntimeControlPlaneStatus, RequestFailure> {
        let now = self
            .monotonic_ms()
            .map_err(RequestFailure::poison_without_terminal)?;
        let instances = lock(&self.registered_instances, "read_runtime_status_registry")?
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let scheduler = lock(&self.scheduler, "read_runtime_status_scheduler")?;
        let mut projected = Vec::with_capacity(instances.len());
        for instance in instances {
            let instance_id = instance.instance_id();
            let active = scheduler.active_lease(instance_id);
            let queued_request_count =
                u32::try_from(scheduler.queued_count(instance_id)).map_err(|_| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        "runtime_status_queue_count_overflow",
                        "project_runtime_control_plane_status",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?;
            projected.push(
                RuntimeInstanceStatus::new(
                    instance.instance_alias,
                    instance_id,
                    active.is_some(),
                    queued_request_count,
                    scheduler.cooldown_active(instance_id, now),
                    active
                        .as_ref()
                        .is_some_and(|lease| lease.destructive_step_active()),
                    active
                        .as_ref()
                        .is_some_and(|lease| lease.preempt_requested()),
                )
                .map_err(|_| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        "runtime_status_projection_invalid",
                        "project_runtime_control_plane_status",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?,
            );
        }
        let status = RuntimeControlPlaneStatus::new(self.owner_epoch, projected).map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "runtime_status_projection_invalid",
                "project_runtime_control_plane_status",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        Ok(status)
    }

    fn monitor_status(&self) -> Result<OperationSuccess, RequestFailure> {
        let status = lock(&self.monitor_registry, "read_monitor_registry")?
            .status(self.owner_epoch)
            .map_err(RequestFailure::poison_without_terminal)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::MonitorStatus { status },
        })
    }

    fn configure_monitor(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        policy: RuntimeMonitorPolicy,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        let links = self.append_client_command_intent(
            original,
            request,
            resolved.instance_id(),
            EventAction::MonitorConfigure,
        )?;
        let update = match lock(&self.monitor_registry, "configure_monitor_registry")?.configure(
            instance_alias,
            policy,
            unix_ms_now().map_err(RequestFailure::from)?,
        ) {
            Ok(update) => update,
            Err(error) => {
                return Err(self.monitor_mutation_failure(
                    links,
                    EventAction::MonitorConfigure,
                    error,
                )?);
            }
        };
        self.monitor_mutation_success(links, EventAction::MonitorConfigure, update, |status| {
            RuntimeResult::MonitorConfigured { status }
        })
    }

    fn clear_monitor(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        let links = self.append_client_command_intent(
            original,
            request,
            resolved.instance_id(),
            EventAction::MonitorClear,
        )?;
        let update =
            match lock(&self.monitor_registry, "clear_monitor_registry")?.clear(instance_alias) {
                Ok(update) => update,
                Err(error) => {
                    return Err(self.monitor_mutation_failure(
                        links,
                        EventAction::MonitorClear,
                        error,
                    )?);
                }
            };
        self.monitor_mutation_success(links, EventAction::MonitorClear, update, |status| {
            RuntimeResult::MonitorCleared { status }
        })
    }

    fn monitor_mutation_success(
        &self,
        links: EventLinksDraft,
        action: EventAction,
        update: MonitorUpdate,
        result: impl FnOnce(actingcommand_contract::RuntimeMonitorInstanceStatus) -> RuntimeResult,
    ) -> Result<OperationSuccess, RequestFailure> {
        let effect = if update.changed {
            EffectDisposition::Performed
        } else {
            EffectDisposition::NotPerformed
        };
        let event = self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            CommandPayloadDraft::validated(action, effect, AuditInput::new()),
        )?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&event)),
            result: result(update.status),
        })
    }

    fn monitor_mutation_failure(
        &self,
        links: EventLinksDraft,
        action: EventAction,
        error: RuntimeHostError,
    ) -> Result<RequestFailure, RequestFailure> {
        let event = self.append_event(
            EventSeverity::Error,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            CommandPayloadDraft::rejected(
                action,
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::Indeterminate,
                AuditInput::new(),
            ),
        )?;
        Ok(RequestFailure::poison(error, Some(terminal(&event))))
    }

    fn run_monitor_probe(&self, probe: &DueMonitorProbe) -> RuntimeHostResult<()> {
        let started_at_unix_ms = unix_ms_now()?;
        let instance = self.monitor_instance(&probe.instance_alias)?;
        let issued = self
            .events
            .issuer()
            .issue_monitor_probe(instance.instance_id())
            .map_err(|_| {
                RuntimeHostError::fatal(
                    "monitor_probe_id_issue_failed",
                    "run_monitor_probe",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
        let links = issued.event_links();
        self.append_event_raw(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            MonitorPayloadDraft::requested(AuditInput::new()),
        )?;
        self.append_event_raw(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            MonitorPayloadDraft::started(AuditInput::new()),
        )?;
        self.append_event_raw(
            EventSeverity::Info,
            EventSource::Device,
            OriginModule::Capture,
            EventActor::Runtime,
            links.clone(),
            CapturePayloadDraft::requested(EventAction::CaptureObserve, AuditInput::new()),
        )?;
        self.append_event_raw(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Recognition,
            EventActor::Runtime,
            links.clone(),
            RecognitionPayloadDraft::requested(EventAction::RecognitionObserve, AuditInput::new()),
        )?;

        let frame = match self.execution.capture(&probe.instance_alias) {
            Ok(frame) => frame,
            Err(error) => {
                let error = RuntimeHostError::execution("run_monitor_capture", &error);
                return self.finish_monitor_failure(probe, &links, started_at_unix_ms, error, true);
            }
        };
        let artifact_png = match frame.png_for_artifact() {
            Ok(png) => png,
            Err(_) => {
                let error = RuntimeHostError::request(
                    "capture_frame_invalid",
                    "run_monitor_capture",
                    RuntimeErrorCode::CaptureFailed,
                );
                return self.finish_monitor_failure(probe, &links, started_at_unix_ms, error, true);
            }
        };
        let write_context =
            ArtifactWriteContext::new(issued.artifact_links(), links.clone(), unix_ms_now()?);
        let mut sink = RuntimeArtifactEventSink {
            ledger: &self.ledger,
            events: &self.events,
        };
        self.artifacts
            .put(
                ArtifactWriteRequest::new(
                    ArtifactKind::CaptureFrame,
                    &artifact_png,
                    write_context,
                    ArtifactIssuePolicy::new(
                        ArtifactProducer::CaptureStore,
                        RetentionClass::Adaptive,
                        ArtifactRedactionState::NotRequired,
                    ),
                ),
                &mut sink,
            )
            .map_err(|_| artifact_store_error("persist_monitor_observation"))?;
        self.append_event_raw(
            EventSeverity::Info,
            EventSource::Device,
            OriginModule::Capture,
            EventActor::Runtime,
            links.clone(),
            CapturePayloadDraft::completed(
                EventAction::CaptureObserve,
                EffectDisposition::Performed,
                frame.width,
                frame.height,
                AuditInput::new(),
            ),
        )?;

        let observation = match self.execution.observe_monitor(
            &probe.instance_alias,
            probe.policy.expected_page(),
            &frame,
        ) {
            Ok(observation) => observation,
            Err(error) => {
                let error = RuntimeHostError::execution("classify_monitor_observation", &error);
                return self.finish_monitor_failure(
                    probe,
                    &links,
                    started_at_unix_ms,
                    error,
                    false,
                );
            }
        };
        let decision =
            decide_monitor(probe.policy.decision_policy(), &observation).map_err(|_| {
                RuntimeHostError::fatal(
                    "monitor_decision_invalid",
                    "run_monitor_probe",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
        self.append_event_raw(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Recognition,
            EventActor::Runtime,
            links.clone(),
            RecognitionPayloadDraft::completed(
                EventAction::RecognitionObserve,
                EffectDisposition::Performed,
                frame.width,
                frame.height,
                RecognitionVerdict::FrameDecoded,
                AuditInput::new(),
            ),
        )?;
        self.append_event_raw(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            MonitorPayloadDraft::completed(
                EffectDisposition::Performed,
                observation,
                decision.clone(),
                AuditInput::new(),
            ),
        )?;
        let completed_at_unix_ms = unix_ms_now()?;
        let current = lock(&self.monitor_registry, "complete_monitor_probe")?.complete_probe(
            probe,
            started_at_unix_ms,
            completed_at_unix_ms,
            decision.clone(),
        )?;
        if current {
            self.record_monitor_recovery_coordination(&instance, &issued, &decision)?;
        }
        Ok(())
    }

    fn record_monitor_recovery_coordination(
        &self,
        instance: &RegisteredInstance,
        issued: &IssuedMonitorProbe,
        decision: &actingcommand_contract::MonitorDecision,
    ) -> RuntimeHostResult<()> {
        let Some(recovery) = decision.recovery() else {
            return Ok(());
        };
        let admission = self.monitor_recovery_admission(instance.instance_id())?;
        let links = admission.lease_id.map_or_else(
            || issued.event_links(),
            |lease_id| issued.event_links_with_lease(lease_id),
        );
        let (severity, payload) = if admission.admitted() {
            (
                EventSeverity::Info,
                MonitorPayloadDraft::recovery_admitted(recovery, AuditInput::new()),
            )
        } else {
            (
                EventSeverity::Warning,
                MonitorPayloadDraft::recovery_deferred(
                    recovery,
                    admission.reason,
                    AuditInput::new(),
                ),
            )
        };
        self.append_event_raw(
            severity,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links,
            payload,
        )?;
        Ok(())
    }

    fn monitor_recovery_admission(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<MonitorRecoveryAdmission> {
        let now = self.monotonic_ms()?;
        let scheduler = lock(&self.scheduler, "coordinate_monitor_recovery")?;
        if let Some(active) = scheduler.active_lease(instance_id) {
            let token = active.token();
            if token.owner_epoch() != self.owner_epoch || token.instance_id() != instance_id {
                return Err(RuntimeHostError::fatal(
                    "monitor_recovery_fencing_state_invalid",
                    "coordinate_monitor_recovery",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            let reason = if token.expires_at_monotonic_ms() <= now {
                MonitorRecoveryCoordinationReason::LeaseExpired
            } else if active.destructive_step_active() {
                MonitorRecoveryCoordinationReason::DestructiveStepActive
            } else if active.preempt_requested() {
                MonitorRecoveryCoordinationReason::PreemptionPending
            } else {
                MonitorRecoveryCoordinationReason::ActiveLease
            };
            return Ok(MonitorRecoveryAdmission {
                reason,
                lease_id: Some(token.lease_id()),
            });
        }
        let reason = if scheduler.cooldown_active(instance_id, now) {
            MonitorRecoveryCoordinationReason::TakeoverCooldown
        } else if scheduler.queued_count(instance_id) > 0 {
            MonitorRecoveryCoordinationReason::QueuedLeaseRequests
        } else {
            MonitorRecoveryCoordinationReason::SchedulerAvailable
        };
        Ok(MonitorRecoveryAdmission {
            reason,
            lease_id: None,
        })
    }

    fn finish_monitor_failure(
        &self,
        probe: &DueMonitorProbe,
        links: &EventLinksDraft,
        started_at_unix_ms: u64,
        error: RuntimeHostError,
        capture_failed: bool,
    ) -> RuntimeHostResult<()> {
        let runtime_code = error.projection().code;
        let diagnostic = if capture_failed {
            DiagnosticCode::CaptureFailed
        } else {
            DiagnosticCode::RecognitionFailed
        };
        if capture_failed {
            self.append_event_raw(
                EventSeverity::Error,
                EventSource::Device,
                OriginModule::Capture,
                EventActor::Runtime,
                links.clone(),
                CapturePayloadDraft::failed(
                    EventAction::CaptureObserve,
                    diagnostic,
                    EffectDisposition::NotPerformed,
                    AuditInput::new(),
                ),
            )?;
        }
        self.append_event_raw(
            EventSeverity::Error,
            EventSource::Runtime,
            OriginModule::Recognition,
            EventActor::Runtime,
            links.clone(),
            RecognitionPayloadDraft::failed(
                EventAction::RecognitionObserve,
                diagnostic,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            ),
        )?;
        self.append_event_raw(
            EventSeverity::Error,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            MonitorPayloadDraft::failed(
                diagnostic,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            ),
        )?;
        let completed_at_unix_ms = unix_ms_now()?;
        lock(&self.monitor_registry, "fail_monitor_probe")?.fail_probe(
            probe,
            started_at_unix_ms,
            completed_at_unix_ms,
            runtime_code,
        )?;
        if error.code() == "monitor_observation_invalid" {
            return Err(error);
        }
        Ok(())
    }

    fn monitor_instance(&self, instance_alias: &str) -> RuntimeHostResult<RegisteredInstance> {
        let registered = lock(&self.registered_instances, "resolve_monitor_instance")?
            .values()
            .find(|instance| instance.instance_alias == instance_alias)
            .cloned()
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    "monitor_instance_unknown",
                    "resolve_monitor_instance",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
        let resolved = self
            .execution
            .resolve(instance_alias)
            .map_err(|error| RuntimeHostError::execution("resolve_monitor_instance", &error))?;
        if resolved.instance_id() != registered.instance_id
            || resolved.audit_endpoint() != registered.audit_endpoint
        {
            return Err(RuntimeHostError::fatal(
                "runtime_instance_identity_mismatch",
                "resolve_monitor_instance",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        Ok(registered)
    }

    fn acquire_lease(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        request_id: RequestId,
        instance_alias: &str,
        holder_id: actingcommand_contract::HolderId,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        let instance_guard = self.instance_guard(resolved.instance_id())?;
        let _admission = lock(&instance_guard, "lock_instance_admission")?;
        self.expire_instance_if_due(resolved.instance_id())?;
        let preparation = {
            let mut scheduler = lock(&self.scheduler, "prepare_lease")?;
            scheduler.prepare_acquire(
                request_id,
                resolved.instance_id(),
                holder_id,
                connection_id,
                self.monotonic_ms()?,
            )
        };
        let preparation = match preparation {
            Ok(preparation) => preparation,
            Err(error) => {
                self.append_lease_requested(request, &resolved)?;
                return Err(self.scheduler_denied(request, &resolved, None, error)?);
            }
        };
        self.grant_prepared_lease(request, request_id, &resolved, preparation)
    }

    fn grant_prepared_lease(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        request_id: RequestId,
        resolved: &RegisteredInstance,
        preparation: LeasePreparation,
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
        let links = self.events.request_links(
            request,
            Some(resolved.instance_id()),
            Some(token.lease_id()),
            Some(action_id),
        );
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

    fn queue_lease(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        holder_id: actingcommand_contract::HolderId,
        policy: LeaseQueuePolicy,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        let instance_guard = self.instance_guard(resolved.instance_id())?;
        let admission = lock(&instance_guard, "lock_instance_admission")?;
        self.expire_instance_if_due(resolved.instance_id())?;
        let outcome = lock(&self.scheduler, "queue_lease")?.request_queued(
            QueueLeaseRequest::new(
                original.request_id(),
                resolved.instance_id(),
                holder_id,
                connection_id,
                policy.priority(),
                policy.timeout_ms(),
            ),
            self.monotonic_ms()?,
        );
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                self.append_lease_requested(request, &resolved)?;
                return Err(self.scheduler_denied(request, &resolved, None, error)?);
            }
        };
        let (decision, expired) = outcome.into_parts();
        self.record_expired_queued(expired)?;
        match decision {
            QueueAdmissionDecision::Lease(preparation) if preparation.is_existing() => {
                let token = preparation.token().clone();
                let terminal =
                    self.existing_queue_grant_terminal(original.request_id(), token.lease_id())?;
                Ok(OperationSuccess {
                    state: RuntimeReceiptState::Admitted,
                    terminal: Some(terminal),
                    result: RuntimeResult::LeaseGranted { token },
                })
            }
            QueueAdmissionDecision::Lease(preparation) => {
                self.grant_prepared_lease(request, original.request_id(), &resolved, preparation)
            }
            QueueAdmissionDecision::Queued(queued) => {
                if let Some(existing) = lock(&self.queued_requests, "read_queued_request")?
                    .get(&original.request_id())
                    .cloned()
                {
                    if existing.request != *original
                        || existing.instance != resolved
                        || existing.connection_id != connection_id
                    {
                        return Err(RequestFailure::poison_without_terminal(
                            RuntimeHostError::fatal(
                                "queued_request_identity_mismatch",
                                "queue_lease",
                                RuntimeErrorCode::RuntimeFatal,
                            ),
                        ));
                    }
                    let terminal = self.existing_request_terminal(
                        original.request_id(),
                        EventType::SchedulerQueued,
                    )?;
                    return Ok(OperationSuccess {
                        state: RuntimeReceiptState::Queued,
                        terminal: Some(terminal),
                        result: RuntimeResult::LeaseQueued {
                            status: queued.status().map_err(|error| {
                                RequestFailure::poison_without_terminal(
                                    RuntimeHostError::scheduler("queue_lease_status", &error),
                                )
                            })?,
                        },
                    });
                }
                self.append_lease_requested(request, &resolved)?;
                let mut terminal_event =
                    self.append_scheduler_queued(request, &resolved, &queued)?;
                if queued.preempt_requested() {
                    terminal_event =
                        self.append_scheduler_preempted(request, &resolved, &queued)?;
                }
                let context = QueuedRequestContext {
                    request: original.clone(),
                    instance: resolved,
                    connection_id,
                };
                if lock(&self.queued_requests, "register_queued_request")?
                    .insert(original.request_id(), context)
                    .is_some()
                {
                    return Err(RequestFailure::poison_without_terminal(
                        RuntimeHostError::fatal(
                            "queued_request_collision",
                            "queue_lease",
                            RuntimeErrorCode::RuntimeFatal,
                        ),
                    ));
                }
                if let Some((token, transferred)) =
                    self.promote_idle_preemption(&queued, &admission)?
                {
                    return Ok(OperationSuccess {
                        state: RuntimeReceiptState::Admitted,
                        terminal: Some(terminal(&transferred)),
                        result: RuntimeResult::LeaseGranted { token },
                    });
                }
                Ok(OperationSuccess {
                    state: RuntimeReceiptState::Queued,
                    terminal: Some(terminal(&terminal_event)),
                    result: RuntimeResult::LeaseQueued {
                        status: queued.status().map_err(|error| {
                            RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                                "queue_lease_status",
                                &error,
                            ))
                        })?,
                    },
                })
            }
        }
    }

    fn poll_queued_lease(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        queued_request_id: RequestId,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let context = lock(&self.queued_requests, "read_queued_request")?
            .get(&queued_request_id)
            .cloned();
        if context.is_none()
            && let Some(record) = lock(&self.queue_terminals, "read_queue_terminal")?
                .entries
                .get(&queued_request_id)
                .copied()
        {
            if record.connection_id != connection_id {
                return Err(RequestFailure::request(
                    RuntimeHostError::request(
                        "lease_queue_connection_mismatch",
                        "poll_queued_lease",
                        RuntimeErrorCode::QueueConnectionMismatch,
                    ),
                    RuntimeReceiptState::Denied,
                    None,
                ));
            }
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    "lease_queue_expired",
                    "poll_queued_lease",
                    RuntimeErrorCode::QueueExpired,
                ),
                RuntimeReceiptState::Denied,
                Some(record.terminal),
            ));
        }
        let instance_guard = context
            .as_ref()
            .map(|context| self.instance_guard(context.instance.instance_id()))
            .transpose()?;
        let _admission = instance_guard
            .as_ref()
            .map(|guard| lock(guard, "lock_instance_admission"))
            .transpose()?;
        let poll = lock(&self.scheduler, "poll_queued_lease")?.poll_queued(
            queued_request_id,
            connection_id,
            self.monotonic_ms()?,
        );
        match poll {
            Ok(QueuePoll::Granted(token)) => {
                let terminal =
                    self.existing_queue_grant_terminal(queued_request_id, token.lease_id())?;
                Ok(OperationSuccess {
                    state: RuntimeReceiptState::Admitted,
                    terminal: Some(terminal),
                    result: RuntimeResult::LeaseGranted { token },
                })
            }
            Ok(QueuePoll::Pending(queued)) => Ok(OperationSuccess {
                state: RuntimeReceiptState::Queued,
                terminal: None,
                result: RuntimeResult::LeasePending {
                    status: queued.status().map_err(|error| {
                        RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                            "poll_queued_lease",
                            &error,
                        ))
                    })?,
                },
            }),
            Err(SchedulerError::QueueExpired) => {
                let context = context.ok_or_else(|| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        "expired_queue_context_missing",
                        "poll_queued_lease",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?;
                self.remove_queued_context(queued_request_id, connection_id)?;
                let event =
                    self.append_queue_terminal(&context, DiagnosticCode::LeaseQueueExpired)?;
                self.remember_queue_expiry(&context, &event)?;
                Err(RequestFailure::request(
                    RuntimeHostError::scheduler("poll_queued_lease", &SchedulerError::QueueExpired),
                    RuntimeReceiptState::Denied,
                    Some(terminal(&event)),
                ))
            }
            Err(error) => Err(self.scheduler_denied_error(
                request,
                context.as_ref().map(|value| value.instance.instance_id()),
                None,
                context
                    .as_ref()
                    .map_or("", |value| value.instance.audit_endpoint()),
                RuntimeHostError::scheduler("poll_queued_lease", &error),
            )?),
        }
    }

    fn cancel_queued_lease(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        queued_request_id: RequestId,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let context = lock(&self.queued_requests, "read_queued_request")?
            .get(&queued_request_id)
            .cloned();
        let instance_guard = context
            .as_ref()
            .map(|context| self.instance_guard(context.instance.instance_id()))
            .transpose()?;
        let _admission = instance_guard
            .as_ref()
            .map(|guard| lock(guard, "lock_instance_admission"))
            .transpose()?;
        let cancelled = lock(&self.scheduler, "cancel_queued_lease")?
            .cancel_queued(queued_request_id, connection_id);
        let cancelled = match cancelled {
            Ok(cancelled) => cancelled,
            Err(error) => {
                return Err(self.scheduler_denied_error(
                    request,
                    None,
                    None,
                    "",
                    RuntimeHostError::scheduler("cancel_queued_lease", &error),
                )?);
            }
        };
        let context = self.take_queued_context(&cancelled)?;
        let event = self.append_queue_terminal(&context, DiagnosticCode::LeaseQueueCancelled)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Cancelled,
            terminal: Some(terminal(&event)),
            result: RuntimeResult::LeaseQueueCancelled {
                request_id: queued_request_id,
                instance_id: cancelled.queued().instance_id(),
            },
        })
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

    fn existing_queue_grant_terminal(
        &self,
        request_id: RequestId,
        lease_id: LeaseId,
    ) -> Result<TerminalEvent, RequestFailure> {
        if let Some(terminal) =
            self.query_single_terminal(request_id, Some(lease_id), EventType::LeaseTransferred)?
        {
            return Ok(terminal);
        }
        self.query_single_terminal(request_id, Some(lease_id), EventType::LeaseGranted)?
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "queue_grant_terminal_missing",
                    "recover_queued_lease_grant",
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

    fn append_scheduler_queued(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        resolved: &RegisteredInstance,
        queued: &QueuedLease,
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
            SchedulerPayloadDraft::queued(
                EventAction::ScheduleAdmit,
                queued.priority(),
                queued.position(),
                queued.deadline_monotonic_ms(),
                queued.preempt_requested(),
                audit_endpoint(resolved.audit_endpoint()),
            ),
        )
    }

    fn append_scheduler_preempted(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        resolved: &RegisteredInstance,
        queued: &QueuedLease,
    ) -> Result<PersistedEvent, RequestFailure> {
        let active = lock(&self.scheduler, "read_preemption_state")?
            .active_lease(resolved.instance_id())
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "preemption_active_lease_missing",
                    "record_scheduler_preemption",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })?;
        if !active.preempt_requested() {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "preemption_state_mismatch",
                    "record_scheduler_preemption",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        let links = self.events.request_links(
            request,
            Some(resolved.instance_id()),
            Some(active.token().lease_id()),
            None,
        );
        self.append_event(
            EventSeverity::Warning,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links,
            SchedulerPayloadDraft::preempted(
                EventAction::ScheduleAdmit,
                active.token().holder_id(),
                active.token().lease_id(),
                queued.request_id(),
                queued.priority(),
                active.destructive_step_active(),
                audit_endpoint(resolved.audit_endpoint()),
            ),
        )
    }

    fn record_expired_queued(&self, expired: Vec<QueuedLease>) -> Result<(), RequestFailure> {
        for queued in expired {
            let context = self.take_queued_context_for(&queued)?;
            let event = self.append_queue_terminal(&context, DiagnosticCode::LeaseQueueExpired)?;
            self.remember_queue_expiry(&context, &event)?;
        }
        Ok(())
    }

    fn remember_queue_expiry(
        &self,
        context: &QueuedRequestContext,
        event: &PersistedEvent,
    ) -> Result<(), RequestFailure> {
        lock(&self.queue_terminals, "record_queue_terminal")?.insert(
            context.request.request_id(),
            QueueTerminalRecord {
                connection_id: context.connection_id,
                terminal: terminal(event),
            },
        );
        Ok(())
    }

    fn cancel_instance_queue(
        &self,
        instance_id: InstanceId,
        diagnostic: DiagnosticCode,
    ) -> Result<(), RequestFailure> {
        let removed = lock(&self.scheduler, "cancel_instance_queue")?
            .remove_queued_for_instance(instance_id)
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                    "cancel_instance_queue",
                    &error,
                ))
            })?;
        for cancelled in removed {
            let context = self.take_queued_context(&cancelled)?;
            self.append_queue_terminal(&context, diagnostic)?;
        }
        Ok(())
    }

    fn take_queued_context(
        &self,
        cancelled: &CancelledQueuedLease,
    ) -> Result<QueuedRequestContext, RequestFailure> {
        self.take_queued_context_for(cancelled.queued())
    }

    fn take_queued_context_for(
        &self,
        queued: &QueuedLease,
    ) -> Result<QueuedRequestContext, RequestFailure> {
        let context = self.remove_queued_context(queued.request_id(), queued.connection_id())?;
        if context.instance.instance_id() != queued.instance_id() {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "queued_request_instance_mismatch",
                    "remove_queued_request",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        Ok(context)
    }

    fn remove_queued_context(
        &self,
        request_id: RequestId,
        connection_id: ConnectionId,
    ) -> Result<QueuedRequestContext, RequestFailure> {
        let context = lock(&self.queued_requests, "remove_queued_request")?
            .remove(&request_id)
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "queued_request_context_missing",
                    "remove_queued_request",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })?;
        if context.connection_id != connection_id {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "queued_request_connection_mismatch",
                    "remove_queued_request",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        Ok(context)
    }

    fn append_queue_terminal(
        &self,
        context: &QueuedRequestContext,
        diagnostic: DiagnosticCode,
    ) -> Result<PersistedEvent, RequestFailure> {
        let validated = context.request.validate().map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "queued_request_context_invalid",
                "record_queued_request_terminal",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        let links =
            self.events
                .request_links(&validated, Some(context.instance.instance_id()), None, None);
        self.append_event(
            EventSeverity::Warning,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links,
            SchedulerPayloadDraft::denied(
                EventAction::ScheduleAdmit,
                diagnostic,
                audit_endpoint(context.instance.audit_endpoint()),
            ),
        )
    }

    fn expire_queued_for_instance(&self, instance_id: InstanceId) -> Result<(), RequestFailure> {
        let expired = lock(&self.scheduler, "expire_queued_requests")?
            .take_expired_for_instance(instance_id, self.monotonic_ms()?)
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                    "expire_queued_requests",
                    &error,
                ))
            })?;
        self.record_expired_queued(expired)
    }

    fn perform_transfer(
        &self,
        prepared: Box<PreparedLeaseTransfer>,
    ) -> Result<PersistedEvent, RequestFailure> {
        let from = prepared.from_token().clone();
        let to = prepared.to_token().clone();
        let queued_request_id = prepared.queued_request_id();
        let to_connection_id = prepared.to_connection_id();
        let priority = prepared.priority();
        let action = match prepared.reason() {
            LeaseTransferReason::Preempted => EventAction::LeaseAcquire,
            LeaseTransferReason::Expired => EventAction::LeaseExpire,
            LeaseTransferReason::ExplicitRelease
            | LeaseTransferReason::Disconnect
            | LeaseTransferReason::BackendFailure
            | LeaseTransferReason::HostShutdown => EventAction::LeaseRelease,
        };
        let context = lock(&self.queued_requests, "read_transfer_request")?
            .get(&queued_request_id)
            .cloned()
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "lease_transfer_context_missing",
                    "prepare_lease_transfer",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })?;
        if context.connection_id != to_connection_id
            || context.instance.instance_id() != to.instance_id()
        {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "lease_transfer_context_mismatch",
                    "prepare_lease_transfer",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        let validated = context.request.validate().map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "lease_transfer_request_invalid",
                "prepare_lease_transfer",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        let action_id = self
            .events
            .action_id()
            .map_err(RequestFailure::poison_without_terminal)?;
        let links = self.events.request_links(
            &validated,
            Some(to.instance_id()),
            Some(to.lease_id()),
            Some(action_id),
        );
        self.append_event(
            EventSeverity::Info,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links.clone(),
            LeasePayloadDraft::transition_intent(
                action,
                audit_endpoint(context.instance.audit_endpoint()),
            ),
        )?;
        let transferred = self.append_event(
            EventSeverity::Info,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links,
            LeasePayloadDraft::transferred(
                action,
                EffectDisposition::Performed,
                from.holder_id(),
                from.lease_id(),
                to.holder_id(),
                to.lease_id(),
                queued_request_id,
                priority,
                audit_endpoint(context.instance.audit_endpoint()),
            ),
        )?;
        let committed = lock(&self.scheduler, "commit_lease_transfer")?
            .commit_transfer(prepared, self.monotonic_ms()?);
        let committed = committed.map_err(|_| {
            RequestFailure::poison(
                RuntimeHostError::fatal(
                    "lease_transfer_commit_failed_after_durable_fact",
                    "commit_lease_transfer",
                    RuntimeErrorCode::RuntimeFatal,
                ),
                Some(terminal(&transferred)),
            )
        })?;
        if committed != to {
            return Err(RequestFailure::poison(
                RuntimeHostError::fatal(
                    "lease_transfer_token_mismatch_after_durable_fact",
                    "commit_lease_transfer",
                    RuntimeErrorCode::RuntimeFatal,
                ),
                Some(terminal(&transferred)),
            ));
        }
        self.remove_queued_context(queued_request_id, to_connection_id)?;
        self.persist_active_instances()
            .map_err(RequestFailure::poison_without_terminal)?;
        Ok(transferred)
    }

    /// The caller holds the per-instance admission guard, so no lease mutation can invalidate the
    /// prepared transfer before its durable authorization fact is committed.
    fn promote_idle_preemption(
        &self,
        queued: &QueuedLease,
        _admission: &MutexGuard<'_, ()>,
    ) -> Result<Option<(LeaseToken, PersistedEvent)>, RequestFailure> {
        if !queued.preempt_requested() {
            return Ok(None);
        }
        let transfer = {
            let mut scheduler = lock(&self.scheduler, "prepare_idle_preemption")?;
            let active = scheduler
                .active_lease(queued.instance_id())
                .ok_or_else(|| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        "idle_preemption_active_lease_missing",
                        "prepare_idle_preemption",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?;
            scheduler
                .prepare_transfer(
                    active.token(),
                    active.connection_id(),
                    LeaseTransferReason::Preempted,
                    None,
                    self.monotonic_ms()?,
                )
                .map_err(|error| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                        "prepare_idle_preemption",
                        &error,
                    ))
                })?
        };
        match transfer {
            TransferPreparation::Ready(prepared) => {
                let token = prepared.to_token().clone();
                self.perform_transfer(prepared)
                    .map(|event| Some((token, event)))
            }
            TransferPreparation::Deferred => Ok(None),
            TransferPreparation::NoCandidate => Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "idle_preemption_candidate_missing",
                    "prepare_idle_preemption",
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
                return self.release_via_transfer(request, token, &resolved, prepared);
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
        let links = self.events.request_links(
            request,
            Some(token.instance_id()),
            Some(token.lease_id()),
            Some(action_id),
        );
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

    fn observe_readonly(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        self.append_request_lifecycle(
            original,
            request,
            resolved.instance_id(),
            EventAction::RuntimeReadonlyObserve,
        )?;
        self.append_scheduler_admitted(request, &resolved, None)?;
        let completed =
            self.capture_readonly_observation(request, instance_alias, resolved.instance_id())?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&completed.terminal)),
            result: RuntimeResult::ReadonlyObservationCompleted {
                observation: completed.observation,
            },
        })
    }

    fn capture_sequence(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        spec: CaptureSequenceSpec,
    ) -> Result<OperationSuccess, RequestFailure> {
        spec.validate().map_err(|_| {
            RequestFailure::request(
                RuntimeHostError::request(
                    "capture_sequence_spec_invalid",
                    "capture_sequence",
                    RuntimeErrorCode::InvalidRequest,
                ),
                RuntimeReceiptState::Denied,
                None,
            )
        })?;
        let resolved = self.resolve_instance(instance_alias)?;
        self.append_request_lifecycle(
            original,
            request,
            resolved.instance_id(),
            EventAction::RuntimeCaptureSequence,
        )?;
        self.append_scheduler_admitted(request, &resolved, None)?;
        let mut observations = Vec::with_capacity(usize::from(spec.frame_count()));
        let mut last_terminal = None;
        for index in 0..spec.frame_count() {
            let completed =
                self.capture_readonly_observation(request, instance_alias, resolved.instance_id())?;
            observations.push(completed.observation);
            last_terminal = Some(completed.terminal);
            if index + 1 < spec.frame_count() && spec.interval_ms() > 0 {
                thread::sleep(Duration::from_millis(spec.interval_ms()));
            }
        }
        let sequence = CaptureSequence::new(spec, observations).map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "capture_sequence_result_invalid",
                "capture_sequence",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        let terminal_event = last_terminal.ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "capture_sequence_terminal_missing",
                "capture_sequence",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&terminal_event)),
            result: RuntimeResult::CaptureSequenceCompleted { sequence },
        })
    }

    fn capture_readonly_observation(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        instance_id: InstanceId,
    ) -> Result<CompletedReadonlyObservation, RequestFailure> {
        let capability = self.issue_readonly_capability(instance_id)?;
        let debug_run =
            if request.actor() == EventActor::Lab && request.source() == EventSource::Lab {
                lock(&self.debug_runs, "read_runtime_debug_run")?
                    .get(&request.correlation_id())
                    .map(|context| (context.task_id, context.run_id))
            } else {
                None
            };
        let mut links = capability.event_links(request);
        if let Some((task_id, run_id)) = debug_run {
            links = links.with_task_id(task_id).with_run_id(run_id);
        }
        self.append_event(
            EventSeverity::Info,
            EventSource::Device,
            OriginModule::Capture,
            EventActor::Runtime,
            links.clone(),
            CapturePayloadDraft::requested(EventAction::CaptureObserve, AuditInput::new()),
        )?;
        self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Recognition,
            EventActor::Runtime,
            links.clone(),
            RecognitionPayloadDraft::requested(EventAction::RecognitionObserve, AuditInput::new()),
        )?;
        let frame = match self.execution.capture(instance_alias) {
            Ok(frame) => frame,
            Err(error) => {
                self.append_event(
                    EventSeverity::Error,
                    EventSource::Device,
                    OriginModule::Capture,
                    EventActor::Runtime,
                    links.clone(),
                    CapturePayloadDraft::failed(
                        EventAction::CaptureObserve,
                        DiagnosticCode::CaptureFailed,
                        EffectDisposition::NotPerformed,
                        AuditInput::new(),
                    ),
                )?;
                let event = self.append_event(
                    EventSeverity::Error,
                    EventSource::Runtime,
                    OriginModule::Recognition,
                    EventActor::Runtime,
                    links.clone(),
                    RecognitionPayloadDraft::failed(
                        EventAction::RecognitionObserve,
                        DiagnosticCode::CaptureFailed,
                        EffectDisposition::NotPerformed,
                        AuditInput::new(),
                    ),
                )?;
                return Err(RequestFailure::request(
                    RuntimeHostError::execution("execute_capture_backend", &error),
                    RuntimeReceiptState::Failed,
                    Some(terminal(&event)),
                ));
            }
        };
        let artifact_png = match frame.png_for_artifact() {
            Ok(png) => png,
            Err(_) => {
                self.append_event(
                    EventSeverity::Error,
                    EventSource::Device,
                    OriginModule::Capture,
                    EventActor::Runtime,
                    links.clone(),
                    CapturePayloadDraft::failed(
                        EventAction::CaptureObserve,
                        DiagnosticCode::CaptureFailed,
                        EffectDisposition::Indeterminate,
                        AuditInput::new(),
                    ),
                )?;
                let event = self.append_event(
                    EventSeverity::Error,
                    EventSource::Runtime,
                    OriginModule::Recognition,
                    EventActor::Runtime,
                    links.clone(),
                    RecognitionPayloadDraft::failed(
                        EventAction::RecognitionObserve,
                        DiagnosticCode::CaptureFailed,
                        EffectDisposition::NotPerformed,
                        AuditInput::new(),
                    ),
                )?;
                return Err(RequestFailure::request(
                    RuntimeHostError::request(
                        "capture_frame_invalid",
                        "observe_readonly",
                        RuntimeErrorCode::CaptureFailed,
                    ),
                    RuntimeReceiptState::Failed,
                    Some(terminal(&event)),
                ));
            }
        };
        let mut artifact_links = capability.artifact_links(request);
        if let Some((_, run_id)) = debug_run {
            artifact_links = artifact_links.with_run_id(run_id);
        }
        let write_context = ArtifactWriteContext::new(
            artifact_links,
            links.clone(),
            unix_ms_now().map_err(RequestFailure::poison_without_terminal)?,
        );
        let mut sink = RuntimeArtifactEventSink {
            ledger: &self.ledger,
            events: &self.events,
        };
        let stored = self
            .artifacts
            .put(
                ArtifactWriteRequest::new(
                    ArtifactKind::CaptureFrame,
                    &artifact_png,
                    write_context,
                    ArtifactIssuePolicy::new(
                        ArtifactProducer::CaptureStore,
                        if request.actor() == EventActor::Lab
                            && request.source() == EventSource::Lab
                        {
                            RetentionClass::DebugFull
                        } else {
                            RetentionClass::Adaptive
                        },
                        ArtifactRedactionState::NotRequired,
                    ),
                ),
                &mut sink,
            )
            .map_err(|_| {
                RequestFailure::poison_without_terminal(artifact_store_error(
                    "persist_readonly_observation",
                ))
            })?;
        let observation = ReadonlyObservation::new(
            frame.width,
            frame.height,
            RecognitionVerdict::FrameDecoded,
            runtime_capture_backend(frame.backend_name),
            stored.reference().project(true),
        )
        .map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "readonly_observation_invalid",
                "observe_readonly",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        self.append_capture_completed(links.clone(), observation.width(), observation.height())?;
        let event = self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Recognition,
            EventActor::Runtime,
            links,
            RecognitionPayloadDraft::completed(
                EventAction::RecognitionObserve,
                EffectDisposition::Performed,
                observation.width(),
                observation.height(),
                observation.verdict(),
                AuditInput::new(),
            ),
        )?;
        Ok(CompletedReadonlyObservation {
            observation,
            terminal: event,
        })
    }

    fn recover_safe_reset(
        &self,
        request: &RuntimeRequest,
        instance_id: InstanceId,
    ) -> Result<Option<OperationSuccess>, RequestFailure> {
        let events = self
            .ledger
            .query(EventQuery {
                request_id: Some(request.request_id()),
                ..EventQuery::default()
            })
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error("recover_safe_reset"))
            })?;
        if events.is_empty() {
            return Ok(None);
        }
        if events.iter().any(|event| {
            event.links().correlation_id() != Some(&request.correlation_id())
                || event
                    .links()
                    .instance_id()
                    .is_some_and(|actual| actual != &instance_id)
        }) {
            return Err(safe_reset_replay_denied(
                "safe_reset_request_identity_reused",
            ));
        }
        let c4_lifecycle = events.iter().any(|event| {
            matches!(
                event.event_type(),
                EventType::CliCommand | EventType::LabRequest
            )
        }) && events
            .iter()
            .any(|event| event.event_type() == EventType::CommandValidated);
        if !c4_lifecycle {
            return Err(safe_reset_replay_denied("safe_reset_request_id_reused"));
        }
        let committed = events
            .iter()
            .filter(|event| {
                event.event_type() == EventType::InputCommitted
                    && matches!(
                        event.payload(),
                        EventPayload::Input(InputPayload::Committed(detail))
                            if detail.action() == EventAction::InputReset
                    )
            })
            .collect::<Vec<_>>();
        let released = events
            .iter()
            .filter(|event| event.event_type() == EventType::LeaseReleased)
            .collect::<Vec<_>>();
        match (committed.as_slice(), released.as_slice()) {
            ([input], [release]) if input.sequence() < release.sequence() => {
                let action_id = input.links().action_id().copied().ok_or_else(|| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        "safe_reset_action_id_missing",
                        "recover_safe_reset",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?;
                Ok(Some(OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: Some(terminal(release)),
                    result: RuntimeResult::SafeResetCompleted { action_id },
                }))
            }
            ([], []) => Err(safe_reset_replay_denied(
                "safe_reset_previous_attempt_incomplete",
            )),
            _ => Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "safe_reset_durable_state_inconsistent",
                    "recover_safe_reset",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            )),
        }
    }

    fn safe_reset(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        holder_id: actingcommand_contract::HolderId,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        if let Some(recovered) = self.recover_safe_reset(original, resolved.instance_id())? {
            return Ok(recovered);
        }
        self.append_request_lifecycle(
            original,
            request,
            resolved.instance_id(),
            EventAction::InputReset,
        )?;
        let acquired = self.acquire_lease(
            request,
            original.request_id(),
            instance_alias,
            holder_id,
            connection_id,
        )?;
        let RuntimeResult::LeaseGranted { token } = acquired.result else {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "safe_reset_lease_result_invalid",
                    "execute_safe_reset",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        };
        let action = match self.input(request, &token, &InputAction::Reset, connection_id) {
            Ok(success) => success,
            Err(failure) => {
                return Err(self.cleanup_composite_failure(token, connection_id, failure));
            }
        };
        let RuntimeResult::InputCommitted { action_id } = action.result else {
            return Err(self.cleanup_composite_failure(
                token,
                connection_id,
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "safe_reset_input_result_invalid",
                    "execute_safe_reset",
                    RuntimeErrorCode::RuntimeFatal,
                )),
            ));
        };
        let released =
            match self.release_lease(request, original.request_id(), &token, connection_id) {
                Ok(success) => success,
                Err(failure) => {
                    return Err(self.cleanup_composite_failure(token, connection_id, failure));
                }
            };
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: released.terminal,
            result: RuntimeResult::SafeResetCompleted { action_id },
        })
    }

    fn recover_application_lifecycle(
        &self,
        request: &RuntimeRequest,
        instance_id: InstanceId,
        action: ApplicationLifecycleAction,
    ) -> Result<Option<OperationSuccess>, RequestFailure> {
        let events = self
            .ledger
            .query(EventQuery {
                request_id: Some(request.request_id()),
                ..EventQuery::default()
            })
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error(
                    "recover_application_lifecycle",
                ))
            })?;
        if events.is_empty() {
            return Ok(None);
        }
        if events.iter().any(|event| {
            event.links().correlation_id() != Some(&request.correlation_id())
                || event
                    .links()
                    .instance_id()
                    .is_some_and(|actual| actual != &instance_id)
        }) {
            return Err(application_replay_denied(
                "application_lifecycle_request_identity_reused",
            ));
        }
        let expected_action = action.event_action();
        let completed = events
            .iter()
            .filter(|event| {
                matches!(
                    event.payload(),
                    EventPayload::Application(ApplicationPayload::Completed(detail))
                        if detail.action() == expected_action
                )
            })
            .collect::<Vec<_>>();
        let released = events
            .iter()
            .filter(|event| event.event_type() == EventType::LeaseReleased)
            .collect::<Vec<_>>();
        match (completed.as_slice(), released.as_slice()) {
            ([application], [release]) if application.sequence() < release.sequence() => {
                let action_id = application.links().action_id().copied().ok_or_else(|| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        "application_lifecycle_action_id_missing",
                        "recover_application_lifecycle",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?;
                Ok(Some(OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: Some(terminal(release)),
                    result: RuntimeResult::ApplicationLifecycleCompleted { action_id, action },
                }))
            }
            ([], []) => Err(application_replay_denied(
                "application_lifecycle_previous_attempt_incomplete",
            )),
            _ => Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "application_lifecycle_durable_state_inconsistent",
                    "recover_application_lifecycle",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            )),
        }
    }

    fn application_lifecycle(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        holder_id: actingcommand_contract::HolderId,
        action: ApplicationLifecycleAction,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        if let Some(recovered) =
            self.recover_application_lifecycle(original, resolved.instance_id(), action)?
        {
            return Ok(recovered);
        }
        self.append_request_lifecycle(
            original,
            request,
            resolved.instance_id(),
            action.event_action(),
        )?;
        let acquired = self.acquire_lease(
            request,
            original.request_id(),
            instance_alias,
            holder_id,
            connection_id,
        )?;
        let RuntimeResult::LeaseGranted { token } = acquired.result else {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "application_lifecycle_lease_result_invalid",
                    "execute_application_lifecycle",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        };
        let executed = match self.application_control(request, &token, action, connection_id) {
            Ok(success) => success,
            Err(failure) => {
                return Err(self.cleanup_composite_failure(token, connection_id, failure));
            }
        };
        let RuntimeResult::ApplicationLifecycleCompleted { action_id, .. } = executed.result else {
            return Err(self.cleanup_composite_failure(
                token,
                connection_id,
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "application_lifecycle_result_invalid",
                    "execute_application_lifecycle",
                    RuntimeErrorCode::RuntimeFatal,
                )),
            ));
        };
        let released =
            match self.release_lease(request, original.request_id(), &token, connection_id) {
                Ok(success) => success,
                Err(failure) => {
                    return Err(self.cleanup_composite_failure(token, connection_id, failure));
                }
            };
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: released.terminal,
            result: RuntimeResult::ApplicationLifecycleCompleted { action_id, action },
        })
    }

    fn run_contained_task(
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
            self.recover_contained_task(original, resolved.instance_id(), task_request)?
        {
            return Ok(recovered);
        }
        let _active_run = self.begin_contained_run(original.request_id())?;
        let prepared = prepare_contained_task(instance_alias, task_request)?;
        self.append_request_lifecycle(
            original,
            request,
            resolved.instance_id(),
            EventAction::RuntimeTaskRun,
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
        let acquired = self.acquire_lease(
            request,
            original.request_id(),
            instance_alias,
            holder_id,
            connection_id,
        )?;
        let RuntimeResult::LeaseGranted { token } = acquired.result else {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "contained_task_lease_result_invalid",
                    "run_contained_task",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        };
        let mut runtime = RuntimeContainedTask {
            host: self,
            request,
            token: &token,
            instance_alias,
            connection_id,
            task_id,
            run_id,
            last_frame_id: None,
            current_recognition_id: None,
            step_actions: BTreeMap::new(),
            finalizing: None,
        };
        let execution = prepared.run(&mut runtime);
        let finalizing = runtime.finalizing;
        drop(runtime);
        let outcome = match execution {
            Ok(outcome) => outcome,
            Err(ContainedTaskRunError::Boundary(mut failure)) => {
                if !failure.poison_runtime {
                    let event = self.append_contained_task_terminal(
                        request,
                        &token,
                        ContainedTaskTerminalDraft {
                            task_id,
                            run_id,
                            outcome: TaskOutcome::Failure,
                            intent_already_recorded: finalizing.is_some(),
                            final_page: None,
                            executed_steps: 0,
                            failure_code: Some(failure.error.code()),
                        },
                    )?;
                    failure.terminal = Some(terminal(&event));
                }
                return Err(self.cleanup_composite_failure(token, connection_id, failure));
            }
            Err(ContainedTaskRunError::Task(error)) => {
                let event = self.append_contained_task_terminal(
                    request,
                    &token,
                    ContainedTaskTerminalDraft {
                        task_id,
                        run_id,
                        outcome: TaskOutcome::Failure,
                        intent_already_recorded: finalizing.is_some(),
                        final_page: None,
                        executed_steps: 0,
                        failure_code: Some(error.code()),
                    },
                )?;
                let failure = RequestFailure::request(
                    RuntimeHostError::request(
                        error.code(),
                        "run_contained_task",
                        RuntimeErrorCode::BackendOperationFailed,
                    ),
                    RuntimeReceiptState::Failed,
                    Some(terminal(&event)),
                );
                return Err(self.cleanup_composite_failure(token, connection_id, failure));
            }
        };
        if finalizing != Some(outcome.outcome) {
            return Err(self.cleanup_composite_failure(
                token,
                connection_id,
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "contained_task_finalizing_state_invalid",
                    "run_contained_task",
                    RuntimeErrorCode::RuntimeFatal,
                )),
            ));
        }
        let task_terminal = self.append_contained_task_terminal(
            request,
            &token,
            ContainedTaskTerminalDraft {
                task_id,
                run_id,
                outcome: outcome.outcome,
                intent_already_recorded: true,
                final_page: outcome.final_page.clone(),
                executed_steps: outcome.executed_steps,
                failure_code: None,
            },
        )?;
        match self.release_lease(request, original.request_id(), &token, connection_id) {
            Ok(_) => Ok(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: Some(terminal(&task_terminal)),
                result: RuntimeResult::ContainedTaskCompleted {
                    run_id: *run_id.transport(),
                    task_id: *task_id.transport(),
                    outcome: outcome.outcome,
                    final_page: outcome.final_page,
                    executed_steps: outcome.executed_steps,
                },
            }),
            Err(failure) => Err(self.cleanup_composite_failure(token, connection_id, failure)),
        }
    }

    fn recover_contained_task(
        &self,
        request: &RuntimeRequest,
        instance_id: InstanceId,
        task_request: &ContainedTaskRequest,
    ) -> Result<Option<OperationSuccess>, RequestFailure> {
        let active = lock(&self.contained_runs, "read_active_contained_runs")?
            .contains(&request.request_id());
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
            .filter_map(|(_, fact)| match fact {
                TaskSemanticFact::PackageAdmitted { package_sha256, .. } => {
                    Some(package_sha256.as_str())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if packages.len() != 1 || packages[0] != task_request.expected_sha256() {
            return Err(contained_task_replay_denied(
                "contained_task_request_package_reused",
            ));
        }
        let terminals = semantic
            .iter()
            .filter_map(|(event, fact)| match fact {
                TaskSemanticFact::TerminalCommitted {
                    outcome,
                    final_page,
                    executed_steps,
                    failure_code,
                } => Some((
                    *event,
                    *outcome,
                    final_page.clone(),
                    *executed_steps,
                    failure_code.as_deref(),
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        let [(terminal_event, outcome, final_page, executed_steps, _failure_code)] =
            terminals.as_slice()
        else {
            return if terminals.is_empty() {
                Err(contained_task_replay_denied(if active {
                    "contained_task_already_running"
                } else {
                    "contained_task_previous_attempt_incomplete"
                }))
            } else {
                Err(RequestFailure::poison_without_terminal(
                    RuntimeHostError::fatal(
                        "contained_task_terminal_state_inconsistent",
                        "recover_contained_task",
                        RuntimeErrorCode::RuntimeFatal,
                    ),
                ))
            };
        };
        let task_id = terminal_event.links().task_id().copied().ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "contained_task_identity_missing",
                "recover_contained_task",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        let run_id = terminal_event.links().run_id().copied().ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "contained_task_identity_missing",
                "recover_contained_task",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        match outcome {
            TaskOutcome::Success => Ok(Some(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: Some(terminal(terminal_event)),
                result: RuntimeResult::ContainedTaskCompleted {
                    run_id,
                    task_id,
                    outcome: *outcome,
                    final_page: final_page.clone(),
                    executed_steps: *executed_steps,
                },
            })),
            TaskOutcome::Failure | TaskOutcome::Cancelled => Err(RequestFailure::request(
                RuntimeHostError::request(
                    "contained_task_recovered_terminal_failure",
                    "recover_contained_task",
                    RuntimeErrorCode::BackendOperationFailed,
                ),
                match outcome {
                    TaskOutcome::Failure => RuntimeReceiptState::Failed,
                    TaskOutcome::Cancelled => RuntimeReceiptState::Cancelled,
                    TaskOutcome::Success => unreachable!(),
                },
                Some(terminal(terminal_event)),
            )),
        }
    }

    fn begin_contained_run(
        &self,
        request_id: RequestId,
    ) -> Result<ActiveContainedRun<'_>, RequestFailure> {
        let mut active = lock(&self.contained_runs, "begin_contained_run")?;
        if !active.insert(request_id) {
            return Err(contained_task_replay_denied(
                "contained_task_already_running",
            ));
        }
        drop(active);
        Ok(ActiveContainedRun {
            active: &self.contained_runs,
            request_id,
        })
    }

    fn append_contained_task_terminal(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        draft: ContainedTaskTerminalDraft,
    ) -> Result<PersistedEvent, RequestFailure> {
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
        let terminals = self
            .ledger
            .query(EventQuery {
                task_id: Some(*draft.task_id.transport()),
                run_id: Some(*draft.run_id.transport()),
                ..EventQuery::default()
            })
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error(
                    "check_contained_task_terminal",
                ))
            })?
            .into_iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                    TaskSemanticFact::TerminalCommitted { outcome, .. } => Some(*outcome),
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>();
        if let [committed_outcome] = terminals.as_slice() {
            let rejected = self.append_event(
                EventSeverity::Error,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                links,
                TaskPayloadDraft::semantic(
                    TaskSemanticFact::TerminalRejected {
                        committed_outcome: *committed_outcome,
                        attempted_outcome: draft.outcome,
                        reason: "terminal_already_committed".to_string(),
                    },
                    AuditInput::new(),
                ),
            )?;
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
        if !draft.intent_already_recorded {
            self.append_event(
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
            )?;
        }
        self.append_event(
            match draft.outcome {
                TaskOutcome::Success => EventSeverity::Info,
                TaskOutcome::Failure => EventSeverity::Error,
                TaskOutcome::Cancelled => EventSeverity::Warning,
            },
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
                },
                AuditInput::new(),
            ),
        )
    }

    fn issue_readonly_capability(
        &self,
        instance_id: InstanceId,
    ) -> Result<IssuedReadOnlyCaptureCapability, RequestFailure> {
        self.events
            .issuer()
            .issue_readonly_capture_capability(self.owner_epoch, instance_id)
            .map_err(|_| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "readonly_capability_issue_failed",
                    "issue_readonly_capability",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })
    }

    fn append_request_lifecycle(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_id: InstanceId,
        action: EventAction,
    ) -> Result<(), RequestFailure> {
        let links = self.append_client_command_intent(original, request, instance_id, action)?;
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

    fn append_client_command_intent(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_id: InstanceId,
        action: EventAction,
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
        let links = self
            .events
            .request_links(request, Some(instance_id), None, None);
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

    fn append_capture_completed(
        &self,
        links: EventLinksDraft,
        width: u32,
        height: u32,
    ) -> Result<PersistedEvent, RequestFailure> {
        self.append_event(
            EventSeverity::Info,
            EventSource::Device,
            OriginModule::Capture,
            EventActor::Runtime,
            links,
            CapturePayloadDraft::completed(
                EventAction::CaptureObserve,
                EffectDisposition::Performed,
                width,
                height,
                AuditInput::new(),
            ),
        )
    }

    fn cleanup_composite_failure(
        &self,
        token: LeaseToken,
        connection_id: ConnectionId,
        failure: RequestFailure,
    ) -> RequestFailure {
        match self.cleanup_token(&token, connection_id, LeaseReleaseReason::BackendFailure) {
            Ok(()) => failure,
            Err(error) => RequestFailure::poison(error, failure.terminal),
        }
    }

    fn input(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        action: &InputAction,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let (resolved, transferred) = {
            let instance_guard = self.instance_guard(token.instance_id())?;
            let admission = lock(&instance_guard, "lock_instance_admission")?;
            let resolved = self.validated_instance(request, token, connection_id)?;
            let transferred =
                self.transfer_preempted_while_guarded(token, connection_id, &admission)?;
            (resolved, transferred)
        };
        if transferred {
            return Err(self.scheduler_denied_error(
                request,
                Some(token.instance_id()),
                Some(token.lease_id()),
                resolved.audit_endpoint(),
                RuntimeHostError::scheduler(
                    "input_preempted_at_safe_boundary",
                    &SchedulerError::TransferNotSafe,
                ),
            )?);
        }
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
        let event_action = action.event_action();
        let intent = self
            .events
            .draft(
                EventSeverity::Info,
                EventSource::Device,
                OriginModule::DeviceProxy,
                EventActor::Runtime,
                links.clone(),
                InputPayloadDraft::intent(event_action, audit_endpoint(resolved.audit_endpoint())),
            )
            .and_then(|draft| self.events.sanitize(draft))
            .map_err(RequestFailure::poison_without_terminal)?;
        let plan = CriticalEventPlan::new(CriticalOperation::DeviceWrite, intent)
            .map_err(|_| RequestFailure::poison_without_terminal(critical_plan_error()))?;
        let endpoint = resolved.audit_endpoint.clone();
        let instance_alias = resolved.instance_alias.clone();
        let outcome_links = links.clone();
        let failure_links = links;
        let action_for_worker = action.clone();
        let result = execute_critical(
            &self.ledger,
            self.events.fingerprinter(),
            plan,
            || {
                let destructive =
                    lock(&self.scheduler, "begin_destructive_input").and_then(|mut scheduler| {
                        scheduler
                            .begin_destructive_step(token, connection_id, self.monotonic_ms()?)
                            .map_err(|error| {
                                RuntimeHostError::scheduler("begin_destructive_input", &error)
                            })
                    });
                if let Err(error) = destructive {
                    return CriticalActionReport::Failed {
                        error: ActionFailure::scheduler(error),
                        effect: EffectDisposition::NotPerformed,
                    };
                }
                match self.execution.input(&instance_alias, action_for_worker) {
                    Ok(()) => CriticalActionReport::Succeeded {
                        value: (),
                        effect: DefiniteEffectDisposition::Performed,
                    },
                    Err(error) => CriticalActionReport::Failed {
                        error: ActionFailure::backend(RuntimeHostError::execution(
                            "execute_input_backend",
                            &error,
                        )),
                        effect: EffectDisposition::Indeterminate,
                    },
                }
            },
            |_, effect| {
                self.events
                    .draft(
                        EventSeverity::Info,
                        EventSource::Device,
                        OriginModule::DeviceProxy,
                        EventActor::Runtime,
                        outcome_links,
                        InputPayloadDraft::committed(
                            event_action,
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
                        EventSource::Device,
                        OriginModule::DeviceProxy,
                        EventActor::Runtime,
                        failure_links,
                        InputPayloadDraft::failed(
                            event_action,
                            error.diagnostic,
                            effect,
                            audit_endpoint(&endpoint),
                        ),
                    )
                    .map_err(|_| actingcommand_contract::SanitizationError::fingerprinter_failure())
            },
        );
        match result {
            Ok(receipt) => {
                self.finish_destructive_input(token, connection_id)?;
                self.transfer_preempted_if_ready(token, connection_id)?;
                Ok(OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: Some(terminal(receipt.outcome())),
                    result: RuntimeResult::InputCommitted { action_id },
                })
            }
            Err(CriticalExecutionError::Action { error, outcome, .. }) => {
                if error.destructive_started {
                    self.finish_destructive_input(token, connection_id)?;
                }
                if error.transfer_after {
                    self.transfer_preempted_if_ready(token, connection_id)?;
                }
                let release_after = error.release_after;
                let failure = RequestFailure {
                    state: RuntimeReceiptState::Failed,
                    terminal: Some(terminal(&outcome)),
                    error: Box::new(error.error),
                    poison_runtime: error.poison_runtime,
                };
                if release_after {
                    self.cleanup_token(token, connection_id, LeaseReleaseReason::BackendFailure)
                        .map_err(RequestFailure::poison_without_terminal)?;
                }
                Err(failure)
            }
            Err(error) => Err(RequestFailure::poison_without_terminal(
                critical_execution_error(&error),
            )),
        }
    }

    fn application_control(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        action: ApplicationLifecycleAction,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let (resolved, transferred) = {
            let instance_guard = self.instance_guard(token.instance_id())?;
            let admission = lock(&instance_guard, "lock_instance_admission")?;
            let resolved = self.validated_instance(request, token, connection_id)?;
            let transferred =
                self.transfer_preempted_while_guarded(token, connection_id, &admission)?;
            (resolved, transferred)
        };
        if transferred {
            return Err(self.scheduler_denied_error(
                request,
                Some(token.instance_id()),
                Some(token.lease_id()),
                resolved.audit_endpoint(),
                RuntimeHostError::scheduler(
                    "application_lifecycle_preempted_at_safe_boundary",
                    &SchedulerError::TransferNotSafe,
                ),
            )?);
        }
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
        let event_action = action.event_action();
        let intent = self
            .events
            .draft(
                EventSeverity::Info,
                EventSource::Device,
                OriginModule::DeviceProxy,
                EventActor::Runtime,
                links.clone(),
                ApplicationPayloadDraft::intent(
                    event_action,
                    audit_endpoint(resolved.audit_endpoint()),
                ),
            )
            .and_then(|draft| self.events.sanitize(draft))
            .map_err(RequestFailure::poison_without_terminal)?;
        let plan = CriticalEventPlan::new(CriticalOperation::ApplicationLifecycle, intent)
            .map_err(|_| RequestFailure::poison_without_terminal(critical_plan_error()))?;
        let endpoint = resolved.audit_endpoint.clone();
        let instance_alias = resolved.instance_alias.clone();
        let outcome_links = links.clone();
        let failure_links = links;
        let result = execute_critical(
            &self.ledger,
            self.events.fingerprinter(),
            plan,
            || {
                let destructive = lock(&self.scheduler, "begin_destructive_application").and_then(
                    |mut scheduler| {
                        scheduler
                            .begin_destructive_step(token, connection_id, self.monotonic_ms()?)
                            .map_err(|error| {
                                RuntimeHostError::scheduler("begin_destructive_application", &error)
                            })
                    },
                );
                if let Err(error) = destructive {
                    return CriticalActionReport::Failed {
                        error: ActionFailure::scheduler(error),
                        effect: EffectDisposition::NotPerformed,
                    };
                }
                match self.execution.control_application(&instance_alias, action) {
                    Ok(()) => CriticalActionReport::Succeeded {
                        value: (),
                        effect: DefiniteEffectDisposition::Performed,
                    },
                    Err(error) => CriticalActionReport::Failed {
                        error: ActionFailure::backend(RuntimeHostError::execution(
                            "execute_application_backend",
                            &error,
                        )),
                        effect: EffectDisposition::Indeterminate,
                    },
                }
            },
            |_, effect| {
                self.events
                    .draft(
                        EventSeverity::Info,
                        EventSource::Device,
                        OriginModule::DeviceProxy,
                        EventActor::Runtime,
                        outcome_links,
                        ApplicationPayloadDraft::completed(
                            event_action,
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
                        EventSource::Device,
                        OriginModule::DeviceProxy,
                        EventActor::Runtime,
                        failure_links,
                        ApplicationPayloadDraft::failed(
                            event_action,
                            error.diagnostic,
                            effect,
                            audit_endpoint(&endpoint),
                        ),
                    )
                    .map_err(|_| actingcommand_contract::SanitizationError::fingerprinter_failure())
            },
        );
        match result {
            Ok(receipt) => {
                self.finish_destructive_input(token, connection_id)?;
                self.transfer_preempted_if_ready(token, connection_id)?;
                Ok(OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: Some(terminal(receipt.outcome())),
                    result: RuntimeResult::ApplicationLifecycleCompleted { action_id, action },
                })
            }
            Err(CriticalExecutionError::Action { error, outcome, .. }) => {
                if error.destructive_started {
                    self.finish_destructive_input(token, connection_id)?;
                }
                if error.transfer_after {
                    self.transfer_preempted_if_ready(token, connection_id)?;
                }
                let release_after = error.release_after;
                let failure = RequestFailure {
                    state: RuntimeReceiptState::Failed,
                    terminal: Some(terminal(&outcome)),
                    error: Box::new(error.error),
                    poison_runtime: error.poison_runtime,
                };
                if release_after {
                    self.cleanup_token(token, connection_id, LeaseReleaseReason::BackendFailure)
                        .map_err(RequestFailure::poison_without_terminal)?;
                }
                Err(failure)
            }
            Err(error) => Err(RequestFailure::poison_without_terminal(
                critical_execution_error(&error),
            )),
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
            TransferPreparation::Ready(prepared) => self.perform_transfer(prepared).map(|_| true),
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
    ) -> Result<OperationSuccess, RequestFailure> {
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
        let _admission = lock(&instance_guard, "lock_instance_admission")?;
        self.expire_queued_for_instance(token.instance_id())
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
                    self.cleanup_via_transfer(token, &resolved, reason, prepared)?;
                    return Ok(());
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
        let links = self.events.synthetic_links(token, action_id)?;
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
            Err(CriticalExecutionError::Action { error, .. }) => {
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

    fn expire_due_leases(&self) -> RuntimeHostResult<()> {
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
        }
        Ok(())
    }

    fn expire_all_queued_runtime(&self) -> RuntimeHostResult<()> {
        let instance_ids = lock(&self.registered_instances, "read_instance_registry")?
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for instance_id in instance_ids {
            let instance_guard = self
                .instance_guard(instance_id)
                .map_err(|failure| *failure.error)?;
            let _admission = lock(&instance_guard, "lock_instance_admission")?;
            let expired = lock(&self.scheduler, "expire_queued_requests")?
                .take_expired_for_instance(instance_id, self.monotonic_ms()?)
                .map_err(|error| RuntimeHostError::scheduler("expire_queued_requests", &error))?;
            self.record_expired_queued(expired)
                .map_err(|failure| *failure.error)?;
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
            record_failure(
                &mut failure,
                self.cleanup_token(&token, connection_id, reason),
            );
        }
        failure.map_or(Ok(()), Err)
    }

    fn close(self) -> RuntimeHostResult<()> {
        let tokens = lock(&self.scheduler, "list_runtime_leases")?.active_tokens();
        let mut failure = None;
        for token in tokens {
            let connection_id =
                lock(&self.scheduler, "read_lease_connection").and_then(|scheduler| {
                    scheduler.connection_for_token(&token).map_err(|error| {
                        RuntimeHostError::scheduler("read_lease_connection", &error)
                    })
                });
            match connection_id {
                Ok(connection_id) => record_failure(
                    &mut failure,
                    self.cleanup_token(&token, connection_id, LeaseReleaseReason::HostShutdown),
                ),
                Err(error) => record_failure(&mut failure, Err(error)),
            }
        }
        record_failure(
            &mut failure,
            self.execution
                .close()
                .map_err(|error| RuntimeHostError::execution("close_execution_kernel", &error)),
        );
        let HostShared {
            owner,
            ledger,
            fatal,
            ..
        } = self;
        if let Some(error) = fatal.current()? {
            record_failure(&mut failure, Err(error));
        }
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
        let resolved = self.execution.resolve(instance_alias).map_err(|error| {
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
        {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "runtime_instance_identity_mismatch",
                    "resolve_runtime_instance",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        Ok(registered)
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
        })
    }

    fn append_event(
        &self,
        severity: EventSeverity,
        source: EventSource,
        module: OriginModule,
        actor: EventActor,
        links: EventLinksDraft,
        payload: impl Into<actingcommand_contract::EventPayloadDraft>,
    ) -> Result<PersistedEvent, RequestFailure> {
        self.append_event_raw(severity, source, module, actor, links, payload)
            .map_err(RequestFailure::poison_without_terminal)
    }

    fn require_agent_dispatcher(&self, operation: &'static str) -> Result<(), RequestFailure> {
        self.agent_dispatcher_config
            .as_ref()
            .map(|_| ())
            .ok_or_else(|| {
                agent_request_failure(RuntimeHostError::request(
                    "agent_dispatcher_disabled",
                    operation,
                    RuntimeErrorCode::InvalidRequest,
                ))
            })
    }

    fn append_event_raw(
        &self,
        severity: EventSeverity,
        source: EventSource,
        module: OriginModule,
        actor: EventActor,
        links: EventLinksDraft,
        payload: impl Into<actingcommand_contract::EventPayloadDraft>,
    ) -> RuntimeHostResult<PersistedEvent> {
        let gate = lock(&self.fact_write_gate, "append_runtime_event")?;
        let event =
            self.append_event_under_fact_gate(severity, source, module, actor, links, payload)?;
        self.synchronize_fact_store_under_gate()?;
        drop(gate);
        self.observe_pipeline_event(&event)?;
        Ok(event)
    }

    fn append_event_under_fact_gate(
        &self,
        severity: EventSeverity,
        source: EventSource,
        module: OriginModule,
        actor: EventActor,
        links: EventLinksDraft,
        payload: impl Into<actingcommand_contract::EventPayloadDraft>,
    ) -> RuntimeHostResult<PersistedEvent> {
        let draft = self
            .events
            .draft(severity, source, module, actor, links, payload)?;
        let draft = self.events.sanitize(draft)?;
        let event = self
            .ledger
            .append(draft)
            .map_err(|_| ledger_error("append_runtime_event"))?;
        Ok(event)
    }

    fn synchronize_fact_store(&self) -> RuntimeHostResult<()> {
        let _gate = lock(&self.fact_write_gate, "synchronize_fact_store")?;
        self.synchronize_fact_store_under_gate()
    }

    fn synchronize_fact_store_under_gate(&self) -> RuntimeHostResult<()> {
        let result: RuntimeHostResult<()> = (|| {
            let mut facts = lock(&self.facts, "synchronize_fact_store")?;
            facts.synchronize(&self.ledger)?;
            for invalidation in facts.pending_invalidations() {
                let persisted = self.append_event_under_fact_gate(
                    EventSeverity::Info,
                    EventSource::Runtime,
                    OriginModule::FactStore,
                    EventActor::Runtime,
                    self.events.system_links()?,
                    FactPayloadDraft::invalidated(invalidation.clone(), AuditInput::new()),
                )?;
                facts.acknowledge_generated_invalidation(&invalidation, persisted.sequence())?;
            }
            Ok(())
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
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
            }),
            Err(error) => Err(RequestFailure::poison_without_terminal(
                critical_execution_error(&error),
            )),
        }
    }
}

#[derive(Clone)]
struct ContainedTaskTerminalDraft {
    task_id: IssuedTaskId,
    run_id: IssuedRunId,
    outcome: TaskOutcome,
    intent_already_recorded: bool,
    final_page: Option<String>,
    executed_steps: u32,
    failure_code: Option<&'static str>,
}

struct ActiveContainedRun<'a> {
    active: &'a Mutex<BTreeSet<RequestId>>,
    request_id: RequestId,
}

impl Drop for ActiveContainedRun<'_> {
    fn drop(&mut self) {
        let mut active = self
            .active
            .lock()
            .expect("active contained-run registry poisoned");
        assert!(
            active.remove(&self.request_id),
            "active contained-run identity missing during cleanup"
        );
    }
}

struct RuntimeContainedTask<'a> {
    host: &'a HostShared,
    request: &'a ValidatedRuntimeRequest<'a>,
    token: &'a LeaseToken,
    instance_alias: &'a str,
    connection_id: ConnectionId,
    task_id: IssuedTaskId,
    run_id: IssuedRunId,
    last_frame_id: Option<IssuedFrameId>,
    current_recognition_id: Option<IssuedRecognitionId>,
    step_actions: BTreeMap<u32, (IssuedActionId, String)>,
    finalizing: Option<TaskOutcome>,
}

impl RuntimeContainedTask<'_> {
    fn links(&self) -> EventLinksDraft {
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
}

impl ContainedTaskRuntime for RuntimeContainedTask<'_> {
    type Error = RequestFailure;

    fn capture(&mut self) -> Result<Frame, Self::Error> {
        let frame_id = self
            .host
            .events
            .issuer()
            .mint_frame_id()
            .map_err(|_| RequestFailure::poison_without_terminal(runtime_identifier_error()))?;
        let links = self.links().with_frame_id(frame_id);
        self.host.append_event(
            EventSeverity::Info,
            EventSource::Device,
            OriginModule::Capture,
            EventActor::Runtime,
            links.clone(),
            CapturePayloadDraft::requested(EventAction::CaptureObserve, AuditInput::new()),
        )?;
        match self.host.execution.capture(self.instance_alias) {
            Ok(frame) => {
                let artifact_png = frame.png_for_artifact().map_err(|_| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        "contained_task_frame_invalid",
                        "run_contained_task_capture",
                        RuntimeErrorCode::CaptureFailed,
                    ))
                })?;
                let write_context = ArtifactWriteContext::new(
                    self.request
                        .task_artifact_links(self.run_id)
                        .with_frame_id(frame_id),
                    links,
                    unix_ms_now().map_err(RequestFailure::poison_without_terminal)?,
                );
                let mut sink = RuntimeArtifactEventSink {
                    ledger: &self.host.ledger,
                    events: &self.host.events,
                };
                self.host
                    .artifacts
                    .put(
                        ArtifactWriteRequest::new(
                            ArtifactKind::CaptureFrame,
                            &artifact_png,
                            write_context,
                            ArtifactIssuePolicy::new(
                                ArtifactProducer::CaptureStore,
                                RetentionClass::DebugFull,
                                ArtifactRedactionState::NotRequired,
                            ),
                        ),
                        &mut sink,
                    )
                    .map_err(|_| {
                        RequestFailure::poison_without_terminal(artifact_store_error(
                            "persist_contained_task_frame",
                        ))
                    })?;
                self.last_frame_id = Some(frame_id);
                Ok(frame)
            }
            Err(error) => {
                let runtime_error =
                    RuntimeHostError::execution("run_contained_task_capture", &error);
                let failed = self.host.append_event(
                    EventSeverity::Error,
                    EventSource::Device,
                    OriginModule::Capture,
                    EventActor::Runtime,
                    links,
                    CapturePayloadDraft::failed(
                        EventAction::CaptureObserve,
                        DiagnosticCode::CaptureFailed,
                        EffectDisposition::NotPerformed,
                        AuditInput::new(),
                    ),
                )?;
                Err(RequestFailure {
                    state: RuntimeReceiptState::Failed,
                    terminal: Some(terminal(&failed)),
                    poison_runtime: runtime_error.is_fatal(),
                    error: Box::new(runtime_error),
                })
            }
        }
    }

    fn input(&mut self, action: InputAction) -> Result<(), Self::Error> {
        let success = self
            .host
            .input(self.request, self.token, &action, self.connection_id)?;
        if matches!(success.result, RuntimeResult::InputCommitted { .. }) {
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
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        match trace {
            ContainedTaskTrace::PackageAdmitted {
                task_label,
                package_label,
                package_sha256,
            } => self.append_task(
                EventSeverity::Info,
                self.links(),
                TaskPayloadDraft::semantic(
                    TaskSemanticFact::PackageAdmitted {
                        package_label,
                        task_label,
                        package_sha256,
                    },
                    AuditInput::new(),
                ),
            ),
            ContainedTaskTrace::RunStarted => self.append_task(
                EventSeverity::Info,
                self.links(),
                TaskPayloadDraft::semantic(TaskSemanticFact::RunStarted, AuditInput::new()),
            ),
            ContainedTaskTrace::CaptureCompleted { width, height } => {
                let frame_id = self.last_frame_id.ok_or_else(|| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        "contained_task_frame_identity_missing",
                        "run_contained_task",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?;
                let links = self.links().with_frame_id(frame_id);
                self.host.append_event(
                    EventSeverity::Info,
                    EventSource::Device,
                    OriginModule::Capture,
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
            }
            ContainedTaskTrace::RecognitionStarted {
                candidate_pages,
                width,
                height,
            } => {
                if self.current_recognition_id.is_some() {
                    return Err(RequestFailure::poison_without_terminal(
                        RuntimeHostError::fatal(
                            "contained_task_recognition_state_invalid",
                            "run_contained_task",
                            RuntimeErrorCode::RuntimeFatal,
                        ),
                    ));
                }
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
            }
            ContainedTaskTrace::RecognitionCompleted {
                candidate_pages,
                page_label,
                width,
                height,
            } => {
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
                self.host.append_event(
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
                )?;
                self.append_task(
                    EventSeverity::Info,
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
                )?;
                self.current_recognition_id = None;
                Ok(())
            }
            ContainedTaskTrace::StepStarted {
                step_index,
                operation_label,
                from_page,
            } => {
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
                        },
                        AuditInput::new(),
                    ),
                )
            }
            ContainedTaskTrace::EffectIntent {
                step_index,
                operation_label,
                action,
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
                self.append_task(
                    EventSeverity::Info,
                    self.links().with_action_id(action_id),
                    TaskPayloadDraft::semantic(
                        TaskSemanticFact::EffectIntent {
                            step_index,
                            operation_label,
                            action,
                        },
                        AuditInput::new(),
                    ),
                )
            }
            ContainedTaskTrace::EffectCompleted {
                step_index,
                operation_label,
            } => {
                let action_id =
                    contained_task_step_action(&self.step_actions, step_index, &operation_label)?;
                self.append_task(
                    EventSeverity::Info,
                    self.links().with_action_id(action_id),
                    TaskPayloadDraft::semantic(
                        TaskSemanticFact::EffectCompleted {
                            step_index,
                            operation_label,
                        },
                        AuditInput::new(),
                    ),
                )
            }
            ContainedTaskTrace::StepFinished {
                step_index,
                operation_label,
                page_label,
            } => {
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
                        },
                        AuditInput::new(),
                    ),
                )
            }
            ContainedTaskTrace::Finalizing { outcome } => {
                if self.finalizing.replace(outcome).is_some() || !self.step_actions.is_empty() {
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

struct RuntimeArtifactEventSink<'a> {
    ledger: &'a GlobalLedger,
    events: &'a RuntimeEvents,
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

const fn runtime_capture_backend(backend: CaptureBackendName) -> RuntimeCaptureBackend {
    match backend {
        CaptureBackendName::AdbScreencap => RuntimeCaptureBackend::AdbScreencap,
        CaptureBackendName::AdbScreencapEncode => RuntimeCaptureBackend::AdbScreencapEncode,
        CaptureBackendName::AdbScreencapRawGzip => RuntimeCaptureBackend::AdbScreencapRawGzip,
        CaptureBackendName::DroidcastRaw => RuntimeCaptureBackend::DroidcastRaw,
        CaptureBackendName::NemuIpc => RuntimeCaptureBackend::NemuIpc,
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
        }
    }

    fn poison(error: RuntimeHostError, terminal: Option<TerminalEvent>) -> Self {
        Self {
            state: RuntimeReceiptState::Failed,
            terminal,
            error: Box::new(error),
            poison_runtime: true,
        }
    }

    fn poison_without_terminal(error: RuntimeHostError) -> Self {
        Self::poison(error, None)
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
            error,
        }
    }

    fn backend(error: RuntimeHostError) -> Self {
        Self {
            diagnostic: DiagnosticCode::BackendOperationFailed,
            effect: EffectDisposition::Indeterminate,
            poison_runtime: false,
            release_after: true,
            destructive_started: true,
            transfer_after: false,
            error,
        }
    }

    fn poison(error: RuntimeHostError) -> Self {
        Self {
            diagnostic: DiagnosticCode::RuntimeDiagnostic,
            effect: EffectDisposition::Indeterminate,
            poison_runtime: true,
            release_after: false,
            destructive_started: false,
            transfer_after: false,
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
                let connection_id = shared.next_connection_id.fetch_add(1, Ordering::Relaxed);
                let connection_id = match ConnectionId::new(connection_id) {
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
                let thread = thread::Builder::new()
                    .name("actingcommand-runtime-client".to_string())
                    .spawn(move || {
                        connection_boundary(
                            stream,
                            connection_shared,
                            connection_id,
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
            RuntimeHostError::fatal(
                "runtime_connection_panicked",
                "join_runtime_connection",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        record_failure(&mut failure, result);
    }
    failure.map_or(Ok(()), Err)
}

fn connection_boundary(
    stream: TcpStream,
    shared: Arc<HostShared>,
    connection_id: ConnectionId,
    maximum_frame_bytes: usize,
    io_timeout: Duration,
) -> RuntimeHostResult<()> {
    let result = catch_unwind(AssertUnwindSafe(|| {
        connection_loop(
            stream,
            &shared,
            connection_id,
            maximum_frame_bytes,
            io_timeout,
        )
    }));
    let mut failure = match result {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error),
        Err(_) => Some(RuntimeHostError::fatal(
            "runtime_connection_panicked",
            "serve_runtime_connection",
            RuntimeErrorCode::RuntimeFatal,
        )),
    };
    let reason = if shared.fatal.is_shutdown_requested() {
        LeaseReleaseReason::HostShutdown
    } else {
        LeaseReleaseReason::Disconnect
    };
    record_failure(
        &mut failure,
        shared.cleanup_connection(connection_id, reason),
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
            record_failure(failure, shared.fatal.mark(error.clone()));
        }
        record_failure(failure, result);
    }
}

fn connection_loop(
    mut stream: TcpStream,
    shared: &HostShared,
    connection_id: ConnectionId,
    maximum_frame_bytes: usize,
    io_timeout: Duration,
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
        let frame = match read_frame(&mut stream, maximum_frame_bytes)? {
            FrameRead::Data(frame) => frame,
            FrameRead::Idle => continue,
            FrameRead::Closed => return Ok(()),
        };
        let request = serde_json::from_slice::<RuntimeRequest>(&frame)
            .map_err(|_| protocol_error("runtime_request_decode_failed"))?;
        let receipt = match cache.get(&request)? {
            Some(receipt) => receipt,
            None => {
                let receipt = shared.process_request(&request, connection_id)?;
                cache.insert(request.clone(), receipt.clone());
                receipt
            }
        };
        write_frame(&mut stream, &receipt, maximum_frame_bytes)?;
    }
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

fn monitor_probe_loop(shared: Arc<HostShared>) -> RuntimeHostResult<()> {
    while !shared.fatal.is_shutdown_requested() {
        let now_unix_ms = unix_ms_now()?;
        let due = lock(&shared.monitor_registry, "read_due_monitors")?
            .due(now_unix_ms, MAX_MONITOR_PROBES_PER_TICK)?;
        for probe in due {
            if shared.fatal.is_shutdown_requested() {
                return Ok(());
            }
            if let Err(error) = shared.run_monitor_probe(&probe) {
                shared.fatal.mark(error.clone())?;
                return Err(error);
            }
        }
        thread::sleep(MONITOR_POLL_INTERVAL);
    }
    Ok(())
}

const fn is_pipeline_event(event_type: EventType) -> bool {
    matches!(
        event_type,
        EventType::CaptureRequested
            | EventType::CaptureCompleted
            | EventType::CaptureFailed
            | EventType::RecognitionRequested
            | EventType::RecognitionCompleted
            | EventType::RecognitionFailed
            | EventType::TaskEffectIntent
            | EventType::TaskEffectCompleted
            | EventType::TaskStepFinished
            | EventType::TaskCompleted
            | EventType::TaskFailed
            | EventType::TaskCancelled
    )
}

fn performance_monitor_loop(
    shared: Arc<HostShared>,
    sample_interval: Duration,
) -> RuntimeHostResult<()> {
    while !shared.fatal.is_shutdown_requested() {
        thread::sleep(sample_interval);
        if shared.fatal.is_shutdown_requested() {
            break;
        }
        let observed_at_unix_ms = unix_ms_now()?;
        match shared.sample_performance(observed_at_unix_ms) {
            Ok(true) => break,
            Ok(false) => {}
            Err(error) => {
                shared.fatal.mark(error.clone())?;
                return Err(error);
            }
        }
    }
    Ok(())
}

fn append_runtime_start_event(
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
    state_root: &Path,
    takeover: bool,
) -> RuntimeHostResult<()> {
    let action = if takeover {
        EventAction::RuntimeTakeover
    } else {
        EventAction::RuntimeStart
    };
    let payload = if takeover {
        RuntimePayloadDraft::takeover(action, audit_path(state_root))
    } else {
        RuntimePayloadDraft::started(action, audit_path(state_root))
    };
    let draft = events.draft(
        EventSeverity::Info,
        EventSource::Runtime,
        OriginModule::Runtime,
        EventActor::Runtime,
        EventLinksDraft::default(),
        payload,
    )?;
    let draft = events.sanitize(draft)?;
    ledger
        .append(draft)
        .map(|_| ())
        .map_err(|_| ledger_error("append_runtime_start"))
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

fn inspect_debug_package(request: &PackageDebugRequest) -> RuntimeHostResult<PackageDebugSummary> {
    let path = Path::new(request.package_path());
    if !path.is_absolute() {
        return Err(debug_package_error("debug_package_path_not_absolute"));
    }
    let file =
        fs::File::open(path).map_err(|_| debug_package_error("debug_package_open_failed"))?;
    let metadata = file
        .metadata()
        .map_err(|_| debug_package_error("debug_package_metadata_failed"))?;
    if !metadata.is_file() || metadata.len() > DEFAULT_MAX_COMPRESSED_BYTES {
        return Err(debug_package_error("debug_package_file_invalid"));
    }
    let mut bytes = Vec::new();
    file.take(DEFAULT_MAX_COMPRESSED_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| debug_package_error("debug_package_read_failed"))?;
    if bytes.len() as u64 > DEFAULT_MAX_COMPRESSED_BYTES {
        return Err(debug_package_error("debug_package_file_too_large"));
    }
    let expected = Sha256Hash::parse_hex(request.expected_sha256())
        .map_err(|_| debug_package_error("debug_package_hash_invalid"))?;
    let instance = ContainmentInstanceId::new("runtime-debug-package")
        .map_err(|_| debug_package_error("debug_package_instance_invalid"))?;
    let mut containment = Containment::new();
    let bundle = containment
        .load(&instance, &bytes, &expected)
        .map_err(|_| debug_package_error("debug_package_containment_failed"))?;
    PackageDebugSummary::new(
        bundle.task_id().as_str(),
        bundle.verified_hash().to_string(),
        match bundle.layout() {
            PackageLayout::Lab => PackageDebugLayout::Lab,
            PackageLayout::Module => PackageDebugLayout::Module,
        },
        u32::try_from(bundle.entry_count())
            .map_err(|_| debug_package_error("debug_package_entry_count_overflow"))?,
        bundle.resident_bytes(),
        u32::try_from(bundle.task_count())
            .map_err(|_| debug_package_error("debug_package_task_count_overflow"))?,
        bundle.recognition_pack_path().is_some(),
        bundle.pages_path().is_some(),
        bundle.navigation_path().is_some(),
    )
    .map_err(|_| debug_package_error("debug_package_summary_invalid"))
}

fn prepare_contained_task(
    instance_alias: &str,
    request: &ContainedTaskRequest,
) -> Result<PreparedContainedTask, RequestFailure> {
    let path = Path::new(request.package_path());
    if !path.is_absolute() {
        return Err(contained_task_package_failure(
            "contained_task_path_not_absolute",
        ));
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
    let expected = ExternalExpectedSha256::parse_hex(request.expected_sha256())
        .map_err(|_| contained_task_package_failure("contained_task_package_hash_invalid"))?;
    PreparedContainedTask::load(instance_alias, &bytes, expected)
        .map_err(|error| contained_task_package_failure(error.code()))
}

fn contained_task_package_failure(code: &'static str) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::request(code, "run_contained_task", RuntimeErrorCode::PackageInvalid),
        RuntimeReceiptState::Denied,
        None,
    )
}

fn runtime_identifier_error() -> RuntimeHostError {
    RuntimeHostError::fatal(
        "runtime_identifier_issue_failed",
        "run_contained_task",
        RuntimeErrorCode::RuntimeFatal,
    )
}

fn runtime_evidence_documents(
    run_id: IssuedRunId,
    task_id: IssuedTaskId,
    task_outcome: TaskOutcome,
    terminal_receipt: &actingcommand_contract::ProjectedEvent,
    events: &[actingcommand_contract::ProjectedEvent],
) -> Result<EvidenceExportDocuments, RequestFailure> {
    let warning_count = events
        .iter()
        .filter(|event| event.severity == EventSeverity::Warning)
        .count();
    let error_count = events
        .iter()
        .filter(|event| matches!(event.severity, EventSeverity::Error | EventSeverity::Fatal))
        .count();
    let result = EvidenceJsonDocument::from_serializable(&serde_json::json!({
        "schema_version": "actingcommand.runtime.evidence-result.v1",
        "run_id": run_id.transport(),
        "task_id": task_id.transport(),
        "task_outcome": task_outcome,
        "terminal_receipt": terminal_receipt,
    }))
    .map_err(|error| {
        RequestFailure::request(
            evidence_request_error(error.code()),
            RuntimeReceiptState::Failed,
            Some(terminal_from_projected(terminal_receipt)),
        )
    })?;
    let diagnostics = EvidenceJsonDocument::from_serializable(&serde_json::json!({
        "schema_version": "actingcommand.runtime.evidence-diagnostics.v1",
        "event_count": events.len(),
        "warning_count": warning_count,
        "error_count": error_count,
    }))
    .map_err(|error| {
        RequestFailure::request(
            evidence_request_error(error.code()),
            RuntimeReceiptState::Failed,
            Some(terminal_from_projected(terminal_receipt)),
        )
    })?;
    EvidenceExportDocuments::new(
        result,
        diagnostics,
        "Runtime-owned Lab debug evidence export",
    )
    .map_err(|error| {
        RequestFailure::request(
            evidence_request_error(error.code()),
            RuntimeReceiptState::Failed,
            Some(terminal_from_projected(terminal_receipt)),
        )
    })
}

fn task_outcome_payload(outcome: TaskOutcome) -> TaskPayloadDraft {
    match outcome {
        TaskOutcome::Success => TaskPayloadDraft::completed(
            EventAction::ArtifactExport,
            EffectDisposition::Performed,
            AuditInput::new(),
        ),
        TaskOutcome::Failure => TaskPayloadDraft::failed(
            EventAction::ArtifactExport,
            DiagnosticCode::RuntimeDiagnostic,
            EffectDisposition::Performed,
            AuditInput::new(),
        ),
        TaskOutcome::Cancelled => TaskPayloadDraft::cancelled(
            EventAction::ArtifactExport,
            EffectDisposition::NotPerformed,
            AuditInput::new(),
        ),
    }
}

const fn task_outcome_severity(outcome: TaskOutcome) -> EventSeverity {
    match outcome {
        TaskOutcome::Success => EventSeverity::Info,
        TaskOutcome::Failure => EventSeverity::Error,
        TaskOutcome::Cancelled => EventSeverity::Warning,
    }
}

const fn task_outcome_event_type(outcome: TaskOutcome) -> EventType {
    match outcome {
        TaskOutcome::Success => EventType::TaskCompleted,
        TaskOutcome::Failure => EventType::TaskFailed,
        TaskOutcome::Cancelled => EventType::TaskCancelled,
    }
}

const fn terminal_from_projected(event: &actingcommand_contract::ProjectedEvent) -> TerminalEvent {
    TerminalEvent {
        sequence: event.sequence,
        event_id: event.event_id,
    }
}

fn evidence_request_error(code: &'static str) -> RuntimeHostError {
    RuntimeHostError::request(
        code,
        "export_runtime_evidence",
        RuntimeErrorCode::EvidenceExportFailed,
    )
}

fn debug_package_error(code: &'static str) -> RuntimeHostError {
    RuntimeHostError::request(code, "debug_package", RuntimeErrorCode::PackageInvalid)
}

fn runtime_error_receipt(
    request: &RuntimeRequest,
    state: RuntimeReceiptState,
    terminal: Option<TerminalEvent>,
    error: RuntimeErrorProjection,
) -> RuntimeHostResult<RuntimeReceipt> {
    RuntimeReceipt::error(request, state, terminal, error).map_err(|_| receipt_error())
}

fn safe_reset_replay_denied(code: &'static str) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::request(
            code,
            "recover_safe_reset",
            RuntimeErrorCode::ProtocolInvalid,
        ),
        RuntimeReceiptState::Denied,
        None,
    )
}

fn application_replay_denied(code: &'static str) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::request(
            code,
            "recover_application_lifecycle",
            RuntimeErrorCode::ProtocolInvalid,
        ),
        RuntimeReceiptState::Denied,
        None,
    )
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
) -> PolicyDispatchEventData {
    PolicyDispatchEventData {
        decision_id: intent.decision_id.clone(),
        task_id: intent.task_id.clone(),
        instance_id: intent.instance_id.clone(),
        operation_id: intent.operation_id.clone(),
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
    }
}

fn policy_execution_severity(data: &PolicyExecutionEventData) -> EventSeverity {
    match &data.outcome {
        actingcommand_contract::PolicyExecutionOutcome::Succeeded { .. } => EventSeverity::Info,
        actingcommand_contract::PolicyExecutionOutcome::Failed { failure }
            if failure.effective_class
                == actingcommand_contract::PolicyFailureClass::Recoverable =>
        {
            EventSeverity::Warning
        }
        actingcommand_contract::PolicyExecutionOutcome::Failed { .. } => EventSeverity::Error,
    }
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

fn governance_authentication_denied(code: &'static str) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::request(
            code,
            "authenticate_governance",
            RuntimeErrorCode::InvalidRequest,
        ),
        RuntimeReceiptState::Denied,
        None,
    )
}

fn constant_time_digest_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn agent_request_failure(error: RuntimeHostError) -> RequestFailure {
    if error.is_fatal() {
        RequestFailure::poison_without_terminal(error)
    } else {
        RequestFailure::request(error, RuntimeReceiptState::Denied, None)
    }
}

fn proposal_request_failure(error: RuntimeHostError) -> RequestFailure {
    if error.is_fatal() {
        RequestFailure::poison_without_terminal(error)
    } else {
        RequestFailure::request(error, RuntimeReceiptState::Denied, None)
    }
}

fn project_interface_failure(error: RuntimeHostError) -> RequestFailure {
    if error.is_fatal() {
        RequestFailure::poison_without_terminal(error)
    } else {
        RequestFailure::request(error, RuntimeReceiptState::Denied, None)
    }
}

fn proposal_report_is_verified(
    events: &[PersistedEvent],
    reference: &ProjectedArtifactReference,
) -> bool {
    proposal_report_verified_sequence(events, reference).is_some()
}

fn proposal_report_verified_sequence(
    events: &[PersistedEvent],
    reference: &ProjectedArtifactReference,
) -> Option<u64> {
    events.iter().find_map(|event| {
        event
            .artifacts()
            .iter()
            .any(|artifact| artifact.project(true) == *reference)
            .then_some(event.sequence())
    })
}

fn strategic_evidence_pointer(
    reference: &ProjectedArtifactReference,
) -> RuntimeHostResult<StrategicEvidencePointer> {
    Ok(StrategicEvidencePointer {
        artifact_id: artifact_id_text(reference)?,
        sha256: reference.sha256.clone(),
    })
}

fn artifact_id_text(reference: &ProjectedArtifactReference) -> RuntimeHostResult<String> {
    serde_json::to_value(reference.artifact_id)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| {
            RuntimeHostError::fatal(
                "artifact_identity_encode_failed",
                "prepare_strategic_report",
                RuntimeErrorCode::RuntimeFatal,
            )
        })
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
    let mut failure = join_runtime_thread(sweep_thread, "join_runtime_sweeper").err();
    record_failure(
        &mut failure,
        join_runtime_thread(monitor_thread, "join_runtime_monitor"),
    );
    record_failure(
        &mut failure,
        join_runtime_thread(performance_thread, "join_runtime_performance"),
    );
    if let Err(error) = fs::remove_file(info_path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        record_failure(
            &mut failure,
            Err(RuntimeHostError::fatal(
                "runtime_info_remove_failed",
                "abort_runtime_start",
                RuntimeErrorCode::RuntimeFatal,
            )),
        );
    }
    match Arc::try_unwrap(shared) {
        Ok(shared) => record_failure(&mut failure, shared.close()),
        Err(_) => record_failure(
            &mut failure,
            Err(RuntimeHostError::fatal(
                "runtime_reference_leaked",
                "abort_runtime_start",
                RuntimeErrorCode::RuntimeFatal,
            )),
        ),
    }
    failure.map_or(Ok(()), Err)
}

fn record_failure(slot: &mut Option<RuntimeHostError>, result: RuntimeHostResult<()>) {
    if let Err(error) = result
        && slot.is_none()
    {
        *slot = Some(error);
    }
}
