// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #429 R-8 (register §1.308; `contracts/runtime-interface.md`, "Workbench request
//! kinds", the rows `EditPackSchedule` and `PackScheduleView`; `contracts/instance-fact-store.md`,
//! "Pack schedule facts"): a task package switched on or off for one instance.
//!
//! - **`EditPackSchedule`** replaces one package's rows in one scope's `session.pack.schedule`
//!   list. The request's rows are checked before any lock; then, in one hold of the fact write
//!   gate, the scope's list is read, its version compared with `expected_version` (absent compares
//!   nothing, `0` means the scope has no record, any other value must be the list's version, the
//!   sequence of the `fact.published` that holds it; another is `fact_observation_version_conflict`
//!   with `current_version`), the package's rows replaced (`rows: []` withdraws them), the whole
//!   list checked and published as one `fact.published` that keeps the request's origin. Nothing
//!   else changes: a run in progress is not stopped.
//! - **`PackScheduleView`** answers, for one registered instance, each scope's list version (`0`
//!   without a record) and, per task package that a task of the active catalog covering the
//!   instance runs, in package id order, the decision now with its layer, its row, its next change
//!   and the layer below's decision. It reads memory only and records nothing.

use super::facts::{FactPublication, FactPublicationPurpose};
use super::planning::planning_request_failure;
use super::*;
use crate::codes::{HostCode, HostLocation};
use crate::pack_schedule::{self, Layers, PACK_SCHEDULE_FACT_KEY, Row};
use actingcommand_contract::{
    FactObservation, PackScheduleEditRow, PackScheduleEdited, PackSchedulePackage,
    PackScheduleScopeVersion, PackScheduleView,
};
use actingcommand_policy::ScopeSelector;

const EDIT_OPERATION: HostLocation = HostLocation::EditPackSchedule;
const VIEW_OPERATION: HostLocation = HostLocation::PackScheduleView;

impl HostShared {
    /// `EditPackSchedule` (Workflow #429 R-8): see the module's description. A refusal is Denied
    /// and appends nothing.
    pub(super) fn edit_pack_schedule(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        scope: &FactScope,
        package_id: &str,
        rows: &[PackScheduleEditRow],
        expected_version: Option<u64>,
    ) -> Result<OperationSuccess, RequestFailure> {
        let FactPublication {
            event_id, sequence, ..
        } = self
            .edit_pack_schedule_list(validated, scope, package_id, rows, expected_version)
            .map_err(planning_request_failure)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(TerminalEvent { sequence, event_id }),
            result: RuntimeResult::PackScheduleEdited {
                edited: PackScheduleEdited {
                    version: sequence,
                    event_id,
                },
            },
            failure_outcome: None,
        })
    }

    /// The edit's read, compare, replace, check and write, in one hold of the fact write gate.
    fn edit_pack_schedule_list(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        scope: &FactScope,
        package_id: &str,
        rows: &[PackScheduleEditRow],
        expected_version: Option<u64>,
    ) -> RuntimeHostResult<FactPublication> {
        let edited = pack_schedule::edit_rows(package_id, rows)
            .map_err(|refusal| refusal.into_error(EDIT_OPERATION))?;
        let result: RuntimeHostResult<FactPublication> = (|| {
            let _gate = lock(&self.fact_write_gate, EDIT_OPERATION)?;
            self.synchronize_fact_store_under_gate()?;
            let now = self.clock.sample()?.unix_ms;
            let current = lock(&self.facts, EDIT_OPERATION)?
                .active_revision(scope, PACK_SCHEDULE_FACT_KEY)
                .map(|(record, sequence, _)| (record.clone(), sequence));
            let current_version = current.as_ref().map_or(0, |(_, sequence)| *sequence);
            if expected_version.is_some_and(|expected| expected != current_version) {
                return Err(RuntimeHostError::request(
                    HostCode::FactObservationVersionConflict,
                    EDIT_OPERATION,
                    RuntimeErrorCode::InvalidRequest,
                )
                .with_outcome_values(|outcome| {
                    outcome
                        .fact_key(PACK_SCHEDULE_FACT_KEY)
                        .current_version(current_version)
                }));
            }
            // The other packages' rows of the list in effect; an expired list holds none.
            let kept = match &current {
                Some((record, _)) if !record.is_expired(now) => pack_schedule::stored_rows(record)
                    .map_err(|refusal| refusal.into_error(EDIT_OPERATION))?,
                _ => Vec::new(),
            };
            let list = pack_schedule::replace_rows(kept, package_id, edited)
                .map_err(|refusal| refusal.into_error(EDIT_OPERATION))?;
            let record = pack_schedule::edit_record(scope.clone(), &list, now, EDIT_OPERATION)?;
            self.publish_facts_under_gate(
                FactObservation {
                    records: vec![record],
                },
                Some(validated),
                FactPublicationPurpose::Ordinary,
                (validated.source(), validated.actor()),
            )
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    /// `PackScheduleView` (Workflow #429 R-8): see the module's description. An alias no
    /// instance has is Denied `instance_unknown`, as the instance reads refuse it.
    pub(super) fn pack_schedule_view(
        &self,
        instance_alias: &str,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.pack_schedule_view_read(instance_alias)
            .map(|view| OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: None,
                result: RuntimeResult::PackScheduleViewed {
                    view: Box::new(view),
                },
                failure_outcome: None,
            })
            .map_err(planning_request_failure)
    }

    /// The view's reads, each lock released before the next is taken: the registry, the policy
    /// inputs, the manifest, the policy (the active catalog and the package of each of its tasks,
    /// read again from the packages only while no evaluation built the table), then the fact
    /// store under its gate.
    fn pack_schedule_view_read(&self, instance_alias: &str) -> RuntimeHostResult<PackScheduleView> {
        let known = lock(&self.registered_instances, VIEW_OPERATION)?
            .values()
            .any(|registered| registered.instance_alias == instance_alias);
        if !known {
            return Err(RuntimeHostError::request(
                RuntimeErrorCode::InstanceUnknown,
                HostLocation::ResolveRuntimeInstance,
                RuntimeErrorCode::InstanceUnknown,
            ));
        }
        let instances = lock(&self.policy_inputs, VIEW_OPERATION)?
            .as_ref()
            .map(|inputs| inputs.facts().instances.clone())
            .unwrap_or_default();
        let instance = instances
            .iter()
            .find(|instance| instance.instance_id == instance_alias)
            .cloned();
        let manifest = lock(&self.procedure_manifest, VIEW_OPERATION)?.clone();
        let (catalog, cached) = {
            let policy = lock(self.policy()?, VIEW_OPERATION)?;
            (policy.active_loaded(), policy.task_packages())
        };
        let now = self.clock.sample()?.unix_ms;
        let task_packages = match (&catalog, cached) {
            (_, Some(cached)) => cached,
            (Some(catalog), None) => {
                crate::remedy::build_table(catalog.compiled(), manifest.as_ref(), &instances, now)
                    .task_packages()
                    .clone()
            }
            (None, None) => BTreeMap::new(),
        };
        // Every package a task of the active catalog that covers the instance runs, once, in
        // package id order; a package only a row names is not one of them.
        let packages = catalog
            .as_ref()
            .map(|catalog| {
                catalog
                    .compiled()
                    .catalog()
                    .tasks
                    .tasks
                    .iter()
                    .filter(|task| covers(&task.scope, instance_alias, instance.as_ref()))
                    .filter_map(|task| task_packages.get(&task.id).cloned())
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let mut scopes = vec![FactScope::Instance {
            instance_id: instance_alias.to_owned(),
        }];
        if let Some(instance) = &instance {
            scopes.push(FactScope::Server {
                server_id: instance.server_id.clone(),
            });
            scopes.push(FactScope::Game {
                game_id: instance.game_id.clone(),
            });
        }
        let lists = {
            let _gate = lock(&self.fact_write_gate, VIEW_OPERATION)?;
            self.synchronize_fact_store_under_gate()?;
            let facts = lock(&self.facts, VIEW_OPERATION)?;
            scopes
                .iter()
                .map(|scope| {
                    let revision = facts.active_revision(scope, PACK_SCHEDULE_FACT_KEY);
                    let version = revision.as_ref().map_or(0, |(_, sequence, _)| *sequence);
                    // An expired list holds no row, and one that cannot be read (#419) decides
                    // nothing, as the evaluation reads them.
                    let rows = revision
                        .filter(|(record, _, _)| !record.is_expired(now))
                        .and_then(|(record, _, _)| pack_schedule::stored_rows(record).ok());
                    (scope.clone(), version, rows)
                })
                .collect::<Vec<_>>()
        };
        let mut layers: [Option<Vec<Row>>; 3] = [None, None, None];
        let mut versions = Vec::with_capacity(lists.len());
        for (index, (scope, version, rows)) in lists.into_iter().enumerate() {
            layers[index] = rows;
            versions.push(PackScheduleScopeVersion { scope, version });
        }
        let layers = Layers::new(layers);
        Ok(PackScheduleView {
            instance_alias: instance_alias.to_owned(),
            scopes: versions,
            packages: packages
                .into_iter()
                .map(|package_id| {
                    let decision = pack_schedule::resolve(&layers, &package_id, now);
                    PackSchedulePackage {
                        package_id,
                        schedulable: decision.schedulable,
                        source: decision.source,
                        row: decision.row.as_ref().map(Row::edit_row),
                        next_change_unix_ms: decision.next_change_unix_ms,
                        below: decision.below,
                    }
                })
                .collect(),
        })
    }
}

/// Whether a task of `scope` covers the instance `alias` (with the server and game the policy
/// inputs give it, when they name it).
fn covers(scope: &ScopeSelector, alias: &str, instance: Option<&InstanceSnapshot>) -> bool {
    match scope {
        ScopeSelector::Instance { instance_id } => instance_id == alias,
        ScopeSelector::Server { server_id } => {
            instance.is_some_and(|instance| instance.server_id == *server_id)
        }
        ScopeSelector::Game { game_id } => {
            instance.is_some_and(|instance| instance.game_id == *game_id)
        }
    }
}
