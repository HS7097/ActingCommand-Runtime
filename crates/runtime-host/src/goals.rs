// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463 V3, which is Workflow #455 G-R2b with the variables in it (register §1.383;
//! `contracts/runtime-interface.md`, "Goal request kinds"; `contracts/package-variables.md`,
//! "Reading and setting a value by name"): what the goal page's two requests decide, as pure
//! functions of what the host read.
//!
//! - **The page** ([`GoalPage`]): the application's game, what the workbench holds of its
//!   `goals.json` (an accepted file only; a refused file applies no entry), the packages the
//!   active catalog binds on the application (a catalog task whose scope covers it, and the
//!   package the remedy table read for that task) with their variables and remedy parameters, the
//!   pools it can target with their capacities, and which variables do not apply in its game root.
//! - **The checks before the gate** ([`check_edit`]): the name and the value of an `EditGoal`
//!   against the page and the stored record, each refusal one [`GoalRefusal`] with the word of
//!   [`GoalProblem`]; a change that passes is one [`GoalRecordEdit`], which the host writes under the
//!   fact write gate.
//! - **The view** ([`view_entries`]): every entry of the page in its order, the stored values no
//!   entry offers any more, and the stored targets that are no declared resource entry's.
//!
//! Nothing here reads a clock, a file, a lock or the ledger.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::{
    DeclaredGoal, DeclaredGoalKind, DefaultOrigin, GoalEntryState, GoalEntryType, GoalOrphan,
    GoalProblem, GoalResourceState, GoalSetBy, GoalText, GoalUnit, GoalValueSource, GoalViewEntry,
    MAX_GOAL_SAFE_INTEGER, NotTargetableResource, PackageVariable, ResourceTargetCondition,
    ResourceTargetConditionState, StandardPackGoals, TargetableResource, VariableKind,
    VariableProblem, VariableSource, goal_name, variable_value_json,
};
use actingcommand_policy::{
    GoalRecordEdit, GoalRecordRefusal, ResolvedVariable, StoredGoalEntry, StoredGoalRecord,
    StoredGoalTarget, TargetCondition, TargetSetter, goal_entry_value_fits,
};
use serde_json::Value;

/// A remedy parameter a bound package's `provides` declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoundParameter {
    pub(crate) capability: String,
    pub(crate) irreversible: bool,
    pub(crate) name: String,
    pub(crate) options: Vec<String>,
    pub(crate) default_option: String,
}

/// A package the active catalog binds on the application: the first catalog task that runs it,
/// its id, the variables it declares and the remedy parameters its `provides` declare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoundPackage {
    pub(crate) task_id: String,
    pub(crate) package_id: String,
    pub(crate) variables: Vec<PackageVariable>,
    pub(crate) parameters: Vec<BoundParameter>,
}

/// What the checks and the view read of one application's goal page besides its record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GoalPage {
    pub(crate) game_id: String,
    /// The game's `goals.json` when the workbench holds it accepted.
    pub(crate) declaration: Option<StandardPackGoals>,
    /// The bound packages, each once, in the order of the catalog task that first runs it.
    pub(crate) packages: Vec<BoundPackage>,
    /// The pools the application can target, as `ResourceTargetView` judges them.
    pub(crate) targetable: Vec<TargetableResource>,
    pub(crate) not_targetable: Vec<NotTargetableResource>,
    /// The capacity of each targetable pool whose capacity reads one, by pool id.
    pub(crate) capacities: BTreeMap<String, u64>,
    /// Each variable name of the bound packages that does not apply in the game root, with the
    /// word that says why (the table's root check).
    pub(crate) not_applying: BTreeMap<String, VariableProblem>,
}

impl GoalPage {
    /// The first declaration of the variable `name` among the bound packages.
    fn variable(&self, name: &str) -> Option<&PackageVariable> {
        self.packages
            .iter()
            .flat_map(|package| &package.variables)
            .find(|variable| variable.name == name)
    }

    /// Whether `name` is a remedy parameter of a bound package.
    fn parameter(&self, name: &str) -> bool {
        self.packages
            .iter()
            .flat_map(|package| &package.parameters)
            .any(|parameter| parameter.name == name)
    }
}

/// Why an `EditGoal` was refused before the fact write gate: the word, and the values that
/// locate it (`json_pointer` within the value, `range_min` and `range_max`, `pool_id`,
/// `variable_problem`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GoalRefusal {
    pub(crate) problem: GoalProblem,
    pub(crate) pointer: Option<String>,
    pub(crate) range: Option<(i64, i64)>,
    pub(crate) pool_id: Option<String>,
    pub(crate) variable_problem: Option<VariableProblem>,
}

impl GoalRefusal {
    fn new(problem: GoalProblem) -> Self {
        Self {
            problem,
            pointer: None,
            range: None,
            pool_id: None,
            variable_problem: None,
        }
    }

    fn at(mut self, pointer: impl Into<String>) -> Self {
        self.pointer = Some(pointer.into());
        self
    }

    fn within(mut self, min: i64, max: i64) -> Self {
        self.range = Some((min, max));
        self
    }

    /// The pool the value names, when it is a text an outcome carries as it is.
    fn pool(mut self, pool_id: &str) -> Self {
        self.pool_id = carried(pool_id).then(|| pool_id.to_owned());
        self
    }

    /// The refusal of a record that cannot take the edit.
    pub(crate) fn of_record(refusal: GoalRecordRefusal) -> Option<Self> {
        match refusal {
            GoalRecordRefusal::Unreadable => Some(Self::new(GoalProblem::RecordUnreadable)),
            GoalRecordRefusal::ValueTooLarge => {
                Some(Self::new(GoalProblem::ValueTooLarge).within(0, max_entry_value_bytes()))
            }
            GoalRecordRefusal::ValueInvalid => Some(Self::new(GoalProblem::WrongType)),
            GoalRecordRefusal::TargetsFull => Some(
                Self::new(GoalProblem::RecordOutOfLimits)
                    .within(0, max_i64(actingcommand_contract::MAX_RESOURCE_TARGETS)),
            ),
            GoalRecordRefusal::EntriesFull => {
                Some(Self::new(GoalProblem::RecordOutOfLimits).within(
                    0,
                    max_i64(actingcommand_policy::MAX_RESOURCE_TARGET_ENTRIES),
                ))
            }
            // An internal fault, not the caller's: the host fails the request on its own code.
            GoalRecordRefusal::Inconsistent => None,
        }
    }
}

/// Whether a text the caller wrote (a member's name, a pool id) is carried in a refusal's values as
/// it is: 1 to 128 bytes without a control character, `/` or `~` (no pointer escape is needed).
fn carried(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 128
        && !text
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '~'))
}

/// A bound as an `i64` value of an outcome.
fn max_i64(bound: usize) -> i64 {
    i64::try_from(bound).unwrap_or(i64::MAX)
}

/// The longest canonical text of a stored value, in bytes.
fn max_entry_value_bytes() -> i64 {
    max_i64(actingcommand_policy::MAX_RESOURCE_TARGET_ENTRY_VALUE_BYTES)
}

/// The largest `at_least` a target takes: the safe integers.
const MAX_AT_LEAST: u64 = MAX_GOAL_SAFE_INTEGER.unsigned_abs();

/// The one change of an `EditGoal`, as the request names it: set `name` to the JSON text
/// `value_json`, or clear it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditRequest<'a> {
    Set { name: &'a str, value_json: &'a str },
    Clear { name: &'a str },
}

/// Workflow #463 V3 (`contracts/runtime-interface.md`, "Goal request kinds", the checks before the
/// gate): the change `request` makes to the record `stored` of the application whose page is
/// `page`, or why it is refused. A name outside an entry name's form is `name_unknown`. Then, in
/// this order, the name is:
///
/// 1. an entry of the accepted `goals.json`: a `heading` or a `note` holds no value
///    (`holds_no_value`); a `tier` is `tier_not_built`; a `resource` entry takes `{at_least}` on
///    its pool, as an other target does (below); a `variable` placement is the variable of its name
///    (2), or `name_unknown` when no bound package declares it;
/// 2. a variable of a bound package: one that does not apply is `variable_not_applying` with its
///    `variable_problem`; else its value is judged against its declaration ([`check_variable`])
///    and the store's bound;
/// 3. a remedy parameter of a bound package: `remedy_parameter`;
/// 4. for a `set`, an other target: the value `{resource, at_least}` on a targetable pool, as
///    [`check_target`] judges it; for a `clear`, a stored value or a stored target of the name;
/// 5. else `name_unknown`.
///
/// A `clear` of a known name that holds nothing is `value_not_set`; a `clear` of a resource entry
/// removes the target on its pool whatever its id.
pub(crate) fn check_edit(
    page: &GoalPage,
    stored: &StoredGoalRecord,
    request: EditRequest<'_>,
) -> Result<GoalRecordEdit, GoalRefusal> {
    let name = match request {
        EditRequest::Set { name, .. } | EditRequest::Clear { name } => name,
    };
    if !goal_name(name) {
        return Err(GoalRefusal::new(GoalProblem::NameUnknown));
    }
    let value = match request {
        EditRequest::Set { value_json, .. } => Some(
            serde_json::from_str::<Value>(value_json)
                .map_err(|_| GoalRefusal::new(GoalProblem::ValueNotJson))?,
        ),
        EditRequest::Clear { .. } => None,
    };
    let targets = stored.targets();
    let declared = page
        .declaration
        .as_ref()
        .and_then(|declaration| declaration.entry(name));
    if let Some(declared) = declared {
        match &declared.kind {
            DeclaredGoalKind::Heading | DeclaredGoalKind::Note => {
                return Err(GoalRefusal::new(GoalProblem::HoldsNoValue));
            }
            DeclaredGoalKind::Tier { .. } => {
                return Err(GoalRefusal::new(GoalProblem::TierNotBuilt));
            }
            DeclaredGoalKind::Resource { pool, .. } => {
                return match &value {
                    Some(value) => {
                        let at_least = check_target(page, value, Some(pool))?;
                        Ok(GoalRecordEdit::SetTarget {
                            id: name.to_owned(),
                            resource: pool.clone(),
                            at_least,
                        })
                    }
                    None => targets
                        .iter()
                        .find(|stored| stored.target.resource == *pool)
                        .map(|stored| GoalRecordEdit::ClearTarget {
                            id: stored.target.id.clone(),
                        })
                        .ok_or_else(|| GoalRefusal::new(GoalProblem::ValueNotSet)),
                };
            }
            DeclaredGoalKind::Variable if page.variable(name).is_none() => {
                return Err(GoalRefusal::new(GoalProblem::NameUnknown));
            }
            DeclaredGoalKind::Variable => {}
        }
    }
    let stored_entry = stored.entries().iter().any(|entry| entry.name == name);
    if let Some(variable) = page.variable(name) {
        return match &value {
            Some(value) => {
                if let Some(problem) = page.not_applying.get(name) {
                    let mut refusal = GoalRefusal::new(GoalProblem::VariableNotApplying);
                    refusal.variable_problem = Some(*problem);
                    return Err(refusal);
                }
                check_variable(variable, value)?;
                fits_the_store(value)?;
                Ok(GoalRecordEdit::SetEntry {
                    name: name.to_owned(),
                    value: value.clone(),
                })
            }
            None if stored_entry => Ok(GoalRecordEdit::ClearEntry {
                name: name.to_owned(),
            }),
            None => Err(GoalRefusal::new(GoalProblem::ValueNotSet)),
        };
    }
    if page.parameter(name) {
        return Err(GoalRefusal::new(GoalProblem::RemedyParameter));
    }
    match &value {
        Some(value) if value.get("resource").is_some() => {
            let at_least = check_target(page, value, None)?;
            let resource = value
                .get("resource")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            Ok(GoalRecordEdit::SetTarget {
                id: name.to_owned(),
                resource,
                at_least,
            })
        }
        Some(_) => Err(GoalRefusal::new(GoalProblem::NameUnknown)),
        None if stored_entry => Ok(GoalRecordEdit::ClearEntry {
            name: name.to_owned(),
        }),
        None if targets.iter().any(|stored| stored.target.id == name) => {
            Ok(GoalRecordEdit::ClearTarget {
                id: name.to_owned(),
            })
        }
        None => Err(GoalRefusal::new(GoalProblem::NameUnknown)),
    }
}

/// Whether the store keeps `value` (its canonical text at most 1024 bytes).
fn fits_the_store(value: &Value) -> Result<(), GoalRefusal> {
    goal_entry_value_fits(value).map_err(|refusal| {
        GoalRefusal::of_record(refusal).unwrap_or_else(|| GoalRefusal::new(GoalProblem::WrongType))
    })
}

/// Workflow #463 V3: `value`, set on an application, against the declaration of `variable` (the
/// strict rules of a write, where a run's judgement of a stored value leaves keys out):
///
/// - `choice`: a string (else `wrong_type`) that is one of the options (else `not_an_option`);
/// - `integer`: a whole number (else `wrong_type`) from `min` to `max` (else `out_of_range`,
///   with the range) on a step from `min` (else `off_step`, `range_min` the variable's `min`);
/// - `switch`: a boolean (else `wrong_type`);
/// - `ordered`: a list (else `wrong_type`) whose items, at their pointer, are strings (else
///   `wrong_type`), each once (else `key_repeated`), each an option (else `not_an_option`), and
///   whose count is from `min_count` to `max_count` (else `count_out_of_range`).
pub(crate) fn check_variable(variable: &PackageVariable, value: &Value) -> Result<(), GoalRefusal> {
    match &variable.kind {
        VariableKind::Choice { options, .. } => {
            let Value::String(chosen) = value else {
                return Err(GoalRefusal::new(GoalProblem::WrongType));
            };
            if options.iter().any(|option| option.value == *chosen) {
                Ok(())
            } else {
                Err(GoalRefusal::new(GoalProblem::NotAnOption))
            }
        }
        VariableKind::Integer { min, max, step, .. } => {
            let Some(number) = value.as_i64() else {
                return Err(GoalRefusal::new(GoalProblem::WrongType));
            };
            if !(*min..=*max).contains(&number) {
                return Err(GoalRefusal::new(GoalProblem::OutOfRange).within(*min, *max));
            }
            if (i128::from(number) - i128::from(*min)) % i128::from(*step) != 0 {
                return Err(GoalRefusal::new(GoalProblem::OffStep).within(*min, *max));
            }
            Ok(())
        }
        VariableKind::Switch { .. } => match value {
            Value::Bool(_) => Ok(()),
            _ => Err(GoalRefusal::new(GoalProblem::WrongType)),
        },
        VariableKind::Ordered {
            options,
            min_count,
            max_count,
            ..
        } => {
            let Value::Array(items) = value else {
                return Err(GoalRefusal::new(GoalProblem::WrongType));
            };
            let mut seen = BTreeSet::new();
            for (index, item) in items.iter().enumerate() {
                let pointer = format!("/{index}");
                let Value::String(key) = item else {
                    return Err(GoalRefusal::new(GoalProblem::WrongType).at(pointer));
                };
                if !seen.insert(key.as_str()) {
                    return Err(GoalRefusal::new(GoalProblem::KeyRepeated).at(pointer));
                }
                if !options.iter().any(|option| option.value == *key) {
                    return Err(GoalRefusal::new(GoalProblem::NotAnOption).at(pointer));
                }
            }
            let count = u64::try_from(items.len()).unwrap_or(u64::MAX);
            if !(*min_count..=*max_count).contains(&count) {
                return Err(GoalRefusal::new(GoalProblem::CountOutOfRange).within(
                    i64::try_from(*min_count).unwrap_or(i64::MAX),
                    i64::try_from(*max_count).unwrap_or(i64::MAX),
                ));
            }
            Ok(())
        }
    }
}

/// Workflow #463 V3: a resource value, the `at_least` it holds, or why it is refused. For a
/// resource entry (`pool` its `governs.pool`) the value is `{at_least}`; for an other target it is
/// `{resource, at_least}`. Every other member is `wrong_type` at it, but `by_unix_ms`, which is
/// `deadline_unsupported`; `at_least` is a whole number (else `wrong_type`) from 1 to the safe
/// integers and not above the pool's capacity when its capacity reads one (else `out_of_range`);
/// the pool is one the application can target (else `pool_not_targetable`) whose valuation gives
/// the step and the importance a goal by name does not carry (else `pool_unvalued`).
pub(crate) fn check_target(
    page: &GoalPage,
    value: &Value,
    pool: Option<&String>,
) -> Result<u64, GoalRefusal> {
    let Value::Object(members) = value else {
        return Err(GoalRefusal::new(GoalProblem::WrongType));
    };
    let allowed: &[&str] = match pool {
        Some(_) => &["at_least", "by_unix_ms"],
        None => &["resource", "at_least", "by_unix_ms"],
    };
    if let Some(member) = members
        .keys()
        .find(|member| !allowed.contains(&member.as_str()))
    {
        let refusal = GoalRefusal::new(GoalProblem::WrongType);
        return Err(if carried(member) {
            refusal.at(format!("/{member}"))
        } else {
            refusal
        });
    }
    if members.contains_key("by_unix_ms") {
        return Err(GoalRefusal::new(GoalProblem::DeadlineUnsupported).at("/by_unix_ms"));
    }
    let pool = match pool {
        Some(pool) => pool.clone(),
        None => match members.get("resource") {
            Some(Value::String(resource)) => resource.clone(),
            _ => return Err(GoalRefusal::new(GoalProblem::WrongType).at("/resource")),
        },
    };
    let Some(at_least) = members.get("at_least").and_then(Value::as_u64) else {
        return Err(GoalRefusal::new(GoalProblem::WrongType).at("/at_least"));
    };
    let Some(targetable) = page
        .targetable
        .iter()
        .find(|targetable| targetable.resource == pool)
    else {
        return Err(GoalRefusal::new(GoalProblem::PoolNotTargetable).pool(&pool));
    };
    if targetable.scale_required || targetable.importance_required {
        return Err(GoalRefusal::new(GoalProblem::PoolUnvalued).pool(&pool));
    }
    let ceiling = page
        .capacities
        .get(&pool)
        .copied()
        .unwrap_or(MAX_AT_LEAST)
        .min(MAX_AT_LEAST);
    if !(1..=ceiling).contains(&at_least) {
        return Err(GoalRefusal::new(GoalProblem::OutOfRange)
            .at("/at_least")
            .within(1, i64::try_from(ceiling).unwrap_or(i64::MAX))
            .pool(&pool));
    }
    Ok(at_least)
}

/// Who set a stored row, as the page shows it.
fn set_by(setter: TargetSetter) -> GoalSetBy {
    GoalSetBy {
        unix_ms: setter.set_unix_ms,
        channel: setter.channel,
        initiator: setter.initiator,
    }
}

/// One variable of the page: its name, its first declaration, every bound package that declares
/// it, and the value the next dispatch of the first catalog task that runs one of them hands its
/// run ([`ResolvedVariable`], the run's own resolution), when one was resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PageVariable<'a> {
    pub(crate) variable: &'a PackageVariable,
    pub(crate) packages: Vec<String>,
    pub(crate) resolved: Option<&'a ResolvedVariable>,
}

/// The variables of `page`, each once, in the order the bound packages first declare them, each
/// with `resolved`'s value of the first package's task that declares it (`resolved` gives the
/// resolved values of a task, in declaration order).
pub(crate) fn page_variables<'a>(
    page: &'a GoalPage,
    resolved: &dyn Fn(&str) -> Option<&'a [ResolvedVariable]>,
) -> Vec<PageVariable<'a>> {
    let mut rows: Vec<PageVariable<'a>> = Vec::new();
    for package in &page.packages {
        let values = resolved(&package.task_id);
        for variable in &package.variables {
            match rows
                .iter_mut()
                .find(|row| row.variable.name == variable.name)
            {
                Some(row) => {
                    if !row.packages.contains(&package.package_id) {
                        row.packages.push(package.package_id.clone());
                    }
                }
                None => rows.push(PageVariable {
                    variable,
                    packages: vec![package.package_id.clone()],
                    resolved: values.and_then(|values| {
                        values
                            .iter()
                            .find(|value| value.variable_name == variable.name)
                    }),
                }),
            }
        }
    }
    rows
}

/// What the view lists: the entries in their order, the stored values no entry offers any more,
/// and the ids of the stored targets that are no declared resource entry's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ViewEntries {
    pub(crate) entries: Vec<GoalViewEntry>,
    pub(crate) orphans: Vec<GoalOrphan>,
    pub(crate) other_targets: Vec<String>,
}

/// Workflow #463 V3 (`contracts/runtime-interface.md`, "Goal request kinds", the view): the
/// entries of `page` with the stored record `stored`, `variables` its variables
/// ([`page_variables`]) and `conditions` the stored targets' conditions:
///
/// - each entry of the accepted `goals.json`, in its order, with the entry as its reader read it
///   ([`declared_entry`]): a `heading` and a `note` as they are; a `resource` entry with the
///   target on its pool (its `{at_least}` the value, set, who set it) or no goal (the default),
///   the pool's reading and the target's gap; a `tier` with its default and any stored value,
///   `not_built`; a placement with its variable ([`variable_entry`]), or, when no bound package
///   declares it, alone;
/// - then each variable no entry places, in the order the bound packages declare them;
/// - the orphans: each stored value whose name is no variable of the page and no entry of the
///   accepted file;
/// - the other targets: each stored target that is not on a declared resource entry's pool.
pub(crate) fn view_entries(
    page: &GoalPage,
    stored: &StoredGoalRecord,
    variables: &[PageVariable<'_>],
    conditions: &[ResourceTargetCondition],
) -> ViewEntries {
    let targets = stored.targets();
    let entries_stored = stored.entries();
    let stored_entry = |name: &str| stored_entry_of(entries_stored, name);
    let mut placed = BTreeSet::new();
    let mut declared_pools = BTreeSet::new();
    let mut entries = Vec::new();
    for declared in page
        .declaration
        .iter()
        .flat_map(|declaration| declaration.entries.iter())
    {
        let raw_entry = Some(declared_entry(declared));
        let entry = match &declared.kind {
            DeclaredGoalKind::Heading | DeclaredGoalKind::Note => {
                let entry_type = if declared.kind == DeclaredGoalKind::Heading {
                    GoalEntryType::Heading
                } else {
                    GoalEntryType::Note
                };
                let mut entry = GoalViewEntry::new(declared.name.as_str(), entry_type, true);
                entry.entry = raw_entry;
                entry
            }
            DeclaredGoalKind::Resource { pool, .. } => {
                declared_pools.insert(pool.as_str());
                resource_entry(
                    declared.name.as_str(),
                    pool,
                    &targets,
                    page,
                    conditions,
                    raw_entry,
                )
            }
            DeclaredGoalKind::Tier { default, .. } => {
                let mut entry =
                    GoalViewEntry::new(declared.name.as_str(), GoalEntryType::Tier, true);
                entry.entry = raw_entry;
                entry.default_json = Some(variable_value_json(&Value::String(default.clone())));
                entry.state = Some(GoalEntryState::NotBuilt);
                entry.source = Some(GoalValueSource::Default);
                if let Some(stored) = stored_entry(declared.name.as_str()) {
                    entry.value_json = Some(stored.value_json.clone());
                    entry.set = Some(set_by(stored.set));
                }
                placed.insert(declared.name.as_str());
                entry
            }
            DeclaredGoalKind::Variable => {
                placed.insert(declared.name.as_str());
                match variables
                    .iter()
                    .find(|row| row.variable.name == declared.name)
                {
                    Some(row) => variable_entry(
                        row,
                        stored_entry(declared.name.as_str()),
                        page.not_applying.get(&declared.name).copied(),
                        true,
                        raw_entry,
                    ),
                    None => {
                        let mut entry = GoalViewEntry::new(
                            declared.name.as_str(),
                            GoalEntryType::Variable,
                            true,
                        );
                        entry.entry = raw_entry;
                        if let Some(stored) = stored_entry(declared.name.as_str()) {
                            entry.value_json = Some(stored.value_json.clone());
                            entry.set = Some(set_by(stored.set));
                        }
                        entry
                    }
                }
            }
        };
        entries.push(entry);
    }
    for row in variables {
        if placed.contains(row.variable.name.as_str()) {
            continue;
        }
        entries.push(variable_entry(
            row,
            stored_entry(row.variable.name.as_str()),
            page.not_applying.get(&row.variable.name).copied(),
            false,
            None,
        ));
    }
    let offered = entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect::<BTreeSet<_>>();
    let orphans = entries_stored
        .iter()
        .filter(|stored| !offered.contains(stored.name.as_str()))
        .map(|stored| GoalOrphan {
            name: stored.name.clone(),
            value_json: stored.value_json.clone(),
            set: Some(set_by(stored.set)),
        })
        .collect();
    let other_targets = targets
        .iter()
        .filter(|stored| !declared_pools.contains(stored.target.resource.as_str()))
        .map(|stored| stored.target.id.clone())
        .collect();
    ViewEntries {
        entries,
        orphans,
        other_targets,
    }
}

/// The stored entry of `name` among `entries`.
fn stored_entry_of<'a>(entries: &'a [StoredGoalEntry], name: &str) -> Option<&'a StoredGoalEntry> {
    entries.iter().find(|entry| entry.name == name)
}

/// A `resource` entry: its pool, the stored target on it (whatever its id) as its value, the pool's
/// reading as the application's targetable pools show it, and the target's gap when its condition
/// is computed.
fn resource_entry(
    name: &str,
    pool: &str,
    targets: &[StoredGoalTarget],
    page: &GoalPage,
    conditions: &[ResourceTargetCondition],
    raw_entry: Option<Value>,
) -> GoalViewEntry {
    let mut entry = GoalViewEntry::new(name, GoalEntryType::Resource, true);
    entry.entry = raw_entry;
    entry.state = Some(GoalEntryState::InEffect);
    let target = targets.iter().find(|stored| stored.target.resource == pool);
    let mut state = GoalResourceState {
        pool: pool.to_owned(),
        target_id: None,
        observation: page
            .targetable
            .iter()
            .find(|targetable| targetable.resource == pool)
            .map(|targetable| targetable.observation.clone()),
        gap: None,
    };
    match target {
        Some(stored) => {
            let TargetCondition::AtLeast { amount } = stored.target.condition;
            let value = serde_json::json!({ "at_least": amount });
            entry.value_json = Some(variable_value_json(&value));
            entry.effective_json = entry.value_json.clone();
            entry.source = Some(GoalValueSource::Set);
            entry.set = stored.set.map(set_by);
            state.target_id = Some(stored.target.id.clone());
            state.gap = conditions
                .iter()
                .find(|condition| condition.target_id == stored.target.id)
                .and_then(|condition| match condition.state {
                    ResourceTargetConditionState::Computed { gap, .. } => Some(gap),
                    ResourceTargetConditionState::AwaitingObservation { .. } => None,
                });
        }
        None => entry.source = Some(GoalValueSource::Default),
    }
    entry.resource = Some(state);
    entry
}

/// A variable's entry: its declaration (with `default_origin` `model` when it says so), the
/// packages that declare it, its default, the stored value and who set it, the value the next run
/// reads with its source and its word (the run's own resolution, `row.resolved`; a variable that
/// does not apply in the game root, `not_applying`, says so even with no stored value), the stored
/// keys of an `ordered` value the run leaves out, and whether it is in effect: `not_applied` when
/// the run reads the default with a word (a stored value not applied, or a variable that does not
/// apply), `in_effect` otherwise (`keys_left_out`, a value applied in part, included).
fn variable_entry(
    row: &PageVariable<'_>,
    stored: Option<&StoredGoalEntry>,
    not_applying: Option<VariableProblem>,
    placed: bool,
    raw_entry: Option<Value>,
) -> GoalViewEntry {
    let variable = row.variable;
    let mut entry = GoalViewEntry::new(variable.name.as_str(), GoalEntryType::Variable, placed);
    entry.entry = raw_entry;
    entry.variable = Some(variable.declaration.clone());
    if variable.default_origin == DefaultOrigin::Model {
        entry.default_origin = Some(DefaultOrigin::Model.as_str().to_owned());
    }
    entry.packages.clone_from(&row.packages);
    let default = variable.default_value();
    entry.default_json = Some(variable_value_json(&default));
    if let Some(stored) = stored {
        entry.value_json = Some(stored.value_json.clone());
        entry.set = Some(set_by(stored.set));
    }
    let (effective, source, problem) = match row.resolved {
        Some(resolved) => (
            resolved.value.clone(),
            resolved.source,
            resolved.problem.or(not_applying),
        ),
        None => (default, VariableSource::Default, not_applying),
    };
    entry.source = Some(match source {
        VariableSource::Set => GoalValueSource::Set,
        VariableSource::Default => GoalValueSource::Default,
    });
    // The keys of a stored `ordered` value the run's value does not hold: the ones it left out.
    if let (VariableKind::Ordered { .. }, Some(stored), VariableSource::Set) =
        (&variable.kind, stored, source)
    {
        let read = effective.as_array().map(Vec::as_slice).unwrap_or_default();
        entry.removed_keys = stored
            .value
            .as_array()
            .into_iter()
            .flatten()
            .filter(|key| !read.contains(key))
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
    }
    entry.display = display(variable, &effective);
    entry.effective_json = Some(variable_value_json(&effective));
    entry.variable_problem = problem.map(|problem| problem.as_str().to_owned());
    entry.state = Some(match (source, problem) {
        (VariableSource::Default, Some(_)) => GoalEntryState::NotApplied,
        _ => GoalEntryState::InEffect,
    });
    entry
}

/// An entry of an accepted `goals.json` as its reader read it: its name, type, block, texts, and
/// the members of its type (a resource entry's pool and rate, a tier's rules, options and
/// default).
pub(crate) fn declared_entry(goal: &DeclaredGoal) -> Value {
    let text = |text: &GoalText| {
        Value::Object(
            text.iter()
                .map(|(locale, written)| (locale.clone(), Value::String(written.clone())))
                .collect(),
        )
    };
    let mut entry = serde_json::Map::new();
    entry.insert("name".to_owned(), Value::String(goal.name.clone()));
    entry.insert(
        "type".to_owned(),
        Value::String(goal.goal_type().as_str().to_owned()),
    );
    if let Some(block) = goal.block {
        entry.insert("block".to_owned(), Value::String(block.as_str().to_owned()));
    }
    if let Some(title) = &goal.title {
        entry.insert("title".to_owned(), text(title));
    }
    if let Some(help) = &goal.help {
        entry.insert("help".to_owned(), text(help));
    }
    match &goal.kind {
        DeclaredGoalKind::Resource { pool, rate } => {
            entry.insert("governs".to_owned(), serde_json::json!({ "pool": pool }));
            if let Some(rate) = rate {
                entry.insert(
                    "rate".to_owned(),
                    serde_json::json!({
                        "floor_milli_per_day": rate.floor_milli_per_day,
                        "ceiling_milli_per_day": rate.ceiling_milli_per_day,
                    }),
                );
            }
        }
        DeclaredGoalKind::Tier {
            rules,
            options,
            default,
        } => {
            entry.insert("governs".to_owned(), serde_json::json!({ "rules": rules }));
            entry.insert(
                "options".to_owned(),
                Value::Array(
                    options
                        .iter()
                        .map(|option| {
                            serde_json::json!({
                                "value": option.value,
                                "title": text(&option.title),
                                "scale_milli": option.scale_milli,
                            })
                        })
                        .collect(),
                ),
            );
            entry.insert("default".to_owned(), Value::String(default.clone()));
        }
        DeclaredGoalKind::Heading | DeclaredGoalKind::Note | DeclaredGoalKind::Variable => {}
    }
    Value::Object(entry)
}

/// `value` in the declaration's own texts, by locale: a `choice` value's option title, an
/// `integer` value with its unit's text; none for another kind, a time unit or no unit.
fn display(variable: &PackageVariable, value: &Value) -> Option<BTreeMap<String, String>> {
    match (&variable.kind, value) {
        (VariableKind::Choice { options, .. }, Value::String(chosen)) => options
            .iter()
            .find(|option| option.value == *chosen)
            .map(|option| option.title.iter().cloned().collect()),
        (
            VariableKind::Integer {
                unit: Some(GoalUnit::Text(text)),
                ..
            },
            Value::Number(number),
        ) => Some(
            text.iter()
                .map(|(locale, unit)| (locale.clone(), format!("{number} {unit}")))
                .collect(),
        ),
        _ => None,
    }
}
