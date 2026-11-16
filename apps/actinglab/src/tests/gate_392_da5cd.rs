// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #392 DA-5cd (register §1.437; `contracts/display-adapter.md` section 12,
// `contracts/lab-recording.md`, "A recording's size"; coordinator work order
// rd-1116/brief-392-da5cd-offline-scale-and-record-refusal.md): offline recognition takes a scene
// that is a pure scale of its package's coordinate space, and a recording is refused on a display
// that is not the base size.
// - Offline, one test per command the slice touches (`observe`, `do --dry-run`, `ensure
//   --dry-run`, `wait`, `recognize`, `is-visible`, `detect-page`, `current-page`, `tap-target
//   --dry-run`, `navigate --dry-run`), each through `da5cd_offline_row` on a synthetic 8x4 package
//   whose frame is 2x2 blocks: the frame enlarged 3 times (24x12) and 3/2 times (12x6) by nearest
//   neighbour gives the same answer as the 8x4 frame, its identifiers, times, the scene's path and
//   `frame_coordinates` aside; a frame of another ratio (8x6) and one smaller than the package
//   (4x2) are each refused `lab_scene_size_unsupported` (exit 2) with the word of the frame gate
//   that refused it, the scene's size and the package's. Adapted by the train of DA-5ab and
//   DA-5cd (register §1.436 and §1.437, "Train"; its rules R1 and R2), as the requirement changes
//   what they pin: the enlarged frames' answers name `frame_coordinates` in the base form
//   (`{space: base, base_pixels: [8, 4], reduced_from: [the scene's size], orientation,
//   applies_to}`) in place of DA-5cd's own member; the 8x4 frame's answer is as before (offline
//   `observe` and `do` state the frame's own `{device_pixels: [8, 4], ...}`, the others none);
//   and 5/4 (10x5) is no longer refused (the live observation takes it: the train's tests below).
//   `recognize`, which holds no identifier, answers the enlarged frames exactly as the 8x4 one but
//   for `frame_coordinates`.
// - `da5cd_record_start_on_a_display_that_is_not_the_base_size_starts_nothing`: the scripted
//   Runtime knows the instance's display at 1920x1080: `record start` fails
//   `record_display_not_base` with the display's size and the base size, after the handshake and
//   the display read alone (nothing asked of the scheduling, so the instance is not held), and the
//   session directory holds nothing but the recording lock's own files.
// - `da5cd_record_start_on_a_display_of_the_base_size_starts_as_before`: at 1280x720 the start
//   holds the instance with its own pause as before, its answer naming the display the Runtime
//   last observed and when (fix round 1: `display_last_observed`).
// - `da5cd_a_recording_of_another_display_is_listed_refused_and_still_read`: a recording whose
//   header names no base size (a unit test's `record start` writes none) and whose frames are
//   1920x1080 (as one made on such a display before this slice): fix round 1, the code
//   `record_size_not_base`: `record ready` lists `size_not_base` first, `record stop` refuses it
//   and writes nothing, a frame added to it is refused, `record status` reads it and `record
//   archive` archives it.
// Fix round 1 (coordinator work order rd-1116/brief-392-da5cd-fix2.md; review rd-1116/review-1781.md,
// its sections (a), (b), (f) and (g)):
// - `da5cd_fix1_record_start_writes_the_base_size_and_checks_the_first_frame`: `record start`
//   with the installed base size (this thread's test switch set to `RECORDING_BASE_SIZE`) and no
//   Runtime, so no display is known: the recording's header names `base_size` 1280x720 and no
//   size; a first frame of another aspect (2400x1080), one smaller than the base (640x360) and one
//   of 1920x1080 are each refused `record_display_not_base` with its size and the base size, the
//   recording's files byte for byte as they were; a frame of 1280x720 then opens step 1 and the
//   recording takes its size.
// - `da5cd_fix1_detect_page_tap_target_and_navigate_carry_their_values_through_mcp`: through
//   `serve --stdio --front mcp`, the refusal `lab_scene_size_unsupported` of `detect-page`,
//   `tap-target` with `dry_run` and `navigate` with `dry_run` on a scene of another ratio carries
//   its values on the line (exit 1, as every refusal of actinglab's own line).
// The train of DA-5ab and DA-5cd (register §1.436 and §1.437, "Train"; rule R1: a stored image is
// judged, reduced and answered as the live observation takes a frame of its size, 5/4 accepted):
// - `da5_train_a_stored_five_quarters_scene_is_taken_by_detect_page`: `detect-page` on the 8x4
//   frame enlarged 5/4 times (10x5, nearest neighbour; the entry page's block stays whole) detects
//   the entry page and names `frame_coordinates` in the base form, `reduced_from` [10, 5].
// - `da5_train_a_stored_five_quarters_scene_is_taken_by_a_dry_run`: `ensure --dry-run` to `page_a`
//   on the same 10x5 scene plans its route from the entry page, with the same `frame_coordinates`.
// Synthetic neutral packages and frames, temporary roots, a scripted Runtime on loopback; no
// device, no clock, no sleep.

use super::gate_434_lab5b4::{Isolated, printed};
use super::gate_463_s17::{INSTANCE, Place, RECORD_ID, instance, operations, paused, status, wire};
use super::*;
use crate::cli_module::{self, serve};
use crate::commands::session_record::recording_base_for_tests;
use actingcommand_contract::{
    DISPLAY_FRAME_EXTENT_FACT_KEY, FactContent, FactRecord, FactScalar, FactScope, FactValue,
    InstanceFactView, LabResult, RuntimeRequest, RuntimeResult, RuntimeStateSource,
};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_lab::{
    AttachFrameOutcome, AttachFrameRequest, RECORDING_BASE_SIZE, RecordingLock, record_attach_frame,
};
use actingcommand_lab_ops::initiator::ENVIRONMENT_VARIABLE;
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedRuntime,
};
use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

/// The operation of `record start`'s read of the instance's display, as the wire spells it.
pub(super) const DISPLAY_READ: &str = "instance_fact_snapshot";

/// The scripted answer to `record start`'s read of `INSTANCE`'s display: the Runtime's
/// `display.frame_extent` of it, `extent`, or none.
pub(super) fn display_known(
    extent: Option<(u32, u32)>,
) -> impl FnOnce(&RuntimeRequest) -> ScriptedAnswer + Send + 'static {
    move |request| {
        let records = extent
            .map(|(width, height)| display_record(width, height))
            .into_iter()
            .collect::<Vec<_>>();
        let total = u32::try_from(records.len()).expect("a count of records");
        ScriptedAnswer::completed(
            request,
            RuntimeResult::InstanceFactSnapshot {
                snapshot: Box::new(InstanceFactView {
                    instance_alias: INSTANCE.to_owned(),
                    server_id: None,
                    game_id: None,
                    records,
                }),
                source: RuntimeStateSource {
                    instance_facts: None,
                    runtime_facts: None,
                    ledger_committed_head: 1,
                    sampled_started_at_unix_ms: 1,
                    sampled_completed_at_unix_ms: 1,
                },
                records_total: Some(total),
            },
        )
    }
}

/// `INSTANCE`'s `display.frame_extent` of `width` x `height`, as the Runtime records it.
fn display_record(width: u32, height: u32) -> FactRecord {
    FactRecord {
        scope: FactScope::Instance {
            instance_id: INSTANCE.to_owned(),
        },
        key: DISPLAY_FRAME_EXTENT_FACT_KEY.to_owned(),
        content: FactContent::Inline {
            value: FactValue::RecordList(vec![BTreeMap::from([
                ("width".to_owned(), FactScalar::Integer(i64::from(width))),
                ("height".to_owned(), FactScalar::Integer(i64::from(height))),
            ])]),
        },
        observed_at_unix_ms: 1,
        expires_at_unix_ms: None,
        ttl_policy: None,
        confidence_milli: 1_000,
        source_detector: "runtime.display-adapter".to_owned(),
        source_snapshot_id: "display:da5cd".to_owned(),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "0".repeat(64),
        invalidate_on: Vec::new(),
    }
}

// ---- Offline: a scene that is a pure scale of its package ----

const ENTRY: [u8; 3] = [40, 90, 160];
const PAGE_A: [u8; 3] = [200, 60, 60];
const BUTTON: [u8; 3] = [60, 200, 60];
const BLANK: [u8; 3] = [0, 0, 0];
const ENTRY_PAGE: &str = "neutral/entry_page";
const PAGE_A_PAGE: &str = "neutral/page_a";
/// The package's coordinate space.
const BASE: (u32, u32) = (8, 4);

/// The package's frame, in 2x2 blocks: the entry page's mark at the top left, the button beside
/// it, the rest blank.
fn base_pixel(x: u32, y: u32) -> [u8; 3] {
    match (x / 2, y / 2) {
        (0, 0) => ENTRY,
        (1, 0) => BUTTON,
        _ => BLANK,
    }
}

/// The package's frame at `width` x `height` by nearest neighbour: pixel (x, y) shows the base
/// pixel (x * 8 / width, y * 4 / height). Enlarged 3 or 3/2 times, each 2x2 block stays whole, so
/// the area average gives the base frame back exactly.
fn frame_png(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..height {
        for x in 0..width {
            pixels.extend(base_pixel(x * BASE.0 / width, y * BASE.1 / height));
        }
    }
    Frame::from_pixels(
        width,
        height,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
    .encode_png_fast()
    .expect("fixture PNG")
}

/// The package: its pages told apart by the top left block (`entry_page`, `page_a`), the button
/// beside it, `ui/button` guarding `press_button` and `ui/free` guarding nothing, and one
/// navigation edge from the entry page to `page_a` that presses the button.
fn package_bytes() -> Vec<u8> {
    let colour = |id: &str, x: i32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": x, "y": 0, "width": 2, "height": 2}, "expected": expected})
    };
    let button = |id: &str| {
        let mut target = colour(id, 2, BUTTON);
        target["click"] = json!({"x": 2, "y": 0, "width": 2, "height": 2});
        target
    };
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": BASE.0, "height": BASE.1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            colour("page/entry_page", 0, ENTRY),
            colour("page/page_a", 0, PAGE_A),
            button("ui/button"),
            button("ui/free")
        ]
    });
    let entries = [
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
                "coordinate_space": {"width": BASE.0, "height": BASE.1},
                "entry_page": "entry_page",
                "target_page": "page_a",
                "operations": [{
                    "id": "press_button",
                    "purpose": "neutral fixture",
                    "from": "entry_page",
                    "to": "page_a",
                    "click": {"kind": "point", "x": 3, "y": 1},
                    "guard": {
                        "page_id": "entry_page",
                        "target_id": "ui/button",
                        "expected_rect": {"x": 2, "y": 0, "width": 2, "height": 2},
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
                {"id": PAGE_A_PAGE, "required": ["page/page_a"]}
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
                    "to_page": PAGE_A_PAGE,
                    "click": {"kind": "rect", "x": 2, "y": 0, "width": 2, "height": 2}
                }],
                "destructive_actions": []
            }),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in &entries {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec(value).expect("JSON"))
            .expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

/// A context with no configuration, no recording session and an empty Runtime state root, the
/// package and its frames by size. The fields drop in order, the environment lock last.
struct Offline {
    package: String,
    files: TempDir,
    _runtime_env: RuntimeStateEnvGuard,
    _runtime: TempDir,
    _app_env: TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Offline {
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
        let package = files.path().join("package.zip");
        fs::write(&package, package_bytes()).unwrap();
        Self {
            package: package.display().to_string(),
            files,
            _runtime_env: runtime_env,
            _runtime: runtime,
            _app_env: app_env,
            _guard: guard,
        }
    }

    /// The package's frame at `width` x `height`, written as a scene file: its path.
    fn scene(&self, width: u32, height: u32) -> String {
        let path = self
            .files
            .path()
            .join(format!("scene-{width}x{height}.png"));
        fs::write(&path, frame_png(width, height)).unwrap();
        path.display().to_string()
    }

    /// What `--json <words> --scene <scene> --zip <package>` answers.
    fn run(&self, words: &[&str], scene: &str) -> CliResult {
        let mut line = vec!["--json".to_owned()];
        line.extend(words.iter().map(|word| (*word).to_owned()));
        line.extend(["--scene".to_owned(), scene.to_owned()]);
        line.extend(["--zip".to_owned(), self.package.clone()]);
        run_cli(line, true)
    }
}

/// The members a run mints or reads from where it runs (its identifiers, its times, the scene's
/// path) and the member that states the frame's size (the train of DA-5: `frame_coordinates`, the
/// base form on a reduced scene): what two answers on the same screen may differ in.
const VOLATILE: [&str; 10] = [
    "req_id",
    "reco_id",
    "action_id",
    "action_ids",
    "wf_id",
    "evidence_id",
    "elapsed_ms",
    "evaluation_duration_ms",
    "frame_source",
    "frame_coordinates",
];

/// The train of DA-5 (R2): `frame_coordinates` of the 8x4 package's frame as it is, byte for byte
/// what offline `observe` and `do` stated before.
fn frame_own_coordinates() -> Value {
    json!({
        "device_pixels": [BASE.0, BASE.1],
        "orientation": "landscape",
        "applies_to": ["tap", "tap_rect", "swipe"]
    })
}

/// The train of DA-5 (R2): `frame_coordinates` of a scene of `width` x `height` reduced to the 8x4
/// package's base, the base form the live observation states.
fn reduced_coordinates(width: u32, height: u32) -> Value {
    json!({
        "space": "base",
        "base_pixels": [BASE.0, BASE.1],
        "reduced_from": [width, height],
        "orientation": "landscape",
        "applies_to": ["tap", "tap_rect", "swipe"]
    })
}

/// `answer` without its [`VOLATILE`] members, at every depth: what it decided.
fn decision(answer: &Value) -> Value {
    match answer {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(name, _)| !VOLATILE.contains(&name.as_str()))
                .map(|(name, value)| (name.clone(), decision(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(decision).collect()),
        other => other.clone(),
    }
}

/// The answer of `words` on the scene `scene`, a success: its data.
#[track_caller]
fn answered(context: &Offline, words: &[&str], scene: &str) -> Value {
    let result = context.run(words, scene);
    let shown = result.envelope_json();
    assert_eq!(result.exit_code(), 0, "{words:?} on {scene}: {shown}");
    result
        .envelope
        .data
        .clone()
        .unwrap_or_else(|| panic!("{words:?} on {scene}: no data: {shown}"))
}

/// `words` on a scene of `width` x `height` the package cannot use, refused before anything is
/// evaluated: `lab_scene_size_unsupported` (exit 2), its outcome with the adapter's word
/// `problem`, the scene's size and the package's. `detect-page`, `tap-target` and `navigate` carry
/// the refusal's details inside their trace's.
#[track_caller]
fn refused(context: &Offline, words: &[&str], width: u32, height: u32, problem: &str) {
    let scene = context.scene(width, height);
    let result = context.run(words, &scene);
    let shown = result.envelope_json();
    assert_eq!(
        result.exit_code(),
        2,
        "{words:?} on {width}x{height}: {shown}"
    );
    let error = result
        .envelope
        .error
        .as_ref()
        .unwrap_or_else(|| panic!("{words:?} on {width}x{height} was not refused: {shown}"));
    assert_eq!(
        error.code, "lab_scene_size_unsupported",
        "{words:?} on {width}x{height}: {shown}"
    );
    let details = error.details.clone().unwrap_or(Value::Null);
    let named = if details.get("blocked_error").is_some() {
        &details["details"]
    } else {
        &details
    };
    assert_eq!(
        named["outcome"]["code"], "lab_scene_size_unsupported",
        "{words:?} on {width}x{height}: {shown}"
    );
    assert_eq!(
        named["outcome"]["values"],
        json!({
            "display_problem": problem,
            "frame_width": width,
            "frame_height": height,
            "base_width": BASE.0,
            "base_height": BASE.1
        }),
        "{words:?} on {width}x{height}: {shown}"
    );
}

/// Whether an offline command's answer on a frame of its package's size states the frame's
/// `frame_coordinates` (offline `observe` and `do` do; the others state none).
#[derive(Clone, Copy)]
enum OwnCoordinates {
    Stated,
    None,
}

/// The slice's requirement for one offline command `words`, whose answer on the package's frame
/// holds `expected` at the JSON pointer `pointer` (so that the comparison decides something): the
/// frame enlarged 3 and 3/2 times answers as the package's frame, naming `frame_coordinates` in
/// the base form (the train of DA-5, R2); the package's frame states
/// the frame's own `frame_coordinates` where the command stated them before, and none otherwise
/// (`own`); a scene of another ratio and one smaller than the package are refused.
fn da5cd_offline_row(words: &[&str], pointer: &str, expected: Value, own: OwnCoordinates) {
    let context = Offline::new();
    let base_scene = context.scene(BASE.0, BASE.1);
    let base = answered(&context, words, &base_scene);
    assert_eq!(base.pointer(pointer), Some(&expected), "{words:?}: {base}");
    match own {
        OwnCoordinates::Stated => assert_eq!(
            base["frame_coordinates"],
            frame_own_coordinates(),
            "{words:?}: {base}"
        ),
        OwnCoordinates::None => {
            assert!(base.get("frame_coordinates").is_none(), "{words:?}: {base}")
        }
    }
    for (width, height) in [(24, 12), (12, 6)] {
        let scene = context.scene(width, height);
        let scaled = answered(&context, words, &scene);
        assert_eq!(
            scaled["frame_coordinates"],
            reduced_coordinates(width, height),
            "{words:?} on {width}x{height}: {scaled}"
        );
        assert_eq!(
            decision(&scaled),
            decision(&base),
            "{words:?} on {width}x{height}: {scaled}"
        );
    }
    refused(&context, words, 8, 6, "layout_undeclared");
    refused(&context, words, 4, 2, "scale_below_base");
}

#[test]
fn da5cd_observe_takes_a_scaled_scene() {
    da5cd_offline_row(
        &["observe", "--pretty"],
        "/page",
        json!(ENTRY_PAGE),
        OwnCoordinates::Stated,
    );
}

#[test]
fn da5cd_do_dry_run_takes_a_scaled_scene() {
    da5cd_offline_row(
        &["--dry-run", "do", "ui/free", "--pretty"],
        "/state",
        json!("planned"),
        OwnCoordinates::Stated,
    );
}

#[test]
fn da5cd_ensure_dry_run_takes_a_scaled_scene() {
    da5cd_offline_row(
        &["--dry-run", "ensure", PAGE_A_PAGE, "--pretty"],
        "/state",
        json!("planned"),
        OwnCoordinates::None,
    );
}

#[test]
fn da5cd_wait_takes_a_scaled_scene() {
    da5cd_offline_row(
        &[
            "wait",
            "--page",
            ENTRY_PAGE,
            "--timeout-ms",
            "0",
            "--pretty",
        ],
        "/state",
        json!("arrived"),
        OwnCoordinates::None,
    );
}

#[test]
fn da5cd_recognize_takes_a_scaled_scene() {
    let words = ["recognize", "--target", "ui/free"];
    da5cd_offline_row(&words, "/passed", json!(true), OwnCoordinates::None);
    // `recognize` holds no identifier: the enlarged frame's answer is the package frame's and the
    // member.
    let context = Offline::new();
    let base_scene = context.scene(BASE.0, BASE.1);
    let base = answered(&context, &words, &base_scene);
    let scene = context.scene(24, 12);
    let mut scaled = answered(&context, &words, &scene);
    let member = scaled
        .as_object_mut()
        .expect("recognize answers an object")
        .remove("frame_coordinates");
    assert_eq!(member, Some(reduced_coordinates(24, 12)));
    assert_eq!(scaled, base);
}

#[test]
fn da5cd_is_visible_takes_a_scaled_scene() {
    da5cd_offline_row(
        &["is-visible", "ui/free"],
        "/visible",
        json!(true),
        OwnCoordinates::None,
    );
}

#[test]
fn da5cd_detect_page_takes_a_scaled_scene() {
    da5cd_offline_row(
        &["detect-page"],
        "/page",
        json!(ENTRY_PAGE),
        OwnCoordinates::None,
    );
}

#[test]
fn da5cd_current_page_takes_a_scaled_scene() {
    da5cd_offline_row(
        &["current-page"],
        "/page",
        json!(ENTRY_PAGE),
        OwnCoordinates::None,
    );
}

#[test]
fn da5cd_tap_target_dry_run_takes_a_scaled_scene() {
    da5cd_offline_row(
        &["--dry-run", "tap-target", "ui/free"],
        "/status",
        json!("planned"),
        OwnCoordinates::None,
    );
}

#[test]
fn da5cd_navigate_dry_run_takes_a_scaled_scene() {
    da5cd_offline_row(
        &["--dry-run", "navigate", "--to", PAGE_A_PAGE],
        "/status",
        json!("planned"),
        OwnCoordinates::None,
    );
}

/// The train of DA-5 (R1): a stored image 5/4 the package's size (10x5 for 8x4) is taken as the
/// live observation takes a frame of that size, not refused: `words` answer `expected` at
/// `pointer`, naming `frame_coordinates` in the base form, `reduced_from` [10, 5]. The 2x2 blocks
/// do not stay whole at 5/4, but the entry page's mark (the top left block, the device's 3x3
/// pixels) does: the entry page is detected.
fn five_quarters_taken(words: &[&str], pointer: &str, expected: Value) {
    let context = Offline::new();
    let scene = context.scene(10, 5);
    let answer = answered(&context, words, &scene);
    assert_eq!(
        answer.pointer(pointer),
        Some(&expected),
        "{words:?}: {answer}"
    );
    assert_eq!(
        answer["frame_coordinates"],
        reduced_coordinates(10, 5),
        "{words:?}: {answer}"
    );
}

#[test]
fn da5_train_a_stored_five_quarters_scene_is_taken_by_detect_page() {
    five_quarters_taken(&["detect-page"], "/page", json!(ENTRY_PAGE));
}

#[test]
fn da5_train_a_stored_five_quarters_scene_is_taken_by_a_dry_run() {
    five_quarters_taken(
        &["--dry-run", "ensure", PAGE_A_PAGE, "--pretty"],
        "/state",
        json!("planned"),
    );
}

// ---- Recordings: the base size ----

/// Every file of the session directory `state` but the recording lock's own files
/// (`record-<instance>.lock` and its holder file), which every state-writing record command takes
/// before it reads anything, a refused one too: their names, in order.
fn session_files(state: &str) -> Vec<String> {
    let Ok(entries) = fs::read_dir(state) else {
        return Vec::new();
    };
    let mut names = entries
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| !name.contains(".lock"))
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[test]
fn da5cd_record_start_on_a_display_that_is_not_the_base_size_starts_nothing() {
    let _guard = env_lock();
    let place = Place::new();
    let runtime = ScriptedRuntime::start(&place.runtime(), |_| {
        vec![ScriptedConnection::new().request(display_known(Some((1920, 1080))))]
    });
    let (code, refused) = place.start(RECORD_ID, &[]);
    let record = runtime.finish();

    assert_eq!(code, 1, "{refused}");
    assert_eq!(
        refused["outcome"]["code"], "record_display_not_base",
        "{refused}"
    );
    assert_eq!(
        refused["outcome"]["values"],
        json!({"frame_width": 1920, "frame_height": 1080, "base_width": 1280, "base_height": 720}),
        "{refused}"
    );
    assert_eq!(
        refused["details"]["display"],
        json!({"width": 1920, "height": 1080}),
        "{refused}"
    );
    assert_eq!(
        refused["details"]["base"],
        json!({"width": 1280, "height": 720}),
        "{refused}"
    );
    // The handshake and the display read: nothing was asked of the instance's scheduling, so it is
    // not held.
    assert!(record.unscripted.is_empty(), "a connection beyond the one");
    assert_eq!(record.connections.len(), 1);
    assert_eq!(operations(&record.connections[0]), wire(&[DISPLAY_READ]));
    // Nothing written: no session header, no recording, no file but the lock's.
    assert!(!place.header_path().exists(), "no session header");
    assert!(
        !place.recording_dir(RECORD_ID).exists(),
        "no recording directory"
    );
    assert_eq!(session_files(&place.session()), Vec::<String>::new());
}

#[test]
fn da5cd_record_start_on_a_display_of_the_base_size_starts_as_before() {
    let _guard = env_lock();
    let place = Place::new();
    let runtime = ScriptedRuntime::start(&place.runtime(), |owner_epoch| {
        vec![
            ScriptedConnection::new()
                .request(display_known(Some((1280, 720))))
                .request(status(owner_epoch, vec![instance(INSTANCE, None)]))
                .request(paused),
        ]
    });
    let (code, started) = place.start(RECORD_ID, &[]);
    let record = runtime.finish();

    assert_eq!(code, 0, "{started}");
    assert_eq!(started["status"], "started", "{started}");
    assert_eq!(started["lab_recording"]["status"], "active", "{started}");
    assert_eq!(started["hold"]["state"], "own", "{started}");
    assert_eq!(
        started["display_last_observed"],
        json!({"width": 1280, "height": 720, "observed_at_unix_ms": 1}),
        "{started}"
    );
    assert!(record.unscripted.is_empty(), "a connection beyond the one");
    assert_eq!(record.connections.len(), 1);
    assert_eq!(
        operations(&record.connections[0]),
        wire(&[DISPLAY_READ, "status", "pause_scheduling"])
    );
    assert!(place.header_path().is_file(), "the session header");
}

/// What `actinglab --json --instance neutral.a record <action> --state-dir <state> <more>` prints:
/// its exit code and its line.
fn record(state: &str, action: &str, more: &[&str]) -> (u8, Value) {
    let mut line = vec![
        "--json",
        "--instance",
        INSTANCE,
        "record",
        action,
        "--state-dir",
        state,
    ];
    line.extend(more);
    let exit = printed(&line);
    let answered = exit
        .line
        .clone()
        .unwrap_or_else(|text| panic!("{line:?} printed no line: {text}"));
    (exit.code, answered)
}

/// Every file under `dir` by its path relative to it, with its bytes, in name order.
fn files_under(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    let mut pending = vec![(dir.to_path_buf(), String::new())];
    while let Some((path, prefix)) = pending.pop() {
        for entry in fs::read_dir(&path).expect("directory") {
            let entry = entry.expect("entry");
            let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
            if entry.file_type().expect("file type").is_dir() {
                pending.push((entry.path(), format!("{name}/")));
            } else {
                files.push((name, fs::read(entry.path()).expect("file")));
            }
        }
    }
    files.sort();
    files
}

/// A frame of `width` x `height` whose every pixel is the entry page's colour, as a PNG.
fn solid_png(width: u32, height: u32) -> Vec<u8> {
    let pixels = (0..width * height).flat_map(|_| ENTRY).collect();
    Frame::from_pixels(
        width,
        height,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("a fixture frame")
    .encode_png_fast()
    .expect("its PNG")
}

/// `png` added, as a Runtime observation, to the active recording of `INSTANCE` in the session
/// directory `state`, under the recording lock, which is released when it answers.
fn attach(state: &str, png: Vec<u8>) -> LabResult<AttachFrameOutcome> {
    let lock = RecordingLock::acquire(Path::new(state), INSTANCE, "gate_392_da5cd", "Lab-CLI")
        .expect("recording lock");
    record_attach_frame(
        &lock,
        AttachFrameRequest {
            png,
            source: "runtime_observation".to_owned(),
            runtime_artifact: None,
            capture_backend: None,
            freshness: None,
        },
    )
}

#[test]
fn da5cd_a_recording_of_another_display_is_listed_refused_and_still_read() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    // Started while nothing listens: no display is known, and (a unit test's `record start` writes
    // no base size) the recording takes its first frame's size, as one made before this slice took
    // it.
    let (code, started) = record(
        &state,
        "start",
        &["--task-id", "neutral_task", "--record-id", "rec_1080"],
    );
    assert_eq!(code, 0, "{started}");
    assert_eq!(started["display_last_observed"], Value::Null, "{started}");
    attach(&state, solid_png(1920, 1080))
        .unwrap_or_else(|error| panic!("frame: {}: {}", error.code, error.message));
    let dir = Path::new(&state)
        .join("record-artifacts")
        .join("rec_1080")
        .join("lab");
    let before = files_under(&dir);
    let values =
        json!({"frame_width": 1920, "frame_height": 1080, "base_width": 1280, "base_height": 720});

    let (code, ready) = record(&state, "ready", &[]);
    assert_eq!(code, 0, "{ready}");
    assert_eq!(ready["missing"][0]["order"], 1, "{ready}");
    assert_eq!(ready["missing"][0]["what"], "size_not_base", "{ready}");
    assert_eq!(
        ready["missing"][0]["code"], "record_size_not_base",
        "{ready}"
    );

    let (code, stopped) = record(&state, "stop", &[]);
    assert_eq!(code, 1, "{stopped}");
    assert_eq!(
        stopped["outcome"]["code"], "record_size_not_base",
        "{stopped}"
    );
    assert_eq!(stopped["outcome"]["values"], values, "{stopped}");
    assert_eq!(files_under(&dir), before, "record stop wrote nothing");

    // Fix round 1: a frame added to such a recording is refused before it is stored.
    let added = attach(&state, solid_png(1920, 1080)).expect_err("a frame added to it");
    assert_eq!(added.code, "record_size_not_base", "{}", added.message);
    let details = added.details.clone().unwrap_or(Value::Null);
    assert_eq!(details["outcome"]["values"], values, "{details}");
    assert_eq!(files_under(&dir), before, "the refused frame wrote nothing");

    let (code, shown) = record(&state, "status", &[]);
    assert_eq!(code, 0, "{shown}");
    assert_eq!(shown["lab"]["status"], "active", "{shown}");
    assert_eq!(
        shown["lab"]["coordinate_space"],
        json!({"width": 1920, "height": 1080}),
        "{shown}"
    );

    let (code, archived) = record(&state, "archive", &[]);
    assert_eq!(code, 0, "{archived}");
    assert_eq!(archived["status"], "archived", "{archived}");
    assert_eq!(archived["lab"]["manifest"]["written"], true, "{archived}");
}

// ---- Fix round 1: the base size in the recording's header, checked at its first frame ----

/// The recording `rec_base`'s header (`recording.json`) under the session directory `state`.
fn base_header(state: &str) -> Value {
    let path = Path::new(state)
        .join("record-artifacts")
        .join("rec_base")
        .join("lab")
        .join("recording.json");
    serde_json::from_slice(&fs::read(&path).expect("the recording")).expect("its JSON")
}

#[test]
fn da5cd_fix1_record_start_writes_the_base_size_and_checks_the_first_frame() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    // The installed `record start` writes `RECORDING_BASE_SIZE`; a unit test's writes it only when
    // its thread says so.
    let _base = recording_base_for_tests::set(RECORDING_BASE_SIZE);
    let state = isolated.path("session");
    // Nothing listens: no display is known, and the start goes on.
    let (code, started) = record(
        &state,
        "start",
        &["--task-id", "neutral_task", "--record-id", "rec_base"],
    );
    assert_eq!(code, 0, "{started}");
    assert_eq!(started["display_last_observed"], Value::Null, "{started}");
    let header = base_header(&state);
    assert_eq!(
        header["base_size"],
        json!({"width": 1280, "height": 720}),
        "{header}"
    );
    assert_eq!(header["coordinate_space"], Value::Null, "{header}");
    let dir = Path::new(&state)
        .join("record-artifacts")
        .join("rec_base")
        .join("lab");
    let before = files_under(&dir);

    // Another aspect, smaller than the base size, and 1920x1080 with no display known: each first
    // frame refused before anything of it is kept.
    for (width, height) in [(2400, 1080), (640, 360), (1920, 1080)] {
        let refused = attach(&state, solid_png(width, height))
            .expect_err("a first frame that is not the base size");
        assert_eq!(
            refused.code, "record_display_not_base",
            "{width}x{height}: {}",
            refused.message
        );
        let details = refused.details.clone().unwrap_or(Value::Null);
        assert_eq!(
            details["outcome"]["values"],
            json!({
                "frame_width": width,
                "frame_height": height,
                "base_width": 1280,
                "base_height": 720
            }),
            "{width}x{height}: {details}"
        );
        assert_eq!(
            details["display"],
            json!({"width": width, "height": height}),
            "{width}x{height}: {details}"
        );
        assert_eq!(
            files_under(&dir),
            before,
            "{width}x{height}: no frame, no step, the header as it was"
        );
    }

    // The base size: the frame opens step 1 and the recording takes its size.
    let accepted = attach(&state, solid_png(1280, 720))
        .unwrap_or_else(|error| panic!("1280x720: {}: {}", error.code, error.message));
    assert_eq!((accepted.step, accepted.step_opened), (1, true));
    let header = base_header(&state);
    assert_eq!(
        header["coordinate_space"],
        json!({"width": 1280, "height": 720}),
        "{header}"
    );
    assert_eq!(
        header["base_size"],
        json!({"width": 1280, "height": 720}),
        "{header}"
    );
    assert_eq!(
        header["steps"].as_array().map(Vec::len),
        Some(1),
        "{header}"
    );
}

#[test]
fn da5cd_fix1_detect_page_tap_target_and_navigate_carry_their_values_through_mcp() {
    let context = Offline::new();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    // A scene of another ratio than the package's 8x4.
    let scene = context.scene(8, 6);
    let package = context.package.as_str();
    let requests = [
        json!({"op": "detect-page", "params": {"scene": [scene], "zip": package}}),
        json!({"op": "tap-target", "params": {
            "target": "ui/free", "dry_run": true, "scene": scene, "zip": package
        }}),
        json!({"op": "navigate", "params": {
            "to": PAGE_A_PAGE, "dry_run": true, "scene": scene, "zip": package
        }}),
    ];
    let table = cli_module::table().expect("the operation table");
    let input = requests
        .iter()
        .map(|request| format!("{request}\n"))
        .collect::<String>();
    let mut output = Vec::new();
    let mut developer = Vec::new();
    serve::serve_as(
        table,
        Some(serve::Front::Mcp),
        input.as_bytes(),
        &mut output,
        &mut developer,
    )
    .expect("the loop ends");
    let printed = String::from_utf8(output).expect("UTF-8 lines");
    let lines = printed.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), requests.len(), "one line each: {printed}");
    for (text, request) in lines.iter().zip(&requests) {
        let envelope = serde_json::from_str::<Value>(text)
            .unwrap_or_else(|error| panic!("one JSON line ({error}): {text}"));
        // actinglab's own line exits 1 for every refusal; the envelope says so.
        assert_eq!(envelope["exit_code"], 1, "{request}: {envelope}");
        let outcome = &envelope["line"]["outcome"];
        assert_eq!(
            outcome["code"], "lab_scene_size_unsupported",
            "{request}: {envelope}"
        );
        assert_eq!(
            outcome["values"],
            json!({
                "display_problem": "layout_undeclared",
                "frame_width": 8,
                "frame_height": 6,
                "base_width": BASE.0,
                "base_height": BASE.1
            }),
            "{request}: {envelope}"
        );
    }
}
