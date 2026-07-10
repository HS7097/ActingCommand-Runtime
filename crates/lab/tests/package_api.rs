// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_lab::{
    PackageBuildTaskRequest, PackageBuildTaskResponse, PackageEnvOptions, PackageResolution,
    PackageSource, PackageValidateRequest, PackageValidationResponse, ResourceConvertRequest,
    ResourceConvertResponse,
};
use serde::Serialize;
use std::path::PathBuf;

fn assert_serializable<T: Serialize>() {}

#[test]
fn package_family_exposes_typed_requests_and_responses() {
    let _validate = PackageValidateRequest {
        zip_path: PathBuf::from("bundle.zip"),
        include_entries: false,
    };
    let _build = PackageBuildTaskRequest {
        source: PackageSource::Local(PathBuf::from("resources")),
        task_id: "task".to_string(),
        game: Some("arknights".to_string()),
        server: Some("cn".to_string()),
        locale: None,
        package_id: None,
        execution_mode: None,
        resolution: Some(PackageResolution {
            width: 1280,
            height: 720,
        }),
        include_recovery: false,
        out: PathBuf::from("task.zip"),
        dry_run: true,
        env: PackageEnvOptions::default(),
    };
    let _convert = ResourceConvertRequest {
        repo: PathBuf::from("resources"),
        game: Some("arknights".to_string()),
        server: Some("cn".to_string()),
        locale: Some("zh-CN".to_string()),
        maa_tasks_root: None,
        dry_run: true,
    };

    assert_serializable::<PackageValidationResponse>();
    assert_serializable::<PackageBuildTaskResponse>();
    assert_serializable::<ResourceConvertResponse>();
}
