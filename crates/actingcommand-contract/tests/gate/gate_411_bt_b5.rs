// SPDX-License-Identifier: AGPL-3.0-only

//! #411 B5, slice BT-B5 (0.12.x B train; B-train plan §2.5 point 1, test row T2 of §5.3; register
//! §1.154; contract key K37): the static walk of a phased route treats a page a stage's operation
//! confirms that is one of the stage's `end_pages` as a terminal, which needs exactly one mapping
//! for its effect condition and page (`task_phases_outcome_incomplete` otherwise); an end page
//! that is also a target page of its stage is refused as `task_phases_invalid` at that end page.
//! The route is synthetic and neutral: `reach` opens the panel from home, `return` comes back.

use actingcommand_contract::{
    SchedulingOutcomeDeclaration, TaskPhase, TaskPhaseEntry, TaskPhaseOperation,
    validate_phased_route_located,
};
use serde_json::{Value, json};

// A tap from `from` to the pages `to`.
fn operation<'a>(id: &'a str, from: &'a str, to: &[&str]) -> TaskPhaseOperation<'a> {
    TaskPhaseOperation {
        id,
        from,
        destinations: to.iter().map(|page| (*page).to_owned()).collect(),
        retryable: false,
        may_be_passed_over: false,
        on_empty_key: None,
        max_runs: None,
        on_guard_refused_key: None,
    }
}

// The two stages; `reach` lists `end_pages`.
fn stages(end_pages: Value) -> Vec<TaskPhase> {
    serde_json::from_value(json!([
        {"id": "reach", "operations": ["open"], "target_pages": ["panel"],
         "end_pages": end_pages},
        {"id": "return", "operations": ["back"], "target_pages": ["home"]}
    ]))
    .expect("the stages")
}

// Each mapping `(key, page)` maps the page under no designated effect.
fn scheduling(mappings: &[(&str, &str)]) -> SchedulingOutcomeDeclaration {
    let mappings = mappings
        .iter()
        .map(|(key, page)| {
            json!({"outcome_key": key, "effect": "no_designated_effect",
                   "terminal_pages": [page]})
        })
        .collect::<Vec<_>>();
    serde_json::from_value(json!({"mappings": mappings})).expect("the scheduling outcome")
}

#[test]
fn bt_b5_t2_an_end_page_needs_one_mapping_and_is_no_target_page_of_its_stage() {
    let entries = [TaskPhaseEntry {
        phase: 0,
        page: "home",
    }];
    let targets = ["home".to_owned()];
    // `open` reaches the panel or the notice, an end page of `reach`.
    let to_notice = [
        operation("open", "home", &["panel", "notice"]),
        operation("back", "panel", &["home"]),
    ];
    let notice = stages(json!(["notice"]));

    let mapped = scheduling(&[("returned", "home"), ("notice_shown", "notice")]);
    validate_phased_route_located("neutral", &entries, &notice, &targets, &to_notice, &mapped)
        .unwrap_or_else(|refusal| panic!("an end page with its one mapping: {refusal:?}"));

    let unmapped = scheduling(&[("returned", "home")]);
    let refusal = validate_phased_route_located(
        "neutral", &entries, &notice, &targets, &to_notice, &unmapped,
    )
    .expect_err("an end page without a mapping");
    assert_eq!(
        refusal.code.as_str(),
        "task_phases_outcome_incomplete",
        "{refusal:?}"
    );

    // `reach` lists its own target page, the panel, as an end page.
    let to_panel = [
        operation("open", "home", &["panel"]),
        operation("back", "panel", &["home"]),
    ];
    let overlap = scheduling(&[("returned", "home"), ("panel_shown", "panel")]);
    let refusal = validate_phased_route_located(
        "neutral",
        &entries,
        &stages(json!(["panel"])),
        &targets,
        &to_panel,
        &overlap,
    )
    .expect_err("an end page that is a target page of its stage");
    assert_eq!(refusal.code.as_str(), "task_phases_invalid", "{refusal:?}");
    let location = refusal.location.as_deref().expect("a located refusal");
    assert_eq!(location.json_pointer, "/phases/0/end_pages/0");
    assert_eq!(location.page_id.as_deref(), Some("panel"));
}
