// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #406 T3 (exec gate key 111; register §1.137; plan Q17, Q19; brief T3 requirements 1, 2
// and 6; Alice 2026-10-13 「…同时支持在特定方框里在一个拖动下拐很多弯的拖动方法」): the click
// `single_touch_path_drag_v1` names a box, its turns, its shortest segment, its duration and its
// corner hold, and the kernel generates the path from the step's action seed: `turns + 2` points
// in the box, each segment at least `min_segment_px` long (the box's corner farthest from the
// previous point when no draw is), the duration split by the segments' lengths, and no sampling
// evidence. Pack containment admits the click within its bounds in the task file (`box`) and
// compiled (`box_rect`), and the kernel's drive reader reads it as a segmented drag. Every
// expected value comes from the brief's text, written again below, never from the kernel; boxes
// and pages are neutral.

use super::tests::admission_deadline;
use super::*;
use crate::drive::parse_navigation_click;
use crate::{DriveNavigationClick, DriveNavigationGraph, DrivePathDrag};
use actingcommand_contract::{ResourceDeclarationIssue, ResourceDeclarationReason};
use actingcommand_pack_containment::source::{
    Bundle, OperationParser, declaration_file_requests, validate_navigation_declarations,
};
use actingcommand_recognition_pack::PackRect;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

// "The draw" of brief T12, written from its text alone (as in `gate_406_t12.rs`): xorshift64 from
// the normalized seed; `gaussian_milli` sums twelve 24-bit uniforms; `hotspot_point` redraws a
// point outside the box at most `max_redraws` times and then takes the centre; the uniform draw
// takes `x + next mod width`, then `y + next mod height`.
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

    fn hotspot_point(
        &mut self,
        (x, y, width, height): (i32, i32, i32, i32),
        sigma: u32,
        max_redraws: u32,
    ) -> (i32, i32) {
        let (cx, cy) = (
            i128::from(x) + i128::from(width) / 2,
            i128::from(y) + i128::from(height) / 2,
        );
        let (left, top) = (i128::from(x), i128::from(y));
        for _ in 0..=max_redraws {
            let zx = self.gaussian_milli();
            let zy = self.gaussian_milli();
            let px = cx + zx * i128::from(sigma) * i128::from(width) / 1_000_000;
            let py = cy + zy * i128::from(sigma) * i128::from(height) / 1_000_000;
            if (left..left + i128::from(width)).contains(&px)
                && (top..top + i128::from(height)).contains(&py)
            {
                return point_of(px, py);
            }
        }
        point_of(cx, cy)
    }

    fn uniform_point(&mut self, (x, y, width, height): (i32, i32, i32, i32)) -> (i32, i32) {
        let px =
            i128::from(x) + i128::from(self.next_output() % u64::try_from(width).expect("width"));
        let py =
            i128::from(y) + i128::from(self.next_output() % u64::try_from(height).expect("height"));
        point_of(px, py)
    }
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

fn squared(from: (i32, i32), to: (i32, i32)) -> i64 {
    let (dx, dy) = (i64::from(to.0 - from.0), i64::from(to.1 - from.1));
    dx * dx + dy * dy
}

// The path drag click of the compiled form, `box_rect` and its numbers.
fn path_click(
    (x, y, width, height): (i32, i32, i32, i32),
    turns: u64,
    min_segment_px: u64,
    duration_ms: u64,
    corner_hold_ms: u64,
) -> Value {
    json!({
        "kind": "single_touch_path_drag_v1",
        "box_rect": {"x": x, "y": y, "width": width, "height": height},
        "turns": turns,
        "min_segment_px": min_segment_px,
        "duration_ms": duration_ms,
        "corner_hold_ms": corner_hold_ms
    })
}

// A generated path drag: its box, points, segment durations and corner hold.
struct GeneratedPath {
    rect: (i32, i32, i32, i32),
    points: Vec<(i32, i32)>,
    durations: Vec<u64>,
    corner_hold_ms: u64,
}

// The input the kernel generates for `click` with the step's draw `draw`; a path drag records no
// sampling evidence (plan Q19), and its action keeps the contract.
fn generate(click: &Value, draw: PointDraw) -> GeneratedPath {
    let click: TaskClick = serde_json::from_value(click.clone()).expect("task click");
    click
        .validate(&RESOLUTION, None, "0.9")
        .unwrap_or_else(|error| panic!("the click is admitted: {error}"));
    let (action, sampling) = click
        .draw_input::<std::convert::Infallible>(&RESOLUTION, None, draw, &mut no_touch_choice)
        .map_err(uniform_draw_error)
        .unwrap_or_else(|error| panic!("the path is generated: {error}"));
    assert!(
        sampling.is_none(),
        "a path drag carries no sampling evidence"
    );
    action
        .validate()
        .unwrap_or_else(|error| panic!("the generated action keeps the contract: {error:?}"));
    match action {
        InputAction::SingleTouchPathDragV1 {
            box_x,
            box_y,
            box_width,
            box_height,
            points,
            segment_duration_ms,
            corner_hold_ms,
        } => GeneratedPath {
            rect: (box_x, box_y, box_width, box_height),
            points: points.iter().map(|point| (point.x, point.y)).collect(),
            durations: segment_duration_ms,
            corner_hold_ms,
        },
        other => panic!("a path drag, got {other:?}"),
    }
}

fn touch_draw(seed: u64, touch: TouchSampling) -> PointDraw {
    PointDraw {
        action_seed: Some(seed),
        touch: Some(touch),
    }
}

// Requirement 2: for fixed seeds and boxes, with the touch keys and without them, the generated
// drag keeps its box and corner hold, has `turns + 2` points in the box and one duration per
// segment, every segment at least `min_segment_px` long (Euclidean, whole pixels); its first point
// is one draw in the box (the hotspot with the touch keys, the uniform draw without, seed 0's
// state without a seed), and it carries no sampling evidence.
#[test]
fn t3_a_generated_path_has_turns_plus_two_points_in_the_box_with_spaced_segments() {
    for (rect, turns, min_segment_px) in [
        ((100, 100, 400, 300), 5, 150),
        ((50, 600, 120, 40), 3, 40),
        ((300, 200, 31, 200), 30, 80),
    ] {
        let click = path_click(rect, turns, min_segment_px, 4_000, 90);
        let draws = (0..24)
            .map(|index| {
                let seed = mixed_seed(index);
                (
                    touch_draw(seed, DEFAULT_TOUCH),
                    Reference::new(seed).hotspot_point(rect, 250, 8),
                )
            })
            .chain((24..32).map(|index| {
                let seed = mixed_seed(index);
                (
                    PointDraw::uniform(Some(seed)),
                    Reference::new(seed).uniform_point(rect),
                )
            }))
            .chain([(
                PointDraw {
                    action_seed: None,
                    touch: Some(DEFAULT_TOUCH),
                },
                Reference::new(0).hotspot_point(rect, 250, 8),
            )]);
        for (draw, first) in draws {
            let path = generate(&click, draw);
            let case = format!("box {rect:?}, draw {draw:?}: {:?}", path.points);
            assert_eq!(path.rect, rect, "{case}");
            assert_eq!(path.corner_hold_ms, 90, "{case}");
            assert_eq!(
                path.points.len(),
                usize::try_from(turns + 2).expect("count"),
                "{case}"
            );
            assert_eq!(path.durations.len(), path.points.len() - 1, "{case}");
            assert_eq!(path.points[0], first, "{case}");
            let (x, y, width, height) = rect;
            for point in &path.points {
                assert!(
                    (x..x + width).contains(&point.0) && (y..y + height).contains(&point.1),
                    "{point:?} outside the box; {case}"
                );
            }
            let least = i64::try_from(min_segment_px * min_segment_px).expect("square");
            for segment in path.points.windows(2) {
                assert!(
                    squared(segment[0], segment[1]) >= least,
                    "segment {segment:?} shorter than {min_segment_px}; {case}"
                );
            }
        }
    }
}

// Requirement 2: `duration_ms` is split in proportion to the segments' lengths in whole pixels
// (the integer square root of the squared length): each segment but the last takes
// ⌊duration · length / total length⌋, the last the remainder, and they sum to the declared
// duration. At the duration's bounds every segment takes 1 ms (duration turns + 1) or 60 000 ms
// (duration (turns + 1) × 60 000).
#[test]
fn t3_segment_durations_follow_the_lengths_and_sum_to_the_declared_duration() {
    let rect = (100, 100, 400, 300);
    let turns = 4;
    for index in 0..16 {
        let draw = touch_draw(mixed_seed(index), DEFAULT_TOUCH);
        let path = generate(&path_click(rect, turns, 120, 3_000, 0), draw);
        let lengths = path
            .points
            .windows(2)
            .map(|segment| {
                u64::try_from(squared(segment[0], segment[1]))
                    .expect("a squared length")
                    .isqrt()
            })
            .collect::<Vec<_>>();
        let total = lengths.iter().sum::<u64>();
        let mut expected = lengths[..lengths.len() - 1]
            .iter()
            .map(|length| 3_000 * length / total)
            .collect::<Vec<_>>();
        expected.push(3_000 - expected.iter().sum::<u64>());
        assert_eq!(path.durations, expected, "lengths {lengths:?}");
        assert_eq!(path.durations.iter().sum::<u64>(), 3_000);

        let shortest = generate(&path_click(rect, turns, 120, turns + 1, 0), draw);
        assert_eq!(shortest.durations, [1_u64; 5]);
        let longest = generate(&path_click(rect, turns, 120, (turns + 1) * 60_000, 0), draw);
        assert_eq!(longest.durations, [60_000_u64; 5]);
    }
}

// Requirement 2 and plan Q17: a next point whose `max_redraws + 1` draws all stay closer than
// `min_segment_px` to the previous point is the box's corner farthest from it. With a spread of
// 1 ‰ every hotspot draw of the 100 × 60 box is its centre (250, 130), so after the centre the
// next point is the corner farthest from the centre, the top left (200, 100); from the corner the
// centre is far enough again.
#[test]
fn t3_a_point_with_no_spaced_draw_takes_the_farthest_corner() {
    let rect = (200, 100, 100, 60);
    let narrow = TouchSampling {
        sigma_ratio_milli: 1,
        max_redraws: 0,
        candidate_count: 1,
    };
    // The corner farthest from the centre, by the rule: of the four corner pixels the farthest,
    // the first in the order top left, top right, bottom left, bottom right.
    let centre = (250, 130);
    let corners = [(200, 100), (299, 100), (200, 159), (299, 159)];
    let farthest = corners.iter().copied().fold(corners[0], |best, corner| {
        if squared(centre, corner) > squared(centre, best) {
            corner
        } else {
            best
        }
    });
    assert_eq!(farthest, (200, 100));
    for index in 0..4 {
        let path = generate(
            &path_click(rect, 4, 49, 2_000, 50),
            touch_draw(mixed_seed(index), narrow),
        );
        assert_eq!(
            path.points,
            [centre, farthest, centre, farthest, centre, farthest],
            "seed {index}"
        );
    }
}

// The control declaration of a neutral 16 × 9 package.
fn control() -> Value {
    json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.test.path",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 16, "height": 9},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    })
}

// The neutral task whose one operation from `home` to `page_b` is `click`.
fn task(click: Value) -> Value {
    let probe = |page: &str, expected: [u8; 3]| {
        json!({
            "id": format!("{page}_colour"),
            "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
            "expected": expected
        })
    };
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "T3 fixture",
        "coordinate_space": {"width": 16, "height": 9},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [probe("home", [0, 0, 255]), probe("page_b", [255, 0, 0])],
        "page_rules": {
            "home": {"required": ["home_colour"]},
            "page_b": {"required": ["page_b_colour"]}
        },
        "entry_page": "home",
        "target_page": "page_b",
        "operations": [{
            "id": "trace",
            "from": "home",
            "to": "page_b",
            "click": click,
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }],
        "scheduling_outcome": {"mappings": [{
            "outcome_key": "traced_k",
            "effect": "no_designated_effect",
            "terminal_pages": ["page_b"]
        }]}
    })
}

// The task file as the source declaration gate reads it: the one operation from `home` to
// `page_b` whose click is `click`, with the members the gate requires of a task file.
fn source_task(click: Value) -> Value {
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 16, "height": 9},
        "entry_page": "home",
        "target_page": "page_b",
        "operations": [{
            "id": "trace",
            "from": "home",
            "to": "page_b",
            "click": click,
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }]
    })
}

// Kernel admission of the package whose task is `task`, from its content table in memory.
fn admit(task: &Value) -> Result<PreparedContainedTask, ContainedTaskError> {
    let entries = [
        ("control.json", control()),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        ("resources/operations/task/task.json", task.clone()),
    ]
    .into_iter()
    .map(|(path, value)| {
        (
            path.to_owned(),
            serde_json::to_vec_pretty(&value).expect("package JSON"),
        )
    })
    .collect();
    PreparedContainedTask::load_content_entries("t3-test", entries, None, admission_deadline())
}

// The task file as an author writes it: the click's box is `box`.
fn source_form(click: &Value) -> Value {
    let mut click = click.clone();
    let object = click.as_object_mut().expect("click object");
    let rect = object.remove("box_rect").expect("box_rect");
    object.insert("box".to_owned(), rect);
    click
}

fn source_bundle(task: Value) -> Bundle {
    Bundle {
        task_id: "task".to_owned(),
        dir: PathBuf::from("operations/task"),
        data: task,
    }
}

// Requirement 1 (plan Q17): a path drag outside its bounds is refused at package admission with
// the existing declaration refusal at the member, in the task file (`box`, the source
// declaration gate) and compiled (`box_rect`, the kernel's admission), and by the kernel's own
// click check (`contained_task_operation_invalid`): 31 turns, a shortest segment above
// ⌊(max(width, height) − 1) / 2⌋ (7 for the 16 × 9 box), and a box beyond the task's 16 × 9
// resolution. The same click within its bounds is admitted in both forms.
#[test]
fn t3_a_path_drag_outside_its_bounds_is_refused_at_admission() {
    let admitted = path_click((0, 0, 16, 9), 3, 7, 1_200, 80);
    declaration_file_requests(&[source_bundle(source_task(source_form(&admitted)))])
        .unwrap_or_else(|error| panic!("the source form is admitted: {error:?}"));
    admit(&task(admitted.clone()))
        .unwrap_or_else(|error| panic!("the compiled form is admitted: {error}"));

    for (case, click, member) in [
        (
            "31 turns",
            path_click((0, 0, 16, 9), 31, 7, 32_000, 80),
            "turns",
        ),
        (
            "a shortest segment above the bound",
            path_click((0, 0, 16, 9), 3, 8, 1_200, 80),
            "min_segment_px",
        ),
        (
            "a box beyond the resolution",
            path_click((4, 2, 16, 8), 3, 7, 1_200, 80),
            "box_rect",
        ),
    ] {
        let source = source_form(&click);
        let source_member = if member == "box_rect" { "box" } else { member };
        let Err(refused) = declaration_file_requests(&[source_bundle(source_task(source))]) else {
            panic!("{case}: the source form is refused");
        };
        assert_eq!(refused.code, "resource_declaration_invalid", "{case}");
        let issue: ResourceDeclarationIssue =
            serde_json::from_value(refused.details.expect("the issue")).expect("issue shape");
        assert_eq!(
            issue.declaration_file, "operations/task/task.json",
            "{case}"
        );
        assert_eq!(
            issue.field_path,
            format!("/operations/0/click/{source_member}"),
            "{case}"
        );
        assert_eq!(
            issue.reason,
            ResourceDeclarationReason::InvalidValue,
            "{case}"
        );

        let Err(refused) = admit(&task(click.clone())) else {
            panic!("{case}: the compiled form is refused at admission");
        };
        assert_eq!(
            refused.code().as_str(),
            "resource_declaration_invalid",
            "{case}"
        );
        let issue = refused.declaration_issue().expect("the issue");
        assert_eq!(
            issue.field_path,
            format!("/operations/0/click/{member}"),
            "{case}"
        );
        assert_eq!(
            issue.reason,
            ResourceDeclarationReason::InvalidValue,
            "{case}"
        );

        let parsed: TaskClick = serde_json::from_value(click).expect("task click");
        let Err(refused) = parsed.validate(
            &Resolution {
                width: 16,
                height: 9,
            },
            None,
            "0.9",
        ) else {
            panic!("{case}: the kernel's click check refuses it");
        };
        assert_eq!(
            refused.code().as_str(),
            ExecutionKernelCode::ContainedTaskOperationInvalid.as_str(),
            "{case}"
        );
    }
}

// A source operation of `task` from `home`, to `to`, whose click is the source path drag `click`.
fn source_operation(id: &str, to: Value, click: &Value) -> Value {
    json!({
        "id": id,
        "from": "home",
        "to": to,
        "click": click,
        "unguarded_trusted_coordinate": true,
        "retryable": false
    })
}

fn pack_rect((x, y, width, height): (i32, i32, i32, i32)) -> PackRect {
    PackRect {
        x,
        y,
        width,
        height,
    }
}

// Requirements 1 and 6: source compilation writes a path drag's `box` as `box_rect`, in the
// compiled operation and in the generated navigation, as an edge to a page, an edge to a page set
// and a page operation; the declaration gate admits that navigation. The kernel's drive reader
// reads the click as a segmented drag with its box (`DrivePathDrag`); no route passes the edge,
// while a plain tap's edge stays a route; and as a destructive page operation the path drag
// covers its box: a tap inside the box is refused as overlapping, one outside it is not.
#[test]
fn t3_the_drive_reader_and_compilation_read_the_path_drag() {
    let rect = (40, 60, 200, 120);
    let compiled = path_click(rect, 3, 50, 1_200, 80);
    let source = source_form(&compiled);
    let tap = |id: &str, to: &str, x: i32| {
        json!({
            "id": id,
            "from": "home",
            "to": to,
            "click": {"kind": "rect", "x": x, "y": 100, "width": 10, "height": 10},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        })
    };
    let source_task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 320, "height": 240},
        "entry_page": "home",
        "target_page": "page_b",
        "operations": [
            source_operation("trace_to_page_b", json!("page_b"), &source),
            source_operation("trace_to_either", json!(["page_b", "page_c"]), &source),
            source_operation("trace_in_place", Value::Null, &source),
            tap("tap_inside_box", "page_c", 100),
            tap("tap_outside_box", "page_d", 280)
        ]
    });
    let parser = OperationParser {
        root: PathBuf::new(),
        game: "neutral".to_owned(),
        server: "test".to_owned(),
        locale: "en-US".to_owned(),
        coordinate_space: json!({"width": 320, "height": 240}),
        defaults: json!({}),
        resource_ids: HashSet::new(),
        bundles: vec![source_bundle(source_task)],
        existing_navigation: None,
        maa_task_overlays: HashMap::new(),
    };

    let primitives = parser.build_primitives().expect("compiled operations");
    let primitive = primitives["primitives"]
        .as_array()
        .expect("primitives")
        .iter()
        .find(|primitive| primitive["id"] == "trace_to_page_b")
        .expect("the path drag operation");
    assert_eq!(primitive["click"], compiled);

    let navigation = parser.build_navigation().expect("generated navigation");
    let edge = |id: &str| {
        navigation["navigation"]
            .as_array()
            .expect("edges")
            .iter()
            .find(|edge| edge["id"] == id)
            .unwrap_or_else(|| panic!("no edge {id} in {navigation}"))
            .clone()
    };
    assert_eq!(edge("trace_to_page_b")["click"], compiled);
    assert_eq!(edge("trace_to_page_b")["to_page"], "neutral/page_b");
    assert_eq!(edge("trace_to_either")["click"], compiled);
    assert_eq!(
        edge("trace_to_either")["to_pages"],
        json!(["neutral/page_b", "neutral/page_c"])
    );
    for list in ["page_operations", "destructive_actions"] {
        let [operation] = navigation[list].as_array().expect(list).as_slice() else {
            panic!("one page operation in {list}: {navigation}");
        };
        assert_eq!(operation["id"], "trace_in_place");
        assert_eq!(operation["click"], compiled);
    }
    validate_navigation_declarations(
        Path::new("navigation/neutral.test.navigation.json"),
        &navigation,
    )
    .unwrap_or_else(|error| panic!("the gate admits the navigation: {error:?}"));

    assert_eq!(
        parse_navigation_click(&compiled).expect("the drive reader reads the path drag"),
        DriveNavigationClick::PathDrag(DrivePathDrag {
            box_rect: pack_rect(rect),
        })
    );
    let graph = DriveNavigationGraph::parse_json(&navigation.to_string())
        .expect("a navigation graph with path drags");
    assert!(
        graph.find_route("neutral/home", "neutral/page_b").is_none(),
        "no route passes a path drag"
    );
    let inside = graph
        .find_route("neutral/home", "neutral/page_c")
        .expect("the tap's route");
    assert_eq!(inside.len(), 1);
    assert_eq!(inside[0].id(), "tap_inside_box");
    assert!(
        graph.validate_route(&inside).is_err(),
        "the path drag's box is a destructive region"
    );
    let outside = graph
        .find_route("neutral/home", "neutral/page_d")
        .expect("the other tap's route");
    graph
        .validate_route(&outside)
        .unwrap_or_else(|error| panic!("a tap outside the box: {error:?}"));
}
