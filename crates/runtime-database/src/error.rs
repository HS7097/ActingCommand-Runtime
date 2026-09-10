// SPDX-License-Identifier: AGPL-3.0-only

use std::error::Error;
use std::fmt;

/// Physical database failures retain the state owner's established diagnostic identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDatabaseError {
    code: &'static str,
    operation: &'static str,
    detail: Option<String>,
    warnings: Vec<crate::MaintenanceWarning>,
}

impl RuntimeDatabaseError {
    pub(crate) const fn new(code: &'static str, operation: &'static str) -> Self {
        Self {
            code,
            operation,
            detail: None,
            warnings: Vec::new(),
        }
    }

    pub(crate) fn io(code: &'static str, operation: &'static str, error: &std::io::Error) -> Self {
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

    pub(crate) fn sql(operation: &'static str, error: &rusqlite::Error) -> Self {
        Self {
            code: "database_maintenance_sql_failed",
            operation,
            warnings: Vec::new(),
            detail: error
                .sqlite_error()
                .map(|error| format!("sqlite={}", error.extended_code)),
        }
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

    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub const fn operation(&self) -> &'static str {
        self.operation
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
