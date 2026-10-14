// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #396 REC-5 logic test of the kernel (REC-M v1.2.1 §7, §12): a template that matched
// at a declared factor gives its resampled box to a `target` click, and its `offset` click
// scales the four declared numbers by that factor, `(v * factor + 500) div 1000` with the sign
// of `v`, a width or height at least 1. The evaluation is synthetic and neutral.

use super::*;
use serde_json::{Value, json};

/// A passed template evaluation whose box is `(300, 200, 18, 14)`: a 24 by 18 template
/// resampled by 750 milli when `scale_milli` is 750.
fn matched(scale_milli: Option<u32>) -> TargetEvaluation {
    TargetEvaluation {
        id: "icon".to_string(),
        kind: TargetKind::Template,
        passed: true,
        template: Some(actingcommand_recognition_pack::TemplateEvaluation {
            x: 300,
            y: 200,
            width: 18,
            height: 14,
            raw_score: 1.0,
            score: 1.0,
            threshold: 0.9,
            scale_milli,
        }),
        color: None,
        ocr: None,
        nn: None,
        color_digest: None,
        composite: None,
        sample_evaluations: Vec::new(),
        sampling: None,
        message: "matched".to_string(),
        failure: None,
        value_test: None,
        uniqueness: None,
    }
}

/// The box a click samples its tap from.
fn click_box(click: &Value, target: &TargetEvaluation) -> (i32, i32, i32, i32) {
    let click: TaskClick = serde_json::from_value(click.clone()).expect("click");
    let (_, sampling) = click
        .input_action(
            &Resolution {
                width: 1280,
                height: 720,
            },
            Some(target),
            Some(7),
        )
        .expect("a sampled tap");
    let region = sampling.expect("sampling evidence").source_regions()[0];
    (region.x(), region.y(), region.width(), region.height())
}

#[test]
fn a_click_offset_from_a_template_matched_at_750_milli_scales_with_the_factor() {
    let offset = json!({
        "kind": "offset", "target_id": "icon",
        "offset": {"x": 30, "y": -7, "width": 41, "height": 10}
    });
    let target = json!({"kind": "target", "target_id": "icon"});

    // 30 -> 23, -7 -> -5 (5.25 rounds to 5), 41 -> 31 (30.75), 10 -> 8 (7.5 rounds half up).
    assert_eq!(click_box(&offset, &matched(Some(750))), (323, 195, 31, 8));
    assert_eq!(click_box(&target, &matched(Some(750))), (300, 200, 18, 14));
    // A template without scales keeps the declared offset.
    assert_eq!(click_box(&offset, &matched(None)), (330, 193, 41, 10));
}
