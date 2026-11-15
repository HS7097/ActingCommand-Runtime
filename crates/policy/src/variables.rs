// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463 V2 (register §1.382; `contracts/package-variables.md`, "The run"): the values a
//! selected dispatch hands its run for the variables its package declares.
//!
//! - **The table** ([`VariableTable`]): built by the host once per remedy table (the active
//!   catalog's bound packages, read metadata-only), with each game's root check run once: the
//!   contract crate's [`check_root_variables`] over the packages the table read for the tasks of
//!   that game's instances, with [`IrreversiblePackages`] of what they provide and what their
//!   remedies list (a package the table could not read counts as unreadable). Memory only;
//!   dropped with the table it belongs to.
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

use crate::codes::PolicyCode;
use crate::evaluator::scope_matches_instance;
use crate::resource_targets::{StoredEntry, stored_v3_entries};
use crate::{CompiledCatalog, DecisionReason, EvaluationFacts, RemedyPlan};

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

/// The variables of one task's package.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskVariables {
    package_id: String,
    variables: Vec<PackageVariable>,
}

/// The variables of the active catalog's bound packages, and per game the names the root check
/// keeps from applying there. Built once per remedy table ([`VariableTable::build`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VariableTable {
    tasks: BTreeMap<String, TaskVariables>,
    games: BTreeMap<String, BTreeMap<String, VariableProblem>>,
}

impl VariableTable {
    /// The table of `tasks` (in catalog order): per task whose package declares variables, its
    /// package and variables; per game any task covers, the root check of the packages read for
    /// that game's tasks (each package once, in catalog order), against no `goals.json` (the
    /// Runtime reads none) and with [`IrreversiblePackages`] of every such package, a task whose
    /// package could not be read counting as an unreadable package of the game. The answer is
    /// read by [`variables_not_applying`]: one word per name that does not apply.
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
            let checked = check_root_variables(
                &root,
                &BTreeSet::new(),
                &IrreversiblePackages::new(declared),
            );
            judged.insert(game.to_owned(), variables_not_applying(&root, &checked));
        }
        let tasks = tasks
            .iter()
            .filter_map(|task| {
                let package = task.package.as_ref()?;
                (!package.variables.is_empty()).then(|| {
                    (
                        task.task_id.clone(),
                        TaskVariables {
                            package_id: package.package_id.clone(),
                            variables: package.variables.clone(),
                        },
                    )
                })
            })
            .collect();
        Self {
            tasks,
            games: judged,
        }
    }

    /// Whether no bound package declares a variable.
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Workflow #463 V3: the package the catalog task `task_id` runs and the variables it
    /// declares, in declaration order, as the table read them; `None` when it declares none or the
    /// table could not read it.
    pub fn task_variables(&self, task_id: &str) -> Option<(&str, &[PackageVariable])> {
        self.tasks
            .get(task_id)
            .map(|task| (task.package_id.as_str(), task.variables.as_slice()))
    }

    /// The word the root check keeps the variable `name` from applying with in `game`
    /// (`root_unchecked` when no root check judged that game); `None` when it applies. Workflow
    /// #463 V3 moved this reading of the table out of [`plan_variable_values`], which calls it, so
    /// that the goal page reads the one judgement a dispatch reads.
    pub fn not_applying(&self, game: &str, name: &str) -> Option<VariableProblem> {
        match self.games.get(game) {
            Some(names) => names.get(name).copied(),
            None => Some(VariableProblem::RootUnchecked),
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
    /// Who set the application's value, when it holds one, applied or not.
    pub set_by: Option<VariableSetBy>,
    /// Why the application's value was not applied; `None` when it was or there is none.
    pub problem: Option<VariableProblem>,
}

/// A value set on an application that a dispatch of the pair does not hand to its run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotAppliedValue {
    pub package_id: String,
    pub variable_name: String,
    /// The variable's declaration ([`PackageVariable::declaration_sha256`]).
    pub declaration_sha256: String,
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
/// instances whose task's bound package declares variables in `table` and whose scope covers the
/// instance. Each variable, in declaration order, reads:
///
/// - with no entry row of its name in the instance's v3 record: the default, source `default`;
/// - with one, when the root check keeps the name from applying in the instance's game (or no
///   check judged that game, `root_unchecked`): the default, with the word;
/// - else the entry's value as [`PackageVariable::judge`] lets it through, source `set`, or the
///   default with the judgement's word; a value whose text is longer than a reason carries
///   ([`MAX_VARIABLE_VALUE_JSON_BYTES`]) is `value_too_long`.
///
/// A record that holds a value names who set it, applied or not; a value not applied is also one
/// [`NotAppliedValue`] of the pair.
pub fn plan_variable_values(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    table: &VariableTable,
) -> VariablePlan {
    let mut plan = VariablePlan::default();
    if table.is_empty() {
        return plan;
    }
    for instance in &facts.instances {
        let entries = stored_v3_entries(facts, &instance.instance_id);
        for task in &catalog.catalog().tasks.tasks {
            let Some(declared) = table.tasks.get(&task.id) else {
                continue;
            };
            if !scope_matches_instance(&task.scope, instance) {
                continue;
            }
            let mut pair = PairVariables {
                values: Vec::with_capacity(declared.variables.len()),
                not_applied: Vec::new(),
            };
            for variable in &declared.variables {
                let root = table.not_applying(&instance.game_id, &variable.name);
                let resolved = resolve(variable, entries.get(&variable.name), root);
                if let Some(problem) = resolved.problem {
                    pair.not_applied.push(NotAppliedValue {
                        package_id: declared.package_id.clone(),
                        variable_name: variable.name.clone(),
                        declaration_sha256: variable.declaration_sha256(),
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

/// One variable's value: `entry` its entry row, `root` the word the root check keeps its name
/// from applying with.
fn resolve(
    variable: &PackageVariable,
    entry: Option<&StoredEntry>,
    root: Option<VariableProblem>,
) -> ResolvedVariable {
    let set_by = entry.map(|entry| VariableSetBy {
        set_unix_ms: entry.set_unix_ms,
        channel: entry.channel,
    });
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
    let judged = match root {
        Some(problem) => Err(problem),
        None => variable.judge(&entry.value),
    }
    .and_then(|value| {
        if variable_value_json(&value).len() > MAX_VARIABLE_VALUE_JSON_BYTES {
            Err(VariableProblem::ValueTooLong)
        } else {
            Ok(value)
        }
    });
    match judged {
        Ok(value) => ResolvedVariable {
            variable_name: variable.name.clone(),
            value,
            source: VariableSource::Set,
            set_by,
            problem: None,
        },
        Err(problem) => default(Some(problem)),
    }
}

/// The `variable_values` record of one value: its name, its source, `value_json` when its text
/// fits a reason (a default longer than that is left out, and the run reads its package's
/// default), who set the application's value, and the word of a value not applied.
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
