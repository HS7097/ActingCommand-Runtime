// SPDX-License-Identifier: AGPL-3.0-only

//! The `ledger_observer` section (Workflow #383 AO-1; `contracts/ledger-observer.md` §15,
//! `contracts/v012-register.md` §5). Every key is parsed, checked and defaulted from v0.12.0 on,
//! the keys only 0.12.x slices read included (order-v012 R4): the write path's retries, stall
//! limit, probe period and dump limit take effect with AO-1; the capture queue's budget, tiers,
//! batches and request ring with AO-2; the tag lists with AO-2 and AO-5; the probes' periods,
//! sample size, lag threshold and gap window with AO-8. P0-LO (`actingd-config` revision 10) adds
//! the deadline of the start's ledger open and the share of it above which the open writes the
//! warning `ledger_open_slow`; both are read when the ledger opens. LN-5 (#422, LEAN-L,
//! `actingd-config` revision 12, v0.12.13) adds the rows of one page of a history read, read when
//! the ledger opens. LN-TUNE (#422, `actingd-config` revision 12) adds how long a caller waits for
//! one reply of the ledger writer, read when the ledger opens. LN-8 (#422, `actingd-config`
//! revision 13) adds how many of the newest records the start's ledger open authenticates, read
//! when the ledger opens. LN-9 (#422, `actingd-config` revision 13) adds the pace and the limits
//! of the ledger's background scrub: its rows per second (settable, read after each chunk), the
//! rows of one chunk and the least time between two passes (both read when the ledger opens).
//!
//! The section is read whole and checked whole (M387 §5.3): the tiers rise strictly within
//! `(0, 1]`, the hysteresis lies within `(0, 1)` so each release line is below its activation
//! line, the gap between tiers 2 and 3 holds at least one spill batch, and the waits list one
//! wait per write attempt. A broken rule is `config_value_invalid` (`section_check`) for the key
//! it concerns, at start, in `check-config` and through an instruction alike. Every default lives
//! in the defaults file.

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{
    DefaultSource, FilePath, KeyName, KeyRange, KeyRow, NotSettable, Settable, ValueType,
};
use actingcommand_contract::FactScalar;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The section's name: the first segment of its keys.
pub const SECTION: &str = "ledger_observer";
/// The capture queue's memory segment byte limit (§7; AO-2).
pub const BUFFER_BYTES: &str = "ledger_observer.buffer_bytes";
/// The capture queue's memory segment record limit (§7; AO-2).
pub const BUFFER_RECORDS: &str = "ledger_observer.buffer_records";
/// Tier 1 of the capture queue (§7; AO-2).
pub const BUFFER_TIER1_RATIO: &str = "ledger_observer.buffer.tier1_ratio";
/// Tier 2: spilling starts (§7; AO-2).
pub const BUFFER_TIER2_RATIO: &str = "ledger_observer.buffer.tier2_ratio";
/// Tier 3: alarm, everything spills (§7; AO-2).
pub const BUFFER_TIER3_RATIO: &str = "ledger_observer.buffer.tier3_ratio";
/// Release line = tier × (1 − hysteresis) (§7; AO-2).
pub const BUFFER_HYSTERESIS_RATIO: &str = "ledger_observer.buffer.hysteresis_ratio";
/// Bytes per spill (§7; AO-2).
pub const SPILL_BATCH_BYTES: &str = "ledger_observer.spill_batch_bytes";
/// The dump file's limit (§9.2; AO-1).
pub const DUMP_MAX_BYTES: &str = "ledger_observer.dump_max_bytes";
/// Records per flush batch below tier 1 (§8; AO-2).
pub const FLUSH_BATCH_RECORDS: &str = "ledger_observer.flush_batch_records";
/// Records per flush batch at tier 1 and above (§8; AO-2).
pub const FLUSH_BATCH_RECORDS_BUSY: &str = "ledger_observer.flush_batch_records_busy";
/// Attempts of one write, the first included (§9.1; AO-1).
pub const WRITE_ATTEMPTS: &str = "ledger_observer.write_attempts";
/// The wait before each attempt, in milliseconds (§9.1; AO-1).
pub const WRITE_RETRY_BACKOFF_MS: &str = "ledger_observer.write_retry_backoff_ms";
/// One attempt's stall limit (§9.1, T8; AO-1).
pub const FLUSH_ATTEMPT_STALL_MS: &str = "ledger_observer.flush_attempt_stall_ms";
/// The flush lag threshold of probe T4 (§14; AO-8).
pub const FLUSH_LAG_WARN_MS: &str = "ledger_observer.flush_lag_warn_ms";
/// The period of probes T3, T4, T5 and T8 (§14; T8 with AO-1).
pub const HEALTH_PROBE_INTERVAL_MS: &str = "ledger_observer.health_probe_interval_ms";
/// The period of probes T1, T2 and T7 (§14; AO-8).
pub const PROBE_INTERVAL_MS: &str = "ledger_observer.probe_interval_ms";
/// Keys per T1 sample (§14; AO-8).
pub const PROBE_SAMPLE_KEYS: &str = "ledger_observer.probe_sample_keys";
/// The ring size of the index table's request rows (§11.1; AO-2, AO-6).
pub const REQUEST_INDEX_RECORDS: &str = "ledger_observer.request_index_records";
/// The window of "recent gaps" in status (§10; AO-8).
pub const STATUS_GAP_WINDOW_MS: &str = "ledger_observer.status_gap_window_ms";
/// The tagged workbench keys (§11.2; AO-2, AO-5).
pub const TAG_WORKBENCH_PATTERNS: &str = "ledger_observer.tag.workbench_patterns";
/// The workbench keys known to be untagged and re-observed (§11.2; AO-5).
pub const TAG_REOBSERVE_PREFIXES: &str = "ledger_observer.tag.reobserve_prefixes";
/// #411 A51: the work budget of one read on the ledger writer's thread, in milliseconds.
pub const READ_BUDGET_MS: &str = "ledger_observer.read_budget_ms";
/// #411 A51: how long the Runtime's close waits for the ledger writer, in milliseconds.
pub const CLOSE_TIMEOUT_MS: &str = "ledger_observer.close_timeout_ms";
/// P0-LO: the deadline of the ledger open at start, in milliseconds.
pub const OPEN_TIMEOUT_MS: &str = "ledger_observer.open_timeout_ms";
/// P0-LO: the share of `open_timeout_ms`, in thousandths, above which the open's measured time
/// writes the warning `ledger_open_slow`.
pub const OPEN_WARN_RATIO_MILLI: &str = "ledger_observer.open_warn_ratio_milli";
/// LN-5 (#422, LEAN-L §3.7): the rows of one page of a history read; the ledger reads its
/// history from the store a page at a time.
pub const HISTORY_PAGE_ROWS: &str = "ledger_observer.history_page_rows";
/// #422 LN-TUNE: how long a caller waits for one reply of the ledger writer, in milliseconds
/// (10 000 by default; until LN-TUNE the ledger's constant). A read's budget stays below it, so
/// a read stops before its caller stops waiting for it.
pub const WRITER_REPLY_TIMEOUT_MS: &str = "ledger_observer.writer_reply_timeout_ms";
/// LN-8 (#422, LEAN-L §3.2 step 2, §3.7): how many of the newest records the start's ledger open
/// reads and authenticates; older records are authenticated when they are read, and by
/// `ledger-maintenance verify`.
pub const OPEN_VERIFY_TAIL_RECORDS: &str = "ledger_observer.open_verify_tail_records";
/// #422 LN-9 (LEAN-L §3.7, §6; Alice's ruling 57): the rows per second the ledger's background
/// scrub reads and authenticates (625 by default: about 40 minutes a pass at 1.5 million records).
/// Settable: the scrub reads the rate in effect before each chunk.
pub const SCRUB_ROWS_PER_SECOND: &str = "ledger_observer.scrub_rows_per_second";
/// #422 LN-9: the rows of one scrub chunk, each chunk in a read transaction of its own.
pub const SCRUB_CHUNK_ROWS: &str = "ledger_observer.scrub_chunk_rows";
/// #422 LN-9: the least time from the start of one scrub pass to the start of the next, in
/// milliseconds; the first pass starts when the ledger opens.
pub const SCRUB_PASS_INTERVAL_MS: &str = "ledger_observer.scrub_pass_interval_ms";

/// #422 LN-9 (LEAN-L §3.7): the scrub's rate lies within 1 and 100 000 rows a second.
const SCRUB_ROWS_PER_SECOND_RANGE: KeyRange = KeyRange::Integer {
    min: 1,
    max: 100_000,
};

/// #422 LN-9: a scrub chunk holds one row at least and 4 096 at most, as a history page does.
const SCRUB_CHUNK_ROWS_RANGE: KeyRange = KeyRange::Integer { min: 1, max: 4_096 };

/// #422 LN-9: a pass starts at most once a minute and at least once a day.
const SCRUB_PASS_INTERVAL_RANGE: KeyRange = KeyRange::Integer {
    min: 60_000,
    max: 86_400_000,
};

/// #422 LN-TUNE: the writer's reply limit lies within 1 s and the 600 s the maintenance limits
/// accept.
const WRITER_REPLY_TIMEOUT_RANGE: KeyRange = KeyRange::Integer {
    min: 1_000,
    max: 600_000,
};

/// #411 A51: `read_budget_ms` lies below the writer's default reply limit; the section check
/// keeps it below the reply limit in effect too (`writer_reply_timeout_ms`, #422 LN-TUNE).
const READ_BUDGET_RANGE: KeyRange = KeyRange::Integer { min: 1, max: 9_999 };

/// P0-LO: the open's deadline lies within 10 s and the 600 s the maintenance limits accept.
const OPEN_TIMEOUT_RANGE: KeyRange = KeyRange::Integer {
    min: 10_000,
    max: 600_000,
};

/// P0-LO: the warning line lies within a tenth of the deadline and the deadline itself.
const OPEN_WARN_RATIO_RANGE: KeyRange = KeyRange::Integer {
    min: 100,
    max: 1_000,
};

/// LN-5: a history page holds one row at least and 4096 at most.
const HISTORY_PAGE_ROWS_RANGE: KeyRange = KeyRange::Integer { min: 1, max: 4_096 };

/// LN-8 (LEAN-L §3.7): the open authenticates one record at least and a million at most.
const OPEN_VERIFY_TAIL_RECORDS_RANGE: KeyRange = KeyRange::Integer {
    min: 1,
    max: 1_000_000,
};

/// The largest attempt count a configuration may name: the write path counts in `u32`.
const MAX_ATTEMPTS: i64 = u32::MAX as i64;

/// The section as the configuration file and the defaults file write it: every member may be
/// left out and none is unknown (M387 §5.3). Each value keeps its JSON here; the key table
/// checks its type and range, the whole-section check its rules.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerObserverFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    buffer_bytes: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    buffer_records: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    buffer: Option<BufferFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spill_batch_bytes: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dump_max_bytes: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    flush_batch_records: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    flush_batch_records_busy: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    write_attempts: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    write_retry_backoff_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    flush_attempt_stall_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    flush_lag_warn_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    health_probe_interval_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    probe_interval_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    probe_sample_keys: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    request_index_records: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status_gap_window_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tag: Option<TagFile>,
    // #411 A51 (`actingd-config` revision 8): a Runtime before A51 refuses a file that names
    // either, because the section denies unknown members.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    read_budget_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    close_timeout_ms: Option<Value>,
    // Interface anchor `actingd-config` revision 10 (contracts/component-interfaces.md): a Runtime
    // before P0-LO refuses a file that names either, because the section denies unknown members;
    // a file that names neither keeps its revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    open_timeout_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    open_warn_ratio_milli: Option<Value>,
    // Interface anchor `actingd-config` revision 12 (contracts/component-interfaces.md): a Runtime
    // before LN-5 refuses a file that names it, because the section denies unknown members; a
    // file that does not name it keeps its revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    history_page_rows: Option<Value>,
    // Interface anchor `actingd-config` revision 12 (contracts/component-interfaces.md): a Runtime
    // before LN-TUNE refuses a file that names it, because the section denies unknown members; a
    // file that does not name it keeps its revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    writer_reply_timeout_ms: Option<Value>,
    // Interface anchor `actingd-config` revision 13 (contracts/component-interfaces.md): a Runtime
    // before LN-8 refuses a file that names it, because the section denies unknown members; a
    // file that does not name it keeps its revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    open_verify_tail_records: Option<Value>,
    // LN-9 joins revision 13: a Runtime before LN-9 refuses a file that names one, because the
    // section denies unknown members; a file that names none keeps its revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scrub_rows_per_second: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scrub_chunk_rows: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scrub_pass_interval_ms: Option<Value>,
}

/// The `buffer` member: the capture queue's tiers.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BufferFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tier1_ratio: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tier2_ratio: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tier3_ratio: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hysteresis_ratio: Option<Value>,
}

/// The `tag` member: which workbench keys are tagged.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TagFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workbench_patterns: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reobserve_prefixes: Option<Value>,
}

/// Read when the ledger opens: the capture queue, the writer and its probe exist before any
/// instruction value is restored (§15: "the queue exists before the ledger opens").
const AT_OPEN: Settable = Settable::No(NotSettable::BeforeLedgerOpen);
/// Read where it is used, by a probe or a status read (§15: "probe periods true").
const WHEN_USED: Settable = Settable::Yes;
/// At least one, in the key's unit.
const POSITIVE: KeyRange = KeyRange::Integer {
    min: 1,
    max: i64::MAX,
};

const fn row(
    name: &'static str,
    value_type: ValueType,
    range: KeyRange,
    settable: Settable,
    reader: &'static str,
) -> KeyRow {
    KeyRow {
        name: KeyName::Exact(name),
        file_path: FilePath::Key,
        value_type,
        range,
        default: DefaultSource::File,
        settable,
        reader,
        since: "0.12.0",
    }
}

/// One row per key of the section, in the order of `contracts/ledger-observer.md` §15.
pub(crate) static ROWS: &[KeyRow] = &[
    row(BUFFER_BYTES, ValueType::Integer, POSITIVE, AT_OPEN, "AO-2"),
    row(
        BUFFER_RECORDS,
        ValueType::Integer,
        POSITIVE,
        AT_OPEN,
        "AO-2",
    ),
    row(
        BUFFER_TIER1_RATIO,
        ValueType::Decimal,
        KeyRange::Any,
        AT_OPEN,
        "AO-2",
    ),
    row(
        BUFFER_TIER2_RATIO,
        ValueType::Decimal,
        KeyRange::Any,
        AT_OPEN,
        "AO-2",
    ),
    row(
        BUFFER_TIER3_RATIO,
        ValueType::Decimal,
        KeyRange::Any,
        AT_OPEN,
        "AO-2",
    ),
    row(
        BUFFER_HYSTERESIS_RATIO,
        ValueType::Decimal,
        KeyRange::Any,
        AT_OPEN,
        "AO-2",
    ),
    row(
        SPILL_BATCH_BYTES,
        ValueType::Integer,
        POSITIVE,
        AT_OPEN,
        "AO-2",
    ),
    row(
        DUMP_MAX_BYTES,
        ValueType::Integer,
        POSITIVE,
        AT_OPEN,
        "AO-1",
    ),
    row(
        FLUSH_BATCH_RECORDS,
        ValueType::Integer,
        POSITIVE,
        AT_OPEN,
        "AO-2",
    ),
    row(
        FLUSH_BATCH_RECORDS_BUSY,
        ValueType::Integer,
        POSITIVE,
        AT_OPEN,
        "AO-2",
    ),
    row(
        WRITE_ATTEMPTS,
        ValueType::Integer,
        KeyRange::Integer {
            min: 1,
            max: MAX_ATTEMPTS,
        },
        AT_OPEN,
        "AO-1",
    ),
    row(
        WRITE_RETRY_BACKOFF_MS,
        ValueType::List,
        KeyRange::Any,
        AT_OPEN,
        "AO-1",
    ),
    row(
        FLUSH_ATTEMPT_STALL_MS,
        ValueType::DurationMs,
        POSITIVE,
        AT_OPEN,
        "AO-1",
    ),
    row(
        FLUSH_LAG_WARN_MS,
        ValueType::DurationMs,
        POSITIVE,
        WHEN_USED,
        "AO-8",
    ),
    row(
        HEALTH_PROBE_INTERVAL_MS,
        ValueType::DurationMs,
        POSITIVE,
        AT_OPEN,
        "AO-1",
    ),
    row(
        PROBE_INTERVAL_MS,
        ValueType::DurationMs,
        POSITIVE,
        WHEN_USED,
        "AO-8",
    ),
    row(
        PROBE_SAMPLE_KEYS,
        ValueType::Integer,
        POSITIVE,
        WHEN_USED,
        "AO-8",
    ),
    row(
        REQUEST_INDEX_RECORDS,
        ValueType::Integer,
        POSITIVE,
        AT_OPEN,
        "AO-2",
    ),
    row(
        STATUS_GAP_WINDOW_MS,
        ValueType::DurationMs,
        POSITIVE,
        WHEN_USED,
        "AO-8",
    ),
    row(
        TAG_WORKBENCH_PATTERNS,
        ValueType::List,
        KeyRange::Any,
        AT_OPEN,
        "AO-2",
    ),
    row(
        TAG_REOBSERVE_PREFIXES,
        ValueType::List,
        KeyRange::Any,
        AT_OPEN,
        "AO-5",
    ),
    // #411 A51: read when the ledger opens, as the writer's other limits are.
    row_since(READ_BUDGET_MS, READ_BUDGET_RANGE, "A51", "0.12.9"),
    row_since(CLOSE_TIMEOUT_MS, POSITIVE, "A51", "0.12.9"),
    // P0-LO: the start's ledger open reads both before any instruction value is restored.
    row_since(OPEN_TIMEOUT_MS, OPEN_TIMEOUT_RANGE, "P0-LO", "0.12.11"),
    KeyRow {
        name: KeyName::Exact(OPEN_WARN_RATIO_MILLI),
        file_path: FilePath::Key,
        value_type: ValueType::Integer,
        range: OPEN_WARN_RATIO_RANGE,
        default: DefaultSource::File,
        settable: AT_OPEN,
        reader: "P0-LO",
        since: "0.12.11",
    },
    // LN-5: read when the ledger opens, as the writer's other limits are.
    KeyRow {
        name: KeyName::Exact(HISTORY_PAGE_ROWS),
        file_path: FilePath::Key,
        value_type: ValueType::Integer,
        range: HISTORY_PAGE_ROWS_RANGE,
        default: DefaultSource::File,
        settable: AT_OPEN,
        reader: "LN-5",
        since: "0.12.13",
    },
    // #422 LN-TUNE: read when the ledger opens, as the writer's other limits are.
    row_since(
        WRITER_REPLY_TIMEOUT_MS,
        WRITER_REPLY_TIMEOUT_RANGE,
        "LN-TUNE",
        "0.12.13",
    ),
    // #422 LN-8: read when the ledger opens; it bounds that open.
    KeyRow {
        name: KeyName::Exact(OPEN_VERIFY_TAIL_RECORDS),
        file_path: FilePath::Key,
        value_type: ValueType::Integer,
        range: OPEN_VERIFY_TAIL_RECORDS_RANGE,
        default: DefaultSource::File,
        settable: AT_OPEN,
        reader: "LN-8",
        since: "0.12.14",
    },
    // #422 LN-9: the rate is read before each chunk (settable); the chunk's rows and the pass
    // interval when the ledger opens, as the writer's other limits are.
    KeyRow {
        name: KeyName::Exact(SCRUB_ROWS_PER_SECOND),
        file_path: FilePath::Key,
        value_type: ValueType::Integer,
        range: SCRUB_ROWS_PER_SECOND_RANGE,
        default: DefaultSource::File,
        settable: WHEN_USED,
        reader: "LN-9",
        since: "0.12.14",
    },
    KeyRow {
        name: KeyName::Exact(SCRUB_CHUNK_ROWS),
        file_path: FilePath::Key,
        value_type: ValueType::Integer,
        range: SCRUB_CHUNK_ROWS_RANGE,
        default: DefaultSource::File,
        settable: AT_OPEN,
        reader: "LN-9",
        since: "0.12.14",
    },
    row_since(
        SCRUB_PASS_INTERVAL_MS,
        SCRUB_PASS_INTERVAL_RANGE,
        "LN-9",
        "0.12.14",
    ),
];

/// A milliseconds key read when the ledger opens, with the release that added its row.
const fn row_since(
    name: &'static str,
    range: KeyRange,
    reader: &'static str,
    since: &'static str,
) -> KeyRow {
    KeyRow {
        name: KeyName::Exact(name),
        file_path: FilePath::Key,
        value_type: ValueType::DurationMs,
        range,
        default: DefaultSource::File,
        settable: AT_OPEN,
        reader,
        since,
    }
}

/// The capture queue's tiers (§7).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaptureTiers {
    pub tier1_ratio: f64,
    pub tier2_ratio: f64,
    pub tier3_ratio: f64,
    pub hysteresis_ratio: f64,
}

/// The `ledger_observer` section in effect, read and checked whole.
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerObserverConfig {
    pub buffer_bytes: u64,
    pub buffer_records: u64,
    pub tiers: CaptureTiers,
    pub spill_batch_bytes: u64,
    pub dump_max_bytes: u64,
    pub flush_batch_records: u64,
    pub flush_batch_records_busy: u64,
    pub write_attempts: u32,
    pub write_retry_backoff_ms: Vec<u64>,
    pub flush_attempt_stall_ms: u64,
    pub flush_lag_warn_ms: u64,
    pub health_probe_interval_ms: u64,
    pub probe_interval_ms: u64,
    pub probe_sample_keys: u64,
    pub request_index_records: u64,
    pub status_gap_window_ms: u64,
    pub tag_workbench_patterns: Vec<String>,
    pub tag_reobserve_prefixes: Vec<String>,
    /// #411 A51.
    pub read_budget_ms: u64,
    pub close_timeout_ms: u64,
    /// P0-LO: the deadline of the start's ledger open.
    pub open_timeout_ms: u64,
    /// P0-LO: an open whose measured time exceeds `open_timeout_ms` × this / 1000 writes
    /// `ledger_open_slow`.
    pub open_warn_ratio_milli: u64,
    /// LN-5 (#422): the rows of one page of a history read.
    pub history_page_rows: u64,
    /// #422 LN-TUNE: how long a caller waits for one reply of the ledger writer.
    pub writer_reply_timeout_ms: u64,
    /// #422 LN-8: how many of the newest records the start's ledger open authenticates.
    pub open_verify_tail_records: u64,
    /// #422 LN-9: the rows per second of the ledger's background scrub.
    pub scrub_rows_per_second: u64,
    /// #422 LN-9: the rows of one scrub chunk.
    pub scrub_chunk_rows: u64,
    /// #422 LN-9: the least time from the start of one scrub pass to the start of the next.
    pub scrub_pass_interval_ms: u64,
}

fn invalid(key: &str, problem: ConfigValueProblem) -> ConfigFailure {
    ConfigFailure::invalid_value(key, problem)
}

/// The values of the section's keys, read one by one from the configuration in effect.
struct Reader<'a> {
    value: &'a dyn Fn(&str) -> Option<FactScalar>,
}

impl Reader<'_> {
    fn scalar(&self, key: &str) -> Result<FactScalar, ConfigFailure> {
        (self.value)(key).ok_or_else(|| invalid(key, ConfigValueProblem::WrongType))
    }

    fn count(&self, key: &str) -> Result<u64, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::Integer(number) => {
                u64::try_from(number).map_err(|_| invalid(key, ConfigValueProblem::OutOfRange))
            }
            _ => Err(invalid(key, ConfigValueProblem::WrongType)),
        }
    }

    fn duration(&self, key: &str) -> Result<u64, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::DurationMs(ms) => Ok(ms),
            _ => Err(invalid(key, ConfigValueProblem::WrongType)),
        }
    }

    /// A list or decimal key's value, from its canonical JSON text.
    fn json(&self, key: &str) -> Result<Value, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::String(text) => serde_json::from_str::<Value>(&text)
                .map_err(|_| invalid(key, ConfigValueProblem::WrongType)),
            _ => Err(invalid(key, ConfigValueProblem::WrongType)),
        }
    }

    fn ratio(&self, key: &str) -> Result<f64, ConfigFailure> {
        self.json(key)?
            .as_f64()
            .ok_or_else(|| invalid(key, ConfigValueProblem::WrongType))
    }

    fn waits(&self, key: &str) -> Result<Vec<u64>, ConfigFailure> {
        match self.json(key)? {
            Value::Array(items) => items
                .iter()
                .map(|item| {
                    item.as_u64()
                        .ok_or_else(|| invalid(key, ConfigValueProblem::WrongType))
                })
                .collect(),
            _ => Err(invalid(key, ConfigValueProblem::WrongType)),
        }
    }

    /// A list of key patterns or prefixes: each a non-empty text without space or control
    /// characters.
    fn keys(&self, key: &str) -> Result<Vec<String>, ConfigFailure> {
        let Value::Array(items) = self.json(key)? else {
            return Err(invalid(key, ConfigValueProblem::WrongType));
        };
        items
            .into_iter()
            .map(|item| match item {
                Value::String(text)
                    if !text.is_empty()
                        && !text.chars().any(|character| {
                            character.is_whitespace() || character.is_control()
                        }) =>
                {
                    Ok(text)
                }
                Value::String(_) => Err(invalid(key, ConfigValueProblem::SectionCheck)),
                _ => Err(invalid(key, ConfigValueProblem::WrongType)),
            })
            .collect()
    }
}

impl LedgerObserverConfig {
    /// The section from `value` (the configuration in effect), checked whole (M387 §5.3).
    pub fn parse(value: &dyn Fn(&str) -> Option<FactScalar>) -> Result<Self, ConfigFailure> {
        let reader = Reader { value };
        let tiers = CaptureTiers {
            tier1_ratio: reader.ratio(BUFFER_TIER1_RATIO)?,
            tier2_ratio: reader.ratio(BUFFER_TIER2_RATIO)?,
            tier3_ratio: reader.ratio(BUFFER_TIER3_RATIO)?,
            hysteresis_ratio: reader.ratio(BUFFER_HYSTERESIS_RATIO)?,
        };
        // The tiers rise strictly within (0, 1]; each release line, tier × (1 − hysteresis),
        // lies below its activation line (§7 "configuration check").
        let tier1_rises = tiers.tier1_ratio > 0.0 && tiers.tier1_ratio < tiers.tier2_ratio;
        let tier2_rises = tiers.tier2_ratio < tiers.tier3_ratio;
        let tier3_within = tiers.tier3_ratio <= 1.0;
        let hysteresis_within = tiers.hysteresis_ratio > 0.0 && tiers.hysteresis_ratio < 1.0;
        for (holds, key) in [
            (tier1_rises, BUFFER_TIER1_RATIO),
            (tier2_rises, BUFFER_TIER2_RATIO),
            (tier3_within, BUFFER_TIER3_RATIO),
            (hysteresis_within, BUFFER_HYSTERESIS_RATIO),
        ] {
            if !holds {
                return Err(invalid(key, ConfigValueProblem::SectionCheck));
            }
        }
        let buffer_bytes = reader.count(BUFFER_BYTES)?;
        let spill_batch_bytes = reader.count(SPILL_BATCH_BYTES)?;
        // Tier 3 − tier 2 leaves room for at least one spill batch (§7).
        let spill_room = (tiers.tier3_ratio - tiers.tier2_ratio) * (buffer_bytes as f64);
        if spill_room < spill_batch_bytes as f64 {
            return Err(invalid(SPILL_BATCH_BYTES, ConfigValueProblem::SectionCheck));
        }
        let write_attempts = u32::try_from(reader.count(WRITE_ATTEMPTS)?)
            .map_err(|_| invalid(WRITE_ATTEMPTS, ConfigValueProblem::OutOfRange))?;
        let write_retry_backoff_ms = reader.waits(WRITE_RETRY_BACKOFF_MS)?;
        // One wait before each attempt, the first included (§9.1).
        if usize::try_from(write_attempts).ok() != Some(write_retry_backoff_ms.len()) {
            return Err(invalid(
                WRITE_RETRY_BACKOFF_MS,
                ConfigValueProblem::SectionCheck,
            ));
        }
        // #411 A51: a read stops before its caller stops waiting for it (#422 LN-TUNE: the
        // reply limit in effect), and the close waits longer than one read.
        let writer_reply_timeout_ms = reader.duration(WRITER_REPLY_TIMEOUT_MS)?;
        let read_budget_ms = reader.duration(READ_BUDGET_MS)?;
        if read_budget_ms == 0 || read_budget_ms >= writer_reply_timeout_ms {
            return Err(invalid(READ_BUDGET_MS, ConfigValueProblem::SectionCheck));
        }
        let close_timeout_ms = reader.duration(CLOSE_TIMEOUT_MS)?;
        if close_timeout_ms <= read_budget_ms {
            return Err(invalid(CLOSE_TIMEOUT_MS, ConfigValueProblem::SectionCheck));
        }
        Ok(Self {
            buffer_bytes,
            buffer_records: reader.count(BUFFER_RECORDS)?,
            tiers,
            spill_batch_bytes,
            dump_max_bytes: reader.count(DUMP_MAX_BYTES)?,
            flush_batch_records: reader.count(FLUSH_BATCH_RECORDS)?,
            flush_batch_records_busy: reader.count(FLUSH_BATCH_RECORDS_BUSY)?,
            write_attempts,
            write_retry_backoff_ms,
            flush_attempt_stall_ms: reader.duration(FLUSH_ATTEMPT_STALL_MS)?,
            flush_lag_warn_ms: reader.duration(FLUSH_LAG_WARN_MS)?,
            health_probe_interval_ms: reader.duration(HEALTH_PROBE_INTERVAL_MS)?,
            probe_interval_ms: reader.duration(PROBE_INTERVAL_MS)?,
            probe_sample_keys: reader.count(PROBE_SAMPLE_KEYS)?,
            request_index_records: reader.count(REQUEST_INDEX_RECORDS)?,
            status_gap_window_ms: reader.duration(STATUS_GAP_WINDOW_MS)?,
            tag_workbench_patterns: reader.keys(TAG_WORKBENCH_PATTERNS)?,
            tag_reobserve_prefixes: reader.keys(TAG_REOBSERVE_PREFIXES)?,
            read_budget_ms,
            close_timeout_ms,
            open_timeout_ms: reader.duration(OPEN_TIMEOUT_MS)?,
            open_warn_ratio_milli: reader.count(OPEN_WARN_RATIO_MILLI)?,
            history_page_rows: reader.count(HISTORY_PAGE_ROWS)?,
            writer_reply_timeout_ms,
            open_verify_tail_records: reader.count(OPEN_VERIFY_TAIL_RECORDS)?,
            scrub_rows_per_second: reader.count(SCRUB_ROWS_PER_SECOND)?,
            scrub_chunk_rows: reader.count(SCRUB_CHUNK_ROWS)?,
            scrub_pass_interval_ms: reader.duration(SCRUB_PASS_INTERVAL_MS)?,
        })
    }
}
