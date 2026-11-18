// SPDX-License-Identifier: AGPL-3.0-only

// #463 PRIV-3 (contracts/lab-recording.md, "A recording's destination" and "Regression cases
// written with the package"; register §1.483; the owner's ruling of 11-18 that quantities an
// account holds, shown on a screen, are private and their pixels are never stored in a
// repository): a recording has a destination, and a public recording's scenes hold no pixel of a
// value reading; amendment 1 (the owner allows a package to read a quantity at run time, only its
// pixels may not be stored): a target that needs a withheld reading is left out of a case and
// named. Recognition packs as their JSON reads, and recordings made with record mark, on a 160x90
// base; frames are synthetic gray with neutral noise patches; a scripted OCR provider reads a text
// chosen by the gray value of the top-left pixel of the region it reads; no Runtime, no device; the
// tests read no clock and never sleep.
// - `priv3_a_recording_without_the_member_is_public_and_a_local_one_is_stored_and_answered` (1).
// - `priv3_a_public_recording_without_a_reading_generates_what_a_local_one_does` (2).
// - `priv3_a_public_value_test_is_left_out_of_its_case_and_its_side_withheld` (3, amendment 1).
// - `priv3_a_local_recording_is_todays_with_the_one_provenance_mark` (4).
// - `priv3_a_selects_quantity_column_is_withheld_and_a_shared_read_that_serves_it_as_a_whole` (5).
// - `priv3_a_banded_row_is_withheld_by_its_band_an_unbanded_one_refuses_and_a_reach_is_filled`
//   (6).
// - `priv3_a_box_drawn_over_a_reading_refuses_and_passes_once_the_recording_is_local` (7, amendment
//   1; fix round 1: the box is a colour probe, since a template's crop over a reading is refused
//   before any case, test 10).
// - `priv3_another_destination_at_stop_generates_again_and_the_same_does_nothing` (8).
// - `priv3_the_operation_table_carries_destination_with_its_closed_list` (9).
// Fix round 1 (rd-1119/brief-463-priv3-fix1.md):
// - `priv3_fix1_a_template_kept_over_a_reading_refuses_whether_or_not_its_case_passes` (10).
// - `priv3_fix1_an_unbounded_reading_refuses_the_self_check_cases_with_its_code` (11).
// The command line's third word is `actinglab`'s `gate_463_priv3_cli`.

use crate::recording::cases::{self, Patch, regression_cases, scene_regions};
use crate::recording::crosscheck::{Failures, FrameCache};
use crate::recording::frames::decode_frame;
use crate::recording::generate::{
    DEFAULT_APPLICATION_ARRIVAL_TIMEOUT_MS, DEFAULT_ARRIVAL_TIMEOUT_MS, StopSettings,
    plan as generate_plan,
};
use crate::recording::model::{LabRecording, RecordDestination, RecordSize, RecordWithheld};
use crate::recording::store::{old_record_path, recording_path};
use crate::recording_api::package_without_cases;
use crate::{
    ClickSpec, LAB_RECORD_MARK_SCHEMA, LabOcr, MarkFamily, MarkRequest, MarkSpec, RecordRect,
    RecordStartOptions, RecordStopOptions, RecordingLock, record_mark, record_start, record_status,
    record_stop,
};
use actingcommand_contract::LabError;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, PackReadingDeclarations, PackRect, RecognitionPack, VisionProvider,
    VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;

const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;
/// The recording's size and the package's base.
const SIZE: RecordSize = RecordSize {
    width: WIDTH,
    height: HEIGHT,
};
/// The made-up model of the OCR targets; no test loads it.
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const RED: [u8; 3] = [230, 20, 20];
const BLACK: [u8; 3] = [0, 0, 0];

const fn rect(x: i32, y: i32, width: i32, height: i32) -> PackRect {
    PackRect {
        x,
        y,
        width,
        height,
    }
}

/// A page square, a colour mark's region.
const SQUARE: PackRect = rect(0, 0, 16, 16);

// ---- Packs -----------------------------------------------------------------------------------

// A recognition pack of `targets`, the candidate layouts `layouts` and the target consensus
// `consensus` (`target_consensus`, by target id), as its JSON reads.
fn pack(targets: Value, layouts: Value, consensus: Value) -> RecognitionPack {
    serde_json::from_value(json!({
        "schema_version": "0.7",
        "game": "neutral",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "targets": targets,
        "candidate_layouts": layouts,
        "target_consensus": consensus
    }))
    .expect("a recognition pack")
}

fn color(id: &str, area: PackRect) -> Value {
    json!({"type": "color", "id": id, "region": area, "expected": RED})
}

// A template target searched for in `region`, its press box the same rectangle.
fn template(id: &str, region: PackRect) -> Value {
    json!({"type": "template", "id": id, "template_path": "templates/neutral.png",
           "region": region, "click": region})
}

fn ocr(id: &str, region: PackRect, expected: &[&str]) -> Value {
    json!({"type": "ocr", "id": id, "region": region, "languages": ["en"], "timeout_ms": 1000,
           "match_mode": "contains", "expected": expected, "case_sensitive": false,
           "minimum_confidence": 0.8, "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256})
}

// A value test that the number the OCR target `side` reads is at least 1: its side is a reading
// whatever the entry task declares.
fn at_least_one(id: &str, side: &str) -> Value {
    json!({"type": "value_test", "id": id,
           "left": {"target_id": side, "trim": "whitespace_v1",
                    "value": {"type": "unsigned_integer", "min": 0, "max": 999}},
           "op": "ge", "right": {"const": 1}})
}

// A select's identity feature `name`, read by OCR aliases; with `target` and `offset` when it reads
// its target at an offset.
fn identity_feature(name: &str, placed: Option<(&str, i32, i32)>) -> Value {
    let mut feature = json!({"name": name, "value": "identity", "identity": {
        "entries": [{"id": "alpha", "aliases": ["ALPHA"]}],
        "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                        "minimum_confidence_milli": 800}}});
    if let Some((target, x, y)) = placed {
        feature["target"] = json!(target);
        feature["offset"] = json!({"x": x, "y": y});
    }
    feature
}

// An integer feature `name` (`ocr_integer`, a quantity column), with `target` and `offset` when it
// reads its target at an offset.
fn integer_feature(name: &str, placed: Option<(&str, i32, i32)>) -> Value {
    let mut feature = json!({"name": name, "value": "ocr_integer",
                             "integer": {"min": 0, "max": 9999, "minimum_confidence_milli": 900}});
    if let Some((target, x, y)) = placed {
        feature["target"] = json!(target);
        feature["offset"] = json!({"x": x, "y": y});
    }
    feature
}

// A select's `fixed_slots` layout `select/slots` on page `list`: one slot `slot` (its press box
// too) whose name `slot/name` and quantity `slot/count` it reads, and a shared OCR read over
// `shared` when given.
fn select_layout(slot: PackRect, shared: Option<PackRect>) -> Value {
    let mut layout = json!({"id": "select/slots", "page_id": "list", "kind": "fixed_slots",
                            "features": [identity_feature("name", None),
                                         integer_feature("count", None)],
                            "slots": [{"rect": slot, "click": slot,
                                       "targets": {"name": "slot/name", "count": "slot/count"}}]});
    if let Some(shared) = shared {
        layout["shared_ocr"] = json!({"region": shared, "languages": ["en"], "timeout_ms": 1000,
                                      "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256,
                                      "blocks": "detect"});
    }
    layout
}

// A `repeated_anchor` layout `rows` on page `list`: the template `anchor`'s matches, searched at
// its own size, are its instances; it reads `row/count`'s quantity at the offset (40, 1) of each
// match, inside the instance's rectangle, and declares the readable band `band` when given.
fn rows_layout(band: Option<PackRect>) -> Value {
    let mut layout = json!({"id": "rows", "page_id": "list", "kind": "repeated_anchor",
                            "anchor": "row/anchor", "max_instances": 4, "order": "top_to_bottom",
                            "instance_rect": rect(0, 0, 60, 12), "click": rect(0, 0, 60, 12),
                            "features": [integer_feature("count", Some(("row/count", 40, 1)))]});
    if let Some(band) = band {
        layout["readable_band"] = json!(band);
    }
    layout
}

// ---- Scenes ----------------------------------------------------------------------------------

// Whether `area` covers the pixel at `(x, y)`.
fn covers(area: PackRect, x: u32, y: u32) -> bool {
    let (column, row) = (i64::from(x), i64::from(y));
    column >= i64::from(area.x)
        && row >= i64::from(area.y)
        && column < i64::from(area.x) + i64::from(area.width)
        && row < i64::from(area.y) + i64::from(area.height)
}

// A frame of distinct colours, none of them the canvas colour.
fn screen(x: u32, y: u32) -> [u8; 3] {
    [
        u8::try_from(1 + x % 200).expect("a channel"),
        u8::try_from(1 + y % 200).expect("a channel"),
        77,
    ]
}

// The PNG of a 160x90 frame whose pixel at `(x, y)` is `color(x, y)`, encoded as `backend` does.
fn png_of(color: &dyn Fn(u32, u32) -> [u8; 3], backend: CaptureBackendName) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            pixels.extend(color(x, y));
        }
    }
    Frame::from_pixels(WIDTH, HEIGHT, pixels, PixelFormat::Rgb8, backend)
        .expect("a frame")
        .encode_png_fast()
        .expect("a PNG")
}

// The scene of `screen` a public recording's case holds: a black canvas with the frame's pixels
// inside the rectangles `pasted` and outside every rectangle `withheld`, built here pixel by pixel,
// apart from the Lab's scene builder.
fn expected_scene(pasted: &[PackRect], withheld: &[PackRect]) -> Vec<u8> {
    png_of(
        &|x, y| {
            let shown = pasted.iter().any(|area| covers(*area, x, y))
                && !withheld.iter().any(|area| covers(*area, x, y));
            if shown { screen(x, y) } else { BLACK }
        },
        CaptureBackendName::AdbScreencap,
    )
}

// The patch of `area`, as the scene builder keys its rectangles.
fn patch_of(area: PackRect) -> Patch {
    let unsigned = |value: i32| u32::try_from(value).expect("a patch inside the frame");
    Patch {
        y: unsigned(area.y),
        x: unsigned(area.x),
        height: unsigned(area.height),
        width: unsigned(area.width),
    }
}

// The patches of `expected`, each with the targets that read it in order.
fn patches(expected: &[(PackRect, &[&str])]) -> BTreeMap<Patch, Vec<String>> {
    expected
        .iter()
        .map(|(area, readers)| {
            (
                patch_of(*area),
                readers.iter().map(|reader| (*reader).to_owned()).collect(),
            )
        })
        .collect()
}

// The scene the Lab builds of `package` for a public recording on the synthetic screen.
fn public_scene(package: &RecognitionPack, name: &str) -> Vec<u8> {
    let built = scene_regions(
        package,
        &PackReadingDeclarations::default(),
        SIZE,
        RecordDestination::Public,
    )
    .expect("the scene regions");
    let frame = decode_frame(png_of(&screen, CaptureBackendName::FixtureSimulation), name)
        .expect("the frame");
    built.scene(&frame, SIZE).expect("the scene")
}

// ---- Tests -----------------------------------------------------------------------------------

// The brief's test 5: a select whose slot holds a name and a quantity column (an `ocr_integer`
// feature's target, the list's `SelectQuantity`, a reading whatever the entry task declares),
// public: the quantity's region is withheld and filled, also where another target's patch (a colour
// probe over the panel) covers it, and the name's region is pasted; the answer names the rectangle
// and its target. The same select read by a shared read that serves the quantity column: the whole
// shared read's rectangle is withheld, the name inside it too. A local recording withholds nothing.
#[test]
fn priv3_a_selects_quantity_column_is_withheld_and_a_shared_read_that_serves_it_as_a_whole() {
    let name = rect(100, 60, 40, 12);
    let count = rect(100, 72, 20, 10);
    let panel = rect(96, 70, 30, 14);
    let slot = rect(100, 60, 40, 24);
    let targets = json!([
        color("page/square", SQUARE),
        color("panel/color", panel),
        ocr("slot/name", name, &["ALPHA"]),
        ocr("slot/count", count, &[])
    ]);
    let alone = pack(
        targets.clone(),
        json!([select_layout(slot, None)]),
        json!({}),
    );
    let built = scene_regions(
        &alone,
        &PackReadingDeclarations::default(),
        SIZE,
        RecordDestination::Public,
    )
    .expect("the scene regions");
    assert_eq!(
        built.pasted,
        patches(&[
            (SQUARE, &["page/square"]),
            (panel, &["panel/color"]),
            (name, &["slot/name"])
        ])
    );
    assert_eq!(built.withheld, patches(&[(count, &["slot/count"])]));
    assert!(built.unbounded.is_empty(), "{:?}", built.unbounded);
    assert_eq!(
        built.answer(&Vec::<String>::new()),
        Some(vec![RecordWithheld {
            x: 100,
            y: 72,
            width: 20,
            height: 10,
            targets: vec!["slot/count".to_owned()],
            forbidden: Vec::new(),
        }])
    );
    assert!(
        public_scene(&alone, "the select's screen")
            == expected_scene(&[SQUARE, panel, name], &[count]),
        "the quantity column is not the canvas colour in the scene"
    );
    // A local recording's scene regions are the pasted regions of today, nothing withheld.
    let local = scene_regions(
        &alone,
        &PackReadingDeclarations::default(),
        SIZE,
        RecordDestination::Local,
    )
    .expect("the local scene regions");
    assert!(local.withheld.is_empty() && local.unbounded.is_empty());
    assert_eq!(local.answer(&Vec::<String>::new()), None);

    // The shared read over the slot gives its text to the quantity column: withheld as a whole.
    let shared = pack(targets, json!([select_layout(slot, Some(slot))]), json!({}));
    let built = scene_regions(
        &shared,
        &PackReadingDeclarations::default(),
        SIZE,
        RecordDestination::Public,
    )
    .expect("the scene regions with the shared read");
    assert_eq!(
        built.withheld,
        patches(&[(slot, &["select/slots"]), (count, &["slot/count"])])
    );
    assert!(
        public_scene(&shared, "the shared read's screen")
            == expected_scene(&[SQUARE, panel, name], &[slot, count]),
        "the shared read's rectangle is not the canvas colour in the scene"
    );
}

// The brief's test 6: a quantity a `repeated_anchor` layout reads at an offset from its anchor's
// match, public. With a readable band, the bound the list gives, the band, is withheld whole (the
// anchor's patch inside it too), and the column's own region; without a band, its bound is the whole
// coordinate space and the recording cannot be public (`unbounded` names the target). A fixed
// reading (a value test's side) whose target consensus moves it 8 to the right: its reach, not only
// its area, is the canvas colour, inside another target's patch.
#[test]
fn priv3_a_banded_row_is_withheld_by_its_band_an_unbanded_one_refuses_and_a_reach_is_filled() {
    let band = rect(0, 50, 160, 40);
    let anchor = rect(150, 80, 10, 10);
    let column = rect(0, 0, 16, 10);
    let targets = json!([
        template("row/anchor", anchor),
        ocr("row/count", column, &[])
    ]);
    let banded = pack(targets.clone(), json!([rows_layout(Some(band))]), json!({}));
    let built = scene_regions(
        &banded,
        &PackReadingDeclarations::default(),
        SIZE,
        RecordDestination::Public,
    )
    .expect("the banded scene regions");
    assert_eq!(built.pasted, patches(&[(anchor, &["row/anchor"])]));
    assert_eq!(
        built.withheld,
        patches(&[(column, &["row/count"]), (band, &["row/count"])])
    );
    assert!(built.unbounded.is_empty(), "{:?}", built.unbounded);
    assert!(
        public_scene(&banded, "the banded list's screen")
            == expected_scene(&[anchor], &[column, band]),
        "the band is not the canvas colour in the scene"
    );

    let unbanded = pack(targets, json!([rows_layout(None)]), json!({}));
    let built = scene_regions(
        &unbanded,
        &PackReadingDeclarations::default(),
        SIZE,
        RecordDestination::Public,
    )
    .expect("the unbanded scene regions");
    assert_eq!(built.unbounded, vec!["row/count".to_owned()]);
    assert_eq!(built.withheld, patches(&[(column, &["row/count"])]));
    // The self-check `cases` refuses each target `unbounded` names with the one code.
    let refusal = cases::reading_unbounded("row/count");
    assert_eq!(refusal.code, "record_case_reading_unbounded");
    assert_eq!(
        refusal.details.as_ref().map(|details| &details["target"]),
        Some(&json!("row/count"))
    );

    let area = rect(40, 40, 20, 10);
    let reach = rect(40, 40, 28, 10);
    let probe = rect(36, 36, 40, 20);
    let jittered = pack(
        json!([
            ocr("text/count", area, &[]),
            at_least_one("check/count", "text/count"),
            color("probe/over", probe)
        ]),
        json!([]),
        json!({"text/count": {"samples": [{}, {"dx": 8}], "k": 1}}),
    );
    let built = scene_regions(
        &jittered,
        &PackReadingDeclarations::default(),
        SIZE,
        RecordDestination::Public,
    )
    .expect("the jittered scene regions");
    assert_eq!(built.pasted, patches(&[(probe, &["probe/over"])]));
    assert_eq!(built.withheld, patches(&[(reach, &["text/count"])]));
    assert!(
        public_scene(&jittered, "the jittered reading's screen")
            == expected_scene(&[probe], &[reach]),
        "the reading's reach is not the canvas colour in the scene"
    );
}

// ---- Recordings ------------------------------------------------------------------------------

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_priv3";
const TASK_ID: &str = "neutral_task";
const BACKGROUND: u8 = 60;
/// The template of step 1 and the template of step 2, each searched at its crop.
const BUTTON: RecordRect = record_rect(8, 8, 12, 12);
const NEXT: RecordRect = record_rect(8, 40, 12, 12);
/// The tap of step 1.
const TAP: RecordRect = record_rect(26, 24, 8, 8);
/// The region a value test's left side reads: a quantity on the screen.
const LEFT: RecordRect = record_rect(50, 4, 40, 10);
/// A colour probe drawn over the quantity's region (test 7).
const BADGE: RecordRect = record_rect(46, 2, 12, 12);
/// Fix round 1 (test 10): a template whose crop meets the quantity's region, and the part of it
/// that does, which its screen shows black, the canvas colour.
const CORNER: RecordRect = record_rect(80, 8, 12, 12);
const CORNER_OVER: RecordRect = record_rect(80, 8, 10, 6);
/// The gray value of a region's top-left pixel that makes the scripted provider read `25000`.
const READS_25000: u8 = 101;

const fn record_rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

// Reads `25000` where the top-left pixel of the requested region is `READS_25000`, at confidence
// 0.95; any other pixel reads nothing.
#[derive(Debug)]
struct ScriptedOcr;

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
        let index = (request.region.y as usize * request.frame.width as usize
            + request.region.x as usize)
            * 3;
        let text = if request.frame.rgb8_pixels.get(index).copied() == Some(READS_25000) {
            "25000"
        } else {
            ""
        };
        let blocks = if text.is_empty() {
            Vec::new()
        } else {
            vec![OcrProviderTextBlock {
                text: text.to_owned(),
                rect: request.region,
                confidence: Some(0.95),
            }]
        };
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: text.to_owned(),
            blocks,
            confidence: Some(0.95),
            detection: None,
        })
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

// A deterministic generator of neutral gray noise.
struct Noise(u64);

impl Noise {
    // A value in `low..=high`.
    fn between(&mut self, low: u8, high: u8) -> u8 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        low + ((self.0 >> 33) % (u64::from(high - low) + 1)) as u8
    }
}

// A gray image, row-major; it is drawn with equal red, green and blue.
#[derive(Clone)]
struct Gray {
    width: u32,
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
        Self { width, values }
    }

    fn frame() -> Self {
        Self::from_fn(WIDTH, HEIGHT, |_, _| BACKGROUND)
    }

    fn noise(seed: u64) -> Self {
        let mut noise = Noise(seed);
        Self::from_fn(12, 12, |_, _| noise.between(1, 255))
    }

    // `image` drawn with its top-left corner at the rectangle's.
    fn draw(mut self, at: RecordRect, image: &Gray) -> Self {
        let rows = image.values.len() as u32 / image.width;
        for row in 0..rows {
            for column in 0..image.width {
                self.values[((at.y as u32 + row) * self.width + at.x as u32 + column) as usize] =
                    image.values[(row * image.width + column) as usize];
            }
        }
        self
    }

    // The top-left pixel of `region` set to `value`, which selects what the provider reads there.
    fn reads(mut self, region: RecordRect, value: u8) -> Self {
        self.values[(region.y as u32 * self.width + region.x as u32) as usize] = value;
        self
    }

    // The gray value at `(x, y)`.
    fn at(&self, x: u32, y: u32) -> u8 {
        self.values[(y * self.width + x) as usize]
    }

    fn png(&self) -> Vec<u8> {
        png_of(
            &|x, y| [self.at(x, y); 3],
            CaptureBackendName::FixtureSimulation,
        )
    }
}

// Step 1's screen: the button, and the quantity at `LEFT`, which reads 25000.
fn first() -> Gray {
    Gray::frame()
        .draw(BUTTON, &Gray::noise(0x0463_0301))
        .reads(LEFT, READS_25000)
}

// Step 1's screen with a badge drawn over the quantity's region (test 7).
fn badged() -> Gray {
    Gray::frame()
        .draw(BUTTON, &Gray::noise(0x0463_0301))
        .draw(BADGE, &Gray::noise(0x0463_0303))
        .reads(LEFT, READS_25000)
}

// Fix round 1 (test 10): step 1's screen with a template at `CORNER` over the quantity's region,
// black where the two meet, so that the canvas colour a public scene fills the quantity's region
// with changes nothing of the template there.
fn cornered() -> Gray {
    Gray::frame()
        .draw(BUTTON, &Gray::noise(0x0463_0301))
        .draw(CORNER, &Gray::noise(0x0463_0304))
        .draw(CORNER_OVER, &Gray::from_fn(10, 6, |_, _| 0))
        .reads(LEFT, READS_25000)
}

// Step 2's screen: the next mark.
fn second() -> Gray {
    Gray::frame().draw(NEXT, &Gray::noise(0x0463_0302))
}

fn template_mark(id: &str, crop: RecordRect) -> MarkSpec {
    MarkSpec {
        search: Some(crop),
        ..MarkSpec::region_only(id.to_owned(), MarkFamily::Template, crop)
    }
}

fn tap() -> ClickSpec {
    ClickSpec {
        region: Some(TAP),
        ..ClickSpec::default()
    }
}

// `value/count >= 25000`, its left side an OCR read of `LEFT`: a value reading the page requires.
fn count_at_least_25000() -> MarkSpec {
    serde_json::from_value(json!({
        "id": "value/count",
        "family": "value_test",
        "left": {
            "region": LEFT,
            "languages": ["en"],
            "timeout_ms": 1000,
            "minimum_confidence": 0.8,
            "model_ref": "neutral-ocr",
            "model_sha256": MODEL_SHA256,
            "trim": "whitespace_v1",
            "value": {"type": "unsigned_integer", "min": 0, "max": 99999}
        },
        "op": "ge",
        "right": {"const": 25000}
    }))
    .expect("a value-test mark request")
}

// A state directory with an active session of `INSTANCE` and its Lab recording of `destination`,
// before any step, with the scripted OCR provider; `started` is record start's answer. The lock is
// dropped before the directory.
struct Bench {
    lock: RecordingLock,
    ocr: LabOcr,
    started: Value,
    temp: TempDir,
}

impl Bench {
    fn new(destination: RecordDestination) -> Self {
        let temp = TempDir::new().expect("temporary directory");
        let state = temp.path().join("state");
        fs::create_dir_all(&state).expect("state directory");
        // The session file `record start` writes before the Lab recording is created.
        fs::write(
            old_record_path(&state, INSTANCE),
            serde_json::to_vec(&json!({
                "record_id": RECORD_ID,
                "task_id": TASK_ID,
                "instance": INSTANCE,
                "status": "active",
                "started_at_unix_ms": 1_700_000_000_000_u64
            }))
            .expect("session JSON"),
        )
        .expect("session file");
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_priv3", "Lab-CLI")
            .expect("recording lock");
        let started = record_start(
            &lock,
            &RecordStartOptions {
                game: Some("neutral".to_owned()),
                server: Some("test".to_owned()),
                locale: Some("en-US".to_owned()),
                destination,
                ..RecordStartOptions::default()
            },
        )
        .expect("record start");
        let started = serde_json::to_value(&started).expect("record start JSON");
        Self {
            lock,
            ocr: LabOcr::with_provider(Arc::new(ScriptedOcr)),
            started,
            temp,
        }
    }

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    // A step with its own frame `image`, the marks `add` and the click `click`.
    fn step(&self, name: &str, image: &Gray, add: Vec<MarkSpec>, click: Option<ClickSpec>) {
        let path = self.temp.path().join(format!("{name}.png"));
        fs::write(&path, image.png()).expect("frame file");
        let request = MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            frame: Some(path.display().to_string()),
            add,
            click,
            ..MarkRequest::default()
        };
        record_mark(&self.lock, &request, false, &self.ocr)
            .unwrap_or_else(|error| panic!("{name}: {} {:?}", error.code, error.details));
    }

    // The business: step 1 on `image` with the marks `add` and the tap, step 2 the final screen.
    fn record(&self, image: &Gray, add: Vec<MarkSpec>) {
        self.step("first", image, add, Some(tap()));
        self.step(
            "second",
            &second(),
            vec![template_mark("ui/next", NEXT)],
            None,
        );
    }

    // `record stop [--destination <word>]`: the `lab` output with its status.
    fn stop(&self, destination: Option<RecordDestination>) -> Result<Value, LabError> {
        let options = RecordStopOptions {
            destination,
            ..RecordStopOptions::default()
        };
        let outcome = record_stop(&self.lock, &options, &self.ocr)?;
        let mut lab = serde_json::to_value(&outcome.lab).expect("lab JSON");
        lab["lab_status"] = json!(outcome.lab_status);
        Ok(lab)
    }

    // A stop that must answer.
    fn stopped(&self, destination: Option<RecordDestination>) -> Value {
        self.stop(destination).unwrap_or_else(|error| {
            panic!(
                "record stop: {}: {} {:?}",
                error.code, error.message, error.details
            )
        })
    }

    // The files of the package `record stop` wrote (a template makes it a ZIP).
    fn package(&self, lab: &Value) -> BTreeMap<String, Vec<u8>> {
        let path = lab["path"].as_str().expect("package path");
        let bytes = fs::read(path).expect("package file");
        expand_content_container(&bytes, ContentContainer::Zip, ContainmentLimits::default())
            .expect("package container")
    }

    // The case `id` `record stop` wrote beside the package: its `case.json` and its scene.
    fn case(&self, lab: &Value, id: &str) -> (Value, Vec<u8>) {
        let directory =
            Path::new(lab["cases"]["directory"].as_str().expect("cases directory")).join(id);
        let case = fs::read(directory.join("case.json")).expect("case.json");
        let scene = fs::read(directory.join("scene.png")).expect("scene.png");
        (serde_json::from_slice(&case).expect("case JSON"), scene)
    }

    // Every file of the cases' directory, by its name below it.
    fn case_files(&self, lab: &Value) -> BTreeMap<String, Vec<u8>> {
        let directory = Path::new(lab["cases"]["directory"].as_str().expect("cases directory"));
        let mut files = BTreeMap::new();
        for case in fs::read_dir(directory).expect("cases directory") {
            let case = case.expect("a case").path();
            for file in ["case.json", "scene.png"] {
                let name = format!(
                    "{}/{file}",
                    case.file_name().expect("a case name").to_string_lossy()
                );
                files.insert(name, fs::read(case.join(file)).expect("a case file"));
            }
        }
        files
    }

    fn recording_bytes(&self) -> Vec<u8> {
        fs::read(recording_path(&self.state(), RECORD_ID)).expect("recording file")
    }

    fn recording(&self) -> Value {
        serde_json::from_slice(&self.recording_bytes()).expect("recording")
    }

    // `record status`'s `lab`.
    fn status(&self) -> Value {
        let status = record_status(&self.state(), INSTANCE)
            .unwrap_or_else(|error| panic!("record status: {}: {}", error.code, error.message));
        serde_json::to_value(&status).expect("record status JSON")
    }
}

// The entry task of the package `entries`.
fn task(entries: &BTreeMap<String, Vec<u8>>) -> Value {
    let bytes = entries
        .get(&format!("resources/operations/{TASK_ID}/task.json"))
        .expect("task.json");
    serde_json::from_slice(bytes).expect("task JSON")
}

// The entry task of `entries` without the member the clock writes (`generated_at_unix_ms`, the
// recording's last write) and without the destination's mark.
fn task_unmarked(entries: &BTreeMap<String, Vec<u8>>) -> Value {
    let mut task = task(entries);
    if let Some(provenance) = task["provenance"].as_object_mut() {
        provenance.remove("generated_at_unix_ms");
        provenance.remove("destination");
    }
    task
}

// That the packages `one` and `other` hold the same files, byte for byte, but for the entry task,
// which differs at most by the clock's member and the destination's mark.
fn assert_same_package_but_the_mark(
    one: &BTreeMap<String, Vec<u8>>,
    other: &BTreeMap<String, Vec<u8>>,
) {
    let entry = format!("resources/operations/{TASK_ID}/task.json");
    assert_eq!(
        one.keys().collect::<Vec<_>>(),
        other.keys().collect::<Vec<_>>()
    );
    for (name, bytes) in one {
        if *name != entry {
            assert!(other.get(name) == Some(bytes), "{name} differs");
        }
    }
    assert_eq!(task_unmarked(one), task_unmarked(other));
}

// The scene of step 1's screen `image` built here pixel by pixel, apart from the Lab's builder: a
// black canvas with the frame's pixels inside the rectangles `pasted` and nothing else.
fn step_scene(image: &Gray, pasted: &[RecordRect]) -> Vec<u8> {
    let inside = |area: &RecordRect, x: u32, y: u32| {
        covers(rect(area.x, area.y, area.width, area.height), x, y)
    };
    png_of(
        &|x, y| {
            if pasted.iter().any(|area| inside(area, x, y)) {
                [image.at(x, y); 3]
            } else {
                BLACK
            }
        },
        CaptureBackendName::AdbScreencap,
    )
}

// The rectangles of a case's `scene_construction`, `(x, y, width, height)`.
fn construction(case: &Value) -> Vec<(u64, u64, u64, u64)> {
    case["scene_construction"]["rects"]
        .as_array()
        .expect("rects")
        .iter()
        .map(|rect| {
            let at = |name: &str| rect[name].as_u64().expect("a coordinate");
            (at("x"), at("y"), at("width"), at("height"))
        })
        .collect()
}

// The brief's test 1: a recording made before the slice, without the member, loads as public and
// serialises unchanged; `record start --destination local` stores the word and `record start` and
// `record status` answer it, as they answer `public`; a third word is no destination, refused by the
// closed list, as a word and in a recording.
#[test]
fn priv3_a_recording_without_the_member_is_public_and_a_local_one_is_stored_and_answered() {
    let public = Bench::new(RecordDestination::Public);
    let stored = public.recording();
    assert!(stored.get("destination").is_none(), "{stored}");
    let loaded: LabRecording =
        serde_json::from_slice(&public.recording_bytes()).expect("the recording loads");
    assert_eq!(loaded.destination, RecordDestination::Public);
    assert_eq!(
        serde_json::to_value(&loaded).expect("the recording serialises"),
        stored
    );
    assert_eq!(
        public.started["destination"], "public",
        "{}",
        public.started
    );
    assert_eq!(public.status()["destination"], "public");

    let local = Bench::new(RecordDestination::Local);
    assert_eq!(local.recording()["destination"], "local");
    let loaded: LabRecording =
        serde_json::from_slice(&local.recording_bytes()).expect("the recording loads");
    assert_eq!(loaded.destination, RecordDestination::Local);
    assert_eq!(local.started["destination"], "local", "{}", local.started);
    assert_eq!(local.status()["destination"], "local");

    for other in ["", "Local", "private", "remote", "github"] {
        assert!(RecordDestination::from_word(other).is_none(), "{other:?}");
        assert!(
            serde_json::from_value::<RecordDestination>(json!(other)).is_err(),
            "{other:?}"
        );
    }
    let mut unknown = stored;
    unknown["destination"] = json!("remote");
    assert!(serde_json::from_value::<LabRecording>(unknown).is_err());
}

// The brief's test 2: a public recording that reads no value generates, of the same recording,
// exactly what a local one does but for the local mark: every file of the package byte for byte but
// the entry task, which differs by the clock's member and the mark alone, and every case's
// `case.json` and scene byte for byte; the answer has no `withheld` and no `readings_left_out`, and
// says `public`; no `provenance.destination`. A local recording's cases are built as before this
// slice, so the public one's are today's.
#[test]
fn priv3_a_public_recording_without_a_reading_generates_what_a_local_one_does() {
    let bench = Bench::new(RecordDestination::Public);
    bench.record(&first(), vec![template_mark("ui/button", BUTTON)]);
    let public = bench.stopped(None);
    assert_eq!(public["lab_status"], "generated", "{public}");
    assert_eq!(public["destination"], "public", "{public}");
    assert!(public.get("withheld").is_none(), "{public}");
    assert!(public.get("readings_left_out").is_none(), "{public}");
    let public_package = bench.package(&public);
    assert!(
        task(&public_package)["provenance"]
            .get("destination")
            .is_none()
    );
    assert!(bench.recording().get("destination").is_none());
    let public_cases = bench.case_files(&public);

    let local = bench.stopped(Some(RecordDestination::Local));
    assert_eq!(local["lab_status"], "regenerated", "{local}");
    let local_package = bench.package(&local);
    assert_eq!(task(&local_package)["provenance"]["destination"], "local");
    assert_same_package_but_the_mark(&public_package, &local_package);
    assert_eq!(public_cases, bench.case_files(&local));
    let (case, scene) = bench.case(&public, "step_01");
    assert!(case["expect"].get("observe").is_some(), "{case}");
    assert!(scene == step_scene(&first(), &[BUTTON, NEXT]));
}

// The brief's test 3 with amendment 1's first test: a public recording whose page requires a value
// test (a fixed reading, its side's region): it generates; its side's region is withheld, the
// canvas colour in the scene and not in `scene_construction`; the case of its page leaves the value
// test out, without the page judgement (its page is not recognised on the scene without it), and
// pins the button, and the answer names it (`readings_left_out`) and the rectangle and its target
// (`withheld`); the final screen's case is judged as before; no `provenance.destination`. That a
// patch of another target over a withheld rectangle has the hole is tests 5 and 6.
#[test]
fn priv3_a_public_value_test_is_left_out_of_its_case_and_its_side_withheld() {
    let bench = Bench::new(RecordDestination::Public);
    bench.record(
        &first(),
        vec![template_mark("ui/button", BUTTON), count_at_least_25000()],
    );
    let lab = bench.stopped(None);
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    assert_eq!(
        lab["withheld"],
        json!([{"x": 50, "y": 4, "width": 40, "height": 10, "targets": ["value/count/left"]}]),
        "{lab}"
    );
    let left_out = lab["readings_left_out"]
        .as_array()
        .expect("readings_left_out");
    assert_eq!(left_out.len(), 1, "{lab}");
    assert_eq!(left_out[0]["case"], "step_01");
    assert_eq!(left_out[0]["targets"], json!(["value/count"]));
    assert_eq!(left_out[0]["page_judged"], false);
    assert_eq!(lab["cases"]["ids"], json!(["step_01", "final"]), "{lab}");

    let (case, scene) = bench.case(&lab, "step_01");
    assert_eq!(
        case["expect"],
        json!({"recognize": [{"target": "ui/button", "passed": true}]}),
        "{case}"
    );
    assert_eq!(construction(&case), [(8, 8, 12, 12), (8, 40, 12, 12)]);
    assert!(
        scene == step_scene(&first(), &[BUTTON, NEXT]),
        "the quantity's pixels are in the scene"
    );
    let (last, _) = bench.case(&lab, "final");
    assert!(last["expect"].get("observe").is_some(), "{last}");
    let entries = bench.package(&lab);
    assert!(task(&entries)["provenance"].get("destination").is_none());
    assert_eq!(
        task(&entries)["page_rules"]["step_01"]["required"],
        json!(["ui/button", "value/count"])
    );
}

// The brief's test 4 with amendment 1's second: the same recording made local: its cases are
// today's (the value test expected with the page judgement, the side's region pasted: the scene as
// the contract's table builds it, byte for byte), its answer names nothing withheld or left out, and
// its package is the public one's but for `provenance.destination: "local"` in the entry task, and
// no other file says `destination`.
#[test]
fn priv3_a_local_recording_is_todays_with_the_one_provenance_mark() {
    let bench = Bench::new(RecordDestination::Local);
    bench.record(
        &first(),
        vec![template_mark("ui/button", BUTTON), count_at_least_25000()],
    );
    let local = bench.stopped(None);
    assert_eq!(local["lab_status"], "generated", "{local}");
    assert_eq!(local["destination"], "local", "{local}");
    assert!(local.get("withheld").is_none(), "{local}");
    assert!(local.get("readings_left_out").is_none(), "{local}");
    let (case, scene) = bench.case(&local, "step_01");
    assert!(case["expect"].get("observe").is_some(), "{case}");
    assert_eq!(
        case["expect"]["recognize"],
        json!([
            {"target": "ui/button", "passed": true},
            {"target": "value/count", "passed": true}
        ])
    );
    assert!(
        scene == step_scene(&first(), &[BUTTON, NEXT, LEFT]),
        "the local scene is not today's"
    );
    let local_package = bench.package(&local);
    let marked = task(&local_package);
    assert_eq!(marked["provenance"]["destination"], "local");
    let mut unmarked = marked.clone();
    if let Some(provenance) = unmarked["provenance"].as_object_mut() {
        provenance.remove("destination");
    }
    assert!(
        !serde_json::to_string(&unmarked)
            .expect("task text")
            .contains("\"destination\"")
    );
    let entry = format!("resources/operations/{TASK_ID}/task.json");
    for (name, bytes) in &local_package {
        if *name != entry {
            assert!(
                !String::from_utf8_lossy(bytes).contains("\"destination\""),
                "{name} says destination"
            );
        }
    }

    let public = bench.stopped(Some(RecordDestination::Public));
    assert_eq!(public["lab_status"], "regenerated", "{public}");
    assert_same_package_but_the_mark(&local_package, &bench.package(&public));
}

// The brief's test 7, as amendment 1 makes it: a public recording whose page requires a colour
// probe drawn over the quantity a value test reads (a box over a reading, a target that needs no
// reading itself and keeps no picture: fix round 1 refuses a template's crop there before any case,
// test 10): its case fails on that probe, which reads the canvas colour inside the withheld
// rectangle, `record_case_reading_withheld` naming the probe, the rectangle and (fix round 1) the
// reading, nothing written; the same recording stopped with `--destination local`, on the
// recording still open, generates.
#[test]
fn priv3_a_box_drawn_over_a_reading_refuses_and_passes_once_the_recording_is_local() {
    let bench = Bench::new(RecordDestination::Public);
    bench.record(
        &badged(),
        vec![
            template_mark("ui/button", BUTTON),
            MarkSpec::region_only("ui/badge".to_owned(), MarkFamily::Color, BADGE),
            count_at_least_25000(),
        ],
    );
    let before = bench.recording_bytes();
    let refusal = bench.stop(None).expect_err("the public stop refuses");
    assert_eq!(refusal.code, "record_case_reading_withheld", "{refusal:?}");
    let details = refusal.details.clone().expect("details");
    assert_eq!(details["case"], "step_01", "{details}");
    assert_eq!(
        details["withheld"],
        json!([{"target": "ui/badge",
                "withheld": [{"x": 50, "y": 4, "width": 40, "height": 10}],
                "readings": ["value/count/left"]}]),
        "{details}"
    );
    assert_eq!(bench.recording_bytes(), before, "nothing is written");

    let lab = bench.stopped(Some(RecordDestination::Local));
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    assert_eq!(lab["destination"], "local", "{lab}");
    assert_eq!(bench.recording()["destination"], "local");
}

// The brief's test 8: on a stopped public recording, `record stop --destination local` changes the
// stored destination and generates again (the mark appears, the side's region is pasted again);
// the same word, or none, generates nothing again and writes nothing; `--destination public` again
// takes the mark off and withholds the region again.
#[test]
fn priv3_another_destination_at_stop_generates_again_and_the_same_does_nothing() {
    let bench = Bench::new(RecordDestination::Public);
    bench.record(
        &first(),
        vec![template_mark("ui/button", BUTTON), count_at_least_25000()],
    );
    let public = bench.stopped(None);
    assert_eq!(public["lab_status"], "generated", "{public}");
    let path = public["path"].as_str().expect("package path").to_owned();
    let package = fs::read(&path).expect("package file");
    let recording = bench.recording_bytes();

    let same = bench.stopped(Some(RecordDestination::Public));
    assert_eq!(same["lab_status"], "already_generated", "{same}");
    assert_eq!(same["destination"], "public", "{same}");
    assert_eq!(fs::read(&path).expect("package file"), package);
    assert_eq!(bench.recording_bytes(), recording);

    let local = bench.stopped(Some(RecordDestination::Local));
    assert_eq!(local["lab_status"], "regenerated", "{local}");
    assert_eq!(local["destination"], "local", "{local}");
    assert!(local.get("withheld").is_none(), "{local}");
    assert_eq!(
        task(&bench.package(&local))["provenance"]["destination"],
        "local"
    );
    assert_eq!(bench.recording()["destination"], "local");
    let (_, scene) = bench.case(&local, "step_01");
    assert!(scene == step_scene(&first(), &[BUTTON, NEXT, LEFT]));

    let package = fs::read(&path).expect("package file");
    let recording = bench.recording_bytes();
    let none = bench.stopped(None);
    assert_eq!(none["lab_status"], "already_generated", "{none}");
    assert_eq!(fs::read(&path).expect("package file"), package);
    assert_eq!(bench.recording_bytes(), recording);

    let again = bench.stopped(Some(RecordDestination::Public));
    assert_eq!(again["lab_status"], "regenerated", "{again}");
    assert!(again.get("withheld").is_some(), "{again}");
    assert!(
        task(&bench.package(&again))["provenance"]
            .get("destination")
            .is_none()
    );
    assert!(bench.recording().get("destination").is_none());
    let (_, scene) = bench.case(&again, "step_01");
    assert!(scene == step_scene(&first(), &[BUTTON, NEXT]));
}

// Fix round 1 (the coordinator's ruling: a template's stored picture holds no pixel of a reading),
// test 10: a public recording whose step 1 holds a template whose crop meets the quantity a value
// test reads, where its screen is the canvas colour already, so that its case would pass on the
// scene all the same: record stop refuses `record_case_reading_withheld` before any case, naming the
// template, its crop (`kept`), the withheld rectangle and the reading, and writes nothing; the same
// recording made local generates as today: the case with the page judgement and every target the
// page requires, the template's crop and the quantity's region pasted.
#[test]
fn priv3_fix1_a_template_kept_over_a_reading_refuses_whether_or_not_its_case_passes() {
    let marks = || {
        vec![
            template_mark("ui/button", BUTTON),
            template_mark("ui/corner", CORNER),
            count_at_least_25000(),
        ]
    };
    let screen = cornered();
    // Where the crop meets the quantity's region the screen is the canvas colour, so a public
    // scene leaves the template's pixels as they are.
    for y in 8..14 {
        for x in 80..90 {
            assert_eq!(screen.at(x, y), 0, "({x}, {y})");
        }
    }
    let public = Bench::new(RecordDestination::Public);
    public.record(&screen, marks());
    let before = public.recording_bytes();
    let refusal = public.stop(None).expect_err("the public stop refuses");
    assert_eq!(refusal.code, "record_case_reading_withheld", "{refusal:?}");
    let details = refusal.details.clone().expect("details");
    assert!(details.get("case").is_none(), "{details}");
    assert_eq!(details["target"], "ui/corner", "{details}");
    assert_eq!(
        details["withheld"],
        json!([{"target": "ui/corner",
                "kept": {"x": 80, "y": 8, "width": 12, "height": 12},
                "withheld": [{"x": 50, "y": 4, "width": 40, "height": 10}],
                "readings": ["value/count/left"]}]),
        "{details}"
    );
    assert_eq!(public.recording_bytes(), before, "nothing is written");

    let local = Bench::new(RecordDestination::Local);
    local.record(&screen, marks());
    let lab = local.stopped(None);
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    assert!(lab.get("withheld").is_none(), "{lab}");
    let (case, scene) = local.case(&lab, "step_01");
    assert!(case["expect"].get("observe").is_some(), "{case}");
    assert_eq!(
        case["expect"]["recognize"],
        json!([
            {"target": "ui/button", "passed": true},
            {"target": "ui/corner", "passed": true},
            {"target": "value/count", "passed": true}
        ]),
        "{case}"
    );
    assert!(
        scene == step_scene(&screen, &[BUTTON, CORNER, NEXT, LEFT]),
        "the local scene is not today's"
    );
}

// Fix round 1, test 11 (the review's remark): the unbounded reading driven to its code through the
// self-check `cases`, which record stop runs last. No recording makes a reading nothing confines:
// every reading a recorded package declares is a value test's side, a fixed rectangle. So the
// package the self-checks before `cases` build of a public recording with a value test
// (`package_without_cases`, as record stop builds it) has that side's region made the keyword
// `full_frame` in its entry task, and `cases` is run on it as record stop runs it (its first failure
// the refusal): `record_case_reading_unbounded` naming the side, before any case is made.
#[test]
fn priv3_fix1_an_unbounded_reading_refuses_the_self_check_cases_with_its_code() {
    let bench = Bench::new(RecordDestination::Public);
    bench.record(
        &first(),
        vec![template_mark("ui/button", BUTTON), count_at_least_25000()],
    );
    let recording: LabRecording =
        serde_json::from_slice(&bench.recording_bytes()).expect("the recording");
    let settings = StopSettings {
        game: Some("neutral".to_owned()),
        server: Some("test".to_owned()),
        locale: Some("en-US".to_owned()),
        package_id: None,
        requires: None,
        timeout_ms: None,
        arrival_timeout_ms: DEFAULT_ARRIVAL_TIMEOUT_MS,
        application_arrival_timeout_ms: DEFAULT_APPLICATION_ARRIVAL_TIMEOUT_MS,
    };
    let plan = generate_plan(&recording, &settings)
        .unwrap_or_else(|error| panic!("the plan: {}: {}", error.code, error.message));
    let bytes = package_without_cases(&plan, &bench.ocr).expect("the package");
    let mut entries =
        expand_content_container(&bytes, ContentContainer::Zip, ContainmentLimits::default())
            .expect("the package's files");
    let name = format!("resources/operations/{TASK_ID}/task.json");
    let mut task: Value = serde_json::from_slice(&entries[&name]).expect("task JSON");
    let side = task["ocr_targets"]
        .as_array_mut()
        .expect("ocr_targets")
        .iter_mut()
        .find(|target| target["id"] == "value/count/left")
        .expect("the value test's side");
    side["region"] = json!({"mode": "full_frame"});
    entries.insert(name, serde_json::to_vec_pretty(&task).expect("task bytes"));
    let refusal = regression_cases(
        &plan,
        &entries,
        &FrameCache::new(),
        &bench.ocr,
        &mut Failures::first(),
    )
    .expect_err("the self-check cases refuses");
    assert_eq!(refusal.code, "record_case_reading_unbounded", "{refusal:?}");
    assert_eq!(
        refusal.details.as_ref().map(|details| &details["target"]),
        Some(&json!("value/count/left")),
        "{refusal:?}"
    );
}

// The row `name` of the operation table this tree holds.
fn table_row(name: &str) -> Value {
    let table: Value = serde_json::from_str(include_str!("../../../contracts/lab-operations.json"))
        .expect("the operation table");
    table["operations"]
        .as_array()
        .expect("operations")
        .iter()
        .find(|operation| operation["name"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("the row {name}"))
}

// The brief's test 9: the operation table's rows `record start` and `record stop` declare the
// option `destination`, not required, of the type `enum:local|public`, whose words are the Lab's
// (`RecordDestination::WORDS`), each read back to itself and no other word read. The MCP tools are
// generated from these rows: an enumeration's words become its property's `enum`
// (`apps/actinglab-mcp/src/tools.rs`, `value_schema`), which `gate_434_lab5g2a` holds for every
// parameter of every offered row.
#[test]
fn priv3_the_operation_table_carries_destination_with_its_closed_list() {
    for name in ["record start", "record stop"] {
        let row = table_row(name);
        let destination = row["params"]
            .as_array()
            .expect("params")
            .iter()
            .find(|param| param["name"] == "destination")
            .unwrap_or_else(|| panic!("{name} declares destination"));
        assert_eq!(destination["form"], "option", "{destination}");
        assert_eq!(destination["required"], false, "{destination}");
        assert_eq!(destination["type"], "enum:local|public", "{destination}");
        let words = destination["type"]
            .as_str()
            .and_then(|kind| kind.strip_prefix("enum:"))
            .expect("an enumeration")
            .split('|')
            .collect::<Vec<_>>();
        assert_eq!(words, RecordDestination::WORDS);
    }
    for word in RecordDestination::WORDS {
        assert_eq!(
            RecordDestination::from_word(word).map(RecordDestination::word),
            Some(word)
        );
        assert_eq!(
            serde_json::to_value(RecordDestination::from_word(word)).expect("the word as JSON"),
            json!(word)
        );
    }
}
