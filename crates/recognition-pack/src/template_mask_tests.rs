// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #396 REC-7 logic tests (REC-M v1.2.1 §9.1, §9.3, §12): a schema 0.6 pack still
// refuses a template mask; a mask selects at least 16 template pixels, with some variance under
// ccoeff_normed; a masked see-through text strip still matches on a new background where the
// unmasked template does not; and the ccorr_normed threshold floor checked at admission.
// Templates and frames are synthetic neutral gray patterns.

use crate::*;
use actingcommand_contract::ResourceDeclarationReason;
use serde_json::{Value, json};

#[derive(Debug)]
struct Assets(HashMap<String, Vec<u8>>);

impl AssetResolver for Assets {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
        self.0
            .get(path)
            .cloned()
            .ok_or_else(|| RecognitionPackError::fatal("no such fixture asset"))
    }
}

/// A deterministic generator of neutral gray noise.
struct Noise(u64);

impl Noise {
    fn below(&mut self, limit: u64) -> u8 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % limit) as u8
    }
}

/// A gray image, row-major.
struct Gray {
    width: u32,
    height: u32,
    values: Vec<u8>,
}

impl Gray {
    fn from_fn(width: u32, height: u32, mut value: impl FnMut(u32, u32) -> u8) -> Self {
        let mut values = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                values.push(value(x, y));
            }
        }
        Self {
            width,
            height,
            values,
        }
    }

    /// Equal red, green and blue: its gray value is exactly `value` when read.
    fn rgb(&self) -> Vec<u8> {
        self.values.iter().flat_map(|value| [*value; 3]).collect()
    }

    fn scene(&self) -> Scene {
        Scene::from_rgb8(self.width, self.height, &self.rgb()).expect("scene")
    }

    /// An RGB PNG of one stored deflate block per 65,535 bytes.
    fn png(&self) -> Vec<u8> {
        let mut scanlines = Vec::new();
        for row in self.rgb().chunks((self.width * 3) as usize) {
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

/// A schema 0.7 pack of `targets` on a 96 by 48 frame under `metric`.
fn pack_json(metric: &str, template_threshold: Option<f64>, targets: Vec<Value>) -> Value {
    let mut defaults = json!({"match_metric": metric});
    if let Some(threshold) = template_threshold {
        defaults["template_threshold"] = json!(threshold);
    }
    json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": 96, "height": 48},
        "defaults": defaults,
        "targets": targets
    })
}

/// A full-frame template target on `t.png`; `threshold` and `mask` when given.
fn template(id: &str, threshold: Option<f64>, mask: Option<(u8, u8)>) -> Value {
    let mut target =
        json!({"type": "template", "id": id, "template_path": "t.png", "region": "full_frame"});
    if let Some(threshold) = threshold {
        target["threshold"] = json!(threshold);
    }
    if let Some((lower, upper)) = mask {
        target["mask"] = json!({"type": "range", "lower": lower, "upper": upper});
    }
    target
}

fn evaluator(pack: Value, template: &Gray) -> RecognitionEvaluator {
    let pack = load_pack_from_json_str(&pack.to_string()).expect("pack loads");
    RecognitionEvaluator::with_asset_resolver(
        pack,
        Arc::new(Assets(HashMap::from([(
            "t.png".to_owned(),
            template.png(),
        )]))),
    )
    .expect("evaluator")
}

/// The pointer and reason of a refusal at admission.
fn refusal(error: &RecognitionPackError) -> (&str, ResourceDeclarationReason) {
    let issue = error.declaration_issue().expect("a declaration refusal");
    (issue.field_path.as_str(), issue.reason)
}

/// 0 and 255 in a checkerboard: its flat-field score is 1 / sqrt(2), 707 milli.
fn checkerboard() -> Gray {
    Gray::from_fn(8, 8, |x, y| if (x + y) % 2 == 0 { 0 } else { 255 })
}

#[test]
fn a_schema_0_6_pack_still_refuses_a_mask_and_schema_0_7_reads_its_gray_range() {
    for (schema, refused) in [("0.6", true), ("0.7", false)] {
        let mut pack = pack_json(
            "ccoeff_normed",
            None,
            vec![template("strip", Some(0.9), Some((200, 255)))],
        );
        pack["schema_version"] = json!(schema);
        let result = load_pack_from_json_str(&pack.to_string());
        if refused {
            let error = result.expect_err("schema 0.6 refuses every mask");
            assert_eq!(error.code(), RecognitionPackErrorCode::UnsupportedTarget);
            assert_eq!(
                refusal(&error),
                (
                    "/targets/0/mask",
                    ResourceDeclarationReason::UnconsumedField
                )
            );
        } else {
            let pack = result.expect("schema 0.7 reads a gray-range mask");
            let RecognitionTarget::Template(target) = &pack.targets[0] else {
                panic!("a template target");
            };
            assert_eq!(
                target.mask,
                Some(RecognitionMask::Range {
                    lower: 200,
                    upper: 255
                })
            );
        }
    }
}

#[test]
fn a_mask_that_selects_fewer_than_16_template_pixels_is_refused() {
    // Unselected pixels are 50; the first `selected` pixels take varied values in the range.
    let masked = |selected: u32| {
        Gray::from_fn(8, 4, |x, y| {
            let index = y * 8 + x;
            if index < selected {
                (200 + index * 3) as u8
            } else {
                50
            }
        })
    };
    let pack = pack_json(
        "ccoeff_normed",
        None,
        vec![template("strip", Some(0.9), Some((200, 255)))],
    );

    evaluator(pack.clone(), &masked(16))
        .admit_template_assets()
        .expect("16 selected pixels are admitted");
    let error = evaluator(pack, &masked(15))
        .admit_template_assets()
        .expect_err("15 selected pixels are too few");

    assert_eq!(
        refusal(&error),
        ("/targets/0/mask", ResourceDeclarationReason::InvalidValue)
    );
}

#[test]
fn a_ccoeff_mask_whose_pixels_have_no_variance_is_refused() {
    let flat_inside = Gray::from_fn(8, 4, |x, _| if x < 5 { 220 } else { 50 });
    let error = evaluator(
        pack_json(
            "ccoeff_normed",
            None,
            vec![template("strip", Some(0.9), Some((200, 255)))],
        ),
        &flat_inside,
    )
    .admit_template_assets()
    .expect_err("20 selected pixels of one value");

    assert_eq!(
        refusal(&error),
        ("/targets/0/mask", ResourceDeclarationReason::InvalidValue)
    );
}

#[test]
fn a_masked_see_through_text_strip_matches_on_a_new_background_where_the_plain_template_fails() {
    // Text pixels take 200..=250 in a fixed layout; the background behind them is noise in
    // 0..=150, one noise in the template and another in the frame.
    let is_text = |x: u32, y: u32| (x * 7 + y * 3) % 11 < 4;
    let text = |x: u32, y: u32| (200 + (x + 2 * y) % 51) as u8;
    let mut template_noise = Noise(0x0396_0711);
    let strip = Gray::from_fn(32, 12, |x, y| {
        if is_text(x, y) {
            text(x, y)
        } else {
            template_noise.below(151)
        }
    });
    let mut frame_noise = Noise(0x0396_0712);
    let mut frame = Gray::from_fn(96, 48, |_, _| frame_noise.below(151));
    for y in 0..12 {
        for x in 0..32 {
            if is_text(x, y) {
                frame.values[((20 + y) * 96 + 40 + x) as usize] = text(x, y);
            }
        }
    }
    let evaluator = evaluator(
        pack_json(
            "ccoeff_normed",
            None,
            vec![
                template("strip/masked", Some(0.9), Some((200, 255))),
                template("strip/plain", Some(0.9), None),
            ],
        ),
        &strip,
    );
    evaluator
        .admit_template_assets()
        .expect("the masked strip is admitted");
    let scene = frame.scene();

    let masked = evaluator
        .evaluate_target(&scene, "strip/masked")
        .expect("masked evaluation");
    let plain = evaluator
        .evaluate_target(&scene, "strip/plain")
        .expect("plain evaluation");

    assert!(masked.passed, "{masked:?}");
    let matched = masked.template.expect("masked template evidence");
    assert_eq!((matched.x, matched.y), (40, 20));
    assert!(!plain.passed, "{plain:?}");
}

#[test]
fn a_ccorr_threshold_below_the_flat_field_floor_is_refused_and_one_at_it_is_admitted() {
    let admit = |threshold: f64| {
        evaluator(
            pack_json(
                "ccorr_normed",
                None,
                vec![template("board", Some(threshold), None)],
            ),
            &checkerboard(),
        )
        .admit_template_assets()
    };

    admit(0.757).expect("707 + 50 milli is the floor");
    let error = admit(0.756).expect_err("one milli below the floor");

    assert_eq!(error.code(), RecognitionPackErrorCode::InvalidPackage);
    assert_eq!(
        refusal(&error),
        (
            "/targets/0/threshold",
            ResourceDeclarationReason::InvalidValue
        )
    );
    assert!(
        error.message().contains("flat_score_milli=707"),
        "{}",
        error.message()
    );
}

#[test]
fn a_zero_threshold_is_not_checked_against_the_floor() {
    // One gray value: the flat-field score is 1, so any threshold above 0 lies below the floor.
    let flat = Gray::from_fn(8, 8, |_, _| 200);
    evaluator(
        pack_json(
            "ccorr_normed",
            None,
            vec![template("always", Some(0.0), None)],
        ),
        &flat,
    )
    .admit_template_assets()
    .expect("a threshold of 0 declares a target that always passes");
}

#[test]
fn ccoeff_templates_have_no_threshold_floor() {
    evaluator(
        pack_json(
            "ccoeff_normed",
            None,
            vec![template("board", Some(0.5), None)],
        ),
        &checkerboard(),
    )
    .admit_template_assets()
    .expect("ccoeff_normed scores a region of one brightness 0");
}

#[test]
fn a_floor_refusal_of_an_inherited_threshold_points_at_the_default() {
    let error = evaluator(
        pack_json(
            "ccorr_normed",
            Some(0.7),
            vec![template("board", None, None)],
        ),
        &checkerboard(),
    )
    .admit_template_assets()
    .expect_err("0.7 lies below 757 milli");

    assert_eq!(
        refusal(&error),
        (
            "/defaults/template_threshold",
            ResourceDeclarationReason::InvalidValue
        )
    );
    assert!(error.message().contains("flat_score_milli=707"));
}
