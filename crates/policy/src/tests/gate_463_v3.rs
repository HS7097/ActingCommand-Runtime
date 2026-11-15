// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 V3 gate tests, policy half (0.12.61; register §1.383; coordinator work order
// rd-1115/brief-463-v3-goals-view-edit.md; contracts/resource-targets.md, "One value by name"):
// the record one `EditGoal` writes over an instance's stored `session.resource_targets` record
// (`edit_goal_record`) and the goal page's reading of it (`stored_goal_record`). One row is
// replaced, added or removed; every other row is kept as it is stored, the rows another request
// wrote with who set them; the whole record is the v3 reader's; a record left empty is the
// withdrawal; a v2 policy's targets become v3 targets with its lifetime; a v1 policy is
// unreadable; the bounds refuse. The instance, the names and the pools are made up; no clock.

use std::collections::BTreeMap;

use actingcommand_contract::{FactScalar, Initiator, RuntimeChannel};
use serde_json::json;

use crate::{
    GoalRecordEdit, GoalRecordRefusal, StoredGoalRecord, TargetCondition, TargetSetter,
    edit_goal_record, stored_goal_record,
};

const INSTANCE: &str = "instance-a";
const POOL: &str = "pool.neutral.points";
const OTHER_POOL: &str = "pool.neutral.other";

type Rows = Vec<BTreeMap<String, FactScalar>>;

/// Who sets a row: the Lab command line at `at`.
fn lab(at: u64) -> TargetSetter {
    TargetSetter {
        set_unix_ms: at,
        channel: RuntimeChannel::Lab,
        initiator: Some(Initiator::LabCli),
    }
}

/// Who sets a row: the console at `at`.
fn console(at: u64) -> TargetSetter {
    TargetSetter {
        set_unix_ms: at,
        channel: RuntimeChannel::Ui,
        initiator: None,
    }
}

/// The rows `edit` writes over `stored`, set by `setter`; the edit must be taken.
fn edited(stored: Option<&Rows>, edit: GoalRecordEdit, setter: TargetSetter) -> Rows {
    edit_goal_record(stored.map(Vec::as_slice), None, INSTANCE, &edit, setter)
        .unwrap_or_else(|refusal| panic!("{edit:?} is taken: {refusal:?}"))
        .composed
        .rows
}

fn set_entry(name: &str, value: serde_json::Value) -> GoalRecordEdit {
    GoalRecordEdit::SetEntry {
        name: name.to_owned(),
        value,
    }
}

/// The row of kind `kind` and name (or id) `name` among `rows`.
fn row<'a>(rows: &'a Rows, kind: &str, name: &str) -> &'a BTreeMap<String, FactScalar> {
    let member = if kind == "entry" { "name" } else { "id" };
    rows.iter()
        .find(|row| {
            row.get("row") == Some(&FactScalar::String(kind.to_owned()))
                && row.get(member) == Some(&FactScalar::String(name.to_owned()))
        })
        .unwrap_or_else(|| panic!("no {kind} row {name}: {rows:?}"))
}

fn text(value: &str) -> FactScalar {
    FactScalar::String(value.to_owned())
}

// An entry set on no record is a v3 record of that one entry row, `{row, name, value_json,
// set_unix_ms, channel, initiator}`, its value's canonical text, which the goal page reads back with
// who set it; a second set of the same name replaces that row where it stands; clearing the last
// entry leaves the withdrawal, which the page reads as no record.
#[test]
fn v3_policy_an_entry_set_on_no_record_is_read_back_and_clearing_it_withdraws() {
    let rows = edited(None, set_entry("pick", json!("item_a")), lab(1_000));
    assert_eq!(rows.len(), 2, "the header and the entry: {rows:?}");
    assert_eq!(
        row(&rows, "entry", "pick"),
        &BTreeMap::from([
            ("row".to_owned(), text("entry")),
            ("name".to_owned(), text("pick")),
            ("value_json".to_owned(), text("\"item_a\"")),
            ("set_unix_ms".to_owned(), FactScalar::Integer(1_000)),
            ("channel".to_owned(), text("lab")),
            ("initiator".to_owned(), text("Lab-CLI")),
        ])
    );
    let StoredGoalRecord::V3 { targets, entries } =
        stored_goal_record(Some(rows.as_slice()), None, INSTANCE)
    else {
        panic!("a v3 record: {rows:?}");
    };
    assert!(targets.is_empty());
    assert_eq!(entries.len(), 1);
    assert_eq!(
        (
            entries[0].name.as_str(),
            &entries[0].value,
            entries[0].value_json.as_str(),
            entries[0].set
        ),
        ("pick", &json!("item_a"), "\"item_a\"", lab(1_000))
    );

    let again = edited(
        Some(&rows),
        set_entry("pick", json!("item_b")),
        console(2_000),
    );
    assert_eq!(again.len(), 2);
    assert_eq!(
        row(&again, "entry", "pick")["value_json"],
        text("\"item_b\"")
    );
    assert_eq!(row(&again, "entry", "pick")["channel"], text("ui"));
    assert!(!row(&again, "entry", "pick").contains_key("initiator"));

    let cleared = edited(
        Some(&again),
        GoalRecordEdit::ClearEntry {
            name: "pick".to_owned(),
        },
        lab(3_000),
    );
    assert_eq!(
        cleared[0]["schema_version"],
        text("actingcommand.resource-targets.v2"),
        "the withdrawal: {cleared:?}"
    );
    assert_eq!(cleared.len(), 1, "a withdrawal holds its header only");
    assert_eq!(
        stored_goal_record(Some(cleared.as_slice()), None, INSTANCE),
        StoredGoalRecord::Empty
    );
}

// A second entry and a target join the record after the rows there; the rows another request
// wrote stay as they are stored, who set them included; a target on a pool is mode `adjust` with
// the setter's fields; a target on the same pool under another id replaces it and names it, and
// one of the same id on another pool goes too; clearing a target removes its row only.
#[test]
fn v3_policy_one_row_changes_and_every_other_row_is_kept_as_stored() {
    let first = edited(None, set_entry("pick", json!("item_a")), lab(1_000));
    let second = edited(Some(&first), set_entry("floor", json!(40)), console(2_000));
    assert_eq!(
        row(&second, "entry", "pick"),
        row(&first, "entry", "pick"),
        "kept as stored"
    );
    assert_eq!(row(&second, "entry", "floor")["value_json"], text("40"));

    let targeted = edited(
        Some(&second),
        GoalRecordEdit::SetTarget {
            id: "points".to_owned(),
            resource: POOL.to_owned(),
            at_least: 500,
        },
        lab(3_000),
    );
    assert_eq!(
        row(&targeted, "target", "points"),
        &BTreeMap::from([
            ("row".to_owned(), text("target")),
            ("id".to_owned(), text("points")),
            ("resource".to_owned(), text(POOL)),
            ("at_least".to_owned(), FactScalar::Integer(500)),
            ("mode".to_owned(), text("adjust")),
            ("weight".to_owned(), text("score_stage")),
            ("set_unix_ms".to_owned(), FactScalar::Integer(3_000)),
            ("channel".to_owned(), text("lab")),
            ("initiator".to_owned(), text("Lab-CLI")),
        ])
    );
    assert_eq!(
        row(&targeted, "entry", "floor"),
        row(&second, "entry", "floor")
    );

    let replaced = edit_goal_record(
        Some(targeted.as_slice()),
        None,
        INSTANCE,
        &GoalRecordEdit::SetTarget {
            id: "stock".to_owned(),
            resource: POOL.to_owned(),
            at_least: 700,
        },
        console(4_000),
    )
    .expect("the target on the pool is replaced");
    assert_eq!(replaced.replaced_target_id.as_deref(), Some("points"));
    let StoredGoalRecord::V3 { targets, entries } =
        stored_goal_record(Some(replaced.composed.rows.as_slice()), None, INSTANCE)
    else {
        panic!("a v3 record");
    };
    assert_eq!(targets.len(), 1, "one target on the pool");
    assert_eq!(targets[0].target.id, "stock");
    assert_eq!(
        targets[0].target.condition,
        TargetCondition::AtLeast { amount: 700 }
    );
    assert_eq!(targets[0].set, Some(console(4_000)));
    assert_eq!(entries.len(), 2, "the entries are kept");

    let moved = edited(
        Some(&replaced.composed.rows),
        GoalRecordEdit::SetTarget {
            id: "stock".to_owned(),
            resource: OTHER_POOL.to_owned(),
            at_least: 9,
        },
        lab(5_000),
    );
    let StoredGoalRecord::V3 { targets, .. } =
        stored_goal_record(Some(moved.as_slice()), None, INSTANCE)
    else {
        panic!("a v3 record");
    };
    assert_eq!(
        targets
            .iter()
            .map(|stored| (stored.target.id.as_str(), stored.target.resource.as_str()))
            .collect::<Vec<_>>(),
        [("stock", OTHER_POOL)],
        "an id is unique: the one on the other pool went"
    );

    let cleared = edited(
        Some(&moved),
        GoalRecordEdit::ClearTarget {
            id: "stock".to_owned(),
        },
        lab(6_000),
    );
    let read = stored_goal_record(Some(cleared.as_slice()), None, INSTANCE);
    assert!(read.targets().is_empty());
    assert_eq!(read.entries().len(), 2);
}

// A v2 policy's targets become v3 targets with the policy's lifetime and no setter when an entry
// is set over it; a v1 policy and rows no reader reads are refused, nothing composed.
#[test]
fn v3_policy_a_v2_policy_becomes_v3_targets_and_a_v1_policy_is_unreadable() {
    let header = |version: &str| {
        BTreeMap::from([
            ("row".to_owned(), text("policy")),
            ("schema_version".to_owned(), text(version)),
            ("instance".to_owned(), text(INSTANCE)),
            (
                "policy_sha256".to_owned(),
                text(&format!("sha256:{}", "a".repeat(64))),
            ),
        ])
    };
    let target = BTreeMap::from([
        ("row".to_owned(), text("target")),
        ("id".to_owned(), text("points")),
        ("resource".to_owned(), text(POOL)),
        ("at_least".to_owned(), FactScalar::Integer(300)),
        ("mode".to_owned(), text("adjust")),
        ("weight".to_owned(), text("score_stage")),
    ]);
    let v2 = vec![header("actingcommand.resource-targets.v2"), target.clone()];
    let rows = edit_goal_record(
        Some(v2.as_slice()),
        Some(90_000),
        INSTANCE,
        &set_entry("pick", json!("item_a")),
        lab(1_000),
    )
    .expect("an entry over a v2 policy")
    .composed
    .rows;
    let StoredGoalRecord::V3 { targets, entries } =
        stored_goal_record(Some(rows.as_slice()), None, INSTANCE)
    else {
        panic!("a v3 record: {rows:?}");
    };
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].target.valid_until_unix_ms, Some(90_000));
    assert_eq!(targets[0].set, None, "the v2 target names no setter");
    assert_eq!(entries.len(), 1);

    let v1 = vec![header("actingcommand.resource-targets.v1"), target];
    assert_eq!(
        edit_goal_record(
            Some(v1.as_slice()),
            Some(90_000),
            INSTANCE,
            &set_entry("pick", json!("item_a")),
            lab(1_000),
        ),
        Err(GoalRecordRefusal::Unreadable)
    );
    assert_eq!(
        stored_goal_record(Some(v1.as_slice()), Some(90_000), INSTANCE),
        StoredGoalRecord::Unreadable
    );
}

// A value whose canonical text is longer than 1024 bytes, a fraction, a 97th entry and a 17th
// target are refused with their word, nothing composed.
#[test]
fn v3_policy_the_bounds_refuse_an_edit() {
    let long = json!("x".repeat(1_100));
    assert_eq!(
        edit_goal_record(None, None, INSTANCE, &set_entry("pick", long), lab(1)),
        Err(GoalRecordRefusal::ValueTooLarge)
    );
    assert_eq!(
        edit_goal_record(None, None, INSTANCE, &set_entry("pick", json!(0.5)), lab(1)),
        Err(GoalRecordRefusal::ValueInvalid)
    );
    let mut rows = None::<Rows>;
    for index in 0..96 {
        rows = Some(edited(
            rows.as_ref(),
            set_entry(&format!("entry_{index}"), json!(index)),
            lab(1),
        ));
    }
    assert_eq!(
        edit_goal_record(
            rows.as_deref(),
            None,
            INSTANCE,
            &set_entry("entry_96", json!(96)),
            lab(1)
        ),
        Err(GoalRecordRefusal::EntriesFull)
    );
    let mut rows = None::<Rows>;
    for index in 0..16 {
        rows = Some(edited(
            rows.as_ref(),
            GoalRecordEdit::SetTarget {
                id: format!("target_{index}"),
                resource: format!("pool.neutral.{index}"),
                at_least: 1,
            },
            lab(1),
        ));
    }
    assert_eq!(
        edit_goal_record(
            rows.as_deref(),
            None,
            INSTANCE,
            &GoalRecordEdit::SetTarget {
                id: "target_16".to_owned(),
                resource: "pool.neutral.16".to_owned(),
                at_least: 1,
            },
            lab(1)
        ),
        Err(GoalRecordRefusal::TargetsFull)
    );
}
