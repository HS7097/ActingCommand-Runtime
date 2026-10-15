// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 LAB-2 item 7 (wave-15 rulings; coordinator ruling 7 of wave 16; register §1.83):
// the kernel's navigation input reader accepts a REC-4 `offset` click,
// `{"kind": "offset", "target_id", "offset": {x, y, width, height}}`, the click a Lab recording
// generates for `--click` with `--click-follow` and source compilation for a `rect_move`. It
// presses the box at the top-left corner of the box its target matches plus the offset, the
// offset scaled by the factor a template with `scales` matched at (lab-recording.md, "Following
// a template"; REC-5). A package whose navigation holds one is therefore admitted for online
// observation, and so can serve as a carrier package, and its element is that box on the frame
// observed. Packages and frames are synthetic and neutral.

use crate::drive::parse_navigation_input;
use crate::{DriveNavigationGraph, DriveSemanticInput, PreparedPageObservation, drive_offset_rect};
use actingcommand_contract::page_projection::{FrameIdentity, FrameKind};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_recognition::resample::resample_pixels;
use actingcommand_recognition_pack::PackRect;
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;
const BACKGROUND: u8 = 60;
const ICON: u32 = 16;

// The click a Lab recording generates for `--click 24,20,8,8 --click-follow ui/zoom` on the
// icon's crop at (20, 16): the rectangle minus the crop's top-left corner.
fn offset_click() -> Value {
    json!({"kind": "offset", "target_id": "ui/zoom", "offset": {"x": 4, "y": 4, "width": 8, "height": 8}})
}

fn rect(x: i32, y: i32, width: i32, height: i32) -> PackRect {
    PackRect {
        x,
        y,
        width,
        height,
    }
}

// A deterministic generator of neutral gray noise.
struct Noise(u64);

impl Noise {
    fn sample(&mut self) -> u8 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        1 + ((self.0 >> 33) % 255) as u8
    }
}

// A gray image, row-major, drawn with equal red, green and blue.
struct Gray {
    width: u32,
    height: u32,
    values: Vec<u8>,
}

impl Gray {
    fn flat(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            values: vec![BACKGROUND; (width * height) as usize],
        }
    }

    fn icon() -> Self {
        let mut noise = Noise(0x0390_0771);
        Self {
            width: ICON,
            height: ICON,
            values: (0..ICON * ICON).map(|_| noise.sample()).collect(),
        }
    }

    // This image resampled by `factor_milli` with the primitive every scaled search uses.
    fn scaled(&self, factor_milli: u32) -> Self {
        let width = (self.width * factor_milli + 500) / 1000;
        let height = (self.height * factor_milli + 500) / 1000;
        Self {
            width,
            height,
            values: resample_pixels(&self.values, (self.width, self.height), 1, (width, height))
                .expect("resampled"),
        }
    }

    // `image` drawn with its top-left corner at `(x, y)`.
    fn draw(mut self, x: u32, y: u32, image: &Gray) -> Self {
        for row in 0..image.height {
            for column in 0..image.width {
                self.values[((y + row) * self.width + x + column) as usize] =
                    image.values[(row * image.width + column) as usize];
            }
        }
        self
    }

    fn png(&self) -> Vec<u8> {
        let pixels = self
            .values
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
        .expect("fixture frame")
        .encode_png_fast()
        .expect("fixture PNG")
    }
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

fn json_bytes(value: Value) -> Vec<u8> {
    serde_json::to_vec(&value).expect("JSON")
}

// An observation package (the Lab resource layout) whose home page shows the icon `ui/zoom`,
// a template with scales 1000 and 750, and whose one navigation edge from it is `click`.
fn package(click: Value) -> Vec<u8> {
    zip_entries(&[
        (
            "control.json",
            json_bytes(json!({
                "game": "neutral",
                "server": "test",
                "package_id": "neutral.navoffset",
                "entry_task_id": "task"
            })),
        ),
        (
            "resources/manifest.json",
            json_bytes(json!({"entry_task_id": "task"})),
        ),
        (
            "resources/operations/task/task.json",
            json_bytes(json!({"task_id": "task"})),
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            json_bytes(json!({
                "schema_version": "0.7",
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": WIDTH, "height": HEIGHT},
                "targets": [
                    {"type": "template", "id": "ui/zoom",
                     "template_path": "recognition/assets/zoom.png",
                     "region": "full_frame", "threshold": 0.9,
                     "scales": {"factors_milli": [1000, 750]}},
                    {"type": "color", "id": "page/detail",
                     "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                     "expected": [0, 0, 255]}
                ]
            })),
        ),
        ("resources/recognition/assets/zoom.png", Gray::icon().png()),
        (
            "resources/recognition/neutral.test.pages.json",
            json_bytes(json!({
                "schema_version": "0.6",
                "pages": [
                    {"id": "neutral/home", "required": ["ui/zoom"]},
                    {"id": "neutral/detail", "required": ["page/detail"]}
                ]
            })),
        ),
        (
            "resources/navigation/neutral.test.navigation.json",
            json_bytes(json!({
                "schema_version": "0.6",
                "game": "neutral",
                "server": "test",
                "navigation": [{
                    "id": "open_detail",
                    "from_page": "neutral/home",
                    "to_page": "neutral/detail",
                    "click": click,
                    "source": ""
                }]
            })),
        ),
    ])
}

// The navigation element `open_detail` of the observation of `frame`.
fn observed_element(observation: &PreparedPageObservation, frame: &Gray) -> Value {
    let evaluated = observation
        .evaluate(
            &frame.png(),
            FrameIdentity {
                kind: FrameKind::Rgb8,
                sha256: "0".repeat(64),
                width: WIDTH,
                height: HEIGHT,
            },
        )
        .unwrap_or_else(|error| panic!("observation: {error} ({})", error.cause()));
    assert_eq!(evaluated.projection.page, "neutral/home");
    evaluated
        .projection
        .elements
        .iter()
        .find(|element| element["resource_id"] == "open_detail")
        .cloned()
        .unwrap_or_else(|| panic!("no open_detail element in {:?}", evaluated.projection))
}

// The drive reader reads an `offset` click with its target and its offset, and a navigation
// graph holding one routes through it.
#[test]
fn navoffset_the_drive_reader_reads_an_offset_click_with_its_target_and_offset() {
    let expected = DriveSemanticInput::TargetOffset {
        target_id: "ui/zoom".to_string(),
        offset: rect(4, 4, 8, 8),
    };
    assert_eq!(
        parse_navigation_input(&offset_click()).expect("an offset click"),
        expected
    );

    let graph = DriveNavigationGraph::parse_json(
        &json!({
            "game": "neutral",
            "navigation": [{
                "id": "open_detail",
                "from_page": "neutral/home",
                "to_page": "neutral/detail",
                "click": offset_click(),
                "source": ""
            }]
        })
        .to_string(),
    )
    .expect("a navigation graph with an offset edge");
    let route = graph
        .find_route("neutral/home", "neutral/detail")
        .expect("route");
    assert_eq!(route.len(), 1);
    assert_eq!(route[0].input(), &expected);
}

// The box is the matched box's top-left corner plus the offset; a template matched at a declared
// factor scales the four numbers by it, (v * factor + 500) div 1000 with the sign of v, as the
// kernel's own `offset` click does (REC-5).
#[test]
fn navoffset_the_box_is_the_matched_corner_plus_the_offset_scaled_by_the_match_factor() {
    let matched = rect(300, 200, 18, 14);
    let offset = rect(30, -7, 41, 10);

    // 30 -> 23, -7 -> -5 (5.25 rounds to 5), 41 -> 31 (30.75), 10 -> 8 (7.5 rounds half up).
    assert_eq!(
        drive_offset_rect(matched, Some(750), offset).expect("scaled"),
        rect(323, 195, 31, 8)
    );
    assert_eq!(
        drive_offset_rect(matched, Some(1000), offset).expect("identity"),
        rect(330, 193, 41, 10)
    );
    // A template without scales keeps the declared offset.
    assert_eq!(
        drive_offset_rect(matched, None, offset).expect("declared"),
        rect(330, 193, 41, 10)
    );
}

// A package whose navigation holds an offset click is admitted for online observation (the
// carrier package's admission), and its element is the box the click presses on the frame
// observed: on the frame where the icon is at (20, 16) the box (24, 20, 8, 8), on the frame where
// it is zoomed to 750 at (52, 16) the scaled offset (3, 3, 6, 6) from there, (55, 19, 6, 6).
#[test]
fn navoffset_a_package_with_an_offset_click_is_observed_online_with_its_box_on_the_frame() {
    let observation = PreparedPageObservation::load("node.a", &package(offset_click()), &[], None)
        .unwrap_or_else(|error| {
            panic!(
                "the package is admitted for online observation: {error} ({})",
                error.cause()
            )
        });
    let icon = Gray::icon();

    let element = observed_element(&observation, &Gray::flat(WIDTH, HEIGHT).draw(20, 16, &icon));
    assert_eq!(element["target_id"], "ui/zoom", "{element}");
    assert_eq!(
        element["recognition_basis"], "target_evaluation",
        "{element}"
    );
    assert_eq!(element["actionable"], json!(true), "{element}");
    assert_eq!(
        element["input"],
        json!({"type": "tap", "rect": {"x": 24, "y": 20, "width": 8, "height": 8}, "point": {"x": 28, "y": 24}}),
        "{element}"
    );

    let zoomed = Gray::flat(WIDTH, HEIGHT).draw(52, 16, &icon.scaled(750));
    let element = observed_element(&observation, &zoomed);
    assert_eq!(element["actionable"], json!(true), "{element}");
    assert_eq!(
        element["input"],
        json!({"type": "tap", "rect": {"x": 55, "y": 19, "width": 6, "height": 6}, "point": {"x": 58, "y": 22}}),
        "{element}"
    );
}
