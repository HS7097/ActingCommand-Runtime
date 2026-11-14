// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #455 G-R1 (register §1.356; `contracts/standard-pack-goals.md`): a game root's optional
//! goal declaration, `goals.json` (`actingcommand.goals.v1`), beside `bundle.json`: the goal entries
//! the game offers on one page, in file order, each with its stable name, its type, its block, its
//! texts and what it governs (a pool of `schedule.json` or rules of `schedule.json`), or (Workflow
//! #463 V1) the place on the page of a variable some package of the root declares.
//!
//! Two pure steps. [`parse_standard_pack_goals`] reads the file strictly into typed entries: its
//! shape and limits, the names, the five types and their members, the blocks, the ranges, the
//! defaults and the rates. [`check_standard_pack_goal_references`] then holds the typed entries
//! to what the game root offers ([`GoalReferences`]: the composed schedule's pools and rules, each
//! rule with the package it runs, the bundle's packages, each with what its control declaration
//! provides, and the variables its packages declare). Every violation is one
//! [`StandardPackRootIssue`] of [`STANDARD_PACK_GOALS_FILE`] at its JSON pointer, in document order
//! (a member repeated in one object first), at most [`MAX_STANDARD_PACK_GOALS_ISSUES`]; the
//! references are judged only for a file the reader accepts.
//!
//! G-R1 fix round 1 (the independent review): a package can do something irreversible when it
//! provides a capability that any package of the game root marks irreversible
//! (contracts/remedies.md §2.3), whether it marks it itself or not; a `tier` entry is held to the
//! package each rule it governs runs; and [`GoalReferences`] is made only by
//! [`GoalReferences::new`], from what every package of the root provides, so no reader can leave
//! that answer empty by omission. G-R1 fix round 2 (the differential review): a `tier` entry is
//! also held to the packages the rules that follow its rules run ([`GoalRule::followers`]).
//!
//! Workflow #463 V1 (`contracts/package-variables.md`): the four types that governed packages
//! (`integer`, `choice`, `switch`, `ordered`) are replaced by the placement `variable`, and the
//! value-type code they had moved to [`crate::package_variables`], which reads a package's
//! `variables` with this module's [`Check`]. A placement is held to the variables the root's
//! packages declare (`goal_variable_unknown`) and, as a governed package was, to the packages that
//! declare it (`goal_governs_irreversible`).
//!
//! The file stores no value: a value is the installation's, set by a person or an agent in a later
//! slice. The Runtime interprets no text here: titles, help texts, unit texts, option titles and
//! locales are opaque, measured in bytes only, and ids and option values are compared for equality
//! only. No Runtime reader opens the file in this slice (its root reader, its startup and run
//! preparation, the policy crate and check-config); `actinglab standard-pack check` reports a
//! refused file without failing the root, and `package bundle --goals` refuses one.

use crate::standard_pack::{StandardPackProblem, StandardPackRootIssue};
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
/// The longest id an entry names (a pool, a rule), in bytes.
pub const MAX_GOAL_REFERENCE_BYTES: usize = 128;
/// The most rules one `governs` names.
pub const MAX_GOAL_GOVERNED: usize = 32;
/// The fewest options of a `tier` entry, and (Workflow #463 V1) of a `choice` variable.
pub const MIN_GOAL_CHOICE_OPTIONS: usize = 2;
/// The most options of a `tier` entry, and (Workflow #463 V1) of a `choice` variable.
pub const MAX_GOAL_CHOICE_OPTIONS: usize = 32;
/// Workflow #463 V1: the most options of an `ordered` variable.
pub const MAX_GOAL_LIST_VALUES: usize = 1024;
/// Workflow #463 V1: the largest `max_count` of an `ordered` variable.
pub const MAX_GOAL_ORDERED_COUNT: u64 = 32;
/// The longest option value, in bytes.
pub const MAX_GOAL_VALUE_BYTES: usize = 64;
/// The largest `scale_milli` of a `tier` option (the smallest is 1).
pub const MAX_GOAL_SCALE_MILLI: i64 = 1_000_000;
/// The largest integer of the file, in magnitude: the safe integers of JSON, 2^53 - 1.
pub const MAX_GOAL_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// The type of an entry (`type`), one of the five of version 1 (Workflow #463 V1, which replaced
/// the four types that governed packages by `variable`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GoalType {
    /// A group title between entries; no value, governs nothing.
    Heading,
    /// The author's text; no value, governs nothing.
    Note,
    /// A goal on one pool: an amount to hold, later with a deadline.
    Resource,
    /// One of 2 to 32 tiers, each scaling the fixed value of the rules it governs.
    Tier,
    /// Workflow #463 V1: the place on the page of a variable a package of the root declares; its
    /// value, its texts and its block are the variable's.
    Variable,
}

impl GoalType {
    /// The five types of version 1, in the contract's order.
    pub const ALL: [Self; 5] = [
        Self::Heading,
        Self::Note,
        Self::Resource,
        Self::Tier,
        Self::Variable,
    ];

    /// The type's word in the file.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Heading => "heading",
            Self::Note => "note",
            Self::Resource => "resource",
            Self::Tier => "tier",
            Self::Variable => "variable",
        }
    }

    /// The type a word names; `None` for any other word (a later version's type, and the four
    /// types Workflow #463 V1 replaced, too).
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|goal_type| goal_type.as_str() == word)
    }

    /// The block every reader shows the type with: the only block version 1 gives it. `None` for
    /// a `variable` placement, which a reader shows with the placed variable's block.
    pub const fn base_block(self) -> Option<GoalBlock> {
        match self {
            Self::Heading => Some(GoalBlock::Group),
            Self::Note => Some(GoalBlock::Text),
            Self::Resource => Some(GoalBlock::GoalRow),
            Self::Tier => Some(GoalBlock::Choice),
            Self::Variable => None,
        }
    }

    /// The members an entry of the type must hold.
    const fn required(self) -> &'static [&'static str] {
        match self {
            Self::Heading | Self::Note => HEADING_REQUIRED,
            Self::Resource => RESOURCE_REQUIRED,
            Self::Tier => TIER_REQUIRED,
            Self::Variable => VARIABLE_REQUIRED,
        }
    }

    /// The members an entry of the type may hold besides the required ones.
    const fn optional(self) -> &'static [&'static str] {
        match self {
            Self::Heading | Self::Note | Self::Tier => COMMON_OPTIONAL,
            Self::Resource => RESOURCE_OPTIONAL,
            Self::Variable => VARIABLE_OPTIONAL,
        }
    }
}

const HEADING_REQUIRED: &[&str] = &["name", "type", "title"];
const RESOURCE_REQUIRED: &[&str] = &["name", "type", "title", "governs"];
const TIER_REQUIRED: &[&str] = &["name", "type", "title", "governs", "options", "default"];
/// Workflow #463 V1: a placement writes no texts and no value of its own.
const VARIABLE_REQUIRED: &[&str] = &["name", "type"];
const COMMON_OPTIONAL: &[&str] = &["block", "help"];
const RESOURCE_OPTIONAL: &[&str] = &["block", "help", "rate"];
const VARIABLE_OPTIONAL: &[&str] = &["block"];

/// A block (`block`): how a reader shows an entry or (Workflow #463 V1) a variable. Version 1
/// gives each type one block, its base block; a reader that meets a block it does not know shows
/// the type's base block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GoalBlock {
    /// A heading between entries (`heading`).
    Group,
    /// Text to read, not to change (`note`).
    Text,
    /// An amount to hold on a pool, with its current reading (`resource`).
    GoalRow,
    /// A number typed in, with its unit and its limits (an `integer` variable).
    Number,
    /// One of a few options, side by side up to four, in a drop-down above (`tier`, a `choice`
    /// variable).
    Choice,
    /// On or off (a `switch` variable).
    Switch,
    /// Values in an order a person sets, with search when the list is long (an `ordered`
    /// variable).
    OrderedList,
}

impl GoalBlock {
    /// The seven blocks of version 1.
    pub const ALL: [Self; 7] = [
        Self::Group,
        Self::Text,
        Self::GoalRow,
        Self::Number,
        Self::Choice,
        Self::Switch,
        Self::OrderedList,
    ];

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

    /// The block a word names; `None` for any other word.
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|block| block.as_str() == word)
    }

    /// Workflow #463 V1: whether the block is one a variable is shown with (`number`, `choice`,
    /// `switch`, `ordered_list`), the only blocks a placement may write.
    pub const fn shows_a_variable(self) -> bool {
        matches!(
            self,
            Self::Number | Self::Choice | Self::Switch | Self::OrderedList
        )
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

    /// Workflow #463 V1: the names the entries use for anything but a `variable` placement, which
    /// no variable of the root may have (`variable_conflict` of the root check,
    /// [`crate::check_root_variables`]).
    pub fn names_besides_placements(&self) -> BTreeSet<&str> {
        self.entries
            .iter()
            .filter(|entry| entry.kind != DeclaredGoalKind::Variable)
            .map(|entry| entry.name.as_str())
            .collect()
    }
}

/// One entry of the page: its stable name (the Lab's name and, later, its fact name), the block it
/// is shown with, its texts and its type with the type's members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredGoal {
    pub name: String,
    /// The block written, or the type's base block when the entry writes none. Workflow #463 V1:
    /// for a `variable` placement the block it writes, `None` when it writes none (a reader then
    /// shows the placed variable's block).
    pub block: Option<GoalBlock>,
    /// The entry's title. Workflow #463 V1: `None` for a `variable` placement, whose variable
    /// carries its own texts.
    pub title: Option<GoalText>,
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
    /// `governs.rules` and its tiers (a later slice compiles them).
    Tier {
        rules: Vec<String>,
        options: Vec<GoalTierOption>,
        default: String,
    },
    /// Workflow #463 V1: the place of the variable the entry's name names.
    Variable,
}

impl DeclaredGoalKind {
    pub const fn goal_type(&self) -> GoalType {
        match self {
            Self::Heading => GoalType::Heading,
            Self::Note => GoalType::Note,
            Self::Resource { .. } => GoalType::Resource,
            Self::Tier { .. } => GoalType::Tier,
            Self::Variable => GoalType::Variable,
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

/// An `integer` variable's `unit` (Workflow #463 V1; an `integer` entry's before it): a time word,
/// whose values are milliseconds a reader shows in that unit, or a text per locale.
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

    pub(crate) fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|unit| unit.as_str() == word)
    }
}

/// One option of a `choice` or `ordered` variable (Workflow #463 V1; of such an entry before it).
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

/// A rule of `schedule.json` as the references hold it (G-R1 fix round 1): the package it runs,
/// which a `tier` entry that governs the rule is held to, and whether it writes
/// `fixed_value_milli`; fix round 2: and the rules that follow it, whose packages the tier is held
/// to as well.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalRule {
    /// The rule's `package_id`.
    pub package_id: String,
    /// The rule writes `fixed_value_milli`.
    pub fixed_value: bool,
    /// G-R1 fix round 2: the ids of the rules that follow this one, in the composition's order:
    /// the chain followers the composition lists as riding with it (`benefit_followers`), which run
    /// when it completes (`dependency_completed`, contracts/scheduling/README.md, "Ties and chain
    /// heads"), so that scaling its value makes them run more with it. Required, so that no
    /// reader leaves them out by omission.
    pub followers: Vec<String>,
}

/// Workflow #463 V1: a variable of the game root as the references hold it: the block its kind is
/// shown with (the first declaration's, in registry order) and every package that declares it, in
/// registry order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalVariable {
    pub block: GoalBlock,
    pub packages: Vec<String>,
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
    /// Workflow #463 V1: the variables the packages of the root declare, by name; none until a
    /// reader adds them, so that every placement is then `goal_variable_unknown`.
    pub variables: BTreeMap<String, GoalVariable>,
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
    /// itself or not. A package `provides` does not name is no package of the root: a rule that
    /// runs it, or a variable it declares, is never passed. The pools, the rules and the variables
    /// start empty.
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
            variables: BTreeMap::new(),
            packages,
        }
    }

    /// Whether the package `package_id` can do something irreversible; `None` when it is no
    /// package of the root.
    fn irreversible(&self, package_id: &str) -> Option<bool> {
        self.packages.get(package_id).copied()
    }

    /// G-R1 fix round 2: whether a rule that follows `rule` ([`GoalRule::followers`]), directly
    /// or through further followers, runs a package that can do something irreversible, or is a
    /// rule or runs a package the references were not told of (never passed). Each follower is
    /// visited once, so a cycle of followers ends the walk.
    fn follower_irreversible(&self, rule: &GoalRule) -> bool {
        let mut visited = BTreeSet::new();
        let mut pending = vec![rule];
        while let Some(current) = pending.pop() {
            for follower_id in &current.followers {
                if !visited.insert(follower_id.as_str()) {
                    continue;
                }
                match self.rules.get(follower_id) {
                    Some(follower) if self.irreversible(&follower.package_id) == Some(false) => {
                        pending.push(follower);
                    }
                    _ => return true,
                }
            }
        }
        false
    }
}

/// `goals.json`'s bytes, read for the game directory `game` (contract "The checks", the reader's
/// rows). Every violation is one issue at its JSON pointer, at most
/// [`MAX_STANDARD_PACK_GOALS_ISSUES`]: every member repeated in one object first, then the others
/// in document order:
///
/// - `goal_out_of_limits` (fix round 1): the file is larger than [`MAX_STANDARD_PACK_GOALS_BYTES`]
///   (the whole document); a list, a map, a text's locales, or the bytes of a name, an option
///   value, an id, a locale or a text outside its limit (at what exceeds);
/// - `malformed`: no JSON or no object (the whole document; a byte-order mark too); a member
///   repeated in one object, unknown, missing or not of its type (an integer outside the safe
///   integers too), a `schema_version` other than [`STANDARD_PACK_GOALS_SCHEMA_VERSION`], a name,
///   an option value or an id outside its form;
/// - `game_mismatch`: `game` is not the directory's;
/// - `goal_name_duplicate`: a name an earlier entry has (at the later `name`);
/// - `goal_type_unknown`: a `type` that is none of the five of version 1 (Workflow #463 V1: the
///   four types it replaced too);
/// - `goal_block_mismatch`: a `block` other than the type's base block, or (Workflow #463 V1) a
///   placement's `block` that is no block a variable is shown with;
/// - `goal_range_invalid`: an option value written twice, a tier's `scale_milli` outside 1 to
///   [`MAX_GOAL_SCALE_MILLI`];
/// - `goal_default_invalid`: a tier's `default` that is none of its option values;
/// - `goal_rate_invalid`: a `rate` whose ceiling is below its floor.
pub fn parse_standard_pack_goals(
    bytes: &[u8],
    game: &str,
) -> Result<StandardPackGoals, Vec<StandardPackRootIssue>> {
    let mut check = Check::new(STANDARD_PACK_GOALS_FILE);
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
/// - `goal_package_unknown`: (fix round 1) the package a governed rule runs, or (Workflow #463 V1)
///   a package that declares a placed variable, is not a package of `bundle.json`;
/// - `goal_governs_irreversible`: (fix round 1) the package a governed rule runs can do something
///   irreversible ([`GoalReferences::new`]); or (fix round 2, at the governed rule) a rule that
///   follows it, directly or through further followers, runs such a package or one the
///   references were not told of; or (Workflow #463 V1, at the placement's `name`) a package that
///   declares the placed variable can;
/// - `goal_rule_unknown`: a `tier` entry's rule is not a rule of `schedule.json`;
/// - `goal_tier_without_fixed_value`: that rule, its package and its followers passed, writes no
///   `fixed_value_milli`;
/// - Workflow #463 V1: `goal_variable_unknown` (a placement names no variable a package of the
///   root declares, at its `name`) and `goal_block_mismatch` (a placement's `block` is not the
///   placed variable's, at its `block`).
pub fn check_standard_pack_goal_references(
    goals: &StandardPackGoals,
    references: &GoalReferences,
) -> Vec<StandardPackRootIssue> {
    let mut check = Check::new(STANDARD_PACK_GOALS_FILE);
    for (index, goal) in goals.entries.iter().enumerate() {
        let pointer = format!("/entries/{index}");
        let governs = at(&pointer, "governs");
        // G-R1 fix round 2, for whoever adds an entry kind: this match has no wildcard, so a new
        // kind needs an arm, and that arm must either judge everything the kind can reach (its
        // packages, its rules and their followers, its pools) against the references, or state
        // why the kind can reach no package, as a heading and a note reach none. An empty arm
        // without that reason lets the kind past the safety line.
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
                        // Fix round 2: a rule that follows the governed rule runs when it
                        // completes, so scaling the governed rule's value runs it more too; the
                        // governed rule is held to its followers' packages, at its own pointer.
                        Some(false) if references.follower_irreversible(rule) => {
                            check.issue(StandardPackProblem::GoalGovernsIrreversible, at_rule);
                        }
                        Some(false) if !rule.fixed_value => {
                            check.issue(StandardPackProblem::GoalTierWithoutFixedValue, at_rule);
                        }
                        Some(false) => {}
                    }
                }
            }
            // Workflow #463 V1: a placement reaches the packages that declare the variable it
            // places, so it is held to each of them as a governed package was: admission keeps a
            // variable off a package that marks an irreversible capability itself, and this arm
            // keeps a placement off one that provides a capability another package marks so.
            DeclaredGoalKind::Variable => {
                let at_name = at(&pointer, "name");
                let Some(variable) = references.variables.get(&goal.name) else {
                    check.issue(StandardPackProblem::GoalVariableUnknown, at_name);
                    continue;
                };
                let reached = variable
                    .packages
                    .iter()
                    .map(|package_id| references.irreversible(package_id))
                    .collect::<Vec<_>>();
                if reached.contains(&None) {
                    check.issue(StandardPackProblem::GoalPackageUnknown, at_name);
                } else if reached.contains(&Some(true)) {
                    check.issue(StandardPackProblem::GoalGovernsIrreversible, at_name);
                } else if goal.block.is_some_and(|block| block != variable.block) {
                    check.issue(
                        StandardPackProblem::GoalBlockMismatch,
                        at(&pointer, "block"),
                    );
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

/// A tier's option value: 1 to [`MAX_GOAL_VALUE_BYTES`] bytes of `[a-z0-9][a-z0-9_.:-]*`.
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

/// An id an entry names: 1 to [`MAX_GOAL_REFERENCE_BYTES`] bytes without a control character.
fn opaque_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_GOAL_REFERENCE_BYTES && !id.chars().any(char::is_control)
}

/// Fix round 1: what is wrong with `text`, a name, an option value or an id of the form `form`:
/// `goal_out_of_limits` when it is empty or longer than `max` bytes, else `malformed` when it
/// breaks its form; `None` when it keeps both. Workflow #463 V1: shared with the reader of a
/// package's `variables`.
pub(crate) fn bounded(
    text: &str,
    max: usize,
    form: fn(&str) -> bool,
) -> Option<StandardPackProblem> {
    if text.is_empty() || text.len() > max {
        Some(StandardPackProblem::GoalOutOfLimits)
    } else if form(text) {
        None
    } else {
        Some(StandardPackProblem::Malformed)
    }
}

/// The checks of one document: the issues found so far. Workflow #463 V1: shared with the reader
/// of a package's `variables` ([`crate::package_variables`]), which reads the value types this
/// reader read before it, with the file it reports in.
pub(crate) struct Check {
    file: &'static str,
    issues: Vec<StandardPackRootIssue>,
}

impl Check {
    /// The checks of a document of `file`, with no issue yet.
    pub(crate) const fn new(file: &'static str) -> Self {
        Self {
            file,
            issues: Vec::new(),
        }
    }

    /// The issues found so far, in the order they were found.
    pub(crate) fn issues(&self) -> &[StandardPackRootIssue] {
        &self.issues
    }

    pub(crate) fn issue(&mut self, problem: StandardPackProblem, pointer: impl Into<String>) {
        if self.issues.len() < MAX_STANDARD_PACK_GOALS_ISSUES {
            self.issues
                .push(StandardPackRootIssue::new(self.file, problem, pointer));
        }
    }

    pub(crate) fn malformed(&mut self, pointer: impl Into<String>) {
        self.issue(StandardPackProblem::Malformed, pointer);
    }

    /// Fix round 1: a bound of the shape broken, at what exceeds it.
    pub(crate) fn out_of_limits(&mut self, pointer: impl Into<String>) {
        self.issue(StandardPackProblem::GoalOutOfLimits, pointer);
    }

    /// An object of the members `required` and `optional`: an unknown member and a missing one
    /// are `malformed` at its pointer, a value that is no object at the value's.
    pub(crate) fn object<'v>(
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

    pub(crate) fn members(
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
    pub(crate) fn list<'v>(
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
    pub(crate) fn string(&mut self, value: &Value, pointer: &str, max: usize) -> Option<String> {
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
    pub(crate) fn text(&mut self, value: &Value, pointer: &str, max: usize) -> Option<GoalText> {
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

    /// Workflow #463 V1: an entry's `title` (required, reported missing with its members) and its
    /// optional `help`, as texts; `None` when either breaks its rules.
    pub(crate) fn texts(
        &mut self,
        object: &Map<String, Value>,
        pointer: &str,
    ) -> Option<(GoalText, Option<GoalText>)> {
        let title = object
            .get("title")
            .and_then(|title| self.text(title, &at(pointer, "title"), MAX_PACK_UI_TITLE_BYTES));
        let help = match object.get("help") {
            None => Some(None),
            Some(help) => self
                .text(help, &at(pointer, "help"), MAX_PACK_UI_SUMMARY_BYTES)
                .map(Some),
        };
        Some((title?, help?))
    }

    /// An integer within the safe integers of JSON.
    pub(crate) fn integer(&mut self, value: &Value, pointer: &str) -> Option<i64> {
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
    pub(crate) fn count(&mut self, value: &Value, pointer: &str) -> Option<u64> {
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
        let block = self.block(object.get("block"), &pointer, goal_type.base_block());
        // Workflow #463 V1: a placement writes no texts; the placed variable carries its own.
        let texts = match goal_type {
            GoalType::Variable => Some((None, None)),
            _ => self
                .texts(object, &pointer)
                .map(|(title, help)| (Some(title), help)),
        };
        let kind = self.kind(goal_type, object, &pointer);
        let (title, help) = texts?;
        Some(DeclaredGoal {
            name: name?,
            block: block?,
            title,
            help,
            kind: kind?,
        })
    }

    /// The `block` an entry writes, judged against its type's base block `base`: left out, the
    /// base block; written, the base block. Workflow #463 V1: a placement (no base block) may
    /// leave it out (`None`) or write a block a variable is shown with, which the references hold
    /// to the placed variable's. Any other word is `goal_block_mismatch`. Shared with the reader
    /// of a package's `variables`, which gives its type's base block.
    pub(crate) fn block(
        &mut self,
        value: Option<&Value>,
        pointer: &str,
        base: Option<GoalBlock>,
    ) -> Option<Option<GoalBlock>> {
        let at_block = at(pointer, "block");
        match (value, base) {
            (None, base) => Some(base),
            (Some(Value::String(word)), Some(base)) if word == base.as_str() => Some(Some(base)),
            (Some(Value::String(word)), None) => match GoalBlock::from_word(word) {
                Some(block) if block.shows_a_variable() => Some(Some(block)),
                _ => {
                    self.issue(StandardPackProblem::GoalBlockMismatch, at_block);
                    None
                }
            },
            (Some(Value::String(_)), Some(_)) => {
                self.issue(StandardPackProblem::GoalBlockMismatch, at_block);
                None
            }
            (Some(_), _) => {
                self.malformed(at_block);
                None
            }
        }
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
            GoalType::Variable => Some(DeclaredGoalKind::Variable),
        }
    }

    /// `governs` as `{pool}`: one id.
    fn governs_pool(&mut self, object: &Map<String, Value>, pointer: &str) -> Option<String> {
        let at_governs = at(pointer, "governs");
        let governs = self.object(object.get("governs")?, &at_governs, &["pool"], &[])?;
        self.reference(governs.get("pool")?, &at(&at_governs, "pool"))
    }

    /// `governs` as `{<member>: [ids]}` (`rules`): 1 to [`MAX_GOAL_GOVERNED`] ids, each named
    /// once.
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
            let value = option.get("value").and_then(|value| {
                self.option_value(value, &at(&at_option, "value"), &mut seen, option_value)
            });
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

    /// An option value of the form `form` (its length [`bounded`]) not written before in its
    /// list (`goal_range_invalid`). Workflow #463 V1: a tier's of [`option_value`], a variable's
    /// of the token form.
    pub(crate) fn option_value(
        &mut self,
        value: &Value,
        pointer: &str,
        seen: &mut BTreeSet<String>,
        form: fn(&str) -> bool,
    ) -> Option<String> {
        let Some(word) = value.as_str() else {
            self.malformed(pointer);
            return None;
        };
        if let Some(problem) = bounded(word, MAX_GOAL_VALUE_BYTES, form) {
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

    /// A `tier` entry's or (Workflow #463 V1) a `choice` variable's `default`: one of `values`,
    /// judged once the options hold.
    pub(crate) fn one_of(
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
}

#[cfg(test)]
#[path = "standard_pack_goals_tests.rs"]
mod tests;
