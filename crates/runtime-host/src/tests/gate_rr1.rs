// SPDX-License-Identifier: AGPL-3.0-only

// RR-1 (register §1.221; coordinator work order `rd-1029/brief-retry-livelock.md`): the production
// defect of v0.12.30 on 10-30 08:4x. A failed task's immediate rerun (R22) was selected at every
// evaluation and refused `policy_retry_round_ended` at every admission, because the evaluation
// offered it on its live round alone while the admission also required the intent to keep the
// round's activity profile and expected duration, and the pair's expected duration E (SCH-M ED-2)
// had changed since the round opened. Its instance ran nothing else. Now the evaluation offers an
// immediate rerun only by the admission's own predicate, and an immediate rerun the admission
// still refuses with `policy_retry_round_ended` ends its round at once
// (`contracts/policy-suspension.md`, "Immediate rerun (R22)").
//
// A neutral fixture: the fixture instance; the failing task `fixture.observe` (priority 100, a
// cooldown of 11.5 h, so after its failure only the immediate rerun can bring it back before then)
// and its twin (priority 1, no cooldown), the instance's other candidate, both bound to one
// procedure at the neutral package's location (they share its run-duration samples); an activity
// window of the whole UTC day; a manual Runtime clock; the package admission after start held.
// Each run is settled through the policy API with a recoverable failure that carries a failure
// identity, as a linear or page-graph run's settlement writes one, or with a success. Every
// expected value is the work order's.

use super::*;
use crate::failure_identity::{FailureIdentity, IdentityFrame, IdentityPackage};
use crate::pack_admission::PackAdmissionHold;
use crate::policy_control::PolicyValueRecord;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{PolicyControlStateRecord, PolicyExecutionEventData};
use actingcommand_policy::{PolicyEvaluation, TaskDecision};
use serde_json::{Value, json};

/// The neutral package's `package_id` (every neutral fixture package declares it).
const PACKAGE_ID: &str = "neutral.semantic.task";
/// The task whose run fails.
const FAILING: &str = "fixture.observe";
/// The instance's other candidate.
const OTHER: &str = "fixture.observe-twin";
const PROCEDURE: &str = "procedure.observe";
/// The tasks' declared duration D.
const DECLARED_MS: u64 = 60_000;
/// The shipped margin and number of runs of E.
const MARGIN_MS: u64 = 300_000;
const RUNS: usize = 5;
/// Ten hours: no E of these tests reaches it.
const MAX_RUNTIME_MS: u64 = 36_000_000;
/// The failing task's cooldown: 11.5 h.
const COOLDOWN_MS: u64 = 41_400_000;
const MINUTE_MS: u64 = 60_000;
const DAY_MS: u64 = 86_400_000;
const IMMEDIATE: &str = "failure_retry_immediate";
const COOLDOWN_ACTIVE: &str = "task_cooldown_active";
const ROUND_ENDED: &str = "policy_retry_round_ended";
/// The failure the runs end with: recoverable, accumulating, never a condition.
const FAILURE_CODE: &str = "page_confirmation_failed";

// ---- The fixture.

/// The fixture catalog: both tasks due at every evaluation (an interval trigger of 1 ms); the
/// failing task first by priority but held 11.5 h by its cooldown after a dispatch; the twin
/// without a cooldown at the lowest priority; an activity window of the whole day without an
/// interval.
fn catalog() -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("fixture tasks");
    let failing = &mut tasks["tasks"][0];
    failing["trigger"] = json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"}, "every_ms": 1, "anchor_ms": 0}
    });
    failing["expected_duration_ms"] = json!(DECLARED_MS);
    failing["loop_budget"] = json!({
        "daily_limit": 1000,
        "window_iteration_limit": 1000,
        "max_runtime_ms": MAX_RUNTIME_MS
    });
    failing["on_failure"]["escalation_threshold"] = json!(1000);
    let mut other = failing.clone();
    failing["cooldown_ms"] = json!(COOLDOWN_MS);
    other["id"] = json!(OTHER);
    other["priority"] = json!(1);
    other["cooldown_ms"] = json!(0);
    other["produces"] = json!([]);
    other["instance_overrides"] = json!([]);
    tasks["tasks"]
        .as_array_mut()
        .expect("fixture tasks array")
        .push(other);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("fixture task bytes");
    let mut activity: Value =
        serde_json::from_slice(&sources.activity.bytes).expect("fixture activity");
    let profile = &mut activity["profiles"][0];
    profile["windows"] = json!([{
        "weekdays": [1, 2, 3, 4, 5, 6, 7],
        "utc_offset_minutes": 0,
        "start_minute_of_day": 0,
        "end_minute_of_day": 0
    }]);
    profile["daily_budget"] = json!(1000);
    profile["max_window_iterations"] = json!(1000);
    profile["session_max_ms"] = json!(DAY_MS);
    profile["minimum_interval_ms"] = json!(1);
    profile["maximum_interval_ms"] = json!(1);
    sources.activity.bytes = serde_json::to_vec_pretty(&activity).expect("fixture activity bytes");
    sources
}

/// The procedure bindings: the fixture procedure at `package`'s location.
fn manifest(package: &Path) -> ProcedureManifest {
    ProcedureManifest::new([PROCEDURE, "procedure.observe-b", "procedure.detect"].map(
        |procedure_ref| {
            let binding = ProcedureBinding::new(
                procedure_ref,
                "operation.observe",
                vec!["after_observation".to_owned()],
            )
            .expect("procedure binding");
            if procedure_ref == PROCEDURE {
                binding.with_package_path(package.to_string_lossy())
            } else {
                binding
            }
        },
    ))
    .expect("procedure manifest")
}

/// One host on `root` and its pieces, which a restart reuses.
struct Bench {
    root: TempDir,
    clock: Arc<ManualRuntimeClock>,
    state: Arc<FakeState>,
    instance: InstanceId,
    package: PathBuf,
}

impl Bench {
    fn new() -> Self {
        let root = TempDir::new().expect("tempdir");
        let path = root.path().join("rr1-task.zip");
        fs::write(&path, neutral_region_contained_task_package(false)).expect("write the package");
        Self {
            root,
            clock: Arc::new(ManualRuntimeClock::new(
                POLICY_NOW_UNIX_MS,
                POLICY_NOW_UNIX_MS,
            )),
            state: Arc::new(FakeState::default()),
            instance: instance_id(),
            package: path,
        }
    }

    fn path(&self) -> String {
        self.package.to_string_lossy().into_owned()
    }

    /// A host with the fixture catalog active.
    fn start(&self) -> RuntimeHost {
        let (hold, _release, _finished) = PackAdmissionHold::open();
        let host_config = config(&self.root)
            .with_runtime_clock(self.clock.clone())
            .with_procedure_manifest(manifest(&self.package))
            .with_pack_admission_hold_for_test(hold);
        let provider = FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            self.instance,
            Arc::clone(&self.state),
        )
        .fixture_simulation();
        let host = RuntimeHost::start(host_config, Arc::new(provider)).expect("runtime host");
        host.activate_policy_catalog(&catalog())
            .expect("activate the fixture catalog");
        host
    }

    /// Evaluates at `at` (the Runtime clock moved there first).
    fn evaluate(&self, host: &RuntimeHost, at: u64, seed: u64) -> PolicyEvaluation {
        advance_manual_clock_to(&self.clock, at);
        host.evaluate_policy_cycle_with_test_inputs(
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
        .expect("an evaluation")
    }

    /// Moves the Runtime clock to `at`.
    fn at(&self, at: u64) {
        advance_manual_clock_to(&self.clock, at);
    }
}

/// The evaluation's dispatch of `task` on the fixture instance and its reason chain, if any.
fn dispatch_of(
    evaluation: &PolicyEvaluation,
    task: &str,
) -> Option<(DispatchIntent, DecisionReasonChain)> {
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == task && intent.instance_id == POLICY_INSTANCE_ALIAS)?
        .clone();
    let chain = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the dispatch reason chain")
        .clone();
    Some((intent, chain))
}

/// The evaluation's decision of `task` on the fixture instance.
fn decision<'a>(evaluation: &'a PolicyEvaluation, task: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == task
                && decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS)
        })
        .unwrap_or_else(|| panic!("a decision of {task}: {evaluation:#?}"))
}

fn has_reason(decision: &TaskDecision, code: &str) -> bool {
    decision
        .reasons
        .iter()
        .any(|reason| reason.outcome.code().as_str() == code)
}

fn chain_has(chain: &DecisionReasonChain, code: &str) -> bool {
    chain
        .reasons
        .iter()
        .any(|reason| reason.outcome.code().as_str() == code)
}

/// Approves the catalog and admits the dispatch, which must be granted.
fn admit(
    host: &RuntimeHost,
    intent: &DispatchIntent,
    chain: &DecisionReasonChain,
) -> Box<PolicyRunContext> {
    record_policy_approval(host, intent);
    // A capacity sample is fresh only for a while on the Runtime clock, which the tests move.
    host.capacity_sampler_for_test().expect("capacity owner")()
        .expect("capacity sample at the admission time");
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(intent, chain, &policy_context(host, intent))
        .expect("the dispatch is admitted")
    else {
        panic!("expected a granted dispatch")
    };
    context
}

/// A recoverable failure with a failure identity: R22 reruns it at once.
fn identified_failure() -> PolicyExecutionInput {
    let identity = FailureIdentity {
        base: FAILURE_CODE.to_owned(),
        key: "0".repeat(12),
        main: IdentityPackage::measured(PACKAGE_ID, None),
        layers: Vec::new(),
        frame: IdentityFrame::None,
    }
    .encode();
    PolicyExecutionInput::Failed {
        outcome: Outcome::new(
            actingcommand_execution_kernel::ExecutionKernelCode::PageConfirmationFailed,
        ),
        failure_identity: Some(identity),
        class: PolicyFailureClass::Recoverable,
    }
}

/// Settles the admitted dispatch `intent` at `at` as `input` and ends its lease.
fn settle(
    bench: &Bench,
    host: &RuntimeHost,
    context: &PolicyRunContext,
    intent: &DispatchIntent,
    input: &PolicyExecutionInput,
    at: u64,
) -> PolicyExecutionEventData {
    bench.at(at);
    let execution = host
        .record_policy_dispatch_outcome(&intent.decision_id, input)
        .expect("settle the dispatch");
    host.release_policy_dispatch_lease_for_test(context)
        .expect("end the dispatch's lease");
    execution
}

/// Settles the failing task's dispatch with the identified failure; it is scheduled for a retry.
/// Returns the retry time.
fn fail(
    bench: &Bench,
    host: &RuntimeHost,
    context: &PolicyRunContext,
    intent: &DispatchIntent,
    at: u64,
) -> u64 {
    let execution = settle(bench, host, context, intent, &identified_failure(), at);
    let PolicyExecutionOutcome::Failed { failure } = &execution.outcome else {
        panic!("expected a failure: {execution:?}")
    };
    assert!(failure.condition.is_none(), "an ordinary failure");
    assert_eq!(
        failure.disposition,
        PolicyFailureDisposition::RetryScheduled,
        "{failure:?}"
    );
    failure.retry_at_unix_ms.expect("a retry time")
}

/// Adds the samples `durations` (in that order, the last the newest), each run ending one second
/// after the previous one from `from`; returns the last one's time.
fn add_samples(bench: &Bench, host: &RuntimeHost, durations: &[u64], from: u64) -> u64 {
    let mut at = from;
    for duration in durations {
        at += 1_000;
        bench.at(at);
        host.add_run_duration_sample_for_test(POLICY_INSTANCE_ALIAS, PACKAGE_ID, *duration, at)
            .expect("record the sample");
    }
    at
}

/// E of `samples`, newest first, by the model's formula.
fn expected_of(samples: &[u64]) -> u64 {
    let used = &samples[..samples.len().min(RUNS)];
    let sum = used.iter().sum::<u64>();
    sum.div_ceil(used.len() as u64) + MARGIN_MS
}

/// The `policy.dispatch_rejected` records of `task`, and how many carry `code`.
fn rejections(host: &RuntimeHost, task: &str, code: &str) -> (usize, usize) {
    let rejected = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::PolicyDispatchRejected),
            ..EventQuery::default()
        })
        .expect("the rejected dispatches")
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Policy(PolicyPayload::DispatchRejected(payload))
                if payload.task_id() == task =>
            {
                Some(
                    payload
                        .rejection()
                        .is_some_and(|rejection| rejection.outcome.code().as_str() == code),
                )
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    (
        rejected.len(),
        rejected.iter().filter(|carries| **carries).count(),
    )
}

/// `task`'s decision memory record as the host holds it; it equals the record today's replay of
/// the dispatch history rebuilds.
fn held_record(host: &RuntimeHost, task: &str) -> PolicyControlStateRecord {
    let (held, replayed, _) = host
        .decision_memory_for_test()
        .expect("the decision memory");
    let key = format!("decision_memory/{POLICY_INSTANCE_ALIAS}/{task}");
    let record = |records: &BTreeMap<String, PolicyValueRecord>| match records.get(&key) {
        Some(PolicyValueRecord::Control(record)) => (**record).clone(),
        other => panic!("the value record of {task}: {other:?}"),
    };
    assert_eq!(
        record(&held),
        record(&replayed),
        "{task}: held and replayed"
    );
    record(&held)
}

/// The end of the fixture's activity window, the whole UTC day, that holds `at`.
fn window_end(at: u64) -> u64 {
    (at / DAY_MS + 1) * DAY_MS
}

/// Tests 2, 3 and 5: the failing task fails 30 minutes before its window ends; its immediate
/// rerun is evaluated before the end and reaches its admission after it, which refuses it
/// `policy_retry_round_ended`. Returns the host, the first run's evaluation time and the window
/// end; the Runtime clock stands one second after the end.
fn refused_across_the_window_end(bench: &Bench) -> (RuntimeHost, u64, u64) {
    let host = bench.start();
    let end = window_end(POLICY_NOW_UNIX_MS);
    let first_at = end - 30 * MINUTE_MS;
    let evaluation = bench.evaluate(&host, first_at, 21);
    let (intent, chain) = dispatch_of(&evaluation, FAILING)
        .unwrap_or_else(|| panic!("the failing task runs first: {evaluation:#?}"));
    assert!(!chain_has(&chain, IMMEDIATE), "{chain:#?}");
    let context = admit(&host, &intent, &chain);
    let retry_at = fail(bench, &host, &context, &intent, first_at + 1_000);

    let evaluation = bench.evaluate(&host, retry_at + 1_000, 22);
    let (rerun, chain) = dispatch_of(&evaluation, FAILING)
        .unwrap_or_else(|| panic!("the immediate rerun before the end: {evaluation:#?}"));
    assert!(chain_has(&chain, IMMEDIATE), "{chain:#?}");
    assert!(
        held_record(&host, FAILING).retry_round.is_some(),
        "the round is open before the end"
    );

    bench.at(end + 1_000);
    record_policy_approval(&host, &rerun);
    host.capacity_sampler_for_test().expect("capacity owner")()
        .expect("capacity sample at the admission time");
    let refused = host
        .admit_policy_dispatch(&rerun, &chain, &policy_context(&host, &rerun))
        .expect_err("the window ended between the evaluation and the admission");
    assert_eq!(refused.code(), ROUND_ENDED, "{refused:?}");
    (host, first_at, end)
}

// ---- The tests.

// Test 1, the production shape: a failed task with an open round; its expected duration E
// changes; the next evaluation does not propose it as an immediate rerun, and the instance's other
// eligible candidate is selected and admitted; nothing is refused. Two legs, the two ways E
// changes while a round is open: a new run sample of the package, and the package becoming known
// after the round opened (a round opened by the first run after a start carries the declared
// duration, and that run's own preparation makes the samples readable). Before E changes, the
// pair is rerun at once as before (R22).
#[test]
fn rr1_t1_a_changed_expected_duration_is_not_offered_at_once_and_the_instance_runs_another() {
    for (leg, known_before) in [
        ("a new run sample", true),
        ("the package becomes known", false),
    ] {
        let bench = Bench::new();
        let host = bench.start();
        if known_before {
            host.learn_package_for_test(&bench.path(), PACKAGE_ID)
                .expect("the package's id at its location");
        }
        let at = add_samples(&bench, &host, &[1_000], POLICY_NOW_UNIX_MS);
        let opened_with = if known_before {
            expected_of(&[1_000])
        } else {
            DECLARED_MS
        };

        // The failing task's normal trigger opens its round, and its run fails.
        let first_at = at + MINUTE_MS;
        let evaluation = bench.evaluate(&host, first_at, 11);
        let (intent, chain) = dispatch_of(&evaluation, FAILING)
            .unwrap_or_else(|| panic!("{leg}: the failing task runs first: {evaluation:#?}"));
        assert_eq!(intent.expected_duration_ms, opened_with, "{leg}");
        let context = admit(&host, &intent, &chain);
        let retry_at = fail(&bench, &host, &context, &intent, first_at + 1_000);
        let round = held_record(&host, FAILING)
            .retry_round
            .unwrap_or_else(|| panic!("{leg}: the round is open"));
        assert_eq!(round.expected_duration_ms, opened_with, "{leg}");

        // While E is the round's, the pair is rerun at once.
        let evaluation = bench.evaluate(&host, retry_at + 1_000, 12);
        assert!(
            has_reason(decision(&evaluation, FAILING), IMMEDIATE),
            "{leg}: rerun at once while its terms hold: {evaluation:#?}"
        );

        // E changes.
        let at = if known_before {
            add_samples(&bench, &host, &[3_000], retry_at + 1_000)
        } else {
            host.learn_package_for_test(&bench.path(), PACKAGE_ID)
                .expect("the package's id at its location");
            retry_at + 1_000
        };
        let changed = if known_before {
            expected_of(&[3_000, 1_000])
        } else {
            expected_of(&[1_000])
        };
        assert_ne!(changed, opened_with, "{leg}");

        let evaluation = bench.evaluate(&host, at + 1_000, 13);
        let failing = decision(&evaluation, FAILING);
        assert!(
            !has_reason(failing, IMMEDIATE),
            "{leg}: not proposed as an immediate rerun: {failing:#?}"
        );
        assert!(
            has_reason(failing, COOLDOWN_ACTIVE),
            "{leg}: it waits for its own trigger and cooldown: {failing:#?}"
        );
        assert!(dispatch_of(&evaluation, FAILING).is_none(), "{leg}");
        let (other, chain) = dispatch_of(&evaluation, OTHER).unwrap_or_else(|| {
            panic!("{leg}: the instance's other candidate is selected: {evaluation:#?}")
        });
        assert_eq!(other.expected_duration_ms, changed, "{leg}");
        admit(&host, &other, &chain);
        assert_eq!(
            rejections(&host, FAILING, ROUND_ENDED),
            (0, 0),
            "{leg}: nothing is refused"
        );
        assert!(host.fatal_error().expect("runtime health").is_none());
        host.close().expect("close the host");
    }
}

// Test 2: the window ends between the evaluation and the admission. The refused immediate rerun
// is withdrawn at once (its round is gone from the decision memory and its value record, and the
// replay of the refusal rebuilds the same; its failure state stays), the next round selects the
// instance's other candidate, and the pair is refused no more than once.
#[test]
fn rr1_t2_a_refused_immediate_rerun_is_withdrawn_and_the_next_round_runs_another() {
    let bench = Bench::new();
    let (host, _, end) = refused_across_the_window_end(&bench);
    let record = held_record(&host, FAILING);
    assert!(record.retry_round.is_none(), "withdrawn: {record:#?}");
    assert_eq!(
        record
            .failure_streak
            .as_ref()
            .map(|streak| streak.disposition),
        Some(PolicyFailureDisposition::RetryScheduled),
        "the failure state stays: {record:#?}"
    );

    let evaluation = bench.evaluate(&host, end + 2_000, 23);
    let failing = decision(&evaluation, FAILING);
    assert!(!has_reason(failing, IMMEDIATE), "{failing:#?}");
    assert!(dispatch_of(&evaluation, FAILING).is_none());
    let (other, chain) = dispatch_of(&evaluation, OTHER)
        .unwrap_or_else(|| panic!("the instance's other candidate: {evaluation:#?}"));
    let context = admit(&host, &other, &chain);
    settle(
        &bench,
        &host,
        &context,
        &other,
        &PolicyExecutionInput::Succeeded,
        end + 3_000,
    );
    let evaluation = bench.evaluate(&host, end + 4_000, 24);
    assert!(!has_reason(decision(&evaluation, FAILING), IMMEDIATE));
    assert_eq!(
        rejections(&host, FAILING, ROUND_ENDED),
        (1, 1),
        "one refusal, not repeated"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// Test 3: after the withdrawal, the pair's next legal normal trigger (its cooldown over) opens a
// new round and the task runs.
#[test]
fn rr1_t3_a_legal_normal_trigger_afterwards_opens_a_new_round_and_the_task_runs() {
    let bench = Bench::new();
    let (host, first_at, _) = refused_across_the_window_end(&bench);
    let at = first_at + COOLDOWN_MS + MINUTE_MS;
    let evaluation = bench.evaluate(&host, at, 31);
    let (intent, chain) = dispatch_of(&evaluation, FAILING)
        .unwrap_or_else(|| panic!("the normal trigger runs it: {evaluation:#?}"));
    assert!(
        !chain_has(&chain, IMMEDIATE),
        "a normal dispatch: {chain:#?}"
    );
    let context = admit(&host, &intent, &chain);
    let round = held_record(&host, FAILING)
        .retry_round
        .expect("the normal admission opens a new round");
    assert_eq!(
        &round.admission,
        context.admission(),
        "the new round's origin"
    );
    assert!(round.admission.activity.admitted_at_unix_ms >= at);
    assert_eq!(
        rejections(&host, FAILING, ROUND_ENDED),
        (1, 1),
        "the run is admitted"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// Test 4 (existing behaviour): the success of the run the new round opened with clears the round
// and the failure state.
#[test]
fn rr1_t4_success_still_clears_the_round() {
    let bench = Bench::new();
    let (host, first_at, _) = refused_across_the_window_end(&bench);
    let at = first_at + COOLDOWN_MS + MINUTE_MS;
    let evaluation = bench.evaluate(&host, at, 41);
    let (intent, chain) = dispatch_of(&evaluation, FAILING)
        .unwrap_or_else(|| panic!("the normal trigger runs it: {evaluation:#?}"));
    let context = admit(&host, &intent, &chain);
    assert!(held_record(&host, FAILING).retry_round.is_some());
    settle(
        &bench,
        &host,
        &context,
        &intent,
        &PolicyExecutionInput::Succeeded,
        at + 1_000,
    );
    let record = held_record(&host, FAILING);
    assert!(record.retry_round.is_none(), "{record:#?}");
    assert!(record.failure_streak.is_none(), "{record:#?}");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// Test 5: a restart after the withdrawal. The start loads the pair's value record written with the
// refusal and does not resurrect the round; the loaded decision memory equals the one today's
// replay of the dispatch history rebuilds (which withdraws it again from the recorded refusal);
// the next evaluation offers no immediate rerun and runs the other candidate.
#[test]
fn rr1_t5_a_restart_does_not_resurrect_the_withdrawn_immediate_rerun() {
    let bench = Bench::new();
    let (host, _, end) = refused_across_the_window_end(&bench);
    let before = held_record(&host, FAILING);
    assert!(before.retry_round.is_none());
    host.close().expect("close the first host");

    let restarted = bench.start();
    assert!(
        restarted
            .fatal_error()
            .expect("read the fatal state")
            .is_none()
    );
    let after = held_record(&restarted, FAILING);
    assert!(after.retry_round.is_none(), "not resurrected: {after:#?}");
    assert_eq!(
        after.failure_streak, before.failure_streak,
        "the failure state the refusal kept"
    );
    let evaluation = bench.evaluate(&restarted, end + 2_000, 51);
    assert!(!has_reason(decision(&evaluation, FAILING), IMMEDIATE));
    assert!(dispatch_of(&evaluation, OTHER).is_some(), "{evaluation:#?}");
    assert_eq!(rejections(&restarted, FAILING, ROUND_ENDED), (1, 1));
    restarted.close().expect("close the restarted host");
}
