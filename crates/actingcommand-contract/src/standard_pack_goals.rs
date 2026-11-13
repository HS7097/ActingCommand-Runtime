// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #455 G-R1 (register §1.350; `contracts/standard-pack-goals.md`): a game root's optional
//! goal declaration, `goals.json` (`actingcommand.goals.v1`), beside `bundle.json`: the goal entries
//! the game offers on one page, in file order, each with its stable name, its type, its block, its
//! texts and what it governs (a pool of `schedule.json`, rules of `schedule.json` or packages of
//! `bundle.json`).
//!
//! Two pure steps. [`parse_standard_pack_goals`] reads the file strictly into typed entries: its
//! shape and limits, the names, the eight types of version 1 and their members, the blocks, the
//! ranges, the defaults and the rates. [`check_standard_pack_goal_references`] then holds the typed
//! entries to what the game root offers ([`GoalReferences`]: the composed schedule's pools and
//! rules, each rule with the package it runs, the bundle's packages, each with what its control
//! declaration provides, and the root's tables). Every violation is one [`StandardPackRootIssue`]
//! of [`STANDARD_PACK_GOALS_FILE`] at its JSON pointer, in document order (a member repeated in one
//! object first), at most [`MAX_STANDARD_PACK_GOALS_ISSUES`]; the references are judged only for a
//! file the reader accepts.
//!
//! G-R1 fix round 1 (the independent review): a package can do something irreversible when it
//! provides a capability that any package of the game root marks irreversible
//! (contracts/remedies.md §2.3), whether it marks it itself or not; a `tier` entry is held to the
//! package each rule it governs runs; and [`GoalReferences`] is made only by
//! [`GoalReferences::new`], from what every package of the root provides, so no reader can leave
//! that answer empty by omission.
//!
//! The file stores no value: a value is the installation's, set by a person or an agent in a later
//! slice. The Runtime interprets no text here: titles, help texts, unit texts, option titles,
//! names of table keys and locales are opaque, measured in bytes only, and ids and option values
//! are compared for equality only. No Runtime reader opens the file in this slice (its root reader,
//! its startup and run preparation, the policy crate and check-config); `actinglab standard-pack
//! check` reports a refused file without failing the root, and `package bundle --goals` refuses
//! one.

use crate::standard_pack::{StandardPackProblem, StandardPackRootIssue};
use crate::standard_pack_table::standard_pack_table_id;
use crate::standard_pack_ui::{
    MAX_PACK_UI_LOCALE_BYTES, MAX_PACK_UI_LOCALES, MAX_PACK_UI_SUMMARY_BYTES,
    MAX_PACK_UI_TITLE_BYTES, at, read_document,
};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The game root's goal declaration (G-R1).
pub const STANDARD_PACK_GOALS_FILE: &str = "goals.json";
/// The one version of `goals.json` this build reads.
pub const STANDARD_PACK_GOALS_SCHEMA_VERSION: &str = "actingcommand.goals.v1";
/// The `package` interface revision a bundle that carries `goals.json` declares, `[12, 12]`
/// (`contracts/component-interfaces.md`, the `package` row 12).
pub const GOALS_PACKAGE_REVISION: u64 = 12;
/// The largest `goals.json`, in bytes (1 MiB).
pub const MAX_STANDARD_PACK_GOALS_BYTES: usize = 1_048_576;
/// The most violations one refused `goals.json` reports; the first ones in document order.
pub const MAX_STANDARD_PACK_GOALS_ISSUES: usize = 64;
/// The most entries of `entries`: the entry rows the application's goal record keeps beside its
/// targets and task rows within the scheduler's 256 rows.
pub const MAX_GOAL_ENTRIES: usize = 96;
/// The longest entry name, in bytes.
pub const MAX_GOAL_NAME_BYTES: usize = 64;
/// The longest id an entry names (a pool, a rule, a package), and the longest key of a table, in
/// bytes.
pub const MAX_GOAL_REFERENCE_BYTES: usize = 128;
/// The most packages or rules one `governs` names.
pub const MAX_GOAL_GOVERNED: usize = 32;
/// The fewest options of a `choice` or `tier` entry.
pub const MIN_GOAL_CHOICE_OPTIONS: usize = 2;
/// The most options of a `choice` or `tier` entry.
pub const MAX_GOAL_CHOICE_OPTIONS: usize = 32;
/// The most values of an `ordered` entry: its options, or the keys of its table, and the names
/// under its `from_table`.
pub const MAX_GOAL_LIST_VALUES: usize = 1024;
/// The largest `max_count` of an `ordered` entry.
pub const MAX_GOAL_ORDERED_COUNT: u64 = 32;
/// The longest option value, in bytes.
pub const MAX_GOAL_VALUE_BYTES: usize = 64;
/// The largest `scale_milli` of a `tier` option (the smallest is 1).
pub const MAX_GOAL_SCALE_MILLI: i64 = 1_000_000;
/// The largest integer of the file, in magnitude: the safe integers of JSON, 2^53 - 1.
pub const MAX_GOAL_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// The type of an entry (`type`), one of the eight of version 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GoalType {
    /// A group title between entries; no value, governs nothing.
    Heading,
    /// The author's text; no value, governs nothing.
    Note,
    /// A goal on one pool: an amount to hold, later with a deadline.
    Resource,
    /// A whole number within a range, on a step.
    Integer,
    /// One of 2 to 32 options.
    Choice,
    /// On or off.
    Switch,
    /// An ordered list of values, from options or from a table's keys.
    Ordered,
    /// One of 2 to 32 tiers, each scaling the fixed value of the rules it governs.
    Tier,
}

impl GoalType {
    /// The eight types of version 1, in the contract's order.
    pub const ALL: [Self; 8] = [
        Self::Heading,
        Self::Note,
        Self::Resource,
        Self::Integer,
        Self::Choice,
        Self::Switch,
        Self::Ordered,
        Self::Tier,
    ];

    /// The type's word in the file.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Heading => "heading",
            Self::Note => "note",
            Self::Resource => "resource",
            Self::Integer => "integer",
            Self::Choice => "choice",
            Self::Switch => "switch",
            Self::Ordered => "ordered",
            Self::Tier => "tier",
        }
    }

    /// The type a word names; `None` for any other word (a later version's type too).
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|goal_type| goal_type.as_str() == word)
    }

    /// The block every reader shows the type with: the only block version 1 gives it.
    pub const fn base_block(self) -> GoalBlock {
        match self {
            Self::Heading => GoalBlock::Group,
            Self::Note => GoalBlock::Text,
            Self::Resource => GoalBlock::GoalRow,
            Self::Integer => GoalBlock::Number,
            Self::Choice | Self::Tier => GoalBlock::Choice,
            Self::Switch => GoalBlock::Switch,
            Self::Ordered => GoalBlock::OrderedList,
        }
    }

    /// The members an entry of the type must hold.
    const fn required(self) -> &'static [&'static str] {
        match self {
            Self::Heading | Self::Note => HEADING_REQUIRED,
            Self::Resource => RESOURCE_REQUIRED,
            Self::Integer => INTEGER_REQUIRED,
            Self::Choice | Self::Tier => CHOICE_REQUIRED,
            Self::Switch => SWITCH_REQUIRED,
            Self::Ordered => ORDERED_REQUIRED,
        }
    }

    /// The members an entry of the type may hold besides the required ones.
    const fn optional(self) -> &'static [&'static str] {
        match self {
            Self::Heading | Self::Note | Self::Choice | Self::Switch | Self::Tier => {
                COMMON_OPTIONAL
            }
            Self::Resource => RESOURCE_OPTIONAL,
            Self::Integer => INTEGER_OPTIONAL,
            Self::Ordered => ORDERED_OPTIONAL,
        }
    }
}

const HEADING_REQUIRED: &[&str] = &["name", "type", "title"];
const RESOURCE_REQUIRED: &[&str] = &["name", "type", "title", "governs"];
const INTEGER_REQUIRED: &[&str] = &["name", "type", "title", "governs", "min", "max", "default"];
const CHOICE_REQUIRED: &[&str] = &["name", "type", "title", "governs", "options", "default"];
const SWITCH_REQUIRED: &[&str] = &["name", "type", "title", "governs", "default"];
const ORDERED_REQUIRED: &[&str] = &[
    "name",
    "type",
    "title",
    "governs",
    "min_count",
    "max_count",
    "default",
];
const COMMON_OPTIONAL: &[&str] = &["block", "help"];
const RESOURCE_OPTIONAL: &[&str] = &["block", "help", "rate"];
const INTEGER_OPTIONAL: &[&str] = &["block", "help", "step", "unit"];
const ORDERED_OPTIONAL: &[&str] = &["block", "help", "options", "from_table"];

/// A block (`block`): how a reader shows an entry. Version 1 gives each type one block, its base
/// block; a reader that meets a block it does not know shows the type's base block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GoalBlock {
    /// A heading between entries (`heading`).
    Group,
    /// Text to read, not to change (`note`).
    Text,
    /// An amount to hold on a pool, with its current reading (`resource`).
    GoalRow,
    /// A number typed in, with its unit and its limits (`integer`).
    Number,
    /// One of a few options, side by side up to four, in a drop-down above (`choice`, `tier`).
    Choice,
    /// On or off (`switch`).
    Switch,
    /// Values in an order a person sets, with search when the list is long (`ordered`).
    OrderedList,
}

impl GoalBlock {
    /// The block's word in the file.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Group => "group",
            Self::Text => "text",
            Self::GoalRow => "goal_row",
            Self::Number => "number",
            Self::Choice => "choice",
            Self::Switch => "switch",
            Self::OrderedList => "ordered_list",
        }
    }
}

/// A text per locale, in the file's order: opaque to the Runtime.
pub type GoalText = Vec<(String, String)>;

/// A `goals.json` that keeps every rule of the reader: its entries in file order, which is their
/// position on the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardPackGoals {
    /// The game directory the file belongs to (its `game`).
    pub game: String,
    pub entries: Vec<DeclaredGoal>,
}

impl StandardPackGoals {
    /// The entry named `name`.
    pub fn entry(&self, name: &str) -> Option<&DeclaredGoal> {
        self.entries.iter().find(|entry| entry.name == name)
    }

    /// Every package an entry governs.
    pub fn governed_packages(&self) -> BTreeSet<&str> {
        self.entries
            .iter()
            .flat_map(|entry| entry.kind.packages())
            .map(String::as_str)
            .collect()
    }

    /// Every table an `ordered` entry takes its values from.
    pub fn tables(&self) -> BTreeSet<&str> {
        self.entries
            .iter()
            .filter_map(|entry| match &entry.kind {
                DeclaredGoalKind::Ordered {
                    values: GoalValues::Table { table, .. },
                    ..
                } => Some(table.as_str()),
                _ => None,
            })
            .collect()
    }
}

/// One entry of the page: its stable name (the Lab's name and, later, its fact name), the block it
/// is shown with, its texts and its type with the type's members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredGoal {
    pub name: String,
    /// The block written, or the type's base block when the entry writes none.
    pub block: GoalBlock,
    pub title: GoalText,
    pub help: Option<GoalText>,
    pub kind: DeclaredGoalKind,
}

impl DeclaredGoal {
    pub const fn goal_type(&self) -> GoalType {
        self.kind.goal_type()
    }
}

/// An entry's type with its members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclaredGoalKind {
    Heading,
    Note,
    /// `governs.pool`, and the optional `rate` a deadline reads (a later slice). Its default is
    /// "no goal": the entry writes none.
    Resource {
        pool: String,
        rate: Option<GoalRate>,
    },
    /// `governs.packages`, `min`, `max`, `step` (1 when the entry writes none), `unit` and
    /// `default`.
    Integer {
        packages: Vec<String>,
        min: i64,
        max: i64,
        step: i64,
        unit: Option<GoalUnit>,
        default: i64,
    },
    Choice {
        packages: Vec<String>,
        options: Vec<GoalOption>,
        default: String,
    },
    Switch {
        packages: Vec<String>,
        default: bool,
    },
    Ordered {
        packages: Vec<String>,
        values: GoalValues,
        min_count: u64,
        max_count: u64,
        default: Vec<String>,
    },
    /// `governs.rules` and its tiers (a later slice compiles them).
    Tier {
        rules: Vec<String>,
        options: Vec<GoalTierOption>,
        default: String,
    },
}

impl DeclaredGoalKind {
    pub const fn goal_type(&self) -> GoalType {
        match self {
            Self::Heading => GoalType::Heading,
            Self::Note => GoalType::Note,
            Self::Resource { .. } => GoalType::Resource,
            Self::Integer { .. } => GoalType::Integer,
            Self::Choice { .. } => GoalType::Choice,
            Self::Switch { .. } => GoalType::Switch,
            Self::Ordered { .. } => GoalType::Ordered,
            Self::Tier { .. } => GoalType::Tier,
        }
    }

    /// The packages the entry governs; none for a type that governs a pool, rules or nothing.
    pub fn packages(&self) -> &[String] {
        match self {
            Self::Integer { packages, .. }
            | Self::Choice { packages, .. }
            | Self::Switch { packages, .. }
            | Self::Ordered { packages, .. } => packages.as_slice(),
            Self::Heading | Self::Note | Self::Resource { .. } | Self::Tier { .. } => &[],
        }
    }
}

/// A `resource` entry's `rate`: the pool's net income per day, times 1000, signed, at its low and
/// its high end, written by the resource side; read by a deadline (a later slice).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoalRate {
    pub floor_milli_per_day: i64,
    pub ceiling_milli_per_day: i64,
}

/// An `integer` entry's `unit`: a time word, whose values are milliseconds a reader shows in that
/// unit, or a text per locale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalUnit {
    Time(GoalTimeUnit),
    Text(GoalText),
}

/// The time words of `unit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GoalTimeUnit {
    Second,
    Minute,
    Hour,
    Day,
}

impl GoalTimeUnit {
    pub const ALL: [Self; 4] = [Self::Second, Self::Minute, Self::Hour, Self::Day];

    /// The unit's word in the file.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Second => "second",
            Self::Minute => "minute",
            Self::Hour => "hour",
            Self::Day => "day",
        }
    }

    fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|unit| unit.as_str() == word)
    }
}

/// One option of a `choice` or `ordered` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalOption {
    pub value: String,
    pub title: GoalText,
}

/// One tier of a `tier` entry: its value, its title and the scale, times 1000, it gives the fixed
/// value of the rules the entry governs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalTierOption {
    pub value: String,
    pub title: GoalText,
    pub scale_milli: i64,
}

/// Where an `ordered` entry's values come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalValues {
    /// `options`.
    Options(Vec<GoalOption>),
    /// `from_table`: the keys of a table of the game root, with the names a reader shows for some
    /// of them, per locale.
    Table {
        table: String,
        names: Vec<(String, GoalText)>,
    },
}

/// A rule of `schedule.json` as the references hold it (G-R1 fix round 1): the package it runs,
/// which a `tier` entry that governs the rule is held to, and whether it writes
/// `fixed_value_milli`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalRule {
    /// The rule's `package_id`.
    pub package_id: String,
    /// The rule writes `fixed_value_milli`.
    pub fixed_value: bool,
}

/// What the references of a declaration are held to (`check_standard_pack_goal_references`): what
/// the game root offers. G-R1 fix round 1: made only by [`GoalReferences::new`], from what every
/// package of the root provides; there is no default, so a reader cannot leave the answer to
/// which packages can do something irreversible empty by omission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalReferences {
    /// The pools of the composed schedule by id, each `true` when its `valuation` holds both
    /// `scale` and `gap`; none without `schedule.json`.
    pub pools: BTreeMap<String, bool>,
    /// The rules of `schedule.json` by id; none without `schedule.json`.
    pub rules: BTreeMap<String, GoalRule>,
    /// The tables of the game root by id, each with its distinct keys in row order. `None` when
    /// the reader holds no tables (`package bundle`): then no table reference is judged.
    pub tables: Option<BTreeMap<String, Vec<String>>>,
    /// The packages of the game root's `bundle.json` by id, each `true` when it can do something
    /// irreversible ([`GoalReferences::new`]).
    packages: BTreeMap<String, bool>,
}

impl GoalReferences {
    /// The references of a game root whose `bundle.json` lists exactly the packages of
    /// `provides`, each with the capabilities its control declaration provides (`provides[]`),
    /// every one `true` when its entry writes `irreversible: true`. A capability is irreversible
    /// when any package of the root marks it so (contracts/remedies.md §2.3, "any provider"), and
    /// a package can do something irreversible when it provides such a capability, marked by
    /// itself or not. A package `provides` does not name is no package of the root: an entry that
    /// governs it, or a rule that runs it, is never passed. The pools and the rules start empty,
    /// the tables `None`.
    pub fn new(provides: BTreeMap<String, BTreeMap<String, bool>>) -> Self {
        let irreversible = provides
            .values()
            .flatten()
            .filter(|(_, marked)| **marked)
            .map(|(capability, _)| capability.clone())
            .collect::<BTreeSet<_>>();
        let packages = provides
            .into_iter()
            .map(|(package_id, capabilities)| {
                let can = capabilities
                    .keys()
                    .any(|capability| irreversible.contains(capability));
                (package_id, can)
            })
            .collect();
        Self {
            pools: BTreeMap::new(),
            rules: BTreeMap::new(),
            tables: None,
            packages,
        }
    }

    /// Whether the package `package_id` can do something irreversible; `None` when it is no
    /// package of the root.
    fn irreversible(&self, package_id: &str) -> Option<bool> {
        self.packages.get(package_id).copied()
    }
}

/// `goals.json`'s bytes, read for the game directory `game` (contract "The checks", the reader's
/// rows). Every violation is one issue at its JSON pointer, at most
/// [`MAX_STANDARD_PACK_GOALS_ISSUES`]: every member repeated in one object first, then the others
/// in document order:
///
/// - `goal_out_of_limits` (fix round 1): the file is larger than [`MAX_STANDARD_PACK_GOALS_BYTES`]
///   (the whole document); a list, a map, a text's locales, or the bytes of a name, an option
///   value, an id, a key, a locale or a text outside its limit (at what exceeds);
/// - `malformed`: no JSON or no object (the whole document; a byte-order mark too); a member
///   repeated in one object, unknown, missing or not of its type (an integer outside the safe
///   integers too, but for an `integer` entry's `default`), a `schema_version` other than
///   [`STANDARD_PACK_GOALS_SCHEMA_VERSION`], a name, an option value, an id, a key or a word
///   outside its form, an `ordered` entry with both `options` and `from_table` (at `from_table`)
///   or with neither (at `options`);
/// - `game_mismatch`: `game` is not the directory's;
/// - `goal_name_duplicate`: a name an earlier entry has (at the later `name`);
/// - `goal_type_unknown`: a `type` that is none of the eight of version 1;
/// - `goal_block_mismatch`: a `block` other than the type's base block;
/// - `goal_range_invalid`: what an entry declares as its values cannot hold one (an integer's `min`
///   above its `max`, a `step` below 1, an option value written twice, an `ordered` entry's
///   `max_count` of 0 or above [`MAX_GOAL_ORDERED_COUNT`], its `min_count` above its `max_count`
///   or above the number of its options, a tier's `scale_milli` outside 1 to
///   [`MAX_GOAL_SCALE_MILLI`]);
/// - `goal_default_invalid`: a `default` that is not a value of the declaration (an `integer`
///   entry's that is no whole number in its range on its step, whatever its type or size);
/// - `goal_rate_invalid`: a `rate` whose ceiling is below its floor.
pub fn parse_standard_pack_goals(
    bytes: &[u8],
    game: &str,
) -> Result<StandardPackGoals, Vec<StandardPackRootIssue>> {
    let mut check = Check::default();
    if bytes.len() > MAX_STANDARD_PACK_GOALS_BYTES {
        check.out_of_limits("");
        return Err(check.issues);
    }
    let mut repeated = Vec::new();
    let Some(document) = read_document(bytes, &mut repeated) else {
        check.malformed("");
        return Err(check.issues);
    };
    // The reader meets a repeated member while it reads the document, before any member is
    // judged: these come first (contract "The checks").
    for pointer in repeated {
        check.malformed(pointer);
    }
    let goals = check.document(&document, game);
    match goals {
        Some(goals) if check.issues.is_empty() => Ok(goals),
        // Every path that gives no declaration records why; an empty list would hide a refusal.
        _ if check.issues.is_empty() => {
            check.malformed("");
            Err(check.issues)
        }
        _ => Err(check.issues),
    }
}

/// The references of an accepted declaration, held to what the game root offers (contract "The
/// checks", the references' rows), entry by entry in file order:
///
/// - `goal_pool_unknown`: a `resource` entry's pool is not a pool of the composed schedule;
/// - `goal_pool_unvalued`: its pool's `valuation` lacks `scale` or `gap`;
/// - `goal_package_unknown`: a governed package, or (fix round 1) the package a governed rule
///   runs, is not a package of `bundle.json`;
/// - `goal_governs_irreversible`: a governed package, or (fix round 1) the package a governed rule
///   runs, can do something irreversible ([`GoalReferences::new`]);
/// - `goal_rule_unknown`: a `tier` entry's rule is not a rule of `schedule.json`;
/// - `goal_tier_without_fixed_value`: that rule, its package passed, writes no
///   `fixed_value_milli`;
/// - with the root's tables: `goal_table_unknown` (an `ordered` entry's table is no table of the
///   root, or a key under its `names` no key of that table), `goal_range_invalid` (the table holds
///   more than [`MAX_GOAL_LIST_VALUES`] keys, or fewer than the entry's `min_count`) and
///   `goal_default_invalid` (a value of its `default` is no key of the table).
pub fn check_standard_pack_goal_references(
    goals: &StandardPackGoals,
    references: &GoalReferences,
) -> Vec<StandardPackRootIssue> {
    let mut check = Check::default();
    for (index, goal) in goals.entries.iter().enumerate() {
        let pointer = format!("/entries/{index}");
        let governs = at(&pointer, "governs");
        match &goal.kind {
            DeclaredGoalKind::Heading | DeclaredGoalKind::Note => {}
            DeclaredGoalKind::Resource { pool, .. } => {
                let at_pool = at(&governs, "pool");
                match references.pools.get(pool) {
                    None => check.issue(StandardPackProblem::GoalPoolUnknown, at_pool),
                    Some(false) => check.issue(StandardPackProblem::GoalPoolUnvalued, at_pool),
                    Some(true) => {}
                }
            }
            DeclaredGoalKind::Tier { rules, .. } => {
                let at_rules = at(&governs, "rules");
                for (position, rule_id) in rules.iter().enumerate() {
                    let at_rule = format!("{at_rules}/{position}");
                    let Some(rule) = references.rules.get(rule_id) else {
                        check.issue(StandardPackProblem::GoalRuleUnknown, at_rule);
                        continue;
                    };
                    // Fix round 1: a rule names the package it runs, and a tier that governs the
                    // rule scales that package's weight; the rule is held to its package as a
                    // governed package is.
                    match references.irreversible(&rule.package_id) {
                        None => check.issue(StandardPackProblem::GoalPackageUnknown, at_rule),
                        Some(true) => {
                            check.issue(StandardPackProblem::GoalGovernsIrreversible, at_rule);
                        }
                        Some(false) if !rule.fixed_value => {
                            check.issue(StandardPackProblem::GoalTierWithoutFixedValue, at_rule);
                        }
                        Some(false) => {}
                    }
                }
            }
            DeclaredGoalKind::Integer { packages, .. }
            | DeclaredGoalKind::Choice { packages, .. }
            | DeclaredGoalKind::Switch { packages, .. }
            | DeclaredGoalKind::Ordered { packages, .. } => {
                let at_packages = at(&governs, "packages");
                for (position, package_id) in packages.iter().enumerate() {
                    let at_package = format!("{at_packages}/{position}");
                    match references.irreversible(package_id) {
                        None => check.issue(StandardPackProblem::GoalPackageUnknown, at_package),
                        Some(true) => {
                            check.issue(StandardPackProblem::GoalGovernsIrreversible, at_package);
                        }
                        Some(false) => {}
                    }
                }
                if let (
                    DeclaredGoalKind::Ordered {
                        values: GoalValues::Table { table, names },
                        min_count,
                        default,
                        ..
                    },
                    Some(tables),
                ) = (&goal.kind, &references.tables)
                {
                    check.table_values(&pointer, table, names, *min_count, default, tables);
                }
            }
        }
    }
    check.issues
}

/// An entry's name: 1 to [`MAX_GOAL_NAME_BYTES`] bytes of `[a-z0-9][a-z0-9_]*`.
pub fn goal_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= MAX_GOAL_NAME_BYTES
        && bytes
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// An option value: 1 to [`MAX_GOAL_VALUE_BYTES`] bytes of `[a-z0-9][a-z0-9_.:-]*`.
fn option_value(value: &str) -> bool {
    let mut bytes = value.bytes();
    value.len() <= MAX_GOAL_VALUE_BYTES
        && bytes
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_.:-".contains(&byte)
        })
}

/// An id an entry names, or a key of a table: 1 to [`MAX_GOAL_REFERENCE_BYTES`] bytes without a
/// control character.
fn opaque_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_GOAL_REFERENCE_BYTES && !id.chars().any(char::is_control)
}

/// Fix round 1: what is wrong with `text`, a name, an option value, an id or a key of the form
/// `form`: `goal_out_of_limits` when it is empty or longer than `max` bytes, else `malformed`
/// when it breaks its form; `None` when it keeps both.
fn bounded(text: &str, max: usize, form: fn(&str) -> bool) -> Option<StandardPackProblem> {
    if text.is_empty() || text.len() > max {
        Some(StandardPackProblem::GoalOutOfLimits)
    } else if form(text) {
        None
    } else {
        Some(StandardPackProblem::Malformed)
    }
}

/// The checks of one document: the issues found so far.
#[derive(Default)]
struct Check {
    issues: Vec<StandardPackRootIssue>,
}

impl Check {
    fn issue(&mut self, problem: StandardPackProblem, pointer: impl Into<String>) {
        if self.issues.len() < MAX_STANDARD_PACK_GOALS_ISSUES {
            self.issues.push(StandardPackRootIssue::new(
                STANDARD_PACK_GOALS_FILE,
                problem,
                pointer,
            ));
        }
    }

    fn malformed(&mut self, pointer: impl Into<String>) {
        self.issue(StandardPackProblem::Malformed, pointer);
    }

    /// Fix round 1: a bound of the shape broken, at what exceeds it.
    fn out_of_limits(&mut self, pointer: impl Into<String>) {
        self.issue(StandardPackProblem::GoalOutOfLimits, pointer);
    }

    /// An object of the members `required` and `optional`: an unknown member and a missing one
    /// are `malformed` at its pointer, a value that is no object at the value's.
    fn object<'v>(
        &mut self,
        value: &'v Value,
        pointer: &str,
        required: &[&str],
        optional: &[&str],
    ) -> Option<&'v Map<String, Value>> {
        let Some(object) = value.as_object() else {
            self.malformed(pointer);
            return None;
        };
        self.members(object, pointer, required, optional);
        Some(object)
    }

    fn members(
        &mut self,
        object: &Map<String, Value>,
        pointer: &str,
        required: &[&str],
        optional: &[&str],
    ) {
        for name in object.keys() {
            if !required.contains(&name.as_str()) && !optional.contains(&name.as_str()) {
                self.malformed(at(pointer, name));
            }
        }
        for name in required {
            if !object.contains_key(*name) {
                self.malformed(at(pointer, name));
            }
        }
    }

    /// A list of `min..=max` items; a count outside is `goal_out_of_limits` at the list.
    fn list<'v>(
        &mut self,
        value: &'v Value,
        pointer: &str,
        (min, max): (usize, usize),
    ) -> Option<&'v [Value]> {
        let Some(items) = value.as_array() else {
            self.malformed(pointer);
            return None;
        };
        if !(min..=max).contains(&items.len()) {
            self.out_of_limits(pointer);
        }
        Some(items.as_slice())
    }

    /// A string of 1 to `max` bytes, never read further: no string is `malformed`, one of another
    /// length `goal_out_of_limits`.
    fn string(&mut self, value: &Value, pointer: &str, max: usize) -> Option<String> {
        match value.as_str() {
            Some(text) if !text.is_empty() && text.len() <= max => Some(text.to_owned()),
            Some(_) => {
                self.out_of_limits(pointer);
                None
            }
            None => {
                self.malformed(pointer);
                None
            }
        }
    }

    /// A text: 1 to 16 locales of 1 to 35 bytes each, each text 1 to `max` bytes, as `ui.json`'s;
    /// a count or a length outside is `goal_out_of_limits` at what exceeds.
    fn text(&mut self, value: &Value, pointer: &str, max: usize) -> Option<GoalText> {
        let Some(locales) = value.as_object() else {
            self.malformed(pointer);
            return None;
        };
        let mut complete = (1..=MAX_PACK_UI_LOCALES).contains(&locales.len());
        if !complete {
            self.out_of_limits(pointer);
        }
        let mut text = Vec::new();
        for (locale, value) in locales {
            let at_locale = at(pointer, locale);
            if locale.is_empty() || locale.len() > MAX_PACK_UI_LOCALE_BYTES {
                self.out_of_limits(at_locale);
                complete = false;
                continue;
            }
            match self.string(value, &at_locale, max) {
                Some(written) => text.push((locale.clone(), written)),
                None => complete = false,
            }
        }
        complete.then_some(text)
    }

    /// An integer within the safe integers of JSON.
    fn integer(&mut self, value: &Value, pointer: &str) -> Option<i64> {
        match value.as_i64() {
            Some(integer)
                if (-MAX_GOAL_SAFE_INTEGER..=MAX_GOAL_SAFE_INTEGER).contains(&integer) =>
            {
                Some(integer)
            }
            _ => {
                self.malformed(pointer);
                None
            }
        }
    }

    /// A whole number from 0 within the safe integers of JSON.
    fn count(&mut self, value: &Value, pointer: &str) -> Option<u64> {
        match value.as_u64() {
            Some(count) if count <= MAX_GOAL_SAFE_INTEGER.unsigned_abs() => Some(count),
            _ => {
                self.malformed(pointer);
                None
            }
        }
    }

    /// An id an entry names ([`opaque_id`]; its length [`bounded`]).
    fn reference(&mut self, value: &Value, pointer: &str) -> Option<String> {
        let Some(id) = value.as_str() else {
            self.malformed(pointer);
            return None;
        };
        match bounded(id, MAX_GOAL_REFERENCE_BYTES, opaque_id) {
            None => Some(id.to_owned()),
            Some(problem) => {
                self.issue(problem, pointer);
                None
            }
        }
    }

    fn document(&mut self, value: &Value, game: &str) -> Option<StandardPackGoals> {
        let document = self.object(value, "", &["schema_version", "game", "entries"], &[])?;
        if let Some(version) = document.get("schema_version")
            && version.as_str() != Some(STANDARD_PACK_GOALS_SCHEMA_VERSION)
        {
            self.malformed("/schema_version");
        }
        match document.get("game").map(Value::as_str) {
            Some(None) => self.malformed("/game"),
            Some(Some(declared)) if declared != game => {
                self.issue(StandardPackProblem::GameMismatch, "/game");
            }
            _ => {}
        }
        let entries = self.list(document.get("entries")?, "/entries", (0, MAX_GOAL_ENTRIES))?;
        let mut names = BTreeSet::new();
        let mut typed = Vec::new();
        let mut complete = true;
        for (index, entry) in entries.iter().enumerate() {
            match self.entry(entry, index, &mut names) {
                Some(goal) => typed.push(goal),
                None => complete = false,
            }
        }
        complete.then(|| StandardPackGoals {
            game: game.to_owned(),
            entries: typed,
        })
    }

    /// One entry: its name, its type, its members, its block, its texts and its type's members.
    /// An entry whose type is unknown is not read further.
    fn entry(
        &mut self,
        value: &Value,
        index: usize,
        names: &mut BTreeSet<String>,
    ) -> Option<DeclaredGoal> {
        let pointer = format!("/entries/{index}");
        let Some(object) = value.as_object() else {
            self.malformed(&pointer);
            return None;
        };
        let name = self.name(object.get("name"), &pointer, names);
        let at_type = at(&pointer, "type");
        let goal_type = match object.get("type") {
            None => {
                self.malformed(at_type);
                return None;
            }
            Some(Value::String(word)) => match GoalType::from_word(word) {
                Some(goal_type) => goal_type,
                None => {
                    self.issue(StandardPackProblem::GoalTypeUnknown, at_type);
                    return None;
                }
            },
            Some(_) => {
                self.malformed(at_type);
                return None;
            }
        };
        self.members(object, &pointer, goal_type.required(), goal_type.optional());
        let base = goal_type.base_block();
        let block = match object.get("block") {
            None => Some(base),
            Some(Value::String(word)) if word == base.as_str() => Some(base),
            Some(Value::String(_)) => {
                self.issue(
                    StandardPackProblem::GoalBlockMismatch,
                    at(&pointer, "block"),
                );
                None
            }
            Some(_) => {
                self.malformed(at(&pointer, "block"));
                None
            }
        };
        let title = object
            .get("title")
            .and_then(|title| self.text(title, &at(&pointer, "title"), MAX_PACK_UI_TITLE_BYTES));
        let help = match object.get("help") {
            None => Some(None),
            Some(help) => self
                .text(help, &at(&pointer, "help"), MAX_PACK_UI_SUMMARY_BYTES)
                .map(Some),
        };
        let kind = self.kind(goal_type, object, &pointer);
        Some(DeclaredGoal {
            name: name?,
            block: block?,
            title: title?,
            help: help?,
            kind: kind?,
        })
    }

    /// The entry's name ([`goal_name`]; its length [`bounded`]), once in the file. A missing name
    /// is reported with the entry's members.
    fn name(
        &mut self,
        value: Option<&Value>,
        pointer: &str,
        names: &mut BTreeSet<String>,
    ) -> Option<String> {
        let at_name = at(pointer, "name");
        let Value::String(name) = value? else {
            self.malformed(at_name);
            return None;
        };
        if let Some(problem) = bounded(name, MAX_GOAL_NAME_BYTES, goal_name) {
            self.issue(problem, at_name);
            return None;
        }
        if names.insert(name.clone()) {
            Some(name.clone())
        } else {
            self.issue(StandardPackProblem::GoalNameDuplicate, at_name);
            None
        }
    }

    fn kind(
        &mut self,
        goal_type: GoalType,
        object: &Map<String, Value>,
        pointer: &str,
    ) -> Option<DeclaredGoalKind> {
        match goal_type {
            GoalType::Heading => Some(DeclaredGoalKind::Heading),
            GoalType::Note => Some(DeclaredGoalKind::Note),
            GoalType::Resource => {
                let pool = self.governs_pool(object, pointer);
                let rate = match object.get("rate") {
                    None => Some(None),
                    Some(rate) => self.rate(rate, &at(pointer, "rate")).map(Some),
                };
                Some(DeclaredGoalKind::Resource {
                    pool: pool?,
                    rate: rate?,
                })
            }
            GoalType::Integer => self.integer_kind(object, pointer),
            GoalType::Choice => {
                let packages = self.governs_list(object, pointer, "packages");
                let options = object.get("options").and_then(|options| {
                    self.options(
                        options,
                        &at(pointer, "options"),
                        (MIN_GOAL_CHOICE_OPTIONS, MAX_GOAL_CHOICE_OPTIONS),
                    )
                });
                let values = options.as_ref().map(|options| {
                    options
                        .iter()
                        .map(|option| option.value.clone())
                        .collect::<Vec<_>>()
                });
                let default = self.one_of(object, pointer, values.as_deref());
                Some(DeclaredGoalKind::Choice {
                    packages: packages?,
                    options: options?,
                    default: default?,
                })
            }
            GoalType::Switch => {
                let packages = self.governs_list(object, pointer, "packages");
                let default = match object.get("default") {
                    Some(Value::Bool(default)) => Some(*default),
                    Some(_) => {
                        self.issue(
                            StandardPackProblem::GoalDefaultInvalid,
                            at(pointer, "default"),
                        );
                        None
                    }
                    None => None,
                };
                Some(DeclaredGoalKind::Switch {
                    packages: packages?,
                    default: default?,
                })
            }
            GoalType::Ordered => self.ordered_kind(object, pointer),
            GoalType::Tier => {
                let rules = self.governs_list(object, pointer, "rules");
                let options = object
                    .get("options")
                    .and_then(|options| self.tier_options(options, &at(pointer, "options")));
                let values = options.as_ref().map(|options| {
                    options
                        .iter()
                        .map(|option| option.value.clone())
                        .collect::<Vec<_>>()
                });
                let default = self.one_of(object, pointer, values.as_deref());
                Some(DeclaredGoalKind::Tier {
                    rules: rules?,
                    options: options?,
                    default: default?,
                })
            }
        }
    }

    /// `governs` as `{pool}`: one id.
    fn governs_pool(&mut self, object: &Map<String, Value>, pointer: &str) -> Option<String> {
        let at_governs = at(pointer, "governs");
        let governs = self.object(object.get("governs")?, &at_governs, &["pool"], &[])?;
        self.reference(governs.get("pool")?, &at(&at_governs, "pool"))
    }

    /// `governs` as `{<member>: [ids]}` (`packages` or `rules`): 1 to [`MAX_GOAL_GOVERNED`] ids,
    /// each named once.
    fn governs_list(
        &mut self,
        object: &Map<String, Value>,
        pointer: &str,
        member: &str,
    ) -> Option<Vec<String>> {
        let at_governs = at(pointer, "governs");
        let governs = self.object(object.get("governs")?, &at_governs, &[member], &[])?;
        let at_list = at(&at_governs, member);
        let items = self.list(governs.get(member)?, &at_list, (1, MAX_GOAL_GOVERNED))?;
        let mut seen = BTreeSet::new();
        let mut ids = Vec::new();
        let mut complete = true;
        for (index, item) in items.iter().enumerate() {
            let at_item = format!("{at_list}/{index}");
            match self.reference(item, &at_item) {
                Some(id) if seen.insert(id.clone()) => ids.push(id),
                Some(_) => {
                    self.malformed(at_item);
                    complete = false;
                }
                None => complete = false,
            }
        }
        complete.then_some(ids)
    }

    /// A `rate`: `{floor_milli_per_day, ceiling_milli_per_day}`, the ceiling not below the floor.
    fn rate(&mut self, value: &Value, pointer: &str) -> Option<GoalRate> {
        let rate = self.object(
            value,
            pointer,
            &["floor_milli_per_day", "ceiling_milli_per_day"],
            &[],
        )?;
        let floor = rate
            .get("floor_milli_per_day")
            .and_then(|floor| self.integer(floor, &at(pointer, "floor_milli_per_day")));
        let ceiling = rate
            .get("ceiling_milli_per_day")
            .and_then(|ceiling| self.integer(ceiling, &at(pointer, "ceiling_milli_per_day")));
        let (floor, ceiling) = (floor?, ceiling?);
        if ceiling < floor {
            self.issue(
                StandardPackProblem::GoalRateInvalid,
                at(pointer, "ceiling_milli_per_day"),
            );
            return None;
        }
        Some(GoalRate {
            floor_milli_per_day: floor,
            ceiling_milli_per_day: ceiling,
        })
    }

    fn integer_kind(
        &mut self,
        object: &Map<String, Value>,
        pointer: &str,
    ) -> Option<DeclaredGoalKind> {
        let packages = self.governs_list(object, pointer, "packages");
        let min = object
            .get("min")
            .and_then(|min| self.integer(min, &at(pointer, "min")));
        let max = object
            .get("max")
            .and_then(|max| self.integer(max, &at(pointer, "max")));
        let step = match object.get("step") {
            None => Some(1),
            Some(step) => self.integer(step, &at(pointer, "step")),
        };
        let unit = match object.get("unit") {
            None => Some(None),
            Some(unit) => self.unit(unit, &at(pointer, "unit")).map(Some),
        };
        let range = match (min, max, step) {
            (Some(min), Some(max), Some(step)) => {
                if min > max {
                    self.issue(StandardPackProblem::GoalRangeInvalid, at(pointer, "min"));
                    None
                } else if step < 1 {
                    self.issue(StandardPackProblem::GoalRangeInvalid, at(pointer, "step"));
                    None
                } else {
                    Some((min, max, step))
                }
            }
            _ => None,
        };
        // The default is judged against a range that holds; a broken range is its own refusal.
        let default = match (range, object.get("default")) {
            (Some((min, max, step)), Some(default)) => match default.as_i64() {
                Some(default)
                    if (min..=max).contains(&default)
                        && (i128::from(default) - i128::from(min)) % i128::from(step) == 0 =>
                {
                    Some(default)
                }
                _ => {
                    self.issue(
                        StandardPackProblem::GoalDefaultInvalid,
                        at(pointer, "default"),
                    );
                    None
                }
            },
            _ => None,
        };
        let (min, max, step) = range?;
        Some(DeclaredGoalKind::Integer {
            packages: packages?,
            min,
            max,
            step,
            unit: unit?,
            default: default?,
        })
    }

    /// A `unit`: a time word or a text.
    fn unit(&mut self, value: &Value, pointer: &str) -> Option<GoalUnit> {
        match value {
            Value::String(word) => match GoalTimeUnit::from_word(word) {
                Some(unit) => Some(GoalUnit::Time(unit)),
                None => {
                    self.malformed(pointer);
                    None
                }
            },
            _ => self
                .text(value, pointer, MAX_PACK_UI_TITLE_BYTES)
                .map(GoalUnit::Text),
        }
    }

    /// `options`: `bounds` options `{value, title}`, each value written once.
    fn options(
        &mut self,
        value: &Value,
        pointer: &str,
        bounds: (usize, usize),
    ) -> Option<Vec<GoalOption>> {
        let items = self.list(value, pointer, bounds)?;
        let mut seen = BTreeSet::new();
        let mut options = Vec::new();
        let mut complete = true;
        for (index, item) in items.iter().enumerate() {
            let at_option = format!("{pointer}/{index}");
            let Some(option) = self.object(item, &at_option, &["value", "title"], &[]) else {
                complete = false;
                continue;
            };
            let value = option
                .get("value")
                .and_then(|value| self.option_value(value, &at(&at_option, "value"), &mut seen));
            let title = option.get("title").and_then(|title| {
                self.text(title, &at(&at_option, "title"), MAX_PACK_UI_TITLE_BYTES)
            });
            match (value, title) {
                (Some(value), Some(title)) => options.push(GoalOption { value, title }),
                _ => complete = false,
            }
        }
        complete.then_some(options)
    }

    /// A `tier` entry's `options`: 2 to 32 tiers `{value, title, scale_milli}`, each value written
    /// once, each scale from 1 to [`MAX_GOAL_SCALE_MILLI`].
    fn tier_options(&mut self, value: &Value, pointer: &str) -> Option<Vec<GoalTierOption>> {
        let items = self.list(
            value,
            pointer,
            (MIN_GOAL_CHOICE_OPTIONS, MAX_GOAL_CHOICE_OPTIONS),
        )?;
        let mut seen = BTreeSet::new();
        let mut options = Vec::new();
        let mut complete = true;
        for (index, item) in items.iter().enumerate() {
            let at_option = format!("{pointer}/{index}");
            let Some(option) =
                self.object(item, &at_option, &["value", "title", "scale_milli"], &[])
            else {
                complete = false;
                continue;
            };
            let value = option
                .get("value")
                .and_then(|value| self.option_value(value, &at(&at_option, "value"), &mut seen));
            let title = option.get("title").and_then(|title| {
                self.text(title, &at(&at_option, "title"), MAX_PACK_UI_TITLE_BYTES)
            });
            let scale_milli = option
                .get("scale_milli")
                .and_then(|scale| self.scale(scale, &at(&at_option, "scale_milli")));
            match (value, title, scale_milli) {
                (Some(value), Some(title), Some(scale_milli)) => options.push(GoalTierOption {
                    value,
                    title,
                    scale_milli,
                }),
                _ => complete = false,
            }
        }
        complete.then_some(options)
    }

    /// An option value ([`option_value`]; its length [`bounded`]) not written before in its list
    /// (`goal_range_invalid`).
    fn option_value(
        &mut self,
        value: &Value,
        pointer: &str,
        seen: &mut BTreeSet<String>,
    ) -> Option<String> {
        let Some(word) = value.as_str() else {
            self.malformed(pointer);
            return None;
        };
        if let Some(problem) = bounded(word, MAX_GOAL_VALUE_BYTES, option_value) {
            self.issue(problem, pointer);
            return None;
        }
        if seen.insert(word.to_owned()) {
            Some(word.to_owned())
        } else {
            self.issue(StandardPackProblem::GoalRangeInvalid, pointer);
            None
        }
    }

    /// A tier's `scale_milli`: a safe integer ([`Check::integer`]: `malformed` otherwise, fix
    /// round 1, as every integer member of the file) from 1 to [`MAX_GOAL_SCALE_MILLI`]
    /// (`goal_range_invalid` otherwise).
    fn scale(&mut self, value: &Value, pointer: &str) -> Option<i64> {
        let scale = self.integer(value, pointer)?;
        if (1..=MAX_GOAL_SCALE_MILLI).contains(&scale) {
            Some(scale)
        } else {
            self.issue(StandardPackProblem::GoalRangeInvalid, pointer);
            None
        }
    }

    /// A `choice` or `tier` entry's `default`: one of `values`, judged once the options hold.
    fn one_of(
        &mut self,
        object: &Map<String, Value>,
        pointer: &str,
        values: Option<&[String]>,
    ) -> Option<String> {
        let (Some(values), Some(default)) = (values, object.get("default")) else {
            return None;
        };
        match default.as_str() {
            Some(default) if values.iter().any(|value| value == default) => {
                Some(default.to_owned())
            }
            _ => {
                self.issue(
                    StandardPackProblem::GoalDefaultInvalid,
                    at(pointer, "default"),
                );
                None
            }
        }
    }

    fn ordered_kind(
        &mut self,
        object: &Map<String, Value>,
        pointer: &str,
    ) -> Option<DeclaredGoalKind> {
        let packages = self.governs_list(object, pointer, "packages");
        let values = match (object.get("options"), object.get("from_table")) {
            (Some(options), None) => self
                .options(options, &at(pointer, "options"), (1, MAX_GOAL_LIST_VALUES))
                .map(GoalValues::Options),
            (None, Some(table)) => self.from_table(table, &at(pointer, "from_table")),
            (Some(_), Some(_)) => {
                self.malformed(at(pointer, "from_table"));
                None
            }
            (None, None) => {
                self.malformed(at(pointer, "options"));
                None
            }
        };
        let min_count = object
            .get("min_count")
            .and_then(|count| self.count(count, &at(pointer, "min_count")));
        let max_count = object
            .get("max_count")
            .and_then(|count| self.count(count, &at(pointer, "max_count")));
        let counts = match (min_count, max_count) {
            (Some(min_count), Some(max_count)) => {
                let options = match &values {
                    Some(GoalValues::Options(options)) => Some(options.len() as u64),
                    _ => None,
                };
                if max_count == 0 || max_count > MAX_GOAL_ORDERED_COUNT {
                    self.issue(
                        StandardPackProblem::GoalRangeInvalid,
                        at(pointer, "max_count"),
                    );
                    None
                } else if min_count > max_count
                    || options.is_some_and(|options| min_count > options)
                {
                    self.issue(
                        StandardPackProblem::GoalRangeInvalid,
                        at(pointer, "min_count"),
                    );
                    None
                } else {
                    Some((min_count, max_count))
                }
            }
            _ => None,
        };
        let default = match (counts, &values, object.get("default")) {
            (Some(counts), Some(values), Some(default)) => {
                self.ordered_default(default, &at(pointer, "default"), counts, values)
            }
            _ => None,
        };
        let (min_count, max_count) = counts?;
        Some(DeclaredGoalKind::Ordered {
            packages: packages?,
            values: values?,
            min_count,
            max_count,
            default: default?,
        })
    }

    /// `from_table`: `{table, names?}`, the table an id of the table rule, each name's key an id
    /// and its text a title.
    fn from_table(&mut self, value: &Value, pointer: &str) -> Option<GoalValues> {
        let from_table = self.object(value, pointer, &["table"], &["names"])?;
        let table = match from_table.get("table")?.as_str() {
            Some(table) if standard_pack_table_id(table) => Some(table.to_owned()),
            _ => {
                self.malformed(at(pointer, "table"));
                None
            }
        };
        let names = match from_table.get("names") {
            None => Some(Vec::new()),
            Some(names) => self.names(names, &at(pointer, "names")),
        };
        Some(GoalValues::Table {
            table: table?,
            names: names?,
        })
    }

    fn names(&mut self, value: &Value, pointer: &str) -> Option<Vec<(String, GoalText)>> {
        let Some(members) = value.as_object() else {
            self.malformed(pointer);
            return None;
        };
        let mut complete = members.len() <= MAX_GOAL_LIST_VALUES;
        if !complete {
            self.out_of_limits(pointer);
        }
        let mut names = Vec::new();
        for (key, text) in members {
            let at_key = at(pointer, key);
            if let Some(problem) = bounded(key, MAX_GOAL_REFERENCE_BYTES, opaque_id) {
                self.issue(problem, at_key);
                complete = false;
                continue;
            }
            match self.text(text, &at_key, MAX_PACK_UI_TITLE_BYTES) {
                Some(text) => names.push((key.clone(), text)),
                None => complete = false,
            }
        }
        complete.then_some(names)
    }

    /// An `ordered` entry's `default`: a list of its values, each once, `counts` long. A table's
    /// keys are judged with the root's tables ([`check_standard_pack_goal_references`]).
    fn ordered_default(
        &mut self,
        value: &Value,
        pointer: &str,
        (min_count, max_count): (u64, u64),
        values: &GoalValues,
    ) -> Option<Vec<String>> {
        let Some(items) = value.as_array() else {
            self.issue(StandardPackProblem::GoalDefaultInvalid, pointer);
            return None;
        };
        let mut seen = BTreeSet::new();
        let mut chosen = Vec::new();
        let mut complete = true;
        for (index, item) in items.iter().enumerate() {
            let offered = match (item.as_str(), values) {
                (Some(key), GoalValues::Options(options)) => {
                    options.iter().any(|option| option.value == key)
                }
                (Some(key), GoalValues::Table { .. }) => opaque_id(key),
                (None, _) => false,
            };
            match item.as_str() {
                Some(key) if offered && seen.insert(key.to_owned()) => chosen.push(key.to_owned()),
                _ => {
                    self.issue(
                        StandardPackProblem::GoalDefaultInvalid,
                        format!("{pointer}/{index}"),
                    );
                    complete = false;
                }
            }
        }
        if !(min_count..=max_count).contains(&(items.len() as u64)) {
            self.issue(StandardPackProblem::GoalDefaultInvalid, pointer);
            complete = false;
        }
        complete.then_some(chosen)
    }

    /// The references of an `ordered` entry at `pointer` whose values are the keys of `table`.
    fn table_values(
        &mut self,
        pointer: &str,
        table: &str,
        names: &[(String, GoalText)],
        min_count: u64,
        default: &[String],
        tables: &BTreeMap<String, Vec<String>>,
    ) {
        let at_table = at(pointer, "from_table");
        let Some(keys) = tables.get(table) else {
            self.issue(
                StandardPackProblem::GoalTableUnknown,
                at(&at_table, "table"),
            );
            return;
        };
        if keys.len() > MAX_GOAL_LIST_VALUES {
            self.issue(
                StandardPackProblem::GoalRangeInvalid,
                at(&at_table, "table"),
            );
            return;
        }
        let keys = keys.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let at_names = at(&at_table, "names");
        for (key, _) in names {
            if !keys.contains(key.as_str()) {
                self.issue(StandardPackProblem::GoalTableUnknown, at(&at_names, key));
            }
        }
        if min_count > keys.len() as u64 {
            self.issue(
                StandardPackProblem::GoalRangeInvalid,
                at(pointer, "min_count"),
            );
        }
        for (index, key) in default.iter().enumerate() {
            if !keys.contains(key.as_str()) {
                self.issue(
                    StandardPackProblem::GoalDefaultInvalid,
                    format!("{pointer}/default/{index}"),
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "standard_pack_goals_tests.rs"]
mod tests;
