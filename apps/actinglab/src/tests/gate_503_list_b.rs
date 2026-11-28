// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #503 LIST-B (0.12.93; register §1.536; contracts/component-interfaces.md, the `package`
// row 29; contracts/resource-verify.md, "The `package` revision"; contracts/resource-readings.md,
// "List readings", "Offline"; contracts/lab-operations.json, the row `package list-check`).
//
// The revision: a `repeated_anchor` layout's picture pool (`identity_templates`) and its feature's
// `region` are refused by every Runtime before 0.12.93 at its declaration gate (`UnknownField` at
// the member), so a pack that declares either takes the `package` revision 29, which
// `standard-pack build`'s revision table counts, in a pack of either mode; a slot's pool keeps its
// revision.
//
// The offline check: over a package and a directory of saved frames, `package list-check` answers,
// for each position read from pictures, the mapping's evidence (the best key and its score, the
// best `other` entry by its target and its score, the threshold, the margin and the mark), in its
// JSON and as text. The package is a synthetic `neutral` step plan (`open`, `read_members` passing
// over, `close`), its frames 64 x 60, cards in one column at content y 10 + 12 k: a noise marker
// (the anchor) at x 4, a signature patch at x 12 and a picture of 6 x 4 at (17, 1) of the card's
// kind. The keys are `member-a` (picture 0) and `member-b` (picture 1); the pool holds them and the
// `other` picture 2, scored in the region (12, 0, 8, 6) at the threshold 900 and the margin 100.
// The one screen shows `member-a`, the `other` card and a card of kind 3, which the pool has no
// picture of; it is saved twice (`0001.png`, `0002.png`). The pack reads no text; the check is
// given a provider that reads none. No Runtime, no device.

use crate::cli_module::own_line;
use crate::cli_result::CliResult;
use crate::list_check_cli;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_lab::LabOcr;
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult, VisionProvider,
    VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;
use zip::{ZipWriter, write::FileOptions};

const COMMAND: &str = "package list-check";
const PACK: &str = "neutral.list";
const WIDTH: u32 = 64;
const HEIGHT: u32 = 60;
const HOME: [u8; 3] = [255, 0, 255];
const LIST: [u8; 3] = [255, 0, 0];
const DONE: [u8; 3] = [255, 255, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const BACKGROUND: [u8; 3] = [30, 31, 32];
// The kinds of the screen's three cards: `member-a`, the `other` picture, a kind without one.
const SCREEN: [u32; 3] = [0, 2, 3];

/// A zip entry: its name and its bytes.
type Entry = (String, Vec<u8>);

fn entry(name: String, value: &Value) -> Entry {
    (name, serde_json::to_vec(value).expect("entry JSON"))
}

/// The revision `content_revision` gives the zip of `entries`.
fn revision_of(entries: &[Entry]) -> u64 {
    let zip = entries.iter().cloned().collect::<BTreeMap<_, _>>();
    crate::resource_verify::revision::content_revision(&zip)
}

/// [`PACK`]'s `control.json` naming the execution mode `mode`, and its task file holding the one
/// candidate layout `layout`.
fn pack_entries(mode: &str, layout: Value) -> Vec<Entry> {
    vec![
        entry(
            format!("packs/{PACK}/control.json"),
            &json!({"execution_mode": mode}),
        ),
        entry(
            format!("packs/{PACK}/resources/operations/task/task.json"),
            &json!({"candidate_layouts": [layout]}),
        ),
    ]
}

// The row of revision 29: a layout's `identity_templates` and a feature's `region` each need 29,
// whatever they hold, in a page-graph pack and in a `linear_steps` pack alike; a layout without
// them keeps its revision (3), and so does a slot's `identity_templates`.
#[test]
fn list_b_a_layouts_picture_pool_needs_revision_29_and_nothing_else_does() {
    let pooled = json!({"kind": "repeated_anchor", "identity_templates": {"member": []},
                        "features": [{"name": "member"}]});
    let placed = json!({"kind": "repeated_anchor",
                        "features": [{"name": "member", "region": {"x": 0, "y": 0}}]});
    for mode in ["navigable_route", "linear_steps"] {
        assert_eq!(
            revision_of(&pack_entries(mode, pooled.clone())),
            29,
            "{mode}"
        );
        assert_eq!(
            revision_of(&pack_entries(mode, placed.clone())),
            29,
            "{mode}"
        );
    }
    let slotted = json!({"kind": "fixed_slots",
                         "features": [{"name": "member"}],
                         "slots": [{"identity_templates": {"member": []}}]});
    assert_eq!(
        revision_of(&pack_entries("navigable_route", slotted)),
        3,
        "a slot's pool"
    );
    let plain = json!({"kind": "repeated_anchor", "features": [{"name": "member"}]});
    assert_eq!(revision_of(&pack_entries("navigable_route", plain)), 3);
}

// Deterministic pixel noise, LIST-A's marker.
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

// The gray level of the picture of `kind` at (`x`, `y`), 16 to 239.
fn picture(kind: u32, x: u32, y: u32) -> u8 {
    let mut state = (u64::from(kind) + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (u64::from(y * 8 + x) << 20)
        ^ 0x5bd1_e995;
    state ^= state >> 33;
    state = state.wrapping_mul(0xff51_afd7_ed55_8ccd);
    state ^= state >> 33;
    state = state.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    state ^= state >> 33;
    16 + (state % 224) as u8
}

fn signature(slot: u32) -> [u8; 3] {
    let red = u8::try_from(30 + 36 * slot).expect("level");
    [red, 220 - red / 2, 90]
}

// The one screen of the list, both its marks shown: card k at content y 10 + 12 k shows the picture
// of `SCREEN[k]`.
fn pixel(x: u32, y: u32) -> [u8; 3] {
    if y == 0 && x < 4 {
        return match x {
            0 => LIST,
            1 | 3 => GREEN,
            2 => BLUE,
            _ => BACKGROUND,
        };
    }
    for (slot, kind) in (0_u32..).zip(SCREEN) {
        let top = 10 + 12 * slot;
        if !(top..top + 8).contains(&y) {
            continue;
        }
        let within = y - top;
        match x {
            4..=11 if within < 6 => return noise(x - 4, within),
            12..=15 if within < 4 => return signature(slot),
            17..=22 if (1..5).contains(&within) => return [picture(kind, x - 17, within - 1); 3],
            _ => {}
        }
    }
    BACKGROUND
}

fn encoded(width: u32, height: u32, values: Vec<u8>) -> Vec<u8> {
    Frame::from_pixels(
        width,
        height,
        values,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
    .encode_png_fast()
    .expect("fixture PNG")
}

fn screen_png() -> Vec<u8> {
    encoded(
        WIDTH,
        HEIGHT,
        (0..HEIGHT)
            .flat_map(|y| (0..WIDTH).flat_map(move |x| pixel(x, y)))
            .collect(),
    )
}

fn marker_png() -> Vec<u8> {
    encoded(
        8,
        6,
        (0..6)
            .flat_map(|y| (0..8).flat_map(move |x| noise(x, y)))
            .collect(),
    )
}

fn picture_png(kind: u32) -> Vec<u8> {
    encoded(
        6,
        4,
        (0..4)
            .flat_map(|y| (0..6).flat_map(move |x| [picture(kind, x, y); 3]))
            .collect(),
    )
}

// A provider that reads no text: the pack declares none.
#[derive(Debug)]
struct NoText;

impl VisionProvider for NoText {
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
        _request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no text in this fixture",
        ))
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

fn card_layout() -> Value {
    json!({
        "id": "layout/cards", "page_id": "neutral/list", "kind": "repeated_anchor",
        "anchor": "ui/card", "max_instances": 16, "order": "top_to_bottom",
        "instance_rect": {"x": 0, "y": 0, "width": 24, "height": 8},
        "click": {"x": 2, "y": 1, "width": 4, "height": 4},
        "readable_band": {"x": 0, "y": 4, "width": WIDTH, "height": 56},
        "unknown_identity": "reject",
        "features": [{"name": "member", "value": "identity",
                      "region": {"x": 12, "y": 0, "width": 8, "height": 6},
                      "identity": {
                          "entries": [{"id": "member-a"}, {"id": "member-b"}],
                          "recognition": {"kind": "icon_templates", "minimum_score_milli": 900,
                                          "minimum_margin_milli": 100}}}],
        "identity_templates": {"member": [
            {"id": "member-a", "target_id": "card/pic_0"},
            {"id": "member-b", "target_id": "card/pic_1"},
            {"other": true, "target_id": "card/pic_2"}
        ]}
    })
}

fn scan() -> Value {
    json!({
        "identity_feature": "member",
        "axis": "vertical",
        "advance": {"kind": "drag",
                    "from": {"x": 30, "y": 40, "width": 20, "height": 10},
                    "to": {"x": 30, "y": 14, "width": 20, "height": 10},
                    "duration_ms": 300, "min_content_step_px": 12, "max_content_step_px": 48},
        "reposition": {"kind": "drag",
                       "from": {"x": 30, "y": 50, "width": 20, "height": 10},
                       "to": {"x": 30, "y": 8, "width": 20, "height": 10},
                       "duration_ms": 300, "max_content_step_px": 56},
        "settle": {"interval_ms": 1, "timeout_ms": 1000},
        "track": {"area": {"x": 0, "y": 4, "width": WIDTH, "height": 56},
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

fn task() -> Value {
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "entry_page": "home",
        "target_page": "done",
        "max_steps": 3,
        "scheduling_outcome": {"mappings": [
            {"outcome_key": "listed", "effect": "no_designated_effect",
             "terminal_pages": ["done"]}
        ]},
        "candidate_layouts": [card_layout()],
        "operations": [
            {"id": "open", "from": "home", "to": "list",
             "click": {"kind": "point", "x": 1, "y": 1},
             "unguarded_trusted_coordinate": true,
             "expect_after": {"page_id": "list", "timeout_ms": 1000}},
            {"id": "read_members", "from": "list", "to": "list",
             "guard": {"page_id": "list", "target_id": "guard/list",
                       "expected_rect": {"x": 3, "y": 0, "width": 1, "height": 1},
                       "color_probe": "guard/list"},
             "read_list": {"layout_id": "layout/cards", "key_feature": "member",
                           "key_column": "id", "columns": [],
                           "fact_key": "inventory.list.example_kind_a",
                           "valid_for_ms": 86_400_000, "scan": scan(),
                           "others": "pass_over"}},
            {"id": "close", "from": "list", "to": "done",
             "click": {"kind": "point", "x": 40, "y": 30},
             "guard": {"page_id": "list", "target_id": "guard/end",
                       "expected_rect": {"x": 2, "y": 0, "width": 1, "height": 1},
                       "color_probe": "guard/end"},
             "expect_after": {"page_id": "done", "timeout_ms": 1000}}
        ]
    })
}

fn pack() -> Value {
    let colour = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let mut targets = vec![
        colour("page/home", 0, HOME),
        colour("page/list", 0, LIST),
        colour("page/done", 0, DONE),
        colour("check/list_start", 1, GREEN),
        colour("check/list_end", 2, BLUE),
        colour("guard/list", 3, GREEN),
        colour("guard/end", 2, BLUE),
        json!({"type": "template", "id": "ui/card", "template_path": "assets/card.png",
               "region": "full_frame", "threshold": 0.95}),
    ];
    targets.extend((0..3).map(|kind| {
        json!({"type": "template", "id": format!("card/pic_{kind}"),
               "template_path": format!("assets/pic_{kind}.png"),
               "region": "full_frame", "threshold": 0.9})
    }));
    json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed", "color_max_distance": 0.0},
        "targets": targets,
        "candidate_layouts": [card_layout()]
    })
}

fn package_zip() -> Vec<u8> {
    let json_bytes = |value: Value| serde_json::to_vec(&value).expect("JSON bytes");
    let declared = ["home", "list", "done"].map(|name| {
        json!({"id": format!("neutral/{name}"), "required": [format!("page/{name}")],
               "optional": [], "forbidden": []})
    });
    let mut entries = vec![
        (
            "control.json".to_owned(),
            json_bytes(json!({
                "schema_version": "Lab-1y.control.v2",
                "package_id": "neutral.linear.list",
                "execution_mode": "linear_steps",
                "game": "neutral",
                "server": "test",
                "resolution": {"width": WIDTH, "height": HEIGHT},
                "entry_task_id": "task",
                "capture_interval_ms": 100,
                "step_timeout_ms": 32_000,
                "timeout_ms": 300_000,
                "max_steps": 3
            })),
        ),
        (
            "resources/manifest.json".to_owned(),
            json_bytes(json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        (
            "resources/operations/task/task.json".to_owned(),
            json_bytes(task()),
        ),
        ("resources/assets/card.png".to_owned(), marker_png()),
        (
            "resources/recognition/neutral.test.pack.json".to_owned(),
            json_bytes(pack()),
        ),
        (
            "resources/recognition/neutral.test.pages.json".to_owned(),
            json_bytes(json!({"schema_version": "0.3", "pages": declared})),
        ),
    ];
    entries.extend((0..3).map(|kind| {
        (
            format!("resources/assets/pic_{kind}.png"),
            picture_png(kind),
        )
    }));
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, content) in entries {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&content).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

// The package and the frames, written to a temporary directory.
struct Fixture {
    _root: TempDir,
    package: PathBuf,
    frames: PathBuf,
}

fn written() -> Fixture {
    let root = TempDir::new().expect("a temporary directory");
    let package = root.path().join("neutral.linear.list.zip");
    fs::write(&package, package_zip()).expect("the package");
    let frames = root.path().join("frames");
    fs::create_dir(&frames).expect("the frames' directory");
    let screen = screen_png();
    for name in ["0001.png", "0002.png"] {
        fs::write(frames.join(name), &screen).expect("a frame");
    }
    Fixture {
        _root: root,
        package,
        frames,
    }
}

fn text_of(result: crate::CliOutcome<Value>) -> String {
    let exit = match result {
        Ok(answer) => own_line::success(COMMAND, answer),
        Err(error) => own_line::exit(&CliResult::err(COMMAND.to_owned(), error, true)),
    };
    list_check_cli::people(&exit).expect("the answer prints as text")
}

// The one judged picture read of `position`.
fn judged(position: &Value) -> &Value {
    let reads = position["reads"].as_array().expect("its reads");
    assert_eq!(reads.len(), 1, "{position}");
    assert_eq!(
        reads[0]["samples"],
        json!([]),
        "no text was read: {position}"
    );
    let pictures = reads[0]["pictures"].as_array().expect("its pictures");
    assert_eq!(pictures.len(), 1, "{position}");
    &pictures[0]
}

// Ruling 8 and Addendum 1, A3: the check over the one screen. The list is not whole (the card of
// kind 3, `list_position_unread`, count 1), so the check is `package_list_check_not_whole` with
// the answer in its details. Each position read from pictures carries the mapping's evidence:
// `member-a`'s card known, its best key `member-a` by `card/pic_0` at 990 or more, the threshold
// 900 and the margin 100; the `other` card passed over, none of the keys `out_of_domain`, its best
// `other` `card/pic_2` at 990 or more; the card of kind 3 unread `identity_unread` with its
// identity Unknown `low_confidence`, its read unread `low_confidence`, every score below 900. The
// text form prints a line of each read with the same evidence.
#[test]
fn list_b_the_list_check_answers_the_pictures_evidence_of_each_kind_of_position() {
    let fixture = written();
    let refused = list_check_cli::list_check(
        &fixture.package,
        &fixture.frames,
        Some("read_members"),
        &LabOcr::with_provider(Arc::new(NoText)),
    )
    .expect_err("the list is not whole");
    assert_eq!(refused.code, "package_list_check_not_whole", "{refused:?}");
    let details = refused.details.clone().expect("the answer in the details");
    let step = &details["steps"][0];
    assert_eq!(
        step["miss"],
        json!({"reading_problem": "list_position_unread", "count": 1}),
        "{step}"
    );
    assert_eq!(step["passed_over"], 1, "{step}");
    let positions = step["positions"].as_array().expect("its positions");
    assert_eq!(positions.len(), 3, "{step}");

    assert_eq!(
        positions[0]["answer"],
        json!({"kind": "known", "key": "member-a"})
    );
    let known = judged(&positions[0]);
    assert_eq!(known["mark"], "known", "{known}");
    assert_eq!(known["best_key"]["key"], "member-a", "{known}");
    assert_eq!(known["best_key"]["target_id"], "card/pic_0", "{known}");
    assert!(
        known["best_key"]["score_milli"]
            .as_u64()
            .is_some_and(|score| score >= 990),
        "{known}"
    );
    assert!(
        known["best_other"]["score_milli"]
            .as_u64()
            .is_some_and(|score| score < 900),
        "{known}"
    );
    assert_eq!(
        (
            known["minimum_score_milli"].clone(),
            known["minimum_margin_milli"].clone()
        ),
        (json!(900), json!(100))
    );

    assert_eq!(positions[1]["answer"], json!({"kind": "passed_over"}));
    let passed = judged(&positions[1]);
    assert_eq!(passed["mark"], "none_of_the_keys", "{passed}");
    assert_eq!(passed["unread_cause"], "out_of_domain", "{passed}");
    assert_eq!(passed["best_other"]["target_id"], "card/pic_2", "{passed}");
    assert!(
        passed["best_other"]["score_milli"]
            .as_u64()
            .is_some_and(|score| score >= 990),
        "{passed}"
    );

    assert_eq!(
        positions[2]["answer"],
        json!({"kind": "unread", "cause": "identity_unread",
               "identity_unknown": "low_confidence"})
    );
    let unread = judged(&positions[2]);
    assert_eq!(unread["mark"], "unread", "{unread}");
    assert_eq!(unread["unread_cause"], "low_confidence", "{unread}");
    for best in ["best_key", "best_other"] {
        assert!(
            unread[best]["score_milli"]
                .as_u64()
                .is_some_and(|score| score < 900),
            "{best}: {unread}"
        );
    }

    let text = text_of(Err(refused));
    for (position, line) in [
        (
            0,
            "    viewport 0: known picture, best key member-a (card/pic_0) at ",
        ),
        (
            1,
            "    viewport 0: none_of_the_keys picture, out_of_domain, best key ",
        ),
        (
            2,
            "    viewport 0: unread picture, low_confidence, best key ",
        ),
    ] {
        assert!(
            text.lines().any(|printed| printed.starts_with(line)
                && printed.ends_with(", threshold 900, margin 100")),
            "position {position}: {line}\n{text}"
        );
    }
    assert!(
        text.lines()
            .any(|printed| printed.contains(", best other card/pic_2 at ")),
        "{text}"
    );
}
