// SPDX-License-Identifier: AGPL-3.0-only

// #419 SUB-LAG-1 (brief rd-1031/brief-sub-lag-1.md; register §1.261): `contracts/ledger-store.md`
// "Reads, projections and subscriptions". A lag ends only that subscription: `subscription_lagged`
// is an error, never fatal, with `ledger_sequence`, the event the subscriber's live queue could not
// take; the writer and every other subscriber go on. A writer failure is still fatal to every
// subscriber.
//
// Run on the SQLite store and on the Segment store, as gate L-3 G8 is. Nothing waits on a clock:
// the commit barrier is a writer command (`TestSubscriberCount`), answered only after the writer
// delivered every committed event, and `GATE_WAIT` bounds a positive wait only.

use super::{config, event, sqlite_contract};
use crate::global::{
    GlobalLedger, GlobalLedgerConfig, GlobalLedgerError, GlobalLedgerResult, WriterCommand,
};
use actingcommand_contract::SubscriptionCursor;
use serde_json::Value;
use std::sync::mpsc;
use std::time::Duration;
use tempfile::TempDir;

const GATE_WAIT: Duration = Duration::from_secs(30);

// The commit barrier: the writer answers only after it delivered every committed event.
fn subscriber_count(ledger: &GlobalLedger) -> usize {
    let (response, count) = mpsc::sync_channel(1);
    ledger
        .sender
        .as_ref()
        .expect("writer sender")
        .send(WriterCommand::TestSubscriberCount { response })
        .expect("request the subscriber count");
    count.recv_timeout(GATE_WAIT).expect("the subscriber count")
}

// Two subscribers on live queues of one event: the slow one reads nothing and lags at the second
// event, the other reads each event. The lag is not fatal and ends only the slow subscription; the
// writer and the other subscriber go on. A writer failure then reaches the other subscriber as
// fatal, and the slow one keeps its lag.
fn a_writer_failure_is_still_fatal_while_a_lag_is_not(
    open: impl Fn(GlobalLedgerConfig) -> GlobalLedgerResult<GlobalLedger>,
) {
    let temp = TempDir::new().expect("temp");
    let mut ledger = open(
        config(&temp, "sub-lag-writer")
            .with_reply_timeout(GATE_WAIT)
            .with_subscription_capacity(1),
    )
    .expect("ledger");
    let mut slow = ledger
        .subscribe(SubscriptionCursor::default())
        .expect("the slow subscription");
    let mut reader = ledger
        .subscribe(SubscriptionCursor::default())
        .expect("the reading subscription");

    let first = ledger
        .append(event("evt-sub-lag-first"))
        .expect("first event");
    assert_eq!(
        reader.recv_timeout(GATE_WAIT).expect("the reader's first"),
        first
    );
    let second = ledger
        .append(event("evt-sub-lag-second"))
        .expect("second event overflows the slow queue");
    assert_eq!(
        subscriber_count(&ledger),
        1,
        "the lag removed only the slow subscriber"
    );

    let lagged = slow
        .recv_timeout(GATE_WAIT)
        .expect_err("the slow subscription lagged");
    assert_eq!(lagged.code(), "subscription_lagged");
    assert!(!lagged.is_fatal(), "a lag is not a ledger fault");
    assert_eq!(
        lagged
            .outcome_values()
            .and_then(|outcome| outcome.value("ledger_sequence"))
            .and_then(Value::as_u64),
        Some(second.sequence()),
        "the lag names the event the queue could not take"
    );
    assert_eq!(
        slow.resume_cursor().after_sequence,
        0,
        "the slow subscription resumes where it stood"
    );

    // The writer and the other subscriber go on.
    assert_eq!(
        reader.recv_timeout(GATE_WAIT).expect("the reader's second"),
        second
    );
    let third = ledger
        .append(event("evt-sub-lag-third"))
        .expect("the writer keeps committing");
    assert_eq!(
        reader.recv_timeout(GATE_WAIT).expect("the reader's third"),
        third
    );

    // A writer failure is still fatal to every subscriber.
    let failure = GlobalLedgerError::fatal_untyped("test_terminal", "test_writer_failure");
    ledger
        .sender
        .as_ref()
        .expect("writer sender")
        .send(WriterCommand::TestTerminalFailure {
            error: failure.clone(),
        })
        .expect("inject a writer failure");
    ledger.sender.take();
    let writer = ledger.writer.take().expect("writer handle");
    assert_eq!(
        writer
            .join()
            .expect("the writer does not panic")
            .expect_err("the writer ends with its failure"),
        failure
    );
    let ended = reader
        .recv_timeout(GATE_WAIT)
        .expect_err("the writer failure reaches the reader");
    assert!(ended.is_fatal(), "a writer failure is still fatal");
    assert_eq!(ended, failure);
    assert_eq!(
        slow.recv_timeout(GATE_WAIT)
            .expect_err("the lag stays latched"),
        lagged
    );
}

#[test]
fn a_writer_failure_is_still_fatal_to_subscribers() {
    a_writer_failure_is_still_fatal_while_a_lag_is_not(sqlite_contract::open);
    a_writer_failure_is_still_fatal_while_a_lag_is_not(GlobalLedger::open);
}
