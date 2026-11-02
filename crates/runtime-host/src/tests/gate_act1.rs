// SPDX-License-Identifier: AGPL-3.0-only

// ACT-1 and ACT-2 (Workflow #407, register §1.250; design `rd-1031/check-speed/model-check.md`
// §1.6, §1.7 and §7; coordinator brief `brief-slices.md`): what the host hands the kernel with a
// step's pacing from the `execution.action_gap` keys.
//
// - The settled path goes with `mode` `settled` at a pacing stage up to `settled_max_stage`; above
//   it, and under `declared`, none does, and the step takes its `post_delay_ms`.
// - The gap adds to the pacing term `pre_input_wait_base_ms × (f − 1000) / 1000` and is never
//   multiplied by the factor: `fixed` adds `fixed_ms`, `random` a value of its range drawn per step
//   from a seed and kept for the run's diagnostics, `performance` nothing, and `declared` nothing
//   whatever the kind.
//
// The pacing term below: base 300 ms, so 0 at the factor 1000 and 75 ms at 1250.

use crate::codes::ActionGapCode;
use crate::performance::{
    EffectivePacing, action_gap_drawn, action_gap_fell_back, apply_action_gap,
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

// The pre-input wait of a step at the factor `factor_milli` under `gap`, from `seed`.
fn pre_input_wait(gap: &ActionGapConfig, factor_milli: u32, seed: u64) -> Duration {
    apply_action_gap(paced(factor_milli), gap, PerformancePacingStage::S0, seed)
        .0
        .pre_input_wait
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
        let (pacing, _) = apply_action_gap(paced(1_000), &gap, stage, 0);
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
        let (pacing, drawn) = apply_action_gap(paced(1_250), &declared, stage, 7);
        assert_eq!(pacing, paced(1_250), "{stage:?}");
        assert_eq!(drawn, None, "{stage:?}");
    }
}

#[test]
fn gap_fixed_zero_adds_nothing() {
    let gap = ActionGapConfig {
        kind: ActionGapKind::Fixed,
        fixed_ms: 0,
        ..settled()
    };
    assert_eq!(pre_input_wait(&gap, 1_000, 0), Duration::ZERO);
    assert_eq!(pre_input_wait(&gap, 1_250, 0), ms(75));
}

#[test]
fn gap_random_stays_in_range_and_is_seeded() {
    let gap = ActionGapConfig {
        kind: ActionGapKind::Random,
        random_min_ms: 100,
        random_max_ms: 300,
        ..settled()
    };
    let mut drawn = Vec::new();
    for seed in 0..1_000 {
        let (pacing, value) =
            apply_action_gap(paced(1_000), &gap, PerformancePacingStage::S0, seed);
        let value = value.expect("a drawn gap is kept for the diagnostics");
        assert!((100..=300).contains(&value), "seed {seed}: {value}");
        assert_eq!(pacing.pre_input_wait, ms(value), "seed {seed}");
        assert_eq!(
            apply_action_gap(paced(1_000), &gap, PerformancePacingStage::S0, seed).1,
            Some(value),
            "the same seed draws the same gap"
        );
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
    // `fixed_ms` is another kind's: the wait is the curve's alone, 0 at the factor 1000.
    let gap = ActionGapConfig {
        kind: ActionGapKind::Performance,
        fixed_ms: 500,
        ..settled()
    };
    for factor_milli in [1_000, 1_250, 2_000] {
        assert_eq!(
            pre_input_wait(&gap, factor_milli, 0),
            paced(factor_milli).pre_input_wait,
            "{factor_milli}"
        );
    }
    assert_eq!(pre_input_wait(&gap, 1_000, 0), Duration::ZERO);
}

#[test]
fn gap_adds_to_the_pacing_term() {
    let fixed = ActionGapConfig {
        kind: ActionGapKind::Fixed,
        fixed_ms: 40,
        ..settled()
    };
    assert_eq!(pre_input_wait(&fixed, 1_000, 0), ms(40));
    assert_eq!(pre_input_wait(&fixed, 1_250, 0), ms(115));
    let random = ActionGapConfig {
        kind: ActionGapKind::Random,
        random_min_ms: 10,
        random_max_ms: 10,
        ..settled()
    };
    assert_eq!(pre_input_wait(&random, 1_250, 0), ms(85));
}

#[test]
fn gap_is_never_multiplied_by_the_factor() {
    // The wait less the pacing term is `fixed_ms` at every factor.
    let gap = ActionGapConfig {
        kind: ActionGapKind::Fixed,
        fixed_ms: 40,
        ..settled()
    };
    for factor_milli in [1_000, 1_250, 2_000, 4_000] {
        assert_eq!(
            pre_input_wait(&gap, factor_milli, 0) - paced(factor_milli).pre_input_wait,
            ms(40),
            "{factor_milli}"
        );
    }
}
