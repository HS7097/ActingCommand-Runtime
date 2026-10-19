// SPDX-License-Identifier: AGPL-3.0-only

// #411 Lab B-2 (G8 offline; plan-lab-l5 §2 B-2 R1-R4, R7; coordinator decisions Q1, Q7, Q9, Q11
// of 2026-10-19; register §1.131; exec gate key 107): the offline Lab takes a frame set.
// - R1: `--scene <png>` may be given 1 to 5 times, oldest first, on the offline paths of
//   `observe`, `do --dry-run`, `detect-page`, `current-page`, `recognize`, `is-visible`, `ensure`
//   and `wait`; the last frame is the current frame (`frame_source`, `observation.frame`,
//   `frame_coordinates`).
// - R2: page detection and every target are evaluated over all the frames, oldest first, so a
//   target consensus decides on its last required frames; a candidate layout of offline
//   `observe`'s page is projected over its last `required_frames()` frames.
// - R3: counts and sizes are refused before anything is evaluated, with `validation_failed`
//   (exit 2) naming `--scene` and its `argument_problem`.
// - R4: `recognition_coverage` gives the frames provided whenever the package declares target
//   consensus; `candidate_coverage` the frames the layout used.
// - R7: every other option of a Lab2 path given more than once is refused as `repeated`.
// Every expected value comes from those requirement lines. Packages and frames are synthetic and
// neutral, and no Runtime or device is involved.

use super::*;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use sha2::{Digest, Sha256};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

/// The first pixel of a frame tells its page, the second shows the button or not, and the third
/// only tells the frames of a set apart.
const ENTRY: [u8; 3] = [40, 90, 160];
const OTHER: [u8; 3] = [90, 90, 90];
const PAGE_A: [u8; 3] = [200, 60, 60];
const BUTTON: [u8; 3] = [60, 200, 60];
const BLANK: [u8; 3] = [0, 0, 0];

const ENTRY_PAGE: &str = "neutral/entry_page";

/// What the package declares about several frames.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Agreement {
    /// Nothing: every target reads one frame.
    None,
    /// The entry page's target needs 2 of 3 frames (`k` 2 over frames 0, 1 and 2).
    Consensus,
    /// The entry page has the layout `choices`, whose feature `ready` needs both of 2 frames.
    Layout,
}

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

    /// A frame of `pixels` (one row) written as `name`.
    fn frame(&self, name: &str, pixels: &[[u8; 3]]) -> String {
        let png = Frame::from_pixels(
            pixels.len() as u32,
            1,
            pixels.concat(),
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

    /// The three-pixel frame `name`: `page`, `button` and the marker `index`.
    fn scene(&self, name: &str, page: [u8; 3], button: [u8; 3], index: u8) -> String {
        self.frame(name, &[page, button, [index; 3]])
    }

    /// The package `name`, its pages told apart by the frame's first pixel (`entry_page`,
    /// `page_a`). Its targets on the second pixel are the same button: `ui/button` guards
    /// `press_button`, which presses on `entry_page`; `ui/free` guards nothing. `agreement` adds
    /// the entry page's target consensus or its layout.
    fn package(&self, name: &str, agreement: Agreement) -> String {
        let colour = |id: &str, x: i32, expected: [u8; 3]| {
            json!({"type": "color", "id": id,
                   "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
        };
        let button = |id: &str| {
            let mut target = colour(id, 1, BUTTON);
            target["click"] = json!({"x": 1, "y": 0, "width": 1, "height": 1});
            target
        };
        let mut pack = json!({
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
        match agreement {
            Agreement::None => {}
            Agreement::Consensus => {
                pack["target_consensus"] = json!({"page/entry_page": {
                    "samples": [{"frame": 0}, {"frame": 1}, {"frame": 2}],
                    "k": 2,
                    "sample_interval_ms": 1
                }});
            }
            Agreement::Layout => {
                let whole = json!({"x": 0, "y": 0, "width": 3, "height": 1});
                pack["candidate_layouts"] = json!([{
                    "id": "choices",
                    "page_id": ENTRY_PAGE,
                    "kind": "fixed_slots",
                    "sample_interval_ms": 1,
                    "features": [{
                        "name": "ready",
                        "value": "passed",
                        "consensus": {"samples": [{"frame": 0}, {"frame": 1}],
                                      "aggregate": {"kind": "k_of_n", "k": 2}}
                    }],
                    "slots": [{"rect": whole, "click": whole, "targets": {"ready": "ui/button"}}]
                }]);
            }
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

/// The RGB8 hash of a three-pixel frame, as the projection and the candidate evidence name it.
fn rgb8_sha256(page: [u8; 3], button: [u8; 3], index: u8) -> String {
    format!("{:x}", Sha256::digest([page, button, [index; 3]].concat()))
}

/// `words`, then `--scene` for each of `scenes`, run with `--json` and the package.
fn run_on_scenes(words: &[&str], scenes: &[&str], package: &str) -> CliResult {
    let mut line = vec!["--json".to_string()];
    line.extend(words.iter().map(|word| (*word).to_string()));
    for scene in scenes {
        line.extend(["--scene".to_string(), (*scene).to_string()]);
    }
    line.extend(["--zip".to_string(), package.to_string()]);
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

/// The answer is a refusal with `code` and `exit`, and no data; its details.
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

/// The R3 refusal: `validation_failed` (exit 2) naming `--scene` as `problem`; its details.
#[track_caller]
fn scene_refusal(result: &CliResult, problem: &str, what: &str) -> Value {
    let details = refusal(result, "validation_failed", 2, what);
    assert_eq!(details["argument"], "--scene", "{what}: {details}");
    assert_eq!(details["argument_problem"], problem, "{what}: {details}");
    details
}

/// The coverage of `provided` frames for the consensus package, which requires 3.
fn consensus_coverage(provided: usize) -> Value {
    json!({"provided_frames": provided, "required_frames": 3, "aggregation": "production_owner"})
}

/// The two frame sets of the consensus package: (A, B, A), on which the entry page's target
/// passes on 2 of 3 frames, and (B, B, A), on which it passes on the last frame only.
struct ConsensusSets {
    agreeing: [String; 3],
    last_only: [String; 3],
}

impl ConsensusSets {
    fn new(context: &Isolated) -> Self {
        Self {
            agreeing: [
                context.scene("a1.png", ENTRY, BUTTON, 1),
                context.scene("b2.png", OTHER, BUTTON, 2),
                context.scene("a3.png", ENTRY, BUTTON, 3),
            ],
            last_only: [
                context.scene("b4.png", OTHER, BUTTON, 4),
                context.scene("b5.png", OTHER, BUTTON, 5),
                context.scene("a6.png", ENTRY, BUTTON, 6),
            ],
        }
    }

    fn agreeing(&self) -> Vec<&str> {
        self.agreeing.iter().map(String::as_str).collect()
    }

    fn last_only(&self) -> Vec<&str> {
        self.last_only.iter().map(String::as_str).collect()
    }
}

/// R1, R2, R4: offline `observe` decides target consensus over the given scenes, oldest first:
/// k = 2 of 3 is recognised on (A, B, A) and not on (B, B, A), whose current frame alone shows
/// the page; `recognition_coverage` gives 3 frames; the frame source and the observation's frame
/// are the current (last) frame.
#[test]
fn b2_observe_evaluates_target_consensus_over_the_given_scenes() {
    let context = Isolated::new();
    let package = context.package("consensus.zip", Agreement::Consensus);
    let sets = ConsensusSets::new(&context);

    let observed = data(
        &run_on_scenes(&["--verbose", "observe"], &sets.agreeing(), &package),
        "observe (A, B, A)",
    );
    assert_eq!(observed["state"], "observed", "{observed}");
    assert_eq!(observed["page"], ENTRY_PAGE, "{observed}");
    assert_eq!(
        observed["recognition_coverage"],
        consensus_coverage(3),
        "{observed}"
    );
    assert_eq!(
        observed["frame_source"]["path"], sets.agreeing[2],
        "{observed}"
    );
    assert_eq!(
        observed["observation"]["frame"]["sha256"],
        rgb8_sha256(ENTRY, BUTTON, 3),
        "{observed}"
    );

    let unknown = data(
        &run_on_scenes(&["--verbose", "observe"], &sets.last_only(), &package),
        "observe (B, B, A)",
    );
    assert_eq!(unknown["state"], "unknown", "{unknown}");
    assert_eq!(unknown["matched"], false, "{unknown}");
    assert_eq!(
        unknown["recognition_coverage"],
        consensus_coverage(3),
        "{unknown}"
    );
}

/// R1, R2, R4: `detect-page`, `current-page` and `do --scene --dry-run` decide on the same scene
/// sets as `observe`, each answer with `recognition_coverage`; do plans on the current frame's
/// coordinate system.
#[test]
fn b2_detect_page_current_page_and_do_decide_on_the_scene_set() {
    let context = Isolated::new();
    let package = context.package("consensus.zip", Agreement::Consensus);
    let sets = ConsensusSets::new(&context);

    for (scenes, page, matched) in [
        (sets.agreeing(), ENTRY_PAGE, true),
        (sets.last_only(), "standby", false),
    ] {
        for verb in ["detect-page", "current-page"] {
            let detected = data(&run_on_scenes(&[verb], &scenes, &package), verb);
            assert_eq!(detected["page"], page, "{verb}: {detected}");
            assert_eq!(detected["matched"], matched, "{verb}: {detected}");
            assert_eq!(
                detected["recognition_coverage"],
                consensus_coverage(3),
                "{verb}: {detected}"
            );
        }
        let planned = data(
            &run_on_scenes(&["--dry-run", "do", "ui/free"], &scenes, &package),
            "do --dry-run",
        );
        assert_eq!(planned["state"], "planned", "{planned}");
        assert_eq!(planned["page"], page, "{planned}");
        assert_eq!(planned["observation"]["matched"], matched, "{planned}");
        assert_eq!(
            planned["recognition_coverage"],
            consensus_coverage(3),
            "{planned}"
        );
        assert_eq!(
            planned["frame_coordinates"]["device_pixels"],
            json!([3, 1]),
            "{planned}"
        );
    }
}

/// R1, R2 (decision Q7): `recognize`, `is-visible`, `ensure --scene` and `wait --scene` take the
/// scene set too: the entry page's target passes on (A, B, A) and not on (B, B, A).
#[test]
fn b2_recognize_is_visible_ensure_and_wait_take_the_scene_set() {
    let context = Isolated::new();
    let package = context.package("consensus.zip", Agreement::Consensus);
    let sets = ConsensusSets::new(&context);

    for (scenes, agrees) in [(sets.agreeing(), true), (sets.last_only(), false)] {
        let recognized = data(
            &run_on_scenes(
                &["recognize", "--target", "page/entry_page"],
                &scenes,
                &package,
            ),
            "recognize",
        );
        assert_eq!(recognized["passed"], agrees, "{recognized}");
        assert_eq!(
            recognized["recognition_coverage"],
            consensus_coverage(3),
            "{recognized}"
        );
        let visible = data(
            &run_on_scenes(&["is-visible", "page/entry_page"], &scenes, &package),
            "is-visible",
        );
        assert_eq!(visible["visible"], agrees, "{visible}");
        assert_eq!(
            visible["recognition_coverage"],
            consensus_coverage(3),
            "{visible}"
        );

        let ensure = run_on_scenes(&["--dry-run", "ensure", ENTRY_PAGE], &scenes, &package);
        let wait = run_on_scenes(
            &["wait", "--page", ENTRY_PAGE, "--timeout-ms", "0"],
            &scenes,
            &package,
        );
        if agrees {
            assert_eq!(data(&ensure, "ensure")["state"], "already_at_target");
            assert_eq!(data(&wait, "wait")["state"], "arrived");
        } else {
            refusal(&ensure, "current_page_unknown", 3, "ensure on (B, B, A)");
            refusal(&wait, "wait_timeout", 3, "wait on (B, B, A)");
        }
    }
}

/// R2, R4 (decision Q11): a two-frame layout given three scenes is projected over the last two,
/// oldest first: its feature agrees there (both show the button) though not over the first two;
/// `candidate_coverage` gives the 2 frames it used, and its evidence names their frames.
#[test]
fn b2_a_layout_is_projected_over_its_last_frames() {
    let context = Isolated::new();
    let package = context.package("layout.zip", Agreement::Layout);
    let scenes = [
        context.scene("l1.png", ENTRY, BLANK, 1),
        context.scene("l2.png", ENTRY, BUTTON, 2),
        context.scene("l3.png", ENTRY, BUTTON, 3),
    ];
    let scenes = scenes.iter().map(String::as_str).collect::<Vec<_>>();

    let observed = data(
        &run_on_scenes(&["--verbose", "observe"], &scenes, &package),
        "observe with a two-frame layout",
    );
    assert_eq!(observed["page"], ENTRY_PAGE, "{observed}");
    assert_eq!(
        observed["candidate_coverage"]["provided_frames"], 2,
        "{observed}"
    );
    let projection = &observed["candidate_evidence"][0];
    assert_eq!(projection["layout_id"], "choices", "{observed}");
    assert_eq!(
        projection["candidates"][0]["features"]["ready"],
        json!({"type": "boolean", "value": true, "confidence": null}),
        "{observed}"
    );
    let frames = projection["recognition_evidence"][0]["samples"]
        .as_array()
        .unwrap_or_else(|| panic!("no feature samples: {observed}"))
        .iter()
        .map(|sample| sample["frame_rgb8_sha256"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        frames,
        vec![
            json!(rgb8_sha256(ENTRY, BUTTON, 2)),
            json!(rgb8_sha256(ENTRY, BUTTON, 3))
        ],
        "{observed}"
    );
}

/// R3: the six refusals of the frame counts and sizes, each `validation_failed` (exit 2) naming
/// `--scene`, before anything is evaluated; offline `observe` and `current-page` alike, the
/// layout's on `observe`, whose page it is.
#[test]
fn b2_scene_counts_and_sizes_are_refused_before_evaluation() {
    let context = Isolated::new();
    let consensus = context.package("consensus.zip", Agreement::Consensus);
    let layout = context.package("layout.zip", Agreement::Layout);
    let plain = context.package("plain.zip", Agreement::None);
    let a = context.scene("a.png", ENTRY, BUTTON, 1);
    let b = context.scene("b.png", ENTRY, BUTTON, 2);
    let c = context.scene("c.png", ENTRY, BUTTON, 3);
    let narrow = context.frame("narrow.png", &[ENTRY, BUTTON]);
    let six = (1..=6)
        .map(|index| context.scene(&format!("s{index}.png"), ENTRY, BUTTON, index))
        .collect::<Vec<_>>();
    let six = six.iter().map(String::as_str).collect::<Vec<_>>();

    for verb in ["observe", "current-page"] {
        let details = scene_refusal(
            &run_on_scenes(&[verb], &six, &consensus),
            "out_of_range",
            &format!("{verb} with six frames"),
        );
        assert_eq!(details["range_min"], 1, "{details}");
        assert_eq!(details["range_max"], 5, "{details}");

        let details = scene_refusal(
            &run_on_scenes(&[verb], &[&a, &b], &consensus),
            "out_of_range",
            &format!("{verb} with fewer frames than the consensus requires"),
        );
        assert_eq!(details["range_min"], 3, "{details}");
        assert_eq!(details["range_max"], 5, "{details}");
        assert_eq!(details["required_frames"], 3, "{details}");
        assert_eq!(details["provided_frames"], 2, "{details}");

        scene_refusal(
            &run_on_scenes(&[verb], &[&a, &b], &plain),
            "repeated",
            &format!("{verb} with two frames for a package without agreement"),
        );
        scene_refusal(
            &run_on_scenes(&[verb], &[&a, &a, &c], &consensus),
            "repeated",
            &format!("{verb} with one path twice"),
        );
        let details = scene_refusal(
            &run_on_scenes(&[verb], &[&a, &narrow, &c], &consensus),
            "unusable",
            &format!("{verb} with frames of different sizes"),
        );
        assert_eq!(details["index"], 1, "{details}");
    }

    let details = scene_refusal(
        &run_on_scenes(&["observe"], &[&a], &layout),
        "out_of_range",
        "observe of a page whose layout needs more frames",
    );
    assert_eq!(details["range_min"], 2, "{details}");
    assert_eq!(details["layout_id"], "choices", "{details}");
}

/// R7 (decision Q9): every path of `observe`, `do`, `ensure` and `wait` refuses an option it
/// takes once that is given twice, `validation_failed` (exit 2) naming it as `repeated`, before
/// anything is loaded, captured or pressed, instead of taking the last one.
#[test]
fn b2_a_repeated_option_is_refused_on_every_lab2_path() {
    let context = Isolated::new();
    let absent = |name: &str| context.files.path().join(name).display().to_string();
    let (first, second, scene) = (
        absent("first-package"),
        absent("second-package"),
        absent("scene.png"),
    );
    let (first, second, scene) = (first.as_str(), second.as_str(), scene.as_str());
    let paths: [(&str, &[&str]); 10] = [
        ("observe --scene", &["observe", "--scene", scene]),
        ("observe --capture", &["observe", "--capture"]),
        (
            "do --scene",
            &["--dry-run", "do", "ui/free", "--scene", scene],
        ),
        (
            "do --capture --dry-run",
            &["--dry-run", "do", "ui/free", "--capture"],
        ),
        ("do --capture", &["do", "--tap", "1,0", "--capture"]),
        (
            "ensure --scene",
            &["--dry-run", "ensure", ENTRY_PAGE, "--scene", scene],
        ),
        (
            "ensure --capture --dry-run",
            &["--dry-run", "ensure", ENTRY_PAGE, "--capture"],
        ),
        ("ensure --capture", &["ensure", ENTRY_PAGE, "--capture"]),
        (
            "wait --scene",
            &["wait", "--page", ENTRY_PAGE, "--scene", scene],
        ),
        (
            "wait --capture",
            &["wait", "--page", ENTRY_PAGE, "--capture"],
        ),
    ];
    for (path, words) in paths {
        let line = ["--json"]
            .iter()
            .chain(words)
            .chain(["--package", first, "--package", second].iter())
            .map(|word| (*word).to_string())
            .collect::<Vec<_>>();
        let details = refusal(&run_cli(line, true), "validation_failed", 2, path);
        assert_eq!(details["argument"], "--package", "{path}: {details}");
        assert_eq!(details["argument_problem"], "repeated", "{path}: {details}");
        assert_eq!(
            details["arguments"],
            json!([{"argument": "--package", "argument_problem": "repeated"}]),
            "{path}: {details}"
        );
    }
}
