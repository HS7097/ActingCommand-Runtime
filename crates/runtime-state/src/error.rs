// SPDX-License-Identifier: AGPL-3.0-only

use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeStateErrorClass {
    Request,
    Fatal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeStateError {
    code: &'static str,
    operation: &'static str,
    class: RuntimeStateErrorClass,
}

impl RuntimeStateError {
    pub const fn request(code: &'static str, operation: &'static str) -> Self {
        Self {
            code,
            operation,
            class: RuntimeStateErrorClass::Request,
        }
    }

    pub const fn fatal(code: &'static str, operation: &'static str) -> Self {
        Self {
            code,
            operation,
            class: RuntimeStateErrorClass::Fatal,
        }
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
        write!(formatter, "{} during {}", self.code, self.operation)
    }
}

impl Error for RuntimeStateError {}

impl From<actingcommand_runtime_database::RuntimeDatabaseError> for RuntimeStateError {
    fn from(error: actingcommand_runtime_database::RuntimeDatabaseError) -> Self {
        Self::fatal(error.code(), error.operation())
    }
}

pub type RuntimeStateResult<T> = Result<T, RuntimeStateError>;
