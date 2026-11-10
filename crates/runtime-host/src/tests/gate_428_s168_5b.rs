// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #428 S168-5b (register §1.291, its seven named tests; contracts/selection-graph.md,
// "Judge step"), the host's halves of tests 1, 2 and 5. A package's judge step evaluates three
// fixed slots over the neutral list of the SEL-L4b host gate (`gate_308_l4b`): a 64 x 96 frame
// whose cards 0 to 2 are the slots, each read for its name `member-0k` by the scripted OCR
// provider; pixel (0, 0) is red on the list page. The step records its decision with
// `judge_only` and ends the run Completed on the list page with one of its two keys, which the
// host maps through the mapping whose `judge_result` that key names. A fake device paints the
// list; it counts every input it is handed. The ids are made up.

use super::gate_308_l4b::{ALIAS, CardNames, HEIGHT, WIDTH, list_screen, member};
use super::*;
use actingcommand_contract::TaskSelectionConfirmation;

// The second instance (`TESTSERIAL0002` in the register's words).
const OTHER_ALIAS: &str = "neutral.other";
const JUDGED: &str = "choices_judged";
const UNKNOWN: &str = "choices_unknown";
const JUDGE_INPUT_REFUSED: &str = "contained_task_judge_input_refused";
const SLOTS: u32 = 3;

// One device instance: a display of the package's resolution observed (Workflow #392 DA-A),
// showing the list.
fn device() -> Arc<FakeState> {
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((WIDTH, HEIGHT));
    let screen = list_screen();
    *state.pixel_script.lock().expect("pixel script") =
        Some(Arc::new(move |_: usize, _: usize| screen.clone()));
    state
}

fn start(root: &TempDir, devices: &[(&str, &Arc<FakeState>)]) -> RuntimeHost {
    let provider = FakeProvider::from_entries(
        devices
            .iter()
            .map(|(alias, state)| ((*alias).to_owned(), instance_id(), Arc::clone(state))),
    )
    .with_vision_provider(Arc::new(CardNames));
    RuntimeHost::start(config(root), Arc::new(provider)).expect("runtime host")
}

// Card k's slot: the rectangles a list card has at its anchor, (4, 10 + 12 k).
fn slot(card: u32) -> serde_json::Value {
    let top = 10 + 12 * card;
    serde_json::json!({"rect": {"x": 4, "y": top, "width": 16, "height": 8},
                       "click": {"x": 6, "y": top + 1, "width": 4, "height": 4},
                       "targets": {}})
}

// The package: one judge step `judge_choices` on the list page over the first three cards as
// fixed slots, whose `member` identity holds `identities`; its document judges worth the members
// `worth` and chooses every survivor. The scheduling outcome maps the judge's two keys on the list
// page by `judge_result`, and the target page to another key.
fn judge_package(identities: &[String], worth: &[String]) -> Vec<u8> {
    let space = serde_json::json!({"width": WIDTH, "height": HEIGHT});
    let layout = serde_json::json!({
        "id": "layout/slots", "page_id": "neutral/list", "kind": "fixed_slots",
        "unknown_identity": "reject",
        "features": [{
            "name": "member", "value": "identity", "target": "card/name",
            "offset": {"x": 0, "y": 0},
            "identity": {
                "entries": identities.iter()
                    .map(|id| serde_json::json!({"id": id, "aliases": [id]}))
                    .collect::<Vec<_>>(),
                "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                                "minimum_confidence_milli": 800}}
        }],
        "slots": (0..SLOTS).map(slot).collect::<Vec<_>>()
    });
    let policy = serde_json::json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "choices",
        "applies_to": {
            "candidate_layout_id": "layout/slots",
            "outcome_keys": {"selected": "choices_selected", "empty": "choices_empty",
                             "insufficient": "choices_insufficient",
                             "ambiguous": "choices_ambiguous",
                             "unknown": "choices_evaluated_unknown"}
        },
        "fields": [{"name": "member",
                    "value_type": {"type": "enum_string", "allowed": identities}}],
        "facts": [],
        "gates": [{"gate_id": "worth",
                   "predicate": {"kind": "string_in",
                                 "value": {"source": "field", "field": "member"},
                                 "allowed": worth},
                   "on_unknown": {"kind": "drop_candidate"}}],
        "scoring": [],
        "selection": {"mode": "none_allowed", "required_count": SLOTS},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    });
    let control = serde_json::json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.judge.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": space,
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 10_000,
        "timeout_ms": 40_000,
        "max_steps": 2
    });
    let task = serde_json::json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": space,
        "target_page": "detail",
        "scheduling_outcome": {"mappings": [
            {"outcome_key": JUDGED, "effect": "no_designated_effect",
             "terminal_pages": ["list"], "judge_result": "judged"},
            {"outcome_key": UNKNOWN, "effect": "no_designated_effect",
             "terminal_pages": ["list"], "judge_result": "unknown"},
            {"outcome_key": "detail_reached", "effect": "no_designated_effect",
             "terminal_pages": ["detail"]}
        ]},
        "candidate_layouts": [layout],
        "operations": [{
            "id": "judge_choices",
            "from": "list",
            "to": "list",
            "judge": {"layout_id": "layout/slots", "policy": {"path": "policies/judge.json"},
                      "outcome_key": JUDGED, "unknown_outcome_key": UNKNOWN}
        }]
    });
    let color = |id: &str, expected: [u8; 3]| {
        serde_json::json!({"type": "color", "id": id,
                           "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                           "expected": expected})
    };
    let pack = serde_json::json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": space,
        "defaults": {"match_metric": "ccoeff_normed", "color_max_distance": 0.0},
        "targets": [
            color("page/list", [255, 0, 0]),
            color("page/detail", [0, 0, 255]),
            {"type": "ocr", "id": "card/name",
             "region": {"x": 0, "y": 0, "width": 12, "height": 4},
             "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact",
             "expected": [member(0)], "case_sensitive": true, "minimum_confidence": 0.8,
             "model_ref": "PP-OCRv6_medium", "model_sha256": "a".repeat(64)}
        ],
        "candidate_layouts": [layout]
    });
    let pages = serde_json::json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/list", "required": ["page/list"], "optional": [], "forbidden": []},
            {"id": "neutral/detail", "required": ["page/detail"], "optional": [],
             "forbidden": []}
        ]
    });
    let json = |value: serde_json::Value| serde_json::to_vec(&value).expect("package JSON");
    let entries = [
        ("control.json", json(control)),
        (
            "resources/manifest.json",
            json(serde_json::json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        ("resources/operations/task/task.json", json(task)),
        (
            "resources/operations/task/policies/judge.json",
            json(policy),
        ),
        ("resources/recognition/neutral.test.pack.json", json(pack)),
        ("resources/recognition/neutral.test.pages.json", json(pages)),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, bytes) in entries {
        zip.start_file(path, options).expect("package entry");
        zip.write_all(&bytes).expect("package content");
    }
    zip.finish().expect("finish package").into_inner()
}

// Every member the list shows and one more, `member-00` and `member-01` judged worth.
fn known_package() -> Vec<u8> {
    judge_package(
        &(0..=6).map(member).collect::<Vec<_>>(),
        &[member(0), member(1)],
    )
}

fn write(root: &TempDir, name: &str, bytes: Vec<u8>) -> PathBuf {
    let path = root.path().join(name);
    fs::write(&path, bytes).expect("write the package");
    path
}

// One manual run of `package` on `alias`; its receipt and correlation.
fn run(host: &RuntimeHost, alias: &str, package: &Path) -> (RuntimeReceipt, CorrelationId) {
    let mut client = TestClient::connect(host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            alias,
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string()).expect("task request"),
        ),
    );
    (client.send(&request), correlation_id)
}

// The semantic task facts of the run `correlation_id`, in ledger order.
fn task_facts(host: &RuntimeHost, correlation_id: CorrelationId) -> Vec<TaskSemanticFact> {
    let mut events = host
        .query_persisted_events_for_test(EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        })
        .expect("the run's records");
    events.sort_by_key(PersistedEvent::sequence);
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => Some(payload.fact().clone()),
            _ => None,
        })
        .collect()
}

// The judge step's facts of a step's life, in order.
fn step_kinds(facts: &[TaskSemanticFact]) -> Vec<&'static str> {
    facts
        .iter()
        .filter_map(|fact| match fact {
            TaskSemanticFact::StepStarted { .. } => Some("step_started"),
            TaskSemanticFact::SelectionEvaluated { .. } => Some("selection_evaluated"),
            TaskSemanticFact::EffectIntent { .. } => Some("effect_intent"),
            TaskSemanticFact::EffectCompleted { .. } => Some("effect_completed"),
            TaskSemanticFact::StepFinished { .. } => Some("step_finished"),
            _ => None,
        })
        .collect()
}

// The run ended Completed on the list page after one step, the judge's record `judge_only`, and
// its terminal's scheduling disposition is `key` with no designated effect.
fn assert_judge_end(
    host: &RuntimeHost,
    receipt: &RuntimeReceipt,
    correlation_id: CorrelationId,
    key: &str,
) {
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
    let facts = task_facts(host, correlation_id);
    assert_eq!(
        step_kinds(&facts),
        ["step_started", "selection_evaluated", "step_finished"]
    );
    let selection = facts
        .iter()
        .find_map(|fact| match fact {
            TaskSemanticFact::SelectionEvaluated { selection, .. } => Some(selection),
            _ => None,
        })
        .expect("the judge's record");
    assert_eq!(selection.confirmation, TaskSelectionConfirmation::JudgeOnly);
    let dispositions = facts
        .iter()
        .filter_map(|fact| match fact {
            TaskSemanticFact::TerminalCommitted {
                scheduling_disposition,
                ..
            } => Some(scheduling_disposition.as_ref()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [Some(disposition)] = dispositions.as_slice() else {
        panic!("one terminal with a scheduling disposition: {dispositions:?}");
    };
    assert_eq!(disposition.outcome_key(), key);
    assert!(
        matches!(
            disposition.effect(),
            SchedulingEffectEvidence::NoDesignatedEffect
        ),
        "{:?}",
        disposition.effect()
    );
}

// S168-5b test 1, the host's half: the judge over three known slots, two of them worth, ends the
// run Completed on the list page with the step's `outcome_key`, which the host maps through the
// mapping `judged`; the device is handed no input.
#[test]
fn judge_known_ends_with_outcome_key() {
    let root = TempDir::new().expect("tempdir");
    let package = write(&root, "judge.zip", known_package());
    let state = device();
    let host = start(&root, &[(ALIAS, &state)]);

    let (receipt, correlation_id) = run(&host, ALIAS, &package);

    assert_judge_end(&host, &receipt, correlation_id, JUDGED);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// S168-5b test 2, the host's half: the same judge whose layout does not know `member-02`, which
// the third slot shows (a name out of the dictionary), ends the run Completed with the step's
// `unknown_outcome_key`, which the host maps through the mapping `unknown`.
#[test]
fn judge_unknown_slot_ends_with_unknown_key() {
    let root = TempDir::new().expect("tempdir");
    let identities = (0..=6)
        .filter(|card| *card != 2)
        .map(member)
        .collect::<Vec<_>>();
    let package = write(
        &root,
        "judge.zip",
        judge_package(&identities, &[member(0), member(1)]),
    );
    let state = device();
    let host = start(&root, &[(ALIAS, &state)]);

    let (receipt, correlation_id) = run(&host, ALIAS, &package);

    assert_judge_end(&host, &receipt, correlation_id, UNKNOWN);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    host.close().expect("close the host");
}

// The run `correlation_id` ended `contained_task_judge_input_refused` before the device: its
// device was handed no input and the ledger holds no effect intent or completed effect of it.
fn assert_judge_input_refused(
    host: &RuntimeHost,
    state: &FakeState,
    receipt: &RuntimeReceipt,
    correlation_id: CorrelationId,
) {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Failed,
        "{:?}",
        receipt.outcome()
    );
    assert_eq!(
        receipt.outcome().code().as_str(),
        JUDGE_INPUT_REFUSED,
        "{:?}",
        receipt.outcome()
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    let facts = task_facts(host, correlation_id);
    assert!(
        !step_kinds(&facts)
            .iter()
            .any(|kind| matches!(*kind, "effect_intent" | "effect_completed")),
        "{:?}",
        step_kinds(&facts)
    );
}

// Nothing is held on `alias`: no scheduling pause, global or of the instance, and the Runtime
// is not poisoned.
fn assert_nothing_held(host: &RuntimeHost, alias: &str) {
    let (global, instances) = host
        .scheduling_pauses_for_test()
        .expect("the scheduling pauses");
    assert!(global.is_none(), "{global:?}");
    assert!(!instances.contains_key(alias), "{instances:?}");
    assert!(host.fatal_error().expect("runtime health").is_none());
}

// S168-5b test 5, the host's half: a host handed an effect intent for the judge step's action
// (right after the step's record, as a faulty interpreter would hand it) ends that run
// `contained_task_judge_input_refused` before the device: the device is handed no input and the
// ledger holds no effect intent of the step. A run of another instance goes on and ends with the
// judge's key; the refused instance runs again and completes, with nothing paused or poisoned on
// it. Fix 1 (layer 3 is the host's on its own): a host handed an input with no effect intent
// after the run's judge record ends that run the same way, before the device.
#[test]
fn judge_performs_no_input() {
    let root = TempDir::new().expect("tempdir");
    let package = write(&root, "judge.zip", known_package());
    let (first, second) = (device(), device());
    let host = start(&root, &[(ALIAS, &first), (OTHER_ALIAS, &second)]);

    host.inject_judge_effect_intent_for_test()
        .expect("inject an effect intent");
    let (receipt, correlation_id) = run(&host, ALIAS, &package);
    assert_judge_input_refused(&host, &first, &receipt, correlation_id);

    let (receipt, correlation_id) = run(&host, OTHER_ALIAS, &package);
    assert_judge_end(&host, &receipt, correlation_id, JUDGED);
    assert_eq!(second.input_count.load(Ordering::Acquire), 0);

    // The refused instance is not held: it runs the judge again and completes.
    assert_nothing_held(&host, ALIAS);
    let (receipt, correlation_id) = run(&host, ALIAS, &package);
    assert_judge_end(&host, &receipt, correlation_id, JUDGED);
    assert_eq!(first.input_count.load(Ordering::Acquire), 0);
    assert_nothing_held(&host, ALIAS);

    // An input with no effect intent before it, after the judge's record.
    host.inject_judge_input_for_test().expect("inject an input");
    let (receipt, correlation_id) = run(&host, ALIAS, &package);
    assert_judge_input_refused(&host, &first, &receipt, correlation_id);
    assert_nothing_held(&host, ALIAS);
    host.close().expect("close the host");
}
