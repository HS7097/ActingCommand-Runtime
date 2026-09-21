// SPDX-License-Identifier: AGPL-3.0-only

use crate::{
    ExecutionBackendProvider, ExecutionInputOutcome, ExecutionKernelError, ExecutionKernelResult,
    ExecutionResourceCloseOutcome, ExecutionSession, InputFrameContext, PreparedInputAction,
    ResolvedExecutionInstance,
};
use actingcommand_contract::{
    ApplicationLifecycleAction, CaptureGeometryObservation, EmulatorInstanceAction, FencedWrite,
    FrameId, InputAction, InputFrameReference, InstanceId, MonitorObservation,
};
use actingcommand_device::{
    DeviceCloseAuthority, EmulatorControlOutcome, EmulatorControlResult, Frame, InputOperationCheck,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError, Weak};
use std::thread;
use std::time::Instant;

struct KernelState {
    session_generation: u64,
    sessions: BTreeMap<InstanceId, Arc<ExecutionSession>>,
    closed: bool,
    close_result: Option<ExecutionKernelResult<()>>,
    instance_closes: BTreeMap<InstanceId, ExecutionKernelResult<ExecutionResourceCloseOutcome>>,
}

/// Resident daemon authority for production execution backend sessions.
pub struct ExecutionKernel {
    provider: Arc<dyn ExecutionBackendProvider>,
    state: Mutex<KernelState>,
}

/// Weak association with the exact session that produced a retained frame.
pub struct CaptureGeometrySessionRef {
    instance_id: InstanceId,
    session: Weak<ExecutionSession>,
}

impl CaptureGeometrySessionRef {
    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub fn same_session(&self, other: &Self) -> bool {
        self.instance_id == other.instance_id && Weak::ptr_eq(&self.session, &other.session)
    }
}

impl ExecutionKernel {
    pub fn new(provider: Arc<dyn ExecutionBackendProvider>) -> Self {
        Self {
            provider,
            state: Mutex::new(KernelState {
                session_generation: 0,
                sessions: BTreeMap::new(),
                closed: false,
                close_result: None,
                instance_closes: BTreeMap::new(),
            }),
        }
    }

    pub fn resolve(
        &self,
        instance_alias: &str,
    ) -> ExecutionKernelResult<ResolvedExecutionInstance> {
        self.provider
            .resolve(instance_alias)
            .ok_or_else(|| ExecutionKernelError::fatal("execution_instance_unknown"))
    }

    pub fn vision_provider(
        &self,
    ) -> Option<Arc<dyn actingcommand_recognition_pack::VisionProvider>> {
        self.provider.vision_provider()
    }

    pub fn prepare_input(&self, action: InputAction) -> ExecutionKernelResult<PreparedInputAction> {
        action.try_into()
    }

    pub fn input(&self, instance_alias: &str, action: InputAction) -> ExecutionKernelResult<()> {
        let action = self.prepare_input(action)?;
        self.input_prepared(instance_alias, action).map(|_| ())
    }

    pub fn input_prepared(
        &self,
        instance_alias: &str,
        action: PreparedInputAction,
    ) -> ExecutionKernelResult<ExecutionInputOutcome> {
        self.input_prepared_with_registration_guard(instance_alias, action, ())
    }

    /// Release the Host journal lock after session registration, before backend work.
    pub fn input_prepared_with_registration_guard<G>(
        &self,
        instance_alias: &str,
        action: PreparedInputAction,
        registration_guard: G,
    ) -> ExecutionKernelResult<ExecutionInputOutcome> {
        let session = self.session(instance_alias)?;
        drop(registration_guard);
        let result = session.input_prepared(action);
        self.finish_session_operation(&session, result)
    }

    /// Host keeps failed input resources until it admits close under the current lease.
    pub fn input_prepared_retained_with_registration_guard<G>(
        &self,
        instance_alias: &str,
        action: PreparedInputAction,
        registration_guard: G,
    ) -> ExecutionKernelResult<ExecutionInputOutcome> {
        self.input_prepared_in_frame(instance_alias, action, None, None, None, registration_guard)
    }

    pub fn input_prepared_in_frame<G>(
        &self,
        instance_alias: &str,
        action: PreparedInputAction,
        frame: Option<InputFrameReference>,
        check: Option<Arc<dyn InputOperationCheck>>,
        step: Option<Arc<FencedWrite>>,
        registration_guard: G,
    ) -> ExecutionKernelResult<ExecutionInputOutcome> {
        let session = self.session(instance_alias)?;
        drop(registration_guard);
        session
            .input_prepared_in_frame(action, frame, check, step)
            .map_err(|error| error.with_instance_id(session.resolved().instance_id()))
    }

    pub fn capture(&self, instance_alias: &str) -> ExecutionKernelResult<Frame> {
        let session = self.session(instance_alias)?;
        let result = session.capture();
        self.finish_session_operation(&session, result)
    }

    /// Host retains the session while deciding whether a real close lease is available.
    pub fn capture_retained(&self, instance_alias: &str) -> ExecutionKernelResult<Frame> {
        self.capture_retained_with_registration_guard(instance_alias, ())
    }

    pub fn capture_retained_with_registration_guard<G>(
        &self,
        instance_alias: &str,
        registration_guard: G,
    ) -> ExecutionKernelResult<Frame> {
        self.capture_frame_retained_with_geometry_session_and_registration_guard(
            instance_alias,
            None,
            registration_guard,
        )
        .map(|(frame, _)| frame)
    }

    pub fn capture_frame_retained_with_registration_guard<G>(
        &self,
        instance_alias: &str,
        frame_id: Option<FrameId>,
        registration_guard: G,
    ) -> ExecutionKernelResult<Frame> {
        self.capture_frame_retained_with_geometry_session_and_registration_guard(
            instance_alias,
            frame_id,
            registration_guard,
        )
        .map(|(frame, _)| frame)
    }

    /// Carries the producing session without retaining another backend holder.
    pub fn capture_retained_with_geometry_session_and_registration_guard<G>(
        &self,
        instance_alias: &str,
        registration_guard: G,
    ) -> ExecutionKernelResult<(Frame, CaptureGeometrySessionRef)> {
        self.capture_frame_retained_with_geometry_session_and_registration_guard(
            instance_alias,
            None,
            registration_guard,
        )
    }

    /// Carries the producing session and the pending input frame identity together.
    pub fn capture_frame_retained_with_geometry_session_and_registration_guard<G>(
        &self,
        instance_alias: &str,
        frame_id: Option<FrameId>,
        registration_guard: G,
    ) -> ExecutionKernelResult<(Frame, CaptureGeometrySessionRef)> {
        let session = self.session(instance_alias)?;
        drop(registration_guard);
        let frame = session
            .capture_frame_retained(frame_id)
            .map_err(|error| error.with_instance_id(session.resolved().instance_id()))?;
        let reference = CaptureGeometrySessionRef {
            instance_id: session.resolved().instance_id(),
            session: Arc::downgrade(&session),
        };
        Ok((frame, reference))
    }

    pub fn finish_failed_capture(
        &self,
        primary: ExecutionKernelError,
        authority: DeviceCloseAuthority,
    ) -> ExecutionKernelError {
        let Some(instance) = primary.instance_id() else {
            return primary;
        };
        match self.close_instance(instance, authority) {
            Ok(outcome) => primary.with_stdio_observations(outcome.vendor_stdio()),
            Err(cleanup) => ExecutionKernelError::merge_cleanup(primary, cleanup),
        }
    }

    /// Called by the Runtime after its original capture event has committed.
    pub fn commit_input_frame(
        &self,
        alias: &str,
        reference: InputFrameReference,
    ) -> ExecutionKernelResult<InputFrameContext> {
        self.existing_frame_session(alias)?
            .commit_input_frame(reference)
    }

    pub fn resolve_input_frame(
        &self,
        alias: &str,
        reference: InputFrameReference,
    ) -> ExecutionKernelResult<InputFrameContext> {
        self.existing_frame_session(alias)?
            .resolve_input_frame(reference)
    }

    fn existing_frame_session(&self, alias: &str) -> ExecutionKernelResult<Arc<ExecutionSession>> {
        let instance = self.resolve(alias)?.instance_id();
        self.lock_state()?
            .sessions
            .get(&instance)
            .cloned()
            .ok_or_else(|| ExecutionKernelError::fatal("input_frame_session_missing"))
    }

    /// Reads only the capture object belonging to the original producing session.
    pub fn observe_capture_geometry(
        &self,
        reference: &CaptureGeometrySessionRef,
        deadline: Instant,
    ) -> ExecutionKernelResult<CaptureGeometryObservation> {
        self.capture_geometry_session(reference, deadline)?
            .observe_geometry(deadline)
            .map_err(|error| error.with_instance_id(reference.instance_id))
    }

    /// Checks the retained frame binding without querying the producer again.
    pub fn validate_capture_geometry_session(
        &self,
        reference: &CaptureGeometrySessionRef,
        deadline: Instant,
    ) -> ExecutionKernelResult<()> {
        self.capture_geometry_session(reference, deadline)?
            .validate_geometry_open(deadline)
            .map_err(|error| error.with_instance_id(reference.instance_id))
    }

    fn capture_geometry_session(
        &self,
        reference: &CaptureGeometrySessionRef,
        deadline: Instant,
    ) -> ExecutionKernelResult<Arc<ExecutionSession>> {
        crate::session::geometry_remaining(deadline)?;
        let state = self.state.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => crate::session::geometry_unavailable(
                "capture_geometry_kernel_busy",
                "the existing kernel session registry is busy",
            ),
            TryLockError::Poisoned(_) => {
                ExecutionKernelError::fatal("execution_kernel_state_poisoned")
            }
        })?;
        crate::session::geometry_remaining(deadline)?;
        if state.closed {
            return Err(state
                .close_result
                .as_ref()
                .and_then(|result| result.as_ref().err())
                .cloned()
                .unwrap_or_else(|| {
                    crate::session::geometry_unavailable(
                        "capture_geometry_kernel_closed",
                        "the kernel is closed",
                    )
                }));
        }
        if let Some(Err(error)) = state.instance_closes.get(&reference.instance_id)
            && error.resource_quiescence()
                == Some(actingcommand_contract::ResourceQuiescence::Unconfirmed)
        {
            return Err(error.clone());
        }
        let session = state.sessions.get(&reference.instance_id).ok_or_else(|| {
            state
                .instance_closes
                .get(&reference.instance_id)
                .and_then(|result| result.as_ref().err())
                .cloned()
                .unwrap_or_else(|| {
                    crate::session::geometry_unavailable(
                        "capture_geometry_session_missing",
                        "the original producing session is no longer registered",
                    )
                })
        })?;
        if !Weak::ptr_eq(&reference.session, &Arc::downgrade(session)) {
            return Err(crate::session::geometry_unavailable(
                "capture_geometry_session_changed",
                "the registered session differs from the frame-producing session",
            ));
        }
        let session = Arc::clone(session);
        drop(state);
        Ok(session)
    }

    pub fn control_application(
        &self,
        instance_alias: &str,
        action: ApplicationLifecycleAction,
    ) -> ExecutionKernelResult<()> {
        self.control_application_with_registration_guard(instance_alias, action, ())
    }

    pub fn control_application_with_registration_guard<G>(
        &self,
        instance_alias: &str,
        action: ApplicationLifecycleAction,
        registration_guard: G,
    ) -> ExecutionKernelResult<()> {
        let session = self.session(instance_alias)?;
        drop(registration_guard);
        let result = session.control_application(action);
        self.finish_session_operation(&session, result)
    }

    /// One ADB baseline probe (slice #316-B3), driven on the provider directly like
    /// `control_instance`, outside any session; the device error comes back untyped.
    pub fn probe_adb_baseline(
        &self,
        instance_alias: &str,
    ) -> actingcommand_device::DeviceResult<()> {
        self.provider.probe_adb_baseline(instance_alias)
    }

    pub fn probe_adb_baseline_until(
        &self,
        instance_alias: &str,
        deadline: Instant,
        stopped: &dyn Fn() -> bool,
    ) -> ExecutionKernelResult<()> {
        self.provider
            .probe_adb_baseline_until(instance_alias, deadline, stopped)
            .map_err(|error| {
                ExecutionKernelError::device("application_adb_baseline_failed", &error)
            })
    }

    /// Read-only: what the provider reports in the foreground of the instance next to its
    /// assigned application (slice #316-B3). Drives the provider directly like
    /// `control_instance`, outside any session; the device error comes back untyped so the
    /// host can classify it as an ADB failure itself.
    pub fn observe_foreground_application(
        &self,
        instance_alias: &str,
    ) -> actingcommand_device::DeviceResult<crate::ForegroundApplicationObservation> {
        self.provider.observe_foreground_application(instance_alias)
    }

    /// Drives the provider's instance control surface directly: no session is opened, touched
    /// or closed here. The host closes the instance's device session first and does not
    /// reopen it afterwards (it opens lazily on the next lease).
    pub fn control_instance(
        &self,
        instance_alias: &str,
        action: EmulatorInstanceAction,
    ) -> EmulatorControlResult<EmulatorControlOutcome> {
        self.provider.control_instance(instance_alias, action)
    }

    /// Rebinds the provider's endpoint after emulator control. A retained session would keep
    /// the previous endpoint identity, so one still open here is an invariant violation: the
    /// host closes the instance's session before every control action.
    pub fn rebind_discovered_endpoint(
        &self,
        instance_alias: &str,
        adb_port: Option<u16>,
    ) -> ExecutionKernelResult<()> {
        let resolved = self.resolve(instance_alias)?;
        if self.has_session(resolved.instance_id())? {
            return Err(ExecutionKernelError::fatal(
                "execution_endpoint_rebind_session_open",
            ));
        }
        self.provider
            .rebind_discovered_endpoint(instance_alias, adb_port)
            .map_err(|error| {
                ExecutionKernelError::device("execution_endpoint_rebind_failed", &error)
            })
    }

    pub fn observe_monitor(
        &self,
        instance_alias: &str,
        expected_page: &str,
        frame: &Frame,
    ) -> ExecutionKernelResult<MonitorObservation> {
        let observation = self
            .provider
            .observe_monitor(instance_alias, expected_page, frame)?;
        observation
            .validate()
            .map_err(|_| ExecutionKernelError::fatal("monitor_observation_invalid"))?;
        if observation.expected_page() != expected_page {
            return Err(ExecutionKernelError::fatal("monitor_observation_invalid"));
        }
        Ok(observation)
    }

    pub fn control_application_retained_with_registration_guard<G>(
        &self,
        instance_alias: &str,
        action: ApplicationLifecycleAction,
        step: Option<Arc<FencedWrite>>,
        registration_guard: G,
    ) -> ExecutionKernelResult<()> {
        let session = self.session(instance_alias)?;
        drop(registration_guard);
        session
            .control_application_retained(action, step)
            .map_err(|error| error.with_instance_id(session.resolved().instance_id()))
    }

    pub fn close(&self) -> ExecutionKernelResult<()> {
        let mut state = self.lock_state()?;
        if let Some(result) = &state.close_result {
            return result.clone();
        }
        let sessions = {
            state.closed = true;
            std::mem::take(&mut state.sessions)
        };
        let mut failure = None;
        let mut closed_sessions = Vec::new();
        for (instance, result) in &state.instance_closes {
            if let Err(error) = result
                && error.resource_quiescence()
                    == Some(actingcommand_contract::ResourceQuiescence::Unconfirmed)
            {
                closed_sessions.push((*instance, error.clone()));
                failure = Some(match failure {
                    None => error.clone(),
                    Some(primary) => ExecutionKernelError::merge(primary, error.clone()),
                });
            }
        }
        for (instance_id, session) in sessions {
            if let Err(error) = session.close_with_authority(DeviceCloseAuthority::LocalOnly) {
                closed_sessions.push((instance_id, error.clone()));
                failure = Some(match failure {
                    Some(primary) => ExecutionKernelError::merge(primary, error),
                    None => error,
                });
            }
        }
        let result = failure.map_or(Ok(()), |error| {
            Err(error.with_closed_sessions(closed_sessions))
        });
        state.close_result = Some(result.clone());
        result
    }

    pub fn close_instance(
        &self,
        instance_id: InstanceId,
        authority: DeviceCloseAuthority,
    ) -> ExecutionKernelResult<ExecutionResourceCloseOutcome> {
        self.close_instance_with_input_check(instance_id, authority, None)
    }

    pub fn close_instance_with_input_check(
        &self,
        instance_id: InstanceId,
        authority: DeviceCloseAuthority,
        input_check: Option<Arc<dyn InputOperationCheck>>,
    ) -> ExecutionKernelResult<ExecutionResourceCloseOutcome> {
        let mut state = self.lock_state()?;
        let session = {
            if state.closed {
                return Err(ExecutionKernelError::fatal("execution_kernel_closed"));
            }
            if let Some(result) = state.instance_closes.get(&instance_id) {
                return result.clone();
            }
            state.sessions.remove(&instance_id)
        };
        let Some(session) = session else {
            return Ok(ExecutionResourceCloseOutcome::confirmed(0));
        };
        let result = session
            .close_with_input_check(authority, input_check)
            .map_err(|error| error.with_instance_id(instance_id));
        state.instance_closes.insert(instance_id, result.clone());
        result
    }

    pub fn has_session(&self, instance_id: InstanceId) -> ExecutionKernelResult<bool> {
        Ok(self.lock_state()?.sessions.contains_key(&instance_id))
    }

    pub fn owned_instance_ids(&self) -> ExecutionKernelResult<Vec<InstanceId>> {
        let state = self.lock_state()?;
        let mut instances = state.sessions.keys().copied().collect::<BTreeSet<_>>();
        instances.extend(
            state
                .instance_closes
                .iter()
                .filter_map(|(instance, result)| {
                    result
                        .as_ref()
                        .is_err_and(|error| {
                            error.resource_quiescence()
                                == Some(actingcommand_contract::ResourceQuiescence::Unconfirmed)
                        })
                        .then_some(*instance)
                }),
        );
        Ok(instances.into_iter().collect())
    }

    pub fn has_owned_resources(&self, instance_id: InstanceId) -> ExecutionKernelResult<bool> {
        let state = self.lock_state()?;
        Ok(state.sessions.contains_key(&instance_id)
            || state
                .instance_closes
                .get(&instance_id)
                .is_some_and(|result| {
                    result.as_ref().is_err_and(|error| {
                        error.resource_quiescence()
                            == Some(actingcommand_contract::ResourceQuiescence::Unconfirmed)
                    })
                }))
    }

    /// Host has retired permitted resources. Remaining sessions must retain their owners.
    pub fn close_after_resource_retirement(&self) -> ExecutionKernelResult<()> {
        {
            let mut state = self.lock_state()?;
            if let Some(result) = &state.close_result {
                return result.clone();
            }
            state.closed = true;
            for (instance, session) in std::mem::take(&mut state.sessions) {
                let error = ExecutionKernelError::device(
                    "execution_resource_close_incomplete",
                    &actingcommand_device::DeviceError::fatal(
                        "execution session retained because resource-close authority or completion was not established",
                    ).with_resource_close_cause(
                        actingcommand_device::DeviceResourceKind::InProcessWorker,
                        actingcommand_device::DeviceResourceClosePhase::Close,
                        "execution_kernel", None, None,
                        actingcommand_device::DeviceResourceQuiescence::Unconfirmed, 1,
                    ),
                ).with_instance_id(instance);
                std::mem::forget(session);
                state.instance_closes.entry(instance).or_insert(Err(error));
            }
        }
        self.close()
    }

    pub fn has_sessions(&self) -> ExecutionKernelResult<bool> {
        let state = self.lock_state()?;
        Ok(!state.sessions.is_empty()
            || state.instance_closes.values().any(|result| {
                result.as_ref().is_err_and(|error| {
                    error.resource_quiescence()
                        == Some(actingcommand_contract::ResourceQuiescence::Unconfirmed)
                })
            }))
    }

    /// Reads the retained close outcome without attempting resource retirement again.
    pub fn unconfirmed_instance_close_error(
        &self,
        instance_id: InstanceId,
    ) -> ExecutionKernelResult<Option<ExecutionKernelError>> {
        Ok(self
            .lock_state()?
            .instance_closes
            .get(&instance_id)
            .and_then(|result| result.as_ref().err())
            .filter(|error| {
                error.resource_quiescence()
                    == Some(actingcommand_contract::ResourceQuiescence::Unconfirmed)
            })
            .cloned())
    }

    fn session(&self, instance_alias: &str) -> ExecutionKernelResult<Arc<ExecutionSession>> {
        let resolved = self.resolve(instance_alias)?;
        let mut state = self.lock_state()?;
        if state.closed {
            return Err(ExecutionKernelError::fatal("execution_kernel_closed"));
        }
        if let Some(session) = state.sessions.get(&resolved.instance_id()) {
            if session.resolved() != &resolved {
                return Err(ExecutionKernelError::fatal(
                    "execution_instance_identity_mismatch",
                ));
            }
            return Ok(Arc::clone(session));
        }
        if let Some(Err(error)) = state.instance_closes.get(&resolved.instance_id())
            && error.resource_quiescence()
                == Some(actingcommand_contract::ResourceQuiescence::Unconfirmed)
        {
            return Err(error.clone());
        }
        state.instance_closes.remove(&resolved.instance_id());
        let generation = state
            .session_generation
            .checked_add(1)
            .ok_or_else(|| ExecutionKernelError::fatal("execution_session_generation_exhausted"))?;
        state.session_generation = generation;
        let session = Arc::new(ExecutionSession::start(
            Arc::clone(&self.provider),
            instance_alias.to_string(),
            resolved.clone(),
            generation,
        )?);
        state
            .sessions
            .insert(resolved.instance_id(), Arc::clone(&session));
        Ok(session)
    }

    fn finish_session_operation<T>(
        &self,
        session: &Arc<ExecutionSession>,
        result: ExecutionKernelResult<T>,
    ) -> ExecutionKernelResult<T> {
        let error = match result {
            Ok(value) => return Ok(value),
            Err(error) => error.with_instance_id(session.resolved().instance_id()),
        };
        if error.resource_quiescence()
            == Some(actingcommand_contract::ResourceQuiescence::Unconfirmed)
        {
            self.lock_state()?
                .instance_closes
                .insert(session.resolved().instance_id(), Err(error.clone()));
        }
        match self.retire_failed_session(session) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(ExecutionKernelError::merge_retirement(error, cleanup)),
        }
    }

    fn retire_failed_session(
        &self,
        failed_session: &Arc<ExecutionSession>,
    ) -> ExecutionKernelResult<()> {
        let instance_id = failed_session.resolved().instance_id();
        let mut state = self.lock_state()?;
        let is_current = state
            .sessions
            .get(&instance_id)
            .is_some_and(|session| Arc::ptr_eq(session, failed_session));
        if is_current {
            state.sessions.remove(&instance_id);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn session_for_test(
        &self,
        instance_alias: &str,
    ) -> ExecutionKernelResult<Arc<ExecutionSession>> {
        self.session(instance_alias)
    }

    #[cfg(test)]
    pub(crate) fn retire_failed_session_for_test(
        &self,
        failed_session: &Arc<ExecutionSession>,
    ) -> ExecutionKernelResult<()> {
        self.retire_failed_session(failed_session)
    }

    #[cfg(test)]
    pub(crate) fn finish_failed_session_for_test(
        &self,
        failed_session: &Arc<ExecutionSession>,
        primary: ExecutionKernelError,
    ) -> ExecutionKernelResult<()> {
        self.finish_session_operation(failed_session, Err(primary))
    }

    #[cfg(test)]
    pub(crate) fn poison_state_for_test(&self) {
        let _state = self.state.lock().expect("kernel state");
        panic!("poison kernel state");
    }

    #[cfg(test)]
    pub(crate) fn clear_state_poison_for_test(&self) {
        self.state.clear_poison();
    }

    fn lock_state(&self) -> ExecutionKernelResult<MutexGuard<'_, KernelState>> {
        self.state
            .lock()
            .map_err(|_| ExecutionKernelError::fatal("execution_kernel_state_poisoned"))
    }
}

impl Drop for ExecutionKernel {
    fn drop(&mut self) {
        if thread::panicking() {
            return;
        }
        if self
            .state
            .get_mut()
            .is_ok_and(|state| state.close_result.is_some())
        {
            return;
        }
        if let Err(error) = self.close() {
            panic!("{error}");
        }
    }
}
