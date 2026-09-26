// SPDX-License-Identifier: AGPL-3.0-only

use super::policy_dispatch::scheduling_resume_selfcheck;
use super::*;
use actingcommand_contract::SchedulingResumeSelfCheck;

impl HostShared {
    pub(super) fn cleanup_composite_failure(
        &self,
        token: LeaseToken,
        connection_id: ConnectionId,
        failure: RequestFailure,
    ) -> RequestFailure {
        match self.retain_unconfirmed_resources(&failure.error, EventLinksDraft::default()) {
            Ok(true) => return failure,
            Ok(false) => {}
            Err(retain_failure) => return failure.replace_with_poison(retain_failure),
        }
        match self.cleanup_token(&token, connection_id, LeaseReleaseReason::BackendFailure) {
            Ok(()) => failure,
            Err(error) => failure.replace_with_poison(error),
        }
    }

    pub(super) fn cleanup_composite_failure_with_run_links(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: LeaseToken,
        connection_id: ConnectionId,
        run_links: Option<RuntimeRunLinks>,
        failure: RequestFailure,
    ) -> RequestFailure {
        match self.retain_unconfirmed_resources(&failure.error, EventLinksDraft::default()) {
            Ok(true) => return failure,
            Ok(false) => {}
            Err(retain_failure) => return failure.replace_with_poison(retain_failure),
        }
        let cleanup = match run_links {
            Some(run_links) => self.cleanup_scheduled_failure_with_run_links(
                request,
                &token,
                connection_id,
                run_links,
            ),
            None => self.cleanup_token(&token, connection_id, LeaseReleaseReason::BackendFailure),
        };
        match cleanup {
            Ok(()) => failure,
            Err(error) => failure.replace_with_poison(error),
        }
    }

    pub(super) fn finish_destructive_input(
        &self,
        step: Option<Arc<FencedWrite>>,
        connection_id: ConnectionId,
    ) -> Result<(), RequestFailure> {
        let Some(witness) = step.and_then(|step| Arc::try_unwrap(step).ok()) else {
            let mut error = RuntimeHostError::fatal(
                "fenced_step_not_reclaimed",
                "finish_destructive_input",
                RuntimeErrorCode::RuntimeFatal,
            );
            error.lifecycle.resource_quiescence = Some(ResourceQuiescence::Unconfirmed);
            self.retain_unconfirmed_resources(&error, EventLinksDraft::default())
                .map_err(RequestFailure::poison_without_terminal)?;
            return Err(RequestFailure::poison_without_terminal(error));
        };
        lock(&self.scheduler, "finish_destructive_input")?
            .finish_destructive_step(witness, connection_id)
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                    "finish_destructive_input",
                    &error,
                ))
            })
    }

    pub(super) fn mark_resources_in_use(&self) -> RuntimeHostResult<MutexGuard<'_, OwnerGuard>> {
        let mut owner = lock(&self.owner, "mark_owner_resources_in_use")?;
        owner.set_resource_disposition(OwnerResourceDisposition::InUse)?;
        Ok(owner)
    }

    fn record_owner_resource_close(&self) -> RuntimeHostResult<OwnerResourceDisposition> {
        // The same owner lock spans InUse and session registration on every acquisition.
        let mut owner = lock(&self.owner, "record_owner_resource_close")?;
        if let Some(disposition) = owner.retained_resource_disposition()? {
            return Ok(disposition);
        }
        let has_sessions = self.execution.has_sessions().map_err(|error| {
            RuntimeHostError::execution("inspect_remaining_execution_sessions", &error)
        })?;
        let disposition = if has_sessions {
            OwnerResourceDisposition::InUse
        } else {
            OwnerResourceDisposition::ConfirmedClosed
        };
        owner.set_resource_disposition(disposition)?;
        Ok(disposition)
    }

    pub(super) fn retain_unconfirmed_resources(
        &self,
        error: &RuntimeHostError,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<bool> {
        if error.lifecycle.resource_quiescence != Some(ResourceQuiescence::Unconfirmed) {
            return Ok(false);
        }
        let error = error.clone().into_fatal();
        let lifecycle_result = self.append_lifecycle_failure(
            RuntimeLifecycleFailureStage::SessionClose,
            RuntimeLifecycleFailure::Host(&error),
            links,
            None,
        );
        let retain_result = lock(&self.owner, "retain_unconfirmed_owner")
            .and_then(|mut owner| owner.retain_unconfirmed());
        let fatal_result = self.fatal.mark(error.clone());
        lifecycle_result?;
        retain_result?;
        fatal_result?;
        Ok(true)
    }

    pub(super) fn close_instance_resources(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
        links: EventLinksDraft,
    ) -> Result<(), RequestFailure> {
        self.close_instance_resources_result(token, connection_id, links)?
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::execution(
                    "close_execution_session",
                    &error,
                ))
            })
    }

    pub(super) fn close_instance_resources_result(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
        links: EventLinksDraft,
    ) -> Result<Result<(), ExecutionKernelError>, RequestFailure> {
        if let Some(error) = self
            .execution
            .unconfirmed_instance_close_error(token.instance_id())
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::execution(
                    "read_execution_close_result",
                    &error,
                ))
            })?
        {
            return Ok(Err(error));
        }
        let has_session = self
            .execution
            .has_owned_resources(token.instance_id())
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::execution(
                    "inspect_execution_session",
                    &error,
                ))
            })?;
        if !has_session {
            self.record_owner_resource_close()?;
            return Ok(Ok(()));
        }
        let witness = Arc::new(
            lock(&self.scheduler, "begin_destructive_resource_close")?
                .begin_resource_close(token, connection_id, self.monotonic_ms()?)
                .map_err(|error| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                        "begin_destructive_resource_close",
                        &error,
                    ))
                })?,
        );

        match self.execution.close_instance_with_input_check(
            token.instance_id(),
            DeviceCloseAuthority::FencedDeviceWrite(Arc::clone(&witness)),
            self.nemu_close_check(token, Arc::clone(&witness), connection_id)
                .map_err(RequestFailure::poison_without_terminal)?,
        ) {
            Ok(outcome) => {
                self.append_stdio_close_observations(
                    outcome.vendor_stdio(),
                    Some(token.instance_id()),
                    links.clone(),
                )
                .map_err(RequestFailure::poison_without_terminal)?;
                let owner_disposition = self.record_owner_resource_close()?;
                self.append_lifecycle_observed(
                    RuntimeLifecyclePhase::ResourceQuiescence {
                        instance_id: token.instance_id(),
                        resource_count: outcome.resource_count(),
                        quiescence: outcome.quiescence(),
                        owner_disposition,
                    },
                    links,
                )
                .map_err(RequestFailure::poison_without_terminal)?;
                self.finish_destructive_input(Some(witness), connection_id)?;
                Ok(Ok(()))
            }
            Err(execution_error) => {
                let error =
                    RuntimeHostError::execution("close_execution_session", &execution_error);
                let lifecycle_result = self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::SessionClose,
                    RuntimeLifecycleFailure::Host(&error),
                    links,
                    None,
                );
                if error.lifecycle.resource_quiescence == Some(ResourceQuiescence::Unconfirmed) {
                    let retain_result = lock(&self.owner, "retain_unconfirmed_owner")
                        .and_then(|mut owner| owner.retain_unconfirmed());
                    let fatal_result = self.fatal.mark(error.clone());
                    lifecycle_result.map_err(RequestFailure::poison_without_terminal)?;
                    retain_result.map_err(RequestFailure::poison_without_terminal)?;
                    fatal_result.map_err(RequestFailure::poison_without_terminal)?;
                    return Ok(Err(execution_error));
                }
                lifecycle_result.map_err(RequestFailure::poison_without_terminal)?;
                self.record_owner_resource_close()?;
                self.finish_destructive_input(Some(witness), connection_id)?;
                Ok(Err(execution_error))
            }
        }
    }

    /// Grants a dedicated lease on `instance_id` to the Runtime-owned connection
    /// `connection_value`: a resource-close-only lease (`prepare_resource_close`, a waiting lease
    /// queue is `TransferNotSafe`), granted with `CapacityUse::Drain`. The caller holds the
    /// instance admission guard. Returns the token, its connection and the grant's request id.
    pub(super) fn grant_dedicated_instance_lease(
        &self,
        instance_id: InstanceId,
        connection_value: u64,
    ) -> RuntimeHostResult<(LeaseToken, ConnectionId, RequestId)> {
        let resolved = lock(&self.registered_instances, "read_resource_close_instance")?
            .get(&instance_id)
            .cloned()
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    "resource_close_instance_missing",
                    "acquire_resource_close_lease",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
        let request_id = self
            .events
            .issuer()
            .mint_request_id()
            .map_err(|_| runtime_identifier_error())?;
        let holder_id = self
            .events
            .issuer()
            .mint_holder_id()
            .map_err(|_| runtime_identifier_error())?;
        let connection_id = ConnectionId::new(connection_value).map_err(|error| {
            RuntimeHostError::scheduler("build_resource_close_connection", &error)
        })?;
        let preparation = lock(&self.scheduler, "prepare_resource_close_lease")?
            .prepare_resource_close(
                *request_id.transport(),
                instance_id,
                *holder_id.transport(),
                connection_id,
                self.monotonic_ms()?,
            )
            .map_err(|error| RuntimeHostError::scheduler("prepare_resource_close_lease", &error))?;
        let token = preparation.token().clone();
        let grant_request_id = *request_id.transport();
        let grant_links = self
            .events
            .synthetic_links(&token, self.events.action_id()?)?
            .with_request_id(request_id);
        self.grant_prepared_lease_with_links(
            &resolved,
            preparation,
            grant_links,
            CapacityUse::Drain,
        )
        .map_err(|failure| *failure.error)?;
        Ok((token, connection_id, grant_request_id))
    }

    /// The instance admission guard excludes capture registration and business native calls.
    /// A dedicated close lease is released with `release_reason` once the close is confirmed.
    pub(super) fn close_retained_instance_while_guarded(
        &self,
        instance_id: InstanceId,
        links: EventLinksDraft,
        reuse_active_lease: bool,
        release_reason: LeaseReleaseReason,
        admission: &MutexGuard<'_, ()>,
    ) -> RuntimeHostResult<Result<(), ExecutionKernelError>> {
        if !self
            .execution
            .has_owned_resources(instance_id)
            .map_err(|error| RuntimeHostError::execution("inspect_retained_session", &error))?
        {
            return Ok(Ok(()));
        }
        let active = lock(&self.scheduler, "read_resource_close_lease")?
            .active_tokens()
            .into_iter()
            .find(|token| token.instance_id() == instance_id);
        let (token, connection_id, acquired) = if let Some(token) = active {
            if !reuse_active_lease {
                return Err(RuntimeHostError::scheduler(
                    "acquire_resource_close_lease",
                    &SchedulerError::Busy {
                        holder_id: token.holder_id(),
                        lease_id: token.lease_id(),
                        expires_at_monotonic_ms: token.expires_at_monotonic_ms(),
                    },
                ));
            }
            let connection_id = lock(&self.scheduler, "read_resource_close_connection")?
                .connection_for_token(&token)
                .map_err(|error| {
                    RuntimeHostError::scheduler("read_resource_close_connection", &error)
                })?;
            (token, connection_id, false)
        } else {
            let (token, connection_id, _) =
                self.grant_dedicated_instance_lease(instance_id, RESOURCE_CLOSE_CONNECTION_VALUE)?;
            (token, connection_id, true)
        };
        let result = self
            .close_instance_resources_result(&token, connection_id, links)
            .map_err(|failure| *failure.error)?;
        let confirmed = result
            .as_ref()
            .err()
            .is_none_or(|error| error.resource_quiescence() == Some(ResourceQuiescence::Confirmed));
        if acquired && confirmed {
            self.cleanup_token_inner(&token, connection_id, release_reason, None, Some(admission))?;
        }
        Ok(result)
    }

    pub(super) fn finish_input_failure(
        &self,
        primary: ExecutionKernelError,
        token: &LeaseToken,
        step: Option<Arc<FencedWrite>>,
        connection_id: ConnectionId,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<ExecutionKernelError> {
        if primary.resource_quiescence() == Some(ResourceQuiescence::Unconfirmed) {
            return Ok(primary);
        }
        let close_result: RuntimeHostResult<Result<(), ExecutionKernelError>> = (|| {
            let instance_guard = self
                .instance_guard(token.instance_id())
                .map_err(|failure| *failure.error)?;
            let _admission = lock(&instance_guard, "lock_instance_admission")?;
            self.finish_destructive_input(step, connection_id)
                .map_err(|failure| *failure.error)?;
            self.close_instance_resources_result(token, connection_id, links.clone())
                .map_err(|failure| *failure.error)
        })();
        match close_result {
            Ok(Ok(())) => Ok(primary),
            Ok(Err(cleanup)) => Ok(ExecutionKernelError::merge_cleanup(primary, cleanup)),
            Err(cleanup) => {
                let primary = RuntimeHostError::execution("execute_input_backend", &primary);
                self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::SessionClose,
                    RuntimeLifecycleFailure::Host(&primary),
                    links.clone(),
                    None,
                )?;
                let cleanup = cleanup.into_fatal();
                self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::SessionClose,
                    RuntimeLifecycleFailure::Host(&cleanup),
                    links,
                    None,
                )?;
                lock(&self.owner, "retain_unconfirmed_owner")?.retain_unconfirmed()?;
                self.fatal.mark(cleanup.clone())?;
                Err(cleanup)
            }
        }
    }

    pub(super) fn finish_capture_failure_while_guarded(
        &self,
        primary: ExecutionKernelError,
        links: EventLinksDraft,
        admission: &MutexGuard<'_, ()>,
    ) -> RuntimeHostResult<ExecutionKernelError> {
        if primary.code() == "execution_session_close_pending" {
            return Ok(primary);
        }
        let Some(instance_id) = primary.instance_id() else {
            return Ok(primary);
        };
        match self.close_retained_instance_while_guarded(
            instance_id,
            links.clone(),
            true,
            LeaseReleaseReason::HostShutdown,
            admission,
        ) {
            Ok(Ok(())) => Ok(primary),
            Ok(Err(cleanup)) => Ok(ExecutionKernelError::merge_cleanup(primary, cleanup)),
            Err(cleanup) => {
                let primary = RuntimeHostError::execution("finish_capture_failure", &primary);
                self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::SessionClose,
                    RuntimeLifecycleFailure::Host(&primary),
                    links.clone(),
                    None,
                )?;
                let cleanup = cleanup.into_fatal();
                self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::SessionClose,
                    RuntimeLifecycleFailure::Host(&cleanup),
                    links,
                    None,
                )?;
                lock(&self.owner, "retain_unconfirmed_owner")?.retain_unconfirmed()?;
                self.fatal.mark(cleanup.clone())?;
                Err(cleanup)
            }
        }
    }

    // The controlled preparation phase of a physical instance's device connection (Workflow #317
    // sc3, the strict reading of #316 goal 5).
    //
    // `prepare_instance_connection` connects and self-checks one physical instance outside any
    // client lease: under the instance admission guard it takes a dedicated preparation lease (the
    // close-lease precedent: a resource-close-only lease of a fixed Runtime connection, granted
    // with `CapacityUse::Drain`; a waiting lease queue is `TransferNotSafe`), opens the instance's
    // input and capture backends through `ExecutionKernel::open_instance_backends` (a Nemu pair
    // once), records the opens exactly as every open is recorded (`backend.open_observed`, the
    // `backend.selfcheck.*` facts, one `device.self_check` status hint per entry and the policy
    // availability they gate), then closes the session and releases the lease. It sends no input
    // and keeps no frame. A failing step is recorded and leaves the instance unavailable; nothing
    // is retried. Only a fatal failure (a ledger append, an unconfirmed close) is returned.
    //
    // Triggers: daemon start (every registered physical instance in order, before the host
    // answers), emulator control `start` / `restart`, an instance `ResumeScheduling` and the
    // explicit `SelfCheckInstance` request. The automatic triggers and the availability gate
    // concern device self-checked instances only (`RegisteredInstance::device_self_checked`):
    // fixtures and providers without a device endpoint are exempt.

    /// Trigger (a): at daemon start, after the registry and the fact seeds and before any
    /// thread is spawned, withdraws the policy availability of every registered physical
    /// instance and runs its preparation phase, in registry order. A failed preparation leaves
    /// its instance unavailable and does not stop the start; only a fatal failure does.
    pub(super) fn prepare_physical_instances_on_start(&self) -> RuntimeHostResult<()> {
        let instances = lock(&self.registered_instances, "list_physical_instances")?
            .values()
            .filter(|instance| instance.device_self_checked())
            .map(|instance| (instance.instance_alias.clone(), instance.instance_id))
            .collect::<Vec<_>>();
        for (instance_alias, instance_id) in instances {
            self.withhold_policy_instance_availability(instance_id)?;
            let links = self
                .events
                .system_links()?
                .with_instance_id(self.events.issuer().issue_registered_instance(instance_id));
            let instance_guard = self
                .instance_guard(instance_id)
                .map_err(|failure| *failure.error)?;
            let admission = lock(&instance_guard, "lock_instance_admission")?;
            self.prepare_instance_connection(&instance_alias, instance_id, links, &admission)?;
        }
        Ok(())
    }

    /// Trigger (d): `SelfCheckInstance` — the operator's manual reconnect and self-check of one
    /// physical instance; the receipt carries the self-check in the resume receipt's shape.
    pub(super) fn self_check_instance(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
    ) -> Result<OperationSuccess, RequestFailure> {
        let instance_id = self.resolve_instance(instance_alias)?.instance_id();
        let instance_guard = self.instance_guard(instance_id)?;
        let admission = lock(&instance_guard, "lock_instance_admission")?;
        let links = self
            .events
            .request_links(request, Some(instance_id), None, None);
        let selfcheck = self
            .prepare_instance_connection(instance_alias, instance_id, links, &admission)
            .map_err(RequestFailure::poison_without_terminal)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::InstanceSelfChecked {
                instance_alias: instance_alias.to_owned(),
                selfcheck,
            },
        })
    }

    /// The preparation phase of one physical instance; the caller holds its admission guard.
    /// The opens are recorded under `links` (which name the instance). Returns the self-check
    /// projected from the opens it made, with the code of the failing step, if any.
    pub(super) fn prepare_instance_connection(
        &self,
        instance_alias: &str,
        instance_id: InstanceId,
        links: EventLinksDraft,
        admission: &MutexGuard<'_, ()>,
    ) -> RuntimeHostResult<SchedulingResumeSelfCheck> {
        let frame_owner = self
            .events
            .issuer()
            .mint_request_id()
            .map_err(|_| runtime_identifier_error())?;
        // The frame memory owner of every capture path: the first frame of a capture opened here
        // is charged to it and dropped inside the open.
        let frame_store = actingcommand_artifact_store::FrameStore::new(
            frame_retention::spill_root(self.artifacts.root(), frame_owner.transport())
                .map_err(RuntimeHostError::artifact)?,
            frame_retention::capture_frame_store_config(),
        )
        .map_err(RuntimeHostError::artifact)?;
        let (token, connection_id, _) = match self
            .grant_dedicated_instance_lease(instance_id, CONNECTION_PREPARATION_CONNECTION_VALUE)
        {
            Ok(granted) => granted,
            Err(error) if error.is_fatal() => return Err(error),
            Err(error) => {
                let failure_code = error.code();
                self.record_connection_preparation_failure(instance_id, links, error)?;
                return Ok(scheduling_resume_selfcheck(&[], Some(failure_code)));
            }
        };
        let registration = self.mark_resources_in_use()?;
        let (observations, mut failure_code) = match self.execution.open_instance_backends(
            instance_alias,
            registration,
            frame_store.memory_budget(),
        ) {
            Ok(observations) => {
                self.append_backend_open_observations(
                    &observations,
                    links.clone(),
                    EventSource::Device,
                    OriginModule::DeviceProxy,
                )?;
                (observations, None)
            }
            Err(error) => {
                self.append_backend_open_failure_observations(
                    &error,
                    links.clone(),
                    EventSource::Device,
                    OriginModule::DeviceProxy,
                )?;
                (
                    error.failure_context().backend_open_observations().to_vec(),
                    Some(error.code()),
                )
            }
        };
        // Releasing the preparation lease closes the session; the self-check facts stay. The
        // lease is released only once the close is confirmed, as for every dedicated close
        // lease; its queue is empty (checked at the grant, and the admission guard is held).
        let closed = self
            .close_instance_resources_result(&token, connection_id, links.clone())
            .map_err(|failure| *failure.error)?;
        let confirmed = closed
            .as_ref()
            .err()
            .is_none_or(|error| error.resource_quiescence() == Some(ResourceQuiescence::Confirmed));
        if confirmed {
            self.cleanup_token_inner(
                &token,
                connection_id,
                LeaseReleaseReason::HostShutdown,
                None,
                Some(admission),
            )?;
        }
        if let Err(close_error) = closed {
            // The close path recorded the failure; an unconfirmed close retained the owner and
            // marked the Runtime fatal, as on every close path.
            if close_error.resource_quiescence() == Some(ResourceQuiescence::Unconfirmed) {
                return Err(RuntimeHostError::execution(
                    "close_prepared_instance_session",
                    &close_error,
                )
                .into_fatal());
            }
            failure_code = failure_code.or(Some(close_error.code()));
        }
        // An open failure is already a `failed` self-check; any other failing step (a failed
        // open without reports, a failed close) still leaves the instance unavailable.
        if failure_code.is_some() {
            self.withhold_policy_instance_availability(instance_id)?;
        }
        Ok(scheduling_resume_selfcheck(&observations, failure_code))
    }

    /// A preparation step other than an open failed without being fatal (the preparation lease
    /// was refused): one `runtime.failed` record with stage
    /// `runtime.lifecycle.connection_preparation` names the instance and the code, and the
    /// instance stays or becomes unavailable.
    fn record_connection_preparation_failure(
        &self,
        instance_id: InstanceId,
        links: EventLinksDraft,
        mut error: RuntimeHostError,
    ) -> RuntimeHostResult<()> {
        error.lifecycle.instance_id = Some(instance_id);
        self.append_lifecycle_failure(
            RuntimeLifecycleFailureStage::ConnectionPreparation,
            RuntimeLifecycleFailure::Host(&error),
            links,
            None,
        )?;
        self.withhold_policy_instance_availability(instance_id)
    }
}
