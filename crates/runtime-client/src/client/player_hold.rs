// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #429 R-7a (register §1.233, since 0.12.35; `runtime-client` 4): the player hold's
//! client methods (contracts/player-hold.md, "Lost receipts"; design R-7a §3.3.2).
//!
//! Each request is prepared, which fixes its identity and writes nothing, then submitted once on
//! the handle that prepared it. When the receipt never arrives after the frame was written (the
//! connection ended before the receipt header, or the receipt-read deadline passed), the method
//! settles the request itself: it reads `Status` once on a new connection and calls the
//! contract's `settle_player_hold`. It never sends a request again and never changes it. Every
//! other failure after the frame was sent is returned as it is, unsettled.

use super::*;
use actingcommand_contract::{
    PlayerHoldAcquired, PlayerHoldAttempt, PlayerHoldExpectation, PlayerHoldOperationKind,
    PlayerHoldReleased, PlayerHoldSettlementTrigger, PlayerHoldSettlementVerdict,
    PlayerHoldStartupChoice, settle_player_hold,
};

/// One player hold request whose identity exists before its frame is written, as
/// `PreparedContainedTask` (CL:338-363). Only the handle that prepared it submits it, once.
struct PreparedPlayerHold {
    shared: Arc<RuntimeClientShared>,
    selected_correlation: Option<CorrelationId>,
    instance_alias: String,
    kind: PlayerHoldOperationKind,
    expected_hold_id: RequestId,
    runtime_request: RuntimeRequest,
}

impl PreparedPlayerHold {
    fn owner_epoch(&self) -> OwnerEpoch {
        self.shared.info.owner_epoch()
    }

    fn attempt(&self, trigger: PlayerHoldSettlementTrigger) -> PlayerHoldAttempt {
        PlayerHoldAttempt {
            kind: self.kind,
            request_id: self.runtime_request.request_id(),
            sent_in_owner_epoch: self.owner_epoch(),
            expected_hold_id: self.expected_hold_id,
            trigger,
        }
    }

    const fn operation(&self) -> Location {
        match self.kind {
            PlayerHoldOperationKind::Acquire => ContractLocation::AcquirePlayerHold.location(),
            PlayerHoldOperationKind::Release => ContractLocation::ReleasePlayerHold.location(),
        }
    }
}

/// A prepared `AcquirePlayerHold` (R-7a §3.3.2): its `request_id` is the future `hold_id`.
#[must_use = "a prepared player hold is sent only through submit_acquire_player_hold"]
pub struct PreparedPlayerHoldAcquire(PreparedPlayerHold);

/// A prepared `ReleasePlayerHold` (R-7a §3.3.2).
#[must_use = "a prepared player hold release is sent only through submit_release_player_hold"]
pub struct PreparedPlayerHoldRelease(PreparedPlayerHold);

macro_rules! prepared_accessors {
    ($name:ident) => {
        impl $name {
            /// The request's `request_id`, readable before it is submitted.
            pub fn request_id(&self) -> RequestId {
                self.0.runtime_request.request_id()
            }

            /// The owner epoch of the handle that prepared it, the epoch it is sent in.
            pub fn owner_epoch(&self) -> OwnerEpoch {
                self.0.owner_epoch()
            }

            /// The attempt `settle_player_hold` judges. A console stores `attempt(reopened)`
            /// before it submits, so it can settle after it reopened.
            pub fn attempt(&self, trigger: PlayerHoldSettlementTrigger) -> PlayerHoldAttempt {
                self.0.attempt(trigger)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_struct(stringify!($name))
                    .field("request_id", &self.request_id())
                    .field("owner_epoch", &self.owner_epoch())
                    .finish()
            }
        }
    };
}

prepared_accessors!(PreparedPlayerHoldAcquire);
prepared_accessors!(PreparedPlayerHoldRelease);

/// The receipt waits of the two player hold requests (R-7a §3.1, §3.2): the socket read timeout
/// is the whole wait formula, the receipt-read deadline the formula without the IO margin, so a
/// socket timeout always finds the deadline passed (`runtime_receipt_timeout`). `None` for any
/// other request.
pub(crate) struct PlayerHoldReceiptWaits {
    pub(crate) socket: Duration,
    pub(crate) deadline: Duration,
}

pub(crate) fn player_hold_receipt_waits(
    operation: &RuntimeOperation,
    io_timeout: Duration,
) -> Option<RuntimeClientResult<PlayerHoldReceiptWaits>> {
    let (deadline, location) = match operation {
        RuntimeOperation::AcquirePlayerHold {
            drain_timeout_ms, ..
        } => (
            actingcommand_contract::player_hold_acquire_receipt_deadline(*drain_timeout_ms),
            ContractLocation::AcquirePlayerHold.location(),
        ),
        RuntimeOperation::ReleasePlayerHold { .. } => (
            Some(actingcommand_contract::player_hold_release_receipt_deadline()),
            ContractLocation::ReleasePlayerHold.location(),
        ),
        _ => return None,
    };
    #[cfg(test)]
    let deadline = tests_override::deadline().or(deadline);
    Some(
        deadline
            .and_then(|deadline| {
                deadline
                    .checked_add(io_timeout)
                    .map(|socket| PlayerHoldReceiptWaits { socket, deadline })
            })
            .ok_or_else(|| {
                RuntimeClientError::fatal(ClientCode::RuntimeReceiptTimeoutOverflow, location)
            }),
    )
}

impl RuntimeClient {
    /// Fixes one `AcquirePlayerHold`'s request and correlation on this handle without writing
    /// anything (R-7a §3.3.2). Its `request_id` is the hold's `hold_id`.
    pub fn prepare_acquire_player_hold(
        &self,
        instance_alias: &str,
        drain_timeout_ms: u64,
    ) -> RuntimeClientResult<PreparedPlayerHoldAcquire> {
        let prepared = self.prepare_player_hold(
            instance_alias,
            PlayerHoldOperationKind::Acquire,
            None,
            RuntimeOperation::AcquirePlayerHold {
                instance_alias: instance_alias.to_owned(),
                drain_timeout_ms,
            },
        )?;
        Ok(PreparedPlayerHoldAcquire(prepared))
    }

    /// Fixes one `ReleasePlayerHold`'s request on this handle without writing anything.
    pub fn prepare_release_player_hold(
        &self,
        instance_alias: &str,
        expected: PlayerHoldExpectation,
        startup_package: Option<PlayerHoldStartupChoice>,
    ) -> RuntimeClientResult<PreparedPlayerHoldRelease> {
        let prepared = self.prepare_player_hold(
            instance_alias,
            PlayerHoldOperationKind::Release,
            Some(expected.hold_id),
            RuntimeOperation::ReleasePlayerHold {
                instance_alias: instance_alias.to_owned(),
                expected,
                startup_package,
            },
        )?;
        Ok(PreparedPlayerHoldRelease(prepared))
    }

    /// Writes one prepared `AcquirePlayerHold` once and waits for its receipt (R-7a §3.1). A lost
    /// receipt is settled from `Status`: `runtime_player_hold_settled`,
    /// `runtime_player_hold_unconfirmed` or `runtime_operation_unsupported`, each with
    /// `player_hold_settlement()` and `player_hold_attempt()`.
    pub fn submit_acquire_player_hold(
        &self,
        prepared: PreparedPlayerHoldAcquire,
    ) -> RuntimeClientResult<PlayerHoldAcquired> {
        let PreparedPlayerHoldAcquire(prepared) = prepared;
        let operation = prepared.operation();
        let hold_id = prepared.runtime_request.request_id();
        match self.submit_player_hold(prepared)? {
            // The hold's id is the acquire's own request id (R-7a §2.4).
            RuntimeResult::PlayerHoldAcquired(acquired) if acquired.hold.hold_id == hold_id => {
                Ok(*acquired)
            }
            _ => Err(self.unexpected_result(operation)),
        }
    }

    /// Writes one prepared `ReleasePlayerHold` once and waits for its receipt (R-7a §3.2), with
    /// the settlement of a lost receipt as `submit_acquire_player_hold`.
    pub fn submit_release_player_hold(
        &self,
        prepared: PreparedPlayerHoldRelease,
    ) -> RuntimeClientResult<PlayerHoldReleased> {
        let PreparedPlayerHoldRelease(prepared) = prepared;
        let operation = prepared.operation();
        let alias = prepared.instance_alias.clone();
        let hold_id = prepared.expected_hold_id;
        match self.submit_player_hold(prepared)? {
            RuntimeResult::PlayerHoldReleased(released)
                if released.instance_alias == alias && released.hold_id == hold_id =>
            {
                Ok(*released)
            }
            _ => Err(self.unexpected_result(operation)),
        }
    }

    /// `prepare_acquire_player_hold`, then `submit_acquire_player_hold`.
    pub fn acquire_player_hold(
        &self,
        instance_alias: &str,
        drain_timeout_ms: u64,
    ) -> RuntimeClientResult<PlayerHoldAcquired> {
        let prepared = self.prepare_acquire_player_hold(instance_alias, drain_timeout_ms)?;
        self.submit_acquire_player_hold(prepared)
    }

    /// `prepare_release_player_hold`, then `submit_release_player_hold`.
    pub fn release_player_hold(
        &self,
        instance_alias: &str,
        expected: PlayerHoldExpectation,
        startup_package: Option<PlayerHoldStartupChoice>,
    ) -> RuntimeClientResult<PlayerHoldReleased> {
        let prepared =
            self.prepare_release_player_hold(instance_alias, expected, startup_package)?;
        self.submit_release_player_hold(prepared)
    }

    fn prepare_player_hold(
        &self,
        instance_alias: &str,
        kind: PlayerHoldOperationKind,
        expected_hold_id: Option<RequestId>,
        operation: RuntimeOperation,
    ) -> RuntimeClientResult<PreparedPlayerHold> {
        let location = match kind {
            PlayerHoldOperationKind::Acquire => ContractLocation::AcquirePlayerHold.location(),
            PlayerHoldOperationKind::Release => ContractLocation::ReleasePlayerHold.location(),
        };
        let connection = self.connection(location)?;
        let runtime_request = connection.request(location, operation, self.correlation)?;
        drop(connection);
        Ok(PreparedPlayerHold {
            shared: Arc::clone(&self.shared),
            selected_correlation: self.correlation_id(),
            instance_alias: instance_alias.to_owned(),
            kind,
            expected_hold_id: expected_hold_id.unwrap_or_else(|| runtime_request.request_id()),
            runtime_request,
        })
    }

    /// Writes the prepared request once and reads its receipt; a lost receipt is settled
    /// (R-7a §3.3.2 "when to settle"): the connection ended before the receipt header of this
    /// frame (`dropped`) or the receipt-read deadline passed (`timed_out`). Every other failure is
    /// returned as it is.
    fn submit_player_hold(
        &self,
        prepared: PreparedPlayerHold,
    ) -> RuntimeClientResult<RuntimeResult> {
        let operation_name = prepared.operation();
        if !Arc::ptr_eq(&self.shared, &prepared.shared)
            || self.correlation_id() != prepared.selected_correlation
        {
            return Err(RuntimeClientError::fatal(
                ClientCode::RuntimePlayerHoldPreparedClientMismatch,
                operation_name,
            ));
        }
        let request = prepared.runtime_request.clone();
        let request_id = request.request_id();
        let exchanged = {
            let mut connection = self.connection(operation_name)?;
            // A connection an earlier call latched fails before anything is written: that is no
            // lost receipt of this request.
            if let Some(error) = &connection.terminal_error {
                return Err(error.clone());
            }
            self.exchange_receipt(
                &mut connection,
                operation_name,
                request.operation().clone(),
                request,
                None,
            )
        };
        let trigger = match &exchanged {
            Ok(_) => None,
            Err(error) if dropped_before_receipt(error, request_id) => {
                Some(PlayerHoldSettlementTrigger::Dropped)
            }
            Err(error)
                if error.projection().is_none()
                    && error.code() == ClientCode::RuntimeReceiptTimeout.code() =>
            {
                Some(PlayerHoldSettlementTrigger::TimedOut)
            }
            Err(_) => None,
        };
        match (exchanged, trigger) {
            (Ok(receipt), _) => receipt
                .result()
                .cloned()
                .ok_or_else(|| self.unexpected_result(operation_name)),
            (Err(error), Some(trigger)) => {
                Err(self.settle_lost_player_hold(&prepared, prepared.attempt(trigger), error))
            }
            (Err(error), None) => Err(error),
        }
    }

    /// R-7a §3.3.2 "how the method settles": read `Status` once on a new connection, settle, and
    /// leave the handle as the post-condition table says: in the same owner epoch the new
    /// connection is in place and usable; in another epoch the handle stays latched
    /// `runtime_owner_epoch_changed` and a temporary client read `Status`; when nothing could be
    /// read the handle stays latched with that failure (P1).
    fn settle_lost_player_hold(
        &self,
        prepared: &PreparedPlayerHold,
        attempt: PlayerHoldAttempt,
        original: RuntimeClientError,
    ) -> RuntimeClientError {
        let operation = prepared.operation();
        let mut related = vec![original];
        let status = match self.status_after_lost_receipt(operation, attempt.trigger) {
            Ok((status, epoch_changed)) => {
                related.extend(epoch_changed);
                Some(status)
            }
            Err(failure) => {
                related.push(failure);
                None
            }
        };
        let settlement =
            settle_player_hold(Some(&attempt), &prepared.instance_alias, status.as_ref());
        let code = match settlement.decision().map(|decision| decision.verdict) {
            Some(PlayerHoldSettlementVerdict::Settled) => ClientCode::RuntimePlayerHoldSettled,
            Some(PlayerHoldSettlementVerdict::Unsupported) => {
                ClientCode::RuntimeOperationUnsupported
            }
            Some(PlayerHoldSettlementVerdict::Unconfirmed) | None => {
                ClientCode::RuntimePlayerHoldUnconfirmed
            }
        };
        let mut error = RuntimeClientError::fatal(code, operation);
        for failure in related {
            error = error.with_related(failure);
        }
        error.with_player_hold(settlement, attempt)
    }

    /// One `Status` read after a lost receipt, and the epoch change when there was one. The
    /// timed-out connection is shut down first, so a late receipt never mixes in.
    fn status_after_lost_receipt(
        &self,
        operation: Location,
        trigger: PlayerHoldSettlementTrigger,
    ) -> RuntimeClientResult<(RuntimeControlPlaneStatus, Option<RuntimeClientError>)> {
        if trigger == PlayerHoldSettlementTrigger::TimedOut {
            let mut connection = self.connection(operation)?;
            if let Err(error) = connection.stream.shutdown(Shutdown::Both)
                && error.kind() != std::io::ErrorKind::NotConnected
            {
                // The timed-out connection could not be closed: read nothing on this handle.
                let failure =
                    RuntimeClientError::fatal(ClientCode::RuntimeReceiptReadFailed, operation);
                connection.terminal_error = Some(failure.clone());
                return Err(failure);
            }
        }
        let info = read_runtime_info(&self.shared.state_root)
            .map_err(|failure| self.latch_with(operation, failure))?;
        if info.owner_epoch() == self.shared.info.owner_epoch() {
            match self
                .reconnect_in_owner_epoch(operation)
                .and_then(|()| self.status())
            {
                Ok(status) => return Ok((status, None)),
                Err(failure) if failure.code() != ClientCode::RuntimeOwnerEpochChanged.code() => {
                    return Err(self.latch_with(operation, failure));
                }
                // The handshake met another epoch: read through a temporary client below.
                Err(_) => {}
            }
        }
        let epoch_changed = self.latch_with(
            operation,
            RuntimeClientError::fatal(ClientCode::RuntimeOwnerEpochChanged, operation),
        );
        let config = {
            let connection = self.connection(operation)?;
            RuntimeClientConfig {
                state_root: self.shared.state_root.clone(),
                channel: connection.channel.channel,
                via: connection.channel.via,
                actor: connection.actor,
                source: connection.source,
                io_timeout: connection.io_timeout,
                backend_open_timeout: connection.backend_open_timeout,
                maximum_frame_bytes: connection.maximum_frame_bytes,
                deadline: connection.deadline,
                detail: connection.detail,
            }
        };
        let temporary = RuntimeClient::connect(config)
            .map_err(|failure| failure.with_related(epoch_changed.clone()))?;
        let status = temporary
            .status()
            .map_err(|failure| failure.with_related(epoch_changed.clone()))?;
        drop(temporary);
        Ok((status, Some(epoch_changed)))
    }

    /// Latches this handle with `failure`, replacing a latch an earlier failure left (the
    /// settlement's post-condition: the handle's error is this read's).
    fn latch_with(&self, operation: Location, failure: RuntimeClientError) -> RuntimeClientError {
        if let Ok(mut connection) = self.connection(operation) {
            connection.terminal_error = Some(failure.clone());
        }
        failure
    }

    /// Test seam (R-7a §3.1): replaces this handle's session deadline after its handshake, so a
    /// gate runs a request under a session deadline already reached without reading the clock,
    /// and the connect itself is not cut.
    #[cfg(test)]
    pub(crate) fn set_session_deadline_for_test(&self, deadline: Option<Instant>) {
        self.shared
            .connection
            .lock()
            .expect("the session's connection")
            .deadline = deadline;
    }
}

/// Test seam: a receipt-read deadline shorter than the contract's, for this thread, so the
/// timeout paths run in milliseconds. The socket timeout stays the deadline plus the IO margin.
/// Last in the file: no item follows a test module.
#[cfg(test)]
pub(crate) mod tests_override {
    use std::cell::Cell;
    use std::time::Duration;

    thread_local! {
        static DEADLINE: Cell<Option<Duration>> = const { Cell::new(None) };
    }

    pub(crate) fn set(deadline: Option<Duration>) {
        DEADLINE.with(|cell| cell.set(deadline));
    }

    pub(crate) fn deadline() -> Option<Duration> {
        DEADLINE.with(Cell::get)
    }
}
