// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #455 RR3 (register §1.538; `contracts/standard-pack-goal-tables.md`, "What is
//! computed"; `contracts/resource-targets.md`, "Needs computed from the records"): what the
//! characters an instance's stored records name still need and do not have, as one pure function
//! of what the host read before an evaluation ([`instance_needs`]): the instance's stored records
//! (read by the goal records' own reader), the root's tables as the scheduler workbench keeps them
//! for the generation in force ([`RecordsRoot`]), the instance's progress lists as the fact store
//! holds them, and the holdings through the evaluator's own projection ([`NeedsSources`]).
//!
//! - **The walk** of one record: for each column its `to` names (each sub of a column of
//!   `per: "sub"`), the current position is the tuple of the column's reading columns in the key's
//!   row, equal member for member to one of the ladder's positions; the need is the sum of the
//!   steps from it to the target in the ladder's order (a key's override of a class's step taken
//!   instead, a role resolved to the key's own item); a target at or behind the current position
//!   asks nothing. A prerequisite of a column, asked by a step the walk takes (every step without
//!   `from`; one that arrives at `from` or beyond with it), takes the required column to its
//!   `at_least` as well; a condition is noted and asks nothing.
//! - **Not known adds nothing**, each part with its own note: a list that is missing, expired, of
//!   confidence 0 or not keyed by its key column (the table's `held_by` list: the whole record); a
//!   known list without the key's row; a row of the key without the column's reading column;
//!   readings that form no position. A `held_by` list read whole without the key: not held,
//!   known. A record whose entry, key or target the root in force does not declare is not
//!   applied, with the word a goal edit of it would be refused with now (the edit's own check);
//!   nor is one whose table and key an earlier stored record names already under another entry
//!   (`key_repeated`: two entries may name one table, and a key is walked once). The result is a
//!   lower bound of the need.
//! - **Taking apart**: the records' needs are summed by item, then the items are taken in the
//!   recipes' topological order from the top: an item with a recipe is covered by its holding up
//!   to its need and the rest is made (rounded up by `makes`), adding its `from` and `cost`; one
//!   whose holding is not known adds nothing below itself (#455 RR3c: nor one whose holding the
//!   production told under a goal's gap on its pool since its reading left unread). What remains
//!   on an item without a recipe is its pool's need; an item without a recipe and without a pool
//!   is noted and weighs nothing. #455 MAKE (register §1.558): the pool of an item with a recipe
//!   whose holding is known and below its need asks, beside what its holding covers, the made
//!   part: what its items in hand make now, the least over its `from` and `cost` items of
//!   ⌊holding ÷ what one making takes⌋ makings (an item named in both, the sum), at most those the
//!   need asks, times `makes`, at most the part not covered; an item whose holding is not known or
//!   unread, or without a pool, makes none, and with none the pool is as before. Without a maker
//!   of the pool on the instance ([`NeedsSources::has_maker`]) nothing is made and the items'
//!   holdings are not read for it. Its items keep their needs either way.
//!
//! Nothing here reads a clock, a file, a lock or the ledger, and nothing computed is stored.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use actingcommand_contract::{
    FactRecord, FactScalar, GoalColumnRead, GoalKey, GoalKeyColumn, GoalLadder, GoalProblem,
    GoalProgressList, GoalRecipe, GoalRequirement, GoalStep, GoalTable, GoalTableItem, GoalUse,
    MAX_GOAL_SAFE_INTEGER, StandardPackGoalTables,
};
use actingcommand_policy::{
    InstanceRecordNeeds, NeedHolding, PoolNeed, PoolNeedKind, ProgressListUnknown, RecordNeedsNote,
    RecordRef,
};
use serde_json::{Map, Value};

use crate::goal_records::{StoredGoalRecords, StoredRecordRow};
use crate::goals::{check_to, position_of};
use crate::pool_list_rows::list_rows_of;

/// The lowest confidence a progress list is read with: above 0, as a fact a predicate reads
/// without its own minimum.
const MIN_PROGRESS_CONFIDENCE_MILLI: u16 = 1;
/// The largest need of a pool: the safe integers, the bound of a target's `at_least`.
const MAX_NEED: u64 = MAX_GOAL_SAFE_INTEGER.unsigned_abs();

/// What the scheduler workbench keeps of one game root for the needs: the `records` entries of
/// its accepted `goals.json` (entry name to table name), and its accepted `goal-tables.json` with
/// the sha256 of its bytes, for the generation in force.
#[derive(Debug, Clone, Default)]
pub(crate) struct RecordsRoot {
    pub(crate) entries: BTreeMap<String, String>,
    pub(crate) tables: Option<(Arc<StandardPackGoalTables>, String)>,
}

/// What the workbench keeps for the needs, by game.
pub(crate) type RecordRoots = BTreeMap<String, RecordsRoot>;

/// What the computation reads of one instance besides its records and its root: its progress
/// lists as the fact store holds them, and, through the evaluator's own projection of the
/// evaluation's inputs, whether a list is fresh and what a pool holds.
pub(crate) trait NeedsSources {
    /// The instance's list fact under `fact_key`.
    fn list(&self, fact_key: &str) -> Option<&FactRecord>;
    /// Whether a list observed at `observed_at_unix_ms` with the expiry `expires_at_unix_ms` is
    /// fresh at the evaluation's time; fresh, its expiry.
    fn list_freshness(
        &self,
        fact_key: &str,
        observed_at_unix_ms: u64,
        expires_at_unix_ms: Option<u64>,
    ) -> Result<Option<u64>, ProgressListUnknown>;
    /// The holding of the pool the root's files write `pool_id`.
    fn holding(&self, pool_id: &str) -> NeedHolding;
    /// #455 MAKE fix round 1 (register §1.558): whether the instance has a maker of the pool the
    /// root's files write `pool_id`, the pool of an item whose recipe's `from` items are held in the
    /// pools `from` (`NeedsProjection::has_maker`, the evaluation's own answer). No default: every
    /// source says it.
    fn has_maker(&self, pool_id: &str, from: &[String]) -> bool;
}

/// The needs of the instance whose stored `session.goal_records` record its reader read as
/// `stored`, observed at `records_observed_at_unix_ms`, against `root`, its game's (`None`: the
/// workbench keeps nothing of it, so no entry applies), reading `sources` (module doc). `None`
/// for an instance without stored records (no record, or a header alone): nothing is computed
/// and its evaluation is as without this slice.
pub(crate) fn instance_needs(
    stored: &StoredGoalRecords,
    records_observed_at_unix_ms: u64,
    root: Option<&RecordsRoot>,
    sources: &dyn NeedsSources,
) -> Option<InstanceRecordNeeds> {
    let mut notes = Vec::new();
    let records: &[StoredRecordRow] = match stored {
        StoredGoalRecords::Absent => return None,
        StoredGoalRecords::Read(records) if records.is_empty() => return None,
        StoredGoalRecords::Read(records) => records,
        StoredGoalRecords::Unreadable => {
            notes.push(RecordNeedsNote::NotApplied {
                record: None,
                problem: GoalProblem::RecordUnreadable,
            });
            &[]
        }
    };
    let tables = root.and_then(|root| root.tables.as_ref());
    let mut lists = Lists {
        sources,
        read: BTreeMap::new(),
        fresh_until_unix_ms: None,
    };
    let mut raw = BTreeMap::<String, u128>::new();
    // Fix round 1 (the review): two records entries may name one table, and a key recorded under
    // both would be walked twice and its need added twice. The first record of a table's key, in
    // the stored order, is walked; a later one is not applied (`key_repeated`), so the result
    // stays a lower bound of the need.
    let mut walked = BTreeSet::<(String, String)>::new();
    for stored_record in records {
        let record = RecordRef {
            entry: stored_record.entry.clone(),
            key: stored_record.key.clone(),
        };
        match applied(root, stored_record) {
            Err(problem) => notes.push(RecordNeedsNote::NotApplied {
                record: Some(record),
                problem,
            }),
            Ok((table, _, _))
                if !walked.insert((table.name.clone(), stored_record.key.clone())) =>
            {
                notes.push(RecordNeedsNote::NotApplied {
                    record: Some(record),
                    problem: GoalProblem::KeyRepeated,
                });
            }
            Ok((table, key, to)) => {
                let mut walk = Walk {
                    table,
                    key,
                    record: &record,
                    lists: &mut lists,
                    notes: &mut notes,
                };
                walk.need(&to, &mut raw);
            }
        }
    }
    let mut fresh_until_unix_ms = lists.fresh_until_unix_ms;
    let taken = match tables {
        Some((tables, _)) => take_apart(tables, raw, sources, &mut notes, &mut fresh_until_unix_ms),
        None => TakenApart::default(),
    };
    Some(
        InstanceRecordNeeds::new(
            tables.map(|(_, sha256)| sha256.clone()),
            records_observed_at_unix_ms,
            taken.pools,
            notes,
            fresh_until_unix_ms,
        )
        .with_making_from(taken.making_from),
    )
}

/// The table, the key and the target of `record` when the root in force declares them, or the
/// word a goal edit of it would be refused with now: an entry that applies no table of the root's
/// accepted tables `name_unknown`, a key the table does not hold `not_an_option`, a target the
/// edit's own check refuses (`check_to`) its word.
fn applied<'t>(
    root: Option<&'t RecordsRoot>,
    record: &StoredRecordRow,
) -> Result<(&'t GoalTable, &'t GoalKey, Map<String, Value>), GoalProblem> {
    let table = root
        .and_then(|root| {
            let table = root.entries.get(&record.entry)?;
            root.tables
                .as_ref()
                .and_then(|(tables, _)| tables.table(table))
        })
        .ok_or(GoalProblem::NameUnknown)?;
    let key = table.key(&record.key).ok_or(GoalProblem::NotAnOption)?;
    let to = serde_json::from_str::<Value>(&record.to).map_err(|_| GoalProblem::WrongType)?;
    check_to(table, key, Some(&to), "").map_err(|refusal| refusal.problem)?;
    match to {
        Value::Object(columns) => Ok((table, key, columns)),
        _ => Err(GoalProblem::WrongType),
    }
}

/// The rows of a list.
type ListRows<'s> = &'s [BTreeMap<String, FactScalar>];

/// The progress lists one instance's records read, each judged once, and the earliest expiry of
/// those read.
struct Lists<'s> {
    sources: &'s dyn NeedsSources,
    read: BTreeMap<String, Result<ListRows<'s>, ProgressListUnknown>>,
    fresh_until_unix_ms: Option<u64>,
}

impl<'s> Lists<'s> {
    /// The rows of `list`, or why it gives no reading.
    fn rows(&mut self, list: &GoalProgressList) -> Result<ListRows<'s>, ProgressListUnknown> {
        if let Some(read) = self.read.get(&list.fact_key) {
            return *read;
        }
        let read = self.judge(list);
        self.read.insert(list.fact_key.clone(), read);
        read
    }

    /// `list` judged by the rules a list a pool is read from obeys: present, of a confidence above
    /// 0, a record list keyed by its key column (`list_rows_of`, the one implementation), not
    /// invalidated by a reset nor past its expiry.
    fn judge(&mut self, list: &GoalProgressList) -> Result<ListRows<'s>, ProgressListUnknown> {
        let sources = self.sources;
        let record = sources.list(&list.fact_key);
        let rows = list_rows_of(record, MIN_PROGRESS_CONFIDENCE_MILLI, &list.key_column)?;
        let Some(record) = record else {
            return Err(ProgressListUnknown::FactMissing);
        };
        let expiry = sources.list_freshness(
            &list.fact_key,
            record.observed_at_unix_ms,
            record.expires_at_unix_ms,
        )?;
        self.fresh_until_unix_ms = earliest(self.fresh_until_unix_ms, expiry);
        Ok(rows)
    }
}

/// The row of `rows` whose `key_column` holds the text `key`.
fn row_of<'r>(
    rows: &'r [BTreeMap<String, FactScalar>],
    key_column: &str,
    key: &str,
) -> Option<&'r BTreeMap<String, FactScalar>> {
    rows.iter()
        .find(|row| matches!(row.get(key_column), Some(FactScalar::String(text)) if text == key))
}

/// One ladder a record walks: its column (and sub), the ladder, the reading columns its current
/// position is read from, and the key's overrides of a class's steps.
struct Lane<'t> {
    label: String,
    ladder: &'t GoalLadder,
    reads: &'t [String],
    overrides: &'t [(usize, GoalStep)],
}

impl Lane<'_> {
    /// The step that goes from position `index` to the next: the key's override, else the
    /// ladder's.
    fn step(&self, index: usize) -> Option<&GoalStep> {
        self.overrides
            .iter()
            .find(|(at, _)| *at == index)
            .map(|(_, step)| step)
            .or_else(|| self.ladder.steps.get(index))
    }
}

/// The lane `key` walks in the column `column` of `table` (of `per: "sub"`, its sub `sub`).
fn lane_of<'t>(
    table: &'t GoalTable,
    key: &'t GoalKey,
    column: &str,
    sub: Option<&str>,
) -> Option<Lane<'t>> {
    let declared = table.column(column)?;
    let written = key
        .columns
        .iter()
        .find(|(name, _)| name == column)
        .map(|(_, written)| written);
    match (&declared.read, sub) {
        (GoalColumnRead::Class(reads), None) => Some(Lane {
            label: column.to_owned(),
            ladder: table.class(&key.class)?.ladder(column)?,
            reads: reads.as_slice(),
            overrides: match written {
                Some(GoalKeyColumn::Steps(steps)) => steps.as_slice(),
                _ => &[],
            },
        }),
        (GoalColumnRead::Key(reads), None) => match written {
            Some(GoalKeyColumn::Ladder(ladder)) => Some(Lane {
                label: column.to_owned(),
                ladder,
                reads: reads.as_slice(),
                overrides: &[],
            }),
            _ => None,
        },
        (GoalColumnRead::Sub(subs), Some(sub)) => {
            let reads = subs
                .iter()
                .find(|(name, _)| name == sub)
                .map(|(_, reads)| reads.as_slice())?;
            let Some(GoalKeyColumn::Subs(ladders)) = written else {
                return None;
            };
            let ladder = ladders
                .iter()
                .find(|(name, _)| name == sub)
                .map(|(_, ladder)| ladder)?;
            Some(Lane {
                label: format!("{column}/{sub}"),
                ladder,
                reads,
                overrides: &[],
            })
        }
        _ => None,
    }
}

/// A lane by its column and its sub.
type LaneKey = (String, Option<String>);

/// The walk of one applied record.
struct Walk<'w, 't, 's> {
    table: &'t GoalTable,
    key: &'t GoalKey,
    record: &'w RecordRef,
    lists: &'w mut Lists<'s>,
    notes: &'w mut Vec<RecordNeedsNote>,
}

impl Walk<'_, '_, '_> {
    /// Adds to `raw`, by item, what the record's target `to` needs from the key's current
    /// positions (module doc, "The walk"); a part not known adds nothing and is noted.
    fn need(&mut self, to: &Map<String, Value>, raw: &mut BTreeMap<String, u128>) {
        if !self.held() {
            return;
        }
        let mut targets = BTreeMap::<LaneKey, usize>::new();
        for (column, target) in to {
            match target {
                Value::Object(subs) => {
                    for (sub, position) in subs {
                        self.note_target(&mut targets, column, Some(sub.as_str()), position);
                    }
                }
                position => self.note_target(&mut targets, column, None, position),
            }
        }
        let mut currents = BTreeMap::<LaneKey, Option<usize>>::new();
        let mut conditions = BTreeSet::<(String, String)>::new();
        loop {
            let mut raised = false;
            let walked = targets
                .iter()
                .map(|(lane_key, target)| (lane_key.clone(), *target))
                .collect::<Vec<_>>();
            for (lane_key, target) in walked {
                let Some(lane) = lane_of(self.table, self.key, &lane_key.0, lane_key.1.as_deref())
                else {
                    continue;
                };
                let current = match currents.get(&lane_key) {
                    Some(current) => *current,
                    None => {
                        let read = self.read_current(&lane);
                        currents.insert(lane_key.clone(), read);
                        read
                    }
                };
                // A position not known, or a target at or behind it: no step is taken, and
                // nothing is asked.
                if current.is_none_or(|current| current >= target) {
                    continue;
                }
                let Some(declared) = self.table.column(&lane_key.0) else {
                    continue;
                };
                for requirement in &declared.requires {
                    match requirement {
                        GoalRequirement::Condition(condition) => {
                            if conditions.insert((lane_key.0.clone(), condition.clone())) {
                                self.notes.push(RecordNeedsNote::Condition {
                                    record: self.record.clone(),
                                    column: lane_key.0.clone(),
                                    condition: condition.clone(),
                                });
                            }
                        }
                        GoalRequirement::Column {
                            column,
                            at_least,
                            from,
                        } => {
                            // Without `from` every step asks it; with it, a step that arrives at
                            // `from` or beyond, which the walk takes when its target is there.
                            let asked = match from {
                                None => true,
                                Some(from) => lane
                                    .ladder
                                    .positions
                                    .iter()
                                    .position(|position| position == from)
                                    .is_some_and(|from_index| target >= from_index),
                            };
                            if !asked {
                                continue;
                            }
                            let Some(at) =
                                self.table.ladders_of(self.key, column).first().and_then(
                                    |ladder| {
                                        ladder
                                            .positions
                                            .iter()
                                            .position(|position| position == at_least)
                                    },
                                )
                            else {
                                continue;
                            };
                            let required = (column.clone(), None);
                            if targets.get(&required).is_none_or(|existing| *existing < at) {
                                targets.insert(required, at);
                                raised = true;
                            }
                        }
                    }
                }
            }
            if !raised {
                break;
            }
        }
        for (lane_key, target) in &targets {
            let Some(Some(current)) = currents.get(lane_key).copied() else {
                continue;
            };
            let Some(lane) = lane_of(self.table, self.key, &lane_key.0, lane_key.1.as_deref())
            else {
                continue;
            };
            for index in current..*target {
                let Some(step) = lane.step(index) else {
                    continue;
                };
                for (used, amount) in step {
                    let item = match used {
                        GoalUse::Item(item) => Some(item.as_str()),
                        GoalUse::Role(role) => self
                            .key
                            .own
                            .iter()
                            .find(|(name, _)| name == role)
                            .map(|(_, item)| item.as_str()),
                    };
                    if let Some(item) = item {
                        let entry = raw.entry(item.to_owned()).or_default();
                        *entry = entry.saturating_add(u128::from(*amount));
                    }
                }
            }
        }
    }

    /// Whether the key is held: the table's `held_by` list, read whole, holds a row of it (a table
    /// without one holds every key). Not known, the whole record is noted pending; not held, it is
    /// noted not held; either adds nothing.
    fn held(&mut self) -> bool {
        let Some(held_by) = self.table.progress.iter().find(|list| list.held_by) else {
            return true;
        };
        match self.lists.rows(held_by) {
            Err(reason) => {
                self.notes.push(RecordNeedsNote::ProgressUnknown {
                    record: self.record.clone(),
                    column: None,
                    fact_key: held_by.fact_key.clone(),
                    reason,
                });
                false
            }
            Ok(rows) if row_of(rows, &held_by.key_column, &self.key.name).is_none() => {
                self.notes.push(RecordNeedsNote::NotHeld {
                    record: self.record.clone(),
                });
                false
            }
            Ok(_) => true,
        }
    }

    /// Notes the target `position` of the lane `column` (its sub `sub`) in `targets`, as its index
    /// on the lane's ladder; the edit's check held it to the ladder already.
    fn note_target(
        &self,
        targets: &mut BTreeMap<LaneKey, usize>,
        column: &str,
        sub: Option<&str>,
        position: &Value,
    ) {
        let Some(lane) = lane_of(self.table, self.key, column, sub) else {
            return;
        };
        let Some(position) = position_of(position) else {
            return;
        };
        if let Some(index) = lane
            .ladder
            .positions
            .iter()
            .position(|written| *written == position)
        {
            targets.insert((column.to_owned(), sub.map(str::to_owned)), index);
        }
    }

    /// The index of the key's current position on `lane`'s ladder, read from its reading columns
    /// in the key's rows of the table's lists; `None`, noted with its cause, when it is not known.
    fn read_current(&mut self, lane: &Lane<'_>) -> Option<usize> {
        let mut position = Vec::with_capacity(lane.reads.len());
        for reading in lane.reads {
            let Some(list) = self
                .table
                .progress
                .iter()
                .find(|list| list.columns.iter().any(|column| column == reading))
            else {
                self.position_unknown(lane);
                return None;
            };
            let rows = match self.lists.rows(list) {
                Ok(rows) => rows,
                Err(reason) => {
                    self.notes.push(RecordNeedsNote::ProgressUnknown {
                        record: self.record.clone(),
                        column: Some(lane.label.clone()),
                        fact_key: list.fact_key.clone(),
                        reason,
                    });
                    return None;
                }
            };
            // Fix round 1 (the review): two causes, two notes. A list without the key's row has
            // not covered the character, which its next reading may; a row without the reading
            // column is a package that does not write it, which no later reading mends.
            let Some(row) = row_of(rows, &list.key_column, &self.key.name) else {
                self.notes.push(RecordNeedsNote::RowMissing {
                    record: self.record.clone(),
                    column: lane.label.clone(),
                    fact_key: list.fact_key.clone(),
                });
                return None;
            };
            match row.get(reading) {
                Some(FactScalar::Integer(value)) => position.push(*value),
                // Workflow #455 RR3a (0.12.96): a truth value reads as a whole number, false 0 and
                // true 1. A list's `passed` column says whether a mark is on the card (a badge that
                // is there or is not), and a ladder's position may count such marks.
                Some(FactScalar::Boolean(marked)) => position.push(i64::from(*marked)),
                None => {
                    self.notes.push(RecordNeedsNote::ReadingMissing {
                        record: self.record.clone(),
                        column: lane.label.clone(),
                        fact_key: list.fact_key.clone(),
                        reading: reading.clone(),
                    });
                    return None;
                }
                Some(_) => {
                    self.position_unknown(lane);
                    return None;
                }
            }
        }
        let found = lane
            .ladder
            .positions
            .iter()
            .position(|written| *written == position);
        if found.is_none() {
            self.position_unknown(lane);
        }
        found
    }

    fn position_unknown(&mut self, lane: &Lane<'_>) {
        self.notes.push(RecordNeedsNote::PositionUnknown {
            record: self.record.clone(),
            column: lane.label.clone(),
        });
    }
}

/// The earlier of two optional expiries.
fn earliest(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (one, None) | (None, one) => one,
    }
}

/// `value` within the largest need.
fn bounded(value: u128) -> u64 {
    u64::try_from(value).map_or(MAX_NEED, |value| value.min(MAX_NEED))
}

/// The items of `tables` in the recipes' topological order from the top: an item comes after every
/// item whose recipe names it (in `from` or `cost`), the file's order among the rest. The recipes
/// are free of cycles (the tables' reader refuses one), so every item is listed once.
fn recipe_order<'t>(
    tables: &'t StandardPackGoalTables,
    items: &BTreeMap<&str, &'t GoalTableItem>,
) -> Vec<&'t GoalTableItem> {
    let mut users = BTreeMap::<&'t str, usize>::new();
    for item in &tables.items {
        users.entry(item.name.as_str()).or_insert(0);
        if let Some(recipe) = &item.recipe {
            for (input, _) in recipe.from.iter().chain(&recipe.cost) {
                *users.entry(input.as_str()).or_insert(0) += 1;
            }
        }
    }
    let mut ready = tables
        .items
        .iter()
        .filter(|item| users.get(item.name.as_str()) == Some(&0))
        .map(|item| item.name.as_str())
        .collect::<VecDeque<_>>();
    let mut order = Vec::with_capacity(tables.items.len());
    while let Some(name) = ready.pop_front() {
        let Some(item) = items.get(name).copied() else {
            continue;
        };
        order.push(item);
        if let Some(recipe) = &item.recipe {
            for (input, _) in recipe.from.iter().chain(&recipe.cost) {
                if let Some(count) = users.get_mut(input.as_str()) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        ready.push_back(input.as_str());
                    }
                }
            }
        }
    }
    order
}

/// #455 MAKE (register §1.558): what [`take_apart`] gives: each pool's need, by the pool as the
/// root's files write it, and, for each pool whose need holds a made part
/// ([`PoolNeedKind::Making`]), the pools of its recipe's `from` items, which say who makes it.
#[derive(Debug, Default)]
struct TakenApart {
    pools: BTreeMap<String, PoolNeed>,
    making_from: BTreeMap<String, Vec<String>>,
}

/// #455 MAKE (register §1.558; module doc, "Taking apart"): how many makings of `recipe` the
/// holdings of its items pay for now, at most `makings` (those the need asks), with the earliest
/// end of the holdings read: the least, over its `from` and `cost` items, of ⌊the item's holding ÷
/// what one making takes of it⌋, each holding read as an item's own ([`NeedsSources::holding`]) and
/// taken whole, not shared out among the items that use it. Fix round 1: an item named in both
/// `from` and `cost` takes the sum of its two amounts for one making. An item whose holding is not
/// known or unread, or that names no pool, pays for none: `(0, None)`.
fn makings_in_hand(
    recipe: &GoalRecipe,
    items: &BTreeMap<&str, &GoalTableItem>,
    sources: &dyn NeedsSources,
    makings: u128,
) -> (u128, Option<u64>) {
    if makings == 0 {
        return (0, None);
    }
    let mut per_making = BTreeMap::<&str, u128>::new();
    for (input, per) in recipe.from.iter().chain(&recipe.cost) {
        let taken = per_making.entry(input.as_str()).or_default();
        *taken = taken.saturating_add(u128::from(*per));
    }
    let mut paid = makings;
    let mut fresh_until_unix_ms = None;
    for (input, per) in per_making {
        let holding = items
            .get(input)
            .and_then(|item| item.pool.as_deref())
            .map(|pool| sources.holding(pool));
        let Some(NeedHolding::Known {
            held,
            fresh_until_unix_ms: holding_fresh_until,
        }) = holding
        else {
            return (0, None);
        };
        paid = paid.min(u128::from(held) / per.max(1));
        fresh_until_unix_ms = earliest(fresh_until_unix_ms, holding_fresh_until);
    }
    (paid, fresh_until_unix_ms)
}

/// The needs `need`, by item, taken apart once (module doc, "Taking apart"), by pool as the root's
/// files write it; the earliest end of the holdings read lowers `fresh_until_unix_ms`. #455 MAKE
/// (register §1.558): an item with a recipe whose holding `h` is known and below its need `n`
/// asks its pool `h + made` as well (`Making`), `made = min(n − h, k·makes)` with `k` the makings
/// its items in hand pay for now ([`makings_in_hand`]), and their holdings lower the freshness
/// then; with `made` 0 the pool is `Covered`, byte for byte as before. Fix round 1: `k` is 0, and
/// no holding of its items is read for it, when the instance has no maker of the pool
/// ([`NeedsSources::has_maker`]). Its recipe's items keep the needs the taking apart gives them
/// either way.
fn take_apart(
    tables: &StandardPackGoalTables,
    mut need: BTreeMap<String, u128>,
    sources: &dyn NeedsSources,
    notes: &mut Vec<RecordNeedsNote>,
    fresh_until_unix_ms: &mut Option<u64>,
) -> TakenApart {
    let items = tables
        .items
        .iter()
        .map(|item| (item.name.as_str(), item))
        .collect::<BTreeMap<_, _>>();
    let mut pools = BTreeMap::new();
    let mut making_from = BTreeMap::<String, Vec<String>>::new();
    for item in recipe_order(tables, &items) {
        let Some(amount) = need.get(&item.name).copied().filter(|amount| *amount > 0) else {
            continue;
        };
        match (&item.recipe, &item.pool) {
            (Some(recipe), Some(pool)) => match sources.holding(pool) {
                NeedHolding::Known {
                    held,
                    fresh_until_unix_ms: holding_fresh_until,
                } => {
                    *fresh_until_unix_ms = earliest(*fresh_until_unix_ms, holding_fresh_until);
                    let covered = amount.min(u128::from(held));
                    let makes = u128::from(recipe.makes.max(1));
                    let makings = (amount - covered).div_ceil(makes);
                    // Fix round 1 (the review's F1): the pools of the recipe's `from` items first;
                    // without a maker on the instance nothing is made and no holding of its items
                    // is read for it, so the pool is as before.
                    let from = recipe
                        .from
                        .iter()
                        .filter_map(|(input, _)| items.get(input.as_str()))
                        .filter_map(|from_item| from_item.pool.clone())
                        .collect::<Vec<_>>();
                    let (paid, inputs_fresh_until) =
                        if makings > 0 && sources.has_maker(pool, &from) {
                            makings_in_hand(recipe, &items, sources, makings)
                        } else {
                            (0, None)
                        };
                    let made = (amount - covered).min(paid.saturating_mul(makes));
                    let pool_need = if made == 0 {
                        PoolNeed {
                            need: bounded(covered),
                            kind: PoolNeedKind::Covered,
                        }
                    } else {
                        *fresh_until_unix_ms = earliest(*fresh_until_unix_ms, inputs_fresh_until);
                        making_from.insert(pool.clone(), from);
                        PoolNeed {
                            need: bounded(covered + made),
                            kind: PoolNeedKind::Making {
                                made: bounded(made),
                            },
                        }
                    };
                    pools.insert(pool.clone(), pool_need);
                    if makings == 0 {
                        continue;
                    }
                    for (input, per) in recipe.from.iter().chain(&recipe.cost) {
                        let entry = need.entry(input.clone()).or_default();
                        *entry = entry.saturating_add(makings.saturating_mul(u128::from(*per)));
                    }
                }
                NeedHolding::Unknown(reason) => {
                    pools.insert(
                        pool.clone(),
                        PoolNeed {
                            need: bounded(amount),
                            kind: PoolNeedKind::Unsplit,
                        },
                    );
                    notes.push(RecordNeedsNote::HoldingUnknown {
                        item: item.name.clone(),
                        pool_id: pool.clone(),
                        reason,
                    });
                }
                // #455 RR3c (register §1.543): a holding the production told under a goal's gap on
                // its pool since its reading left unread is not known either: the same lower bound,
                // its own note (`fact_stale`).
                NeedHolding::Unread => {
                    pools.insert(
                        pool.clone(),
                        PoolNeed {
                            need: bounded(amount),
                            kind: PoolNeedKind::Unsplit,
                        },
                    );
                    notes.push(RecordNeedsNote::HoldingUnread {
                        item: item.name.clone(),
                        pool_id: pool.clone(),
                    });
                }
            },
            (None, Some(pool)) => {
                pools.insert(
                    pool.clone(),
                    PoolNeed {
                        need: bounded(amount),
                        kind: PoolNeedKind::Lands,
                    },
                );
            }
            (_, None) => notes.push(RecordNeedsNote::ItemUnweighed {
                item: item.name.clone(),
                need: bounded(amount),
            }),
        }
    }
    TakenApart { pools, making_from }
}
