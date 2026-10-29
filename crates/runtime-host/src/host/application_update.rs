// SPDX-License-Identifier: AGPL-3.0-only

//! UPD-1 (design-upd1 §3.3; register §1.215): the host's writes of the instance fact
//! `health.application.update_required` (`crate::application_update`), its only writer:
//!
//! - true, before the terminal of any run (scheduled, direct, a startup package's, a ladder
//!   rung's) that a shared interrupt page with `result: application_update_required` ended
//!   (`standard_pack.rs`, `handle_shared_interrupt`);
//! - false, before the successful terminal of a `store_update` package's own run, scheduled or
//!   direct (`append_contained_task_terminal`);
//! - false, at the start, for each configured instance that has no record yet (beside the
//!   policy instance seeds); a record already there, `true` included, is kept.
//!
//! A write of the value the active record already holds appends nothing. A run's record is
//! observed at max(now on the Runtime clock, the previous record's observation + 1); the seed at
//! [`APPLICATION_UPDATE_SEED_OBSERVED_AT_UNIX_MS`] (fix round 2: it is the default, not an
//! observation, so it never post-dates the time of an evaluation). No record expires or carries
//! `invalidate_on`.

use super::contained_task::ContainedTaskTerminalDraft;
use super::facts::FactPublicationPurpose;
use super::*;
use crate::application_update::{
    APPLICATION_UPDATE_DETECTOR, APPLICATION_UPDATE_FACT_KEY, APPLICATION_UPDATE_RUN_PREFIX,
    APPLICATION_UPDATE_SCHEMA, APPLICATION_UPDATE_SEED,
    APPLICATION_UPDATE_SEED_OBSERVED_AT_UNIX_MS,
};
use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::codes::ContractLocation;

impl HostShared {
    /// Records `required` as the instance `alias`'s `health.application.update_required`, from
    /// `source_snapshot_id`, unless its active record already holds that value. The record is
    /// observed at max(now on the Runtime clock, the previous record's observation + 1). A refusal
    /// of the store is returned as it is (the caller fails its run, Fail Loud); a fatal one too.
    pub(super) fn record_application_update(
        &self,
        alias: &str,
        required: bool,
        source_snapshot_id: String,
    ) -> RuntimeHostResult<()> {
        let scope = FactScope::Instance {
            instance_id: alias.to_owned(),
        };
        let content = FactContent::Inline {
            value: ContractFactValue::Boolean(required),
        };
        let active = self.active_fact_revision(&scope, APPLICATION_UPDATE_FACT_KEY)?;
        if active
            .as_ref()
            .is_some_and(|(record, _, _)| record.content == content)
        {
            return Ok(());
        }
        let now_unix_ms = self.clock.sample()?.unix_ms;
        let observed_at_unix_ms = active.as_ref().map_or(now_unix_ms, |(record, _, _)| {
            now_unix_ms.max(record.observed_at_unix_ms.saturating_add(1))
        });
        self.publish_application_update(alias, required, source_snapshot_id, observed_at_unix_ms)
    }

    /// Publishes one record of the instance `alias`'s `health.application.update_required`:
    /// `required`, from `source_snapshot_id`, observed at `observed_at_unix_ms`, through the key's
    /// only entry (`FactPublicationPurpose::ApplicationUpdate`).
    fn publish_application_update(
        &self,
        alias: &str,
        required: bool,
        source_snapshot_id: String,
        observed_at_unix_ms: u64,
    ) -> RuntimeHostResult<()> {
        let scope = FactScope::Instance {
            instance_id: alias.to_owned(),
        };
        let value = ContractFactValue::Boolean(required);
        let digest = application_update_digest(alias, observed_at_unix_ms, &value)?;
        self.publish_facts(
            actingcommand_contract::FactObservation {
                records: vec![FactRecord {
                    scope,
                    key: APPLICATION_UPDATE_FACT_KEY.to_owned(),
                    content: FactContent::Inline { value },
                    observed_at_unix_ms,
                    expires_at_unix_ms: None,
                    ttl_policy: None,
                    confidence_milli: 1_000,
                    source_detector: APPLICATION_UPDATE_DETECTOR.to_owned(),
                    source_snapshot_id,
                    schema_version: APPLICATION_UPDATE_SCHEMA.to_owned(),
                    resource_bundle_hash: digest,
                    invalidate_on: Vec::new(),
                }],
            },
            None,
            FactPublicationPurpose::ApplicationUpdate,
        )
        .map(|_| ())
    }

    /// The start's seed (design §3.3): `false` for each configured policy instance that holds no
    /// record of the key, so a store update rule's trigger is never Unknown. A record already
    /// there (true or false) is kept; a second start appends nothing. Any refusal fails the start,
    /// as a policy instance seed's does.
    ///
    /// UPD-1 fix round 2: the seed is observed at [`APPLICATION_UPDATE_SEED_OBSERVED_AT_UNIX_MS`],
    /// not at the start. The fact is an ordinary evaluation input, and the policy evaluator refuses
    /// a whole input that holds a fact observed after the evaluation's time; a seed dated at the
    /// start made every evaluation or forward projection asked about an earlier time fail
    /// (`policy_evaluation_rejected`; `policy_evaluation_input_invalid` for a forward projection)
    /// for a default nothing observed.
    pub(super) fn seed_application_update_facts(&self) -> RuntimeHostResult<()> {
        let Some(inputs) =
            lock(&self.policy_inputs, HostLocation::SeedPolicyInstanceFacts)?.clone()
        else {
            return Ok(());
        };
        for instance in inputs.instance_seeds() {
            let scope = FactScope::Instance {
                instance_id: instance.instance_id.clone(),
            };
            if self
                .active_fact_revision(&scope, APPLICATION_UPDATE_FACT_KEY)?
                .is_some()
            {
                continue;
            }
            self.publish_application_update(
                &instance.instance_id,
                false,
                APPLICATION_UPDATE_SEED.to_owned(),
                APPLICATION_UPDATE_SEED_OBSERVED_AT_UNIX_MS,
            )?;
        }
        Ok(())
    }

    /// Before a run's terminal (design §3.3): a `store_update` package's successful run clears
    /// its instance's fact. A store refusal fails the run instead (its terminal carries the
    /// refusal; the fact stays as it was); a fatal failure poisons it. Every other terminal
    /// writes nothing.
    pub(super) fn clear_application_update_before_terminal(
        &self,
        draft: &mut ContainedTaskTerminalDraft,
    ) -> Result<Option<RequestFailure>, RequestFailure> {
        let Some(alias) = draft.store_update_alias.take() else {
            return Ok(None);
        };
        if draft.outcome != TaskOutcome::Success {
            return Ok(None);
        }
        let source = application_update_run_snapshot(draft.run_id.transport());
        match self.record_application_update(&alias, false, source) {
            Ok(()) => Ok(None),
            Err(error) if error.is_fatal() => Err(RequestFailure::poison_without_terminal(error)),
            Err(error) => {
                draft.outcome = TaskOutcome::Failure;
                draft.final_page = None;
                draft.failure = Some(error.outcome());
                draft.failure_severe = false;
                draft.scheduling_outcome = None;
                draft.selected_scheduling_outcome = None;
                Ok(Some(RequestFailure::request(
                    error,
                    RuntimeReceiptState::Failed,
                    None,
                )))
            }
        }
    }
}

/// The `source_snapshot_id` of a run's write: `application_update:run:<run_id>`.
pub(super) fn application_update_run_snapshot(run_id: &RunId) -> String {
    format!(
        "{APPLICATION_UPDATE_RUN_PREFIX}{}",
        crate::failure_identity::identifier_text(run_id)
    )
}

/// The lowercase hex SHA-256 of the instance alias, the observation time and the value: the
/// record's `resource_bundle_hash`.
fn application_update_digest(
    alias: &str,
    observed_at_unix_ms: u64,
    value: &ContractFactValue,
) -> RuntimeHostResult<String> {
    let bytes = serde_json::to_vec(&(alias, observed_at_unix_ms, value)).map_err(|_| {
        RuntimeHostError::fatal(
            HostCode::FactSnapshotEncodeFailed,
            ContractLocation::PublishFacts,
            RuntimeErrorCode::RuntimeHalted,
        )
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
