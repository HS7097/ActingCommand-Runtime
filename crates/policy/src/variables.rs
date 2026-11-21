// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463 V2 (register §1.382; `contracts/package-variables.md`, "The run"): the values a
//! selected dispatch hands its run for the variables its package declares.
//!
//! - **The table** ([`VariableTable`]), memory only. Fix round 1 (the coordinator's ruling: the
//!   judgement of "does not apply" is the root check's, over the whole root): with a composed
//!   standard pack the host's scheduler workbench builds it ([`VariableTable::of_roots`]) from the
//!   judgement of each game root it read ([`RootVariableJudgement`]): the contract crate's
//!   [`judge_root_variables`] over every package of the registry, held ones included, against the
//!   root's `goals.json` as its reader read it (Workflow #455 G-SLx, register §1.501: no longer
//!   with the root's judgement of irreversibility); it is built again at each check of the
//!   workbench. Fix round 2: a game whose root no check judges now (its root failed its check or
//!   its composition is refused) reads every variable at its default with `root_unchecked`, and a
//!   dispatched package the workbench could not read is named ([`VariablePlan::unread`]). Without
//!   a composed standard pack the host's remedy table builds it once per table
//!   ([`VariableTable::build`]), as before: the catalog's bound packages, with each game's root
//!   judged once over the packages the table read for the tasks of that game's instances (a
//!   package the table could not read declares nothing the judgement reads). Each variable's
//!   declaration sha256 is computed when the table is built.
//! - **The resolution** ([`plan_variable_values`]): per (task, instance) pair whose package
//!   declares variables, per variable: the application's entry row of the variable's name in its
//!   `session.resource_targets` v3 record, as the evaluation's facts hold it, judged at use by
//!   [`resolve_variable`] (fix round 2: public, the one function a view calls too); else the
//!   package default. A variable the root's judgement keeps from applying is handed no value from
//!   outside. There is no third source: nothing here computes or rewrites a value.
//! - **The record** ([`RemedyPlan::with_variable_values`]): the pair's
//!   `policy_dispatch_parameters` gains `variable_values`, one record per variable of the
//!   package, which the evaluator attaches to a selected dispatch only. A pair whose package
//!   declares no variable gets nothing, so its reason chain is byte for byte as before. Fix round
//!   2: the form the ledger records it in stays within an outcome record's bound
//!   ([`recorded_dispatch_parameters`]).
//!
//! Resolution does not read `scheduling.remedy.enabled`: the host adds the values to the plan
//! whether remedies are on or off. Pure and deterministic: no clock, no file, no ledger.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::{ENVELOPE_BODY_BUDGET, Outcome, Record};
use actingcommand_contract::{
    MAX_VARIABLE_VALUE_JSON_BYTES, PackageVariable, RootGoalDocument, RootPackageVariables,
    RootVariableVerdict, RootVariablesJudged, RuntimeChannel, VariableProblem, VariableSource,
    judge_root_variables, variable_value_json,
};
use sha2::{Digest, Sha256};

use crate::codes::PolicyCode;
use crate::evaluator::scope_matches_instance;
use crate::resource_targets::{StoredEntry, set_channel, stored_v3_entries};
use crate::{
    CompiledCatalog, DecisionReason, EvaluationFacts, RemedyPlan, STANDARD_PACK_PROCEDURE_PREFIX,
    TaskSpec,
};

/// Fix round 2: the state of the files of a package of a game root the workbench never read
/// (its root failed at every check since the start), for [`UnreadVariables::files_state`].
pub const ROOT_NEVER_READ: &str = "root_never_read";

/// One catalog task as the host's table read it: the games of the instances its scope covered
/// when the table was built, and what its bound package declares (`None` when the table could
/// not read it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariableTableTask {
    pub task_id: String,
    pub games: BTreeSet<String>,
    pub package: Option<VariablePackage>,
}

/// What one bound package declares that the variables of a run depend on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariablePackage {
    pub package_id: String,
    pub variables: Vec<PackageVariable>,
    /// The fact keys its designated effects read their caps from.
    pub cap_fact_keys: Vec<String>,
}

/// One variable as the table holds it: its declaration and (fix round 1) that declaration's
/// sha256 ([`PackageVariable::declaration_sha256`]), computed once when the table is built.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TableVariable {
    variable: PackageVariable,
    declaration_sha256: String,
}

/// The variables of one package, in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskVariables {
    package_id: String,
    variables: Vec<TableVariable>,
}

impl TaskVariables {
    fn of(package_id: &str, variables: &[PackageVariable]) -> Self {
        Self {
            package_id: package_id.to_owned(),
            variables: variables
                .iter()
                .map(|variable| TableVariable {
                    variable: variable.clone(),
                    declaration_sha256: variable.declaration_sha256(),
                })
                .collect(),
        }
    }
}

/// Workflow #463 V2 fix round 1 (`contracts/package-variables.md`, "A variable that does not
/// apply"): one game root's variables as the host's scheduler workbench judged them over the
/// whole root: what each package of the registry it could read declares, held packages included,
/// and the names that do not apply there, each with its word. Fix round 2: the contract crate's
/// [`judge_root_variables`] gives them (the goal document as its reader read it included), with a
/// verdict per name ([`Self::verdicts`]: its packages, its declaration, whether it applies and the
/// detail of the cause), and the packages of the root the workbench could not read
/// ([`Self::unreadable_packages`]). Memory only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RootVariableJudgement {
    /// Each package that declares variables, by its registry id.
    packages: BTreeMap<String, TaskVariables>,
    not_applying: BTreeMap<String, VariableProblem>,
    /// Fix round 2: a verdict per name, in the order the root first declares it.
    verdicts: Vec<RootVariableVerdict>,
    /// Fix round 2: each package of the registry whose declaration could not be read, by its
    /// registry id, with the state of its files when the workbench read the root.
    unreadable: BTreeMap<String, String>,
}

impl RootVariableJudgement {
    /// The judgement of a root whose readable packages are `packages` (each by its registry id),
    /// judged as `judged` says ([`judge_root_variables`] of the same packages), whose packages
    /// that could not be read are `unreadable` (each with the state of its files, which says when
    /// it is the same unreadable package).
    pub fn new(
        packages: &[RootPackageVariables],
        judged: RootVariablesJudged,
        unreadable: BTreeMap<String, String>,
    ) -> Self {
        Self {
            packages: packages
                .iter()
                .filter(|package| !package.variables.is_empty())
                .map(|package| {
                    (
                        package.package_id.clone(),
                        TaskVariables::of(&package.package_id, &package.variables),
                    )
                })
                .collect(),
            not_applying: judged.not_applying(),
            verdicts: judged.verdicts,
            unreadable,
        }
    }

    /// The names of the root that do not apply, each with the word a record says it with.
    pub fn not_applying(&self) -> &BTreeMap<String, VariableProblem> {
        &self.not_applying
    }

    /// Fix round 2: a verdict per name of the root, in the order the root first declares it: the
    /// packages that declare it, its declaration, and why it does not apply with the detail of
    /// the cause. Whether a dispatch hands it a value also depends on whether a check judges the
    /// root now ([`VariableTable::root_word`]).
    pub fn verdicts(&self) -> &[RootVariableVerdict] {
        &self.verdicts
    }

    /// Fix round 2: the packages of the root whose declaration could not be read, by registry id,
    /// each with the state of its files.
    pub fn unreadable_packages(&self) -> &BTreeMap<String, String> {
        &self.unreadable
    }

    /// The variables the package `package_id` of the root declares, in declaration order; empty
    /// when it declares none or could not be read.
    pub fn package_variables(&self, package_id: &str) -> Vec<&PackageVariable> {
        self.packages
            .get(package_id)
            .map(|package| {
                package
                    .variables
                    .iter()
                    .map(|declared| &declared.variable)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Fix round 2 (`contracts/package-variables.md`, "A dispatched package whose variables cannot be
/// read"): a dispatched package the host's scheduler workbench could not read, so its variables
/// are not known: its runs read the package's defaults and the dispatch carries no value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadVariables {
    pub package_id: String,
    /// The state of its files when the workbench last read its root (an opaque text that changes
    /// when one of its declaration files does), or [`ROOT_NEVER_READ`]: what "once" of its
    /// warning is counted by, with the instance and the package.
    pub files_state: String,
}

/// The variables of the packages the dispatches run, and per game the names that do not apply
/// there. Built by the host's scheduler workbench with a composed standard pack
/// ([`VariableTable::of_roots`]), else once per remedy table ([`VariableTable::build`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VariableTable {
    /// Without a composed standard pack: each catalog task's package, by task id.
    tasks: BTreeMap<String, TaskVariables>,
    /// Fix round 1, with a composed standard pack: per game, each package of its root by the
    /// procedure a composed task runs it with (`package.<package_id>`).
    procedures: BTreeMap<String, BTreeMap<String, TaskVariables>>,
    games: BTreeMap<String, BTreeMap<String, VariableProblem>>,
    /// Fix round 2, with a composed standard pack: per game, each package of its root the
    /// workbench could not read, by procedure.
    unreadable: BTreeMap<String, BTreeMap<String, UnreadVariables>>,
    /// Fix round 2: the games whose root no check judges now: its root failed its check, or its
    /// composition is refused or not activated (the workbench's games without goals in effect).
    unjudged: BTreeSet<String>,
    /// Fix round 2: built by the workbench ([`Self::of_roots`]).
    composed: bool,
}

/// What the table holds of the package a task runs on an instance of a game.
enum TaskLookup<'a> {
    Declares(&'a TaskVariables),
    Unread(UnreadVariables),
    Nothing,
}

impl VariableTable {
    /// The mode without a composed standard pack (the host's remedy table): the table of `tasks`
    /// (in catalog order): per task whose package declares variables, its package and variables;
    /// per game any task covers, the judgement of the packages read for that game's tasks (each
    /// package once, in catalog order), with no `goals.json` (fix round 2: the contract crate's
    /// [`judge_root_variables`] with [`RootGoalDocument::Absent`]): one word per name that does
    /// not apply. A task whose package could not be read adds nothing to its game's judgement
    /// (Workflow #455 G-SLx, register §1.501: until 0.12.77 it counted as an unreadable package of
    /// the game, which kept every name of the game from applying, `package_unreadable`).
    pub fn build(tasks: &[VariableTableTask]) -> Self {
        let games = tasks
            .iter()
            .flat_map(|task| task.games.iter().map(String::as_str))
            .collect::<BTreeSet<_>>();
        let mut judged = BTreeMap::new();
        for game in games {
            let mut root = Vec::<RootPackageVariables>::new();
            let mut read = BTreeSet::<&str>::new();
            for task in tasks.iter().filter(|task| task.games.contains(game)) {
                let Some(package) = &task.package else {
                    continue;
                };
                if !read.insert(package.package_id.as_str()) {
                    continue;
                }
                root.push(RootPackageVariables {
                    package_id: package.package_id.clone(),
                    variables: package.variables.clone(),
                    cap_fact_keys: package.cap_fact_keys.clone(),
                });
            }
            judged.insert(
                game.to_owned(),
                judge_root_variables(&root, RootGoalDocument::Absent).not_applying(),
            );
        }
        let tasks = tasks
            .iter()
            .filter_map(|task| {
                let package = task.package.as_ref()?;
                (!package.variables.is_empty()).then(|| {
                    (
                        task.task_id.clone(),
                        TaskVariables::of(&package.package_id, &package.variables),
                    )
                })
            })
            .collect();
        Self {
            tasks,
            games: judged,
            ..Self::default()
        }
    }

    /// Workflow #463 V2 fix round 1: the table with a composed standard pack, of the judgement of
    /// each game root the host's scheduler workbench read, by game: a composed task of the game
    /// runs the package its procedure names (`package.<package_id>`), and reads its variables as
    /// the root's judgement holds them. Fix round 2: `unjudged` are the games whose root no check
    /// judges now, whose every variable reads its default with `root_unchecked`
    /// ([`Self::root_word`]); a composed task whose package the workbench could not read, or of a
    /// game it never read, is [`VariablePlan::unread`].
    pub fn of_roots<'a>(
        roots: impl IntoIterator<Item = (&'a String, &'a RootVariableJudgement)>,
        unjudged: &BTreeSet<String>,
    ) -> Self {
        let procedure = |package_id: &str| format!("{STANDARD_PACK_PROCEDURE_PREFIX}{package_id}");
        let mut table = Self {
            unjudged: unjudged.clone(),
            composed: true,
            ..Self::default()
        };
        for (game, root) in roots {
            table.procedures.insert(
                game.clone(),
                root.packages
                    .iter()
                    .map(|(package_id, package)| (procedure(package_id), package.clone()))
                    .collect(),
            );
            table.unreadable.insert(
                game.clone(),
                root.unreadable
                    .iter()
                    .map(|(package_id, files_state)| {
                        (
                            procedure(package_id),
                            UnreadVariables {
                                package_id: package_id.clone(),
                                files_state: files_state.clone(),
                            },
                        )
                    })
                    .collect(),
            );
            table.games.insert(game.clone(), root.not_applying.clone());
        }
        table
    }

    /// Whether no package of the table declares a variable.
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty() && self.procedures.values().all(BTreeMap::is_empty)
    }

    /// Fix round 2 (`contracts/package-variables.md`, "A variable that does not apply"): the word
    /// that keeps the variable `name` from applying on an instance of `game`, `None` when it
    /// applies: `root_unchecked` when no check judges the game's root now, else the root's
    /// judgement. A dispatch reads it, and a view reads it to say what a dispatch would read.
    pub fn root_word(&self, game: &str, name: &str) -> Option<VariableProblem> {
        if self.unjudged.contains(game) {
            return Some(VariableProblem::RootUnchecked);
        }
        match self.games.get(game) {
            Some(names) => names.get(name).copied(),
            None => Some(VariableProblem::RootUnchecked),
        }
    }

    /// What the table holds of the package `task` runs on an instance of `game`.
    fn lookup(&self, task: &TaskSpec, game: &str) -> TaskLookup<'_> {
        if let Some(declared) = self.tasks.get(&task.id) {
            return TaskLookup::Declares(declared);
        }
        if !self.composed {
            return TaskLookup::Nothing;
        }
        if let Some(declared) = self
            .procedures
            .get(game)
            .and_then(|procedures| procedures.get(&task.procedure_ref))
        {
            return TaskLookup::Declares(declared);
        }
        if let Some(unread) = self
            .unreadable
            .get(game)
            .and_then(|unreadable| unreadable.get(&task.procedure_ref))
        {
            return TaskLookup::Unread(unread.clone());
        }
        match task
            .procedure_ref
            .strip_prefix(STANDARD_PACK_PROCEDURE_PREFIX)
        {
            Some(package_id) if !self.games.contains_key(game) => {
                TaskLookup::Unread(UnreadVariables {
                    package_id: package_id.to_owned(),
                    files_state: ROOT_NEVER_READ.to_owned(),
                })
            }
            _ => TaskLookup::Nothing,
        }
    }

    /// Workflow #463 V3: the package the catalog task `task` runs on an instance of `game` and the
    /// variables it declares, in declaration order, as the table holds them: the one reading a
    /// dispatch makes (`lookup`), with or without a composed standard pack (fix round 1:
    /// without one, the table is the only memory that holds a task's variables, so the goal page
    /// reads them here; with one, they are also [`RootVariableJudgement::package_variables`] of
    /// the package the task's procedure names). `None` when the package declares none, the table
    /// could not read it, or it is none of the table's.
    pub fn declared_variables(
        &self,
        task: &TaskSpec,
        game: &str,
    ) -> Option<(&str, Vec<&PackageVariable>)> {
        match self.lookup(task, game) {
            TaskLookup::Declares(declared) => Some((
                declared.package_id.as_str(),
                declared
                    .variables
                    .iter()
                    .map(|declared| &declared.variable)
                    .collect(),
            )),
            TaskLookup::Unread(_) | TaskLookup::Nothing => None,
        }
    }
}

/// When and through which channel the value an application holds was set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VariableSetBy {
    pub set_unix_ms: u64,
    pub channel: RuntimeChannel,
}

/// One variable's value as a selected dispatch of its pair hands it to the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedVariable {
    pub variable_name: String,
    /// The value the run reads: the set value as judged, or the default.
    pub value: serde_json::Value,
    pub source: VariableSource,
    /// Who set the application's value, when it holds one, applied or not, and who set it could
    /// be read.
    pub set_by: Option<VariableSetBy>,
    /// Why the application's value was not applied (the source is then `default`); fix round 1:
    /// or `keys_left_out`, an `ordered` value applied in part (the source stays `set`). `None`
    /// when the value was applied whole or there is none.
    pub problem: Option<VariableProblem>,
}

impl ResolvedVariable {
    /// Fix round 1: why a value set on the application was not applied, the run reading the
    /// default; `None` when it was applied, whole or in part, or none is set.
    pub fn not_applied(&self) -> Option<VariableProblem> {
        match self.source {
            VariableSource::Default => self.problem,
            VariableSource::Set => None,
        }
    }
}

/// A value set on an application that a dispatch of the pair does not hand to its run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotAppliedValue {
    pub package_id: String,
    pub variable_name: String,
    /// The variable's declaration ([`PackageVariable::declaration_sha256`]).
    pub declaration_sha256: String,
    /// Fix round 1: `sha256:<hex>` of the stored value's compact JSON, so that another value not
    /// applied is warned once more.
    pub value_sha256: String,
    pub problem: VariableProblem,
}

/// The variables of one (task, instance) pair.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PairVariables {
    values: Vec<ResolvedVariable>,
    not_applied: Vec<NotAppliedValue>,
}

/// The variable values of one evaluation, per (task, instance) pair whose package declares
/// variables ([`plan_variable_values`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VariablePlan {
    pairs: BTreeMap<(String, String), PairVariables>,
    /// Fix round 2: each pair whose package the workbench could not read.
    unread: BTreeMap<(String, String), UnreadVariables>,
}

impl VariablePlan {
    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty() && self.unread.is_empty()
    }

    /// The values a dispatch of `task_id` on `instance_id` hands its run, in declaration order;
    /// `None` when its package declares no variable.
    pub fn values(&self, task_id: &str, instance_id: &str) -> Option<&[ResolvedVariable]> {
        self.pairs
            .get(&(task_id.to_owned(), instance_id.to_owned()))
            .map(|pair| pair.values.as_slice())
    }

    /// The values set on `instance_id` that a dispatch of `task_id` there does not hand its run.
    pub fn not_applied(&self, task_id: &str, instance_id: &str) -> &[NotAppliedValue] {
        self.pairs
            .get(&(task_id.to_owned(), instance_id.to_owned()))
            .map_or(&[], |pair| pair.not_applied.as_slice())
    }

    /// Fix round 2: the package of a dispatch of `task_id` on `instance_id` whose variables the
    /// workbench could not read: the dispatch carries no value and its run reads the package's
    /// defaults. `None` when the table knows the package (or it is none of the workbench's).
    pub fn unread(&self, task_id: &str, instance_id: &str) -> Option<&UnreadVariables> {
        self.unread
            .get(&(task_id.to_owned(), instance_id.to_owned()))
    }
}

/// Workflow #463 V2: the variable values of every (task, instance) pair of `catalog` on `facts`'
/// instances whose task's package declares variables in `table` and whose scope covers the
/// instance. Each variable, in declaration order, reads what [`resolve_variable`] answers for
/// the instance's entry row of its name (its v3 record's, [`stored_v3_entries`]) and the word
/// that keeps it from applying in the instance's game ([`VariableTable::root_word`]). A value not
/// applied is also one [`NotAppliedValue`] of the pair. Fix round 2: a pair whose package the
/// workbench could not read is [`VariablePlan::unread`] instead.
pub fn plan_variable_values(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    table: &VariableTable,
) -> VariablePlan {
    plan_with_entries(catalog, facts, table, |instance_id| {
        stored_v3_entries(facts, instance_id)
    })
}

/// [`plan_variable_values`] with the entry rows of each instance as `entries_of` gives them (the
/// v3 record's, [`stored_v3_entries`]; the gate tests give rows the v3 reader never passes),
/// read once per instance, when a pair of it reads a variable.
pub(crate) fn plan_with_entries(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    table: &VariableTable,
    entries_of: impl Fn(&str) -> BTreeMap<String, StoredEntry>,
) -> VariablePlan {
    let mut plan = VariablePlan::default();
    for instance in &facts.instances {
        let mut entries = None;
        for task in &catalog.catalog().tasks.tasks {
            let lookup = table.lookup(task, &instance.game_id);
            if matches!(lookup, TaskLookup::Nothing)
                || !scope_matches_instance(&task.scope, instance)
            {
                continue;
            }
            let pair = (task.id.clone(), instance.instance_id.clone());
            let declared = match lookup {
                TaskLookup::Declares(declared) => declared,
                TaskLookup::Unread(unread) => {
                    plan.unread.insert(pair, unread);
                    continue;
                }
                TaskLookup::Nothing => continue,
            };
            let stored = entries.get_or_insert_with(|| entries_of(instance.instance_id.as_str()));
            let mut variables = PairVariables {
                values: Vec::with_capacity(declared.variables.len()),
                not_applied: Vec::new(),
            };
            for declared_variable in &declared.variables {
                let variable = &declared_variable.variable;
                let root = table.root_word(&instance.game_id, &variable.name);
                let entry = stored.get(&variable.name);
                let resolved = resolve_variable(variable, entry, root);
                if let (Some(problem), Some(entry)) = (resolved.not_applied(), entry) {
                    variables.not_applied.push(NotAppliedValue {
                        package_id: declared.package_id.clone(),
                        variable_name: variable.name.clone(),
                        declaration_sha256: declared_variable.declaration_sha256.clone(),
                        value_sha256: value_sha256(&entry.value),
                        problem,
                    });
                }
                variables.values.push(resolved);
            }
            plan.pairs.insert(pair, variables);
        }
    }
    plan
}

/// `sha256:<hex>` of `value`'s compact JSON ([`variable_value_json`]).
fn value_sha256(value: &serde_json::Value) -> String {
    text_sha256(&variable_value_json(value))
}

/// `sha256:<hex>` of `text`.
fn text_sha256(text: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(text.as_bytes()))
}

/// Workflow #463 V2 (`contracts/package-variables.md`, "A set value that no longer fits"); fix
/// round 2: public, the one function that judges a value stored on an application against a
/// declaration, which a dispatch calls ([`plan_variable_values`]) and a view calls to say what a
/// dispatch would read, so the two cannot disagree. `entry` is the application's entry row of
/// the variable's name ([`stored_v3_entries`]), `root` the word that keeps the name from applying
/// in the application's game ([`VariableTable::root_word`]). The answer:
///
/// - with no entry: the default, source `default`, no word;
/// - with one, when `root` names a word: the default with that word;
/// - with one whose `set_unix_ms` or `channel` could not be read: the default with
///   `entry_unreadable`;
/// - else the entry's value as [`PackageVariable::judge`] lets it through, source `set` (with
///   `keys_left_out` when keys of an `ordered` value were left out), or the default with the
///   judgement's word.
///
/// A record that holds a value names who set it, applied or not.
pub fn resolve_variable(
    variable: &PackageVariable,
    entry: Option<&StoredEntry>,
    root: Option<VariableProblem>,
) -> ResolvedVariable {
    let set_by = entry.and_then(|entry| entry.set_by);
    let default = |problem| ResolvedVariable {
        variable_name: variable.name.clone(),
        value: variable.default_value(),
        source: VariableSource::Default,
        set_by,
        problem,
    };
    let Some(entry) = entry else {
        return default(None);
    };
    let judged = match (root, entry.set_by) {
        (Some(problem), _) => Err(problem),
        // Fix round 1: a stored value whose row says nothing readable of who set it is not
        // applied, and says so.
        (None, None) => Err(VariableProblem::EntryUnreadable),
        (None, Some(_)) => variable.judge(&entry.value),
    };
    match judged {
        Ok(value) => {
            // Fix round 1: an `ordered` value applied in part is marked; it was applied.
            let problem = variable
                .leaves_keys_out(&entry.value, &value)
                .then_some(VariableProblem::KeysLeftOut);
            ResolvedVariable {
                variable_name: variable.name.clone(),
                value,
                source: VariableSource::Set,
                set_by,
                problem,
            }
        }
        Err(problem) => default(Some(problem)),
    }
}

/// The `variable_values` record of one value: its name, its source, `value_json` when its text
/// fits a reason (fix round 1: every value a run reads of a set value does; a default longer than
/// that is left out, and the run reads its package's default), who set the application's value,
/// and the word of a value not applied or applied in part.
fn value_record(resolved: &ResolvedVariable) -> Record {
    let text = variable_value_json(&resolved.value);
    RecordedValue {
        name: resolved.variable_name.clone(),
        source: resolved.source,
        text: (text.len() <= MAX_VARIABLE_VALUE_JSON_BYTES).then_some(text),
        set_by: resolved.set_by,
        problem: resolved.problem,
        left_out: false,
    }
    .record()
}

/// One `variable_values` record as [`value_record`] writes it and
/// [`recorded_dispatch_parameters`] reads it back.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordedValue {
    name: String,
    source: VariableSource,
    /// `value_json`.
    text: Option<String>,
    set_by: Option<VariableSetBy>,
    problem: Option<VariableProblem>,
    /// Fix round 2: the record carries the text's sha256 and length in its place.
    left_out: bool,
}

impl RecordedValue {
    fn record(&self) -> Record {
        let mut record = Record::new()
            .variable_name(self.name.as_str())
            .variable_source(self.source);
        if let Some(text) = &self.text {
            record = if self.left_out {
                record
                    .value_sha256(text_sha256(text))
                    .value_bytes(text.len())
                    .value_json_left_out(true)
            } else {
                record.value_json(text.as_str())
            };
        }
        if let Some(set_by) = self.set_by {
            record = record
                .set_unix_ms(set_by.set_unix_ms)
                .set_channel(set_by.channel);
        }
        if let Some(problem) = self.problem {
            record = record.variable_problem(problem);
        }
        record
    }

    /// The record `written` as [`Self::record`] wrote it, with its whole text; `None` for any
    /// other record (one already written with its text left out included).
    fn read(written: &serde_json::Value) -> Option<Self> {
        let text = |key: &str| written.get(key).and_then(serde_json::Value::as_str);
        if written.get("value_sha256").is_some() {
            return None;
        }
        let source = text("variable_source")?;
        let set_by = match (written.get("set_unix_ms"), text("set_channel")) {
            (None, None) => None,
            (Some(at), Some(channel)) => Some(VariableSetBy {
                set_unix_ms: at.as_u64()?,
                channel: set_channel(channel)?,
            }),
            _ => return None,
        };
        let problem = match text("variable_problem") {
            None => None,
            Some(word) => Some(
                VariableProblem::ALL
                    .iter()
                    .copied()
                    .find(|problem| problem.as_str() == word)?,
            ),
        };
        Some(Self {
            name: text("variable_name")?.to_owned(),
            source: VariableSource::ALL
                .iter()
                .copied()
                .find(|known| known.as_str() == source)?,
            text: text("value_json").map(str::to_owned),
            set_by,
            problem,
            left_out: false,
        })
    }
}

/// Whether the record form of `outcome` (a ledger record's reason) stays within an outcome
/// record's bound ([`ENVELOPE_BODY_BUDGET`]).
fn record_fits(outcome: &Outcome) -> bool {
    serde_json::to_vec(outcome).is_ok_and(|bytes| bytes.len() <= ENVELOPE_BODY_BUDGET)
}

/// Workflow #463 V2 fix round 2 (`contracts/package-variables.md`, "The record"): the form in
/// which the ledger records the reason `outcome` of a dispatch's chain. A
/// `policy_dispatch_parameters` whose record form passes an outcome record's bound
/// ([`ENVELOPE_BODY_BUDGET`], the 32 KiB of an envelope less its own members' room; sixteen
/// values of 1024 bytes beside the most parameters and schedule values a reason can carry pass
/// it) carries, for each value whose text does not fit, the longest first until it fits, the
/// text's sha256 (`value_sha256`) and length in bytes (`value_bytes`) in place of `value_json`,
/// and says so (`value_json_left_out`). Every other reason, and a reason that fits, is recorded
/// as it is. Only the recorded form changes: the dispatch is never dropped, and its run reads the
/// values from the reason chain it was admitted with, in memory, whole.
pub fn recorded_dispatch_parameters(outcome: &Outcome) -> Outcome {
    if !outcome.is(PolicyCode::PolicyDispatchParameters) || record_fits(outcome) {
        return outcome.clone();
    }
    let Some(mut records) = outcome
        .value("variable_values")
        .and_then(serde_json::Value::as_array)
        .and_then(|written| {
            written
                .iter()
                .map(RecordedValue::read)
                .collect::<Option<Vec<_>>>()
        })
    else {
        return outcome.clone();
    };
    let mut longest = (0..records.len())
        .filter(|index| records[*index].text.is_some())
        .collect::<Vec<_>>();
    longest.sort_by_key(|index| {
        (
            Reverse(records[*index].text.as_ref().map_or(0, String::len)),
            *index,
        )
    });
    let mut recorded = outcome.clone();
    for index in longest {
        records[index].left_out = true;
        recorded = outcome
            .clone()
            .variable_values(records.iter().map(RecordedValue::record));
        if record_fits(&recorded) {
            break;
        }
    }
    recorded
}

impl RemedyPlan {
    /// Workflow #463 V2: this plan with the values of `variables`: the `policy_dispatch_parameters`
    /// of each pair whose package declares variables gains `variable_values`, one record per
    /// variable in declaration order, after what it carries (`run_parameters`); a pair the plan
    /// gives no such reason gets one with these values alone. A pair whose package declares no
    /// variable keeps its reason, or none, as it was.
    #[must_use]
    pub fn with_variable_values(mut self, variables: &VariablePlan) -> Self {
        for (pair, values) in &variables.pairs {
            let outcome = match self.run_parameters.remove(pair) {
                Some(reason) => reason.outcome,
                None => Outcome::new(PolicyCode::PolicyDispatchParameters),
            }
            .variable_values(values.values.iter().map(value_record));
            self.run_parameters.insert(
                pair.clone(),
                DecisionReason {
                    detail: outcome.to_string(),
                    outcome,
                },
            );
        }
        self
    }
}
