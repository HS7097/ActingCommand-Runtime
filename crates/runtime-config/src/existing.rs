// SPDX-License-Identifier: AGPL-3.0-only

//! The keys the start-up assembly of actingd parsed and reported before D1b (Workflow #387 model
//! v1.2 §5.5 last table row, §1.3). Their parsing and defaults stay where they are until a slice
//! moves their section (§5.1); here they are hung in the key table, none settable while the
//! Runtime runs (§12 question 2): to change one, change the configuration file, commit it with
//! acsetup and restart.

use crate::table::{
    DefaultSource, FilePath, KeyName, KeyRange, KeyRow, NotSettable, Settable, ValueType,
};

/// The release from which these keys have their rows.
const SINCE: &str = "0.12.0";
/// The reader of every key here: the start-up assembly.
const READER: &str = "actingd";

const fn row(
    name: &'static str,
    file_path: FilePath,
    value_type: ValueType,
    default: DefaultSource,
    why_not: NotSettable,
) -> KeyRow {
    KeyRow {
        name: KeyName::Exact(name),
        file_path,
        value_type,
        range: KeyRange::Any,
        default,
        settable: Settable::No(why_not),
        reader: READER,
        since: SINCE,
    }
}

use DefaultSource::{Code, None as NoDefault};
use FilePath::{Absent, Key};
use NotSettable::{BeforeLedgerOpen, Derived, NoFileMember, ReadAtAssembly};
use ValueType::{Boolean, DurationMs, Integer, String as Text};

/// One row per key of the manifest `apps/actingd/src/config/manifest.rs` builds.
pub(crate) static ROWS: &[KeyRow] = &[
    row("bind_host", Key, Text, NoDefault, BeforeLedgerOpen),
    row("bind_port", Key, Integer, Code, BeforeLedgerOpen),
    row("device_diagnostic_mode", Key, Text, Code, ReadAtAssembly),
    row(
        "frame_retention_enabled",
        Key,
        Boolean,
        Code,
        ReadAtAssembly,
    ),
    row(
        "frame_retention_dedup_error",
        Key,
        Boolean,
        Code,
        ReadAtAssembly,
    ),
    row(
        "frame_retention_dedup_lab",
        Key,
        Boolean,
        Code,
        ReadAtAssembly,
    ),
    row(
        "secret_fingerprint_salt_bytes",
        Absent,
        Integer,
        NoDefault,
        Derived,
    ),
    row("allow_env_overrides", Key, Boolean, Code, ReadAtAssembly),
    row(
        "governance.allowed_clients",
        Key,
        Text,
        Code,
        ReadAtAssembly,
    ),
    row(
        "policy.catalog_transition.kind",
        Key,
        Text,
        NoDefault,
        ReadAtAssembly,
    ),
    row(
        "policy.catalog_transition.expected_active_catalog_hash",
        Key,
        Text,
        NoDefault,
        ReadAtAssembly,
    ),
    row(
        "policy.monitor.io_timeout_ms",
        Key,
        DurationMs,
        Code,
        ReadAtAssembly,
    ),
    row(
        "policy.monitor.retry_limit",
        Key,
        Integer,
        Code,
        ReadAtAssembly,
    ),
    row(
        "policy.monitor.retry_backoff_ms",
        Key,
        DurationMs,
        Code,
        ReadAtAssembly,
    ),
    row(
        "policy.monitor.retry_backoff_max_ms",
        Key,
        DurationMs,
        Code,
        ReadAtAssembly,
    ),
    row("mumu_root", Key, Text, NoDefault, ReadAtAssembly),
    row(
        "device_paths.nemu_folder",
        Key,
        Text,
        NoDefault,
        ReadAtAssembly,
    ),
    row(
        "device_paths.nemu_ipc_dll",
        Key,
        Text,
        NoDefault,
        ReadAtAssembly,
    ),
    row(
        "device_paths.droidcast_apk",
        Key,
        Text,
        NoDefault,
        ReadAtAssembly,
    ),
    row(
        "device_paths.minitouch_path",
        Key,
        Text,
        NoDefault,
        ReadAtAssembly,
    ),
    row(
        "device_paths.maatouch_path",
        Key,
        Text,
        NoDefault,
        ReadAtAssembly,
    ),
    row("instances_count", Absent, Integer, NoDefault, Derived),
    row(
        "instances_deferred_count",
        Absent,
        Integer,
        NoDefault,
        Derived,
    ),
    row(
        "instances_startup_package_count",
        Absent,
        Integer,
        NoDefault,
        Derived,
    ),
    row(
        "scheduler.maximum_client_heartbeat_interval_ms",
        Absent,
        DurationMs,
        Code,
        NoFileMember,
    ),
    row(
        "scheduler.takeover_cooldown_ms",
        Absent,
        DurationMs,
        Code,
        NoFileMember,
    ),
    row(
        "scheduler.lease_ttl_ms",
        Absent,
        DurationMs,
        Code,
        NoFileMember,
    ),
    row(
        "scheduler.maximum_queue_timeout_ms",
        Absent,
        DurationMs,
        Code,
        NoFileMember,
    ),
    row(
        "scheduler.max_queue_depth_per_instance",
        Absent,
        Integer,
        Code,
        NoFileMember,
    ),
    row(
        "policy_cadence.debounce_ms",
        Absent,
        DurationMs,
        Code,
        NoFileMember,
    ),
    row(
        "policy_cadence.cooldown_ms",
        Absent,
        DurationMs,
        Code,
        NoFileMember,
    ),
    row(
        "policy_cadence.reconciliation_interval_ms",
        Absent,
        DurationMs,
        Code,
        NoFileMember,
    ),
    row(
        "policy_cadence.clock_jump_threshold_ms",
        Absent,
        DurationMs,
        Code,
        NoFileMember,
    ),
    row("io_timeout_ms", Absent, DurationMs, Code, NoFileMember),
    row("maximum_frame_bytes", Absent, Integer, Code, NoFileMember),
    row(
        "performance_control.escalation_samples",
        Absent,
        Integer,
        Code,
        NoFileMember,
    ),
    row(
        "performance_control.recovery_samples",
        Absent,
        Integer,
        Code,
        NoFileMember,
    ),
    row(
        "performance_control.transition_cooldown_ms",
        Absent,
        DurationMs,
        Code,
        NoFileMember,
    ),
    // Workflow #385 P-c: the controller's clock jump and heavy-dispatch limits and the
    // monitor's sampling period and pressure streaks are keys of the `performance` section
    // (`performance.pacing.*`, `performance.monitor.*`, `performance.pressure_*_samples`).
    row(
        "capacity_thresholds.hard_bytes",
        Key,
        Integer,
        Code,
        ReadAtAssembly,
    ),
    row(
        "capacity_thresholds.soft_bytes",
        Key,
        Integer,
        Code,
        ReadAtAssembly,
    ),
    row(
        "agent_dispatcher.max_attempts",
        Key,
        Integer,
        NoDefault,
        ReadAtAssembly,
    ),
    row(
        "agent_dispatcher.max_session_ms",
        Key,
        DurationMs,
        NoDefault,
        ReadAtAssembly,
    ),
    row(
        "agent_dispatcher.max_projection_events",
        Key,
        Integer,
        NoDefault,
        ReadAtAssembly,
    ),
    row(
        "mumu_manager.control_timeout_ms",
        Absent,
        DurationMs,
        Code,
        NoFileMember,
    ),
    row(
        "mumu_manager.state_wait_start_ms",
        Absent,
        DurationMs,
        Code,
        NoFileMember,
    ),
    row(
        "mumu_manager.state_wait_stop_ms",
        Absent,
        DurationMs,
        Code,
        NoFileMember,
    ),
];
