// SPDX-License-Identifier: AGPL-3.0-only

// 0.12.28 LAB-SMALL (coordinator work order rd-1029/brief-wave2d.md; triage rd-1028/triage-p71.md
// B47; inventory rd-1028/inventory-012.md P60, P61): offline `observe`.
// - B47: a candidate layout of the recognised page whose feature needs an OCR model the Tools do
//   not hold is not projected; the page is still answered, and `candidate_not_projected` names
//   the layout, the item and the code (`candidate_feature_provider_missing`), with the OCR state.
// - P60 (#411 A69): an answer refused as `observe_identity_limit_exceeded` writes no
//   `--with-frame` file; an answer given writes it.
// - P61 (#411 A70): a page not recognised is never `standby`, neither in the answer nor in its
//   projection.
// Packages and frames are synthetic and neutral; no Runtime, device or OCR model is involved.

use super::*;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

/// The first pixel of a frame tells its page; the second shows the button.
const ENTRY: [u8; 3] = [40, 90, 160];
const OTHER: [u8; 3] = [90, 90, 90];
const BUTTON: [u8; 3] = [60, 200, 60];

const ENTRY_PAGE: &str = "neutral/entry_page";

/// A context with no configuration, no recording session and an empty Runtime state root, and a
/// directory for the packages and frames. The fields drop in order, the environment lock last.
struct Isolated {
    files: TempDir,
    _runtime_env: RuntimeStateEnvGuard,
    _runtime: TempDir,
    _app_env: TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Isolated {
    fn new() -> Self {
        let guard = env_lock();
        let app_env = set_isolated_app_env();
        set_missing_config_env();
        unsafe {
            env::remove_var(SESSION_STATE_ENV);
        }
        let runtime = TempDir::new().unwrap();
        let runtime_env = use_runtime_state_root(runtime.path());
        Self {
            _guard: guard,
            _app_env: app_env,
            _runtime_env: runtime_env,
            _runtime: runtime,
            files: TempDir::new().unwrap(),
        }
    }

    /// The two-pixel frame `name`: `page`, then the button.
    fn scene(&self, name: &str, page: [u8; 3]) -> String {
        let png = Frame::from_pixels(
            2,
            1,
            [page, BUTTON].concat(),
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("fixture frame")
        .encode_png_fast()
        .expect("fixture PNG");
        let path = self.files.path().join(name);
        fs::write(&path, png).unwrap();
        path.display().to_string()
    }

    /// The package `name`: its one page told by the first pixel; with `ocr_layout` the page has
    /// the fixed-slot layout `choices`, whose feature `label` reads the OCR target `text/label`.
    fn package(&self, name: &str, ocr_layout: bool) -> String {
        let mut pack = json!({
            "schema_version": "0.7",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": 2, "height": 1},
            "defaults": {"color_max_distance": 0.0},
            "targets": [
                {"type": "color", "id": "page/entry_page",
                 "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": ENTRY},
                {"type": "color", "id": "ui/button",
                 "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": BUTTON}
            ]
        });
        if ocr_layout {
            pack["targets"]
                .as_array_mut()
                .expect("targets")
                .push(json!({
                    "type": "ocr", "id": "text/label",
                    "region": {"x": 1, "y": 0, "width": 1, "height": 1},
                    "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact",
                    "expected": ["open"], "case_sensitive": false, "minimum_confidence": 0.9,
                    "model_ref": "neutral-ocr", "model_sha256": "a".repeat(64)
                }));
            let slot = json!({"x": 1, "y": 0, "width": 1, "height": 1});
            pack["candidate_layouts"] = json!([{
                "id": "choices",
                "page_id": ENTRY_PAGE,
                "kind": "fixed_slots",
                "sample_interval_ms": 0,
                "features": [
                    {"name": "ready", "value": "passed"},
                    {"name": "label", "value": "passed"}
                ],
                "slots": [{"rect": slot, "click": slot,
                           "targets": {"ready": "ui/button", "label": "text/label"}}]
            }]);
        }
        let bytes = zip_entries(&[
            (
                "control.json",
                json!({"game": "neutral", "server": "test", "entry_task_id": "task"}),
            ),
            (
                "resources/manifest.json",
                json!({"schema_version": "0.3", "entry_task_id": "task"}),
            ),
            ("resources/operations/task/task.json", json!({})),
            ("resources/recognition/neutral.test.pack.json", pack),
            (
                "resources/recognition/neutral.test.pages.json",
                json!({"schema_version": "0.3", "pages": [
                    {"id": ENTRY_PAGE, "required": ["page/entry_page"]}
                ]}),
            ),
        ]);
        let path = self.files.path().join(name);
        fs::write(&path, bytes).unwrap();
        path.display().to_string()
    }
}

fn zip_entries(entries: &[(&str, Value)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in entries {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec(value).expect("JSON"))
            .expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

/// `--json observe <extra> --scene <scene> --zip <package>`: `extra` follows the command word, so
/// an observe option such as `--with-frame` reaches observe and is not read as the command.
fn observe(extra: &[&str], scene: &str, package: &str) -> CliResult {
    let mut line = vec!["--json".to_string(), "observe".to_string()];
    line.extend(extra.iter().map(|word| (*word).to_string()));
    line.extend(["--scene", scene, "--zip", package].map(str::to_string));
    run_cli(line, true)
}

/// The answer is a success; its data.
#[track_caller]
fn data(result: &CliResult, what: &str) -> Value {
    let shown = result.envelope_json();
    assert_eq!(result.exit_code(), 0, "{what}: {shown}");
    result
        .envelope
        .data
        .clone()
        .unwrap_or_else(|| panic!("{what}: no data: {shown}"))
}

#[test]
fn b47_a_layout_whose_ocr_feature_has_no_model_is_named_and_the_page_still_answered() {
    let context = Isolated::new();
    let package = context.package("ocr-layout.zip", true);
    let scene = context.scene("entry.png", ENTRY);

    let observed = data(
        &observe(&["--verbose"], &scene, &package),
        "observe of a page whose layout needs OCR",
    );
    assert_eq!(observed["page"], ENTRY_PAGE, "{observed}");
    assert_eq!(observed["matched"], true, "{observed}");
    assert_eq!(observed["observation"]["page"], ENTRY_PAGE, "{observed}");
    assert!(observed.get("candidate_sets").is_none(), "{observed}");
    let not_projected = observed["candidate_not_projected"]
        .as_array()
        .unwrap_or_else(|| panic!("no candidate_not_projected: {observed}"));
    assert_eq!(not_projected.len(), 1, "{observed}");
    assert_eq!(not_projected[0]["layout_id"], "choices", "{observed}");
    assert_eq!(not_projected[0]["item"], "features", "{observed}");
    assert_eq!(
        not_projected[0]["code"], "candidate_feature_provider_missing",
        "{observed}"
    );
    assert!(
        not_projected[0]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("text/label")),
        "{observed}"
    );
    assert_eq!(observed["ocr"]["available"], false, "{observed}");
}

#[test]
fn p60_a_refused_answer_writes_no_frame_file_and_a_given_one_does() {
    let context = Isolated::new();
    let package = context.package("plain.zip", false);
    let scene = context.scene("entry.png", ENTRY);

    // A path of more than 2 KiB: Min's identity, which keeps `frame_path` whole, no longer fits
    // its hard byte limit.
    let mut long = context.files.path().join("frames");
    for index in 0..12 {
        long.push(format!("{index:02}{}", "d".repeat(180)));
    }
    let long = long.join("frame.png");
    assert!(long.as_os_str().len() > 2200);
    let refused = observe(
        &["--with-frame", &long.display().to_string()],
        &scene,
        &package,
    );
    let shown = refused.envelope_json();
    assert_eq!(refused.exit_code(), 2, "{shown}");
    assert_eq!(
        refused
            .envelope
            .error
            .as_ref()
            .map(|error| error.code.as_str()),
        Some("observe_identity_limit_exceeded"),
        "{shown}"
    );
    assert!(!long.exists(), "a refused answer wrote {}", long.display());
    assert!(
        !context.files.path().join("frames").exists(),
        "a refused answer created the frame directory"
    );

    let short = context.files.path().join("kept").join("frame.png");
    let given = data(
        &observe(
            &["--with-frame", &short.display().to_string()],
            &scene,
            &package,
        ),
        "observe --with-frame with a short path",
    );
    assert_eq!(given["frame_path"], short.display().to_string(), "{given}");
    assert!(short.is_file(), "{given}");
}

#[test]
fn p61_a_page_not_recognised_is_never_standby() {
    let context = Isolated::new();
    let package = context.package("plain.zip", false);
    let scene = context.scene("other.png", OTHER);

    let observed = data(
        &observe(&["--verbose"], &scene, &package),
        "observe of a frame no page matches",
    );
    assert_eq!(observed["matched"], false, "{observed}");
    assert_eq!(observed["page"], "unknown", "{observed}");
    assert_eq!(observed["standby"], false, "{observed}");
    assert_eq!(observed["observation"]["matched"], false, "{observed}");
    assert_eq!(observed["observation"]["standby"], false, "{observed}");
}
