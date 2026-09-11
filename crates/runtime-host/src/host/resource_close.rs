// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

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
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> Result<(), RequestFailure> {
        lock(&self.scheduler, "finish_destructive_input")?
            .finish_destructive_step(token, connection_id)
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

    fn close_instance_resources_result(
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
        lock(&self.scheduler, "begin_destructive_resource_close")?
            .begin_resource_close(token, connection_id, self.monotonic_ms()?)
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                    "begin_destructive_resource_close",
                    &error,
                ))
            })?;

        match self
            .execution
            .close_instance(token.instance_id(), DeviceCloseAuthority::FencedDeviceWrite)
        {
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
                lock(&self.scheduler, "finish_destructive_resource_close")?
                    .finish_destructive_step(token, connection_id)
                    .map_err(|error| {
                        RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                            "finish_destructive_resource_close",
                            &error,
                        ))
                    })?;
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
                lock(&self.scheduler, "finish_destructive_resource_close")?
                    .finish_destructive_step(token, connection_id)
                    .map_err(|scheduler_error| {
                        RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                            "finish_destructive_resource_close",
                            &scheduler_error,
                        ))
                    })?;
                Ok(Err(execution_error))
            }
        }
    }

    /// The instance admission guard excludes capture registration and business native calls.
    pub(super) fn close_retained_instance_while_guarded(
        &self,
        instance_id: InstanceId,
        links: EventLinksDraft,
        reuse_active_lease: bool,
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
            let connection_id =
                ConnectionId::new(RESOURCE_CLOSE_CONNECTION_VALUE).map_err(|error| {
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
                .map_err(|error| {
                    RuntimeHostError::scheduler("prepare_resource_close_lease", &error)
                })?;
            let token = preparation.token().clone();
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
            self.cleanup_token_inner(
                &token,
                connection_id,
                LeaseReleaseReason::HostShutdown,
                None,
                Some(admission),
            )?;
        }
        Ok(result)
    }

    pub(super) fn finish_input_failure(
        &self,
        primary: ExecutionKernelError,
        token: &LeaseToken,
        connection_id: ConnectionId,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<ExecutionKernelError> {
        let close_result: RuntimeHostResult<Result<(), ExecutionKernelError>> = (|| {
            let instance_guard = self
                .instance_guard(token.instance_id())
                .map_err(|failure| *failure.error)?;
            let _admission = lock(&instance_guard, "lock_instance_admission")?;
            self.finish_destructive_input(token, connection_id)
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
}
