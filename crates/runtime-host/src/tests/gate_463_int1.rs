// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463 INT-1 (register §1.473; contracts/package-reference.md, "Standard package root";
//! contracts/README.md, "Shared interrupt pages"; coordinator work order
//! `rd-1118/brief-463-int1-interrupt-end-only.md` and its rulings
//! `rd-1118/brief-463-int1-rulings.md`): a shared interrupt page whose `interrupts.json` entry
//! names no handler (`handlers: []`, `then: end_run`) ends the run where the run meets it, with
//! nothing pressed, no handler layer and no `interrupt_handled` record, as its `result` says; the
//! word `environment_reset` (with a handler or without) ends the run with
//! `contained_task_environment_reset`, class `budget_or_permission`, which is rerun only and never
//! accumulates toward a pause.
//!
//! Fix round 1 (`rd-1118/brief-463-int1-fix1.md`, the review's way (a)): a linear step's closing
//! decision, which only recognises the shared interrupt pages, gets from the host the end of the
//! run such a page gives (`SharedInterruptShown::Ended`) and the kernel carries it out of the
//! decision as it is; an entry with a handler seen there stays `interrupt_shown`.
//!
//! The fixtures are NAV-6's (`gate_373_nav6`): the synthetic `neutral` game, whose overlay page
//! `notice` (a white first pixel) stands here for the screen the environment throws the game back
//! to, the run's own pages `away` (red) and `home` (blue), the feature package of `pages.json`
//! and the handler `close_notice` (to `away`). Linear runs wait on a manual Runtime clock; no
//! wall clock is read.

use super::gate_373_nav6::{
    AWAY, B43_TASK, BLACK, CLOSE_NOTICE, GameRoot, HOME, OWN_NOTICE, Overlay, RED, Run, TO_AWAY,
    U1_ARRIVAL_MS, away_to_home, b43_failed_scheduled_run, b43_recorded_failure, b43_rerun_time,
    clock_now, entry_away_to_home, frame, handler_task, interrupt, ladder_started, operation,
    overlay_then_away_then_home, pausing_policy_sources, run, run_at, run_package, script,
    standard_pack_reads, start, start_with, text, write_linear_away_to_home,
};
use super::*;
use crate::codes::HostCode;
use crate::failure_identity::{FailureIdentity, failure_accumulates};
use actingcommand_contract::outcome::{Category, CodeStr, LadderClass};
use actingcommand_contract::{PolicyFailureRecord, TaskInterruptThen, scheduled_failure_code_str};
use serde_json::{Value, json};

const ENVIRONMENT_RESET: &str = "contained_task_environment_reset";
const ESCALATED: &str = "contained_task_interrupt_escalated";
const UPDATE_REQUIRED: &str = "contained_task_application_update_required";
const ROOT_INVALID: &str = "standard_pack_root_invalid";
const UNRECORDED_SCREEN: &str =
    actingcommand_execution_kernel::ExecutionKernelCode::ContainedTaskLinearUnrecordedScreen
        .as_str();
const CONFIRMATION_FAILED: &str =
    actingcommand_execution_kernel::ExecutionKernelCode::PageConfirmationFailed.as_str();
const UPDATE_KEY: &str = "health.application.update_required";

/// The entry of `notice` that names no handler: `then: end_run`, `max_per_run` 8 (which a page
/// that ends the run at its first meeting never reaches; its range is as before), and `result`
/// when one is given.
fn without_handlers(result: Option<&str>) -> Value {
    let mut entry = json!({"page": "notice", "handlers": [], "then": "end_run", "max_per_run": 8});
    if let Some(result) = result {
        entry["result"] = json!(result);
    }
    entry
}

/// A game root with the feature package and the shared interrupt pages `entries`; with
/// `handler`, also NAV-6's handler of `notice`.
fn root_of(base: &Path, entries: Value, handler: bool) -> GameRoot {
    let mut root = GameRoot::new(base);
    root.features();
    if handler {
        root.pack(
            CLOSE_NOTICE,
            "navigable_route",
            handler_task("notice", "first_white", TO_AWAY),
        );
    }
    root.bundle().pages().interrupts(entries);
    root
}

/// A run that `code` ended (`category`, `page_id` `notice`), as its failed terminal records it.
fn assert_ended(run: &Run, code: &str, category: Category, case: &str) {
    assert_eq!(
        run.receipt.state(),
        RuntimeReceiptState::Failed,
        "{case}: {:?}",
        run.receipt
    );
    let failure = run.failure();
    assert_eq!(failure.code().as_str(), code, "{case}");
    assert_eq!(failure.category(), category, "{case}");
    assert_eq!(text(&failure, "page_id"), Some("notice"), "{case}");
}

/// The ledger of a run that a page without handlers ended: no `interrupt_handled` record and no
/// layer of `role: interrupt_handler` (no `EntryRecoveryPackageAdmitted`, `EntryRecoveryCompleted`
/// or `EntryRecoveryFailed` of that role).
fn assert_nothing_handled(run: &Run, case: &str) {
    let handled = run.handled();
    assert!(handled.is_empty(), "{case}: {handled:?}");
    let (admitted, completed, failed) = run.handler_layers();
    assert_eq!(admitted, 0, "{case}: no handler layer");
    assert!(completed.is_empty(), "{case}: {completed:?}");
    assert!(failed.is_empty(), "{case}: {failed:?}");
}

/// Fix round 1: the frames of the run of [`write_linear_away_to_home`] on the Runtime clock
/// `clock`: `away` until the step's tap; after it, with `closing`, U1's frames (`gate_373_nav6`'s
/// `u1_the_recognise_only_entry_answers_shown_and_presses_counts_and_records_nothing`: a black
/// first pixel, no page of the run or of the feature package, until the arrival's 100 ms have
/// passed since the first frame after the tap, so the arrival's frames and the closing decision's
/// first frame are black and its next frame is the first to show the page), then `notice`;
/// without it, `notice` from the first frame after the tap on. The count of the frames taken that
/// show `notice`.
fn notice_after_the_tap(
    state: &FakeState,
    clock: &Arc<ManualRuntimeClock>,
    closing: bool,
) -> Arc<std::sync::atomic::AtomicUsize> {
    let script_clock = Arc::clone(clock);
    let shown = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let shown_frames = Arc::clone(&shown);
    let tapped_at = std::sync::Mutex::new(None::<u64>);
    script(state, move |_capture, inputs| {
        if inputs == 0 {
            return frame(RED, None);
        }
        let now_ms = script_clock.monotonic_ms();
        let tapped_ms = *tapped_at
            .lock()
            .expect("the tap's time")
            .get_or_insert(now_ms);
        if closing && now_ms - tapped_ms <= U1_ARRIVAL_MS {
            frame(BLACK, None)
        } else {
            shown_frames.fetch_add(1, Ordering::AcqRel);
            frame(RED, Some(Overlay::Notice))
        }
    });
    shown
}

/// Fix round 1 (the review: the scheduled request's package is a directory, so its procedure is
/// bound at it, as `gate_463_u1b` binds its source package): the fixture manifest with the
/// catalog task's procedure `procedure.observe` bound at `package`, as actingd binds a scheduled
/// procedure (`scheduled_execution.package_path`).
fn procedures_at(package: &Path) -> ProcedureManifest {
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
                binding.with_package_path(package.to_string_lossy())
            } else {
                binding
            }
        }),
    )
    .expect("procedure manifest")
}

/// Fix round 1: every publication of `health.application.update_required` in `events`, in ledger
/// order: its event's sequence and its value (as `gate_373_nav6`'s UPD-1 tests read them; a direct
/// run's host has the one instance).
fn update_publications(events: &[PersistedEvent]) -> Vec<(u64, bool)> {
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Fact(actingcommand_contract::FactPayload::Published(payload)) => {
                Some((event.sequence(), payload.record()))
            }
            _ => None,
        })
        .filter(|(_, record)| record.key == UPDATE_KEY)
        .filter_map(|(sequence, record)| match &record.content {
            FactContent::Inline {
                value: ContractFactValue::Boolean(value),
            } => Some((sequence, *value)),
            _ => None,
        })
        .collect()
}

/// A host on `clock` for a scheduled run of the fixture catalog's task on its instance, with the
/// game root `root` and the procedure manifest `procedures` (as `gate_373_nav6`'s B43-2 starts
/// one).
fn scheduled_host(
    temp: &TempDir,
    clock: &Arc<ManualRuntimeClock>,
    state: &Arc<FakeState>,
    root: &GameRoot,
    procedures: ProcedureManifest,
) -> RuntimeHost {
    state.physical_task_geometry.store(true, Ordering::Release);
    RuntimeHost::start(
        config(temp)
            .with_runtime_clock(clock.clone())
            .with_procedure_manifest(procedures)
            .with_standard_packs_root(Some(root.root.clone())),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(state),
        )),
    )
    .expect("runtime host")
}

/// A settled `contained_task_environment_reset` failure: rerun only (its identity's frame segment
/// is the decision's, so no later failure repeats it), class recoverable, count 1, scheduled for a
/// retry: nothing pauses the pair.
fn assert_rerun_only(failure: &PolicyFailureRecord, case: &str) {
    assert_eq!(
        scheduled_failure_code_str(&failure.outcome).as_str(),
        ENVIRONMENT_RESET,
        "{case}"
    );
    let identity = failure
        .failure_identity
        .as_deref()
        .and_then(FailureIdentity::parse)
        .unwrap_or_else(|| panic!("{case}: a failure identity: {:?}", failure.failure_identity));
    assert_eq!(identity.base, ENVIRONMENT_RESET, "{case}");
    assert!(!identity.accumulates(), "{case}: rerun only: {identity:?}");
    assert_eq!(
        failure.original_class,
        PolicyFailureClass::Recoverable,
        "{case}"
    );
    assert_eq!(
        failure.effective_class,
        PolicyFailureClass::Recoverable,
        "{case}"
    );
    assert_eq!(failure.consecutive_same_error, 1, "{case}");
    assert_eq!(
        failure.disposition,
        PolicyFailureDisposition::RetryScheduled,
        "{case}"
    );
}

/// The brief's first test (fix round 1: the closing decision's branch on frames that show the page
/// there, the scheduled part as its neighbours make it): a `linear_steps` run (one tap from `away`
/// to `home`, its arrival expected within 100 ms, no retry, so its closing decision follows) meets
/// the page of an entry without handlers (`result` `environment_reset`) after its tap: on the
/// arrival's frames, where the arrival's wait offers each frame to the shared interrupt pages, and
/// first at the closing decision, on U1's frames ([`notice_after_the_tap`]: the arrival's frames
/// and the decision's first frame black, its next frame the first to show the page), where the
/// host answers the decision with the end of the run and the kernel carries it out. Both times the
/// run ends with `contained_task_environment_reset` (error, `page_id`), neither
/// `contained_task_linear_unrecorded_screen` nor `page_confirmation_failed`, after the step's own
/// tap and nothing else: no `interrupt_handled`, no handler layer; at the closing decision the
/// frame that shows the page is taken once (the decision ends on it). Scheduled under the catalog
/// value that pauses a repeated failure, the procedure bound at the package's directory as
/// `gate_463_u1b` binds its own, the same run on the arrival's frames is settled rerun only, class
/// recoverable, a retry scheduled: the task is not suspended, and no ladder starts.
#[test]
fn int1_a_linear_run_meeting_a_page_without_handlers_in_a_wait_ends_by_its_entry() {
    for (case, closing) in [
        ("on the arrival's frames", false),
        ("first at the closing decision", true),
    ] {
        let state = Arc::new(FakeState::default());
        let temp = TempDir::new().expect("tempdir");
        let root = root_of(
            temp.path(),
            json!([without_handlers(Some("environment_reset"))]),
            false,
        );
        let package = temp.path().join("linear");
        write_linear_away_to_home(&package);
        let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
        let shown = notice_after_the_tap(&state, &clock, closing);
        let host = start_with(
            config(&temp).with_runtime_clock(clock.clone()),
            &state,
            Some(&root),
        );
        let ended = run_at(&host, &package);
        host.close().expect("close runtime host");

        assert_ended(&ended, ENVIRONMENT_RESET, Category::Error, case);
        assert_ne!(ended.failure().code().as_str(), UNRECORDED_SCREEN, "{case}");
        assert_ne!(
            ended.failure().code().as_str(),
            CONFIRMATION_FAILED,
            "{case}"
        );
        assert_eq!(
            state.input_count.load(Ordering::Acquire),
            1,
            "{case}: the step's own tap only"
        );
        assert_nothing_handled(&ended, case);
        if closing {
            assert_eq!(
                shown.load(Ordering::Acquire),
                1,
                "{case}: the decision ends on the first frame that shows the page"
            );
        }
    }

    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let root = root_of(
        temp.path(),
        json!([without_handlers(Some("environment_reset"))]),
        false,
    );
    let package = temp.path().join("linear");
    write_linear_away_to_home(&package);
    script(&state, |_, inputs| {
        if inputs == 0 {
            frame(RED, None)
        } else {
            frame(RED, Some(Overlay::Notice))
        }
    });
    let request = ContainedTaskRequest::new(package.to_string_lossy().into_owned())
        .expect("scheduled request");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = scheduled_host(&temp, &clock, &state, &root, procedures_at(&package));
    host.activate_policy_catalog(&pausing_policy_sources())
        .expect("activate policy catalog");
    let (context, error) = b43_failed_scheduled_run(&host, &clock, &request, POLICY_NOW_UNIX_MS, 7);
    let failure = b43_recorded_failure(&host, context.run_id());
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    host.close().expect("close runtime host");
    assert_eq!(error.code(), ENVIRONMENT_RESET);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert_rerun_only(&failure, "the scheduled linear run");
    assert!(
        !ladder_started(&events),
        "no default trigger reads the class"
    );
}

/// Fix round 1: an entry with a handler, its page seen first at a linear step's closing decision,
/// stays what #463 U1 made it there, recognised and not handled. On the first test's closing
/// frames ([`notice_after_the_tap`]) with the notice's entry naming its handler `close_notice`,
/// under `then: resume` and under `then: end_run` with the word `environment_reset` (which ends the
/// run where its page is handled, after the handler), the closing decision's verdict is
/// `interrupt_shown`: the run ends with `page_confirmation_failed` carrying `confirmation_problem`
/// `interrupt_shown`, after the step's own tap only, the frame that shows the page taken once, no
/// handler layer and nothing handled.
#[test]
fn int1_an_entry_with_a_handler_seen_at_the_closing_decision_stays_interrupt_shown() {
    let mut ending = interrupt("notice", CLOSE_NOTICE, json!("end_run"), 1);
    ending["result"] = json!("environment_reset");
    for (case, entry) in [
        (
            "resume",
            interrupt("notice", CLOSE_NOTICE, json!("resume"), 1),
        ),
        ("end_run with environment_reset", ending),
    ] {
        let state = Arc::new(FakeState::default());
        let temp = TempDir::new().expect("tempdir");
        let root = root_of(temp.path(), json!([entry]), true);
        let package = temp.path().join("linear");
        write_linear_away_to_home(&package);
        let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
        let shown = notice_after_the_tap(&state, &clock, true);
        let host = start_with(
            config(&temp).with_runtime_clock(clock.clone()),
            &state,
            Some(&root),
        );
        let ended = run_at(&host, &package);
        host.close().expect("close runtime host");

        assert_eq!(
            ended.receipt.state(),
            RuntimeReceiptState::Failed,
            "{case}: {:?}",
            ended.receipt
        );
        let failure = ended.failure();
        assert_eq!(failure.code().as_str(), CONFIRMATION_FAILED, "{case}");
        assert_eq!(
            text(&failure, "confirmation_problem"),
            Some("interrupt_shown"),
            "{case}: {failure:?}"
        );
        assert_eq!(
            state.input_count.load(Ordering::Acquire),
            1,
            "{case}: the step's own tap only"
        );
        assert_eq!(
            shown.load(Ordering::Acquire),
            1,
            "{case}: the decision ends on the first frame that shows the page"
        );
        assert_nothing_handled(&ended, case);
    }
}

/// Fix round 1: `application_update_required` on an entry without handlers, its page seen first at
/// a linear step's closing decision (the first test's closing frames, [`notice_after_the_tap`]):
/// the instance's `health.application.update_required` is published true once, before the run's
/// failed terminal, and the run ends with `contained_task_application_update_required` (warning,
/// `page_id`), in the order of the other places the page is met (an offer in a wait or at an
/// entry; `gate_373_nav6`'s UPD-1 A9 after a handler); after the step's own tap only, on the first
/// frame that shows the page, nothing handled and no handler layer.
#[test]
fn int1_an_update_page_without_handlers_at_the_closing_decision_records_its_fact_first() {
    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let root = root_of(
        temp.path(),
        json!([without_handlers(Some("application_update_required"))]),
        false,
    );
    let package = temp.path().join("linear");
    write_linear_away_to_home(&package);
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let shown = notice_after_the_tap(&state, &clock, true);
    let host = start_with(
        config(&temp).with_runtime_clock(clock.clone()),
        &state,
        Some(&root),
    );
    let ended = run_at(&host, &package);
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    host.close().expect("close runtime host");

    assert_ended(
        &ended,
        UPDATE_REQUIRED,
        Category::Warning,
        "at the closing decision",
    );
    assert_eq!(
        state.input_count.load(Ordering::Acquire),
        1,
        "the step's own tap only"
    );
    assert_eq!(
        shown.load(Ordering::Acquire),
        1,
        "the decision ends on the first frame that shows the page"
    );
    assert_nothing_handled(&ended, "at the closing decision");
    let publications = update_publications(&events);
    let [(published_at, true)] = publications.as_slice() else {
        panic!("one publication of true: {publications:?}");
    };
    let terminal = events
        .iter()
        .find(|event| event.event_type() == EventType::TaskFailed)
        .map(PersistedEvent::sequence)
        .expect("a failed task terminal");
    assert!(*published_at < terminal, "the fact before the terminal");
}

/// The brief's second test: the same page met at the run's entry check. A `linear_steps` run
/// (its entry page `away`) and a page-graph run with the entry set `{away}` each find the page on
/// every frame: the frame on which the entry check ended unmatched is offered, and the run ends
/// with `contained_task_environment_reset` before any input, nothing handled and no layer run.
#[test]
fn int1_the_page_met_at_the_entry_check_ends_the_run_before_any_input() {
    for (case, linear) in [("linear_steps", true), ("page-graph", false)] {
        let state = Arc::new(FakeState::default());
        let temp = TempDir::new().expect("tempdir");
        let root = root_of(
            temp.path(),
            json!([without_handlers(Some("environment_reset"))]),
            false,
        );
        script(&state, |_, _| frame(RED, Some(Overlay::Notice)));
        let ended = if linear {
            let package = temp.path().join("linear");
            write_linear_away_to_home(&package);
            let host = start_with(
                config(&temp).with_runtime_clock(Arc::new(ManualRuntimeClock::new(1_000, 0))),
                &state,
                Some(&root),
            );
            let ended = run_at(&host, &package);
            host.close().expect("close runtime host");
            ended
        } else {
            let host = start(&temp, &state, Some(&root));
            let ended = run(&host, &temp, "run", &entry_away_to_home(false));
            host.close().expect("close runtime host");
            ended
        };

        assert_ended(&ended, ENVIRONMENT_RESET, Category::Error, case);
        assert_eq!(
            state.input_count.load(Ordering::Acquire),
            0,
            "{case}: nothing pressed"
        );
        assert_nothing_handled(&ended, case);
    }
}

/// The brief's third test, and what `result` does on an entry without handlers: a page-graph run
/// whose first frame shows the page ends at once with nothing pressed and nothing handled, by its
/// entry's word: `environment_reset` with `contained_task_environment_reset` (error); no word with
/// `contained_task_interrupt_escalated` (error), as an entry with a handler ends today;
/// `application_update_required` with `contained_task_application_update_required` (warning).
/// `max_per_run` is 8 and the first meeting ends the run.
#[test]
fn int1_a_page_graph_run_ends_by_the_entry_s_result_with_nothing_pressed() {
    for (case, result, code, category) in [
        (
            "environment_reset",
            Some("environment_reset"),
            ENVIRONMENT_RESET,
            Category::Error,
        ),
        ("no result", None, ESCALATED, Category::Error),
        (
            "application_update_required",
            Some("application_update_required"),
            UPDATE_REQUIRED,
            Category::Warning,
        ),
    ] {
        let state = Arc::new(FakeState::default());
        let temp = TempDir::new().expect("tempdir");
        let root = root_of(temp.path(), json!([without_handlers(result)]), false);
        overlay_then_away_then_home(&state, 0, Overlay::Notice);
        let host = start(&temp, &state, Some(&root));
        let ended = run(&host, &temp, "run", &away_to_home());
        host.close().expect("close runtime host");

        assert_ended(&ended, code, category, case);
        assert_eq!(
            state.input_count.load(Ordering::Acquire),
            0,
            "{case}: nothing pressed"
        );
        assert_nothing_handled(&ended, case);
    }
}

/// The brief's fourth test, the Runtime's half: a page-graph package that declares `notice` itself
/// (its own page, with its own operation `dismiss` to `away`) is not ended by the entry without
/// handlers: it dismisses the page and goes on to `home` as before (two inputs, its own), and no
/// shared interrupt acts. `standard-pack check`'s warning for such a package is
/// `gate_373_navt`'s `int1_a_pack_declaring_a_page_without_handlers_is_warned_as_before`.
#[test]
fn int1_a_package_that_declares_the_page_itself_handles_it_as_before() {
    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let root = root_of(
        temp.path(),
        json!([without_handlers(Some("environment_reset"))]),
        false,
    );
    overlay_then_away_then_home(&state, 0, Overlay::Notice);
    let own_page = run_package(
        json!({
            "schema_version": "0.6",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "coordinate_space": {"width": 16, "height": 9},
            "entry_page": "away",
            "target_page": "home",
            "operations": [
                operation("dismiss", "notice", "away"),
                operation("go", "away", "home")
            ]
        }),
        &[("away", AWAY), ("home", HOME), ("notice", OWN_NOTICE)],
    );
    let host = start(&temp, &state, Some(&root));
    let ended = run(&host, &temp, "run", &own_page);
    host.close().expect("close runtime host");

    assert!(ended.succeeded(), "{:?}", ended.receipt);
    assert_eq!(state.input_count.load(Ordering::Acquire), 2);
    assert_nothing_handled(&ended, "a page of its own");
}

/// The rulings' test: under the catalog value that pauses a pair whose failure repeats, two
/// scheduled runs in a row meet the page of an entry without handlers (`environment_reset`) on
/// the same frame and end with `contained_task_environment_reset`, nothing pressed. Each failure
/// is rerun only (class recoverable, count 1, a retry scheduled), the second's identity is not the
/// first's, no ladder starts, and the next evaluation reruns the pair at once
/// (`failure_retry_immediate`) with no `policy_task_paused`. The code is of class
/// `budget_or_permission` and accumulates in no scope.
#[test]
fn int1_two_environment_resets_in_a_row_leave_the_pair_unpaused_and_rerun_at_once() {
    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let root = root_of(
        temp.path(),
        json!([without_handlers(Some("environment_reset"))]),
        false,
    );
    script(&state, |_, _| frame(RED, Some(Overlay::Notice)));
    let package_path = temp.path().join("scheduled.zip");
    fs::write(&package_path, away_to_home()).expect("write the scheduled package");
    let request = ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
        .expect("scheduled request");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = scheduled_host(
        &temp,
        &clock,
        &state,
        &root,
        procedure_manifest_with_primary(
            b"neutral interrupt fixture",
            vec!["after_observation".to_owned()],
        ),
    );
    host.activate_policy_catalog(&pausing_policy_sources())
        .expect("activate policy catalog");

    let (first_context, first_error) =
        b43_failed_scheduled_run(&host, &clock, &request, POLICY_NOW_UNIX_MS, 7);
    assert_eq!(first_error.code(), ENVIRONMENT_RESET);
    let first = b43_recorded_failure(&host, first_context.run_id());
    assert_rerun_only(&first, "first");

    let (second_context, second_error) = b43_failed_scheduled_run(
        &host,
        &clock,
        &request,
        b43_rerun_time(&first, &first_context),
        8,
    );
    assert_eq!(second_error.code(), ENVIRONMENT_RESET);
    let second = b43_recorded_failure(&host, second_context.run_id());
    assert_rerun_only(&second, "second");
    assert_ne!(
        second.failure_identity, first.failure_identity,
        "a repeat never carries the first identity"
    );
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    assert!(
        !ladder_started(&events),
        "no default trigger reads the class"
    );
    assert_eq!(
        state.input_count.load(Ordering::Acquire),
        0,
        "nothing pressed"
    );

    let at = b43_rerun_time(&second, &second_context).max(clock_now(&clock));
    advance_manual_clock_to(&clock, at);
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: at,
                monotonic_ms: at,
            },
            9,
            PolicyTrigger::FactsChanged,
        )
        .expect("evaluate the policy");
    host.close().expect("close runtime host");
    let evaluation = cycle.evaluation.expect("an evaluation");
    let paused = evaluation
        .decisions
        .iter()
        .filter(|decision| {
            decision.task_id == B43_TASK
                && decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS)
        })
        .flat_map(|decision| decision.reasons.iter())
        .any(|reason| reason.outcome.code().as_str() == "policy_task_paused");
    assert!(!paused, "no pause keeps the pair: {evaluation:#?}");
    let rerun = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == B43_TASK && intent.instance_id == POLICY_INSTANCE_ALIAS)
        .unwrap_or_else(|| panic!("the pair is dispatched again: {evaluation:#?}"));
    let chain = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == rerun.reason_chain_id)
        .expect("the rerun's reason chain");
    assert!(
        chain.reasons.iter().any(|reason| {
            reason
                .outcome
                .is(actingcommand_policy::PolicyCode::FailureRetryImmediate)
        }),
        "the rerun is the immediate rerun: {chain:#?}"
    );

    let code = HostCode::ContainedTaskEnvironmentReset;
    assert_eq!(code.as_str(), ENVIRONMENT_RESET);
    assert_eq!(code.ladder_class(), LadderClass::BudgetOrPermission);
    let code = CodeStr::from(code.code());
    for scope in ["prepare", "gate", "pre:0123abcd", "main"] {
        for restart_segment in [false, true] {
            assert!(
                !failure_accumulates(&code, scope, restart_segment),
                "{scope} {restart_segment}"
            );
        }
    }
}

/// The rulings' last test: the word on an entry with a handler. The handler runs as a layer of
/// the run (its tap the run's one input, its layer completed on `away`), one `interrupt_handled`
/// record follows (`then: end_run`, count 1), and the run ends with
/// `contained_task_environment_reset` (error, `page_id`), which accumulates in no scope.
#[test]
fn int1_the_word_on_an_entry_with_a_handler_ends_the_run_after_it_and_never_accumulates() {
    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let mut entry = interrupt("notice", CLOSE_NOTICE, json!("end_run"), 1);
    entry["result"] = json!("environment_reset");
    let root = root_of(temp.path(), json!([entry]), true);
    overlay_then_away_then_home(&state, 0, Overlay::Notice);
    let host = start(&temp, &state, Some(&root));
    let ended = run(&host, &temp, "run", &away_to_home());
    host.close().expect("close runtime host");

    assert_ended(&ended, ENVIRONMENT_RESET, Category::Error, "with a handler");
    assert_eq!(
        state.input_count.load(Ordering::Acquire),
        1,
        "the handler's tap only"
    );
    let handled = ended.handled();
    let [(page, handler, then, count, outcome)] = handled.as_slice() else {
        panic!("one handled interrupt: {handled:?}");
    };
    assert_eq!(page, "notice");
    assert_eq!(handler, CLOSE_NOTICE);
    assert_eq!(*then, TaskInterruptThen::EndRun);
    assert_eq!(*count, 1);
    assert_eq!(outcome.code().as_str(), "contained_task_interrupt_handled");
    let (admitted, completed, failed) = ended.handler_layers();
    assert_eq!(admitted, 1);
    assert_eq!(completed, ["neutral/away"]);
    assert!(failed.is_empty(), "{failed:?}");
    let code = CodeStr::from(HostCode::ContainedTaskEnvironmentReset.code());
    for scope in ["prepare", "gate", "pre:0123abcd", "main"] {
        for restart_segment in [false, true] {
            assert!(
                !failure_accumulates(&code, scope, restart_segment),
                "{scope} {restart_segment}"
            );
        }
    }
}

/// The brief's fifth test, the Runtime's reader (`package bundle` and `standard-pack check` are
/// `gate_373_navt`'s `int1_an_entry_without_handlers_is_read_on_end_run_only_by_both_tools`): an
/// empty `handlers` beside `then: "resume"` or `{"run": [...]}`, and `limit_cause` beside an empty
/// `handlers`, make the game root broken: the run is refused in its preparation
/// (`standard_pack_root_invalid`, `Denied`), with no task record and no input, and the start's
/// record of the root names the problem at its pointer (`packages_count` at
/// `/interrupts/0/handlers`; `interrupt_limit_cause_invalid` at `/interrupts/0/limit_cause`).
#[test]
fn int1_an_empty_handlers_beside_resume_or_run_or_a_limit_cause_refuses_the_run() {
    let mut with_limit = without_handlers(None);
    with_limit["limit_cause"] = json!("environment");
    for (case, entry, problem, pointer) in [
        (
            "resume",
            json!({"page": "notice", "handlers": [], "then": "resume", "max_per_run": 1}),
            "packages_count",
            "/interrupts/0/handlers",
        ),
        (
            "run",
            json!({"page": "notice", "handlers": [], "then": {"run": [CLOSE_NOTICE]},
                   "max_per_run": 1}),
            "packages_count",
            "/interrupts/0/handlers",
        ),
        (
            "limit_cause",
            with_limit,
            "interrupt_limit_cause_invalid",
            "/interrupts/0/limit_cause",
        ),
    ] {
        let state = Arc::new(FakeState::default());
        let temp = TempDir::new().expect("tempdir");
        let root = root_of(temp.path(), json!([entry]), true);
        let host = start(&temp, &state, Some(&root));
        let refused = run(&host, &temp, "run", &away_to_home());
        let reads = standard_pack_reads(&host);
        host.close().expect("close runtime host");

        assert_eq!(
            refused.receipt.state(),
            RuntimeReceiptState::Denied,
            "{case}"
        );
        assert_eq!(
            refused.receipt.outcome().code().as_str(),
            ROOT_INVALID,
            "{case}"
        );
        assert!(refused.facts().is_empty(), "{case}: no task record");
        assert_eq!(state.input_count.load(Ordering::Acquire), 0, "{case}");
        let [invalid] = reads.as_slice() else {
            panic!("{case}: one record of the game root: {reads:?}");
        };
        assert_eq!(invalid.code().as_str(), ROOT_INVALID, "{case}");
        assert_eq!(
            text(invalid, "standard_pack_problem"),
            Some(problem),
            "{case}"
        );
        assert_eq!(text(invalid, "json_pointer"), Some(pointer), "{case}");
    }
}
