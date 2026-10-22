// SPDX-License-Identifier: AGPL-3.0-only

//! #411 B16 part 1, slice BT-KEY (0.12.x B train; B-train plan §2.11 items 1 and 3, test row T3
//! of §5.3; register §1.164; contract key K47): in the static walk of a phased route, the state
//! of an operation capped with a key (`on_max_runs: {"outcome_key": K}`) is a terminal under the
//! one keyed-terminal rule: exactly one mapping with that state's effect condition lists the page,
//! and its key is K (`task_phases_outcome_incomplete` otherwise); and `max_runs` on a designated
//! operation is refused (`task_phases_designated_effect_invalid`). The routes are synthetic and
//! neutral: one stage on `page_a` whose target page is `page_b`.

use actingcommand_contract::{
    SchedulingOutcomeDeclaration, TaskPhase, TaskPhaseEntry, TaskPhaseOperation, TaskPhaseRunCap,
    validate_phased_route_located,
};
use serde_json::{Value, json};

// A tap from `page_a` to the pages `to`, with its per-run cap.
fn operation<'a>(
    id: &'a str,
    to: &[&str],
    max_runs: Option<TaskPhaseRunCap<'a>>,
) -> TaskPhaseOperation<'a> {
    TaskPhaseOperation {
        id,
        from: "page_a",
        destinations: to.iter().map(|page| (*page).to_owned()).collect(),
        retryable: false,
        may_be_passed_over: false,
        on_empty_key: None,
        max_runs,
        on_guard_refused_key: None,
        on_timeout_key: None,
    }
}

// The one stage, with its operations, from `page_a` to `page_b`.
fn stage(operations: &[&str]) -> Vec<TaskPhase> {
    serde_json::from_value(json!([
        {"id": "stage", "operations": operations, "target_pages": ["page_b"]}
    ]))
    .expect("the stage")
}

fn scheduling(declaration: Value) -> SchedulingOutcomeDeclaration {
    serde_json::from_value(declaration).expect("the scheduling outcome")
}

// Each mapping `(key, page)` maps the page under no designated effect.
fn mapped(mappings: &[(&str, &str)]) -> SchedulingOutcomeDeclaration {
    let mappings = mappings
        .iter()
        .map(|(key, page)| {
            json!({"outcome_key": key, "effect": "no_designated_effect",
                   "terminal_pages": [page]})
        })
        .collect::<Vec<_>>();
    scheduling(json!({"mappings": mappings}))
}

#[test]
fn bt_key_t3_a_keyed_cap_needs_its_one_mapping_and_no_designated_operation_is_capped() {
    let entries = [TaskPhaseEntry {
        phase: 0,
        page: "page_a",
    }];
    let targets = ["page_b".to_owned()];
    let walk = |phases: &[TaskPhase],
                operations: &[TaskPhaseOperation<'_>],
                scheduling: &SchedulingOutcomeDeclaration| {
        validate_phased_route_located(
            "neutral", &entries, phases, &targets, operations, scheduling,
        )
    };

    // `op_a` stays on `page_a` and is capped at 3 runs with the key `repeat_capped`: the state on
    // `page_a` is a terminal whose one mapping is `repeat_capped`'s.
    let keyed = TaskPhaseRunCap {
        limit: 3,
        outcome_key: Some("repeat_capped"),
    };
    let capped = [operation("op_a", &["page_a"], Some(keyed))];
    let one_stage = stage(&["op_a"]);
    walk(
        &one_stage,
        &capped,
        &mapped(&[("repeat_capped", "page_a"), ("finished", "page_b")]),
    )
    .unwrap_or_else(|refusal| panic!("a keyed cap with its one mapping: {refusal:?}"));
    for (case, mappings) in [
        ("no mapping of page_a", vec![("finished", "page_b")]),
        (
            "page_a mapped to another key",
            vec![("other_key", "page_a"), ("finished", "page_b")],
        ),
    ] {
        let refusal = walk(&one_stage, &capped, &mapped(&mappings))
            .expect_err("a keyed cap without its one mapping");
        assert_eq!(
            refusal.code.as_str(),
            "task_phases_outcome_incomplete",
            "{case}: {refusal:?}"
        );
    }

    // `op_a` is the designated operation, from `page_a` to `page_b`; `op_b` too reaches
    // `page_b`. Without `max_runs` the route is admitted; with it, it is refused.
    let designated = scheduling(json!({
        "designated_operation": "op_a",
        "mappings": [
            {"outcome_key": "done_effect", "effect": "designated_effect_completed",
             "terminal_pages": ["page_b"]},
            {"outcome_key": "done_plain", "effect": "no_designated_effect",
             "terminal_pages": ["page_b"]}
        ]
    }));
    let two_operations = stage(&["op_a", "op_b"]);
    walk(
        &two_operations,
        &[
            operation("op_a", &["page_b"], None),
            operation("op_b", &["page_b"], None),
        ],
        &designated,
    )
    .unwrap_or_else(|refusal| panic!("a designated operation without max_runs: {refusal:?}"));
    let next_candidate = TaskPhaseRunCap {
        limit: 3,
        outcome_key: None,
    };
    let refusal = walk(
        &two_operations,
        &[
            operation("op_a", &["page_b"], Some(next_candidate)),
            operation("op_b", &["page_b"], None),
        ],
        &designated,
    )
    .expect_err("max_runs on the designated operation");
    assert_eq!(
        refusal.code.as_str(),
        "task_phases_designated_effect_invalid",
        "{refusal:?}"
    );
}
