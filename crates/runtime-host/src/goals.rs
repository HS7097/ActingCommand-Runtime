// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463 V3, which is Workflow #455 G-R2b with the variables in it (register §1.383;
//! `contracts/runtime-interface.md`, "Goal request kinds"; `contracts/package-variables.md`,
//! "Reading and setting a value by name"): what the goal page's two requests decide, as pure
//! functions of what the host read.
//!
//! - **The page** ([`GoalPage`]): the application's game, whether what the page is judged by is
//!   in memory (fix round 1: until it is, every edit is `not_checked_yet` and the view lists
//!   nothing; fix round 2: or `root_failing` or `no_schedule`, one word per cause), what the
//!   scheduler workbench holds of its `goals.json` (an accepted file only; a
//!   refused file applies no entry), the packages the active catalog binds on the application (a
//!   catalog task whose scope covers it, and the package the dispatch's table or the remedy table
//!   holds for that task) with their variables and remedy parameters, the pools it can target
//!   with their capacities, which variables do not apply in its game root (the word a dispatch
//!   reads) and, fix round 1, the root's judgement of each of its names (its packages and the
//!   detail of why it does not apply).
//! - **The checks** ([`check_edit`]), which the host runs under the fact write gate after it
//!   compares the version: the name and the value of an `EditGoal` against the page and the stored
//!   record, each refusal one [`GoalRefusal`] with the word of [`GoalProblem`]; a change that
//!   passes is one [`GoalRecordEdit`]. Fix round 1: a variable's value is judged by the dispatch's
//!   own function (`resolve_variable`), so what `set` takes is what a dispatch applies whole; a
//!   `clear` removes a stored row whatever its name's entry is.
//! - **The view** ([`view_entries`]): every entry of the page in its order, the stored values no
//!   entry shows, and the stored targets that are no declared resource entry's.
//!
//! Nothing here reads a clock, a file, a lock or the ledger.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::{
    DeclaredGoal, DeclaredGoalKind, DefaultOrigin, GoalEntryState, GoalEntryType, GoalOrphan,
    GoalProblem, GoalResourceState, GoalSetBy, GoalText, GoalUnit, GoalValueSource,
    GoalVariableCause, GoalViewEntry, MAX_GOAL_SAFE_INTEGER, PackageVariable,
    ResourceTargetCondition, ResourceTargetConditionState, RootVariableVerdict, StandardPackGoals,
    TargetableResource, VariableCause, VariableKind, VariableProblem, VariableSource, goal_name,
    variable_value_json,
};
use actingcommand_policy::{
    GoalRecordEdit, GoalRecordRefusal, ResolvedVariable, StoredEntry, StoredGoalEntry,
    StoredGoalRecord, StoredGoalTarget, TargetCondition, TargetSetter, VariableSetBy,
    goal_entry_value_fits, resolve_variable,
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
    /// Fix round 1: whether what the page is judged by is in the Runtime's memory (with a
    /// composed standard pack the scheduler workbench's check of the game root, and the remedy
    /// table the last evaluation built for the active catalog and procedure manifest). When it is
    /// not, the page holds nothing else, the view lists nothing and every edit is refused. Fix
    /// round 2: `None` when it is; else the one word of the cause every edit is refused with:
    /// `not_checked_yet` (not in memory yet, which passes by itself), `root_failing` (the
    /// workbench checks the game root and no check since the start could read it) or
    /// `no_schedule` (the game has no standard pack root with `schedule.json`).
    pub(crate) unchecked: Option<GoalProblem>,
    /// The game's `goals.json` when the workbench holds it accepted.
    pub(crate) declaration: Option<StandardPackGoals>,
    /// The bound packages, each once, in the order of the catalog task that first runs it.
    pub(crate) packages: Vec<BoundPackage>,
    /// The pools the application can target, as `ResourceTargetView` judges them.
    pub(crate) targetable: Vec<TargetableResource>,
    /// The capacity of each targetable pool whose capacity reads one, by pool id.
    pub(crate) capacities: BTreeMap<String, u64>,
    /// Each variable name of the bound packages that does not apply in the game root, with the
    /// word that says why (the word a dispatch reads from its table, `root_word`).
    pub(crate) not_applying: BTreeMap<String, VariableProblem>,
    /// Fix round 1: the game root's judgement of each of its variable names, by name (every
    /// package of the root that declares it, its first declaration and why it does not apply),
    /// as the workbench's last check of the root holds it; empty without a composed standard pack.
    pub(crate) root: BTreeMap<String, RootVariableVerdict>,
}

impl GoalPage {
    /// Fix round 1: the page of `game_id` while what it is judged by is not in memory yet. Fix
    /// round 2: for the cause `word` (`not_checked_yet`, `root_failing` or `no_schedule`).
    pub(crate) fn not_checked(game_id: String, word: GoalProblem) -> Self {
        Self {
            game_id,
            unchecked: Some(word),
            declaration: None,
            packages: Vec::new(),
            targetable: Vec::new(),
            capacities: BTreeMap::new(),
            not_applying: BTreeMap::new(),
            root: BTreeMap::new(),
        }
    }

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

    /// Fix round 1: whether `name` is declared in the game root (a placement of the accepted
    /// `goals.json`, or a variable a package of the root declares) but by no bound package, so
    /// it is not offered on this application (`not_offered`).
    fn not_offered(&self, name: &str) -> bool {
        let placed = self
            .declaration
            .as_ref()
            .and_then(|declaration| declaration.entry(name))
            .is_some_and(|declared| declared.kind == DeclaredGoalKind::Variable);
        self.variable(name).is_none() && (placed || self.root.contains_key(name))
    }
}

/// Why an `EditGoal` was refused: the word, and the values that locate it (`json_pointer` within
/// the value, `range_min` and `range_max`, `pool_id`, `variable_problem`).
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

/// Workflow #463 V3 (`contracts/runtime-interface.md`, "Goal request kinds", the checks): the
/// change `request` makes to the record `stored` of the application whose page is `page`, the row
/// set by `set_by`, or why it is refused. The host runs it under the fact write gate, after it
/// compared the version, on the record it is about to change. A name outside an entry name's form
/// is `name_unknown`, a `set` whose value is no JSON `value_not_json`; then (fix round 1) a page
/// not in memory yet refuses every request `not_checked_yet` (fix round 2: or, for a game root
/// that fails its checks or a game without `schedule.json`, `root_failing` or `no_schedule`). A
/// `set` is judged by [`check_set`], a `clear` by [`check_clear`].
pub(crate) fn check_edit(
    page: &GoalPage,
    stored: &StoredGoalRecord,
    request: EditRequest<'_>,
    set_by: VariableSetBy,
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
    if let Some(word) = page.unchecked {
        return Err(GoalRefusal::new(word));
    }
    let declared = page
        .declaration
        .as_ref()
        .and_then(|declaration| declaration.entry(name))
        .map(|declared| &declared.kind);
    match &value {
        Some(value) => check_set(page, name, declared, value, set_by),
        None => check_clear(page, stored, name, declared),
    }
}

/// A `set` of `name` (whose entry of the accepted `goals.json` is `declared`) to `value`, in this
/// order:
///
/// 1. an entry of the accepted `goals.json`: a `heading` or a `note` holds no value
///    (`holds_no_value`); a `tier` is `tier_not_built`; a `resource` entry takes `{at_least}` on
///    its pool, as an other target does (below), and (Workflow #455 B3) its goal carries the
///    `scale` and `importance_milli` the entry carries; a `variable` placement is the variable of
///    its name (2);
/// 2. a variable of a bound package: one that does not apply is `variable_not_applying` with its
///    `variable_problem`; else its value is judged by the dispatch's own function
///    ([`judge_value`]) and held to the store's bound;
/// 3. a remedy parameter of a bound package: `remedy_parameter`;
/// 4. fix round 1: a name the game root declares (a placement, or a variable of a package of the
///    root) that no bound package declares: `not_offered`, the word the view lists it with;
/// 5. an other target: the value `{resource, at_least}` on a targetable pool, as [`check_target`]
///    judges it;
/// 6. else `name_unknown`.
fn check_set(
    page: &GoalPage,
    name: &str,
    declared: Option<&DeclaredGoalKind>,
    value: &Value,
    set_by: VariableSetBy,
) -> Result<GoalRecordEdit, GoalRefusal> {
    match declared {
        Some(DeclaredGoalKind::Heading | DeclaredGoalKind::Note) => {
            return Err(GoalRefusal::new(GoalProblem::HoldsNoValue));
        }
        Some(DeclaredGoalKind::Tier { .. }) => {
            return Err(GoalRefusal::new(GoalProblem::TierNotBuilt));
        }
        Some(DeclaredGoalKind::Resource {
            pool,
            scale,
            importance_milli,
            ..
        }) => {
            // Workflow #455 B3: the goal carries the step and the importance the entry carries.
            let own = OwnWeights {
                scale: *scale,
                importance_milli: *importance_milli,
            };
            let at_least = check_target(page, value, Some(pool), own)?;
            return Ok(GoalRecordEdit::SetTarget {
                id: name.to_owned(),
                resource: pool.clone(),
                at_least,
                scale: own.scale,
                importance_milli: own.importance_milli,
            });
        }
        Some(DeclaredGoalKind::Variable) | None => {}
    }
    if let Some(variable) = page.variable(name) {
        if let Some(problem) = page.not_applying.get(name) {
            let mut refusal = GoalRefusal::new(GoalProblem::VariableNotApplying);
            refusal.variable_problem = Some(*problem);
            return Err(refusal);
        }
        judge_value(variable, value, set_by)?;
        fits_the_store(value)?;
        return Ok(GoalRecordEdit::SetEntry {
            name: name.to_owned(),
            value: value.clone(),
        });
    }
    if page.parameter(name) {
        return Err(GoalRefusal::new(GoalProblem::RemedyParameter));
    }
    if page.not_offered(name) {
        return Err(GoalRefusal::new(GoalProblem::NotOffered));
    }
    let Some(resource) = value.get("resource") else {
        return Err(GoalRefusal::new(GoalProblem::NameUnknown));
    };
    let resource = resource.as_str().unwrap_or_default().to_owned();
    let at_least = check_target(page, value, None, OwnWeights::default())?;
    Ok(GoalRecordEdit::SetTarget {
        id: name.to_owned(),
        resource,
        at_least,
        scale: None,
        importance_milli: None,
    })
}

/// A `clear` of `name` (whose entry of the accepted `goals.json` is `declared`). Fix round 1: a
/// value nobody can remove must not exist, so what is stored under the name is removed whatever
/// the name's entry is: a resource entry's target on its pool (whatever its id), else the stored
/// entry row of the name, else the stored target of that id. With nothing stored under it the
/// name's own word answers: `holds_no_value` for a heading or a note, `tier_not_built` for a tier,
/// `value_not_set` for a resource entry or a variable of a bound package, `remedy_parameter`,
/// `not_offered` for a name the root declares that no bound package declares, else
/// `name_unknown`.
fn check_clear(
    page: &GoalPage,
    stored: &StoredGoalRecord,
    name: &str,
    declared: Option<&DeclaredGoalKind>,
) -> Result<GoalRecordEdit, GoalRefusal> {
    let targets = stored.targets();
    if let Some(DeclaredGoalKind::Resource { pool, .. }) = declared
        && let Some(target) = targets.iter().find(|kept| kept.target.resource == *pool)
    {
        return Ok(GoalRecordEdit::ClearTarget {
            id: target.target.id.clone(),
        });
    }
    if stored.entries().iter().any(|entry| entry.name == name) {
        return Ok(GoalRecordEdit::ClearEntry {
            name: name.to_owned(),
        });
    }
    if targets.iter().any(|kept| kept.target.id == name) {
        return Ok(GoalRecordEdit::ClearTarget {
            id: name.to_owned(),
        });
    }
    let problem = match declared {
        Some(DeclaredGoalKind::Heading | DeclaredGoalKind::Note) => GoalProblem::HoldsNoValue,
        Some(DeclaredGoalKind::Tier { .. }) => GoalProblem::TierNotBuilt,
        Some(DeclaredGoalKind::Resource { .. }) => GoalProblem::ValueNotSet,
        Some(DeclaredGoalKind::Variable) | None if page.variable(name).is_some() => {
            GoalProblem::ValueNotSet
        }
        Some(DeclaredGoalKind::Variable) | None if page.parameter(name) => {
            GoalProblem::RemedyParameter
        }
        Some(DeclaredGoalKind::Variable) | None if page.not_offered(name) => {
            GoalProblem::NotOffered
        }
        Some(DeclaredGoalKind::Variable) | None => GoalProblem::NameUnknown,
    };
    Err(GoalRefusal::new(problem))
}

/// Whether the store keeps `value` (its canonical text at most 1024 bytes).
fn fits_the_store(value: &Value) -> Result<(), GoalRefusal> {
    goal_entry_value_fits(value).map_err(|refusal| {
        GoalRefusal::of_record(refusal).unwrap_or_else(|| GoalRefusal::new(GoalProblem::WrongType))
    })
}

/// Fix round 1 (the coordinator's ruling: what `set` takes is exactly what a dispatch applies):
/// `value`, set on the application by `set_by`, judged by the dispatch's own function
/// (`resolve_variable`, the policy crate's), as the application's stored entry of the variable
/// with no word of the root (the caller has found that the variable applies). It is taken when
/// the run would read it as it is set; any other answer refuses it with [`write_refusal`] of the
/// judgement's word, an `ordered` value whose keys the run would leave out included.
fn judge_value(
    variable: &PackageVariable,
    value: &Value,
    set_by: VariableSetBy,
) -> Result<(), GoalRefusal> {
    let entry = StoredEntry {
        value: value.clone(),
        set_by: Some(set_by),
    };
    let resolved = resolve_variable(variable, Some(&entry), None);
    match (resolved.source, resolved.problem) {
        (VariableSource::Set, None) => Ok(()),
        (_, problem) => Err(write_refusal(variable, value, problem)),
    }
}

/// The refusal of a write whose value the dispatch's judgement answers with `problem`, in the
/// words of a write: `not_an_option` for `not_an_option`, `keys_removed` and `keys_left_out` (a
/// key that is none of the options), `out_of_range` and `off_step` with the variable's `min` and
/// `max`, `key_repeated`, `count_out_of_range` with `min_count` and `max_count`, and `wrong_type`
/// for `type_changed` (no other word comes of a value whose setter is known and whose variable
/// applies). An `ordered` value's item the word is about is named by its pointer
/// ([`item_at`]).
fn write_refusal(
    variable: &PackageVariable,
    value: &Value,
    problem: Option<VariableProblem>,
) -> GoalRefusal {
    use VariableProblem as Judged;
    let word = match problem {
        Some(Judged::NotAnOption | Judged::KeysRemoved | Judged::KeysLeftOut) => {
            GoalProblem::NotAnOption
        }
        Some(Judged::OutOfRange) => GoalProblem::OutOfRange,
        Some(Judged::OffStep) => GoalProblem::OffStep,
        Some(Judged::KeyRepeated) => GoalProblem::KeyRepeated,
        Some(Judged::CountOutOfRange) => GoalProblem::CountOutOfRange,
        _ => GoalProblem::WrongType,
    };
    let mut refusal = GoalRefusal::new(word);
    match (&variable.kind, word) {
        (
            VariableKind::Integer { min, max, .. },
            GoalProblem::OutOfRange | GoalProblem::OffStep,
        ) => refusal = refusal.within(*min, *max),
        (
            VariableKind::Ordered {
                min_count,
                max_count,
                ..
            },
            GoalProblem::CountOutOfRange,
        ) => {
            refusal = refusal.within(
                i64::try_from(*min_count).unwrap_or(i64::MAX),
                i64::try_from(*max_count).unwrap_or(i64::MAX),
            );
        }
        _ => {}
    }
    if let Some(index) = item_at(variable, value, word) {
        refusal = refusal.at(format!("/{index}"));
    }
    refusal
}

/// Where in an `ordered` variable's `value` the refusal `word` is: the first item that is no
/// string (`wrong_type`), the first key named a second time (`key_repeated`), or the first key that
/// is none of the options (`not_an_option`); `None` for another kind or word. It only says where:
/// the word is the dispatch's judgement's.
fn item_at(variable: &PackageVariable, value: &Value, word: GoalProblem) -> Option<usize> {
    let (VariableKind::Ordered { options, .. }, Value::Array(items)) = (&variable.kind, value)
    else {
        return None;
    };
    match word {
        GoalProblem::WrongType => items.iter().position(|item| !item.is_string()),
        GoalProblem::KeyRepeated => {
            (0..items.len()).find(|index| items[..*index].contains(&items[*index]))
        }
        GoalProblem::NotAnOption => items.iter().position(|item| {
            item.as_str()
                .is_none_or(|key| !options.iter().any(|option| option.value == key))
        }),
        _ => None,
    }
}

/// Workflow #455 B3 (register §1.519): the step and the importance a `resource` entry of a version
/// 3 `goals.json` carries, which the goal it sets carries as its own `scale` and
/// `importance_milli`; none for an entry that carries none and for an other target.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct OwnWeights {
    pub(crate) scale: Option<u64>,
    pub(crate) importance_milli: Option<u64>,
}

/// Workflow #463 V3: a resource value, the `at_least` it holds, or why it is refused. For a
/// resource entry (`pool` its `governs.pool`) the value is `{at_least}`; for an other target it is
/// `{resource, at_least}`. Every other member is `wrong_type` at it, but `by_unix_ms`, which is
/// `deadline_unsupported`; `at_least` is a whole number (else `wrong_type`) from 1 to the safe
/// integers and not above the pool's capacity when its capacity reads one (else `out_of_range`);
/// the pool is one the application can target (else `pool_not_targetable`) whose valuation gives
/// the step and the importance a goal by name does not carry (else `pool_unvalued`). Workflow #455
/// B3: what the goal carries itself (`own`, a resource entry's) the pool's valuation need not
/// give: `pool_unvalued` only for a step or an importance the pool requires and `own` lacks.
pub(crate) fn check_target(
    page: &GoalPage,
    value: &Value,
    pool: Option<&String>,
    own: OwnWeights,
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
    if (targetable.scale_required && own.scale.is_none())
        || (targetable.importance_required && own.importance_milli.is_none())
    {
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
/// run ([`ResolvedVariable`], the run's own resolution), when one was resolved. Workflow #500 SV-2
/// (register §1.511): the resolution is the one of the value set on the application and the
/// package default alone, and `carried` says whether a catalog task bound on the application
/// carries a value of its own for the variable (its rule's `variables`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PageVariable<'a> {
    pub(crate) variable: &'a PackageVariable,
    pub(crate) packages: Vec<String>,
    pub(crate) resolved: Option<&'a ResolvedVariable>,
    pub(crate) carried: bool,
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
                .iter()
                .position(|row| row.variable.name == variable.name)
            {
                Some(index) => {
                    if !rows[index].packages.contains(&package.package_id) {
                        rows[index].packages.push(package.package_id.clone());
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
                    carried: false,
                }),
            }
        }
    }
    rows
}

/// What the view lists: the entries in their order, the stored values no entry shows, and the ids
/// of the stored targets that are no declared resource entry's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
///   `not_built`; a placement with its variable ([`variable_entry`]), or, fix round 1, when no
///   bound package declares it, `not_offered` with what the root declares of it
///   ([`not_offered_entry`]);
/// - then each variable no entry places, in the order the bound packages declare them;
/// - the orphans: each stored value that no `tier`, no placement and no listed variable shows (a
///   value stored under a heading's, a note's or a resource entry's name is one, so the view
///   shows every stored value and a `clear` of its name removes it);
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
    let mut shown = BTreeSet::new();
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
                shown.insert(declared.name.as_str());
                entry
            }
            DeclaredGoalKind::Variable => {
                shown.insert(declared.name.as_str());
                let verdict = page.root.get(&declared.name);
                match variables
                    .iter()
                    .find(|row| row.variable.name == declared.name)
                {
                    Some(row) => variable_entry(
                        row,
                        stored_entry(declared.name.as_str()),
                        page.not_applying.get(&declared.name).copied(),
                        verdict,
                        true,
                        raw_entry,
                    ),
                    None => not_offered_entry(
                        declared.name.as_str(),
                        stored_entry(declared.name.as_str()),
                        verdict,
                        raw_entry,
                    ),
                }
            }
        };
        entries.push(entry);
    }
    for row in variables {
        let name = row.variable.name.as_str();
        if shown.contains(name) {
            continue;
        }
        shown.insert(name);
        entries.push(variable_entry(
            row,
            stored_entry(name),
            page.not_applying.get(name).copied(),
            page.root.get(name),
            false,
            None,
        ));
    }
    let orphans = entries_stored
        .iter()
        .filter(|stored| !shown.contains(stored.name.as_str()))
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

/// A variable's entry: its declaration (with `default_origin` `model` when it says so), the bound
/// packages that declare it and (fix round 1) the root's other packages that declare it
/// (`unbound_packages`), its default, the stored value and who set it, the value the next run
/// reads with its source and its word (the run's own resolution, `row.resolved`; a variable that
/// does not apply in the game root, `not_applying`, says so even with no stored value) and, fix
/// round 1, the detail of the root's judgement when the word is the root's (`variable_cause`), the
/// stored keys of an `ordered` value the run leaves out, and whether it is in effect:
/// `not_applied` when the run reads the default with a word (a stored value not applied, or a
/// variable that does not apply), `in_effect` otherwise (`keys_left_out`, a value applied in part,
/// included). Workflow #500 SV-2 (register §1.511): of a variable a bound task's rule carries a
/// value for (`row.carried`), with no stored value applied, the rules decide: no source, value or
/// display is named.
fn variable_entry(
    row: &PageVariable<'_>,
    stored: Option<&StoredGoalEntry>,
    not_applying: Option<VariableProblem>,
    verdict: Option<&RootVariableVerdict>,
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
    if let Some(verdict) = verdict {
        entry.unbound_packages = verdict
            .packages
            .iter()
            .filter(|package| !row.packages.contains(package))
            .cloned()
            .collect();
    }
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
    // Workflow #500 SV-2 (register §1.511): a set value that is applied is what every next
    // dispatch hands its run (a rule that carries another value is held back); without one, a
    // variable a bound task carries a value for is decided by the rule of the task that runs, so
    // the page names no one value: no `source`, `effective_json` or `display`.
    let shown = match source {
        VariableSource::Set => Some(GoalValueSource::Set),
        VariableSource::Default if !row.carried => Some(GoalValueSource::Default),
        VariableSource::Default | VariableSource::Rule => None,
    };
    entry.source = shown;
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
    if shown.is_some() {
        entry.display = display(variable, &effective);
        entry.effective_json = Some(variable_value_json(&effective));
    }
    entry.variable_problem = problem.map(|problem| problem.as_str().to_owned());
    // The root's detail, when the word the run gives is the root's own (not `root_unchecked`,
    // which a root that fails its check gives every name, nor a stored value's own word).
    entry.variable_cause = verdict
        .and_then(|verdict| verdict.not_applying.as_ref())
        .filter(|cause| Some(cause.problem) == problem)
        .and_then(|cause| goal_cause(&cause.cause));
    entry.state = Some(match (source, problem) {
        (VariableSource::Default, Some(_)) => GoalEntryState::NotApplied,
        _ => GoalEntryState::InEffect,
    });
    entry
}

/// Fix round 1: a placement of `goals.json` whose variable no bound package declares, `not_offered`
/// (an `EditGoal` that sets it is refused with the word of the same name): the entry as its reader
/// read it, what the root declares of it (its first declaration and the root's packages that
/// declare it, none of them bound on the application) and any value stored under its name, which a
/// `clear` removes.
fn not_offered_entry(
    name: &str,
    stored: Option<&StoredGoalEntry>,
    verdict: Option<&RootVariableVerdict>,
    raw_entry: Option<Value>,
) -> GoalViewEntry {
    let mut entry = GoalViewEntry::new(name, GoalEntryType::Variable, true);
    entry.entry = raw_entry;
    entry.state = Some(GoalEntryState::NotOffered);
    if let Some(verdict) = verdict {
        entry.variable = Some(verdict.declaration.declaration.clone());
        entry.unbound_packages.clone_from(&verdict.packages);
    }
    if let Some(stored) = stored {
        entry.value_json = Some(stored.value_json.clone());
        entry.set = Some(set_by(stored.set));
    }
    entry
}

/// Fix round 1: the detail of why a variable does not apply in the root, as the view shows it; none
/// for a word without a detail.
fn goal_cause(cause: &VariableCause) -> Option<GoalVariableCause> {
    Some(match cause {
        VariableCause::DeclarationConflict { packages } => GoalVariableCause::DeclarationConflict {
            packages: packages.clone(),
        },
        VariableCause::GoalEntry { goal_type } => GoalVariableCause::GoalEntry {
            goal_type: goal_type.map(|goal_type| goal_type.as_str().to_owned()),
        },
        VariableCause::NameOutOfLimits { position } => GoalVariableCause::NameOutOfLimits {
            position: u64::try_from(*position).unwrap_or(u64::MAX),
        },
        VariableCause::CapRange { packages, member } => GoalVariableCause::CapRange {
            packages: packages.clone(),
            member: (*member).to_owned(),
        },
        VariableCause::WordOnly => return None,
    })
}

/// An entry of an accepted `goals.json` as its reader read it: its name, type, block, texts, and
/// the members of its type (a resource entry's pool and rate, and, Workflow #455 B3, its own
/// `scale` and `importance_milli` when it carries them; a tier's rules, options and default).
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
        DeclaredGoalKind::Resource {
            pool,
            rate,
            scale,
            importance_milli,
        } => {
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
            // Workflow #455 B3: the goal's own step and importance, as the entry carries them.
            if let Some(scale) = scale {
                entry.insert("scale".to_owned(), Value::from(*scale));
            }
            if let Some(importance_milli) = importance_milli {
                entry.insert(
                    "importance_milli".to_owned(),
                    Value::from(*importance_milli),
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
