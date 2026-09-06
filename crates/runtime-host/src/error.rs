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
    diagnostic_detail: Option<Box<DiagnosticDetailDraft>>,
    cleanup_cause: Option<Box<CleanupCauseDraft>>,
    pub(crate) recorded_event: Arc<OnceLock<EventId>>,
    pub(crate) causes: Vec<ExecutionLifecycleCause>,
    pub(crate) instance_id: Option<InstanceId>,
    pub(crate) native_detail: Option<Box<actingcommand_contract::LifecycleNativeDetail>>,
    pub(crate) resource_quiescence: Option<ResourceQuiescence>,
}

impl PartialEq for RuntimeHostError {
    fn eq(&self, other: &Self) -> bool {
        self.code == other.code
            && self.operation == other.operation
            && self.projection == other.projection
            && self.lifecycle.diagnostic_detail == other.lifecycle.diagnostic_detail
            && self.lifecycle.cleanup_cause == other.lifecycle.cleanup_cause
    }
}
impl Eq for RuntimeHostError {}

impl RuntimeHostError {
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
            "capture_backend_open_failed" | "capture_backend_operation_failed" => {
                RuntimeErrorCode::CaptureFailed
            }
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
                diagnostic_detail: error.diagnostic_detail().cloned().map(Box::new),
                cleanup_cause: error.cleanup_cause().cloned().map(Box::new),
                recorded_event: Arc::clone(error.recorded_event()),
                causes: error.lifecycle_causes().to_vec(),
                instance_id: error.instance_id(),
                native_detail: None,
                resource_quiescence: error.resource_quiescence(),
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
}

impl fmt::Debug for RuntimeHostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeHostError")
            .field("code", &self.code)
            .field("operation", &self.operation)
            .field("fatal", &self.is_fatal())
            .finish()
    }
}

impl fmt::Display for RuntimeHostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "runtime host error {} during {}",
            self.code, self.operation
        )
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
