// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #392 DA-5ab (`contracts/display-adapter.md` section 11; the coordinator's work order of
// 11-16 on board #392, tests "observation" and "a display of another aspect, and one smaller than
// the base"): the Lab's online observation brings every frame to its package's base with the frame
// gate's own map and resample, and evaluates and projects there.
//
// - On frames three and one and a half times the base's size, made from the base frame by nearest
//   neighbour (every feature of the base frame lies on whole blocks of 2x2 pixels, so the area
//   average gives the base frame back exactly), the observation recognises the same page with the
//   same rectangles, on a projection frame of the base's size, and its RGB8 hash is the base
//   frame's. On the base frame itself the frame is used as it is: the projection's frame is the
//   identity the caller gave, and the RGB8 hash is that of the frame's own pixels.
// - A frame of another ratio, one smaller than the base and one of the other orientation are
//   refused with the code a run gives the same frame, `display_map_unresolved` with its
//   `display_problem` at `display_map_build` and the frame's and the base's sizes, never a
//   projection failure.
//
// The package and the frames are synthetic and neutral.

use crate::codes::ExecutionKernelCode;
use crate::{EvaluatedPageObservation, PreparedPageObservation};
use actingcommand_contract::PageObservationStatus;
use actingcommand_contract::page_projection::{FrameIdentity, FrameKind};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_recognition::Scene;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

/// The package's base.
const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;
const BACKGROUND: u8 = 60;
/// The icon's side, in pixels of the base; it is drawn in blocks of 2x2.
const ICON: u32 = 16;
/// Where the icon lies on the base frame (both even).
const ICON_AT: (u32, u32) = (20, 16);
/// The made-up digest of the device frame's artifact the host would name.
const ARTIFACT: &str = "5ab5ab5ab5ab5ab5ab5ab5ab5ab5ab5ab5ab5ab5ab5ab5ab5ab5ab5ab5ab5ab5";

/// A gray image, row-major, drawn with equal red, green and blue.
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

    /// A neutral icon of `ICON` x `ICON` pixels whose every 2x2 block has one value from a
    /// deterministic generator.
    fn icon() -> Self {
        let mut state = 0x0392_05ab_u64;
        let blocks = (0..(ICON / 2) * (ICON / 2))
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                1 + ((state >> 33) % 255) as u8
            })
            .collect::<Vec<_>>();
        Self {
            width: ICON,
            height: ICON,
            values: (0..ICON * ICON)
                .map(|index| {
                    let (x, y) = (index % ICON, index / ICON);
                    blocks[((y / 2) * (ICON / 2) + x / 2) as usize]
                })
                .collect(),
        }
    }

    /// `image` drawn with its top-left corner at `(x, y)`.
    fn draw(mut self, (x, y): (u32, u32), image: &Gray) -> Self {
        for row in 0..image.height {
            for column in 0..image.width {
                self.values[((y + row) * self.width + x + column) as usize] =
                    image.values[(row * image.width + column) as usize];
            }
        }
        self
    }

    /// This image enlarged by `num / den` by nearest neighbour: the output pixel `(x, y)` is the
    /// input pixel `(x * den / num, y * den / num)`, rounded down.
    fn enlarged(&self, num: u32, den: u32) -> Self {
        let (width, height) = (self.width * num / den, self.height * num / den);
        Self {
            width,
            height,
            values: (0..width * height)
                .map(|index| {
                    let (x, y) = (index % width * den / num, index / width * den / num);
                    self.values[(y * self.width + x) as usize]
                })
                .collect(),
        }
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

/// The base frame: the icon on a flat background.
fn base_frame() -> Gray {
    Gray::flat(WIDTH, HEIGHT).draw(ICON_AT, &Gray::icon())
}

fn json_bytes(value: Value) -> Vec<u8> {
    serde_json::to_vec(&value).expect("JSON")
}

/// An observation package (the Lab resource layout) of base `WIDTH` x `HEIGHT` whose home page
/// shows the icon `ui/icon`, and whose one navigation edge from it presses the box 4 pixels inside
/// the icon's matched box (an `offset` click), so that the element's rectangle is where the icon
/// was found on the frame.
fn package() -> Vec<u8> {
    let entries = [
        (
            "control.json",
            json_bytes(json!({
                "game": "neutral",
                "server": "test",
                "package_id": "neutral.da5ab",
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
                "defaults": {"match_metric": "ccoeff_normed"},
                "targets": [
                    {"type": "template", "id": "ui/icon",
                     "template_path": "recognition/assets/icon.png",
                     "region": "full_frame", "threshold": 0.9},
                    {"type": "color", "id": "page/detail",
                     "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                     "expected": [0, 0, 255]}
                ]
            })),
        ),
        ("resources/recognition/assets/icon.png", Gray::icon().png()),
        (
            "resources/recognition/neutral.test.pages.json",
            json_bytes(json!({
                "schema_version": "0.6",
                "pages": [
                    {"id": "neutral/home", "required": ["ui/icon"]},
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
                    "click": {"kind": "offset", "target_id": "ui/icon",
                              "offset": {"x": 4, "y": 4, "width": 8, "height": 8}},
                    "source": ""
                }]
            })),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, bytes) in entries {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&bytes).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

fn admitted() -> PreparedPageObservation {
    PreparedPageObservation::load("node.a", &package(), &[], None).unwrap_or_else(|error| {
        panic!(
            "the package is admitted for online observation: {error} ({})",
            error.cause()
        )
    })
}

/// The identity the host gives a device frame of `width` x `height`: its artifact's digest and
/// its own size.
fn device_identity(width: u32, height: u32) -> FrameIdentity {
    FrameIdentity {
        kind: FrameKind::Artifact,
        sha256: ARTIFACT.to_owned(),
        width,
        height,
    }
}

/// The observation of `frame`, inspecting.
fn observed(observation: &PreparedPageObservation, frame: &Gray) -> EvaluatedPageObservation {
    observation
        .evaluate(&frame.png(), device_identity(frame.width, frame.height))
        .unwrap_or_else(|error| {
            panic!(
                "{}x{}: observation: {error} ({})",
                frame.width,
                frame.height,
                error.cause()
            )
        })
}

/// The navigation element `open_detail`'s input of a projection.
fn element_input(evaluated: &EvaluatedPageObservation) -> Value {
    evaluated
        .projection
        .elements
        .iter()
        .find(|element| element["resource_id"] == "open_detail")
        .map(|element| element["input"].clone())
        .unwrap_or_else(|| panic!("no open_detail element in {:?}", evaluated.projection))
}

#[test]
fn da5ab_an_observation_on_a_frame_three_and_one_and_a_half_times_the_base_sees_the_base() {
    let observation = admitted();
    assert_eq!(observation.base(), (WIDTH, HEIGHT));
    let base = base_frame();

    // The base frame: recognised on its own pixels, the projection's frame the identity given.
    let on_base = observed(&observation, &base);
    assert_eq!(on_base.status, PageObservationStatus::Recognized);
    assert_eq!(on_base.projection.page, "neutral/home");
    assert_eq!(
        element_input(&on_base),
        json!({"type": "tap", "rect": {"x": 24, "y": 20, "width": 8, "height": 8}, "point": {"x": 28, "y": 24}}),
        "the icon matched at (20, 16)"
    );
    assert_eq!(on_base.projection.frame, device_identity(WIDTH, HEIGHT));
    let own = Scene::from_png(&base.png()).expect("the base frame's scene");
    assert_eq!(
        on_base.rgb8_sha256,
        format!("{:x}", Sha256::digest(own.rgb8_pixels())),
        "the base frame is observed as it is"
    );

    // Three and one and a half times the base: the same page, the same rectangles and the same
    // projection, on the base frame the reduction gives back.
    for (num, den) in [(3_u32, 1_u32), (3, 2)] {
        let device = base.enlarged(num, den);
        assert_eq!(
            (device.width, device.height),
            (WIDTH * num / den, HEIGHT * num / den),
            "setup: {num}/{den}"
        );
        let on_device = observed(&observation, &device);
        assert_eq!(
            on_device.status,
            PageObservationStatus::Recognized,
            "{num}/{den}"
        );
        assert_eq!(on_device.projection.page, "neutral/home", "{num}/{den}");
        assert_eq!(
            element_input(&on_device),
            element_input(&on_base),
            "{num}/{den}: the same rectangle, in the base's coordinates"
        );
        assert_eq!(
            on_device.projection.frame,
            device_identity(WIDTH, HEIGHT),
            "{num}/{den}: the device frame's artifact at the base's size"
        );
        assert_eq!(
            on_device.projection, on_base.projection,
            "{num}/{den}: the same projection"
        );
        assert_eq!(
            on_device.rgb8_sha256, on_base.rgb8_sha256,
            "{num}/{den}: the reduced frame is the base frame"
        );
    }
}

#[test]
fn da5ab_a_frame_of_another_ratio_or_smaller_than_the_base_is_the_runs_refusal() {
    let observation = admitted();
    for (width, height, problem) in [
        (96_u32, 80_u32, "layout_undeclared"),
        (48, 32, "scale_below_base"),
        (64, 96, "orientation_mismatch"),
    ] {
        let frame = Gray::flat(width, height);
        let Err(error) = observation.evaluate(&frame.png(), device_identity(width, height)) else {
            panic!("{width}x{height}: a frame the map cannot take is refused");
        };
        assert_eq!(
            error.code().as_str(),
            "display_map_unresolved",
            "{width}x{height}: {error} ({})",
            error.cause()
        );
        assert_ne!(
            error.code(),
            ExecutionKernelCode::ObservationProjectionFailed.code(),
            "{width}x{height}"
        );
        let values = error
            .values()
            .unwrap_or_else(|| panic!("{width}x{height}: the refusal carries its values"));
        assert_eq!(
            values.value("display_problem"),
            Some(&json!(problem)),
            "{width}x{height}"
        );
        assert_eq!(
            values.value("stage"),
            Some(&json!("display_map_build")),
            "{width}x{height}"
        );
        for (key, expected) in [
            ("frame_width", width),
            ("frame_height", height),
            ("base_width", WIDTH),
            ("base_height", HEIGHT),
        ] {
            assert_eq!(
                values.value(key),
                Some(&json!(expected)),
                "{width}x{height}: {key}"
            );
        }
    }
}
