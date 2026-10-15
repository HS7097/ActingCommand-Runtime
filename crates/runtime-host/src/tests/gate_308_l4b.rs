// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #308 SEL-L4b, gate key host 77 (frozen selector model S0 v2.1 §1.4, §3.2 rule 1, §3.4
// path P4): a package whose one select step declares a scan runs through the host. Nobody the
// policy lists is on the list and nothing is unknown, so the step's `on_empty` ends the run
// Completed on the step's own page: the kernel records the step's start, its decision and its
// finish on that page, and no input; the host derives the terminal's scheduling disposition from
// the one mapping for (no designated effect, the step's page) and cross-checks its key with the
// kernel's.
//
// The list is the neutral one of the SEL-L3 gate, on one screen: a 64 x 96 frame shows one column
// of six cards, card k a noise marker (the anchor template, 8 x 6 px) at x 4 and y 10 + 12 k with
// a 4 x 4 patch of its own colour to its right. Pixel (0, 0) is red on the list page, (1, 0)
// green (the list's start), (2, 0) blue (its end) and (3, 0) white (the step's guard). The
// scripted OCR provider reads a card's name, `member-0k`, from its patch. The policy lists only
// `member-06`, which no card shows.

use super::*;
use actingcommand_contract::{
    ScanCoverageState, ScanReposition, ScanViewportMotion, TaskSelectionBlocking,
    TaskSelectionConfirmation, TaskSelectionOutcome, TaskSelectionProjection,
    TaskSelectionUnknownCheck, TaskSelectionUpperBound,
};

const ALIAS: &str = "neutral.instance";
const WIDTH: u32 = 64;
const HEIGHT: u32 = 96;
// The anchor template's size.
const MARKER: (u32, u32) = (8, 6);
// The cards on the screen, and the one more identity the layout knows.
const CARDS: u32 = 6;
const LISTED: &str = "member-06";
const ON_EMPTY_KEY: &str = "nobody_listed";

const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const WHITE: [u8; 3] = [255, 255, 255];
const BACKGROUND: [u8; 3] = [30, 31, 32];

// Deterministic pixel noise: a shifted copy of it correlates with it near zero.
fn noise(x: u32, y: u32) -> [u8; 3] {
    let mut state = u64::from(y * 64 + x).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x2545_f491;
    let mut next = || {
        state ^= state >> 33;
        state = state.wrapping_mul(0xff51_afd7_ed55_8ccd);
        state ^= state >> 33;
        state.to_le_bytes()[0]
    };
    [next(), next(), next()]
}

// The red level of card k's patch; the green and blue follow from it.
fn level(card: u32) -> u8 {
    u8::try_from(30 + 36 * card).expect("level")
}

// The business ID every identity of the layout reads as, card k's `member-0k`.
fn member(card: u32) -> String {
    format!("member-{card:02}")
}

// One pixel of the list screen.
fn list_pixel(x: u32, y: u32) -> [u8; 3] {
    match (x, y) {
        (0, 0) => return RED,
        (1, 0) => return GREEN,
        (2, 0) => return BLUE,
        (3, 0) => return WHITE,
        _ => {}
    }
    for card in 0..CARDS {
        let top = 10 + 12 * card;
        if (top..top + MARKER.1).contains(&y) && (4..4 + MARKER.0).contains(&x) {
            return noise(x - 4, y - top);
        }
        if (top..top + 4).contains(&y) && (12..16).contains(&x) {
            let red = level(card);
            return [red, 220 - red / 2, 90];
        }
    }
    BACKGROUND
}

// Every pixel of the list screen, by its row-major index, as the fake device paints it.
fn list_screen() -> Vec<(usize, [u8; 3])> {
    (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| list_pixel(x, y)))
        .enumerate()
        .collect()
}

// Reads a card's name from the colour of its patch: the name region starts at the card's
// anchor, and the patch lies 8 px to its right.
#[derive(Debug)]
struct CardNames;

impl VisionProvider for CardNames {
    fn require_ocr_model(
        &self,
        model_ref: &str,
        model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        if model_ref == "PP-OCRv6_medium"
            && model_sha256.len() == 64
            && model_sha256.bytes().all(|byte| byte == b'a')
        {
            Ok(())
        } else {
            Err(VisionProviderError::new(
                VisionProviderErrorCode::ModelMismatch,
                "unexpected OCR model identity",
            ))
        }
    }

    fn require_nn_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        let x = usize::try_from(request.region.x + 9).expect("x");
        let y = usize::try_from(request.region.y + 1).expect("y");
        let width = usize::try_from(request.frame.width).expect("width");
        let red = request.frame.rgb8_pixels.get((y * width + x) * 3).copied();
        let text = (0..CARDS)
            .find(|card| Some(level(*card)) == red)
            .map_or_else(String::new, member);
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text,
            blocks: Vec::new(),
            confidence: Some(0.99),
            detection: None,
        })
    }

    fn classify(
        &self,
        _request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }
}

// The scan over the one-screen list (S0 §2.1): the G7 `repeated_anchor` layout's identity
// feature, a drag forward and a reposition that the admission's static checks accept in this
// frame, and start and end checks that both pass on the first viewport, so no move is made.
fn scan() -> serde_json::Value {
    serde_json::json!({
        "identity_feature": "member",
        "axis": "vertical",
        "advance": {"kind": "drag",
                    "from": {"x": 20, "y": 70, "width": 20, "height": 10},
                    "to": {"x": 20, "y": 30, "width": 20, "height": 10},
                    "duration_ms": 300, "min_content_step_px": 12,
                    "max_content_step_px": 60},
        "reposition": {"kind": "drag",
                       "from": {"x": 20, "y": 80, "width": 20, "height": 10},
                       "to": {"x": 20, "y": 10, "width": 20, "height": 10},
                       "duration_ms": 300, "max_content_step_px": 90},
        "settle": {"interval_ms": 1, "timeout_ms": 1000},
        "track": {"area": {"x": 0, "y": 0, "width": WIDTH, "height": HEIGHT},
                  "edge_margin_px": 2,
                  "signature": {"rect": {"x": 8, "y": 0, "width": 4, "height": 4},
                                "columns": 1, "rows": 1},
                  "signature_max_distance_milli": 500,
                  "position_tolerance_px": 1,
                  "min_overlap_instances": 2,
                  "spacing": {"max_axis_gap_px": 20, "columns": 1,
                              "max_trailing_gap_px": 100, "max_cross_lead_px": 10,
                              "max_cross_gap_px": 10}},
        "start": {"check": "check/list_start", "rewind": false},
        "end": {"check": "check/list_end"},
        "bounds": {"max_viewports": 8, "max_reposition_viewports": 4, "max_ms": 20_000}
    })
}

// A member set (no scoring term, mode `none_allowed`, the last tie-break key `candidate_id
// lowest_first`) whose gate admits only the listed identity.
fn policy(identities: &[String]) -> serde_json::Value {
    serde_json::json!({
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
                                 "allowed": [LISTED]},
                   "on_unknown": {"kind": "drop_candidate"}}],
        "scoring": [],
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    })
}

// The package: one select step `pick` from `list` to `detail` over the card layout, with its
// scan, its policy and `on_empty`; the scheduling outcome maps the step's own page without a
// designated effect to the `on_empty` key, and the target page to another key. The task
// declares the card layout, against which admission checks the step's policy, and the
// recognition pack holds it for the run.
fn scan_select_package() -> Vec<u8> {
    let identities = (0..=CARDS).map(member).collect::<Vec<_>>();
    let space = serde_json::json!({"width": WIDTH, "height": HEIGHT});
    let layout = serde_json::json!({
        "id": "layout/cards", "page_id": "neutral/list", "kind": "repeated_anchor",
        "anchor": "ui/card", "max_instances": 8, "order": "top_to_bottom",
        "instance_rect": {"x": 0, "y": 0, "width": 16, "height": 8},
        "click": {"x": 2, "y": 1, "width": 4, "height": 4},
        "readable_band": {"x": 0, "y": 0, "width": WIDTH, "height": HEIGHT},
        "unknown_identity": "reject",
        "features": [{"name": "member", "value": "identity", "target": "card/name",
                      "offset": {"x": 0, "y": 0},
                      "identity": {
                          "entries": identities.iter()
                              .map(|id| serde_json::json!({"id": id, "aliases": [id]}))
                              .collect::<Vec<_>>(),
                          "recognition": {"kind": "ocr_aliases", "max_distance": 1,
                                          "minimum_margin": 1,
                                          "minimum_confidence_milli": 800}}}]
    });
    let control = serde_json::json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.scan.task",
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
            "select": {"layout_id": "layout/cards",
                       "policy": {"path": "policies/listed.json"},
                       "scan": scan(),
                       "on_empty": {"outcome_key": ON_EMPTY_KEY}},
            "retryable": false
        }]
    });
    let color = |id: &str, x: u32, expected: [u8; 3]| {
        serde_json::json!({"type": "color", "id": id,
                           "region": {"x": x, "y": 0, "width": 1, "height": 1},
                           "expected": expected})
    };
    let pack = serde_json::json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": space,
        "defaults": {"match_metric": "ccoeff_normed", "color_max_distance": 0.0},
        "targets": [
            color("page/list", 0, RED),
            color("page/detail", 0, BLUE),
            color("check/list_start", 1, GREEN),
            color("check/list_end", 2, BLUE),
            color("guard/list", 3, WHITE),
            {"type": "template", "id": "ui/card", "template_path": "recognition/assets/card.png",
             "region": "full_frame", "threshold": 0.95},
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
    let marker = Frame::from_pixels(
        MARKER.0,
        MARKER.1,
        (0..MARKER.1)
            .flat_map(|y| (0..MARKER.0).flat_map(move |x| noise(x, y)))
            .collect(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("marker frame")
    .png_for_artifact()
    .expect("marker PNG");
    let json = |value: serde_json::Value| serde_json::to_vec(&value).expect("package JSON");
    let entries = [
        ("control.json", json(control)),
        (
            "resources/manifest.json",
            json(serde_json::json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        ("resources/operations/task/task.json", json(task)),
        (
            "resources/operations/task/policies/listed.json",
            json(policy(&identities)),
        ),
        ("resources/recognition/neutral.test.pack.json", json(pack)),
        ("resources/recognition/assets/card.png", marker),
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

// S0 §1.4 and §3.4 P4, end to end: the scan reads the whole one-screen list on its first
// viewport, every card is read and gated out, nothing is unknown, and the decision is the
// evaluator's `empty`; the step's `on_empty` ends the run Completed on `list` with no input. The
// ledger holds the step's start, its one decision record (the v2 projection with coverage
// `complete`, `selection_result` empty, `confirmation` not_attempted and `unknown_check`) and its
// finish on `list`, and no effect intent or completed effect; the terminal's scheduling
// disposition carries the `on_empty` key with no designated effect.
#[test]
fn l4b_a_scan_select_that_finds_nobody_listed_ends_the_run_with_its_on_empty_key() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("scan-select.zip");
    fs::write(&package, scan_select_package()).expect("write the package");
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

    // No input: no tap, no drag.
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    assert!(
        state
            .input_actions
            .lock()
            .expect("fake input actions lock")
            .is_empty()
    );
    assert!(
        state
            .segmented_swipe_plans
            .lock()
            .expect("fake segmented swipe plans lock")
            .is_empty()
    );

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

    // The step: its start, its decision and its finish, and nothing of an effect between them.
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
    // The step finishes on its own page, outside any phase.
    let finished = facts
        .iter()
        .find_map(|fact| match fact {
            TaskSemanticFact::StepFinished {
                step_index,
                page_label,
                phase,
                ..
            } => Some((*step_index, page_label.as_str(), phase.is_none())),
            _ => None,
        })
        .expect("the step's finish");
    assert_eq!(finished, (0, "neutral/list", true));

    // The decision record of S0 §3.4 P4.
    let records = facts
        .iter()
        .filter_map(|fact| match fact {
            TaskSemanticFact::SelectionEvaluated {
                step_index,
                selection,
                ..
            } => Some((*step_index, selection.as_ref())),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(0, record)] = records.as_slice() else {
        panic!("one decision record of step 0, got {}", records.len());
    };
    assert_eq!(
        record.projection.schema_version(),
        "actingcommand.candidate-projection.v2"
    );
    let TaskSelectionProjection::V2(projection) = &record.projection else {
        panic!("a scan projection, got {:?}", record.projection);
    };
    assert_eq!(projection.coverage().state, ScanCoverageState::Complete);
    assert_eq!(
        projection
            .viewports()
            .iter()
            .map(|viewport| viewport.motion)
            .collect::<Vec<_>>(),
        [ScanViewportMotion::Start]
    );
    assert_eq!(
        projection.reposition(),
        ScanReposition {
            rewind_viewports: 0,
            return_viewports: 0
        }
    );
    assert!(projection.candidates().is_empty());
    assert!(projection.unknown().is_empty());
    assert_eq!(projection.gated_out().count, CARDS);
    assert_eq!(record.selection_result, TaskSelectionOutcome::Empty);
    assert_eq!(record.outcome_key, "card_empty");
    assert!(record.selected.is_empty());
    assert_eq!(record.confirmation, TaskSelectionConfirmation::NotAttempted);
    assert_eq!(
        record.unknown_check,
        Some(TaskSelectionUnknownCheck {
            unknown_instances: 0,
            blocking: TaskSelectionBlocking::None,
            upper_bound: TaskSelectionUpperBound::NoneUnknown,
        })
    );

    // The terminal: Completed on `list` after one step, its disposition the `on_empty` key
    // with no designated effect.
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
