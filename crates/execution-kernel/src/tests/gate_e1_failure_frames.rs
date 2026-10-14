// SPDX-License-Identifier: AGPL-3.0-only

// Test plan v2.2 E-1 G8 (exec key 13): the failure-frame comparison of
// `contracts/policy-suspension.md`, section "Error frames": both frames are digested whole with
// `color_digest.v1` on a grid of `min(32, width)` by `min(18, height)` cells; a cell has changed
// when `|dR| + |dG| + |dB|` of its quantized channels exceeds `CELL_DELTA = 6`; the frames are
// different when more than `MAX_CHANGED_MILLI = 250` changed cells per thousand, or when their
// sizes differ, otherwise similar; an undecodable frame is `png_invalid` (section "Settlement of
// an accumulating failure"). Reason codes as `contracts/outcome-codes/execution-kernel.json`
// registers them (`size_changed`, `digest_cells_changed`, `png_invalid`). Requirement basis:
// `rd-1009/tests/req-basis-el.md` section E-1, row "G8 failure frames". Synthetic frames only.

use crate::{FailureFrameVerdict, compare_failure_frames};
use actingcommand_contract::outcome::Code;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};

// 64x36 pixels: a grid of min(32, 64) x min(18, 36) = 32 x 18 = 576 cells of 2x2 pixels.
const WIDTH: u32 = 64;
const HEIGHT: u32 = 36;
const COLUMNS: u32 = 32;
// 250 changed cells per thousand of 576 cells.
const CELLS_AT_250_MILLI: u32 = 144;

// A channel value whose quantized level, `floor(value / 8)`, is exactly `level`.
fn channel(level: u32) -> u8 {
    u8::try_from(level * 8 + 4).expect("a quantized level is at most 31")
}

// The quantized [R, G, B] levels of cell `index` of the reference frame, each at most 19.
fn reference_levels(index: u32) -> [u32; 3] {
    let (column, row) = (index % COLUMNS, index / COLUMNS);
    [
        (column + row) % 20,
        (column * 3 + row) % 20,
        (column + row * 5) % 20,
    ]
}

// A PNG frame of `width` x `height` pixels in 2x2 cells; cell `index` has the reference levels
// raised by `shift(index)`, so every pixel of a cell has the same value.
fn frame_png(width: u32, height: u32, shift: impl Fn(u32) -> [u32; 3]) -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let index = (y / 2) * COLUMNS + x / 2;
            let levels = reference_levels(index)
                .into_iter()
                .zip(shift(index))
                .map(|(level, raise)| channel(level + raise));
            pixels.extend(levels);
        }
    }
    Frame::from_pixels(
        width,
        height,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
    .encode_png_fast()
    .expect("fixture PNG")
}

fn reference_png() -> Vec<u8> {
    frame_png(WIDTH, HEIGHT, |_| [0, 0, 0])
}

// policy-suspension.md "Error frames": different only when MORE than 250 changed cells per
// thousand. 144 of 576 cells is exactly 250 per thousand: similar; 145 cells is the next count
// above: different (`digest_cells_changed`).
#[test]
fn gate_e1_failure_frames_exactly_250_per_mille_is_similar_and_one_cell_more_is_different() {
    let previous = reference_png();
    let changed = |count: u32| {
        frame_png(WIDTH, HEIGHT, move |index| {
            if index < count { [4, 3, 0] } else { [0, 0, 0] }
        })
    };

    let at_limit = compare_failure_frames(&previous, &changed(CELLS_AT_250_MILLI))
        .expect("frames of one size compare");
    assert_eq!(at_limit.verdict, FailureFrameVerdict::Similar);
    assert!(at_limit.reason.is_none(), "{at_limit:?}");
    assert_eq!(at_limit.changed_cells_milli, Some(250));

    let above = compare_failure_frames(&previous, &changed(CELLS_AT_250_MILLI + 1))
        .expect("frames of one size compare");
    assert_eq!(above.verdict, FailureFrameVerdict::Different);
    assert_eq!(above.reason.map(Code::as_str), Some("digest_cells_changed"));
}

// policy-suspension.md "Error frames": a cell has changed when `|dR| + |dG| + |dB|` exceeds
// `CELL_DELTA = 6`. Every cell moved by 2 + 2 + 2 = 6 levels: no cell changed, similar; every
// cell moved by 3 + 2 + 2 = 7 levels: every cell changed, different.
#[test]
fn gate_e1_failure_frames_a_cell_difference_of_6_is_unchanged_and_7_is_changed() {
    let previous = reference_png();

    let by_six = compare_failure_frames(&previous, &frame_png(WIDTH, HEIGHT, |_| [2, 2, 2]))
        .expect("frames of one size compare");
    assert_eq!(by_six.verdict, FailureFrameVerdict::Similar);
    assert!(by_six.reason.is_none(), "{by_six:?}");
    assert_eq!(by_six.changed_cells_milli, Some(0));

    let by_seven = compare_failure_frames(&previous, &frame_png(WIDTH, HEIGHT, |_| [3, 2, 2]))
        .expect("frames of one size compare");
    assert_eq!(by_seven.verdict, FailureFrameVerdict::Different);
    assert_eq!(
        by_seven.reason.map(Code::as_str),
        Some("digest_cells_changed")
    );
    assert_eq!(by_seven.changed_cells_milli, Some(1000));
}

// policy-suspension.md "Error frames": frames whose sizes differ are different
// (`size_changed`), whatever they show.
#[test]
fn gate_e1_failure_frames_of_different_sizes_are_different() {
    let previous = reference_png();
    let taller = frame_png(WIDTH, HEIGHT + 2, |_| [0, 0, 0]);

    let comparison = compare_failure_frames(&previous, &taller).expect("both frames decode");
    assert_eq!(comparison.verdict, FailureFrameVerdict::Different);
    assert_eq!(comparison.reason.map(Code::as_str), Some("size_changed"));
}

// policy-suspension.md "Settlement of an accumulating failure": a frame that does not decode as
// PNG makes the comparison unavailable with the reason `png_invalid`, on either side.
#[test]
fn gate_e1_failure_frames_an_undecodable_png_is_png_invalid() {
    let valid = reference_png();
    // A frame artifact cut short: the PNG signature, its header and the start of the next chunk.
    let truncated = &valid[..40];

    let current = compare_failure_frames(&valid, truncated).expect_err("the current frame");
    assert_eq!(current.reason().as_str(), "png_invalid");
    let previous = compare_failure_frames(truncated, &valid).expect_err("the previous frame");
    assert_eq!(previous.reason().as_str(), "png_invalid");
}
