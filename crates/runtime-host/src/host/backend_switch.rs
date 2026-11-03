// SPDX-License-Identifier: AGPL-3.0-only

//! #402 HS-2 (design `rd-1031/backend-switch/model-backend-switch.md` §3; register §1.270): an
//! instance's touch and capture backends switched while the Runtime runs, by the
//! `ConfigureSettings` that names the instance's two backend keys.
//!
//! - **The boundary** (§3.1). Only a free instance switches: the switch is an operator claim
//!   (`ClaimKind::BackendSwitch`) in the instance's one queue, which waits at most
//!   `CONNECTION_PREPARATION_CLAIM_WAIT_MS`, as a self-check does; a run (even between two of its
//!   steps), a ladder, a lease or a Lab operation that holds the instance longer refuses the
//!   switch (`instance_busy`), never defers it. A player's hold refuses it at the entry
//!   (`player_held`); under the admission guard a #403 fence (`instance_fenced`) and a pending
//!   endpoint or an unbound handset (`endpoint_unavailable`) refuse it.
//! - **The order** (§3.2, §3.3). The kept session closes, the registration takes the new
//!   backends (`ExecutionKernel::switch_instance_backends`, the registration's own checks), and a
//!   connection preparation opens and self-checks them on the switch's key
//!   (`PreparationPurpose::Switch`: its failure starts no ladder). Passed, the settings commit
//!   under the gate (`host.config_overrides`, `config.parameters.instance`,
//!   `device.backend_switched`, `command.validated`). Failed, or refused by the ledger: the
//!   candidate session closes, the registration takes the previous backends and they are
//!   prepared again (that preparation may start a ladder); the request is refused
//!   (`open_failed`, or `restore_failed` when the previous backends do not open either: that
//!   instance is unavailable, no other is touched, #419).
//! - **The restore at start** (§3.8). The backends in effect after the start's restore are
//!   written into the registry before any instance is prepared (`apply_restored_backends`); a
//!   restored backend that does not open leaves its instance unavailable, never the file's.

use super::recovery_ladder::PendingRecoveryLadder;
use super::resource_close::{PreparationLease, PreparationPurpose};
use super::resource_fence::resource_fence_error;
use super::selfcheck_emulator::EmulatorStatusRead;
use super::*;
use crate::codes::{BackendSwitchProblem, ConfigHostLocation, HostCode, HostLocation};
use actingcommand_contract::codes::RuntimeErrorCode;
use actingcommand_contract::{
    ConfigSettingChange, DeviceBackendChange, DeviceBackendSwitched, InstanceGate,
    NemuAppIndexSource, RecoveryTriggerStage,
};
use actingcommand_device::{CaptureBackendChoice, TouchBackendChoice};
use actingcommand_runtime_config::InstructionStamp;

/// An instance's two backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SwitchBackends {
    pub(super) touch: TouchBackendChoice,
    pub(super) capture: CaptureBackendChoice,
}

/// The switch a checked `ConfigureSettings` makes (design HS-M §3.2 step 1): the registered
/// instance and its backends before and after.
pub(super) struct BackendSwitchPlan {
    pub(super) instance_id: InstanceId,
    pub(super) instance_alias: String,
    pub(super) from: SwitchBackends,
    pub(super) to: SwitchBackends,
}

/// What the switch needs of its request: the changes it commits again, their stamp, the
/// command's links and the plan.
pub(super) struct SwitchRequest<'a> {
    pub(super) changes: &'a [ConfigSettingChange],
    pub(super) stamp: &'a InstructionStamp,
    pub(super) links: EventLinksDraft,
    pub(super) plan: BackendSwitchPlan,
}

/// `backend_switch_refused` for `instance_alias` with `problem` (design HS-M §3.6), projected as
/// the kind of failure it is.
fn refused(instance_alias: &str, problem: BackendSwitchProblem) -> RuntimeHostError {
    let runtime_code = match problem {
        BackendSwitchProblem::InstanceBusy => RuntimeErrorCode::QueueExpired,
        BackendSwitchProblem::PlayerHeld => RuntimeErrorCode::LeaseBusy,
        BackendSwitchProblem::InstanceFenced => RuntimeErrorCode::BackendOperationFailed,
        BackendSwitchProblem::EndpointUnavailable
        | BackendSwitchProblem::OpenFailed
        | BackendSwitchProblem::RestoreFailed => RuntimeErrorCode::BackendOpenFailed,
    };
    RuntimeHostError::request(
        HostCode::BackendSwitchRefused,
        HostLocation::SwitchInstanceBackends,
        runtime_code,
    )
    .with_outcome_values(|outcome| {
        outcome
            .alias(instance_alias)
            .backend_switch_problem(problem)
    })
}

/// A refusal `refused` with the failure behind it, when there is one.
fn caused(error: RuntimeHostError, cause: Option<Outcome>) -> RuntimeHostError {
    match cause {
        Some(cause) => error.with_caused_by(cause),
        None => error,
    }
}

/// How the previous backends came back after a failed candidate (design HS-M §3.2 step 5).
struct Restored {
    /// The restore's own failure: the candidate's close, the registration's switch back or the
    /// previous backends' preparation; `None` when they opened and passed again.
    failure: Option<Outcome>,
    /// The ladder the restore's preparation admitted, which the switch's key goes to.
    ladder: Option<PendingRecoveryLadder>,
}

/// What a commit under the gate came to.
enum Committed {
    Done(Box<OperationSuccess>),
    /// The changes no longer apply, or the ledger refused `host.config_overrides`: nothing was
    /// recorded or changed, and the request is refused in this state after the device's restore.
    Refused(Box<RuntimeHostError>, RuntimeReceiptState),
}

impl HostShared {
    /// #402 HS-2 (design HS-M §3.2): the switch of a checked `ConfigureSettings`, after its
    /// `command.received` (the gate is free). Every refusal is the command's `command.rejected`
    /// with the refusal as its outcome; only a failed record or an unconfirmed close no fence
    /// takes stops the Runtime.
    pub(super) fn switch_instance_backends(
        &self,
        original: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        connection_id: ConnectionId,
        request: SwitchRequest<'_>,
    ) -> Result<OperationSuccess, RequestFailure> {
        let SwitchRequest {
            changes,
            stamp,
            links,
            plan,
        } = request;
        let instance_alias = plan.instance_alias.as_str();
        // §3.1: a player's hold refuses at the entry, as it refuses a self-check; a fence or an
        // unavailable endpoint, whose key the switch would wait for in vain, too. The gates are
        // read again under the admission guard once the claim is granted (step 3.1).
        if let Some(error) = self.switch_gate(&plan)? {
            return Err(self.refuse_settings(links, error, RuntimeReceiptState::Denied));
        }
        // §3.2 step 2: the switch's claim, at most `CONNECTION_PREPARATION_CLAIM_WAIT_MS`.
        let token = match self.await_preparation_claim(
            original,
            instance_alias,
            connection_id,
            ClaimKind::BackendSwitch,
        ) {
            Ok(token) => token,
            Err(failure) if failure.poison_runtime => return Err(failure),
            Err(failure) => {
                let error = refused(instance_alias, BackendSwitchProblem::InstanceBusy)
                    .with_caused_by(failure.error.outcome());
                return Err(self.refuse_settings(links, error, RuntimeReceiptState::Denied));
            }
        };
        let request_id = original.request_id();
        // The switch shows as the self-check it is, from its grant.
        let _activity = self
            .begin_instance_activity(
                plan.instance_id,
                super::instance_activity::ActivityStart::Selfcheck {
                    request_id: Some(request_id),
                },
            )
            .map_err(RequestFailure::poison_without_terminal)?;
        let device_links = self
            .events
            .request_links(validated, Some(plan.instance_id), None, None);
        let key = SwitchKey {
            validated,
            request_id,
            token: &token,
            connection_id,
        };
        self.switch_on_claim(&key, changes, stamp, links, device_links, &plan)
    }

    /// The switch on its granted key (`key`), under the instance's admission guard for its
    /// device steps and its commit. The key is released (or handed to the restore's ladder)
    /// before the request answers.
    fn switch_on_claim(
        &self,
        key: &SwitchKey<'_, '_>,
        changes: &[ConfigSettingChange],
        stamp: &InstructionStamp,
        links: EventLinksDraft,
        device_links: EventLinksDraft,
        plan: &BackendSwitchPlan,
    ) -> Result<OperationSuccess, RequestFailure> {
        let instance_id = plan.instance_id;
        let instance_alias = plan.instance_alias.as_str();
        let instance_guard = self.instance_guard(instance_id)?;
        let admission = self.lock_admission(&instance_guard, instance_id)?;
        // §3.2 step 3.1: the control gates.
        if let Some(error) = self.switch_gate(plan)? {
            drop(admission);
            self.release_switch_key(key, None)?;
            return Err(self.refuse_settings(links, error, RuntimeReceiptState::Denied));
        }
        // Step 3.2: the kept session closes first (the multi-Nemu gate leaves none on most
        // instances).
        if let Err(close) = self.close_switch_session(key, instance_id, device_links.clone())? {
            drop(admission);
            self.release_switch_key(key, None)?;
            let problem = if self.resource_fenced(instance_id)? {
                BackendSwitchProblem::InstanceFenced
            } else {
                BackendSwitchProblem::OpenFailed
            };
            let error = refused(instance_alias, problem).with_caused_by(close);
            return Err(self.refuse_settings(links, error, RuntimeReceiptState::Failed));
        }
        // Step 3.3: the registration takes the candidate backends.
        let nemu_app_index = match self.execution()?.switch_instance_backends(
            instance_alias,
            plan.to.touch,
            plan.to.capture,
        ) {
            Ok(index) => index,
            Err(error) => {
                drop(admission);
                self.release_switch_key(key, None)?;
                let cause =
                    RuntimeHostError::execution(HostLocation::SwitchInstanceBackends, &error);
                let error = refused(instance_alias, BackendSwitchProblem::OpenFailed)
                    .with_caused_by(cause.outcome());
                return Err(self.refuse_settings(links, error, RuntimeReceiptState::Failed));
            }
        };
        // Step 3.4: the candidate's preparation on the switch's key; its failure starts no
        // ladder.
        let candidate = self
            .prepare_connection_attempt(
                instance_alias,
                instance_id,
                device_links.clone(),
                &admission,
                RecoveryTriggerStage::ConnectionPreparation,
                PreparationLease::Held {
                    token: key.token,
                    connection_id: key.connection_id,
                },
                EmulatorStatusRead::First,
                PreparationPurpose::Switch,
            )
            .map_err(RequestFailure::poison_without_terminal)?;
        // Step 4: the commit, under the gate.
        let refusal = if candidate.passed() {
            // #402 HS-3: the file's index, else the one the candidate's paired open resolved.
            let (nemu_app_index, nemu_app_index_source) = match nemu_app_index {
                Some(index) => (Some(index), Some(NemuAppIndexSource::File)),
                None => match self.execution()?.resolved_nemu_app_index(instance_alias) {
                    Some(index) => (Some(index), Some(NemuAppIndexSource::Resolved)),
                    None => (None, None),
                },
            };
            let switched = DeviceBackendSwitched {
                instance_alias: instance_alias.to_owned(),
                touch: DeviceBackendChange {
                    from: plan.from.touch.as_str().to_owned(),
                    to: plan.to.touch.as_str().to_owned(),
                },
                capture: DeviceBackendChange {
                    from: plan.from.capture.as_str().to_owned(),
                    to: plan.to.capture.as_str().to_owned(),
                },
                nemu_app_index,
                nemu_app_index_source,
                session_generation: candidate.generation.unwrap_or_default(),
            };
            match self.commit_switch(changes, stamp, &links, device_links.clone(), switched)? {
                Committed::Done(success) => {
                    drop(admission);
                    self.release_switch_key(key, None)?;
                    return Ok(*success);
                }
                Committed::Refused(error, state) => (*error, state),
            }
        } else {
            let error = caused(
                refused(instance_alias, BackendSwitchProblem::OpenFailed),
                candidate.failure.clone(),
            );
            (error, RuntimeReceiptState::Failed)
        };
        // Step 5: the previous backends come back.
        let restored = self.restore_backends(key, plan, device_links, &admission)?;
        drop(admission);
        // A failed restore that admitted no ladder first cancels the instance's queued startup
        // claim, as a self-check's failed preparation does, so the release never hands the key
        // to a startup run on an instance whose backends do not open.
        if restored.failure.is_some() && restored.ladder.is_none() {
            self.cancel_queued_startup_claims(instance_id)?;
        }
        self.release_switch_key(key, restored.ladder)?;
        let (error, state) = refusal;
        let error = match restored.failure {
            None => error,
            // Step 5.4: the previous backends did not open either.
            Some(restore) if candidate.passed() => error.with_note(restore),
            // Both failures are its causes: the candidate's, then the restore's.
            Some(restore) => {
                let candidate_failure = candidate.failure;
                refused(instance_alias, BackendSwitchProblem::RestoreFailed)
                    .with_outcome_values(|outcome| {
                        let outcome = outcome
                            .alias(instance_alias)
                            .backend_switch_problem(BackendSwitchProblem::RestoreFailed);
                        match candidate_failure {
                            Some(candidate_failure) => outcome.caused_by(candidate_failure),
                            None => outcome,
                        }
                    })
                    .with_caused_by(restore)
            }
        };
        Err(self.refuse_settings(links, error, state))
    }

    /// The control gates of a switch (design HS-M §3.1, §3.2 step 3.1), read under the
    /// instance's admission guard: a player's hold, a #403 fence, a pending endpoint or an
    /// unbound handset. `None`: the switch may go on.
    fn switch_gate(&self, plan: &BackendSwitchPlan) -> RuntimeHostResult<Option<RuntimeHostError>> {
        let instance_alias = plan.instance_alias.as_str();
        if self.player_holds(instance_alias)? {
            return Ok(Some(
                refused(instance_alias, BackendSwitchProblem::PlayerHeld)
                    .with_caused_by(Outcome::new(HostCode::PlayerHoldActive).alias(instance_alias)),
            ));
        }
        if self.resource_fenced(plan.instance_id)? {
            return Ok(Some(
                refused(instance_alias, BackendSwitchProblem::InstanceFenced).with_caused_by(
                    resource_fence_error(
                        plan.instance_id,
                        RuntimeErrorCode::BackendOperationFailed,
                    )
                    .outcome(),
                ),
            ));
        }
        let registry = lock(
            &self.registered_instances,
            HostLocation::SwitchInstanceBackends,
        )?;
        let unavailable = registry.get(&plan.instance_id).is_none_or(|registered| {
            registered
                .adb_endpoint
                .as_ref()
                .is_some_and(ResolvedInstanceEndpoint::is_pending)
                || registered
                    .bound_adb_endpoint()
                    .and_then(ResolvedAdbEndpoint::handset)
                    .is_some_and(|handset| handset.binding().is_none())
        });
        drop(registry);
        Ok(unavailable.then(|| refused(instance_alias, BackendSwitchProblem::EndpointUnavailable)))
    }

    /// Whether a player holds `instance_alias`: the player hold's dimension of its control gates
    /// (false until R-7b brings the hold).
    fn player_holds(&self, instance_alias: &str) -> RuntimeHostResult<bool> {
        Ok(
            lock(&self.scheduling_pause, HostLocation::SwitchInstanceBackends)?
                .control_gates(instance_alias, false)
                .device
                .contains(&InstanceGate::PlayerHold),
        )
    }

    /// Closes the instance's kept session through the fenced close path on the switch's key.
    /// `Err` inside: the close's failure (its resources confirmed, or the instance fenced); an
    /// unconfirmed close that fenced nothing retained the owner and marked the Runtime fatal on
    /// its path, and stops the request.
    fn close_switch_session(
        &self,
        key: &SwitchKey<'_, '_>,
        instance_id: InstanceId,
        links: EventLinksDraft,
    ) -> Result<Result<(), Outcome>, RequestFailure> {
        let open = self
            .execution()?
            .has_session(instance_id)
            .map_err(|error| {
                RuntimeHostError::execution(HostLocation::SwitchInstanceBackends, &error)
            })?;
        if !open {
            return Ok(Ok(()));
        }
        match self.close_instance_resources_result(key.token, key.connection_id, links)? {
            Ok(()) => Ok(Ok(())),
            Err(close_error) => {
                let failure =
                    RuntimeHostError::execution(HostLocation::SwitchInstanceBackends, &close_error);
                if close_error.resource_quiescence() == Some(ResourceQuiescence::Unconfirmed)
                    && !self.resource_fenced(instance_id)?
                {
                    return Err(RequestFailure::poison_without_terminal(
                        failure.into_fatal(),
                    ));
                }
                Ok(Err(failure.outcome()))
            }
        }
    }

    /// The settings commit of a switch whose candidate passed (design HS-M §3.2 step 4): under
    /// the gate the changes apply again over the configuration in effect now (no version is
    /// compared; the instance's backend keys change only under the switch's claim), then
    /// `host.config_overrides`, `config.parameters.instance`, `device.backend_switched` and
    /// `command.validated` are recorded and the values take effect.
    fn commit_switch(
        &self,
        changes: &[ConfigSettingChange],
        stamp: &InstructionStamp,
        links: &EventLinksDraft,
        device_links: EventLinksDraft,
        switched: DeviceBackendSwitched,
    ) -> Result<Committed, RequestFailure> {
        let context = self.config_check()?;
        let _gate = lock(
            &self.config_override_gate,
            ConfigHostLocation::ConfigureSettings,
        )?;
        let current = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?.clone();
        let applied = match self.apply_settings(&current, changes, stamp, &context) {
            Ok(applied) => applied,
            Err(error) if error.is_fatal() => {
                return Err(RequestFailure::poison_without_terminal(error));
            }
            Err(error) => {
                return Ok(Committed::Refused(
                    Box::new(error),
                    RuntimeReceiptState::Denied,
                ));
            }
        };
        if let Some(failure) = self.persist_settings(&applied)? {
            return Ok(Committed::Refused(
                Box::new(failure),
                RuntimeReceiptState::Failed,
            ));
        }
        self.commit_settings(applied, links.clone(), Some((device_links, switched)))
            .map(|success| Committed::Done(Box::new(success)))
    }

    /// Design HS-M §3.2 step 5: the candidate's session closes, the registration takes the
    /// previous backends, and they are prepared again on the switch's key, as a self-check is
    /// (its failure may start a ladder, which the key then goes to).
    fn restore_backends(
        &self,
        key: &SwitchKey<'_, '_>,
        plan: &BackendSwitchPlan,
        links: EventLinksDraft,
        admission: &MutexGuard<'_, ()>,
    ) -> Result<Restored, RequestFailure> {
        let instance_alias = plan.instance_alias.as_str();
        if let Err(close) = self.close_switch_session(key, plan.instance_id, links.clone())? {
            return Ok(Restored {
                failure: Some(close),
                ladder: None,
            });
        }
        if let Err(error) = self.execution()?.switch_instance_backends(
            instance_alias,
            plan.from.touch,
            plan.from.capture,
        ) {
            let failure = RuntimeHostError::execution(HostLocation::SwitchInstanceBackends, &error);
            // The registration kept the candidate: the instance stays unavailable (its failed
            // self-check withheld it) until a later switch or a restart.
            self.withhold_policy_instance_availability(plan.instance_id)?;
            return Ok(Restored {
                failure: Some(failure.outcome()),
                ladder: None,
            });
        }
        let again = self
            .prepare_connection_attempt(
                instance_alias,
                plan.instance_id,
                links,
                admission,
                RecoveryTriggerStage::ConnectionPreparation,
                PreparationLease::Held {
                    token: key.token,
                    connection_id: key.connection_id,
                },
                EmulatorStatusRead::First,
                PreparationPurpose::Connection,
            )
            .map_err(RequestFailure::poison_without_terminal)?;
        // A failed preparation carries its failing step's outcome (`PreparationAttempt::passed`).
        Ok(Restored {
            failure: again.failure,
            ladder: again.ladder,
        })
    }

    /// Releases the switch's key, or hands it to the ladder the restore's preparation admitted
    /// (Workflow #369 H-1).
    fn release_switch_key(
        &self,
        key: &SwitchKey<'_, '_>,
        ladder: Option<PendingRecoveryLadder>,
    ) -> Result<(), RequestFailure> {
        match ladder {
            Some(ladder) => self.release_to_preparation_ladder(
                key.validated,
                key.request_id,
                key.token,
                key.connection_id,
                ladder,
            ),
            None => self
                .release_lease(
                    key.validated,
                    key.request_id,
                    key.token,
                    key.connection_id,
                    None,
                )
                .map(|_| ()),
        }
    }

    /// #402 HS-2 (design HS-M §3.8): at start, after the instruction values are restored and
    /// before any instance is prepared, the registry takes the backends in effect of each
    /// instance whose registration names others (a restored switch). No session exists yet, so
    /// no device is touched; the start's preparation then opens the backends in effect, and one
    /// that does not open leaves its instance unavailable (never the file's backends). The
    /// registration refusing backends the configuration's own check passed is an internal
    /// invariant: the start fails, as for a registration it refuses from the file.
    pub(super) fn apply_restored_backends(&self) -> RuntimeHostResult<()> {
        for (instance_alias, backends) in self.backends_in_effect()? {
            let resolved = self
                .execution()?
                .resolve(&instance_alias)
                .map_err(|error| {
                    RuntimeHostError::execution(HostLocation::ApplyRestoredBackends, &error)
                })?;
            let Some(configuration) = resolved.configuration() else {
                continue;
            };
            if configuration.input_backend == backends.touch.as_str()
                && configuration.capture_backend == backends.capture.as_str()
            {
                continue;
            }
            self.execution()?
                .switch_instance_backends(&instance_alias, backends.touch, backends.capture)
                .map_err(|error| {
                    RuntimeHostError::execution(HostLocation::ApplyRestoredBackends, &error)
                        .into_fatal()
                })?;
        }
        Ok(())
    }
}

/// The switch's granted key and the request it belongs to.
struct SwitchKey<'r, 'v> {
    validated: &'r ValidatedRuntimeRequest<'v>,
    request_id: RequestId,
    token: &'r LeaseToken,
    connection_id: ConnectionId,
}

/// #402 HS-2, test builds: a player's hold, a #403 fence and the multi-Nemu gate, as a test
/// sets and reads them.
#[cfg(test)]
impl RuntimeHost {
    /// A player holds `instance_alias` (`held`), or no longer does: the player hold's dimension
    /// of its control gates, which R-7b will set.
    pub(crate) fn hold_player_for_test(
        &self,
        instance_alias: &str,
        held: bool,
    ) -> RuntimeHostResult<()> {
        let location = crate::codes::test_spellings::TestLocation::HoldPlayerForTest;
        let shared = self.shared_ref(location)?;
        lock(&shared.scheduling_pause, location)?.hold_player_for_test(instance_alias, held);
        Ok(())
    }

    /// Fences `instance_alias` (#403) as a failure of its own work does: an instance fault of its
    /// capture backend (as ISO-M's containment tests fence one), which the fence's record names
    /// as its cause.
    pub(crate) fn fence_instance_for_test(&self, instance_alias: &str) -> RuntimeHostResult<()> {
        let location = crate::codes::test_spellings::TestLocation::FenceInstanceForTest;
        let shared = self.shared_ref(location)?;
        let instance_id = shared
            .resolve_instance(instance_alias)
            .map_err(|failure| *failure.error)?
            .instance_id();
        let failure = RuntimeHostError::instance_fault(
            actingcommand_contract::codes::SharedCode::CaptureBackendOperationFailed,
            location,
            RuntimeErrorCode::CaptureFailed,
            instance_id,
        );
        shared
            .fence_contained_failure(&failure, instance_id, shared.events.system_links()?)?
            .map(|_| ())
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    crate::codes::test_spellings::TestCode::TestInstanceMissing,
                    location,
                    RuntimeErrorCode::RuntimeHalted,
                )
            })
    }

    /// The ledger refuses the next record of `host.config_overrides` (not as the Runtime's
    /// fatal): the commit's `host_config_overrides_persist_failed`.
    pub(crate) fn fail_next_config_persist_for_test(&self) -> RuntimeHostResult<()> {
        self.shared_ref(crate::codes::test_spellings::TestLocation::FailNextConfigPersistForTest)?
            .config_persist_failure_for_test
            .store(true, Ordering::Release);
        Ok(())
    }

    /// Whether the multi-Nemu gate keeps `instance_alias`'s session at a lease end now.
    pub(crate) fn keeps_device_session_for_test(
        &self,
        instance_alias: &str,
    ) -> RuntimeHostResult<bool> {
        let location = crate::codes::test_spellings::TestLocation::KeepsDeviceSessionForTest;
        let shared = self.shared_ref(location)?;
        let instance_id = shared
            .resolve_instance(instance_alias)
            .map_err(|failure| *failure.error)?
            .instance_id();
        shared.keeps_device_session(instance_id)
    }
}
