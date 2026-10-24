// SPDX-License-Identifier: AGPL-3.0-only

//! The `host_claims` section (Workflow #387 D1c, frozen model M387 v1.2 §2.1 row 8, §4.3, §7.1):
//! what every Runtime-held key (a recovery ladder's, a startup package's) leaves alone before a
//! step. A key whose remaining time plus `renewal_slack_ms` already covers a step's bound is not
//! renewed. Its own section, since the startup package's key renews through the same function
//! as the ladder's. #411 A64 (ruling C-43, `actingd-config` revision 11) adds the end grace: a
//! contained run that reaches its deadline renews its lease once by `run_end_grace_ms` before its
//! end work, and the lease sweep leaves that lease to the run until the grace ends.

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{DefaultSource, FilePath, KeyName, KeyRange, KeyRow, Settable, ValueType};
use actingcommand_contract::FactScalar;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The section's name: the first segment of its keys.
pub const SECTION: &str = "host_claims";
/// How much short of a step's bound a key may be and still not be renewed, in milliseconds.
pub const RENEWAL_SLACK_MS: &str = "host_claims.renewal_slack_ms";
/// #411 A64 (ruling C-43): how long a run's lease lasts from the start of its end phase (its
/// deadline reached), in milliseconds.
pub const RUN_END_GRACE_MS: &str = "host_claims.run_end_grace_ms";

/// #411 A64: the end grace lies above the shipped heartbeat reserve (5 s, which a lease TTL must
/// exceed) and within ten minutes.
const RUN_END_GRACE_RANGE: KeyRange = KeyRange::Integer {
    min: 10_000,
    max: 600_000,
};

/// The section as a file writes it (§5.3): every member may be left out, none is unknown.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostClaimsFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    renewal_slack_ms: Option<Value>,
    // Interface anchor `actingd-config` revision 11 (contracts/component-interfaces.md): a Runtime
    // before #411 A64 refuses a file that names it, because the section denies unknown members;
    // a file that does not name it keeps its revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run_end_grace_ms: Option<Value>,
}

pub(crate) static ROWS: &[KeyRow] = &[
    KeyRow {
        name: KeyName::Exact(RENEWAL_SLACK_MS),
        file_path: FilePath::Key,
        value_type: ValueType::DurationMs,
        range: KeyRange::Any,
        default: DefaultSource::File,
        // §5.5, §5.8: read at the next renewal.
        settable: Settable::Yes,
        reader: "D1c",
        since: "0.12.0",
    },
    KeyRow {
        name: KeyName::Exact(RUN_END_GRACE_MS),
        file_path: FilePath::Key,
        value_type: ValueType::DurationMs,
        range: RUN_END_GRACE_RANGE,
        default: DefaultSource::File,
        // Read when a run reaches its deadline.
        settable: Settable::Yes,
        reader: "A64",
        since: "0.12.12",
    },
];

/// The slack in effect, from `value` (the configuration in effect).
pub(crate) fn renewal_slack_ms(
    value: &dyn Fn(&str) -> Option<FactScalar>,
) -> Result<u64, ConfigFailure> {
    match value(RENEWAL_SLACK_MS) {
        Some(FactScalar::DurationMs(ms)) => Ok(ms),
        _ => Err(ConfigFailure::invalid_value(
            RENEWAL_SLACK_MS,
            ConfigValueProblem::WrongType,
        )),
    }
}

/// #411 A64: the end grace in effect, from `value` (the configuration in effect).
pub(crate) fn run_end_grace_ms(
    value: &dyn Fn(&str) -> Option<FactScalar>,
) -> Result<u64, ConfigFailure> {
    match value(RUN_END_GRACE_MS) {
        Some(FactScalar::DurationMs(ms)) => Ok(ms),
        _ => Err(ConfigFailure::invalid_value(
            RUN_END_GRACE_MS,
            ConfigValueProblem::WrongType,
        )),
    }
}
