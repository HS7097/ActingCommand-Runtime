// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_device::{DeviceError, DeviceErrorSeverity};
use std::error::Error;
use std::fmt;

pub type ExecutionKernelResult<T> = Result<T, ExecutionKernelError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionKernelError {
    code: &'static str,
    secondary_code: Option<&'static str>,
    device_severity: Option<DeviceErrorSeverity>,
}

impl ExecutionKernelError {
    pub(crate) const fn fatal(code: &'static str) -> Self {
        Self {
            code,
            secondary_code: None,
            device_severity: None,
        }
    }

    pub(crate) fn device(code: &'static str, error: &DeviceError) -> Self {
        Self {
            code,
            secondary_code: None,
            device_severity: Some(error.severity()),
        }
    }

    pub(crate) fn merge(primary: Self, secondary: Self) -> Self {
        if primary == secondary {
            return primary;
        }
        Self {
            code: primary.code,
            secondary_code: Some(secondary.code),
            device_severity: merge_severity(primary.device_severity, secondary.device_severity),
        }
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub const fn secondary_code(&self) -> Option<&'static str> {
        self.secondary_code
    }

    pub const fn device_severity(&self) -> Option<DeviceErrorSeverity> {
        self.device_severity
    }

    pub const fn is_fatal(&self) -> bool {
        !matches!(self.device_severity, Some(DeviceErrorSeverity::Transient))
    }
}

impl fmt::Display for ExecutionKernelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.secondary_code {
            Some(secondary) => write!(
                formatter,
                "execution kernel error {} with cleanup error {secondary}",
                self.code
            ),
            None => write!(formatter, "execution kernel error {}", self.code),
        }
    }
}

impl Error for ExecutionKernelError {}

const fn merge_severity(
    left: Option<DeviceErrorSeverity>,
    right: Option<DeviceErrorSeverity>,
) -> Option<DeviceErrorSeverity> {
    match (left, right) {
        (Some(DeviceErrorSeverity::Fatal), _) | (_, Some(DeviceErrorSeverity::Fatal)) => {
            Some(DeviceErrorSeverity::Fatal)
        }
        (Some(DeviceErrorSeverity::Transient), _) | (_, Some(DeviceErrorSeverity::Transient)) => {
            Some(DeviceErrorSeverity::Transient)
        }
        (None, None) => None,
    }
}
