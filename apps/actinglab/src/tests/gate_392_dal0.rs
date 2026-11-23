// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #392 DA-L0 (register §1.521; `contracts/display-adapter.md` section 14; coordinator work
// orders rd-1123/brief-392-dal0.md, its tests 1 to 8, and rd-1123/brief-392-dal0-fix1.md, its
// tests 1 to 7): `actinglab layout probe`, offline, says where each template mark of a page went on
// a saved frame of the base's ratio or wider. A synthetic package whose base is 128x72 (16:9): its
// page `neutral/other`, first in the page set, requires two templates drawn on no frame; its page
// `neutral/wide` requires seven template marks (8x8 patterns of 2x2 cells, and `stretched`, a 40x8
// bar of a pattern, a fill and a pattern of low contrast), a colour and a check; its page
// `neutral/rules` requires `centre_a`, `repeated` (drawn twice on the wide frame) and `declared`,
// has `member`, `absent` and the colour in an `any_of` group, `optional` optional and `other_b`
// forbidden. Every template's search box is the recording's default box for a margin of 48 (its
// crop grown by 48 on each side and clamped to the base), but `declared`'s, whose box across is a
// search area declared by hand. Every 8-high mark lies on a row whose box gives its place back.
// Every frame is drawn on its scale frame in 2x2 blocks and enlarged by nearest neighbour, so that
// the probe's one area average gives the drawing back exactly. The patterns were chosen, and every
// score below checked, by a simulation of the exact `ccoeff_normed` scores (threshold 0.95): a drawn
// mark scores 1 at its place and at most 0.75 anywhere else; `stretched` 0.885 on the 20:9 frame,
// `absent` 0.707.
// - `dal0_t1_a_twenty_by_nine_frame_classes_each_mark_as_the_table_says` (1): the 240x108 frame (20:9)
//   is brought to 160x72 by 3/2, room 32, places 0, 16 and 32: `centre_a` and `centre_b` centre (dx
//   16), `start` start (0), `end` end (32), `inset` other (26: the end edge less an inset of 6),
//   `stretched` missing (its best place its base place, at its start), `absent` missing (its best
//   place 4 across and -48 down); the counts, the dx values and the reading "edges are followed; an
//   inset of about 6"; the other page missing twice, "not this page"; overall the thirteen marks.
// - `dal0_t2_a_frame_of_the_base_ratio_twice_its_size_has_every_mark_as_mapped` (2): 256x144, factor
//   2, no room: every mark of the wide page as_mapped at its base place, "as mapped: a frame of the
//   base's ratio".
// - `dal0_t3_a_taller_a_smaller_and_a_portrait_frame_are_refused_with_their_word` (3): 128x96
//   `ratio_below_base`, 96x54 `scale_below_base`, 72x128 `orientation_mismatch`, each
//   `lab_scene_size_unsupported` (exit 2) with the frame's size and the base's.
// - `dal0_t4_a_room_below_twice_the_tolerance_classes_no_mark` (4): 132x72, room 4: no mark has a
//   class, every count is 0 and every reading "the room is too small to tell"; the offsets stay.
// - `dal0_t5_without_a_page_named_the_page_on_the_frame_comes_first` (5): the wide page, second in
//   the page set, first in `pages_by_share`; with `--page neutral/other` that page alone; an
//   unknown page refused.
// - `dal0_t6_marks_of_the_other_families_are_listed_as_not_probed` (6): the colour and the check of
//   the wide page under `not_probed` with their rule, kind and declared rectangle, no mark of them.
// - `dal0_t7_the_answer_holds_no_member_beyond_its_list` (7; fix round 1, test 7): test 1's answer,
//   member by member at every depth with the members fix round 1 adds, and every text of it an id,
//   a page, a rule, a kind, a class, a way the base place was taken or a reading.
// - `dal0_t8_the_mcp_face_answers_the_command_lines_members` (8): through `serve --stdio --front
//   mcp`, the row `layout probe` on test 1's fixture answers exit 0 and the line the command line
//   prints, member for member.
// - `dal0_fix1_t1_a_box_cut_off_at_an_edge_gives_the_place_the_mark_was_cut_at` (fix 1): `start`,
//   10 pixels from the left edge, its box (0, 0, 66, 64) cut off at the left and the top, has its
//   base place (10, 8) by `default_growth` both ways, and on the 20:9 frame, where it stays at the
//   start edge, `dx` 0 and `start` (the box's middle would have said `dx` -19, `other`); `end`'s box
//   cut off at the right, `member`'s at the bottom: their places by the same rule.
// - `dal0_fix1_t2_a_declared_search_area_leaves_its_axis_unknown` (fix 2): `declared`, its box across
//   12 wide (no default growth), is `unknown` across and `default_growth` down: no `dx`, `dy` 0, no
//   class, counted `unknown`, out of its page's reading.
// - `dal0_fix1_t3_a_repeated_mark_is_counted_and_left_out_of_the_reading` (fix 3): `repeated`, drawn
//   twice on the 20:9 frame, answers `matches` 2 and `repeated` true with one of its two places;
//   its page reads "a centred base-ratio window" without it; drawn once on the base, 1 and false.
// - `dal0_fix1_t4_every_mark_a_page_names_is_probed_with_its_rule` (fix 4): the rules page's marks in
//   the order required, any_of, optional, forbidden, each with its rule, and the colour of its
//   `any_of` group under `not_probed` with the rule `any_of`.
// - `dal0_fix1_t5_a_frame_a_pixel_off_the_base_shape_has_no_room` (fix 5): 257x144 and 255x144, taken
//   by the run path's frame gate as the base's shape: no room, the base as the scale frame, factor
//   2, never `ratio_below_base` nor a room of 1.
// - `dal0_fix1_t6_the_boundary_of_too_small_and_a_tolerance_given` (fix 6): rooms 17 (too small: 8 is
//   at most 8) and 18 (not: 9) with the default tolerance 4; room 18 with `--tolerance-px 5` too
//   small (9 is at most 10).
// No Runtime, no device, no clock: temporary files and the embedded operation table.

use super::gate_434_lab5b4::printed;
use super::*;
use crate::cli_module::{self, serve};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_lab::LAB_RECORDING_DEFAULT_SEARCH_MARGIN;
use actingcommand_lab_ops::initiator::ENVIRONMENT_VARIABLE;
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

/// The package's base, its coordinate_space.
const BASE: (u32, u32) = (128, 72);
/// The gray of every pixel no mark covers.
const BACKGROUND: u8 = 128;
/// The gray of the bar's middle.
const FILL: u8 = 200;
/// Every template mark's threshold (the pack's default).
const THRESHOLD: f64 = 0.95;
const OTHER_PAGE: &str = "neutral/other";
const WIDE_PAGE: &str = "neutral/wide";
const RULES_PAGE: &str = "neutral/rules";

/// A square template mark: its id, its 4x4 cells (`#` white, `.` black, each 2x2 pixels) and its
/// base place, the corner of its 8x8 template.
type Square = (&'static str, [&'static str; 4], (u32, u32));

/// The square marks the base draws, at their base places.
const DRAWN: [Square; 10] = [
    ("start", ["###.", ".##.", ".##.", "...#"], (10, 8)),
    ("centre_a", ["#.#.", "###.", "#..#", "...#"], (44, 8)),
    ("repeated", ["#...", "..##", ".#.#", "#.##"], (80, 8)),
    ("end", ["#.#.", "..##", "##..", "#.#."], (116, 8)),
    ("declared", ["##.#", ".###", "#...", "...#"], (30, 56)),
    ("absent", ["#.##", "#.#.", ".#.#", ".#.."], (60, 56)),
    ("centre_b", ["##..", ".###", "#...", "#..#"], (76, 56)),
    ("inset", ["#.##", "..##", ".##.", ".#.."], (116, 56)),
    ("member", [".#..", "#.##", "..#.", ".###"], (84, 64)),
    ("optional", ["##.#", "#..#", "..#.", ".#.#"], (104, 64)),
];
/// The square marks of the other page, drawn on no frame.
const OTHER_SQUARES: [Square; 2] = [
    ("other_a", ["..#.", "..##", ".##.", "##.#"], (20, 8)),
    ("other_b", ["#...", "##..", ".###", ".##."], (100, 56)),
];
/// The bar: its id, its start (black and white), its end (96 and 160), its base place and the fill
/// between its ends at the base.
const BAR: &str = "stretched";
const BAR_START: [&str; 4] = ["..##", "##..", "#.##", "#..."];
const BAR_END: [&str; 4] = ["##..", ".##.", "##..", "..##"];
const BAR_AT: (u32, u32) = (4, 64);
const BAR_MIDDLE: u32 = 24;
/// Every template mark's search box (x, y, width, height), written by hand: the recording's
/// default box for a margin of 48 (the crop grown by 48 on each side and clamped to the base), but
/// `declared`'s, whose box across is 2 pixels around its crop.
const BOXES: [(&str, [i64; 4]); 13] = [
    ("start", [0, 0, 66, 64]),
    ("centre_a", [0, 0, 100, 64]),
    ("repeated", [32, 0, 96, 64]),
    ("end", [68, 0, 60, 64]),
    ("declared", [28, 8, 12, 64]),
    ("absent", [12, 8, 104, 64]),
    ("centre_b", [28, 8, 100, 64]),
    ("inset", [68, 8, 60, 64]),
    ("member", [36, 16, 92, 56]),
    ("optional", [56, 16, 72, 56]),
    ("other_a", [0, 0, 76, 64]),
    ("other_b", [52, 8, 76, 64]),
    (BAR, [0, 16, 92, 56]),
];
/// The colour mark and its rectangle.
const SWATCH: &str = "swatch";
const SWATCH_AT: (i64, i64, i64, i64) = (120, 64, 6, 6);
/// The check of the wide page.
const CHECK: &str = "check";
/// The marks the wide page requires, in its order.
const WIDE_REQUIRED: [&str; 9] = [
    "centre_a", "centre_b", "start", "end", "inset", BAR, "absent", SWATCH, CHECK,
];
/// The template marks of the rules page with their rules, in the order the page names them.
const RULES_MARKS: [(&str, &str); 7] = [
    ("centre_a", "required"),
    ("repeated", "required"),
    ("declared", "required"),
    ("member", "any_of"),
    ("absent", "any_of"),
    ("optional", "optional"),
    ("other_b", "forbidden"),
];

/// Where test 1's 20:9 frame draws the single square marks on its 160x72 scale frame: the centred
/// ones moved by half the room (16), `start` by nothing, `end` by the room (32), `inset` by the room
/// less an inset of 6, `declared` by nothing; `absent` is drawn nowhere. `repeated` is drawn at
/// [`WIDE_REPEATED`]. Its bar keeps its start at the start edge: its fill is 32 longer.
const WIDE_AT: [(&str, (u32, u32)); 8] = [
    ("start", (10, 8)),
    ("centre_a", (60, 8)),
    ("end", (148, 8)),
    ("declared", (30, 56)),
    ("centre_b", (92, 56)),
    ("inset", (142, 56)),
    ("member", (100, 64)),
    ("optional", (120, 64)),
];
/// `repeated` twice: at its base place and moved by the room.
const WIDE_REPEATED: [(u32, u32); 2] = [(80, 8), (112, 8)];
const WIDE_BAR_MIDDLE: u32 = 56;

/// A row of test 1's table: the mark, its base place (x, y), its template's size, the corner of its
/// best place, its offsets (dx, dy) and its class.
type TableRow = (
    &'static str,
    [i64; 2],
    [i64; 2],
    [i64; 2],
    [i64; 2],
    &'static str,
);

/// Test 1's table, in the wide page's order.
const WIDE_TABLE: [TableRow; 7] = [
    ("centre_a", [44, 8], [8, 8], [60, 8], [16, 0], "centre"),
    ("centre_b", [76, 56], [8, 8], [92, 56], [16, 0], "centre"),
    ("start", [10, 8], [8, 8], [10, 8], [0, 0], "start"),
    ("end", [116, 8], [8, 8], [148, 8], [32, 0], "end"),
    ("inset", [116, 56], [8, 8], [142, 56], [26, 0], "other"),
    ("stretched", [4, 64], [40, 8], [4, 64], [0, 0], "missing"),
    ("absent", [60, 56], [8, 8], [64, 8], [4, -48], "missing"),
];

/// A gray picture, row by row.
struct Picture {
    width: u32,
    height: u32,
    gray: Vec<u8>,
}

impl Picture {
    fn blank(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            gray: vec![BACKGROUND; (width * height) as usize],
        }
    }

    /// The 8x8 picture of 4x4 cells: `#` is `light`, `.` is `dark`.
    fn cells(rows: [&str; 4], dark: u8, light: u8) -> Self {
        let mut picture = Self::blank(8, 8);
        for (row, pattern) in rows.iter().enumerate() {
            for (column, cell) in pattern.chars().enumerate() {
                let value = if cell == '#' { light } else { dark };
                for y in 0..2 {
                    for x in 0..2 {
                        picture.set(column as u32 * 2 + x, row as u32 * 2 + y, value);
                    }
                }
            }
        }
        picture
    }

    fn set(&mut self, x: u32, y: u32, value: u8) {
        let index = (y * self.width + x) as usize;
        self.gray[index] = value;
    }

    fn at(&self, x: u32, y: u32) -> u8 {
        self.gray[(y * self.width + x) as usize]
    }

    /// `other` drawn with its top left corner at `corner`.
    fn paste(&mut self, other: &Picture, corner: (u32, u32)) {
        for y in 0..other.height {
            for x in 0..other.width {
                self.set(corner.0 + x, corner.1 + y, other.at(x, y));
            }
        }
    }

    /// This picture at `width` x `height` by nearest neighbour: pixel (x, y) shows this picture's
    /// (x * its width / width, y * its height / height). Enlarged 3/2 or 2 times, each 2x2 block
    /// stays whole, so that the area average gives this picture back.
    fn enlarged(&self, width: u32, height: u32) -> Self {
        let mut picture = Self::blank(width, height);
        for y in 0..height {
            for x in 0..width {
                picture.set(
                    x,
                    y,
                    self.at(x * self.width / width, y * self.height / height),
                );
            }
        }
        picture
    }

    /// The picture as an RGB PNG, every pixel gray.
    fn png(&self) -> Vec<u8> {
        let pixels = self
            .gray
            .iter()
            .flat_map(|value| [*value; 3])
            .collect::<Vec<_>>();
        Frame::from_pixels(
            self.width,
            self.height,
            pixels,
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("a fixture picture")
        .encode_png_fast()
        .expect("its PNG")
    }
}

/// The picture of the square mark `id`.
fn square(id: &str) -> Picture {
    let (_, rows, _) = DRAWN
        .into_iter()
        .chain(OTHER_SQUARES)
        .find(|(name, ..)| *name == id)
        .unwrap_or_else(|| panic!("{id} is a square mark"));
    Picture::cells(rows, 0, 255)
}

/// The bar with `middle` pixels of fill between its start and its end.
fn bar(middle: u32) -> Picture {
    let mut picture = Picture {
        width: 16 + middle,
        height: 8,
        gray: vec![FILL; ((16 + middle) * 8) as usize],
    };
    picture.paste(&Picture::cells(BAR_START, 0, 255), (0, 0));
    picture.paste(&Picture::cells(BAR_END, 96, 160), (8 + middle, 0));
    picture
}

/// The base's layout, every drawn mark at its base place, on a canvas of `width` x 72 (the base's
/// 128, or wider with background to its right).
fn base_layout(width: u32) -> Picture {
    let mut picture = Picture::blank(width, BASE.1);
    for (id, _, at) in DRAWN {
        picture.paste(&square(id), at);
    }
    picture.paste(&bar(BAR_MIDDLE), BAR_AT);
    picture
}

/// Test 1's scale frame: 160x72 as [`WIDE_AT`] and [`WIDE_REPEATED`] draw it.
fn wide_layout() -> Picture {
    let mut picture = Picture::blank(160, BASE.1);
    for (id, at) in WIDE_AT {
        picture.paste(&square(id), at);
    }
    for at in WIDE_REPEATED {
        picture.paste(&square("repeated"), at);
    }
    picture.paste(&bar(WIDE_BAR_MIDDLE), BAR_AT);
    picture
}

/// Where the package keeps the template of `id`, below its resource root.
fn asset(id: &str) -> String {
    format!("operations/task/assets/{id}.png")
}

/// The package: control, manifest and task, the recognition pack (schema 0.7, base 128x72,
/// `ccoeff_normed`, threshold 0.95), the page set (the other page first) and the templates.
fn package_bytes() -> Vec<u8> {
    let mut targets = BOXES
        .into_iter()
        .map(|(id, region)| {
            json!({
                "type": "template",
                "id": id,
                "template_path": asset(id),
                "region": {"x": region[0], "y": region[1], "width": region[2], "height": region[3]}
            })
        })
        .collect::<Vec<_>>();
    targets.push(json!({
        "type": "color",
        "id": SWATCH,
        "region": {"x": SWATCH_AT.0, "y": SWATCH_AT.1, "width": SWATCH_AT.2, "height": SWATCH_AT.3},
        "expected": [BACKGROUND, BACKGROUND, BACKGROUND]
    }));
    targets.push(json!({
        "type": "composite", "id": CHECK, "mode": "all_of", "members": [SWATCH, "start"]
    }));
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": BASE.0, "height": BASE.1},
        "defaults": {"template_threshold": THRESHOLD, "match_metric": "ccoeff_normed"},
        "targets": targets
    });
    let pages = json!({"schema_version": "0.3", "pages": [
        {"id": OTHER_PAGE, "required": ["other_a", "other_b"]},
        {"id": WIDE_PAGE, "required": WIDE_REQUIRED},
        {
            "id": RULES_PAGE,
            "required": ["centre_a", "repeated", "declared"],
            "any_of": [["member", "absent", SWATCH]],
            "optional": ["optional"],
            "forbidden": ["other_b"]
        }
    ]});
    let mut entries = vec![
        (
            "control.json".to_owned(),
            serde_json::to_vec(
                &json!({"game": "neutral", "server": "test", "entry_task_id": "task"}),
            )
            .expect("JSON"),
        ),
        (
            "resources/manifest.json".to_owned(),
            serde_json::to_vec(&json!({"schema_version": "0.3", "entry_task_id": "task"}))
                .expect("JSON"),
        ),
        (
            "resources/operations/task/task.json".to_owned(),
            b"{}".to_vec(),
        ),
        (
            "resources/recognition/neutral.test.pack.json".to_owned(),
            serde_json::to_vec(&pack).expect("JSON"),
        ),
        (
            "resources/recognition/neutral.test.pages.json".to_owned(),
            serde_json::to_vec(&pages).expect("JSON"),
        ),
    ];
    for (id, ..) in OTHER_SQUARES.into_iter().chain(DRAWN) {
        entries.push((format!("resources/{}", asset(id)), square(id).png()));
    }
    entries.push((format!("resources/{}", asset(BAR)), bar(BAR_MIDDLE).png()));
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, bytes) in &entries {
        zip.start_file(path.as_str(), options).expect("zip entry");
        zip.write_all(bytes).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

/// A context with no configuration, no recording session and an empty Runtime state root, the
/// package and its frames. The fields drop in order, the environment lock last.
struct Probe {
    package: String,
    files: TempDir,
    _runtime_env: RuntimeStateEnvGuard,
    _runtime: TempDir,
    _app_env: TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Probe {
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

    /// `picture` written as the scene file `name`: its path.
    fn scene(&self, name: &str, picture: &Picture) -> String {
        let path = self.files.path().join(format!("{name}.png"));
        fs::write(&path, picture.png()).unwrap();
        path.display().to_string()
    }

    /// The words of `--json layout probe --scene <scene> --zip <package>` and `more`.
    fn line<'a>(&'a self, scene: &'a str, more: &[&'a str]) -> Vec<&'a str> {
        let mut line = vec![
            "--json",
            "layout",
            "probe",
            "--scene",
            scene,
            "--zip",
            self.package.as_str(),
        ];
        line.extend(more);
        line
    }

    /// What the command line answers.
    fn run(&self, scene: &str, more: &[&str]) -> CliResult {
        run_cli(self.line(scene, more), true)
    }

    /// The command line's answer, a success: its data.
    #[track_caller]
    fn answered(&self, scene: &str, more: &[&str]) -> Value {
        let result = self.run(scene, more);
        let shown = result.envelope_json();
        assert_eq!(result.exit_code(), 0, "{shown}");
        result
            .envelope
            .data
            .clone()
            .unwrap_or_else(|| panic!("no data: {shown}"))
    }
}

/// Test 1's frame: 240x108 (20:9), its scale frame enlarged 3/2 times.
fn twenty_by_nine() -> Picture {
    wide_layout().enlarged(240, 108)
}

fn rect(x: i64, y: i64, width: i64, height: i64) -> Value {
    json!({"x": x, "y": y, "width": width, "height": height})
}

fn boxed(region: [i64; 4]) -> Value {
    rect(region[0], region[1], region[2], region[3])
}

fn size(width: u32, height: u32) -> Value {
    json!({"width": width, "height": height})
}

/// A base place on one axis: `at`, taken by `default_growth`, or `unknown`.
fn axis(at: Option<i64>) -> Value {
    match at {
        Some(at) => json!({"at": at, "by": "default_growth"}),
        None => json!({"at": null, "by": "unknown"}),
    }
}

/// The search box of the template mark `id`.
fn region_of(id: &str) -> [i64; 4] {
    BOXES
        .into_iter()
        .find(|(name, _)| *name == id)
        .map(|(_, region)| region)
        .unwrap_or_else(|| panic!("{id} is a template mark"))
}

/// The counts of a summary, in the classes' order, `unknown` last.
fn counts(classes: [u32; 6], unknown: u32) -> Value {
    let [as_mapped, start, centre, end, other, missing] = classes;
    json!({
        "as_mapped": as_mapped, "start": start, "centre": centre, "end": end, "other": other,
        "missing": missing, "unknown": unknown
    })
}

/// The page `id` of `answer`.
fn page<'a>(answer: &'a Value, id: &str) -> &'a Value {
    answer["pages"]
        .as_array()
        .expect("pages")
        .iter()
        .find(|candidate| candidate["page"] == id)
        .unwrap_or_else(|| panic!("no page {id}: {answer}"))
}

/// The mark `id` of the page answer `answered`.
fn mark<'a>(answered: &'a Value, id: &str) -> &'a Value {
    answered["marks"]
        .as_array()
        .expect("marks")
        .iter()
        .find(|candidate| candidate["id"] == id)
        .unwrap_or_else(|| panic!("no mark {id}: {answered}"))
}

/// The members of a JSON object, in order.
fn members(value: &Value) -> Vec<String> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("not a JSON object: {value}"))
        .keys()
        .cloned()
        .collect()
}

fn score(mark: &Value) -> f64 {
    mark["score"].as_f64().expect("a score")
}

/// The ids of a page answer's marks, in order.
fn mark_ids(answered: &Value) -> Vec<&str> {
    answered["marks"]
        .as_array()
        .expect("marks")
        .iter()
        .map(|entry| entry["id"].as_str().expect("an id"))
        .collect()
}

#[test]
fn dal0_t1_a_twenty_by_nine_frame_classes_each_mark_as_the_table_says() {
    let context = Probe::new();
    let scene = context.scene("wide", &twenty_by_nine());
    let answer = context.answered(&scene, &[]);

    assert_eq!(answer["frame"], size(240, 108), "{answer}");
    assert_eq!(answer["base"], size(BASE.0, BASE.1), "{answer}");
    assert_eq!(answer["factor"], json!({"num": 3, "den": 2}), "{answer}");
    assert_eq!(answer["scale_frame"], size(160, 72), "{answer}");
    assert_eq!(answer["room"], 32, "{answer}");
    assert_eq!(answer["tolerance_px"], 4, "{answer}");
    assert_eq!(
        answer["places"],
        json!({"start": 0, "centre": 16, "end": 32}),
        "{answer}"
    );
    assert_eq!(answer["room_too_small"], json!(false), "{answer}");

    let wide = page(&answer, WIDE_PAGE);
    for (id, base, template, best, offsets, class) in WIDE_TABLE {
        let found = mark(wide, id);
        assert_eq!(found["rule"], "required", "{id}: {found}");
        assert_eq!(found["region"], boxed(region_of(id)), "{id}: {found}");
        assert_eq!(
            found["template"],
            json!({"width": template[0], "height": template[1]}),
            "{id}: {found}"
        );
        assert_eq!(
            found["base"],
            json!({"x": axis(Some(base[0])), "y": axis(Some(base[1]))}),
            "{id}: {found}"
        );
        assert_eq!(
            found["best"],
            rect(best[0], best[1], template[0], template[1]),
            "{id}: {found}"
        );
        assert_eq!(found["dx"], offsets[0], "{id}: {found}");
        assert_eq!(found["dy"], offsets[1], "{id}: {found}");
        assert_eq!(found["class"], class, "{id}: {found}");
        assert_eq!(found["repeated"], json!(false), "{id}: {found}");
        assert!(
            (found["threshold"].as_f64().expect("a threshold") - THRESHOLD).abs() < 1e-6,
            "{id}: {found}"
        );
        if class == "missing" {
            assert!(score(found) < THRESHOLD, "{id}: {found}");
            assert_eq!(found["matches"], 0, "{id}: {found}");
        } else {
            assert!(score(found) > 0.999, "{id}: {found}");
            assert_eq!(found["matches"], 1, "{id}: {found}");
        }
    }
    assert_eq!(mark_ids(wide), WIDE_TABLE.map(|(id, ..)| id), "{wide}");
    assert_eq!(
        wide["summary"],
        json!({
            "probed": 7,
            "found": 5,
            "counts": counts([0, 1, 2, 1, 1, 2], 0),
            "dx_values": [
                {"dx": 0, "count": 1}, {"dx": 16, "count": 2}, {"dx": 26, "count": 1},
                {"dx": 32, "count": 1}
            ],
            "reading": "edges are followed; an inset of about 6"
        }),
        "{wide}"
    );

    let other = page(&answer, OTHER_PAGE);
    for id in ["other_a", "other_b"] {
        let found = mark(other, id);
        assert_eq!(found["class"], "missing", "{id}: {found}");
        assert!(score(found) < THRESHOLD, "{id}: {found}");
    }
    assert_eq!(
        other["summary"],
        json!({
            "probed": 2, "found": 0, "counts": counts([0, 0, 0, 0, 0, 2], 0), "dx_values": [],
            "reading": "not this page"
        }),
        "{other}"
    );
    // Overall: the thirteen template marks once, `repeated` at the start or at the end.
    let overall = &answer["overall"];
    assert_eq!(overall["probed"], 13, "{overall}");
    assert_eq!(overall["found"], 9, "{overall}");
    let tally = &overall["counts"];
    assert_eq!(
        (
            &tally["centre"],
            &tally["other"],
            &tally["missing"],
            &tally["unknown"]
        ),
        (&json!(4), &json!(1), &json!(4), &json!(1)),
        "{overall}"
    );
    assert_eq!(
        tally["start"].as_u64().expect("a count") + tally["end"].as_u64().expect("a count"),
        3,
        "{overall}"
    );
    assert_eq!(
        overall["reading"], "edges are followed; an inset of about 6",
        "{overall}"
    );
}

#[test]
fn dal0_t2_a_frame_of_the_base_ratio_twice_its_size_has_every_mark_as_mapped() {
    let context = Probe::new();
    let scene = context.scene("twice", &base_layout(BASE.0).enlarged(256, 144));
    let answer = context.answered(&scene, &[]);

    assert_eq!(answer["frame"], size(256, 144), "{answer}");
    assert_eq!(answer["factor"], json!({"num": 2, "den": 1}), "{answer}");
    assert_eq!(answer["scale_frame"], size(BASE.0, BASE.1), "{answer}");
    assert_eq!(answer["room"], 0, "{answer}");
    assert_eq!(
        answer["places"],
        json!({"start": 0, "centre": 0, "end": 0}),
        "{answer}"
    );
    assert_eq!(answer["room_too_small"], json!(false), "{answer}");
    let wide = page(&answer, WIDE_PAGE);
    for (id, base, template, ..) in WIDE_TABLE {
        let found = mark(wide, id);
        assert_eq!(
            found["best"],
            rect(base[0], base[1], template[0], template[1]),
            "{id}: {found}"
        );
        assert_eq!((&found["dx"], &found["dy"]), (&json!(0), &json!(0)), "{id}");
        assert_eq!(found["class"], "as_mapped", "{id}: {found}");
        assert!(score(found) > 0.999, "{id}: {found}");
    }
    assert_eq!(
        wide["summary"],
        json!({
            "probed": 7, "found": 7, "counts": counts([7, 0, 0, 0, 0, 0], 0),
            "dx_values": [{"dx": 0, "count": 7}],
            "reading": "as mapped: a frame of the base's ratio"
        }),
        "{wide}"
    );
    assert_eq!(
        page(&answer, OTHER_PAGE)["summary"]["reading"],
        "not this page",
        "{answer}"
    );
    assert_eq!(
        answer["pages_by_share"],
        json!([
            {"page": WIDE_PAGE, "found": 7, "probed": 7},
            {"page": RULES_PAGE, "found": 6, "probed": 7},
            {"page": OTHER_PAGE, "found": 0, "probed": 2}
        ]),
        "{answer}"
    );
}

#[test]
fn dal0_t3_a_taller_a_smaller_and_a_portrait_frame_are_refused_with_their_word() {
    let context = Probe::new();
    for (width, height, word) in [
        (128, 96, "ratio_below_base"),
        (96, 54, "scale_below_base"),
        (72, 128, "orientation_mismatch"),
    ] {
        let scene = context.scene(
            &format!("refused-{width}x{height}"),
            &Picture::blank(width, height),
        );
        let result = context.run(&scene, &[]);
        let shown = result.envelope_json();
        assert_eq!(result.exit_code(), 2, "{width}x{height}: {shown}");
        let error = result
            .envelope
            .error
            .as_ref()
            .unwrap_or_else(|| panic!("{width}x{height} was not refused: {shown}"));
        assert_eq!(
            error.code, "lab_scene_size_unsupported",
            "{width}x{height}: {shown}"
        );
        let details = error.details.clone().unwrap_or(Value::Null);
        assert_eq!(
            details["outcome"]["code"], "lab_scene_size_unsupported",
            "{width}x{height}: {shown}"
        );
        assert_eq!(
            details["outcome"]["values"],
            json!({
                "display_problem": word,
                "frame_width": width,
                "frame_height": height,
                "base_width": BASE.0,
                "base_height": BASE.1
            }),
            "{width}x{height}: {shown}"
        );
        assert_eq!(details["scene"], size(width, height), "{shown}");
        assert_eq!(details["coordinate_space"], size(BASE.0, BASE.1), "{shown}");
    }
}

#[test]
fn dal0_t4_a_room_below_twice_the_tolerance_classes_no_mark() {
    let context = Probe::new();
    // 132x72: the base's height, 4 columns more than its width; no resample.
    let scene = context.scene("narrow-room", &base_layout(132));
    let answer = context.answered(&scene, &[]);

    assert_eq!(answer["factor"], json!({"num": 1, "den": 1}), "{answer}");
    assert_eq!(answer["scale_frame"], size(132, 72), "{answer}");
    assert_eq!(answer["room"], 4, "{answer}");
    assert_eq!(
        answer["places"],
        json!({"start": 0, "centre": 2, "end": 4}),
        "{answer}"
    );
    assert_eq!(answer["room_too_small"], json!(true), "{answer}");
    let none = counts([0; 6], 0);
    for answered in answer["pages"].as_array().expect("pages") {
        for entry in answered["marks"].as_array().expect("marks") {
            assert_eq!(entry["class"], Value::Null, "{entry}");
        }
        assert_eq!(answered["summary"]["counts"], none, "{answered}");
        assert_eq!(
            answered["summary"]["reading"], "the room is too small to tell",
            "{answered}"
        );
    }
    assert_eq!(answer["overall"]["counts"], none, "{answer}");
    assert_eq!(
        answer["overall"]["reading"], "the room is too small to tell",
        "{answer}"
    );
    // The measurement stays: every mark of the wide page found where the base puts it.
    let wide = page(&answer, WIDE_PAGE);
    assert_eq!(wide["summary"]["found"], 7, "{wide}");
    assert_eq!(
        wide["summary"]["dx_values"],
        json!([{"dx": 0, "count": 7}]),
        "{wide}"
    );
    assert_eq!(
        (&mark(wide, "start")["dx"], &mark(wide, "start")["dy"]),
        (&json!(0), &json!(0)),
        "{wide}"
    );
}

#[test]
fn dal0_t5_without_a_page_named_the_page_on_the_frame_comes_first() {
    let context = Probe::new();
    let scene = context.scene("wide", &twenty_by_nine());
    let answer = context.answered(&scene, &[]);
    // The package's order: the other page first.
    assert_eq!(
        answer["pages"]
            .as_array()
            .expect("pages")
            .iter()
            .map(|entry| entry["page"].clone())
            .collect::<Vec<_>>(),
        [json!(OTHER_PAGE), json!(WIDE_PAGE), json!(RULES_PAGE)],
        "{answer}"
    );
    assert_eq!(
        members(&answer).first().map(String::as_str),
        Some("pages_by_share"),
        "the first line: {answer}"
    );
    // The wide page and the rules page each find 5 of their 7 marks: the package's order.
    assert_eq!(
        answer["pages_by_share"],
        json!([
            {"page": WIDE_PAGE, "found": 5, "probed": 7},
            {"page": RULES_PAGE, "found": 5, "probed": 7},
            {"page": OTHER_PAGE, "found": 0, "probed": 2}
        ]),
        "{answer}"
    );

    // A page named: that page alone.
    let named = context.answered(&scene, &["--page", OTHER_PAGE]);
    assert_eq!(
        named["pages_by_share"],
        json!([{"page": OTHER_PAGE, "found": 0, "probed": 2}]),
        "{named}"
    );
    let pages = named["pages"].as_array().expect("pages");
    assert_eq!(pages.len(), 1, "{named}");
    assert_eq!(pages[0], *page(&answer, OTHER_PAGE), "{named}");
    assert_eq!(named["overall"], pages[0]["summary"], "{named}");

    // A page the package does not have: refused before anything is searched.
    let result = context.run(&scene, &["--page", "neutral/nowhere"]);
    let shown = result.envelope_json();
    assert_eq!(result.exit_code(), 2, "{shown}");
    assert_eq!(
        result
            .envelope
            .error
            .as_ref()
            .map(|error| error.code.as_str()),
        Some("validation_failed"),
        "{shown}"
    );
}

#[test]
fn dal0_t6_marks_of_the_other_families_are_listed_as_not_probed() {
    let context = Probe::new();
    let scene = context.scene("wide", &twenty_by_nine());
    let answer = context.answered(&scene, &["--page", WIDE_PAGE]);
    let wide = page(&answer, WIDE_PAGE);
    assert_eq!(
        wide["not_probed"],
        json!([
            {
                "id": SWATCH,
                "rule": "required",
                "kind": "color",
                "region": rect(SWATCH_AT.0, SWATCH_AT.1, SWATCH_AT.2, SWATCH_AT.3)
            },
            {"id": CHECK, "rule": "required", "kind": "composite", "region": null}
        ]),
        "{wide}"
    );
    for id in [SWATCH, CHECK] {
        assert!(
            !mark_ids(wide).contains(&id),
            "{id} is no probed mark: {wide}"
        );
    }
    assert_eq!(wide["summary"]["probed"], 7, "{wide}");
}

/// Every member name at every depth of `value`, by where it is, checked against `expected`; every
/// text a member holds, gathered into `texts`.
fn shape(value: &Value, at: &str, texts: &mut Vec<String>, failures: &mut Vec<String>) {
    let expected: &[&str] = match at {
        "answer" => &[
            "pages_by_share",
            "frame",
            "base",
            "factor",
            "scale_frame",
            "room",
            "tolerance_px",
            "places",
            "room_too_small",
            "pages",
            "overall",
        ],
        "share" => &["page", "found", "probed"],
        "size" => &["width", "height"],
        "factor" => &["num", "den"],
        "places" => &["start", "centre", "end"],
        "page" => &["page", "marks", "not_probed", "summary"],
        "mark" => &[
            "id",
            "rule",
            "region",
            "template",
            "base",
            "best",
            "score",
            "threshold",
            "matches",
            "repeated",
            "dx",
            "dy",
            "class",
        ],
        "base_place" => &["x", "y"],
        "axis" => &["at", "by"],
        "not_probed" => &["id", "rule", "kind", "region"],
        "rect" => &["x", "y", "width", "height"],
        "summary" => &["probed", "found", "counts", "dx_values", "reading"],
        "counts" => &[
            "as_mapped",
            "start",
            "centre",
            "end",
            "other",
            "missing",
            "unknown",
        ],
        "dx" => &["dx", "count"],
        other => {
            failures.push(format!("no shape for {other}"));
            return;
        }
    };
    let Some(object) = value.as_object() else {
        failures.push(format!("{at}: not an object: {value}"));
        return;
    };
    if object.keys().map(String::as_str).collect::<Vec<_>>() != expected {
        failures.push(format!("{at}: members {:?}", object.keys()));
    }
    for (name, member) in object {
        let inner = match (at, name.as_str()) {
            ("answer", "pages_by_share") => Some("share"),
            ("answer", "frame" | "base" | "scale_frame") | ("mark", "template") => Some("size"),
            ("answer", "factor") => Some("factor"),
            ("answer", "places") => Some("places"),
            ("answer", "pages") => Some("page"),
            ("answer" | "page", "overall" | "summary") => Some("summary"),
            ("page", "marks") => Some("mark"),
            ("page", "not_probed") => Some("not_probed"),
            ("mark", "base") => Some("base_place"),
            ("base_place", "x" | "y") => Some("axis"),
            ("mark", "region" | "best") | ("not_probed", "region") => Some("rect"),
            ("summary", "counts") => Some("counts"),
            ("summary", "dx_values") => Some("dx"),
            _ => None,
        };
        let items = match member {
            Value::Array(items) => items.iter().collect::<Vec<_>>(),
            Value::Null => Vec::new(),
            single => vec![single],
        };
        for item in items {
            match (inner, item) {
                (Some(kind), _) => shape(item, kind, texts, failures),
                (None, Value::String(text)) => texts.push(text.clone()),
                (None, Value::Number(_) | Value::Bool(_)) => {}
                (None, other) => failures.push(format!("{at}.{name}: {other}")),
            }
        }
    }
}

#[test]
fn dal0_t7_the_answer_holds_no_member_beyond_its_list() {
    let context = Probe::new();
    let scene = context.scene("wide", &twenty_by_nine());
    let answer = context.answered(&scene, &[]);
    let mut texts = Vec::new();
    let mut failures = Vec::new();
    shape(&answer, "answer", &mut texts, &mut failures);
    assert!(failures.is_empty(), "{failures:#?}: {answer}");
    // Every text is a page, a mark's id, a rule, a kind, a class, a way a base place was taken or
    // a reading: nothing read from the frame.
    let ids = WIDE_REQUIRED
        .into_iter()
        .chain(RULES_MARKS.map(|(id, _)| id))
        .chain(["other_a"])
        .collect::<Vec<_>>();
    let words = [
        OTHER_PAGE,
        WIDE_PAGE,
        RULES_PAGE,
        "required",
        "any_of",
        "optional",
        "forbidden",
        "color",
        "composite",
        "centre",
        "start",
        "end",
        "other",
        "missing",
        "default_growth",
        "unknown",
        "not this page",
        "edges are followed; an inset of about 6",
        "a centred base-ratio window",
    ];
    for text in &texts {
        assert!(
            ids.contains(&text.as_str()) || words.contains(&text.as_str()),
            "a text the list does not hold: {text}"
        );
    }
}

#[test]
fn dal0_t8_the_mcp_face_answers_the_command_lines_members() {
    let context = Probe::new();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let scene = context.scene("wide", &twenty_by_nine());
    let command_line = printed(&context.line(&scene, &[]));
    assert_eq!(command_line.code, 0, "{:?}", command_line.line);
    let line = command_line
        .line
        .clone()
        .unwrap_or_else(|text| panic!("the command line printed no line: {text}"));
    // The line is the answer's members beside the line's own outcome.
    let mut answer = context.answered(&scene, &[]);
    answer
        .as_object_mut()
        .expect("an object")
        .insert("outcome".to_owned(), line["outcome"].clone());
    assert_eq!(line, answer, "{line}");

    let request = json!({
        "op": "layout probe",
        "params": {"scene": scene, "zip": context.package}
    });
    let table = cli_module::table().expect("the operation table");
    let mut output = Vec::new();
    let mut developer = Vec::new();
    serve::serve_as(
        table,
        Some(serve::Front::Mcp),
        format!("{request}\n").as_bytes(),
        &mut output,
        &mut developer,
    )
    .expect("the loop ends");
    let served = String::from_utf8(output).expect("UTF-8 lines");
    let lines = served.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "one line: {served}");
    let envelope = serde_json::from_str::<Value>(lines[0])
        .unwrap_or_else(|error| panic!("one JSON line ({error}): {served}"));
    assert_eq!(envelope["exit_code"], 0, "{envelope}");
    assert_eq!(
        envelope["line"], line,
        "the MCP face's members are the command line's"
    );
}

#[test]
fn dal0_fix1_t1_a_box_cut_off_at_an_edge_gives_the_place_the_mark_was_cut_at() {
    assert_eq!(
        LAB_RECORDING_DEFAULT_SEARCH_MARGIN, 48,
        "the boxes of this fixture are the recording's default boxes for a margin of 48"
    );
    let context = Probe::new();
    let scene = context.scene("wide", &twenty_by_nine());
    let answer = context.answered(&scene, &["--page", WIDE_PAGE]);
    let wide = page(&answer, WIDE_PAGE);
    // `start`, 10 from the left and 8 from the top: its box cut off at the left and the top. The
    // box's middle would have been (29, 28): dx -19, `other`.
    assert_eq!(region_of("start"), [0, 0, 66, 64]);
    // `end`: its box cut off at the right and the top.
    assert_eq!(region_of("end"), [68, 0, 60, 64]);
    // The mark, its base place, its best place on the 20:9 frame, its offsets and its class.
    for (id, base, best, offsets, class) in [
        ("start", [10, 8], [10, 8], [0, 0], "start"),
        ("centre_a", [44, 8], [60, 8], [16, 0], "centre"),
        ("end", [116, 8], [148, 8], [32, 0], "end"),
    ] {
        let found = mark(wide, id);
        assert_eq!(found["region"], boxed(region_of(id)), "{id}: {found}");
        assert_eq!(found["template"], size(8, 8), "{id}: {found}");
        assert_eq!(
            found["base"],
            json!({"x": axis(Some(base[0])), "y": axis(Some(base[1]))}),
            "{id}: {found}"
        );
        assert_eq!(found["best"], rect(best[0], best[1], 8, 8), "{id}: {found}");
        assert_eq!(
            (&found["dx"], &found["dy"]),
            (&json!(offsets[0]), &json!(offsets[1])),
            "{id}: {found}"
        );
        assert_eq!(found["class"], class, "{id}: {found}");
    }
    // Cut off at the bottom: its crop one margin inside its top.
    let whole = context.answered(&scene, &["--page", RULES_PAGE]);
    let member = mark(page(&whole, RULES_PAGE), "member");
    assert_eq!(member["region"], boxed([36, 16, 92, 56]), "{member}");
    assert_eq!(
        member["base"],
        json!({"x": axis(Some(84)), "y": axis(Some(64))}),
        "{member}"
    );
    assert_eq!(
        (&member["dx"], &member["dy"], &member["class"]),
        (&json!(16), &json!(0), &json!("centre")),
        "{member}"
    );
}

#[test]
fn dal0_fix1_t2_a_declared_search_area_leaves_its_axis_unknown() {
    let context = Probe::new();
    let scene = context.scene("wide", &twenty_by_nine());
    let answer = context.answered(&scene, &["--page", RULES_PAGE]);
    let rules = page(&answer, RULES_PAGE);
    let declared = mark(rules, "declared");
    assert_eq!(declared["region"], boxed([28, 8, 12, 64]), "{declared}");
    assert_eq!(
        declared["base"],
        json!({"x": axis(None), "y": axis(Some(56))}),
        "{declared}"
    );
    assert_eq!(declared["best"], rect(30, 56, 8, 8), "{declared}");
    assert!(score(declared) > 0.999, "{declared}");
    assert_eq!(
        (&declared["dx"], &declared["dy"], &declared["class"]),
        (&Value::Null, &json!(0), &Value::Null),
        "{declared}"
    );
    assert_eq!(rules["summary"]["counts"]["unknown"], 1, "{rules}");
    assert_eq!(rules["summary"]["counts"]["other"], 0, "{rules}");
    assert_eq!(
        rules["summary"]["reading"], "a centred base-ratio window",
        "{rules}"
    );
}

#[test]
fn dal0_fix1_t3_a_repeated_mark_is_counted_and_left_out_of_the_reading() {
    let context = Probe::new();
    let scene = context.scene("wide", &twenty_by_nine());
    let answer = context.answered(&scene, &["--page", RULES_PAGE]);
    let rules = page(&answer, RULES_PAGE);
    let repeated = mark(rules, "repeated");
    assert_eq!(repeated["matches"], 2, "{repeated}");
    assert_eq!(repeated["repeated"], json!(true), "{repeated}");
    assert!(score(repeated) > 0.999, "{repeated}");
    // Its best place is one of its two copies, classed by it.
    let best = (
        repeated["best"]["x"].as_i64().expect("x"),
        repeated["best"]["y"].as_i64().expect("y"),
    );
    let (dx, class) = match best {
        (80, 8) => (0, "start"),
        (112, 8) => (32, "end"),
        other => panic!("not one of the two copies: {other:?}: {repeated}"),
    };
    assert_eq!(
        (&repeated["dx"], &repeated["dy"], &repeated["class"]),
        (&json!(dx), &json!(0), &json!(class)),
        "{repeated}"
    );
    // The page reads its other found marks, all at the centre, as it would without `repeated`.
    let summary = &rules["summary"];
    assert_eq!(summary["counts"]["centre"], 3, "{summary}");
    assert_eq!(
        summary["reading"], "a centred base-ratio window",
        "{summary}"
    );

    // Drawn once, on the base: one place, not repeated.
    let once = context.scene("once", &base_layout(BASE.0));
    let base = context.answered(&once, &["--page", RULES_PAGE]);
    let single = mark(page(&base, RULES_PAGE), "repeated");
    assert_eq!(
        (&single["matches"], &single["repeated"], &single["class"]),
        (&json!(1), &json!(false), &json!("as_mapped")),
        "{single}"
    );
}

#[test]
fn dal0_fix1_t4_every_mark_a_page_names_is_probed_with_its_rule() {
    let context = Probe::new();
    let scene = context.scene("wide", &twenty_by_nine());
    let answer = context.answered(&scene, &["--page", RULES_PAGE]);
    let rules = page(&answer, RULES_PAGE);
    let named = rules["marks"]
        .as_array()
        .expect("marks")
        .iter()
        .map(|entry| {
            (
                entry["id"].as_str().expect("an id"),
                entry["rule"].as_str().expect("a rule"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(named, RULES_MARKS, "{rules}");
    assert_eq!(
        rules["not_probed"],
        json!([{
            "id": SWATCH,
            "rule": "any_of",
            "kind": "color",
            "region": rect(SWATCH_AT.0, SWATCH_AT.1, SWATCH_AT.2, SWATCH_AT.3)
        }]),
        "{rules}"
    );
    // Probed as the required ones are: the any_of member and the optional mark found at the
    // centre, the forbidden mark and the other any_of member not found.
    for (id, class) in [
        ("member", "centre"),
        ("optional", "centre"),
        ("absent", "missing"),
        ("other_b", "missing"),
    ] {
        assert_eq!(mark(rules, id)["class"], class, "{id}: {rules}");
    }
    assert_eq!(
        (&rules["summary"]["probed"], &rules["summary"]["found"]),
        (&json!(7), &json!(5)),
        "{rules}"
    );
}

#[test]
fn dal0_fix1_t5_a_frame_a_pixel_off_the_base_shape_has_no_room() {
    let context = Probe::new();
    for width in [257, 255] {
        let scene = context.scene(
            &format!("off-{width}"),
            &base_layout(BASE.0).enlarged(width, 144),
        );
        let answer = context.answered(&scene, &[]);
        assert_eq!(answer["frame"], size(width, 144), "{width}: {answer}");
        assert_eq!(
            answer["scale_frame"],
            size(BASE.0, BASE.1),
            "{width}: {answer}"
        );
        assert_eq!(answer["room"], 0, "{width}: {answer}");
        assert_eq!(
            answer["factor"],
            json!({"num": 2, "den": 1}),
            "{width}: {answer}"
        );
        assert_eq!(
            answer["places"],
            json!({"start": 0, "centre": 0, "end": 0}),
            "{width}: {answer}"
        );
        assert_eq!(answer["room_too_small"], json!(false), "{width}: {answer}");
    }
}

#[test]
fn dal0_fix1_t6_the_boundary_of_too_small_and_a_tolerance_given() {
    let context = Probe::new();
    // The base's height, 17 and 18 columns more than its width: no resample.
    let seventeen = context.scene("room-17", &base_layout(BASE.0 + 17));
    let answer = context.answered(&seventeen, &[]);
    assert_eq!(answer["room"], 17, "{answer}");
    assert_eq!(
        answer["places"],
        json!({"start": 0, "centre": 8, "end": 17}),
        "{answer}"
    );
    assert_eq!(answer["room_too_small"], json!(true), "{answer}");
    assert_eq!(
        mark(page(&answer, WIDE_PAGE), "start")["class"],
        Value::Null,
        "{answer}"
    );

    let eighteen = context.scene("room-18", &base_layout(BASE.0 + 18));
    let answer = context.answered(&eighteen, &[]);
    assert_eq!(answer["room"], 18, "{answer}");
    assert_eq!(
        answer["places"],
        json!({"start": 0, "centre": 9, "end": 18}),
        "{answer}"
    );
    assert_eq!(answer["room_too_small"], json!(false), "{answer}");
    assert_eq!(
        mark(page(&answer, WIDE_PAGE), "start")["class"],
        "start",
        "{answer}"
    );

    let given = context.answered(&eighteen, &["--tolerance-px", "5"]);
    assert_eq!(given["tolerance_px"], 5, "{given}");
    assert_eq!(given["room_too_small"], json!(true), "{given}");
    assert_eq!(
        given["overall"]["reading"], "the room is too small to tell",
        "{given}"
    );
}
