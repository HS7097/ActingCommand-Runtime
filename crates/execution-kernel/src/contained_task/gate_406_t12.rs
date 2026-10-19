// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #406 T12 (exec gate key 110; register §1.136; plan Q13 to Q16; brief T12 "The draw",
// requirements 2, 3 and 5): with the touch keys a runtime hands it, the kernel draws every box
// point as a centre hotspot. A single-region press draws `candidate_count` candidates and lands
// where the runtime's landing history chooses (`center_hotspot_least_recent_v1`; one candidate is
// `center_hotspot_normal_v1`); a drag draws one point per region (`center_hotspot_normal_v1`).
// Without touch keys the kernel draws uniformly as before (plan Q14). Every expected point comes
// from `Reference` below, "The draw" written again from the brief's text, never from the kernel.

use super::*;
use actingcommand_contract::{InputSamplingAlgorithm, InputSamplingParameters};
use serde_json::json;

const RESOLUTION: Resolution = Resolution {
    width: 1_000,
    height: 1_000,
};

// The default touch keys of plan Q13 (sigma 250 ‰, 8 redraws, 4 candidates).
const DEFAULT_TOUCH: TouchSampling = TouchSampling {
    sigma_ratio_milli: 250,
    max_redraws: 8,
    candidate_count: 4,
};

// "The draw" of brief T12, written from its text alone: xorshift64 from the normalized seed;
// `gaussian_milli` sums twelve 24-bit uniforms; `hotspot_point` redraws a point outside the box
// at most `max_redraws` times and then takes the centre; all arithmetic in i128, truncating.
struct Reference {
    state: u64,
}

impl Reference {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x9e37_79b9_7f4a_7c15
            } else {
                seed
            },
        }
    }

    fn next_output(&mut self) -> u64 {
        let mut value = self.state;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.state = value;
        value
    }

    fn gaussian_milli(&mut self) -> i128 {
        let mut sum: i128 = 0;
        for _ in 0..12 {
            sum += i128::from(self.next_output() >> 40);
        }
        ((2 * sum - 12 * ((1_i128 << 24) - 1)) * 1000) / (2 * (1_i128 << 24))
    }

    // The first attempt's point, without the box test (test 3 looks for one outside the box).
    fn first_attempt(
        &mut self,
        (x, y, width, height): (i32, i32, i32, i32),
        sigma: u32,
    ) -> (i128, i128) {
        let (cx, cy) = centre_of((x, y, width, height));
        let zx = self.gaussian_milli();
        let zy = self.gaussian_milli();
        (
            cx + zx * i128::from(sigma) * i128::from(width) / 1_000_000,
            cy + zy * i128::from(sigma) * i128::from(height) / 1_000_000,
        )
    }

    fn hotspot_point(
        &mut self,
        (x, y, width, height): (i32, i32, i32, i32),
        sigma: u32,
        max_redraws: u32,
    ) -> (i32, i32) {
        let (cx, cy) = centre_of((x, y, width, height));
        let (left, top) = (i128::from(x), i128::from(y));
        let (right, bottom) = (left + i128::from(width), top + i128::from(height));
        for _ in 0..=max_redraws {
            let zx = self.gaussian_milli();
            let zy = self.gaussian_milli();
            let px = cx + zx * i128::from(sigma) * i128::from(width) / 1_000_000;
            let py = cy + zy * i128::from(sigma) * i128::from(height) / 1_000_000;
            if (left..right).contains(&px) && (top..bottom).contains(&py) {
                return point_of(px, py);
            }
        }
        point_of(cx, cy)
    }

    // xorshift64_uniform_rect_v1: x + next mod width, then y + next mod height.
    fn uniform_point(&mut self, (x, y, width, height): (i32, i32, i32, i32)) -> (i32, i32) {
        let px = i64::from(x) + (self.next_output() % u64::try_from(width).expect("width")) as i64;
        let py =
            i64::from(y) + (self.next_output() % u64::try_from(height).expect("height")) as i64;
        (
            i32::try_from(px).expect("x on the screen"),
            i32::try_from(py).expect("y on the screen"),
        )
    }
}

fn centre_of((x, y, width, height): (i32, i32, i32, i32)) -> (i128, i128) {
    (
        i128::from(x) + i128::from(width) / 2,
        i128::from(y) + i128::from(height) / 2,
    )
}

fn point_of(x: i128, y: i128) -> (i32, i32) {
    (
        i32::try_from(x).expect("x on the screen"),
        i32::try_from(y).expect("y on the screen"),
    )
}

// Well-mixed action seeds, as the host's SHA-256 derivation gives (SplitMix64 of the index).
fn mixed_seed(index: u64) -> u64 {
    let mut value = index.wrapping_add(1).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn region(x: i32, y: i32, width: i32, height: i32) -> InputSamplingRegion {
    InputSamplingRegion::new(x, y, width, height).expect("sampling region")
}

fn click(value: serde_json::Value) -> TaskClick {
    serde_json::from_value(value).expect("task click")
}

fn rect_click((x, y, width, height): (i32, i32, i32, i32)) -> TaskClick {
    click(json!({"kind": "rect", "x": x, "y": y, "width": width, "height": height}))
}

// A runtime that only draws: its action seed, its touch keys and its landing history of the box
// are scripted, and it keeps every choice the kernel asks of it. It never captures, inputs or
// records.
struct DrawRuntime {
    seed: Option<u64>,
    touch: Option<TouchSampling>,
    history: BTreeMap<(i32, i32), u64>,
    asked: Vec<(InputSamplingRegion, Vec<(i32, i32)>)>,
}

impl DrawRuntime {
    fn new(seed: u64, touch: Option<TouchSampling>) -> Self {
        Self {
            seed: Some(seed),
            touch,
            history: BTreeMap::new(),
            asked: Vec::new(),
        }
    }
}

impl ContainedTaskRuntime for DrawRuntime {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        Err("a draw captures nothing")
    }

    fn action_seed(
        &mut self,
        _step_index: u32,
        _operation_label: &str,
    ) -> Result<Option<u64>, Self::Error> {
        Ok(self.seed)
    }

    fn touch_sampling(&mut self) -> Result<Option<TouchSampling>, Self::Error> {
        Ok(self.touch)
    }

    fn choose_touch_candidate(
        &mut self,
        region: &InputSamplingRegion,
        candidates: &[(i32, i32)],
    ) -> Result<usize, Self::Error> {
        self.asked.push((*region, candidates.to_vec()));
        Ok(least_recent_candidate(candidates, |point| {
            self.history.get(&point).copied()
        }))
    }

    fn input(
        &mut self,
        _action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        Err("a draw inputs nothing")
    }

    fn record(&mut self, _trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        Err("a draw records nothing")
    }
}

// The input of `press` as a step of `runtime` draws it: the step's draw from the runtime, the
// runtime's choice for a press with several candidates.
fn drawn(press: &TaskClick, runtime: &mut DrawRuntime) -> (InputAction, InputSamplingEvidence) {
    let draw = match step_point_draw(runtime, 0, "press") {
        Ok(draw) => draw,
        Err(error) => panic!("the step's draw: {error:?}"),
    };
    match press.draw_input::<&'static str>(
        &RESOLUTION,
        None,
        draw,
        &mut runtime_touch_choice(runtime),
    ) {
        Ok((action, Some(sampling))) => (action, sampling),
        Ok((action, None)) => panic!("a seeded draw records its sampling: {action:?}"),
        Err(error) => panic!("the press is drawn: {error:?}"),
    }
}

fn tap_point(action: &InputAction) -> (i32, i32) {
    match action {
        InputAction::Tap { x, y } | InputAction::LongTap { x, y, .. } => (*x, *y),
        other => panic!("a press, got {other:?}"),
    }
}

fn parameters(
    sigma_ratio_milli: u32,
    max_redraws: u32,
    candidates: Option<(u32, u32)>,
) -> InputSamplingParameters {
    InputSamplingParameters {
        sigma_ratio_milli,
        max_redraws,
        candidate_count: candidates.map(|(count, _)| count),
        chosen_candidate: candidates.map(|(_, chosen)| chosen),
    }
}

// Requirement 2 and "The draw": for fixed seeds and boxes, a single-region press with one
// candidate lands on the reference's hotspot point, a drag's two points are the reference's two
// successive points, and a press with three candidates offers the runtime the reference's three
// successive points (an empty history takes the first).
#[test]
fn t12_the_hotspot_draw_matches_the_v1_reference() {
    let boxes = [
        (10, 20, 8, 6),
        (100, 50, 200, 80),
        (0, 0, 1_000, 600),
        (640, 480, 31, 17),
    ];
    let one = TouchSampling {
        candidate_count: 1,
        ..DEFAULT_TOUCH
    };
    let three = TouchSampling {
        candidate_count: 3,
        ..DEFAULT_TOUCH
    };
    for seed in [0, 1, 77, 0x0123_4567_89ab_cdef, u64::MAX, mixed_seed(5)] {
        for rect in boxes {
            let (action, _) = drawn(&rect_click(rect), &mut DrawRuntime::new(seed, Some(one)));
            let expected = Reference::new(seed).hotspot_point(rect, 250, 8);
            assert_eq!(tap_point(&action), expected, "seed {seed} box {rect:?}");

            let mut runtime = DrawRuntime::new(seed, Some(three));
            let (action, _) = drawn(&rect_click(rect), &mut runtime);
            let mut reference = Reference::new(seed);
            let candidates = (0..3)
                .map(|_| reference.hotspot_point(rect, 250, 8))
                .collect::<Vec<_>>();
            assert_eq!(
                runtime.asked,
                vec![(region(rect.0, rect.1, rect.2, rect.3), candidates.clone())],
                "seed {seed} box {rect:?}"
            );
            assert_eq!(
                tap_point(&action),
                candidates[0],
                "seed {seed} box {rect:?}"
            );
        }
        let (from, to) = (boxes[1], boxes[3]);
        let drag = click(json!({
            "kind": "drag",
            "from_rect": {"x": from.0, "y": from.1, "width": from.2, "height": from.3},
            "to_rect": {"x": to.0, "y": to.1, "width": to.2, "height": to.3},
            "duration_ms": 400
        }));
        let (action, _) = drawn(&drag, &mut DrawRuntime::new(seed, Some(DEFAULT_TOUCH)));
        let mut reference = Reference::new(seed);
        let start = reference.hotspot_point(from, 250, 8);
        let end = reference.hotspot_point(to, 250, 8);
        assert_eq!(
            action,
            InputAction::Swipe {
                x1: start.0,
                y1: start.1,
                x2: end.0,
                y2: end.1,
                duration_ms: 400
            },
            "seed {seed}"
        );
    }
}

// Requirement 2 (Alice 2026-10-13, 「默认都是中心热点」): 10 000 draws from a fixed sequence of
// seeds in a 200 × 80 box with sigma 250 all land in the box, their mean lies within 1 % of the
// box's size of its centre, and the points per pixel in the box's middle fifth (in each axis) are
// at least four times those in its outer tenth (within a tenth of the size of an edge).
#[test]
fn t12_hotspot_draws_stay_in_the_box_and_thin_out_towards_the_edge() {
    const DRAWS: u64 = 10_000;
    let (x, y, width, height) = (100, 50, 200, 80);
    let press = rect_click((x, y, width, height));
    let one = TouchSampling {
        candidate_count: 1,
        ..DEFAULT_TOUCH
    };
    let (mut sum_x, mut sum_y) = (0_i64, 0_i64);
    let (mut middle, mut outer) = (0_i64, 0_i64);
    for index in 0..DRAWS {
        let (action, _) = drawn(&press, &mut DrawRuntime::new(mixed_seed(index), Some(one)));
        let (px, py) = tap_point(&action);
        assert!(
            (x..x + width).contains(&px) && (y..y + height).contains(&py),
            "draw {index}: ({px}, {py}) outside the box"
        );
        sum_x += i64::from(px);
        sum_y += i64::from(py);
        let (dx, dy) = (px - x, py - y);
        if (2 * width / 5..3 * width / 5).contains(&dx)
            && (2 * height / 5..3 * height / 5).contains(&dy)
        {
            middle += 1;
        }
        if dx < width / 10
            || dx >= width - width / 10
            || dy < height / 10
            || dy >= height - height / 10
        {
            outer += 1;
        }
    }
    let draws = i64::try_from(DRAWS).expect("draw count");
    // The centre (x + width / 2, y + height / 2) = (200, 90); 1 % of the box is 2 and 0.8 pixels.
    assert!(
        (sum_x - 200 * draws).abs() * 100 <= i64::from(width) * draws,
        "mean x {} not within 1 % of the width of 200",
        sum_x as f64 / draws as f64
    );
    assert!(
        (sum_y - 90 * draws).abs() * 100 <= i64::from(height) * draws,
        "mean y {} not within 1 % of the height of 90",
        sum_y as f64 / draws as f64
    );
    // The middle fifth is 40 × 16 = 640 pixels; the outer tenth is the box less its inner
    // 160 × 64, 16 000 − 10 240 = 5 760 pixels.
    let (middle_area, outer_area) = (640_i64, 5_760_i64);
    assert!(
        middle * outer_area >= 4 * outer * middle_area,
        "middle {middle} draws on {middle_area} px, outer {outer} draws on {outer_area} px"
    );
}

// "The draw", `hotspot_point`: with `max_redraws` 0 a draw whose one attempt falls outside the
// box takes the box's centre, `(x + width / 2, y + height / 2)`, and the evidence names the
// hotspot with its two parameters.
#[test]
fn t12_a_draw_that_never_lands_in_the_box_takes_the_centre() {
    let rect = (100, 50, 200, 80);
    let touch = TouchSampling {
        sigma_ratio_milli: 1_000,
        max_redraws: 0,
        candidate_count: 1,
    };
    let seed = (0..64)
        .map(mixed_seed)
        .find(|&seed| {
            let (px, py) = Reference::new(seed).first_attempt(rect, 1_000);
            let inside = (100..300).contains(&px) && (50..130).contains(&py);
            !inside
        })
        .expect("a seed among the first 64 whose first attempt falls outside the box");
    let (action, sampling) = drawn(&rect_click(rect), &mut DrawRuntime::new(seed, Some(touch)));
    assert_eq!(action, InputAction::Tap { x: 200, y: 90 });
    assert_eq!(
        sampling.algorithm(),
        InputSamplingAlgorithm::CenterHotspotNormalV1
    );
    assert_eq!(sampling.parameters(), Some(&parameters(1_000, 0, None)));
}

// Requirement 2 and plan Q16 (Alice 2026-10-14, 「我们选择距离上一次被选择的点击点最久的一个就行」):
// a press lands on its first candidate whose point the box's history does not hold; when every
// point was used, on the one used longest ago, ties to the lowest index; the evidence names the
// chosen candidate. The runtime keeps the scripted history and chooses by the kernel's rule.
#[test]
fn t12_least_recent_prefers_an_unused_candidate_then_the_oldest_use() {
    let rect = (200, 150, 400, 300);
    let seed = mixed_seed(17);
    let mut reference = Reference::new(seed);
    let candidates = (0..4)
        .map(|_| reference.hotspot_point(rect, 250, 8))
        .collect::<Vec<_>>();
    let distinct = candidates.iter().collect::<BTreeSet<_>>();
    assert_eq!(
        distinct.len(),
        4,
        "four distinct candidates: {candidates:?}"
    );
    for (case, uses, chosen) in [
        ("an empty history", [None, None, None, None], 0),
        ("the third unused", [Some(5), Some(2), None, Some(1)], 2),
        ("every point used", [Some(5), Some(2), Some(7), Some(3)], 1),
        ("two oldest alike", [Some(4), Some(2), Some(2), Some(9)], 1),
    ] {
        let mut runtime = DrawRuntime::new(seed, Some(DEFAULT_TOUCH));
        for (point, used) in candidates.iter().zip(uses) {
            if let Some(used) = used {
                runtime.history.insert(*point, used);
            }
        }
        let (action, sampling) = drawn(&rect_click(rect), &mut runtime);
        assert_eq!(runtime.asked.len(), 1, "{case}");
        assert_eq!(tap_point(&action), candidates[chosen], "{case}");
        assert_eq!(
            sampling.algorithm(),
            InputSamplingAlgorithm::CenterHotspotLeastRecentV1,
            "{case}"
        );
        assert_eq!(
            sampling.parameters(),
            Some(&parameters(
                250,
                8,
                Some((4, u32::try_from(chosen).expect("index")))
            )),
            "{case}"
        );
    }
}

// Requirement 5: a 1 × 1 box whose point the history holds draws four equal candidates and
// presses its point, without an error, as the rule chooses (every candidate used alike: the
// first).
#[test]
fn t12_a_one_point_box_presses_its_point() {
    for kind in ["rect", "long_press"] {
        let press = click(json!({
            "kind": kind, "x": 7, "y": 9, "width": 1, "height": 1, "duration_ms": 600
        }));
        let mut runtime = DrawRuntime::new(mixed_seed(3), Some(DEFAULT_TOUCH));
        runtime.history.insert((7, 9), 3);
        let (action, sampling) = drawn(&press, &mut runtime);
        assert_eq!(tap_point(&action), (7, 9), "{kind}");
        assert_eq!(
            runtime.asked,
            vec![(region(7, 9, 1, 1), vec![(7, 9); 4])],
            "{kind}"
        );
        assert_eq!(
            sampling.algorithm(),
            InputSamplingAlgorithm::CenterHotspotLeastRecentV1,
            "{kind}"
        );
        assert_eq!(
            sampling.parameters(),
            Some(&parameters(250, 8, Some((4, 0)))),
            "{kind}"
        );
        assert_eq!(sampling.source_regions(), [region(7, 9, 1, 1)], "{kind}");
    }
}

// Requirement 3 and plan Q15: a tap and a long press record the least-recent draw with all four
// parameters; a drag and the vertical-brake drag record the hotspot draw with the two spread
// parameters and their two regions; with `candidate_count` 1 a tap records the hotspot draw.
#[test]
fn t12_presses_and_drags_record_their_algorithm_and_parameters() {
    let touch = TouchSampling {
        sigma_ratio_milli: 300,
        max_redraws: 5,
        candidate_count: 3,
    };
    let seed = mixed_seed(29);
    let presses = [
        click(json!({"kind": "rect", "x": 100, "y": 100, "width": 50, "height": 40})),
        click(json!({
            "kind": "long_press", "x": 100, "y": 100, "width": 50, "height": 40, "duration_ms": 900
        })),
    ];
    for press in &presses {
        let (_, sampling) = drawn(press, &mut DrawRuntime::new(seed, Some(touch)));
        assert_eq!(
            sampling.algorithm(),
            InputSamplingAlgorithm::CenterHotspotLeastRecentV1,
            "{}",
            press.kind
        );
        assert_eq!(
            sampling.parameters(),
            Some(&parameters(300, 5, Some((3, 0)))),
            "{}",
            press.kind
        );
        assert_eq!(sampling.action_seed(), seed);
        assert_eq!(sampling.source_regions(), [region(100, 100, 50, 40)]);
    }

    let drags = [
        click(json!({
            "kind": "drag",
            "from_rect": {"x": 100, "y": 100, "width": 50, "height": 40},
            "to_rect": {"x": 500, "y": 300, "width": 60, "height": 30},
            "duration_ms": 400
        })),
        click(json!({
            "kind": "single_touch_drag_with_vertical_brake_v1",
            "from_rect": {"x": 100, "y": 100, "width": 50, "height": 40},
            "corner_rect": {"x": 500, "y": 300, "width": 60, "height": 30},
            "horizontal_duration_ms": SEGMENTED_SWIPE_HORIZONTAL_DURATION_MS,
            "corner_hold_ms": SEGMENTED_SWIPE_CORNER_HOLD_MS,
            "brake_distance_px": SEGMENTED_SWIPE_BRAKE_DISTANCE_PX,
            "brake_duration_ms": SEGMENTED_SWIPE_BRAKE_DURATION_MS
        })),
    ];
    for drag in &drags {
        let mut runtime = DrawRuntime::new(seed, Some(touch));
        let (_, sampling) = drawn(drag, &mut runtime);
        assert!(
            runtime.asked.is_empty(),
            "{}: a drag asks no choice",
            drag.kind
        );
        assert_eq!(
            sampling.algorithm(),
            InputSamplingAlgorithm::CenterHotspotNormalV1,
            "{}",
            drag.kind
        );
        assert_eq!(
            sampling.parameters(),
            Some(&parameters(300, 5, None)),
            "{}",
            drag.kind
        );
        assert_eq!(
            sampling.source_regions(),
            [region(100, 100, 50, 40), region(500, 300, 60, 30)],
            "{}",
            drag.kind
        );
    }

    let one = TouchSampling {
        candidate_count: 1,
        ..touch
    };
    let mut runtime = DrawRuntime::new(seed, Some(one));
    let (_, sampling) = drawn(&presses[0], &mut runtime);
    assert!(runtime.asked.is_empty(), "one candidate asks no choice");
    assert_eq!(
        sampling.algorithm(),
        InputSamplingAlgorithm::CenterHotspotNormalV1
    );
    assert_eq!(sampling.parameters(), Some(&parameters(300, 5, None)));
}

// Plan Q14: a runtime that hands no touch keys (the default `touch_sampling()`), even one with a
// landing history, gets the uniform draw it got before: `xorshift64_uniform_rect_v1` without
// parameters, the reference's uniform points, and no choice asked.
#[test]
fn t12_without_a_touch_sampling_the_kernel_keeps_the_uniform_draw() {
    // A seed and a landing history, and the trait's default `touch_sampling()`.
    struct SeedOnly {
        inner: DrawRuntime,
    }

    impl ContainedTaskRuntime for SeedOnly {
        type Error = &'static str;

        fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
            self.inner.capture()
        }

        fn action_seed(
            &mut self,
            step_index: u32,
            operation_label: &str,
        ) -> Result<Option<u64>, Self::Error> {
            self.inner.action_seed(step_index, operation_label)
        }

        fn choose_touch_candidate(
            &mut self,
            region: &InputSamplingRegion,
            candidates: &[(i32, i32)],
        ) -> Result<usize, Self::Error> {
            self.inner.choose_touch_candidate(region, candidates)
        }

        fn input(
            &mut self,
            action: InputAction,
            frame: Option<InputFrameContext>,
        ) -> Result<(), Self::Error> {
            self.inner.input(action, frame)
        }

        fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
            self.inner.record(trace)
        }
    }

    let seed = mixed_seed(41);
    let rect = (100, 100, 50, 40);
    let drag = click(json!({
        "kind": "drag",
        "from_rect": {"x": 100, "y": 100, "width": 50, "height": 40},
        "to_rect": {"x": 500, "y": 300, "width": 60, "height": 30},
        "duration_ms": 400
    }));
    for (press, expected) in [
        (rect_click(rect), {
            let point = Reference::new(seed).uniform_point(rect);
            InputAction::Tap {
                x: point.0,
                y: point.1,
            }
        }),
        (drag, {
            let mut reference = Reference::new(seed);
            let start = reference.uniform_point(rect);
            let end = reference.uniform_point((500, 300, 60, 30));
            InputAction::Swipe {
                x1: start.0,
                y1: start.1,
                x2: end.0,
                y2: end.1,
                duration_ms: 400,
            }
        }),
    ] {
        let mut runtime = SeedOnly {
            inner: DrawRuntime::new(seed, None),
        };
        let draw = match step_point_draw(&mut runtime, 0, "press") {
            Ok(draw) => draw,
            Err(error) => panic!("the step's draw: {error:?}"),
        };
        let (action, sampling) = match press.draw_input::<&'static str>(
            &RESOLUTION,
            None,
            draw,
            &mut runtime_touch_choice(&mut runtime),
        ) {
            Ok((action, Some(sampling))) => (action, sampling),
            Ok((action, None)) => panic!("a seeded draw records its sampling: {action:?}"),
            Err(error) => panic!("the press is drawn: {error:?}"),
        };
        assert_eq!(action, expected, "{}", press.kind);
        assert_eq!(
            sampling.algorithm(),
            InputSamplingAlgorithm::Xorshift64UniformRectV1,
            "{}",
            press.kind
        );
        assert_eq!(sampling.parameters(), None, "{}", press.kind);
        assert!(runtime.inner.asked.is_empty(), "{}", press.kind);
    }
}
