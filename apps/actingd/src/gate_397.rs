// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #397 (MON-1), test plan gap G1 (inventory key 4, "Policy monitor"): a transient I/O
//! failure of the policy monitor's own request to its Runtime is retried with a warning, not
//! fatal.
//!
//! Real-device failure, F:\AC 2026-10-11 20:12 JST (Runtime ceef6f90): with the host at 100 % CPU
//! the receipt header of one subscription did not arrive within the monitor's 5 s I/O timeout
//! (`runtime_receipt_header_failed` during `exchange_runtime_request`, `TimedOut`, OS error
//! 10060). actingd ended as fatal (ledger seq 269802 `runtime.failed`, 269803
//! `shutdown_requested`) and the watchdog held the Runtime down until a manual start.
//!
//! The scripted answers are what the Runtime client returns to the monitor's subscription; the
//! monitor's answer to them is driven as its loop drives it: a batch resets the count, a failure
//! is retried, given up or ends the monitor.

use super::monitor_retry::{
    MonitorRequestFailure, MonitorRetries, MonitorRetryStep, PolicyMonitorSettings,
    ReceiptHeaderFailure, RuntimeIdentity,
};
use actingcommand_contract::IdentifierIssuer;
use actingcommand_contract::outcome::{Category, Outcome};
use actingcommand_runtime_client::ClientLocation;
use serde_json::json;
use std::io::ErrorKind;
use std::time::Duration;

/// The OS error of the F:\AC receipt-header read (`WSAETIMEDOUT`).
const RECEIPT_TIMEOUT_OS_ERROR: i32 = 10_060;
/// The bound these runs configure: three retries in a row.
const RETRY_LIMIT: u16 = 3;

/// `policy.monitor` as these runs configure it.
fn settings() -> PolicyMonitorSettings {
    PolicyMonitorSettings {
        io_timeout: Duration::from_secs(15),
        retry_limit: RETRY_LIMIT,
        retry_backoff: Duration::from_millis(1_000),
        retry_backoff_max: Duration::from_millis(3_000),
    }
}

fn identity(issuer: &IdentifierIssuer, pid: u32) -> RuntimeIdentity {
    RuntimeIdentity {
        owner_epoch: *issuer.mint_owner_epoch().expect("owner epoch").transport(),
        pid,
    }
}

/// One answer of the scripted Runtime client to the monitor's subscription.
#[derive(Clone, Copy)]
enum Answer {
    /// A batch of events.
    Batch,
    /// The receipt header did not arrive within the I/O timeout: `TimedOut` (Windows) or
    /// `WouldBlock` (the same read timeout elsewhere), for the request sent to that Runtime.
    HeaderTimeout(ErrorKind, RuntimeIdentity),
}

fn header_timeout(kind: ErrorKind, sent_to: RuntimeIdentity) -> MonitorRequestFailure {
    MonitorRequestFailure {
        operation: ClientLocation::ExchangeRuntimeRequest.location(),
        receipt_header: Some(ReceiptHeaderFailure {
            kind,
            os_error: (kind == ErrorKind::TimedOut).then_some(RECEIPT_TIMEOUT_OS_ERROR),
            sent_to: Some(sent_to),
        }),
    }
}

/// How the monitor ended.
#[derive(Debug)]
enum Ended {
    /// Past the bound: after the give-up outcome.
    GaveUp(Outcome),
    /// At once.
    Failed,
}

/// What the monitor did with the scripted answers.
struct Run {
    /// Each retry's warning and the wait before the next attempt.
    warnings: Vec<(Outcome, Duration)>,
    /// The batches the monitor went on with.
    served: usize,
    /// `None`: the monitor was still monitoring after the last answer.
    ended: Option<Ended>,
}

fn monitor(answers: &[Answer], runtime: RuntimeIdentity) -> Run {
    let mut retries = MonitorRetries::new(settings());
    let mut run = Run {
        warnings: Vec::new(),
        served: 0,
        ended: None,
    };
    for answer in answers {
        match *answer {
            Answer::Batch => {
                retries.succeeded();
                run.served += 1;
            }
            Answer::HeaderTimeout(kind, sent_to) => {
                match retries.failed(&header_timeout(kind, sent_to), runtime) {
                    MonitorRetryStep::Retry { warning, backoff } => {
                        run.warnings.push((warning, backoff));
                    }
                    MonitorRetryStep::GiveUp { outcome } => {
                        run.ended = Some(Ended::GaveUp(outcome));
                        break;
                    }
                    MonitorRetryStep::Fail => {
                        run.ended = Some(Ended::Failed);
                        break;
                    }
                }
            }
        }
    }
    run
}

/// One retry's warning: the registered warning code, with the attempt count, the bound, the I/O
/// kind, the failed operation and the wait before the next attempt as typed values.
#[track_caller]
fn assert_retry_warning(retry: &(Outcome, Duration), attempts: u32, io_kind: &str, wait_ms: u64) {
    let (warning, backoff) = retry;
    assert_eq!(
        warning.code().as_str(),
        "policy_monitor_request_retried",
        "{warning}"
    );
    assert_eq!(warning.category(), Category::Warning, "{warning}");
    for (key, expected) in [
        ("attempts", json!(attempts)),
        ("limit_count", json!(RETRY_LIMIT)),
        ("io_kind", json!(io_kind)),
        ("operation", json!("exchange_runtime_request")),
        ("retry_after_ms", json!(wait_ms)),
    ] {
        assert_eq!(warning.value(key), Some(&expected), "{key} of {warning}");
    }
    assert_eq!(*backoff, Duration::from_millis(wait_ms), "{warning}");
}

/// #397: one receipt-header timeout of the monitor's request, and a few in a row up to the
/// bound, are retried with a warning each, and the monitor goes on with the batches that follow.
/// The wait starts at `retry_backoff`, doubles per retry in a row and stays within
/// `retry_backoff_max`; a batch starts the count again.
#[test]
fn gate_receipt_header_timeouts_under_the_bound_are_warnings_and_the_monitor_goes_on() {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let own = identity(&issuer, 72_564);

    // The F:\AC case: one timeout between batches.
    let run = monitor(
        &[
            Answer::Batch,
            Answer::HeaderTimeout(ErrorKind::TimedOut, own),
            Answer::Batch,
            Answer::Batch,
        ],
        own,
    );
    assert!(
        run.ended.is_none(),
        "one receipt-header timeout ended the monitor: {:?}",
        run.ended
    );
    assert_eq!(run.served, 3, "the monitor goes on after the retry");
    assert_eq!(run.warnings.len(), 1);
    assert_retry_warning(&run.warnings[0], 1, "timed_out", 1_000);
    assert_eq!(
        run.warnings[0].0.value("os_error"),
        Some(&json!(RECEIPT_TIMEOUT_OS_ERROR))
    );

    // Up to the bound in a row, `TimedOut` and `WouldBlock` alike, then a batch; the next
    // timeout is the first again.
    let run = monitor(
        &[
            Answer::HeaderTimeout(ErrorKind::TimedOut, own),
            Answer::HeaderTimeout(ErrorKind::WouldBlock, own),
            Answer::HeaderTimeout(ErrorKind::TimedOut, own),
            Answer::Batch,
            Answer::HeaderTimeout(ErrorKind::WouldBlock, own),
            Answer::Batch,
        ],
        own,
    );
    assert!(
        run.ended.is_none(),
        "receipt-header timeouts up to the bound ended the monitor: {:?}",
        run.ended
    );
    assert_eq!(run.served, 2, "the monitor goes on after the retries");
    assert_eq!(run.warnings.len(), 4);
    assert_retry_warning(&run.warnings[0], 1, "timed_out", 1_000);
    assert_retry_warning(&run.warnings[1], 2, "would_block", 2_000);
    assert_retry_warning(&run.warnings[2], 3, "timed_out", 3_000);
    assert_retry_warning(&run.warnings[3], 1, "would_block", 1_000);
}

/// #397: the monitor still ends when receipt-header timeouts in a row outlast the bound, after
/// the give-up outcome, and at once when the timed-out request went to another Runtime: another
/// owner epoch, or the same epoch in another process.
#[test]
fn gate_past_the_bound_or_after_an_owner_change_the_monitor_still_ends() {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let own = identity(&issuer, 72_564);

    let timeouts = [Answer::HeaderTimeout(ErrorKind::TimedOut, own); RETRY_LIMIT as usize + 1];
    let run = monitor(&timeouts, own);
    assert_eq!(run.warnings.len(), usize::from(RETRY_LIMIT));
    let Some(Ended::GaveUp(outcome)) = &run.ended else {
        panic!(
            "receipt-header timeouts past the bound did not end the monitor with its give-up: {:?}",
            run.ended
        );
    };
    assert_eq!(
        outcome.code().as_str(),
        "policy_monitor_retries_exhausted",
        "{outcome}"
    );
    assert_eq!(outcome.category(), Category::Error, "{outcome}");
    for (key, expected) in [
        ("attempts", json!(RETRY_LIMIT + 1)),
        ("limit_count", json!(RETRY_LIMIT)),
        ("io_kind", json!("timed_out")),
        ("operation", json!("exchange_runtime_request")),
    ] {
        assert_eq!(outcome.value(key), Some(&expected), "{key} of {outcome}");
    }

    let other_epoch = identity(&issuer, own.pid);
    let other_process = RuntimeIdentity {
        owner_epoch: own.owner_epoch,
        pid: own.pid + 1,
    };
    for sent_to in [other_epoch, other_process] {
        let run = monitor(
            &[
                Answer::Batch,
                Answer::HeaderTimeout(ErrorKind::TimedOut, sent_to),
            ],
            own,
        );
        assert!(
            matches!(run.ended, Some(Ended::Failed)),
            "a timed-out request to another Runtime must end the monitor at once: {:?}",
            run.ended
        );
        assert!(run.warnings.is_empty());
    }
}
