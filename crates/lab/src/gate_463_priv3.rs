// SPDX-License-Identifier: AGPL-3.0-only

// #463 PRIV-3 (contracts/lab-recording.md, "A recording's destination" and "Regression cases
// written with the package"; register §1.483; the owner's ruling of 11-18 that quantities an
// account holds, shown on a screen, are private and their pixels are never stored in a
// repository): a public recording's scenes hold no pixel of a value reading. Recognition packs as
// their JSON reads, on a 160x90 base; frames are synthetic; no Runtime, no device, no OCR model;
// the tests read no clock and never sleep.
// - `priv3_a_selects_quantity_column_is_withheld_and_a_shared_read_that_serves_it_as_a_whole` (5).
// - `priv3_a_banded_row_is_withheld_by_its_band_an_unbanded_one_refuses_and_a_reach_is_filled`
//   (6).

use crate::recording::cases::{Patch, scene_regions};
use crate::recording::frames::decode_frame;
use crate::recording::model::{RecordDestination, RecordSize, RecordWithheld};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_recognition_pack::{PackReadingDeclarations, PackRect, RecognitionPack};
use serde_json::{Value, json};
use std::collections::BTreeMap;

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
