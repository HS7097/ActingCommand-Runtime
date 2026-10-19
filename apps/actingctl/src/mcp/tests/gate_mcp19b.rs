// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #386 SR-6b, MCP-19b (register §1.144; plan-mcp19 §2, Q4, Q8, Q9):
//! - apps key 80 (M19B-2): `ac_schedule` fits its answer to the output budget by cutting candidate
//!   rows only, from the last row of the last entry backwards, each entry that lost rows marked
//!   `candidates_truncated: true`; `evaluated_at_unix_ms`, `catalog_hash` and each entry's
//!   `instance_alias`, `next_wake_unix_ms`, `selected`, `not_running` and `candidates_total` are
//!   never cut, and an answer that still does not fit is `output_budget_exceeded`.
//! - apps key 81 (M19B-4): `ac_diagnose`'s `not_run` lists, per entry of the explanation and in
//!   its order, the rows whose state is `deferred`, at most `scheduling.not_run.diagnose_rows`
//!   (from `config.parameters.scheduling`) per instance, each with its `instance_alias`, and
//!   `not_run_total` when one was left out; a bound that cannot be read is a warning, the
//!   diagnosis is incomplete and has no `not_run`.

use super::super::protocol::Era;
use super::super::runs::Diagnosis;
use super::super::schedule::{cut_to_fit, within_budget};
use super::super::tools::{ToolError, ToolSuccess, call_result};
use crate::schedule::{self, DIAGNOSE_ROWS_KEY};
use actingcommand_contract::outcome::{CodeStr, Outcome};
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, FactScalar, OriginModule, RUNTIME_FACT_SCHEMA_VERSION,
    RuntimeConfigManifest, RuntimeFactSnapshot, RuntimeScheduleExplanation,
    ScheduleCandidateEligibility, ScheduleCandidateState, ScheduleDispatchState,
    ScheduleExplanationCandidate, ScheduleExplanationInstance, ScheduleExplanationNotRunning,
    ScheduleExplanationSelected,
};
use serde_json::{Value, json};

const EVALUATED_AT: u64 = 1_700_042_460_000;
const HOUR_MS: u64 = 3_600_000;

fn code(spelling: &str) -> CodeStr {
    Outcome::relayed(spelling).code().clone()
}

/// A candidate row of `task_id` whose last reason is the last of `reasons`.
fn row(
    task_id: &str,
    state: ScheduleCandidateState,
    eligibility: ScheduleCandidateEligibility,
    reasons: &[&str],
    next_wake_unix_ms: Option<u64>,
) -> ScheduleExplanationCandidate {
    let why = reasons.last().expect("a row has a reason");
    ScheduleExplanationCandidate {
        task_id: task_id.to_owned(),
        state,
        eligibility,
        why: Outcome::relayed(why),
        reasons: reasons.iter().copied().map(code).collect(),
        reasons_omitted: None,
        next_wake_unix_ms,
        since_unix_ms: EVALUATED_AT,
    }
}

/// A due row another task outranked.
fn outranked(task_id: &str) -> ScheduleExplanationCandidate {
    row(
        task_id,
        ScheduleCandidateState::Deferred,
        ScheduleCandidateEligibility::True,
        &["task_eligible", "instance_already_selected"],
        None,
    )
}

/// A row blocked by its own trigger, waking in `hours`.
fn waiting(task_id: &str, hours: u64) -> ScheduleExplanationCandidate {
    row(
        task_id,
        ScheduleCandidateState::Blocked,
        ScheduleCandidateEligibility::False,
        &["trigger_false"],
        Some(EVALUATED_AT + hours * HOUR_MS),
    )
}

/// The entry of `alias`: `fixture.slot` selected, the first candidate its not-running row.
fn entry(
    alias: &str,
    candidates: Vec<ScheduleExplanationCandidate>,
) -> ScheduleExplanationInstance {
    let first = candidates.first().expect("an entry with a candidate");
    ScheduleExplanationInstance {
        instance_alias: alias.to_owned(),
        next_wake_unix_ms: Some(EVALUATED_AT + HOUR_MS),
        selected: Some(ScheduleExplanationSelected {
            task_id: "fixture.slot".to_owned(),
            decision_id: format!("decision:{alias}"),
            dispatch_state: ScheduleDispatchState::Intent,
        }),
        not_running: ScheduleExplanationNotRunning {
            task_id: Some(first.task_id.clone()),
            why: first.why.clone(),
            next_wake_unix_ms: first.next_wake_unix_ms,
            since_unix_ms: first.since_unix_ms,
        },
        candidates_total: u32::try_from(candidates.len()).expect("candidate count"),
        candidates,
    }
}

fn explanation(instances: Vec<ScheduleExplanationInstance>) -> RuntimeScheduleExplanation {
    let explanation = RuntimeScheduleExplanation {
        evaluated_at_unix_ms: Some(EVALUATED_AT),
        catalog_hash: Some(format!("sha256:{}", "0".repeat(64))),
        instances,
    };
    assert_eq!(explanation.validate(), Ok(()), "setup: a valid answer");
    explanation
}

/// The task ids of each entry's candidate rows in `answer`.
fn task_ids(answer: &Value) -> Vec<Vec<String>> {
    answer["instances"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| {
            entry["candidates"]
                .as_array()
                .map(|rows| {
                    rows.iter()
                        .map(|row| row["task_id"].as_str().expect("a task id").to_owned())
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect()
}

/// Every candidate row left in `answer`.
fn rows_left(answer: &Value) -> usize {
    task_ids(answer).iter().map(Vec::len).sum()
}

/// `answer` without what the cut may change: each entry's `candidates` and its mark.
fn uncut(answer: &Value) -> Value {
    let mut kept = answer.clone();
    for entry in kept["instances"].as_array_mut().expect("entries") {
        let members = entry.as_object_mut().expect("an entry");
        members.remove("candidates");
        members.remove("candidates_truncated");
    }
    kept
}

#[test]
fn mcp19b_ac_schedule_cuts_candidate_rows_from_the_end() {
    let answer = json!(explanation(vec![
        entry(
            "neutral.first",
            vec![
                outranked("fixture.second"),
                waiting("fixture.later", 1),
                waiting("fixture.last", 2)
            ],
        ),
        entry(
            "neutral.second",
            vec![
                outranked("fixture.second"),
                waiting("fixture.later", 1),
                waiting("fixture.last", 2)
            ],
        ),
    ]));
    let all = ["fixture.second", "fixture.later", "fixture.last"]
        .map(str::to_owned)
        .to_vec();

    // Four rows fit: the second entry loses its last two rows, the first keeps all three.
    let mut four = answer.clone();
    cut_to_fit(&mut four, |candidate| rows_left(candidate) <= 4);
    assert_eq!(
        task_ids(&four),
        vec![all.clone(), vec!["fixture.second".to_owned()]],
        "{four:#}"
    );
    assert_eq!(four["instances"][0].get("candidates_truncated"), None);
    assert_eq!(four["instances"][1]["candidates_truncated"], json!(true));
    assert_eq!(uncut(&four), uncut(&answer), "nothing but rows was cut");

    // Two rows fit: the second entry loses every row, then the first its last one.
    let mut two = answer.clone();
    cut_to_fit(&mut two, |candidate| rows_left(candidate) <= 2);
    assert_eq!(
        task_ids(&two),
        vec![all[..2].to_vec(), Vec::new()],
        "{two:#}"
    );
    for index in 0..2 {
        assert_eq!(
            two["instances"][index]["candidates_truncated"],
            json!(true),
            "{two:#}"
        );
    }
    assert_eq!(uncut(&two), uncut(&answer), "nothing but rows was cut");

    // An answer that fits is not cut.
    let mut whole = answer.clone();
    cut_to_fit(&mut whole, |_| true);
    assert_eq!(whole, answer);

    // An answer whose uncuttable part alone is over the budget: every row is cut, the rest stays,
    // and the call answers output_budget_exceeded.
    let many = json!(explanation(
        (0..64)
            .map(|index| entry(
                &format!("neutral.instance.{index:03}"),
                vec![outranked("fixture.second")]
            ))
            .collect(),
    ));
    let ToolSuccess { result, .. } = within_budget(many.clone());
    assert_eq!(rows_left(&result), 0, "every row is cut first");
    assert_eq!(uncut(&result), uncut(&many), "nothing but rows was cut");
    let answered = call_result(Ok(ToolSuccess::new(result)), Era::Modern);
    assert_eq!(
        answered["structuredContent"]["error"]["outcome"]["code"],
        json!("output_budget_exceeded"),
        "{answered}"
    );
}

/// The program facts with the section `config.parameters.scheduling` holding `parameters`.
fn facts(parameters: Vec<ConfigParameter>) -> RuntimeFactSnapshot {
    let manifest = RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    };
    RuntimeFactSnapshot {
        schema_version: RUNTIME_FACT_SCHEMA_VERSION.to_owned(),
        ledger_position: 1,
        taken_at_unix_ms: EVALUATED_AT,
        snapshot_id: 1,
        part: 1,
        parts: 1,
        records: vec![manifest.parameter_section_fact_record(
            "scheduling",
            EVALUATED_AT,
            OriginModule::Runtime,
        )],
    }
}

fn parameter(key: &str, value: FactScalar) -> ConfigParameter {
    ConfigParameter {
        key: key.to_owned(),
        value,
        source: ConfigParameterSource::Explicit,
        settable: true,
    }
}

/// The program facts with the bound `rows`.
fn bound(rows: i64) -> RuntimeFactSnapshot {
    facts(vec![parameter(
        DIAGNOSE_ROWS_KEY,
        FactScalar::Integer(rows),
    )])
}

/// A diagnosis with only `not_run` and its warnings, rendered as ac_diagnose answers it.
fn render(explanation: &RuntimeScheduleExplanation, facts: RuntimeFactSnapshot) -> ToolSuccess {
    let mut warnings = Vec::new();
    let not_run = schedule::not_run(Ok(explanation.clone()), Ok(facts), &mut warnings);
    Diagnosis {
        instance: Value::Null,
        since: EVALUATED_AT - 24 * HOUR_MS,
        incomplete: false,
        runs: Vec::new(),
        errors: Vec::new(),
        error_sequences: Vec::new(),
        errors_more: false,
        errors_next: None,
        snapshot: 121_000,
        continuable: true,
        repeating: Vec::new(),
        suspended: Vec::new(),
        packs: Vec::new(),
        remedies: Vec::new(),
        lift: Vec::new(),
        report_warnings: Vec::new(),
        report_status: json!({"status": "ok", "through_sequence": 121_000}),
        report_outcome: None,
        clock_slots: Vec::new(),
        clock_slots_total: 0,
        runs_grouped: Vec::new(),
        failures_grouped: Vec::new(),
        not_run,
        warnings,
    }
    .render_within_budget(|_| -> Result<Value, ToolError> {
        Ok(json!({"events": [], "more": false, "snapshot_ledger_position": 121_000}))
    })
    .unwrap_or_else(|error| panic!("ac_diagnose rendering: {}", error.to_value()))
}

/// `row` as `not_run` lists it: the candidate row with its instance's alias.
fn listed(alias: &str, row: &ScheduleExplanationCandidate) -> Value {
    let mut listed = json!(row);
    listed["instance_alias"] = json!(alias);
    listed
}

#[test]
fn mcp19b_diagnose_lists_deferred_rows_up_to_the_configured_bound() {
    // Three deferred rows (two outranked, one waiting for the scheduler's random delay) and two
    // rows blocked by their own trigger, in the answer's order.
    let deferred = [
        outranked("fixture.second"),
        outranked("fixture.third"),
        row(
            "fixture.fourth",
            ScheduleCandidateState::Deferred,
            ScheduleCandidateEligibility::True,
            &["task_eligible", "dispatch_random_delay_waiting"],
            Some(EVALUATED_AT + HOUR_MS / 4),
        ),
    ];
    let blocked = [waiting("fixture.later", 1), waiting("fixture.last", 2)];
    let one = explanation(vec![entry(
        "neutral.first",
        deferred.iter().chain(&blocked).cloned().collect(),
    )]);

    let two_rows = render(&one, bound(2));
    assert_eq!(
        two_rows.result["not_run"],
        json!([
            listed("neutral.first", &deferred[0]),
            listed("neutral.first", &deferred[1]),
        ]),
        "{:#}",
        two_rows.result
    );
    assert_eq!(two_rows.result["not_run_total"], json!(3));
    assert!(two_rows.warnings.is_empty());
    assert_eq!(two_rows.result.get("incomplete"), None);

    let no_rows = render(&one, bound(0));
    assert_eq!(no_rows.result["not_run"], json!([]));
    assert_eq!(no_rows.result["not_run_total"], json!(3));

    // Every instance: each entry gives up to the bound, in the answer's order.
    let second = [outranked("fixture.second"), waiting("fixture.later", 1)];
    let both = explanation(vec![
        entry(
            "neutral.first",
            deferred.iter().chain(&blocked).cloned().collect(),
        ),
        entry("neutral.second", second.to_vec()),
    ]);
    let every = render(&both, bound(2));
    assert_eq!(
        every.result["not_run"],
        json!([
            listed("neutral.first", &deferred[0]),
            listed("neutral.first", &deferred[1]),
            listed("neutral.second", &second[0]),
        ]),
        "{:#}",
        every.result
    );
    assert_eq!(every.result["not_run_total"], json!(4));

    // Every deferred row within the bound: no total.
    let whole = render(&both, bound(10));
    assert_eq!(
        whole.result["not_run"].as_array().map(Vec::len),
        Some(4),
        "{:#}",
        whole.result
    );
    assert_eq!(whole.result.get("not_run_total"), None);

    // A scheduling section without the row: a warning, incomplete, no not_run.
    let unbound = render(
        &one,
        facts(vec![parameter(
            "scheduling.random_delay.enabled",
            FactScalar::Boolean(false),
        )]),
    );
    assert_eq!(unbound.result.get("not_run"), None, "{:#}", unbound.result);
    assert_eq!(unbound.result.get("not_run_total"), None);
    assert_eq!(unbound.result["incomplete"], json!(true));
    assert_eq!(
        unbound
            .warnings
            .iter()
            .map(|warning| warning["outcome"]["code"].clone())
            .collect::<Vec<_>>(),
        vec![json!("config_facts_missing")],
        "one warning"
    );
}
