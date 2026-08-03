use super::{CliError, CliOutcome};
use actingcommand_device::{CaptureBackendChoice, TouchBackendChoice};
use actingcommand_lab::UserConfig;
use serde_json::{Value, json};

pub(super) fn config_get(config: &UserConfig, key: &str) -> CliOutcome<Value> {
    match key {
        "adb_path" => Ok(json!(config.adb_path)),
        "runtime_endpoint" => Ok(json!(config.runtime_endpoint)),
        "run_root" => Ok(json!(config.run_root)),
        "resource_root" => Ok(json!(config.resource_root)),
        key if key.starts_with("instance.") => get_instance_value(config, key),
        _ => Err(CliError::usage(format!("unknown config key: {key}"))),
    }
}

pub(super) fn config_set(config: &mut UserConfig, key: &str, value: &str) -> CliOutcome<()> {
    match key {
        "adb_path" => config.adb_path = Some(value.to_string()),
        "runtime_endpoint" => config.runtime_endpoint = Some(value.to_string()),
        "run_root" => config.run_root = Some(value.to_string()),
        "resource_root" => config.resource_root = Some(value.to_string()),
        key if key.starts_with("instance.") => set_instance_value(config, key, value)?,
        _ => return Err(CliError::usage(format!("unknown config key: {key}"))),
    }
    Ok(())
}

fn get_instance_value(config: &UserConfig, key: &str) -> CliOutcome<Value> {
    let parts = key.split('.').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err(CliError::usage(
            "instance config keys use instance.<id>.serial|game|server|package|adb_path|capture_backend|touch_backend",
        ));
    }
    let instance = config.instances.get(parts[1]);
    let value = match parts[2] {
        "serial" => instance.and_then(|instance| instance.serial.clone()),
        "game" => instance.and_then(|instance| instance.game.clone()),
        "server" => instance.and_then(|instance| instance.server.clone()),
        "package" => instance.and_then(|instance| instance.package.clone()),
        "adb_path" => instance.and_then(|instance| instance.adb_path.clone()),
        "capture_backend" => instance.and_then(|instance| instance.capture_backend.clone()),
        "touch_backend" => instance.and_then(|instance| instance.touch_backend.clone()),
        other => return Err(CliError::usage(format!("unknown instance field: {other}"))),
    };
    Ok(json!(value))
}

fn set_instance_value(config: &mut UserConfig, key: &str, value: &str) -> CliOutcome<()> {
    let parts = key.split('.').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err(CliError::usage(
            "instance config keys use instance.<id>.serial|game|server|package|adb_path|capture_backend|touch_backend",
        ));
    }
    let instance = config.instances.entry(parts[1].to_string()).or_default();
    match parts[2] {
        "serial" => instance.serial = Some(value.to_string()),
        "game" => instance.game = Some(value.to_string()),
        "server" => instance.server = Some(value.to_string()),
        "package" => instance.package = Some(value.to_string()),
        "adb_path" => instance.adb_path = Some(value.to_string()),
        "capture_backend" => {
            CaptureBackendChoice::parse(value).map_err(|err| CliError::usage(err.to_string()))?;
            instance.capture_backend = Some(value.to_string());
        }
        "touch_backend" => {
            TouchBackendChoice::parse(value).map_err(|err| CliError::usage(err.to_string()))?;
            instance.touch_backend = Some(value.to_string());
        }
        other => return Err(CliError::usage(format!("unknown instance field: {other}"))),
    }
    Ok(())
}
