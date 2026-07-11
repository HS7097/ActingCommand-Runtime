// SPDX-License-Identifier: AGPL-3.0-only

use std::error::Error;
use std::fmt;

pub type ArtifactStoreResult<T> = Result<T, ArtifactStoreError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactStoreError {
    code: &'static str,
    operation: &'static str,
    detail: String,
}

impl ArtifactStoreError {
    pub fn fatal(code: &'static str, operation: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            operation,
            detail: detail.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn operation(&self) -> &'static str {
        self.operation
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    pub const fn is_fatal(&self) -> bool {
        true
    }

    pub(crate) fn with_secondary(mut self, secondary: &Self) -> Self {
        use std::fmt::Write as _;
        let _ = write!(
            self.detail,
            "; secondary {} during {}: {}",
            secondary.code, secondary.operation, secondary.detail
        );
        self
    }
}

impl fmt::Display for ArtifactStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "artifact store fatal {} during {}: {}",
            self.code, self.operation, self.detail
        )
    }
}

impl Error for ArtifactStoreError {}
