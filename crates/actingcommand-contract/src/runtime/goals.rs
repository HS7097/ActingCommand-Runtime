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
//!   ([`GoalEdited`]). A value the checks before the gate refuse is `goal_value_invalid` with the
//!   word [`GoalProblem`] gives the situation.

use crate::{EventId, Initiator, ResourceTargetObservation, ResourceTargetView, RuntimeChannel};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

crate::outcome_vocabulary! {
    /// Workflow #463 V3 (vocabulary `goal_problem`): why an `EditGoal` was refused before the fact
    /// write gate (`goal_value_invalid`), one word per situation a reader can tell apart.
    pub enum GoalProblem: "goal_problem" {
        /// The name is no entry of the application's page (no entry of `goals.json`, no variable
        /// of a package the active catalog binds on the application, no remedy parameter of such
        /// a package) and, for a `set`, the value is no other resource target
        /// `{resource, at_least}`; for a `clear`, no stored value and no target has the name.
        NameUnknown => "name_unknown",
        /// The name is a `heading` or a `note` entry of `goals.json`, which holds no value.
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
        /// `value_json` is no JSON text.
        ValueNotJson => "value_not_json",
        /// The value is not of the entry's type, or an object of other members than its own
        /// (`json_pointer` names the member).
        WrongType => "wrong_type",
        /// An `integer` value below `min` or above `max`, or a resource `at_least` below 1 or
        /// above the largest amount (`range_min`, `range_max`).
        OutOfRange => "out_of_range",
        /// An `integer` value within its range but on no step from `min` (`range_min` the
        /// variable's `min`).
        OffStep => "off_step",
        /// A `choice` value, or an item of an `ordered` value, that is none of the declaration's
        /// options (`json_pointer` names the item).
        NotAnOption => "not_an_option",
        /// An `ordered` value that names one key twice (`json_pointer` names the later one).
        KeyRepeated => "key_repeated",
        /// An `ordered` value of fewer than `min_count` or more than `max_count` keys
        /// (`range_min`, `range_max`).
        CountOutOfRange => "count_out_of_range",
        /// The value's canonical text is longer than the store keeps, 1024 bytes (`range_max`).
        ValueTooLarge => "value_too_large",
        /// The pool of a resource value is no pool the application can target (`pool_id`).
        PoolNotTargetable => "pool_not_targetable",
        /// The pool of a resource value is targetable only with its own step or importance, which
        /// a goal by name does not carry: its valuation lacks `scale` or `gap` (`pool_id`).
        PoolUnvalued => "pool_unvalued",
        /// A resource value carries `by_unix_ms`: no reader of this build reads a deadline
        /// (Workflow #455 G-R4 builds it), so none is stored.
        DeadlineUnsupported => "deadline_unsupported",
        /// A `clear` of a name that holds no stored value: nothing to clear.
        ValueNotSet => "value_not_set",
        /// The application's stored record is neither a v2 nor a v3 record the readers read (a v1
        /// policy, or rows the reader refuses): write its whole document with `targets set` first.
        RecordUnreadable => "record_unreadable",
        /// The edited record would hold more targets (16) or entries (96) than a record keeps
        /// (`range_max`).
        RecordOutOfLimits => "record_out_of_limits",
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
/// opaque token compared only for equality), the event of the `fact.published` that holds the
/// record, whether nothing was appended because the record already held the change (`replayed`),
/// and, for a resource value, the id of the target it replaced on the same pool when that target
/// had another id.
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
/// holds its `session.resource_targets` record, `0` without one: the `expected_version` of the
/// next `EditGoal`), what the workbench holds of its game's `goals.json`, the page's entries in
/// their order, the stored values no entry offers any more, the bound packages' remedy parameters
/// (read-only), the ids of the stored targets that are no declared resource entry's, and the
/// resource target view as `ResourceTargetView` answers it.
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
/// a refused file, its first problem (`standard_pack_problem` and its pointer).
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
}

/// Whether the game's `goals.json` is in effect, as the scheduler workbench's last read of the
/// game root holds it: none in the root (`absent`), read and taken (`accepted`), refused by its
/// reader (`refused`: no entry applies for that game; its variables and resource targets still
/// do), there but not readable (`unreadable`: no variable of the root applies, `goals_unreadable`),
/// or not read by this Runtime (`not_read`: no workbench reads the game root, in configured mode,
/// or no check has read it since the start).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalDeclarationRead {
    Absent,
    Accepted,
    Refused,
    Unreadable,
    NotRead,
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
/// reader of this build reads it (`not_built`: a `tier` entry before Workflow #455 G-R5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalEntryState {
    InEffect,
    NotApplied,
    NotBuilt,
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
/// the target that holds its value and the pool's reading.
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
            default_json: None,
            value_json: None,
            effective_json: None,
            removed_keys: Vec::new(),
            source: None,
            set: None,
            state: None,
            variable_problem: None,
            display: None,
            resource: None,
        }
    }
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
