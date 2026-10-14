// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #398 RD-1b (model-398 v1.1 §3.1, §3.2, §3.6): an instance's device kind and the
//! handsets' bindings.
//!
//! - **Device kind.** [`check_device`] checks an instance's `device` member against the fields
//!   the instance names: a handset names no endpoint, serial or discovery key, uses USB, and
//!   selects only the backends a handset has.
//! - **Bindings.** [`bind_handsets`] reads the private binding file once, through the contract's
//!   reader, when the configuration is assembled (start and check-config), and only when the
//!   configuration has a handset. A handset without a binding is registered unbound (a start
//!   goes on without it; check-config refuses `device_binding_missing`); a file that cannot be
//!   read or breaks its rules refuses the assembly (`device_binding_file_invalid`).
//! - A serial leaves this module only inside the device layer's `DeviceSerial`, which shows its
//!   label `handset:<alias>` wherever it is printed.

use super::InstanceConfig;
use crate::codes::{ActingdCode, DeviceConfigProblem};
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_contract::{
    DeviceBindingProblem, MAX_HANDSET_ALIAS_BYTES, device_bindings_path, read_device_bindings,
};
use actingcommand_device::{CaptureBackendChoice, DeviceKind, DeviceSerial, TouchBackendChoice};
use actingcommand_runtime_host::{HandsetBinding, HandsetEndpoint};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// `device.kind` of an emulator (also the kind of an instance without `device`).
const KIND_EMULATOR: &str = "emulator";
/// `device.kind` of a handset, a real phone.
const KIND_HANDSET: &str = "handset";
/// `device.transport` of a handset on USB, the one transport of R1 (`tcp` is reserved, §9).
const TRANSPORT_USB: &str = "usb";

/// An instance entry's `device` member (§3.1). `transport` is a handset's only; absent, a
/// handset is on USB.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InstanceDeviceConfigFile {
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transport: Option<String>,
}

/// A refused device declaration: the instance and what is wrong (`device_config_invalid`).
pub(crate) struct DeviceConfigRefusal {
    alias: String,
    problem: DeviceConfigProblem,
}

/// A refused private binding file: its path and what is wrong (`device_binding_file_invalid`).
pub(crate) struct DeviceBindingRefusal {
    path: PathBuf,
    problem: DeviceBindingProblem,
}

impl DeviceBindingRefusal {
    pub(crate) fn code(&self) -> Code {
        ActingdCode::DeviceBindingFileInvalid.code()
    }
}

/// A device refusal of the assembly, with what `check-config` reports as `error.detail` and the
/// start writes to its developer line.
pub(crate) enum DeviceRefusal {
    Config(DeviceConfigRefusal),
    BindingFile(DeviceBindingRefusal),
    /// A handset the private binding file binds no device to (`device_binding_missing`):
    /// refused by `check-config` only.
    Missing(String),
}

impl DeviceRefusal {
    pub(crate) fn code(&self) -> Code {
        match self {
            Self::Config(_) => ActingdCode::DeviceConfigInvalid.code(),
            Self::BindingFile(refused) => refused.code(),
            Self::Missing(_) => ActingdCode::DeviceBindingMissing.code(),
        }
    }

    /// Workflow #378 A6a (model section 7.4): the refusal with the instance or the file it
    /// names and why, as `check-config` and a start report it.
    pub(crate) fn outcome(&self) -> Outcome {
        let outcome = Outcome::new(self.code());
        match self {
            Self::Config(refused) => outcome
                .alias(&refused.alias)
                .device_config_problem(refused.problem),
            Self::BindingFile(refused) => outcome
                .path(&refused.path)
                .device_binding_problem(refused.problem),
            Self::Missing(alias) => outcome.alias(alias),
        }
    }
}

impl fmt::Display for DeviceRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(refused) => write!(
                formatter,
                "instance {:?} device {}",
                refused.alias, refused.problem
            ),
            Self::BindingFile(refused) => write!(
                formatter,
                "private binding file {:?}: {}",
                refused.path.to_string_lossy(),
                refused.problem
            ),
            Self::Missing(alias) => write!(
                formatter,
                "instance {alias:?} is a handset the private binding file binds no device to"
            ),
        }
    }
}

/// The device kind of a device instance (not a fixture), once its `device` member fits the
/// fields it names (§3.1, §5.1, coordinator ruling 10-13 on the alias).
pub(super) fn check_device(instance: &InstanceConfig) -> Result<DeviceKind, DeviceConfigRefusal> {
    let refused = |problem| DeviceConfigRefusal {
        alias: instance.alias.clone(),
        problem,
    };
    let (kind, transport) = match &instance.device {
        None => (DeviceKind::Emulator, None),
        Some(device) => (
            match device.kind.as_str() {
                KIND_EMULATOR => DeviceKind::Emulator,
                KIND_HANDSET => DeviceKind::Handset,
                _ => return Err(refused(DeviceConfigProblem::KindUnknown)),
            },
            device.transport.as_deref(),
        ),
    };
    if !matches!(
        (kind, transport),
        (DeviceKind::Emulator, None) | (DeviceKind::Handset, None | Some(TRANSPORT_USB))
    ) {
        return Err(refused(DeviceConfigProblem::TransportUnsupported));
    }
    if kind == DeviceKind::Handset {
        if instance.alias.len() > MAX_HANDSET_ALIAS_BYTES {
            return Err(refused(DeviceConfigProblem::AliasTooLong));
        }
        if instance.serial.is_some() {
            return Err(refused(DeviceConfigProblem::SerialInConfig));
        }
        if instance.host.is_some() || instance.port.is_some() || instance.connect.is_some() {
            return Err(refused(DeviceConfigProblem::EndpointWithHandset));
        }
        if instance.instance_index.is_some()
            || instance.instance_name.is_some()
            || instance.nemu_app_index.is_some()
        {
            return Err(refused(DeviceConfigProblem::DiscoveryKeyWithHandset));
        }
    }
    // A backend name that does not parse is refused by its own code later; one that parses
    // must be a backend this kind of device has.
    let touch = instance
        .touch_backend
        .as_deref()
        .and_then(|name| TouchBackendChoice::parse(name).ok());
    let capture = instance
        .capture_backend
        .as_deref()
        .and_then(|name| CaptureBackendChoice::parse(name).ok());
    if touch.is_some_and(|choice| !choice.allowed_on(kind))
        || capture.is_some_and(|choice| !choice.allowed_on(kind))
    {
        return Err(refused(DeviceConfigProblem::BackendKindUnsupported));
    }
    Ok(kind)
}

/// The handsets' bindings from the private binding file (§3.2, §3.6).
pub(super) struct HandsetBindings {
    /// Per handset alias: its device, or unbound when the file has no binding for it.
    pub(super) endpoints: BTreeMap<String, HandsetEndpoint>,
    /// The handsets without a binding, in configuration order.
    pub(super) unbound: Vec<String>,
    /// The aliases the file binds that no handset of the configuration has, in file order.
    pub(super) unused: Vec<String>,
}

/// Reads the private binding file of `state_root` once and binds each handset of `handsets`
/// (aliases, in configuration order) to its device. Each bound serial is the device layer's
/// `DeviceSerial`, labelled `handset:<alias>`.
pub(super) fn bind_handsets(
    state_root: &Path,
    handsets: &[String],
) -> Result<HandsetBindings, DeviceBindingRefusal> {
    let bindings = read_device_bindings(state_root).map_err(|problem| DeviceBindingRefusal {
        path: device_bindings_path(state_root),
        problem,
    })?;
    let mut endpoints = BTreeMap::new();
    let mut unbound = Vec::new();
    for alias in handsets {
        let endpoint = match bindings.get(alias) {
            Some(binding) => HandsetEndpoint::Bound(HandsetBinding::new(
                DeviceSerial::handset(binding.serial(), alias),
                binding.bound_at_unix_ms,
                binding.bound_via,
            )),
            None => {
                unbound.push(alias.clone());
                HandsetEndpoint::Unbound
            }
        };
        endpoints.insert(alias.clone(), endpoint);
    }
    let unused = bindings
        .iter()
        .filter(|binding| !handsets.contains(&binding.alias))
        .map(|binding| binding.alias.clone())
        .collect();
    Ok(HandsetBindings {
        endpoints,
        unbound,
        unused,
    })
}

#[cfg(test)]
mod tests {
    // Workflow #398 RD-1b (model-398 v1.1 §12, rows T1, T5, T6 and T9): a handset instance's
    // device declaration, its binding from the private binding file, and an emulator-only
    // configuration that RD-1b leaves as it was. The serials are neutral fakes, distinct from the
    // other actingd tests' so the process-wide label of each stays its own.
    use super::super::*;
    use super::*;
    use actingcommand_contract::{
        DEVICE_BINDINGS_SCHEMA, DeviceBoundVia, EventPayload, EventQuery, EventType, FactScalar,
        IdentifierIssuer, RuntimePayload,
    };
    use actingcommand_ledger::{GlobalLedger, GlobalLedgerEvidenceConfig};
    use actingcommand_runtime_host::{ResolvedInstanceEndpoint, RuntimeHost};
    use serde_json::{Value, json};
    use tempfile::TempDir;

    const ALIAS: &str = "neutral.handset";
    const BOUND_AT_UNIX_MS: u64 = 1_700_000_000_000;

    fn instance_id() -> InstanceId {
        *IdentifierIssuer::new()
            .expect("issuer")
            .mint_instance_id()
            .expect("instance id")
            .transport()
    }

    /// A handset instance on USB, with the backends a handset has.
    fn handset_instance(instance_id: InstanceId) -> Value {
        json!({
            "alias": ALIAS,
            "instance_id": instance_id,
            "application_id": "neutral.application",
            "adb_path": "neutral-adb",
            "touch_backend": "adb_shell_input",
            "capture_backend": "adb",
            "device": {"kind": "handset", "transport": "usb"}
        })
    }

    fn config(state_root: &Path, instance: Value) -> ActingdConfigFile {
        serde_json::from_value(json!({
            "schema_version": CONFIG_SCHEMA_VERSION,
            "state_root": state_root,
            "bind_host": "127.0.0.1",
            "bind_port": 0,
            "secret_fingerprint_salt": "0123456789abcdef",
            "instances": [instance]
        }))
        .expect("typed config")
    }

    fn binding(alias: &str, serial: &str) -> Value {
        json!({
            "alias": alias,
            "serial": serial,
            "bound_at_unix_ms": BOUND_AT_UNIX_MS,
            "bound_via": "cli"
        })
    }

    fn write_private_file(state_root: &Path, bytes: &[u8]) {
        let path = device_bindings_path(state_root);
        std::fs::create_dir_all(path.parent().expect("the private directory"))
            .expect("create the private directory");
        std::fs::write(&path, bytes).expect("write the private binding file");
    }

    fn write_bindings(state_root: &Path, bindings: Vec<Value>) {
        let file = json!({"schema": DEVICE_BINDINGS_SCHEMA, "bindings": bindings});
        write_private_file(
            state_root,
            &serde_json::to_vec(&file).expect("binding file"),
        );
    }

    /// T1 (§3.1, §3.4, §11.2): a handset bound in the private binding file is addressed by that
    /// serial on both its targets, never `adb connect`ed, and labelled `handset:<alias>`; the
    /// registry keeps the binding's time and channel; the manifest names the instance's device
    /// kind and never the serial.
    #[test]
    fn rd1b_t1_a_bound_handset_targets_its_serial_and_never_connects() {
        const SERIAL: &str = "TESTSERIAL0011";
        let root = TempDir::new().expect("tempdir");
        write_bindings(root.path(), vec![binding(ALIAS, SERIAL)]);
        let id = instance_id();
        let assembly = config(root.path(), handset_instance(id))
            .assemble()
            .expect("handset assembly");
        assert!(assembly.unbound_handsets.is_empty());
        assert!(assembly.unused_device_bindings.is_empty());

        let [spec] = assembly.provider.instances.as_slice() else {
            panic!("one registered instance");
        };
        let InstanceMode::Real(registration) = spec.mode() else {
            panic!("a device instance");
        };
        for target in [registration.input_target(), registration.capture_target()] {
            assert_eq!(target.adb_arg(), SERIAL);
            assert!(!target.connect, "a handset on USB is never adb-connected");
            assert_eq!(target.label(), "handset:neutral.handset");
        }

        let instance = manifest::instance_id_text(id).expect("instance id text");
        let kind = assembly
            .manifest
            .parameters
            .iter()
            .find(|parameter| parameter.key == format!("instance.{instance}.device_kind"))
            .expect("the instance's device kind");
        assert_eq!(kind.value, FactScalar::String("handset".to_owned()));
        let manifest = serde_json::to_string(&assembly.manifest).expect("manifest JSON");
        assert!(!manifest.contains(SERIAL));

        let registry = assembly
            .provider
            .into_registry()
            .expect("configured registry");
        let resolved = registry.resolve(ALIAS).expect("the registered handset");
        let endpoint = resolved
            .adb_endpoint()
            .and_then(ResolvedInstanceEndpoint::bound)
            .expect("a bound endpoint");
        assert!(endpoint.serial_configured());
        let bound = endpoint
            .handset()
            .and_then(HandsetEndpoint::binding)
            .expect("a bound handset");
        assert_eq!(bound.serial().adb_arg(), SERIAL);
        assert_eq!(
            (bound.bound_at_unix_ms(), bound.bound_via()),
            (BOUND_AT_UNIX_MS, DeviceBoundVia::Cli)
        );
    }

    /// T5 (§3.6): a private binding file that is not JSON of its shape, that binds an alias
    /// twice or a serial twice refuses the assembly, so the start, with
    /// `device_binding_file_invalid` and the file's problem.
    #[test]
    fn rd1b_t5_a_broken_private_binding_file_refuses_the_start() {
        let file = |bindings: Vec<Value>| {
            serde_json::to_vec(&json!({"schema": DEVICE_BINDINGS_SCHEMA, "bindings": bindings}))
                .expect("binding file")
        };
        for (bytes, problem) in [
            (b"{not json".to_vec(), DeviceBindingProblem::Malformed),
            (
                file(vec![
                    binding(ALIAS, "TESTSERIAL0011"),
                    binding(ALIAS, "TESTSERIAL0012"),
                ]),
                DeviceBindingProblem::DuplicateAlias,
            ),
            (
                file(vec![
                    binding(ALIAS, "TESTSERIAL0011"),
                    binding("neutral.other", "TESTSERIAL0011"),
                ]),
                DeviceBindingProblem::DuplicateSerial,
            ),
        ] {
            let root = TempDir::new().expect("tempdir");
            write_private_file(root.path(), &bytes);
            let config = config(root.path(), handset_instance(instance_id()));
            let refused = config.device_refusal().expect("a device refusal");
            assert_eq!(
                refused.code(),
                ActingdCode::DeviceBindingFileInvalid.code(),
                "{problem}"
            );
            // Workflow #378 A6a: the refusal's outcome names the file and why (it was v1's
            // `error.detail`).
            assert_eq!(
                serde_json::Value::Object(refused.outcome().values().clone()),
                json!({
                    "path": device_bindings_path(root.path()).to_string_lossy(),
                    "device_binding_problem": problem.as_str(),
                })
            );
            assert_eq!(
                config.assemble().err().map(Code::as_str),
                Some(ActingdCode::DeviceBindingFileInvalid.as_str()),
                "{problem}"
            );
        }
    }

    /// T6 (§3.1, §5.1, coordinator ruling 10-13): a handset that names a serial, an endpoint or
    /// a discovery key, selects DroidCast or Nemu IPC capture, names the reserved `tcp`
    /// transport, or has an alias longer than 248 bytes is refused as `device_config_invalid`
    /// with its problem.
    #[test]
    fn rd1b_t6_device_fields_that_do_not_fit_a_handset_are_refused() {
        let long_alias = "a".repeat(MAX_HANDSET_ALIAS_BYTES + 1);
        let id = instance_id();
        for (member, value, problem) in [
            (
                "serial",
                json!("TESTSERIAL0012"),
                DeviceConfigProblem::SerialInConfig,
            ),
            (
                "host",
                json!("127.0.0.1"),
                DeviceConfigProblem::EndpointWithHandset,
            ),
            (
                "port",
                json!(16384),
                DeviceConfigProblem::EndpointWithHandset,
            ),
            (
                "connect",
                json!(false),
                DeviceConfigProblem::EndpointWithHandset,
            ),
            (
                "instance_index",
                json!(0),
                DeviceConfigProblem::DiscoveryKeyWithHandset,
            ),
            (
                "nemu_app_index",
                json!(0),
                DeviceConfigProblem::DiscoveryKeyWithHandset,
            ),
            (
                "capture_backend",
                json!("droidcast_raw"),
                DeviceConfigProblem::BackendKindUnsupported,
            ),
            (
                "capture_backend",
                json!("nemu_ipc"),
                DeviceConfigProblem::BackendKindUnsupported,
            ),
            (
                "device",
                json!({"kind": "handset", "transport": "tcp"}),
                DeviceConfigProblem::TransportUnsupported,
            ),
            (
                "alias",
                json!(long_alias),
                DeviceConfigProblem::AliasTooLong,
            ),
        ] {
            let root = TempDir::new().expect("tempdir");
            let mut instance = handset_instance(id);
            instance[member] = value;
            let alias = instance["alias"].clone();
            let config = config(root.path(), instance);
            let refused = config.device_refusal().expect("a device refusal");
            assert_eq!(
                serde_json::Value::Object(refused.outcome().values().clone()),
                json!({"alias": alias, "device_config_problem": problem.as_str()}),
                "{member}"
            );
            assert_eq!(
                config.assemble().err().map(Code::as_str),
                Some(ActingdCode::DeviceConfigInvalid.as_str()),
                "{member}"
            );
        }
    }

    /// T9 (§12): an emulator-only configuration assembles as before RD-1b: no device row in the
    /// manifest, no binding read or reported, its target addressed by HOST:PORT, and its
    /// `runtime.instance_bound` with exactly the members it had (HOST:PORT, no device).
    #[test]
    fn rd1b_t9_an_emulator_only_configuration_assembles_and_binds_as_before() {
        let root = TempDir::new().expect("tempdir");
        let id = instance_id();
        let assembly = config(
            root.path(),
            json!({
                "alias": "neutral.emulator",
                "instance_id": id,
                "application_id": "neutral.application",
                "adb_path": root.path().join("missing-adb.exe"),
                "port": 16416,
                "touch_backend": "adb_shell_input",
                "capture_backend": "adb"
            }),
        )
        .assemble()
        .expect("emulator assembly");
        assert!(assembly.unbound_handsets.is_empty());
        assert!(assembly.unused_device_bindings.is_empty());
        assert!(
            assembly
                .manifest
                .parameters
                .iter()
                .all(|parameter| !parameter.key.ends_with(".device_kind")),
            "an emulator-only configuration reports no device kind"
        );
        let [spec] = assembly.provider.instances.as_slice() else {
            panic!("one registered instance");
        };
        let InstanceMode::Real(registration) = spec.mode() else {
            panic!("a device instance");
        };
        for target in [registration.input_target(), registration.capture_target()] {
            assert!(target.serial.is_none());
            assert!(target.connect);
            assert_eq!(target.label(), "127.0.0.1:16416");
        }

        let registry = assembly
            .provider
            .into_registry()
            .expect("configured registry");
        let host = RuntimeHost::start(assembly.host, Arc::new(registry)).expect("runtime host");
        host.close().expect("close host");
        let ledger =
            GlobalLedger::open_evidence(GlobalLedgerEvidenceConfig::new(root.path()), |_| None)
                .expect("read the ledger");
        let bindings = ledger
            .query(&EventQuery {
                event_type: Some(EventType::RuntimeInstanceBound),
                ..EventQuery::default()
            })
            .into_iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Runtime(RuntimePayload::InstanceBound(binding)) => {
                    Some(serde_json::to_value(binding).expect("binding JSON"))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let [binding] = bindings.as_slice() else {
            panic!("one instance binding: {bindings:?}");
        };
        let members = binding
            .as_object()
            .expect("a binding object")
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            members,
            BTreeSet::from([
                "action",
                "instance_alias",
                "provenance",
                "adb_host",
                "adb_port",
                "serial_configured",
                "binding_source",
                "audit",
            ])
        );
        assert_eq!(
            (
                &binding["adb_host"],
                &binding["adb_port"],
                &binding["serial_configured"],
                &binding["binding_source"],
            ),
            (
                &json!("127.0.0.1"),
                &json!(16416),
                &json!(false),
                &json!("explicit")
            )
        );
    }
}
