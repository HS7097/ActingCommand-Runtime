// SPDX-License-Identifier: AGPL-3.0-only

//! The `device_state` section (Workflow #398 RD-5b, model-398 v1.1 §7.8; `contracts/
//! v012-register.md` §1.59): how the Runtime reads a handset's device state and when that state
//! holds the handset.
//!
//! - `sample_interval_ms` and `fact_ttl_ms`: a bound handset at `device` is read every interval,
//!   and the `device.*` facts it writes live for the TTL;
//! - `wake_settle_ms`: the wait after the wake key, and after a keyguard dismissal, before the
//!   screen is read again;
//! - `keepalive_margin_ms`: inside a run, the next input is preceded by a screen check once the
//!   time since the last one passes the screen-off timeout read on the handset less this margin;
//! - `keyguard_insecure`: `dismiss` (one `wm dismiss-keyguard`) or `wait` for a keyguard that
//!   asks for no credential;
//! - `battery_min_percent`: below it, and on no power source, the handset holds (`battery_low`);
//! - `thermal_hold_at` and `thermal_resume_below`: the thermal status that holds the handset and
//!   the one it must fall below before the hold lifts (hysteresis);
//! - `in_use_gate` and `in_use_backoff_ms`: whether another application in the foreground holds
//!   the handset at a run's start, and how long before the foreground is looked at again.
//!
//! Every key is read where it is used, so an instruction may change each while the Runtime runs.
//! The section is read and checked whole (M387 §5.3): the TTL holds at least one sampling
//! interval and the resume level is below the hold level; a broken rule is
//! `config_value_invalid` (`section_check`) for the key it concerns. Every default lives in the
//! defaults file; these are starting values that the real-device check may tune (§16 item 5).
//! `in_use_gate` is off by default: the handset is a dedicated one (Alice, 2026-10-13, model-398
//! §16 item 2).

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{DefaultSource, FilePath, KeyName, KeyRange, KeyRow, Settable, ValueType};
use actingcommand_contract::FactScalar;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The section's name: the first segment of its keys.
pub const SECTION: &str = "device_state";
/// How often a bound handset at `device` is read, in milliseconds.
pub const SAMPLE_INTERVAL_MS: &str = "device_state.sample_interval_ms";
/// How long a `device.*` fact of a sample lives, in milliseconds.
pub const FACT_TTL_MS: &str = "device_state.fact_ttl_ms";
/// The wait after the wake key or a keyguard dismissal, in milliseconds.
pub const WAKE_SETTLE_MS: &str = "device_state.wake_settle_ms";
/// The margin before the screen-off timeout at which an input is preceded by a screen check, in
/// milliseconds.
pub const KEEPALIVE_MARGIN_MS: &str = "device_state.keepalive_margin_ms";
/// What a keyguard without a credential gets: `dismiss` or `wait`.
pub const KEYGUARD_INSECURE: &str = "device_state.keyguard_insecure";
/// The battery level below which a handset on no power source holds, in percent.
pub const BATTERY_MIN_PERCENT: &str = "device_state.battery_min_percent";
/// The thermal status at which a handset holds.
pub const THERMAL_HOLD_AT: &str = "device_state.thermal_hold_at";
/// The thermal status a held handset must fall below before the hold lifts.
pub const THERMAL_RESUME_BELOW: &str = "device_state.thermal_resume_below";
/// Whether another application in the foreground holds a handset at a run's start.
pub const IN_USE_GATE: &str = "device_state.in_use_gate";
/// How long before the foreground of a handset in use is looked at again, in milliseconds.
pub const IN_USE_BACKOFF_MS: &str = "device_state.in_use_backoff_ms";

/// The words of `keyguard_insecure`.
pub const KEYGUARD_INSECURE_TOKENS: &[&str] = &["dismiss", "wait"];
/// The sampling interval's bounds (model-398 §7.8). Schema.
const MIN_SAMPLE_INTERVAL_MS: i64 = 5_000;
const MAX_SAMPLE_INTERVAL_MS: i64 = 3_600_000;
/// The settle wait's bounds (model-398 §7.8). Schema.
const MIN_WAKE_SETTLE_MS: i64 = 100;
const MAX_WAKE_SETTLE_MS: i64 = 10_000;
/// The battery level's bounds, in percent (model-398 §7.8). Schema.
const MAX_BATTERY_PERCENT: i64 = 100;
/// The in-use backoff's bounds (model-398 §7.8). Schema.
const MIN_IN_USE_BACKOFF_MS: i64 = 10_000;
const MAX_IN_USE_BACKOFF_MS: i64 = 3_600_000;

actingcommand_contract::outcome_vocabulary! {
    /// A handset's thermal status (vocabulary `thermal_status`, model-398 §7.1, §10.1): Android's
    /// `THERMAL_STATUS_*` levels, mildest first. The two thermal keys take these words, and
    /// `device_thermal_hold` names the status that held the handset.
    pub enum ThermalLevel: "thermal_status" {
        None => "none",
        Light => "light",
        Moderate => "moderate",
        Severe => "severe",
        Critical => "critical",
        Emergency => "emergency",
        Shutdown => "shutdown",
    }
}

/// The words of the two thermal keys, mildest first.
pub const THERMAL_TOKENS: &[&str] = &[
    "none",
    "light",
    "moderate",
    "severe",
    "critical",
    "emergency",
    "shutdown",
];

/// The section as the configuration file and the defaults file write it: every member may be
/// left out and none is unknown (M387 §5.3).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceStateFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sample_interval_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fact_ttl_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wake_settle_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    keepalive_margin_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    keyguard_insecure: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    battery_min_percent: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thermal_hold_at: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thermal_resume_below: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    in_use_gate: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    in_use_backoff_ms: Option<Value>,
}

const fn row(name: &'static str, value_type: ValueType, range: KeyRange) -> KeyRow {
    KeyRow {
        name: KeyName::Exact(name),
        file_path: FilePath::Key,
        value_type,
        range,
        default: DefaultSource::File,
        // Read where it is used: the sampling keys at the next sample, the others at the next
        // start gate or input.
        settable: Settable::Yes,
        reader: "RD-5b",
        since: "0.12.0",
    }
}

const fn milliseconds(name: &'static str, min: i64, max: i64) -> KeyRow {
    row(name, ValueType::DurationMs, KeyRange::Integer { min, max })
}

/// One row per key of the section, in the order of model-398 §7.8.
pub(crate) static ROWS: &[KeyRow] = &[
    milliseconds(
        SAMPLE_INTERVAL_MS,
        MIN_SAMPLE_INTERVAL_MS,
        MAX_SAMPLE_INTERVAL_MS,
    ),
    milliseconds(FACT_TTL_MS, 1, i64::MAX),
    milliseconds(WAKE_SETTLE_MS, MIN_WAKE_SETTLE_MS, MAX_WAKE_SETTLE_MS),
    milliseconds(KEEPALIVE_MARGIN_MS, 0, i64::MAX),
    row(
        KEYGUARD_INSECURE,
        ValueType::String,
        KeyRange::Tokens(KEYGUARD_INSECURE_TOKENS),
    ),
    row(
        BATTERY_MIN_PERCENT,
        ValueType::Integer,
        KeyRange::Integer {
            min: 0,
            max: MAX_BATTERY_PERCENT,
        },
    ),
    row(
        THERMAL_HOLD_AT,
        ValueType::String,
        KeyRange::Tokens(THERMAL_TOKENS),
    ),
    row(
        THERMAL_RESUME_BELOW,
        ValueType::String,
        KeyRange::Tokens(THERMAL_TOKENS),
    ),
    row(IN_USE_GATE, ValueType::Boolean, KeyRange::Any),
    milliseconds(
        IN_USE_BACKOFF_MS,
        MIN_IN_USE_BACKOFF_MS,
        MAX_IN_USE_BACKOFF_MS,
    ),
];

/// What a keyguard without a credential gets (model-398 §7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyguardInsecureAction {
    /// One `wm dismiss-keyguard`.
    Dismiss,
    /// Nothing is sent; a person unlocks the handset, as for a keyguard with a credential.
    Wait,
}

/// The `device_state` section in effect, read and checked whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceStateConfig {
    pub sample_interval_ms: u64,
    pub fact_ttl_ms: u64,
    pub wake_settle_ms: u64,
    pub keepalive_margin_ms: u64,
    pub keyguard_insecure: KeyguardInsecureAction,
    pub battery_min_percent: u8,
    pub thermal_hold_at: ThermalLevel,
    pub thermal_resume_below: ThermalLevel,
    pub in_use_gate: bool,
    pub in_use_backoff_ms: u64,
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

    fn duration(&self, key: &str) -> Result<u64, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::DurationMs(ms) => Ok(ms),
            _ => Err(invalid(key, ConfigValueProblem::WrongType)),
        }
    }

    fn thermal(&self, key: &str) -> Result<ThermalLevel, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::String(word) => ThermalLevel::ALL
                .iter()
                .copied()
                .find(|level| level.as_str() == word)
                .ok_or_else(|| invalid(key, ConfigValueProblem::OutOfRange)),
            _ => Err(invalid(key, ConfigValueProblem::WrongType)),
        }
    }
}

impl DeviceStateConfig {
    /// The section from `value` (the configuration in effect), checked whole (M387 §5.3).
    pub fn parse(value: &dyn Fn(&str) -> Option<FactScalar>) -> Result<Self, ConfigFailure> {
        let reader = Reader { value };
        let sample_interval_ms = reader.duration(SAMPLE_INTERVAL_MS)?;
        let fact_ttl_ms = reader.duration(FACT_TTL_MS)?;
        // A sample's facts live at least until the next sample (model-398 §7.8).
        if fact_ttl_ms < sample_interval_ms {
            return Err(invalid(FACT_TTL_MS, ConfigValueProblem::SectionCheck));
        }
        let keyguard_insecure = match reader.scalar(KEYGUARD_INSECURE)? {
            FactScalar::String(action) if action == "dismiss" => KeyguardInsecureAction::Dismiss,
            FactScalar::String(action) if action == "wait" => KeyguardInsecureAction::Wait,
            FactScalar::String(_) => {
                return Err(invalid(KEYGUARD_INSECURE, ConfigValueProblem::OutOfRange));
            }
            _ => return Err(invalid(KEYGUARD_INSECURE, ConfigValueProblem::WrongType)),
        };
        let battery_min_percent = match reader.scalar(BATTERY_MIN_PERCENT)? {
            FactScalar::Integer(percent) => u8::try_from(percent)
                .ok()
                .filter(|percent| i64::from(*percent) <= MAX_BATTERY_PERCENT)
                .ok_or_else(|| invalid(BATTERY_MIN_PERCENT, ConfigValueProblem::OutOfRange))?,
            _ => return Err(invalid(BATTERY_MIN_PERCENT, ConfigValueProblem::WrongType)),
        };
        let thermal_hold_at = reader.thermal(THERMAL_HOLD_AT)?;
        let thermal_resume_below = reader.thermal(THERMAL_RESUME_BELOW)?;
        // The hold lifts below a lower level than the one it starts at (model-398 §7.4, §7.8).
        if thermal_resume_below >= thermal_hold_at {
            return Err(invalid(
                THERMAL_RESUME_BELOW,
                ConfigValueProblem::SectionCheck,
            ));
        }
        let in_use_gate = match reader.scalar(IN_USE_GATE)? {
            FactScalar::Boolean(gate) => gate,
            _ => return Err(invalid(IN_USE_GATE, ConfigValueProblem::WrongType)),
        };
        Ok(Self {
            sample_interval_ms,
            fact_ttl_ms,
            wake_settle_ms: reader.duration(WAKE_SETTLE_MS)?,
            keepalive_margin_ms: reader.duration(KEEPALIVE_MARGIN_MS)?,
            keyguard_insecure,
            battery_min_percent,
            thermal_hold_at,
            thermal_resume_below,
            in_use_gate,
            in_use_backoff_ms: reader.duration(IN_USE_BACKOFF_MS)?,
        })
    }
}
