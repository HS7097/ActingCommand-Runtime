// SPDX-License-Identifier: AGPL-3.0-only

use crate::ipc::{DEFAULT_RUNTIME_MAX_FRAME_BYTES, exchange};
use crate::{RuntimeClientError, RuntimeClientResult};
use actingcommand_contract::{
    ActionId, CaptureSequenceSpec, CorrelationId, EventActor, EventQuery, EventSource,
    IdentifierIssuer, InputAction, IssuedCorrelationId, LeaseQueuePolicy, LeaseQueueStatus,
    LeaseToken, OwnerEpoch, PackageDebugRequest, ProjectedEvent, ProjectionProfile,
    RUNTIME_INFO_FILE, RequestId, ResourceAuthoringEvent, RuntimeControlPlaneStatus,
    RuntimeDebugEvent, RuntimeEventBatch, RuntimeEvidenceExportRequest, RuntimeInfo,
    RuntimeMonitorInstanceStatus, RuntimeMonitorPolicy, RuntimeMonitorRegistryStatus,
    RuntimeOperation, RuntimeReceipt, RuntimeRequest, RuntimeResult, RuntimeSubscriptionRequest,
    TerminalEvent,
};
use serde::Serialize;
use std::fmt;
use std::fs;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_RUNTIME_IO_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_BACKEND_OPEN_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_RUNTIME_IO_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_BACKEND_OPEN_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_RUNTIME_INFO_BYTES: u64 = 64 * 1024;

/// Discovery, identity, framing, and timeout configuration for one local Runtime session.
#[derive(Clone)]
pub struct RuntimeClientConfig {
    state_root: PathBuf,
    actor: EventActor,
    source: EventSource,
    io_timeout: Duration,
    backend_open_timeout: Duration,
    maximum_frame_bytes: usize,
}

impl RuntimeClientConfig {
    pub fn new(state_root: impl Into<PathBuf>, actor: EventActor, source: EventSource) -> Self {
        Self {
            state_root: state_root.into(),
            actor,
            source,
            io_timeout: DEFAULT_RUNTIME_IO_TIMEOUT,
            backend_open_timeout: DEFAULT_BACKEND_OPEN_TIMEOUT,
            maximum_frame_bytes: DEFAULT_RUNTIME_MAX_FRAME_BYTES,
        }
    }

    pub fn with_io_timeout(mut self, io_timeout: Duration) -> Self {
        self.io_timeout = io_timeout;
        self
    }

    pub fn with_maximum_frame_bytes(mut self, maximum_frame_bytes: usize) -> Self {
        self.maximum_frame_bytes = maximum_frame_bytes;
        self
    }

    pub fn with_backend_open_timeout(mut self, backend_open_timeout: Duration) -> Self {
        self.backend_open_timeout = backend_open_timeout;
        self
    }

    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    fn validate(&self) -> RuntimeClientResult<()> {
        if self.state_root.as_os_str().is_empty()
            || self.io_timeout.is_zero()
            || self.io_timeout > MAX_RUNTIME_IO_TIMEOUT
            || self.backend_open_timeout.is_zero()
            || self.backend_open_timeout > MAX_BACKEND_OPEN_TIMEOUT
            || self.maximum_frame_bytes == 0
            || self.maximum_frame_bytes > DEFAULT_RUNTIME_MAX_FRAME_BYTES
        {
            return Err(RuntimeClientError::fatal(
                "runtime_client_config_invalid",
                "connect_runtime",
            ));
        }
        Ok(())
    }
}

impl fmt::Debug for RuntimeClientConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeClientConfig")
            .field("state_root", &"<redacted>")
            .field("actor", &self.actor)
            .field("source", &self.source)
            .field("io_timeout", &self.io_timeout)
            .field("backend_open_timeout", &self.backend_open_timeout)
            .field("maximum_frame_bytes", &self.maximum_frame_bytes)
            .finish()
    }
}

struct RuntimeConnection {
    stream: TcpStream,
    ids: IdentifierIssuer,
    actor: EventActor,
    source: EventSource,
    io_timeout: Duration,
    backend_open_timeout: Duration,
    maximum_frame_bytes: usize,
    terminal_error: Option<RuntimeClientError>,
}

struct RuntimeClientShared {
    info: RuntimeInfo,
    connection: Mutex<RuntimeConnection>,
}

/// Cloneable handle to one connection-bound Runtime IPC session.
#[derive(Clone)]
pub struct RuntimeClient {
    shared: Arc<RuntimeClientShared>,
}

/// Correlation-scoped authoring ingress. Runtime remains the only global-ledger writer.
#[derive(Clone)]
pub struct RuntimeAuthoringSession {
    client: RuntimeClient,
    correlation: IssuedCorrelationId,
}

/// Correlation-scoped Lab adapter for Runtime-owned capture, scheduling, input, and ledger facts.
#[derive(Clone)]
pub struct RuntimeDebugSession {
    client: RuntimeClient,
    correlation: IssuedCorrelationId,
}

/// Host receipt plus its correlation-scoped durable ledger projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFlowOutput {
    receipt: RuntimeReceipt,
    events: Vec<ProjectedEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseAdmission {
    Granted(LeaseToken),
    Queued(LeaseQueueStatus),
}

impl RuntimeFlowOutput {
    pub const fn receipt(&self) -> &RuntimeReceipt {
        &self.receipt
    }

    pub fn events(&self) -> &[ProjectedEvent] {
        &self.events
    }
}

impl RuntimeClient {
    pub fn connect(config: RuntimeClientConfig) -> RuntimeClientResult<Self> {
        config.validate()?;
        let info = read_runtime_info(config.state_root())?;
        let address = info
            .socket_addr()
            .map_err(|_| RuntimeClientError::fatal("runtime_info_invalid", "connect_runtime"))?;
        let stream = TcpStream::connect_timeout(&address, config.io_timeout)
            .map_err(|_| RuntimeClientError::fatal("runtime_connect_failed", "connect_runtime"))?;
        stream
            .set_read_timeout(Some(config.io_timeout))
            .map_err(|_| {
                RuntimeClientError::fatal("runtime_read_timeout_failed", "connect_runtime")
            })?;
        stream
            .set_write_timeout(Some(config.io_timeout))
            .map_err(|_| {
                RuntimeClientError::fatal("runtime_write_timeout_failed", "connect_runtime")
            })?;
        stream.set_nodelay(true).map_err(|_| {
            RuntimeClientError::fatal("runtime_tcp_nodelay_failed", "connect_runtime")
        })?;
        let client = Self {
            shared: Arc::new(RuntimeClientShared {
                info,
                connection: Mutex::new(RuntimeConnection {
                    stream,
                    ids: IdentifierIssuer::new().map_err(|_| {
                        RuntimeClientError::fatal(
                            "runtime_identifier_issuer_failed",
                            "connect_runtime",
                        )
                    })?,
                    actor: config.actor,
                    source: config.source,
                    io_timeout: config.io_timeout,
                    backend_open_timeout: config.backend_open_timeout,
                    maximum_frame_bytes: config.maximum_frame_bytes,
                    terminal_error: None,
                }),
            }),
        };
        let observed_epoch = client.health()?;
        if observed_epoch != client.shared.info.owner_epoch() {
            return Err(RuntimeClientError::fatal(
                "runtime_owner_epoch_changed",
                "connect_runtime",
            ));
        }
        Ok(client)
    }

    pub fn runtime_info(&self) -> &RuntimeInfo {
        &self.shared.info
    }

    pub fn health(&self) -> RuntimeClientResult<OwnerEpoch> {
        match self.execute("runtime_health", RuntimeOperation::Health)? {
            RuntimeResult::Health { owner_epoch } => Ok(owner_epoch),
            _ => Err(self.unexpected_result("runtime_health")),
        }
    }

    pub fn status(&self) -> RuntimeClientResult<RuntimeControlPlaneStatus> {
        match self.execute("runtime_status", RuntimeOperation::Status)? {
            RuntimeResult::Status { status } => Ok(status),
            _ => Err(self.unexpected_result("runtime_status")),
        }
    }

    pub fn monitor_status(&self) -> RuntimeClientResult<RuntimeMonitorRegistryStatus> {
        match self.execute("runtime_monitor_status", RuntimeOperation::MonitorStatus)? {
            RuntimeResult::MonitorStatus { status } => Ok(status),
            _ => Err(self.unexpected_result("runtime_monitor_status")),
        }
    }

    pub fn configure_monitor(
        &self,
        instance_alias: &str,
        policy: RuntimeMonitorPolicy,
    ) -> RuntimeClientResult<RuntimeMonitorInstanceStatus> {
        match self.execute(
            "runtime_monitor_configure",
            RuntimeOperation::ConfigureMonitor {
                instance_alias: instance_alias.to_string(),
                policy,
            },
        )? {
            RuntimeResult::MonitorConfigured { status } => Ok(status),
            _ => Err(self.unexpected_result("runtime_monitor_configure")),
        }
    }

    pub fn clear_monitor(
        &self,
        instance_alias: &str,
    ) -> RuntimeClientResult<RuntimeMonitorInstanceStatus> {
        match self.execute(
            "runtime_monitor_clear",
            RuntimeOperation::ClearMonitor {
                instance_alias: instance_alias.to_string(),
            },
        )? {
            RuntimeResult::MonitorCleared { status } => Ok(status),
            _ => Err(self.unexpected_result("runtime_monitor_clear")),
        }
    }

    pub fn acquire_lease(&self, instance_alias: &str) -> RuntimeClientResult<LeaseToken> {
        let holder = self
            .connection("issue_lease_holder")?
            .ids
            .mint_holder_id()
            .map_err(|_| {
                RuntimeClientError::fatal("runtime_identifier_issue_failed", "acquire_lease")
            })?;
        match self.execute(
            "acquire_lease",
            RuntimeOperation::acquire_lease(instance_alias, holder),
        )? {
            RuntimeResult::LeaseGranted { token } => Ok(token),
            _ => Err(self.unexpected_result("acquire_lease")),
        }
    }

    pub fn queue_lease(
        &self,
        instance_alias: &str,
        policy: LeaseQueuePolicy,
    ) -> RuntimeClientResult<LeaseAdmission> {
        let holder = self
            .connection("issue_queued_lease_holder")?
            .ids
            .mint_holder_id()
            .map_err(|_| {
                RuntimeClientError::fatal("runtime_identifier_issue_failed", "queue_lease")
            })?;
        match self.execute(
            "queue_lease",
            RuntimeOperation::queue_lease(instance_alias, holder, policy),
        )? {
            RuntimeResult::LeaseGranted { token } => Ok(LeaseAdmission::Granted(token)),
            RuntimeResult::LeaseQueued { status } => Ok(LeaseAdmission::Queued(status)),
            _ => Err(self.unexpected_result("queue_lease")),
        }
    }

    pub fn poll_queued_lease(
        &self,
        queued_request_id: RequestId,
    ) -> RuntimeClientResult<LeaseAdmission> {
        match self.execute(
            "poll_queued_lease",
            RuntimeOperation::PollQueuedLease { queued_request_id },
        )? {
            RuntimeResult::LeaseGranted { token } => Ok(LeaseAdmission::Granted(token)),
            RuntimeResult::LeasePending { status } => Ok(LeaseAdmission::Queued(status)),
            _ => Err(self.unexpected_result("poll_queued_lease")),
        }
    }

    pub fn cancel_queued_lease(&self, queued_request_id: RequestId) -> RuntimeClientResult<()> {
        match self.execute(
            "cancel_queued_lease",
            RuntimeOperation::CancelQueuedLease { queued_request_id },
        )? {
            RuntimeResult::LeaseQueueCancelled { request_id, .. }
                if request_id == queued_request_id =>
            {
                Ok(())
            }
            _ => Err(self.unexpected_result("cancel_queued_lease")),
        }
    }

    pub fn renew_lease(&self, token: &LeaseToken) -> RuntimeClientResult<LeaseToken> {
        match self.execute(
            "renew_lease",
            RuntimeOperation::RenewLease {
                token: token.clone(),
            },
        )? {
            RuntimeResult::LeaseRenewed { token } => Ok(token),
            _ => Err(self.unexpected_result("renew_lease")),
        }
    }

    pub fn release_lease(&self, token: &LeaseToken) -> RuntimeClientResult<()> {
        match self.execute(
            "release_lease",
            RuntimeOperation::ReleaseLease {
                token: token.clone(),
            },
        )? {
            RuntimeResult::LeaseReleased {
                instance_id,
                lease_id,
            } if instance_id == token.instance_id() && lease_id == token.lease_id() => Ok(()),
            _ => Err(self.unexpected_result("release_lease")),
        }
    }

    pub fn observe_readonly(&self, instance_alias: &str) -> RuntimeClientResult<RuntimeFlowOutput> {
        let correlation = self.issue_correlation("observe_readonly")?;
        let correlation_id = *correlation.transport();
        let receipt = self.execute_receipt_with_correlation(
            "observe_readonly",
            RuntimeOperation::ObserveReadonly {
                instance_alias: instance_alias.to_string(),
            },
            correlation,
            None,
        )?;
        if !matches!(
            receipt.result(),
            Some(RuntimeResult::ReadonlyObservationCompleted { .. })
        ) {
            return Err(self.unexpected_result("observe_readonly"));
        }
        self.flow_output(receipt, correlation_id)
    }

    pub fn capture_sequence(
        &self,
        instance_alias: &str,
        spec: CaptureSequenceSpec,
    ) -> RuntimeClientResult<RuntimeFlowOutput> {
        spec.validate().map_err(|_| {
            RuntimeClientError::fatal("runtime_capture_sequence_invalid", "capture_sequence")
        })?;
        let response_timeout = {
            let connection = self.connection("capture_sequence")?;
            capture_sequence_response_timeout(connection.backend_open_timeout, spec)?
        };
        let correlation = self.issue_correlation("capture_sequence")?;
        let correlation_id = *correlation.transport();
        let receipt = self.execute_receipt_with_correlation(
            "capture_sequence",
            RuntimeOperation::CaptureSequence {
                instance_alias: instance_alias.to_string(),
                spec,
            },
            correlation,
            Some(response_timeout),
        )?;
        if !matches!(
            receipt.result(),
            Some(RuntimeResult::CaptureSequenceCompleted { .. })
        ) {
            return Err(self.unexpected_result("capture_sequence"));
        }
        self.flow_output(receipt, correlation_id)
    }

    pub fn safe_reset(&self, instance_alias: &str) -> RuntimeClientResult<RuntimeFlowOutput> {
        let connection = self.connection("safe_reset")?;
        let correlation = connection.ids.mint_correlation_id().map_err(|_| {
            RuntimeClientError::fatal("runtime_identifier_issue_failed", "safe_reset")
        })?;
        let holder = connection.ids.mint_holder_id().map_err(|_| {
            RuntimeClientError::fatal("runtime_identifier_issue_failed", "safe_reset")
        })?;
        let correlation_id = *correlation.transport();
        drop(connection);
        let receipt = self.execute_receipt_with_correlation(
            "safe_reset",
            RuntimeOperation::safe_reset(instance_alias, holder),
            correlation,
            None,
        )?;
        if !matches!(
            receipt.result(),
            Some(RuntimeResult::SafeResetCompleted { .. })
        ) {
            return Err(self.unexpected_result("safe_reset"));
        }
        self.flow_output(receipt, correlation_id)
    }

    pub fn input(&self, token: &LeaseToken, action: InputAction) -> RuntimeClientResult<()> {
        let response_timeout = {
            let connection = self.connection("runtime_input")?;
            input_response_timeout(connection.io_timeout, &action)?
        };
        match self.execute_with_timeout(
            "runtime_input",
            RuntimeOperation::Input {
                token: token.clone(),
                action,
            },
            Some(response_timeout),
        )? {
            RuntimeResult::InputCommitted { .. } => Ok(()),
            _ => Err(self.unexpected_result("runtime_input")),
        }
    }

    pub fn query_events(
        &self,
        query: EventQuery,
        profile: ProjectionProfile,
    ) -> RuntimeClientResult<Vec<ProjectedEvent>> {
        match self.execute(
            "query_runtime_events",
            RuntimeOperation::QueryEvents { query, profile },
        )? {
            RuntimeResult::Events { events } => Ok(events),
            _ => Err(self.unexpected_result("query_runtime_events")),
        }
    }

    pub fn subscribe_events(
        &self,
        request: RuntimeSubscriptionRequest,
    ) -> RuntimeClientResult<RuntimeEventBatch> {
        request.validate().map_err(|_| {
            RuntimeClientError::fatal(
                "runtime_subscription_request_invalid",
                "subscribe_runtime_events",
            )
        })?;
        let response_timeout = self
            .connection("subscribe_runtime_events")?
            .io_timeout
            .checked_add(Duration::from_millis(request.wait_ms()))
            .ok_or_else(|| {
                RuntimeClientError::fatal(
                    "runtime_subscription_timeout_invalid",
                    "subscribe_runtime_events",
                )
            })?;
        match self.execute_with_timeout(
            "subscribe_runtime_events",
            RuntimeOperation::SubscribeEvents { request },
            Some(response_timeout),
        )? {
            RuntimeResult::EventBatch { batch } => Ok(batch),
            _ => Err(self.unexpected_result("subscribe_runtime_events")),
        }
    }

    pub fn begin_authoring_session(&self) -> RuntimeClientResult<RuntimeAuthoringSession> {
        let connection = self.connection("begin_resource_authoring")?;
        if connection.actor != EventActor::Lab || connection.source != EventSource::Lab {
            return Err(RuntimeClientError::fatal(
                "runtime_authoring_origin_invalid",
                "begin_resource_authoring",
            ));
        }
        let correlation = connection.ids.mint_correlation_id().map_err(|_| {
            RuntimeClientError::fatal(
                "runtime_identifier_issue_failed",
                "begin_resource_authoring",
            )
        })?;
        drop(connection);
        Ok(RuntimeAuthoringSession {
            client: self.clone(),
            correlation,
        })
    }

    pub fn begin_debug_session(&self) -> RuntimeClientResult<RuntimeDebugSession> {
        let connection = self.connection("begin_runtime_debug")?;
        if connection.actor != EventActor::Lab || connection.source != EventSource::Lab {
            return Err(RuntimeClientError::fatal(
                "runtime_debug_origin_invalid",
                "begin_runtime_debug",
            ));
        }
        let correlation = connection.ids.mint_correlation_id().map_err(|_| {
            RuntimeClientError::fatal("runtime_identifier_issue_failed", "begin_runtime_debug")
        })?;
        drop(connection);
        Ok(RuntimeDebugSession {
            client: self.clone(),
            correlation,
        })
    }

    fn execute(
        &self,
        operation_name: &'static str,
        operation: RuntimeOperation,
    ) -> RuntimeClientResult<RuntimeResult> {
        self.execute_with_timeout(operation_name, operation, None)
    }

    fn execute_with_timeout(
        &self,
        operation_name: &'static str,
        operation: RuntimeOperation,
        response_timeout: Option<Duration>,
    ) -> RuntimeClientResult<RuntimeResult> {
        let receipt = self.execute_receipt(operation_name, operation, response_timeout)?;
        let Some(result) = receipt.result().cloned() else {
            return Err(self.unexpected_result(operation_name));
        };
        Ok(result)
    }

    fn execute_receipt(
        &self,
        operation_name: &'static str,
        operation: RuntimeOperation,
        response_timeout: Option<Duration>,
    ) -> RuntimeClientResult<RuntimeReceipt> {
        let mut connection = self.connection(operation_name)?;
        let request = connection.request(operation_name, operation.clone())?;
        self.exchange_receipt(
            &mut connection,
            operation_name,
            operation,
            request,
            response_timeout,
        )
    }

    fn execute_receipt_with_correlation(
        &self,
        operation_name: &'static str,
        operation: RuntimeOperation,
        correlation: IssuedCorrelationId,
        response_timeout: Option<Duration>,
    ) -> RuntimeClientResult<RuntimeReceipt> {
        let mut connection = self.connection(operation_name)?;
        let request =
            connection.request_with_correlation(operation_name, operation.clone(), correlation)?;
        self.exchange_receipt(
            &mut connection,
            operation_name,
            operation,
            request,
            response_timeout,
        )
    }

    fn exchange_receipt(
        &self,
        connection: &mut RuntimeConnection,
        operation_name: &'static str,
        operation: RuntimeOperation,
        request: RuntimeRequest,
        response_timeout: Option<Duration>,
    ) -> RuntimeClientResult<RuntimeReceipt> {
        if let Some(error) = &connection.terminal_error {
            return Err(error.clone());
        }
        let response_timeout = response_timeout.unwrap_or(match &operation {
            RuntimeOperation::AcquireLease { .. } | RuntimeOperation::SafeReset { .. } => {
                connection.backend_open_timeout
            }
            _ => connection.io_timeout,
        });
        let maximum_frame_bytes = connection.maximum_frame_bytes;
        if connection
            .stream
            .set_read_timeout(Some(response_timeout))
            .is_err()
        {
            return Err(connection.latch(RuntimeClientError::fatal(
                "runtime_read_timeout_failed",
                operation_name,
            )));
        }
        let exchange_result =
            exchange::<_, RuntimeReceipt>(&mut connection.stream, &request, maximum_frame_bytes);
        if connection
            .stream
            .set_read_timeout(Some(connection.io_timeout))
            .is_err()
        {
            return Err(connection.latch(RuntimeClientError::fatal(
                "runtime_read_timeout_restore_failed",
                operation_name,
            )));
        }
        let receipt = match exchange_result {
            Ok(receipt) => receipt,
            Err(error) => return Err(connection.latch(error)),
        };
        if receipt.validate().is_err() {
            return Err(connection.latch(RuntimeClientError::fatal(
                "runtime_receipt_invalid",
                operation_name,
            )));
        }
        if receipt.request_id() != request.request_id()
            || receipt.correlation_id() != request.correlation_id()
        {
            return Err(connection.latch(RuntimeClientError::fatal(
                "runtime_receipt_identity_mismatch",
                operation_name,
            )));
        }
        if let Some(error) = receipt.error_projection() {
            let error = RuntimeClientError::rejected(operation_name, error.clone());
            return Err(if error.is_fatal() {
                connection.latch(error)
            } else {
                error
            });
        }
        if receipt.result().is_none() {
            return Err(connection.latch(RuntimeClientError::fatal(
                "runtime_result_missing",
                operation_name,
            )));
        }
        Ok(receipt)
    }

    fn issue_correlation(
        &self,
        operation: &'static str,
    ) -> RuntimeClientResult<IssuedCorrelationId> {
        self.connection(operation)?
            .ids
            .mint_correlation_id()
            .map_err(|_| RuntimeClientError::fatal("runtime_identifier_issue_failed", operation))
    }

    fn flow_output(
        &self,
        receipt: RuntimeReceipt,
        correlation_id: CorrelationId,
    ) -> RuntimeClientResult<RuntimeFlowOutput> {
        let events = self
            .query_events(
                EventQuery {
                    correlation_id: Some(correlation_id),
                    ..EventQuery::default()
                },
                ProjectionProfile::Forensic,
            )
            .map_err(|error| {
                RuntimeClientError::after_commit(
                    "runtime_projection_failed_after_terminal",
                    "query_runtime_flow_projection",
                    receipt.clone(),
                    error,
                )
            })?;
        Ok(RuntimeFlowOutput { receipt, events })
    }

    fn unexpected_result(&self, operation: &'static str) -> RuntimeClientError {
        let error = RuntimeClientError::fatal("runtime_result_unexpected", operation);
        match self.connection(operation) {
            Ok(mut connection) => connection.latch(error),
            Err(lock_error) => lock_error,
        }
    }

    fn connection(
        &self,
        operation: &'static str,
    ) -> RuntimeClientResult<MutexGuard<'_, RuntimeConnection>> {
        self.shared
            .connection
            .lock()
            .map_err(|_| RuntimeClientError::fatal("runtime_connection_poisoned", operation))
    }
}

impl RuntimeAuthoringSession {
    pub const fn correlation_id(&self) -> CorrelationId {
        *self.correlation.transport()
    }

    pub fn append(&self, event: ResourceAuthoringEvent) -> RuntimeClientResult<TerminalEvent> {
        event.validate().map_err(|_| {
            RuntimeClientError::fatal(
                "runtime_authoring_event_invalid",
                "record_resource_authoring_event",
            )
        })?;
        let expected_phase = event.phase();
        let receipt = self.client.execute_receipt_with_correlation(
            "record_resource_authoring_event",
            RuntimeOperation::RecordAuthoringEvent { event },
            self.correlation,
            None,
        )?;
        if !matches!(
            receipt.result(),
            Some(RuntimeResult::AuthoringEventRecorded { phase }) if *phase == expected_phase
        ) {
            return Err(self
                .client
                .unexpected_result("record_resource_authoring_event"));
        }
        receipt.terminal().ok_or_else(|| {
            self.client
                .unexpected_result("record_resource_authoring_event")
        })
    }

    pub fn query_events(
        &self,
        profile: ProjectionProfile,
    ) -> RuntimeClientResult<Vec<ProjectedEvent>> {
        self.client.query_events(
            EventQuery {
                correlation_id: Some(self.correlation_id()),
                ..EventQuery::default()
            },
            profile,
        )
    }
}

impl RuntimeDebugSession {
    pub const fn correlation_id(&self) -> CorrelationId {
        *self.correlation.transport()
    }

    pub fn debug_package(
        &self,
        request: PackageDebugRequest,
    ) -> RuntimeClientResult<RuntimeReceipt> {
        request.validate().map_err(|_| {
            RuntimeClientError::fatal("runtime_debug_package_invalid", "debug_package")
        })?;
        let timeout = self
            .client
            .connection("debug_package")?
            .backend_open_timeout;
        let receipt = self.client.execute_receipt_with_correlation(
            "debug_package",
            RuntimeOperation::DebugPackage { request },
            self.correlation,
            Some(timeout),
        )?;
        if !matches!(
            receipt.result(),
            Some(RuntimeResult::PackageDebugCompleted { .. })
        ) {
            return Err(self.client.unexpected_result("debug_package"));
        }
        Ok(receipt)
    }

    pub fn export_evidence(
        &self,
        request: RuntimeEvidenceExportRequest,
    ) -> RuntimeClientResult<RuntimeReceipt> {
        request.validate().map_err(|_| {
            RuntimeClientError::fatal("runtime_evidence_export_invalid", "export_evidence")
        })?;
        let timeout = self
            .client
            .connection("export_evidence")?
            .backend_open_timeout;
        let receipt = self.client.execute_receipt_with_correlation(
            "export_evidence",
            RuntimeOperation::ExportEvidence { request },
            self.correlation,
            Some(timeout),
        )?;
        if !matches!(
            receipt.result(),
            Some(RuntimeResult::EvidenceExportCompleted { .. })
        ) {
            return Err(self.client.unexpected_result("export_evidence"));
        }
        Ok(receipt)
    }

    pub fn observe_readonly(&self, instance_alias: &str) -> RuntimeClientResult<RuntimeReceipt> {
        let receipt = self.client.execute_receipt_with_correlation(
            "debug_observe_readonly",
            RuntimeOperation::ObserveReadonly {
                instance_alias: instance_alias.to_string(),
            },
            self.correlation,
            None,
        )?;
        if !matches!(
            receipt.result(),
            Some(RuntimeResult::ReadonlyObservationCompleted { .. })
        ) {
            return Err(self.client.unexpected_result("debug_observe_readonly"));
        }
        Ok(receipt)
    }

    pub fn capture_sequence(
        &self,
        instance_alias: &str,
        spec: CaptureSequenceSpec,
    ) -> RuntimeClientResult<RuntimeReceipt> {
        spec.validate().map_err(|_| {
            RuntimeClientError::fatal("runtime_capture_sequence_invalid", "debug_capture_sequence")
        })?;
        let response_timeout = {
            let connection = self.client.connection("debug_capture_sequence")?;
            capture_sequence_response_timeout(connection.backend_open_timeout, spec)?
        };
        let receipt = self.client.execute_receipt_with_correlation(
            "debug_capture_sequence",
            RuntimeOperation::CaptureSequence {
                instance_alias: instance_alias.to_string(),
                spec,
            },
            self.correlation,
            Some(response_timeout),
        )?;
        if !matches!(
            receipt.result(),
            Some(RuntimeResult::CaptureSequenceCompleted { .. })
        ) {
            return Err(self.client.unexpected_result("debug_capture_sequence"));
        }
        Ok(receipt)
    }

    pub fn acquire_lease(&self, instance_alias: &str) -> RuntimeClientResult<LeaseToken> {
        let connection = self.client.connection("debug_acquire_lease")?;
        let holder = connection.ids.mint_holder_id().map_err(|_| {
            RuntimeClientError::fatal("runtime_identifier_issue_failed", "debug_acquire_lease")
        })?;
        drop(connection);
        let receipt = self.client.execute_receipt_with_correlation(
            "debug_acquire_lease",
            RuntimeOperation::acquire_lease(instance_alias, holder),
            self.correlation,
            None,
        )?;
        match receipt.result() {
            Some(RuntimeResult::LeaseGranted { token }) => Ok(token.clone()),
            _ => Err(self.client.unexpected_result("debug_acquire_lease")),
        }
    }

    pub fn renew_lease(&self, token: &LeaseToken) -> RuntimeClientResult<LeaseToken> {
        let receipt = self.client.execute_receipt_with_correlation(
            "debug_renew_lease",
            RuntimeOperation::RenewLease {
                token: token.clone(),
            },
            self.correlation,
            None,
        )?;
        match receipt.result() {
            Some(RuntimeResult::LeaseRenewed { token }) => Ok(token.clone()),
            _ => Err(self.client.unexpected_result("debug_renew_lease")),
        }
    }

    pub fn input(&self, token: &LeaseToken, action: InputAction) -> RuntimeClientResult<ActionId> {
        let response_timeout = {
            let connection = self.client.connection("debug_runtime_input")?;
            input_response_timeout(connection.io_timeout, &action)?
        };
        let receipt = self.client.execute_receipt_with_correlation(
            "debug_runtime_input",
            RuntimeOperation::Input {
                token: token.clone(),
                action,
            },
            self.correlation,
            Some(response_timeout),
        )?;
        match receipt.result() {
            Some(RuntimeResult::InputCommitted { action_id }) => Ok(*action_id),
            _ => Err(self.client.unexpected_result("debug_runtime_input")),
        }
    }

    pub fn release_lease(&self, token: &LeaseToken) -> RuntimeClientResult<()> {
        let receipt = self.client.execute_receipt_with_correlation(
            "debug_release_lease",
            RuntimeOperation::ReleaseLease {
                token: token.clone(),
            },
            self.correlation,
            None,
        )?;
        match receipt.result() {
            Some(RuntimeResult::LeaseReleased {
                instance_id,
                lease_id,
            }) if *instance_id == token.instance_id() && *lease_id == token.lease_id() => Ok(()),
            _ => Err(self.client.unexpected_result("debug_release_lease")),
        }
    }

    pub fn record_event(&self, event: RuntimeDebugEvent) -> RuntimeClientResult<TerminalEvent> {
        event.validate().map_err(|_| {
            RuntimeClientError::fatal("runtime_debug_event_invalid", "record_runtime_debug_event")
        })?;
        let expected_phase = event.phase();
        let receipt = self.client.execute_receipt_with_correlation(
            "record_runtime_debug_event",
            RuntimeOperation::RecordDebugEvent { event },
            self.correlation,
            None,
        )?;
        if !matches!(
            receipt.result(),
            Some(RuntimeResult::DebugEventRecorded { phase }) if *phase == expected_phase
        ) {
            return Err(self.client.unexpected_result("record_runtime_debug_event"));
        }
        receipt
            .terminal()
            .ok_or_else(|| self.client.unexpected_result("record_runtime_debug_event"))
    }

    pub fn query_events(
        &self,
        profile: ProjectionProfile,
    ) -> RuntimeClientResult<Vec<ProjectedEvent>> {
        self.client.query_events(
            EventQuery {
                correlation_id: Some(self.correlation_id()),
                ..EventQuery::default()
            },
            profile,
        )
    }
}

impl fmt::Debug for RuntimeAuthoringSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeAuthoringSession")
            .field("correlation", &"<opaque-correlation>")
            .finish()
    }
}

impl fmt::Debug for RuntimeDebugSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeDebugSession")
            .field("correlation", &"<opaque-correlation>")
            .finish()
    }
}

impl fmt::Debug for RuntimeClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeClient")
            .field("runtime_info", &"<validated-loopback-runtime>")
            .finish()
    }
}

impl RuntimeConnection {
    fn latch(&mut self, error: RuntimeClientError) -> RuntimeClientError {
        if self.terminal_error.is_none() {
            self.terminal_error = Some(error);
        }
        self.terminal_error
            .clone()
            .expect("terminal error was set above")
    }

    fn request(
        &self,
        operation_name: &'static str,
        operation: RuntimeOperation,
    ) -> RuntimeClientResult<RuntimeRequest> {
        let correlation = self.ids.mint_correlation_id().map_err(|_| {
            RuntimeClientError::fatal("runtime_identifier_issue_failed", operation_name)
        })?;
        self.request_with_correlation(operation_name, operation, correlation)
    }

    fn request_with_correlation(
        &self,
        operation_name: &'static str,
        operation: RuntimeOperation,
        correlation: IssuedCorrelationId,
    ) -> RuntimeClientResult<RuntimeRequest> {
        RuntimeRequest::new(
            self.ids.mint_request_id().map_err(|_| {
                RuntimeClientError::fatal("runtime_identifier_issue_failed", operation_name)
            })?,
            correlation,
            None,
            self.actor,
            self.source,
            unix_ms_now()?,
            operation,
        )
        .map_err(|_| RuntimeClientError::fatal("runtime_request_invalid", operation_name))
    }
}

fn read_runtime_info(state_root: &Path) -> RuntimeClientResult<RuntimeInfo> {
    let path = state_root.join(RUNTIME_INFO_FILE);
    let metadata = fs::metadata(&path)
        .map_err(|_| RuntimeClientError::fatal("runtime_info_unavailable", "discover_runtime"))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_RUNTIME_INFO_BYTES {
        return Err(RuntimeClientError::fatal(
            "runtime_info_invalid",
            "discover_runtime",
        ));
    }
    let bytes = fs::read(path)
        .map_err(|_| RuntimeClientError::fatal("runtime_info_read_failed", "discover_runtime"))?;
    let info = serde_json::from_slice::<RuntimeInfo>(&bytes)
        .map_err(|_| RuntimeClientError::fatal("runtime_info_decode_failed", "discover_runtime"))?;
    info.validate()
        .map_err(|_| RuntimeClientError::fatal("runtime_info_invalid", "discover_runtime"))?;
    Ok(info)
}

fn unix_ms_now() -> RuntimeClientResult<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| RuntimeClientError::fatal("runtime_clock_invalid", "create_request"))?
        .as_millis();
    u64::try_from(millis)
        .map_err(|_| RuntimeClientError::fatal("runtime_clock_overflow", "create_request"))
}

fn input_response_timeout(
    io_timeout: Duration,
    action: &InputAction,
) -> RuntimeClientResult<Duration> {
    let duration_ms = match action {
        InputAction::LongTap { duration_ms, .. } | InputAction::Swipe { duration_ms, .. } => {
            *duration_ms
        }
        _ => 0,
    };
    io_timeout
        .checked_add(Duration::from_millis(duration_ms))
        .ok_or_else(|| RuntimeClientError::fatal("runtime_input_timeout_overflow", "runtime_input"))
}

fn capture_sequence_response_timeout(
    backend_open_timeout: Duration,
    spec: CaptureSequenceSpec,
) -> RuntimeClientResult<Duration> {
    let planned_wait_ms = spec.planned_wait_ms().map_err(|_| {
        RuntimeClientError::fatal("runtime_capture_sequence_invalid", "capture_sequence")
    })?;
    backend_open_timeout
        .checked_add(Duration::from_millis(planned_wait_ms))
        .ok_or_else(|| {
            RuntimeClientError::fatal(
                "runtime_capture_sequence_timeout_overflow",
                "capture_sequence",
            )
        })
}
