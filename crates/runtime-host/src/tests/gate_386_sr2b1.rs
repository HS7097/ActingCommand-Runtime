// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #386 SR-2b-1 (design `rd-1028/design-p22.md` §6.2 and §8 row SR-2b-1, tests ① to ⑥;
// `contracts/policy-suspension.md`, "Condition termination and owed reruns"): a run's first
// settlement decides its `condition` from its failure code, the `scheduling.rerun` values in
// effect and its pair's condition terminations in this occurrence (its activity window). Only a
// recoverable failure of a task that is not sensitive, with a code the configured table lists,
// ends by a condition: the record carries `condition{refunded, count, expires_at_unix_ms}`, the
// counts are given back when `refund_counts` says so, the streak does not grow, and the owed
// rerun is kept in the pair's L7 value record. A pair's condition termination above
// `max_per_occurrence` settles as an ordinary recoverable failure and records
// `policy_rerun_exhausted`. A run a restart settles from its terminal is decided by the same
// rule; one with no terminal stays `policy_settlement_interrupted`. Every expected value is the
// design's; the Runtime clock is a manual one except where the fixture's injected capture failure
// needs the Runtime's own (test ⑥, as the crash fixture it follows). Alice's decision 92
// (2026-10-29, Runtime 0.12.28) puts the operator pause's `contained_task_paused` in the shipped
// table: test `sr92` pins it, and test ② was adapted.

use super::gate_386_sr2a::{once_a_day_sources, sampling_ended};
use super::*;
use crate::policy_control::PolicyValueRecord;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, FactScalar, PolicyAdmissionRecord,
    PolicyConditionSettlement, PolicyControlStateRecord, PolicyFailureRecord,
    RuntimeConfigManifest,
};
use actingcommand_runtime_config::scheduling::{
    RERUN_CONDITION_CODES, RERUN_MAX_PER_OCCURRENCE, RERUN_REFUND_COUNTS,
};
use serde_json::json;

pub(super) const TASK: &str = "fixture.observe";
const SAMPLING_CODE: &str = "recognition_sample_deadline_exceeded";
const EXHAUSTED_CODE: &str = "policy_rerun_exhausted";
const DAY_MS: u64 = 86_400_000;
const HOUR_MS: u64 = 3_600_000;
const BETWEEN_RUNS_MS: u64 = 600_000;

// ---- The fixture.

/// The end of the budget fixtures' activity window, the whole UTC day, that holds `at`.
pub(super) fn whole_day_window_end(at: u64) -> u64 {
    (at / DAY_MS + 1) * DAY_MS
}

/// The end of the catalog fixture's activity window, 08:00 to 22:00 UTC, that holds `at`.
fn daytime_window_end(at: u64) -> u64 {
    at / DAY_MS * DAY_MS + 22 * HOUR_MS
}

/// The once-a-day task, sensitive.
fn sensitive_once_a_day_sources() -> CatalogSources {
    let mut sources = once_a_day_sources();
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("task fixture");
    tasks["tasks"][0]["sensitive"] = json!(true);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("task bytes");
    sources
}

/// An operator pause (or a deploy's drain) cut the run off at its drain timeout
/// (`contained_task_paused`), recoverable. Since 0.12.28 the shipped table lists it (Alice's
/// decision 92, 2026-10-29).
fn operator_paused() -> PolicyExecutionInput {
    PolicyExecutionInput::Failed {
        outcome: Outcome::new(RuntimeErrorCode::ContainedTaskPaused.code()),
        failure_identity: None,
        class: PolicyFailureClass::Recoverable,
    }
}

/// The run was cancelled (`contained_task_cancelled`), recoverable: a planned stop like the
/// operator pause, which the shipped table does not list.
fn cancelled() -> PolicyExecutionInput {
    PolicyExecutionInput::Failed {
        outcome: Outcome::new(RuntimeErrorCode::ContainedTaskCancelled.code()),
        failure_identity: None,
        class: PolicyFailureClass::Recoverable,
    }
}

/// A configuration whose explicit keys are `settings`.
pub(super) fn rerun_manifest(settings: Vec<(&str, FactScalar)>) -> RuntimeConfigManifest {
    let mut parameters = settings
        .into_iter()
        .map(|(key, value)| ConfigParameter {
            key: key.to_owned(),
            value,
            source: ConfigParameterSource::Explicit,
            settable: false,
        })
        .collect::<Vec<_>>();
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// `scheduling.rerun.condition_codes` listing `codes`.
pub(super) fn condition_codes(codes: &[&str]) -> (&'static str, FactScalar) {
    (
        RERUN_CONDITION_CODES,
        FactScalar::String(serde_json::to_string(codes).expect("the code list")),
    )
}

/// A host on `root` for the fixture instance `registered`, its Runtime clock a manual one at
/// `at`; `manifest` its configuration's explicit keys, when there are any.
pub(super) fn start(
    root: &TempDir,
    registered: InstanceId,
    at: u64,
    manifest: Option<RuntimeConfigManifest>,
) -> (RuntimeHost, Arc<ManualRuntimeClock>) {
    let clock = Arc::new(ManualRuntimeClock::new(at, 0));
    let mut host_config = config(root).with_runtime_clock(clock.clone());
    if let Some(manifest) = manifest {
        host_config = host_config.with_config_manifest(manifest);
    }
    let host = RuntimeHost::start(
        host_config,
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    (host, clock)
}

/// The pair's dispatch evaluated and admitted at `at` (the catalog approved first when
/// `approve`); its intent and its admission.
pub(super) fn admit(
    host: &RuntimeHost,
    at: u64,
    seed: u64,
    approve: bool,
) -> (DispatchIntent, PolicyAdmissionRecord) {
    let (_, intent, reasons) =
        evaluated_policy_dispatch_at(host, PolicyTrigger::Recovery, at, seed);
    if approve {
        record_policy_approval(host, &intent);
    }
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(host, &intent))
        .expect("the admission")
    else {
        panic!("expected a granted admission")
    };
    (intent, context.admission().clone())
}

/// One run of the pair on a fresh host at `at`: admitted, 75 s later settled as `input`; the
/// host is closed. Returns the run's admission and its failure record.
fn one_run(
    root: &TempDir,
    registered: InstanceId,
    at: u64,
    seed: u64,
    first: Option<&CatalogSources>,
    manifest: Option<RuntimeConfigManifest>,
    input: &PolicyExecutionInput,
) -> (PolicyAdmissionRecord, PolicyFailureRecord) {
    let (host, clock) = start(root, registered, at, manifest);
    if let Some(sources) = first {
        host.activate_policy_catalog(sources)
            .expect("activate the catalog");
    }
    let (intent, admission) = admit(&host, at, seed, first.is_some());
    clock.advance(75_000);
    let settled = host
        .record_policy_dispatch_outcome(&intent.decision_id, input)
        .expect("the settlement");
    let failure = failure_of(&settled);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
    (admission, failure)
}

pub(super) fn failure_of(
    data: &actingcommand_contract::PolicyExecutionEventData,
) -> PolicyFailureRecord {
    let PolicyExecutionOutcome::Failed { failure } = &data.outcome else {
        panic!("expected a failure: {data:?}")
    };
    failure.clone()
}

/// The pair's value record as the host holds it, which must equal the one today's replay of
/// every dispatch event rebuilds.
fn held_record(host: &RuntimeHost) -> PolicyControlStateRecord {
    let (held, replayed, _) = host
        .decision_memory_for_test()
        .expect("the decision memory");
    let key = format!("decision_memory/{POLICY_INSTANCE_ALIAS}/{TASK}");
    let record = |records: &BTreeMap<String, PolicyValueRecord>| match records.get(&key) {
        Some(PolicyValueRecord::Control(record)) => (**record).clone(),
        other => panic!("the pair's value record: {other:?}"),
    };
    assert_eq!(record(&held), record(&replayed), "held and replayed");
    record(&held)
}

/// The pair's latest L7 record in the ledger.
fn latest_ledger_record(host: &RuntimeHost) -> PolicyControlStateRecord {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::PolicyControlStateRecorded),
        ..EventQuery::default()
    })
    .expect("the control state records")
    .iter()
    .filter_map(|event| match event.payload() {
        EventPayload::Policy(PolicyPayload::ControlStateRecorded(payload))
            if payload.state().task_id == TASK =>
        {
            Some(payload.state().clone())
        }
        _ => None,
    })
    .next_back()
    .expect("the pair's L7 record")
}

/// The `policy_rerun_exhausted` records (`goal_missed` planning signals).
fn exhausted_records(host: &RuntimeHost) -> Vec<(Option<String>, String, Outcome)> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::PolicyPlanningSignalObserved),
        ..EventQuery::default()
    })
    .expect("the planning signals")
    .iter()
    .filter_map(|event| match event.payload() {
        EventPayload::Policy(PolicyPayload::PlanningSignalObserved(payload))
            if payload.kind() == PolicyPlanningSignalKind::GoalMissed
                && payload.outcome().code().as_str() == EXHAUSTED_CODE =>
        {
            Some((
                payload.task_id().map(str::to_owned),
                payload.instance_id().to_owned(),
                payload.outcome().clone(),
            ))
        }
        _ => None,
    })
    .collect()
}

// ---- ①

// A recoverable sampling timeout of a task that is not sensitive ends by a condition:
// `condition{refunded: true, count: 1, expires_at_unix_ms: the window's end}`, the streak does
// not grow (the record keeps the pair's counters, none here, and the pair has no failure
// streak), and the once-a-day task enters again the same day.
#[test]
fn sr2b1_t1_a_sampling_timeout_ends_by_a_condition_refunds_and_keeps_the_streak() {
    let root = TempDir::new().expect("tempdir");
    let registered = instance_id();
    let (host, clock) = start(&root, registered, POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&once_a_day_sources())
        .expect("activate the catalog");
    let (first, admission) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    assert_eq!(admission.budget.task_daily_used, 1);
    clock.advance(75_000);
    let ended = failure_of(
        &host
            .record_policy_dispatch_outcome(&first.decision_id, &sampling_ended())
            .expect("the condition settlement"),
    );
    assert_eq!(
        ended.condition,
        Some(PolicyConditionSettlement {
            refunded: true,
            count: 1,
            expires_at_unix_ms: whole_day_window_end(POLICY_NOW_UNIX_MS),
        })
    );
    assert_eq!(ended.consecutive_same_error, 0);
    assert_eq!(ended.escalation_streak, 0);
    assert_eq!(ended.effective_class, PolicyFailureClass::Recoverable);
    assert_eq!(ended.disposition, PolicyFailureDisposition::Continue);
    assert!(held_record(&host).failure_streak.is_none(), "no streak");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");

    // Later the same day the once-a-day task is admitted again: its count was given back.
    let at = POLICY_NOW_UNIX_MS + BETWEEN_RUNS_MS;
    let (host, _clock) = start(&root, registered, at, None);
    let (_, rerun) = admit(&host, at, 101, false);
    assert_eq!(
        rerun.budget.task_daily_used, 1,
        "the daily count was given back"
    );
    assert_eq!(
        rerun.budget.task_window_used, 1,
        "the window count was given back"
    );
    host.close().expect("close the host");
}

// ---- ②

// A sensitive task's sampling timeout, a code the shipped table does not list
// (`contained_task_cancelled`), and a code a configured table does not list each settle as an
// ordinary failure; the configured table is read, so a code it lists ends by a condition.
// Adapted for Alice's decision 92 (0.12.28): the shipped table's unlisted code was the operator
// pause's `contained_task_paused`, which the shipped table now lists (test `sr92` below).
#[test]
fn sr2b1_t2_a_sensitive_task_or_an_unlisted_code_is_an_ordinary_failure() {
    // A sensitive task.
    let root = TempDir::new().expect("tempdir");
    let (_, failure) = one_run(
        &root,
        instance_id(),
        POLICY_NOW_UNIX_MS,
        100,
        Some(&sensitive_once_a_day_sources()),
        None,
        &sampling_ended(),
    );
    assert_eq!(failure.condition, None, "sensitive");
    assert!(failure.sensitive);
    assert_eq!(failure.effective_class, PolicyFailureClass::Severe);

    // The shipped table does not list a cancellation.
    let root = TempDir::new().expect("tempdir");
    let (_, failure) = one_run(
        &root,
        instance_id(),
        POLICY_NOW_UNIX_MS,
        100,
        Some(&budget_policy_sources(1)),
        None,
        &cancelled(),
    );
    assert_eq!(failure.condition, None, "not in the shipped table");
    assert_eq!(failure.consecutive_same_error, 1);

    // A configured table that lists only the operator pause.
    let root = TempDir::new().expect("tempdir");
    let registered = instance_id();
    let table = || rerun_manifest(vec![condition_codes(&["contained_task_paused"])]);
    let (_, failure) = one_run(
        &root,
        registered,
        POLICY_NOW_UNIX_MS,
        100,
        Some(&budget_policy_sources(1)),
        Some(table()),
        &sampling_ended(),
    );
    assert_eq!(failure.condition, None, "not in the configured table");
    assert_eq!(failure.consecutive_same_error, 1);
    let at = POLICY_NOW_UNIX_MS + BETWEEN_RUNS_MS;
    let (_, failure) = one_run(
        &root,
        registered,
        at,
        101,
        None,
        Some(table()),
        &operator_paused(),
    );
    assert_eq!(
        failure.condition,
        Some(PolicyConditionSettlement {
            refunded: true,
            count: 1,
            expires_at_unix_ms: whole_day_window_end(at),
        }),
        "the configured table lists it"
    );
}

// ---- Decision 92

// Alice's decision 92 (2026-10-29, Runtime 0.12.28): the shipped
// `scheduling.rerun.condition_codes` lists `contained_task_paused`. Under the shipped table, a
// scheduled run of a task that is not sensitive which an operator pause cut off ends by a
// condition: `condition{refunded: true, count: 1, expires_at_unix_ms: the window's end}`, no
// streak (the record keeps the pair's counters, none here, and the pair has no failure streak),
// and the rerun is owed with the pause's code. After the resume, later the same day, the
// once-a-day task is dispatched again as the owed rerun (`policy_rerun_owed`), its daily and
// window counts given back. The same pause of a sensitive task stays an ordinary failure (SR-2a).
#[test]
fn sr92_an_operator_pause_ends_a_scheduled_run_by_a_condition_under_the_shipped_table() {
    let paused_code = RuntimeErrorCode::ContainedTaskPaused.code();
    let root = TempDir::new().expect("tempdir");
    let registered = instance_id();
    let window_end = whole_day_window_end(POLICY_NOW_UNIX_MS);
    let (host, clock) = start(&root, registered, POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&once_a_day_sources())
        .expect("activate the catalog");
    let (first, admission) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    assert_eq!(admission.budget.task_daily_used, 1);
    clock.advance(75_000);
    let paused = failure_of(
        &host
            .record_policy_dispatch_outcome(&first.decision_id, &operator_paused())
            .expect("the condition settlement"),
    );
    assert_eq!(
        paused.condition,
        Some(PolicyConditionSettlement {
            refunded: true,
            count: 1,
            expires_at_unix_ms: window_end,
        }),
        "the shipped table lists the operator pause"
    );
    assert_eq!(paused.consecutive_same_error, 0);
    assert_eq!(paused.escalation_streak, 0);
    assert_eq!(paused.effective_class, PolicyFailureClass::Recoverable);
    assert_eq!(paused.disposition, PolicyFailureDisposition::Continue);
    let record = held_record(&host);
    assert!(record.failure_streak.is_none(), "no streak");
    let owed = record.condition_reruns.expect("the owed rerun");
    assert_eq!(owed.failure_code.as_str(), paused_code.as_str());
    assert_eq!(owed.count, 1);
    assert_eq!(owed.expires_at_unix_ms, window_end);

    // After the resume: the owed rerun is promoted and admitted, its counts given back.
    let at = POLICY_NOW_UNIX_MS + BETWEEN_RUNS_MS;
    clock.set_unix_ms(at);
    let (_, rerun, chain) = evaluated_policy_dispatch_at(&host, PolicyTrigger::Recovery, at, 101);
    assert_eq!(rerun.task_id, TASK);
    let codes = chain
        .reasons
        .iter()
        .map(|reason| reason.outcome.code().as_str().to_owned())
        .collect::<Vec<_>>();
    let promoted = chain
        .reasons
        .iter()
        .map(|reason| &reason.outcome)
        .find(|outcome| outcome.code().as_str() == "policy_rerun_owed")
        .unwrap_or_else(|| panic!("the rerun is owed: {codes:?}"));
    assert_eq!(
        promoted.value("failure_code"),
        Some(&json!(paused_code.as_str()))
    );
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&rerun, &chain, &policy_context(&host, &rerun))
        .expect("the rerun's admission")
    else {
        panic!("expected a granted admission")
    };
    assert_eq!(
        context.admission().budget.task_daily_used,
        1,
        "the daily count was given back"
    );
    assert_eq!(
        context.admission().budget.task_window_used,
        1,
        "the window count was given back"
    );
    drop(context);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");

    // A sensitive task's paused run stays an ordinary failure under the shipped table.
    let root = TempDir::new().expect("tempdir");
    let (_, failure) = one_run(
        &root,
        instance_id(),
        POLICY_NOW_UNIX_MS,
        100,
        Some(&sensitive_once_a_day_sources()),
        None,
        &operator_paused(),
    );
    assert_eq!(failure.condition, None, "sensitive");
    assert!(failure.sensitive);
    assert_eq!(failure.effective_class, PolicyFailureClass::Severe);
}

// ---- ③

// With `max_per_occurrence = 2` (shipped), the pair's third consecutive condition termination
// in one occurrence settles as an ordinary recoverable failure, without `condition` and not
// refunded, and records `policy_rerun_exhausted` (warning) once, as a `goal_missed` planning
// signal with the count and the limit; the owed rerun is gone. The once-a-day task then cannot
// enter again that day: it ran no more often than its refunded count allows.
#[test]
fn sr2b1_t3_the_third_condition_termination_is_exhausted() {
    let root = TempDir::new().expect("tempdir");
    let registered = instance_id();
    let window_end = whole_day_window_end(POLICY_NOW_UNIX_MS);
    for (index, count) in [(0_u64, 1_u32), (1, 2)] {
        let (admission, failure) = one_run(
            &root,
            registered,
            POLICY_NOW_UNIX_MS + index * BETWEEN_RUNS_MS,
            100 + index,
            (index == 0).then(once_a_day_sources).as_ref(),
            None,
            &sampling_ended(),
        );
        assert_eq!(admission.budget.task_daily_used, 1, "run {index}");
        assert_eq!(
            failure.condition,
            Some(PolicyConditionSettlement {
                refunded: true,
                count,
                expires_at_unix_ms: window_end,
            }),
            "run {index}"
        );
    }

    let at = POLICY_NOW_UNIX_MS + 2 * BETWEEN_RUNS_MS;
    let (host, clock) = start(&root, registered, at, None);
    let (third, admission) = admit(&host, at, 102, false);
    assert_eq!(admission.budget.task_daily_used, 1);
    clock.advance(75_000);
    let failure = failure_of(
        &host
            .record_policy_dispatch_outcome(&third.decision_id, &sampling_ended())
            .expect("the exhausted settlement"),
    );
    assert_eq!(failure.condition, None, "an ordinary failure");
    assert_eq!(failure.original_class, PolicyFailureClass::Recoverable);
    assert_eq!(failure.consecutive_same_error, 1, "its first real failure");
    let records = exhausted_records(&host);
    let [(task_id, instance_id, outcome)] = records.as_slice() else {
        panic!("one policy_rerun_exhausted: {records:#?}")
    };
    assert_eq!(task_id.as_deref(), Some(TASK));
    assert_eq!(instance_id, POLICY_INSTANCE_ALIAS);
    assert_eq!(outcome.value("task_id"), Some(&json!(TASK)));
    assert_eq!(
        outcome.value("instance_id"),
        Some(&json!(POLICY_INSTANCE_ALIAS))
    );
    assert_eq!(outcome.value("count"), Some(&json!(3)));
    assert_eq!(outcome.value("limit_count"), Some(&json!(2)));
    assert_eq!(
        held_record(&host).condition_reruns,
        None,
        "no rerun is owed"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");

    // The exhausted run kept its count: the once-a-day task is not admitted again that day.
    let at = at + BETWEEN_RUNS_MS;
    let (host, _clock) = start(&root, registered, at, None);
    let evaluation = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: at,
                monotonic_ms: at,
            },
            103,
            PolicyTrigger::Recovery,
        )
        .expect("the evaluation")
        .evaluation
        .expect("an evaluation");
    assert!(evaluation.dispatch_intents.is_empty(), "{evaluation:#?}");
    assert!(
        evaluation.decisions[0]
            .reasons
            .iter()
            .any(|reason| reason.outcome.code().as_str() == "policy_budget_exhausted"),
        "{evaluation:#?}"
    );
    assert_eq!(exhausted_records(&host).len(), 1, "recorded once");
    host.close().expect("close the host");
}

// ---- ④

// With `scheduling.rerun.refund_counts = false`, the condition records `refunded: false` and the
// run keeps its counts: the next admission the same day is the second.
#[test]
fn sr2b1_t4_without_refund_counts_the_counts_are_kept() {
    let root = TempDir::new().expect("tempdir");
    let registered = instance_id();
    let kept = || rerun_manifest(vec![(RERUN_REFUND_COUNTS, FactScalar::Boolean(false))]);
    let (admission, failure) = one_run(
        &root,
        registered,
        POLICY_NOW_UNIX_MS,
        100,
        Some(&budget_policy_sources(1)),
        Some(kept()),
        &sampling_ended(),
    );
    assert_eq!(admission.budget.task_daily_used, 1);
    assert_eq!(
        failure.condition,
        Some(PolicyConditionSettlement {
            refunded: false,
            count: 1,
            expires_at_unix_ms: whole_day_window_end(POLICY_NOW_UNIX_MS),
        })
    );
    let at = POLICY_NOW_UNIX_MS + BETWEEN_RUNS_MS;
    let (host, _clock) = start(&root, registered, at, Some(kept()));
    let (_, next) = admit(&host, at, 101, false);
    assert_eq!(
        next.budget.task_daily_used, 2,
        "the count was not given back"
    );
    assert_eq!(next.budget.task_window_used, 2);
    host.close().expect("close the host");
}

// ---- ⑤

// The pair's L7 record carries `condition_reruns` as the settlement wrote it; a restart loads it
// from the record (the root's second start loads the tagged state), the owed rerun is back, and
// the pair's next condition termination in the same window counts on from it.
#[test]
fn sr2b1_t5_l7_carries_the_owed_rerun_across_a_restart() {
    let root = TempDir::new().expect("tempdir");
    let registered = instance_id();
    let window_end = whole_day_window_end(POLICY_NOW_UNIX_MS);
    let (host, clock) = start(&root, registered, POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&once_a_day_sources())
        .expect("activate the catalog");
    let (first, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    clock.advance(75_000);
    host.record_policy_dispatch_outcome(&first.decision_id, &sampling_ended())
        .expect("the condition settlement");
    let written = latest_ledger_record(&host)
        .condition_reruns
        .expect("the L7 record owes a rerun");
    assert_eq!(written.failure_code.as_str(), SAMPLING_CODE);
    assert_eq!(written.count, 1);
    assert_eq!(written.expires_at_unix_ms, window_end);
    host.close().expect("close the host");

    let at = POLICY_NOW_UNIX_MS + BETWEEN_RUNS_MS;
    let (host, clock) = start(&root, registered, at, None);
    assert_eq!(
        held_record(&host).condition_reruns,
        Some(written),
        "the owed rerun is loaded"
    );
    let (rerun, _) = admit(&host, at, 101, false);
    clock.advance(75_000);
    let failure = failure_of(
        &host
            .record_policy_dispatch_outcome(&rerun.decision_id, &sampling_ended())
            .expect("the second condition settlement"),
    );
    assert_eq!(
        failure.condition,
        Some(PolicyConditionSettlement {
            refunded: true,
            count: 2,
            expires_at_unix_ms: window_end,
        })
    );
    let owed = latest_ledger_record(&host)
        .condition_reruns
        .expect("still owed");
    assert_eq!(owed.count, 2);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// ---- ⑥

/// The failure code and class of the run's one `task.failed` terminal.
fn terminal_failure(host: &RuntimeHost, run_id: RunId) -> (String, PolicyFailureClass) {
    let terminals = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::TaskFailed),
            run_id: Some(run_id),
            ..EventQuery::default()
        })
        .expect("the run's terminal");
    let [terminal] = terminals.as_slice() else {
        panic!("one task.failed: {terminals:#?}")
    };
    let EventPayload::Task(TaskPayload::Semantic(payload)) = terminal.payload() else {
        panic!("a semantic terminal")
    };
    let TaskSemanticFact::TerminalCommitted {
        task_result: TaskOutcome::Failure,
        outcome: Some(outcome),
        failure_severe,
        ..
    } = payload.fact()
    else {
        panic!("a failed terminal")
    };
    (
        actingcommand_contract::scheduled_failure_code_str(outcome)
            .as_str()
            .to_owned(),
        actingcommand_contract::scheduled_failure_class(outcome.code(), *failure_severe),
    )
}

/// The one execution record of `decision_id`.
fn execution_failure(host: &RuntimeHost, decision_id: &str) -> PolicyFailureRecord {
    let records = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::PolicyExecutionRecorded),
            ..EventQuery::default()
        })
        .expect("the execution records")
        .into_iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload))
                if payload.decision_id() == decision_id =>
            {
                match payload.policy_result() {
                    PolicyExecutionOutcome::Failed { failure } => Some(failure.clone()),
                    PolicyExecutionOutcome::Succeeded { .. } => None,
                }
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let [failure] = records.as_slice() else {
        panic!("one failed execution record: {records:#?}")
    };
    failure.clone()
}

/// The scheduled run's live settlement fails at its execution append (as the daemon dying there
/// leaves it); the Runtime latches on that ledger failure, as a crash ends it.
fn run_unsettled(host: &RuntimeHost, context: &PolicyRunContext, package_path: &Path) {
    host.fail_next_policy_execution_append_for_test()
        .expect("arm the append failure");
    let request = ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
        .expect("the scheduled request");
    let error = host
        .run_scheduled_contained_task(context, &request)
        .expect_err("the settlement's append fails");
    assert_eq!(error.code(), "ledger_failure");
}

/// Closes a host whose Runtime latched on the injected ledger failure.
fn close_latched(host: RuntimeHost) {
    if let Err(error) = host.close() {
        assert_eq!(error.code(), "ledger_failure", "{error:?}");
    }
}

/// A scheduled run on a fresh host on `root` whose recoverable `task.failed` terminal is written
/// but which is not settled before the host closes (the fixture of `crash_recovery`'s
/// `task-recoverable` case, in process). Returns the run's decision id, its terminal's failure
/// code and its admission time.
fn unsettled_run_with_terminal(
    root: &TempDir,
    registered: InstanceId,
    package: &[u8],
) -> (String, String, u64) {
    let package_path = root.path().join("scheduled-task.zip");
    fs::write(&package_path, package).expect("write the package");
    let state = Arc::new(FakeState::default());
    state.fail_capture.store(true, Ordering::Release);
    state
        .transient_capture_failure
        .store(true, Ordering::Release);
    let host = RuntimeHost::start(
        config(root).with_procedure_manifest(procedure_manifest_with_primary(
            package,
            vec!["after_observation".to_owned()],
        )),
        Arc::new(FakeProvider::one(POLICY_INSTANCE_ALIAS, registered, state).fixture_simulation()),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate the catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("the admission")
    else {
        panic!("expected a granted admission")
    };
    let admitted_at = context.admission().activity.admitted_at_unix_ms;
    run_unsettled(&host, &context, &package_path);
    let (code, class) = terminal_failure(&host, context.run_id());
    assert_eq!(class, PolicyFailureClass::Recoverable, "{code}");
    drop(context);
    close_latched(host);
    (intent.decision_id, code, admitted_at)
}

// A scheduled run whose `task.failed` terminal is written but which was not settled before the
// restart (the fixture of `crash_recovery`'s `task-recoverable` case, in process): the start's
// reconciliation settles it from its terminal by the live rule, here with a configured table
// listing the terminal's code, so it ends by a condition. Over `max_per_occurrence` it settles as
// an ordinary failure, and the start records its `policy_rerun_exhausted` itself, before any
// evaluation (fix round 1, review L3). A run with no terminal (its lease expired before it ran)
// is settled as `policy_settlement_interrupted`, Severe and without `condition`, even with that
// code listed.
#[test]
fn sr2b1_t6_the_start_settles_a_run_from_its_terminal_by_the_same_rule() {
    // A run with a recoverable `task.failed` terminal.
    let root = TempDir::new().expect("tempdir");
    let registered = instance_id();
    let package = neutral_contained_task_package(false);
    let (decision_id, code, admitted_at) = unsettled_run_with_terminal(&root, registered, &package);

    // The restart's configuration lists the terminal's code as a condition code.
    let table = rerun_manifest(vec![condition_codes(&[code.as_str()])]);
    let host = RuntimeHost::start(
        config(&root).with_config_manifest(table),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("the restart settles the run");
    let failure = execution_failure(&host, &decision_id);
    assert_eq!(
        actingcommand_contract::scheduled_failure_code_str(&failure.outcome).as_str(),
        code
    );
    assert_eq!(
        failure.condition,
        Some(PolicyConditionSettlement {
            refunded: true,
            count: 1,
            expires_at_unix_ms: daytime_window_end(admitted_at),
        }),
        "settled from its terminal by the live rule"
    );
    let owed = held_record(&host).condition_reruns.expect("the owed rerun");
    assert_eq!(owed.failure_code.as_str(), code);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the restarted host");

    // The same run with `max_per_occurrence = 0`: its first condition termination is over the
    // limit, so the start settles it as an ordinary failure and records the warning before it
    // returns; no evaluation runs here.
    let root = TempDir::new().expect("tempdir");
    let registered = instance_id();
    let (decision_id, code, _) = unsettled_run_with_terminal(&root, registered, &package);
    let table = rerun_manifest(vec![
        condition_codes(&[code.as_str()]),
        (RERUN_MAX_PER_OCCURRENCE, FactScalar::Integer(0)),
    ]);
    let host = RuntimeHost::start(
        config(&root).with_config_manifest(table),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("the restart settles the run");
    assert_eq!(
        execution_failure(&host, &decision_id).condition,
        None,
        "an ordinary failure"
    );
    assert_eq!(
        exhausted_records(&host)
            .iter()
            .map(|(_, _, outcome)| (outcome.value("count"), outcome.value("limit_count")))
            .collect::<Vec<_>>(),
        vec![(Some(&json!(1)), Some(&json!(0)))],
        "the start records policy_rerun_exhausted without waiting for an evaluation"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the restarted host");

    // A run with no terminal: its lease expired before it ran.
    let root = TempDir::new().expect("tempdir");
    let registered = instance_id();
    fs::write(package_path_in(&root), &package).expect("write the package");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_scheduler(SchedulerConfig {
                maximum_client_heartbeat_interval_ms: 20,
                takeover_cooldown_ms: 40,
                lease_ttl_ms: 200,
                ..SchedulerConfig::default()
            })
            .with_procedure_manifest(procedure_manifest_with_primary(
                &package,
                vec!["after_observation".to_owned()],
            )),
        Arc::new(
            FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                registered,
                Arc::new(FakeState::default()),
            )
            .fixture_simulation(),
        ),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate the catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("the admission")
    else {
        panic!("expected a granted admission")
    };
    clock.advance(250);
    host.expire_lease_once_for_test(context.lease_token())
        .expect("expire the run's lease");
    run_unsettled(&host, &context, &package_path_in(&root));
    drop(context);
    close_latched(host);

    let interrupted =
        actingcommand_contract::codes::SharedCode::PolicySettlementInterrupted.as_str();
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(Arc::new(ManualRuntimeClock::new(POLICY_RESTART_UNIX_MS, 0)))
            .with_config_manifest(rerun_manifest(vec![condition_codes(&[interrupted])])),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("the restart settles the run");
    let failure = execution_failure(&host, &intent.decision_id);
    assert_eq!(failure.outcome.code().as_str(), interrupted);
    assert_eq!(failure.original_class, PolicyFailureClass::Severe);
    assert_eq!(failure.condition, None, "never by a condition");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the restarted host");
}

fn package_path_in(root: &TempDir) -> PathBuf {
    root.path().join("scheduled-task.zip")
}
