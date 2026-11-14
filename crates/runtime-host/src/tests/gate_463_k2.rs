// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 K2 (register §1.380; contracts/linear-steps.md, "Endings"), the host's half: a
// `linear_steps` package whose one step `buy` leads from `shop` to its target page `done`, and whose
// scheduling outcome names the ending page `short` beside it, runs directly through the host. After
// the tap the screen shows `short`: the kernel finishes the run Completed on it with no key of its
// own, and the host derives the terminal's scheduling disposition from the one mapping of that page,
// the ending's key. After the same tap the screen shows `done`: the usual ending keeps its own key.
// The host's rule is unchanged; this pins what it gives an ending.
//
// The package is a `linear_steps` source package of game `neutral`, server `test`, 2 x 1: `shop`
// is a red first pixel (as the fake device shows it before any input), `done` a blue one and `short`
// a white one. The run is on the test's manual clock (test plan E-3: the kernel waits on it, a
// capture spends no time).

use super::instance_workers::all_events;
use super::*;
use serde_json::json;

const BOUGHT: &str = "task_bought";
const SHORT_BALANCE: &str = "task_short_balance";

// Writes the package as a source package directory at `directory`: one tap `buy` from `shop` to
// `done`, expected within 200 ms, no retry; the mappings of `done` and of the ending page `short`.
fn write_package(directory: &Path) {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.k2.buy",
        "execution_mode": "linear_steps",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 50,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 1
    });
    let probe = |id: &str, expected: [u8; 3]| {
        json!({"id": id,
               "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
               "expected": expected})
    };
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "K2 fixture",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [
            probe("first_red", [255, 0, 0]),
            probe("first_blue", [0, 0, 255]),
            probe("first_white", [255, 255, 255])
        ],
        "page_rules": {
            "shop": {"required": ["first_red"]},
            "done": {"required": ["first_blue"]},
            "short": {"required": ["first_white"]}
        },
        "entry_page": "shop",
        "target_page": "done",
        "max_steps": 1,
        "scheduling_outcome": {"mappings": [
            {"outcome_key": BOUGHT, "effect": "no_designated_effect", "terminal_pages": ["done"]},
            {"outcome_key": SHORT_BALANCE, "effect": "no_designated_effect",
             "terminal_pages": ["short"]}
        ]},
        "operations": [{
            "id": "buy",
            "from": "shop",
            "to": "done",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "expect_after": {"page_id": "done", "timeout_ms": 200}
        }]
    });
    for (path, value) in [
        ("control.json", control),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        ("resources/operations/task/task.json", task),
    ] {
        let path = directory.join(path);
        fs::create_dir_all(path.parent().expect("package file parent")).expect("package dir");
        fs::write(
            path,
            serde_json::to_vec_pretty(&value).expect("package JSON"),
        )
        .expect("write package file");
    }
}

// One direct run: its receipt, the host's ledger and the inputs the device took.
struct DirectRun {
    receipt: RuntimeReceipt,
    events: Vec<PersistedEvent>,
    inputs: usize,
}

// Runs the package directly on a device instance whose first pixel turns `after` once the tap is
// made.
fn run_with_screen_after_tap(after: [u8; 3]) -> DirectRun {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("k2-buy");
    write_package(&package);
    let state = Arc::new(FakeState::default());
    *state.pixel_script.lock().expect("pixel script") =
        Some(Arc::new(move |_: usize, inputs: usize| {
            if inputs == 0 {
                Vec::<(usize, [u8; 3])>::new()
            } else {
                vec![(0_usize, after)]
            }
        }));
    // A direct run needs a device instance whose display is observed (Workflow #392 DA-A).
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((2, 1));
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let run = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            POLICY_INSTANCE_ALIAS,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.to_string_lossy().into_owned())
                .expect("task request"),
        ),
    );
    let receipt = host
        .process_request_for_test(&run, ConnectionId::new(463).expect("connection"))
        .expect("run receipt");
    let events = all_events(&host);
    let inputs = state.input_count.load(Ordering::Acquire);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close runtime host");
    DirectRun {
        receipt,
        events,
        inputs,
    }
}

// B.2, end to end: after the tap the screen shows the ending page `short`, then, in a second run,
// the target page `done`. Each run is Completed after its one step and one tap on the page shown,
// and its terminal's scheduling disposition carries that page's mapping's key, with no designated
// effect: the ending's key for `short`, the usual key for `done`.
#[test]
fn k2_the_host_keys_a_run_that_ends_on_an_ending_by_its_mapping() {
    for (case, after, page, key) in [
        (
            "the ending",
            [255_u8, 255, 255],
            "neutral/short",
            SHORT_BALANCE,
        ),
        ("the usual ending", [0_u8, 0, 255], "neutral/done", BOUGHT),
    ] {
        let DirectRun {
            receipt,
            events,
            inputs,
        } = run_with_screen_after_tap(after);

        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{case}: {:?}",
            receipt.outcome()
        );
        assert!(
            matches!(
                receipt.result(),
                Some(RuntimeResult::ContainedTaskCompleted {
                    task_result: TaskOutcome::Success,
                    final_page: Some(final_page),
                    executed_steps: 1,
                    ..
                }) if final_page == page
            ),
            "{case}: {:?}",
            receipt.result()
        );
        assert_eq!(inputs, 1, "{case}");
        let terminals = events
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                    TaskSemanticFact::TerminalCommitted {
                        task_result,
                        final_page,
                        scheduling_disposition,
                        ..
                    } => Some((
                        *task_result,
                        final_page.as_deref(),
                        scheduling_disposition.as_ref(),
                    )),
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>();
        let [(task_result, final_page, disposition)] = terminals.as_slice() else {
            panic!("{case}: one terminal, got {}", terminals.len());
        };
        assert_eq!(*task_result, TaskOutcome::Success, "{case}");
        assert_eq!(*final_page, Some(page), "{case}");
        let disposition = disposition.expect("a scheduling disposition");
        assert_eq!(disposition.outcome_key(), key, "{case}");
        assert!(
            matches!(
                disposition.effect(),
                SchedulingEffectEvidence::NoDesignatedEffect
            ),
            "{case}: {:?}",
            disposition.effect()
        );
    }
}
