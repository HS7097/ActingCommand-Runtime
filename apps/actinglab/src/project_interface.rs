// SPDX-License-Identifier: AGPL-3.0-only

//! Declarative project assembly for ActingLab operation and recognition resources.
//!
//! This module only resolves user-facing options and presets into a runnable
//! configuration contract. It does not read resource repositories, execute
//! operations, open devices, or start scheduler work.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectInterface {
    pub schema_version: String,
    pub project_id: String,
    #[serde(default)]
    pub options: BTreeMap<String, ProjectOption>,
    #[serde(default)]
    pub presets: BTreeMap<String, ProjectPreset>,
    #[serde(default)]
    pub operations: BTreeMap<String, OperationSpec>,
    #[serde(default)]
    pub recognition: BTreeMap<String, RecognitionSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectOption {
    #[serde(default)]
    pub default: Value,
    #[serde(default)]
    pub allowed: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectPreset {
    #[serde(default)]
    pub options: BTreeMap<String, Value>,
    #[serde(default)]
    pub operation: Option<String>,
    #[serde(default)]
    pub recognition: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationSpec {
    pub task_path: String,
    #[serde(default)]
    pub profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecognitionSpec {
    #[serde(default)]
    pub pack_path: Option<String>,
    #[serde(default)]
    pub page_set_path: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectSelection {
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub options: BTreeMap<String, Value>,
    #[serde(default)]
    pub operation: Option<String>,
    #[serde(default)]
    pub recognition: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunnableConfig {
    pub project_id: String,
    pub operation: OperationBinding,
    pub recognition: Vec<RecognitionBinding>,
    pub options: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationBinding {
    pub id: String,
    pub task_path: String,
    #[serde(default)]
    pub profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecognitionBinding {
    pub id: String,
    #[serde(default)]
    pub pack_path: Option<String>,
    #[serde(default)]
    pub page_set_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInterfaceError {
    message: String,
}

impl ProjectInterfaceError {
    pub fn fatal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ProjectInterfaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for ProjectInterfaceError {}

pub type ProjectInterfaceResult<T> = Result<T, ProjectInterfaceError>;

pub fn assemble_runnable_config(
    interface: &ProjectInterface,
    selection: &ProjectSelection,
) -> ProjectInterfaceResult<RunnableConfig> {
    validate_interface(interface)?;
    let preset = selection
        .preset
        .as_deref()
        .map(|preset_id| preset_by_id(interface, preset_id))
        .transpose()?;

    let options = resolve_options(interface, preset, selection)?;
    let operation_id = selection
        .operation
        .as_deref()
        .or_else(|| preset.and_then(|preset| preset.operation.as_deref()))
        .ok_or_else(|| {
            ProjectInterfaceError::fatal("project interface selection must resolve an operation id")
        })?;
    let operation = operation_binding(interface, operation_id)?;
    let recognition_ids = resolve_recognition_ids(interface, preset, selection)?;
    let recognition = recognition_ids
        .iter()
        .map(|id| recognition_binding(interface, id))
        .collect::<ProjectInterfaceResult<Vec<_>>>()?;

    Ok(RunnableConfig {
        project_id: interface.project_id.clone(),
        operation,
        recognition,
        options,
    })
}

fn validate_interface(interface: &ProjectInterface) -> ProjectInterfaceResult<()> {
    if interface.project_id.trim().is_empty() {
        return Err(ProjectInterfaceError::fatal(
            "project interface project_id must not be empty",
        ));
    }
    if interface.operations.is_empty() {
        return Err(ProjectInterfaceError::fatal(
            "project interface must declare at least one operation",
        ));
    }
    for (id, operation) in &interface.operations {
        if id.trim().is_empty() {
            return Err(ProjectInterfaceError::fatal(
                "project interface operation id must not be empty",
            ));
        }
        if operation.task_path.trim().is_empty() {
            return Err(ProjectInterfaceError::fatal(format!(
                "project interface operation '{id}' task_path must not be empty"
            )));
        }
    }
    for (id, recognition) in &interface.recognition {
        if id.trim().is_empty() {
            return Err(ProjectInterfaceError::fatal(
                "project interface recognition id must not be empty",
            ));
        }
        if recognition.pack_path.is_none() && recognition.page_set_path.is_none() {
            return Err(ProjectInterfaceError::fatal(format!(
                "project interface recognition '{id}' must declare pack_path or page_set_path"
            )));
        }
    }
    for (id, option) in &interface.options {
        if id.trim().is_empty() {
            return Err(ProjectInterfaceError::fatal(
                "project interface option id must not be empty",
            ));
        }
        if option.default.is_null() {
            return Err(ProjectInterfaceError::fatal(format!(
                "project interface option '{id}' default must not be null"
            )));
        }
        validate_option_value(interface, id, &option.default)?;
    }
    for (id, preset) in &interface.presets {
        if id.trim().is_empty() {
            return Err(ProjectInterfaceError::fatal(
                "project interface preset id must not be empty",
            ));
        }
        if let Some(operation_id) = &preset.operation
            && !interface.operations.contains_key(operation_id)
        {
            return Err(ProjectInterfaceError::fatal(format!(
                "project interface preset '{id}' references missing operation '{operation_id}'"
            )));
        }
        for recognition_id in &preset.recognition {
            if !interface.recognition.contains_key(recognition_id) {
                return Err(ProjectInterfaceError::fatal(format!(
                    "project interface preset '{id}' references missing recognition '{recognition_id}'"
                )));
            }
        }
        for (option_id, value) in &preset.options {
            validate_option_value(interface, option_id, value)?;
        }
    }
    Ok(())
}

fn preset_by_id<'a>(
    interface: &'a ProjectInterface,
    preset_id: &str,
) -> ProjectInterfaceResult<&'a ProjectPreset> {
    interface.presets.get(preset_id).ok_or_else(|| {
        ProjectInterfaceError::fatal(format!("project interface preset '{preset_id}' is missing"))
    })
}

fn resolve_options(
    interface: &ProjectInterface,
    preset: Option<&ProjectPreset>,
    selection: &ProjectSelection,
) -> ProjectInterfaceResult<BTreeMap<String, Value>> {
    let mut values = interface
        .options
        .iter()
        .map(|(id, option)| (id.clone(), option.default.clone()))
        .collect::<BTreeMap<_, _>>();
    if let Some(preset) = preset {
        apply_options(interface, &mut values, &preset.options)?;
    }
    apply_options(interface, &mut values, &selection.options)?;
    for (id, value) in &values {
        validate_option_value(interface, id, value)?;
    }
    Ok(values)
}

fn apply_options(
    interface: &ProjectInterface,
    values: &mut BTreeMap<String, Value>,
    updates: &BTreeMap<String, Value>,
) -> ProjectInterfaceResult<()> {
    for (id, value) in updates {
        if !interface.options.contains_key(id) {
            return Err(ProjectInterfaceError::fatal(format!(
                "project interface option '{id}' is missing"
            )));
        }
        values.insert(id.clone(), value.clone());
    }
    Ok(())
}

fn validate_option_value(
    interface: &ProjectInterface,
    id: &str,
    value: &Value,
) -> ProjectInterfaceResult<()> {
    let Some(option) = interface.options.get(id) else {
        return Err(ProjectInterfaceError::fatal(format!(
            "project interface option '{id}' is missing"
        )));
    };
    if !option.allowed.is_empty() && !option.allowed.iter().any(|allowed| allowed == value) {
        return Err(ProjectInterfaceError::fatal(format!(
            "project interface option '{id}' does not allow value {value}"
        )));
    }
    Ok(())
}

fn operation_binding(
    interface: &ProjectInterface,
    operation_id: &str,
) -> ProjectInterfaceResult<OperationBinding> {
    let operation = interface.operations.get(operation_id).ok_or_else(|| {
        ProjectInterfaceError::fatal(format!(
            "project interface operation '{operation_id}' is missing"
        ))
    })?;
    Ok(OperationBinding {
        id: operation_id.to_string(),
        task_path: operation.task_path.clone(),
        profile: operation.profile.clone(),
    })
}

fn resolve_recognition_ids(
    interface: &ProjectInterface,
    preset: Option<&ProjectPreset>,
    selection: &ProjectSelection,
) -> ProjectInterfaceResult<Vec<String>> {
    let mut ids = preset
        .map(|preset| preset.recognition.clone())
        .unwrap_or_default();
    ids.extend(selection.recognition.clone());
    if ids.is_empty() {
        ids.extend(interface.recognition.keys().cloned());
    }
    let mut seen = BTreeSet::new();
    ids.retain(|id| seen.insert(id.clone()));
    Ok(ids)
}

fn recognition_binding(
    interface: &ProjectInterface,
    recognition_id: &str,
) -> ProjectInterfaceResult<RecognitionBinding> {
    let recognition = interface.recognition.get(recognition_id).ok_or_else(|| {
        ProjectInterfaceError::fatal(format!(
            "project interface recognition '{recognition_id}' is missing"
        ))
    })?;
    Ok(RecognitionBinding {
        id: recognition_id.to_string(),
        pack_path: recognition.pack_path.clone(),
        page_set_path: recognition.page_set_path.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn project_interface_assembles_runnable_config() {
        let interface = sample_interface();
        let selection = ProjectSelection {
            preset: Some("daily".to_string()),
            options: BTreeMap::from([("server".to_string(), json!("remote"))]),
            operation: None,
            recognition: vec!["pages".to_string()],
        };

        let config =
            assemble_runnable_config(&interface, &selection).expect("config should assemble");

        assert_eq!(config.project_id, "project-alpha");
        assert_eq!(config.operation.id, "daily");
        assert_eq!(config.operation.task_path, "operations/daily/task.json");
        assert_eq!(config.options.get("server"), Some(&json!("remote")));
        assert_eq!(config.recognition.len(), 2);
        assert_eq!(config.recognition[0].id, "home");
        assert_eq!(config.recognition[1].id, "pages");
    }

    #[test]
    fn project_interface_rejects_unknown_option_value() {
        let interface = sample_interface();
        let selection = ProjectSelection {
            options: BTreeMap::from([("server".to_string(), json!("region-z"))]),
            operation: Some("daily".to_string()),
            ..Default::default()
        };

        let error = assemble_runnable_config(&interface, &selection)
            .expect_err("unknown option value must fail");

        assert!(error.to_string().contains("does not allow value"));
    }

    #[test]
    fn project_interface_rejects_missing_operation() {
        let interface = sample_interface();
        let selection = ProjectSelection {
            operation: Some("missing".to_string()),
            ..Default::default()
        };

        let error =
            assemble_runnable_config(&interface, &selection).expect_err("missing operation fails");

        assert!(error.to_string().contains("operation 'missing' is missing"));
    }

    #[test]
    fn project_interface_rejects_misspelled_default_key() {
        let json = r#"{
            "schema_version": "actinglab.project_interface.v0.1",
            "project_id": "project-alpha",
            "options": {
                "server": {
                    "defualt": "local",
                    "allowed": ["local", "remote"]
                }
            },
            "operations": {
                "daily": { "task_path": "operations/daily/task.json" }
            }
        }"#;

        let error = serde_json::from_str::<ProjectInterface>(json)
            .expect_err("misspelled option key must be rejected");

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn project_interface_rejects_null_default() {
        let mut interface = sample_interface();
        interface
            .options
            .get_mut("server")
            .expect("server option")
            .default = Value::Null;

        let error = assemble_runnable_config(
            &interface,
            &ProjectSelection {
                operation: Some("daily".to_string()),
                ..Default::default()
            },
        )
        .expect_err("null default must be rejected");

        assert!(error.to_string().contains("default must not be null"));
    }

    #[test]
    fn project_interface_rejects_bad_preset_operation_reference_at_load_time() {
        let mut interface = sample_interface();
        interface
            .presets
            .get_mut("daily")
            .expect("daily preset")
            .operation = Some("missing".to_string());

        let error = assemble_runnable_config(
            &interface,
            &ProjectSelection {
                operation: Some("daily".to_string()),
                ..Default::default()
            },
        )
        .expect_err("bad preset operation must be rejected during interface validation");

        assert!(
            error
                .to_string()
                .contains("preset 'daily' references missing operation 'missing'")
        );
    }

    #[test]
    fn project_interface_rejects_bad_preset_recognition_reference_at_load_time() {
        let mut interface = sample_interface();
        interface
            .presets
            .get_mut("daily")
            .expect("daily preset")
            .recognition = vec!["missing".to_string()];

        let error = assemble_runnable_config(
            &interface,
            &ProjectSelection {
                operation: Some("daily".to_string()),
                ..Default::default()
            },
        )
        .expect_err("bad preset recognition must be rejected during interface validation");

        assert!(
            error
                .to_string()
                .contains("preset 'daily' references missing recognition 'missing'")
        );
    }

    fn sample_interface() -> ProjectInterface {
        ProjectInterface {
            schema_version: "actinglab.project_interface.v0.1".to_string(),
            project_id: "project-alpha".to_string(),
            options: BTreeMap::from([(
                "server".to_string(),
                ProjectOption {
                    default: json!("local"),
                    allowed: vec![json!("local"), json!("remote")],
                },
            )]),
            presets: BTreeMap::from([(
                "daily".to_string(),
                ProjectPreset {
                    options: BTreeMap::new(),
                    operation: Some("daily".to_string()),
                    recognition: vec!["home".to_string()],
                },
            )]),
            operations: BTreeMap::from([(
                "daily".to_string(),
                OperationSpec {
                    task_path: "operations/daily/task.json".to_string(),
                    profile: Some("profile-primary".to_string()),
                },
            )]),
            recognition: BTreeMap::from([
                (
                    "home".to_string(),
                    RecognitionSpec {
                        pack_path: Some("recognition/home.pack.json".to_string()),
                        page_set_path: None,
                    },
                ),
                (
                    "pages".to_string(),
                    RecognitionSpec {
                        pack_path: None,
                        page_set_path: Some("pages/primary.pages.json".to_string()),
                    },
                ),
            ]),
        }
    }
}
