// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #387 D1b, test row D1-M8 (frozen model v1.2 §8.2): the same changes sent through
//! `actingctl config set|clear` and through `ac_config_set` leave the same values in effect, and
//! an MCP session with only the observer tier may send them (§5.6: observer until U-3).

use super::super::settings::changes as mcp_changes;
use super::super::tools::{Tier, TierSet, find};
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, FactScalar, RuntimeConfigManifest,
};
use actingcommand_runtime_config::{
    CheckContext, EffectiveConfig, InstructionStamp, STUCK_RECOVERY, STUCK_RECOVERY_COOLDOWN_SECS,
    instance_key,
};
use serde_json::json;

const INSTANCE: &str = "instance_0123456789abcdef0123456789abcdef";

#[test]
fn d1_m8_cli_and_mcp_changes_leave_the_same_values_in_effect_from_an_observer_session() {
    let enabled = instance_key(INSTANCE, STUCK_RECOVERY);
    let cooldown = instance_key(INSTANCE, STUCK_RECOVERY_COOLDOWN_SECS);
    // The file layer of one instance: both keys settable, at their defaults.
    let file_layer = EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters: vec![
            ConfigParameter {
                key: enabled.clone(),
                value: FactScalar::Boolean(true),
                source: ConfigParameterSource::Default,
                settable: true,
            },
            ConfigParameter {
                key: cooldown.clone(),
                value: FactScalar::Integer(600),
                source: ConfigParameterSource::Default,
                settable: true,
            },
        ],
    })
    .expect("the file layer");
    let stamp = InstructionStamp {
        set_unix_ms: 1,
        set_in_owner_epoch: "owner_epoch_of_the_test".to_owned(),
        request_id: "request_of_the_test".to_owned(),
    };
    let context = CheckContext::new(20);

    // `actingctl config set <key>=<json>…` and `ac_config_set {changes}`.
    let through_cli = crate::config_changes(
        "set",
        &[format!("{cooldown}=120"), format!("{enabled}=false")],
    )
    .expect("the CLI changes");
    let through_mcp = mcp_changes(Some(&json!([
        {"key": cooldown, "value": 120},
        {"key": enabled, "value": false},
    ])))
    .expect("the MCP changes");
    let set_by_cli = file_layer
        .apply(&through_cli, &stamp, &context)
        .expect("the CLI set applies")
        .config;
    let set_by_mcp = file_layer
        .apply(&through_mcp, &stamp, &context)
        .expect("the MCP set applies")
        .config;
    assert_eq!(set_by_cli.manifest(), set_by_mcp.manifest());
    assert_eq!(set_by_cli.value(&cooldown), Some(&FactScalar::Integer(120)));
    assert_eq!(
        set_by_cli.value(&enabled),
        Some(&FactScalar::Boolean(false))
    );

    // `actingctl config clear <key>` and `ac_config_set {changes: [{key}]}`.
    let clear_by_cli =
        crate::config_changes("clear", std::slice::from_ref(&cooldown)).expect("the CLI clear");
    let clear_by_mcp = mcp_changes(Some(&json!([{"key": cooldown}]))).expect("the MCP clear");
    let cleared_by_cli = set_by_cli
        .apply(&clear_by_cli, &stamp, &context)
        .expect("the CLI clear applies")
        .config;
    let cleared_by_mcp = set_by_mcp
        .apply(&clear_by_mcp, &stamp, &context)
        .expect("the MCP clear applies")
        .config;
    assert_eq!(cleared_by_cli.manifest(), cleared_by_mcp.manifest());
    assert_eq!(
        cleared_by_cli.value(&cooldown),
        Some(&FactScalar::Integer(600))
    );

    // The tool is served to a session that enables only the observer tier.
    let tool = find("ac_config_set").expect("ac_config_set is in the tool table");
    assert_eq!(tool.tier, Tier::Observer);
    assert!(TierSet::default().contains(tool.tier));
}
