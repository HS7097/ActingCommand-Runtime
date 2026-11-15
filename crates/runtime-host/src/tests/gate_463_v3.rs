// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 V3, which is Workflow #455 G-R2b with the variables in it, host half (0.12.61;
// register §1.383; contracts/runtime-interface.md, "Goal request kinds"; coordinator work order
// rd-1115/brief-463-v3-goals-view-edit.md): one application's goals read (`GoalView`) and set
// (`EditGoal`) by name.
//
// Two parts. The checks before the gate and the view's list are pure functions of a page
// (`crate::goals`), tested here on a made-up page: an accepted `goals.json` of each type, two bound
// packages that share a variable, three pools and a remedy parameter. The host's half runs, as
// `gate_463_v2` does, in composer mode over one game root on disk, a manual Runtime clock and one
// fake instance: the root's one rule is due at every evaluation and its package declares the
// variables of each scene. Every name, pool and value is made up.

use super::gate_442_rem1::write_package;
use super::gate_rl1b::{
    GAME_A, POOL_A, SERVER_A, bundle, pool, profiles, resources, rule, write_json,
};
use super::*;
use crate::goals::{
    BoundPackage, BoundParameter, EditRequest, GoalPage, check_edit, page_variables, view_entries,
};
use actingcommand_contract::{
    DeclaredInitiator, GoalDeclarationRead, GoalEdit, GoalEdited, GoalEntryState, GoalEntryType,
    GoalProblem, GoalSetBy, GoalValueSource, GoalView, GoalViewEntry, Initiator, PackageVariable,
    ResourceTargetObservation, RuntimeChannel, StandardPackGoals, TargetableResource,
    VariableProblem, VariableSource, parse_package_variables, parse_standard_pack_goals,
};
use actingcommand_policy::{
    GoalRecordEdit, ResolvedVariable, StoredGoalRecord, TargetSetter, VariableSetBy,
    edit_goal_record, stored_goal_record,
};
use serde_json::{Value, json};

// ---- The pure half: a made-up page.

const GAME: &str = "neutral";
const PAGE_INSTANCE: &str = "instance-a";
const POINTS: &str = "pool.neutral.points";
const OTHER_POOL: &str = "pool.neutral.other";
const UNVALUED: &str = "pool.neutral.unvalued";
const PACKAGE_A: &str = "neutral.pkg_a";
const PACKAGE_B: &str = "neutral.pkg_b";
const TASK_A: &str = "neutral.task_a";
const TASK_B: &str = "neutral.task_b";

fn options(items: &[&str]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|item| json!({"value": item, "title": {"en": item.replace('_', " ")}}))
            .collect(),
    )
}

/// `pick` (choice of item_a and item_b, default item_b), `floor` (integer 0 to 200 on steps of
/// 10, default 0, the model's), `order` (ordered, 1 to 2 of item_a, item_b, item_c, default
/// item_b) and `extra` (switch, default false).
fn package_a_variables() -> Value {
    json!([
        {"name": "pick", "type": "choice", "title": {"en": "Item"},
         "options": options(&["item_a", "item_b"]), "default": "item_b"},
        {"name": "floor", "type": "integer", "title": {"en": "Keep at least"},
         "min": 0, "max": 200, "step": 10, "default": 0, "default_origin": "model"},
        {"name": "order", "type": "ordered", "title": {"en": "Order"},
         "options": options(&["item_a", "item_b", "item_c"]), "min_count": 1, "max_count": 2,
         "default": ["item_b"]},
        {"name": "extra", "type": "switch", "title": {"en": "Also the extra"}, "default": false}
    ])
}

fn parsed(declaration: &Value) -> Vec<PackageVariable> {
    parse_package_variables(declaration, &[]).expect("the fixture's variables")
}

/// An accepted `goals.json` with one entry of each type and two placements, one of a variable no
/// bound package declares.
fn goals_entries() -> Value {
    json!([
        {"name": "group_a", "type": "heading", "title": {"en": "Group A"}},
        {"name": "about", "type": "note", "title": {"en": "About"}, "help": {"en": "Text."}},
        {"name": "points", "type": "resource", "governs": {"pool": POINTS},
         "title": {"en": "Points"}},
        {"name": "weight", "type": "tier", "governs": {"rules": ["neutral.spend"]},
         "title": {"en": "Weight"},
         "options": [{"value": "less", "title": {"en": "Less"}, "scale_milli": 500},
                     {"value": "normal", "title": {"en": "Normal"}, "scale_milli": 1000}],
         "default": "normal"},
        {"name": "pick", "type": "variable"},
        {"name": "missing", "type": "variable"}
    ])
}

fn goals() -> StandardPackGoals {
    let document = json!({"schema_version": "actingcommand.goals.v2", "game": GAME,
                          "entries": goals_entries()});
    parse_standard_pack_goals(&serde_json::to_vec(&document).expect("goals bytes"), GAME)
        .unwrap_or_else(|issues| panic!("the fixture goals.json reads: {issues:?}"))
}

fn targetable(resource: &str, valued: bool) -> TargetableResource {
    TargetableResource {
        resource: resource.to_owned(),
        fact_key: format!("resource.{resource}"),
        producing_tasks: vec![TASK_A.to_owned()],
        default_scale: valued.then_some(10),
        default_importance_milli: valued.then_some(100),
        scale_required: !valued,
        importance_required: !valued,
        observation: ResourceTargetObservation {
            current: Some(30),
            observed_at_unix_ms: Some(1),
            pending: None,
        },
    }
}

/// The page: `declaration`; package A with the four variables and one remedy parameter, package
/// B sharing `floor`; the points pool (capacity 120) and the other pool valued, a third unvalued;
/// `extra` does not apply in the game root.
fn page(declaration: Option<StandardPackGoals>) -> GoalPage {
    let shared = json!([package_a_variables()[1].clone()]);
    GoalPage {
        game_id: GAME.to_owned(),
        declaration,
        packages: vec![
            BoundPackage {
                task_id: TASK_A.to_owned(),
                package_id: PACKAGE_A.to_owned(),
                variables: parsed(&package_a_variables()),
                parameters: vec![BoundParameter {
                    capability: "neutral.cap".to_owned(),
                    irreversible: false,
                    name: "route_param".to_owned(),
                    options: vec!["near".to_owned(), "far".to_owned()],
                    default_option: "near".to_owned(),
                }],
            },
            BoundPackage {
                task_id: TASK_B.to_owned(),
                package_id: PACKAGE_B.to_owned(),
                variables: parsed(&shared),
                parameters: Vec::new(),
            },
        ],
        targetable: vec![
            targetable(POINTS, true),
            targetable(OTHER_POOL, true),
            targetable(UNVALUED, false),
        ],
        not_targetable: Vec::new(),
        capacities: BTreeMap::from([(POINTS.to_owned(), 120)]),
        not_applying: BTreeMap::from([("extra".to_owned(), VariableProblem::RootUnchecked)]),
    }
}

fn setter(at: u64, channel: RuntimeChannel, initiator: Option<Initiator>) -> TargetSetter {
    TargetSetter {
        set_unix_ms: at,
        channel,
        initiator,
    }
}

/// A stored v3 record: `pick` = item_a (Lab, 1000), the orphan `stray` = 5 (console, 2000), the
/// target `points` on the points pool at least 50 (Lab, 3000) and the other target `other` on
/// the other pool at least 7 (Lab, 4000).
fn stored() -> StoredGoalRecord {
    let lab = |at| setter(at, RuntimeChannel::Lab, Some(Initiator::LabCli));
    let edits = [
        (
            GoalRecordEdit::SetEntry {
                name: "pick".to_owned(),
                value: json!("item_a"),
            },
            lab(1_000),
        ),
        (
            GoalRecordEdit::SetEntry {
                name: "stray".to_owned(),
                value: json!(5),
            },
            setter(2_000, RuntimeChannel::Ui, None),
        ),
        (
            GoalRecordEdit::SetTarget {
                id: "points".to_owned(),
                resource: POINTS.to_owned(),
                at_least: 50,
            },
            lab(3_000),
        ),
        (
            GoalRecordEdit::SetTarget {
                id: "other".to_owned(),
                resource: OTHER_POOL.to_owned(),
                at_least: 7,
            },
            lab(4_000),
        ),
    ];
    let mut rows: Option<Vec<BTreeMap<String, actingcommand_contract::FactScalar>>> = None;
    for (edit, set) in edits {
        rows = Some(
            edit_goal_record(rows.as_deref(), None, PAGE_INSTANCE, &edit, set)
                .unwrap_or_else(|refusal| panic!("{edit:?}: {refusal:?}"))
                .composed
                .rows,
        );
    }
    stored_goal_record(rows.as_deref(), None, PAGE_INSTANCE)
}

fn set<'a>(name: &'a str, value_json: &'a str) -> EditRequest<'a> {
    EditRequest::Set { name, value_json }
}

/// The word `request` is refused with on the page with the stored record, and the refusal's
/// pointer and range.
#[track_caller]
fn refused(
    page: &GoalPage,
    stored: &StoredGoalRecord,
    request: EditRequest<'_>,
) -> (GoalProblem, Option<String>, Option<(i64, i64)>) {
    let refusal = check_edit(page, stored, request).expect_err("the request is refused");
    (refusal.problem, refusal.pointer, refusal.range)
}

// Each situation of the checks before the gate is its own word: a name outside the form, a value
// that is no JSON, a heading or a note, a tier, a placement no bound package declares, a remedy
// parameter, a variable that does not apply (with its word), each type's checks, a resource
// entry's and an other target's checks (a deadline, the capacity, an unvalued and an untargetable
// pool), and a clear of a name that holds nothing. What passes is the one row it writes.
#[test]
fn v3_host_each_situation_of_the_checks_is_its_own_word() {
    use GoalProblem as P;
    let page = page(Some(goals()));
    let stored = stored();
    let empty = StoredGoalRecord::Empty;
    let word = |request| refused(&page, &stored, request).0;

    assert_eq!(word(set("Not A Name", "1")), P::NameUnknown);
    assert_eq!(word(set("pick", "{")), P::ValueNotJson);
    assert_eq!(word(set("group_a", "1")), P::HoldsNoValue);
    assert_eq!(word(set("about", "1")), P::HoldsNoValue);
    assert_eq!(word(set("weight", "\"less\"")), P::TierNotBuilt);
    assert_eq!(word(set("missing", "\"x\"")), P::NameUnknown);
    assert_eq!(word(set("route_param", "\"far\"")), P::RemedyParameter);
    let not_applying = check_edit(&page, &stored, set("extra", "true")).expect_err("extra");
    assert_eq!(
        (not_applying.problem, not_applying.variable_problem),
        (P::VariableNotApplying, Some(VariableProblem::RootUnchecked))
    );

    // choice
    assert_eq!(word(set("pick", "\"item_c\"")), P::NotAnOption);
    assert_eq!(word(set("pick", "3")), P::WrongType);
    // integer
    assert_eq!(
        refused(&page, &stored, set("floor", "250")),
        (P::OutOfRange, None, Some((0, 200)))
    );
    assert_eq!(
        refused(&page, &stored, set("floor", "15")),
        (P::OffStep, None, Some((0, 200)))
    );
    assert_eq!(word(set("floor", "\"ten\"")), P::WrongType);
    // ordered
    assert_eq!(
        refused(&page, &stored, set("order", "[\"item_a\",\"item_a\"]")),
        (P::KeyRepeated, Some("/1".to_owned()), None)
    );
    assert_eq!(
        refused(&page, &stored, set("order", "[\"item_z\"]")),
        (P::NotAnOption, Some("/0".to_owned()), None)
    );
    assert_eq!(
        refused(&page, &stored, set("order", "[1]")),
        (P::WrongType, Some("/0".to_owned()), None)
    );
    assert_eq!(
        refused(&page, &stored, set("order", "[]")),
        (P::CountOutOfRange, None, Some((1, 2)))
    );
    assert_eq!(
        word(set("order", "[\"item_a\",\"item_b\",\"item_c\"]")),
        P::CountOutOfRange
    );
    assert_eq!(word(set("order", "\"item_a\"")), P::WrongType);

    // a resource entry and an other target
    assert_eq!(
        refused(&page, &stored, set("points", "{\"at_least\":500}")),
        (P::OutOfRange, Some("/at_least".to_owned()), Some((1, 120)))
    );
    assert_eq!(
        refused(
            &page,
            &stored,
            set("points", "{\"at_least\":5,\"by_unix_ms\":9}")
        ),
        (P::DeadlineUnsupported, Some("/by_unix_ms".to_owned()), None)
    );
    assert_eq!(
        refused(&page, &stored, set("points", "{\"amount\":5}")),
        (P::WrongType, Some("/amount".to_owned()), None)
    );
    assert_eq!(
        word(set("points", "{\"at_least\":0}")),
        P::OutOfRange,
        "a target is at least 1"
    );
    let unvalued = check_edit(
        &page,
        &stored,
        set(
            "spare",
            &format!("{{\"resource\":\"{UNVALUED}\",\"at_least\":5}}"),
        ),
    )
    .expect_err("an unvalued pool");
    assert_eq!(
        (unvalued.problem, unvalued.pool_id.as_deref()),
        (P::PoolUnvalued, Some(UNVALUED))
    );
    assert_eq!(
        word(set(
            "spare",
            "{\"resource\":\"pool.neutral.none\",\"at_least\":5}"
        )),
        P::PoolNotTargetable
    );
    assert_eq!(word(set("spare", "5")), P::NameUnknown);

    // clears of a name that holds nothing
    assert_eq!(
        refused(&page, &empty, EditRequest::Clear { name: "pick" }).0,
        P::ValueNotSet
    );
    assert_eq!(
        refused(&page, &empty, EditRequest::Clear { name: "points" }).0,
        P::ValueNotSet
    );
    assert_eq!(
        refused(&page, &stored, EditRequest::Clear { name: "nothing" }).0,
        P::NameUnknown
    );

    // what passes
    let passes = |request| check_edit(&page, &stored, request).expect("the request passes");
    assert_eq!(
        passes(set("pick", "\"item_b\"")),
        GoalRecordEdit::SetEntry {
            name: "pick".to_owned(),
            value: json!("item_b")
        }
    );
    assert_eq!(
        passes(set("floor", "40")),
        GoalRecordEdit::SetEntry {
            name: "floor".to_owned(),
            value: json!(40)
        }
    );
    assert_eq!(
        passes(set("order", "[\"item_c\",\"item_a\"]")),
        GoalRecordEdit::SetEntry {
            name: "order".to_owned(),
            value: json!(["item_c", "item_a"])
        }
    );
    assert_eq!(
        passes(set("points", "{\"at_least\":120}")),
        GoalRecordEdit::SetTarget {
            id: "points".to_owned(),
            resource: POINTS.to_owned(),
            at_least: 120
        }
    );
    assert_eq!(
        passes(set(
            "spare",
            &format!("{{\"resource\":\"{OTHER_POOL}\",\"at_least\":9}}")
        )),
        GoalRecordEdit::SetTarget {
            id: "spare".to_owned(),
            resource: OTHER_POOL.to_owned(),
            at_least: 9
        }
    );
    assert_eq!(
        passes(EditRequest::Clear { name: "points" }),
        GoalRecordEdit::ClearTarget {
            id: "points".to_owned()
        }
    );
    assert_eq!(
        passes(EditRequest::Clear { name: "stray" }),
        GoalRecordEdit::ClearEntry {
            name: "stray".to_owned()
        }
    );
    assert_eq!(
        passes(EditRequest::Clear { name: "other" }),
        GoalRecordEdit::ClearTarget {
            id: "other".to_owned()
        }
    );
    // Without a file a declared entry's name is no name at all.
    assert_eq!(
        refused(&page_without_file(), &stored, set("group_a", "1")).0,
        P::NameUnknown
    );
}

fn page_without_file() -> GoalPage {
    page(None)
}

fn listed<'a>(entries: &'a [GoalViewEntry], name: &str) -> &'a GoalViewEntry {
    entries
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("no entry {name}: {entries:#?}"))
}

// The view lists the accepted file's entries in their order (a heading and a note as written, a
// resource entry with the target on its pool, a tier not built with its default, a placement with
// its variable, a placement no package declares alone), then the variables no entry places in the
// order the packages declare them, a shared variable once with both packages; a variable that does
// not apply is listed `not_applied` with its word; the run's resolution gives what is in effect;
// the stored values no entry offers are the orphans and the targets on no declared pool the other
// targets. Without the file (or with a refused one) only the variables are listed.
#[test]
fn v3_host_the_view_lists_the_file_then_the_unplaced_variables_and_the_orphans() {
    let page = page(Some(goals()));
    let stored = stored();
    let resolved_a = vec![ResolvedVariable {
        variable_name: "pick".to_owned(),
        value: json!("item_a"),
        source: VariableSource::Set,
        set_by: Some(VariableSetBy {
            set_unix_ms: 1_000,
            channel: RuntimeChannel::Lab,
        }),
        problem: None,
    }];
    let resolved = |task: &str| (task == TASK_A).then_some(resolved_a.as_slice());
    let variables = page_variables(&page, &resolved);
    let view = view_entries(&page, &stored, &variables, &[]);

    assert_eq!(
        view.entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.entry_type, entry.placed))
            .collect::<Vec<_>>(),
        [
            ("group_a", GoalEntryType::Heading, true),
            ("about", GoalEntryType::Note, true),
            ("points", GoalEntryType::Resource, true),
            ("weight", GoalEntryType::Tier, true),
            ("pick", GoalEntryType::Variable, true),
            ("missing", GoalEntryType::Variable, true),
            ("floor", GoalEntryType::Variable, false),
            ("order", GoalEntryType::Variable, false),
            ("extra", GoalEntryType::Variable, false),
        ]
    );
    let entries = &view.entries;
    // Each entry as the reader read it, its type's block filled in.
    assert_eq!(
        listed(entries, "about").entry,
        Some(json!({"name": "about", "type": "note", "block": "text",
                    "title": {"en": "About"}, "help": {"en": "Text."}}))
    );
    assert_eq!(
        listed(entries, "points").entry,
        Some(
            json!({"name": "points", "type": "resource", "block": "goal_row",
                    "title": {"en": "Points"}, "governs": {"pool": POINTS}})
        )
    );
    assert_eq!(
        listed(entries, "pick").entry,
        Some(json!({"name": "pick", "type": "variable"}))
    );
    let lab = Some(GoalSetBy {
        unix_ms: 1_000,
        channel: RuntimeChannel::Lab,
        initiator: Some(Initiator::LabCli),
    });

    let points = listed(entries, "points");
    assert_eq!(points.value_json.as_deref(), Some("{\"at_least\":50}"));
    assert_eq!(points.source, Some(GoalValueSource::Set));
    assert_eq!(points.state, Some(GoalEntryState::InEffect));
    let resource = points.resource.as_ref().expect("the pool's state");
    assert_eq!(
        (resource.pool.as_str(), resource.target_id.as_deref()),
        (POINTS, Some("points"))
    );
    assert_eq!(
        resource.observation.as_ref().and_then(|seen| seen.current),
        Some(30)
    );

    let weight = listed(entries, "weight");
    assert_eq!(
        (
            weight.default_json.as_deref(),
            weight.state,
            weight.source,
            weight.value_json.as_deref()
        ),
        (
            Some("\"normal\""),
            Some(GoalEntryState::NotBuilt),
            Some(GoalValueSource::Default),
            None
        )
    );

    let pick = listed(entries, "pick");
    assert_eq!(pick.packages, [PACKAGE_A]);
    assert_eq!(pick.variable.as_ref(), Some(&package_a_variables()[0]));
    assert_eq!(pick.default_json.as_deref(), Some("\"item_b\""));
    assert_eq!(pick.value_json.as_deref(), Some("\"item_a\""));
    assert_eq!(pick.effective_json.as_deref(), Some("\"item_a\""));
    assert_eq!(pick.source, Some(GoalValueSource::Set));
    assert_eq!(pick.set, lab);
    assert_eq!(pick.state, Some(GoalEntryState::InEffect));
    assert_eq!(
        pick.display,
        Some(BTreeMap::from([("en".to_owned(), "item a".to_owned())]))
    );

    let missing = listed(entries, "missing");
    assert!(missing.variable.is_none() && missing.value_json.is_none() && missing.state.is_none());

    let floor = listed(entries, "floor");
    assert_eq!(
        floor.packages,
        [PACKAGE_A, PACKAGE_B],
        "one row, both packages"
    );
    assert_eq!(
        (
            floor.default_origin.as_deref(),
            pick.default_origin.as_deref()
        ),
        (Some("model"), None),
        "the declaration's default_origin, as declared"
    );
    assert_eq!(
        (floor.effective_json.as_deref(), floor.source, floor.state),
        (
            Some("0"),
            Some(GoalValueSource::Default),
            Some(GoalEntryState::InEffect)
        )
    );

    let extra = listed(entries, "extra");
    assert_eq!(
        (extra.state, extra.variable_problem.as_deref()),
        (Some(GoalEntryState::NotApplied), Some("root_unchecked"))
    );

    assert_eq!(
        view.orphans
            .iter()
            .map(|orphan| (orphan.name.as_str(), orphan.value_json.as_str()))
            .collect::<Vec<_>>(),
        [("stray", "5")]
    );
    assert_eq!(
        view.orphans[0].set,
        Some(GoalSetBy {
            unix_ms: 2_000,
            channel: RuntimeChannel::Ui,
            initiator: None,
        })
    );
    assert_eq!(view.other_targets, ["other"]);

    // Without the file: the variables only, `pick` unplaced, and the values no variable offers
    // (`stray`) the orphans; every target is an other target.
    let bare = page_without_file();
    let variables = page_variables(&bare, &resolved);
    let view = view_entries(&bare, &stored, &variables, &[]);
    assert_eq!(
        view.entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.placed))
            .collect::<Vec<_>>(),
        [
            ("pick", false),
            ("floor", false),
            ("order", false),
            ("extra", false)
        ]
    );
    assert_eq!(view.orphans.len(), 1);
    assert_eq!(view.other_targets, ["points", "other"]);
}

// ---- The host's half.

const TASK: &str = "fixture.goals";
const INSTANCE: &str = POLICY_INSTANCE_ALIAS;
const RESULT: [u8; 3] = [255, 255, 255];
const HOME: [u8; 3] = [255, 0, 0];
const PARAMETERS: &str = "policy_dispatch_parameters";
const VIEWED: &str = "request_goal_view_completed";
const EDITED: &str = "request_edit_goal_completed";
const INVALID: &str = "goal_value_invalid";

/// The one variable `pick` of the first scene.
fn pick_only() -> Value {
    json!([{"name": "pick", "type": "choice", "title": {"en": "Item"},
            "options": options(&["item_a", "item_b"]), "default": "item_b"}])
}

/// Places the game root below `packs`: its bundle, its `schedule.json` with the one rule (due at
/// every evaluation, producing the pool, whose valuation names its step and its importance), and
/// the package, whose control declaration declares `variables`.
fn place_game_root(packs: &Path, variables: &Value) {
    let game = packs.join(GAME_A);
    write_json(
        &game.join("bundle.json"),
        &bundle(GAME_A, SERVER_A, &[TASK]),
    );
    let mut goals = rule(TASK, TASK, 100, Some(POOL_A));
    goals["trigger"] = json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"}, "every_ms": 1,
                     "anchor_ms": 0}
    });
    goals["loop_budget"]["window_iteration_limit"] = json!(16);
    goals["loop_budget"]["max_runtime_ms"] = json!(3_600_000);
    let mut activity = profiles("", Some(POOL_A));
    for profile in &mut activity {
        profile["max_window_iterations"] = json!(16);
    }
    let mut valued = pool(POOL_A);
    valued["valuation"]["gap"] = json!({"rule": "shortfall_linear", "weight_milli": 100});
    write_json(
        &game.join("schedule.json"),
        &json!({
            "schema_version": "actingcommand.schedule.v1",
            "requires": [],
            "rules": [goals],
            "pools": [valued],
            "activity": activity,
            "timeline": [],
            "held": []
        }),
    );
    let package = game.join(TASK);
    write_package(&package, TASK, None, None);
    declare(packs, variables);
}

/// Writes `variables` as the package's declared variables.
fn declare(packs: &Path, variables: &Value) {
    let control_path = packs.join(GAME_A).join(TASK).join("control.json");
    let mut control: Value =
        serde_json::from_slice(&fs::read(&control_path).expect("the control declaration"))
            .expect("control JSON");
    control["variables"] = variables.clone();
    write_json(&control_path, &control);
}

fn facts() -> EvaluationFacts {
    let mut facts = policy_facts();
    facts.outcomes.clear();
    facts.instances = vec![InstanceSnapshot {
        instance_id: INSTANCE.to_owned(),
        server_id: SERVER_A.to_owned(),
        game_id: GAME_A.to_owned(),
        host_id: "fixture-host-a".to_owned(),
        available: true,
        capability_operation_ids: vec![actingcommand_policy::STANDARD_PACK_OPERATION_ID.to_owned()],
        preferred_task_ids: Vec::new(),
    }];
    facts
}

fn lab_request(client: &TestClient, operation: RuntimeOperation) -> RuntimeRequest {
    RuntimeRequest::new(
        client.ids.mint_request_id().expect("request id"),
        client.ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Lab,
        EventSource::Lab,
        1,
        operation,
    )
    .expect("lab runtime request")
}

/// A request as the console sends it: the `ui` channel its envelope implies.
fn ui_request(client: &TestClient, operation: RuntimeOperation) -> RuntimeRequest {
    RuntimeRequest::new(
        client.ids.mint_request_id().expect("request id"),
        client.ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::User,
        EventSource::Ui,
        1,
        operation,
    )
    .expect("ui runtime request")
}

struct Scene {
    state_root: TempDir,
    packs: TempDir,
    instance: InstanceId,
    device: Arc<FakeState>,
    clock: Arc<ManualRuntimeClock>,
    host: RuntimeHost,
}

fn start_host(
    state_root: &TempDir,
    packs: &TempDir,
    clock: &Arc<ManualRuntimeClock>,
    device: &Arc<FakeState>,
    instance: InstanceId,
) -> RuntimeHost {
    RuntimeHost::start(
        config(state_root)
            .with_policy_inputs(PolicyInputSnapshot::new(facts(), policy_resources()))
            .with_runtime_clock(clock.clone())
            .with_standard_packs_root(Some(packs.path().to_owned()))
            .with_standard_pack_schedule(true),
        Arc::new(FakeProvider::one(INSTANCE, instance, Arc::clone(device)).fixture_simulation()),
    )
    .expect("start the composer-mode host")
}

impl Scene {
    fn new(variables: &Value) -> Self {
        let state_root = TempDir::new().expect("state root");
        let packs = TempDir::new().expect("standard-pack root");
        place_game_root(packs.path(), variables);
        let device = Arc::new(FakeState::default());
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let instance = instance_id();
        let host = start_host(&state_root, &packs, &clock, &device, instance);
        Self {
            state_root,
            packs,
            instance,
            device,
            clock,
            host,
        }
    }

    /// The host closed and started again on the same files, its clock at `at`.
    fn restart(self, at: u64) -> Self {
        let Self {
            state_root,
            packs,
            instance,
            device,
            host,
            ..
        } = self;
        host.close().expect("close the composer-mode host");
        let clock = Arc::new(ManualRuntimeClock::new(at, 0));
        let host = start_host(&state_root, &packs, &clock, &device, instance);
        Self {
            state_root,
            packs,
            instance,
            device,
            clock,
            host,
        }
    }

    /// The instance's goal page, read on a connection of the CLI.
    fn view(&self) -> GoalView {
        let mut client = TestClient::connect(&self.host);
        let request = client.request(RuntimeOperation::GoalView {
            instance_alias: INSTANCE.to_owned(),
        });
        let receipt = client.send(&request);
        assert_eq!(receipt.outcome().code().as_str(), VIEWED, "{receipt:?}");
        let Some(RuntimeResult::GoalViewed { view }) = receipt.result() else {
            panic!("a goal view: {receipt:?}");
        };
        (**view).clone()
    }

    /// `EditGoal` of `op` with `expected_version`, on a connection bound `lab` with `Lab-CLI`
    /// and the Lab's envelope, the Runtime clock at `at` first.
    fn edit(&self, op: GoalEdit, expected_version: Option<u64>, at: u64) -> RuntimeReceipt {
        advance_manual_clock_to(&self.clock, at);
        let mut client = TestClient::connect(&self.host);
        let bind = client.request(RuntimeOperation::BindChannel {
            channel: RuntimeChannel::Lab,
            via: None,
            initiator: Some(DeclaredInitiator::from(Initiator::LabCli)),
        });
        let bound = client.send(&bind);
        assert_eq!(bound.state(), RuntimeReceiptState::Completed, "{bound:?}");
        let request = lab_request(
            &client,
            RuntimeOperation::EditGoal {
                instance_alias: INSTANCE.to_owned(),
                expected_version,
                op,
            },
        );
        client.send(&request)
    }

    fn sample_capacity(&self) {
        self.host
            .capacity_sampler_for_test()
            .expect("capacity owner")()
        .expect("capacity sample");
    }

    /// Evaluates at `at`, admits the dispatch of the rule, runs it (`home`, one tap, `result`)
    /// and settles it: the reason chain the dispatch carried and its run's context.
    fn evaluate_and_run(&self, at: u64, seed: u64) -> (DecisionReasonChain, Box<PolicyRunContext>) {
        advance_manual_clock_to(&self.clock, at);
        self.sample_capacity();
        let evaluation = self
            .host
            .evaluate_policy_cycle_with_test_inputs(
                &facts(),
                &resources(&[POOL_A]),
                EvaluationTime {
                    unix_ms: at,
                    monotonic_ms: at,
                },
                seed,
                PolicyTrigger::FactsChanged,
            )
            .unwrap_or_else(|error| panic!("evaluate the policy at {at}: {error}"))
            .evaluation
            .expect("a full evaluation");
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == TASK && intent.instance_id == INSTANCE)
            .unwrap_or_else(|| panic!("a dispatch of {TASK}: {evaluation:#?}"))
            .clone();
        let chain = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("the intent's reason chain")
            .clone();
        self.sample_capacity();
        let admission = PolicyAdmissionContext {
            fact_ledger_position: intent.input_ledger_position,
            fact_snapshot_id: intent.fact_snapshot_id.clone(),
            approval_fact_ids: BTreeSet::new(),
            fencing_owner_epoch: self.host.runtime_info().owner_epoch(),
            now_unix_ms: intent.prerequisites.evaluated_at_unix_ms,
        };
        let request = ContainedTaskRequest::new(
            self.packs
                .path()
                .join(GAME_A)
                .join(TASK)
                .to_string_lossy()
                .into_owned(),
        )
        .expect("contained task request");
        let PolicyDispatchAdmission::Granted { context } = self
            .host
            .admit_scheduled_policy_dispatch(&intent, &chain, &admission, &request)
            .unwrap_or_else(|error| panic!("admit {TASK}: {error}"))
        else {
            panic!("expected one policy run context")
        };
        let taps = self.device.input_count.load(Ordering::Acquire);
        *self.device.pixel_script.lock().expect("pixel script") =
            Some(Arc::new(move |_: usize, inputs: usize| {
                vec![(0, if inputs <= taps { HOME } else { RESULT })]
            }));
        let receipt = self
            .host
            .run_scheduled_contained_task(&context, &request)
            .expect("the scheduled run");
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
        self.host
            .complete_scheduled_policy_run(&context, &receipt)
            .expect("the run settles");
        (chain, context)
    }

    fn close(self) {
        self.host.close().expect("close the composer-mode host");
    }
}

fn set_op(name: &str, value: &Value) -> GoalEdit {
    GoalEdit::Set {
        name: name.to_owned(),
        value_json: value.to_string(),
    }
}

fn clear_op(name: &str) -> GoalEdit {
    GoalEdit::Clear {
        name: name.to_owned(),
    }
}

/// What a taken `EditGoal` answered.
#[track_caller]
fn taken(receipt: &RuntimeReceipt) -> GoalEdited {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(receipt.outcome().code().as_str(), EDITED, "{receipt:?}");
    let Some(RuntimeResult::GoalEdited { edited }) = receipt.result() else {
        panic!("an edited goal: {receipt:?}");
    };
    edited.clone()
}

/// A refused `EditGoal`: Denied with `code`, no result; its outcome.
#[track_caller]
fn denied(receipt: &RuntimeReceipt, code: &str) -> actingcommand_contract::outcome::Outcome {
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    assert_eq!(receipt.outcome().code().as_str(), code, "{receipt:?}");
    assert!(receipt.result().is_none(), "{receipt:?}");
    receipt.outcome().clone()
}

/// The `variable_values` record of `name` on the chain's one `policy_dispatch_parameters`.
fn dispatched(chain: &DecisionReasonChain, name: &str) -> Value {
    let parameters = chain
        .reasons
        .iter()
        .filter(|reason| reason.outcome.code().as_str() == PARAMETERS)
        .collect::<Vec<_>>();
    assert_eq!(parameters.len(), 1, "{chain:?}");
    parameters[0]
        .outcome
        .value("variable_values")
        .and_then(Value::as_array)
        .and_then(|records| {
            records
                .iter()
                .find(|record| record["variable_name"] == json!(name))
        })
        .cloned()
        .unwrap_or_else(|| panic!("no value of {name}: {chain:?}"))
}

/// The view's entry `name` agrees with what the dispatch handed its run: the value in effect is
/// the record's `value_json`, the source its `variable_source`, the word its `variable_problem`.
#[track_caller]
fn agrees(view: &GoalView, chain: &DecisionReasonChain, name: &str) {
    let entry = listed(&view.entries, name);
    let record = dispatched(chain, name);
    assert_eq!(
        entry.effective_json.as_deref(),
        record["value_json"].as_str(),
        "{name}: {record}"
    );
    let source = match entry.source {
        Some(GoalValueSource::Set) => "set",
        Some(GoalValueSource::Default) => "default",
        None => "none",
    };
    assert_eq!(Some(source), record["variable_source"].as_str(), "{name}");
    assert_eq!(
        entry.variable_problem.as_deref(),
        record["variable_problem"].as_str(),
        "{name}"
    );
    assert_eq!(
        entry.state == Some(GoalEntryState::NotApplied),
        source == "default" && entry.variable_problem.is_some(),
        "{name}"
    );
}

fn run_values(context: &PolicyRunContext) -> Vec<(String, Value)> {
    crate::fact_refresh::dispatch_parameters(context)
        .expect("the run's dispatch parameters")
        .variables
}

fn next_eligible(context: &PolicyRunContext) -> u64 {
    context.admission().activity.next_eligible_unix_ms
}

// The work order's tests "the view without the file", "set, get and clear of a variable", "a
// version conflict", "not newer", "the console's gate takes both kinds", "a restart reloads the
// values" and "a value the view shows in effect is the value the next dispatch's overlay holds",
// on the variable `pick`. After the workbench's first check (the first policy cycle) the variable
// is listed unplaced. The declaration is `not_read`: the marked gap of `goal_declaration` (the
// workbench keeps no parsed `goals.json` on #1728's head `020a5681`), which "the view with the
// file and with a refused file" waits for; the pure tests below hold the view of an accepted file.
#[test]
fn v3_host_a_value_set_by_name_is_the_next_dispatch_s_and_a_restart_reloads_it() {
    let scene = Scene::new(&pick_only());
    let (chain, context) = scene.evaluate_and_run(POLICY_NOW_UNIX_MS, 6);
    let start = next_eligible(&context);
    let view = scene.view();
    agrees(&view, &chain, "pick");
    assert_eq!(
        (view.instance_alias.as_str(), view.game_id.as_str()),
        (INSTANCE, GAME_A)
    );
    assert_eq!(view.declaration.state, GoalDeclarationRead::NotRead);
    assert_eq!(view.version, 0);
    assert!(view.orphans.is_empty() && view.other_targets.is_empty());
    let names = |view: &GoalView| {
        view.entries
            .iter()
            .map(|entry| entry.name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&view), ["pick"]);
    let pick = listed(&view.entries, "pick");
    assert!(!pick.placed);
    assert_eq!(pick.packages, [TASK]);
    assert_eq!(
        (
            pick.default_json.as_deref(),
            pick.value_json.as_deref(),
            pick.effective_json.as_deref(),
            pick.source,
            pick.state
        ),
        (
            Some("\"item_b\""),
            None,
            Some("\"item_b\""),
            Some(GoalValueSource::Default),
            Some(GoalEntryState::InEffect)
        )
    );

    // Set by name through the Lab: the view names it, who set it and when; the next dispatch
    // hands it to its run, as the view said.
    let edited = taken(&scene.edit(set_op("pick", &json!("item_a")), Some(0), start));
    assert_eq!(edited.replaced_target_id, None);
    let view = scene.view();
    assert_eq!(view.version, edited.version);
    let pick = listed(&view.entries, "pick");
    assert_eq!(pick.value_json.as_deref(), Some("\"item_a\""));
    assert_eq!(pick.effective_json.as_deref(), Some("\"item_a\""));
    assert_eq!(pick.source, Some(GoalValueSource::Set));
    assert_eq!(
        pick.set,
        Some(GoalSetBy {
            unix_ms: start,
            channel: RuntimeChannel::Lab,
            initiator: Some(Initiator::LabCli),
        })
    );
    let (chain, context) = scene.evaluate_and_run(start, 7);
    agrees(&view, &chain, "pick");
    assert_eq!(run_values(&context), [("pick".to_owned(), json!("item_a"))]);

    // Another version, a value the declaration does not take, then one write and another at the
    // same instant.
    let at = next_eligible(&context);
    let conflict = denied(
        &scene.edit(set_op("pick", &json!("item_b")), Some(0), at),
        "fact_observation_version_conflict",
    );
    assert_eq!(
        conflict.value("current_version"),
        Some(&json!(edited.version))
    );
    let invalid = denied(
        &scene.edit(set_op("pick", &json!("item_z")), None, at),
        INVALID,
    );
    assert_eq!(invalid.value("goal_problem"), Some(&json!("not_an_option")));
    assert_eq!(invalid.value("goal_name"), Some(&json!("pick")));
    assert_eq!(scene.view().version, edited.version, "nothing was written");
    let second = taken(&scene.edit(set_op("pick", &json!("item_b")), Some(edited.version), at));
    assert!(second.version > edited.version);
    denied(
        &scene.edit(set_op("pick", &json!("item_a")), None, at),
        "fact_observation_not_newer",
    );
    assert_eq!(scene.view().version, second.version);

    // The console's gate takes both kinds; its clear leaves the default, set by no one.
    let mut console = TestClient::connect(&scene.host);
    let request = ui_request(
        &console,
        RuntimeOperation::GoalView {
            instance_alias: INSTANCE.to_owned(),
        },
    );
    let receipt = console.send(&request);
    assert_eq!(receipt.outcome().code().as_str(), VIEWED, "{receipt:?}");
    advance_manual_clock_to(&scene.clock, at + 1);
    let request = ui_request(
        &console,
        RuntimeOperation::EditGoal {
            instance_alias: INSTANCE.to_owned(),
            expected_version: Some(second.version),
            op: clear_op("pick"),
        },
    );
    let cleared = taken(&console.send(&request));
    let view = scene.view();
    assert_eq!(view.version, cleared.version);
    let pick = listed(&view.entries, "pick");
    assert_eq!(
        (
            pick.value_json.as_deref(),
            pick.effective_json.as_deref(),
            pick.source,
            pick.set
        ),
        (
            None,
            Some("\"item_b\""),
            Some(GoalValueSource::Default),
            None
        )
    );
    denied(&scene.edit(clear_op("pick"), None, at + 2), INVALID);

    // A restart reloads the stored value unchanged: after the start's first check the page is
    // the one before, and the dispatch hands the value to its run.
    taken(&scene.edit(set_op("pick", &json!("item_a")), None, at + 3));
    let before = scene.view();
    let scene = scene.restart(at + 4);
    let (chain, context) = scene.evaluate_and_run(at + 4, 8);
    let after = scene.view();
    assert_eq!(after.version, before.version);
    assert_eq!(after.entries, before.entries);
    assert_eq!(
        listed(&after.entries, "pick").value_json.as_deref(),
        Some("\"item_a\"")
    );
    agrees(&after, &chain, "pick");
    assert_eq!(run_values(&context), [("pick".to_owned(), json!("item_a"))]);
    scene.close();
}

// An other target (a target no resource entry of `goals.json` names): `{resource, at_least}` on
// the pool, whose valuation names its step and its importance, is one target row; another name on
// the same pool replaces it and names it; the capacity, a deadline and a pool the application
// cannot target are refused with their words; the clear removes the row, and a second clear finds
// nothing to clear. The first policy cycle composes the root first.
#[test]
fn v3_host_a_target_by_name_replaces_the_one_on_its_pool_and_is_cleared() {
    let scene = Scene::new(&pick_only());
    let (_, context) = scene.evaluate_and_run(POLICY_NOW_UNIX_MS, 6);
    let start = next_eligible(&context);
    let first = taken(&scene.edit(
        set_op("stock", &json!({"resource": POOL_A, "at_least": 50})),
        None,
        start,
    ));
    assert_eq!(first.replaced_target_id, None);
    let view = scene.view();
    assert_eq!(view.other_targets, ["stock"]);
    assert_eq!(view.version, first.version);

    let replaced = taken(&scene.edit(
        set_op("reserve", &json!({"resource": POOL_A, "at_least": 60})),
        None,
        start + 1,
    ));
    assert_eq!(replaced.replaced_target_id.as_deref(), Some("stock"));
    assert_eq!(scene.view().other_targets, ["reserve"]);

    let capacity = denied(
        &scene.edit(
            set_op("reserve", &json!({"resource": POOL_A, "at_least": 500})),
            None,
            start + 2,
        ),
        INVALID,
    );
    for (key, expected) in [
        ("goal_problem", json!("out_of_range")),
        ("json_pointer", json!("/at_least")),
        ("range_min", json!(1)),
        ("range_max", json!(120)),
        ("pool_id", json!(POOL_A)),
    ] {
        assert_eq!(capacity.value(key), Some(&expected), "{key}: {capacity}");
    }
    let deadline = denied(
        &scene.edit(
            set_op(
                "reserve",
                &json!({"resource": POOL_A, "at_least": 5, "by_unix_ms": start}),
            ),
            None,
            start + 2,
        ),
        INVALID,
    );
    assert_eq!(
        deadline.value("goal_problem"),
        Some(&json!("deadline_unsupported"))
    );
    let elsewhere = denied(
        &scene.edit(
            set_op(
                "reserve",
                &json!({"resource": "pool.neutral.none", "at_least": 5}),
            ),
            None,
            start + 2,
        ),
        INVALID,
    );
    assert_eq!(
        elsewhere.value("goal_problem"),
        Some(&json!("pool_not_targetable"))
    );

    let cleared = taken(&scene.edit(clear_op("reserve"), None, start + 3));
    let view = scene.view();
    assert!(view.other_targets.is_empty(), "{view:?}");
    assert_eq!(view.version, cleared.version);
    let again = denied(&scene.edit(clear_op("reserve"), None, start + 4), INVALID);
    assert_eq!(again.value("goal_problem"), Some(&json!("name_unknown")));
    scene.close();
}

/// The second scene's declaration before the update: `pick` (choice of item_a and item_b),
/// `floor` (integer 0 to 200 on steps of 10), `order` (ordered, 1 to 3 of item_a, item_b,
/// item_c), `extra` (switch, default false), `mode` (switch), `level` (integer 0 to 10, default 1)
/// and `gone` (switch).
fn before_update() -> Value {
    json!([
        {"name": "pick", "type": "choice", "title": {"en": "Item"},
         "options": options(&["item_a", "item_b"]), "default": "item_b"},
        {"name": "floor", "type": "integer", "title": {"en": "Keep at least"},
         "min": 0, "max": 200, "step": 10, "default": 0},
        {"name": "order", "type": "ordered", "title": {"en": "Order"},
         "options": options(&["item_a", "item_b", "item_c"]), "min_count": 1, "max_count": 3,
         "default": ["item_b"]},
        {"name": "extra", "type": "switch", "title": {"en": "Extra"}, "default": false},
        {"name": "mode", "type": "switch", "title": {"en": "Mode"}, "default": false},
        {"name": "level", "type": "integer", "title": {"en": "Level"},
         "min": 0, "max": 10, "step": 1, "default": 1},
        {"name": "gone", "type": "switch", "title": {"en": "Gone"}, "default": false}
    ])
}

/// The declaration after the update: `pick` lost item_a, `floor` ends at 100, `order` lost
/// item_c, `extra` defaults to true, `mode` is a choice, `level` defaults to 2, `gone` is gone.
fn after_update() -> Value {
    json!([
        {"name": "pick", "type": "choice", "title": {"en": "Item"},
         "options": options(&["item_b", "item_c"]), "default": "item_b"},
        {"name": "floor", "type": "integer", "title": {"en": "Keep at least"},
         "min": 0, "max": 100, "step": 10, "default": 0},
        {"name": "order", "type": "ordered", "title": {"en": "Order"},
         "options": options(&["item_a", "item_b"]), "min_count": 1, "max_count": 3,
         "default": ["item_b"]},
        {"name": "extra", "type": "switch", "title": {"en": "Extra"}, "default": true},
        {"name": "mode", "type": "choice", "title": {"en": "Mode"},
         "options": options(&["on", "off"]), "default": "off"},
        {"name": "level", "type": "integer", "title": {"en": "Level"},
         "min": 0, "max": 10, "step": 1, "default": 2}
    ])
}

// The work order's rows of 3.6 (what a pack update does to a stored value) and "a value the view
// shows in effect is the value the next dispatch's overlay holds": values set by name before an
// update of the package, then the update and a restart. An option removed (`pick`), a range
// narrowed (`floor`) and a type changed (`mode`) read the default `not_applied` with their words;
// an ordered value whose key was removed reads the rest as set, in effect with the run's word
// `keys_left_out`, and names the key left out (`order`); a changed default is followed by the value no one set (`extra`) and not by a set one
// (`level`); a variable the package no longer declares leaves its value an orphan (`gone`). Every
// variable's line is what the next dispatch hands its run, and no stored value was rewritten.
#[test]
fn v3_host_a_pack_update_leaves_each_stored_value_as_the_design_says() {
    let scene = Scene::new(&before_update());
    let (_, context) = scene.evaluate_and_run(POLICY_NOW_UNIX_MS, 6);
    let start = next_eligible(&context);
    for (offset, (name, value)) in [
        ("pick", json!("item_a")),
        ("floor", json!(150)),
        ("order", json!(["item_c", "item_a"])),
        ("mode", json!(true)),
        ("level", json!(5)),
        ("gone", json!(true)),
    ]
    .into_iter()
    .enumerate()
    {
        let at = start + u64::try_from(offset).expect("an offset");
        taken(&scene.edit(set_op(name, &value), None, at));
    }
    let set_before = scene.view();
    assert!(set_before.orphans.is_empty(), "{set_before:?}");

    declare(scene.packs.path(), &after_update());
    let scene = scene.restart(start + 10);
    let (chain, context) = scene.evaluate_and_run(start + 10, 11);
    let view = scene.view();
    assert_eq!(
        view.version, set_before.version,
        "no stored value was rewritten"
    );
    let line = |name: &str| {
        let entry = listed(&view.entries, name);
        (
            entry.value_json.clone(),
            entry.effective_json.clone(),
            entry.source,
            entry.state,
            entry.variable_problem.clone(),
        )
    };
    let text = |value: &str| Some(value.to_owned());
    use GoalEntryState as State;
    use GoalValueSource as Source;
    assert_eq!(
        line("pick"),
        (
            text("\"item_a\""),
            text("\"item_b\""),
            Some(Source::Default),
            Some(State::NotApplied),
            text("not_an_option")
        )
    );
    assert_eq!(
        line("floor"),
        (
            text("150"),
            text("0"),
            Some(Source::Default),
            Some(State::NotApplied),
            text("out_of_range")
        )
    );
    assert_eq!(
        line("mode"),
        (
            text("true"),
            text("\"off\""),
            Some(Source::Default),
            Some(State::NotApplied),
            text("type_changed")
        )
    );
    // Applied in part: the run's word, the value still set and in effect.
    assert_eq!(
        line("order"),
        (
            text("[\"item_c\",\"item_a\"]"),
            text("[\"item_a\"]"),
            Some(Source::Set),
            Some(State::InEffect),
            text("keys_left_out")
        )
    );
    assert_eq!(listed(&view.entries, "order").removed_keys, ["item_c"]);
    assert_eq!(
        line("extra"),
        (
            None,
            text("true"),
            Some(Source::Default),
            Some(State::InEffect),
            None
        )
    );
    assert_eq!(
        line("level"),
        (
            text("5"),
            text("5"),
            Some(Source::Set),
            Some(State::InEffect),
            None
        )
    );
    assert_eq!(
        view.orphans
            .iter()
            .map(|orphan| (orphan.name.as_str(), orphan.value_json.as_str()))
            .collect::<Vec<_>>(),
        [("gone", "true")]
    );
    assert!(view.entries.iter().all(|entry| entry.name != "gone"));

    // What the view shows is what the dispatch after the update handed its run.
    for name in ["pick", "floor", "order", "extra", "mode", "level"] {
        agrees(&view, &chain, name);
    }
    let read = run_values(&context);
    assert_eq!(
        read.iter()
            .find(|(name, _)| name == "order")
            .map(|(_, value)| value),
        Some(&json!(["item_a"]))
    );
    scene.close();
}
