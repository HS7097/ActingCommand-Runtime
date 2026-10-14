// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #396 REC-5 logic tests (REC-M v1.2.1 §7, §12): a template shown at 0.75 matches at
// 750 milli with its resampled box and `scale_milli`; equal scores keep the earlier factor; a
// second place at another factor is a second place; factors out of range, repeated or too small
// once resampled are refused; a relative color region and a `template_relative` OCR region scale
// with the factor; a masked template matches once resampled; a scaled template cannot declare
// target consensus; a `ccoeff_normed` template whose only factor is 1000 evaluates as one without
// scales; and the frame is converted to the base coordinates before the template is resampled.
// Templates and frames are synthetic neutral patterns; OCR is a scripted provider.

use crate::*;
use actingcommand_contract::ResourceDeclarationReason;
use actingcommand_recognition::resample::{resample_pixels, scaled_length};
use serde_json::{Value, json};
use std::sync::Mutex;

const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// A deterministic generator of neutral gray noise.
struct Noise(u64);

impl Noise {
    /// A value in `low..=high`.
    fn between(&mut self, low: u8, high: u8) -> u8 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        low + ((self.0 >> 33) % (u64::from(high - low) + 1)) as u8
    }
}

/// A gray image, row-major.
#[derive(Clone)]
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

    fn noise(width: u32, height: u32, seed: u64) -> Self {
        let mut noise = Noise(seed);
        Self::from_fn(width, height, |_, _| noise.between(1, 255))
    }

    /// This image resampled by `factor_milli` with the primitive every search uses.
    fn scaled(&self, factor_milli: u32) -> Self {
        let (width, height) = (
            scaled_length(self.width, factor_milli),
            scaled_length(self.height, factor_milli),
        );
        Self {
            width,
            height,
            values: resample_pixels(&self.values, (self.width, self.height), 1, (width, height))
                .expect("resampled"),
        }
    }

    /// Equal red, green and blue: its gray value is exactly `value` when read.
    fn rgb(&self) -> Vec<u8> {
        self.values.iter().flat_map(|value| [*value; 3]).collect()
    }

    fn png(&self) -> Vec<u8> {
        encode_png(self.width, self.height, &self.rgb())
    }

    /// `image` drawn with its top-left corner at `(x, y)`; with `only`, the pixels it admits.
    fn draw(&mut self, x: u32, y: u32, image: &Gray, only: impl Fn(u8) -> bool) {
        for row in 0..image.height {
            for column in 0..image.width {
                let value = image.values[(row * image.width + column) as usize];
                if only(value) {
                    self.values[((y + row) * self.width + x + column) as usize] = value;
                }
            }
        }
    }

    fn scene(&self) -> Scene {
        Scene::from_rgb8(self.width, self.height, &self.rgb()).expect("scene")
    }
}

#[derive(Debug, Default)]
struct Assets(HashMap<String, Vec<u8>>);

impl AssetResolver for Assets {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
        self.0
            .get(path)
            .cloned()
            .ok_or_else(|| RecognitionPackError::fatal("no such fixture asset"))
    }
}

fn assets(template: &Gray) -> Arc<Assets> {
    Arc::new(Assets(HashMap::from([(
        "icon.png".to_owned(),
        template.png(),
    )])))
}

/// A template target on `icon.png` over `region`, threshold 0.9, declaring `factors` as its
/// scales unless there are none.
fn template(id: &str, region: Value, factors: &[u32]) -> Value {
    let mut target = json!({
        "type": "template", "id": id, "template_path": "icon.png", "region": region,
        "threshold": 0.9
    });
    if !factors.is_empty() {
        target["scales"] = json!({"factors_milli": factors});
    }
    target
}

fn pack_json(width: u32, height: u32, metric: &str, targets: Vec<Value>) -> Value {
    json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": width, "height": height},
        "defaults": {"match_metric": metric},
        "targets": targets
    })
}

fn evaluator(pack: Value, template: &Gray) -> RecognitionEvaluator {
    RecognitionEvaluator::with_asset_resolver(
        load_pack_from_json_str(&pack.to_string()).expect("pack loads"),
        assets(template),
    )
    .expect("evaluator")
}

fn rect(x: i32, y: i32, width: i32, height: i32) -> PackRect {
    PackRect {
        x,
        y,
        width,
        height,
    }
}

/// The box and factor of a template evaluation.
fn hit(evaluation: &TargetEvaluation) -> (PackRect, Option<u32>) {
    let template = evaluation.template.expect("template evidence");
    (
        rect(template.x, template.y, template.width, template.height),
        template.scale_milli,
    )
}

/// The pointer and reason of a refusal.
fn refusal(error: &RecognitionPackError) -> (&str, ResourceDeclarationReason) {
    let issue = error.declaration_issue().expect("a declaration refusal");
    (issue.field_path.as_str(), issue.reason)
}

#[test]
fn a_template_shown_at_three_quarters_matches_at_750_with_its_resampled_box() {
    let icon = Gray::noise(24, 18, 0x0396_0501);
    let mut frame = Gray::from_fn(320, 240, |_, _| 0);
    frame.draw(100, 60, &icon.scaled(750), |_| true);

    let evaluation = evaluator(
        pack_json(
            320,
            240,
            "ccoeff_normed",
            vec![template("icon/zoom", json!("full_frame"), &[1000, 750])],
        ),
        &icon,
    )
    .evaluate_target(&frame.scene(), "icon/zoom")
    .expect("evaluated");

    assert!(evaluation.passed, "{evaluation:?}");
    // 24 x 0.75 = 18 and 18 x 0.75 = 13.5, rounded half up to 14.
    assert_eq!(hit(&evaluation), (rect(100, 60, 18, 14), Some(750)));
    let value = serde_json::to_value(&evaluation).expect("serialize");
    assert_eq!(value.pointer("/template/scale_milli"), Some(&json!(750)));
}

#[test]
fn equal_scores_at_two_factors_keep_the_earlier_factor() {
    // One gray value in the template and in the frame: every position of every factor scores
    // `ccorr_normed` 1 exactly.
    let flat = Gray::from_fn(8, 8, |_, _| 100);
    let frame = Gray::from_fn(64, 48, |_, _| 200);
    let region = json!({"x": 10, "y": 10, "width": 40, "height": 30});
    for (factors, expected) in [
        ([750, 1000], (rect(10, 10, 6, 6), Some(750))),
        ([1000, 750], (rect(10, 10, 8, 8), Some(1000))),
    ] {
        let mut target = template("flat", region.clone(), &factors);
        target["threshold"] = json!(0.0);
        let evaluation = evaluator(pack_json(64, 48, "ccorr_normed", vec![target]), &flat)
            .evaluate_target(&frame.scene(), "flat")
            .expect("evaluated");

        assert!(evaluation.passed);
        assert_eq!(hit(&evaluation), expected, "{factors:?}");
    }
}

#[test]
fn a_second_place_at_another_factor_makes_a_unique_template_fail() {
    let icon = Gray::noise(24, 18, 0x0396_0502);
    let mut frame = Gray::from_fn(320, 240, |_, _| 0);
    frame.draw(40, 30, &icon, |_| true);
    frame.draw(200, 150, &icon.scaled(750), |_| true);
    let mut target = template("icon/unique", json!("full_frame"), &[1000, 750]);
    target["unique"] = json!({"margin_milli": 50});

    let evaluation = evaluator(pack_json(320, 240, "ccoeff_normed", vec![target]), &icon)
        .evaluate_target(&frame.scene(), "icon/unique")
        .expect("evaluated");

    assert!(!evaluation.passed);
    let failure = evaluation.failure.as_ref().expect("a typed failure");
    assert!(failure.is(RecognitionPackCode::TargetMatchNotUnique));
    let second = evaluation
        .uniqueness
        .and_then(|uniqueness| uniqueness.second_rect)
        .expect("the second place");
    let (best, _) = hit(&evaluation);
    // The two copies score alike; whichever is best, the other is its second place.
    let mut places = vec![best, second];
    places.sort_by_key(|place| place.x);
    assert_eq!(places, [rect(40, 30, 24, 18), rect(200, 150, 18, 14)]);
}

#[test]
fn factors_out_of_range_or_repeated_are_refused_when_the_pack_loads() {
    for (factors, pointer) in [
        (json!([1000, 499]), "/targets/0/scales/factors_milli/1"),
        (json!([2001]), "/targets/0/scales/factors_milli/0"),
        (json!([750, 1000, 750]), "/targets/0/scales/factors_milli/2"),
    ] {
        let mut target = template("icon/zoom", json!("full_frame"), &[]);
        target["scales"] = json!({"factors_milli": factors});
        let error = load_pack_from_json_str(
            &pack_json(320, 240, "ccoeff_normed", vec![target]).to_string(),
        )
        .expect_err("refused");

        assert_eq!(
            refusal(&error),
            (pointer, ResourceDeclarationReason::InvalidValue),
            "{factors}"
        );
    }
}

#[test]
fn a_factor_that_leaves_a_side_below_4_pixels_is_refused_at_admission() {
    let admit = |size: u32| {
        evaluator(
            pack_json(
                320,
                240,
                "ccoeff_normed",
                vec![template("icon/small", json!("full_frame"), &[1000, 500])],
            ),
            &Gray::noise(size, size, 0x0396_0503),
        )
        .admit_template_assets()
    };

    admit(8).expect("8 x 0.5 = 4 pixels a side are admitted");
    let error = admit(6).expect_err("6 x 0.5 = 3 pixels a side are too small");

    assert_eq!(
        refusal(&error),
        (
            "/targets/0/scales/factors_milli/1",
            ResourceDeclarationReason::InvalidValue
        )
    );
}

#[test]
fn a_relative_color_region_scales_with_the_factor_the_template_matched_at() {
    let icon = Gray::noise(24, 18, 0x0396_0504);
    let mut frame = Gray::from_fn(320, 240, |_, _| 0);
    frame.draw(100, 60, &icon.scaled(750), |_| true);
    let mut pixels = frame.rgb();
    // The declared region (30, 4, 8, 6) at 750 milli: (23, 3, 6, 5) from the match.
    for y in 63..68 {
        for x in 123..129 {
            let start = ((y * 320 + x) * 3) as usize;
            pixels[start..start + 3].copy_from_slice(&[255, 0, 0]);
        }
    }
    let scene = Scene::from_rgb8(320, 240, &pixels).expect("scene");
    let mut target = template("icon/red", json!("full_frame"), &[1000, 750]);
    target["color_check"] = json!({
        "region": {"mode": "template_relative", "anchor_target_id": "icon/red",
                   "offset": {"x": 30, "y": 4}, "width": 8, "height": 6},
        "expected": [255, 0, 0], "max_distance": 10.0
    });

    let evaluation = evaluator(pack_json(320, 240, "ccoeff_normed", vec![target]), &icon)
        .evaluate_target(&scene, "icon/red")
        .expect("evaluated");

    assert!(evaluation.passed, "{evaluation:?}");
    assert_eq!(hit(&evaluation), (rect(100, 60, 18, 14), Some(750)));
    assert_eq!(
        evaluation.color.and_then(|color| color.region),
        Some(rect(123, 63, 6, 5))
    );
}

type Respond = dyn Fn(PackRect) -> OcrProviderResult + Send + Sync;

/// Answers every OCR request with one block over its region and records each region.
struct ScriptedOcr {
    respond: Box<Respond>,
    regions: Mutex<Vec<PackRect>>,
}

impl fmt::Debug for ScriptedOcr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ScriptedOcr")
    }
}

impl VisionProvider for ScriptedOcr {
    fn require_ocr_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Ok(())
    }

    fn require_nn_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        self.regions
            .lock()
            .expect("region log")
            .push(request.region);
        Ok((self.respond)(request.region))
    }

    fn classify(
        &self,
        _request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }
}

#[test]
fn a_template_relative_ocr_region_scales_with_the_factor_its_anchor_matched_at() {
    let icon = Gray::noise(24, 18, 0x0396_0505);
    let mut frame = Gray::from_fn(320, 240, |_, _| 0);
    frame.draw(40, 30, &icon.scaled(750), |_| true);
    let provider = Arc::new(ScriptedOcr {
        respond: Box::new(|region| OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: "ALPHA".to_owned(),
            blocks: vec![OcrProviderTextBlock {
                text: "ALPHA".to_owned(),
                rect: region,
                confidence: Some(0.875),
            }],
            confidence: Some(0.875),
            detection: None,
        }),
        regions: Mutex::new(Vec::new()),
    });
    let ocr = json!({
        "type": "ocr", "id": "text/beside",
        "region": {"mode": "template_relative", "anchor_target_id": "icon/anchor",
                   "offset": {"x": 30, "y": 0}, "width": 80, "height": 20},
        "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": ["ALPHA"],
        "case_sensitive": false, "minimum_confidence": 0.8,
        "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256
    });

    let evaluation = RecognitionEvaluator::with_vision_provider(
        load_pack_from_json_str(
            &pack_json(
                320,
                240,
                "ccoeff_normed",
                vec![
                    template("icon/anchor", json!("full_frame"), &[1000, 750]),
                    ocr,
                ],
            )
            .to_string(),
        )
        .expect("pack loads"),
        assets(&icon),
        provider.clone(),
    )
    .expect("evaluator")
    .evaluate_target(&frame.scene(), "text/beside")
    .expect("evaluated");

    assert!(evaluation.passed, "{evaluation:?}");
    // The anchor matched at (40, 30) at 750 milli: offset (23, 0), size 60 x 15.
    let region = rect(63, 30, 60, 15);
    assert_eq!(*provider.regions.lock().expect("region log"), [region]);
    let evidence = &evaluation.ocr.as_ref().expect("OCR evidence").region;
    assert_eq!(evidence.roi, Some(OcrRegionRect::from(region)));
}

#[test]
fn a_masked_template_matches_once_resampled() {
    // Text blocks of 4 by 4 pixels in 200..=250, one value per block, over noise in 0..=150. At
    // 750 milli every resampled pixel lies inside one source block, so a block keeps its value.
    let mut background = Noise(0x0396_0506);
    let is_text = |x: u32, y: u32| (x / 4 + y / 4).is_multiple_of(3);
    let text = |x: u32, y: u32| (200 + (x / 4 * 7 + y / 4 * 3) % 51) as u8;
    let strip = Gray::from_fn(32, 16, |x, y| {
        if is_text(x, y) {
            text(x, y)
        } else {
            background.between(0, 150)
        }
    });
    let mut other = Noise(0x0396_0507);
    let mut frame = Gray::from_fn(160, 80, |_, _| other.between(0, 150));
    frame.draw(40, 20, &strip.scaled(750), |value| value >= 200);
    let mut target = template("strip", json!("full_frame"), &[1000, 750]);
    target["mask"] = json!({"type": "range", "lower": 200, "upper": 255});

    let evaluation = evaluator(pack_json(160, 80, "ccoeff_normed", vec![target]), &strip)
        .evaluate_target(&frame.scene(), "strip")
        .expect("evaluated");

    assert!(evaluation.passed, "{evaluation:?}");
    assert_eq!(hit(&evaluation), (rect(40, 20, 24, 12), Some(750)));
}

#[test]
fn a_scaled_template_cannot_declare_target_consensus() {
    let mut pack = pack_json(
        320,
        240,
        "ccoeff_normed",
        vec![template("icon/zoom", json!("full_frame"), &[1000, 750])],
    );
    pack["target_consensus"] = json!({"icon/zoom": {"samples": [{}], "k": 1}});

    let error = RecognitionEvaluator::with_asset_resolver(
        load_pack_from_json_str(&pack.to_string()).expect("pack loads"),
        assets(&Gray::noise(24, 18, 0x0396_0508)),
    )
    .expect_err("refused");

    assert!(
        error
            .message()
            .contains("template 'icon/zoom' declares scales, which target consensus does not take"),
        "{}",
        error.message()
    );
}

#[test]
fn a_ccoeff_template_whose_only_factor_is_1000_evaluates_as_one_without_scales() {
    let icon = Gray::noise(24, 18, 0x0396_0509);
    let mut background = Noise(0x0396_050a);
    let mut frame = Gray::from_fn(160, 120, |_, _| background.between(0, 255));
    frame.draw(50, 40, &icon, |_| true);
    let region = json!({"x": 20, "y": 10, "width": 100, "height": 90});
    let evaluator = evaluator(
        pack_json(
            160,
            120,
            "ccoeff_normed",
            vec![
                template("icon/plain", region.clone(), &[]),
                template("icon/scaled", region, &[1000]),
            ],
        ),
        &icon,
    );
    let scene = frame.scene();

    let plain = evaluator
        .evaluate_target(&scene, "icon/plain")
        .expect("evaluated");
    let scaled = evaluator
        .evaluate_target(&scene, "icon/scaled")
        .expect("evaluated");

    assert_eq!(scaled.passed, plain.passed);
    let (plain, scaled) = (
        plain.template.expect("template evidence"),
        scaled.template.expect("template evidence"),
    );
    assert_eq!(
        (scaled.x, scaled.y, scaled.width, scaled.height),
        (plain.x, plain.y, plain.width, plain.height)
    );
    assert_eq!(scaled.raw_score.to_bits(), plain.raw_score.to_bits());
    assert_eq!(scaled.score.to_bits(), plain.score.to_bits());
    assert_eq!((plain.scale_milli, scaled.scale_milli), (None, Some(1000)));
}

#[test]
fn the_frame_is_converted_to_the_base_coordinates_before_the_template_is_resampled() {
    // The package's base is 160 x 90 and its interface shows the icon at 0.75 there. The device
    // frame is twice as large; a stand-in for the Workflow #392 conversion brings it back to the
    // base with the same primitive before any template is searched.
    let icon = Gray::noise(24, 18, 0x0396_050b);
    let mut base = Gray::from_fn(160, 90, |_, _| 0);
    base.draw(50, 30, &icon.scaled(750), |_| true);
    let device = Gray::from_fn(320, 180, |x, y| base.values[(y / 2 * 160 + x / 2) as usize]);
    let evaluator = evaluator(
        pack_json(
            160,
            90,
            "ccoeff_normed",
            vec![template("icon/zoom", json!("full_frame"), &[1000, 750])],
        ),
        &icon,
    );

    evaluator
        .evaluate_target(&device.scene(), "icon/zoom")
        .expect_err("a device frame is not in the base coordinates");
    let converted = resample_pixels(&device.rgb(), (320, 180), 3, (160, 90)).expect("converted");
    assert_eq!(converted, base.rgb());
    let evaluation = evaluator
        .evaluate_target(
            &Scene::from_rgb8(160, 90, &converted).expect("scene"),
            "icon/zoom",
        )
        .expect("evaluated");

    assert!(evaluation.passed, "{evaluation:?}");
    assert_eq!(hit(&evaluation), (rect(50, 30, 18, 14), Some(750)));
}

/// An uncompressed RGB PNG.
fn encode_png(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    let mut scanlines = Vec::new();
    for row in pixels.chunks((width * 3) as usize) {
        scanlines.push(0);
        scanlines.extend_from_slice(row);
    }
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
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
