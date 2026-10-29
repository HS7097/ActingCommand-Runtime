// SPDX-License-Identifier: AGPL-3.0-only

//! The defaults file `distribution/windows/actingd.defaults.json` (Workflow #387 model v1.2
//! §5.2): the lowest layer of every key whose `default` is `file`, compiled into the program and
//! shipped beside the example configuration. It is parsed by its own structure, with no
//! top-level required member; a member without parsing code is refused. Each section adds its
//! own member here (D1b ships the mechanism; D1c adds `recovery_ladder` and `host_claims`;
//! Workflow #385 P-c adds `performance`; Workflow #386 SR-1 adds `scheduling`; Workflow #383
//! AO-1 adds `ledger_observer`; Workflow #398 RD-2 adds `device_link`; Workflow #398 RD-5b adds
//! `device_state`; Workflow #390 DISC-1 adds `startup_discovery`; Workflow #406 T12 adds
//! `touch`; #411 A50 adds `emulator_control`; #411 A66 adds `watchdog`; PERF-B1 adds
//! `execution`; IDLE-1 adds `idle`; ROUT-L RL-1b adds `standard_packs`, the scheduler
//! workbench's two keys; Workflow #392 DA-4b-D adds `display_safe_area`; #422 LN-7c Q adds
//! `frame_retention_evidence_max_age_days`; Workflow #421 INST-1 adds `install`; RB-2 adds
//! `host`).

use crate::device_link::DeviceLinkFile;
use crate::device_state::DeviceStateFile;
use crate::display_safe_area::DisplaySafeAreaFile;
use crate::emulator_control::EmulatorControlFile;
use crate::execution::ExecutionFile;
use crate::host::HostFile;
use crate::host_claims::HostClaimsFile;
use crate::idle::IdleFile;
use crate::install::InstallFile;
use crate::ledger_observer::LedgerObserverFile;
use crate::performance::PerformanceFile;
use crate::recovery_ladder::RecoveryLadderFile;
use crate::resolve::member;
use crate::scheduling::SchedulingFile;
use crate::standard_packs::StandardPacksDefaultsFile;
use crate::startup_discovery::StartupDiscoveryFile;
use crate::table::find_row;
use crate::touch::TouchFile;
use crate::value::parse_value;
use crate::watchdog::WatchdogFile;
use actingcommand_contract::FactScalar;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

/// The defaults file the program embeds; the build ships the same bytes.
pub const SHIPPED_DEFAULTS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../distribution/windows/actingd.defaults.json"
));

/// The defaults file's sections, each as strict as in the configuration file.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefaultsFile {
    #[serde(default)]
    pub recovery_ladder: Option<RecoveryLadderFile>,
    #[serde(default)]
    pub host_claims: Option<HostClaimsFile>,
    #[serde(default)]
    pub performance: Option<PerformanceFile>,
    #[serde(default)]
    pub scheduling: Option<SchedulingFile>,
    #[serde(default)]
    pub ledger_observer: Option<LedgerObserverFile>,
    /// Workflow #389 U-2: the two Lab options, `false` (not installed).
    #[serde(default)]
    pub lab_module_authoring: Option<bool>,
    #[serde(default)]
    pub lab_module_debugging: Option<bool>,
    #[serde(default)]
    pub device_link: Option<DeviceLinkFile>,
    #[serde(default)]
    pub device_state: Option<DeviceStateFile>,
    #[serde(default)]
    pub startup_discovery: Option<StartupDiscoveryFile>,
    #[serde(default)]
    pub touch: Option<TouchFile>,
    /// #411 A50.
    #[serde(default)]
    pub emulator_control: Option<EmulatorControlFile>,
    /// #411 A66.
    #[serde(default)]
    pub watchdog: Option<WatchdogFile>,
    /// PERF-B1 (Workflow #407 B1).
    #[serde(default)]
    pub execution: Option<ExecutionFile>,
    /// IDLE-1.
    #[serde(default)]
    pub idle: Option<IdleFile>,
    /// ROUT-L RL-1b: `check_interval_ms` and `cached_generations` (no root).
    #[serde(default)]
    pub standard_packs: Option<StandardPacksDefaultsFile>,
    /// Workflow #392 DA-4b-D.
    #[serde(default)]
    pub display_safe_area: Option<DisplaySafeAreaFile>,
    /// #422 LN-7c Q: the evidence-frame rule's age, 15 days.
    #[serde(default)]
    pub frame_retention_evidence_max_age_days: Option<Value>,
    /// Workflow #421 INST-1.
    #[serde(default)]
    pub install: Option<InstallFile>,
    /// RB-2: both priorities `normal`.
    #[serde(default)]
    pub host: Option<HostFile>,
}

/// A defaults file that is not a JSON object of known sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultsInvalid;

impl DefaultsFile {
    /// Parses a defaults file: its sections, and the JSON the file layer reads defaults from.
    pub fn parse(text: &str) -> Result<(Self, Value), DefaultsInvalid> {
        let value = serde_json::from_str::<Value>(text).map_err(|_| DefaultsInvalid)?;
        if !value.is_object() {
            return Err(DefaultsInvalid);
        }
        let sections = Self::deserialize(&value).map_err(|_| DefaultsInvalid)?;
        Ok((sections, value))
    }

    /// The embedded defaults file, parsed.
    pub fn shipped() -> Result<(Self, Value), DefaultsInvalid> {
        Self::parse(SHIPPED_DEFAULTS)
    }
}

/// The embedded defaults file as JSON, parsed once; `None` when it does not parse (a start
/// then refuses it, `DefaultsFile::shipped`).
pub(crate) fn shipped_json() -> Option<&'static Value> {
    static PARSED: OnceLock<Option<Value>> = OnceLock::new();
    PARSED
        .get_or_init(|| DefaultsFile::shipped().ok().map(|(_, value)| value))
        .as_ref()
}

/// The defaults file's value of `key`, as the key table reads it: the lowest layer of a key
/// whose `default` is the file, for a configuration in effect that does not carry the key (a
/// host started without actingd's manifest).
pub(crate) fn shipped_scalar(key: &str) -> Option<FactScalar> {
    let (row, _) = find_row(key)?;
    let value = member(shipped_json()?, key)?;
    parse_value(row, key, value).ok()
}

/// `sha256:<hex>` of `bytes`: how `config.selection` names the configuration and defaults files.
pub fn sha256_text(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// The digest of the embedded defaults file (§5.2, §5.7).
pub fn shipped_defaults_sha256() -> String {
    sha256_text(SHIPPED_DEFAULTS.as_bytes())
}
