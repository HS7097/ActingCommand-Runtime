// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #503 LIST-A3 (0.12.91; register §1.534, rulings 3 and 4; contracts/lab-operations.json,
// the row `package list-check`; contracts/resource-readings.md, "List readings", "Offline"): the
// Lab's offline check of a list reading over saved frames. Over a package and a directory of
// saved frames it answers, for one `read_list` step by its id or for every one of the task, every
// position the scan read with the reads of its name, in its JSON and as text for a person; a step
// that is no list reading, one the task does not declare and a missing directory are usage
// errors with the table's codes; it writes nothing.
//
// The package and the frames are LIST-A's kernel fixture (`gate_503_list_a` of the execution
// kernel), defined here again and written to a temporary directory: a synthetic `neutral` step
// plan (`open`, `read_members` with `others: pass_over`, `read_every` without it, `close`), its
// frames 64 x 60, cards in one column at content y 10 + 12 k, each a noise marker (the anchor) at
// x 4, a signature patch, and a name patch a scripted OCR provider reads the name (its red level)
// and the confidence (its green level, in hundredths) from. The wanted keys are `member-a`,
// `member-b` and `member-c` (aliases `alpha`, `bravo`, `charlie`, within 1 at a margin of 1, a
// floor of 800, the limit 2). The one screen shows `alpha`, `zulu`, `cxxxxxe` (a `charlie` card
// read wrong, 5 from `charlie`, 7 from the others) and `yankee`, both marks shown, saved twice
// (`0001.png`, `0002.png`: the dispatched frame and the capture it settles on). The OCR provider
// is given to the check as a test's provider (`LabOcr::with_provider`); no Runtime, no device.

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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;
use zip::{ZipWriter, write::FileOptions};

const COMMAND: &str = "package list-check";
const WIDTH: u32 = 64;
const HEIGHT: u32 = 60;
const NAMES: [&str; 7] = [
    "alpha", "bravo", "charlie", "zulu", "yankee", "alpzz", "cxxxxxe",
];
const MEMBERS: [&str; 3] = ["member-a", "member-b", "member-c"];
const ALPHA: usize = 0;
const ZULU: usize = 3;
const YANKEE: usize = 4;
const MISREAD_CHARLIE: usize = 6;
const SURE: u8 = 99;
const HOME: [u8; 3] = [255, 0, 255];
const LIST: [u8; 3] = [255, 0, 0];
const DONE: [u8; 3] = [255, 255, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const BACKGROUND: [u8; 3] = [30, 31, 32];

// Deterministic pixel noise, the SEL-L6b gate's.
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

fn signature(slot: u32) -> [u8; 3] {
    let red = u8::try_from(30 + 36 * slot).expect("level");
    [red, 220 - red / 2, 90]
}

fn name_level(name: usize) -> u8 {
    u8::try_from(40 + 30 * name).expect("name level")
}

// The one screen of the list, both its marks shown: card k at content y 10 + 12 k reads
// `names[k]` at `SURE`.
fn pixel(names: &[usize], x: u32, y: u32) -> [u8; 3] {
    if y == 0 && x < 4 {
        return match x {
            0 => LIST,
            1 | 3 => GREEN,
            2 => BLUE,
            _ => BACKGROUND,
        };
    }
    for (slot, name) in (0_u32..).zip(names) {
        let top = 10 + 12 * slot;
        if (top..top + 6).contains(&y) && (4..12).contains(&x) {
            return noise(x - 4, y - top);
        }
        if (top..top + 4).contains(&y) && (12..16).contains(&x) {
            return signature(slot);
        }
        if (top..top + 4).contains(&y) && (16..20).contains(&x) {
            return [name_level(*name), SURE, 90];
        }
    }
    BACKGROUND
}

fn frame(width: u32, height: u32, pixels: Vec<u8>) -> Frame {
    Frame::from_pixels(
        width,
        height,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
}

// The screen as a PNG file.
fn screen_png(names: &[usize]) -> Vec<u8> {
    let pixels = (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).flat_map(move |x| pixel(names, x, y)))
        .collect();
    frame(WIDTH, HEIGHT, pixels)
        .encode_png_fast()
        .expect("screen PNG")
}

// The anchor template: the noise marker, 8 x 6 px.
fn marker_png() -> Vec<u8> {
    let pixels = (0..6)
        .flat_map(|y| (0..8).flat_map(move |x| noise(x, y)))
        .collect();
    frame(8, 6, pixels).encode_png_fast().expect("marker PNG")
}

// Reads a card's name and the read's confidence from its name patch, 12 px right of its anchor.
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
        let x = usize::try_from(request.region.x + 13).expect("x");
        let y = usize::try_from(request.region.y + 1).expect("y");
        let width = usize::try_from(request.frame.width).expect("width");
        let at = (y * width + x) * 3;
        let red = request.frame.rgb8_pixels[at];
        let hundredths = request.frame.rgb8_pixels[at + 1];
        let text = (0..NAMES.len())
            .position(|name| name_level(name) == red)
            .map_or("", |name| NAMES[name]);
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: text.to_owned(),
            blocks: Vec::new(),
            confidence: Some(f32::from(hundredths) / 100.0),
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
        "readable_band": {"x": 0, "y": 4, "width": WIDTH, "height": 56},
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

fn scan() -> Value {
    json!({
        "identity_feature": "member",
        "axis": "vertical",
        "advance": {"kind": "drag",
                    "from": {"x": 20, "y": 40, "width": 20, "height": 10},
                    "to": {"x": 20, "y": 14, "width": 20, "height": 10},
                    "duration_ms": 300, "min_content_step_px": 12, "max_content_step_px": 48},
        "reposition": {"kind": "drag",
                       "from": {"x": 20, "y": 50, "width": 20, "height": 10},
                       "to": {"x": 20, "y": 8, "width": 20, "height": 10},
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

// A list reading `id` of the list into the key `fact_key`, with `others` when it is given.
fn reading(id: &str, fact_key: &str, others: Option<Value>) -> Value {
    let mut read = json!({"layout_id": "layout/cards", "key_feature": "member",
                          "key_column": "id", "columns": [], "fact_key": fact_key,
                          "valid_for_ms": 86_400_000, "scan": scan()});
    if let Some(others) = others {
        read["others"] = others;
    }
    json!({
        "id": id, "from": "list", "to": "list",
        "guard": {"page_id": "list", "target_id": "guard/list",
                  "expected_rect": {"x": 3, "y": 0, "width": 1, "height": 1},
                  "color_probe": "guard/list"},
        "read_list": read
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
        "max_steps": 4,
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
            reading("read_members", "inventory.list.example_kind_a", Some(json!("pass_over"))),
            reading("read_every", "inventory.list.example_kind_b", None),
            {"id": "close", "from": "list", "to": "done",
             "click": {"kind": "point", "x": 30, "y": 30},
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
    json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed", "color_max_distance": 0.0},
        "targets": [
            colour("page/home", 0, HOME),
            colour("page/list", 0, LIST),
            colour("page/done", 0, DONE),
            colour("check/list_start", 1, GREEN),
            colour("check/list_end", 2, BLUE),
            colour("guard/list", 3, GREEN),
            colour("guard/end", 2, BLUE),
            {"type": "template", "id": "ui/card", "template_path": "assets/card.png",
             "region": "full_frame", "threshold": 0.95},
            {"type": "ocr", "id": "card/name",
             "region": {"x": 0, "y": 0, "width": 16, "height": 4},
             "languages": ["en"], "timeout_ms": 2000, "match_mode": "exact",
             "expected": ["alpha"], "case_sensitive": true, "minimum_confidence": 0.1,
             "model_ref": "PP-OCRv6_medium", "model_sha256": "a".repeat(64)}
        ],
        "candidate_layouts": [card_layout()]
    })
}

// The package, a zip of the kernel's form.
fn package_zip() -> Vec<u8> {
    let json_bytes = |value: Value| serde_json::to_vec(&value).expect("JSON bytes");
    let pages = ["home", "list", "done"].map(|page| {
        json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")],
               "optional": [], "forbidden": []})
    });
    let entries = [
        (
            "control.json",
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
                "max_steps": 4
            })),
        ),
        (
            "resources/manifest.json",
            json_bytes(json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        ("resources/operations/task/task.json", json_bytes(task())),
        ("resources/assets/card.png", marker_png()),
        (
            "resources/recognition/neutral.test.pack.json",
            json_bytes(pack()),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json_bytes(json!({"schema_version": "0.3", "pages": pages})),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, content) in entries {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&content).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

// Every file under `root`, by its path, with its bytes.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).expect("a directory of the snapshot") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let bytes = fs::read(&path).expect("a file of the snapshot");
                files.insert(path, bytes);
            }
        }
    }
    files
}

// The package and the frames, written to a temporary directory.
struct Fixture {
    root: TempDir,
    package: PathBuf,
    frames: PathBuf,
}

fn written() -> Fixture {
    let root = TempDir::new().expect("a temporary directory");
    let package = root.path().join("neutral.linear.list.zip");
    fs::write(&package, package_zip()).expect("the package");
    let frames = root.path().join("frames");
    fs::create_dir(&frames).expect("the frames' directory");
    let screen = screen_png(&[ALPHA, ZULU, MISREAD_CHARLIE, YANKEE]);
    for name in ["0001.png", "0002.png"] {
        fs::write(frames.join(name), &screen).expect("a frame");
    }
    Fixture {
        root,
        package,
        frames,
    }
}

// The check with the scripted provider.
fn check(fixture: &Fixture, step: Option<&str>) -> crate::CliOutcome<Value> {
    list_check_cli::list_check(
        &fixture.package,
        &fixture.frames,
        step,
        &LabOcr::with_provider(Arc::new(CardNames)),
    )
}

// The text for a person of the answer `result` of the check.
fn text_of(result: crate::CliOutcome<Value>) -> String {
    let exit = match result {
        Ok(answer) => own_line::success(COMMAND, answer),
        Err(error) => own_line::exit(&CliResult::err(COMMAND.to_owned(), error, true)),
    };
    list_check_cli::people(&exit).expect("the answer prints as text")
}

// One step by its id and every step: `read_members` passes over, so its list is whole and holds
// `member-a` alone; the misread card is listed passed over with its text and its nearest name, and
// the wanted keys no card showed are `member-b` and `member-c`; the answer is the check's success
// and its text says so. Every step checks `read_members` and then `read_every`, which declares no
// `others`, in declaration order over the same frames: its list is not whole (the three other
// cards unread, `list_position_unread` count 3), so the check is `package_list_check_not_whole`
// with both steps in its details, and its text names the code first and the unread list's miss.
// Nothing is written: every file under the temporary directory that holds the package and the
// frames is the same, by its path and its bytes, before and after both checks, and no file is
// added there; the answers say that nothing was executed and no ledger was written.
#[test]
fn list_a3_the_check_answers_one_step_and_every_step_as_json_and_as_text_and_writes_nothing() {
    let fixture = written();
    let before = snapshot(fixture.root.path());

    let answer = check(&fixture, Some("read_members"))
        .unwrap_or_else(|error| panic!("read_members: the check answers: {error:?}"));
    assert_eq!(answer["whole"], true, "{answer}");
    assert_eq!(answer["executed"], false, "{answer}");
    assert_eq!(
        answer["production_global_ledger_written"], false,
        "{answer}"
    );
    assert_eq!(answer["frames_given"], 2, "{answer}");
    let steps = answer["steps"].as_array().expect("its steps");
    assert_eq!(steps.len(), 1, "{answer}");
    let step = &steps[0];
    assert_eq!(step["operation_id"], "read_members");
    assert_eq!(step["status"], "whole");
    assert_eq!(step["passed_over"], 3);
    assert_eq!(
        step["list"]["rows"],
        json!([{"id": {"type": "string", "value": "member-a"}}])
    );
    assert_eq!(
        step["summary"],
        json!({"whole": true, "positions": 4, "passed_over": 3, "unread": 0,
               "wanted_shown": ["member-a"], "wanted_not_shown": ["member-b", "member-c"]})
    );
    let misread = &step["positions"][2];
    assert_eq!(misread["position_index"], 2, "{misread}");
    assert_eq!(
        misread["answer"],
        json!({"kind": "passed_over"}),
        "{misread}"
    );
    assert_eq!(misread["reads"][0]["viewport"], 0, "{misread}");
    assert_eq!(misread["reads"][0]["mark"], "none_of_the_keys", "{misread}");
    assert_eq!(
        misread["reads"][0]["samples"][0],
        json!({"text": "cxxxxxe", "confidence_milli": 990, "mark": "none_of_the_keys",
               "unread_cause": "beyond_distance",
               "nearest": {"key": "member-c", "alias": "charlie", "distance": 5, "limit": 2}}),
        "{misread}"
    );
    assert_eq!(
        step["positions"][0]["answer"],
        json!({"kind": "known", "key": "member-a"})
    );
    let text = text_of(Ok(answer.clone()));
    for line in [
        "package list-check: package neutral.linear.list, task task, 2 frames: every list whole",
        "step read_members (inventory.list.example_kind_a): whole, 4 positions read, 3 passed \
         over, 0 unread",
        "  wanted keys a card showed: member-a",
        "  wanted keys no card showed: member-b, member-c",
        "  position 0 (axis 10, cross 4): known member-a",
        "  position 2 (axis 34, cross 4): passed over",
        "    viewport 0: none_of_the_keys \"cxxxxxe\", confidence 990, beyond_distance, nearest \
         member-c \"charlie\" at 5, limit 2",
    ] {
        assert!(
            text.lines().any(|printed| printed == line),
            "{line}\n{text}"
        );
    }

    let refused = check(&fixture, None).expect_err("every step: not every list is whole");
    assert_eq!(refused.code, "package_list_check_not_whole", "{refused:?}");
    let details = refused.details.clone().expect("the answer in the details");
    assert_eq!(details["whole"], false, "{details}");
    let every = details["steps"]
        .as_array()
        .expect("its steps")
        .iter()
        .map(|step| {
            (
                step["operation_id"].clone(),
                step["status"].clone(),
                step["miss"].clone(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        every,
        vec![
            (json!("read_members"), json!("whole"), Value::Null),
            (
                json!("read_every"),
                json!("not_whole"),
                json!({"reading_problem": "list_position_unread", "count": 3})
            ),
        ]
    );
    for member in ["status", "list", "summary", "positions"] {
        assert_eq!(
            details["steps"][0][member], answer["steps"][0][member],
            "the step by its id answers its {member} as it does among every step"
        );
    }
    let text = text_of(Err(refused));
    assert!(text.starts_with("package_list_check_not_whole: "), "{text}");
    assert!(
        text.lines().any(|line| line
            == "step read_every (inventory.list.example_kind_b): not whole (list_position_unread, \
                count 3), 4 positions read, 0 passed over, 3 unread"),
        "{text}"
    );

    assert_eq!(
        snapshot(fixture.root.path()),
        before,
        "nothing is written beside the package and the frames, and neither is changed"
    );
}

// The usage errors, with the table's codes: a step that is no list reading (`close`) is
// `validation_failed` naming `--step` as `unusable`, a step the task does not declare `--step`
// `not_recognised`, each refused before any frame is read; and a frames' directory that does not
// exist is `replay_frames_unreadable`, on actinglab's own command line (exit 2 on its envelope),
// before the package is read, so no OCR provider is needed for it.
#[test]
fn list_a3_a_step_that_is_no_list_reading_and_a_missing_directory_are_usage_errors() {
    let fixture = written();
    for (step, problem) in [("close", "unusable"), ("missing_step", "not_recognised")] {
        let refused = check(&fixture, Some(step)).expect_err(step);
        assert_eq!(refused.code, "validation_failed", "{step}: {refused:?}");
        let details = refused.details.clone().unwrap_or(Value::Null);
        assert_eq!(details["argument"], "--step", "{step}: {details}");
        assert_eq!(details["argument_problem"], problem, "{step}: {details}");
    }

    let _guard = super::env_lock();
    let _app_env = super::set_isolated_app_env();
    super::set_missing_config_env();
    let package = fixture.package.display().to_string();
    let missing = fixture.root.path().join("no_frames").display().to_string();
    let result = crate::run_cli(
        [
            "--json",
            "package",
            "list-check",
            "--package",
            package.as_str(),
            "--frames",
            missing.as_str(),
        ],
        true,
    );
    let shown = result.envelope_json();
    assert_eq!(result.exit_code(), 2, "{shown}");
    assert!(result.envelope.data.is_none(), "{shown}");
    let error = result.envelope.error.as_ref().expect("refused");
    assert_eq!(error.code, "replay_frames_unreadable", "{shown}");
}
