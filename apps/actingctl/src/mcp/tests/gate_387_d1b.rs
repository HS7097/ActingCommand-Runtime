// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #387 D1b, test row D1-M8 (frozen model v1.2 §8.2): the same changes sent through
//! `actingctl config set|clear` and through `ac_config_set` leave the same values in effect, and
//! an MCP session with only the observer tier may send them (§5.6: observer until U-3).

use super::super::jobs::Jobs;
use super::super::runtime::RuntimeAccess;
use super::super::settings::changes as mcp_changes;
use super::super::tools::{Tier, TierSet, ToolContext, find};
use actingcommand_contract::{
    ApplicationLifecycleAction, ConfigParameter, ConfigParameterSource, EventActor, EventSource,
    FactScalar, FactValue, FencedWrite, RuntimeConfigManifest, config_parameter_section,
    config_parameters_fact_key,
};
use actingcommand_device::{
    CaptureBackend, DeviceError, DeviceResult, FrameMemoryBudget, InputBackend, OpenedBackend,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use actingcommand_runtime_config::{
    CheckContext, ConfigCode, DeviceBounds, EffectiveConfig, InstructionStamp, STUCK_RECOVERY,
    STUCK_RECOVERY_COOLDOWN_SECS, instance_key,
};
use actingcommand_runtime_host::{
    ExecutionBackendProvider, ResolvedExecutionInstance, RuntimeHost, RuntimeHostConfig,
};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tempfile::TempDir;

const INSTANCE: &str = "instance_0123456789abcdef0123456789abcdef";
const SALT: &[u8] = b"gate-387-d1b-test-salt";

// A Runtime without instances: ConfigureSettings needs no device.
struct NoInstances;

impl ExecutionBackendProvider for NoInstances {
    fn instance_aliases(&self) -> Vec<String> {
        Vec::new()
    }

    fn resolve(&self, _instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        None
    }

    fn open_input(
        &self,
        _instance_alias: &str,
    ) -> DeviceResult<OpenedBackend<Box<dyn InputBackend>>> {
        Err(DeviceError::fatal("the test Runtime has no instance"))
    }

    fn open_capture(
        &self,
        _instance_alias: &str,
        _memory: Option<&FrameMemoryBudget>,
    ) -> DeviceResult<OpenedBackend<Box<dyn CaptureBackend>>> {
        Err(DeviceError::fatal("the test Runtime has no instance"))
    }

    fn control_application(
        &self,
        _witness: &FencedWrite,
        _instance_alias: &str,
        _action: ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        Err(DeviceError::fatal("the test Runtime has no instance"))
    }
}

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
    // The per-instance section's check reads neither the heartbeat bound nor the device
    // layer's bounds (Workflow #387 D1c added the latter to the check context).
    let context = CheckContext::new(
        20,
        DeviceBounds {
            application_stop_ms: 0,
            emulator_stop_ms: 0,
            emulator_start_ms: 0,
        },
    );

    // `actingctl config set <key>=<json>…` and `ac_config_set {changes}`.
    let through_cli = crate::config_changes(
        "set",
        &[format!("{cooldown}=120"), format!("{enabled}=false")],
    )
    .expect("the CLI changes");
    let Ok(through_mcp) = mcp_changes(Some(&json!([
        {"key": cooldown, "value": 120},
        {"key": enabled, "value": false},
    ]))) else {
        panic!("the MCP changes are refused");
    };
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
    let Ok(clear_by_mcp) = mcp_changes(Some(&json!([{"key": cooldown}]))) else {
        panic!("the MCP clear is refused");
    };
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

// Workflow #378 RK-1 (A5, coordinator ruling #743): an `ac_config_set` refused by the Runtime
// answers a tool error whose top is the denied receipt's outcome (`config_key_unknown` for an
// unknown key) with the receipt's `receipt_state` and `request_id`, and the catalog this build
// embeds declares every key of that top for its code. The whole request is refused and nothing
// changes (`config_key_unknown`, model v1.2 §5.6: all changes or none), so the valid change sent
// with the unknown key is not in effect afterwards.
#[test]
fn rk1_an_unknown_key_refuses_ac_config_set_whole_with_a_declared_receipt_top() {
    let root = TempDir::new().expect("tempdir");
    let cooldown = instance_key(INSTANCE, STUCK_RECOVERY_COOLDOWN_SECS);
    // A misspelt member: no row of the key table has it.
    let misspelt = instance_key(INSTANCE, "stuck_recovery_cooldown_seconds");
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(root.path(), SALT).with_config_manifest(RuntimeConfigManifest {
            subsystems: Vec::new(),
            parameters: vec![ConfigParameter {
                key: cooldown.clone(),
                value: FactScalar::Integer(600),
                source: ConfigParameterSource::Default,
                settable: true,
            }],
        }),
        Arc::new(NoInstances),
    )
    .expect("the Runtime starts");

    let runtime = RuntimeAccess::new(None, Some(root.path().to_path_buf()));
    let cancelled = AtomicBool::new(false);
    let jobs = Jobs::new();
    let context = ToolContext {
        runtime: &runtime,
        cancelled: &cancelled,
        deadline: super::gate_mcpt1::unreached_deadline(),
        session: 1,
        jobs: &jobs,
    };
    let tool = find("ac_config_set").expect("ac_config_set is in the tool table");
    let Value::Object(arguments) = json!({"changes": [
        {"key": cooldown, "value": 120},
        {"key": misspelt, "value": 120},
    ]}) else {
        panic!("the arguments are an object");
    };
    let Err(error) = (tool.run)(&context, &arguments) else {
        panic!("ac_config_set with an unknown key was accepted");
    };
    let top = error.outcome_ref();
    assert!(
        top.is(ConfigCode::ConfigKeyUnknown),
        "the refusal's top: {top:?}"
    );
    assert_eq!(
        top.value("receipt_state"),
        Some(&json!("denied")),
        "the refusal's top: {top:?}"
    );
    assert!(
        top.value("request_id")
            .and_then(Value::as_str)
            .is_some_and(|request_id| !request_id.is_empty()),
        "the refusal's top: {top:?}"
    );
    let catalog =
        actingcommand_contract::outcome::catalog_json().expect("the embedded catalog parses");
    let declared = &catalog["codes"][top.code().as_str()]["values"];
    let undeclared = top
        .values()
        .keys()
        .filter(|key| declared.get(key.as_str()).is_none())
        .collect::<Vec<_>>();
    assert!(
        undeclared.is_empty(),
        "keys on the top of {} that the catalog does not declare for the code: {undeclared:?}",
        top.code().as_str()
    );

    // Nothing changed: the configuration in effect still has the file layer's value.
    let reader = RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect a reader");
    let snapshot = reader
        .runtime_fact_snapshot()
        .expect("the runtime fact snapshot");
    drop(reader);
    let section = config_parameters_fact_key(config_parameter_section(&cooldown));
    let Some(FactValue::RecordList(rows)) = snapshot
        .records
        .iter()
        .find(|record| record.key == section)
        .map(|record| &record.value)
    else {
        panic!("no record list {section}");
    };
    let row = rows
        .iter()
        .find(|row| row.get("key") == Some(&FactScalar::String(cooldown.clone())))
        .unwrap_or_else(|| panic!("no row {cooldown}: {rows:?}"));
    assert_eq!(row.get("value"), Some(&FactScalar::Integer(600)));
    assert_eq!(
        row.get("source"),
        Some(&FactScalar::String("default".to_owned()))
    );
    host.close().expect("close the Runtime");
}
