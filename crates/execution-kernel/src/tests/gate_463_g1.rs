// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 SEL-GRID G1 (register §1.490; contracts/selection-graph.md, "Shared OCR read",
// "What the read recognizes"; the coordinator's work order
// rd-1119/brief-463-g1-shared-read-serves-name-boxes.md, its tests 6 and 7): a layout's shared read
// sends the name boxes it serves through the Runtime's vision boundary (`VisionFfiProvider`, the
// adapter the Runtime and the Lab read text through) to the engine, in the picture the engine is
// given; the engine keeps the boxes whose centre lies in one by the provider's rule
// (`ocr_box_is_served`, the rule the PP-OCR engine applies, written once in the contract crate),
// and the kernel gives each kept block to the target whose region holds its centre.
//
// - On a frame twice the package's base, brought to the base as a Lab frame is
//   (`lab_scene_on_base`), the engine is given the base's picture and the name boxes in the base's
//   coordinates; with a chain it is given the processed region alone and the boxes moved onto its
//   origin; both slots read their names either way (test 7).
// - Of boxes centred on each edge and corner of a slot's name region, or one pixel past it, the
//   engine keeps exactly the boxes the kernel then gives to that slot's target, and those are the
//   boxes whose centre the region holds, its left and top edges inside and its right and bottom
//   edges outside (test 6).
//
// The frames are flat gray and the packages synthetic; the engine's detector finds the boxes a
// test gives it. No model, no device, no clock.

use crate::{VisionFfiProvider, VisionModelIdentity, lab_scene_on_base};
use actingcommand_contract::CandidateFeature;
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::{
    FsAssetResolver, PackRect, RecognitionEvaluator, load_pack_from_json_str,
};
use actingcommand_vision_ffi::{
    OcrDetectionCount, OcrEngine, OcrInferenceRequest, OcrInferenceResult, OcrTextBlock,
    VisionBackendKind, VisionFfiResult, VisionRect, ocr_box_is_served,
};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const MODEL_REF: &str = "PP-OCRv6_medium";
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const LAYOUT: &str = "layout/slots";
/// The package's base, and the device frame of test 7, twice its size.
const BASE: (u32, u32) = (160, 90);
const DEVICE: (u32, u32) = (320, 180);

/// What the engine was asked, and the boxes it kept.
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    frame: (u32, u32),
    region: VisionRect,
    served: Vec<VisionRect>,
    kept: Vec<VisionRect>,
}

/// The engine's record of its requests, shared with the test.
type Log = Arc<Mutex<Vec<Seen>>>;

/// An engine whose detector finds `boxes`, texts at rectangles of the picture it is given. It
/// keeps, when the request names served name boxes, the boxes that lie in one by the provider's
/// rule, reads each kept box as its text, reports the detected and served counts and records
/// every request.
struct Engine {
    boxes: Vec<(String, VisionRect)>,
    seen: Log,
}

impl OcrEngine for Engine {
    fn read_text(&mut self, request: OcrInferenceRequest) -> VisionFfiResult<OcrInferenceResult> {
        let kept = self
            .boxes
            .iter()
            .filter(|(_, rect)| {
                request.served.is_empty() || ocr_box_is_served(&request.served, *rect)
            })
            .cloned()
            .collect::<Vec<_>>();
        self.seen.lock().expect("request log").push(Seen {
            frame: (request.frame.width, request.frame.height),
            region: request.region,
            served: request.served.clone(),
            kept: kept.iter().map(|(_, rect)| *rect).collect(),
        });
        let detection = OcrDetectionCount {
            detected: count(self.boxes.len()),
            limit: 64,
            served: (!request.served.is_empty()).then(|| count(kept.len())),
        };
        let blocks = kept
            .into_iter()
            .map(|(text, rect)| OcrTextBlock {
                text,
                rect,
                confidence: Some(0.9),
                strips: Vec::new(),
            })
            .collect::<Vec<_>>();
        Ok(OcrInferenceResult {
            text: blocks
                .iter()
                .map(|block| block.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            blocks,
            confidence: Some(0.9),
            backend: VisionBackendKind::FastDeployPpocr,
            warnings: Vec::new(),
            ppocr_diagnostics: Vec::new(),
            detection: Some(detection),
            below_floor: Vec::new(),
        })
    }
}

fn count(boxes: usize) -> u32 {
    u32::try_from(boxes).expect("a count of boxes")
}

/// The Runtime's vision boundary over the engine of `boxes`, and the engine's request log.
fn boundary(boxes: Vec<(String, VisionRect)>) -> (Arc<VisionFfiProvider>, Log) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider = VisionFfiProvider::new(
        Some((
            Box::new(Engine {
                boxes,
                seen: Arc::clone(&seen),
            }),
            VisionModelIdentity::new(MODEL_REF, MODEL_SHA256).expect("the model's identity"),
        )),
        None,
    )
    .expect("the vision boundary");
    (Arc::new(provider), seen)
}

fn requests(seen: &Mutex<Vec<Seen>>) -> Vec<Seen> {
    seen.lock().expect("request log").clone()
}

fn rect(x: i32, y: i32, width: i32, height: i32) -> PackRect {
    PackRect {
        x,
        y,
        width,
        height,
    }
}

fn vision(rect: PackRect) -> VisionRect {
    VisionRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

fn pack_rect(rect: VisionRect) -> PackRect {
    PackRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

fn rect_json(rect: PackRect) -> Value {
    json!({"x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height})
}

/// A white-text chain a shared read may declare (pixel by pixel).
fn chain() -> Value {
    json!({"steps": [
        {"kind": "keep_colors", "colors": [[255, 255, 255]], "tolerance": 40},
        {"kind": "invert"}
    ]})
}

/// A package of base `BASE` whose `fixed_slots` layout reads one OCR target per slot, `(id, slot
/// rectangle, name region)` each, through a shared read over `shared`, with `chain` on the read
/// and its targets when given; `feature`, named `member`, reads each slot's target.
fn slots_pack(
    shared: PackRect,
    slots: &[(&str, PackRect, PackRect)],
    feature: Value,
    chain: Option<Value>,
) -> Value {
    let targets = slots
        .iter()
        .map(|(id, _, region)| {
            let mut target = json!({
                "type": "ocr", "id": id, "region": rect_json(*region), "languages": ["en"],
                "timeout_ms": 1000, "match_mode": "exact", "expected": ["unused"],
                "case_sensitive": false, "minimum_confidence": 0.5, "model_ref": MODEL_REF,
                "model_sha256": MODEL_SHA256
            });
            if let Some(chain) = &chain {
                target["preprocess"] = chain.clone();
            }
            target
        })
        .collect::<Vec<_>>();
    let mut shared_ocr = json!({
        "region": rect_json(shared), "languages": ["en"], "timeout_ms": 5000,
        "model_ref": MODEL_REF, "model_sha256": MODEL_SHA256,
        "blocks": "detect", "line_merge": {"gap_factor": 0}
    });
    if let Some(chain) = &chain {
        shared_ocr["preprocess"] = chain.clone();
    }
    let slot_values = slots
        .iter()
        .map(|(id, slot, _)| {
            json!({"rect": rect_json(*slot), "click": rect_json(*slot), "targets": {"member": id}})
        })
        .collect::<Vec<_>>();
    json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": BASE.0, "height": BASE.1},
        "targets": targets,
        "candidate_layouts": [{
            "id": LAYOUT, "page_id": "neutral/slots", "kind": "fixed_slots",
            "features": [feature], "slots": slot_values, "shared_ocr": shared_ocr
        }]
    })
}

fn evaluator(pack: Value, provider: Arc<VisionFfiProvider>) -> RecognitionEvaluator {
    RecognitionEvaluator::with_vision_provider(
        load_pack_from_json_str(&pack.to_string()).expect("the package's recognition pack"),
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        provider,
    )
    .expect("the evaluator")
}

/// A flat gray frame of `size`.
fn flat((width, height): (u32, u32)) -> Scene {
    Scene::from_rgb8(width, height, &vec![60; (width * height * 3) as usize]).expect("the frame")
}

/// Whether `rect` lies inside a picture of `size`.
fn inside(rect: VisionRect, (width, height): (u32, u32)) -> bool {
    rect.x >= 0
        && rect.y >= 0
        && i64::from(rect.x) + i64::from(rect.width) <= i64::from(width)
        && i64::from(rect.y) + i64::from(rect.height) <= i64::from(height)
}

// Test 7's two slots and their shared read, in the base's coordinates.
const SHARED: PackRect = PackRect {
    x: 8,
    y: 8,
    width: 144,
    height: 40,
};
const NAME_0: PackRect = PackRect {
    x: 12,
    y: 12,
    width: 56,
    height: 12,
};
const NAME_1: PackRect = PackRect {
    x: 92,
    y: 12,
    width: 56,
    height: 12,
};

fn member() -> Value {
    json!({
        "name": "member", "value": "identity",
        "identity": {"entries": [
            {"id": "alpha", "aliases": ["ALPHA"]},
            {"id": "bravo", "aliases": ["BRAVO"]}
        ], "recognition": {
            "kind": "ocr_aliases", "max_distance": 0, "minimum_margin": 1,
            "minimum_confidence_milli": 800
        }}
    })
}

/// Test 7's texts on the base: a name in each slot's name region and a price below the first,
/// moved by `-origin` into the picture the engine is given.
fn names_and_price(origin: (i32, i32)) -> Vec<(String, VisionRect)> {
    let at = |x: i32, y: i32| vision(rect(x - origin.0, y - origin.1, 30, 8));
    vec![
        ("ALPHA".to_owned(), at(20, 14)),
        ("BRAVO".to_owned(), at(100, 14)),
        ("PRICE".to_owned(), at(20, 30)),
    ]
}

// Test 7: a device frame twice the base, brought to the base as the Lab brings its frames; the
// engine is given the base's picture, never the device's, and the two name regions in the base's
// coordinates inside it; with a chain, the processed region alone and the regions moved onto its
// origin. Either way it keeps the two names and not the price, and each slot reads its name.
#[test]
fn g1_t7_on_a_reduced_frame_the_name_boxes_are_in_the_picture_the_engine_is_given() {
    let device = flat(DEVICE);
    let reduced = lab_scene_on_base(BASE, &device)
        .expect("a pure scale of the base")
        .expect("a frame of another size than the base")
        .scene;
    assert_eq!((reduced.width(), reduced.height()), BASE, "setup");
    let slots = [
        ("slot/name_00", rect(10, 10, 60, 30), NAME_0),
        ("slot/name_01", rect(90, 10, 60, 30), NAME_1),
    ];

    for (chained, origin, picture) in [
        (false, (0, 0), BASE),
        (true, (SHARED.x, SHARED.y), (144, 40)),
    ] {
        let (provider, seen) = boundary(names_and_price(origin));
        let evaluator = evaluator(
            slots_pack(SHARED, &slots, member(), chained.then(chain)),
            provider,
        );

        let projection = evaluator
            .scene_context(&reduced)
            .project_candidates(LAYOUT)
            .unwrap_or_else(|error| panic!("chained {chained}: {error:?}"));

        let moved = |rect: PackRect| {
            vision(PackRect {
                x: rect.x - origin.0,
                y: rect.y - origin.1,
                ..rect
            })
        };
        let names = names_and_price(origin);
        assert_eq!(
            requests(&seen),
            [Seen {
                frame: picture,
                region: moved(SHARED),
                served: vec![moved(NAME_0), moved(NAME_1)],
                kept: vec![names[0].1, names[1].1],
            }],
            "chained {chained}"
        );
        for served in &requests(&seen)[0].served {
            assert!(inside(*served, picture), "chained {chained}: {served:?}");
        }
        let members = projection
            .candidates()
            .iter()
            .map(|candidate| match candidate.features.get("member") {
                Some(CandidateFeature::Identity { value, .. }) => value.clone(),
                other => panic!("chained {chained}: {} has {other:?}", candidate.id),
            })
            .collect::<Vec<_>>();
        assert_eq!(members, ["alpha", "bravo"], "chained {chained}");
    }
}

// Test 6's name region, x 100..140 and y 50..70.
const EDGE_REGION: PackRect = PackRect {
    x: 100,
    y: 50,
    width: 40,
    height: 20,
};

/// Boxes whose centres `(x + floor(w/2), y + floor(h/2))` lie on each edge and corner of
/// `EDGE_REGION`, or one pixel past it, of even and odd sizes, each with whether the region holds
/// its centre: its left and top edges inside, its right and bottom edges outside.
fn edge_boxes() -> Vec<(String, VisionRect, bool)> {
    [
        // The top-left corner (100, 50), with an even and an odd box.
        ("TL", rect(98, 48, 4, 4), true),
        ("TL-ODD", rect(97, 47, 7, 7), true),
        // The top-right corner (140, 50); its last column (139, 50).
        ("TR", rect(138, 48, 4, 4), false),
        ("TR-IN", rect(137, 48, 5, 4), true),
        // The bottom-left corner (100, 70); its last row (100, 69).
        ("BL", rect(98, 68, 4, 4), false),
        ("BL-IN", rect(98, 67, 4, 5), true),
        // The bottom-right corner (140, 70); the last pixel (139, 69).
        ("BR", rect(138, 68, 4, 4), false),
        ("BR-IN", rect(137, 67, 5, 5), true),
        // The left edge (100, 60); one pixel left of it (99, 60).
        ("L", rect(98, 58, 4, 4), true),
        ("L-OUT", rect(97, 58, 4, 4), false),
        // The top edge (120, 50); one pixel above it (120, 49).
        ("T", rect(118, 48, 4, 4), true),
        ("T-OUT", rect(118, 47, 4, 4), false),
        // The right edge (140, 60); one pixel left of it (139, 60).
        ("R", rect(138, 58, 4, 4), false),
        ("R-IN", rect(137, 58, 5, 4), true),
        // The bottom edge (120, 70); one pixel above it (120, 69).
        ("B", rect(118, 68, 4, 4), false),
        ("B-IN", rect(118, 67, 4, 5), true),
    ]
    .into_iter()
    .map(|(text, at, holds)| (text.to_owned(), vision(at), holds))
    .collect()
}

fn sorted(mut rects: Vec<PackRect>) -> Vec<PackRect> {
    rects.sort_by_key(|rect| (rect.y, rect.x, rect.width, rect.height));
    rects
}

// Test 6: the engine keeps exactly the boxes the kernel gives to the slot's target, and they are
// the boxes whose centre the region holds; the others are dropped before they are recognized.
#[test]
fn g1_t6_the_engine_keeps_a_box_exactly_when_the_kernel_gives_it_to_the_target() {
    let edges = edge_boxes();
    let (provider, seen) = boundary(
        edges
            .iter()
            .map(|(text, at, _)| (text.clone(), *at))
            .collect(),
    );
    let evaluator = evaluator(
        slots_pack(
            rect(8, 8, 144, 74),
            &[("slot/name", rect(96, 46, 50, 30), EDGE_REGION)],
            json!({"name": "member", "value": "passed"}),
            None,
        ),
        provider,
    );
    let scene = flat(BASE);

    let mut given = Vec::new();
    evaluator
        .scene_context(&scene)
        .project_candidates_with_samples(LAYOUT, &[&scene], &mut |target, result, _, _| {
            if target == "slot/name"
                && let Ok(evaluation) = result
            {
                given = evaluation
                    .ocr
                    .as_ref()
                    .expect("the target's OCR evidence")
                    .blocks
                    .iter()
                    .map(|block| block.rect)
                    .collect();
            }
            Ok(())
        })
        .expect("the slot projects");

    let asked = requests(&seen);
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].served, [vision(EDGE_REGION)]);
    let kept = asked[0]
        .kept
        .iter()
        .copied()
        .map(pack_rect)
        .collect::<Vec<_>>();
    let holds = edges
        .iter()
        .filter(|(_, _, holds)| *holds)
        .map(|(_, at, _)| pack_rect(*at))
        .collect::<Vec<_>>();
    assert_eq!(holds.len(), 9, "setup");
    assert_eq!(sorted(kept.clone()), sorted(given));
    assert_eq!(sorted(kept), sorted(holds));
}
