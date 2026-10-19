// SPDX-License-Identifier: AGPL-3.0-only

//! The per-instance section (Workflow #387 model v1.2 §5.5): `instance.<instance id>.<member>`,
//! in the file the member of the `instances[]` entry with that `instance_id`. D1b hangs the two
//! existing stuck-recovery members here, settable while the Runtime runs; D1c gives them the
//! `recovery_ladder` section's `enabled` and `cooldown_secs` in effect as their defaults (§5.5
//! `section:`, §2.1 row 18b). Workflow #405 (coordinator's order to #386 SR-1, 2026-10-14): each
//! instance's own random delay, `scheduling.random_delay.enabled`, `min_ms` and `max_ms`, in its
//! entry's `scheduling` member, defaulting to the `scheduling` section's keys of the same name;
//! parsed and checked only, no slice reads them yet. Workflow #398 RD-1b adds `device_kind`, the
//! entry's `device.kind` (model-398 v1.1 §11.2): reported only for an instance that names it,
//! read at assembly. Workflow #384 D2-S2 adds `recognition_package`, the entry's
//! `recognition_package.package`: reported only for an instance that names it, read at assembly.
//! Workflow #369 S6a adds `start_emulator` (ORD §3.5 item 17): `true` lets the daemon start the
//! instance's stopped emulator at its start (S6c); default `none`, so a file that does not name
//! it leaves it off and reports no row; read at assembly. Workflow #398 RD-4 adds
//! `recovery_ladder_profile` (model-398 v1.1 §6.2): the ladder profile the instance climbs by,
//! before the section's `kind_profiles`; default `none` (no row unless the entry names it),
//! settable, checked with the `recovery_ladder` section. Workflow #411 B15 R3 (model-b15 §5.2)
//! adds `scheduling.remedy.choices`, the instance's own remedy choice rows in its entry's
//! `scheduling` member: default the `scheduling` section's list; when set, it replaces that list
//! as a whole; settable; its rows checked as the section's are.

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::recovery_ladder;
use crate::scheduling::{
    self, DELAY, RANDOM_DELAY_ENABLED, RANDOM_DELAY_MAX_MS, RANDOM_DELAY_MIN_MS, REMEDY_CHOICES,
};
use crate::table::{
    DefaultSource, FilePath, KeyName, KeyRange, KeyRow, NotSettable, Settable, ValueType,
    instance_key,
};
use actingcommand_contract::{FactScalar, InstanceStuckRecovery};

/// The member that turns the stuck-recovery ladder on or off for one instance.
pub const STUCK_RECOVERY: &str = "stuck_recovery";
/// The member that sets one instance's stuck-recovery cool-down, in seconds.
pub const STUCK_RECOVERY_COOLDOWN_SECS: &str = "stuck_recovery_cooldown_secs";
/// Workflow #398 RD-1b: the kind of device the instance drives, `device.kind` in its entry.
pub const DEVICE_KIND: &str = "device_kind";
/// The values of [`DEVICE_KIND`] (model-398 v1.1 §3.1).
pub const DEVICE_KIND_TOKENS: &[&str] = &["emulator", "handset"];
/// Workflow #384 D2-S2 (D2 model §3.2, v1.4.2): the instance's recognition maintenance package,
/// `recognition_package.package` in its entry: a location only, never a digest.
pub const RECOGNITION_PACKAGE: &str = "recognition_package";
/// Workflow #369 S6a: whether the daemon starts the instance's stopped emulator at its start, the
/// entry's `start_emulator`; discovery-bound instances only, off unless the file names it.
pub const START_EMULATOR: &str = "start_emulator";
/// Workflow #398 RD-4 (model-398 v1.1 §6.2): the ladder profile the instance climbs by, the
/// entry's `recovery_ladder_profile`; it comes before the `recovery_ladder` section's
/// `kind_profiles` and must name one of its `profiles`.
pub const RECOVERY_LADDER_PROFILE: &str = "recovery_ladder_profile";

pub(crate) static ROWS: &[KeyRow] = &[
    KeyRow {
        name: KeyName::PerInstance(STUCK_RECOVERY),
        file_path: FilePath::InstanceMember,
        value_type: ValueType::Boolean,
        range: KeyRange::Any,
        default: DefaultSource::Section(recovery_ladder::ENABLED),
        settable: Settable::Yes,
        reader: "D1c",
        since: "0.12.0",
    },
    KeyRow {
        name: KeyName::PerInstance(STUCK_RECOVERY_COOLDOWN_SECS),
        file_path: FilePath::InstanceMember,
        value_type: ValueType::Integer,
        range: KeyRange::Integer {
            min: 1,
            max: InstanceStuckRecovery::MAX_COOLDOWN_SECS as i64,
        },
        default: DefaultSource::Section(recovery_ladder::COOLDOWN_SECS),
        settable: Settable::Yes,
        reader: "D1c",
        since: "0.12.0",
    },
    random_delay_row(RANDOM_DELAY_ENABLED, ValueType::Boolean, KeyRange::Any),
    random_delay_row(RANDOM_DELAY_MIN_MS, ValueType::DurationMs, DELAY),
    random_delay_row(RANDOM_DELAY_MAX_MS, ValueType::DurationMs, DELAY),
    KeyRow {
        name: KeyName::PerInstance(DEVICE_KIND),
        file_path: FilePath::InstanceMemberAt("device.kind"),
        value_type: ValueType::String,
        range: KeyRange::Tokens(DEVICE_KIND_TOKENS),
        default: DefaultSource::None,
        settable: Settable::No(NotSettable::ReadAtAssembly),
        reader: "RD-1b",
        since: "0.12.0",
    },
    KeyRow {
        name: KeyName::PerInstance(RECOGNITION_PACKAGE),
        file_path: FilePath::InstanceMemberAt("recognition_package.package"),
        value_type: ValueType::String,
        range: KeyRange::Any,
        default: DefaultSource::None,
        settable: Settable::No(NotSettable::ReadAtAssembly),
        reader: "D2-S2",
        since: "0.12.0",
    },
    KeyRow {
        name: KeyName::PerInstance(START_EMULATOR),
        file_path: FilePath::InstanceMember,
        value_type: ValueType::Boolean,
        range: KeyRange::Any,
        default: DefaultSource::None,
        settable: Settable::No(NotSettable::ReadAtAssembly),
        reader: "S6c",
        since: "0.12.0",
    },
    // Workflow #398 RD-4 (model-398 §6.2): reported only for an instance that names it;
    // settable while the Runtime runs, as the `recovery_ladder` section is; a ladder takes it
    // into its snapshot when it is admitted.
    KeyRow {
        name: KeyName::PerInstance(RECOVERY_LADDER_PROFILE),
        file_path: FilePath::InstanceMember,
        value_type: ValueType::String,
        range: KeyRange::Any,
        default: DefaultSource::None,
        settable: Settable::Yes,
        reader: "RD-4",
        since: "0.12.0",
    },
    // Workflow #411 B15 R3 (model-b15 §5.2): the instance's own remedy choice rows; the section's
    // list in effect is its default, and a list the instance sets replaces it as a whole.
    KeyRow {
        name: KeyName::PerInstance(REMEDY_CHOICES),
        file_path: FilePath::InstanceMember,
        value_type: ValueType::List,
        range: KeyRange::Any,
        default: DefaultSource::Section(REMEDY_CHOICES),
        settable: Settable::No(NotSettable::ReadAtAssembly),
        reader: "B15-R4",
        since: "0.12.3",
    },
];

/// Workflow #405: one instance's random delay member, named as the section-level key whose value
/// in effect is its default.
const fn random_delay_row(member: &'static str, value_type: ValueType, range: KeyRange) -> KeyRow {
    KeyRow {
        name: KeyName::PerInstance(member),
        file_path: FilePath::InstanceMember,
        value_type,
        range,
        default: DefaultSource::Section(member),
        settable: Settable::Yes,
        reader: "#405",
        since: "0.12.0",
    }
}

/// The whole-section check of one instance (§5.3): its two stuck-recovery members together form
/// valid stuck-recovery settings, (Workflow #405) its random delay's `min_ms` is at most its
/// `max_ms` as both are in effect, each its own or the section's, and (Workflow #411 B15 R3) its
/// remedy choice rows in effect keep the rows' rules. A member the section does not carry is not
/// checked.
pub(crate) fn check_instance(
    instance: &str,
    value: impl Fn(&str) -> Option<FactScalar>,
) -> Result<(), ConfigFailure> {
    let read = |key: &str| value(key);
    let reader = scheduling::Reader { value: &read };
    let delay_keys = [
        RANDOM_DELAY_ENABLED,
        RANDOM_DELAY_MIN_MS,
        RANDOM_DELAY_MAX_MS,
    ]
    .map(|member| instance_key(instance, member));
    if delay_keys.iter().all(|key| value(key).is_some()) {
        let [enabled, min_ms, max_ms] = &delay_keys;
        scheduling::random_delay(
            &reader,
            [enabled.as_str(), min_ms.as_str(), max_ms.as_str()],
        )?;
    }
    let choices_key = instance_key(instance, REMEDY_CHOICES);
    if value(&choices_key).is_some() {
        scheduling::remedy_choices(&reader, &choices_key)?;
    }
    let enabled_key = instance_key(instance, STUCK_RECOVERY);
    let cooldown_key = instance_key(instance, STUCK_RECOVERY_COOLDOWN_SECS);
    let failed = |key: &str| ConfigFailure::invalid_value(key, ConfigValueProblem::SectionCheck);
    let enabled = match value(&enabled_key) {
        None => true,
        Some(FactScalar::Boolean(enabled)) => enabled,
        Some(_) => return Err(failed(&enabled_key)),
    };
    let cooldown_secs = match value(&cooldown_key) {
        None => return Ok(()),
        Some(FactScalar::Integer(secs)) => {
            u32::try_from(secs).map_err(|_| failed(&cooldown_key))?
        }
        Some(_) => return Err(failed(&cooldown_key)),
    };
    InstanceStuckRecovery {
        enabled,
        cooldown_secs,
    }
    .validate()
    .map_err(|_| failed(&cooldown_key))
}
