// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 SEL-GRID G2 (register §1.491; contracts/selection-graph.md, "The lines of a name
// box"; the coordinator's work order rd-1119/brief-463-g2-name-lines.md, its tests 1 and 7 through
// the Runtime's vision boundary): a layout's shared read that declares the lines of its name boxes
// sends them, with the name boxes, through `VisionFfiProvider`, the adapter the Runtime and the
// Lab read text through, to the engine, in the pixels of the picture the engine is given; the
// engine plans each served box by the vision boundary's own planner (`ocr_name_line_plan`, the one
// the PP-OCR engine calls) on that picture and answers a cut box with its strips; the kernel joins
// them into the name.
//
// - A name of two lines 3 px apart found as one block is unread without the member (the block is
//   read as one line, poorly), read with it, and is the name read when its lines are found apart
//   (test 1).
// - On a frame twice the package's base, brought to the base as a Lab frame is
//   (`lab_scene_on_base`), the engine is given the base's picture and the declared line height of
//   21 base pixels, never the device's 42; the cut falls between the lines and the name reads
//   (test 7).
//
// The frames are flat gray with each line of a name drawn as dark vertical stripes every third
// column; the engine's detector finds the boxes a test gives it, and its recognizer reads a region
// as a recognizer of one line reads it: one drawn line whole at 0.95, parts of two or more run
// together at 0.3. No model, no device, no clock.

use crate::{VisionFfiProvider, VisionModelIdentity, lab_scene_on_base};
use actingcommand_contract::CandidateFeature;
use actingcommand_contract::candidate_projection::CandidateUnknownReason;
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::{
    FsAssetResolver, RecognitionEvaluator, load_pack_from_json_str,
};
use actingcommand_vision_ffi::{
    OcrDetectionCount, OcrEngine, OcrInferenceRequest, OcrInferenceResult, OcrNameLinePlan,
    OcrNameLines, OcrTextBlock, OcrTextStrip, VisionBackendKind, VisionFfiResult, VisionRect,
    ocr_box_is_served, ocr_name_line_plan,
};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const MODEL_REF: &str = "PP-OCRv6_medium";
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const LAYOUT: &str = "layout/slots";
/// The package's base, and the device frame of test 7, twice its size.
const BASE: (u32, u32) = (200, 120);
const DEVICE: (u32, u32) = (400, 240);
const BACKGROUND: u8 = 200;
const INK: u8 = 40;

/// Slot 0's name of two lines and slot 1's of one, in the base's pixels.
const ALPHA: VisionRect = VisionRect {
    x: 20,
    y: 20,
    width: 60,
    height: 21,
};
const BRAVO: VisionRect = VisionRect {
    x: 20,
    y: 44,
    width: 60,
    height: 21,
};
const MERGED: VisionRect = VisionRect {
    x: 20,
    y: 20,
    width: 60,
    height: 45,
};
const CHARLIE: VisionRect = VisionRect {
    x: 120,
    y: 30,
    width: 60,
    height: 22,
};

/// What the engine was asked.
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    frame: (u32, u32),
    served: Vec<VisionRect>,
    lines: Option<OcrNameLines>,
}

type Log = Arc<Mutex<Vec<Seen>>>;

/// The engine of the module comment: its detector finds `boxes`, its recognizer reads `lines`.
struct Engine {
    lines: Vec<(&'static str, VisionRect)>,
    boxes: Vec<VisionRect>,
    seen: Log,
}

impl Engine {
    /// What the recognizer of one line reads in `region`: the drawn lines that overlap it.
    fn recognize(&self, region: VisionRect) -> Option<(String, f32)> {
        let overlaps = |rect: VisionRect| {
            rect.x < region.x + region.width
                && region.x < rect.x + rect.width
                && rect.y < region.y + region.height
                && region.y < rect.y + rect.height
        };
        let held = self
            .lines
            .iter()
            .filter(|(_, rect)| overlaps(*rect))
            .collect::<Vec<_>>();
        match held.as_slice() {
            [] => None,
            [(text, rect)]
                if rect.y >= region.y && rect.y + rect.height <= region.y + region.height =>
            {
                Some(((*text).to_owned(), 0.95))
            }
            more => Some((more.iter().map(|(text, _)| *text).collect(), 0.3)),
        }
    }
}

impl OcrEngine for Engine {
    fn read_text(&mut self, request: OcrInferenceRequest) -> VisionFfiResult<OcrInferenceResult> {
        self.seen.lock().expect("request log").push(Seen {
            frame: (request.frame.width, request.frame.height),
            served: request.served.clone(),
            lines: request.lines,
        });
        let frame = request.frame.view();
        let kept = self
            .boxes
            .iter()
            .copied()
            .filter(|rect| request.served.is_empty() || ocr_box_is_served(&request.served, *rect))
            .collect::<Vec<_>>();
        let mut blocks = Vec::new();
        let mut below_floor = Vec::new();
        let mut recognitions = 0_u32;
        for rect in &kept {
            let plan = request.lines.map_or(OcrNameLinePlan::Whole, |lines| {
                ocr_name_line_plan(&frame, *rect, lines)
            });
            recognitions += plan.recognitions();
            match plan {
                OcrNameLinePlan::BelowFloor => below_floor.push(*rect),
                OcrNameLinePlan::Whole => {
                    if let Some((text, confidence)) = self.recognize(*rect) {
                        blocks.push(OcrTextBlock {
                            text,
                            rect: *rect,
                            confidence: Some(confidence),
                            strips: Vec::new(),
                        });
                    }
                }
                OcrNameLinePlan::Cut(cut) => {
                    let strips = cut
                        .iter()
                        .filter_map(|strip| {
                            self.recognize(*strip)
                                .map(|(text, confidence)| OcrTextStrip {
                                    text,
                                    rect: *strip,
                                    confidence: Some(confidence),
                                })
                        })
                        .collect::<Vec<_>>();
                    if !strips.is_empty() {
                        blocks.push(OcrTextBlock {
                            text: strips
                                .iter()
                                .map(|strip| strip.text.as_str())
                                .collect::<Vec<_>>()
                                .join("\n"),
                            rect: *rect,
                            confidence: strips
                                .iter()
                                .filter_map(|strip| strip.confidence)
                                .reduce(f32::min),
                            strips,
                        });
                    }
                }
            }
        }
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
            detection: Some(OcrDetectionCount {
                detected: count(self.boxes.len()),
                limit: 64,
                served: (!request.served.is_empty()).then_some(recognitions),
            }),
            below_floor,
        })
    }
}

fn count(boxes: usize) -> u32 {
    u32::try_from(boxes).expect("a count of boxes")
}

/// The Runtime's vision boundary over the engine of `lines` and `boxes`, and the engine's log.
fn boundary(
    lines: Vec<(&'static str, VisionRect)>,
    boxes: Vec<VisionRect>,
) -> (Arc<VisionFfiProvider>, Log) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider = VisionFfiProvider::new(
        Some((
            Box::new(Engine {
                lines,
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

fn rect_json(x: i32, y: i32, width: i32, height: i32) -> Value {
    json!({"x": x, "y": y, "width": width, "height": height})
}

/// A package of base `BASE` whose `fixed_slots` layout reads each of two slots' name boxes,
/// (10, 10, 80, 60) and (110, 10, 80, 60), through one shared read over the frame, its `lines`
/// when given.
fn slots_pack(lines: Option<Value>) -> Value {
    let target = |id: &str, x: i32| {
        json!({
            "type": "ocr", "id": id, "region": rect_json(x, 10, 80, 60), "languages": ["en"],
            "timeout_ms": 1000, "match_mode": "exact", "expected": ["unused"],
            "case_sensitive": false, "minimum_confidence": 0.5, "model_ref": MODEL_REF,
            "model_sha256": MODEL_SHA256
        })
    };
    let mut shared = json!({
        "region": rect_json(0, 0, 200, 120), "languages": ["en"], "timeout_ms": 5000,
        "model_ref": MODEL_REF, "model_sha256": MODEL_SHA256,
        "blocks": "detect", "line_merge": {"gap_factor": 0}
    });
    if let Some(lines) = lines {
        shared["lines"] = lines;
    }
    let slot = |x: i32, id: &str| {
        json!({"rect": rect_json(x, 5, 90, 110), "click": rect_json(x, 5, 90, 110),
               "targets": {"member": id}})
    };
    json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": BASE.0, "height": BASE.1},
        "targets": [target("slot/name_00", 10), target("slot/name_01", 110)],
        "candidate_layouts": [{
            "id": LAYOUT, "page_id": "neutral/slots", "kind": "fixed_slots",
            "features": [{
                "name": "member", "value": "identity",
                "identity": {"entries": [
                    {"id": "alpha_bravo", "aliases": ["ALPHA BRAVO"]},
                    {"id": "charlie", "aliases": ["CHARLIE"]}
                ], "recognition": {
                    "kind": "ocr_aliases", "max_distance": 0, "minimum_margin": 1,
                    "minimum_confidence_milli": 800
                }}
            }],
            "slots": [slot(5, "slot/name_00"), slot(105, "slot/name_01")],
            "shared_ocr": shared
        }]
    })
}

fn two_lines() -> Value {
    json!({"max": 2, "line_height": 21})
}

fn evaluator(pack: Value, provider: Arc<VisionFfiProvider>) -> RecognitionEvaluator {
    RecognitionEvaluator::with_vision_provider(
        load_pack_from_json_str(&pack.to_string()).expect("the package's recognition pack"),
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        provider,
    )
    .expect("the evaluator")
}

/// A frame of `size` whose names are drawn `scale` times the base's: each line of `lines` as dark
/// vertical stripes every third column.
fn drawn((width, height): (u32, u32), scale: i32, lines: &[(&str, VisionRect)]) -> Scene {
    let mut pixels = vec![BACKGROUND; (width * height * 3) as usize];
    for (_, rect) in lines {
        for y in rect.y * scale..(rect.y + rect.height) * scale {
            for x in (rect.x * scale..(rect.x + rect.width) * scale).step_by(3) {
                let at = ((y as u32 * width + x as u32) * 3) as usize;
                pixels[at..at + 3].copy_from_slice(&[INK; 3]);
            }
        }
    }
    Scene::from_rgb8(width, height, &pixels).expect("the frame")
}

/// Each slot's `member` on `scene`: its identity, or the reason it is Unknown.
fn members(
    evaluator: &RecognitionEvaluator,
    scene: &Scene,
) -> Vec<Result<String, CandidateUnknownReason>> {
    evaluator
        .scene_context(scene)
        .project_candidates(LAYOUT)
        .expect("the slots project")
        .candidates()
        .iter()
        .map(|candidate| match candidate.features.get("member") {
            Some(CandidateFeature::Identity { value, .. }) => Ok(value.clone()),
            Some(CandidateFeature::Unknown { reason, .. }) => Err(*reason),
            other => panic!("{} has {other:?}", candidate.id),
        })
        .collect()
}

fn names() -> Vec<(&'static str, VisionRect)> {
    vec![("ALPHA", ALPHA), ("BRAVO", BRAVO), ("CHARLIE", CHARLIE)]
}

// Test 1 through the boundary: slot 0's two lines found as one block (20, 20, 60, 45). Without the
// member the engine is sent no lines and reads the block as one line, poorly: slot 0 is Unknown,
// low_confidence. With {max 2, line_height 21} the engine is sent the lines as declared, cuts the
// block by the boundary's planner between the lines and reads each strip: slot 0 is alpha_bravo.
// Found apart, the same lines read without the member give slot 0 the same identity.
#[test]
fn g2_two_lines_through_the_boundary_are_unread_without_lines_and_read_with_them() {
    let scene = drawn(BASE, 1, &names());
    let merged = vec![MERGED, CHARLIE];
    // The slots' name boxes, as the read sends them: the targets' regions, in the base's pixels.
    let boxes = vec![
        VisionRect {
            x: 10,
            y: 10,
            width: 80,
            height: 60,
        },
        VisionRect {
            x: 110,
            y: 10,
            width: 80,
            height: 60,
        },
    ];

    let (provider, seen) = boundary(names(), merged.clone());
    let today = members(&evaluator(slots_pack(None), provider), &scene);
    assert_eq!(
        today,
        [
            Err(CandidateUnknownReason::LowConfidence),
            Ok("charlie".to_owned())
        ]
    );
    assert_eq!(
        requests(&seen),
        [Seen {
            frame: BASE,
            served: boxes.clone(),
            lines: None,
        }]
    );

    let (provider, seen) = boundary(names(), merged);
    let cut = members(&evaluator(slots_pack(Some(two_lines())), provider), &scene);
    assert_eq!(
        cut,
        [Ok("alpha_bravo".to_owned()), Ok("charlie".to_owned())]
    );
    assert_eq!(
        requests(&seen),
        [Seen {
            frame: BASE,
            served: boxes,
            lines: Some(OcrNameLines {
                max: 2,
                line_height: 21,
            }),
        }]
    );

    let (provider, _) = boundary(names(), vec![ALPHA, BRAVO, CHARLIE]);
    let apart = members(&evaluator(slots_pack(None), provider), &scene);
    assert_eq!(apart, cut);
}

// Test 7: a device frame twice the base, its names drawn twice as large (slot 0's lines 42 px high,
// 6 px apart), brought to the base as the Lab brings its frames. The engine is given the base's
// picture (200 x 120) and the lines as declared, 21 base pixels, not the device's 42: on that
// picture the block of 45 px is cut between the lines and slot 0 reads its name.
#[test]
fn g2_on_a_reduced_frame_the_line_height_is_in_the_picture_the_engine_is_given() {
    let device = drawn(DEVICE, 2, &names());
    let reduced = lab_scene_on_base(BASE, &device)
        .expect("a pure scale of the base")
        .expect("a frame of another size than the base")
        .scene;
    assert_eq!((reduced.width(), reduced.height()), BASE, "setup");
    let (provider, seen) = boundary(names(), vec![MERGED, CHARLIE]);

    let read = members(
        &evaluator(slots_pack(Some(two_lines())), provider),
        &reduced,
    );

    assert_eq!(
        read,
        [Ok("alpha_bravo".to_owned()), Ok("charlie".to_owned())]
    );
    let asked = requests(&seen);
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].frame, BASE);
    assert_eq!(
        asked[0].lines,
        Some(OcrNameLines {
            max: 2,
            line_height: 21,
        })
    );
}
