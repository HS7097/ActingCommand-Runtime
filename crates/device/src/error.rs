// SPDX-License-Identifier: AGPL-3.0-only

use std::error::Error;
use std::fmt;

pub type DeviceResult<T> = Result<T, DeviceError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceErrorSeverity {
    Fatal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceError {
    severity: DeviceErrorSeverity,
    message: String,
}

impl DeviceError {
    pub fn fatal(message: impl Into<String>) -> Self {
        Self {
            severity: DeviceErrorSeverity::Fatal,
            message: message.into(),
        }
    }

    pub fn severity(&self) -> DeviceErrorSeverity {
        self.severity
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.severity, self.message)
    }
}

impl Error for DeviceError {}
