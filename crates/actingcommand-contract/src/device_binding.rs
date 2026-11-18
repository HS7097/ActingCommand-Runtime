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
//! - Workflow #479 F-2 (`contracts/device-binding.md`, "An iOS handset"): a handset's device
//!   system ([`DeviceSystem`], `device.system`: `android`, also what an absent member means, or
//!   `ios`), and the iOS row of the private file: the alias, `system` `ios` and the phone's
//!   `device_key`, the fingerprint `acib` made of the phone's identifier under the installation's
//!   own secret, never the identifier. A file with an iOS row is `actingcommand.device-bindings.v2`;
//!   a file without one is written and read as before, version 1, byte for byte.

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
/// Workflow #479 F-2: the private binding file's `schema` once it holds an iOS handset's row, a
/// kind of row a reader of version 1 refuses (its rows deny unknown members). A file without one
/// keeps [`DEVICE_BINDINGS_SCHEMA`] ([`device_bindings_schema`]).
pub const DEVICE_BINDINGS_SCHEMA_V2: &str = "actingcommand.device-bindings.v2";
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

/// Workflow #479 F-2: the device system of a handset, an instance entry's `device.system` (a
/// handset's member only): `android`, also what an absent member means, or `ios`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceSystem {
    Android,
    Ios,
}

impl DeviceSystem {
    /// The configuration's word.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Android => "android",
            Self::Ios => "ios",
        }
    }
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
        /// Two bindings name the same device: one device is bound to one instance only (two
        /// Android rows of one serial, or, Workflow #479 F-2, two iOS rows of one device key).
        DuplicateSerial => "duplicate_serial",
        /// A serial is not 1 to 64 bytes of printable ASCII without white space.
        SerialInvalid => "serial_invalid",
        /// Workflow #479 F-2: an iOS row's `device_key` is not `sha256:` and 64 lowercase
        /// hexadecimal digits.
        DeviceKeyInvalid => "device_key_invalid",
        /// Workflow #479 F-2: the row of a handset's alias is of another device system than the
        /// handset's configuration names (an iOS row for an Android handset, or the reverse).
        SystemMismatch => "system_mismatch",
    }
}

crate::outcome_vocabulary! {
    /// The state of a handset's wired device link (vocabulary `device_link_state`, Workflow
    /// #398 RD-2, model-398 §4.2, §10.1). The first six are what the adb server reports for the
    /// bound serial; the last two are the Runtime's own.
    pub enum DeviceLinkState: "device_link_state" {
        /// adb reports `device`: the handset answers.
        Device => "device",
        /// The bound serial is not in adb's device list (the cable is out).
        Absent => "absent",
        /// adb reports `offline`.
        Offline => "offline",
        /// adb reports `unauthorized`: a person allows USB debugging on the handset.
        Unauthorized => "unauthorized",
        /// adb reports `no permissions` (a driver problem on the host).
        NoPermissions => "no_permissions",
        /// adb reports another state (`authorizing`, `connecting`, `recovery`, `sideload`,
        /// `bootloader` and the like).
        Other => "other",
        /// The link watch is not running, so the Runtime cannot see the link.
        Unwatched => "unwatched",
        /// The private binding file binds no device to the handset instance.
        Unbound => "unbound",
    }
}

crate::outcome_vocabulary! {
    /// What went wrong with the device-link watch, the one `adb track-devices` process of the
    /// Runtime (vocabulary `device_watch_problem`, Workflow #398 RD-2, model-398 §4.3, §10.1).
    pub enum DeviceWatchProblem: "device_watch_problem" {
        /// The watch process could not be started.
        SpawnFailed => "spawn_failed",
        /// The watch process ended, or its output closed.
        Exited => "exited",
        /// The watch's output has a shape the parser does not know.
        ProtocolInvalid => "protocol_invalid",
        /// The watch ended more often within the restart window than the restart limit allows.
        RestartBudgetSpent => "restart_budget_spent",
    }
}

/// One binding of the private file: an instance alias and the device of its handset, an Android
/// handset's adb serial or (Workflow #479 F-2) an iOS handset's device key.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "DeviceBindingRow", into = "DeviceBindingRow")]
pub struct DeviceBinding {
    pub alias: String,
    device: BoundDevice,
    pub bound_at_unix_ms: u64,
    pub bound_via: DeviceBoundVia,
}

/// Workflow #479 F-2: what a row binds the alias to.
#[derive(Clone, PartialEq, Eq)]
enum BoundDevice {
    /// An Android handset's adb serial.
    Serial(String),
    /// An iOS handset's device key: the fingerprint `acib` made of the phone's identifier under
    /// the installation's own secret (`sha256:` and 64 lowercase hexadecimal digits).
    Key(String),
}

/// Workflow #479 F-2: a row of the file as written: an Android row or an iOS row. Fix round 1:
/// each kind of row is read by its own members and denies every other one, so a version-1 file is
/// read exactly as before this slice (an Android row with any `system` member, `null` included,
/// is no row) and any other mix of members is no row either; the file's reader answers
/// `malformed` for it.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum DeviceBindingRow {
    Android(AndroidBindingRow),
    Ios(IosBindingRow),
}

/// An Android row, `{alias, serial, bound_at_unix_ms, bound_via}`: the row of version 1, member
/// for member and in that order.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AndroidBindingRow {
    alias: String,
    serial: String,
    bound_at_unix_ms: u64,
    bound_via: DeviceBoundVia,
}

/// Workflow #479 F-2: an iOS row, `{alias, system: "ios", device_key, bound_at_unix_ms,
/// bound_via}`.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IosBindingRow {
    alias: String,
    system: IosRowSystem,
    device_key: String,
    bound_at_unix_ms: u64,
    bound_via: DeviceBoundVia,
}

/// Workflow #479 F-2 fix round 1: the one word an iOS row's `system` holds.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum IosRowSystem {
    Ios,
}

impl From<DeviceBindingRow> for DeviceBinding {
    fn from(row: DeviceBindingRow) -> Self {
        match row {
            DeviceBindingRow::Android(row) => Self {
                alias: row.alias,
                device: BoundDevice::Serial(row.serial),
                bound_at_unix_ms: row.bound_at_unix_ms,
                bound_via: row.bound_via,
            },
            DeviceBindingRow::Ios(row) => Self {
                alias: row.alias,
                device: match row.system {
                    IosRowSystem::Ios => BoundDevice::Key(row.device_key),
                },
                bound_at_unix_ms: row.bound_at_unix_ms,
                bound_via: row.bound_via,
            },
        }
    }
}

impl From<DeviceBinding> for DeviceBindingRow {
    fn from(binding: DeviceBinding) -> Self {
        match binding.device {
            BoundDevice::Serial(serial) => Self::Android(AndroidBindingRow {
                alias: binding.alias,
                serial,
                bound_at_unix_ms: binding.bound_at_unix_ms,
                bound_via: binding.bound_via,
            }),
            BoundDevice::Key(device_key) => Self::Ios(IosBindingRow {
                alias: binding.alias,
                system: IosRowSystem::Ios,
                device_key,
                bound_at_unix_ms: binding.bound_at_unix_ms,
                bound_via: binding.bound_via,
            }),
        }
    }
}

impl DeviceBinding {
    /// An Android handset's row: `alias` bound to the adb serial `serial`.
    pub fn new(
        alias: impl Into<String>,
        serial: impl Into<String>,
        bound_at_unix_ms: u64,
        bound_via: DeviceBoundVia,
    ) -> Self {
        Self {
            alias: alias.into(),
            device: BoundDevice::Serial(serial.into()),
            bound_at_unix_ms,
            bound_via,
        }
    }

    /// Workflow #479 F-2: an iOS handset's row: `alias` bound to the phone of `device_key`, the
    /// fingerprint `acib` made of its identifier under the installation's own secret.
    pub fn ios(
        alias: impl Into<String>,
        device_key: impl Into<String>,
        bound_at_unix_ms: u64,
        bound_via: DeviceBoundVia,
    ) -> Self {
        Self {
            alias: alias.into(),
            device: BoundDevice::Key(device_key.into()),
            bound_at_unix_ms,
            bound_via,
        }
    }

    /// The raw serial of an Android row: for the adb command line and the record's fingerprint
    /// only. `None` for an iOS row, which holds no identifier of its phone.
    pub fn serial(&self) -> Option<&str> {
        match &self.device {
            BoundDevice::Serial(serial) => Some(serial),
            BoundDevice::Key(_) => None,
        }
    }

    /// Workflow #479 F-2: the device key of an iOS row; `None` for an Android row.
    pub fn device_key(&self) -> Option<&str> {
        match &self.device {
            BoundDevice::Serial(_) => None,
            BoundDevice::Key(device_key) => Some(device_key),
        }
    }

    /// Workflow #479 F-2: the device system of the row's handset.
    pub const fn system(&self) -> DeviceSystem {
        match self.device {
            BoundDevice::Serial(_) => DeviceSystem::Android,
            BoundDevice::Key(_) => DeviceSystem::Ios,
        }
    }
}

impl fmt::Debug for DeviceBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("DeviceBinding");
        debug.field("alias", &self.alias);
        match &self.device {
            BoundDevice::Serial(_) => debug.field("serial", &"<redacted>"),
            BoundDevice::Key(_) => debug
                .field("system", &DeviceSystem::Ios)
                .field("device_key", &"<redacted>"),
        };
        debug
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
/// white space, and no alias and no serial twice. Workflow #479 F-2: a file of
/// [`DEVICE_BINDINGS_SCHEMA_V2`] may hold iOS rows, each device key of the fingerprint's form
/// (`device_key_invalid`) and none twice (`duplicate_serial`: one device bound twice); a file of
/// [`DEVICE_BINDINGS_SCHEMA`] holds Android rows alone, as before (an iOS row in it is
/// `malformed`).
pub fn parse_device_bindings(bytes: &[u8]) -> Result<DeviceBindings, DeviceBindingProblem> {
    let file = serde_json::from_slice::<DeviceBindingsFile>(bytes)
        .map_err(|_| DeviceBindingProblem::Malformed)?;
    let ios_rows_allowed = match file.schema.as_str() {
        DEVICE_BINDINGS_SCHEMA => false,
        DEVICE_BINDINGS_SCHEMA_V2 => true,
        _ => return Err(DeviceBindingProblem::Malformed),
    };
    if !ios_rows_allowed
        && file
            .bindings
            .iter()
            .any(|binding| binding.system() == DeviceSystem::Ios)
    {
        return Err(DeviceBindingProblem::Malformed);
    }
    check_bindings(&file.bindings)?;
    Ok(DeviceBindings {
        bindings: file.bindings,
    })
}

/// Workflow #479 F-2: the `schema` a file holding `bindings` is written with: version 2 when one
/// of them is an iOS row, version 1 otherwise, so a file of Android rows alone is what it was.
pub fn device_bindings_schema(bindings: &[DeviceBinding]) -> &'static str {
    if bindings
        .iter()
        .any(|binding| binding.system() == DeviceSystem::Ios)
    {
        DEVICE_BINDINGS_SCHEMA_V2
    } else {
        DEVICE_BINDINGS_SCHEMA
    }
}

/// Every alias one an instance bound to a device can have, every serial and device key of its
/// form, and no alias and no device twice.
fn check_bindings(bindings: &[DeviceBinding]) -> Result<(), DeviceBindingProblem> {
    let mut aliases = BTreeSet::new();
    let mut serials = BTreeSet::new();
    let mut device_keys = BTreeSet::new();
    for binding in bindings {
        if crate::validate_instance_alias(&binding.alias).is_err()
            || binding.alias.len() > MAX_HANDSET_ALIAS_BYTES
        {
            return Err(DeviceBindingProblem::Malformed);
        }
        match &binding.device {
            BoundDevice::Serial(serial) if !device_serial_valid(serial) => {
                return Err(DeviceBindingProblem::SerialInvalid);
            }
            BoundDevice::Key(device_key) if !device_key_valid(device_key) => {
                return Err(DeviceBindingProblem::DeviceKeyInvalid);
            }
            BoundDevice::Serial(_) | BoundDevice::Key(_) => {}
        }
        if !aliases.insert(binding.alias.as_str()) {
            return Err(DeviceBindingProblem::DuplicateAlias);
        }
        let first = match &binding.device {
            BoundDevice::Serial(serial) => serials.insert(serial.as_str()),
            BoundDevice::Key(device_key) => device_keys.insert(device_key.as_str()),
        };
        if !first {
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

/// Workflow #479 F-2: whether `device_key` has the form of a device key, the contract's
/// fingerprint: `sha256:` and 64 lowercase hexadecimal digits.
pub fn device_key_valid(device_key: &str) -> bool {
    device_key.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}
