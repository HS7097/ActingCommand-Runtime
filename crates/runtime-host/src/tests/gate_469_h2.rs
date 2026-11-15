// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #469 H2 (0.12.62; register §1.398; contracts/policy-suspension.md, "Why a pair is
// paused"; the design `rd-1115/status-codes-a-design.md`, place 4; coordinator work order
// `rd-1115/brief-469-word-slices.md`): a paused pair's `policy_task_paused` carries why its
// settlement paused it, `pause_cause`, and the failure's code, `failure_code`; a pair in its retry
// backoff carries the failure's code on `policy_retry_backoff_active`. The word comes from the
// execution record the settlement wrote, in the settlement's own order, the first that holds (a
// severe failure, then a sensitive task, then the streak, then `on_failure`; one fixture holds two
// causes at once), so a replay and a restart that loads the pair's streak from its value record
// give the same word, and `actingd suspended` shows it on the pair's `suspended` row. The word is
// a value of the outcome only: the failure identity the record keeps, its counts and the pair's
// streak in its value record are what the settlement wrote without it, and the reason's detail
// names neither the word nor the code.
//
// Each fixture runs the budget fixture's one task `fixture.observe` on `fixture-instance-a`: a run
// is a fresh start of the host on the root at its time (the root's first start activates and
// approves the catalog), the pair's dispatch evaluated and admitted, the Runtime clock moved on by
// the run's runtime, then settled with the run's input; the runs are 10 minutes apart. The
// evaluations that read the pair's reason are the pair's due evaluations: the live one 1 s after
// the last settlement on the same host, then one on each of two more starts of the root, 10
// minutes apart.
//
// Which starts these are (Workflow #422 LN-4, `contracts/ledger-observer.md` §11.6): a root's
// first start writes no seeding marker; its second finds none and seeds, replaying the whole
// history; every later start loads the decision memory from its value records, as every restart
// of an installation does, and only such a start rebuilds a paused streak without its word and
// gives the word back from the dispatch journal (`PolicyControlState::restore_pause_causes`). So
// the start after the runs is the root's seeding start for a one-run fixture (the word a replay
// gives) and a loading start for a two-run fixture; the start checked last is a loading start in
// every fixture. Each test asserts the kind of both from the host's start report (`StartReport`,
// `tagged_state`), so it fails if a change makes the checked start a seeding start again. The
// backoff's test does the same: its second start seeds, its third loads.

use super::gate_386_sr2b1::{admit, failure_of, start};
use super::*;
use crate::codes::{HostCode, PauseCause};
use crate::failure_identity::{FailureIdentity, IdentityFrame, IdentityPackage};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_contract::{
    PolicyControlStateRecord, PolicyFailureRecord, PolicyFailureStreakRecord,
    StartReportTaggedState, scheduled_failure_code_str,
};
use serde_json::{Value, json};

const TASK: &str = "fixture.observe";
const BETWEEN_RUNS_MS: u64 = 600_000;
// A run's runtime, within the budget fixture's 300 s runtime budget.
const RUN_MS: u64 = 75_000;
// A run's runtime above that budget.
const OVER_BUDGET_MS: u64 = 400_000;
// The budget fixture's backoff of a first retry (`on_failure.retry_backoff_ms`).
const BACKOFF_MS: u64 = 30_000;

// ---- The fixture.

/// The budget fixture with its one task changed by `mutate`.
fn sources_with(mutate: impl FnOnce(&mut Value)) -> CatalogSources {
    let mut sources = budget_policy_sources(1);
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("the task fixture");
    mutate(&mut tasks["tasks"][0]);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("the task bytes");
    sources
}

/// A failure identity whose base is `code` (its main package not measured, no frame): what the
/// record must keep byte for byte.
fn identity(code: Code) -> String {
    FailureIdentity {
        base: code.as_str().to_owned(),
        key: "0".repeat(12),
        main: IdentityPackage {
            id_hash: "0a1b2c3d".to_owned(),
            hash: "x".to_owned(),
        },
        layers: Vec::new(),
        frame: IdentityFrame::None,
    }
    .encode()
}

/// A failed run with `code`, `identity` and `class`.
fn failed(code: Code, identity: Option<String>, class: PolicyFailureClass) -> PolicyExecutionInput {
    PolicyExecutionInput::Failed {
        outcome: Outcome::new(code),
        failure_identity: identity,
        class,
    }
}

/// The capture failure every fixture but two fails with.
fn capture_failed() -> Code {
    RuntimeErrorCode::CaptureFailed.code()
}

/// The pair's reason with `code` at an evaluation at `at` (the Runtime clock moved there first):
/// its outcome and its detail. The pair is not dispatched.
fn pair_reason(
    host: &RuntimeHost,
    clock: &ManualRuntimeClock,
    at: u64,
    seed: u64,
    code: HostCode,
) -> (Outcome, String) {
    advance_manual_clock_to(clock, at);
    let evaluation = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: at,
                monotonic_ms: at,
            },
            seed,
            PolicyTrigger::FactsChanged,
        )
        .expect("evaluate the policy")
        .evaluation
        .expect("an evaluation");
    assert!(
        !evaluation
            .dispatch_intents
            .iter()
            .any(|intent| intent.task_id == TASK),
        "the pair is not dispatched: {evaluation:#?}"
    );
    evaluation
        .decisions
        .iter()
        .filter(|decision| {
            decision.task_id == TASK
                && decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS)
        })
        .flat_map(|decision| decision.reasons.iter())
        .find(|reason| reason.outcome.is(code))
        .map(|reason| (reason.outcome.clone(), reason.detail.clone()))
        .unwrap_or_else(|| panic!("the pair's {}: {evaluation:#?}", code.as_str()))
}

/// The pair's latest L7 value record in the ledger.
fn latest_value_record(host: &RuntimeHost) -> PolicyControlStateRecord {
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

/// How this start of the host restored the decision memory, as its start report says (Workflow
/// #422 LN-4): `Load` from the value records, or `Seed` (no marker: today's recoveries replayed).
fn tagged_start(host: &RuntimeHost) -> Option<StartReportTaggedState> {
    let mut client = TestClient::connect(host);
    let request = client.request(RuntimeOperation::StartReport);
    let receipt = client.send(&request);
    let Some(RuntimeResult::StartReport { report }) = receipt.result() else {
        panic!("expected the start report: {receipt:?}");
    };
    report.tagged_state
}

/// The pair's pause at an evaluation at `at`: it carries `word` as `pause_cause` and `code` as
/// `failure_code`, and its detail names neither.
fn assert_pause(
    host: &RuntimeHost,
    clock: &ManualRuntimeClock,
    at: u64,
    seed: u64,
    word: &str,
    code: &str,
    case: &str,
) {
    let (outcome, detail) = pair_reason(host, clock, at, seed, HostCode::PolicyTaskPaused);
    assert_eq!(
        outcome.value("pause_cause"),
        Some(&json!(word)),
        "{word}: {case}"
    );
    assert_eq!(
        outcome.value("failure_code"),
        Some(&json!(code)),
        "{word}: {case}"
    );
    assert!(!detail.contains(word), "{word}: {case}: {detail}");
    assert!(!detail.contains(code), "{word}: {case}: {detail}");
}

/// The word's fixture: what the settlement recorded is what it records without the word (the
/// identity byte for byte, the counts, the pair's streak in its value record built from the
/// record's own members alone, serialized byte for byte), and the word is on the pause.
struct Fixture<'a> {
    word: PauseCause,
    sources: &'a CatalogSources,
    /// Each run's input and runtime; the last one pauses the pair.
    runs: &'a [(PolicyExecutionInput, u64)],
    /// The code the paused run's record names.
    failure_code: Code,
    /// The identity the paused run's record keeps.
    failure_identity: Option<String>,
    /// The paused run's `consecutive_same_error` and `escalation_streak`.
    counts: (u16, u16),
}

/// Runs `fixture` and checks the word live, on the start after the runs, on the loading start
/// after that and in `actingd suspended`, and that the identity and the streak are unchanged by
/// it. Returns the paused run's record.
fn check(fixture: &Fixture<'_>) -> PolicyFailureRecord {
    let word = fixture.word.as_str();
    let code = fixture.failure_code.as_str();
    let root = TempDir::new().expect("tempdir");
    let registered = instance_id();
    let mut at = POLICY_NOW_UNIX_MS;
    let mut paused: Option<PolicyFailureRecord> = None;
    for (index, ((input, runtime_ms), seed)) in fixture.runs.iter().zip(100_u64..).enumerate() {
        let first = index == 0;
        let (host, clock) = start(&root, registered, at, None);
        if first {
            host.activate_policy_catalog(fixture.sources)
                .expect("activate the catalog");
        }
        let (intent, _) = admit(&host, at, seed, first);
        clock.advance(*runtime_ms);
        let failure = failure_of(
            &host
                .record_policy_dispatch_outcome(&intent.decision_id, input)
                .expect("the settlement"),
        );
        assert!(
            host.fatal_error().expect("runtime health").is_none(),
            "{word}"
        );
        if index + 1 == fixture.runs.len() {
            // The settlement as it is without the word.
            assert_eq!(
                failure.disposition,
                PolicyFailureDisposition::PausedTask,
                "{word}"
            );
            assert_eq!(
                scheduled_failure_code_str(&failure.outcome).as_str(),
                code,
                "{word}"
            );
            assert_eq!(
                failure.failure_identity, fixture.failure_identity,
                "{word}: the identity byte for byte"
            );
            assert_eq!(
                (failure.consecutive_same_error, failure.escalation_streak),
                fixture.counts,
                "{word}: the counts"
            );
            let streak = latest_value_record(&host)
                .failure_streak
                .expect("the pair's streak");
            let unchanged = PolicyFailureStreakRecord {
                failure_code: scheduled_failure_code_str(&failure.outcome),
                failure_identity: failure.failure_identity.clone(),
                class: failure.original_class,
                count: failure.consecutive_same_error,
                escalation_streak: failure.escalation_streak,
                disposition: failure.disposition,
                retry_at_unix_ms: failure.retry_at_unix_ms,
                decision_id: intent.decision_id.clone(),
            };
            let recorded = serde_json::to_string(&streak).expect("the streak's JSON");
            assert_eq!(
                recorded,
                serde_json::to_string(&unchanged).expect("the unchanged streak's JSON"),
                "{word}: the value record's streak byte for byte"
            );
            assert!(!recorded.contains("pause_cause"), "{word}: {recorded}");

            // Live: the pause carries the word and the failure's code, not its detail.
            assert_pause(
                &host,
                &clock,
                at + runtime_ms + 1_000,
                200,
                word,
                code,
                "live",
            );
            paused = Some(failure);
        }
        host.close().expect("close the host");
        at += BETWEEN_RUNS_MS;
    }
    let paused = paused.expect("the paused run");

    // The root's next start. After one run it is the root's second start, which finds no marker
    // and seeds: it replays the history, the settlement among it, so the word is the one a replay
    // gives. After two runs (the second run's start seeded) it loads.
    let after_runs = if fixture.runs.len() == 1 {
        StartReportTaggedState::Seed
    } else {
        StartReportTaggedState::Load
    };
    let (host, clock) = start(&root, registered, at, None);
    assert_eq!(
        tagged_start(&host),
        Some(after_runs),
        "{word}: the start after the runs"
    );
    assert_pause(
        &host,
        &clock,
        at,
        300,
        word,
        code,
        "the start after the runs",
    );
    host.close().expect("close the host started after the runs");
    at += BETWEEN_RUNS_MS;

    // A restart as every restart of an installation is: it loads the pair's streak from its value
    // record, without the word, and gives the word back from the streak's execution record in the
    // dispatch journal: the same word.
    let (host, clock) = start(&root, registered, at, None);
    assert_eq!(
        tagged_start(&host),
        Some(StartReportTaggedState::Load),
        "{word}: the restart checked loads its decision memory from its value records"
    );
    assert_pause(
        &host,
        &clock,
        at,
        400,
        word,
        code,
        "after a loading restart",
    );
    host.close().expect("close the restarted host");

    // `actingd suspended` on the closed root: the pair's row carries the word.
    let report = crate::suspension_report(&SuspensionReportRequest {
        config_path: Path::new("actingd.config.json"),
        config_modified_unix_ms: None,
        host: &config(&root),
        catalog: fixture.sources,
    })
    .unwrap_or_else(|error| panic!("actingd suspended: {error}"));
    let rows = report["suspended"].as_array().expect("the suspended rows");
    let [row] = rows.as_slice() else {
        panic!("{word}: one suspended row: {report:#}")
    };
    assert_eq!(row["task_id"], TASK, "{word}");
    assert_eq!(row["pause_cause"], word, "{word}: {row:#}");
    assert_eq!(row["failure"]["code"], code, "{word}");
    assert_eq!(
        row["consecutive_same_error"],
        json!(paused.consecutive_same_error),
        "{word}"
    );
    paused
}

// ---- One test per word.

// A severe failure (the failure point judged it severe) pauses the pair at its first occurrence:
// `severe_failure`.
#[test]
fn h2_a_severe_failure_pauses_with_severe_failure() {
    let failure_identity = identity(capture_failed());
    let sources = budget_policy_sources(1);
    check(&Fixture {
        word: PauseCause::SevereFailure,
        sources: &sources,
        runs: &[(
            failed(
                capture_failed(),
                Some(failure_identity.clone()),
                PolicyFailureClass::Severe,
            ),
            RUN_MS,
        )],
        failure_code: capture_failed(),
        failure_identity: Some(failure_identity),
        counts: (1, 1),
    });
}

// A linear step on a screen its package never recorded (Workflow #463 U1, class severe) pauses the
// pair at its first occurrence: `unrecorded_screen`.
#[test]
fn h2_an_unrecorded_screen_pauses_with_unrecorded_screen() {
    let code: Code =
        actingcommand_execution_kernel::ExecutionKernelCode::ContainedTaskLinearUnrecordedScreen
            .code();
    let failure_identity = identity(code);
    let sources = budget_policy_sources(1);
    check(&Fixture {
        word: PauseCause::UnrecordedScreen,
        sources: &sources,
        runs: &[(
            failed(
                code,
                Some(failure_identity.clone()),
                PolicyFailureClass::Severe,
            ),
            RUN_MS,
        )],
        failure_code: code,
        failure_identity: Some(failure_identity),
        counts: (1, 1),
    });
}

// A recoverable failure of a run that took longer than its runtime budget is the policy's rewrite,
// `policy_runtime_budget_exceeded` (severe, no identity), which pauses the pair:
// `runtime_budget_exceeded`.
#[test]
fn h2_a_run_over_its_runtime_budget_pauses_with_runtime_budget_exceeded() {
    let sources = budget_policy_sources(1);
    check(&Fixture {
        word: PauseCause::RuntimeBudgetExceeded,
        sources: &sources,
        runs: &[(
            failed(
                capture_failed(),
                Some(identity(capture_failed())),
                PolicyFailureClass::Recoverable,
            ),
            OVER_BUDGET_MS,
        )],
        failure_code: actingcommand_contract::POLICY_RUNTIME_BUDGET_EXCEEDED,
        failure_identity: None,
        counts: (1, 1),
    });
}

// The first interrupted settlement of the pair does not hold it; the second in a row pauses it:
// `interrupted_again`.
#[test]
fn h2_a_second_interrupted_settlement_in_a_row_pauses_with_interrupted_again() {
    let code = SharedCode::PolicySettlementInterrupted.code();
    let interrupted = failed(code, None, PolicyFailureClass::Severe);
    let sources = budget_policy_sources(1);
    check(&Fixture {
        word: PauseCause::InterruptedAgain,
        sources: &sources,
        runs: &[(interrupted.clone(), RUN_MS), (interrupted, RUN_MS)],
        failure_code: code,
        failure_identity: None,
        counts: (2, 2),
    });
}

// A recoverable failure of a task marked sensitive pauses the pair at once: `sensitive_task`.
#[test]
fn h2_a_sensitive_task_s_failure_pauses_with_sensitive_task() {
    let failure_identity = identity(capture_failed());
    let sources = sources_with(|task| task["sensitive"] = json!(true));
    check(&Fixture {
        word: PauseCause::SensitiveTask,
        sources: &sources,
        runs: &[(
            failed(
                capture_failed(),
                Some(failure_identity.clone()),
                PolicyFailureClass::Recoverable,
            ),
            RUN_MS,
        )],
        failure_code: capture_failed(),
        failure_identity: Some(failure_identity),
        counts: (1, 1),
    });
}

// Two causes at once: a sensitive task's recoverable failure of a run that took longer than its
// runtime budget is the policy's rewrite (severe), so the record is both severe and sensitive; the
// settlement's order (a severe source before a sensitive task, the first that holds) gives
// `runtime_budget_exceeded`, not `sensitive_task`.
#[test]
fn h2_a_sensitive_task_over_its_runtime_budget_pauses_with_runtime_budget_exceeded() {
    let sources = sources_with(|task| task["sensitive"] = json!(true));
    let paused = check(&Fixture {
        word: PauseCause::RuntimeBudgetExceeded,
        sources: &sources,
        runs: &[(
            failed(
                capture_failed(),
                Some(identity(capture_failed())),
                PolicyFailureClass::Recoverable,
            ),
            OVER_BUDGET_MS,
        )],
        failure_code: actingcommand_contract::POLICY_RUNTIME_BUDGET_EXCEEDED,
        failure_identity: None,
        counts: (1, 1),
    });
    // Both causes hold on the record the word is read from.
    assert!(paused.sensitive, "the task is sensitive: {paused:?}");
    assert_eq!(
        paused.original_class,
        PolicyFailureClass::Severe,
        "the rewrite is severe: {paused:?}"
    );
}

// The same recoverable failure twice, under an escalation threshold of 2 with retries left, pauses
// the pair at the second: `repeated_failure`.
#[test]
fn h2_the_same_failure_up_to_the_threshold_pauses_with_repeated_failure() {
    let failure_identity = identity(capture_failed());
    let repeated = failed(
        capture_failed(),
        Some(failure_identity.clone()),
        PolicyFailureClass::Recoverable,
    );
    let sources = sources_with(|task| {
        task["on_failure"] = json!({
            "action": "continue",
            "retry_limit": 3,
            "retry_backoff_ms": BACKOFF_MS,
            "escalation_threshold": 2
        });
    });
    check(&Fixture {
        word: PauseCause::RepeatedFailure,
        sources: &sources,
        runs: &[(repeated.clone(), RUN_MS), (repeated, RUN_MS)],
        failure_code: capture_failed(),
        failure_identity: Some(failure_identity),
        counts: (2, 2),
    });
}

// A recoverable failure of a task with no retry whose `on_failure` is `pause` pauses the pair at
// its first occurrence, below its escalation threshold: `retries_exhausted`.
#[test]
fn h2_a_task_out_of_retries_set_to_pause_pauses_with_retries_exhausted() {
    let failure_identity = identity(capture_failed());
    let sources = sources_with(|task| {
        task["on_failure"] = json!({
            "action": "pause",
            "retry_limit": 0,
            "retry_backoff_ms": BACKOFF_MS,
            "escalation_threshold": 3
        });
    });
    check(&Fixture {
        word: PauseCause::RetriesExhausted,
        sources: &sources,
        runs: &[(
            failed(
                capture_failed(),
                Some(failure_identity.clone()),
                PolicyFailureClass::Recoverable,
            ),
            RUN_MS,
        )],
        failure_code: capture_failed(),
        failure_identity: Some(failure_identity),
        counts: (1, 1),
    });
}

// ---- The backoff.

/// The pair's backoff at an evaluation at `at`: it carries `code` as `failure_code`, no
/// `pause_cause`, and its detail does not name the code.
fn assert_backoff(
    host: &RuntimeHost,
    clock: &ManualRuntimeClock,
    at: u64,
    seed: u64,
    code: Code,
    case: &str,
) {
    let (outcome, detail) = pair_reason(host, clock, at, seed, HostCode::PolicyRetryBackoffActive);
    assert_eq!(
        outcome.value("failure_code"),
        Some(&json!(code.as_str())),
        "{case}"
    );
    assert_eq!(outcome.value("pause_cause"), None, "{case}");
    assert!(!detail.contains(code.as_str()), "{case}: {detail}");
    assert!(
        host.fatal_error().expect("runtime health").is_none(),
        "{case}"
    );
}

// A recoverable failure with retries left schedules a retry: until its retry time the pair's
// `policy_retry_backoff_active` carries the failure's code as `failure_code` and no
// `pause_cause`, on the same host, on the root's second start, which seeds (it replays the
// settlement), and on its third, which loads the pair's streak from its value record; the start
// report says which each was. The settlement records the identity byte for byte and the first
// counts.
#[test]
fn h2_a_backoff_names_the_failure_it_follows() {
    let failure_identity = identity(capture_failed());
    let code = capture_failed();
    let sources = budget_policy_sources(1);
    let root = TempDir::new().expect("tempdir");
    let registered = instance_id();
    let (host, clock) = start(&root, registered, POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&sources)
        .expect("activate the catalog");
    let (intent, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    clock.advance(RUN_MS);
    let settled = host
        .record_policy_dispatch_outcome(
            &intent.decision_id,
            &failed(
                code,
                Some(failure_identity.clone()),
                PolicyFailureClass::Recoverable,
            ),
        )
        .expect("the settlement");
    let failure = failure_of(&settled);
    assert_eq!(
        failure.disposition,
        PolicyFailureDisposition::RetryScheduled
    );
    assert_eq!(failure.failure_identity, Some(failure_identity));
    assert_eq!(
        (failure.consecutive_same_error, failure.escalation_streak),
        (1, 1)
    );
    let settled_at = settled.observed_at_unix_ms;
    assert_eq!(failure.retry_at_unix_ms, Some(settled_at + BACKOFF_MS));
    assert_backoff(&host, &clock, settled_at + 1_000, 200, code, "live");
    host.close().expect("close the host");

    // The root's second start finds no marker and seeds: it replays the settlement.
    let seeded_at = settled_at + 5_000;
    let (host, clock) = start(&root, registered, seeded_at, None);
    assert_eq!(
        tagged_start(&host),
        Some(StartReportTaggedState::Seed),
        "the root's second start seeds"
    );
    assert_backoff(&host, &clock, seeded_at, 300, code, "the seeding start");
    host.close().expect("close the seeding host");

    // Its third start loads the pair's streak from its value record, as every restart of an
    // installation does; still within the backoff.
    let loaded_at = settled_at + 10_000;
    let (host, clock) = start(&root, registered, loaded_at, None);
    assert_eq!(
        tagged_start(&host),
        Some(StartReportTaggedState::Load),
        "the restart checked loads its decision memory from its value records"
    );
    assert_backoff(
        &host,
        &clock,
        loaded_at,
        400,
        code,
        "after a loading restart",
    );
    host.close().expect("close the restarted host");
}
