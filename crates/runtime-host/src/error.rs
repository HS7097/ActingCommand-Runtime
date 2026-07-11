// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{RuntimeErrorCode, RuntimeErrorProjection};
use actingcommand_device::DeviceError;
use actingcommand_scheduler::SchedulerError;
use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub type RuntimeHostResult<T> = Result<T, RuntimeHostError>;

#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeHostError {
    code: &'static str,
    operation: &'static str,
    projection: RuntimeErrorProjection,
}

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

    pub(crate) const fn fatal(
        code: &'static str,
        operation: &'static str,
        runtime_code: RuntimeErrorCode,
    ) -> Self {
        Self {
            code,
            operation,
            projection: RuntimeErrorProjection::new(runtime_code, true),
        }
    }

    pub(crate) const fn request(
        code: &'static str,
        operation: &'static str,
        runtime_code: RuntimeErrorCode,
    ) -> Self {
        Self {
            code,
            operation,
            projection: RuntimeErrorProjection::new(runtime_code, false),
        }
    }

    pub(crate) const fn with_projection(
        code: &'static str,
        operation: &'static str,
        projection: RuntimeErrorProjection,
    ) -> Self {
        Self {
            code,
            operation,
            projection,
        }
    }

    pub(crate) fn scheduler(operation: &'static str, error: &SchedulerError) -> Self {
        Self::with_projection(error.code(), operation, error.projection())
    }

    pub(crate) fn backend_open(error: &DeviceError) -> Self {
        Self::with_projection(
            "backend_open_failed",
            "open_input_backend",
            RuntimeErrorProjection::new(
                RuntimeErrorCode::BackendOpenFailed,
                !error.is_fallback_eligible(),
            ),
        )
    }

    pub(crate) fn backend_operation(error: &DeviceError) -> Self {
        Self::with_projection(
            "backend_operation_failed",
            "execute_input_backend",
            RuntimeErrorProjection::new(
                RuntimeErrorCode::BackendOperationFailed,
                !error.is_fallback_eligible(),
            ),
        )
    }

    pub(crate) fn backend_close() -> Self {
        Self::fatal(
            "backend_close_failed",
            "close_input_backend",
            RuntimeErrorCode::RuntimeFatal,
        )
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
