// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 A58, #428 CFL-2 logic tests (design CF v2 §8 CFL-2, tests 1 to 9 and 11 to 13;
// test 10, the ledger rows, is the runtime host's): recognition preprocessing wired into the OCR
// reads. A read without a chain hands the provider the frame itself; a chained read hands over
// only its processed region, explicit in its block mode, and its blocks come back in frame
// coordinates; the chain is part of a read's identity and budget; a kept observation and a
// remembered read reuse their evidence without running the chain; a shared read serves only its
// own chain and counts each target over its own region; preprocessing that spends the budget calls
// no provider. A recording provider answers on a synthetic frame: a grey background with a grey
// step window the template anchor matches, a striped white-and-red region and a few white
// squares. Clocks are fake and read fixed offsets from one epoch; nothing sleeps.

use crate::ocr_preprocess::{chain_runs, test_epoch};
use crate::*;
use actingcommand_contract::ResourceDeclarationReason;
use serde_json::json;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const MODEL_REF: &str = "PP-OCRv6_medium";
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const WIDTH: u32 = 64;
const HEIGHT: u32 = 24;
const TONES: &str = "assets/tones.png";
const GROUND: [u8; 3] = [150, 150, 150];
const WHITE: [u8; 3] = [255, 255, 255];
const RED: [u8; 3] = [200, 40, 40];
/// Where the anchor's grey step window lies.
const ANCHOR: (i32, i32) = (4, 4);
/// The region most targets read: white and red rows, 36 to the right of and 4 below the anchor.
const ROI: PackRect = PackRect {
    x: 40,
    y: 8,
    width: 16,
    height: 8,
};
/// A layout's shared read over the right half, and its three slot targets' regions.
const SHARED: PackRect = PackRect {
    x: 32,
    y: 0,
    width: 32,
    height: 24,
};
const SLOT_A: PackRect = PackRect {
    x: 34,
    y: 2,
    width: 12,
    height: 8,
};
const SLOT_B: PackRect = PackRect {
    x: 50,
    y: 2,
    width: 12,
    height: 8,
};
const SLOT_C: PackRect = PackRect {
    x: 34,
    y: 14,
    width: 12,
    height: 8,
};

/// What the provider was handed for one read.
#[derive(Debug, Clone)]
struct Seen {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    region: PackRect,
    blocks: OcrProviderBlocks,
    timeout_ms: u64,
}

type Respond = dyn Fn(&Seen) -> OcrProviderResult + Send + Sync;

/// Records every OCR request it is handed and answers it from what it saw.
struct RecordingOcr {
    respond: Box<Respond>,
    seen: Mutex<Vec<Seen>>,
}

impl fmt::Debug for RecordingOcr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RecordingOcr")
    }
}

impl RecordingOcr {
    fn new(respond: impl Fn(&Seen) -> OcrProviderResult + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            respond: Box::new(respond),
            seen: Mutex::new(Vec::new()),
        })
    }

    /// Answers one block `"7"` at 2 right of and 1 below the region's corner, with `confidence`.
    fn seven(confidence: f32) -> Arc<Self> {
        Self::new(move |seen| seven(seen, confidence))
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().expect("request log").clone()
    }

    fn record(&self, request: &OcrProviderRequest<'_>) -> OcrProviderResult {
        let seen = Seen {
            width: request.frame.width,
            height: request.frame.height,
            pixels: request.frame.rgb8_pixels.to_vec(),
            region: request.region,
            blocks: request.blocks,
            timeout_ms: request.timeout_ms,
        };
        let result = (self.respond)(&seen);
        self.seen.lock().expect("request log").push(seen);
        result
    }
}

impl VisionProvider for RecordingOcr {
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
        Ok(self.record(&request))
    }

    fn read_text_with_execution_evidence(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderObservation, VisionProviderError> {
        let result = self.record(&request);
        Ok(OcrProviderObservation {
            result,
            execution: Some(execution(request.model_ref, request.model_sha256)),
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

fn execution(model_ref: &str, model_sha256: &str) -> OcrProviderExecutionEvidence {
    OcrProviderExecutionEvidence {
        invocation_id: "invocation-1".to_owned(),
        session_id: "session-1".to_owned(),
        session_generation: 1,
        requested_provider: OcrExecutionProviderKind::Cpu,
        resolved_provider: OcrExecutionProviderKind::Cpu,
        requested_cuda_ordinal: None,
        requested_cuda_identity: None,
        resolved_cuda_ordinal: None,
        resolved_cuda_identity: None,
        provider_implementation: "fixture".to_owned(),
        provider_binary_sha256: "b".repeat(64),
        runtime_version: "fixture".to_owned(),
        model_ref: model_ref.to_owned(),
        model_sha256: model_sha256.to_owned(),
        cpu_ep_registered: true,
        cpu_fallback_disabled: false,
        fallback_forbidden: true,
        fallback_observed: None,
        complete: true,
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

fn result(blocks: Vec<OcrProviderTextBlock>, confidence: f32) -> OcrProviderResult {
    OcrProviderResult {
        ppocr_diagnostics: Vec::new(),
        text: blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        blocks,
        confidence: Some(confidence),
        detection: None,
    }
}

/// One block `"7"` at 2 right of and 1 below the corner of the region the provider was handed.
fn seven(seen: &Seen, confidence: f32) -> OcrProviderResult {
    result(
        vec![OcrProviderTextBlock {
            text: "7".to_owned(),
            rect: rect(seen.region.x + 2, seen.region.y + 1, 10, 5),
            confidence: Some(confidence),
        }],
        confidence,
    )
}

/// The frame (module doc).
fn scene() -> Scene {
    let mut pixels = GROUND.repeat((WIDTH * HEIGHT) as usize);
    let mut fill = |x: i32, y: i32, width: i32, height: i32, color: [u8; 3]| {
        for row in y..y + height {
            for column in x..x + width {
                let start = ((row * WIDTH as i32 + column) * 3) as usize;
                pixels[start..start + 3].copy_from_slice(&color);
            }
        }
    };
    fill(ANCHOR.0, ANCHOR.1, 4, 6, [100; 3]);
    fill(ANCHOR.0 + 4, ANCHOR.1, 4, 6, [200; 3]);
    for row in 0..ROI.height {
        let color = if row % 2 == 0 { WHITE } else { RED };
        fill(ROI.x, ROI.y + row, ROI.width, 1, color);
    }
    fill(SLOT_A.x + 2, SLOT_A.y + 2, 4, 4, WHITE);
    fill(SLOT_B.x + 1, SLOT_B.y + 1, 6, 3, WHITE);
    fill(SLOT_C.x, SLOT_C.y, 3, 8, RED);
    Scene::from_rgb8(WIDTH, HEIGHT, &pixels).expect("scene")
}

/// The template the anchor searches for: an 8x6 grey step, 100 on the left and 200 on the right.
fn tones() -> Vec<u8> {
    let mut pixels = Vec::new();
    for _ in 0..6 {
        for column in 0..8 {
            pixels.extend_from_slice(&[if column < 4 { 100 } else { 200 }; 3]);
        }
    }
    encode_png(8, 6, &pixels)
}

#[derive(Debug)]
struct Assets;

impl AssetResolver for Assets {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
        if path == TONES {
            Ok(tones())
        } else {
            Err(RecognitionPackError::fatal(format!("no asset {path}")))
        }
    }
}

/// The white-text chain of the author examples: keep near-white, then invert.
fn white_chain() -> Value {
    json!({"steps": [
        {"kind": "keep_colors", "colors": [[255, 255, 255], [240, 240, 232]], "tolerance": 40},
        {"kind": "invert"}
    ]})
}

/// The same colors in the other order: another chain.
fn reordered_chain() -> Value {
    json!({"steps": [
        {"kind": "keep_colors", "colors": [[240, 240, 232], [255, 255, 255]], "tolerance": 40},
        {"kind": "invert"}
    ]})
}

fn chain(value: &Value) -> OcrPreprocess {
    OcrPreprocess::from_json(value).expect("a legal chain")
}

fn region_json(region: PackRect) -> Value {
    json!({"x": region.x, "y": region.y, "width": region.width, "height": region.height})
}

/// An OCR target over `region` (a pack region), with `preprocess` when given.
fn ocr(id: &str, region: Value, preprocess: Option<Value>) -> Value {
    let mut target = json!({
        "type": "ocr", "id": id, "region": region, "languages": ["en"], "timeout_ms": 1000,
        "match_mode": "exact", "expected": ["7"], "case_sensitive": false,
        "minimum_confidence": 0.9, "model_ref": MODEL_REF, "model_sha256": MODEL_SHA256
    });
    if let Some(preprocess) = preprocess {
        target["preprocess"] = preprocess;
    }
    target
}

/// A locate target over `region` reading `blocks` (`"region"` or `"detect"`).
fn located(id: &str, region: Value, blocks: &str, preprocess: Option<Value>) -> Value {
    let mut target = ocr(id, region, preprocess);
    target["locate"] = if blocks == "detect" {
        json!({"normalize": "identity_v1", "blocks": "detect", "line_merge": {"gap_factor": 0}})
    } else {
        json!({"normalize": "identity_v1", "blocks": "region"})
    };
    target
}

fn anchor() -> Value {
    json!({
        "type": "template", "id": "icon/anchor", "template_path": TONES, "region": "full_frame",
        "threshold": 0.95
    })
}

fn relative(width: i32, height: i32) -> Value {
    json!({
        "mode": "template_relative", "anchor_target_id": "icon/anchor",
        "offset": {"x": ROI.x - ANCHOR.0, "y": ROI.y - ANCHOR.1}, "width": width, "height": height
    })
}

fn pack(targets: Vec<Value>, layouts: Vec<Value>) -> Value {
    let mut pack = json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed"},
        "targets": targets
    });
    if !layouts.is_empty() {
        pack["candidate_layouts"] = Value::Array(layouts);
    }
    pack
}

/// A `fixed_slots` layout of three slots over the shared read, reading `slot/a`, `slot/b` and
/// `slot/c`, with the shared read's chain `preprocess` when given.
fn slots_layout(preprocess: Option<Value>) -> Value {
    let slot = |rect: PackRect, target: &str| {
        let whole = PackRect {
            x: rect.x - 2,
            y: rect.y - 2,
            width: 16,
            height: 12,
        };
        json!({"rect": region_json(whole), "click": region_json(whole), "targets": {"count": target}})
    };
    let mut shared = json!({
        "region": region_json(SHARED), "languages": ["en"], "timeout_ms": 5000,
        "model_ref": MODEL_REF, "model_sha256": MODEL_SHA256,
        "blocks": "detect", "line_merge": {"gap_factor": 0}
    });
    if let Some(preprocess) = preprocess {
        shared["preprocess"] = preprocess;
    }
    json!({
        "id": "layout/cells", "page_id": "neutral/cells", "kind": "fixed_slots",
        "features": [{"name": "count", "value": "ocr_integer",
                      "integer": {"min": 0, "max": 999, "minimum_confidence_milli": 800}}],
        "slots": [slot(SLOT_A, "slot/a"), slot(SLOT_B, "slot/b"), slot(SLOT_C, "slot/c")],
        "shared_ocr": shared
    })
}

fn build(pack: Value, provider: Arc<RecordingOcr>) -> RecognitionEvaluator {
    RecognitionEvaluator::with_vision_provider(
        load_pack_from_json_str(&pack.to_string()).expect("the pack loads"),
        Arc::new(Assets),
        provider,
    )
    .expect("the evaluator is built")
}

/// The pack's refusal: its pointer and reason.
fn refusal(pack: Value) -> (String, ResourceDeclarationReason) {
    let error = load_pack_from_json_str(&pack.to_string()).expect_err("the pack is refused");
    let issue = error
        .declaration_issue()
        .unwrap_or_else(|| panic!("a declaration refusal: {error}"));
    (issue.field_path.clone(), issue.reason)
}

/// What CFL-1 gives for `region` of `scene` under `preprocess`, as R = G = B pixels, with its
/// evidence.
fn processed(
    scene: &Scene,
    region: PackRect,
    preprocess: &OcrPreprocess,
) -> (Vec<u8>, actingcommand_recognition::PreprocessEvidence) {
    let output = scene
        .preprocess_region(region.into(), preprocess.chain())
        .expect("CFL-1 processes the region");
    let evidence = output.evidence;
    let pixels = output
        .image
        .into_raw()
        .into_iter()
        .flat_map(|value| [value, value, value])
        .collect();
    (pixels, evidence)
}

fn ocr_evidence(evaluation: &TargetEvaluation) -> &OcrEvaluation {
    evaluation.ocr.as_deref().expect("OCR evidence")
}

/// The frame itself, handed as before: its size, bytes, the declared region and block mode.
fn assert_frame_itself(seen: &Seen, scene: &Scene, region: PackRect, blocks: OcrProviderBlocks) {
    assert_eq!((seen.width, seen.height), (WIDTH, HEIGHT));
    assert!(seen.pixels == scene.rgb8_pixels(), "the frame's own bytes");
    assert_eq!(seen.region, region);
    assert_eq!(seen.blocks, blocks);
}

#[test]
fn cfl2_t1_without_a_chain_every_read_hands_the_provider_the_frame_itself() {
    let scene = scene();
    let detect = OcrProviderBlocks::Detect {
        line_merge_gap_factor: Some(0),
    };
    let runs = chain_runs();

    // An ordinary target (`evaluate_ocr`, both of its calls), its observation, and a locate
    // target (`read_text_once`).
    let provider = RecordingOcr::seven(0.95);
    let evaluator = build(
        pack(
            vec![
                ocr("text/plain", region_json(ROI), None),
                located("text/located", region_json(ROI), "region", None),
            ],
            Vec::new(),
        ),
        provider.clone(),
    );
    let plain = evaluator
        .evaluate_target(&scene, "text/plain")
        .expect("evaluated");
    let kept = HashSet::from(["text/plain".to_owned()]);
    let gate = evaluator
        .scene_context(&scene)
        .with_remembered_targets(&kept);
    gate.evaluate_target("text/plain").expect("evaluated");
    let observation = evaluator
        .evaluate_ocr_observation(&scene, "text/plain")
        .expect("observed");
    let located = evaluator
        .evaluate_target(&scene, "text/located")
        .expect("evaluated");
    let seen = provider.seen();
    assert_eq!(seen.len(), 4);
    for read in &seen[..3] {
        assert_frame_itself(read, &scene, ROI, OcrProviderBlocks::Implicit);
    }
    assert_frame_itself(&seen[3], &scene, ROI, OcrProviderBlocks::Region);
    assert_eq!(ocr_evidence(&plain).preprocess, None);
    assert_eq!(ocr_evidence(&located).preprocess, None);
    assert_eq!(observation.preprocess, None);
    let written = serde_json::to_value(&plain).expect("serialized");
    assert!(written["ocr"].get("preprocess").is_none());
    assert!(written["ocr"].get("shared_read").is_none());

    // A layout's shared read (`read_shared_text`).
    let provider = RecordingOcr::new(|seen| seven(seen, 0.95));
    let evaluator = build(
        pack(
            vec![
                ocr("slot/a", region_json(SLOT_A), None),
                ocr("slot/b", region_json(SLOT_B), None),
                ocr("slot/c", region_json(SLOT_C), None),
            ],
            vec![slots_layout(None)],
        ),
        provider.clone(),
    );
    evaluator
        .scene_context(&scene)
        .project_candidates("layout/cells")
        .expect("the layout projects");
    let seen = provider.seen();
    assert_eq!(seen.len(), 1);
    assert_frame_itself(&seen[0], &scene, SHARED, detect);
    assert_eq!(chain_runs(), runs, "no chain ran");
}

#[test]
fn cfl2_t2_a_chained_read_hands_over_its_processed_region_and_moves_the_blocks_back() {
    let scene = scene();
    let white = chain(&white_chain());
    let (expected, _) = processed(&scene, ROI, &white);
    let provider = RecordingOcr::seven(0.95);
    let evaluator = build(
        pack(
            vec![
                anchor(),
                ocr("text/rect", region_json(ROI), Some(white_chain())),
                ocr(
                    "text/relative",
                    relative(ROI.width, ROI.height),
                    Some(white_chain()),
                ),
                located(
                    "text/region",
                    region_json(ROI),
                    "region",
                    Some(white_chain()),
                ),
                located(
                    "text/detect",
                    region_json(ROI),
                    "detect",
                    Some(white_chain()),
                ),
            ],
            Vec::new(),
        ),
        provider.clone(),
    );
    let block = rect(ROI.x + 2, ROI.y + 1, 10, 5);
    for (id, blocks) in [
        ("text/rect", OcrProviderBlocks::Region),
        ("text/relative", OcrProviderBlocks::Region),
        ("text/region", OcrProviderBlocks::Region),
        (
            "text/detect",
            OcrProviderBlocks::Detect {
                line_merge_gap_factor: Some(0),
            },
        ),
    ] {
        let evaluation = evaluator.evaluate_target(&scene, id).expect("evaluated");
        let seen = provider.seen().pop().expect("one read");
        assert_eq!((seen.width, seen.height), (16, 8), "{id}");
        assert_eq!(seen.region, rect(0, 0, ROI.width, ROI.height), "{id}");
        assert_eq!(seen.blocks, blocks, "{id}: Implicit becomes Region");
        assert!(seen.pixels == expected, "{id}: CFL-1's output, R = G = B");
        let ocr = ocr_evidence(&evaluation);
        assert_eq!(ocr.blocks.len(), 1, "{id}");
        assert_eq!(
            ocr.blocks[0].rect, block,
            "{id}: moved back onto the region"
        );
        assert_eq!(ocr.region.roi, Some(OcrRegionRect::from(ROI)), "{id}");
        let evidence = ocr.preprocess.as_ref().expect("the chain's evidence");
        assert_eq!(evidence.rect, ROI, "{id}");
        assert_eq!(evidence.scope, OcrPreprocessScope::Target, "{id}");
    }
    assert_eq!(provider.seen().len(), 4);
}

#[test]
fn cfl2_t3_a_chained_read_passes_the_confidence_gate_that_the_frame_itself_misses() {
    let scene = scene();
    // Only a binary region of the target's size reads at 0.95; anything else at 0.816.
    let provider = RecordingOcr::new(|seen| {
        let binary = seen.pixels.chunks(3).all(|pixel| {
            pixel[0] == pixel[1] && pixel[1] == pixel[2] && matches!(pixel[0], 0 | 255)
        });
        let small = (seen.width, seen.height) == (16, 8);
        seven(seen, if binary && small { 0.95 } else { 0.816 })
    });
    let evaluator = build(
        pack(
            vec![
                ocr("count/chained", region_json(ROI), Some(white_chain())),
                ocr("count/plain", region_json(ROI), None),
            ],
            Vec::new(),
        ),
        provider,
    );
    let chained = evaluator
        .evaluate_target(&scene, "count/chained")
        .expect("evaluated");
    let plain = evaluator
        .evaluate_target(&scene, "count/plain")
        .expect("evaluated");
    assert!(chained.passed, "{chained:?}");
    assert_eq!(ocr_evidence(&chained).confidence, Some(0.95));
    assert!(!plain.passed, "{plain:?}");
    assert_eq!(ocr_evidence(&plain).confidence, Some(0.816));
    assert_eq!(plain.message, "ocr confidence below threshold");
}

#[test]
fn cfl2_t4_a_chained_target_over_the_whole_frame_declares_locate() {
    let whole = rect(0, 0, WIDTH as i32, HEIGHT as i32);
    for (region, mut targets) in [
        (json!("full_frame"), Vec::new()),
        (region_json(whole), Vec::new()),
        (relative(WIDTH as i32, HEIGHT as i32), vec![anchor()]),
    ] {
        targets.push(ocr("text/whole", region.clone(), Some(white_chain())));
        let index = targets.len() - 1;
        assert_eq!(
            refusal(pack(targets, Vec::new())),
            (
                format!("/targets/{index}/preprocess"),
                ResourceDeclarationReason::InvalidValue
            ),
            "{region}"
        );
    }

    // With `locate` it reads, its block mode explicit, the whole frame processed.
    let scene = scene();
    let provider = RecordingOcr::seven(0.95);
    let evaluator = build(
        pack(
            vec![located(
                "text/whole",
                json!("full_frame"),
                "detect",
                Some(white_chain()),
            )],
            Vec::new(),
        ),
        provider.clone(),
    );
    let evaluation = evaluator
        .evaluate_target(&scene, "text/whole")
        .expect("evaluated");
    let seen = provider.seen().pop().expect("one read");
    assert_eq!((seen.width, seen.height), (WIDTH, HEIGHT));
    assert_eq!(
        seen.blocks,
        OcrProviderBlocks::Detect {
            line_merge_gap_factor: Some(0)
        }
    );
    assert!(seen.pixels != scene.rgb8_pixels(), "the processed frame");
    assert!(ocr_evidence(&evaluation).preprocess.is_some());
}

#[test]
fn cfl2_t5_one_region_read_with_two_chains_is_two_reads_and_with_one_chain_one_read() {
    let scene = scene();
    let provider = RecordingOcr::seven(0.95);
    let evaluator = build(
        pack(
            vec![
                located(
                    "text/first",
                    region_json(ROI),
                    "region",
                    Some(white_chain()),
                ),
                located("text/same", region_json(ROI), "region", Some(white_chain())),
                located(
                    "text/other",
                    region_json(ROI),
                    "region",
                    Some(reordered_chain()),
                ),
            ],
            Vec::new(),
        ),
        provider.clone(),
    );
    let context = evaluator.scene_context(&scene);
    context.evaluate_target("text/first").expect("evaluated");
    context.evaluate_target("text/same").expect("evaluated");
    assert_eq!(provider.seen().len(), 1, "the same chain: one read");
    context.evaluate_target("text/other").expect("evaluated");
    assert_eq!(provider.seen().len(), 2, "another chain: its own read");

    // The static budget: five targets over one region count once with one chain, and as five
    // reads, past the four-call budget, with five chains.
    let chains = [
        json!({"steps": [{"kind": "gray"}]}),
        json!({"steps": [{"kind": "gray"}, {"kind": "invert"}]}),
        json!({"steps": [{"kind": "gray"}, {"kind": "contrast", "low": 10, "high": 200}]}),
        json!({"steps": [{"kind": "gray"}, {"kind": "threshold", "value": 128}]}),
        white_chain(),
    ];
    let same = [
        white_chain(),
        white_chain(),
        white_chain(),
        white_chain(),
        white_chain(),
    ];
    assert!(static_reads(&same).iter().all(Result::is_ok));
    let counted = static_reads(&chains);
    assert!(counted[..4].iter().all(Result::is_ok));
    let overrun = counted[4].expect_err("the fifth chain is a fifth read");
    assert!(matches!(overrun.budget, TextReadBudget::Calls));
    assert_eq!((overrun.used, overrun.limit), (4, 4));
}

/// One consumer's static count of locate targets `text/<k>` over the same region, one per chain
/// of `chains`: the count's answer to each target, in order.
fn static_reads(chains: &[Value]) -> Vec<Result<(), TextReadBudgetOverrun>> {
    let targets = chains
        .iter()
        .enumerate()
        .map(|(index, chain)| {
            located(
                &format!("text/{index}"),
                region_json(ROI),
                "region",
                Some(chain.clone()),
            )
        })
        .collect::<Vec<_>>();
    let evaluator = build(pack(targets, Vec::new()), RecordingOcr::seven(0.95));
    let mut count = TextReadCount::default();
    (0..chains.len())
        .map(|index| evaluator.count_text_reads(&mut count, &format!("text/{index}")))
        .collect()
}

#[test]
fn cfl2_t6_kept_and_remembered_reads_reuse_their_evidence_without_running_the_chain() {
    let scene = scene();
    let provider = RecordingOcr::seven(0.95);
    let evaluator = build(
        pack(
            vec![
                ocr("count/kept", region_json(ROI), Some(white_chain())),
                located(
                    "text/first",
                    region_json(ROI),
                    "region",
                    Some(white_chain()),
                ),
                located("text/same", region_json(ROI), "region", Some(white_chain())),
            ],
            Vec::new(),
        ),
        provider.clone(),
    );

    // REC-6: the page gate's read of a kept target, then the same frame's collection.
    let kept = HashSet::from(["count/kept".to_owned()]);
    let gate = evaluator
        .scene_context(&scene)
        .with_remembered_targets(&kept);
    let runs = chain_runs();
    let first = gate.evaluate_target("count/kept").expect("evaluated");
    let again = gate.evaluate_target("count/kept").expect("evaluated");
    assert_eq!(chain_runs(), runs + 1);
    assert_eq!(provider.seen().len(), 1);
    let evidence = ocr_evidence(&first).preprocess.clone().expect("evidence");
    assert_eq!(ocr_evidence(&again).preprocess.as_ref(), Some(&evidence));
    let collection = evaluator
        .scene_context(&scene)
        .with_remembered_observations(gate.take_remembered_observations());
    let observation = collection
        .evaluate_ocr_observation("count/kept")
        .expect("observed");
    assert_eq!(observation.preprocess.as_ref(), Some(&evidence));
    assert_eq!(chain_runs(), runs + 1, "the kept observation runs no chain");
    assert_eq!(provider.seen().len(), 1);

    // REC-1: a remembered text read of the same request.
    let context = evaluator.scene_context(&scene);
    let runs = chain_runs();
    let first = context.evaluate_target("text/first").expect("evaluated");
    let same = context.evaluate_target("text/same").expect("evaluated");
    assert_eq!(chain_runs(), runs + 1, "the remembered read runs no chain");
    assert_eq!(provider.seen().len(), 2);
    assert_eq!(
        ocr_evidence(&first).preprocess,
        ocr_evidence(&same).preprocess
    );
}

#[test]
fn cfl2_t7_the_observation_path_hands_over_and_a_saved_frame_report_carries_the_evidence() {
    let scene = scene();
    let white = chain(&white_chain());
    let (expected, cfl1) = processed(&scene, ROI, &white);
    let provider = RecordingOcr::seven(0.95);
    let evaluator = build(
        pack(
            vec![ocr("count/field", region_json(ROI), Some(white_chain()))],
            Vec::new(),
        ),
        provider.clone(),
    );
    let kernel = evaluator
        .evaluate_target(&scene, "count/field")
        .expect("evaluated");
    let observed = evaluator
        .evaluate_ocr_observation(&scene, "count/field")
        .expect("observed");
    let saved = evaluator
        .evaluate_ocr_observation_with_timeout(&scene, "count/field", 1000)
        .expect("observed");
    for seen in &provider.seen()[1..] {
        assert_eq!(seen.region, rect(0, 0, ROI.width, ROI.height));
        assert_eq!(seen.blocks, OcrProviderBlocks::Region);
        assert!(seen.pixels == expected);
    }
    let evidence = ocr_evidence(&kernel).preprocess.clone().expect("evidence");
    assert_eq!(evidence.kept_milli, cfl1.kept_milli);
    assert_eq!(evidence.inverted, cfl1.inverted);
    assert_eq!(evidence.threshold_value, cfl1.threshold_value);
    assert_eq!(observed.preprocess.as_ref(), Some(&evidence));
    assert_eq!(saved.preprocess.as_ref(), Some(&evidence));
    assert_eq!(saved.blocks[0].rect, rect(ROI.x + 2, ROI.y + 1, 10, 5));
    // A saved frame's OCR report writes the observation as it is: the evidence with it.
    let written = serde_json::to_value(&saved).expect("serialized");
    assert_eq!(
        written["preprocess"],
        json!({
            "algorithm": "preprocess.v1",
            "steps": white_chain()["steps"],
            "scope": "target",
            "rect": region_json(ROI),
            "kept_milli": cfl1.kept_milli,
            "inverted": true,
            "threshold_value": null
        })
    );
}

#[test]
fn cfl2_t8_a_shared_read_serves_only_its_own_chain_and_counts_each_target_over_its_region() {
    let scene = scene();
    let white = chain(&white_chain());
    let provider = RecordingOcr::new(|seen| {
        let mut read = seven(seen, 0.95);
        read.blocks[0].rect = rect(seen.region.x + 3, seen.region.y + 3, 6, 4);
        read.detection = Some(OcrProviderDetection {
            detected: 3,
            limit: 64,
        });
        read
    });
    let targets = vec![
        ocr("slot/a", region_json(SLOT_A), Some(white_chain())),
        ocr("slot/b", region_json(SLOT_B), None),
        ocr("slot/c", region_json(SLOT_C), Some(reordered_chain())),
    ];
    let evaluator = build(
        pack(targets.clone(), vec![slots_layout(Some(white_chain()))]),
        provider.clone(),
    );
    let shared = evaluator.pack().candidate_layouts[0]
        .shared_ocr
        .clone()
        .expect("a shared read");
    let context = evaluator.scene_context(&scene);
    let sample = CandidateSampleVariant {
        frame: 0,
        dx: 0,
        dy: 0,
        template_metric: None,
    };
    let mut reads = BTreeMap::new();
    let served = context
        .shared_text(&shared, "slot/a", None, sample, &mut reads)
        .expect("read")
        .expect("served by the shared read");
    for other in ["slot/b", "slot/c"] {
        assert!(
            context
                .shared_text(&shared, other, None, sample, &mut reads)
                .expect("read")
                .is_none(),
            "{other} reads on its own"
        );
    }
    let seen = provider.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].region, rect(0, 0, SHARED.width, SHARED.height));
    let slot = ocr_evidence(&served);
    assert_eq!(slot.blocks[0].rect, rect(SHARED.x + 3, SHARED.y + 3, 6, 4));
    let evidence = slot.preprocess.as_ref().expect("the slot's evidence");
    let (_, own) = processed(&scene, SLOT_A, &white);
    let (_, whole) = processed(&scene, SHARED, &white);
    assert_eq!(evidence.scope, OcrPreprocessScope::Shared);
    assert_eq!(evidence.rect, SLOT_A);
    assert_eq!(evidence.source_rect, Some(SHARED));
    assert_eq!(
        evidence.kept_milli, own.kept_milli,
        "as the slot on its own"
    );
    assert_ne!(own.kept_milli, whole.kept_milli);
    assert_eq!(evidence.inverted, own.inverted);
    assert_eq!(
        slot.shared_read,
        Some(OcrSharedReadEvidence {
            rect: SHARED,
            detected: Some(3),
            limit: Some(64),
            kept_milli: Some(whole.kept_milli),
            threshold_value: None,
        })
    );

    // Truncation still fails loud with a chain.
    let truncating = RecordingOcr::new(|seen| {
        let mut read = seven(seen, 0.95);
        read.detection = Some(OcrProviderDetection {
            detected: 65,
            limit: 64,
        });
        read
    });
    let evaluator = build(
        pack(targets, vec![slots_layout(Some(white_chain()))]),
        truncating,
    );
    let error = evaluator
        .scene_context(&scene)
        .shared_text(&shared, "slot/a", None, sample, &mut BTreeMap::new())
        .expect_err("a truncated read fails");
    assert!(
        error
            .outcome()
            .is_some_and(|outcome| outcome.is(RecognitionPackCode::TextReadBlocksTruncated)),
        "{error}"
    );

    // A shared read takes no Otsu threshold.
    let otsu = json!({"steps": [{"kind": "gray"}, {"kind": "threshold", "value": "otsu"}]});
    assert_eq!(
        refusal(pack(
            vec![ocr("slot/a", region_json(SLOT_A), Some(otsu.clone()))],
            vec![slots_layout(Some(otsu))],
        )),
        (
            "/candidate_layouts/0/shared_ocr/preprocess/steps/1/value".to_owned(),
            ResourceDeclarationReason::InvalidValue
        )
    );
}

thread_local! {
    static SAVED_TICKS: Cell<u32> = const { Cell::new(0) };
    static SPENT_TICKS: Cell<u32> = const { Cell::new(0) };
}

/// One hour after the epoch: a deadline the frame's own checks before the read never reach.
fn deadline() -> Instant {
    test_epoch() + Duration::from_secs(3600)
}

/// Half a second before the deadline, on every read of the clock.
fn late_clock() -> Instant {
    deadline() - Duration::from_millis(500)
}

/// The epoch, then 400 ms later on every further read.
fn saved_clock() -> Instant {
    let tick = SAVED_TICKS.with(|ticks| ticks.replace(ticks.get() + 1));
    test_epoch() + Duration::from_millis(if tick == 0 { 0 } else { 400 })
}

/// The epoch, then the whole 1000 ms budget later on every further read.
fn spent_clock() -> Instant {
    let tick = SPENT_TICKS.with(|ticks| ticks.replace(ticks.get() + 1));
    test_epoch() + Duration::from_millis(if tick == 0 { 0 } else { 1000 })
}

#[test]
fn cfl2_t9_preprocessing_that_spends_the_budget_calls_no_provider() {
    let scene = scene();
    let provider = RecordingOcr::seven(0.95);
    let evaluator = build(
        pack(
            vec![ocr("count/field", region_json(ROI), Some(white_chain()))],
            Vec::new(),
        ),
        provider.clone(),
    );

    // The sample deadline: after the chain, 500 ms are left for a 1000 ms read.
    let runs = chain_runs();
    let error = evaluator
        .scene_context(&scene)
        .with_sample_deadline(deadline())
        .with_search_clock(late_clock)
        .evaluate_target("count/field")
        .expect_err("the read no longer fits the sample");
    assert!(
        error
            .message()
            .contains("recognition sample budget insufficient"),
        "{error}"
    );
    assert_eq!(chain_runs(), runs + 1);
    assert!(provider.seen().is_empty(), "no provider call");

    // A saved frame's request budget: the chain's 400 ms leave 600 ms to the provider.
    let context = evaluator
        .scene_context(&scene)
        .with_search_clock(saved_clock);
    evaluator
        .evaluate_ocr_observation_in_scene(&context, "count/field", Some(1000))
        .expect("observed");
    assert_eq!(
        provider
            .seen()
            .iter()
            .map(|seen| seen.timeout_ms)
            .collect::<Vec<_>>(),
        [600]
    );
    // All of it spent: the existing error, and no provider call.
    let context = evaluator
        .scene_context(&scene)
        .with_search_clock(spent_clock);
    let error = evaluator
        .evaluate_ocr_observation_in_scene(&context, "count/field", Some(1000))
        .expect_err("nothing left");
    assert_eq!(error.message(), "OCR request budget exhausted");
    assert_eq!(provider.seen().len(), 1);
}

/// Review L3: the recheck after the chain guards a layout's shared read too. Its check before the
/// chain passes (the deadline is an hour away on the wall clock), the chain runs once, and then
/// the context's clock leaves 500 ms for the read's 5000 ms: the read fails with the existing
/// error and the provider is not called.
#[test]
fn cfl2_t9b_a_chained_shared_read_that_spends_the_budget_calls_no_provider() {
    let scene = scene();
    let provider = RecordingOcr::seven(0.95);
    let evaluator = build(
        pack(
            vec![
                ocr("slot/a", region_json(SLOT_A), Some(white_chain())),
                ocr("slot/b", region_json(SLOT_B), None),
                ocr("slot/c", region_json(SLOT_C), Some(reordered_chain())),
            ],
            vec![slots_layout(Some(white_chain()))],
        ),
        provider.clone(),
    );
    let shared = evaluator.pack().candidate_layouts[0]
        .shared_ocr
        .clone()
        .expect("a shared read");
    let sample = CandidateSampleVariant {
        frame: 0,
        dx: 0,
        dy: 0,
        template_metric: None,
    };
    let runs = chain_runs();
    let error = evaluator
        .scene_context(&scene)
        .with_sample_deadline(deadline())
        .with_search_clock(late_clock)
        .shared_text(&shared, "slot/a", None, sample, &mut BTreeMap::new())
        .expect_err("the shared read no longer fits the sample");
    assert!(
        error
            .message()
            .contains("recognition sample budget insufficient"),
        "{error}"
    );
    assert_eq!(chain_runs(), runs + 1, "refused after the chain");
    assert!(provider.seen().is_empty(), "no provider call");
}

#[test]
fn cfl2_t11_admission_refuses_each_malformed_chain_at_its_pointer() {
    use ResourceDeclarationReason::{
        InvalidType, InvalidValue, MissingField, UnconsumedField, UnknownField,
    };
    let keep = |member: &str, value: Value| {
        let mut step = json!({"kind": "keep_colors", "colors": [[255, 255, 255]], "tolerance": 40});
        step[member] = value;
        json!({"steps": [step]})
    };
    let cases = [
        (json!([]), "", InvalidType),
        (
            json!({"steps": [{"kind": "gray"}], "fallback": []}),
            "/fallback",
            UnknownField,
        ),
        (json!({}), "/steps", MissingField),
        (json!({"steps": {"kind": "gray"}}), "/steps", InvalidType),
        (json!({"steps": []}), "/steps", InvalidValue),
        (
            json!({"steps": [{"kind": "gray"}, {"kind": "contrast", "low": 1, "high": 2},
                             {"kind": "threshold", "value": 9}, {"kind": "invert"},
                             {"kind": "invert"}]}),
            "/steps",
            InvalidValue,
        ),
        (json!({"steps": ["gray"]}), "/steps/0", InvalidType),
        (json!({"steps": [{}]}), "/steps/0/kind", MissingField),
        (
            json!({"steps": [{"kind": 1}]}),
            "/steps/0/kind",
            InvalidType,
        ),
        (
            json!({"steps": [{"kind": "blur"}]}),
            "/steps/0/kind",
            InvalidValue,
        ),
        (
            json!({"steps": [{"kind": "gray", "low": 1}]}),
            "/steps/0/low",
            UnknownField,
        ),
        (
            json!({"steps": [{"kind": "keep_colors", "colors": [[1, 2, 3]]}]}),
            "/steps/0/tolerance",
            MissingField,
        ),
        (
            keep("tolerance", json!(256)),
            "/steps/0/tolerance",
            InvalidValue,
        ),
        (
            keep("tolerance", json!("40")),
            "/steps/0/tolerance",
            InvalidType,
        ),
        (
            keep("colors", json!([[255, 255]])),
            "/steps/0/colors/0",
            InvalidValue,
        ),
        (
            keep("colors", json!([[255, -1, 0]])),
            "/steps/0/colors/0/1",
            InvalidValue,
        ),
        (keep("colors", json!([])), "/steps/0/colors", InvalidValue),
        (
            json!({"steps": [{"kind": "keep_range", "space": "lab", "lower": [0, 0, 0], "upper": [1, 1, 1]}]}),
            "/steps/0/space",
            InvalidValue,
        ),
        (
            json!({"steps": [{"kind": "keep_range", "space": "hsv", "lower": [180, 0, 0], "upper": [10, 255, 255]}]}),
            "/steps/0/lower",
            InvalidValue,
        ),
        (
            json!({"steps": [{"kind": "invert"}]}),
            "/steps/0/kind",
            InvalidValue,
        ),
        (
            json!({"steps": [{"kind": "gray"}, {"kind": "gray"}]}),
            "/steps/1/kind",
            InvalidValue,
        ),
        (
            json!({"steps": [{"kind": "gray"}, {"kind": "threshold", "value": 0}]}),
            "/steps/1/value",
            InvalidValue,
        ),
        (
            json!({"steps": [{"kind": "gray"}, {"kind": "threshold", "value": "auto"}]}),
            "/steps/1/value",
            InvalidValue,
        ),
        (
            json!({"steps": [{"kind": "gray"}, {"kind": "contrast", "low": 9, "high": 9}]}),
            "/steps/1/high",
            InvalidValue,
        ),
    ];
    for (preprocess, pointer, reason) in cases {
        assert_eq!(
            refusal(pack(
                vec![ocr("text/one", region_json(ROI), Some(preprocess.clone()))],
                Vec::new(),
            )),
            (format!("/targets/0/preprocess{pointer}"), reason),
            "{preprocess}"
        );
    }

    // Only a pack of schema 0.7 takes it, and only on an OCR target.
    let mut old = pack(
        vec![ocr("text/one", region_json(ROI), Some(white_chain()))],
        Vec::new(),
    );
    old["schema_version"] = json!("0.6");
    assert_eq!(
        refusal(old),
        ("/targets/0/preprocess".to_owned(), UnconsumedField)
    );
    let color = json!({
        "type": "color", "id": "color/probe", "region": region_json(ROI),
        "expected": [255, 255, 255], "preprocess": white_chain()
    });
    assert_eq!(
        refusal(pack(vec![color], Vec::new())),
        ("/targets/0/preprocess".to_owned(), UnknownField)
    );

    // The declaration round-trips: the parser reads back what the writer writes.
    for chain_json in [white_chain(), reordered_chain()] {
        let parsed = chain(&chain_json);
        assert_eq!(parsed.to_json(), chain_json);
        assert_eq!(chain(&parsed.to_json()), parsed);
    }
}

#[test]
fn cfl2_t12_a_target_whose_chain_differs_from_its_shared_read_is_named_once() {
    let divergences = |a: Option<Value>, b: Option<Value>, c: Option<Value>| {
        load_pack_from_json_str(
            &pack(
                vec![
                    ocr("slot/a", region_json(SLOT_A), a),
                    ocr("slot/b", region_json(SLOT_B), b),
                    ocr("slot/c", region_json(SLOT_C), c),
                ],
                vec![slots_layout(Some(white_chain()))],
            )
            .to_string(),
        )
        .expect("the pack loads")
        .preprocess_divergences()
    };
    assert!(
        divergences(
            Some(white_chain()),
            Some(white_chain()),
            Some(white_chain())
        )
        .is_empty()
    );
    assert_eq!(
        divergences(Some(white_chain()), None, Some(reordered_chain())),
        vec![
            (0_usize, "slot/b".to_owned()),
            (0_usize, "slot/c".to_owned())
        ]
    );
}

#[test]
fn cfl2_t13_the_chain_runs_once_per_provider_call_and_a_refused_read_calls_nothing() {
    let scene = scene();
    let provider = RecordingOcr::seven(0.95);
    let evaluator = build(
        pack(
            vec![
                ocr("count/chained", region_json(ROI), Some(white_chain())),
                ocr("count/plain", region_json(ROI), None),
            ],
            Vec::new(),
        ),
        provider.clone(),
    );
    let runs = chain_runs();
    for _ in 0..3 {
        evaluator
            .evaluate_target(&scene, "count/chained")
            .expect("evaluated");
    }
    evaluator
        .evaluate_target(&scene, "count/plain")
        .expect("evaluated");
    let seen = provider.seen();
    assert_eq!(seen.len(), 4);
    assert_eq!(chain_runs(), runs + 3, "once before each chained call");
    for read in &seen[..3] {
        assert_eq!(
            read.pixels.len(),
            3 * 16 * 8,
            "the hand-over buffer is 3·w·h"
        );
    }

    // Refused after the chain: the chain ran, the provider was not called.
    let error = evaluator
        .scene_context(&scene)
        .with_sample_deadline(deadline())
        .with_search_clock(late_clock)
        .evaluate_target("count/chained")
        .expect_err("refused");
    assert!(error.message().contains("budget insufficient"), "{error}");
    assert_eq!(chain_runs(), runs + 4);
    assert_eq!(provider.seen().len(), 4);
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
