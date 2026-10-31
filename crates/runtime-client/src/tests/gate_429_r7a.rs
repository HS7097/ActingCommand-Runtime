// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-7a (register §1.233; contracts/player-hold.md, "Lost receipts"; design R-7a
// §7.1 "client methods"): the runtime client's player hold methods over `ScriptedRuntime`, with
// no sleep and no device.
// 1. Prepare writes nothing and fixes the request id; submit writes once, never again.
// 2. The trigger: the connection ending before the receipt header is `dropped`; the receipt-read
//    deadline passing is `runtime_receipt_timeout` and `timed_out`, also with a session deadline
//    shorter than the deadline; every other failure is returned as it is, unsettled (the
//    `player_hold_interrupted` receipt and R-7a's `not_implemented` among them).
// 3. The method settles once from `Status` and leaves the handle as the post-condition table
//    says: same epoch, usable; another epoch, latched `runtime_owner_epoch_changed`; unreadable,
//    latched with that failure.
// 4. The two settlement codes' class, and the typed settlement on the three settlement codes.
// 5. The receipt waits: the socket timeout is the whole formula, the deadline the formula without
//    the IO margin.

use super::unreached_deadline;
use crate::client::{player_hold_receipt_waits, player_hold_waits_for_test};
use crate::ipc::{ReceiptReadDeadline, exchange};
use crate::test_support::{ScriptedAnswer, ScriptedConnection, ScriptedRuntime};
use crate::{
    ClientCode, RuntimeClient, RuntimeClientConfig, RuntimeClientError, RuntimeClientErrorClass,
};
use actingcommand_contract::outcome::Code;
use actingcommand_contract::{
    EventActor, EventSource, IdentifierIssuer, OwnerEpoch, PlayerHoldAttempt,
    PlayerHoldExpectation, PlayerHoldOperation, PlayerHoldOperationKind, PlayerHoldOperationResult,
    PlayerHoldOperationState, PlayerHoldPhase, PlayerHoldSettlement, PlayerHoldSettlementRule,
    PlayerHoldSettlementTrigger, PlayerHoldSettlementVerdict, PlayerHoldSource,
    PlayerHoldSourceActor, PlayerHoldState, RequestId, RuntimeChannel, RuntimeControlPlaneStatus,
    RuntimeErrorCode, RuntimeErrorProjection, RuntimeInstanceStatus, RuntimeOperation,
    RuntimeReceipt, RuntimeReceiptState, RuntimeRequest, RuntimeResult, SchedulingDrainSummary,
    settle_player_hold,
};
use std::io::Read;
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tempfile::TempDir;

const INSTANCE: &str = "node-01";
/// An IO budget no answered exchange of these tests comes near.
const IO: Duration = Duration::from_secs(10);
/// A receipt-read deadline short enough for a test (the seam `player_hold_waits_for_test`).
const TEST_DEADLINE: Duration = Duration::from_millis(200);
/// The IO margin beside it: the socket timeout is the two together.
const SHORT_IO: Duration = Duration::from_millis(300);

fn connect(state_root: &Path, io_timeout: Duration) -> RuntimeClient {
    RuntimeClient::connect(
        RuntimeClientConfig::new(
            state_root,
            RuntimeChannel::Cli,
            EventActor::Cli,
            EventSource::Cli,
        )
        .with_io_timeout(io_timeout),
    )
    .expect("runtime client")
}

/// A deadline already reached when the client first checks it: `unreached_deadline` less the
/// hour it grants (the form of `gate_422_p32a::reached_deadline`).
fn reached_deadline() -> Instant {
    unreached_deadline()
        .checked_sub(Duration::from_secs(60 * 60))
        .expect("the deadline unreached_deadline grants, less its hour")
}

fn ids() -> IdentifierIssuer {
    IdentifierIssuer::new().expect("identifier issuer")
}

fn source() -> PlayerHoldSource {
    PlayerHoldSource {
        actor: PlayerHoldSourceActor::Cli,
        channel: RuntimeChannel::Cli,
        via: None,
    }
}

fn held(hold_id: RequestId) -> PlayerHoldState {
    PlayerHoldState {
        hold_id,
        revision: 1,
        phase: PlayerHoldPhase::Held,
        since_unix_ms: 1_000,
        source: source(),
        drain_deadline_unix_ms: None,
        grace_deadline_unix_ms: None,
        close_started_unix_ms: None,
        reached_held_unix_ms: Some(2_000),
        restored_from_owner_epoch: None,
        restored_from_phase: None,
    }
}

fn acquire_record(request_id: RequestId, state: PlayerHoldOperationState) -> PlayerHoldOperation {
    let running = state == PlayerHoldOperationState::Running;
    PlayerHoldOperation {
        request_id,
        kind: PlayerHoldOperationKind::Acquire,
        state,
        phase: running.then_some(PlayerHoldPhase::Draining),
        hold_id: request_id,
        source: source(),
        since_unix_ms: 1_000,
        ended_unix_ms: (!running).then_some(3_000),
        code: None,
        failed_phase: None,
        result: (!running).then(|| PlayerHoldOperationResult {
            revision: Some(1),
            drained: Some(SchedulingDrainSummary {
                finished: 0,
                cancelled: 0,
            }),
            ..PlayerHoldOperationResult::default()
        }),
    }
}

fn release_record_running(request_id: RequestId, hold_id: RequestId) -> PlayerHoldOperation {
    PlayerHoldOperation {
        request_id,
        kind: PlayerHoldOperationKind::Release,
        state: PlayerHoldOperationState::Running,
        phase: Some(PlayerHoldPhase::Releasing),
        hold_id,
        source: source(),
        since_unix_ms: 1_000,
        ended_unix_ms: None,
        code: None,
        failed_phase: None,
        result: None,
    }
}

/// The status of one instance with the hold and the record given.
fn status_of(
    owner_epoch: OwnerEpoch,
    hold: Option<PlayerHoldState>,
    record: Option<PlayerHoldOperation>,
) -> RuntimeControlPlaneStatus {
    let instance = RuntimeInstanceStatus::new(
        INSTANCE,
        *ids().mint_instance_id().expect("instance id").transport(),
        false,
        0,
        false,
        false,
        false,
    )
    .expect("instance")
    .with_player_hold(hold)
    .expect("hold")
    .with_player_hold_operation(record)
    .expect("record");
    RuntimeControlPlaneStatus::new(owner_epoch, vec![instance]).expect("status")
}

fn status_answer(request: &RuntimeRequest, status: RuntimeControlPlaneStatus) -> ScriptedAnswer {
    assert!(
        matches!(request.operation(), RuntimeOperation::Status),
        "a status read, got {:?}",
        request.operation()
    );
    ScriptedAnswer::completed(request, RuntimeResult::Status { status })
}

fn is_hold_request(request: &RuntimeRequest) -> bool {
    matches!(
        request.operation(),
        RuntimeOperation::AcquirePlayerHold { .. } | RuntimeOperation::ReleasePlayerHold { .. }
    )
}

fn decision(
    error: &RuntimeClientError,
) -> (PlayerHoldSettlementRule, PlayerHoldSettlementVerdict, bool) {
    let Some(PlayerHoldSettlement::Decided(decision)) = error.player_hold_settlement() else {
        panic!("a settlement decision: {error:?}");
    };
    (decision.rule, decision.verdict, decision.is_final)
}

fn expected(owner_epoch: OwnerEpoch, hold_id: RequestId) -> PlayerHoldExpectation {
    PlayerHoldExpectation {
        owner_epoch,
        hold_id,
        revision: 1,
    }
}

type Seen = Arc<Mutex<Option<RequestId>>>;

fn seen_id(seen: &Seen) -> RequestId {
    seen.lock()
        .expect("seen")
        .expect("the hold request arrived first")
}

// 1: prepare writes nothing; submit writes the prepared request once, the answer's hold is the
// request's own id, and the attempt names it.
#[test]
fn r7a_prepare_writes_nothing_and_submit_writes_the_prepared_request_once() {
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), |_| {
        vec![ScriptedConnection::new().request(|request| {
            assert!(is_hold_request(request));
            ScriptedAnswer::completed(
                request,
                RuntimeResult::PlayerHoldAcquired(Box::new(
                    actingcommand_contract::PlayerHoldAcquired {
                        hold: held(request.request_id()),
                        drained: SchedulingDrainSummary {
                            finished: 1,
                            cancelled: 0,
                        },
                    },
                )),
            )
        })]
    });
    let client = connect(root.path(), IO);
    let prepared = client
        .prepare_acquire_player_hold(INSTANCE, 60_000)
        .expect("prepared");
    let request_id = prepared.request_id();
    assert_eq!(prepared.owner_epoch(), runtime.owner_epoch());
    let attempt = prepared.attempt(PlayerHoldSettlementTrigger::Reopened);
    assert_eq!(
        attempt,
        PlayerHoldAttempt {
            kind: PlayerHoldOperationKind::Acquire,
            request_id,
            sent_in_owner_epoch: runtime.owner_epoch(),
            expected_hold_id: request_id,
            trigger: PlayerHoldSettlementTrigger::Reopened,
        }
    );
    let acquired = client
        .submit_acquire_player_hold(prepared)
        .expect("acquired");
    assert_eq!(acquired.hold.hold_id, request_id);
    drop(client);
    let record = runtime.finish();
    assert_eq!(
        record.connections[0]
            .count(|operation| matches!(operation, RuntimeOperation::AcquirePlayerHold { .. })),
        1
    );
    let sent = record.connections[0]
        .requests
        .iter()
        .find(|request| is_hold_request(request))
        .expect("the hold request");
    assert_eq!(sent.request_id(), request_id);
    assert!(record.unscripted.is_empty(), "{:?}", record.unscripted);

    // A prepared request submitted on another handle is refused before anything is written.
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), |_| {
        vec![ScriptedConnection::new(), ScriptedConnection::new()]
    });
    let first = connect(root.path(), IO);
    let second = connect(root.path(), IO);
    let prepared = first
        .prepare_release_player_hold(INSTANCE, expected(runtime.owner_epoch(), request_id), None)
        .expect("prepared");
    let error = second
        .submit_release_player_hold(prepared)
        .expect_err("another handle");
    assert_eq!(
        error.code(),
        ClientCode::RuntimePlayerHoldPreparedClientMismatch.code()
    );
    assert!(error.player_hold_settlement().is_none());
    drop((first, second));
    let record = runtime.finish();
    for connection in &record.connections {
        assert_eq!(
            connection
                .count(|operation| matches!(operation, RuntimeOperation::ReleasePlayerHold { .. })),
            0
        );
    }
}

// 2, 3: an older Runtime drops the frame: same epoch, no revision-4 sign: A7 / R8,
// `runtime_operation_unsupported` with the settlement; the new connection is in place and the
// handle's `status()` works.
#[test]
fn r7a_a_dropped_frame_on_an_older_runtime_is_unsupported_and_the_handle_stays_usable() {
    for kind in [
        PlayerHoldOperationKind::Acquire,
        PlayerHoldOperationKind::Release,
    ] {
        let root = TempDir::new().expect("tempdir");
        let runtime = ScriptedRuntime::start(root.path(), |owner_epoch| {
            vec![
                ScriptedConnection::new().request(|request| {
                    assert!(is_hold_request(request));
                    ScriptedAnswer::Close
                }),
                ScriptedConnection::new()
                    .request(move |request| {
                        status_answer(request, status_of(owner_epoch, None, None))
                    })
                    .request(move |request| {
                        status_answer(request, status_of(owner_epoch, None, None))
                    }),
            ]
        });
        let client = connect(root.path(), IO);
        let error = match kind {
            PlayerHoldOperationKind::Acquire => client
                .acquire_player_hold(INSTANCE, 1_000)
                .map(|_| ())
                .expect_err("dropped"),
            PlayerHoldOperationKind::Release => client
                .release_player_hold(
                    INSTANCE,
                    expected(
                        runtime.owner_epoch(),
                        *ids().mint_request_id().expect("id").transport(),
                    ),
                    None,
                )
                .map(|_| ())
                .expect_err("dropped"),
        };
        assert_eq!(
            error.code(),
            ClientCode::RuntimeOperationUnsupported.code(),
            "{error:?}"
        );
        let (rule, verdict, is_final) = decision(&error);
        assert_eq!(
            rule,
            match kind {
                PlayerHoldOperationKind::Acquire => PlayerHoldSettlementRule::A7,
                PlayerHoldOperationKind::Release => PlayerHoldSettlementRule::R8,
            }
        );
        assert_eq!(verdict, PlayerHoldSettlementVerdict::Unsupported);
        assert!(is_final);
        assert_eq!(
            error.player_hold_attempt().map(|attempt| attempt.trigger),
            Some(PlayerHoldSettlementTrigger::Dropped)
        );
        assert_ne!(error.disposition(), RuntimeClientErrorClass::Uncertain);
        client.status().expect("the new connection is in place");
        drop(client);
        let record = runtime.finish();
        assert!(record.unscripted.is_empty(), "{:?}", record.unscripted);
        let holds: usize = record
            .connections
            .iter()
            .map(|connection| {
                connection.count(|operation| {
                    matches!(
                        operation,
                        RuntimeOperation::AcquirePlayerHold { .. }
                            | RuntimeOperation::ReleasePlayerHold { .. }
                    )
                })
            })
            .sum();
        assert_eq!(holds, 1, "sent once, never again");
    }
}

// 3, 4: dropped after the Runtime accepted and finished: A2 `runtime_player_hold_settled`,
// not uncertain, class `runtime`, the typed settlement and attempt on it and its values on the
// outcome.
#[test]
fn r7a_a_dropped_receipt_of_a_finished_acquire_settles_completed() {
    let root = TempDir::new().expect("tempdir");
    let seen: Seen = Arc::default();
    let runtime = ScriptedRuntime::start(root.path(), {
        let seen = Arc::clone(&seen);
        move |owner_epoch| {
            let first = Arc::clone(&seen);
            vec![
                ScriptedConnection::new().request(move |request| {
                    *first.lock().expect("seen") = Some(request.request_id());
                    ScriptedAnswer::Close
                }),
                ScriptedConnection::new().request(move |request| {
                    let r = seen_id(&seen);
                    status_answer(
                        request,
                        status_of(
                            owner_epoch,
                            Some(held(r)),
                            Some(acquire_record(r, PlayerHoldOperationState::Completed)),
                        ),
                    )
                }),
            ]
        }
    });
    let client = connect(root.path(), IO);
    let error = client
        .acquire_player_hold(INSTANCE, 1_000)
        .expect_err("dropped");
    drop(client);
    assert_eq!(error.code(), ClientCode::RuntimePlayerHoldSettled.code());
    assert_eq!(
        decision(&error),
        (
            PlayerHoldSettlementRule::A2,
            PlayerHoldSettlementVerdict::Settled,
            true
        )
    );
    assert!(error.is_fatal());
    assert_eq!(error.disposition(), RuntimeClientErrorClass::Runtime);
    let outcome = error.outcome();
    assert_eq!(
        outcome.value("settlement_outcome"),
        Some(&serde_json::json!("completed"))
    );
    assert_eq!(
        outcome.value("settlement_rule"),
        Some(&serde_json::json!("A2"))
    );
    assert_eq!(
        outcome.value("settlement_trigger"),
        Some(&serde_json::json!("dropped"))
    );
    assert_eq!(
        outcome.value("settlement_final"),
        Some(&serde_json::json!(true))
    );
    runtime.finish();
}

// 2, 3: the receipt-read deadline passes (the test seam's 200 ms; the socket timeout is it plus
// the IO margin): `runtime_receipt_timeout` and `timed_out`; the timed-out connection is closed
// and never read again; the settlement reads the status on a new connection, and the handle's
// `status()` works afterwards. A running own record is A1 / R1; none is A9 / R6.
#[test]
fn r7a_a_receipt_timeout_is_timed_out_and_settles_on_a_new_connection() {
    player_hold_waits_for_test::set(Some(TEST_DEADLINE));
    let hold_id = *ids().mint_request_id().expect("hold id").transport();
    for (kind, own_running, rule) in [
        (
            PlayerHoldOperationKind::Acquire,
            true,
            PlayerHoldSettlementRule::A1,
        ),
        (
            PlayerHoldOperationKind::Acquire,
            false,
            PlayerHoldSettlementRule::A9,
        ),
        (
            PlayerHoldOperationKind::Release,
            true,
            PlayerHoldSettlementRule::R1,
        ),
        (
            PlayerHoldOperationKind::Release,
            false,
            PlayerHoldSettlementRule::R6,
        ),
    ] {
        let root = TempDir::new().expect("tempdir");
        let seen: Seen = Arc::default();
        let runtime = ScriptedRuntime::start(root.path(), {
            let seen = Arc::clone(&seen);
            move |owner_epoch| {
                let first = Arc::clone(&seen);
                let shown = move || {
                    let r = seen_id(&seen);
                    match (kind, own_running) {
                        (PlayerHoldOperationKind::Acquire, true) => status_of(
                            owner_epoch,
                            Some(PlayerHoldState {
                                phase: PlayerHoldPhase::Draining,
                                drain_deadline_unix_ms: Some(61_000),
                                grace_deadline_unix_ms: Some(91_000),
                                reached_held_unix_ms: None,
                                ..held(r)
                            }),
                            Some(acquire_record(r, PlayerHoldOperationState::Running)),
                        ),
                        (PlayerHoldOperationKind::Acquire, false) => {
                            status_of(owner_epoch, None, None)
                        }
                        (PlayerHoldOperationKind::Release, true) => status_of(
                            owner_epoch,
                            Some(PlayerHoldState {
                                phase: PlayerHoldPhase::Releasing,
                                ..held(hold_id)
                            }),
                            Some(release_record_running(r, hold_id)),
                        ),
                        (PlayerHoldOperationKind::Release, false) => {
                            status_of(owner_epoch, Some(held(hold_id)), None)
                        }
                    }
                };
                let again = shown.clone();
                vec![
                    ScriptedConnection::new().request(move |request| {
                        *first.lock().expect("seen") = Some(request.request_id());
                        ScriptedAnswer::StallUntilClosed
                    }),
                    ScriptedConnection::new()
                        .request(move |request| status_answer(request, shown()))
                        .request(move |request| status_answer(request, again())),
                ]
            }
        });
        let client = connect(root.path(), SHORT_IO);
        let error = match kind {
            PlayerHoldOperationKind::Acquire => client
                .acquire_player_hold(INSTANCE, 1_000)
                .map(|_| ())
                .expect_err("timed out"),
            PlayerHoldOperationKind::Release => client
                .release_player_hold(INSTANCE, expected(runtime.owner_epoch(), hold_id), None)
                .map(|_| ())
                .expect_err("timed out"),
        };
        assert_eq!(
            error.code(),
            ClientCode::RuntimePlayerHoldUnconfirmed.code(),
            "{error:?}"
        );
        assert!(
            format!("{error:?}").contains("runtime_receipt_timeout"),
            "{error:?}"
        );
        assert_eq!(decision(&error).0, rule);
        assert_eq!(
            error.player_hold_attempt().map(|attempt| attempt.trigger),
            Some(PlayerHoldSettlementTrigger::TimedOut)
        );
        assert_eq!(error.disposition(), RuntimeClientErrorClass::Uncertain);
        assert!(error.is_fatal());
        client.status().expect("same epoch: the handle is usable");
        drop(client);
        // The stalled connection ended when the client closed it; nothing was sent again.
        let record = runtime.finish();
        assert_eq!(
            record.connections[0].requests.len(),
            3,
            "Health, BindChannel, the request"
        );
        assert!(record.unscripted.is_empty(), "{:?}", record.unscripted);
    }
    player_hold_waits_for_test::set(None);
}

// 2: a session deadline shorter than the receipt-read deadline still ends in
// `runtime_receipt_timeout` and `timed_out`. The session's deadline is one already reached,
// set after the handshake (the seam `set_session_deadline_for_test`, so the connect is not cut):
// the receipt read is cut to it and the frame is still written once. The session is over, so
// the settlement's reconnect waits 1 ms for an answer the script never gives: P1.
#[test]
fn r7a_a_session_deadline_shorter_than_the_deadline_is_still_timed_out() {
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), |_| {
        vec![ScriptedConnection::new().request(|_| ScriptedAnswer::StallUntilClosed)]
    });
    let client = connect(root.path(), IO);
    client.set_session_deadline_for_test(Some(reached_deadline()));
    let error = client
        .acquire_player_hold(INSTANCE, 600_000)
        .expect_err("cut by the session");
    drop(client);
    assert!(
        format!("{error:?}").contains("runtime_receipt_timeout"),
        "{error:?}"
    );
    assert_eq!(
        error.player_hold_attempt().map(|attempt| attempt.trigger),
        Some(PlayerHoldSettlementTrigger::TimedOut)
    );
    assert_eq!(decision(&error).0, PlayerHoldSettlementRule::P1);
    let record = runtime.finish();
    assert_eq!(
        record.connections[0]
            .count(|operation| matches!(operation, RuntimeOperation::AcquirePlayerHold { .. })),
        1
    );
    for unscripted in &record.unscripted {
        assert_eq!(
            unscripted
                .count(|operation| matches!(operation, RuntimeOperation::AcquirePlayerHold { .. })),
            0,
            "never sent again"
        );
    }
}

// 3: the Runtime restarted between the frame and the settlement: a temporary client reads the
// new epoch's status (A11), the main code is the settlement's, and the handle stays latched
// `runtime_owner_epoch_changed`.
#[test]
fn r7a_another_epoch_settles_through_a_temporary_client_and_latches_the_handle() {
    let root = TempDir::new().expect("tempdir");
    let seen: Seen = Arc::default();
    let old = ScriptedRuntime::start(root.path(), {
        let seen = Arc::clone(&seen);
        move |_| {
            vec![ScriptedConnection::new().request(move |request| {
                *seen.lock().expect("seen") = Some(request.request_id());
                ScriptedAnswer::Close
            })]
        }
    });
    let client = connect(root.path(), IO);
    let first_epoch = old.owner_epoch();
    // The restart writes the new runtime-info before the frame goes out.
    let new = ScriptedRuntime::start(root.path(), {
        let seen = Arc::clone(&seen);
        move |owner_epoch| {
            vec![ScriptedConnection::new().request(move |request| {
                let r = seen_id(&seen);
                status_answer(request, status_of(owner_epoch, Some(held(r)), None))
            })]
        }
    });
    assert_ne!(new.owner_epoch(), first_epoch);
    let error = client
        .acquire_player_hold(INSTANCE, 1_000)
        .expect_err("dropped");
    assert_eq!(
        error.code(),
        ClientCode::RuntimePlayerHoldSettled.code(),
        "{error:?}"
    );
    assert_eq!(decision(&error).0, PlayerHoldSettlementRule::A11);
    assert!(format!("{error:?}").contains("runtime_owner_epoch_changed"));
    let after = client.status().expect_err("latched");
    assert_eq!(after.code(), ClientCode::RuntimeOwnerEpochChanged.code());
    drop(client);
    old.finish();
    let record = new.finish();
    assert!(record.unscripted.is_empty(), "{:?}", record.unscripted);
}

// 3: nothing can be read after the receipt was lost (the reconnect is never answered): P1, not
// final, and the handle stays latched.
#[test]
fn r7a_an_unreadable_status_is_p1_and_the_handle_stays_latched() {
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), |_| {
        vec![ScriptedConnection::new().request(|_| ScriptedAnswer::Close)]
    });
    let client = connect(root.path(), Duration::from_millis(250));
    let error = client
        .acquire_player_hold(INSTANCE, 1_000)
        .expect_err("dropped");
    assert_eq!(
        error.code(),
        ClientCode::RuntimePlayerHoldUnconfirmed.code(),
        "{error:?}"
    );
    assert_eq!(
        decision(&error),
        (
            PlayerHoldSettlementRule::P1,
            PlayerHoldSettlementVerdict::Unconfirmed,
            false
        )
    );
    assert!(client.status().is_err(), "latched");
    drop(client);
    let record = runtime.finish();
    assert_eq!(record.unscripted.len(), 1, "the reconnect, never answered");
}

// 2: a real receipt is never settled: R-7a's `not_implemented`, and `player_hold_interrupted`
// (Failed, projection `runtime_unavailable`), are returned as they are, not uncertain and without
// a settlement; the stored `attempt(reopened)` settles A11 once the restarted Runtime shows the
// hold restored.
#[test]
fn r7a_a_real_receipt_is_returned_unsettled() {
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), |_| {
        vec![ScriptedConnection::new().request(|request| {
            let code = Code::relayed("player_hold_interrupted").expect("registered");
            let outcome = RuntimeErrorProjection::new(RuntimeErrorCode::RuntimeUnavailable, false)
                .receipt_outcome(Some(code), None);
            ScriptedAnswer::failed(request, RuntimeReceiptState::Failed, outcome)
        })]
    });
    let client = connect(root.path(), IO);
    let prepared = client
        .prepare_acquire_player_hold(INSTANCE, 1_000)
        .expect("prepared");
    let stored = prepared.attempt(PlayerHoldSettlementTrigger::Reopened);
    let stored: PlayerHoldAttempt =
        serde_json::from_value(serde_json::to_value(&stored).expect("store")).expect("load");
    let error = client
        .submit_acquire_player_hold(prepared)
        .expect_err("interrupted");
    drop(client);
    assert_eq!(
        error
            .receipt_outcome()
            .map(|outcome| outcome.code().as_str()),
        Some("player_hold_interrupted")
    );
    assert_ne!(error.disposition(), RuntimeClientErrorClass::Uncertain);
    assert!(error.player_hold_settlement().is_none());
    assert!(error.player_hold_attempt().is_none());
    runtime.finish();
    let restarted = status_of(
        *ids().mint_owner_epoch().expect("epoch").transport(),
        Some(PlayerHoldState {
            reached_held_unix_ms: None,
            restored_from_owner_epoch: Some(stored.sent_in_owner_epoch),
            restored_from_phase: Some(actingcommand_contract::PlayerHoldRestoredFrom::Acquiring),
            ..held(stored.request_id)
        }),
        None,
    );
    let PlayerHoldSettlement::Decided(settled) =
        settle_player_hold(Some(&stored), INSTANCE, Some(&restarted))
    else {
        panic!("a decision");
    };
    assert_eq!(settled.rule, PlayerHoldSettlementRule::A11);
    assert!(settled.restored);

    // R-7a's Runtime answers `not_implemented`: returned as it is, sent once.
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), |_| {
        vec![ScriptedConnection::new().request(|request| {
            let outcome = RuntimeErrorProjection::new(RuntimeErrorCode::InvalidRequest, false)
                .receipt_outcome(
                    Some(actingcommand_contract::codes::SharedCode::NotImplemented.code()),
                    None,
                );
            ScriptedAnswer::failed(request, RuntimeReceiptState::Denied, outcome)
        })]
    });
    let client = connect(root.path(), IO);
    let error = client
        .release_player_hold(
            INSTANCE,
            expected(
                runtime.owner_epoch(),
                *ids().mint_request_id().expect("id").transport(),
            ),
            None,
        )
        .expect_err("not implemented");
    drop(client);
    assert_eq!(
        error
            .receipt_outcome()
            .map(|outcome| outcome.code().as_str()),
        Some("not_implemented")
    );
    assert!(error.player_hold_settlement().is_none());
    runtime.finish();
}

// 4: the two settlement codes' class, and the accessors on other errors.
#[test]
fn r7a_the_settlement_codes_have_their_class() {
    let settled = RuntimeClientError::fatal(
        ClientCode::RuntimePlayerHoldSettled,
        actingcommand_contract::codes::ContractLocation::AcquirePlayerHold,
    )
    .with_related(RuntimeClientError::fatal(
        ClientCode::RuntimeReceiptTimeout,
        actingcommand_contract::codes::ContractLocation::AcquirePlayerHold,
    ));
    assert!(settled.is_fatal());
    assert_eq!(settled.disposition(), RuntimeClientErrorClass::Runtime);
    let unconfirmed = RuntimeClientError::fatal(
        ClientCode::RuntimePlayerHoldUnconfirmed,
        actingcommand_contract::codes::ContractLocation::ReleasePlayerHold,
    );
    assert!(unconfirmed.is_fatal());
    assert_eq!(
        unconfirmed.disposition(),
        RuntimeClientErrorClass::Uncertain
    );
    assert!(settled.player_hold_settlement().is_none());
    assert!(unconfirmed.player_hold_attempt().is_none());
}

// 5: the socket read timeout is the whole formula, the deadline the formula without the IO
// margin, for each request.
#[test]
fn r7a_the_receipt_waits_are_the_contracts_formulas() {
    let io = Duration::from_millis(5_000);
    let ids = ids();
    let acquire = RuntimeOperation::AcquirePlayerHold {
        instance_alias: INSTANCE.to_owned(),
        drain_timeout_ms: 60_000,
    };
    let release = RuntimeOperation::ReleasePlayerHold {
        instance_alias: INSTANCE.to_owned(),
        expected: expected(
            *ids.mint_owner_epoch().expect("epoch").transport(),
            *ids.mint_request_id().expect("id").transport(),
        ),
        startup_package: None,
    };
    for (operation, deadline) in [(acquire, 120_000), (release, 33_000)] {
        let waits = player_hold_receipt_waits(&operation, io)
            .expect("a hold request")
            .expect("no overflow");
        assert_eq!(waits.deadline, Duration::from_millis(deadline));
        assert_eq!(waits.socket, Duration::from_millis(deadline) + io);
        assert_eq!(
            crate::client::receipt_response_timeout(&operation, io, Duration::from_secs(60))
                .expect("socket timeout"),
            waits.socket
        );
    }
    assert!(player_hold_receipt_waits(&RuntimeOperation::Status, io).is_none());
}

// 2: at the IPC layer, a connection that ends before the deadline is `runtime_receipt_header_failed`
// (end of stream: `dropped`), and one that ends after it is the deadline's timeout code.
#[test]
fn r7a_a_close_before_the_deadline_is_dropped_and_after_it_is_a_timeout() {
    let ids = ids();
    for (deadline, code) in [
        (
            unreached_deadline(),
            ClientCode::RuntimeReceiptHeaderFailed.code(),
        ),
        (reached_deadline(), ClientCode::RuntimeReceiptTimeout.code()),
    ] {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let address = listener.local_addr().expect("address");
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut header = [0_u8; 4];
            stream.read_exact(&mut header).expect("frame header");
            let mut body = vec![0_u8; u32::from_be_bytes(header) as usize];
            stream.read_exact(&mut body).expect("frame body");
        });
        let mut stream = TcpStream::connect(address).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(60)))
            .expect("read timeout");
        let request = RuntimeRequest::new(
            ids.mint_request_id().expect("request id"),
            ids.mint_correlation_id().expect("correlation id"),
            None,
            EventActor::Cli,
            EventSource::Cli,
            1,
            RuntimeOperation::Status,
        )
        .expect("request");
        let error = exchange::<_, RuntimeReceipt>(
            &mut stream,
            &request,
            1024 * 1024,
            Some(ReceiptReadDeadline::at(
                deadline,
                ClientCode::RuntimeReceiptTimeout,
            )),
            None,
            None,
        )
        .expect_err("the peer closed");
        peer.join().expect("peer");
        assert_eq!(error.code(), code);
    }
}
