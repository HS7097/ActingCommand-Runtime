// SPDX-License-Identifier: AGPL-3.0-only

use crate::codes::DatabaseCode;
use actingcommand_contract::outcome::{Code, Location};
use std::error::Error;
use std::fmt;

/// Physical database failures retain the state owner's established diagnostic identity.
#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeDatabaseError {
    code: Code,
    operation: Location,
    detail: Option<String>,
    warnings: Vec<crate::MaintenanceWarning>,
}

impl RuntimeDatabaseError {
    pub(crate) const fn new(code: Code, operation: Location) -> Self {
        Self {
            code,
            operation,
            detail: None,
            warnings: Vec::new(),
        }
    }

    pub(crate) fn io(code: Code, operation: Location, error: &std::io::Error) -> Self {
        Self {
            code,
            operation,
            warnings: Vec::new(),
            detail: Some(format!(
                "kind={:?};os={:?}",
                error.kind(),
                error.raw_os_error()
            )),
        }
    }

    pub(crate) fn sql(operation: Location, error: &rusqlite::Error) -> Self {
        Self {
            code: DatabaseCode::DatabaseMaintenanceSqlFailed.code(),
            operation,
            warnings: Vec::new(),
            detail: error
                .sqlite_error()
                .map(|error| format!("sqlite={}", error.extended_code)),
        }
    }

    pub(crate) fn with_detail(mut self, detail: String) -> Self {
        self.detail = Some(detail);
        self
    }

    pub fn warnings(&self) -> &[crate::MaintenanceWarning] {
        &self.warnings
    }
    pub(crate) fn with_warnings(mut self, warnings: Vec<crate::MaintenanceWarning>) -> Self {
        self.warnings = warnings;
        self
    }

    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    pub const fn code(&self) -> Code {
        self.code
    }

    pub const fn operation(&self) -> Location {
        self.operation
    }
}

/// The derived form this error printed before its code and operation were typed: both print
/// as quoted spellings, so a `{:?}` rendering (which callers keep as native detail) is
/// unchanged.
impl fmt::Debug for RuntimeDatabaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeDatabaseError")
            .field("code", &self.code.as_str())
            .field("operation", &self.operation.as_str())
            .field("detail", &self.detail)
            .field("warnings", &self.warnings)
            .finish()
    }
}

impl fmt::Display for RuntimeDatabaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} during {}", self.code, self.operation)?;
        if let Some(detail) = &self.detail {
            write!(formatter, ": {detail}")?;
        }
        for warning in &self.warnings {
            write!(
                formatter,
                "; WARNING {} during {} sqlite={} recovery={}",
                warning.code, warning.operation, warning.sqlite_code, warning.recovery
            )?;
        }
        Ok(())
    }
}

impl Error for RuntimeDatabaseError {}

pub type RuntimeDatabaseResult<T> = Result<T, RuntimeDatabaseError>;
