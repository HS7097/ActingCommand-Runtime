// SPDX-License-Identifier: AGPL-3.0-only

//! MuMu instance discovery through the vendor's documented `MuMuManager.exe` command line.
//!
//! Only the read-only subcommands `version` and `info -v all` are ever dispatched. The
//! hidden `api` subcommand and every mutating subcommand (`control`, `setting`, `launch`,
//! `shutdown`, `restart`, ...) are never used by this module. No vendor-private file is read.

use crate::adb::{
    ACTINGCOMMAND_NEMU_FOLDER_ENV, CommandProgram, decode_adb_text, run_raw_with_timeout,
};
use crate::emulator::{
    EmulatorCapability, EmulatorCapabilityAvailability, EmulatorCapabilityBackend,
    EmulatorCapabilityEvidence, EmulatorCapabilityImplementation, EmulatorCapabilityProfile,
    EmulatorVersionEvidence,
};
use crate::mumu::{
    MumuInstallSource, MumuInstallation, canonicalize_backend_file, canonicalize_install_root,
    display_paths, known_vendor_parent_dirs, path_is_within_mumu_root, resolve_mumu_adb,
    resolve_mumu_installation_from_sources, select_unique_installation,
};
use crate::{
    DeviceError, DeviceErrorCategory, DeviceErrorDiagnosticMessage, DeviceErrorSensitivity,
    DeviceResult, NemuResolutionContext, NemuResolutionCountKind, NemuResolutionReason,
};
use std::cmp::Ordering;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Runtime policy floor for `MuMuManager version`. The vendor documents only V4.0.0.3179 as
/// the `MuMuManager` baseline and states no minimum for the `info` JSON shape this module
/// parses; 6.3.2.0 is the oldest line the Runtime chooses to support, not a vendor fact.
pub const MUMU_MANAGER_MINIMUM_VERSION: MumuManagerVersion = MumuManagerVersion([6, 3, 2, 0]);
pub const MUMU_MANAGER_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
pub const MAX_MUMU_INSTANCE_NAME_BYTES: usize = 256;
const MAX_MUMU_PLAYER_STATE_BYTES: usize = 64;
const MAX_MUMU_ADB_HOST_BYTES: usize = 64;
const MAX_MUMU_MANAGER_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(not(windows))]
const CREATE_NO_WINDOW: u32 = 0;

const MUMU_MANAGER_PROGRAM: CommandProgram = CommandProgram {
    name: "MuMuManager",
    stdout_reader: "mumu_manager_stdout",
    stderr_reader: "mumu_manager_stderr",
    windows_creation_flags: CREATE_NO_WINDOW,
};

/// Where the `MuMuManager.exe` install root came from, in resolution priority order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MumuManagerSource {
    ExplicitRoot,
    FolderEnvironment,
    RunningProcess,
    RegistryUninstall,
    VendorEnumeration,
}

impl MumuManagerSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExplicitRoot => "explicit_root",
            Self::FolderEnvironment => "env:ACTINGCOMMAND_NEMU_FOLDER",
            Self::RunningProcess => "mumu_running_process",
            Self::RegistryUninstall => "mumu_registry_uninstall",
            Self::VendorEnumeration => "mumu_vendor_enumeration",
        }
    }

    const fn install_source(self) -> MumuInstallSource {
        match self {
            Self::ExplicitRoot | Self::FolderEnvironment => MumuInstallSource::ExplicitFolder,
            Self::RunningProcess => MumuInstallSource::RunningProcess,
            Self::RegistryUninstall => MumuInstallSource::RegistryUninstall,
            Self::VendorEnumeration => MumuInstallSource::VendorEnumeration,
        }
    }
}

/// A four-part `MuMuManager version` value, ordered part by part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MumuManagerVersion([u32; 4]);

impl MumuManagerVersion {
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = [0_u32; 4];
        let mut count = 0;
        for part in text.split('.') {
            if count == 4 || part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            parts[count] = part.parse().ok()?;
            count += 1;
        }
        (count == 4).then_some(Self(parts))
    }

    pub const fn parts(self) -> [u32; 4] {
        self.0
    }
}

impl PartialOrd for MumuManagerVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MumuManagerVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        for (left, right) in self.0.iter().zip(other.0.iter()) {
            match left.cmp(right) {
                Ordering::Equal => continue,
                unequal => return unequal,
            }
        }
        Ordering::Equal
    }
}

impl fmt::Display for MumuManagerVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [major, minor, patch, build] = self.0;
        write!(formatter, "{major}.{minor}.{patch}.{build}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMumuManager {
    pub install_root: PathBuf,
    pub mumu_manager_path: PathBuf,
    pub adb_path: PathBuf,
    pub source: MumuManagerSource,
    /// `DisplayVersion` of the uninstall entry, advisory only; `MuMuManager version` is authoritative.
    pub registry_display_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredMumuInstance {
    pub install_root: PathBuf,
    pub mumu_manager_path: PathBuf,
    pub adb_path: PathBuf,
    pub instance_index: u16,
    pub instance_name: String,
    pub adb_host: String,
    pub adb_port: u16,
    /// `is_process_started && is_android_started`.
    pub running: bool,
    pub player_state: String,
    pub mumu_version: MumuManagerVersion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MumuDiscoveryReport {
    pub source: MumuManagerSource,
    pub mumu_manager_path: PathBuf,
    pub version: MumuManagerVersion,
    pub registry_display_version: Option<String>,
    pub instances: Vec<DiscoveredMumuInstance>,
}

/// The single discovery entry used by the daemon and the device-test probe.
pub fn discover_mumu_instances(explicit_root: Option<&Path>) -> DeviceResult<MumuDiscoveryReport> {
    discover_mumu_instances_inner(explicit_root).map_err(with_mumu_manager_discovery_detail)
}

fn discover_mumu_instances_inner(
    explicit_root: Option<&Path>,
) -> DeviceResult<MumuDiscoveryReport> {
    let manager = resolve_mumu_manager(explicit_root)?;
    let version = query_version(&manager.mumu_manager_path)?;
    let instances = query_instances(&manager, version)?;
    Ok(MumuDiscoveryReport {
        source: manager.source,
        mumu_manager_path: manager.mumu_manager_path,
        version,
        registry_display_version: manager.registry_display_version,
        instances,
    })
}

/// Provider id of the profile derived from a `MuMuManager` discovery report.
pub const MUMU_CAPABILITY_PROVIDER_ID: &str = "mumu.manager";
const MUMU_MANAGER_COMMAND_REFERENCE_URL: &str =
    "https://mumu.163.com/help/20240807/40912_1170006.html";

/// Derives the MuMu capability profile from one discovery report. Pure: nothing is dispatched.
///
/// `inventory.read` and `instance.status.read` are available because `info -v all` answered;
/// `instance.start|stop|restart` are documented (`control -v <index> launch|shutdown|restart`)
/// but not yet exercised by the Runtime, so they stay unverified; every other capability is
/// unsupported and unavailable through this provider.
pub fn mumu_capability_profile(
    report: &MumuDiscoveryReport,
) -> DeviceResult<EmulatorCapabilityProfile> {
    let evidence = EmulatorCapability::ALL
        .into_iter()
        .map(|capability| {
            let (implementation, availability, failure_semantics, evidence_ref) = match capability {
                EmulatorCapability::InventoryRead | EmulatorCapability::InstanceStatusRead => (
                    EmulatorCapabilityImplementation::Supported,
                    EmulatorCapabilityAvailability::Available,
                    "MuMuManager info -v all answered with exit 0 and a parseable instance map at discovery. A refusal surfaces as a fatal typed device error (mumu_manager.run, .exit, .decode, .json, .errcode, .shape, .output_bound), never as an empty inventory.",
                    "mumu_manager.info",
                ),
                EmulatorCapability::InstanceStart
                | EmulatorCapability::InstanceStop
                | EmulatorCapability::InstanceRestart => (
                    EmulatorCapabilityImplementation::Supported,
                    EmulatorCapabilityAvailability::Unverified,
                    "Documented as MuMuManager control -v <index> launch|shutdown|restart, not yet exercised by the Runtime: no control subcommand is dispatched. Execution requires the emulator control slice.",
                    MUMU_MANAGER_COMMAND_REFERENCE_URL,
                ),
                EmulatorCapability::InstanceCreate
                | EmulatorCapability::InstanceClone
                | EmulatorCapability::InstanceDelete
                | EmulatorCapability::InstanceConfigure
                | EmulatorCapability::SnapshotManage => (
                    EmulatorCapabilityImplementation::Unsupported,
                    EmulatorCapabilityAvailability::Unavailable,
                    "Instance creation, cloning, deletion, configuration and snapshots are not driven by this Runtime; MuMuManager is never asked to perform them.",
                    MUMU_CAPABILITY_PROVIDER_ID,
                ),
                EmulatorCapability::ApplicationControl | EmulatorCapability::AdbBridge => (
                    EmulatorCapabilityImplementation::Unsupported,
                    EmulatorCapabilityAvailability::Unavailable,
                    "Application control and the ADB bridge are not driven through MuMuManager; the instance is reached through its discovered ADB target, not through this provider claim.",
                    MUMU_CAPABILITY_PROVIDER_ID,
                ),
                EmulatorCapability::InputTap
                | EmulatorCapability::InputLongTap
                | EmulatorCapability::InputSwipe
                | EmulatorCapability::InputSegmentedSwipe
                | EmulatorCapability::InputKey
                | EmulatorCapability::InputText
                | EmulatorCapability::InputReset
                | EmulatorCapability::CaptureFrame
                | EmulatorCapability::ApplicationLaunch
                | EmulatorCapability::ApplicationStop
                | EmulatorCapability::ApplicationRestart => (
                    EmulatorCapabilityImplementation::Unsupported,
                    EmulatorCapabilityAvailability::Unavailable,
                    "Input, capture and application lifecycle belong to the touch/capture backend registry and the ADB application lifecycle path, not to MuMuManager.",
                    MUMU_CAPABILITY_PROVIDER_ID,
                ),
            };
            EmulatorCapabilityEvidence::new(capability, availability, failure_semantics, evidence_ref)?
                .with_implementation(implementation)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(EmulatorCapabilityProfile::new(
        MUMU_CAPABILITY_PROVIDER_ID,
        EmulatorVersionEvidence::Exact {
            value: report.version.to_string(),
        },
        evidence,
    )?)
}

/// Capability backend over one already obtained discovery report; probing dispatches nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MumuEmulatorCapabilityBackend {
    report: MumuDiscoveryReport,
}

impl MumuEmulatorCapabilityBackend {
    pub const fn new(report: MumuDiscoveryReport) -> Self {
        Self { report }
    }

    /// Runs `discover_mumu_instances` once (`version` and `info -v all` only) and keeps the report.
    pub fn from_discovery(explicit_root: Option<&Path>) -> DeviceResult<Self> {
        discover_mumu_instances(explicit_root).map(Self::new)
    }

    pub const fn report(&self) -> &MumuDiscoveryReport {
        &self.report
    }
}

impl EmulatorCapabilityBackend for MumuEmulatorCapabilityBackend {
    fn probe_capabilities(&mut self) -> DeviceResult<EmulatorCapabilityProfile> {
        mumu_capability_profile(&self.report)
    }
}

fn with_mumu_manager_discovery_detail(error: DeviceError) -> DeviceError {
    let producer_complete = error.diagnostic().is_some() && error.diagnostic_context().is_some();
    let producer_message = error.diagnostic_message().is_some();
    let error = error
        .with_diagnostic_if_absent(DeviceErrorCategory::Protocol, "mumu_manager.discover")
        .with_diagnostic_context_if_absent(
            "mumu_manager",
            "discover_instances",
            DeviceErrorSensitivity::Sensitive,
        );
    if producer_complete || producer_message {
        error
    } else {
        error.with_diagnostic_message(DeviceErrorDiagnosticMessage::MumuManagerDiscoveryFailed)
    }
}

/// Resolves `MuMuManager.exe` in priority order: explicit root, `ACTINGCOMMAND_NEMU_FOLDER`,
/// a running MuMu process, the Windows uninstall registry entry, then vendor folder enumeration.
pub fn resolve_mumu_manager(explicit_root: Option<&Path>) -> DeviceResult<ResolvedMumuManager> {
    if let Some(root) = explicit_root {
        return manager_in_root(root, MumuManagerSource::ExplicitRoot, None);
    }
    if let Some(root) = std::env::var_os(ACTINGCOMMAND_NEMU_FOLDER_ENV).map(PathBuf::from) {
        return manager_in_root(&root, MumuManagerSource::FolderEnvironment, None);
    }
    let running = crate::discovery::running_mumu_executable_paths()?;
    if let Some(installation) = resolve_mumu_installation_from_sources(None, &running, &[])? {
        return manager_in_root(&installation.root, MumuManagerSource::RunningProcess, None);
    }
    let entries = registry::mumu_uninstall_entries()?;
    if !entries.is_empty() {
        let mut roots = Vec::with_capacity(entries.len());
        for entry in &entries {
            let root = canonicalize_install_root(
                &entry.install_root,
                MumuInstallSource::RegistryUninstall,
            )
            .map_err(|error| {
                let message = format!("{} (registry key {})", error.message(), entry.key_path);
                error.with_message(message)
            })?;
            let candidates = mumu_manager_candidates(&root);
            if !candidates.iter().any(|path| path.is_file()) {
                return Err(DeviceError::fatal(format!(
                    "registry key {} names MuMu install root {} but no MuMuManager.exe candidate exists; checked: {}",
                    entry.key_path,
                    root.display(),
                    display_paths(&candidates)
                ))
                .with_nemu_resolution_context_if_absent(
                    NemuResolutionContext::new(NemuResolutionReason::CandidateAbsent)
                        .with_count(NemuResolutionCountKind::ManagerExecutables, 0, false)
                        .with_source(MumuInstallSource::RegistryUninstall),
                ));
            }
            roots.push(root);
        }
        let installation = select_unique_installation(roots, MumuInstallSource::RegistryUninstall)?;
        let advisory = entries
            .into_iter()
            .next()
            .and_then(|entry| entry.display_version);
        return manager_in_root(
            &installation.root,
            MumuManagerSource::RegistryUninstall,
            advisory,
        );
    }
    let vendor_parents = known_vendor_parent_dirs();
    let Some(installation) = resolve_mumu_installation_from_sources(None, &[], &vendor_parents)?
    else {
        return Err(DeviceError::fatal(format!(
            "no MuMu installation was found: configure mumu_root, set {ACTINGCOMMAND_NEMU_FOLDER_ENV}, start MuMu, or install it at a registered or vendor path"
        ))
        .with_nemu_resolution_context_if_absent(
            NemuResolutionContext::new(NemuResolutionReason::InstallationAbsent)
                .with_count(NemuResolutionCountKind::InstallationRoots, 0, false)
                .with_source(MumuInstallSource::VendorEnumeration),
        ));
    };
    manager_in_root(
        &installation.root,
        MumuManagerSource::VendorEnumeration,
        None,
    )
}

/// The only `MuMuManager.exe` locations in an install: `nx_main` (v5/v6) or legacy `shell`.
fn mumu_manager_candidates(root: &Path) -> Vec<PathBuf> {
    vec![
        root.join("nx_main").join("MuMuManager.exe"),
        root.join("shell").join("MuMuManager.exe"),
    ]
}

fn manager_in_root(
    root: &Path,
    source: MumuManagerSource,
    registry_display_version: Option<String>,
) -> DeviceResult<ResolvedMumuManager> {
    let install_source = source.install_source();
    let root = canonicalize_install_root(root, install_source)?;
    let candidates = mumu_manager_candidates(&root);
    let Some(candidate) = candidates.iter().find(|path| path.is_file()) else {
        return Err(DeviceError::fatal(format!(
            "MuMuManager.exe discovery selected source={} install_root={} but no candidate file exists; checked: {}",
            source.as_str(),
            root.display(),
            display_paths(&candidates)
        ))
        .with_nemu_resolution_context_if_absent(
            NemuResolutionContext::new(NemuResolutionReason::CandidateAbsent)
                .with_count(NemuResolutionCountKind::ManagerExecutables, 0, false)
                .with_source(install_source),
        ));
    };
    let mumu_manager_path = canonicalize_backend_file(candidate, "MuMuManager executable")?;
    if !path_is_within_mumu_root(&mumu_manager_path, &root) {
        return Err(DeviceError::fatal(format!(
            "MuMuManager executable resolved outside selected installation root {}: {}",
            root.display(),
            mumu_manager_path.display()
        ))
        .with_nemu_resolution_context_if_absent(
            NemuResolutionContext::new(NemuResolutionReason::CandidateOutsideRoot)
                .with_source(install_source),
        ));
    }
    let installation = MumuInstallation {
        root: root.clone(),
        source: install_source,
    };
    let adb_path = resolve_mumu_adb(&installation)?;
    Ok(ResolvedMumuManager {
        install_root: root,
        mumu_manager_path,
        adb_path,
        source,
        registry_display_version,
    })
}

/// Runs `MuMuManager version` and enforces `MUMU_MANAGER_MINIMUM_VERSION`.
pub fn query_version(mumu_manager_path: &Path) -> DeviceResult<MumuManagerVersion> {
    let args = ["version"];
    let document = run_json(mumu_manager_path, &args)?;
    let observed = document
        .as_object()
        .and_then(|object| object.get("version"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            shape_error(
                &args,
                "top-level object with a string \"version\" field",
                &document,
            )
        })?;
    let version = MumuManagerVersion::parse(observed).ok_or_else(|| {
        DeviceError::fatal(format!(
            "MuMuManager version {observed:?} is not a four-part numeric version; required at least {MUMU_MANAGER_MINIMUM_VERSION} (Runtime policy floor; the vendor documents no minimum for this interface)"
        ))
        .with_diagnostic(DeviceErrorCategory::Response, "mumu_manager.version")
        .with_nemu_resolution_context_if_absent(NemuResolutionContext::new(
            NemuResolutionReason::ProviderVersionUnparseable,
        ))
    })?;
    if version < MUMU_MANAGER_MINIMUM_VERSION {
        return Err(DeviceError::fatal(format!(
            "MuMuManager version {version} is below the Runtime policy floor {MUMU_MANAGER_MINIMUM_VERSION} (observed={version} required={MUMU_MANAGER_MINIMUM_VERSION}); the floor is a Runtime policy, not a vendor-documented minimum"
        ))
        .with_diagnostic(DeviceErrorCategory::Response, "mumu_manager.version_floor")
        .with_nemu_resolution_context_if_absent(NemuResolutionContext::new(
            NemuResolutionReason::ProviderVersionBelowMinimum,
        )));
    }
    Ok(version)
}

/// Runs `MuMuManager info -v all` and parses every instance entry.
pub fn query_instances(
    manager: &ResolvedMumuManager,
    mumu_version: MumuManagerVersion,
) -> DeviceResult<Vec<DiscoveredMumuInstance>> {
    let args = ["info", "-v", "all"];
    let document = run_json(&manager.mumu_manager_path, &args)?;
    let Some(object) = document.as_object() else {
        return Err(shape_error(
            &args,
            "top-level object keyed by instance index",
            &document,
        ));
    };
    if let Some(envelope) = errcode_envelope(object) {
        return Err(DeviceError::fatal(format!(
            "MuMuManager {} returned an error envelope with exit 0: {envelope}",
            args.join(" ")
        ))
        .with_diagnostic(DeviceErrorCategory::Response, "mumu_manager.errcode"));
    }
    let mut instances = Vec::new();
    if object.contains_key("index") {
        // `info -v <single>` shape: one flat instance object instead of a map.
        instances.push(parse_instance(None, object, manager, mumu_version, &args)?);
    } else {
        for (key, value) in object {
            let Some(entry) = value.as_object() else {
                return Err(shape_error(
                    &args,
                    &format!("object for instance entry {key:?}"),
                    value,
                ));
            };
            if let Some(envelope) = errcode_envelope(entry) {
                return Err(DeviceError::fatal(format!(
                    "MuMuManager {} entry {key:?} carries an error envelope: {envelope}",
                    args.join(" ")
                ))
                .with_diagnostic(DeviceErrorCategory::Response, "mumu_manager.entry"));
            }
            instances.push(parse_instance(
                Some(key),
                entry,
                manager,
                mumu_version,
                &args,
            )?);
        }
    }
    instances.sort_by_key(|instance| instance.instance_index);
    Ok(instances)
}

fn errcode_envelope(object: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    let errcode = object.get("errcode")?;
    let errmsg = object
        .get("errmsg")
        .map_or_else(|| "<absent>".to_owned(), serde_json::Value::to_string);
    Some(format!("errcode={errcode} errmsg={errmsg}"))
}

fn parse_instance(
    key: Option<&str>,
    entry: &serde_json::Map<String, serde_json::Value>,
    manager: &ResolvedMumuManager,
    mumu_version: MumuManagerVersion,
    args: &[&str],
) -> DeviceResult<DiscoveredMumuInstance> {
    let entry_label = key.map_or_else(|| "<flat>".to_owned(), |key| format!("{key:?}"));
    let field = |name: &str| {
        entry.get(name).ok_or_else(|| {
            DeviceError::fatal(format!(
                "MuMuManager {} entry {entry_label} is missing field {name:?}",
                args.join(" ")
            ))
            .with_diagnostic(DeviceErrorCategory::Response, "mumu_manager.shape")
        })
    };
    let invalid = |name: &str, expected: &str, value: &serde_json::Value| {
        DeviceError::fatal(format!(
            "MuMuManager {} entry {entry_label} field {name:?} is not {expected}: {value}",
            args.join(" ")
        ))
        .with_diagnostic(DeviceErrorCategory::Response, "mumu_manager.shape")
    };
    let text = |name: &str, max_bytes: usize| -> DeviceResult<String> {
        let value = field(name)?;
        let text = value
            .as_str()
            .filter(|text| {
                !text.is_empty() && text.len() <= max_bytes && !text.chars().any(char::is_control)
            })
            .ok_or_else(|| {
                invalid(
                    name,
                    &format!("a non-empty control-free string of at most {max_bytes} bytes"),
                    value,
                )
            })?;
        Ok(text.to_owned())
    };
    let boolean = |name: &str| -> DeviceResult<bool> {
        let value = field(name)?;
        value
            .as_bool()
            .ok_or_else(|| invalid(name, "a boolean", value))
    };
    let index_value = field("index")?;
    let instance_index = index_value
        .as_str()
        .and_then(|text| text.parse::<u16>().ok())
        .ok_or_else(|| invalid("index", "a string holding a 16-bit index", index_value))?;
    if let Some(key) = key
        && key != instance_index.to_string()
    {
        return Err(invalid(
            "index",
            &format!("consistent with its map key {key:?}"),
            index_value,
        ));
    }
    let instance_name = text("name", MAX_MUMU_INSTANCE_NAME_BYTES)?;
    let adb_host = text("adb_host_ip", MAX_MUMU_ADB_HOST_BYTES)?;
    let port_value = field("adb_port")?;
    let adb_port = port_value
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port != 0)
        .ok_or_else(|| invalid("adb_port", "a non-zero 16-bit port number", port_value))?;
    let process_started = boolean("is_process_started")?;
    let android_started = boolean("is_android_started")?;
    let player_state = text("player_state", MAX_MUMU_PLAYER_STATE_BYTES)?;
    Ok(DiscoveredMumuInstance {
        install_root: manager.install_root.clone(),
        mumu_manager_path: manager.mumu_manager_path.clone(),
        adb_path: manager.adb_path.clone(),
        instance_index,
        instance_name,
        adb_host,
        adb_port,
        running: process_started && android_started,
        player_state,
        mumu_version,
    })
}

fn shape_error(args: &[&str], expected: &str, value: &serde_json::Value) -> DeviceError {
    let mut rendered = value.to_string();
    if rendered.len() > 512 {
        let mut end = 512;
        while !rendered.is_char_boundary(end) {
            end -= 1;
        }
        rendered.truncate(end);
        rendered.push_str("...");
    }
    DeviceError::fatal(format!(
        "MuMuManager {} output is not {expected}: {rendered}",
        args.join(" ")
    ))
    .with_diagnostic(DeviceErrorCategory::Response, "mumu_manager.shape")
}

/// Runs one read-only subcommand, requires exit 0 and strict UTF-8 stdout, and parses JSON.
fn run_json(mumu_manager_path: &Path, args: &[&str]) -> DeviceResult<serde_json::Value> {
    let path = mumu_manager_path.to_str().ok_or_else(|| {
        DeviceError::fatal(format!(
            "MuMuManager path is not valid UTF-8: {}",
            mumu_manager_path.display()
        ))
        .with_diagnostic(DeviceErrorCategory::BackendLaunch, "mumu_manager.path")
    })?;
    let output = run_raw_with_timeout(
        MUMU_MANAGER_PROGRAM,
        path,
        args,
        MUMU_MANAGER_COMMAND_TIMEOUT,
    )
    .map_err(|error| {
        error.with_diagnostic_if_absent(DeviceErrorCategory::BackendLaunch, "mumu_manager.run")
    })?;
    if output.stdout.len() > MAX_MUMU_MANAGER_OUTPUT_BYTES {
        return Err(DeviceError::fatal(format!(
            "MuMuManager {} produced {} stdout bytes, over the {MAX_MUMU_MANAGER_OUTPUT_BYTES} byte bound",
            args.join(" "),
            output.stdout.len()
        ))
        .with_diagnostic(DeviceErrorCategory::Response, "mumu_manager.output_bound"));
    }
    let stdout = decode_adb_text(output.stdout, "stdout", args);
    let stderr = decode_adb_text(output.stderr, "stderr", args);
    if !output.status.success() {
        let envelope = serde_json::from_str::<serde_json::Value>(stdout.text.trim())
            .ok()
            .and_then(|value| value.as_object().and_then(errcode_envelope))
            .unwrap_or_else(|| "no errcode envelope".to_owned());
        return Err(DeviceError::fatal(format!(
            "MuMuManager {} failed with {} ({envelope})\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            output.status,
            stdout.diagnostic_text(),
            stderr.diagnostic_text()
        ))
        .with_diagnostic(DeviceErrorCategory::ChildExit, "mumu_manager.exit"));
    }
    if stdout.lossy {
        return Err(DeviceError::fatal(format!(
            "MuMuManager {} stdout is not valid UTF-8: {}",
            args.join(" "),
            stdout.diagnostic_text()
        ))
        .with_diagnostic(DeviceErrorCategory::Response, "mumu_manager.decode"));
    }
    serde_json::from_str(stdout.text.trim_start_matches('\u{feff}').trim()).map_err(|error| {
        DeviceError::fatal(format!(
            "MuMuManager {} stdout is not JSON: {error}\nstdout:\n{}",
            args.join(" "),
            stdout.text
        ))
        .with_diagnostic(DeviceErrorCategory::Protocol, "mumu_manager.json")
    })
}

/// One `MuMuPlayer*` entry under the standard Windows uninstall keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegistryUninstallEntry {
    pub(crate) key_path: String,
    pub(crate) install_root: PathBuf,
    pub(crate) display_version: Option<String>,
}

/// Reads only the standard uninstall values `InstallLocation`, `DisplayIcon` and
/// `DisplayVersion` of `MuMuPlayer*` subkeys. `HKLM\SOFTWARE\Netease` and
/// `HKCU\SOFTWARE\Netease` carry per-user identifiers and are never opened.
#[cfg(windows)]
mod registry {
    use super::RegistryUninstallEntry;
    use crate::{
        DeviceError, DeviceErrorCategory, DeviceResult, MumuInstallSource, NemuResolutionContext,
        NemuResolutionReason,
    };
    use std::path::{Path, PathBuf};
    use std::ptr;
    use windows_sys::Win32::Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, WIN32_ERROR,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_EXPAND_SZ, REG_SZ,
        REG_VALUE_TYPE, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW,
    };

    const UNINSTALL_KEYS: [(&str, &str); 3] = [
        (
            "HKLM",
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (
            "HKLM",
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (
            "HKCU",
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
    ];
    const MAX_KEY_NAME_CHARS: usize = 256;
    const MAX_VALUE_BYTES: u32 = 32 * 1024;

    struct OpenKey(HKEY);

    impl Drop for OpenKey {
        fn drop(&mut self) {
            // SAFETY: the handle was returned by a successful RegOpenKeyExW and is closed once.
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn hive(label: &str) -> HKEY {
        if label == "HKLM" {
            HKEY_LOCAL_MACHINE
        } else {
            HKEY_CURRENT_USER
        }
    }

    fn registry_error(stage: &'static str, key_path: &str, code: WIN32_ERROR) -> DeviceError {
        DeviceError::fatal(format!(
            "Windows registry {stage} failed for {key_path}: win32 error {code}"
        ))
        .with_diagnostic(DeviceErrorCategory::Native, "mumu_manager.registry")
    }

    fn entry_invalid(key_path: &str, detail: String) -> DeviceError {
        DeviceError::fatal(format!(
            "registry uninstall entry {key_path} is not usable: {detail}"
        ))
        .with_nemu_resolution_context_if_absent(
            NemuResolutionContext::new(NemuResolutionReason::RegistryEntryInvalid)
                .with_source(MumuInstallSource::RegistryUninstall),
        )
    }

    fn open(parent: HKEY, sub_key: &str, key_path: &str) -> DeviceResult<Option<OpenKey>> {
        let mut handle: HKEY = ptr::null_mut();
        // SAFETY: the subkey is NUL-terminated UTF-16 and the out pointer is valid.
        let status =
            unsafe { RegOpenKeyExW(parent, wide(sub_key).as_ptr(), 0, KEY_READ, &mut handle) };
        match status {
            ERROR_SUCCESS => Ok(Some(OpenKey(handle))),
            ERROR_FILE_NOT_FOUND => Ok(None),
            code => Err(registry_error("open", key_path, code)),
        }
    }

    fn sub_key_names(key: &OpenKey, key_path: &str) -> DeviceResult<Vec<String>> {
        let mut names = Vec::new();
        let mut index = 0_u32;
        loop {
            let mut buffer = [0_u16; MAX_KEY_NAME_CHARS];
            let mut length = buffer.len() as u32;
            // SAFETY: the buffer and length pointer are valid for the call; unused outputs are null.
            let status = unsafe {
                RegEnumKeyExW(
                    key.0,
                    index,
                    buffer.as_mut_ptr(),
                    &mut length,
                    ptr::null(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            };
            match status {
                ERROR_SUCCESS => {}
                ERROR_NO_MORE_ITEMS => return Ok(names),
                code => return Err(registry_error("enumerate", key_path, code)),
            }
            let name = String::from_utf16(&buffer[..length as usize]).map_err(|_| {
                registry_error(
                    "enumerate (non-UTF-16 subkey name)",
                    key_path,
                    ERROR_SUCCESS,
                )
            })?;
            names.push(name);
            index += 1;
        }
    }

    /// Reads one `REG_SZ`/`REG_EXPAND_SZ` value; any other type is a typed shape error.
    fn string_value(key: &OpenKey, key_path: &str, name: &str) -> DeviceResult<Option<String>> {
        let wide_name = wide(name);
        let mut value_type: REG_VALUE_TYPE = 0;
        let mut size = 0_u32;
        // SAFETY: a null data pointer requests the size only; the other pointers are valid.
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                wide_name.as_ptr(),
                ptr::null(),
                &mut value_type,
                ptr::null_mut(),
                &mut size,
            )
        };
        match status {
            ERROR_SUCCESS => {}
            ERROR_FILE_NOT_FOUND => return Ok(None),
            code => return Err(registry_error("query", key_path, code)),
        }
        if value_type != REG_SZ && value_type != REG_EXPAND_SZ {
            return Err(entry_invalid(
                key_path,
                format!(
                    "value {name:?} has registry type {value_type}, expected REG_SZ or REG_EXPAND_SZ"
                ),
            ));
        }
        if size > MAX_VALUE_BYTES {
            return Err(entry_invalid(
                key_path,
                format!("value {name:?} is {size} bytes, over the {MAX_VALUE_BYTES} byte bound"),
            ));
        }
        let mut buffer = vec![0_u16; (size as usize).div_ceil(2)];
        let mut written = size;
        // SAFETY: the buffer holds `size` bytes and `written` starts at that capacity.
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                wide_name.as_ptr(),
                ptr::null(),
                &mut value_type,
                buffer.as_mut_ptr().cast::<u8>(),
                &mut written,
            )
        };
        if status != ERROR_SUCCESS || written > size {
            return Err(entry_invalid(
                key_path,
                format!("value {name:?} changed while being read (status {status})"),
            ));
        }
        buffer.truncate((written as usize) / 2);
        while buffer.last() == Some(&0) {
            buffer.pop();
        }
        let text = String::from_utf16(&buffer)
            .map_err(|_| entry_invalid(key_path, format!("value {name:?} is not UTF-16")))?;
        Ok(Some(text))
    }

    fn install_root_from(entry: &OpenKey, key_path: &str) -> DeviceResult<PathBuf> {
        if let Some(location) = string_value(entry, key_path, "InstallLocation")?
            .map(|value| value.trim().trim_matches('"').to_owned())
            .filter(|value| !value.is_empty())
        {
            return Ok(PathBuf::from(location));
        }
        let icon = string_value(entry, key_path, "DisplayIcon")?
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                entry_invalid(
                    key_path,
                    "InstallLocation and DisplayIcon are both empty".into(),
                )
            })?;
        let icon = icon.trim_matches('"');
        let icon_path = match icon.rsplit_once(',') {
            Some((path, suffix)) if suffix.bytes().all(|byte| byte.is_ascii_digit()) => path,
            _ => icon,
        };
        Path::new(icon_path)
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .ok_or_else(|| {
                entry_invalid(
                    key_path,
                    format!("DisplayIcon {icon:?} has no parent directory"),
                )
            })
    }

    pub(super) fn mumu_uninstall_entries() -> DeviceResult<Vec<RegistryUninstallEntry>> {
        let mut entries = Vec::new();
        for (hive_label, uninstall) in UNINSTALL_KEYS {
            let uninstall_path = format!(r"{hive_label}\{uninstall}");
            let Some(parent) = open(hive(hive_label), uninstall, &uninstall_path)? else {
                continue;
            };
            for name in sub_key_names(&parent, &uninstall_path)? {
                if !name.to_ascii_lowercase().starts_with("mumuplayer") {
                    continue;
                }
                let key_path = format!(r"{uninstall_path}\{name}");
                let Some(entry) = open(parent.0, &name, &key_path)? else {
                    continue;
                };
                let install_root = install_root_from(&entry, &key_path)?;
                let display_version = string_value(&entry, &key_path, "DisplayVersion")?
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty());
                entries.push(RegistryUninstallEntry {
                    key_path,
                    install_root,
                    display_version,
                });
            }
        }
        Ok(entries)
    }
}

#[cfg(not(windows))]
mod registry {
    use super::RegistryUninstallEntry;
    use crate::{
        DeviceError, DeviceResult, MumuInstallSource, NemuResolutionContext, NemuResolutionReason,
    };

    pub(super) fn mumu_uninstall_entries() -> DeviceResult<Vec<RegistryUninstallEntry>> {
        Err(DeviceError::fatal(
            "the Windows uninstall registry source is unavailable on this platform",
        )
        .with_nemu_resolution_context_if_absent(
            NemuResolutionContext::new(NemuResolutionReason::RegistrySourceUnavailable)
                .with_source(MumuInstallSource::RegistryUninstall),
        ))
    }
}
