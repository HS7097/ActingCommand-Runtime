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

use crate::standard_pack::{StandardPackProblem, StandardPackRootIssue};
use crate::standard_pack_catalog::{at, read_document};
use crate::standard_pack_goals::{
    Check, GoalReferences, MAX_GOAL_NAME_BYTES, MAX_GOAL_REFERENCE_BYTES, goal_name,
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

/// The prefix of a step's member that names a role of the key, not an item: `@<role>`.
const ROLE_PREFIX: &str = "@";
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

/// One prerequisite of a column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalRequirement {
    /// The column `column` is at `at_least` or beyond; asked from the requiring column's position
    /// `from` on, or for the whole column without it.
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
/// `table_reading_column_duplicate`.
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
            &[],
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
        Some(StandardPackGoalTables {
            game: game.to_owned(),
            items: items?,
            tables: tables?,
        })
    }

    /// `items`: 1 to [`MAX_GOAL_TABLE_ITEMS`] items by name.
    fn items(&mut self, value: &Value) -> Option<Vec<GoalTableItem>> {
        let map = self.map(value, "/items", (1, MAX_GOAL_TABLE_ITEMS))?;
        let mut items = Vec::new();
        let mut complete = true;
        for (name, item) in map {
            match self.item(name, item, &at("/items", name)) {
                Some(item) => items.push(item),
                None => complete = false,
            }
        }
        complete.then_some(items)
    }

    /// One item `{pool?, recipe?}`, its name of [`item_word`].
    fn item(&mut self, name: &str, value: &Value, pointer: &str) -> Option<GoalTableItem> {
        let named = self.named(name, pointer, MAX_GOAL_TABLE_ID_BYTES, item_word);
        let object = self
            .check
            .object(value, pointer, &[], &["pool", "recipe"])?;
        let pool = match object.get("pool") {
            None => Some(None),
            Some(pool) => self
                .name_value(pool, &at(pointer, "pool"), MAX_GOAL_REFERENCE_BYTES, opaque)
                .map(Some),
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
    /// their reading columns each listed once among them.
    fn progress(&mut self, value: &Value, pointer: &str) -> Option<Vec<GoalProgressList>> {
        let list = self
            .check
            .list(value, pointer, (1, MAX_GOAL_TABLE_PROGRESS_LISTS))?;
        let mut seen = BTreeSet::new();
        let mut held = false;
        let mut lists = Vec::new();
        let mut complete = true;
        for (index, item) in list.iter().enumerate() {
            let at_list = format!("{pointer}/{index}");
            match self.progress_list(item, &at_list, &mut seen, &mut held) {
                Some(read) => lists.push(read),
                None => complete = false,
            }
        }
        complete.then_some(lists)
    }

    /// One list `{fact_key, key_column, held_by?, columns}`: a list reading's key
    /// (`table_list_key_invalid` otherwise), its key column and 1 to 63 columns of a list
    /// reading's column form, `held_by` a boolean (a second `true` of the table `malformed`).
    fn progress_list(
        &mut self,
        value: &Value,
        pointer: &str,
        seen: &mut BTreeSet<String>,
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
            .and_then(|key| self.fact_key(key, &at(pointer, "fact_key")));
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
            )
        });
        Some(GoalProgressList {
            fact_key: fact_key?,
            key_column: key_column?,
            held_by: held_by?,
            columns: columns?,
        })
    }

    /// A list reading's `fact_key` (`table_list_key_invalid` when it is no such key).
    fn fact_key(&mut self, value: &Value, pointer: &str) -> Option<String> {
        let Some(key) = value.as_str() else {
            self.check.malformed(pointer);
            return None;
        };
        if list_reading_fact_key_valid(key) {
            Some(key.to_owned())
        } else {
            self.issue(StandardPackProblem::TableListKeyInvalid, pointer);
            None
        }
    }

    /// A list of `min..=max` reading column names, each new to `seen`
    /// (`table_reading_column_duplicate` at the later) and, with `known`, one of them
    /// (`table_column_unknown`).
    fn reading_columns(
        &mut self,
        value: &Value,
        pointer: &str,
        bounds: (usize, usize),
        seen: &mut BTreeSet<String>,
        known: Option<&BTreeSet<String>>,
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
            } else if !seen.insert(column.clone()) {
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
    /// each pointer once).
    fn roles(&mut self, table: &GoalTable, pointer: &str) {
        let mut reported = BTreeSet::new();
        for key in &table.keys {
            let roles = key
                .own
                .iter()
                .map(|(role, _)| role.as_str())
                .collect::<BTreeSet<_>>();
            let at_key = at(&at(pointer, "keys"), &key.name);
            if let Some(class) = table.class(&key.class) {
                let at_class = at(&at(pointer, "classes"), &class.name);
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
                        let (taken, at_step) = match overridden {
                            Some((_, written)) => {
                                (written, format!("{}/steps/{index}", at(&at_key, column)))
                            }
                            None => (step, format!("{}/steps/{index}", at(&at_class, column))),
                        };
                        self.step_roles(taken, &at_step, &roles, &mut reported);
                    }
                }
            }
            for (column, written) in &key.columns {
                let at_column = at(&at_key, column);
                match written {
                    GoalKeyColumn::Steps(_) => {}
                    GoalKeyColumn::Ladder(ladder) => {
                        self.ladder_roles(ladder, &at_column, &roles, &mut reported);
                    }
                    GoalKeyColumn::Subs(subs) => {
                        for (sub, ladder) in subs {
                            self.ladder_roles(ladder, &at(&at_column, sub), &roles, &mut reported);
                        }
                    }
                }
            }
        }
    }

    /// The roles of every step of `ladder` at `pointer` ([`Self::step_roles`]).
    fn ladder_roles(
        &mut self,
        ladder: &GoalLadder,
        pointer: &str,
        roles: &BTreeSet<&str>,
        reported: &mut BTreeSet<String>,
    ) {
        for (index, step) in ladder.steps.iter().enumerate() {
            self.step_roles(step, &format!("{pointer}/steps/{index}"), roles, reported);
        }
    }

    /// Each role `step` at `pointer` names that is none of `roles`, once per pointer.
    fn step_roles(
        &mut self,
        step: &GoalStep,
        pointer: &str,
        roles: &BTreeSet<&str>,
        reported: &mut BTreeSet<String>,
    ) {
        for (used, _) in step {
            if let GoalUse::Role(role) = used
                && !roles.contains(role.as_str())
            {
                let at_use = at(pointer, &format!("{ROLE_PREFIX}{role}"));
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
