// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #455 RR1 (register §1.535; `contracts/standard-pack-goal-tables.md`): a game root's
//! table of the game's rules for raising, `goal-tables.json` (`actingcommand.goal-tables.v1`),
//! beside `goals.json`. A `records` entry of `goals.json` names one of its tables; a record (a
//! later slice's) names a key of that table and the grade to raise it to. The file says what each
//! grade costs and how the costs come apart, never how many of anything an account holds or wants:
//!
//! - `items`: every item a step, a recipe or a key names, each with the pool that holds it and, for
//!   an item that is made, its `recipe` (what one making turns out, what it is made from and what
//!   making it costs). An item with a recipe is taken apart; one without is where taking apart
//!   stops, and the need of it lands on its pool.
//! - `tables`: each with the lists its progress is read from (`progress`, list readings by fact
//!   key), its columns (each read from reading columns, with its prerequisites), its classes with
//!   their ladders and its keys with their class, their own items (`own`, by role) and their
//!   overrides.
//!
//! Two pure steps, as for `goals.json`. [`parse_standard_pack_goal_tables`] reads the file alone:
//! its forms and limits, every name used declared, the ladders well formed, the recipes and the
//! prerequisites free of cycles, an item with a recipe naming a pool, and each prerequisite's
//! position held by the classes and keys that have steps in the requiring column.
//! [`check_standard_pack_goal_table_references`] then holds an accepted file's pools to the
//! composed root, with the same [`GoalReferences`] the goal check holds `goals.json` to. Every
//! violation is one [`StandardPackRootIssue`] of [`STANDARD_PACK_GOAL_TABLES_FILE`] at its JSON
//! pointer, at most [`crate::MAX_STANDARD_PACK_GOALS_ISSUES`]. Nothing is computed here: what a
//! record needs is a later slice's.
//!
//! The core stays game-neutral: every name is an opaque string, compared for equality.
//!
//! Workflow #455 RR4a (register §1.546; `contracts/standard-pack-goal-tables.md`, "Names for
//! people"): the document may have a fifth member, `names`, the names a person reads of what the
//! file declares (its items; a table's keys, columns, subs, classes and conditions) and how a
//! position of a column, or of one sub, is said ([`GoalTableNames`]), each text the localized text
//! of a `goals.json` entry's `title` ([`GoalText`], [`Check::text`]). The reader reads it last of
//! the forms, against what the file declares; a file without it reads as before, its names empty.
//! [`say_goal_position`] says a position; nothing else is computed from a name.

use crate::standard_pack::{StandardPackProblem, StandardPackRootIssue};
use crate::standard_pack_catalog::{MAX_PACK_CATALOG_TITLE_BYTES, at, read_document};
use crate::standard_pack_goals::{
    Check, GoalReferences, GoalText, MAX_GOAL_NAME_BYTES, MAX_GOAL_REFERENCE_BYTES, goal_name,
};
use crate::taskflow::{
    MAX_LIST_READING_COLUMNS, list_reading_column_valid, list_reading_fact_key_valid,
};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The game root's table of the game's rules for raising (RR1).
pub const STANDARD_PACK_GOAL_TABLES_FILE: &str = "goal-tables.json";
/// The one version of `goal-tables.json` this build reads.
pub const STANDARD_PACK_GOAL_TABLES_SCHEMA_VERSION: &str = "actingcommand.goal-tables.v1";
/// The `package` interface revision a bundle that carries `goal-tables.json` declares: `[28, 28]`
/// (`contracts/component-interfaces.md`, the `package` row 28, with `goals.json` version 3).
pub const GOAL_TABLES_PACKAGE_REVISION: u64 = 28;
/// The largest `goal-tables.json`, in bytes (4 MiB).
pub const MAX_STANDARD_PACK_GOAL_TABLES_BYTES: usize = 4 * 1_048_576;
/// The most items of `items`.
pub const MAX_GOAL_TABLE_ITEMS: usize = 1024;
/// The most tables of `tables`.
pub const MAX_GOAL_TABLES: usize = 16;
/// The most progress lists of a table.
pub const MAX_GOAL_TABLE_PROGRESS_LISTS: usize = 4;
/// The most columns of a table.
pub const MAX_GOAL_TABLE_COLUMNS: usize = 8;
/// The most reading columns a column (or a sub) reads, the width of its positions.
pub const MAX_GOAL_TABLE_READ: usize = 4;
/// The most subs of a column of `per: "sub"`.
pub const MAX_GOAL_TABLE_SUBS: usize = 8;
/// The most prerequisites of a column.
pub const MAX_GOAL_TABLE_REQUIRES: usize = 4;
/// The most classes of a table.
pub const MAX_GOAL_TABLE_CLASSES: usize = 64;
/// The most keys of a table: the rows a list reading holds.
pub const MAX_GOAL_TABLE_KEYS: usize = 1024;
/// The fewest positions of a ladder.
pub const MIN_GOAL_TABLE_POSITIONS: usize = 2;
/// The most positions of a ladder.
pub const MAX_GOAL_TABLE_POSITIONS: usize = 512;
/// The most items one step uses.
pub const MAX_GOAL_TABLE_STEP_USES: usize = 16;
/// The most items a recipe's `from` or `cost` names.
pub const MAX_GOAL_RECIPE_INPUTS: usize = 16;
/// The most roles a key's `own` names.
pub const MAX_GOAL_KEY_ROLES: usize = 8;
/// The longest name of an item or a key, in bytes.
pub const MAX_GOAL_TABLE_ID_BYTES: usize = 128;
/// Workflow #455 RR4a: the `package` interface revision a bundle whose `goal-tables.json` writes
/// `names` declares: `[32, 32]` (`contracts/component-interfaces.md`, the `package` row 32); a file
/// without the member keeps [`GOAL_TABLES_PACKAGE_REVISION`] ([`goal_tables_package_revision`]).
pub const GOAL_TABLE_NAMES_PACKAGE_REVISION: u64 = 32;
/// Workflow #455 RR4a: the fewest segments of one list of how positions are said.
pub const MIN_GOAL_POSITION_SEGMENTS: usize = 1;
/// Workflow #455 RR4a: the most segments of one list of how positions are said.
pub const MAX_GOAL_POSITION_SEGMENTS: usize = 64;

/// The prefix of a step's member that names a role of the key, not an item: `@<role>`.
const ROLE_PREFIX: &str = "@";
/// Workflow #455 RR4a: the document's member that holds the names a person reads.
const NAMES_MEMBER: &str = "names";
/// Workflow #455 RR4a: the members of one table's names.
const TABLE_NAMING_MEMBERS: [&str; 7] = [
    "keys",
    "columns",
    "subs",
    "classes",
    "conditions",
    "positions",
    "sub_positions",
];
/// The longest name of a reading column (a list reading's column), in bytes.
const MAX_READING_COLUMN_BYTES: usize = 32;
/// The members of a key that are no column's: no column may be named like them.
const KEY_MEMBERS: [&str; 2] = ["class", "own"];

/// A `goal-tables.json` that keeps every rule of the reader, in the file's order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardPackGoalTables {
    /// The game directory the file belongs to (its `game`).
    pub game: String,
    pub items: Vec<GoalTableItem>,
    pub tables: Vec<GoalTable>,
    /// Workflow #455 RR4a: the names a person reads, as `names` writes them; empty for a file
    /// without the member. The Runtime computes nothing from them.
    pub names: GoalTableNames,
}

impl StandardPackGoalTables {
    /// The table named `name`.
    pub fn table(&self, name: &str) -> Option<&GoalTable> {
        self.tables.iter().find(|table| table.name == name)
    }

    /// The item named `name`.
    pub fn item(&self, name: &str) -> Option<&GoalTableItem> {
        self.items.iter().find(|item| item.name == name)
    }

    /// The names of the tables, which a `records` entry of `goals.json` may name
    /// ([`GoalReferences::tables`]).
    pub fn table_names(&self) -> BTreeSet<String> {
        self.tables.iter().map(|table| table.name.clone()).collect()
    }
}

/// One item: its name, the pool that holds it (none: it is shown and weighs nothing), and how it
/// is made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalTableItem {
    pub name: String,
    pub pool: Option<String>,
    pub recipe: Option<GoalRecipe>,
}

/// How an item is made: one making turns out `makes` of it from `from` and costs `cost`, each an
/// item with its amount, in the file's order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalRecipe {
    pub makes: u64,
    pub from: Vec<(String, u64)>,
    pub cost: Vec<(String, u64)>,
}

/// One table: its name, the lists its progress is read from, its columns, classes and keys, each
/// in the file's order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalTable {
    pub name: String,
    pub progress: Vec<GoalProgressList>,
    pub columns: Vec<GoalColumn>,
    pub classes: Vec<GoalClass>,
    pub keys: Vec<GoalKey>,
}

impl GoalTable {
    /// The column named `name`.
    pub fn column(&self, name: &str) -> Option<&GoalColumn> {
        self.columns.iter().find(|column| column.name == name)
    }

    /// The class named `name`.
    pub fn class(&self, name: &str) -> Option<&GoalClass> {
        self.classes.iter().find(|class| class.name == name)
    }

    /// The key named `name`.
    pub fn key(&self, name: &str) -> Option<&GoalKey> {
        self.keys.iter().find(|key| key.name == name)
    }

    /// The ladders `key` has in the column `column`: its class's ladder of a column of
    /// `per: "class"` (whose positions no override changes), its own of one of `per: "key"`, and
    /// one for each of its subs of one of `per: "sub"`; none when it has no steps there.
    pub fn ladders_of<'a>(&'a self, key: &'a GoalKey, column: &str) -> Vec<&'a GoalLadder> {
        let Some(declared) = self.column(column) else {
            return Vec::new();
        };
        match declared.read.per() {
            GoalColumnPer::Class => self
                .class(&key.class)
                .and_then(|class| class.ladder(column))
                .into_iter()
                .collect(),
            GoalColumnPer::Key | GoalColumnPer::Sub => key
                .columns
                .iter()
                .filter(|(name, _)| name == column)
                .flat_map(|(_, written)| match written {
                    GoalKeyColumn::Steps(_) => Vec::new(),
                    GoalKeyColumn::Ladder(ladder) => vec![ladder],
                    GoalKeyColumn::Subs(subs) => subs.iter().map(|(_, ladder)| ladder).collect(),
                })
                .collect(),
        }
    }
}

/// One list a table's progress is read from: the list reading's `fact_key`, the column whose
/// value is a key of the table (`key_column`), whether a list read whole that holds no row of a
/// key says the key is not held (`held_by`, at most one list of a table), and the reading columns
/// it gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalProgressList {
    pub fact_key: String,
    pub key_column: String,
    pub held_by: bool,
    pub columns: Vec<String>,
}

/// One column of a table: its name, the reading columns its positions are read from, and its
/// prerequisites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalColumn {
    pub name: String,
    pub read: GoalColumnRead,
    pub requires: Vec<GoalRequirement>,
}

/// Where a column's ladders are written and what their positions are read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalColumnRead {
    /// `per: "class"`: in each class (a key may override a step), its positions read from these
    /// reading columns.
    Class(Vec<String>),
    /// `per: "key"`: in each key, its positions read from these reading columns.
    Key(Vec<String>),
    /// `per: "sub"`: in each key, one ladder for each sub, each read from its reading columns.
    Sub(Vec<(String, Vec<String>)>),
}

impl GoalColumnRead {
    /// The word of `per` this reading is written with.
    pub fn per(&self) -> GoalColumnPer {
        match self {
            Self::Class(_) => GoalColumnPer::Class,
            Self::Key(_) => GoalColumnPer::Key,
            Self::Sub(_) => GoalColumnPer::Sub,
        }
    }
}

/// A column's `per`: where its ladders are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GoalColumnPer {
    Class,
    Key,
    Sub,
}

impl GoalColumnPer {
    /// The three words, in the contract's order.
    pub const ALL: [Self; 3] = [Self::Class, Self::Key, Self::Sub];

    /// The word in the file.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Class => "class",
            Self::Key => "key",
            Self::Sub => "sub",
        }
    }

    /// The `per` a word names; `None` for any other word.
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|per| per.as_str() == word)
    }
}

/// A position on a ladder: one integer for each reading column its column reads, in their order.
pub type GoalPosition = Vec<i64>;

/// One prerequisite of a column (fix round 1: what it means,
/// `contracts/standard-pack-goal-tables.md`, "Prerequisites").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalRequirement {
    /// The column `column`, another column of the table and not one of `per: "sub"`, stands at
    /// `at_least` or beyond: at that position or at one after it in the `positions` list of the
    /// ladder the key walks in `column` (its class's or its own), the ladder's order, never the
    /// numbers compared. Without `from`, before any step of the requiring column is taken; with
    /// `from`, a position of the requiring column's own ladder (of each of a key's subs for a
    /// column of `per: "sub"`), before a step that arrives at `from` or beyond is taken, and a step
    /// that arrives before it asks nothing.
    Column {
        column: String,
        at_least: GoalPosition,
        from: Option<GoalPosition>,
    },
    /// A condition that is no column's (a word of the file's), marked and not counted.
    Condition(String),
}

/// One class of a table: its ladders, by column, in the file's order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalClass {
    pub name: String,
    pub ladders: Vec<(String, GoalLadder)>,
}

impl GoalClass {
    /// The class's ladder of the column `column`.
    pub fn ladder(&self, column: &str) -> Option<&GoalLadder> {
        self.ladders
            .iter()
            .find(|(name, _)| name == column)
            .map(|(_, ladder)| ladder)
    }
}

/// A ladder: its positions, each distinct and of its column's width, and one step fewer, each what
/// going from one position to the next uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalLadder {
    pub positions: Vec<GoalPosition>,
    pub steps: Vec<GoalStep>,
}

impl GoalLadder {
    /// Whether `position` is one of the ladder's positions.
    pub fn holds(&self, position: &[i64]) -> bool {
        self.positions.iter().any(|written| written == position)
    }
}

/// What one step uses: each item or role with its amount, in the file's order.
pub type GoalStep = Vec<(GoalUse, u64)>;

/// What a step's member names.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GoalUse {
    /// An item of `items`.
    Item(String),
    /// A role (`@<role>`), the item of which each key names in its `own`.
    Role(String),
}

/// One key of a table: its class, its own items by role and what it writes of each column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalKey {
    pub name: String,
    pub class: String,
    pub own: Vec<(String, String)>,
    pub columns: Vec<(String, GoalKeyColumn)>,
}

/// What a key writes of one column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalKeyColumn {
    /// Of a column of `per: "class"`: the steps of its class's ladder it uses instead, by index.
    Steps(Vec<(usize, GoalStep)>),
    /// Of a column of `per: "key"`: its ladder.
    Ladder(GoalLadder),
    /// Of a column of `per: "sub"`: its ladder of each sub.
    Subs(Vec<(String, GoalLadder)>),
}

/// Workflow #455 RR4a: texts by the id of what they name, in the file's order, each a localized
/// text of a `goals.json` entry's `title` ([`GoalText`]).
pub type GoalNamed = Vec<(String, GoalText)>;
/// Workflow #455 RR4a: how the positions of one column, or of one sub, are said: its segments, in
/// the file's order.
pub type GoalSegments = Vec<GoalPositionSegment>;
/// Workflow #455 RR4a: the lists of a column's subs that have their own ([`GoalSegments`]), by
/// sub, in the file's order.
pub type GoalSubSegments = Vec<(String, GoalSegments)>;

/// Workflow #455 RR4a (`contracts/standard-pack-goal-tables.md`, "Names for people"): the names a
/// person reads, as the file's `names` writes them, each in the file's order. Nothing must be
/// named: what has no name here is shown by its id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GoalTableNames {
    /// The items' names, by item.
    pub items: GoalNamed,
    /// The tables' names, by table.
    pub tables: Vec<(String, GoalTableNaming)>,
}

impl GoalTableNames {
    /// The name of the item `item`.
    pub fn item(&self, item: &str) -> Option<&GoalText> {
        find_named(&self.items, item)
    }

    /// The names of the table `table`.
    pub fn table(&self, table: &str) -> Option<&GoalTableNaming> {
        find_named(&self.tables, table)
    }
}

/// Workflow #455 RR4a: one table's names, each by what it names, in the file's order, and how its
/// positions are said ([`say_goal_position`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GoalTableNaming {
    pub keys: GoalNamed,
    pub columns: GoalNamed,
    /// By column of `per: "sub"`, its subs' names.
    pub subs: Vec<(String, GoalNamed)>,
    pub classes: GoalNamed,
    /// The names of conditions the table's prerequisites name.
    pub conditions: GoalNamed,
    /// By column, how its positions are said (for a column of `per: "sub"`, every sub without a
    /// list of its own).
    pub positions: Vec<(String, GoalSegments)>,
    /// By column of `per: "sub"`, the lists of the subs that have their own.
    pub sub_positions: Vec<(String, GoalSubSegments)>,
}

impl GoalTableNaming {
    /// The name of the key `key`.
    pub fn key(&self, key: &str) -> Option<&GoalText> {
        find_named(&self.keys, key)
    }

    /// The name of the column `column`.
    pub fn column(&self, column: &str) -> Option<&GoalText> {
        find_named(&self.columns, column)
    }

    /// The name of the sub `sub` of the column `column`.
    pub fn sub(&self, column: &str, sub: &str) -> Option<&GoalText> {
        find_named(find_named(&self.subs, column)?, sub)
    }

    /// The name of the class `class`.
    pub fn class(&self, class: &str) -> Option<&GoalText> {
        find_named(&self.classes, class)
    }

    /// The name of the condition `condition`.
    pub fn condition(&self, condition: &str) -> Option<&GoalText> {
        find_named(&self.conditions, condition)
    }

    /// The segments of the column `column`'s `positions`.
    pub fn segments(&self, column: &str) -> Option<&[GoalPositionSegment]> {
        find_named(&self.positions, column).map(Vec::as_slice)
    }

    /// The segments the sub `sub` of the column `column` has of its own (`sub_positions`).
    pub fn sub_segments(&self, column: &str, sub: &str) -> Option<&[GoalPositionSegment]> {
        find_named(find_named(&self.sub_positions, column)?, sub).map(Vec::as_slice)
    }
}

/// Workflow #455 RR4a: one segment of how positions are said: the first elements of a position it
/// says (`prefix`, 0 to the width's elements) and its text, whose strings may hold placeholders
/// `{0}` to `{3}`, each naming an element of the position by its index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalPositionSegment {
    pub prefix: GoalPosition,
    pub text: GoalText,
}

/// Workflow #455 RR4a (`contracts/standard-pack-goal-tables.md`, "Names for people"): how the table
/// whose names are `naming` says the position `position` of its column `column`, of the sub `sub`
/// for a column of `per: "sub"`: by the sub's own list when the file gives it one, else by the
/// column's `positions`; of that list, by the segment whose prefix equals the position's first
/// elements, the longest such; its text in every language it is written in, each placeholder `{i}`
/// filled with the decimal number of the position's element `i`. `None` when no list or no segment
/// says it, or a placeholder names an element the position does not have. It chooses no language:
/// the caller shows the languages as the pack wrote them.
pub fn say_goal_position(
    naming: &GoalTableNaming,
    column: &str,
    sub: Option<&str>,
    position: &[i64],
) -> Option<GoalText> {
    let list = sub
        .and_then(|named| naming.sub_segments(column, named))
        .or_else(|| naming.segments(column))?;
    let said = list
        .iter()
        .filter(|segment| position.starts_with(&segment.prefix))
        .max_by_key(|segment| segment.prefix.len())?;
    said.text
        .iter()
        .map(|(language, template)| Some((language.clone(), fill_template(template, position)?)))
        .collect()
}

/// Workflow #455 RR4a (`contracts/component-interfaces.md`, the `package` rows 28 and 32): the
/// `package` revision a bundle zip that carries `goal-tables.json` (`bytes`) needs: 32
/// ([`GOAL_TABLE_NAMES_PACKAGE_REVISION`]) for a document that writes the member `names`, whatever
/// it holds, 28 ([`GOAL_TABLES_PACKAGE_REVISION`]) for any other, as the revision of
/// `pack-catalog.json` is read from its bytes (`catalog_package_revision`; a file the bundle step
/// refuses is never built into a zip).
pub fn goal_tables_package_revision(bytes: &[u8]) -> u64 {
    let document = serde_json::from_slice::<Value>(bytes).ok();
    let names = document
        .as_ref()
        .and_then(Value::as_object)
        .is_some_and(|members| members.contains_key(NAMES_MEMBER));
    if names {
        GOAL_TABLE_NAMES_PACKAGE_REVISION
    } else {
        GOAL_TABLES_PACKAGE_REVISION
    }
}

/// The value named `name` of a list of names in the file's order.
fn find_named<'n, T>(list: &'n [(String, T)], name: &str) -> Option<&'n T> {
    list.iter()
        .find(|(named, _)| named == name)
        .map(|(_, found)| found)
}

/// Workflow #455 RR4a: a piece of one string of a segment's text: a run of characters, or a
/// placeholder naming an element of the position by its index.
enum TemplatePiece<'t> {
    Run(&'t str),
    Element(usize),
}

/// Workflow #455 RR4a: the pieces of the segment's string `template`: every `{` begins a
/// placeholder `{<i>}` of one decimal digit `i` below [`MAX_GOAL_TABLE_READ`] (`{0}` to `{3}`), and
/// every `}` ends one; `None` when a brace does not. Outside a segment's text a brace is an
/// ordinary character.
fn template_pieces(template: &str) -> Option<Vec<TemplatePiece<'_>>> {
    let mut pieces = Vec::new();
    let mut rest = template;
    while let Some(brace) = rest.find(['{', '}']) {
        let (run, from) = rest.split_at(brace);
        if !run.is_empty() {
            pieces.push(TemplatePiece::Run(run));
        }
        let [b'{', digit @ b'0'..=b'9', b'}', ..] = from.as_bytes() else {
            return None;
        };
        let index = usize::from(digit - b'0');
        if index >= MAX_GOAL_TABLE_READ {
            return None;
        }
        pieces.push(TemplatePiece::Element(index));
        rest = &from[3..];
    }
    if !rest.is_empty() {
        pieces.push(TemplatePiece::Run(rest));
    }
    Some(pieces)
}

/// Workflow #455 RR4a: `template` with each placeholder filled with the decimal number of the
/// element of `position` it names; `None` when a brace is no placeholder or a placeholder names an
/// element the position does not have.
fn fill_template(template: &str, position: &[i64]) -> Option<String> {
    let mut filled = String::with_capacity(template.len());
    for piece in template_pieces(template)? {
        match piece {
            TemplatePiece::Run(run) => filled.push_str(run),
            TemplatePiece::Element(index) => filled.push_str(&position.get(index)?.to_string()),
        }
    }
    Some(filled)
}

/// Workflow #455 RR4a: the width a column's `positions` are held to: the count of the reading
/// columns it reads, for a column of `per: "sub"` its narrowest sub's.
fn column_width(column: &GoalColumn) -> usize {
    match &column.read {
        GoalColumnRead::Class(read) | GoalColumnRead::Key(read) => read.len(),
        GoalColumnRead::Sub(subs) => subs.iter().map(|(_, read)| read.len()).min().unwrap_or(0),
    }
}

/// Workflow #455 RR4a: the subs of the column `column` of `table`, each with its width (the count
/// of the reading columns it reads), when the column is of `per: "sub"`; `None` for any other
/// column and for a name that is no column of the table.
fn sub_widths<'t>(table: &'t GoalTable, column: &str) -> Option<Vec<(&'t str, usize)>> {
    match &table.column(column)?.read {
        GoalColumnRead::Sub(subs) => Some(
            subs.iter()
                .map(|(sub, read)| (sub.as_str(), read.len()))
                .collect(),
        ),
        GoalColumnRead::Class(_) | GoalColumnRead::Key(_) => None,
    }
}

/// `goal-tables.json`'s bytes, read for the game directory `game` (contract "The checks", the
/// file alone). Every violation is one issue at its JSON pointer, at most
/// [`crate::MAX_STANDARD_PACK_GOALS_ISSUES`]: every member repeated in one object first, then the forms
/// and limits in the reader's order (the document's members, `items` in file order, then each table
/// in file order, its members in the order `name`, `progress`, `columns`, `classes`, `keys`), then,
/// for a file whose forms all hold, what the file says of itself: the recipes (an item with a
/// recipe and no pool, an item a recipe names that the file does not declare, the cycles), then
/// each table's prerequisites (an unknown column or one of `per: "sub"`, the cycles), roles and
/// prerequisites' positions. The words: `malformed`, `game_mismatch`, `table_out_of_limits`,
/// `table_name_duplicate`, `table_item_unknown`, `table_role_unknown`, `table_class_unknown`,
/// `table_column_unknown`, `table_ladder_invalid`, `table_recipe_cycle`, `table_requires_cycle`,
/// `table_requires_invalid`, `table_recipe_without_pool`, `table_list_key_invalid` and
/// `table_reading_column_duplicate`; fix round 1: `table_pool_duplicate` (a pool an earlier item
/// names) and `table_list_key_duplicate` (a fact key an earlier list of the table names), each in
/// the reader's order at the later, and a list's key column among its own columns
/// `table_reading_column_duplicate` there. Workflow #455 RR4a: the member `names` is read last of
/// the forms, after the tables, against what the file declares, with the words `malformed` (a form,
/// a key that names nothing the file declares, a repeated prefix, a prefix longer than its width, a
/// brace of a segment's string that is no placeholder or a placeholder at or above its width) and
/// `table_out_of_limits` (a count or a length), each at the pointer of what is wrong.
pub fn parse_standard_pack_goal_tables(
    bytes: &[u8],
    game: &str,
) -> Result<StandardPackGoalTables, Vec<StandardPackRootIssue>> {
    let mut reader = Reader {
        check: Check::with_limits(
            STANDARD_PACK_GOAL_TABLES_FILE,
            StandardPackProblem::TableOutOfLimits,
        ),
    };
    if bytes.len() > MAX_STANDARD_PACK_GOAL_TABLES_BYTES {
        reader.check.out_of_limits("");
        return Err(reader.check.into_issues());
    }
    let mut repeated = Vec::new();
    let Some(document) = read_document(bytes, &mut repeated) else {
        reader.check.malformed("");
        return Err(reader.check.into_issues());
    };
    for pointer in repeated {
        reader.check.malformed(pointer);
    }
    let read = reader.document(&document, game);
    if let Some(tables) = &read
        && reader.check.issues().is_empty()
    {
        reader.within(tables);
    }
    match read {
        Some(tables) if reader.check.issues().is_empty() => Ok(tables),
        // Every path that gives no tables records why; an empty list would hide a refusal.
        _ if reader.check.issues().is_empty() => {
            reader.check.malformed("");
            Err(reader.check.into_issues())
        }
        _ => Err(reader.check.into_issues()),
    }
}

/// The pools of an accepted `goal-tables.json`, held to the composed root (contract "The checks",
/// against the root), item by item in file order: `table_pool_unknown` (an item's pool is no pool
/// of the composed `schedule.json`, [`GoalReferences::pools`]) and `table_pool_unvalued` (the pool
/// of an item without a recipe, the one a need lands on, is one a goal cannot be put on: the
/// judgement `goal_pool_unvalued` reads, the same map). The pool of an item with a recipe need only
/// be a pool of the root (every pool declares its reading); an item without a pool names nothing.
pub fn check_standard_pack_goal_table_references(
    tables: &StandardPackGoalTables,
    references: &GoalReferences,
) -> Vec<StandardPackRootIssue> {
    let mut check = Check::with_limits(
        STANDARD_PACK_GOAL_TABLES_FILE,
        StandardPackProblem::TableOutOfLimits,
    );
    for item in &tables.items {
        let Some(pool) = &item.pool else {
            continue;
        };
        let at_pool = at(&at("/items", &item.name), "pool");
        match references.pools.get(pool) {
            None => check.issue(StandardPackProblem::TablePoolUnknown, at_pool),
            Some(false) if item.recipe.is_none() => {
                check.issue(StandardPackProblem::TablePoolUnvalued, at_pool);
            }
            Some(_) => {}
        }
    }
    check.into_issues()
}

/// An item's or a key's name: any text without a control character (its length is judged
/// apart); an item's never begins with the role prefix `@`.
fn opaque(name: &str) -> bool {
    !name.chars().any(char::is_control)
}

/// An item's name ([`opaque`], not beginning with `@`).
fn item_word(name: &str) -> bool {
    !name.starts_with(ROLE_PREFIX) && opaque(name)
}

/// A step override's index: a whole number in decimal without a leading zero.
fn step_index(word: &str) -> Option<usize> {
    let canonical = !word.is_empty()
        && word.bytes().all(|byte| byte.is_ascii_digit())
        && (word == "0" || !word.starts_with('0'));
    if canonical {
        word.parse::<usize>().ok()
    } else {
        None
    }
}

/// Whether `start` reaches itself through `edges` (each name to the names it leads to).
fn reaches_itself(start: &str, edges: &BTreeMap<&str, Vec<&str>>) -> bool {
    let mut seen = BTreeSet::new();
    let mut next = edges.get(start).cloned().unwrap_or_default();
    while let Some(name) = next.pop() {
        if name == start {
            return true;
        }
        if seen.insert(name) {
            next.extend(edges.get(name).into_iter().flatten().copied());
        }
    }
    false
}

/// The checks of one `goal-tables.json`: the issues found so far.
struct Reader {
    check: Check,
}

impl Reader {
    fn issue(&mut self, problem: StandardPackProblem, pointer: impl Into<String>) {
        self.check.issue(problem, pointer);
    }

    /// A map (an object whose members are names the file chooses) of `min..=max` members; a count
    /// outside is `table_out_of_limits` at the map.
    fn map<'v>(
        &mut self,
        value: &'v Value,
        pointer: &str,
        (min, max): (usize, usize),
    ) -> Option<&'v Map<String, Value>> {
        let Some(map) = value.as_object() else {
            self.check.malformed(pointer);
            return None;
        };
        if !(min..=max).contains(&map.len()) {
            self.check.out_of_limits(pointer);
        }
        Some(map)
    }

    /// Whether `name` is 1 to `max` bytes (`table_out_of_limits` otherwise) of the form `form`
    /// (`malformed` otherwise), at `pointer`.
    fn named(&mut self, name: &str, pointer: &str, max: usize, form: fn(&str) -> bool) -> bool {
        if name.is_empty() || name.len() > max {
            self.check.out_of_limits(pointer);
            false
        } else if form(name) {
            true
        } else {
            self.check.malformed(pointer);
            false
        }
    }

    /// A string that is a name ([`Self::named`]).
    fn name_value(
        &mut self,
        value: &Value,
        pointer: &str,
        max: usize,
        form: fn(&str) -> bool,
    ) -> Option<String> {
        let Some(name) = value.as_str() else {
            self.check.malformed(pointer);
            return None;
        };
        self.named(name, pointer, max, form)
            .then(|| name.to_owned())
    }

    /// An amount: a whole number within the safe integers (`malformed` otherwise) from 1
    /// (`table_out_of_limits` for 0).
    fn amount(&mut self, value: &Value, pointer: &str) -> Option<u64> {
        let amount = self.check.count(value, pointer)?;
        if amount == 0 {
            self.check.out_of_limits(pointer);
            return None;
        }
        Some(amount)
    }

    fn document(&mut self, value: &Value, game: &str) -> Option<StandardPackGoalTables> {
        let document = self.check.object(
            value,
            "",
            &["schema_version", "game", "items", "tables"],
            &[NAMES_MEMBER],
        )?;
        match document.get("schema_version").map(Value::as_str) {
            None | Some(Some(STANDARD_PACK_GOAL_TABLES_SCHEMA_VERSION)) => {}
            Some(_) => self.check.malformed("/schema_version"),
        }
        match document.get("game").map(Value::as_str) {
            Some(None) => self.check.malformed("/game"),
            Some(Some(declared)) if declared != game => {
                self.issue(StandardPackProblem::GameMismatch, "/game");
            }
            _ => {}
        }
        let items = document.get("items").and_then(|items| self.items(items));
        // The names the file declares as items, which a step names; read from the map itself, so
        // that an item with another problem is no unknown item too.
        let names = document
            .get("items")
            .and_then(Value::as_object)
            .map(|items| items.keys().cloned().collect::<BTreeSet<_>>())
            .unwrap_or_default();
        let tables = document
            .get("tables")
            .and_then(|tables| self.tables(tables, &names));
        let (items, tables) = (items?, tables?);
        // Workflow #455 RR4a: the names, read against what the file declares, so after the
        // tables; a file without them reads as before.
        let named = match document.get(NAMES_MEMBER) {
            None => GoalTableNames::default(),
            Some(declared) => self.names(declared, &items, &tables)?,
        };
        Some(StandardPackGoalTables {
            game: game.to_owned(),
            items,
            tables,
            names: named,
        })
    }

    /// Workflow #455 RR4a: `names`, `{items?, tables?}`, read against the items and the tables the
    /// file declares (`items`, `tables`): every key of its maps names something the file declares
    /// (`malformed` at the key otherwise: an unknown member), and every text is a `goals.json`
    /// entry's `title` ([`Check::text`], its bounds and its word for a bound,
    /// `table_out_of_limits`).
    fn names(
        &mut self,
        value: &Value,
        items: &[GoalTableItem],
        tables: &[GoalTable],
    ) -> Option<GoalTableNames> {
        let pointer = at("", NAMES_MEMBER);
        let object = self
            .check
            .object(value, &pointer, &[], &["items", "tables"])?;
        let named_items = self.named_member(object, &pointer, "items", |name| {
            items.iter().any(|item| item.name == name)
        });
        let named_tables = match object.get("tables") {
            None => Some(Vec::new()),
            Some(map) => self.named_tables(map, &at(&pointer, "tables"), tables),
        };
        Some(GoalTableNames {
            items: named_items?,
            tables: named_tables?,
        })
    }

    /// The member `member` of `object`, the object at `pointer`: a map of texts by what they name
    /// ([`Self::named_texts`]); none when it is absent.
    fn named_member(
        &mut self,
        object: &Map<String, Value>,
        pointer: &str,
        member: &str,
        declared: impl Fn(&str) -> bool,
    ) -> Option<GoalNamed> {
        match object.get(member) {
            None => Some(Vec::new()),
            Some(map) => self.named_texts(map, &at(pointer, member), declared),
        }
    }

    /// A map of texts by what they name: each key one `declared` takes (`malformed` at the key
    /// otherwise), each text a title's ([`Check::text`]).
    fn named_texts(
        &mut self,
        value: &Value,
        pointer: &str,
        declared: impl Fn(&str) -> bool,
    ) -> Option<GoalNamed> {
        let Some(map) = value.as_object() else {
            self.check.malformed(pointer);
            return None;
        };
        let mut named = Vec::new();
        let mut complete = true;
        for (name, written) in map {
            let at_name = at(pointer, name);
            if !declared(name.as_str()) {
                self.check.malformed(at_name);
                complete = false;
                continue;
            }
            match self
                .check
                .text(written, &at_name, MAX_PACK_CATALOG_TITLE_BYTES)
            {
                Some(text) => named.push((name.clone(), text)),
                None => complete = false,
            }
        }
        complete.then_some(named)
    }

    /// `names.tables`: by table, a table the file declares (`malformed` at its key otherwise), its
    /// names ([`Self::table_naming`]).
    fn named_tables(
        &mut self,
        value: &Value,
        pointer: &str,
        tables: &[GoalTable],
    ) -> Option<Vec<(String, GoalTableNaming)>> {
        let Some(map) = value.as_object() else {
            self.check.malformed(pointer);
            return None;
        };
        let mut named = Vec::new();
        let mut complete = true;
        for (name, naming) in map {
            let at_table = at(pointer, name);
            let Some(table) = tables.iter().find(|declared| declared.name == *name) else {
                self.check.malformed(at_table);
                complete = false;
                continue;
            };
            match self.table_naming(naming, &at_table, table) {
                Some(read) => named.push((name.clone(), read)),
                None => complete = false,
            }
        }
        complete.then_some(named)
    }

    /// One table's names, `{keys?, columns?, subs?, classes?, conditions?, positions?,
    /// sub_positions?}`, read against `table`: its keys, its columns, the subs of its columns of
    /// `per: "sub"`, its classes, the conditions its prerequisites name, and its columns' widths.
    fn table_naming(
        &mut self,
        value: &Value,
        pointer: &str,
        table: &GoalTable,
    ) -> Option<GoalTableNaming> {
        let object = self
            .check
            .object(value, pointer, &[], &TABLE_NAMING_MEMBERS)?;
        let conditions = table
            .columns
            .iter()
            .flat_map(|column| &column.requires)
            .filter_map(|requirement| match requirement {
                GoalRequirement::Condition(condition) => Some(condition.as_str()),
                GoalRequirement::Column { .. } => None,
            })
            .collect::<BTreeSet<_>>();
        let keys = self.named_member(object, pointer, "keys", |name| table.key(name).is_some());
        let columns = self.named_member(object, pointer, "columns", |name| {
            table.column(name).is_some()
        });
        let subs = match object.get("subs") {
            None => Some(Vec::new()),
            Some(map) => self.named_subs(map, &at(pointer, "subs"), table),
        };
        let classes = self.named_member(object, pointer, "classes", |name| {
            table.class(name).is_some()
        });
        let named_conditions = self.named_member(object, pointer, "conditions", |name| {
            conditions.contains(name)
        });
        let positions = match object.get("positions") {
            None => Some(Vec::new()),
            Some(map) => self.position_lists(map, &at(pointer, "positions"), table),
        };
        let sub_positions = match object.get("sub_positions") {
            None => Some(Vec::new()),
            Some(map) => self.sub_position_lists(map, &at(pointer, "sub_positions"), table),
        };
        Some(GoalTableNaming {
            keys: keys?,
            columns: columns?,
            subs: subs?,
            classes: classes?,
            conditions: named_conditions?,
            positions: positions?,
            sub_positions: sub_positions?,
        })
    }

    /// A table's `subs`: by column, one of `per: "sub"` (`malformed` at its key otherwise, a name
    /// that is no column of the table too), the names of its subs.
    fn named_subs(
        &mut self,
        value: &Value,
        pointer: &str,
        table: &GoalTable,
    ) -> Option<Vec<(String, GoalNamed)>> {
        let Some(map) = value.as_object() else {
            self.check.malformed(pointer);
            return None;
        };
        let mut named = Vec::new();
        let mut complete = true;
        for (column, texts) in map {
            let at_column = at(pointer, column);
            let Some(widths) = sub_widths(table, column) else {
                self.check.malformed(at_column);
                complete = false;
                continue;
            };
            let declared = |sub: &str| widths.iter().any(|(written, _)| *written == sub);
            match self.named_texts(texts, &at_column, declared) {
                Some(read) => named.push((column.clone(), read)),
                None => complete = false,
            }
        }
        complete.then_some(named)
    }

    /// A table's `positions`: by column, a column of the table (`malformed` at its key otherwise),
    /// how its positions are said, held to its width ([`column_width`]).
    fn position_lists(
        &mut self,
        value: &Value,
        pointer: &str,
        table: &GoalTable,
    ) -> Option<Vec<(String, GoalSegments)>> {
        let Some(map) = value.as_object() else {
            self.check.malformed(pointer);
            return None;
        };
        let mut lists = Vec::new();
        let mut complete = true;
        for (column, list) in map {
            let at_column = at(pointer, column);
            let Some(declared) = table.column(column) else {
                self.check.malformed(at_column);
                complete = false;
                continue;
            };
            match self.segment_list(list, &at_column, column_width(declared)) {
                Some(read) => lists.push((column.clone(), read)),
                None => complete = false,
            }
        }
        complete.then_some(lists)
    }

    /// A table's `sub_positions`: by column, one of `per: "sub"` (`malformed` at its key
    /// otherwise), by sub, one of its subs (`malformed` at its key otherwise), how that sub's
    /// positions are said, held to the sub's own width.
    fn sub_position_lists(
        &mut self,
        value: &Value,
        pointer: &str,
        table: &GoalTable,
    ) -> Option<Vec<(String, GoalSubSegments)>> {
        let Some(map) = value.as_object() else {
            self.check.malformed(pointer);
            return None;
        };
        let mut lists = Vec::new();
        let mut complete = true;
        for (column, by_sub) in map {
            let at_column = at(pointer, column);
            let (Some(widths), Some(subs)) = (sub_widths(table, column), by_sub.as_object()) else {
                self.check.malformed(at_column);
                complete = false;
                continue;
            };
            let mut read = Vec::new();
            let mut whole = true;
            for (sub, list) in subs {
                let at_sub = at(&at_column, sub);
                let Some(&(_, width)) = widths.iter().find(|(written, _)| *written == sub.as_str())
                else {
                    self.check.malformed(at_sub);
                    whole = false;
                    continue;
                };
                match self.segment_list(list, &at_sub, width) {
                    Some(segments) => read.push((sub.clone(), segments)),
                    None => whole = false,
                }
            }
            if whole {
                lists.push((column.clone(), read));
            } else {
                complete = false;
            }
        }
        complete.then_some(lists)
    }

    /// How positions of `width` elements are said: [`MIN_GOAL_POSITION_SEGMENTS`] to
    /// [`MAX_GOAL_POSITION_SEGMENTS`] segments (`table_out_of_limits` at the list otherwise), each
    /// prefix written once (the later `malformed` at its `prefix`).
    fn segment_list(&mut self, value: &Value, pointer: &str, width: usize) -> Option<GoalSegments> {
        let list = self.check.list(
            value,
            pointer,
            (MIN_GOAL_POSITION_SEGMENTS, MAX_GOAL_POSITION_SEGMENTS),
        )?;
        let mut prefixes = BTreeSet::new();
        let mut read = Vec::new();
        let mut complete = true;
        for (index, item) in list.iter().enumerate() {
            match self.segment(item, &format!("{pointer}/{index}"), width, &mut prefixes) {
                Some(segment) => read.push(segment),
                None => complete = false,
            }
        }
        complete.then_some(read)
    }

    /// One segment `{prefix, text}`: its `prefix` ([`Self::segment_prefix`]) new to `prefixes`, the
    /// prefixes of the segments before it in its list (`malformed` at it otherwise), and its `text`
    /// ([`Self::segment_text`]).
    fn segment(
        &mut self,
        value: &Value,
        pointer: &str,
        width: usize,
        prefixes: &mut BTreeSet<GoalPosition>,
    ) -> Option<GoalPositionSegment> {
        let object = self
            .check
            .object(value, pointer, &["prefix", "text"], &[])?;
        let at_prefix = at(pointer, "prefix");
        let prefix = object.get("prefix").and_then(|written| {
            let read = self.segment_prefix(written, &at_prefix, width)?;
            if prefixes.insert(read.clone()) {
                Some(read)
            } else {
                self.check.malformed(&at_prefix);
                None
            }
        });
        let text = object
            .get("text")
            .and_then(|written| self.segment_text(written, &at(pointer, "text"), width));
        Some(GoalPositionSegment {
            prefix: prefix?,
            text: text?,
        })
    }

    /// A segment's `prefix`: safe integers (`malformed` at an element otherwise), at most `width`
    /// of them (`malformed` at the prefix otherwise; a width is never more than 4).
    fn segment_prefix(
        &mut self,
        value: &Value,
        pointer: &str,
        width: usize,
    ) -> Option<GoalPosition> {
        let Some(parts) = value.as_array() else {
            self.check.malformed(pointer);
            return None;
        };
        let mut elements = Vec::with_capacity(parts.len());
        for (index, part) in parts.iter().enumerate() {
            if let Some(element) = self.check.integer(part, &format!("{pointer}/{index}")) {
                elements.push(element);
            }
        }
        if elements.len() != parts.len() {
            return None;
        }
        if elements.len() > width {
            self.check.malformed(pointer);
            return None;
        }
        Some(elements)
    }

    /// A segment's `text`: a title's text ([`Check::text`]) every string of which holds a brace
    /// only in a placeholder `{0}` to `{3}` that names an element below `width`
    /// ([`template_pieces`]; `malformed` at the string otherwise).
    fn segment_text(&mut self, value: &Value, pointer: &str, width: usize) -> Option<GoalText> {
        let text = self
            .check
            .text(value, pointer, MAX_PACK_CATALOG_TITLE_BYTES)?;
        let mut complete = true;
        for (language, template) in &text {
            let fills = template_pieces(template).is_some_and(|pieces| {
                pieces.iter().all(|piece| match *piece {
                    TemplatePiece::Run(_) => true,
                    TemplatePiece::Element(index) => index < width,
                })
            });
            if !fills {
                self.check.malformed(at(pointer, language));
                complete = false;
            }
        }
        complete.then_some(text)
    }

    /// `items`: 1 to [`MAX_GOAL_TABLE_ITEMS`] items by name, each pool named by one of them.
    fn items(&mut self, value: &Value) -> Option<Vec<GoalTableItem>> {
        let map = self.map(value, "/items", (1, MAX_GOAL_TABLE_ITEMS))?;
        let mut items = Vec::new();
        let mut pools = BTreeSet::new();
        let mut complete = true;
        for (name, item) in map {
            match self.item(name, item, &at("/items", name), &mut pools) {
                Some(item) => items.push(item),
                None => complete = false,
            }
        }
        complete.then_some(items)
    }

    /// One item `{pool?, recipe?}`, its name of [`item_word`]. Fix round 1: its pool is new to
    /// `pools`, the pools the items before it name (`table_pool_duplicate` at its `pool`
    /// otherwise): a holding has one name in the file, and what a later slice subtracts of a
    /// pool's holding it subtracts once.
    fn item(
        &mut self,
        name: &str,
        value: &Value,
        pointer: &str,
        pools: &mut BTreeSet<String>,
    ) -> Option<GoalTableItem> {
        let named = self.named(name, pointer, MAX_GOAL_TABLE_ID_BYTES, item_word);
        let object = self
            .check
            .object(value, pointer, &[], &["pool", "recipe"])?;
        let at_pool = at(pointer, "pool");
        let pool = match object.get("pool") {
            None => Some(None),
            Some(pool) => self
                .name_value(pool, &at_pool, MAX_GOAL_REFERENCE_BYTES, opaque)
                .and_then(|id| {
                    if pools.insert(id.clone()) {
                        Some(Some(id))
                    } else {
                        self.issue(StandardPackProblem::TablePoolDuplicate, &at_pool);
                        None
                    }
                }),
        };
        let recipe = match object.get("recipe") {
            None => Some(None),
            Some(recipe) => self.recipe(recipe, &at(pointer, "recipe")).map(Some),
        };
        if !named {
            return None;
        }
        Some(GoalTableItem {
            name: name.to_owned(),
            pool: pool?,
            recipe: recipe?,
        })
    }

    /// A `recipe`: `{makes, from, cost?}`, `from` 1 to [`MAX_GOAL_RECIPE_INPUTS`] items, `cost` 0
    /// to as many.
    fn recipe(&mut self, value: &Value, pointer: &str) -> Option<GoalRecipe> {
        let object = self
            .check
            .object(value, pointer, &["makes", "from"], &["cost"])?;
        let makes = object
            .get("makes")
            .and_then(|makes| self.amount(makes, &at(pointer, "makes")));
        let from = object
            .get("from")
            .and_then(|from| self.uses(from, &at(pointer, "from"), 1));
        let cost = match object.get("cost") {
            None => Some(Vec::new()),
            Some(cost) => self.uses(cost, &at(pointer, "cost"), 0),
        };
        Some(GoalRecipe {
            makes: makes?,
            from: from?,
            cost: cost?,
        })
    }

    /// A recipe's `from` or `cost`: `min` to [`MAX_GOAL_RECIPE_INPUTS`] items, each with its
    /// amount. Whether each is declared is judged once the whole file is read.
    fn uses(&mut self, value: &Value, pointer: &str, min: usize) -> Option<Vec<(String, u64)>> {
        let map = self.map(value, pointer, (min, MAX_GOAL_RECIPE_INPUTS))?;
        let mut uses = Vec::new();
        let mut complete = true;
        for (name, amount) in map {
            let at_use = at(pointer, name);
            let named = self.named(name, &at_use, MAX_GOAL_TABLE_ID_BYTES, item_word);
            let amount = self.amount(amount, &at_use);
            match (named, amount) {
                (true, Some(amount)) => uses.push((name.clone(), amount)),
                _ => complete = false,
            }
        }
        complete.then_some(uses)
    }

    /// `tables`: 1 to [`MAX_GOAL_TABLES`] tables, each name once.
    fn tables(&mut self, value: &Value, items: &BTreeSet<String>) -> Option<Vec<GoalTable>> {
        let list = self.check.list(value, "/tables", (1, MAX_GOAL_TABLES))?;
        let mut names = BTreeSet::new();
        let mut tables = Vec::new();
        let mut complete = true;
        for (index, table) in list.iter().enumerate() {
            let pointer = format!("/tables/{index}");
            match self.table(table, &pointer, &mut names, items) {
                Some(table) => tables.push(table),
                None => complete = false,
            }
        }
        complete.then_some(tables)
    }

    /// One table, its members read in the order `name`, `progress`, `columns`, `classes`, `keys`:
    /// the classes and keys are read against the columns, the keys against the classes too, and
    /// neither is read without what it is read against.
    fn table(
        &mut self,
        value: &Value,
        pointer: &str,
        names: &mut BTreeSet<String>,
        items: &BTreeSet<String>,
    ) -> Option<GoalTable> {
        let object = self.check.object(
            value,
            pointer,
            &["name", "progress", "columns", "classes", "keys"],
            &[],
        )?;
        let name = object
            .get("name")
            .and_then(|name| self.unique_name(name, &at(pointer, "name"), names, &[]));
        let progress = object
            .get("progress")
            .and_then(|progress| self.progress(progress, &at(pointer, "progress")));
        let reading = progress.as_ref().map(|lists| {
            lists
                .iter()
                .flat_map(|list| list.columns.iter().cloned())
                .collect::<BTreeSet<_>>()
        });
        let columns = object
            .get("columns")
            .and_then(|columns| self.columns(columns, &at(pointer, "columns"), reading.as_ref()))?;
        let classes = object
            .get("classes")
            .and_then(|classes| self.classes(classes, &at(pointer, "classes"), &columns, items))?;
        let keys = object
            .get("keys")
            .and_then(|keys| self.keys(keys, &at(pointer, "keys"), &columns, &classes, items));
        Some(GoalTable {
            name: name?,
            progress: progress?,
            columns,
            classes,
            keys: keys?,
        })
    }

    /// A name of an entry name's form, new to `names` (`table_name_duplicate` at the later), and
    /// none of `reserved` (`malformed`).
    fn unique_name(
        &mut self,
        value: &Value,
        pointer: &str,
        names: &mut BTreeSet<String>,
        reserved: &[&str],
    ) -> Option<String> {
        let name = self.name_value(value, pointer, MAX_GOAL_NAME_BYTES, goal_name)?;
        if reserved.contains(&name.as_str()) {
            self.check.malformed(pointer);
            return None;
        }
        if names.insert(name.clone()) {
            Some(name)
        } else {
            self.issue(StandardPackProblem::TableNameDuplicate, pointer);
            None
        }
    }

    /// `progress`: 1 to [`MAX_GOAL_TABLE_PROGRESS_LISTS`] lists, at most one of them `held_by`,
    /// their reading columns each listed once among them; fix round 1: each fact key named by one
    /// of them.
    fn progress(&mut self, value: &Value, pointer: &str) -> Option<Vec<GoalProgressList>> {
        let list = self
            .check
            .list(value, pointer, (1, MAX_GOAL_TABLE_PROGRESS_LISTS))?;
        let mut seen = BTreeSet::new();
        let mut keys = BTreeSet::new();
        let mut held = false;
        let mut lists = Vec::new();
        let mut complete = true;
        for (index, item) in list.iter().enumerate() {
            let at_list = format!("{pointer}/{index}");
            match self.progress_list(item, &at_list, &mut seen, &mut keys, &mut held) {
                Some(read) => lists.push(read),
                None => complete = false,
            }
        }
        complete.then_some(lists)
    }

    /// One list `{fact_key, key_column, held_by?, columns}`: a list reading's key
    /// (`table_list_key_invalid` otherwise) that no list before it names (fix round 1,
    /// `table_list_key_duplicate` otherwise, `keys` the ones they name), its key column and 1 to
    /// 63 columns of a list reading's column form, each new to `seen` and, fix round 1, none of
    /// them its key column (`table_reading_column_duplicate` otherwise, at the column), `held_by`
    /// a boolean (a second `true` of the table `malformed`).
    fn progress_list(
        &mut self,
        value: &Value,
        pointer: &str,
        seen: &mut BTreeSet<String>,
        keys: &mut BTreeSet<String>,
        held: &mut bool,
    ) -> Option<GoalProgressList> {
        let object = self.check.object(
            value,
            pointer,
            &["fact_key", "key_column", "columns"],
            &["held_by"],
        )?;
        let fact_key = object
            .get("fact_key")
            .and_then(|key| self.fact_key(key, &at(pointer, "fact_key"), keys));
        let key_column = object.get("key_column").and_then(|column| {
            self.name_value(
                column,
                &at(pointer, "key_column"),
                MAX_READING_COLUMN_BYTES,
                list_reading_column_valid,
            )
        });
        let at_held = at(pointer, "held_by");
        let held_by = match object.get("held_by") {
            None => Some(false),
            Some(Value::Bool(true)) if *held => {
                self.check.malformed(at_held);
                None
            }
            Some(Value::Bool(written)) => {
                *held |= *written;
                Some(*written)
            }
            Some(_) => {
                self.check.malformed(at_held);
                None
            }
        };
        let columns = object.get("columns").and_then(|columns| {
            self.reading_columns(
                columns,
                &at(pointer, "columns"),
                (1, MAX_LIST_READING_COLUMNS),
                seen,
                None,
                key_column.as_deref(),
            )
        });
        Some(GoalProgressList {
            fact_key: fact_key?,
            key_column: key_column?,
            held_by: held_by?,
            columns: columns?,
        })
    }

    /// A list reading's `fact_key` (`table_list_key_invalid` when it is no such key), new to
    /// `keys`, the fact keys of the table's lists before it (fix round 1:
    /// `table_list_key_duplicate` otherwise).
    fn fact_key(
        &mut self,
        value: &Value,
        pointer: &str,
        keys: &mut BTreeSet<String>,
    ) -> Option<String> {
        let Some(key) = value.as_str() else {
            self.check.malformed(pointer);
            return None;
        };
        if !list_reading_fact_key_valid(key) {
            self.issue(StandardPackProblem::TableListKeyInvalid, pointer);
            None
        } else if keys.insert(key.to_owned()) {
            Some(key.to_owned())
        } else {
            self.issue(StandardPackProblem::TableListKeyDuplicate, pointer);
            None
        }
    }

    /// A list of `min..=max` reading column names, each new to `seen`
    /// (`table_reading_column_duplicate` at the later) and, with `known`, one of them
    /// (`table_column_unknown`); fix round 1: with `key`, a progress list's own key column, none of
    /// them `key` (`table_reading_column_duplicate` at it: the list names that column twice).
    fn reading_columns(
        &mut self,
        value: &Value,
        pointer: &str,
        bounds: (usize, usize),
        seen: &mut BTreeSet<String>,
        known: Option<&BTreeSet<String>>,
        key: Option<&str>,
    ) -> Option<Vec<String>> {
        let list = self.check.list(value, pointer, bounds)?;
        let mut columns = Vec::new();
        let mut complete = true;
        for (index, item) in list.iter().enumerate() {
            let at_column = format!("{pointer}/{index}");
            let Some(column) = self.name_value(
                item,
                &at_column,
                MAX_READING_COLUMN_BYTES,
                list_reading_column_valid,
            ) else {
                complete = false;
                continue;
            };
            if known.is_some_and(|known| !known.contains(&column)) {
                self.issue(StandardPackProblem::TableColumnUnknown, at_column);
                complete = false;
            } else if key == Some(column.as_str()) || !seen.insert(column.clone()) {
                self.issue(StandardPackProblem::TableReadingColumnDuplicate, at_column);
                complete = false;
            } else {
                columns.push(column);
            }
        }
        complete.then_some(columns)
    }

    /// `columns`: 1 to [`MAX_GOAL_TABLE_COLUMNS`] columns, each name once.
    fn columns(
        &mut self,
        value: &Value,
        pointer: &str,
        reading: Option<&BTreeSet<String>>,
    ) -> Option<Vec<GoalColumn>> {
        let list = self
            .check
            .list(value, pointer, (1, MAX_GOAL_TABLE_COLUMNS))?;
        let mut names = BTreeSet::new();
        let mut columns = Vec::new();
        let mut complete = true;
        for (index, item) in list.iter().enumerate() {
            let at_column = format!("{pointer}/{index}");
            match self.column(item, &at_column, &mut names, reading) {
                Some(column) => columns.push(column),
                None => complete = false,
            }
        }
        complete.then_some(columns)
    }

    /// One column `{name, per, read, requires?}` of `per` `class` or `key`, or `{name, per, subs,
    /// requires?}` of `per` `sub`; its name is none of a key's own members (`class`, `own`).
    /// `reading` holds the table's reading columns, `None` when its progress could not be read
    /// (then what a column reads is not judged).
    fn column(
        &mut self,
        value: &Value,
        pointer: &str,
        names: &mut BTreeSet<String>,
        reading: Option<&BTreeSet<String>>,
    ) -> Option<GoalColumn> {
        let Some(object) = value.as_object() else {
            self.check.malformed(pointer);
            return None;
        };
        let at_per = at(pointer, "per");
        let per = match object.get("per") {
            None => None,
            Some(Value::String(word)) => GoalColumnPer::from_word(word),
            Some(_) => None,
        };
        if object.contains_key("per") && per.is_none() {
            self.check.malformed(&at_per);
        }
        let required: &[&str] = match per {
            Some(GoalColumnPer::Sub) => &["name", "per", "subs"],
            _ => &["name", "per", "read"],
        };
        self.check.members(object, pointer, required, &["requires"]);
        let name = object
            .get("name")
            .and_then(|name| self.unique_name(name, &at(pointer, "name"), names, &KEY_MEMBERS));
        let read = match per {
            None => None,
            Some(GoalColumnPer::Sub) => object
                .get("subs")
                .and_then(|subs| self.subs(subs, &at(pointer, "subs"), reading))
                .map(GoalColumnRead::Sub),
            Some(per) => object
                .get("read")
                .and_then(|read| {
                    self.reading_columns(
                        read,
                        &at(pointer, "read"),
                        (1, MAX_GOAL_TABLE_READ),
                        &mut BTreeSet::new(),
                        reading,
                        None,
                    )
                })
                .map(|read| match per {
                    GoalColumnPer::Class => GoalColumnRead::Class(read),
                    GoalColumnPer::Key | GoalColumnPer::Sub => GoalColumnRead::Key(read),
                }),
        };
        let requires = match object.get("requires") {
            None => Some(Vec::new()),
            Some(requires) => self.requires(requires, &at(pointer, "requires")),
        };
        Some(GoalColumn {
            name: name?,
            read: read?,
            requires: requires?,
        })
    }

    /// A column's `subs`: 1 to [`MAX_GOAL_TABLE_SUBS`] subs by name, each with 1 to
    /// [`MAX_GOAL_TABLE_READ`] reading columns.
    fn subs(
        &mut self,
        value: &Value,
        pointer: &str,
        reading: Option<&BTreeSet<String>>,
    ) -> Option<Vec<(String, Vec<String>)>> {
        let map = self.map(value, pointer, (1, MAX_GOAL_TABLE_SUBS))?;
        let mut subs = Vec::new();
        let mut complete = true;
        for (sub, read) in map {
            let at_sub = at(pointer, sub);
            let named = self.named(sub, &at_sub, MAX_GOAL_NAME_BYTES, goal_name);
            let read = self.reading_columns(
                read,
                &at_sub,
                (1, MAX_GOAL_TABLE_READ),
                &mut BTreeSet::new(),
                reading,
                None,
            );
            match (named, read) {
                (true, Some(read)) => subs.push((sub.clone(), read)),
                _ => complete = false,
            }
        }
        complete.then_some(subs)
    }

    /// A column's `requires`: 0 to [`MAX_GOAL_TABLE_REQUIRES`] prerequisites.
    fn requires(&mut self, value: &Value, pointer: &str) -> Option<Vec<GoalRequirement>> {
        let list = self
            .check
            .list(value, pointer, (0, MAX_GOAL_TABLE_REQUIRES))?;
        let mut requires = Vec::new();
        let mut complete = true;
        for (index, item) in list.iter().enumerate() {
            match self.requirement(item, &format!("{pointer}/{index}")) {
                Some(requirement) => requires.push(requirement),
                None => complete = false,
            }
        }
        complete.then_some(requires)
    }

    /// One prerequisite: `{condition}`, or `{column, at_least, from?}`. Whether the column is the
    /// table's, and whether its positions are held, is judged once the whole file is read.
    fn requirement(&mut self, value: &Value, pointer: &str) -> Option<GoalRequirement> {
        let Some(object) = value.as_object() else {
            self.check.malformed(pointer);
            return None;
        };
        if object.contains_key("condition") {
            self.check.members(object, pointer, &["condition"], &[]);
            let condition = object.get("condition").and_then(|condition| {
                self.name_value(
                    condition,
                    &at(pointer, "condition"),
                    MAX_GOAL_NAME_BYTES,
                    goal_name,
                )
            })?;
            return Some(GoalRequirement::Condition(condition));
        }
        self.check
            .members(object, pointer, &["column", "at_least"], &["from"]);
        let column = object.get("column").and_then(|column| {
            self.name_value(
                column,
                &at(pointer, "column"),
                MAX_GOAL_NAME_BYTES,
                goal_name,
            )
        });
        let at_least = object
            .get("at_least")
            .and_then(|position| self.position(position, &at(pointer, "at_least")));
        let from = match object.get("from") {
            None => Some(None),
            Some(position) => self.position(position, &at(pointer, "from")).map(Some),
        };
        Some(GoalRequirement::Column {
            column: column?,
            at_least: at_least?,
            from: from?,
        })
    }

    /// A prerequisite's position: 1 to [`MAX_GOAL_TABLE_READ`] safe integers.
    fn position(&mut self, value: &Value, pointer: &str) -> Option<GoalPosition> {
        let list = self.check.list(value, pointer, (1, MAX_GOAL_TABLE_READ))?;
        let mut position = Vec::new();
        let mut complete = true;
        for (index, item) in list.iter().enumerate() {
            match self.check.integer(item, &format!("{pointer}/{index}")) {
                Some(part) => position.push(part),
                None => complete = false,
            }
        }
        complete.then_some(position)
    }

    /// `classes`: 1 to [`MAX_GOAL_TABLE_CLASSES`] classes by name, each with its ladders of the
    /// table's columns of `per: "class"`.
    fn classes(
        &mut self,
        value: &Value,
        pointer: &str,
        columns: &[GoalColumn],
        items: &BTreeSet<String>,
    ) -> Option<Vec<GoalClass>> {
        let map = self.map(value, pointer, (1, MAX_GOAL_TABLE_CLASSES))?;
        let mut classes = Vec::new();
        let mut complete = true;
        for (name, class) in map {
            let at_class = at(pointer, name);
            let named = self.named(name, &at_class, MAX_GOAL_NAME_BYTES, goal_name);
            let ladders = self.class_ladders(class, &at_class, columns, items);
            match (named, ladders) {
                (true, Some(ladders)) => classes.push(GoalClass {
                    name: name.clone(),
                    ladders,
                }),
                _ => complete = false,
            }
        }
        complete.then_some(classes)
    }

    /// A class's ladders by column: a column the table does not declare is
    /// `table_column_unknown`, one not of `per: "class"` `table_ladder_invalid`.
    fn class_ladders(
        &mut self,
        value: &Value,
        pointer: &str,
        columns: &[GoalColumn],
        items: &BTreeSet<String>,
    ) -> Option<Vec<(String, GoalLadder)>> {
        let map = self.map(value, pointer, (0, MAX_GOAL_TABLE_COLUMNS))?;
        let mut ladders = Vec::new();
        let mut complete = true;
        for (column, ladder) in map {
            let at_column = at(pointer, column);
            let Some(declared) = columns.iter().find(|declared| declared.name == *column) else {
                self.issue(StandardPackProblem::TableColumnUnknown, at_column);
                complete = false;
                continue;
            };
            let GoalColumnRead::Class(read) = &declared.read else {
                self.issue(StandardPackProblem::TableLadderInvalid, at_column);
                complete = false;
                continue;
            };
            match self.ladder(ladder, &at_column, read.len(), items) {
                Some(ladder) => ladders.push((column.clone(), ladder)),
                None => complete = false,
            }
        }
        complete.then_some(ladders)
    }

    /// A ladder `{positions, steps}` of positions of `width` integers.
    fn ladder(
        &mut self,
        value: &Value,
        pointer: &str,
        width: usize,
        items: &BTreeSet<String>,
    ) -> Option<GoalLadder> {
        let object = self
            .check
            .object(value, pointer, &["positions", "steps"], &[])?;
        let positions = object
            .get("positions")
            .and_then(|positions| self.positions(positions, &at(pointer, "positions"), width));
        let steps = object
            .get("steps")
            .and_then(|steps| self.steps(steps, &at(pointer, "steps"), items));
        let (positions, steps) = (positions?, steps?);
        if steps.len() + 1 != positions.len() {
            self.issue(
                StandardPackProblem::TableLadderInvalid,
                at(pointer, "steps"),
            );
            return None;
        }
        Some(GoalLadder { positions, steps })
    }

    /// A ladder's `positions`: [`MIN_GOAL_TABLE_POSITIONS`] (`table_ladder_invalid` below) to
    /// [`MAX_GOAL_TABLE_POSITIONS`] (`table_out_of_limits` above) positions, each `width` safe
    /// integers and written once (`table_ladder_invalid` otherwise, at the position).
    fn positions(
        &mut self,
        value: &Value,
        pointer: &str,
        width: usize,
    ) -> Option<Vec<GoalPosition>> {
        let Some(list) = value.as_array() else {
            self.check.malformed(pointer);
            return None;
        };
        if list.len() > MAX_GOAL_TABLE_POSITIONS {
            self.check.out_of_limits(pointer);
            return None;
        }
        if list.len() < MIN_GOAL_TABLE_POSITIONS {
            self.issue(StandardPackProblem::TableLadderInvalid, pointer);
            return None;
        }
        let mut seen = BTreeSet::new();
        let mut positions = Vec::new();
        let mut complete = true;
        for (index, item) in list.iter().enumerate() {
            let at_position = format!("{pointer}/{index}");
            let Some(parts) = item.as_array() else {
                self.check.malformed(&at_position);
                complete = false;
                continue;
            };
            let mut position = Vec::with_capacity(parts.len());
            for (part, part_value) in parts.iter().enumerate() {
                if let Some(read) = self
                    .check
                    .integer(part_value, &format!("{at_position}/{part}"))
                {
                    position.push(read);
                }
            }
            if position.len() != parts.len() {
                complete = false;
            } else if position.len() != width || !seen.insert(position.clone()) {
                self.issue(StandardPackProblem::TableLadderInvalid, at_position);
                complete = false;
            } else {
                positions.push(position);
            }
        }
        complete.then_some(positions)
    }

    /// A ladder's `steps`, at most one fewer than the most positions.
    fn steps(
        &mut self,
        value: &Value,
        pointer: &str,
        items: &BTreeSet<String>,
    ) -> Option<Vec<GoalStep>> {
        let Some(list) = value.as_array() else {
            self.check.malformed(pointer);
            return None;
        };
        if list.len() >= MAX_GOAL_TABLE_POSITIONS {
            self.check.out_of_limits(pointer);
            return None;
        }
        let mut steps = Vec::new();
        let mut complete = true;
        for (index, item) in list.iter().enumerate() {
            match self.step(item, &format!("{pointer}/{index}"), items) {
                Some(step) => steps.push(step),
                None => complete = false,
            }
        }
        complete.then_some(steps)
    }

    /// One step: 0 to [`MAX_GOAL_TABLE_STEP_USES`] items or roles, each with its amount.
    fn step(&mut self, value: &Value, pointer: &str, items: &BTreeSet<String>) -> Option<GoalStep> {
        let map = self.map(value, pointer, (0, MAX_GOAL_TABLE_STEP_USES))?;
        let mut step = Vec::new();
        let mut complete = true;
        for (word, amount) in map {
            let at_use = at(pointer, word);
            let used = self.used(word, &at_use, items);
            let amount = self.amount(amount, &at_use);
            match (used, amount) {
                (Some(used), Some(amount)) => step.push((used, amount)),
                _ => complete = false,
            }
        }
        complete.then_some(step)
    }

    /// A step's member: `@<role>`, a role of an entry name's form, or an item the file declares
    /// (`table_item_unknown` otherwise).
    fn used(&mut self, word: &str, pointer: &str, items: &BTreeSet<String>) -> Option<GoalUse> {
        if let Some(role) = word.strip_prefix(ROLE_PREFIX) {
            return self
                .named(role, pointer, MAX_GOAL_NAME_BYTES, goal_name)
                .then(|| GoalUse::Role(role.to_owned()));
        }
        if !self.named(word, pointer, MAX_GOAL_TABLE_ID_BYTES, item_word) {
            return None;
        }
        if items.contains(word) {
            Some(GoalUse::Item(word.to_owned()))
        } else {
            self.issue(StandardPackProblem::TableItemUnknown, pointer);
            None
        }
    }

    /// `keys`: 1 to [`MAX_GOAL_TABLE_KEYS`] keys by name.
    fn keys(
        &mut self,
        value: &Value,
        pointer: &str,
        columns: &[GoalColumn],
        classes: &[GoalClass],
        items: &BTreeSet<String>,
    ) -> Option<Vec<GoalKey>> {
        let map = self.map(value, pointer, (1, MAX_GOAL_TABLE_KEYS))?;
        let mut keys = Vec::new();
        let mut complete = true;
        for (name, key) in map {
            let at_key = at(pointer, name);
            let named = self.named(name, &at_key, MAX_GOAL_TABLE_ID_BYTES, opaque);
            let key = self.key(name, key, &at_key, columns, classes, items);
            match (named, key) {
                (true, Some(key)) => keys.push(key),
                _ => complete = false,
            }
        }
        complete.then_some(keys)
    }

    /// One key `{class, own?, <column>...}`: its class one of the table's
    /// (`table_class_unknown`), its own items declared, and every other member a column of the
    /// table (`table_column_unknown`) that it writes as the column's `per` says.
    fn key(
        &mut self,
        name: &str,
        value: &Value,
        pointer: &str,
        columns: &[GoalColumn],
        classes: &[GoalClass],
        items: &BTreeSet<String>,
    ) -> Option<GoalKey> {
        let Some(object) = value.as_object() else {
            self.check.malformed(pointer);
            return None;
        };
        let at_class = at(pointer, "class");
        let class = match object.get("class") {
            None => {
                self.check.malformed(&at_class);
                None
            }
            Some(class) => self.key_class(class, &at_class, classes),
        };
        let own = match object.get("own") {
            None => Some(Vec::new()),
            Some(own) => self.own(own, &at(pointer, "own"), items),
        };
        let of_class = class
            .as_ref()
            .and_then(|class| classes.iter().find(|declared| declared.name == *class));
        let mut written = Vec::new();
        let mut complete = true;
        for (member, member_value) in object {
            if KEY_MEMBERS.contains(&member.as_str()) {
                continue;
            }
            let at_member = at(pointer, member);
            let Some(column) = columns.iter().find(|column| column.name == *member) else {
                self.issue(StandardPackProblem::TableColumnUnknown, at_member);
                complete = false;
                continue;
            };
            match self.key_column(member_value, &at_member, column, of_class, items) {
                Some(key_column) => written.push((member.clone(), key_column)),
                None => complete = false,
            }
        }
        let (class, own) = (class?, own?);
        complete.then(|| GoalKey {
            name: name.to_owned(),
            class,
            own,
            columns: written,
        })
    }

    /// A key's `class`: a class of the table.
    fn key_class(&mut self, value: &Value, pointer: &str, classes: &[GoalClass]) -> Option<String> {
        let class = self.name_value(value, pointer, MAX_GOAL_NAME_BYTES, goal_name)?;
        if classes.iter().any(|declared| declared.name == class) {
            Some(class)
        } else {
            self.issue(StandardPackProblem::TableClassUnknown, pointer);
            None
        }
    }

    /// A key's `own`: 0 to [`MAX_GOAL_KEY_ROLES`] roles, each with the item the file declares
    /// that the role names for the key.
    fn own(
        &mut self,
        value: &Value,
        pointer: &str,
        items: &BTreeSet<String>,
    ) -> Option<Vec<(String, String)>> {
        let map = self.map(value, pointer, (0, MAX_GOAL_KEY_ROLES))?;
        let mut own = Vec::new();
        let mut complete = true;
        for (role, item) in map {
            let at_role = at(pointer, role);
            let named = self.named(role, &at_role, MAX_GOAL_NAME_BYTES, goal_name);
            let item = self.name_value(item, &at_role, MAX_GOAL_TABLE_ID_BYTES, item_word);
            match (named, item) {
                (true, Some(item)) if items.contains(&item) => own.push((role.clone(), item)),
                (true, Some(_)) => {
                    self.issue(StandardPackProblem::TableItemUnknown, at_role);
                    complete = false;
                }
                _ => complete = false,
            }
        }
        complete.then_some(own)
    }

    /// What a key writes of `column`, as its `per` says: of `class`, `{steps: {<index>: step}}`,
    /// each index a step of its class's ladder of the column (`table_ladder_invalid` when the class,
    /// `class` when it is known, has no such ladder or no such step); of `key`, a ladder; of `sub`,
    /// a ladder of each of the column's subs it writes (`table_column_unknown` for another).
    fn key_column(
        &mut self,
        value: &Value,
        pointer: &str,
        column: &GoalColumn,
        class: Option<&GoalClass>,
        items: &BTreeSet<String>,
    ) -> Option<GoalKeyColumn> {
        match &column.read {
            GoalColumnRead::Class(_) => {
                let object = self.check.object(value, pointer, &["steps"], &[])?;
                let at_steps = at(pointer, "steps");
                let steps = self.map(
                    object.get("steps")?,
                    &at_steps,
                    (1, MAX_GOAL_TABLE_POSITIONS),
                )?;
                let ladder = class.and_then(|class| class.ladder(&column.name));
                if class.is_some() && ladder.is_none() {
                    self.issue(StandardPackProblem::TableLadderInvalid, pointer);
                    return None;
                }
                let mut overrides = Vec::new();
                let mut complete = true;
                for (index, step) in steps {
                    let at_step = at(&at_steps, index);
                    let Some(position) = step_index(index) else {
                        self.check.malformed(&at_step);
                        complete = false;
                        continue;
                    };
                    if ladder.is_some_and(|ladder| position >= ladder.steps.len()) {
                        self.issue(StandardPackProblem::TableLadderInvalid, at_step);
                        complete = false;
                        continue;
                    }
                    match self.step(step, &at_step, items) {
                        Some(step) => overrides.push((position, step)),
                        None => complete = false,
                    }
                }
                complete.then_some(GoalKeyColumn::Steps(overrides))
            }
            GoalColumnRead::Key(read) => self
                .ladder(value, pointer, read.len(), items)
                .map(GoalKeyColumn::Ladder),
            GoalColumnRead::Sub(subs) => {
                let map = self.map(value, pointer, (1, MAX_GOAL_TABLE_SUBS))?;
                let mut ladders = Vec::new();
                let mut complete = true;
                for (sub, ladder) in map {
                    let at_sub = at(pointer, sub);
                    let Some((_, read)) = subs.iter().find(|(declared, _)| declared == sub) else {
                        self.issue(StandardPackProblem::TableColumnUnknown, at_sub);
                        complete = false;
                        continue;
                    };
                    match self.ladder(ladder, &at_sub, read.len(), items) {
                        Some(ladder) => ladders.push((sub.clone(), ladder)),
                        None => complete = false,
                    }
                }
                complete.then_some(GoalKeyColumn::Subs(ladders))
            }
        }
    }

    /// What the file says of itself, judged once its forms all hold: the recipes, then each
    /// table's prerequisites, roles and prerequisites' positions.
    fn within(&mut self, tables: &StandardPackGoalTables) {
        let declared = tables
            .items
            .iter()
            .map(|item| item.name.as_str())
            .collect::<BTreeSet<_>>();
        for item in &tables.items {
            let Some(recipe) = &item.recipe else {
                continue;
            };
            let pointer = at("/items", &item.name);
            // Its holding is what taking it apart subtracts: without a pool its need could never
            // be known.
            if item.pool.is_none() {
                self.issue(StandardPackProblem::TableRecipeWithoutPool, &pointer);
            }
            let at_recipe = at(&pointer, "recipe");
            for (member, uses) in [("from", &recipe.from), ("cost", &recipe.cost)] {
                let at_member = at(&at_recipe, member);
                for (used, _) in uses {
                    if !declared.contains(used.as_str()) {
                        self.issue(StandardPackProblem::TableItemUnknown, at(&at_member, used));
                    }
                }
            }
        }
        let made_from = tables
            .items
            .iter()
            .filter_map(|item| {
                let recipe = item.recipe.as_ref()?;
                let inputs = recipe
                    .from
                    .iter()
                    .chain(&recipe.cost)
                    .map(|(used, _)| used.as_str())
                    .collect::<Vec<_>>();
                Some((item.name.as_str(), inputs))
            })
            .collect::<BTreeMap<_, _>>();
        for item in &tables.items {
            if reaches_itself(&item.name, &made_from) {
                self.issue(
                    StandardPackProblem::TableRecipeCycle,
                    at(&at("/items", &item.name), "recipe"),
                );
            }
        }
        for (index, table) in tables.tables.iter().enumerate() {
            let pointer = format!("/tables/{index}");
            self.prerequisites(table, &pointer);
            self.roles(table, &pointer);
            self.positions_held(table, &pointer);
        }
    }

    /// A table's prerequisites: each names a column of the table (`table_column_unknown`) whose
    /// ladders are one per key or class (`table_requires_invalid` for `per: "sub"`), at its
    /// `column`; then each column that reaches itself through them (`table_requires_cycle`).
    fn prerequisites(&mut self, table: &GoalTable, pointer: &str) {
        let at_columns = at(pointer, "columns");
        for (index, column) in table.columns.iter().enumerate() {
            for (position, requirement) in column.requires.iter().enumerate() {
                let GoalRequirement::Column {
                    column: required, ..
                } = requirement
                else {
                    continue;
                };
                let at_column = format!("{at_columns}/{index}/requires/{position}/column");
                match table.column(required) {
                    None => self.issue(StandardPackProblem::TableColumnUnknown, at_column),
                    Some(found) if found.read.per() == GoalColumnPer::Sub => {
                        self.issue(StandardPackProblem::TableRequiresInvalid, at_column);
                    }
                    Some(_) => {}
                }
            }
        }
        let required = table
            .columns
            .iter()
            .map(|column| {
                let names = column
                    .requires
                    .iter()
                    .filter_map(|requirement| match requirement {
                        GoalRequirement::Column { column, .. } => Some(column.as_str()),
                        GoalRequirement::Condition(_) => None,
                    })
                    .collect::<Vec<_>>();
                (column.name.as_str(), names)
            })
            .collect::<BTreeMap<_, _>>();
        for (index, column) in table.columns.iter().enumerate() {
            if reaches_itself(&column.name, &required) {
                self.issue(
                    StandardPackProblem::TableRequiresCycle,
                    format!("{at_columns}/{index}/requires"),
                );
            }
        }
    }

    /// A table's roles: every step a key takes (its class's, as the key overrides them, and its
    /// own) names roles the key declares in its `own` (`table_role_unknown` at the step's member,
    /// each pointer once). Fix round 1: a step's pointer is built only when the step names a role
    /// its key does not declare, never for a step that is in order.
    fn roles(&mut self, table: &GoalTable, pointer: &str) {
        let mut reported = BTreeSet::new();
        for key in &table.keys {
            let roles = key
                .own
                .iter()
                .map(|(role, _)| role.as_str())
                .collect::<BTreeSet<_>>();
            if let Some(class) = table.class(&key.class) {
                for (column, ladder) in &class.ladders {
                    let overrides = key
                        .columns
                        .iter()
                        .find_map(|(name, written)| match written {
                            GoalKeyColumn::Steps(steps) if name == column => Some(steps),
                            _ => None,
                        });
                    for (index, step) in ladder.steps.iter().enumerate() {
                        let overridden = overrides.and_then(|steps| {
                            steps.iter().find(|(position, _)| *position == index)
                        });
                        // The step is the key's override or its class's, at the one's member.
                        let (taken, owner, name) = match overridden {
                            Some((_, written)) => (written, "keys", key.name.as_str()),
                            None => (step, "classes", class.name.as_str()),
                        };
                        self.step_roles(taken, &roles, &mut reported, || {
                            format!(
                                "{}/steps/{index}",
                                at(&at(&at(pointer, owner), name), column)
                            )
                        });
                    }
                }
            }
            for (column, written) in &key.columns {
                let at_column = || at(&at(&at(pointer, "keys"), &key.name), column);
                match written {
                    GoalKeyColumn::Steps(_) => {}
                    GoalKeyColumn::Ladder(ladder) => {
                        self.ladder_roles(ladder, &roles, &mut reported, at_column);
                    }
                    GoalKeyColumn::Subs(subs) => {
                        for (sub, ladder) in subs {
                            self.ladder_roles(ladder, &roles, &mut reported, || {
                                at(&at_column(), sub)
                            });
                        }
                    }
                }
            }
        }
    }

    /// The roles of every step of `ladder`, whose pointer `pointer` builds
    /// ([`Self::step_roles`]).
    fn ladder_roles(
        &mut self,
        ladder: &GoalLadder,
        roles: &BTreeSet<&str>,
        reported: &mut BTreeSet<String>,
        pointer: impl Fn() -> String,
    ) {
        for (index, step) in ladder.steps.iter().enumerate() {
            self.step_roles(step, roles, reported, || {
                format!("{}/steps/{index}", pointer())
            });
        }
    }

    /// Each role `step` names that is none of `roles`, once per pointer: `pointer` builds the
    /// step's pointer, called only for such a role.
    fn step_roles(
        &mut self,
        step: &GoalStep,
        roles: &BTreeSet<&str>,
        reported: &mut BTreeSet<String>,
        pointer: impl Fn() -> String,
    ) {
        for (used, _) in step {
            if let GoalUse::Role(role) = used
                && !roles.contains(role.as_str())
            {
                let at_use = at(&pointer(), &format!("{ROLE_PREFIX}{role}"));
                if reported.insert(at_use.clone()) {
                    self.issue(StandardPackProblem::TableRoleUnknown, at_use);
                }
            }
        }
    }

    /// A table's prerequisites' positions, held only for the classes and keys that have steps in
    /// the requiring column (a class without a ladder of it is not held to them): the required
    /// column's ladder of each holds `at_least`, and each of their ladders of the requiring column
    /// holds `from`; otherwise `table_requires_invalid` at that member, once. A prerequisite whose
    /// column is unknown or of `per: "sub"` was refused at its `column` already.
    fn positions_held(&mut self, table: &GoalTable, pointer: &str) {
        for (index, column) in table.columns.iter().enumerate() {
            for (position, requirement) in column.requires.iter().enumerate() {
                let GoalRequirement::Column {
                    column: required,
                    at_least,
                    from,
                } = requirement
                else {
                    continue;
                };
                let Some(required) = table.column(required) else {
                    continue;
                };
                if required.read.per() == GoalColumnPer::Sub {
                    continue;
                }
                let mut held = true;
                let mut starts = true;
                for class in &table.classes {
                    let Some(own) = class.ladder(&column.name) else {
                        continue;
                    };
                    if required.read.per() == GoalColumnPer::Class
                        && !class
                            .ladder(&required.name)
                            .is_some_and(|ladder| ladder.holds(at_least))
                    {
                        held = false;
                    }
                    if from.as_ref().is_some_and(|start| !own.holds(start)) {
                        starts = false;
                    }
                }
                for key in &table.keys {
                    let ladders = table.ladders_of(key, &column.name);
                    if ladders.is_empty() {
                        continue;
                    }
                    if !table
                        .ladders_of(key, &required.name)
                        .first()
                        .is_some_and(|ladder| ladder.holds(at_least))
                    {
                        held = false;
                    }
                    if from
                        .as_ref()
                        .is_some_and(|start| ladders.iter().any(|ladder| !ladder.holds(start)))
                    {
                        starts = false;
                    }
                }
                let at_requirement = format!("{pointer}/columns/{index}/requires/{position}");
                if !held {
                    self.issue(
                        StandardPackProblem::TableRequiresInvalid,
                        at(&at_requirement, "at_least"),
                    );
                }
                if !starts {
                    self.issue(
                        StandardPackProblem::TableRequiresInvalid,
                        at(&at_requirement, "from"),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "standard_pack_goal_tables_tests.rs"]
mod tests;
