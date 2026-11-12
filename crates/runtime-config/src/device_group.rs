// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #437 MAPP-1 (`contracts/device-group.md`; register §1.323, §1.328): an `instances[]`
//! entry's `device_group`, the name the instances that share one device give it. It is read once
//! at assembly and is not settable; the manifest reports it as `instance.<instance id>.device_group`
//! only for an entry that names it. Entries that name one value are one device group; a value
//! one entry names alone is a group of one and behaves as an entry without it.
//!
//! A member of a group of several turns on no device-level action the first slice does not make
//! group-aware ("Device-level actions"): `start_emulator` `true`, an `idle.action` in effect
//! other than `none`, a ladder profile in effect whose rungs hold `emulator_restart`. actingd
//! refuses such a file at assembly with its own code (`device_group_config_invalid`,
//! `device_action_unsupported`); while the Runtime runs, the whole-instance check refuses an
//! instruction that would turn one on as `config_value_invalid` with `section_check` on the key
//! ([`crate::EffectiveConfig::device_action_in_effect`]).

use crate::table::{
    DefaultSource, FilePath, KeyName, KeyRange, KeyRow, NotSettable, Settable, ValueType,
};

/// The entry's member that names its device group.
pub const DEVICE_GROUP: &str = "device_group";

/// The `idle.action` word that turns no idle action on, the one a member of a group of several
/// names.
pub(crate) const IDLE_ACTION_NONE: &str = "none";

/// The row of `instance.<instance id>.device_group`: a name, no default (no row for an entry
/// that does not name it), read once at assembly.
pub(crate) const ROW: KeyRow = KeyRow {
    name: KeyName::PerInstance(DEVICE_GROUP),
    file_path: FilePath::InstanceMember,
    value_type: ValueType::String,
    range: KeyRange::Any,
    default: DefaultSource::None,
    settable: Settable::No(NotSettable::ReadAtAssembly),
    reader: "MAPP-1",
    since: "0.12.56",
};

/// A device-level action a member of a group of several may not turn on in the first slice
/// (`contracts/device-group.md`, "Device-level actions", rows 1, 3 and 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceAction {
    /// The configured start at daemon start, `start_emulator` `true`.
    StartEmulator,
    /// An idle action in effect other than `none`.
    IdleAction,
    /// A ladder profile in effect whose rungs hold `emulator_restart`.
    EmulatorRestartRung,
}

impl DeviceAction {
    /// The entry's member that turns the action on, the `config_key` actingd's refusal names.
    pub const fn member(self) -> &'static str {
        match self {
            Self::StartEmulator => crate::instance::START_EMULATOR,
            Self::IdleAction => crate::idle::ACTION,
            Self::EmulatorRestartRung => crate::instance::RECOVERY_LADDER_PROFILE,
        }
    }
}
