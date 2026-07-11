// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{RuntimeErrorCode, RuntimeErrorProjection};
use std::error::Error;
use std::fmt;

pub type RuntimeClientResult<T> = Result<T, RuntimeClientError>;

/// Redacted local transport or typed Runtime rejection error.
#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeClientError {
    code: &'static str,
    operation: &'static str,
    projection: Option<RuntimeErrorProjection>,
}

impl RuntimeClientError {
    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub const fn operation(&self) -> &'static str {
        self.operation
    }

    pub fn is_fatal(&self) -> bool {
        self.projection.as_ref().is_none_or(|value| value.fatal)
    }

    pub fn is_fallback_eligible(&self) -> bool {
        self.projection.as_ref().is_some_and(|value| {
            !value.fatal
                && matches!(
                    value.code,
                    RuntimeErrorCode::LeaseBusy
                        | RuntimeErrorCode::LeaseCooldown
                        | RuntimeErrorCode::BackendOpenFailed
                        | RuntimeErrorCode::BackendOperationFailed
                )
        })
    }

    pub const fn projection(&self) -> Option<&RuntimeErrorProjection> {
        self.projection.as_ref()
    }

    pub(crate) const fn fatal(code: &'static str, operation: &'static str) -> Self {
        Self {
            code,
            operation,
            projection: None,
        }
    }

    pub(crate) const fn rejected(
        operation: &'static str,
        projection: RuntimeErrorProjection,
    ) -> Self {
        Self {
            code: "runtime_request_rejected",
            operation,
            projection: Some(projection),
        }
    }
}

impl fmt::Debug for RuntimeClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeClientError")
            .field("code", &self.code)
            .field("operation", &self.operation)
            .field("fatal", &self.is_fatal())
            .field(
                "runtime_code",
                &self.projection.as_ref().map(|value| value.code),
            )
            .finish()
    }
}

impl fmt::Display for RuntimeClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.projection {
            Some(projection) => write!(
                formatter,
                "runtime client error {} during {} with runtime code {:?}",
                self.code, self.operation, projection.code
            ),
            None => write!(
                formatter,
                "runtime client error {} during {}",
                self.code, self.operation
            ),
        }
    }
}

impl Error for RuntimeClientError {}
