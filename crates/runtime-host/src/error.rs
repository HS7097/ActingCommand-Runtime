// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{
    CleanupCauseDraft, DiagnosticDetailDraft, EventId, InstanceId, ResourceQuiescence,
    RuntimeErrorCode, RuntimeErrorProjection,
};
use actingcommand_execution_kernel::{ExecutionKernelError, ExecutionLifecycleCause};
use actingcommand_runtime_state::RuntimeStateError;
use actingcommand_scheduler::SchedulerError;
use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

pub type RuntimeHostResult<T> = Result<T, RuntimeHostError>;

#[derive(Clone)]
pub struct RuntimeHostError {
    code: &'static str,
    operation: &'static str,
    projection: RuntimeErrorProjection,
    pub(crate) lifecycle: Box<RuntimeHostFailureContext>,
}

#[derive(Clone, Default)]
pub(crate) struct RuntimeHostFailureContext {
    pub(crate) task_timing: Option<Box<actingcommand_contract::TaskTimingObservations>>,
    pub(crate) capacity: Option<actingcommand_contract::CapacityDecision>,
    pub(crate) raw_os_error: Option<i32>,
    pub(crate) adb_recovery: Option<Box<actingcommand_contract::AdbTargetRecovery>>,
    pub(crate) incomplete_device_diagnostic_summary: Option<(&'static str, &'static str)>,
    diagnostic_detail: Option<Box<DiagnosticDetailDraft>>,
    cleanup_cause: Option<Box<CleanupCauseDraft>>,
    pub(crate) recorded_event: Arc<OnceLock<EventId>>,
    pub(crate) causes: Vec<ExecutionLifecycleCause>,
    pub(crate) instance_id: Option<InstanceId>,
    pub(crate) native_detail: Option<Box<actingcommand_contract::LifecycleNativeDetail>>,
    pub(crate) resource_quiescence: Option<ResourceQuiescence>,
    pub(crate) policy_rejection: Option<Box<actingcommand_contract::PolicyDispatchRejection>>,
    pub(crate) resource_declaration:
        Option<Box<actingcommand_contract::ResourceDeclarationRejection>>,
    pub(crate) resource_declaration_event: Option<actingcommand_contract::TerminalEvent>,
}

impl PartialEq for RuntimeHostError {
    fn eq(&self, other: &Self) -> bool {
        self.code == other.code
            && self.operation == other.operation
            && self.projection == other.projection
            && self.lifecycle.diagnostic_detail == other.lifecycle.diagnostic_detail
            && self.lifecycle.cleanup_cause == other.lifecycle.cleanup_cause
            && self.lifecycle.policy_rejection == other.lifecycle.policy_rejection
            && self.lifecycle.resource_declaration == other.lifecycle.resource_declaration
            && self.lifecycle.resource_declaration_event
                == other.lifecycle.resource_declaration_event
            && self.lifecycle.incomplete_device_diagnostic_summary
                == other.lifecycle.incomplete_device_diagnostic_summary
    }
}
impl Eq for RuntimeHostError {}

impl From<actingcommand_policy::PolicyEvaluationError> for RuntimeHostError {
    fn from(_: actingcommand_policy::PolicyEvaluationError) -> Self {
        Self::request(
            "policy_evaluation_rejected",
            "evaluate_policy_cycle",
            RuntimeErrorCode::InvalidRequest,
        )
    }
}

impl RuntimeHostError {
    pub(crate) fn policy_rejection(&self) -> actingcommand_contract::PolicyDispatchRejection {
        let mut rejection = self
            .lifecycle
            .policy_rejection
            .as_deref()
            .cloned()
            .unwrap_or(actingcommand_contract::PolicyDispatchRejection {
                code: self.code.to_owned(),
                operation: self.operation.to_owned(),
                fatal: self.is_fatal(),
                budget: None,
                next_eligible_unix_ms: None,
            });
        rejection.fatal = self.is_fatal();
        rejection
    }

    pub(crate) fn artifact(error: actingcommand_artifact_store::ArtifactStoreError) -> Self {
        let mut result = if error.is_fatal() {
            Self::fatal(
                error.code(),
                error.operation(),
                RuntimeErrorCode::RuntimeFatal,
            )
        } else {
            Self::request(
                error.code(),
                error.operation(),
                RuntimeErrorCode::InvalidRequest,
            )
        };
        result.lifecycle.native_detail = Some(Box::new(error.native_detail()));
        result.lifecycle.capacity = error.capacity().cloned();
        result.lifecycle.raw_os_error = error.raw_os_error();
        result
    }
    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub const fn operation(&self) -> &'static str {
        self.operation
    }

    pub const fn is_fatal(&self) -> bool {
        self.projection.fatal
    }

    pub const fn projection(&self) -> &RuntimeErrorProjection {
        &self.projection
    }

    pub fn resource_declaration(
        &self,
    ) -> Option<&actingcommand_contract::ResourceDeclarationRejection> {
        self.lifecycle.resource_declaration.as_deref()
    }

    pub(crate) fn into_fatal(mut self) -> Self {
        self.projection.fatal = true;
        self
    }

    pub(crate) fn diagnostic_detail(&self) -> Option<&DiagnosticDetailDraft> {
        self.lifecycle.diagnostic_detail.as_deref()
    }

    pub(crate) fn cleanup_cause(&self) -> Option<&CleanupCauseDraft> {
        self.lifecycle.cleanup_cause.as_deref()
    }

    pub(crate) fn fatal(
        code: &'static str,
        operation: &'static str,
        runtime_code: RuntimeErrorCode,
    ) -> Self {
        Self {
            code,
            operation,
            projection: RuntimeErrorProjection::new(runtime_code, true),
            lifecycle: Box::default(),
        }
    }

    pub(crate) fn request(
        code: &'static str,
        operation: &'static str,
        runtime_code: RuntimeErrorCode,
    ) -> Self {
        Self {
            code,
            operation,
            projection: RuntimeErrorProjection::new(runtime_code, false),
            lifecycle: Box::default(),
        }
    }

    pub(crate) fn with_projection(
        code: &'static str,
        operation: &'static str,
        projection: RuntimeErrorProjection,
    ) -> Self {
        Self {
            code,
            operation,
            projection,
            lifecycle: Box::default(),
        }
    }

    pub(crate) fn scheduler(operation: &'static str, error: &SchedulerError) -> Self {
        Self::with_projection(error.code(), operation, error.projection())
    }

    pub(crate) fn execution(operation: &'static str, error: &ExecutionKernelError) -> Self {
        let runtime_code = match error.code() {
            "input_backend_open_failed" => RuntimeErrorCode::BackendOpenFailed,
            "input_backend_operation_failed" => RuntimeErrorCode::BackendOperationFailed,
            "capture_backend_open_failed"
            | "capture_backend_operation_failed"
            | "execution_session_close_pending" => RuntimeErrorCode::CaptureFailed,
            "monitor_observation_unavailable" | "monitor_observation_failed" => {
                RuntimeErrorCode::RecognitionFailed
            }
            _ => RuntimeErrorCode::RuntimeFatal,
        };
        let runtime_error = Self {
            code: error.code(),
            operation,
            projection: RuntimeErrorProjection::new(runtime_code, error.is_fatal()),
            lifecycle: Box::new(RuntimeHostFailureContext {
                task_timing: None,
                capacity: None,
                raw_os_error: None,
                adb_recovery: error.adb_recovery().cloned().map(Box::new),
                incomplete_device_diagnostic_summary: None,
                diagnostic_detail: error.diagnostic_detail().cloned().map(Box::new),
                cleanup_cause: error.cleanup_cause().cloned().map(Box::new),
                recorded_event: Arc::clone(error.recorded_event()),
                causes: error.lifecycle_causes().to_vec(),
                instance_id: error.instance_id(),
                native_detail: error.native_detail().cloned().map(Box::new),
                resource_quiescence: error.resource_quiescence(),
                policy_rejection: None,
                resource_declaration: None,
                resource_declaration_event: None,
            }),
        };
        if error.resource_quiescence() == Some(ResourceQuiescence::Unconfirmed) {
            runtime_error.into_fatal()
        } else {
            runtime_error
        }
    }

    pub(crate) fn state(error: &RuntimeStateError) -> Self {
        if error.is_fatal() {
            Self::fatal(
                error.code(),
                error.operation(),
                RuntimeErrorCode::RuntimeFatal,
            )
        } else {
            Self::request(
                error.code(),
                error.operation(),
                RuntimeErrorCode::InvalidRequest,
            )
        }
    }

    pub(crate) fn with_native_detail(mut self, detail: String) -> Self {
        let mut end = detail.len().min(1024);
        while !detail.is_char_boundary(end) {
            end -= 1;
        }
        self.lifecycle.native_detail = Some(Box::new(
            actingcommand_contract::LifecycleNativeDetail::new(&detail[..end], end < detail.len()),
        ));
        self
    }

    pub(crate) fn with_related_failure(mut self, relation: &'static str, other: &Self) -> Self {
        if self.lifecycle.task_timing.is_none() {
            self.lifecycle.task_timing = other.lifecycle.task_timing.clone();
        }
        if self.lifecycle.capacity.is_none() {
            self.lifecycle.capacity = other.lifecycle.capacity.clone();
        }
        if self.code == other.code
            && self.operation == other.operation
            && self.lifecycle.native_detail == other.lifecycle.native_detail
        {
            return self;
        }
        let mut text = format!(
            "primary {} during {}; {relation} {} during {}",
            self.code, self.operation, other.code, other.operation
        );
        let details = [
            self.lifecycle.native_detail.as_deref(),
            other.lifecycle.native_detail.as_deref(),
        ];
        let count = details.iter().flatten().count();
        let budget = 1024usize.saturating_sub(text.len() + count * 3) / count.max(1);
        let mut truncated = false;
        for detail in details.into_iter().flatten() {
            let mut end = detail.text().len().min(budget);
            while !detail.text().is_char_boundary(end) {
                end -= 1;
            }
            truncated |= detail.truncated() || end < detail.text().len();
            text.push_str(" | ");
            text.push_str(&detail.text()[..end]);
        }
        self = self.with_native_detail(text);
        self.lifecycle.recorded_event = Arc::new(OnceLock::new());
        if truncated && let Some(detail) = self.lifecycle.native_detail.as_deref() {
            self.lifecycle.native_detail = Some(Box::new(
                actingcommand_contract::LifecycleNativeDetail::new(detail.text(), true),
            ));
        }
        self
    }
}

impl fmt::Debug for RuntimeHostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeHostError")
            .field("code", &self.code)
            .field("operation", &self.operation)
            .field("fatal", &self.is_fatal())
            .field(
                "incomplete_device_diagnostic_summary",
                &self.lifecycle.incomplete_device_diagnostic_summary,
            )
            .finish()
    }
}

impl fmt::Display for RuntimeHostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "runtime host error {} during {}",
            self.code, self.operation
        )?;
        if let Some((code, operation)) = self.lifecycle.incomplete_device_diagnostic_summary {
            write!(
                formatter,
                "; device diagnostic summary incomplete: {code} during {operation}"
            )?;
        }
        Ok(())
    }
}

impl Error for RuntimeHostError {}

#[derive(Clone, Default)]
pub(crate) struct FatalState {
    inner: Arc<Mutex<Option<RuntimeHostError>>>,
    shutdown: Arc<AtomicBool>,
}

impl FatalState {
    pub(crate) fn mark(&self, error: RuntimeHostError) -> RuntimeHostResult<()> {
        if !error.is_fatal() {
            return Err(RuntimeHostError::fatal(
                "nonfatal_error_marked_fatal",
                "mark_runtime_fatal",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        let mut current = self.inner.lock().map_err(|_| {
            RuntimeHostError::fatal(
                "fatal_state_poisoned",
                "mark_runtime_fatal",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        if current.is_none() {
            *current = Some(error);
        }
        self.shutdown.store(true, Ordering::Release);
        Ok(())
    }

    pub(crate) fn current(&self) -> RuntimeHostResult<Option<RuntimeHostError>> {
        self.inner.lock().map(|value| value.clone()).map_err(|_| {
            RuntimeHostError::fatal(
                "fatal_state_poisoned",
                "read_runtime_fatal",
                RuntimeErrorCode::RuntimeFatal,
            )
        })
    }

    pub(crate) fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
    }

    pub(crate) fn is_shutdown_requested(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }
}
