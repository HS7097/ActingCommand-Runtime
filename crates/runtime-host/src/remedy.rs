// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #411 B15, slice R4 (`contracts/remedies.md` sections 5 and 7; model-b15 §7, §9, §10,
//! §12, §14): the host's side of the remedies packages declare.
//!
//! - **The table** (§9.1): the bound packages' declarations, read metadata-only through each
//!   catalog task's procedure location, with the cycles over tasks refused edge by edge and the
//!   findings `capability_unbound` and `parameters_mismatch`. A package that cannot be read
//!   contributes nothing and is named; building never fails a start or an activation. Memory only.
//! - **The state** (§7.8): the outcome key each settled run of a task that declares a remedy
//!   ended on (read from its terminal's `scheduling_disposition` whether or not the catalog
//!   references it), the `signal:remedy:` planning signals already recorded and the stops no
//!   lift followed. The runs themselves are the policy host's dispatch projection, rebuilt in
//!   ledger order at start; the keys and signals are rebuilt there too. Memory only.
//! - **The signals** (§7.5 to §7.7): the plan's stops, lifts and expiries as `goal_missed`
//!   planning signals, each checked against the planning-signal projection before it is written.
//!
//! Needs, capabilities, parameters and options are opaque tokens compared for equality only.
//! Nothing here pairs a need with a capability or orders capabilities.

use crate::codes::{HostCode, HostLocation, RemedyHostCode, SchedulingRuleHostCode};
use crate::{ProcedureBinding, ProcedureManifest, RuntimeHostError};
use actingcommand_contract::outcome::{Code, Outcome, RemedyProblem};
use actingcommand_contract::{
    PolicyPlanningSignalEventData, PolicyPlanningSignalKind, RuntimeErrorCode,
};
use actingcommand_execution_kernel::{
    ContainedTaskCode, PreparedContainedTask, RemedyDeclarations,
};
use actingcommand_policy::{
    CompiledCatalog, InstanceSnapshot, PolicyCode, RemedyChoiceRow, RemedySignal, RemedyStop,
    RemedyTable, RemedyTableCapability, RemedyTableEdge, RemedyTableParameter, RemedyTableProvider,
    RemedyTableRemedy, ScopeSelector, TaskSpec,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, Instant};

/// The `signal_id` prefix of the remedy planning signals (`signal:remedy:<kind>:<hex>`).
pub(crate) const SIGNAL_PREFIX: &str = "signal:remedy:";
/// The instance label the table reads a package under: its declarations do not depend on one.
const DECLARATIONS_LABEL: &str = "remedy-table";
/// How long the table may read one package's declarations.
const DECLARATIONS_BUDGET: Duration = Duration::from_secs(60);

/// What the evaluation and the admission check read of the configuration (model §5.2): the
/// switch, the requests per layer and round, the choice rows in effect per instance alias, and
/// the least time between two rebuilds of the table while a round is open or a stop is recorded
/// (`scheduling.suspension.lift_check_min_interval_ms`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemedyContext {
    pub(crate) enabled: bool,
    pub(crate) max_per_round: u8,
    pub(crate) configuration: BTreeMap<String, Vec<RemedyChoiceRow>>,
    pub(crate) table_interval_ms: u64,
}

/// One finding of the table (model §9.1): a remedy a bound package declares that cannot be used
/// as configured. Its values are those of `policy_remedy_declaration_unusable`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemedyFinding {
    pub(crate) problem: RemedyProblem,
    pub(crate) task_id: String,
    pub(crate) outcome_key: String,
    pub(crate) need: String,
    pub(crate) capability: Option<String>,
    pub(crate) instance_id: Option<String>,
    pub(crate) package_id: Option<String>,
}

/// A package location the table could not read: it contributes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnreadablePackage {
    pub(crate) path: String,
    /// The catalog tasks whose procedure locates it.
    pub(crate) tasks: Vec<String>,
    /// The reader's refusal.
    pub(crate) code: ContainedTaskCode,
}

impl UnreadablePackage {
    /// The developer line that names the package (Fail Loud: a package the table could not read
    /// is never silent). The running Runtime writes it when the table in effect gains the
    /// package; check-config writes it for its offline table.
    pub(crate) fn developer_line(&self) -> String {
        format!(
            "actingd remedy_table unreadable code={} tasks={} path={}",
            self.code,
            self.tasks.join(","),
            self.path,
        )
    }
}

/// The table of one catalog generation, as built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BuiltTable {
    pub(crate) table: RemedyTable,
    pub(crate) findings: Vec<RemedyFinding>,
    pub(crate) unreadable: Vec<UnreadablePackage>,
    catalog_hash: String,
    built_at_unix_ms: u64,
}

impl BuiltTable {
    /// Whether a mapping of `task_id`'s package declares a remedy.
    pub(crate) fn declares_remedies(&self, task_id: &str) -> bool {
        self.table
            .remedies
            .iter()
            .any(|remedy| remedy.task_id == task_id)
    }

    /// Whether `task_id`'s mapping of `outcome_key` declares a remedy.
    pub(crate) fn remedy_key(&self, task_id: &str, outcome_key: &str) -> bool {
        self.table
            .remedies
            .iter()
            .any(|remedy| remedy.task_id == task_id && remedy.outcome_key == outcome_key)
    }

    /// The tasks whose runs the journal holds: every task that declares a remedy or provides a
    /// capability.
    pub(crate) fn journal_tasks(&self) -> BTreeSet<&str> {
        self.table
            .remedies
            .iter()
            .map(|remedy| remedy.task_id.as_str())
            .chain(
                self.table
                    .providers
                    .iter()
                    .map(|provider| provider.task_id.as_str()),
            )
            .collect()
    }
}

/// Builds the table of `catalog` (model §9.1): each catalog task's package, located by its
/// procedure binding, is read metadata-only (`read_remedy_declarations`), each location once; a
/// location that cannot be read contributes nothing and is listed. Then the cycles over tasks are
/// refused edge by edge, and the findings are collected over `instances`.
pub(crate) fn build_table(
    catalog: &CompiledCatalog,
    manifest: Option<&ProcedureManifest>,
    instances: &[InstanceSnapshot],
    built_at_unix_ms: u64,
) -> BuiltTable {
    let tasks = &catalog.catalog().tasks.tasks;
    let mut locations = BTreeMap::<&str, Vec<&TaskSpec>>::new();
    for task in tasks {
        if let Some(path) = manifest
            .and_then(|manifest| manifest.binding(&task.procedure_ref))
            .and_then(ProcedureBinding::package_path)
        {
            locations.entry(path).or_default().push(task);
        }
    }
    let mut declared = BTreeMap::<&str, RemedyDeclarations>::new();
    let mut unreadable = Vec::new();
    for (path, bound) in &locations {
        match PreparedContainedTask::read_remedy_declarations(
            DECLARATIONS_LABEL,
            Path::new(path),
            Instant::now() + DECLARATIONS_BUDGET,
        ) {
            Ok(declarations) => {
                for task in bound {
                    declared.insert(task.id.as_str(), declarations.clone());
                }
            }
            Err(error) => unreadable.push(UnreadablePackage {
                path: (*path).to_owned(),
                tasks: bound.iter().map(|task| task.id.clone()).collect(),
                code: error.code(),
            }),
        }
    }
    let mut remedies = Vec::new();
    let mut providers = Vec::new();
    let mut packages = BTreeMap::<&str, &str>::new();
    for task in tasks {
        let Some(declarations) = declared.get(task.id.as_str()) else {
            continue;
        };
        packages.insert(task.id.as_str(), declarations.package_id.as_str());
        for (outcome_key, remedy) in &declarations.remedies {
            remedies.push(RemedyTableRemedy {
                task_id: task.id.clone(),
                outcome_key: outcome_key.clone(),
                need: remedy.need().to_owned(),
                capabilities: remedy.capabilities().to_vec(),
                default: remedy.default_capabilities().to_vec(),
                problem: None,
            });
        }
        if !declarations.provides.is_empty() {
            providers.push(RemedyTableProvider {
                task_id: task.id.clone(),
                provides: declarations
                    .provides
                    .iter()
                    .map(|provided| RemedyTableCapability {
                        capability: provided.capability().to_owned(),
                        irreversible: provided.irreversible(),
                        parameters: provided
                            .parameters()
                            .iter()
                            .map(|parameter| RemedyTableParameter {
                                name: parameter.name().to_owned(),
                                options: parameter.options().to_vec(),
                                default: parameter.default_option().to_owned(),
                            })
                            .collect(),
                    })
                    .collect(),
            });
        }
    }
    let refused_edges = cycle_edges(&remedies, &providers);
    let findings = findings(
        tasks,
        &remedies,
        &providers,
        &refused_edges,
        &packages,
        instances,
    );
    for remedy in &mut remedies {
        if findings.iter().any(|finding| {
            finding.problem == RemedyProblem::CapabilityUnbound
                && finding.task_id == remedy.task_id
                && finding.outcome_key == remedy.outcome_key
        }) {
            remedy.problem = Some(RemedyProblem::CapabilityUnbound);
        }
    }
    BuiltTable {
        table: RemedyTable {
            remedies,
            providers,
            refused_edges,
        },
        findings,
        unreadable,
        catalog_hash: catalog.catalog_hash().to_owned(),
        built_at_unix_ms,
    }
}

/// The edges "blocked task to a provider of its remedies' capabilities" that lie on a cycle over
/// tasks: an edge whose provider reaches its blocked task. Each is refused; the rest stay usable.
fn cycle_edges(
    remedies: &[RemedyTableRemedy],
    providers: &[RemedyTableProvider],
) -> Vec<RemedyTableEdge> {
    let mut edges = BTreeSet::<(&str, &str)>::new();
    for remedy in remedies {
        for capability in &remedy.capabilities {
            for provider in providers {
                if provider.task_id != remedy.task_id
                    && provider
                        .provides
                        .iter()
                        .any(|provided| provided.capability == *capability)
                {
                    edges.insert((remedy.task_id.as_str(), provider.task_id.as_str()));
                }
            }
        }
    }
    edges
        .iter()
        .filter(|(blocked, provider)| reaches(&edges, provider, blocked))
        .map(|(blocked, provider)| RemedyTableEdge {
            blocked_task_id: (*blocked).to_owned(),
            provider_task_id: (*provider).to_owned(),
        })
        .collect()
}

/// Whether `to` is reachable from `from` over `edges`.
fn reaches(edges: &BTreeSet<(&str, &str)>, from: &str, to: &str) -> bool {
    let mut pending = vec![from];
    let mut visited = BTreeSet::new();
    while let Some(task) = pending.pop() {
        if task == to {
            return true;
        }
        if !visited.insert(task) {
            continue;
        }
        pending.extend(
            edges
                .iter()
                .filter(|(blocked, _)| *blocked == task)
                .map(|(_, provider)| *provider),
        );
    }
    false
}

/// The table's findings (model §9.1, `policy_remedy_declaration_unusable`): per remedy, a
/// capability with no provider task on an instance its task covers (`capability_unbound`; with
/// no instance known, none in the catalog), an edge refused as part of a cycle (`cycle`), and
/// providers of one capability on one instance whose parameter declarations differ
/// (`parameters_mismatch`).
fn findings(
    tasks: &[TaskSpec],
    remedies: &[RemedyTableRemedy],
    providers: &[RemedyTableProvider],
    refused_edges: &[RemedyTableEdge],
    packages: &BTreeMap<&str, &str>,
    instances: &[InstanceSnapshot],
) -> Vec<RemedyFinding> {
    let scope_of = |task_id: &str| {
        tasks
            .iter()
            .find(|task| task.id == task_id)
            .map(|task| &task.scope)
    };
    let provided = |provider: &RemedyTableProvider, capability: &str| {
        provider
            .provides
            .iter()
            .find(|provided| provided.capability == capability)
            .map(|provided| {
                provided
                    .parameters
                    .iter()
                    .map(|parameter| {
                        (
                            parameter.name.clone(),
                            parameter.options.iter().cloned().collect::<BTreeSet<_>>(),
                            parameter.default.clone(),
                        )
                    })
                    .collect::<BTreeSet<_>>()
            })
    };
    let mut findings = Vec::new();
    for remedy in remedies {
        let finding = |problem, capability: Option<&str>, instance: Option<&str>| RemedyFinding {
            problem,
            task_id: remedy.task_id.clone(),
            outcome_key: remedy.outcome_key.clone(),
            need: remedy.need.clone(),
            capability: capability.map(str::to_owned),
            instance_id: instance.map(str::to_owned),
            package_id: packages
                .get(remedy.task_id.as_str())
                .map(|package| (*package).to_owned()),
        };
        let covered = instances
            .iter()
            .filter(|instance| {
                scope_of(remedy.task_id.as_str()).is_some_and(|scope| covers(scope, instance))
            })
            .collect::<Vec<_>>();
        for capability in &remedy.capabilities {
            let candidates = providers
                .iter()
                .filter(|provider| provider.task_id != remedy.task_id)
                .filter_map(|provider| {
                    provided(provider, capability.as_str())
                        .map(|declaration| (provider, declaration))
                })
                .collect::<Vec<_>>();
            if instances.is_empty() {
                if candidates.is_empty() {
                    findings.push(finding(
                        RemedyProblem::CapabilityUnbound,
                        Some(capability.as_str()),
                        None,
                    ));
                }
            } else {
                for instance in &covered {
                    let on_instance = candidates
                        .iter()
                        .filter(|(provider, _)| {
                            scope_of(provider.task_id.as_str())
                                .is_some_and(|scope| covers(scope, instance))
                        })
                        .collect::<Vec<_>>();
                    if on_instance.is_empty() {
                        findings.push(finding(
                            RemedyProblem::CapabilityUnbound,
                            Some(capability.as_str()),
                            Some(instance.instance_id.as_str()),
                        ));
                    } else if on_instance
                        .iter()
                        .map(|(_, declaration)| declaration)
                        .collect::<BTreeSet<_>>()
                        .len()
                        > 1
                    {
                        findings.push(finding(
                            RemedyProblem::ParametersMismatch,
                            Some(capability.as_str()),
                            Some(instance.instance_id.as_str()),
                        ));
                    }
                }
            }
            for (provider, _) in &candidates {
                if refused_edges.iter().any(|edge| {
                    edge.blocked_task_id == remedy.task_id
                        && edge.provider_task_id == provider.task_id
                }) {
                    findings.push(finding(
                        RemedyProblem::Cycle,
                        Some(capability.as_str()),
                        None,
                    ));
                    break;
                }
            }
        }
    }
    findings
}

/// Whether a task of `scope` covers `instance`.
fn covers(scope: &ScopeSelector, instance: &InstanceSnapshot) -> bool {
    match scope {
        ScopeSelector::Instance { instance_id } => *instance_id == instance.instance_id,
        ScopeSelector::Server { server_id } => *server_id == instance.server_id,
        ScopeSelector::Game { game_id } => *game_id == instance.game_id,
    }
}

/// Workflow #411 B15 R7: one configuration list of remedy choice rows check-config compares with
/// the table (model-b15 §5.2): the section's (`scheduling.remedy.choices`, no `instance_id`) or
/// an instance's own (`instance.<instance id>.scheduling.remedy.choices`), by its registry id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemedyChoiceList {
    pub instance_id: Option<String>,
    pub rows: Vec<actingcommand_runtime_config::RemedyChoiceRow>,
}

/// Workflow #411 B15 R7: one check-config note `policy_remedy_declaration_unusable`, and for a
/// configuration row the developer detail that names the row and its list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemedyDeclarationNote {
    pub outcome: Outcome,
    pub detail: Option<String>,
}

/// Workflow #411 B15 R7: what `actingd check-config` reports about declared remedies: its notes,
/// and for each package the offline table could not read, the developer line the running Runtime
/// writes for it (no note: the configuration stays valid).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemedyDeclarationNotes {
    pub notes: Vec<RemedyDeclarationNote>,
    pub unreadable: Vec<String>,
}

/// Workflow #411 B15 R7 (model-b15 §9.1, §13; `contracts/remedies.md` section 10): what
/// `actingd check-config` notes about declared remedies. R4's table is built offline over the
/// catalog of `catalog` and the package locations of `config`'s procedure manifest, for the
/// instances of its policy inputs; each finding (`capability_unbound`, `cycle`,
/// `parameters_mismatch`) is one note naming its task, outcome key, need, capability, instance
/// and package. Then each row of `lists` whose names the bound packages do not declare is one
/// note: an `allow` or `none` row whose need no bound remedy declares, or an `allow` row whose
/// capability that need's remedies do not list (`choice_invalid`); a `parameter` row whose
/// capability no bound provider declares, whose parameter that capability does not declare, or
/// whose value is not one of its options (`parameter_invalid`); a per-instance list's notes name
/// its `instance_id`. A package the table cannot read gives no note but its developer line, as at
/// run time. Nothing is written. A catalog that does not compile gives no note: the check's
/// catalog preview refuses it.
pub fn remedy_declaration_notes(
    config: &crate::RuntimeHostConfig,
    catalog: &actingcommand_policy::CatalogSources,
    lists: &[RemedyChoiceList],
) -> RemedyDeclarationNotes {
    let Ok(compiled) = actingcommand_policy::compile_catalog(catalog) else {
        return RemedyDeclarationNotes {
            notes: Vec::new(),
            unreadable: Vec::new(),
        };
    };
    let instances = config
        .policy_inputs()
        .map_or(&[][..], |inputs| inputs.facts().instances.as_slice());
    let built = build_table(&compiled, config.procedure_manifest(), instances, 0);
    let mut notes = built
        .findings
        .iter()
        .map(|finding| {
            let mut outcome = Outcome::new(RemedyHostCode::PolicyRemedyDeclarationUnusable)
                .remedy_problem(finding.problem)
                .task_id(finding.task_id.as_str())
                .outcome_key(finding.outcome_key.as_str())
                .need(finding.need.as_str());
            if let Some(capability) = &finding.capability {
                outcome = outcome.capability(capability.as_str());
            }
            if let Some(instance_id) = &finding.instance_id {
                outcome = outcome.instance_id(instance_id.as_str());
            }
            if let Some(package_id) = &finding.package_id {
                outcome = outcome.package_id(package_id.as_str());
            }
            RemedyDeclarationNote {
                outcome,
                detail: None,
            }
        })
        .collect::<Vec<_>>();
    for list in lists.iter().filter(|list| list.rows.is_empty()) {
        let config_key = list.instance_id.as_deref().map_or_else(
            || actingcommand_runtime_config::scheduling::REMEDY_CHOICES.to_owned(),
            |instance_id| {
                actingcommand_runtime_config::instance_key(
                    instance_id,
                    actingcommand_runtime_config::scheduling::REMEDY_CHOICES,
                )
            },
        );
        for row in &list.rows {
            let Some((outcome, named)) = unusable_row(&built.table, row) else {
                continue;
            };
            let outcome = match &list.instance_id {
                Some(instance_id) => outcome.instance_id(instance_id.as_str()),
                None => outcome,
            };
            notes.push(RemedyDeclarationNote {
                outcome,
                detail: Some(format!("config_key={config_key} {named}")),
            });
        }
    }
    RemedyDeclarationNotes {
        notes,
        unreadable: Vec::new(),
    }
}

/// A configuration row whose names the table does not declare, as its note (without the list's
/// instance) and the row as developer text; `None` for a row the table declares.
fn unusable_row(
    table: &RemedyTable,
    row: &actingcommand_runtime_config::RemedyChoiceRow,
) -> Option<(Outcome, String)> {
    use actingcommand_runtime_config::RemedyChoiceRow as ConfigRow;
    let note = || Outcome::new(RemedyHostCode::PolicyRemedyDeclarationUnusable);
    let declared_need = |need: &str| table.remedies.iter().any(|remedy| remedy.need == need);
    match row {
        ConfigRow::Allow { need, capability } => {
            let named = format!("row=allow need={need} capability={capability}");
            if !declared_need(need) {
                Some((
                    note()
                        .remedy_problem(RemedyProblem::ChoiceInvalid)
                        .need(need.as_str()),
                    named,
                ))
            } else if !table
                .remedies
                .iter()
                .any(|remedy| remedy.need == *need && remedy.capabilities.contains(capability))
            {
                Some((
                    note()
                        .remedy_problem(RemedyProblem::ChoiceInvalid)
                        .need(need.as_str())
                        .capability(capability.as_str()),
                    named,
                ))
            } else {
                None
            }
        }
        ConfigRow::NoRemedy { need } => (!declared_need(need)).then(|| {
            (
                note()
                    .remedy_problem(RemedyProblem::ChoiceInvalid)
                    .need(need.as_str()),
                format!("row=none need={need}"),
            )
        }),
        ConfigRow::Parameter {
            capability,
            parameter,
            value,
        } => {
            let declared = table
                .providers
                .iter()
                .flat_map(|provider| &provider.provides)
                .filter(|provided| provided.capability == *capability)
                .flat_map(|provided| &provided.parameters)
                .any(|declared| declared.name == *parameter && declared.options.contains(value));
            (!declared).then(|| {
                (
                    note()
                        .remedy_problem(RemedyProblem::ParameterInvalid)
                        .capability(capability.as_str()),
                    format!(
                        "row=parameter capability={capability} parameter={parameter} value={value}"
                    ),
                )
            })
        }
    }
}

/// One admitted run of a task that declares a remedy, as the start reads its terminal's key
/// (model §7.8): newest first per pair, until its latest root blocked run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemedyKeyRead {
    pub(crate) decision_id: String,
    pub(crate) task_id: String,
    /// A completed success: its terminal names the key.
    pub(crate) read: bool,
    /// Its chain names a round (a request or a retry): never a root.
    pub(crate) round_run: bool,
}

/// The remedy state of the policy host (model §7.8, §9.1). Memory only.
#[derive(Debug, Default)]
pub(crate) struct RemedyState {
    table: Option<BuiltTable>,
    /// Per decision, the outcome key the run's terminal names (a settled successful run of a
    /// task that declares a remedy).
    keys: BTreeMap<String, String>,
    /// Every `signal:remedy:` signal id recorded.
    seen: BTreeSet<String>,
    /// The recorded stops no lift followed, by their round's `<hex>`.
    stops: BTreeMap<String, RemedyStop>,
    /// Whether the last evaluation's plan held, requested, retried or signalled anything.
    active: bool,
    /// The signals the last evaluation decided, not yet in the projection, for the recorder.
    pending: Vec<PolicyPlanningSignalEventData>,
}

impl RemedyState {
    pub(crate) fn table(&self) -> Option<&BuiltTable> {
        self.table.as_ref()
    }

    /// Takes `table` as the table in effect. Each finding and each unreadable package the table
    /// it replaces did not have is written as a developer line (Fail Loud: a remedy nothing can
    /// serve, or a package the table could not read, is never silent).
    pub(crate) fn set_table(&mut self, table: BuiltTable) {
        let previous = self.table.take();
        for finding in &table.findings {
            if previous
                .as_ref()
                .is_some_and(|previous| previous.findings.contains(finding))
            {
                continue;
            }
            println!(
                "actingd remedy_table finding remedy_problem={} task_id={} outcome_key={} need={} capability={} instance_id={} package_id={}",
                finding.problem.as_str(),
                finding.task_id,
                finding.outcome_key,
                finding.need,
                finding.capability.as_deref().unwrap_or("-"),
                finding.instance_id.as_deref().unwrap_or("-"),
                finding.package_id.as_deref().unwrap_or("-"),
            );
        }
        for package in &table.unreadable {
            if previous
                .as_ref()
                .is_some_and(|previous| previous.unreadable.contains(package))
            {
                continue;
            }
            println!("{}", package.developer_line());
        }
        self.table = Some(table);
    }

    /// Whether the table is to be (re)built for the active catalog `catalog_hash` at `now`: there
    /// is none or it is another catalog's (the start, an activation), or, while a round is open
    /// or a stop is recorded, at most once per `interval_ms` (a package fixed in place is seen
    /// without a restart, as the suspension lift checks see it).
    pub(crate) fn table_due(&self, catalog_hash: &str, now_unix_ms: u64, interval_ms: u64) -> bool {
        match &self.table {
            None => true,
            Some(table) if table.catalog_hash != catalog_hash => true,
            Some(table) => {
                (self.active || !self.stops.is_empty())
                    && now_unix_ms.saturating_sub(table.built_at_unix_ms) >= interval_ms
            }
        }
    }

    /// Whether `task_id` declares a remedy in the table.
    pub(crate) fn declares_remedies(&self, task_id: &str) -> bool {
        self.table
            .as_ref()
            .is_some_and(|table| table.declares_remedies(task_id))
    }

    /// Records the outcome key the terminal of `decision_id`'s run names.
    pub(crate) fn note_key(&mut self, decision_id: &str, outcome_key: String) {
        self.keys.insert(decision_id.to_owned(), outcome_key);
    }

    /// The outcome key the terminal of `decision_id`'s run names, once noted.
    pub(crate) fn outcome_key(&self, decision_id: &str) -> Option<&str> {
        self.keys.get(decision_id).map(String::as_str)
    }

    /// Keeps the keys of the runs a journal still holds.
    pub(crate) fn retain_keys(&mut self, decisions: &BTreeSet<String>) {
        self.keys
            .retain(|decision_id, _| decisions.contains(decision_id));
    }

    pub(crate) fn seen(&self) -> &BTreeSet<String> {
        &self.seen
    }

    pub(crate) fn stops(&self) -> Vec<RemedyStop> {
        self.stops.values().cloned().collect()
    }

    /// One recorded planning signal, live or in ledger order at start: a `signal:remedy:` id is
    /// seen; a stop (one of the three stop codes, with its round's decision and inputs digest)
    /// is recorded for its round; a lift ends its round's stop.
    pub(crate) fn observe_signal(
        &mut self,
        signal_id: &str,
        instance_id: &str,
        task_id: Option<&str>,
        outcome: &Outcome,
    ) {
        let Some(rest) = signal_id.strip_prefix(SIGNAL_PREFIX) else {
            return;
        };
        self.seen.insert(signal_id.to_owned());
        if let Some(hex) = rest.strip_prefix("stopped:") {
            let text = |key: &str| {
                outcome
                    .value(key)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            };
            let stop_code = [
                PolicyCode::PolicyRemedyUnchosen,
                PolicyCode::PolicyRemedyUnavailable,
                PolicyCode::PolicyRemedyExhausted,
            ]
            .into_iter()
            .any(|code| outcome.is(code));
            if let (true, Some(task_id), Some(decision_id), Some(digest)) = (
                stop_code,
                task_id.map(str::to_owned).or_else(|| text("task_id")),
                text("decision_id"),
                text("remedy_inputs_digest"),
            ) {
                self.stops.insert(
                    hex.to_owned(),
                    RemedyStop {
                        task_id,
                        instance_id: instance_id.to_owned(),
                        decision_id,
                        remedy_inputs_digest: digest,
                        outcome: outcome.clone(),
                    },
                );
            }
        } else if let Some(hex) = rest.strip_prefix("lifted:") {
            self.stops.remove(hex);
        }
    }

    /// What an evaluation leaves: whether its plan acted, and its signals not yet recorded.
    pub(crate) fn finish_evaluation(
        &mut self,
        active: bool,
        pending: Vec<PolicyPlanningSignalEventData>,
    ) {
        self.active = active;
        self.pending = pending;
    }

    /// The signals of the last evaluation, once.
    pub(crate) fn take_pending(&mut self) -> Vec<PolicyPlanningSignalEventData> {
        std::mem::take(&mut self.pending)
    }
}

/// The host's codes of persistent gates an eligibility deferral can carry (the clock-slot
/// table's `policy_task_paused` and `pack_admission_suspended`; R2 req 6).
pub(crate) fn persistent_gates() -> Vec<Code> {
    vec![
        HostCode::PolicyTaskPaused.code(),
        SchedulingRuleHostCode::PackAdmissionSuspended.code(),
    ]
}

/// A choice row of the configuration as the plan reads it.
pub(crate) fn policy_row(row: &actingcommand_runtime_config::RemedyChoiceRow) -> RemedyChoiceRow {
    match row {
        actingcommand_runtime_config::RemedyChoiceRow::Allow { need, capability } => {
            RemedyChoiceRow::Allow {
                need: need.clone(),
                capability: capability.clone(),
            }
        }
        actingcommand_runtime_config::RemedyChoiceRow::NoRemedy { need } => {
            RemedyChoiceRow::NoRemedy { need: need.clone() }
        }
        actingcommand_runtime_config::RemedyChoiceRow::Parameter {
            capability,
            parameter,
            value,
        } => RemedyChoiceRow::Parameter {
            capability: capability.clone(),
            parameter: parameter.clone(),
            value: value.clone(),
        },
    }
}

/// A plan's stop, lift or expiry as the `goal_missed` planning signal the host appends: its
/// `instance_id` the alias, its `task_id` the round's root task, its outcome the code with its
/// values, observed at the evaluation time.
pub(crate) fn signal_event(
    signal: RemedySignal,
    observed_at_unix_ms: u64,
) -> PolicyPlanningSignalEventData {
    PolicyPlanningSignalEventData {
        signal_id: signal.signal_id,
        instance_id: signal.instance_id,
        task_id: Some(signal.task_id),
        kind: PolicyPlanningSignalKind::GoalMissed,
        outcome: signal.outcome,
        observed_at_unix_ms,
        detection_budget: None,
    }
}

/// Whether a reason of a dispatch's chain names a remedy round: `policy_remedy_requested` or
/// `policy_remedy_retry`.
pub(crate) fn is_round_reason(outcome: &Outcome) -> bool {
    outcome.is(PolicyCode::PolicyRemedyRequested) || outcome.is(PolicyCode::PolicyRemedyRetry)
}

/// Whether two round reasons name the same round and layer: the same code, blocked task, round
/// (`decision_id`) and layer (`remedy_depth`).
pub(crate) fn same_layer(current: &Outcome, named: &Outcome) -> bool {
    current.code() == named.code()
        && ["task_id", "decision_id", "remedy_depth"]
            .into_iter()
            .all(|key| current.value(key).is_some() && current.value(key) == named.value(key))
}

/// `policy_remedy_round_closed` (model §7.9) for the round reason `named` of a dispatch to
/// `instance_alias`: the round's blocked task, the instance and the round.
pub(crate) fn round_closed(named: &Outcome, instance_alias: &str) -> RuntimeHostError {
    let text = |key: &str| {
        named
            .value(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let (task_id, decision_id) = (text("task_id"), text("decision_id"));
    RuntimeHostError::request(
        RemedyHostCode::PolicyRemedyRoundClosed,
        HostLocation::ValidatePolicyDispatch,
        RuntimeErrorCode::InvalidRequest,
    )
    .with_outcome_values(|outcome| {
        outcome
            .task_id(task_id.as_str())
            .instance_id(instance_alias)
            .decision_id(decision_id.as_str())
    })
}
