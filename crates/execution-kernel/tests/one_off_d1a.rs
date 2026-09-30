// SPDX-License-Identifier: AGPL-3.0-only

//! One-off evidence for Workflow #308 D1a (C1), to be reverted. It checks the golden values
//! of `contracts/color-digest.md` against `color_digest.v1`, and compares the algorithm with
//! a naive per-pixel implementation on every template image of a published bundle, using
//! the grids authors will use (8x8, and 7x5, which splits most images unevenly).

use actingcommand_recognition::color_digest::{
    self, COLOR_DIGEST_V1, ColorDigest, ColorDigestAlgorithm, ColorDigestErrorCode,
    ColorDigestGrid, ColorDigestThresholds,
};
use actingcommand_recognition::{Rect, Scene};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Write};
use std::path::PathBuf;
use std::process::Command;

type Entries = BTreeMap<String, Vec<u8>>;

const ARCHIVE_URL: &str =
    "https://github.com/HS7097/ActingCommand/archive/536f048a3cac26ddbb0391895391f98967704cb0.zip";
const BUNDLE_SHA256: &str = "df524eac6290c4c0b435f3872a89e12dbe4f49e37555fa102bf96dd71b696af8";
const GRIDS: [(u32, u32); 2] = [(8, 8), (7, 5)];

fn emit(line: &str) {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "D1A-ONEOFF {}", line.replace('\n', " | "))
        .unwrap_or_else(|error| panic!("write stdout: {error}"));
    stdout
        .flush()
        .unwrap_or_else(|error| panic!("flush stdout: {error}"));
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn download_archive() -> Vec<u8> {
    let output = Command::new("curl")
        .args(["-sSfL", "--retry", "3", ARCHIVE_URL])
        .output()
        .unwrap_or_else(|error| panic!("start curl: {error}"));
    assert!(
        output.status.success(),
        "curl failed: {} {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn unzip(bytes: &[u8]) -> Entries {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .unwrap_or_else(|error| panic!("open zip: {error}"));
    let mut entries = Entries::new();
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .unwrap_or_else(|error| panic!("zip entry {index}: {error}"));
        if file.is_dir() {
            continue;
        }
        let name = file.name().to_owned();
        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .unwrap_or_else(|error| panic!("read {name}: {error}"));
        entries.insert(name, data);
    }
    entries
}

fn rect(x: u32, y: u32, width: u32, height: u32) -> Rect {
    Rect {
        x: x as i32,
        y: y as i32,
        width: width as i32,
        height: height as i32,
    }
}

fn grid(columns: u32, rows: u32) -> ColorDigestGrid {
    ColorDigestGrid::new(columns, rows).unwrap_or_else(|error| panic!("{error}"))
}

/// Pixel-major reference: every pixel finds its cell by scanning the cell boundaries, and a
/// cell's value is the floored mean floored again by 8.
fn naive_cells(
    rgb: &[u8],
    frame_width: u32,
    (x, y, width, height): (u32, u32, u32, u32),
    (columns, rows): (u32, u32),
) -> Vec<[u8; 3]> {
    let cells = (columns * rows) as usize;
    let mut sums = vec![[0_u64; 3]; cells];
    let mut counts = vec![0_u64; cells];
    let owner = |pixel: u32, start: u32, length: u32, parts: u32| {
        (0..parts)
            .find(|&part| {
                let low = start + part * length / parts;
                let high = start + (part + 1) * length / parts;
                (low..high).contains(&pixel)
            })
            .unwrap_or_else(|| panic!("pixel {pixel} has no cell"))
    };
    for pixel_y in y..y + height {
        let row = owner(pixel_y, y, height, rows);
        for pixel_x in x..x + width {
            let column = owner(pixel_x, x, width, columns);
            let cell = (row * columns + column) as usize;
            let offset = ((pixel_y * frame_width + pixel_x) * 3) as usize;
            for (sum, value) in sums[cell].iter_mut().zip(&rgb[offset..offset + 3]) {
                *sum += u64::from(*value);
            }
            counts[cell] += 1;
        }
    }
    sums.iter()
        .zip(&counts)
        .map(|(sum, &count)| {
            assert!(count > 0, "empty cell");
            sum.map(|channel| (channel / count / 8) as u8)
        })
        .collect()
}

fn naive_hex(cells: &[[u8; 3]]) -> String {
    cells
        .iter()
        .flatten()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `(active_cells, mean_milli, max_cell, worst_cell)` by direct enumeration.
fn naive_distance(
    expected: &[[u8; 3]],
    observed: &[[u8; 3]],
    exclude: &[u32],
) -> (u32, u32, u32, u32) {
    let active = (0..expected.len() as u32)
        .filter(|index| !exclude.contains(index))
        .collect::<Vec<_>>();
    let distances = active
        .iter()
        .map(|&index| {
            let (want, seen) = (expected[index as usize], observed[index as usize]);
            (0..3)
                .map(|channel| (i32::from(want[channel]) - i32::from(seen[channel])).unsigned_abs())
                .sum::<u32>()
        })
        .collect::<Vec<_>>();
    let total = distances.iter().map(|&value| u64::from(value)).sum::<u64>();
    let max_cell = *distances.iter().max().expect("active cell");
    let worst = distances
        .iter()
        .position(|&value| value == max_cell)
        .expect("worst cell");
    (
        active.len() as u32,
        (total * 1000 / active.len() as u64) as u32,
        max_cell,
        active[worst],
    )
}

fn contract_text() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/color-digest.md");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn check_golden() {
    let left = [200_u8, 100, 50];
    let right = [[0_u8, 0, 0], [255, 255, 255], [10, 20, 30], [40, 50, 60]];
    let mut pixels = Vec::new();
    for row in 0..2 {
        pixels.extend(left);
        pixels.extend(left);
        pixels.extend(right[row * 2]);
        pixels.extend(right[row * 2 + 1]);
    }
    let scene = Scene::from_rgb8(4, 2, &pixels).unwrap_or_else(|error| panic!("{error}"));
    let golden_grid = grid(2, 1);
    let expected = ColorDigest::compute(&scene, rect(0, 0, 4, 2), golden_grid)
        .unwrap_or_else(|error| panic!("{error}"));
    let observed = ColorDigest::from_hex(golden_grid, "1a0c06090a0d")
        .unwrap_or_else(|error| panic!("{error}"));
    let distance =
        color_digest::distance(&expected, &observed, &[]).unwrap_or_else(|error| panic!("{error}"));
    let spans = (0..3)
        .map(|index| {
            color_digest::cell_span(0, 10, 3, index).unwrap_or_else(|error| panic!("{error}"))
        })
        .collect::<Vec<_>>();
    let widths = spans
        .iter()
        .map(|(low, high)| high - low)
        .collect::<Vec<_>>();
    emit(&format!(
        "golden digest={} cells={:?} observed={} active_cells={} mean_milli={} max_cell={} worst_cell={} spans(w=10,C=3)={spans:?} widths={widths:?}",
        expected.to_hex(),
        expected.cells(),
        observed.to_hex(),
        distance.active_cells,
        distance.mean_milli,
        distance.max_cell,
        distance.worst_cell
    ));
    assert_eq!(expected.to_hex(), "190c06090a0a");
    assert_eq!(expected.cells(), &[[25, 12, 6], [9, 10, 10]]);
    assert_eq!(
        (
            distance.active_cells,
            distance.mean_milli,
            distance.max_cell,
            distance.worst_cell
        ),
        (2, 2000, 3, 1)
    );
    assert_eq!(widths, [3, 3, 4]);

    // The contract states the same values byte for byte.
    let contract = contract_text();
    let [left_cell, right_cell] = [expected.cells()[0], expected.cells()[1]];
    let statements = [
        format!("Digest: `{}`.", expected.to_hex()),
        format!(
            "The observed digest `{}` compared with it",
            observed.to_hex()
        ),
        format!(
            "cell `[{}, {}, {}]`",
            left_cell[0], left_cell[1], left_cell[2]
        ),
        format!(
            "cell `[{}, {}, {}]`",
            right_cell[0], right_cell[1], right_cell[2]
        ),
        "`d = (1, 3)`".to_owned(),
        format!("`mean_milli = {}`", distance.mean_milli),
        format!("`max_cell = {}`", distance.max_cell),
        format!("`worst_cell = {}`", distance.worst_cell),
        format!("`[{}, {})`", spans[0].0, spans[0].1),
        format!("`[{}, {})`", spans[1].0, spans[1].1),
        format!("`[{}, {})`", spans[2].0, spans[2].1),
        format!(
            "column widths {}, {} and {}",
            widths[0], widths[1], widths[2]
        ),
    ];
    for statement in &statements {
        assert!(
            contract.contains(statement.as_str()),
            "contract lacks {statement:?}"
        );
    }
    emit(&format!(
        "contract byte-identical statements={} ok",
        statements.len()
    ));
}

fn check_errors(sample: &Scene) {
    let (width, height) = (sample.width(), sample.height());
    let eight = grid(8, 8);
    let uneven = grid(7, 5);
    let full = rect(0, 0, width, height);
    let digest =
        ColorDigest::compute(sample, full, eight).unwrap_or_else(|error| panic!("{error}"));
    let cases = [
        (
            "grid changed without recomputing cells",
            ColorDigest::from_hex(uneven, &digest.to_hex()).map(|_| ()),
            ColorDigestErrorCode::CellsInvalid,
        ),
        (
            "cells pasted in uppercase",
            ColorDigest::from_hex(grid(2, 1), "190C06090A0A").map(|_| ()),
            ColorDigestErrorCode::CellsInvalid,
        ),
        (
            "expected and observed grids differ",
            ColorDigest::compute(sample, full, uneven)
                .and_then(|observed| color_digest::distance(&digest, &observed, &[]))
                .map(|_| ()),
            ColorDigestErrorCode::GridMismatch,
        ),
        (
            "region declared past the frame edge",
            ColorDigest::compute(sample, rect(1, 0, width, height), eight).map(|_| ()),
            ColorDigestErrorCode::RegionInvalid,
        ),
        (
            "grid finer than the region",
            ColorDigest::compute(sample, rect(0, 0, 6, height), eight).map(|_| ()),
            ColorDigestErrorCode::GridInvalid,
        ),
        (
            "exclude_cells not ascending",
            color_digest::distance(&digest, &digest, &[5, 2]).map(|_| ()),
            ColorDigestErrorCode::ExcludeCellsInvalid,
        ),
        (
            "unknown algorithm version",
            ColorDigestAlgorithm::parse("color_digest.v2").map(|_| ()),
            ColorDigestErrorCode::AlgorithmUnknown,
        ),
    ];
    for (name, result, code) in cases {
        let error = result.expect_err(name);
        emit(&format!("error case={name:?} -> {error}"));
        assert_eq!(error.code(), code, "{name}");
    }
    assert_eq!(
        ColorDigestAlgorithm::parse(COLOR_DIGEST_V1)
            .unwrap_or_else(|error| panic!("{error}"))
            .as_str(),
        COLOR_DIGEST_V1
    );
}

fn images() -> Vec<(String, Scene)> {
    let archive = unzip(&download_archive());
    let (bundle_name, bundle_bytes) = archive
        .iter()
        .find(|(path, _)| path.contains("/bundles/") && path.ends_with(".zip"))
        .unwrap_or_else(|| panic!("no bundle in the archive"));
    let bundle_sha256 = sha256_hex(bundle_bytes);
    assert_eq!(bundle_sha256, BUNDLE_SHA256, "{bundle_name}");
    let bundle = unzip(bundle_bytes);
    // Identical images shared by several packages are compared once, in path order.
    let mut seen = BTreeSet::new();
    let mut unique = BTreeMap::new();
    let mut packs = 0;
    for (path, bytes) in &bundle {
        if !path.ends_with(".zip") {
            continue;
        }
        packs += 1;
        for (name, data) in unzip(bytes) {
            let sha256 = sha256_hex(&data);
            if name.ends_with(".png") && seen.insert(sha256.clone()) {
                unique.insert((name, sha256), data);
            }
        }
    }
    emit(&format!(
        "bundle={bundle_sha256} packs={packs} unique_png={}",
        unique.len()
    ));
    unique
        .into_iter()
        .map(|((name, _), data)| {
            let scene = Scene::from_png(&data).unwrap_or_else(|error| panic!("{name}: {error}"));
            (name, scene)
        })
        .collect()
}

fn check_image(name: &str, scene: &Scene) -> usize {
    let (width, height) = (scene.width(), scene.height());
    let inner = (
        width / 5,
        height / 5,
        width - width / 5 - width / 7,
        height - height / 5 - height / 7,
    );
    let mut checks = 0;
    let mut summary = Vec::new();
    for roi in [(0, 0, width, height), inner] {
        for (columns, rows) in GRIDS {
            let digest =
                ColorDigest::compute(scene, rect(roi.0, roi.1, roi.2, roi.3), grid(columns, rows))
                    .unwrap_or_else(|error| panic!("{name} {roi:?} {columns}x{rows}: {error}"));
            let naive = naive_cells(scene.rgb8_pixels(), width, roi, (columns, rows));
            assert_eq!(
                digest.cells(),
                naive.as_slice(),
                "{name} {roi:?} {columns}x{rows}"
            );
            let hex = digest.to_hex();
            assert_eq!(hex, naive_hex(&naive), "{name} {roi:?} {columns}x{rows}");
            assert_eq!(hex.len() as u32, 6 * columns * rows);
            let parsed = ColorDigest::from_hex(grid(columns, rows), &hex)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(parsed, digest, "{name} round trip");
            let itself = color_digest::distance(&digest, &parsed, &[])
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(
                (itself.mean_milli, itself.max_cell, itself.worst_cell),
                (0, 0, 0)
            );
            checks += 1;
            summary.push(format!("{columns}x{rows}@{roi:?}"));
        }
    }
    emit(&format!(
        "image={name} size={width}x{height} match={checks} [{}]",
        summary.join(" ")
    ));
    checks
}

fn check_pair(left: &(String, Scene), right: &(String, Scene)) -> usize {
    let (width, height) = (left.1.width(), left.1.height());
    let mut checks = 0;
    for (columns, rows) in GRIDS {
        let cells = columns * rows;
        let masks = [
            Vec::new(),
            (0..cells)
                .filter(|index| index % 4 == 1)
                .collect::<Vec<_>>(),
        ];
        let digests = [&left.1, &right.1].map(|scene| {
            ColorDigest::compute(scene, rect(0, 0, width, height), grid(columns, rows))
                .unwrap_or_else(|error| panic!("{error}"))
        });
        for mask in &masks {
            let measured = color_digest::distance(&digests[0], &digests[1], mask)
                .unwrap_or_else(|error| panic!("{error}"));
            let naive = naive_distance(digests[0].cells(), digests[1].cells(), mask);
            let actual = (
                measured.active_cells,
                measured.mean_milli,
                measured.max_cell,
                measured.worst_cell,
            );
            assert_eq!(actual, naive, "{} vs {}", left.0, right.0);
            let at_limit = ColorDigestThresholds {
                max_mean_milli: measured.mean_milli,
                max_cell: Some(measured.max_cell),
            };
            assert!(measured.passes(at_limit));
            if measured.mean_milli > 0 {
                assert!(!measured.passes(ColorDigestThresholds {
                    max_mean_milli: measured.mean_milli - 1,
                    max_cell: None,
                }));
            }
            if measured.max_cell > 0 {
                assert!(!measured.passes(ColorDigestThresholds {
                    max_mean_milli: measured.mean_milli,
                    max_cell: Some(measured.max_cell - 1),
                }));
            }
            emit(&format!(
                "pair {} vs {} size={width}x{height} grid={columns}x{rows} excluded={} active_cells={} mean_milli={} max_cell={} worst_cell={} naive=equal",
                left.0,
                right.0,
                mask.len(),
                measured.active_cells,
                measured.mean_milli,
                measured.max_cell,
                measured.worst_cell
            ));
            checks += 1;
        }
    }
    checks
}

#[test]
fn one_off_d1a_color_digest_golden_and_real_images() {
    check_golden();
    let images = images();
    check_errors(&images[0].1);
    let digests = images
        .iter()
        .map(|(name, scene)| check_image(name, scene))
        .sum::<usize>();
    let mut distances = 0;
    for (index, left) in images.iter().enumerate() {
        for right in &images[index + 1..] {
            if (left.1.width(), left.1.height()) == (right.1.width(), right.1.height()) {
                distances += check_pair(left, right);
            }
        }
    }
    emit(&format!(
        "summary images={} digest_comparisons={digests} distance_comparisons={distances} all equal",
        images.len()
    ));
    assert!(digests > 0 && distances > 0);
}
