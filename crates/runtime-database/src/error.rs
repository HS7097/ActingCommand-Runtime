// SPDX-License-Identifier: AGPL-3.0-only

use std::error::Error;
use std::fmt;

/// Physical database failures retain the state owner's established diagnostic identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDatabaseError {
    code: &'static str,
    operation: &'static str,
}

impl RuntimeDatabaseError {
    pub(crate) const fn new(code: &'static str, operation: &'static str) -> Self {
        Self { code, operation }
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
        write!(formatter, "{} during {}", self.code, self.operation)
    }
}

impl Error for RuntimeDatabaseError {}

pub type RuntimeDatabaseResult<T> = Result<T, RuntimeDatabaseError>;
