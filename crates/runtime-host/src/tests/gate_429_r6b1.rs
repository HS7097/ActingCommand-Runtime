// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-6b1 (register §1.314 R6B-6 and R6B-8, question 3, §1.315;
// `contracts/policy-suspension.md`, "A stopped run"): the readers of a stopped run's record.
//
// - The projections (`failures_through`, `fold_settlements_by`, the task snapshot's terminal and
//   the dispatch journal, read by the functions the host reads them with): a stopped run shows
//   `cancelled` in its pair's snapshot; `last_duration_ms`, `failure_streak` and the four
//   settlement facts stay those of the pair's run before; a later run's failure count counts on
//   from that run; and the journal keeps that run, and the value record names it, while a stopped
//   run is the pair's latest.
// - The remedy journal: a stopped run ends the Runtime's own rerun its dispatch served as a failed
//   run does (a retry closes its round, a provider's request is used).
// - `actingd suspended`: no stopped run is listed as a failure or a pause.
//
// Nothing writes the kind yet, so each record is built with the contract's own types; the report's
// records are appended to a closed state root by an offline writer, as RL-1c fix 2's gate does.

use super::gate_rl1c_fix2::{LATEST_START_UNIX_MS, PAUSED_TASK, fixture, report_within};
use super::*;
use crate::SuspendedReadBudget;
use crate::policy_host::{
    PAIR_PROJECTION_ADMITTED_AT_UNIX_MS, PairProjectionForTest, pair_projection_for_test,
    remedy_settlement,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    AuditInput, EventDraft, EventLinksDraft, EventOrigin, EventPayloadDraft, PerformanceContext,
    PolicyDispatchEventData, PolicyExecutionEventData, PolicyFailureRecord, PolicyPayloadDraft,
    PolicyReasonRecord, TaskPayloadDraft,
};
use actingcommand_ledger::{LedgerMaintenance, Sha256SecretFingerprinter};
use actingcommand_policy::{REMEDY_JOURNAL_MAX_RUNS, RemedySettlement, TaskTerminalState};
use actingcommand_runtime_database::MaintenanceLimits;
use serde_json::{Value, json};

/// The State seed of `config` and the offline writer's owner id: neutral fixture words.
const STATE_KEY: &[u8] = b"runtime-host-test-salt";
const WRITER: &str = "r6b1-offline-writer";
/// A pair whose only run was stopped: a neutral id outside the catalog.
const STOPPED_TASK: &str = "fixture.r6b1.stopped";

fn stopped() -> PolicyExecutionOutcome {
    PolicyExecutionOutcome::Stopped {
        outcome: Outcome::new(RuntimeErrorCode::ContainedTaskCancelled),
        runtime_ms: 4_000,
    }
}

fn failed() -> PolicyExecutionOutcome {
    PolicyExecutionOutcome::Failed {
        failure: PolicyFailureRecord {
            outcome: Outcome::new(RuntimeErrorCode::CaptureFailed),
            failure_identity: None,
            reported_success: false,
            original_class: PolicyFailureClass::Recoverable,
            effective_class: PolicyFailureClass::Recoverable,
            consecutive_same_error: 1,
            escalation_streak: 1,
            performance_tax_exempt: false,
            retry_attempt: 0,
            disposition: PolicyFailureDisposition::Continue,
            retry_at_unix_ms: None,
            runtime_ms: 9_000,
            sensitive: false,
            perf_context: Box::new(PerformanceContext::unavailable(
                PAIR_PROJECTION_ADMITTED_AT_UNIX_MS,
            )),
            condition: None,
        },
    }
}

fn succeeded() -> PolicyExecutionOutcome {
    PolicyExecutionOutcome::Succeeded { runtime_ms: 5_000 }
}

/// Run `index`'s settlement time in [`pair_projection_for_test`].
fn settled_at(index: u64) -> u64 {
    PAIR_PROJECTION_ADMITTED_AT_UNIX_MS + index * 60_000 + 10_000
}

// A success, a failure, then a stopped run: the stopped run counts no failure (it keeps the
// failure's 1); the pair's snapshot shows `cancelled`, and its settlement (the four settlement
// facts, `last_duration_ms` and `failure_streak`) is the failure's: 10,000 ms from its admission
// to its record, one failure in a row. A failure after the stop counts on from that failure.
#[test]
fn r6b1_p1_a_stopped_run_shows_cancelled_and_keeps_the_settlement_of_the_run_before() {
    let projection = pair_projection_for_test(&[succeeded(), failed(), stopped()]);
    assert_eq!(projection.failures, vec![0, 1, 1]);
    assert_eq!(projection.terminal, Some(TaskTerminalState::Cancelled));
    let settlement = projection.settlement.expect("the pair's settlement");
    assert_eq!(settlement.catalog_task_id, "task-synthetic");
    assert_eq!(settlement.instance_alias, "instance-synthetic");
    assert!(!settlement.succeeded, "the run before failed");
    assert_eq!(settlement.duration_ms, 10_000);
    assert_eq!(settlement.failure_streak, 1);
    assert_eq!(settlement.completed_at_unix_ms, settled_at(1));
    assert_eq!(
        projection.named,
        vec![1, 2],
        "the value record names the stopped latest run and the run before it"
    );
    assert_eq!(projection.kept, vec![0, 1, 2]);

    let later = pair_projection_for_test(&[succeeded(), failed(), stopped(), failed()]);
    assert_eq!(
        later.failures,
        vec![0, 1, 1, 2],
        "a later failure counts on from the run before the stop"
    );
    assert_eq!(later.terminal, Some(TaskTerminalState::Failed));
    let settlement = later.settlement.expect("the pair's settlement");
    assert_eq!(settlement.failure_streak, 2);
    assert_eq!(settlement.completed_at_unix_ms, settled_at(3));
    assert_eq!(
        later.named,
        vec![3],
        "a pair whose latest run settled names it alone"
    );

    let after_success = pair_projection_for_test(&[succeeded(), stopped()]);
    assert_eq!(after_success.failures, vec![0, 0]);
    assert_eq!(after_success.terminal, Some(TaskTerminalState::Cancelled));
    let settlement = after_success.settlement.expect("the pair's settlement");
    assert!(settlement.succeeded);
    assert_eq!(settlement.duration_ms, 5_000, "the success's runtime");
    assert_eq!(settlement.failure_streak, 0);
    assert_eq!(settlement.completed_at_unix_ms, settled_at(0));
}

// A pair whose only run was stopped has no settlement: no settlement fact and no last duration
// or failure streak, while its snapshot shows `cancelled`.
#[test]
fn r6b1_p2_a_pair_whose_only_run_was_stopped_has_no_settlement() {
    let PairProjectionForTest {
        failures,
        settlement,
        terminal,
        named,
        kept,
    } = pair_projection_for_test(&[stopped()]);
    assert_eq!(failures, vec![0]);
    assert_eq!(settlement, None);
    assert_eq!(terminal, Some(TaskTerminalState::Cancelled));
    assert_eq!(named, vec![0]);
    assert_eq!(kept, vec![0]);
}

// A success, then 70 stopped runs: the journal keeps the latest 64 admitted runs of a pair, and
// besides them the success, which the pair's settlement still reads, so its settlement is the
// success's however many stops follow; the value record names the latest stop and the success.
// The same 71 runs all successes keep no run outside the bound.
#[test]
fn r6b1_p3_the_journal_keeps_the_settled_run_behind_many_stops() {
    let later = u64::try_from(REMEDY_JOURNAL_MAX_RUNS).expect("the journal's bound") + 6;
    let mut outcomes = vec![succeeded()];
    outcomes.extend((0..later).map(|_| stopped()));
    let projection = pair_projection_for_test(&outcomes);
    let last = usize::try_from(later).expect("an index");
    assert!(projection.kept.contains(&0), "the success is kept");
    assert!(!projection.kept.contains(&1), "an old stop is pruned");
    assert!(projection.kept.contains(&last));
    assert_eq!(projection.named, vec![0, last]);
    let settlement = projection.settlement.expect("the pair's settlement");
    assert!(settlement.succeeded);
    assert_eq!(settlement.completed_at_unix_ms, settled_at(0));
    assert_eq!(projection.terminal, Some(TaskTerminalState::Cancelled));
    assert!(projection.failures.iter().all(|count| *count == 0));

    let successes = (0..=last).map(|_| succeeded()).collect::<Vec<_>>();
    let all_successes = pair_projection_for_test(&successes);
    assert!(
        !all_successes.kept.contains(&0),
        "without a stop the bound prunes as before"
    );
    assert_eq!(all_successes.named, vec![last]);
}

// Question 3: a stopped run of the remedy journal settles as a failed one does, so the planner
// closes a retry's round and counts a provider's request as used; a success still reads its key.
#[test]
fn r6b1_p4_a_stopped_remedy_run_ends_its_round_as_a_failed_run_does() {
    assert_eq!(
        remedy_settlement(Some(&stopped()), || panic!("a stopped run names no key")),
        RemedySettlement::Failed
    );
    assert_eq!(
        remedy_settlement(Some(&failed()), || panic!("a failed run names no key")),
        RemedySettlement::Failed
    );
    assert_eq!(
        remedy_settlement(Some(&succeeded()), || Some("fixture-key".to_owned())),
        RemedySettlement::Succeeded {
            outcome_key: Some("fixture-key".to_owned())
        }
    );
    assert_eq!(
        remedy_settlement(None, || panic!("a run in flight names no key")),
        RemedySettlement::InFlight
    );
}

/// An offline writer of a closed state root: one stopped scheduled run of `task_id` (its
/// dispatch intent, its `task.failed` terminal with `contained_task_cancelled` and its `stopped`
/// record) at `at_unix_ms`.
fn append_stopped_run(root: &TempDir, at_unix_ms: u64, task_id: &str, decision_id: &str) {
    let database = Arc::new(
        RuntimeStateStore::open_database(root.path(), STATE_KEY).expect("the state database"),
    );
    let limits = MaintenanceLimits::default();
    let ledger = LedgerMaintenance::acquire(
        root.path(),
        false,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("the ledger's maintenance owner")
    .open_writer(database, WRITER.to_owned())
    .expect("the offline writer");
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let fingerprinter = Sha256SecretFingerprinter::new(WRITER.as_bytes()).expect("fingerprinter");
    let append =
        |at: u64, origin: EventOrigin, links: EventLinksDraft, payload: EventPayloadDraft| {
            let draft = EventDraft::new(
                issuer.mint_event_id().expect("event id"),
                at,
                EventSeverity::Info,
                origin,
                links,
                payload,
            )
            .sanitize(&fingerprinter)
            .expect("a fixture record");
            ledger.append(draft).expect("append a fixture record");
        };
    let links = EventLinksDraft::default()
        .with_instance_id(issuer.mint_instance_id().expect("instance id"))
        .with_request_id(issuer.mint_request_id().expect("request id"))
        .with_correlation_id(issuer.mint_correlation_id().expect("correlation id"))
        .with_task_id(issuer.mint_task_id().expect("task id"))
        .with_run_id(issuer.mint_run_id().expect("run id"));
    let policy = EventOrigin::new(
        EventSource::Scheduler,
        OriginModule::Policy,
        EventActor::Scheduler,
    );
    let placeholder = |digit: char| format!("sha256:{}", digit.to_string().repeat(64));
    append(
        at_unix_ms,
        policy.clone(),
        links
            .clone()
            .with_action_id(issuer.mint_action_id().expect("action id")),
        PolicyPayloadDraft::dispatch_intent(
            PolicyDispatchEventData {
                decision_id: decision_id.to_owned(),
                task_id: task_id.to_owned(),
                instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
                operation_id: "operation.r6b1".to_owned(),
                procedure_binding_digest: placeholder('b'),
                reason_chain_id: "reasons.r6b1".to_owned(),
                reasons: vec![PolicyReasonRecord(Outcome::relayed("placement_selected"))],
                catalog_hash: placeholder('c'),
                catalog_version: 1,
                input_ledger_position: 1,
                fact_snapshot_id: "facts.r6b1".to_owned(),
                approval_fact_ids: Vec::new(),
                urgency_milli: 0,
            },
            AuditInput::new(),
        )
        .into(),
    );
    append(
        at_unix_ms + 10,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        links.clone(),
        TaskPayloadDraft::semantic(
            TaskSemanticFact::TerminalCommitted {
                task_result: TaskOutcome::Failure,
                final_page: None,
                executed_steps: Some(0),
                outcome: Some(Outcome::new(RuntimeErrorCode::ContainedTaskCancelled)),
                failure_severe: false,
                scheduling_disposition: None,
                task_timing: None,
            },
            AuditInput::new(),
        )
        .into(),
    );
    append(
        at_unix_ms + 100,
        policy,
        links,
        PolicyPayloadDraft::execution_recorded(
            PolicyExecutionEventData {
                decision_id: decision_id.to_owned(),
                task_id: task_id.to_owned(),
                instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
                observed_at_unix_ms: at_unix_ms + 100,
                outcome: stopped(),
            },
            AuditInput::new(),
        )
        .into(),
    );
    ledger.close().expect("close the offline writer");
}

/// The task ids a list of the report names.
fn listed(report: &Value, list: &str) -> Vec<String> {
    report[list]
        .as_array()
        .unwrap_or_else(|| panic!("{list} is a list: {report:#}"))
        .iter()
        .map(|entry| entry["task_id"].as_str().expect("a task id").to_owned())
        .collect()
}

// `actingd suspended` on RL-1c fix 2's closed root (one paused run of `PAUSED_TASK`), extended
// with a stopped run of a pair that has no other run, and a stopped run of the paused pair after
// its pause. Neither stopped run is listed as a failure or a pause: the pair with only a stopped
// run is in no list, and the paused pair is listed by its paused run, its latest failure, as the
// decision memory keeps it.
#[test]
fn r6b1_p5_actingd_suspended_lists_no_stopped_run_as_a_failure_or_a_pause() {
    let root = fixture(true);
    append_stopped_run(
        &root,
        LATEST_START_UNIX_MS + 20_000,
        STOPPED_TASK,
        "decision.r6b1.stopped-alone",
    );
    append_stopped_run(
        &root,
        LATEST_START_UNIX_MS + 30_000,
        PAUSED_TASK,
        "decision.r6b1.stopped-after-pause",
    );
    let report = report_within(&root, SuspendedReadBudget::until(None));
    assert_eq!(
        listed(&report, "suspended"),
        vec![PAUSED_TASK.to_owned()],
        "{report:#}"
    );
    assert_eq!(
        report["suspended"][0]["paused"]["decision_id"],
        json!("decision.rl1c-fix2.paused"),
        "the paused pair is listed by its paused run, not by its stopped one: {report:#}"
    );
    for list in ["suspended", "lifted", "repeating"] {
        assert!(
            !listed(&report, list)
                .iter()
                .any(|task| task == STOPPED_TASK),
            "{list}: {report:#}"
        );
    }
    assert!(listed(&report, "lifted").is_empty(), "{report:#}");
    assert!(listed(&report, "repeating").is_empty(), "{report:#}");
}
