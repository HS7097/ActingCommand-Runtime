// SPDX-License-Identifier: AGPL-3.0-only

// #411 Lab RP (exec key 115; register §1.142; contracts/remedies.md section 4): the Lab's
// single-step select evaluations, `dry_run_select` and the scan replay (`replay_scan`), read a
// package's run parameters as a run without a dispatch does: every parameter the package declares
// with its declared default, as `session.dispatch.parameter.<name>`, each given value replacing
// its parameter's default as a dispatch's chosen value does. A package that declares no parameter
// evaluates exactly as before, and a parameter given to it is refused, never ignored.
//
// The list is SEL-L6b's (gate_308_l6b): one column of six cards, `member-a` to `member-f` from
// top to bottom. The package here provides `cap_x` with `param_p` (options `opt_1` and `opt_2`,
// default `opt_2`); its select step's policy keeps `member-a` to `member-c` for `opt_1` and
// `member-d` to `member-f` for `opt_2`, and aborts the evaluation when the value is unknown.
// Every expected value comes from the slice's requirement lines; names are neutral.

use super::gate_308_l6b::{
    MEMBERS, PUBLISHED_AT, card_scan, forward_run, list, scan_task, snapshot_facts,
};
use super::*;
use crate::{OfflineReplayFacts, dry_run_scan, dry_run_select, replay_scan};
use actingcommand_contract::{InstanceFactContext, InstanceFactSnapshot, ScanCoverageState};
use actingcommand_selection_policy::{
    SelectionDecision, SelectionFactSnapshot, SelectionOutcome, SelectionPolicy,
};
use serde_json::json;

const PARAMETER_KEY: &str = "session.dispatch.parameter.param_p";
const NOW_UNIX_MS: u64 = 1_000_000;

// SEL-L6b's scan task as a provider of `cap_x` with `param_p`, its select step's policy keyed on
// the parameter.
fn parameter_task() -> PreparedContainedTask {
    let mut task = scan_task();
    task.control.provides = vec![
        serde_json::from_value(json!({
            "capability": "cap_x",
            "parameters": [{"name": "param_p", "options": ["opt_1", "opt_2"],
                            "default": "opt_2"}]
        }))
        .expect("provided capability"),
    ];
    let keeps = |option: &str, members: &[&str]| {
        json!({"kind": "all", "of": [
            {"kind": "string_in", "value": {"source": "fact", "fact_key": PARAMETER_KEY},
             "allowed": [option]},
            {"kind": "string_in", "value": {"source": "field", "field": "member"},
             "allowed": members}
        ]})
    };
    let policy: SelectionPolicy = serde_json::from_value(json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "cards-by-parameter",
        "applies_to": {
            "candidate_layout_id": "layout/cards",
            "outcome_keys": {"selected": "card_selected", "empty": "card_empty",
                             "insufficient": "card_insufficient",
                             "ambiguous": "card_ambiguous", "unknown": "card_unknown"}
        },
        "fields": [{"name": "member",
                    "value_type": {"type": "enum_string", "allowed": MEMBERS}}],
        "facts": [{"fact_key": PARAMETER_KEY,
                   "value_type": {"type": "enum_string", "allowed": ["opt_1", "opt_2"]},
                   "max_age_ms": 60_000, "minimum_confidence_milli": 1_000}],
        "gates": [{"gate_id": "by_parameter",
                   "predicate": {"kind": "any", "of": [keeps("opt_1", &MEMBERS[..3]),
                                                       keeps("opt_2", &MEMBERS[3..])]},
                   "on_unknown": {"kind": "abort_evaluation"}}],
        "scoring": [],
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    }))
    .expect("selection policy");
    policy.validate().expect("valid selection policy");
    task.program.operations[0].prepared_select = Some(Box::new(
        selection::PreparedSelect::for_test("layout/cards", policy),
    ));
    task
}

// An instance's stored fact snapshot without records, in the package's game and server.
fn instance_snapshot() -> InstanceFactSnapshot {
    InstanceFactSnapshot {
        snapshot_id: "snapshot-a".to_owned(),
        ledger_position: 7,
        context: InstanceFactContext {
            instance_id: "instance-a".to_owned(),
            server_id: "test".to_owned(),
            game_id: "neutral".to_owned(),
        },
        records: Vec::new(),
    }
}

// The cards `dry_run_select` selects on the list's first frame, and its outcome.
fn selected(task: &PreparedContainedTask, parameters: &RunParameters) -> SelectionDecision {
    dry_run_select(
        task,
        "pick",
        &list(true, false),
        (&instance_snapshot(), NOW_UNIX_MS),
        parameters,
        None,
    )
    .expect("the select step evaluates on the saved frame")
    .decision
}

// The identities of the replayed scan's members and the gated-out identities, over the recorded
// forward run of the whole list with the empty offline snapshot.
fn scanned(task: &PreparedContainedTask, parameters: &RunParameters) -> (Vec<String>, Vec<String>) {
    let scan = replay_scan(
        task,
        "pick",
        &card_scan(),
        forward_run(),
        &OfflineReplayFacts::Empty {
            now_unix_ms: NOW_UNIX_MS,
        },
        parameters,
    )
    .expect("the replayed scan gives a result");
    assert_eq!(scan.coverage, ScanCoverageState::Complete, "{scan:?}");
    assert_eq!(scan.decision, None, "{scan:?}");
    (
        scan.members
            .into_iter()
            .map(|member| member.identity)
            .collect(),
        scan.gated_out,
    )
}

fn identities(members: &[&str]) -> Vec<String> {
    members.iter().map(|member| (*member).to_owned()).collect()
}

// Requirement 1: without given values, both single-step evaluations read the declared default
// `opt_2`: the dry run selects the fourth card (`member-d`, the lowest card `opt_2` keeps) and
// the scan keeps `member-d` to `member-f`, gating out the others. The verbs pass the parameters
// of no given value, which are the declared defaults a run without a dispatch reads.
#[test]
fn lab_rp_a_single_step_select_reads_the_declared_default() {
    let task = parameter_task();
    let parameters = task.given_run_parameters(&[]).expect("no value is given");
    assert_eq!(parameters, task.default_run_parameters());

    let decision = selected(&task, &parameters);
    assert_eq!(
        decision.outcome,
        SelectionOutcome::Selected { count: 1 },
        "{decision:?}"
    );
    assert_eq!(decision.selected, ["layout/cards#03"]);

    assert_eq!(
        scanned(&task, &parameters),
        (identities(&MEMBERS[3..]), identities(&MEMBERS[..3]))
    );
}

// Requirement 2: a given value replaces its parameter's default, as a dispatch's chosen value
// does: with `param_p` = `opt_1` the dry run selects the first card (`member-a`) and the scan
// keeps `member-a` to `member-c`, gating out the others.
#[test]
fn lab_rp_a_given_parameter_replaces_the_default() {
    let task = parameter_task();
    let parameters = task
        .given_run_parameters(&[("param_p".to_owned(), "opt_1".to_owned())])
        .expect("a declared option of a declared parameter");

    let decision = selected(&task, &parameters);
    assert_eq!(
        decision.outcome,
        SelectionOutcome::Selected { count: 1 },
        "{decision:?}"
    );
    assert_eq!(decision.selected, ["layout/cards#00"]);

    assert_eq!(
        scanned(&task, &parameters),
        (identities(&MEMBERS[..3]), identities(&MEMBERS[3..]))
    );
}

// Requirement 3: a package that declares no parameter (SEL-L6b's own) evaluates exactly as it
// did before: the scan replay gives the result of the scan over the given facts as they are, and
// the dry run the evaluation over them; a parameter given to it is refused, never ignored.
#[test]
fn lab_rp_a_package_without_parameters_is_unchanged() {
    let task = scan_task();
    let parameters = task.given_run_parameters(&[]).expect("no value is given");
    let OfflineReplayFacts::Snapshot {
        snapshot,
        read_at_unix_ms,
    } = snapshot_facts(PUBLISHED_AT + 500)
    else {
        panic!("a given snapshot");
    };
    let record_lists = task.program.operations[0]
        .prepared_select
        .as_deref()
        .expect("the select step")
        .policy()
        .record_list_keys();
    let as_given = SelectionFactSnapshot::from_instance_snapshot_with_lists(
        &snapshot,
        read_at_unix_ms,
        &record_lists,
    );

    let replayed = replay_scan(
        &task,
        "pick",
        &card_scan(),
        forward_run(),
        &OfflineReplayFacts::Snapshot {
            snapshot: snapshot.clone(),
            read_at_unix_ms,
        },
        &parameters,
    )
    .expect("the replayed scan gives a result");
    let over_the_facts = dry_run_scan(
        &task,
        "pick",
        &card_scan(),
        forward_run(),
        &as_given,
        read_at_unix_ms,
    )
    .expect("the scan over the given facts gives a result");
    assert_eq!(replayed, over_the_facts);
    assert_eq!(replayed.members.len(), MEMBERS.len(), "{replayed:?}");

    let dry_run = dry_run_select(
        &task,
        "pick",
        &list(true, false),
        (&snapshot, read_at_unix_ms),
        &parameters,
        None,
    )
    .expect("the select step evaluates on the saved frame");
    let over_the_facts = selection::dry_run_select_with_facts(
        &task,
        "pick",
        &list(true, false),
        (&as_given, read_at_unix_ms),
        None,
    )
    .expect("the evaluation over the given facts");
    assert_eq!(dry_run, over_the_facts);

    assert_eq!(
        task.given_run_parameters(&[("param_p".to_owned(), "opt_1".to_owned())]),
        Err(GivenParameterRefusal {
            name: "param_p".to_owned(),
            problem: GivenParameterProblem::Undeclared,
        })
    );
}
