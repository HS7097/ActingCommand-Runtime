// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #453 S2 (register §1.357; contracts/scheduling-pause.md, "Pauses held at a first
// start"; contracts/runtime-fact-store.md, `host.start_pause_applied`): an entry that declares
// `start_paused` is held by the first start that sees it, with the instance pause `instance.added`,
// before any instance is prepared, and recorded in the tagged runtime fact
// `host.start_pause_applied` after its pause, so no later start holds it again. Host starts on temp
// state roots with the fixture the start-hold tests use (`config`, `FakeProvider`; the S6c and
// PAUSE-AUTO helpers for the configured emulator start at daemon start). Two instances, FIRST
// declaring the member and SECOND not. Every wait returns as soon as what it waits for is
// reported; `GATE_WAIT` only bounds a broken run. No test reads a clock: times are compared only
// as values read back from the same root.

use super::emulator_controller::EmulatorCall;
use super::gate_369_s6c::{
    GATE_WAIT, START, autostart_host, claims_after, connectivity, head, instance_scope, pause,
    resume, stop,
};
use super::*;
use crate::host::START_PAUSE_APPLIED_REFUSED_FOR_TEST;
use actingcommand_contract::codes::SanitizationCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    FactScalar, INSTANCE_ADDED_PAUSE_REASON, InstancePauseStage, InstancePauseState,
    RuntimeFactScope, RuntimePayload,
};
use actingcommand_host_metrics::NetworkConnectivity;
use serde_json::{Value, json};

const FIRST: &str = "start_paused.first";
const SECOND: &str = "start_paused.second";
/// The fact of the entries whose first-start pause was applied.
const APPLIED: &str = "host.start_pause_applied";
/// The fact of the held pauses.
const PAUSES: &str = "host.scheduling_pause";

/// A start on `root` of FIRST (`ids.0`) and SECOND (`ids.1`), each on a default fake device, with
/// the instances of `declared` declaring `start_paused`.
fn start(
    root: &TempDir,
    ids: (InstanceId, InstanceId),
    declared: &[InstanceId],
) -> RuntimeHostResult<RuntimeHost> {
    RuntimeHost::start(
        config(root).with_start_paused(declared.iter().copied().collect()),
        Arc::new(FakeProvider::from_entries(vec![
            (FIRST.to_owned(), ids.0, Arc::new(FakeState::default())),
            (SECOND.to_owned(), ids.1, Arc::new(FakeState::default())),
        ])),
    )
}

/// `id` as the configuration and `host.start_pause_applied` spell it.
fn id_text(id: InstanceId) -> String {
    match serde_json::to_value(id).expect("an instance id encodes") {
        Value::String(text) => text,
        other => panic!("an instance id is text: {other}"),
    }
}

/// The instance pauses the host holds, by alias; the global pause holds nothing here.
fn instance_pauses(host: &RuntimeHost) -> BTreeMap<String, InstancePauseState> {
    let (global, instances) = host.scheduling_pauses_for_test().expect("the held pauses");
    assert!(global.is_none(), "no global pause: {global:?}");
    instances
}

/// The aliases the host holds paused, in order.
fn paused_aliases(host: &RuntimeHost) -> Vec<String> {
    instance_pauses(host).into_keys().collect()
}

/// The rows of `host.start_pause_applied` the host holds, if it holds the fact.
fn applied_rows(host: &RuntimeHost) -> Option<Vec<BTreeMap<String, FactScalar>>> {
    host.runtime_fact_for_test(&RuntimeFactScope::Runtime, APPLIED)
        .expect("the program facts")
        .map(|record| match record.value {
            ContractFactValue::RecordList(rows) => rows,
            other => panic!("host.start_pause_applied is a record list: {other:?}"),
        })
}

/// The instance ids the applied rows name, in row order; each row is exactly its `instance_id`
/// and its `applied_at_unix_ms`.
fn applied_ids(host: &RuntimeHost) -> Vec<String> {
    applied_rows(host)
        .unwrap_or_default()
        .iter()
        .map(|row| {
            assert_eq!(row.len(), 2, "{row:?}");
            assert!(
                matches!(
                    row.get("applied_at_unix_ms"),
                    Some(FactScalar::TimestampMs(_))
                ),
                "{row:?}"
            );
            match row.get("instance_id") {
                Some(FactScalar::String(id)) => id.clone(),
                other => panic!("a row names its instance id: {other:?}"),
            }
        })
        .collect()
}

/// Each `runtime.fact_recorded` in the root's ledger, in ledger order: its sequence, whether its
/// scope is the Runtime's, and its key.
fn fact_records(host: &RuntimeHost) -> Vec<(u64, bool, String)> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::RuntimeFactRecorded),
        ..EventQuery::default()
    })
    .expect("the fact records")
    .iter()
    .filter_map(|event| match event.payload() {
        EventPayload::Runtime(RuntimePayload::FactRecorded(recorded)) => Some((
            event.sequence(),
            recorded.record().scope == RuntimeFactScope::Runtime,
            recorded.record().key.clone(),
        )),
        _ => None,
    })
    .collect()
}

/// The sequences of the records of `key`, in ledger order.
fn recorded(host: &RuntimeHost, key: &str) -> Vec<u64> {
    fact_records(host)
        .into_iter()
        .filter(|(_, _, recorded)| recorded == key)
        .map(|(sequence, _, _)| sequence)
        .collect()
}

fn reported(lines: &[Outcome], code: &str) -> usize {
    lines
        .iter()
        .filter(|line| line.code().as_str() == code)
        .count()
}

/// The daemon start's report lines once `done` holds for them (none is taken).
fn await_reports(host: &RuntimeHost, done: impl Fn(&[Outcome]) -> bool) -> Vec<Outcome> {
    host.emulator_autostart_reports()
        .await_lines_for_test(done, GATE_WAIT)
        .expect("the awaited report lines")
}

/// The Starts the scripted emulator was asked for.
fn control_starts(state: &FakeState) -> usize {
    state
        .emulator
        .calls()
        .iter()
        .filter(|call| **call == EmulatorCall::Control(START))
        .count()
}

/// FIRST's pause as the hold sets it: `instance.added`, revision 1 (the scope's first), stage
/// `released`.
fn assert_held_added(pause: &InstancePauseState) {
    assert_eq!(pause.pause_reason.as_str(), INSTANCE_ADDED_PAUSE_REASON);
    assert_eq!(pause.revision, 1);
    assert_eq!(pause.stage, InstancePauseStage::Released);
}

// (1) A first start of an entry with the member: FIRST is paused with instance.added, revision 1,
// stage released, and SECOND is not; the applied row names FIRST's id; the pause row is recorded
// before the applied row, one record each. The daemon start's configured emulator start (both
// instances name it, both emulators stopped) starts SECOND's emulator and not FIRST's: no claim on
// FIRST, no Start.
#[test]
fn s2_1_a_first_start_holds_the_entry_records_it_after_its_pause_and_starts_no_emulator_of_it() {
    let root = TempDir::new().expect("tempdir");
    let first_state = Arc::new(FakeState::default());
    let second_state = Arc::new(FakeState::default());
    let (first, second) = (instance_id(), instance_id());
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(
        &root,
        &[
            (FIRST, first, &first_state),
            (SECOND, second, &second_state),
        ],
        read,
        |config| config.with_start_paused(BTreeSet::from([first])),
    );

    let pauses = instance_pauses(&host);
    assert_eq!(
        pauses.keys().cloned().collect::<Vec<_>>(),
        [FIRST.to_owned()],
        "{pauses:?}"
    );
    assert_held_added(&pauses[FIRST]);
    assert_eq!(applied_ids(&host), [id_text(first)]);
    let pause_records = recorded(&host, PAUSES);
    let applied_records = recorded(&host, APPLIED);
    let ([pause_record], [applied_record]) = (&pause_records[..], &applied_records[..]) else {
        panic!("one pause record and one applied record: {pause_records:?} {applied_records:?}");
    };
    assert!(
        pause_record < applied_record,
        "the pause row first, the applied row second"
    );

    stop(&host, FIRST, 4531);
    stop(&host, SECOND, 4532);
    let before = head(&host);
    assert_eq!(
        host.enqueue_emulator_autostart().expect("enqueue"),
        1,
        "SECOND alone"
    );
    let lines = await_reports(&host, |lines| {
        reported(lines, "emulator_autostart_started") == 1
    });
    let started = lines
        .iter()
        .find(|line| line.code().as_str() == "emulator_autostart_started")
        .expect("the start");
    assert_eq!(started.value("alias"), Some(&json!(SECOND)));
    assert_eq!(control_starts(&second_state), 1, "SECOND's emulator starts");
    assert_eq!(claims_after(&host, second, before).len(), 1);
    assert_eq!(control_starts(&first_state), 0, "FIRST's does not");
    assert!(
        claims_after(&host, first, before).is_empty(),
        "no claim on FIRST"
    );
    assert_eq!(
        paused_aliases(&host),
        [FIRST.to_owned()],
        "FIRST stays paused"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (2) After a person resumed FIRST, a restart holds nothing: no pause, and the applied row is the
// one the first start recorded (read back from the same root), with no second record.
#[test]
fn s2_2_after_the_resume_a_restart_holds_nothing_and_leaves_the_applied_row() {
    let root = TempDir::new().expect("tempdir");
    let ids = (instance_id(), instance_id());
    let host = start(&root, ids, &[ids.0]).expect("the first start");
    assert_eq!(paused_aliases(&host), [FIRST.to_owned()]);
    let rows = applied_rows(&host).expect("the applied row");
    let receipt = resume(&host, instance_scope(FIRST), 4533);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert!(paused_aliases(&host).is_empty(), "resumed");
    host.close().expect("close the first host");

    let restarted = start(&root, ids, &[ids.0]).expect("the restart");
    assert!(paused_aliases(&restarted).is_empty(), "not paused again");
    assert_eq!(
        applied_rows(&restarted),
        Some(rows),
        "the applied row unchanged"
    );
    assert_eq!(recorded(&restarted, APPLIED).len(), 1, "no second record");
    assert!(restarted.fatal_error().expect("runtime health").is_none());
    restarted.close().expect("close the restarted host");
}

// (3) FIRST paused for another reason (an operator's pause, restored by the start) when the member
// is first seen: that pause is kept as it was (its reason and since; restored at revision 1,
// released), the hold records no pause of its own (the start's one pause record is the restore's),
// and the applied row is still recorded.
#[test]
fn s2_3_an_instance_paused_for_another_reason_keeps_that_pause_and_is_still_recorded() {
    let root = TempDir::new().expect("tempdir");
    let ids = (instance_id(), instance_id());
    let host = start(&root, ids, &[]).expect("a start without the member");
    let receipt = pause(&host, instance_scope(FIRST), 4534);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let operators = instance_pauses(&host)[FIRST].clone();
    assert_ne!(operators.pause_reason.as_str(), INSTANCE_ADDED_PAUSE_REASON);
    let pause_records = recorded(&host, PAUSES).len();
    host.close().expect("close the first host");

    let restarted = start(&root, ids, &[ids.0]).expect("the start that first sees the member");
    let pauses = instance_pauses(&restarted);
    assert_eq!(
        pauses.keys().cloned().collect::<Vec<_>>(),
        [FIRST.to_owned()],
        "{pauses:?}"
    );
    let kept = &pauses[FIRST];
    assert_eq!(kept.pause_reason, operators.pause_reason, "its reason kept");
    assert_eq!(
        kept.since_unix_ms, operators.since_unix_ms,
        "its since kept"
    );
    assert_eq!(kept.revision, 1);
    assert_eq!(kept.stage, InstancePauseStage::Released);
    assert_eq!(
        recorded(&restarted, PAUSES).len(),
        pause_records + 1,
        "the restore's record alone"
    );
    assert_eq!(applied_ids(&restarted), [id_text(ids.0)]);
    let pause_records = recorded(&restarted, PAUSES);
    let applied_records = recorded(&restarted, APPLIED);
    assert!(
        pause_records.last() < applied_records.first(),
        "{pause_records:?} {applied_records:?}"
    );
    restarted.close().expect("close the restarted host");
}

// (4) A start that ends between the two records (its applied record refused after the pause row
// was recorded) fails; the next start pauses FIRST (or finds its pause) with instance.added and
// records the row; a third start finds the row and writes nothing of its own: the applied row is
// unchanged, and its one pause record is the restore's.
#[test]
fn s2_4_a_start_that_ends_between_the_two_records_is_completed_by_the_next_start_once() {
    let root = TempDir::new().expect("tempdir");
    let ids = (instance_id(), instance_id());
    START_PAUSE_APPLIED_REFUSED_FOR_TEST.with(|refused| refused.set(true));
    let Err(failed) = start(&root, ids, &[ids.0]) else {
        panic!("the start ends between the two records");
    };
    assert_eq!(
        failed.code(),
        SanitizationCode::RuntimeFactRecordListTooLarge.as_str(),
        "{failed:?}"
    );
    assert!(
        !START_PAUSE_APPLIED_REFUSED_FOR_TEST.with(std::cell::Cell::get),
        "the refusal was taken"
    );

    let second = start(&root, ids, &[ids.0]).expect("the next start");
    let pauses = instance_pauses(&second);
    assert_eq!(
        pauses.keys().cloned().collect::<Vec<_>>(),
        [FIRST.to_owned()],
        "{pauses:?}"
    );
    assert_held_added(&pauses[FIRST]);
    assert_eq!(applied_ids(&second), [id_text(ids.0)]);
    assert_eq!(recorded(&second, APPLIED).len(), 1);
    let rows = applied_rows(&second).expect("the applied row");
    let pause_records = recorded(&second, PAUSES).len();
    second.close().expect("close the second host");

    let third = start(&root, ids, &[ids.0]).expect("the third start");
    let pauses = instance_pauses(&third);
    assert_eq!(
        pauses.keys().cloned().collect::<Vec<_>>(),
        [FIRST.to_owned()],
        "{pauses:?}"
    );
    assert_held_added(&pauses[FIRST]);
    assert_eq!(
        applied_rows(&third),
        Some(rows),
        "the applied row unchanged"
    );
    assert_eq!(recorded(&third, APPLIED).len(), 1, "no applied record");
    assert_eq!(
        recorded(&third, PAUSES).len(),
        pause_records + 1,
        "the restore's record alone"
    );
    third.close().expect("close the third host");
}

// (5) A configuration without the member, as the restart test of `lifecycle.rs`
// (`scheduling_pauses_survive_a_restart_until_resumed`) finds a new root: nothing restored, no
// pause, no `host.start_pause_applied` held or recorded and no `host.scheduling_pause` recorded, at
// the first start and at a restart. Against a second host started alike on another new root (the
// same two instance ids) whose FIRST declares the member: their fact records are the same, record
// by record by scope and key (no value, id or time is compared), up to the hold, where the member
// adds exactly the pause record and the applied record, in that order; the host without it has
// neither anywhere.
#[test]
fn s2_5_a_configuration_without_the_member_holds_nothing_and_records_nothing_more() {
    let root = TempDir::new().expect("tempdir");
    let ids = (instance_id(), instance_id());
    let host = start(&root, ids, &[]).expect("a start without the member");
    assert!(host.scheduling_pause_restore().is_empty());
    assert!(paused_aliases(&host).is_empty());
    assert_eq!(applied_rows(&host), None);
    assert!(recorded(&host, APPLIED).is_empty());
    assert!(recorded(&host, PAUSES).is_empty());
    let without = fact_records(&host)
        .into_iter()
        .map(|(_, runtime, key)| (runtime, key))
        .collect::<Vec<_>>();
    host.close().expect("close the first host");

    let restarted = start(&root, ids, &[]).expect("a restart without the member");
    assert!(restarted.scheduling_pause_restore().is_empty());
    assert!(paused_aliases(&restarted).is_empty());
    assert_eq!(applied_rows(&restarted), None);
    assert!(recorded(&restarted, APPLIED).is_empty());
    assert!(recorded(&restarted, PAUSES).is_empty());
    restarted.close().expect("close the restarted host");

    let other_root = TempDir::new().expect("tempdir");
    let declared = start(&other_root, ids, &[ids.0]).expect("a start with the member");
    let with = fact_records(&declared)
        .into_iter()
        .map(|(_, runtime, key)| (runtime, key))
        .collect::<Vec<_>>();
    declared.close().expect("close the host with the member");
    let held_at = with
        .iter()
        .position(|(_, key)| key == PAUSES)
        .expect("the hold's pause record");
    assert_eq!(
        with[held_at..held_at + 2],
        [(true, PAUSES.to_owned()), (true, APPLIED.to_owned())],
        "{with:?}"
    );
    assert!(without.len() >= held_at, "{without:?}");
    assert_eq!(
        without[..held_at],
        with[..held_at],
        "every fact record up to the hold, without and with the member"
    );
}
