// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #427-B (plan r2 point 5, #427 comment 6460013989, and review C4, comment 6459928838;
// approved whole by Alice 10-28 12:4x; `contracts/scheduling/README.md`, "Shared run limits";
// `contracts/performance-control.md`): through the host's policy cycle, the shared run limits
// bind the same in each pacing mode. The release default `observe` never slows anything down,
// so the protection is the limits themselves: the heavy-scene slots (`.normal` while the
// performance controller is at Normal, `.paused` above it) and the optional
// `performance.pacing.max_concurrent_runs`, which the host hands to every evaluation with the
// policy dispatches admitted or running. The schedule explanation read answers the deferral with
// its numbers. Two instances share one host; A's dispatch is admitted while B is held, then B is
// released and the next cycles judge B's candidate. Times are fixed Unix milliseconds on a
// manual Runtime clock; nothing sleeps.

use super::*;
use crate::{PolicyDispatchAdmission, PolicyTrigger};
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, FactScalar, PerformancePacingMode,
    RuntimeConfigManifest, RuntimeScheduleExplanation,
};
use actingcommand_policy::{PolicyEvaluation, SchedulingDecisionState, TaskDecision};
use actingcommand_runtime_config::performance::pacing::MODE;

/// The fixture tasks' weighted load sums to 600 milli: heavy on a host whose threshold is this.
const HEAVY_AT_600: u64 = 600;
const B_TASK: &str = "fixture.observe-b";
/// The spacing of host samples: the default hysteresis takes two samples per level and a 4 s
/// cooldown, so one level per two samples.
const HOST_SAMPLE_SPACING_MS: u64 = 2_000;
const MODES: [(&str, PerformancePacingMode); 3] = [
    ("observe", PerformancePacingMode::Observe),
    ("pace", PerformancePacingMode::Pace),
    ("full", PerformancePacingMode::Full),
];

/// The configuration a start hands the host: only `performance.pacing.mode`, the rest default.
fn mode_manifest(mode: &str) -> RuntimeConfigManifest {
    let mut parameters = vec![ConfigParameter {
        key: MODE.to_owned(),
        value: FactScalar::String(mode.to_owned()),
        source: ConfigParameterSource::Explicit,
        settable: false,
    }];
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("the key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// A host of A and B in `mode`, with the controller's `control`, the two-task catalog active.
fn two_instance_host(root: &TempDir, mode: &str, control: PerformanceControlConfig) -> RuntimeHost {
    let host = RuntimeHost::start(
        config(root)
            .with_runtime_clock(Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0)))
            .with_performance_control(control)
            .with_config_manifest(mode_manifest(mode)),
        Arc::new(FakeProvider::from_entries([
            (
                POLICY_INSTANCE_ALIAS.to_owned(),
                instance_id(),
                Arc::new(FakeState::default()),
            ),
            (
                POLICY_INSTANCE_ALIAS_B.to_owned(),
                instance_id(),
                Arc::new(FakeState::default()),
            ),
        ])),
    )
    .expect("the two-instance host");
    host.activate_policy_catalog(&pending_policy_sources(1))
        .expect("the two-instance catalog");
    host
}

/// The pacing mode the host's status read answers.
fn pacing_mode(host: &RuntimeHost) -> PerformancePacingMode {
    let mut client = TestClient::connect(host);
    let receipt = client.send(&client.request(RuntimeOperation::PerformanceStatus));
    let Some(RuntimeResult::PerformanceStatus { status }) = receipt.result() else {
        panic!("expected a performance status, got {receipt:?}");
    };
    status.pacing_mode
}

/// One policy cycle at the fixture's time; B runs on A's host, so the two share its limits.
fn evaluate(host: &RuntimeHost, trigger: PolicyTrigger) -> PolicyEvaluation {
    let mut facts = pending_policy_facts();
    for instance in &mut facts.instances {
        instance.host_id = "fixture-host-a".to_owned();
    }
    host.evaluate_policy_cycle_with_test_inputs(
        &facts,
        &policy_resources(),
        EvaluationTime {
            unix_ms: POLICY_NOW_UNIX_MS,
            monotonic_ms: POLICY_NOW_UNIX_MS,
        },
        7,
        trigger,
    )
    .expect("a policy cycle")
    .evaluation
    .expect("an evaluated cycle")
}

/// A's dispatch is chosen while B's lease holds B, and admitted; then B's lease is released.
fn admit_a_while_b_is_held(host: &RuntimeHost) {
    let mut holder = TestClient::connect(host);
    let (_, token) = holder.acquire(POLICY_INSTANCE_ALIAS_B);
    let evaluation = evaluate(host, PolicyTrigger::FactsChanged);
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.instance_id == POLICY_INSTANCE_ALIAS)
        .unwrap_or_else(|| panic!("A's dispatch intent: {evaluation:#?}"))
        .clone();
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("A's reason chain")
        .clone();
    record_policy_approval(host, &intent);
    assert!(matches!(
        host.admit_policy_dispatch(&intent, &reasons, &policy_context(host, &intent))
            .expect("A's admission"),
        PolicyDispatchAdmission::Granted { .. }
    ));
    let release = holder.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(
        holder.send(&release).state(),
        RuntimeReceiptState::Completed
    );
}

/// Raises the controller above Normal (QosReduced) with host responsiveness samples under the
/// default hysteresis, the last at the fixture's time.
fn raise_above_normal(host: &RuntimeHost) {
    let samples = 8;
    let first_at = POLICY_NOW_UNIX_MS - (samples - 1) * HOST_SAMPLE_SPACING_MS;
    for index in 0..samples {
        host.observe_performance_control_for_test(PerformanceControlObservation {
            observed_at_unix_ms: first_at + index * HOST_SAMPLE_SPACING_MS,
            host_responsiveness_basis_points: Some(5_000),
            third_party_pressure_basis_points: None,
            foreground_fullscreen: false,
        })
        .expect("host responsiveness sample");
    }
    assert_ne!(
        host.performance_control_directive(POLICY_INSTANCE_ALIAS)
            .expect("directive")
            .level,
        PerformanceControlLevel::Normal
    );
}

fn b_decision(evaluation: &PolicyEvaluation) -> &TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == B_TASK
                && decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS_B)
        })
        .unwrap_or_else(|| panic!("B's decision: {evaluation:#?}"))
}

fn b_chosen(evaluation: &PolicyEvaluation) -> bool {
    evaluation
        .dispatch_intents
        .iter()
        .any(|intent| intent.instance_id == POLICY_INSTANCE_ALIAS_B)
}

/// An outcome's code and its four numbers: `used`, `limit_count`, `admitted_count`,
/// `round_selected_count`.
fn slots(outcome: &actingcommand_contract::outcome::Outcome) -> (&str, [i64; 4]) {
    let values = outcome.values();
    let number = |key: &str| {
        values
            .get(key)
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_else(|| panic!("{key} in {values:?}"))
    };
    (
        outcome.code().as_str(),
        [
            number("used"),
            number("limit_count"),
            number("admitted_count"),
            number("round_selected_count"),
        ],
    )
}

/// The numbers of the decision's reason with `code`: the evaluator's last reason, though the host
/// may note `eligible_since_unknown` after it.
fn slots_of<'a>(decision: &'a TaskDecision, code: &str) -> (&'a str, [i64; 4]) {
    slots(
        &decision
            .reasons
            .iter()
            .find(|reason| reason.outcome.code().as_str() == code)
            .unwrap_or_else(|| panic!("a reason {code}: {decision:#?}"))
            .outcome,
    )
}

/// The schedule explanation read of B, with its candidate rows.
fn explanation_of_b(host: &RuntimeHost) -> RuntimeScheduleExplanation {
    let mut client = TestClient::connect(host);
    let receipt = client.send(&client.request(RuntimeOperation::ScheduleExplanation {
        instance: Some(POLICY_INSTANCE_ALIAS_B.to_owned()),
        candidates: true,
    }));
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let Some(RuntimeResult::ScheduleExplanation { explanation }) = receipt.result() else {
        panic!("expected a schedule explanation, got {receipt:?}");
    };
    (**explanation).clone()
}

/// In each mode: two heavy-scene slots at Normal, one under pressure. With A's heavy dispatch
/// admitted, B's heavy candidate is chosen at Normal (1 of 2 taken); above Normal the limit is
/// `.paused`, A takes it, and B is blocked `heavy_scene_budget_exhausted` with 1 of 1 used, 1
/// admitted, 0 chosen.
#[test]
fn gate_427b_the_heavy_limit_binds_normal_and_under_pressure_in_observe_pace_and_full() {
    for (mode, expected) in MODES {
        let root = TempDir::new().expect("tempdir");
        let host = two_instance_host(
            &root,
            mode,
            PerformanceControlConfig::default()
                .with_heavy_dispatch_limits(2, 1)
                .with_heavy_threshold_milli(HEAVY_AT_600),
        );
        assert_eq!(pacing_mode(&host), expected, "{mode}");
        admit_a_while_b_is_held(&host);

        let evaluation = evaluate(&host, PolicyTrigger::Recovery);
        assert!(b_chosen(&evaluation), "{mode} at Normal: {evaluation:#?}");

        raise_above_normal(&host);
        let evaluation = evaluate(&host, PolicyTrigger::Recovery);
        assert!(!b_chosen(&evaluation), "{mode} under pressure");
        let decision = b_decision(&evaluation);
        assert_eq!(decision.state, SchedulingDecisionState::Blocked, "{mode}");
        let exhausted = "heavy_scene_budget_exhausted";
        assert_eq!(
            slots_of(decision, exhausted),
            (exhausted, [1, 1, 1, 0]),
            "{mode}"
        );
        host.close().expect("close the host");
    }
}

/// In each mode: `max_concurrent_runs` 1 and A's (light) dispatch admitted, so B's candidate
/// waits `policy_concurrent_runs_deferred` with 1 of 1 used, 1 admitted, 0 chosen; the schedule
/// explanation read answers B's not-running row and candidate row with those numbers.
#[test]
fn gate_427b_the_max_key_binds_in_observe_pace_and_full_and_the_explanation_says_the_numbers() {
    for (mode, expected) in MODES {
        let root = TempDir::new().expect("tempdir");
        let host = two_instance_host(
            &root,
            mode,
            PerformanceControlConfig::default().with_max_concurrent_runs(Some(1)),
        );
        assert_eq!(pacing_mode(&host), expected, "{mode}");
        admit_a_while_b_is_held(&host);

        let evaluation = evaluate(&host, PolicyTrigger::Recovery);
        assert!(!b_chosen(&evaluation), "{mode}: {evaluation:#?}");
        let decision = b_decision(&evaluation);
        let numbers = ("policy_concurrent_runs_deferred", [1, 1, 1, 0]);
        assert_eq!(slots_of(decision, numbers.0), numbers, "{mode}");

        let explanation = explanation_of_b(&host);
        let [entry] = explanation.instances.as_slice() else {
            panic!("B's one entry: {explanation:#?}");
        };
        assert_eq!(entry.instance_alias, POLICY_INSTANCE_ALIAS_B);
        assert_eq!(entry.not_running.task_id.as_deref(), Some(B_TASK));
        assert_eq!(slots(&entry.not_running.why), numbers, "{mode}");
        let row = entry
            .candidates
            .iter()
            .find(|row| row.task_id == B_TASK)
            .unwrap_or_else(|| panic!("B's row: {entry:#?}"));
        assert_eq!(slots(&row.why), numbers, "{mode}");
        host.close().expect("close the host");
    }
}
