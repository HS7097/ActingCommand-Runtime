// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #396 REC-8 logic tests of shared layout reads (REC-M v1.2.1 §10, §12): a
// `fixed_slots` layout's one shared detection read gives every slot target inside it its
// blocks, so sixteen slots cost one provider call; a slot without blocks is Unknown; the
// projection counts the read as one provider evaluation; a truncated read fails; a slot target
// outside the read reads on its own; and a shared read no target uses is refused. A scripted
// OCR provider answers on a blank neutral frame.

use crate::*;
use actingcommand_contract::CandidateFeature;
use serde_json::{Value, json};
use std::sync::Mutex;

const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const FRAME_WIDTH: u32 = 640;
const FRAME_HEIGHT: u32 = 360;
const LAYOUT: &str = "layout/roster";
// The shared read covers four rows of four slots.
const SHARED: PackRect = PackRect {
    x: 40,
    y: 40,
    width: 560,
    height: 280,
};
const SHARED_DETECT: OcrProviderBlocks = OcrProviderBlocks::Detect {
    line_merge_gap_factor: Some(0),
};

type Respond = dyn Fn(PackRect, OcrProviderBlocks) -> OcrProviderResult + Send + Sync;

// Answers every OCR request from its region and block mode and records each request.
struct ScriptedOcr {
    respond: Box<Respond>,
    requests: Mutex<Vec<(PackRect, OcrProviderBlocks)>>,
}

impl fmt::Debug for ScriptedOcr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ScriptedOcr")
    }
}

impl ScriptedOcr {
    fn new(
        respond: impl Fn(PackRect, OcrProviderBlocks) -> OcrProviderResult + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            respond: Box::new(respond),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<(PackRect, OcrProviderBlocks)> {
        self.requests.lock().expect("request log").clone()
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
        self.requests
            .lock()
            .expect("request log")
            .push((request.region, request.blocks));
        Ok((self.respond)(request.region, request.blocks))
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

fn rect(x: i32, y: i32, width: i32, height: i32) -> PackRect {
    PackRect {
        x,
        y,
        width,
        height,
    }
}

fn rect_json(rect: PackRect) -> Value {
    json!({"x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height})
}

// Slot `index` of the four by four grid inside the shared read.
fn slot_rect(index: usize) -> PackRect {
    let (column, row) = ((index % 4) as i32, (index / 4) as i32);
    rect(40 + 140 * column, 40 + 70 * row, 120, 50)
}

// The name region is the slot's upper half, the level region its lower half.
fn name_region(slot: PackRect) -> PackRect {
    rect(slot.x, slot.y, slot.width, 25)
}

fn level_region(slot: PackRect) -> PackRect {
    rect(slot.x, slot.y + 25, slot.width, 25)
}

// A detected block whose center lies in the slot's name region.
fn name_block(slot: PackRect, index: usize) -> OcrProviderTextBlock {
    OcrProviderTextBlock {
        text: format!("NAME {index:02}"),
        rect: rect(slot.x + 10, slot.y + 5, 80, 15),
        confidence: Some(0.9),
    }
}

// A detected block whose center lies in the slot's level region.
fn level_block(slot: PackRect, level: usize) -> OcrProviderTextBlock {
    OcrProviderTextBlock {
        text: level.to_string(),
        rect: rect(slot.x + 10, slot.y + 30, 40, 15),
        confidence: Some(0.9),
    }
}

fn result(
    blocks: Vec<OcrProviderTextBlock>,
    detection: Option<OcrProviderDetection>,
) -> OcrProviderResult {
    OcrProviderResult {
        ppocr_diagnostics: Vec::new(),
        text: blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        blocks,
        confidence: Some(0.9),
        detection,
    }
}

fn ocr(id: &str, region: PackRect) -> Value {
    json!({
        "type": "ocr", "id": id, "region": rect_json(region), "languages": ["en"],
        "timeout_ms": 1000, "match_mode": "exact", "expected": ["unused"],
        "case_sensitive": false, "minimum_confidence": 0.5, "model_ref": "PP-OCRv6_medium",
        "model_sha256": MODEL_SHA256
    })
}

fn shared_ocr(region: PackRect) -> Value {
    json!({
        "region": rect_json(region), "languages": ["en"], "timeout_ms": 5000,
        "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256,
        "blocks": "detect", "line_merge": {"gap_factor": 0}
    })
}

// The identity feature over the sixteen names `NAME 00` to `NAME 15`.
fn member_feature() -> Value {
    let entries = (0..16)
        .map(|index| json!({"id": format!("member-{index:02}"), "aliases": [format!("NAME {index:02}")]}))
        .collect::<Vec<_>>();
    json!({
        "name": "member", "value": "identity",
        "identity": {"entries": entries, "recognition": {
            "kind": "ocr_aliases", "max_distance": 0, "minimum_margin": 1,
            "minimum_confidence_milli": 800
        }}
    })
}

fn level_feature() -> Value {
    json!({
        "name": "level", "value": "ocr_integer",
        "integer": {"min": 0, "max": 99, "minimum_confidence_milli": 800}
    })
}

// A roster pack: one name target per slot rectangle, and with `levels` one level target each;
// the layout declares `shared` as its `shared_ocr` when given.
fn roster_pack(slots: &[PackRect], levels: bool, shared: Option<Value>) -> Value {
    let mut targets = Vec::new();
    let mut slot_values = Vec::new();
    for (index, slot) in slots.iter().enumerate() {
        let name = format!("slot/name_{index:02}");
        targets.push(ocr(&name, name_region(*slot)));
        let mut slot_targets = json!({"member": name});
        if levels {
            let level = format!("slot/level_{index:02}");
            targets.push(ocr(&level, level_region(*slot)));
            slot_targets["level"] = json!(level);
        }
        slot_values.push(json!({
            "rect": rect_json(*slot), "click": rect_json(*slot), "targets": slot_targets
        }));
    }
    let mut features = vec![member_feature()];
    if levels {
        features.push(level_feature());
    }
    let mut layout = json!({
        "id": LAYOUT, "page_id": "neutral/roster", "kind": "fixed_slots",
        "features": features, "slots": slot_values
    });
    if let Some(shared) = shared {
        layout["shared_ocr"] = shared;
    }
    json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": FRAME_WIDTH, "height": FRAME_HEIGHT},
        "targets": targets,
        "candidate_layouts": [layout]
    })
}

fn evaluator(
    pack: Value,
    provider: Arc<ScriptedOcr>,
) -> RecognitionPackResult<RecognitionEvaluator> {
    let pack = load_pack_from_json_str(&pack.to_string())?;
    RecognitionEvaluator::with_vision_provider(
        pack,
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        provider,
    )
}

fn scene() -> Scene {
    Scene::from_rgb8(
        FRAME_WIDTH,
        FRAME_HEIGHT,
        &vec![0; (FRAME_WIDTH * FRAME_HEIGHT * 3) as usize],
    )
    .expect("blank scene")
}

fn sixteen_slots() -> Vec<PackRect> {
    (0..16).map(slot_rect).collect()
}

fn member(
    projection: &actingcommand_contract::CandidateProjection,
    index: usize,
) -> &CandidateFeature {
    &projection.candidates()[index].features["member"]
}

#[test]
fn sixteen_slots_take_their_names_from_one_shared_read() {
    let slots = sixteen_slots();
    let blocks = slots
        .iter()
        .enumerate()
        .map(|(index, slot)| name_block(*slot, index))
        .collect::<Vec<_>>();
    let provider = ScriptedOcr::new(move |_, _| result(blocks.clone(), None));
    let evaluator = evaluator(
        roster_pack(&slots, false, Some(shared_ocr(SHARED))),
        provider.clone(),
    )
    .expect("a roster with a shared read is admitted");
    let scene = scene();

    let projection = evaluator
        .scene_context(&scene)
        .project_candidates(LAYOUT)
        .expect("the roster projects");

    assert_eq!(provider.requests(), [(SHARED, SHARED_DETECT)]);
    assert_eq!(projection.candidates().len(), 16);
    for index in 0..16 {
        assert!(
            matches!(member(&projection, index), CandidateFeature::Identity { value, .. }
                if *value == format!("member-{index:02}")),
            "slot {index} reads its own name: {:?}",
            member(&projection, index)
        );
    }
}

#[test]
fn a_slot_without_blocks_in_its_region_is_unknown() {
    let slots = sixteen_slots();
    let blocks = slots
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != 5)
        .map(|(index, slot)| name_block(*slot, index))
        .collect::<Vec<_>>();
    let provider = ScriptedOcr::new(move |_, _| result(blocks.clone(), None));
    let evaluator = evaluator(
        roster_pack(&slots, false, Some(shared_ocr(SHARED))),
        provider.clone(),
    )
    .expect("a roster with a shared read is admitted");
    let scene = scene();

    let projection = evaluator
        .scene_context(&scene)
        .project_candidates(LAYOUT)
        .expect("the roster projects");

    assert_eq!(provider.requests().len(), 1);
    assert!(
        matches!(member(&projection, 5), CandidateFeature::Unknown { .. }),
        "slot 5 has no text: {:?}",
        member(&projection, 5)
    );
    assert!(matches!(
        member(&projection, 4),
        CandidateFeature::Identity { value, .. } if value == "member-04"
    ));
}

#[test]
fn a_projection_served_by_the_shared_read_counts_one_provider_evaluation() {
    // Sixteen slots with a name and a level each are 32 OCR evaluations on their own.
    let slots = sixteen_slots();
    let provider = ScriptedOcr::new(|_, _| result(Vec::new(), None));
    let refused = evaluator(roster_pack(&slots, true, None), provider)
        .expect_err("32 OCR evaluations exceed the projection budget");
    assert!(
        refused.message().contains(
            "/candidate_layouts/0/slots: 32 OCR and NN evaluations exceed the 16-evaluation projection budget"
        ),
        "{}",
        refused.message()
    );

    let blocks = slots
        .iter()
        .enumerate()
        .flat_map(|(index, slot)| [name_block(*slot, index), level_block(*slot, index + 10)])
        .collect::<Vec<_>>();
    let provider = ScriptedOcr::new(move |_, _| result(blocks.clone(), None));
    let evaluator = evaluator(
        roster_pack(&slots, true, Some(shared_ocr(SHARED))),
        provider.clone(),
    )
    .expect("one shared read fits the projection budget");
    let layout = evaluator.candidate_layout(LAYOUT).expect("the layout");
    assert_eq!(
        layout.maximum_provider_and_wait_ms(&evaluator),
        5000,
        "one provider call: the shared read's timeout"
    );
    let scene = scene();

    let projection = evaluator
        .scene_context(&scene)
        .project_candidates(LAYOUT)
        .expect("the roster projects");

    assert_eq!(provider.requests().len(), 1);
    assert!(matches!(
        projection.candidates()[15].features["level"],
        CandidateFeature::Integer { value: 25, .. }
    ));
}

#[test]
fn a_truncated_shared_read_fails_with_text_read_blocks_truncated() {
    let slots = sixteen_slots();
    let blocks = slots
        .iter()
        .enumerate()
        .map(|(index, slot)| name_block(*slot, index))
        .collect::<Vec<_>>();
    let provider = ScriptedOcr::new(move |_, _| {
        result(
            blocks.clone(),
            Some(OcrProviderDetection {
                detected: 65,
                limit: 64,
            }),
        )
    });
    let evaluator = evaluator(
        roster_pack(&slots, false, Some(shared_ocr(SHARED))),
        provider.clone(),
    )
    .expect("a roster with a shared read is admitted");
    let scene = scene();

    let failure = evaluator
        .scene_context(&scene)
        .project_candidates(LAYOUT)
        .expect_err("a slot may lack its blocks");

    assert_eq!(failure.code().as_str(), "candidate_feature_failed");
    let outcome = failure
        .cause()
        .and_then(RecognitionPackError::outcome)
        .expect("the read's outcome");
    assert!(outcome.is(RecognitionPackCode::TextReadBlocksTruncated));
    assert_eq!(outcome.value("stage"), Some(&json!("text_read_shared")));
    assert_eq!(outcome.value("count"), Some(&json!(65)));
    assert_eq!(outcome.value("limit_count"), Some(&json!(64)));
    assert_eq!(provider.requests().len(), 1);
}

#[test]
fn a_slot_target_outside_the_shared_read_reads_on_its_own() {
    // Slot 1 lies below the shared read.
    let slots = [slot_rect(0), rect(40, 325, 120, 30)];
    let inside = name_block(slots[0], 0);
    let provider = ScriptedOcr::new(move |region, blocks| match blocks {
        OcrProviderBlocks::Implicit => result(
            vec![OcrProviderTextBlock {
                text: "NAME 01".to_owned(),
                rect: region,
                confidence: Some(0.9),
            }],
            None,
        ),
        _ => result(vec![inside.clone()], None),
    });
    let evaluator = evaluator(
        roster_pack(&slots, false, Some(shared_ocr(SHARED))),
        provider.clone(),
    )
    .expect("a roster with a shared read is admitted");
    let scene = scene();

    let projection = evaluator
        .scene_context(&scene)
        .project_candidates(LAYOUT)
        .expect("the roster projects");

    assert_eq!(
        provider.requests(),
        [
            (SHARED, SHARED_DETECT),
            (name_region(slots[1]), OcrProviderBlocks::Implicit)
        ]
    );
    assert!(matches!(
        member(&projection, 0),
        CandidateFeature::Identity { value, .. } if value == "member-00"
    ));
    assert!(matches!(
        member(&projection, 1),
        CandidateFeature::Identity { value, .. } if value == "member-01"
    ));
}

#[test]
fn a_shared_read_no_slot_target_uses_is_refused() {
    let provider = ScriptedOcr::new(|_, _| result(Vec::new(), None));

    let refused = evaluator(
        roster_pack(
            &sixteen_slots(),
            false,
            Some(shared_ocr(rect(0, 0, 30, 30))),
        ),
        provider.clone(),
    )
    .expect_err("a shared read no target uses");

    assert!(
        refused.message().contains(
            "/candidate_layouts/0/shared_ocr: no slot or feature target reads its text from the shared read"
        ),
        "{}",
        refused.message()
    );
    assert!(provider.requests().is_empty());
}
