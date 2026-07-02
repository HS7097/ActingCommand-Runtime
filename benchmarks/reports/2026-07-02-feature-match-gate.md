# FeatureMatch Benchmark Gate Report · 2026-07-02

## Scope

This report closes the P6.5-A E pre-research gate for MAA-style FeatureMatch.

The gate checks whether ActingCommand-Runtime can proceed with a pure Rust FeatureMatch implementation before adding OCR/NN/CLI work.

## Resource freshness

Resource repositories were mirrored before this review:

| Repository | Commit |
| --- | --- |
| `ActingCommand-Resources-Arknights` | `2ab7ccd` |
| `ActingCommand-Resources-AzurLane` | `e778cc7c` |
| `ActingCommand-Resources-BlueArchive` | `7cf3bae` |

These repositories currently provide recognition packs, operation assets, and upstream-derived templates. They do not yet provide a curated FeatureMatch benchmark dataset with paired full frames, transformed views, ground-truth homography or click regions, and expected cross-resolution matches.

## Current Runtime recognition state

`actingcommand-recognition` currently depends on:

- `image`
- `imageproc`

The current recognition engine supports:

- normalized template matching;
- normalized correlation coefficient matching;
- color comparison;
- pack-level thresholding in `actingcommand-recognition-pack`.

No current Runtime source dependency or code path provides SIFT, ORB, AKAZE, RANSAC, OpenCV, or a feature-descriptor matcher.

## Gate result

Pure Rust FeatureMatch is not accepted at this gate.

Reasons:

- `imageproc` does not provide an ORB/SIFT/AKAZE FeatureMatch implementation in the current dependency surface.
- No reviewed pure Rust ORB implementation has been selected or benchmarked.
- The refreshed resource repositories do not yet contain the required real-frame benchmark corpus for cross-resolution FeatureMatch acceptance.
- Adding an unbenchmarked self-written ORB implementation here would be speculative and would expand scope beyond the benchmark gate.

## Decision

Route FeatureMatch to the R-class FFI decision lane.

The next implementation decision should compare:

- OpenCV FFI through `opencv-rust`;
- any MAA-compatible C API boundary if it can expose the needed recognition behavior cleanly;
- a separately reviewed pure Rust feature-descriptor crate only if a real-frame benchmark corpus is first added.

Until that decision is made, Runtime should not expose FeatureMatch as a passing recognition primitive.

## Size impact

This gate adds no production dependencies and no recognition hot-path code.

Current size impact: `0 MB`.

Future FFI candidates must record their size impact before adoption. OpenCV-style FFI is expected to be materially larger than the current Rust-only recognition dependency set.

## Deferred acceptance test

`feature_match_recognizes_across_resolutions` is intentionally not added in this unit because the pure Rust gate did not pass and no FeatureMatch backend is active.

That test should be added with the eventual FeatureMatch backend and must use a real-frame corpus with known expected matches across resolution or scale changes.
