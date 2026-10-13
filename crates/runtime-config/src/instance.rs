// SPDX-License-Identifier: AGPL-3.0-only

//! The per-instance section (Workflow #387 model v1.2 §5.5): `instance.<instance id>.<member>`,
//! in the file the member of the `instances[]` entry with that `instance_id`. D1b hangs the two
//! existing stuck-recovery members here, settable while the Runtime runs; their defaults stay in
//! code until D1c's `recovery_ladder` section gives them `section:` defaults (§5.5).

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{
    DefaultSource, FilePath, KeyName, KeyRange, KeyRow, Settable, ValueType, instance_key,
};
use actingcommand_contract::{FactScalar, InstanceStuckRecovery};
use serde_json::Value;

/// The member that turns the stuck-recovery ladder on or off for one instance.
pub const STUCK_RECOVERY: &str = "stuck_recovery";
/// The member that sets one instance's stuck-recovery cool-down, in seconds.
pub const STUCK_RECOVERY_COOLDOWN_SECS: &str = "stuck_recovery_cooldown_secs";

pub(crate) static ROWS: &[KeyRow] = &[
    KeyRow {
        name: KeyName::PerInstance(STUCK_RECOVERY),
        file_path: FilePath::InstanceMember,
        value_type: ValueType::Boolean,
        range: KeyRange::Any,
        default: DefaultSource::Code,
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
        default: DefaultSource::Code,
        settable: Settable::Yes,
        reader: "D1c",
        since: "0.12.0",
    },
];

/// The code default of a per-instance row (`default = code`): the contract's
/// `InstanceStuckRecovery` default until D1c moves it into the `recovery_ladder` section.
pub fn code_default(row: &KeyRow) -> Option<Value> {
    let defaults = InstanceStuckRecovery::default();
    match row.name {
        KeyName::PerInstance(STUCK_RECOVERY) => Some(Value::Bool(defaults.enabled)),
        KeyName::PerInstance(STUCK_RECOVERY_COOLDOWN_SECS) => {
            Some(Value::from(defaults.cooldown_secs))
        }
        _ => None,
    }
}

/// The whole-section check of one instance (§5.3): its two members together form valid
/// stuck-recovery settings. A member the section does not carry is not checked.
pub(crate) fn check_instance(
    instance: &str,
    value: impl Fn(&str) -> Option<FactScalar>,
) -> Result<(), ConfigFailure> {
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
