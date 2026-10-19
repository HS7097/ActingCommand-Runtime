// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #406 T4 (exec key 112; register §1.138; plan Q20, Q14; requirement basis:
// rd-1019/plan-405-406/brief-T4.md, Requirement 1 and 2, decisions.md Q13 and Q20).
//
// - `package dry-run` and `replay` draw as the kernel from the run's seed (`--seed`, else 0): the
//   dry run reports the seed and how its decision's points were drawn, and a replay given the
//   reported seed draws every point again.
// - Without a touch section (a process outside every root) the offline draw is uniform and
//   named `xorshift64_uniform_rect_v1`.
// - Offline `do --scene` given a seed plans the press at the hotspot point drawn from it with the
//   root configuration's touch keys (here a test configuration file), not at the centre.
//
// Expected points come from the reference of the draw below, written from the contract
// (`contracts/resource-declarations.md`, "Touch sampling" and "The Lab's draw"). Packages,
// frames and seeds are synthetic and neutral; no Runtime or device is involved.

use super::*;
use crate::lab_draw::TEST_TOUCH_CONFIG_ENV;
use actingcommand_contract::InputAction;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

/// The reference of the draws, from the contract's text.
mod reference {
    use sha2::{Digest, Sha256};

    pub(super) fn action_seed(seed: u64, step_index: u32, label: &str) -> u64 {
        let mut hasher = Sha256::new();
        hasher.update(b"ActingCommand offline action seed v1\0");
        hasher.update(seed.to_be_bytes());
        hasher.update(step_index.to_be_bytes());
        hasher.update((label.len() as u64).to_be_bytes());
        hasher.update(label.as_bytes());
        let digest = hasher.finalize();
        u64::from_be_bytes(digest[..8].try_into().expect("eight bytes"))
    }

    fn next(state: &mut u64) -> u64 {
        let mut value = *state;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        *state = value;
        value
    }

    fn gaussian_milli(state: &mut u64) -> i128 {
        let span = 1_i128 << 24;
        let sum = (0..12).map(|_| i128::from(next(state) >> 40)).sum::<i128>();
        ((2 * sum - 12 * (span - 1)) * 1000) / (2 * span)
    }

    pub(super) fn box_point(
        (x, y, width, height): (i32, i32, i32, i32),
        action_seed: u64,
        touch: Option<(u32, u32)>,
    ) -> (i32, i32) {
        let mut state = if action_seed == 0 {
            0x9e37_79b9_7f4a_7c15
        } else {
            action_seed
        };
        let Some((sigma, max_redraws)) = touch else {
            let dx = next(&mut state) % u64::try_from(width).expect("width");
            let dy = next(&mut state) % u64::try_from(height).expect("height");
            return (
                x + i32::try_from(dx).expect("x"),
                y + i32::try_from(dy).expect("y"),
            );
        };
        let (x, y, width, height) = (
            i128::from(x),
            i128::from(y),
            i128::from(width),
            i128::from(height),
        );
        let (cx, cy) = (x + width / 2, y + height / 2);
        let sigma = i128::from(sigma);
        let mut point = (cx, cy);
        for _ in 0..=max_redraws {
            let zx = gaussian_milli(&mut state);
            let zy = gaussian_milli(&mut state);
            let px = cx + zx * sigma * width / 1_000_000;
            let py = cy + zy * sigma * height / 1_000_000;
            if (x..x + width).contains(&px) && (y..y + height).contains(&py) {
                point = (px, py);
                break;
            }
        }
        (
            i32::try_from(point.0).expect("x"),
            i32::try_from(point.1).expect("y"),
        )
    }
}

const WIDTH: u32 = 80;
const HEIGHT: u32 = 40;
// The click box of the package's one operation.
const BOX: (i32, i32, i32, i32) = (20, 10, 40, 20);

/// A context with no configuration, no recording session and no test touch configuration, and a
/// directory for the package and its frames. The environment lock is released last.
struct Isolated {
    files: TempDir,
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
            env::remove_var(TEST_TOUCH_CONFIG_ENV);
        }
        Self {
            _guard: guard,
            _app_env: app_env,
            files: TempDir::new().unwrap(),
        }
    }

    fn path(&self, name: &str) -> String {
        self.files.path().join(name).display().to_string()
    }

    /// The root configuration's touch keys: spread 250, 8 redraws.
    fn use_touch_section(&self) {
        let path = self.path("actingd.config.json");
        fs::write(
            &path,
            br#"{"touch": {"sampling": {"sigma_ratio_milli": 250, "max_redraws": 8}}}"#,
        )
        .unwrap();
        unsafe {
            env::set_var(TEST_TOUCH_CONFIG_ENV, &path);
        }
    }

    /// The package of one operation from `home` to `terminal` that presses BOX, guarded by the
    /// colour at (1, 0); a page is told by the colour at (0, 0).
    fn package(&self) -> String {
        let (x, y, width, height) = BOX;
        let color = |id: &str, at: u32, expected: [u8; 3]| {
            json!({"type": "color", "id": id,
                   "region": {"x": at, "y": 0, "width": 1, "height": 1}, "expected": expected})
        };
        let entries = [
            (
                "control.json",
                json!({
                    "schema_version": "Lab-1y.control.v1",
                    "package_id": "neutral.t4.offline",
                    "execution_mode": "navigable_route",
                    "game": "neutral",
                    "server": "test",
                    "resolution": {"width": WIDTH, "height": HEIGHT},
                    "entry_task_id": "task",
                    "capture_interval_ms": 1,
                    "step_timeout_ms": 10,
                    "timeout_ms": 100,
                    "max_steps": 2
                }),
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
                    "coordinate_space": {"width": WIDTH, "height": HEIGHT},
                    "entry_page": "home",
                    "target_page": "terminal",
                    "operations": [{
                        "id": "open_terminal",
                        "from": "home",
                        "to": "terminal",
                        "click": {"kind": "rect", "x": x, "y": y, "width": width,
                                  "height": height},
                        "guard": {
                            "page_id": "home",
                            "target_id": "guard/ready",
                            "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                            "color_probe": "guard/ready"
                        }
                    }]
                }),
            ),
            (
                "resources/recognition/neutral.test.pack.json",
                json!({
                    "schema_version": "0.3",
                    "game": "neutral",
                    "server": "test",
                    "coordinate_space": {"width": WIDTH, "height": HEIGHT},
                    "defaults": {"color_max_distance": 0.0},
                    "targets": [
                        color("page/home", 0, [255, 0, 0]),
                        color("page/terminal", 0, [0, 0, 255]),
                        color("guard/ready", 1, [0, 255, 0])
                    ]
                }),
            ),
            (
                "resources/recognition/neutral.test.pages.json",
                json!({"schema_version": "0.3", "pages": [
                    {"id": "neutral/home", "required": ["page/home"], "optional": [],
                     "forbidden": []},
                    {"id": "neutral/terminal", "required": ["page/terminal"], "optional": [],
                     "forbidden": []}
                ]}),
            ),
        ];
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, value) in entries {
            zip.start_file(name, options).expect("zip entry");
            zip.write_all(&serde_json::to_vec(&value).expect("JSON"))
                .expect("zip bytes");
        }
        let bytes = zip.finish().expect("finish zip").into_inner();
        let path = self.path("neutral.t4.offline.zip");
        fs::write(&path, bytes).unwrap();
        path
    }

    /// A PNG of a page: its colour at (0, 0) and, on `home`, the guard's at (1, 0).
    fn frame(&self, name: &str, page: [u8; 3], guard: [u8; 3]) -> String {
        let mut pixels = vec![0_u8; (WIDTH * HEIGHT * 3) as usize];
        pixels[..3].copy_from_slice(&page);
        pixels[3..6].copy_from_slice(&guard);
        let png = Frame::from_pixels(
            WIDTH,
            HEIGHT,
            pixels,
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("fixture frame")
        .encode_png_fast()
        .expect("fixture PNG");
        let path = self.path(name);
        fs::write(&path, png).unwrap();
        path
    }

    /// A frame directory: `home`, then `terminal`.
    fn frames(&self) -> String {
        let directory = self.files.path().join("frames");
        fs::create_dir(&directory).unwrap();
        for (name, page) in [("0001.png", [255, 0, 0]), ("0002.png", [0, 0, 255])] {
            let path = self.frame(name, page, [0, 255, 0]);
            fs::rename(&path, directory.join(name)).unwrap();
        }
        directory.display().to_string()
    }

    /// `package dry-run` of the package on its `home` frame, then `words`.
    fn dry_run(&self, out: &str, words: &[&str]) -> CliResult {
        let package = self.package();
        let home = self.frame("home.png", [255, 0, 0], [0, 255, 0]);
        let mut line = [
            "--json",
            "package",
            "dry-run",
            "--zip",
            package.as_str(),
            "--fixture",
            home.as_str(),
            "--out",
        ]
        .map(str::to_string)
        .to_vec();
        line.push(self.path(out));
        line.extend(words.iter().map(|word| (*word).to_string()));
        run_cli(line, true)
    }
}

impl Drop for Isolated {
    fn drop(&mut self) {
        unsafe {
            env::remove_var(TEST_TOUCH_CONFIG_ENV);
        }
    }
}

/// The answer's data of a command that succeeded.
#[track_caller]
fn data(result: &CliResult) -> Value {
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    result.envelope.data.clone().expect("data")
}

fn tap(point: (i32, i32)) -> Value {
    serde_json::to_value(InputAction::Tap {
        x: point.0,
        y: point.1,
    })
    .expect("tap JSON")
}

// Requirement 1: the dry run given the seed 41 draws its press from the action seed the reference
// derives for step 0 / `open_terminal` (uniform here: the test process has no touch section) and
// reports the seed; `replay --seed 41`, the reported seed, sends that very input with the same
// sampling, and reports the seed too.
#[test]
fn t4_a_replay_with_the_reported_seed_repeats_every_point() {
    let context = Isolated::new();
    let dry = data(&context.dry_run("dry.zip", &["--seed", "41"]));
    let action_seed = reference::action_seed(41, 0, "open_terminal");
    assert_eq!(dry["seed"], json!(41), "{dry}");
    assert_eq!(
        dry["decision"]["action"],
        tap(reference::box_point(BOX, action_seed, None)),
        "{dry}"
    );
    assert_eq!(
        dry["decision"]["sampling"]["action_seed"],
        json!(action_seed),
        "{dry}"
    );

    let reported = dry["seed"].as_u64().expect("the reported seed").to_string();
    let package = context.package();
    let frames = context.frames();
    let replayed = data(&run_cli(
        [
            "--json",
            "replay",
            "--package",
            package.as_str(),
            "--frames",
            frames.as_str(),
            "--seed",
            reported.as_str(),
        ],
        true,
    ));
    assert_eq!(replayed["seed"], json!(41), "{replayed}");
    let effect = &replayed["steps"][0]["effects"][0];
    assert_eq!(effect["action"], dry["decision"]["action"], "{replayed}");
    assert_eq!(
        effect["sampling"], dry["decision"]["sampling"],
        "{replayed}"
    );
}

// Requirement 1 and plan Q14: without a touch section the dry run (no `--seed`: the seed 0) draws
// its press uniformly from the reference's action seed and names the draw
// `xorshift64_uniform_rect_v1`, without parameters, over the click box.
#[test]
fn t4_without_a_touch_section_the_offline_draw_is_uniform_and_named() {
    let context = Isolated::new();
    let dry = data(&context.dry_run("dry.zip", &[]));
    let action_seed = reference::action_seed(0, 0, "open_terminal");
    let (x, y, width, height) = BOX;
    assert_eq!(dry["seed"], json!(0), "{dry}");
    assert_eq!(
        dry["decision"]["action"],
        tap(reference::box_point(BOX, action_seed, None)),
        "{dry}"
    );
    assert_eq!(
        dry["decision"]["sampling"],
        json!({
            "algorithm": "xorshift64_uniform_rect_v1",
            "action_seed": action_seed,
            "source_regions": [{"x": x, "y": y, "width": width, "height": height}]
        }),
        "{dry}"
    );
}

// Requirement 2 (offline): `do neutral_button --scene --dry-run --seed 23` with the root
// configuration's touch keys (spread 250, 8 redraws) plans the press in the target's click box
// (20, 10, 40, 20) at the reference's hotspot point for the command's first press labelled with
// the target, not at the centre (40, 20), and answers the seed and how it drew the point.
#[test]
fn t4_the_lab_presses_a_box_at_its_seeded_hotspot_point() {
    let context = Isolated::new();
    context.use_touch_section();
    let (x, y, width, height) = BOX;
    let root = TempDir::new().unwrap();
    let recognition = root.path().join("recognition");
    let navigation = root.path().join("navigation");
    fs::create_dir(&recognition).unwrap();
    fs::create_dir(&navigation).unwrap();
    let pack = recognition.join("neutral.test.pack.json");
    let pages = recognition.join("neutral.test.pages.json");
    let graph = navigation.join("neutral.test.navigation.json");
    fs::write(
        &pack,
        serde_json::to_vec(&json!({
            "schema_version": "0.3",
            "coordinate_space": {"width": 1, "height": 1},
            "targets": [
                {"type": "color", "id": "home_anchor",
                 "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
                {"type": "color", "id": "neutral_button",
                 "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0],
                 "click": {"x": x, "y": y, "width": width, "height": height}}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        &pages,
        br#"{"schema_version": "0.3", "pages": [{"id": "neutral/home", "required": ["home_anchor"]}]}"#,
    )
    .unwrap();
    fs::write(
        &graph,
        br#"{"schema_version": "0.3", "game": "neutral", "server": "test", "navigation": [], "destructive_actions": []}"#,
    )
    .unwrap();
    let fixture = seal_semantic_fixture(root, "neutral", "test", &pack, &pages, Some(&graph));
    let scene = context.path("scene.png");
    fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();

    let zip = fixture.zip.display().to_string();
    let planned = data(&run_cli(
        [
            "--json",
            "do",
            "neutral_button",
            "--scene",
            scene.as_str(),
            "--dry-run",
            "--seed",
            "23",
            "--zip",
            zip.as_str(),
        ],
        true,
    ));

    let action_seed = reference::action_seed(23, 0, "neutral_button");
    let (px, py) = reference::box_point(BOX, action_seed, Some((250, 8)));
    assert_ne!((px, py), (40, 20), "the reference point is not the centre");
    let click = &planned["actual_click"];
    assert_eq!(click["point"], json!({"x": px, "y": py}), "{planned}");
    assert_eq!(click["seed"], json!(23), "{planned}");
    assert_eq!(
        click["sampling"],
        json!({
            "algorithm": "center_hotspot_normal_v1",
            "action_seed": action_seed,
            "source_regions": [{"x": x, "y": y, "width": width, "height": height}],
            "parameters": {"sigma_ratio_milli": 250, "max_redraws": 8}
        }),
        "{planned}"
    );
}
