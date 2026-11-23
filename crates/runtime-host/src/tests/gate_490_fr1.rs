// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #490 FR-1 (0.12.81; register §1.517; contracts/policy-suspension.md, "Lifting (R19)" and
// "`actingd suspended`"; contracts/scheduling/README.md, "Fact refresh", "Its own gates"): a fact
// refresh is never held by a pause. On gate_384_d2s4's chain fixture (the instance binds the
// recognition maintenance package and the catalog's task reads a key the instance never had, so
// every evaluation requests the reading), a manual Runtime clock and a fake device, the fact
// refresh's dispatch is admitted and settled as a severe failure (`artifact_store_failure`, the
// class a fatal failure without a terminal state gets), which the settlement records as it records
// every severe failure: paused at its first occurrence. The record is unchanged by this slice, so
// a root written so is a ledger of before this version.
//
// - h3 (test 3): the failure counts once in the row the scheduler's gate reads (the snapshot is
//   `failed` with a failure streak of 1); within its failure backoff the reading waits under its
//   own gate (`task_cooldown_active`, the waiting task naming the failure's code and the backoff's
//   end), a millisecond before the end still, and at the end it is dispatched and admitted: no
//   decision names `policy_task_paused` or `failure_pause`.
// - h4 (test 4): the root's second start (it seeds: the paused settlement is replayed as before,
//   no fatal) and its third (it loads the pair from its value record) each select the reading at
//   their first evaluation, the third admits it; neither the decisions nor the not-run explanation
//   name the pause, and `actingd suspended` lists the pair neither as suspended nor as lifted and
//   warns nothing of it.
//
// No clock is read in this file.

use super::gate_384_d2s4::{BUSINESS_TASK, chain_sources, intent_of, recognition_package};
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::StartReportTaggedState;
use actingcommand_contract::outcome::Outcome;
use actingcommand_policy::{FACT_REFRESH_TASK_ID, PolicyEvaluation, TaskTerminalState};
use serde_json::json;

/// The reading's runtime before its severe failure.
const RUN_MS: u64 = 75_000;
/// `scheduling.fact_refresh.failure_backoff_ms`, shipped.
const BACKOFF_MS: u64 = 600_000;
const HOUR_MS: u64 = 3_600_000;
const PAUSED: &str = "policy_task_paused";
const COOLDOWN: &str = "task_cooldown_active";
const PENDING: &str = "fact_refresh_pending";

// ---- The fixture.

/// A host on `root` for the registered instance `registered`, its Runtime clock a manual one at
/// `at`, the instance binding the recognition maintenance package the root holds (written at the
/// root's first start).
fn start(
    root: &TempDir,
    registered: InstanceId,
    at: u64,
) -> (RuntimeHost, Arc<ManualRuntimeClock>) {
    let package = root.path().join("recognition.zip");
    if !package.exists() {
        fs::write(&package, recognition_package(false)).expect("write the recognition package");
    }
    let clock = Arc::new(ManualRuntimeClock::new(at, 0));
    let host = RuntimeHost::start(
        config(root)
            .with_runtime_clock(clock.clone())
            .with_recognition_packages(BTreeMap::from([(
                POLICY_INSTANCE_ALIAS.to_owned(),
                package,
            )]))
            .with_config_manifest(day_boundary_off_manifest(&registered)),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    (host, clock)
}

/// One full policy cycle at `at`, the Runtime clock moved there first.
fn evaluate_at(
    host: &RuntimeHost,
    clock: &ManualRuntimeClock,
    at: u64,
    seed: u64,
) -> PolicyEvaluation {
    advance_manual_clock_to(clock, at);
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
    .expect("policy cycle")
    .evaluation
    .expect("a full evaluation")
}

/// Admits the reading `evaluation` selected (the catalog approved first when `approve`); its
/// intent. The run's context is let go at once, as `gate_386_sr2b1::admit` lets it go.
fn admit_reading(
    host: &RuntimeHost,
    evaluation: &PolicyEvaluation,
    approve: bool,
) -> DispatchIntent {
    let (refresh, chain) = intent_of(evaluation, FACT_REFRESH_TASK_ID);
    if approve {
        record_policy_approval(host, &refresh);
    }
    let admission = host
        .admit_policy_dispatch(&refresh, &chain, &policy_context(host, &refresh))
        .expect("the reading's admission");
    assert!(
        matches!(admission, PolicyDispatchAdmission::Granted { .. }),
        "the reading is admitted"
    );
    refresh
}

/// What a reading settles with when the artifact store fails under it: `artifact_store_failure`,
/// raised only as fatal, its class severe.
fn severe() -> PolicyExecutionInput {
    PolicyExecutionInput::Failed {
        outcome: Outcome::new(HostCode::ArtifactStoreFailure),
        failure_identity: None,
        class: PolicyFailureClass::Severe,
    }
}

/// The reading selected and admitted at `at` on the root's first start, settled `RUN_MS` later as
/// a severe failure, which pauses its pair as every severe failure does; the settlement's instant.
fn fail_severely(host: &RuntimeHost, clock: &ManualRuntimeClock, at: u64) -> u64 {
    let evaluation = evaluate_at(host, clock, at, 31);
    let refresh = admit_reading(host, &evaluation, true);
    clock.advance(RUN_MS);
    let settled = host
        .record_policy_dispatch_outcome(&refresh.decision_id, &severe())
        .expect("the settlement");
    let PolicyExecutionOutcome::Failed { failure } = &settled.outcome else {
        panic!("a failure: {settled:?}");
    };
    assert_eq!(
        failure.disposition,
        PolicyFailureDisposition::PausedTask,
        "the settlement records a pause, as before: {failure:?}"
    );
    assert_eq!(failure.original_class, PolicyFailureClass::Severe);
    assert_eq!(failure.consecutive_same_error, 1);
    assert!(host.fatal_error().expect("runtime health").is_none());
    settled.observed_at_unix_ms
}

/// The reading's task snapshot as the evaluation projects it at `at`.
fn reading_snapshot(host: &RuntimeHost, at: u64) -> actingcommand_policy::TaskRuntimeSnapshot {
    host.policy_task_snapshots_for_test(at)
        .expect("the task snapshots")
        .into_iter()
        .find(|snapshot| {
            snapshot.task_id == FACT_REFRESH_TASK_ID
                && snapshot.instance_id == POLICY_INSTANCE_ALIAS
        })
        .expect("the reading's snapshot")
}

/// The codes of the reasons of `task_id`'s decision on the instance.
fn codes_of(evaluation: &PolicyEvaluation, task_id: &str) -> Vec<String> {
    evaluation
        .decisions
        .iter()
        .filter(|decision| {
            decision.task_id == task_id
                && decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS)
        })
        .flat_map(|decision| decision.reasons.iter())
        .map(|reason| reason.outcome.code().as_str().to_owned())
        .collect()
}

/// No decision on the instance names the pause: no `policy_task_paused` and no `failure_pause`.
fn assert_not_paused(evaluation: &PolicyEvaluation, case: &str) {
    for decision in evaluation
        .decisions
        .iter()
        .filter(|decision| decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS))
    {
        for reason in &decision.reasons {
            assert!(
                !reason.outcome.is(HostCode::PolicyTaskPaused),
                "{case}: {}: {:?}",
                decision.task_id,
                decision.reasons
            );
            assert_ne!(
                reason.outcome.value("refresh_limit"),
                Some(&json!("failure_pause")),
                "{case}: {}",
                decision.task_id
            );
        }
    }
}

/// The instance's not-run explanation after the latest evaluation (`ScheduleExplanation` with its
/// candidate rows): the reading is the task selected, and neither the not-running row nor any
/// candidate row names the pause.
fn assert_explained_as_selected(host: &RuntimeHost, case: &str) {
    let mut client = TestClient::connect(host);
    let request = client.request(RuntimeOperation::ScheduleExplanation {
        instance: Some(POLICY_INSTANCE_ALIAS.to_owned()),
        candidates: true,
    });
    let receipt = client.send(&request);
    let Some(RuntimeResult::ScheduleExplanation { explanation }) = receipt.result() else {
        panic!("{case}: a schedule explanation: {receipt:?}");
    };
    let entry = explanation
        .instances
        .first()
        .unwrap_or_else(|| panic!("{case}: the instance's entry: {explanation:?}"));
    assert_eq!(
        entry
            .selected
            .as_ref()
            .map(|selected| selected.task_id.as_str()),
        Some(FACT_REFRESH_TASK_ID),
        "{case}: {entry:?}"
    );
    assert!(
        !entry.not_running.why.is(HostCode::PolicyTaskPaused),
        "{case}: {entry:?}"
    );
    for row in &entry.candidates {
        assert!(
            row.reasons.iter().all(|listed| listed.as_str() != PAUSED),
            "{case}: {row:?}"
        );
        assert_ne!(
            row.why.value("refresh_limit"),
            Some(&json!("failure_pause")),
            "{case}: {row:?}"
        );
    }
}

/// How this start of the host restored the decision memory, as its start report says (Workflow
/// #422 LN-4): `Load` from the value records, or `Seed` (no marker: the history replayed).
fn tagged_start(host: &RuntimeHost) -> Option<StartReportTaggedState> {
    let mut client = TestClient::connect(host);
    let request = client.request(RuntimeOperation::StartReport);
    let receipt = client.send(&request);
    let Some(RuntimeResult::StartReport { report }) = receipt.result() else {
        panic!("expected the start report: {receipt:?}");
    };
    report.tagged_state
}

// ---- The tests.

// Test 3: a severe failure of the reading does not hold it. It counts as one failure in the row the
// gate reads; within its failure backoff the reading waits under its own gate, and at the
// backoff's end it is dispatched and admitted, with no pause named anywhere.
#[test]
fn fr1_h3_a_severe_failure_of_the_reading_counts_once_and_it_reads_again_after_its_backoff() {
    let root = TempDir::new().expect("tempdir");
    let registered = instance_id();
    let (host, clock) = start(&root, registered, POLICY_NOW_UNIX_MS);
    host.activate_policy_catalog(&chain_sources())
        .expect("activate the chain catalog");
    let settled_at = fail_severely(&host, &clock, POLICY_NOW_UNIX_MS);

    // Ruling 3's check: the failure that paused the pair is a failed run with its count.
    let snapshot = reading_snapshot(&host, settled_at);
    assert_eq!(
        snapshot.terminal_state,
        Some(TaskTerminalState::Failed),
        "{snapshot:?}"
    );
    assert_eq!(snapshot.failure_streak, Some(1), "{snapshot:?}");

    // Within the backoff: the reading's own gate holds it.
    let backoff_end = settled_at + BACKOFF_MS;
    let evaluation = evaluate_at(&host, &clock, settled_at + 1_000, 32);
    assert!(
        evaluation
            .dispatch_intents
            .iter()
            .all(|intent| intent.task_id != FACT_REFRESH_TASK_ID),
        "{evaluation:#?}"
    );
    let reading = codes_of(&evaluation, FACT_REFRESH_TASK_ID);
    assert_eq!(
        reading.last().map(String::as_str),
        Some(COOLDOWN),
        "{reading:?}"
    );
    let pending = evaluation
        .decisions
        .iter()
        .filter(|decision| decision.task_id == BUSINESS_TASK)
        .flat_map(|decision| decision.reasons.iter())
        .find(|reason| reason.outcome.code().as_str() == PENDING)
        .unwrap_or_else(|| panic!("the task waits for the reading: {evaluation:#?}"));
    assert_eq!(pending.outcome.value("gate_code"), Some(&json!(COOLDOWN)));
    assert_eq!(
        pending.outcome.value("failure_code"),
        Some(&json!("artifact_store_failure"))
    );
    assert_eq!(
        pending.outcome.value("next_eligible_unix_ms"),
        Some(&json!(backoff_end))
    );
    assert_not_paused(&evaluation, "within the backoff");

    let evaluation = evaluate_at(&host, &clock, backoff_end - 1, 33);
    assert!(
        evaluation
            .dispatch_intents
            .iter()
            .all(|intent| intent.task_id != FACT_REFRESH_TASK_ID),
        "a millisecond before the backoff's end: {evaluation:#?}"
    );

    // At the backoff's end the reading is dispatched and admitted.
    let evaluation = evaluate_at(&host, &clock, backoff_end, 34);
    assert_not_paused(&evaluation, "at the backoff's end");
    let refresh = admit_reading(&host, &evaluation, false);
    assert_eq!(refresh.task_id, FACT_REFRESH_TASK_ID);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// Test 4: a ledger that already holds a paused pair of the reading (its severe failure recorded on
// the root's first start, as before this version). The root's second start seeds and replays it
// as before; its first evaluation selects the reading, and neither its decisions nor the not-run
// explanation name the pause. The third start loads the pair from its value record; its first
// evaluation selects the reading and its admission admits it. `actingd suspended` on the closed
// root lists the pair neither under `suspended` nor under `lifted` and warns nothing of it.
#[test]
fn fr1_h4_a_ledger_holding_a_paused_reading_admits_it_at_the_first_evaluation() {
    let root = TempDir::new().expect("tempdir");
    let registered = instance_id();
    let (host, clock) = start(&root, registered, POLICY_NOW_UNIX_MS);
    host.activate_policy_catalog(&chain_sources())
        .expect("activate the chain catalog");
    let settled_at = fail_severely(&host, &clock, POLICY_NOW_UNIX_MS);
    host.close().expect("close the first host");

    let seeded_at = settled_at + HOUR_MS;
    let (host, clock) = start(&root, registered, seeded_at);
    assert_eq!(
        tagged_start(&host),
        Some(StartReportTaggedState::Seed),
        "the root's second start seeds"
    );
    let evaluation = evaluate_at(&host, &clock, seeded_at, 41);
    let (selected, _) = intent_of(&evaluation, FACT_REFRESH_TASK_ID);
    assert_eq!(selected.instance_id, POLICY_INSTANCE_ALIAS);
    assert_not_paused(&evaluation, "the seeding start");
    assert_explained_as_selected(&host, "the seeding start");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the seeding host");

    let loaded_at = seeded_at + BACKOFF_MS;
    let (host, clock) = start(&root, registered, loaded_at);
    assert_eq!(
        tagged_start(&host),
        Some(StartReportTaggedState::Load),
        "the root's third start loads its decision memory from its value records"
    );
    let evaluation = evaluate_at(&host, &clock, loaded_at, 42);
    assert_not_paused(&evaluation, "the loading start");
    assert_explained_as_selected(&host, "the loading start");
    let refresh = admit_reading(&host, &evaluation, false);
    assert_eq!(refresh.task_id, FACT_REFRESH_TASK_ID);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the loading host");

    let report = crate::suspension_report(&SuspensionReportRequest {
        config_path: Path::new("actingd.config.json"),
        config_modified_unix_ms: None,
        host: &config(&root),
        catalog: &chain_sources(),
    })
    .unwrap_or_else(|error| panic!("actingd suspended: {error}"));
    for member in ["suspended", "lifted"] {
        let rows = report[member]
            .as_array()
            .unwrap_or_else(|| panic!("the {member} rows: {report:#}"));
        assert!(
            rows.iter()
                .all(|row| row["task_id"] != FACT_REFRESH_TASK_ID),
            "{member}: {report:#}"
        );
    }
    let pair = format!("{FACT_REFRESH_TASK_ID}/{POLICY_INSTANCE_ALIAS}");
    let warnings = report["warnings"].as_array().expect("the warnings");
    assert!(
        warnings
            .iter()
            .all(|warning| warning.as_str().is_none_or(|text| !text.ends_with(&pair))),
        "{report:#}"
    );
}
