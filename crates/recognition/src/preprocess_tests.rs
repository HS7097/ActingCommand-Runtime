// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 A58, #428 CFL-1 (design CF §6 CFL-1, tests 1 to 6): every step kind's golden bytes
// on a small region, the HSV hue range that wraps through 0, Otsu's threshold, the gray step
// against the scene's gray, determinism, and the refusals of the chain syntax. The CFL-1 revision
// round (design CF v2 §5.3, §5.7, §8) adds the evidence `kept_milli` (before `invert`) and
// `inverted`, the hue 179, the Otsu split a remainder decides, and the region domain of 2^24
// pixels. The expected bytes were computed off-repo by an independent reference of the arithmetic
// in contracts/recognition-preprocess.md (the gray values with the sRGB luminance row in f32, each
// at least 0.05 away from a rounding tie). Regions are synthetic.

use super::*;
use image::{Rgb, RgbImage};

/// The colored region of test 1, 4x2, row-major.
const COLORED: [[u8; 3]; 8] = [
    [255, 255, 255],
    [240, 240, 232],
    [250, 30, 20],
    [20, 200, 40],
    [30, 60, 220],
    [128, 128, 128],
    [100, 200, 50],
    [0, 0, 0],
];

/// The neutral (R = G = B) levels of test 1, 4x2; their gray is the level itself.
const NEUTRAL: [u8; 8] = [0, 50, 51, 100, 125, 199, 200, 255];

fn region(width: u32, height: u32, pixels: &[[u8; 3]]) -> RgbImage {
    assert_eq!(pixels.len(), (width * height) as usize, "fixture size");
    RgbImage::from_fn(width, height, |x, y| Rgb(pixels[(y * width + x) as usize]))
}

fn neutral(width: u32, height: u32, levels: &[u8]) -> RgbImage {
    let pixels = levels.iter().map(|level| [*level; 3]).collect::<Vec<_>>();
    region(width, height, &pixels)
}

fn chain(steps: Vec<PreprocessStep>) -> Preprocess {
    Preprocess::new(steps).expect("a legal chain")
}

fn applied(steps: Vec<PreprocessStep>, pixels: &RgbImage) -> (Vec<u8>, PreprocessEvidence) {
    let output = chain(steps).apply(pixels).expect("applied");
    assert_eq!(output.image.dimensions(), pixels.dimensions());
    (output.image.into_raw(), output.evidence)
}

/// The evidence of a chain without `invert`.
fn evidence(kept_milli: u32, threshold_value: Option<u8>) -> PreprocessEvidence {
    PreprocessEvidence {
        kept_milli,
        inverted: false,
        threshold_value,
    }
}

/// The evidence of a chain that ends with `invert`: `kept_milli` is counted before it.
fn inverted_evidence(kept_milli: u32, threshold_value: Option<u8>) -> PreprocessEvidence {
    PreprocessEvidence {
        kept_milli,
        inverted: true,
        threshold_value,
    }
}

/// A neutral region whose values are `runs` of `(value, count)`, in row-major order.
fn neutral_runs(width: u32, height: u32, runs: &[(u8, usize)]) -> RgbImage {
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 3);
    for (value, count) in runs {
        pixels.resize(pixels.len() + count * 3, *value);
    }
    RgbImage::from_raw(width, height, pixels).expect("fixture size")
}

fn keep_colors(colors: &[[u8; 3]], tolerance: u8) -> PreprocessStep {
    PreprocessStep::KeepColors {
        colors: colors.to_vec(),
        tolerance,
    }
}

fn keep_range(space: ColorSpace, lower: [u8; 3], upper: [u8; 3]) -> PreprocessStep {
    PreprocessStep::KeepRange {
        space,
        lower,
        upper,
    }
}

fn contrast(low: u8, high: u8) -> PreprocessStep {
    PreprocessStep::Contrast { low, high }
}

fn threshold(value: u8) -> PreprocessStep {
    PreprocessStep::Threshold {
        value: ThresholdValue::Fixed(value),
    }
}

fn otsu() -> PreprocessStep {
    PreprocessStep::Threshold {
        value: ThresholdValue::Otsu,
    }
}

/// A step of `kind` whose members are all in range.
fn legal_step(kind: PreprocessKind) -> PreprocessStep {
    match kind {
        PreprocessKind::KeepColors => keep_colors(&[[255, 255, 255]], 40),
        PreprocessKind::KeepRange => keep_range(ColorSpace::Hsv, [0, 0, 190], [179, 60, 255]),
        PreprocessKind::Gray => PreprocessStep::Gray,
        PreprocessKind::Contrast => contrast(50, 200),
        PreprocessKind::Threshold => otsu(),
        PreprocessKind::Invert => PreprocessStep::Invert,
    }
}

fn refusal(steps: Vec<PreprocessStep>) -> PreprocessError {
    Preprocess::new(steps).expect_err("refused")
}

fn assert_refused(
    steps: Vec<PreprocessStep>,
    step: Option<usize>,
    member: Option<&str>,
    reason: PreprocessErrorReason,
) {
    let error = refusal(steps);
    assert_eq!(
        (error.step(), error.member(), error.reason()),
        (step, member, reason),
        "{error}"
    );
}

#[test]
fn each_step_kind_turns_a_small_region_into_its_golden_bytes() {
    let colored = region(4, 2, &COLORED);
    let levels = neutral(4, 2, &NEUTRAL);

    // keep_colors: the per-channel maximum difference, the tolerance inclusive.
    let colors = [[255, 255, 255], [120, 128, 136]];
    assert_eq!(
        applied(vec![keep_colors(&colors, 23)], &colored),
        (vec![255, 255, 0, 0, 0, 255, 0, 0], evidence(375, None))
    );
    assert_eq!(
        applied(vec![keep_colors(&colors, 22)], &colored),
        (vec![255, 0, 0, 0, 0, 255, 0, 0], evidence(250, None))
    );

    // keep_range in RGB and in HSV; invert flips the kept pixels.
    assert_eq!(
        applied(
            vec![keep_range(ColorSpace::Rgb, [200, 0, 0], [255, 100, 100])],
            &colored
        ),
        (vec![0, 0, 255, 0, 0, 0, 0, 0], evidence(125, None))
    );
    let light = keep_range(ColorSpace::Hsv, [0, 0, 190], [179, 60, 255]);
    assert_eq!(
        applied(vec![light.clone()], &colored),
        (vec![255, 255, 0, 0, 0, 0, 0, 0], evidence(250, None))
    );
    // `kept_milli` is counted before `invert`: the two kept pixels, 250.
    assert_eq!(
        applied(vec![light, PreprocessStep::Invert], &colored),
        (
            vec![0, 0, 255, 255, 255, 255, 255, 255],
            inverted_evidence(250, None)
        )
    );
    assert_eq!(
        COLORED.map(rgb_to_hsv),
        [
            [0, 0, 255],
            [30, 9, 240],
            [1, 235, 250],
            [63, 230, 200],
            [115, 220, 220],
            [0, 0, 128],
            [50, 191, 200],
            [0, 0, 0],
        ]
    );
    for (pixel, hsv) in [
        ([0, 255, 0], [60, 255, 255]),
        ([0, 0, 255], [120, 255, 255]),
        ([255, 255, 0], [30, 255, 255]),
        ([0, 255, 255], [90, 255, 255]),
        ([255, 0, 255], [150, 255, 255]),
        // 359.76 degrees, 179.88 on the scale, rounds to 180 and so to 0.
        ([255, 0, 1], [0, 255, 255]),
        // 358.12 degrees, 179.06 on the scale: floor(-225 / 510) = -1, and -1 mod 180 = 179; a
        // truncating division would give 0.
        ([255, 0, 8], [179, 255, 255]),
    ] {
        assert_eq!(rgb_to_hsv(pixel), hsv, "{pixel:?}");
    }

    // gray: the scene's gray. (100, 200, 50) is 168 here, where
    // floor((2126 R + 7152 G + 722 B) / 10000) would give 167.
    assert_eq!(
        applied(vec![PreprocessStep::Gray], &colored),
        (
            vec![255, 239, 76, 150, 65, 128, 168, 0],
            evidence(625, None)
        )
    );
    assert_eq!(
        applied(vec![PreprocessStep::Gray], &levels),
        (NEUTRAL.to_vec(), evidence(375, None))
    );

    // contrast, threshold and invert after gray.
    assert_eq!(
        applied(vec![PreprocessStep::Gray, contrast(50, 200)], &levels),
        (vec![0, 0, 2, 85, 128, 253, 255, 255], evidence(500, None))
    );
    assert_eq!(
        applied(vec![PreprocessStep::Gray, threshold(128)], &levels),
        (vec![0, 0, 0, 0, 0, 255, 255, 255], evidence(375, Some(128)))
    );
    assert_eq!(
        applied(vec![PreprocessStep::Gray, PreprocessStep::Invert], &levels),
        (
            vec![255, 205, 204, 155, 130, 56, 55, 0],
            inverted_evidence(375, None)
        )
    );
    assert_eq!(
        applied(
            vec![
                PreprocessStep::Gray,
                contrast(50, 200),
                threshold(128),
                PreprocessStep::Invert,
            ],
            &levels
        ),
        (
            vec![255, 255, 255, 255, 0, 0, 0, 0],
            inverted_evidence(500, Some(128))
        )
    );
}

#[test]
fn an_hsv_hue_range_whose_lower_is_above_its_upper_wraps_through_0() {
    let pixels = [
        [255, 0, 0],
        [255, 0, 40],
        [255, 40, 0],
        [255, 128, 0],
        [0, 0, 255],
        [255, 0, 128],
        [200, 180, 180],
        [128, 0, 0],
    ];
    assert_eq!(
        pixels.map(rgb_to_hsv),
        [
            [0, 255, 255],
            [175, 255, 255],
            [5, 255, 255],
            [15, 255, 255],
            [120, 255, 255],
            [165, 255, 255],
            [0, 26, 200],
            [0, 255, 128],
        ]
    );
    let pixels = region(4, 2, &pixels);

    // Hue 170 to 179 and 0 to 10: the reds on both sides of 0; the pale red fails saturation.
    assert_eq!(
        applied(
            vec![keep_range(ColorSpace::Hsv, [170, 100, 100], [10, 255, 255])],
            &pixels
        ),
        (vec![255, 255, 255, 0, 0, 0, 0, 255], evidence(500, None))
    );
    // Hue 11 to 169 keeps exactly the other saturated pixels.
    assert_eq!(
        applied(
            vec![keep_range(ColorSpace::Hsv, [11, 100, 100], [169, 255, 255])],
            &pixels
        ),
        (vec![0, 0, 0, 255, 255, 255, 0, 0], evidence(375, None))
    );
}

#[test]
fn otsu_splits_between_two_peaks_takes_the_smallest_tie_and_zeroes_a_flat_region() {
    let gray_otsu = || vec![PreprocessStep::Gray, otsu()];

    // Two peaks, 50 to 75 and 150 to 240: the split falls between them, at the smallest t of
    // the gap (75), so the threshold is 76.
    let peaks = neutral(
        4,
        4,
        &[
            50, 55, 60, 65, 70, 75, 150, 160, 170, 180, 190, 200, 210, 220, 230, 240,
        ],
    );
    let mut expected = vec![0; 6];
    expected.extend([255; 10]);
    assert_eq!(
        applied(gray_otsu(), &peaks),
        (expected, evidence(625, Some(76)))
    );

    // 0, 100, 200: the splits {0} | {100, 200} and {0, 100} | {200} have the same variance; the
    // smallest t (0) wins, so the threshold is 1.
    assert_eq!(
        applied(gray_otsu(), &neutral(3, 1, &[0, 100, 200])),
        (vec![0, 255, 255], evidence(666, Some(1)))
    );

    // One value everywhere: no split, the whole region is 0 and there is no threshold.
    assert_eq!(
        applied(gray_otsu(), &neutral(3, 2, &[77; 6])),
        (vec![0; 6], evidence(0, None))
    );
}

#[test]
fn otsu_lets_the_remainder_decide_between_equal_quotients() {
    // 0 0 0 1 1 1 2 4: the splits t = 1 (900 / 12) and t = 2 (529 / 7) have the same quotient,
    // 75; the remainders 0 / 12 < 4 / 7 decide for t = 2, so the threshold is 3. A comparison by
    // the quotient alone would keep t = 1 and give 2.
    assert_eq!(
        applied(
            vec![PreprocessStep::Gray, otsu()],
            &neutral(4, 2, &[0, 0, 0, 1, 1, 1, 2, 4])
        ),
        (vec![0, 0, 0, 0, 0, 0, 0, 255], evidence(125, Some(3)))
    );
}

#[test]
fn apply_accepts_a_720p_region_and_a_region_of_2_24_pixels() {
    let gray_otsu = || chain(vec![PreprocessStep::Gray, otsu()]);

    // 1280 x 720: 0, 128 and 255, 460 000, 1 600 and 460 000 times. A direct cross-multiplication
    // of two splits would need 129 bits here. The split {0} against {128, 255} wins: threshold 1.
    let output = gray_otsu()
        .apply(&neutral_runs(
            1280,
            720,
            &[(0, 460_000), (128, 1_600), (255, 460_000)],
        ))
        .expect("a 720p region");
    assert_eq!(output.evidence, evidence(500, Some(1)));
    let mut expected = vec![0_u8; 460_000];
    expected.resize(921_600, 255);
    assert!(
        output.image.as_raw()[..] == expected[..],
        "720p output bytes"
    );

    // 4096 x 4096 = 2^24 pixels, half 0 and half 255: a^2 reaches its widest, 108 bits. Every
    // split scores the same, so the smallest t, 0, wins: threshold 1.
    let half = 1_usize << 23;
    let output = gray_otsu()
        .apply(&neutral_runs(4096, 4096, &[(0, half), (255, half)]))
        .expect("a region of 2^24 pixels");
    assert_eq!(output.evidence, evidence(500, Some(1)));
    let mut expected = vec![0_u8; half];
    expected.resize(2 * half, 255);
    assert!(
        output.image.as_raw()[..] == expected[..],
        "2^24 output bytes"
    );
}

#[test]
fn the_domain_accepts_up_to_2_24_pixels_and_apply_refuses_a_larger_region() {
    assert_eq!(check_region_size(4096, 4096), Ok(16_777_216));
    assert_eq!(check_region_size(16_777_216, 1), Ok(16_777_216));
    assert_eq!(check_region_size(1, 1), Ok(1));
    for (width, height) in [(4097, 4096), (16_777_217, 1), (u32::MAX, u32::MAX)] {
        assert_eq!(
            check_region_size(width, height),
            Err(PreprocessApplyError::RegionTooLarge { width, height })
        );
    }
    assert_eq!(
        check_region_size(0, 5),
        Err(PreprocessApplyError::EmptyRegion {
            width: 0,
            height: 5
        })
    );

    // apply refuses before any step runs, for every shape of chain.
    let larger = RgbImage::new(4097, 4096);
    for steps in [
        vec![PreprocessStep::Gray, otsu()],
        vec![keep_colors(&[[255, 255, 255]], 40), PreprocessStep::Invert],
    ] {
        assert_eq!(
            chain(steps).apply(&larger).err(),
            Some(PreprocessApplyError::RegionTooLarge {
                width: 4097,
                height: 4096
            })
        );
    }
}

#[test]
fn a_gray_chain_gives_the_scene_gray_byte_for_byte() {
    // 4096 distinct colors: 16 levels on each channel.
    let (width, height) = (64_u32, 64_u32);
    let mut pixels = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            pixels.extend([
                ((x % 16) * 17) as u8,
                ((y % 16) * 17) as u8,
                ((x / 16 + 4 * (y / 16)) * 17) as u8,
            ]);
        }
    }
    let scene = Scene::from_rgb8(width, height, &pixels).expect("scene");
    let gray = chain(vec![PreprocessStep::Gray]);

    let whole = scene
        .preprocess_region(
            Rect {
                x: 0,
                y: 0,
                width: 64,
                height: 64,
            },
            &gray,
        )
        .expect("whole frame");
    assert_eq!(whole.image.as_raw(), scene.gray.as_raw());

    let part = scene
        .preprocess_region(
            Rect {
                x: 5,
                y: 7,
                width: 30,
                height: 20,
            },
            &gray,
        )
        .expect("region");
    assert_eq!(
        part.image.as_raw(),
        imageops::crop_imm(&scene.gray, 5, 7, 30, 20)
            .to_image()
            .as_raw()
    );
}

#[test]
fn the_same_region_and_chain_give_the_same_bytes_and_evidence_twice() {
    let pixels = RgbImage::from_fn(32, 24, |x, y| {
        Rgb([
            ((x * 37 + y * 11) % 256) as u8,
            ((x * 5 + y * 53 + 90) % 256) as u8,
            ((x * y * 3 + 17) % 256) as u8,
        ])
    });
    let chains = [
        vec![keep_colors(&[[255, 255, 255], [40, 200, 90]], 30)],
        vec![
            keep_range(ColorSpace::Rgb, [0, 100, 0], [200, 255, 120]),
            PreprocessStep::Invert,
        ],
        vec![
            keep_range(ColorSpace::Hsv, [150, 40, 60], [20, 255, 255]),
            PreprocessStep::Invert,
        ],
        vec![PreprocessStep::Gray],
        vec![PreprocessStep::Gray, contrast(30, 220), otsu()],
        vec![
            PreprocessStep::Gray,
            contrast(30, 220),
            threshold(140),
            PreprocessStep::Invert,
        ],
    ];
    for steps in chains {
        let chain = chain(steps);
        let first = chain.apply(&pixels).expect("first");
        let second = chain.apply(&pixels).expect("second");
        assert_eq!(first, second, "{chain:?}");
    }
}

#[test]
fn only_the_two_chain_shapes_are_accepted_and_a_refusal_names_the_step() {
    use PreprocessKind::{Contrast, Gray, Invert, KeepColors, KeepRange, Threshold};

    // Every sequence of one to four kinds, shortest first, each length in the order of `ALL`:
    // exactly the twelve of the two shapes are accepted.
    let legal = [
        vec![KeepColors],
        vec![KeepRange],
        vec![Gray],
        vec![KeepColors, Invert],
        vec![KeepRange, Invert],
        vec![Gray, Contrast],
        vec![Gray, Threshold],
        vec![Gray, Invert],
        vec![Gray, Contrast, Threshold],
        vec![Gray, Contrast, Invert],
        vec![Gray, Threshold, Invert],
        vec![Gray, Contrast, Threshold, Invert],
    ];
    let mut sequences = vec![vec![]];
    let mut accepted = Vec::new();
    for _ in 0..MAX_STEPS {
        sequences = sequences
            .iter()
            .flat_map(|sequence: &Vec<PreprocessKind>| {
                PreprocessKind::ALL.map(|kind| {
                    let mut longer = sequence.clone();
                    longer.push(kind);
                    longer
                })
            })
            .collect();
        for sequence in &sequences {
            if Preprocess::new(sequence.iter().copied().map(legal_step).collect()).is_ok() {
                accepted.push(sequence.clone());
            }
        }
    }
    assert_eq!(accepted, legal);

    // The step count.
    assert_refused(vec![], None, None, PreprocessErrorReason::NoSteps);
    let five = vec![
        PreprocessStep::Gray,
        contrast(50, 200),
        threshold(128),
        PreprocessStep::Invert,
        PreprocessStep::Invert,
    ];
    assert_eq!(refusal(five.clone()).pointer(), "/steps");
    assert_refused(
        five,
        None,
        None,
        PreprocessErrorReason::TooManySteps { count: 5 },
    );

    // The first step.
    assert_refused(
        vec![contrast(50, 200)],
        Some(0),
        Some("kind"),
        PreprocessErrorReason::FirstStepNotAllowed { kind: Contrast },
    );
    assert_refused(
        vec![PreprocessStep::Invert, PreprocessStep::Gray],
        Some(0),
        Some("kind"),
        PreprocessErrorReason::FirstStepNotAllowed { kind: Invert },
    );

    // The order.
    let reversed = vec![PreprocessStep::Gray, threshold(128), contrast(50, 200)];
    assert_eq!(refusal(reversed.clone()).pointer(), "/steps/2/kind");
    assert_refused(
        reversed,
        Some(2),
        Some("kind"),
        PreprocessErrorReason::StepOutOfOrder {
            kind: Contrast,
            after: Threshold,
        },
    );
    assert_refused(
        vec![legal_step(KeepColors), contrast(50, 200)],
        Some(1),
        Some("kind"),
        PreprocessErrorReason::StepOutOfOrder {
            kind: Contrast,
            after: KeepColors,
        },
    );
    assert_refused(
        vec![legal_step(KeepRange), PreprocessStep::Gray],
        Some(1),
        Some("kind"),
        PreprocessErrorReason::StepOutOfOrder {
            kind: Gray,
            after: KeepRange,
        },
    );
    assert_refused(
        vec![legal_step(KeepColors), legal_step(KeepRange)],
        Some(1),
        Some("kind"),
        PreprocessErrorReason::StepOutOfOrder {
            kind: KeepRange,
            after: KeepColors,
        },
    );

    // A repeated kind.
    assert_refused(
        vec![
            PreprocessStep::Gray,
            PreprocessStep::Invert,
            PreprocessStep::Invert,
        ],
        Some(2),
        Some("kind"),
        PreprocessErrorReason::StepRepeated { kind: Invert },
    );
    assert_refused(
        vec![PreprocessStep::Gray, PreprocessStep::Gray],
        Some(1),
        Some("kind"),
        PreprocessErrorReason::StepRepeated { kind: Gray },
    );

    // The declared names.
    for kind in PreprocessKind::ALL {
        assert_eq!(PreprocessKind::from_name(kind.name()), Some(kind));
    }
    assert_eq!(
        PreprocessKind::ALL.map(PreprocessKind::name),
        [
            "keep_colors",
            "keep_range",
            "gray",
            "contrast",
            "threshold",
            "invert"
        ]
    );
    assert_eq!(PreprocessKind::from_name("Gray"), None);
    assert_eq!(ColorSpace::from_name("rgb"), Some(ColorSpace::Rgb));
    assert_eq!(ColorSpace::from_name("hsv"), Some(ColorSpace::Hsv));
    assert_eq!(ColorSpace::from_name("HSV"), None);
}

#[test]
fn members_out_of_range_are_refused_at_their_step_and_member() {
    let distinct = (0..8_u8).map(|index| [index, 0, 0]).collect::<Vec<_>>();
    assert!(Preprocess::new(vec![keep_colors(&distinct, 0)]).is_ok());

    // keep_colors: 1 to 8 colors, none repeated.
    assert_refused(
        vec![keep_colors(&[], 10)],
        Some(0),
        Some("colors"),
        PreprocessErrorReason::ColorCount { count: 0 },
    );
    let mut nine = distinct.clone();
    nine.push([8, 0, 0]);
    assert_refused(
        vec![keep_colors(&nine, 10)],
        Some(0),
        Some("colors"),
        PreprocessErrorReason::ColorCount { count: 9 },
    );
    assert_refused(
        vec![keep_colors(&[[1, 2, 3], [4, 5, 6], [1, 2, 3]], 10)],
        Some(0),
        Some("colors"),
        PreprocessErrorReason::ColorRepeated { index: 2 },
    );

    // keep_range: an HSV hue is at most 179; lower <= upper, where only the HSV hue may wrap.
    assert!(
        Preprocess::new(vec![keep_range(
            ColorSpace::Hsv,
            [179, 0, 0],
            [0, 255, 255]
        )])
        .is_ok()
    );
    assert_refused(
        vec![keep_range(ColorSpace::Hsv, [180, 0, 0], [10, 255, 255])],
        Some(0),
        Some("lower"),
        PreprocessErrorReason::HueOutOfRange { value: 180 },
    );
    assert_refused(
        vec![keep_range(ColorSpace::Hsv, [0, 0, 0], [180, 255, 255])],
        Some(0),
        Some("upper"),
        PreprocessErrorReason::HueOutOfRange { value: 180 },
    );
    assert_refused(
        vec![keep_range(ColorSpace::Hsv, [0, 200, 0], [179, 100, 255])],
        Some(0),
        Some("upper"),
        PreprocessErrorReason::BoundsReversed { channel: 1 },
    );
    assert_refused(
        vec![keep_range(ColorSpace::Rgb, [200, 0, 0], [100, 255, 255])],
        Some(0),
        Some("upper"),
        PreprocessErrorReason::BoundsReversed { channel: 0 },
    );
    assert_refused(
        vec![keep_range(ColorSpace::Rgb, [0, 0, 101], [255, 255, 100])],
        Some(0),
        Some("upper"),
        PreprocessErrorReason::BoundsReversed { channel: 2 },
    );

    // contrast: low below high.
    let equal = vec![PreprocessStep::Gray, contrast(100, 100)];
    assert_eq!(refusal(equal.clone()).pointer(), "/steps/1/high");
    assert_refused(
        equal,
        Some(1),
        Some("high"),
        PreprocessErrorReason::ContrastBoundsReversed {
            low: 100,
            high: 100,
        },
    );
    assert_refused(
        vec![PreprocessStep::Gray, contrast(200, 50)],
        Some(1),
        Some("high"),
        PreprocessErrorReason::ContrastBoundsReversed { low: 200, high: 50 },
    );

    // threshold: a fixed value is 1 to 255.
    assert!(Preprocess::new(vec![PreprocessStep::Gray, threshold(1)]).is_ok());
    assert!(Preprocess::new(vec![PreprocessStep::Gray, threshold(255)]).is_ok());
    assert_refused(
        vec![PreprocessStep::Gray, contrast(10, 20), threshold(0)],
        Some(2),
        Some("value"),
        PreprocessErrorReason::ThresholdZero,
    );
}
