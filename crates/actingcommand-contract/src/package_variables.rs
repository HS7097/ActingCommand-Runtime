// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463 V1 (register §1.378; `contracts/package-variables.md`): a package's declared
//! variables, the optional top-level member `variables` of its `control.json`, beside `provides`.
//! A variable is a named slot the package's policies read at run time and do not choose
//! themselves: "which one" (`choice`), "how many" (`integer`), "on or off" (`switch`) and "in which
//! order" (`ordered`), each with its texts, its bounds and its default, which the package's author
//! or the model's export writes (`default_origin: "model"`).
//!
//! One reader, [`parse_package_variables`], shared by kernel admission and the source parser: 1 to
//! [`MAX_PACKAGE_VARIABLES`] entries read with the value-type code of the goal declaration
//! ([`crate::standard_pack_goals`]'s `Check`, whose four package-governing types this slice moved
//! here, without `governs` and without `from_table`), names unique across the variables and every
//! parameter of the package's `provides`, and none at all on a package that provides an
//! irreversible capability (the line between a parameter and a variable is irreversibility). Every
//! refusal is one [`VariablesViolation`]: its word and the JSON pointer, in `control.json`, of what
//! breaks the rule.
//!
//! The root check, [`check_root_variables`], holds the variables of a game root's packages to each
//! other and to its `goals.json`: a name several packages declare is one variable only when their
//! declarations are byte-identical, no entry of `goals.json` uses it for anything but its
//! placement, the root declares at most [`MAX_ROOT_VARIABLES`] names, no package that counts as
//! able to do something irreversible by the whole root declares it (fix round 1: the rule of
//! [`IrreversiblePackages`], the one the goal check holds a placement to), and a variable a package
//! reads as a designated effect's cap is an `integer` from 0 to at most [`MAX_VARIABLE_CAP`]. A
//! name that breaks one of these is reported and does not apply; the root still runs.
//!
//! Workflow #463 V2 (register §1.382; `contracts/package-variables.md`, "The run"): the rules a
//! value is judged by when a run reads it, one copy for the policy crate, the kernel and the Lab:
//! [`PackageVariable::judge`] (a value set on an application that no longer fits the declaration
//! gives the word [`VariableProblem`] says it with, and the run reads the default),
//! [`PackageVariable::fact_value`] (the overlay `session.dispatch.parameter.<name>` with its
//! kind's type) and [`variables_not_applying`] (the names the root check keeps from applying,
//! with their word). A package without `variables` reads as before. Fix round 2: the one
//! judgement of a whole root, [`judge_root_variables`], takes the goal document as a reader read
//! it ([`RootGoalDocument`]: absent, accepted, refused or unreadable) and answers, per name, the
//! packages that declare it, its declaration, and why it does not apply with the detail of the
//! cause ([`RootVariableVerdict`]); `standard-pack check`, the Runtime's scheduler workbench and
//! the Runtime's table without a composed standard pack all call it.

use crate::standard_pack::{StandardPackProblem, StandardPackRootIssue};
use crate::standard_pack_goals::{
    Check, DeclaredGoal, GoalBlock, GoalOption, GoalText, GoalTimeUnit, GoalType, GoalUnit,
    GoalVariable, IrreversiblePackages, MAX_GOAL_CHOICE_OPTIONS, MAX_GOAL_ENTRIES,
    MAX_GOAL_LIST_VALUES, MAX_GOAL_NAME_BYTES, MAX_GOAL_ORDERED_COUNT, MIN_GOAL_CHOICE_OPTIONS,
    StandardPackGoals, bounded,
};
use crate::standard_pack_ui::{MAX_PACK_UI_TITLE_BYTES, at};
use crate::{
    FactScalar, FactValue, MAX_DESIGNATED_EFFECT_REPEATS, ProvidedCapability, remedy_token_valid,
};
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// A package's control declaration, whose `variables` this module reads.
pub const PACKAGE_CONTROL_FILE: &str = "control.json";
/// The pointer of the member in `control.json`.
pub const VARIABLES_POINTER: &str = "/variables";
/// The `package` interface revision a bundle whose packages declare variables declares,
/// `[14, 14]` (`contracts/component-interfaces.md`, the `package` row 14; row 13 is Workflow #463
/// K1's, a `linear_steps` task with a select step).
pub const VARIABLES_PACKAGE_REVISION: u64 = 14;
/// The most variables one package declares.
pub const MAX_PACKAGE_VARIABLES: usize = 16;
/// The most variable names of one game root: the entry rows an application's goal record keeps
/// (`contracts/resource-targets.md`), as many as a `goals.json` may hold entries.
pub const MAX_ROOT_VARIABLES: usize = MAX_GOAL_ENTRIES;
/// The largest `max` of an `integer` variable a package reads as a designated effect's cap: the
/// highest cap a designated effect takes.
pub const MAX_VARIABLE_CAP: i64 = MAX_DESIGNATED_EFFECT_REPEATS as i64;
/// The key prefix under which a run reads a variable (Workflow #463 V2), the one remedy parameters
/// use (`contracts/remedies.md` §4); the root check reads it to know which variable a designated
/// effect's cap reads.
pub const VARIABLE_FACT_KEY_PREFIX: &str = "session.dispatch.parameter.";
/// Workflow #463 V2: the longest text of a value a dispatch's reason carries
/// (`variable_values[].value_json`). Fix round 1: a `json` key of the key table, which holds the
/// longest value text the goal store keeps for an entry, so every value a run is to read of a set
/// value fits; only a declared default may be longer.
pub const MAX_VARIABLE_VALUE_JSON_BYTES: usize = crate::outcome::JSON_MAX_BYTES;
/// Workflow #463 V2: the key column of the rows an `ordered` variable is overlaid with.
pub const ORDERED_VARIABLE_KEY_COLUMN: &str = "key";
/// Workflow #463 V2: the column of the rows an `ordered` variable is overlaid with that holds each
/// key's place in the list, 1 for the first.
pub const ORDERED_VARIABLE_RANK_COLUMN: &str = "rank";

crate::outcome_vocabulary! {
    /// Workflow #463 V2 (vocabulary `variable_source`): where the value a policy run reads of a
    /// variable comes from.
    pub enum VariableSource: "variable_source" {
        /// The application's entry row of the variable's name in its `session.resource_targets`
        /// v3 record, the value a person or an agent set.
        Set => "set",
        /// The package's declared default.
        Default => "default",
    }
}

crate::outcome_vocabulary! {
    /// Workflow #463 V2 (vocabulary `variable_problem`): why a value set on an application was
    /// not handed to a run, which reads the package's default instead; one word per situation.
    /// Fix round 1: and `keys_left_out`, an `ordered` value handed to the run in part.
    pub enum VariableProblem: "variable_problem" {
        /// A `choice` value that is none of the declaration's options.
        NotAnOption => "not_an_option",
        /// An `integer` value below `min` or above `max`.
        OutOfRange => "out_of_range",
        /// An `integer` value within the range but on no step from `min`.
        OffStep => "off_step",
        /// A value that is not of the variable's type.
        TypeChanged => "type_changed",
        /// An `ordered` value whose keys the declaration still offers are fewer than `min_count`.
        KeysRemoved => "keys_removed",
        /// An `ordered` value of offered keys only whose count is outside `min_count` to
        /// `max_count`, or whose offered keys are more than `max_count`.
        CountOutOfRange => "count_out_of_range",
        /// An `ordered` value that names one key twice.
        KeyRepeated => "key_repeated",
        /// Fix round 1: an `ordered` value naming keys the declaration no longer offers, whose
        /// offered keys are enough: the run reads those, in their order (the source stays
        /// `set`), and no warning is written, since the value was applied.
        KeysLeftOut => "keys_left_out",
        /// Fix round 1: the application's entry row of the name holds a value but who set it
        /// (its `set_unix_ms` or its `channel`) could not be read.
        EntryUnreadable => "entry_unreadable",
        /// A package that declares the variable counts as able to do something irreversible by
        /// the root (`variable_irreversible_by_root`) because it provides, or its remedies list,
        /// a capability a package of the root marks irreversible.
        IrreversibleByRoot => "irreversible_by_root",
        /// Fix round 1: a package of the game root could not be read, so the root check counts
        /// every package of the root as able to do something irreversible (its
        /// `variable_irreversible_by_root`, the safe side): no package that declares the variable
        /// provides, or lists, a capability marked irreversible.
        PackageUnreadable => "package_unreadable",
        /// Fix round 1: the game root's `goals.json` could not be read, so which names it takes
        /// is not known.
        GoalsUnreadable => "goals_unreadable",
        /// The root's declarations of the name are not byte-identical (`variable_conflict`).
        DeclarationConflict => "declaration_conflict",
        /// An entry of the root's `goals.json` uses the name for anything but its placement
        /// (`variable_conflict` at the name).
        GoalNameConflict => "goal_name_conflict",
        /// The name is the root's 97th or later (`variable_out_of_limits`).
        NameOutOfLimits => "name_out_of_limits",
        /// A package reads the variable as a designated effect's cap and it is no `integer` from
        /// 0 to 1000 (`variable_cap_range`).
        CapRange => "cap_range",
        /// No root check judged the variable for the application's game: without a composed
        /// standard pack, the Runtime's table was built before an instance of that game was
        /// known.
        RootUnchecked => "root_unchecked",
    }
}

/// The type of a variable (`type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VariableType {
    /// One of 2 to 32 options.
    Choice,
    /// A whole number within a range, on a step.
    Integer,
    /// On or off.
    Switch,
    /// An ordered list of 0 to 32 of 1 to 1024 options (a set when the package ignores the
    /// order; one pick among many when `min_count` and `max_count` are 1).
    Ordered,
}

impl VariableType {
    /// The four types, in the contract's order.
    pub const ALL: [Self; 4] = [Self::Choice, Self::Integer, Self::Switch, Self::Ordered];

    /// The type's word in `control.json`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Choice => "choice",
            Self::Integer => "integer",
            Self::Switch => "switch",
            Self::Ordered => "ordered",
        }
    }

    /// The type a word names; `None` for any other word.
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|variable_type| variable_type.as_str() == word)
    }

    /// The block every reader shows the type with: the only block version 2 of `goals.json` gives
    /// it, version 1's.
    pub const fn base_block(self) -> GoalBlock {
        match self {
            Self::Choice => GoalBlock::Choice,
            Self::Integer => GoalBlock::Number,
            Self::Switch => GoalBlock::Switch,
            Self::Ordered => GoalBlock::OrderedList,
        }
    }

    /// The members a variable of the type must hold.
    const fn required(self) -> &'static [&'static str] {
        match self {
            Self::Choice => CHOICE_REQUIRED,
            Self::Integer => INTEGER_REQUIRED,
            Self::Switch => SWITCH_REQUIRED,
            Self::Ordered => ORDERED_REQUIRED,
        }
    }

    /// The members a variable of the type may hold besides the required ones.
    const fn optional(self) -> &'static [&'static str] {
        match self {
            Self::Integer => INTEGER_OPTIONAL,
            Self::Choice | Self::Switch | Self::Ordered => COMMON_OPTIONAL,
        }
    }
}

const CHOICE_REQUIRED: &[&str] = &["name", "type", "title", "options", "default"];
const INTEGER_REQUIRED: &[&str] = &["name", "type", "title", "min", "max", "default"];
const SWITCH_REQUIRED: &[&str] = &["name", "type", "title", "default"];
const ORDERED_REQUIRED: &[&str] = &[
    "name",
    "type",
    "title",
    "options",
    "min_count",
    "max_count",
    "default",
];
const COMMON_OPTIONAL: &[&str] = &["block", "help", "default_origin"];
const INTEGER_OPTIONAL: &[&str] = &["block", "help", "step", "unit", "default_origin"];

/// Who wrote a variable's default: the package's author (`pack`, no `default_origin`) or the
/// model's export (`default_origin: "model"`). It decides nothing; only the provenance listing
/// of `standard-pack check` reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DefaultOrigin {
    Pack,
    Model,
}

impl DefaultOrigin {
    /// The origin's word: `model` is the one `default_origin` may write; `pack` is its absence.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pack => "pack",
            Self::Model => "model",
        }
    }
}

/// One declared variable: its name (the Lab's name and, in a later slice, the run key's last
/// segment), the block it is shown with, its texts, its type with the type's members, the origin
/// of its default, and the entry as written, which the root check compares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageVariable {
    pub name: String,
    /// The block written, or the type's base block when the entry writes none.
    pub block: GoalBlock,
    pub title: GoalText,
    pub help: Option<GoalText>,
    pub kind: VariableKind,
    pub default_origin: DefaultOrigin,
    /// The entry of `variables` as written.
    pub declaration: Value,
}

impl PackageVariable {
    pub const fn variable_type(&self) -> VariableType {
        self.kind.variable_type()
    }

    /// The fact key a run reads the variable's value under (Workflow #463 V2), and a designated
    /// effect's cap names to read it: [`VARIABLE_FACT_KEY_PREFIX`] and the name.
    pub fn fact_key(&self) -> String {
        format!("{VARIABLE_FACT_KEY_PREFIX}{}", self.name)
    }

    /// Workflow #463 V2: the declared default as a JSON value: a string, an integer, a boolean
    /// or a list of option values.
    pub fn default_value(&self) -> Value {
        match &self.kind {
            VariableKind::Choice { default, .. } => Value::String(default.clone()),
            VariableKind::Integer { default, .. } => Value::from(*default),
            VariableKind::Switch { default } => Value::Bool(*default),
            VariableKind::Ordered { default, .. } => {
                Value::Array(default.iter().cloned().map(Value::String).collect())
            }
        }
    }

    /// Workflow #463 V2: `sha256:<hex>` of the entry as written, in compact JSON: a changed
    /// declaration of the name has another one, so `goal_value_not_applied` is written once per
    /// (application, name, this).
    pub fn declaration_sha256(&self) -> String {
        let compact = serde_json::to_vec(&self.declaration).unwrap_or_default();
        format!("sha256:{:x}", Sha256::digest(compact))
    }

    /// Workflow #463 V2 (`contracts/package-variables.md`, "A set value that no longer fits"):
    /// `value`, set on an application, judged against this declaration when a run is to read it.
    /// The value the run reads, or the word that says why it reads the default instead:
    ///
    /// - `choice`: a string (else `type_changed`) that is one of the options (else
    ///   `not_an_option`);
    /// - `integer`: a whole number in the safe range (else `type_changed`) from `min` to `max`
    ///   (else `out_of_range`) on a step from `min` (else `off_step`);
    /// - `switch`: a boolean (else `type_changed`);
    /// - `ordered`: a list of strings (else `type_changed`), each once (else `key_repeated`); the
    ///   keys the declaration no longer offers are left out and the rest is read in its order;
    ///   fewer than `min_count` left is `keys_removed` when a key was left out and
    ///   `count_out_of_range` when none was, more than `max_count` is `count_out_of_range`.
    ///
    /// The stored value is never rewritten: this only says what one run reads. Fix round 1: what
    /// a run reads of an `ordered` value whose keys some were left out of says so
    /// ([`Self::leaves_keys_out`]).
    pub fn judge(&self, value: &Value) -> Result<Value, VariableProblem> {
        match &self.kind {
            VariableKind::Choice { options, .. } => {
                let Value::String(chosen) = value else {
                    return Err(VariableProblem::TypeChanged);
                };
                if options.iter().any(|option| option.value == *chosen) {
                    Ok(value.clone())
                } else {
                    Err(VariableProblem::NotAnOption)
                }
            }
            VariableKind::Integer { min, max, step, .. } => {
                let Some(number) = value.as_i64() else {
                    return Err(VariableProblem::TypeChanged);
                };
                if !(*min..=*max).contains(&number) {
                    Err(VariableProblem::OutOfRange)
                } else if (i128::from(number) - i128::from(*min)) % i128::from(*step) != 0 {
                    Err(VariableProblem::OffStep)
                } else {
                    Ok(Value::from(number))
                }
            }
            VariableKind::Switch { .. } => match value {
                Value::Bool(_) => Ok(value.clone()),
                _ => Err(VariableProblem::TypeChanged),
            },
            VariableKind::Ordered {
                options,
                min_count,
                max_count,
                ..
            } => {
                let Value::Array(items) = value else {
                    return Err(VariableProblem::TypeChanged);
                };
                let mut seen = BTreeSet::new();
                for item in items {
                    let Value::String(key) = item else {
                        return Err(VariableProblem::TypeChanged);
                    };
                    if !seen.insert(key.as_str()) {
                        return Err(VariableProblem::KeyRepeated);
                    }
                }
                let offered = items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|key| options.iter().any(|option| option.value == *key))
                    .collect::<Vec<_>>();
                let count = offered.len() as u64;
                if count < *min_count {
                    return Err(if offered.len() < items.len() {
                        VariableProblem::KeysRemoved
                    } else {
                        VariableProblem::CountOutOfRange
                    });
                }
                if count > *max_count {
                    return Err(VariableProblem::CountOutOfRange);
                }
                Ok(Value::Array(
                    offered
                        .into_iter()
                        .map(|key| Value::String(key.to_owned()))
                        .collect(),
                ))
            }
        }
    }

    /// Workflow #463 V2 fix round 1 (`contracts/package-variables.md`, "A set value that no longer
    /// fits"): whether `read`, what [`Self::judge`] let through of the set value `set`, leaves keys
    /// of it out: an `ordered` variable whose declaration no longer offers some keys of the set
    /// list. The run reads `read`; the record says `keys_left_out`. Never for another kind, whose
    /// judgement lets a value through whole or not at all.
    pub fn leaves_keys_out(&self, set: &Value, read: &Value) -> bool {
        matches!(self.kind, VariableKind::Ordered { .. }) && set != read
    }

    /// Workflow #463 V2 (`contracts/package-variables.md`, "The run"): the value of the overlay
    /// record `session.dispatch.parameter.<name>` that holds `value` (the default, or what
    /// [`Self::judge`] let through): a `choice` a string, a `switch` a boolean, an `integer` an
    /// integer, an `ordered` variable a record list of one row `{key, rank}` per key in its
    /// order, `rank` from 1. `None` for a value not of the variable's type.
    pub fn fact_value(&self, value: &Value) -> Option<FactValue> {
        match &self.kind {
            VariableKind::Choice { .. } => value
                .as_str()
                .map(|text| FactValue::String(text.to_owned())),
            VariableKind::Integer { .. } => value.as_i64().map(FactValue::Integer),
            VariableKind::Switch { .. } => value.as_bool().map(FactValue::Boolean),
            VariableKind::Ordered { .. } => value
                .as_array()?
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let rank = i64::try_from(index).ok()?.checked_add(1)?;
                    Some(BTreeMap::from([
                        (
                            ORDERED_VARIABLE_KEY_COLUMN.to_owned(),
                            FactScalar::String(item.as_str()?.to_owned()),
                        ),
                        (
                            ORDERED_VARIABLE_RANK_COLUMN.to_owned(),
                            FactScalar::Integer(rank),
                        ),
                    ]))
                })
                .collect::<Option<Vec<_>>>()
                .map(FactValue::RecordList),
        }
    }
}

/// Workflow #463 V2: the text a dispatch's reason carries a variable's value as (`value_json`):
/// compact JSON, which for the values a run reads (option values, whole numbers, booleans and
/// lists of option values) is their one spelling.
pub fn variable_value_json(value: &Value) -> String {
    value.to_string()
}

/// A variable's type with its members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VariableKind {
    Choice {
        options: Vec<GoalOption>,
        default: String,
    },
    /// `min`, `max`, `step` (1 when the entry writes none), `unit` and `default`.
    Integer {
        min: i64,
        max: i64,
        step: i64,
        unit: Option<GoalUnit>,
        default: i64,
    },
    Switch {
        default: bool,
    },
    Ordered {
        options: Vec<GoalOption>,
        min_count: u64,
        max_count: u64,
        default: Vec<String>,
    },
}

impl VariableKind {
    pub const fn variable_type(&self) -> VariableType {
        match self {
            Self::Choice { .. } => VariableType::Choice,
            Self::Integer { .. } => VariableType::Integer,
            Self::Switch { .. } => VariableType::Switch,
            Self::Ordered { .. } => VariableType::Ordered,
        }
    }
}

/// The rule a refused `variables` breaks: the `reason` of kernel admission's
/// `contained_task_control_invalid`, and the word of the source parser's
/// `resource_declaration_invalid` message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariablesProblem {
    /// The list, an entry or a member breaks its shape, a limit, a range or its default.
    VariablesInvalid,
    /// An entry's name is an earlier variable's or a parameter's of the package's `provides`.
    VariableNameTaken,
    /// The package provides an irreversible capability, so it declares no variable.
    VariablesOnIrreversible,
}

impl VariablesProblem {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VariablesInvalid => "variables_invalid",
            Self::VariableNameTaken => "variable_name_taken",
            Self::VariablesOnIrreversible => "variables_on_irreversible",
        }
    }
}

/// A refused `variables`: the rule it breaks and the JSON pointer, in `control.json`, of the
/// member that breaks it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariablesViolation {
    pub problem: VariablesProblem,
    pub pointer: String,
}

impl VariablesViolation {
    fn new(problem: VariablesProblem, pointer: impl Into<String>) -> Self {
        Self {
            problem,
            pointer: pointer.into(),
        }
    }
}

/// The member `variables` (`value`) of a control declaration whose `provides` is `provides`
/// (already read; empty when it declares none), in declaration order. The first rule broken, in
/// this order, refuses it:
///
/// - `variables_on_irreversible`, at `/variables`: a capability of `provides` is irreversible;
/// - `variables_invalid`: not a list of 1 to [`MAX_PACKAGE_VARIABLES`] entries (at
///   `/variables`); then, entry by entry, the first problem of the goal declaration's value-type
///   rules at its pointer: an entry that is no object, a member that is unknown, missing or not of
///   its type, a name outside `^[a-z][a-z0-9_]{0,63}$`, a type that is none of the four, a block
///   that is not the type's, a text, a range, a step, an option value outside the name's form or
///   written twice, a count, a default that is no value of the declaration, a `default_origin`
///   other than `"model"`;
/// - `variable_name_taken`, at `/variables/<i>/name`, once the entry reads: a name an earlier
///   variable or any parameter of `provides` has.
pub fn parse_package_variables(
    value: &Value,
    provides: &[ProvidedCapability],
) -> Result<Vec<PackageVariable>, VariablesViolation> {
    if provides.iter().any(ProvidedCapability::irreversible) {
        return Err(VariablesViolation::new(
            VariablesProblem::VariablesOnIrreversible,
            VARIABLES_POINTER,
        ));
    }
    let mut taken = provides
        .iter()
        .flat_map(ProvidedCapability::parameters)
        .map(|parameter| parameter.name().to_owned())
        .collect::<BTreeSet<_>>();
    let mut check = Check::new(PACKAGE_CONTROL_FILE);
    let items = check.list(value, VARIABLES_POINTER, (1, MAX_PACKAGE_VARIABLES));
    if let Some(violation) = first_refusal(&check) {
        return Err(violation);
    }
    let items = items.ok_or_else(|| {
        VariablesViolation::new(VariablesProblem::VariablesInvalid, VARIABLES_POINTER)
    })?;
    let mut variables = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let pointer = format!("{VARIABLES_POINTER}/{index}");
        let variable = check.variable(item, &pointer);
        if let Some(violation) = first_refusal(&check) {
            return Err(violation);
        }
        // Every path that reads no variable records why; this one would hide a refusal.
        let variable = variable.ok_or_else(|| {
            VariablesViolation::new(VariablesProblem::VariablesInvalid, pointer.as_str())
        })?;
        if !taken.insert(variable.name.clone()) {
            return Err(VariablesViolation::new(
                VariablesProblem::VariableNameTaken,
                at(&pointer, "name"),
            ));
        }
        variables.push(variable);
    }
    Ok(variables)
}

/// The first issue `check` found, as the refusal `variables_invalid` at its pointer.
fn first_refusal(check: &Check) -> Option<VariablesViolation> {
    check.issues().first().map(|issue| {
        VariablesViolation::new(VariablesProblem::VariablesInvalid, issue.pointer.as_str())
    })
}

impl Check {
    /// One entry of `variables`: its name, its type, its members, its block, its texts, its
    /// type's members and the origin of its default. An entry whose type is unknown is not read
    /// further.
    fn variable(&mut self, value: &Value, pointer: &str) -> Option<PackageVariable> {
        let Some(object) = value.as_object() else {
            self.malformed(pointer);
            return None;
        };
        let name = self.variable_name(object.get("name"), pointer);
        let at_type = at(pointer, "type");
        let Some(variable_type) = object
            .get("type")
            .and_then(Value::as_str)
            .and_then(VariableType::from_word)
        else {
            self.malformed(at_type);
            return None;
        };
        self.members(
            object,
            pointer,
            variable_type.required(),
            variable_type.optional(),
        );
        let block = self
            .block(
                object.get("block"),
                pointer,
                Some(variable_type.base_block()),
            )
            .flatten();
        let texts = self.texts(object, pointer);
        let kind = self.variable_kind(variable_type, object, pointer);
        let default_origin = match object.get("default_origin") {
            None => Some(DefaultOrigin::Pack),
            Some(Value::String(word)) if word == DefaultOrigin::Model.as_str() => {
                Some(DefaultOrigin::Model)
            }
            Some(_) => {
                self.malformed(at(pointer, "default_origin"));
                None
            }
        };
        let (title, help) = texts?;
        Some(PackageVariable {
            name: name?,
            block: block?,
            title,
            help,
            kind: kind?,
            default_origin: default_origin?,
            declaration: value.clone(),
        })
    }

    /// A variable's name: 1 to 64 bytes of `[a-z][a-z0-9_]*`, the remedy token form, so that it
    /// can be a parameter's name and a goal entry's name ([`bounded`]). A missing name is reported
    /// with the entry's members.
    fn variable_name(&mut self, value: Option<&Value>, pointer: &str) -> Option<String> {
        let at_name = at(pointer, "name");
        let Value::String(name) = value? else {
            self.malformed(at_name);
            return None;
        };
        if let Some(problem) = bounded(name, MAX_GOAL_NAME_BYTES, remedy_token_valid) {
            self.issue(problem, at_name);
            return None;
        }
        Some(name.clone())
    }

    /// The value-type members of a variable of `variable_type` (the code of the goal
    /// declaration's `integer`, `choice`, `switch` and `ordered` entries, moved here by Workflow
    /// #463 V1 without `governs` and without `from_table`).
    fn variable_kind(
        &mut self,
        variable_type: VariableType,
        object: &Map<String, Value>,
        pointer: &str,
    ) -> Option<VariableKind> {
        match variable_type {
            VariableType::Choice => {
                let options = object.get("options").and_then(|options| {
                    self.variable_options(
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
                Some(VariableKind::Choice {
                    options: options?,
                    default: default?,
                })
            }
            VariableType::Integer => self.integer_variable(object, pointer),
            VariableType::Switch => match object.get("default") {
                Some(Value::Bool(default)) => Some(VariableKind::Switch { default: *default }),
                Some(_) => {
                    self.issue(
                        StandardPackProblem::GoalDefaultInvalid,
                        at(pointer, "default"),
                    );
                    None
                }
                None => None,
            },
            VariableType::Ordered => self.ordered_variable(object, pointer),
        }
    }

    fn integer_variable(
        &mut self,
        object: &Map<String, Value>,
        pointer: &str,
    ) -> Option<VariableKind> {
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
        Some(VariableKind::Integer {
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

    /// `options`: `bounds` options `{value, title}`, each value of the token form written once,
    /// so that every option can be a member of an `enum_string` fact and an identity id.
    fn variable_options(
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
            let value = option.get("value").and_then(|value| {
                self.option_value(
                    value,
                    &at(&at_option, "value"),
                    &mut seen,
                    remedy_token_valid,
                )
            });
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

    fn ordered_variable(
        &mut self,
        object: &Map<String, Value>,
        pointer: &str,
    ) -> Option<VariableKind> {
        let options = object.get("options").and_then(|options| {
            self.variable_options(options, &at(pointer, "options"), (1, MAX_GOAL_LIST_VALUES))
        });
        let min_count = object
            .get("min_count")
            .and_then(|count| self.count(count, &at(pointer, "min_count")));
        let max_count = object
            .get("max_count")
            .and_then(|count| self.count(count, &at(pointer, "max_count")));
        let counts = match (min_count, max_count) {
            (Some(min_count), Some(max_count)) => {
                let offered = options.as_ref().map(|options| options.len() as u64);
                if max_count == 0 || max_count > MAX_GOAL_ORDERED_COUNT {
                    self.issue(
                        StandardPackProblem::GoalRangeInvalid,
                        at(pointer, "max_count"),
                    );
                    None
                } else if min_count > max_count
                    || offered.is_some_and(|offered| min_count > offered)
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
        let default = match (counts, &options, object.get("default")) {
            (Some(counts), Some(options), Some(default)) => {
                self.ordered_default(default, &at(pointer, "default"), counts, options)
            }
            _ => None,
        };
        let (min_count, max_count) = counts?;
        Some(VariableKind::Ordered {
            options: options?,
            min_count,
            max_count,
            default: default?,
        })
    }

    /// An `ordered` variable's `default`: a list of its option values, each once, `counts` long.
    fn ordered_default(
        &mut self,
        value: &Value,
        pointer: &str,
        (min_count, max_count): (u64, u64),
        options: &[GoalOption],
    ) -> Option<Vec<String>> {
        let Some(items) = value.as_array() else {
            self.issue(StandardPackProblem::GoalDefaultInvalid, pointer);
            return None;
        };
        let mut seen = BTreeSet::new();
        let mut chosen = Vec::new();
        let mut complete = true;
        for (index, item) in items.iter().enumerate() {
            match item.as_str() {
                Some(key)
                    if options.iter().any(|option| option.value == key)
                        && seen.insert(key.to_owned()) =>
                {
                    chosen.push(key.to_owned());
                }
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
}

/// One package of a game root as the root check reads it: its id, the variables its control
/// declaration declares (admitted) and the fact keys its designated effects read their caps from
/// (`scheduling_outcome.designated_effects[].max_repeats.fact_key`), in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootPackageVariables {
    pub package_id: String,
    pub variables: Vec<PackageVariable>,
    pub cap_fact_keys: Vec<String>,
}

/// A problem of the root check: the package whose declaration it is at, and the issue, of
/// [`PACKAGE_CONTROL_FILE`], at its pointer in that package's control declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootVariableIssue {
    pub package_id: String,
    pub issue: StandardPackRootIssue,
}

/// A variable that applies in the game root: its name, every package that declares it (in
/// registry order) and the origin of its default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedVariable {
    pub name: String,
    pub packages: Vec<String>,
    pub default_origin: DefaultOrigin,
}

/// What the root check found: the variables that apply, each once in the order the root first
/// declares it; the problems, name by name in that order, each declaration in registry order;
/// and (fix round 1) the variables that do not apply because a package that counts as able to do
/// something irreversible by the whole root declares them, each once in that order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RootVariables {
    pub applied: Vec<AppliedVariable>,
    pub issues: Vec<RootVariableIssue>,
    pub not_applying: Vec<NotApplyingVariable>,
}

impl RootVariables {
    /// The provenance listing of `standard-pack check`: each variable that applies by who wrote
    /// its default, in the order of [`Self::applied`], and (fix round 1) each variable of
    /// [`Self::not_applying`] with the word that says why it does not apply.
    pub fn provenance(&self) -> VariableOrigins {
        let mut origins = VariableOrigins {
            not_applying: self.not_applying.clone(),
            ..VariableOrigins::default()
        };
        for variable in &self.applied {
            match variable.default_origin {
                DefaultOrigin::Pack => origins.pack.push(variable.name.clone()),
                DefaultOrigin::Model => origins.model.push(variable.name.clone()),
            }
        }
        origins
    }
}

/// The names of a game root's variables by who wrote their default, each list in the root's
/// order: no `default_origin` (`pack`), or `"model"` (`model`); and (fix round 1) under
/// `not_applying`, absent when empty, each variable that a package which counts as able to do
/// something irreversible by the whole root declares, which does not apply although its
/// declarations agree.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct VariableOrigins {
    pub pack: Vec<String>,
    pub model: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub not_applying: Vec<NotApplyingVariable>,
}

/// Fix round 1: a variable of the provenance listing that does not apply, by its name, and the
/// word of the root check's warning that says why (`variable_irreversible_by_root`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NotApplyingVariable {
    pub name: String,
    #[serde(serialize_with = "problem_word")]
    pub standard_pack_problem: StandardPackProblem,
}

/// A problem as its word.
fn problem_word<S: serde::Serializer>(
    problem: &StandardPackProblem,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(problem.as_str())
}

/// The root check of the variables `packages` declare (the root's packages in registry order),
/// against `goal_names`, the names an accepted `goals.json` uses for anything but a placement
/// ([`crate::StandardPackGoals::names_besides_placements`]; none without one). Name by name, in the
/// order the root first declares it, the first of these holds and the name does not apply:
///
/// - `variable_out_of_limits`: the name is the root's 97th or later, at each declaration's `name`;
/// - `variable_conflict`: the declarations of the name are not byte-identical (compared as written,
///   member order included, in compact JSON), at each declaration; or an entry of `goals.json`
///   uses the name, at each declaration's `name`;
/// - `variable_irreversible_by_root` (fix round 1): a package that declares it counts as able to
///   do something irreversible by the whole root, as `irreversible` judges it
///   ([`IrreversiblePackages::counts_irreversible`], the one copy of the rule the goal check holds
///   a placement to; a package it was not told of counts), at each such package's declaration;
///   the name is listed in [`RootVariables::not_applying`];
/// - `variable_cap_range`: a package that declares it reads a designated effect's cap from its
///   fact key ([`PackageVariable::fact_key`]) and the variable is no `integer` from 0 to at most
///   [`MAX_VARIABLE_CAP`], at that package's declaration's `type` (fix round 1: another type; a
///   cap is read from an `integer` only), `min` (fix round 1: below 0) or `max` (above
///   [`MAX_VARIABLE_CAP`]), the first of these that holds.
pub fn check_root_variables(
    packages: &[RootPackageVariables],
    goal_names: &BTreeSet<&str>,
    irreversible: &IrreversiblePackages,
) -> RootVariables {
    let root = RootDeclarations::of(packages);
    let mut checked = RootVariables::default();
    for (position, name) in root.order.iter().enumerate() {
        let Some(declarations) = root.declared.get(name) else {
            continue;
        };
        let Some(&first_at) = declarations.first() else {
            continue;
        };
        let first = variable_at(packages, first_at);
        let Some(refusal) = refusal_of(
            packages,
            position,
            name,
            declarations,
            first,
            goal_names,
            irreversible,
        ) else {
            checked.applied.push(AppliedVariable {
                name: (*name).to_owned(),
                packages: declarations
                    .iter()
                    .map(|(package, _)| packages[*package].package_id.clone())
                    .collect(),
                default_origin: first.default_origin,
            });
            continue;
        };
        // Fix round 1: the provenance listing marks such a variable as not applying, with why.
        if refusal.problem == StandardPackProblem::VariableIrreversibleByRoot {
            checked.not_applying.push(NotApplyingVariable {
                name: (*name).to_owned(),
                standard_pack_problem: refusal.problem,
            });
        }
        for (package, variable) in refusal.declarations {
            let at_variable = format!("{VARIABLES_POINTER}/{variable}");
            let pointer = match refusal.member {
                Some(member) => at(&at_variable, member),
                None => at_variable,
            };
            checked.issues.push(RootVariableIssue {
                package_id: packages[package].package_id.clone(),
                issue: StandardPackRootIssue::new(PACKAGE_CONTROL_FILE, refusal.problem, pointer),
            });
        }
    }
    checked
}

/// Workflow #463 V2 (`contracts/package-variables.md`, "A variable that does not apply"): each
/// name of `packages` that the root check `checked` ([`check_root_variables`] of the same
/// packages) keeps from applying, with the word a run's record and `goal_value_not_applied` say
/// it with: `variable_out_of_limits` is `name_out_of_limits`, `variable_conflict` at a
/// declaration `declaration_conflict` and at a name (a `goals.json` entry's) `goal_name_conflict`,
/// `variable_irreversible_by_root` `irreversible_by_root` and `variable_cap_range` `cap_range`.
/// The check reports one problem per name, so a name has one word. It reads the check's answer
/// only: the rules stay [`check_root_variables`]'s.
///
/// Fix round 1 (one word, one cause): `variable_irreversible_by_root` arises in two situations
/// that `irreversible` (the one the check was given) tells apart. A declaring package that
/// provides, or whose remedies list, a capability a package of the root marks irreversible
/// ([`IrreversiblePackages::package`] `Some(true)`) is `irreversible_by_root`; otherwise the check
/// counted the packages irreversible because a package of the root could not be read
/// ([`IrreversiblePackages::unreadable`]), or because a declaring package is none it was told of:
/// `package_unreadable`.
pub fn variables_not_applying(
    packages: &[RootPackageVariables],
    checked: &RootVariables,
    irreversible: &IrreversiblePackages,
) -> BTreeMap<String, VariableProblem> {
    let mut not_applying = BTreeMap::new();
    for issue in &checked.issues {
        let Some(package) = packages
            .iter()
            .find(|package| package.package_id == issue.package_id)
        else {
            continue;
        };
        let Some(rest) = issue
            .issue
            .pointer
            .strip_prefix(VARIABLES_POINTER)
            .and_then(|rest| rest.strip_prefix('/'))
        else {
            continue;
        };
        let (index, member) = match rest.split_once('/') {
            Some((index, member)) => (index, Some(member)),
            None => (rest, None),
        };
        let Some(variable) = index
            .parse::<usize>()
            .ok()
            .and_then(|index| package.variables.get(index))
        else {
            continue;
        };
        let word = match (issue.issue.problem, member) {
            (StandardPackProblem::VariableOutOfLimits, _) => VariableProblem::NameOutOfLimits,
            (StandardPackProblem::VariableConflict, Some("name")) => {
                VariableProblem::GoalNameConflict
            }
            (StandardPackProblem::VariableConflict, _) => VariableProblem::DeclarationConflict,
            (StandardPackProblem::VariableIrreversibleByRoot, _) => {
                if irreversible.package(&issue.package_id) == Some(true) {
                    VariableProblem::IrreversibleByRoot
                } else {
                    VariableProblem::PackageUnreadable
                }
            }
            (StandardPackProblem::VariableCapRange, _) => VariableProblem::CapRange,
            _ => continue,
        };
        // Fix round 1: one declaring package that can do something irreversible by itself is
        // the name's cause, whatever an unreadable package adds.
        let kept = not_applying.entry(variable.name.clone()).or_insert(word);
        if *kept == VariableProblem::PackageUnreadable
            && word == VariableProblem::IrreversibleByRoot
        {
            *kept = word;
        }
    }
    not_applying
}

/// Why a name of the root does not apply, at which member of which declarations, each
/// `(package index, variable index)`.
struct Refusal {
    problem: StandardPackProblem,
    /// The member of each declaration the problem is at; `None` for the declaration itself.
    member: Option<&'static str>,
    declarations: Vec<(usize, usize)>,
}

/// The variable `at` names in `packages`, `(package index, variable index)`.
fn variable_at(
    packages: &[RootPackageVariables],
    (package, variable): (usize, usize),
) -> &PackageVariable {
    &packages[package].variables[variable]
}

/// The first rule of [`check_root_variables`] the name `name`, the root's `position`th
/// (from 0), breaks with its `declarations`, the first of them `first`, against `goal_names` and
/// the root's judgement of irreversibility `irreversible`; `None` when it applies.
fn refusal_of(
    packages: &[RootPackageVariables],
    position: usize,
    name: &str,
    declarations: &[(usize, usize)],
    first: &PackageVariable,
    goal_names: &BTreeSet<&str>,
    irreversible: &IrreversiblePackages,
) -> Option<Refusal> {
    let every = |problem, member| {
        Some(Refusal {
            problem,
            member,
            declarations: declarations.to_vec(),
        })
    };
    if position >= MAX_ROOT_VARIABLES {
        return every(StandardPackProblem::VariableOutOfLimits, Some("name"));
    }
    let compact = |variable: &PackageVariable| serde_json::to_vec(&variable.declaration).ok();
    if declarations
        .iter()
        .any(|declaration| compact(variable_at(packages, *declaration)) != compact(first))
    {
        return every(StandardPackProblem::VariableConflict, None);
    }
    if goal_names.contains(name) {
        return every(StandardPackProblem::VariableConflict, Some("name"));
    }
    // Fix round 1: the whole root's judgement, the one copy the goal check holds a placement to.
    // Admission keeps variables off a package that marks an irreversible capability itself; a
    // package that provides one unmarked, or whose remedies list one, is judged here.
    let on_irreversible = declarations
        .iter()
        .copied()
        .filter(|(package, _)| irreversible.counts_irreversible(&packages[*package].package_id))
        .collect::<Vec<_>>();
    if !on_irreversible.is_empty() {
        return Some(Refusal {
            problem: StandardPackProblem::VariableIrreversibleByRoot,
            member: None,
            declarations: on_irreversible,
        });
    }
    // A designated effect's cap reads a whole number from 0 to 1000 before the run's first step,
    // from an `integer` variable only (fix round 1: its `min` and any other type too).
    let key = first.fact_key();
    let readers = declarations
        .iter()
        .copied()
        .filter(|(package, _)| packages[*package].cap_fact_keys.contains(&key))
        .collect::<Vec<_>>();
    if readers.is_empty() {
        return None;
    }
    Some(Refusal {
        problem: StandardPackProblem::VariableCapRange,
        member: Some(cap_member(first)?),
        declarations: readers,
    })
}

/// The member of `variable`'s declaration at which a designated effect's cap read from it breaks
/// the rule (its `min` below 0, its `max` above [`MAX_VARIABLE_CAP`], or its `type` when it is no
/// `integer`); `None` when the cap may read it.
fn cap_member(variable: &PackageVariable) -> Option<&'static str> {
    match &variable.kind {
        VariableKind::Integer { min, .. } if *min < 0 => Some("min"),
        VariableKind::Integer { max, .. } if *max > MAX_VARIABLE_CAP => Some("max"),
        VariableKind::Integer { .. } => None,
        VariableKind::Choice { .. }
        | VariableKind::Switch { .. }
        | VariableKind::Ordered { .. } => Some("type"),
    }
}

/// Workflow #463 V2 fix round 2 (`contracts/package-variables.md`, "A variable that does not
/// apply"): what the root's judgement ([`judge_root_variables`]) takes for the game root's goal
/// document `goals.json`, as a reader read it with [`crate::parse_standard_pack_goals`]: absent,
/// accepted with its entries, refused (it applies no entry and takes no name), or unreadable (an
/// I/O failure other than its absence: which names it takes is not known, so no variable of the
/// root applies).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootGoalDocument<'a> {
    Absent,
    Accepted(&'a StandardPackGoals),
    Refused,
    Unreadable,
}

/// Workflow #463 V2 fix round 2: the detail of why a variable of a game root does not apply, beside
/// its word ([`NotApplyingCause`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VariableCause {
    /// `declaration_conflict`: the declaring packages whose declaration of the name is not
    /// byte-identical to the first one's, in registry order (the first is the verdict's first
    /// package).
    DeclarationConflict { packages: Vec<String> },
    /// `goal_name_conflict`: the type of the entry of the accepted `goals.json` that takes the
    /// name (the entry is named like the variable).
    GoalEntry { goal_type: Option<GoalType> },
    /// `irreversible_by_root`: per declaring package that can do something irreversible by
    /// itself, the capabilities it provides or its remedies list that a package of the root marks
    /// irreversible.
    IrreversibleCapabilities {
        packages: BTreeMap<String, Vec<String>>,
    },
    /// `package_unreadable`: the packages of the root whose declaration could not be read, and a
    /// declaring package the judgement was not told of.
    UnreadablePackages { packages: Vec<String> },
    /// `name_out_of_limits`: the name's place in the root's order, from 1.
    NameOutOfLimits { position: usize },
    /// `cap_range`: the declaring packages that read the variable as a designated effect's cap,
    /// and the member of the declaration the problem is at (`type`, `min` or `max`).
    CapRange {
        packages: Vec<String>,
        member: &'static str,
    },
    /// No detail beyond the word: `goals_unreadable` (the reader that could not read the file
    /// keeps its failure).
    WordOnly,
}

/// Workflow #463 V2 fix round 2: why a variable of a game root does not apply: the word a run's
/// record and `goal_value_not_applied` say it with, and its detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotApplyingCause {
    pub problem: VariableProblem,
    pub cause: VariableCause,
}

/// Workflow #463 V2 fix round 2: one variable name of a game root as the root's judgement
/// ([`judge_root_variables`]) gives it: every package that declares it (registry order), its first
/// declaration, and why it does not apply (`None`: it applies).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootVariableVerdict {
    pub name: String,
    pub packages: Vec<String>,
    pub declaration: PackageVariable,
    pub not_applying: Option<NotApplyingCause>,
}

impl RootVariableVerdict {
    /// Whether the variable applies in the root.
    pub const fn applies(&self) -> bool {
        self.not_applying.is_none()
    }
}

/// Workflow #463 V2 fix round 2: the root's judgement of its variables
/// ([`judge_root_variables`]): the root check's answer, which `standard-pack check` reports, and
/// a verdict per name in the order the root first declares it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RootVariablesJudged {
    pub checked: RootVariables,
    pub verdicts: Vec<RootVariableVerdict>,
}

impl RootVariablesJudged {
    /// Each name that does not apply, with its word.
    pub fn not_applying(&self) -> BTreeMap<String, VariableProblem> {
        self.verdicts
            .iter()
            .filter_map(|verdict| {
                verdict
                    .not_applying
                    .as_ref()
                    .map(|cause| (verdict.name.clone(), cause.problem))
            })
            .collect()
    }
}

/// Workflow #463 V2 fix round 2 (`contracts/package-variables.md`, "A variable that does not
/// apply"; the coordinator's ruling: the rules of "does not apply" have one copy, here): the
/// judgement of the variables `packages` declare (the root's packages in registry order, those a
/// reader could read) over the whole game root, the one `standard-pack check` and the Runtime's
/// scheduler workbench give. The goal document counts as [`RootGoalDocument`] says: an accepted
/// file's entries take the names they use for anything but a placement, a refused or absent file
/// takes none, and an unreadable one keeps every name of the root from applying
/// (`goals_unreadable`) that no other rule already keeps. Then [`check_root_variables`] with
/// `irreversible` and [`variables_not_applying`] give each name its word, and each word its
/// detail ([`VariableCause`]).
pub fn judge_root_variables(
    packages: &[RootPackageVariables],
    goals: RootGoalDocument<'_>,
    irreversible: &IrreversiblePackages,
) -> RootVariablesJudged {
    let goal_names = match goals {
        RootGoalDocument::Accepted(accepted) => accepted.names_besides_placements(),
        RootGoalDocument::Absent | RootGoalDocument::Refused | RootGoalDocument::Unreadable => {
            BTreeSet::new()
        }
    };
    let checked = check_root_variables(packages, &goal_names, irreversible);
    let words = variables_not_applying(packages, &checked, irreversible);
    let root = RootDeclarations::of(packages);
    let mut verdicts = Vec::with_capacity(root.order.len());
    for (position, name) in root.order.iter().enumerate() {
        let Some(declarations) = root.declared.get(name) else {
            continue;
        };
        let Some(&first_at) = declarations.first() else {
            continue;
        };
        let first = variable_at(packages, first_at);
        let word = match (words.get(*name), goals) {
            (Some(word), _) => Some(*word),
            (None, RootGoalDocument::Unreadable) => Some(VariableProblem::GoalsUnreadable),
            (None, _) => None,
        };
        verdicts.push(RootVariableVerdict {
            name: (*name).to_owned(),
            packages: declarations
                .iter()
                .map(|(package, _)| packages[*package].package_id.clone())
                .collect(),
            declaration: first.clone(),
            not_applying: word.map(|problem| NotApplyingCause {
                problem,
                cause: cause_of(
                    problem,
                    packages,
                    position,
                    declarations,
                    first,
                    goals,
                    irreversible,
                ),
            }),
        });
    }
    RootVariablesJudged { checked, verdicts }
}

/// The detail of the word `problem` of the name the root's `position`th (from 0), whose
/// declarations in `packages` are `declarations`, the first `first`.
fn cause_of(
    problem: VariableProblem,
    packages: &[RootPackageVariables],
    position: usize,
    declarations: &[(usize, usize)],
    first: &PackageVariable,
    goals: RootGoalDocument<'_>,
    irreversible: &IrreversiblePackages,
) -> VariableCause {
    let id_of = |(package, _): &(usize, usize)| packages[*package].package_id.clone();
    match problem {
        VariableProblem::DeclarationConflict => {
            let compact =
                |variable: &PackageVariable| serde_json::to_vec(&variable.declaration).ok();
            VariableCause::DeclarationConflict {
                packages: declarations
                    .iter()
                    .filter(|declaration| {
                        compact(variable_at(packages, **declaration)) != compact(first)
                    })
                    .map(id_of)
                    .collect(),
            }
        }
        VariableProblem::GoalNameConflict => VariableCause::GoalEntry {
            goal_type: match goals {
                RootGoalDocument::Accepted(accepted) => {
                    accepted.entry(&first.name).map(DeclaredGoal::goal_type)
                }
                RootGoalDocument::Absent
                | RootGoalDocument::Refused
                | RootGoalDocument::Unreadable => None,
            },
        },
        VariableProblem::IrreversibleByRoot => VariableCause::IrreversibleCapabilities {
            packages: declarations
                .iter()
                .filter_map(|(package, _)| {
                    let package_id = &packages[*package].package_id;
                    irreversible
                        .irreversible_capabilities(package_id)
                        .filter(|reached| !reached.is_empty())
                        .map(|reached| (package_id.clone(), reached.iter().cloned().collect()))
                })
                .collect(),
        },
        VariableProblem::PackageUnreadable => VariableCause::UnreadablePackages {
            packages: irreversible
                .unreadable()
                .iter()
                .cloned()
                .chain(
                    declarations
                        .iter()
                        .map(id_of)
                        .filter(|package_id| irreversible.package(package_id).is_none()),
                )
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        },
        VariableProblem::NameOutOfLimits => VariableCause::NameOutOfLimits {
            position: position + 1,
        },
        VariableProblem::CapRange => {
            let key = first.fact_key();
            VariableCause::CapRange {
                packages: declarations
                    .iter()
                    .filter(|(package, _)| packages[*package].cap_fact_keys.contains(&key))
                    .map(id_of)
                    .collect(),
                member: cap_member(first).unwrap_or("type"),
            }
        }
        VariableProblem::GoalsUnreadable
        | VariableProblem::NotAnOption
        | VariableProblem::OutOfRange
        | VariableProblem::OffStep
        | VariableProblem::TypeChanged
        | VariableProblem::KeysRemoved
        | VariableProblem::CountOutOfRange
        | VariableProblem::KeyRepeated
        | VariableProblem::KeysLeftOut
        | VariableProblem::EntryUnreadable
        | VariableProblem::RootUnchecked => VariableCause::WordOnly,
    }
}

/// The variables of a game root as the references of its `goals.json` hold them
/// ([`crate::GoalReferences`]): every name a package of `packages` declares, whether the root
/// check lets it apply or not, with the first declaration's block and every package that declares
/// it, in registry order.
pub fn goal_variables(packages: &[RootPackageVariables]) -> BTreeMap<String, GoalVariable> {
    let mut variables = BTreeMap::<String, GoalVariable>::new();
    for package in packages {
        for variable in &package.variables {
            variables
                .entry(variable.name.clone())
                .or_insert_with(|| GoalVariable {
                    block: variable.block,
                    packages: Vec::new(),
                })
                .packages
                .push(package.package_id.clone());
        }
    }
    variables
}

/// The names the packages of a root declare.
struct RootDeclarations<'a> {
    /// Each name once, in the order the root first declares it.
    order: Vec<&'a str>,
    /// Each name's declarations, `(package index, variable index)` in registry order.
    declared: BTreeMap<&'a str, Vec<(usize, usize)>>,
}

impl<'a> RootDeclarations<'a> {
    fn of(packages: &'a [RootPackageVariables]) -> Self {
        let mut order = Vec::new();
        let mut declared = BTreeMap::<&str, Vec<(usize, usize)>>::new();
        for (package_index, package) in packages.iter().enumerate() {
            for (variable_index, variable) in package.variables.iter().enumerate() {
                let declarations = declared.entry(variable.name.as_str()).or_default();
                if declarations.is_empty() {
                    order.push(variable.name.as_str());
                }
                declarations.push((package_index, variable_index));
            }
        }
        Self { order, declared }
    }
}

#[cfg(test)]
#[path = "package_variables_tests.rs"]
mod tests;
