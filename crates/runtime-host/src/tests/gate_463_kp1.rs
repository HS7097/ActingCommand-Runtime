// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 KP-1 (register §1.448; contracts/linear-steps.md, "Key steps", "A key the
// instance's input does not send"), the host's half, fix round 1 (`rd-1116/brief-463-kp1-fix1.md`,
// the independent review's point: the host's `key_input_support` had no test): a `linear_steps`
// package whose second step presses the back key runs directly through the host on an instance
// whose capability table says its input does not implement keys (the row `input.key`
// unsupported, as the registry declares it for every input but MaaTouch). The run is refused with
// `contained_task_linear_key_unsupported` naming the key step, and the ledger holds no input and
// no effect intent: nothing was sent and nothing was intended.
//
// The package is a `linear_steps` source package of game `neutral`, server `test`, 2 x 1: `home`
// is a red first pixel, `list` a blue one and `done` a white one; a green second pixel is the key
// step's guard mark. `open` taps from `home` into `list`; `back` presses the back key from `list`
// to `done`. The run is on the test's manual clock (test plan E-3).

use super::instance_workers::all_events;
use super::*;
use actingcommand_contract::{
    EmulatorCapability, EmulatorCapabilityAvailability, EmulatorCapabilityEvidence,
    EmulatorCapabilityImplementation, EmulatorCapabilityProfile, EmulatorVersionEvidence,
    EmulatorVersionUnavailableReason,
};
use actingcommand_execution_kernel::ResolvedExecutionInstance;
use serde_json::json;

const KEY_UNSUPPORTED: &str =
    actingcommand_execution_kernel::ExecutionKernelCode::ContainedTaskLinearKeyUnsupported.as_str();

// Writes the package as a source package directory at `directory`: the tap `open` from `home` to
// `list`, then the key step `back` from `list` to `done`, guarded by the mark; each expected within
// 200 ms, no retry. The key train's test (`gate_463_train_key`) runs it too.
pub(super) fn write_package(directory: &Path) {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.kp1.back",
        "execution_mode": "linear_steps",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 50,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    let probe = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"id": id,
               "region": {"mode": "rect", "rect": {"x": x, "y": 0, "width": 1, "height": 1}},
               "expected": expected})
    };
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "KP-1 fixture",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [
            probe("first_red", 0, [255, 0, 0]),
            probe("first_blue", 0, [0, 0, 255]),
            probe("first_white", 0, [255, 255, 255]),
            probe("second_green", 1, [0, 255, 0])
        ],
        "page_rules": {
            "home": {"required": ["first_red"]},
            "list": {"required": ["first_blue"]},
            "done": {"required": ["first_white"]}
        },
        "entry_page": "home",
        "target_page": "done",
        "max_steps": 2,
        "scheduling_outcome": {"mappings": [
            {"outcome_key": "task_back", "effect": "no_designated_effect",
             "terminal_pages": ["done"]}
        ]},
        "operations": [
            {"id": "open", "from": "home", "to": "list",
             "click": {"kind": "point", "x": 1, "y": 0},
             "unguarded_trusted_coordinate": true,
             "expect_after": {"page_id": "list", "timeout_ms": 200}},
            {"id": "back", "from": "list", "to": "done",
             "key": {"key": "back"},
             "guard": {"page_id": "list", "target_id": "second_green",
                       "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                       "color_probe": "second_green"},
             "expect_after": {"page_id": "done", "timeout_ms": 200}}
        ]
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

// The capability table of an instance whose configured input sends no key, in the registry's form
// (`capability_profile` in `provider.rs`) for an input other than MaaTouch: `input.key` and
// `input.text` unsupported and unavailable, every other row supported and unverified.
fn table_without_key() -> EmulatorCapabilityProfile {
    let evidence = EmulatorCapability::ALL
        .into_iter()
        .map(|capability| {
            let (implementation, availability) = if matches!(
                capability,
                EmulatorCapability::InputKey | EmulatorCapability::InputText
            ) {
                (
                    EmulatorCapabilityImplementation::Unsupported,
                    EmulatorCapabilityAvailability::Unavailable,
                )
            } else {
                (
                    EmulatorCapabilityImplementation::Supported,
                    EmulatorCapabilityAvailability::Unverified,
                )
            };
            EmulatorCapabilityEvidence::new(
                capability,
                availability,
                "neutral test row",
                "minitouch",
            )
            .and_then(|row| row.with_implementation(implementation))
            .expect("a capability row")
        })
        .collect::<Vec<_>>();
    EmulatorCapabilityProfile::new(
        "runtime.execution_backend_registry",
        EmulatorVersionEvidence::Unavailable {
            reason: EmulatorVersionUnavailableReason::NotProbed,
        },
        evidence,
    )
    .expect("a capability table")
}

// The run, sent as `actingctl task-run` sends it, on the device instance whose table says its input
// sends no key: it ends Failed with the code, its receipt and its failed terminal naming `back`
// (`pack_operation_id`). The device took no input, and the ledger holds no input record (intent,
// commit or failure) and no effect intent of the run.
#[test]
fn kp1_fix1_an_input_without_the_key_refuses_the_run_with_no_input_and_no_intent() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("kp1-back");
    write_package(&package);
    let state = Arc::new(FakeState::default());
    // A direct run needs a device instance whose display is observed (Workflow #392 DA-A).
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((2, 1));
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let resolved = ResolvedExecutionInstance::new(instance_id(), "127.0.0.1:16384")
        .with_capabilities(table_without_key());
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock),
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id(), Arc::clone(&state))
                .with_resolved_for(POLICY_INSTANCE_ALIAS, resolved),
        ),
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

    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    let terminals = events
        .iter()
        .filter(|event| event.event_type() == EventType::TaskFailed)
        .filter_map(|event| match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                TaskSemanticFact::TerminalCommitted {
                    outcome: Some(outcome),
                    ..
                } => Some(outcome.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    let [terminal] = terminals.as_slice() else {
        panic!("one failed terminal with its outcome: {terminals:?}");
    };
    for (carrier, outcome) in [("terminal", terminal), ("receipt", receipt.outcome())] {
        assert_eq!(outcome.code().as_str(), KEY_UNSUPPORTED, "{carrier}");
        assert_eq!(
            outcome.value("pack_operation_id").cloned(),
            Some(json!("back")),
            "{carrier}"
        );
    }

    assert_eq!(inputs, 0, "the device took an input");
    let input_records = events
        .iter()
        .map(PersistedEvent::event_type)
        .filter(|kind| {
            matches!(
                kind,
                EventType::InputIntent | EventType::InputCommitted | EventType::InputFailed
            )
        })
        .collect::<Vec<_>>();
    assert!(input_records.is_empty(), "{input_records:?}");
    let intents = events
        .iter()
        .filter(|event| {
            matches!(
                event.payload(),
                EventPayload::Task(TaskPayload::Semantic(payload))
                    if matches!(payload.fact(), TaskSemanticFact::EffectIntent { .. })
            )
        })
        .count();
    assert_eq!(intents, 0, "an effect intent was written");
}
