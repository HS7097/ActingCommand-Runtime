// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #455 RR3 (register §1.538; `contracts/resource-targets.md`, "Needs computed from the
//! records"): the host's half of the needs computed from the instances' stored records. Before
//! each evaluation, and again at the admission of an intent, under the fact write gate the
//! evaluation's inputs are projected under, the host reads each instance's `session.goal_records`
//! record (with its own reader, `crate::goal_records`) and its progress lists from the fact store,
//! what the scheduler workbench keeps of its game root from memory ([`RecordRoots`], read before
//! the gates), and the holdings (#455 MAKE: and whether a pool has a maker) through the evaluator's
//! own projection of the same inputs and its active catalog
//! (`actingcommand_policy::NeedsProjection`), and computes the needs with the one pure function
//! (`crate::goal_needs::instance_needs`). The result is given to the evaluator in memory and to
//! the instance's view identity; nothing is stored. An instance without stored records has none.

use super::*;
use crate::goal_needs::{NeedsSources, RecordRoots, instance_needs};
use crate::goal_records::StoredGoalRecords;
use actingcommand_contract::GOAL_RECORDS_FACT_KEY;
use actingcommand_policy::{
    GivenPoolReadings, HandedRuns, NeedHolding, NeedsProjection, ProgressListUnknown, RecordNeeds,
};

/// What the needs read of one evaluation's inputs: the facts it projected, the readings of the
/// pools read from rows of a list of the same projection, and (#455 RR3c, register §1.543) the
/// handed runs at the facts' ledger position, the list the evaluation itself is given, so a
/// holding a run left unread is unread to both.
pub(super) struct NeedsInputs<'i> {
    pub(super) facts: &'i EvaluationFacts,
    pub(super) readings: &'i GivenPoolReadings,
    pub(super) handed: &'i HandedRuns,
}

/// What the computation reads of one instance: its facts as the store holds them under the gate,
/// and the evaluator's projection of the evaluation's inputs.
struct HostSources<'s, 'c> {
    store: &'s InstanceFactStore,
    scope: FactScope,
    instance_id: &'s str,
    projection: &'s NeedsProjection<'c>,
}

impl NeedsSources for HostSources<'_, '_> {
    fn list(&self, fact_key: &str) -> Option<&FactRecord> {
        self.store.active_record(&self.scope, fact_key)
    }

    fn list_freshness(
        &self,
        fact_key: &str,
        observed_at_unix_ms: u64,
        expires_at_unix_ms: Option<u64>,
    ) -> Result<Option<u64>, ProgressListUnknown> {
        self.projection.list_freshness(
            self.instance_id,
            fact_key,
            observed_at_unix_ms,
            expires_at_unix_ms,
        )
    }

    fn holding(&self, pool_id: &str) -> NeedHolding {
        self.projection.holding(self.instance_id, pool_id)
    }

    fn has_maker(&self, pool_id: &str, from: &[String]) -> bool {
        self.projection.has_maker(self.instance_id, pool_id, from)
    }
}

impl HostShared {
    /// The needs computed from the stored records of each instance of the inputs' `facts` (only
    /// the instance `only` when it names one) that holds any, with the active catalog of `policy`,
    /// the inputs' readings of the same projection and (#455 RR3c) handed runs, and the
    /// workbench's `roots`, at the evaluation's `time`. The caller holds `policy` and the fact
    /// write gate under which `facts` were projected, so the store holds what the projection read.
    /// Without an instance that holds stored records, or without an active catalog, nothing is
    /// computed. A projection the evaluator cannot make is `policy_evaluation_rejected`, as the
    /// evaluation itself would be.
    pub(super) fn record_needs_under_gate(
        &self,
        policy: &PolicyHost,
        inputs: NeedsInputs<'_>,
        roots: &RecordRoots,
        time: EvaluationTime,
        only: Option<&str>,
    ) -> RuntimeHostResult<RecordNeeds> {
        let NeedsInputs {
            facts,
            readings,
            handed,
        } = inputs;
        let mut needs = RecordNeeds::default();
        let store = lock(&self.facts, HostLocation::ProjectPolicyFacts)?;
        let mut holders = Vec::new();
        for instance in &facts.instances {
            if only.is_some_and(|only| only != instance.instance_id) {
                continue;
            }
            let scope = FactScope::Instance {
                instance_id: instance.instance_id.clone(),
            };
            let Some(record) = store.active_record(&scope, GOAL_RECORDS_FACT_KEY) else {
                continue;
            };
            // Content that is no record list is a record the reader does not read.
            let rows: &[BTreeMap<String, FactScalar>] = match &record.content {
                FactContent::Inline {
                    value: ContractFactValue::RecordList(rows),
                } => rows.as_slice(),
                _ => &[],
            };
            let stored = StoredGoalRecords::read(Some(rows), &instance.instance_id);
            if matches!(&stored, StoredGoalRecords::Read(records) if records.is_empty()) {
                continue;
            }
            holders.push((instance, stored, record.observed_at_unix_ms));
        }
        if holders.is_empty() {
            return Ok(needs);
        }
        let Some(catalog) = policy.active_loaded() else {
            return Ok(needs);
        };
        let projection = NeedsProjection::new(catalog.compiled(), facts, readings, time, handed)
            .map_err(|error| {
                RuntimeHostError::request(
                    HostCode::PolicyEvaluationRejected,
                    HostLocation::ProjectPolicyFacts,
                    RuntimeErrorCode::InvalidRequest,
                )
                .with_native_detail(error.to_string())
            })?;
        for (instance, stored, observed_at_unix_ms) in holders {
            let sources = HostSources {
                store: &store,
                scope: FactScope::Instance {
                    instance_id: instance.instance_id.clone(),
                },
                instance_id: &instance.instance_id,
                projection: &projection,
            };
            if let Some(computed) = instance_needs(
                &stored,
                observed_at_unix_ms,
                roots.get(&instance.game_id),
                &sources,
            ) {
                needs.give(instance.instance_id.clone(), computed);
            }
        }
        Ok(needs)
    }
}

#[cfg(test)]
impl RuntimeHost {
    /// Workflow #455 RR3, test only: while `roots` is `Some`, the needs read it instead of what the
    /// scheduler workbench keeps of each game root.
    pub(crate) fn set_goal_record_roots_for_test(
        &self,
        roots: Option<RecordRoots>,
    ) -> RuntimeHostResult<()> {
        *lock(
            &self
                .shared_ref(HostLocation::StandardPackRead)?
                .goal_record_roots_for_test,
            HostLocation::StandardPackRead,
        )? = roots;
        Ok(())
    }

    /// Workflow #455 RR3, test only: publishes `record` as the goal edit publishes an instance's
    /// `session.goal_records` record (the key's one purpose).
    pub(crate) fn publish_goal_records_for_test(
        &self,
        record: FactRecord,
    ) -> RuntimeHostResult<()> {
        self.work_ref(ContractLocation::PublishFact)?
            .publish_facts(
                actingcommand_contract::FactObservation {
                    records: vec![record],
                },
                None,
                super::facts::FactPublicationPurpose::GoalRecords,
            )
            .map(|_| ())
    }

    /// Workflow #455 RR3, test only: the needs an evaluation at `time` is given, computed from
    /// the authoritative projection now, under the gates an evaluation reads it under.
    pub(crate) fn record_needs_for_test(
        &self,
        time: EvaluationTime,
    ) -> RuntimeHostResult<RecordNeeds> {
        let shared = self.shared_ref(HostLocation::ProjectPolicyFacts)?;
        let roots = shared.goal_record_roots()?;
        let _outcome_gate = lock(
            &shared.policy_outcome_gate,
            HostLocation::SnapshotPolicyOutcomeState,
        )?;
        let mut policy = lock(shared.policy()?, HostLocation::ReadPolicyOutcomeKeys)?;
        let outcome_keys = policy.outcome_key_snapshot()?;
        let _fact_gate = lock(&shared.fact_write_gate, HostLocation::ProjectPolicyFacts)?;
        let (facts, _, readings) = shared.project_authoritative_policy_inputs_under_gate(
            &mut policy,
            HostLocation::ProjectPolicyFacts,
            &outcome_keys,
            None,
        )?;
        let handed = policy.handed_runs(facts.ledger_position);
        let inputs = NeedsInputs {
            facts: &facts,
            readings: &readings,
            handed: &handed,
        };
        shared.record_needs_under_gate(&policy, inputs, &roots, time, None)
    }
}
