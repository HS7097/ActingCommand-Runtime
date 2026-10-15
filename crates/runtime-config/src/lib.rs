// SPDX-License-Identifier: AGPL-3.0-only

//! The Runtime's shared configuration mechanism (Workflow #387 D1b, frozen model v1.2 §5).
//!
//! - The key table: one row per configuration key with its file location, type, range, default
//!   source, whether an instruction may change it while the Runtime runs, its reader and since
//!   when (§5.5). One file per section (§5.1).
//! - The defaults file `actingd.defaults.json`, compiled in (§5.2).
//! - The file layer of the rows this crate resolves: defaults ⊕ file, a list replaced whole
//!   (§5.3).
//! - The configuration in effect: the file layer ⊕ instruction values, changed by
//!   `ConfigureSettings` (all changes or none) and restored at start (§5.6, §5.7, §12 question 3).
//! - The sections (D1c): `recovery_ladder` (§3, §4, §7), `host_claims` (§4.3) and
//!   `standard_packs` (Workflow #373 NAV-6, moved here); `performance` (Workflow #385 P-c, PERF
//!   §10); `scheduling` (Workflow #386 SR-1, with Workflow #405's random delay);
//!   `ledger_observer` (Workflow #383 AO-1, `contracts/ledger-observer.md` §15); the two Lab
//!   options `lab_module_authoring` and `lab_module_debugging` (Workflow #389 U-2); `device_link`
//!   (Workflow #398 RD-2, model-398 §4.6); `device_state` (Workflow #398 RD-5b, model-398 §7.8).
//!
//! actingd reads the files and hands the assembled manifest to the host; the host applies
//! instructions through this crate. It depends on the contract only (§5.1).

#![forbid(unsafe_code)]

mod codes;
mod defaults;
pub mod device_link;
pub mod device_state;
mod effective;
mod existing;
mod host_claims;
mod instance;
pub mod lab_module;
pub mod ledger_observer;
pub mod performance;
pub mod recovery_ladder;
mod resolve;
pub mod scheduling;
mod standard_packs;
mod table;
mod value;

pub use codes::{ConfigCode, ConfigValueProblem};
pub use defaults::{
    DefaultsFile, DefaultsInvalid, SHIPPED_DEFAULTS, sha256_text, shipped_defaults_sha256,
};
pub use device_link::{DeviceLinkConfig, DeviceLinkFile, OfflineAction};
pub use device_state::{DeviceStateConfig, DeviceStateFile, KeyguardInsecureAction, ThermalLevel};
pub use effective::{
    AppliedSettings, CheckContext, ConfigFailure, DeviceBounds, EffectiveConfig, InstructionStamp,
    InstructionValue, OVERRIDE_BASE_JSON, OVERRIDE_BASE_SOURCE, OVERRIDE_CHANNEL,
    OVERRIDE_CONFIG_KEY, OVERRIDE_REQUEST_ID, OVERRIDE_SET_IN_OWNER_EPOCH, OVERRIDE_SET_UNIX_MS,
    OVERRIDE_VALUE_JSON, OverridesInvalid, RestoreNote,
};
pub use host_claims::{HostClaimsFile, RENEWAL_SLACK_MS};
pub use instance::{
    DEVICE_KIND, DEVICE_KIND_TOKENS, RECOGNITION_PACKAGE, START_EMULATOR, STUCK_RECOVERY,
    STUCK_RECOVERY_COOLDOWN_SECS,
};
pub use ledger_observer::{CaptureTiers, LedgerObserverConfig, LedgerObserverFile};
pub use performance::PerformanceFile;
pub use recovery_ladder::{
    AlreadyRunning, BurstWindow, CodePattern, CooldownOpensOn, DirectRuns, EntryUnavailable,
    HoldConfig, LadderConfig, LadderRules, MatchedBy, PreparationSuperseded, RecoveryLadderFile,
    RungConfig, TriggerItem, TriggerVerdict, ladder_class_of,
};
pub use resolve::{file_value, member, render_parameters};
pub use scheduling::{
    BlackoutConfig, CostCommitConfig, FactRefreshConfig, FeatureLockConfig, HostBlackout,
    InstanceSchedulingFile, PacingStage, RandomDelay, RerunConfig, RerunExpiry, SchedulingConfig,
    SchedulingFile,
};
pub use standard_packs::{ROOT as STANDARD_PACKS_ROOT, StandardPacksFile};
pub use table::{
    DefaultSource, FilePath, INSTANCE_SECTION, KeyName, KeyRange, KeyRow, NotSettable, Settable,
    ValueType, assembly_rows, find_row, instance_key, mechanism_rows, rows,
};
pub use value::{canonical_json, parse_value, scalar_json};

/// Annotates the rows the start-up assembly built with whether an instruction may change them
/// (§5.5 `settable`); a key the key table does not have is `config_key_unknown`.
pub fn annotate_settable(
    parameters: &mut [actingcommand_contract::ConfigParameter],
) -> Result<(), ConfigFailure> {
    for parameter in parameters {
        let (row, _) = find_row(&parameter.key).ok_or_else(|| {
            ConfigFailure::new(ConfigCode::ConfigKeyUnknown, parameter.key.clone())
        })?;
        parameter.settable = row.settable.is_settable();
    }
    Ok(())
}

// Workflow #398 RD-2.
#[cfg(test)]
mod device_link_tests;
// Workflow #398 RD-5b.
#[cfg(test)]
mod device_state_tests;
#[cfg(test)]
mod ladder_tests;
// Workflow #383 AO-1.
#[cfg(test)]
mod ledger_observer_tests;
// Workflow #386 SR-1.
#[cfg(test)]
mod perf_tests;
#[cfg(test)]
mod scheduling_tests;
#[cfg(test)]
mod tests;
