// SPDX-License-Identifier: AGPL-3.0-only

//! The `host_claims` section (Workflow #387 D1c, frozen model M387 v1.2 §2.1 row 8, §4.3, §7.1):
//! what every Runtime-held key (a recovery ladder's, a startup package's) leaves alone before a
//! step. A key whose remaining time plus `renewal_slack_ms` already covers a step's bound is not
//! renewed. Its own section, since the startup package's key renews through the same function
//! as the ladder's.

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

/// The section as a file writes it (§5.3): every member may be left out, none is unknown.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostClaimsFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    renewal_slack_ms: Option<Value>,
}

pub(crate) static ROWS: &[KeyRow] = &[KeyRow {
    name: KeyName::Exact(RENEWAL_SLACK_MS),
    file_path: FilePath::Key,
    value_type: ValueType::DurationMs,
    range: KeyRange::Any,
    default: DefaultSource::File,
    // §5.5, §5.8: read at the next renewal.
    settable: Settable::Yes,
    reader: "D1c",
    since: "0.12.0",
}];

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
