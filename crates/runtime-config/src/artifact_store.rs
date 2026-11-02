// SPDX-License-Identifier: AGPL-3.0-only

//! The `artifact_store` section (Workflow #407 PF-1; `contracts/v012-register.md` §1.248;
//! `contracts/capacity-admission.md`): how the artifact store works, so far how often an open
//! stream asks the capacity owner.
//!
//! - `stream_admit_chunk_bytes`: the bytes one capacity decision covers on a stream. A write within
//!   what is left of the last decision's chunk takes no new decision; 0 takes one for every write.
//!
//! Host-wide. The host hands the value in effect to the store at start and after every change, and
//! a stream takes it when it opens, so an instruction takes effect at the next stream. The default
//! lives in the defaults file. A value outside its range is `config_value_invalid`
//! (`out_of_range`) at start, in `check-config` and in an instruction; a value of another type is
//! `wrong_type`.

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{DefaultSource, FilePath, KeyName, KeyRange, KeyRow, Settable, ValueType};
use actingcommand_contract::FactScalar;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The section's name: the first segment of its keys.
pub const SECTION: &str = "artifact_store";
/// The bytes one capacity decision covers on a stream.
pub const STREAM_ADMIT_CHUNK_BYTES: &str = "artifact_store.stream_admit_chunk_bytes";

/// The bounds of `stream_admit_chunk_bytes`. Schema: 0 is a decision for every write; at most
/// 64 MiB, so a refusal that comes one chunk late stays small beside the capacity thresholds.
pub const STREAM_ADMIT_CHUNK_RANGE: KeyRange = KeyRange::Integer {
    min: 0,
    max: 64 * 1024 * 1024,
};

/// The section as the configuration file and the defaults file write it: every member may be
/// left out and none is unknown (M387 §5.3).
// Interface anchor `actingd-config` revision 24 (contracts/component-interfaces.md): a Runtime
// before PF-1 refuses a file that names the section, because the file denies unknown members; a
// file that does not name it keeps its revision.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactStoreFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stream_admit_chunk_bytes: Option<Value>,
}

/// One row per key of the section.
pub(crate) static ROWS: &[KeyRow] = &[KeyRow {
    name: KeyName::Exact(STREAM_ADMIT_CHUNK_BYTES),
    file_path: FilePath::Key,
    value_type: ValueType::Integer,
    range: STREAM_ADMIT_CHUNK_RANGE,
    default: DefaultSource::File,
    settable: Settable::Yes,
    reader: "PF-1",
    since: "0.12.37",
}];

/// `stream_admit_chunk_bytes` in effect (its range is the key table's, checked where a value
/// enters the configuration).
pub(crate) fn stream_admit_chunk_bytes(
    value: &dyn Fn(&str) -> Option<FactScalar>,
) -> Result<u64, ConfigFailure> {
    match value(STREAM_ADMIT_CHUNK_BYTES) {
        Some(FactScalar::Integer(bytes)) => u64::try_from(bytes).map_err(|_| {
            ConfigFailure::invalid_value(STREAM_ADMIT_CHUNK_BYTES, ConfigValueProblem::OutOfRange)
                .with_range(STREAM_ADMIT_CHUNK_RANGE)
        }),
        _ => Err(ConfigFailure::invalid_value(
            STREAM_ADMIT_CHUNK_BYTES,
            ConfigValueProblem::WrongType,
        )),
    }
}
