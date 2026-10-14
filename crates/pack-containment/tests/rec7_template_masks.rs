// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #396 REC-7 (REC-M v1.2.1 §9.1-§9.3, §12) through a source package admitted as the
// Runtime and `actinglab package bundle` admit one: a task schema 0.9 `maskRange` pair and `mask`
// object derive the same pack mask; `verify_templates[]` takes an absolute and a relative
// `color_check`, and evaluating such a target (the target a `verify_template` guard names)
// checks the color with the template; and a template below its flat-field floor is refused
// where the package is admitted, at the derived pack's threshold, with its flat-field score.
// Templates and frames are synthetic and neutral.

use actingcommand_contract::{
    ContentDirectory, ContentDirectoryVersion, PackageRef, ResourceDeclarationReason,
};
use actingcommand_pack_containment::{Containment, ContainmentError, InstanceId, LoadedBundle};
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::{PackRegion, RecognitionMask, RecognitionTarget};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 32;

/// An RGB image, black unless painted.
struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Canvas {
    fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; (width * height * 3) as usize],
        }
    }

    /// A gray image (equal red, green and blue) of `gray(x, y)`.
    fn gray(width: u32, height: u32, gray: impl Fn(u32, u32) -> u8) -> Self {
        let mut canvas = Self::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let value = gray(x, y);
                canvas.set(x, y, [value; 3]);
            }
        }
        canvas
    }

    fn set(&mut self, x: u32, y: u32, color: [u8; 3]) {
        let start = ((y * self.width + x) * 3) as usize;
        self.pixels[start..start + 3].copy_from_slice(&color);
    }

    fn paste(&mut self, left: u32, top: u32, image: &Canvas) {
        for y in 0..image.height {
            for x in 0..image.width {
                let start = ((y * image.width + x) * 3) as usize;
                let mut color = [0; 3];
                color.copy_from_slice(&image.pixels[start..start + 3]);
                self.set(left + x, top + y, color);
            }
        }
    }

    fn fill(&mut self, left: u32, top: u32, width: u32, height: u32, color: [u8; 3]) {
        for y in top..top + height {
            for x in left..left + width {
                self.set(x, y, color);
            }
        }
    }

    fn scene(&self) -> Scene {
        Scene::from_rgb8(self.width, self.height, &self.pixels).expect("scene")
    }

    /// An RGB PNG of one stored deflate block per 65,535 bytes.
    fn png(&self) -> Vec<u8> {
        let mut scanlines = Vec::new();
        for row in self.pixels.chunks((self.width * 3) as usize) {
            scanlines.push(0);
            scanlines.extend_from_slice(row);
        }
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut header = Vec::new();
        header.extend_from_slice(&self.width.to_be_bytes());
        header.extend_from_slice(&self.height.to_be_bytes());
        header.extend_from_slice(&[8, 2, 0, 0, 0]);
        chunk(&mut png, b"IHDR", &header);
        let mut zlib = vec![0x78, 0x01];
        let blocks = scanlines.chunks(65_535).collect::<Vec<_>>();
        for (index, block) in blocks.iter().enumerate() {
            zlib.push(u8::from(index + 1 == blocks.len()));
            let length = block.len() as u16;
            zlib.extend_from_slice(&length.to_le_bytes());
            zlib.extend_from_slice(&(!length).to_le_bytes());
            zlib.extend_from_slice(block);
        }
        let (mut a, mut b) = (1_u32, 0_u32);
        for byte in &scanlines {
            a = (a + u32::from(*byte)) % 65_521;
            b = (b + a) % 65_521;
        }
        zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
        chunk(&mut png, b"IDAT", &zlib);
        chunk(&mut png, b"IEND", &[]);
        png
    }
}

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let mut crc = 0xffff_ffff_u32;
    for byte in kind.iter().chain(data) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    png.extend_from_slice(&(!crc).to_be_bytes());
}

/// A source package of one task schema 0.9 `task` (its `anchors`, `verify_templates` and
/// `defaults` given) whose page `home` is its entry and target, with `assets` in its directory.
fn source_entries(
    defaults: Value,
    anchors: Value,
    verify_templates: Value,
    assets: &[(&str, &Canvas)],
) -> BTreeMap<String, Vec<u8>> {
    let control = json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.rec7",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": HEIGHT},
        "entry_task_id": "task"
    });
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "neutral REC-7 fixture",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": defaults,
        "anchors": anchors,
        "verify_templates": verify_templates,
        "entry_page": "home",
        "target_page": "home",
        "operations": []
    });
    let mut entries = BTreeMap::from([
        (
            "control.json".to_owned(),
            serde_json::to_vec_pretty(&control).expect("control JSON"),
        ),
        (
            "resources/operations/resources.json".to_owned(),
            br#"{"schema_version":"1.0","resources":[],"resource_count":0}"#.to_vec(),
        ),
        (
            "resources/operations/task/task.json".to_owned(),
            serde_json::to_vec_pretty(&task).expect("task JSON"),
        ),
    ]);
    for (path, image) in assets {
        entries.insert(format!("resources/operations/task/{path}"), image.png());
    }
    entries
}

/// Admits `entries` as the Runtime admits a source package.
fn admit(
    containment: &mut Containment,
    entries: BTreeMap<String, Vec<u8>>,
) -> Result<&LoadedBundle, ContainmentError> {
    containment.load_content_entries(
        &InstanceId::new("neutral-rec7").expect("instance"),
        entries,
        &PackageRef::ContentDirectory(ContentDirectory {
            schema_version: ContentDirectoryVersion::V1,
            sha256: "d".repeat(64),
        }),
        false,
        Instant::now() + Duration::from_secs(60),
    )
}

fn whole_frame() -> Value {
    json!({"mode": "rect", "rect": {"x": 0, "y": 0, "width": WIDTH, "height": HEIGHT}})
}

/// 0 and 255 in a checkerboard of 8 by 8 pixels.
fn checkerboard() -> Canvas {
    Canvas::gray(8, 8, |x, y| if (x + y) % 2 == 0 { 0 } else { 255 })
}

#[test]
fn a_mask_range_pair_and_a_mask_object_derive_the_same_pack_mask() {
    // Bright strokes in 200..=249 on a darker ground below 200.
    let strip = Canvas::gray(16, 8, |x, y| {
        if (x + y) % 3 == 0 {
            (200 + (x * 5 + y * 3) % 50) as u8
        } else {
            (60 + (x * 11 + y * 7) % 80) as u8
        }
    });
    let entries = source_entries(
        json!({"template_threshold": 0.9, "color_max_distance": 0.0, "match_metric": "ccoeff_normed"}),
        json!([
            {"id": "home", "template": "assets/STRIP.png", "region": whole_frame(),
             "maskRange": [200, 255]},
            {"id": "home_alt", "template": "assets/STRIP.png", "region": whole_frame(),
             "mask": {"type": "range", "lower": 200, "upper": 255}}
        ]),
        json!([]),
        &[("assets/STRIP.png", &strip)],
    );
    let mut containment = Containment::new();
    let bundle = admit(&mut containment, entries).expect("the masked source is admitted");
    let pack = bundle.evaluator().expect("a recognition pack").pack();

    assert_eq!(pack.schema_version, "0.7");
    for id in ["page/home", "page/home_alt"] {
        let target = pack
            .targets
            .iter()
            .find_map(|target| match target {
                RecognitionTarget::Template(template) if template.id == id => Some(template),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{id} is derived"));
        assert_eq!(
            target.mask,
            Some(RecognitionMask::Range {
                lower: 200,
                upper: 255
            }),
            "{id}"
        );
    }
}

#[test]
fn verify_templates_take_absolute_and_relative_color_checks_and_their_targets_check_the_color() {
    let entries = source_entries(
        json!({"template_threshold": 0.9, "color_max_distance": 0.0, "match_metric": "ccoeff_normed"}),
        json!([{"id": "home", "template": "assets/BOARD.png", "region": whole_frame()}]),
        json!([
            {"id": "verify/absolute", "template": "assets/ABSOLUTE.png", "region": whole_frame(),
             "threshold": 0.9,
             "color_check": {"region": {"mode": "rect", "rect": {"x": 40, "y": 20, "width": 4, "height": 4}},
                             "expected": [200, 40, 40]}},
            {"id": "verify/relative", "template": "assets/RELATIVE.png", "region": whole_frame(),
             "threshold": 0.9,
             "color_check": {"region": {"mode": "template_relative", "anchor_target_id": "verify/relative",
                                        "offset": {"x": 0, "y": 8}, "width": 8, "height": 2},
                             "expected": [40, 40, 200]}}
        ]),
        &[
            ("assets/BOARD.png", &checkerboard()),
            ("assets/ABSOLUTE.png", &checkerboard()),
            ("assets/RELATIVE.png", &checkerboard()),
        ],
    );
    let mut containment = Containment::new();
    let bundle =
        admit(&mut containment, entries).expect("verify_templates color checks are admitted");
    let evaluator = bundle.evaluator().expect("a recognition pack");
    for (id, relative) in [("verify/absolute", false), ("verify/relative", true)] {
        let check = evaluator
            .pack()
            .targets
            .iter()
            .find_map(|target| match target {
                RecognitionTarget::Template(template) if template.id == id => {
                    template.color_check.as_ref()
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("{id} carries its color check"));
        assert_eq!(
            matches!(check.region, PackRegion::TemplateRelative(_)),
            relative,
            "{id}"
        );
    }

    // The board at (10, 10); the absolute patch at (40, 20) and the relative one just below the
    // board, in their expected colors, or left black.
    let frame = |colored: bool| {
        let mut canvas = Canvas::new(WIDTH, HEIGHT);
        canvas.paste(10, 10, &checkerboard());
        if colored {
            canvas.fill(40, 20, 4, 4, [200, 40, 40]);
            canvas.fill(10, 18, 8, 2, [40, 40, 200]);
        }
        canvas.scene()
    };
    for id in ["verify/absolute", "verify/relative"] {
        let colored = evaluator
            .evaluate_target(&frame(true), id)
            .expect("evaluation in color");
        let black = evaluator
            .evaluate_target(&frame(false), id)
            .expect("evaluation without color");

        assert!(colored.passed, "{id}: {colored:?}");
        let matched = colored.template.expect("template evidence");
        assert_eq!((matched.x, matched.y), (10, 10), "{id}");
        assert!(!black.passed, "{id}: {black:?}");
    }
}

#[test]
fn a_template_below_its_flat_field_floor_is_refused_where_the_package_is_admitted() {
    // One gray value: the flat-field score is 1000 milli, above any threshold but 0.
    let flat = Canvas::gray(8, 8, |_, _| 120);
    let entries = source_entries(
        json!({"template_threshold": 0.9, "color_max_distance": 0.0}),
        json!([{"id": "home", "template": "assets/FLAT.png", "region": whole_frame()}]),
        json!([]),
        &[("assets/FLAT.png", &flat)],
    );
    let mut containment = Containment::new();
    let Err(ContainmentError::ResourceDeclaration { issue, detail }) =
        admit(&mut containment, entries)
    else {
        panic!("the flat ccorr_normed template is refused at admission");
    };

    assert!(
        issue
            .declaration_file
            .ends_with("recognition/neutral.test.pack.json"),
        "{issue:?}"
    );
    assert!(
        issue.field_path.starts_with("/targets/") && issue.field_path.ends_with("/threshold"),
        "{issue:?}"
    );
    assert_eq!(issue.reason, ResourceDeclarationReason::InvalidValue);
    assert!(
        detail
            .as_deref()
            .is_some_and(|detail| detail.contains("flat_score_milli=1000")),
        "{detail:?}"
    );
}
