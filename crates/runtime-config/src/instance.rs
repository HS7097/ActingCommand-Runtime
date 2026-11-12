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
//! it leaves it off and reports no row; read at assembly until R-3 (below). Workflow #398 RD-4 adds
//! `recovery_ladder_profile` (model-398 v1.1 §6.2): the ladder profile the instance climbs by,
//! before the section's `kind_profiles`; default `none` (no row unless the entry names it),
//! settable, checked with the `recovery_ladder` section. Workflow #411 B15 R3 (model-b15 §5.2)
//! adds `scheduling.remedy.choices`, the instance's own remedy choice rows in its entry's
//! `scheduling` member: default the `scheduling` section's list; when set, it replaces that list
//! as a whole; settable; its rows checked as the section's are. Workflow #386 SR-4a1 (SCH-M §3.2
//! D-1, D-6) adds `scheduling.blackout`, the instance's own blackout list in its entry's
//! `scheduling` member (default none: an instance whose entry names none has no entries of its
//! own; settable; its entries checked as the host list's are, refused as `blackout_config_invalid`
//! with `blackout_source` `instance`), and `scheduling.day_boundary.quiet`, default the section's
//! `scheduling.day_boundary.quiet`, settable. SCH-M ED-1 (§3.4 "Configuration") adds
//! `scheduling.expected_runtime.enabled`, `.runs` and `.margin_ms`, in the entry's `scheduling`
//! member, each defaulting to the `scheduling` section's key of the same name, settable. IDLE-1
//! adds `idle.action`, `.min_gap_ms`, `.close_app_wake_lead_ms` and `.stop_emulator_wake_lead_ms`,
//! in the entry's `idle` member, each defaulting to the `idle` section's key of the same name,
//! settable; checked whole per instance, and `stop_emulator` only on an instance whose emulator
//! the Runtime starts (`start_emulator`); IDLE-2 adds `.return_home_min_gap_ms` and
//! `.stop_emulator_min_gap_ms` the same way. Workflow #392 DA-4b-D adds
//! `display_safe_area.model`, the device model the instance declares (the entry's
//! `display_safe_area.model` member), default the `display_safe_area` section's key, settable.
//! SS-4 (register §1.231) adds `scheduling.no_progress.enabled`, in the entry's `scheduling`
//! member, default the `scheduling` section's key of the same name, settable. STILL-1 (register
//! §1.249) adds `execution.capture.still_limit_ms`, in the entry's `execution` member, default the
//! `execution` section's key of the same name, settable. LOAD-3 (register §1.260) adds
//! `execution.loading.mode`, in the entry's `execution` member, no default of its own (the
//! section's is in effect), settable. #402 HS-1 (register §1.267, design HS-M §2.1) adds
//! `touch_backend` and `capture_backend`, the entry's existing members, which a device instance's
//! entry always names and a fixture's never does: default `none`; their whole-instance check takes
//! each kind of device's backends from the check context the host fills in, and a request that
//! names one names only the backend keys of one instance. #402 HS-2 (register §1.270, design HS-M
//! §3) makes both settable: the host switches the instance's backends when an instruction changes
//! them ([`backend_switch_instance`]). Workflow #429 R-3 (register §1.272) makes `start_emulator`
//! settable: every automatic emulator start reads the value in effect at its next decision, and
//! the whole-instance check refuses `true` on an instance the check context does not name as bound
//! by discovery (`check_start_emulator`). #428 SCHED-C1e (register §1.299 SC1-12) adds
//! `scheduling.pool_lines`, the instance's own lines of its pools, in the entry's `scheduling`
//! member: no default (each pool's catalog lines are in effect), settable; checked whole per
//! instance, and an instruction names only pools the active generation gives the instance with a
//! declared valuation (`EffectiveConfig::apply`, the check context's valued pools).

use crate::codes::ConfigValueProblem;
use crate::display_safe_area;
use crate::effective::ConfigFailure;
use crate::execution;
use crate::idle;
use crate::recovery_ladder;
use crate::scheduling::{
    self, DAY_BOUNDARY_QUIET, DELAY, EXPECTED_RUNTIME_ENABLED, EXPECTED_RUNTIME_MARGIN_MS,
    EXPECTED_RUNTIME_MARGIN_RANGE, EXPECTED_RUNTIME_RUNS, EXPECTED_RUNTIME_RUNS_RANGE,
    INSTANCE_BLACKOUT, INSTANCE_POOL_LINES, NO_PROGRESS_ENABLED, RANDOM_DELAY_ENABLED,
    RANDOM_DELAY_MAX_MS, RANDOM_DELAY_MIN_MS, REMEDY_CHOICES,
};
use crate::table::{
    DefaultSource, FilePath, KeyName, KeyRange, KeyRow, NotSettable, Settable, ValueType,
    instance_key,
};
use actingcommand_contract::{ConfigSettingChange, FactScalar, InstanceStuckRecovery};
use std::collections::BTreeSet;

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
/// Workflow #429 R-3: settable; every automatic emulator start reads it in effect.
pub const START_EMULATOR: &str = "start_emulator";
/// Workflow #398 RD-4 (model-398 v1.1 §6.2): the ladder profile the instance climbs by, the
/// entry's `recovery_ladder_profile`; it comes before the `recovery_ladder` section's
/// `kind_profiles` and must name one of its `profiles`.
pub const RECOVERY_LADDER_PROFILE: &str = "recovery_ladder_profile";
/// #402 HS-1 (HS-M §2.1): the instance's touch backend, the entry's `touch_backend`.
pub const TOUCH_BACKEND: &str = "touch_backend";
/// #402 HS-1: the instance's capture backend, the entry's `capture_backend`.
pub const CAPTURE_BACKEND: &str = "capture_backend";
/// #402 HS-1: the values of [`TOUCH_BACKEND`], the device layer's explicit touch backends by
/// their own names (`auto` and `auto-fastest` are none: every instance names its backend).
pub const TOUCH_BACKEND_TOKENS: &[&str] = &["adb_shell_input", "maatouch", "minitouch", "nemu_ipc"];
/// #402 HS-1: the values of [`CAPTURE_BACKEND`], the device layer's explicit capture backends.
pub const CAPTURE_BACKEND_TOKENS: &[&str] = &["adb", "droidcast_raw", "nemu_ipc", "scrcpy"];
/// #402 HS-1: Nemu IPC, a word of both lists; Nemu touch works only beside Nemu capture.
const NEMU_IPC: &str = "nemu_ipc";
/// The [`DEVICE_KIND`] of a handset.
pub(crate) const HANDSET: &str = "handset";

/// #402 HS-1 (HS-M §2.1; register §1.267): the instance's two backends. A device instance's
/// entry always names both and a fixture's names neither (actingd refuses the rest), so they have
/// no default. #402 HS-2 (HS-M §3; register §1.270): settable; the host switches the instance's
/// device backends before an instruction that changes them takes effect.
const BACKEND_ROWS: [KeyRow; 2] = [
    backend_row(TOUCH_BACKEND, TOUCH_BACKEND_TOKENS),
    backend_row(CAPTURE_BACKEND, CAPTURE_BACKEND_TOKENS),
];

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
    // #402 HS-1: the device's two backends, beside its kind.
    BACKEND_ROWS[0],
    BACKEND_ROWS[1],
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
    // Workflow #429 R-3 (register §1.272; since 0.12.39 settable): reported only for an instance
    // that names it or that an instruction sets it for; the daemon start, the emulator watch and
    // IDLE-1's wake read it in effect at each decision, and `true` needs a discovery-bound instance
    // (`check_start_emulator`).
    KeyRow {
        name: KeyName::PerInstance(START_EMULATOR),
        file_path: FilePath::InstanceMember,
        value_type: ValueType::Boolean,
        range: KeyRange::Any,
        default: DefaultSource::None,
        settable: Settable::Yes,
        reader: "R-3",
        since: "0.12.0",
    },
    // Workflow #398 RD-4 (model-398 §6.2): reported only for an instance that names it (or, since
    // 0.12.31, Workflow #429 R-1, that an instruction sets it for); settable while the Runtime
    // runs, as the `recovery_ladder` section is; a ladder takes it into its snapshot when it is
    // admitted.
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
        settable: Settable::Yes,
        reader: "B15-R4",
        since: "0.12.3",
    },
    // Workflow #386 SR-4a1 (SCH-M §3.2 D-1): the instance's own blackout list; reported only for
    // an instance whose entry names one (none: no entries of its own); settable while the
    // Runtime runs, since 0.12.31 (Workflow #429 R-1) also for an instance whose entry names
    // none, and clearing that instruction leaves the instance with no entries of its own.
    KeyRow {
        name: KeyName::PerInstance(INSTANCE_BLACKOUT),
        file_path: FilePath::InstanceMember,
        value_type: ValueType::List,
        range: KeyRange::Any,
        default: DefaultSource::None,
        settable: Settable::Yes,
        reader: "SR-4a2",
        since: "0.12.12",
    },
    // Workflow #386 SR-4a1 (SCH-M §3.2 D-6): the instance's own day-boundary switch; the
    // section's value in effect is its default.
    KeyRow {
        name: KeyName::PerInstance(DAY_BOUNDARY_QUIET),
        file_path: FilePath::InstanceMember,
        value_type: ValueType::Boolean,
        range: KeyRange::Any,
        default: DefaultSource::Section(DAY_BOUNDARY_QUIET),
        settable: Settable::Yes,
        reader: "SR-4a2",
        since: "0.12.12",
    },
    // SCH-M ED-1 (§3.4 "Configuration"): the instance's own expected-duration keys; the
    // section's values in effect are their defaults.
    expected_runtime_row(EXPECTED_RUNTIME_ENABLED, ValueType::Boolean, KeyRange::Any),
    expected_runtime_row(
        EXPECTED_RUNTIME_RUNS,
        ValueType::Integer,
        EXPECTED_RUNTIME_RUNS_RANGE,
    ),
    expected_runtime_row(
        EXPECTED_RUNTIME_MARGIN_MS,
        ValueType::DurationMs,
        EXPECTED_RUNTIME_MARGIN_RANGE,
    ),
    // IDLE-1: the instance's own idle keys; the `idle` section's values in effect are their
    // defaults.
    idle::INSTANCE_ROWS[0],
    idle::INSTANCE_ROWS[1],
    idle::INSTANCE_ROWS[2],
    idle::INSTANCE_ROWS[3],
    // IDLE-2: the instance's own thresholds of `auto`'s tiers; the section's are their defaults.
    idle::INSTANCE_ROWS[4],
    idle::INSTANCE_ROWS[5],
    // Workflow #392 DA-4b-D: the device model the instance declares; the section's value in
    // effect is its default.
    display_safe_area::INSTANCE_ROW,
    // SS-4 (register §1.231): the instance's own no-progress switch; the section's value in
    // effect is its default.
    KeyRow {
        name: KeyName::PerInstance(NO_PROGRESS_ENABLED),
        file_path: FilePath::InstanceMember,
        value_type: ValueType::Boolean,
        range: KeyRange::Any,
        default: DefaultSource::Section(NO_PROGRESS_ENABLED),
        settable: Settable::Yes,
        reader: "SS-4",
        since: "0.12.34",
    },
    // STILL-1 (register §1.249): the instance's own still limit; the section's value in effect is
    // its default.
    execution::INSTANCE_STILL_LIMIT_ROW,
    // LOAD-3 (register §1.260): the instance's own loading mode; reported only for an instance
    // whose entry names one or an instruction sets one, otherwise the section's is in effect.
    execution::INSTANCE_LOADING_MODE_ROW,
    // #428 SCHED-C1e (register §1.299 SC1-12): the instance's own pool lines; reported only for
    // an instance whose entry names them or an instruction sets them (no default: each pool's
    // catalog lines are in effect); settable; checked whole with the instance, and an instruction
    // names only pools the active generation gives the instance with a declared valuation.
    KeyRow {
        name: KeyName::PerInstance(INSTANCE_POOL_LINES),
        file_path: FilePath::InstanceMember,
        value_type: ValueType::Object,
        range: KeyRange::Any,
        default: DefaultSource::None,
        settable: Settable::Yes,
        reader: "SCHED-C1e",
        since: "0.12.54",
    },
    // Workflow #437 MAPP-1 (`contracts/device-group.md`, "The configuration form"): the entry's
    // device group, reported only for an entry that names it, read once at assembly.
    crate::device_group::ROW,
];

/// SCH-M ED-1: one instance's expected-duration member, named as the section-level key whose
/// value in effect is its default.
const fn expected_runtime_row(
    member: &'static str,
    value_type: ValueType,
    range: KeyRange,
) -> KeyRow {
    KeyRow {
        name: KeyName::PerInstance(member),
        file_path: FilePath::InstanceMember,
        value_type,
        range,
        default: DefaultSource::Section(member),
        settable: Settable::Yes,
        reader: "ED-2",
        since: "0.12.12",
    }
}

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

/// #402 HS-1: one of the instance's two backend members, a word of `tokens`; settable since
/// HS-2, which reads it when it switches the instance's backends.
const fn backend_row(member: &'static str, tokens: &'static [&'static str]) -> KeyRow {
    KeyRow {
        name: KeyName::PerInstance(member),
        file_path: FilePath::InstanceMember,
        value_type: ValueType::String,
        range: KeyRange::Tokens(tokens),
        default: DefaultSource::None,
        settable: Settable::Yes,
        reader: "HS-2",
        since: "0.12.39",
    }
}

/// #402 HS-1 (HS-M §2.1): the backends one kind of device has, by their place in
/// [`TOUCH_BACKEND_TOKENS`] and [`CAPTURE_BACKEND_TOKENS`]. The host fills it in from the device
/// layer's own rule, since this crate depends on the contract only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceBackends {
    touch: [bool; TOUCH_BACKEND_TOKENS.len()],
    capture: [bool; CAPTURE_BACKEND_TOKENS.len()],
}

impl DeviceBackends {
    /// No backend at all: a fixture's, and each kind's until the caller fills the rules in.
    pub const NONE: Self = Self {
        touch: [false; TOUCH_BACKEND_TOKENS.len()],
        capture: [false; CAPTURE_BACKEND_TOKENS.len()],
    };

    /// The backends of the two lists that `touch` and `capture` admit, each asked once per word.
    pub fn admitting(touch: impl Fn(&str) -> bool, capture: impl Fn(&str) -> bool) -> Self {
        Self {
            touch: std::array::from_fn(|at| {
                TOUCH_BACKEND_TOKENS.get(at).copied().is_some_and(&touch)
            }),
            capture: std::array::from_fn(|at| {
                CAPTURE_BACKEND_TOKENS
                    .get(at)
                    .copied()
                    .is_some_and(&capture)
            }),
        }
    }

    /// Whether this kind of device has the touch backend `name`.
    pub fn has_touch(&self, name: &str) -> bool {
        admits(TOUCH_BACKEND_TOKENS, &self.touch, name)
    }

    /// Whether this kind of device has the capture backend `name`.
    pub fn has_capture(&self, name: &str) -> bool {
        admits(CAPTURE_BACKEND_TOKENS, &self.capture, name)
    }
}

/// Whether the word `name` of `tokens` is one `admitted` marks.
fn admits(tokens: &[&str], admitted: &[bool], name: &str) -> bool {
    tokens
        .iter()
        .zip(admitted)
        .any(|(token, admitted)| *admitted && *token == name)
}

/// #402 HS-1 (HS-M §2.1): the backends each kind of device has, the table the whole-instance
/// backend check reads. A fixture has none; that is the check's own rule, not the host's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendKindRules {
    /// An emulator's: an instance whose entry names no `device`, or `device.kind` `emulator`.
    pub emulator: DeviceBackends,
    /// A handset's: an instance whose `device_kind` is `handset`.
    pub handset: DeviceBackends,
}

impl BackendKindRules {
    /// No backend on any kind: the rules of a check context nobody filled in, which refuse every
    /// backend in effect rather than pass one unchecked.
    pub const NONE: Self = Self {
        emulator: DeviceBackends::NONE,
        handset: DeviceBackends::NONE,
    };
}

/// #402 HS-1: whether `row` is one of the two backend rows.
pub(crate) fn is_backend_row(row: &KeyRow) -> bool {
    BACKEND_ROWS.iter().any(|backend| backend.name == row.name)
}

/// #402 HS-1: the instance id `key` names when it is one of the two backend keys.
fn backend_instance(key: &str) -> Option<&str> {
    BACKEND_ROWS
        .iter()
        .find_map(|row| row.matches(key).flatten())
}

/// #402 HS-2 (HS-M §3.2; register §1.270): the instance whose backends `changes` switch, the
/// instance id the first backend key names; `None` for a request that names no backend key. A
/// request that [`EffectiveConfig::apply`](crate::EffectiveConfig::apply) admitted names only
/// that instance's backend keys.
pub fn backend_switch_instance(changes: &[ConfigSettingChange]) -> Option<&str> {
    changes
        .iter()
        .find_map(|change| backend_instance(&change.key))
}

/// #402 HS-1 (HS-M §2.1 "一次只切一台"): a request that names a backend key names only the
/// backend keys of that one instance (one or both), so that all of it or none of it is one switch
/// of one device. Otherwise the first key that is not one of them is `config_value_invalid`
/// (`backend_switch_not_alone`), the other key or the other instance's backend key.
pub(crate) fn check_backend_change(changes: &[ConfigSettingChange]) -> Result<(), ConfigFailure> {
    let Some(instance) = changes
        .iter()
        .find_map(|change| backend_instance(&change.key))
    else {
        return Ok(());
    };
    match changes
        .iter()
        .find(|change| backend_instance(&change.key) != Some(instance))
    {
        Some(other) => Err(ConfigFailure::invalid_value(
            other.key.clone(),
            ConfigValueProblem::BackendSwitchNotAlone,
        )),
        None => Ok(()),
    }
}

/// #402 HS-1 (HS-M §2.1): the whole-instance backend check of `instance`, on its two backends in
/// effect. Its kind of device gives the backends it may have: none for a fixture (an instance whose
/// file layer names neither backend, `file_names_backends` false: actingd requires both of a
/// device instance and refuses both on a fixture), the handset's of `rules` for an instance whose
/// `device_kind` is `handset`, the emulator's otherwise. Another backend is `config_value_invalid`
/// (`backend_kind_unsupported`) for its key; touch `nemu_ipc` beside another capture backend is
/// `backend_pair_required` for the touch key (Nemu capture beside any touch backend is free). An
/// instance with neither backend in effect is not checked.
pub(crate) fn check_backends(
    instance: &str,
    value: impl Fn(&str) -> Option<FactScalar>,
    file_names_backends: bool,
    rules: &BackendKindRules,
) -> Result<(), ConfigFailure> {
    let touch_key = instance_key(instance, TOUCH_BACKEND);
    let capture_key = instance_key(instance, CAPTURE_BACKEND);
    let name = |key: &str| match value(key) {
        None => Ok(None),
        Some(FactScalar::String(name)) => Ok(Some(name)),
        Some(_) => Err(ConfigFailure::invalid_value(
            key,
            ConfigValueProblem::WrongType,
        )),
    };
    let touch = name(&touch_key)?;
    let capture = name(&capture_key)?;
    if touch.is_none() && capture.is_none() {
        return Ok(());
    }
    let handset = matches!(
        value(&instance_key(instance, DEVICE_KIND)),
        Some(FactScalar::String(kind)) if kind == HANDSET
    );
    let backends = match (file_names_backends, handset) {
        (false, _) => DeviceBackends::NONE,
        (true, true) => rules.handset,
        (true, false) => rules.emulator,
    };
    let unsupported =
        |key: &str| ConfigFailure::invalid_value(key, ConfigValueProblem::BackendKindUnsupported);
    if touch
        .as_deref()
        .is_some_and(|touch| !backends.has_touch(touch))
    {
        return Err(unsupported(&touch_key));
    }
    if capture
        .as_deref()
        .is_some_and(|capture| !backends.has_capture(capture))
    {
        return Err(unsupported(&capture_key));
    }
    if touch.as_deref() == Some(NEMU_IPC) && capture.as_deref() != Some(NEMU_IPC) {
        return Err(ConfigFailure::invalid_value(
            touch_key,
            ConfigValueProblem::BackendPairRequired,
        ));
    }
    Ok(())
}

/// Workflow #429 R-3 (register §1.272): whether `row` is the `start_emulator` row.
pub(crate) fn is_start_emulator_row(row: &KeyRow) -> bool {
    row.name == KeyName::PerInstance(START_EMULATOR)
}

/// Workflow #429 R-3 (register §1.272; actingd's own rule for the file, Workflow #369 S6a): the
/// instance's `start_emulator` `true` in effect needs an instance whose emulator the Runtime can
/// start, one bound by discovery. `discovery_bound` names those instances as the host knows them
/// (its registered physical instances bound by `instance_index` or `instance_name`); any other is
/// `config_value_invalid` (`discovery_binding_required`) for the key: a fixture, a handset, an
/// instance bound by its port. `None`: the context does not know the instances (actingd's check of
/// the file, which refuses the same value earlier with its own code,
/// `start_emulator_requires_discovery_binding`), and the rule is not asked. `false`, and no value,
/// pass on every instance.
pub(crate) fn check_start_emulator(
    instance: &str,
    value: impl Fn(&str) -> Option<FactScalar>,
    discovery_bound: Option<&BTreeSet<String>>,
) -> Result<(), ConfigFailure> {
    let key = instance_key(instance, START_EMULATOR);
    match value(&key) {
        None | Some(FactScalar::Boolean(false)) => Ok(()),
        Some(FactScalar::Boolean(true)) => match discovery_bound {
            Some(bound) if !bound.contains(instance) => Err(ConfigFailure::invalid_value(
                key,
                ConfigValueProblem::DiscoveryBindingRequired,
            )),
            _ => Ok(()),
        },
        Some(_) => Err(ConfigFailure::invalid_value(
            key,
            ConfigValueProblem::WrongType,
        )),
    }
}

/// The whole-section check of one instance (§5.3): its two stuck-recovery members together form
/// valid stuck-recovery settings, (Workflow #405) its random delay's `min_ms` is at most its
/// `max_ms` as both are in effect, each its own or the section's, (Workflow #411 B15 R3) its
/// remedy choice rows in effect keep the rows' rules, and (Workflow #386 SR-4a1) its own
/// blackout entries keep the entries' rules (`index` is the instance's position in the file's
/// `instances`, for the entries' pointers). A member the section does not carry is not checked.
pub(crate) fn check_instance(
    instance: &str,
    index: usize,
    value: impl Fn(&str) -> Option<FactScalar>,
) -> Result<(), ConfigFailure> {
    let read = |key: &str| value(key);
    scheduling::instance_blackouts(&read, instance, index)?;
    // STILL-1: the instance's own still limit is 0 or within the on range.
    let still_limit_key = instance_key(instance, execution::CAPTURE_STILL_LIMIT_MS);
    if value(&still_limit_key).is_some() {
        execution::still_limit_ms(&read, &still_limit_key)?;
    }
    // IDLE-1: the instance's idle keys in effect, checked whole; an instance whose action in
    // effect is `stop_emulator` is one whose emulator the Runtime starts (`start_emulator`), since
    // its wake starts the emulator through that configured start. IDLE-2: `auto` needs no
    // `start_emulator`; on an instance without it the tier stops short of `stop_emulator`.
    let idle_action_key = instance_key(instance, idle::ACTION);
    if value(&idle_action_key).is_some() {
        let config = idle::instance_idle(&read, instance)?;
        if config.action == idle::IdleAction::StopEmulator
            && value(&instance_key(instance, START_EMULATOR)) != Some(FactScalar::Boolean(true))
        {
            return Err(ConfigFailure::invalid_value(
                idle_action_key,
                ConfigValueProblem::SectionCheck,
            ));
        }
    }
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
    // #428 SCHED-C1e: the instance's own pool lines keep their shape and their order where the
    // values tell it.
    scheduling::instance_pool_lines(&read, instance)?;
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
