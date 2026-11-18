// SPDX-License-Identifier: AGPL-3.0-only

// #463 S1-R (contracts/lab-recording.md, "A ruler on the pictures a recording hands back"; register
// §1.476): an optional ruler (`edges`, `grid`) on the previews `record review` writes, in the
// stored frame's own pixels, never on a stored frame; fix round 1 (the coordinator's ruling): the
// ruler is `record review`'s only, and `record frames` is exactly what it is on main. The pictures
// are drawn by the previews' own renderer (`preview::render`) on synthetic frames of one flat
// colour, and the commands read a recording made as stage one makes it, with S2-5's bench
// (`gate_463_s25_draft`: two recorded taps on its 160x90 screens of gray with a square of noise,
// then the final screen; step 1 marked with a recognize box and a press box over its button).
// - `s1r_without_a_ruler_every_picture_and_answer_is_as_before`: an answer without a ruler has
//   today's members and no `ruler`, and a preview's file name is today's (no recorded file needed);
//   a copy with nothing to draw is the stored frame decoded and encoded again, byte for byte;
//   `record review`'s previews keep their names and bytes across a ruled review and are written
//   again byte for byte, its answer is the same and has no `ruler`, and at the ruler's places they
//   are the stored frame; no recorded file changes.
// - `s1r_edges_ticks_every_50_numbers_every_100_in_the_fonts_digits`: on a 320x240 copy the ticks
//   at 50 and 100 along both edges in the dark tone over the light halo, a pixel away from the
//   edges the frame's own, the numbers 100 (top) and 200 (left) cell by cell in the font, whose
//   digits 0 and 1 are pinned, and 300, which would not fit, not drawn; `record review` with
//   `edges` names its previews as ruled and answers `ruler`; beside `no_preview` it is refused.
// - `s1r_grid_lines_at_the_hundreds_only_under_the_boxes_and_the_press`: the dashed lines at 100
//   and 200, nothing at 50 or 150 away from the edges, and a box and a tap drawn over them.
// - `s1r_frames_under_100_pixels_and_of_three_or_four_bytes_a_pixel`: 60x40 and 1x1 frames of
//   three bytes a pixel and a 30x90 one of four, with each ruler: no panic, each copy of its size,
//   what fits drawn, the rest the frame's own, the alpha kept.
// - `s1r_record_frames_is_as_on_main_and_a_ruled_preview_has_a_name_of_its_own`: `record frames`'
//   row is main's (no `ruler`, `writes` false, its help saying it reads only), its request main's
//   four members, its answer main's members, each listed frame the stored frame by its path, and
//   it writes nothing; a ruled preview of `record review` is written beside the plain preview of
//   the same step, in the recording's `lab/preview/`, as `s<nn>-<role>-ruled-<word>.png`, never
//   over the plain one, whose bytes stay.
// - `s1r_the_operation_table_carries_ruler_with_its_closed_list`: the row `record review` declares
//   the option `ruler` of type `enum:edges|grid`, the Lab's words, and `record frames` none; the MCP
//   schema is generated from the row (`apps/actinglab-mcp`, an enumeration's words in `enum`, held
//   for every offered row's parameters by `gate_434_lab5g2a`).
// No Runtime, no device; the tests read no clock and never sleep.

use crate::gate_463_s25_draft::{self as draft, Bench, INSTANCE, POINTS};
use crate::recording::frames::decode_frame;
use crate::recording::preview::{
    DIGITS, Glyph, Overlay, PRESS_BOX_COLOUR, PRESS_COLOUR, RULER_DARK, RULER_FONT_SCALE,
    RULER_LIGHT, render,
};
use crate::recording::review::preview_path;
use crate::recording::store::{hex_sha256, lab_dir};
use crate::{
    PreviewRuler, RecordFramesOutcome, RecordFramesRequest, RecordPoint, RecordRect,
    RecordReviewOutcome, RecordReviewRequest, RecordStopOptions, record_frames, record_review,
};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

// The colour of the synthetic frames, and its alpha on a frame of four bytes a pixel.
const FRAME_RGB: [u8; 3] = [90, 140, 200];
const FRAME_ALPHA: u8 = 200;
// The bench's background gray, where nothing of a screen is drawn.
const BENCH_GRAY: [u8; 3] = [60, 60, 60];

// The PNG of a flat frame of `FRAME_RGB` of `width` by `height`, of three bytes a pixel or, with
// `Rgba8`, of four with `FRAME_ALPHA`.
fn flat(width: u32, height: u32, format: PixelFormat) -> Vec<u8> {
    let pixel = match format {
        PixelFormat::Rgb8 => FRAME_RGB.to_vec(),
        PixelFormat::Rgba8 => [FRAME_RGB[0], FRAME_RGB[1], FRAME_RGB[2], FRAME_ALPHA].to_vec(),
    };
    let count = usize::try_from(width * height).expect("pixels");
    Frame::from_pixels(
        width,
        height,
        pixel.repeat(count),
        format,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
    .encode_png_fast()
    .expect("fixture PNG")
}

// The decoded pixels of a PNG.
struct Image {
    width: u32,
    height: u32,
    channels: usize,
    pixels: Vec<u8>,
}

impl Image {
    fn of(png: Vec<u8>) -> Self {
        let frame = decode_frame(png, "a PNG of the test").expect("a PNG").frame;
        let channels = match frame.pixel_format {
            PixelFormat::Rgb8 => 3,
            PixelFormat::Rgba8 => 4,
        };
        Self {
            width: frame.width,
            height: frame.height,
            channels,
            pixels: frame.pixels,
        }
    }

    fn read(path: &str) -> Self {
        Self::of(fs::read(path).expect("the PNG file"))
    }

    fn start(&self, x: i64, y: i64) -> usize {
        let x = usize::try_from(x).expect("x");
        let y = usize::try_from(y).expect("y");
        let width = usize::try_from(self.width).expect("width");
        (y * width + x) * self.channels
    }

    // The red, green and blue of the pixel at `x`, `y`.
    fn at(&self, x: i64, y: i64) -> [u8; 3] {
        let start = self.start(x, y);
        [
            self.pixels[start],
            self.pixels[start + 1],
            self.pixels[start + 2],
        ]
    }

    // The alpha of the pixel at `x`, `y`, on a picture of four bytes a pixel.
    fn alpha(&self, x: i64, y: i64) -> Option<u8> {
        (self.channels == 4).then(|| self.pixels[self.start(x, y) + 3])
    }
}

// The copy of `png` with `overlay` and `ruler` drawn, decoded.
fn drawn(png: &[u8], overlay: &Overlay, ruler: Option<PreviewRuler>) -> Image {
    let copy = render(png.to_vec(), overlay, ruler, "the test's frame")
        .unwrap_or_else(|error| panic!("render: {}: {}", error.code, error.message));
    Image::of(copy)
}

// Asserts that the number `text` stands with its first digit's top-left pixel at `at`, in the
// font's digits: every pixel of a drawn cell (`#`) dark, every pixel of another cell not dark.
fn assert_number(image: &Image, text: &str, at: (i64, i64)) {
    // Three cells and one empty one from a digit to the next.
    let advance = 4 * RULER_FONT_SCALE;
    for (place, digit) in (0_i64..).zip(text.bytes()) {
        let glyph = DIGITS[usize::from(digit - b'0')];
        let left = at.0 + place * advance;
        for (line, row) in (0_i64..).zip(glyph) {
            let top = at.1 + line * RULER_FONT_SCALE;
            for (column, cell) in (0_i64..).zip(row.bytes()) {
                let x = left + column * RULER_FONT_SCALE;
                for down in 0..RULER_FONT_SCALE {
                    for across in 0..RULER_FONT_SCALE {
                        let pixel = image.at(x + across, top + down);
                        if cell == b'#' {
                            assert_eq!(
                                pixel,
                                RULER_DARK,
                                "{text} at {},{}",
                                x + across,
                                top + down
                            );
                        } else {
                            assert_ne!(
                                pixel,
                                RULER_DARK,
                                "{text} at {},{}",
                                x + across,
                                top + down
                            );
                        }
                    }
                }
            }
        }
    }
}

// The recording: two taps recorded with the screen after each (screens 0, 1 and 2), the final
// screen the second tap's screen after; step 1 marked with a recognize box and a press box over its
// button.
fn two_taps() -> Bench {
    let bench = Bench::new();
    for (index, point) in POINTS.into_iter().take(2).enumerate() {
        bench.press(
            &draft::tap_at(point, draft::one_capture_after()),
            vec![draft::screen(index), draft::screen(index + 1)],
        );
    }
    bench.marked(json!({"step": 1, "page": "first",
                        "boxes": [draft::button_box("recognize", 0), draft::button_box("press", 0)]}));
    bench
}

// `record review` of `request` on the bench's recording, `record ready`'s options without flags.
fn review(bench: &Bench, request: &RecordReviewRequest) -> RecordReviewOutcome {
    record_review(
        &bench.state(),
        INSTANCE,
        request,
        &RecordStopOptions::default(),
        &bench.ocr,
    )
    .unwrap_or_else(|error| {
        panic!(
            "record review: {}: {} {:?}",
            error.code, error.message, error.details
        )
    })
}

// `record frames` of step `step`, its request built with every member it has, main's four: a member
// added to it does not compile here.
fn frames(bench: &Bench, step: u32) -> RecordFramesOutcome {
    record_frames(
        &bench.state(),
        INSTANCE,
        &RecordFramesRequest {
            record_id: None,
            step,
            before: None,
            after: None,
        },
    )
    .unwrap_or_else(|error| {
        panic!(
            "record frames: {}: {} {:?}",
            error.code, error.message, error.details
        )
    })
}

// Each preview an answer of `record review` names: the frame it is drawn on and the preview.
fn previews(answer: &RecordReviewOutcome) -> Vec<(String, String)> {
    let json = serde_json::to_value(answer).expect("answer JSON");
    let mut named = Vec::new();
    for step in json["steps"].as_array().expect("steps") {
        for role in ["before", "after"] {
            if let (Some(path), Some(preview)) =
                (step[role]["path"].as_str(), step[role]["preview"].as_str())
            {
                named.push((path.to_owned(), preview.to_owned()));
            }
        }
    }
    named
}

// Every file under `dir`, by its `/`-separated path relative to it, with its bytes, in name order;
// none when `dir` does not exist.
fn files_under(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    if !dir.exists() {
        return files;
    }
    let mut pending = vec![(dir.to_path_buf(), String::new())];
    while let Some((path, prefix)) = pending.pop() {
        for entry in fs::read_dir(&path).expect("directory") {
            let entry = entry.expect("entry");
            let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
            if entry.file_type().expect("file type").is_dir() {
                pending.push((entry.path(), format!("{name}/")));
            } else {
                files.push((name, fs::read(entry.path()).expect("file")));
            }
        }
    }
    files.sort();
    files
}

// The files of the recording's directory but its previews.
fn recorded_files(bench: &Bench, record_id: &str) -> Vec<(String, Vec<u8>)> {
    files_under(&lab_dir(&bench.state(), record_id))
        .into_iter()
        .filter(|(name, _)| !name.starts_with("preview/"))
        .collect()
}

// What the answer says of `ruler`.
fn ruler_member(ruler: &str) -> Value {
    let mut member = json!({"kind": ruler, "space": "frame_pixels", "tick_px": 50,
                            "long_tick_px": 100, "number_px": 100, "top_numbers": "x",
                            "left_numbers": "y"});
    if ruler == "grid" {
        member["line_px"] = json!(100);
    }
    member
}

// The file names of the previews `named` (as `previews` gives them), in their order.
fn file_names(named: &[(String, String)]) -> Vec<String> {
    named
        .iter()
        .map(|(_, preview)| {
            Path::new(preview)
                .file_name()
                .expect("a file name")
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

// The members of the JSON object `value`, sorted.
fn keys(value: &Value) -> Vec<String> {
    let mut keys = value
        .as_object()
        .unwrap_or_else(|| panic!("an object: {value}"))
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    keys
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

// The work order's test 1: without the parameter every byte is as before. Needing no recorded file:
// an answer of `record review` without a ruler has today's members and no `ruler`, and a preview's
// file name without one is today's, `lab/preview/s<nn>-<role>.png`. A copy with nothing to draw is
// the stored frame decoded and encoded again, byte for byte, as the renderer wrote it before the
// ruler existed. `record review` without a ruler writes its previews under their names; a review
// with `grid` writes ruled previews under other names and leaves them byte for byte; a review
// without it again writes them byte for byte and answers the same, without `ruler`; at the places
// a ruler draws on, a preview without one is its stored frame. No recorded file changes.
#[test]
fn s1r_without_a_ruler_every_picture_and_answer_is_as_before() {
    let bare = RecordReviewOutcome {
        record_id: "r1".to_owned(),
        session: "active".to_owned(),
        steps: Vec::new(),
        steps_total: 0,
        warnings: Vec::new(),
        ruler: None,
    };
    assert_eq!(
        serde_json::to_value(&bare).expect("answer JSON"),
        json!({"record_id": "r1", "session": "active", "steps": [], "steps_total": 0,
               "warnings": []}),
        "an answer without a ruler has no ruler member"
    );
    let lab = Path::new("lab");
    assert_eq!(
        preview_path(lab, 1, "before", None),
        lab.join("preview").join("s01-before.png")
    );
    assert_eq!(
        preview_path(lab, 12, "after", None),
        lab.join("preview").join("s12-after.png")
    );

    for png in [flat(320, 240, PixelFormat::Rgb8), draft::screen(0)] {
        let copy = render(png.clone(), &Overlay::default(), None, "the test's frame")
            .unwrap_or_else(|error| panic!("render: {}: {}", error.code, error.message));
        let again = Frame::from_png(png, CaptureBackendName::AdbScreencap)
            .expect("the frame decodes")
            .encode_png_fast()
            .expect("the frame encodes");
        assert_eq!(copy, again, "a copy with nothing drawn on it");
    }

    let bench = two_taps();
    let plain_request = RecordReviewRequest::default();
    let first = review(&bench, &plain_request);
    let record_id = first.record_id.clone();
    let recorded = recorded_files(&bench, &record_id);
    let plain = previews(&first);
    assert_eq!(
        file_names(&plain),
        [
            "s01-before.png",
            "s01-after.png",
            "s02-before.png",
            "s02-after.png",
            "s03-before.png"
        ],
        "the previews keep their names"
    );
    let bytes = plain
        .iter()
        .map(|(_, preview)| fs::read(preview).expect("a preview"))
        .collect::<Vec<_>>();
    let json = serde_json::to_value(&first).expect("answer JSON");
    assert!(json.get("ruler").is_none(), "{json}");

    let ruled = review(
        &bench,
        &RecordReviewRequest {
            ruler: Some(PreviewRuler::Grid),
            ..RecordReviewRequest::default()
        },
    );
    for ((_, preview), (_, ruled_preview)) in plain.iter().zip(previews(&ruled)) {
        assert_ne!(
            preview, &ruled_preview,
            "a ruled preview has a name of its own"
        );
    }
    for ((_, preview), before) in plain.iter().zip(&bytes) {
        assert_eq!(&fs::read(preview).expect("a preview"), before, "{preview}");
    }

    let third = review(&bench, &plain_request);
    assert_eq!(third, first, "the same answer");
    for ((_, preview), before) in previews(&third).iter().zip(&bytes) {
        assert_eq!(&fs::read(preview).expect("a preview"), before, "{preview}");
    }
    // Where a ruler draws (a tick at 50 on each edge, a longer one at 100, its number, a grid line),
    // a preview without one is its stored frame.
    for (path, preview) in &plain {
        let copy = Image::read(preview);
        let frame = Image::read(path);
        for (x, y) in [(50, 0), (0, 50), (100, 5), (106, 2), (100, 60)] {
            assert_eq!(copy.at(x, y), frame.at(x, y), "{preview} at {x},{y}");
        }
    }

    assert_eq!(
        recorded_files(&bench, &record_id),
        recorded,
        "a recorded file changed"
    );
}

// The work order's test 2: `edges`. On a copy of 320x240, along the top edge a tick of 6 pixels at
// 50 and of 12 at 100, along the left edge the same, each dark over a light halo, and beyond them
// the frame's own pixels; a pixel away from the edges is the frame's own, a grid's place too. The
// number 100 beside its tick on the top edge and 200 below its tick on the left edge are the
// font's digits, cell by cell (the digits 0 and 1 pinned here); 300, which would run past the right
// edge, is not drawn. `record review` with `edges` names each preview as ruled, its copy carrying
// the ruler, and answers `ruler`; beside `no_preview` it is refused and writes nothing.
#[test]
fn s1r_edges_ticks_every_50_numbers_every_100_in_the_fonts_digits() {
    assert_eq!(DIGITS[0], ["###", "#.#", "#.#", "#.#", "###"]);
    assert_eq!(DIGITS[1], [".#.", "##.", ".#.", ".#.", "###"]);
    let png = flat(320, 240, PixelFormat::Rgb8);
    let copy = drawn(&png, &Overlay::default(), Some(PreviewRuler::Edges));
    assert_eq!((copy.width, copy.height), (320, 240));
    // Each check: the pixel and what it is.
    let checks = [
        // The tick at 50 on the top edge, its halo and the frame below it.
        ((50, 0), RULER_DARK),
        ((50, 5), RULER_DARK),
        ((50, 6), RULER_LIGHT),
        ((49, 2), RULER_LIGHT),
        ((51, 2), RULER_LIGHT),
        ((50, 8), FRAME_RGB),
        // The longer tick at 100.
        ((100, 0), RULER_DARK),
        ((100, 11), RULER_DARK),
        ((100, 12), RULER_LIGHT),
        ((100, 14), FRAME_RGB),
        // The left edge.
        ((0, 50), RULER_DARK),
        ((5, 50), RULER_DARK),
        ((6, 50), RULER_LIGHT),
        ((2, 49), RULER_LIGHT),
        ((8, 50), FRAME_RGB),
        ((0, 100), RULER_DARK),
        ((11, 100), RULER_DARK),
        ((12, 100), RULER_LIGHT),
        // Away from the edges, and where a grid's line would be.
        ((75, 75), FRAME_RGB),
        ((160, 160), FRAME_RGB),
        ((100, 150), FRAME_RGB),
        ((150, 100), FRAME_RGB),
    ];
    for ((x, y), expected) in checks {
        assert_eq!(copy.at(x, y), expected, "at {x},{y}");
    }
    assert_number(&copy, "100", (104, 2));
    assert_number(&copy, "200", (2, 204));
    // 300 would end past the right edge: none of its cells is drawn.
    for x in 304..320 {
        for y in 1..13 {
            assert_eq!(copy.at(x, y), FRAME_RGB, "at {x},{y}");
        }
    }

    let bench = two_taps();
    let answer = review(
        &bench,
        &RecordReviewRequest {
            step: Some(1),
            ruler: Some(PreviewRuler::Edges),
            ..RecordReviewRequest::default()
        },
    );
    let json = serde_json::to_value(&answer).expect("answer JSON");
    assert_eq!(json["ruler"], ruler_member("edges"), "{json}");
    let named = previews(&answer);
    assert_eq!(
        file_names(&named),
        ["s01-before-ruled-edges.png", "s01-after-ruled-edges.png"]
    );
    for (path, preview) in &named {
        let copy = Image::read(preview);
        let frame = Image::read(path);
        assert_eq!((copy.width, copy.height), (frame.width, frame.height));
        assert_eq!(copy.at(50, 0), RULER_DARK, "{preview}");
        assert_eq!(copy.at(100, 11), RULER_DARK, "{preview}");
        assert_eq!(frame.at(100, 11), BENCH_GRAY, "{path}");
        assert_number(&copy, "100", (104, 2));
        assert_eq!(copy.at(150, 80), frame.at(150, 80), "{preview}");
    }

    let preview_dir = lab_dir(&bench.state(), &answer.record_id).join("preview");
    let written = files_under(&preview_dir);
    let refused = record_review(
        &bench.state(),
        INSTANCE,
        &RecordReviewRequest {
            no_preview: true,
            ruler: Some(PreviewRuler::Edges),
            ..RecordReviewRequest::default()
        },
        &RecordStopOptions::default(),
        &bench.ocr,
    )
    .expect_err("a ruler without previews");
    assert_eq!(refused.code, "validation_failed", "{}", refused.message);
    assert_eq!(
        refused.details,
        Some(json!({"argument": "ruler", "reason": "previews_only"}))
    );
    assert_eq!(files_under(&preview_dir), written, "a refusal wrote a file");
}

// The work order's test 3: `grid`. On a copy of 320x240 with a press box and a tap, the lines at
// 100 and 200 across the frame both ways, in dashes of 4 pixels, dark first; nothing at 50 or 150,
// or beside a line, away from the edges. The box's outline and the tap's point lie over the lines:
// the ruler is drawn first, and where nothing else is drawn the line shows.
#[test]
fn s1r_grid_lines_at_the_hundreds_only_under_the_boxes_and_the_press() {
    let png = flat(320, 240, PixelFormat::Rgb8);
    let overlay = Overlay {
        boxes: vec![(
            RecordRect {
                x: 80,
                y: 120,
                width: 60,
                height: 40,
            },
            PRESS_BOX_COLOUR,
        )],
        glyph: Some(Glyph::Tap(RecordPoint { x: 200, y: 200 })),
    };
    let copy = drawn(&png, &overlay, Some(PreviewRuler::Grid));
    let bare = drawn(&png, &Overlay::default(), Some(PreviewRuler::Grid));
    assert_eq!((copy.width, copy.height), (320, 240));
    let checks = [
        // The vertical line at 100: from 16 to 19 the fifth dash, dark; from 20 the sixth, light.
        ((100, 16), RULER_DARK),
        ((100, 20), RULER_LIGHT),
        ((200, 60), RULER_LIGHT),
        ((300, 40), RULER_DARK),
        // The horizontal lines at 100 and 200.
        ((120, 100), RULER_DARK),
        ((300, 100), RULER_LIGHT),
        ((60, 200), RULER_LIGHT),
        // Beside the lines, at 50 and 150, and between them: the frame's own.
        ((99, 60), FRAME_RGB),
        ((101, 60), FRAME_RGB),
        ((150, 60), FRAME_RGB),
        ((50, 60), FRAME_RGB),
        ((60, 99), FRAME_RGB),
        ((60, 101), FRAME_RGB),
        ((60, 150), FRAME_RGB),
        ((60, 50), FRAME_RGB),
        ((250, 220), FRAME_RGB),
    ];
    for ((x, y), expected) in checks {
        assert_eq!(copy.at(x, y), expected, "at {x},{y}");
        assert_eq!(bare.at(x, y), expected, "at {x},{y} without the overlay");
    }
    // The box's top and bottom sides and the tap's point, where the lines cross them, are the box's
    // and the press's colours; without the overlay the lines are there.
    assert_eq!(copy.at(100, 120), PRESS_BOX_COLOUR, "the box's top side");
    assert_eq!(copy.at(100, 121), PRESS_BOX_COLOUR, "the box's top side");
    assert_eq!(bare.at(100, 120), RULER_DARK);
    assert_eq!(copy.at(100, 159), PRESS_BOX_COLOUR, "the box's bottom side");
    assert_eq!(bare.at(100, 159), RULER_LIGHT);
    assert_eq!(copy.at(200, 200), PRESS_COLOUR, "the tap's point");
    assert_eq!(bare.at(200, 200), RULER_DARK);
    // Inside the box, under no outline, the line shows.
    assert_eq!(copy.at(100, 130), RULER_DARK);
}

// The work order's test 4: frames under 100 pixels and of three and four bytes a pixel. A 60x40
// and a 1x1 frame of three bytes and a 30x90 one of four, each with `edges` and with `grid`: no
// panic, each copy of its frame's size; on 60x40 the tick at 50 on the top edge, nothing on the
// left one or across; on 30x90 the tick at 50 on the left edge, its alpha kept, nothing on the top
// one; elsewhere the frame's own, with its alpha.
#[test]
fn s1r_frames_under_100_pixels_and_of_three_or_four_bytes_a_pixel() {
    for ruler in [PreviewRuler::Edges, PreviewRuler::Grid] {
        let small = drawn(
            &flat(60, 40, PixelFormat::Rgb8),
            &Overlay::default(),
            Some(ruler),
        );
        assert_eq!((small.width, small.height), (60, 40), "{ruler:?}");
        assert_eq!(small.at(50, 0), RULER_DARK, "{ruler:?}");
        assert_eq!(small.at(50, 5), RULER_DARK, "{ruler:?}");
        assert_eq!(small.at(49, 0), RULER_LIGHT, "{ruler:?}");
        for (x, y) in [(0, 0), (20, 20), (0, 39), (59, 39), (50, 8), (40, 20)] {
            assert_eq!(small.at(x, y), FRAME_RGB, "{ruler:?} at {x},{y}");
        }

        let one = drawn(
            &flat(1, 1, PixelFormat::Rgb8),
            &Overlay::default(),
            Some(ruler),
        );
        assert_eq!((one.width, one.height), (1, 1), "{ruler:?}");
        assert_eq!(one.at(0, 0), FRAME_RGB, "{ruler:?}");

        let narrow = drawn(
            &flat(30, 90, PixelFormat::Rgba8),
            &Overlay::default(),
            Some(ruler),
        );
        assert_eq!((narrow.width, narrow.height), (30, 90), "{ruler:?}");
        assert_eq!(narrow.channels, 4, "{ruler:?}");
        assert_eq!(narrow.at(0, 50), RULER_DARK, "{ruler:?}");
        assert_eq!(narrow.at(5, 50), RULER_DARK, "{ruler:?}");
        assert_eq!(narrow.at(2, 51), RULER_LIGHT, "{ruler:?}");
        assert_eq!(narrow.alpha(0, 50), Some(FRAME_ALPHA), "{ruler:?}");
        for (x, y) in [(0, 0), (15, 20), (29, 0), (20, 50), (29, 89)] {
            assert_eq!(narrow.at(x, y), FRAME_RGB, "{ruler:?} at {x},{y}");
            assert_eq!(
                narrow.alpha(x, y),
                Some(FRAME_ALPHA),
                "{ruler:?} at {x},{y}"
            );
        }
    }
}

// The work order's test 5, as fix round 1 rules it: the ruler is `record review`'s only, and
// `record frames` answers and is listed exactly as on main. Its row is main's: its parameters
// main's six and no `ruler`, `writes` false, its help saying it reads only, nothing of this slice
// in it. Its request is main's four members (`frames`), its answer main's members, and each frame
// it lists (the press's, those before and after it) main's members, its `path` the stored frame
// under `frames/`, whose bytes hash to the sha256 the answer gives; it writes nothing (the
// recording's directory byte for byte the same, no `preview/` made) and answers the same again,
// after a ruled review too. A ruled preview is `record review`'s: of step 2 with `grid`, written in
// the same `lab/preview/` as the plain previews of step 2, as `s02-before-ruled-grid.png` and
// `s02-after-ruled-grid.png`, beside `s02-before.png` and `s02-after.png`, which keep their bytes,
// so that it is never taken for the plain preview of the same step: exactly those four files, each
// ruled preview of its stored frame's size and carrying the ruler.
#[test]
fn s1r_record_frames_is_as_on_main_and_a_ruled_preview_has_a_name_of_its_own() {
    let row = table_row("record frames");
    assert_eq!(row["writes"], false, "{row}");
    let params = row["params"]
        .as_array()
        .expect("params")
        .iter()
        .map(|param| param["name"].as_str().expect("a parameter's name"))
        .collect::<Vec<_>>();
    assert_eq!(
        params,
        [
            "instance",
            "step",
            "before",
            "after",
            "record_id",
            "state_dir"
        ]
    );
    assert_eq!(
        row["help"],
        "Lists the stored frames captured just before and just after one recorded step's press, \
         each with its path and capture time; reads only."
    );
    let text = row.to_string();
    assert!(!text.contains("ruler"), "{row}");
    assert!(!text.contains("S1-R"), "{row}");

    let bench = two_taps();
    let first = frames(&bench, 2);
    let lab = lab_dir(&bench.state(), &first.record_id);
    let recorded = files_under(&lab);
    assert_eq!(frames(&bench, 2), first, "the same answer again");
    assert_eq!(files_under(&lab), recorded, "record frames wrote a file");
    assert!(
        !lab.join("preview").exists(),
        "record frames wrote a preview"
    );
    let json = serde_json::to_value(&first).expect("answer JSON");
    assert_eq!(
        keys(&json),
        [
            "after",
            "after_total",
            "artifact_step",
            "before",
            "before_total",
            "kind",
            "press",
            "record_id",
            "step",
            "warnings",
        ],
        "{json}"
    );
    let press_frame = &json["press"]["frame"];
    assert_eq!(
        keys(press_frame),
        [
            "captured_at_unix_ms",
            "frame_id",
            "path",
            "sha256",
            "stored_under_step",
        ],
        "{json}"
    );
    let mut listed = vec![press_frame];
    for side in ["before", "after"] {
        let side_frames = json[side].as_array().expect("a side");
        assert!(!side_frames.is_empty(), "{side}: {json}");
        for frame in side_frames {
            assert_eq!(
                keys(frame),
                [
                    "captured_at_unix_ms",
                    "frame_id",
                    "offset_ms",
                    "path",
                    "sha256",
                    "stored_under_step",
                ],
                "{json}"
            );
            listed.push(frame);
        }
    }
    for frame in listed {
        let frame_id = frame["frame_id"].as_str().expect("a frame id");
        let path = Path::new(frame["path"].as_str().expect("the stored frame's path"));
        assert_eq!(
            path.parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str()),
            Some("frames"),
            "{frame_id}: {}",
            path.display()
        );
        let stored = fs::read(path).expect("the stored frame");
        assert_eq!(frame["sha256"], json!(hex_sha256(&stored)), "{frame_id}");
    }

    let plain = previews(&review(
        &bench,
        &RecordReviewRequest {
            step: Some(2),
            ..RecordReviewRequest::default()
        },
    ));
    assert_eq!(file_names(&plain), ["s02-before.png", "s02-after.png"]);
    let plain_bytes = plain
        .iter()
        .map(|(_, preview)| fs::read(preview).expect("a preview"))
        .collect::<Vec<_>>();
    let ruled = previews(&review(
        &bench,
        &RecordReviewRequest {
            step: Some(2),
            ruler: Some(PreviewRuler::Grid),
            ..RecordReviewRequest::default()
        },
    ));
    assert_eq!(
        file_names(&ruled),
        ["s02-before-ruled-grid.png", "s02-after-ruled-grid.png"]
    );
    for (((path, preview), (ruled_path, ruled_preview)), bytes) in
        plain.iter().zip(&ruled).zip(&plain_bytes)
    {
        assert_eq!(path, ruled_path, "the same stored frame");
        assert_eq!(
            Path::new(ruled_preview).parent(),
            Path::new(preview).parent(),
            "{ruled_preview}: beside the plain preview"
        );
        assert_eq!(
            Path::new(preview)
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str()),
            Some("preview"),
            "{preview}"
        );
        assert_eq!(&fs::read(preview).expect("a preview"), bytes, "{preview}");
        let copy = Image::read(ruled_preview);
        let frame = Image::read(path);
        assert_eq!((copy.width, copy.height), (frame.width, frame.height));
        assert_eq!(copy.at(50, 0), RULER_DARK, "{ruled_preview}");
        assert_eq!(frame.at(100, 60), BENCH_GRAY, "{path}");
        assert_eq!(
            copy.at(100, 60),
            RULER_LIGHT,
            "{ruled_preview}: the line at 100"
        );
    }
    let mut expected = file_names(&plain);
    expected.extend(file_names(&ruled));
    expected.sort();
    let written = files_under(&lab.join("preview"))
        .into_iter()
        .map(|(name, _)| name)
        .collect::<Vec<_>>();
    assert_eq!(
        written, expected,
        "the ruled previews beside the plain ones"
    );
    assert_eq!(
        frames(&bench, 2),
        first,
        "record frames after a ruled review"
    );
}

// The work order's test 6: the operation table's row `record review` declares the option `ruler`,
// not required, of the type `enum:edges|grid`, whose words are the Lab's (`PreviewRuler::WORDS`),
// each read back to itself and no other word read; the row `record frames` declares none (fix round
// 1). The MCP tools are generated from these rows: an enumeration's words become its property's
// `enum` (`apps/actinglab-mcp/src/tools.rs`, `value_schema`), which `gate_434_lab5g2a` holds for
// every parameter of every offered row; and a wrong word is refused by the closed list's rule
// there and in `serve --stdio`.
#[test]
fn s1r_the_operation_table_carries_ruler_with_its_closed_list() {
    let row = table_row("record review");
    assert_eq!(row["writes"], true, "{row}");
    let ruler = row["params"]
        .as_array()
        .expect("params")
        .iter()
        .find(|param| param["name"] == "ruler")
        .expect("record review declares ruler");
    assert_eq!(ruler["form"], "option", "{ruler}");
    assert_eq!(ruler["required"], false, "{ruler}");
    assert_eq!(ruler["type"], "enum:edges|grid", "{ruler}");
    let words = ruler["type"]
        .as_str()
        .and_then(|kind| kind.strip_prefix("enum:"))
        .expect("an enumeration")
        .split('|')
        .collect::<Vec<_>>();
    assert_eq!(words, PreviewRuler::WORDS);
    let frames_row = table_row("record frames");
    let frames_params = frames_row["params"].as_array().expect("params");
    assert!(
        frames_params.iter().all(|param| param["name"] != "ruler"),
        "record frames declares no ruler: {frames_row}"
    );
    for word in PreviewRuler::WORDS {
        assert_eq!(
            PreviewRuler::from_word(word).map(PreviewRuler::word),
            Some(word)
        );
        assert_eq!(
            serde_json::to_value(PreviewRuler::from_word(word)).expect("the word as JSON"),
            json!(word)
        );
    }
    for other in ["", "Edges", "grids", "ruled", "lines"] {
        assert!(PreviewRuler::from_word(other).is_none(), "{other:?}");
    }
}
