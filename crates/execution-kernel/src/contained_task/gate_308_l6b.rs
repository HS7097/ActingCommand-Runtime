// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #308 SEL-L6b (FCM row SEL-L6b; compromise T-01): the scan of a select step replayed
// over a saved frame sequence with the facts of the whole-package replay (SEL-L6a). Every frame
// of the sequence is a capture of the kernel's own scan path (`dry_run_scan`): the step's
// dispatched frame, then each capture the scan takes, as a forward run recorded them. The
// scan's policy evaluates the given fact snapshot at the instant it was read, or the empty
// offline snapshot; a scan that needs a frame the sequence does not hold ends with a typed
// failure, never as a pass (frozen selector model S0 v2.1 §1.1, §6 row L3 and review R1-M11b).
//
// The list is SEL-L3's neutral one, read in full: a 64 x 96 frame shows one column of six
// cards, card k a noise marker (the anchor template, 8 x 6 px) at x 4 and y 10 + 12 k with a
// 4 x 4 patch of its own colour to its right. Pixel (0, 0) is red on the list page, pixel
// (1, 0) green at the list's start and pixel (2, 0) blue at its end. The readable band is the
// whole frame, so every card is read; the OCR provider reads a card's name from its patch.

use super::*;
use crate::{OfflineReplayFacts, replay_scan};
use actingcommand_contract::{
    FactContent, FactRecord, FactScope, FactValue, InstanceFactContext, InstanceFactSnapshot,
    ScanCoverageState, ScanStartEvidence, ScanUnknownCause,
};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    AssetResolver, NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    RecognitionPack, RecognitionPackResult, VisionProviderError, VisionProviderErrorCode,
};
use actingcommand_selection_policy::{GateUnknownHandling, SelectionOutcome, UnknownReason};
use serde_json::json;

const WIDTH: u32 = 64;
const HEIGHT: u32 = 96;
// The cards' names, top to bottom, and the identities they map to.
const NAMES: [&str; 6] = ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot"];
pub(super) const MEMBERS: [&str; 6] = [
    "member-a", "member-b", "member-c", "member-d", "member-e", "member-f",
];
// The fact the scan's policy gates on, published at `PUBLISHED_AT` and fresh for a minute.
const FACT_KEY: &str = "session.example.enabled";
pub(super) const PUBLISHED_AT: u64 = 1_000_000;
const MAX_AGE_MS: u64 = 60_000;

// Deterministic pixel noise: a shifted copy of it correlates with it near zero.
fn noise(x: u32, y: u32) -> [u8; 3] {
    let mut state = u64::from(y * 64 + x).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x2545_f491;
    let mut next = || {
        state ^= state >> 33;
        state = state.wrapping_mul(0xff51_afd7_ed55_8ccd);
        state ^= state >> 33;
        (state & 0xff) as u8
    };
    [next(), next(), next()]
}

fn image(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Frame {
    let pixel = &pixel;
    Frame::from_pixels(
        width,
        height,
        (0..height)
            .flat_map(|y| (0..width).flat_map(move |x| pixel(x, y)))
            .collect(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
}

// The red level of card k's patch; the green and blue follow from it.
fn level(card: u32) -> u8 {
    u8::try_from(30 + 36 * card).expect("level")
}

// The list with its start and end marks.
pub(super) fn list(start: bool, end: bool) -> Frame {
    image(WIDTH, HEIGHT, |x, y| {
        match (x, y) {
            (0, 0) => return [255, 0, 0],
            (1, 0) if start => return [0, 255, 0],
            (2, 0) if end => return [0, 0, 255],
            _ => {}
        }
        for card in 0..6 {
            let top = 10 + 12 * card;
            if (top..top + 6).contains(&y) && (4..12).contains(&x) {
                return noise(x - 4, y - top);
            }
            if (top..top + 4).contains(&y) && (12..16).contains(&x) {
                let red = level(card);
                return [red, 220 - red / 2, 90];
            }
        }
        [30, 31, 32]
    })
}

#[derive(Debug)]
struct Marker(Vec<u8>);

impl AssetResolver for Marker {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
        assert_eq!(path, "assets/card.png");
        Ok(self.0.clone())
    }
}

// Reads a card's name from the colour of its patch: the name region starts at the card's
// anchor, and the patch lies 8 px to its right.
#[derive(Debug)]
struct CardNames;

impl VisionProvider for CardNames {
    fn require_ocr_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Ok(())
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
        let red = request.frame.rgb8_pixels[(y * width + x) * 3];
        let text = (0..6)
            .position(|card| level(card) == red)
            .map_or("", |card| NAMES[card]);
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: text.to_owned(),
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

// A prepared task whose one select step `pick` scans the list page over the card layout; its
// policy keeps a card only while the instance fact `FACT_KEY` is true and at most a minute old.
pub(super) fn scan_task() -> PreparedContainedTask {
    let pack: RecognitionPack = serde_json::from_value(json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed", "color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/list",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
            {"type": "color", "id": "check/list_start",
             "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": [0, 255, 0]},
            {"type": "color", "id": "check/list_end",
             "region": {"x": 2, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]},
            {"type": "template", "id": "ui/card", "template_path": "assets/card.png",
             "region": "full_frame", "threshold": 0.95},
            {"type": "ocr", "id": "card/name",
             "region": {"x": 0, "y": 0, "width": 12, "height": 4},
             "languages": ["en"], "timeout_ms": 2000, "match_mode": "exact",
             "expected": ["alpha"], "case_sensitive": true, "minimum_confidence": 0.8,
             "model_ref": "PP-OCRv6_medium", "model_sha256": "a".repeat(64)}
        ],
        "candidate_layouts": [{
            "id": "layout/cards", "page_id": "neutral/list", "kind": "repeated_anchor",
            "anchor": "ui/card", "max_instances": 16, "order": "top_to_bottom",
            "instance_rect": {"x": 0, "y": 0, "width": 16, "height": 8},
            "click": {"x": 2, "y": 1, "width": 4, "height": 4},
            "unknown_identity": "reject",
            "features": [{"name": "member", "value": "identity", "target": "card/name",
                          "offset": {"x": 0, "y": 0},
                          "identity": {
                              "entries": MEMBERS.iter().zip(NAMES)
                                  .map(|(id, name)| json!({"id": id, "aliases": [name]}))
                                  .collect::<Vec<_>>(),
                              "recognition": {"kind": "ocr_aliases", "max_distance": 1,
                                              "minimum_margin": 1,
                                              "minimum_confidence_milli": 800}}}]
        }]
    }))
    .expect("recognition pack");
    let marker = image(8, 6, noise).png_for_artifact().expect("marker PNG");
    let evaluator = RecognitionEvaluator::with_vision_provider(
        pack,
        Arc::new(Marker(marker)),
        Arc::new(CardNames),
    )
    .expect("recognition evaluator");
    let control: TaskControl = serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.scan.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": HEIGHT},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 10_000,
        "timeout_ms": 60_000
    }))
    .expect("task control");
    let mut program: TaskProgram = serde_json::from_value(json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "target_page": "list",
        "operations": [{
            "id": "pick", "from": "list", "unguarded_trusted_coordinate": true,
            "select": {"layout_id": "layout/cards",
                       "policy": {"path": "policies/select.json", "sha256": "b".repeat(64)}}
        }]
    }))
    .expect("task program");
    let policy: actingcommand_selection_policy::SelectionPolicy = serde_json::from_value(json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "cards-enabled",
        "applies_to": {
            "candidate_layout_id": "layout/cards",
            "outcome_keys": {"selected": "card_selected", "empty": "card_empty",
                             "insufficient": "card_insufficient",
                             "ambiguous": "card_ambiguous", "unknown": "card_unknown"}
        },
        "fields": [{"name": "member",
                    "value_type": {"type": "enum_string", "allowed": MEMBERS}}],
        "facts": [{"fact_key": FACT_KEY, "value_type": {"type": "boolean"},
                   "max_age_ms": MAX_AGE_MS, "minimum_confidence_milli": 1_000}],
        "gates": [{"gate_id": "enabled",
                   "predicate": {"kind": "boolean_equals",
                                 "value": {"source": "fact", "fact_key": FACT_KEY},
                                 "expected": true},
                   "on_unknown": {"kind": "drop_candidate"}}],
        "scoring": [],
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    }))
    .expect("selection policy");
    policy.validate().expect("valid selection policy");
    program.operations[0].prepared_select = Some(Box::new(selection::PreparedSelect::for_test(
        "layout/cards",
        policy,
    )));
    let page_set: PageSet = serde_json::from_value(json!({
        "schema_version": "0.3",
        "pages": [{"id": "neutral/list", "required": ["page/list"]}]
    }))
    .expect("page set");
    let detector = PageDetector::new(page_set).expect("page detector");
    detector
        .validate(&evaluator)
        .expect("page detector targets");
    PreparedContainedTask {
        control,
        program,
        evaluator,
        detector,
        entry_page: None,
        scheduling_outcome: None,
        post_admission_ocr: None,
        post_admission_fields: None,
        entry_count: 1,
        task_count: 1,
    }
}

// The scan over the card list (SEL-L3's): one drag forward, no rewind.
pub(super) fn card_scan() -> ScanDeclaration {
    serde_json::from_value(json!({
        "identity_feature": "member",
        "axis": "vertical",
        "advance": {"kind": "drag",
                    "from": {"x": 20, "y": 60, "width": 20, "height": 10},
                    "to": {"x": 20, "y": 20, "width": 20, "height": 10},
                    "duration_ms": 300, "min_content_step_px": 12,
                    "max_content_step_px": 40},
        "reposition": {"kind": "drag",
                       "from": {"x": 20, "y": 70, "width": 20, "height": 10},
                       "to": {"x": 20, "y": 10, "width": 20, "height": 10},
                       "duration_ms": 300, "max_content_step_px": 60},
        "settle": {"interval_ms": 1, "timeout_ms": 1000},
        "track": {"area": {"x": 0, "y": 4, "width": WIDTH, "height": 92},
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
        "bounds": {"max_viewports": 8, "max_reposition_viewports": 4, "max_ms": 60_000}
    }))
    .expect("scan declaration")
}

// The fact snapshot a Runtime answered for the instance, read at `read_at_unix_ms`.
pub(super) fn snapshot_facts(read_at_unix_ms: u64) -> OfflineReplayFacts {
    let record = FactRecord {
        scope: FactScope::Instance {
            instance_id: "instance-a".to_owned(),
        },
        key: FACT_KEY.to_owned(),
        content: FactContent::Inline {
            value: FactValue::Boolean(true),
        },
        observed_at_unix_ms: PUBLISHED_AT,
        expires_at_unix_ms: None,
        ttl_policy: None,
        confidence_milli: 1_000,
        source_detector: "decision-state".to_owned(),
        source_snapshot_id: "decision-state:enabled".to_owned(),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "0".repeat(64),
        invalidate_on: Vec::new(),
    };
    OfflineReplayFacts::Snapshot {
        snapshot: InstanceFactSnapshot {
            snapshot_id: "snapshot-a".to_owned(),
            ledger_position: 7,
            context: InstanceFactContext {
                instance_id: "instance-a".to_owned(),
                server_id: "test".to_owned(),
                game_id: "neutral".to_owned(),
            },
            records: vec![record],
        },
        read_at_unix_ms,
    }
}

// A recorded forward run of the whole list: the dispatched frame at the list's start and its
// settle capture, one drag the list absorbs at its end, and the end frame's settle capture.
pub(super) fn forward_run() -> Vec<Frame> {
    vec![
        list(true, false),
        list(true, false),
        list(false, true),
        list(false, true),
    ]
}

// T-01: the scan replays over the recorded forward run with the given snapshot, evaluated at
// the instant the Runtime read it. Read while the fact was fresh, the scan covers the list and
// every card is a member, fixed on the first viewport with the identity its name maps to; read
// after the fact's age ran out, or with the empty offline snapshot, the scan still covers the
// list, and every card is read but dropped as unknown by the policy's gate.
#[test]
fn l6b_t1_a_scan_select_replays_over_a_saved_forward_run_with_the_given_facts() {
    let task = scan_task();
    let scan = card_scan();

    let fresh = replay_scan(
        &task,
        "pick",
        &scan,
        forward_run(),
        &snapshot_facts(PUBLISHED_AT + 500),
        &task.default_run_parameters(),
    )
    .expect("the replayed scan covers the list");
    assert_eq!(fresh.coverage, ScanCoverageState::Complete);
    assert_eq!(fresh.positions, 6);
    assert_eq!(
        fresh
            .viewports
            .iter()
            .map(|viewport| viewport.content_offset_px)
            .collect::<Vec<_>>(),
        [0, 0]
    );
    assert_eq!(
        fresh
            .members
            .iter()
            .map(|member| (member.identity.as_str(), member.fixed_viewport))
            .collect::<Vec<_>>(),
        MEMBERS.iter().map(|id| (*id, 0)).collect::<Vec<_>>()
    );
    assert!(fresh.unknown.is_empty(), "{:?}", fresh.unknown);
    assert_eq!(fresh.decision, None);

    for facts in [
        snapshot_facts(PUBLISHED_AT + MAX_AGE_MS + 1),
        OfflineReplayFacts::Empty {
            now_unix_ms: PUBLISHED_AT,
        },
    ] {
        let dropped = replay_scan(
            &task,
            "pick",
            &scan,
            forward_run(),
            &facts,
            &task.default_run_parameters(),
        )
        .expect("the replayed scan covers the list");
        assert_eq!(dropped.coverage, ScanCoverageState::Complete);
        assert!(dropped.members.is_empty(), "{:?}", dropped.members);
        assert_eq!(
            dropped
                .unknown
                .iter()
                .map(|unknown| (unknown.identity.as_deref(), unknown.cause))
                .collect::<Vec<_>>(),
            MEMBERS
                .iter()
                .map(|id| (Some(*id), ScanUnknownCause::UnknownDropped))
                .collect::<Vec<_>>()
        );
    }
}

// S0 §6 row L3 and R1-M11b: a replay that needs a frame it does not hold ends with a typed
// failure, never as a pass. The recording stops at the list's start; the scan's drag toward
// the end needs a capture the sequence does not hold.
#[test]
fn l6b_t2_a_recording_that_stops_before_the_list_ends_fails_typed() {
    let task = scan_task();
    let error = replay_scan(
        &task,
        "pick",
        &card_scan(),
        vec![list(true, false), list(true, false)],
        &snapshot_facts(PUBLISHED_AT + 500),
        &task.default_run_parameters(),
    )
    .expect_err("the sequence holds no frame after the drag");
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::SelectionScanReplayFrameMissing.as_str()
    );
}

// Wave-14 coordinator ruling SEL-L6b item 2: an `aborted` scan is never a pass (Fail Loud).
// selection-graph.md "Offline": a scan the evaluator ended before its coverage is `aborted`, the
// result carries the evaluator's decision, and only coverage `complete` or `early_stopped` exits
// 0. Selector model S0 §2.6 and §3.4 path P3: an evaluator `unknown` outcome in a viewport becomes
// the decision immediately, with coverage `incomplete`, which has its start only (§1.9); no
// member was fixed before it. Here the policy's gate aborts the evaluation on an unknown fact, and
// the snapshot was read after the fact's age ran out (S0 §4.3: a stale fact reads `fact_stale`
// and the evaluation aborts): the scan reads the six cards of its first viewport and the
// evaluator aborts there. actinglab's exit for this result is not reachable in CI: a scan's
// identity feature reads an OCR target, and actinglab without an OCR model refuses such a
// package before any frame.
#[test]
fn gate_l6b_a_scan_the_evaluator_aborts_in_a_viewport_replays_aborted_never_covered() {
    let mut task = scan_task();
    let mut policy = task.program.operations[0]
        .prepared_select
        .as_deref()
        .expect("the select step")
        .policy()
        .clone();
    policy.gates[0].on_unknown = GateUnknownHandling::AbortEvaluation;
    policy.validate().expect("valid selection policy");
    task.program.operations[0].prepared_select = Some(Box::new(
        selection::PreparedSelect::for_test("layout/cards", policy),
    ));

    let aborted = replay_scan(
        &task,
        "pick",
        &card_scan(),
        forward_run(),
        &snapshot_facts(PUBLISHED_AT + MAX_AGE_MS + 1),
        &task.default_run_parameters(),
    )
    .expect("the evaluator's abort is the replayed scan's result, not a failure");

    assert_eq!(aborted.coverage, ScanCoverageState::Incomplete);
    assert_eq!(aborted.start, Some(ScanStartEvidence::StartCheck));
    assert_eq!(aborted.viewports.len(), 1);
    assert!(aborted.members.is_empty(), "{:?}", aborted.members);
    let decision = aborted.decision.expect("the evaluator's decision");
    assert!(
        matches!(
            decision.outcome,
            SelectionOutcome::Unknown {
                reason: UnknownReason::FactStale,
                ..
            }
        ),
        "{:?}",
        decision.outcome
    );
    assert_eq!(decision.outcome_key, "card_unknown");
}
