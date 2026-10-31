// SPDX-License-Identifier: AGPL-3.0-only

// Workflow 114-A (register `contracts/v012-register.md` §1.236; design
// `rd-1031/design-114-reasons-v5.md` §11.3 "测试" with its freeze addendum): the reader of
// `policy.not_run_changed` before any writer. Every record here is a JSON record of the frozen
// shape; the golden samples are its canonical bytes (compact JSON in member order), and a record
// that reads back writes them again byte for byte. Neutral ids only: `instance-a`, `task.*`,
// `decision.*`, fixed typed ids. No clock is read.

use super::*;
use crate::outcome::Category;
use crate::{
    ChannelVia, EventFamily, EventPayload, EventSeverity, EventSource, EventType, InstanceGate,
    InstancePauseStage, LedgerView, LedgerViewDefinition, PlayerHoldNext, PlayerHoldOperationKind,
    PlayerHoldOperationState, PlayerHoldPhase, PlayerHoldRestoredFrom, PlayerHoldSourceActor,
    PlayerHoldStartupPackage, PolicyPayload, RuntimeChannel, ScheduleCandidateEligibility,
    ScheduleCandidateState, ScheduleDispatchState, ScheduleHistoryClose, ScheduleHistoryDispatch,
    ScheduleHistoryHeadState, ScheduleHistoryIncompleteBecause, ScheduleHistoryInconsistentKind,
    ScheduleHistoryPrecision, ScheduleHistoryRetiredReason, ScheduleHistoryScope,
    ScheduleHistorySegmentRole, ScheduleHistoryTaskKind, ScheduleHistoryUnknownBecause,
    StartupPackageWithheld,
};
use serde_json::json;

const DATA: &str = "/payload/data";

/// A change set: three entries (a row from a code, a newly selected key, a selected key that
/// left), the control change of a hold that ended into an instance pause, the summary's first
/// value and the hold's end with its completed release.
const GOLDEN_CHANGE: &str = r#"{"family":"policy","payload":{"kind":"not_run_changed","data":{
"action":"policy.not_run","kind":"change","instance_alias":"instance-a",
"stream":{"owner_epoch":"epoch_00000000000000000000000000000002","seq":4},
"prev":{"sequence":41,"owner_epoch":"epoch_00000000000000000000000000000002","seq":3},
"base":{"sequence":39,"seq":1},
"evaluated_at_unix_ms":1761900000000,"eval_seq":7,
"catalog_hash":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
"budget":{"window_start_unix_ms":1761897600000,"used":4,"bytes":4096},
"entries":[
{"task_id":"task.alpha","from":{"code":"benefit_unknown"},"to":{"task_id":"task.alpha","why":{"code":"dispatch_paused_instance","category":"info","values":{}},"layer":"instance","state":"deferred","eligibility":"true","since_unix_ms":1761900000000,"class":"control","class_since_unix_ms":1761900000000,"next_wake_unix_ms":1761900060000}},
{"task_id":"task.beta","from":null,"to":{"gone":"selected","decision_id":"decision.beta.7","dispatch":"pending","attempts":1}},
{"task_id":"task.gamma","from":{"gone":"selected","dispatch":"rejected","dispatch_code":"dispatch_paused_instance","attempts":2},"to":{"gone":"task_absent"}}
],
"control":{
"from":{"player_hold":{"hold_id":"request_0000000000000000000000000000000a","revision":2,"phase":"releasing","since_unix_ms":1761899940000,"source":{"actor":"user","channel":"ui"},"reached_held_unix_ms":1761899950000},"effective":"player_hold"},
"to":{"instance_pause":{"revision":3,"pause_reason":"operator.check","since_unix_ms":1761899998000,"stage":"paused"},"effective":"instance_pause"}
},
"summary":{"from":null,"to":{"task_id":"task.alpha","why":{"code":"dispatch_paused_instance","category":"info","values":{}},"since_unix_ms":1761900000000,"layer":"instance","class":"control","class_since_unix_ms":1761900000000}},
"player_hold_end":{"hold_id":"request_0000000000000000000000000000000a","operation":{"request_id":"request_0000000000000000000000000000000b","kind":"release","state":"completed","source":{"actor":"lab","channel":"lab","via":"mcp"},"since_unix_ms":1761899995000,"ended_unix_ms":1761899999000,"next":"instance_paused","startup_package":"scheduled","startup_package_withheld":"instance_paused","pauses_at_end":{"instance_revision":3}}},
"audit":{}
}}}"#;

/// A baseline's first part of two, the new epoch's first record after an abnormal end: `prior`
/// with the previous gap and last cycle, and `closes` of the previous epoch's marker, its counts
/// lost across the epoch.
const GOLDEN_BASELINE: &str = r#"{"family":"policy","payload":{"kind":"not_run_changed","data":{
"action":"policy.not_run","kind":"baseline","instance_alias":"instance-a",
"stream":{"owner_epoch":"epoch_00000000000000000000000000000003","seq":1},
"prev":{"sequence":120,"owner_epoch":"epoch_00000000000000000000000000000002","seq":9},
"base":null,
"evaluated_at_unix_ms":1761904800000,"eval_seq":1,
"catalog_hash":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
"budget":{"window_start_unix_ms":1761904800000,"used":2,"bytes":8192},
"prior":{"started_at_unix_ms":1761904000000,"first_evaluated_at_unix_ms":1761904100000,"previous_owner_epoch":"epoch_00000000000000000000000000000002","previous_close":"unconfirmed","previous_gap":{"start_unix_ms":1761903000000,"end_unix_ms":1761904000000,"sequence":121},"previous_last_cycle":{"eval_seq":40,"evaluated_at_unix_ms":1761902900000},"stream_epoch":"epoch_00000000000000000000000000000002","stream_link":"instance_00000000000000000000000000000001","suppression_open":true},
"closes":{"marker":{"sequence":120,"owner_epoch":"epoch_00000000000000000000000000000002","seq":9},"from_unix_ms":1761902000000,"window_end_unix_ms":1761904800000,"to_unix_ms":1761904800000,"trigger":"records","limit_records":120,"suppressed":null,"unknown_because":"crossed_epoch"},
"baseline_reason":"budget",
"rows":[{"task_id":"task.alpha","why":{"code":"policy_benefit_nonpositive","category":"info","values":{"task_id":"task.alpha"}},"layer":"candidate","value_gate":"at","state":"blocked","eligibility":"false","since_unix_ms":1761904800000}],
"selected":[{"task_id":"task.delta","decision_id":"decision.delta.1","dispatch":"admitted","attempts":1}],
"gone":["task.epsilon"],
"control":{"global_pause":{"revision":5,"pause_reason":"operator.check","since_unix_ms":1761904700000},"resource_fence":{"cause":"device_unresponsive","since_unix_ms":1761904710000,"awaiting_person":true,"person_wait_since_unix_ms":1761904720000},"effective":"global_pause"},
"summary":{"why":{"code":"due_task_missing","category":"info","values":{}},"next_wake_unix_ms":1761905400000,"since_unix_ms":1761904800000,"layer":"gate"},
"part":1,"parts":2,
"prev_base":{"sequence":100,"seq":1},
"audit":{}
}}}"#;

/// The budget marker.
const GOLDEN_SUPPRESSED: &str = r#"{"family":"policy","payload":{"kind":"not_run_changed","data":{
"action":"policy.not_run","kind":"suppressed","instance_alias":"instance-a",
"stream":{"owner_epoch":"epoch_00000000000000000000000000000002","seq":5},
"prev":{"sequence":42,"owner_epoch":"epoch_00000000000000000000000000000002","seq":4},
"base":{"sequence":39,"seq":1},
"evaluated_at_unix_ms":1761900100000,"eval_seq":8,
"catalog_hash":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
"budget":{"window_start_unix_ms":1761897600000,"used":120,"bytes":61440},
"window":{"start_unix_ms":1761897600000,"end_unix_ms":1761901200000},
"trigger":"records","limit_records":120,"pending_key_changes":2,
"audit":{}
}}}"#;

/// A retirement of an alias the configuration no longer has, with the keys it wrote.
const GOLDEN_RETIRED: &str = r#"{"family":"policy","payload":{"kind":"not_run_changed","data":{
"action":"policy.not_run","kind":"retired","instance_alias":"instance-a",
"stream":{"owner_epoch":"epoch_00000000000000000000000000000002","seq":6},
"prev":{"sequence":43,"owner_epoch":"epoch_00000000000000000000000000000002","seq":5},
"base":{"sequence":39,"seq":1},
"evaluated_at_unix_ms":1761901300000,"eval_seq":9,
"catalog_hash":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
"budget":{"window_start_unix_ms":1761901200000,"used":1,"bytes":512},
"reason":"unconfigured","gone":["task.alpha","task.beta"],
"audit":{}
}}}"#;

/// A retirement after a failed write-side check: no `gone`, the check named.
const GOLDEN_RETIRED_FAULTED: &str = r#"{"family":"policy","payload":{"kind":"not_run_changed","data":{
"action":"policy.not_run","kind":"retired","instance_alias":"instance-a",
"stream":{"owner_epoch":"epoch_00000000000000000000000000000002","seq":6},
"prev":{"sequence":43,"owner_epoch":"epoch_00000000000000000000000000000002","seq":5},
"base":{"sequence":39,"seq":1},
"evaluated_at_unix_ms":1761901300000,"eval_seq":9,
"catalog_hash":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
"budget":{"window_start_unix_ms":1761901200000,"used":121,"bytes":61952},
"reason":"recorder_faulted","check":"golden_table",
"audit":{}
}}}"#;

/// A hold in an acquire phase: the two deadlines, no `reached_held_unix_ms`.
const GOLDEN_HOLD_ACQUIRING: &str = r#"{"hold_id":"request_00000000000000000000000000000014","revision":1,"phase":"draining","since_unix_ms":1761899000000,"source":{"actor":"cli","channel":"mcp"},"drain_deadline_unix_ms":1761899060000,"grace_deadline_unix_ms":1761899090000}"#;
/// A hold restored `held` from a stop during its acquire: no `reached_held_unix_ms`.
const GOLDEN_HOLD_RESTORED: &str = r#"{"hold_id":"request_00000000000000000000000000000015","revision":1,"phase":"held","since_unix_ms":1761899000000,"source":{"actor":"user","channel":"ui"},"restored_from_owner_epoch":"epoch_00000000000000000000000000000002","restored_from_phase":"acquiring"}"#;
const GOLDEN_GLOBAL_PAUSE: &str =
    r#"{"revision":5,"pause_reason":"operator.check","since_unix_ms":1761904700000}"#;
const GOLDEN_INSTANCE_PAUSE: &str = r#"{"revision":3,"pause_reason":"operator.check","since_unix_ms":1761899998000,"stage":"draining"}"#;
const GOLDEN_FENCE: &str =
    r#"{"cause":"device_unresponsive","since_unix_ms":1761904710000,"awaiting_person":false}"#;
/// A control block with all four members: `effective` is the player hold.
const GOLDEN_CONTROL: &str = r#"{"player_hold":{"hold_id":"request_00000000000000000000000000000015","revision":1,"phase":"held","since_unix_ms":1761899000000,"source":{"actor":"user","channel":"ui"},"restored_from_owner_epoch":"epoch_00000000000000000000000000000002","restored_from_phase":"acquiring"},"global_pause":{"revision":5,"pause_reason":"operator.check","since_unix_ms":1761904700000},"instance_pause":{"revision":3,"pause_reason":"operator.check","since_unix_ms":1761899998000,"stage":"draining"},"resource_fence":{"cause":"device_unresponsive","since_unix_ms":1761904710000,"awaiting_person":false},"effective":"player_hold"}"#;
const GOLDEN_ROW: &str = r#"{"task_id":"task.alpha","why":{"code":"instance_already_selected","category":"info","values":{}},"layer":"round","state":"deferred","eligibility":"unknown","since_unix_ms":1761900000000,"class":"turn","class_since_unix_ms":1761899000000}"#;
const GOLDEN_SUMMARY: &str = r#"{"task_id":"task.alpha","why":{"code":"trigger_false","category":"info","values":{}},"next_wake_unix_ms":1761900600000,"since_unix_ms":1761900000000,"layer":"candidate"}"#;
const GOLDEN_SELECTED: &str = r#"{"task_id":"task.delta","decision_id":"decision.delta.2","dispatch":"rejected","dispatch_code":"dispatch_resource_fenced","attempts":3}"#;
/// `PlayerHoldEnd` with its operation: an acquire that failed while draining.
const GOLDEN_END_OPERATION: &str = r#"{"hold_id":"request_00000000000000000000000000000014","operation":{"request_id":"request_00000000000000000000000000000014","kind":"acquire","state":"failed","failed_phase":"draining","code":"player_hold_drain_timeout","source":{"actor":"cli","channel":"mcp"},"since_unix_ms":1761899000000,"ended_unix_ms":1761899090000}}"#;
/// `PlayerHoldEnd` whose operation record the next accepted operation replaced.
const GOLDEN_END_SUPERSEDED: &str = r#"{"hold_id":"request_00000000000000000000000000000016","superseded":{"request_id":"request_00000000000000000000000000000017","kind":"acquire"}}"#;
/// `PlayerHoldEnd` with neither: the end's operation is not recorded.
const GOLDEN_END_HOLD_ONLY: &str = r#"{"hold_id":"request_00000000000000000000000000000018"}"#;

/// Canonical bytes: compact JSON, members in their written order.
fn compact(text: &str) -> Vec<u8> {
    let value: Value = serde_json::from_str(text).expect("the golden sample is JSON");
    // `Value`'s display is its compact JSON.
    value.to_string().into_bytes()
}

fn stored(text: &str) -> Value {
    serde_json::from_str(text).expect("the golden record is JSON")
}

/// The reader: decode, validate, and write the same bytes again.
fn read_back(record: &Value) -> EventPayload {
    let payload: EventPayload = serde_json::from_value(record.clone()).expect("the record decodes");
    payload.validate().expect("the record validates");
    assert_eq!(
        serde_json::to_vec(&payload).expect("the record encodes"),
        serde_json::to_vec(record).expect("the stored record encodes"),
        "the record reads back byte for byte"
    );
    payload
}

fn not_run(payload: &EventPayload) -> &PolicyNotRunChangedPayload {
    let EventPayload::Policy(PolicyPayload::NotRunChanged(record)) = payload else {
        panic!("a policy.not_run_changed record");
    };
    record
}

fn does_not_decode(record: &Value, what: &str) {
    assert!(
        serde_json::from_value::<EventPayload>(record.clone()).is_err(),
        "{what}: the record does not decode"
    );
}

fn fails_validation(record: &Value, what: &str) {
    let payload: EventPayload = serde_json::from_value(record.clone())
        .unwrap_or_else(|error| panic!("{what}: the record decodes: {error}"));
    let Err(refused) = payload.validate() else {
        panic!("{what}: the reader refuses it");
    };
    // A shape refusal is `invalid_policy_payload`; a code's spelling and the catalog hash keep
    // the rules every policy record shares.
    assert!(
        [
            SanitizationCode::InvalidPolicyPayload.code(),
            SanitizationCode::InvalidPolicyToken.code(),
            SanitizationCode::InvalidCatalogHash.code(),
        ]
        .contains(&refused.code()),
        "{what}: {refused}"
    );
}

fn passes(record: &Value, what: &str) {
    let payload: EventPayload = serde_json::from_value(record.clone())
        .unwrap_or_else(|error| panic!("{what}: the record decodes: {error}"));
    if let Err(error) = payload.validate() {
        panic!("{what}: the reader reads it: {error}");
    }
}

/// `record` with the member at `pointer` set to `value`; the member's object must exist.
fn with(mut record: Value, pointer: &str, value: Value) -> Value {
    let (parent, name) = pointer.rsplit_once('/').expect("a member pointer");
    record
        .pointer_mut(parent)
        .and_then(Value::as_object_mut)
        .unwrap_or_else(|| panic!("no object at {parent}"))
        .insert(name.to_owned(), value);
    record
}

/// `record` without the member at `pointer`.
fn without(mut record: Value, pointer: &str) -> Value {
    let (parent, name) = pointer.rsplit_once('/').expect("a member pointer");
    record
        .pointer_mut(parent)
        .and_then(Value::as_object_mut)
        .unwrap_or_else(|| panic!("no object at {parent}"))
        .shift_remove(name)
        .unwrap_or_else(|| panic!("no member {pointer}"));
    record
}

/// A serde word.
fn word<T: Serialize>(value: T) -> String {
    match serde_json::to_value(value).expect("a word encodes") {
        Value::String(word) => word,
        other => panic!("a word is a string, not {other}"),
    }
}

fn words<T: Copy + Serialize>(all: &[T]) -> Vec<String> {
    all.iter().map(|value| word(*value)).collect()
}

// 114-A test "载荷往返" and "金样": every kind of record (with `prior`, `closes` and
// `player_hold_end`) reads back its golden bytes, and its typed members are the written ones.
#[test]
fn n114a_every_record_kind_reads_back_its_golden_bytes() {
    let goldens = [
        (GOLDEN_CHANGE, NotRunKind::Change),
        (GOLDEN_BASELINE, NotRunKind::Baseline),
        (GOLDEN_SUPPRESSED, NotRunKind::Suppressed),
        (GOLDEN_RETIRED, NotRunKind::Retired),
        (GOLDEN_RETIRED_FAULTED, NotRunKind::Retired),
    ];
    for (golden, kind) in goldens {
        let record = stored(golden);
        assert_eq!(
            serde_json::to_vec(&record).expect("bytes"),
            compact(golden),
            "the sample is canonical"
        );
        let payload = read_back(&record);
        assert_eq!(payload.event_type(), EventType::PolicyNotRunChanged);
        assert_eq!(payload.family(), EventFamily::Policy);
        assert_eq!(payload.action(), EventAction::PolicyNotRun);
        assert_eq!(not_run(&payload).kind(), kind);
    }

    let change = read_back(&stored(GOLDEN_CHANGE));
    let change_record = not_run(&change);
    assert_eq!(change_record.head().stream.seq, 4);
    let NotRunBody::Change(body) = change_record.body() else {
        panic!("a change set");
    };
    assert_eq!(body.entries.len(), 3);
    assert!(matches!(
        &body.entries[2].to,
        NotRunTo::Gone(NotRunGone::TaskAbsent {})
    ));
    let end = body.player_hold_end.as_ref().expect("the hold's end");
    let operation = end.operation.as_ref().expect("its operation");
    assert_eq!(operation.next, Some(NotRunHoldNext::InstancePaused));
    assert_eq!(
        body.control
            .as_ref()
            .expect("a control change")
            .to
            .effective,
        Some(NotRunInstanceGate::InstancePause)
    );

    let baseline = read_back(&stored(GOLDEN_BASELINE));
    let baseline_record = not_run(&baseline);
    let prior = baseline_record.head().prior.as_ref().expect("prior");
    assert_eq!(prior.previous_close, NotRunPreviousClose::Unconfirmed);
    let closes = baseline_record.head().closes.as_ref().expect("closes");
    assert_eq!(closes.suppressed, None);
    assert_eq!(
        closes.unknown_because,
        Some(NotRunSuppressedUnknown::CrossedEpoch)
    );

    let faulted = read_back(&stored(GOLDEN_RETIRED_FAULTED));
    let NotRunBody::Retired(retired) = not_run(&faulted).body() else {
        panic!("a retirement");
    };
    assert_eq!(retired.reason, NotRunRetiredReason::RecorderFaulted);
    assert_eq!(retired.check, Some(NotRunCheck::GoldenTable));
    assert_eq!(retired.gone, None);
}

/// One ledger copy: decoded and written again byte for byte.
fn copy_reads_back<T: Serialize + DeserializeOwned>(golden: &str, what: &str) -> T {
    let copy: T = serde_json::from_str(golden).unwrap_or_else(|error| panic!("{what}: {error}"));
    assert_eq!(
        serde_json::to_vec(&copy).expect("the copy encodes"),
        compact(golden),
        "{what} reads back byte for byte"
    );
    copy
}

// 114-A test "金样": every ledger copy and the three forms of `PlayerHoldEnd` read back their
// golden bytes; each copy also reads inside a record the reader accepts.
#[test]
fn n114a_every_ledger_copy_and_hold_end_reads_back_its_golden_bytes() {
    copy_reads_back::<NotRunPlayerHold>(GOLDEN_HOLD_ACQUIRING, "a hold while acquiring");
    copy_reads_back::<NotRunPlayerHold>(GOLDEN_HOLD_RESTORED, "a restored hold");
    copy_reads_back::<NotRunGlobalPause>(GOLDEN_GLOBAL_PAUSE, "the global pause");
    copy_reads_back::<NotRunInstancePause>(GOLDEN_INSTANCE_PAUSE, "the instance pause");
    copy_reads_back::<NotRunResourceFence>(GOLDEN_FENCE, "the resource fence");
    let control = copy_reads_back::<NotRunControl>(GOLDEN_CONTROL, "the control block");
    assert_eq!(control.effective, Some(NotRunInstanceGate::PlayerHold));
    copy_reads_back::<NotRunRow>(GOLDEN_ROW, "a row");
    copy_reads_back::<NotRunSummary>(GOLDEN_SUMMARY, "the summary");
    copy_reads_back::<NotRunSelected>(GOLDEN_SELECTED, "a selected key");
    for (golden, what) in [
        (GOLDEN_END_OPERATION, "an end with its operation"),
        (GOLDEN_END_SUPERSEDED, "an end superseded"),
        (GOLDEN_END_HOLD_ONLY, "an end with the hold only"),
    ] {
        copy_reads_back::<NotRunPlayerHoldEnd>(golden, what);
    }

    // Inside records: a change set with no entry and only a hold's end, each form; a baseline
    // with the whole control block, the copies of a row and a selected key.
    let change = stored(GOLDEN_CHANGE);
    let bare = without(
        without(
            with(change, &format!("{DATA}/entries"), json!([])),
            &format!("{DATA}/control"),
        ),
        &format!("{DATA}/summary"),
    );
    for golden in [
        GOLDEN_END_OPERATION,
        GOLDEN_END_SUPERSEDED,
        GOLDEN_END_HOLD_ONLY,
    ] {
        read_back(&with(
            bare.clone(),
            &format!("{DATA}/player_hold_end"),
            stored(golden),
        ));
    }
    let baseline = stored(GOLDEN_BASELINE);
    let baseline = with(baseline, &format!("{DATA}/control"), stored(GOLDEN_CONTROL));
    let baseline = with(
        baseline,
        &format!("{DATA}/rows"),
        json!([stored(GOLDEN_ROW)]),
    );
    let baseline = with(
        baseline,
        &format!("{DATA}/selected"),
        json!([stored(GOLDEN_SELECTED)]),
    );
    let baseline = with(baseline, &format!("{DATA}/summary"), stored(GOLDEN_SUMMARY));
    read_back(&baseline);
    let acquiring = with(
        stored(GOLDEN_CONTROL),
        "/player_hold",
        stored(GOLDEN_HOLD_ACQUIRING),
    );
    read_back(&with(
        stored(GOLDEN_BASELINE),
        &format!("{DATA}/control"),
        acquiring,
    ));
}

// 114-A test: unknown members, missing members, `seq = 0`, too many parts, rows, entries and
// bytes are refused.
#[test]
fn n114a_the_shape_refuses_unknown_and_missing_members_and_the_ranges() {
    let change = stored(GOLDEN_CHANGE);
    let baseline = stored(GOLDEN_BASELINE);
    let retired = stored(GOLDEN_RETIRED);
    for (pointer, what) in [
        ("/surplus", "an unknown member of the head"),
        ("/rows", "a baseline's member on a change set"),
        ("/entries/0/to/surplus", "an unknown member of a row"),
        (
            "/control/to/instance_pause/surplus",
            "an unknown member of a copy",
        ),
        (
            "/player_hold_end/operation/surplus",
            "an unknown member of the end",
        ),
    ] {
        does_not_decode(
            &with(change.clone(), &format!("{DATA}{pointer}"), json!(1)),
            what,
        );
    }
    does_not_decode(
        &with(baseline.clone(), &format!("{DATA}/prior/surplus"), json!(1)),
        "an unknown member of prior",
    );
    does_not_decode(
        &with(retired.clone(), &format!("{DATA}/window"), json!({})),
        "a marker's member on a retirement",
    );
    for member in [
        "prev", "base", "kind", "action", "audit", "stream", "budget",
    ] {
        does_not_decode(
            &without(change.clone(), &format!("{DATA}/{member}")),
            &format!("no {member}"),
        );
    }
    does_not_decode(
        &without(baseline.clone(), &format!("{DATA}/prev_base")),
        "no prev_base",
    );
    does_not_decode(
        &without(change.clone(), &format!("{DATA}/entries/1/from")),
        "an entry without from",
    );
    does_not_decode(
        &without(baseline.clone(), &format!("{DATA}/prior/stream_link")),
        "prior without stream_link",
    );
    does_not_decode(
        &without(baseline.clone(), &format!("{DATA}/closes/suppressed")),
        "closes without suppressed",
    );

    for (pointer, what) in [
        ("/stream/seq", "seq 0"),
        ("/prev/seq", "a prev seq 0"),
        ("/base/seq", "a base seq 0"),
    ] {
        fails_validation(
            &with(change.clone(), &format!("{DATA}{pointer}"), json!(0)),
            what,
        );
    }
    fails_validation(
        &with(change.clone(), &format!("{DATA}/instance_alias"), json!("")),
        "an empty alias",
    );
    fails_validation(
        &with(
            change.clone(),
            &format!("{DATA}/catalog_hash"),
            json!("sha256:short"),
        ),
        "a malformed catalog hash",
    );
    for (part, parts, what) in [
        (0, 2, "part 0"),
        (3, 2, "part past parts"),
        (1, 17, "17 parts"),
    ] {
        fails_validation(
            &with(
                with(baseline.clone(), &format!("{DATA}/part"), json!(part)),
                &format!("{DATA}/parts"),
                json!(parts),
            ),
            what,
        );
    }
    fails_validation(
        &with(baseline.clone(), &format!("{DATA}/part"), json!(2)),
        "a second part without its base",
    );
    passes(
        &with(
            with(baseline.clone(), &format!("{DATA}/part"), json!(2)),
            &format!("{DATA}/base"),
            json!({"sequence": 122, "seq": 1}),
        ),
        "a second part with its base",
    );
    fails_validation(
        &with(
            baseline.clone(),
            &format!("{DATA}/base"),
            json!({"sequence": 122, "seq": 1}),
        ),
        "a first part with a base",
    );
    // Small rows and keys, so the count, not the byte bound, decides.
    let row = json!({"task_id": "t", "why": {"code": "c", "category": "info", "values": {}},
        "layer": "gate", "state": "blocked", "eligibility": "false", "since_unix_ms": 1});
    fails_validation(
        &with(
            baseline.clone(),
            &format!("{DATA}/rows"),
            Value::Array(vec![row.clone(); NOT_RUN_BASELINE_PART_MAX_ITEMS + 1]),
        ),
        "257 rows",
    );
    passes(
        &with(
            baseline.clone(),
            &format!("{DATA}/rows"),
            Value::Array(vec![row; NOT_RUN_BASELINE_PART_MAX_ITEMS]),
        ),
        "256 rows",
    );
    let selected =
        json!({"task_id": "t", "decision_id": "d", "dispatch": "pending", "attempts": 1});
    passes(
        &with(
            baseline.clone(),
            &format!("{DATA}/selected"),
            Value::Array(vec![selected.clone(); NOT_RUN_BASELINE_PART_MAX_ITEMS]),
        ),
        "256 selected keys",
    );
    fails_validation(
        &with(
            baseline.clone(),
            &format!("{DATA}/selected"),
            Value::Array(vec![selected; NOT_RUN_BASELINE_PART_MAX_ITEMS + 1]),
        ),
        "257 selected keys",
    );
    let absent = json!({"task_id": "task.alpha", "from": null, "to": {"gone": "task_absent"}});
    fails_validation(
        &with(
            change.clone(),
            &format!("{DATA}/entries"),
            Value::Array(vec![absent.clone(); NOT_RUN_CHANGE_MAX_ENTRIES + 1]),
        ),
        "65 entries",
    );
    passes(
        &with(
            change.clone(),
            &format!("{DATA}/entries"),
            Value::Array(vec![absent; NOT_RUN_CHANGE_MAX_ENTRIES]),
        ),
        "64 entries",
    );
    let empty = without(
        without(
            without(
                with(change, &format!("{DATA}/entries"), json!([])),
                &format!("{DATA}/control"),
            ),
            &format!("{DATA}/summary"),
        ),
        &format!("{DATA}/player_hold_end"),
    );
    fails_validation(&empty, "a change set that changes nothing");
    let long: Vec<Value> = (0..300)
        .map(|index| json!(format!("task.{index:03}.{}", "x".repeat(240))))
        .collect();
    fails_validation(
        &with(retired, &format!("{DATA}/gone"), Value::Array(long)),
        "a record over 64 KiB",
    );
}

// 114-A test "闭合词表外的词被拒": a word outside any closed vocabulary does not decode.
#[test]
fn n114a_a_word_outside_a_closed_vocabulary_does_not_decode() {
    let change = stored(GOLDEN_CHANGE);
    let baseline = stored(GOLDEN_BASELINE);
    for (pointer, what) in [
        ("/kind", "kind"),
        ("/entries/0/to/layer", "layer"),
        ("/entries/0/to/state", "state"),
        ("/entries/0/to/eligibility", "eligibility"),
        ("/entries/0/to/class", "class"),
        ("/entries/0/from/class", "a from's class"),
        ("/entries/1/to/gone", "gone"),
        ("/entries/1/to/dispatch", "dispatch"),
        ("/control/from/effective", "effective"),
        ("/control/from/player_hold/phase", "the hold's phase"),
        (
            "/control/from/player_hold/source/actor",
            "the source's actor",
        ),
        (
            "/control/from/player_hold/source/channel",
            "the source's channel",
        ),
        ("/control/to/instance_pause/stage", "the pause's stage"),
        ("/player_hold_end/operation/kind", "the operation's kind"),
        ("/player_hold_end/operation/state", "the operation's state"),
        ("/player_hold_end/operation/next", "next"),
        (
            "/player_hold_end/operation/startup_package",
            "startup_package",
        ),
        (
            "/player_hold_end/operation/startup_package_withheld",
            "startup_package_withheld",
        ),
        ("/player_hold_end/operation/source/via", "via"),
    ] {
        does_not_decode(
            &with(
                change.clone(),
                &format!("{DATA}{pointer}"),
                json!("not_a_word"),
            ),
            what,
        );
    }
    for (pointer, what) in [
        ("/baseline_reason", "baseline_reason"),
        ("/rows/0/value_gate", "value_gate"),
        ("/prior/previous_close", "previous_close"),
        ("/closes/trigger", "trigger"),
        ("/closes/unknown_because", "unknown_because"),
    ] {
        does_not_decode(
            &with(
                baseline.clone(),
                &format!("{DATA}{pointer}"),
                json!("not_a_word"),
            ),
            what,
        );
    }
    does_not_decode(
        &with(
            stored(GOLDEN_RETIRED_FAULTED),
            &format!("{DATA}/check"),
            json!("not_a_word"),
        ),
        "check",
    );
    does_not_decode(
        &with(
            stored(GOLDEN_RETIRED),
            &format!("{DATA}/reason"),
            json!("not_a_word"),
        ),
        "reason",
    );
    does_not_decode(
        &with(
            stored(GOLDEN_SUPPRESSED),
            &format!("{DATA}/trigger"),
            json!("not_a_word"),
        ),
        "a marker's trigger",
    );
    // `startup_package_withheld` takes only the two pauses of `player_hold_next`.
    does_not_decode(
        &with(
            change,
            &format!("{DATA}/player_hold_end/operation/startup_package_withheld"),
            json!("reconnected"),
        ),
        "withheld reconnected",
    );
}

// 114-A test "读侧不查码": an unregistered code of legal spelling reads and writes back byte for
// byte; a layer and class the golden table would not give read; an `effective` that is present
// but not first reads; one that names an absent member is refused; an illegal spelling is
// refused.
#[test]
fn n114a_the_reader_does_not_check_codes_golden_table_or_gate_order() {
    let change = stored(GOLDEN_CHANGE);
    let unknown = json!({"code": "n114a_code_no_catalog_lists", "category": "info", "values": {}});
    let carried = with(
        change.clone(),
        &format!("{DATA}/entries/0/to/why"),
        unknown.clone(),
    );
    let carried = with(
        carried,
        &format!("{DATA}/entries/0/from/code"),
        json!("n114a_earlier_code_no_catalog_lists"),
    );
    let carried = with(carried, &format!("{DATA}/summary/to/why"), unknown);
    let carried = with(
        carried,
        &format!("{DATA}/entries/2/from/dispatch_code"),
        json!("n114a_refusal_no_catalog_lists"),
    );
    let payload = read_back(&carried);
    let NotRunBody::Change(body) = not_run(&payload).body() else {
        panic!("a change set");
    };
    let NotRunTo::Row(row) = &body.entries[0].to else {
        panic!("a row");
    };
    assert_eq!(row.why.code().as_str(), "n114a_code_no_catalog_lists");
    assert_eq!(
        row.why.code().registered(),
        None,
        "the code is not registered"
    );

    // `benefit_unknown` is a candidate-layer code at the value gate; written as a gate-layer
    // code of the waiting-for-a-turn class, it still reads.
    let off_table = with(
        change.clone(),
        &format!("{DATA}/entries/0/to/why"),
        json!({"code": "benefit_unknown", "category": "info", "values": {}}),
    );
    let off_table = with(
        off_table,
        &format!("{DATA}/entries/0/to/layer"),
        json!("gate"),
    );
    let off_table = with(
        off_table,
        &format!("{DATA}/entries/0/to/class"),
        json!("turn"),
    );
    read_back(&off_table);

    // Both pauses in effect, `effective` the global one (not first in R-7a §2.6): it reads.
    let both = with(
        change.clone(),
        &format!("{DATA}/control/to/global_pause"),
        stored(GOLDEN_GLOBAL_PAUSE),
    );
    passes(
        &with(
            both.clone(),
            &format!("{DATA}/control/to/effective"),
            json!("global_pause"),
        ),
        "an effective gate that is not the first",
    );
    fails_validation(
        &with(
            both,
            &format!("{DATA}/control/to/effective"),
            json!("resource_fence"),
        ),
        "an effective gate whose member is absent",
    );
    fails_validation(
        &without(change.clone(), &format!("{DATA}/control/to/effective")),
        "members without effective",
    );
    fails_validation(
        &with(
            change.clone(),
            &format!("{DATA}/control/from"),
            json!({"effective": "player_hold"}),
        ),
        "effective without members",
    );

    for (pointer, spelling, what) in [
        ("/entries/0/to/why/code", "has space", "a why with a space"),
        ("/entries/0/to/why/code", "", "an empty why"),
        (
            "/entries/0/from/code",
            "tab\tcode",
            "a from code with a tab",
        ),
        (
            "/entries/2/from/dispatch_code",
            "line\ncode",
            "a dispatch code with a newline",
        ),
        ("/summary/to/why/code", "", "an empty summary why"),
        (
            "/player_hold_end/operation/source/channel",
            "lab",
            "(control) a lab channel",
        ),
    ] {
        let record = with(change.clone(), &format!("{DATA}{pointer}"), json!(spelling));
        if what.starts_with("(control)") {
            passes(&record, what);
        } else {
            fails_validation(&record, what);
        }
    }
    let fenced = with(
        stored(GOLDEN_BASELINE),
        &format!("{DATA}/control/resource_fence/cause"),
        json!("bad cause"),
    );
    fails_validation(&fenced, "a fence cause with a space");
    let failed = with(
        stored(GOLDEN_CHANGE),
        &format!("{DATA}/player_hold_end"),
        stored(GOLDEN_END_OPERATION),
    );
    fails_validation(
        &with(
            failed,
            &format!("{DATA}/player_hold_end/operation/code"),
            json!(""),
        ),
        "an empty operation code",
    );
}

// 114-A test "v5 的自洽": `prior`, the hold copy, `player_hold_end` and `retired`.
#[test]
fn n114a_the_v5_self_checks_refuse_what_they_name() {
    let baseline = stored(GOLDEN_BASELINE);
    // `suppression_open` false: the first evaluation is this record's own.
    let closed = with(
        baseline.clone(),
        &format!("{DATA}/prior/suppression_open"),
        json!(false),
    );
    fails_validation(&closed, "an open-suppression-free prior evaluated earlier");
    passes(
        &with(
            closed,
            &format!("{DATA}/prior/first_evaluated_at_unix_ms"),
            json!(1_761_904_800_000_u64),
        ),
        "an open-suppression-free prior evaluated at the record",
    );
    fails_validation(
        &with(baseline.clone(), &format!("{DATA}/prev"), Value::Null),
        "closes without prev",
    );
    fails_validation(
        &with(baseline, &format!("{DATA}/rows/0/layer"), json!("instance")),
        "value_gate on another layer",
    );

    let change = stored(GOLDEN_CHANGE);
    let hold = format!("{DATA}/control/from/player_hold");
    does_not_decode(
        &without(change.clone(), &format!("{hold}/source/channel")),
        "a hold source without channel",
    );
    fails_validation(
        &with(change.clone(), &format!("{hold}/source/via"), json!("mcp")),
        "via on the ui channel",
    );
    let held = with(change.clone(), &format!("{hold}/phase"), json!("held"));
    passes(&held, "a held hold with reached_held_unix_ms");
    fails_validation(
        &without(held.clone(), &format!("{hold}/reached_held_unix_ms")),
        "a held hold without reached_held_unix_ms",
    );
    let restored = with(
        with(
            without(held, &format!("{hold}/reached_held_unix_ms")),
            &format!("{hold}/restored_from_owner_epoch"),
            json!("epoch_00000000000000000000000000000001"),
        ),
        &format!("{hold}/restored_from_phase"),
        json!("acquiring"),
    );
    passes(&restored, "a hold restored from its acquire");

    let end = format!("{DATA}/player_hold_end");
    fails_validation(
        &with(
            change.clone(),
            &format!("{end}/superseded"),
            json!({"request_id": "request_00000000000000000000000000000017", "kind": "acquire"}),
        ),
        "operation and superseded",
    );
    let operation = format!("{end}/operation");
    let scheduling = with(
        with(
            change.clone(),
            &format!("{operation}/next"),
            json!("scheduling_paused"),
        ),
        &format!("{operation}/startup_package_withheld"),
        json!("scheduling_paused"),
    );
    fails_validation(
        &with(
            scheduling.clone(),
            &format!("{operation}/pauses_at_end"),
            json!({"instance_revision": 3, "global_revision": 5}),
        ),
        "scheduling_paused with an instance revision",
    );
    passes(
        &with(
            scheduling,
            &format!("{operation}/pauses_at_end"),
            json!({"global_revision": 5}),
        ),
        "scheduling_paused with the global revision",
    );
    fails_validation(
        &with(
            change.clone(),
            &format!("{operation}/startup_package_withheld"),
            json!("scheduling_paused"),
        ),
        "withheld other than next",
    );
    fails_validation(
        &with(
            change.clone(),
            &format!("{operation}/startup_package"),
            json!("none"),
        ),
        "withheld without a scheduled startup package",
    );
    passes(
        &without(
            with(
                change,
                &format!("{operation}/startup_package"),
                json!("none"),
            ),
            &format!("{operation}/startup_package_withheld"),
        ),
        "no startup package, nothing withheld",
    );

    let faulted = stored(GOLDEN_RETIRED_FAULTED);
    fails_validation(
        &with(
            faulted.clone(),
            &format!("{DATA}/gone"),
            json!(["task.alpha"]),
        ),
        "recorder_faulted with gone",
    );
    fails_validation(
        &without(faulted, &format!("{DATA}/check")),
        "recorder_faulted without check",
    );
    fails_validation(
        &with(
            stored(GOLDEN_RETIRED),
            &format!("{DATA}/check"),
            json!("construction"),
        ),
        "unconfigured with check",
    );
    passes(
        &without(stored(GOLDEN_RETIRED), &format!("{DATA}/gone")),
        "unconfigured with no gone listed",
    );

    let row = format!("{DATA}/entries/0/to");
    fails_validation(
        &without(stored(GOLDEN_CHANGE), &format!("{row}/class_since_unix_ms")),
        "a class without its since",
    );
    fails_validation(
        &without(stored(GOLDEN_CHANGE), &format!("{DATA}/summary/to/class")),
        "a summary since without its class",
    );
}

// The freeze addendum §2, R-7a alignment 4: the copies carry over every condition of #1466's
// `PlayerHoldState::validate` and `PlayerHoldOperation::validate`; `phase` and `failed_phase`
// are never `held` (the ledger's tightening).
#[test]
fn n114a_the_copies_carry_over_r7a_two_validates() {
    let hold = format!("{DATA}/control/from/player_hold");
    let change = stored(GOLDEN_CHANGE);
    fails_validation(
        &with(change.clone(), &format!("{hold}/revision"), json!(0)),
        "a hold revision 0",
    );
    fails_validation(
        &with(change.clone(), &format!("{hold}/since_unix_ms"), json!(0)),
        "a hold since 0",
    );
    let acquiring = with(
        without(change.clone(), &format!("{hold}/reached_held_unix_ms")),
        &format!("{hold}/phase"),
        json!("handing_back"),
    );
    fails_validation(&acquiring, "an acquiring hold without its deadlines");
    let deadlines = with(
        with(
            acquiring.clone(),
            &format!("{hold}/drain_deadline_unix_ms"),
            json!(1_761_899_960_000_u64),
        ),
        &format!("{hold}/grace_deadline_unix_ms"),
        json!(1_761_899_990_000_u64),
    );
    passes(&deadlines, "an acquiring hold with both deadlines");
    passes(
        &with(
            deadlines.clone(),
            &format!("{hold}/close_started_unix_ms"),
            json!(1_761_899_970_000_u64),
        ),
        "a hand-back with its close started",
    );
    fails_validation(
        &without(deadlines.clone(), &format!("{hold}/grace_deadline_unix_ms")),
        "an acquiring hold with one deadline",
    );
    fails_validation(
        &with(
            deadlines.clone(),
            &format!("{hold}/reached_held_unix_ms"),
            json!(1_761_899_970_000_u64),
        ),
        "an acquiring hold that reached held",
    );
    fails_validation(
        &with(
            change.clone(),
            &format!("{hold}/drain_deadline_unix_ms"),
            json!(1_761_899_960_000_u64),
        ),
        "a releasing hold with a deadline",
    );
    fails_validation(
        &with(
            change.clone(),
            &format!("{hold}/close_started_unix_ms"),
            json!(1_761_899_970_000_u64),
        ),
        "a close started outside the hand-back",
    );
    fails_validation(
        &with(
            change.clone(),
            &format!("{hold}/restored_from_phase"),
            json!("held"),
        ),
        "restored_from_phase without its epoch",
    );

    // The operation.
    let failed = with(
        stored(GOLDEN_CHANGE),
        &format!("{DATA}/player_hold_end"),
        stored(GOLDEN_END_OPERATION),
    );
    let operation = format!("{DATA}/player_hold_end/operation");
    passes(&failed, "an acquire that failed while draining");
    fails_validation(
        &with(
            failed.clone(),
            &format!("{operation}/failed_phase"),
            json!("held"),
        ),
        "a failed phase held",
    );
    fails_validation(
        &without(failed.clone(), &format!("{operation}/failed_phase")),
        "a failure without its phase",
    );
    fails_validation(
        &without(failed.clone(), &format!("{operation}/code")),
        "a failure without its code",
    );
    fails_validation(
        &without(failed.clone(), &format!("{operation}/ended_unix_ms")),
        "a failure without its end",
    );
    fails_validation(
        &with(
            failed.clone(),
            &format!("{operation}/next"),
            json!("reconnected"),
        ),
        "an acquire with a release member",
    );
    fails_validation(
        &with(
            failed.clone(),
            &format!("{operation}/request_id"),
            json!("request_00000000000000000000000000000019"),
        ),
        "an acquire that is not the ended hold",
    );
    fails_validation(
        &with(
            failed.clone(),
            &format!("{operation}/since_unix_ms"),
            json!(0),
        ),
        "an operation since 0",
    );
    let running = without(
        without(
            without(
                with(
                    failed.clone(),
                    &format!("{operation}/state"),
                    json!("running"),
                ),
                &format!("{operation}/failed_phase"),
            ),
            &format!("{operation}/code"),
        ),
        &format!("{operation}/ended_unix_ms"),
    );
    fails_validation(&running, "a running operation without its phase");
    passes(
        &with(
            running.clone(),
            &format!("{operation}/phase"),
            json!("draining"),
        ),
        "a running acquire in an acquire phase",
    );
    fails_validation(
        &with(
            running.clone(),
            &format!("{operation}/phase"),
            json!("releasing"),
        ),
        "a running acquire in the release's phase",
    );
    fails_validation(
        &with(
            running.clone(),
            &format!("{operation}/phase"),
            json!("held"),
        ),
        "a running acquire held",
    );
    let release = with(
        with(running, &format!("{operation}/kind"), json!("release")),
        &format!("{operation}/phase"),
        json!("releasing"),
    );
    passes(&release, "a running release");
    fails_validation(
        &with(release, &format!("{operation}/phase"), json!("draining")),
        "a running release in an acquire phase",
    );

    let completed = stored(GOLDEN_CHANGE);
    fails_validation(
        &without(completed.clone(), &format!("{operation}/startup_package")),
        "a completed release without its startup package",
    );
    fails_validation(
        &without(completed.clone(), &format!("{operation}/next")),
        "a completed release without next",
    );
    fails_validation(
        &with(
            completed.clone(),
            &format!("{operation}/selfcheck_failure_code"),
            json!("device_unresponsive"),
        ),
        "a pause with a self-check failure",
    );
    fails_validation(
        &with(
            completed.clone(),
            &format!("{operation}/pauses_at_end"),
            json!({"instance_revision": 0}),
        ),
        "a pause revision 0",
    );
    fails_validation(
        &with(
            completed.clone(),
            &format!("{operation}/pauses_at_end"),
            json!({}),
        ),
        "pauses_at_end with no revision",
    );
    fails_validation(
        &with(
            completed.clone(),
            &format!("{operation}/code"),
            json!("player_hold_interrupted"),
        ),
        "a completed operation with a code",
    );
    let reconnected = without(
        without(
            with(
                completed,
                &format!("{operation}/next"),
                json!("reconnected"),
            ),
            &format!("{operation}/startup_package_withheld"),
        ),
        &format!("{operation}/pauses_at_end"),
    );
    passes(&reconnected, "a reconnected release");
    passes(
        &with(
            reconnected.clone(),
            &format!("{operation}/selfcheck_failure_code"),
            json!("device_unresponsive"),
        ),
        "a reconnected release whose self-check failed",
    );
    fails_validation(
        &with(
            reconnected,
            &format!("{operation}/pauses_at_end"),
            json!({"instance_revision": 3}),
        ),
        "a reconnected release with pauses",
    );
}

/// Asserts one closed vocabulary: its name, its frozen words in order, each word's serde
/// spelling, and a word outside it refused.
macro_rules! assert_vocabulary {
    ($type:ty, $name:literal, [$($word:literal),+ $(,)?]) => {{
        let expected: &[&str] = &[$($word),+];
        assert_eq!(<$type>::VOCABULARY, $name);
        let spelled: Vec<&str> = <$type>::ALL.iter().map(|value| value.as_str()).collect();
        assert_eq!(spelled, expected, "{}", $name);
        for value in <$type>::ALL {
            assert_eq!(word(*value), value.as_str(), "{}", $name);
            assert_eq!(
                serde_json::from_value::<$type>(json!(value.as_str())).expect("the word decodes"),
                *value,
                "{}",
                $name
            );
        }
        assert!(
            serde_json::from_value::<$type>(json!("not_a_frozen_word")).is_err(),
            "{}",
            $name
        );
    }};
}

// 114-A test "词表": every ledger vocabulary and every `ScheduleHistory` vocabulary has the
// words design §11.3 freezes (with the freeze addendum: `not_run_check` on `retired`, the
// precision `turn`), and no other.
#[test]
fn n114a_every_ledger_and_schedule_history_vocabulary_has_its_frozen_words() {
    assert_vocabulary!(
        NotRunKind,
        "not_run_kind",
        ["change", "baseline", "suppressed", "retired"]
    );
    assert_vocabulary!(
        NotRunBaselineReason,
        "not_run_baseline_reason",
        [
            "start",
            "catalog",
            "binding",
            "coalesced",
            "periodic",
            "count",
            "budget"
        ]
    );
    assert_vocabulary!(
        NotRunGoneReason,
        "not_run_gone_reason",
        ["selected", "task_absent"]
    );
    assert_vocabulary!(
        NotRunDispatch,
        "not_run_dispatch",
        ["pending", "intent", "admitted", "rejected", "completed"]
    );
    assert_vocabulary!(
        NotRunLayer,
        "not_run_layer",
        ["candidate", "round", "instance", "gate"]
    );
    assert_vocabulary!(
        NotRunValueGate,
        "not_run_value_gate",
        ["before", "at", "after"]
    );
    assert_vocabulary!(NotRunClass, "not_run_class", ["turn", "control"]);
    assert_vocabulary!(
        NotRunCandidateState,
        "not_run_candidate_state",
        ["blocked", "deferred"]
    );
    assert_vocabulary!(
        NotRunCandidateEligibility,
        "not_run_candidate_eligibility",
        ["true", "false", "unknown"]
    );
    assert_vocabulary!(
        NotRunInstancePauseStage,
        "not_run_instance_pause_stage",
        ["draining", "paused", "released"]
    );
    assert_vocabulary!(
        NotRunPreviousClose,
        "not_run_previous_close",
        ["clean", "fatal", "unconfirmed", "none"]
    );
    assert_vocabulary!(
        NotRunSuppressedUnknown,
        "not_run_suppressed_unknown",
        ["crossed_epoch"]
    );
    assert_vocabulary!(
        NotRunRetiredReason,
        "not_run_retired_reason",
        ["unconfigured", "unregistered", "recorder_faulted"]
    );
    assert_vocabulary!(NotRunTrigger, "not_run_trigger", ["records", "bytes"]);
    assert_vocabulary!(
        NotRunCheck,
        "not_run_check",
        [
            "code_registered",
            "golden_table",
            "control_order",
            "construction"
        ]
    );

    assert_vocabulary!(
        ScheduleHistoryScope,
        "schedule_history_scope",
        ["coverage", "instance_control", "instance_summary", "task"]
    );
    assert_vocabulary!(
        ScheduleHistoryTaskKind,
        "schedule_history_task_kind",
        [
            "row",
            "selected",
            "task_absent",
            "catalog_changed",
            "retired"
        ]
    );
    assert_vocabulary!(
        ScheduleHistoryPrecision,
        "schedule_history_precision",
        [
            "exact",
            "turn",
            "merged",
            "tail_unconfirmed",
            "not_running",
            "not_evaluated",
            "unknown",
            "inconsistent"
        ]
    );
    assert_vocabulary!(
        ScheduleHistoryUnknownBecause,
        "schedule_history_unknown_because",
        [
            "not_started",
            "epoch_unrecorded",
            "unregistered",
            "bad_baseline",
            "read_limit",
            "recorder_faulted"
        ]
    );
    assert_vocabulary!(
        ScheduleHistoryInconsistentKind,
        "schedule_history_inconsistent_kind",
        [
            "seq_gap",
            "seq_duplicate",
            "prev_mismatch",
            "from_mismatch",
            "control_mismatch",
            "ledger_gap"
        ]
    );
    assert_vocabulary!(
        ScheduleHistoryIncompleteBecause,
        "schedule_history_incomplete_because",
        ["page", "segment_cap", "frame", "read_budget"]
    );
    assert_vocabulary!(
        ScheduleHistoryDispatch,
        "schedule_history_dispatch",
        [
            "pending",
            "intent",
            "admitted",
            "rejected",
            "completed",
            "unknown",
            "beyond_window"
        ]
    );
    assert_vocabulary!(
        ScheduleHistoryHeadState,
        "schedule_history_head_state",
        [
            "exact",
            "merging",
            "not_evaluated",
            "unregistered",
            "retired",
            "recorder_faulted"
        ]
    );
    assert_vocabulary!(
        ScheduleHistoryClose,
        "schedule_history_close",
        ["clean", "fatal", "unconfirmed"]
    );
    assert_vocabulary!(
        ScheduleHistoryRetiredReason,
        "schedule_history_retired_reason",
        ["unconfigured", "unregistered"]
    );
    assert_vocabulary!(
        ScheduleHistorySegmentRole,
        "schedule_history_segment_role",
        [
            "record",
            "merge_wait",
            "not_evaluated",
            "not_running",
            "tail",
            "epoch_unrecorded",
            "not_started",
            "unregistered",
            "bad_baseline",
            "read_limit",
            "ledger_gap"
        ]
    );

    // The two `gone` tags spell `not_run_gone_reason`.
    let from_selected = NotRunFromGone::Selected {
        dispatch: NotRunDispatch::Pending,
        dispatch_code: None,
        attempts: 1,
    };
    let gone_selected = NotRunGone::Selected {
        decision_id: "decision.alpha.1".to_owned(),
        dispatch: NotRunDispatch::Pending,
        dispatch_code: None,
        attempts: 1,
    };
    for (encoded, reason) in [
        (serde_json::to_value(&from_selected), from_selected.gone()),
        (
            serde_json::to_value(NotRunFromGone::TaskAbsent {}),
            NotRunFromGone::TaskAbsent {}.gone(),
        ),
        (serde_json::to_value(&gone_selected), gone_selected.gone()),
        (
            serde_json::to_value(NotRunGone::TaskAbsent {}),
            NotRunGone::TaskAbsent {}.gone(),
        ),
    ] {
        let encoded = encoded.expect("a gone key encodes");
        assert_eq!(encoded["gone"], json!(reason.as_str()));
    }
}

// The freeze addendum §2, R-7a alignment 5: the reused vocabularies are the ledger's own plain
// enums, word for word R-7a's `ALL`; the copies of `channel`, `ChannelVia`,
// `StartupPackageWithheld`, `InstancePauseStage` and `ScheduleExplanation`'s three enums map
// every source word (an exhaustive match: a new source word does not compile until the copy
// decides) to the same spelling, and hold no other word.
#[test]
fn n114a_the_reused_vocabularies_equal_their_sources_word_for_word() {
    assert_eq!(
        words(NotRunInstanceGate::ALL),
        words(InstanceGate::ALL),
        "instance_gate"
    );
    assert_eq!(
        words(NotRunHoldPhase::ALL),
        words(PlayerHoldPhase::ALL),
        "player_hold_phase"
    );
    assert_eq!(
        words(NotRunHoldSourceActor::ALL),
        words(PlayerHoldSourceActor::ALL),
        "player_hold_source_actor"
    );
    assert_eq!(
        words(NotRunHoldOperationKind::ALL),
        words(PlayerHoldOperationKind::ALL),
        "player_hold_operation_kind"
    );
    assert_eq!(
        words(NotRunHoldOperationState::ALL),
        words(PlayerHoldOperationState::ALL),
        "player_hold_operation_state"
    );
    assert_eq!(
        words(NotRunHoldNext::ALL),
        words(PlayerHoldNext::ALL),
        "player_hold_next"
    );
    assert_eq!(
        words(NotRunHoldStartupPackage::ALL),
        words(PlayerHoldStartupPackage::ALL),
        "player_hold_startup_package"
    );
    assert_eq!(
        words(NotRunHoldRestoredFrom::ALL),
        words(PlayerHoldRestoredFrom::ALL),
        "player_hold_restored_from"
    );
    // The instance gates keep R-7a §2.6's order, which `effective` names.
    for (copy, source) in NotRunInstanceGate::ALL.iter().zip(InstanceGate::ALL) {
        assert_eq!(copy.as_str(), source.as_str());
    }

    fn channel(source: RuntimeChannel) -> NotRunChannel {
        match source {
            RuntimeChannel::Cli => NotRunChannel::Cli,
            RuntimeChannel::Mcp => NotRunChannel::Mcp,
            RuntimeChannel::Ui => NotRunChannel::Ui,
            RuntimeChannel::Lab => NotRunChannel::Lab,
        }
    }
    fn via(source: ChannelVia) -> NotRunChannelVia {
        match source {
            ChannelVia::Mcp => NotRunChannelVia::Mcp,
            ChannelVia::Ui => NotRunChannelVia::Ui,
        }
    }
    fn withheld(source: StartupPackageWithheld) -> NotRunStartupPackageWithheld {
        match source {
            StartupPackageWithheld::InstancePaused => NotRunStartupPackageWithheld::InstancePaused,
            StartupPackageWithheld::SchedulingPaused => {
                NotRunStartupPackageWithheld::SchedulingPaused
            }
        }
    }
    fn stage(source: InstancePauseStage) -> NotRunInstancePauseStage {
        match source {
            InstancePauseStage::Draining => NotRunInstancePauseStage::Draining,
            InstancePauseStage::Paused => NotRunInstancePauseStage::Paused,
            InstancePauseStage::Released => NotRunInstancePauseStage::Released,
        }
    }
    fn dispatch(source: ScheduleDispatchState) -> NotRunDispatch {
        match source {
            ScheduleDispatchState::Pending => NotRunDispatch::Pending,
            ScheduleDispatchState::Intent => NotRunDispatch::Intent,
            ScheduleDispatchState::Admitted => NotRunDispatch::Admitted,
            ScheduleDispatchState::Rejected => NotRunDispatch::Rejected,
            ScheduleDispatchState::Completed => NotRunDispatch::Completed,
        }
    }
    fn state(source: ScheduleCandidateState) -> NotRunCandidateState {
        match source {
            ScheduleCandidateState::Deferred => NotRunCandidateState::Deferred,
            ScheduleCandidateState::Blocked => NotRunCandidateState::Blocked,
        }
    }
    fn eligibility(source: ScheduleCandidateEligibility) -> NotRunCandidateEligibility {
        match source {
            ScheduleCandidateEligibility::True => NotRunCandidateEligibility::True,
            ScheduleCandidateEligibility::False => NotRunCandidateEligibility::False,
            ScheduleCandidateEligibility::Unknown => NotRunCandidateEligibility::Unknown,
        }
    }
    let channels = [
        RuntimeChannel::Cli,
        RuntimeChannel::Mcp,
        RuntimeChannel::Ui,
        RuntimeChannel::Lab,
    ];
    for source in channels {
        assert_eq!(word(channel(source)), word(source), "channel");
    }
    assert_eq!(NotRunChannel::ALL.len(), channels.len());
    let vias = [ChannelVia::Mcp, ChannelVia::Ui];
    for source in vias {
        assert_eq!(word(via(source)), word(source), "via");
    }
    assert_eq!(NotRunChannelVia::ALL.len(), vias.len());
    let withheld_words = [
        StartupPackageWithheld::InstancePaused,
        StartupPackageWithheld::SchedulingPaused,
    ];
    for source in withheld_words {
        assert_eq!(
            word(withheld(source)),
            word(source),
            "startup_package_withheld"
        );
    }
    assert_eq!(
        NotRunStartupPackageWithheld::ALL.len(),
        withheld_words.len()
    );
    let stages = [
        InstancePauseStage::Draining,
        InstancePauseStage::Paused,
        InstancePauseStage::Released,
    ];
    for source in stages {
        assert_eq!(word(stage(source)), word(source), "instance pause stage");
    }
    assert_eq!(NotRunInstancePauseStage::ALL.len(), stages.len());
    let dispatches = [
        ScheduleDispatchState::Pending,
        ScheduleDispatchState::Intent,
        ScheduleDispatchState::Admitted,
        ScheduleDispatchState::Rejected,
        ScheduleDispatchState::Completed,
    ];
    for source in dispatches {
        assert_eq!(word(dispatch(source)), word(source), "dispatch state");
    }
    assert_eq!(NotRunDispatch::ALL.len(), dispatches.len());
    let states = [
        ScheduleCandidateState::Deferred,
        ScheduleCandidateState::Blocked,
    ];
    for source in states {
        assert_eq!(word(state(source)), word(source), "candidate state");
    }
    assert_eq!(NotRunCandidateState::ALL.len(), states.len());
    let eligibilities = [
        ScheduleCandidateEligibility::True,
        ScheduleCandidateEligibility::False,
        ScheduleCandidateEligibility::Unknown,
    ];
    for source in eligibilities {
        assert_eq!(
            word(eligibility(source)),
            word(source),
            "candidate eligibility"
        );
    }
    assert_eq!(NotRunCandidateEligibility::ALL.len(), eligibilities.len());
    // `schedule_history_dispatch` is the five dispatch words and two more.
    assert_eq!(
        words(&ScheduleHistoryDispatch::ALL[..5]),
        words(NotRunDispatch::ALL),
        "schedule_history_dispatch"
    );
    // `schedule_history_close` is `not_run_previous_close` without `none`.
    assert_eq!(
        words(ScheduleHistoryClose::ALL),
        words(&NotRunPreviousClose::ALL[..3]),
        "schedule_history_close"
    );
    // `schedule_history_retired_reason` is `not_run_retired_reason` without `recorder_faulted`.
    assert_eq!(
        words(ScheduleHistoryRetiredReason::ALL),
        words(&NotRunRetiredReason::ALL[..2]),
        "schedule_history_retired_reason"
    );
}

// 114-A test "视图 DDL 不变" (its contract side): the record type is of the policy family, has
// the category info, carries no outcome and is in no view's type list, and the policy family in
// no view's family list. The view DDL is built only from these lists (the ledger's
// `views::predicate`), so it is the DDL of f64b3a12 byte for byte. The ledger checks the
// durability (`gate_114a`).
#[test]
fn n114a_the_record_type_is_policy_info_and_in_no_view_type_list() {
    let event_type = EventType::PolicyNotRunChanged;
    assert_eq!(event_type.family(), EventFamily::Policy);
    assert_eq!(event_type.category(), Category::Info);
    assert_eq!(word(event_type), "policy.not_run_changed");
    assert_eq!(word(EventAction::PolicyNotRun), "policy.not_run");
    let payload = read_back(&stored(GOLDEN_CHANGE));
    assert!(payload.outcome().is_none(), "the record carries no outcome");
    assert_eq!(
        crate::record_severity(EventSeverity::Info, &payload),
        EventSeverity::Info
    );
    for view in LedgerView::ALL {
        if let LedgerViewDefinition::Types { families, events } = view.definition() {
            assert!(!events.contains(&event_type), "{view:?} lists the type");
            assert!(
                !families.contains(&EventFamily::Policy),
                "{view:?} lists the policy family"
            );
        }
    }
    assert_eq!(
        LedgerView::memberships(event_type, EventSeverity::Info, EventSource::Runtime, false),
        vec![LedgerView::Events]
    );
}
