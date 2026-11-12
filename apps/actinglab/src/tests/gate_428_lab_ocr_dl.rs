// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #428 LAB-OCR-DL (register §1.331; coordinator work order
// rd-1112/brief-lab-ocr-deadline.md; the coordinator's rulings of 11-12 on board #428, not the
// owner's words): the switch `--ocr-deadline-max` of an offline evaluation of saved scenes.
// - `lab_ocr_dl_the_answer_names_the_raised_deadline_only_with_the_switch`: on a saved scene, each
//   of the ten rows that takes the switch (`observe`, `do --dry-run`, `ensure --dry-run`, `wait`,
//   `recognize`, `is-visible`, `detect-page`, `current-page`, `tap-target --dry-run` and `navigate
//   --dry-run`) answers with the member `ocr_deadline_raised_to_ms` 60000 when it is given and with
//   no such member when it is not; `recognize`'s answer, which holds no identifier, is otherwise
//   the same.
// - `lab_ocr_dl_the_switch_off_a_saved_scene_is_refused`: given beside `--capture` (`observe`,
//   `do`, `do --dry-run`, `ensure`, `ensure --dry-run`, `wait`) or with no `--scene` (the four verbs
//   offline and the six readers), the switch is refused before any package is loaded or any frame
//   read: `validation_failed` (exit 2) naming it, `argument_problem` `conflicting`.
// The package and the frame are synthetic and neutral and hold no OCR target; no Runtime, no
// device, no OCR model, no clock.

use super::*;
use crate::offline_ocr::{OCR_DEADLINE_MAX_FLAG, OCR_DEADLINE_MEMBER};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

/// The first pixel of the frame tells its page, the second shows the button.
const ENTRY: [u8; 3] = [40, 90, 160];
const PAGE_A: [u8; 3] = [200, 60, 60];
const BUTTON: [u8; 3] = [60, 200, 60];
const BLANK: [u8; 3] = [0, 0, 0];

const ENTRY_PAGE: &str = "neutral/entry_page";

/// A context with no configuration, no recording session and an empty Runtime state root, the
/// saved scene (the entry page with its button) and the package. The fields drop in order, the
/// environment lock last.
struct Isolated {
    scene: String,
    package: String,
    _files: TempDir,
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
        let files = TempDir::new().unwrap();
        let scene = files.path().join("scene.png");
        let png = Frame::from_pixels(
            3,
            1,
            [ENTRY, BUTTON, BLANK].concat(),
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("fixture frame")
        .encode_png_fast()
        .expect("fixture PNG");
        fs::write(&scene, png).unwrap();
        let package = files.path().join("package.zip");
        fs::write(&package, package_bytes()).unwrap();
        Self {
            scene: scene.display().to_string(),
            package: package.display().to_string(),
            _files: files,
            _runtime_env: runtime_env,
            _runtime: runtime,
            _app_env: app_env,
            _guard: guard,
        }
    }
}

/// The package: its pages told apart by the frame's first pixel (`entry_page`, `page_a`); its
/// targets on the second pixel are the same button, `ui/button` guarding `press_button` and
/// `ui/free` guarding nothing; one navigation edge from the entry page to `page_a`.
fn package_bytes() -> Vec<u8> {
    let colour = |id: &str, x: i32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let button = |id: &str| {
        let mut target = colour(id, 1, BUTTON);
        target["click"] = json!({"x": 1, "y": 0, "width": 1, "height": 1});
        target
    };
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 3, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            colour("page/entry_page", 0, ENTRY),
            colour("page/page_a", 0, PAGE_A),
            button("ui/button"),
            button("ui/free")
        ]
    });
    zip_entries(&[
        (
            "control.json",
            json!({"game": "neutral", "server": "test", "entry_task_id": "task"}),
        ),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        (
            "resources/operations/task/task.json",
            json!({
                "schema_version": "0.6",
                "task_id": "task",
                "game": "neutral",
                "server_scope": ["test"],
                "goal": "neutral fixture",
                "coordinate_space": {"width": 3, "height": 1},
                "entry_page": "entry_page",
                "target_page": "page_a",
                "operations": [{
                    "id": "press_button",
                    "purpose": "neutral fixture",
                    "from": "entry_page",
                    "to": "page_a",
                    "click": {"kind": "point", "x": 1, "y": 0},
                    "guard": {
                        "page_id": "entry_page",
                        "target_id": "ui/button",
                        "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                        "color_probe": "ui/button"
                    }
                }]
            }),
        ),
        ("resources/recognition/neutral.test.pack.json", pack),
        (
            "resources/recognition/neutral.test.pages.json",
            json!({"schema_version": "0.3", "pages": [
                {"id": ENTRY_PAGE, "required": ["page/entry_page"]},
                {"id": "neutral/page_a", "required": ["page/page_a"]}
            ]}),
        ),
        (
            "resources/navigation/neutral.test.navigation.json",
            json!({
                "schema_version": "0.3",
                "game": "neutral",
                "server": "test",
                "navigation": [{
                    "id": "open_page_a",
                    "from_page": ENTRY_PAGE,
                    "to_page": "neutral/page_a",
                    "click": {"kind": "rect", "x": 1, "y": 0, "width": 1, "height": 1}
                }],
                "destructive_actions": []
            }),
        ),
    ])
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

/// The ten rows that take the switch, each by the words of a line that evaluates the saved scene
/// and answers.
const ROWS: [&[&str]; 10] = [
    &["observe"],
    &["--dry-run", "do", "ui/free"],
    &["--dry-run", "ensure", ENTRY_PAGE],
    &["wait", "--page", ENTRY_PAGE, "--timeout-ms", "0"],
    &["recognize", "--target", "ui/free"],
    &["is-visible", "ui/free"],
    &["detect-page"],
    &["current-page"],
    &["--dry-run", "tap-target", "ui/free"],
    &["--dry-run", "navigate", "--to", ENTRY_PAGE],
];

/// The answer of `words` on the saved scene with the package, with the switch when `raised`: a
/// success; its data.
#[track_caller]
fn answered(context: &Isolated, words: &[&str], raised: bool) -> Value {
    let mut line = vec!["--json".to_owned()];
    line.extend(words.iter().map(|word| (*word).to_owned()));
    line.extend(["--scene".to_owned(), context.scene.clone()]);
    if raised {
        line.push(OCR_DEADLINE_MAX_FLAG.to_owned());
    }
    line.extend(["--zip".to_owned(), context.package.clone()]);
    let result = run_cli(line, true);
    let shown = result.envelope_json();
    assert_eq!(result.exit_code(), 0, "{words:?}, raised {raised}: {shown}");
    result
        .envelope
        .data
        .clone()
        .unwrap_or_else(|| panic!("{words:?}, raised {raised}: no data: {shown}"))
}

#[test]
fn lab_ocr_dl_the_answer_names_the_raised_deadline_only_with_the_switch() {
    let context = Isolated::new();
    for words in ROWS {
        let with = answered(&context, words, true);
        assert_eq!(
            with[OCR_DEADLINE_MEMBER],
            json!(60_000),
            "{words:?}: {with}"
        );
        let without = answered(&context, words, false);
        assert!(
            without.get(OCR_DEADLINE_MEMBER).is_none(),
            "{words:?}: {without}"
        );
    }
    // The answer `recognize` gives holds no identifier a run mints: with the switch it is the
    // answer without it and the member.
    let words = ROWS[4];
    let mut with = answered(&context, words, true);
    let member = with
        .as_object_mut()
        .expect("recognize answers an object")
        .remove(OCR_DEADLINE_MEMBER);
    assert_eq!(member, Some(json!(60_000)), "{words:?}");
    assert_eq!(with, answered(&context, words, false), "{words:?}");
}

/// `words` with the switch, refused before anything is loaded: `validation_failed` (exit 2)
/// naming the switch as `conflicting`, and no data. `detect-page`, `tap-target` and `navigate`
/// carry the refusal's details inside their trace's.
#[track_caller]
fn refused_off_a_saved_scene(context: &Isolated, words: &[&str]) {
    let mut line = vec!["--json".to_owned()];
    line.extend(words.iter().map(|word| (*word).to_owned()));
    line.push(OCR_DEADLINE_MAX_FLAG.to_owned());
    line.extend(["--zip".to_owned(), context.package.clone()]);
    let result = run_cli(line, true);
    let shown = result.envelope_json();
    assert_eq!(result.exit_code(), 2, "{words:?}: {shown}");
    assert!(result.envelope.data.is_none(), "{words:?}: {shown}");
    let error = result
        .envelope
        .error
        .as_ref()
        .unwrap_or_else(|| panic!("{words:?} was not refused: {shown}"));
    assert_eq!(error.code, "validation_failed", "{words:?}: {shown}");
    let details = error.details.clone().unwrap_or(Value::Null);
    let named = if details.get("blocked_error").is_some() {
        &details["details"]
    } else {
        &details
    };
    assert_eq!(
        named,
        &json!({"argument": OCR_DEADLINE_MAX_FLAG, "argument_problem": "conflicting"}),
        "{words:?}: {shown}"
    );
}

#[test]
fn lab_ocr_dl_the_switch_off_a_saved_scene_is_refused() {
    let context = Isolated::new();
    let scene = context.scene.clone();
    let scene = scene.as_str();
    // A live frame: beside `--capture`, also where `--scene` names a saved one too.
    let live: [&[&str]; 7] = [
        &["observe", "--capture"],
        &["observe", "--capture", "--scene", scene],
        &["--dry-run", "do", "ui/free", "--capture"],
        &["do", "--tap", "1,0", "--capture"],
        &["--dry-run", "ensure", ENTRY_PAGE, "--capture"],
        &["ensure", ENTRY_PAGE, "--capture"],
        &["wait", "--page", ENTRY_PAGE, "--capture"],
    ];
    // No frame: the four verbs offline and the six readers without `--scene`.
    let none: [&[&str]; 10] = [
        &["observe"],
        &["--dry-run", "do", "ui/free"],
        &["--dry-run", "ensure", ENTRY_PAGE],
        &["wait", "--page", ENTRY_PAGE],
        &["recognize", "--target", "ui/free"],
        &["is-visible", "ui/free"],
        &["detect-page"],
        &["current-page"],
        &["--dry-run", "tap-target", "ui/free"],
        &["--dry-run", "navigate", "--to", ENTRY_PAGE],
    ];
    for words in live.iter().chain(none.iter()) {
        refused_off_a_saved_scene(&context, words);
    }
}
