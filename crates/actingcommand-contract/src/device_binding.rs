// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #398 RD-1b (model-398 v1.1 §3.1, §3.2, §3.4): the device kind of an instance and
//! the private device-binding file (`contracts/device-binding.md`).
//!
//! - **Device kind.** An instance entry's optional `device` member names the kind of device the
//!   instance drives: `emulator` (also when the member is absent) or `handset`, a real phone on
//!   USB. Only a handset names a `transport`; `usb` is the one transport of R1.
//! - **Private binding file.** `<state root>\private\device-bindings.json`
//!   (`actingcommand.device-bindings.v1`) maps an instance alias to the adb serial of the
//!   handset it drives. It is the one place a device serial lives: never a member of the actingd
//!   configuration, a configuration generation, an export, an archive or a record. A person and
//!   `actingctl devices bind|unbind` (RD-1c) write it; the Runtime only reads it, once, when it
//!   assembles its configuration (start and check-config).
//! - Nothing here interprets a serial beyond its form, and nothing prints one: a binding's
//!   `Debug` leaves the serial out.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The state root's directory of files that are never exported, archived or recorded.
pub const PRIVATE_DIRECTORY: &str = "private";
/// The private binding file's name in [`PRIVATE_DIRECTORY`].
pub const DEVICE_BINDINGS_FILE: &str = "device-bindings.json";
/// The private binding file's `schema`.
pub const DEVICE_BINDINGS_SCHEMA: &str = "actingcommand.device-bindings.v1";
/// The largest private binding file read, in bytes.
pub const MAX_DEVICE_BINDINGS_FILE_BYTES: u64 = 1024 * 1024;
/// The longest device serial, in bytes (§3.2).
pub const MAX_DEVICE_SERIAL_BYTES: usize = 64;
/// The longest alias of an instance bound to a device (coordinator ruling 10-13): its label
/// `handset:<alias>` stays within the 256 bytes of a `name` value.
pub const MAX_HANDSET_ALIAS_BYTES: usize = 248;

/// The private binding file of the state root `state_root`.
pub fn device_bindings_path(state_root: &Path) -> PathBuf {
    state_root
        .join(PRIVATE_DIRECTORY)
        .join(DEVICE_BINDINGS_FILE)
}

/// The kind of device an instance drives (§3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceDeviceKind {
    Emulator,
    Handset,
}

/// Who wrote a binding (§3.2 `bound_via`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceBoundVia {
    Cli,
    Mcp,
    Ui,
    Manual,
}

crate::outcome_vocabulary! {
    /// What is wrong with the private binding file (vocabulary `device_binding_problem`,
    /// Workflow #398 RD-1b, model-398 §10.1).
    pub enum DeviceBindingProblem: "device_binding_problem" {
        /// The file exists but cannot be read as a file.
        Unreadable => "unreadable",
        /// The file is not JSON of the file's shape and schema, is larger than its bound, or
        /// names an alias no instance could have.
        Malformed => "malformed",
        /// Two bindings name the same alias.
        DuplicateAlias => "duplicate_alias",
        /// Two bindings name the same serial: one device is bound to one instance only.
        DuplicateSerial => "duplicate_serial",
        /// A serial is not 1 to 64 bytes of printable ASCII without white space.
        SerialInvalid => "serial_invalid",
    }
}

/// One binding of the private file: an instance alias and the serial of its handset.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceBinding {
    pub alias: String,
    serial: String,
    pub bound_at_unix_ms: u64,
    pub bound_via: DeviceBoundVia,
}

impl DeviceBinding {
    pub fn new(
        alias: impl Into<String>,
        serial: impl Into<String>,
        bound_at_unix_ms: u64,
        bound_via: DeviceBoundVia,
    ) -> Self {
        Self {
            alias: alias.into(),
            serial: serial.into(),
            bound_at_unix_ms,
            bound_via,
        }
    }

    /// The raw serial: for the adb command line and the record's fingerprint only.
    pub fn serial(&self) -> &str {
        &self.serial
    }
}

impl fmt::Debug for DeviceBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceBinding")
            .field("alias", &self.alias)
            .field("serial", &"<redacted>")
            .field("bound_at_unix_ms", &self.bound_at_unix_ms)
            .field("bound_via", &self.bound_via)
            .finish()
    }
}

/// The private binding file as written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceBindingsFile {
    pub schema: String,
    pub bindings: Vec<DeviceBinding>,
}

/// The checked bindings of one state root, in file order: aliases and serials unique.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceBindings {
    bindings: Vec<DeviceBinding>,
}

impl DeviceBindings {
    /// The binding of `alias`, if the file has one.
    pub fn get(&self, alias: &str) -> Option<&DeviceBinding> {
        self.bindings.iter().find(|binding| binding.alias == alias)
    }

    /// Every binding, in file order.
    pub fn iter(&self) -> impl Iterator<Item = &DeviceBinding> {
        self.bindings.iter()
    }
}

/// Reads the private binding file of `state_root`. An absent file binds nothing; a file that
/// cannot be read, or breaks a rule of [`parse_device_bindings`], is refused with its problem.
pub fn read_device_bindings(state_root: &Path) -> Result<DeviceBindings, DeviceBindingProblem> {
    let path = device_bindings_path(state_root);
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(DeviceBindings::default());
        }
        Err(_) => return Err(DeviceBindingProblem::Unreadable),
    };
    if !metadata.is_file() {
        return Err(DeviceBindingProblem::Unreadable);
    }
    if metadata.len() > MAX_DEVICE_BINDINGS_FILE_BYTES {
        return Err(DeviceBindingProblem::Malformed);
    }
    let bytes = fs::read(&path).map_err(|_| DeviceBindingProblem::Unreadable)?;
    parse_device_bindings(&bytes)
}

/// The rules of the private binding file (§3.2): its schema and shape, every alias one an
/// instance bound to a device can have, every serial 1 to 64 bytes of printable ASCII without
/// white space, and no alias and no serial twice.
pub fn parse_device_bindings(bytes: &[u8]) -> Result<DeviceBindings, DeviceBindingProblem> {
    let file = serde_json::from_slice::<DeviceBindingsFile>(bytes)
        .map_err(|_| DeviceBindingProblem::Malformed)?;
    if file.schema != DEVICE_BINDINGS_SCHEMA {
        return Err(DeviceBindingProblem::Malformed);
    }
    check_bindings(&file.bindings)?;
    Ok(DeviceBindings {
        bindings: file.bindings,
    })
}

/// Every alias one an instance bound to a device can have, every serial of its form, and no
/// alias and no serial twice.
fn check_bindings(bindings: &[DeviceBinding]) -> Result<(), DeviceBindingProblem> {
    let mut aliases = BTreeSet::new();
    let mut serials = BTreeSet::new();
    for binding in bindings {
        if crate::validate_instance_alias(&binding.alias).is_err()
            || binding.alias.len() > MAX_HANDSET_ALIAS_BYTES
        {
            return Err(DeviceBindingProblem::Malformed);
        }
        if !device_serial_valid(&binding.serial) {
            return Err(DeviceBindingProblem::SerialInvalid);
        }
        if !aliases.insert(binding.alias.as_str()) {
            return Err(DeviceBindingProblem::DuplicateAlias);
        }
        if !serials.insert(binding.serial.as_str()) {
            return Err(DeviceBindingProblem::DuplicateSerial);
        }
    }
    Ok(())
}

/// Whether `serial` has the form of a device serial (§3.2).
pub fn device_serial_valid(serial: &str) -> bool {
    (1..=MAX_DEVICE_SERIAL_BYTES).contains(&serial.len())
        && serial.bytes().all(|byte| byte.is_ascii_graphic())
}
