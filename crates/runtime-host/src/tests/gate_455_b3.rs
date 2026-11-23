// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 B3 (0.12.82; register §1.519; contracts/standard-pack-goals.md, "An entry";
// contracts/resource-targets.md, "One value by name"; coordinator work order
// rd-1123/brief-455-b3.md): a `resource` entry of a version 3 `goals.json` carries the goal's own
// step and importance for a pool that declares no valuation, and a value set on it writes the
// application's goal with them. Tested on what `EditGoal` and `GoalView` run: the page's pure
// functions (`crate::goals`: the checks of an edit, the view's list and the entry as the view
// writes it), the policy crate's one-row edit of the record, the goal page's reader of the record
// and the goal document's reader, on a made-up page: seven pools the application can target, two
// valued, no package bound. Every name, pool and value is made up; no clock is read. Fix round 1
// (brief rd-1123/brief-455-b3-fix1.md, ruling 1): the goal carries an entry's number only where
// the pool's valuation does not give it.

use std::collections::BTreeMap;

use crate::goals::{EditRequest, GoalPage, GoalRefusal, check_edit, declared_entry, view_entries};
use actingcommand_contract::{
    DeclaredGoalKind, FactScalar, GoalEntryState, GoalEntryType, GoalProblem, GoalResourceState,
    GoalSetBy, GoalValueSource, GoalViewEntry, Initiator, ResourceTargetCondition,
    ResourceTargetConditionState, ResourceTargetObservation, ResourceTargetPendingReason,
    RuntimeChannel, StandardPackGoals, TargetableResource, parse_standard_pack_goals,
};
use actingcommand_policy::{
    GoalRecordEdit, RESOURCE_TARGETS_SCHEMA_VERSION_V3, StoredGoalRecord, TargetCondition,
    TargetSetter, VariableSetBy, edit_goal_record, parse_resource_targets, stored_goal_record,
};
use serde_json::{Value, json};

const GAME: &str = "neutral";
const INSTANCE: &str = "instance-a";
/// A pool without a valuation, which the entry `lacking` governs with its own step 10 and
/// importance 1000.
const LACKING: &str = "pool.neutral.lacking";
/// A pool without a valuation, which the entry `largest` governs with the largest step and
/// importance the goal document takes.
const LARGEST: &str = "pool.neutral.largest";
/// A pool without a valuation, which the entry `bare` governs with neither.
const BARE: &str = "pool.neutral.bare";
/// A pool without a valuation, which the entry `stepped` governs with its own step alone (a file
/// the Lab's check refuses; the Runtime's workbench holds no references).
const STEPPED: &str = "pool.neutral.stepped";
/// A pool whose valuation gives the step and the importance, which the entry `points` governs.
const POINTS: &str = "pool.neutral.points";
/// Fix round 1: a pool without a valuation, which the entry `weighted` governs with its own
/// importance alone (a file the Lab's check refuses).
const WEIGHTED: &str = "pool.neutral.weighted";
/// Fix round 1: a pool whose valuation gives the step and the importance, which the entry `priced`
/// governs with its own step and importance too (a file the Lab's check refuses,
/// `goal_valuation_conflict`, and that can still be placed).
const PRICED: &str = "pool.neutral.priced";
/// The largest step the goal document takes, 2^53 - 1.
const LARGEST_STEP: u64 = 9_007_199_254_740_991;

/// The contract's neutral example of `goals.json`, version 2 (contracts/standard-pack-goals.md,
/// "Neutral example").
const NEUTRAL_EXAMPLE: &str = r#"{"schema_version": "actingcommand.goals.v2", "game": "neutral",
 "entries": [
  {"name": "stock", "type": "heading", "title": {"en": "Stock", "zh-Hans": "储备"}},
  {"name": "points", "type": "resource", "governs": {"pool": "pool.neutral.points"},
   "title": {"en": "Points to hold", "zh-Hans": "要攒的点数"},
   "rate": {"floor_milli_per_day": 120000, "ceiling_milli_per_day": 900000}},
  {"name": "route", "type": "variable"}]}"#;

type Rows = Vec<BTreeMap<String, FactScalar>>;

/// The game's `goals.json`, version 3: one `resource` entry on each pool.
fn declaration() -> StandardPackGoals {
    let document = json!({"schema_version": "actingcommand.goals.v3", "game": GAME, "entries": [
        {"name": "lacking", "type": "resource", "governs": {"pool": LACKING},
         "title": {"en": "Material"}, "scale": 10, "importance_milli": 1000},
        {"name": "largest", "type": "resource", "governs": {"pool": LARGEST},
         "title": {"en": "Largest"}, "scale": LARGEST_STEP, "importance_milli": 1_000_000},
        {"name": "bare", "type": "resource", "governs": {"pool": BARE}, "title": {"en": "Bare"}},
        {"name": "stepped", "type": "resource", "governs": {"pool": STEPPED},
         "title": {"en": "Stepped"}, "scale": 10},
        {"name": "points", "type": "resource", "governs": {"pool": POINTS},
         "title": {"en": "Points"}},
        {"name": "weighted", "type": "resource", "governs": {"pool": WEIGHTED},
         "title": {"en": "Weighted"}, "importance_milli": 1000},
        {"name": "priced", "type": "resource", "governs": {"pool": PRICED},
         "title": {"en": "Priced"}, "scale": 10, "importance_milli": 1000}
    ]});
    parse_standard_pack_goals(&serde_json::to_vec(&document).expect("goals bytes"), GAME)
        .unwrap_or_else(|issues| panic!("the fixture goals.json reads: {issues:?}"))
}

/// A reading of 30, observed at 1.
fn reading() -> ResourceTargetObservation {
    ResourceTargetObservation {
        current: Some(30),
        observed_at_unix_ms: Some(1),
        pending: None,
    }
}

/// No reading of the pool yet.
fn no_reading() -> ResourceTargetObservation {
    ResourceTargetObservation {
        current: None,
        observed_at_unix_ms: None,
        pending: Some(ResourceTargetPendingReason::Missing),
    }
}

/// A pool the application can target, read as `observation`: its valuation gives a goal the step
/// and the importance when `valued`, else a goal on it must carry both.
fn pool_offered(
    resource: &str,
    valued: bool,
    observation: ResourceTargetObservation,
) -> TargetableResource {
    TargetableResource {
        resource: resource.to_owned(),
        fact_key: format!("inventory.{resource}"),
        producing_tasks: vec!["neutral.task".to_owned()],
        default_scale: valued.then_some(10),
        default_importance_milli: valued.then_some(100),
        scale_required: !valued,
        importance_required: !valued,
        observation,
    }
}

/// The page: [`declaration`], no package bound, the seven pools targetable, [`LACKING`] read as
/// `observation` and the others at 30.
fn page_with(observation: ResourceTargetObservation) -> GoalPage {
    GoalPage {
        game_id: GAME.to_owned(),
        unchecked: None,
        declaration: Some(declaration()),
        packages: Vec::new(),
        targetable: vec![
            pool_offered(LACKING, false, observation),
            pool_offered(LARGEST, false, reading()),
            pool_offered(BARE, false, reading()),
            pool_offered(STEPPED, false, reading()),
            pool_offered(POINTS, true, reading()),
            pool_offered(WEIGHTED, false, reading()),
            pool_offered(PRICED, true, reading()),
        ],
        capacities: BTreeMap::new(),
        not_applying: BTreeMap::new(),
        root: BTreeMap::new(),
    }
}

/// Who sets a value: the Lab's command line at 5000.
fn lab_setter() -> TargetSetter {
    TargetSetter {
        set_unix_ms: 5_000,
        channel: RuntimeChannel::Lab,
        initiator: Some(Initiator::LabCli),
    }
}

/// The setter a value is judged with, [`lab_setter`]'s.
fn judged_by_lab() -> VariableSetBy {
    VariableSetBy {
        set_unix_ms: 5_000,
        channel: RuntimeChannel::Lab,
    }
}

/// What the checks of `EditGoal` answer on `page` with no stored record for a `set` of `name` to
/// `value_json`.
fn set_on(page: &GoalPage, name: &str, value_json: &str) -> Result<GoalRecordEdit, GoalRefusal> {
    check_edit(
        page,
        &StoredGoalRecord::Empty,
        EditRequest::Set { name, value_json },
        judged_by_lab(),
    )
}

/// The rows `edit` writes over `stored`, set by [`lab_setter`]; the edit must be taken.
fn written(stored: Option<&Rows>, edit: &GoalRecordEdit) -> Rows {
    edit_goal_record(
        stored.map(Vec::as_slice),
        None,
        INSTANCE,
        edit,
        lab_setter(),
    )
    .unwrap_or_else(|refusal| panic!("{edit:?} is taken: {refusal:?}"))
    .composed
    .rows
}

/// The target row of `id` among `rows`.
fn target_row<'a>(rows: &'a Rows, id: &str) -> &'a BTreeMap<String, FactScalar> {
    rows.iter()
        .find(|row| {
            row.get("row") == Some(&FactScalar::String("target".to_owned()))
                && row.get("id") == Some(&FactScalar::String(id.to_owned()))
        })
        .unwrap_or_else(|| panic!("no target row {id}: {rows:?}"))
}

fn text(value: &str) -> FactScalar {
    FactScalar::String(value.to_owned())
}

/// The goal document's reader (`ApplyResourceTargets`' parse) of a version 3 document of the one
/// target `target`, as the record holds it in document form: it must take it.
fn assert_the_document_reader_takes(target: &Value) {
    let document = json!({
        "schema_version": RESOURCE_TARGETS_SCHEMA_VERSION_V3,
        "instance": INSTANCE,
        "targets": [target]
    });
    let bytes = serde_json::to_vec(&document).expect("document bytes");
    if let Err(error) = parse_resource_targets(&bytes) {
        panic!("the goal document's reader refuses {document}: {error:?}");
    }
}

// The brief's test 3: a value set on a `resource` entry that carries its own step and importance
// (`{at_least: 40}` on `lacking`, its pool declaring no valuation) is the one row `SetTarget` with
// the entry's `scale` 10 and `importance_milli` 1000. The record the policy crate writes holds
// them on the target row, the goal page's reader of the record reads them back with who set them,
// and the goal document's reader takes the target in document form; so for the largest step and
// importance the goal document takes. By the same request and the same record a value on the
// entry of a valued pool writes a target without them, as before, and the first target is kept as
// stored. An entry that carries neither, or one of the two, on a pool without a valuation is
// refused `pool_unvalued` with its pool. Clearing the entry removes its target; clearing the last
// target leaves the withdrawal, no record.
#[test]
fn b3_host_a_value_on_an_entry_with_its_own_step_and_importance_writes_them_and_clear_removes_it() {
    let page = page_with(reading());
    let lacking = set_on(&page, "lacking", "{\"at_least\":40}").expect("the value is taken");
    assert_eq!(
        lacking,
        GoalRecordEdit::SetTarget {
            id: "lacking".to_owned(),
            resource: LACKING.to_owned(),
            at_least: 40,
            scale: Some(10),
            importance_milli: Some(1000),
        }
    );
    let rows = written(None, &lacking);
    assert_eq!(
        target_row(&rows, "lacking"),
        &BTreeMap::from([
            ("row".to_owned(), text("target")),
            ("id".to_owned(), text("lacking")),
            ("resource".to_owned(), text(LACKING)),
            ("at_least".to_owned(), FactScalar::Integer(40)),
            ("mode".to_owned(), text("adjust")),
            ("weight".to_owned(), text("score_stage")),
            ("scale".to_owned(), FactScalar::Integer(10)),
            ("importance_milli".to_owned(), FactScalar::Integer(1000)),
            ("set_unix_ms".to_owned(), FactScalar::Integer(5_000)),
            ("channel".to_owned(), text("lab")),
            ("initiator".to_owned(), text("Lab-CLI")),
        ])
    );
    let targets = stored_goal_record(Some(rows.as_slice()), None, INSTANCE).targets();
    assert_eq!(targets.len(), 1, "{targets:?}");
    let stored = &targets[0];
    assert_eq!(
        (
            stored.target.id.as_str(),
            stored.target.resource.as_str(),
            stored.target.condition,
            stored.target.scale,
            stored.target.importance_milli,
            stored.set,
        ),
        (
            "lacking",
            LACKING,
            TargetCondition::AtLeast { amount: 40 },
            Some(10),
            Some(1000),
            Some(lab_setter()),
        )
    );
    let in_document_form = serde_json::to_value(&stored.target).expect("the target as JSON");
    assert_eq!(
        in_document_form,
        json!({"id": "lacking", "resource": LACKING,
               "condition": {"kind": "at_least", "amount": 40},
               "scale": 10, "importance_milli": 1000,
               "apply": {"mode": "adjust", "weight": "score_stage"}})
    );
    assert_the_document_reader_takes(&in_document_form);

    // The goal document's bounds: the largest step and importance are taken and read back.
    let largest = set_on(&page, "largest", "{\"at_least\":1}").expect("the value is taken");
    assert_eq!(
        largest,
        GoalRecordEdit::SetTarget {
            id: "largest".to_owned(),
            resource: LARGEST.to_owned(),
            at_least: 1,
            scale: Some(LARGEST_STEP),
            importance_milli: Some(1_000_000),
        }
    );
    let with_largest = written(Some(&rows), &largest);
    let largest_target = stored_goal_record(Some(with_largest.as_slice()), None, INSTANCE)
        .targets()
        .into_iter()
        .find(|kept| kept.target.id == "largest")
        .expect("the largest target is read back");
    assert_eq!(
        (
            largest_target.target.scale,
            largest_target.target.importance_milli
        ),
        (Some(LARGEST_STEP), Some(1_000_000))
    );
    assert_the_document_reader_takes(
        &serde_json::to_value(&largest_target.target).expect("the target as JSON"),
    );

    // A valued pool's entry, by the same request and record: no step or importance of its own.
    let points = set_on(&page, "points", "{\"at_least\":50}").expect("the value is taken");
    assert_eq!(
        points,
        GoalRecordEdit::SetTarget {
            id: "points".to_owned(),
            resource: POINTS.to_owned(),
            at_least: 50,
            scale: None,
            importance_milli: None,
        }
    );
    let all = written(Some(&with_largest), &points);
    let points_row = target_row(&all, "points");
    assert_eq!(
        (points_row.get("scale"), points_row.get("importance_milli")),
        (None, None),
        "{points_row:?}"
    );
    assert_eq!(
        target_row(&all, "lacking"),
        target_row(&rows, "lacking"),
        "kept as stored"
    );

    // Neither, or the step alone, on a pool without a valuation: `pool_unvalued` with its pool.
    for (name, pool_id) in [("bare", BARE), ("stepped", STEPPED)] {
        let refusal = set_on(&page, name, "{\"at_least\":40}").expect_err("the value is refused");
        assert_eq!(
            (refusal.problem, refusal.pool_id.as_deref()),
            (GoalProblem::PoolUnvalued, Some(pool_id)),
            "{name}"
        );
    }

    // Clearing the entry removes its target, the others kept; the last one cleared is no record.
    let recorded = stored_goal_record(Some(all.as_slice()), None, INSTANCE);
    let clear = check_edit(
        &page,
        &recorded,
        EditRequest::Clear { name: "lacking" },
        judged_by_lab(),
    )
    .expect("the clear is taken");
    assert_eq!(
        clear,
        GoalRecordEdit::ClearTarget {
            id: "lacking".to_owned()
        }
    );
    let cleared = written(Some(&all), &clear);
    assert_eq!(
        stored_goal_record(Some(cleared.as_slice()), None, INSTANCE)
            .targets()
            .iter()
            .map(|kept| kept.target.id.clone())
            .collect::<Vec<_>>(),
        ["largest", "points"]
    );
    let only = stored_goal_record(Some(rows.as_slice()), None, INSTANCE);
    assert_eq!(
        check_edit(
            &page,
            &only,
            EditRequest::Clear { name: "lacking" },
            judged_by_lab()
        ),
        Ok(clear.clone())
    );
    let withdrawn = written(Some(&rows), &clear);
    assert_eq!(
        stored_goal_record(Some(withdrawn.as_slice()), None, INSTANCE),
        StoredGoalRecord::Empty
    );
}

/// The entry `name` as the view lists it on `page` with the record `record` and the stored
/// targets' conditions `conditions`.
fn listed_entry(
    page: &GoalPage,
    record: &StoredGoalRecord,
    conditions: &[ResourceTargetCondition],
    name: &str,
) -> GoalViewEntry {
    view_entries(page, record, &[], conditions)
        .entries
        .into_iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("{name} is listed"))
}

/// What the page shows of the resource entry `name` on `pool_id` whose reader read it as `entry`:
/// placed and in effect, the pool read as `observation` with `gap`; with `at_least`, the value the
/// Lab set at 5000 on the target of the entry's name; without, the default, no goal.
fn shown(
    (name, pool_id): (&str, &str),
    entry: Value,
    observation: ResourceTargetObservation,
    gap: Option<u64>,
    at_least: Option<u64>,
) -> GoalViewEntry {
    let mut expected = GoalViewEntry::new(name, GoalEntryType::Resource, true);
    expected.entry = Some(entry);
    expected.state = Some(GoalEntryState::InEffect);
    expected.resource = Some(GoalResourceState {
        pool: pool_id.to_owned(),
        target_id: at_least.map(|_| name.to_owned()),
        observation: Some(observation),
        gap,
    });
    match at_least {
        Some(amount) => {
            expected.value_json = Some(format!("{{\"at_least\":{amount}}}"));
            expected.effective_json = expected.value_json.clone();
            expected.source = Some(GoalValueSource::Set);
            expected.set = Some(GoalSetBy {
                unix_ms: 5_000,
                channel: RuntimeChannel::Lab,
                initiator: Some(Initiator::LabCli),
            });
        }
        None => expected.source = Some(GoalValueSource::Default),
    }
    expected
}

// The brief's test 4: the goal page shows an entry that carries its own step and importance as it
// shows any `resource` entry. With `{at_least: 40}` set on `lacking`: placed, in effect, its entry
// as its reader read it (its `scale` and `importance_milli` in it), the value and who set it, the
// pool and the target; with a reading of 30 the reading and the gap 10, without a reading the
// pending reading and no gap. With no value: the default (no goal), the pool's reading or the
// pending one, no target and no gap. The entry of the valued pool `points` with `{at_least: 50}`
// set is shown in the same members.
#[test]
fn b3_host_the_page_shows_such_an_entry_with_and_without_a_reading() {
    let rows = written(
        None,
        &GoalRecordEdit::SetTarget {
            id: "lacking".to_owned(),
            resource: LACKING.to_owned(),
            at_least: 40,
            scale: Some(10),
            importance_milli: Some(1000),
        },
    );
    let record = stored_goal_record(Some(rows.as_slice()), None, INSTANCE);
    let entry = json!({"name": "lacking", "type": "resource", "block": "goal_row",
                       "title": {"en": "Material"}, "governs": {"pool": LACKING},
                       "scale": 10, "importance_milli": 1000});
    let condition = |state| ResourceTargetCondition {
        target_id: "lacking".to_owned(),
        resource: LACKING.to_owned(),
        fact_key: format!("inventory.{LACKING}"),
        state,
    };
    let computed = condition(ResourceTargetConditionState::Computed {
        current: 30,
        observed_at_unix_ms: 1,
        gap: 10,
    });
    let awaiting = condition(ResourceTargetConditionState::AwaitingObservation {
        reason: ResourceTargetPendingReason::Missing,
    });
    let lacking = ("lacking", LACKING);

    assert_eq!(
        listed_entry(&page_with(reading()), &record, &[computed], "lacking"),
        shown(lacking, entry.clone(), reading(), Some(10), Some(40)),
        "a value set, with a reading"
    );
    assert_eq!(
        listed_entry(&page_with(no_reading()), &record, &[awaiting], "lacking"),
        shown(lacking, entry.clone(), no_reading(), None, Some(40)),
        "a value set, without a reading"
    );
    assert_eq!(
        listed_entry(
            &page_with(reading()),
            &StoredGoalRecord::Empty,
            &[],
            "lacking"
        ),
        shown(lacking, entry.clone(), reading(), None, None),
        "no value, with a reading"
    );
    assert_eq!(
        listed_entry(
            &page_with(no_reading()),
            &StoredGoalRecord::Empty,
            &[],
            "lacking"
        ),
        shown(lacking, entry, no_reading(), None, None),
        "no value, without a reading"
    );

    let with_points = written(
        None,
        &GoalRecordEdit::SetTarget {
            id: "points".to_owned(),
            resource: POINTS.to_owned(),
            at_least: 50,
            scale: None,
            importance_milli: None,
        },
    );
    assert_eq!(
        listed_entry(
            &page_with(reading()),
            &stored_goal_record(Some(with_points.as_slice()), None, INSTANCE),
            &[],
            "points"
        ),
        shown(
            ("points", POINTS),
            json!({"name": "points", "type": "resource", "block": "goal_row",
                   "title": {"en": "Points"}, "governs": {"pool": POINTS}}),
            reading(),
            None,
            Some(50)
        ),
        "the entry of a valued pool"
    );
}

// The brief's test 5, the host's half: an entry without the new members is written by the view as
// before B3. The resource entry of the contract's neutral example (version 2), as the view writes
// what its reader read, is the golden text below, the bytes of main; the same file labelled
// version 3 reads to the same declaration; and the entry `lacking`, which carries the members, is
// written with them after its own members.
#[test]
fn b3_host_an_entry_without_the_members_is_written_as_before() {
    let version_2 = parse_standard_pack_goals(NEUTRAL_EXAMPLE.as_bytes(), GAME)
        .unwrap_or_else(|issues| panic!("the neutral example reads: {issues:?}"));
    let points = version_2.entry("points").expect("the resource entry");
    assert_eq!(
        serde_json::to_string(&declared_entry(points)).expect("the entry as JSON"),
        concat!(
            r#"{"name":"points","type":"resource","block":"goal_row","#,
            r#""title":{"en":"Points to hold","zh-Hans":"要攒的点数"},"#,
            r#""governs":{"pool":"pool.neutral.points"},"#,
            r#""rate":{"floor_milli_per_day":120000,"ceiling_milli_per_day":900000}}"#
        )
    );
    let relabelled = NEUTRAL_EXAMPLE.replace("actingcommand.goals.v2", "actingcommand.goals.v3");
    assert_eq!(
        parse_standard_pack_goals(relabelled.as_bytes(), GAME),
        Ok(version_2.clone())
    );

    let carried = declaration();
    let lacking = carried.entry("lacking").expect("the entry lacking");
    assert_eq!(
        serde_json::to_string(&declared_entry(lacking)).expect("the entry as JSON"),
        concat!(
            r#"{"name":"lacking","type":"resource","block":"goal_row","#,
            r#""title":{"en":"Material"},"governs":{"pool":"pool.neutral.lacking"},"#,
            r#""scale":10,"importance_milli":1000}"#
        )
    );
}

// Fix round 1, test (a) (the coordinator's ruling 1): an entry that carries both numbers beside a
// pool whose valuation gives both (`priced`: a file the Lab's check refuses
// `goal_valuation_conflict` and that can still be placed) gets the goal without numbers of its
// own. `SetTarget` names neither, its target row is the row main writes for a valued pool's entry,
// and the edit the policy crate composes (its rows and its identity) is the one the same entry
// without the numbers gives.
#[test]
fn b3_fix1_an_entry_with_both_numbers_on_a_valued_pool_writes_the_row_of_an_entry_without_them() {
    let page = page_with(reading());
    let with_numbers = set_on(&page, "priced", "{\"at_least\":40}").expect("the value is taken");
    assert_eq!(
        with_numbers,
        GoalRecordEdit::SetTarget {
            id: "priced".to_owned(),
            resource: PRICED.to_owned(),
            at_least: 40,
            scale: None,
            importance_milli: None,
        }
    );
    let mut bare_page = page_with(reading());
    let bare_entry = bare_page
        .declaration
        .as_mut()
        .and_then(|goals| {
            goals
                .entries
                .iter_mut()
                .find(|declared| declared.name == "priced")
        })
        .expect("the entry priced");
    bare_entry.kind = DeclaredGoalKind::Resource {
        pool: PRICED.to_owned(),
        rate: None,
        scale: None,
        importance_milli: None,
    };
    let without_numbers =
        set_on(&bare_page, "priced", "{\"at_least\":40}").expect("the value is taken");
    assert_eq!(with_numbers, without_numbers);
    let composed = |edit: &GoalRecordEdit| {
        edit_goal_record(None, None, INSTANCE, edit, lab_setter())
            .unwrap_or_else(|refusal| panic!("{edit:?} is taken: {refusal:?}"))
    };
    assert_eq!(
        composed(&with_numbers),
        composed(&without_numbers),
        "the same rows and identity"
    );
    let rows = written(None, &with_numbers);
    assert_eq!(
        target_row(&rows, "priced"),
        &BTreeMap::from([
            ("row".to_owned(), text("target")),
            ("id".to_owned(), text("priced")),
            ("resource".to_owned(), text(PRICED)),
            ("at_least".to_owned(), FactScalar::Integer(40)),
            ("mode".to_owned(), text("adjust")),
            ("weight".to_owned(), text("score_stage")),
            ("set_unix_ms".to_owned(), FactScalar::Integer(5_000)),
            ("channel".to_owned(), text("lab")),
            ("initiator".to_owned(), text("Lab-CLI")),
        ])
    );
}

// Fix round 1, test (b): an entry that carries one of the two numbers beside a pool without a
// valuation (`weighted` its importance alone, `stepped` its step alone) is refused `pool_unvalued`
// with its pool, on no record and on a record that holds a goal: the check gives no edit, and the
// host composes and publishes only an edit the check gives, so nothing is written.
#[test]
fn b3_fix1_an_entry_with_one_number_on_a_pool_without_a_valuation_is_refused_pool_unvalued() {
    let page = page_with(reading());
    let lacking = set_on(&page, "lacking", "{\"at_least\":40}").expect("the value is taken");
    let rows = written(None, &lacking);
    let recorded = stored_goal_record(Some(rows.as_slice()), None, INSTANCE);
    for (name, pool_id) in [("weighted", WEIGHTED), ("stepped", STEPPED)] {
        for record in [&StoredGoalRecord::Empty, &recorded] {
            let refusal = check_edit(
                &page,
                record,
                EditRequest::Set {
                    name,
                    value_json: "{\"at_least\":40}",
                },
                judged_by_lab(),
            )
            .expect_err("the value is refused");
            assert_eq!(
                (refusal.problem, refusal.pool_id.as_deref()),
                (GoalProblem::PoolUnvalued, Some(pool_id)),
                "{name}"
            );
        }
    }
}
