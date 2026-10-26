// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #386 SR-4a3 (SCH-M frozen model v1.0 §3.2 D-3, §6 row SR-4a3; frozen scheduling
//! model v1.4.2 §5.2a, §5.3 rule 1; `contracts/scheduling-blackout.md`, "Complete silence" and
//! "Gradual quiet"): what the host's own entries read of the blackouts.
//!
//! - **One reader.** A blackout of an instance is SR-4a1's pure judgement
//!   (`actingcommand_policy::blackout_at` / `next_blackout_start`) over its three sources: the
//!   host list `scheduling.blackout.host` and the instance's own list
//!   (`instance.<id>.scheduling.blackout`) in effect now, and the active catalog's `maintenance`
//!   events, on the Runtime clock. Nothing here knows a game, a time of day or a rule.
//! - **The catalog source** is a copy of the active catalog's `maintenance` events, taken at the
//!   start and by every lease sweep tick (when the policy lock is free; a contended tick keeps
//!   the copy it has), never by an entry: an entry may hold an instance's admission guard, and
//!   copying events keeps no unloaded generation alive. A `maintenance` event the judgement cannot
//!   read (one with a duration and no V2 validity) is left out of the copy and recorded once per
//!   catalog generation as `blackout_config_invalid` (`field` its `validity` in the timeline
//!   document, its `blackout_id`, `blackout_source` `catalog`; Fail Loud).
//! - **Empty lists.** With no entry in either list and no `maintenance` event, no entry reads a
//!   blackout and every behaviour is the one before SR-4a3.
//! - **The sweep's step** (`sweep_blackout_silence`): for every registered instance the blackout
//!   that covers it now goes to direction ①'s counter (records made inside it count for nothing)
//!   and drops the ladder claims that wait for the instance (`recovery_ladder_suppressed`
//!   `blackout`). The startup package and the emulator autostart are held by the instance's
//!   routine gate (`ClaimGate::Silenced`); the sweep's own pump grants them at the first tick
//!   after the end, which is their bounded wait's wake.

use super::*;
use crate::codes::ConfigHostLocation;
use crate::failure_identity::identifier_text;
use actingcommand_contract::outcome::BlackoutSource;
use actingcommand_policy::{
    Blackout, BlackoutEntry, BlackoutSources, BlackoutWindow, TimelineEvent, TimelineEventKind,
};

/// The host's copy of the blackout sources it does not read from the configuration (a leaf
/// lock: nothing else is taken while it is held).
#[derive(Default)]
pub(super) struct BlackoutSilence {
    /// The hash of the catalog generation `maintenance` was copied from; `None` before the first
    /// copy or while no generation is active.
    catalog_hash: Option<String>,
    /// The readable `maintenance` events of that generation.
    maintenance: Arc<Vec<TimelineEvent>>,
    /// Test only: the due monitor probes a blackout skipped.
    #[cfg(test)]
    probes_silenced: u64,
}

/// One instance's blackout sources as the configuration in effect and the catalog copy hold
/// them now, with the gradual quiet's keys.
struct SilenceSources {
    host: Vec<BlackoutEntry>,
    own: BTreeMap<String, Vec<BlackoutEntry>>,
    catalog: Arc<Vec<TimelineEvent>>,
    quiet_margin_ms: u64,
    hard_quiet_ms: u64,
}

impl SilenceSources {
    fn empty(&self) -> bool {
        self.host.is_empty() && self.own.values().all(Vec::is_empty) && self.catalog.is_empty()
    }
}

/// A configuration entry as the pure judgement reads it.
fn judged(entries: Vec<actingcommand_runtime_config::BlackoutEntry>) -> Vec<BlackoutEntry> {
    entries
        .into_iter()
        .map(|entry| BlackoutEntry {
            id: entry.id,
            window: match entry.window {
                actingcommand_runtime_config::BlackoutWindow::Weekly {
                    weekdays,
                    utc_offset_minutes,
                    start_minute_of_day,
                    end_minute_of_day,
                } => BlackoutWindow::Weekly {
                    weekdays,
                    utc_offset_minutes,
                    start_minute_of_day,
                    end_minute_of_day,
                },
                actingcommand_runtime_config::BlackoutWindow::Once {
                    from_unix_ms,
                    until_unix_ms,
                } => BlackoutWindow::Once {
                    from_unix_ms,
                    until_unix_ms,
                },
            },
        })
        .collect()
}

/// A configuration key that cannot be read stops the Runtime, as every reader of the
/// configuration in effect does: its values passed their check when they entered.
fn config_fatal(failure: actingcommand_runtime_config::ConfigFailure) -> RuntimeHostError {
    RuntimeHostError::fatal(
        failure.failure_code(),
        ConfigHostLocation::ReadRuntimeConfig,
        RuntimeErrorCode::RuntimeHalted,
    )
    .with_outcome_values(|outcome| failure.with_values(outcome))
}

impl HostShared {
    /// The blackout that covers `instance_id` (whose alias is `alias`) now, or none.
    pub(super) fn blackout_covering(
        &self,
        instance_id: InstanceId,
        alias: &str,
    ) -> RuntimeHostResult<Option<Blackout>> {
        self.instance_silence(instance_id, alias, None)
    }

    /// Gradual quiet (`scheduling-blackout.md`, rules 1, 3 and 4) of work the Runtime starts on
    /// its own whose time limit is `limit_ms`: the blackout that covers the instance now, or the
    /// next one that starts within `limit_ms` plus `scheduling.blackout.quiet_margin_ms` (and
    /// from `start − hard_quiet_ms` on); none when the work may start.
    pub(super) fn blackout_within(
        &self,
        instance_id: InstanceId,
        alias: &str,
        limit_ms: u64,
    ) -> RuntimeHostResult<Option<Blackout>> {
        self.instance_silence(instance_id, alias, Some(limit_ms))
    }

    fn instance_silence(
        &self,
        instance_id: InstanceId,
        alias: &str,
        limit_ms: Option<u64>,
    ) -> RuntimeHostResult<Option<Blackout>> {
        let sources = self.silence_sources(instance_id, alias)?;
        if sources.empty() {
            return Ok(None);
        }
        let (server_id, game_id) = self.silence_identity(alias)?;
        let instance = InstanceSnapshot {
            instance_id: alias.to_owned(),
            server_id,
            game_id,
            host_id: String::new(),
            available: true,
            capability_operation_ids: Vec::new(),
            preferred_task_ids: Vec::new(),
        };
        let judged_sources = BlackoutSources {
            host: &sources.host,
            instances: &sources.own,
            catalog: &sources.catalog,
        };
        let sample = self.clock.sample()?;
        let time = EvaluationTime {
            unix_ms: sample.unix_ms,
            monotonic_ms: sample.monotonic_ms,
        };
        // The judgement fails only on an interval past Unix time or a broken union: the readable
        // sources cannot build one, so a failure is an invariant breach.
        let fatal = |error: actingcommand_policy::PolicyEvaluationError| {
            RuntimeHostError::from(error).into_fatal()
        };
        if let Some(covering) =
            actingcommand_policy::blackout_at(&judged_sources, &instance, time).map_err(fatal)?
        {
            return Ok(Some(covering));
        }
        let Some(limit_ms) = limit_ms else {
            return Ok(None);
        };
        let reach_ms = limit_ms
            .saturating_add(sources.quiet_margin_ms)
            .max(sources.hard_quiet_ms);
        actingcommand_policy::next_blackout_start(
            &judged_sources,
            &instance,
            time,
            time.unix_ms.saturating_add(reach_ms),
        )
        .map_err(fatal)
    }

    /// The configuration's two lists for the instance and the catalog copy.
    fn silence_sources(
        &self,
        instance_id: InstanceId,
        alias: &str,
    ) -> RuntimeHostResult<SilenceSources> {
        let (host, own, quiet_margin_ms, hard_quiet_ms) = {
            let config = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?;
            let blackout = config.scheduling().map_err(config_fatal)?.blackout;
            let own = config
                .instance_blackouts(&identifier_text(&instance_id))
                .map_err(config_fatal)?;
            (
                blackout.host,
                own,
                blackout.quiet_margin_ms,
                blackout.hard_quiet_ms,
            )
        };
        let catalog =
            Arc::clone(&lock(&self.blackout_silence, HostLocation::ReadRoutineGate)?.maintenance);
        Ok(SilenceSources {
            host: judged(host),
            own: BTreeMap::from([(alias.to_owned(), judged(own))]),
            catalog,
            quiet_margin_ms,
            hard_quiet_ms,
        })
    }

    /// The server and game the policy names the instance by (a catalog event's scope matches
    /// them); empty for an instance the policy does not name.
    fn silence_identity(&self, alias: &str) -> RuntimeHostResult<(String, String)> {
        Ok(lock(&self.policy_inputs, HostLocation::ReadPolicyInputs)?
            .as_ref()
            .and_then(|inputs| {
                inputs
                    .instance_identities()
                    .find(|identity| identity.instance_id == alias)
                    .map(|identity| (identity.server_id.to_owned(), identity.game_id.to_owned()))
            })
            .unwrap_or_default())
    }

    /// Copies the active catalog's readable `maintenance` events when its generation changed:
    /// at the start (`wait`, the policy lock taken) and at every sweep tick (only when the
    /// policy lock is free). An unreadable event is recorded once per generation and left out.
    pub(super) fn refresh_blackout_catalog(&self, wait: bool) -> RuntimeHostResult<()> {
        let Some(policy) = self.policy.get() else {
            return Ok(());
        };
        let active = if wait {
            lock(policy, HostLocation::ReadActivePolicyCatalog)?.active_loaded()
        } else {
            match policy.try_lock() {
                Ok(policy) => policy.active_loaded(),
                Err(TryLockError::WouldBlock) => return Ok(()),
                Err(TryLockError::Poisoned(_)) => {
                    return Err(lock_poison_error(HostLocation::ReadActivePolicyCatalog));
                }
            }
        };
        let hash = active
            .as_ref()
            .map(|catalog| catalog.compiled().catalog_hash().to_owned());
        if lock(&self.blackout_silence, HostLocation::ReadRoutineGate)?.catalog_hash == hash {
            return Ok(());
        }
        let mut readable = Vec::new();
        let mut unreadable = Vec::new();
        for (index, event) in active
            .iter()
            .flat_map(|catalog| {
                catalog
                    .compiled()
                    .catalog()
                    .timeline
                    .events
                    .iter()
                    .enumerate()
            })
            .filter(|(_, event)| {
                event.event_kind == TimelineEventKind::Maintenance && event.duration_ms > 0
            })
        {
            if event.validity.is_some() {
                readable.push(event.clone());
            } else {
                unreadable.push((index, event.id.clone()));
            }
        }
        drop(active);
        {
            let mut silence = lock(&self.blackout_silence, HostLocation::ReadRoutineGate)?;
            silence.catalog_hash = hash;
            silence.maintenance = Arc::new(readable);
        }
        for (index, blackout_id) in unreadable {
            let error = RuntimeHostError::request(
                actingcommand_runtime_config::ConfigCode::BlackoutConfigInvalid,
                HostLocation::ReadActivePolicyCatalog,
                RuntimeErrorCode::InvalidRequest,
            )
            .with_outcome_values(|outcome| {
                outcome
                    .field(format!("/events/{index}/validity"))
                    .blackout_id(blackout_id.as_str())
                    .blackout_source(BlackoutSource::Catalog)
            });
            self.append_lifecycle_failure(
                RuntimeLifecycleFailureStage::PolicyMonitor,
                RuntimeLifecycleFailure::Host(&error),
                self.events.system_links()?,
                None,
            )?;
        }
        Ok(())
    }

    /// The lease sweep's step (before its pump): the catalog copy, then for every registered
    /// instance the blackout that covers it now: direction ①'s counter learns it, and the ladder
    /// claims that wait for the instance are dropped.
    pub(super) fn sweep_blackout_silence(&self) -> RuntimeHostResult<()> {
        self.refresh_blackout_catalog(false)?;
        let instances = lock(
            &self.registered_instances,
            HostLocation::ReadInstanceRegistry,
        )?
        .values()
        .map(|instance| (instance.instance_id(), instance.instance_alias.clone()))
        .collect::<Vec<_>>();
        for (instance_id, alias) in instances {
            if self.fatal.is_shutdown_requested() {
                return Ok(());
            }
            let covering = self.blackout_covering(instance_id, &alias)?;
            lock(
                &self.stability_counter,
                HostLocation::RecoveryStabilityCount,
            )?
            .set_silenced(
                instance_id,
                covering
                    .as_ref()
                    .map(|blackout| (blackout.start_unix_ms, blackout.end_unix_ms)),
            );
            if let Some(blackout) = covering {
                self.drop_waiting_ladder_claims(instance_id, &blackout)?;
            }
        }
        Ok(())
    }

    /// The routine gate `gate` (the scheduling pauses and a #403 fence) with the instance's
    /// blackout silence laid over it: a blackout that covers it holds its startup package and its
    /// emulator autostart; one that starts before its startup package could end (the package's
    /// response deadline, rule 3) holds the startup package.
    pub(super) fn silenced_gate(
        &self,
        instance_id: InstanceId,
        alias: &str,
        gate: ClaimGate,
    ) -> RuntimeHostResult<ClaimGate> {
        let startup_ms = self
            .startup_packages
            .get()
            .and_then(|packages| packages.get(&instance_id))
            .map(ContainedTaskRequest::response_deadline_ms);
        let silence = match startup_ms {
            Some(limit_ms) => self.blackout_within(instance_id, alias, limit_ms)?,
            None => self.blackout_covering(instance_id, alias)?,
        };
        let Some(blackout) = silence else {
            return Ok(gate);
        };
        let covers = blackout.start_unix_ms <= self.clock.sample()?.unix_ms;
        Ok(gate.silenced(covers))
    }

    /// Whether a blackout covers the instance a due monitor probe of `alias` would observe: the
    /// probe then takes no frame and records nothing, and stays due.
    pub(super) fn monitor_probe_silenced(&self, alias: &str) -> RuntimeHostResult<bool> {
        let instance_id = lock(&self.registered_instances, HostLocation::ReadDueMonitors)?
            .values()
            .find(|instance| instance.instance_alias == alias)
            .map(RegisteredInstance::instance_id);
        let Some(instance_id) = instance_id else {
            return Ok(false);
        };
        let silenced = self.blackout_covering(instance_id, alias)?.is_some();
        #[cfg(test)]
        if silenced {
            lock(&self.blackout_silence, HostLocation::ReadDueMonitors)?.probes_silenced += 1;
        }
        Ok(silenced)
    }

    /// Test only: the due monitor probes a blackout skipped so far.
    #[cfg(test)]
    pub(super) fn monitor_probes_silenced_for_test(&self) -> RuntimeHostResult<u64> {
        Ok(lock(&self.blackout_silence, HostLocation::ReadDueMonitors)?.probes_silenced)
    }
}
