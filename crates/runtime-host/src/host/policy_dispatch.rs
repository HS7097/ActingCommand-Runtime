// SPDX-License-Identifier: AGPL-3.0-only

use super::contained_task::{
    PauseDrainEnd, SCHEDULING_PAUSE_DRAIN_POLL_INTERVAL, SCHEDULING_PAUSE_RETRY_INTERVAL,
    SchedulingPauseDeadlines, pause_drain_incomplete, pause_drain_interrupted,
};
use super::failure_scope::Contained;
use super::resource_close::RetainedClose;
use super::scheduling_pause::{PauseOrigin, pause_row};
use super::*;
use crate::codes::HostCode;
use crate::codes::HostLocation;
use crate::codes::SchedulingRuleHostCode;
use crate::fact_store::LeftOutRecordLists;
use crate::policy_host::{AvailabilityWithdrawals, DeviceQueue, InstanceOccupancy, SetupCall};
use crate::{PolicyRecomputeDirective, PolicyRecomputeKind, PolicyRecomputeReason};
use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::outcome::{CauseRelation, Code, Location, Outcome};
use actingcommand_contract::{
    BackendObservationStatus, BackendOpenEntry, InstancePauseStage, InstancePauseState, OwnerEpoch,
    PauseObstacle, PolicyRandomDelay, SchedulingDrainSummary, SchedulingPauseExpectation,
    SchedulingPauseMode, SchedulingPauseReason, SchedulingPauseScope, SchedulingPauseState,
    SchedulingResumeCaptureCheck, SchedulingResumeSelfCheck, SchedulingResumeTouchCheck,
};

/// Workflow #369 W-1 (review L-6): how long a policy admission retries an admission guard that
/// a keyless observe or probe holds, before it ends its intent as contended.
const POLICY_ADMISSION_CONTENTION_RETRY: Duration = Duration::from_millis(100);
/// Workflow #369 W-1: how far ahead a contended dispatch may run again.
const DISPATCH_CONTENDED_RETRY_AFTER_MS: u64 = 1_000;

/// #503 CYC-1 (register §1.526, ruling 1): `lock` for one of the gates a policy cycle reads its
/// inputs under, adding the microseconds the cycle waited for it to `waited` (on the cycle's lap
/// clock). Tests: the delay planned for the gates' wait is taken while the first one is waited for.
fn lock_waited<'a, T>(
    mutex: &'a std::sync::Mutex<T>,
    operation: HostLocation,
    waited: &mut u64,
) -> RuntimeHostResult<std::sync::MutexGuard<'a, T>> {
    let waiting = crate::policy_host::lap_instant();
    #[cfg(test)]
    crate::policy_host::delay_cycle_stretch_for_test(crate::policy_host::CycleStretch::GatesWait);
    let guard = lock(mutex, operation)?;
    *waited = waited.saturating_add(crate::policy_host::micros_since(waiting));
    Ok(guard)
}

/// Workflow #369 W-1: a dispatch refused because its instance is held, or because its admission
/// guard stayed contended, is an Info record; every other refusal stays an Error.
fn policy_rejection_severity(code: &str) -> EventSeverity {
    // Workflow #386 SR-4a2: the blackout and day-boundary refusals are waits, of category info.
    // Workflow #437 MAPP-1b: a member's wait for another member of its device is the device's
    // form of `dispatch_instance_held`, of category info.
    if code == HostCode::DispatchInstanceHeld.as_str()
        || code
            == actingcommand_policy::PolicyCode::DeviceGroupWaiting
                .code()
                .as_str()
        || code == HostCode::DispatchInstanceContended.as_str()
        || code == SchedulingRuleHostCode::BlackoutActive.as_str()
        || code == SchedulingRuleHostCode::BlackoutApproaching.as_str()
        || code == SchedulingRuleHostCode::PolicyDayBoundaryApproaching.as_str()
    {
        EventSeverity::Info
    } else {
        EventSeverity::Error
    }
}

/// Workflow #369 W-1 (review L-6): the refusal that ends a dispatch intent whose admission guard
/// stayed taken; it may run again about one second later. Workflow #378 A4 (part B L3): its
/// `instance_id` is the resolved instance's opaque identifier, not the policy's alias.
fn dispatch_contended_error(
    now_unix_ms: u64,
    instance_id: InstanceId,
) -> RuntimeHostResult<RuntimeHostError> {
    let next_eligible_unix_ms = now_unix_ms
        .checked_add(DISPATCH_CONTENDED_RETRY_AFTER_MS)
        .ok_or_else(|| {
            policy_admission_fatal(
                HostCode::PolicyAdmissionClockOverflow,
                HostLocation::AdmitPolicyDispatch,
            )
        })?;
    let mut error = RuntimeHostError::with_projection(
        HostCode::DispatchInstanceContended,
        HostLocation::AdmitPolicyDispatch,
        RuntimeErrorProjection::new(RuntimeErrorCode::InvalidRequest, false)
            .with_retry_after(DISPATCH_CONTENDED_RETRY_AFTER_MS),
    )
    .with_outcome_values(|outcome| {
        outcome
            .instance_id(crate::failure_identity::identifier_text(&instance_id))
            .next_eligible_unix_ms(next_eligible_unix_ms)
    });
    let mut rejection = error.policy_rejection();
    rejection.next_eligible_unix_ms = Some(next_eligible_unix_ms);
    error.lifecycle.policy_rejection = Some(Box::new(rejection));
    Ok(error)
}

/// Workflow #369 W-1: what the one queue of an instance shows to policy.
impl HostShared {
    /// Every registered instance's occupancy as a policy cycle starts, by alias.
    fn policy_instance_occupancy(
        &self,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<BTreeMap<String, InstanceOccupancy>> {
        let instances = lock(
            &self.registered_instances,
            HostLocation::ReadPolicyInstanceRegistry,
        )?
        .values()
        .map(|instance| (instance.instance_id(), instance.instance_alias.clone()))
        .collect::<Vec<_>>();
        let mut occupancy = BTreeMap::new();
        for (instance_id, instance_alias) in instances {
            if let Some(state) = self.instance_occupancy(instance_id, now_unix_ms)? {
                occupancy.insert(instance_alias, state);
            }
        }
        Ok(occupancy)
    }

    /// Workflow #437 MAPP-1 (`contracts/device-group.md`, "One pick per device per round"): the
    /// device each registered member of a device group of several runs on, by alias, as the
    /// scheduler keys it (the text of its device key), for the evaluator's one pick per device.
    /// An instance alone on its device is not named, and the evaluator keys it by its own alias
    /// as before; memory only.
    ///
    /// Workflow #437 ORD-1c (`contracts/scheduling/README.md`, "Order within one device"): when
    /// the map names at least one device of several, with the order's inputs
    /// (`DeviceGroups::with_order`): the member the host holds in front of each device by alias
    /// (`HostShared::device_fronts`), the queue-wait starts this evaluation applied
    /// (`wait_starts`) of the members not in front, and
    /// `scheduling.device_group.max_queue_wait_ms` in effect; and the members it names, each with
    /// whether it is in front, for the evaluation's queue-wait verdicts. Otherwise the map alone,
    /// exactly as before the slice, and no member.
    fn policy_device_groups(
        &self,
        wait_starts: &BTreeMap<(String, String), u64>,
    ) -> RuntimeHostResult<(actingcommand_policy::DeviceGroups, DeviceQueue)> {
        let instances = lock(
            &self.registered_instances,
            HostLocation::ReadPolicyInstanceRegistry,
        )?
        .values()
        .map(|instance| (instance.instance_id(), instance.instance_alias.clone()))
        .collect::<Vec<_>>();
        let devices = {
            let scheduler = lock(&self.scheduler, HostLocation::ReadPolicyInstanceOccupancy)?;
            instances
                .into_iter()
                .map(|(instance_id, alias)| (scheduler.device_key(instance_id), instance_id, alias))
                .collect::<Vec<_>>()
        };
        let mut members = BTreeMap::<InstanceId, Vec<(InstanceId, String)>>::new();
        for (device, instance_id, alias) in devices {
            members
                .entry(device)
                .or_default()
                .push((instance_id, alias));
        }
        let several = members
            .into_iter()
            .filter(|(_, aliases)| aliases.len() > 1)
            .collect::<Vec<_>>();
        let by_alias = several
            .iter()
            .flat_map(|(device, aliases)| {
                let device = crate::failure_identity::identifier_text(device);
                aliases
                    .iter()
                    .map(move |(_, alias)| (alias.clone(), device.clone()))
            })
            .collect();
        let groups = actingcommand_policy::DeviceGroups::new(by_alias);
        if several.is_empty() || !crate::policy_host::queue_order_on() {
            return Ok((groups, DeviceQueue::default()));
        }
        let fronts = self.device_fronts()?;
        let mut order = actingcommand_policy::DeviceOrder::default()
            .default_max_queue_wait_ms(self.max_queue_wait_ms()?);
        let mut queue = BTreeMap::new();
        for (device, aliases) in &several {
            let front = fronts.get(device);
            for (instance_id, alias) in aliases {
                let in_front = front == Some(instance_id);
                if in_front {
                    order = order.front(alias.clone());
                }
                queue.insert(alias.clone(), in_front);
            }
        }
        for ((task_id, alias), unix_ms) in wait_starts {
            if queue.get(alias) == Some(&false) {
                order = order.wait_start(task_id.clone(), alias.clone(), *unix_ms);
            }
        }
        Ok((groups.with_order(order), DeviceQueue::new(queue)))
    }

    /// Q-3 (6): policy dispatches only to an instance with no holder, no eligible waiting claim
    /// and no takeover cooldown. Read under the scheduler lock, with the routine gate read
    /// just before it.
    pub(super) fn instance_occupancy(
        &self,
        instance_id: InstanceId,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<Option<InstanceOccupancy>> {
        // Workflow #437 MAPP-1b (`contracts/device-group.md`, "One queue, one worker, one lease per
        // device"): a member of a device group of several reads its device across the group.
        if self.in_group_of_several(instance_id) {
            return Ok(self.device_occupancy(instance_id, now_unix_ms)?.occupancy);
        }
        let gate = self.routine_gate(instance_id)?;
        let now_monotonic_ms = self.monotonic_ms()?;
        // Workflow #427-A (C3): a policy run handed to the instance's worker holds the instance
        // from its hand-off to its settlement, also once its lease is released.
        let lane_held = self.policy_run_registered(instance_id)?;
        let scheduler = lock(&self.scheduler, HostLocation::ReadPolicyInstanceOccupancy)?;
        let count = scheduler.eligible_count(instance_id, gate, now_monotonic_ms);
        // A lapsed holder is no hold: the sweep, or the admission's own expiry, ends it.
        let holder_kind = scheduler
            .active_lease(instance_id)
            .filter(|lease| lease.token().expires_at_monotonic_ms() > now_monotonic_ms)
            .map(|lease| lease.kind())
            .or_else(|| lane_held.then_some(ClaimKind::PolicyDispatch))
            .or_else(|| scheduler.first_eligible_kind(instance_id, gate, now_monotonic_ms));
        if let Some(holder_kind) = holder_kind {
            return Ok(Some(InstanceOccupancy::Held { holder_kind, count }));
        }
        let Some(remaining_ms) = scheduler.cooldown_remaining_ms(instance_id, now_monotonic_ms)
        else {
            return Ok(None);
        };
        let until_unix_ms = now_unix_ms.checked_add(remaining_ms).ok_or_else(|| {
            policy_admission_fatal(
                HostCode::PolicyAdmissionClockOverflow,
                HostLocation::ReadPolicyInstanceOccupancy,
            )
        })?;
        Ok(Some(InstanceOccupancy::Cooldown { until_unix_ms }))
    }

    /// Workflow #427-A (C1), test builds and the `test-hooks` feature: runs the hook a test set
    /// for the point between a policy evaluation's sample and its validation, then reports a
    /// forced snapshot change (`policy_outcome_snapshot_changed`, as the validation would) while
    /// the test's count of them lasts.
    #[cfg(any(test, feature = "test-hooks"))]
    fn between_policy_sample_and_validation_for_test(&self) -> RuntimeHostResult<()> {
        let hook = lock(
            &self.policy_sample_hook_for_test,
            HostLocation::EvaluatePolicyCycle,
        )?
        .take();
        if let Some(hook) = hook {
            hook();
        }
        let forced = self
            .policy_snapshot_changes_for_test
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_sub(1)
            })
            .is_ok();
        if forced {
            return Err(RuntimeHostError::request(
                HostCode::PolicyOutcomeSnapshotChanged,
                HostLocation::ValidatePolicyOutcomeKeySnapshot,
                RuntimeErrorCode::InvalidRequest,
            ));
        }
        Ok(())
    }

    /// Workflow #427-A fix 1, test builds and the `test-hooks` feature: runs, once, the hook a
    /// test set for a scheduling pause's release that waits for a policy run an instance worker
    /// holds.
    #[cfg(any(test, feature = "test-hooks"))]
    fn pause_waits_for_lane_for_test(&self) -> RuntimeHostResult<()> {
        let hook = lock(
            &self.pause_lane_wait_hook_for_test,
            HostLocation::ReadPolicyRunsInFlight,
        )?
        .take();
        if let Some(hook) = hook {
            hook();
        }
        Ok(())
    }

    /// #480 PG-1, test builds: runs, once, the hook a test set for an instance pause's hand-back
    /// that finds its instance's key held; fix round 1: and counts every such check.
    #[cfg(test)]
    fn pause_release_meets_lease_for_test(&self) -> RuntimeHostResult<()> {
        self.pause_release_lease_meetings_for_test
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let hook = lock(
            &self.pause_release_lease_hook_for_test,
            HostLocation::ReleasePausedInstance,
        )?
        .take();
        if let Some(hook) = hook {
            hook();
        }
        Ok(())
    }

    /// #480 PG-2c fix round 1, test builds: runs, once, the hook a test set for a continuation of
    /// a standing pause that finds its instance's admission guard taken.
    #[cfg(test)]
    fn pause_meets_taken_guard_for_test(&self) -> RuntimeHostResult<()> {
        let hook = lock(
            &self.pause_guard_taken_hook_for_test,
            HostLocation::ReleasePausedInstance,
        )?
        .take();
        if let Some(hook) = hook {
            hook();
        }
        Ok(())
    }

    /// #480 PG-2c fix round 1, test builds: runs, once, the hook a test set for a check of a
    /// standing pause that failed and whose `check_failed` is noted.
    #[cfg(test)]
    fn pause_check_failed_for_test(&self) -> RuntimeHostResult<()> {
        let hook = lock(
            &self.pause_check_failed_hook_for_test,
            HostLocation::ReleasePausedInstance,
        )?
        .take();
        if let Some(hook) = hook {
            hook();
        }
        Ok(())
    }

    /// #480 PG-2c fix round 1, test builds: while the test's count of them lasts, a check of a
    /// pause's hand-back fails as a failure of the Runtime that is not fatal and concerns only
    /// the operation does (`pause_check_failed_for_test`).
    #[cfg(test)]
    fn injected_pause_check_failure_for_test(&self) -> Result<(), RequestFailure> {
        let injected = self
            .pause_check_failures_for_test
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_sub(1)
            })
            .is_ok();
        if injected {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    crate::codes::test_spellings::TestCode::PauseCheckFailedForTest,
                    HostLocation::ReleasePausedInstance,
                    RuntimeErrorCode::InvalidRequest,
                ),
                RuntimeReceiptState::Failed,
                None,
            ));
        }
        Ok(())
    }

    /// #480 PG-2c fix round 1, test builds: runs, once, the hook a test set for a person's resume
    /// of a pause not yet `released`, between its read of the stage and its persist gate.
    #[cfg(test)]
    fn before_unquiet_resume_for_test(&self) -> RuntimeHostResult<()> {
        let hook = lock(
            &self.unquiet_resume_hook_for_test,
            HostLocation::ReleasePausedInstance,
        )?
        .take();
        if let Some(hook) = hook {
            hook();
        }
        Ok(())
    }

    /// #480 PG-2c fix round 2 (item 5), test builds: runs, once, the hook a test set for a `stand`
    /// pause's hand-back just before it asks whether it may close the session.
    #[cfg(test)]
    fn before_hand_back_close_question_for_test(&self) -> RuntimeHostResult<()> {
        let hook = lock(
            &self.hand_back_question_hook_for_test,
            HostLocation::ReleasePausedInstance,
        )?
        .take();
        if let Some(hook) = hook {
            hook();
        }
        Ok(())
    }

    /// #480 PG-2c fix round 2 (item 5), test builds: runs, once, the hook a test set for a
    /// person's resume that marked its lift without a reconnect and has not lifted yet.
    #[cfg(test)]
    fn after_resume_marks_lifting_for_test(&self) -> RuntimeHostResult<()> {
        let hook = lock(
            &self.resume_lifting_hook_for_test,
            HostLocation::ReleasePausedInstance,
        )?
        .take();
        if let Some(hook) = hook {
            hook();
        }
        Ok(())
    }

    /// #480 PG-2d fix round 1, test builds: runs, once, the hook a test set for an instance pause's
    /// request whose stages ended standing or kept at a shutdown, before it takes the persist gate
    /// to record that moment (`record_held_pause_moment`). #480 PG-2e: and for a continuation whose
    /// stages ended `released` or kept at a shutdown, whichever of the two comes first.
    #[cfg(test)]
    fn before_held_pause_record_for_test(&self) -> RuntimeHostResult<()> {
        let hook = lock(
            &self.held_pause_record_hook_for_test,
            HostLocation::PauseInstanceScheduling,
        )?
        .take();
        if let Some(hook) = hook {
            hook();
        }
        Ok(())
    }

    /// #480 PG-2c fix round 2 (item 4), test builds: runs, once, the hook a test set for a `stand`
    /// pause's request that finds its instance's admission guard taken.
    #[cfg(test)]
    fn pause_request_meets_taken_guard_for_test(&self) -> RuntimeHostResult<()> {
        let hook = lock(
            &self.pause_request_guard_taken_hook_for_test,
            HostLocation::ReleasePausedInstance,
        )?
        .take();
        if let Some(hook) = hook {
            hook();
        }
        Ok(())
    }

    /// The admission's own check (the race since the evaluation): a held instance refuses the
    /// dispatch with `dispatch_instance_held`, recorded at Info after its intent. A takeover
    /// cooldown is left to the lease's own refusal.
    fn dispatch_held_error(
        &self,
        instance_alias: &str,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<Option<RuntimeHostError>> {
        let instance_id = self
            .resolve_instance(instance_alias)
            .map_err(|failure| *failure.error)?
            .instance_id();
        // Workflow #437 MAPP-1b (`contracts/device-group.md`, "One pick per device per round"):
        // another member of the device has it (its hold, its fence or its holder): the member's
        // wait, `device_group_waiting`, named by the alias the intent names, at info.
        if let Some(hold) = self.admission_device_group_hold(instance_id, now_unix_ms)? {
            return Ok(Some(
                RuntimeHostError::request(
                    actingcommand_policy::PolicyCode::DeviceGroupWaiting.code(),
                    HostLocation::AdmitPolicyDispatch,
                    RuntimeErrorCode::InvalidRequest,
                )
                .with_outcome_values(|outcome| hold.with_values(outcome, instance_alias)),
            ));
        }
        let Some(InstanceOccupancy::Held { holder_kind, count }) =
            self.instance_occupancy(instance_id, now_unix_ms)?
        else {
            return Ok(None);
        };
        Ok(Some(
            RuntimeHostError::request(
                HostCode::DispatchInstanceHeld,
                HostLocation::AdmitPolicyDispatch,
                RuntimeErrorCode::InvalidRequest,
            )
            // Ledger revision 3: the instance, the holder and the waiting count. Workflow #378
            // A4 (part B L3): the resolved instance's opaque identifier, not the policy's alias.
            .with_outcome_values(|outcome| {
                outcome
                    .instance_id(crate::failure_identity::identifier_text(&instance_id))
                    .holder_kind(crate::error::ClaimKindToken(holder_kind))
                    .count(count)
            }),
        ))
    }
}

fn validate_static_fact_pool_authority(
    catalog: &actingcommand_policy::CompiledCatalog,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
    operation: impl Into<Location>,
) -> RuntimeHostResult<()> {
    use actingcommand_policy::ObservationRef;
    let operation: Location = operation.into();
    for pool in &catalog.catalog().pools.pools {
        if pool.value_source.is_static() {
            continue;
        }
        // #455 P2 (register §1.505, ruling 10): a pool read from rows of a list is bound to its
        // list, whose reading the host computes; a configured reading of it, or a configured fact
        // under its list's key, is a second authority, as for a pool read from its own fact.
        let fact_key = match &pool.observation {
            ObservationRef::Fact { fact_key } | ObservationRef::ListRows { fact_key, .. } => {
                fact_key
            }
            ObservationRef::Outcome { .. } => {
                return Err(policy_admission_request(
                    HostCode::PolicyPoolBindingInvalid,
                    operation,
                ));
            }
        };
        if resources.pools.iter().any(|value| value.pool_id == pool.id)
            || facts
                .facts
                .iter()
                .any(|fact| fact.scope == pool.scope && fact.fact_key == *fact_key)
        {
            return Err(policy_admission_request(
                HostCode::PolicyPoolAuthorityConflict,
                operation,
            ));
        }
    }
    Ok(())
}

#[derive(Clone)]
struct TrustedPolicyDispatch {
    intent: DispatchIntent,
    reason_chain: DecisionReasonChain,
    observed_monotonic_ms: u64,
    /// Workflow #369 W-1: the intent's instance view of the evaluation's inputs
    /// (`dispatch_view_identity`).
    view_identity: String,
    /// Workflow #455 RR3 (register §1.538, ruling 6): the identity of the needs computed from the
    /// stored records of the intent's instance for its evaluation, `None` without stored records,
    /// and that evaluation's time, at which the admission computes them again.
    needs_identity: Option<String>,
    evaluation_time: EvaluationTime,
    /// Workflow #405 D1 (register X14): the random delay the intent was dispatched under, which
    /// its `policy.dispatch_intent` record carries; `None` for an instance with the delay off.
    random_delay: Option<PolicyRandomDelay>,
}

/// Workflow #369 W-1 (model-369 v3.1 §3.3: "Other instances are evaluated and run in the same
/// cycle"): the identity of the part of a projection that one instance's intent reads. It hashes
/// that instance's snapshot, the facts and the catalog pools whose scope selects the instance,
/// the priority offsets for it or for every instance, its own task states and outcomes, and
/// every host. A pool the catalog does not declare counts as selecting every instance. The
/// cycle's earlier dispatches on other instances (their intents, runs, settlement records and
/// device facts) change only records outside it. Without the instance's snapshot no scope
/// resolves, so the whole snapshot identity is the view. Workflow #455 RR3 (register §1.538,
/// ruling 6): with `needs_identity`, the identity of the needs computed from the instance's stored
/// records, hashed after the rest; without stored records the bytes hashed are those of before.
fn dispatch_view_identity(
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
    pool_scopes: &BTreeMap<String, actingcommand_policy::ScopeSelector>,
    instance_id: &str,
    needs_identity: Option<&str>,
) -> RuntimeHostResult<String> {
    use actingcommand_policy::ScopeSelector;
    let Some(instance) = facts
        .instances
        .iter()
        .find(|instance| instance.instance_id == instance_id)
    else {
        return Ok(facts.fact_snapshot_id.clone());
    };
    let selects = |scope: &ScopeSelector| match scope {
        ScopeSelector::Instance { instance_id } => *instance_id == instance.instance_id,
        ScopeSelector::Server { server_id } => *server_id == instance.server_id,
        ScopeSelector::Game { game_id } => *game_id == instance.game_id,
    };
    let observed = facts
        .facts
        .iter()
        .filter(|fact| selects(&fact.scope))
        .collect::<Vec<_>>();
    let outcomes = facts
        .outcomes
        .iter()
        .filter(|outcome| outcome.instance_id == instance_id)
        .collect::<Vec<_>>();
    let tasks = facts
        .tasks
        .iter()
        .filter(|task| task.instance_id == instance_id)
        .collect::<Vec<_>>();
    let offsets = facts
        .priority_offsets
        .iter()
        .filter(|offset| {
            offset
                .instance_id
                .as_deref()
                .is_none_or(|offset_instance| offset_instance == instance_id)
        })
        .collect::<Vec<_>>();
    let mut pools = resources
        .pools
        .iter()
        .filter(|pool| pool_scopes.get(&pool.pool_id).is_none_or(selects))
        .collect::<Vec<_>>();
    pools.sort_by(|left, right| left.pool_id.cmp(&right.pool_id));
    let mut hosts = resources.hosts.iter().collect::<Vec<_>>();
    hosts.sort_by(|left, right| left.host_id.cmp(&right.host_id));
    let bytes = match needs_identity {
        None => serde_json::to_vec(&(instance, observed, outcomes, tasks, offsets, pools, hosts)),
        Some(needs) => serde_json::to_vec(&(
            instance, observed, outcomes, tasks, offsets, pools, hosts, needs,
        )),
    }
    .map_err(|_| {
        RuntimeHostError::fatal(
            HostCode::FactSnapshotEncodeFailed,
            HostLocation::HashPolicyFactSnapshot,
            RuntimeErrorCode::RuntimeHalted,
        )
    })?;
    Ok(format!(
        "snapshot:policy-dispatch-view:{:x}",
        Sha256::digest(bytes)
    ))
}

/// The scope of every pool the active catalog declares, by pool id (`dispatch_view_identity`).
fn catalog_pool_scopes(
    policy: &PolicyHost,
) -> BTreeMap<String, actingcommand_policy::ScopeSelector> {
    policy
        .active_loaded()
        .map(|catalog| {
            catalog
                .compiled()
                .catalog()
                .pools
                .pools
                .iter()
                .map(|pool| (pool.id.clone(), pool.scope.clone()))
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Default)]
pub(super) struct TrustedPolicyDispatchStore {
    entries: BTreeMap<String, TrustedPolicyDispatch>,
    order: VecDeque<String>,
}

/// What an intent's instance view is computed from (`dispatch_view_identity`): the evaluation's
/// facts and resources, the scopes of the active catalog's pools, and (Workflow #455 RR3) the needs
/// computed from the instances' stored records for the evaluation at `time`.
struct DispatchView<'a> {
    facts: &'a EvaluationFacts,
    resources: &'a EvaluationResources,
    pool_scopes: &'a BTreeMap<String, actingcommand_policy::ScopeSelector>,
    record_needs: &'a actingcommand_policy::RecordNeeds,
    time: EvaluationTime,
}

impl TrustedPolicyDispatchStore {
    /// Records the cycle's pending intents with the instance view of each in the evaluation's
    /// inputs `view` (`dispatch_view_identity`) and (Workflow #405 D1) the random delay each was
    /// dispatched under (`random_delays`, by decision id). Workflow #455 RR3: with the identity of
    /// the needs computed for its instance and the evaluation's time.
    fn record_cycle(
        &mut self,
        cycle: &PolicyCycle,
        observed_monotonic_ms: u64,
        view: &DispatchView<'_>,
        random_delays: &BTreeMap<String, PolicyRandomDelay>,
    ) -> RuntimeHostResult<()> {
        let Some(evaluation) = &cycle.evaluation else {
            return Ok(());
        };
        for intent in &cycle.pending_dispatch_intents {
            let reason_chain = evaluation
                .reason_chains
                .iter()
                .find(|reason| reason.id == intent.reason_chain_id)
                .ok_or_else(|| {
                    policy_admission_fatal(
                        SharedCode::PolicyReasonChainMissing,
                        HostLocation::RecordTrustedPolicyDispatch,
                    )
                })?;
            let needs_identity = view
                .record_needs
                .get(&intent.instance_id)
                .map(|needs| needs.identity().to_owned());
            let trusted = TrustedPolicyDispatch {
                intent: intent.clone(),
                reason_chain: reason_chain.clone(),
                observed_monotonic_ms,
                view_identity: dispatch_view_identity(
                    view.facts,
                    view.resources,
                    view.pool_scopes,
                    &intent.instance_id,
                    needs_identity.as_deref(),
                )?,
                needs_identity,
                evaluation_time: view.time,
                random_delay: random_delays.get(&intent.decision_id).copied(),
            };
            if let Some(existing) = self.entries.get(&intent.decision_id) {
                if existing.intent != trusted.intent
                    || existing.reason_chain != trusted.reason_chain
                {
                    return Err(policy_admission_fatal(
                        HostCode::PolicyDecisionIdentityConflict,
                        HostLocation::RecordTrustedPolicyDispatch,
                    ));
                }
                continue;
            }
            self.order.push_back(intent.decision_id.clone());
            self.entries.insert(intent.decision_id.clone(), trusted);
        }
        while self.order.len() > MAX_TRUSTED_POLICY_DISPATCHES {
            if let Some(expired) = self.order.pop_front() {
                self.entries.remove(&expired);
            }
        }
        Ok(())
    }

    fn authorize(
        &self,
        intent: &DispatchIntent,
        reason_chain: &DecisionReasonChain,
    ) -> RuntimeHostResult<TrustedPolicyDispatch> {
        let trusted = self.entries.get(&intent.decision_id).ok_or_else(|| {
            policy_admission_request(
                HostCode::PolicyDecisionNotHostEvaluated,
                HostLocation::AuthorizePolicyDispatch,
            )
        })?;
        if trusted.intent != *intent || trusted.reason_chain != *reason_chain {
            return Err(policy_admission_request(
                HostCode::PolicyTrustedContextMismatch,
                HostLocation::AuthorizePolicyDispatch,
            ));
        }
        Ok(trusted.clone())
    }
}

/// Workflow #191 ps1: the operator's scheduling pauses. They have no expiry. The global pause
/// and every instance pause hold their own revision; setting or lifting a gate bumps the
/// revision of its scope. Since Workflow #361 B1 the held pauses are also the runtime fact
/// `host.scheduling_pause` (`scheduling_pause.rs`), recorded before the gate changes, and a
/// restart restores them.
#[derive(Clone, Default)]
pub(super) struct SchedulingPauseTable {
    global_revision: u64,
    global: Option<SchedulingPauseState>,
    instance_revisions: BTreeMap<String, u64>,
    instances: BTreeMap<String, InstancePauseState>,
    /// Workflow #191 ps2: per instance alias, the connections whose contained run ended
    /// cancelled and whose client has not reset the instance yet (`SafeReset`). An instance
    /// pause hands the device back only once none is left.
    client_resets: BTreeMap<String, BTreeSet<ConnectionId>>,
    /// Workflow #361 B1: where each held pause came from (`None` is the global pause).
    origins: BTreeMap<Option<String>, PauseOrigin>,
    /// The instance aliases a player holds: the player hold's dimension of the control gates.
    /// Empty until R-7b brings the hold (Workflow #429); #402 HS-2's tests set it
    /// (`hold_player_for_test`) to show that a backend switch reads the dimension.
    player_held: BTreeSet<String>,
    /// #480 PG-2c: the mode each instance pause its own request set was asked with, in memory
    /// only. A restored pause and a pause a start holds have none: they are `released`, where the
    /// mode changes nothing (`mode_of` answers `stand` for them). #480 PG-2e: the status answers
    /// it (`answered_instance_state`).
    modes: BTreeMap<String, SchedulingPauseMode>,
    /// #480 PG-2c fix round 1 (`contracts/scheduling-pause.md`, "Resume"): per instance alias,
    /// where the hand-back's session close of its `stand` pause and a person's resume of it met,
    /// each side's step taken under this table's lock. Ends with the pause (`lift_instance`).
    hand_backs: BTreeMap<String, HandBack>,
    /// #480 PG-2c fix round 1: per instance alias, what the last check of its `stand` pause's
    /// stages found in its way, in memory only, and the failure codes already said in the daemon's
    /// log for this pause. Ends with the pause. #480 PG-2e: the status answers what it found
    /// before stage `released` (`answered_instance_state`); stage (b)'s drain notes what it reads
    /// too (`HostShared::note_pause_drain`).
    stage_checks: BTreeMap<String, StageCheck>,
    /// #480 PG-2d (`contracts/scheduling-pause.md`, "A pause's life in the ledger"): per instance
    /// alias, the links of the request that set its pause, in memory only, so that a record of the
    /// pause's life written by another request (a person's resume) carries them. A restored pause
    /// and a pause a start holds have none. Ends with the pause (`lift_instance`).
    pause_links: BTreeMap<String, EventLinksDraft>,
}

/// #480 PG-2c fix round 1 (`contracts/scheduling-pause.md`, "Resume"): where the hand-back's close
/// of a `stand` pause and a person's resume of it met, each decided under the pause table's lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HandBack {
    /// The hand-back asked whether its pause was still held and began its close in one step: a
    /// resume from then on reconnects the instance (the path a `released` pause's resume takes).
    Closing,
    /// A resume found no close begun and lifts the pause without a reconnect: the hand-back closes
    /// nothing; once the resume lifted the pause it ends, and a resume that failed ends the mark.
    Lifting,
}

/// #480 PG-2c fix round 1: what a hand-back that is about to close its instance's session learns
/// under the pause table's lock ([`SchedulingPauseTable::begin_hand_back_close`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseStep {
    /// Its pause is held at its revision and no resume is lifting it: the close has begun.
    Begun,
    /// A resume lifted its pause (or a later pause holds the instance): it closes nothing.
    Lifted,
    /// A resume is lifting its pause: it closes nothing now and looks again at its next check.
    ResumeLifting,
}

/// #480 PG-2c fix round 1: a `stand` pause's last check, in memory only (#480 PG-2d: a
/// `quiet_or_lift` pause's too, for the record of its lift).
#[derive(Debug, Clone, Default)]
struct StageCheck {
    /// What was in the way at that check, each word once in the vocabulary's order.
    in_the_way: Vec<PauseObstacle>,
    /// The codes of the failures of its checks already said in the daemon's log: one line per
    /// pause and code, not one per try.
    said: BTreeSet<&'static str>,
    /// #480 PG-2d: a tool's hold (`quiet_or_lift`) whose stages failed because something was
    /// still in the way at the end of its caller's time (`in_the_way` says what), so that its lift
    /// writes `scheduling_pause_lifted_not_quiet`; set where that failure arises, never read off its
    /// code. Fix round 1: also a hold whose hand-back met a refusing queued entry or whose close
    /// lease was refused: every hold that fails without its instance having become quiet.
    not_quiet: bool,
}

impl SchedulingPauseTable {
    /// #402 HS-2, test builds: a player holds `instance_alias` (`held`), or no longer does.
    #[cfg(test)]
    pub(super) fn hold_player_for_test(&mut self, instance_alias: &str, held: bool) {
        if held {
            self.player_held.insert(instance_alias.to_owned());
        } else {
            self.player_held.remove(instance_alias);
        }
    }

    /// Workflow #361 B1: the rows of `host.scheduling_pause` for the held pauses, the global
    /// pause first, then the instance pauses by alias.
    pub(super) fn persisted_rows(&self) -> RuntimeHostResult<Vec<BTreeMap<String, FactScalar>>> {
        let origin = |scope: Option<String>| {
            self.origins.get(&scope).ok_or_else(|| {
                RuntimeHostError::fatal(
                    HostCode::SchedulingPauseOriginMissing,
                    HostLocation::PersistSchedulingPause,
                    RuntimeErrorCode::RuntimeHalted,
                )
            })
        };
        let mut rows = Vec::with_capacity(self.instances.len() + 1);
        if let Some(pause) = &self.global {
            rows.push(pause_row(
                None,
                &pause.pause_reason,
                pause.since_unix_ms,
                origin(None)?,
            ));
        }
        for (alias, pause) in &self.instances {
            rows.push(pause_row(
                Some(alias),
                &pause.pause_reason,
                pause.since_unix_ms,
                origin(Some(alias.clone()))?,
            ));
        }
        Ok(rows)
    }

    pub(super) const fn holds_global(&self) -> bool {
        self.global.is_some()
    }

    pub(super) fn holds_instance(&self, instance_alias: &str) -> bool {
        self.instances.contains_key(instance_alias)
    }

    pub(super) fn set_global(&mut self, state: SchedulingPauseState, origin: PauseOrigin) {
        self.global_revision = state.revision;
        self.global = Some(state);
        self.origins.insert(None, origin);
    }

    fn lift_global(&mut self) -> RuntimeHostResult<u64> {
        self.global = None;
        self.origins.remove(&None);
        next_scheduling_pause_revision(&mut self.global_revision)
    }

    pub(super) fn set_instance(
        &mut self,
        instance_alias: &str,
        state: InstancePauseState,
        origin: PauseOrigin,
    ) {
        self.instance_revisions
            .insert(instance_alias.to_owned(), state.revision);
        self.instances.insert(instance_alias.to_owned(), state);
        self.origins.insert(Some(instance_alias.to_owned()), origin);
    }

    /// Workflow #369 E3 (#670 safety net): holds an instance pause set at start, at the scope's
    /// next revision, with stage `released` (nothing runs on the instance yet and its device is
    /// left as it is), unless one is held already. Returns whether it set one.
    pub(super) fn hold_instance_at_start(
        &mut self,
        instance_alias: &str,
        pause_reason: Code,
        since_unix_ms: u64,
        origin: PauseOrigin,
    ) -> RuntimeHostResult<bool> {
        if self.instances.contains_key(instance_alias) {
            return Ok(false);
        }
        let pause_reason =
            SchedulingPauseReason::new(pause_reason.as_str()).map_err(|_| start_hold_invalid())?;
        self.hold_named_instance_at_start(instance_alias, pause_reason, since_unix_ms, origin)
    }

    /// Workflow #453 S2 (`contracts/scheduling-pause.md`, "Pauses held at a first start"):
    /// [`Self::hold_instance_at_start`] with a pause reason name rather than a code (the first
    /// start's pause of an entry that declares `start_paused` is `instance.added`). Returns
    /// whether it set one; a pause the instance holds already stays as it is. Fix 1: the release
    /// of a pause that left such an entry unapplied sets it too, right after the lift.
    pub(super) fn hold_named_instance_at_start(
        &mut self,
        instance_alias: &str,
        pause_reason: SchedulingPauseReason,
        since_unix_ms: u64,
        origin: PauseOrigin,
    ) -> RuntimeHostResult<bool> {
        if self.instances.contains_key(instance_alias) {
            return Ok(false);
        }
        let revision = next_scheduling_pause_revision(
            self.instance_revisions
                .entry(instance_alias.to_owned())
                .or_default(),
        )?;
        let state = InstancePauseState {
            revision,
            pause_reason,
            since_unix_ms,
            stage: InstancePauseStage::Released,
            pause_mode: None,
            in_the_way: Vec::new(),
        };
        state.validate().map_err(|_| start_hold_invalid())?;
        self.set_instance(instance_alias, state, origin);
        Ok(true)
    }

    /// Moves an instance pause of `revision` from stage `from` to `to`; any other state means
    /// the pause this request set was lost.
    fn advance_instance(
        &mut self,
        instance_alias: &str,
        revision: u64,
        from: InstancePauseStage,
        to: InstancePauseStage,
    ) -> RuntimeHostResult<()> {
        let state = self
            .instances
            .get_mut(instance_alias)
            .filter(|state| state.revision == revision && state.stage == from)
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    HostCode::SchedulingPauseStateLost,
                    ContractLocation::PauseScheduling,
                    RuntimeErrorCode::RuntimeHalted,
                )
            })?;
        state.stage = to;
        Ok(())
    }

    fn awaits_client_reset(&self, instance_alias: &str) -> bool {
        self.client_resets
            .get(instance_alias)
            .is_some_and(|connections| !connections.is_empty())
    }

    pub(super) fn install_cleanup_pending(&self) -> bool {
        self.client_resets
            .values()
            .any(|connections| !connections.is_empty())
    }

    /// Workflow #429 R-7a2 (R-7a §2.6 O-3, O-4): the control gates of `instance_alias`, in the
    /// one order: its pauses from this table (its own in any stage, the global one) and
    /// `resource_fence`, its #403 fence in any stage, read at the same moment by the caller. The
    /// player hold's dimension is false until R-7b (only #402 HS-2's tests set it,
    /// `player_held`). Its effective code is `control_gates::effective_code`'s.
    pub(super) fn control_gates(
        &self,
        instance_alias: &str,
        resource_fence: bool,
    ) -> actingcommand_contract::ControlGates {
        actingcommand_contract::ControlGates::evaluate(actingcommand_contract::ControlInputs {
            player_hold: self.player_holds(instance_alias),
            instance_pause: self.instances.contains_key(instance_alias),
            global_pause: self.global.is_some(),
            resource_fence,
        })
    }

    /// Whether a player holds `instance_alias`: the player hold's dimension of its control gates.
    /// Workflow #437 MAPP-1b: the other members of a device group of several read it too, and
    /// wait `device_group_waiting` `player_hold` while it holds the device.
    pub(super) fn player_holds(&self, instance_alias: &str) -> bool {
        self.player_held.contains(instance_alias)
    }

    /// Whether a pause holds `instance_alias`: its own, in any stage, or the global one. The
    /// recovery ladder's A6 reads only this (R-7a §2.6, row 4d: it tells the two apart nowhere).
    pub(super) fn pauses(&self, instance_alias: &str) -> bool {
        self.global.is_some() || self.instances.contains_key(instance_alias)
    }

    pub(super) fn global_state(&self) -> Option<SchedulingPauseState> {
        self.global.clone()
    }

    pub(super) fn instance_state(&self, instance_alias: &str) -> Option<InstancePauseState> {
        self.instances.get(instance_alias).cloned()
    }

    #[cfg(test)]
    pub(super) fn instance_states(&self) -> BTreeMap<String, InstancePauseState> {
        self.instances.clone()
    }

    /// WATCH-P (0.12.32), tests: the held pause of `instance_alias` in `stage`, its revision kept.
    /// `false` when the instance holds none.
    #[cfg(test)]
    pub(super) fn set_instance_stage_for_test(
        &mut self,
        instance_alias: &str,
        stage: InstancePauseStage,
    ) -> bool {
        let Some(state) = self.instances.get_mut(instance_alias) else {
            return false;
        };
        state.stage = stage;
        true
    }

    pub(super) fn lift_instance(&mut self, instance_alias: &str) -> RuntimeHostResult<u64> {
        self.instances.remove(instance_alias);
        self.origins.remove(&Some(instance_alias.to_owned()));
        self.modes.remove(instance_alias);
        self.hand_backs.remove(instance_alias);
        self.stage_checks.remove(instance_alias);
        self.pause_links.remove(instance_alias);
        next_scheduling_pause_revision(
            self.instance_revisions
                .entry(instance_alias.to_owned())
                .or_default(),
        )
    }

    /// #480 PG-2c: the mode of the instance pause of `instance_alias` its own request set; `stand`
    /// for every other pause (a restored one, one a start holds), as an absent member is.
    pub(super) fn mode_of(&self, instance_alias: &str) -> SchedulingPauseMode {
        self.modes.get(instance_alias).copied().unwrap_or_default()
    }

    /// #480 PG-2c: whether the instance pause of `instance_alias` at `revision` is still held, in
    /// any stage (a resume lifts it, and a later pause has another revision).
    pub(super) fn holds_instance_at(&self, instance_alias: &str, revision: u64) -> bool {
        self.instances
            .get(instance_alias)
            .is_some_and(|state| state.revision == revision)
    }

    /// #480 PG-2c fix round 1 (b): the hand-back of the `stand` pause of `instance_alias` at
    /// `revision`, its instance's admission guard in hand, just before it closes the session:
    /// whether its pause is still held at its revision and, in the same step, the mark that its
    /// close has begun ([`CloseStep`]).
    fn begin_hand_back_close(&mut self, instance_alias: &str, revision: u64) -> CloseStep {
        if !self.holds_instance_at(instance_alias, revision) {
            return CloseStep::Lifted;
        }
        if self.hand_backs.get(instance_alias) == Some(&HandBack::Lifting) {
            return CloseStep::ResumeLifting;
        }
        self.hand_backs
            .insert(instance_alias.to_owned(), HandBack::Closing);
        CloseStep::Begun
    }

    /// #480 PG-2c fix round 1: the close the hand-back of the pause at `revision` began closed
    /// nothing (its lease was refused): a resume from now on lifts without a reconnect again.
    fn end_hand_back_close(&mut self, instance_alias: &str, revision: u64) {
        if self.holds_instance_at(instance_alias, revision)
            && self.hand_backs.get(instance_alias) == Some(&HandBack::Closing)
        {
            self.hand_backs.remove(instance_alias);
        }
    }

    /// #480 PG-2c fix round 1 (b): a person's resume of the `stand` pause of `instance_alias`
    /// still `draining` or `paused`, under the persist gate: `false` when the hand-back's close has
    /// begun or the stage reached `released` meanwhile (the resume takes the reconnect path);
    /// otherwise the mark that it lifts the pause without a reconnect, and `true`.
    fn begin_resume_lift(&mut self, instance_alias: &str) -> bool {
        let quiet = self
            .instances
            .get(instance_alias)
            .is_none_or(|state| state.stage == InstancePauseStage::Released);
        if quiet || self.hand_backs.get(instance_alias) == Some(&HandBack::Closing) {
            return false;
        }
        self.hand_backs
            .insert(instance_alias.to_owned(), HandBack::Lifting);
        true
    }

    /// #480 PG-2c fix round 1: a resume that marked its lift failed before the lift: the mark
    /// ends, and the hand-back goes on.
    fn end_resume_lift(&mut self, instance_alias: &str) {
        if self.hand_backs.get(instance_alias) == Some(&HandBack::Lifting) {
            self.hand_backs.remove(instance_alias);
        }
    }

    /// #480 PG-2c fix round 1: what the last check of the `stand` pause of `instance_alias` at
    /// `revision` found in its way, kept in memory; nothing for a pause no longer held there.
    /// #480 PG-2d: a `quiet_or_lift` pause's too, which the record of its lift reads.
    fn note_in_the_way(&mut self, instance_alias: &str, revision: u64, words: Vec<PauseObstacle>) {
        if self.holds_instance_at(instance_alias, revision) {
            self.stage_checks
                .entry(instance_alias.to_owned())
                .or_default()
                .in_the_way = words;
        }
    }

    /// #480 PG-2c fix round 1: what the last check of the pause of `instance_alias` found in its
    /// way (empty when no check noted anything).
    pub(super) fn noted_in_the_way(&self, instance_alias: &str) -> Vec<PauseObstacle> {
        self.stage_checks
            .get(instance_alias)
            .map(|check| check.in_the_way.clone())
            .unwrap_or_default()
    }

    /// #480 PG-2e (`contracts/scheduling-pause.md`, "Status"; register §1.485): the pause of
    /// `instance_alias` as the status answers it, read from this table alone (the status reads a
    /// copy taken under the table's lock and runs no check of its own): its mode, the one its own
    /// request set, else `stand`, the mode the Runtime treats a pause no request set as
    /// ([`Self::mode_of`]: a restored pause, one a start holds, one held again at a release); and,
    /// before stage `released`, what the pause's own latest check noted in its way
    /// ([`Self::noted_in_the_way`]), absent when it noted nothing or no check ran yet. `None` for
    /// an instance its own pause does not hold.
    pub(super) fn answered_instance_state(
        &self,
        instance_alias: &str,
    ) -> Option<InstancePauseState> {
        let mut state = self.instances.get(instance_alias)?.clone();
        state.pause_mode = Some(self.mode_of(instance_alias));
        state.in_the_way = if state.stage == InstancePauseStage::Released {
            Vec::new()
        } else {
            self.noted_in_the_way(instance_alias)
        };
        Some(state)
    }

    /// #480 PG-2d: the `quiet_or_lift` pause of `instance_alias` at `revision` was not quiet by the
    /// end of its caller's time (fix round 1: or fails otherwise without its instance having become
    /// quiet, a refusing queued entry or a refused close lease): what was in the way then
    /// (`in_the_way`; `None`: what its last check noted), kept for its lift
    /// ([`Self::not_quiet_in_the_way`]); nothing for a pause no longer held there.
    fn mark_not_quiet(
        &mut self,
        instance_alias: &str,
        revision: u64,
        in_the_way: Option<Vec<PauseObstacle>>,
    ) {
        if !self.holds_instance_at(instance_alias, revision) {
            return;
        }
        let check = self
            .stage_checks
            .entry(instance_alias.to_owned())
            .or_default();
        if let Some(words) = in_the_way {
            check.in_the_way = words;
        }
        check.not_quiet = true;
    }

    /// #480 PG-2d: what was in the way of the pause of `instance_alias` when it failed before it was
    /// quiet ([`Self::mark_not_quiet`]); `None` when it failed otherwise.
    fn not_quiet_in_the_way(&self, instance_alias: &str) -> Option<Vec<PauseObstacle>> {
        self.stage_checks
            .get(instance_alias)
            .filter(|check| check.not_quiet)
            .map(|check| check.in_the_way.clone())
    }

    /// #480 PG-2c fix round 1: whether the failure spelled `spelling` of a check of the pause of
    /// `instance_alias` at `revision` is said now: the first time for this pause, never again.
    fn first_saying(
        &mut self,
        instance_alias: &str,
        revision: u64,
        spelling: &'static str,
    ) -> bool {
        self.holds_instance_at(instance_alias, revision)
            && self
                .stage_checks
                .entry(instance_alias.to_owned())
                .or_default()
                .said
                .insert(spelling)
    }
}

fn next_scheduling_pause_revision(revision: &mut u64) -> RuntimeHostResult<u64> {
    *revision = revision.checked_add(1).ok_or_else(|| {
        RuntimeHostError::fatal(
            HostCode::SchedulingPauseRevisionOverflow,
            HostLocation::ChangeSchedulingPause,
            RuntimeErrorCode::RuntimeHalted,
        )
    })?;
    Ok(*revision)
}

/// What an instance resume did to the instance's pause: lifted it (the scope's new revision), or,
/// Workflow #453 S2 fix 1, replaced it by the `instance.added` pause of an entry whose
/// `start_paused` was not applied yet (that pause's revision).
enum InstanceResumeLift {
    Lifted(u64),
    HeldAgain(u64),
}

/// A pause a start would hold that is no valid pause state (Workflow #369 E3).
fn start_hold_invalid() -> RuntimeHostError {
    RuntimeHostError::fatal(
        HostCode::SchedulingPauseStateInvalid,
        HostLocation::HoldStartInstancePause,
        RuntimeErrorCode::RuntimeHalted,
    )
}

fn scheduling_pause_denied(
    code: impl Into<Code>,
    operation: impl Into<Location>,
) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::request(code, operation, RuntimeErrorCode::InvalidRequest),
        RuntimeReceiptState::Denied,
        None,
    )
}

/// Workflow #338 R4: a conditional resume lifts only the pause the caller saw. Called with the
/// pause table locked; `revision` is the scope's current revision (zero for an instance scope
/// that was never paused).
fn check_scheduling_pause_expectation(
    expected: Option<&SchedulingPauseExpectation>,
    owner_epoch: OwnerEpoch,
    revision: u64,
) -> Result<(), RequestFailure> {
    let Some(expected) = expected else {
        return Ok(());
    };
    if expected.owner_epoch != owner_epoch {
        return Err(scheduling_pause_denied(
            HostCode::SchedulingPauseOwnerEpochMismatch,
            ContractLocation::ResumeScheduling,
        ));
    }
    if expected.revision != revision {
        return Err(scheduling_pause_denied(
            HostCode::SchedulingPauseRevisionMismatch,
            ContractLocation::ResumeScheduling,
        ));
    }
    Ok(())
}

/// The refusals of an instance resume, read on `table`, the pause table its caller has locked:
/// the expectation (Workflow #338 R4) holds and the instance's pause is `Released`, or (Workflow
/// #403) the instance is `fenced` and not paused; otherwise `scheduling_not_paused`,
/// `scheduling_pause_draining` or `scheduling_pause_releasing`. #480 PG-2c: a person's pause
/// (`stand`) is resumable in every stage; a tool's hold (`quiet_or_lift`) keeps those refusals.
fn check_resumable(
    table: &SchedulingPauseTable,
    instance_alias: &str,
    expected: Option<&SchedulingPauseExpectation>,
    fenced: bool,
    owner_epoch: OwnerEpoch,
) -> Result<(), RequestFailure> {
    check_scheduling_pause_expectation(
        expected,
        owner_epoch,
        table
            .instance_revisions
            .get(instance_alias)
            .copied()
            .unwrap_or(0),
    )?;
    let stand = table.mode_of(instance_alias) == SchedulingPauseMode::Stand;
    match table.instances.get(instance_alias).map(|state| state.stage) {
        None if fenced => Ok(()),
        None => Err(scheduling_pause_denied(
            HostCode::SchedulingNotPaused,
            ContractLocation::ResumeScheduling,
        )),
        Some(InstancePauseStage::Draining) if !stand => Err(scheduling_pause_denied(
            HostCode::SchedulingPauseDraining,
            ContractLocation::ResumeScheduling,
        )),
        Some(InstancePauseStage::Paused) if !stand => Err(scheduling_pause_denied(
            HostCode::SchedulingPauseReleasing,
            ContractLocation::ResumeScheduling,
        )),
        Some(_) => Ok(()),
    }
}

/// #480 PG-2c: the instance pause whose stages run: its instance, the revision its request set
/// and the mode it was asked with.
#[derive(Clone, Copy)]
struct PausedInstance<'a> {
    alias: &'a str,
    instance_id: InstanceId,
    revision: u64,
    mode: SchedulingPauseMode,
}

/// #480 PG-2c (`contracts/scheduling-pause.md`, "Two modes"): until when the stages of an
/// instance pause run on.
#[derive(Clone, Copy)]
enum StagesBound {
    /// Its request's time: runs still in flight at `drain` are asked to stop at their next
    /// checkpoint, and by `grace` the request answers (a `quiet_or_lift` pause fails there and is
    /// lifted; a `stand` pause answers `scheduling_pause_standing`).
    Caller(SchedulingPauseDeadlines),
    /// A standing pause's stages after its request answered: runs still in flight are asked to
    /// stop at once and are waited for; only a quiet instance, a person's resume or the shutdown
    /// end the stages.
    Unbounded,
}

impl StagesBound {
    /// When a run still in flight is asked to stop at its next checkpoint.
    fn stop_runs_at(self) -> Instant {
        match self {
            Self::Caller(deadlines) => deadlines.drain,
            Self::Unbounded => Instant::now(),
        }
    }

    /// The end of the caller's time, if any.
    const fn grace(self) -> Option<Instant> {
        match self {
            Self::Caller(deadlines) => Some(deadlines.grace),
            Self::Unbounded => None,
        }
    }
}

/// #480 PG-2c: how the stages of an instance pause ended, short of a failure (which lifts a
/// `quiet_or_lift` pause and never a `stand` one). Every end but `Released` is a `stand` pause's.
enum StagesEnd {
    /// Nothing uses the instance's device any more: the stage is `released`.
    Released(SchedulingDrainSummary),
    /// A shutdown came before `released`: the pause is kept in the stage it had reached, its fact
    /// unchanged, and the next start restores it (`scheduling_pause_kept_at_shutdown`).
    KeptAtShutdown {
        stage: InstancePauseStage,
        drained: SchedulingDrainSummary,
    },
    /// The caller's time ended first with something still in the way
    /// (`scheduling_pause_standing`): the host carries the rest of the stages on, unless only an
    /// unconfirmed close stands in the way (`carry_on` false: it is not tried again). Fix round 1:
    /// `failure`, when the last check failed (`check_failed`), is that failure's outcome, the
    /// cause of `scheduling_pause_standing`.
    Standing {
        stage: InstancePauseStage,
        in_the_way: Vec<PauseObstacle>,
        carry_on: bool,
        failure: Option<Outcome>,
        drained: SchedulingDrainSummary,
    },
    /// A person's resume lifted the pause while its stages ran
    /// (`scheduling_pause_lifted_by_resume`): nothing more is done.
    Resumed {
        stage: InstancePauseStage,
        drained: SchedulingDrainSummary,
    },
}

/// #480 PG-2c: how an instance pause's hand-back (`release_paused_instance`) ended, short of a
/// failure.
enum ReleaseEnd {
    /// Nothing uses the instance's device any more: the stage goes on to `released`.
    Released,
    /// A `stand` pause met a shutdown before it was released.
    KeptAtShutdown,
    /// A `stand` pause's caller's time ended with something still in the way (fix round 1: with
    /// the outcome of its last check's failure, when that check failed).
    Standing {
        in_the_way: Vec<PauseObstacle>,
        carry_on: bool,
        failure: Option<Outcome>,
    },
    /// A person's resume lifted a `stand` pause meanwhile.
    Lifted,
}

/// #480 PG-2c fix round 1: what one check of a pause's hand-back (`hand_back_check`) found.
enum HandBackCheck {
    /// The hand-back ended.
    End(ReleaseEnd),
    /// The check looks again at once (a run was drained again, or a shutdown came while the
    /// request waited for the admission guard).
    Again,
    /// Something is still in the way, or a resume is lifting the pause: the next check comes after
    /// one poll.
    Wait,
    /// The session close's lease was refused: nothing was closed, and the close is tried again
    /// after `SCHEDULING_PAUSE_RETRY_INTERVAL`.
    CloseRefused,
}

/// #480 PG-2c: the word of what a lease of `kind` that holds the paused instance is: the
/// Runtime's own automatic emulator start (`Autostart`, whatever asked for it), its stuck-recovery
/// ladder (`Ladder`, and `LadderContinuation`, the kind its continuation is queued with, as
/// `SeedScheduler::ladder_active` reads the two), or another party's lease. Before 0.12.73 (#480
/// PG-1's R4′ and PG-2b) the Runtime's own two were waited for past the grace by every pause;
/// since PG-2c a `stand` pause waits for everything past it and a `quiet_or_lift` one for nothing.
const fn lease_obstacle(kind: ClaimKind) -> PauseObstacle {
    match kind {
        ClaimKind::Autostart => PauseObstacle::AutomaticStart,
        ClaimKind::Ladder | ClaimKind::LadderContinuation => PauseObstacle::RecoveryLadder,
        _ => PauseObstacle::Lease,
    }
}

/// #480 PG-1 (fix rounds 1 and 2): what is in the way of an instance pause's hand-back
/// (`release_paused_instance`), as one reading finds it.
#[derive(Debug, Clone, Copy)]
struct ReleaseObstacles {
    /// A lease whose token names the instance.
    lease_held: bool,
    /// #480 PG-2c: the kind of that lease's claim (an automatic emulator start that asked its last
    /// question holds its claim's lease until its drive ends: `contracts/scheduling-pause.md`, "Two
    /// modes", fix round 2).
    lease_kind: Option<ClaimKind>,
    /// A queued entry that refuses the hand-back (`TransferNotSafe`); the instance's own queued
    /// autostart claim is none (`AutostartEndsAtGrant`).
    queue_waiting: bool,
    /// A contained run on the instance, drained again.
    run_in_flight: bool,
    /// The reset a client owes after its cancelled run (`SafeReset`).
    reset_awaited: bool,
    /// Workflow #427-A: a policy run the instance's worker still holds.
    lane_working: bool,
}

impl ReleaseObstacles {
    /// Nothing uses the instance's device any more (a refusing queued entry is answered first).
    const fn none_left(self) -> bool {
        !self.run_in_flight && !self.reset_awaited && !self.lease_held && !self.lane_working
    }

    /// #480 PG-2c: what is in the way, each word once in the vocabulary's order (with
    /// `close_refused` when the session close's lease was just refused).
    fn in_the_way(self, close_refused: bool) -> Vec<PauseObstacle> {
        let mut words = BTreeSet::new();
        if self.run_in_flight {
            words.insert(PauseObstacle::ContainedRun);
        }
        if self.lane_working {
            words.insert(PauseObstacle::PolicyRun);
        }
        if let Some(kind) = self.lease_kind {
            words.insert(lease_obstacle(kind));
        }
        if self.queue_waiting {
            words.insert(PauseObstacle::QueuedEntry);
        }
        if self.reset_awaited {
            words.insert(PauseObstacle::ClientReset);
        }
        if close_refused {
            words.insert(PauseObstacle::CloseRefused);
        }
        words.into_iter().collect()
    }
}

/// #480 PG-2c: the cause a `stand` pause's completed receipt carries (`caused_by` its success
/// code), or none when it was released within its caller's time.
fn stages_cause(end: &StagesEnd) -> Option<Outcome> {
    match end {
        StagesEnd::Released(_) => None,
        StagesEnd::KeptAtShutdown { stage, .. } => {
            Some(Outcome::new(HostCode::SchedulingPauseKeptAtShutdown).pause_stage(*stage))
        }
        StagesEnd::Standing {
            stage,
            in_the_way,
            failure,
            ..
        } => {
            let standing = Outcome::new(HostCode::SchedulingPauseStanding)
                .pause_stage(*stage)
                .in_the_way(in_the_way.iter().copied());
            // Fix round 1: a failed last check (`check_failed`) has its failure as the cause.
            Some(match failure {
                Some(failure) => standing.caused_by(failure.clone()),
                None => standing,
            })
        }
        StagesEnd::Resumed { stage, .. } => {
            Some(Outcome::new(HostCode::SchedulingPauseLiftedByResume).pause_stage(*stage))
        }
    }
}

/// Workflow #191 ps2: a failed device hand-back fails the pause request (`Failed`); a fatal
/// error poisons it.
fn scheduling_pause_release_failure(error: RuntimeHostError) -> RequestFailure {
    if error.is_fatal() {
        RequestFailure::poison_without_terminal(error)
    } else {
        RequestFailure::request(error, RuntimeReceiptState::Failed, None)
    }
}

/// A tool's hold whose hand-back still met something in the way at its grace (Workflow #191 ps2;
/// #480 PG-2c fix round 1: the instance's admission guard still taken then too).
fn scheduling_pause_release_busy() -> RequestFailure {
    scheduling_pause_release_failure(RuntimeHostError::request(
        HostCode::SchedulingPauseReleaseBusy,
        HostLocation::ReleasePausedInstance,
        RuntimeErrorCode::LeaseBusy,
    ))
}

/// #480 PG-2c fix round 1 (the coordinator's ruling 4; #419: one instance's trouble does not take
/// the Runtime down): whether a failure of a check of a `stand` pause's stages is tried again: one
/// the one decision point (`contain_failure`) would end only the operation with, neither latching
/// the Runtime nor fencing the instance. Decided by the failure's scope, never by its code; a
/// fatal failure, and one that fences the instance, stay as they are.
fn tried_again(failure: &RequestFailure, instance_id: InstanceId) -> bool {
    !failure.poison_runtime
        && failure.error.scope_at(Some(instance_id)) == crate::FailureScope::Operation
}

/// Workflow #191 ps2: the self-check of an instance resume, projected from the open reports of
/// its own reconnect. Capture: the last Capture or Nemu pair report, `ok` when its open status
/// and `capture_check` passed, with its frame size and selected backend. Touch: the last Input or
/// Nemu pair report, `ok` when its open status and `input_check` passed, with its selected
/// backend and its connection's bounds (the input geometry, else the handshake limits). A side
/// without a report of this reconnect (the preparation lease was refused, the close of a
/// session the instance still held failed, or an open failed before reaching that side) is
/// `ok: false` with no values.
/// Since Workflow #317 sc3 it projects every preparation phase's opens.
pub(super) fn scheduling_resume_selfcheck(
    observations: &[actingcommand_device::BackendOpenObservation],
    failure_code: Option<&str>,
) -> SchedulingResumeSelfCheck {
    let report = |entry: BackendOpenEntry| {
        observations
            .iter()
            .rev()
            .map(|observation| &observation.report)
            .find(|report| report.entry == entry || report.entry == BackendOpenEntry::NemuPair)
    };
    let passed = BackendObservationStatus::Passed;
    let capture = report(BackendOpenEntry::Capture);
    let touch = report(BackendOpenEntry::Input);
    let positive = |x: i32, y: i32| (x > 0 && y > 0).then_some((x, y));
    let bounds = touch.and_then(|report| {
        report
            .input_geometry
            .as_ref()
            .and_then(|geometry| positive(geometry.natural_max_x, geometry.natural_max_y))
            .or_else(|| {
                report
                    .handshake
                    .as_ref()
                    .and_then(|handshake| positive(handshake.max_x, handshake.max_y))
            })
    });
    SchedulingResumeSelfCheck {
        capture: SchedulingResumeCaptureCheck {
            ok: capture
                .is_some_and(|report| report.status == passed && report.capture_check == passed),
            width: capture.and_then(|report| report.frame_width),
            height: capture.and_then(|report| report.frame_height),
            capture_backend: capture.and_then(|report| report.selected.clone()),
        },
        touch: SchedulingResumeTouchCheck {
            ok: touch.is_some_and(|report| report.status == passed && report.input_check == passed),
            backend: touch.and_then(|report| report.selected.clone()),
            max_x: bounds.map(|(max_x, _)| max_x),
            max_y: bounds.map(|(_, max_y)| max_y),
            invasive: false,
        },
        failure_code: failure_code.map(str::to_owned),
    }
}

struct PolicyAdmissionAppender<'a> {
    ledger: &'a GlobalLedger,
    initial_fact_gate: RefCell<Option<MutexGuard<'a, ()>>>,
    // Workflow #191 U5-F1: the events this admission appended (at most the intent and its
    // outcome), which the admission applies to the policy projection itself.
    appended: RefCell<Vec<PersistedEvent>>,
}

impl<'a> PolicyAdmissionAppender<'a> {
    fn new(ledger: &'a GlobalLedger, initial_fact_gate: MutexGuard<'a, ()>) -> Self {
        Self {
            ledger,
            initial_fact_gate: RefCell::new(Some(initial_fact_gate)),
            appended: RefCell::new(Vec::new()),
        }
    }

    /// The appended events; consuming the appender also releases the fact write gate it still
    /// holds when the intent append failed.
    fn into_appended(self) -> Vec<PersistedEvent> {
        self.appended.into_inner()
    }
}

impl EventAppender for PolicyAdmissionAppender<'_> {
    fn append_durable(
        &self,
        draft: actingcommand_contract::SanitizedEventDraft,
    ) -> actingcommand_ledger::GlobalLedgerResult<PersistedEvent> {
        let event = self.ledger.append_durable(draft)?;
        self.initial_fact_gate.borrow_mut().take();
        self.appended.borrow_mut().push(event.clone());
        Ok(event)
    }
}

/// ISO-PE (#419): where a policy evaluation takes its time from.
#[derive(Clone, Copy)]
pub(super) enum PolicyCycleClock {
    /// The Runtime clock, sampled once the evaluation holds `fact_write_gate`, the gate under
    /// which it reads its inputs: every fact, outcome and settlement it reads was committed
    /// before the sample, so none was observed after the evaluation's time. (Until 0.12.28 the
    /// time was sampled before the evaluation waited for its gates, so an input a concurrent
    /// settlement observed and committed in between was "later than the evaluation time".)
    Sampled,
    /// A test's fixed time and monotonic reading.
    #[cfg(test)]
    Fixed {
        time: EvaluationTime,
        monotonic_ms: u64,
    },
}

/// ISO-PE (#419): the policy evaluations in a row that did not evaluate (skipped ones and, fix 1,
/// waits for an outcome projection not ready yet), and whether this run of them was escalated to
/// a person. An evaluation that evaluated ends the run.
#[derive(Debug, Default)]
pub(super) struct PolicyEvaluationSkips {
    consecutive: u32,
    escalated: bool,
}

impl PolicyEvaluationSkips {
    /// One more skip: the count in a row, and whether this one escalates (the first to reach
    /// `escalate_after` in this run of skips).
    fn record(&mut self, escalate_after: u32) -> (u32, bool) {
        self.consecutive = self.consecutive.saturating_add(1);
        let escalates = !self.escalated && self.consecutive >= escalate_after;
        self.escalated |= escalates;
        (self.consecutive, escalates)
    }

    /// ISO-PE fix 1 (review M1): one more evaluation whose outcome projection is not ready yet.
    /// While fewer than `escalate_after` evaluations in a row have not evaluated, it waits
    /// (`true`: actingd defers it by one debounce) and counts as one of them, with no record;
    /// once they reach `escalate_after` it does not wait (`false`) and is skipped, and
    /// [`Self::record`] counts it, so the skip escalates as every run of skips does.
    fn wait_for_projection(&mut self, escalate_after: u32) -> bool {
        let waits = self.consecutive < escalate_after;
        if waits {
            self.consecutive += 1;
        }
        waits
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// ISO-PE (#419): whether a failed policy evaluation is skipped rather than returned. Only a
/// failure whose scope is `Global` may stop the Runtime ([`RuntimeHostError::halts_runtime`]):
/// a fatal failure the Runtime cannot vouch for (the ledger, a poisoned lock, the clock, the
/// configuration in effect, the shared policy projection) and one on the closed Global list. Two
/// non-global failures keep a bounded handling of their own, so this rule does not skip them:
/// `policy_outcome_snapshot_changed` (re-sampled by actingd) and `outcome_projection_not_ready`
/// (deferred by actingd's debounce while it waits, and skipped once
/// `scheduling.evaluation.escalate_after` evaluations in a row have not evaluated:
/// `HostShared::wait_for_policy_projection`).
pub(crate) fn skips_policy_evaluation(error: &RuntimeHostError) -> bool {
    !error.halts_runtime()
        && error.code() != HostCode::PolicyOutcomeSnapshotChanged.as_str()
        && error.code() != SharedCode::OutcomeProjectionNotReady.as_str()
}

impl HostShared {
    /// One policy evaluation for actingd's driver. ISO-PE (#419): an evaluation whose failure is
    /// not global ([`skips_policy_evaluation`]) is skipped: its failure is recorded with its
    /// detail (`policy_evaluation_skipped`, warning), and the cycle is a deferred one with no
    /// evaluation, due `scheduling.evaluation.retry_ms` later, so the driver keeps its trigger and
    /// evaluates again then; the Runtime keeps running. `scheduling.evaluation.escalate_after`
    /// skips in a row record `policy_evaluation_awaiting_person` (error) once. An outcome
    /// projection not ready yet waits a bounded number of times before it is skipped too
    /// ([`Self::wait_for_policy_projection`]).
    pub(super) fn evaluate_policy_cycle(
        &self,
        trigger: PolicyTrigger,
    ) -> RuntimeHostResult<PolicyCycle> {
        let evaluated = self
            .evaluate_policy_cycle_authoritative(PolicyCycleClock::Sampled, None, trigger)
            .and_then(|mut cycle| {
                // IDLE-1: the idle check right after the evaluation and before actingd admits
                // its dispatches, so a wake's claim holds an idle instance before a due dispatch
                // reaches it.
                self.check_idle_instances(false)?;
                // #486 ASLEEP-5 (register §1.524, ruling 2): when the driver looks again for the
                // instances this check left asleep is the host's to say, from what the check did
                // with each; the evaluation names no wake for the task an asleep instance is woken
                // for.
                if cycle.evaluation.is_some() {
                    cycle.idle_wake_unix_ms = self.idle_look_again()?;
                }
                Ok(cycle)
            });
        match evaluated {
            Ok(cycle) => {
                if cycle.evaluation.is_some() {
                    lock(
                        &self.policy_evaluation_skips,
                        HostLocation::EvaluatePolicyCycle,
                    )?
                    .reset();
                }
                Ok(cycle)
            }
            Err(error) if skips_policy_evaluation(&error) => self.skip_policy_evaluation(&error),
            Err(error)
                if !error.halts_runtime()
                    && error.code() == SharedCode::OutcomeProjectionNotReady.as_str() =>
            {
                self.wait_for_policy_projection(error)
            }
            Err(error) => Err(error),
        }
    }

    /// ISO-PE fix 1 (review M1): an evaluation whose outcome projection is not ready yet. While
    /// fewer than `scheduling.evaluation.escalate_after` evaluations in a row have not evaluated,
    /// the failure is returned and actingd defers the trigger by one debounce; the wait counts as
    /// one of them. Once they reach it, the next one is skipped as any failure that is not
    /// global: `policy_evaluation_skipped` with its detail, evaluated again
    /// `scheduling.evaluation.retry_ms` later, and `policy_evaluation_awaiting_person` as for
    /// every run of skips. An evaluation that evaluates resets the count. (Until fix 1 it was
    /// returned every time: a projection that stayed not ready was evaluated again every debounce
    /// with no record, no daemon line, no bound and no escalation.)
    fn wait_for_policy_projection(
        &self,
        failure: RuntimeHostError,
    ) -> RuntimeHostResult<PolicyCycle> {
        let escalate_after = self.evaluation_retry_settings()?.escalate_after;
        let waits = lock(
            &self.policy_evaluation_skips,
            HostLocation::EvaluatePolicyCycle,
        )?
        .wait_for_projection(escalate_after);
        if waits {
            return Err(failure);
        }
        self.skip_policy_evaluation(&failure)
    }

    /// ISO-PE (#419): records the skipped evaluation's failure (`policy_evaluation_skipped`,
    /// and `policy_evaluation_awaiting_person` when this skip reaches
    /// `scheduling.evaluation.escalate_after` in a row) and returns the deferred cycle that
    /// evaluates again `scheduling.evaluation.retry_ms` later. A failure to record is returned.
    fn skip_policy_evaluation(&self, failure: &RuntimeHostError) -> RuntimeHostResult<PolicyCycle> {
        // A Runtime that latched a condition meanwhile is stopping whatever this failure is (its
        // monitor stops it): the failure is returned as before, not skipped.
        if self.fatal.current()?.is_some() {
            return Err(failure.clone());
        }
        let settings = self.evaluation_retry_settings()?;
        let now_unix_ms = self.runtime_clock_sample()?.unix_ms;
        let eligible_at_unix_ms = now_unix_ms.checked_add(settings.retry_ms).ok_or_else(|| {
            RuntimeHostError::fatal(
                HostCode::PolicyTimeOverflow,
                HostLocation::EvaluatePolicyCycle,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let (count, escalates) = lock(
            &self.policy_evaluation_skips,
            HostLocation::EvaluatePolicyCycle,
        )?
        .record(settings.escalate_after);
        // The failure as its record carries it: its code, its operation, the policy's code with
        // the refused input's values, and the policy's developer text as the foreign link.
        let cause = failure.record_outcome();
        let skipped = Outcome::new(HostCode::PolicyEvaluationSkipped)
            .operation(HostLocation::EvaluatePolicyCycle)
            .count(count)
            .limit_count(settings.escalate_after)
            .retry_after_ms(settings.retry_ms)
            .next_eligible_unix_ms(eligible_at_unix_ms)
            .caused_by(cause.clone());
        self.record_policy_evaluation_skip(EventSeverity::Warning, skipped)?;
        if escalates {
            let awaiting = Outcome::new(HostCode::PolicyEvaluationAwaitingPerson)
                .operation(HostLocation::EvaluatePolicyCycle)
                .count(count)
                .limit_count(settings.escalate_after)
                .retry_after_ms(settings.retry_ms)
                .caused_by(cause);
            self.record_policy_evaluation_skip(EventSeverity::Error, awaiting)?;
        }
        Ok(PolicyCycle {
            directive: PolicyRecomputeDirective {
                kind: PolicyRecomputeKind::Deferred,
                reason: PolicyRecomputeReason::Debounce,
                eligible_at_unix_ms,
            },
            evaluation: None,
            pending_dispatch_intents: Vec::new(),
            detection_planning_signals: Vec::new(),
            clock_slot_signals: Vec::new(),
            // Workflow #438 HELD-1: a skipped evaluation writes no held record and no end.
            task_held_signals: Vec::new(),
            measurement: None,
            fact_refresh_transitions: Vec::new(),
            idle_wake_unix_ms: None,
        })
    }

    /// ISO-PE (#419): one `runtime.failed` record of a skipped evaluation (its severity the
    /// outcome's category) with system links, handed to the daemon's log too.
    fn record_policy_evaluation_skip(
        &self,
        severity: EventSeverity,
        outcome: Outcome,
    ) -> RuntimeHostResult<()> {
        self.append_event_raw(
            severity,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            self.events.system_links()?,
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::NotPerformed,
                outcome.clone(),
                DiagnosticDetailDraft::new(
                    "policy_evaluation",
                    RuntimeLifecycleFailureStage::PolicyDriver.location(),
                    "runtime_host",
                    HostLocation::EvaluatePolicyCycle.as_str(),
                ),
                AuditInput::new(),
            ),
        )?;
        self.fatal.report_warning(&outcome);
        Ok(())
    }

    /// ISO-PE (#419), test builds: runs the hook a test set for the point after an evaluation
    /// started and before it takes the gates under which it reads its inputs. Fix 1 (review M1):
    /// then reports an outcome projection not ready yet (`outcome_projection_not_ready`, as the
    /// projection would) while the test's count of them lasts.
    #[cfg(test)]
    fn before_policy_inputs_read_for_test(&self) -> RuntimeHostResult<()> {
        let hook = lock(
            &self.policy_inputs_read_hook_for_test,
            HostLocation::EvaluatePolicyCycle,
        )?
        .take();
        if let Some(hook) = hook {
            hook();
        }
        let not_ready = self
            .policy_projection_not_ready_for_test
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_sub(1)
            })
            .is_ok();
        if not_ready {
            return Err(RuntimeHostError::request(
                SharedCode::OutcomeProjectionNotReady,
                HostLocation::ProjectPolicySchedulingOutcomes,
                RuntimeErrorCode::RuntimeUnavailable,
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn evaluate_policy_cycle_with_test_inputs(
        &self,
        facts: &EvaluationFacts,
        resources: &EvaluationResources,
        time: EvaluationTime,
        seed: u64,
        trigger: PolicyTrigger,
    ) -> RuntimeHostResult<PolicyCycle> {
        {
            let _gate = lock(
                &self.fact_write_gate,
                crate::codes::test_spellings::TestLocation::SetTestPolicyInputs,
            )?;
            *lock(
                &self.policy_inputs,
                crate::codes::test_spellings::TestLocation::SetTestPolicyInputs,
            )? = Some(PolicyInputSnapshot::new(facts.clone(), resources.clone()));
        }
        // The replaced configuration seeds the instance fact store exactly as a startup does.
        self.seed_policy_instance_facts()?;
        let clock = PolicyCycleClock::Fixed {
            time,
            monotonic_ms: self.monotonic_ms()?,
        };
        self.evaluate_policy_cycle_authoritative(clock, Some(seed), trigger)
    }

    pub(super) fn evaluate_policy_cycle_authoritative(
        &self,
        clock: PolicyCycleClock,
        seed: Option<u64>,
        trigger: PolicyTrigger,
    ) -> RuntimeHostResult<PolicyCycle> {
        // #486 ASLEEP-5 (register §1.524, ruling 4): the cycle's start, from which the time up to
        // its capped record is counted for the daemon's line of the cycle. #503 CYC-1 (register
        // §1.526, ruling 1): on the cycle's lap clock, as each stretch of it below.
        let cycle_started = crate::policy_host::lap_instant();
        if trigger == PolicyTrigger::Reconciliation {
            self.reconcile_pending_policy_settlements()?;
        }
        // IDLE-1P (#405): the instances the idle action closed as this cycle starts, read before
        // any other lock of the cycle is taken. #486 ASLEEP-1: with them, the instances whose
        // emulator it stops or holds stopped.
        let idle_closed = self.idle_closed_for_policy()?;
        let held_stopped = self.idle_emulators_held_stopped()?;
        // #486 ASLEEP-2: and the instances whose application it closes or holds closed.
        let held_closed = self.idle_applications_held_closed()?;
        // #486 ASLEEP-1 fix round 1: the instances whose idle action in effect can stop their
        // emulator (#486 ASLEEP-2, ruling 2: or close their application), whose idle plans take no
        // wake from a decision a gate without a time holds.
        let idle_sleepers = self.idle_sleepers_for_policy()?;
        // ROUT-L RL-1b (frozen model v1.0 §3.3): the scheduler workbench's check, at most once
        // per `standard_packs.check_interval_ms`, before this cycle reads its manifest and
        // catalog. Nothing in configured mode. ISO-PE: it reads the monotonic clock as the cycle
        // starts; the evaluation's own time is sampled under the fact gate below.
        let started_monotonic_ms = match clock {
            PolicyCycleClock::Sampled => self.monotonic_ms()?,
            #[cfg(test)]
            PolicyCycleClock::Fixed { monotonic_ms, .. } => monotonic_ms,
        };
        self.check_standard_pack_rules(started_monotonic_ms)?;
        // Workflow #455 G-R2s round 2: the games whose root the workbench could not compose,
        // read right after its check; their instances' resource targets are left out below.
        let uncomposed = self.uncomposed_standard_pack_games()?;
        // Workflow #463 V2 fix round 1: the variable table the workbench's last read of each
        // game root judged over the whole root, read from memory; none in configured mode.
        let standard_pack_variables = self.standard_pack_variables()?;
        // Workflow #455 RR3 (register §1.538, ruling 1): what the workbench keeps of each game root
        // that the needs computed from stored records read (its records entries and its accepted
        // tables), from memory, before the gates.
        let record_roots = self.goal_record_roots()?;
        #[cfg(test)]
        self.before_policy_inputs_read_for_test()?;
        // #503 CYC-1 (ruling 1): what precedes the gates ends here; from here to their release the
        // waits to take them are counted apart from the work done under them and the projection.
        let gates_reached = crate::policy_host::lap_instant();
        let mut gates_wait_micros = 0_u64;
        let _detection_gate = lock_waited(
            &self.detection_write_gate,
            HostLocation::PlanPolicyDetection,
            &mut gates_wait_micros,
        )?;
        let manifest = lock_waited(
            &self.procedure_manifest,
            HostLocation::ReadProcedureManifest,
            &mut gates_wait_micros,
        )?;
        let procedure_manifest = manifest.clone().ok_or_else(|| {
            policy_admission_request(
                HostCode::ProcedureManifestUnconfigured,
                HostLocation::EvaluatePolicyCycle,
            )
        })?;
        drop(manifest);
        let (
            time,
            observed_monotonic_ms,
            outcome_keys,
            facts,
            resources,
            missing_instance_facts,
            unknown_offset_tasks,
            left_out_lists,
            availability_withdrawals,
            pool_readings,
            record_needs,
            handed_runs,
            dispatches_read,
            queue_wait_starts,
            entry_blocked,
            projection_micros,
        ) = {
            let _outcome_gate = lock_waited(
                &self.policy_outcome_gate,
                HostLocation::SnapshotPolicyOutcomeState,
                &mut gates_wait_micros,
            )?;
            // Workflow #490 ENTRY-A: the entry-blocked instances, read under the gate a settlement
            // blocks its instance under, so no evaluation sees a run's settlement without its
            // block.
            let entry_blocked = self.entry_block_gates()?;
            let mut policy = lock_waited(
                self.policy()?,
                HostLocation::ReadPolicyOutcomeKeys,
                &mut gates_wait_micros,
            )?;
            // The previous evaluated cycle's eligibility verdicts take effect here, before
            // this cycle's inputs are projected (Workflow #308 slice 5b). Workflow #437 ORD-1c:
            // and its queue-wait verdicts, whose starts this evaluation passes.
            policy.apply_eligibility_verdicts();
            let queue_wait_starts = policy.apply_queue_wait_verdicts();
            let outcome_keys = policy.outcome_key_snapshot()?;
            let _gate = lock_waited(
                &self.fact_write_gate,
                HostLocation::ProjectPolicyFacts,
                &mut gates_wait_micros,
            )?;
            #[cfg(test)]
            crate::policy_host::delay_cycle_stretch_for_test(
                crate::policy_host::CycleStretch::GatesHeld,
            );
            // ISO-PE (#419): the evaluation's time, sampled under the gates its inputs are
            // written under (`policy_outcome_gate` for a settlement and its four facts,
            // `fact_write_gate` for every fact). Every input read below was committed before this
            // sample, so none was observed after the evaluation's time; sampled before the gates,
            // a settlement committed while the evaluation waited for them made the evaluation
            // reject its inputs (`policy_evaluation_input_invalid`, production 10-29 11:19).
            let (time, observed_monotonic_ms) = match clock {
                PolicyCycleClock::Sampled => {
                    let sample = self.runtime_clock_sample()?;
                    (
                        EvaluationTime {
                            unix_ms: sample.unix_ms,
                            monotonic_ms: sample.monotonic_ms,
                        },
                        sample.monotonic_ms,
                    )
                }
                #[cfg(test)]
                PolicyCycleClock::Fixed { time, monotonic_ms } => (time, monotonic_ms),
            };
            // #486 ASLEEP-5 (ruling 4): the projection of the cycle's inputs, the finished runs'
            // outcomes among them, timed for the daemon's line of the cycle.
            let projection_started = crate::policy_host::lap_instant();
            let (
                facts,
                resources,
                missing_instance_facts,
                unknown_offset_tasks,
                left_out_lists,
                availability_withdrawals,
                pool_readings,
            ) = self.project_authoritative_policy_inputs_with_gaps_under_gate(
                &mut policy,
                HostLocation::EvaluatePolicyCycle,
                &outcome_keys,
                None,
                Some(time.unix_ms),
                Some(time.unix_ms),
            )?;
            let projection_micros = crate::policy_host::micros_since(projection_started);
            // #477 LF-1 (register §1.509): the pairs whose latest dispatch at the inputs' position
            // carried a `run_cap`, with when that run ended; #455 RR3c (register §1.543) and RR3d
            // (register §1.548): and every dispatch that carried `gaps_remaining`, a run that failed
            // at its entry passed over. Read once, here: the needs below and the evaluation read the
            // one list, so a pool a run left unread is unread to both.
            let handed_runs = policy.handed_runs(facts.ledger_position);
            // Workflow #455 RR3 (register §1.538, rulings 1 and 6): the needs computed from each
            // instance's stored records, under the gate these inputs were projected under, with
            // the holdings through the evaluator's own projection of them.
            let record_needs = self.record_needs_under_gate(
                &policy,
                super::record_needs::NeedsInputs {
                    facts: &facts,
                    readings: &pool_readings,
                    handed: &handed_runs,
                },
                &record_roots,
                time,
                None,
            )?;
            // IDLE-3 (#447, register §1.332): the policy's own dispatches as these inputs count
            // them, read under the guard they were projected under (a settlement that comes
            // between this read and the evaluation below is not in them).
            let dispatches_read = policy.idle_dispatches_read(facts.ledger_position);
            (
                time,
                observed_monotonic_ms,
                outcome_keys,
                facts,
                resources,
                missing_instance_facts,
                unknown_offset_tasks,
                left_out_lists,
                availability_withdrawals,
                pool_readings,
                record_needs,
                handed_runs,
                dispatches_read,
                queue_wait_starts,
                entry_blocked,
                projection_micros,
            )
        };
        // #503 CYC-1 (ruling 1): the gates above are released; the cycle's other inputs follow,
        // up to the wait for the evaluation's policy lock.
        let inputs_started = crate::policy_host::lap_instant();
        // Workflow #428 S168-LB1a (`contracts/instance-fact-store.md`, "Left out of the
        // projection"): each record list the projection left out for its rows is recorded once
        // after a start, and its key raises no fact refresh demand on the instances it applies to.
        self.report_left_out_record_lists(&left_out_lists)?;
        let mut left_out = left_out_facts(&left_out_lists);
        // #455 P2 (register §1.505, rulings 6 and 8): the evaluation is given the readings of the
        // pools read from rows of a list beside the left-out facts, and each list's key is left
        // out for its pool's instance, so it raises no fact refresh demand.
        left_out.give_pool_readings(pool_readings);
        // Workflow #455 RR3 (register §1.538, ruling 6): and the needs computed from each
        // instance's stored records, one more kind beside them, for this evaluation alone.
        left_out.give_record_needs(record_needs);
        // Workflow #455 G-R2s round 2 (register §1.369; `contracts/resource-targets.md`, "A task
        // that can do something irreversible"): a game whose root cannot be composed has no goals
        // in effect until it composes; every instance of it reads as without a target record.
        for instance in &facts.instances {
            if uncomposed.contains(&instance.game_id) {
                left_out.void_targets(&instance.instance_id);
            }
        }
        // Workflow #429 R-2 (A.1 契约 1): the shared run limits in effect as this cycle starts,
        // read before the policy and controller locks; a run already admitted is not cut.
        let run_limits = self.shared_run_limits()?;
        let workloads = lock(self.policy()?, HostLocation::ReadPolicyPerformanceWorkloads)?
            .active_performance_workloads()?;
        let mut controlled_resources = resources.clone();
        lock(
            &self.performance_control,
            HostLocation::ApplyPolicyPerformanceControl,
        )?
        .apply_to_resources_with(
            &mut controlled_resources.hosts,
            &workloads,
            run_limits.heavy,
        )?;
        let seed = match seed {
            Some(seed) => seed,
            None => runtime_policy_seed(&facts.fact_snapshot_id, time, self.owner_epoch)?,
        };
        // Workflow #191 ps1: the scheduling pauses as this cycle starts; a paused candidate is
        // deferred exactly as admission would refuse it. Workflow #429 R-7a2 (R-7a §2.6 O-4):
        // with them, at the same step, the instances a #403 fence holds; the control gates of an
        // instance are judged from both, in the one order (position 1, its 1a to 1d).
        let scheduling_pause = lock(
            &self.scheduling_pause,
            HostLocation::ReadPolicySchedulingPause,
        )?
        .clone();
        let fenced_aliases = self.resource_fenced_aliases()?;
        let control_gates = |instance_alias: &str| {
            scheduling_pause.control_gates(instance_alias, fenced_aliases.contains(instance_alias))
        };
        // #486 ASLEEP-1 (register §1.497): the instances asleep as this cycle's inputs were
        // projected (their emulator stopped by their idle action, and nothing but that stop
        // withdrew their availability), which the evaluation judges as if they were available.
        // #486 ASLEEP-2 (register §1.506): with them, the instances whose application their idle
        // action closed or is closing, which the projection holds available.
        let asleep = self.asleep_for_policy(
            &held_stopped,
            &held_closed,
            &facts.instances,
            &availability_withdrawals,
            &fenced_aliases,
        )?;
        // Workflow #369 W-1: every instance's one queue as this cycle starts; a held instance's
        // candidates are deferred, and nothing is written for them.
        let occupancy = self.policy_instance_occupancy(time.unix_ms)?;
        // Workflow #437 MAPP-1: the device each member of a group of several runs on. Workflow
        // #437 ORD-1c: with the order's inputs when the map names a device of several.
        let (device_groups, device_queue) = self.policy_device_groups(&queue_wait_starts)?;
        // Workflow #437 MAPP-1b: each member of a group of several that another member keeps
        // from its device (its hold, its fence, its holder), with the pauses and fences above.
        let device_group_holds =
            self.policy_device_group_holds(&scheduling_pause, &fenced_aliases, time.unix_ms)?;
        // Workflow #437 MAPP-2b: each member whose switch cooldown still runs, with its end; none
        // while no switch has failed (never for an instance alone on its device).
        let switch_cooldowns = self.policy_switch_cooldowns(time.unix_ms)?;
        // Workflow #336 L6 (§12.7): a paused pair is judged against the startup configuration;
        // PKG-S3: a due paused pair's packages are measured (at most once per interval).
        let suspension_lift = self.suspension_lift_view(&procedure_manifest)?;
        // Workflow #384 D2-S4: the fact refresh pace in effect as this cycle starts.
        let fact_refresh_settings = self.fact_refresh_settings()?;
        // Workflow #411 B15 R4: the remedy keys in effect as this cycle starts.
        let remedy_aliases = facts
            .instances
            .iter()
            .map(|instance| instance.instance_id.as_str())
            .collect::<Vec<_>>();
        let remedy_context =
            self.remedy_context(&remedy_aliases, HostLocation::EvaluatePolicyCycle)?;
        // Workflow #405 D1: each instance's random delay in effect as this cycle starts.
        let random_delay_context =
            self.random_delay_context(&remedy_aliases, HostLocation::EvaluatePolicyCycle)?;
        // SS-4 (register §1.231): each instance's no-progress switch in effect as this cycle
        // starts.
        let no_progress_context =
            self.no_progress_context(&remedy_aliases, HostLocation::EvaluatePolicyCycle)?;
        // #428 SCHED-C1e (register §1.299 SC1-12): each instance's own pool lines in effect as
        // this cycle starts.
        let pool_lines_context =
            self.pool_lines_context(&remedy_aliases, HostLocation::EvaluatePolicyCycle)?;
        // Workflow #438 HELD-1 (register §1.322 HELD-R8): each instance's held-record keys in
        // effect as this cycle starts; only the held records read them, after the evaluation.
        let task_held_context =
            self.task_held_context(&remedy_aliases, HostLocation::EvaluatePolicyCycle)?;
        // Workflow #411 B41: the page each instance rests on as this cycle starts.
        let chain_handover = self.chain_handover_inputs(&facts.instances)?;
        // Workflow #386 SR-4a2: the blackout keys in effect as this cycle starts.
        let blackout_context =
            self.blackout_context(&remedy_aliases, HostLocation::EvaluatePolicyCycle)?;
        // Workflow #386 SR-2b-2: the owed reruns' gate keys and each instance's effective pacing
        // stage as this cycle starts, read under the performance lock alone, before the policy
        // lock.
        let rerun_settings = self.rerun_gate_settings()?;
        let rerun_stages = remedy_aliases
            .iter()
            .map(|alias| Ok(((*alias).to_owned(), self.effective_pacing_stage(alias)?)))
            .collect::<RuntimeHostResult<BTreeMap<_, _>>>()?;
        // Workflow #427-A (C1), tests: what happens between the sample above and its validation
        // below, and the forced snapshot changes.
        #[cfg(any(test, feature = "test-hooks"))]
        self.between_policy_sample_and_validation_for_test()?;
        // #503 CYC-1 (ruling 1): the cycle's inputs end here; then the wait for the evaluation's
        // policy lock, the work under it before the evaluation (with the evaluation's own inputs,
        // `evaluation_setup`) and after it up to the lock's release (with the evaluation's last
        // bookkeeping, `after_held`).
        let lock_waiting = crate::policy_host::lap_instant();
        let (
            mut cycle,
            eligibility_unknown_pairs,
            pool_scopes,
            random_delays,
            blackout_holds,
            locked,
            evaluating,
            returned,
            locked_setup_calls,
        ) = {
            #[cfg(test)]
            crate::policy_host::delay_cycle_stretch_for_test(
                crate::policy_host::CycleStretch::PolicyLockWait,
            );
            let mut policy = lock(self.policy()?, HostLocation::EvaluatePolicyCycle)?;
            let locked = crate::policy_host::lap_instant();
            // #503 CYC-2 (register §1.528, ruling 1): the calls under the lock before the
            // evaluation, timed one by one as the evaluation's own setup is.
            let mut setup = crate::policy_host::SetupTimer::start(policy.catalog_costs());
            // Workflow #427-A (C1): another instance's run may settle between the sample and
            // this validation; its `policy_outcome_snapshot_changed` is a request error, which
            // actingd answers with a bounded re-sample.
            policy.validate_outcome_key_snapshot(&outcome_keys)?;
            setup.lap(SetupCall::OutcomeKeys, policy.catalog_costs());
            // Workflow #386 SR-4a2 (position 4): the instances a blackout covers as this cycle
            // starts, judged once for the evaluation and the fact refresh's recorder.
            let blackout_holds =
                policy.blackout_holds(&blackout_context, &facts.instances, time)?;
            setup.lap(SetupCall::Blackouts, policy.catalog_costs());
            // #477 LF-1 (register §1.509): the pairs whose latest dispatch at the inputs' position
            // carried a `run_cap`, with when that run ended; a pool such a run's task consumes is
            // unknown in the evaluation until it is read after that end. #455 RR3c (register
            // §1.543): the list read with the needs, under the gates; so is a pool on which the
            // production told under a goal's gap since its reading reaches that gap (#455 RR3d,
            // register §1.548: summed over the dispatches since the reading).
            left_out.give_handed_runs(handed_runs);
            setup.lap(SetupCall::HandedRuns, policy.catalog_costs());
            let evaluating = crate::policy_host::lap_instant();
            let cycle = policy.evaluate(
                &facts,
                &controlled_resources,
                PolicyEvaluationContext {
                    procedure_manifest: &procedure_manifest,
                    time,
                    seed,
                    trigger,
                    sampled_at_monotonic_ms: observed_monotonic_ms,
                    control_gates: &control_gates,
                    instance_occupancy: &|instance_alias| occupancy.get(instance_alias).copied(),
                    suspension_lift: &suspension_lift,
                    pack_admission: &self.pack_admission,
                    fact_refresh: FactRefreshContext {
                        settings: fact_refresh_settings,
                        bindings: &self.fact_refresh_bindings,
                        instance_facts_loaded_at_unix_ms: self.instance_facts_loaded_at_unix_ms,
                        idle_closed: &idle_closed,
                        left_out: &left_out,
                    },
                    remedy: &remedy_context,
                    random_delay: &random_delay_context,
                    chain_handover: &chain_handover,
                    blackout: &blackout_context,
                    blackout_holds: &blackout_holds,
                    rerun: crate::policy_host::RerunContext {
                        settings: rerun_settings,
                        stages: &rerun_stages,
                    },
                    max_concurrent_runs: run_limits.max_concurrent_runs,
                    no_progress_enabled: &no_progress_context,
                    pool_lines: &pool_lines_context,
                    task_held: &task_held_context,
                    device_groups: &device_groups,
                    device_queue: &device_queue,
                    device_group_holds: &device_group_holds,
                    switch_cooldowns: &switch_cooldowns,
                    dispatches_read,
                    standard_pack_variables: standard_pack_variables.as_deref(),
                    availability_withdrawals: &availability_withdrawals,
                    asleep: &asleep,
                    idle_sleepers: &idle_sleepers,
                    entry_blocked: &entry_blocked,
                },
            )?;
            let returned = crate::policy_host::lap_instant();
            let unknown = if cycle.evaluation.is_some() {
                policy.eligibility_unknown_pairs()
            } else {
                BTreeSet::new()
            };
            // Workflow #405 D1: the random delay each pending intent was dispatched under.
            let random_delays = policy.dispatch_random_delays(&cycle.pending_dispatch_intents);
            (
                cycle,
                unknown,
                catalog_pool_scopes(&policy),
                random_delays,
                blackout_holds,
                locked,
                evaluating,
                returned,
                setup.times(),
            )
        };
        let released = crate::policy_host::lap_instant();
        // #428 SCHED-C1d (register §1.299 SC1-11): an evaluated cycle that changed a capped
        // episode records `host.capped_episodes`; a record the store refuses is a warning.
        if cycle.evaluation.is_some() {
            #[cfg(test)]
            crate::policy_host::delay_cycle_stretch_for_test(
                crate::policy_host::CycleStretch::CappedRecord,
            );
            self.keep_capped_episodes(RuntimeLifecycleFailureStage::PolicyDriver)?;
        }
        // #486 ASLEEP-5 (register §1.524, ruling 4): the projection's time, and the time from the
        // cycle's start to the end of its capped record, for the daemon's line of the cycle;
        // nothing is stored. #503 CYC-1 (register §1.526, ruling 1): with every other stretch the
        // host times, each from the end of the one before.
        if let Some(measurement) = cycle.measurement.as_mut() {
            use crate::policy_host::micros_between;
            let capped_ended = crate::policy_host::lap_instant();
            measurement.projection_micros = projection_micros;
            measurement.until_capped_micros = micros_between(cycle_started, capped_ended);
            let stretches = &mut measurement.stretches;
            stretches.before_gates_micros = micros_between(cycle_started, gates_reached);
            stretches.gates_wait_micros = gates_wait_micros;
            stretches.gates_held_micros = micros_between(gates_reached, inputs_started)
                .saturating_sub(gates_wait_micros)
                .saturating_sub(projection_micros);
            stretches.cycle_inputs_micros = micros_between(inputs_started, lock_waiting);
            stretches.policy_lock_wait_micros = micros_between(lock_waiting, locked);
            stretches.evaluation_setup_micros = stretches
                .evaluation_setup_micros
                .saturating_add(micros_between(locked, evaluating));
            // #503 CYC-2 (register §1.528, ruling 1): with its calls under the lock.
            stretches
                .evaluation_setup_calls
                .add_all(&locked_setup_calls);
            stretches.after_held_micros = stretches
                .after_held_micros
                .saturating_add(micros_between(returned, released));
            stretches.capped_record_micros = micros_between(released, capped_ended);
        }
        attach_eligible_since_unknown_reasons(&mut cycle, &eligibility_unknown_pairs);
        attach_missing_instance_fact_reasons(
            &mut cycle,
            &missing_instance_facts,
            facts.ledger_position,
        );
        attach_unknown_priority_offset_task_reasons(
            &mut cycle,
            &unknown_offset_tasks,
            facts.ledger_position,
        );
        for signal in &cycle.detection_planning_signals {
            self.record_policy_planning_signal(signal.clone())?;
        }
        for signal in &cycle.clock_slot_signals {
            self.record_policy_planning_signal(signal.clone())?;
        }
        // Workflow #438 HELD-1 (register §1.322 HELD-R5): the ends of reported holds, then the
        // held records, appended with the cycle's planning signals; none changes a decision.
        for signal in &cycle.task_held_signals {
            self.record_policy_planning_signal(signal.clone())?;
        }
        // Workflow #411 B15 R4 (model-b15 §7.5 to §7.7): the remedy stops, lifts and expiries
        // this evaluation decided, none of them in the projection when it was read.
        let remedy_signals =
            lock(self.policy()?, HostLocation::RecordPolicyPlanningSignal)?.take_remedy_signals();
        for signal in remedy_signals {
            self.record_policy_planning_signal(signal)?;
        }
        // Workflow #463 V2 (register §1.382): each value set on an application that this cycle's
        // new dispatches do not hand their runs, once per (instance, name, declaration, stored
        // value) since the start.
        let variable_warnings =
            lock(self.policy()?, HostLocation::EvaluatePolicyCycle)?.take_variable_warnings();
        self.report_variable_warnings(variable_warnings)?;
        // #484 IDLE-W2 fix round 1 (review-1845; `runtime-config.md`, "What wakes an idle
        // instance"): each failure of the idle plan's look ahead that does not stop the Runtime
        // (one that does ended the evaluation) is said in the daemon's log as a skipped background
        // step is; the candidate it was read for was counted due, as before.
        let look_ahead_failures = lock(self.policy()?, HostLocation::EvaluatePolicyCycle)?
            .take_idle_look_ahead_failures();
        for failure in &look_ahead_failures {
            self.fatal.report_skipped(failure);
        }
        // Workflow #386 SR-2b-1: the `policy_rerun_exhausted` records of runs an online
        // reconciliation settled (this cycle's), not yet recorded; the start records its own
        // before it completes.
        let rerun_signals =
            lock(self.policy()?, HostLocation::RecordPolicyPlanningSignal)?.take_rerun_signals();
        for signal in rerun_signals {
            self.record_policy_planning_signal(signal)?;
        }
        // Workflow #386 SR-3 (scheduling model §11): the suspensions and lifts of task packages
        // this evaluation acted on, each copied once per instance.
        for signal in self.pack_admission.take_signals()? {
            self.record_policy_planning_signal(signal)?;
        }
        // Workflow #384 D2-S5 (D2 model §5.1 MP-3): the cycle's yield point hands the reasons of
        // its waiting candidates to the fact refresh's transition recorder, which only produces
        // transitions: they stay in memory (the cycle's report), no decision reads them and
        // nothing is written to the ledger. An instance the closure's positions 1-2 held as the
        // cycle started (Workflow #429 R-7a2: a control gate, then its queue) raised no demand,
        // so its keys are not observed; Workflow #386 SR-4a2: nor did one a blackout covered
        // (position 4).
        if let Some(evaluation) = &cycle.evaluation {
            let overdue_ms = self.fact_refresh_overdue_ms()?;
            let held = |instance_alias: &str| {
                crate::fact_refresh::member_hold(
                    instance_alias,
                    &control_gates(instance_alias),
                    switch_cooldowns.get(instance_alias).copied(),
                    device_group_holds.get(instance_alias),
                    occupancy.get(instance_alias).copied(),
                )
                .is_some()
                    || blackout_holds.contains_key(instance_alias)
            };
            let (transitions, cycle_seq) = {
                let mut recorder = lock(
                    &self.fact_refresh_recorder,
                    HostLocation::RecordFactRefreshTransitions,
                )?;
                let transitions = recorder.observe(evaluation, &held, time.unix_ms, overdue_ms);
                (transitions, recorder.cycle_seq())
            };
            cycle.fact_refresh_transitions = transitions;
            // #486 ASLEEP-5 (ruling 4): the cycle's number, as the recorder counts evaluated
            // cycles (`cycle_seq`).
            if let Some(measurement) = cycle.measurement.as_mut() {
                measurement.cycle_seq = cycle_seq;
            }
        }
        lock(
            &self.trusted_policy_dispatches,
            HostLocation::RecordTrustedPolicyDispatches,
        )?
        .record_cycle(
            &cycle,
            observed_monotonic_ms,
            &DispatchView {
                facts: &facts,
                resources: &resources,
                pool_scopes: &pool_scopes,
                record_needs: left_out.record_needs(),
                time,
            },
            &random_delays,
        )?;
        Ok(cycle)
    }

    /// `RuntimeOperation::FactRefreshStatus` (Workflow #384 D2-S5, D2 model §6 S5, §8 T39;
    /// register §1.68): the keys the fact refresh's transition recorder holds as waited for, read
    /// from memory. Nothing is read from or appended to the ledger; an answer larger than one IPC
    /// frame is refused `runtime_fact_refresh_status_too_large_for_frame` instead of failing the
    /// frame write.
    pub(super) fn fact_refresh_status_read(
        &self,
        request: &RuntimeRequest,
    ) -> Result<OperationSuccess, RequestFailure> {
        let status = lock(
            &self.fact_refresh_recorder,
            HostLocation::ReadFactRefreshStatus,
        )?
        .status();
        let result = RuntimeResult::FactRefreshStatus {
            status: Box::new(status),
        };
        let reply_bytes = RuntimeReceipt::success(
            request,
            RuntimeReceiptState::Completed,
            None,
            result.clone(),
        )
        .ok()
        .and_then(|receipt| serde_json::to_vec(&receipt).ok())
        .ok_or_else(|| RequestFailure::poison_without_terminal(receipt_error()))?
        .len();
        if reply_bytes > self.maximum_frame_bytes {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    HostCode::RuntimeFactRefreshStatusTooLargeForFrame,
                    HostLocation::ReadFactRefreshStatus,
                    RuntimeErrorCode::InvalidRequest,
                )
                .with_native_detail(format!(
                    "reply is {reply_bytes} bytes; one frame carries at most {} bytes",
                    self.maximum_frame_bytes
                )),
                RuntimeReceiptState::Denied,
                None,
            ));
        }
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result,
            failure_outcome: None,
        })
    }

    /// Workflow #384 D2-S5, test only: the transition recorder in `recorder`'s state, as an
    /// earlier run of cycles would have left it.
    #[cfg(test)]
    pub(super) fn replace_fact_refresh_recorder_for_test(
        &self,
        recorder: crate::fact_refresh_transitions::FactRefreshRecorder,
    ) -> RuntimeHostResult<()> {
        *lock(
            &self.fact_refresh_recorder,
            HostLocation::RecordFactRefreshTransitions,
        )? = recorder;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn replace_procedure_manifest_for_test(
        &self,
        procedure_manifest: ProcedureManifest,
    ) -> RuntimeHostResult<()> {
        let _gate = lock(
            &self.fact_write_gate,
            crate::codes::test_spellings::TestLocation::ReplaceProcedureManifestForTest,
        )?;
        *lock(
            &self.procedure_manifest,
            crate::codes::test_spellings::TestLocation::ReplaceProcedureManifestForTest,
        )? = Some(procedure_manifest);
        Ok(())
    }

    /// #455 P2 (register §1.505): with the readings of the pools read from rows of a list, computed
    /// from the same projection.
    pub(super) fn project_authoritative_policy_inputs_under_gate(
        &self,
        policy: &mut PolicyHost,
        operation: impl Into<Location>,
        outcome_keys: &PolicyOutcomeKeySnapshot,
        as_of_ledger_position: Option<u64>,
    ) -> RuntimeHostResult<(
        EvaluationFacts,
        EvaluationResources,
        actingcommand_policy::GivenPoolReadings,
    )> {
        let (facts, resources, _, _, _, _, readings) = self
            .project_authoritative_policy_inputs_with_gaps_under_gate(
                policy,
                operation,
                outcome_keys,
                as_of_ledger_position,
                None,
                None,
            )?;
        Ok((facts, resources, readings))
    }

    /// Projects the evaluation inputs at the requested position and reports, per instance
    /// alias, the policy instance facts the store did not hold there (Workflow #313 item 4),
    /// and the tasks named by a priority offset that the active catalog does not declare
    /// (Workflow #308 slice 4a-2). `offsets_at_unix_ms` is the evaluation instant: an offset
    /// whose TTL has elapsed by then is not passed; without it every active offset is.
    /// `eligibility_as_of_unix_ms` is the evaluation instant whose memory-only eligibility
    /// ages are projected (Workflow #308 slice 5b); without it none are. Workflow #428 S168-LB1a:
    /// and the record lists the projection left out for their rows. Workflow #469 H1: and who
    /// withdrew the availability of each instance projected unavailable. #455 P2 (register
    /// §1.505, ruling 1): and the readings of the active catalog's pools read from rows of a list,
    /// each computed from its instance's list in the same projection, whose numbers the resources
    /// hold where a fact pool's projected reading goes.
    /// The caller holds `policy` and then `fact_write_gate` (Workflow #191 L1 lock order).
    pub(super) fn project_authoritative_policy_inputs_with_gaps_under_gate(
        &self,
        policy: &mut PolicyHost,
        operation: impl Into<Location>,
        outcome_keys: &PolicyOutcomeKeySnapshot,
        as_of_ledger_position: Option<u64>,
        offsets_at_unix_ms: Option<u64>,
        eligibility_as_of_unix_ms: Option<u64>,
    ) -> RuntimeHostResult<(
        EvaluationFacts,
        EvaluationResources,
        MissingPolicyInstanceFacts,
        UnknownPriorityOffsetTasks,
        LeftOutRecordLists,
        AvailabilityWithdrawals,
        actingcommand_policy::GivenPoolReadings,
    )> {
        let operation: Location = operation.into();
        self.synchronize_fact_store_under_gate()?;
        let inputs = lock(&self.policy_inputs, HostLocation::ReadPolicyInputs)?
            .clone()
            .ok_or_else(|| {
                policy_admission_request(HostCode::PolicyInputsUnconfigured, operation)
            })?;
        self.validate_policy_input_authority(&inputs, operation)?;
        let latest_ledger_position = self
            .ledger
            .latest_sequence()
            .map_err(|_| ledger_error(HostLocation::ReadPolicyFactPosition))?;
        let ledger_position = match as_of_ledger_position {
            Some(position) if position == 0 || position > latest_ledger_position => {
                return Err(policy_admission_request(
                    HostCode::PolicyInputPositionUnavailable,
                    operation,
                ));
            }
            Some(position) => position,
            None => latest_ledger_position,
        };
        #[cfg(test)]
        let ledger_position = if as_of_ledger_position.is_none() {
            match self
                .policy_outcome_projection_position_override
                .swap(0, Ordering::AcqRel)
            {
                0 => ledger_position,
                injected => injected,
            }
        } else {
            ledger_position
        };
        // The instance set is read from the fact store at the projected position; the
        // configuration contributes the static identity only (alias, host, server, game).
        let fact_store = lock(&self.facts, HostLocation::ProjectPolicyFacts)?;
        let historical;
        let fact_projection = if ledger_position == latest_ledger_position {
            &*fact_store
        } else {
            historical = fact_store.at_position(&self.ledger, ledger_position)?;
            &historical
        };
        let mut base_facts = inputs.facts().clone();
        let (instances, missing_instance_facts, availability_withdrawals) =
            project_policy_instances(&inputs, fact_projection);
        base_facts.instances = instances;
        // Manual priority offsets (Workflow #308 slice 4a-2) come from the store's
        // `session.task.<task>.priority_offset` records at the projected position.
        let contexts = base_facts
            .instances
            .iter()
            .map(|instance| InstanceFactContext {
                instance_id: instance.instance_id.clone(),
                server_id: instance.server_id.clone(),
                game_id: instance.game_id.clone(),
            })
            .collect::<Vec<_>>();
        let live_offsets = fact_projection
            .active_priority_offsets(&contexts, operation)?
            .into_iter()
            .filter(|offset| {
                offsets_at_unix_ms.is_none_or(|now| {
                    offset
                        .expires_at_unix_ms
                        .is_none_or(|expires| now <= expires)
                })
            })
            .collect::<Vec<_>>();
        for offset in
            crate::fact_store::resolve_priority_offsets(&live_offsets, &base_facts.instances)
        {
            if base_facts.priority_offsets.iter().any(|configured| {
                configured.task_id == offset.task_id && configured.instance_id == offset.instance_id
            }) {
                return Err(policy_admission_request(
                    HostCode::PolicyFactAuthorityConflict,
                    operation,
                ));
            }
            base_facts.priority_offsets.push(offset);
        }
        base_facts.tasks = policy.task_runtime_snapshots(
            &self.ledger,
            ledger_position,
            eligibility_as_of_unix_ms,
        )?;
        base_facts.tasks.retain(|state| {
            base_facts
                .instances
                .iter()
                .any(|instance| instance.instance_id == state.instance_id)
        });
        // SCH-M ED-2 (frozen model v1.0 §3.4 D-15): each pair's expected duration E from its
        // instance's `health.run_duration` record at the projected position.
        self.project_expected_runtimes(policy, fact_projection, &mut base_facts)?;
        let authoritative_outcomes = lock(
            &self.authoritative_policy_outcomes,
            HostLocation::ProjectPolicySchedulingOutcomes,
        )?;
        if base_facts
            .outcomes
            .iter()
            .any(|outcome| outcome_keys.keys.contains_key(&outcome.task_id))
        {
            return Err(policy_admission_request(
                HostCode::PolicyOutcomeAuthorityConflict,
                operation,
            ));
        }
        for ((task_id, _), expected_run) in &outcome_keys.completed_runs {
            // Workflow #372 K5: the entry is keyed by the active task; the run, and its
            // authoritative outcome, by the id it ran as, which may be one the task continues.
            let key = &(
                expected_run.catalog_task_id.clone(),
                expected_run.instance_alias.clone(),
            );
            let Some(expected_keys) = outcome_keys.keys.get(task_id) else {
                continue;
            };
            // Workflow #429 R-6b: a stopped run projects no outcome, as a failed one projects none.
            if matches!(
                expected_run.execution_outcome,
                PolicyExecutionOutcome::Failed { .. } | PolicyExecutionOutcome::Stopped { .. }
            ) {
                if authoritative_outcomes.contains_key(key) {
                    return Err(RuntimeHostError::fatal(
                        HostCode::PolicyOutcomeFailedRunResidual,
                        operation,
                        RuntimeErrorCode::RuntimeHalted,
                    ));
                }
                continue;
            }
            let outcome = authoritative_outcomes.get(key).ok_or_else(|| {
                RuntimeHostError::request(
                    SharedCode::OutcomeProjectionNotReady,
                    operation,
                    RuntimeErrorCode::RuntimeUnavailable,
                )
            })?;
            let identity = outcome.identity();
            if !completed_run_matches_outcome(expected_run, outcome) {
                return Err(RuntimeHostError::request(
                    SharedCode::OutcomeProjectionNotReady,
                    operation,
                    RuntimeErrorCode::RuntimeUnavailable,
                ));
            }
            // Workflow #390 OUT-1 (FCM O-04): admission refuses a catalog key the package does
            // not declare, so the package declared every key the run's own catalog referenced.
            // Each of them that the run did not produce and the active catalog still references
            // was declared and not produced: it evaluates `false`.
            let produced_key = outcome.disposition().outcome_key();
            let produced_listed = expected_keys.contains(produced_key);
            let unproduced_keys = expected_keys
                .iter()
                .filter(|key| {
                    key.as_str() != produced_key
                        && expected_run.referenced_outcome_keys.contains(key.as_str())
                })
                .cloned()
                .collect::<Vec<_>>();
            // Workflow #400: the pair's latest outcome carries a key the active catalog no longer
            // lists for its task, and the active catalog lists none of the other keys the run's
            // catalog referenced (a later catalog changed the task's outcome keys). Like a latest
            // completion without a run identity (#399), the pair has no usable outcome: it
            // projects nothing, so the evaluator reads the keys it references as
            // `outcome_missing` until the task completes under this catalog. Other pairs and
            // instances project as before.
            if !produced_listed && unproduced_keys.is_empty() {
                continue;
            }
            if !base_facts
                .instances
                .iter()
                .any(|instance| instance.instance_id == identity.instance_alias())
            {
                continue;
            }
            if ledger_position < identity.terminal_sequence() {
                return Err(RuntimeHostError::request(
                    SharedCode::OutcomeProjectionNotReady,
                    operation,
                    RuntimeErrorCode::RuntimeUnavailable,
                ));
            }
            #[cfg(test)]
            if self
                .policy_outcome_projection_failures
                .swap(0, Ordering::AcqRel)
                != 0
            {
                return Err(RuntimeHostError::fatal(
                    crate::codes::test_spellings::TestCode::PolicyOutcomeProjectionInjectedFailure,
                    operation,
                    RuntimeErrorCode::RuntimeHalted,
                ));
            }
            let projected = self
                .ledger
                .project_scheduling_outcomes(identity.clone(), ledger_position)
                .map_err(|error| {
                    if error.code() == SharedCode::OutcomeProjectionNotReady.as_str()
                        || error.code()
                            == actingcommand_ledger::LedgerCode::OutcomeProjectionPositionInvalid
                                .as_str()
                    {
                        RuntimeHostError::request(
                            SharedCode::OutcomeProjectionNotReady,
                            operation,
                            RuntimeErrorCode::RuntimeUnavailable,
                        )
                    } else {
                        ledger_error(HostLocation::ProjectPolicySchedulingOutcome)
                    }
                })?;
            if projected.scheduling_result() != outcome {
                return Err(RuntimeHostError::fatal(
                    HostCode::PolicyOutcomeProjectionMismatch,
                    operation,
                    RuntimeErrorCode::RuntimeHalted,
                ));
            }
            // The ledger is append-only, so a run's admission request confirmed through its
            // terminal stays confirmed: a repeated cycle does not re-read it (Workflow #313
            // goal 5, #317 item F).
            let through_sequence = identity.terminal_sequence();
            if !policy.completed_run_admission_confirmed(expected_run, through_sequence) {
                validate_completed_run_admission_request(
                    &self.ledger,
                    expected_run,
                    through_sequence,
                )?;
                policy.record_completed_run_admission_confirmed(expected_run, through_sequence);
            }
            // A failed run was skipped above, so the settled duration is the success's
            // `runtime_ms`, the value slice 5b records as `last_duration_ms`.
            let PolicyExecutionOutcome::Succeeded { runtime_ms } = &expected_run.execution_outcome
            else {
                return Err(RuntimeHostError::fatal(
                    HostCode::PolicyOutcomeFailedRunResidual,
                    operation,
                    RuntimeErrorCode::RuntimeHalted,
                ));
            };
            let settlement_value = |value: u64| {
                i64::try_from(value)
                    .map(PolicyFactValue::Integer)
                    .map_err(|_| {
                        RuntimeHostError::fatal(
                            HostCode::PolicySettlementOutcomeOverflow,
                            operation,
                            RuntimeErrorCode::RuntimeHalted,
                        )
                    })
            };
            // A settled run whose outcome the active catalog references projects it and two
            // settlement records that share its observation instant, activity window and expiry
            // (Workflow #313 goal 5). Each unproduced key projects `Boolean(false)` with the same
            // instant, window and expiry (OUT-1).
            let observed_at_unix_ms = outcome.terminal_timestamp_unix_ms();
            let mut records = Vec::new();
            if produced_listed {
                records.extend([
                    (produced_key.to_owned(), PolicyFactValue::Boolean(true)),
                    (
                        format!("{produced_key}.duration_ms"),
                        settlement_value(*runtime_ms)?,
                    ),
                    (
                        format!("{produced_key}.completed_at_unix_ms"),
                        settlement_value(observed_at_unix_ms)?,
                    ),
                ]);
            }
            for key in unproduced_keys {
                if !records.iter().any(|(recorded, _)| *recorded == key) {
                    records.push((key, PolicyFactValue::Boolean(false)));
                }
            }
            for (outcome_key, value) in records {
                base_facts.outcomes.push(ObservedOutcome {
                    task_id: task_id.clone(),
                    instance_id: identity.instance_alias().to_owned(),
                    outcome_key,
                    value,
                    observed_at_unix_ms,
                    expires_at_unix_ms: None,
                    activity_window_id: Some(expected_run.activity_window_id.clone()),
                });
            }
        }
        let catalog = policy.active_loaded();
        // ROUT-L RL-1b (RL-1a review, #386 6399836824 item 1; #419; model §3.6): under a composed
        // generation a configured static pool reading names a pool of a game in use or none; a
        // reading for a pool no composed game declares is left out (the workbench records it
        // once per pool), so it never stops the evaluation of every instance. A configured
        // catalog's readings are passed as they are.
        let static_resources = match &catalog {
            Some(catalog)
                if catalog.generation().catalog_id()
                    == actingcommand_policy::STANDARD_PACKS_CATALOG_ID =>
            {
                let mut resources = inputs.resources().clone();
                resources.pools.retain(|reading| {
                    catalog
                        .compiled()
                        .catalog()
                        .pools
                        .pools
                        .iter()
                        .any(|pool| pool.id == reading.pool_id)
                });
                resources
            }
            _ => inputs.resources().clone(),
        };
        let mut unknown_offset_tasks = UnknownPriorityOffsetTasks::new();
        if let Some(catalog) = &catalog {
            validate_static_fact_pool_authority(
                catalog.compiled(),
                inputs.facts(),
                &static_resources,
                operation,
            )?;
            // An offset for a task the catalog does not declare is still passed (the
            // evaluator ignores it) and only reported.
            for offset in &base_facts.priority_offsets {
                if !catalog
                    .compiled()
                    .catalog()
                    .tasks
                    .tasks
                    .iter()
                    .any(|task| task.id == offset.task_id)
                {
                    unknown_offset_tasks.insert(offset.task_id.clone());
                }
            }
        }
        let facts = fact_projection.overlay_policy_facts(
            &base_facts,
            &static_resources,
            ledger_position,
        )?;
        let (resources, readings) = if let Some(catalog) = catalog {
            fact_projection.validate_pool_sources(catalog.compiled(), |scope| {
                self.fact_scope_instances(scope)
            })?;
            // #455 P2 (register §1.505, rulings 1 and 5): each pool read from rows of a list is
            // read from its instance's list in this projection, at this position, under the gate
            // the fact pools are projected under; its number goes where a fact pool's projected
            // reading goes, so it enters the snapshot's identity below and each instance's view.
            let readings = fact_projection.list_pool_readings(catalog.compiled());
            let resources = actingcommand_policy::project_fact_pools_with_readings(
                catalog.compiled(),
                &facts,
                &static_resources,
                &readings,
            );
            (resources, readings)
        } else {
            (
                static_resources,
                actingcommand_policy::GivenPoolReadings::default(),
            )
        };
        // Workflow #428 S168-LB1a: with the record lists the projection left out for their rows.
        let (facts, left_out) = fact_projection.overlay_policy_facts_with_left_out(
            &base_facts,
            &resources,
            ledger_position,
        )?;
        Ok((
            facts,
            resources,
            missing_instance_facts,
            unknown_offset_tasks,
            left_out,
            availability_withdrawals,
            readings,
        ))
    }

    /// The registered alias set must equal the configured static identity set (alias, host,
    /// server, game), and every configured host must exist in `resources.hosts`. The
    /// configured `instances` snapshot is not compared beyond its identity: availability,
    /// capabilities and preferred tasks are read from the instance fact store.
    pub(super) fn validate_policy_input_authority(
        &self,
        inputs: &PolicyInputSnapshot,
        operation: impl Into<Location>,
    ) -> RuntimeHostResult<()> {
        let operation: Location = operation.into();
        let registered = lock(
            &self.registered_instances,
            HostLocation::ValidatePolicyInstanceMetadata,
        )?;
        let registered_aliases = registered
            .values()
            .map(|instance| instance.instance_alias.as_str())
            .collect::<BTreeSet<_>>();
        let configured_aliases = inputs
            .instance_identities()
            .map(|identity| identity.instance_id)
            .collect::<BTreeSet<_>>();
        if registered_aliases != configured_aliases {
            return Err(policy_admission_request(
                HostCode::PolicyInstanceMetadataUntrusted,
                operation,
            ));
        }
        let host_ids = inputs
            .resources()
            .hosts
            .iter()
            .map(|host| host.host_id.as_str())
            .collect::<BTreeSet<_>>();
        if inputs
            .instance_identities()
            .any(|identity| !host_ids.contains(identity.host_id))
        {
            return Err(policy_admission_request(
                HostCode::PolicyResourceMetadataUntrusted,
                operation,
            ));
        }
        Ok(())
    }

    pub(super) fn project_policy_forward(
        &self,
        facts: &EvaluationFacts,
        resources: &EvaluationResources,
        time: EvaluationTime,
        seed: u64,
        config: ForwardProjectionConfig,
    ) -> RuntimeHostResult<ForwardProjection> {
        let declared_facts = facts;
        let (facts, fact_projection) = {
            let mut fact_projection = lock(&self.facts, HostLocation::ProjectForwardFacts)?.clone();
            fact_projection.synchronize(&self.ledger)?;
            let ledger_position = self
                .ledger
                .latest_sequence()
                .map_err(|_| ledger_error(HostLocation::ProjectForwardFactPosition))?;
            let facts =
                fact_projection.overlay_external_policy_facts(facts, resources, ledger_position)?;
            (facts, fact_projection)
        };
        let (catalog, workloads) = {
            let policy = lock(self.policy()?, HostLocation::ProjectForwardCatalog)?;
            let catalog = policy.active_loaded().ok_or_else(|| {
                RuntimeHostError::request(
                    SharedCode::PolicyCatalogUnavailable,
                    ContractLocation::ProjectPolicyForward,
                    RuntimeErrorCode::InvalidRequest,
                )
            })?;
            (catalog, policy.active_performance_workloads()?)
        };
        validate_static_fact_pool_authority(
            catalog.compiled(),
            declared_facts,
            resources,
            ContractLocation::ProjectPolicyForward,
        )?;
        fact_projection
            .validate_pool_sources(catalog.compiled(), |scope| self.fact_scope_instances(scope))?;
        // #455 P2 (register §1.505, ruling 7): a pool read from rows of a list starts from the
        // reading computed from its instance's list in the same projection, carried across the
        // horizon as a fact reading is.
        let readings = fact_projection.list_pool_readings(catalog.compiled());
        let mut resources = actingcommand_policy::project_fact_pools_with_readings(
            catalog.compiled(),
            &facts,
            resources,
            &readings,
        );
        // Workflow #429 R-2: the heavy-dispatch limits in effect, as an evaluation reads them.
        let run_limits = self.shared_run_limits()?;
        lock(
            &self.performance_control,
            HostLocation::ApplyForwardPerformanceControl,
        )?
        .apply_to_resources_with(&mut resources.hosts, &workloads, run_limits.heavy)?;
        project_forward_with_readings(
            catalog.compiled(),
            &facts,
            &resources,
            &readings,
            time,
            seed,
            config,
        )
        .map_err(|error| {
            RuntimeHostError::request(
                error.code(),
                ContractLocation::ProjectPolicyForward,
                RuntimeErrorCode::InvalidRequest,
            )
        })
    }

    pub(super) fn admit_policy_dispatch(
        &self,
        intent: &DispatchIntent,
        reason_chain: &DecisionReasonChain,
        context: &PolicyAdmissionContext,
        task_request: Option<&ContainedTaskRequest>,
    ) -> RuntimeHostResult<PolicyDispatchAdmission> {
        let mut rejection = None;
        let result = (|| {
            // SCH-M ED-2 (coordinator ruling 10-24 14:1x): the declared duration of the intent's
            // task spec, which an intent record names its own duration against.
            let declared_duration_ms = {
                let policy = lock(self.policy()?, HostLocation::ValidatePolicyDispatch)?;
                if let Some(replay) = policy.replay_admission(&self.ledger, intent, reason_chain)? {
                    return Ok(replay);
                }
                policy.declared_duration_ms(&intent.task_id)
            };
            // SCH-M ED-2: the configured declared duration of a fact refresh read.
            let refresh_declared_ms = self.fact_refresh_settings()?.expected_duration_ms;
            let trusted = lock(
                &self.trusted_policy_dispatches,
                HostLocation::AuthorizeTrustedPolicyDispatch,
            )?
            .authorize(intent, reason_chain)?;
            // Workflow #455 RR3 (register §1.538, ruling 6): what the workbench keeps of each game
            // root for the needs computed from stored records, from memory, before any lock the
            // admission takes, so the needs of the intent's instance are computed again below.
            let record_roots = self.goal_record_roots()?;
            // Workflow #411 B15 R4 (model-b15 §7.9): an intent that names a remedy round is
            // checked against the remedy keys in effect for its instance.
            let remedy_context = if reason_chain
                .reasons
                .iter()
                .any(|reason| crate::remedy::is_round_reason(&reason.outcome))
            {
                Some(self.remedy_context(
                    &[intent.instance_id.as_str()],
                    HostLocation::AdmitPolicyDispatch,
                )?)
            } else {
                None
            };
            // Workflow #386 SR-4a2: the blackout keys in effect for the intent's instance, and the
            // response deadline of the request it is admitted with (the evaluation's own rule
            // when it is admitted without one).
            let blackout_context = self.blackout_context(
                &[intent.instance_id.as_str()],
                HostLocation::AdmitPolicyDispatch,
            )?;
            let response_deadline_ms = task_request.map_or_else(
                || crate::policy_host::evaluation_deadline_ms(intent),
                ContainedTaskRequest::response_deadline_ms,
            );
            if context.fact_ledger_position != trusted.intent.input_ledger_position
                || context.fact_snapshot_id != trusted.intent.fact_snapshot_id
                || context.fencing_owner_epoch != self.owner_epoch
            {
                return Err(policy_admission_request(
                    HostCode::PolicyAdmissionContextUntrusted,
                    HostLocation::AdmitPolicyDispatch,
                ));
            }
            let admission_time = || {
                let elapsed_ms = self
                    .monotonic_ms()?
                    .checked_sub(trusted.observed_monotonic_ms)
                    .ok_or_else(|| {
                        policy_admission_fatal(
                            HostCode::PolicyAdmissionClockRegressed,
                            HostLocation::AdmitPolicyDispatch,
                        )
                    })?;
                trusted
                    .intent
                    .prerequisites
                    .evaluated_at_unix_ms
                    .checked_add(elapsed_ms)
                    .ok_or_else(|| {
                        policy_admission_fatal(
                            HostCode::PolicyAdmissionClockOverflow,
                            HostLocation::AdmitPolicyDispatch,
                        )
                    })
            };
            let now_unix_ms = admission_time()?;
            // Approval projection and dispatch admission share one order so a concurrent revocation
            // cannot appear in the ledger before a dispatch authorized by the superseded fact.
            let _governance_gate = lock(
                &self.governance_write_gate,
                HostLocation::ProjectPolicyApprovals,
            )?;
            // ROUT-L RL-1b (model §3.5): an intent of a composed generation names no approval,
            // so its approval check passes without a projection.
            let approval_fact_ids = if intent.approval_refs.is_empty() {
                BTreeSet::new()
            } else {
                match ApprovalProjection::recover(&self.ledger, Arc::clone(&self.state)) {
                    Ok(projection) => projection.active_for_dispatch(intent),
                    // #419 LN-WS-F (R1): an approval read the writer did not answer in time, or
                    // stopped at its budget, fails only this admission, before its intent is
                    // appended; the next evaluation derives the dispatch again. Any other failure
                    // latches, as before.
                    Err(error) => {
                        if !unanswered_ledger_read(&error) {
                            self.fatal.mark(error.clone())?;
                        }
                        return Err(error);
                    }
                }
            };
            let authoritative_context = PolicyAdmissionContext {
                fact_ledger_position: trusted.intent.input_ledger_position,
                fact_snapshot_id: trusted.intent.fact_snapshot_id.clone(),
                approval_fact_ids,
                fencing_owner_epoch: self.owner_epoch,
                now_unix_ms,
            };
            let context = &authoritative_context;
            // Workflow #191 ps1, #403, Workflow #429 R-7a2 (R-7a §2.6 O-1, O-4): the control
            // gates read now, under the admission lock, in the one order (the player hold from
            // R-7b, this instance's pause, the global pause, this instance's #403 fence, whatever
            // an intent evaluated before it says); the first refuses the dispatch with its
            // effective code before the instance's queue, the performance and the capacity gates
            // are consulted.
            let fenced = self.resource_fenced_alias(&intent.instance_id)?;
            let control = lock(
                &self.scheduling_pause,
                HostLocation::GatePolicySchedulingPause,
            )?
            .control_gates(&intent.instance_id, fenced.is_some());
            let control_error = crate::control_gates::effective_code(&control).map(|code| {
                let error = RuntimeHostError::request(
                    code,
                    HostLocation::AdmitPolicyDispatch,
                    RuntimeErrorCode::InvalidRequest,
                );
                // The fence's refusal names the fenced instance by its opaque identifier, as the
                // instance's queue does (Workflow #378 A4, part B L3).
                match fenced.filter(|_| code == HostCode::DispatchResourceFenced) {
                    Some(instance_id) => error.with_outcome_values(|outcome| {
                        outcome.instance_id(crate::failure_identity::identifier_text(&instance_id))
                    }),
                    None => error,
                }
            });
            // Workflow #369 W-1: a holder or an eligible waiting claim, read under the scheduler
            // lock, rejects the dispatch at Info after its intent.
            let held = match control_error {
                Some(_) => None,
                None => self.dispatch_held_error(&intent.instance_id, context.now_unix_ms)?,
            };
            let mut gate_error = match (control_error, held) {
                (Some(control_error), _) => Some(control_error),
                (None, Some(held)) => Some(held),
                (None, None) => match lock(
                    &self.performance_control,
                    HostLocation::GatePolicyPerformanceDispatch,
                )?
                .gate_dispatch(
                    &intent.instance_id,
                    intent.prerequisites.urgency_milli,
                    context.now_unix_ms,
                )? {
                    PerformanceDispatchGate::Allowed => None,
                    PerformanceDispatchGate::Deferred {
                        reason,
                        deadline_disposition,
                        event,
                        retry_after_ms,
                    } => {
                        if let Some(event) = event {
                            self.record_performance_events(&[
                                PerformanceSemanticEvent::BalanceChanged(event),
                            ])?;
                        }
                        let code = if deadline_disposition
                        == Some(
                            actingcommand_contract::PerformanceDeadlineDisposition::CapacityFailure,
                        ) {
                        HostCode::PerformanceCapacityDeadlineConflict.code()
                    } else {
                        reason
                    };
                        let mut projection =
                            RuntimeErrorProjection::new(RuntimeErrorCode::InvalidRequest, false);
                        if let Some(retry_after_ms) = retry_after_ms {
                            projection = projection.with_retry_after(retry_after_ms);
                        }
                        let mut error = RuntimeHostError::with_projection(
                            code,
                            HostLocation::AdmitPolicyDispatch,
                            projection,
                        );
                        // A throttle deferral tells the driver when this instance may start: the
                        // error carries the wait and the rejection record the absolute time.
                        if let Some(retry_after_ms) = retry_after_ms {
                            let next_eligible_unix_ms = context
                                .now_unix_ms
                                .checked_add(retry_after_ms)
                                .ok_or_else(|| {
                                    policy_admission_fatal(
                                        HostCode::PolicyAdmissionClockOverflow,
                                        HostLocation::AdmitPolicyDispatch,
                                    )
                                })?;
                            let mut rejection = error.policy_rejection();
                            rejection.next_eligible_unix_ms = Some(next_eligible_unix_ms);
                            error.lifecycle.policy_rejection = Some(Box::new(rejection));
                        }
                        Some(error)
                    }
                },
            };
            if gate_error.is_none()
                && let Err(error) = self.admit_capacity()
            {
                if error.is_fatal() {
                    return Err(error);
                }
                gate_error = Some(error);
            }
            let resolved = self
                .resolve_instance(&intent.instance_id)
                .map_err(|failure| *failure.error)?;
            let blackout_instance_id =
                crate::failure_identity::identifier_text(&resolved.instance_id());
            let request_id = self
                .events
                .issuer()
                .mint_request_id()
                .map_err(|_| policy_id_error(HostLocation::IssuePolicyRequestId))?;
            let correlation_id = self
                .events
                .issuer()
                .mint_correlation_id()
                .map_err(|_| policy_id_error(HostLocation::IssuePolicyCorrelationId))?;
            let holder = self
                .events
                .issuer()
                .mint_holder_id()
                .map_err(|_| policy_id_error(HostLocation::IssuePolicyHolderId))?;
            let task_id = self
                .events
                .issuer()
                .mint_task_id()
                .map_err(|_| policy_id_error(HostLocation::IssuePolicyTaskId))?;
            let run_id = self
                .events
                .issuer()
                .mint_run_id()
                .map_err(|_| policy_id_error(HostLocation::IssuePolicyRunId))?;
            let run_links = RuntimeRunLinks::new(task_id, run_id);
            let holder_id = *holder.transport();
            let request = RuntimeRequest::new(
                request_id,
                correlation_id,
                None,
                EventActor::Agent,
                EventSource::Adapter,
                context.now_unix_ms,
                RuntimeOperation::acquire_lease(intent.instance_id.clone(), holder),
            )
            .map_err(|_| policy_contract_error(HostLocation::BuildPolicyRuntimeRequest))?;
            let validated = request
                .validate()
                .map_err(|_| policy_contract_error(HostLocation::ValidatePolicyRuntimeRequest))?;
            let connection_id = ConnectionId::new(POLICY_CONNECTION_VALUE).map_err(|error| {
                RuntimeHostError::scheduler(HostLocation::BuildPolicyConnection, &error)
            })?;
            let action_id = self.events.action_id()?;
            let links = run_links.apply(self.events.request_links(
                &validated,
                Some(resolved.instance_id()),
                None,
                Some(action_id),
            ));
            let data = policy_event_data(intent, reason_chain)?;
            // Workflow #405 D1 (register X14): the intent of a pair dispatched under a random
            // delay records it; the admitted, rejected and completed records never do. SCH-M
            // ED-2 (coordinator ruling 10-24 14:1x): an intent whose duration is not its task
            // spec's declared one (its expected duration E, or a fact refresh's configured
            // duration) records it, so recovery reserves it again.
            let recorded_duration_ms = declared_duration_ms
                .filter(|declared| *declared != intent.expected_duration_ms)
                .map(|_| intent.expected_duration_ms);
            let intent_draft = PolicyPayloadDraft::dispatch_intent_with(
                data.clone(),
                trusted.random_delay,
                recorded_duration_ms,
                AuditInput::new(),
            );
            let event = self.events.draft(
                EventSeverity::Info,
                EventSource::Scheduler,
                OriginModule::Policy,
                EventActor::Scheduler,
                links.clone(),
                intent_draft,
            )?;
            let event = self.events.sanitize(event)?;
            let plan = CriticalEventPlan::new(CriticalOperation::PolicyDispatch, event)
                .map_err(|_| critical_plan_error())?;
            // Workflow #191 U5-F1: the policy guard is taken after the outcome gate and held
            // from here across the intent append, the lease and the outcome append until both
            // events are applied (lock order: policy_outcome_gate -> policy -> fact_write_gate).
            let outcome_gate = lock(
                &self.policy_outcome_gate,
                HostLocation::SnapshotPolicyOutcomeState,
            )?;
            let mut policy = lock(self.policy()?, HostLocation::ReadPolicyOutcomeKeys)?;
            let (outcome_keys, current_facts, current_resources, current_needs, fact_gate) = {
                let outcome_keys = policy.outcome_key_snapshot()?;
                if outcome_keys.generation.as_ref().is_none_or(|generation| {
                    generation.catalog_hash() != intent.catalog_hash
                        || generation.catalog_version() != intent.catalog_version
                }) {
                    return Err(policy_admission_request(
                        HostCode::CatalogActiveGenerationChanged,
                        HostLocation::AdmitPolicyDispatch,
                    ));
                }
                let fact_gate = lock(
                    &self.fact_write_gate,
                    HostLocation::ValidatePolicyFactFreshness,
                )?;
                // The eligibility ages are projected as of the intent's evaluation instant, as
                // that evaluation projected them (Workflow #308 slice 5b).
                let (current_facts, current_resources, _, _, _, _, current_readings) = self
                    .project_authoritative_policy_inputs_with_gaps_under_gate(
                        &mut policy,
                        HostLocation::AdmitPolicyDispatch,
                        &outcome_keys,
                        None,
                        None,
                        Some(trusted.intent.prerequisites.evaluated_at_unix_ms),
                    )?;
                // Workflow #455 RR3 (register §1.538, ruling 6): the needs of the intent's
                // instance computed again from what the store holds now, at its evaluation's
                // time, under the same gate. #455 RR3c (register §1.543): with the handed runs at
                // the position of these inputs, as an evaluation of them would be given.
                let current_handed = policy.handed_runs(current_facts.ledger_position);
                let current_needs = self.record_needs_under_gate(
                    &policy,
                    super::record_needs::NeedsInputs {
                        facts: &current_facts,
                        readings: &current_readings,
                        handed: &current_handed,
                    },
                    &record_roots,
                    trusted.evaluation_time,
                    Some(&intent.instance_id),
                )?;
                (
                    outcome_keys,
                    current_facts,
                    current_resources,
                    current_needs,
                    fact_gate,
                )
            };
            drop(outcome_gate);
            // Workflow #369 W-1 (model-369 v3.1 §3.3): the intents of one cycle are admitted one
            // after another, and an earlier one's dispatch on another instance moves the snapshot
            // identity (its task state, settlement and device facts). The intent is stale only
            // when its own instance view moved too. Workflow #455 RR3 (ruling 6): the needs
            // computed from its instance's stored records are part of that view, and the lists
            // and the tables they read are in no snapshot identity: an intent whose needs moved
            // is stale whatever the snapshot.
            let current_needs_identity = current_needs
                .get(&intent.instance_id)
                .map(|needs| needs.identity().to_owned());
            if (current_facts.fact_snapshot_id != trusted.intent.fact_snapshot_id
                || current_needs_identity != trusted.needs_identity)
                && dispatch_view_identity(
                    &current_facts,
                    &current_resources,
                    &catalog_pool_scopes(&policy),
                    &intent.instance_id,
                    current_needs_identity.as_deref(),
                )? != trusted.view_identity
            {
                return Err(policy_admission_request(
                    HostCode::PolicyFactsStale,
                    HostLocation::AdmitPolicyDispatch,
                ));
            }
            // SCH-M ED-2 (§3.4 D-15): the duration the admission's own projection gives the
            // intent's pair, which the intent's must equal.
            let admission_duration = crate::policy_host::AdmissionDuration {
                expected_runtime_ms: current_facts
                    .tasks
                    .iter()
                    .find(|state| {
                        state.task_id == intent.task_id && state.instance_id == intent.instance_id
                    })
                    .and_then(|state| state.expected_runtime_ms),
                refresh_declared_ms,
            };
            let procedure_manifest = lock(
                &self.procedure_manifest,
                HostLocation::ValidateProcedureManifest,
            )?
            .clone()
            .ok_or_else(|| {
                policy_admission_request(
                    HostCode::ProcedureManifestUnconfigured,
                    HostLocation::AdmitPolicyDispatch,
                )
            })?;
            procedure_manifest.validate_intent(intent, HostLocation::AdmitPolicyDispatch)?;
            // Workflow #336 L6 (§12.7): the evaluation's lift rule, under the admission lock; its
            // measurements are the evaluation's while the interval runs (PKG-S3).
            let suspension_lift = self.suspension_lift_view(&procedure_manifest)?;
            let appender = PolicyAdmissionAppender::new(&self.ledger, fact_gate);
            let success_links = links.clone();
            let failure_links = links;
            let success_data = data.clone();
            let failure_data = data;
            let result = execute_critical(
                &appender,
                self.events.fingerprinter(),
                plan,
                || {
                    #[cfg(test)]
                    policy_crash_test_barrier("after_policy_intent");
                    if let Some(error) = gate_error.clone() {
                        return CriticalActionReport::Failed {
                            error: RequestFailure::request(
                                error,
                                RuntimeReceiptState::Denied,
                                None,
                            ),
                            effect: EffectDisposition::NotPerformed,
                        };
                    }
                    let ledger_high_watermark = match self.ledger.latest_sequence() {
                        Ok(position) => position,
                        Err(_) => {
                            return CriticalActionReport::Failed {
                                error: RequestFailure::poison_without_terminal(ledger_error(
                                    HostLocation::ReadPolicyLedgerPosition,
                                )),
                                effect: EffectDisposition::NotPerformed,
                            };
                        }
                    };
                    if let Err(error) = policy.validate_outcome_key_snapshot(&outcome_keys) {
                        return CriticalActionReport::Failed {
                            error: RequestFailure::request(
                                error,
                                RuntimeReceiptState::Denied,
                                None,
                            ),
                            effect: EffectDisposition::NotPerformed,
                        };
                    }
                    // Lock acquisition and durable intent append may cross a retry boundary.
                    // Final eligibility and the receipt use one fresh, trusted monotonic time.
                    let now_unix_ms = match admission_time() {
                        Ok(now) => now,
                        Err(error) => {
                            return CriticalActionReport::Failed {
                                error: RequestFailure::poison_without_terminal(error),
                                effect: EffectDisposition::NotPerformed,
                            };
                        }
                    };
                    let final_context = PolicyAdmissionContext {
                        now_unix_ms,
                        ..context.clone()
                    };
                    let context = &final_context;
                    let catalog = match policy.validate_dispatch(
                        intent,
                        reason_chain,
                        context,
                        self.owner_epoch,
                        ledger_high_watermark,
                        admission_duration,
                    ) {
                        Ok(catalog) => catalog,
                        Err(error) => {
                            let failure = if error.is_fatal() {
                                RequestFailure::poison_without_terminal(error)
                            } else {
                                RequestFailure::request(error, RuntimeReceiptState::Denied, None)
                            };
                            return CriticalActionReport::Failed {
                                error: failure,
                                effect: EffectDisposition::NotPerformed,
                            };
                        }
                    };
                    // Workflow #386 SR-4a2 (`scheduling-blackout.md`, "Order of the deferrals",
                    // positions 4 and 7, and "Complete silence"): a blackout, the quiet before one
                    // or the day boundary, judged again at this admission's own time, refuses at
                    // Info with the code the evaluation would have deferred with (no lease, no
                    // run). It comes after the intent's own checks and before the ceiling of its
                    // facts' freshness: a catalog `maintenance` event is a timeline boundary, so
                    // an intent evaluated before one is past that ceiling once the event starts,
                    // and its crossing must still be refused `blackout_active`, as for every
                    // source. Positions 4 and 7 also precede the remedy check (position 8a).
                    if let Err(error) = self.monotonic_ms().and_then(|monotonic_ms| {
                        policy.blackout_admission(
                            &blackout_context,
                            intent,
                            &current_facts.instances,
                            EvaluationTime {
                                unix_ms: context.now_unix_ms,
                                monotonic_ms,
                            },
                            response_deadline_ms,
                            &blackout_instance_id,
                        )
                    }) {
                        let failure = if error.is_fatal() {
                            RequestFailure::poison_without_terminal(error)
                        } else {
                            RequestFailure::request(error, RuntimeReceiptState::Denied, None)
                        };
                        return CriticalActionReport::Failed {
                            error: failure,
                            effect: EffectDisposition::NotPerformed,
                        };
                    }
                    // The intent's facts are still fresh at this admission's time
                    // (`policy_facts_stale` otherwise), judged after the blackout re-check above.
                    if let Err(error) = policy.validate_dispatch_freshness(intent, context) {
                        let failure = if error.is_fatal() {
                            RequestFailure::poison_without_terminal(error)
                        } else {
                            RequestFailure::request(error, RuntimeReceiptState::Denied, None)
                        };
                        return CriticalActionReport::Failed {
                            error: failure,
                            effect: EffectDisposition::NotPerformed,
                        };
                    }
                    // Workflow #411 B15 R4 (model-b15 §7.9): a remedy request or retry is
                    // admitted only while its round and layer are still open and unserved; else
                    // it is refused `policy_remedy_round_closed` (no lease, no run).
                    if let Some(remedy_context) = &remedy_context
                        && let Err(error) = policy.validate_remedy_dispatch(
                            intent,
                            reason_chain,
                            &current_facts,
                            remedy_context,
                            context.now_unix_ms,
                        )
                    {
                        let failure = if error.is_fatal() {
                            RequestFailure::poison_without_terminal(error)
                        } else {
                            RequestFailure::request(error, RuntimeReceiptState::Denied, None)
                        };
                        return CriticalActionReport::Failed {
                            error: failure,
                            effect: EffectDisposition::NotPerformed,
                        };
                    }
                    let admission_record = match policy.preview_admission(
                        intent,
                        context.now_unix_ms,
                        &suspension_lift,
                    ) {
                        Ok(record) => record,
                        Err(error) => {
                            let failure = if error.is_fatal() {
                                RequestFailure::poison_without_terminal(error)
                            } else {
                                RequestFailure::request(error, RuntimeReceiptState::Denied, None)
                            };
                            return CriticalActionReport::Failed {
                                error: failure,
                                effect: EffectDisposition::NotPerformed,
                            };
                        }
                    };
                    // Workflow #437 MAPP-2d: every dispatch, a member's of a device group of
                    // several included, is granted its run's lease (its response deadline and the
                    // heartbeat reserve); a member's switch runs inside it and its run renews it
                    // after the switch's startup run (`HostShared::switch_before_run`).
                    let lease_ttl_ms = task_request
                        .map(|task_request| {
                            task_request.validate().map_err(|_| {
                                RequestFailure::request(
                                    policy_admission_request(
                                        HostCode::PolicyTaskRequestInvalid,
                                        HostLocation::AdmitPolicyDispatch,
                                    ),
                                    RuntimeReceiptState::Denied,
                                    None,
                                )
                            })?;
                            self.contained_task_lease_ttl(task_request)
                        })
                        .transpose();
                    let lease_ttl_ms = match lease_ttl_ms {
                        Ok(ttl) => ttl,
                        Err(error) => {
                            return CriticalActionReport::Failed {
                                error,
                                effect: EffectDisposition::NotPerformed,
                            };
                        }
                    };
                    // Workflow #369 W-1: contention on the admission guard alone is not a
                    // hold; a retry that still finds it taken ends the intent as contended.
                    let admission = match self.try_acquire_lease(
                        RuntimeLeaseAcquisition {
                            request: &validated,
                            request_id: request.request_id(),
                            instance_alias: &intent.instance_id,
                            holder_id,
                            connection_id,
                            run_links: Some(run_links),
                            lease_ttl_ms,
                            kind: ClaimKind::PolicyDispatch,
                        },
                        POLICY_ADMISSION_CONTENTION_RETRY,
                    ) {
                        Ok(Some(success)) => Ok(success),
                        Ok(None) => {
                            match dispatch_contended_error(
                                context.now_unix_ms,
                                resolved.instance_id(),
                            ) {
                                Ok(error) => Err(RequestFailure::request(
                                    error,
                                    RuntimeReceiptState::Denied,
                                    None,
                                )),
                                Err(error) => Err(RequestFailure::poison_without_terminal(error)),
                            }
                        }
                        Err(failure) => Err(failure),
                    };
                    match admission {
                        Ok(success) => match success.result {
                            RuntimeResult::LeaseGranted { token } => {
                                #[cfg(test)]
                                policy_crash_test_barrier("after_lease_grant");
                                if let Err(error) =
                                    policy.commit_admission(intent, &admission_record, reason_chain)
                                {
                                    return CriticalActionReport::Failed {
                                        error: RequestFailure::poison_without_terminal(error),
                                        effect: EffectDisposition::Indeterminate,
                                    };
                                }
                                #[cfg(test)]
                                policy_crash_test_barrier("after_budget_commit");
                                CriticalActionReport::Succeeded {
                                    value: (token, catalog, admission_record),
                                    effect: DefiniteEffectDisposition::Performed,
                                }
                            }
                            _ => CriticalActionReport::Failed {
                                error: RequestFailure::poison_without_terminal(
                                    RuntimeHostError::fatal(
                                        HostCode::PolicyLeaseResultInvalid,
                                        HostLocation::AdmitPolicyDispatch,
                                        RuntimeErrorCode::RuntimeHalted,
                                    ),
                                ),
                                effect: EffectDisposition::Indeterminate,
                            },
                        },
                        Err(error) => {
                            let effect = if error.poison_runtime {
                                EffectDisposition::Indeterminate
                            } else {
                                EffectDisposition::NotPerformed
                            };
                            CriticalActionReport::Failed { error, effect }
                        }
                    }
                },
                |(_, _, admission), _| {
                    self.events
                        .draft(
                            EventSeverity::Info,
                            EventSource::Scheduler,
                            OriginModule::Policy,
                            EventActor::Scheduler,
                            success_links,
                            PolicyPayloadDraft::dispatch_admitted(
                                success_data,
                                admission.clone(),
                                AuditInput::new(),
                            ),
                        )
                        .map_err(|_| {
                            actingcommand_contract::SanitizationError::fingerprinter_failure()
                        })
                },
                |failure, effect| {
                    self.events
                        .draft(
                            policy_rejection_severity(failure.error.code()),
                            EventSource::Scheduler,
                            OriginModule::Policy,
                            EventActor::Scheduler,
                            failure_links,
                            PolicyPayloadDraft::dispatch_rejected_with_reason(
                                failure_data,
                                effect,
                                failure.error.policy_rejection(),
                                AuditInput::new(),
                            ),
                        )
                        .map_err(|_| {
                            actingcommand_contract::SanitizationError::fingerprinter_failure()
                        })
                },
            );
            // Workflow #191 U5-F1: still under the policy guard, the admission applies its own
            // intent and outcome; an append that returned an error re-aligns the projection
            // with the whole ledger. Consuming the appender first releases a fact write gate
            // an intent append failure left held.
            let appended = appender.into_appended();
            let applied = match &result {
                // Whole-event equality, so the sequence and the event id match too.
                Ok(receipt) => match appended.as_slice() {
                    [intent, outcome]
                        if intent == receipt.intent() && outcome == receipt.outcome() =>
                    {
                        policy.apply_admission_events(
                            receipt.intent(),
                            receipt.outcome(),
                            Some(&receipt.value().2),
                        )
                    }
                    _ => Err(policy_admission_fatal(
                        HostCode::PolicyAdmissionEventInvalid,
                        HostLocation::ApplyPolicyAdmissionEvents,
                    )),
                },
                Err(CriticalExecutionError::Action { outcome, .. }) => match appended.as_slice() {
                    [intent, appended_outcome] if appended_outcome == outcome.as_ref() => {
                        policy.apply_admission_events(intent, outcome, None)
                    }
                    _ => Err(policy_admission_fatal(
                        HostCode::PolicyAdmissionEventInvalid,
                        HostLocation::ApplyPolicyAdmissionEvents,
                    )),
                },
                Err(
                    CriticalExecutionError::IntentAppend(_)
                    | CriticalExecutionError::OutcomeUndurable { .. },
                ) => policy.refresh_dispatches(&self.ledger),
            };
            // Workflow #422 LN-3: the admission's changes of the decision memory, as value
            // records, still under the policy guard.
            let applied = applied.and_then(|()| self.record_policy_values(&mut policy));
            let applied = match applied {
                Ok(()) => Ok(()),
                Err(error) => {
                    let error = if error.is_fatal() {
                        error
                    } else {
                        error.into_fatal()
                    };
                    self.fatal.mark(error.clone())?;
                    Err(error)
                }
            };
            drop(policy);
            if let Err(applied) = applied {
                return Err(match result {
                    Err(CriticalExecutionError::Action { error, outcome, .. }) => {
                        rejection = Some((
                            outcome,
                            self.events.request_links(
                                &validated,
                                Some(resolved.instance_id()),
                                None,
                                None,
                            ),
                        ));
                        (*error.error).with_complete_failure(
                            crate::error::RuntimeFailureRelation::AdmissionRecord,
                            applied,
                        )
                    }
                    Err(error) => critical_execution_error(&error).with_complete_failure(
                        crate::error::RuntimeFailureRelation::AdmissionRecord,
                        applied,
                    ),
                    Ok(_) => applied,
                });
            }
            match result {
                Ok(receipt) => {
                    let started_at_monotonic_ms = self.monotonic_ms()?;
                    let (token, catalog, admission) = receipt.into_value();
                    let clock = PolicyDispatchClock::live(
                        admission.activity.admitted_at_unix_ms,
                        started_at_monotonic_ms,
                    );
                    if lock(
                        &self.policy_dispatch_clocks,
                        HostLocation::RecordPolicyDispatchStart,
                    )?
                    .insert(intent.decision_id.clone(), clock)
                    .is_some()
                    {
                        let error = policy_admission_fatal(
                            HostCode::PolicyDispatchClockIdentityConflict,
                            HostLocation::RecordPolicyDispatchStart,
                        );
                        self.fatal.mark(error.clone())?;
                        return Err(error);
                    }
                    Ok(PolicyDispatchAdmission::Granted {
                        context: Box::new(PolicyRunContext::new(
                            request,
                            correlation_id,
                            run_id,
                            task_id,
                            catalog,
                            token,
                            admission,
                            intent.clone(),
                            reason_chain.clone(),
                        )?),
                    })
                }
                Err(CriticalExecutionError::Action { error, outcome, .. }) => {
                    rejection = Some((
                        outcome,
                        self.events.request_links(
                            &validated,
                            Some(resolved.instance_id()),
                            None,
                            None,
                        ),
                    ));
                    if error.poison_runtime {
                        self.fatal.mark((*error.error).clone())?;
                    }
                    Err(*error.error)
                }
                Err(error) => {
                    let error = critical_execution_error(&error);
                    self.fatal.mark(error.clone())?;
                    Err(error)
                }
            }
        })();
        self.record_policy_admission_result(intent, result, rejection)
    }

    pub(super) fn record_policy_admission_result(
        &self,
        intent: &DispatchIntent,
        result: RuntimeHostResult<PolicyDispatchAdmission>,
        rejection: Option<(Box<PersistedEvent>, EventLinksDraft)>,
    ) -> RuntimeHostResult<PolicyDispatchAdmission> {
        match result {
            Err(error) if !error.is_fatal() || rejection.is_some() => {
                let recorded = match rejection {
                    // Workflow #369 W-1: a held instance is a wait, not a failure. The
                    // record of its key tokens (`holder_kind`, `count`) takes the Info
                    // severity of its rejection instead of Error (§5.10: no error point).
                    Some((outcome, links)) => self.record_required_failure_with_severity(
                        &error,
                        &outcome,
                        links,
                        (policy_rejection_severity(error.code()) == EventSeverity::Info)
                            .then_some(EventSeverity::Info),
                    ),
                    None => self.append_lifecycle_failure(
                        RuntimeLifecycleFailureStage::OperationCleanup,
                        RuntimeLifecycleFailure::PolicyAdmission {
                            error: &error,
                            decision_id: &intent.decision_id,
                        },
                        EventLinksDraft::default(),
                        None,
                    ),
                };
                match recorded {
                    Ok(()) => Err(error),
                    Err(writer) => {
                        let complete = error
                            .with_complete_failure(
                                crate::error::RuntimeFailureRelation::AdmissionRecord,
                                writer,
                            )
                            .into_fatal();
                        match self.fatal.mark(complete.clone()) {
                            Ok(()) => Err(complete),
                            Err(mark) => Err(complete.with_complete_failure(
                                crate::error::RuntimeFailureRelation::LifecycleRecord,
                                mark,
                            )),
                        }
                    }
                }
            }
            // Workflow #421 INST-2: the dispatches granted since the start, for `StartReport` (a
            // suppressed replay runs nothing).
            Ok(admission @ PolicyDispatchAdmission::Granted { .. }) => {
                self.count_start_dispatch(&intent.instance_id)?;
                Ok(admission)
            }
            result => result,
        }
    }

    /// `PauseScheduling` (Workflow #191 ps1, ps2). `Global` closes the dispatch gate of every
    /// instance and closes no device session. `Instance` closes the gate of one physical
    /// instance at once (stage `Draining`), drains its in-flight contained runs (stage
    /// `Paused`), waits until nothing uses the device (stage `Released`, ps2; the device session
    /// stays open, Workflow #191 H, except outside the multi-Nemu gate) and then answers; a
    /// failed stage lifts the gate again.
    ///
    /// #480 PG-2c (`contracts/scheduling-pause.md`, "Two modes"; the coordinator's rulings on
    /// board #480: a pause a person asked for closes the gate at once and stands until a person
    /// lifts it): `pause_mode` says what an instance pause does when something is still in the
    /// way. `quiet_or_lift` (a tool's hold) is the all-or-nothing of before 0.12.72 for every
    /// obstacle: past its grace it fails and is lifted, as at a shutdown. `stand` (a person's) is
    /// never lifted by the Runtime: at the end of its caller's time it answers Completed with
    /// `scheduling_pause_standing` (its stage and what is in the way) and a thread of the host
    /// carries its stages on (`carry_pause_on`); a shutdown before `released` answers
    /// `scheduling_pause_kept_at_shutdown`, a person's resume meanwhile
    /// `scheduling_pause_lifted_by_resume`, both informing; a fatal failure of a stage answers the
    /// failure and lifts nothing either. The global pause has no stages: the mode changes nothing.
    ///
    /// #480 PG-2c fix round 1 (the coordinator's ruling 4; #419: one instance's trouble does not
    /// take the Runtime down): everything that can refuse the request for its own input (its
    /// instance, its pause already held, its deadlines) is checked before the row is recorded, so a
    /// refused request never became a pause. After the row is recorded a `stand` pause's check that
    /// fails with a failure the decision point would neither latch nor fence is tried again
    /// (`release_paused_instance`, `check_failed`), and only a fatal failure or one that fences the
    /// instance reaches this request's answer.
    pub(super) fn pause_scheduling(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        scope: &SchedulingPauseScope,
        pause_reason: &SchedulingPauseReason,
        drain_timeout_ms: u64,
        pause_mode: SchedulingPauseMode,
    ) -> Result<OperationSuccess, RequestFailure> {
        let since_unix_ms = self
            .runtime_clock_sample()
            .map_err(RequestFailure::poison_without_terminal)?
            .unix_ms;
        // Workflow #361 B1: the persist gate orders the record of the pauses a change leads to
        // before the change itself; a failed record leaves the gate as it was.
        let origin = PauseOrigin::set_in(self.owner_epoch)?;
        let (instance_alias, instance_id, revision, deadlines, links) = match scope {
            SchedulingPauseScope::Global => {
                let _persist = lock(
                    &self.scheduling_pause_persist_gate,
                    HostLocation::PersistGlobalSchedulingPause,
                )?;
                let mut prospective =
                    lock(&self.scheduling_pause, HostLocation::PauseGlobalScheduling)?.clone();
                if prospective.global.is_some() {
                    return Err(scheduling_pause_denied(
                        HostCode::SchedulingAlreadyPaused,
                        ContractLocation::PauseScheduling,
                    ));
                }
                let revision = next_scheduling_pause_revision(&mut prospective.global_revision)?;
                let state = SchedulingPauseState {
                    revision,
                    pause_reason: pause_reason.clone(),
                    since_unix_ms,
                };
                prospective.set_global(state.clone(), origin.clone());
                self.persist_scheduling_pauses(&prospective)?;
                lock(&self.scheduling_pause, HostLocation::PauseGlobalScheduling)?
                    .set_global(state, origin);
                return Ok(OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: None,
                    result: RuntimeResult::SchedulingPaused {
                        scope: scope.clone(),
                        revision,
                        drained: None,
                    },
                    failure_outcome: None,
                });
            }
            SchedulingPauseScope::Instance { instance_alias } => {
                let instance_id = self.resolve_instance(instance_alias)?.instance_id();
                // #480 PG-2c fix round 1: what can refuse the request for its own input is checked
                // before its row is recorded; its deadlines too.
                let deadlines = self.scheduling_pause_deadlines(drain_timeout_ms)?;
                let _persist = lock(
                    &self.scheduling_pause_persist_gate,
                    HostLocation::PersistInstanceSchedulingPause,
                )?;
                let mut prospective = lock(
                    &self.scheduling_pause,
                    HostLocation::PauseInstanceScheduling,
                )?
                .clone();
                if prospective.instances.contains_key(instance_alias) {
                    return Err(scheduling_pause_denied(
                        HostCode::SchedulingAlreadyPaused,
                        ContractLocation::PauseScheduling,
                    ));
                }
                let revision = next_scheduling_pause_revision(
                    prospective
                        .instance_revisions
                        .entry(instance_alias.clone())
                        .or_default(),
                )?;
                let state = InstancePauseState {
                    revision,
                    pause_reason: pause_reason.clone(),
                    since_unix_ms,
                    stage: InstancePauseStage::Draining,
                    pause_mode: None,
                    in_the_way: Vec::new(),
                };
                prospective.set_instance(instance_alias, state.clone(), origin.clone());
                self.persist_scheduling_pauses(&prospective)?;
                let links = self
                    .events
                    .request_links(request, Some(instance_id), None, None);
                let mut table = lock(
                    &self.scheduling_pause,
                    HostLocation::PauseInstanceScheduling,
                )?;
                table.set_instance(instance_alias, state, origin);
                // #480 PG-2c: the mode lives in memory only; a restored pause is `released`.
                table.modes.insert(instance_alias.clone(), pause_mode);
                // #480 PG-2d: and so do this request's links, which every record of the pause's
                // life carries, one a person's resume writes included.
                table
                    .pause_links
                    .insert(instance_alias.clone(), links.clone());
                drop(table);
                (instance_alias, instance_id, revision, deadlines, links)
            }
        };
        let staged = self.run_instance_pause_stages(
            PausedInstance {
                alias: instance_alias,
                instance_id,
                revision,
                mode: pause_mode,
            },
            InstancePauseStage::Draining,
            links.clone(),
            StagesBound::Caller(deadlines),
        );
        let end = match (staged, pause_mode) {
            (Ok(end), _) => end,
            // No half-open pause: a failed stage of a tool's hold lifts the gate it closed, and the
            // persisted pause with it (Workflow #361 B1); a failed record poisons the request.
            // #480 PG-2d: a hold whose caller's time ended before its instance was quiet (marked
            // where that failure arose, `mark_not_quiet`) writes, once lifted, the record
            // `scheduling_pause_lifted_not_quiet`, read before the lift ends the pause's state. A
            // read of the table that fails here fails the lift too, which then poisons the request.
            // Fix round 1 (the coordinator's ruling: every lift of a hold without quiet is
            // recorded): so does a hold whose hand-back met a refusing queued entry or whose close
            // lease was refused, marked there too. One record per lift: a hold's stages run on this
            // request's thread alone (no continuation carries a hold on), each place that marks it
            // returns its failure at once, the mark is read here once, and the lift ends it with
            // the pause's state (`lift_instance`).
            (Err(failure), SchedulingPauseMode::QuietOrLift) => {
                let not_quiet = lock(
                    &self.scheduling_pause,
                    HostLocation::LiftFailedInstancePause,
                )
                .map(|table| table.not_quiet_in_the_way(instance_alias));
                return match self.lift_failed_instance_pause(instance_alias) {
                    Ok(()) => {
                        if let Ok(Some(in_the_way)) = not_quiet {
                            self.record_pause_moment(
                                Ok(links),
                                Outcome::new(HostCode::SchedulingPauseLiftedNotQuiet)
                                    .pause_reason(pause_reason.as_str())
                                    .in_the_way(in_the_way)
                                    .caused_by(failure.error.outcome()),
                            );
                        }
                        Err(failure)
                    }
                    Err(error) => Err(failure.replace_with_poison(error)),
                };
            }
            // #480 PG-2c: a person's pause is never lifted by the Runtime, an error of its own
            // included. What is left of a stage the Runtime could not run (a fatal failure, which
            // latches the Runtime or fences the instance) is the answer; the gate stays closed and
            // the persisted pause names the instance, so the next start restores it. Fix round 1:
            // a failure that would only fail the operation never comes here: the hand-back tries
            // its check again (`check_failed`), and the drain and the stage moves fail only fatally.
            // Fix round 2 (item 7): one way for both threads. A failure that is not fatal and
            // concerns the instance goes to the one decision point here, as a continuation's does
            // (`carry_on_instance_pause`): the instance is fenced and the request fails with the
            // fence's failure, nothing lifted; a fatal one the request door contains as before.
            (Err(failure), SchedulingPauseMode::Stand) => {
                if failure.poison_runtime
                    || !matches!(
                        failure.error.scope_at(Some(instance_id)),
                        crate::FailureScope::Instance(_)
                    )
                {
                    return Err(failure);
                }
                return Err(
                    match self.contain_failure(
                        (*failure.error).clone(),
                        Some(instance_id),
                        links,
                    )? {
                        Contained::Fenced(fence) | Contained::Failed(fence) => {
                            failure.fenced(fence)
                        }
                        Contained::Latched(error) => RequestFailure {
                            error: Box::new(error),
                            poison_runtime: true,
                            ..failure
                        },
                    },
                );
            }
        };
        let drained = match &end {
            StagesEnd::Released(drained)
            | StagesEnd::KeptAtShutdown { drained, .. }
            | StagesEnd::Standing { drained, .. }
            | StagesEnd::Resumed { drained, .. } => *drained,
        };
        let cause = stages_cause(&end);
        // #480 PG-2d (`contracts/scheduling-pause.md`, "A pause's life in the ledger"): the caller's
        // time ended with something in the way (`scheduling_pause_standing`), or a shutdown kept the
        // pause (`scheduling_pause_kept_at_shutdown`): one record of the receipt's own cause, written
        // before the continuation starts. A pause quiet within the caller's time writes none, and a
        // resume's lift is the resume's. Fix round 1 (the review of #1834, its first remark): a
        // person's resume can lift the pause after these stages ended and before this record; the
        // record is therefore decided under the persist gate, which orders it against the resume's
        // record, and written only while the pause still holds (`record_held_pause_moment`).
        if matches!(
            end,
            StagesEnd::Standing { .. } | StagesEnd::KeptAtShutdown { .. }
        ) && let Some(cause) = &cause
        {
            // Tests: the stages ended, before the persist gate.
            #[cfg(test)]
            self.before_held_pause_record_for_test()?;
            self.record_held_pause_moment(instance_alias, revision, links.clone(), cause.clone());
        }
        if let StagesEnd::Standing {
            stage,
            carry_on: true,
            ..
        } = &end
        {
            self.carry_pause_on(
                PausedInstance {
                    alias: instance_alias,
                    instance_id,
                    revision,
                    mode: pause_mode,
                },
                *stage,
                links,
            )?;
        }
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::SchedulingPaused {
                scope: scope.clone(),
                revision,
                drained: Some(drained),
            },
            failure_outcome: cause,
        })
    }

    /// #480 PG-2d (`contracts/scheduling-pause.md`, "A pause's life in the ledger"): one record of a
    /// moment of an instance pause's life: a `runtime.lifecycle_observed` `device_state_observed`
    /// record carrying `outcome` under `links` (the pause request's, which name the instance), its
    /// severity the outcome's category. A record that cannot be written (`links` not built, or the
    /// append failed) changes nothing the pause does: the failure is said in the daemon's log
    /// (`FatalState::report_skipped`, the line a skipped background step gets), the record it could
    /// not write linked to it; a write the ledger gave up on stops the Runtime by its own path, as
    /// for every append.
    fn record_pause_moment(&self, links: RuntimeHostResult<EventLinksDraft>, outcome: Outcome) {
        let appended = links.and_then(|links| {
            self.append_event_raw(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                links,
                RuntimePayloadDraft::lifecycle_observed(
                    self.owner_epoch,
                    RuntimeLifecyclePhase::DeviceStateObserved {
                        outcome: outcome.clone(),
                    },
                    AuditInput::new(),
                ),
            )
        });
        if let Err(error) = appended {
            self.fatal
                .report_skipped(&error.outcome().link(CauseRelation::Related, outcome));
        }
    }

    /// #480 PG-2d fix round 1 (the review of #1834, its first remark): the record of a moment that
    /// says the instance pause of `instance_alias` at `revision` still holds, written by its own
    /// request (`scheduling_pause_standing`, `scheduling_pause_kept_at_shutdown`) and, since #480
    /// PG-2e (the review's leftover), by its continuation (`scheduling_pause_settled`,
    /// `scheduling_pause_kept_at_shutdown`), which holds no lock when it calls. The persist gate
    /// orders it against the record of a person's resume, which the resume writes with its lift
    /// under the same gate (`resume_unquiet_instance_pause`): under it, the pause still held at its
    /// revision is recorded ([`Self::record_pause_moment`]), and a pause a resume lifted first (or
    /// held again, at another revision) writes nothing, the resume's records saying what happened.
    /// The gate is taken with nothing else held, then the pause table's lock inside it, the order
    /// every pause and resume takes them in; nothing the pause does depends on the record, and a
    /// gate or a table that cannot be read is a record that cannot be written (said in the daemon's
    /// log by `record_pause_moment`).
    fn record_held_pause_moment(
        &self,
        instance_alias: &str,
        revision: u64,
        links: EventLinksDraft,
        outcome: Outcome,
    ) {
        let held = lock(
            &self.scheduling_pause_persist_gate,
            HostLocation::PersistInstanceSchedulingPause,
        )
        .and_then(|persist| {
            let held = lock(
                &self.scheduling_pause,
                HostLocation::PauseInstanceScheduling,
            )?
            .holds_instance_at(instance_alias, revision);
            Ok((persist, held))
        });
        match held {
            Ok((_persist, true)) => self.record_pause_moment(Ok(links), outcome),
            // Lifted first: the resume's records say what happened.
            Ok((_persist, false)) => {}
            Err(error) => self.record_pause_moment(Err(error), outcome),
        }
    }

    /// #480 PG-2d: the links of a record of the pause of `instance_id` written by another request
    /// than the pause's: the pause request's, kept with the pause (`kept`); else (no pause a
    /// resume lifts before it is quiet lacks them) the instance under fresh system links, as the
    /// emulator watch links its records.
    fn pause_record_links(
        &self,
        kept: Option<EventLinksDraft>,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<EventLinksDraft> {
        match kept {
            Some(links) => Ok(links),
            None => self.events.system_links().map(|links| {
                links.with_instance_id(self.events.issuer().issue_registered_instance(instance_id))
            }),
        }
    }

    /// #480 PG-2c: the stages of the instance pause `paused`, from `from` (`draining`, or `paused`
    /// for a continuation the request left there) until they end (`StagesEnd`) or fail, within
    /// `bound`. The drain (b) and the hand-back (c) as before; a `stand` pause's stages end, short
    /// of a failure, at a shutdown (kept), at the end of its caller's time (standing) or at a
    /// person's resume (lifted), each in the stage it had reached; a `quiet_or_lift` pause's are
    /// released or fail as before 0.12.72.
    fn run_instance_pause_stages(
        &self,
        paused: PausedInstance<'_>,
        from: InstancePauseStage,
        links: EventLinksDraft,
        bound: StagesBound,
    ) -> Result<StagesEnd, RequestFailure> {
        let stand = paused.mode == SchedulingPauseMode::Stand;
        // A stand pause's stages watch whether a person's resume lifted it.
        let watched = stand.then_some((paused.alias, paused.revision));
        let mut drained = SchedulingDrainSummary {
            finished: 0,
            cancelled: 0,
        };
        // Workflow #479 F-2 fix round 2, the coordinator's follow-up (`contracts/device-binding.md`,
        // "An iOS handset"): an instance whose device system has no backend in this build is inert:
        // nothing can run, press, capture or hold a session on it, so there is nothing to drain,
        // to wait for (a lease, a queued entry, a run in flight or its admission guard: none can
        // reach its device) or to hand back and close. Its pause is quiet at once, in both modes,
        // its stages moved to `released` here; no stage can fail for it, so a tool's hold over
        // several instances is never lifted because of it.
        if self.without_device_backend(paused.instance_id)? {
            if from == InstancePauseStage::Draining
                && !self.advance_instance_pause(
                    paused,
                    InstancePauseStage::Draining,
                    InstancePauseStage::Paused,
                )?
            {
                return Ok(StagesEnd::Resumed {
                    stage: InstancePauseStage::Draining,
                    drained,
                });
            }
            if !self.advance_instance_pause(
                paused,
                InstancePauseStage::Paused,
                InstancePauseStage::Released,
            )? {
                return Ok(StagesEnd::Resumed {
                    stage: InstancePauseStage::Paused,
                    drained,
                });
            }
            return Ok(StagesEnd::Released(drained));
        }
        if from == InstancePauseStage::Draining {
            // #480 PG-2e: the drain notes what it reads in the pause's way at every poll.
            match self.drain_contained_runs_for_pause(
                paused.instance_id,
                bound.stop_runs_at(),
                bound.grace(),
                watched,
                Some((paused.alias, paused.revision)),
            )? {
                PauseDrainEnd::Drained(summary) => drained = summary,
                PauseDrainEnd::Interrupted(summary) if stand => {
                    return Ok(StagesEnd::KeptAtShutdown {
                        stage: InstancePauseStage::Draining,
                        drained: summary,
                    });
                }
                PauseDrainEnd::Interrupted(_) => return Err(pause_drain_interrupted()),
                PauseDrainEnd::PastGrace(summary) if stand => {
                    let in_the_way = vec![PauseObstacle::ContainedRun];
                    self.note_in_the_way(paused, &in_the_way)?;
                    return Ok(StagesEnd::Standing {
                        stage: InstancePauseStage::Draining,
                        in_the_way,
                        carry_on: true,
                        failure: None,
                        drained: summary,
                    });
                }
                PauseDrainEnd::PastGrace(_) => {
                    // #480 PG-2d: a tool's hold not quiet in its caller's time.
                    self.mark_not_quiet(paused, Some(vec![PauseObstacle::ContainedRun]))?;
                    return Err(pause_drain_incomplete());
                }
                PauseDrainEnd::Lifted(summary) => {
                    return Ok(StagesEnd::Resumed {
                        stage: InstancePauseStage::Draining,
                        drained: summary,
                    });
                }
            }
            if !self.advance_instance_pause(
                paused,
                InstancePauseStage::Draining,
                InstancePauseStage::Paused,
            )? {
                return Ok(StagesEnd::Resumed {
                    stage: InstancePauseStage::Draining,
                    drained,
                });
            }
        }
        match self.release_paused_instance(paused, links, bound, &mut drained)? {
            ReleaseEnd::Released => {
                if !self.advance_instance_pause(
                    paused,
                    InstancePauseStage::Paused,
                    InstancePauseStage::Released,
                )? {
                    return Ok(StagesEnd::Resumed {
                        stage: InstancePauseStage::Paused,
                        drained,
                    });
                }
                Ok(StagesEnd::Released(drained))
            }
            ReleaseEnd::KeptAtShutdown => Ok(StagesEnd::KeptAtShutdown {
                stage: InstancePauseStage::Paused,
                drained,
            }),
            ReleaseEnd::Standing {
                in_the_way,
                carry_on,
                failure,
            } => Ok(StagesEnd::Standing {
                stage: InstancePauseStage::Paused,
                in_the_way,
                carry_on,
                failure,
                drained,
            }),
            ReleaseEnd::Lifted => Ok(StagesEnd::Resumed {
                stage: InstancePauseStage::Paused,
                drained,
            }),
        }
    }

    /// Moves the pause `paused` from stage `from` to `to`. A `quiet_or_lift` pause that is no
    /// longer there in `from` was lost (fatal, as before: it cannot be resumed before
    /// `released`); a `stand` pause may have been lifted by a person's resume meanwhile (#480
    /// PG-2c): `false`, nothing moved.
    fn advance_instance_pause(
        &self,
        paused: PausedInstance<'_>,
        from: InstancePauseStage,
        to: InstancePauseStage,
    ) -> Result<bool, RequestFailure> {
        let location = if to == InstancePauseStage::Paused {
            HostLocation::FinishInstancePauseDrain
        } else {
            HostLocation::FinishInstancePauseRelease
        };
        let mut table = lock(&self.scheduling_pause, location)?;
        if paused.mode == SchedulingPauseMode::Stand
            && !table.holds_instance_at(paused.alias, paused.revision)
        {
            return Ok(false);
        }
        table.advance_instance(paused.alias, paused.revision, from, to)?;
        Ok(true)
    }

    /// #480 PG-2c: whether the instance pause of `alias` at `revision` is still held (a person's
    /// resume lifts a `stand` pause in every stage).
    pub(super) fn instance_pause_held(
        &self,
        alias: &str,
        revision: u64,
    ) -> RuntimeHostResult<bool> {
        Ok(lock(
            &self.scheduling_pause,
            HostLocation::ReadSchedulingPauseTable,
        )?
        .holds_instance_at(alias, revision))
    }

    /// #480 PG-2c: a standing pause's request answered; a thread of the host carries the rest of
    /// its stages on from `stage`, in memory only (`run_instance_pause_stages` with no bound):
    /// until `released`, a person's resume, an unconfirmed close (not tried again) or the
    /// shutdown, which joins it (`join_pause_carry_ons`) and keeps the pause. A failure of the
    /// Runtime there goes to the one decision point as a request's does (`contain_failure`): it
    /// fences the instance or latches the Runtime, and lifts nothing. A thread that cannot be
    /// started is fatal (`scheduling_pause_carry_on_spawn_failed`). Fix round 1 (item 5): the
    /// continuations that have ended are joined first (`reap_pause_carry_ons`), so the list holds
    /// only the running ones.
    fn carry_pause_on(
        &self,
        paused: PausedInstance<'_>,
        stage: InstancePauseStage,
        links: EventLinksDraft,
    ) -> Result<(), RequestFailure> {
        self.reap_pause_carry_ons()?;
        let spawn_failed = || {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                HostCode::SchedulingPauseCarryOnSpawnFailed,
                HostLocation::ReleasePausedInstance,
                RuntimeErrorCode::RuntimeHalted,
            ))
        };
        let shared = self
            .own_handle
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(spawn_failed)?;
        let alias = paused.alias.to_owned();
        let (instance_id, revision, mode) = (paused.instance_id, paused.revision, paused.mode);
        let thread = thread::Builder::new()
            .name(format!("actingd-pause-{alias}"))
            .spawn(move || {
                shared.carry_on_instance_pause(
                    PausedInstance {
                        alias: &alias,
                        instance_id,
                        revision,
                        mode,
                    },
                    stage,
                    links,
                )
            })
            .map_err(|_| spawn_failed())?;
        lock(&self.pause_carry_ons, HostLocation::ReleasePausedInstance)?.push(thread);
        Ok(())
    }

    /// #480 PG-2c: the thread root of `carry_pause_on`. Fix round 2 (item 8): a panic of the
    /// continuation is caught here, where the thread runs, as the instance worker's is
    /// (`instance_worker_loop`), and seen at once: `runtime_thread_panicked`, fatal, latched now, as
    /// the worker's unrecovered panic is (before this round it was seen only at the thread's join).
    fn carry_on_instance_pause(
        &self,
        paused: PausedInstance<'_>,
        stage: InstancePauseStage,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<()> {
        let staged = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.run_instance_pause_stages(paused, stage, links.clone(), StagesBound::Unbounded)
        }));
        let failure = match staged {
            // #480 PG-2d (`contracts/scheduling-pause.md`, "A pause's life in the ledger"): the
            // standing pause became quiet by itself (`scheduling_pause_settled`), or a shutdown kept
            // it (`scheduling_pause_kept_at_shutdown`, as its request would have answered): one
            // record each, under the request's links. A person's resume wrote its own; an
            // unconfirmed close stands with the fence's own records.
            Ok(Ok(end)) => {
                let moment = match &end {
                    StagesEnd::Released(_) => Some(Outcome::new(HostCode::SchedulingPauseSettled)),
                    StagesEnd::KeptAtShutdown { .. } => stages_cause(&end),
                    StagesEnd::Standing { .. } | StagesEnd::Resumed { .. } => None,
                };
                // #480 PG-2e (PG-2d's leftover, the review of #1834): written as the request writes
                // its own, decided under the persist gate and only while the pause still holds at
                // its revision (`record_held_pause_moment`), so a lift that took the gate first
                // leaves the record unwritten and no record of the resume comes before it.
                if let Some(outcome) = moment {
                    // Tests: the continuation's stages ended, before the persist gate.
                    #[cfg(test)]
                    self.before_held_pause_record_for_test()?;
                    self.record_held_pause_moment(paused.alias, paused.revision, links, outcome);
                }
                return Ok(());
            }
            Ok(Err(failure)) => failure,
            Err(_) => {
                let error = RuntimeHostError::fatal(
                    HostCode::RuntimeThreadPanicked,
                    HostLocation::ReleasePausedInstance,
                    RuntimeErrorCode::RuntimeHalted,
                );
                self.fatal.mark(error.clone())?;
                return Err(error);
            }
        };
        match self.contain_failure(*failure.error, Some(paused.instance_id), links)? {
            Contained::Fenced(_) => Ok(()),
            Contained::Latched(error) => Err(error),
            // Fix round 1 (the coordinator's ruling 4): a failure the decision point would end
            // only the operation with is the one `tried_again` takes, and the hand-back tries its
            // check again instead of returning it (`check_failed`); the drain and the stage moves
            // fail only fatally. One that still came here would be a broken rule of this module:
            // nothing answers it, so it is latched, never left unseen.
            Contained::Failed(error) => {
                let error = error.into_fatal();
                self.fatal.mark(error.clone())?;
                Err(error)
            }
        }
    }

    /// #480 PG-2c fix round 1 (item 5): joins the continuations that have ended, before the next
    /// one starts. A continuation's own failure was handled where it arose (fenced, or latched:
    /// `carry_on_instance_pause`), so its result is latched again only as the Runtime keeps its
    /// first condition; a panic is `runtime_thread_panicked`, fatal, as every Runtime thread's is
    /// at its join (`join_runtime_thread`; fix round 2: a continuation's own stages catch theirs
    /// where the thread runs and latch it at once, so only a panic outside them is first seen
    /// here). Either fails this request.
    fn reap_pause_carry_ons(&self) -> Result<(), RequestFailure> {
        let ended = {
            let mut threads = lock(&self.pause_carry_ons, HostLocation::ReleasePausedInstance)?;
            let (ended, running): (Vec<_>, Vec<_>) = std::mem::take(&mut *threads)
                .into_iter()
                .partition(JoinHandle::is_finished);
            *threads = running;
            ended
        };
        let mut failure = None;
        for thread in ended {
            if let Err(error) =
                join_runtime_thread(Some(thread), HostLocation::ReleasePausedInstance)
                && failure.is_none()
            {
                failure = Some(error);
            }
        }
        let Some(error) = failure else {
            return Ok(());
        };
        self.fatal.mark(error.clone())?;
        Err(RequestFailure::poison_without_terminal(error))
    }

    /// #480 PG-2c: joins the threads that carry standing pauses on; each ends with the shutdown
    /// request. The first failure is returned, every thread joined.
    pub(super) fn join_pause_carry_ons(&self) -> RuntimeHostResult<()> {
        let threads = std::mem::take(&mut *lock(
            &self.pause_carry_ons,
            HostLocation::ReleasePausedInstance,
        )?);
        let mut failure = None;
        for thread in threads {
            if let Err(error) =
                join_runtime_thread(Some(thread), HostLocation::ReleasePausedInstance)
                && failure.is_none()
            {
                failure = Some(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    /// The two deadlines of an instance pause that starts now (`SchedulingPauseDeadlines::after`).
    /// #480 PG-1, test builds: both come `scheduling_pause_deadlines_for_test` after the start when
    /// a test set it.
    fn scheduling_pause_deadlines(
        &self,
        drain_timeout_ms: u64,
    ) -> Result<SchedulingPauseDeadlines, RequestFailure> {
        #[cfg(test)]
        if let Some(after) = self.scheduling_pause_deadlines_for_test {
            let at = Instant::now() + after;
            return Ok(SchedulingPauseDeadlines {
                drain: at,
                grace: at,
            });
        }
        SchedulingPauseDeadlines::after(drain_timeout_ms)
    }

    /// #480 PG-2c fix round 1: how long a `stand` pause waits before it tries a refused session
    /// close or a failed check again (`SCHEDULING_PAUSE_RETRY_INTERVAL`); test builds: the
    /// interval a test set (`scheduling_pause_retry_for_test`).
    fn scheduling_pause_retry_interval(&self) -> Duration {
        #[cfg(test)]
        if let Some(interval) = self.scheduling_pause_retry_for_test {
            return interval;
        }
        SCHEDULING_PAUSE_RETRY_INTERVAL
    }

    /// #480 PG-2c fix round 1: what the last check of the `stand` pause `paused` found in its way,
    /// kept in memory with the pause (`SchedulingPauseTable::note_in_the_way`). #480 PG-2d: of a
    /// `quiet_or_lift` pause too, which the record of its lift reads when its caller's time ends
    /// before it is quiet ([`Self::mark_not_quiet`]); it decides nothing for that pause.
    fn note_in_the_way(
        &self,
        paused: PausedInstance<'_>,
        in_the_way: &[PauseObstacle],
    ) -> Result<(), RequestFailure> {
        lock(&self.scheduling_pause, HostLocation::ReleasePausedInstance)?.note_in_the_way(
            paused.alias,
            paused.revision,
            in_the_way.to_vec(),
        );
        Ok(())
    }

    /// #480 PG-2e (`contracts/scheduling-pause.md`, "Status"): what one poll of stage (b)'s drain
    /// of the instance pause of `alias` at `revision` read in its way, kept with the pause as a
    /// check of the hand-back keeps what it reads, so that the status answers it: a run of the
    /// instance in flight (`contained_run`), or nothing. Both modes; it decides nothing. The drain
    /// that the hand-back runs again notes nothing: the hand-back's check noted, just before it,
    /// everything it read, the run among it.
    pub(super) fn note_pause_drain(
        &self,
        alias: &str,
        revision: u64,
        runs_in_flight: bool,
    ) -> Result<(), RequestFailure> {
        let words = if runs_in_flight {
            vec![PauseObstacle::ContainedRun]
        } else {
            Vec::new()
        };
        lock(
            &self.scheduling_pause,
            HostLocation::FinishInstancePauseDrain,
        )?
        .note_in_the_way(alias, revision, words);
        Ok(())
    }

    /// #480 PG-2d: the `quiet_or_lift` pause `paused` was not quiet by the end of its caller's
    /// time and fails for it: what was in the way (`in_the_way`; `None`: what its last check
    /// noted) is kept, marked, for the record its lift writes (`scheduling_pause_lifted_not_quiet`,
    /// `pause_scheduling`). Called where that failure arises, so its record is decided by where
    /// the hold failed, never by the failure's code. Fix round 1 (the coordinator's ruling: every
    /// lift of a hold without quiet is recorded): also where a refusing queued entry or a refused
    /// close lease fails it (`hand_back_check`); a hold that fails for anything else (an
    /// unconfirmed or otherwise failed close, a failed check, a shutdown, a fatal error) is not
    /// marked.
    fn mark_not_quiet(
        &self,
        paused: PausedInstance<'_>,
        in_the_way: Option<Vec<PauseObstacle>>,
    ) -> Result<(), RequestFailure> {
        lock(&self.scheduling_pause, HostLocation::ReleasePausedInstance)?.mark_not_quiet(
            paused.alias,
            paused.revision,
            in_the_way,
        );
        Ok(())
    }

    /// #480 PG-2c fix round 1 (the coordinator's ruling 4): a check of the `stand` pause `paused`
    /// failed with `error`, a failure `tried_again` takes: what is in the way is `check_failed`
    /// (kept in memory with the pause), and the daemon's log says the failure once per pause and
    /// code, as a skipped background step is said, not at every try. No ledger record.
    fn note_check_failed(
        &self,
        paused: PausedInstance<'_>,
        error: &RuntimeHostError,
    ) -> Result<(), RequestFailure> {
        let say = {
            let mut table = lock(&self.scheduling_pause, HostLocation::ReleasePausedInstance)?;
            table.note_in_the_way(
                paused.alias,
                paused.revision,
                vec![PauseObstacle::CheckFailed],
            );
            table.first_saying(paused.alias, paused.revision, error.code())
        };
        if say {
            self.fatal.report_skipped(&error.outcome());
        }
        // Tests: the check failed and its word is noted.
        #[cfg(test)]
        self.pause_check_failed_for_test()?;
        Ok(())
    }

    /// #480 PG-2c fix round 1 (b): the hand-back of the `stand` pause `paused`, its admission
    /// guard in hand, about to close the session: under the pause table's lock, whether the pause
    /// is still held at its revision, and the mark that its close has begun
    /// (`SchedulingPauseTable::begin_hand_back_close`).
    fn begin_hand_back_close(
        &self,
        paused: PausedInstance<'_>,
    ) -> Result<CloseStep, RequestFailure> {
        Ok(
            lock(&self.scheduling_pause, HostLocation::ReleasePausedInstance)?
                .begin_hand_back_close(paused.alias, paused.revision),
        )
    }

    /// Workflow #191 ps2 (c): waits until the paused instance's device is idle. Under the
    /// instance admission guard it re-checks that nothing still uses the device: a queued entry
    /// the pause does not hold back fails at once (`TransferNotSafe`; Workflow #369 §5.1, review
    /// M-2: an operator entry; neither a gated routine entry, the startup claim included since
    /// #369 S6b, nor a holding ladder's continuation, whose ladder is the lease waited
    /// for below); a contained run that started after (b) (a
    /// policy dispatch admitted before the gate closed) is drained again under the pause's
    /// deadlines; the reset a client owes after its cancelled run (`SafeReset`) and an active
    /// lease are waited for until the grace deadline (`scheduling_pause_release_busy`). #480 PG-1
    /// and PG-2b waited for the Runtime's own automatic emulator start and stuck-recovery ladder
    /// past the grace for every requester; #480 PG-2c (below) moves that wait to a person's pause
    /// and widens it to everything. A ladder's claim still queued is a routine entry the pause
    /// holds back, which refuses nothing, and the instance's own queued autostart claim refuses
    /// nothing either (`AutostartEndsAtGrant`: granted under the pause, it ends at its first tick
    /// without a Start), here and at every grant of a dedicated instance lease. The gate stays
    /// closed throughout. The
    /// device session then stays open (Workflow #191 H: a pause is not a disconnect). Only an
    /// instance outside the multi-Nemu gate (`keeps_device_session`) closes it, through
    /// `close_retained_instance_while_guarded` without reusing a lease: the dedicated close
    /// lease is released with `LeaseReleaseReason::InstancePaused`; with no device session open
    /// it closes nothing. #480 PG-2b: that close lease is granted past the instance's own entries
    /// this pause holds back (its routine entries: a startup package, a ladder's claim), which
    /// cannot be granted while the pause holds and are granted in their place after the resume
    /// (`grant_dedicated_instance_lease`, `actingcommand_scheduler::PauseHandBack`); fix round 1:
    /// this hand-back says so itself (`close_retained_instance_with`, `hand_back`).
    ///
    /// #480 PG-2c (`contracts/scheduling-pause.md`, "Two modes"; it replaces PG-1's R4′ and R4″
    /// and PG-2b's wait for the ladder, which every requester had): a `quiet_or_lift` pause (a
    /// tool's hold) is all or nothing within its caller's time, for every obstacle, the Runtime's
    /// own included: a refusing queued entry fails it at once, anything still in the way past the
    /// grace fails it (`scheduling_pause_release_busy`), a shutdown fails it
    /// (`scheduling_pause_release_interrupted`), and so do a refused or an unconfirmed close; each
    /// failure lifts it. A `stand` pause (a person's) fails for none of these: a refusing queued
    /// entry is waited for as a lease is; a refused close lease (`close_refused`) is tried again;
    /// an unconfirmed close fenced the instance (`close_unconfirmed`) and is not tried again; on
    /// its request's thread the wait for the instance's admission guard and every other wait end
    /// with the caller's time (`ReleaseEnd::Standing`, what is in the way read then), and its
    /// continuation waits for everything with no bound; a shutdown keeps it (`KeptAtShutdown`)
    /// unless nothing is in the way of an instance inside the multi-Nemu gate, which ends as
    /// released (PG-1 fix round 3); a person's resume ends it (`Lifted`).
    ///
    /// #480 PG-2c fix round 1 (`hand_back_check`): no wait for the admission guard is blind, a
    /// continuation's included; the close and a person's resume decide under the pause table's
    /// lock (`begin_hand_back_close`, `SchedulingPauseTable::begin_resume_lift`), so a resumed
    /// instance keeps its session or gets it back from the resume's reconnect; a `stand` pause's
    /// check that fails with a failure `tried_again` takes is tried again (`check_failed`), as a
    /// refused close lease is, at most once per `SCHEDULING_PAUSE_RETRY_INTERVAL`. #480 PG-2d (the
    /// review of PG-2c's last round): after a failed check the whole check (`hand_back_check`)
    /// rests until `check_again_at`, so what is in the way is not read meanwhile; only the loop's
    /// top checks (fatal, lifted, shutdown) and the end of the caller's time keep the
    /// `SCHEDULING_PAUSE_DRAIN_POLL_INTERVAL` poll. A refused close rests only the close
    /// (`close_again_at`): the check still runs at every poll and reads what is in the way.
    fn release_paused_instance(
        &self,
        paused: PausedInstance<'_>,
        links: EventLinksDraft,
        bound: StagesBound,
        drained: &mut SchedulingDrainSummary,
    ) -> Result<ReleaseEnd, RequestFailure> {
        let stand = paused.mode == SchedulingPauseMode::Stand;
        // A policy admission that read the gate before (a) holds this gate until its lease is
        // granted or refused; passing it once makes that lease visible to the check below.
        drop(lock(
            &self.governance_write_gate,
            HostLocation::AwaitPolicyAdmissionsForPause,
        )?);
        // #480 PG-2c fix round 1 (the coordinator's ruling 4): not before when a refused session
        // close, and a check that failed, are tried again (a `stand` pause's), and the outcome of
        // that failure while it stands, which the request's answer carries as its cause.
        let mut close_again_at: Option<Instant> = None;
        let mut check_again_at: Option<Instant> = None;
        let mut failure: Option<Outcome> = None;
        loop {
            if let Some(error) = self
                .fatal
                .current()
                .map_err(RequestFailure::poison_without_terminal)?
            {
                return Err(RequestFailure::poison_without_terminal(error));
            }
            if stand
                && !self
                    .instance_pause_held(paused.alias, paused.revision)
                    .map_err(RequestFailure::poison_without_terminal)?
            {
                return Ok(ReleaseEnd::Lifted);
            }
            if self.fatal.is_shutdown_requested() {
                return self.release_at_shutdown(paused);
            }
            if check_again_at.is_none_or(|at| Instant::now() >= at) {
                match self.hand_back_check(paused, &links, bound, drained, close_again_at) {
                    Ok(HandBackCheck::End(end)) => return Ok(end),
                    Ok(HandBackCheck::Again) => {
                        check_again_at = None;
                        failure = None;
                        continue;
                    }
                    Ok(HandBackCheck::Wait) => {
                        check_again_at = None;
                        failure = None;
                    }
                    Ok(HandBackCheck::CloseRefused) => {
                        check_again_at = None;
                        failure = None;
                        close_again_at =
                            Some(Instant::now() + self.scheduling_pause_retry_interval());
                    }
                    Err(failed) if stand && tried_again(&failed, paused.instance_id) => {
                        self.note_check_failed(paused, &failed.error)?;
                        failure = Some(failed.error.outcome());
                        check_again_at =
                            Some(Instant::now() + self.scheduling_pause_retry_interval());
                    }
                    Err(failed) => return Err(failed),
                }
            }
            // #480 PG-2c: the end of the caller's time; a continuation has none.
            if let Some(grace) = bound.grace()
                && Instant::now() >= grace
            {
                if stand {
                    return Ok(ReleaseEnd::Standing {
                        in_the_way: self.noted_in_the_way(paused)?,
                        carry_on: true,
                        failure,
                    });
                }
                // #480 PG-2d: a tool's hold not quiet in its caller's time; what its last check
                // noted was in the way.
                self.mark_not_quiet(paused, None)?;
                return Err(scheduling_pause_release_busy());
            }
            thread::sleep(SCHEDULING_PAUSE_DRAIN_POLL_INTERVAL);
        }
    }

    /// #480 PG-2c (PG-1 fix rounds 2 and 3): the hand-back of the pause `paused` at a shutdown. A
    /// tool's hold fails (`scheduling_pause_release_interrupted`) and is lifted. A person's pause
    /// is kept, judged on what is in the way at the shutdown, read again here outside the
    /// admission guard: with nothing in the way and no refusing queued entry the hand-back ends as
    /// it would have, `released`, or, outside the multi-Nemu gate, where it would still close the
    /// instance's session, kept. Fix round 1: a read that fails with a failure `tried_again`
    /// takes keeps it too.
    fn release_at_shutdown(
        &self,
        paused: PausedInstance<'_>,
    ) -> Result<ReleaseEnd, RequestFailure> {
        if paused.mode != SchedulingPauseMode::Stand {
            return Err(scheduling_pause_release_failure(RuntimeHostError::request(
                HostCode::SchedulingPauseReleaseInterrupted,
                HostLocation::ReleasePausedInstance,
                RuntimeErrorCode::RuntimeUnavailable,
            )));
        }
        let quiet = (|| -> Result<bool, RequestFailure> {
            let obstacles = self.release_obstacles(paused.alias, paused.instance_id)?;
            Ok(!obstacles.queue_waiting
                && obstacles.none_left()
                && self.keeps_device_session(paused.instance_id)?)
        })();
        match quiet {
            Ok(true) => Ok(ReleaseEnd::Released),
            Ok(false) => Ok(ReleaseEnd::KeptAtShutdown),
            Err(failed) if tried_again(&failed, paused.instance_id) => {
                Ok(ReleaseEnd::KeptAtShutdown)
            }
            Err(failed) => Err(failed),
        }
    }

    /// #480 PG-2c fix round 1: one check of the hand-back of the pause `paused`
    /// (`release_paused_instance`, which calls it at every poll but while a failed check waits to
    /// be tried again). (a) No wait for the instance's admission guard is blind: the request of a
    /// `stand` pause waits for it until the end of its caller's time, then answers what is in the
    /// way read outside the guard (it may be empty: nothing was in the way at that read, and the
    /// hand-back had not finished); the request of a `quiet_or_lift` pause waits for it until its
    /// grace, then fails `scheduling_pause_release_busy` (ruling 3: every obstacle within the
    /// caller's time), or at a shutdown `scheduling_pause_release_interrupted` (the loop's top
    /// check); a continuation takes it only if it is free, and else goes round the loop's top
    /// checks (fatal, lifted, shutdown) after one poll. With the guard in hand: what is in the way
    /// (kept in memory, with `close_refused` while a refused close waits to be tried again); a
    /// refusing queued entry fails a tool's hold at once (`TransferNotSafe`); with nothing left,
    /// inside the multi-Nemu gate `released`, outside it the session close, which a `stand` pause
    /// begins only after (b) asking under the pause table's lock whether it is still held at its
    /// revision, the mark that its close has begun set in the same step (a resume lifting it
    /// meanwhile: nothing closed). A close lease the scheduler refused closed nothing:
    /// `close_refused` for a `stand` pause, tried again after `SCHEDULING_PAUSE_RETRY_INTERVAL`;
    /// any other failure of the close is the check's failure (`check_failed` when `tried_again`
    /// takes it). Then, outside the guard, a run in flight is drained again.
    fn hand_back_check(
        &self,
        paused: PausedInstance<'_>,
        links: &EventLinksDraft,
        bound: StagesBound,
        drained: &mut SchedulingDrainSummary,
        close_again_at: Option<Instant>,
    ) -> Result<HandBackCheck, RequestFailure> {
        let stand = paused.mode == SchedulingPauseMode::Stand;
        let instance_id = paused.instance_id;
        // Tests: a check that fails as a failure of the Runtime that is not fatal would.
        #[cfg(test)]
        self.injected_pause_check_failure_for_test()?;
        let instance_guard = self.instance_guard(instance_id)?;
        let until = match bound {
            StagesBound::Caller(deadlines) => deadlines.grace,
            StagesBound::Unbounded => Instant::now(),
        };
        // Fix round 2 (item 4): a `stand` pause's wait for the guard watches its pause too.
        let watched = stand.then_some((paused.alias, paused.revision));
        let Some(admission) = self.admission_within(&instance_guard, until, watched)? else {
            // The loop's top check keeps or lifts the pause as its mode says.
            if self.fatal.is_shutdown_requested() {
                return Ok(HandBackCheck::Again);
            }
            // Fix round 2 (item 4): a person's resume lifted the pause (its revision) while the
            // request waited for the guard: the request answers `scheduling_pause_lifted_by_resume`,
            // writes nothing into the pause's state and starts no continuation.
            if stand && !self.instance_pause_held(paused.alias, paused.revision)? {
                return Ok(HandBackCheck::End(ReleaseEnd::Lifted));
            }
            return match bound {
                StagesBound::Unbounded => {
                    // Tests: a continuation found the guard taken.
                    #[cfg(test)]
                    self.pause_meets_taken_guard_for_test()?;
                    Ok(HandBackCheck::Wait)
                }
                StagesBound::Caller(_) if stand => {
                    let in_the_way = self
                        .release_obstacles(paused.alias, instance_id)?
                        .in_the_way(false);
                    self.note_in_the_way(paused, &in_the_way)?;
                    Ok(HandBackCheck::End(ReleaseEnd::Standing {
                        in_the_way,
                        carry_on: true,
                        failure: None,
                    }))
                }
                // #480 PG-2d: a tool's hold whose admission guard another party still holds at
                // its grace is not quiet in its time; what is in the way is read outside the guard,
                // as a person's pause reads it there.
                StagesBound::Caller(_) => {
                    let in_the_way = self
                        .release_obstacles(paused.alias, instance_id)?
                        .in_the_way(false);
                    self.mark_not_quiet(paused, Some(in_the_way))?;
                    Err(scheduling_pause_release_busy())
                }
            };
        };
        let obstacles = self.release_obstacles(paused.alias, instance_id)?;
        // The close path's own rule (`prepare_resource_close`: a non-empty queue is
        // `TransferNotSafe`), applied before any wait: a queued client waits for the device.
        // #480 PG-2c: a person's pause waits for that client instead.
        if obstacles.queue_waiting && !stand {
            // #480 PG-2d fix round 1: a tool's hold lifted without its instance quiet; what this
            // check read was in the way (the refusing entry, `queued_entry`, among it).
            self.mark_not_quiet(paused, Some(obstacles.in_the_way(false)))?;
            return Err(scheduling_pause_release_failure(
                RuntimeHostError::scheduler(
                    HostLocation::PrepareResourceCloseLease,
                    &SchedulerError::TransferNotSafe,
                ),
            ));
        }
        let quiet = !obstacles.queue_waiting && obstacles.none_left();
        let close_waits = close_again_at.is_some_and(|at| Instant::now() < at);
        self.note_in_the_way(paused, &obstacles.in_the_way(quiet && close_waits))?;
        if quiet && !close_waits {
            // Workflow #191 H (owner ruling 2026-09-29): a pause is not a disconnect; only an
            // instance outside the multi-Nemu gate closes its session here.
            if self.keeps_device_session(instance_id)? {
                return Ok(HandBackCheck::End(ReleaseEnd::Released));
            }
            if stand {
                // Tests (fix round 2, item 5): after the loop's top check, before the question.
                #[cfg(test)]
                self.before_hand_back_close_question_for_test()?;
                match self.begin_hand_back_close(paused)? {
                    CloseStep::Begun => {}
                    CloseStep::Lifted => return Ok(HandBackCheck::End(ReleaseEnd::Lifted)),
                    CloseStep::ResumeLifting => return Ok(HandBackCheck::Wait),
                }
            }
            // #480 PG-2b fix round 1: this close is the pause's hand-back, and says so
            // (`hand_back`), so its close lease passes what this pause holds back.
            match self.close_retained_instance_with(
                instance_id,
                links.clone(),
                false,
                LeaseReleaseReason::InstancePaused,
                true,
                &admission,
            ) {
                Ok(RetainedClose::Ran(Ok(()))) => {
                    return Ok(HandBackCheck::End(ReleaseEnd::Released));
                }
                // ISO-2 (#419, ISO-M rows D54, C32): a close whose resources were confirmed is
                // gone and its failure recorded; the pause holds. Workflow #403: an unconfirmed
                // close fenced the instance; a tool's hold fails with the fence's non-fatal
                // failure, a person's pause stays `paused` (#480 PG-2c), the close not tried
                // again: a person's resume reconnects the fenced instance.
                Ok(RetainedClose::Ran(Err(close_error))) => {
                    return match self.settle_retained_close_failure(
                        HostLocation::ClosePausedInstanceSession,
                        instance_id,
                        &close_error,
                    )? {
                        None => Ok(HandBackCheck::End(ReleaseEnd::Released)),
                        Some(_) if stand => {
                            let in_the_way = vec![PauseObstacle::CloseUnconfirmed];
                            self.note_in_the_way(paused, &in_the_way)?;
                            Ok(HandBackCheck::End(ReleaseEnd::Standing {
                                in_the_way,
                                carry_on: false,
                                failure: None,
                            }))
                        }
                        Some(error) => Err(scheduling_pause_release_failure(error)),
                    };
                }
                // #480 PG-2c fix round 1: a close lease the scheduler refused (another member's
                // entry ahead, the takeover cooldown, a holder) closed nothing: a person's pause
                // ends its mark and tries again after the retry interval.
                Ok(RetainedClose::LeaseRefused(_)) if stand => {
                    lock(&self.scheduling_pause, HostLocation::ReleasePausedInstance)?
                        .end_hand_back_close(paused.alias, paused.revision);
                    self.note_in_the_way(paused, &obstacles.in_the_way(true))?;
                    return Ok(HandBackCheck::CloseRefused);
                }
                // #480 PG-2d fix round 1: a tool's hold whose close lease was refused (it closed
                // nothing) is lifted without its instance quiet; `close_refused` was in the way.
                Ok(RetainedClose::LeaseRefused(error)) => {
                    self.mark_not_quiet(paused, Some(obstacles.in_the_way(true)))?;
                    return Err(scheduling_pause_release_failure(error));
                }
                Err(error) => return Err(scheduling_pause_release_failure(error)),
            }
        }
        drop(admission);
        // #480 PG-1, tests: the hand-back found its instance's key held.
        #[cfg(test)]
        if obstacles.lease_held {
            self.pause_release_meets_lease_for_test()?;
        }
        if obstacles.run_in_flight {
            let watched = stand.then_some((paused.alias, paused.revision));
            // #480 PG-2e: this check noted what it read, the run among it; the drain notes nothing.
            let again = match self.drain_contained_runs_for_pause(
                instance_id,
                bound.stop_runs_at(),
                bound.grace(),
                watched,
                None,
            )? {
                PauseDrainEnd::Drained(again) => again,
                PauseDrainEnd::Interrupted(_) if stand => {
                    return Ok(HandBackCheck::End(ReleaseEnd::KeptAtShutdown));
                }
                PauseDrainEnd::Interrupted(_) => return Err(pause_drain_interrupted()),
                PauseDrainEnd::PastGrace(_) if stand => {
                    return Ok(HandBackCheck::End(ReleaseEnd::Standing {
                        in_the_way: self.noted_in_the_way(paused)?,
                        carry_on: true,
                        failure: None,
                    }));
                }
                PauseDrainEnd::PastGrace(_) => {
                    // #480 PG-2d: a tool's hold not quiet in its caller's time; what this check
                    // noted (the run drained again among it) was in the way.
                    self.mark_not_quiet(paused, None)?;
                    return Err(pause_drain_incomplete());
                }
                PauseDrainEnd::Lifted(_) => return Ok(HandBackCheck::End(ReleaseEnd::Lifted)),
            };
            let overflow = || {
                RuntimeHostError::fatal(
                    HostCode::SchedulingPauseDrainCountOverflow,
                    HostLocation::ReleasePausedInstance,
                    RuntimeErrorCode::RuntimeHalted,
                )
            };
            drained.finished = drained
                .finished
                .checked_add(again.finished)
                .ok_or_else(overflow)?;
            drained.cancelled = drained
                .cancelled
                .checked_add(again.cancelled)
                .ok_or_else(overflow)?;
            return Ok(HandBackCheck::Again);
        }
        // Workflow #427-A fix 1, tests: the release waits here for a lane's run.
        #[cfg(any(test, feature = "test-hooks"))]
        if obstacles.lane_working {
            self.pause_waits_for_lane_for_test()?;
        }
        Ok(HandBackCheck::Wait)
    }

    /// #480 PG-2c fix round 1: what the last check of the pause `paused` found in its way, as kept
    /// in memory with the pause.
    fn noted_in_the_way(
        &self,
        paused: PausedInstance<'_>,
    ) -> Result<Vec<PauseObstacle>, RequestFailure> {
        Ok(
            lock(&self.scheduling_pause, HostLocation::ReleasePausedInstance)?
                .noted_in_the_way(paused.alias),
        )
    }

    /// #480 PG-2c: the instance's admission guard for a pause's hand-back, waited for only until
    /// `until` (the end of its request's time; now, for a continuation, which tries it once) or a
    /// shutdown: `None` then, with the guard still taken (a Start holds it through its readiness
    /// wait). A poisoned guard is fatal. Fix round 2 (item 4): with `watched`, a `stand` pause's
    /// alias and revision, the wait also ends (`None`) once a person's resume lifted that pause.
    fn admission_within<'a>(
        &self,
        guard: &'a Mutex<()>,
        until: Instant,
        watched: Option<(&str, u64)>,
    ) -> RuntimeHostResult<Option<MutexGuard<'a, ()>>> {
        loop {
            match guard.try_lock() {
                Ok(admission) => return Ok(Some(admission)),
                Err(std::sync::TryLockError::WouldBlock) => {}
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    return Err(lock_poison_error(HostLocation::LockInstanceAdmission));
                }
            }
            if Instant::now() >= until || self.fatal.is_shutdown_requested() {
                return Ok(None);
            }
            if let Some((alias, revision)) = watched {
                // Tests: a request with time left found the guard taken.
                #[cfg(test)]
                self.pause_request_meets_taken_guard_for_test()?;
                if !self.instance_pause_held(alias, revision)? {
                    return Ok(None);
                }
            }
            thread::sleep(SCHEDULING_PAUSE_DRAIN_POLL_INTERVAL);
        }
    }

    /// #480 PG-1 (fix rounds 1 and 2): what is in the way of the hand-back of `instance_id`'s
    /// pause now (`release_paused_instance`, which reads it under the instance's admission guard
    /// at every check, and again, outside it, when a shutdown comes). Workflow #437 MAPP-1b
    /// (`contracts/device-group.md`, "One queue, one worker, one lease per device"): the pause of
    /// one member of a device group waits only for that member's own work: a lease whose token
    /// names it and the queued claims that serve it. For an instance alone on its device these
    /// are its lease and its queue, as before. Fix round 1 (item 3): the instance's own queued
    /// autostart claim is no such work: under this pause it ends at its first tick, touching
    /// nothing; and that lease's kind, the word `in_the_way` gives it (#480 PG-2c,
    /// `lease_obstacle`; fix round 2: an automatic start past its last question is that lease's
    /// `automatic_start`, the commit mark gone, `HostShared::automatic_start_hold`).
    fn release_obstacles(
        &self,
        instance_alias: &str,
        instance_id: InstanceId,
    ) -> Result<ReleaseObstacles, RequestFailure> {
        let gate = self.routine_gate(instance_id)?;
        let now_monotonic_ms = self
            .monotonic_ms()
            .map_err(RequestFailure::poison_without_terminal)?;
        let (lease_kind, queue_waiting) = {
            let scheduler = lock(&self.scheduler, HostLocation::ReadReleasedInstanceLease)?;
            let lease = scheduler
                .active_lease(instance_id)
                .filter(|lease| lease.token().instance_id() == instance_id);
            (
                lease.map(|lease| lease.kind()),
                scheduler.member_refusing_count(
                    instance_id,
                    actingcommand_scheduler::AutostartEndsAtGrant {
                        gates: gate,
                        member: instance_id,
                    },
                    now_monotonic_ms,
                ) > 0,
            )
        };
        let run_in_flight = lock(&self.contained_runs, HostLocation::ReadReleasedInstanceRuns)?
            .values()
            .any(|control| control.instance_id == instance_id);
        let reset_awaited = lock(
            &self.scheduling_pause,
            HostLocation::ReadAwaitedClientResets,
        )?
        .awaits_client_reset(instance_alias);
        // Workflow #427-A (C2, C3): a policy run its instance's worker still holds (from its
        // hand-off to its settlement) is waited for, as an active lease is.
        let lane_working = self
            .policy_run_working(instance_id)
            .map_err(RequestFailure::poison_without_terminal)?;
        Ok(ReleaseObstacles {
            lease_held: lease_kind.is_some(),
            lease_kind,
            queue_waiting,
            run_in_flight,
            reset_awaited,
            lane_working,
        })
    }

    /// `ResumeScheduling` (Workflow #191 ps1, ps2): lifts the matching gate and bumps its
    /// revision. An instance whose pause is still draining (`Draining`) or handing its device
    /// back (`Paused`) cannot be resumed yet. An instance resume then reconnects the device at
    /// once instead of at the next lazy open (ps2); a global resume reconnects nothing. With
    /// `expected` (Workflow #338 R4) the owner epoch and the scope's revision are compared first,
    /// under the same lock as the lift.
    ///
    /// Workflow #369 W-3: an instance resume checks those refusals before anything else, so a
    /// resume they refuse takes no claim. Its reconnect's `ResumeReconnect` claim is then granted
    /// before anything changes; it is an operator entry, so the closed gate does not hold it
    /// back, while every routine entry stays held back. Not granted within
    /// `CONNECTION_PREPARATION_CLAIM_WAIT_MS`, the resume is refused `lease_queue_expired` and
    /// nothing has changed. Once granted, the gate is lifted as before (the refusals checked
    /// again, since the pause can change while the claim waits; a refusal then releases the key
    /// and lifts nothing), the preparation runs on the claim's key, and the key's release hands
    /// it on, or pumps, so a claim the lift made eligible follows the reconnect. No dispatch can
    /// reach the instance while the key is held.
    ///
    /// Workflow #453 S2 fix 1: an instance whose entry's `start_paused` the start left unapplied
    /// (a pause of another reason held it) is held with `instance.added` in the lift's place,
    /// under the same persist gate and on the same key; no reconnect runs
    /// (`HostShared::hold_start_pause_at_release`).
    pub(super) fn resume_scheduling(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        scope: &SchedulingPauseScope,
        expected: Option<&SchedulingPauseExpectation>,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let SchedulingPauseScope::Instance { instance_alias } = scope else {
            let revision = {
                let _persist = lock(
                    &self.scheduling_pause_persist_gate,
                    HostLocation::PersistGlobalSchedulingResume,
                )?;
                let mut prospective = {
                    let table = lock(&self.scheduling_pause, ContractLocation::ResumeScheduling)?;
                    check_scheduling_pause_expectation(
                        expected,
                        self.owner_epoch,
                        table.global_revision,
                    )?;
                    if table.global.is_none() {
                        return Err(scheduling_pause_denied(
                            HostCode::SchedulingNotPaused,
                            ContractLocation::ResumeScheduling,
                        ));
                    }
                    table.clone()
                };
                prospective.lift_global()?;
                self.persist_scheduling_pauses(&prospective)?;
                lock(&self.scheduling_pause, ContractLocation::ResumeScheduling)?.lift_global()?
            };
            return Ok(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: None,
                result: RuntimeResult::SchedulingResumed {
                    scope: scope.clone(),
                    revision,
                    selfcheck: None,
                },
                failure_outcome: None,
            });
        };
        let instance_id = self.resolve_instance(instance_alias)?.instance_id();
        // Workflow #403: a fenced instance resumes even when it is not paused: the resume is a
        // person's re-proof of its close (the reconnect's preparation and self-check).
        let fenced = self.resource_fenced(instance_id)?;
        let unquiet = self
            .resumable_instance_pauses(instance_alias, expected, fenced)?
            .instance_state(instance_alias)
            .is_some_and(|state| state.stage != InstancePauseStage::Released);
        // #480 PG-2c: a person's pause still draining or handing its device back is lifted at once,
        // with no reconnect claim (something still uses the instance) and no reconnect, unless the
        // instance is fenced, whose resume is a person's re-proof of its close as before. Fix round
        // 1 (b): unless, under the persist gate, its hand-back's close has begun or its stage has
        // reached `released` meanwhile: then it takes the path below, a `released` pause's.
        if unquiet && !fenced {
            // Tests: between the stage read and the persist gate.
            #[cfg(test)]
            self.before_unquiet_resume_for_test()?;
            if let Some(resumed) =
                self.resume_unquiet_instance_pause(scope, instance_alias, instance_id, expected)?
            {
                return Ok(resumed);
            }
        }
        // Workflow #479 F-2 fix round 2, the coordinator's follow-up (`contracts/device-binding.md`,
        // "An iOS handset"): an instance whose device system has no backend in this build has
        // nothing to reconnect, as the start's preparation skips it: its pause is lifted and no
        // reconnect is attempted (no claim, no preparation, no open), and the answer and the ledger
        // say so (`scheduling_resume_without_backend`); nothing failed.
        if self.without_device_backend(instance_id)? {
            return self.resume_without_device_backend(
                scope,
                instance_alias,
                instance_id,
                expected,
                fenced,
            );
        }
        let token = self.await_preparation_claim(
            original,
            instance_alias,
            connection_id,
            ClaimKind::ResumeReconnect,
        )?;
        let lifted = (|| -> Result<InstanceResumeLift, RequestFailure> {
            let _persist = lock(
                &self.scheduling_pause_persist_gate,
                HostLocation::PersistInstanceSchedulingResume,
            )?;
            let mut prospective =
                self.resumable_instance_pauses(instance_alias, expected, fenced)?;
            // Workflow #453 S2 fix 1 (`contracts/scheduling-pause.md`, "Pauses held at a first
            // start"): the release of a pause while the instance's `start_paused` is not applied
            // yet holds it with `instance.added` instead, in this step; nothing without the member.
            if let Some(revision) =
                self.hold_start_pause_at_release(instance_alias, instance_id, &mut prospective)?
            {
                return Ok(InstanceResumeLift::HeldAgain(revision));
            }
            prospective.lift_instance(instance_alias)?;
            self.persist_scheduling_pauses(&prospective)?;
            Ok(InstanceResumeLift::Lifted(
                lock(&self.scheduling_pause, ContractLocation::ResumeScheduling)?
                    .lift_instance(instance_alias)?,
            ))
        })();
        let revision = match lifted {
            Ok(InstanceResumeLift::Lifted(revision)) => revision,
            // Workflow #453 S2 fix 1: the instance is paused again, so no reconnect runs and the
            // claim's key is released at once (a routine claim stays gated behind the new pause).
            // The answer says so in the resume's shape: the new pause's revision and a self-check
            // with both sides `ok: false` and `failure_code` `dispatch_paused_instance`, the
            // instance's control gate.
            Ok(InstanceResumeLift::HeldAgain(revision)) => {
                self.release_lease(request, original.request_id(), &token, connection_id, None)?;
                return Ok(OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: None,
                    result: RuntimeResult::SchedulingResumed {
                        scope: scope.clone(),
                        revision,
                        selfcheck: Some(scheduling_resume_selfcheck(
                            &[],
                            Some(HostCode::DispatchPausedInstance.as_str()),
                        )),
                    },
                    failure_outcome: None,
                });
            }
            Err(failure) => {
                return Err(
                    match self.release_lease(
                        request,
                        original.request_id(),
                        &token,
                        connection_id,
                        None,
                    ) {
                        Ok(_) => failure,
                        // ISO-5a (#419, ISO-M row D55): a release of the reconnect claim the
                        // scheduler refused (its lease lapsed or is gone) fails only this
                        // resume; a fatal one poisons as before.
                        Err(release) => failure.with_cleanup_failure(*release.error),
                    },
                );
            }
        };
        let selfcheck = self.reconnect_resumed_instance(
            request,
            original.request_id(),
            instance_alias,
            instance_id,
            &token,
            connection_id,
        )?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::SchedulingResumed {
                scope: scope.clone(),
                revision,
                selfcheck: Some(selfcheck),
            },
            failure_outcome: None,
        })
    }

    /// The pause table an instance resume would lift: the expectation (Workflow #338 R4) holds
    /// and the instance's pause is `Released`, or (Workflow #403) the instance is `fenced` and
    /// not paused. Otherwise today's refusal: `scheduling_not_paused`,
    /// `scheduling_pause_draining` or `scheduling_pause_releasing`. #480 PG-2c: a person's pause
    /// (`stand`) is resumable in every stage; a tool's hold (`quiet_or_lift`) keeps those
    /// refusals.
    fn resumable_instance_pauses(
        &self,
        instance_alias: &str,
        expected: Option<&SchedulingPauseExpectation>,
        fenced: bool,
    ) -> Result<SchedulingPauseTable, RequestFailure> {
        let table = lock(&self.scheduling_pause, ContractLocation::ResumeScheduling)?;
        check_resumable(&table, instance_alias, expected, fenced, self.owner_epoch)?;
        Ok(table.clone())
    }

    /// #480 PG-2c (`contracts/scheduling-pause.md`, "Resume"): the resume of a person's pause
    /// (`stand`) still in stage `draining` or `paused` on an instance that is not fenced. Under the
    /// persist gate, the refusals checked again, the pause is lifted (its rows recorded first) or,
    /// Workflow #453 S2 fix 1, held again with `instance.added`. No reconnect claim is waited for
    /// and no reconnect runs: what still uses the instance goes on (a run asked to stop at its next
    /// checkpoint still stops there), the session the hand-back has not closed outside the
    /// multi-Nemu gate stays open, and the pause's own request, or the thread that carries its
    /// stages on, ends at its next check (`scheduling_pause_lifted_by_resume` on the request).
    /// Answers `SchedulingResumed` with the scope's new revision and (fix round 3: an instance's
    /// resume always carries one) a self-check with both sides `ok: false`, no values and no
    /// `failure_code` (no report of a reconnect: none ran), and the receipt's outcome carries the
    /// informing `scheduling_resume_without_reconnect` as its success code's cause; nothing failed.
    ///
    /// #480 PG-2c fix round 1 (b): with the refusals, under the pause table's lock, it reads where
    /// the hand-back is (`SchedulingPauseTable::begin_resume_lift`): its close begun, or the stage
    /// `released` reached meanwhile, it lifts nothing here and answers `None`, and the resume takes
    /// the path a `released` pause's resume takes (the reconnect claim, the reconnect, `selfcheck`
    /// answered); otherwise it marks in the same step that it lifts without a reconnect, which a
    /// hand-back about to close meets (`begin_hand_back_close`: it closes nothing). A lift that
    /// then fails ends the mark.
    fn resume_unquiet_instance_pause(
        &self,
        scope: &SchedulingPauseScope,
        instance_alias: &str,
        instance_id: InstanceId,
        expected: Option<&SchedulingPauseExpectation>,
    ) -> Result<Option<OperationSuccess>, RequestFailure> {
        let _persist = lock(
            &self.scheduling_pause_persist_gate,
            HostLocation::PersistInstanceSchedulingResume,
        )?;
        let (mut prospective, pause_links) = {
            let mut table = lock(&self.scheduling_pause, ContractLocation::ResumeScheduling)?;
            check_resumable(&table, instance_alias, expected, false, self.owner_epoch)?;
            if !table.begin_resume_lift(instance_alias) {
                return Ok(None);
            }
            // #480 PG-2d: the pause request's links, for the record of this lift.
            let pause_links = table.pause_links.get(instance_alias).cloned();
            (table.clone(), pause_links)
        };
        // Tests (fix round 2, item 5): the lift is marked (`Lifting`) and not done yet.
        #[cfg(test)]
        self.after_resume_marks_lifting_for_test()?;
        let lifted = (|| -> Result<OperationSuccess, RequestFailure> {
            if let Some(revision) =
                self.hold_start_pause_at_release(instance_alias, instance_id, &mut prospective)?
            {
                return Ok(OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: None,
                    result: RuntimeResult::SchedulingResumed {
                        scope: scope.clone(),
                        revision,
                        selfcheck: Some(scheduling_resume_selfcheck(
                            &[],
                            Some(HostCode::DispatchPausedInstance.as_str()),
                        )),
                    },
                    failure_outcome: None,
                });
            }
            prospective.lift_instance(instance_alias)?;
            self.persist_scheduling_pauses(&prospective)?;
            let revision = lock(&self.scheduling_pause, ContractLocation::ResumeScheduling)?
                .lift_instance(instance_alias)?;
            // Fix round 3 (the coordinator's rulings: an instance's resume always carries a
            // self-check, and its `failure_code` stays for a reconnect that failed, which the
            // console shows as one): no reconnect ran, so both sides are `ok: false` with no
            // values and no `failure_code`, and the receipt's outcome says why, the informing
            // `scheduling_resume_without_reconnect` as the success code's cause.
            let cause = Outcome::new(HostCode::SchedulingResumeWithoutReconnect);
            // #480 PG-2d (`contracts/scheduling-pause.md`, "A pause's life in the ledger"): the
            // pause was lifted before it was quiet: one record of the receipt's own cause, under
            // the pause request's links (kept with the pause from its request; were they missing,
            // the instance's own). Written here, inside the persist gate with the lift, on purpose.
            // #480 PG-2e (the review of #1834, its second remark; this comment said another reason
            // before): no record that the pause holds can come after this one wherever this one is
            // written, since the lift is made under the gate and every such record is decided and
            // written under it only while the pause still holds (`record_held_pause_moment`, the
            // request's and since PG-2e the continuation's). Inside the gate this record comes
            // before every record of whoever holds the gate next (a later pause of the instance,
            // its own records among them).
            self.record_pause_moment(
                self.pause_record_links(pause_links, instance_id),
                cause.clone(),
            );
            Ok(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: None,
                result: RuntimeResult::SchedulingResumed {
                    scope: scope.clone(),
                    revision,
                    selfcheck: Some(scheduling_resume_selfcheck(&[], None)),
                },
                failure_outcome: Some(cause),
            })
        })();
        match lifted {
            Ok(resumed) => Ok(Some(resumed)),
            // The lift did not happen: the mark ends, and the hand-back goes on.
            Err(failure) => Err(
                match lock(&self.scheduling_pause, ContractLocation::ResumeScheduling) {
                    Ok(mut table) => {
                        table.end_resume_lift(instance_alias);
                        failure
                    }
                    Err(error) => failure.replace_with_poison(error),
                },
            ),
        }
    }

    /// Workflow #479 F-2 fix round 2, the coordinator's follow-up: whether `instance_id` is a
    /// registered instance whose device system has no backend in this build (an iOS handset,
    /// bound or not), which is inert.
    fn without_device_backend(&self, instance_id: InstanceId) -> Result<bool, RequestFailure> {
        Ok(lock(
            &self.registered_instances,
            HostLocation::ReadInstanceRegistry,
        )?
        .get(&instance_id)
        .is_some_and(|instance| instance.device_system_without_input_backend().is_some()))
    }

    /// Workflow #479 F-2 fix round 2, the coordinator's follow-up (`contracts/device-binding.md`,
    /// "An iOS handset"): the resume of an instance whose device system has no backend in this
    /// build, a person's or a tool's. Under the persist gate, the pause's refusals checked again
    /// (`resumable_instance_pauses`), it is lifted (its rows recorded first) or, Workflow #453 S2
    /// fix 1, held again with `instance.added`, as every instance resume's is; then no reconnect
    /// is attempted, as the start's preparation attempts none: no claim, no preparation, no open,
    /// nothing sent. Answers `SchedulingResumed` with the scope's new revision and a self-check of
    /// both sides `ok: false`, no values and no `failure_code` (no reconnect ran, so none is
    /// reported), the receipt's outcome carrying the informing `scheduling_resume_without_backend`
    /// as its success code's cause, and the ledger one record of it under the pause request's links
    /// (`record_pause_moment`), as `scheduling_resume_without_reconnect`'s lift writes its own.
    /// Nothing failed and nothing is recorded as a failure.
    fn resume_without_device_backend(
        &self,
        scope: &SchedulingPauseScope,
        instance_alias: &str,
        instance_id: InstanceId,
        expected: Option<&SchedulingPauseExpectation>,
        fenced: bool,
    ) -> Result<OperationSuccess, RequestFailure> {
        let _persist = lock(
            &self.scheduling_pause_persist_gate,
            HostLocation::PersistInstanceSchedulingResume,
        )?;
        let mut prospective = self.resumable_instance_pauses(instance_alias, expected, fenced)?;
        if let Some(revision) =
            self.hold_start_pause_at_release(instance_alias, instance_id, &mut prospective)?
        {
            return Ok(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: None,
                result: RuntimeResult::SchedulingResumed {
                    scope: scope.clone(),
                    revision,
                    selfcheck: Some(scheduling_resume_selfcheck(
                        &[],
                        Some(HostCode::DispatchPausedInstance.as_str()),
                    )),
                },
                failure_outcome: None,
            });
        }
        // The pause request's links, for the record of this lift, read before the lift ends them.
        let pause_links = lock(&self.scheduling_pause, ContractLocation::ResumeScheduling)?
            .pause_links
            .get(instance_alias)
            .cloned();
        prospective.lift_instance(instance_alias)?;
        self.persist_scheduling_pauses(&prospective)?;
        let revision = lock(&self.scheduling_pause, ContractLocation::ResumeScheduling)?
            .lift_instance(instance_alias)?;
        let cause = Outcome::new(HostCode::SchedulingResumeWithoutBackend);
        self.record_pause_moment(
            self.pause_record_links(pause_links, instance_id),
            cause.clone(),
        );
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::SchedulingResumed {
                scope: scope.clone(),
                revision,
                selfcheck: Some(scheduling_resume_selfcheck(&[], None)),
            },
            failure_outcome: Some(cause),
        })
    }

    /// Workflow #191 ps2: the reconnect of an instance resume, since Workflow #317 sc3 (c) the
    /// instance's preparation phase, since Workflow #369 W-3 on the reconnect claim's key
    /// (`prepare_on_operator_claim`): the close of a session the instance still holds
    /// (Workflow #191 h3: a reconnect is a disconnect and a connect, and a failed close skips
    /// the opens); the opens of its input and capture backends through
    /// `ExecutionKernel::open_instance_backends`, recorded as every open is recorded
    /// (`backend.open_observed`, the `backend.selfcheck.*` facts, `device.self_check` and the
    /// availability they gate); then the key's release. The opened session stays open for the
    /// instance's next leases (Workflow #191 H); a failed open, or an instance outside the
    /// multi-Nemu gate, closes it before the release. A failing step is reported in the
    /// self-check with its code and leaves the instance unavailable; it never rolls the resume
    /// back and is not retried. Only a fatal failure (an unconfirmed close, a failed record)
    /// fails the request.
    fn reconnect_resumed_instance(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        request_id: RequestId,
        instance_alias: &str,
        instance_id: InstanceId,
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> Result<SchedulingResumeSelfCheck, RequestFailure> {
        // Workflow #429 R-0b (register §1.238): the reconnect shows while it holds its claim's
        // key, with the resume's request; its guard drops however the reconnect returns.
        let _activity = self
            .begin_instance_activity(
                instance_id,
                super::instance_activity::ActivityStart::Reconnect { request_id },
            )
            .map_err(RequestFailure::poison_without_terminal)?;
        self.prepare_on_operator_claim(
            request,
            request_id,
            instance_alias,
            instance_id,
            token,
            connection_id,
        )
    }

    /// Workflow #191 ps2: the client on `connection_id` owes `instance_alias` a reset after its
    /// cancelled contained run.
    pub(super) fn await_client_reset(
        &self,
        instance_alias: &str,
        connection_id: ConnectionId,
    ) -> RuntimeHostResult<()> {
        lock(&self.scheduling_pause, HostLocation::AwaitClientReset)?
            .client_resets
            .entry(instance_alias.to_owned())
            .or_default()
            .insert(connection_id);
        Ok(())
    }

    /// Workflow #191 ps2: the client on `connection_id` finished a reset of `instance_alias`.
    pub(super) fn settle_client_reset(
        &self,
        instance_alias: &str,
        connection_id: ConnectionId,
    ) -> RuntimeHostResult<()> {
        let mut table = lock(&self.scheduling_pause, HostLocation::SettleClientReset)?;
        if let Some(connections) = table.client_resets.get_mut(instance_alias) {
            connections.remove(&connection_id);
            if connections.is_empty() {
                table.client_resets.remove(instance_alias);
            }
        }
        Ok(())
    }

    /// Workflow #191 ps2: a closed connection owes no reset any more.
    pub(super) fn forget_client_resets(
        &self,
        connection_id: ConnectionId,
    ) -> RuntimeHostResult<()> {
        lock(&self.scheduling_pause, HostLocation::ForgetClientResets)?
            .client_resets
            .retain(|_, connections| {
                connections.remove(&connection_id);
                !connections.is_empty()
            });
        Ok(())
    }

    pub(super) fn pinned_policy_catalog(
        &self,
        decision_id: &str,
    ) -> RuntimeHostResult<Option<CatalogGeneration>> {
        Ok(
            lock(self.policy()?, HostLocation::ReadPinnedPolicyCatalog)?
                .pinned_catalog(decision_id),
        )
    }

    pub(super) fn project_policy_input_identity(
        &self,
        as_of_ledger_position: u64,
    ) -> Result<OperationSuccess, RequestFailure> {
        let identity = (|| {
            if as_of_ledger_position == 0 {
                return Err(RuntimeHostError::request(
                    HostCode::PolicyInputPositionUnavailable,
                    ContractLocation::ProjectPolicyInputIdentity,
                    RuntimeErrorCode::InvalidRequest,
                ));
            }
            let _outcome_gate = lock(
                &self.policy_outcome_gate,
                HostLocation::SnapshotPolicyInputIdentityOutcomeState,
            )?;
            let mut policy = lock(
                self.policy()?,
                HostLocation::ReadPolicyInputIdentityOutcomeKeys,
            )?;
            let outcome_keys = policy.outcome_key_snapshot()?;
            let _fact_gate = lock(
                &self.fact_write_gate,
                HostLocation::ProjectPolicyInputIdentityFacts,
            )?;
            let (facts, _, _) = self.project_authoritative_policy_inputs_under_gate(
                &mut policy,
                ContractLocation::ProjectPolicyInputIdentity,
                &outcome_keys,
                Some(as_of_ledger_position),
            )?;
            RuntimePolicyInputIdentity::new(facts.ledger_position, facts.fact_snapshot_id).map_err(
                |_| {
                    RuntimeHostError::fatal(
                        HostCode::PolicyInputIdentityInvalid,
                        ContractLocation::ProjectPolicyInputIdentity,
                        RuntimeErrorCode::RuntimeHalted,
                    )
                },
            )
        })()
        .map_err(planning_request_failure)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::PolicyInputIdentityProjected { identity },
            failure_outcome: None,
        })
    }
}

/// Per instance alias: the policy instance facts the store did not hold, or held in the wrong
/// shape, at the projected position. Such an instance is projected `available = false`.
pub(super) type MissingPolicyInstanceFacts = BTreeMap<String, Vec<&'static str>>;

/// Builds the evaluation's `instances` from the instance fact store (Workflow #313 item 4):
/// one entry per configured static identity, with `available`, `capability_operation_ids` and
/// `preferred_task_ids` read from the three policy instance facts that apply to it (the most
/// specific scope first). An instance missing any of the three, or holding one that is not an
/// inline value of the expected shape, is projected unavailable and reported under its alias
/// with the keys it lacks; it is never silently available. Workflow #469 H1 (register §1.397):
/// every instance projected unavailable is also returned with who withdrew it, read from the
/// `session.instance.available` record resolved here and the instance's configured seed in
/// `inputs` ([`HostShared::availability_withdrawal`]).
fn project_policy_instances(
    inputs: &PolicyInputSnapshot,
    store: &InstanceFactStore,
) -> (
    Vec<InstanceSnapshot>,
    MissingPolicyInstanceFacts,
    AvailabilityWithdrawals,
) {
    let mut instances = Vec::new();
    let mut missing_instance_facts = MissingPolicyInstanceFacts::new();
    let mut withdrawals = AvailabilityWithdrawals::new();
    for identity in inputs.instance_identities() {
        let context = InstanceFactContext {
            instance_id: identity.instance_id.to_owned(),
            server_id: identity.server_id.to_owned(),
            game_id: identity.game_id.to_owned(),
        };
        let mut missing = Vec::new();
        let available_record = store.resolve_active(&context, POLICY_INSTANCE_AVAILABLE_KEY);
        let available = available_record
            .and_then(policy_instance_boolean)
            .unwrap_or_else(|| {
                missing.push(POLICY_INSTANCE_AVAILABLE_KEY);
                false
            });
        let capability_operation_ids = store
            .resolve_active(&context, POLICY_INSTANCE_CAPABILITIES_KEY)
            .and_then(|record| policy_instance_string_list(record, POLICY_INSTANCE_OPERATION_FIELD))
            .unwrap_or_else(|| {
                missing.push(POLICY_INSTANCE_CAPABILITIES_KEY);
                Vec::new()
            });
        let preferred_task_ids = store
            .resolve_active(&context, POLICY_INSTANCE_PREFERRED_TASKS_KEY)
            .and_then(|record| policy_instance_string_list(record, POLICY_INSTANCE_TASK_FIELD))
            .unwrap_or_else(|| {
                missing.push(POLICY_INSTANCE_PREFERRED_TASKS_KEY);
                Vec::new()
            });
        let available = available && missing.is_empty();
        if !missing.is_empty() {
            missing_instance_facts.insert(identity.instance_id.to_owned(), missing);
        }
        if !available {
            // Workflow #469 H1 fix 2: the instance's configured seed, which these inputs hold;
            // with it `false`, the configuration is who withdrew the availability.
            let seed_available = inputs
                .instance_seeds()
                .find(|seed| seed.instance_id == identity.instance_id)
                .is_none_or(|seed| seed.available);
            withdrawals.insert(
                identity.instance_id.to_owned(),
                HostShared::availability_withdrawal(available_record, seed_available),
            );
        }
        instances.push(InstanceSnapshot {
            instance_id: identity.instance_id.to_owned(),
            server_id: identity.server_id.to_owned(),
            game_id: identity.game_id.to_owned(),
            host_id: identity.host_id.to_owned(),
            available,
            capability_operation_ids,
            preferred_task_ids,
        });
    }
    (instances, missing_instance_facts, withdrawals)
}

fn policy_instance_boolean(record: &FactRecord) -> Option<bool> {
    match &record.content {
        FactContent::Inline {
            value: ContractFactValue::Boolean(value),
        } => Some(*value),
        _ => None,
    }
}

/// Every row must hold exactly the string field `field`; anything else is not a usable value.
fn policy_instance_string_list(record: &FactRecord, field: &str) -> Option<Vec<String>> {
    let FactContent::Inline {
        value: ContractFactValue::RecordList(rows),
    } = &record.content
    else {
        return None;
    };
    rows.iter()
        .map(|row| match (row.len(), row.get(field)) {
            (1, Some(FactScalar::String(value))) => Some(value.clone()),
            _ => None,
        })
        .collect()
}

/// Records `policy_instance_fact_missing` (with the missing `fact_key`) on every decision made
/// for an instance the projection found incomplete, next to the evaluator's own reasons for that
/// decision.
fn attach_missing_instance_fact_reasons(
    cycle: &mut PolicyCycle,
    missing_instance_facts: &MissingPolicyInstanceFacts,
    ledger_position: u64,
) {
    if missing_instance_facts.is_empty() {
        return;
    }
    let Some(evaluation) = cycle.evaluation.as_mut() else {
        return;
    };
    for decision in &mut evaluation.decisions {
        let Some(instance_id) = decision.instance_id.clone() else {
            continue;
        };
        let Some(missing) = missing_instance_facts.get(&instance_id) else {
            continue;
        };
        for key in missing {
            decision.reasons.push(DecisionReason {
                outcome: Outcome::new(HostCode::PolicyInstanceFactMissing).fact_key(*key),
                detail: format!(
                    "instance '{instance_id}' has no usable `{key}` fact at ledger position \
                     {ledger_position}; it is projected unavailable"
                ),
            });
        }
    }
}

/// Records `eligible_since_unknown` on every decision whose pair started aging in a cycle
/// evaluated before any cycle's eligibility verdicts were applied since the host started
/// (Workflow #308 slice 5b). Eligibility ages are memory-only and no ledger event records an
/// evaluation, so the age such a pair had before this host is not rebuilt: it counts from this
/// cycle.
fn attach_eligible_since_unknown_reasons(
    cycle: &mut PolicyCycle,
    unknown_pairs: &BTreeSet<(String, String)>,
) {
    if unknown_pairs.is_empty() {
        return;
    }
    let Some(evaluation) = cycle.evaluation.as_mut() else {
        return;
    };
    for decision in &mut evaluation.decisions {
        let Some(instance_id) = decision.instance_id.clone() else {
            continue;
        };
        if !unknown_pairs.contains(&(decision.task_id.clone(), instance_id)) {
            continue;
        }
        decision.reasons.push(DecisionReason {
            outcome: Outcome::new(HostCode::EligibleSinceUnknown),
            detail: "eligibility age is kept in memory only and is not rebuilt after a start; \
                     this candidate's age counts from this cycle"
                .to_owned(),
        });
    }
}

/// The tasks named by a priority offset of an evaluation that its active catalog does not
/// declare (Workflow #308 slice 4a-2).
pub(super) type UnknownPriorityOffsetTasks = BTreeSet<String>;

/// Reports each unknown offset task once per evaluated cycle: one explanatory decision of
/// that task, without an instance, blocked, with the single reason
/// `priority_offset_unknown_task` (with its `task_id`). The evaluator never saw such a task,
/// so this decision selects nothing and carries no rank; the offset stays in the inputs and is
/// ignored there.
fn attach_unknown_priority_offset_task_reasons(
    cycle: &mut PolicyCycle,
    unknown_offset_tasks: &UnknownPriorityOffsetTasks,
    ledger_position: u64,
) {
    let Some(evaluation) = cycle.evaluation.as_mut() else {
        return;
    };
    for task_id in unknown_offset_tasks {
        evaluation
            .decisions
            .push(actingcommand_policy::TaskDecision {
                task_id: task_id.clone(),
                instance_id: None,
                eligibility: actingcommand_policy::EligibilityState::Unknown,
                state: actingcommand_policy::SchedulingDecisionState::Blocked,
                rank: None,
                detection_suggestions: Vec::new(),
                reasons: vec![DecisionReason {
                    outcome: Outcome::new(HostCode::PriorityOffsetUnknownTask)
                        .task_id(task_id.as_str()),
                    detail: format!(
                        "a priority offset at ledger position {ledger_position} names task \
                     '{task_id}', which the active catalog does not declare; the offset is \
                     ignored"
                    ),
                }],
                next_wake_unix_ms: None,
            });
    }
}

fn policy_event_data(
    intent: &DispatchIntent,
    reason_chain: &DecisionReasonChain,
) -> RuntimeHostResult<PolicyDispatchEventData> {
    let procedure_binding_digest = intent.procedure_binding_digest.clone().ok_or_else(|| {
        policy_admission_fatal(
            HostCode::ProcedureBindingDigestMissing,
            HostLocation::BuildPolicyDispatchEvent,
        )
    })?;
    Ok(PolicyDispatchEventData {
        decision_id: intent.decision_id.clone(),
        task_id: intent.task_id.clone(),
        instance_id: intent.instance_id.clone(),
        operation_id: intent.operation_id.clone(),
        procedure_binding_digest,
        reason_chain_id: reason_chain.id.clone(),
        // Workflow #463 V2 fix round 2: each reason in its recorded form, as a replay compares it.
        reasons: crate::policy_host::recorded_reasons(reason_chain),
        catalog_hash: intent.catalog_hash.clone(),
        catalog_version: intent.catalog_version,
        input_ledger_position: intent.input_ledger_position,
        fact_snapshot_id: intent.fact_snapshot_id.clone(),
        approval_fact_ids: intent.approval_refs.clone(),
        urgency_milli: intent.prerequisites.urgency_milli,
    })
}

fn runtime_policy_seed(
    fact_snapshot_id: &str,
    time: EvaluationTime,
    owner_epoch: actingcommand_contract::OwnerEpoch,
) -> RuntimeHostResult<u64> {
    let bytes = serde_json::to_vec(&(fact_snapshot_id, time, owner_epoch)).map_err(|_| {
        RuntimeHostError::fatal(
            HostCode::PolicySeedEncodeFailed,
            HostLocation::DerivePolicySeed,
            RuntimeErrorCode::RuntimeHalted,
        )
    })?;
    let digest = Sha256::digest(bytes);
    let mut seed = [0_u8; 8];
    seed.copy_from_slice(&digest[..8]);
    Ok(u64::from_be_bytes(seed))
}

/// Workflow #428 S168-LB1a (`contracts/instance-fact-store.md`, "Left out of the projection"): the
/// keys the projection left out, per instance alias they apply to, as the evaluation reads them.
fn left_out_facts(lists: &LeftOutRecordLists) -> actingcommand_policy::LeftOutFacts {
    let mut left_out = actingcommand_policy::LeftOutFacts::default();
    for list in lists {
        for instance in &list.instances {
            left_out.insert(instance, &list.fact_key);
        }
    }
    left_out
}

impl HostShared {
    /// Workflow #428 S168-LB1a (`contracts/instance-fact-store.md`, "Left out of the
    /// projection"; the coordinator's answer 3 of register §1.309): one Warning `runtime.failed`
    /// `policy_fact_rows_exceeded` (`fact_key`, `count` the record's rows) under the system links
    /// for each record list the projection left out for its rows, the first time this start left
    /// that record out, and none after it; also handed to the daemon's log. What was warned of is
    /// kept in the policy host's memory only, so a restart warns once more. A failed append fails
    /// the cycle as every ledger write does.
    fn report_left_out_record_lists(&self, lists: &LeftOutRecordLists) -> RuntimeHostResult<()> {
        if lists.is_empty() {
            return Ok(());
        }
        let due = {
            let mut policy = lock(self.policy()?, HostLocation::ProjectPolicyFacts)?;
            lists
                .iter()
                .filter(|list| {
                    policy.note_rows_exceeded_warning(&list.scope, &list.fact_key, list.event_id)
                })
                .collect::<Vec<_>>()
        };
        for list in due {
            let outcome = Outcome::new(crate::codes::PolicyFactRowsCode::Exceeded)
                .operation(HostLocation::ProjectPolicyFacts)
                .fact_key(list.fact_key.as_str())
                .count(u32::try_from(list.rows).unwrap_or(u32::MAX));
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
                        "policy_fact_projection",
                        RuntimeLifecycleFailureStage::PolicyDriver.location(),
                        "runtime_host",
                        HostLocation::ProjectPolicyFacts.as_str(),
                    ),
                    AuditInput::new(),
                ),
            )?;
            self.fatal.report_warning(&outcome);
        }
        Ok(())
    }

    /// Workflow #463 V2 (`contracts/package-variables.md`, "The run"): each
    /// `goal_value_not_applied` as a Warning `runtime.failed` record of the policy driver, as
    /// `policy_fact_rows_exceeded` is written; the run goes on with the variable's default.
    fn report_variable_warnings(&self, warnings: Vec<Outcome>) -> RuntimeHostResult<()> {
        for outcome in warnings {
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
                        "policy_variable_values",
                        RuntimeLifecycleFailureStage::PolicyDriver.location(),
                        "runtime_host",
                        HostLocation::EvaluatePolicyCycle.as_str(),
                    ),
                    AuditInput::new(),
                ),
            )?;
            self.fatal.report_warning(&outcome);
        }
        Ok(())
    }
}
