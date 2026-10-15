// SPDX-License-Identifier: AGPL-3.0-only

//! The `performance` section (Workflow #385 P-c, frozen model PERF v1.4.2 §10): every key of
//! performance pacing, parsed, checked and defaulted in v0.12.0 even where its reader arrives
//! later (order-v012 R4; `contracts/v012-register.md` §5). One file per subsection:
//!
//! - [`monitor`]: `performance.monitor.*`, `performance.load.*`, `performance.pressure.*` and the
//!   two pressure streaks, which keep their file members `performance.pressure_start_samples` /
//!   `performance.pressure_end_samples` (§10.1; register §5);
//! - [`responsiveness`]: `performance.responsiveness.*` (§10.2);
//! - [`pacing`]: `performance.pacing.*` (§10.3);
//! - [`calibration`]: `performance.calibration.*` (§10.4).
//!
//! Every default lives in the defaults file; a configuration names only what it changes, so a
//! key it leaves out (`performance.pacing.mode` above all, §10.4 "缺键才用发布默认值") takes the
//! running release's default. The section is checked whole (§4.1, §10.4): at start and in
//! `check-config` a broken rule is `perf_config_invalid` naming the key, through an instruction
//! the same values are `config_value_invalid`.

pub mod calibration;
pub mod monitor;
pub mod pacing;
pub mod responsiveness;

use crate::codes::{ConfigCode, ConfigValueProblem};
use crate::effective::ConfigFailure;
use crate::table::{
    DefaultSource, FilePath, KeyName, KeyRange, KeyRow, NotSettable, Settable, ValueType,
};
use actingcommand_contract::FactScalar;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

/// The section's name: the first segment of its keys.
pub const SECTION: &str = "performance";

/// The rows of the section, one slice per subsection file, in manifest order.
pub(crate) fn sections() -> [&'static [KeyRow]; 4] {
    [
        monitor::ROWS,
        responsiveness::ROWS,
        pacing::ROWS,
        calibration::ROWS,
    ]
}

fn rows() -> impl Iterator<Item = &'static KeyRow> {
    sections().into_iter().flatten()
}

/// The section as the configuration file and the defaults file write it: every member may be
/// left out and none is unknown (M387 §5.3). A member is known when the key table has its key,
/// or keys below it for an object; its value is kept as JSON here, and the key table checks its
/// type and range, the whole-section check its rules.
#[derive(Debug, Default, Clone, Serialize)]
#[serde(transparent)]
pub struct PerformanceFile(Map<String, Value>);

impl PerformanceFile {
    /// The member of the full key `key` (`performance.<path>`), when the file names it.
    pub fn value_at(&self, key: &str) -> Option<&Value> {
        let path = key.strip_prefix(SECTION)?.strip_prefix('.')?;
        let mut segments = path.split('.');
        let first = self.0.get(segments.next()?)?;
        segments
            .try_fold(first, |value, segment| value.get(segment))
            .filter(|value| !value.is_null())
    }
}

impl<'de> Deserialize<'de> for PerformanceFile {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let members = Map::<String, Value>::deserialize(deserializer)?;
        match unknown_member(&members, SECTION) {
            Some(name) => Err(D::Error::unknown_field(&name, &[])),
            None => Ok(Self(members)),
        }
    }
}

/// The first member under `prefix` that is neither a key of the section nor an object holding
/// keys of it; a `null` member is absent.
fn unknown_member(members: &Map<String, Value>, prefix: &str) -> Option<String> {
    for (name, member) in members {
        let path = format!("{prefix}.{name}");
        let is_key = rows().any(|row| matches!(row.name, KeyName::Exact(key) if key == path));
        if member.is_null() || is_key {
            continue;
        }
        let below = format!("{path}.");
        let holds_keys = rows().any(|row| match row.name {
            KeyName::Exact(key) => key.starts_with(below.as_str()),
            KeyName::PerInstance(_) => false,
        });
        match member {
            Value::Object(inner) if holds_keys => {
                if let Some(found) = unknown_member(inner, &path) {
                    return Some(found);
                }
            }
            _ => return Some(path),
        }
    }
    None
}

/// The release from which every key of the section has its row.
const SINCE: &str = "0.12.0";

/// One row of the section: at the key in the file, default from the defaults file.
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
        since: SINCE,
    }
}

/// Read once when the Runtime starts (M387 §5.5: "采样周期等 false").
const AT_START: Settable = Settable::No(NotSettable::ReadAtAssembly);
/// Read when used (M387 §5.5: "曲线与 `hold.*` true").
const WHEN_USED: Settable = Settable::Yes;

const fn between(min: i64, max: i64) -> KeyRange {
    KeyRange::Integer { min, max }
}

const fn at_least(min: i64) -> KeyRange {
    KeyRange::Integer { min, max: i64::MAX }
}

/// Basis points: 0..=10 000.
const BASIS_POINTS: KeyRange = between(0, 10_000);
/// The pacing stages (§4.1, §4.4): a configured stage is one of these words.
pub const STAGES: &[&str] = &["S0", "S1", "S2", "S3", "S4"];
/// The stage words as a key range.
const STAGE_WORDS: KeyRange = KeyRange::Tokens(STAGES);

/// The position of a stage word in [`STAGES`].
pub fn stage_index(word: &str) -> Option<usize> {
    STAGES.iter().position(|known| *known == word)
}

/// `perf_config_invalid` for `config_key`, for `problem` (§10.4 "值里写键名").
fn failure(config_key: &str, problem: ConfigValueProblem) -> ConfigFailure {
    ConfigFailure::with_problem(ConfigCode::PerfConfigInvalid, config_key, problem)
}

/// The values of the section's keys, read one by one from the configuration in effect.
struct Values<'a> {
    value: &'a dyn Fn(&str) -> Option<FactScalar>,
}

impl Values<'_> {
    fn integer(&self, key: &'static str) -> Result<i64, ConfigFailure> {
        match (self.value)(key) {
            Some(FactScalar::Integer(number)) => Ok(number),
            Some(FactScalar::DurationMs(ms)) => {
                i64::try_from(ms).map_err(|_| failure(key, ConfigValueProblem::OutOfRange))
            }
            _ => Err(failure(key, ConfigValueProblem::WrongType)),
        }
    }

    fn word(&self, key: &'static str) -> Result<String, ConfigFailure> {
        match (self.value)(key) {
            Some(FactScalar::String(text)) => Ok(text),
            _ => Err(failure(key, ConfigValueProblem::WrongType)),
        }
    }

    /// Workflow #385 P-m: a boolean key's value.
    fn flag(&self, key: &'static str) -> Result<bool, ConfigFailure> {
        match (self.value)(key) {
            Some(FactScalar::Boolean(flag)) => Ok(flag),
            _ => Err(failure(key, ConfigValueProblem::WrongType)),
        }
    }

    /// Workflow #385 P-m: an integer key's value that is not negative and fits `T`.
    fn unsigned<T: TryFrom<i64>>(&self, key: &'static str) -> Result<T, ConfigFailure> {
        T::try_from(self.integer(key)?).map_err(|_| failure(key, ConfigValueProblem::OutOfRange))
    }

    /// A list or object key's value, from its canonical JSON text.
    fn json(&self, key: &'static str) -> Result<Value, ConfigFailure> {
        match (self.value)(key) {
            Some(FactScalar::String(text)) => serde_json::from_str::<Value>(&text)
                .map_err(|_| failure(key, ConfigValueProblem::WrongType)),
            _ => Err(failure(key, ConfigValueProblem::WrongType)),
        }
    }
}

/// An integer member of a list or object value, within `min..=max`.
fn integer_within(
    key: &str,
    value: Option<&Value>,
    min: i64,
    max: i64,
) -> Result<i64, ConfigFailure> {
    let number = value
        .and_then(Value::as_i64)
        .ok_or_else(|| failure(key, ConfigValueProblem::WrongType))?;
    if (min..=max).contains(&number) {
        Ok(number)
    } else {
        Err(failure(key, ConfigValueProblem::OutOfRange))
    }
}

/// An object value whose members are exactly `names`.
fn exact_object<'v>(
    key: &str,
    value: &'v Value,
    names: &[&str],
) -> Result<&'v Map<String, Value>, ConfigFailure> {
    let members = value
        .as_object()
        .ok_or_else(|| failure(key, ConfigValueProblem::WrongType))?;
    if members.len() != names.len() || names.iter().any(|name| !members.contains_key(*name)) {
        return Err(failure(key, ConfigValueProblem::WrongType));
    }
    Ok(members)
}

/// A pair `[first, second]` of integers, each within `min..=max`.
fn pair_within(
    key: &str,
    value: Option<&Value>,
    min: i64,
    max: i64,
) -> Result<(i64, i64), ConfigFailure> {
    match value.and_then(Value::as_array).map(Vec::as_slice) {
        Some([first, second]) => Ok((
            integer_within(key, Some(first), min, max)?,
            integer_within(key, Some(second), min, max)?,
        )),
        _ => Err(failure(key, ConfigValueProblem::WrongType)),
    }
}

/// `Ok` when `holds`, otherwise the section's broken rule on `key`.
fn rule(key: &str, holds: bool) -> Result<(), ConfigFailure> {
    if holds {
        Ok(())
    } else {
        Err(failure(key, ConfigValueProblem::SectionCheck))
    }
}

/// The whole-section check (§4.1, §10.4; M387 §5.3), on the values in effect.
pub(crate) fn check(value: &dyn Fn(&str) -> Option<FactScalar>) -> Result<(), ConfigFailure> {
    let values = Values { value };
    monitor::check(&values)?;
    responsiveness::check(&values)?;
    pacing::check(&values)?;
    calibration::check(&values)
}

/// Workflow #385 P-m (PERF §3, §4, §4.6, §10): what the shadow measurement reads, in effect.
/// `user_curve` says whether a person wrote `performance.pacing.curve` (in the file or by an
/// instruction): then the learning view has no cap (§4.5.3 "人工覆盖", §4.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerformanceMeasurement {
    pub responsiveness: responsiveness::ResponsivenessSettings,
    pub pacing: pacing::PacingSettings,
    pub learning: calibration::LearningSettings,
    pub user_curve: bool,
    /// `performance.pressure.gpu_aggregate` (§3.4): how the GPU's engines make one utilisation.
    pub gpu_aggregate: monitor::GpuAggregate,
}

impl PerformanceMeasurement {
    /// Whether the learning view caps the normal view (§4.5.2, §4.6): not under a person's
    /// curve and not with `calibration.apply = off`.
    pub fn learning_cap(&self) -> bool {
        !self.user_curve && self.learning.apply != calibration::CalibrationApply::Off
    }
}

/// Workflow #385 P-m: reads what the shadow measurement needs from the values in effect. The
/// section passed its whole-section check when it was assembled, so a refusal here names the
/// key as that check would.
pub(crate) fn measurement(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    user_curve: bool,
) -> Result<PerformanceMeasurement, ConfigFailure> {
    let values = Values { value };
    Ok(PerformanceMeasurement {
        responsiveness: responsiveness::read(&values)?,
        pacing: pacing::read(&values)?,
        learning: calibration::read(&values)?,
        user_curve,
        gpu_aggregate: monitor::gpu_aggregate(&values)?,
    })
}
