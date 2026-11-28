// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #455 RR4b-1 (register §1.557; `contracts/runtime-interface.md`, "Goal request kinds";
//! `contracts/standard-pack-goals.md`, "What a client sees"): what `GoalPageView` answers, as pure
//! functions of what the host read under the gates the evaluation takes ([`read_part`]).
//!
//! - **`records`**: for each `records` entry of the accepted `goals.json`, in file order, its
//!   `entry` row and then its stored records in their stored order; then, for each name the stored
//!   records hold that no records entry declares, in their stored order, its `entry` row and its
//!   records. A record's applied word, its key's held state and its lanes, in the table's order
//!   (target, the position a prerequisite raises it to, the current position or why it is not
//!   known, the conditions), are what the walk of the needs' computation read
//!   (`crate::goal_needs::walk_records`, the one function both call); a position is said through
//!   the pack's names (`say_goal_position`).
//! - **`options`** of a records entry the root applies: its table's columns, then its classes,
//!   then its keys, each in file order; a key's held state and current positions are read as the
//!   walk reads them (`crate::goal_needs::read_key`), only for the keys of the page.
//! - **`positions`** of one ladder: a class's (read once for every key of the class) or the one a
//!   key walks.
//! - **The page**: the rows after the cursor, at most `limit`, cut before the row that would make
//!   the serialized page pass [`MAX_GOAL_PAGE_VIEW_BYTES`], at least one; a record row that would
//!   pass [`MAX_GOAL_PAGE_VIEW_ROW_BYTES`] is answered without its texts. `next` is present exactly
//!   when rows remain.
//! - **The cursor** ([`PageState`]): one that came over other stored records (the part `records`
//!   alone, Addendum 1) or over another declaration (the accepted tables' sha256, the root's
//!   records entries) is refused with its own word, never mixed with the state now.
//!
//! No amount of what a record lacks is answered. Nothing here reads a clock, a file, a lock or the
//! ledger, and nothing is stored.

use std::collections::BTreeSet;

use actingcommand_contract::outcome::CodeStr;
use actingcommand_contract::{
    DeclaredGoal, DeclaredGoalKind, GoalClass, GoalColumn, GoalColumnPer, GoalColumnRead,
    GoalDeclarationState, GoalHeldState, GoalKey, GoalKeyHeld, GoalKeyLane, GoalKeyPosition,
    GoalLadder, GoalLadderRef, GoalLadderSource, GoalLanePosition, GoalLaneUnknown,
    GoalPageColumnRequirement, GoalPageCondition, GoalPageCursor, GoalPageCut, GoalPageDigest,
    GoalPageLadder, GoalPageLane, GoalPagePart, GoalPageRecordsState, GoalPageRequirement,
    GoalPageRow, GoalPageSub, GoalPageTables, GoalPageText, GoalPageView, GoalProblem,
    GoalRequirement, GoalTable, GoalTableNaming, GoalText, MAX_GOAL_PAGE_VIEW_BYTES,
    MAX_GOAL_PAGE_VIEW_ROW_BYTES, StandardPackGoalTables, StandardPackGoals, say_goal_position,
};
use actingcommand_policy::{PolicyCode, RecordNeedsNote};
use serde::Serialize;

use crate::goal_needs::{
    KeyHeld, LaneCurrent, LaneKey, NeedsSources, ProgressLists, RecordJudgement, RecordsRoot,
    WalkedLane, key_lanes, ladder_of, read_key, walk_records,
};
use crate::goal_records::{StoredGoalRecords, StoredRecordRow};
use crate::goals::declared_entry;

/// The pointers of a part's refusal.
const AT_ENTRY: &str = "/part/entry";
const AT_KEY: &str = "/part/ladder/key";
const AT_CLASS: &str = "/part/ladder/class";
const AT_COLUMN: &str = "/part/ladder/column";
const AT_SUB: &str = "/part/ladder/sub";

/// Why a page is not answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PageFailure {
    /// The cursor's rows answered pass the part's rows: `invalid_goal_page_view`.
    Invalid,
    /// The part names what the root does not have: `goal_page_view_refused` with the edit's
    /// word at its pointer.
    Refused {
        problem: GoalProblem,
        pointer: &'static str,
    },
    /// The cursor of `records` came over other stored records: `goal_page_view_records_moved`.
    RecordsMoved,
    /// The cursor came over another declaration: `goal_page_view_declaration_moved`.
    DeclarationMoved,
    /// Fix round 1: a request with a cursor reached a page that is not in memory, its
    /// declaration's state being `problem` (`not_checked_yet`, `root_failing` or `no_schedule`):
    /// `goal_page_view_declaration_absent`. The host answers it before any part is read.
    DeclarationAbsent { problem: GoalProblem },
    /// A row the reader took gives back no setter or no target (no row the reader takes does).
    RecordUnread,
    /// A row or the page does not serialize.
    Unencodable,
}

/// What a cursor is compared with now: the stored records' version (the sequence of the
/// `fact.published` that holds the instance's `session.goal_records` record, `0` without one), the
/// root's accepted tables' sha256, and the digest of its records entries ([`entries_digest`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PageState {
    pub(crate) records_version: u64,
    pub(crate) tables_sha256: Option<String>,
    pub(crate) entries_sha256: String,
}

impl PageState {
    /// The cursor of the page of `part` that begins after `after` rows.
    fn cursor(&self, part: &GoalPagePart, after: u32) -> GoalPageCursor {
        GoalPageCursor {
            records_version: part.pages_records().then_some(self.records_version),
            tables_sha256: self.tables_sha256.clone(),
            entries_sha256: self.entries_sha256.clone(),
            part_sha256: part.digest(),
            after,
        }
    }

    /// The rows `cursor` (of `part`, as the request's check holds it) says were answered, or why
    /// it is refused: a declaration that moved (the tables' sha256, the records entries; every
    /// part), else, for `records` alone (Addendum 1), stored records that moved.
    fn after(
        &self,
        part: &GoalPagePart,
        cursor: Option<&GoalPageCursor>,
    ) -> Result<u32, PageFailure> {
        let Some(cursor) = cursor else {
            return Ok(0);
        };
        if cursor.tables_sha256 != self.tables_sha256
            || cursor.entries_sha256 != self.entries_sha256
        {
            return Err(PageFailure::DeclarationMoved);
        }
        if part.pages_records() && cursor.records_version != Some(self.records_version) {
            return Err(PageFailure::RecordsMoved);
        }
        Ok(cursor.after)
    }
}

/// The `records` entries of `declaration`, in file order, with their table and bound.
fn records_entries(
    declaration: Option<&StandardPackGoals>,
) -> impl Iterator<Item = (&DeclaredGoal, &String, u64)> {
    declaration
        .into_iter()
        .flat_map(|declaration| declaration.entries.iter())
        .filter_map(|goal| match &goal.kind {
            DeclaredGoalKind::Records { table, max_records } => Some((goal, table, *max_records)),
            _ => None,
        })
}

/// The digest of the records entries of `declaration` (each name, table and `max_records`, in file
/// order) a cursor carries (`entries_sha256`).
pub(crate) fn entries_digest(declaration: Option<&StandardPackGoals>) -> String {
    let mut digest = GoalPageDigest::new();
    for (goal, table, max_records) in records_entries(declaration) {
        digest.text(&goal.name);
        digest.text(table);
        digest.number(max_records);
    }
    digest.finish()
}

/// What the host read of the page under the gates: the accepted `goals.json` and
/// `goal-tables.json` (none for a file not accepted), the workbench's root as the needs read it,
/// the stored goal records, and what a cursor is compared with now.
pub(crate) struct PageRead<'a> {
    pub(crate) declaration: Option<&'a StandardPackGoals>,
    pub(crate) tables: Option<&'a StandardPackGoalTables>,
    pub(crate) root: Option<&'a RecordsRoot>,
    pub(crate) stored: &'a StoredGoalRecords,
    pub(crate) now: PageState,
}

/// The head of a page of `instance_alias`, its members but the rows: the game, the goal page's
/// `version`, what the workbench holds of `goals.json` (`declaration`) and of `goal-tables.json`
/// (`tables`), and the Runtime's clock at the read; no row, none in all, no cut and no next.
pub(crate) fn page_head(
    instance_alias: &str,
    game_id: &str,
    version: u64,
    declaration: GoalDeclarationState,
    tables: GoalPageTables,
    read_at_unix_ms: u64,
) -> GoalPageView {
    GoalPageView {
        instance_alias: instance_alias.to_owned(),
        game_id: game_id.to_owned(),
        version,
        declaration,
        tables,
        read_at_unix_ms,
        fresh_until_unix_ms: None,
        records_state: None,
        ladder: None,
        rows: Vec::new(),
        rows_total: 0,
        cut: None,
        next: None,
    }
}

/// One page of `part` after `cursor`, at most `limit` rows (`None`: the part's default), of the
/// page `read` holds, reading the progress lists through `sources` (module doc).
pub(crate) fn read_part(
    mut head: GoalPageView,
    read: &PageRead<'_>,
    sources: &dyn NeedsSources,
    part: &GoalPagePart,
    cursor: Option<&GoalPageCursor>,
    limit: Option<u32>,
) -> Result<GoalPageView, PageFailure> {
    let after = read.now.after(part, cursor)?;
    let limit = limit.unwrap_or_else(|| part.default_limit());
    let next_at = |answered: u32| read.now.cursor(part, answered);
    match part {
        GoalPagePart::Records => {
            let records: &[StoredRecordRow] = match read.stored {
                StoredGoalRecords::Read(records) => records,
                StoredGoalRecords::Absent | StoredGoalRecords::Unreadable => &[],
            };
            let walk = walk_records(records, read.root, sources);
            let rows = records_rows(read, records, &walk.records)?;
            head.records_state = Some(match read.stored {
                StoredGoalRecords::Absent => GoalPageRecordsState::Absent,
                StoredGoalRecords::Read(_) => GoalPageRecordsState::Read,
                StoredGoalRecords::Unreadable => GoalPageRecordsState::Unreadable,
            });
            head.fresh_until_unix_ms = walk.fresh_until_unix_ms;
            let total = rows.len();
            let start = start_of(after, total)?;
            page(
                head,
                total,
                after,
                limit,
                rows.into_iter().skip(start).map(Ok),
                next_at,
            )
        }
        GoalPagePart::Options { entry } => {
            let (table, naming) = entry_table(read, entry)?;
            let items = table
                .columns
                .iter()
                .map(OptionItem::Column)
                .chain(table.classes.iter().map(OptionItem::Class))
                .chain(table.keys.iter().map(OptionItem::Key))
                .collect::<Vec<_>>();
            let total = items.len();
            let start = start_of(after, total)?;
            let mut lists = ProgressLists::new(sources);
            // Fix round 1: the lists are read as the page's keys are, so the head is measured
            // with the freshness at its largest and given its value once the page is built.
            head.fresh_until_unix_ms = Some(u64::MAX);
            let rows = items[start..].iter().map(|item| {
                Ok(match item {
                    OptionItem::Column(column) => column_row(column, naming),
                    OptionItem::Class(class) => class_row(class, naming),
                    OptionItem::Key(key) => {
                        let (held, lanes) = read_key(table, key, entry, &mut lists);
                        key_row(table, key, naming, &held, lanes)
                    }
                })
            });
            let mut answered = page(head, total, after, limit, rows, next_at)?;
            answered.fresh_until_unix_ms = lists.fresh_until_unix_ms();
            Ok(answered)
        }
        GoalPagePart::Positions { entry, ladder } => {
            let (table, naming) = entry_table(read, entry)?;
            let (read_ladder, column, sub, class) = ladder_ref(table, ladder)?;
            head.ladder = Some(GoalPageLadder {
                source: source_of(table, column),
                class: class.map(str::to_owned),
                positions: index_of(read_ladder.positions.len()),
            });
            let total = read_ladder.positions.len();
            let start = start_of(after, total)?;
            let remaining = &read_ladder.positions[start..];
            let rows = remaining.iter().enumerate().map(|(offset, position)| {
                Ok(GoalPageRow::Position {
                    index: index_of(start + offset),
                    position: position.clone(),
                    said: said(naming, column, sub, position),
                })
            });
            page(head, total, after, limit, rows, next_at)
        }
    }
}

/// The index of the first row after `after` rows answered, of a part of `total` rows; more rows
/// answered than the part holds is no cursor of it.
fn start_of(after: u32, total: usize) -> Result<usize, PageFailure> {
    usize::try_from(after)
        .ok()
        .filter(|start| *start <= total)
        .ok_or(PageFailure::Invalid)
}

/// A count or an index within the contract's bounds (at most 1024 rows of a kind, 512 positions).
fn index_of(index: usize) -> u16 {
    u16::try_from(index).unwrap_or(u16::MAX)
}

/// The page of `head`: of a part of `total` rows, the `rows` after `after`, at most `limit`, cut
/// before the row that would make the serialized page pass [`MAX_GOAL_PAGE_VIEW_BYTES`] (counted
/// with its largest `next` and `cut`), at least one; `next` (`next_at` the rows answered) and
/// `cut` exactly when rows remain.
fn page(
    mut head: GoalPageView,
    total: usize,
    after: u32,
    limit: u32,
    rows: impl Iterator<Item = Result<GoalPageRow, PageFailure>>,
    next_at: impl Fn(u32) -> GoalPageCursor,
) -> Result<GoalPageView, PageFailure> {
    let total = u32::try_from(total).unwrap_or(u32::MAX);
    head.rows_total = total;
    head.rows = Vec::new();
    head.next = Some(next_at(total));
    head.cut = Some(GoalPageCut::Bytes);
    let mut used = encoded_len(&head)?;
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    let mut taken = Vec::new();
    let mut cut = None;
    for row in rows {
        if taken.len() >= limit {
            cut = Some(GoalPageCut::Count);
            break;
        }
        let row = row?;
        // The row and the comma before it.
        let size = encoded_len(&row)?.saturating_add(1);
        if !taken.is_empty() && used.saturating_add(size) > MAX_GOAL_PAGE_VIEW_BYTES {
            cut = Some(GoalPageCut::Bytes);
            break;
        }
        used = used.saturating_add(size);
        taken.push(row);
    }
    let answered = after.saturating_add(u32::try_from(taken.len()).unwrap_or(u32::MAX));
    head.rows = taken;
    head.next = (answered < total).then(|| next_at(answered));
    head.cut = head.next.as_ref().and(cut);
    Ok(head)
}

/// The length of `value` serialized.
fn encoded_len(value: &impl Serialize) -> Result<usize, PageFailure> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|_| PageFailure::Unencodable)
}

/// The rows of `records`: of `read`'s stored `records` (in their order), each judged by the walk
/// (`judged`, in the same order).
fn records_rows(
    read: &PageRead<'_>,
    records: &[StoredRecordRow],
    judged: &[RecordJudgement],
) -> Result<Vec<GoalPageRow>, PageFailure> {
    let walked = records.iter().zip(judged).collect::<Vec<_>>();
    let mut rows = Vec::new();
    let mut declared = BTreeSet::new();
    let mut before: Option<&str> = None;
    for goal in read
        .declaration
        .into_iter()
        .flat_map(|declaration| declaration.entries.iter())
    {
        if let DeclaredGoalKind::Records { table, .. } = &goal.kind {
            declared.insert(goal.name.as_str());
            let table = read.tables.and_then(|tables| tables.table(table));
            let of_entry = walked
                .iter()
                .filter(|(record, _)| record.entry == goal.name)
                .collect::<Vec<_>>();
            rows.push(GoalPageRow::Entry {
                name: goal.name.clone(),
                declared: true,
                after_entry: before.map(str::to_owned),
                entry: Some(declared_entry(goal)),
                applies: table.is_some(),
                problem: table
                    .is_none()
                    .then(|| GoalProblem::NameUnknown.as_str().to_owned()),
                records_total: index_of(of_entry.len()),
            });
            for (record, judgement) in of_entry {
                rows.push(capped_record_row(read, table, record, judgement)?);
            }
        }
        before = Some(goal.name.as_str());
    }
    let mut undeclared = Vec::<&str>::new();
    for record in records {
        if !declared.contains(record.entry.as_str()) && !undeclared.contains(&record.entry.as_str())
        {
            undeclared.push(record.entry.as_str());
        }
    }
    for name in undeclared {
        let of_entry = walked
            .iter()
            .filter(|(record, _)| record.entry == name)
            .collect::<Vec<_>>();
        rows.push(GoalPageRow::Entry {
            name: name.to_owned(),
            declared: false,
            after_entry: None,
            entry: None,
            applies: false,
            problem: Some(GoalProblem::NameUnknown.as_str().to_owned()),
            records_total: index_of(of_entry.len()),
        });
        for (record, judgement) in of_entry {
            rows.push(capped_record_row(read, None, record, judgement)?);
        }
    }
    Ok(rows)
}

/// The row of `record`, with its texts, or without them (`texts_left_out`) when with them it would
/// pass [`MAX_GOAL_PAGE_VIEW_ROW_BYTES`].
fn capped_record_row(
    read: &PageRead<'_>,
    table: Option<&GoalTable>,
    record: &StoredRecordRow,
    judgement: &RecordJudgement,
) -> Result<GoalPageRow, PageFailure> {
    let naming = table.and_then(|table| read.tables?.names.table(&table.name));
    let row = record_row(table, naming, record, judgement, true)?;
    if encoded_len(&row)? <= MAX_GOAL_PAGE_VIEW_ROW_BYTES {
        return Ok(row);
    }
    record_row(table, naming, record, judgement, false)
}

/// The row of `record` of an entry whose table is `table` (named by `naming`), as the walk judged
/// it (`judgement`), with the pack's texts when `texts`.
fn record_row(
    table: Option<&GoalTable>,
    naming: Option<&GoalTableNaming>,
    record: &StoredRecordRow,
    judgement: &RecordJudgement,
    texts: bool,
) -> Result<GoalPageRow, PageFailure> {
    let naming = naming.filter(|_| texts);
    let key = table.and_then(|table| table.key(&record.key));
    // Fix round 1: the lanes in the table's order (`key_lanes`: the columns in the file's order
    // and, of a column of `per: "sub"`, its subs in the column's), a lane only a prerequisite
    // raised in its column's place; the walk's map is read as it is.
    let lanes = match (judgement, table, key) {
        (Ok(walked), Some(table), Some(key)) => key_lanes(table, key)
            .into_iter()
            .filter_map(|lane_key| {
                let lane = walked.lanes.get(&lane_key)?;
                page_lane(table, key, naming, &lane_key, lane)
            })
            .collect(),
        _ => Vec::new(),
    };
    Ok(GoalPageRow::Record {
        entry: record.entry.clone(),
        key: record.key.clone(),
        key_name: named(naming.and_then(|naming| naming.key(&record.key))),
        to: record.target().ok_or(PageFailure::RecordUnread)?,
        set: record.set_by().ok_or(PageFailure::RecordUnread)?,
        applied: judgement.is_ok(),
        problem: judgement
            .as_ref()
            .err()
            .map(|problem| problem.as_str().to_owned()),
        held: judgement.as_ref().ok().map(|walked| held_of(&walked.held)),
        lanes,
        texts_left_out: !texts,
    })
}

/// The lane `lane_key` of a record of `key` as the walk read it (`lane`); none when the key has no
/// such ladder (which an applied record's lane always has).
fn page_lane(
    table: &GoalTable,
    key: &GoalKey,
    naming: Option<&GoalTableNaming>,
    lane_key: &LaneKey,
    lane: &WalkedLane,
) -> Option<GoalPageLane> {
    let (column, sub) = (lane_key.0.as_str(), lane_key.1.as_deref());
    let ladder = ladder_of(table, key, column, sub)?;
    let at = |index: usize| lane_position(ladder, index, naming, column, sub);
    let raised = lane.target.is_none_or(|own| lane.walked_to > own);
    let (current, unknown) = match &lane.current {
        Some(Ok(index)) => (at(*index), None),
        Some(Err(note)) => (None, lane_unknown(note)),
        None => (None, None),
    };
    Some(GoalPageLane {
        column: column.to_owned(),
        sub: sub.map(str::to_owned),
        column_name: named(naming.and_then(|naming| naming.column(column))),
        sub_name: named(sub.and_then(|sub| naming?.sub(column, sub))),
        source: source_of(table, column),
        target: lane.target.and_then(at),
        raised_to: if raised { at(lane.walked_to) } else { None },
        current,
        unknown,
        conditions: lane
            .conditions
            .iter()
            .map(|condition| GoalPageCondition {
                condition: condition.clone(),
                name: named(naming.and_then(|naming| naming.condition(condition))),
            })
            .collect(),
    })
}

/// The position `index` of `ladder` (of the column `column`, the sub `sub`), as the pack says it.
fn lane_position(
    ladder: &GoalLadder,
    index: usize,
    naming: Option<&GoalTableNaming>,
    column: &str,
    sub: Option<&str>,
) -> Option<GoalLanePosition> {
    let position = ladder.positions.get(index)?;
    Some(GoalLanePosition {
        index: index_of(index),
        position: position.clone(),
        said: said(naming, column, sub, position),
    })
}

/// How the pack says `position` of the column `column` (the sub `sub`).
fn said(
    naming: Option<&GoalTableNaming>,
    column: &str,
    sub: Option<&str>,
    position: &[i64],
) -> Option<GoalPageText> {
    naming
        .and_then(|naming| say_goal_position(naming, column, sub, position))
        .map(GoalPageText)
}

/// A text of the pack, as the page carries it.
fn named(text: Option<&GoalText>) -> Option<GoalPageText> {
    text.map(|text| GoalPageText(text.clone()))
}

/// Where the ladders of the column `column` of `table` are written.
fn source_of(table: &GoalTable, column: &str) -> GoalLadderSource {
    match table.column(column).map(|declared| declared.read.per()) {
        Some(GoalColumnPer::Class) => GoalLadderSource::Class,
        Some(GoalColumnPer::Key | GoalColumnPer::Sub) | None => GoalLadderSource::Own,
    }
}

/// A key's held state as the page says it.
fn held_of(held: &KeyHeld) -> GoalKeyHeld {
    let (state, fact_key, unknown_word) = match held {
        KeyHeld::Every => (GoalHeldState::Held, None, None),
        KeyHeld::Held { fact_key } => (GoalHeldState::Held, Some(fact_key.clone()), None),
        KeyHeld::NotHeld { fact_key } => (GoalHeldState::NotHeld, Some(fact_key.clone()), None),
        KeyHeld::Unknown { fact_key, reason } => (
            GoalHeldState::Unknown,
            Some(fact_key.clone()),
            Some(reason.token().to_owned()),
        ),
    };
    GoalKeyHeld {
        state,
        fact_key,
        unknown_word,
    }
}

/// Why a current position is not known, as the walk noted it (`note`, one of the four notes of a
/// reading); none for another note.
fn lane_unknown(note: &RecordNeedsNote) -> Option<GoalLaneUnknown> {
    let (code, fact_key, reading, unknown_word) = match note {
        RecordNeedsNote::ProgressUnknown {
            fact_key, reason, ..
        } => (
            PolicyCode::GoalRecordsProgressUnknown,
            Some(fact_key.clone()),
            None,
            Some(reason.token().to_owned()),
        ),
        RecordNeedsNote::RowMissing { fact_key, .. } => (
            PolicyCode::GoalRecordsRowMissing,
            Some(fact_key.clone()),
            None,
            None,
        ),
        RecordNeedsNote::ReadingMissing {
            fact_key, reading, ..
        } => (
            PolicyCode::GoalRecordsReadingMissing,
            Some(fact_key.clone()),
            Some(reading.clone()),
            None,
        ),
        RecordNeedsNote::PositionUnknown { .. } => {
            (PolicyCode::GoalRecordsPositionUnknown, None, None, None)
        }
        _ => return None,
    };
    Some(GoalLaneUnknown {
        code: CodeStr::from(code.code()),
        fact_key,
        reading,
        unknown_word,
    })
}

/// The table of the records entry `entry` the root applies, with its names; else the refusal
/// `name_unknown` at the part's entry.
fn entry_table<'a>(
    read: &PageRead<'a>,
    entry: &str,
) -> Result<(&'a GoalTable, Option<&'a GoalTableNaming>), PageFailure> {
    let unknown = PageFailure::Refused {
        problem: GoalProblem::NameUnknown,
        pointer: AT_ENTRY,
    };
    let Some((_, table, _)) =
        records_entries(read.declaration).find(|(goal, _, _)| goal.name == entry)
    else {
        return Err(unknown);
    };
    let tables = read.tables.ok_or_else(|| unknown.clone())?;
    let table = tables.table(table).ok_or(unknown)?;
    Ok((table, tables.names.table(&table.name)))
}

/// What [`ladder_ref`] finds: a ladder, its column, its sub and the class whose ladder it is.
type NamedLadder<'t> = (&'t GoalLadder, &'t str, Option<&'t str>, Option<&'t str>);

/// The ladder `ladder` names in `table`, with its column, its sub and the class whose ladder it
/// is; else the refusal: a key or a class the table does not hold `not_an_option`, a ladder it does
/// not have `out_of_range`, each at its pointer.
fn ladder_ref<'t>(
    table: &'t GoalTable,
    ladder: &'t GoalLadderRef,
) -> Result<NamedLadder<'t>, PageFailure> {
    let refused = |problem, pointer| PageFailure::Refused { problem, pointer };
    match ladder {
        GoalLadderRef::Class { class, column } => {
            let declared = table
                .class(class)
                .ok_or_else(|| refused(GoalProblem::NotAnOption, AT_CLASS))?;
            let found = declared
                .ladder(column)
                .ok_or_else(|| refused(GoalProblem::OutOfRange, AT_COLUMN))?;
            Ok((found, column.as_str(), None, Some(declared.name.as_str())))
        }
        GoalLadderRef::Key { key, column, sub } => {
            let declared = table
                .key(key)
                .ok_or_else(|| refused(GoalProblem::NotAnOption, AT_KEY))?;
            let found = ladder_of(table, declared, column, sub.as_deref()).ok_or_else(|| {
                refused(
                    GoalProblem::OutOfRange,
                    if sub.is_some() { AT_SUB } else { AT_COLUMN },
                )
            })?;
            let class = (source_of(table, column) == GoalLadderSource::Class)
                .then_some(declared.class.as_str());
            Ok((found, column.as_str(), sub.as_deref(), class))
        }
    }
}

/// One row of `options` before it is built.
enum OptionItem<'t> {
    Column(&'t GoalColumn),
    Class(&'t GoalClass),
    Key(&'t GoalKey),
}

/// The row of the column `column`, named by `naming`.
fn column_row(column: &GoalColumn, naming: Option<&GoalTableNaming>) -> GoalPageRow {
    let subs = match &column.read {
        GoalColumnRead::Sub(subs) => subs
            .iter()
            .map(|(sub, _)| GoalPageSub {
                sub: sub.clone(),
                name: named(naming.and_then(|naming| naming.sub(&column.name, sub))),
            })
            .collect(),
        GoalColumnRead::Class(_) | GoalColumnRead::Key(_) => Vec::new(),
    };
    let requires = column
        .requires
        .iter()
        .map(|requirement| match requirement {
            GoalRequirement::Column {
                column: required,
                at_least,
                from,
            } => GoalPageRequirement::Column(GoalPageColumnRequirement {
                column: required.clone(),
                at_least: at_least.clone(),
                at_least_said: said(naming, required, None, at_least),
                from: from.clone(),
                from_said: from
                    .as_ref()
                    .and_then(|from| said(naming, &column.name, None, from)),
            }),
            GoalRequirement::Condition(condition) => {
                GoalPageRequirement::Condition(GoalPageCondition {
                    condition: condition.clone(),
                    name: named(naming.and_then(|naming| naming.condition(condition))),
                })
            }
        })
        .collect();
    GoalPageRow::Column {
        column: column.name.clone(),
        name: named(naming.and_then(|naming| naming.column(&column.name))),
        per: column.read.per().as_str().to_owned(),
        subs,
        requires,
    }
}

/// The row of the class `class`, named by `naming`.
fn class_row(class: &GoalClass, naming: Option<&GoalTableNaming>) -> GoalPageRow {
    GoalPageRow::Class {
        class: class.name.clone(),
        name: named(naming.and_then(|naming| naming.class(&class.name))),
    }
}

/// The row of the key `key` of `table`, named by `naming`, with its held state and each of its
/// lanes (`crate::goal_needs::key_lanes`) as the walk's readings read them (`read_key`).
fn key_row(
    table: &GoalTable,
    key: &GoalKey,
    naming: Option<&GoalTableNaming>,
    held: &KeyHeld,
    lanes: Vec<(LaneKey, Option<LaneCurrent>)>,
) -> GoalPageRow {
    let lanes = lanes
        .into_iter()
        .filter_map(|((column, sub), current)| {
            let ladder = ladder_of(table, key, &column, sub.as_deref())?;
            let (current, unknown) = match current {
                Some(Ok(index)) => (
                    ladder.positions.get(index).map(|position| GoalKeyPosition {
                        index: index_of(index),
                        position: position.clone(),
                    }),
                    None,
                ),
                Some(Err(note)) => (None, lane_unknown(&note)),
                None => (None, None),
            };
            let source = source_of(table, &column);
            let positions = index_of(ladder.positions.len());
            Some(GoalKeyLane {
                column,
                sub,
                source,
                positions,
                current,
                unknown,
            })
        })
        .collect();
    GoalPageRow::Key {
        key: key.name.clone(),
        name: named(naming.and_then(|naming| naming.key(&key.name))),
        class: key.class.clone(),
        held: held_of(held),
        lanes,
    }
}
