// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463 V3, which is Workflow #455 G-R2b with the variables in it (register §1.383;
//! `contracts/runtime-interface.md`, "Goal request kinds"): one application's goals, read and set
//! by name.
//!
//! - **`GoalView`** reads, for one registered instance, its goal page from memory only: what the
//!   scheduler workbench keeps of the game root from one check to the next
//!   (`standard_pack_goal_roots`: the root's parsed `goals.json` or why it is not applied, the
//!   judgement of its variables over the whole root, and the variable table a dispatch reads);
//!   then, under the locks of `ResourceTargetView` (policy outcome gate, policy, fact write gate),
//!   the authoritative projection, the resource target view, the active catalog, the remedy table
//!   the last evaluation built (the bound packages with what they provide; fix round 1: only when
//!   it is the table of the active catalog and of the procedure manifest in place, never read
//!   from the packages), the remedy parameters the last evaluation resolved and the instance's
//!   `session.resource_targets` record with its version; then, without a lock, the values the
//!   next dispatch of each bound package hands its run, through the run's own resolution
//!   (`plan_variable_values`), and the page (`crate::goals::view_entries`). Until the workbench has
//!   checked the game root and an evaluation has built that table, the page answers
//!   `not_checked_yet` and lists nothing. Fix round 2: one word per cause of a page that lists
//!   nothing, which is also the word every `EditGoal` is refused with: `not_checked_yet` (not in
//!   memory yet, which passes by itself), `root_failing` (the workbench checks the game root and
//!   no check since the start could read it; the state names why) or `no_schedule` (the game has
//!   no standard pack root with `schedule.json`: nothing to check). It records nothing.
//! - **`EditGoal`** reads the same page first, then, in one hold of the fact write gate, reads the
//!   record, compares its version with `expected_version` (absent compares nothing; the version is
//!   the sequence of the `fact.published` that holds the record, `0` without one; another is
//!   `fact_observation_version_conflict` with `current_version`), then checks the name and the
//!   value against the record it is about to change (`crate::goals::check_edit`,
//!   `goal_value_invalid`), replaces or removes the one row (`edit_goal_record`), and publishes the
//!   whole record as one `fact.published` keeping the request's origin. So a request of another
//!   version is a version conflict whatever its value. The row it writes names the Runtime's clock
//!   at the request, the request's channel and, for a request from the Lab, the initiator its
//!   connection declared.

use super::facts::{FactPublication, FactPublicationPurpose};
use super::planning::planning_request_failure;
use super::resource_targets::policy_record;
use super::*;
use crate::codes::{GoalValueCode, HostCode, HostLocation};
use crate::goals::{
    BoundPackage, BoundParameter, EditRequest, GoalPage, GoalRefusal, ViewEntries, check_edit,
    page_variables, view_entries,
};
use crate::remedy::{BuiltTable, covers};
use actingcommand_contract::{
    FactObservation, GoalDeclarationRead, GoalDeclarationState, GoalEdit, GoalEdited, GoalProblem,
    GoalRemedyParameter, GoalView, RESOURCE_TARGETS_FACT_KEY, ResourceTargetView,
    StandardPackGoals,
};
use actingcommand_policy::{
    TargetSetter, VariableSetBy, VariableTable, edit_goal_record, plan_variable_values,
    pool_capacity, stored_goal_record,
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

/// What the page is judged by, once it is in memory: the active catalog and the variable table a
/// dispatch reads.
struct PageSources {
    catalog: LoadedCatalog,
    variables: Arc<VariableTable>,
}

/// What a request read of the page: the page itself, what the workbench holds of the game's
/// `goals.json`, the projection, what the page is judged by (`None` until it is in memory), the
/// resource target view, the remedy parameters the last evaluation resolved by task, and the
/// record.
struct PageRead {
    page: GoalPage,
    declaration: GoalDeclarationState,
    facts: EvaluationFacts,
    sources: Option<PageSources>,
    target_view: ResourceTargetView,
    resolved_parameters: BTreeMap<String, DecisionReason>,
    record: StoredRecord,
}

impl HostShared {
    /// `GoalView` (Workflow #463 V3): see the module's description. An alias no instance has is
    /// Denied `instance_unknown`.
    pub(super) fn goal_view(
        &self,
        instance_alias: &str,
    ) -> Result<OperationSuccess, RequestFailure> {
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
        // Fix round 1: a page not in memory yet lists nothing.
        let listed = match &read.sources {
            Some(sources) => {
                let stored = stored_goal_record(
                    read.record.rows.as_deref(),
                    read.record.expires_at_unix_ms,
                    instance_alias,
                );
                // The values the next dispatch of each bound package hands its run: the run's own
                // resolution, over the same projection and the table a dispatch reads.
                let plan = plan_variable_values(
                    sources.catalog.compiled(),
                    &read.facts,
                    &sources.variables,
                );
                let variables = page_variables(&read.page, &|task_id: &str| {
                    plan.values(task_id, instance_alias)
                });
                let conditions = read
                    .target_view
                    .active
                    .as_ref()
                    .map(|active| active.conditions.clone())
                    .unwrap_or_default();
                view_entries(&read.page, &stored, &variables, &conditions)
            }
            None => ViewEntries::default(),
        };
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
        self.edit_goal_record(
            validated,
            instance_alias,
            expected_version,
            op,
            connection_id,
        )
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
        // The setter a value is judged with, as the dispatch's own judgement reads a stored row.
        let set_by = VariableSetBy {
            set_unix_ms: setter.set_unix_ms,
            channel: setter.channel,
        };
        let scope = FactScope::Instance {
            instance_id: instance_alias.to_owned(),
        };
        let written: RuntimeHostResult<(FactPublication, Option<String>)> = (|| {
            let _gate = lock(&self.fact_write_gate, EDIT_OPERATION)?;
            self.synchronize_fact_store_under_gate()?;
            // Fix round 2: the store's lock only for the read; the publication below takes it.
            let record = {
                let store = lock(&self.facts, EDIT_OPERATION)?;
                StoredRecord::of(&store, &scope)
            };
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
            // The checks read the record they are about to change, so they run here, under the
            // gate, after the version comparison.
            let stored = stored_goal_record(
                record.rows.as_deref(),
                record.expires_at_unix_ms,
                instance_alias,
            );
            let change = check_edit(&read.page, &stored, request, set_by)
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

    /// The page of `instance_alias` at `time`, every refusal naming `operation`, read from memory
    /// only: the instance's game from the policy inputs (an instance they do not configure is
    /// `resource_target_view_instance_unconfigured`, as the resource target view refuses it); with
    /// a composed standard pack, what the scheduler workbench keeps of each game root from one
    /// check to the next (read before the policy: the workbench's check holds its state while it
    /// locks the policy); the digest of the procedure manifest in place; then, under the locks of
    /// `ResourceTargetView`, the projection, the resource target view, the active catalog, the
    /// remedy table the last evaluation built, the remedy parameters it resolved and the record.
    ///
    /// Fix round 1: the page is judged by the variable table a dispatch reads (the workbench's
    /// with a composed standard pack, else the remedy table's), the remedy table (only when it is
    /// the active catalog's and the manifest's, as the next evaluation finds it) and, with a
    /// composed standard pack, the workbench's last check of the game root (its `goals.json` as
    /// read and the judgement of its variables). Until all of them are in memory the page is not
    /// checked yet ([`GoalPage::not_checked`], `not_checked_yet`); no package is read for it.
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
        let goal_roots = self.standard_pack_goal_roots()?;
        let manifest_digest = lock(&self.procedure_manifest, operation)?
            .as_ref()
            .map(|manifest| manifest.digest());
        let scope = FactScope::Instance {
            instance_id: instance_alias.to_owned(),
        };
        let (facts, catalog, table, target_view, resolved_parameters, record) = {
            let _outcome_gate = lock(&self.policy_outcome_gate, operation)?;
            let mut policy = lock(self.policy()?, operation)?;
            let outcome_keys = policy.outcome_key_snapshot()?;
            let _fact_gate = lock(&self.fact_write_gate, operation)?;
            let (facts, _, readings) = self.project_authoritative_policy_inputs_under_gate(
                &mut policy,
                operation,
                &outcome_keys,
                None,
            )?;
            // #455 P2 (register §1.505, ruling 7): a resource entry's current reads a pool read
            // from rows of a list from its reading, through the resource target view.
            let target_view = self.resource_target_view_of(
                &policy,
                &facts,
                &readings,
                instance_alias,
                time,
                operation.into(),
            )?;
            // Fix round 2: the store's lock only for the read.
            let record = {
                let store = lock(&self.facts, operation)?;
                StoredRecord::of(&store, &scope)
            };
            (
                facts,
                policy.active_loaded(),
                policy.built_table(),
                target_view,
                policy.resolved_parameters(instance_alias),
                record,
            )
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
        // The remedy table the last evaluation built, when it is the one the next evaluation keeps:
        // built for the active catalog from the procedure manifest in place.
        let table = table.filter(|table| {
            catalog.as_ref().is_some_and(|catalog| {
                table.catalog_hash() == catalog.compiled().catalog_hash()
                    && table.manifest_digest() == manifest_digest.as_deref()
            })
        });
        let root = goal_roots
            .as_ref()
            .and_then(|roots| roots.roots.get(&game_id));
        // The variable table a dispatch reads: the workbench's with a composed standard pack, else
        // the remedy table's.
        let sources = match (catalog, &table, &goal_roots) {
            (Some(catalog), Some(_), Some(roots)) if root.is_some() => Some(PageSources {
                catalog,
                variables: Arc::clone(&roots.table),
            }),
            (Some(catalog), Some(table), None) => Some(PageSources {
                catalog,
                variables: Arc::new(table.variables().clone()),
            }),
            _ => None,
        };
        // Fix round 2: one word per cause of a page that lists nothing. A root the workbench's
        // last check keeps nothing of answers why (asking again would not change it); else what
        // the page is judged by is not in memory yet.
        let unread = goal_roots
            .as_ref()
            .and_then(|roots| roots.unread.get(&game_id));
        let (Some(sources), Some(table)) = (sources, table) else {
            let (word, declaration) = unchecked_state(unread);
            return Ok(PageRead {
                page: GoalPage::not_checked(game_id, word),
                declaration,
                facts,
                sources: None,
                target_view,
                resolved_parameters,
                record,
            });
        };
        let (declaration, accepted) = goal_declaration(root.map(|root| &root.goals));
        let packages = bound_packages(&sources.catalog, &table, &sources.variables, &instance);
        let capacities = target_view
            .targetable
            .iter()
            .filter_map(|targetable| {
                pool_capacity(
                    sources.catalog.compiled(),
                    &facts,
                    &instance,
                    &targetable.resource,
                    time,
                )
                .map(|capacity| (targetable.resource.clone(), capacity))
            })
            .collect();
        // The one judgement a dispatch reads of whether each variable applies in the game root:
        // its word, read from the dispatch's table (`root_word`, which a dispatch calls).
        let not_applying = packages
            .iter()
            .flat_map(|package| &package.variables)
            .filter_map(|variable| {
                sources
                    .variables
                    .root_word(&game_id, &variable.name)
                    .map(|problem| (variable.name.clone(), problem))
            })
            .collect();
        // Fix round 1: the root's judgement of each of its names (every package of the root that
        // declares it, its declaration, and why it does not apply with the detail of the cause).
        let verdicts = root
            .map(|root| {
                root.variables
                    .verdicts()
                    .iter()
                    .map(|verdict| (verdict.name.clone(), verdict.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let page = GoalPage {
            game_id,
            unchecked: None,
            declaration: accepted,
            packages,
            targetable: target_view.targetable.clone(),
            capacities,
            not_applying,
            root: verdicts,
        };
        Ok(PageRead {
            page,
            declaration,
            facts,
            sources: Some(sources),
            target_view,
            resolved_parameters,
            record,
        })
    }
}

/// A declaration state of `state` with nothing else.
fn declaration_state(state: GoalDeclarationRead) -> GoalDeclarationState {
    GoalDeclarationState {
        state,
        sha256: None,
        standard_pack_problem: None,
        json_pointer: None,
        path: None,
        io_kind: None,
    }
}

/// Fix round 2 (#1731, item 3): the word every edit of a page that lists nothing is refused
/// with, and the page's declaration state, by what the workbench's last check keeps of the game
/// root (`unread`): `root_failing` for a root that failed at that check and that no check since
/// the start read, with why as the check recorded it (the file, and the reader's first problem
/// with its pointer or the operating system's failure kind); `no_schedule` for a game without a
/// standard pack root with `schedule.json`; else `not_checked_yet`.
fn unchecked_state(unread: Option<&RootUnread>) -> (GoalProblem, GoalDeclarationState) {
    match unread {
        Some(RootUnread::Failing(failure)) => {
            let text = |outcome: &actingcommand_contract::outcome::Outcome, key: &str| {
                outcome
                    .value(key)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            };
            let os_failure = failure.causes().first().map(|link| link.outcome());
            (
                GoalProblem::RootFailing,
                GoalDeclarationState {
                    state: GoalDeclarationRead::RootFailing,
                    sha256: None,
                    standard_pack_problem: text(failure, "standard_pack_problem"),
                    json_pointer: text(failure, "json_pointer"),
                    path: text(failure, "path"),
                    io_kind: os_failure.and_then(|cause| text(cause, "io_kind")),
                },
            )
        }
        Some(RootUnread::NoSchedule) => (
            GoalProblem::NoSchedule,
            declaration_state(GoalDeclarationRead::NoSchedule),
        ),
        None => (
            GoalProblem::NotCheckedYet,
            declaration_state(GoalDeclarationRead::NotCheckedYet),
        ),
    }
}

/// Fix round 1: what the workbench's last check of the game root holds of its `goals.json`
/// (`goals`; `None` without a composed standard pack, where no workbench reads a game root,
/// `not_read`): its state and the accepted file. A refused file names its sha256 and its first
/// problem with its pointer; it, an unreadable file and an absent one list no entry.
fn goal_declaration(
    goals: Option<&RootGoals>,
) -> (GoalDeclarationState, Option<StandardPackGoals>) {
    match goals {
        None => (declaration_state(GoalDeclarationRead::NotRead), None),
        Some(RootGoals::Absent) => (declaration_state(GoalDeclarationRead::Absent), None),
        Some(RootGoals::Accepted(accepted)) => (
            declaration_state(GoalDeclarationRead::Accepted),
            Some(accepted.clone()),
        ),
        Some(RootGoals::Refused { sha256, issues }) => {
            let first = issues.first();
            (
                GoalDeclarationState {
                    state: GoalDeclarationRead::Refused,
                    sha256: Some(sha256.clone()),
                    standard_pack_problem: first.map(|issue| issue.problem.as_str().to_owned()),
                    json_pointer: first.map(|issue| issue.pointer.clone()),
                    path: None,
                    io_kind: None,
                },
                None,
            )
        }
        Some(RootGoals::Unreadable(_)) => {
            (declaration_state(GoalDeclarationRead::Unreadable), None)
        }
    }
}

/// The packages the active catalog binds on `instance`: each catalog task whose scope covers it
/// and whose package the variable table a dispatch reads (`variables`) or the remedy table
/// (`table`) knows, in catalog order, each package once (its first task), with the variables the
/// variable table holds for it (the dispatch's own reading, `declared_variables`) and the
/// parameters its `provides` declare.
fn bound_packages(
    catalog: &LoadedCatalog,
    table: &BuiltTable,
    variables: &VariableTable,
    instance: &InstanceSnapshot,
) -> Vec<BoundPackage> {
    let mut packages = Vec::<BoundPackage>::new();
    for task in &catalog.compiled().catalog().tasks.tasks {
        if !covers(&task.scope, instance) {
            continue;
        }
        let declared = variables.declared_variables(task, &instance.game_id);
        let package_id = match &declared {
            Some((package_id, _)) => Some((*package_id).to_owned()),
            None => table.task_packages().get(&task.id).cloned(),
        };
        let Some(package_id) = package_id else {
            continue;
        };
        if packages
            .iter()
            .any(|package| package.package_id == package_id)
        {
            continue;
        }
        let parameters = table
            .table
            .providers
            .iter()
            .filter(|provider| provider.task_id == task.id)
            .flat_map(|provider| &provider.provides)
            .flat_map(|capability| {
                capability
                    .parameters
                    .iter()
                    .map(|parameter| BoundParameter {
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
            package_id,
            variables: declared
                .map(|(_, declared)| declared.into_iter().cloned().collect())
                .unwrap_or_default(),
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
