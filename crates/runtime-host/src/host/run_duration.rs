// SPDX-License-Identifier: AGPL-3.0-only

//! SCH-M slice ED-2 (frozen model v1.0 §3.4; Alice 10-22: 「每个任务执行所需的时间可以动态的从账本读，
//! 近5次该任务包的平均时间加5分钟」; ruling 39 R6.2 「即便版本升级，也不清空历史最近五次的平均耗时」,
//! R6.3 「放在账本的实例的基础信息库那一组标签下，启动时随模拟器连接完成载入内存。之后只更新内存，账本
//! 由探针更新」): the host side of the expected duration E.
//!
//! - **Write.** At the settlement of a successful scheduled policy run, the duration of the run
//!   becomes the newest sample of its package's row in its instance's `health.run_duration`
//!   record, and the instance's whole record is published in one `fact.published` (origin and
//!   actor `runtime`, `source_detector` `runtime.run-duration`, no expiry, no `invalidate_on`).
//!   A failed run, a run a condition ended, a direct run, a Lab operation and a dispatch no
//!   scheduled path prepared (its package unknown) add none.
//! - **Load.** The record lives only in the ledger: it is an ordinary tagged record of the
//!   instance store, which the start loads from the tag index (Workflow #422 LN-2) before the
//!   first evaluation.
//! - **Projection.** Every authoritative projection of the evaluation inputs fills
//!   `TaskRuntimeSnapshot.expected_runtime_ms` and `.expected_runtime_runs` of each pair whose
//!   package has samples on an instance whose `scheduling.expected_runtime.enabled` is on. The
//!   record is consumed here only: it is never an ordinary fact of an evaluation.
//! - With the switch off for an instance, nothing is written for it and nothing is projected, so
//!   its evaluations are those without samples, byte for byte.

use super::facts::FactPublicationPurpose;
use super::*;
use crate::codes::{RunDurationCode, RunDurationLocation};
use crate::run_duration::{
    RUN_DURATION_DETECTOR, RUN_DURATION_FACT_KEY, RUN_DURATION_SCHEMA,
    RUN_DURATION_SNAPSHOT_PREFIX, RunDurations,
};
use actingcommand_policy::{
    FACT_REFRESH_TASK_ID, ScopeSelector, TaskRuntimeSnapshot, expected_runtime, fact_refresh_task,
};

impl HostShared {
    /// The sample a settled policy run gives (SCH-M D-13), recorded once, at the live
    /// settlement that appended its execution record: a success of a run whose scheduled path
    /// admitted its main package. Anything else adds none.
    pub(super) fn record_run_duration_sample(
        &self,
        data: &PolicyExecutionEventData,
    ) -> RuntimeHostResult<()> {
        let PolicyExecutionOutcome::Succeeded { runtime_ms } = &data.outcome else {
            return Ok(());
        };
        let Some(package_id) = self.scheduled_main_package(&data.decision_id)? else {
            return Ok(());
        };
        self.add_run_duration_sample(
            &data.instance_id,
            &package_id,
            *runtime_ms,
            data.observed_at_unix_ms,
        )
    }

    /// Adds one sample of `package_id` on the instance `alias` (`duration_ms`, the run's terminal
    /// at `terminal_unix_ms`) and publishes the instance's whole record (SCH-M D-14). The
    /// observation time is max(terminal time, the previous record's observation + 1), so a
    /// record is always newer than the one it replaces. A refusal of the store (a full store,
    /// for one) is recorded as `run_duration_row_dropped` with the refusal as its cause and the
    /// settlement goes on; a fatal failure is returned. Each row the write drops is recorded as
    /// `run_duration_row_dropped`.
    pub(super) fn add_run_duration_sample(
        &self,
        alias: &str,
        package_id: &str,
        duration_ms: u64,
        terminal_unix_ms: u64,
    ) -> RuntimeHostResult<()> {
        let settings = self.expected_runtime_settings(alias)?;
        if !settings.enabled {
            return Ok(());
        }
        let scope = FactScope::Instance {
            instance_id: alias.to_owned(),
        };
        let active = self.active_fact_revision(&scope, RUN_DURATION_FACT_KEY)?;
        let previous_observed = active
            .as_ref()
            .map(|(record, _, _)| record.observed_at_unix_ms);
        let mut durations = match active.as_ref().map(|(record, _, _)| &record.content) {
            None => RunDurations::default(),
            Some(FactContent::Inline { value }) => RunDurations::read(Some(value)),
            // Only the Runtime writes the record, inline; anything else is one unreadable row.
            Some(FactContent::Artifact { .. }) => RunDurations::unreadable_record(),
        };
        let dropped = durations.add(package_id, duration_ms, terminal_unix_ms, settings.runs);
        let observed_at_unix_ms = previous_observed.map_or(terminal_unix_ms, |previous| {
            terminal_unix_ms.max(previous.saturating_add(1))
        });
        let value = durations.value();
        let digest = run_duration_digest(alias, observed_at_unix_ms, &value)?;
        let record = FactRecord {
            scope,
            key: RUN_DURATION_FACT_KEY.to_owned(),
            content: FactContent::Inline { value },
            observed_at_unix_ms,
            expires_at_unix_ms: None,
            ttl_policy: None,
            confidence_milli: 1_000,
            source_detector: RUN_DURATION_DETECTOR.to_owned(),
            source_snapshot_id: format!("{RUN_DURATION_SNAPSHOT_PREFIX}{digest}"),
            schema_version: RUN_DURATION_SCHEMA.to_owned(),
            resource_bundle_hash: digest,
            invalidate_on: Vec::new(),
        };
        match self.publish_facts(
            actingcommand_contract::FactObservation {
                records: vec![record],
            },
            None,
            FactPublicationPurpose::RunDuration,
        ) {
            Ok(_) => {}
            Err(error) if error.is_fatal() => return Err(error),
            Err(error) => {
                return self.record_run_duration_dropped(
                    Outcome::new(RunDurationCode::RowDropped)
                        .instance_id(alias)
                        .package_id(package_id)
                        .caused_by(error.record_outcome()),
                );
            }
        }
        for package in dropped {
            let outcome = Outcome::new(RunDurationCode::RowDropped).instance_id(alias);
            self.record_run_duration_dropped(match &package {
                Some(package) => outcome.package_id(package.as_str()),
                None => outcome,
            })?;
        }
        Ok(())
    }

    /// One Warning `runtime.failed` with system links carrying `outcome`
    /// (`run_duration_row_dropped`), handed to the daemon's log too.
    fn record_run_duration_dropped(&self, outcome: Outcome) -> RuntimeHostResult<()> {
        self.append_event_raw(
            EventSeverity::Warning,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            self.events.system_links()?,
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::NotPerformed,
                outcome.clone(),
                DiagnosticDetailDraft::new(
                    "expected_runtime",
                    RuntimeLifecycleFailureStage::PolicyDriver.location(),
                    "runtime_host",
                    RunDurationLocation::RecordSample.as_str(),
                ),
                AuditInput::new(),
            ),
        )?;
        self.fatal.report_warning(&outcome);
        Ok(())
    }

    /// SCH-M D-15 "Identity": fills each evaluated pair's expected duration E into `facts.tasks`
    /// from its instance's `health.run_duration` record in `fact_projection` (the store at the
    /// projected position). A pair gets E when its instance's switch is on and its task's
    /// package has samples there; its snapshot is created when the pair has none yet. Tasks
    /// that share a package share its samples. A pair whose package no scheduled run prepared
    /// since the start (its id at its binding's location unknown), or whose row cannot be read,
    /// keeps its declared value. The caller holds `policy` and then `fact_write_gate`.
    pub(super) fn project_expected_runtimes(
        &self,
        policy: &PolicyHost,
        fact_projection: &InstanceFactStore,
        facts: &mut EvaluationFacts,
    ) -> RuntimeHostResult<()> {
        let Some(catalog) = policy.active_loaded() else {
            return Ok(());
        };
        // The instances whose switch is on and whose record holds samples.
        let mut sampled = Vec::new();
        for instance in &facts.instances {
            let settings = self.expected_runtime_settings(&instance.instance_id)?;
            if !settings.enabled {
                continue;
            }
            let Some(FactContent::Inline { value }) = fact_projection
                .active_record(
                    &FactScope::Instance {
                        instance_id: instance.instance_id.clone(),
                    },
                    RUN_DURATION_FACT_KEY,
                )
                .map(|record| &record.content)
            else {
                continue;
            };
            sampled.push((instance, settings, RunDurations::read(Some(value))));
        }
        if sampled.is_empty() {
            return Ok(());
        }
        let procedure_packages = self.procedure_packages()?;
        let refresh_declared_ms = self.fact_refresh_settings()?.expected_duration_ms;
        let mut expected = Vec::new();
        for (instance, settings, durations) in sampled {
            for task in &catalog.compiled().catalog().tasks.tasks {
                if !scope_covers(&task.scope, instance) {
                    continue;
                }
                let Some(package_id) = procedure_packages.get(&task.procedure_ref) else {
                    continue;
                };
                let samples = durations.samples(package_id);
                if samples.is_empty() {
                    continue;
                }
                // SS-3: a task that writes no `max_runtime_ms` puts no ceiling on E.
                let runtime = expected_runtime(
                    samples,
                    settings,
                    task.expected_duration_ms,
                    task.max_runtime_ms().unwrap_or(u64::MAX),
                )?;
                expected.push((task.id.clone(), instance.instance_id.clone(), runtime));
            }
            // The fact refresh reads with the instance's bound recognition maintenance package.
            if let Some((package_id, _)) = self
                .fact_refresh_bindings
                .get(&instance.instance_id)
                .and_then(|binding| binding.marks.as_ref())
            {
                let samples = durations.samples(package_id);
                if !samples.is_empty() {
                    let runtime = expected_runtime(
                        samples,
                        settings,
                        refresh_declared_ms,
                        fact_refresh_task().max_runtime_ms().unwrap_or(u64::MAX),
                    )?;
                    expected.push((
                        FACT_REFRESH_TASK_ID.to_owned(),
                        instance.instance_id.clone(),
                        runtime,
                    ));
                }
            }
        }
        for (task_id, instance_id, runtime) in expected {
            let (Some(expected_runtime_ms), runs) = runtime.snapshot_members() else {
                continue;
            };
            let index = match facts
                .tasks
                .iter()
                .position(|state| state.task_id == task_id && state.instance_id == instance_id)
            {
                Some(index) => index,
                None => {
                    facts.tasks.push(TaskRuntimeSnapshot {
                        task_id,
                        instance_id,
                        last_dispatched_unix_ms: None,
                        eligible_since_unix_ms: None,
                        terminal_state: None,
                        completed_window: None,
                        last_duration_ms: None,
                        failure_streak: None,
                        expected_runtime_ms: None,
                        expected_runtime_runs: None,
                    });
                    facts.tasks.len() - 1
                }
            };
            facts.tasks[index].expected_runtime_ms = Some(expected_runtime_ms);
            facts.tasks[index].expected_runtime_runs = runs;
        }
        Ok(())
    }

    /// The `package_id` of each procedure whose binding names a location a scheduled run's
    /// preparation admitted a package at in this process, by procedure alias.
    fn procedure_packages(&self) -> RuntimeHostResult<BTreeMap<String, String>> {
        let admitted = self.pack_admission.prepared_package_ids()?;
        let manifest = lock(&self.procedure_manifest, RunDurationLocation::Project)?;
        Ok(manifest
            .as_ref()
            .map(|manifest| {
                manifest
                    .bindings()
                    .filter_map(|binding| {
                        let package_id = admitted.get(binding.package_path()?)?;
                        Some((binding.procedure_ref().to_owned(), package_id.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// SCH-M ED-2, test only: one sample of `package_id` on `alias` written as a settlement
    /// writes it, with the run's terminal at `terminal_unix_ms`.
    #[cfg(test)]
    pub(super) fn add_run_duration_sample_for_test(
        &self,
        alias: &str,
        package_id: &str,
        duration_ms: u64,
        terminal_unix_ms: u64,
    ) -> RuntimeHostResult<()> {
        let _gate = lock(&self.policy_outcome_gate, RunDurationLocation::RecordSample)?;
        self.add_run_duration_sample(alias, package_id, duration_ms, terminal_unix_ms)
    }

    /// SCH-M ED-2, test only: the rows of `alias`'s record as the store holds it now.
    #[cfg(test)]
    pub(super) fn run_durations_for_test(&self, alias: &str) -> RuntimeHostResult<RunDurations> {
        let active = self.active_fact_revision(
            &FactScope::Instance {
                instance_id: alias.to_owned(),
            },
            RUN_DURATION_FACT_KEY,
        )?;
        Ok(
            match active.as_ref().map(|(record, _, _)| &record.content) {
                Some(FactContent::Inline { value }) => RunDurations::read(Some(value)),
                _ => RunDurations::default(),
            },
        )
    }
}

/// Whether a task's scope covers the instance (its instance, its server or its game).
fn scope_covers(scope: &ScopeSelector, instance: &InstanceSnapshot) -> bool {
    match scope {
        ScopeSelector::Instance { instance_id } => *instance_id == instance.instance_id,
        ScopeSelector::Server { server_id } => *server_id == instance.server_id,
        ScopeSelector::Game { game_id } => *game_id == instance.game_id,
    }
}

/// The lowercase hex SHA-256 of the instance alias, the observation time and the record's
/// value: the publication's `source_snapshot_id` (`run_duration:<digest>`) and
/// `resource_bundle_hash`.
fn run_duration_digest(
    alias: &str,
    observed_at_unix_ms: u64,
    value: &ContractFactValue,
) -> RuntimeHostResult<String> {
    let bytes = serde_json::to_vec(&(alias, observed_at_unix_ms, value)).map_err(|_| {
        RuntimeHostError::fatal(
            HostCode::FactSnapshotEncodeFailed,
            RunDurationLocation::RecordSample,
            RuntimeErrorCode::RuntimeHalted,
        )
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
