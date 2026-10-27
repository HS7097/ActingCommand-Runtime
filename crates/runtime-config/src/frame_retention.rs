// SPDX-License-Identifier: AGPL-3.0-only

//! #422 LN-7c Q (`rd-1026/ln7c/final-query.md` §5; Alice's rule for evidence frames, the 14-day
//! question answered A): the evidence-frame rule's key, a top-level member beside the three frame
//! retention switches of `actingd-config` revision 4 (`existing.rs`). An evidence frame in `kept\`
//! that no agent or person confirmed reading is removed once it is this many days old by its
//! ingestion time; a confirmed one may go at any age. There is no 14-day key. The frame cleaner
//! reads the value in effect at each pass start, so an instruction (`ac_config_set`) takes effect
//! at the next pass; actingd reports it in `config.parameters.top_level`, which `actingledger
//! frames` reads. Interface anchor `actingd-config` revision 16 (contracts/component-interfaces.md):
//! a Runtime before LN-7c Q refuses a file that names it; a file that does not keeps its revision.

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{DefaultSource, FilePath, KeyName, KeyRange, KeyRow, Settable, ValueType};
use actingcommand_contract::FactScalar;

/// `frame_retention_evidence_max_age_days`: 15 by default, `1..=3650`, settable.
pub const EVIDENCE_MAX_AGE_DAYS: &str = "frame_retention_evidence_max_age_days";

/// A day at the least, ten years at the most.
const EVIDENCE_MAX_AGE_DAYS_RANGE: KeyRange = KeyRange::Integer { min: 1, max: 3_650 };

pub(crate) static ROWS: &[KeyRow] = &[KeyRow {
    name: KeyName::Exact(EVIDENCE_MAX_AGE_DAYS),
    file_path: FilePath::Key,
    value_type: ValueType::Integer,
    range: EVIDENCE_MAX_AGE_DAYS_RANGE,
    default: DefaultSource::File,
    settable: Settable::Yes,
    reader: "LN-7c Q",
    since: "0.12.17",
}];

/// `frame_retention_evidence_max_age_days` in effect, in days (its range is the key table's,
/// checked where a value enters the configuration).
pub(crate) fn evidence_max_age_days(
    value: &dyn Fn(&str) -> Option<FactScalar>,
) -> Result<u64, ConfigFailure> {
    match value(EVIDENCE_MAX_AGE_DAYS) {
        Some(FactScalar::Integer(days)) => u64::try_from(days).map_err(|_| {
            ConfigFailure::invalid_value(EVIDENCE_MAX_AGE_DAYS, ConfigValueProblem::OutOfRange)
        }),
        _ => Err(ConfigFailure::invalid_value(
            EVIDENCE_MAX_AGE_DAYS,
            ConfigValueProblem::WrongType,
        )),
    }
}
