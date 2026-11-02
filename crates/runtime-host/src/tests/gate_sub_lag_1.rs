// SPDX-License-Identifier: AGPL-3.0-only

//! #419 SUB-LAG-1 (brief rd-1031/brief-sub-lag-1.md; register §1.261;
//! contracts/runtime-interface.md "A subscriber that falls behind"): a `SubscribeEvents` request
//! whose subscriber falls behind ends only its subscription and that request, `failed` and not
//! fatal, with the cursor to subscribe again from; the Runtime goes on.
//!
//! The lag is made on the test's own thread. The request is served by `process_request_for_test`,
//! and `AFTER_SUBSCRIBE_FOR_TEST`, which runs right after its subscription registered and before it
//! receives anything, records more client actions over another connection than the subscriber's
//! live queue holds. An action's receipt comes back once the writer committed it, and the writer
//! delivers a commit's events to the live queues before it takes its next command, so by the last
//! receipt the queue has overflowed. Nothing sleeps, and no clock is read here.

use super::*;
use crate::host::AFTER_SUBSCRIBE_FOR_TEST;
use actingcommand_contract::outcome::Detail;
use actingcommand_contract::{RuntimeSubscriptionRequest, SubscriptionCursor};
use serde_json::{Value, json};

/// A subscription's live queue holds 64 events by default (contracts/ledger-store.md, "Reads,
/// projections and subscriptions"). Each action commits at least one event, so two actions more
/// than that overflow the queue before the last action is committed.
const LAG_ACTIONS: usize = 64 + 2;
/// The connection the test's own requests are served on.
const TEST_CONNECTION: u64 = 41_901;
/// Bounds the batches that read the actions back; each batch reads up to its maximum.
const MAX_RESUME_BATCHES: usize = 16;

/// One client action, the request whose effect is its own record.
fn client_action() -> ClientActionRecord {
    ClientActionRecord::new("settings", "lag", ClientActionKind::Button, None, None)
        .expect("client action")
}

/// A subscription of `query` from `cursor`, at most `max_events` events.
fn subscribe(query: EventQuery, cursor: SubscriptionCursor, max_events: u16) -> RuntimeOperation {
    RuntimeOperation::SubscribeEvents {
        request: RuntimeSubscriptionRequest::new(
            query,
            ProjectionProfile::Forensic,
            cursor,
            100,
            max_events,
        )
        .expect("subscription request"),
    }
}

/// Makes the next subscription on this thread fall behind: right after it registered, another
/// connection records `LAG_ACTIONS` client actions, each answered once committed.
fn lag_next_subscription(host: &RuntimeHost) {
    let mut writer = TestClient::connect(host);
    AFTER_SUBSCRIBE_FOR_TEST.with(|after| {
        *after.borrow_mut() = Some(Box::new(move || -> Box<dyn std::any::Any> {
            for _ in 0..LAG_ACTIONS {
                let request = writer.request(RuntimeOperation::RecordClientAction {
                    action: client_action(),
                });
                let receipt = writer.send(&request);
                assert_eq!(
                    receipt.state(),
                    RuntimeReceiptState::Completed,
                    "{receipt:?}"
                );
            }
            Box::new(())
        }));
    });
}

/// Serves `request` on the test's own thread.
fn serve(host: &RuntimeHost, request: &RuntimeRequest) -> RuntimeReceipt {
    host.process_request_for_test(
        request,
        ConnectionId::new(TEST_CONNECTION).expect("connection"),
    )
    .expect("a receipt")
}

/// The request failed, not fatally, with `subscription_lagged` over `runtime_unavailable` and
/// the cursor `resume_after_sequence`; returns its `ledger_sequence`.
fn assert_lagged(receipt: &RuntimeReceipt, resume_after_sequence: u64) -> u64 {
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    assert_eq!(
        receipt.outcome().code().as_str(),
        "subscription_lagged",
        "{receipt:?}"
    );
    let projection = receipt.error_projection().expect("the request's failure");
    assert_eq!(projection.code, RuntimeErrorCode::RuntimeUnavailable);
    assert!(!projection.fatal, "{receipt:?}");
    let lagged_at = receipt
        .outcome()
        .value("ledger_sequence")
        .and_then(Value::as_u64)
        .expect("the event the queue could not take");
    assert!(lagged_at > resume_after_sequence, "{receipt:?}");
    assert_eq!(
        Value::Object(receipt.outcome().values().clone()),
        json!({
            "operation": "receive_runtime_subscription",
            "runtime_code": "runtime_unavailable",
            "host_halted": false,
            "ledger_sequence": lagged_at,
            "resume_after_sequence": resume_after_sequence
        }),
        "{receipt:?}"
    );
    lagged_at
}

/// Nothing latched and no stop was requested.
fn assert_runtime_running(host: &RuntimeHost) {
    assert!(
        host.fatal_error().expect("runtime health").is_none(),
        "nothing latched"
    );
    assert!(
        !host.is_shutdown_requested().expect("shutdown flag"),
        "no stop requested"
    );
}

/// The event batch of `receipt`.
fn batch(receipt: &RuntimeReceipt) -> &actingcommand_contract::RuntimeEventBatch {
    let Some(RuntimeResult::EventBatch { batch }) = receipt.result() else {
        panic!("an event batch: {receipt:?}");
    };
    batch
}

#[test]
fn a_lagged_subscription_ends_only_its_request() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, "node.a", Arc::new(FakeState::default()));
    let ids = IdentifierIssuer::new().expect("identifier issuer");

    lag_next_subscription(&host);
    let request = runtime_request(
        &ids,
        subscribe(EventQuery::default(), SubscriptionCursor::default(), 16),
    );
    let failed = serve(&host, &request);

    // The lag preempted the replay: nothing was delivered, so the cursor is the request's own.
    assert_lagged(&failed, 0);
    assert_runtime_running(&host);
    host.close().expect("close host");
}

#[test]
fn the_runtime_keeps_serving_after_a_subscriber_lags() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, "node.a", Arc::new(FakeState::default()));
    let ids = IdentifierIssuer::new().expect("identifier issuer");

    lag_next_subscription(&host);
    let request = runtime_request(
        &ids,
        subscribe(EventQuery::default(), SubscriptionCursor::default(), 16),
    );
    let lagged_at = assert_lagged(&serve(&host, &request), 0);
    assert_runtime_running(&host);

    // The writer keeps committing.
    let mut client = TestClient::connect(&host);
    let action = client.request(RuntimeOperation::RecordClientAction {
        action: client_action(),
    });
    let recorded = client.send(&action);
    assert_eq!(
        recorded.state(),
        RuntimeReceiptState::Completed,
        "{recorded:?}"
    );
    let recorded_at = recorded.terminal().expect("the action's terminal").sequence;
    assert!(recorded_at > lagged_at, "{recorded:?}");

    // The door keeps answering reads.
    let status = client.request(RuntimeOperation::Status);
    let status = client.send(&status);
    assert!(
        matches!(status.result(), Some(RuntimeResult::Status { .. })),
        "{status:?}"
    );

    // Another subscription reads the event just committed.
    let next = client.request(subscribe(
        EventQuery::default(),
        SubscriptionCursor {
            after_sequence: recorded_at - 1,
        },
        1,
    ));
    let next = client.send(&next);
    assert_eq!(next.state(), RuntimeReceiptState::Completed, "{next:?}");
    let [event] = batch(&next).events() else {
        panic!("one event: {next:?}");
    };
    assert_eq!(event.sequence, recorded_at);
    assert_runtime_running(&host);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn a_lagged_subscriber_can_resume_from_its_cursor() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, "node.a", Arc::new(FakeState::default()));
    let mut console = TestClient::connect(&host);

    // The subscriber reads its first event and keeps the cursor after it.
    let first = console.request(subscribe(
        EventQuery::default(),
        SubscriptionCursor::default(),
        1,
    ));
    let first = console.send(&first);
    let cursor = batch(&first).next_cursor();
    assert!(cursor.after_sequence > 0, "{first:?}");

    // Its next request falls behind. Asked from the console (codes mode), the failure still
    // names the cursor to subscribe again from: where the subscriber stood, since it was
    // delivered nothing.
    lag_next_subscription(&host);
    let request = console.governance_request(subscribe(EventQuery::default(), cursor, 16));
    let failed = serve(&host, &request);
    assert_lagged(&failed, cursor.after_sequence);
    let written = failed.clone().for_request_detail(&request);
    assert_eq!(written.outcome_envelope().detail(), Detail::Codes);
    assert_eq!(
        Value::Object(written.outcome().values().clone()),
        json!({
            "operation": "receive_runtime_subscription",
            "runtime_code": "runtime_unavailable",
            "host_halted": false,
            "resume_after_sequence": cursor.after_sequence
        }),
        "{written:?}"
    );
    let resume = SubscriptionCursor {
        after_sequence: written
            .outcome()
            .value("resume_after_sequence")
            .and_then(Value::as_u64)
            .expect("the cursor to resume from"),
    };
    assert_runtime_running(&host);

    // Subscribing again from it reads on right after it, with no gap.
    let next = console.request(subscribe(EventQuery::default(), resume, 1));
    let next = console.send(&next);
    let [event] = batch(&next).events() else {
        panic!("the next event: {next:?}");
    };
    assert_eq!(event.sequence, resume.after_sequence + 1);

    // Every action recorded while it lagged arrives after the cursor, in order.
    let actions = EventQuery {
        event_type: Some(EventType::ClientAction),
        ..EventQuery::default()
    };
    let mut after = resume;
    let mut sequences = Vec::new();
    for _ in 0..MAX_RESUME_BATCHES {
        if sequences.len() == LAG_ACTIONS {
            break;
        }
        let read = console.request(subscribe(actions.clone(), after, 256));
        let read = console.send(&read);
        assert_eq!(read.state(), RuntimeReceiptState::Completed, "{read:?}");
        let read = batch(&read);
        sequences.extend(read.events().iter().map(|event| event.sequence));
        after = read.next_cursor();
    }
    assert_eq!(sequences.len(), LAG_ACTIONS, "{sequences:?}");
    assert!(
        sequences
            .first()
            .is_some_and(|first| *first > resume.after_sequence),
        "{sequences:?}"
    );
    assert!(
        sequences.windows(2).all(|pair| pair[0] < pair[1]),
        "{sequences:?}"
    );
    assert_runtime_running(&host);
    drop(console);
    host.close().expect("close host");
}
