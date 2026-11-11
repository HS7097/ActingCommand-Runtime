// SPDX-License-Identifier: AGPL-3.0-only

//! ISO-START-1 (#419; register §1.301): one pair's refused settlement fact no longer stops the
//! Runtime's start. The start derives every registered pair's four settlement facts again
//! (`record_policy_settlement_facts_on_start`): a pair the store refuses as stale is recorded
//! once and skipped, a pair refused for a reason of its own is recorded and its instance held,
//! and a store whose ledger cannot be written still fails the start. The test's pairs are
//! settlements the step writes after the recovered ones (`START_SETTLEMENT_FACTS_FOR_TEST`);
//! the refusals are the store's own.

use super::gate_369_s6c::instance_status;
use super::*;
use crate::host::{START_SETTLEMENT_FACTS_FOR_TEST, StartSettlementFactsForTest};
use crate::policy_host::PolicySettlement;
use actingcommand_contract::outcome::{CauseRelation, Outcome};
use actingcommand_contract::{RuntimeFactRecord, RuntimeFactScope, RuntimePayload};

/// The first start's Runtime clock, and the restart's an hour later.
const FIRST_START_UNIX_MS: u64 = POLICY_NOW_UNIX_MS;
const RESTART_UNIX_MS: u64 = POLICY_NOW_UNIX_MS + 3_600_000;
/// When the test's settlements settled, before the first start: their records' observation.
const SETTLED_UNIX_MS: u64 = POLICY_NOW_UNIX_MS - 600_000;
/// A later run's observation of a key, after the settlement and before the restart.
const LATER_RUN_UNIX_MS: u64 = SETTLED_UNIX_MS + 60_000;
/// The catalog tasks of the test's pairs.
const TASK_ONE: &str = "fixture.iso-start-one";
const TASK_TWO: &str = "fixture.iso-start-two";
const TASK_STALE: &str = "fixture.iso-start-stale";

/// The provider of instances A and B, each with a fresh fake state.
fn provider(instance_a: InstanceId, instance_b: InstanceId) -> Arc<FakeProvider> {
    Arc::new(FakeProvider::from_entries([
        (
            POLICY_INSTANCE_ALIAS.to_owned(),
            instance_a,
            Arc::new(FakeState::default()),
        ),
        (
            POLICY_INSTANCE_ALIAS_B.to_owned(),
            instance_b,
            Arc::new(FakeState::default()),
        ),
    ]))
}

/// A start of A and B over `root` with its Runtime clock at `clock_unix_ms`.
fn start_at(
    root: &TempDir,
    clock_unix_ms: u64,
    instance_a: InstanceId,
    instance_b: InstanceId,
) -> RuntimeHostResult<RuntimeHost> {
    let clock = Arc::new(ManualRuntimeClock::new(clock_unix_ms, 0));
    RuntimeHost::start(
        config(root).with_runtime_clock(clock),
        provider(instance_a, instance_b),
    )
}

/// The restart, at `RESTART_UNIX_MS`, whose settlement-fact step also writes `settlements`;
/// with `store_fails` that step's first write fails as an append the ledger cannot make.
fn restart_with(
    root: &TempDir,
    instance_a: InstanceId,
    instance_b: InstanceId,
    settlements: Vec<PolicySettlement>,
    store_fails: bool,
) -> RuntimeHostResult<RuntimeHost> {
    START_SETTLEMENT_FACTS_FOR_TEST.with(|hook| {
        *hook.borrow_mut() = Some(StartSettlementFactsForTest {
            settlements,
            store_fails,
        });
    });
    let started = start_at(root, RESTART_UNIX_MS, instance_a, instance_b);
    let left = START_SETTLEMENT_FACTS_FOR_TEST.with(|hook| hook.borrow_mut().take());
    assert!(
        left.is_none(),
        "the start's settlement-fact step took the test's settlements"
    );
    started
}

/// A succeeded settlement of `task` on `instance_alias` that ran `duration_ms`, settled at
/// `SETTLED_UNIX_MS`.
fn settlement(task: &str, instance_alias: &str, duration_ms: u64) -> PolicySettlement {
    PolicySettlement {
        catalog_task_id: task.to_owned(),
        instance_alias: instance_alias.to_owned(),
        duration_ms,
        succeeded: true,
        failure_streak: 0,
        completed_at_unix_ms: SETTLED_UNIX_MS,
    }
}

/// `value` as the integer fact the store holds.
fn integer(value: u64) -> ContractFactValue {
    ContractFactValue::Integer(i64::try_from(value).expect("a value of the test's range"))
}

/// The four records main writes for `settled` in the scope of `instance_id` (Workflow #308
/// slice 5b): its duration, `succeeded`, a failure streak of 0 and its settlement time, each
/// observed at the settlement time, with source `policy` and no lifetime.
fn written_as_on_main(
    settled: &PolicySettlement,
    instance_id: InstanceId,
) -> Vec<RuntimeFactRecord> {
    [
        ("last_duration_ms", integer(settled.duration_ms)),
        (
            "last_outcome",
            ContractFactValue::String("succeeded".to_owned()),
        ),
        ("failure_streak", integer(settled.failure_streak)),
        (
            "completed_at_unix_ms",
            integer(settled.completed_at_unix_ms),
        ),
    ]
    .into_iter()
    .map(|(suffix, value)| RuntimeFactRecord {
        scope: RuntimeFactScope::Instance { instance_id },
        key: format!("task.{}.{suffix}", settled.catalog_task_id),
        value,
        observed_at_unix_ms: settled.completed_at_unix_ms,
        source: OriginModule::Policy,
        ttl_ms: None,
    })
    .collect()
}

/// The record the Runtime's fact store holds for `key` on `instance_id`.
fn stored(host: &RuntimeHost, instance_id: InstanceId, key: &str) -> Option<RuntimeFactRecord> {
    host.runtime_fact_for_test(&RuntimeFactScope::Instance { instance_id }, key)
        .expect("the Runtime's fact store")
}

/// The outcomes of the `policy_settlement_facts_skipped` records under `instance_id`'s links, in
/// ledger order; each is a Warning.
fn skipped_records(host: &RuntimeHost, instance_id: InstanceId) -> Vec<Outcome> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::RuntimeFailed),
        instance_id: Some(instance_id),
        ..EventQuery::default()
    })
    .expect("the instance's runtime failures")
    .into_iter()
    .filter_map(|event| match event.payload() {
        EventPayload::Runtime(RuntimePayload::Failed(record))
            if record.outcome().code().as_str() == "policy_settlement_facts_skipped" =>
        {
            assert_eq!(event.severity(), EventSeverity::Warning, "a Warning");
            Some(record.outcome().clone())
        }
        _ => None,
    })
    .collect()
}

/// An integer value of `outcome`, if any.
fn unix_ms(outcome: &Outcome, key: &str) -> Option<u64> {
    outcome.value(key).and_then(serde_json::Value::as_u64)
}

/// Test 1 (a stale pair): A's stale pair already holds a later run's record of its first key,
/// newer than the settlement the restart derives (the edge of register §1.296). The restart
/// derives three pairs, the stale one between the two others. It completes with no host fault,
/// writes the two others' facts as main writes them, keeps the later run's record and writes
/// none of the stale pair's, records the skip once (`runtime_fact_stale`, the key, both
/// observations) and holds no instance.
#[test]
fn iso_start1_a_stale_pair_is_skipped_once_and_the_two_others_are_written() {
    let root = TempDir::new().expect("tempdir");
    let (instance_a, instance_b) = (instance_id(), instance_id());
    let later_run = RuntimeFactRecord {
        scope: RuntimeFactScope::Instance {
            instance_id: instance_a,
        },
        key: format!("task.{TASK_STALE}.last_duration_ms"),
        value: integer(9_000),
        observed_at_unix_ms: LATER_RUN_UNIX_MS,
        source: OriginModule::Policy,
        ttl_ms: None,
    };
    let host =
        start_at(&root, FIRST_START_UNIX_MS, instance_a, instance_b).expect("the first start");
    host.record_runtime_fact_for_test(later_run.clone())
        .expect("the later run's record");
    host.close().expect("close the first start");

    let one = settlement(TASK_ONE, POLICY_INSTANCE_ALIAS, 42_000);
    let stale = settlement(TASK_STALE, POLICY_INSTANCE_ALIAS, 7_000);
    let two = settlement(TASK_TWO, POLICY_INSTANCE_ALIAS_B, 43_000);
    let host = restart_with(
        &root,
        instance_a,
        instance_b,
        vec![one.clone(), stale, two.clone()],
        false,
    )
    .expect("the restart completes");
    assert!(host.fatal_error().expect("runtime health").is_none());

    for (settled, instance_id) in [(&one, instance_a), (&two, instance_b)] {
        for expected in written_as_on_main(settled, instance_id) {
            assert_eq!(
                stored(&host, instance_id, &expected.key).as_ref(),
                Some(&expected),
                "{} is written as on main",
                expected.key
            );
        }
    }
    assert_eq!(
        stored(&host, instance_a, &later_run.key),
        Some(later_run.clone()),
        "the later run's record stands"
    );
    for suffix in ["last_outcome", "failure_streak", "completed_at_unix_ms"] {
        let key = format!("task.{TASK_STALE}.{suffix}");
        assert_eq!(
            stored(&host, instance_a, &key),
            None,
            "{key} is not written"
        );
    }

    let skipped = skipped_records(&host, instance_a);
    assert_eq!(skipped.len(), 1, "the skip is recorded once: {skipped:#?}");
    let record = &skipped[0];
    for (key, value) in [
        ("failure_code", "runtime_fact_stale"),
        ("task_id", TASK_STALE),
        ("instance_id", POLICY_INSTANCE_ALIAS),
        ("fact_key", later_run.key.as_str()),
        ("operation", "record_policy_settlement_facts"),
    ] {
        assert_eq!(outcome_str(record, key), Some(value), "{key}: {record:#?}");
    }
    assert_eq!(
        unix_ms(record, "observed_at_unix_ms"),
        Some(SETTLED_UNIX_MS),
        "the refused record's observation: {record:#?}"
    );
    assert!(
        outcome_links(record, CauseRelation::CausedBy)
            .iter()
            .any(|refusal| refusal.code().as_str() == "runtime_fact_stale"
                && unix_ms(refusal, "observed_at_unix_ms") == Some(LATER_RUN_UNIX_MS)),
        "the refusal names the stored record's observation: {record:#?}"
    );
    assert!(skipped_records(&host, instance_b).is_empty());
    let (_, paused) = host
        .scheduling_pauses_for_test()
        .expect("scheduling pauses");
    assert!(paused.is_empty(), "nothing is held: {paused:?}");
    host.close().expect("close host");
}

/// Test 2 (a pair refused for a reason of its own): A's pair names a catalog task of 128 bytes,
/// so its fact keys exceed the store's key bound and the store refuses its first record
/// (`invalid_runtime_fact_key`) before anything of it is appended. The restart completes with no
/// host fault. A is held, with `policy_settlement_facts_skipped` as the pause reason the status
/// read shows; its pair is recorded once and none of its records is written. B's pair is
/// written as main writes it, and B is not held.
#[test]
fn iso_start1_a_pair_refused_for_its_own_reason_holds_only_its_instance() {
    let root = TempDir::new().expect("tempdir");
    let (instance_a, instance_b) = (instance_id(), instance_id());
    let host =
        start_at(&root, FIRST_START_UNIX_MS, instance_a, instance_b).expect("the first start");
    host.close().expect("close the first start");

    let long_task = format!("fixture.{}", "long".repeat(30));
    let refused = settlement(&long_task, POLICY_INSTANCE_ALIAS, 42_000);
    let written = settlement(TASK_TWO, POLICY_INSTANCE_ALIAS_B, 43_000);
    let host = restart_with(
        &root,
        instance_a,
        instance_b,
        vec![refused, written.clone()],
        false,
    )
    .expect("the restart completes");
    assert!(host.fatal_error().expect("runtime health").is_none());

    let held = instance_status(&host, POLICY_INSTANCE_ALIAS)
        .pause()
        .cloned()
        .expect("A is held");
    assert_eq!(
        held.pause_reason.as_str(),
        "policy_settlement_facts_skipped"
    );
    assert!(
        instance_status(&host, POLICY_INSTANCE_ALIAS_B)
            .pause()
            .is_none(),
        "B is not held"
    );

    for expected in written_as_on_main(&written, instance_b) {
        assert_eq!(
            stored(&host, instance_b, &expected.key).as_ref(),
            Some(&expected),
            "{} is written as on main",
            expected.key
        );
    }
    for suffix in [
        "last_duration_ms",
        "last_outcome",
        "failure_streak",
        "completed_at_unix_ms",
    ] {
        let key = format!("task.{long_task}.{suffix}");
        assert_eq!(
            stored(&host, instance_a, &key),
            None,
            "{key} is not written"
        );
    }

    let skipped = skipped_records(&host, instance_a);
    assert_eq!(
        skipped.len(),
        1,
        "the refusal is recorded once: {skipped:#?}"
    );
    let record = &skipped[0];
    let refused_key = format!("task.{long_task}.last_duration_ms");
    for (key, value) in [
        ("failure_code", "invalid_runtime_fact_key"),
        ("task_id", long_task.as_str()),
        ("instance_id", POLICY_INSTANCE_ALIAS),
        ("fact_key", refused_key.as_str()),
    ] {
        assert_eq!(outcome_str(record, key), Some(value), "{key}: {record:#?}");
    }
    assert!(skipped_records(&host, instance_b).is_empty());
    host.close().expect("close host");
}

/// Test 3 (the store itself fails): the restart's settlement-fact step meets a store whose
/// ledger append fails (`ledger_failure`), a fault of no one pair: the start fails with it,
/// fatal, as on main.
#[test]
fn iso_start1_a_store_that_cannot_be_written_still_fails_the_start() {
    let root = TempDir::new().expect("tempdir");
    let (instance_a, instance_b) = (instance_id(), instance_id());
    let host =
        start_at(&root, FIRST_START_UNIX_MS, instance_a, instance_b).expect("the first start");
    host.close().expect("close the first start");

    let Err(failed) = restart_with(
        &root,
        instance_a,
        instance_b,
        vec![settlement(TASK_ONE, POLICY_INSTANCE_ALIAS, 42_000)],
        true,
    ) else {
        panic!("the start did not fail");
    };
    assert_eq!(failed.code(), "ledger_failure", "{failed:?}");
    assert!(failed.is_fatal(), "{failed:?}");
}
