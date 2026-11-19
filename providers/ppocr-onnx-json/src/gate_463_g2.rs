// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 SEL-GRID G2 (register §1.491; contracts/selection-graph.md, "The lines of a name
// box"; contracts/vision-model-folders.md, the `detector.max_boxes` row; the coordinator's work
// order rd-1119/brief-463-g2-name-lines.md, the provider's side of its tests): a `Detect` request
// that names served name boxes and `lines` plans each served box by them on the frame it is given:
// a box lower than half a line is not read and is reported apart, a box at least 1.25 lines high is
// cut into its lines at the rows of least ink near the equal positions and each strip recognized on
// its own, every other box is recognized whole; the cap counts recognitions.
//
// No ONNX Runtime and no clock: the request runs the engine's own path (`read_request`) over a
// fake whose detector finds the boxes a test gives it and whose recognizer reads a region as a
// recognizer of one line reads it: a region that holds one drawn line whole reads that line's text
// at 0.95; a region that holds parts of two lines or more, or a part of one, reads their texts run
// together at 0.3; a region that holds no line reads nothing. It records every region it is asked.
// The frame is a light gray 200 with each line drawn as dark (40) vertical stripes every third
// column, the way letters give a row of a line its edges and leave a row between lines without
// any; every expected value comes from that drawing, the rules of the planner and the numbers
// below.

use super::*;
use actingcommand_vision_ffi::VisionFrame;

const TIMEOUT_MS: u64 = 1_000;
const FOLDER_GAP_FACTOR: u32 = 3;
const WIDTH: u32 = 200;
const HEIGHT: u32 = 120;
const BACKGROUND: u8 = 200;
const INK: u8 = 40;
/// The name box every test serves: x 10..190, y 10..110.
const NAME_BOX: VisionRect = VisionRect {
    x: 10,
    y: 10,
    width: 180,
    height: 100,
};
/// The lines of a name box of two: at most two of 21 pixels, as measured on 11-19.
const TWO_LINES: OcrNameLines = OcrNameLines {
    max: 2,
    line_height: 21,
};

fn rect(x: i32, y: i32, width: i32, height: i32) -> VisionRect {
    VisionRect {
        x,
        y,
        width,
        height,
    }
}

/// One drawn line of text: its rectangle and the text the fake reads from it.
#[derive(Clone)]
struct Line {
    rect: VisionRect,
    text: &'static str,
}

fn line(text: &'static str, top: i32, height: i32) -> Line {
    Line {
        rect: rect(20, top, 120, height),
        text,
    }
}

/// The frame with `lines` drawn on the background.
fn frame(lines: &[Line]) -> VisionFrame {
    let mut pixels = vec![BACKGROUND; (WIDTH * HEIGHT * 3) as usize];
    for drawn in lines {
        let rect = drawn.rect;
        for y in rect.y..rect.y + rect.height {
            for x in (rect.x..rect.x + rect.width).step_by(3) {
                let at = ((y as u32 * WIDTH + x as u32) * 3) as usize;
                pixels[at..at + 3].copy_from_slice(&[INK; 3]);
            }
        }
    }
    VisionFrame {
        width: WIDTH,
        height: HEIGHT,
        pixel_format: VisionPixelFormat::Rgb8,
        pixels,
    }
}

/// The fake of the module comment over `lines`, its detector finding `boxes`.
struct Stages {
    lines: Vec<Line>,
    boxes: Vec<VisionRect>,
    recognized: Vec<VisionRect>,
}

impl Stages {
    fn new(lines: &[Line], boxes: Vec<VisionRect>) -> Self {
        Self {
            lines: lines.to_vec(),
            boxes,
            recognized: Vec::new(),
        }
    }
}

/// Whether the rows `top..bottom` of `inner` lie inside those of `outer`.
fn rows_within(inner: VisionRect, outer: VisionRect) -> bool {
    inner.y >= outer.y && inner.y + inner.height <= outer.y + outer.height
}

/// Whether the rows of `a` and `b` overlap.
fn rows_overlap(a: VisionRect, b: VisionRect) -> bool {
    a.y < b.y + b.height && b.y < a.y + a.height
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
        let held = self
            .lines
            .iter()
            .filter(|drawn| rows_overlap(drawn.rect, region))
            .collect::<Vec<_>>();
        Ok(match held.as_slice() {
            [] => DecodedText {
                text: String::new(),
                confidence: None,
            },
            [one] if rows_within(one.rect, region) => DecodedText {
                text: one.text.to_string(),
                confidence: Some(0.95),
            },
            more => DecodedText {
                text: more.iter().map(|drawn| drawn.text).collect(),
                confidence: Some(0.3),
            },
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

/// One `Detect` request (factor 0) over the whole frame of `lines`, serving `served` with `lines`
/// when given, read on `stages` with the folder's `max_boxes`, as the engine reads it.
fn read(
    stages: &mut Stages,
    served: &[VisionRect],
    lines: Option<OcrNameLines>,
    max_boxes: u32,
) -> OcrInferenceResult {
    let frame = frame(&stages.lines);
    let languages = vec!["en".to_string()];
    let request = OcrInferenceRequestView {
        frame: frame.view(),
        region: rect(0, 0, WIDTH as i32, HEIGHT as i32),
        languages: &languages,
        timeout_ms: TIMEOUT_MS,
        blocks: OcrBlockMode::Detect {
            line_merge: OcrLineMerge::GapFactor(0),
        },
        served,
        lines,
    };
    request.validate().expect("a valid request");
    let result = read_request(
        stages,
        FOLDER_GAP_FACTOR,
        max_boxes,
        request,
        &mut Vec::new(),
        &mut InvocationRoles::default(),
    )
    .unwrap_or_else(|error| panic!("the read: {error:?}"));
    result
        .validate_for(request)
        .expect("the answer is valid for its request");
    result
}

fn texts(result: &OcrInferenceResult) -> Vec<&str> {
    result
        .blocks
        .iter()
        .map(|block| block.text.as_str())
        .collect()
}

fn strip_texts(block: &OcrTextBlock) -> Vec<&str> {
    block
        .strips
        .iter()
        .map(|strip| strip.text.as_str())
        .collect()
}

// Test 1, the provider's side: a name of two lines 21 px high and 3 px apart (rows 20..41 and
// 44..65), detected as one box of 45 px, about twice the line height. Read as today (no lines) the
// box is recognized whole, as one line, and reads both lines run together at 0.3. With the lines
// of two the box is cut at row 43, between them (45 px make round(45 / 21) = 2 strips; the equal
// position, row 20 + 23, has no ink, so it stays), each strip is recognized on its own and reads
// its line, and the box answers "ALPHA\nBRAVO" at 0.95 with its two strips: the texts the same
// lines read when the detector finds them apart, two boxes of one line each, which the lines read
// whole as they are read without them.
#[test]
fn g2_two_lines_detected_as_one_box_are_cut_and_read_as_when_detected_apart() {
    let lines = [line("ALPHA", 20, 21), line("BRAVO", 44, 21)];
    let merged = rect(20, 20, 120, 45);

    let mut stages = Stages::new(&lines, vec![merged]);
    let today = read(&mut stages, &[NAME_BOX], None, 64);
    assert_eq!(stages.recognized, [merged]);
    assert_eq!(texts(&today), ["ALPHABRAVO"]);
    assert_eq!(today.blocks[0].confidence, Some(0.3));
    assert!(today.blocks[0].strips.is_empty());

    let mut stages = Stages::new(&lines, vec![merged]);
    let cut = read(&mut stages, &[NAME_BOX], Some(TWO_LINES), 64);
    let strips = [rect(20, 20, 120, 23), rect(20, 43, 120, 22)];
    assert_eq!(stages.recognized, strips);
    assert_eq!(texts(&cut), ["ALPHA\nBRAVO"]);
    assert_eq!(cut.blocks[0].rect, merged);
    assert_eq!(cut.blocks[0].confidence, Some(0.95));
    assert_eq!(strip_texts(&cut.blocks[0]), ["ALPHA", "BRAVO"]);
    assert_eq!(
        cut.blocks[0]
            .strips
            .iter()
            .map(|strip| strip.rect)
            .collect::<Vec<_>>(),
        strips
    );
    assert_eq!(
        cut.detection,
        Some(OcrDetectionCount {
            detected: 1,
            limit: 64,
            served: Some(2),
        })
    );

    let apart = [rect(20, 20, 120, 21), rect(20, 44, 120, 21)];
    for lines_declared in [None, Some(TWO_LINES)] {
        let mut stages = Stages::new(&lines, apart.to_vec());
        let detected_apart = read(&mut stages, &[NAME_BOX], lines_declared, 64);
        assert_eq!(stages.recognized, apart, "{lines_declared:?}");
        assert_eq!(texts(&detected_apart), strip_texts(&cut.blocks[0]));
        assert!(
            detected_apart
                .blocks
                .iter()
                .all(|block| block.strips.is_empty())
        );
    }
}

// Test 2: lines of unequal height, 19 px (rows 20..39) and 23 px (rows 42..65), 3 px apart, one box
// of 45 px. The equal position, row 43, is the second line's second row; within a quarter of the
// line height (5 rows) the least ink lies on the rows between the lines, 39 to 41, and the cut goes
// to the closest of them, row 41: the strips 20..41 and 41..65 each hold one line whole and read
// it. A cut at the equal position would have given the first strip a row of the second line.
#[test]
fn g2_the_cut_goes_to_the_rows_between_lines_of_unequal_height() {
    let lines = [line("ALPHA", 20, 19), line("BRAVO", 42, 23)];
    let merged = rect(20, 20, 120, 45);
    let equal = 20 + 23;
    assert!(
        rows_overlap(lines[1].rect, rect(20, equal, 120, 1)),
        "setup"
    );

    let mut stages = Stages::new(&lines, vec![merged]);
    let cut = read(&mut stages, &[NAME_BOX], Some(TWO_LINES), 64);

    let strips = [rect(20, 20, 120, 21), rect(20, 41, 120, 24)];
    assert_eq!(stages.recognized, strips);
    assert_ne!(strips[1].y, equal);
    assert_eq!(texts(&cut), ["ALPHA\nBRAVO"]);
    assert_eq!(strip_texts(&cut.blocks[0]), ["ALPHA", "BRAVO"]);
}

// Test 3: a name of one line (22 px, rows 30..52) in a box that allows two: its box is about one
// line high, under the factor (22 < 1.25 x 21), so it is recognized whole, once, and the answer is
// the answer without lines, member for member.
#[test]
fn g2_a_name_of_one_line_in_a_box_of_two_reads_as_without_lines() {
    let lines = [line("CHARLIE", 30, 22)];
    let one = rect(20, 30, 120, 22);

    let mut stages = Stages::new(&lines, vec![one]);
    let without = read(&mut stages, &[NAME_BOX], None, 64);
    assert_eq!(stages.recognized, [one]);

    let mut stages = Stages::new(&lines, vec![one]);
    let with = read(&mut stages, &[NAME_BOX], Some(TWO_LINES), 64);
    assert_eq!(stages.recognized, [one]);
    assert_eq!(with, without);
    assert_eq!(texts(&with), ["CHARLIE"]);
}

// Test 4: three lines of 21 px, 3 px apart (rows 20..41, 44..65, 68..89), one box of 69 px. With
// `max` 3 it is cut in round(69 / 21) = 3 strips at rows 43 and 66, both between lines, and reads
// "ALPHA\nBRAVO\nCHARLIE". With `max` 2 it is cut in two at the equal position, row 55, inside the
// second line (no row within 5 of it has less ink): each strip holds parts of two lines and reads
// them run together at 0.3, so the name reads poorly; a box of three lines is the author's to
// declare with `max` 3.
#[test]
fn g2_three_lines_are_cut_in_three_with_max_three_and_in_two_with_max_two() {
    let lines = [
        line("ALPHA", 20, 21),
        line("BRAVO", 44, 21),
        line("CHARLIE", 68, 21),
    ];
    let merged = rect(20, 20, 120, 69);

    let mut stages = Stages::new(&lines, vec![merged]);
    let three = read(
        &mut stages,
        &[NAME_BOX],
        Some(OcrNameLines {
            max: 3,
            line_height: 21,
        }),
        64,
    );
    assert_eq!(
        stages.recognized,
        [
            rect(20, 20, 120, 23),
            rect(20, 43, 120, 23),
            rect(20, 66, 120, 23)
        ]
    );
    assert_eq!(texts(&three), ["ALPHA\nBRAVO\nCHARLIE"]);
    assert_eq!(three.blocks[0].confidence, Some(0.95));

    let mut stages = Stages::new(&lines, vec![merged]);
    let two = read(&mut stages, &[NAME_BOX], Some(TWO_LINES), 64);
    assert_eq!(
        stages.recognized,
        [rect(20, 20, 120, 35), rect(20, 55, 120, 34)]
    );
    assert_eq!(texts(&two), ["ALPHABRAVO\nBRAVOCHARLIE"]);
    assert_eq!(two.blocks[0].confidence, Some(0.3));
}

// Test 5, the provider's side: a box of 9 px inside the name box beside a name of one line, lower
// than half the line height (9 < 10.5): with the lines it is not recognized, it is answered in
// `below_floor`, and it takes no recognition (served 1); without the lines it is recognized as
// every box is (served 2).
#[test]
fn g2_a_box_below_half_a_line_is_not_read_and_is_reported() {
    let lines = [line("CHARLIE", 30, 22), line("SPECK", 70, 9)];
    let name = rect(20, 30, 120, 22);
    let speck = rect(20, 70, 120, 9);

    let mut stages = Stages::new(&lines, vec![name, speck]);
    let with = read(&mut stages, &[NAME_BOX], Some(TWO_LINES), 64);
    assert_eq!(stages.recognized, [name]);
    assert_eq!(texts(&with), ["CHARLIE"]);
    assert_eq!(with.below_floor, [speck]);
    assert_eq!(with.detection.and_then(|count| count.served), Some(1));

    let mut stages = Stages::new(&lines, vec![name, speck]);
    let without = read(&mut stages, &[NAME_BOX], None, 64);
    assert_eq!(stages.recognized, [name, speck]);
    assert!(without.below_floor.is_empty());
    assert_eq!(without.detection.and_then(|count| count.served), Some(2));
}

// Test 6, the provider's side: three served boxes of two lines each, under a cap of 4, are six
// recognitions once cut: the count says 6 served over the limit 4, which the kernel refuses as
// truncated, and only the strips of the first two boxes, the first 4 recognitions in the
// detector's order, are recognized.
#[test]
fn g2_the_cap_counts_a_cut_box_as_its_strips() {
    let lines = [line("ALPHA", 20, 21), line("BRAVO", 44, 21)];
    let boxes = [
        rect(20, 20, 120, 45),
        rect(20, 20, 120, 45),
        rect(20, 20, 120, 45),
    ];
    let mut stages = Stages::new(&lines, boxes.to_vec());

    let result = read(&mut stages, &[NAME_BOX], Some(TWO_LINES), 4);

    assert_eq!(
        result.detection,
        Some(OcrDetectionCount {
            detected: 3,
            limit: 4,
            served: Some(6),
        })
    );
    assert_eq!(stages.recognized.len(), 4);
    assert_eq!(texts(&result), ["ALPHA\nBRAVO", "ALPHA\nBRAVO"]);
}

// The request's guard: lines come with served name boxes and within their bounds (1 to 3 lines,
// at least 8 px), else the request is refused InvalidRequest before it is read; the same request
// within them is valid.
#[test]
fn g2_lines_need_served_boxes_and_their_bounds() {
    let frame = frame(&[]);
    let languages = vec!["en".to_string()];
    let request = |served: &'static [VisionRect], lines: OcrNameLines| {
        OcrInferenceRequestView {
            frame: frame.view(),
            region: rect(0, 0, WIDTH as i32, HEIGHT as i32),
            languages: &languages,
            timeout_ms: TIMEOUT_MS,
            blocks: OcrBlockMode::Detect {
                line_merge: OcrLineMerge::GapFactor(0),
            },
            served,
            lines: Some(lines),
        }
        .validate()
    };
    const SERVED: &[VisionRect] = &[NAME_BOX];
    const NONE: &[VisionRect] = &[];
    for (served, lines) in [
        (NONE, TWO_LINES),
        (
            SERVED,
            OcrNameLines {
                max: 0,
                line_height: 21,
            },
        ),
        (
            SERVED,
            OcrNameLines {
                max: 4,
                line_height: 21,
            },
        ),
        (
            SERVED,
            OcrNameLines {
                max: 2,
                line_height: 7,
            },
        ),
    ] {
        let error = request(served, lines).expect_err("refused");
        assert_eq!(
            error.code(),
            VisionFfiErrorCode::InvalidRequest,
            "{lines:?}"
        );
    }
    request(SERVED, TWO_LINES).expect("lines of a served box within their bounds");
}
