// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 SEL-GRID G1 (register §1.490; contracts/vision-model-folders.md, the
// `detector.max_boxes` row; the coordinator's work order
// rd-1119/brief-463-g1-shared-read-serves-name-boxes.md, its tests 2 and 6 and the provider's side
// of tests 3 and 5): a `Detect` request that names served name boxes keeps, between the merge of
// boxes into lines and the folder's `max_boxes`, only the boxes whose centre lies in one,
// recognizes those and counts them apart; a request that names none reads as before.
//
// No ONNX Runtime and no clock: the request runs the engine's own path (`read_request`) over a fake
// whose detector finds the boxes a test gives it and whose recognizer reads each box it is asked
// for as the text `x,y` of its corner and records the box. Every expected value comes from the
// rule and the fixtures below. The boxes are those of ten cards in four columns, in rows of four,
// four and two, 100 by 80 pixels apart: a card's name box is 92 x 36 at (4, 12) from its corner,
// its name one line or two inside it, and six other texts centred outside it.

use super::*;
use actingcommand_vision_ffi::VisionFrame;

const TIMEOUT_MS: u64 = 1_000;
/// The folder's `merge_gap_factor` by default; a `Detect` request with a factor replaces it.
const FOLDER_GAP_FACTOR: u32 = 3;

/// A model whose detector is built: a detection finds `boxes`; a recognition reads its box's
/// corner and records the box.
struct Stages {
    boxes: Vec<VisionRect>,
    recognized: Vec<VisionRect>,
}

impl Stages {
    fn new(boxes: Vec<VisionRect>) -> Self {
        Self {
            boxes,
            recognized: Vec::new(),
        }
    }
}

impl RequestStages for Stages {
    fn clock(&self) -> Duration {
        Duration::ZERO
    }

    fn has_detector(&self) -> bool {
        true
    }

    fn ready_detector(&mut self, _diagnostics: &mut PpocrDiagnostics) -> VisionFfiResult<()> {
        Ok(())
    }

    fn recognize(
        &mut self,
        _frame: &VisionFrameView<'_>,
        region: VisionRect,
        _budget: Duration,
    ) -> Result<DecodedText, ProviderInvokeError> {
        self.recognized.push(region);
        Ok(DecodedText {
            text: format!("{},{}", region.x, region.y),
            confidence: Some(0.9),
        })
    }

    fn detect(
        &mut self,
        _frame: &VisionFrameView<'_>,
        _region: VisionRect,
        _merge_gap_factor: Option<u32>,
        _budget: Duration,
    ) -> Result<Vec<DetectedTextBox>, ProviderInvokeError> {
        Ok(self
            .boxes
            .iter()
            .map(|rect| DetectedTextBox {
                rect: *rect,
                confidence: 0.9,
            })
            .collect())
    }
}

fn rect(x: i32, y: i32, width: i32, height: i32) -> VisionRect {
    VisionRect {
        x,
        y,
        width,
        height,
    }
}

/// The frame of the cards, and the request's region, the whole of it.
fn frame() -> VisionFrame {
    VisionFrame {
        width: 420,
        height: 260,
        pixel_format: VisionPixelFormat::Rgb8,
        pixels: vec![0; 420 * 260 * 3],
    }
}

/// One `Detect` request (factor 0) over the whole frame with the name boxes `served`, read on
/// `stages` with the folder's `max_boxes`, as the engine reads it.
fn read(stages: &mut Stages, served: &[VisionRect], max_boxes: u32) -> OcrInferenceResult {
    let frame = frame();
    let languages = vec!["en".to_string()];
    let request = OcrInferenceRequestView {
        frame: frame.view(),
        region: rect(0, 0, 420, 260),
        languages: &languages,
        timeout_ms: TIMEOUT_MS,
        blocks: OcrBlockMode::Detect {
            line_merge: OcrLineMerge::GapFactor(0),
        },
        served,
    };
    request.validate().expect("a valid request");
    read_request(
        stages,
        FOLDER_GAP_FACTOR,
        max_boxes,
        request,
        &mut Vec::new(),
        &mut InvocationRoles::default(),
    )
    .unwrap_or_else(|error| panic!("the read: {error:?}"))
}

/// The cards' corners.
fn cards() -> Vec<(i32, i32)> {
    [
        (10, 10),
        (110, 10),
        (210, 10),
        (310, 10),
        (10, 90),
        (110, 90),
        (210, 90),
        (310, 90),
        (10, 170),
        (110, 170),
    ]
    .to_vec()
}

fn name_box((x, y): (i32, i32)) -> VisionRect {
    rect(x + 4, y + 12, 92, 36)
}

/// A card's name: two lines at 14..29 and 31..46 on an even card, one at 22..38 on an odd one.
fn name_lines(index: usize, (x, y): (i32, i32)) -> Vec<VisionRect> {
    if index % 2 == 0 {
        vec![rect(x + 8, y + 14, 60, 15), rect(x + 8, y + 31, 50, 15)]
    } else {
        vec![rect(x + 10, y + 22, 70, 16)]
    }
}

/// A card's six other texts, two centred above its name box and four below it.
fn others((x, y): (i32, i32)) -> Vec<VisionRect> {
    vec![
        rect(x + 16, y + 1, 30, 9),
        rect(x + 50, y + 1, 40, 9),
        rect(x + 4, y + 50, 40, 9),
        rect(x + 50, y + 50, 40, 9),
        rect(x + 4, y + 60, 40, 9),
        rect(x + 50, y + 60, 40, 9),
    ]
}

/// Every box the detector finds, card by card, in reading order of the cards: each card's name
/// lines, then its other texts; and the name lines alone, in the same order.
fn boxes() -> (Vec<VisionRect>, Vec<VisionRect>) {
    let mut all = Vec::new();
    let mut names = Vec::new();
    for (index, card) in cards().into_iter().enumerate() {
        let lines = name_lines(index, card);
        all.extend(lines.iter().copied());
        names.extend(lines);
        all.extend(others(card));
    }
    (all, names)
}

// Test 2: with the ten cards' name boxes, the recognizer runs on the 15 name lines alone, in the
// detector's order, not on the 75 boxes detected; the answer holds those 15 blocks and counts 75
// detected and 15 served. The same boxes without name boxes are read as before: the first 64 of
// the 75 recognized, 75 detected and no served count, which the kernel refuses as truncated; and
// 64 boxes, at the cap, are all recognized.
#[test]
fn g1_t2_a_read_with_name_boxes_recognizes_only_the_boxes_they_hold() {
    let (all, names) = boxes();
    assert_eq!((all.len(), names.len()), (75, 15), "setup");
    let served = cards().into_iter().map(name_box).collect::<Vec<_>>();

    let mut stages = Stages::new(all.clone());
    let result = read(&mut stages, &served, 64);
    assert_eq!(stages.recognized, names);
    assert_eq!(
        result
            .blocks
            .iter()
            .map(|block| block.rect)
            .collect::<Vec<_>>(),
        names
    );
    assert_eq!(result.blocks[0].text, "18,24");
    assert_eq!(
        result.detection,
        Some(OcrDetectionCount {
            detected: 75,
            limit: 64,
            served: Some(15),
        })
    );

    let mut stages = Stages::new(all.clone());
    let result = read(&mut stages, &[], 64);
    assert_eq!(stages.recognized.as_slice(), &all[..64]);
    assert_eq!(
        result.detection,
        Some(OcrDetectionCount {
            detected: 75,
            limit: 64,
            served: None,
        })
    );

    let mut stages = Stages::new(all[..64].to_vec());
    let result = read(&mut stages, &[], 64);
    assert_eq!(stages.recognized.as_slice(), &all[..64]);
    assert_eq!(
        result.detection,
        Some(OcrDetectionCount {
            detected: 64,
            limit: 64,
            served: None,
        })
    );
}

// Test 3, the provider's side: more served boxes than the cap (15 over 10) are cut to the first 10
// in the detector's order and counted whole, so the kernel can tell the served count was over.
#[test]
fn g1_t3_served_boxes_over_the_cap_are_cut_and_counted_whole() {
    let (all, names) = boxes();
    let served = cards().into_iter().map(name_box).collect::<Vec<_>>();
    let mut stages = Stages::new(all);

    let result = read(&mut stages, &served, 10);

    assert_eq!(stages.recognized.as_slice(), &names[..10]);
    assert_eq!(
        result.detection,
        Some(OcrDetectionCount {
            detected: 75,
            limit: 10,
            served: Some(15),
        })
    );
}

// The name box of test 6, x 100..140 and y 50..70, its right and bottom edges outside.
const EDGE_BOX: VisionRect = VisionRect {
    x: 100,
    y: 50,
    width: 40,
    height: 20,
};

/// Boxes whose centres `(x + floor(w/2), y + floor(h/2))` lie on each edge and corner of
/// `EDGE_BOX`, or one pixel past it, of even and odd sizes, with whether the kernel's rule gives
/// each to the target of that box: the left and top edges inside, the right and bottom outside.
fn edge_boxes() -> Vec<(VisionRect, bool)> {
    vec![
        // The top-left corner (100, 50), inside, with an even and an odd box.
        (rect(98, 48, 4, 4), true),
        (rect(97, 47, 7, 7), true),
        // The top-right corner (140, 50), outside; its last column (139, 50), inside.
        (rect(138, 48, 4, 4), false),
        (rect(137, 48, 5, 4), true),
        // The bottom-left corner (100, 70), outside; its last row (100, 69), inside.
        (rect(98, 68, 4, 4), false),
        (rect(98, 67, 4, 5), true),
        // The bottom-right corner (140, 70), outside; the last pixel (139, 69), inside.
        (rect(138, 68, 4, 4), false),
        (rect(137, 67, 5, 5), true),
        // The left edge (100, 60), inside; one pixel left of it (99, 60), outside.
        (rect(98, 58, 4, 4), true),
        (rect(97, 58, 4, 4), false),
        // The top edge (120, 50), inside; one pixel above it (120, 49), outside.
        (rect(118, 48, 4, 4), true),
        (rect(118, 47, 4, 4), false),
        // The right edge (140, 60), outside; one pixel left of it (139, 60), inside.
        (rect(138, 58, 4, 4), false),
        (rect(137, 58, 5, 4), true),
        // The bottom edge (120, 70), outside; one pixel above it (120, 69), inside.
        (rect(118, 68, 4, 4), false),
        (rect(118, 67, 4, 5), true),
    ]
}

// Test 6, the provider's side: of boxes centred on each edge and corner of a name box, the read
// keeps and recognizes exactly those the kernel gives to that box's target (`edge_boxes`), by the
// one rule both apply (`ocr_box_is_served`, the contract crate's), and counts them as served.
#[test]
fn g1_t6_the_provider_keeps_a_box_exactly_when_the_kernel_gives_it_to_the_target() {
    let edges = edge_boxes();
    for (rect, inside) in &edges {
        assert_eq!(ocr_box_is_served(&[EDGE_BOX], *rect), *inside, "{rect:?}");
    }
    let mut stages = Stages::new(edges.iter().map(|(rect, _)| *rect).collect());

    let result = read(&mut stages, &[EDGE_BOX], 64);

    let inside = edges
        .iter()
        .filter(|(_, inside)| *inside)
        .map(|(rect, _)| *rect)
        .collect::<Vec<_>>();
    assert_eq!(stages.recognized, inside);
    assert_eq!(
        result.detection,
        Some(OcrDetectionCount {
            detected: 16,
            limit: 64,
            served: Some(9),
        })
    );
}
