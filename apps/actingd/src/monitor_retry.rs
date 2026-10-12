// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #397 (MON-1): the policy monitor's answer when a request of its own to its Runtime
//! fails. The monitor loop (`monitor_policy` in `main.rs`) hands every client failure that the
//! #376 refusal table leaves to it to [`MonitorRetries::failed`].
//!
//! A transient I/O failure is a receipt header that did not arrive within the I/O timeout
//! (`runtime_receipt_header_failed` with `TimedOut`, or `WouldBlock`, the same read timeout on
//! other platforms) of a request sent to this Runtime. Up to `policy.monitor.retry_limit` of them
//! in a row are retried on a fresh connection after a growing wait, each reported as the warning
//! `policy_monitor_request_retried`; the next one is reported as
//! `policy_monitor_retries_exhausted` and ends actingd with the request's failure. Every other
//! failure, and a request that went to another owner epoch or process, ends actingd at once.
//!
//! Workflow #383 AO-7: the policy monitor now wakes the driver from the host's business
//! notifications and sends no request to its Runtime, so nothing outside the tests (`gate_397`)
//! calls the retry. `policy.monitor` is still read, checked and reported; retiring it with this
//! retry, its codes and its tests is left to the coordinator (ORD 376-B2).
#![cfg_attr(not(test), allow(dead_code))]

use crate::codes::ActingdCode;
use actingcommand_contract::outcome::{IoKind, Location, Outcome};
use actingcommand_contract::{OwnerEpoch, RuntimeInfo};
use actingcommand_runtime_client::{ClientCode, RuntimeClientError};
use std::io::ErrorKind;
use std::time::Duration;

/// `policy.monitor.io_timeout_ms` default. The subscription is answered at once (`wait_ms` 0), so
/// the timeout only matters while the host is slow to answer: 5 s (the client default, v0.11.4
/// flaky-tests report B1) did not cover a host at 100 % CPU on F:\AC (#397).
pub(super) const DEFAULT_IO_TIMEOUT_MS: u64 = 15_000;
/// `policy.monitor.retry_limit` default.
pub(super) const DEFAULT_RETRY_LIMIT: u16 = 5;
/// `policy.monitor.retry_backoff_ms` default.
pub(super) const DEFAULT_RETRY_BACKOFF_MS: u64 = 1_000;
/// `policy.monitor.retry_backoff_max_ms` default.
pub(super) const DEFAULT_RETRY_BACKOFF_MAX_MS: u64 = 30_000;

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

impl Default for PolicyMonitorSettings {
    fn default() -> Self {
        Self {
            io_timeout: Duration::from_millis(DEFAULT_IO_TIMEOUT_MS),
            retry_limit: DEFAULT_RETRY_LIMIT,
            retry_backoff: Duration::from_millis(DEFAULT_RETRY_BACKOFF_MS),
            retry_backoff_max: Duration::from_millis(DEFAULT_RETRY_BACKOFF_MAX_MS),
        }
    }
}

impl PolicyMonitorSettings {
    /// The wait after the `attempt`-th failure in a row (1-based): `retry_backoff`, doubled for
    /// each earlier failure in the row, at most `retry_backoff_max`.
    pub(super) fn retry_backoff_after(&self, attempt: u32) -> Duration {
        let doublings = attempt.saturating_sub(1).min(31);
        self.retry_backoff
            .checked_mul(1_u32 << doublings)
            .map_or(self.retry_backoff_max, |backoff| {
                backoff.min(self.retry_backoff_max)
            })
    }
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

    /// The answer to a failed request of the monitor that ran in `runtime`.
    pub(super) fn failed(
        &mut self,
        failure: &MonitorRequestFailure,
        runtime: RuntimeIdentity,
    ) -> MonitorRetryStep {
        let Some(header) = failure.receipt_header.filter(|header| {
            matches!(header.kind, ErrorKind::TimedOut | ErrorKind::WouldBlock)
                && header.sent_to == Some(runtime)
        }) else {
            return MonitorRetryStep::Fail;
        };
        self.consecutive = self.consecutive.saturating_add(1);
        let outcome = |code: ActingdCode| {
            let outcome = Outcome::new(code)
                .operation(failure.operation)
                .attempts(self.consecutive)
                .limit_count(self.settings.retry_limit)
                .io_kind(IoKind::from_error_kind(header.kind));
            match header.os_error {
                Some(os_error) => outcome.os_error(os_error),
                None => outcome,
            }
        };
        if self.consecutive > u32::from(self.settings.retry_limit) {
            return MonitorRetryStep::GiveUp {
                outcome: outcome(ActingdCode::PolicyMonitorRetriesExhausted),
            };
        }
        let backoff = self.settings.retry_backoff_after(self.consecutive);
        MonitorRetryStep::Retry {
            warning: outcome(ActingdCode::PolicyMonitorRequestRetried).retry_after_ms(backoff),
            backoff,
        }
    }
}
