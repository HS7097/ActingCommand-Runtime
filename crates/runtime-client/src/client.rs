// SPDX-License-Identifier: AGPL-3.0-only

use crate::ipc::{DEFAULT_RUNTIME_MAX_FRAME_BYTES, exchange};
use crate::{RuntimeClientError, RuntimeClientResult};
use actingcommand_contract::{
    EventActor, EventQuery, EventSource, IdentifierIssuer, InputAction, LeaseToken, OwnerEpoch,
    ProjectedEvent, ProjectionProfile, RUNTIME_INFO_FILE, ReadOnlyCaptureCapability, RuntimeInfo,
    RuntimeOperation, RuntimeReceipt, RuntimeRequest, RuntimeResult,
};
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

    pub fn admit_readonly(
        &self,
        instance_alias: &str,
    ) -> RuntimeClientResult<ReadOnlyCaptureCapability> {
        match self.execute(
            "admit_readonly",
            RuntimeOperation::AdmitReadonly {
                instance_alias: instance_alias.to_string(),
            },
        )? {
            RuntimeResult::ReadOnlyAdmitted { capability } => Ok(capability),
            _ => Err(self.unexpected_result("admit_readonly")),
        }
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
        let mut connection = self.connection(operation_name)?;
        if let Some(error) = &connection.terminal_error {
            return Err(error.clone());
        }
        let response_timeout = response_timeout.unwrap_or_else(|| match &operation {
            RuntimeOperation::AcquireLease { .. } => connection.backend_open_timeout,
            _ => connection.io_timeout,
        });
        let request = connection.request(operation_name, operation)?;
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
        let Some(result) = receipt.result().cloned() else {
            return Err(connection.latch(RuntimeClientError::fatal(
                "runtime_result_missing",
                operation_name,
            )));
        };
        Ok(result)
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
        RuntimeRequest::new(
            self.ids.mint_request_id().map_err(|_| {
                RuntimeClientError::fatal("runtime_identifier_issue_failed", operation_name)
            })?,
            self.ids.mint_correlation_id().map_err(|_| {
                RuntimeClientError::fatal("runtime_identifier_issue_failed", operation_name)
            })?,
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
