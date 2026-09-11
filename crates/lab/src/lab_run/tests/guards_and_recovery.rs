// SPDX-License-Identifier: AGPL-3.0-only

#[test]
fn roi_comparison_waits_until_roi_becomes_stable() {
    let baseline = color_target_evaluation("target/button", [0, 0, 0], true);
    let changed = color_target_evaluation("target/button", [8, 0, 0], true);
    assert!(!target_evaluations_stable_for_wait(&baseline, &changed));
    assert!(target_evaluations_stable_for_wait(&changed, &changed));
}

#[test]
fn roi_comparison_passes_static_roi_on_first_followup_frame() {
    let baseline = color_target_evaluation("target/button", [0, 0, 0], true);
    assert!(target_evaluations_stable_for_wait(&baseline, &baseline));
}

#[test]
fn roi_comparison_rejects_continuously_changing_roi() {
    let mut previous = color_target_evaluation("target/button", [0, 0, 0], true);
    for mean in [[3, 0, 0], [6, 0, 0], [9, 0, 0]] {
        let current = color_target_evaluation("target/button", mean, true);
        assert!(!target_evaluations_stable_for_wait(&previous, &current));
        previous = current;
    }
}

#[test]
fn roi_comparison_resets_when_target_fails() {
    let baseline = color_target_evaluation("target/button", [0, 0, 0], true);
    let failed = color_target_evaluation("target/button", [0, 0, 0], false);
    assert!(!target_evaluations_stable_for_wait(&baseline, &failed));
    assert!(!target_evaluations_stable_for_wait(&failed, &baseline));
    assert!(target_evaluations_stable_for_wait(&baseline, &baseline));
}
#[test]
fn page_namespace_matches_operation_anchors_without_blind_split() {
    assert_eq!(canonical_page_anchor("arknights", "arknights/home"), "home");
    assert_eq!(
        canonical_page_anchor("arknights", "arknights/navigation/home_to_task"),
        "navigation/home_to_task"
    );
    assert_eq!(canonical_page_anchor("arknights", "home"), "home");
    assert!(page_anchor_matches("arknights", "arknights/home", "home"));
    assert!(page_anchor_matches("arknights", "home", "home"));
    assert!(page_anchor_matches(
        "arknights",
        "arknights/quickswitch_dropdown",
        "quickswitch_dropdown"
    ));
    assert!(!page_anchor_matches(
        "arknights",
        "bluearchive/home",
        "home"
    ));
}
