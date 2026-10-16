// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #398 RD-4 (model-398 v1.1 §6.2, §6.3, §6.6, option B; §12 rows RD-4-T3 and the
// configuration half of RD-4-T7): the ladder profiles and the rule `rules.start_beyond_profile`
// in the `recovery_ladder` section. Every expected value comes from the model: the keys, their
// checks and the order of use (§6.2), the defaults file's profile (§6.3) and the rule with its
// three values and default `exhaust` (§6.6, coordinator ruling of 2026-10-15).

use super::*;
use crate::recovery_ladder::{KIND_PROFILES, PROFILES, RULE_START_BEYOND_PROFILE};
use actingcommand_contract::{
    ConfigParameterSource, ConfigSettingChange, FactScalar, InstanceDeviceKind, RecoveryRung,
    RuntimeConfigManifest,
};
use serde_json::{Value, json};

// The check's inputs, chosen for the rows below (as `ladder_tests`).
const HEARTBEAT_MS: u64 = 5_000;
const DEVICE: DeviceBounds = DeviceBounds {
    application_stop_ms: 60_000,
    emulator_stop_ms: 200_000,
    emulator_start_ms: 190_000,
};
const INSTANCE: &str = "instance_0123456789abcdef0123456789abcd96";

fn context() -> CheckContext {
    CheckContext::new(HEARTBEAT_MS, DEVICE)
}

// The configuration a start reads from a file that names `section` as its `recovery_ladder` and
// `profile` as the instance's `recovery_ladder_profile`.
fn from_file(section: Value, profile: Option<&str>) -> EffectiveConfig {
    let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file parses");
    let mut entry = json!({"instance_id": INSTANCE});
    if let Some(profile) = profile {
        entry[RECOVERY_LADDER_PROFILE] = json!(profile);
    }
    let parameters = render_parameters(
        &mechanism_rows(),
        &[INSTANCE.to_owned()],
        &defaults,
        &json!({"recovery_ladder": section, "instances": [entry]}),
        &|_| None,
    )
    .expect("every key resolves");
    EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    })
    .expect("every key is in the key table")
}

fn stamp() -> InstructionStamp {
    InstructionStamp {
        set_unix_ms: 1,
        set_in_owner_epoch: "epoch".to_owned(),
        request_id: "request".to_owned(),
        channel: None,
    }
}

fn set(key: &str, value: Value) -> ConfigSettingChange {
    ConfigSettingChange {
        key: key.to_owned(),
        value: Some(value),
    }
}

fn actions(rungs: &[RungConfig]) -> Vec<RecoveryRung> {
    rungs.iter().map(RungConfig::action).collect()
}

// One RD-4-T3 row: the file, the instruction that sets the same broken value on a running
// configuration (started from `running`), the key the refusal names and why.
struct Row {
    case: &'static str,
    section: Value,
    profile: Option<&'static str>,
    running: Option<&'static str>,
    changes: Vec<ConfigSettingChange>,
    config_key: String,
    problem: ConfigValueProblem,
}

fn rd4_t3_rows() -> Vec<Row> {
    let instance_profile = instance_key(INSTANCE, RECOVERY_LADDER_PROFILE);
    let two = json!({"rungs": [{"action": "return_home"}, {"action": "application_restart"}]});
    let low_probe = json!({"handset": {"rungs": [
        {"action": "application_restart", "adb_probe_ms": 999},
    ]}});
    let four_rungs = json!({"handset": {"rungs": [
        {"action": "return_home"},
        {"action": "application_restart"},
        {"action": "emulator_restart"},
        {"action": "return_home"},
    ]}});
    vec![
        Row {
            case: "the kind table names a profile the section does not have",
            section: json!({"kind_profiles": {"handset": "absent_profile"}}),
            profile: None,
            running: None,
            changes: vec![set(KIND_PROFILES, json!({"handset": "absent_profile"}))],
            config_key: KIND_PROFILES.to_owned(),
            problem: ConfigValueProblem::OutOfRange,
        },
        Row {
            case: "an instance names a profile the section does not have",
            section: json!({}),
            profile: Some("absent_profile"),
            running: Some("handset"),
            changes: vec![set(&instance_profile, json!("absent_profile"))],
            config_key: instance_profile,
            problem: ConfigValueProblem::OutOfRange,
        },
        Row {
            case: "a profile whose rung bound is below the schema's",
            section: json!({"profiles": low_probe}),
            profile: None,
            running: None,
            changes: vec![set(PROFILES, low_probe)],
            config_key: format!("{PROFILES}.handset"),
            problem: ConfigValueProblem::BoundOutOfRange,
        },
        Row {
            case: "a profile of more rungs than actions",
            section: json!({"profiles": four_rungs}),
            profile: None,
            running: None,
            changes: vec![set(PROFILES, four_rungs)],
            config_key: format!("{PROFILES}.handset"),
            problem: ConfigValueProblem::RungsInvalid,
        },
        Row {
            case: "removing the profile the kind table names",
            section: json!({"profiles": {"other_profile": two}}),
            profile: None,
            running: None,
            changes: vec![set(PROFILES, json!({"other_profile": two}))],
            config_key: KIND_PROFILES.to_owned(),
            problem: ConfigValueProblem::OutOfRange,
        },
    ]
}

// RD-4-T3 (§6.2): a profile name no profile has (in the kind table, in an instance's key, or a
// profile the kind table still names removed) and a profile's rungs outside their schema refuse
// the whole section with `recovery_ladder_config_invalid` naming the key (the profile's own for
// its rungs) and why; the same values sent by an instruction are `config_value_invalid` and the
// configuration in effect stays.
#[test]
fn rd4_t3_a_profile_no_one_has_or_profile_rungs_out_of_bounds_refuse_the_section() {
    for row in rd4_t3_rows() {
        let refused = from_file(row.section.clone(), row.profile)
            .check(&context())
            .expect_err(row.case);
        assert_eq!(
            refused.failure_code(),
            ConfigCode::RecoveryLadderConfigInvalid,
            "{}",
            row.case
        );
        assert_eq!(refused.config_key(), row.config_key, "{}", row.case);
        assert_eq!(refused.problem(), Some(row.problem), "{}", row.case);

        let running = from_file(json!({}), row.running);
        let before = running
            .recovery_ladder(&context())
            .expect("the defaults pass");
        let refused = running
            .apply(&row.changes, &stamp(), &context())
            .expect_err(row.case);
        assert_eq!(
            refused.failure_code(),
            ConfigCode::ConfigValueInvalid,
            "{}",
            row.case
        );
        assert_eq!(refused.config_key(), row.config_key, "{}", row.case);
        assert_eq!(refused.problem(), Some(row.problem), "{}", row.case);
        assert_eq!(
            running.recovery_ladder(&context()),
            Ok(before),
            "{}",
            row.case
        );
    }
}

// RD-4-T7, the configuration half (§6.3, §6.6): the defaults give the handset kind the profile
// `handset` (return home, then restart the application) and the rule `exhaust`, and an emulator
// none; the rule is a settable key of `config.parameters` and part of the digest. Under `exhaust`
// and `last_rung` a start rung is checked against the section's rungs only; under `refuse` every
// start rung must fit every profile, so the defaults' `navigation_exhausted` (rung 3) against the
// two-rung handset profile is `start_rung_out_of_range` naming that profile, at start and as
// `config_value_invalid` through an instruction, and the configuration in effect stays.
#[test]
fn rd4_t7_the_rule_is_configured_and_refuse_checks_every_start_rung_against_every_profile() {
    let defaults = from_file(json!({}), None)
        .recovery_ladder(&context())
        .expect("the defaults pass");
    assert_eq!(defaults.start_beyond_profile, StartBeyondProfile::Exhaust);
    let handset = defaults
        .profile_for(INSTANCE, InstanceDeviceKind::Handset)
        .expect("the handset kind has a profile");
    assert_eq!(handset.name, "handset");
    assert_eq!(
        actions(&handset.rungs),
        vec![RecoveryRung::ReturnHome, RecoveryRung::ApplicationRestart]
    );
    assert_eq!(
        defaults.profile_for(INSTANCE, InstanceDeviceKind::Emulator),
        None
    );
    // The instance's own key comes before the kind table.
    let own = from_file(
        json!({"profiles": {
            "handset": {"rungs": [{"action": "return_home"}, {"action": "application_restart"}]},
            "short_profile": {"rungs": [{"action": "application_restart"}]},
        }}),
        Some("short_profile"),
    )
    .recovery_ladder(&context())
    .expect("the instance names a profile the section has");
    for kind in [InstanceDeviceKind::Handset, InstanceDeviceKind::Emulator] {
        assert_eq!(
            own.profile_for(INSTANCE, kind)
                .map(|profile| profile.name.as_str()),
            Some("short_profile"),
            "{kind:?}"
        );
    }

    let parameters = from_file(json!({}), None).manifest().parameters;
    let rule = parameters
        .iter()
        .find(|parameter| parameter.key == RULE_START_BEYOND_PROFILE)
        .expect("the rule is a row of config.parameters");
    assert_eq!(rule.value, FactScalar::String("exhaust".to_owned()));
    assert_eq!(rule.source, ConfigParameterSource::Default);
    assert!(rule.settable);

    let mut digests = vec![defaults.digest().to_owned()];
    for value in ["last_rung", "exhaust"] {
        let ladder = from_file(json!({"rules": {"start_beyond_profile": value}}), None)
            .recovery_ladder(&context())
            .unwrap_or_else(|failure| panic!("{value} passes: {failure:?}"));
        digests.push(ladder.digest().to_owned());
    }
    assert_ne!(digests[0], digests[1], "the rule is part of the digest");
    assert_eq!(digests[0], digests[2]);

    let refused = from_file(json!({"rules": {"start_beyond_profile": "refuse"}}), None)
        .check(&context())
        .expect_err("refuse with a start rung beyond a profile");
    assert_eq!(
        refused.failure_code(),
        ConfigCode::RecoveryLadderConfigInvalid
    );
    assert_eq!(refused.config_key(), format!("{PROFILES}.handset"));
    assert_eq!(
        refused.problem(),
        Some(ConfigValueProblem::StartRungOutOfRange)
    );
    let running = from_file(json!({}), None);
    let refused = running
        .apply(
            &[set(RULE_START_BEYOND_PROFILE, json!("refuse"))],
            &stamp(),
            &context(),
        )
        .expect_err("refuse by instruction");
    assert_eq!(refused.failure_code(), ConfigCode::ConfigValueInvalid);
    assert_eq!(refused.config_key(), format!("{PROFILES}.handset"));
    assert_eq!(
        refused.problem(),
        Some(ConfigValueProblem::StartRungOutOfRange)
    );
    assert_eq!(running.recovery_ladder(&context()), Ok(defaults));

    // Under `refuse`, start rungs that fit every profile pass.
    let fitting = from_file(
        json!({
            "rules": {"start_beyond_profile": "refuse"},
            "triggers": [{
                "id": "pack_logic",
                "stages": ["task"],
                "classes": ["pack_logic_mismatch"],
                "start_rung": 2,
            }],
        }),
        None,
    )
    .recovery_ladder(&context())
    .expect("every start rung fits every profile");
    assert_eq!(fitting.start_beyond_profile, StartBeyondProfile::Refuse);
}

// RD-4 deviation 6, accepted (train-19 notes, note-864, 10-16 01:1x; model-398 §6.2: each member
// of `kind_profiles` is a device kind naming one of `profiles`): a member whose key is no device
// kind (the kinds are `emulator` and `handset`) is refused like a name no profile has, at start
// with `recovery_ladder_config_invalid` and by an instruction with `config_value_invalid`, both
// `out_of_range` naming `recovery_ladder.kind_profiles`; the configuration in effect stays.
#[test]
fn gate_rd4_an_unknown_device_kind_in_kind_profiles_is_refused_out_of_range_naming_the_key() {
    let unknown_kind = json!({"tablet": "handset"});
    let refused = from_file(json!({"kind_profiles": unknown_kind}), None)
        .check(&context())
        .expect_err("a kind outside the device kinds");
    assert_eq!(
        refused.failure_code(),
        ConfigCode::RecoveryLadderConfigInvalid
    );
    assert_eq!(refused.config_key(), KIND_PROFILES);
    assert_eq!(refused.problem(), Some(ConfigValueProblem::OutOfRange));

    let running = from_file(json!({}), None);
    let before = running
        .recovery_ladder(&context())
        .expect("the defaults pass");
    let refused = running
        .apply(&[set(KIND_PROFILES, unknown_kind)], &stamp(), &context())
        .expect_err("a kind outside the device kinds, by instruction");
    assert_eq!(refused.failure_code(), ConfigCode::ConfigValueInvalid);
    assert_eq!(refused.config_key(), KIND_PROFILES);
    assert_eq!(refused.problem(), Some(ConfigValueProblem::OutOfRange));
    assert_eq!(running.recovery_ladder(&context()), Ok(before));
}
