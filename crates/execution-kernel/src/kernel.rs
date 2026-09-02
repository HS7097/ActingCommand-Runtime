// SPDX-License-Identifier: AGPL-3.0-only

use crate::{
    ExecutionBackendProvider, ExecutionKernelError, ExecutionKernelResult, ExecutionSession,
    PreparedInputAction, ResolvedExecutionInstance,
};
use actingcommand_contract::{
    ApplicationLifecycleAction, InputAction, InstanceId, MonitorObservation,
};
use actingcommand_device::Frame;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

struct KernelState {
    sessions: BTreeMap<InstanceId, Arc<ExecutionSession>>,
    closed: bool,
}

/// Resident daemon authority for production execution backend sessions.
pub struct ExecutionKernel {
    provider: Arc<dyn ExecutionBackendProvider>,
    state: Mutex<KernelState>,
}

impl ExecutionKernel {
    pub fn new(provider: Arc<dyn ExecutionBackendProvider>) -> Self {
        Self {
            provider,
            state: Mutex::new(KernelState {
                sessions: BTreeMap::new(),
                closed: false,
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
        self.input_prepared(instance_alias, action)
    }

    pub fn input_prepared(
        &self,
        instance_alias: &str,
        action: PreparedInputAction,
    ) -> ExecutionKernelResult<()> {
        let session = self.session(instance_alias)?;
        let result = session.input_prepared(action);
        self.finish_session_operation(&session, result)
    }

    pub fn capture(&self, instance_alias: &str) -> ExecutionKernelResult<Frame> {
        let session = self.session(instance_alias)?;
        let result = session.capture();
        self.finish_session_operation(&session, result)
    }

    pub fn control_application(
        &self,
        instance_alias: &str,
        action: ApplicationLifecycleAction,
    ) -> ExecutionKernelResult<()> {
        let session = self.session(instance_alias)?;
        let result = session.control_application(action);
        self.finish_session_operation(&session, result)
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

    pub fn close(&self) -> ExecutionKernelResult<()> {
        let sessions = {
            let mut state = self.lock_state()?;
            if state.closed {
                return Ok(());
            }
            state.closed = true;
            std::mem::take(&mut state.sessions)
        };
        let mut failure = None;
        for session in sessions.into_values() {
            if let Err(error) = session.close() {
                failure = Some(match failure {
                    Some(primary) => ExecutionKernelError::merge(primary, error),
                    None => error,
                });
            }
        }
        failure.map_or(Ok(()), Err)
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
        let session = Arc::new(ExecutionSession::start(
            Arc::clone(&self.provider),
            instance_alias.to_string(),
            resolved.clone(),
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
            Err(error) => error,
        };
        match self.retire_failed_session(session) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(ExecutionKernelError::merge(error, cleanup)),
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
        if let Err(error) = self.close() {
            panic!("{error}");
        }
    }
}
