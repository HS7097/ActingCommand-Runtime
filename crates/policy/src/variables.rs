// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463 V2 (register §1.382; `contracts/package-variables.md`, "The run"): the values a
//! selected dispatch hands its run for the variables its package declares.
//!
//! - **The table** ([`VariableTable`]), memory only. Fix round 1 (the coordinator's ruling: the
//!   judgement of "does not apply" is the root check's, over the whole root): with a composed
//!   standard pack the host's scheduler workbench builds it ([`VariableTable::of_roots`]) from the
//!   judgement of each game root it read ([`RootVariableJudgement`]): the contract crate's
//!   [`check_root_variables`] over every package of the registry, held ones included, against the
//!   names the root's accepted `goals.json` takes, with [`IrreversiblePackages`] of what every
//!   package provides and what its remedies list; it is built again whenever the workbench reads a
//!   root again. Without a composed standard pack the host's remedy table builds it once per table
//!   ([`VariableTable::build`]), as before: the catalog's bound packages, with each game's root
//!   check run once over the packages the table read for the tasks of that game's instances (a
//!   package the table could not read counts as unreadable). Each variable's declaration sha256 is
//!   computed when the table is built.
//! - **The resolution** ([`plan_variable_values`]): per (task, instance) pair whose package
//!   declares variables, per variable: the application's entry row of the variable's name in its
//!   `session.resource_targets` v3 record, as the evaluation's facts hold it, judged at use
//!   ([`PackageVariable::judge`]); else the package default. A variable the root check keeps
//!   from applying is handed no value from outside. There is no third source: nothing here
//!   computes or rewrites a value.
//! - **The record** ([`RemedyPlan::with_variable_values`]): the pair's
//!   `policy_dispatch_parameters` gains `variable_values`, one record per variable of the
//!   package, which the evaluator attaches to a selected dispatch only. A pair whose package
//!   declares no variable gets nothing, so its reason chain is byte for byte as before.
//!
//! Resolution does not read `scheduling.remedy.enabled`: the host adds the values to the plan
//! whether remedies are on or off. Pure and deterministic: no clock, no file, no ledger.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::{Outcome, Record};
use actingcommand_contract::{
    DeclaredCapabilities, IrreversiblePackages, MAX_VARIABLE_VALUE_JSON_BYTES, PackageVariable,
    RootPackageVariables, RuntimeChannel, VariableProblem, VariableSource, check_root_variables,
    variable_value_json, variables_not_applying,
};
use sha2::{Digest, Sha256};

use crate::codes::PolicyCode;
use crate::evaluator::scope_matches_instance;
use crate::resource_targets::{StoredEntry, stored_v3_entries};
use crate::{
    CompiledCatalog, DecisionReason, EvaluationFacts, RemedyPlan, STANDARD_PACK_PROCEDURE_PREFIX,
    TaskSpec,
};

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
    /// What it provides and what its remedies list, for the rule of irreversibility.
    pub capabilities: DeclaredCapabilities,
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
/// and the names that do not apply there, each with its word: [`variables_not_applying`] of the
/// contract crate's [`check_root_variables`] over those packages, and the workbench's own words
/// for what it could not read (`goals_unreadable`). Memory only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RootVariableJudgement {
    /// Each package that declares variables, by its registry id.
    packages: BTreeMap<String, TaskVariables>,
    not_applying: BTreeMap<String, VariableProblem>,
}

impl RootVariableJudgement {
    /// The judgement of a root whose readable packages are `packages` (each by its registry id)
    /// and whose names that do not apply are `not_applying`.
    pub fn new(
        packages: &[RootPackageVariables],
        not_applying: BTreeMap<String, VariableProblem>,
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
            not_applying,
        }
    }

    /// The names of the root that do not apply, each with the word a record says it with.
    pub fn not_applying(&self) -> &BTreeMap<String, VariableProblem> {
        &self.not_applying
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
}

impl VariableTable {
    /// The mode without a composed standard pack (the host's remedy table): the table of `tasks`
    /// (in catalog order): per task whose package declares variables, its package and variables;
    /// per game any task covers, the root check of the packages read for that game's tasks (each
    /// package once, in catalog order), against no `goals.json` and with [`IrreversiblePackages`]
    /// of every such package, a task whose package could not be read counting as an unreadable
    /// package of the game. The answer is read by [`variables_not_applying`]: one word per name
    /// that does not apply (fix round 1: `package_unreadable` where only an unreadable package
    /// makes the root count a package irreversible).
    pub fn build(tasks: &[VariableTableTask]) -> Self {
        let games = tasks
            .iter()
            .flat_map(|task| task.games.iter().map(String::as_str))
            .collect::<BTreeSet<_>>();
        let mut judged = BTreeMap::new();
        for game in games {
            let mut root = Vec::<RootPackageVariables>::new();
            let mut declared = BTreeMap::<String, Option<DeclaredCapabilities>>::new();
            for task in tasks.iter().filter(|task| task.games.contains(game)) {
                match &task.package {
                    Some(package) => {
                        if declared.contains_key(&package.package_id) {
                            continue;
                        }
                        declared.insert(
                            package.package_id.clone(),
                            Some(package.capabilities.clone()),
                        );
                        root.push(RootPackageVariables {
                            package_id: package.package_id.clone(),
                            variables: package.variables.clone(),
                            cap_fact_keys: package.cap_fact_keys.clone(),
                        });
                    }
                    // A package the table could not read leaves every capability of the game
                    // unknown: the safe side of the one rule.
                    None => {
                        declared.insert(format!("unread:{}", task.task_id), None);
                    }
                }
            }
            let irreversible = IrreversiblePackages::new(declared);
            let checked = check_root_variables(&root, &BTreeSet::new(), &irreversible);
            judged.insert(
                game.to_owned(),
                variables_not_applying(&root, &checked, &irreversible),
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
            procedures: BTreeMap::new(),
            games: judged,
        }
    }

    /// Workflow #463 V2 fix round 1: the table with a composed standard pack, of the judgement of
    /// each game root the host's scheduler workbench read, by game: a composed task of the game
    /// runs the package its procedure names (`package.<package_id>`), and reads its variables as
    /// the root's judgement holds them.
    pub fn of_roots(roots: &BTreeMap<String, RootVariableJudgement>) -> Self {
        Self {
            tasks: BTreeMap::new(),
            procedures: roots
                .iter()
                .map(|(game, root)| {
                    (
                        game.clone(),
                        root.packages
                            .iter()
                            .map(|(package_id, package)| {
                                (
                                    format!("{STANDARD_PACK_PROCEDURE_PREFIX}{package_id}"),
                                    package.clone(),
                                )
                            })
                            .collect(),
                    )
                })
                .collect(),
            games: roots
                .iter()
                .map(|(game, root)| (game.clone(), root.not_applying.clone()))
                .collect(),
        }
    }

    /// Whether no package of the table declares a variable.
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty() && self.procedures.values().all(BTreeMap::is_empty)
    }

    /// The variables of the package `task` runs on an instance of `game`.
    fn task_variables(&self, task: &TaskSpec, game: &str) -> Option<&TaskVariables> {
        self.tasks.get(&task.id).or_else(|| {
            self.procedures
                .get(game)
                .and_then(|procedures| procedures.get(&task.procedure_ref))
        })
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
}

impl VariablePlan {
    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
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
}

/// Workflow #463 V2: the variable values of every (task, instance) pair of `catalog` on `facts`'
/// instances whose task's package declares variables in `table` and whose scope covers the
/// instance. Each variable, in declaration order, reads:
///
/// - with no entry row of its name in the instance's v3 record: the default, source `default`;
/// - with one, when the root check keeps the name from applying in the instance's game (or no
///   check judged that game, `root_unchecked`): the default, with the word;
/// - fix round 1: with one whose `set_unix_ms` or `channel` could not be read, the default with
///   `entry_unreadable`;
/// - else the entry's value as [`PackageVariable::judge`] lets it through, source `set` (fix round
///   1: with `keys_left_out` when keys of an `ordered` value were left out), or the default with
///   the judgement's word.
///
/// A record that holds a value names who set it, applied or not; a value not applied is also one
/// [`NotAppliedValue`] of the pair.
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
/// v3 record's, [`stored_v3_entries`]; the gate tests give rows the v3 reader never passes).
pub(crate) fn plan_with_entries(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    table: &VariableTable,
    entries_of: impl Fn(&str) -> BTreeMap<String, StoredEntry>,
) -> VariablePlan {
    let mut plan = VariablePlan::default();
    if table.is_empty() {
        return plan;
    }
    for instance in &facts.instances {
        let entries = entries_of(instance.instance_id.as_str());
        let judged = table.games.get(&instance.game_id);
        for task in &catalog.catalog().tasks.tasks {
            let Some(declared) = table.task_variables(task, &instance.game_id) else {
                continue;
            };
            if !scope_matches_instance(&task.scope, instance) {
                continue;
            }
            let mut pair = PairVariables {
                values: Vec::with_capacity(declared.variables.len()),
                not_applied: Vec::new(),
            };
            for declared_variable in &declared.variables {
                let variable = &declared_variable.variable;
                let root = match judged {
                    Some(names) => names.get(&variable.name).copied(),
                    None => Some(VariableProblem::RootUnchecked),
                };
                let entry = entries.get(&variable.name);
                let resolved = resolve(variable, entry, root);
                if let (Some(problem), Some(entry)) = (resolved.not_applied(), entry) {
                    pair.not_applied.push(NotAppliedValue {
                        package_id: declared.package_id.clone(),
                        variable_name: variable.name.clone(),
                        declaration_sha256: declared_variable.declaration_sha256.clone(),
                        value_sha256: value_sha256(&entry.value),
                        problem,
                    });
                }
                pair.values.push(resolved);
            }
            plan.pairs
                .insert((task.id.clone(), instance.instance_id.clone()), pair);
        }
    }
    plan
}

/// `sha256:<hex>` of `value`'s compact JSON ([`variable_value_json`]).
fn value_sha256(value: &serde_json::Value) -> String {
    format!(
        "sha256:{:x}",
        Sha256::digest(variable_value_json(value).as_bytes())
    )
}

/// One variable's value: `entry` its entry row, `root` the word the root check keeps its name
/// from applying with.
fn resolve(
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
    let mut record = Record::new()
        .variable_name(resolved.variable_name.as_str())
        .variable_source(resolved.source);
    let text = variable_value_json(&resolved.value);
    if text.len() <= MAX_VARIABLE_VALUE_JSON_BYTES {
        record = record.value_json(text);
    }
    if let Some(set_by) = resolved.set_by {
        record = record
            .set_unix_ms(set_by.set_unix_ms)
            .set_channel(set_by.channel);
    }
    if let Some(problem) = resolved.problem {
        record = record.variable_problem(problem);
    }
    record
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
