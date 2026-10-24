// SPDX-License-Identifier: AGPL-3.0-only

// SCH-M slice ED-1 gate tests (frozen model v1.0 `rd-1024/sched-design/model.md` §3.4 D-12, D-15
// and §6 row ED-1; Alice 10-22: 「近5次该任务包的平均时间加5分钟」). Every expected value comes from
// D-12:
// - E = min(⌈mean(S)⌉ + margin_ms, M), S the latest min(runs, k) of k samples, M the task's
//   `loop_budget.max_runtime_ms`;
// - k = 0 → E = D (the declared value); k = 1–4 → the mean of those + margin; k ≥ runs → the mean
//   of the latest `runs` + margin;
// - `enabled = false` gives E = D, and every evaluation is byte-identical to the one without
//   samples;
// and from D-15: E replaces the declared value as the dispatch intent's duration; the same
// snapshot evaluated twice gives the same evaluation.

use serde_json::Value;

use super::selection_catalog::{NOW, catalog, instance};
use crate::{
    CompiledCatalog, EvaluationFacts, EvaluationResources, EvaluationTime, ExpectedRuntime,
    ExpectedRuntimeSettings, ExpectedRuntimeSource, HostResourceSnapshot, InstanceSnapshot,
    PolicyEvaluation, TaskRuntimeSnapshot, expected_runtime,
};

const TASK_A: &str = "task-a";
const INSTANCE_A: &str = "instance-a";
// The model's defaults: five runs, five minutes.
const DEFAULTS: ExpectedRuntimeSettings = ExpectedRuntimeSettings {
    enabled: true,
    runs: 5,
    margin_ms: 300_000,
};
// A declared value and a runtime ceiling far above the samples, unless a case says otherwise.
const DECLARED_MS: u64 = 1_560_000;
const CEILING_MS: u64 = 1_800_000;

fn e(samples: &[u64], settings: ExpectedRuntimeSettings, ceiling_ms: u64) -> ExpectedRuntime {
    expected_runtime(samples, settings, DECLARED_MS, ceiling_ms).expect("E is computed")
}

fn declared() -> ExpectedRuntime {
    ExpectedRuntime {
        expected_ms: DECLARED_MS,
        source: ExpectedRuntimeSource::Declared,
    }
}

// D-12: with no sample, E is the declared value, and the snapshot carries nothing.
#[test]
fn ed1_t1_no_sample_is_the_declared_value() {
    let none = e(&[], DEFAULTS, CEILING_MS);
    assert_eq!(none, declared());
    assert_eq!(none.snapshot_members(), (None, None));
}

// D-12: one to four samples: their mean, rounded up, plus the margin.
#[test]
fn ed1_t2_one_to_four_runs_are_their_mean_plus_the_margin() {
    assert_eq!(
        e(&[120_000], DEFAULTS, CEILING_MS),
        ExpectedRuntime {
            expected_ms: 420_000,
            source: ExpectedRuntimeSource::Runs(1),
        }
    );
    // (100 000 + 200 000 + 301 000) / 3 = 200 333.3…, rounded up.
    let three = e(&[100_000, 200_000, 301_000], DEFAULTS, CEILING_MS);
    assert_eq!(
        three,
        ExpectedRuntime {
            expected_ms: 500_334,
            source: ExpectedRuntimeSource::Runs(3),
        }
    );
    assert_eq!(three.snapshot_members(), (Some(500_334), Some(3)));
    assert_eq!(
        e(&[100_000, 200_000, 300_000, 400_000], DEFAULTS, CEILING_MS),
        ExpectedRuntime {
            expected_ms: 550_000,
            source: ExpectedRuntimeSource::Runs(4),
        }
    );
}

// D-12: more samples than `runs`: only the latest `runs` (newest first) count.
#[test]
fn ed1_t3_more_runs_than_the_setting_use_the_latest_only() {
    let samples = [10_000, 20_000, 30_000, 40_000, 50_000, 900_000, 900_000];
    assert_eq!(
        e(&samples, DEFAULTS, CEILING_MS),
        ExpectedRuntime {
            expected_ms: 330_000,
            source: ExpectedRuntimeSource::Runs(5),
        }
    );
    let three = ExpectedRuntimeSettings {
        runs: 3,
        ..DEFAULTS
    };
    assert_eq!(
        e(&samples, three, CEILING_MS),
        ExpectedRuntime {
            expected_ms: 320_000,
            source: ExpectedRuntimeSource::Runs(3),
        }
    );
}

// D-12: E is never above the task's own runtime ceiling.
#[test]
fn ed1_t4_e_is_capped_at_the_task_runtime_ceiling() {
    assert_eq!(
        e(&[900_000], DEFAULTS, 1_000_000),
        ExpectedRuntime {
            expected_ms: 1_000_000,
            source: ExpectedRuntimeSource::Runs(1),
        }
    );
}

// The fixture's task: declared 60 000 ms, ceiling 300 000 ms, due at NOW on instance A.
fn fixture() -> (CompiledCatalog, InstanceSnapshot) {
    (catalog(&[(TASK_A, 1)], None), instance(INSTANCE_A, &[]))
}

fn snapshot(members: (Option<u64>, Option<u8>)) -> TaskRuntimeSnapshot {
    TaskRuntimeSnapshot {
        task_id: TASK_A.to_owned(),
        instance_id: INSTANCE_A.to_owned(),
        last_dispatched_unix_ms: None,
        eligible_since_unix_ms: None,
        terminal_state: None,
        completed_window: None,
        last_duration_ms: Some(90_000),
        failure_streak: None,
        expected_runtime_ms: members.0,
        expected_runtime_runs: members.1,
    }
}

fn evaluate(
    catalog: &CompiledCatalog,
    instance: &InstanceSnapshot,
    state: TaskRuntimeSnapshot,
) -> PolicyEvaluation {
    let inputs = EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-ed1".to_owned(),
        facts: Vec::new(),
        outcomes: Vec::new(),
        tasks: vec![state],
        instances: vec![instance.clone()],
        priority_offsets: Vec::new(),
    };
    let resources = EvaluationResources {
        pools: Vec::new(),
        hosts: vec![HostResourceSnapshot {
            host_id: instance.host_id.clone(),
            cpu_available_milli: 1_000,
            gpu_available_milli: 1_000,
            io_available_milli: 1_000,
            host_responsiveness_basis_points: 10_000,
            third_party_pressure_basis_points: 0,
            heavy_dispatch_limit: 1,
            active_heavy_dispatches: 0,
            heavy_threshold_milli: crate::DEFAULT_HEAVY_THRESHOLD_MILLI,
        }],
    };
    let time = EvaluationTime {
        unix_ms: NOW,
        monotonic_ms: NOW,
    };
    crate::evaluate(catalog, &inputs, &resources, time, 11).expect("the fixture evaluates")
}

fn bytes(value: &impl serde::Serialize) -> Vec<u8> {
    serde_json::to_vec(value).expect("serializes")
}

// D-12, D-15: with the switch off, the samples give the declared value and no snapshot member,
// so the snapshot's bytes and the evaluation's are those of a pair without samples; with it on,
// the dispatch intent carries E instead of the declared value.
#[test]
fn ed1_t5_with_the_switch_off_the_evaluation_is_byte_identical() {
    let (catalog, instance) = fixture();
    let samples = [100_000, 140_000];
    let off = ExpectedRuntimeSettings {
        enabled: false,
        runs: 5,
        margin_ms: 60_000,
    };
    let off_e = expected_runtime(&samples, off, 60_000, 300_000).expect("E is computed");
    assert_eq!(
        off_e,
        ExpectedRuntime {
            expected_ms: 60_000,
            source: ExpectedRuntimeSource::Declared,
        }
    );
    let without = snapshot((None, None));
    let switched_off = snapshot(off_e.snapshot_members());
    assert_eq!(bytes(&switched_off), bytes(&without));
    let snapshot_json: Value = serde_json::from_slice(&bytes(&switched_off)).expect("JSON");
    assert!(snapshot_json.get("expected_runtime_ms").is_none());
    assert!(snapshot_json.get("expected_runtime_runs").is_none());
    let baseline = evaluate(&catalog, &instance, without);
    assert_eq!(
        bytes(&evaluate(&catalog, &instance, switched_off)),
        bytes(&baseline)
    );
    assert_eq!(
        baseline.dispatch_intents.len(),
        1,
        "the fixture task is due"
    );
    assert_eq!(baseline.dispatch_intents[0].expected_duration_ms, 60_000);

    // On: E = ⌈(100 000 + 140 000) / 2⌉ + 60 000 = 180 000, within the ceiling.
    let on = ExpectedRuntimeSettings {
        enabled: true,
        ..off
    };
    let on_e = expected_runtime(&samples, on, 60_000, 300_000).expect("E is computed");
    assert_eq!(on_e.snapshot_members(), (Some(180_000), Some(2)));
    let with_e = evaluate(&catalog, &instance, snapshot(on_e.snapshot_members()));
    assert_eq!(with_e.dispatch_intents.len(), 1);
    assert_eq!(with_e.dispatch_intents[0].expected_duration_ms, 180_000);
}

// D-15 (identity): the same snapshot with E, evaluated twice, gives the same evaluation.
#[test]
fn ed1_t6_the_same_snapshot_evaluated_twice_is_identical() {
    let (catalog, instance) = fixture();
    let state = snapshot((Some(180_000), Some(2)));
    let first = evaluate(&catalog, &instance, state.clone());
    let second = evaluate(&catalog, &instance, state);
    assert_eq!(bytes(&first), bytes(&second));
    assert_eq!(first.dispatch_intents[0].expected_duration_ms, 180_000);
}
