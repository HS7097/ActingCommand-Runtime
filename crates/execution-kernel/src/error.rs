// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{
    CleanupCauseDraft, CleanupCauseSeverity, DiagnosticDetailDraft, EventId, InstanceId,
    LifecycleCauseDraft, LifecycleFailurePhase, LifecycleNativeDetail, Sensitivity,
};
use actingcommand_device::{
    DeviceClosePhase, DeviceError, DeviceErrorSensitivity, DeviceErrorSeverity,
};
use std::error::Error;
use std::fmt;
use std::sync::{Arc, OnceLock};

pub type ExecutionKernelResult<T> = Result<T, ExecutionKernelError>;

#[derive(Debug, Clone)]
pub struct ExecutionLifecycleCause {
    pub cause: LifecycleCauseDraft,
    pub recorded_event: Arc<OnceLock<EventId>>,
}

#[derive(Clone)]
pub struct ExecutionKernelError {
    code: &'static str,
    secondary_code: Option<&'static str>,
    device_severity: Option<DeviceErrorSeverity>,
    diagnostic_detail: Option<Box<DiagnosticDetailDraft>>,
    cleanup_cause: Option<Box<CleanupCauseDraft>>,
    lifecycle: Box<ExecutionFailureContext>,
}

#[derive(Clone, Default)]
struct ExecutionFailureContext {
    recorded_event: Arc<OnceLock<EventId>>,
    causes: Vec<ExecutionLifecycleCause>,
    closed_sessions: Vec<(InstanceId, ExecutionKernelError)>,
    instance_id: Option<InstanceId>,
}

impl PartialEq for ExecutionKernelError {
    fn eq(&self, other: &Self) -> bool {
        self.code == other.code
            && self.secondary_code == other.secondary_code
            && self.device_severity == other.device_severity
            && self.diagnostic_detail == other.diagnostic_detail
            && self.cleanup_cause == other.cleanup_cause
    }
}
impl Eq for ExecutionKernelError {}

impl ExecutionKernelError {
    pub(crate) fn fatal(code: &'static str) -> Self {
        Self {
            code,
            secondary_code: None,
            device_severity: None,
            diagnostic_detail: None,
            cleanup_cause: None,
            lifecycle: Box::default(),
        }
    }

    pub(crate) fn device(code: &'static str, error: &DeviceError) -> Self {
        Self {
            code,
            secondary_code: None,
            device_severity: Some(error.severity()),
            diagnostic_detail: device_diagnostic_detail(error),
            cleanup_cause: None,
            lifecycle: Box::new(ExecutionFailureContext {
                causes: error
                    .close_causes()
                    .iter()
                    .map(|cause| ExecutionLifecycleCause {
                        cause: LifecycleCauseDraft::new(
                            match cause.phase {
                                DeviceClosePhase::Reset => LifecycleFailurePhase::Reset,
                                DeviceClosePhase::ChildStop => LifecycleFailurePhase::ChildStop,
                                DeviceClosePhase::StderrReaderJoin => {
                                    LifecycleFailurePhase::StderrReaderJoin
                                }
                                DeviceClosePhase::UnexpectedStderr => {
                                    LifecycleFailurePhase::UnexpectedStderr
                                }
                            },
                            cause.backend,
                            code,
                            match cause.severity {
                                DeviceErrorSeverity::Transient => CleanupCauseSeverity::Transient,
                                DeviceErrorSeverity::Fatal => CleanupCauseSeverity::Fatal,
                            },
                        )
                        .with_native_detail(LifecycleNativeDetail::new(
                            &cause.detail,
                            cause.detail_truncated,
                        )),
                        recorded_event: Arc::new(OnceLock::new()),
                    })
                    .collect(),
                ..ExecutionFailureContext::default()
            }),
        }
    }

    pub(crate) fn merge(mut primary: Self, mut secondary: Self) -> Self {
        if primary.lifecycle.causes.is_empty() {
            primary.lifecycle.causes = std::mem::take(&mut secondary.lifecycle.causes);
        }
        if primary.cleanup_cause.is_none() {
            primary.cleanup_cause = secondary.cleanup_cause.take();
        }
        if primary.code == secondary.code
            && primary.secondary_code == secondary.secondary_code
            && primary.device_severity == secondary.device_severity
        {
            return primary;
        }
        Self {
            code: primary.code,
            secondary_code: Some(secondary.code),
            device_severity: merge_severity(primary.device_severity, secondary.device_severity),
            diagnostic_detail: primary.diagnostic_detail,
            cleanup_cause: primary.cleanup_cause,
            lifecycle: primary.lifecycle,
        }
    }

    pub(crate) fn merge_cleanup(mut primary: Self, secondary: Self) -> Self {
        if primary.cleanup_cause.is_none() {
            primary.cleanup_cause = Some(Box::new(CleanupCauseDraft::new(
                secondary.code,
                if secondary.is_fatal() {
                    CleanupCauseSeverity::Fatal
                } else {
                    CleanupCauseSeverity::Transient
                },
                secondary.diagnostic_detail().cloned(),
            )));
        }
        Self::merge(primary, secondary)
    }

    pub(crate) fn merge_retirement(mut primary: Self, secondary: Self) -> Self {
        primary.lifecycle.causes.push(ExecutionLifecycleCause {
            cause: LifecycleCauseDraft::new(
                LifecycleFailurePhase::Retirement,
                "execution_kernel",
                secondary.code,
                if secondary.is_fatal() {
                    CleanupCauseSeverity::Fatal
                } else {
                    CleanupCauseSeverity::Transient
                },
            )
            .with_detail(secondary.diagnostic_detail().cloned()),
            recorded_event: Arc::clone(&secondary.lifecycle.recorded_event),
        });
        Self::merge(primary, secondary)
    }

    pub fn recorded_event(&self) -> &Arc<OnceLock<EventId>> {
        &self.lifecycle.recorded_event
    }
    pub const fn instance_id(&self) -> Option<InstanceId> {
        self.lifecycle.instance_id
    }
    pub(crate) fn with_instance_id(mut self, instance_id: InstanceId) -> Self {
        self.lifecycle.instance_id = Some(instance_id);
        self
    }
    pub fn lifecycle_causes(&self) -> &[ExecutionLifecycleCause] {
        &self.lifecycle.causes
    }
    pub fn take_closed_sessions(&mut self) -> Vec<(InstanceId, ExecutionKernelError)> {
        std::mem::take(&mut self.lifecycle.closed_sessions)
    }
    pub(crate) fn with_closed_sessions(
        mut self,
        sessions: Vec<(InstanceId, ExecutionKernelError)>,
    ) -> Self {
        self.lifecycle.closed_sessions = sessions;
        self
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

    pub fn diagnostic_detail(&self) -> Option<&DiagnosticDetailDraft> {
        self.diagnostic_detail.as_deref()
    }

    pub fn cleanup_cause(&self) -> Option<&CleanupCauseDraft> {
        self.cleanup_cause.as_deref()
    }

    pub const fn is_fatal(&self) -> bool {
        !matches!(self.device_severity, Some(DeviceErrorSeverity::Transient))
    }
}

impl fmt::Debug for ExecutionKernelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExecutionKernelError")
            .field("code", &self.code)
            .field("secondary_code", &self.secondary_code)
            .field("device_severity", &self.device_severity)
            .finish()
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

fn device_diagnostic_detail(error: &DeviceError) -> Option<Box<DiagnosticDetailDraft>> {
    let diagnostic = error.diagnostic()?;
    let context = error.diagnostic_context()?;
    Some(Box::new(DiagnosticDetailDraft::new(
        diagnostic.category().as_str(),
        diagnostic.stage(),
        context.backend(),
        context.operation(),
        error
            .diagnostic_message()
            .unwrap_or_else(|| error.message()),
        match context.declared_sensitivity() {
            DeviceErrorSensitivity::Public => Sensitivity::Public,
            DeviceErrorSensitivity::Internal => Sensitivity::Internal,
            DeviceErrorSensitivity::Sensitive => Sensitivity::Sensitive,
            DeviceErrorSensitivity::Secret => Sensitivity::Secret,
        },
    )))
}

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
