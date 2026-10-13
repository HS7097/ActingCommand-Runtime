// SPDX-License-Identifier: AGPL-3.0-only

//! The configuration mechanism's outcome codes (Workflow #387 model v1.2 §6.4; Workflow #378).
//! Every entry is listed in the fragment `contracts/outcome-codes/runtime-config.json`; the
//! outcome guard (G1) checks both directions. Every entry is pending.

actingcommand_contract::outcome_codes! {
    /// The codes of the shared configuration mechanism and of the sections it parses.
    pub enum ConfigCode {
        /// Error. A `ConfigureSettings` change names a key the key table does not have, or a
        /// per-instance key of an instance that is not configured (key `config_key`; D1b).
        ConfigKeyUnknown => "config_key_unknown": error / none,
        /// Error. A `ConfigureSettings` change names a key the Runtime cannot change while it
        /// runs (key `config_key`; D1b).
        ConfigKeyNotSettable => "config_key_not_settable": error / none,
        /// Error. A `ConfigureSettings` change carries a value of the wrong type or outside the
        /// key's range, or the section it belongs to fails its whole-section check (keys
        /// `config_key`, and `range_min` / `range_max` for a bounded key; D1b).
        ConfigValueInvalid => "config_value_invalid": error / none,
        /// Info. At start an instruction value was dropped because the configuration file
        /// changed the key since the value was set (key `config_key`; D1b, §12 question 3).
        ConfigOverrideSuperseded => "config_override_superseded": info / none,
        /// Warning. At start an instruction value was kept although the default it overrode
        /// changed (key `config_key`; D1b, §12 question 3).
        ConfigOverrideBaseChanged => "config_override_base_changed": warning / none,
        /// Warning. At start an instruction value was dropped because its key no longer exists,
        /// can no longer be changed while running, or its value is no longer valid (key
        /// `config_key`; D1b).
        ConfigOverrideDropped => "config_override_dropped": warning / none,
        /// Error. The `recovery_ladder` section breaks a rule (key `config_key`; D1c emits it).
        RecoveryLadderConfigInvalid => "recovery_ladder_config_invalid": error / none,
        /// Warning. The recovery ladder met a code the embedded catalog does not list (D1c
        /// emits it and fixes its values).
        RecoveryLadderCodeUnclassified => "recovery_ladder_code_unclassified": warning / none,
        /// Error. A host blackout entry of the configuration breaks a rule (keys `field`, and
        /// `blackout_id` when the entry has one; registered by SR-0, parsed in this crate by
        /// SR-4a, which emits it).
        BlackoutConfigInvalid => "blackout_config_invalid": error / none,
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Why a configuration value is refused (vocabulary `config_value_problem`, the reason
    /// `config_value_invalid` carries; Workflow #387 model v1.2 §5.6).
    pub enum ConfigValueProblem: "config_value_problem" {
        WrongType => "wrong_type",
        OutOfRange => "out_of_range",
        SectionCheck => "section_check",
        RepeatedKey => "repeated_key",
    }
}
