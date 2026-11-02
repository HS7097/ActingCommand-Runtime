// SPDX-License-Identifier: AGPL-3.0-only

// ACT-1 and ACT-2 (Workflow #407, register §1.250; design `rd-1031/check-speed/model-check.md`
// §1.6, §1.7 and §7; coordinator brief `brief-slices.md`): what the host hands the kernel with a
// step's pacing from the `execution.action_gap` keys.
//
// - The settled path goes with `mode` `settled` at a pacing stage up to `settled_max_stage`; above
//   it, and under `declared`, none does, and the step takes its `post_delay_ms`.
// - The pre-input wait stays the pacing term alone, `pre_input_wait_base_ms × (f − 1000) / 1000`,
//   under every kind. The step's gap is asked apart: `fixed` is `fixed_ms`, `random` a value of its
//   range from a seed, `performance` nothing, and `declared` nothing whatever the kind. The kernel's
//   gate holds where it is waited.
//
// The pacing term below: base 300 ms, so 0 at the factor 1000 and 75 ms at 1250.

use crate::codes::ActionGapCode;
use crate::performance::{
    EffectivePacing, action_gap_drawn, action_gap_fell_back, apply_settled_path,
};
use actingcommand_contract::PerformancePacingStage;
use actingcommand_execution_kernel::{SettledPath, StepPacing};
use actingcommand_runtime_config::{
    ActionGapConfig, ActionGapKind, ActionGapMode, EffectiveConfig,
};
use std::time::Duration;

const PRE_INPUT_WAIT_BASE_MS: u64 = 300;
const STAGES: [PerformancePacingStage; 5] = [
    PerformancePacingStage::S0,
    PerformancePacingStage::S1,
    PerformancePacingStage::S2,
    PerformancePacingStage::S3,
    PerformancePacingStage::S4,
];

// The shipped keys, `settled`.
fn settled() -> ActionGapConfig {
    ActionGapConfig {
        mode: ActionGapMode::Settled,
        ..EffectiveConfig::default()
            .action_gap_section()
            .expect("the shipped keys")
    }
}

// What a step at the factor `factor_milli` is handed without the keys.
fn paced(factor_milli: u32) -> StepPacing {
    EffectivePacing {
        factor_milli,
        extra_wait_rounds: 0,
    }
    .step_pacing(0, PRE_INPUT_WAIT_BASE_MS)
}

fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

#[test]
fn stage_above_settled_max_uses_declared() {
    // `settled_max_stage` s1: S0 and S1 go on the path with the keys' values, S2 and above do not.
    let gap = ActionGapConfig {
        poll_ms: 20,
        settle_frames: 2,
        settle_change_max_milli: 300,
        settled_max_stage: PerformancePacingStage::S1,
        ..settled()
    };
    for stage in STAGES {
        let pacing = apply_settled_path(paced(1_000), &gap, stage);
        let expected =
            (stage <= PerformancePacingStage::S1).then(|| SettledPath::new(ms(20), 2, 300));
        assert_eq!(pacing.settled_path, expected, "{stage:?}");
    }
    // `declared`: none at any stage, and the pacing as it came.
    let declared = ActionGapConfig {
        mode: ActionGapMode::Declared,
        ..gap
    };
    for stage in STAGES {
        assert_eq!(
            apply_settled_path(paced(1_250), &declared, stage),
            paced(1_250),
            "{stage:?}"
        );
    }
}

#[test]
fn gap_fixed_zero_adds_nothing() {
    let gap = ActionGapConfig {
        kind: ActionGapKind::Fixed,
        fixed_ms: 0,
        ..settled()
    };
    assert_eq!(gap.gap_ms(7), 0);
    assert!(!gap.draws());
    // `declared` adds nothing whatever the kind's value.
    let declared = ActionGapConfig {
        mode: ActionGapMode::Declared,
        fixed_ms: 500,
        ..gap
    };
    assert_eq!(declared.gap_ms(7), 0);
}

#[test]
fn gap_random_stays_in_range_and_is_seeded() {
    let gap = ActionGapConfig {
        kind: ActionGapKind::Random,
        random_min_ms: 100,
        random_max_ms: 300,
        ..settled()
    };
    assert!(gap.draws());
    let mut drawn = Vec::new();
    for seed in 0..1_000 {
        let value = gap.gap_ms(seed);
        assert!((100..=300).contains(&value), "seed {seed}: {value}");
        assert_eq!(gap.gap_ms(seed), value, "the same seed draws the same gap");
        drawn.push(value);
    }
    assert_eq!(drawn.iter().min(), Some(&100));
    assert_eq!(drawn.iter().max(), Some(&300));

    // Both records of the code: the drawn value, and an operation's fall-back.
    let record = action_gap_drawn(150);
    assert!(record.is(ActionGapCode::Recorded));
    assert_eq!(record.value("action_gap_ms"), Some(&serde_json::json!(150)));
    let record = action_gap_fell_back("neutral.package", "open");
    assert!(record.is(ActionGapCode::Recorded));
    assert_eq!(
        record.value("pack_operation_id"),
        Some(&serde_json::json!("open"))
    );
}

#[test]
fn gap_performance_is_the_pre_input_curve() {
    // `fixed_ms` is another kind's: no gap, and the pre-input wait is the curve's alone, 0 at the
    // factor 1000.
    let gap = ActionGapConfig {
        kind: ActionGapKind::Performance,
        fixed_ms: 500,
        ..settled()
    };
    assert_eq!(gap.gap_ms(7), 0);
    assert!(!gap.draws());
    for (factor_milli, curve) in [(1_000, 0), (1_250, 75), (2_000, 300)] {
        let pacing = apply_settled_path(paced(factor_milli), &gap, PerformancePacingStage::S0);
        assert_eq!(pacing.pre_input_wait, ms(curve), "{factor_milli}");
    }
}
