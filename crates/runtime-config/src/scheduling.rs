// SPDX-License-Identifier: AGPL-3.0-only

//! The `scheduling` section (Workflow #386 SR-1, frozen scheduling model v1.4.2 §9;
//! `contracts/v012-register.md` §5). The Runtime reads the base number of observe-only retries
//! of a timed-out observation from v0.12.0 on (§2.1 item 4). The keys only 0.12.x slices read
//! (`rerun`, `cost_commit`, `blackout`, `feature_lock`, `not_run`) are parsed, checked and
//! defaulted now, so that the one v0.12.0 configuration revision carries them (order §3.5 item
//! 18). Workflow #405: the random task delay (`random_delay`), whose section-level keys are every
//! instance's defaults (the per-instance keys are in the per-instance section), parsed only.
//! Workflow #384 D2-S2: `fact_refresh.recognition_join_timeout_ms`, how long a successful run
//! waits for the readings it handed to recognition, read by the host when a run joins. Workflow
//! #384 D2-S4: `fact_refresh.enabled`, `min_interval_ms`, `daily_limit`, `failure_backoff_ms` and
//! `max_consecutive_failures`, the pace of the scheduler's recognition maintenance package
//! dispatches, read by the host at every policy cycle. Workflow #384 D2-S5:
//! `fact_refresh.overdue_ms`, how long one instance may wait for one key before the transition
//! recorder reports `fact_reader_overdue`; it has no default (unset, nothing is reported) and is
//! read by the host at every policy cycle. Workflow #336 PKG-S3:
//! `suspension.lift_check_min_interval_ms`, the least time between two measurements of one
//! package's content hash by the suspension lift checks, read by the host at each check.
//! Workflow #386 SR-3: `pack_admission.check_in_check_config`, `check_after_start` and
//! `suspend_codes`, the task package admission check of `check-config` and of the background
//! after start, and the admission codes that suspend a task package, read where they are used.
//! Workflow #411 B15 R3 (model-b15 §5.2, v0.12.3): `remedy.enabled`, `remedy.max_per_round` and
//! `remedy.choices`, the switch, the request bound and the host-wide choice rows of declared
//! remedies (each instance's own list is in the per-instance section and replaces this one as a
//! whole), parsed and checked here; R4 reads them in the evaluation.
//! Workflow #386 SR-4a1 (SCH-M frozen model v1.0 §3.2 D-1, D-4, D-6): a blackout list holds
//! `weekly` entries (the host shape; an entry without `kind` is one) and `once` entries
//! `{kind: "once", id, from_unix_ms, until_unix_ms}`, at most 64 per list; each instance has its
//! own list (`instances[].scheduling.blackout`, key `instance.<instance id>.scheduling.blackout`,
//! no default: an instance whose entry names none has no entries of its own);
//! `blackout.quiet_horizon` (`deadline` | `expected`) and `day_boundary.quiet` (also per
//! instance) are parsed and checked here; SR-4a2 reads them.
//! SCH-M slice ED-1 (frozen model v1.0 §3.4 D-12; Alice 10-22 「近5次该任务包的平均时间加5分钟」):
//! `expected_runtime.enabled`, `expected_runtime.runs` (`1..=16`) and `expected_runtime.margin_ms`
//! (`0..=3600000`), each also per instance (`instance.<instance id>.scheduling.expected_runtime.*`,
//! defaulting to the section's), and `fact_refresh.expected_duration_ms` (`1..=1800000`), the
//! declared duration D of a fact refresh read; parsed and checked here, read by slice ED-2.
//! Workflow #427-A (plan r2 point 6): `parallel.enabled` (default `true` since 0.12.31, Workflow
//! #429 R-2 and Alice's ruling 98; `false` before), whether the policy driver hands each admitted
//! scheduled run to its instance's worker instead of running it on its own thread; read by
//! actingd at the start of every policy round.
//!
//! The section is read whole and checked whole (M387 §5.3). At start and in `check-config` a
//! blackout entry that breaks a rule is `blackout_config_invalid` (with `field`, `blackout_source`
//! naming its list, and `blackout_id` when the entry has a usable id); every other broken rule is
//! `config_value_invalid`. Through an instruction both are `config_value_invalid` and the
//! Runtime keeps its configuration. Every default lives in the defaults file.

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::recovery_ladder::{Listed, listed};
use crate::table::{
    DefaultSource, FilePath, KeyName, KeyRange, KeyRow, Settable, ValueType, instance_key,
};
use actingcommand_contract::FactScalar;
use actingcommand_contract::outcome::BlackoutSource;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

/// The section's name: the first segment of its keys.
pub const SECTION: &str = "scheduling";
/// The base number of observe-only retries of an observation whose sampling timed out (§2.1).
pub const SAMPLE_TIMEOUT_RETRIES: &str = "scheduling.recognition.sample_timeout_retries";
/// The codes that end a run by a condition (§4.1; read at live settlement, SR-2b).
pub const RERUN_CONDITION_CODES: &str = "scheduling.rerun.condition_codes";
/// Condition terminations per occurrence before an ordinary failure (§4.4).
pub const RERUN_MAX_PER_OCCURRENCE: &str = "scheduling.rerun.max_per_occurrence";
/// The highest pacing stage at which an owed rerun runs (§4.3).
pub const RERUN_RESUME_STAGE: &str = "scheduling.rerun.resume_stage";
/// How long the stage must stay at or below the resume stage (§4.3).
pub const RERUN_RESUME_SETTLE_MS: &str = "scheduling.rerun.resume_settle_ms";
/// Whether owed reruns rank first (§4.3).
pub const RERUN_PROMOTE: &str = "scheduling.rerun.promote";
/// Whether a condition termination gives back its counts (§4.2 item 3).
pub const RERUN_REFUND_COUNTS: &str = "scheduling.rerun.refund_counts";
/// When an owed rerun expires (§4.4; one value).
pub const RERUN_EXPIRES: &str = "scheduling.rerun.expires";
/// Whether entry-cost candidates rank first (§4.6).
pub const COST_COMMIT_PROMOTE: &str = "scheduling.cost_commit.promote";
/// The highest pacing stage at which an entry-cost candidate starts (§4.6).
pub const COST_COMMIT_MAX_STAGE: &str = "scheduling.cost_commit.max_stage";
/// The wall-clock margin before an entry-cost step (§4.6).
pub const COST_COMMIT_MARGIN_MS: &str = "scheduling.cost_commit.margin_ms";
/// The host blackout entries (§5.1 b; a list).
pub const BLACKOUT_HOST: &str = "scheduling.blackout.host";
/// The margin of gradual quiet (§5.3).
pub const BLACKOUT_QUIET_MARGIN_MS: &str = "scheduling.blackout.quiet_margin_ms";
/// The hard quiet before a blackout start (§5.3 item 3).
pub const BLACKOUT_HARD_QUIET_MS: &str = "scheduling.blackout.hard_quiet_ms";
/// Whether the safety net stops runs at a blackout start (§5.3 item 5).
pub const BLACKOUT_STOP_IN_FLIGHT_AT_START: &str = "scheduling.blackout.stop_in_flight_at_start";
/// Workflow #386 SR-4a1 (SCH-M §3.2 D-4): the horizon of gradual quiet, the run's hard deadline
/// (`deadline`) or its expected duration (`expected`).
pub const BLACKOUT_QUIET_HORIZON: &str = "scheduling.blackout.quiet_horizon";
/// Workflow #386 SR-4a1 (SCH-M §3.2 D-1): the member of an instance's own blackout list, a list
/// of blackout entries (`instance.<instance id>.scheduling.blackout`; in the file the
/// `scheduling.blackout` member of its `instances[]` entry).
pub const INSTANCE_BLACKOUT: &str = "scheduling.blackout";
/// Workflow #386 SR-4a1 (SCH-M §3.2 D-6): whether a Runtime-started policy run must fit its
/// activity window by its expected duration. Also the member of an instance's own value
/// (`instance.<instance id>.scheduling.day_boundary.quiet`).
pub const DAY_BOUNDARY_QUIET: &str = "scheduling.day_boundary.quiet";
/// How long a recorded locked feature stays valid (§7.2).
pub const FEATURE_LOCK_LOCKED_VALID_FOR_MS: &str = "scheduling.feature_lock.locked_valid_for_ms";
/// The codes a run ends with on a locked feature (§7.2; a list).
pub const FEATURE_LOCK_CODES: &str = "scheduling.feature_lock.codes";
/// The most rows `ac_diagnose` lists under `not_run` (§8.2).
pub const NOT_RUN_DIAGNOSE_ROWS: &str = "scheduling.not_run.diagnose_rows";
/// Workflow #405: whether the scheduler delays each task it dispatches by a random time. Also the
/// member of an instance's own value (`instance.<instance id>.scheduling.random_delay.enabled`).
pub const RANDOM_DELAY_ENABLED: &str = "scheduling.random_delay.enabled";
/// Workflow #405: the shortest random delay, in milliseconds (also a per-instance member).
pub const RANDOM_DELAY_MIN_MS: &str = "scheduling.random_delay.min_ms";
/// Workflow #405: the longest random delay, in milliseconds (also a per-instance member).
pub const RANDOM_DELAY_MAX_MS: &str = "scheduling.random_delay.max_ms";

/// Workflow #384 D2-S2 (D2 model §3.4, §3.5): how long a successful run waits before
/// `Finalizing` for the readings of the frames it handed to recognition.
pub const FACT_REFRESH_RECOGNITION_JOIN_TIMEOUT_MS: &str =
    "scheduling.fact_refresh.recognition_join_timeout_ms";
/// Workflow #384 D2-S4 (D2 model §3.4): whether the scheduler dispatches an instance's
/// recognition maintenance package for the unknown facts its candidates read.
pub const FACT_REFRESH_ENABLED: &str = "scheduling.fact_refresh.enabled";
/// Workflow #384 D2-S4: the least time between two such dispatches on one instance.
pub const FACT_REFRESH_MIN_INTERVAL_MS: &str = "scheduling.fact_refresh.min_interval_ms";
/// Workflow #384 D2-S4: the most such dispatches on one instance in one activity day.
pub const FACT_REFRESH_DAILY_LIMIT: &str = "scheduling.fact_refresh.daily_limit";
/// Workflow #384 D2-S4: the least time from a failed run of the package to its next dispatch.
pub const FACT_REFRESH_FAILURE_BACKOFF_MS: &str = "scheduling.fact_refresh.failure_backoff_ms";
/// Workflow #384 D2-S4: the failed runs in a row after which the package rests for the rest of
/// the activity day.
pub const FACT_REFRESH_MAX_CONSECUTIVE_FAILURES: &str =
    "scheduling.fact_refresh.max_consecutive_failures";
/// SCH-M ED-1 (§3.4 D-12): the declared duration D of a fact refresh read, in milliseconds.
pub const FACT_REFRESH_EXPECTED_DURATION_MS: &str = "scheduling.fact_refresh.expected_duration_ms";
/// SCH-M ED-1 (§3.4 D-12): whether the expected duration E is the mean of a pair's latest
/// successful runs plus the margin (`false`: always the declared value). Also the member of an
/// instance's own value (`instance.<instance id>.scheduling.expected_runtime.enabled`).
pub const EXPECTED_RUNTIME_ENABLED: &str = "scheduling.expected_runtime.enabled";
/// SCH-M ED-1: the most latest runs the mean reads (also a per-instance member).
pub const EXPECTED_RUNTIME_RUNS: &str = "scheduling.expected_runtime.runs";
/// SCH-M ED-1: what is added to the mean, in milliseconds (also a per-instance member).
pub const EXPECTED_RUNTIME_MARGIN_MS: &str = "scheduling.expected_runtime.margin_ms";
/// Workflow #384 D2-S5 (D2 model §3.4, §3.9 item 6): how long one instance's candidates may wait
/// for one key before the transition recorder reports `fact_reader_overdue`. No default: unset,
/// nothing is reported.
pub const FACT_REFRESH_OVERDUE_MS: &str = "scheduling.fact_refresh.overdue_ms";

/// Workflow #336 PKG-S3 (M-PKG §3.4, D1): the least time between two measurements of one
/// package's content hash by the suspension lift checks.
pub const SUSPENSION_LIFT_CHECK_MIN_INTERVAL_MS: &str =
    "scheduling.suspension.lift_check_min_interval_ms";

/// Workflow #386 SR-3 (§2.4 item 2): whether `check-config` admits every package of
/// `policy.procedure_manifest` with this Runtime's kernel.
pub const PACK_ADMISSION_CHECK_IN_CHECK_CONFIG: &str =
    "scheduling.pack_admission.check_in_check_config";
/// Workflow #386 SR-3 (§2.4 item 2): whether the daemon admits them in the background after its
/// start.
pub const PACK_ADMISSION_CHECK_AFTER_START: &str = "scheduling.pack_admission.check_after_start";
/// Workflow #386 SR-3 (§2.4 item 3): the admission codes that suspend a task package (a list).
pub const PACK_ADMISSION_SUSPEND_CODES: &str = "scheduling.pack_admission.suspend_codes";

/// Workflow #411 B15 R3 (model-b15 §3.5, §5.2): whether declared remedies are on; `false` turns
/// every remedy key back into a plain named outcome.
pub const REMEDY_ENABLED: &str = "scheduling.remedy.enabled";
/// Workflow #411 B15 R3 (model-b15 §7.4): the remedy requests per blocked layer and round.
pub const REMEDY_MAX_PER_ROUND: &str = "scheduling.remedy.max_per_round";
/// Workflow #411 B15 R3 (model-b15 §5.1, §5.2): the host-wide remedy choice rows, a list of
/// objects of the shape of the instance fact `session.remedy.choices`. Also the member of an
/// instance's own list (`instance.<instance id>.scheduling.remedy.choices`), which, when set,
/// replaces this list as a whole.
pub const REMEDY_CHOICES: &str = "scheduling.remedy.choices";

/// Workflow #427-A (plan r2 point 6): whether the policy driver runs each admitted scheduled run
/// on its instance's worker (`true`, the default since 0.12.31: one run per instance at a time,
/// different instances at once) or on its own thread, one after another (`false`).
pub const PARALLEL_ENABLED: &str = "scheduling.parallel.enabled";
/// ISO-PE (#419; `actingd-config` revision 20): how long the policy driver waits after an
/// evaluation the Runtime skipped (one whose failure is not global) before it evaluates again.
pub const EVALUATION_RETRY_MS: &str = "scheduling.evaluation.retry_ms";
/// ISO-PE (#419; revision 20): the skipped evaluations in a row after which the Runtime records
/// that the policy evaluation waits for a person (`policy_evaluation_awaiting_person`).
pub const EVALUATION_ESCALATE_AFTER: &str = "scheduling.evaluation.escalate_after";

/// The longest random delay any key may name: one day (the coordinator's order for Workflow
/// #405, 2026-10-14). Schema, not a threshold.
const MAX_RANDOM_DELAY_MS: i64 = 86_400_000;
/// The largest count an integer key of the section may name: the kernel and the readers count
/// in `u32`. Schema, not a threshold.
const MAX_COUNT: i64 = u32::MAX as i64;
/// The pacing stages a stage key may name: the vocabulary `pacing_stage` of the performance
/// model (s0 normal … s4 severe).
pub const PACING_STAGE_TOKENS: &[&str] = &["s0", "s1", "s2", "s3", "s4"];
/// The words of `rerun.expires`.
const EXPIRES_TOKENS: &[&str] = &["window_end"];
/// Workflow #386 SR-4a1: the words of `blackout.quiet_horizon`.
pub const QUIET_HORIZON_TOKENS: &[&str] = &["deadline", "expected"];
/// Workflow #386 SR-4a1 (SCH-M §3.2 D-1): the most entries one blackout list holds. A range, not a
/// threshold.
const MAX_BLACKOUT_ENTRIES: usize = 64;
/// Workflow #386 SR-4a1: the member of a blackout entry that names its kind, and the kinds; an
/// entry without it is `weekly`.
const BLACKOUT_KIND: &str = "kind";
const BLACKOUT_WEEKLY: &str = "weekly";
const BLACKOUT_ONCE: &str = "once";
/// A host blackout entry's bounds: the shape of an activity window
/// (`contracts/scheduling/v2/activity.schema.json`, `window`). Schema.
const MAX_UTC_OFFSET_MINUTES: i64 = 840;
const MAX_MINUTE_OF_DAY: i64 = 1_439;
const MAX_WEEKDAY: i64 = 7;
/// A host blackout id's bound: the outcome key `blackout_id` is an `id` (at most 256 bytes, no
/// control character).
const MAX_BLACKOUT_ID_BYTES: usize = 256;
/// Workflow #411 B15 R3: the most remedy choice rows one list holds, the `record_list` bound of
/// the instance fact whose shape the rows take (model-b15 §5.1). Schema.
const MAX_REMEDY_CHOICE_ROWS: usize = 256;
/// Workflow #411 B15 R3: the longest remedy choice name, `^[a-z][a-z0-9_]{0,63}$` (model-b15
/// §4.1, §5.1). Schema.
const MAX_REMEDY_NAME_BYTES: usize = 64;
/// Workflow #411 B15 R3: the remedy choice row member that names its kind, and the kinds.
const CHOICE_ROW: &str = "row";
const CHOICE_ALLOW: &str = "allow";
const CHOICE_NONE: &str = "none";
const CHOICE_PARAMETER: &str = "parameter";
/// Workflow #411 B15 R3: the members of a remedy choice row besides `row`.
const CHOICE_NEED: &str = "need";
const CHOICE_CAPABILITY: &str = "capability";
const CHOICE_PARAMETER_NAME: &str = "parameter";
const CHOICE_VALUE: &str = "value";
/// Workflow #411 B15 R3: the word no capability may be (model-b15 §4.1).
const CAPABILITY_NEVER: &str = "none";

macro_rules! file_struct {
    ($(#[$meta:meta])* $name:ident { $($member:ident),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Default, Deserialize, Serialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $(
                #[serde(default, skip_serializing_if = "Option::is_none")]
                $member: Option<Value>,
            )+
        }
    };
}

/// The section as the configuration file and the defaults file write it: every member may be
/// left out and none is unknown (M387 §5.3). Each value keeps its JSON here; the key table checks
/// its type and range, the whole-section check its rules.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulingFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recognition: Option<RecognitionFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rerun: Option<RerunFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cost_commit: Option<CostCommitFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    blackout: Option<BlackoutFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    feature_lock: Option<FeatureLockFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    not_run: Option<NotRunFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    random_delay: Option<RandomDelayFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fact_refresh: Option<FactRefreshFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    suspension: Option<SuspensionFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pack_admission: Option<PackAdmissionFile>,
    // Interface anchor `actingd-config` revision 6 (contracts/component-interfaces.md): a Runtime
    // before v0.12.3 refuses a file that names it, because the section denies unknown fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    remedy: Option<RemedyFile>,
    // Interface anchor `actingd-config` revision 11 (contracts/component-interfaces.md; Workflow
    // #386 SR-4a1): a Runtime before SR-4a1 refuses a file that names it, because the section
    // denies unknown fields; a file that does not name it keeps its revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    day_boundary: Option<DayBoundaryFile>,
    // Interface anchor `actingd-config` revision 11 (contracts/component-interfaces.md; SCH-M
    // ED-1): a Runtime before ED-1 refuses a file that names it, because the section denies
    // unknown fields; a file that does not name it keeps its revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_runtime: Option<ExpectedRuntimeFile>,
    // Interface anchor `actingd-config` revision 19 (contracts/component-interfaces.md; Workflow
    // #427-A): a Runtime before #427-A refuses a file that names it, because the section denies
    // unknown fields; a file that does not name it keeps its revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parallel: Option<ParallelFile>,
    // Interface anchor `actingd-config` revision 20 (contracts/component-interfaces.md; ISO-PE,
    // #419): a Runtime before ISO-PE refuses a file that names it, because the section denies
    // unknown fields; a file that does not name it keeps its revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    evaluation: Option<EvaluationFile>,
}

/// Workflow #405: the `scheduling` member of an `instances[]` entry, which holds that instance's
/// own random delay (`instance.<instance id>.scheduling.random_delay.*`). Workflow #411 B15 R3:
/// and its own remedy choice rows (`instance.<instance id>.scheduling.remedy.choices`).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceSchedulingFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    random_delay: Option<RandomDelayFile>,
    // Interface anchor `actingd-config` revision 6 (contracts/component-interfaces.md): a Runtime
    // before v0.12.3 refuses a file that names it, because the entry denies unknown fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    remedy: Option<InstanceRemedyFile>,
    // Interface anchor `actingd-config` revision 11 (contracts/component-interfaces.md; Workflow
    // #386 SR-4a1): the instance's own blackout list and day-boundary switch. A Runtime before
    // SR-4a1 refuses a file that names either, because the entry denies unknown fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    blackout: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    day_boundary: Option<DayBoundaryFile>,
    // Interface anchor `actingd-config` revision 11 (contracts/component-interfaces.md; SCH-M
    // ED-1): the instance's own expected-duration keys. A Runtime before ED-1 refuses a file that
    // names them, because the entry denies unknown fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_runtime: Option<ExpectedRuntimeFile>,
}

file_struct!(RecognitionFile {
    sample_timeout_retries
});
file_struct!(RerunFile {
    condition_codes,
    max_per_occurrence,
    resume_stage,
    resume_settle_ms,
    promote,
    refund_counts,
    expires,
});
file_struct!(CostCommitFile {
    promote,
    max_stage,
    margin_ms
});
file_struct!(BlackoutFile {
    host,
    quiet_margin_ms,
    hard_quiet_ms,
    stop_in_flight_at_start,
    // Interface anchor `actingd-config` revision 11 (Workflow #386 SR-4a1).
    quiet_horizon,
});
file_struct!(FeatureLockFile {
    locked_valid_for_ms,
    codes
});
file_struct!(NotRunFile { diagnose_rows });
file_struct!(
    /// Workflow #384 D2-S2, D2-S4: the fact refresh keys.
    FactRefreshFile {
        enabled,
        min_interval_ms,
        daily_limit,
        failure_backoff_ms,
        max_consecutive_failures,
        recognition_join_timeout_ms,
        overdue_ms,
        // Interface anchor `actingd-config` revision 11 (SCH-M ED-1).
        expected_duration_ms,
    }
);
file_struct!(
    /// Workflow #336 PKG-S3: the suspension lift checks.
    SuspensionFile {
        lift_check_min_interval_ms
    }
);
file_struct!(
    /// Workflow #386 SR-3: the task package admission check and its suspend codes.
    PackAdmissionFile {
        check_in_check_config,
        check_after_start,
        suspend_codes,
    }
);
file_struct!(
    /// Workflow #405: a random delay, section-level or one instance's.
    RandomDelayFile {
        enabled,
        min_ms,
        max_ms
    }
);
file_struct!(
    /// Workflow #411 B15 R3: the remedy keys of the section.
    RemedyFile {
        enabled,
        max_per_round,
        choices
    }
);
file_struct!(
    /// Workflow #411 B15 R3: an instance's own remedy choice rows.
    InstanceRemedyFile { choices }
);
file_struct!(
    /// Workflow #386 SR-4a1 (SCH-M §3.2 D-6): the day-boundary switch, section-level or one
    /// instance's.
    DayBoundaryFile { quiet }
);
file_struct!(
    /// SCH-M ED-1 (§3.4 D-12): the expected-duration keys, section-level or one instance's.
    ExpectedRuntimeFile {
        enabled,
        runs,
        margin_ms
    }
);
file_struct!(
    /// Workflow #427-A: the switch of the per-instance lanes.
    ParallelFile { enabled }
);
file_struct!(
    /// ISO-PE (#419): the wait after a skipped policy evaluation and when it escalates.
    EvaluationFile {
        retry_ms,
        escalate_after
    }
);

const fn row(
    name: &'static str,
    value_type: ValueType,
    range: KeyRange,
    reader: &'static str,
) -> KeyRow {
    KeyRow {
        name: KeyName::Exact(name),
        file_path: FilePath::Key,
        value_type,
        range,
        default: DefaultSource::File,
        // Every key of the section can change while the Runtime runs: each is read where it is
        // used (§9: settings, MCP and CLI read and change them).
        settable: Settable::Yes,
        reader,
        since: "0.12.0",
    }
}

const COUNT: KeyRange = KeyRange::Integer {
    min: 0,
    max: MAX_COUNT,
};
const STAGE: KeyRange = KeyRange::Tokens(PACING_STAGE_TOKENS);
/// Workflow #405: a random delay bound, `0..=86400000` ms.
pub(crate) const DELAY: KeyRange = KeyRange::Integer {
    min: 0,
    max: MAX_RANDOM_DELAY_MS,
};
/// Workflow #384 D2-S2: a join timeout, `0..=1800000` ms: no wait beyond the longest task budget.
const JOIN_TIMEOUT: KeyRange = KeyRange::Integer {
    min: 0,
    max: actingcommand_contract::MAX_CONTAINED_TASK_TIMEOUT_MS as i64,
};
/// Workflow #411 B15 R3: the remedy requests per blocked layer and round, `1..=8` (model-b15
/// §5.2; coordinator decision 5 of 2026-10-19). Schema, not a threshold.
const REMEDY_REQUESTS: KeyRange = KeyRange::Integer { min: 1, max: 8 };
/// SCH-M ED-1 (§3.4, "Configuration"): the latest runs the mean reads, `1..=16`, the bound of a
/// `health.run_duration` row (`d1` … `d16`). Schema, not a threshold.
pub(crate) const EXPECTED_RUNTIME_RUNS_RANGE: KeyRange = KeyRange::Integer { min: 1, max: 16 };
/// SCH-M ED-1: the margin added to the mean, `0..=3600000` ms.
pub(crate) const EXPECTED_RUNTIME_MARGIN_RANGE: KeyRange = KeyRange::Integer {
    min: 0,
    max: 3_600_000,
};
/// SCH-M ED-1: the declared duration of a fact refresh read, `1..=1800000` ms: no longer than the
/// longest task budget.
const FACT_REFRESH_EXPECTED_DURATION: KeyRange = KeyRange::Integer {
    min: 1,
    max: actingcommand_contract::MAX_CONTAINED_TASK_TIMEOUT_MS as i64,
};
/// ISO-PE (#419): the wait after a skipped policy evaluation, `1..=3600000` ms: never zero, so a
/// failure that repeats never turns into a busy loop, and at most an hour.
const EVALUATION_RETRY: KeyRange = KeyRange::Integer {
    min: 1,
    max: 3_600_000,
};
/// ISO-PE (#419): the skipped evaluations in a row before the escalation, at least one.
const EVALUATION_ESCALATE: KeyRange = KeyRange::Integer {
    min: 1,
    max: MAX_COUNT,
};

/// ISO-PE (#419): a key of the `evaluation` member, read from the release that joins
/// `actingd-config` revision 20.
const fn iso_pe_row(name: &'static str, value_type: ValueType, range: KeyRange) -> KeyRow {
    KeyRow {
        since: "0.12.28",
        ..row(name, value_type, range, "ISO-PE")
    }
}

/// Workflow #411 B15 R3: a remedy key's row, read by R4 from v0.12.3 on.
const fn remedy_row(name: &'static str, value_type: ValueType, range: KeyRange) -> KeyRow {
    KeyRow {
        since: "0.12.3",
        ..row(name, value_type, range, "B15-R4")
    }
}

/// Workflow #386 SR-4a1: a key of SCH-M §3.2, parsed and checked from the release that joins
/// `actingd-config` revision 11, read by SR-4a2.
const fn sr4a1_row(name: &'static str, value_type: ValueType, range: KeyRange) -> KeyRow {
    KeyRow {
        since: "0.12.12",
        ..row(name, value_type, range, "SR-4a2")
    }
}

/// SCH-M ED-1: a key of SCH-M §3.4, parsed and checked from the release that joins
/// `actingd-config` revision 11, read by ED-2.
const fn ed1_row(name: &'static str, value_type: ValueType, range: KeyRange) -> KeyRow {
    KeyRow {
        since: "0.12.12",
        ..row(name, value_type, range, "ED-2")
    }
}

/// One row per key of the section (`contracts/v012-register.md` §5), in the model's order.
pub(crate) static ROWS: &[KeyRow] = &[
    row(SAMPLE_TIMEOUT_RETRIES, ValueType::Integer, COUNT, "SR-1"),
    row(
        RERUN_CONDITION_CODES,
        ValueType::List,
        KeyRange::Any,
        "SR-2b",
    ),
    row(RERUN_MAX_PER_OCCURRENCE, ValueType::Integer, COUNT, "SR-2b"),
    row(RERUN_RESUME_STAGE, ValueType::String, STAGE, "SR-2b"),
    row(
        RERUN_RESUME_SETTLE_MS,
        ValueType::DurationMs,
        KeyRange::Any,
        "SR-2b",
    ),
    row(RERUN_PROMOTE, ValueType::Boolean, KeyRange::Any, "SR-2b"),
    row(
        RERUN_REFUND_COUNTS,
        ValueType::Boolean,
        KeyRange::Any,
        "SR-2b",
    ),
    row(
        RERUN_EXPIRES,
        ValueType::String,
        KeyRange::Tokens(EXPIRES_TOKENS),
        "SR-2b",
    ),
    row(
        COST_COMMIT_PROMOTE,
        ValueType::Boolean,
        KeyRange::Any,
        "SR-10",
    ),
    row(COST_COMMIT_MAX_STAGE, ValueType::String, STAGE, "SR-10"),
    row(
        COST_COMMIT_MARGIN_MS,
        ValueType::DurationMs,
        KeyRange::Any,
        "SR-10",
    ),
    row(BLACKOUT_HOST, ValueType::List, KeyRange::Any, "SR-4a"),
    row(
        BLACKOUT_QUIET_MARGIN_MS,
        ValueType::DurationMs,
        KeyRange::Any,
        "SR-4a",
    ),
    row(
        BLACKOUT_HARD_QUIET_MS,
        ValueType::DurationMs,
        KeyRange::Any,
        "SR-4a",
    ),
    row(
        BLACKOUT_STOP_IN_FLIGHT_AT_START,
        ValueType::Boolean,
        KeyRange::Any,
        "SR-4b",
    ),
    sr4a1_row(
        BLACKOUT_QUIET_HORIZON,
        ValueType::String,
        KeyRange::Tokens(QUIET_HORIZON_TOKENS),
    ),
    sr4a1_row(DAY_BOUNDARY_QUIET, ValueType::Boolean, KeyRange::Any),
    row(
        FEATURE_LOCK_LOCKED_VALID_FOR_MS,
        ValueType::DurationMs,
        KeyRange::Any,
        "SR-5a",
    ),
    row(FEATURE_LOCK_CODES, ValueType::List, KeyRange::Any, "SR-5b"),
    row(NOT_RUN_DIAGNOSE_ROWS, ValueType::Integer, COUNT, "SR-6b"),
    row(
        RANDOM_DELAY_ENABLED,
        ValueType::Boolean,
        KeyRange::Any,
        "#405",
    ),
    row(RANDOM_DELAY_MIN_MS, ValueType::DurationMs, DELAY, "#405"),
    row(RANDOM_DELAY_MAX_MS, ValueType::DurationMs, DELAY, "#405"),
    row(
        FACT_REFRESH_ENABLED,
        ValueType::Boolean,
        KeyRange::Any,
        "D2-S4",
    ),
    row(
        FACT_REFRESH_MIN_INTERVAL_MS,
        ValueType::DurationMs,
        KeyRange::Any,
        "D2-S4",
    ),
    row(FACT_REFRESH_DAILY_LIMIT, ValueType::Integer, COUNT, "D2-S4"),
    row(
        FACT_REFRESH_FAILURE_BACKOFF_MS,
        ValueType::DurationMs,
        KeyRange::Any,
        "D2-S4",
    ),
    row(
        FACT_REFRESH_MAX_CONSECUTIVE_FAILURES,
        ValueType::Integer,
        COUNT,
        "D2-S4",
    ),
    row(
        FACT_REFRESH_RECOGNITION_JOIN_TIMEOUT_MS,
        ValueType::DurationMs,
        JOIN_TIMEOUT,
        "D2-S2",
    ),
    // Workflow #384 D2-S5: no default (register §5: unset = not reported), so only a file that
    // names the key carries it; since 0.12.31 (Workflow #429 R-1) an instruction can set it
    // either way, and clearing that instruction leaves it unset again.
    KeyRow {
        default: DefaultSource::None,
        ..row(
            FACT_REFRESH_OVERDUE_MS,
            ValueType::DurationMs,
            KeyRange::Any,
            "D2-S5",
        )
    },
    row(
        SUSPENSION_LIFT_CHECK_MIN_INTERVAL_MS,
        ValueType::DurationMs,
        KeyRange::Any,
        "PKG-S3",
    ),
    row(
        PACK_ADMISSION_CHECK_IN_CHECK_CONFIG,
        ValueType::Boolean,
        KeyRange::Any,
        "SR-3",
    ),
    row(
        PACK_ADMISSION_CHECK_AFTER_START,
        ValueType::Boolean,
        KeyRange::Any,
        "SR-3",
    ),
    row(
        PACK_ADMISSION_SUSPEND_CODES,
        ValueType::List,
        KeyRange::Any,
        "SR-3",
    ),
    remedy_row(REMEDY_ENABLED, ValueType::Boolean, KeyRange::Any),
    remedy_row(REMEDY_MAX_PER_ROUND, ValueType::Integer, REMEDY_REQUESTS),
    remedy_row(REMEDY_CHOICES, ValueType::List, KeyRange::Any),
    ed1_row(EXPECTED_RUNTIME_ENABLED, ValueType::Boolean, KeyRange::Any),
    ed1_row(
        EXPECTED_RUNTIME_RUNS,
        ValueType::Integer,
        EXPECTED_RUNTIME_RUNS_RANGE,
    ),
    ed1_row(
        EXPECTED_RUNTIME_MARGIN_MS,
        ValueType::DurationMs,
        EXPECTED_RUNTIME_MARGIN_RANGE,
    ),
    ed1_row(
        FACT_REFRESH_EXPECTED_DURATION_MS,
        ValueType::DurationMs,
        FACT_REFRESH_EXPECTED_DURATION,
    ),
    // Workflow #427-A: read by actingd at the start of every policy round, so an instruction
    // takes effect at the next round; a run already handed to a worker finishes there.
    KeyRow {
        since: "0.12.22",
        ..row(
            PARALLEL_ENABLED,
            ValueType::Boolean,
            KeyRange::Any,
            "#427-A",
        )
    },
    // ISO-PE (#419): read by the host at every skipped policy evaluation, so an instruction
    // takes effect at the next one.
    iso_pe_row(EVALUATION_RETRY_MS, ValueType::DurationMs, EVALUATION_RETRY),
    iso_pe_row(
        EVALUATION_ESCALATE_AFTER,
        ValueType::Integer,
        EVALUATION_ESCALATE,
    ),
];

/// A pacing stage a stage key names (the vocabulary `pacing_stage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PacingStage {
    S0,
    S1,
    S2,
    S3,
    S4,
}

impl PacingStage {
    fn parse(token: &str) -> Option<Self> {
        match token {
            "s0" => Some(Self::S0),
            "s1" => Some(Self::S1),
            "s2" => Some(Self::S2),
            "s3" => Some(Self::S3),
            "s4" => Some(Self::S4),
            _ => None,
        }
    }
}

/// When an owed rerun expires (§4.4): the end of the pair's bookkeeping window, the only value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RerunExpiry {
    WindowEnd,
}

/// The `rerun` keys (§4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RerunConfig {
    pub condition_codes: Vec<String>,
    pub max_per_occurrence: u32,
    pub resume_stage: PacingStage,
    pub resume_settle_ms: u64,
    pub promote: bool,
    pub refund_counts: bool,
    pub expires: RerunExpiry,
}

/// The `cost_commit` keys (§4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CostCommitConfig {
    pub promote: bool,
    pub max_stage: PacingStage,
    pub margin_ms: u64,
}

/// When one blackout entry covers (`contracts/scheduling-blackout.md`, "Configuration
/// blackouts"; Workflow #386 SR-4a1 adds `once`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlackoutWindow {
    /// §5.1 b: the weekdays it starts on (1–7), the offset of its clock from UTC and its start
    /// and end minutes of day; an end at or before the start crosses midnight.
    Weekly {
        weekdays: Vec<u8>,
        utc_offset_minutes: i16,
        start_minute_of_day: u16,
        end_minute_of_day: u16,
    },
    /// `[from_unix_ms, until_unix_ms)`, non-empty.
    Once {
        from_unix_ms: u64,
        until_unix_ms: u64,
    },
}

/// One entry of a blackout list: the host list or an instance's own (SCH-M §3.2 D-1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlackoutEntry {
    pub id: String,
    pub window: BlackoutWindow,
}

/// Workflow #386 SR-4a1 (SCH-M §3.2 D-4): the horizon of gradual quiet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuietHorizon {
    /// The run's hard response deadline.
    Deadline,
    /// The run's expected duration E.
    Expected,
}

/// The `blackout` keys (§5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlackoutConfig {
    pub host: Vec<BlackoutEntry>,
    pub quiet_margin_ms: u64,
    pub hard_quiet_ms: u64,
    pub stop_in_flight_at_start: bool,
    /// Workflow #386 SR-4a1.
    pub quiet_horizon: QuietHorizon,
}

/// The `feature_lock` keys (§7.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureLockConfig {
    pub locked_valid_for_ms: u64,
    pub codes: Vec<String>,
}

/// Workflow #405: a random delay, section-level or one instance's in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RandomDelay {
    pub enabled: bool,
    pub min_ms: u64,
    pub max_ms: u64,
}

/// Workflow #384 D2-S4: the pace of the scheduler's recognition maintenance package dispatches
/// (D2 model §3.4), read at every policy cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FactRefreshConfig {
    pub enabled: bool,
    pub min_interval_ms: u64,
    pub daily_limit: u32,
    pub failure_backoff_ms: u64,
    pub max_consecutive_failures: u32,
    /// Workflow #384 D2-S5: when one key's wait is reported overdue; `None` when unset.
    pub overdue_ms: Option<u64>,
    /// SCH-M ED-1 (§3.4 D-12): the declared duration D of a fact refresh read.
    pub expected_duration_ms: u64,
}

/// SCH-M ED-1 (§3.4 D-12, "Configuration"): the expected-duration keys in effect, the section's
/// or, from `EffectiveConfig::expected_runtime`, one instance's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpectedRuntimeConfig {
    /// `false`: E is the declared value, and every evaluation is the one without samples.
    pub enabled: bool,
    /// The most latest successful runs the mean reads: `1..=16`.
    pub runs: u8,
    /// What is added to the mean, in milliseconds: `0..=3600000`.
    pub margin_ms: u64,
}

/// Workflow #386 SR-3 (§2.4): the task package admission check of `check-config` and of the
/// background after start, and the admission codes that suspend a task package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackAdmissionConfig {
    pub check_in_check_config: bool,
    pub check_after_start: bool,
    pub suspend_codes: Vec<String>,
}

/// Workflow #411 B15 R3: one remedy choice row (model-b15 §5.1), the shape of a row of the
/// instance fact `session.remedy.choices` and of the configuration's lists. Every name is an
/// opaque token the packages declare; nothing here interprets it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RemedyChoiceRow {
    /// `{"row": "allow", "need", "capability"}`: this source allows `capability` for `need`.
    Allow { need: String, capability: String },
    /// `{"row": "none", "need"}`: this source decides that `need` has no remedy.
    NoRemedy { need: String },
    /// `{"row": "parameter", "capability", "parameter", "value"}`: this source sets the value of
    /// a provider's parameter.
    Parameter {
        capability: String,
        parameter: String,
        value: String,
    },
}

/// Workflow #411 B15 R3 (model-b15 §5.2): the remedy keys in effect, the section's or, from
/// `EffectiveConfig::remedy`, one instance's: the switch, the requests per blocked layer and
/// round, and the choice rows in effect (the instance's own list when it has one, otherwise the
/// section's).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemedyConfig {
    pub enabled: bool,
    pub max_per_round: u8,
    pub rows: Vec<RemedyChoiceRow>,
}

/// ISO-PE (#419): the policy evaluation's retry after an evaluation the Runtime skipped, and the
/// skipped evaluations in a row after which it waits for a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvaluationRetryConfig {
    pub retry_ms: u64,
    pub escalate_after: u32,
}

/// The `scheduling` section in effect, read and checked whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulingConfig {
    pub sample_timeout_retries: u32,
    pub rerun: RerunConfig,
    pub cost_commit: CostCommitConfig,
    pub blackout: BlackoutConfig,
    pub feature_lock: FeatureLockConfig,
    pub not_run_diagnose_rows: u32,
    pub random_delay: RandomDelay,
    pub fact_refresh: FactRefreshConfig,
    pub pack_admission: PackAdmissionConfig,
    /// Workflow #411 B15 R3: the remedy keys, with the section's choice rows.
    pub remedy: RemedyConfig,
    /// Workflow #386 SR-4a1 (SCH-M §3.2 D-6): the section's day-boundary switch.
    pub day_boundary_quiet: bool,
    /// SCH-M ED-1: the section's expected-duration keys.
    pub expected_runtime: ExpectedRuntimeConfig,
    /// Workflow #427-A: whether the policy driver runs scheduled runs on the instance workers.
    pub parallel_enabled: bool,
    /// ISO-PE (#419): the retry and the escalation of a skipped policy evaluation.
    pub evaluation: EvaluationRetryConfig,
}

fn invalid(key: &str, problem: ConfigValueProblem) -> ConfigFailure {
    ConfigFailure::invalid_value(key, problem)
}

/// The values of the section's keys, read one by one from the configuration in effect.
pub(crate) struct Reader<'a> {
    pub(crate) value: &'a dyn Fn(&str) -> Option<FactScalar>,
}

impl Reader<'_> {
    fn scalar(&self, key: &str) -> Result<FactScalar, ConfigFailure> {
        (self.value)(key).ok_or_else(|| invalid(key, ConfigValueProblem::WrongType))
    }

    pub(crate) fn boolean(&self, key: &str) -> Result<bool, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::Boolean(flag) => Ok(flag),
            _ => Err(invalid(key, ConfigValueProblem::WrongType)),
        }
    }

    fn count(&self, key: &str) -> Result<u32, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::Integer(number) => {
                u32::try_from(number).map_err(|_| invalid(key, ConfigValueProblem::OutOfRange))
            }
            _ => Err(invalid(key, ConfigValueProblem::WrongType)),
        }
    }

    pub(crate) fn duration(&self, key: &str) -> Result<u64, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::DurationMs(ms) => Ok(ms),
            _ => Err(invalid(key, ConfigValueProblem::WrongType)),
        }
    }

    /// A duration key without a default: `None` when the configuration does not carry it.
    fn optional_duration(&self, key: &str) -> Result<Option<u64>, ConfigFailure> {
        match (self.value)(key) {
            None => Ok(None),
            Some(FactScalar::DurationMs(ms)) => Ok(Some(ms)),
            Some(_) => Err(invalid(key, ConfigValueProblem::WrongType)),
        }
    }

    fn stage(&self, key: &str) -> Result<PacingStage, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::String(token) => PacingStage::parse(&token)
                .ok_or_else(|| invalid(key, ConfigValueProblem::OutOfRange)),
            _ => Err(invalid(key, ConfigValueProblem::WrongType)),
        }
    }

    fn list(&self, key: &str) -> Result<Vec<Value>, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::String(text) => match serde_json::from_str::<Value>(&text) {
                Ok(Value::Array(items)) => Ok(items),
                _ => Err(invalid(key, ConfigValueProblem::WrongType)),
            },
            _ => Err(invalid(key, ConfigValueProblem::WrongType)),
        }
    }

    /// A list of codes the embedded catalog lists and has not retired: an unlisted code is
    /// `code_unknown`, a retired one `code_retired`.
    fn codes(&self, key: &str) -> Result<Vec<String>, ConfigFailure> {
        self.list(key)?
            .into_iter()
            .map(|entry| {
                let Value::String(code) = entry else {
                    return Err(invalid(key, ConfigValueProblem::WrongType));
                };
                match listed(&code) {
                    Listed::Active => Ok(code),
                    Listed::Retired => Err(invalid(key, ConfigValueProblem::CodeRetired)),
                    Listed::Unlisted => Err(invalid(key, ConfigValueProblem::CodeUnknown)),
                }
            })
            .collect()
    }
}

/// Workflow #405: a random delay from its three keys (`enabled`, `min_ms`, `max_ms`, spelled by
/// `keys`), the bounds in order. `min_ms` above `max_ms` is `config_value_invalid` for the
/// `min_ms` key, `section_check`: the pair is checked as it is in effect, so changing one side
/// alone is checked against the other.
pub(crate) fn random_delay(
    reader: &Reader<'_>,
    keys: [&str; 3],
) -> Result<RandomDelay, ConfigFailure> {
    let [enabled, min_ms, max_ms] = keys;
    let delay = RandomDelay {
        enabled: reader.boolean(enabled)?,
        min_ms: reader.duration(min_ms)?,
        max_ms: reader.duration(max_ms)?,
    };
    // Each bound's own range (`0..=86400000`) is the key table's, checked wherever a value
    // enters (the file layer, the defaults file, an instruction).
    if delay.min_ms > delay.max_ms {
        return Err(invalid(min_ms, ConfigValueProblem::SectionCheck));
    }
    Ok(delay)
}

/// Workflow #405 D1: the random delay in effect for the instance whose registry id is
/// `instance_id`: each member the instance's own key
/// (`instance.<instance_id>.scheduling.random_delay.<member>`) when the configuration carries it,
/// otherwise the section's, the pair checked as it is in effect (as SR-1 checks it).
pub(crate) fn instance_random_delay(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    instance_id: &str,
) -> Result<RandomDelay, ConfigFailure> {
    let reader = Reader { value };
    let [enabled, min_ms, max_ms] = [
        RANDOM_DELAY_ENABLED,
        RANDOM_DELAY_MIN_MS,
        RANDOM_DELAY_MAX_MS,
    ]
    .map(|member| {
        let own = instance_key(instance_id, member);
        if value(&own).is_some() {
            own
        } else {
            member.to_owned()
        }
    });
    random_delay(
        &reader,
        [enabled.as_str(), min_ms.as_str(), max_ms.as_str()],
    )
}

/// One `weekly` blackout entry as written: every member required (but `kind`, absent or
/// `weekly`), none unknown.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WeeklyBlackoutFile {
    #[serde(default, rename = "kind")]
    _kind: Option<Value>,
    id: Value,
    weekdays: Value,
    utc_offset_minutes: Value,
    start_minute_of_day: Value,
    end_minute_of_day: Value,
}

/// One `once` blackout entry as written: every member required, none unknown.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OnceBlackoutFile {
    #[serde(rename = "kind")]
    _kind: Value,
    id: Value,
    from_unix_ms: Value,
    until_unix_ms: Value,
}

/// Workflow #386 SR-4a1: where a blackout list lives: the key that holds it, the list it is
/// (vocabulary `blackout_source`) and the JSON pointer of the list in the configuration file.
pub(crate) struct BlackoutList<'a> {
    pub(crate) config_key: &'a str,
    pub(crate) source: BlackoutSource,
    pub(crate) pointer: String,
}

impl BlackoutList<'_> {
    /// The JSON pointer of a member of entry `index`, or of the entry itself.
    fn entry_pointer(&self, index: usize, member: Option<&str>) -> String {
        let entry = format!("{}/{index}", self.pointer);
        match member {
            Some(member) => format!("{entry}/{member}"),
            None => entry,
        }
    }
}

/// An integer within `min..=max`.
fn bounded(value: &Value, min: i64, max: i64) -> Option<i64> {
    value.as_i64().filter(|number| (min..=max).contains(number))
}

/// A usable blackout id: one to 256 bytes, no control character.
fn blackout_id(value: &Value) -> Option<&str> {
    value.as_str().filter(|id| {
        (1..=MAX_BLACKOUT_ID_BYTES).contains(&id.len()) && !id.chars().any(char::is_control)
    })
}

/// The entries of one blackout list (`contracts/scheduling-blackout.md`, "Configuration
/// blackouts"): at most 64; each a `weekly` entry (an object of `id`, `weekdays`,
/// `utc_offset_minutes`, `start_minute_of_day` and `end_minute_of_day`, with `kind` absent or
/// `weekly`) or a `once` entry (`kind` `once`, `id`, `from_unix_ms` and a later
/// `until_unix_ms`); ids unique in the list. A broken rule is `blackout_config_invalid` with the
/// member it concerns and the list it is in.
pub(crate) fn blackout_entries(
    list: &BlackoutList<'_>,
    items: Vec<Value>,
) -> Result<Vec<BlackoutEntry>, ConfigFailure> {
    let mut ids = BTreeSet::new();
    let mut entries = Vec::with_capacity(items.len().min(MAX_BLACKOUT_ENTRIES));
    for (index, item) in items.into_iter().enumerate() {
        let id = item.get("id").and_then(blackout_id).map(str::to_owned);
        let refused = |member: Option<&str>| {
            ConfigFailure::blackout(
                list.config_key,
                list.entry_pointer(index, member),
                id.clone(),
                list.source,
            )
        };
        if index >= MAX_BLACKOUT_ENTRIES {
            return Err(refused(None));
        }
        let unique_id = |value: &Value| match blackout_id(value) {
            Some(entry_id) if !ids.contains(entry_id) => Ok(entry_id.to_owned()),
            _ => Err(refused(Some("id"))),
        };
        let (entry_id, window) = match item.get(BLACKOUT_KIND) {
            None => weekly_entry(&item, &unique_id, &refused)?,
            Some(Value::String(kind)) if kind == BLACKOUT_WEEKLY => {
                weekly_entry(&item, &unique_id, &refused)?
            }
            Some(Value::String(kind)) if kind == BLACKOUT_ONCE => {
                once_entry(&item, &unique_id, &refused)?
            }
            Some(_) => return Err(refused(Some(BLACKOUT_KIND))),
        };
        ids.insert(entry_id.clone());
        entries.push(BlackoutEntry {
            id: entry_id,
            window,
        });
    }
    Ok(entries)
}

/// One `weekly` entry: the shape and bounds of an activity window
/// (`contracts/scheduling/v2/activity.schema.json`, `window`).
fn weekly_entry(
    item: &Value,
    unique_id: &dyn Fn(&Value) -> Result<String, ConfigFailure>,
    refused: &dyn Fn(Option<&str>) -> ConfigFailure,
) -> Result<(String, BlackoutWindow), ConfigFailure> {
    let entry =
        serde_json::from_value::<WeeklyBlackoutFile>(item.clone()).map_err(|_| refused(None))?;
    let entry_id = unique_id(&entry.id)?;
    let weekdays = entry
        .weekdays
        .as_array()
        .filter(|days| (1..=MAX_WEEKDAY as usize).contains(&days.len()))
        .and_then(|days| {
            days.iter()
                .map(|day| bounded(day, 1, MAX_WEEKDAY).and_then(|day| u8::try_from(day).ok()))
                .collect::<Option<Vec<_>>>()
        })
        .filter(|days| days.iter().collect::<BTreeSet<_>>().len() == days.len())
        .ok_or_else(|| refused(Some("weekdays")))?;
    let utc_offset_minutes = bounded(
        &entry.utc_offset_minutes,
        -MAX_UTC_OFFSET_MINUTES,
        MAX_UTC_OFFSET_MINUTES,
    )
    .and_then(|offset| i16::try_from(offset).ok())
    .ok_or_else(|| refused(Some("utc_offset_minutes")))?;
    let minute = |value: &Value, member: &str| {
        bounded(value, 0, MAX_MINUTE_OF_DAY)
            .and_then(|minute| u16::try_from(minute).ok())
            .ok_or_else(|| refused(Some(member)))
    };
    let start_minute_of_day = minute(&entry.start_minute_of_day, "start_minute_of_day")?;
    let end_minute_of_day = minute(&entry.end_minute_of_day, "end_minute_of_day")?;
    Ok((
        entry_id,
        BlackoutWindow::Weekly {
            weekdays,
            utc_offset_minutes,
            start_minute_of_day,
            end_minute_of_day,
        },
    ))
}

/// One `once` entry: `[from_unix_ms, until_unix_ms)`, half-open and non-empty.
fn once_entry(
    item: &Value,
    unique_id: &dyn Fn(&Value) -> Result<String, ConfigFailure>,
    refused: &dyn Fn(Option<&str>) -> ConfigFailure,
) -> Result<(String, BlackoutWindow), ConfigFailure> {
    let entry =
        serde_json::from_value::<OnceBlackoutFile>(item.clone()).map_err(|_| refused(None))?;
    let entry_id = unique_id(&entry.id)?;
    let from_unix_ms = entry
        .from_unix_ms
        .as_u64()
        .ok_or_else(|| refused(Some("from_unix_ms")))?;
    let until_unix_ms = entry
        .until_unix_ms
        .as_u64()
        .filter(|until| *until > from_unix_ms)
        .ok_or_else(|| refused(Some("until_unix_ms")))?;
    Ok((
        entry_id,
        BlackoutWindow::Once {
            from_unix_ms,
            until_unix_ms,
        },
    ))
}

/// Workflow #386 SR-4a1: `blackout.quiet_horizon` from `reader`.
fn quiet_horizon(reader: &Reader<'_>) -> Result<QuietHorizon, ConfigFailure> {
    match reader.scalar(BLACKOUT_QUIET_HORIZON)? {
        FactScalar::String(token) if token == QUIET_HORIZON_TOKENS[0] => Ok(QuietHorizon::Deadline),
        FactScalar::String(token) if token == QUIET_HORIZON_TOKENS[1] => Ok(QuietHorizon::Expected),
        FactScalar::String(_) => Err(invalid(
            BLACKOUT_QUIET_HORIZON,
            ConfigValueProblem::OutOfRange,
        )),
        _ => Err(invalid(
            BLACKOUT_QUIET_HORIZON,
            ConfigValueProblem::WrongType,
        )),
    }
}

/// Workflow #386 SR-4a1 (SCH-M §3.2 D-1): the entries of the own blackout list of the instance
/// whose registry id is `instance_id`, the `index`-th entry of the file's `instances`; none when
/// the configuration carries no list for it.
pub(crate) fn instance_blackouts(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    instance_id: &str,
    index: usize,
) -> Result<Vec<BlackoutEntry>, ConfigFailure> {
    let key = instance_key(instance_id, INSTANCE_BLACKOUT);
    if value(&key).is_none() {
        return Ok(Vec::new());
    }
    let items = Reader { value }.list(&key)?;
    blackout_entries(
        &BlackoutList {
            config_key: &key,
            source: BlackoutSource::Instance,
            pointer: format!("/instances/{index}/scheduling/blackout"),
        },
        items,
    )
}

/// Workflow #386 SR-4a1 (SCH-M §3.2 D-6): the day-boundary switch in effect for the instance
/// whose registry id is `instance_id`: its own key when the configuration carries it, otherwise
/// the section's.
pub(crate) fn day_boundary_quiet(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    instance_id: &str,
) -> Result<bool, ConfigFailure> {
    let own = instance_key(instance_id, DAY_BOUNDARY_QUIET);
    let reader = Reader { value };
    if value(&own).is_some() {
        reader.boolean(&own)
    } else {
        reader.boolean(DAY_BOUNDARY_QUIET)
    }
}

impl SchedulingConfig {
    /// The section from `value` (the configuration in effect), checked whole (M387 §5.3).
    pub fn parse(value: &dyn Fn(&str) -> Option<FactScalar>) -> Result<Self, ConfigFailure> {
        let reader = Reader { value };
        let resume_stage = reader.stage(RERUN_RESUME_STAGE)?;
        let expires = match reader.scalar(RERUN_EXPIRES)? {
            FactScalar::String(token) if token == EXPIRES_TOKENS[0] => RerunExpiry::WindowEnd,
            FactScalar::String(_) => {
                return Err(invalid(RERUN_EXPIRES, ConfigValueProblem::OutOfRange));
            }
            _ => return Err(invalid(RERUN_EXPIRES, ConfigValueProblem::WrongType)),
        };
        Ok(Self {
            sample_timeout_retries: reader.count(SAMPLE_TIMEOUT_RETRIES)?,
            rerun: RerunConfig {
                condition_codes: reader.codes(RERUN_CONDITION_CODES)?,
                max_per_occurrence: reader.count(RERUN_MAX_PER_OCCURRENCE)?,
                resume_stage,
                resume_settle_ms: reader.duration(RERUN_RESUME_SETTLE_MS)?,
                promote: reader.boolean(RERUN_PROMOTE)?,
                refund_counts: reader.boolean(RERUN_REFUND_COUNTS)?,
                expires,
            },
            cost_commit: CostCommitConfig {
                promote: reader.boolean(COST_COMMIT_PROMOTE)?,
                max_stage: reader.stage(COST_COMMIT_MAX_STAGE)?,
                margin_ms: reader.duration(COST_COMMIT_MARGIN_MS)?,
            },
            blackout: BlackoutConfig {
                host: blackout_entries(
                    &BlackoutList {
                        config_key: BLACKOUT_HOST,
                        source: BlackoutSource::Host,
                        pointer: "/scheduling/blackout/host".to_owned(),
                    },
                    reader.list(BLACKOUT_HOST)?,
                )?,
                quiet_margin_ms: reader.duration(BLACKOUT_QUIET_MARGIN_MS)?,
                hard_quiet_ms: reader.duration(BLACKOUT_HARD_QUIET_MS)?,
                stop_in_flight_at_start: reader.boolean(BLACKOUT_STOP_IN_FLIGHT_AT_START)?,
                quiet_horizon: quiet_horizon(&reader)?,
            },
            feature_lock: FeatureLockConfig {
                locked_valid_for_ms: reader.duration(FEATURE_LOCK_LOCKED_VALID_FOR_MS)?,
                codes: reader.codes(FEATURE_LOCK_CODES)?,
            },
            not_run_diagnose_rows: reader.count(NOT_RUN_DIAGNOSE_ROWS)?,
            random_delay: random_delay(
                &reader,
                [
                    RANDOM_DELAY_ENABLED,
                    RANDOM_DELAY_MIN_MS,
                    RANDOM_DELAY_MAX_MS,
                ],
            )?,
            fact_refresh: fact_refresh_from(&reader)?,
            pack_admission: pack_admission_from(&reader)?,
            remedy: remedy_from(&reader)?,
            day_boundary_quiet: reader.boolean(DAY_BOUNDARY_QUIET)?,
            expected_runtime: expected_runtime_from(
                &reader,
                [
                    EXPECTED_RUNTIME_ENABLED,
                    EXPECTED_RUNTIME_RUNS,
                    EXPECTED_RUNTIME_MARGIN_MS,
                ],
            )?,
            parallel_enabled: reader.boolean(PARALLEL_ENABLED)?,
            evaluation: EvaluationRetryConfig {
                retry_ms: reader.duration(EVALUATION_RETRY_MS)?,
                escalate_after: reader.count(EVALUATION_ESCALATE_AFTER)?,
            },
        })
    }
}

/// Workflow #427-A: `scheduling.parallel.enabled` in effect.
pub(crate) fn parallel_enabled(
    value: &dyn Fn(&str) -> Option<FactScalar>,
) -> Result<bool, ConfigFailure> {
    Reader { value }.boolean(PARALLEL_ENABLED)
}

/// Workflow #411 B15 R3: the section's remedy keys from `reader`. `max_per_round`'s range
/// (`1..=8`) is the key table's, checked wherever a value enters.
fn remedy_from(reader: &Reader<'_>) -> Result<RemedyConfig, ConfigFailure> {
    let max_per_round = u8::try_from(reader.count(REMEDY_MAX_PER_ROUND)?)
        .map_err(|_| invalid(REMEDY_MAX_PER_ROUND, ConfigValueProblem::OutOfRange))?;
    Ok(RemedyConfig {
        enabled: reader.boolean(REMEDY_ENABLED)?,
        max_per_round,
        rows: remedy_choices(reader, REMEDY_CHOICES)?,
    })
}

/// Workflow #411 B15 R3 (model-b15 §5.2): the remedy keys in effect for the instance whose
/// registry id is `instance_id`: the section's switch and request bound, and the instance's own
/// choice rows when its key is in effect, otherwise the section's. An instance's list replaces
/// the section's as a whole; the two are never merged.
pub(crate) fn remedy(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    instance_id: &str,
) -> Result<RemedyConfig, ConfigFailure> {
    let reader = Reader { value };
    let mut remedy = remedy_from(&reader)?;
    let own = instance_key(instance_id, REMEDY_CHOICES);
    if value(&own).is_some() {
        remedy.rows = remedy_choices(&reader, &own)?;
    }
    Ok(remedy)
}

/// Workflow #411 B15 R3 (model-b15 §5.1): the remedy choice rows of the list key `key`, checked.
/// A broken rule is `config_value_invalid` for `key`, with the JSON pointer of the row (and
/// member) in the key's value:
///
/// - more than 256 rows: `out_of_range`, at the first row beyond the bound;
/// - a row that is not an object, a `row` or a field that is missing or not a string, or a
///   member its kind does not have: `wrong_type`;
/// - a `row` other than `allow`, `none` and `parameter`, a name outside `^[a-z][a-z0-9_]{0,63}$`,
///   or `none` as a capability: `out_of_range`;
/// - a row twice, a `none` and an `allow` of the same need, or two `parameter` rows of the same
///   capability and parameter with different values: `section_check`, at the later row.
///
/// Whether the packages declare the names is not checked here (R7).
pub(crate) fn remedy_choices(
    reader: &Reader<'_>,
    key: &str,
) -> Result<Vec<RemedyChoiceRow>, ConfigFailure> {
    let items = reader.list(key)?;
    if items.len() > MAX_REMEDY_CHOICE_ROWS {
        return Err(choice_refused(
            key,
            MAX_REMEDY_CHOICE_ROWS,
            None,
            ConfigValueProblem::OutOfRange,
        ));
    }
    let mut rows = Vec::with_capacity(items.len());
    let mut seen = BTreeSet::new();
    let mut allowed = BTreeSet::new();
    let mut declined = BTreeSet::new();
    let mut parameters = BTreeSet::new();
    for (index, item) in items.iter().enumerate() {
        let row = choice_row(key, index, item)?;
        let conflict = match &row {
            RemedyChoiceRow::Allow { need, .. } => {
                allowed.insert(need.clone());
                declined.contains(need)
            }
            RemedyChoiceRow::NoRemedy { need } => {
                declined.insert(need.clone());
                allowed.contains(need)
            }
            // Two values for one parameter: nothing says which wins. (The same value again is a
            // row twice.)
            RemedyChoiceRow::Parameter {
                capability,
                parameter,
                ..
            } => !parameters.insert((capability.clone(), parameter.clone())),
        };
        if conflict || !seen.insert(row.clone()) {
            return Err(choice_refused(
                key,
                index,
                None,
                ConfigValueProblem::SectionCheck,
            ));
        }
        rows.push(row);
    }
    Ok(rows)
}

/// Workflow #411 B15 R3: `config_value_invalid` for `key` at row `index` (and `member`).
fn choice_refused(
    key: &str,
    index: usize,
    member: Option<&str>,
    problem: ConfigValueProblem,
) -> ConfigFailure {
    let row = format!("/{index}");
    let pointer = match member {
        // RFC 6901: `~` and `/` in a member name are escaped.
        Some(member) => format!("{row}/{}", member.replace('~', "~0").replace('/', "~1")),
        None => row,
    };
    invalid(key, problem).with_pointer(pointer)
}

/// Workflow #411 B15 R3: a remedy choice name, `^[a-z][a-z0-9_]{0,63}$`.
fn choice_name(name: &str) -> bool {
    name.len() <= MAX_REMEDY_NAME_BYTES
        && name
            .bytes()
            .next()
            .is_some_and(|first| first.is_ascii_lowercase())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// Workflow #411 B15 R3: one remedy choice row at `index` of `key`'s list: an object whose `row`
/// names its kind and whose other members are exactly that kind's fields, each a name.
fn choice_row(key: &str, index: usize, item: &Value) -> Result<RemedyChoiceRow, ConfigFailure> {
    let refused = |member: Option<&str>, problem: ConfigValueProblem| {
        choice_refused(key, index, member, problem)
    };
    let Value::Object(members) = item else {
        return Err(refused(None, ConfigValueProblem::WrongType));
    };
    let Some(Value::String(kind)) = members.get(CHOICE_ROW) else {
        return Err(refused(Some(CHOICE_ROW), ConfigValueProblem::WrongType));
    };
    let fields: &[&str] = match kind.as_str() {
        CHOICE_ALLOW => &[CHOICE_NEED, CHOICE_CAPABILITY],
        CHOICE_NONE => &[CHOICE_NEED],
        CHOICE_PARAMETER => &[CHOICE_CAPABILITY, CHOICE_PARAMETER_NAME, CHOICE_VALUE],
        _ => return Err(refused(Some(CHOICE_ROW), ConfigValueProblem::OutOfRange)),
    };
    if let Some(other) = members
        .keys()
        .find(|member| member.as_str() != CHOICE_ROW && !fields.contains(&member.as_str()))
    {
        return Err(refused(Some(other.as_str()), ConfigValueProblem::WrongType));
    }
    let mut names = Vec::with_capacity(fields.len());
    for &field in fields {
        let Some(Value::String(name)) = members.get(field) else {
            return Err(refused(Some(field), ConfigValueProblem::WrongType));
        };
        if !choice_name(name) || (field == CHOICE_CAPABILITY && name == CAPABILITY_NEVER) {
            return Err(refused(Some(field), ConfigValueProblem::OutOfRange));
        }
        names.push(name.clone());
    }
    Ok(match (kind.as_str(), names.as_slice()) {
        (CHOICE_ALLOW, [need, capability]) => RemedyChoiceRow::Allow {
            need: need.clone(),
            capability: capability.clone(),
        },
        (CHOICE_NONE, [need]) => RemedyChoiceRow::NoRemedy { need: need.clone() },
        (CHOICE_PARAMETER, [capability, parameter, value]) => RemedyChoiceRow::Parameter {
            capability: capability.clone(),
            parameter: parameter.clone(),
            value: value.clone(),
        },
        _ => return Err(refused(None, ConfigValueProblem::WrongType)),
    })
}

/// Workflow #386 SR-3: the `pack_admission` keys from `reader`; a suspend code the catalog does
/// not list is `code_unknown`, a retired one `code_retired`.
fn pack_admission_from(reader: &Reader<'_>) -> Result<PackAdmissionConfig, ConfigFailure> {
    Ok(PackAdmissionConfig {
        check_in_check_config: reader.boolean(PACK_ADMISSION_CHECK_IN_CHECK_CONFIG)?,
        check_after_start: reader.boolean(PACK_ADMISSION_CHECK_AFTER_START)?,
        suspend_codes: reader.codes(PACK_ADMISSION_SUSPEND_CODES)?,
    })
}

/// Workflow #386 SR-3: the `pack_admission` keys in effect, read alone (the host reads them at
/// each admission refusal and at its background check; `check-config` reads them once).
pub(crate) fn pack_admission(
    value: &dyn Fn(&str) -> Option<FactScalar>,
) -> Result<PackAdmissionConfig, ConfigFailure> {
    pack_admission_from(&Reader { value })
}

/// Workflow #384 D2-S4: the fact refresh keys from `reader`, each by the key table's type and
/// range (no rule binds two of them).
fn fact_refresh_from(reader: &Reader<'_>) -> Result<FactRefreshConfig, ConfigFailure> {
    Ok(FactRefreshConfig {
        enabled: reader.boolean(FACT_REFRESH_ENABLED)?,
        min_interval_ms: reader.duration(FACT_REFRESH_MIN_INTERVAL_MS)?,
        daily_limit: reader.count(FACT_REFRESH_DAILY_LIMIT)?,
        failure_backoff_ms: reader.duration(FACT_REFRESH_FAILURE_BACKOFF_MS)?,
        max_consecutive_failures: reader.count(FACT_REFRESH_MAX_CONSECUTIVE_FAILURES)?,
        overdue_ms: reader.optional_duration(FACT_REFRESH_OVERDUE_MS)?,
        expected_duration_ms: reader.duration(FACT_REFRESH_EXPECTED_DURATION_MS)?,
    })
}

/// SCH-M ED-1: the expected-duration keys spelled by `keys` (`enabled`, `runs`, `margin_ms`).
/// Each range is the key table's, checked wherever a value enters.
fn expected_runtime_from(
    reader: &Reader<'_>,
    keys: [&str; 3],
) -> Result<ExpectedRuntimeConfig, ConfigFailure> {
    let [enabled, runs, margin_ms] = keys;
    Ok(ExpectedRuntimeConfig {
        enabled: reader.boolean(enabled)?,
        runs: u8::try_from(reader.count(runs)?)
            .map_err(|_| invalid(runs, ConfigValueProblem::OutOfRange))?,
        margin_ms: reader.duration(margin_ms)?,
    })
}

/// SCH-M ED-1 (§3.4 "Configuration"): the expected-duration keys in effect for the instance whose
/// registry id is `instance_id`: each member the instance's own key
/// (`instance.<instance_id>.scheduling.expected_runtime.<member>`) when the configuration carries
/// it, otherwise the section's.
pub(crate) fn instance_expected_runtime(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    instance_id: &str,
) -> Result<ExpectedRuntimeConfig, ConfigFailure> {
    let keys = [
        EXPECTED_RUNTIME_ENABLED,
        EXPECTED_RUNTIME_RUNS,
        EXPECTED_RUNTIME_MARGIN_MS,
    ]
    .map(|member| {
        let own = instance_key(instance_id, member);
        if value(&own).is_some() {
            own
        } else {
            member.to_owned()
        }
    });
    let [enabled, runs, margin_ms] = &keys;
    expected_runtime_from(
        &Reader { value },
        [enabled.as_str(), runs.as_str(), margin_ms.as_str()],
    )
}

/// Workflow #384 D2-S4: the fact refresh keys in effect, read alone (the host reads them at
/// every policy cycle).
pub(crate) fn fact_refresh(
    value: &dyn Fn(&str) -> Option<FactScalar>,
) -> Result<FactRefreshConfig, ConfigFailure> {
    fact_refresh_from(&Reader { value })
}

/// `scheduling.recognition.sample_timeout_retries` in effect, read alone (the host reads it at
/// each step, §2.1 item 4).
pub(crate) fn sample_timeout_retries(
    value: &dyn Fn(&str) -> Option<FactScalar>,
) -> Result<u32, ConfigFailure> {
    Reader { value }.count(SAMPLE_TIMEOUT_RETRIES)
}

/// Workflow #384 D2-S2: `scheduling.fact_refresh.recognition_join_timeout_ms` in effect, read
/// alone (the host reads it when a run joins its readings).
pub(crate) fn recognition_join_timeout_ms(
    value: &dyn Fn(&str) -> Option<FactScalar>,
) -> Result<u64, ConfigFailure> {
    Reader { value }.duration(FACT_REFRESH_RECOGNITION_JOIN_TIMEOUT_MS)
}

/// Workflow #336 PKG-S3: `scheduling.suspension.lift_check_min_interval_ms` in effect, read alone
/// (the host reads it at each suspension lift check).
pub(crate) fn lift_check_min_interval_ms(
    value: &dyn Fn(&str) -> Option<FactScalar>,
) -> Result<u64, ConfigFailure> {
    Reader { value }.duration(SUSPENSION_LIFT_CHECK_MIN_INTERVAL_MS)
}
