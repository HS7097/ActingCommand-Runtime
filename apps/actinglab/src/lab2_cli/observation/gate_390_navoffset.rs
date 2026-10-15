// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 LAB-2 item 7 (wave-15 rulings; coordinator ruling 7 of wave 16; register §1.83):
// actinglab's navigation input reader accepts a REC-4 `offset` click,
// `{"kind": "offset", "target_id", "offset": {x, y, width, height}}`, the click a Lab recording
// generates for `--click` with `--click-follow` and source compilation for a `rect_move`, and
// `observe` resolves it to the box it presses on the frame: the top-left corner of the box its
// template matches plus the offset, the offset scaled by the factor a template with `scales`
// matched at (lab-recording.md, "Following a template"; REC-5). Frames are synthetic and neutral.

use super::*;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_recognition::resample::resample_pixels;
use actingcommand_recognition_pack::{AssetResolver, RecognitionPackError, RecognitionPackResult};
use std::collections::BTreeMap;
use std::sync::Arc;

const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;
const BACKGROUND: u8 = 60;
const ICON: u32 = 16;

// The click a Lab recording generates for `--click 24,20,8,8 --click-follow ui/zoom` on the
// icon's crop at (20, 16): the rectangle minus the crop's top-left corner.
fn offset_click() -> Value {
    json!({"kind": "offset", "target_id": "ui/zoom", "offset": {"x": 4, "y": 4, "width": 8, "height": 8}})
}

fn declared_offset() -> PackRect {
    PackRect {
        x: 4,
        y: 4,
        width: 8,
        height: 8,
    }
}

fn is_the_offset_click(input: &SemanticInput) -> bool {
    matches!(
        input,
        SemanticInput::TargetOffset { target_id, offset }
            if target_id == "ui/zoom" && *offset == declared_offset()
    )
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
    fn flat() -> Self {
        Self {
            width: WIDTH,
            height: HEIGHT,
            values: vec![BACKGROUND; (WIDTH * HEIGHT) as usize],
        }
    }

    fn icon() -> Self {
        let mut noise = Noise(0x0390_0772);
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

    fn rgb(&self) -> Vec<u8> {
        self.values.iter().flat_map(|value| [*value; 3]).collect()
    }

    fn scene(&self) -> Scene {
        Scene::from_rgb8(self.width, self.height, &self.rgb()).expect("scene")
    }

    fn png(&self) -> Vec<u8> {
        Frame::from_pixels(
            self.width,
            self.height,
            self.rgb(),
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("fixture frame")
        .encode_png_fast()
        .expect("fixture PNG")
    }
}

// The icon's template asset.
#[derive(Debug)]
struct ZoomAsset(Vec<u8>);

impl AssetResolver for ZoomAsset {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
        if path == "zoom.png" {
            Ok(self.0.clone())
        } else {
            Err(RecognitionPackError::fatal("no such fixture asset"))
        }
    }
}

// The icon `ui/zoom`, a template with scales 1000 and 750.
fn evaluator() -> RecognitionEvaluator {
    let pack = load_pack_from_json_str(
        &json!({
            "schema_version": "0.7",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": WIDTH, "height": HEIGHT},
            "targets": [
                {"type": "template", "id": "ui/zoom", "template_path": "zoom.png",
                 "region": "full_frame", "threshold": 0.9,
                 "scales": {"factors_milli": [1000, 750]}}
            ]
        })
        .to_string(),
    )
    .expect("the neutral pack");
    RecognitionEvaluator::with_asset_resolver(pack, Arc::new(ZoomAsset(Gray::icon().png())))
        .expect("evaluator")
}

fn tap(x: u32, y: u32, width: u32, height: u32) -> Option<Geometry> {
    Some(Geometry::Tap {
        rect: shared::Rect {
            x,
            y,
            width,
            height,
        },
        point: shared::Point {
            x: x + width / 2,
            y: y + height / 2,
        },
    })
}

// The reader reads an `offset` click with its target and its offset, as an action input and as
// a navigation edge of a graph.
#[test]
fn navoffset_actinglab_reads_an_offset_click_with_its_target_and_offset() {
    let input = parse_navigation_input(&offset_click()).expect("an offset click");
    assert!(is_the_offset_click(&input), "{input:?}");

    let graph = parse_navigation_graph_value(&json!({
        "game": "neutral",
        "navigation": [{
            "id": "open_detail",
            "from_page": "neutral/home",
            "to_page": "neutral/detail",
            "click": offset_click(),
            "source": ""
        }]
    }))
    .expect("a navigation graph with an offset edge");
    assert_eq!(graph.edges.len(), 1);
    assert!(
        is_the_offset_click(&graph.edges[0].input),
        "{:?}",
        graph.edges[0].input
    );
}

// observe resolves the click to the box it presses on the frame: on the frame where the icon is
// at (20, 16) the box (24, 20, 8, 8); on the frame where it is zoomed to 750 at (52, 16) the
// scaled offset (3, 3, 6, 6) from there, (55, 19, 6, 6), also after a `target` click of the same
// template was resolved on that frame.
#[test]
fn navoffset_actinglab_observe_resolves_an_offset_click_to_its_box_on_the_frame() {
    let evaluator = evaluator();
    let ocr = LabOcr::from_installation(None);
    let input = parse_navigation_input(&offset_click()).expect("an offset click");
    let icon = Gray::icon();

    let primary = Gray::flat().draw(20, 16, &icon).scene();
    let zoomed = Gray::flat().draw(52, 16, &icon.scaled(750)).scene();
    let target_click = SemanticInput::TargetCenter {
        target_id: "ui/zoom".to_string(),
    };
    let mut zoomed_resolved = BTreeMap::new();
    resolve(
        &target_click,
        &evaluator,
        &zoomed,
        &mut zoomed_resolved,
        &ocr,
    )
    .expect("the target click of the zoomed frame");

    let mut primary_resolved = BTreeMap::new();
    for (scene, resolved, expected) in [
        (&primary, &mut primary_resolved, tap(24, 20, 8, 8)),
        (&zoomed, &mut zoomed_resolved, tap(55, 19, 6, 6)),
    ] {
        match resolve(&input, &evaluator, scene, resolved, &ocr).expect("the offset click") {
            ElementResolution::Target {
                target_id,
                passed,
                geometry,
            } => {
                assert_eq!(target_id, "ui/zoom");
                assert!(passed);
                assert_eq!(geometry, expected);
            }
            other => panic!("not a target resolution: {other:?}"),
        }
    }
}
