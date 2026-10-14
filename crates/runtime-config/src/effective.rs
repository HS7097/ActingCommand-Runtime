// SPDX-License-Identifier: AGPL-3.0-only

//! The configuration in effect (Workflow #387 model v1.2 §5.6-§5.9): the file layer actingd
//! assembled (the configuration manifest) with the instruction layer over it. An instruction
//! changes only a settable key, takes effect at once and is kept as one `host.config_overrides`
//! row; a start restores the rows by the "later change wins" rule of §12 question 3.

use crate::codes::{ConfigCode, ConfigValueProblem};
use crate::defaults::shipped_scalar;
use crate::ledger_observer::LedgerObserverConfig;
use crate::recovery_ladder::LadderConfig;
use crate::scheduling::SchedulingConfig;
use crate::table::{DefaultSource, INSTANCE_SECTION, KeyName, KeyRange, KeyRow, find_row, rows};
use crate::value::{canonical_json, parse_value, scalar_json};
use crate::{host_claims, instance, ledger_observer, performance, recovery_ladder, scheduling};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, ConfigSettingChange, ConfigSubsystem,
    ConfiguredSetting, FactScalar, RuntimeConfigManifest, config_parameter_section,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// The `host.config_overrides` row field naming the overridden key.
pub const OVERRIDE_CONFIG_KEY: &str = "config_key";
/// The row field holding the instruction value, canonical JSON text.
pub const OVERRIDE_VALUE_JSON: &str = "value_json";
/// The row field holding the file-layer value the instruction overrode, canonical JSON text.
pub const OVERRIDE_BASE_JSON: &str = "base_json";
/// The row field naming where that overridden value came from: `explicit` or `default`.
pub const OVERRIDE_BASE_SOURCE: &str = "base_source";
/// The row field holding when the instruction was set.
pub const OVERRIDE_SET_UNIX_MS: &str = "set_unix_ms";
/// The row field naming the owner epoch the instruction was set in.
pub const OVERRIDE_SET_IN_OWNER_EPOCH: &str = "set_in_owner_epoch";
/// The row field naming the request that set the instruction.
pub const OVERRIDE_REQUEST_ID: &str = "request_id";
/// The row field naming the channel the instruction came through (written from U-2 on).
pub const OVERRIDE_CHANNEL: &str = "channel";

/// The device layer's own bounds of the steps a recovery ladder's rungs take (M387 §4.2), in
/// milliseconds: a rung configured shorter than the device would time out every time, so the
/// `recovery_ladder` check refuses it. Device facts, not ladder values (Workflow #387 D1c).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceBounds {
    /// Stopping the instance's assigned application (an ADB application command).
    pub application_stop_ms: u64,
    /// An emulator Stop through the control owner, its readiness wait included.
    pub emulator_stop_ms: u64,
    /// An emulator Start through the control owner, its readiness wait included.
    pub emulator_start_ms: u64,
}

/// Facts of other crates a whole-section check reads (§5.1, §5.3): the caller fills them in,
/// so this crate depends on neither the scheduler nor the device layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct CheckContext {
    /// `scheduler.maximum_client_heartbeat_interval_ms` in effect (§4.3).
    pub maximum_client_heartbeat_interval_ms: u64,
    /// Workflow #387 D1c (D1b deviation 14): the device layer's bounds the `recovery_ladder`
    /// check reads (§4.2).
    pub device: DeviceBounds,
}

impl CheckContext {
    pub const fn new(maximum_client_heartbeat_interval_ms: u64, device: DeviceBounds) -> Self {
        Self {
            maximum_client_heartbeat_interval_ms,
            device,
        }
    }
}

/// Why a key or a value is refused: the code, the key it concerns, why the value is refused
/// and, for a bounded key, its range (§5.6 checks: "带 `config_key` 与原因").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigFailure {
    code: ConfigCode,
    config_key: String,
    problem: Option<ConfigValueProblem>,
    range: Option<(i64, i64)>,
    /// Workflow #386 SR-1: the member of a host blackout entry that breaks a rule (a JSON
    /// pointer into the configuration file) and the entry's id when it has a usable one, the
    /// values of `blackout_config_invalid`.
    blackout: Option<(String, Option<String>)>,
}

impl ConfigFailure {
    pub fn new(code: ConfigCode, config_key: impl Into<String>) -> Self {
        Self {
            code,
            config_key: config_key.into(),
            problem: None,
            range: None,
            blackout: None,
        }
    }

    /// Workflow #386 SR-1: `blackout_config_invalid` for the host blackout entry member at
    /// `field` of the key `config_key` (`scheduling.blackout.host`), with the entry's id when it
    /// has a usable one.
    pub fn blackout(
        config_key: impl Into<String>,
        field: impl Into<String>,
        blackout_id: Option<String>,
    ) -> Self {
        Self {
            blackout: Some((field.into(), blackout_id)),
            ..Self::new(ConfigCode::BlackoutConfigInvalid, config_key)
        }
    }

    /// `config_value_invalid` for `config_key`, for `problem`.
    pub fn invalid_value(config_key: impl Into<String>, problem: ConfigValueProblem) -> Self {
        Self::with_problem(ConfigCode::ConfigValueInvalid, config_key, problem)
    }

    /// `code` for `config_key`, for `problem`.
    pub fn with_problem(
        code: ConfigCode,
        config_key: impl Into<String>,
        problem: ConfigValueProblem,
    ) -> Self {
        Self {
            problem: Some(problem),
            ..Self::new(code, config_key)
        }
    }

    /// The failure as an instruction meets it (M387 §5.6): a section's broken rule refuses the
    /// whole request as `config_value_invalid`, keeping its key and why.
    fn into_instruction(self) -> Self {
        match self.code {
            ConfigCode::RecoveryLadderConfigInvalid
            | ConfigCode::PerfConfigInvalid
            | ConfigCode::BlackoutConfigInvalid => Self {
                code: ConfigCode::ConfigValueInvalid,
                problem: self.problem.or(Some(ConfigValueProblem::SectionCheck)),
                blackout: None,
                ..self
            },
            _ => self,
        }
    }

    /// The failure with the key's integer range, when it has one.
    pub fn with_range(mut self, range: KeyRange) -> Self {
        if let KeyRange::Integer { min, max } = range {
            self.range = Some((min, max));
        }
        self
    }

    pub const fn failure_code(&self) -> ConfigCode {
        self.code
    }

    pub fn config_key(&self) -> &str {
        &self.config_key
    }

    pub const fn problem(&self) -> Option<ConfigValueProblem> {
        self.problem
    }

    /// Workflow #386 SR-1: the member pointer of a host blackout entry that breaks a rule.
    pub fn field(&self) -> Option<&str> {
        self.blackout.as_ref().map(|(field, _)| field.as_str())
    }

    /// Workflow #386 SR-1: the id of a host blackout entry that breaks a rule, when it has one.
    pub fn blackout_id(&self) -> Option<&str> {
        self.blackout.as_ref().and_then(|(_, id)| id.as_deref())
    }

    /// `outcome` with this failure's values: `config_key`, `config_value_problem` when the value
    /// is refused and, for a bounded key, `range_min` and `range_max`; for
    /// `blackout_config_invalid` (Workflow #386 SR-1) `field` and `blackout_id` instead, the keys
    /// that code declares.
    pub fn with_values(&self, outcome: Outcome) -> Outcome {
        if let Some((field, blackout_id)) = &self.blackout {
            let outcome = outcome.field(field.clone());
            return match blackout_id {
                Some(blackout_id) => outcome.blackout_id(blackout_id.clone()),
                None => outcome,
            };
        }
        let outcome = outcome.config_key(self.config_key.clone());
        let outcome = match self.problem {
            Some(problem) => outcome.config_value_problem(problem),
            None => outcome,
        };
        match self.range {
            Some((min, max)) => outcome.range_min(min).range_max(max),
            None => outcome,
        }
    }

    /// This failure as an outcome of its own.
    pub fn outcome(&self) -> Outcome {
        self.with_values(Outcome::new(self.code))
    }
}

/// Who set an instruction and when: the request's identity, kept on its row (§5.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionStamp {
    pub set_unix_ms: u64,
    pub set_in_owner_epoch: String,
    pub request_id: String,
    /// Workflow #389 U-2: the channel the instruction came through, when the request passed the
    /// door bound to one.
    pub channel: Option<actingcommand_contract::RuntimeChannel>,
}

/// One instruction value with what it overrode (§5.7 `host.config_overrides` row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionValue {
    pub value: FactScalar,
    pub value_json: String,
    pub base_json: String,
    pub base_source: ConfigParameterSource,
    pub stamp: InstructionStamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Setting {
    key: String,
    row: &'static KeyRow,
    instance: Option<String>,
    file_value: FactScalar,
    file_source: ConfigParameterSource,
    instruction: Option<InstructionValue>,
}

impl Setting {
    /// The canonical JSON of the file-layer value.
    fn file_json(&self) -> Option<String> {
        scalar_json(self.row.value_type, &self.file_value).map(|value| canonical_json(&value))
    }
}

/// What one `ConfigureSettings` leads to: the configuration afterwards, each changed key as it
/// is in effect afterwards (the receipt, §5.6) and the sections whose `config.parameters.<section>`
/// fact changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedSettings {
    pub config: EffectiveConfig,
    pub settings: Vec<ConfiguredSetting>,
    pub sections: BTreeSet<String>,
}

/// One instruction value a start did not restore as it was (§5.7): `config_override_superseded`,
/// `config_override_base_changed` (restored) or `config_override_dropped`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreNote {
    pub note: ConfigCode,
    pub config_key: String,
}

impl RestoreNote {
    pub fn outcome(&self) -> Outcome {
        Outcome::new(self.note).config_key(self.config_key.clone())
    }
}

/// A `host.config_overrides` record that is not the shape this build writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverridesInvalid;

/// The configuration in effect.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectiveConfig {
    subsystems: Vec<ConfigSubsystem>,
    settings: Vec<Setting>,
}

impl EffectiveConfig {
    /// The file layer as actingd assembled it, without any instruction value. Every key must
    /// be in the key table (`config_key_unknown`).
    pub fn from_manifest(manifest: &RuntimeConfigManifest) -> Result<Self, ConfigFailure> {
        let settings = manifest
            .parameters
            .iter()
            .map(|parameter| {
                let (row, instance) = find_row(&parameter.key).ok_or_else(|| {
                    ConfigFailure::new(ConfigCode::ConfigKeyUnknown, parameter.key.clone())
                })?;
                Ok(Setting {
                    key: parameter.key.clone(),
                    row,
                    instance: instance.map(str::to_owned),
                    file_value: parameter.value.clone(),
                    file_source: parameter.source,
                    instruction: None,
                })
            })
            .collect::<Result<Vec<_>, ConfigFailure>>()?;
        Ok(Self {
            subsystems: manifest.subsystems.clone(),
            settings,
        })
    }

    /// The whole-section check of every section (§5.3): the file layer at start, before any
    /// instruction value is restored.
    pub fn check(&self, context: &CheckContext) -> Result<(), ConfigFailure> {
        let sections = self
            .settings
            .iter()
            .map(|setting| config_parameter_section(&setting.key).to_owned())
            .collect::<BTreeSet<_>>();
        self.check_sections(&sections, context)
    }

    /// The manifest as it is in effect: each key's effective value and source (§5.9).
    pub fn manifest(&self) -> RuntimeConfigManifest {
        RuntimeConfigManifest {
            subsystems: self.subsystems.clone(),
            parameters: self
                .settings
                .iter()
                .map(|setting| self.parameter(setting))
                .collect(),
        }
    }

    /// A setting's value in effect and its source: its instruction value; otherwise, for a key
    /// whose default is a section-level key and that the file does not name, that key's
    /// instruction value (Workflow #387 D1c, §5.5 `section:` takes the section key's value in
    /// effect); otherwise its file-layer value.
    fn effective<'a>(&'a self, setting: &'a Setting) -> (&'a FactScalar, ConfigParameterSource) {
        if let Some(instruction) = &setting.instruction {
            return (&instruction.value, ConfigParameterSource::Instruction);
        }
        if setting.file_source == ConfigParameterSource::Default
            && let DefaultSource::Section(section_key) = setting.row.default
            && let Some(instruction) = self
                .setting(section_key)
                .and_then(|section| section.instruction.as_ref())
        {
            return (&instruction.value, ConfigParameterSource::Default);
        }
        (&setting.file_value, setting.file_source)
    }

    fn parameter(&self, setting: &Setting) -> ConfigParameter {
        let (value, source) = self.effective(setting);
        ConfigParameter {
            key: setting.key.clone(),
            value: value.clone(),
            source,
            settable: setting.row.settable.is_settable(),
        }
    }

    /// The effective value of `key`, when the configuration has the key.
    pub fn value(&self, key: &str) -> Option<&FactScalar> {
        self.setting(key).map(|setting| self.effective(setting).0)
    }

    /// The effective value of `key`, or the defaults file's when the configuration does not
    /// carry the key (a host started without actingd's manifest; §5.2: the lowest layer).
    fn value_or_shipped(&self, key: &str) -> Option<FactScalar> {
        self.value(key).cloned().or_else(|| shipped_scalar(key))
    }

    /// Workflow #387 D1c: the `recovery_ladder` section in effect, read and checked whole
    /// (§5.3) with the facts of other crates in `context`.
    pub fn recovery_ladder(&self, context: &CheckContext) -> Result<LadderConfig, ConfigFailure> {
        LadderConfig::parse(&|key: &str| self.value_or_shipped(key), context)
    }

    /// Workflow #387 D1c: `host_claims.renewal_slack_ms` in effect, in milliseconds.
    pub fn renewal_slack_ms(&self) -> Result<u64, ConfigFailure> {
        host_claims::renewal_slack_ms(&|key: &str| self.value_or_shipped(key))
    }

    /// Workflow #385 P-c: the `performance` section in effect, checked whole (PERF §4.1,
    /// §10.4).
    pub fn check_performance(&self) -> Result<(), ConfigFailure> {
        performance::check(&|key: &str| self.value_or_shipped(key))
    }

    /// Workflow #386 SR-1: the `scheduling` section in effect, read and checked whole.
    pub fn scheduling(&self) -> Result<SchedulingConfig, ConfigFailure> {
        SchedulingConfig::parse(&|key: &str| self.value_or_shipped(key))
    }

    /// Workflow #383 AO-1: the `ledger_observer` section in effect, read and checked whole
    /// (`contracts/ledger-observer.md` §15); the host builds the ledger writer's failure plan
    /// from it before the ledger opens.
    pub fn ledger_observer(&self) -> Result<LedgerObserverConfig, ConfigFailure> {
        LedgerObserverConfig::parse(&|key: &str| self.value_or_shipped(key))
    }

    /// Workflow #386 SR-1: `scheduling.recognition.sample_timeout_retries` in effect, the base
    /// number of observe-only retries the host hands the kernel at each step.
    pub fn sample_timeout_retries(&self) -> Result<u32, ConfigFailure> {
        scheduling::sample_timeout_retries(&|key: &str| self.value_or_shipped(key))
    }

    /// Workflow #384 D2-S2: `scheduling.fact_refresh.recognition_join_timeout_ms` in effect, in
    /// milliseconds: how long a successful run waits for the readings it handed to recognition.
    pub fn recognition_join_timeout_ms(&self) -> Result<u64, ConfigFailure> {
        scheduling::recognition_join_timeout_ms(&|key: &str| self.value_or_shipped(key))
    }

    /// The instruction value of `key`, when one is in effect.
    pub fn instruction(&self, key: &str) -> Option<&InstructionValue> {
        self.setting(key)
            .and_then(|setting| setting.instruction.as_ref())
    }

    fn setting(&self, key: &str) -> Option<&Setting> {
        self.settings.iter().find(|setting| setting.key == key)
    }

    /// Applies one `ConfigureSettings`, all changes or none (§5.6): every key in the key table
    /// (`config_key_unknown`), settable (`config_key_not_settable`), each value of the key's
    /// type and range and every changed section whole (`config_value_invalid`). A change without
    /// a value returns the key to its file-layer value.
    pub fn apply(
        &self,
        changes: &[ConfigSettingChange],
        stamp: &InstructionStamp,
        context: &CheckContext,
    ) -> Result<AppliedSettings, ConfigFailure> {
        let mut next = self.clone();
        let mut seen = BTreeSet::new();
        let mut sections = BTreeSet::new();
        for change in changes {
            if !seen.insert(change.key.as_str()) {
                return Err(ConfigFailure::invalid_value(
                    change.key.clone(),
                    ConfigValueProblem::RepeatedKey,
                ));
            }
            let setting = next
                .settings
                .iter_mut()
                .find(|setting| setting.key == change.key)
                .ok_or_else(|| {
                    ConfigFailure::new(ConfigCode::ConfigKeyUnknown, change.key.clone())
                })?;
            if !setting.row.settable.is_settable() {
                return Err(ConfigFailure::new(
                    ConfigCode::ConfigKeyNotSettable,
                    change.key.clone(),
                ));
            }
            setting.instruction = match &change.value {
                None => None,
                Some(value) => {
                    let parsed = parse_value(setting.row, &change.key, value)?;
                    let base_json = setting.file_json().ok_or_else(|| {
                        ConfigFailure::invalid_value(
                            change.key.clone(),
                            ConfigValueProblem::WrongType,
                        )
                    })?;
                    Some(InstructionValue {
                        value: parsed,
                        value_json: canonical_json(value),
                        base_json,
                        base_source: base_source(setting.file_source),
                        stamp: stamp.clone(),
                    })
                }
            };
            sections.insert(config_parameter_section(&change.key).to_owned());
            // A key whose default is the changed one changes with it (§5.5 `section:`).
            for row in rows() {
                if matches!(row.default, DefaultSource::Section(key) if key == change.key) {
                    sections.insert(match row.name {
                        KeyName::PerInstance(_) => INSTANCE_SECTION.to_owned(),
                        KeyName::Exact(name) => config_parameter_section(name).to_owned(),
                    });
                }
            }
        }
        next.check_sections(&sections, context)
            .map_err(ConfigFailure::into_instruction)?;
        let settings = changes
            .iter()
            .map(|change| {
                let setting = next.setting(&change.key).ok_or_else(|| {
                    ConfigFailure::new(ConfigCode::ConfigKeyUnknown, change.key.clone())
                })?;
                let (value, source) = next.effective(setting);
                Ok(ConfiguredSetting {
                    key: change.key.clone(),
                    value: value.clone(),
                    source,
                })
            })
            .collect::<Result<Vec<_>, ConfigFailure>>()?;
        Ok(AppliedSettings {
            config: next,
            settings,
            sections,
        })
    }

    /// The whole-section checks of `sections` against the values in effect (§5.3), with the
    /// facts of other crates in `context`: the `recovery_ladder` section whole (D1c), the
    /// `host_claims` slack (D1c), the `performance` section whole (Workflow #385 P-c), the
    /// `scheduling` section whole (Workflow #386 SR-1), the `ledger_observer` section whole
    /// (Workflow #383 AO-1) and each
    /// instance of the per-instance section (D1b).
    fn check_sections(
        &self,
        sections: &BTreeSet<String>,
        context: &CheckContext,
    ) -> Result<(), ConfigFailure> {
        if sections.contains(recovery_ladder::SECTION) {
            self.recovery_ladder(context)?;
        }
        if sections.contains(host_claims::SECTION) {
            self.renewal_slack_ms()?;
        }
        if sections.contains(performance::SECTION) {
            self.check_performance()?;
        }
        if sections.contains(scheduling::SECTION) {
            self.scheduling()?;
        }
        if sections.contains(ledger_observer::SECTION) {
            self.ledger_observer()?;
        }
        if !sections.contains(INSTANCE_SECTION) {
            return Ok(());
        }
        let instances = self
            .settings
            .iter()
            .filter_map(|setting| setting.instance.as_deref())
            .collect::<BTreeSet<_>>();
        for instance in instances {
            instance::check_instance(instance, |key| self.value(key).cloned())?;
        }
        Ok(())
    }

    /// The `host.config_overrides` rows of the instruction values in effect, in manifest order
    /// (§5.7).
    pub fn override_rows(&self) -> Vec<BTreeMap<String, FactScalar>> {
        self.settings
            .iter()
            .filter_map(|setting| {
                let instruction = setting.instruction.as_ref()?;
                let mut row = BTreeMap::from([
                    (
                        OVERRIDE_CONFIG_KEY.to_owned(),
                        FactScalar::String(setting.key.clone()),
                    ),
                    (
                        OVERRIDE_VALUE_JSON.to_owned(),
                        FactScalar::String(instruction.value_json.clone()),
                    ),
                    (
                        OVERRIDE_BASE_JSON.to_owned(),
                        FactScalar::String(instruction.base_json.clone()),
                    ),
                    (
                        OVERRIDE_BASE_SOURCE.to_owned(),
                        FactScalar::String(instruction.base_source.as_str().to_owned()),
                    ),
                    (
                        OVERRIDE_SET_UNIX_MS.to_owned(),
                        FactScalar::TimestampMs(instruction.stamp.set_unix_ms),
                    ),
                    (
                        OVERRIDE_SET_IN_OWNER_EPOCH.to_owned(),
                        FactScalar::String(instruction.stamp.set_in_owner_epoch.clone()),
                    ),
                    (
                        OVERRIDE_REQUEST_ID.to_owned(),
                        FactScalar::String(instruction.stamp.request_id.clone()),
                    ),
                ]);
                if let Some(channel) = instruction.stamp.channel {
                    row.insert(
                        OVERRIDE_CHANNEL.to_owned(),
                        FactScalar::String(channel.as_str().to_owned()),
                    );
                }
                Some(row)
            })
            .collect()
    }

    /// Restores the instruction values of a recorded `host.config_overrides` at start
    /// (§5.6, §5.7, §12 question 3: the later change wins), and returns one note per value not
    /// restored as it was:
    ///
    /// - the key no longer exists, is no longer settable, or its value no longer fits:
    ///   dropped (`config_override_dropped`);
    /// - it overrode an explicit value and the file changed or removed that value, or it
    ///   overrode a default and the file names the key now: dropped
    ///   (`config_override_superseded`);
    /// - it overrode a default that changed while the file still does not name the key: kept,
    ///   with the new default as what it overrides (`config_override_base_changed`);
    /// - otherwise: kept as it was.
    ///
    /// The restored values then pass the whole-section check of their sections with this
    /// start's file layer (§5.3); a section that fails it drops every value restored into it
    /// (`config_override_dropped`). A record not of the shape this build writes is refused
    /// whole.
    pub fn restore(
        &mut self,
        rows: &[BTreeMap<String, FactScalar>],
        context: &CheckContext,
    ) -> Result<Vec<RestoreNote>, OverridesInvalid> {
        let mut seen = BTreeSet::new();
        let mut notes = Vec::new();
        let mut restored_sections = BTreeSet::new();
        for row in rows {
            let recorded = RecordedOverride::parse(row)?;
            if !seen.insert(recorded.config_key.clone()) {
                return Err(OverridesInvalid);
            }
            let note = |code| RestoreNote {
                note: code,
                config_key: recorded.config_key.clone(),
            };
            let Some(setting) = self
                .settings
                .iter_mut()
                .find(|setting| setting.key == recorded.config_key)
            else {
                notes.push(note(ConfigCode::ConfigOverrideDropped));
                continue;
            };
            let value = serde_json::from_str::<Value>(&recorded.value_json)
                .ok()
                .and_then(|value| {
                    parse_value(setting.row, &recorded.config_key, &value)
                        .ok()
                        .map(|parsed| (parsed, canonical_json(&value)))
                });
            let (Some((value, value_json)), Some(current_json), true) = (
                value,
                setting.file_json(),
                setting.row.settable.is_settable(),
            ) else {
                notes.push(note(ConfigCode::ConfigOverrideDropped));
                continue;
            };
            let file_names_key = setting.file_source == ConfigParameterSource::Explicit;
            let base_json = match recorded.base_source {
                ConfigParameterSource::Explicit
                    if file_names_key && current_json == recorded.base_json =>
                {
                    recorded.base_json.clone()
                }
                ConfigParameterSource::Default if !file_names_key => {
                    if current_json != recorded.base_json {
                        notes.push(note(ConfigCode::ConfigOverrideBaseChanged));
                    }
                    current_json
                }
                _ => {
                    notes.push(note(ConfigCode::ConfigOverrideSuperseded));
                    continue;
                }
            };
            setting.instruction = Some(InstructionValue {
                value,
                value_json,
                base_json,
                base_source: recorded.base_source,
                stamp: recorded.stamp,
            });
            restored_sections.insert(config_parameter_section(&recorded.config_key).to_owned());
        }
        for section in restored_sections {
            if self
                .check_sections(&BTreeSet::from([section.clone()]), context)
                .is_ok()
            {
                continue;
            }
            for setting in &mut self.settings {
                if config_parameter_section(&setting.key) == section
                    && setting.instruction.take().is_some()
                {
                    notes.retain(|note| note.config_key != setting.key);
                    notes.push(RestoreNote {
                        note: ConfigCode::ConfigOverrideDropped,
                        config_key: setting.key.clone(),
                    });
                }
            }
        }
        Ok(notes)
    }
}

/// What an instruction value overrode, as a row records it: `explicit` when the file named the
/// key, `default` for every other file-layer source (the restore rule tells only these two
/// apart, §12 question 3).
fn base_source(file_source: ConfigParameterSource) -> ConfigParameterSource {
    match file_source {
        ConfigParameterSource::Explicit => ConfigParameterSource::Explicit,
        ConfigParameterSource::Default
        | ConfigParameterSource::Discovered
        | ConfigParameterSource::Instruction => ConfigParameterSource::Default,
    }
}

/// One recorded `host.config_overrides` row.
struct RecordedOverride {
    config_key: String,
    value_json: String,
    base_json: String,
    base_source: ConfigParameterSource,
    stamp: InstructionStamp,
}

impl RecordedOverride {
    fn parse(row: &BTreeMap<String, FactScalar>) -> Result<Self, OverridesInvalid> {
        const FIELDS: [&str; 8] = [
            OVERRIDE_CONFIG_KEY,
            OVERRIDE_VALUE_JSON,
            OVERRIDE_BASE_JSON,
            OVERRIDE_BASE_SOURCE,
            OVERRIDE_SET_UNIX_MS,
            OVERRIDE_SET_IN_OWNER_EPOCH,
            OVERRIDE_REQUEST_ID,
            OVERRIDE_CHANNEL,
        ];
        if row.keys().any(|field| !FIELDS.contains(&field.as_str())) {
            return Err(OverridesInvalid);
        }
        let text = |field: &str| match row.get(field) {
            Some(FactScalar::String(text)) => Ok(text.clone()),
            _ => Err(OverridesInvalid),
        };
        let base_source = match text(OVERRIDE_BASE_SOURCE)?.as_str() {
            source if source == ConfigParameterSource::Explicit.as_str() => {
                ConfigParameterSource::Explicit
            }
            source if source == ConfigParameterSource::Default.as_str() => {
                ConfigParameterSource::Default
            }
            _ => return Err(OverridesInvalid),
        };
        let set_unix_ms = match row.get(OVERRIDE_SET_UNIX_MS) {
            Some(FactScalar::TimestampMs(at)) => *at,
            _ => return Err(OverridesInvalid),
        };
        let channel = match row.get(OVERRIDE_CHANNEL) {
            None => None,
            Some(FactScalar::String(channel)) => Some(
                serde_json::from_value::<actingcommand_contract::RuntimeChannel>(Value::String(
                    channel.clone(),
                ))
                .map_err(|_| OverridesInvalid)?,
            ),
            Some(_) => return Err(OverridesInvalid),
        };
        // A key is a configuration key's spelling, the type of the outcome key `config_key`.
        let config_key = text(OVERRIDE_CONFIG_KEY)?;
        if config_key.is_empty()
            || !config_key.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'_' | b'.' | b':' | b'-')
            })
        {
            return Err(OverridesInvalid);
        }
        Ok(Self {
            config_key,
            value_json: text(OVERRIDE_VALUE_JSON)?,
            base_json: text(OVERRIDE_BASE_JSON)?,
            base_source,
            stamp: InstructionStamp {
                set_unix_ms,
                set_in_owner_epoch: text(OVERRIDE_SET_IN_OWNER_EPOCH)?,
                request_id: text(OVERRIDE_REQUEST_ID)?,
                channel,
            },
        })
    }
}
