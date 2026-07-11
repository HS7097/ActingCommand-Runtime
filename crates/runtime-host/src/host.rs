// SPDX-License-Identifier: AGPL-3.0-only

use crate::events::RuntimeEvents;
use crate::ipc::{DEFAULT_RUNTIME_MAX_FRAME_BYTES, FrameRead, read_frame, write_frame};
use crate::monitor::{DueMonitorProbe, MonitorRegistry, MonitorUpdate};
use crate::owner::{OwnerGuard, OwnerStartup};
use crate::time::unix_ms_now;
use crate::{FatalState, RuntimeHostError, RuntimeHostResult};
use actingcommand_artifact_store::{
    ArtifactEventSink, ArtifactStore, ArtifactStoreError, ArtifactStoreResult,
    ArtifactWriteContext, ArtifactWriteRequest,
};
use actingcommand_contract::{
    ArtifactIssuePolicy, ArtifactKind, ArtifactProducer, ArtifactRedactionState, AuditInput,
    CapturePayloadDraft, CaptureSequence, CaptureSequenceSpec, ClientPayloadDraft,
    CommandPayloadDraft, DiagnosticCode, EffectDisposition, EventAction, EventActor, EventDraft,
    EventLinksDraft, EventPayload, EventQuery, EventSeverity, EventSource, EventType, InputAction,
    InputPayload, InputPayloadDraft, InstanceId, IssuedMonitorProbe,
    IssuedReadOnlyCaptureCapability, LeaseId, LeasePayloadDraft, LeaseQueuePolicy, LeaseToken,
    MAX_INSTANCE_ALIAS_BYTES, MonitorPayloadDraft, MonitorRecoveryCoordinationReason, OriginModule,
    RUNTIME_INFO_FILE, ReadonlyObservation, RecognitionPayloadDraft, RecognitionVerdict, RequestId,
    RetentionClass, RuntimeCaptureBackend, RuntimeControlPlaneStatus, RuntimeErrorCode,
    RuntimeErrorProjection, RuntimeInfo, RuntimeInstanceStatus, RuntimeMonitorPolicy,
    RuntimeOperation, RuntimePayloadDraft, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest,
    RuntimeResult, SchedulerPayloadDraft, TerminalEvent, ValidatedRuntimeRequest,
};
use actingcommand_device::CaptureBackendName;
use actingcommand_execution_kernel::{ExecutionBackendProvider, ExecutionKernel, decide_monitor};
use actingcommand_ledger::critical::{
    CriticalActionReport, CriticalEventPlan, CriticalExecutionError, CriticalOperation,
    DefiniteEffectDisposition, LeaseTransitionTarget, execute_critical,
};
use actingcommand_ledger::{GlobalLedger, GlobalLedgerConfig, PersistedEvent};
use actingcommand_scheduler::{
    CancelledQueuedLease, ConnectionId, LeasePreparation, LeaseReleaseReason, LeaseTransferReason,
    PreparedLeaseTransfer, QueueAdmissionDecision, QueueLeaseRequest, QueuePoll, QueuedLease,
    SchedulerConfig, SchedulerError, SeedScheduler, TransferPreparation,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::Write;
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
const MAX_MONITOR_PROBES_PER_TICK: usize = 16;

#[derive(Clone)]
pub struct RuntimeHostConfig {
    state_root: PathBuf,
    bind_address: SocketAddr,
    scheduler: SchedulerConfig,
    maximum_frame_bytes: usize,
    io_timeout: Duration,
    secret_fingerprint_salt: Vec<u8>,
}

impl RuntimeHostConfig {
    pub fn new(state_root: impl Into<PathBuf>, secret_fingerprint_salt: impl AsRef<[u8]>) -> Self {
        Self {
            state_root: state_root.into(),
            bind_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            scheduler: SchedulerConfig::default(),
            maximum_frame_bytes: DEFAULT_RUNTIME_MAX_FRAME_BYTES,
            io_timeout: DEFAULT_RUNTIME_IO_TIMEOUT,
            secret_fingerprint_salt: secret_fingerprint_salt.as_ref().to_vec(),
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

    pub fn with_io_timeout(mut self, io_timeout: Duration) -> Self {
        self.io_timeout = io_timeout;
        self
    }

    pub fn with_maximum_frame_bytes(mut self, maximum_frame_bytes: usize) -> Self {
        self.maximum_frame_bytes = maximum_frame_bytes;
        self
    }

    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    fn validate(&self) -> RuntimeHostResult<()> {
        self.scheduler
            .validate()
            .map_err(|error| RuntimeHostError::scheduler("validate_runtime_config", &error))?;
        if self.state_root.as_os_str().is_empty()
            || !self.bind_address.ip().is_loopback()
            || self.io_timeout.is_zero()
            || self.maximum_frame_bytes == 0
            || self.maximum_frame_bytes > DEFAULT_RUNTIME_MAX_FRAME_BYTES
            || self.secret_fingerprint_salt.is_empty()
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
            .field("maximum_frame_bytes", &self.maximum_frame_bytes)
            .field("io_timeout", &self.io_timeout)
            .field("secret_fingerprint_salt", &"<redacted>")
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
        let started_at_unix_ms = unix_ms_now()?;
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
            owner: Mutex::new(owner),
            ledger,
            artifacts,
            events,
            execution: ExecutionKernel::new(provider),
            registered_instances: Mutex::new(registered_instances),
            monitor_registry: Mutex::new(monitor_registry),
            queued_requests: Mutex::new(BTreeMap::new()),
            queue_terminals: Mutex::new(QueueTerminalStore::default()),
            admission_guards: Mutex::new(BTreeMap::new()),
            next_connection_id: AtomicU64::new(1),
            clock: Instant::now(),
            fatal,
        });
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
                failed_start_cleanup(shared, &info_path, None, None)?;
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
                failed_start_cleanup(shared, &info_path, Some(sweep_thread), None)?;
                return Err(original);
            }
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
                failed_start_cleanup(shared, &info_path, Some(sweep_thread), Some(monitor_thread))?;
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
        })
    }

    pub const fn runtime_info(&self) -> &RuntimeInfo {
        &self.info
    }

    pub fn fatal_error(&self) -> RuntimeHostResult<Option<RuntimeHostError>> {
        self.shared
            .as_ref()
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    "runtime_host_closed",
                    "read_runtime_health",
                    RuntimeErrorCode::RuntimeUnavailable,
                )
            })?
            .fatal
            .current()
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

    pub fn close(mut self) -> RuntimeHostResult<()> {
        self.shutdown()
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

struct HostShared {
    owner_epoch: actingcommand_contract::OwnerEpoch,
    scheduler: Mutex<SeedScheduler>,
    ledger: GlobalLedger,
    artifacts: ArtifactStore,
    owner: Mutex<OwnerGuard>,
    events: RuntimeEvents,
    execution: ExecutionKernel,
    registered_instances: Mutex<BTreeMap<InstanceId, RegisteredInstance>>,
    monitor_registry: Mutex<MonitorRegistry>,
    queued_requests: Mutex<BTreeMap<RequestId, QueuedRequestContext>>,
    queue_terminals: Mutex<QueueTerminalStore>,
    admission_guards: Mutex<BTreeMap<InstanceId, Arc<Mutex<()>>>>,
    next_connection_id: AtomicU64,
    clock: Instant,
    fatal: FatalState,
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
        }
    }

    fn control_plane_status(&self) -> Result<OperationSuccess, RequestFailure> {
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
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::Status { status },
        })
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
        let links = capability.event_links(request);
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
            links,
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
                    capability.event_links(request),
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
                    capability.event_links(request),
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
                    capability.event_links(request),
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
                    capability.event_links(request),
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
        let write_context = ArtifactWriteContext::new(
            capability.artifact_links(request),
            capability.event_links(request),
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
                        RetentionClass::Adaptive,
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
        self.append_capture_completed(
            capability.event_links(request),
            observation.width(),
            observation.height(),
        )?;
        let event = self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Recognition,
            EventActor::Runtime,
            capability.event_links(request),
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
        u64::try_from(self.clock.elapsed().as_millis()).map_err(|_| {
            RuntimeHostError::fatal(
                "monotonic_clock_overflow",
                "read_runtime_clock",
                RuntimeErrorCode::RuntimeFatal,
            )
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

    fn append_event_raw(
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
        self.ledger
            .append(draft)
            .map_err(|_| ledger_error("append_runtime_event"))
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
) -> RuntimeHostResult<()> {
    shared.fatal.request_shutdown();
    let mut failure = join_runtime_thread(sweep_thread, "join_runtime_sweeper").err();
    record_failure(
        &mut failure,
        join_runtime_thread(monitor_thread, "join_runtime_monitor"),
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
