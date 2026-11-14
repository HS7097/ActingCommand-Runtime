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
//! Only the declaration is built here: nothing resolves a value, nothing overlays
//! `session.dispatch.parameter.<name>`, and no run reads a variable (later slices of #463). A
//! package without `variables` reads as before.

use crate::standard_pack::{StandardPackProblem, StandardPackRootIssue};
use crate::standard_pack_goals::{
    Check, GoalBlock, GoalOption, GoalText, GoalTimeUnit, GoalUnit, GoalVariable,
    IrreversiblePackages, MAX_GOAL_CHOICE_OPTIONS, MAX_GOAL_ENTRIES, MAX_GOAL_LIST_VALUES,
    MAX_GOAL_NAME_BYTES, MAX_GOAL_ORDERED_COUNT, MIN_GOAL_CHOICE_OPTIONS, bounded,
};
use crate::standard_pack_ui::{MAX_PACK_UI_TITLE_BYTES, at};
use crate::{MAX_DESIGNATED_EFFECT_REPEATS, ProvidedCapability, remedy_token_valid};
use serde::Serialize;
use serde_json::{Map, Value};
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
/// The key prefix under which a later slice (V2) overlays a variable for a run, the one remedy
/// parameters use (`contracts/remedies.md` §4); the root check reads it only to know which
/// variable a designated effect's cap reads.
pub const VARIABLE_FACT_KEY_PREFIX: &str = "session.dispatch.parameter.";

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

    /// The block every reader shows the type with: the only block of version 1 it takes.
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

    /// The fact key a later slice overlays the variable's value under, and a designated effect's
    /// cap names to read it: [`VARIABLE_FACT_KEY_PREFIX`] and the name.
    pub fn fact_key(&self) -> String {
        format!("{VARIABLE_FACT_KEY_PREFIX}{}", self.name)
    }
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
    let member = match &first.kind {
        VariableKind::Integer { min, .. } if *min < 0 => "min",
        VariableKind::Integer { max, .. } if *max > MAX_VARIABLE_CAP => "max",
        VariableKind::Integer { .. } => return None,
        VariableKind::Choice { .. }
        | VariableKind::Switch { .. }
        | VariableKind::Ordered { .. } => "type",
    };
    Some(Refusal {
        problem: StandardPackProblem::VariableCapRange,
        member: Some(member),
        declarations: readers,
    })
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
