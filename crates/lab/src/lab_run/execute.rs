// SPDX-License-Identifier: AGPL-3.0-only

fn target_stable_with(previous: &TargetEvaluation, current: &TargetEvaluation) -> bool {
    previous.passed && current.passed && target_measurement_stable_with(previous, current)
}

pub fn target_evaluations_stable_for_wait(
    previous: &TargetEvaluation,
    current: &TargetEvaluation,
) -> bool {
    target_stable_with(previous, current)
}

fn target_measurement_stable_with(previous: &TargetEvaluation, current: &TargetEvaluation) -> bool {
    if previous.id != current.id || previous.kind != current.kind {
        return false;
    }
    if !template_evaluation_stable(previous, current) {
        return false;
    }
    color_evaluation_stable(previous, current)
}

fn template_evaluation_stable(previous: &TargetEvaluation, current: &TargetEvaluation) -> bool {
    match (previous.template, current.template) {
        (Some(previous), Some(current)) => {
            (previous.x - current.x).abs() <= ROI_TEMPLATE_POSITION_EPSILON
                && (previous.y - current.y).abs() <= ROI_TEMPLATE_POSITION_EPSILON
                && (previous.score - current.score).abs() <= ROI_TEMPLATE_SCORE_EPSILON
        }
        (None, None) => true,
        _ => false,
    }
}

fn color_evaluation_stable(previous: &TargetEvaluation, current: &TargetEvaluation) -> bool {
    match (previous.color, current.color) {
        (Some(previous), Some(current)) => {
            let mean_stable = previous
                .mean
                .iter()
                .zip(current.mean.iter())
                .all(|(previous, current)| previous.abs_diff(*current) <= ROI_COLOR_MEAN_EPSILON);
            mean_stable
                && (previous.distance - current.distance).abs() <= ROI_COLOR_DISTANCE_EPSILON
        }
        (None, None) => true,
        _ => false,
    }
}
