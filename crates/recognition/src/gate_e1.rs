// SPDX-License-Identifier: AGPL-3.0-only

// Test plan v2.2 E-1 G8 (exec key 17): `color_digest.v1` as `contracts/color-digest.md` defines
// it: the grid size rule, the quantization and the encoding, the error table and the golden
// values. Requirement basis: `rd-1009/tests/req-basis-el.md` section E-1, row "G8 color_digest".
// Every expected value below is the contract's; synthetic frames only.

use crate::color_digest::{
    ColorDigest, ColorDigestAlgorithm, ColorDigestError, ColorDigestGrid, cell_span, distance,
};
use crate::{Rect, Scene};

// The six codes of the contract's "Errors" table, spelled as the contract spells them.
const ALGORITHM_UNKNOWN: &str = "color_digest_algorithm_unknown";
const GRID_INVALID: &str = "color_digest_grid_invalid";
const REGION_INVALID: &str = "color_digest_region_invalid";
const CELLS_INVALID: &str = "color_digest_cells_invalid";
const EXCLUDE_CELLS_INVALID: &str = "color_digest_exclude_cells_invalid";
const GRID_MISMATCH: &str = "color_digest_grid_mismatch";

// The contract's golden digest and the observed digest it is compared with.
const GOLDEN_DIGEST: &str = "190c06090a0a";
const GOLDEN_OBSERVED: &str = "1a0c06090a0d";

fn code<T: std::fmt::Debug>(result: Result<T, ColorDigestError>) -> &'static str {
    result.expect_err("the input is refused").code().as_str()
}

fn grid(columns: u32, rows: u32) -> ColorDigestGrid {
    ColorDigestGrid::new(columns, rows).expect("a grid inside 1..=32 per axis")
}

fn rect(x: i32, y: i32, width: i32, height: i32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn scene(width: u32, height: u32, pixels: &[[u8; 3]]) -> Scene {
    Scene::from_rgb8(width, height, pixels.as_flattened()).expect("RGB8 scene")
}

// The frame of the contract's "Golden values": a 4x2 ROI at (0, 0); the left 2x2 cell is all
// (200, 100, 50), the right 2x2 cell holds (0, 0, 0), (255, 255, 255), (10, 20, 30) and
// (40, 50, 60).
fn golden_scene() -> Scene {
    let left = [200, 100, 50];
    scene(
        4,
        2,
        &[
            left,
            left,
            [0, 0, 0],
            [255, 255, 255],
            left,
            left,
            [10, 20, 30],
            [40, 50, 60],
        ],
    )
}

fn golden_digest() -> ColorDigest {
    ColorDigest::compute(&golden_scene(), rect(0, 0, 4, 2), grid(2, 1)).expect("golden digest")
}

// contracts/color-digest.md, "Golden values": the cells [25, 12, 6] and [9, 10, 10], the
// digest `190c06090a0a`; against `1a0c06090a0d` with no cell excluded `d = (1, 3)`,
// `mean_milli = 2000`, `max_cell = 3`, `worst_cell = 1`; a width of 10 split into 3 columns
// gives [0, 3), [3, 6) and [6, 10).
#[test]
fn gate_e1_color_digest_golden_values() {
    let expected = golden_digest();
    assert_eq!(expected.cells(), &[[25, 12, 6], [9, 10, 10]]);
    assert_eq!(expected.to_hex(), GOLDEN_DIGEST);

    let observed =
        ColorDigest::from_hex(grid(2, 1), GOLDEN_OBSERVED).expect("the observed digest parses");
    let measured = distance(&expected, &observed, &[]).expect("same grid, no exclusion");
    assert_eq!(measured.active_cells, 2);
    assert_eq!(measured.mean_milli, 2000);
    assert_eq!(measured.max_cell, 3);
    assert_eq!(measured.worst_cell, 1);

    let spans = (0..3)
        .map(|column| cell_span(0, 10, 3, column).expect("3 columns fit 10 pixels"))
        .collect::<Vec<_>>();
    assert_eq!(spans, [(0, 3), (3, 6), (6, 10)]);
}

// contracts/color-digest.md, "Input and geometry": `1 <= C <= min(32, width)` and
// `1 <= R <= min(32, height)`; "Errors": `color_digest_grid_invalid` when `C` or `R` is
// outside `1..=32` or exceeds the ROI width or height.
#[test]
fn gate_e1_color_digest_grid_is_one_to_min_32_and_the_roi_per_axis() {
    for (columns, rows) in [(1, 1), (32, 32), (32, 1), (1, 32)] {
        ColorDigestGrid::new(columns, rows)
            .unwrap_or_else(|error| panic!("{columns}x{rows} is a valid grid: {error}"));
    }
    for (columns, rows) in [(0, 1), (1, 0), (33, 1), (1, 33)] {
        assert_eq!(
            code(ColorDigestGrid::new(columns, rows)),
            GRID_INVALID,
            "{columns}x{rows}"
        );
    }

    // A 40x20 ROI takes up to min(32, 40) = 32 columns and min(32, 20) = 20 rows.
    let frame = scene(40, 20, &[[90, 120, 150]; 800]);
    let wide = rect(0, 0, 40, 20);
    ColorDigest::compute(&frame, wide, grid(32, 20)).expect("32x20 fits a 40x20 ROI");
    assert_eq!(
        code(ColorDigest::compute(&frame, wide, grid(32, 21))),
        GRID_INVALID
    );
    // A 4x2 ROI takes up to 4 columns and 2 rows.
    let small = rect(0, 0, 4, 2);
    ColorDigest::compute(&frame, small, grid(4, 2)).expect("4x2 fits a 4x2 ROI");
    assert_eq!(
        code(ColorDigest::compute(&frame, small, grid(5, 2))),
        GRID_INVALID
    );
    assert_eq!(
        code(ColorDigest::compute(&frame, small, grid(4, 3))),
        GRID_INVALID
    );
}

// contracts/color-digest.md, "Cells and quantization": cell (c, r) covers
// [x + floor(c * width / C), x + floor((c + 1) * width / C)) and the same for rows, and
// `q_k = floor(S_k / (8 * n))`; "Encoding": cell `i = r * C + c` contributes `[q_R, q_G, q_B]`,
// written as lowercase hexadecimal, two digits per byte.
#[test]
fn gate_e1_color_digest_quantizes_the_roi_cells_and_encodes_them_row_major() {
    // A 5x3 frame; the ROI is the 4x2 rectangle at (1, 1), on a 2x2 grid of 2x1-pixel cells.
    // Row 0 and column 0 lie outside the ROI and must not count.
    let outside = [250, 0, 0];
    let frame = scene(
        5,
        3,
        &[
            outside,
            outside,
            outside,
            outside,
            outside,
            outside,
            [15, 16, 255],
            [0, 0, 0],
            [255, 255, 255],
            [255, 255, 255],
            outside,
            [7, 8, 9],
            [8, 8, 8],
            [100, 160, 208],
            [108, 168, 216],
        ],
    );
    let digest = ColorDigest::compute(&frame, rect(1, 1, 4, 2), grid(2, 2)).expect("ROI digest");
    // n = 2, so q = floor(S / 16):
    // cell 0 (c 0, r 0): S = (15, 16, 255) -> [0, 1, 15]
    // cell 1 (c 1, r 0): S = (510, 510, 510) -> [31, 31, 31]
    // cell 2 (c 0, r 1): S = (15, 16, 17) -> [0, 1, 1]
    // cell 3 (c 1, r 1): S = (208, 328, 424) -> [13, 20, 26]
    assert_eq!(
        digest.cells(),
        &[[0, 1, 15], [31, 31, 31], [0, 1, 1], [13, 20, 26]]
    );
    let encoded = "00010f1f1f1f0001010d141a";
    assert_eq!(digest.to_hex(), encoded);

    let parsed = ColorDigest::from_hex(grid(2, 2), encoded).expect("the encoding parses");
    assert_eq!(parsed.cells(), digest.cells());
    assert_eq!(parsed.to_hex(), encoded);
}

// contracts/color-digest.md, "Errors": `color_digest_algorithm_unknown` when the algorithm name
// is not exactly `color_digest.v1`.
#[test]
fn gate_e1_color_digest_algorithm_name_is_exact() {
    assert_eq!(
        ColorDigestAlgorithm::parse("color_digest.v1").expect("the exact name"),
        ColorDigestAlgorithm::V1
    );
    for name in ["color_digest.v2", "color_digest", "Color_Digest.v1"] {
        assert_eq!(
            code(ColorDigestAlgorithm::parse(name)),
            ALGORITHM_UNKNOWN,
            "{name:?}"
        );
    }
}

// contracts/color-digest.md, "Input and geometry": the ROI has `x >= 0`, `y >= 0`,
// `width >= 1`, `height >= 1` and lies entirely inside the frame, never clipped; "Errors":
// `color_digest_region_invalid`.
#[test]
fn gate_e1_color_digest_region_outside_the_rule_is_refused() {
    let frame = scene(8, 4, &[[60, 60, 60]; 32]);
    for region in [
        rect(-1, 0, 2, 2),
        rect(0, -1, 2, 2),
        rect(0, 0, 0, 2),
        rect(0, 0, 2, -1),
        rect(7, 0, 2, 2),
        rect(0, 3, 2, 2),
    ] {
        assert_eq!(
            code(ColorDigest::compute(&frame, region, grid(1, 1))),
            REGION_INVALID,
            "{region:?}"
        );
    }
}

// contracts/color-digest.md, "Encoding": `6 * C * R` digits `0-9a-f`, every byte at most
// `0x1f`; a different length, an uppercase digit, any other character or a byte above `0x1f`
// is rejected, never normalized; "Errors": `color_digest_cells_invalid`.
#[test]
fn gate_e1_color_digest_cells_outside_the_encoding_are_refused() {
    ColorDigest::from_hex(grid(2, 1), GOLDEN_DIGEST).expect("the golden digest parses");
    for cells in [
        "190c06090a0",
        "190c06090a0a00",
        "190C06090a0a",
        "190c06090a0g",
        "200c06090a0a",
    ] {
        assert_eq!(
            code(ColorDigest::from_hex(grid(2, 1), cells)),
            CELLS_INVALID,
            "{cells:?}"
        );
    }
}

// contracts/color-digest.md, "Distance": `exclude_cells` is strictly ascending, every index
// is below `C * R`, and at least one cell stays active; "Errors":
// `color_digest_exclude_cells_invalid`.
#[test]
fn gate_e1_color_digest_exclude_cells_outside_the_rule_are_refused() {
    let expected = golden_digest();
    let observed =
        ColorDigest::from_hex(grid(2, 1), GOLDEN_OBSERVED).expect("the observed digest parses");
    for exclude in [&[1, 0][..], &[0, 0], &[2], &[0, 1]] {
        assert_eq!(
            code(distance(&expected, &observed, exclude)),
            EXCLUDE_CELLS_INVALID,
            "{exclude:?}"
        );
    }
}

// contracts/color-digest.md, "Distance": the expected and the observed digest use the same
// grid; "Errors": `color_digest_grid_mismatch`.
#[test]
fn gate_e1_color_digest_different_grids_are_refused() {
    let expected = golden_digest();
    // The same twelve digits read on a 1x2 grid instead of 2x1.
    let observed = ColorDigest::from_hex(grid(1, 2), GOLDEN_DIGEST).expect("a 1x2 digest parses");
    assert_eq!(code(distance(&expected, &observed, &[])), GRID_MISMATCH);
}
