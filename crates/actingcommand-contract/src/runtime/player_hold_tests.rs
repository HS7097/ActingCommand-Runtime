// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-7a (register §1.233; contracts/player-hold.md; design R-7a §7.1 "Tests"): the
// player hold's frozen shapes, their self-checks, the source gate, the one order of the control
// gates, `with_derived_control` and O-7, `instance_gates`, `hold_ended_with_pause`, every row of
// the settlement table and the two receipt waits. Pure: no clock, no I/O.

use super::*;
use crate::codes::{ReceiptCode, RuntimeContractCode};
use crate::outcome::Code;
use crate::{
    EventActor, EventSource, IdentifierIssuer, InstancePauseStage, InstancePauseState,
    InstanceResourceFence, RuntimeOperation, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest,
    RuntimeResult, RuntimeStartReport, SchedulingPauseReason, SchedulingPauseState,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const A: &str = "node.a";
const B: &str = "node.b";

fn ids() -> IdentifierIssuer {
    IdentifierIssuer::new().expect("identifier issuer")
}

fn request_id(ids: &IdentifierIssuer) -> RequestId {
    *ids.mint_request_id().expect("request id").transport()
}

fn epoch(ids: &IdentifierIssuer) -> OwnerEpoch {
    *ids.mint_owner_epoch().expect("owner epoch").transport()
}

fn refused<T: std::fmt::Debug>(result: RuntimeContractResult<T>, code: RuntimeContractCode) {
    let error = result.expect_err("refused");
    assert_eq!(error.code(), Code::from(code), "{error}");
}

/// Serializes, reads back, and checks the value is equal and its JSON keeps its keys.
fn round_trip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(value: &T) -> Value {
    let json = serde_json::to_value(value).expect("encode");
    let back: T = serde_json::from_value(json.clone()).expect("decode");
    assert_eq!(&back, value);
    json
}

/// A member the shape does not know is refused.
fn refuses_unknown<T: DeserializeOwned>(mut json: Value) {
    json.as_object_mut()
        .expect("an object")
        .insert("unknown_member".to_owned(), json!(1));
    assert!(serde_json::from_value::<T>(json).is_err());
}

fn cli() -> PlayerHoldSource {
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
        source: cli(),
        drain_deadline_unix_ms: None,
        grace_deadline_unix_ms: None,
        close_started_unix_ms: None,
        reached_held_unix_ms: Some(2_000),
        restored_from_owner_epoch: None,
        restored_from_phase: None,
    }
}

fn acquiring(hold_id: RequestId, phase: PlayerHoldPhase) -> PlayerHoldState {
    PlayerHoldState {
        phase,
        drain_deadline_unix_ms: Some(61_000),
        grace_deadline_unix_ms: Some(91_000),
        reached_held_unix_ms: None,
        ..held(hold_id)
    }
}

fn in_phase(hold_id: RequestId, phase: PlayerHoldPhase) -> PlayerHoldState {
    if phase.is_acquiring() {
        acquiring(hold_id, phase)
    } else {
        PlayerHoldState {
            phase,
            ..held(hold_id)
        }
    }
}

fn selfcheck() -> SchedulingResumeSelfCheck {
    serde_json::from_value(json!({
        "capture": {"ok": true, "width": 1280, "height": 720, "capture_backend": "fake"},
        "touch": {"ok": true, "backend": "fake", "max_x": 1280, "max_y": 720, "invasive": false},
        "failure_code": null
    }))
    .expect("self-check")
}

fn code_str(spelling: &str) -> CodeStr {
    serde_json::from_value(json!(spelling)).expect("code")
}

fn release_result(next: PlayerHoldNext) -> PlayerHoldOperationResult {
    PlayerHoldOperationResult {
        revision: Some(2),
        next: Some(next),
        selfcheck: (next == PlayerHoldNext::Reconnected).then(selfcheck),
        startup_package: Some(PlayerHoldStartupPackage::None),
        pauses_at_end: match next {
            PlayerHoldNext::Reconnected => None,
            PlayerHoldNext::InstancePaused => Some(PlayerHoldPausesAtEnd {
                instance_revision: Some(1),
                global_revision: None,
            }),
            PlayerHoldNext::SchedulingPaused => Some(PlayerHoldPausesAtEnd {
                instance_revision: None,
                global_revision: Some(1),
            }),
        },
        ..PlayerHoldOperationResult::default()
    }
}

/// A record of `kind` in `state`, with the members its state asks for.
fn record(
    kind: PlayerHoldOperationKind,
    state: PlayerHoldOperationState,
    request_id: RequestId,
    hold_id: RequestId,
) -> PlayerHoldOperation {
    let mut record = PlayerHoldOperation {
        request_id,
        kind,
        state,
        phase: None,
        hold_id,
        source: cli(),
        since_unix_ms: 1_000,
        ended_unix_ms: None,
        code: None,
        failed_phase: None,
        result: None,
    };
    match state {
        PlayerHoldOperationState::Running => {
            record.phase = Some(match kind {
                PlayerHoldOperationKind::Acquire => PlayerHoldPhase::Draining,
                PlayerHoldOperationKind::Release => PlayerHoldPhase::Releasing,
            });
        }
        PlayerHoldOperationState::Completed => {
            record.ended_unix_ms = Some(3_000);
            record.result = Some(match kind {
                PlayerHoldOperationKind::Acquire => PlayerHoldOperationResult {
                    revision: Some(1),
                    drained: Some(SchedulingDrainSummary {
                        finished: 1,
                        cancelled: 0,
                    }),
                    ..PlayerHoldOperationResult::default()
                },
                PlayerHoldOperationKind::Release => release_result(PlayerHoldNext::Reconnected),
            });
        }
        PlayerHoldOperationState::Failed => {
            record.ended_unix_ms = Some(3_000);
            record.code = Some(code_str("player_hold_drain_incomplete"));
            record.failed_phase = Some(match kind {
                PlayerHoldOperationKind::Acquire => PlayerHoldPhase::Draining,
                PlayerHoldOperationKind::Release => PlayerHoldPhase::Releasing,
            });
        }
    }
    record
}

fn pause(revision: u64) -> InstancePauseState {
    InstancePauseState {
        revision,
        pause_reason: SchedulingPauseReason::new("operator.stop").expect("reason"),
        since_unix_ms: 10,
        stage: InstancePauseStage::Released,
    }
}

fn global(revision: u64) -> SchedulingPauseState {
    SchedulingPauseState {
        revision,
        pause_reason: SchedulingPauseReason::new("operator.all").expect("reason"),
        since_unix_ms: 10,
    }
}

fn fence() -> InstanceResourceFence {
    serde_json::from_value(json!({
        "cause": "instance_resources_fenced",
        "since_unix_ms": 5,
        "awaiting_person": true
    }))
    .expect("fence")
}

/// One instance with the members named.
fn instance(
    ids: &IdentifierIssuer,
    alias: &str,
    hold: Option<PlayerHoldState>,
    operation: Option<PlayerHoldOperation>,
    paused: Option<u64>,
    fenced: bool,
) -> RuntimeInstanceStatus {
    RuntimeInstanceStatus::new(
        alias,
        *ids.mint_instance_id().expect("instance id").transport(),
        false,
        0,
        false,
        false,
        false,
    )
    .expect("instance")
    .with_pause(paused.map(pause))
    .expect("pause")
    .with_resource_fence(fenced.then(fence))
    .with_player_hold(hold)
    .expect("hold")
    .with_player_hold_operation(operation)
    .expect("record")
}

fn status(
    owner_epoch: OwnerEpoch,
    instances: Vec<RuntimeInstanceStatus>,
    global_revision: Option<u64>,
) -> RuntimeControlPlaneStatus {
    RuntimeControlPlaneStatus::new(owner_epoch, instances)
        .expect("status")
        .with_scheduling_pause(global_revision.map(global))
        .expect("global pause")
}

fn gates(dispatch: &[InstanceGate], device: &[InstanceGate]) -> ControlGates {
    ControlGates {
        dispatch: dispatch.to_vec(),
        device: device.to_vec(),
    }
}

fn request_as(
    ids: &IdentifierIssuer,
    actor: EventActor,
    source: EventSource,
    operation: RuntimeOperation,
) -> RuntimeContractResult<RuntimeRequest> {
    RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        actor,
        source,
        1,
        operation,
    )
}

fn acquire_operation(drain_timeout_ms: u64) -> RuntimeOperation {
    RuntimeOperation::AcquirePlayerHold {
        instance_alias: A.to_owned(),
        drain_timeout_ms,
    }
}

fn release_operation(expected: PlayerHoldExpectation) -> RuntimeOperation {
    RuntimeOperation::ReleasePlayerHold {
        instance_alias: A.to_owned(),
        expected,
        startup_package: None,
    }
}

fn released(next: PlayerHoldNext) -> PlayerHoldReleased {
    let result = release_result(next);
    PlayerHoldReleased {
        instance_alias: A.to_owned(),
        hold_id: request_id(&ids()),
        revision: 2,
        next,
        selfcheck: result.selfcheck,
        startup_package: PlayerHoldStartupPackage::None,
        startup_package_withheld: None,
        pauses_at_end: result.pauses_at_end,
    }
}

// ---- Shapes ----

#[test]
fn r7a_every_shape_round_trips_refuses_an_unknown_member_and_leaves_no_key_when_empty() {
    let ids = ids();
    let hold_id = request_id(&ids);
    let expected = PlayerHoldExpectation {
        owner_epoch: epoch(&ids),
        hold_id,
        revision: 1,
    };
    // The two requests.
    for operation in [
        acquire_operation(60_000),
        release_operation(expected),
        RuntimeOperation::ReleasePlayerHold {
            instance_alias: A.to_owned(),
            expected,
            startup_package: Some(PlayerHoldStartupChoice::Skip),
        },
    ] {
        let json = serde_json::to_value(&operation).expect("encode");
        let back: RuntimeOperation = serde_json::from_value(json.clone()).expect("decode");
        assert_eq!(back, operation);
        refuses_unknown::<RuntimeOperation>(json);
    }
    let release = serde_json::to_value(release_operation(expected)).expect("encode");
    assert!(release.get("startup_package").is_none(), "{release}");
    refuses_unknown::<PlayerHoldExpectation>(round_trip(&expected));

    // The hold, with its optional members present and absent.
    let mut full = acquiring(hold_id, PlayerHoldPhase::HandingBack);
    full.close_started_unix_ms = Some(70_000);
    let json = round_trip(&full);
    assert_eq!(json["close_started_unix_ms"], json!(70_000));
    refuses_unknown::<PlayerHoldState>(json);
    let plain = round_trip(&held(hold_id));
    assert_eq!(plain["reached_held_unix_ms"], json!(2_000));
    for absent in [
        "drain_deadline_unix_ms",
        "grace_deadline_unix_ms",
        "close_started_unix_ms",
        "restored_from_owner_epoch",
        "restored_from_phase",
    ] {
        assert!(plain.get(absent).is_none(), "{absent}: {plain}");
    }
    let restored_acquiring = PlayerHoldState {
        reached_held_unix_ms: None,
        restored_from_owner_epoch: Some(epoch(&ids)),
        restored_from_phase: Some(PlayerHoldRestoredFrom::Acquiring),
        ..held(hold_id)
    };
    assert!(
        round_trip(&restored_acquiring)
            .get("reached_held_unix_ms")
            .is_none()
    );

    // The records, the results and the attempt.
    for kind in [
        PlayerHoldOperationKind::Acquire,
        PlayerHoldOperationKind::Release,
    ] {
        for state in [
            PlayerHoldOperationState::Running,
            PlayerHoldOperationState::Completed,
            PlayerHoldOperationState::Failed,
        ] {
            let json = round_trip(&record(kind, state, hold_id, hold_id));
            refuses_unknown::<PlayerHoldOperation>(json);
        }
    }
    let acquired = PlayerHoldAcquired {
        hold: held(hold_id),
        drained: SchedulingDrainSummary {
            finished: 0,
            cancelled: 1,
        },
    };
    refuses_unknown::<PlayerHoldAcquired>(round_trip(&acquired));
    for next in [
        PlayerHoldNext::Reconnected,
        PlayerHoldNext::InstancePaused,
        PlayerHoldNext::SchedulingPaused,
    ] {
        let json = round_trip(&released(next));
        if next == PlayerHoldNext::Reconnected {
            assert!(json.get("pauses_at_end").is_none());
            assert!(json.get("startup_package_withheld").is_none());
        } else {
            assert!(json.get("selfcheck").is_none());
        }
        refuses_unknown::<PlayerHoldReleased>(json);
    }
    let control = InstanceControl {
        code: ControlGateCode::DispatchPausedInstance,
        dispatch: vec![InstanceGate::InstancePause],
        device: Vec::new(),
    };
    assert_eq!(
        round_trip(&control),
        json!({"code": "dispatch_paused_instance", "dispatch": ["instance_pause"], "device": []})
    );
    refuses_unknown::<InstanceControl>(round_trip(&control));

    // The results inside a receipt, their members beside `kind`.
    let request = request_as(
        &ids,
        EventActor::Cli,
        EventSource::Cli,
        acquire_operation(1_000),
    )
    .expect("request");
    let receipt = RuntimeReceipt::success(
        &request,
        RuntimeReceiptState::Completed,
        None,
        RuntimeResult::PlayerHoldAcquired(Box::new(acquired)),
    )
    .expect("receipt");
    let json = serde_json::to_value(&receipt).expect("receipt JSON");
    assert_eq!(json["result"]["kind"], json!("player_hold_acquired"));
    assert!(json["result"].get("hold").is_some());
    let back: RuntimeReceipt = serde_json::from_value(json).expect("receipt decodes");
    assert_eq!(back, receipt);
}

#[test]
fn r7a_status_members_round_trip_and_an_instance_without_them_keeps_its_keys() {
    let ids = ids();
    let hold_id = request_id(&ids);
    let bare = instance(&ids, A, None, None, None, false);
    let json = serde_json::to_value(&bare).expect("encode");
    for absent in ["player_hold", "player_hold_operation", "control"] {
        assert!(json.get(absent).is_none(), "{absent}: {json}");
    }
    let with_hold = instance(
        &ids,
        B,
        Some(held(hold_id)),
        Some(record(
            PlayerHoldOperationKind::Acquire,
            PlayerHoldOperationState::Completed,
            hold_id,
            hold_id,
        )),
        None,
        false,
    );
    let full = status(epoch(&ids), vec![bare, with_hold], None)
        .with_derived_control()
        .expect("derived");
    let json = round_trip(&full);
    assert_eq!(
        json["instances"][1]["control"]["code"],
        json!("dispatch_player_held")
    );
    assert_eq!(
        json["instances"][1]["control"]["device"],
        json!(["player_hold"])
    );
    refuses_unknown::<RuntimeInstanceStatus>(json["instances"][1].clone());
    let back: RuntimeControlPlaneStatus = serde_json::from_value(json).expect("decode");
    back.validate().expect("read back, O-7 holds");
}

#[test]
fn r7a_start_report_members_are_absent_when_empty_and_an_earlier_report_reads() {
    let mut report: RuntimeStartReport = serde_json::from_value(json!({
        "owner_epoch": serde_json::to_value(epoch(&ids())).expect("epoch"),
        "started_at_unix_ms": 1,
        "open": {
            "phases": {"sql_read_ms": 0, "verify_ms": 0, "material_ms": 0, "restore_ms": 0,
                       "meta_ms": 0, "tail_ms": 0, "index_ms": 0, "import_ms": 0},
            "sum_ms": 0, "deadline_ms": 120000, "slow": false
        },
        "pauses_restored": 0,
        "instructions_restored": 0
    }))
    .expect("a report of an earlier build, without the three members");
    report.validate().expect("valid");
    assert!(report.player_holds_restored.is_empty() && !report.player_hold_not_durable);
    let json = serde_json::to_value(&report).expect("encode");
    for absent in [
        "player_holds_restored",
        "player_holds_dropped",
        "player_hold_not_durable",
    ] {
        assert!(json.get(absent).is_none(), "{absent}");
    }
    report.player_holds_restored = vec![A.to_owned()];
    report.player_holds_dropped = vec![B.to_owned()];
    report.player_hold_not_durable = true;
    report.validate().expect("valid");
    let back = round_trip(&report);
    assert_eq!(back["player_hold_not_durable"], json!(true));
    report.player_holds_dropped = vec![B.to_owned(), B.to_owned()];
    refused(report.validate(), RuntimeContractCode::InvalidStartReport);
}

#[test]
fn r7a_the_shapes_ruling_134_removed_do_not_decode() {
    let ids = ids();
    let cancel = json!({
        "operation": "cancel_player_hold_acquire",
        "instance_alias": A,
        "hold_id": serde_json::to_value(request_id(&ids)).expect("id")
    });
    assert!(serde_json::from_value::<RuntimeOperation>(cancel).is_err());
    assert!(serde_json::from_value::<PlayerHoldPhase>(json!("cancelling")).is_err());
    assert!(serde_json::from_value::<PlayerHoldOperationKind>(json!("cancel")).is_err());
}

#[test]
fn r7a_drain_bounds_revision_zero_and_a_malformed_hold_id() {
    let ids = ids();
    for (drain, accepted) in [
        (999, false),
        (1_000, true),
        (600_000, true),
        (600_001, false),
    ] {
        let request = request_as(
            &ids,
            EventActor::Cli,
            EventSource::Cli,
            acquire_operation(drain),
        );
        if accepted {
            request.expect("in bounds");
        } else {
            refused(request, RuntimeContractCode::InvalidPlayerHoldDrainTimeout);
        }
    }
    let expected = PlayerHoldExpectation {
        owner_epoch: epoch(&ids),
        hold_id: request_id(&ids),
        revision: 0,
    };
    refused(
        request_as(
            &ids,
            EventActor::Cli,
            EventSource::Cli,
            release_operation(expected),
        ),
        RuntimeContractCode::InvalidPlayerHoldExpectation,
    );
    let mut json = serde_json::to_value(PlayerHoldExpectation {
        revision: 1,
        ..expected
    })
    .expect("encode");
    json["hold_id"] = json!("request_not-hex");
    assert!(serde_json::from_value::<PlayerHoldExpectation>(json).is_err());
}

// ---- Self-checks ----

#[test]
fn r7a_the_operation_record_keeps_its_shape() {
    let ids = ids();
    let r = request_id(&ids);
    let state = |record: PlayerHoldOperation| record.validate();
    let mut running = record(
        PlayerHoldOperationKind::Acquire,
        PlayerHoldOperationState::Running,
        r,
        r,
    );
    running.validate().expect("running");
    running.phase = None;
    refused(state(running), RuntimeContractCode::InvalidPlayerHoldState);

    let failed = record(
        PlayerHoldOperationKind::Acquire,
        PlayerHoldOperationState::Failed,
        r,
        r,
    );
    failed.validate().expect("failed");
    refused(
        state(PlayerHoldOperation {
            code: None,
            ..failed.clone()
        }),
        RuntimeContractCode::InvalidPlayerHoldState,
    );
    refused(
        state(PlayerHoldOperation {
            failed_phase: None,
            ..failed
        }),
        RuntimeContractCode::InvalidPlayerHoldState,
    );

    let completed = record(
        PlayerHoldOperationKind::Release,
        PlayerHoldOperationState::Completed,
        request_id(&ids),
        r,
    );
    completed.validate().expect("completed");
    refused(
        state(PlayerHoldOperation {
            result: None,
            ..completed.clone()
        }),
        RuntimeContractCode::InvalidPlayerHoldState,
    );
    let mut reconnected_with_pauses = completed.clone();
    reconnected_with_pauses
        .result
        .as_mut()
        .expect("result")
        .pauses_at_end = Some(PlayerHoldPausesAtEnd {
        instance_revision: Some(1),
        global_revision: None,
    });
    refused(
        state(reconnected_with_pauses),
        RuntimeContractCode::InvalidPlayerHoldState,
    );

    let lab_via = PlayerHoldSource {
        actor: PlayerHoldSourceActor::Lab,
        channel: RuntimeChannel::Lab,
        via: Some(ChannelVia::Mcp),
    };
    state(PlayerHoldOperation {
        source: lab_via,
        ..completed.clone()
    })
    .expect("via on the Lab channel");
    let ui_via = PlayerHoldSource {
        channel: RuntimeChannel::Ui,
        ..lab_via
    };
    refused(
        state(PlayerHoldOperation {
            source: ui_via,
            ..completed
        }),
        RuntimeContractCode::InvalidPlayerHoldState,
    );
    refused(
        PlayerHoldState {
            source: ui_via,
            ..held(r)
        }
        .validate(),
        RuntimeContractCode::InvalidPlayerHoldState,
    );
}

/// `next` and `pauses_at_end` (R-7a §3.2): one group, checked on `PlayerHoldReleased`
/// (`invalid_player_hold_receipt`) and on a completed release record's result
/// (`invalid_player_hold_state`).
#[test]
fn r7a_next_and_pauses_at_end_agree_on_the_answer_and_on_the_record() {
    let ids = ids();
    let r = request_id(&ids);
    type Change = fn(&mut PlayerHoldReleased);
    let cases: [(&str, PlayerHoldNext, Change, bool); 9] = [
        (
            "instance_paused without instance_revision",
            PlayerHoldNext::InstancePaused,
            |answer| {
                answer.pauses_at_end = Some(PlayerHoldPausesAtEnd {
                    instance_revision: None,
                    global_revision: Some(1),
                });
            },
            false,
        ),
        (
            "instance_paused with both revisions",
            PlayerHoldNext::InstancePaused,
            |answer| {
                answer.pauses_at_end = Some(PlayerHoldPausesAtEnd {
                    instance_revision: Some(1),
                    global_revision: Some(1),
                });
            },
            true,
        ),
        (
            "scheduling_paused without global_revision",
            PlayerHoldNext::SchedulingPaused,
            |answer| {
                answer.pauses_at_end = Some(PlayerHoldPausesAtEnd {
                    instance_revision: None,
                    global_revision: None,
                });
            },
            false,
        ),
        (
            "scheduling_paused with instance_revision",
            PlayerHoldNext::SchedulingPaused,
            |answer| {
                answer.pauses_at_end = Some(PlayerHoldPausesAtEnd {
                    instance_revision: Some(1),
                    global_revision: Some(1),
                });
            },
            false,
        ),
        (
            "reconnected without selfcheck",
            PlayerHoldNext::Reconnected,
            |answer| {
                answer.selfcheck = None;
            },
            false,
        ),
        (
            "reconnected with startup_package_withheld",
            PlayerHoldNext::Reconnected,
            |answer| {
                answer.startup_package = PlayerHoldStartupPackage::Scheduled;
                answer.startup_package_withheld = Some(StartupPackageWithheld::InstancePaused);
            },
            false,
        ),
        (
            "a pause with selfcheck",
            PlayerHoldNext::InstancePaused,
            |answer| {
                answer.selfcheck = Some(selfcheck());
            },
            false,
        ),
        (
            "withheld without scheduled",
            PlayerHoldNext::InstancePaused,
            |answer| {
                answer.startup_package = PlayerHoldStartupPackage::None;
                answer.startup_package_withheld = Some(StartupPackageWithheld::InstancePaused);
            },
            false,
        ),
        (
            "withheld not equal to next",
            PlayerHoldNext::InstancePaused,
            |answer| {
                answer.startup_package = PlayerHoldStartupPackage::Scheduled;
                answer.startup_package_withheld = Some(StartupPackageWithheld::SchedulingPaused);
            },
            false,
        ),
    ];
    for (name, next, change, accepted) in cases {
        let mut answer = released(next);
        answer.validate().expect("the base answer");
        change(&mut answer);
        let mut on_record = record(
            PlayerHoldOperationKind::Release,
            PlayerHoldOperationState::Completed,
            request_id(&ids),
            r,
        );
        on_record.result = Some(PlayerHoldOperationResult {
            revision: Some(answer.revision),
            next: Some(answer.next),
            selfcheck: answer.selfcheck.clone(),
            startup_package: Some(answer.startup_package),
            startup_package_withheld: answer.startup_package_withheld,
            pauses_at_end: answer.pauses_at_end,
            drained: None,
        });
        if accepted {
            answer
                .validate()
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            on_record
                .validate()
                .unwrap_or_else(|error| panic!("{name}: {error}"));
        } else {
            refused(
                answer.validate(),
                RuntimeContractCode::InvalidPlayerHoldReceipt,
            );
            refused(
                on_record.validate(),
                RuntimeContractCode::InvalidPlayerHoldState,
            );
        }
    }
    // A withheld package that matches its pause is accepted.
    let mut withheld = released(PlayerHoldNext::InstancePaused);
    withheld.startup_package = PlayerHoldStartupPackage::Scheduled;
    withheld.startup_package_withheld = Some(StartupPackageWithheld::InstancePaused);
    withheld.validate().expect("withheld behind its pause");
}

#[test]
fn r7a_the_hold_keeps_its_own_shape() {
    let ids = ids();
    let h = request_id(&ids);
    held(h).validate().expect("held");
    acquiring(h, PlayerHoldPhase::Requesting)
        .validate()
        .expect("requesting");
    let invalid = |hold: PlayerHoldState| {
        refused(hold.validate(), RuntimeContractCode::InvalidPlayerHoldState);
    };
    invalid(PlayerHoldState {
        grace_deadline_unix_ms: None,
        ..acquiring(h, PlayerHoldPhase::Draining)
    });
    invalid(PlayerHoldState {
        drain_deadline_unix_ms: Some(61_000),
        ..held(h)
    });
    invalid(PlayerHoldState {
        reached_held_unix_ms: None,
        ..held(h)
    });
    invalid(PlayerHoldState {
        restored_from_owner_epoch: Some(epoch(&ids)),
        restored_from_phase: Some(PlayerHoldRestoredFrom::Acquiring),
        ..held(h)
    });
    invalid(PlayerHoldState {
        reached_held_unix_ms: None,
        restored_from_owner_epoch: Some(epoch(&ids)),
        restored_from_phase: Some(PlayerHoldRestoredFrom::Held),
        ..held(h)
    });
    invalid(PlayerHoldState {
        restored_from_phase: Some(PlayerHoldRestoredFrom::Held),
        ..held(h)
    });
    invalid(PlayerHoldState {
        close_started_unix_ms: Some(70_000),
        ..held(h)
    });
    PlayerHoldState {
        reached_held_unix_ms: None,
        restored_from_owner_epoch: Some(epoch(&ids)),
        restored_from_phase: Some(PlayerHoldRestoredFrom::Acquiring),
        ..held(h)
    }
    .validate()
    .expect("restored from a stop during its acquire");
}

#[test]
fn r7a_every_channel_and_via_round_trips_and_a_missing_channel_does_not_decode() {
    let ids = ids();
    let h = request_id(&ids);
    for channel in [
        RuntimeChannel::Ui,
        RuntimeChannel::Cli,
        RuntimeChannel::Mcp,
        RuntimeChannel::Lab,
    ] {
        let source = PlayerHoldSource {
            actor: PlayerHoldSourceActor::User,
            channel,
            via: None,
        };
        round_trip(&source);
        source.validate().expect("no via");
    }
    for via in [ChannelVia::Mcp, ChannelVia::Ui] {
        let source = PlayerHoldSource {
            actor: PlayerHoldSourceActor::Lab,
            channel: RuntimeChannel::Lab,
            via: Some(via),
        };
        round_trip(&source);
        source.validate().expect("via on Lab");
    }
    let mut hold = serde_json::to_value(held(h)).expect("hold");
    hold["source"]
        .as_object_mut()
        .expect("source")
        .remove("channel");
    assert!(serde_json::from_value::<PlayerHoldState>(hold).is_err());
    let mut operation = serde_json::to_value(record(
        PlayerHoldOperationKind::Acquire,
        PlayerHoldOperationState::Running,
        h,
        h,
    ))
    .expect("record");
    operation["source"]
        .as_object_mut()
        .expect("source")
        .remove("channel");
    assert!(serde_json::from_value::<PlayerHoldOperation>(operation).is_err());
}

#[test]
fn r7a_a_receipt_carries_a_hold_result_only_completed() {
    let ids = ids();
    let request = request_as(
        &ids,
        EventActor::Cli,
        EventSource::Cli,
        acquire_operation(1_000),
    )
    .expect("request");
    let in_acquire = PlayerHoldAcquired {
        hold: acquiring(request.request_id(), PlayerHoldPhase::Draining),
        drained: SchedulingDrainSummary {
            finished: 0,
            cancelled: 0,
        },
    };
    refused(
        RuntimeReceipt::success(
            &request,
            RuntimeReceiptState::Completed,
            None,
            RuntimeResult::PlayerHoldAcquired(Box::new(in_acquire)),
        ),
        RuntimeContractCode::InvalidPlayerHoldReceipt,
    );
    assert_eq!(
        RuntimeOperation::AcquirePlayerHold {
            instance_alias: A.to_owned(),
            drain_timeout_ms: 1_000
        }
        .kind()
        .success_code(),
        Some(ReceiptCode::AcquirePlayerHoldCompleted.code())
    );
}

// ---- The source gate ----

#[test]
fn r7a_the_source_gate_takes_the_three_envelopes_for_both_requests() {
    let ids = ids();
    let expected = PlayerHoldExpectation {
        owner_epoch: epoch(&ids),
        hold_id: request_id(&ids),
        revision: 1,
    };
    for operation in [acquire_operation(60_000), release_operation(expected)] {
        for (actor, source) in [
            (EventActor::User, EventSource::Ui),
            (EventActor::Cli, EventSource::Cli),
            (EventActor::Lab, EventSource::Lab),
        ] {
            request_as(&ids, actor, source, operation.clone()).expect("taken");
        }
        for (actor, source) in [
            (EventActor::Agent, EventSource::Adapter),
            (EventActor::User, EventSource::Cli),
            (EventActor::Cli, EventSource::Lab),
            (EventActor::Ui, EventSource::Ui),
        ] {
            refused(
                request_as(&ids, actor, source, operation.clone()),
                RuntimeContractCode::InvalidPlayerHoldOrigin,
            );
        }
    }
}

// ---- The one order (§2.6) ----

#[test]
fn r7a_control_gates_evaluate_every_combination_in_one_order() {
    use InstanceGate::{GlobalPause, InstancePause, PlayerHold, ResourceFence};
    for bits in 0_u8..16 {
        let inputs = ControlInputs {
            player_hold: bits & 1 != 0,
            instance_pause: bits & 2 != 0,
            global_pause: bits & 4 != 0,
            resource_fence: bits & 8 != 0,
        };
        let mut dispatch = Vec::new();
        if inputs.player_hold {
            dispatch.push(PlayerHold);
        }
        if inputs.instance_pause {
            dispatch.push(InstancePause);
        }
        if inputs.global_pause {
            dispatch.push(GlobalPause);
        }
        if inputs.resource_fence {
            dispatch.push(ResourceFence);
        }
        let evaluated = ControlGates::evaluate(inputs);
        assert_eq!(evaluated.dispatch, dispatch, "{inputs:?}");
        assert_eq!(
            evaluated.device,
            if inputs.player_hold {
                vec![PlayerHold]
            } else {
                Vec::new()
            },
            "{inputs:?}"
        );
        let code = if inputs.player_hold {
            Some(ControlGateCode::DispatchPlayerHeld)
        } else if inputs.instance_pause {
            Some(ControlGateCode::DispatchPausedInstance)
        } else if inputs.global_pause {
            Some(ControlGateCode::DispatchPausedGlobal)
        } else if inputs.resource_fence {
            Some(ControlGateCode::DispatchResourceFenced)
        } else {
            None
        };
        assert_eq!(evaluated.code(), code, "{inputs:?}");
    }
    // The vocabulary's order is the table's.
    assert_eq!(
        InstanceGate::ALL,
        &[PlayerHold, InstancePause, GlobalPause, ResourceFence]
    );
}

/// O-7: a `control` read in agrees with the members, the global pause included.
#[test]
fn r7a_validate_checks_a_control_read_in_against_the_members() {
    let ids = ids();
    let paused = status(
        epoch(&ids),
        vec![instance(&ids, A, None, None, Some(1), false)],
        Some(1),
    );
    let derived = serde_json::to_value(paused.clone().with_derived_control().expect("derived"))
        .expect("encode");
    let read = |json: Value| {
        serde_json::from_value::<RuntimeControlPlaneStatus>(json)
            .expect("decodes")
            .validate()
    };
    read(derived.clone()).expect("agrees with the members");
    let with_control = |control: Value| {
        let mut json = derived.clone();
        json["instances"][0]["control"] = control;
        json
    };
    for control in [
        // one gate missing
        json!({"code": "dispatch_paused_instance", "dispatch": ["instance_pause"], "device": []}),
        // one gate too many
        json!({"code": "dispatch_paused_instance",
               "dispatch": ["instance_pause", "global_pause", "resource_fence"], "device": []}),
        // out of order
        json!({"code": "dispatch_paused_instance",
               "dispatch": ["global_pause", "instance_pause"], "device": []}),
        // the wrong code
        json!({"code": "dispatch_paused_global",
               "dispatch": ["instance_pause", "global_pause"], "device": []}),
        // no dispatch gate
        json!({"code": "dispatch_paused_instance", "dispatch": [], "device": []}),
        // a device gate without a hold
        json!({"code": "dispatch_paused_instance",
               "dispatch": ["instance_pause", "global_pause"], "device": ["player_hold"]}),
    ] {
        refused(
            read(with_control(control)),
            RuntimeContractCode::InvalidRuntimeInstanceControl,
        );
    }
    // An R-7a or older Runtime fills no control: an instance pause without it is accepted.
    let mut absent = derived.clone();
    absent["instances"][0]
        .as_object_mut()
        .expect("instance")
        .remove("control");
    read(absent).expect("no control, not checked");
    // The global pause sits at the top: a control without it is refused, so the check crosses
    // the members.
    let only_global = status(
        epoch(&ids),
        vec![instance(&ids, A, None, None, None, false)],
        Some(1),
    );
    let mut json = serde_json::to_value(only_global).expect("encode");
    json["instances"][0]["control"] =
        json!({"code": "dispatch_paused_instance", "dispatch": [], "device": []});
    refused(
        read(json.clone()),
        RuntimeContractCode::InvalidRuntimeInstanceControl,
    );
    json["instances"][0]["control"] =
        json!({"code": "dispatch_resource_fenced", "dispatch": ["resource_fence"], "device": []});
    refused(
        read(json),
        RuntimeContractCode::InvalidRuntimeInstanceControl,
    );
}

#[test]
fn r7a_with_derived_control_derives_from_the_members_in_any_order_of_the_calls() {
    let ids = ids();
    let owner_epoch = epoch(&ids);
    // Only the global pause.
    let instances = || {
        vec![
            instance(&ids, A, None, None, None, false),
            instance(&ids, B, None, None, None, false),
        ]
    };
    // `new` never fails on `control`: no instance can carry one before the derivation.
    let only_global = RuntimeControlPlaneStatus::new(owner_epoch, instances())
        .expect("new")
        .with_scheduling_pause(Some(global(1)))
        .expect("global pause")
        .with_derived_control()
        .expect("derived");
    for instance in only_global.instances() {
        assert_eq!(
            instance.control(),
            Some(&InstanceControl {
                code: ControlGateCode::DispatchPausedGlobal,
                dispatch: vec![InstanceGate::GlobalPause],
                device: Vec::new(),
            })
        );
    }
    only_global.validate().expect("valid");
    let back: RuntimeControlPlaneStatus =
        serde_json::from_value(serde_json::to_value(&only_global).expect("encode"))
            .expect("decode");
    back.validate().expect("O-7 on the way in");
    assert_eq!(back, only_global);

    // Every instance paused (116), and the global pause too.
    let paused = || {
        vec![
            instance(&ids, A, None, None, Some(1), false),
            instance(&ids, B, None, None, Some(1), false),
        ]
    };
    let expected = InstanceControl {
        code: ControlGateCode::DispatchPausedInstance,
        dispatch: vec![InstanceGate::InstancePause, InstanceGate::GlobalPause],
        device: Vec::new(),
    };
    let forward = status(owner_epoch, paused(), Some(1))
        .with_derived_control()
        .expect("derived");
    // The other order: derived first, then the global pause, which derives again.
    let reverse = status(owner_epoch, paused(), None)
        .with_derived_control()
        .expect("derived")
        .with_scheduling_pause(Some(global(1)))
        .expect("derived again");
    for status in [&forward, &reverse] {
        for instance in status.instances() {
            assert_eq!(instance.control(), Some(&expected));
        }
    }
    // The global pause lifted after the derivation: back to the instance pause alone.
    let lifted = reverse.with_scheduling_pause(None).expect("derived again");
    assert_eq!(
        lifted.instances()[0]
            .control()
            .map(|control| control.dispatch.clone()),
        Some(vec![InstanceGate::InstancePause])
    );

    // No gate: no control.
    let open = status(owner_epoch, instances(), None)
        .with_derived_control()
        .expect("derived");
    assert!(
        open.instances()
            .iter()
            .all(|instance| instance.control().is_none())
    );
    // Again: unchanged.
    assert_eq!(open.clone().with_derived_control().expect("again"), open);
    assert_eq!(
        forward.clone().with_derived_control().expect("again"),
        forward
    );
}

#[test]
fn r7a_instance_gates_reads_the_members_not_control() {
    let ids = ids();
    let r7a_runtime = status(
        epoch(&ids),
        vec![instance(&ids, A, None, None, Some(1), false)],
        None,
    );
    assert!(r7a_runtime.instances()[0].control().is_none());
    assert_eq!(
        r7a_runtime.instance_gates(A),
        Some(gates(&[InstanceGate::InstancePause], &[]))
    );
    assert_eq!(
        r7a_runtime.instance_gates(A).and_then(|gates| gates.code()),
        Some(ControlGateCode::DispatchPausedInstance)
    );
    assert_eq!(r7a_runtime.instance_gates(B), None);
    let derived = r7a_runtime.with_derived_control().expect("derived");
    let control = derived.instances()[0].control().expect("control");
    let gates = derived.instance_gates(A).expect("gates");
    assert_eq!(gates.dispatch, control.dispatch);
    assert_eq!(gates.device, control.device);
    assert_eq!(gates.code(), Some(control.code));
}

#[test]
fn r7a_hold_ended_with_pause_names_the_pause_it_ended_under() {
    let ids = ids();
    let owner_epoch = epoch(&ids);
    let h = request_id(&ids);
    let ended =
        |next: PlayerHoldNext, instance_revision: Option<u64>, global_revision: Option<u64>| {
            let mut record = record(
                PlayerHoldOperationKind::Release,
                PlayerHoldOperationState::Completed,
                request_id(&ids),
                h,
            );
            let result = record.result.as_mut().expect("result");
            result.next = Some(next);
            result.selfcheck = None;
            result.pauses_at_end = Some(PlayerHoldPausesAtEnd {
                instance_revision,
                global_revision,
            });
            if next == PlayerHoldNext::Reconnected {
                *result = release_result(PlayerHoldNext::Reconnected);
            }
            record
        };
    let case = |record: PlayerHoldOperation, pause: Option<u64>, global: Option<u64>| {
        status(
            owner_epoch,
            vec![instance(&ids, A, None, Some(record), pause, false)],
            global,
        )
        .hold_ended_with_pause(A)
    };
    // Ended under 116, 116 still there.
    assert!(case(
        ended(PlayerHoldNext::InstancePaused, Some(3), None),
        Some(3),
        None
    ));
    // 116 lifted, a new instance pause since: another revision.
    assert!(!case(
        ended(PlayerHoldNext::InstancePaused, Some(3), None),
        Some(5),
        None
    ));
    // Ended without a pause, then everything paused.
    assert!(!case(
        ended(PlayerHoldNext::Reconnected, None, None),
        Some(1),
        Some(1)
    ));
    // Ended under both, the instance pause lifted, the global one still there.
    assert!(case(
        ended(PlayerHoldNext::InstancePaused, Some(3), Some(7)),
        None,
        Some(7)
    ));
    // After a restart: no record in the new epoch.
    assert!(
        !status(
            owner_epoch,
            vec![instance(&ids, A, None, None, Some(3), false)],
            None
        )
        .hold_ended_with_pause(A)
    );
    // Only the global revision, and the global pause is another one now.
    assert!(!case(
        ended(PlayerHoldNext::SchedulingPaused, None, Some(7)),
        None,
        Some(8)
    ));
    // A failed release, and a completed acquire, are no ended hold.
    let failed = record(
        PlayerHoldOperationKind::Release,
        PlayerHoldOperationState::Failed,
        request_id(&ids),
        h,
    );
    assert!(!case(failed, Some(3), None));
    let acquired = record(
        PlayerHoldOperationKind::Acquire,
        PlayerHoldOperationState::Completed,
        h,
        h,
    );
    assert!(
        !status(
            owner_epoch,
            vec![instance(
                &ids,
                A,
                Some(held(h)),
                Some(acquired),
                Some(3),
                false
            )],
            None
        )
        .hold_ended_with_pause(A)
    );
}

// ---- The settlement table (§3.3.3) ----

use PlayerHoldOperationKind::{Acquire, Release};
use PlayerHoldOperationState::{Completed, Failed, Running};
use PlayerHoldSettlementRule as Rule;
use PlayerHoldSettlementTrigger::{Dropped, Reopened, TimedOut};

/// What the subject instance shows, and whether another instance carries a revision-4 sign.
struct Scene {
    hold: Option<PlayerHoldState>,
    record: Option<PlayerHoldOperation>,
    sign_elsewhere: bool,
}

struct Ids {
    issuer: IdentifierIssuer,
    e0: OwnerEpoch,
    e1: OwnerEpoch,
    r: RequestId,
    h: RequestId,
    other: RequestId,
}

fn table_ids() -> Ids {
    let issuer = ids();
    Ids {
        e0: epoch(&issuer),
        e1: epoch(&issuer),
        r: request_id(&issuer),
        h: request_id(&issuer),
        other: request_id(&issuer),
        issuer,
    }
}

fn attempt(
    ids: &Ids,
    kind: PlayerHoldOperationKind,
    trigger: PlayerHoldSettlementTrigger,
) -> PlayerHoldAttempt {
    PlayerHoldAttempt {
        kind,
        request_id: ids.r,
        sent_in_owner_epoch: ids.e0,
        expected_hold_id: match kind {
            Acquire => ids.r,
            Release => ids.h,
        },
        trigger,
    }
}

fn scene_status(ids: &Ids, owner_epoch: OwnerEpoch, scene: Scene) -> RuntimeControlPlaneStatus {
    let elsewhere = scene
        .sign_elsewhere
        .then(|| record(Acquire, Completed, ids.other, ids.other));
    status(
        owner_epoch,
        vec![
            instance(&ids.issuer, A, scene.hold, scene.record, None, false),
            instance(&ids.issuer, B, None, elsewhere, None, false),
        ],
        None,
    )
}

fn decide(
    ids: &Ids,
    attempt: &PlayerHoldAttempt,
    owner_epoch: OwnerEpoch,
    scene: Scene,
) -> PlayerHoldDecision {
    let status = scene_status(ids, owner_epoch, scene);
    match settle_player_hold(Some(attempt), A, Some(&status)) {
        PlayerHoldSettlement::Decided(decision) => *decision,
        PlayerHoldSettlement::Current(view) => panic!("a decision, got {view:?}"),
    }
}

fn scene(hold: Option<PlayerHoldState>, record: Option<PlayerHoldOperation>) -> Scene {
    Scene {
        hold,
        record,
        sign_elsewhere: false,
    }
}

fn signed(hold: Option<PlayerHoldState>, record: Option<PlayerHoldOperation>) -> Scene {
    Scene {
        hold,
        record,
        sign_elsewhere: true,
    }
}

/// Every row, every trigger it names, with its verdict, its value and `final`.
#[test]
fn r7a_settle_player_hold_decides_every_row_of_the_table() {
    use PlayerHoldSettlementOutcome as O;
    use PlayerHoldUnconfirmedObserved as U;
    enum Want {
        Settled(O),
        Unconfirmed(U),
        Unsupported,
    }
    let ids = table_ids();
    let (r, h, other) = (ids.r, ids.h, ids.other);
    let rows: Vec<(
        Rule,
        PlayerHoldOperationKind,
        PlayerHoldSettlementTrigger,
        bool,
        Scene,
        Want,
    )> = vec![
        // Acquire, E1 = E0.
        (
            Rule::A1,
            Acquire,
            TimedOut,
            true,
            scene(
                Some(acquiring(r, PlayerHoldPhase::Draining)),
                Some(record(Acquire, Running, r, r)),
            ),
            Want::Unconfirmed(U::InProgress),
        ),
        (
            Rule::A2,
            Acquire,
            Dropped,
            true,
            scene(Some(held(r)), Some(record(Acquire, Completed, r, r))),
            Want::Settled(O::Completed),
        ),
        (
            Rule::A3,
            Acquire,
            Reopened,
            true,
            scene(None, Some(record(Acquire, Failed, r, r))),
            Want::Settled(O::Failed),
        ),
        (
            Rule::A4,
            Acquire,
            TimedOut,
            true,
            scene(
                Some(held(r)),
                Some(record(Acquire, Completed, other, other)),
            ),
            Want::Settled(O::Completed),
        ),
        (
            Rule::A5,
            Acquire,
            TimedOut,
            true,
            scene(Some(acquiring(r, PlayerHoldPhase::Draining)), None),
            Want::Unconfirmed(U::Inconsistent),
        ),
        (
            Rule::A6,
            Acquire,
            TimedOut,
            true,
            scene(None, Some(record(Release, Completed, other, r))),
            Want::Settled(O::Completed),
        ),
        (
            Rule::A7,
            Acquire,
            Dropped,
            true,
            scene(None, None),
            Want::Unsupported,
        ),
        (
            Rule::A8,
            Acquire,
            Dropped,
            true,
            signed(None, None),
            Want::Settled(O::NotPerformed),
        ),
        (
            Rule::A9,
            Acquire,
            TimedOut,
            true,
            scene(None, None),
            Want::Unconfirmed(U::NotRegistered),
        ),
        (
            Rule::A9,
            Acquire,
            Reopened,
            true,
            signed(None, None),
            Want::Unconfirmed(U::NotRegistered),
        ),
        (
            Rule::A10,
            Acquire,
            TimedOut,
            true,
            scene(
                Some(held(other)),
                Some(record(Acquire, Completed, other, other)),
            ),
            Want::Unconfirmed(U::Superseded),
        ),
        // Acquire, E1 ≠ E0.
        (
            Rule::A11,
            Acquire,
            Reopened,
            false,
            scene(Some(held(r)), None),
            Want::Settled(O::Completed),
        ),
        (
            Rule::A12,
            Acquire,
            Reopened,
            false,
            scene(None, Some(record(Release, Completed, other, r))),
            Want::Settled(O::Completed),
        ),
        (
            Rule::A12,
            Acquire,
            Reopened,
            false,
            scene(None, Some(record(Release, Running, other, r))),
            Want::Settled(O::Completed),
        ),
        (
            Rule::A13,
            Acquire,
            Reopened,
            false,
            scene(None, None),
            Want::Unconfirmed(U::EpochChanged),
        ),
        // Release, E1 = E0.
        (
            Rule::R1,
            Release,
            TimedOut,
            true,
            scene(None, Some(record(Release, Running, r, h))),
            Want::Unconfirmed(U::InProgress),
        ),
        (
            Rule::R2,
            Release,
            Dropped,
            true,
            scene(None, Some(record(Release, Completed, r, h))),
            Want::Settled(O::Completed),
        ),
        (
            Rule::R3,
            Release,
            TimedOut,
            true,
            scene(Some(held(h)), Some(record(Release, Failed, r, h))),
            Want::Settled(O::Failed),
        ),
        (
            Rule::R4,
            Release,
            TimedOut,
            true,
            scene(
                Some(in_phase(h, PlayerHoldPhase::Releasing)),
                Some(record(Release, Running, other, h)),
            ),
            Want::Unconfirmed(U::InProgressOther),
        ),
        (
            Rule::R5,
            Release,
            Dropped,
            true,
            scene(Some(held(h)), None),
            Want::Settled(O::NotPerformed),
        ),
        (
            Rule::R6,
            Release,
            TimedOut,
            true,
            scene(Some(held(h)), None),
            Want::Unconfirmed(U::NotRegistered),
        ),
        (
            Rule::R6,
            Release,
            Reopened,
            true,
            scene(Some(held(h)), None),
            Want::Unconfirmed(U::NotRegistered),
        ),
        (
            Rule::R7,
            Release,
            TimedOut,
            true,
            scene(None, Some(record(Release, Completed, other, h))),
            Want::Settled(O::EndedByOther),
        ),
        (
            Rule::R8,
            Release,
            Dropped,
            true,
            scene(None, None),
            Want::Unsupported,
        ),
        (
            Rule::R9,
            Release,
            Dropped,
            true,
            signed(None, None),
            Want::Settled(O::HoldEnded),
        ),
        (
            Rule::R9,
            Release,
            TimedOut,
            true,
            scene(None, None),
            Want::Settled(O::HoldEnded),
        ),
        // Release, E1 ≠ E0.
        (
            Rule::R10,
            Release,
            Reopened,
            false,
            scene(Some(held(h)), None),
            Want::Settled(O::NotPerformed),
        ),
        (
            Rule::R11,
            Release,
            Reopened,
            false,
            scene(None, None),
            Want::Settled(O::HoldEnded),
        ),
    ];
    for (rule, kind, trigger, same_epoch, scene, want) in rows {
        let attempt = attempt(&ids, kind, trigger);
        let epoch = if same_epoch { ids.e0 } else { ids.e1 };
        let decision = decide(&ids, &attempt, epoch, scene);
        assert_eq!(decision.rule, rule, "{rule:?} {trigger:?}");
        assert_eq!(decision.attempt.as_ref(), Some(&attempt), "{rule:?}");
        assert_eq!(decision.status_owner_epoch, Some(epoch), "{rule:?}");
        match want {
            Want::Settled(outcome) => {
                assert_eq!(
                    decision.verdict,
                    PlayerHoldSettlementVerdict::Settled,
                    "{rule:?}"
                );
                assert_eq!(decision.settlement_outcome, Some(outcome), "{rule:?}");
                assert!(decision.is_final, "{rule:?}");
            }
            Want::Unconfirmed(observed) => {
                assert_eq!(
                    decision.verdict,
                    PlayerHoldSettlementVerdict::Unconfirmed,
                    "{rule:?}"
                );
                assert_eq!(decision.observed, Some(observed), "{rule:?}");
                assert_eq!(decision.is_final, observed.is_final(), "{rule:?}");
            }
            Want::Unsupported => {
                assert_eq!(
                    decision.verdict,
                    PlayerHoldSettlementVerdict::Unsupported,
                    "{rule:?}"
                );
                assert!(decision.is_final, "{rule:?}");
            }
        }
        assert_eq!(
            decision.restored,
            matches!(rule, Rule::A11 | Rule::A12),
            "{rule:?}"
        );
        assert_eq!(
            decision.ended_by.is_some(),
            matches!(rule, Rule::A6 | Rule::A12 | Rule::R7),
            "{rule:?}"
        );
    }
}

/// P1 and P2, with and without an attempt; `attempt = None` describes the instance now.
#[test]
fn r7a_settle_player_hold_reads_p_rows_first_and_describes_without_an_attempt() {
    let ids = table_ids();
    let attempt = attempt(&ids, Acquire, Reopened);
    for attempt in [Some(&attempt), None] {
        let PlayerHoldSettlement::Decided(p1) = settle_player_hold(attempt, A, None) else {
            panic!("P1");
        };
        assert_eq!(p1.rule, Rule::P1);
        assert_eq!(
            p1.observed,
            Some(PlayerHoldUnconfirmedObserved::StatusUnreadable)
        );
        assert!(!p1.is_final);
        assert_eq!(p1.attempt.as_ref(), attempt);
    }
    let status = scene_status(&ids, ids.e0, scene(Some(held(ids.r)), None));
    let PlayerHoldSettlement::Decided(p2) =
        settle_player_hold(Some(&attempt), "node.c", Some(&status))
    else {
        panic!("P2");
    };
    assert_eq!(p2.rule, Rule::P2);
    assert_eq!(
        p2.observed,
        Some(PlayerHoldUnconfirmedObserved::InstanceNotListed)
    );
    assert!(p2.is_final);

    let paused = status
        .clone()
        .with_scheduling_pause(Some(global(1)))
        .expect("global");
    let PlayerHoldSettlement::Current(view) = settle_player_hold(None, A, Some(&paused)) else {
        panic!("current");
    };
    assert_eq!(view.alias, A);
    assert_eq!(view.hold, Some(held(ids.r)));
    assert_eq!(view.operation, None);
    assert_eq!(Some(view.gates.clone()), paused.instance_gates(A));
    assert_eq!(view.hold_ended_with_pause, paused.hold_ended_with_pause(A));
}

/// "Own" needs a record whose request id is R: no record and another record both take the
/// not-own rows.
#[test]
fn r7a_own_means_a_record_of_this_request() {
    let ids = table_ids();
    let attempt = attempt(&ids, Acquire, TimedOut);
    let none = decide(&ids, &attempt, ids.e0, scene(None, None));
    assert_eq!(none.rule, Rule::A9);
    let another = decide(
        &ids,
        &attempt,
        ids.e0,
        scene(None, Some(record(Acquire, Failed, ids.other, ids.other))),
    );
    assert_eq!(another.rule, Rule::A10);
}

/// First match wins where two rows hold.
#[test]
fn r7a_the_first_matching_row_decides() {
    let ids = table_ids();
    let (r, h, other) = (ids.r, ids.h, ids.other);
    let acquire = attempt(&ids, Acquire, TimedOut);
    let pairs: Vec<(PlayerHoldAttempt, Scene, Rule)> = vec![
        // Own completed and the hold is R: A2, not A4.
        (
            acquire.clone(),
            scene(Some(held(r)), Some(record(Acquire, Completed, r, r))),
            Rule::A2,
        ),
        // Own running and the hold in its acquire: A1, not A5.
        (
            acquire.clone(),
            scene(
                Some(acquiring(r, PlayerHoldPhase::HandingBack)),
                Some(record(Acquire, Running, r, r)),
            ),
            Rule::A1,
        ),
        // The hold R releasing, another side's release of R: A4, not A6.
        (
            acquire.clone(),
            scene(
                Some(in_phase(r, PlayerHoldPhase::Releasing)),
                Some(record(Release, Running, other, r)),
            ),
            Rule::A4,
        ),
        // The hold H releasing, another side's release of H: R4, not R7.
        (
            attempt(&ids, Release, TimedOut),
            scene(
                Some(in_phase(h, PlayerHoldPhase::Releasing)),
                Some(record(Release, Running, other, h)),
            ),
            Rule::R4,
        ),
        // Another side's release of R failed, the hold back to held: A4, not A6.
        (
            acquire.clone(),
            scene(Some(held(r)), Some(record(Release, Failed, other, r))),
            Rule::A4,
        ),
        // Another side's release of H failed, the hold back to held: R5 (dropped) or R6, not R7.
        (
            attempt(&ids, Release, Dropped),
            scene(Some(held(h)), Some(record(Release, Failed, other, h))),
            Rule::R5,
        ),
        (
            attempt(&ids, Release, TimedOut),
            scene(Some(held(h)), Some(record(Release, Failed, other, h))),
            Rule::R6,
        ),
        (
            attempt(&ids, Release, Reopened),
            scene(Some(held(h)), Some(record(Release, Failed, other, h))),
            Rule::R6,
        ),
    ];
    for (attempt, scene, rule) in pairs {
        assert_eq!(decide(&ids, &attempt, ids.e0, scene).rule, rule);
    }
    // In a new epoch a failed release of R with the hold back to held is A11, not A12.
    let restored = decide(
        &ids,
        &attempt(&ids, Acquire, Reopened),
        ids.e1,
        scene(Some(held(r)), Some(record(Release, Failed, other, r))),
    );
    assert_eq!(restored.rule, Rule::A11);
    // The named combination of review 3: same epoch, no record, H held.
    let dropped = decide(
        &ids,
        &attempt(&ids, Release, Dropped),
        ids.e0,
        scene(Some(held(h)), None),
    );
    assert_eq!(dropped.rule, Rule::R5);
    assert_eq!(
        dropped.settlement_outcome,
        Some(PlayerHoldSettlementOutcome::NotPerformed)
    );
    let timed_out = decide(
        &ids,
        &attempt(&ids, Release, TimedOut),
        ids.e0,
        scene(Some(held(h)), None),
    );
    assert_eq!(timed_out.rule, Rule::R6);
    assert!(timed_out.is_final);
}

#[test]
fn r7a_final_follows_the_vocabulary() {
    use PlayerHoldUnconfirmedObserved as U;
    for (observed, is_final) in [
        (U::InProgress, false),
        (U::InProgressOther, false),
        (U::StatusUnreadable, false),
        (U::Superseded, true),
        (U::EpochChanged, true),
        (U::InstanceNotListed, true),
        (U::Inconsistent, true),
        (U::NotRegistered, true),
    ] {
        assert_eq!(observed.is_final(), is_final, "{observed:?}");
    }
    assert_eq!(U::ALL.len(), 8);
}

/// A final `not_registered` does not say the request will never take effect: a later status in
/// the same epoch settles the same attempt again, and so does a new epoch.
#[test]
fn r7a_a_final_settlement_settles_again_on_a_later_status() {
    let ids = table_ids();
    let (r, h) = (ids.r, ids.h);
    let acquire = attempt(&ids, Acquire, TimedOut);
    assert_eq!(
        decide(&ids, &acquire, ids.e0, scene(None, None)).rule,
        Rule::A9
    );
    assert_eq!(
        decide(
            &ids,
            &acquire,
            ids.e0,
            scene(
                Some(acquiring(r, PlayerHoldPhase::Requesting)),
                Some(record(Acquire, Running, r, r))
            )
        )
        .rule,
        Rule::A1
    );
    assert_eq!(
        decide(
            &ids,
            &acquire,
            ids.e0,
            scene(Some(held(r)), Some(record(Acquire, Completed, r, r)))
        )
        .rule,
        Rule::A2
    );
    assert_eq!(
        decide(&ids, &acquire, ids.e1, scene(Some(held(r)), None)).rule,
        Rule::A11
    );
    assert_eq!(
        decide(&ids, &acquire, ids.e1, scene(None, None)).rule,
        Rule::A13
    );

    let release = attempt(&ids, Release, Reopened);
    assert_eq!(
        decide(&ids, &release, ids.e0, scene(Some(held(h)), None)).rule,
        Rule::R6
    );
    for (state, rule) in [
        (Running, Rule::R1),
        (Completed, Rule::R2),
        (Failed, Rule::R3),
    ] {
        let hold = (state != Completed).then(|| held(h));
        assert_eq!(
            decide(
                &ids,
                &release,
                ids.e0,
                scene(hold, Some(record(Release, state, r, h)))
            )
            .rule,
            rule
        );
    }
    assert_eq!(
        decide(&ids, &release, ids.e1, scene(Some(held(h)), None)).rule,
        Rule::R10
    );
    assert_eq!(
        decide(&ids, &release, ids.e1, scene(None, None)).rule,
        Rule::R11
    );
}

/// The stored attempt: every member public, round-trips, refuses an unknown member, all three
/// triggers; settling with the one read back gives what the original gives.
#[test]
fn r7a_the_attempt_is_stored_and_settles_as_the_original() {
    let ids = table_ids();
    for trigger in [Dropped, TimedOut, Reopened] {
        let original = attempt(&ids, Release, trigger);
        let json = round_trip(&original);
        assert_eq!(
            json["trigger"],
            json!(match trigger {
                Dropped => "dropped",
                TimedOut => "timed_out",
                Reopened => "reopened",
            })
        );
        refuses_unknown::<PlayerHoldAttempt>(json.clone());
        let stored: PlayerHoldAttempt = serde_json::from_value(json).expect("decode");
        let status = scene_status(&ids, ids.e0, scene(Some(held(ids.h)), None));
        assert_eq!(
            settle_player_hold(Some(&stored), A, Some(&status)),
            settle_player_hold(Some(&original), A, Some(&status))
        );
    }
    // Every row's token, A13 among them.
    assert_eq!(Rule::ALL.len(), 26);
    assert!(Rule::ALL.contains(&Rule::A13));
    assert_eq!(serde_json::to_value(Rule::A13).expect("rule"), json!("A13"));
}

// ---- The two receipt waits ----

#[test]
fn r7a_the_receipt_waits_and_deadlines_follow_their_formulas() {
    let io = Duration::from_millis(5_000);
    assert_eq!(PLAYER_HOLD_CLOSE_ALLOWANCE_MS, 30_000);
    assert_eq!(
        player_hold_acquire_receipt_deadline(60_000),
        Some(Duration::from_millis(60_000 + 30_000 + 30_000))
    );
    assert_eq!(
        player_hold_acquire_receipt_wait(60_000, io),
        Some(Duration::from_millis(60_000 + 30_000 + 30_000 + 5_000))
    );
    assert_eq!(
        player_hold_release_receipt_deadline(),
        Duration::from_millis(33_000)
    );
    assert_eq!(
        player_hold_release_receipt_wait(io),
        Some(Duration::from_millis(38_000))
    );
    assert_eq!(player_hold_acquire_receipt_deadline(u64::MAX), None);
}
