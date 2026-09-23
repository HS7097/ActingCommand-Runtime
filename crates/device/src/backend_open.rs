// SPDX-License-Identifier: AGPL-3.0-only

use crate::{DeviceCloseOccurrence, DeviceError};
use actingcommand_contract::{
    BackendObservationStatus, BackendOpenEntry, BackendOpenReport, BackendOpenSource,
};
use std::sync::Arc;

/// An actual prime attempt's check, retained through admission/cleanup failure.
/// Absence on DeviceError means no check was observed, not a failed check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaptureProbeCheck {
    Passed {
        backend: crate::CaptureBackendName,
        width: u32,
        height: u32,
    },
    Failed {
        backend: crate::CaptureBackendName,
    },
}

impl CaptureProbeCheck {
    pub(crate) fn apply_failure(self, report: &mut BackendOpenReport, error: &DeviceError) {
        use actingcommand_contract::{BackendOpenAttempt, BackendOpenStage, LifecycleNativeDetail};
        let backend = match self {
            Self::Passed {
                backend,
                width,
                height,
            } => {
                report.capture_check = BackendObservationStatus::Passed;
                report.frame_width = Some(width);
                report.frame_height = Some(height);
                backend
            }
            Self::Failed { backend } => {
                report.capture_check = BackendObservationStatus::Failed;
                backend
            }
        };
        // The frame check and the candidate outcome are distinct: admission or
        // cleanup still failed, and no backend was selected by this return.
        report.push_attempt(BackendOpenAttempt {
            backend: backend.as_str().into(),
            stage: BackendOpenStage::CaptureProbe,
            status: BackendObservationStatus::Failed,
            elapsed_ms: None,
            detail: LifecycleNativeDetail::bounded(error.message()),
            input_parameters: None,
        });
    }
}

impl From<crate::MumuInstallSource> for actingcommand_contract::BackendInstallationSource {
    fn from(source: crate::MumuInstallSource) -> Self {
        match source {
            crate::MumuInstallSource::ExplicitFolder => Self::ExplicitFolder,
            crate::MumuInstallSource::ConfiguredBackendPath => Self::ConfiguredBackendPath,
            crate::MumuInstallSource::RunningProcess => Self::RunningProcess,
            crate::MumuInstallSource::RegistryUninstall => Self::RegistryUninstall,
            crate::MumuInstallSource::VendorEnumeration => Self::VendorEnumeration,
        }
    }
}

/// One occurrence travels with its triggering result, never with a reusable permission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendOpenObservation {
    pub report: BackendOpenReport,
    pub occurrence: Arc<DeviceCloseOccurrence>,
}

impl BackendOpenObservation {
    pub fn new(report: BackendOpenReport) -> Self {
        Self {
            report,
            occurrence: Arc::new(DeviceCloseOccurrence::default()),
        }
    }
}

pub struct OpenedBackend<T> {
    pub backend: T,
    pub observation: BackendOpenObservation,
}

impl<T> std::ops::Deref for OpenedBackend<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.backend
    }
}

impl<T> std::ops::DerefMut for OpenedBackend<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.backend
    }
}

impl<T> OpenedBackend<T> {
    pub fn new(backend: T, report: BackendOpenReport) -> Self {
        Self {
            backend,
            observation: BackendOpenObservation::new(report),
        }
    }

    pub fn unobserved(backend: T, entry: BackendOpenEntry) -> Self {
        Self::new(backend, BackendOpenReport::unobserved(entry))
    }

    pub fn simulation(backend: T, entry: BackendOpenEntry) -> Self {
        let mut report = BackendOpenReport::unobserved(entry);
        report.source = BackendOpenSource::Simulation;
        report.requested = "fixture_simulation".into();
        report.selected = Some(report.requested.clone());
        report.status = BackendObservationStatus::SimulationNotApplicable;
        report.connection = report.status;
        report.capture_check = report.status;
        if entry != BackendOpenEntry::Capture {
            report.input_check = report.status;
        }
        Self::new(backend, report)
    }

    pub fn map<U>(self, map: impl FnOnce(T) -> U) -> OpenedBackend<U> {
        OpenedBackend {
            backend: map(self.backend),
            observation: self.observation,
        }
    }
}

pub fn observe_open_failure(mut report: BackendOpenReport, error: DeviceError) -> DeviceError {
    if !error.backend_open_observations().is_empty() {
        return error.with_backend_open_configuration(&report);
    }
    if report.entry == BackendOpenEntry::NemuPair
        && report.source == BackendOpenSource::Native
        && let Some(check) = error.input_parameters()
    {
        check.apply_to_report(&mut report);
    }
    report.status = BackendObservationStatus::Failed;
    error.with_backend_open_observation(BackendOpenObservation::new(report))
}

pub(crate) fn observe_touch_action_connection(
    requested: crate::TouchBackendChoice,
    backend: crate::TouchBackendName,
    result: &crate::DeviceResult<crate::ConnectedTouchBackend>,
) -> BackendOpenObservation {
    use actingcommand_contract::{BackendOpenAttempt, BackendOpenStage, LifecycleNativeDetail};
    let mut report = BackendOpenReport::unobserved(BackendOpenEntry::Input);
    report.source = BackendOpenSource::Native;
    report.requested = requested.as_str().into();
    let (check, detail) = match result {
        Ok(connected) => {
            report.status = BackendObservationStatus::Passed;
            report.connection = BackendObservationStatus::Passed;
            report.selected = Some(connected.name.as_str().into());
            report.screen_size = LifecycleNativeDetail::bounded(&connected.device.screen_size);
            (connected.input_parameters.as_ref(), None)
        }
        Err(error) => {
            report.status = BackendObservationStatus::Failed;
            (
                error.input_parameters(),
                LifecycleNativeDetail::bounded(error.message()),
            )
        }
    };
    if let Some(check) = check {
        check.apply_to_report(&mut report);
    }
    report.push_attempt(BackendOpenAttempt {
        backend: backend.as_str().into(),
        stage: BackendOpenStage::Connect,
        status: report.status,
        // The original action fallback span includes the subsequent action.
        // It is not a connection-only measurement and must not be relabelled.
        elapsed_ms: None,
        detail,
        input_parameters: check.cloned(),
    });
    BackendOpenObservation::new(report)
}

impl crate::SelectedTouchBackend {
    pub fn open_report(&self, serial_configured: bool) -> BackendOpenReport {
        use actingcommand_contract::{BackendHandshakeObservation, LifecycleNativeDetail};
        let mut report = self.diagnostics().open_report();
        report.selected = Some(self.backend_name().as_str().into());
        report.status = BackendObservationStatus::Passed;
        report.connection = BackendObservationStatus::Passed;
        report.serial_configured = Some(serial_configured);
        report.input_geometry = self.opened_input_geometry();
        report.screen_size = LifecycleNativeDetail::bounded(&self.device_info().screen_size);
        report.handshake = self
            .handshake_info()
            .map(|value| BackendHandshakeObservation {
                max_contacts: value.max_contacts,
                max_x: value.max_x,
                max_y: value.max_y,
                max_pressure: value.max_pressure,
            });
        if let Some(check) = self.input_parameters() {
            check.apply_to_report(&mut report);
        } else {
            report.input_check = BackendObservationStatus::Unknown;
        }
        report
    }
}

impl crate::TouchBackendDiagnostics {
    pub(crate) fn open_report(&self) -> BackendOpenReport {
        use actingcommand_contract::{BackendOpenAttempt, BackendOpenStage, LifecycleNativeDetail};
        let mut report = BackendOpenReport::unobserved(BackendOpenEntry::Input);
        report.source = BackendOpenSource::Native;
        report.requested = self.requested.as_str().into();
        report.selected = self.selected.map(|backend| backend.as_str().into());
        for attempt in &self.attempts {
            report.push_attempt(BackendOpenAttempt {
                backend: attempt.backend.as_str().into(),
                stage: BackendOpenStage::Connect,
                status: if attempt.ok {
                    BackendObservationStatus::Passed
                } else {
                    BackendObservationStatus::Failed
                },
                elapsed_ms: u64::try_from(attempt.elapsed_ms).ok(),
                detail: attempt
                    .error_reason
                    .as_deref()
                    .and_then(LifecycleNativeDetail::bounded),
                input_parameters: attempt.input_parameters.clone(),
            });
        }
        if let Some(check) = self
            .attempts
            .last()
            .and_then(|attempt| attempt.input_parameters.as_ref())
        {
            check.apply_to_report(&mut report);
        }
        for warning in &self.warnings {
            if let Some(detail) = LifecycleNativeDetail::bounded(warning) {
                if report.warnings.len() < 8 {
                    report.warnings.push(detail);
                } else {
                    report.dropped_warnings = report.dropped_warnings.saturating_add(1);
                }
            }
        }
        report
    }
}

impl crate::SelectedCaptureBackend {
    pub fn open_report(&self) -> BackendOpenReport {
        let mut report = capture_open_report(
            self.diagnostics.requested,
            Some(self.diagnostics.used),
            &self.diagnostics.attempts,
        );
        report.status = BackendObservationStatus::Passed;
        if let Some(selection) = &self.selection {
            report.serial_configured = Some(selection.configured_serial.is_some());
            report.installation_source =
                selection.mumu.as_ref().map(|context| context.source.into());
        }
        if let Some((width, height)) = self.backend.opened_dimensions() {
            report.frame_width = Some(width);
            report.frame_height = Some(height);
            if self.diagnostics.used == crate::CaptureBackendName::NemuIpc {
                report.connection = BackendObservationStatus::Passed;
            }
        }
        report
    }
}

pub(crate) fn capture_open_report(
    requested: crate::CaptureBackendChoice,
    selected: Option<crate::CaptureBackendName>,
    attempts: &[crate::CaptureBackendAttempt],
) -> BackendOpenReport {
    use actingcommand_contract::{BackendOpenAttempt, BackendOpenStage, LifecycleNativeDetail};
    let mut report = BackendOpenReport::unobserved(BackendOpenEntry::Capture);
    report.source = BackendOpenSource::Native;
    report.requested = requested.as_str().into();
    report.selected = selected.map(|backend| backend.as_str().into());
    let automatic = matches!(
        requested,
        crate::CaptureBackendChoice::Auto | crate::CaptureBackendChoice::AutoFastest
    );
    for attempt in attempts {
        let stage = if attempt.cached {
            BackendOpenStage::CachedSelection
        } else if automatic {
            BackendOpenStage::CaptureProbe
        } else {
            BackendOpenStage::Construct
        };
        let status = if attempt.cached {
            BackendObservationStatus::Unknown
        } else if attempt.ok {
            BackendObservationStatus::Passed
        } else {
            BackendObservationStatus::Failed
        };
        if automatic && !attempt.cached && attempt.ok && Some(attempt.backend) == selected {
            report.capture_check = BackendObservationStatus::Passed;
            report.connection = BackendObservationStatus::Passed;
        }
        report.push_attempt(BackendOpenAttempt {
            backend: attempt.backend.as_str().into(),
            stage,
            status,
            elapsed_ms: if attempt.cached {
                None
            } else {
                attempt
                    .elapsed_ms
                    .and_then(|value| u64::try_from(value).ok())
            },
            detail: LifecycleNativeDetail::bounded(&attempt.message),
            input_parameters: None,
        });
    }
    report
}
