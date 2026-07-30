// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    CliError, CliOutcome, GlobalOptions, effective_adb_path_for_instance,
    enforce_path_adb_target_boundary, resolve_instance_id, runtime_capture_backend,
    runtime_state_root,
};
use actingcommand_device::{AdbPathSource, CaptureBackendChoice};
#[cfg(test)]
use actingcommand_device::{DeviceTarget, TouchBackendChoice};
use actingcommand_lab::{InstanceConfig, UserConfig};
use std::path::PathBuf;

pub(super) fn device_config(global: &GlobalOptions, config: &UserConfig) -> CliOutcome<DeviceRuntimeConfig> {
    device_config_for_instance(global, config, None)
}

fn device_config_for_instance(
    global: &GlobalOptions,
    config: &UserConfig,
    instance_override: Option<&str>,
) -> CliOutcome<DeviceRuntimeConfig> {
    let instance_id = match instance_override {
        Some(instance) => instance.to_string(),
        None => resolve_instance_id(global, config)?,
    };
    let instance = config.instances.get(&instance_id);
    #[cfg(test)]
    let mut target = DeviceTarget::default();
    #[cfg(test)]
    if let Some(serial) = instance.and_then(|instance| instance.serial.clone()) {
        target.serial = Some(serial);
    } else if global.instance.as_deref() == Some(instance_id.as_str()) && instance.is_none() {
        target.serial = Some(instance_id.clone());
    }
    let capture_backend = effective_capture_backend_choice(global, &instance_id, instance)?;
    #[cfg(test)]
    let touch_backend = effective_touch_backend_choice(global, &instance_id, instance)?;
    let resolved_adb = effective_adb_path_for_instance(config, instance)?;
    enforce_path_adb_target_boundary(&resolved_adb, instance, capture_backend)?;
    Ok(DeviceRuntimeConfig {
        instance_alias: instance_id,
        runtime_state_root: runtime_state_root()?,
        #[cfg(test)]
        target,
        adb_source: resolved_adb.source,
        adb_warning: resolved_adb.warning,
        capture_backend,
        #[cfg(test)]
        touch_backend,
    })
}

#[derive(Debug)]
pub(super) struct DeviceRuntimeConfig {
    instance_alias: String,
    runtime_state_root: PathBuf,
    #[cfg(test)]
    pub(super) target: DeviceTarget,
    pub(super) adb_source: AdbPathSource,
    pub(super) adb_warning: Option<String>,
    pub(super) capture_backend: CaptureBackendChoice,
    #[cfg(test)]
    pub(super) touch_backend: TouchBackendChoice,
}

impl DeviceRuntimeConfig {
    pub(super) fn runtime_capture_endpoint(&self) -> runtime_capture_backend::RuntimeCaptureEndpoint {
        runtime_capture_backend::RuntimeCaptureEndpoint::new(
            self.instance_alias.clone(),
            self.runtime_state_root.clone(),
        )
    }
}

pub(super) fn effective_capture_backend_choice(
    global: &GlobalOptions,
    instance_id: &str,
    instance: Option<&InstanceConfig>,
) -> CliOutcome<CaptureBackendChoice> {
    if let Some(choice) = global.capture_backend {
        return Ok(choice);
    }
    let Some(value) = instance.and_then(|instance| instance.capture_backend.as_deref()) else {
        return Ok(CaptureBackendChoice::Auto);
    };
    CaptureBackendChoice::parse(value).map_err(|err| {
        CliError::usage(format!(
            "invalid instance.{instance_id}.capture_backend '{value}': {err}"
        ))
    })
}

#[cfg(test)]
fn effective_touch_backend_choice(
    global: &GlobalOptions,
    instance_id: &str,
    instance: Option<&InstanceConfig>,
) -> CliOutcome<TouchBackendChoice> {
    if let Some(choice) = global.touch_backend {
        return Ok(choice);
    }
    let Some(value) = instance.and_then(|instance| instance.touch_backend.as_deref()) else {
        return Ok(TouchBackendChoice::Auto);
    };
    TouchBackendChoice::parse(value).map_err(|err| {
        CliError::usage(format!(
            "invalid instance.{instance_id}.touch_backend '{value}': {err}"
        ))
    })
}
