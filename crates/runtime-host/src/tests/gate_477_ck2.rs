// SPDX-License-Identifier: AGPL-3.0-only

// #477 CK-2 (0.12.81; register §1.516; the coordinator's rulings of 11-23 and their Addendum 1, no
// words of the owner's behind them; contracts/scheduling/README.md, "Clock Sources", "Clock-slot
// records"; contracts/policy-suspension.md, "Lifting (R19)"): the host's half. A pair's task
// snapshot carries `clock_owed` when the pair's latest dispatch never ran (it settled as
// interrupted) or its failure paused the pair of a task that is not sensitive, whatever the
// failure's class; every other settlement projects the snapshot of before. Through the host, on the
// clock-slot fixture of `gate_372k2` (one task, a daily UTC clock at 07:00, the activity window
// 06:00 to 23:30, four runs a day, no cooldown, an activity interval of 1 ms) and a manual Runtime
// clock:
// - a success, a continued failure, a retried failure (the immediate rerun runs as before), a
//   sensitive task's failure, a stopped run and a run a condition ended project no member, as
//   before, and the pair waits for its next occurrence or its own rerun (h1);
// - a failure that paused the pair leaves its occurrence owed: the pause holds the pair, its
//   refused record is written once, the lift of a changed package dispatches it on the same
//   occurrence, a new failure starts a new streak and is held again, and a run that succeeds after
//   the next lift serves it (h2);
// - a dispatch that never ran leaves its occurrence owed: the first interruption in a row lets the
//   pair run again on the same occurrence at once, the second holds it until its package changes,
//   with its refused record written once (h3);
// - a severe failure that pauses at its first occurrence (a screen the package never recorded)
//   leaves its occurrence owed: held, written refused once, and after the repaired package lifts
//   the pause the pair runs on the same occurrence, once (h4).
// Every alias, id and package is neutral.

use super::gate_372k2::{SLOT_TASK, at, slot_rows, slot_sources, slot_task};
use super::gate_386_sr2a::sampling_ended;
use super::gate_386_sr2b1::failure_of;
use super::*;
use crate::failure_identity::{FailureIdentity, IdentityFrame, IdentityPackage};
use actingcommand_contract::PolicyExecutionEventData;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_policy::{PolicyEvaluation, TaskRuntimeSnapshot};
use serde_json::{Value, json};

const SECOND_MS: u64 = 1_000;
const MINUTE_MS: u64 = 60_000;
/// The task's daily clock, 07:00 UTC, as a minute of the day.
const SLOT_MINUTE: u16 = 7 * 60;
/// The package id the failure identities of h2 name for the main package.
const MAIN_ID: &str = "neutral.main";

/// The fixture's own `on_failure`: continued after two retries with a backoff of 30 s, paused by
/// three failures alike.
fn catalog_on_failure() -> Value {
    json!({"action": "continue", "retry_limit": 2, "retry_backoff_ms": 30000, "escalation_threshold": 3})
}

/// No retry: a failure continues at once.
fn continued_at_once() -> Value {
    json!({"action": "continue", "retry_limit": 0, "retry_backoff_ms": 0, "escalation_threshold": 5})
}

/// No retry: a failure pauses the pair at once, its retries used up.
fn paused_at_once() -> Value {
    json!({"action": "pause", "retry_limit": 0, "retry_backoff_ms": 0, "escalation_threshold": 5})
}

/// The clock-slot catalog of `gate_372k2` with its one task's `on_failure` and `sensitive`.
fn slot_catalog(on_failure: Value, sensitive: bool) -> CatalogSources {
    let mut sources = slot_sources(1, &[slot_task(&[SLOT_MINUTE], 4)], None);
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("the slot tasks");
    tasks["tasks"][0]["on_failure"] = on_failure;
    tasks["tasks"][0]["sensitive"] = json!(sensitive);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("the slot task bytes");
    sources
}

/// A recoverable failure without a failure identity (`capture_failed`).
fn recoverable() -> PolicyExecutionInput {
    PolicyExecutionInput::Failed {
        outcome: Outcome::new(RuntimeErrorCode::CaptureFailed),
        failure_identity: None,
        class: PolicyFailureClass::Recoverable,
    }
}

/// The failure identity of a main-scope failure with the code `base` whose run went through the
/// main package with the content hash prefix `main_hash` (`x`: not measured).
fn identity(base: &str, main_hash: &str) -> String {
    FailureIdentity {
        base: base.to_owned(),
        key: "0".repeat(12),
        main: IdentityPackage {
            id_hash: format!("{:x}", Sha256::digest(MAIN_ID.as_bytes()))
                .chars()
                .take(8)
                .collect(),
            hash: main_hash.to_owned(),
        },
        layers: Vec::new(),
        frame: IdentityFrame::None,
    }
    .encode()
}

/// A recoverable step failure (`page_confirmation_failed`) with its failure identity naming the
/// main package's content `main_hash`.
fn step_failure(main_hash: &str) -> PolicyExecutionInput {
    PolicyExecutionInput::Failed {
        outcome: Outcome::new(
            actingcommand_execution_kernel::ExecutionKernelCode::PageConfirmationFailed,
        ),
        failure_identity: Some(identity("page_confirmation_failed", main_hash)),
        class: PolicyFailureClass::Recoverable,
    }
}

/// A linear step met a screen its package never recorded, severe (it pauses its pair at its first
/// occurrence), with the failure identity of the main package's content `main_hash`. The failure
/// carries the values its catalog entry requires, as `gate_469_h2` gives them.
fn unrecorded_screen(main_hash: &str) -> PolicyExecutionInput {
    PolicyExecutionInput::Failed {
        outcome: Outcome::new(
            actingcommand_execution_kernel::ExecutionKernelCode::ContainedTaskLinearUnrecordedScreen,
        )
        .pack_operation_id("open")
        .page_ids(["neutral/list", "neutral/home"])
        .attempts(1_u32),
        failure_identity: Some(identity(
            "contained_task_linear_unrecorded_screen",
            main_hash,
        )),
        class: PolicyFailureClass::Severe,
    }
}

/// A run settled as interrupted: it never ran, or the Runtime's end cut it short.
fn interrupted() -> PolicyExecutionInput {
    PolicyExecutionInput::Failed {
        outcome: Outcome::new(SharedCode::PolicySettlementInterrupted),
        failure_identity: None,
        class: PolicyFailureClass::Severe,
    }
}

/// Writes the assembled package `bytes` at `path`; the first twelve hex digits of its content
/// hash, the SHA-256 of the whole file, as a lift check measures it.
fn write_main(path: &Path, bytes: &[u8]) -> String {
    fs::write(path, bytes).expect("write the main package");
    format!("{:x}", Sha256::digest(bytes))
        .chars()
        .take(12)
        .collect()
}

/// The fixture's procedures, `procedure.observe` (the task's) bound at `main`, as actingd binds a
/// scheduled procedure (`scheduled_execution.package_path`), so a lift check measures it.
fn manifest_at(main: &Path) -> ProcedureManifest {
    ProcedureManifest::new(
        [
            "procedure.observe",
            "procedure.observe-b",
            "procedure.detect",
        ]
        .map(|procedure_ref| {
            let binding = ProcedureBinding::new(
                procedure_ref,
                "operation.observe",
                vec!["after_observation".to_owned()],
            )
            .expect("procedure binding");
            if procedure_ref == "procedure.observe" {
                binding.with_package_path(main.to_string_lossy())
            } else {
                binding
            }
        }),
    )
    .expect("procedure manifest")
}

/// One host on a manual Runtime clock with the clock-slot catalog, as `gate_372k2`'s, its
/// procedure bound at a package when `main` is given.
struct Pair {
    host: RuntimeHost,
    clock: Arc<ManualRuntimeClock>,
    approved: bool,
    _root: TempDir,
}

impl Pair {
    fn start(root: TempDir, sources: &CatalogSources, main: Option<&Path>) -> Self {
        let clock = Arc::new(ManualRuntimeClock::new(at(0, 6, 0), 0));
        let mut facts = policy_facts();
        facts.outcomes.clear();
        let mut host_config = config(&root)
            .with_policy_inputs(PolicyInputSnapshot::new(facts, policy_resources()))
            .with_runtime_clock(clock.clone());
        if let Some(main) = main {
            host_config = host_config.with_procedure_manifest(manifest_at(main));
        }
        let host = RuntimeHost::start(
            host_config,
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                instance_id(),
                Arc::new(FakeState::default()),
            )),
        )
        .expect("start the host");
        host.activate_policy_catalog(sources)
            .expect("activate the catalog");
        Self {
            host,
            clock,
            approved: false,
            _root: root,
        }
    }

    /// Moves the Runtime clock and samples the host's capacity at the new time, as
    /// `gate_372k2`'s fixture does.
    fn set_time(&self, unix_ms: u64) {
        self.clock.set_unix_ms(unix_ms);
        let sample_capacity = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample_capacity().expect("capacity sample");
    }

    fn evaluate(&self, unix_ms: u64) -> PolicyEvaluation {
        self.set_time(unix_ms);
        let mut facts = policy_facts();
        facts.outcomes.clear();
        self.host
            .evaluate_policy_cycle_with_test_inputs(
                &facts,
                &policy_resources(),
                EvaluationTime {
                    unix_ms,
                    monotonic_ms: unix_ms,
                },
                7,
                PolicyTrigger::Recovery,
            )
            .unwrap_or_else(|error| panic!("evaluate at {unix_ms}: {error}"))
            .evaluation
            .unwrap_or_else(|| panic!("an evaluation at {unix_ms}"))
    }

    /// Evaluates at `unix_ms`, admits the pair's dispatch the evaluation selected, settles it as
    /// `input` and ends its lease, as a run of the task does; the settlement's record.
    fn dispatch(&mut self, unix_ms: u64, input: &PolicyExecutionInput) -> PolicyExecutionEventData {
        let evaluation = self.evaluate(unix_ms);
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == SLOT_TASK)
            .unwrap_or_else(|| panic!("the pair is selected at {unix_ms}: {evaluation:#?}"))
            .clone();
        let reasons = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("dispatch reason chain")
            .clone();
        if !self.approved {
            record_policy_approval(&self.host, &intent);
            self.approved = true;
        }
        let PolicyDispatchAdmission::Granted { context } = self
            .host
            .admit_policy_dispatch(&intent, &reasons, &policy_context(&self.host, &intent))
            .unwrap_or_else(|error| panic!("admit at {unix_ms}: {error}"))
        else {
            panic!("the pair is granted at {unix_ms}");
        };
        let settled = self
            .host
            .record_policy_dispatch_outcome(&intent.decision_id, input)
            .expect("the settlement");
        self.host
            .release_policy_dispatch_lease_for_test(&context)
            .expect("end the dispatch's lease");
        settled
    }

    /// The pair's task snapshot as an evaluation at `as_of` projects it.
    fn snapshot(&self, as_of: u64) -> TaskRuntimeSnapshot {
        self.host
            .policy_task_snapshots_for_test(as_of)
            .expect("the task snapshots")
            .into_iter()
            .find(|snapshot| {
                snapshot.task_id == SLOT_TASK && snapshot.instance_id == POLICY_INSTANCE_ALIAS
            })
            .unwrap_or_else(|| panic!("the pair's snapshot at {as_of}"))
    }

    fn close(self) {
        self.host.close().expect("close the host");
    }
}

/// Whether the evaluation selected the pair.
fn selected(evaluation: &PolicyEvaluation) -> bool {
    evaluation
        .dispatch_intents
        .iter()
        .any(|intent| intent.task_id == SLOT_TASK)
}

/// Whether the pair's decision carries a reason with `code`.
fn has_reason(evaluation: &PolicyEvaluation, code: &str) -> bool {
    evaluation
        .decisions
        .iter()
        .filter(|decision| {
            decision.task_id == SLOT_TASK
                && decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS)
        })
        .flat_map(|decision| decision.reasons.iter())
        .any(|reason| reason.outcome.code().as_str() == code)
}

/// The (kind, gate code) of every clock-slot record of `occurrence`, in ledger order.
fn records_of(host: &RuntimeHost, occurrence: u64) -> Vec<(String, Option<String>)> {
    slot_rows(host)
        .into_iter()
        .filter(|row| row.time("occurrence_unix_ms") == Some(occurrence))
        .map(|row| (row.kind.clone(), row.text("gate_code").map(str::to_owned)))
        .collect()
}

/// One refused record with the gate `policy_task_paused`.
fn refused_by_the_pause() -> Vec<(String, Option<String>)> {
    vec![("refused".to_owned(), Some("policy_task_paused".to_owned()))]
}

// Tests 1, 4, 5 and 8 of Addendum 1 (A2). Day 0's 07:00 run succeeds; day 1's 07:00 run settles as
// each case says. None projects `clock_owed`, and the snapshot serializes without it, as before:
// - a success (the golden of a ledger with no failed run), a failure continued at once, a sensitive
//   task's failure (paused as sensitive) and a stopped run: the occurrence is served, at 07:02 the
//   trigger is false; after the continued failure the pair is due again at day 2's 07:00, not
//   before;
// - a failure scheduled for a retry, with its failure identity: the immediate rerun runs at 07:02
//   as before (`failure_retry_immediate`);
// - a sampling timeout ended by a condition: its owed rerun runs at 07:02 as before
//   (`policy_rerun_owed`).
#[test]
fn ck2_h1_a_settlement_that_serves_its_occurrence_projects_no_member_as_before() {
    let stopped = PolicyExecutionInput::Stopped {
        outcome: Outcome::new(RuntimeErrorCode::ContainedTaskCancelled),
    };
    for (case, on_failure, sensitive, input, disposition, rerun_reason) in [
        (
            "success",
            catalog_on_failure(),
            false,
            PolicyExecutionInput::Succeeded,
            None,
            None,
        ),
        (
            "continued",
            continued_at_once(),
            false,
            recoverable(),
            Some(PolicyFailureDisposition::Continue),
            None,
        ),
        (
            "retried",
            catalog_on_failure(),
            false,
            step_failure("x"),
            Some(PolicyFailureDisposition::RetryScheduled),
            Some("failure_retry_immediate"),
        ),
        (
            "sensitive",
            paused_at_once(),
            true,
            recoverable(),
            Some(PolicyFailureDisposition::PausedTask),
            None,
        ),
        ("stopped", paused_at_once(), false, stopped, None, None),
        (
            "condition",
            paused_at_once(),
            false,
            sampling_ended(),
            Some(PolicyFailureDisposition::Continue),
            Some("policy_rerun_owed"),
        ),
    ] {
        let mut pair = Pair::start(
            TempDir::new().expect("tempdir"),
            &slot_catalog(on_failure, sensitive),
            None,
        );
        pair.dispatch(at(0, 7, 0), &PolicyExecutionInput::Succeeded);
        let settled = pair.dispatch(at(1, 7, 0), &input);
        let written = match &settled.outcome {
            PolicyExecutionOutcome::Failed { failure } => Some(failure.disposition),
            PolicyExecutionOutcome::Succeeded { .. } | PolicyExecutionOutcome::Stopped { .. } => {
                None
            }
        };
        assert_eq!(written, disposition, "{case}: {settled:?}");

        let after = at(1, 7, 2);
        let snapshot = pair.snapshot(after);
        assert!(!snapshot.clock_owed, "{case}: {snapshot:?}");
        let projected = serde_json::to_value(&snapshot).expect("the snapshot serializes");
        assert!(projected.get("clock_owed").is_none(), "{case}: {projected}");

        let evaluation = pair.evaluate(after);
        match rerun_reason {
            Some(reason) => assert!(
                selected(&evaluation) && has_reason(&evaluation, reason),
                "{case}: the Runtime's own rerun runs as before: {evaluation:#?}"
            ),
            None => assert!(
                !selected(&evaluation) && has_reason(&evaluation, "trigger_false"),
                "{case}: the occurrence is served: {evaluation:#?}"
            ),
        }
        if case == "continued" {
            assert!(!selected(&pair.evaluate(at(1, 23, 0))), "{case}");
            assert!(
                selected(&pair.evaluate(at(2, 7, 0))),
                "{case}: due at the next occurrence"
            );
        }
        pair.close();
    }
}

// Tests 2 and 9 of Addendum 1 (A1 (b), A3). `on_failure` pauses at the first failure (no retry).
// Day 0's 07:00 run succeeds; day 1's 07:00 run fails with a step failure whose identity records
// the main package's content (first): the pair is paused for it (`retries_exhausted`), and its
// snapshot carries `clock_owed`. At 07:01 and 07:01:30 the pause holds the pair
// (`policy_task_paused`), nothing is dispatched, and the occurrence's one record is refused with
// that gate. The package changes (second): the lift check after its interval lifts the pause
// and the pair runs on the same occurrence at 07:02:10; that run fails again, its identity naming
// the second content, a new streak (count 1), paused again and held at the next evaluations. The
// package changes back: the next lift dispatches it once more, that run succeeds, and the
// occurrence is served: nothing is due at 09:00, and the occurrence still has its one record.
#[test]
fn ck2_h2_a_failure_that_paused_the_pair_leaves_its_occurrence_owed_until_a_run_after_the_lift() {
    let root = TempDir::new().expect("tempdir");
    let main = root.path().join("main.zip");
    let first = write_main(&main, &neutral_contained_task_package(false));
    let mut pair = Pair::start(root, &slot_catalog(paused_at_once(), false), Some(&main));
    let occurrence = at(1, 7, 0);
    pair.dispatch(at(0, 7, 0), &PolicyExecutionInput::Succeeded);

    let paused_for = failure_of(&pair.dispatch(occurrence, &step_failure(&first)));
    assert_eq!(paused_for.disposition, PolicyFailureDisposition::PausedTask);
    assert_eq!(paused_for.effective_class, PolicyFailureClass::Recoverable);
    assert!(pair.snapshot(occurrence + MINUTE_MS).clock_owed);
    for now in [occurrence + MINUTE_MS, occurrence + 90 * SECOND_MS] {
        let evaluation = pair.evaluate(now);
        assert!(
            !selected(&evaluation) && has_reason(&evaluation, "policy_task_paused"),
            "at {now}: the pause holds the pair: {evaluation:#?}"
        );
    }
    assert_eq!(records_of(&pair.host, occurrence), refused_by_the_pause());

    let second = write_main(&main, &neutral_non_home_start_contained_task_package(false));
    let lifted_at = occurrence + 130 * SECOND_MS;
    let again = failure_of(&pair.dispatch(lifted_at, &step_failure(&second)));
    assert_eq!(again.consecutive_same_error, 1, "a new streak: {again:?}");
    assert_eq!(again.disposition, PolicyFailureDisposition::PausedTask);
    for now in [lifted_at + 20 * SECOND_MS, lifted_at + 70 * SECOND_MS] {
        let evaluation = pair.evaluate(now);
        assert!(
            !selected(&evaluation) && has_reason(&evaluation, "policy_task_paused"),
            "at {now}: the new pause holds the pair: {evaluation:#?}"
        );
    }

    write_main(&main, &neutral_contained_task_package(false));
    let rerun_at = lifted_at + 140 * SECOND_MS;
    pair.dispatch(rerun_at, &PolicyExecutionInput::Succeeded);
    assert!(!pair.snapshot(rerun_at + MINUTE_MS).clock_owed);
    let later = pair.evaluate(at(1, 9, 0));
    assert!(
        !selected(&later) && has_reason(&later, "trigger_false"),
        "the occurrence is served: {later:#?}"
    );
    assert_eq!(records_of(&pair.host, occurrence), refused_by_the_pause());
    pair.close();
}

// Tests 3 and 9 of Addendum 1 (A1 (a), A3). Day 0's 07:00 run succeeds; day 1's 07:00 dispatch
// settles as interrupted (`policy_settlement_interrupted`): its occurrence stays owed, as the
// snapshot says. The first interruption in a row does not hold the pair: at
// 07:01 it is dispatched again on the same occurrence, with no record. That one is interrupted
// too, the second in a row: from 07:02 the pause holds the pair until its package changes, and the
// occurrence's one record is refused by it. The package changes: after the lift check's interval
// the pair runs on the same occurrence, succeeds, and nothing is due at 09:00.
#[test]
fn ck2_h3_a_dispatch_that_never_ran_leaves_its_occurrence_owed_as_its_interruption_allows() {
    let root = TempDir::new().expect("tempdir");
    let main = root.path().join("main.zip");
    write_main(&main, &neutral_contained_task_package(false));
    let mut pair = Pair::start(
        root,
        &slot_catalog(catalog_on_failure(), false),
        Some(&main),
    );
    let occurrence = at(1, 7, 0);
    pair.dispatch(at(0, 7, 0), &PolicyExecutionInput::Succeeded);

    let first_cut = failure_of(&pair.dispatch(occurrence, &interrupted()));
    assert_eq!(first_cut.consecutive_same_error, 1);
    assert!(pair.snapshot(occurrence + MINUTE_MS).clock_owed);
    let second_cut = failure_of(&pair.dispatch(occurrence + MINUTE_MS, &interrupted()));
    assert_eq!(second_cut.consecutive_same_error, 2);
    assert!(records_of(&pair.host, occurrence).is_empty());

    for now in [occurrence + 2 * MINUTE_MS, occurrence + 150 * SECOND_MS] {
        let evaluation = pair.evaluate(now);
        assert!(
            !selected(&evaluation) && has_reason(&evaluation, "policy_task_paused"),
            "at {now}: a second interruption in a row holds the pair: {evaluation:#?}"
        );
    }
    assert_eq!(records_of(&pair.host, occurrence), refused_by_the_pause());

    write_main(&main, &neutral_non_home_start_contained_task_package(false));
    let lifted_at = occurrence + 190 * SECOND_MS;
    pair.dispatch(lifted_at, &PolicyExecutionInput::Succeeded);
    assert!(!pair.snapshot(lifted_at + MINUTE_MS).clock_owed);
    let later = pair.evaluate(at(1, 9, 0));
    assert!(
        !selected(&later) && has_reason(&later, "trigger_false"),
        "the occurrence is served: {later:#?}"
    );
    pair.close();
}

// The coordinator's fix round of A1 (b) (any pause but a sensitive task's) and its test: a pause at
// the first occurrence, the common case. Day 0's 07:00 run succeeds; day 1's 07:00 run meets a
// screen its package never recorded (`contained_task_linear_unrecorded_screen`, severe, its
// identity naming the main package's content): the pair is paused at once and the snapshot carries
// `clock_owed`. At 07:01 and 07:01:30 the pause holds the pair and the occurrence's one record is
// refused by it. The package is repaired (its content changes): after the lift check's interval
// the pair runs on the same occurrence, succeeds, and is due once only: nothing at 07:03:10 nor at
// 09:00, and the next occurrence, day 2's 07:00, is due as always.
#[test]
fn ck2_h4_a_first_occurrence_pause_lifted_by_a_repaired_package_runs_on_the_same_occurrence_once() {
    let root = TempDir::new().expect("tempdir");
    let main = root.path().join("main.zip");
    let first = write_main(&main, &neutral_contained_task_package(false));
    let mut pair = Pair::start(
        root,
        &slot_catalog(catalog_on_failure(), false),
        Some(&main),
    );
    let occurrence = at(1, 7, 0);
    pair.dispatch(at(0, 7, 0), &PolicyExecutionInput::Succeeded);

    let paused_for = failure_of(&pair.dispatch(occurrence, &unrecorded_screen(&first)));
    assert_eq!(paused_for.disposition, PolicyFailureDisposition::PausedTask);
    assert_eq!(paused_for.original_class, PolicyFailureClass::Severe);
    assert!(!paused_for.sensitive);
    assert!(pair.snapshot(occurrence + MINUTE_MS).clock_owed);
    for now in [occurrence + MINUTE_MS, occurrence + 90 * SECOND_MS] {
        let evaluation = pair.evaluate(now);
        assert!(
            !selected(&evaluation) && has_reason(&evaluation, "policy_task_paused"),
            "at {now}: the pause holds the pair: {evaluation:#?}"
        );
    }
    assert_eq!(records_of(&pair.host, occurrence), refused_by_the_pause());

    write_main(&main, &neutral_non_home_start_contained_task_package(false));
    let lifted_at = occurrence + 130 * SECOND_MS;
    pair.dispatch(lifted_at, &PolicyExecutionInput::Succeeded);
    assert!(!pair.snapshot(lifted_at + MINUTE_MS).clock_owed);
    for now in [lifted_at + MINUTE_MS, at(1, 9, 0)] {
        let evaluation = pair.evaluate(now);
        assert!(
            !selected(&evaluation) && has_reason(&evaluation, "trigger_false"),
            "at {now}: the occurrence is served, once: {evaluation:#?}"
        );
    }
    assert_eq!(records_of(&pair.host, occurrence), refused_by_the_pause());
    assert!(
        selected(&pair.evaluate(at(2, 7, 0))),
        "the next occurrence is due as always"
    );
    pair.close();
}
