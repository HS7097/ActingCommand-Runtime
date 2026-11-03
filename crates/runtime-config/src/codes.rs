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
        /// Error. The `recovery_ladder` section breaks a rule at start or in `check-config`
        /// (keys `config_key`, and `config_value_problem` saying which rule; D1c). The same
        /// values sent by an instruction are refused as `config_value_invalid`.
        RecoveryLadderConfigInvalid => "recovery_ladder_config_invalid": error / none,
        /// Warning. The recovery ladder met a code the embedded catalog does not list and read
        /// it as class `none`; once per process and code (key `raw_code`, the spelling; D1c).
        RecoveryLadderCodeUnclassified => "recovery_ladder_code_unclassified": warning / none,
        /// Error. A host blackout entry of the configuration breaks a rule (keys `field`, and
        /// `blackout_id` when the entry has one; registered by SR-0). Workflow #386 SR-1 parses
        /// the entries in this crate and refuses a start and check-config with it; through an
        /// instruction the same entries are `config_value_invalid`.
        BlackoutConfigInvalid => "blackout_config_invalid": error / none,
        /// Error. The `performance` section breaks a rule at start or in `check-config` (keys
        /// `config_key`, and `config_value_problem` saying which rule; Workflow #385 P-c, PERF
        /// §11). The same values sent by an instruction are refused as `config_value_invalid`.
        PerfConfigInvalid => "perf_config_invalid": error / none,
        /// Error. The `agent_wake` section breaks a rule at start or in `check-config` (keys
        /// `config_key`, and `config_value_problem` saying which rule; Workflow #432 AGT-1a). The
        /// same values sent by an instruction are refused as `config_value_invalid`.
        AgentWakeConfigInvalid => "agent_wake_config_invalid": error / none,
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
        /// Workflow #387 D1c: two trigger items share an id.
        DuplicateId => "duplicate_id",
        /// A trigger item names no stage, a stage twice, or a stage no trigger item may name.
        StageNotAllowed => "stage_not_allowed",
        /// A trigger item names neither classes nor codes.
        MatchMissing => "match_missing",
        /// A code, a code prefix or a class the catalog does not list.
        CodeUnknown => "code_unknown",
        /// A code the catalog lists as retired (its `replaced_by` names the successor).
        CodeRetired => "code_retired",
        /// `count` and `window_ms` missing or zero on a stability-warnings item, or present on
        /// any other.
        BurstInvalid => "burst_invalid",
        /// A start rung beyond the configured rungs.
        StartRungOutOfRange => "start_rung_out_of_range",
        /// No rung or more than three, an action twice or unknown, or a member of another
        /// action.
        RungsInvalid => "rungs_invalid",
        /// A rung's bound below the device layer's own bound or outside its allowed range.
        BoundOutOfRange => "bound_out_of_range",
        /// A rung's bound with the hold margin beyond the longest renewal a claim is granted.
        RenewalCapExceeded => "renewal_cap_exceeded",
        /// A ladder claim's TTL not above the heartbeat bound, beyond the longest renewal, or
        /// below the hold margin.
        ClaimTtlOutOfRange => "claim_ttl_out_of_range",
        /// Workflow #432 AGT-1a: the `agent_wake` section leaves out one of its keys; it is whole
        /// or absent.
        RequiredMissing => "required_missing",
        /// A rule's kind outside the section's closed list.
        KindUnknown => "kind_unknown",
        /// A rule's `instances` that is neither `*` nor a list of configured aliases.
        InstanceUnknown => "instance_unknown",
        /// A daily cap above the budget, or the budget above the rows `host.agent_wake` keeps.
        CapsExceed => "caps_exceed",
        /// `agent_wake.body.dir` inside a folder the Runtime keeps its own material in.
        DirReserved => "dir_reserved",
        /// A registered channel whose shortest message does not fit `agent_wake.summary.max_chars`.
        MessageFloor => "message_floor",
        /// Workflow #432 AGT-2b: a `daily_summary` rule that names another kind or codes, leaves
        /// out its minute or offset or names one out of range, or whose day cap is not 1; or a
        /// rule of another kind that names a minute or an offset.
        DailySummaryShape => "daily_summary_shape",
        /// HS-1 (#402 HS-M §2.1): a touch or capture backend the instance's kind of device does
        /// not have; a fixture has none.
        BackendKindUnsupported => "backend_kind_unsupported",
        /// HS-1: touch `nemu_ipc` beside a capture backend other than `nemu_ipc`.
        BackendPairRequired => "backend_pair_required",
        /// HS-1: a request that names a backend key names another key too, or the backend keys
        /// of two instances.
        BackendSwitchNotAlone => "backend_switch_not_alone",
        /// A note, never a refusal: the channel registry is missing or cannot be read.
        RegistryUnreadable => "registry_unreadable",
        /// A note, never a refusal: a rule's `to` that the channel registry does not list.
        ChannelUnknown => "channel_unknown",
        /// A note, never a refusal: a program a registered channel names is not there.
        ChannelProgramMissing => "channel_program_missing",
    }
}
