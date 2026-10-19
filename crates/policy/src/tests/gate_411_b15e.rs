// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B15 R2 gate tests (model-b15 §5.3, §6, §7, §8, §9.2, §11;
// `contracts/remedies.md` sections 3.3, 4, 5 and 6; policy gate key 52; register §1.112): the
// evaluator plans the remedies packages declare. A policy run that succeeds on a key whose mapping
// declares a remedy holds its pair; an allowed provider is requested and promoted; a served remedy
// retries the blocked pair once; a blocked provider opens the next layer, at most three; a round
// stops for a person, is lifted by a changed input and expires with its window; a package's
// parameters reach every dispatch with their source. Every expected value comes from the model
// section named on each test.
//
// The catalog is neutral: `t_main`'s package maps `blocked_k` to the need `need_a` with the
// capabilities `cap_x` and `cap_y`; `t_sink_x` provides `cap_x`; `t_sink_y` provides `cap_y`,
// irreversibly, with the parameter `param_p` (`opt_1`, `opt_2`; default `opt_1`). One instance.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::{Code, Outcome};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::{
    CandidateEligibility, CatalogDocumentSource, CatalogSources, CompiledCatalog, DecisionReason,
    DecisionReasonChain, DispatchIntent, EvaluationFacts, EvaluationResources, EvaluationTime,
    FactRefreshInputs, FactRefreshSettings, FactScalar, FactValue, HostResourceSnapshot,
    ObservedFact, PolicyEvaluation, PolicyEvaluationError, REMEDY_CHOICES_FACT_KEY,
    RemedyChoiceRow, RemedyInputs, RemedyJournal, RemedyOverride, RemedyPlan, RemedyRun,
    RemedySettlement, RemedySignal, RemedyStop, RemedyTable, RemedyTableCapability,
    RemedyTableEdge, RemedyTableParameter, RemedyTableProvider, RemedyTableRemedy,
    SchedulingDecisionState, ScopeSelector, TaskDecision, TaskRuntimeSnapshot, compile_catalog,
    evaluate_with_fact_refresh, evaluate_with_remedies, plan_remedies,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
const MAIN: &str = "t_main";
const SINK_X: &str = "t_sink_x";
const SINK_Y: &str = "t_sink_y";
const SINK_M: &str = "t_sink_m";
const MID: &str = "t_mid";
const DEEP: &str = "t_deep";
const LAST: &str = "t_last";
const OTHER: &str = "t_other";
const BLOCKED: &str = "blocked_k";
const DONE: &str = "done";
const NEED: &str = "need_a";
const CAP_X: &str = "cap_x";
const CAP_Y: &str = "cap_y";
const PARAM: &str = "param_p";
const OPT_1: &str = "opt_1";
const OPT_2: &str = "opt_2";
/// The decision of `t_main`'s root blocked run: the round.
const ROOT: &str = "decision:root";
/// The root blocked run, before the latest hourly occurrence: `t_main`'s trigger is due again.
const ROOT_AT: u64 = NOW - 2 * HOUR_MS;
/// The fixture's activity window is the whole day: it ends at the next midnight.
const WINDOW_END: u64 = (NOW / DAY_MS + 1) * DAY_MS;
/// A persistent gate the host's eligibility callback gives (the clock-slot table).
const PAUSED: &str = "policy_task_paused";
/// A transient gate the host's eligibility callback gives (the clock-slot table).
const INTERVAL: &str = "policy_activity_interval_active";

// Model §7.2-§7.3, §8: the blocked pair is held, the provider of the allowed capability is
// requested with its reason (§11 code 2) and promoted above a candidate with a higher total;
// while the request runs nothing more is requested and the pair stays held.
#[test]
fn b15e_a_blocked_pair_is_held_and_its_allowed_provider_requested_and_promoted() {
    let catalog = catalog(
        &[(MAIN, 100), (SINK_X, 50), (SINK_Y, 60), (OTHER, 300)],
        &[],
    );
    let facts = facts(Vec::new(), clocks(&[SINK_X, SINK_Y]));
    let mut runs = Runs::rooted();
    let plan = plan_of(&catalog, &facts, &inputs(table(&[CAP_X]), runs.journal()));
    let requests = requested(&plan, SINK_X);
    assert_eq!(requests.len(), 1);
    assert_values(
        &requests[0],
        &[
            ("task_id", json!(MAIN)),
            ("instance_id", json!(A)),
            ("decision_id", json!(ROOT)),
            ("outcome_key", json!(BLOCKED)),
            ("need", json!(NEED)),
            ("capability", json!(CAP_X)),
            ("capabilities_allowed", json!([CAP_X])),
            ("capabilities_declared", json!([CAP_X, CAP_Y])),
            ("remedy_source", json!("package")),
            ("remedy_depth", json!(1)),
            ("count", json!(1)),
            ("limit_count", json!(2)),
            ("irreversible", json!(false)),
        ],
    );
    assert!(
        plan.trigger_override(SINK_Y, A).is_none(),
        "only an allowed capability is requested"
    );
    assert_hold(&plan, MAIN, BLOCKED, NEED);

    let evaluation = evaluate(&catalog, &facts, &plan, admit_all);
    let intents = intents_on(&evaluation);
    assert_eq!(intents.len(), 1, "{:#?}", evaluation.dispatch_intents);
    assert_eq!(intents[0].task_id, SINK_X);
    let chain = chain_of(&evaluation, intents[0]);
    assert!(
        chain
            .reasons
            .iter()
            .any(|reason| reason.outcome == requests[0])
    );
    assert!(chain.reasons.iter().any(|reason| {
        code(&reason.outcome) == "rank_breakdown" && reason.detail.contains(" promoted=1 ")
    }));
    assert!(
        total(&evaluation, OTHER) > total(&evaluation, SINK_X),
        "the promotion, not the total, put the provider first"
    );
    assert_ne!(
        decision(&evaluation, OTHER).state,
        SchedulingDecisionState::Selected
    );

    runs.push(
        SINK_X,
        run(
            "decision:sink-x",
            NOW - 30_000,
            requests,
            RemedySettlement::InFlight,
        ),
    );
    let facts = self::facts(Vec::new(), clocks(&[SINK_X, SINK_Y, OTHER]));
    let plan = plan_of(&catalog, &facts, &inputs(table(&[CAP_X]), runs.journal()));
    assert!(plan.overrides.is_empty(), "{:?}", plan.overrides);
    assert_hold(&plan, MAIN, BLOCKED, NEED);
    let evaluation = evaluate(&catalog, &facts, &plan, admit_all);
    assert!(intents_on(&evaluation).is_empty());
    let held = decision(&evaluation, MAIN);
    assert_eq!(held.state, SchedulingDecisionState::Deferred);
    let last = held.reasons.last().expect("the hold's reason");
    assert_eq!(code(&last.outcome), "policy_remedy_waiting");
    assert_values(&last.outcome, &waiting(MAIN, BLOCKED, NEED));
}

// Model §5.3 step 1: the first source that names the need decides it entirely: the target record
// before the configuration before the mapping's default; `remedy_source` names it.
#[test]
fn b15e_the_target_record_beats_the_configuration_which_beats_the_package_default() {
    let catalog = catalog(&[(MAIN, 100), (SINK_X, 50), (SINK_Y, 60)], &[]);
    let configured = BTreeMap::from([(A.to_owned(), vec![allow(CAP_Y)])]);
    let target = choices(
        &[&[("row", "allow"), ("need", NEED), ("capability", CAP_X)]],
        NOW - 1_000,
    );
    for (case, observed, configuration, provider, unrequested, source) in [
        (
            "package default",
            Vec::new(),
            BTreeMap::new(),
            SINK_X,
            SINK_Y,
            "package",
        ),
        (
            "configuration",
            Vec::new(),
            configured.clone(),
            SINK_Y,
            SINK_X,
            "configuration",
        ),
        (
            "target record",
            vec![target],
            configured,
            SINK_X,
            SINK_Y,
            "target",
        ),
    ] {
        let facts = facts(observed, clocks(&[SINK_X, SINK_Y]));
        let mut inputs = inputs(table(&[CAP_X]), Runs::rooted().journal());
        inputs.configuration = configuration;
        let plan = plan_of(&catalog, &facts, &inputs);
        let requests = requested(&plan, provider);
        assert_eq!(requests.len(), 1, "{case}");
        assert_eq!(
            value(&requests[0], "remedy_source"),
            json!(source),
            "{case}"
        );
        assert!(plan.trigger_override(unrequested, A).is_none(), "{case}");
    }
}

// Model §5.3 step 1, §7.5: with no source naming the need the pair stops for a person
// (`no_choice`); a `none` row decides even over a package default (`choice_none`). Nothing is
// requested.
#[test]
fn b15e_no_source_or_a_none_row_stops_unchosen_and_requests_nothing() {
    let catalog = catalog(&[(MAIN, 100), (SINK_X, 50), (SINK_Y, 60)], &[]);
    let journal = Runs::rooted().journal();

    let plan = plan_of(
        &catalog,
        &facts(Vec::new(), clocks(&[SINK_X, SINK_Y])),
        &inputs(table(&[]), journal.clone()),
    );
    assert_stopped(
        &plan,
        "policy_remedy_unchosen",
        &[("remedy_problem", json!("no_choice"))],
    );

    let declined = choices(&[&[("row", "none"), ("need", NEED)]], NOW - 1_000);
    let plan = plan_of(
        &catalog,
        &facts(vec![declined], clocks(&[SINK_X, SINK_Y])),
        &inputs(table(&[CAP_X]), journal.clone()),
    );
    assert_stopped(
        &plan,
        "policy_remedy_unchosen",
        &[
            ("remedy_problem", json!("choice_none")),
            ("remedy_source", json!("target")),
        ],
    );

    let mut configured = inputs(table(&[CAP_X]), journal);
    configured.configuration = BTreeMap::from([(
        A.to_owned(),
        vec![RemedyChoiceRow::NoRemedy {
            need: NEED.to_owned(),
        }],
    )]);
    let plan = plan_of(
        &catalog,
        &facts(Vec::new(), clocks(&[SINK_X, SINK_Y])),
        &configured,
    );
    assert_stopped(
        &plan,
        "policy_remedy_unchosen",
        &[
            ("remedy_problem", json!("choice_none")),
            ("remedy_source", json!("configuration")),
        ],
    );
}

// Model §5.3, §7.5 (addendum 2(b)): a choice that names a capability the remedy does not declare
// stops the pair (`choice_invalid`); a parameter value that is not one of the provider's options
// makes the provider not requestable and, with none left, stops it (`parameter_invalid`). Unknown
// names are never ignored.
#[test]
fn b15e_a_choice_naming_an_undeclared_capability_or_option_stops_with_its_problem() {
    let catalog = catalog(&[(MAIN, 100), (SINK_X, 50), (SINK_Y, 60)], &[]);
    let journal = Runs::rooted().journal();

    let undeclared = choices(
        &[&[("row", "allow"), ("need", NEED), ("capability", "cap_z")]],
        NOW - 1_000,
    );
    let plan = plan_of(
        &catalog,
        &facts(vec![undeclared], clocks(&[SINK_X, SINK_Y])),
        &inputs(table(&[CAP_X]), journal.clone()),
    );
    assert_stopped(
        &plan,
        "policy_remedy_unchosen",
        &[
            ("remedy_problem", json!("choice_invalid")),
            ("remedy_source", json!("target")),
            ("capability", json!("cap_z")),
        ],
    );

    let mut configured = inputs(table(&[CAP_X]), journal);
    configured.configuration =
        BTreeMap::from([(A.to_owned(), vec![allow(CAP_Y), parameter("opt_9")])]);
    let plan = plan_of(
        &catalog,
        &facts(Vec::new(), clocks(&[SINK_X, SINK_Y])),
        &configured,
    );
    assert_stopped(
        &plan,
        "policy_remedy_unchosen",
        &[
            ("remedy_problem", json!("parameter_invalid")),
            ("remedy_source", json!("configuration")),
            ("capability", json!(CAP_Y)),
        ],
    );
    assert!(plan.dispatch_parameters(SINK_Y, A).is_none());
}

// Model §7.4: a requested provider's clean success serves the layer; the blocked pair is retried
// once (§11 code 3), as triggered and promoted; while the retry runs it is not retried again, and
// a clean retry resolves the round: nothing is held, requested or retried.
#[test]
fn b15e_a_served_remedy_retries_the_blocked_pair_once_and_a_clean_retry_resolves() {
    let catalog = catalog(&[(MAIN, 100), (SINK_X, 50), (SINK_Y, 60)], &[]);
    // The root blocked run is after the latest hourly occurrence: t_main is not due on its own.
    let root_at = NOW - 50_000;
    let facts = facts(
        Vec::new(),
        vec![
            dispatched(MAIN, root_at),
            dispatched(SINK_X, NOW - 40_000),
            dispatched(SINK_Y, NOW - 30_000),
        ],
    );
    let mut runs = Runs::default();
    runs.push(MAIN, run(ROOT, root_at, Vec::new(), ended(BLOCKED)));
    let inputs_of = |runs: &Runs| inputs(table(&[CAP_X]), runs.journal());

    let requests = requested(&plan_of(&catalog, &facts, &inputs_of(&runs)), SINK_X);
    runs.push(
        SINK_X,
        run("decision:sink-x", NOW - 40_000, requests, ended(DONE)),
    );
    let plan = plan_of(&catalog, &facts, &inputs_of(&runs));
    let retry = retried(&plan, MAIN);
    assert_values(
        &retry,
        &[
            ("task_id", json!(MAIN)),
            ("instance_id", json!(A)),
            ("decision_id", json!(ROOT)),
            ("outcome_key", json!(BLOCKED)),
            ("capability", json!(CAP_X)),
            ("remedy_depth", json!(1)),
            ("count", json!(1)),
            ("limit_count", json!(2)),
        ],
    );
    assert_eq!(plan.overrides.len(), 1, "{:?}", plan.overrides);
    assert!(plan.deferral(MAIN, A).is_none());
    let evaluation = evaluate(&catalog, &facts, &plan, admit_all);
    let intents = intents_on(&evaluation);
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].task_id, MAIN);
    assert!(
        chain_of(&evaluation, intents[0])
            .reasons
            .iter()
            .any(|reason| reason.outcome == retry)
    );

    runs.push(
        MAIN,
        run(
            "decision:retry",
            NOW - 20_000,
            vec![retry],
            RemedySettlement::InFlight,
        ),
    );
    let plan = plan_of(&catalog, &facts, &inputs_of(&runs));
    assert!(plan.overrides.is_empty(), "{:?}", plan.overrides);
    assert_hold(&plan, MAIN, BLOCKED, NEED);

    runs.settle_last(MAIN, ended(DONE));
    let plan = plan_of(&catalog, &facts, &inputs_of(&runs));
    assert!(plan.overrides.is_empty(), "{:?}", plan.overrides);
    assert!(plan.deferrals.is_empty(), "{:?}", plan.deferrals);
    assert!(plan.signals.is_empty(), "{:?}", plan.signals);
}

// Model §7.4: any successful run of a provider of the allowed set admitted after the layer's last
// blocked run serves it, requested or not; one admitted before the block serves nothing.
#[test]
fn b15e_a_provider_running_on_its_own_trigger_serves_the_remedy() {
    let catalog = catalog(&[(MAIN, 100), (SINK_X, 50), (SINK_Y, 60)], &[]);
    let facts = facts(Vec::new(), clocks(&[SINK_X, SINK_Y]));
    let mut runs = Runs::default();
    runs.push(
        SINK_X,
        run(
            "decision:sink-x-before",
            ROOT_AT - HOUR_MS,
            Vec::new(),
            ended(DONE),
        ),
    );
    runs.push(MAIN, root_run());
    let inputs_of = |runs: &Runs| inputs(table(&[CAP_X]), runs.journal());

    let plan = plan_of(&catalog, &facts, &inputs_of(&runs));
    assert_eq!(requested(&plan, SINK_X).len(), 1);
    assert!(plan.trigger_override(MAIN, A).is_none());

    runs.push(
        SINK_X,
        run("decision:sink-x-own", NOW - 30_000, Vec::new(), ended(DONE)),
    );
    let plan = plan_of(&catalog, &facts, &inputs_of(&runs));
    let retry = retried(&plan, MAIN);
    assert_eq!(value(&retry, "capability"), json!(CAP_X));
    assert_eq!(value(&retry, "count"), json!(0));
    assert!(plan.trigger_override(SINK_X, A).is_none());
}

// Model §7.4: a retry that ends blocked again starts a new cycle of the layer while it has
// requests left (the second request counts 2); once the layer used `max_per_round` (2) requests
// and the block stays, the round stops (§11 code 6, `round_limit`).
#[test]
fn b15e_blocked_again_at_the_limit_stops_exhausted_round_limit() {
    let catalog = catalog(&[(MAIN, 100), (SINK_X, 50), (SINK_Y, 60)], &[]);
    let facts = facts(Vec::new(), clocks(&[SINK_X, SINK_Y]));
    let inputs_of = |runs: &Runs| inputs(table(&[CAP_X]), runs.journal());
    let mut runs = Runs::rooted();

    let first = requested(&plan_of(&catalog, &facts, &inputs_of(&runs)), SINK_X);
    runs.push(
        SINK_X,
        run("decision:request-1", ROOT_AT + 60_000, first, ended(DONE)),
    );
    let retry = retried(&plan_of(&catalog, &facts, &inputs_of(&runs)), MAIN);
    runs.push(
        MAIN,
        run(
            "decision:retry-1",
            ROOT_AT + 120_000,
            vec![retry],
            ended(BLOCKED),
        ),
    );
    let second = requested(&plan_of(&catalog, &facts, &inputs_of(&runs)), SINK_X);
    assert_eq!(value(&second[0], "count"), json!(2));
    runs.push(
        SINK_X,
        run("decision:request-2", ROOT_AT + 180_000, second, ended(DONE)),
    );
    let retry = retried(&plan_of(&catalog, &facts, &inputs_of(&runs)), MAIN);
    assert_eq!(value(&retry, "count"), json!(2));
    runs.push(
        MAIN,
        run(
            "decision:retry-2",
            ROOT_AT + 240_000,
            vec![retry],
            ended(BLOCKED),
        ),
    );

    let plan = plan_of(&catalog, &facts, &inputs_of(&runs));
    assert_stopped(
        &plan,
        "policy_remedy_exhausted",
        &[
            ("remedy_problem", json!("round_limit")),
            ("count", json!(2)),
            ("limit_count", json!(2)),
            ("remedy_depth", json!(1)),
        ],
    );
}

// Model §7.1, §7.4: a requested provider that ends blocked on a key with a remedy becomes the next
// layer: its own need is requested at depth 2 and both blocked pairs are held. A provider of the
// third layer that ends blocked never opens a fourth: the round stops (`depth_limit`, depth 3) and
// the chain's other pairs return to their own triggers.
#[test]
fn b15e_a_blocked_provider_opens_the_next_layer_and_layer_four_stops_depth_limit() {
    let catalog = catalog(
        &[(MAIN, 100), (SINK_X, 50), (MID, 40), (DEEP, 30), (LAST, 20)],
        &[],
    );
    let facts = facts(Vec::new(), clocks(&[SINK_X, MID, DEEP, LAST]));
    let chain = RemedyTable {
        remedies: vec![
            remedy(MAIN, BLOCKED, NEED, &[CAP_X], &[CAP_X]),
            remedy(SINK_X, "blocked_m", "need_b", &["cap_m"], &["cap_m"]),
            remedy(MID, "blocked_n", "need_c", &["cap_n"], &["cap_n"]),
            remedy(DEEP, "blocked_o", "need_d", &["cap_o"], &["cap_o"]),
        ],
        providers: vec![
            provides(SINK_X, CAP_X),
            provides(MID, "cap_m"),
            provides(DEEP, "cap_n"),
            provides(LAST, "cap_o"),
        ],
        refused_edges: Vec::new(),
    };
    let inputs_of = |runs: &Runs| inputs(chain.clone(), runs.journal());
    let mut runs = Runs::rooted();

    let first = requested(&plan_of(&catalog, &facts, &inputs_of(&runs)), SINK_X);
    runs.push(
        SINK_X,
        run(
            "decision:layer-1",
            ROOT_AT + 60_000,
            first,
            ended("blocked_m"),
        ),
    );
    let plan = plan_of(&catalog, &facts, &inputs_of(&runs));
    let second = requested(&plan, MID);
    assert_values(
        &second[0],
        &[
            ("task_id", json!(SINK_X)),
            ("instance_id", json!(A)),
            ("decision_id", json!(ROOT)),
            ("outcome_key", json!("blocked_m")),
            ("need", json!("need_b")),
            ("capability", json!("cap_m")),
            ("capabilities_allowed", json!(["cap_m"])),
            ("capabilities_declared", json!(["cap_m"])),
            ("remedy_source", json!("package")),
            ("remedy_depth", json!(2)),
            ("count", json!(1)),
            ("limit_count", json!(2)),
            ("irreversible", json!(false)),
        ],
    );
    assert_hold(&plan, MAIN, BLOCKED, NEED);
    assert_hold(&plan, SINK_X, "blocked_m", "need_b");

    runs.push(
        MID,
        run(
            "decision:layer-2",
            ROOT_AT + 120_000,
            second,
            ended("blocked_n"),
        ),
    );
    let third = requested(&plan_of(&catalog, &facts, &inputs_of(&runs)), DEEP);
    assert_eq!(value(&third[0], "remedy_depth"), json!(3));
    runs.push(
        DEEP,
        run(
            "decision:layer-3",
            ROOT_AT + 180_000,
            third,
            ended("blocked_o"),
        ),
    );

    let plan = plan_of(&catalog, &facts, &inputs_of(&runs));
    assert!(plan.trigger_override(LAST, A).is_none(), "no fourth layer");
    assert_stopped_at(
        &plan,
        "policy_remedy_exhausted",
        ("blocked_n", "need_c"),
        &[
            ("remedy_problem", json!("depth_limit")),
            ("count", json!(1)),
            ("limit_count", json!(2)),
            ("remedy_depth", json!(3)),
        ],
    );
    for task in [SINK_X, MID, DEEP] {
        assert!(plan.deferral(task, A).is_none(), "{task}");
    }
}

// Model §7.3, §9.1: a provider already in the chain is never requested (the only one left is a
// cycle: the round stops), nor a provider on an edge the table's cycle check refused.
#[test]
fn b15e_a_provider_in_the_chain_or_on_a_refused_edge_is_never_requested() {
    let catalog = catalog(
        &[(MAIN, 100), (SINK_X, 50), (SINK_Y, 60), (SINK_M, 40)],
        &[],
    );
    let facts = facts(Vec::new(), clocks(&[SINK_X, SINK_Y, SINK_M]));
    // t_main provides `cap_m`, which the second layer needs.
    let looped = |with_sink_m: bool| {
        let mut providers = vec![provides(MAIN, "cap_m"), provides(SINK_X, CAP_X)];
        if with_sink_m {
            providers.push(provides(SINK_M, "cap_m"));
        }
        RemedyTable {
            remedies: vec![
                remedy(MAIN, BLOCKED, NEED, &[CAP_X], &[CAP_X]),
                remedy(SINK_X, "blocked_m", "need_b", &["cap_m"], &["cap_m"]),
            ],
            providers,
            refused_edges: Vec::new(),
        }
    };
    let mut runs = Runs::rooted();
    let first = requested(
        &plan_of(&catalog, &facts, &inputs(looped(true), runs.journal())),
        SINK_X,
    );
    runs.push(
        SINK_X,
        run(
            "decision:layer-1",
            ROOT_AT + 60_000,
            first,
            ended("blocked_m"),
        ),
    );

    let plan = plan_of(&catalog, &facts, &inputs(looped(true), runs.journal()));
    assert_eq!(requested(&plan, SINK_M).len(), 1);
    assert!(plan.trigger_override(MAIN, A).is_none());
    assert_hold(&plan, MAIN, BLOCKED, NEED);

    let plan = plan_of(&catalog, &facts, &inputs(looped(false), runs.journal()));
    assert!(plan.trigger_override(MAIN, A).is_none());
    assert_stopped_at(
        &plan,
        "policy_remedy_unavailable",
        ("blocked_m", "need_b"),
        &[
            ("remedy_problem", json!("cycle")),
            ("remedy_source", json!("package")),
        ],
    );

    let mut refused = table(&[CAP_X, CAP_Y]);
    refused.refused_edges = vec![RemedyTableEdge {
        blocked_task_id: MAIN.to_owned(),
        provider_task_id: SINK_Y.to_owned(),
    }];
    let plan = plan_of(&catalog, &facts, &inputs(refused, Runs::rooted().journal()));
    assert_eq!(requested(&plan, SINK_X).len(), 1);
    assert!(plan.trigger_override(SINK_Y, A).is_none());
}

// Model §7.3, §7.5 (brief requirement 6): every requested provider behind a persistent gate (the
// host's `policy_task_paused`, or a disabled instance override at placement) stops the round's
// root (`policy_remedy_unavailable`, `provider_unavailable`), signalled once; a transient gate only
// makes the blocked pair wait, with the gate's code and its next eligible time.
#[test]
fn b15e_persistent_gates_stop_unavailable_and_transient_gates_wait() {
    let tasks = [(MAIN, 100), (SINK_X, 50), (SINK_Y, 60)];
    let facts = facts(Vec::new(), clocks(&[SINK_X, SINK_Y]));
    let inputs = inputs(table(&[CAP_X]), Runs::rooted().journal());

    let plain = catalog(&tasks, &[]);
    let disabled = catalog(&tasks, &[SINK_X]);
    for (case, catalog, eligibility) in [
        ("paused by the host", &plain, Gate::Persistent),
        ("disabled at placement", &disabled, Gate::Open),
    ] {
        let plan = plan_of(catalog, &facts, &inputs);
        assert_eq!(requested(&plan, SINK_X).len(), 1, "{case}");
        let evaluation = evaluate(catalog, &facts, &plan, |intent| eligibility.answer(intent));
        assert!(intents_on(&evaluation).is_empty(), "{case}");
        let main = decision(&evaluation, MAIN);
        assert_eq!(main.state, SchedulingDecisionState::Deferred, "{case}");
        let stop = main.reasons.last().expect("the stop").outcome.clone();
        assert_eq!(code(&stop), "policy_remedy_unavailable", "{case}");
        let mut expected = stop_values(BLOCKED, NEED, value(&stop, "remedy_inputs_digest"));
        expected.extend([
            ("remedy_problem", json!("provider_unavailable")),
            ("remedy_source", json!("package")),
        ]);
        assert_values(&stop, &expected);
        assert_eq!(
            plan.signals_after(&evaluation),
            vec![RemedySignal {
                signal_id: format!("signal:remedy:stopped:{}", round_hex(MAIN, ROOT)),
                task_id: MAIN.to_owned(),
                instance_id: A.to_owned(),
                outcome: stop,
            }],
            "{case}"
        );
    }

    let plan = plan_of(&plain, &facts, &inputs);
    let evaluation = evaluate(&plain, &facts, &plan, |intent| {
        Gate::Transient.answer(intent)
    });
    let main = decision(&evaluation, MAIN);
    let hold = &main.reasons.last().expect("the hold").outcome;
    assert_eq!(code(hold), "policy_remedy_waiting");
    let mut expected = waiting(MAIN, BLOCKED, NEED);
    expected.extend([
        ("gate_code", json!(INTERVAL)),
        ("next_eligible_unix_ms", json!(NOW + 120_000)),
    ]);
    assert_values(hold, &expected);
    assert!(plan.signals_after(&evaluation).is_empty());
}

// Model §7.6: a recorded stop whose inputs digest is unchanged keeps the pair stopped, without a
// new signal, also when a fact the digest does not cover changes; a written choice, or a person's
// republication of the same rows, changes the digest and lifts the stop once
// (`policy_remedy_lifted`): the pair follows its own trigger again.
#[test]
fn b15e_a_changed_input_digest_lifts_a_stop_and_an_unchanged_one_keeps_it() {
    let catalog = catalog(&[(MAIN, 100), (SINK_X, 50), (SINK_Y, 60)], &[]);
    let journal = Runs::rooted().journal();
    let observed = |extra: Vec<ObservedFact>| facts(extra, clocks(&[SINK_X, SINK_Y]));

    let unchosen = inputs(table(&[]), journal.clone());
    let first = plan_of(&catalog, &observed(Vec::new()), &unchosen);
    let stop = assert_stopped(
        &first,
        "policy_remedy_unchosen",
        &[("remedy_problem", json!("no_choice"))],
    );
    let recorded = recorded(&unchosen, &first, &stop);
    for unrelated in [Vec::new(), vec![note()]] {
        let plan = plan_of(&catalog, &observed(unrelated), &recorded);
        assert!(plan.signals.is_empty(), "{:?}", plan.signals);
        assert!(plan.overrides.is_empty());
        assert_eq!(
            plan.deferral(MAIN, A)
                .map(|deferral| &deferral.reason.outcome),
            Some(&stop)
        );
    }
    let allowed = choices(
        &[&[("row", "allow"), ("need", NEED), ("capability", CAP_X)]],
        NOW - 1_000,
    );
    let plan = plan_of(&catalog, &observed(vec![allowed]), &recorded);
    assert_lifted(&plan, &stop);

    let declining = inputs(table(&[CAP_X]), journal);
    let declined = |observed_at| choices(&[&[("row", "none"), ("need", NEED)]], observed_at);
    let first = plan_of(&catalog, &observed(vec![declined(NOW - 5_000)]), &declining);
    let stop = assert_stopped(
        &first,
        "policy_remedy_unchosen",
        &[
            ("remedy_problem", json!("choice_none")),
            ("remedy_source", json!("target")),
        ],
    );
    let recorded = self::recorded(&declining, &first, &stop);
    let kept = plan_of(&catalog, &observed(vec![declined(NOW - 5_000)]), &recorded);
    assert!(kept.signals.is_empty(), "{:?}", kept.signals);
    assert!(kept.deferral(MAIN, A).is_some());
    let republished = plan_of(&catalog, &observed(vec![declined(NOW - 1_000)]), &recorded);
    assert_lifted(&republished, &stop);
}

// Model §7.7: a round neither resolved nor stopped when its root's activity window ends expires,
// once (`policy_remedy_expired`): nothing is held or requested; recorded, it is not written again.
#[test]
fn b15e_a_round_still_open_at_its_window_end_expires_once() {
    let catalog = catalog(&[(MAIN, 100), (SINK_X, 50), (SINK_Y, 60)], &[]);
    let facts = facts(Vec::new(), clocks(&[SINK_X, SINK_Y]));
    let window_end = NOW + HOUR_MS;
    let mut runs = Runs::default();
    runs.push(
        MAIN,
        RemedyRun {
            activity_window_end_unix_ms: window_end,
            ..root_run()
        },
    );
    let mut inputs = inputs(table(&[CAP_X]), runs.journal());

    let open = plan_at(&catalog, &facts, &inputs, window_end - 1);
    assert_eq!(requested(&open, SINK_X).len(), 1);
    assert!(open.signals.is_empty());

    let expired = plan_at(&catalog, &facts, &inputs, window_end);
    assert!(expired.overrides.is_empty(), "{:?}", expired.overrides);
    assert!(expired.deferrals.is_empty(), "{:?}", expired.deferrals);
    assert_eq!(expired.signals.len(), 1, "{:?}", expired.signals);
    let signal = &expired.signals[0];
    assert_eq!(
        signal.signal_id,
        format!("signal:remedy:expired:{}", round_hex(MAIN, ROOT))
    );
    assert_eq!(
        (signal.task_id.as_str(), signal.instance_id.as_str()),
        (MAIN, A)
    );
    assert_eq!(code(&signal.outcome), "policy_remedy_expired");
    assert_values(
        &signal.outcome,
        &[
            ("task_id", json!(MAIN)),
            ("instance_id", json!(A)),
            ("decision_id", json!(ROOT)),
            ("outcome_key", json!(BLOCKED)),
            ("need", json!(NEED)),
            ("window_end_unix_ms", json!(window_end)),
        ],
    );

    inputs.signals_seen.insert(signal.signal_id.clone());
    let later = plan_at(&catalog, &facts, &inputs, window_end + HOUR_MS);
    assert!(later.signals.is_empty(), "{:?}", later.signals);
    assert!(later.overrides.is_empty());
    assert!(later.deferrals.is_empty());
}

// Model §5.3 step 2, §6: every dispatch of a task whose package declares parameters carries
// `policy_dispatch_parameters` with each value and its source: the target record, else the
// configuration, else the declared default; a normal run of the provider and a remedy request alike.
#[test]
fn b15e_run_parameters_carry_each_value_with_its_source_on_every_dispatch() {
    let catalog = catalog(&[(MAIN, 100), (SINK_Y, 60)], &[]);
    let target = choices(
        &[&[
            ("row", "parameter"),
            ("capability", CAP_Y),
            ("parameter", PARAM),
            ("value", OPT_1),
        ]],
        NOW - 1_000,
    );
    for (case, observed, configuration, chosen, source) in [
        ("package default", Vec::new(), Vec::new(), OPT_1, "package"),
        (
            "configuration",
            Vec::new(),
            vec![parameter(OPT_2)],
            OPT_2,
            "configuration",
        ),
        (
            "target record",
            vec![target],
            vec![parameter(OPT_2)],
            OPT_1,
            "target",
        ),
    ] {
        // t_main is not due; t_sink_y runs on its own trigger.
        let facts = facts(observed, vec![dispatched(MAIN, NOW - 30_000)]);
        let mut inputs = inputs(table(&[CAP_Y]), Vec::new());
        inputs.configuration = BTreeMap::from([(A.to_owned(), configuration)]);
        let plan = plan_of(&catalog, &facts, &inputs);
        let parameters = plan
            .dispatch_parameters(SINK_Y, A)
            .unwrap_or_else(|| panic!("{case}: t_sink_y's parameters"));
        assert_eq!(code(&parameters.outcome), "policy_dispatch_parameters");
        assert_values(
            &parameters.outcome,
            &[(
                "run_parameters",
                json!([{"parameter_name": PARAM, "parameter_value": chosen, "remedy_source": source}]),
            )],
        );
        let evaluation = evaluate(&catalog, &facts, &plan, admit_all);
        let intents = intents_on(&evaluation);
        assert_eq!(intents.len(), 1, "{case}");
        assert_eq!(intents[0].task_id, SINK_Y, "{case}");
        assert!(
            chain_of(&evaluation, intents[0])
                .reasons
                .contains(parameters),
            "{case}"
        );
    }

    let facts = facts(Vec::new(), clocks(&[SINK_Y]));
    let plan = plan_of(
        &catalog,
        &facts,
        &inputs(table(&[CAP_Y]), Runs::rooted().journal()),
    );
    let requests = requested(&plan, SINK_Y);
    assert_eq!(value(&requests[0], "irreversible"), json!(true));
    let parameters = plan
        .dispatch_parameters(SINK_Y, A)
        .expect("the requested provider's parameters");
    assert_values(
        &parameters.outcome,
        &[(
            "run_parameters",
            json!([{"parameter_name": PARAM, "parameter_value": OPT_1, "remedy_source": "package"}]),
        )],
    );
    let evaluation = evaluate(&catalog, &facts, &plan, admit_all);
    let intents = intents_on(&evaluation);
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].task_id, SINK_Y);
    let chain = chain_of(&evaluation, intents[0]);
    assert!(chain.reasons.contains(parameters));
    assert!(
        chain
            .reasons
            .iter()
            .any(|reason| reason.outcome == requests[0])
    );
}

// Model §8: several allowed remedies are all requested and the existing ranking orders them: the
// higher total wins and the other is recorded as `decision_candidate_not_selected`, whichever task
// that is.
#[test]
fn b15e_two_allowed_providers_are_ranked_by_the_existing_total() {
    let facts = facts(Vec::new(), clocks(&[SINK_X, SINK_Y]));
    for ((x_priority, y_priority), winner, loser) in
        [((50, 60), SINK_Y, SINK_X), ((70, 60), SINK_X, SINK_Y)]
    {
        let catalog = catalog(
            &[(MAIN, 100), (SINK_X, x_priority), (SINK_Y, y_priority)],
            &[],
        );
        let plan = plan_of(
            &catalog,
            &facts,
            &inputs(table(&[CAP_X, CAP_Y]), Runs::rooted().journal()),
        );
        assert_eq!(requested(&plan, SINK_X).len(), 1);
        assert_eq!(requested(&plan, SINK_Y).len(), 1);
        let evaluation = evaluate(&catalog, &facts, &plan, admit_all);
        let intents = intents_on(&evaluation);
        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0].task_id, winner);
        assert!(total(&evaluation, winner) > total(&evaluation, loser));
        assert!(
            chain_of(&evaluation, intents[0])
                .reasons
                .iter()
                .any(|reason| {
                    code(&reason.outcome) == "decision_candidate_not_selected"
                        && reason.outcome.value("task_id") == Some(&json!(loser))
                }),
            "{winner}"
        );
    }
}

// Brief requirement 5: without remedies (no plan, remedies switched off, an empty table) the
// evaluation is byte for byte the one of `evaluate_with_fact_refresh`; the same inputs with
// remedies on are not, so the comparison can tell.
#[test]
fn b15e_without_remedy_inputs_the_evaluation_is_byte_identical() {
    let catalog = catalog(&[(MAIN, 100), (SINK_X, 50), (SINK_Y, 60)], &[]);
    let facts = facts(Vec::new(), clocks(&[SINK_X, SINK_Y]));
    let baseline = evaluate_with_fact_refresh::<PolicyEvaluationError>(
        &catalog,
        &facts,
        &resources(),
        at(NOW),
        11,
        &BTreeSet::new(),
        &refresh(),
        |_| Ok(CandidateEligibility::Eligible),
    )
    .expect("the baseline evaluation");
    let bytes = |evaluation: &PolicyEvaluation| {
        serde_json::to_vec(evaluation).expect("an evaluation serializes")
    };
    let mut disabled = inputs(table(&[CAP_X]), Runs::rooted().journal());
    disabled.enabled = false;
    let empty_table = inputs(RemedyTable::default(), Runs::rooted().journal());
    for (case, plan) in [
        ("no plan", RemedyPlan::default()),
        ("remedies off", plan_of(&catalog, &facts, &disabled)),
        ("empty table", plan_of(&catalog, &facts, &empty_table)),
    ] {
        assert!(plan.is_empty(), "{case}");
        let evaluation = evaluate(&catalog, &facts, &plan, admit_all);
        assert_eq!(evaluation, baseline, "{case}");
        assert_eq!(bytes(&evaluation), bytes(&baseline), "{case}");
    }

    let active = plan_of(
        &catalog,
        &facts,
        &inputs(table(&[CAP_X]), Runs::rooted().journal()),
    );
    assert!(!active.is_empty());
    assert_ne!(
        bytes(&evaluate(&catalog, &facts, &active, admit_all)),
        bytes(&baseline)
    );
}

// The host's answer to a provider's intent in `b15e_persistent_gates_stop_unavailable_and_
// transient_gates_wait`: `t_sink_x` deferred by a persistent or a transient gate, or nothing.
#[derive(Clone, Copy)]
enum Gate {
    Open,
    Persistent,
    Transient,
}

impl Gate {
    fn answer(self, intent: &DispatchIntent) -> CandidateEligibility {
        let (spelling, wake) = match self {
            Self::Open => return CandidateEligibility::Eligible,
            Self::Persistent => (PAUSED, None),
            Self::Transient => (INTERVAL, Some(NOW + 120_000)),
        };
        if intent.task_id != SINK_X {
            return CandidateEligibility::Eligible;
        }
        CandidateEligibility::Deferred {
            reason: DecisionReason {
                outcome: Outcome::relayed(spelling),
                detail: "deferred by the host".to_owned(),
            },
            next_wake_unix_ms: wake,
        }
    }
}

/// The runs of each task on A, in ledger order.
#[derive(Default)]
struct Runs(BTreeMap<&'static str, Vec<RemedyRun>>);

impl Runs {
    /// `t_main`'s root blocked run alone.
    fn rooted() -> Self {
        let mut runs = Self::default();
        runs.push(MAIN, root_run());
        runs
    }

    fn push(&mut self, task_id: &'static str, run: RemedyRun) {
        self.0.entry(task_id).or_default().push(run);
    }

    /// The latest run of `task_id` settles.
    fn settle_last(&mut self, task_id: &'static str, settled: RemedySettlement) {
        let run = self
            .0
            .get_mut(task_id)
            .and_then(|runs| runs.last_mut())
            .expect("a run to settle");
        run.settled_at_unix_ms = Some(run.admitted_at_unix_ms + 60_000);
        run.settled = settled;
    }

    fn journal(&self) -> Vec<RemedyJournal> {
        self.0
            .iter()
            .map(|(task_id, runs)| RemedyJournal {
                task_id: (*task_id).to_owned(),
                instance_id: A.to_owned(),
                runs: runs.clone(),
            })
            .collect()
    }
}

fn root_run() -> RemedyRun {
    run(ROOT, ROOT_AT, Vec::new(), ended(BLOCKED))
}

/// A run admitted at `admitted_at` in the fixture's activity window, with the remedy reasons of
/// its chain; a settled one settled a minute later.
fn run(
    decision_id: &str,
    admitted_at: u64,
    remedy_reasons: Vec<Outcome>,
    settled: RemedySettlement,
) -> RemedyRun {
    let settled_at_unix_ms =
        (!matches!(settled, RemedySettlement::InFlight)).then_some(admitted_at + 60_000);
    RemedyRun {
        decision_id: decision_id.to_owned(),
        admitted_at_unix_ms: admitted_at,
        activity_window_end_unix_ms: WINDOW_END,
        remedy_reasons,
        settled,
        settled_at_unix_ms,
    }
}

fn ended(outcome_key: &str) -> RemedySettlement {
    RemedySettlement::Succeeded {
        outcome_key: Some(outcome_key.to_owned()),
    }
}

fn remedy(
    task_id: &str,
    outcome_key: &str,
    need: &str,
    capabilities: &[&str],
    default: &[&str],
) -> RemedyTableRemedy {
    RemedyTableRemedy {
        task_id: task_id.to_owned(),
        outcome_key: outcome_key.to_owned(),
        need: need.to_owned(),
        capabilities: capabilities.iter().map(|item| (*item).to_owned()).collect(),
        default: default.iter().map(|item| (*item).to_owned()).collect(),
        problem: None,
    }
}

/// A provider of one reversible capability without parameters.
fn provides(task_id: &str, capability: &str) -> RemedyTableProvider {
    RemedyTableProvider {
        task_id: task_id.to_owned(),
        provides: vec![RemedyTableCapability {
            capability: capability.to_owned(),
            irreversible: false,
            parameters: Vec::new(),
        }],
    }
}

/// The neutral table, with `t_main`'s package default.
fn table(default: &[&str]) -> RemedyTable {
    RemedyTable {
        remedies: vec![remedy(MAIN, BLOCKED, NEED, &[CAP_X, CAP_Y], default)],
        providers: vec![
            provides(SINK_X, CAP_X),
            RemedyTableProvider {
                task_id: SINK_Y.to_owned(),
                provides: vec![RemedyTableCapability {
                    capability: CAP_Y.to_owned(),
                    irreversible: true,
                    parameters: vec![RemedyTableParameter {
                        name: PARAM.to_owned(),
                        options: vec![OPT_1.to_owned(), OPT_2.to_owned()],
                        default: OPT_1.to_owned(),
                    }],
                }],
            },
        ],
        refused_edges: Vec::new(),
    }
}

/// Remedies on, `max_per_round` 2, no recorded stop or signal, no configuration rows; the host's
/// persistent gate `policy_task_paused`.
fn inputs(table: RemedyTable, journal: Vec<RemedyJournal>) -> RemedyInputs {
    RemedyInputs {
        enabled: true,
        max_per_round: 2,
        table,
        journal,
        stops: Vec::new(),
        signals_seen: BTreeSet::new(),
        configuration: BTreeMap::new(),
        persistent_gates: vec![Code::relayed(PAUSED).expect("a registered code")],
    }
}

/// The inputs after the host recorded the plan's stop `stop`.
fn recorded(inputs: &RemedyInputs, plan: &RemedyPlan, stop: &Outcome) -> RemedyInputs {
    let mut recorded = inputs.clone();
    recorded.stops = vec![RemedyStop {
        task_id: MAIN.to_owned(),
        instance_id: A.to_owned(),
        decision_id: ROOT.to_owned(),
        remedy_inputs_digest: value(stop, "remedy_inputs_digest")
            .as_str()
            .expect("a digest")
            .to_owned(),
        outcome: stop.clone(),
    }];
    recorded.signals_seen = plan
        .signals
        .iter()
        .map(|signal| signal.signal_id.clone())
        .collect();
    recorded
}

fn allow(capability: &str) -> RemedyChoiceRow {
    RemedyChoiceRow::Allow {
        need: NEED.to_owned(),
        capability: capability.to_owned(),
    }
}

/// A configuration row that sets `t_sink_y`'s parameter.
fn parameter(value: &str) -> RemedyChoiceRow {
    RemedyChoiceRow::Parameter {
        capability: CAP_Y.to_owned(),
        parameter: PARAM.to_owned(),
        value: value.to_owned(),
    }
}

/// A's target record with these rows, observed at `observed_at`, without expiry.
fn choices(rows: &[&[(&str, &str)]], observed_at: u64) -> ObservedFact {
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: REMEDY_CHOICES_FACT_KEY.to_owned(),
        value: FactValue::RecordList(
            rows.iter()
                .map(|row| {
                    row.iter()
                        .map(|(field, text)| {
                            ((*field).to_owned(), FactScalar::String((*text).to_owned()))
                        })
                        .collect()
                })
                .collect(),
        ),
        observed_at_unix_ms: observed_at,
        expires_at_unix_ms: None,
        confidence_milli: 1_000,
    }
}

/// An instance fact the remedy inputs do not cover.
fn note() -> ObservedFact {
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: "session.note".to_owned(),
        value: FactValue::Integer(1),
        observed_at_unix_ms: NOW - 1_000,
        expires_at_unix_ms: None,
        confidence_milli: 1_000,
    }
}

/// `t_main` last ran at the root blocked run (its trigger is due again); each of `recent` ran just
/// now (its trigger is not due).
fn clocks(recent: &[&str]) -> Vec<TaskRuntimeSnapshot> {
    let mut tasks = vec![dispatched(MAIN, ROOT_AT)];
    tasks.extend(
        recent
            .iter()
            .map(|task_id| dispatched(task_id, NOW - 30_000)),
    );
    tasks
}

fn dispatched(task_id: &str, at: u64) -> TaskRuntimeSnapshot {
    TaskRuntimeSnapshot {
        task_id: task_id.to_owned(),
        instance_id: A.to_owned(),
        last_dispatched_unix_ms: Some(at),
        eligible_since_unix_ms: None,
        terminal_state: None,
        completed_window: None,
        last_duration_ms: None,
        failure_streak: None,
    }
}

fn facts(observed: Vec<ObservedFact>, tasks: Vec<TaskRuntimeSnapshot>) -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-remedies".to_owned(),
        facts: observed,
        outcomes: Vec::new(),
        tasks,
        instances: vec![instance(A, &[])],
        priority_offsets: Vec::new(),
    }
}

fn at(unix_ms: u64) -> EvaluationTime {
    EvaluationTime {
        unix_ms,
        monotonic_ms: unix_ms,
    }
}

fn plan_of(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    inputs: &RemedyInputs,
) -> RemedyPlan {
    plan_at(catalog, facts, inputs, NOW)
}

fn plan_at(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    inputs: &RemedyInputs,
    unix_ms: u64,
) -> RemedyPlan {
    plan_remedies(catalog, facts, at(unix_ms), inputs)
        .unwrap_or_else(|error| panic!("the plan of valid inputs: {error}"))
}

fn admit_all(_: &DispatchIntent) -> CandidateEligibility {
    CandidateEligibility::Eligible
}

fn evaluate(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    plan: &RemedyPlan,
    mut eligibility: impl FnMut(&DispatchIntent) -> CandidateEligibility,
) -> PolicyEvaluation {
    evaluate_with_remedies::<PolicyEvaluationError>(
        catalog,
        facts,
        &resources(),
        at(NOW),
        11,
        &BTreeSet::new(),
        &refresh(),
        plan,
        |intent| Ok(eligibility(intent)),
    )
    .unwrap_or_else(|error| panic!("the evaluation of valid inputs: {error}"))
}

fn resources() -> EvaluationResources {
    EvaluationResources {
        pools: Vec::new(),
        hosts: vec![HostResourceSnapshot {
            host_id: instance(A, &[]).host_id,
            cpu_available_milli: 1_000,
            gpu_available_milli: 1_000,
            io_available_milli: 1_000,
            host_responsiveness_basis_points: 10_000,
            third_party_pressure_basis_points: 0,
            heavy_dispatch_limit: 1,
            active_heavy_dispatches: 0,
            heavy_threshold_milli: crate::DEFAULT_HEAVY_THRESHOLD_MILLI,
        }],
    }
}

/// The fact refresh with no reader: no candidate of the fixture reads a realistic key.
fn refresh() -> FactRefreshInputs {
    FactRefreshInputs {
        settings: FactRefreshSettings {
            enabled: true,
            min_interval_ms: 300_000,
            daily_limit: 96,
            failure_backoff_ms: 600_000,
            max_consecutive_failures: 3,
        },
        readers: Vec::new(),
        instance_facts_loaded_at_unix_ms: None,
        instance_holds: BTreeMap::new(),
    }
}

/// The neutral catalog: every task runs on every instance of the fixture game, hourly, with its
/// priority; a task in `disabled` is disabled on A by its instance override.
fn catalog(tasks: &[(&str, i16)], disabled: &[&str]) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.remedies",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-remedies"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": crate::SCHEDULING_SCHEMA_VERSION,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let tasks = tasks
        .iter()
        .map(|(task_id, priority)| {
            let overrides = if disabled.contains(task_id) {
                json!([{
                    "instance_id": A,
                    "enabled": false,
                    "priority": null,
                    "strategic_weight_milli": null,
                    "load_profile": null
                }])
            } else {
                json!([])
            };
            json!({
                "id": task_id,
                "scope": {"kind": "game", "game_id": GAME},
                "entrypoint": {"operation_id": OPERATION},
                "procedure_ref": format!("procedure.{task_id}"),
                "priority": priority,
                "trigger": {
                    "kind": "clock",
                    "schedule": {
                        "kind": "interval",
                        "clock_source": {"kind": "local"},
                        "every_ms": HOUR_MS,
                        "anchor_ms": 0
                    }
                },
                "feedback_stop": {
                    "kind": "outcome",
                    "task_id": task_id,
                    "outcome_key": "completed",
                    "comparison": "eq",
                    "value": {"type": "boolean", "value": true}
                },
                "consumes": [],
                "produces": [],
                "on_failure": {
                    "action": "continue",
                    "retry_limit": 2,
                    "retry_backoff_ms": 30000,
                    "escalation_threshold": 3
                },
                "sensitive": false,
                "next_run_clamp_ms": DAY_MS,
                "yield_points": [],
                "expected_duration_ms": 60000,
                "cooldown_ms": 1000,
                "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
                "loop_budget": {"daily_limit": 24, "window_iteration_limit": 4, "max_runtime_ms": 300000},
                "strategic_weight_milli": 1000,
                "instance_overrides": overrides
            })
        })
        .collect::<Vec<_>>();
    let profile = json!({
        "id": "fixture-activity",
        "scope": {"kind": "game", "game_id": GAME},
        "windows": [{
            "weekdays": [1, 2, 3, 4, 5, 6, 7],
            "utc_offset_minutes": 0,
            "start_minute_of_day": 0,
            "end_minute_of_day": 0
        }],
        "daily_budget": 24,
        "max_window_iterations": 4,
        "session_max_ms": 7200000,
        "detection_budget": {
            "window_dispatch_limit": 4,
            "window_runtime_ms": 300000,
            "expected_duration_ms": 60000
        },
        "minimum_interval_ms": 60000,
        "maximum_interval_ms": 300000,
        "seed_source": "ledger",
        "resample_policy": "same_round_stable",
        "importance_milli": 1000,
        "goals": []
    });
    let source = |name: &str, document: &Value| {
        CatalogDocumentSource::new(
            format!("memory://fixture/{name}.json"),
            serde_json::to_vec(document).expect("fixture document bytes"),
        )
    };
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", Value::Array(tasks))),
        pools: source("pools", &document("pools", json!([]))),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the fixture catalog compiles: {:?}", failure.diagnostics()))
}

/// The `policy_remedy_requested` outcomes of `task_id`'s request override on A.
fn requested(plan: &RemedyPlan, task_id: &str) -> Vec<Outcome> {
    match plan.trigger_override(task_id, A) {
        Some(RemedyOverride::Request { reasons, promoted }) => {
            assert!(*promoted, "{task_id} is promoted");
            reasons
                .iter()
                .map(|reason| reason.outcome.clone())
                .collect()
        }
        other => panic!("{task_id} is not requested: {other:?}"),
    }
}

/// The `policy_remedy_retry` outcome of `task_id`'s retry override on A.
fn retried(plan: &RemedyPlan, task_id: &str) -> Outcome {
    match plan.trigger_override(task_id, A) {
        Some(RemedyOverride::Retry { reason }) => {
            assert_eq!(code(&reason.outcome), "policy_remedy_retry");
            reason.outcome.clone()
        }
        other => panic!("{task_id} is not retried: {other:?}"),
    }
}

/// `task_id` is held on A with `policy_remedy_waiting`, waking at its round's window end.
fn assert_hold(plan: &RemedyPlan, task_id: &str, outcome_key: &str, need: &str) {
    let deferral = plan
        .deferral(task_id, A)
        .unwrap_or_else(|| panic!("{task_id} is held"));
    assert_eq!(deferral.next_wake_unix_ms, Some(WINDOW_END), "{task_id}");
    assert_eq!(code(&deferral.reason.outcome), "policy_remedy_waiting");
    assert_values(
        &deferral.reason.outcome,
        &waiting(task_id, outcome_key, need),
    );
}

fn waiting(task_id: &str, outcome_key: &str, need: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("task_id", json!(task_id)),
        ("instance_id", json!(A)),
        ("outcome_key", json!(outcome_key)),
        ("need", json!(need)),
        ("decision_id", json!(ROOT)),
    ]
}

/// The values every stop of the round carries.
fn stop_values(outcome_key: &str, need: &str, digest: Value) -> Vec<(&'static str, Value)> {
    vec![
        ("task_id", json!(MAIN)),
        ("instance_id", json!(A)),
        ("decision_id", json!(ROOT)),
        ("outcome_key", json!(outcome_key)),
        ("need", json!(need)),
        ("remedy_inputs_digest", digest),
    ]
}

/// The round stops at its first layer; see [`assert_stopped_at`].
fn assert_stopped(plan: &RemedyPlan, code_name: &str, extra: &[(&'static str, Value)]) -> Outcome {
    assert_stopped_at(plan, code_name, (BLOCKED, NEED), extra)
}

/// `t_main` stays deferred with the stop `code_name` (no wake), nothing is requested or retried,
/// and the stop is signalled once under the round's id; the stop names the layer's key and need.
fn assert_stopped_at(
    plan: &RemedyPlan,
    code_name: &str,
    (outcome_key, need): (&str, &str),
    extra: &[(&'static str, Value)],
) -> Outcome {
    assert!(plan.overrides.is_empty(), "{:?}", plan.overrides);
    let deferral = plan.deferral(MAIN, A).expect("t_main stops");
    assert_eq!(deferral.next_wake_unix_ms, None);
    let outcome = &deferral.reason.outcome;
    assert_eq!(code(outcome), code_name);
    let digest = value(outcome, "remedy_inputs_digest");
    assert!(
        digest
            .as_str()
            .is_some_and(|digest| digest.len() == 71 && digest.starts_with("sha256:")),
        "{digest}"
    );
    let mut expected = stop_values(outcome_key, need, digest);
    expected.extend(extra.iter().map(|(key, value)| (*key, value.clone())));
    assert_values(outcome, &expected);
    assert_eq!(
        plan.signals,
        vec![RemedySignal {
            signal_id: format!("signal:remedy:stopped:{}", round_hex(MAIN, ROOT)),
            task_id: MAIN.to_owned(),
            instance_id: A.to_owned(),
            outcome: outcome.clone(),
        }]
    );
    outcome.clone()
}

/// The recorded stop `stop` is lifted once, with a digest of its own, and `t_main` is no longer
/// deferred.
fn assert_lifted(plan: &RemedyPlan, stop: &Outcome) {
    assert!(plan.deferral(MAIN, A).is_none());
    assert_eq!(plan.signals.len(), 1, "{:?}", plan.signals);
    let lifted = &plan.signals[0];
    assert_eq!(
        lifted.signal_id,
        format!("signal:remedy:lifted:{}", round_hex(MAIN, ROOT))
    );
    assert_eq!(code(&lifted.outcome), "policy_remedy_lifted");
    let digest = value(&lifted.outcome, "remedy_inputs_digest");
    assert_ne!(digest, value(stop, "remedy_inputs_digest"));
    assert_values(
        &lifted.outcome,
        &[
            ("task_id", json!(MAIN)),
            ("instance_id", json!(A)),
            ("decision_id", json!(ROOT)),
            ("remedy_inputs_digest", digest),
        ],
    );
}

/// The lowercase SHA-256 of `task \0 instance \0 decision_id` (model §7.5).
fn round_hex(task_id: &str, decision_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(task_id);
    hasher.update([0_u8]);
    hasher.update(A);
    hasher.update([0_u8]);
    hasher.update(decision_id);
    format!("{:x}", hasher.finalize())
}

/// The outcome's values are exactly these.
fn assert_values(outcome: &Outcome, expected: &[(&str, Value)]) {
    let expected = expected
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect::<Map<String, Value>>();
    assert_eq!(outcome.values(), &expected, "{outcome}");
}

fn value(outcome: &Outcome, key: &str) -> Value {
    outcome.value(key).cloned().unwrap_or(Value::Null)
}

fn code(outcome: &Outcome) -> &str {
    outcome.code().as_str()
}

fn intents_on(evaluation: &PolicyEvaluation) -> Vec<&DispatchIntent> {
    evaluation
        .dispatch_intents
        .iter()
        .filter(|intent| intent.instance_id == A)
        .collect()
}

fn chain_of<'a>(
    evaluation: &'a PolicyEvaluation,
    intent: &DispatchIntent,
) -> &'a DecisionReasonChain {
    evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the intent's reason chain")
}

fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| decision.task_id == task_id && decision.instance_id.as_deref() == Some(A))
        .unwrap_or_else(|| panic!("a decision for {task_id} on {A}"))
}

fn total(evaluation: &PolicyEvaluation, task_id: &str) -> i64 {
    decision(evaluation, task_id)
        .rank
        .as_ref()
        .unwrap_or_else(|| panic!("{task_id} is ranked"))
        .total_score
}
