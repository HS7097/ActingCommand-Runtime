// SPDX-License-Identifier: AGPL-3.0-only

use crate::codes::StateCode;
use actingcommand_contract::outcome::{Code, Location};
use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeStateErrorClass {
    Request,
    Fatal,
}

/// A state failure. Every code and location the state store raises is registered
/// (`StateCode`, `StateLocation`, shared contract entries); the error keeps their spellings
/// because a relayed ledger failure keeps the ledger's code, which can be a transaction work
/// code of the runtime host until slice A2b types those (Workflow #378).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeStateError {
    code: &'static str,
    operation: &'static str,
    class: RuntimeStateErrorClass,
    detail: Option<String>,
}

impl RuntimeStateError {
    pub(crate) fn with_detail(mut self, detail: String) -> Self {
        self.detail = Some(detail);
        self
    }
    pub const fn request(code: Code, operation: Location) -> Self {
        Self {
            code: code.as_str(),
            operation: operation.as_str(),
            class: RuntimeStateErrorClass::Request,
            detail: None,
        }
    }

    pub const fn fatal(code: Code, operation: Location) -> Self {
        Self {
            code: code.as_str(),
            operation: operation.as_str(),
            class: RuntimeStateErrorClass::Fatal,
            detail: None,
        }
    }

    /// A ledger failure relayed as a fatal state failure: it keeps the ledger error's code and,
    /// where the state store names none of its own, the ledger's operation.
    pub(crate) const fn relayed_fatal(
        relayed_code: &'static str,
        relayed_operation: &'static str,
    ) -> Self {
        Self {
            code: relayed_code,
            operation: relayed_operation,
            class: RuntimeStateErrorClass::Fatal,
            detail: None,
        }
    }

    pub(crate) fn maintenance_sql(operation: Location, error: &rusqlite::Error) -> Self {
        let mut result = Self::fatal(StateCode::StateSnapshotFailed.code(), operation);
        result.detail = error
            .sqlite_error()
            .map(|error| format!("sqlite={}", error.extended_code));
        result
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub const fn operation(&self) -> &'static str {
        self.operation
    }

    pub const fn class(&self) -> RuntimeStateErrorClass {
        self.class
    }

    pub const fn is_fatal(&self) -> bool {
        matches!(self.class, RuntimeStateErrorClass::Fatal)
    }
}

impl fmt::Display for RuntimeStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} during {}", self.code, self.operation)?;
        if let Some(detail) = &self.detail {
            write!(formatter, ": {detail}")?;
        }
        Ok(())
    }
}

impl Error for RuntimeStateError {}

impl From<actingcommand_runtime_database::RuntimeDatabaseError> for RuntimeStateError {
    fn from(error: actingcommand_runtime_database::RuntimeDatabaseError) -> Self {
        Self::fatal(error.code(), error.operation())
    }
}

pub type RuntimeStateResult<T> = Result<T, RuntimeStateError>;
