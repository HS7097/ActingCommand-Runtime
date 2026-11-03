// SPDX-License-Identifier: AGPL-3.0-only

#[cfg(feature = "fixture-backends")]
mod fixtures;
// HS-4 (#402): the capability table names what the Nemu input performs.
#[cfg(test)]
mod gate_hs4;

use crate::codes::{HostCode, HostLocation};
use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::outcome::Location;
use actingcommand_contract::{
    ApplicationLifecycleAction, EmulatorCapability, EmulatorCapabilityAvailability,
    EmulatorCapabilityEvidence, EmulatorCapabilityImplementation, EmulatorCapabilityProfile,
    EmulatorInstanceAction, EmulatorVersionEvidence, EmulatorVersionUnavailableReason, FencedWrite,
    InstanceId, MAX_INSTANCE_ALIAS_BYTES, RuntimeErrorCode,
};
use actingcommand_device::{
    Adb, AdbConfig, CaptureBackend, CaptureBackendChoice, CaptureBackendConfig, CaptureOpenCause,
    DeviceError, DeviceErrorCategory, DeviceErrorDiagnosticMessage, DeviceErrorSensitivity,
    DeviceLocation, DeviceResult, DeviceTarget, InputBackend, MumuApplicationState,
    MumuDiscoveryReport, NemuAppIndex, NemuAppIndexProblem, NemuApplicationTarget, NemuInputConfig,
    NemuIpcSession, NemuResolutionReason, NemuSessionBackends, PreparedSegmentedSwipePlan,
    TouchBackendChoice, TouchBackendConfig, create_touch_backend_for_fenced_input,
    discover_mumu_instances, mumu_state_wait,
};
use actingcommand_device::{AdbDeviceStateShell, DeviceStateShell};
use actingcommand_device::{AdbTrackDevices, DeviceSerial, TrackDevicesEvent, TrackDevicesStopper};
pub use actingcommand_execution_kernel::{
    DeviceLinkWatch, DeviceLinkWatchEvent, DeviceLinkWatchStop, DeviceProcessPresence,
    DiscoveredInstanceBinding, EmulatorControlFailure, EmulatorControlOutcome,
    EmulatorControlResult, ExecutionBackendProvider, ForegroundApplicationObservation,
    HandsetBinding, HandsetEndpoint, InstanceDiscoveryFailure, InstanceState, PendingAdbEndpoint,
    ProviderDiscoveredInstance, ProviderInstanceDiscovery, RecognitionVisionProvider,
    ResolvedAdbEndpoint, ResolvedExecutionInstance, ResolvedInstanceEndpoint, VisionFfiProvider,
    VisionModelIdentity,
};
#[cfg(feature = "fixture-backends")]
pub use fixtures::FixtureInstanceSpec;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

/// Placeholder port of a pending discovery binding: never dispatched, because every path that
/// opens a session refuses a pending entry first, and always overwritten by the rebind.
const PENDING_ADB_PORT: u16 = 0;

/// The bounded `info -v <index>` read of a discovery-bound instance:
/// `actingcommand_device::read_instance_state` in production, a fake in the registry's tests.
type InstanceStateReader = Arc<dyn Fn(&Path, u16) -> DeviceResult<InstanceState> + Send + Sync>;

/// #402 HS-3 (design HS-M §4.1): the bounded `control -v <index> app info -pkg <package>` read a
/// paired Nemu open makes when the configuration names no `nemu_app_index`, through the given
/// `MuMuManager.exe` and MuMu index: `actingcommand_device::read_application_state` in
/// production (`MUMU_MANAGER_COMMAND_TIMEOUT`), a fake in the registry's tests.
type ApplicationStateReader =
    Arc<dyn Fn(&Path, u16, &str) -> DeviceResult<MumuApplicationState> + Send + Sync>;

/// #402 HS-3: the paired Nemu open itself, `NemuIpcSession::open` in production; the registry's
/// tests put a fake in its place, which opens no device.
type NemuSessionOpener = Arc<
    dyn Fn(
            CaptureBackendConfig,
            NemuApplicationTarget,
            NemuInputConfig,
            Option<&actingcommand_device::FrameMemoryBudget>,
        ) -> DeviceResult<actingcommand_device::OpenedBackend<NemuSessionBackends>>
        + Send
        + Sync,
>;

/// #402 HS-3 (design HS-M §4.1): the application index a paired Nemu open resolves to when the
/// configuration names none and `MuMuManager` reports the application running: the main
/// application, not a clone (the frozen capture bench used it too).
const RESOLVED_NEMU_APP_INDEX: u32 = 0;

/// The typed, serde-free assembly the daemon hands the registry: every instance it registers,
/// the vision provider it injects and the discovery surface it exposes. Built by the daemon
/// from its configuration; consumed once by [`ExecutionBackendRegistry::from_assembly`].
pub struct ProviderAssembly {
    pub instances: Vec<InstanceSpec>,
    pub vision: Option<VisionSpec>,
    pub discovery: Option<DiscoverySpec>,
}

/// One instance to register: its alias, its identity and the backends behind it.
pub struct InstanceSpec {
    alias: String,
    instance_id: InstanceId,
    mode: InstanceMode,
}

pub enum InstanceMode {
    /// A physical device or emulator reached through ADB (and optionally Nemu IPC).
    Real(Box<ExecutionBackendRegistration>),
    /// A device-free fixture replaying configured frames.
    #[cfg(feature = "fixture-backends")]
    Fixture(FixtureInstanceSpec),
}

impl InstanceSpec {
    /// A real instance; alias and identity are the registration's.
    pub fn real(registration: ExecutionBackendRegistration) -> Self {
        Self {
            alias: registration.instance_alias.clone(),
            instance_id: registration.instance_id,
            mode: InstanceMode::Real(Box::new(registration)),
        }
    }

    /// A fixture instance under the same alias rules as a real one.
    #[cfg(feature = "fixture-backends")]
    pub fn fixture(
        alias: impl Into<String>,
        instance_id: InstanceId,
        fixture: FixtureInstanceSpec,
    ) -> RuntimeHostResult<Self> {
        let alias = alias.into();
        validate_alias(&alias)?;
        Ok(Self {
            alias,
            instance_id,
            mode: InstanceMode::Fixture(fixture),
        })
    }

    pub fn alias(&self) -> &str {
        &self.alias
    }

    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub const fn mode(&self) -> &InstanceMode {
        &self.mode
    }
}

/// The vision provider injected into the registry, already constructed by provider startup.
pub struct VisionSpec {
    provider: Arc<dyn RecognitionVisionProvider>,
}

impl VisionSpec {
    pub fn new(provider: Arc<dyn RecognitionVisionProvider>) -> Self {
        Self { provider }
    }
}

/// The provider's instance-discovery surface: `MuMuManager` discovery from an explicit
/// install root or, without one, from the resolver's own sources.
pub struct DiscoverySpec {
    mumu_root: Option<PathBuf>,
    /// The caller-injected `ACTINGCOMMAND_NEMU_FOLDER` value (Workflow #318 cfg3); the
    /// resolver's environment rung takes part only when it is set.
    env_nemu_folder: Option<PathBuf>,
}

impl DiscoverySpec {
    pub fn new(mumu_root: Option<PathBuf>) -> Self {
        Self {
            mumu_root,
            env_nemu_folder: None,
        }
    }

    /// Injects the `ACTINGCOMMAND_NEMU_FOLDER` fallback; `None` leaves that rung out.
    pub fn with_env_nemu_folder(mut self, env_nemu_folder: Option<PathBuf>) -> Self {
        self.env_nemu_folder = env_nemu_folder;
        self
    }

    pub fn mumu_root(&self) -> Option<&Path> {
        self.mumu_root.as_deref()
    }

    /// One `MuMuManager` discovery run, its refusal classified as the provider view reports
    /// it: a version below the policy floor or unparseable is
    /// `mumu_manager_version_unsupported`, anything else (no install, tool spawn, exit,
    /// timeout, decode or JSON failure) `instance_discovery_unavailable`. Binds, rebinds and
    /// registers nothing.
    pub fn discover(&self) -> Result<MumuDiscoveryReport, Box<InstanceDiscoveryFailure>> {
        let env_nemu_folder = self.env_nemu_folder.as_deref();
        discover_mumu_instances(self.mumu_root.as_deref(), env_nemu_folder).map_err(|error| {
            let code = if matches!(
                error
                    .nemu_resolution_context()
                    .map(|context| context.reason()),
                Some(
                    NemuResolutionReason::ProviderVersionBelowMinimum
                        | NemuResolutionReason::ProviderVersionUnparseable
                )
            ) {
                HostCode::MumuManagerVersionUnsupported.as_str()
            } else {
                SharedCode::InstanceDiscoveryUnavailable.as_str()
            };
            Box::new(InstanceDiscoveryFailure { code, error })
        })
    }
}

pub struct ExecutionBackendRegistration {
    instance_alias: String,
    instance_id: InstanceId,
    application_id: String,
    input: TouchBackendConfig,
    capture: CaptureBackendConfig,
    configuration: actingcommand_contract::EffectiveDeviceConfiguration,
    discovered: Option<DiscoveredInstanceBinding>,
    /// Discovery reported the instance stopped: the target carries no port yet.
    endpoint_pending: bool,
    provider_profile: Option<EmulatorCapabilityProfile>,
    nemu_app_index: Option<NemuAppIndex>,
    /// Workflow #398 RD-1b: the handset behind the targets; `None` for an emulator.
    handset: Option<HandsetEndpoint>,
}

impl ExecutionBackendRegistration {
    pub fn new(
        instance_alias: impl Into<String>,
        instance_id: InstanceId,
        application_id: impl Into<String>,
        input: TouchBackendConfig,
        capture: CaptureBackendConfig,
    ) -> RuntimeHostResult<Self> {
        let instance_alias = instance_alias.into();
        let application_id = application_id.into();
        validate_alias(&instance_alias)?;
        validate_application_id(&application_id)?;
        check_backend_selection(
            &input,
            &capture,
            HostLocation::BuildExecutionBackendRegistry,
        )?;
        let milliseconds = |duration: Duration| {
            u64::try_from(duration.as_millis()).map_err(|_| {
                RuntimeHostError::fatal(
                    HostCode::ExecutionConfigurationTimeoutOverflow,
                    HostLocation::BuildExecutionBackendRegistry,
                    RuntimeErrorCode::RuntimeHalted,
                )
            })
        };
        let configuration = actingcommand_contract::EffectiveDeviceConfiguration {
            input_backend: input.requested.as_str().to_owned(),
            capture_backend: capture.requested.as_str().to_owned(),
            input_adb: input.adb_config.adb_path.clone(),
            capture_adb: capture.adb_config.adb_path.clone(),
            // Workflow #398 RD-1a (L1): the record names the target by its label.
            configured_serial: input.target.configured_label(),
            resolved_serial: input.target.label(),
            input_command_timeout_ms: milliseconds(input.adb_config.command_timeout)?,
            capture_command_timeout_ms: milliseconds(capture.adb_config.command_timeout)?,
            capture_timeout_ms: milliseconds(capture.capture_timeout)?,
            configured_mumu_root: capture.nemu.nemu_folder.clone(),
            configured_capture_dll: capture.nemu.dll_path.clone(),
        };
        Ok(Self {
            instance_alias,
            instance_id,
            application_id,
            input,
            capture,
            configuration,
            discovered: None,
            endpoint_pending: false,
            provider_profile: None,
            nemu_app_index: None,
            handset: None,
        })
    }

    /// Workflow #398 RD-1b (model-398 v1.1 §3.1): marks the registration as a handset's. Its
    /// targets are addressed by the bound serial; an unbound handset's targets carry the
    /// unbound stand-in serial and the registry refuses every session on it.
    pub fn with_handset(mut self, handset: HandsetEndpoint) -> Self {
        self.handset = Some(handset);
        self
    }

    /// The input backend's target (the capture target is the same, see [`Self::new`]).
    pub const fn input_target(&self) -> &DeviceTarget {
        &self.input.target
    }

    /// The capture backend's target.
    pub const fn capture_target(&self) -> &DeviceTarget {
        &self.capture.target
    }

    /// Marks the registration as bound through MuMu instance discovery.
    pub fn with_discovered_binding(mut self, discovered: DiscoveredInstanceBinding) -> Self {
        self.discovered = Some(discovered);
        self.endpoint_pending = false;
        self
    }

    /// Marks the registration as discovered stopped: it is registered with a pending endpoint
    /// (no port) and bound once emulator control starts the instance.
    pub fn with_pending_discovered_binding(
        mut self,
        discovered: DiscoveredInstanceBinding,
    ) -> Self {
        self.discovered = Some(discovered);
        self.endpoint_pending = true;
        self
    }

    /// Attaches the provider capability profile admitted at startup; its provider-owned rows
    /// replace the registry placeholder rows when the registration is registered.
    pub fn with_capability_profile(mut self, profile: EmulatorCapabilityProfile) -> Self {
        self.provider_profile = Some(profile);
        self
    }

    /// The configuration's `nemu_app_index`, whatever the registration's backends (#402 HS-3:
    /// an emulator entry may name it beside any backends since `actingd-config` revision 26, and
    /// a backend switch to Nemu touch opens with it). Only a paired Nemu open uses it, and then
    /// the file's value wins: nothing is read from `MuMuManager`.
    pub fn with_nemu_app_index(mut self, app_index: NemuAppIndex) -> RuntimeHostResult<Self> {
        NemuApplicationTarget::new(&self.application_id, app_index).map_err(|error| {
            RuntimeHostError::fatal(
                HostCode::NemuApplicationIdentityInvalid,
                HostLocation::BuildExecutionBackendRegistry,
                RuntimeErrorCode::RuntimeHalted,
            )
            .with_native_detail(error.to_string())
        })?;
        self.nemu_app_index = Some(app_index);
        Ok(self)
    }
}

/// The registration's own checks of a pair of backends (#402 HS-2: shared by the registration
/// and a backend switch, design HS-M §3.2 step 3.3): both chosen explicitly, both on one target,
/// and Nemu touch only beside Nemu capture. A refusal names `operation`.
fn check_backend_selection(
    input: &TouchBackendConfig,
    capture: &CaptureBackendConfig,
    operation: HostLocation,
) -> RuntimeHostResult<()> {
    if matches!(
        input.requested,
        TouchBackendChoice::Auto | TouchBackendChoice::AutoFastest
    ) || matches!(
        capture.requested,
        CaptureBackendChoice::Auto | CaptureBackendChoice::AutoFastest
    ) {
        return Err(RuntimeHostError::fatal(
            HostCode::ExecutionBackendSelectionNotExplicit,
            operation,
            RuntimeErrorCode::RuntimeHalted,
        ));
    }
    if input.target.adb_arg() != capture.target.adb_arg() {
        return Err(RuntimeHostError::fatal(
            HostCode::ExecutionBackendTargetMismatch,
            operation,
            RuntimeErrorCode::RuntimeHalted,
        ));
    }
    if input.requested == TouchBackendChoice::NemuIpc
        && capture.requested != CaptureBackendChoice::NemuIpc
    {
        return Err(RuntimeHostError::fatal(
            HostCode::NemuInputRequiresPairedCapture,
            operation,
            RuntimeErrorCode::RuntimeHalted,
        ));
    }
    Ok(())
}

enum RegistryEntry {
    Real(Box<ExecutionBackendEntry>),
    #[cfg(feature = "fixture-backends")]
    Fixture(fixtures::FixtureEntry),
}

impl RegistryEntry {
    const fn instance_id(&self) -> InstanceId {
        match self {
            Self::Real(entry) => entry.instance_id,
            #[cfg(feature = "fixture-backends")]
            Self::Fixture(fixture) => fixture.instance_id(),
        }
    }

    /// Workflow #398 RD-2: a bound handset's serial and adb; `None` for every other entry.
    fn bound_handset(&self) -> Option<(DeviceSerial, String)> {
        match self {
            Self::Real(entry) => entry.bound_handset(),
            #[cfg(feature = "fixture-backends")]
            Self::Fixture(_) => None,
        }
    }
}

struct ExecutionBackendEntry {
    instance_id: InstanceId,
    application_id: String,
    application_adb: AdbConfig,
    /// The provider capability profile admitted at startup, whose provider-owned rows every
    /// capability table of the entry keeps (#402 HS-2: a backend switch recomputes the table).
    provider_profile: Option<EmulatorCapabilityProfile>,
    nemu_app_index: Option<NemuAppIndex>,
    /// The ADB endpoint and everything built on it, behind a lock because emulator control
    /// (re)binds a discovery-bound entry and (#402 HS-2) a backend switch replaces its backends.
    /// Every guarded section only copies plain data.
    endpoint: Mutex<EntryEndpoint>,
}

struct EntryEndpoint {
    state: ResolvedInstanceEndpoint,
    audit_endpoint: String,
    application_target: DeviceTarget,
    input: TouchBackendConfig,
    capture: CaptureBackendConfig,
    configuration: actingcommand_contract::EffectiveDeviceConfiguration,
    /// The capability table of the backends in effect (#402 HS-2: under the lock, since a
    /// switch replaces it with the backends).
    capabilities: EmulatorCapabilityProfile,
    /// #402 HS-3 (design HS-M §4.1, "the cache"): the Nemu application index a paired open
    /// resolved from `MuMuManager` when the configuration names none, for the entry's MuMu index,
    /// package and root, in memory only. A rebinding drops it, and so does a paired open that
    /// found no display for the package; the next paired open reads again.
    resolved_nemu_app_index: Option<NemuAppIndex>,
}

/// #402 HS-3 (design HS-M §4.1 item 3): where a paired Nemu open reads its application's state.
enum ApplicationStateSource {
    /// A discovery-bound entry: the `MuMuManager.exe` it was discovered through and its index.
    Binding {
        mumu_manager_path: PathBuf,
        instance_index: u16,
    },
    /// An entry without a discovery binding: the capture's own resolution (its MuMu root's one
    /// `MuMuManager.exe`, the index it names or derives from the port).
    Capture,
    /// A handset: it has no `MuMuManager`, which is never read for it.
    Handset,
}

impl EntryEndpoint {
    /// #402 HS-3: where a paired Nemu open of this endpoint reads its application's state.
    fn application_state_source(&self) -> ApplicationStateSource {
        if self.handset().is_some() {
            return ApplicationStateSource::Handset;
        }
        match self.state.discovered_binding() {
            Some(discovered) => ApplicationStateSource::Binding {
                mumu_manager_path: discovered.mumu_manager_path().to_path_buf(),
                instance_index: discovered.instance_index(),
            },
            None => ApplicationStateSource::Capture,
        }
    }

    /// Workflow #398 RD-1b: the handset behind the endpoint; `None` for an emulator.
    fn handset(&self) -> Option<&HandsetEndpoint> {
        self.state.bound().and_then(ResolvedAdbEndpoint::handset)
    }

    /// The typed refusal of every session-opening path while the port is unknown, and of a
    /// handset the private binding file has no serial for (Workflow #398 RD-1b, model-398 §3.6):
    /// no adb command runs against it.
    fn require_bound(&self, operation: impl Into<Location>) -> DeviceResult<()> {
        let operation: Location = operation.into();
        if self.handset() == Some(&HandsetEndpoint::Unbound) {
            return Err(DeviceError::fatal(
                "device binding missing: the private binding file binds no device to this handset instance; bind one and restart",
            )
            .with_diagnostic(
                DeviceErrorCategory::Protocol,
                HostLocation::DeviceEndpointUnbound,
            )
            .with_diagnostic_context(
                "execution_backend_registry",
                operation,
                DeviceErrorSensitivity::Sensitive,
            ));
        }
        if self.state.is_pending() {
            return Err(DeviceError::fatal(
                "adb endpoint pending: the discovered instance was stopped and has reported no ADB port; start it through emulator control first",
            )
            .with_diagnostic(
                DeviceErrorCategory::Protocol,
                HostLocation::AdbEndpointPending,
            )
            .with_diagnostic_context(
                "execution_backend_registry",
                operation,
                DeviceErrorSensitivity::Sensitive,
            ));
        }
        Ok(())
    }

    /// CAP-IDX (since 0.12.27): a discovery-bound entry's capture takes its MuMu index from the
    /// binding, never from the ADB port, so a port MuMu shifted at launch still opens the bound
    /// index. Called where the endpoint is built and on every rebinding.
    fn bind_capture_index(&mut self) {
        if let Some(discovered) = self.state.discovered_binding() {
            self.capture.nemu.instance_id = Some(i32::from(discovered.instance_index()));
        }
    }

    /// CAP-IDX: what a Nemu IPC open of a discovery-bound entry checks its bound port against
    /// (`actingcommand_device::verify_mumu_target_port`); `None` for every other entry.
    fn nemu_port_check(&self) -> Option<NemuPortCheck> {
        if self.capture.requested != CaptureBackendChoice::NemuIpc {
            return None;
        }
        let discovered = self.state.discovered_binding()?;
        Some(NemuPortCheck {
            mumu_manager_path: discovered.mumu_manager_path().to_path_buf(),
            instance_index: discovered.instance_index(),
            bound_port: self.capture.target.port,
        })
    }

    /// Rewrites every port-dependent field for the given target.
    fn set_target(&mut self, host: &str, port: u16) {
        for target in [
            &mut self.application_target,
            &mut self.input.target,
            &mut self.capture.target,
        ] {
            target.host = host.to_owned();
            target.port = port;
        }
        self.audit_endpoint = self.application_target.label();
        self.configuration.resolved_serial = self.audit_endpoint.clone();
    }
}

/// CAP-IDX: a discovery-bound Nemu IPC entry's index, the `MuMuManager` it was discovered
/// through and the port it is bound to, copied out of the endpoint lock before the check reads.
struct NemuPortCheck {
    mumu_manager_path: PathBuf,
    instance_index: u16,
    bound_port: u16,
}

impl NemuPortCheck {
    /// No read on the index's base port; one bounded `info -v <index>` read otherwise, whose
    /// reported port must be the bound one (`target_identity_mismatch` otherwise, #419).
    fn verify(&self, reader: &InstanceStateReader) -> DeviceResult<()> {
        actingcommand_device::verify_mumu_target_port(self.instance_index, self.bound_port, || {
            reader(self.mumu_manager_path.as_path(), self.instance_index)
        })
    }
}

/// What a paired Nemu open reads and opens through: the CAP-IDX `info -v <index>` read, the #402
/// HS-3 `app info -pkg` read and the paired open itself (the registry's own, or its tests' fakes).
struct NemuOpenReads<'a> {
    instance_state: &'a InstanceStateReader,
    application_state: &'a ApplicationStateReader,
    open: &'a NemuSessionOpener,
}

/// The backend-open report of a capture open, before the backend is selected.
fn capture_open_report(
    capture: &CaptureBackendConfig,
) -> actingcommand_contract::BackendOpenReport {
    let mut report = actingcommand_contract::BackendOpenReport::unobserved(
        actingcommand_contract::BackendOpenEntry::Capture,
    );
    report.source = actingcommand_contract::BackendOpenSource::Native;
    report.requested = capture.requested.as_str().into();
    report.serial_configured = Some(capture.target.serial.is_some());
    report.installation_source = capture
        .resolved_mumu
        .as_ref()
        .map(|context| context.source.into());
    report
}

/// The backend-open report of a paired Nemu IPC session open.
fn nemu_pair_open_report() -> actingcommand_contract::BackendOpenReport {
    let mut report = actingcommand_contract::BackendOpenReport::unobserved(
        actingcommand_contract::BackendOpenEntry::NemuPair,
    );
    report.source = actingcommand_contract::BackendOpenSource::Native;
    report.requested = "nemu_ipc".into();
    report
}

impl ExecutionBackendEntry {
    fn new(registration: ExecutionBackendRegistration) -> RuntimeHostResult<Self> {
        let state = match (registration.discovered, registration.endpoint_pending) {
            (Some(discovered), true) => ResolvedInstanceEndpoint::Pending(PendingAdbEndpoint::new(
                registration.input.target.host.clone(),
                discovered,
            )),
            (discovered, _) => {
                let mut adb_endpoint = ResolvedAdbEndpoint::new(
                    registration.input.target.host.clone(),
                    registration.input.target.port,
                    registration.input.target.serial.is_some(),
                );
                if let Some(discovered) = discovered {
                    adb_endpoint = adb_endpoint.with_discovered_binding(discovered);
                }
                if let Some(handset) = registration.handset {
                    adb_endpoint = adb_endpoint.with_handset(handset);
                }
                ResolvedInstanceEndpoint::Bound(adb_endpoint)
            }
        };
        let application_adb = registration.input.adb_config.clone();
        let application_target = registration.input.target.clone();
        let capabilities = entry_capability_profile(
            registration.input.requested,
            registration.capture.requested,
            registration.provider_profile.as_ref(),
        )?;
        let mut endpoint = EntryEndpoint {
            state,
            // Workflow #398 RD-1a: the audit endpoint names the target by its label.
            audit_endpoint: application_target.label(),
            application_target,
            input: registration.input,
            capture: registration.capture,
            configuration: registration.configuration,
            capabilities,
            resolved_nemu_app_index: None,
        };
        if endpoint.state.is_pending() {
            let host = endpoint.application_target.host.clone();
            endpoint.set_target(&host, PENDING_ADB_PORT);
        }
        endpoint.bind_capture_index();
        Ok(Self {
            instance_id: registration.instance_id,
            application_id: registration.application_id,
            application_adb,
            provider_profile: registration.provider_profile,
            nemu_app_index: registration.nemu_app_index,
            endpoint: Mutex::new(endpoint),
        })
    }

    /// The guarded sections never panic, so a poisoned lock still holds a consistent state.
    fn endpoint(&self) -> MutexGuard<'_, EntryEndpoint> {
        self.endpoint.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// #402 HS-2 (design HS-M §3.2 step 3.3): the entry's backends become `touch` and
    /// `capture`, under the lock that guards them, all or nothing: the registration's own checks
    /// (`check_backend_selection`), the record of the configuration in effect and the capability
    /// table recomputed from the new backends and merged with the provider's rows as at
    /// registration. Every other setting of the two backends (adb, target, timeouts, tool paths,
    /// the Nemu folder and index) is the entry's as it was. Opens nothing. Returns the Nemu
    /// application index the configuration names, when `touch` is Nemu IPC (#402 HS-3: without
    /// one, the paired open resolves it, `resolved_nemu_app_index`).
    fn switch_backends(
        &self,
        touch: TouchBackendChoice,
        capture: CaptureBackendChoice,
    ) -> RuntimeHostResult<Option<u32>> {
        let mut endpoint = self.endpoint();
        let input = endpoint.input.clone().with_requested(touch);
        let candidate = endpoint.capture.clone().with_requested(capture);
        check_backend_selection(&input, &candidate, HostLocation::SwitchInstanceBackends)?;
        let capabilities =
            entry_capability_profile(touch, capture, self.provider_profile.as_ref())?;
        endpoint.configuration.input_backend = touch.as_str().to_owned();
        endpoint.configuration.capture_backend = capture.as_str().to_owned();
        endpoint.input = input;
        endpoint.capture = candidate;
        endpoint.capabilities = capabilities;
        Ok(self
            .nemu_app_index
            .filter(|_| touch == TouchBackendChoice::NemuIpc)
            .map(NemuAppIndex::index))
    }

    fn resolve(&self) -> ResolvedExecutionInstance {
        let (state, audit_endpoint, configuration, capabilities) = {
            let endpoint = self.endpoint();
            (
                endpoint.state.clone(),
                endpoint.audit_endpoint.clone(),
                endpoint.configuration.clone(),
                endpoint.capabilities.clone(),
            )
        };
        let resolved = ResolvedExecutionInstance::new(self.instance_id, audit_endpoint)
            .with_configuration(configuration)
            .with_capabilities(capabilities);
        match state {
            ResolvedInstanceEndpoint::Bound(adb_endpoint) => {
                resolved.with_adb_endpoint(adb_endpoint)
            }
            ResolvedInstanceEndpoint::Pending(pending) => resolved.with_pending_endpoint(pending),
        }
    }

    fn open_input(
        &self,
    ) -> DeviceResult<actingcommand_device::OpenedBackend<Box<dyn InputBackend>>> {
        let input = {
            let endpoint = self.endpoint();
            endpoint.require_bound(HostLocation::OpenInput)?;
            endpoint.input.clone()
        };
        let requested = input.requested;
        let serial_configured = input.target.serial.is_some();
        let mut report = actingcommand_contract::BackendOpenReport::unobserved(
            actingcommand_contract::BackendOpenEntry::Input,
        );
        report.source = actingcommand_contract::BackendOpenSource::Native;
        report.requested = requested.as_str().into();
        report.serial_configured = Some(serial_configured);
        create_touch_backend_for_fenced_input(input)
            .map(|backend| {
                let report = backend.open_report(serial_configured);
                actingcommand_device::OpenedBackend::new(
                    Box::new(DeviceRegistryInputDiagnosticBackend::new(
                        Box::new(backend),
                        requested,
                    )) as Box<dyn InputBackend>,
                    report,
                )
            })
            .map_err(|error| actingcommand_device::observe_open_failure(report, error))
    }

    /// The capture configuration an open uses: the bound endpoint's, with the binding's index,
    /// after the CAP-IDX port check (a refusal is observed as a failed capture open).
    fn capture_for_open(&self, reader: &InstanceStateReader) -> DeviceResult<CaptureBackendConfig> {
        let (capture, port_check) = {
            let endpoint = self.endpoint();
            endpoint.require_bound(HostLocation::OpenCapture)?;
            (endpoint.capture.clone(), endpoint.nemu_port_check())
        };
        if let Some(port_check) = port_check {
            port_check.verify(reader).map_err(|error| {
                actingcommand_device::observe_open_failure(capture_open_report(&capture), error)
            })?;
        }
        Ok(capture)
    }

    fn open_capture(
        &self,
        memory: Option<&actingcommand_device::FrameMemoryBudget>,
        reader: &InstanceStateReader,
    ) -> DeviceResult<actingcommand_device::OpenedBackend<Box<dyn CaptureBackend>>> {
        let capture = self.capture_for_open(reader)?;
        let report = capture_open_report(&capture);
        actingcommand_device::create_capture_backend_with_memory(capture, memory)
            .map(|selected| {
                let report = selected.open_report();
                actingcommand_device::OpenedBackend::new(
                    Box::new(selected) as Box<dyn CaptureBackend>,
                    report,
                )
            })
            .map_err(|error| actingcommand_device::observe_open_failure(report, error))
    }

    fn open_nemu_session(
        &self,
        memory: Option<&actingcommand_device::FrameMemoryBudget>,
        reads: &NemuOpenReads<'_>,
    ) -> DeviceResult<Option<actingcommand_device::OpenedBackend<NemuSessionBackends>>> {
        // Same pending guard as `open_input` / `open_capture`: a paired session is opened on
        // the bound target only, with the binding's index after the same CAP-IDX port check.
        let (input, capture, port_check, source, resolved) = {
            let endpoint = self.endpoint();
            if endpoint.input.requested != TouchBackendChoice::NemuIpc {
                return Ok(None);
            }
            endpoint.require_bound(HostLocation::OpenNemuSession)?;
            (
                endpoint.input.clone(),
                endpoint.capture.clone(),
                endpoint.nemu_port_check(),
                endpoint.application_state_source(),
                endpoint.resolved_nemu_app_index,
            )
        };
        if let Some(port_check) = port_check {
            port_check.verify(reads.instance_state).map_err(|error| {
                actingcommand_device::observe_open_failure(nemu_pair_open_report(), error)
            })?;
        }
        // #402 HS-3 (design HS-M §4.1): the file's index wins; without one the index resolved
        // before holds until a rebinding or a missing display, else `MuMuManager` is read once.
        let app_index = match (self.nemu_app_index, resolved) {
            (Some(index), _) | (None, Some(index)) => index,
            (None, None) => {
                let index = self
                    .resolve_nemu_app_index(source, &capture, reads.application_state)
                    .map_err(|error| {
                        actingcommand_device::observe_open_failure(nemu_pair_open_report(), error)
                    })?;
                self.endpoint().resolved_nemu_app_index = Some(index);
                index
            }
        };
        let application = NemuApplicationTarget::new(&self.application_id, app_index)?;
        (reads.open)(
            capture,
            application,
            NemuInputConfig {
                command_timeout: input.adb_config.command_timeout,
                shutdown_timeout: input.maatouch_config.shutdown_timeout,
                tap_hold: input.maatouch_config.tap_hold,
            },
            memory,
        )
        .map_err(|error| {
            // An open that found no display for the package drops the resolution: the next
            // paired open reads the application's state again.
            if error
                .capture_open_failure()
                .is_some_and(|failure| failure.cause == CaptureOpenCause::DisplayMissing)
            {
                self.endpoint().resolved_nemu_app_index = None;
            }
            actingcommand_device::observe_open_failure(nemu_pair_open_report(), error)
        })
        .map(|mut pair| {
            pair.backend.input = Box::new(DeviceRegistryInputDiagnosticBackend::new(
                pair.backend.input,
                TouchBackendChoice::NemuIpc,
            ));
            Some(pair)
        })
    }

    /// #402 HS-3 (design HS-M §4.1 item 2): the application index of a paired open whose
    /// configuration names none, from one bounded `control -v <index> app info -pkg <package>`
    /// read through `source`: `running` is the main application's index; `not_installed`,
    /// `stopped`, a read that gave no answer and one that answered another document refuse the
    /// open, the problem named on the error (`nemu_app_index_problem`). A handset is refused
    /// before anything is read.
    fn resolve_nemu_app_index(
        &self,
        source: ApplicationStateSource,
        capture: &CaptureBackendConfig,
        reader: &ApplicationStateReader,
    ) -> DeviceResult<NemuAppIndex> {
        let refused = |problem: NemuAppIndexProblem, message: String| {
            DeviceError::fatal(message)
                .with_diagnostic(DeviceErrorCategory::Protocol, HostLocation::OpenNemuSession)
                .with_nemu_app_index_problem(problem)
        };
        let (mumu_manager_path, instance_index) = match source {
            ApplicationStateSource::Binding {
                mumu_manager_path,
                instance_index,
            } => (mumu_manager_path, instance_index),
            ApplicationStateSource::Capture => {
                actingcommand_device::nemu_application_state_source(capture).map_err(|error| {
                    error.with_nemu_app_index_problem(NemuAppIndexProblem::InfoUnavailable)
                })?
            }
            ApplicationStateSource::Handset => {
                return Err(refused(
                    NemuAppIndexProblem::InfoUnavailable,
                    "Nemu application index unresolved: a handset has no MuMuManager, which is never read for it".to_owned(),
                ));
            }
        };
        match reader(
            mumu_manager_path.as_path(),
            instance_index,
            self.application_id.as_str(),
        ) {
            Ok(MumuApplicationState::Running) => NemuAppIndex::try_from(RESOLVED_NEMU_APP_INDEX),
            Ok(MumuApplicationState::Stopped) => Err(refused(
                NemuAppIndexProblem::ApplicationNotRunning,
                format!(
                    "Nemu application index unresolved: MuMuManager reports package {} installed and not running on MuMu instance {instance_index}",
                    self.application_id
                ),
            )),
            Ok(MumuApplicationState::NotInstalled) => Err(refused(
                NemuAppIndexProblem::PackageNotInstalled,
                format!(
                    "Nemu application index unresolved: MuMuManager reports package {} not installed on MuMu instance {instance_index}",
                    self.application_id
                ),
            )),
            Err(error) => {
                let problem = actingcommand_device::application_state_read_problem(&error);
                Err(error.with_nemu_app_index_problem(problem))
            }
        }
    }

    /// #402 HS-3: the index the entry's last paired open resolved, while its touch backend is
    /// Nemu IPC and the resolution holds (`resolved_nemu_app_index`).
    fn resolved_nemu_app_index(&self) -> Option<u32> {
        let endpoint = self.endpoint();
        if endpoint.input.requested != TouchBackendChoice::NemuIpc {
            return None;
        }
        endpoint.resolved_nemu_app_index.map(NemuAppIndex::index)
    }

    fn control_application(
        &self,
        witness: &FencedWrite,
        action: ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        let application_target = {
            let endpoint = self.endpoint();
            endpoint.require_bound(HostLocation::ControlApplication)?;
            endpoint.application_target.clone()
        };
        let serial = application_target.adb_arg();
        let adb = Adb::new(self.application_adb.clone());
        adb.ensure_device(&serial, application_target.connect)?;
        match action {
            ApplicationLifecycleAction::Launch => {
                adb.launch_package(witness, &serial, &self.application_id)?;
            }
            ApplicationLifecycleAction::Stop => {
                adb.force_stop(witness, &serial, &self.application_id)?;
            }
            ApplicationLifecycleAction::Restart => {
                adb.force_stop(witness, &serial, &self.application_id)?;
                thread::sleep(Duration::from_millis(500));
                adb.launch_package(witness, &serial, &self.application_id)?;
            }
        }
        Ok(())
    }

    /// The ADB baseline probe: the bound endpoint's `ensure_device` with a connect attempt
    /// allowed (a freshly started emulator answers `adb connect` a few seconds after the
    /// vendor reports it running). A handset on USB has no `adb connect` (Workflow #398 RD-1b,
    /// model-398 §3.1): its probe never attempts one. No session is opened.
    fn probe_adb_baseline(&self) -> DeviceResult<()> {
        let (application_target, connect_allowed) = {
            let endpoint = self.endpoint();
            endpoint.require_bound(HostLocation::ProbeAdbBaseline)?;
            (
                endpoint.application_target.clone(),
                endpoint.handset().is_none(),
            )
        };
        Adb::new(self.application_adb.clone())
            .ensure_device(&application_target.adb_arg(), connect_allowed)
            .map(|_| ())
    }

    fn probe_adb_baseline_until(
        &self,
        deadline: std::time::Instant,
        stopped: &dyn Fn() -> bool,
    ) -> DeviceResult<()> {
        let (application_target, connect_allowed) = {
            let endpoint = self.endpoint();
            endpoint.require_bound(HostLocation::ProbeAdbBaseline)?;
            (
                endpoint.application_target.clone(),
                endpoint.handset().is_none(),
            )
        };
        Adb::new(self.application_adb.clone())
            .ensure_device_until(
                &application_target.adb_arg(),
                connect_allowed,
                deadline,
                stopped,
            )
            .map(|_| ())
    }

    /// The ADB baseline query behind the foreground gate: same bound-endpoint guard as
    /// `control_application`, then one read-only `dumpsys` query; only a failed query
    /// re-checks the transport (Workflow #191 E1). No session is opened; a Nemu paired
    /// session, when one is open, is not consulted.
    fn observe_foreground_application(&self) -> DeviceResult<ForegroundApplicationObservation> {
        let application_target = {
            let endpoint = self.endpoint();
            endpoint.require_bound(ContractLocation::ObserveForegroundApplication)?;
            endpoint.application_target.clone()
        };
        let serial = application_target.adb_arg();
        let adb = Adb::new(self.application_adb.clone());
        Ok(ForegroundApplicationObservation {
            foreground: adb.foreground_package(&serial, application_target.connect)?,
            assigned: self.application_id.clone(),
        })
    }

    /// Workflow #398 RD-2 (model-398 v1.1 §4.1): the serial of a bound handset and the adb it
    /// is reached through; `None` for an emulator and for a handset the private binding file
    /// binds no device to.
    fn bound_handset(&self) -> Option<(DeviceSerial, String)> {
        let endpoint = self.endpoint();
        match endpoint.handset() {
            Some(HandsetEndpoint::Bound(binding)) => Some((
                binding.serial().clone(),
                self.application_adb.adb_path.clone(),
            )),
            Some(HandsetEndpoint::Unbound) | None => None,
        }
    }

    /// Workflow #398 RD-2 (model-398 v1.1 §4.3): one `adb -s <serial> reconnect` of a bound
    /// handset's transport. Any other instance is refused before adb runs.
    fn reconnect_handset(&self) -> DeviceResult<()> {
        let Some((serial, _)) = self.bound_handset() else {
            return Err(DeviceError::fatal(
                "only a bound handset's transport is reconnected",
            ));
        };
        Adb::new(self.application_adb.clone()).reconnect(serial.adb_arg())
    }

    /// Workflow #398 RD-5b (model-398 v1.1 §7.1-§7.3): `work` with the device-state shell of a
    /// bound handset (`adb -s <serial> shell ...` through the entry's adb, RD-5a's closed sets
    /// only) and the entry's application id; `false` for an emulator and for a handset the
    /// private binding file binds no device to, which get no command.
    fn with_handset_state(
        &self,
        work: &mut dyn FnMut(&mut dyn DeviceStateShell, &str),
    ) -> DeviceResult<bool> {
        let target = {
            let endpoint = self.endpoint();
            match endpoint.handset() {
                Some(HandsetEndpoint::Bound(_)) => endpoint.application_target.clone(),
                Some(HandsetEndpoint::Unbound) | None => return Ok(false),
            }
        };
        let adb = Adb::new(self.application_adb.clone());
        let mut shell = AdbDeviceStateShell::new(&adb, &target);
        work(&mut shell, &self.application_id);
        Ok(true)
    }
}

/// Workflow #398 RD-2 (model-398 v1.1 §4.1): the registry's device-link watch, one
/// `adb track-devices` process read as the link state of each bound handset by its alias. The
/// serials stay here; the watch's events name aliases only.
struct RegistryLinkWatch {
    process: AdbTrackDevices,
    handsets: Vec<(String, DeviceSerial)>,
}

impl DeviceLinkWatch for RegistryLinkWatch {
    fn next_event(&mut self) -> DeviceLinkWatchEvent {
        match self.process.next_event() {
            TrackDevicesEvent::List(list) => DeviceLinkWatchEvent::Links(
                self.handsets
                    .iter()
                    .map(|(alias, serial)| (alias.clone(), list.state_of(serial)))
                    .collect(),
            ),
            TrackDevicesEvent::Ended => DeviceLinkWatchEvent::Ended,
            TrackDevicesEvent::ProtocolInvalid => DeviceLinkWatchEvent::ProtocolInvalid,
        }
    }

    fn stopper(&self) -> Arc<dyn DeviceLinkWatchStop> {
        Arc::new(RegistryLinkWatchStop(self.process.stopper()))
    }
}

/// Ends the registry's `adb track-devices` process.
struct RegistryLinkWatchStop(Arc<TrackDevicesStopper>);

impl DeviceLinkWatchStop for RegistryLinkWatchStop {
    fn stop(&self) -> DeviceResult<()> {
        self.0.stop()
    }
}

/// The only [`ExecutionBackendProvider`]: every instance the daemon configured, real or
/// fixture, behind one alias set, one vision provider and one discovery surface.
pub struct ExecutionBackendRegistry {
    entries: BTreeMap<String, RegistryEntry>,
    vision_provider: Option<Arc<dyn RecognitionVisionProvider>>,
    discovery: Option<DiscoverySpec>,
    /// The bounded `info -v <index>` read: `read_instance_state` and the CAP-IDX port check.
    instance_state_reader: InstanceStateReader,
    /// #402 HS-3: the bounded `app info -pkg` read of a paired open without a configured index.
    application_state_reader: ApplicationStateReader,
    /// #402 HS-3: the paired Nemu open.
    nemu_session_opener: NemuSessionOpener,
}

/// The production `info -v <index>` read.
fn mumu_instance_state_reader() -> InstanceStateReader {
    Arc::new(actingcommand_device::read_instance_state)
}

/// #402 HS-3: the production `control -v <index> app info -pkg <package>` read.
fn mumu_application_state_reader() -> ApplicationStateReader {
    Arc::new(actingcommand_device::read_application_state)
}

/// #402 HS-3: the production paired Nemu open.
fn nemu_session_opener() -> NemuSessionOpener {
    Arc::new(NemuIpcSession::open)
}

impl ExecutionBackendRegistry {
    /// A registry of real instances only, without vision or discovery; it must not be empty.
    pub fn new(
        registrations: impl IntoIterator<Item = ExecutionBackendRegistration>,
    ) -> RuntimeHostResult<Self> {
        let mut registry = Self {
            entries: BTreeMap::new(),
            vision_provider: None,
            discovery: None,
            instance_state_reader: mumu_instance_state_reader(),
            application_state_reader: mumu_application_state_reader(),
            nemu_session_opener: nemu_session_opener(),
        };
        for registration in registrations {
            registry.register(registration)?;
        }
        if registry.entries.is_empty() {
            return Err(RuntimeHostError::fatal(
                HostCode::EmptyExecutionBackendRegistry,
                HostLocation::BuildExecutionBackendRegistry,
                RuntimeErrorCode::RuntimeHalted,
            ));
        }
        Ok(registry)
    }

    /// The single owner of backend assembly: registers every instance once under one
    /// alias / instance-id rule (`duplicate_instance_alias` / `duplicate_instance_id`),
    /// routes real and fixture instances, injects the vision provider once and keeps the
    /// discovery surface. Opens are wrapped once, with `observe_open_failure`.
    pub fn from_assembly(assembly: ProviderAssembly) -> RuntimeHostResult<Self> {
        let ProviderAssembly {
            instances,
            vision,
            discovery,
        } = assembly;
        let mut registry = Self {
            entries: BTreeMap::new(),
            vision_provider: vision.map(|spec| spec.provider),
            discovery,
            instance_state_reader: mumu_instance_state_reader(),
            application_state_reader: mumu_application_state_reader(),
            nemu_session_opener: nemu_session_opener(),
        };
        for spec in instances {
            registry.register_spec(spec)?;
        }
        Ok(registry)
    }

    /// Adds one real registration under the same duplicate-alias and duplicate-id rules as
    /// `from_assembly`.
    pub fn register(
        &mut self,
        registration: ExecutionBackendRegistration,
    ) -> RuntimeHostResult<()> {
        self.register_spec(InstanceSpec::real(registration))
    }

    fn register_spec(&mut self, spec: InstanceSpec) -> RuntimeHostResult<()> {
        // #402 HS-3: Nemu touch without `nemu_app_index` registers; its paired open resolves the
        // index (`ExecutionBackendEntry::resolve_nemu_app_index`).
        let InstanceSpec {
            alias,
            instance_id,
            mode,
        } = spec;
        if self.entries.contains_key(&alias) {
            return Err(RuntimeHostError::fatal(
                HostCode::DuplicateInstanceAlias,
                HostLocation::BuildExecutionBackendRegistry,
                RuntimeErrorCode::RuntimeHalted,
            ));
        }
        if self
            .entries
            .values()
            .any(|entry| entry.instance_id() == instance_id)
        {
            return Err(RuntimeHostError::fatal(
                HostCode::DuplicateInstanceId,
                HostLocation::BuildExecutionBackendRegistry,
                RuntimeErrorCode::RuntimeHalted,
            ));
        }
        let entry = match mode {
            InstanceMode::Real(registration) => {
                RegistryEntry::Real(Box::new(ExecutionBackendEntry::new(*registration)?))
            }
            #[cfg(feature = "fixture-backends")]
            InstanceMode::Fixture(fixture) => {
                RegistryEntry::Fixture(fixtures::FixtureEntry::new(instance_id, fixture))
            }
        };
        self.entries.insert(alias, entry);
        Ok(())
    }

    pub fn with_vision_provider(
        mut self,
        vision_provider: Arc<dyn RecognitionVisionProvider>,
    ) -> Self {
        self.vision_provider = Some(vision_provider);
        self
    }

    /// CAP-IDX tests: replaces the `info -v <index>` read with a fake.
    #[cfg(test)]
    pub(crate) fn with_instance_state_reader(
        mut self,
        reader: impl Fn(&Path, u16) -> DeviceResult<InstanceState> + Send + Sync + 'static,
    ) -> Self {
        self.instance_state_reader = Arc::new(reader);
        self
    }

    /// #402 HS-3 tests: replaces the `control -v <index> app info -pkg <package>` read with a
    /// fake.
    #[cfg(test)]
    pub(crate) fn with_application_state_reader(
        mut self,
        reader: impl Fn(&Path, u16, &str) -> DeviceResult<MumuApplicationState> + Send + Sync + 'static,
    ) -> Self {
        self.application_state_reader = Arc::new(reader);
        self
    }

    /// #402 HS-3 tests: replaces the paired Nemu open with a fake, which opens no device.
    #[cfg(test)]
    pub(crate) fn with_nemu_session_opener(
        mut self,
        opener: impl Fn(
            CaptureBackendConfig,
            NemuApplicationTarget,
            NemuInputConfig,
            Option<&actingcommand_device::FrameMemoryBudget>,
        )
            -> DeviceResult<actingcommand_device::OpenedBackend<NemuSessionBackends>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        self.nemu_session_opener = Arc::new(opener);
        self
    }

    /// CAP-IDX tests: the capture configuration `open_capture` would open a real entry with,
    /// after its port check; no backend is created.
    #[cfg(test)]
    pub(crate) fn capture_for_open(
        &self,
        instance_alias: &str,
    ) -> DeviceResult<CaptureBackendConfig> {
        match self.entry(instance_alias)? {
            RegistryEntry::Real(entry) => entry.capture_for_open(&self.instance_state_reader),
            #[cfg(feature = "fixture-backends")]
            RegistryEntry::Fixture(_) => Err(DeviceError::fatal(
                "a fixture simulation instance has no capture configuration",
            )),
        }
    }

    fn entry(&self, instance_alias: &str) -> DeviceResult<&RegistryEntry> {
        self.entries
            .get(instance_alias)
            .ok_or_else(|| DeviceError::fatal("execution backend instance is not registered"))
    }
}

/// The capability table of an entry with these backends: the registry's own rows of `input`
/// and `capture`, merged with the provider's rows when the entry has an admitted profile. #402
/// HS-2: computed at registration and again at every backend switch.
fn entry_capability_profile(
    input: TouchBackendChoice,
    capture: CaptureBackendChoice,
    provider: Option<&EmulatorCapabilityProfile>,
) -> RuntimeHostResult<EmulatorCapabilityProfile> {
    let capabilities = capability_profile(input, capture)?;
    match provider {
        Some(provider) => merge_capability_profile(&capabilities, provider),
        None => Ok(capabilities),
    }
}

/// Records implementation/configuration only; this path never opens or probes a backend.
///
/// HS-4 (#402, design §4.4): the Nemu input performs the tap, the long tap, the swipe, the
/// segmented drags and the reset (`NemuInputSupport`), and neither `key` nor `text`.
fn capability_profile(
    input: TouchBackendChoice,
    capture: CaptureBackendChoice,
) -> RuntimeHostResult<EmulatorCapabilityProfile> {
    let invalid = |_| {
        RuntimeHostError::fatal(
            HostCode::ExecutionCapabilityProfileInvalid,
            HostLocation::BuildExecutionBackendRegistry,
            RuntimeErrorCode::RuntimeHalted,
        )
    };
    let evidence = EmulatorCapability::ALL.into_iter().map(|capability| {
        let (supported, source) = match capability {
            EmulatorCapability::InputTap | EmulatorCapability::InputLongTap
            | EmulatorCapability::InputSwipe | EmulatorCapability::InputReset =>
                (true, input.as_str()),
            EmulatorCapability::InputSegmentedSwipe => (
                matches!(
                    input,
                    TouchBackendChoice::MaaTouch | TouchBackendChoice::Minitouch
                        | TouchBackendChoice::NemuIpc
                ),
                input.as_str(),
            ),
            EmulatorCapability::InputKey | EmulatorCapability::InputText => (
                input == TouchBackendChoice::MaaTouch, input.as_str(),
            ),
            EmulatorCapability::CaptureFrame => (true, capture.as_str()),
            EmulatorCapability::ApplicationLaunch | EmulatorCapability::ApplicationStop
            | EmulatorCapability::ApplicationRestart => (true, "adb_application_lifecycle"),
            EmulatorCapability::InventoryRead | EmulatorCapability::InstanceStatusRead
            | EmulatorCapability::InstanceStart | EmulatorCapability::InstanceStop
            | EmulatorCapability::InstanceRestart | EmulatorCapability::InstanceCreate
            | EmulatorCapability::InstanceClone | EmulatorCapability::InstanceDelete
            | EmulatorCapability::InstanceConfigure | EmulatorCapability::ApplicationControl
            | EmulatorCapability::AdbBridge | EmulatorCapability::SnapshotManage =>
                (false, "execution_backend_registry"),
        };
        let (implementation, availability, refusal) = if supported {
            (EmulatorCapabilityImplementation::Supported, EmulatorCapabilityAvailability::Unverified,
             "Implementation selected; version, assets, connection and device availability have not been verified. Execution requires the existing Runtime admission and backend checks.")
        } else {
            (EmulatorCapabilityImplementation::Unsupported, EmulatorCapabilityAvailability::Unavailable,
             "The registered backend does not implement this capability; it must not be executed through this capability claim.")
        };
        EmulatorCapabilityEvidence::new(capability, availability, refusal, source)?
            .with_implementation(implementation)
    }).collect::<Result<Vec<_>, _>>().map_err(invalid)?;
    EmulatorCapabilityProfile::new(
        "runtime.execution_backend_registry",
        EmulatorVersionEvidence::Unavailable {
            reason: EmulatorVersionUnavailableReason::NotProbed,
        },
        evidence,
    )
    .map_err(invalid)
}

/// Provider id, version and the provider-owned rows (inventory, instance status and
/// lifecycle, instance management, snapshots, ADB bridge) come from the admitted provider
/// profile; input, capture, application lifecycle and application control keep the
/// registry-derived evidence. The result is re-validated as a complete profile.
fn merge_capability_profile(
    registry: &EmulatorCapabilityProfile,
    provider: &EmulatorCapabilityProfile,
) -> RuntimeHostResult<EmulatorCapabilityProfile> {
    let evidence = EmulatorCapability::ALL
        .into_iter()
        .map(|capability| {
            let source = match capability {
                EmulatorCapability::InventoryRead
                | EmulatorCapability::InstanceStatusRead
                | EmulatorCapability::InstanceStart
                | EmulatorCapability::InstanceStop
                | EmulatorCapability::InstanceRestart
                | EmulatorCapability::InstanceCreate
                | EmulatorCapability::InstanceClone
                | EmulatorCapability::InstanceDelete
                | EmulatorCapability::InstanceConfigure
                | EmulatorCapability::SnapshotManage
                | EmulatorCapability::AdbBridge => provider,
                EmulatorCapability::ApplicationControl
                | EmulatorCapability::InputTap
                | EmulatorCapability::InputLongTap
                | EmulatorCapability::InputSwipe
                | EmulatorCapability::InputSegmentedSwipe
                | EmulatorCapability::InputKey
                | EmulatorCapability::InputText
                | EmulatorCapability::InputReset
                | EmulatorCapability::CaptureFrame
                | EmulatorCapability::ApplicationLaunch
                | EmulatorCapability::ApplicationStop
                | EmulatorCapability::ApplicationRestart => registry,
            };
            source.evidence(capability).clone()
        })
        .collect();
    EmulatorCapabilityProfile::new(provider.provider_id(), provider.version().clone(), evidence)
        .map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::ExecutionCapabilityProfileMergeInvalid,
                HostLocation::BuildExecutionBackendRegistry,
                RuntimeErrorCode::RuntimeHalted,
            )
        })
}

impl fmt::Debug for ExecutionBackendRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExecutionBackendRegistry")
            .field("instance_count", &self.entries.len())
            .field("vision_provider", &self.vision_provider.is_some())
            .field("discovery", &self.discovery.is_some())
            .finish()
    }
}

impl ExecutionBackendProvider for ExecutionBackendRegistry {
    fn instance_aliases(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        Some(match self.entries.get(instance_alias)? {
            RegistryEntry::Real(entry) => entry.resolve(),
            #[cfg(feature = "fixture-backends")]
            RegistryEntry::Fixture(fixture) => fixture.resolve(),
        })
    }

    fn open_input(
        &self,
        instance_alias: &str,
    ) -> DeviceResult<actingcommand_device::OpenedBackend<Box<dyn InputBackend>>> {
        match self.entry(instance_alias)? {
            RegistryEntry::Real(entry) => entry.open_input(),
            #[cfg(feature = "fixture-backends")]
            RegistryEntry::Fixture(fixture) => Ok(fixture.open_input()),
        }
    }

    fn open_capture(
        &self,
        instance_alias: &str,
        _memory: Option<&actingcommand_device::FrameMemoryBudget>,
    ) -> DeviceResult<actingcommand_device::OpenedBackend<Box<dyn CaptureBackend>>> {
        match self.entry(instance_alias)? {
            RegistryEntry::Real(entry) => entry.open_capture(_memory, &self.instance_state_reader),
            #[cfg(feature = "fixture-backends")]
            RegistryEntry::Fixture(fixture) => fixture.open_capture(),
        }
    }

    fn open_nemu_session(
        &self,
        instance_alias: &str,
        memory: Option<&actingcommand_device::FrameMemoryBudget>,
    ) -> DeviceResult<Option<actingcommand_device::OpenedBackend<NemuSessionBackends>>> {
        match self.entry(instance_alias)? {
            RegistryEntry::Real(entry) => entry.open_nemu_session(
                memory,
                &NemuOpenReads {
                    instance_state: &self.instance_state_reader,
                    application_state: &self.application_state_reader,
                    open: &self.nemu_session_opener,
                },
            ),
            #[cfg(feature = "fixture-backends")]
            RegistryEntry::Fixture(_) => Ok(None),
        }
    }

    /// #402 HS-3: the index a real entry's last paired open resolved (a fixture has no Nemu).
    fn resolved_nemu_app_index(&self, instance_alias: &str) -> Option<u32> {
        match self.entries.get(instance_alias)? {
            RegistryEntry::Real(entry) => entry.resolved_nemu_app_index(),
            #[cfg(feature = "fixture-backends")]
            RegistryEntry::Fixture(_) => None,
        }
    }

    fn control_application(
        &self,
        witness: &FencedWrite,
        instance_alias: &str,
        action: ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        match self.entry(instance_alias)? {
            RegistryEntry::Real(entry) => entry.control_application(witness, action),
            #[cfg(feature = "fixture-backends")]
            RegistryEntry::Fixture(_) => fixtures::FixtureEntry::control_application(),
        }
    }

    /// Workflow #398 RD-2 (model-398 v1.1 §4.1): one `adb track-devices` process for every
    /// bound handset, through the first one's adb (the installation's); none when the registry
    /// has no bound handset, so an emulator-only configuration runs no watch process.
    fn open_device_link_watch(&self) -> DeviceResult<Option<Box<dyn DeviceLinkWatch>>> {
        let mut adb_path = None;
        let mut handsets = Vec::new();
        for (alias, entry) in &self.entries {
            if let Some((serial, path)) = entry.bound_handset() {
                adb_path.get_or_insert(path);
                handsets.push((alias.clone(), serial));
            }
        }
        let Some(adb_path) = adb_path else {
            return Ok(None);
        };
        let process = AdbTrackDevices::spawn(&adb_path)?;
        Ok(Some(Box::new(RegistryLinkWatch { process, handsets })))
    }

    fn reconnect_handset(&self, instance_alias: &str) -> DeviceResult<()> {
        match self.entry(instance_alias)? {
            RegistryEntry::Real(entry) => entry.reconnect_handset(),
            #[cfg(feature = "fixture-backends")]
            RegistryEntry::Fixture(_) => Err(DeviceError::fatal(
                "a fixture instance has no handset transport to reconnect",
            )),
        }
    }

    fn with_handset_state(
        &self,
        instance_alias: &str,
        work: &mut dyn FnMut(&mut dyn DeviceStateShell, &str),
    ) -> DeviceResult<bool> {
        match self.entry(instance_alias)? {
            RegistryEntry::Real(entry) => entry.with_handset_state(work),
            // A fixture instance is no handset: it gets no device-state command.
            #[cfg(feature = "fixture-backends")]
            RegistryEntry::Fixture(_) => Ok(false),
        }
    }

    fn probe_adb_baseline(&self, instance_alias: &str) -> DeviceResult<()> {
        match self.entry(instance_alias)? {
            RegistryEntry::Real(entry) => entry.probe_adb_baseline(),
            // A fixture has no ADB baseline to wait for.
            #[cfg(feature = "fixture-backends")]
            RegistryEntry::Fixture(_) => Ok(()),
        }
    }

    fn probe_adb_baseline_until(
        &self,
        instance_alias: &str,
        deadline: std::time::Instant,
        stopped: &dyn Fn() -> bool,
    ) -> DeviceResult<()> {
        match self.entry(instance_alias)? {
            RegistryEntry::Real(entry) => entry.probe_adb_baseline_until(deadline, stopped),
            #[cfg(feature = "fixture-backends")]
            RegistryEntry::Fixture(_) => {
                fixtures::FixtureEntry::probe_adb_baseline_until(deadline, stopped)
            }
        }
    }

    fn observe_foreground_application(
        &self,
        instance_alias: &str,
    ) -> DeviceResult<ForegroundApplicationObservation> {
        match self.entry(instance_alias)? {
            RegistryEntry::Real(entry) => entry.observe_foreground_application(),
            #[cfg(feature = "fixture-backends")]
            RegistryEntry::Fixture(_) => fixtures::FixtureEntry::observe_foreground_application(),
        }
    }

    /// Drives `MuMuManager control` for a discovery-bound entry only. Explicit (non-discovered)
    /// entries carry no `MuMuManager` path and are refused with `emulator_control_binding`.
    /// No device session is opened or touched here.
    fn control_instance(
        &self,
        instance_alias: &str,
        action: EmulatorInstanceAction,
    ) -> EmulatorControlResult<EmulatorControlOutcome> {
        let refused = |message: &str, stage: Location| {
            EmulatorControlFailure::without_output(
                DeviceError::fatal(message)
                    .with_diagnostic(DeviceErrorCategory::Protocol, stage)
                    .with_diagnostic_context(
                        "execution_backend_registry",
                        ContractLocation::ControlInstance,
                        DeviceErrorSensitivity::Sensitive,
                    ),
                0,
            )
        };
        let entry = match self.entries.get(instance_alias) {
            Some(RegistryEntry::Real(entry)) => entry,
            #[cfg(feature = "fixture-backends")]
            Some(RegistryEntry::Fixture(_)) => {
                return Err(refused(
                    "emulator control unavailable: a fixture simulation instance has no emulator",
                    HostLocation::EmulatorControlBinding.location(),
                ));
            }
            None => {
                return Err(refused(
                    "execution backend instance is not registered",
                    HostLocation::EmulatorControlRegistration.location(),
                ));
            }
        };
        let Some(discovered) = entry.endpoint().state.discovered_binding().cloned() else {
            return Err(refused(
                "emulator control unavailable: the instance was registered explicitly, without MuMuManager discovery, so no MuMuManager executable is bound to it",
                HostLocation::EmulatorControlBinding.location(),
            ));
        };
        actingcommand_device::control_instance(
            discovered.mumu_manager_path(),
            discovered.instance_index(),
            action,
            mumu_state_wait(action),
        )
    }

    /// Workflow #381 B (review M3): one `info -v <index>` read (bounded by
    /// `MUMU_MANAGER_COMMAND_TIMEOUT`) of a discovery-bound entry, with the same refusals as
    /// `control_instance`. No device session is opened or touched here.
    fn read_instance_state(&self, instance_alias: &str) -> DeviceResult<InstanceState> {
        let refused = |message: &str, stage: Location| {
            DeviceError::fatal(message)
                .with_diagnostic(DeviceErrorCategory::Protocol, stage)
                .with_diagnostic_context(
                    "execution_backend_registry",
                    ContractLocation::ReadInstanceState,
                    DeviceErrorSensitivity::Sensitive,
                )
        };
        let entry = match self.entries.get(instance_alias) {
            Some(RegistryEntry::Real(entry)) => entry,
            #[cfg(feature = "fixture-backends")]
            Some(RegistryEntry::Fixture(_)) => {
                return Err(refused(
                    "emulator state unavailable: a fixture simulation instance has no emulator",
                    HostLocation::EmulatorControlBinding.location(),
                ));
            }
            None => {
                return Err(refused(
                    "execution backend instance is not registered",
                    HostLocation::EmulatorControlRegistration.location(),
                ));
            }
        };
        let Some(discovered) = entry.endpoint().state.discovered_binding().cloned() else {
            return Err(refused(
                "emulator state unavailable: the instance was registered explicitly, without MuMuManager discovery",
                HostLocation::EmulatorControlBinding.location(),
            ));
        };
        (self.instance_state_reader)(discovered.mumu_manager_path(), discovered.instance_index())
    }

    /// Binds a discovery-bound entry to the port the started instance reported (its host is
    /// the one the entry was registered with) or returns it to pending after a stop.
    fn rebind_discovered_endpoint(
        &self,
        instance_alias: &str,
        adb_port: Option<u16>,
    ) -> DeviceResult<()> {
        let refused = |message: &str, stage: Location| {
            DeviceError::fatal(message)
                .with_diagnostic(DeviceErrorCategory::Protocol, stage)
                .with_diagnostic_context(
                    "execution_backend_registry",
                    ContractLocation::RebindDiscoveredEndpoint,
                    DeviceErrorSensitivity::Sensitive,
                )
        };
        let entry = match self.entries.get(instance_alias) {
            Some(RegistryEntry::Real(entry)) => entry,
            #[cfg(feature = "fixture-backends")]
            Some(RegistryEntry::Fixture(_)) => {
                return Err(refused(
                    "endpoint rebinding unavailable: a fixture simulation instance has no emulator",
                    HostLocation::EmulatorControlBinding.location(),
                ));
            }
            None => {
                return Err(refused(
                    "execution backend instance is not registered",
                    HostLocation::EmulatorControlRegistration.location(),
                ));
            }
        };
        let mut endpoint = entry.endpoint();
        let Some(discovered) = endpoint.state.discovered_binding().cloned() else {
            return Err(refused(
                "endpoint rebinding unavailable: the instance was registered explicitly, without MuMuManager discovery",
                HostLocation::EmulatorControlBinding.location(),
            ));
        };
        let host = endpoint.application_target.host.clone();
        match adb_port {
            Some(PENDING_ADB_PORT) => {
                return Err(refused(
                    "endpoint rebinding refused: the started instance reported ADB port 0",
                    HostLocation::AdbEndpointUnresolved.location(),
                ));
            }
            Some(port) => {
                endpoint.set_target(&host, port);
                endpoint.state = ResolvedInstanceEndpoint::Bound(
                    ResolvedAdbEndpoint::new(host, port, false).with_discovered_binding(discovered),
                );
            }
            None => {
                endpoint.set_target(&host, PENDING_ADB_PORT);
                endpoint.state =
                    ResolvedInstanceEndpoint::Pending(PendingAdbEndpoint::new(host, discovered));
            }
        }
        endpoint.bind_capture_index();
        // #402 HS-3 (design HS-M §4.1): a rebinding drops the resolved Nemu application index;
        // the next paired open (the start's preparation) reads it again.
        endpoint.resolved_nemu_app_index = None;
        Ok(())
    }

    /// #402 HS-2 (design HS-M §3.2 step 3.3): a real entry switches its backends under its own
    /// lock (`ExecutionBackendEntry::switch_backends`); a fixture has no device backend to
    /// switch. A refusal of the registration's checks names its code in the device error.
    fn switch_instance_backends(
        &self,
        instance_alias: &str,
        touch: TouchBackendChoice,
        capture: CaptureBackendChoice,
    ) -> DeviceResult<Option<u32>> {
        match self.entries.get(instance_alias) {
            Some(RegistryEntry::Real(entry)) => {
                entry.switch_backends(touch, capture).map_err(|error| {
                    DeviceError::fatal(format!(
                        "backend switch refused by the registration's checks: {}",
                        error.code()
                    ))
                    .with_diagnostic(
                        DeviceErrorCategory::Protocol,
                        HostLocation::SwitchInstanceBackends,
                    )
                })
            }
            #[cfg(feature = "fixture-backends")]
            Some(RegistryEntry::Fixture(_)) => Err(DeviceError::fatal(
                "backend switch unavailable: a fixture simulation instance has no device backends",
            )),
            None => Err(DeviceError::fatal(
                "execution backend instance is not registered",
            )),
        }
    }

    /// On-demand discovery: the same `MuMuManager` resolution as startup, mapped to the
    /// provider view. Binds, rebinds, registers and records nothing.
    fn discover_instances(
        &self,
    ) -> Result<ProviderInstanceDiscovery, Box<InstanceDiscoveryFailure>> {
        let Some(discovery) = &self.discovery else {
            return Err(Box::new(InstanceDiscoveryFailure {
                code: SharedCode::InstanceDiscoveryUnavailable.as_str(),
                error: DeviceError::fatal("instance discovery unsupported by this provider")
                    .with_diagnostic(
                        DeviceErrorCategory::Protocol,
                        ContractLocation::InstanceDiscoveryUnsupported,
                    )
                    .with_diagnostic_context(
                        "execution_backend_registry",
                        ContractLocation::DiscoverInstances,
                        DeviceErrorSensitivity::Sensitive,
                    ),
            }));
        };
        let report = discovery.discover()?;
        Ok(ProviderInstanceDiscovery {
            provider_version: report.version.to_string(),
            instances: report
                .instances
                .into_iter()
                .map(|instance| ProviderDiscoveredInstance {
                    instance_index: instance.instance_index,
                    instance_name: instance.instance_name,
                    adb_host: instance.adb_host,
                    adb_port: instance.adb_port,
                    running: instance.running,
                    android_version: instance.android_version,
                })
                .collect(),
        })
    }

    fn vision_provider(&self) -> Option<Arc<dyn RecognitionVisionProvider>> {
        self.vision_provider.as_ref().map(Arc::clone)
    }
}

/// Completes the diagnostic of every input operation failure a real backend reports without
/// one (Workflow #257 DEVICE-DIAGNOSTIC-v1): category, stage, backend, operation and the
/// registry summary are added only where the producer left them absent.
struct DeviceRegistryInputDiagnosticBackend {
    backend: Box<dyn InputBackend>,
    requested_backend: TouchBackendChoice,
}

impl DeviceRegistryInputDiagnosticBackend {
    fn new(backend: Box<dyn InputBackend>, requested_backend: TouchBackendChoice) -> Self {
        Self {
            backend,
            requested_backend,
        }
    }

    fn run<T>(
        &mut self,
        operation: DeviceLocation,
        execute: impl FnOnce(&mut dyn InputBackend) -> DeviceResult<T>,
    ) -> DeviceResult<T> {
        match execute(self.backend.as_mut()) {
            Ok(value) => Ok(value),
            Err(error) => {
                let producer_complete =
                    error.diagnostic().is_some() && error.diagnostic_context().is_some();
                let error = error
                    .with_diagnostic_if_absent(
                        DeviceErrorCategory::Native,
                        HostLocation::DeviceRegistryInputOperation,
                    )
                    .with_diagnostic_context_if_absent(
                        self.requested_backend.as_str(),
                        operation,
                        DeviceErrorSensitivity::Sensitive,
                    );
                let error = if producer_complete {
                    error
                } else {
                    error.with_diagnostic_message(
                        DeviceErrorDiagnosticMessage::DeviceRegistryInputOperationFailed,
                    )
                };
                Err(error)
            }
        }
    }
}

impl InputBackend for DeviceRegistryInputDiagnosticBackend {
    fn take_backend_open_observations(
        &mut self,
    ) -> Vec<actingcommand_device::BackendOpenObservation> {
        self.backend.take_backend_open_observations()
    }

    fn selection_context(&self) -> Option<actingcommand_device::InputSelectionContext> {
        self.backend.selection_context()
    }

    fn tap(&mut self, witness: &FencedWrite, x: i32, y: i32) -> DeviceResult<()> {
        self.run(DeviceLocation::InputTap, |backend| {
            backend.tap(witness, x, y)
        })
    }

    fn tap_in_frame(
        &mut self,
        witness: &FencedWrite,
        x: i32,
        y: i32,
        context: &actingcommand_device::InputExecutionContext,
    ) -> DeviceResult<()> {
        self.run(DeviceLocation::InputTap, |backend| {
            backend.tap_in_frame(witness, x, y, context)
        })
    }

    fn long_tap(
        &mut self,
        witness: &FencedWrite,
        x: i32,
        y: i32,
        duration_ms: u64,
    ) -> DeviceResult<()> {
        self.run(DeviceLocation::InputLongTap, |backend| {
            backend.long_tap(witness, x, y, duration_ms)
        })
    }

    fn long_tap_in_frame(
        &mut self,
        witness: &FencedWrite,
        x: i32,
        y: i32,
        duration_ms: u64,
        context: &actingcommand_device::InputExecutionContext,
    ) -> DeviceResult<()> {
        self.run(DeviceLocation::InputLongTap, |backend| {
            backend.long_tap_in_frame(witness, x, y, duration_ms, context)
        })
    }

    fn swipe_in_frame(
        &mut self,
        witness: &FencedWrite,
        from: (i32, i32),
        to: (i32, i32),
        duration_ms: u64,
        context: &actingcommand_device::InputExecutionContext,
    ) -> DeviceResult<()> {
        self.run(DeviceLocation::InputSwipe, |backend| {
            backend.swipe_in_frame(witness, from, to, duration_ms, context)
        })
    }

    fn swipe(
        &mut self,
        witness: &FencedWrite,
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        duration_ms: u64,
    ) -> DeviceResult<()> {
        self.run(DeviceLocation::InputSwipe, |backend| {
            backend.swipe(witness, x1, y1, x2, y2, duration_ms)
        })
    }

    fn supports_segmented_swipe(&self) -> bool {
        self.backend.supports_segmented_swipe()
    }

    fn segmented_swipe_prepared(
        &mut self,
        witness: &FencedWrite,
        plan: &PreparedSegmentedSwipePlan,
    ) -> DeviceResult<()> {
        self.run(DeviceLocation::SegmentedSwipe, |backend| {
            backend.segmented_swipe_prepared(witness, plan)
        })
    }

    fn segmented_swipe_prepared_in_frame(
        &mut self,
        witness: &FencedWrite,
        plan: &PreparedSegmentedSwipePlan,
        context: &actingcommand_device::InputExecutionContext,
    ) -> DeviceResult<()> {
        self.run(DeviceLocation::SegmentedSwipe, |backend| {
            backend.segmented_swipe_prepared_in_frame(witness, plan, context)
        })
    }

    fn key(&mut self, witness: &FencedWrite, key: &str) -> DeviceResult<()> {
        self.run(DeviceLocation::InputKey, |backend| {
            backend.key(witness, key)
        })
    }

    fn text(&mut self, witness: &FencedWrite, text: &str) -> DeviceResult<()> {
        self.run(DeviceLocation::InputText, |backend| {
            backend.text(witness, text)
        })
    }

    fn reset(&mut self, witness: &FencedWrite) -> DeviceResult<()> {
        self.run(DeviceLocation::InputReset, |backend| backend.reset(witness))
    }

    fn reset_with_check(
        &mut self,
        witness: &FencedWrite,
        context: &actingcommand_device::InputExecutionContext,
    ) -> DeviceResult<()> {
        self.run(DeviceLocation::InputReset, |backend| {
            backend.reset_with_check(witness, context)
        })
    }

    fn close_once(
        &mut self,
        authority: actingcommand_device::DeviceCloseAuthority,
    ) -> DeviceResult<actingcommand_device::DeviceResourceCloseOutcome> {
        self.run(DeviceLocation::InputClose, |backend| {
            backend.close_once(authority)
        })
    }
}

fn validate_alias(alias: &str) -> RuntimeHostResult<()> {
    if actingcommand_contract::validate_instance_alias(alias).is_err() {
        return Err(RuntimeHostError::fatal(
            SharedCode::InvalidInstanceAlias,
            HostLocation::BuildExecutionBackendRegistry,
            RuntimeErrorCode::RuntimeHalted,
        ));
    }
    Ok(())
}

fn validate_application_id(application_id: &str) -> RuntimeHostResult<()> {
    if application_id.trim().is_empty()
        || application_id.len() > MAX_INSTANCE_ALIAS_BYTES
        || application_id.chars().any(char::is_control)
    {
        return Err(RuntimeHostError::fatal(
            HostCode::InvalidApplicationIdentity,
            HostLocation::BuildExecutionBackendRegistry,
            RuntimeErrorCode::RuntimeHalted,
        ));
    }
    Ok(())
}

// The input-operation diagnostic wrapper's own criteria, moved here with the wrapper from the
// daemon (Workflow #257 DEVICE-DIAGNOSTIC-v1 / #241 segmented swipe v2).
#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_device::{SegmentedDragAction, SegmentedSwipeAction};

    /// A scheduler-shaped step witness for driving a write method directly.
    fn step_witness() -> FencedWrite {
        let issuer = actingcommand_contract::IdentifierIssuer::new().expect("ids");
        actingcommand_contract::issue_fenced_write(
            actingcommand_contract::LeaseToken::new(
                *issuer.mint_owner_epoch().expect("epoch").transport(),
                *issuer.mint_lease_id().expect("lease").transport(),
                *issuer.mint_instance_id().expect("instance").transport(),
                *issuer.mint_holder_id().expect("holder").transport(),
                100,
            )
            .expect("test step token"),
            1,
            std::num::NonZeroU64::new(1).expect("step"),
            actingcommand_contract::FencedWritePurpose::Business,
        )
    }

    #[derive(Debug, Clone, Copy)]
    enum TestInputOperation {
        Tap,
        LongTap,
        Swipe,
        Key,
        Text,
        Reset,
        Close,
    }

    const TEST_INPUT_OPERATIONS: [TestInputOperation; 7] = [
        TestInputOperation::Tap,
        TestInputOperation::LongTap,
        TestInputOperation::Swipe,
        TestInputOperation::Key,
        TestInputOperation::Text,
        TestInputOperation::Reset,
        TestInputOperation::Close,
    ];

    impl TestInputOperation {
        const fn name(self) -> &'static str {
            match self {
                Self::Tap => "tap",
                Self::LongTap => "long_tap",
                Self::Swipe => "swipe",
                Self::Key => "key",
                Self::Text => "text",
                Self::Reset => "reset",
                Self::Close => "close",
            }
        }

        /// The operation the diagnostic wrapper names for this input (Workflow #378 A2-P).
        const fn location(self) -> DeviceLocation {
            match self {
                Self::Tap => DeviceLocation::InputTap,
                Self::LongTap => DeviceLocation::InputLongTap,
                Self::Swipe => DeviceLocation::InputSwipe,
                Self::Key => DeviceLocation::InputKey,
                Self::Text => DeviceLocation::InputText,
                Self::Reset => DeviceLocation::InputReset,
                Self::Close => DeviceLocation::InputClose,
            }
        }

        fn invoke(self, backend: &mut dyn InputBackend) -> DeviceResult<()> {
            let witness = step_witness();
            match self {
                Self::Tap => backend.tap(&witness, 10, 20),
                Self::LongTap => backend.long_tap(&witness, 10, 20, 30),
                Self::Swipe => backend.swipe(&witness, 10, 20, 30, 40, 50),
                Self::Key => backend.key(&witness, "KEYCODE_HOME"),
                Self::Text => backend.text(&witness, "neutral fixture"),
                Self::Reset => backend.reset(&witness),
                Self::Close => backend.close(),
            }
        }
    }

    struct RecordingInputBackend {
        result: DeviceResult<()>,
        calls: Arc<Mutex<Vec<&'static str>>>,
        segmented_swipe_actions: Arc<Mutex<Vec<SegmentedDragAction>>>,
        supports_segmented_swipe: bool,
    }

    impl RecordingInputBackend {
        fn invoke(&self, operation: &'static str) -> DeviceResult<()> {
            self.calls.lock().expect("input calls").push(operation);
            self.result.clone()
        }
    }

    impl InputBackend for RecordingInputBackend {
        fn tap(&mut self, _witness: &FencedWrite, _x: i32, _y: i32) -> DeviceResult<()> {
            self.invoke("tap")
        }

        fn long_tap(
            &mut self,
            _witness: &FencedWrite,
            _x: i32,
            _y: i32,
            _duration_ms: u64,
        ) -> DeviceResult<()> {
            self.invoke("long_tap")
        }

        fn swipe(
            &mut self,
            _witness: &FencedWrite,
            _x1: i32,
            _y1: i32,
            _x2: i32,
            _y2: i32,
            _duration_ms: u64,
        ) -> DeviceResult<()> {
            self.invoke("swipe")
        }

        fn supports_segmented_swipe(&self) -> bool {
            self.supports_segmented_swipe
        }

        fn segmented_swipe_prepared(
            &mut self,
            _witness: &FencedWrite,
            plan: &PreparedSegmentedSwipePlan,
        ) -> DeviceResult<()> {
            self.segmented_swipe_actions
                .lock()
                .expect("segmented swipe actions")
                .push(plan.action());
            self.invoke("segmented_swipe")
        }

        fn key(&mut self, _witness: &FencedWrite, _key: &str) -> DeviceResult<()> {
            self.invoke("key")
        }

        fn text(&mut self, _witness: &FencedWrite, _text: &str) -> DeviceResult<()> {
            self.invoke("text")
        }

        fn reset(&mut self, _witness: &FencedWrite) -> DeviceResult<()> {
            self.invoke("reset")
        }

        fn close_once(
            &mut self,
            _authority: actingcommand_device::DeviceCloseAuthority,
        ) -> DeviceResult<actingcommand_device::DeviceResourceCloseOutcome> {
            self.invoke("close")?;
            Ok(actingcommand_device::DeviceResourceCloseOutcome::confirmed(
                1,
            ))
        }
    }

    struct DiagnosticInputBackendFixture {
        backend: DeviceRegistryInputDiagnosticBackend,
        calls: Arc<Mutex<Vec<&'static str>>>,
        segmented_swipe_actions: Arc<Mutex<Vec<SegmentedDragAction>>>,
    }

    fn diagnostic_input_backend(result: DeviceResult<()>) -> DiagnosticInputBackendFixture {
        diagnostic_input_backend_with(result, TouchBackendChoice::AdbShellInput, false)
    }

    fn diagnostic_input_backend_with(
        result: DeviceResult<()>,
        requested_backend: TouchBackendChoice,
        supports_segmented_swipe: bool,
    ) -> DiagnosticInputBackendFixture {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let segmented_swipe_actions = Arc::new(Mutex::new(Vec::new()));
        DiagnosticInputBackendFixture {
            backend: DeviceRegistryInputDiagnosticBackend::new(
                Box::new(RecordingInputBackend {
                    result,
                    calls: Arc::clone(&calls),
                    segmented_swipe_actions: Arc::clone(&segmented_swipe_actions),
                    supports_segmented_swipe,
                }),
                requested_backend,
            ),
            calls,
            segmented_swipe_actions,
        }
    }

    fn test_segmented_swipe_action() -> SegmentedSwipeAction {
        SegmentedSwipeAction {
            points: [(1095, 355), (105, 357), (105, 257)],
            horizontal_duration_ms: 200,
            corner_hold_ms: 150,
            brake_distance_px: 100,
            brake_duration_ms: 200,
            slope_in: 2,
            slope_out: 0,
        }
    }

    fn native_style_operation_error() -> DeviceError {
        DeviceError::transient(
            "command=\"adb -s 127.0.0.1:16384 shell input tap 10 20\" \
             exit_status=Some(1) stdout=\"fixture stdout line 1\nfixture stdout line 2\" \
             stderr=\"fixture stderr\"",
        )
    }

    // Workflow #257 DEVICE-DIAGNOSTIC-v1: specification criteria for the device wrapper.
    #[test]
    fn device_registry_input_operation_failure_matrix_preserves_error_and_context() {
        for operation in TEST_INPUT_OPERATIONS {
            let original = native_style_operation_error();
            let DiagnosticInputBackendFixture {
                mut backend, calls, ..
            } = diagnostic_input_backend(Err(original.clone()));
            let returned = operation
                .invoke(&mut backend)
                .expect_err("configured operation failure");
            assert_eq!(returned.severity(), original.severity());
            assert_eq!(returned.message(), original.message());
            assert_eq!(
                returned.diagnostic_message(),
                Some("device registry input operation failed")
            );
            assert_eq!(
                returned.is_fallback_eligible(),
                original.is_fallback_eligible()
            );
            let diagnostic = returned.diagnostic().expect("adapter diagnostic");
            assert_eq!(diagnostic.category(), DeviceErrorCategory::Native);
            assert_eq!(
                diagnostic.stage().as_str(),
                "device_registry_input_operation"
            );
            let context = returned
                .diagnostic_context()
                .expect("adapter diagnostic context");
            assert_eq!(context.backend(), "adb_shell_input");
            assert_eq!(context.operation(), operation.location().location());
            assert_eq!(
                context.declared_sensitivity(),
                DeviceErrorSensitivity::Sensitive
            );
            assert_eq!(*calls.lock().expect("input calls"), [operation.name()]);
        }

        let original = DeviceError::transient("producer-owned private input failure")
            .with_diagnostic(
                DeviceErrorCategory::CommandWrite,
                actingcommand_device::codes::DeviceStage::MaatouchStdinWrite.location(),
            )
            .with_diagnostic_context(
                "maatouch",
                actingcommand_device::codes::DeviceLocation::InputSwipe,
                DeviceErrorSensitivity::Internal,
            );
        let DiagnosticInputBackendFixture {
            mut backend, calls, ..
        } = diagnostic_input_backend(Err(original.clone()));
        let returned = TestInputOperation::Reset
            .invoke(&mut backend)
            .expect_err("producer-classified operation failure");
        assert_eq!(returned, original);
        assert_eq!(returned.message(), original.message());
        assert_eq!(returned.diagnostic_message(), None);
        let diagnostic = returned.diagnostic().expect("producer diagnostic");
        assert_eq!(diagnostic.category(), DeviceErrorCategory::CommandWrite);
        assert_eq!(diagnostic.stage().as_str(), "maatouch_stdin_write");
        let context = returned
            .diagnostic_context()
            .expect("producer diagnostic context");
        assert_eq!(context.backend(), "maatouch");
        assert_eq!(context.operation().as_str(), "input_swipe");
        assert_eq!(
            context.declared_sensitivity(),
            DeviceErrorSensitivity::Internal
        );
        assert_eq!(*calls.lock().expect("input calls"), ["reset"]);
    }

    // Test class: specification criterion.
    #[test]
    fn maatouch_write_failure_preserves_typed_diagnostic() {
        let original = DeviceError::transient("Broken pipe (os error 232)").with_diagnostic(
            DeviceErrorCategory::CommandWrite,
            actingcommand_device::codes::DeviceStage::MaatouchStdinWrite.location(),
        );
        let DiagnosticInputBackendFixture {
            mut backend, calls, ..
        } = diagnostic_input_backend_with(
            Err(original.clone()),
            TouchBackendChoice::MaaTouch,
            true,
        );
        let returned = backend
            .swipe(&step_witness(), 10, 20, 30, 40, 50)
            .expect_err("controlled MaaTouch write failure");
        assert_eq!(returned, original);
        assert_eq!(returned.message(), original.message());
        assert_eq!(
            returned.diagnostic().expect("diagnostic").category(),
            DeviceErrorCategory::CommandWrite
        );
        assert_eq!(
            returned.diagnostic().expect("diagnostic").stage().as_str(),
            "maatouch_stdin_write"
        );
        let context = returned.diagnostic_context().expect("context");
        assert_eq!(context.backend(), "maatouch");
        assert_eq!(context.operation().as_str(), "input_swipe");
        assert_eq!(
            context.declared_sensitivity(),
            DeviceErrorSensitivity::Sensitive
        );
        assert_eq!(*calls.lock().expect("input calls"), ["swipe"]);
    }

    // Workflow #241 / DeviceRegistry segmented swipe v2. Specification criterion.
    #[test]
    fn device_registry_input_segmented_capability_matches_inner_backend() {
        for supported in [false, true] {
            let DiagnosticInputBackendFixture { backend, calls, .. } =
                diagnostic_input_backend_with(Ok(()), TouchBackendChoice::MaaTouch, supported);
            assert_eq!(backend.supports_segmented_swipe(), supported);
            assert!(calls.lock().expect("input calls").is_empty());
        }
    }

    // Workflow #241 / DeviceRegistry segmented swipe v2. Specification criterion.
    #[test]
    fn device_registry_input_segmented_swipe_forwards_exact_action() {
        let action = test_segmented_swipe_action();
        let DiagnosticInputBackendFixture {
            mut backend,
            calls,
            segmented_swipe_actions,
        } = diagnostic_input_backend_with(Ok(()), TouchBackendChoice::MaaTouch, true);
        backend
            .segmented_swipe(&step_witness(), action)
            .expect("segmented swipe succeeds");
        assert_eq!(*calls.lock().expect("input calls"), ["segmented_swipe"]);
        assert_eq!(
            *segmented_swipe_actions
                .lock()
                .expect("segmented swipe actions"),
            [SegmentedDragAction::VerticalBrake(action)]
        );
    }

    // Workflow #241 / DeviceRegistry segmented swipe v2. Specification criterion.
    #[test]
    fn device_registry_input_segmented_swipe_failure_preserves_error() {
        let action = test_segmented_swipe_action();
        let original = DeviceError::transient(
            "failed to write segmented swipe to fixture.device:16384: Broken pipe",
        )
        .with_diagnostic(
            DeviceErrorCategory::CommandWrite,
            actingcommand_device::codes::DeviceStage::MaatouchStdinWrite.location(),
        );
        let DiagnosticInputBackendFixture {
            mut backend,
            calls,
            segmented_swipe_actions,
        } = diagnostic_input_backend_with(
            Err(original.clone()),
            TouchBackendChoice::MaaTouch,
            true,
        );
        let returned = backend
            .segmented_swipe(&step_witness(), action)
            .expect_err("segmented swipe failure");
        assert_eq!(returned, original);
        assert_eq!(returned.message(), original.message());
        assert_eq!(
            returned.diagnostic().expect("diagnostic").category(),
            DeviceErrorCategory::CommandWrite
        );
        assert_eq!(
            returned.diagnostic().expect("diagnostic").stage().as_str(),
            "maatouch_stdin_write"
        );
        assert_eq!(
            returned
                .diagnostic_context()
                .expect("context")
                .operation()
                .as_str(),
            "segmented_swipe"
        );
        assert_eq!(*calls.lock().expect("input calls"), ["segmented_swipe"]);
        assert_eq!(
            *segmented_swipe_actions
                .lock()
                .expect("segmented swipe actions"),
            [SegmentedDragAction::VerticalBrake(action)]
        );
    }

    #[test]
    fn device_registry_input_operation_success_matrix_delegates_once() {
        for operation in TEST_INPUT_OPERATIONS {
            let DiagnosticInputBackendFixture {
                mut backend, calls, ..
            } = diagnostic_input_backend(Ok(()));
            operation
                .invoke(&mut backend)
                .expect("configured operation success");
            assert_eq!(*calls.lock().expect("input calls"), [operation.name()]);
        }
    }
}
