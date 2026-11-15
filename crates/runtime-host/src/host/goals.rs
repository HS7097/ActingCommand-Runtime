// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463 V3, which is Workflow #455 G-R2b with the variables in it (register §1.383;
//! `contracts/runtime-interface.md`, "Goal request kinds"): one application's goals, read and set
//! by name.
//!
//! - **`GoalView`** reads, for one registered instance, its goal page: under the locks of
//!   `ResourceTargetView` (policy outcome gate, policy, fact write gate) the authoritative
//!   projection, the resource target view, the active catalog, the remedy table (the bound
//!   packages with their variables; read from the packages only while no evaluation built one),
//!   the remedy parameters the last evaluation resolved and the instance's `session.resource_targets`
//!   record with its version; then, without a lock, the values the next dispatch of each bound
//!   package hands its run, through the run's own resolution (`plan_variable_values`), and the page
//!   (`crate::goals::view_entries`). It reads memory only and records nothing.
//! - **`EditGoal`** reads the same page first, then, in one hold of the fact write gate, reads the
//!   record, compares its version with `expected_version` (absent compares nothing; the version is
//!   the sequence of the `fact.published` that holds the record, `0` without one; another is
//!   `fact_observation_version_conflict` with `current_version`), checks the name and the value
//!   (`crate::goals::check_edit`, `goal_value_invalid`), replaces or removes the one row
//!   (`edit_goal_record`), and publishes the whole record as one `fact.published` keeping the
//!   request's origin. The row it writes names the Runtime's clock at the request, the request's
//!   channel and, for a request from the Lab, the initiator its connection declared.

use super::facts::{FactPublication, FactPublicationPurpose};
use super::planning::planning_request_failure;
use super::resource_targets::policy_record;
use super::*;
use crate::codes::{GoalValueCode, HostCode, HostLocation};
use crate::goals::{
    BoundPackage, BoundParameter, EditRequest, GoalPage, GoalRefusal, check_edit, page_variables,
    view_entries,
};
use crate::remedy::{BuiltTable, covers};
use actingcommand_contract::{
    FactObservation, GoalDeclarationRead, GoalDeclarationState, GoalEdit, GoalEdited,
    GoalRemedyParameter, GoalView, RESOURCE_TARGETS_FACT_KEY, ResourceTargetView,
    StandardPackGoals,
};
use actingcommand_policy::{
    TargetSetter, VariablePlan, edit_goal_record, plan_variable_values, pool_capacity,
    stored_goal_record,
};
use serde_json::Value;

const VIEW_OPERATION: HostLocation = HostLocation::GoalView;
const EDIT_OPERATION: HostLocation = HostLocation::EditGoal;

/// The instance's stored `session.resource_targets` record: its rows (empty for content that is no
/// record list), its expiry and its version (`0` without a record).
struct StoredRecord {
    rows: Option<Vec<BTreeMap<String, FactScalar>>>,
    expires_at_unix_ms: Option<u64>,
    version: u64,
}

impl StoredRecord {
    /// `scope`'s record as the fact store holds it now.
    fn of(store: &InstanceFactStore, scope: &FactScope) -> Self {
        match store.active_revision(scope, RESOURCE_TARGETS_FACT_KEY) {
            Some((record, sequence, _)) => Self {
                rows: Some(match &record.content {
                    FactContent::Inline {
                        value: ContractFactValue::RecordList(rows),
                    } => rows.clone(),
                    _ => Vec::new(),
                }),
                expires_at_unix_ms: record.expires_at_unix_ms,
                version: sequence,
            },
            None => Self {
                rows: None,
                expires_at_unix_ms: None,
                version: 0,
            },
        }
    }
}

/// What a request read of the page: the page itself, what the workbench holds of the game's
/// `goals.json` (its state and its entries as written), the projection, the active catalog, the
/// remedy table, the resource target view, the remedy parameters the last evaluation resolved by
/// task, and the record.
struct PageRead {
    page: GoalPage,
    declaration: GoalDeclarationState,
    raw_entries: Vec<Value>,
    facts: EvaluationFacts,
    catalog: Option<LoadedCatalog>,
    table: Option<BuiltTable>,
    target_view: ResourceTargetView,
    resolved_parameters: BTreeMap<String, DecisionReason>,
    record: StoredRecord,
}

impl HostShared {
    /// `GoalView` (Workflow #463 V3): see the module's description. An alias no instance has is
    /// Denied `instance_unknown`.
    pub(super) fn goal_view(&self, instance_alias: &str) -> Result<OperationSuccess, RequestFailure> {
        self.resolve_instance(instance_alias)?;
        self.goal_view_read(instance_alias)
            .map(|view| OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: None,
                result: RuntimeResult::GoalViewed {
                    view: Box::new(view),
                },
                failure_outcome: None,
            })
            .map_err(planning_request_failure)
    }

    fn goal_view_read(&self, instance_alias: &str) -> RuntimeHostResult<GoalView> {
        let sample = self.runtime_clock_sample()?;
        let time = EvaluationTime {
            unix_ms: sample.unix_ms,
            monotonic_ms: sample.monotonic_ms,
        };
        let read = self.read_goal_page(instance_alias, time, VIEW_OPERATION)?;
        let stored = stored_goal_record(
            read.record.rows.as_deref(),
            read.record.expires_at_unix_ms,
            instance_alias,
        );
        // The values the next dispatch of each bound package hands its run: the run's own
        // resolution, over the same projection and the same table.
        let plan = match (&read.catalog, &read.table) {
            (Some(catalog), Some(table)) => {
                plan_variable_values(catalog.compiled(), &read.facts, table.variables())
            }
            _ => VariablePlan::default(),
        };
        let variables = page_variables(&read.page, &|task_id: &str| {
            plan.values(task_id, instance_alias)
        });
        let conditions = read
            .target_view
            .active
            .as_ref()
            .map(|active| active.conditions.clone())
            .unwrap_or_default();
        let listed = view_entries(
            &read.page,
            &stored,
            &variables,
            &read.raw_entries,
            &conditions,
        );
        Ok(GoalView {
            instance_alias: instance_alias.to_owned(),
            game_id: read.page.game_id.clone(),
            version: read.record.version,
            declaration: read.declaration.clone(),
            entries: listed.entries,
            orphans: listed.orphans,
            remedy_parameters: remedy_parameters(&read.page, &read.resolved_parameters),
            other_targets: listed.other_targets,
            targets: Box::new(read.target_view.clone()),
        })
    }

    /// `EditGoal` (Workflow #463 V3): see the module's description. An alias no instance has is
    /// Denied `instance_unknown`; a refusal is Denied and appends nothing.
    pub(super) fn edit_goal(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        expected_version: Option<u64>,
        op: &GoalEdit,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.resolve_instance(instance_alias)?;
        self.edit_goal_record(validated, instance_alias, expected_version, op, connection_id)
            .map_err(planning_request_failure)
    }

    fn edit_goal_record(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        expected_version: Option<u64>,
        op: &GoalEdit,
        connection_id: ConnectionId,
    ) -> RuntimeHostResult<OperationSuccess> {
        let sample = self.runtime_clock_sample()?;
        let time = EvaluationTime {
            unix_ms: sample.unix_ms,
            monotonic_ms: sample.monotonic_ms,
        };
        let read = self.read_goal_page(instance_alias, time, EDIT_OPERATION)?;
        let request = match op {
            GoalEdit::Set { name, value_json } => EditRequest::Set { name, value_json },
            GoalEdit::Clear { name } => EditRequest::Clear { name },
        };
        // Who sets the row: the request's channel, and the initiator a Lab connection declared;
        // never anything the caller writes.
        let initiator = if validated.source() == EventSource::Lab {
            self.connection_channel(connection_id)?
                .and_then(|bound| bound.initiator)
        } else {
            None
        };
        let setter = validated
            .channel()
            .map(|binding| TargetSetter {
                set_unix_ms: sample.unix_ms,
                channel: binding.channel,
                initiator,
            })
            .ok_or_else(|| {
                RuntimeHostError::request(
                    HostCode::ChannelBindingMissing,
                    EDIT_OPERATION,
                    RuntimeErrorCode::InvalidRequest,
                )
            })?;
        let scope = FactScope::Instance {
            instance_id: instance_alias.to_owned(),
        };
        let written: RuntimeHostResult<(FactPublication, Option<String>)> = (|| {
            let _gate = lock(&self.fact_write_gate, EDIT_OPERATION)?;
            self.synchronize_fact_store_under_gate()?;
            let record = StoredRecord::of(&lock(&self.facts, EDIT_OPERATION)?, &scope);
            if let Some(expected) = expected_version
                && expected != record.version
            {
                let current_version = record.version;
                return Err(RuntimeHostError::request(
                    HostCode::FactObservationVersionConflict,
                    EDIT_OPERATION,
                    RuntimeErrorCode::InvalidRequest,
                )
                .with_outcome_values(|outcome| {
                    outcome
                        .fact_key(RESOURCE_TARGETS_FACT_KEY)
                        .current_version(current_version)
                }));
            }
            let stored = stored_goal_record(
                record.rows.as_deref(),
                record.expires_at_unix_ms,
                instance_alias,
            );
            let change = check_edit(&read.page, &stored, request)
                .map_err(|refusal| goal_value_invalid(op.name(), &refusal))?;
            let edited = edit_goal_record(
                record.rows.as_deref(),
                record.expires_at_unix_ms,
                instance_alias,
                &change,
                setter,
            )
            .map_err(|refusal| match GoalRefusal::of_record(refusal) {
                Some(refusal) => goal_value_invalid(op.name(), &refusal),
                None => RuntimeHostError::fatal(
                    HostCode::ResourceTargetsRecordInvalid,
                    EDIT_OPERATION,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            })?;
            let fact = policy_record(&scope, &edited.composed, sample.unix_ms)?;
            let publication = self.publish_facts_under_gate(
                FactObservation {
                    records: vec![fact],
                },
                Some(validated),
                FactPublicationPurpose::ResourceTargets {
                    expected_version: None,
                },
                (validated.source(), validated.actor()),
            )?;
            Ok((publication, edited.replaced_target_id))
        })();
        if let Err(error) = &written
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        let (
            FactPublication {
                event_id,
                sequence,
                replayed,
                ..
            },
            replaced_target_id,
        ) = written?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(TerminalEvent { sequence, event_id }),
            result: RuntimeResult::GoalEdited {
                edited: GoalEdited {
                    version: sequence,
                    event_id,
                    replayed,
                    replaced_target_id,
                },
            },
            failure_outcome: None,
        })
    }

    /// The page of `instance_alias` at `time`, every refusal naming `operation`: the instance's
    /// game from the policy inputs (an instance they do not configure is
    /// `resource_target_view_instance_unconfigured`, as the resource target view refuses it), what
    /// the workbench holds of the game's `goals.json` (read before the policy: the workbench's
    /// check holds its state while it locks the policy), then, under the locks of
    /// `ResourceTargetView`, the projection, the resource target view, the active catalog, the
    /// remedy table, the remedy parameters the last evaluation resolved and the record; then,
    /// without a lock, the remedy table read from the packages when no evaluation built one for
    /// the active catalog, as `PackScheduleView` reads it, and the page.
    fn read_goal_page(
        &self,
        instance_alias: &str,
        time: EvaluationTime,
        operation: HostLocation,
    ) -> RuntimeHostResult<PageRead> {
        let instances = lock(&self.policy_inputs, operation)?
            .as_ref()
            .map(|inputs| inputs.facts().instances.clone())
            .unwrap_or_default();
        let game_id = instances
            .iter()
            .find(|instance| instance.instance_id == instance_alias)
            .map(|instance| instance.game_id.clone())
            .ok_or_else(|| {
                RuntimeHostError::request(
                    HostCode::ResourceTargetViewInstanceUnconfigured,
                    operation,
                    RuntimeErrorCode::InstanceUnknown,
                )
            })?;
        let (declaration, accepted, raw_entries) = goal_declaration(&game_id);
        let manifest = lock(&self.procedure_manifest, operation)?.clone();
        let scope = FactScope::Instance {
            instance_id: instance_alias.to_owned(),
        };
        let (facts, catalog, table, target_view, resolved_parameters, record) = {
            let _outcome_gate = lock(&self.policy_outcome_gate, operation)?;
            let mut policy = lock(self.policy()?, operation)?;
            let outcome_keys = policy.outcome_key_snapshot()?;
            let _fact_gate = lock(&self.fact_write_gate, operation)?;
            let (facts, _) = self.project_authoritative_policy_inputs_under_gate(
                &mut policy,
                operation,
                &outcome_keys,
                None,
            )?;
            let target_view = self.resource_target_view_of(
                &policy,
                &facts,
                instance_alias,
                time,
                operation.into(),
            )?;
            let record = StoredRecord::of(&lock(&self.facts, operation)?, &scope);
            (
                facts,
                policy.active_loaded(),
                policy.built_table(),
                target_view,
                policy.resolved_parameters(instance_alias),
                record,
            )
        };
        let table = match (table, &catalog) {
            (Some(table), Some(catalog))
                if table.catalog_hash() == catalog.compiled().catalog_hash() =>
            {
                Some(table)
            }
            (_, Some(catalog)) => Some(crate::remedy::build_table(
                catalog.compiled(),
                manifest.as_ref(),
                &instances,
                time.unix_ms,
            )),
            (_, None) => None,
        };
        let instance = facts
            .instances
            .iter()
            .find(|instance| instance.instance_id == instance_alias)
            .cloned()
            .ok_or_else(|| {
                RuntimeHostError::request(
                    HostCode::ResourceTargetViewInstanceUnconfigured,
                    operation,
                    RuntimeErrorCode::InstanceUnknown,
                )
            })?;
        let packages = match (&catalog, &table) {
            (Some(catalog), Some(table)) => bound_packages(catalog, table, &instance),
            _ => Vec::new(),
        };
        let capacities = match &catalog {
            Some(catalog) => target_view
                .targetable
                .iter()
                .filter_map(|targetable| {
                    pool_capacity(
                        catalog.compiled(),
                        &facts,
                        &instance,
                        &targetable.resource,
                        time,
                    )
                    .map(|capacity| (targetable.resource.clone(), capacity))
                })
                .collect(),
            None => BTreeMap::new(),
        };
        let not_applying = match &table {
            Some(table) => packages
                .iter()
                .flat_map(|package| &package.variables)
                .filter_map(|variable| {
                    table
                        .variables()
                        .not_applying(&game_id, &variable.name)
                        .map(|problem| (variable.name.clone(), problem))
                })
                .collect(),
            None => BTreeMap::new(),
        };
        let page = GoalPage {
            game_id,
            declaration: accepted,
            packages,
            targetable: target_view.targetable.clone(),
            not_targetable: target_view.not_targetable.clone(),
            capacities,
            not_applying,
        };
        Ok(PageRead {
            page,
            declaration,
            raw_entries,
            facts,
            catalog,
            table,
            target_view,
            resolved_parameters,
            record,
        })
    }
}

/// What the workbench holds of `game`'s `goals.json`: its state, the accepted file, and its entries
/// as written. In this build no reader of the host opens the file (Workflow #463 V2's fix round
/// gives the workbench's reading of it), so the state is `not_read` and no entry is listed; the
/// variables and the resource targets of the page are listed all the same.
fn goal_declaration(_game: &str) -> (GoalDeclarationState, Option<StandardPackGoals>, Vec<Value>) {
    (
        GoalDeclarationState {
            state: GoalDeclarationRead::NotRead,
            sha256: None,
            standard_pack_problem: None,
            json_pointer: None,
        },
        None,
        Vec::new(),
    )
}

/// The packages the active catalog binds on `instance`: each catalog task whose scope covers it and
/// whose package the remedy table read, in catalog order, each package once (its first task), with
/// the variables it declares and the parameters its `provides` declare.
fn bound_packages(
    catalog: &LoadedCatalog,
    table: &BuiltTable,
    instance: &InstanceSnapshot,
) -> Vec<BoundPackage> {
    let mut packages = Vec::<BoundPackage>::new();
    for task in &catalog.compiled().catalog().tasks.tasks {
        if !covers(&task.scope, instance) {
            continue;
        }
        let Some(package_id) = table.task_packages().get(&task.id) else {
            continue;
        };
        if packages
            .iter()
            .any(|package| package.package_id == *package_id)
        {
            continue;
        }
        let variables = table
            .variables()
            .task_variables(&task.id)
            .map(|(_, variables)| variables.to_vec())
            .unwrap_or_default();
        let parameters = table
            .table
            .providers
            .iter()
            .filter(|provider| provider.task_id == task.id)
            .flat_map(|provider| &provider.provides)
            .flat_map(|capability| {
                capability.parameters.iter().map(|parameter| BoundParameter {
                    capability: capability.capability.clone(),
                    irreversible: capability.irreversible,
                    name: parameter.name.clone(),
                    options: parameter.options.clone(),
                    default_option: parameter.default.clone(),
                })
            })
            .collect();
        packages.push(BoundPackage {
            task_id: task.id.clone(),
            package_id: package_id.clone(),
            variables,
            parameters,
        });
    }
    packages
}

/// The remedy parameters of the page's packages, read-only, each with the value and the source the
/// last evaluation resolved for its task on the instance (`resolved`, the task's
/// `policy_dispatch_parameters`), when it resolved one.
fn remedy_parameters(
    page: &GoalPage,
    resolved: &BTreeMap<String, DecisionReason>,
) -> Vec<GoalRemedyParameter> {
    let mut listed = Vec::new();
    for package in &page.packages {
        let records = resolved
            .get(&package.task_id)
            .and_then(|reason| reason.outcome.value("run_parameters"))
            .and_then(Value::as_array);
        for parameter in &package.parameters {
            let record = records.and_then(|records| {
                records.iter().find(|record| {
                    record.get("parameter_name").and_then(Value::as_str)
                        == Some(parameter.name.as_str())
                })
            });
            let text = |member: &str| {
                record
                    .and_then(|record| record.get(member))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            };
            listed.push(GoalRemedyParameter {
                package_id: package.package_id.clone(),
                task_id: package.task_id.clone(),
                capability: parameter.capability.clone(),
                irreversible: parameter.irreversible,
                parameter_name: parameter.name.clone(),
                options: parameter.options.clone(),
                default_option: parameter.default_option.clone(),
                parameter_value: text("parameter_value"),
                remedy_source: text("remedy_source"),
            });
        }
    }
    listed
}

/// The refusal `goal_value_invalid` of an `EditGoal` of `name` for `refusal`: Denied, nothing
/// appended; `goal_name` when the name has an entry name's form, and the values that locate it.
fn goal_value_invalid(name: &str, refusal: &GoalRefusal) -> RuntimeHostError {
    let refusal = refusal.clone();
    let name = actingcommand_contract::goal_name(name).then(|| name.to_owned());
    RuntimeHostError::request(
        GoalValueCode::Invalid,
        EDIT_OPERATION,
        RuntimeErrorCode::InvalidRequest,
    )
    .with_outcome_values(move |outcome| {
        let mut outcome = outcome.goal_problem(refusal.problem);
        if let Some(name) = name {
            outcome = outcome.goal_name(name);
        }
        if let Some(pointer) = refusal.pointer {
            outcome = outcome.json_pointer(pointer);
        }
        if let Some((min, max)) = refusal.range {
            outcome = outcome.range_min(min).range_max(max);
        }
        if let Some(pool_id) = refusal.pool_id {
            outcome = outcome.pool_id(pool_id);
        }
        if let Some(problem) = refusal.variable_problem {
            outcome = outcome.variable_problem(problem);
        }
        outcome
    })
}
