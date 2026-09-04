// SPDX-License-Identifier: AGPL-3.0-only

use crate::{AdbInputBoundsContext, NemuConfiguredAdbClass, NemuResolutionContext};
use std::error::Error;
use std::fmt;

pub type DeviceResult<T> = Result<T, DeviceError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceErrorSeverity {
    Transient,
    Fatal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceClosePhase {
    Reset,
    ChildStop,
    StderrReaderJoin,
    UnexpectedStderr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCloseCause {
    pub phase: DeviceClosePhase,
    pub backend: &'static str,
    pub severity: DeviceErrorSeverity,
    pub detail: String,
    pub detail_truncated: bool,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceErrorSensitivity {
    Public,
    Internal,
    Sensitive,
    Secret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceErrorDiagnosticMessage {
    AdbDeviceStateAfterConnectAttempt,
    AdbDeviceStateConnectFailed,
    AdbDeviceStateConnectDisabled,
    AdbShellInputDeviceStateUnavailable,
    AdbShellInputBoundsUnavailableOrInvalid,
    AdbShellInputRotationUnavailable,
    DeviceRegistryCaptureOpenFailed,
    DeviceRegistryInputOpenFailed,
    DeviceRegistryInputOperationFailed,
    NemuCaptureIdentityUncoordinated,
    NemuInstallationResolveFailed,
    NemuTargetResolveFailed,
    SegmentedSwipeCapabilityUnsupported,
}

impl DeviceErrorDiagnosticMessage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AdbDeviceStateAfterConnectAttempt => {
                "adb device state is unavailable after one connect attempt"
            }
            Self::AdbDeviceStateConnectFailed => "adb device state and one connect attempt failed",
            Self::AdbDeviceStateConnectDisabled => {
                "adb device state is unavailable and connect is disabled"
            }
            Self::AdbShellInputDeviceStateUnavailable => {
                "adb shell input device state is unavailable"
            }
            Self::AdbShellInputBoundsUnavailableOrInvalid => {
                "adb shell input bounds are unavailable or invalid"
            }
            Self::AdbShellInputRotationUnavailable => "adb shell input rotation is unavailable",
            Self::DeviceRegistryCaptureOpenFailed => "device registry capture open failed",
            Self::DeviceRegistryInputOpenFailed => "device registry input open failed",
            Self::DeviceRegistryInputOperationFailed => "device registry input operation failed",
            Self::NemuCaptureIdentityUncoordinated => "Nemu capture identity is not coordinated",
            Self::NemuInstallationResolveFailed => "MuMu installation resolution failed",
            Self::NemuTargetResolveFailed => "Nemu running target resolution failed",
            Self::SegmentedSwipeCapabilityUnsupported => {
                "selected input backend does not support segmented swipe"
            }
        }
    }
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
pub struct DeviceErrorContext {
    backend: String,
    operation: String,
    declared_sensitivity: DeviceErrorSensitivity,
}

impl DeviceErrorContext {
    pub fn backend(&self) -> &str {
        &self.backend
    }

    pub fn operation(&self) -> &str {
        &self.operation
    }

    pub const fn declared_sensitivity(&self) -> DeviceErrorSensitivity {
        self.declared_sensitivity
    }
}

#[derive(Clone)]
enum StoredDiagnosticMessage {
    Fixed(DeviceErrorDiagnosticMessage),
    NemuResolution(NemuResolutionContext, String),
    AdbInputBounds(AdbInputBoundsContext, String),
}

#[derive(Clone)]
pub struct DeviceError {
    severity: DeviceErrorSeverity,
    message: String,
    diagnostic: Option<DeviceErrorDiagnostic>,
    context: Option<Box<DeviceErrorContext>>,
    diagnostic_message: Option<Box<StoredDiagnosticMessage>>,
    close_causes: Box<[DeviceCloseCause]>,
}

impl DeviceError {
    pub fn transient(message: impl Into<String>) -> Self {
        Self {
            severity: DeviceErrorSeverity::Transient,
            message: message.into(),
            diagnostic: None,
            context: None,
            diagnostic_message: None,
            close_causes: Box::default(),
        }
    }

    pub fn fatal(message: impl Into<String>) -> Self {
        Self {
            severity: DeviceErrorSeverity::Fatal,
            message: message.into(),
            diagnostic: None,
            context: None,
            diagnostic_message: None,
            close_causes: Box::default(),
        }
    }

    pub fn with_severity(severity: DeviceErrorSeverity, message: impl Into<String>) -> Self {
        Self {
            severity,
            message: message.into(),
            diagnostic: None,
            context: None,
            diagnostic_message: None,
            close_causes: Box::default(),
        }
    }

    pub fn with_diagnostic(mut self, category: DeviceErrorCategory, stage: &'static str) -> Self {
        self.diagnostic = Some(DeviceErrorDiagnostic::new(category, stage));
        self
    }

    pub fn close_causes(&self) -> &[DeviceCloseCause] {
        &self.close_causes
    }

    pub fn aggregate_close(backend: &'static str, phases: [Option<Self>; 4]) -> DeviceResult<()> {
        let mut messages = Vec::new();
        let mut causes = Vec::new();
        for (phase, error) in [
            DeviceClosePhase::Reset,
            DeviceClosePhase::ChildStop,
            DeviceClosePhase::StderrReaderJoin,
            DeviceClosePhase::UnexpectedStderr,
        ]
        .into_iter()
        .zip(phases)
        {
            if let Some(error) = error {
                messages.push(error.to_string());
                let mut end = error.message.len().min(1024);
                while !error.message.is_char_boundary(end) {
                    end -= 1;
                }
                causes.push(DeviceCloseCause {
                    phase,
                    backend,
                    severity: error.severity,
                    detail: error.message[..end].to_owned(),
                    detail_truncated: end < error.message.len(),
                });
            }
        }
        if causes.is_empty() {
            return Ok(());
        }
        let mut error = Self::fatal(messages.join("; "));
        error.close_causes = causes.into_boxed_slice();
        Err(error)
    }

    pub fn with_diagnostic_if_absent(
        mut self,
        category: DeviceErrorCategory,
        stage: &'static str,
    ) -> Self {
        if self.diagnostic.is_none() {
            self.diagnostic = Some(DeviceErrorDiagnostic::new(category, stage));
        }
        self
    }

    pub fn with_diagnostic_context(
        mut self,
        backend: impl Into<String>,
        operation: impl Into<String>,
        declared_sensitivity: DeviceErrorSensitivity,
    ) -> Self {
        self.context = Some(Box::new(DeviceErrorContext {
            backend: backend.into(),
            operation: operation.into(),
            declared_sensitivity,
        }));
        self
    }

    pub fn with_diagnostic_context_if_absent(
        mut self,
        backend: impl Into<String>,
        operation: impl Into<String>,
        declared_sensitivity: DeviceErrorSensitivity,
    ) -> Self {
        if self.context.is_none() {
            self.context = Some(Box::new(DeviceErrorContext {
                backend: backend.into(),
                operation: operation.into(),
                declared_sensitivity,
            }));
        }
        self
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    pub fn with_diagnostic_message(mut self, message: DeviceErrorDiagnosticMessage) -> Self {
        self.diagnostic_message = Some(Box::new(StoredDiagnosticMessage::Fixed(message)));
        self
    }

    pub fn with_nemu_resolution_context_if_absent(
        mut self,
        context: NemuResolutionContext,
    ) -> Self {
        if self.diagnostic_message.is_none()
            && !(self.diagnostic.is_some() && self.context.is_some())
        {
            self.diagnostic_message = Some(Box::new(StoredDiagnosticMessage::NemuResolution(
                context,
                context.render(),
            )));
        }
        self
    }

    pub(crate) fn with_nemu_resolution_provenance(
        mut self,
        configured_adb: Option<NemuConfiguredAdbClass>,
        explicit_root: bool,
        explicit_dll: bool,
    ) -> Self {
        if let Some(StoredDiagnosticMessage::NemuResolution(context, rendered)) =
            self.diagnostic_message.as_deref_mut()
        {
            *context = context.with_provenance(configured_adb, explicit_root, explicit_dll);
            *rendered = context.render();
        }
        self
    }

    pub fn with_adb_input_bounds_context_if_absent(
        mut self,
        context: AdbInputBoundsContext,
    ) -> Self {
        if self.diagnostic_message.is_none()
            && !(self.diagnostic.is_some() && self.context.is_some())
        {
            self.diagnostic_message = Some(Box::new(StoredDiagnosticMessage::AdbInputBounds(
                context,
                context.render(),
            )));
        }
        self
    }

    pub fn adb_input_bounds_context(&self) -> Option<AdbInputBoundsContext> {
        match self.diagnostic_message.as_deref() {
            Some(StoredDiagnosticMessage::AdbInputBounds(context, _)) => Some(*context),
            _ => None,
        }
    }

    pub fn nemu_resolution_context(&self) -> Option<NemuResolutionContext> {
        match self.diagnostic_message.as_deref() {
            Some(StoredDiagnosticMessage::NemuResolution(context, _)) => Some(*context),
            _ => None,
        }
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

    pub fn diagnostic_context(&self) -> Option<&DeviceErrorContext> {
        self.context.as_deref()
    }

    pub fn diagnostic_message(&self) -> Option<&str> {
        match self.diagnostic_message.as_deref() {
            Some(StoredDiagnosticMessage::Fixed(message)) => Some(message.as_str()),
            Some(StoredDiagnosticMessage::NemuResolution(_, rendered)) => Some(rendered),
            Some(StoredDiagnosticMessage::AdbInputBounds(_, rendered)) => Some(rendered),
            None => None,
        }
    }
}

impl fmt::Debug for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceError")
            .field("severity", &self.severity)
            .field("message", &self.message)
            .field("diagnostic", &self.diagnostic)
            .field("context", &self.context)
            .finish()
    }
}

impl PartialEq for DeviceError {
    fn eq(&self, other: &Self) -> bool {
        self.severity == other.severity
            && self.message == other.message
            && self.diagnostic == other.diagnostic
    }
}

impl Eq for DeviceError {}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.severity, self.message)
    }
}

impl Error for DeviceError {}
