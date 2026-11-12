// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #437 MAPP-1 (register §1.323, §1.328; `contracts/device-group.md`, "The configuration
// form"): an entry's `device_group` is the row `instance.<instance id>.device_group`, reported only
// for an entry that names it and not settable. While the Runtime runs, the whole-instance check
// refuses an instruction that would turn a device-level action on for a member of a group of
// several (`config_value_invalid`, `section_check`, on the key whose value turns it on), and lets
// the same instruction through on an instance that is no member of one. The load refuses every
// group of several until the slice "order within one device", so these tests build the
// configuration in effect directly (register §1.323, "The code slices"). The ids are made up.

use super::*;
use actingcommand_contract::{ConfigSettingChange, FactScalar, RuntimeConfigManifest};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Two members of the group `device.one`.
const FIRST: &str = "instance_0123456789abcdef0123456789abcdef";
const SECOND: &str = "instance_1123456789abcdef0123456789abcdef";
/// An instance that names no group.
const LONE: &str = "instance_fedcba9876543210fedcba9876543210";
/// The defaults file's profile without `emulator_restart` (MAPP-1).
const PROFILE: &str = "without_emulator_restart";
const DEVICE: DeviceBounds = DeviceBounds {
    application_stop_ms: 60_000,
    emulator_stop_ms: 200_000,
    emulator_start_ms: 190_000,
};

/// The context of a running host: every instance bound by discovery, so `start_emulator` `true`
/// passes its own rule (Workflow #429 R-3) and only the group's rule can refuse it.
fn context() -> CheckContext {
    CheckContext::new(5_000, DEVICE).with_discovery_bound(
        [FIRST, SECOND, LONE]
            .iter()
            .map(|instance| (*instance).to_owned())
            .collect(),
    )
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

/// The configuration in effect of two members of one group, each with `idle.action` `none` and
/// the profile without `emulator_restart`, beside one instance that names no group and keeps the
/// defaults.
fn configuration() -> EffectiveConfig {
    let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file parses");
    let member = |instance_id: &str, alias: &str| {
        json!({
            "instance_id": instance_id,
            "alias": alias,
            "device_group": "device.one",
            "idle": {"action": "none"},
            "recovery_ladder_profile": PROFILE,
        })
    };
    let parameters = render_parameters(
        &mechanism_rows(),
        &[FIRST.to_owned(), SECOND.to_owned(), LONE.to_owned()],
        &defaults,
        &json!({"instances": [
            member(FIRST, "neutral.a"),
            member(SECOND, "neutral.b"),
            {"instance_id": LONE, "alias": "neutral.c"},
        ]}),
        &|_| None,
    )
    .expect("every key resolves");
    EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    })
    .expect("every key is in the key table")
}

#[test]
fn mapp1_device_group_row_is_reported_only_for_an_entry_that_names_it_and_is_not_settable() {
    let config = configuration();
    let first = instance_key(FIRST, DEVICE_GROUP);
    assert_eq!(
        config.value(&first),
        Some(&FactScalar::String("device.one".to_owned()))
    );
    assert_eq!(
        config.value(&instance_key(LONE, DEVICE_GROUP)),
        None,
        "an entry without the member has no row"
    );
    let row = config
        .manifest()
        .parameters
        .into_iter()
        .find(|parameter| parameter.key == first)
        .expect("the member's row");
    assert!(!row.settable);
    let refused = config
        .apply(&[set(&first, json!("device.two"))], &stamp(), &context())
        .expect_err("the row is read at assembly");
    assert_eq!(refused.failure_code(), ConfigCode::ConfigKeyNotSettable);
    assert_eq!(refused.config_key(), first);
    // The two members are one group of several; the lone instance is none.
    assert_eq!(
        config.device_groups_of_several(),
        BTreeMap::from([(
            "device.one".to_owned(),
            vec![FIRST.to_owned(), SECOND.to_owned()]
        )])
    );
    config
        .check(&context())
        .expect("a group of several that turns no device-level action on passes the check");
}

#[test]
fn mapp1_an_instruction_that_turns_a_device_level_action_on_for_a_member_is_refused_section_check()
{
    let config = configuration();
    let first_profile = instance_key(FIRST, RECOVERY_LADDER_PROFILE);
    let with_restart = json!({
        "handset": {"rungs": [{"action": "return_home"}, {"action": "application_restart"}]},
        "without_emulator_restart": {"rungs": [
            {"action": "return_home"},
            {"action": "application_restart"},
            {"action": "emulator_restart"},
        ]},
    });
    // Each instruction, and the key the refusal names: the member's own key, or the key of the
    // section whose value the member takes.
    for (change, refused_key) in [
        (
            set(&instance_key(FIRST, START_EMULATOR), json!(true)),
            instance_key(FIRST, START_EMULATOR),
        ),
        (
            set(&instance_key(SECOND, idle::ACTION), json!("close_app")),
            instance_key(SECOND, idle::ACTION),
        ),
        // The profile the member names now holds `emulator_restart`: the member's key is named.
        (
            set(recovery_ladder::PROFILES, with_restart),
            first_profile.clone(),
        ),
    ] {
        let key = change.key.clone();
        let refused = config
            .apply(std::slice::from_ref(&change), &stamp(), &context())
            .map(|_| ())
            .expect_err("the member would turn a device-level action on");
        assert_eq!(
            refused.failure_code(),
            ConfigCode::ConfigValueInvalid,
            "{key}"
        );
        assert_eq!(
            refused.problem(),
            Some(ConfigValueProblem::SectionCheck),
            "{key}"
        );
        assert_eq!(refused.config_key(), refused_key, "{key}");
    }
    // The configuration in effect is unchanged: nothing was set.
    assert!(config.override_rows().is_empty());
    // The same instructions on the instance that is no member of a group pass.
    for change in [
        set(&instance_key(LONE, START_EMULATOR), json!(true)),
        set(&instance_key(LONE, idle::ACTION), json!("close_app")),
        set(
            &instance_key(LONE, RECOVERY_LADDER_PROFILE),
            json!("handset"),
        ),
    ] {
        let key = change.key.clone();
        config
            .apply(&[change], &stamp(), &context())
            .unwrap_or_else(|failure| panic!("{key} passes on a lone instance: {failure:?}"));
    }
}

#[test]
fn mapp1_the_device_level_actions_in_effect_name_the_key_that_turns_each_on() {
    let config = configuration();
    for member in [FIRST, SECOND] {
        assert_eq!(config.device_action_in_effect(member, &context()), Ok(None));
    }
    // The lone instance takes the defaults: no `start_emulator`, the section's `idle.action`
    // (`close_app`), which is the first action it turns on.
    assert_eq!(
        config.device_action_in_effect(LONE, &context()),
        Ok(Some((DeviceAction::IdleAction, idle::ACTION.to_owned())))
    );
    // With the instance's own `none`, the section's rungs, which hold `emulator_restart`.
    let applied = config
        .apply(
            &[set(&instance_key(LONE, idle::ACTION), json!("none"))],
            &stamp(),
            &context(),
        )
        .expect("a lone instance names its idle action");
    assert_eq!(
        applied.config.device_action_in_effect(LONE, &context()),
        Ok(Some((
            DeviceAction::EmulatorRestartRung,
            recovery_ladder::RUNGS.to_owned()
        )))
    );
    assert_eq!(DeviceAction::StartEmulator.member(), START_EMULATOR);
    assert_eq!(DeviceAction::IdleAction.member(), idle::ACTION);
    assert_eq!(
        DeviceAction::EmulatorRestartRung.member(),
        RECOVERY_LADDER_PROFILE
    );
}
