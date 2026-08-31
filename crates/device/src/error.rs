// SPDX-License-Identifier: AGPL-3.0-only

use std::error::Error;
use std::fmt;

pub type DeviceResult<T> = Result<T, DeviceError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceErrorSeverity {
    Transient,
    Fatal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceErrorCategory {
    BackendLaunch,
    Handshake,
    ChildExit,
    CommandWrite,
    CommandFlush,
    Protocol,
    Response,
    Native,
}

impl DeviceErrorCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BackendLaunch => "backend_launch",
            Self::Handshake => "handshake",
            Self::ChildExit => "child_exit",
            Self::CommandWrite => "command_write",
            Self::CommandFlush => "command_flush",
            Self::Protocol => "protocol",
            Self::Response => "response",
            Self::Native => "native",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceErrorDiagnostic {
    category: DeviceErrorCategory,
    stage: &'static str,
}

impl DeviceErrorDiagnostic {
    pub const fn new(category: DeviceErrorCategory, stage: &'static str) -> Self {
        Self { category, stage }
    }

    pub const fn category(self) -> DeviceErrorCategory {
        self.category
    }

    pub const fn stage(self) -> &'static str {
        self.stage
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceError {
    severity: DeviceErrorSeverity,
    message: String,
    diagnostic: Option<DeviceErrorDiagnostic>,
}

impl DeviceError {
    pub fn transient(message: impl Into<String>) -> Self {
        Self {
            severity: DeviceErrorSeverity::Transient,
            message: message.into(),
            diagnostic: None,
        }
    }

    pub fn fatal(message: impl Into<String>) -> Self {
        Self {
            severity: DeviceErrorSeverity::Fatal,
            message: message.into(),
            diagnostic: None,
        }
    }

    pub fn with_severity(severity: DeviceErrorSeverity, message: impl Into<String>) -> Self {
        Self {
            severity,
            message: message.into(),
            diagnostic: None,
        }
    }

    pub fn with_diagnostic(mut self, category: DeviceErrorCategory, stage: &'static str) -> Self {
        self.diagnostic = Some(DeviceErrorDiagnostic::new(category, stage));
        self
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    pub fn with_severity_and_message(
        mut self,
        severity: DeviceErrorSeverity,
        message: impl Into<String>,
    ) -> Self {
        self.severity = severity;
        self.message = message.into();
        self
    }

    pub fn is_fallback_eligible(&self) -> bool {
        matches!(self.severity, DeviceErrorSeverity::Transient)
    }

    pub fn severity(&self) -> DeviceErrorSeverity {
        self.severity
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub const fn diagnostic(&self) -> Option<DeviceErrorDiagnostic> {
        self.diagnostic
    }
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.severity, self.message)
    }
}

impl Error for DeviceError {}
