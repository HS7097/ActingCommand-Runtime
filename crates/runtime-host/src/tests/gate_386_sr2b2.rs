// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #386 SR-2b-2 (design `rd-1028/design-p22.md` §6.2 and §8 row SR-2b-2, tests ② to ⑤,
// and the coordinator's decision on the SR-2b-1 review's M1; `contracts/policy-suspension.md`,
// "The rerun", "Limits", "After a restart"): a rerun a condition termination owes enters the
// policy evaluation with `policy_rerun_owed`; the instance's pacing stage gates it
// (`policy_rerun_waiting{pacing_stage}` above `scheduling.rerun.resume_stage`, and for
// `resume_settle_ms` after it is back at or below it); past its window end it expires with
// `policy_rerun_expired` and the run's failure shows in the snapshot again; while owed, the run's
// terminal is hidden so no `dependency_completed(head, *)` becomes true; `actingd suspended`
// lists the owed reruns; and a pair exhausted in this occurrence is not promoted again in it.
// Test ① (the ranking) is the policy crate's `gate_386_sr2b2`. Every expected value is the
// design's or the work order's; the Runtime clock is a manual one, the pacing stage a pinned one.

use super::gate_386_sr2a::{once_a_day_sources, sampling_ended};
use super::gate_386_sr2b1::{
    TASK, condition_codes, failure_of, rerun_manifest, start, whole_day_window_end,
};
use super::*;
use crate::failure_identity::{FailureIdentity, IdentityFrame, IdentityPackage};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{PerformancePacingStage, PolicyFailureRecord};
use actingcommand_policy::{
    PolicyEvaluation, TaskDecision, TaskRuntimeSnapshot, TaskTerminalState,
};
use serde_json::{Value, json};

const SAMPLING_CODE: &str = "recognition_sample_deadline_exceeded";
const OWED: &str = "policy_rerun_owed";
const WAITING: &str = "policy_rerun_waiting";
const EXPIRED: &str = "policy_rerun_expired";
const IMMEDIATE: &str = "failure_retry_immediate";
const SETTLE_MS: u64 = 60_000;
const MINUTE_MS: u64 = 60_000;
const SETTLED_AFTER_MS: u64 = 75_000;

/// The pair's run admitted at `at` and, 75 s later, settled as `input`; the clock is set to `at`
/// first. Returns the admission's reason chain codes and the run's failure record.
fn run_at(
    host: &RuntimeHost,
    clock: &ManualRuntimeClock,
    at: u64,
    seed: u64,
    approve: bool,
    input: &PolicyExecutionInput,
) -> (Vec<String>, PolicyFailureRecord) {
    clock.set_unix_ms(at);
    let (cycle, intent, chain) =
        evaluated_policy_dispatch_at(host, PolicyTrigger::Recovery, at, seed);
    drop(cycle);
    if approve {
        record_policy_approval(host, &intent);
    }
    let PolicyDispatchAdmission::Granted { .. } = host
        .admit_policy_dispatch(&intent, &chain, &policy_context(host, &intent))
        .expect("the admission")
    else {
        panic!("expected a granted admission")
    };
    clock.advance(SETTLED_AFTER_MS);
    let failure = failure_of(
        &host
            .record_policy_dispatch_outcome(&intent.decision_id, input)
            .expect("the settlement"),
    );
    let codes = chain
        .reasons
        .iter()
        .map(|reason| reason.outcome.code().as_str().to_owned())
        .collect();
    (codes, failure)
}

/// One policy evaluation at `at` (the Runtime clock set to it first).
fn evaluate_at(
    host: &RuntimeHost,
    clock: &ManualRuntimeClock,
    at: u64,
    seed: u64,
) -> PolicyEvaluation {
    clock.set_unix_ms(at);
    host.evaluate_policy_cycle_with_test_inputs(
        &policy_facts(),
        &policy_resources(),
        EvaluationTime {
            unix_ms: at,
            monotonic_ms: at,
        },
        seed,
        PolicyTrigger::Recovery,
    )
    .expect("the evaluation")
    .evaluation
    .expect("an evaluation")
}

fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == task_id
                && decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS)
        })
        .unwrap_or_else(|| panic!("a decision of {task_id}: {evaluation:#?}"))
}

fn reason<'a>(decision: &'a TaskDecision, code: &str) -> Option<&'a Outcome> {
    decision
        .reasons
        .iter()
        .map(|reason| &reason.outcome)
        .find(|outcome| outcome.code().as_str() == code)
}

fn dispatched(evaluation: &PolicyEvaluation, task_id: &str) -> bool {
    evaluation
        .dispatch_intents
        .iter()
        .any(|intent| intent.task_id == task_id)
}

/// The `policy_rerun_<kind>` records (`goal_missed` planning signals) of `code`.
fn rerun_records(host: &RuntimeHost, code: &str) -> Vec<Outcome> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::PolicyPlanningSignalObserved),
        ..EventQuery::default()
    })
    .expect("the planning signals")
    .iter()
    .filter_map(|event| match event.payload() {
        EventPayload::Policy(PolicyPayload::PlanningSignalObserved(payload))
            if payload.kind() == PolicyPlanningSignalKind::GoalMissed
                && payload.outcome().code().as_str() == code =>
        {
            Some(payload.outcome().clone())
        }
        _ => None,
    })
    .collect()
}

/// The pair's projected task snapshot as the evaluation at `at` reads it.
fn head_snapshot(host: &RuntimeHost, task_id: &str, at: u64) -> TaskRuntimeSnapshot {
    host.policy_task_snapshots_for_test(at)
        .expect("the task snapshots")
        .into_iter()
        .find(|state| state.task_id == task_id && state.instance_id == POLICY_INSTANCE_ALIAS)
        .unwrap_or_else(|| panic!("the snapshot of {task_id}"))
}

/// A fresh host at `at` with the once-a-day catalog whose pair ended by a sampling timeout:
/// the run admitted at `at`, settled 75 s later with `condition{count: 1}`.
fn owed_host(
    root: &TempDir,
    registered: InstanceId,
    at: u64,
    sources: &CatalogSources,
) -> (RuntimeHost, Arc<ManualRuntimeClock>) {
    let (host, clock) = start(root, registered, at, None);
    host.activate_policy_catalog(sources)
        .expect("activate the catalog");
    let (_, failure) = run_at(&host, &clock, at, 100, true, &sampling_ended());
    assert_eq!(
        failure.condition.map(|condition| condition.count),
        Some(1),
        "the run ended by a condition"
    );
    (host, clock)
}

// ---- ②

// At stage S3 (above the shipped resume stage S2) the owed rerun is deferred with
// `policy_rerun_waiting{pacing_stage: s3}` and carries `policy_rerun_owed`; back at S2 it still
// waits 59 s later (`pacing_stage: s2`, wake at the settle's end) and is dispatched 60 s later.
// Its first wait is recorded once.
#[test]
fn sr2b2_t2_the_stage_gate_holds_an_owed_rerun_until_s2_has_settled_60_s() {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = owed_host(
        &root,
        instance_id(),
        POLICY_NOW_UNIX_MS,
        &once_a_day_sources(),
    );
    host.pin_pacing_stage_for_test(POLICY_INSTANCE_ALIAS, PerformancePacingStage::S3)
        .expect("pin S3");
    let above = POLICY_NOW_UNIX_MS + 10 * MINUTE_MS;
    let evaluation = evaluate_at(&host, &clock, above, 201);
    assert!(!dispatched(&evaluation, TASK), "{evaluation:#?}");
    let pair = decision(&evaluation, TASK);
    let owed = reason(pair, OWED).unwrap_or_else(|| panic!("owed: {pair:#?}"));
    assert_eq!(owed.value("failure_code"), Some(&json!(SAMPLING_CODE)));
    assert_eq!(owed.value("count"), Some(&json!(1)));
    let waiting = reason(pair, WAITING).unwrap_or_else(|| panic!("waiting: {pair:#?}"));
    assert_eq!(waiting.value("pacing_stage"), Some(&json!("s3")));
    assert_eq!(waiting.value("next_eligible_unix_ms"), None);

    host.pin_pacing_stage_for_test(POLICY_INSTANCE_ALIAS, PerformancePacingStage::S2)
        .expect("pin S2");
    let back = above + MINUTE_MS;
    let evaluation = evaluate_at(&host, &clock, back, 202);
    let pair = decision(&evaluation, TASK);
    let waiting = reason(pair, WAITING).unwrap_or_else(|| panic!("settling: {pair:#?}"));
    assert_eq!(waiting.value("pacing_stage"), Some(&json!("s2")));
    assert_eq!(
        waiting.value("next_eligible_unix_ms"),
        Some(&json!(back + SETTLE_MS))
    );

    let evaluation = evaluate_at(&host, &clock, back + 59_000, 203);
    assert!(!dispatched(&evaluation, TASK), "59 s: {evaluation:#?}");
    assert!(
        reason(decision(&evaluation, TASK), WAITING).is_some(),
        "{evaluation:#?}"
    );

    let evaluation = evaluate_at(&host, &clock, back + SETTLE_MS, 204);
    assert!(dispatched(&evaluation, TASK), "60 s: {evaluation:#?}");
    let pair = decision(&evaluation, TASK);
    assert!(reason(pair, OWED).is_some(), "{pair:#?}");
    assert!(reason(pair, WAITING).is_none(), "{pair:#?}");
    assert_eq!(rerun_records(&host, WAITING).len(), 1, "recorded once");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// ---- ③

// The stage stays above the resume stage until the window ends: past the end the rerun is no
// longer owed (no `policy_rerun_owed`, no wait), `policy_rerun_expired` (warning) is recorded once
// with the window end, the run's `Failed` terminal is back in the snapshot, and the next day's
// round is the task's ordinary one.
#[test]
fn sr2b2_t3_past_the_window_end_the_rerun_expires_and_the_failure_returns() {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = owed_host(
        &root,
        instance_id(),
        POLICY_NOW_UNIX_MS,
        &once_a_day_sources(),
    );
    let window_end = whole_day_window_end(POLICY_NOW_UNIX_MS);
    host.pin_pacing_stage_for_test(POLICY_INSTANCE_ALIAS, PerformancePacingStage::S3)
        .expect("pin S3");
    let before = window_end - MINUTE_MS;
    let evaluation = evaluate_at(&host, &clock, before, 301);
    assert!(
        reason(decision(&evaluation, TASK), WAITING).is_some(),
        "{evaluation:#?}"
    );
    assert_eq!(head_snapshot(&host, TASK, before).terminal_state, None);
    assert!(rerun_records(&host, EXPIRED).is_empty());

    let after = window_end + 1_000;
    for seed in [302, 303] {
        let evaluation = evaluate_at(&host, &clock, after, seed);
        let pair = decision(&evaluation, TASK);
        assert!(reason(pair, OWED).is_none(), "not owed: {pair:#?}");
        assert!(reason(pair, WAITING).is_none(), "not waiting: {pair:#?}");
    }
    let records = rerun_records(&host, EXPIRED);
    let [expired] = records.as_slice() else {
        panic!("one policy_rerun_expired: {records:#?}")
    };
    assert_eq!(expired.value("task_id"), Some(&json!(TASK)));
    assert_eq!(
        expired.value("instance_id"),
        Some(&json!(POLICY_INSTANCE_ALIAS))
    );
    assert_eq!(expired.value("failure_code"), Some(&json!(SAMPLING_CODE)));
    assert_eq!(
        expired.value("window_end_unix_ms"),
        Some(&json!(window_end))
    );
    let snapshot = head_snapshot(&host, TASK, after);
    assert_eq!(snapshot.terminal_state, Some(TaskTerminalState::Failed));
    assert!(snapshot.completed_window.is_some(), "{snapshot:#?}");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// ---- ④

/// The once-a-day head (priority 100) and two successors (priority 1) on the same instance,
/// one triggered by `dependency_completed(head, succeeded)`, one by `(head, failed)`.
fn chain_sources() -> CatalogSources {
    let mut sources = once_a_day_sources();
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("task fixture");
    tasks["tasks"][0]["priority"] = json!(100);
    let successor = |id: &str, state: &str| {
        let mut task = tasks["tasks"][0].clone();
        task["id"] = json!(id);
        task["priority"] = json!(1);
        task["trigger"] = json!({
            "kind": "dependency_completed", "task_id": TASK, "terminal_states": [state]
        });
        task
    };
    let after_success = successor("fixture.after-success", "succeeded");
    let after_failure = successor("fixture.after-failure", "failed");
    tasks["tasks"]
        .as_array_mut()
        .expect("the task list")
        .extend([after_success, after_failure]);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("task bytes");
    sources
}

// While the head's rerun is owed, its terminal and completed window are hidden: neither
// `dependency_completed(head, succeeded)` nor `dependency_completed(head, failed)` is true, so
// neither successor is eligible. The same failure settled as an ordinary one (a configured table
// without the code) makes `(head, failed)` true.
#[test]
fn sr2b2_t4_while_owed_no_dependency_completed_of_the_head_is_true() {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = owed_host(&root, instance_id(), POLICY_NOW_UNIX_MS, &chain_sources());
    let at = POLICY_NOW_UNIX_MS + 10 * MINUTE_MS;
    let snapshot = head_snapshot(&host, TASK, at);
    assert_eq!(snapshot.terminal_state, None, "{snapshot:#?}");
    assert_eq!(snapshot.completed_window, None, "{snapshot:#?}");
    let evaluation = evaluate_at(&host, &clock, at, 401);
    for successor in ["fixture.after-success", "fixture.after-failure"] {
        let decision = decision(&evaluation, successor);
        assert!(
            reason(decision, "task_eligible").is_none(),
            "{successor} is not triggered: {decision:#?}"
        );
        assert!(!dispatched(&evaluation, successor), "{evaluation:#?}");
    }
    host.close().expect("close the host");

    // Control: the same failure, ordinary.
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(
        &root,
        instance_id(),
        POLICY_NOW_UNIX_MS,
        Some(rerun_manifest(vec![condition_codes(&[])])),
    );
    host.activate_policy_catalog(&chain_sources())
        .expect("activate the catalog");
    let (_, failure) = run_at(
        &host,
        &clock,
        POLICY_NOW_UNIX_MS,
        100,
        true,
        &sampling_ended(),
    );
    assert_eq!(failure.condition, None, "an ordinary failure");
    assert_eq!(
        head_snapshot(&host, TASK, at).terminal_state,
        Some(TaskTerminalState::Failed)
    );
    let evaluation = evaluate_at(&host, &clock, at, 402);
    let after_failure = decision(&evaluation, "fixture.after-failure");
    assert!(
        reason(after_failure, "task_eligible").is_some(),
        "{after_failure:#?}"
    );
    host.close().expect("close the host");
}

// ---- ⑤

/// `actingd suspended` on the closed state root.
fn reruns_report(root: &TempDir) -> Value {
    let report = crate::suspension_report(&SuspensionReportRequest {
        config_path: Path::new("actingd.config.json"),
        config_modified_unix_ms: None,
        host: &config(root),
        catalog: &once_a_day_sources(),
    })
    .unwrap_or_else(|error| panic!("actingd suspended: {error}"));
    report["reruns"].clone()
}

// `actingd suspended` lists the owed rerun from the pair's value record: `owed` after the
// condition termination, `waiting` once the stage gate held it back, `expired` once the ledger's
// time passed its window end; each row with the code, the count and the window end.
#[test]
fn sr2b2_t5_actingd_suspended_lists_owed_waiting_and_expired_reruns() {
    let root = TempDir::new().expect("tempdir");
    let window_end = whole_day_window_end(POLICY_NOW_UNIX_MS);
    let row = |state: &str| {
        json!([{
            "task_id": TASK,
            "instance_id": POLICY_INSTANCE_ALIAS,
            "failure_code": SAMPLING_CODE,
            "count": 1,
            "expires_at_unix_ms": window_end,
            "state": state,
        }])
    };
    let registered = instance_id();
    let (host, _clock) = owed_host(&root, registered, POLICY_NOW_UNIX_MS, &once_a_day_sources());
    host.close().expect("close the host");
    assert_eq!(reruns_report(&root), row("owed"));

    let at = POLICY_NOW_UNIX_MS + 10 * MINUTE_MS;
    let (host, clock) = start(&root, registered, at, None);
    host.pin_pacing_stage_for_test(POLICY_INSTANCE_ALIAS, PerformancePacingStage::S3)
        .expect("pin S3");
    let evaluation = evaluate_at(&host, &clock, at, 501);
    assert!(
        reason(decision(&evaluation, TASK), WAITING).is_some(),
        "{evaluation:#?}"
    );
    host.close().expect("close the host");
    assert_eq!(reruns_report(&root), row("waiting"));

    let after = window_end + 1_000;
    let (host, clock) = start(&root, registered, after, None);
    host.pin_pacing_stage_for_test(POLICY_INSTANCE_ALIAS, PerformancePacingStage::S3)
        .expect("pin S3");
    evaluate_at(&host, &clock, after, 502);
    host.close().expect("close the host");
    assert_eq!(reruns_report(&root), row("expired"));
}

// ---- Coordinator decision (SR-2b-1 review M1)

// With `max_per_occurrence = 2` (shipped), the pair's third condition termination in the window
// is exhausted (an ordinary failure). The pair's next condition termination in the same window
// owes a rerun again (count 1), but it is not promoted: the next evaluation carries no
// `policy_rerun_owed` for the pair, which waits for its own trigger. Before the exhaustion the
// owed rerun was promoted.
//
// SR-2b-2 review M1: the same with a failure that carries a failure identity, so the exhausted
// run's ordinary failure arms R22's immediate rerun (which reruns that run once, unchanged).
// Later, while the pair owes a rerun in the window it was exhausted in, R22 does not rerun it at
// once either (no `failure_retry_immediate`), and the stage gate still holds it: at S3 it waits
// with `policy_rerun_waiting{pacing_stage: s3}` and is not dispatched. Both legs check this.
#[test]
fn sr2b2_t6_a_pair_exhausted_in_this_window_is_not_promoted() {
    for (leg, input, identified) in [
        ("no failure identity", sampling_ended(), false),
        ("a failure identity", identified_sampling_ended(), true),
    ] {
        exhausted_leg(leg, &input, identified);
    }
}

/// `sampling_ended` with a failure identity, as a linear or page-graph run's settlement writes
/// one: an ordinary failure of it schedules R22's immediate rerun.
fn identified_sampling_ended() -> PolicyExecutionInput {
    let PolicyExecutionInput::Failed { outcome, class, .. } = sampling_ended() else {
        panic!("sampling_ended is a failure")
    };
    let identity = FailureIdentity {
        base: SAMPLING_CODE.to_owned(),
        key: "0".repeat(12),
        main: IdentityPackage::measured("fixture.rerun.package", None),
        layers: Vec::new(),
        frame: IdentityFrame::None,
    }
    .encode();
    PolicyExecutionInput::Failed {
        outcome,
        failure_identity: Some(identity),
        class,
    }
}

/// Test ⑥'s scenario with the pair's runs failing as `input`.
fn exhausted_leg(leg: &str, input: &PolicyExecutionInput, identified: bool) {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&budget_policy_sources(1))
        .expect("activate the catalog");
    let step = 10 * MINUTE_MS;
    let at = |index: u64| POLICY_NOW_UNIX_MS + index * step;
    let (codes, failure) = run_at(&host, &clock, at(0), 600, true, input);
    assert!(!codes.iter().any(|code| code == OWED), "{leg}: {codes:?}");
    assert_eq!(
        failure.condition.map(|condition| condition.count),
        Some(1),
        "{leg}"
    );
    let (codes, failure) = run_at(&host, &clock, at(1), 601, false, input);
    assert!(
        codes.iter().any(|code| code == OWED),
        "{leg}: promoted: {codes:?}"
    );
    assert_eq!(
        failure.condition.map(|condition| condition.count),
        Some(2),
        "{leg}"
    );
    let (codes, failure) = run_at(&host, &clock, at(2), 602, false, input);
    assert!(
        codes.iter().any(|code| code == OWED),
        "{leg}: promoted: {codes:?}"
    );
    assert_eq!(failure.condition, None, "{leg}: the third is exhausted");
    if identified {
        assert!(failure.failure_identity.is_some(), "{leg}: {failure:?}");
        assert_eq!(
            failure.disposition,
            PolicyFailureDisposition::RetryScheduled,
            "{leg}: the exhausted run's ordinary failure arms R22"
        );
    }
    let (codes, failure) = run_at(&host, &clock, at(3), 603, false, input);
    assert!(!codes.iter().any(|code| code == OWED), "{leg}: {codes:?}");
    if identified {
        assert!(
            codes.iter().any(|code| code == IMMEDIATE),
            "{leg}: R22 reruns the exhausted run itself: {codes:?}"
        );
    }
    assert_eq!(
        failure.condition.map(|condition| condition.count),
        Some(1),
        "{leg}: a new series in the same window owes a rerun"
    );

    host.pin_pacing_stage_for_test(POLICY_INSTANCE_ALIAS, PerformancePacingStage::S3)
        .expect("pin S3");
    let evaluation = evaluate_at(&host, &clock, at(4), 604);
    assert!(
        !dispatched(&evaluation, TASK),
        "{leg}: held by the stage gate: {evaluation:#?}"
    );
    let pair = decision(&evaluation, TASK);
    assert!(
        reason(pair, OWED).is_none(),
        "{leg}: not promoted after its exhaustion in this window: {pair:#?}"
    );
    assert!(
        reason(pair, IMMEDIATE).is_none(),
        "{leg}: not rerun at once after its exhaustion in this window: {pair:#?}"
    );
    let waiting =
        reason(pair, WAITING).unwrap_or_else(|| panic!("{leg}: waiting at S3: {pair:#?}"));
    assert_eq!(waiting.value("pacing_stage"), Some(&json!("s3")), "{leg}");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}
