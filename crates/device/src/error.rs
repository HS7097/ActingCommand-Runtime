// SPDX-License-Identifier: AGPL-3.0-only

use crate::{AdbInputBoundsContext, NemuConfiguredAdbClass, NemuResolutionContext};
use std::error::Error;
use std::fmt;
use std::sync::{Arc, OnceLock};

pub type DeviceResult<T> = Result<T, DeviceError>;

/// Identity and the downstream ledger receipt travel with the original observation.
#[derive(Default, Debug)]
pub struct DeviceCloseOccurrence {
    recorded_event: OnceLock<Arc<dyn std::any::Any + Send + Sync>>,
}

impl DeviceCloseOccurrence {
    pub fn recorded_event<T: Send + Sync + 'static>(&self) -> Arc<OnceLock<T>> {
        Arc::clone(
            self.recorded_event
                .get_or_init(|| Arc::new(OnceLock::<T>::new())),
        )
        .downcast::<OnceLock<T>>()
        .expect("one ledger event type per device occurrence")
    }
}

impl PartialEq for DeviceCloseOccurrence {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}
impl Eq for DeviceCloseOccurrence {}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceCloseAuthority {
    LocalOnly,
    FencedDeviceWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceResourceQuiescence {
    Confirmed,
    Unconfirmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceResourceKind {
    CaptureBackend,
    InputBackend,
    ProviderConnection,
    VendorStdio,
    ExternalChild,
    InProcessWorker,
    Library,
    FileDescriptor,
    TemporaryPath,
    PipeReader,
    FactoryCandidate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceResourceClosePhase {
    Close,
    AcquisitionCleanup,
    DisconnectSymbol,
    DisconnectCall,
    WorkerSend,
    WorkerReceive,
    WorkerJoin,
    InitialPoll,
    Kill,
    ExitPoll,
    Deadline,
    RestoreFlush,
    RestoreWin32,
    RestoreCrt,
    SnapshotFlush,
    SnapshotRead,
    FileDescriptorClose,
    Unlink,
    LibraryUnload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceResourceCloseOutcome {
    quiescence: DeviceResourceQuiescence,
    resource_count: u16,
}

impl DeviceResourceCloseOutcome {
    pub const fn confirmed(resource_count: u16) -> Self {
        Self {
            quiescence: DeviceResourceQuiescence::Confirmed,
            resource_count,
        }
    }

    pub const fn quiescence(self) -> DeviceResourceQuiescence {
        self.quiescence
    }

    pub const fn resource_count(self) -> u16 {
        self.resource_count
    }

    pub fn combine(self, other: Self) -> Self {
        Self {
            quiescence: if matches!(
                (self.quiescence, other.quiescence),
                (
                    DeviceResourceQuiescence::Confirmed,
                    DeviceResourceQuiescence::Confirmed
                )
            ) {
                DeviceResourceQuiescence::Confirmed
            } else {
                DeviceResourceQuiescence::Unconfirmed
            },
            resource_count: self.resource_count.saturating_add(other.resource_count),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceResourceCloseCause {
    occurrence: Arc<DeviceCloseOccurrence>,
    resource: DeviceResourceKind,
    phase: DeviceResourceClosePhase,
    backend: &'static str,
    candidate_index: Option<u8>,
    native_instance: Option<i32>,
    severity: DeviceErrorSeverity,
    detail: String,
    detail_truncated: bool,
    last_detail: Option<String>,
    last_detail_truncated: bool,
    observation_count: u16,
    dropped_count: u16,
}

impl DeviceResourceCloseCause {
    pub fn occurrence(&self) -> &Arc<DeviceCloseOccurrence> {
        &self.occurrence
    }

    pub fn last_detail(&self) -> Option<&str> {
        self.last_detail.as_deref()
    }

    pub const fn last_detail_truncated(&self) -> bool {
        self.last_detail_truncated
    }
    pub const fn resource(&self) -> DeviceResourceKind {
        self.resource
    }

    pub const fn phase(&self) -> DeviceResourceClosePhase {
        self.phase
    }

    pub const fn backend(&self) -> &'static str {
        self.backend
    }

    pub const fn candidate_index(&self) -> Option<u8> {
        self.candidate_index
    }

    pub const fn native_instance(&self) -> Option<i32> {
        self.native_instance
    }

    pub const fn severity(&self) -> DeviceErrorSeverity {
        self.severity
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    pub const fn detail_truncated(&self) -> bool {
        self.detail_truncated
    }

    pub const fn observation_count(&self) -> u16 {
        self.observation_count
    }

    pub const fn dropped_count(&self) -> u16 {
        self.dropped_count
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCloseCause {
    pub occurrence: Arc<DeviceCloseOccurrence>,
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
    occurrence: Arc<DeviceCloseOccurrence>,
    severity: DeviceErrorSeverity,
    message: String,
    diagnostic: Option<DeviceErrorDiagnostic>,
    context: Option<Box<DeviceErrorContext>>,
    diagnostic_message: Option<Box<StoredDiagnosticMessage>>,
    close_causes: Box<[DeviceCloseCause]>,
    resource_close_causes: Box<[DeviceResourceCloseCause]>,
    resource_quiescence: Option<DeviceResourceQuiescence>,
    resource_count: u16,
}

impl DeviceError {
    pub fn transient(message: impl Into<String>) -> Self {
        Self {
            occurrence: Arc::new(DeviceCloseOccurrence::default()),
            severity: DeviceErrorSeverity::Transient,
            message: message.into(),
            diagnostic: None,
            context: None,
            diagnostic_message: None,
            close_causes: Box::default(),
            resource_close_causes: Box::default(),
            resource_quiescence: None,
            resource_count: 0,
        }
    }

    pub fn fatal(message: impl Into<String>) -> Self {
        Self {
            occurrence: Arc::new(DeviceCloseOccurrence::default()),
            severity: DeviceErrorSeverity::Fatal,
            message: message.into(),
            diagnostic: None,
            context: None,
            diagnostic_message: None,
            close_causes: Box::default(),
            resource_close_causes: Box::default(),
            resource_quiescence: None,
            resource_count: 0,
        }
    }

    pub fn with_severity(severity: DeviceErrorSeverity, message: impl Into<String>) -> Self {
        Self {
            occurrence: Arc::new(DeviceCloseOccurrence::default()),
            severity,
            message: message.into(),
            diagnostic: None,
            context: None,
            diagnostic_message: None,
            close_causes: Box::default(),
            resource_close_causes: Box::default(),
            resource_quiescence: None,
            resource_count: 0,
        }
    }

    pub fn with_diagnostic(mut self, category: DeviceErrorCategory, stage: &'static str) -> Self {
        self.diagnostic = Some(DeviceErrorDiagnostic::new(category, stage));
        self
    }

    pub fn close_causes(&self) -> &[DeviceCloseCause] {
        &self.close_causes
    }

    pub fn resource_close_causes(&self) -> &[DeviceResourceCloseCause] {
        &self.resource_close_causes
    }

    pub const fn resource_quiescence(&self) -> Option<DeviceResourceQuiescence> {
        self.resource_quiescence
    }

    pub const fn resource_count(&self) -> u16 {
        self.resource_count
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_resource_close_cause(
        mut self,
        resource: DeviceResourceKind,
        phase: DeviceResourceClosePhase,
        backend: &'static str,
        candidate_index: Option<u8>,
        native_instance: Option<i32>,
        quiescence: DeviceResourceQuiescence,
        resource_count: u16,
    ) -> Self {
        let mut end = self.message.len().min(1024);
        while !self.message.is_char_boundary(end) {
            end -= 1;
        }
        let cause = DeviceResourceCloseCause {
            occurrence: Arc::new(DeviceCloseOccurrence::default()),
            resource,
            phase,
            backend,
            candidate_index,
            native_instance,
            severity: self.severity,
            detail: self.message[..end].to_owned(),
            detail_truncated: end < self.message.len(),
            last_detail: None,
            last_detail_truncated: false,
            observation_count: 1,
            dropped_count: 0,
        };
        let mut causes = self.resource_close_causes.into_vec();
        merge_resource_cause(&mut causes, cause);
        self.resource_close_causes = causes.into_boxed_slice();
        self.resource_quiescence = Some(match (self.resource_quiescence, quiescence) {
            (Some(DeviceResourceQuiescence::Unconfirmed), _)
            | (_, DeviceResourceQuiescence::Unconfirmed) => DeviceResourceQuiescence::Unconfirmed,
            _ => DeviceResourceQuiescence::Confirmed,
        });
        self.resource_count = self.resource_count.saturating_add(resource_count);
        self
    }

    pub fn with_resource_quiescence(
        mut self,
        quiescence: DeviceResourceQuiescence,
        resource_count: u16,
    ) -> Self {
        self.resource_quiescence = Some(quiescence);
        self.resource_count = self.resource_count.max(resource_count);
        self
    }

    pub fn with_resource_summary(
        mut self,
        quiescence: DeviceResourceQuiescence,
        resource_count: u16,
    ) -> Self {
        self.resource_quiescence = Some(quiescence);
        self.resource_count = resource_count;
        self
    }

    pub fn with_resource_candidate_index(mut self, candidate_index: u8) -> Self {
        for cause in &mut self.resource_close_causes {
            if cause.candidate_index.is_none() {
                cause.candidate_index = Some(candidate_index);
            }
        }
        self
    }

    pub fn merge_resource_cleanup(mut self, cleanup: Self) -> Self {
        let mut causes = self.resource_close_causes.into_vec();
        let mut new_occurrence = cleanup.resource_close_causes.is_empty()
            && !Arc::ptr_eq(&self.occurrence, &cleanup.occurrence);
        for cause in cleanup.resource_close_causes {
            new_occurrence |= merge_resource_cause(&mut causes, cause);
        }
        self.resource_close_causes = causes.into_boxed_slice();
        self.resource_quiescence = match (self.resource_quiescence, cleanup.resource_quiescence) {
            (Some(DeviceResourceQuiescence::Unconfirmed), _)
            | (_, Some(DeviceResourceQuiescence::Unconfirmed)) => {
                Some(DeviceResourceQuiescence::Unconfirmed)
            }
            (Some(DeviceResourceQuiescence::Confirmed), _)
            | (_, Some(DeviceResourceQuiescence::Confirmed)) => {
                Some(DeviceResourceQuiescence::Confirmed)
            }
            (None, None) => None,
        };
        if new_occurrence {
            self.resource_count = self.resource_count.saturating_add(cleanup.resource_count);
        }
        if matches!(cleanup.severity, DeviceErrorSeverity::Fatal) {
            self.severity = DeviceErrorSeverity::Fatal;
        }
        self
    }

    /// Fold repeated observations at the producing operation, before cloning its cause.
    pub(crate) fn fold_resource_observation(&mut self, incoming: Self) -> DeviceResult<()> {
        let current = self
            .resource_close_causes
            .first_mut()
            .expect("resource observation has a cause");
        let next = incoming
            .resource_close_causes
            .first()
            .expect("resource observation has a cause");
        current.observation_count = current.observation_count.checked_add(1).ok_or_else(|| {
            DeviceError::fatal("child close observation capacity exceeded")
                .with_resource_quiescence(DeviceResourceQuiescence::Unconfirmed, 1)
        })?;
        current.last_detail = Some(next.detail.clone());
        current.last_detail_truncated = next.detail_truncated;
        current.dropped_count = current.observation_count.saturating_sub(2);
        Ok(())
    }

    pub fn aggregate_close(backend: &'static str, phases: [Option<Self>; 4]) -> DeviceResult<()> {
        let mut messages = Vec::new();
        let mut causes = Vec::new();
        let mut resource_causes = Vec::new();
        let mut quiescence = DeviceResourceQuiescence::Confirmed;
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
                let disposition = error.resource_quiescence.unwrap_or(match phase {
                    DeviceClosePhase::UnexpectedStderr => DeviceResourceQuiescence::Confirmed,
                    _ => DeviceResourceQuiescence::Unconfirmed,
                });
                if disposition == DeviceResourceQuiescence::Unconfirmed {
                    quiescence = DeviceResourceQuiescence::Unconfirmed;
                }
                for cause in error.resource_close_causes.iter().cloned() {
                    merge_resource_cause(&mut resource_causes, cause);
                }
                messages.push(error.to_string());
                let mut end = error.message.len().min(1024);
                while !error.message.is_char_boundary(end) {
                    end -= 1;
                }
                causes.push(DeviceCloseCause {
                    occurrence: Arc::clone(&error.occurrence),
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
        error.resource_close_causes = resource_causes.into_boxed_slice();
        error.resource_quiescence = Some(quiescence);
        error.resource_count = 1;
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
            && self.resource_quiescence.is_none()
            && self.resource_close_causes.is_empty()
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
            && self.resource_close_causes == other.resource_close_causes
            && self.resource_quiescence == other.resource_quiescence
            && self.resource_count == other.resource_count
    }
}

impl Eq for DeviceError {}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.severity, self.message)
    }
}

impl Error for DeviceError {}

fn merge_resource_cause(
    causes: &mut Vec<DeviceResourceCloseCause>,
    incoming: DeviceResourceCloseCause,
) -> bool {
    if causes
        .iter()
        .any(|cause| Arc::ptr_eq(&cause.occurrence, &incoming.occurrence))
    {
        return false;
    }
    causes.push(incoming);
    true
}
