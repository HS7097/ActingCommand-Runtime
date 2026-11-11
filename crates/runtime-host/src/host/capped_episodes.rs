// SPDX-License-Identifier: AGPL-3.0-only

//! #428 SCHED-C1d (register §1.299 SC1-11; `contracts/scheduling/README.md`, "Episodes
//! survive"; `contracts/runtime-fact-store.md`, `host.capped_episodes`): the capped episodes of
//! the scheduler outlive a start. Iron rule 13 (Alice, and her rulings of 10-09 20:2x and 20:5x):
//! no state file; they are the tagged program fact `host.capped_episodes`, which the ledger
//! records as every program fact and a start loads into memory before the first evaluation, as
//! `host.idle_state` is kept.
//!
//! - **The fact.** Runtime scope, a record list, no lifetime, source `runtime`. Its first row
//!   names the writer, `start_anchor` alone: the start event sequence of the start that recorded
//!   it, as `host.contained_settlement` names it (#405 N-2). Then, per instance and pool with an
//!   open or waiting episode, one episode row (`instance_alias`, `pool_id`, `capped_episode` its
//!   number, `capped_round` its rounds, and while a round is under way `round_started_at_unix_ms`
//!   and `pool_reading`, the reading its plan starts from), its consumer rows (`consumer_task_id`,
//!   `consumed`, `limited`: the consumers key) and, while a round is under way, its member rows in
//!   the round's frozen order (`member_task_id`, `consumed`, and `passed` true with its
//!   `gate_code` when it has one for a member the round passed). A record row holds scalars only,
//!   so an episode's lists are rows of their own after it.
//! - **When.** After every evaluated cycle whose episodes changed, and once by a start that found
//!   the fact, under its own anchor. A Runtime that never has an episode records none.
//! - **The anchor rule** (#405 N-2, as `host.idle_state`'s): a start keeps the fact only when its
//!   writer is the last start that completed its settlement (the anchor `host.contained_settlement`
//!   named before this start). A fact another start recorded (a start that kept none ran in
//!   between, such as a rollback's older build) is stale and dropped; an owner that settled no
//!   start in between (an offline ledger maintenance) leaves it current.
//! - **Failures are warnings** (#419). A value that does not read back, breaks its rules, is
//!   stale by the anchor rule, or names an instance that is not registered or a pool the active
//!   catalog does not give a declared valuation is dropped (whole, or that episode alone) with
//!   `capped_state_failed` (`operation` `load_capped_episodes`), and the start goes on; a record
//!   the fact store refuses is the same warning (`operation` `store_capped_episodes`) and the
//!   Runtime goes on with the episodes in memory, recording them again at the next change. A
//!   fatal fault of the store or the ledger stays fatal, as for every fact. Each warning is one
//!   `runtime.failed` record at Warning under the system links, handed to the daemon's log too.

use super::contained_task::StartAnchors;
use super::*;
use crate::codes::{CappedStateCode, CappedStateLocation, HostCode};
use actingcommand_contract::outcome::{Code, CodeStr, Outcome};
use actingcommand_policy::{CappedStateProblem, KeptConsumer, KeptEpisode, KeptMember, KeptRound};

/// The tagged program fact of the capped episodes (`ledger_observer.tag.workbench_patterns`).
pub(super) const CAPPED_EPISODES_FACT_KEY: &str = "host.capped_episodes";
/// The member of the writer row.
const ROW_ANCHOR: &str = "start_anchor";
/// The members of an episode row.
const ROW_INSTANCE: &str = "instance_alias";
const ROW_POOL: &str = "pool_id";
const ROW_NUMBER: &str = "capped_episode";
const ROW_ROUNDS: &str = "capped_round";
const ROW_ROUND_START: &str = "round_started_at_unix_ms";
const ROW_READING: &str = "pool_reading";
/// The members of a consumer row.
const ROW_CONSUMER: &str = "consumer_task_id";
const ROW_LIMITED: &str = "limited";
/// The members of a member row.
const ROW_MEMBER: &str = "member_task_id";
const ROW_PASSED: &str = "passed";
const ROW_GATE: &str = "gate_code";
/// Of a consumer row and a member row: `⌈C/1000⌉` of one run.
const ROW_CONSUMED: &str = "consumed";

#[cfg(test)]
thread_local! {
    /// SCHED-C1d, test builds only: the next record of `host.capped_episodes` on this thread is
    /// refused as the fact store refuses a record list too large for it, and nothing is written.
    pub(crate) static CAPPED_EPISODES_STORE_REFUSED_FOR_TEST: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// `value` as a record integer. A start anchor, an episode's number and rounds and a consumption
/// stay far below `i64::MAX`; one above it would read back as another value, so it saturates.
fn integer(value: u64) -> FactScalar {
    FactScalar::Integer(i64::try_from(value).unwrap_or(i64::MAX))
}

#[cfg(test)]
impl RuntimeHost {
    /// SCHED-C1d, test only: the capped episodes the policy host holds.
    pub(crate) fn capped_episodes_for_test(
        &self,
    ) -> RuntimeHostResult<actingcommand_policy::CappedMemory> {
        let read = CappedStateLocation::Load;
        Ok(lock(self.shared_ref(read)?.policy()?, read)?.capped_episodes_for_test())
    }
}

/// The rows of `host.capped_episodes` for `episodes` under `anchor` (module doc).
fn capped_rows(episodes: &[KeptEpisode], anchor: u64) -> Vec<BTreeMap<String, FactScalar>> {
    let mut rows = vec![BTreeMap::from([(ROW_ANCHOR.to_owned(), integer(anchor))])];
    for episode in episodes {
        let mut row = BTreeMap::from([
            (
                ROW_INSTANCE.to_owned(),
                FactScalar::String(episode.instance_id.clone()),
            ),
            (
                ROW_POOL.to_owned(),
                FactScalar::String(episode.pool_id.clone()),
            ),
            (ROW_NUMBER.to_owned(), integer(episode.number)),
            (ROW_ROUNDS.to_owned(), integer(episode.rounds)),
        ]);
        if let Some(round) = &episode.round {
            row.insert(
                ROW_ROUND_START.to_owned(),
                FactScalar::TimestampMs(round.started_at_unix_ms),
            );
            row.insert(ROW_READING.to_owned(), FactScalar::Integer(round.reading));
        }
        rows.push(row);
        for consumer in &episode.consumers {
            rows.push(BTreeMap::from([
                (
                    ROW_CONSUMER.to_owned(),
                    FactScalar::String(consumer.task_id.clone()),
                ),
                (ROW_CONSUMED.to_owned(), integer(consumer.consumed)),
                (
                    ROW_LIMITED.to_owned(),
                    FactScalar::Boolean(consumer.limited),
                ),
            ]));
        }
        for member in episode.round.iter().flat_map(|round| &round.order) {
            let mut row = BTreeMap::from([
                (
                    ROW_MEMBER.to_owned(),
                    FactScalar::String(member.task_id.clone()),
                ),
                (ROW_CONSUMED.to_owned(), integer(member.consumed)),
            ]);
            if member.passed {
                row.insert(ROW_PASSED.to_owned(), FactScalar::Boolean(true));
            }
            if let Some(gate) = member.gate_code {
                row.insert(
                    ROW_GATE.to_owned(),
                    FactScalar::String(gate.as_str().to_owned()),
                );
            }
            rows.push(row);
        }
    }
    rows
}

/// The writer's anchor and the kept episodes of a `host.capped_episodes` record; `None` when it
/// is not of the shape this build records (not the Runtime's own record list, no writer row
/// first, a row of no kind or of another member, a value of another type, a consumer or member
/// row before its episode row, a member row of an episode without a round, a gate code this
/// build does not register).
fn kept_capped(record: &RuntimeFactRecord) -> Option<(u64, Vec<KeptEpisode>)> {
    if record.source != OriginModule::Runtime {
        return None;
    }
    let ContractFactValue::RecordList(rows) = &record.value else {
        return None;
    };
    let (writer, rows) = rows.split_first()?;
    let anchor = match writer.get(ROW_ANCHOR) {
        Some(FactScalar::Integer(anchor)) if writer.len() == 1 => u64::try_from(*anchor).ok()?,
        _ => return None,
    };
    let text = |row: &BTreeMap<String, FactScalar>, member: &str| match row.get(member) {
        Some(FactScalar::String(value)) => Some(value.clone()),
        _ => None,
    };
    let count = |row: &BTreeMap<String, FactScalar>, member: &str| match row.get(member) {
        Some(FactScalar::Integer(value)) => u64::try_from(*value).ok(),
        _ => None,
    };
    let mut episodes = Vec::<KeptEpisode>::new();
    for row in rows {
        if row.contains_key(ROW_NUMBER) {
            let round = match (row.get(ROW_ROUND_START), row.get(ROW_READING)) {
                (None, None) if row.len() == 4 => None,
                (Some(FactScalar::TimestampMs(at)), Some(FactScalar::Integer(reading)))
                    if row.len() == 6 =>
                {
                    Some(KeptRound {
                        started_at_unix_ms: *at,
                        reading: *reading,
                        order: Vec::new(),
                    })
                }
                _ => return None,
            };
            episodes.push(KeptEpisode {
                instance_id: text(row, ROW_INSTANCE)?,
                pool_id: text(row, ROW_POOL)?,
                number: count(row, ROW_NUMBER)?,
                rounds: count(row, ROW_ROUNDS)?,
                consumers: Vec::new(),
                round,
            });
        } else if row.contains_key(ROW_CONSUMER) {
            let episode = episodes.last_mut()?;
            let limited = match row.get(ROW_LIMITED) {
                Some(FactScalar::Boolean(limited)) if row.len() == 3 => *limited,
                _ => return None,
            };
            // The consumers key comes before the round's members.
            if episode
                .round
                .as_ref()
                .is_some_and(|round| !round.order.is_empty())
            {
                return None;
            }
            episode.consumers.push(KeptConsumer {
                task_id: text(row, ROW_CONSUMER)?,
                consumed: count(row, ROW_CONSUMED)?,
                limited,
            });
        } else if row.contains_key(ROW_MEMBER) {
            let round = episodes.last_mut()?.round.as_mut()?;
            let passed = match row.get(ROW_PASSED) {
                None => false,
                Some(FactScalar::Boolean(true)) => true,
                Some(_) => return None,
            };
            let gate_code = match row.get(ROW_GATE) {
                None => None,
                Some(FactScalar::String(spelling)) => Some(registered_code(spelling)?),
                Some(_) => return None,
            };
            let members = 2 + usize::from(passed) + usize::from(gate_code.is_some());
            if row.len() != members {
                return None;
            }
            round.order.push(KeptMember {
                task_id: text(row, ROW_MEMBER)?,
                consumed: count(row, ROW_CONSUMED)?,
                passed,
                gate_code,
            });
        } else {
            return None;
        }
    }
    Some((anchor, episodes))
}

/// The registered code `spelling` names, by the catalog this build embeds.
fn registered_code(spelling: &str) -> Option<Code> {
    serde_json::from_value::<CodeStr>(serde_json::Value::String(spelling.to_owned()))
        .ok()?
        .registered()
}

/// `capped_state_failed` at `operation`, naming the fact.
fn capped_state_failed(operation: CappedStateLocation) -> Outcome {
    Outcome::new(CappedStateCode::Failed)
        .operation(operation)
        .fact_key(CAPPED_EPISODES_FACT_KEY)
}

impl HostShared {
    /// SCHED-C1d: the start's restore of the capped episodes from `host.capped_episodes`, after
    /// the start's settlement (`anchors`) and the scheduling pauses and before the first
    /// evaluation (module doc). Each dropped value or episode is one warning; the start goes on.
    /// A start that found the fact records it again under its own anchor.
    pub(super) fn restore_capped_episodes(&self, anchors: StartAnchors) -> RuntimeHostResult<()> {
        let load = CappedStateLocation::Load;
        let recorded = lock(&self.runtime_facts, load)?
            .get(&RuntimeFactScope::Runtime, CAPPED_EPISODES_FACT_KEY)
            .cloned();
        let registered = lock(&self.registered_instances, load)?
            .values()
            .map(|instance| instance.instance_alias.clone())
            .collect::<BTreeSet<_>>();
        let mut warnings = Vec::new();
        let mut kept = Vec::new();
        if let Some(record) = &recorded {
            match kept_capped(record) {
                // Not of this build's shape: dropped whole.
                None => warnings.push(capped_state_failed(load)),
                // Stale by the anchor rule: its writer is not the last start that completed its
                // settlement (a start that kept none ran in between).
                Some((writer, _)) if anchors.previous != Some(writer) => {
                    warnings.push(capped_state_failed(load));
                }
                Some((_, episodes)) => {
                    for episode in episodes {
                        if registered.contains(&episode.instance_id) {
                            kept.push(episode);
                        } else {
                            warnings.push(
                                capped_state_failed(load)
                                    .instance_id(episode.instance_id.as_str())
                                    .pool_id(episode.pool_id.as_str()),
                            );
                        }
                    }
                }
            }
        }
        let restored = lock(self.policy()?, load)?.restore_capped_episodes(
            kept,
            anchors.current,
            recorded.is_some(),
        );
        match restored {
            Ok(dropped) => {
                for (instance_id, pool_id) in dropped {
                    warnings.push(
                        capped_state_failed(load)
                            .instance_id(instance_id.as_str())
                            .pool_id(pool_id.as_str()),
                    );
                }
            }
            // Kept episodes that break the shape's rules: dropped whole, naming the pair.
            Err(
                CappedStateProblem::Repeated {
                    instance_id,
                    pool_id,
                }
                | CappedStateProblem::Invalid {
                    instance_id,
                    pool_id,
                },
            ) => warnings.push(
                capped_state_failed(load)
                    .instance_id(instance_id.as_str())
                    .pool_id(pool_id.as_str()),
            ),
        }
        for warning in warnings {
            self.report_capped_state_failed(
                warning,
                load,
                RuntimeLifecycleFailureStage::PolicyInitialization,
            )?;
        }
        self.keep_capped_episodes(RuntimeLifecycleFailureStage::PolicyInitialization)
    }

    /// SCHED-C1d: records `host.capped_episodes` when the policy host's capped episodes differ
    /// from what it holds: after an evaluated cycle (`stage` the policy driver's), and once by a
    /// start that found the fact (the policy initialization's). A record the fact store refuses
    /// is the warning `capped_state_failed` (`operation` `store_capped_episodes`) and the Runtime
    /// goes on with the episodes in memory; the next change records them again. A fatal failure
    /// stays fatal.
    pub(super) fn keep_capped_episodes(
        &self,
        stage: RuntimeLifecycleFailureStage,
    ) -> RuntimeHostResult<()> {
        let store = CappedStateLocation::Store;
        let Some((anchor, kept)) = lock(self.policy()?, store)?.unrecorded_capped_episodes() else {
            return Ok(());
        };
        // #405 N-2, as `host.idle_state`: the record names this start's anchor, which the start's
        // restore bound; a start whose settlement found no start event has none, and a record it
        // could not name is no record.
        let anchor = anchor.ok_or_else(|| {
            RuntimeHostError::fatal(
                HostCode::ContainedSettlementAnchorInvalid,
                store,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let recorded = self.record_capped_episodes(&kept, anchor);
        lock(self.policy()?, store)?.note_capped_episodes_recorded(kept);
        match recorded {
            Ok(()) => Ok(()),
            Err(error) if error.is_fatal() => Err(error),
            Err(error) => self.report_capped_state_failed(
                capped_state_failed(store).caused_by(error.record_outcome()),
                store,
                stage,
            ),
        }
    }

    /// `host.capped_episodes` of `kept` under `anchor`, observed strictly after the previous
    /// record (a manual clock may not move between two).
    fn record_capped_episodes(&self, kept: &[KeptEpisode], anchor: u64) -> RuntimeHostResult<()> {
        #[cfg(test)]
        if CAPPED_EPISODES_STORE_REFUSED_FOR_TEST.with(|refused| refused.replace(false)) {
            return Err(RuntimeHostError::request(
                actingcommand_contract::codes::SanitizationCode::RuntimeFactRecordListTooLarge,
                CappedStateLocation::Store,
                RuntimeErrorCode::InvalidRequest,
            ));
        }
        let rows = capped_rows(kept, anchor);
        let now = self.runtime_clock_sample()?.unix_ms;
        let previous = lock(&self.runtime_facts, CappedStateLocation::Store)?
            .get(&RuntimeFactScope::Runtime, CAPPED_EPISODES_FACT_KEY)
            .map(|record| record.observed_at_unix_ms);
        self.record_runtime_fact_with_event(RuntimeFactRecord {
            scope: RuntimeFactScope::Runtime,
            key: CAPPED_EPISODES_FACT_KEY.to_owned(),
            value: ContractFactValue::RecordList(rows),
            observed_at_unix_ms: previous
                .map_or(now, |previous| now.max(previous.saturating_add(1))),
            source: OriginModule::Runtime,
            ttl_ms: None,
        })
        .map(|_| ())
    }

    /// One Warning `runtime.failed` `capped_state_failed` (`outcome`, at `operation`) under the
    /// system links, at `stage`, handed to the daemon's log too.
    fn report_capped_state_failed(
        &self,
        outcome: Outcome,
        operation: CappedStateLocation,
        stage: RuntimeLifecycleFailureStage,
    ) -> RuntimeHostResult<()> {
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
                    "policy_capped_episodes",
                    stage.location(),
                    "runtime_host",
                    operation.as_str(),
                ),
                AuditInput::new(),
            ),
        )?;
        self.fatal.report_warning(&outcome);
        Ok(())
    }
}
