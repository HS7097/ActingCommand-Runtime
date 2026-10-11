// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #397 (MON-1): the policy monitor's answer when a request of its own to its Runtime
//! fails. The monitor loop (`monitor_policy` in `main.rs`) hands every client failure that the
//! #376 refusal table leaves to it to [`MonitorRetries::failed`].
//!
//! This is the answer the monitor gives today: every such failure ends actingd
//! (`PolicySubscriptionFailureAction::Fail`), however transient.

// The seam the #397 gate test drives; the fix wires it into the monitor loop.
#![allow(dead_code)]

use actingcommand_contract::outcome::{Location, Outcome};
use actingcommand_contract::{OwnerEpoch, RuntimeInfo};
use actingcommand_runtime_client::{ClientCode, RuntimeClientError};
use std::io::ErrorKind;
use std::time::Duration;

/// `policy.monitor`: the policy monitor's connection to its own Runtime and how it retries a
/// transient failure of one of its requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PolicyMonitorSettings {
    /// The I/O timeout of the monitor's connection: connect, request write, receipt read.
    pub(super) io_timeout: Duration,
    /// Transient failures in a row the monitor retries; the next one ends actingd.
    pub(super) retry_limit: u16,
    /// The wait before the first retry.
    pub(super) retry_backoff: Duration,
    /// The longest wait between two retries.
    pub(super) retry_backoff_max: Duration,
}

/// The Runtime a request went to: its owner epoch and process id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RuntimeIdentity {
    pub(super) owner_epoch: OwnerEpoch,
    pub(super) pid: u32,
}

impl RuntimeIdentity {
    pub(super) fn of(info: &RuntimeInfo) -> Self {
        Self {
            owner_epoch: info.owner_epoch(),
            pid: info.pid(),
        }
    }
}

/// What the monitor's answer reads from one failed request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MonitorRequestFailure {
    /// The client operation that failed (`exchange_runtime_request` for a receipt read).
    pub(super) operation: Location,
    /// The failed receipt-header read, when the request failed reading its receipt header.
    pub(super) receipt_header: Option<ReceiptHeaderFailure>,
}

/// The I/O facts of a failed receipt-header read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ReceiptHeaderFailure {
    pub(super) kind: ErrorKind,
    pub(super) os_error: Option<i32>,
    /// The Runtime the request was sent to, as the client knew it.
    pub(super) sent_to: Option<RuntimeIdentity>,
}

impl MonitorRequestFailure {
    pub(super) fn of(error: &RuntimeClientError) -> Self {
        let receipt_header = error
            .receipt_header_io()
            .filter(|_| error.code() == ClientCode::RuntimeReceiptHeaderFailed.code())
            .map(|io| ReceiptHeaderFailure {
                kind: io.kind(),
                os_error: io.raw_os_error(),
                sent_to: io
                    .expected_owner_epoch()
                    .zip(io.expected_runtime_pid())
                    .map(|(owner_epoch, pid)| RuntimeIdentity {
                        owner_epoch: *owner_epoch,
                        pid,
                    }),
            });
        Self {
            operation: error.operation(),
            receipt_header,
        }
    }
}

/// What the monitor does after one of its own requests failed.
#[derive(Debug)]
pub(super) enum MonitorRetryStep {
    /// Report the warning, wait `backoff`, then send the request again on a fresh connection.
    Retry { warning: Outcome, backoff: Duration },
    /// Report the give-up, then end actingd with the request's failure.
    GiveUp { outcome: Outcome },
    /// End actingd with the request's failure.
    Fail,
}

/// The monitor's failed requests in a row.
#[derive(Debug)]
pub(super) struct MonitorRetries {
    settings: PolicyMonitorSettings,
    /// Transient failures since the last request that succeeded.
    consecutive: u32,
}

impl MonitorRetries {
    pub(super) const fn new(settings: PolicyMonitorSettings) -> Self {
        Self {
            settings,
            consecutive: 0,
        }
    }

    /// A request succeeded: the next failure is the first again.
    pub(super) fn succeeded(&mut self) {
        self.consecutive = 0;
    }

    pub(super) fn failed(
        &mut self,
        _failure: &MonitorRequestFailure,
        _runtime: RuntimeIdentity,
    ) -> MonitorRetryStep {
        MonitorRetryStep::Fail
    }
}
