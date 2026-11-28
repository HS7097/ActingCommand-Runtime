// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463 V3, which is Workflow #455 G-R2b with the variables in it (register §1.383;
//! `runtime-client` 9; `contracts/runtime-interface.md`, "Goal request kinds";
//! `contracts/package-variables.md`, "Reading and setting a value by name"): the wire shapes of
//! the two requests that read and set one application's goals by name.
//!
//! - `GoalView {instance_alias}` reads the application's goal page ([`GoalView`]): the declared
//!   entries of its game's `goals.json` in their order, each variable a package the active catalog
//!   binds on the application declares (at its placement, or after the entries), the values stored
//!   in its `session.resource_targets` v3 record with who set them and whether they are in effect,
//!   the stored values whose name is no longer offered, the packages' remedy parameters (read-only)
//!   and the resource target view. It records nothing.
//! - `EditGoal {instance_alias, expected_version?, op}` sets or clears one value by its name
//!   ([`GoalEdit`]) under the fact write gate, and answers the record's version after the edit
//!   ([`GoalEdited`]). The Runtime checks the name and the value under the gate, after it compares
//!   the version (a request of another version is a version conflict whatever its value); a
//!   refusal is `goal_value_invalid` with the word [`GoalProblem`] gives the situation.
//! - Workflow #455 RR4b-1 (register §1.557; `runtime-client` 15): `GoalPageView {instance_alias,
//!   part, cursor?, limit?}` reads, in pages, what `GoalView` does not say ([`GoalPageView`]): the
//!   records of the application's `records` entries with their positions (`records`), a table's
//!   options (`options`) and one ladder's positions (`positions`). `GoalView` stays as it is, byte
//!   for byte: its types refuse unknown members and a connection negotiates no revision, so a
//!   member added to it would break a client that reads it today.

use crate::outcome::CodeStr;
use crate::{EventId, Initiator, ResourceTargetObservation, ResourceTargetView, RuntimeChannel};
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;

crate::outcome_vocabulary! {
    /// Workflow #463 V3 (vocabulary `goal_problem`): why an `EditGoal` was refused under the fact
    /// write gate, after the version comparison (`goal_value_invalid`), one word per situation a
    /// reader can tell apart.
    pub enum GoalProblem: "goal_problem" {
        /// The name is no entry of the application's page (no entry of `goals.json`, no variable
        /// of a package the active catalog binds on the application, no remedy parameter of such
        /// a package) and no variable of its game root, and, for a `set`, the value is no other
        /// resource target `{resource, at_least}`; for a `clear`, no stored value and no target
        /// has the name.
        NameUnknown => "name_unknown",
        /// The name is a `heading` or a `note` entry of `goals.json`, which holds no value. (From
        /// Workflow #455 RR1, 0.12.92, until Workflow #455 RR2, 0.12.94, a `records` entry
        /// answered it too; since RR2 a records entry holds its records.)
        HoldsNoValue => "holds_no_value",
        /// The name is a `tier` entry of `goals.json`: no reader of this build reads a tier's
        /// value (Workflow #455 G-R5 builds it), so none is stored.
        TierNotBuilt => "tier_not_built",
        /// The name is a remedy parameter of a bound package, which `remedy set` writes with the
        /// permission it scopes; the goal page shows it read-only.
        RemedyParameter => "remedy_parameter",
        /// The name is a variable that does not apply in the application's game root; the value
        /// `variable_problem` says why. No value is stored for it.
        VariableNotApplying => "variable_not_applying",
        /// Fix round 1: the name is declared in the application's game root (a placement of
        /// `goals.json`, or a variable a package of the root declares), but by no package the
        /// active catalog binds on the application: it is not offered there, and the page lists
        /// a placement of it `not_offered`. No value is stored for it.
        NotOffered => "not_offered",
        /// `value_json` is no JSON text.
        ValueNotJson => "value_not_json",
        /// The value is not of the entry's type, or an object of other members than its own
        /// (`json_pointer` names the member). Workflow #455 RR2 (since 0.12.94): for a `records`
        /// entry, a value that is no array, a record that is no object `{key, to}` or holds
        /// another member, a `key` that is no text, a `to` that is no object or names nothing, a
        /// sub column's value that is no object of subs or names none, or a position that is no
        /// array of whole numbers (`json_pointer` names it).
        WrongType => "wrong_type",
        /// An `integer` value below `min` or above `max`, or a resource `at_least` below 1 or
        /// above the largest amount (`range_min`, `range_max`). Workflow #455 RR2 (since
        /// 0.12.94): or a record's position that the key's ladder of that column, or of that sub,
        /// does not hold, a key without such a ladder holding none (`json_pointer` names the
        /// column or the sub).
        OutOfRange => "out_of_range",
        /// An `integer` value within its range but on no step from `min` (`range_min` the
        /// variable's `min`).
        OffStep => "off_step",
        /// A `choice` value, or an item of an `ordered` value, that is none of the declaration's
        /// options (`json_pointer` names the item). Workflow #455 RR2 (since 0.12.94): or a
        /// record's `key` that is no key of the records entry's table (`json_pointer` names it).
        NotAnOption => "not_an_option",
        /// An `ordered` value that names one key twice (`json_pointer` names the later one).
        /// Workflow #455 RR2 (since 0.12.94): or a records value with two records of one key
        /// (`json_pointer` names the later record's `key`).
        KeyRepeated => "key_repeated",
        /// An `ordered` value of fewer than `min_count` or more than `max_count` keys
        /// (`range_min`, `range_max`). Workflow #455 RR2 (since 0.12.94): or a records value of
        /// more records than the entry's `max_records` (`range_min` 0, `range_max` the bound).
        CountOutOfRange => "count_out_of_range",
        /// The value's canonical text is longer than the store keeps, 1024 bytes (`range_max`).
        /// Workflow #455 RR2 (since 0.12.94): or a record's `to` whose canonical text is longer
        /// than that (`json_pointer` names the record's `to`).
        ValueTooLarge => "value_too_large",
        /// The pool of a resource value is no pool the application can target (`pool_id`).
        PoolNotTargetable => "pool_not_targetable",
        /// The pool of a resource value is targetable only with its own step or importance, which
        /// a goal by name does not carry: it has neither a valuation with `scale` and `gap` nor,
        /// as a pool without a valuation, `target_defaults` (#455 B3-P) nor `spend_line` (#455
        /// SL2: the value is then the pool's line, which carries no term) (`pool_id`).
        PoolUnvalued => "pool_unvalued",
        /// A resource value carries `by_unix_ms`: no reader of this build reads a deadline
        /// (Workflow #455 G-R4 builds it), so none is stored.
        DeadlineUnsupported => "deadline_unsupported",
        /// A `clear` of a name that holds no stored value: nothing to clear (Workflow #455 RR2,
        /// since 0.12.94: a `records` entry without stored records among them).
        ValueNotSet => "value_not_set",
        /// The application's stored record is neither a v2 nor a v3 record the readers read (a v1
        /// policy, or rows the reader refuses): write its whole document with `targets set` first.
        /// Workflow #455 RR2 (since 0.12.94): for a `records` entry, its stored
        /// `session.goal_records` record is no record the reader reads, which only a fault of the
        /// Runtime writes.
        RecordUnreadable => "record_unreadable",
        /// The edited record would hold more targets (16) or entries (96) than a record keeps
        /// (`range_max`). Workflow #455 RR2 (since 0.12.94): or the application would hold more
        /// records over all its records entries than its `session.goal_records` record keeps
        /// (128).
        RecordOutOfLimits => "record_out_of_limits",
        /// Fix round 1: what the page is judged by is not in the Runtime's memory yet: the
        /// scheduler workbench has not checked the game root since the start (or since the game
        /// came into use), or no evaluation has read the bound packages for the active catalog and
        /// procedure manifest. Nothing is known of the name and nothing is stored; the page
        /// answers `not_checked_yet` meanwhile. Fix round 2: only this situation, which passes by
        /// itself: send the same request again after the Runtime's next check.
        NotCheckedYet => "not_checked_yet",
        /// Fix round 2: the scheduler workbench checks the application's game root, and no check
        /// since the start could read it (its root files' metadata or their read fails; the page
        /// answers `root_failing` with the failure). Asking again does not help: the root must be
        /// repaired. No name can be set or cleared meanwhile, a resource goal that needs no
        /// declaration (`{resource, at_least}`) neither: while its root fails the game is
        /// uncomposed, so no goal of the application is in effect, and nothing is stored.
        RootFailing => "root_failing",
        /// Fix round 2: the application's game has no standard pack root with `schedule.json`
        /// (the standard-pack root holds no game root for it, or its game root holds no
        /// `schedule.json`), so there is nothing to check: the composed catalog holds no rule and
        /// no pool of the game, no run reads a goal of it, and the page answers `no_schedule`. No
        /// name can be set or cleared, a resource goal that needs no declaration neither (no pool
        /// of the game is one the application can target), and nothing is stored; asking again
        /// does not help until the game root holds `schedule.json`.
        NoSchedule => "no_schedule",
        /// Workflow #455 TK-1 (since 0.12.89): the name is a variable only a rule gives
        /// (`given_by: "rule"` in its declaration): no person sets it, so a `set` stores nothing;
        /// a `clear` answers it only when the application stores no value under the name (a value
        /// stored before the variable became this kind is an orphan, which `clear` removes).
        RuleGiven => "rule_given",
    }
}

/// The one change of an `EditGoal`: set the value of `name` to the JSON text `value_json`, or
/// clear it (the default again; for a resource entry the target is removed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GoalEdit {
    Set { name: String, value_json: String },
    Clear { name: String },
}

impl GoalEdit {
    /// The name the change is of.
    pub fn name(&self) -> &str {
        match self {
            Self::Set { name, .. } | Self::Clear { name } => name,
        }
    }
}

/// What `EditGoal` answers: the record's version after the edit (the next `expected_version`, an
/// opaque token compared only for equality; Workflow #455 RR2, since 0.12.94: the page's version
/// after the edit, the greater of the sequences that hold the application's two records), the
/// event of the `fact.published` that holds the record the edit wrote, whether nothing was
/// appended because the record already held the change (`replayed`), and, for a resource value,
/// the id of the target it replaced on the same pool when that target had another id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalEdited {
    pub version: u64,
    pub event_id: EventId,
    pub replayed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_target_id: Option<String>,
}

/// What `GoalView` answers for one application (`contracts/runtime-interface.md`, "Goal request
/// kinds"): its game, the version of its goal record (the sequence of the `fact.published` that
/// holds its `session.resource_targets` record, `0` without one; Workflow #455 RR2, since 0.12.94:
/// the greater of that and the sequence that holds its `session.goal_records` record: the
/// `expected_version` of the next `EditGoal`), what the workbench holds of its game's
/// `goals.json`, the page's entries in their order, the stored values no entry offers any more,
/// the bound packages' remedy parameters (read-only), the ids of the stored targets that are no
/// declared resource entry's, and the resource target view as `ResourceTargetView` answers it.
/// Workflow #455 RR2: no member speaks of the stored records, which are read as the application's
/// fact `session.goal_records`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalView {
    pub instance_alias: String,
    pub game_id: String,
    pub version: u64,
    pub declaration: GoalDeclarationState,
    pub entries: Vec<GoalViewEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orphans: Vec<GoalOrphan>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remedy_parameters: Vec<GoalRemedyParameter>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub other_targets: Vec<String>,
    pub targets: Box<ResourceTargetView>,
}

/// What the workbench holds of the game's `goals.json`: whether it read one, its sha256, and, for
/// a refused file, its first problem (`standard_pack_problem` and its pointer). Fix round 2: for
/// a game root that fails its checks (`root_failing`), why it fails, as the record the workbench's
/// last check wrote of it names it: the file it failed on (`path`), and the reader's first problem
/// with its pointer when the reader refused that file (`standard_pack_root_invalid`), or the
/// operating system's failure kind (`io_kind`) when the file could not be read or stat'ed
/// (`standard_pack_root_unreadable`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalDeclarationState {
    pub state: GoalDeclarationRead,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standard_pack_problem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_pointer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub io_kind: Option<String>,
}

/// Whether the game's `goals.json` is in effect, as the scheduler workbench's last check of the
/// game root holds it: none in the root (`absent`), read and taken (`accepted`), refused by its
/// reader (`refused`: no entry applies for that game; its variables and resource targets still
/// do), there but not readable (`unreadable`: no variable of the root applies, `goals_unreadable`),
/// or not read by this Runtime (`not_read`: no workbench reads game roots, in configured mode).
/// Fix round 1: `not_checked_yet` when what the page is judged by is not in memory yet (the
/// workbench has not checked the game root since the start, or no evaluation has read the bound
/// packages for the active catalog and procedure manifest): the page then lists nothing, every
/// `EditGoal` is refused `not_checked_yet`, and the caller asks again after the next check. Fix
/// round 2: one word per cause, each also the word every `EditGoal` is refused with while the
/// page lists nothing: `not_checked_yet` only for that, which passes by itself; `root_failing`
/// when the workbench checks the game root and no check since the start could read it (the
/// state names why: the root must be repaired); `no_schedule` when the game has no standard pack
/// root with `schedule.json`, so there is nothing to check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalDeclarationRead {
    Absent,
    Accepted,
    Refused,
    Unreadable,
    NotRead,
    NotCheckedYet,
    RootFailing,
    NoSchedule,
}

/// The type of an entry of the page: the five types of `goals.json` version 2, a variable no
/// entry places among them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalEntryType {
    Heading,
    Note,
    Resource,
    Tier,
    Variable,
}

/// Where the value an entry gives comes from: the value stored on the application, or the
/// declared default (for a resource entry: no goal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalValueSource {
    Set,
    Default,
}

/// Whether an entry's value is in effect: the next run reads it (`in_effect`), the next run reads
/// the default instead (`not_applied`, with the `variable_problem` word that says why), or no
/// reader of this build reads it (`not_built`: a `tier` entry before Workflow #455 G-R5). Fix
/// round 1: `not_offered`, a placement of `goals.json` whose variable no package the active
/// catalog binds on the application declares: no run of the application reads it, and an
/// `EditGoal` that sets it is refused with the word of the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalEntryState {
    InEffect,
    NotApplied,
    NotBuilt,
    NotOffered,
}

/// Who set a stored value, and when: the Runtime's clock at the request and the channel of its
/// connection with the initiator it declared, never the caller's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalSetBy {
    pub unix_ms: u64,
    pub channel: RuntimeChannel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initiator: Option<Initiator>,
}

/// One entry of the page, in its place: an entry of `goals.json` (`placed`, `entry` as its reader
/// read it), or a variable no entry places, after the entries. A variable shows its declaration as
/// its package writes it, `default_origin` `model` when the declaration says its default is the
/// model's, every package of the application that declares it (a shared variable is one entry),
/// its default, the stored value, where the value the next run reads comes from, that value
/// (`effective_json`), the stored keys of an `ordered` value the next run leaves out, whether it
/// is in effect and the `variable_problem` word the run gives it (`not_applied` when the run reads
/// the default instead of a stored value, or the variable does not apply; `keys_left_out` with
/// `in_effect` for a value applied in part), and the value in the declaration's own texts
/// (`display`: an option's title, a number with its unit's text). A resource entry shows its pool,
/// the target that holds its value and the pool's reading. Fix round 1: beside `packages` (the
/// bound packages that declare a variable), `unbound_packages`, the other packages of the game
/// root that declare it, which the active catalog does not bind on the application; and, for a
/// variable that does not apply in the root, the detail of the root's judgement
/// (`variable_cause`). A placement whose variable no bound package declares is `not_offered`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalViewEntry {
    pub name: String,
    pub entry_type: GoalEntryType,
    pub placed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variable: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_origin: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unbound_packages: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_json: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_json: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_json: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<GoalValueSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<GoalSetBy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<GoalEntryState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variable_problem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variable_cause: Option<GoalVariableCause>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<GoalResourceState>,
}

impl GoalViewEntry {
    /// An entry of `name` and `entry_type` with nothing else.
    pub fn new(name: impl Into<String>, entry_type: GoalEntryType, placed: bool) -> Self {
        Self {
            name: name.into(),
            entry_type,
            placed,
            entry: None,
            variable: None,
            default_origin: None,
            packages: Vec::new(),
            unbound_packages: Vec::new(),
            default_json: None,
            value_json: None,
            effective_json: None,
            removed_keys: Vec::new(),
            source: None,
            set: None,
            state: None,
            variable_problem: None,
            variable_cause: None,
            display: None,
            resource: None,
        }
    }
}

/// Fix round 1 (`contracts/runtime-interface.md`, "Goal request kinds"): why a variable does not
/// apply in the game root, beside its `variable_problem` word: the detail of the root's judgement
/// as the scheduler workbench's last check of the root holds it (the contract crate's
/// `judge_root_variables`, the judgement a dispatch reads). One member per word: the packages
/// whose declaration of the name differs from the first (`declaration_conflict`); the type of the
/// `goals.json` entry that takes the name (`goal_name_conflict`); per declaring package, the
/// capabilities that make it irreversible (`irreversible_by_root`); the packages of the root that
/// could not be read (`package_unreadable`); the name's place in the root's order, from 1
/// (`name_out_of_limits`); the packages that read the variable as a designated effect's cap and
/// the member of the declaration at fault (`cap_range`). A word without a detail
/// (`goals_unreadable`, `root_unchecked`) has none. Workflow #455 G-SLx (Runtime 0.12.78, register
/// §1.501): no judgement gives `irreversible_by_root` or `package_unreadable` any more, so no
/// Runtime answers `irreversible_capabilities` or `unreadable_packages`; the two members stay in
/// the shape, which this slice does not change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cause", rename_all = "snake_case", deny_unknown_fields)]
pub enum GoalVariableCause {
    DeclarationConflict {
        packages: Vec<String>,
    },
    GoalEntry {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        goal_type: Option<String>,
    },
    IrreversibleCapabilities {
        packages: BTreeMap<String, Vec<String>>,
    },
    UnreadablePackages {
        packages: Vec<String>,
    },
    NameOutOfLimits {
        position: u64,
    },
    CapRange {
        packages: Vec<String>,
        member: String,
    },
}

/// A resource entry's pool, the target that holds its value (absent: no goal), the pool's reading
/// as the target view shows it to the application, and the target's gap when its reading is known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalResourceState {
    pub pool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<ResourceTargetObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<u64>,
}

/// A value stored on the application whose name no entry and no variable of the page offers any
/// more ("no longer offered"); no run reads it, and `EditGoal` clears it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalOrphan {
    pub name: String,
    pub value_json: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<GoalSetBy>,
}

/// A remedy parameter of a package the active catalog binds on the application, read-only: the
/// package, the task that runs it, the capability that declares the parameter and whether it is
/// irreversible, the parameter's options and default, and the value the last evaluation resolved
/// for the application with its source (`remedy_source`), when one did. `remedy set` writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalRemedyParameter {
    pub package_id: String,
    pub task_id: String,
    pub capability: String,
    pub irreversible: bool,
    pub parameter_name: String,
    pub options: Vec<String>,
    pub default_option: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameter_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remedy_source: Option<String>,
}

// ---- Workflow #455 RR4b-1 (register §1.557; `runtime-client` 15): the goal page in pages ----

/// Workflow #455 RR4b-1: the largest page a `GoalPageView` answers, serialized: 768 KiB, the event
/// page's bound, under the 1 MiB frame.
pub const MAX_GOAL_PAGE_VIEW_BYTES: usize = 786_432;
/// Workflow #455 RR4b-1: the largest row of a page, serialized (256 KiB). A record row that would
/// pass it is answered without its texts (`texts_left_out`).
pub const MAX_GOAL_PAGE_VIEW_ROW_BYTES: usize = 262_144;
/// Workflow #455 RR4b-1: the most rows a `GoalPageView` asks for (`limit`), and the rows a page of
/// `records` or `positions` holds without it.
pub const MAX_GOAL_PAGE_VIEW_LIMIT: u32 = 512;
/// Workflow #455 RR4b-1: the rows a page of `options` holds without `limit`.
pub const DEFAULT_GOAL_PAGE_VIEW_OPTIONS_LIMIT: u32 = 64;

/// The prefix of every digest a cursor carries.
const DIGEST_PREFIX: &str = "sha256:";
/// The hex digits of a digest.
const DIGEST_HEX_DIGITS: usize = 64;

/// Workflow #455 RR4b-1: which part of the goal page a `GoalPageView` reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "part", rename_all = "snake_case", deny_unknown_fields)]
pub enum GoalPagePart {
    /// Each `records` entry of the root's `goals.json` with its stored records, then the stored
    /// records under names no records entry declares.
    Records,
    /// The table of the records entry `entry` that the root applies: its columns, its classes and
    /// its keys.
    Options { entry: String },
    /// The positions of one ladder of the table of the records entry `entry`.
    Positions {
        entry: String,
        ladder: GoalLadderRef,
    },
}

impl GoalPagePart {
    /// The rows a page of this part holds without `limit`.
    pub const fn default_limit(&self) -> u32 {
        match self {
            Self::Options { .. } => DEFAULT_GOAL_PAGE_VIEW_OPTIONS_LIMIT,
            Self::Records | Self::Positions { .. } => MAX_GOAL_PAGE_VIEW_LIMIT,
        }
    }

    /// Addendum 1: whether a cursor of this part carries the stored records' version. Only
    /// `records` does: `options` and `positions` say nothing of the stored records, so an edit of
    /// them between two pages of those parts moves nothing their cursor compares.
    pub const fn pages_records(&self) -> bool {
        matches!(self, Self::Records)
    }

    /// The digest a cursor of this part carries (`part_sha256`): `sha256:` and the lowercase hex
    /// SHA-256 of the part's members, each length-prefixed, in their order.
    pub fn digest(&self) -> String {
        let mut writer = GoalPageDigest::new();
        match self {
            Self::Records => writer.tag(1),
            Self::Options { entry } => {
                writer.tag(2);
                writer.text(entry);
            }
            Self::Positions { entry, ladder } => {
                writer.tag(3);
                writer.text(entry);
                match ladder {
                    GoalLadderRef::Class { class, column } => {
                        writer.tag(4);
                        writer.text(class);
                        writer.text(column);
                    }
                    GoalLadderRef::Key { key, column, sub } => {
                        writer.tag(5);
                        writer.text(key);
                        writer.text(column);
                        match sub {
                            Some(sub) => {
                                writer.tag(6);
                                writer.text(sub);
                            }
                            None => writer.tag(7),
                        }
                    }
                }
            }
        }
        writer.finish()
    }
}

/// The length-prefixed parts a digest of this module is hashed from.
pub struct GoalPageDigest(Sha256);

impl GoalPageDigest {
    /// A writer with nothing written.
    pub fn new() -> Self {
        Self(Sha256::new())
    }

    /// Writes one tag byte.
    pub fn tag(&mut self, tag: u8) {
        self.0.update([tag]);
    }

    /// Writes a number.
    pub fn number(&mut self, number: u64) {
        self.0.update(number.to_be_bytes());
    }

    /// Writes a text, its length first.
    pub fn text(&mut self, text: &str) {
        self.number(text.len() as u64);
        self.0.update(text.as_bytes());
    }

    /// `sha256:` and the lowercase hex digest of what was written.
    pub fn finish(self) -> String {
        format!("{DIGEST_PREFIX}{:x}", self.0.finalize())
    }
}

impl Default for GoalPageDigest {
    fn default() -> Self {
        Self::new()
    }
}

/// Workflow #455 RR4b-1: one ladder of a table: a class's ladder of a column (read once for every
/// key of the class), or the ladder a key walks in a column (of a column of `per: "sub"`, its sub).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "of", rename_all = "snake_case", deny_unknown_fields)]
pub enum GoalLadderRef {
    Class {
        class: String,
        column: String,
    },
    Key {
        key: String,
        column: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sub: Option<String>,
    },
}

/// Workflow #455 RR4b-1: where the next page of a part begins, as the page before it answered it
/// (`next`), sent back as it came. `records_version` (Addendum 1: of the part `records` alone) is
/// the sequence of the `fact.published` that holds the instance's `session.goal_records` record
/// (`0` without one); `tables_sha256` the root's accepted `goal-tables.json`'s (absent without
/// one); `entries_sha256` the digest of the root's `records` entries (name, table, `max_records`,
/// in file order); `part_sha256` the part's ([`GoalPagePart::digest`]); `after` the rows the pages
/// before answered. A cursor whose records moved is `goal_page_view_records_moved`, one whose
/// tables or records entries moved `goal_page_view_declaration_moved`: the reader starts again
/// from the first page. Fix round 1: one sent while the page is not in memory is
/// `goal_page_view_declaration_absent`: the declaration did not move, so the reader keeps what it
/// kept and starts again from the first page once a first page says the declaration is in memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalPageCursor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub records_version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tables_sha256: Option<String>,
    pub entries_sha256: String,
    pub part_sha256: String,
    pub after: u32,
}

/// Workflow #455 RR4b-1: whether a `GoalPageView`'s part, cursor and limit have their form, which
/// needs nothing of the Runtime (`invalid_goal_page_view` otherwise): every name of the part of its
/// form (an entry name's; a ladder's key, class, column and sub 1 to 128 bytes without a control
/// character), `limit` 1 to [`MAX_GOAL_PAGE_VIEW_LIMIT`], and a cursor of this part: its
/// `part_sha256` this part's digest, its records version present exactly for `records`, each
/// digest `sha256:` and 64 lowercase hex digits.
pub(crate) fn goal_page_view_well_formed(
    part: &GoalPagePart,
    cursor: Option<&GoalPageCursor>,
    limit: Option<u32>,
) -> bool {
    let names = match part {
        GoalPagePart::Records => true,
        GoalPagePart::Options { entry } => crate::goal_name(entry),
        GoalPagePart::Positions { entry, ladder } => {
            crate::goal_name(entry)
                && match ladder {
                    GoalLadderRef::Class { class, column } => page_name(class) && page_name(column),
                    GoalLadderRef::Key { key, column, sub } => {
                        page_name(key) && page_name(column) && sub.as_deref().is_none_or(page_name)
                    }
                }
        }
    };
    let limit_in_range = limit.is_none_or(|limit| (1..=MAX_GOAL_PAGE_VIEW_LIMIT).contains(&limit));
    let cursor_of_part = cursor.is_none_or(|cursor| {
        cursor.records_version.is_some() == part.pages_records()
            && cursor.tables_sha256.as_deref().is_none_or(digest_form)
            && digest_form(&cursor.entries_sha256)
            && cursor.part_sha256 == part.digest()
    });
    names && limit_in_range && cursor_of_part
}

/// Whether `name` has the form of a name a part's ladder names.
fn page_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= crate::MAX_GOAL_TABLE_ID_BYTES
        && !name.chars().any(char::is_control)
}

/// Whether `digest` is `sha256:` and 64 lowercase hex digits.
fn digest_form(digest: &str) -> bool {
    digest.strip_prefix(DIGEST_PREFIX).is_some_and(|hex| {
        hex.len() == DIGEST_HEX_DIGITS
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// Workflow #455 RR4b-1: a text the pack wrote, by locale in the pack's order; on the wire an
/// object of locale to string, as a `goals.json` entry's `title`. The Runtime chooses no language
/// and writes none of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalPageText(pub Vec<(String, String)>);

impl Serialize for GoalPageText {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (locale, text) in &self.0 {
            map.serialize_entry(locale, text)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for GoalPageText {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(GoalPageTextVisitor)
    }
}

/// Reads a [`GoalPageText`] in the order its object is written.
struct GoalPageTextVisitor;

impl<'de> Visitor<'de> for GoalPageTextVisitor {
    type Value = GoalPageText;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an object of locale to text")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut texts = Vec::new();
        while let Some((locale, text)) = map.next_entry::<String, String>()? {
            texts.push((locale, text));
        }
        Ok(GoalPageText(texts))
    }
}

/// Workflow #455 RR4b-1: what `GoalPageView` answers, one page of one part. `version` is the goal
/// page's, which `GoalView` answers (the next `EditGoal`'s `expected_version`); `declaration` what
/// the workbench holds of the game's `goals.json`, as `GoalView` says it; `tables` what it holds of
/// `goal-tables.json`, which `GoalView` never says; `read_at_unix_ms` the Runtime's clock at the
/// read; `fresh_until_unix_ms` (`records` and `options`) the earliest expiry of the progress lists
/// read; `records_state` (`records`) the stored `session.goal_records` record; `ladder`
/// (`positions`) the ladder read; then the rows after the cursor, the part's rows in all
/// (`rows_total`), why the page stopped (`cut`) and where the next begins (`next`, exactly when
/// rows remain).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalPageView {
    pub instance_alias: String,
    pub game_id: String,
    pub version: u64,
    pub declaration: GoalDeclarationState,
    pub tables: GoalPageTables,
    pub read_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fresh_until_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub records_state: Option<GoalPageRecordsState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ladder: Option<GoalPageLadder>,
    pub rows: Vec<GoalPageRow>,
    pub rows_total: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut: Option<GoalPageCut>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<GoalPageCursor>,
}

/// Workflow #455 RR4b-1: what the workbench holds of the game root's `goal-tables.json`: whether
/// it read one, its sha256, and for a refused file its first problem with its pointer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalPageTables {
    pub state: GoalTablesRead,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standard_pack_problem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_pointer: Option<String>,
}

/// Workflow #455 RR4b-1: whether the game root's `goal-tables.json` applies: none in the root
/// (`absent`), read and taken (`accepted`), refused by its reader (`refused`), there but not
/// readable (`unreadable`), or not read by this Runtime (`not_read`: no workbench reads game roots).
/// Only an accepted file applies a `records` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalTablesRead {
    Absent,
    Accepted,
    Refused,
    Unreadable,
    NotRead,
}

/// Workflow #455 RR4b-1: the instance's stored `session.goal_records` record as the part `records`
/// read it: none (`absent`), read (`read`), or one its reader does not read (`unreadable`: only a
/// fault of the Runtime writes one, and a `clear` of a records entry rewrites it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalPageRecordsState {
    Absent,
    Read,
    Unreadable,
}

/// Workflow #455 RR4b-1: the ladder the part `positions` read: a class's (`class`, with the class)
/// or a key's own (`own`), and how many positions it has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalPageLadder {
    pub source: GoalLadderSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    pub positions: u16,
}

/// Workflow #455 RR4b-1: where a ladder is written: in the key's class (`class`), or in the key
/// itself (`own`: a column of `per: "key"` or `per: "sub"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalLadderSource {
    Class,
    Own,
}

/// Workflow #455 RR4b-1: why a page stopped before the part's last row: the request's `limit`
/// (`count`), or the row that would have made the page pass [`MAX_GOAL_PAGE_VIEW_BYTES`] (`bytes`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalPageCut {
    Count,
    Bytes,
}

/// Workflow #455 RR4b-1: one row of a page, by its `row` word. The part `records` answers `entry`
/// and `record` rows; `options` its table's `column` rows, then its `class` rows, then its `key`
/// rows, each in the file's order; `positions` one `position` row for each position of the ladder.
/// A `problem` is the word of `goal_problem` a goal edit of it would be refused with now (and
/// `key_repeated` for a key an earlier stored record of the table names); a `name`, `key_name`,
/// `column_name`, `sub_name` or `said` is the pack's text and absent where the pack names nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "row", rename_all = "snake_case", deny_unknown_fields)]
pub enum GoalPageRow {
    /// A records entry: of the root's `goals.json` (`declared`, with the entry before it, any
    /// type, and the entry as its reader read it), or a name the stored records hold that no
    /// records entry declares; whether its table is one of the root's accepted tables (`applies`,
    /// else `problem` `name_unknown`), and how many stored records it holds.
    Entry {
        name: String,
        declared: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after_entry: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entry: Option<Value>,
        applies: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        problem: Option<String>,
        records_total: u16,
    },
    /// A stored record: its entry, its key (and the key's name), its target as stored (`to`, the
    /// canonical object), who set it, whether the computation applies it (else `problem`), the
    /// key's held state as the computation judged it, and one lane for each ladder the
    /// computation walks for it, in the table's order: its columns as the file writes them and,
    /// in a column of `per: "sub"`, its subs as the column writes them; a lane only a
    /// prerequisite raised stands in its column's place.
    Record {
        entry: String,
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key_name: Option<GoalPageText>,
        to: Map<String, Value>,
        set: GoalSetBy,
        applied: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        problem: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        held: Option<GoalKeyHeld>,
        lanes: Vec<GoalPageLane>,
        #[serde(default, skip_serializing_if = "is_false")]
        texts_left_out: bool,
    },
    /// A column of the table: its name, its `per` word, its subs (a column of `per: "sub"`) and
    /// what it requires, in the file's order.
    Column {
        column: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<GoalPageText>,
        per: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        subs: Vec<GoalPageSub>,
        requires: Vec<GoalPageRequirement>,
    },
    /// A class of the table.
    Class {
        class: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<GoalPageText>,
    },
    /// A key of the table: its name, its class, its held state, and each ladder it has with its
    /// size and the key's current position on it.
    Key {
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<GoalPageText>,
        class: String,
        held: GoalKeyHeld,
        lanes: Vec<GoalKeyLane>,
    },
    /// A position of the ladder, by its index, as the pack says it.
    Position {
        index: u16,
        position: Vec<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        said: Option<GoalPageText>,
    },
}

/// Whether a flag is off (absent from the wire).
fn is_false(value: &bool) -> bool {
    !*value
}

/// Workflow #455 RR4b-1: a key's held state, as the table's `held_by` list says it: held (and a
/// table without such a list holds every key, no `fact_key`), not held (the list read whole holds
/// no row of it), or not known (the list gives no reading, with its `unknown_reason`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalKeyHeld {
    pub state: GoalHeldState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact_key: Option<String>,
    #[serde(
        rename = "unknown_reason",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub unknown_word: Option<String>,
}

/// Workflow #455 RR4b-1: the words of [`GoalKeyHeld`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalHeldState {
    Held,
    NotHeld,
    Unknown,
}

/// Workflow #455 RR4b-1: one ladder a stored record's computation walks: its column (and sub) with
/// their names, where the ladder is written, the record's own target on it (absent for a lane only
/// a prerequisite raised), the position a prerequisite of another column takes it to beyond its
/// target (`raised_to`), the key's current position or why it is not known, and the conditions a
/// step the walk takes names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalPageLane {
    pub column: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column_name: Option<GoalPageText>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_name: Option<GoalPageText>,
    pub source: GoalLadderSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<GoalLanePosition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raised_to: Option<GoalLanePosition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<GoalLanePosition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown: Option<GoalLaneUnknown>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<GoalPageCondition>,
}

/// Workflow #455 RR4b-1: a position of a lane's ladder: its index, the position and how the pack
/// says it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalLanePosition {
    pub index: u16,
    pub position: Vec<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<GoalPageText>,
}

/// Workflow #455 RR4b-1: why a key's current position on a ladder is not known: the code the
/// computation notes it with (`goal_records_progress_unknown`, `goal_records_row_missing`,
/// `goal_records_reading_missing` or `goal_records_position_unknown`), with the list's key, the
/// reading column missing and the list's `unknown_reason` where they apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalLaneUnknown {
    pub code: CodeStr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reading: Option<String>,
    #[serde(
        rename = "unknown_reason",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub unknown_word: Option<String>,
}

/// Workflow #455 RR4b-1: a condition (a word of the tables file that is no column's), with its
/// name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalPageCondition {
    pub condition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<GoalPageText>,
}

/// Workflow #455 RR4b-1: a sub of a column of `per: "sub"`, with its name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalPageSub {
    pub sub: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<GoalPageText>,
}

/// Workflow #455 RR4b-1: one prerequisite of a column, as the file writes it: another column at
/// `at_least` (from `from`, a position of the requiring column's own ladder), each position said
/// as the pack says it; or a condition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GoalPageRequirement {
    Column(GoalPageColumnRequirement),
    Condition(GoalPageCondition),
}

/// Workflow #455 RR4b-1: a prerequisite on another column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalPageColumnRequirement {
    pub column: String,
    pub at_least: Vec<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_least_said: Option<GoalPageText>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Vec<i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_said: Option<GoalPageText>,
}

/// Workflow #455 RR4b-1: one ladder a key has (`options`): its column and sub, where it is written,
/// how many positions it has, and the key's current position on it or why it is not known (read
/// only for a key the `held_by` list holds).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalKeyLane {
    pub column: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
    pub source: GoalLadderSource,
    pub positions: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<GoalKeyPosition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown: Option<GoalLaneUnknown>,
}

/// Workflow #455 RR4b-1: a key's current position on a ladder of `options`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalKeyPosition {
    pub index: u16,
    pub position: Vec<i64>,
}
