// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome, FlagArgs, GlobalOptions};
use actingcommand_lab::{
    LabPackageValidationResponse, PackageBuildCatalog, PackageBuildCatalogRequest,
    PackageBuildTaskRequest, PackageEnvOptions, PackageFullArchiveRequest, PackageResolution,
    PackageSource, PackageTaskArchiveRequest,
};
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn run_build_task(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let game = flags.optional("--game").or_else(|| global.game.clone());
    let server = flags.optional("--server").or_else(|| global.server.clone());
    let request = PackageBuildTaskRequest {
        source: package_source(flags)?,
        temporary_root: std::env::temp_dir(),
        task_id: flags.required("--task")?,
        game: game.clone(),
        server: server.clone(),
        locale: flags.optional("--locale"),
        package_id: flags.optional("--package-id"),
        execution_mode: flags.optional("--execution-mode"),
        resolution: parse_resolution(flags)?,
        include_recovery: flags.bool("--include-recovery"),
        out: flags.required_path("--out")?,
        dry_run: global.dry_run || flags.bool("--dry-run"),
        env: package_env(global, flags, game, server),
    };
    let mut lab = super::env_detection::build_readonly_lab()?;
    serialize_response(lab.package_build_task(request)?)
}

pub(super) fn run_build_pack(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let game = flags.optional("--game").or_else(|| global.game.clone());
    let server = flags.optional("--server").or_else(|| global.server.clone());
    let dry_run = global.dry_run || flags.bool("--dry-run");
    let env = package_env(global, flags, game.clone(), server.clone());
    let catalog = PackageBuildCatalog::open(PackageBuildCatalogRequest {
        source: package_source(flags)?,
        temporary_root: std::env::temp_dir(),
        game,
        server,
        locale: flags.optional("--locale"),
    })?;
    let metadata = catalog.metadata();
    let resolution = parse_resolution(flags)?;
    let mut lab = super::env_detection::build_readonly_lab()?;

    if let Some(split_dir) = flags.optional_path("--split-dir") {
        let mut packages = Vec::new();
        let temp_split_dir = if dry_run {
            None
        } else {
            let temp = temp_dir_path(&split_dir);
            fs::create_dir_all(&temp).map_err(|error| {
                CliError::package_invalid(format!("failed to create {}: {error}", temp.display()))
            })?;
            Some(temp)
        };
        let build_dir = temp_split_dir.as_deref().unwrap_or(&split_dir);
        for task_id in catalog.task_ids() {
            let package_id = format!("{}.{}.{}", metadata.game, metadata.server, task_id);
            let final_out = split_dir.join(format!("{package_id}.zip"));
            let build_out = build_dir.join(format!("{package_id}.zip"));
            let validation = match catalog.build_task_archive(
                &mut lab,
                PackageTaskArchiveRequest {
                    task_id: task_id.clone(),
                    package_id,
                    execution_mode: "navigable_route".to_string(),
                    resolution,
                    out: build_out,
                    dry_run,
                    env: env.clone(),
                },
            ) {
                Ok(validation) => validation,
                Err(error) => {
                    if let Some(temp) = &temp_split_dir {
                        let _ = fs::remove_dir_all(temp);
                    }
                    return Err(error);
                }
            };
            packages.push(PackageBuildPackItemResponse {
                task_id,
                out: (!dry_run).then(|| final_out.display().to_string()),
                validation,
            });
        }
        if let Some(temp) = &temp_split_dir {
            move_split_packages(temp, &split_dir)?;
            fs::remove_dir_all(temp).map_err(|error| {
                CliError::package_invalid(format!("failed to remove {}: {error}", temp.display()))
            })?;
        }
        catalog.cleanup()?;
        return serialize_response(PackageBuildPackResponse::Split(Box::new(
            PackageBuildPackSplitResponse {
                status: if dry_run { "validated" } else { "written" }.to_string(),
                mode: "build-pack-split".to_string(),
                repo: metadata.repo.display().to_string(),
                resource_root: metadata.resource_root.display().to_string(),
                resource_layout: metadata.resource_layout,
                from_remote: metadata.from_remote,
                game: metadata.game,
                server: metadata.server,
                dry_run,
                package_count: packages.len(),
                packages,
            },
        )));
    }

    let out = flags
        .optional_path("--out")
        .ok_or_else(|| CliError::usage("missing --out <value>"))?;
    let entry_task_id = flags
        .optional("--entry-task")
        .unwrap_or_else(|| catalog.default_entry_task());
    let package_id = flags
        .optional("--package-id")
        .unwrap_or_else(|| format!("{}.{}.full", metadata.game, metadata.server));
    let execution_mode = flags
        .optional("--execution-mode")
        .unwrap_or_else(|| "recognize_only".to_string());
    let task_count = catalog.task_ids().len();
    let validation = catalog.build_full_archive(
        &mut lab,
        PackageFullArchiveRequest {
            entry_task_id: entry_task_id.clone(),
            package_id: package_id.clone(),
            execution_mode: execution_mode.clone(),
            resolution,
            out: out.clone(),
            dry_run,
            env,
        },
    )?;
    catalog.cleanup()?;
    serialize_response(PackageBuildPackResponse::Full(Box::new(
        PackageBuildPackFullResponse {
            status: if dry_run { "validated" } else { "written" }.to_string(),
            mode: "build-pack".to_string(),
            repo: metadata.repo.display().to_string(),
            resource_root: metadata.resource_root.display().to_string(),
            resource_layout: metadata.resource_layout,
            from_remote: metadata.from_remote,
            game: metadata.game,
            server: metadata.server,
            entry_task_id,
            package_id,
            execution_mode,
            task_count,
            dry_run,
            out: (!dry_run).then(|| out.display().to_string()),
            validation,
        },
    )))
}

#[derive(Serialize)]
#[serde(untagged)]
enum PackageBuildPackResponse {
    Split(Box<PackageBuildPackSplitResponse>),
    Full(Box<PackageBuildPackFullResponse>),
}

#[derive(Serialize)]
struct PackageBuildPackSplitResponse {
    status: String,
    mode: String,
    repo: String,
    resource_root: String,
    resource_layout: String,
    from_remote: Option<String>,
    game: String,
    server: String,
    dry_run: bool,
    package_count: usize,
    packages: Vec<PackageBuildPackItemResponse>,
}

#[derive(Serialize)]
struct PackageBuildPackItemResponse {
    task_id: String,
    out: Option<String>,
    validation: LabPackageValidationResponse,
}

#[derive(Serialize)]
struct PackageBuildPackFullResponse {
    status: String,
    mode: String,
    repo: String,
    resource_root: String,
    resource_layout: String,
    from_remote: Option<String>,
    game: String,
    server: String,
    entry_task_id: String,
    package_id: String,
    execution_mode: String,
    task_count: usize,
    dry_run: bool,
    out: Option<String>,
    validation: LabPackageValidationResponse,
}

fn package_source(flags: &FlagArgs) -> CliOutcome<PackageSource> {
    match (
        flags.optional("--from-remote"),
        flags.optional_path("--repo"),
    ) {
        (Some(_), Some(_)) => Err(CliError::usage(
            "pass either --repo or --from-remote, not both",
        )),
        (Some(url), None) => Ok(PackageSource::Remote(url)),
        (None, Some(path)) => Ok(PackageSource::Local(path)),
        (None, None) => Err(CliError::usage(
            "missing --repo <path> or --from-remote <url>",
        )),
    }
}

fn package_env(
    global: &GlobalOptions,
    flags: &FlagArgs,
    game: Option<String>,
    server: Option<String>,
) -> PackageEnvOptions {
    PackageEnvOptions {
        instance: flags
            .optional("--instance")
            .or_else(|| global.instance.clone()),
        game,
        server,
        env_task: flags.optional("--env-task"),
    }
}

fn parse_resolution(flags: &FlagArgs) -> CliOutcome<Option<PackageResolution>> {
    let Some(value) = flags.optional("--resolution") else {
        return Ok(None);
    };
    let Some((width, height)) = value.split_once('x').or_else(|| value.split_once('X')) else {
        return Err(CliError::usage("--resolution must use <width>x<height>"));
    };
    let width = width
        .parse::<u32>()
        .map_err(|error| CliError::usage(format!("invalid resolution width: {error}")))?;
    let height = height
        .parse::<u32>()
        .map_err(|error| CliError::usage(format!("invalid resolution height: {error}")))?;
    Ok(Some(PackageResolution { width, height }))
}

fn temp_dir_path(target: &Path) -> PathBuf {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("actinglab-split");
    parent.join(format!(
        ".{name}.tmp-{}-{}",
        std::process::id(),
        unique_suffix()
    ))
}

fn move_split_packages(temp: &Path, target: &Path) -> CliOutcome<()> {
    fs::create_dir_all(target).map_err(|error| {
        CliError::package_invalid(format!("failed to create {}: {error}", target.display()))
    })?;
    for entry in fs::read_dir(temp).map_err(|error| {
        CliError::package_invalid(format!("failed to read {}: {error}", temp.display()))
    })? {
        let entry = entry.map_err(|error| {
            CliError::package_invalid(format!("failed to read {}: {error}", temp.display()))
        })?;
        let source = entry.path();
        if !source.is_file() {
            continue;
        }
        let destination = target.join(entry.file_name());
        if destination.exists() {
            fs::remove_file(&destination).map_err(|error| {
                CliError::package_invalid(format!(
                    "failed to replace {}: {error}",
                    destination.display()
                ))
            })?;
        }
        fs::rename(&source, &destination).map_err(|error| {
            CliError::package_invalid(format!(
                "failed to move {} to {}: {error}",
                source.display(),
                destination.display()
            ))
        })?;
    }
    Ok(())
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn serialize_response<T: Serialize>(response: T) -> CliOutcome<Value> {
    serde_json::to_value(response)
        .map_err(|error| CliError::device(format!("failed to serialize Lab response: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;
    use tempfile::TempDir;

    #[test]
    fn package_build_pack_full_command_preserves_defaults_and_response_shape() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        let out = temp.path().join("full.zip");
        write_fixture_repo(&repo);

        let result = super::super::run_cli(
            [
                "--json",
                "package",
                "build-pack",
                "--repo",
                repo.to_str().unwrap(),
                "--entry-task",
                "operator_task",
                "--out",
                out.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0, "{:?}", result.envelope.error);
        let data = result.envelope.data.as_ref().unwrap().as_object().unwrap();
        assert_eq!(
            data.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "dry_run",
                "entry_task_id",
                "execution_mode",
                "from_remote",
                "game",
                "mode",
                "out",
                "package_id",
                "repo",
                "resource_layout",
                "resource_root",
                "server",
                "status",
                "task_count",
                "validation",
            ])
        );
        assert_eq!(data.get("status"), Some(&json!("written")));
        assert_eq!(data.get("mode"), Some(&json!("build-pack")));
        assert_eq!(data.get("game"), Some(&json!("arknights")));
        assert_eq!(data.get("server"), Some(&json!("cn")));
        assert_eq!(data.get("entry_task_id"), Some(&json!("operator_task")));
        assert_eq!(data.get("package_id"), Some(&json!("arknights.cn.full")));
        assert_eq!(data.get("execution_mode"), Some(&json!("recognize_only")));
        assert_eq!(data.get("task_count"), Some(&json!(2)));
        assert_eq!(data.get("dry_run"), Some(&json!(false)));
        assert_eq!(data.get("out"), Some(&json!(out.display().to_string())));
        assert_eq!(data.get("from_remote"), Some(&Value::Null));
        assert_eq!(
            data["validation"].pointer("/control/package_id"),
            Some(&json!("arknights.cn.full"))
        );
        assert_eq!(
            data["validation"].pointer("/control/execution_mode"),
            Some(&json!("recognize_only"))
        );
        assert!(out.is_file());
    }

    #[test]
    fn package_build_pack_split_command_promotes_and_cleans_staging_directory() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        let split_dir = temp.path().join("split");
        write_fixture_repo(&repo);

        let result = super::super::run_cli(
            [
                "--json",
                "package",
                "build-pack",
                "--repo",
                repo.to_str().unwrap(),
                "--split-dir",
                split_dir.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0, "{:?}", result.envelope.error);
        let data = result.envelope.data.as_ref().unwrap().as_object().unwrap();
        assert_eq!(
            data.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "dry_run",
                "from_remote",
                "game",
                "mode",
                "package_count",
                "packages",
                "repo",
                "resource_layout",
                "resource_root",
                "server",
                "status",
            ])
        );
        assert_eq!(data.get("status"), Some(&json!("written")));
        assert_eq!(data.get("mode"), Some(&json!("build-pack-split")));
        assert_eq!(data.get("game"), Some(&json!("arknights")));
        assert_eq!(data.get("server"), Some(&json!("cn")));
        assert_eq!(data.get("package_count"), Some(&json!(2)));
        assert_eq!(data.get("dry_run"), Some(&json!(false)));

        let mut packages = data["packages"]
            .as_array()
            .unwrap()
            .iter()
            .collect::<Vec<_>>();
        packages.sort_by(|left, right| {
            left["task_id"]
                .as_str()
                .unwrap()
                .cmp(right["task_id"].as_str().unwrap())
        });
        for package in packages {
            let task_id = package["task_id"].as_str().unwrap();
            let package_id = format!("arknights.cn.{task_id}");
            let out = split_dir.join(format!("{package_id}.zip"));
            assert_eq!(package["out"], json!(out.display().to_string()));
            assert_eq!(
                package.pointer("/validation/control/package_id"),
                Some(&json!(package_id))
            );
            assert_eq!(
                package.pointer("/validation/control/execution_mode"),
                Some(&json!("navigable_route"))
            );
            assert!(out.is_file());
        }
        assert_eq!(
            fs::read_dir(&split_dir)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_file())
                .count(),
            2
        );
        let staging_prefix = ".split.tmp-";
        let staging_dirs = fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(staging_prefix)
            })
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        assert!(
            staging_dirs.is_empty(),
            "split staging directories were not cleaned: {staging_dirs:?}"
        );
    }

    fn write_fixture_repo(root: &Path) {
        fs::create_dir_all(root.join("operations/operator_task/assets")).unwrap();
        fs::create_dir_all(root.join("operations/return_home/assets")).unwrap();
        fs::create_dir_all(root.join("navigation")).unwrap();
        fs::write(
            root.join("operations/resources.json"),
            serde_json::to_vec_pretty(&json!({
                "schema_version": "1.0",
                "resources": [],
                "resource_count": 0
            }))
            .unwrap(),
        )
        .unwrap();
        write_task_fixture(root, "operator_task", "operator", "OPERATOR.png", 10);
        write_task_fixture(root, "return_home", "home", "HOME.png", 20);
        fs::write(
            root.join("navigation/arknights.cn.navigation.json"),
            serde_json::to_vec_pretty(&json!({
                "schema_version": "0.3",
                "control_points": [{"name": "home", "point": [1, 1]}]
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn write_task_fixture(
        root: &Path,
        task_id: &str,
        page_id: &str,
        asset_name: &str,
        coordinate: i32,
    ) {
        let task_root = root.join("operations").join(task_id);
        fs::write(task_root.join("assets").join(asset_name), one_pixel_png()).unwrap();
        fs::write(
            task_root.join("task.json"),
            serde_json::to_vec_pretty(&json!({
                "schema_version": "0.3",
                "task_id": task_id,
                "game": "arknights",
                "server_scope": ["cn"],
                "goal": "app command fixture",
                "coordinate_space": {"width": 1280, "height": 720},
                "defaults": {"template_threshold": 0.9, "color_max_distance": 20.0},
                "anchors": [{
                    "id": page_id,
                    "template": format!("assets/{asset_name}"),
                    "region": {
                        "mode": "rect",
                        "rect": {"x": coordinate, "y": coordinate, "width": 1, "height": 1}
                    },
                    "threshold": 0.8,
                    "color_check": null
                }],
                "entry_page": page_id,
                "target_page": page_id,
                "operations": [{
                    "id": format!("{task_id}_noop"),
                    "purpose": "fixture",
                    "from": page_id,
                    "to": null,
                    "click": {"kind": "point", "x": coordinate, "y": coordinate},
                    "verify_template": null,
                    "guard": {
                        "page_id": page_id,
                        "target_id": format!("page/{page_id}"),
                        "expected_rect": {
                            "x": coordinate,
                            "y": coordinate,
                            "width": 1,
                            "height": 1
                        },
                        "verify_template": format!("assets/{asset_name}")
                    },
                    "consumes": [],
                    "produces": []
                }]
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn one_pixel_png() -> &'static [u8] {
        &[
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 15, 4,
            0, 9, 251, 3, 253, 167, 89, 75, 221, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
        ]
    }
}
