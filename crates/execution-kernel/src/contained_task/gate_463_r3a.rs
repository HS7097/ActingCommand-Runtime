// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 K-R3a (register §1.392; contracts/selection-graph.md, "Offline"; the design note
// `rd-1115/463-r3-scan-design.md`, section 3.1 and its row K-R3a, which the coordinator accepted on
// board #463): the kernel's offline dry run of a scan select as far as its decision,
// `dry_run_scan_decide`, the entry the Lab's self-test calls. It reads the step's own admitted scan,
// runs the session as online over the given frames to its coverage checks and decides as the step
// does: the decision, the unknown-instance check and the chosen member; nothing returns, confirms or
// is pressed. A draft policy in place of the sealed one is checked as `dry_run_select` checks one,
// and its last tie-break key as admission checks a scan's policy. The fix round K-a adds a scan
// that stops early on the first viewport, the second kind of scan select, and a decision between
// two members that chooses the second.
//
// Every expected value comes from those lines and the package below, never from the code. The
// package is K1's card list (`gate_463_k1`, its scan run): the same six cards, card k a noise marker
// (the anchor template, 8 x 6 px) at x 4 with a 4 x 4 patch of its own colour to its right, which
// the card reader reads as the card's name; the same layout, scan, guard and policy shape; admitted
// by the kernel's own admission. Two things differ so that the list spreads over two viewports: its
// list area is the frame's rows 4 to 55, which hold four cards (the layout's readable band and the
// scan's tracking area are that area, and its moves fit it), and its names table holds a seventh
// name, `golf`, that no card shows. Card k lies at content row 10 + 12 k. The first viewport shows
// the list's top, cards 0 to 3, with its start mark; the second, scrolled by 24 px, cards 2 to 5
// with its end mark. Each viewport is fed twice, as the scan's settle takes it: v0, v0, v1, v1.

use super::*;
use actingcommand_contract::{
    InstanceFactContext, InstanceFactSnapshot, ScanCoverageState, ScanUnknownCause,
    TaskSelectionBlocking, TaskSelectionUnknownCheck, TaskSelectionUpperBound,
};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    VisionProviderError, VisionProviderErrorCode,
};
use actingcommand_selection_policy::{SelectionOutcome, SelectionPolicy};
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 96;
// The list area: the frame's rows 4 to 55.
const LIST_TOP: u32 = 4;
const LIST_HEIGHT: u32 = 52;
// How far the second viewport is scrolled: two cards.
const SCROLLED: u32 = 24;
// The cards on the list, top to bottom.
const CARDS: u32 = 6;
// The names, the first six the cards' and the seventh shown by no card, and their identities.
const NAMES: [&str; 7] = [
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf",
];
const MEMBERS: [&str; 7] = [
    "member-a", "member-b", "member-c", "member-d", "member-e", "member-f", "member-g",
];
const BACKGROUND: [u8; 3] = [30, 31, 32];
// The red level of a patch that no name maps to.
const UNREADABLE: u8 = 10;
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
// The instant the dry run decides at, and the instance its fact snapshot is of.
const NOW_UNIX_MS: u64 = 1_000_000;
const INSTANCE: &str = "neutral.instance";

// Deterministic pixel noise, K1's: a shifted copy of it correlates with it near zero.
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

// The red level of card k's patch; the green and blue follow from it.
fn patch_level(card: u32) -> u8 {
    u8::try_from(30 + 36 * card).expect("level")
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

// The list scrolled by `offset` px. Row 0 holds the page (red), the start mark (green, at the
// list's top), the end mark (blue, at its end) and the step's guard (green); the list area shows the
// content rows `offset` to `offset` + 51. The card `unreadable`, if any, has a patch no name maps
// to.
fn viewport(offset: u32, unreadable: Option<u32>) -> Frame {
    image(WIDTH, HEIGHT, |x, y| {
        if y == 0 && x < 4 {
            return match x {
                0 => [255, 0, 0],
                1 if offset == 0 => [0, 255, 0],
                2 if offset == SCROLLED => [0, 0, 255],
                3 => [0, 255, 0],
                _ => BACKGROUND,
            };
        }
        if !(LIST_TOP..LIST_TOP + LIST_HEIGHT).contains(&y) {
            return BACKGROUND;
        }
        let row = y + offset;
        for card in 0..CARDS {
            let top = 10 + 12 * card;
            if (top..top + 6).contains(&row) && (4..12).contains(&x) {
                return noise(x - 4, row - top);
            }
            if (top..top + 4).contains(&row) && (12..16).contains(&x) {
                let red = if unreadable == Some(card) {
                    UNREADABLE
                } else {
                    patch_level(card)
                };
                return [red, 220 - red / 2, 90];
            }
        }
        BACKGROUND
    })
}

// A forward run over the list: its top, then one move to its end, each viewport fed twice.
fn forward_run(unreadable: Option<u32>) -> Vec<Frame> {
    vec![
        viewport(0, unreadable),
        viewport(0, unreadable),
        viewport(SCROLLED, unreadable),
        viewport(SCROLLED, unreadable),
    ]
}

// The anchor template: the noise marker, 8 x 6 px.
fn marker_png() -> Vec<u8> {
    image(8, 6, noise).png_for_artifact().expect("marker PNG")
}

// Reads a card's name from the colour of its patch, as K1's reader does: the name region starts at
// the card's anchor, and the patch lies 8 px to its right.
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
        let text = (0..CARDS)
            .position(|card| patch_level(card) == red)
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

// The cards as K1's `repeated_anchor` layout, each naming its member, readable in the list area.
fn card_layout() -> Value {
    let entries = MEMBERS
        .iter()
        .zip(NAMES)
        .map(|(id, name)| json!({"id": id, "aliases": [name]}))
        .collect::<Vec<_>>();
    json!({
        "id": "layout/cards", "page_id": "neutral/list", "kind": "repeated_anchor",
        "anchor": "ui/card", "max_instances": 16, "order": "top_to_bottom",
        "instance_rect": {"x": 0, "y": 0, "width": 16, "height": 8},
        "click": {"x": 2, "y": 1, "width": 4, "height": 4},
        "readable_band": {"x": 0, "y": LIST_TOP, "width": WIDTH, "height": LIST_HEIGHT},
        "unknown_identity": "reject",
        "features": [{"name": "member", "value": "identity", "target": "card/name",
                      "offset": {"x": 0, "y": 0},
                      "identity": {
                          "entries": entries,
                          "recognition": {"kind": "ocr_aliases", "max_distance": 1,
                                          "minimum_margin": 1,
                                          "minimum_confidence_milli": 800}}}]
    })
}

// K1's scan over the card list, its tracking area the list area and its moves within it (an
// advance of 12 to 44 px, the list area's 52 less a card's 8): a drag forward, no rewind.
fn card_scan() -> Value {
    json!({
        "identity_feature": "member",
        "axis": "vertical",
        "advance": {"kind": "drag",
                    "from": {"x": 20, "y": 40, "width": 20, "height": 8},
                    "to": {"x": 20, "y": 16, "width": 20, "height": 8},
                    "duration_ms": 300, "min_content_step_px": 12, "max_content_step_px": 44},
        "reposition": {"kind": "drag",
                       "from": {"x": 20, "y": 44, "width": 20, "height": 8},
                       "to": {"x": 20, "y": 8, "width": 20, "height": 8},
                       "duration_ms": 300, "max_content_step_px": 52},
        "settle": {"interval_ms": 1, "timeout_ms": 1000},
        "track": {"area": {"x": 0, "y": LIST_TOP, "width": WIDTH, "height": LIST_HEIGHT},
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
        "bounds": {"max_viewports": 8, "max_reposition_viewports": 4, "max_ms": 120_000}
    })
}

// K1's scan with `early_stop` (fix round K-a): the scan of a select that knows the card it wants,
// which may stop before the end of the list once no position it has not read could outrank its
// best member (S0 §3.3).
fn early_stop_scan() -> Value {
    let mut scan = card_scan();
    scan["early_stop"] = json!("upper_bound");
    scan
}

// The policy `policy_id` over the cards: a card is a member when its identity is `wanted`, chosen
// with the tie-break keys `tie_break`.
fn policy(policy_id: &str, wanted: &str, tie_break: Value) -> Value {
    policy_of(policy_id, &[wanted], tie_break)
}

// The policy `policy_id` over the cards: a card is a member when its identity is one of `wanted`,
// chosen with the tie-break keys `tie_break`.
fn policy_of(policy_id: &str, wanted: &[&str], tie_break: Value) -> Value {
    json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": policy_id,
        "applies_to": {
            "candidate_layout_id": "layout/cards",
            "outcome_keys": {"selected": "card_selected", "empty": "card_empty",
                             "insufficient": "card_insufficient",
                             "ambiguous": "card_ambiguous", "unknown": "card_unknown"}
        },
        "fields": [{"name": "member",
                    "value_type": {"type": "enum_string", "allowed": MEMBERS}}],
        "facts": [],
        "gates": [{"gate_id": "wanted",
                   "predicate": {"kind": "string_in",
                                 "value": {"source": "field", "field": "member"},
                                 "allowed": wanted},
                   "on_unknown": {"kind": "drop_candidate"}}],
        "scoring": [],
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": tie_break
    })
}

// The last tie-break key a scan's policy must have: the lowest candidate ID first.
fn lowest_id_last() -> Value {
    json!([{"kind": "candidate_id", "direction": "lowest_first"}])
}

// Fix round K-a: the highest candidate ID first, then the last key a scan's policy must have. Of
// two members that score alike, the one numbered later in first-seen order is chosen.
fn highest_id_first() -> Value {
    json!([{"kind": "candidate_id", "direction": "highest_first"},
           {"kind": "candidate_id", "direction": "lowest_first"}])
}

// A draft policy as the Lab hands one to the dry run.
fn draft(policy_id: &str, wanted: &str, tie_break: Value) -> SelectionPolicy {
    serde_json::from_value(policy(policy_id, wanted, tie_break)).expect("a selection policy")
}

fn zip_entries(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, bytes) in entries {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(bytes).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON bytes")
}

// K1's linear package of one select step `pick` over the card list's scan `scan`, from the list
// (the entry) to the detail page, guarded by pixel (3, 0), whose sealed policy is `policy`.
fn package(policy: &Value, scan: Value) -> Vec<u8> {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.linear.scan.decide",
        "execution_mode": "linear_steps",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": HEIGHT},
        "entry_task_id": "task",
        "capture_interval_ms": 100,
        "step_timeout_ms": 32_000,
        "timeout_ms": 300_000,
        "max_steps": 1
    });
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "entry_page": "list",
        "target_page": "detail",
        "max_steps": 1,
        "candidate_layouts": [card_layout()],
        "operations": [{
            "id": "pick", "from": "list", "to": "detail",
            "expect_after": {"page_id": "detail", "timeout_ms": 5000, "interval_ms": 200},
            "guard": {"page_id": "list", "target_id": "guard/list",
                      "expected_rect": {"x": 3, "y": 0, "width": 1, "height": 1},
                      "color_probe": "guard/list"},
            "select": {"layout_id": "layout/cards", "policy": {"path": "policies/select.json"},
                       "scan": scan}
        }]
    });
    let colour = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed", "color_max_distance": 0.0},
        "targets": [
            colour("page/list", 0, [255, 0, 0]),
            colour("page/detail", 0, [0, 0, 255]),
            colour("check/list_start", 1, [0, 255, 0]),
            colour("check/list_end", 2, [0, 0, 255]),
            colour("guard/list", 3, [0, 255, 0]),
            {"type": "template", "id": "ui/card", "template_path": "assets/card.png",
             "region": "full_frame", "threshold": 0.95},
            {"type": "ocr", "id": "card/name",
             "region": {"x": 0, "y": 0, "width": 12, "height": 4},
             "languages": ["en"], "timeout_ms": 2000, "match_mode": "exact",
             "expected": ["alpha"], "case_sensitive": true, "minimum_confidence": 0.8,
             "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256}
        ],
        "candidate_layouts": [card_layout()]
    });
    let pages = ["list", "detail"].map(|page| {
        json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")],
               "optional": [], "forbidden": []})
    });
    zip_entries(&[
        ("control.json", bytes(&control)),
        (
            "resources/manifest.json",
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        ("resources/operations/task/task.json", bytes(&task)),
        (
            "resources/operations/task/policies/select.json",
            bytes(policy),
        ),
        ("resources/assets/card.png", marker_png()),
        ("resources/recognition/neutral.test.pack.json", bytes(&pack)),
        (
            "resources/recognition/neutral.test.pages.json",
            bytes(&json!({"schema_version": "0.3", "pages": pages})),
        ),
    ])
}

// The package with `policy` sealed, as contained task admission prepares it, with the card reader.
fn admitted(policy: &Value) -> PreparedContainedTask {
    admitted_scanning(policy, card_scan())
}

// The package with `policy` sealed and `pick` scanning with `scan`, as contained task admission
// prepares it, with the card reader.
fn admitted_scanning(policy: &Value, scan: Value) -> PreparedContainedTask {
    PreparedContainedTask::load_with_vision_provider(
        INSTANCE,
        &package(policy, scan),
        Arc::new(CardNames),
    )
    .unwrap_or_else(|error| panic!("the scan package: refused: {error}"))
}

// The instance's fact snapshot: no facts.
fn snapshot() -> InstanceFactSnapshot {
    InstanceFactSnapshot {
        snapshot_id: "snapshot:test:empty".to_owned(),
        ledger_position: 1,
        context: InstanceFactContext {
            instance_id: INSTANCE.to_owned(),
            server_id: "test".to_owned(),
            game_id: "neutral".to_owned(),
        },
        records: Vec::new(),
    }
}

// The dry run of `pick` over `frames`, with what a run without a dispatch reads, as a recording's
// self-check passes it (fix round K-a): the package's declared parameters and variables at their
// defaults (it declares none of either).
fn dry_run(
    task: &PreparedContainedTask,
    frames: Vec<Frame>,
    policy_override: Option<&SelectionPolicy>,
) -> Result<ScanDryRunDecided, ContainedTaskError> {
    dry_run_scan_decide(
        task,
        "pick",
        frames,
        (&snapshot(), NOW_UNIX_MS),
        &task.default_run_values(),
        policy_override,
    )
}

// What nothing unknown gives the unknown-instance check.
const NOTHING_UNKNOWN: TaskSelectionUnknownCheck = TaskSelectionUnknownCheck {
    unknown_instances: 0,
    blocking: TaskSelectionBlocking::None,
    upper_bound: TaskSelectionUpperBound::NoneUnknown,
};

// Row K-R3a, "selected + chosen": the sealed policy wants `member-e`, card 4, which only the second
// viewport shows. The scan covers the list over the two viewports (content offsets 0 and 24, the
// end mark on the second), reads all six cards and decides `selected` with the one member, numbered
// `layout/cards#00`, with nothing unknown; the chosen member is card 4: position 4, fixed on
// viewport 1 at content row 58, with the identity `member-e`, the scan's one member.
#[test]
fn kr3a_a_scan_over_two_viewports_selects_the_wanted_card_and_names_it() {
    let task = admitted(&policy("cards-wanted", "member-e", lowest_id_last()));

    let decided = dry_run(&task, forward_run(None), None)
        .unwrap_or_else(|error| panic!("expected a decision, got {error}"));

    assert_eq!(decided.scan.coverage, ScanCoverageState::Complete);
    assert_eq!(
        decided
            .scan
            .viewports
            .iter()
            .map(|viewport| (viewport.content_offset_px, viewport.end_check))
            .collect::<Vec<_>>(),
        [(0, false), (24, true)]
    );
    assert_eq!(decided.scan.positions, 6);
    assert_eq!(
        decided.scan.gated_out,
        ["member-a", "member-b", "member-c", "member-d", "member-f"]
    );
    assert!(
        matches!(
            decided.decision.outcome,
            SelectionOutcome::Selected { count: 1 }
        ),
        "{:?}",
        decided.decision.outcome
    );
    assert_eq!(decided.decision.policy_id, "cards-wanted");
    assert_eq!(decided.decision.outcome_key, "card_selected");
    assert_eq!(decided.decision.selected, ["layout/cards#00"]);
    assert_eq!(decided.unknown_check, NOTHING_UNKNOWN);
    let chosen = decided.chosen.expect("the chosen card");
    assert_eq!(
        (
            chosen.identity.as_str(),
            chosen.position_index,
            chosen.fixed_viewport,
            chosen.content_axis_px
        ),
        ("member-e", 4, 1, 58)
    );
    assert_eq!(decided.scan.members, [chosen]);
}

// Row K-R3a, "wanted absent → empty": the sealed policy wants `member-g`, whose name no card shows.
// The scan covers the list and reads all six cards, every one gated out; the decision is `empty`
// with nothing unknown, so nothing blocks it, and nothing is chosen.
#[test]
fn kr3a_a_wanted_name_on_no_card_decides_empty_with_nothing_unknown() {
    let task = admitted(&policy("cards-wanted", "member-g", lowest_id_last()));

    let decided = dry_run(&task, forward_run(None), None)
        .unwrap_or_else(|error| panic!("expected a decision, got {error}"));

    assert_eq!(decided.scan.coverage, ScanCoverageState::Complete);
    assert_eq!(decided.scan.positions, 6);
    assert_eq!(decided.scan.gated_out, MEMBERS[..6]);
    assert!(
        decided.scan.members.is_empty(),
        "{:?}",
        decided.scan.members
    );
    assert!(
        decided.scan.unknown.is_empty(),
        "{:?}",
        decided.scan.unknown
    );
    assert_eq!(decided.decision.outcome, SelectionOutcome::Empty);
    assert_eq!(decided.decision.outcome_key, "card_empty");
    assert!(decided.decision.selected.is_empty());
    assert_eq!(decided.unknown_check, NOTHING_UNKNOWN);
    assert_eq!(decided.chosen, None);
}

// Row K-R3a, "an unread card → blocked": the sealed policy wants `member-e`, and card 4's name reads
// as nothing. The scan still covers the list; card 4 is its one unknown position (position 4, first
// read on viewport 1, no identity: `identity_unread`), every other card is gated out, and the
// decision `empty` is blocked by that unknown instance (rule 2), so nothing is chosen.
#[test]
fn kr3a_an_unreadable_card_blocks_the_empty_decision() {
    let task = admitted(&policy("cards-wanted", "member-e", lowest_id_last()));

    let decided = dry_run(&task, forward_run(Some(4)), None)
        .unwrap_or_else(|error| panic!("expected a decision, got {error}"));

    assert_eq!(decided.scan.coverage, ScanCoverageState::Complete);
    let [unknown] = decided.scan.unknown.as_slice() else {
        panic!("one unknown position: {:?}", decided.scan.unknown);
    };
    assert_eq!(
        (
            unknown.position_index,
            unknown.first_read_viewport,
            unknown.cause,
            unknown.identity.as_deref()
        ),
        (4, 1, ScanUnknownCause::IdentityUnread, None)
    );
    assert!(
        decided.scan.members.is_empty(),
        "{:?}",
        decided.scan.members
    );
    assert_eq!(decided.decision.outcome, SelectionOutcome::Empty);
    assert_eq!(decided.decision.outcome_key, "card_empty");
    assert_eq!(decided.unknown_check.unknown_instances, 1);
    assert_eq!(
        decided.unknown_check.blocking,
        TaskSelectionBlocking::UnknownInstances
    );
    assert_eq!(decided.chosen, None);
}

// Row K-R3a, "override selects another": the sealed policy wants `member-e`; a draft that wants
// `member-b`, card 1, which only the first viewport shows, is evaluated in its place. The decision
// is the draft's (its policy ID), `selected` with the one member `layout/cards#00`, nothing unknown;
// the chosen member is card 1: position 1, fixed on viewport 0 at content row 22, `member-b`.
#[test]
fn kr3a_a_draft_policy_decides_in_place_of_the_sealed_one() {
    let task = admitted(&policy("cards-wanted", "member-e", lowest_id_last()));
    let drafted = draft("cards-draft", "member-b", lowest_id_last());

    let decided = dry_run(&task, forward_run(None), Some(&drafted))
        .unwrap_or_else(|error| panic!("expected a decision, got {error}"));

    assert_eq!(decided.scan.coverage, ScanCoverageState::Complete);
    assert_eq!(decided.decision.policy_id, "cards-draft");
    assert!(
        matches!(
            decided.decision.outcome,
            SelectionOutcome::Selected { count: 1 }
        ),
        "{:?}",
        decided.decision.outcome
    );
    assert_eq!(decided.decision.selected, ["layout/cards#00"]);
    assert_eq!(decided.unknown_check, NOTHING_UNKNOWN);
    let chosen = decided.chosen.expect("the chosen card");
    assert_eq!(
        (
            chosen.identity.as_str(),
            chosen.position_index,
            chosen.fixed_viewport,
            chosen.content_axis_px
        ),
        ("member-b", 1, 0, 22)
    );
    assert_eq!(decided.scan.members, [chosen]);
}

// Row K-R3a, "override without the last tie-break refused": a draft that wants `member-b` but
// declares no tie-break key, so its last key is not `candidate_id lowest_first`, is refused as
// admission refuses such a scan step's policy: `contained_task_select_invalid`, with the admission's
// detail.
#[test]
fn kr3a_a_draft_policy_without_the_lowest_candidate_id_last_is_refused() {
    let task = admitted(&policy("cards-wanted", "member-e", lowest_id_last()));
    let drafted = draft("cards-draft", "member-b", json!([]));

    let error = dry_run(&task, forward_run(None), Some(&drafted))
        .expect_err("a scan's policy needs the lowest candidate ID as its last tie-break key");

    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::ContainedTaskSelectInvalid.as_str()
    );
    assert_eq!(
        error.detail(),
        Some("operation=pick scan tie_break=last_not_candidate_id_lowest_first")
    );
}

// Fix round K-a, the second kind of scan select, which knows the card it wants: the scan declares
// `early_stop`, the sealed policy wants `member-b`, card 1, which the first viewport already shows,
// and the frames are that viewport alone, fed twice. Once it is read, no card is unknown and no
// card not yet seen could outrank card 1 (every member scores 0, and the policy's first tie-break
// key is the lowest candidate ID first), so the scan stops there: coverage `early_stopped` over the
// one viewport at content offset 0, whose end mark is not shown; its four cards, three gated out;
// the decision `selected` with the one member `layout/cards#00`, nothing unknown; the chosen member
// card 1: position 1, fixed on viewport 0 at content row 22, `member-b`.
#[test]
fn kr3a_an_early_stop_scan_decides_on_the_first_viewport_showing_the_wanted_card() {
    let task = admitted_scanning(
        &policy("cards-wanted", "member-b", lowest_id_last()),
        early_stop_scan(),
    );

    let decided = dry_run(&task, vec![viewport(0, None), viewport(0, None)], None)
        .unwrap_or_else(|error| panic!("expected a decision, got {error}"));

    assert_eq!(decided.scan.coverage, ScanCoverageState::EarlyStopped);
    assert_eq!(
        decided
            .scan
            .viewports
            .iter()
            .map(|viewport| (viewport.content_offset_px, viewport.end_check))
            .collect::<Vec<_>>(),
        [(0, false)]
    );
    assert_eq!(decided.scan.positions, 4);
    assert_eq!(decided.scan.gated_out, ["member-a", "member-c", "member-d"]);
    assert!(
        matches!(
            decided.decision.outcome,
            SelectionOutcome::Selected { count: 1 }
        ),
        "{:?}",
        decided.decision.outcome
    );
    assert_eq!(decided.decision.outcome_key, "card_selected");
    assert_eq!(decided.decision.selected, ["layout/cards#00"]);
    assert_eq!(decided.unknown_check, NOTHING_UNKNOWN);
    let chosen = decided.chosen.expect("the chosen card");
    assert_eq!(
        (
            chosen.identity.as_str(),
            chosen.position_index,
            chosen.fixed_viewport,
            chosen.content_axis_px
        ),
        ("member-b", 1, 0, 22)
    );
    assert_eq!(decided.scan.members, [chosen]);
}

// Fix round K-a, two members: the sealed policy wants `member-b` and `member-e`, cards 1 and 4, and
// ranks the highest candidate ID first. The scan covers the list over the two viewports; its
// members, in position order, are card 1 (`layout/cards#00`, first seen on viewport 0) and card 4
// (`layout/cards#01`, on viewport 1); the decision `selected` takes `layout/cards#01`, nothing
// unknown; the chosen member is card 4, the second of the scan's members: position 4, fixed on
// viewport 1 at content row 58, `member-e`.
#[test]
fn kr3a_of_two_members_the_one_the_policy_selects_is_chosen() {
    let task = admitted(&policy_of(
        "cards-two",
        &["member-b", "member-e"],
        highest_id_first(),
    ));

    let decided = dry_run(&task, forward_run(None), None)
        .unwrap_or_else(|error| panic!("expected a decision, got {error}"));

    assert_eq!(decided.scan.coverage, ScanCoverageState::Complete);
    assert_eq!(decided.scan.positions, 6);
    assert_eq!(
        decided.scan.gated_out,
        ["member-a", "member-c", "member-d", "member-f"]
    );
    assert_eq!(
        decided
            .scan
            .members
            .iter()
            .map(|member| (member.identity.as_str(), member.position_index))
            .collect::<Vec<_>>(),
        [("member-b", 1), ("member-e", 4)]
    );
    assert!(
        matches!(
            decided.decision.outcome,
            SelectionOutcome::Selected { count: 1 }
        ),
        "{:?}",
        decided.decision.outcome
    );
    assert_eq!(decided.decision.selected, ["layout/cards#01"]);
    assert_eq!(decided.unknown_check, NOTHING_UNKNOWN);
    let chosen = decided.chosen.expect("the chosen card");
    assert_eq!(
        (
            chosen.identity.as_str(),
            chosen.position_index,
            chosen.fixed_viewport,
            chosen.content_axis_px
        ),
        ("member-e", 4, 1, 58)
    );
    assert_eq!(decided.scan.members.get(1), Some(&chosen));
}
