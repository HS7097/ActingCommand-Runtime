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

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::recovery_ladder;
use crate::scheduling::{
    self, DELAY, RANDOM_DELAY_ENABLED, RANDOM_DELAY_MAX_MS, RANDOM_DELAY_MIN_MS,
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
/// valid stuck-recovery settings, and (Workflow #405) its random delay's `min_ms` is at most its
/// `max_ms` as both are in effect, each its own or the section's. A member the section does not
/// carry is not checked.
pub(crate) fn check_instance(
    instance: &str,
    value: impl Fn(&str) -> Option<FactScalar>,
) -> Result<(), ConfigFailure> {
    let delay_keys = [
        RANDOM_DELAY_ENABLED,
        RANDOM_DELAY_MIN_MS,
        RANDOM_DELAY_MAX_MS,
    ]
    .map(|member| instance_key(instance, member));
    if delay_keys.iter().all(|key| value(key).is_some()) {
        let read = |key: &str| value(key);
        let [enabled, min_ms, max_ms] = &delay_keys;
        scheduling::random_delay(
            &scheduling::Reader { value: &read },
            [enabled.as_str(), min_ms.as_str(), max_ms.as_str()],
        )?;
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
