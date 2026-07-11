// SPDX-License-Identifier: AGPL-3.0-only

use crate::backend::BackendWorker;
use crate::events::RuntimeEvents;
use crate::ipc::{DEFAULT_RUNTIME_MAX_FRAME_BYTES, FrameRead, read_frame, write_frame};
use crate::owner::{OwnerGuard, OwnerStartup};
use crate::time::unix_ms_now;
use crate::{
    FatalState, InputBackendProvider, ResolvedInputInstance, RuntimeHostError, RuntimeHostResult,
};
use actingcommand_contract::{
    AuditInput, DiagnosticCode, EffectDisposition, EventAction, EventActor, EventLinksDraft,
    EventQuery, EventSeverity, EventSource, EventType, InputAction, InputPayloadDraft, InstanceId,
    LeaseId, LeasePayloadDraft, LeaseToken, OriginModule, RUNTIME_INFO_FILE,
    ReadOnlyCaptureCapability, RequestId, RuntimeErrorCode, RuntimeErrorProjection, RuntimeInfo,
    RuntimeOperation, RuntimePayloadDraft, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest,
    RuntimeResult, SchedulerPayloadDraft, TerminalEvent, ValidatedRuntimeRequest,
};
use actingcommand_ledger::critical::{
    CriticalActionReport, CriticalEventPlan, CriticalExecutionError, CriticalOperation,
    DefiniteEffectDisposition, LeaseTransitionTarget, execute_critical,
};
use actingcommand_ledger::{GlobalLedger, GlobalLedgerConfig, PersistedEvent};
use actingcommand_scheduler::{
    ConnectionId, LeasePreparation, LeaseReleaseReason, SchedulerConfig, SchedulerError,
    SeedScheduler,
};
use std::collections::{BTreeMap, VecDeque};
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
const ACCEPT_IDLE_INTERVAL: Duration = Duration::from_millis(20);
const MAX_REQUEST_CACHE_ENTRIES: usize = 4096;

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
}

impl RuntimeHost {
    pub fn start(
        config: RuntimeHostConfig,
        provider: Arc<dyn InputBackendProvider>,
    ) -> RuntimeHostResult<Self> {
        config.validate()?;
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
        let scheduler = SeedScheduler::new(owner_epoch, config.scheduler, takeover_instances, 0)
            .map_err(|error| RuntimeHostError::scheduler("start_runtime_host", &error))?;
        let ledger_owner = format!("actingd-{}-{started_at_unix_ms}", std::process::id());
        let ledger = GlobalLedger::open(GlobalLedgerConfig::new(
            config.state_root.join("ledger"),
            ledger_owner,
        ))
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
            events,
            provider,
            backends: Mutex::new(BTreeMap::new()),
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
                failed_start_cleanup(shared, &info_path, None)?;
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
                failed_start_cleanup(shared, &info_path, Some(sweep_thread))?;
                return Err(original);
            }
        };
        Ok(Self {
            info,
            info_path,
            shared: Some(shared),
            accept_thread: Some(accept_thread),
            sweep_thread: Some(sweep_thread),
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

struct LiveBackend {
    connection_id: ConnectionId,
    audit_endpoint: String,
    worker: BackendWorker,
}

struct HostShared {
    owner_epoch: actingcommand_contract::OwnerEpoch,
    scheduler: Mutex<SeedScheduler>,
    ledger: GlobalLedger,
    owner: Mutex<OwnerGuard>,
    events: RuntimeEvents,
    provider: Arc<dyn InputBackendProvider>,
    backends: Mutex<BTreeMap<LeaseId, Arc<Mutex<LiveBackend>>>>,
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
            RuntimeOperation::RenewLease { token } => {
                self.renew_lease(validated, request.request_id(), token, connection_id)
            }
            RuntimeOperation::ReleaseLease { token } => {
                self.release_lease(validated, request.request_id(), token, connection_id)
            }
            RuntimeOperation::AdmitReadonly { instance_alias } => {
                self.admit_readonly(validated, instance_alias)
            }
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
        if preparation.is_existing() {
            let terminal =
                self.existing_lease_terminal(request_id, preparation.token().lease_id())?;
            return Ok(OperationSuccess {
                state: RuntimeReceiptState::Admitted,
                terminal: Some(terminal),
                result: RuntimeResult::LeaseGranted {
                    token: preparation.token().clone(),
                },
            });
        }
        self.append_lease_requested(request, &resolved)?;
        self.append_scheduler_admitted(request, &resolved, None)?;
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
            || match self.commit_acquire(preparation, instance_alias, &endpoint, connection_id) {
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

    fn existing_lease_terminal(
        &self,
        request_id: RequestId,
        lease_id: LeaseId,
    ) -> Result<TerminalEvent, RequestFailure> {
        let events = self
            .ledger
            .query(EventQuery {
                event_type: Some(EventType::LeaseGranted),
                request_id: Some(request_id),
                lease_id: Some(lease_id),
                ..EventQuery::default()
            })
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error("query_lease_grant"))
            })?;
        match events.as_slice() {
            [event] => Ok(terminal(event)),
            [] => Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "lease_grant_event_missing",
                    "recover_idempotent_acquire",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            )),
            _ => Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "lease_grant_event_duplicated",
                    "recover_idempotent_acquire",
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
        let backend = self.validated_backend(request, token, connection_id)?;
        let backend = lock(&backend, "lock_input_backend")?;
        let resolved = ResolvedInputInstanceForEvent::from_backend(&backend);
        self.append_scheduler_admitted_for_token(request, token, &resolved.audit_endpoint)?;
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
            &resolved.audit_endpoint,
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
        drop(backend);
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
        let backend = self.validated_backend(request, token, connection_id)?;
        let mut backend = lock(&backend, "lock_input_backend")?;
        self.append_scheduler_admitted_for_token(request, token, &backend.audit_endpoint)?;
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
            &backend.audit_endpoint,
        )?;
        let plan = CriticalEventPlan::new(
            CriticalOperation::LeaseTransition(LeaseTransitionTarget::Released),
            intent,
        )
        .map_err(|_| RequestFailure::poison_without_terminal(critical_plan_error()))?;
        let endpoint = backend.audit_endpoint.clone();
        let outcome_links = links.clone();
        let failure_links = links;
        let result = execute_critical(
            &self.ledger,
            self.events.fingerprinter(),
            plan,
            || match self.complete_explicit_release(request_id, token, connection_id, &mut backend)
            {
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
        drop(backend);
        self.map_critical_lease_result(result, RuntimeReceiptState::Completed, |token| {
            RuntimeResult::LeaseReleased {
                instance_id: token.instance_id(),
                lease_id: token.lease_id(),
            }
        })
    }

    fn admit_readonly(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        let event = self.append_scheduler_admitted(request, &resolved, None)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Admitted,
            terminal: Some(terminal(&event)),
            result: RuntimeResult::ReadOnlyAdmitted {
                capability: ReadOnlyCaptureCapability::new(resolved.instance_id()),
            },
        })
    }

    fn input(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        action: &InputAction,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let backend = self.validated_backend(request, token, connection_id)?;
        let backend = lock(&backend, "lock_input_backend")?;
        self.append_scheduler_admitted_for_token(request, token, &backend.audit_endpoint)?;
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
                InputPayloadDraft::intent(event_action, audit_endpoint(&backend.audit_endpoint)),
            )
            .and_then(|draft| self.events.sanitize(draft))
            .map_err(RequestFailure::poison_without_terminal)?;
        let plan = CriticalEventPlan::new(CriticalOperation::DeviceWrite, intent)
            .map_err(|_| RequestFailure::poison_without_terminal(critical_plan_error()))?;
        let endpoint = backend.audit_endpoint.clone();
        let outcome_links = links.clone();
        let failure_links = links;
        let action_for_worker = action.clone();
        let result = execute_critical(
            &self.ledger,
            self.events.fingerprinter(),
            plan,
            || {
                let validation =
                    lock(&self.scheduler, "validate_device_write").and_then(|scheduler| {
                        scheduler
                            .validate_write(token, connection_id, self.monotonic_ms()?)
                            .map_err(|error| {
                                RuntimeHostError::scheduler("validate_device_write", &error)
                            })
                    });
                if let Err(error) = validation {
                    return CriticalActionReport::Failed {
                        error: ActionFailure::scheduler(error),
                        effect: EffectDisposition::NotPerformed,
                    };
                }
                match backend.worker.execute(action_for_worker) {
                    Ok(()) => CriticalActionReport::Succeeded {
                        value: (),
                        effect: DefiniteEffectDisposition::Performed,
                    },
                    Err(error) => CriticalActionReport::Failed {
                        error: ActionFailure::backend(error),
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
        let mapped = match result {
            Ok(receipt) => Ok(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: Some(terminal(receipt.outcome())),
                result: RuntimeResult::InputCommitted { action_id },
            }),
            Err(CriticalExecutionError::Action { error, outcome, .. }) => {
                let release_after = error.release_after;
                let failure = RequestFailure {
                    state: RuntimeReceiptState::Failed,
                    terminal: Some(terminal(&outcome)),
                    error: Box::new(error.error),
                    poison_runtime: error.poison_runtime,
                };
                drop(backend);
                if release_after {
                    self.cleanup_token(token, connection_id, LeaseReleaseReason::BackendFailure)
                        .map_err(RequestFailure::poison_without_terminal)?;
                }
                return Err(failure);
            }
            Err(error) => Err(RequestFailure::poison_without_terminal(
                critical_execution_error(&error),
            )),
        };
        drop(backend);
        mapped
    }

    fn validated_backend(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> Result<Arc<Mutex<LiveBackend>>, RequestFailure> {
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
        lock(&self.backends, "read_backend_registry")?
            .get(&token.lease_id())
            .cloned()
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "active_lease_backend_missing",
                    "read_backend_registry",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })
    }

    fn commit_acquire(
        &self,
        preparation: LeasePreparation,
        instance_alias: &str,
        audit_endpoint: &str,
        connection_id: ConnectionId,
    ) -> Result<LeaseToken, ActionFailure> {
        let token = preparation.token().clone();
        let mut worker = match BackendWorker::open(
            Arc::clone(&self.provider),
            instance_alias.to_string(),
            self.fatal.clone(),
        ) {
            Ok(worker) => worker,
            Err(error) => {
                return Err(if error.is_fatal() {
                    ActionFailure::poison(error)
                } else {
                    ActionFailure {
                        diagnostic: DiagnosticCode::BackendOpenFailed,
                        effect: EffectDisposition::NotPerformed,
                        poison_runtime: false,
                        release_after: false,
                        error,
                    }
                });
            }
        };
        let now = self.monotonic_ms().map_err(ActionFailure::poison)?;
        let mut scheduler = lock(&self.scheduler, "commit_lease").map_err(ActionFailure::poison)?;
        if let Err(error) = scheduler.commit_acquire(preparation, now) {
            let close_error = worker.close().err();
            return Err(close_error.map_or_else(
                || ActionFailure::scheduler(RuntimeHostError::scheduler("commit_lease", &error)),
                ActionFailure::poison,
            ));
        }
        let protected = scheduler.protected_instance_ids(now);
        if let Err(error) = lock(&self.owner, "update_owner_file")
            .and_then(|mut owner| owner.set_active_instances(protected))
        {
            let rollback = scheduler.rollback_lease(&token).err();
            let close = worker.close().err();
            let rollback_error = rollback
                .map(|rollback| RuntimeHostError::scheduler("rollback_lease", &rollback))
                .or(close)
                .unwrap_or(error);
            return Err(ActionFailure::poison(rollback_error));
        }
        let mut backends = match lock(&self.backends, "insert_backend_registry") {
            Ok(backends) => backends,
            Err(error) => {
                let mut failure = error;
                if let Err(error) = scheduler.rollback_lease(&token) {
                    failure = RuntimeHostError::scheduler("rollback_lease", &error);
                }
                let protected = scheduler.protected_instance_ids(now);
                if let Err(error) = lock(&self.owner, "update_owner_file")
                    .and_then(|mut owner| owner.set_active_instances(protected))
                {
                    failure = error;
                }
                if let Err(error) = worker.close() {
                    failure = error;
                }
                return Err(ActionFailure::poison(failure));
            }
        };
        if backends.contains_key(&token.lease_id()) {
            drop(backends);
            let mut failure = RuntimeHostError::fatal(
                "duplicate_backend_guard",
                "insert_backend_registry",
                RuntimeErrorCode::RuntimeFatal,
            );
            if let Err(error) = scheduler.rollback_lease(&token) {
                failure = RuntimeHostError::scheduler("rollback_lease", &error);
            }
            let protected = scheduler.protected_instance_ids(now);
            if let Err(error) = lock(&self.owner, "update_owner_file")
                .and_then(|mut owner| owner.set_active_instances(protected))
            {
                failure = error;
            }
            if let Err(error) = worker.close() {
                failure = error;
            }
            return Err(ActionFailure::poison(failure));
        }
        let live = Arc::new(Mutex::new(LiveBackend {
            connection_id,
            audit_endpoint: audit_endpoint.to_string(),
            worker,
        }));
        backends.insert(token.lease_id(), live);
        Ok(token)
    }

    fn complete_explicit_release(
        &self,
        request_id: RequestId,
        token: &LeaseToken,
        connection_id: ConnectionId,
        backend: &mut LiveBackend,
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
        self.finish_backend_release(token, backend)
    }

    fn finish_backend_release(
        &self,
        token: &LeaseToken,
        backend: &mut LiveBackend,
    ) -> Result<LeaseToken, ActionFailure> {
        let mut failure = self.persist_active_instances().err();
        match lock(&self.backends, "remove_backend_registry") {
            Ok(mut backends) => {
                if backends.remove(&token.lease_id()).is_none() {
                    failure.get_or_insert_with(|| {
                        RuntimeHostError::fatal(
                            "active_lease_backend_missing",
                            "remove_backend_registry",
                            RuntimeErrorCode::RuntimeFatal,
                        )
                    });
                }
            }
            Err(error) => {
                failure.get_or_insert(error);
            }
        }
        if let Err(error) = backend.worker.close() {
            failure.get_or_insert(error);
        }
        if let Some(error) = failure {
            return Err(ActionFailure::poison(error));
        }
        Ok(token.clone())
    }

    fn cleanup_token(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
        reason: LeaseReleaseReason,
    ) -> RuntimeHostResult<()> {
        let backend = lock(&self.backends, "read_backend_registry")?
            .get(&token.lease_id())
            .cloned();
        let Some(backend) = backend else {
            let active = lock(&self.scheduler, "check_cleanup_lease")?
                .active_tokens()
                .into_iter()
                .any(|active| active == *token);
            return if active {
                Err(RuntimeHostError::fatal(
                    "active_lease_backend_missing",
                    "cleanup_runtime_connection",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            } else {
                Ok(())
            };
        };
        let mut backend = lock(&backend, "lock_input_backend")?;
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
            .lease_intent(action, links.clone(), &backend.audit_endpoint)
            .map_err(|failure| *failure.error)?;
        let plan = CriticalEventPlan::new(CriticalOperation::LeaseTransition(target), intent)
            .map_err(|_| critical_plan_error())?;
        let endpoint = backend.audit_endpoint.clone();
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
                    Ok(_) => match self.finish_backend_release(token, &mut backend) {
                        Ok(token) => CriticalActionReport::Succeeded {
                            value: token,
                            effect: DefiniteEffectDisposition::Performed,
                        },
                        Err(error) => CriticalActionReport::Failed {
                            effect: error.effect,
                            error,
                        },
                    },
                    Err(SchedulerError::LeaseMissing | SchedulerError::LeaseMismatch) => {
                        let already_removed = lock(&self.backends, "check_backend_cleanup")
                            .map(|backends| !backends.contains_key(&token.lease_id()));
                        match already_removed {
                            Ok(true) => CriticalActionReport::Succeeded {
                                value: token.clone(),
                                effect: DefiniteEffectDisposition::NotPerformed,
                            },
                            Ok(false) => CriticalActionReport::Failed {
                                error: ActionFailure::poison(RuntimeHostError::fatal(
                                    "scheduler_backend_state_mismatch",
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

    fn expire_due_leases(&self) -> RuntimeHostResult<()> {
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
            let backend = lock(&self.backends, "read_backend_registry")?
                .get(&token.lease_id())
                .cloned();
            let Some(backend) = backend else {
                return Err(RuntimeHostError::fatal(
                    "active_lease_backend_missing",
                    "expire_runtime_lease",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            };
            let connection_id = lock(&backend, "read_backend_connection")?.connection_id;
            self.cleanup_token(&token, connection_id, LeaseReleaseReason::Expired)?;
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
            let connection_id = lock(&self.backends, "read_backend_registry")?
                .get(&token.lease_id())
                .cloned()
                .ok_or_else(|| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        "active_lease_backend_missing",
                        "expire_runtime_lease",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?
                .lock()
                .map_err(|_| {
                    RequestFailure::poison_without_terminal(lock_poison_error(
                        "expire_runtime_lease",
                    ))
                })?
                .connection_id;
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
            let backend = lock(&self.backends, "read_backend_registry")?
                .get(&token.lease_id())
                .cloned();
            if let Some(backend) = backend {
                let connection_id = lock(&backend, "read_backend_connection")?.connection_id;
                record_failure(
                    &mut failure,
                    self.cleanup_token(&token, connection_id, LeaseReleaseReason::HostShutdown),
                );
            } else {
                record_failure(
                    &mut failure,
                    Err(RuntimeHostError::fatal(
                        "active_lease_backend_missing",
                        "close_runtime_host",
                        RuntimeErrorCode::RuntimeFatal,
                    )),
                );
            }
        }
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

    fn resolve_instance(
        &self,
        instance_alias: &str,
    ) -> Result<ResolvedInputInstance, RequestFailure> {
        self.provider.resolve(instance_alias).ok_or_else(|| {
            RequestFailure::request(
                RuntimeHostError::request(
                    "instance_unknown",
                    "resolve_runtime_instance",
                    RuntimeErrorCode::InstanceUnknown,
                ),
                RuntimeReceiptState::Denied,
                None,
            )
        })
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
        resolved: &ResolvedInputInstance,
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
        resolved: &ResolvedInputInstance,
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
        resolved: &ResolvedInputInstance,
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
            error,
        }
    }

    fn backend(error: RuntimeHostError) -> Self {
        Self {
            diagnostic: DiagnosticCode::BackendOperationFailed,
            effect: EffectDisposition::Indeterminate,
            poison_runtime: false,
            release_after: true,
            error,
        }
    }

    fn poison(error: RuntimeHostError) -> Self {
        Self {
            diagnostic: DiagnosticCode::RuntimeDiagnostic,
            effect: EffectDisposition::Indeterminate,
            poison_runtime: true,
            release_after: false,
            error,
        }
    }
}

struct ResolvedInputInstanceForEvent {
    audit_endpoint: String,
}

impl ResolvedInputInstanceForEvent {
    fn from_backend(backend: &LiveBackend) -> Self {
        Self {
            audit_endpoint: backend.audit_endpoint.clone(),
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
) -> RuntimeHostResult<()> {
    shared.fatal.request_shutdown();
    let mut failure = join_runtime_thread(sweep_thread, "join_runtime_sweeper").err();
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
