// SPDX-License-Identifier: AGPL-3.0-only

// #411 Lab L7 (G12; plan-lab-backends §4 L7 R1, R2; register §1.122; exec gate key 100): the Lab
// decides a frame's page and guards `do` as the Runtime does.
// - R1: `do`, `ensure` and `wait` (here offline, with `--scene`) refuse a frame that two pages
//   match with `page_recognition_conflict` (exit 2), its details naming both pages, as
//   `current-page` does, instead of taking the first of them; one matched page is decided.
// - R2: offline `do --scene --dry-run` guards a target that an operation's guard names as the
//   kernel guards that operation before its input: the frame's page must be the guard's page
//   (`any`: every page) before the target is evaluated; a bare target is evaluated as before.
// Every expected value comes from those requirement lines. Packages and frames are synthetic and
// neutral, and no Runtime or device is involved.

use super::*;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

/// The colour of a frame's first pixel, which tells its page, and of its second pixel, where the
/// button shows or does not.
const ENTRY: [u8; 3] = [40, 90, 160];
const PAGE_A: [u8; 3] = [200, 60, 60];
const BUTTON: [u8; 3] = [60, 200, 60];
const BLANK: [u8; 3] = [0, 0, 0];

/// The two pages a frame of the entry colour matches in the package with the twin page, in page
/// order.
const TWIN_PAGES: [&str; 2] = ["neutral/entry_page", "neutral/entry_twin"];

/// A context with no configuration, no recording session and an empty Runtime state root, and a
/// directory for the package and frames. The fields drop in order, the environment lock last.
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

    /// A 2x1 frame written as `name`: `page` at (0, 0) and `button` at (1, 0).
    fn frame(&self, name: &str, page: [u8; 3], button: [u8; 3]) -> String {
        let png = Frame::from_pixels(
            2,
            1,
            [page, button].concat(),
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

    /// The package, written as `name`. Its pages are told apart by the frame's first pixel:
    /// `entry_page` and `page_a`, and with `twin` also `entry_twin`, which the entry colour matches
    /// as well. Its three targets on the second pixel are the same button: `ui/button` guards
    /// `press_button`, which presses on `entry_page`; `ui/anywhere` guards `press_anywhere`, whose
    /// guard names every page (`any`); `ui/free` guards no operation.
    fn package(&self, name: &str, twin: bool) -> String {
        let colour = |id: &str, x: i32, expected: [u8; 3]| {
            json!({"type": "color", "id": id,
                   "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
        };
        let button = |id: &str| {
            let mut target = colour(id, 1, BUTTON);
            target["click"] = json!({"x": 1, "y": 0, "width": 1, "height": 1});
            target
        };
        let page =
            |id: &str| json!({"id": format!("neutral/{id}"), "required": [format!("page/{id}")]});
        let mut pages = vec![page("entry_page")];
        if twin {
            pages.push(page("entry_twin"));
        }
        pages.push(page("page_a"));
        let guarded = |id: &str, from: &str, to: &str, target: &str| {
            json!({
                "id": id,
                "purpose": "neutral fixture",
                "from": from,
                "to": to,
                "click": {"kind": "point", "x": 1, "y": 0},
                "guard": {
                    "page_id": from,
                    "target_id": target,
                    "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                    "color_probe": target
                }
            })
        };
        let bytes = zip_entries(&[
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
                    "coordinate_space": {"width": 2, "height": 1},
                    "entry_page": "entry_page",
                    "target_page": "page_a",
                    "operations": [
                        guarded("press_button", "entry_page", "page_a", "ui/button"),
                        guarded("press_anywhere", "any", "entry_page", "ui/anywhere")
                    ]
                }),
            ),
            (
                "resources/recognition/neutral.test.pack.json",
                json!({
                    "schema_version": "0.3",
                    "game": "neutral",
                    "server": "test",
                    "coordinate_space": {"width": 2, "height": 1},
                    "defaults": {"color_max_distance": 0.0},
                    "targets": [
                        colour("page/entry_page", 0, ENTRY),
                        colour("page/entry_twin", 0, ENTRY),
                        colour("page/page_a", 0, PAGE_A),
                        button("ui/button"),
                        button("ui/anywhere"),
                        button("ui/free")
                    ]
                }),
            ),
            (
                "resources/recognition/neutral.test.pages.json",
                json!({"schema_version": "0.3", "pages": pages}),
            ),
            (
                "resources/navigation/neutral.test.navigation.json",
                json!({
                    "schema_version": "0.3",
                    "game": "neutral",
                    "server": "test",
                    "navigation": [{
                        "id": "open_page_a",
                        "from_page": "neutral/entry_page",
                        "to_page": "neutral/page_a",
                        "click": {"kind": "rect", "x": 1, "y": 0, "width": 1, "height": 1}
                    }],
                    "destructive_actions": []
                }),
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

/// Runs `words` with `--json` and the package.
fn run(words: &[&str], package: &str) -> CliResult {
    let line = ["--json"]
        .iter()
        .chain(words)
        .chain(["--zip", package].iter())
        .map(|word| (*word).to_string())
        .collect::<Vec<_>>();
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

/// The answer is a refusal with `code` and `exit`; its details.
#[track_caller]
fn refusal(result: &CliResult, code: &str, exit: i32, what: &str) -> Value {
    let shown = result.envelope_json();
    assert_eq!(result.exit_code(), exit, "{what}: {shown}");
    let error = result
        .envelope
        .error
        .as_ref()
        .unwrap_or_else(|| panic!("{what} was not refused: {shown}"));
    assert_eq!(error.code, code, "{what}: {shown}");
    assert!(result.envelope.data.is_none(), "{what}: {shown}");
    error.details.clone().unwrap_or(Value::Null)
}

/// R1: on a frame two pages match, offline `do`, `ensure` and `wait` refuse with
/// `page_recognition_conflict` (exit 2) whose details name both pages in page order, as
/// `current-page` refuses it, where they took the first page before; on a frame one page matches,
/// that page is the frame's page.
#[test]
fn l7_offline_do_ensure_and_wait_refuse_a_frame_two_pages_match() {
    let context = Isolated::new();
    let twin = context.package("twin.zip", true);
    let single = context.package("single.zip", false);
    let frame = context.frame("entry.png", ENTRY, BUTTON);
    let frame = frame.as_str();
    let lines: [(&str, &[&str]); 3] = [
        ("do", &["--dry-run", "do", "ui/free", "--scene", frame]),
        (
            "ensure",
            &[
                "--dry-run",
                "ensure",
                "neutral/entry_page",
                "--scene",
                frame,
            ],
        ),
        (
            "wait",
            &["wait", "--page", "neutral/entry_page", "--scene", frame],
        ),
    ];

    for (verb, words) in lines {
        let details = refusal(
            &run(words, &twin),
            "page_recognition_conflict",
            2,
            &format!("{verb} on a frame two pages match"),
        );
        assert_eq!(details["matched_pages"], json!(TWIN_PAGES), "{verb}");
    }

    let planned = data(&run(lines[0].1, &single), "do on one matched page");
    assert_eq!(planned["state"], "planned");
    assert_eq!(planned["page"], "neutral/entry_page");
    let ensured = data(&run(lines[1].1, &single), "ensure on one matched page");
    assert_eq!(ensured["state"], "already_at_target");
    assert_eq!(ensured["page"], "neutral/entry_page");
    let waited = data(&run(lines[2].1, &single), "wait on one matched page");
    assert_eq!(waited["state"], "arrived");
    assert_eq!(waited["page"], "neutral/entry_page");
}

/// R2: `do --scene --dry-run` on a target an operation's guard names is guarded as the kernel
/// guards that operation: on the guard's page the target is evaluated (planned when it passes,
/// `target_not_visible` when it does not); on another page the plan is refused with the kernel's
/// guard refusal, `contained_task_guard_refused` (exit 3), although the target passes there.
#[test]
fn l7_do_guards_an_operation_target_on_its_guard_page_first() {
    let context = Isolated::new();
    let package = context.package("guarded.zip", false);
    let on_its_page = context.frame("entry.png", ENTRY, BUTTON);
    let on_another_page = context.frame("page_a.png", PAGE_A, BUTTON);
    let without_button = context.frame("entry_blank.png", ENTRY, BLANK);
    let line = |frame: &str| {
        run(
            &["--dry-run", "do", "ui/button", "--scene", frame],
            &package,
        )
    };

    let planned = data(&line(on_its_page.as_str()), "the guard's page");
    assert_eq!(planned["state"], "planned");
    assert_eq!(planned["page"], "neutral/entry_page");
    assert_eq!(planned["guard_result"]["passed"], true);

    let details = refusal(
        &line(on_another_page.as_str()),
        "contained_task_guard_refused",
        3,
        "another page",
    );
    assert_eq!(details["state"], "neutral/page_a");

    refusal(
        &line(without_button.as_str()),
        "target_not_visible",
        3,
        "the guard's page without its target",
    );
}

/// R2: a bare target, which no operation's guard names, is evaluated as before on any page; a
/// guard whose page is `any` passes the page check on every page, as the kernel's does.
#[test]
fn l7_do_keeps_the_evaluation_of_a_bare_target_and_an_any_page_guard() {
    let context = Isolated::new();
    let package = context.package("guarded.zip", false);
    let frame = context.frame("page_a.png", PAGE_A, BUTTON);

    for target in ["ui/free", "ui/anywhere"] {
        let planned = data(
            &run(
                &["--dry-run", "do", target, "--scene", frame.as_str()],
                &package,
            ),
            target,
        );
        assert_eq!(planned["state"], "planned", "{target}");
        assert_eq!(planned["page"], "neutral/page_a", "{target}");
        assert_eq!(planned["guard_result"]["passed"], true, "{target}");
    }
}
