// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 K1 (register §1.374; contracts/linear-steps.md, "Select steps"), the host's half: a
// `linear_steps` package whose one step is a single-frame select over fixed slots, with
// `on_empty`, its page mapped to the `on_empty` key, runs directly through the host. Nobody the
// policy lists is on the list and nothing is unknown, so the step's empty decision ends the run
// Completed on its own page with no input: the kernel records the step's start, its decision and
// its finish on that page; the host derives the terminal's scheduling disposition from the one
// mapping of that page, finds the kernel's key, and refuses nothing.
//
// The list screen and the card reader are the SEL-L4b host gate's (`gate_308_l4b`): a 64 x 96
// frame with one column of six cards, card k naming `member-0k`; pixel (0, 0) is red on the list
// page and (3, 0) white, the step's guard. The policy lists only `member-06`, which no card shows.

use super::gate_308_l4b::{
    ALIAS, CardNames, HEIGHT, WIDTH, list_screen, member, table_read_identities,
};
use super::*;
use serde_json::{Value, json};

const ON_EMPTY_KEY: &str = "nobody_listed";

// Card k's slot: the rectangles of card k on the list screen, its anchor at (4, 10 + 12 k).
fn card_slot(card: u32) -> Value {
    let top = 10 + 12 * card;
    json!({"rect": {"x": 4, "y": top, "width": 16, "height": 8},
           "click": {"x": 6, "y": top + 1, "width": 4, "height": 4},
           "targets": {}})
}

// The linear package: its one step `pick` from the list (the entry) to the detail page, a select
// over the six cards as fixed slots whose identities are read by OCR, with `on_empty`; the
// scheduling outcome maps the list to the `on_empty` key and the detail page to another key.
fn linear_select_package() -> Vec<u8> {
    let identities = table_read_identities();
    let space = json!({"width": WIDTH, "height": HEIGHT});
    let layout = json!({
        "id": "layout/cards", "page_id": "neutral/list", "kind": "fixed_slots",
        "unknown_identity": "reject",
        "features": [{
            "name": "member", "value": "identity", "target": "card/name",
            "offset": {"x": 0, "y": 0},
            "identity": {
                "entries": identities.iter()
                    .map(|id| json!({"id": id, "aliases": [id]}))
                    .collect::<Vec<_>>(),
                "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                                "minimum_confidence_milli": 800}}
        }],
        "slots": (0..6).map(card_slot).collect::<Vec<_>>()
    });
    let policy = json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "cards-listed",
        "applies_to": {
            "candidate_layout_id": "layout/cards",
            "outcome_keys": {"selected": "card_selected", "empty": "card_empty",
                             "insufficient": "card_insufficient",
                             "ambiguous": "card_ambiguous", "unknown": "card_unknown"}
        },
        "fields": [{"name": "member",
                    "value_type": {"type": "enum_string", "allowed": identities}}],
        "facts": [],
        "gates": [{"gate_id": "listed",
                   "predicate": {"kind": "string_in",
                                 "value": {"source": "field", "field": "member"},
                                 "allowed": [member(6)]},
                   "on_unknown": {"kind": "drop_candidate"}}],
        "scoring": [],
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    });
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.linear.select",
        "execution_mode": "linear_steps",
        "game": "neutral",
        "server": "test",
        "resolution": space,
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 10_000,
        "timeout_ms": 40_000,
        "max_steps": 1
    });
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": space,
        "entry_page": "list",
        "target_page": "detail",
        "max_steps": 1,
        "scheduling_outcome": {"mappings": [
            {"outcome_key": ON_EMPTY_KEY, "effect": "no_designated_effect",
             "terminal_pages": ["list"]},
            {"outcome_key": "member_opened", "effect": "no_designated_effect",
             "terminal_pages": ["detail"]}
        ]},
        "candidate_layouts": [layout],
        "operations": [{
            "id": "pick",
            "from": "list",
            "to": "detail",
            "expect_after": {"page_id": "detail"},
            "guard": {
                "page_id": "list",
                "target_id": "guard/list",
                "expected_rect": {"x": 3, "y": 0, "width": 1, "height": 1},
                "color_probe": "guard/list"
            },
            "select": {"layout_id": "layout/cards", "policy": {"path": "policies/listed.json"},
                       "on_empty": {"outcome_key": ON_EMPTY_KEY}}
        }]
    });
    let color = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id, "region": {"x": x, "y": 0, "width": 1, "height": 1},
               "expected": expected})
    };
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": space,
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            color("page/list", 0, [255, 0, 0]),
            color("page/detail", 0, [0, 0, 255]),
            color("guard/list", 3, [255, 255, 255]),
            {"type": "ocr", "id": "card/name",
             "region": {"x": 0, "y": 0, "width": 12, "height": 4},
             "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact",
             "expected": [member(0)], "case_sensitive": true, "minimum_confidence": 0.8,
             "model_ref": "PP-OCRv6_medium", "model_sha256": "a".repeat(64)}
        ],
        "candidate_layouts": [layout]
    });
    let pages = json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/list", "required": ["page/list"], "optional": [], "forbidden": []},
            {"id": "neutral/detail", "required": ["page/detail"], "optional": [],
             "forbidden": []}
        ]
    });
    let bytes = |value: &Value| serde_json::to_vec(value).expect("package JSON");
    let entries = [
        ("control.json", bytes(&control)),
        (
            "resources/manifest.json",
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        ("resources/operations/task/task.json", bytes(&task)),
        (
            "resources/operations/task/policies/listed.json",
            bytes(&policy),
        ),
        ("resources/recognition/neutral.test.pack.json", bytes(&pack)),
        (
            "resources/recognition/neutral.test.pages.json",
            bytes(&pages),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, content) in entries {
        zip.start_file(path, options).expect("package entry");
        zip.write_all(&content).expect("package content");
    }
    zip.finish().expect("finish package").into_inner()
}

// A.3, end to end: every card is read and none is listed, so the decision is `empty` with nothing
// unknown, and the step's `on_empty` ends the run Completed on the list after one step, with no
// input. The ledger holds the step's start, its one decision record and its finish on the list,
// and no effect intent or completed effect; the terminal's scheduling disposition carries the
// `on_empty` key with no designated effect.
#[test]
fn k1_a_linear_select_that_ends_empty_gets_its_on_empty_key_from_the_host() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("linear-select.zip");
    fs::write(&package, linear_select_package()).expect("write the package");
    let state = Arc::new(FakeState::default());
    // A direct run needs a device instance whose display is observed (Workflow #392 DA-A): the
    // fake device observes a display of the package's resolution and shows the list on it.
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((WIDTH, HEIGHT));
    let screen = list_screen();
    *state.pixel_script.lock().expect("pixel script") =
        Some(Arc::new(move |_: usize, _: usize| screen.clone()));
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one(ALIAS, instance_id(), Arc::clone(&state))
                .with_vision_provider(Arc::new(CardNames)),
        ),
    )
    .expect("runtime host");

    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            ALIAS,
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string()).expect("task request"),
        ),
    );
    let receipt = client.send(&request);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "the run's outcome: {:?}; the Runtime's fatal error: {:?}",
        receipt.outcome(),
        host.fatal_error()
    );
    assert!(
        matches!(
            receipt.result(),
            Some(RuntimeResult::ContainedTaskCompleted {
                task_result: TaskOutcome::Success,
                final_page: Some(page),
                executed_steps: 1,
                ..
            }) if page == "neutral/list"
        ),
        "{:?}",
        receipt.result()
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);

    let mut events = host
        .query_persisted_events_for_test(EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        })
        .expect("the run's records");
    events.sort_by_key(PersistedEvent::sequence);
    let facts = events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => Some(payload.fact()),
            _ => None,
        })
        .collect::<Vec<_>>();

    // The step: its start, its decision and its finish on the list, and no effect.
    let step = facts
        .iter()
        .filter_map(|fact| match fact {
            TaskSemanticFact::StepStarted {
                operation_label, ..
            } => Some(("step_started", operation_label.as_str())),
            TaskSemanticFact::SelectionEvaluated {
                operation_label, ..
            } => Some(("selection_evaluated", operation_label.as_str())),
            TaskSemanticFact::EffectIntent {
                operation_label, ..
            } => Some(("effect_intent", operation_label.as_str())),
            TaskSemanticFact::EffectCompleted {
                operation_label, ..
            } => Some(("effect_completed", operation_label.as_str())),
            TaskSemanticFact::StepFinished {
                operation_label, ..
            } => Some(("step_finished", operation_label.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        step,
        [
            ("step_started", "pick"),
            ("selection_evaluated", "pick"),
            ("step_finished", "pick")
        ]
    );
    let finished = facts
        .iter()
        .find_map(|fact| match fact {
            TaskSemanticFact::StepFinished {
                step_index,
                page_label,
                ..
            } => Some((*step_index, page_label.as_str())),
            _ => None,
        })
        .expect("the step's finish");
    assert_eq!(finished, (0, "neutral/list"));

    // The terminal: Completed on the list after one step, its disposition the `on_empty` key with
    // no designated effect.
    let terminals = facts
        .iter()
        .filter_map(|fact| match fact {
            TaskSemanticFact::TerminalCommitted {
                task_result,
                final_page,
                executed_steps,
                scheduling_disposition,
                ..
            } => Some((
                *task_result,
                final_page.as_deref(),
                *executed_steps,
                scheduling_disposition.as_ref(),
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(task_result, final_page, executed_steps, disposition)] = terminals.as_slice() else {
        panic!("one terminal, got {}", terminals.len());
    };
    assert_eq!(*task_result, TaskOutcome::Success);
    assert_eq!(*final_page, Some("neutral/list"));
    assert_eq!(*executed_steps, Some(1));
    let disposition = disposition.expect("a scheduling disposition");
    assert_eq!(disposition.outcome_key(), ON_EMPTY_KEY);
    assert!(
        matches!(
            disposition.effect(),
            SchedulingEffectEvidence::NoDesignatedEffect
        ),
        "{:?}",
        disposition.effect()
    );

    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close the host");
}
