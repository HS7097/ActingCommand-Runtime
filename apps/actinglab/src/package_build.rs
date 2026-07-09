// SPDX-License-Identifier: AGPL-3.0-only

use super::env_detection;
use super::lab_run;
use super::resource_convert::{Bundle, ConvertOutputs, OperationConverter};
use super::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, canonical_game, default_server_for_game,
    hex_sha256, resolve_resource_root, zip_io_error, zip_write_error,
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use zip::ZipWriter;
use zip::write::FileOptions;

pub(super) fn run_build_task(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let mut source = ResolvedRepo::from_flags(flags)?;
    let repo = source.path().to_path_buf();
    let resource_root = resolve_resource_root(&repo);
    let converter = load_converter(global, flags, &resource_root.root)?;
    let task_id = flags.required("--task")?;
    let mut task_ids = vec![task_id.clone()];
    let includes_recovery = flags.bool("--include-recovery")
        && task_id != "return_home"
        && converter
            .bundles
            .iter()
            .any(|bundle| bundle.task_id == "return_home");
    if includes_recovery {
        task_ids.push("return_home".to_string());
    }
    let mut outputs = build_task_outputs(&converter, &task_ids, includes_recovery)?;
    env_detection::resolve_env_markers_in_value(
        global,
        flags,
        &resource_root.root,
        &mut outputs.pack,
    )?;
    let entry_bundle = find_bundle(&converter, &task_id)?;
    let resolution = parse_resolution(flags, entry_bundle)?;
    let package_id = flags
        .optional("--package-id")
        .unwrap_or_else(|| format!("{}.{}.{}", converter.game, converter.server, task_id));
    let execution_mode = flags
        .optional("--execution-mode")
        .unwrap_or_else(|| "navigable_route".to_string());
    validate_execution_mode(&execution_mode)?;

    let mut entries = PackageEntries::default();
    entries.add_json(
        "control.json",
        control_json(
            &package_id,
            &execution_mode,
            &converter.game,
            &converter.server,
            resolution,
            &task_id,
        ),
    )?;
    add_resources_json(
        &mut entries,
        &resource_root.root,
        &converter,
        &task_ids,
        true,
    )?;
    add_selected_operations(
        &mut entries,
        global,
        flags,
        &resource_root.root,
        &converter,
        &task_ids,
    )?;
    add_generated_outputs(&mut entries, &converter, &outputs)?;
    add_recognition_target_assets(&mut entries, &resource_root.root, &outputs.pack)?;
    entries.add_manifest(&task_id)?;

    let dry_run = global.dry_run || flags.bool("--dry-run");
    let out = flags.required_path("--out")?;
    let write = write_and_validate_package(&out, entries, dry_run)?;
    source.cleanup()?;
    Ok(json!({
        "status": if dry_run { "validated" } else { "written" },
        "mode": "build-task",
        "repo": repo.display().to_string(),
        "resource_root": resource_root.root.display().to_string(),
        "resource_layout": resource_root.layout,
        "from_remote": source.remote_url(),
        "task_id": task_id,
        "included_tasks": task_ids,
        "game": converter.game,
        "server": converter.server,
        "package_id": package_id,
        "execution_mode": execution_mode,
        "dry_run": dry_run,
        "out": if dry_run { Value::Null } else { Value::String(out.display().to_string()) },
        "validation": write.validation
    }))
}

pub(super) fn run_build_pack(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let mut source = ResolvedRepo::from_flags(flags)?;
    let repo = source.path().to_path_buf();
    let resource_root = resolve_resource_root(&repo);
    let converter = load_converter(global, flags, &resource_root.root)?;
    let dry_run = global.dry_run || flags.bool("--dry-run");

    if let Some(split_dir) = flags.optional_path("--split-dir") {
        let mut packages = Vec::new();
        let temp_split_dir = if dry_run {
            None
        } else {
            let temp = temp_dir_path(&split_dir);
            fs::create_dir_all(&temp).map_err(|err| {
                CliError::package_invalid(format!("failed to create {}: {err}", temp.display()))
            })?;
            Some(temp)
        };
        let build_dir = temp_split_dir.as_deref().unwrap_or(&split_dir);
        for bundle in &converter.bundles {
            let task_ids = vec![bundle.task_id.clone()];
            let outputs = converter.build_selected(&task_ids)?;
            let resolution = parse_resolution(flags, bundle)?;
            let package_id = format!("{}.{}.{}", converter.game, converter.server, bundle.task_id);
            let mut entries = PackageEntries::default();
            entries.add_json(
                "control.json",
                control_json(
                    &package_id,
                    "navigable_route",
                    &converter.game,
                    &converter.server,
                    resolution,
                    &bundle.task_id,
                ),
            )?;
            add_resources_json(
                &mut entries,
                &resource_root.root,
                &converter,
                &task_ids,
                true,
            )?;
            add_selected_operations(
                &mut entries,
                global,
                flags,
                &resource_root.root,
                &converter,
                &task_ids,
            )?;
            add_generated_outputs(&mut entries, &converter, &outputs)?;
            add_recognition_target_assets(&mut entries, &resource_root.root, &outputs.pack)?;
            entries.add_manifest(&bundle.task_id)?;
            let final_out = split_dir.join(format!("{}.zip", package_id));
            let build_out = build_dir.join(format!("{}.zip", package_id));
            let write = match write_and_validate_package(&build_out, entries, dry_run) {
                Ok(write) => write,
                Err(err) => {
                    if let Some(temp) = &temp_split_dir {
                        let _ = fs::remove_dir_all(temp);
                    }
                    return Err(err);
                }
            };
            packages.push(json!({
                "task_id": bundle.task_id,
                "out": if dry_run { Value::Null } else { Value::String(final_out.display().to_string()) },
                "validation": write.validation
            }));
        }
        if let Some(temp) = &temp_split_dir {
            move_split_packages(temp, &split_dir)?;
            fs::remove_dir_all(temp).map_err(|err| {
                CliError::package_invalid(format!("failed to remove {}: {err}", temp.display()))
            })?;
        }
        source.cleanup()?;
        return Ok(json!({
            "status": if dry_run { "validated" } else { "written" },
            "mode": "build-pack-split",
            "repo": repo.display().to_string(),
            "resource_root": resource_root.root.display().to_string(),
            "resource_layout": resource_root.layout,
            "from_remote": source.remote_url(),
            "game": converter.game,
            "server": converter.server,
            "dry_run": dry_run,
            "package_count": packages.len(),
            "packages": packages
        }));
    }

    let out = flags.required_path("--out")?;
    let entry_task = flags
        .optional("--entry-task")
        .unwrap_or_else(|| default_entry_task(&converter));
    let entry_bundle = find_bundle(&converter, &entry_task)?;
    let resolution = parse_resolution(flags, entry_bundle)?;
    let outputs = converter.build_all()?;
    let package_id = flags
        .optional("--package-id")
        .unwrap_or_else(|| format!("{}.{}.full", converter.game, converter.server));
    let execution_mode = flags
        .optional("--execution-mode")
        .unwrap_or_else(|| "recognize_only".to_string());
    validate_execution_mode(&execution_mode)?;

    let all_task_ids = converter
        .bundles
        .iter()
        .map(|bundle| bundle.task_id.clone())
        .collect::<Vec<_>>();
    let mut entries = PackageEntries::default();
    entries.add_json(
        "control.json",
        control_json(
            &package_id,
            &execution_mode,
            &converter.game,
            &converter.server,
            resolution,
            &entry_task,
        ),
    )?;
    add_resources_json(
        &mut entries,
        &resource_root.root,
        &converter,
        &all_task_ids,
        false,
    )?;
    add_selected_operations(
        &mut entries,
        global,
        flags,
        &resource_root.root,
        &converter,
        &all_task_ids,
    )?;
    add_generated_outputs(&mut entries, &converter, &outputs)?;
    entries.add_manifest(&entry_task)?;
    let write = write_and_validate_package(&out, entries, dry_run)?;
    source.cleanup()?;
    Ok(json!({
        "status": if dry_run { "validated" } else { "written" },
        "mode": "build-pack",
        "repo": repo.display().to_string(),
        "resource_root": resource_root.root.display().to_string(),
        "resource_layout": resource_root.layout,
        "from_remote": source.remote_url(),
        "game": converter.game,
        "server": converter.server,
        "entry_task_id": entry_task,
        "package_id": package_id,
        "execution_mode": execution_mode,
        "task_count": all_task_ids.len(),
        "dry_run": dry_run,
        "out": if dry_run { Value::Null } else { Value::String(out.display().to_string()) },
        "validation": write.validation
    }))
}

fn build_task_outputs(
    converter: &OperationConverter,
    task_ids: &[String],
    includes_recovery: bool,
) -> CliOutcome<ConvertOutputs> {
    let selected = converter.build_selected(task_ids)?;
    if !includes_recovery {
        return Ok(selected);
    }
    // Recovery may start from pages outside the entry task, so keep the
    // recognition context broad while leaving executable operations selected.
    let full = converter.build_all()?;
    Ok(ConvertOutputs {
        pack: full.pack,
        pages: full.pages,
        navigation: selected.navigation,
        index: selected.index,
        primitives: selected.primitives,
    })
}

fn load_converter(
    global: &GlobalOptions,
    flags: &FlagArgs,
    repo: &Path,
) -> CliOutcome<OperationConverter> {
    let game = flags.optional("--game").or_else(|| global.game.clone());
    let game = game.as_deref().map(canonical_game).transpose()?;
    let server = flags.optional("--server").or_else(|| global.server.clone());
    let locale = flags.optional("--locale");
    OperationConverter::load(repo, game.as_deref(), server.as_deref(), locale.as_deref())
}

fn find_bundle<'a>(converter: &'a OperationConverter, task_id: &str) -> CliOutcome<&'a Bundle> {
    converter
        .bundles
        .iter()
        .find(|bundle| bundle.task_id == task_id)
        .ok_or_else(|| {
            CliError::package_invalid(format!("missing task operations/{task_id}/task.json"))
        })
}

fn default_entry_task(converter: &OperationConverter) -> String {
    if converter
        .bundles
        .iter()
        .any(|bundle| bundle.task_id == "return_home")
    {
        "return_home".to_string()
    } else {
        converter
            .bundles
            .first()
            .map(|bundle| bundle.task_id.clone())
            .unwrap_or_else(|| {
                format!(
                    "{}.{}",
                    converter.game,
                    default_server_for_game(&converter.game)
                )
            })
    }
}

fn parse_resolution(flags: &FlagArgs, bundle: &Bundle) -> CliOutcome<(u32, u32)> {
    if let Some(value) = flags.optional("--resolution") {
        let Some((width, height)) = value.split_once('x').or_else(|| value.split_once('X')) else {
            return Err(CliError::usage("--resolution must use <width>x<height>"));
        };
        let width = width
            .parse::<u32>()
            .map_err(|err| CliError::usage(format!("invalid resolution width: {err}")))?;
        let height = height
            .parse::<u32>()
            .map_err(|err| CliError::usage(format!("invalid resolution height: {err}")))?;
        return Ok((width, height));
    }
    let space = bundle
        .data
        .get("coordinate_space")
        .ok_or_else(|| CliError::package_invalid("operation bundle missing coordinate_space"))?;
    let width = json_u32(space, "width")?;
    let height = json_u32(space, "height")?;
    Ok((width, height))
}

fn validate_execution_mode(mode: &str) -> CliOutcome<()> {
    if matches!(mode, "navigable_route" | "recognize_only" | "in_page_guard") {
        Ok(())
    } else {
        Err(CliError::usage(format!(
            "unsupported execution mode: {mode}"
        )))
    }
}

fn control_json(
    package_id: &str,
    execution_mode: &str,
    game: &str,
    server: &str,
    resolution: (u32, u32),
    entry_task_id: &str,
) -> Value {
    ordered_object([
        (
            "schema_version",
            Value::String("Lab-1y.control.v1".to_string()),
        ),
        ("package_id", Value::String(package_id.to_string())),
        ("execution_mode", Value::String(execution_mode.to_string())),
        ("game", Value::String(game.to_string())),
        ("server", Value::String(server.to_string())),
        (
            "resolution",
            ordered_object([
                ("width", Value::from(resolution.0)),
                ("height", Value::from(resolution.1)),
            ]),
        ),
        ("entry_task_id", Value::String(entry_task_id.to_string())),
    ])
}

fn add_resources_json(
    entries: &mut PackageEntries,
    repo: &Path,
    converter: &OperationConverter,
    task_ids: &[String],
    subset: bool,
) -> CliOutcome<()> {
    let path = repo.join("operations").join("resources.json");
    let mut resources = read_json_value(&path)?;
    if subset {
        let referenced = referenced_resource_ids(converter, task_ids);
        if let Some(array) = resources.get_mut("resources").and_then(Value::as_array_mut) {
            array.retain(|resource| {
                resource
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| referenced.contains(id))
            });
            let resource_count = array.len();
            if let Some(object) = resources.as_object_mut() {
                object.insert("resource_count".to_string(), Value::from(resource_count));
            }
        }
    }
    entries.add_json("resources/operations/resources.json", resources)
}

fn referenced_resource_ids(
    converter: &OperationConverter,
    task_ids: &[String],
) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for bundle in &converter.bundles {
        if !task_ids.iter().any(|task_id| task_id == &bundle.task_id) {
            continue;
        }
        for operation in array_field(&bundle.data, "operations") {
            for key in ["consumes", "produces"] {
                for value in operation
                    .get(key)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(id) = value.as_str() {
                        ids.insert(id.to_string());
                    }
                }
            }
        }
    }
    ids
}

fn add_selected_operations(
    entries: &mut PackageEntries,
    global: &GlobalOptions,
    flags: &FlagArgs,
    resource_root: &Path,
    converter: &OperationConverter,
    task_ids: &[String],
) -> CliOutcome<()> {
    for bundle in &converter.bundles {
        if task_ids.iter().any(|task_id| task_id == &bundle.task_id) {
            entries.add_operation_dir(
                &bundle.dir,
                &format!("resources/operations/{}", bundle.task_id),
                global,
                flags,
                resource_root,
            )?;
        }
    }
    Ok(())
}

fn add_generated_outputs(
    entries: &mut PackageEntries,
    converter: &OperationConverter,
    outputs: &ConvertOutputs,
) -> CliOutcome<()> {
    let stem = format!("{}.{}", converter.game, converter.server);
    entries.add_json(
        &format!("resources/recognition/{stem}.pack.json"),
        outputs.pack.clone(),
    )?;
    entries.add_json(
        &format!("resources/recognition/{stem}.pages.json"),
        outputs.pages.clone(),
    )?;
    entries.add_json(
        &format!("resources/navigation/{stem}.navigation.json"),
        outputs.navigation.clone(),
    )?;
    entries.add_json(
        "resources/operations/operations.index.json",
        outputs.index.clone(),
    )?;
    entries.add_json(
        "resources/operations/operations.primitives.json",
        outputs.primitives.clone(),
    )
}

fn add_recognition_target_assets(
    entries: &mut PackageEntries,
    repo: &Path,
    pack: &Value,
) -> CliOutcome<()> {
    for target in pack
        .get("targets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(template_path) = target.get("template_path").and_then(Value::as_str) else {
            continue;
        };
        let zip_path = format!("resources/{template_path}");
        if entries.contains(&zip_path) {
            continue;
        }
        entries.add_file(&repo.join(template_path), &zip_path)?;
    }
    Ok(())
}

#[derive(Default)]
struct PackageEntries {
    files: BTreeMap<String, Vec<u8>>,
}

impl PackageEntries {
    fn add_json(&mut self, path: &str, value: Value) -> CliOutcome<()> {
        let mut text = serde_json::to_string_pretty(&value).map_err(|err| {
            CliError::package_invalid(format!("failed to serialize {path}: {err}"))
        })?;
        text.push('\n');
        self.add_bytes(path, text.into_bytes())
    }

    fn add_file(&mut self, source: &Path, zip_path: &str) -> CliOutcome<()> {
        let bytes = fs::read(source).map_err(|err| {
            CliError::package_invalid(format!("failed to read {}: {err}", source.display()))
        })?;
        self.add_bytes(zip_path, bytes)
    }

    fn add_operation_dir(
        &mut self,
        source_dir: &Path,
        zip_prefix: &str,
        global: &GlobalOptions,
        flags: &FlagArgs,
        resource_root: &Path,
    ) -> CliOutcome<()> {
        for path in collect_files(source_dir)? {
            let rel = relative_slash(source_dir, &path)?;
            if rel == "task.json" {
                let mut task = read_json_value(&path)?;
                env_detection::resolve_env_markers_in_value(
                    global,
                    flags,
                    resource_root,
                    &mut task,
                )?;
                self.add_root_relative_operation_assets(&task, resource_root)?;
                self.add_json(&format!("{zip_prefix}/{rel}"), task)?;
            } else {
                self.add_file(&path, &format!("{zip_prefix}/{rel}"))?;
            }
        }
        Ok(())
    }

    fn add_root_relative_operation_assets(
        &mut self,
        task: &Value,
        resource_root: &Path,
    ) -> CliOutcome<()> {
        for path in task_verify_template_paths(task) {
            if path.starts_with("assets/") {
                continue;
            }
            if !is_safe_package_relative_path(path) {
                return Err(CliError::package_invalid(format!(
                    "operation verify_template path is unsafe: {path}"
                )));
            }
            let source = resource_root.join(path);
            if !source.is_file() {
                continue;
            }
            let zip_path = format!("resources/{path}");
            if !self.contains(&zip_path) {
                self.add_file(&source, &zip_path)?;
            }
        }
        Ok(())
    }

    fn add_manifest(&mut self, entry_task_id: &str) -> CliOutcome<()> {
        let files = self
            .files
            .iter()
            .filter(|(path, _)| {
                path.starts_with("resources/") && path.as_str() != "resources/manifest.json"
            })
            .map(|(path, bytes)| {
                ordered_object([
                    (
                        "path",
                        Value::String(path.trim_start_matches("resources/").to_string()),
                    ),
                    (
                        "sha256",
                        Value::String(format!("sha256:{}", hex_sha256(bytes))),
                    ),
                ])
            })
            .collect::<Vec<_>>();
        let manifest = ordered_object([
            ("schema_version", Value::String("0.3".to_string())),
            ("entry_task_id", Value::String(entry_task_id.to_string())),
            ("files", Value::Array(files)),
        ]);
        self.add_json("resources/manifest.json", manifest)
    }

    fn add_bytes(&mut self, path: &str, bytes: Vec<u8>) -> CliOutcome<()> {
        validate_zip_entry_path(path)?;
        if self.files.insert(path.to_string(), bytes).is_some() {
            return Err(CliError::package_invalid(format!(
                "duplicate package entry: {path}"
            )));
        }
        Ok(())
    }

    fn contains(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }
}

struct PackageWrite {
    validation: Value,
}

fn write_and_validate_package(
    out: &Path,
    entries: PackageEntries,
    dry_run: bool,
) -> CliOutcome<PackageWrite> {
    let temp = temp_zip_path(out)?;
    write_zip(&temp, &entries.files)?;
    let validation = match lab_run::validate_lab_package_zip(&temp) {
        Ok(value) => value,
        Err(err) => {
            let _ = fs::remove_file(&temp);
            return Err(err);
        }
    };
    if dry_run {
        fs::remove_file(&temp).map_err(|err| {
            CliError::package_invalid(format!("failed to remove {}: {err}", temp.display()))
        })?;
        return Ok(PackageWrite { validation });
    }
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::package_invalid(format!("failed to create {}: {err}", parent.display()))
        })?;
    }
    if out.exists() {
        if out.is_dir() {
            let _ = fs::remove_file(&temp);
            return Err(CliError::package_invalid(format!(
                "output path is a directory: {}",
                out.display()
            )));
        }
        fs::remove_file(out).map_err(|err| {
            let _ = fs::remove_file(&temp);
            CliError::package_invalid(format!("failed to replace {}: {err}", out.display()))
        })?;
    }
    fs::rename(&temp, out).map_err(|err| {
        let _ = fs::remove_file(&temp);
        CliError::package_invalid(format!(
            "failed to move {} to {}: {err}",
            temp.display(),
            out.display()
        ))
    })?;
    Ok(PackageWrite { validation })
}

fn write_zip(path: &Path, files: &BTreeMap<String, Vec<u8>>) -> CliOutcome<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::package_invalid(format!("failed to create {}: {err}", parent.display()))
        })?;
    }
    let file = File::create(path).map_err(|err| {
        CliError::package_invalid(format!("failed to create {}: {err}", path.display()))
    })?;
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut zip = ZipWriter::new(file);
    for (entry, bytes) in files {
        zip.start_file(entry, options).map_err(zip_write_error)?;
        zip.write_all(bytes).map_err(zip_io_error)?;
    }
    zip.finish().map_err(zip_write_error)?;
    Ok(())
}

struct ResolvedRepo {
    path: PathBuf,
    remote_url: Option<String>,
    temp_root: Option<PathBuf>,
}

impl ResolvedRepo {
    fn from_flags(flags: &FlagArgs) -> CliOutcome<Self> {
        let remote_url = flags.optional("--from-remote");
        let local_repo = flags.optional_path("--repo");
        match (remote_url, local_repo) {
            (Some(_), Some(_)) => Err(CliError::usage(
                "pass either --repo or --from-remote, not both",
            )),
            (Some(url), None) => Self::clone_remote(url),
            (None, Some(path)) => Ok(Self {
                path,
                remote_url: None,
                temp_root: None,
            }),
            (None, None) => Err(CliError::usage(
                "missing --repo <path> or --from-remote <url>",
            )),
        }
    }

    fn clone_remote(url: String) -> CliOutcome<Self> {
        let root = std::env::temp_dir().join(format!(
            "actinglab-resource-remote-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let path = root.join("repo");
        fs::create_dir_all(&root).map_err(|err| {
            CliError::usage(format!("failed to create {}: {err}", root.display()))
        })?;
        let output = Command::new("git")
            .args(["clone", "--depth", "1", &url])
            .arg(&path)
            .output()
            .map_err(|err| CliError::usage(format!("failed to start git clone: {err}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let _ = fs::remove_dir_all(&root);
            return Err(CliError::usage(format!(
                "git clone failed: {}",
                stderr.trim()
            )));
        }
        Ok(Self {
            path,
            remote_url: Some(url),
            temp_root: Some(root),
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn remote_url(&self) -> Value {
        self.remote_url
            .as_ref()
            .map(|url| Value::String(url.clone()))
            .unwrap_or(Value::Null)
    }

    fn cleanup(&mut self) -> CliOutcome<()> {
        if let Some(root) = self.temp_root.take() {
            fs::remove_dir_all(&root).map_err(|err| {
                CliError::package_invalid(format!(
                    "failed to remove remote temp directory {}: {err}",
                    root.display()
                ))
            })?;
        }
        Ok(())
    }
}

impl Drop for ResolvedRepo {
    fn drop(&mut self) {
        if let Some(root) = self.temp_root.take() {
            let _ = fs::remove_dir_all(root);
        }
    }
}

fn read_json_value(path: &Path) -> CliOutcome<Value> {
    let text = fs::read_to_string(path).map_err(|err| {
        CliError::package_invalid(format!("failed to read {}: {err}", path.display()))
    })?;
    serde_json::from_str(&text).map_err(|err| {
        CliError::package_invalid(format!("failed to parse {}: {err}", path.display()))
    })
}

fn collect_files(root: &Path) -> CliOutcome<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_files_inner(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_files_inner(root: &Path, files: &mut Vec<PathBuf>) -> CliOutcome<()> {
    let mut entries = fs::read_dir(root)
        .map_err(|err| {
            CliError::package_invalid(format!("failed to read {}: {err}", root.display()))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            CliError::package_invalid(format!("failed to read {}: {err}", root.display()))
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_files_inner(&path, files)?;
        } else if path.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn relative_slash(root: &Path, path: &Path) -> CliOutcome<String> {
    let rel = path.strip_prefix(root).map_err(|err| {
        CliError::package_invalid(format!(
            "{} is outside {}: {err}",
            path.display(),
            root.display()
        ))
    })?;
    Ok(rel
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

fn validate_zip_entry_path(path: &str) -> CliOutcome<()> {
    if path.ends_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path.starts_with('/')
        || Path::new(path).components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(CliError::package_invalid(format!(
            "unsafe package entry path: {path}"
        )));
    }
    if super::DANGEROUS_EXTENSIONS.iter().any(|extension| {
        Path::new(path)
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case(extension))
    }) {
        return Err(CliError::package_invalid(format!(
            "package entry has dangerous extension: {path}"
        )));
    }
    Ok(())
}

fn is_safe_package_relative_path(path: &str) -> bool {
    !path.ends_with('/')
        && !path.contains('\\')
        && !path.contains(':')
        && !path.starts_with('/')
        && !Path::new(path).components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

fn task_verify_template_paths(task: &Value) -> Vec<&str> {
    let mut paths = Vec::new();
    collect_task_verify_template_paths(task, &mut paths);
    paths.sort_unstable();
    paths.dedup();
    paths
}

fn collect_task_verify_template_paths<'a>(value: &'a Value, paths: &mut Vec<&'a str>) {
    match value {
        Value::Object(object) => {
            if let Some(path) = object.get("verify_template").and_then(Value::as_str)
                && !path.trim().is_empty()
            {
                paths.push(path);
            }
            for value in object.values() {
                collect_task_verify_template_paths(value, paths);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_task_verify_template_paths(value, paths);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn temp_zip_path(out: &Path) -> CliOutcome<PathBuf> {
    let parent = out.parent().unwrap_or_else(|| Path::new("."));
    let file_name = out
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("actinglab-package.zip");
    Ok(parent.join(format!(
        ".{file_name}.tmp-{}-{}.zip",
        std::process::id(),
        unique_suffix()
    )))
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
    fs::create_dir_all(target).map_err(|err| {
        CliError::package_invalid(format!("failed to create {}: {err}", target.display()))
    })?;
    for entry in fs::read_dir(temp).map_err(|err| {
        CliError::package_invalid(format!("failed to read {}: {err}", temp.display()))
    })? {
        let entry = entry.map_err(|err| {
            CliError::package_invalid(format!("failed to read {}: {err}", temp.display()))
        })?;
        let source = entry.path();
        if !source.is_file() {
            continue;
        }
        let file_name = entry.file_name();
        let destination = target.join(file_name);
        if destination.exists() {
            fs::remove_file(&destination).map_err(|err| {
                CliError::package_invalid(format!(
                    "failed to replace {}: {err}",
                    destination.display()
                ))
            })?;
        }
        fs::rename(&source, &destination).map_err(|err| {
            CliError::package_invalid(format!(
                "failed to move {} to {}: {err}",
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

fn json_u32(value: &Value, key: &str) -> CliOutcome<u32> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| CliError::package_invalid(format!("missing u32 field {key}")))
}

fn array_field<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn ordered_object<const N: usize>(fields: [(&str, Value); N]) -> Value {
    let mut map = Map::new();
    for (key, value) in fields {
        map.insert(key.to_string(), value);
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use tempfile::TempDir;
    use zip::ZipArchive;

    #[test]
    fn build_task_package_validates_and_rewrites_template_paths() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        let out = temp.path().join("task.zip");

        let result = super::super::run_cli(
            [
                "--json",
                "package",
                "build-task",
                "--repo",
                repo.to_str().unwrap(),
                "--task",
                "operator_task",
                "--out",
                out.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0, "{:?}", result.envelope.error);
        assert!(out.is_file());
        let entries = read_zip_entries(&out);
        assert!(entries.contains_key("control.json"));
        assert!(entries.contains_key("resources/manifest.json"));
        assert!(entries.contains_key("resources/operations/operator_task/task.json"));
        assert!(
            entries.contains_key("resources/operations/operator_task/assets/PAGE_OPERATOR_0.png")
        );
        assert!(
            entries.contains_key("resources/operations/operator_task/assets/PAGE_OPERATOR_1.png")
        );
        let pack: Value = serde_json::from_slice(
            entries
                .get("resources/recognition/arknights.cn.pack.json")
                .unwrap(),
        )
        .unwrap();
        let paths = pack["targets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|target| target["template_path"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        for path in paths {
            assert!(entries.contains_key(&format!("resources/{path}")), "{path}");
        }
        let pages: Value = serde_json::from_slice(
            entries
                .get("resources/recognition/arknights.cn.pages.json")
                .unwrap(),
        )
        .unwrap();
        let operator = pages["pages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|page| page["id"] == "arknights/operator")
            .unwrap();
        assert_eq!(operator["required"], json!([]));
        assert_eq!(
            operator["any_of"],
            json!([["page/operator_0", "page/operator_1"]])
        );
    }

    #[test]
    fn build_task_accepts_reorganized_repo_root_with_ours_layout() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo.join("ours"));
        let out = temp.path().join("task.zip");

        let result = super::super::run_cli(
            [
                "--json",
                "package",
                "build-task",
                "--repo",
                repo.to_str().unwrap(),
                "--task",
                "operator_task",
                "--out",
                out.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0, "{:?}", result.envelope.error);
        let data = result.envelope.data.as_ref().unwrap().as_object().unwrap();
        assert_eq!(
            data.get("resource_layout").and_then(Value::as_str),
            Some("repo_ours")
        );
        assert!(
            data.get("resource_root")
                .and_then(Value::as_str)
                .is_some_and(|path| path.ends_with("ours"))
        );
        assert!(out.is_file());
    }

    #[test]
    fn build_task_with_recovery_keeps_recovery_recognition_context() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        add_recruit_fixture(&repo);
        add_return_home_recruit_rule(&repo);
        let out = temp.path().join("task.zip");

        let result = super::super::run_cli(
            [
                "--json",
                "package",
                "build-task",
                "--repo",
                repo.to_str().unwrap(),
                "--task",
                "operator_task",
                "--include-recovery",
                "--out",
                out.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0, "{:?}", result.envelope.error);
        let entries = read_zip_entries(&out);
        assert!(entries.contains_key("resources/operations/recruit_task/assets/RECRUIT.png"));
        assert!(!entries.contains_key("resources/operations/recruit_task/task.json"));
        let pages: Value = serde_json::from_slice(
            entries
                .get("resources/recognition/arknights.cn.pages.json")
                .unwrap(),
        )
        .unwrap();
        assert!(
            pages["pages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|page| page["id"] == "arknights/recruit")
        );
        let pack: Value = serde_json::from_slice(
            entries
                .get("resources/recognition/arknights.cn.pack.json")
                .unwrap(),
        )
        .unwrap();
        assert!(
            pack["targets"]
                .as_array()
                .unwrap()
                .iter()
                .any(|target| target["id"] == "page/recruit")
        );
    }

    #[test]
    fn build_pack_package_validates() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        let out = temp.path().join("full.zip");

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
        assert!(out.is_file());
    }

    #[test]
    fn split_pack_writes_one_package_per_task() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        let split_dir = temp.path().join("split");

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
        assert!(split_dir.join("arknights.cn.operator_task.zip").is_file());
        assert!(split_dir.join("arknights.cn.return_home.zip").is_file());
    }

    #[test]
    fn build_task_rejects_dangerous_asset_entry() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        fs::write(
            repo.join("operations/operator_task/assets/bad.ps1"),
            "Write-Host bad",
        )
        .unwrap();

        let result = super::super::run_cli(
            [
                "--json",
                "package",
                "build-task",
                "--repo",
                repo.to_str().unwrap(),
                "--task",
                "operator_task",
                "--out",
                temp.path().join("bad.zip").to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 2);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "package_invalid"
        );
    }

    fn read_zip_entries(path: &Path) -> BTreeMap<String, Vec<u8>> {
        let file = File::open(path).unwrap();
        let mut zip = ZipArchive::new(file).unwrap();
        let mut entries = BTreeMap::new();
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index).unwrap();
            if entry.name().ends_with('/') {
                continue;
            }
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            entries.insert(entry.name().to_string(), bytes);
        }
        entries
    }

    fn write_fixture_repo(root: &Path) {
        fs::create_dir_all(root.join("operations/operator_task/assets")).unwrap();
        fs::create_dir_all(root.join("operations/return_home/assets")).unwrap();
        fs::create_dir_all(root.join("navigation")).unwrap();
        fs::write(
            root.join("operations/resources.json"),
            serde_json::to_string_pretty(&json!({
                "schema_version": "1.0",
                "resources": [
                    {"id": "sanity", "name": {"cn": "sanity"}},
                    {"id": "credit", "name": {"cn": "credit"}}
                ],
                "resource_count": 2
            }))
            .unwrap(),
        )
        .unwrap();
        for path in [
            "operations/operator_task/assets/PAGE_OPERATOR_0.png",
            "operations/operator_task/assets/PAGE_OPERATOR_1.png",
            "operations/operator_task/assets/MIDDLE.png",
            "operations/operator_task/assets/MALL.png",
            "operations/return_home/assets/HOME.png",
        ] {
            fs::write(root.join(path), one_pixel_png()).unwrap();
        }
        fs::write(
            root.join("operations/operator_task/task.json"),
            serde_json::to_string_pretty(&json!({
                "schema_version": "0.3",
                "task_id": "operator_task",
                "game": "arknights",
                "server_scope": ["cn"],
                "goal": "fixture",
                "coordinate_space": {"width": 1280, "height": 720},
                "defaults": {"template_threshold": 0.9, "color_max_distance": 20.0},
                "anchors": [
                    {"id": "operator_0", "template": "assets/PAGE_OPERATOR_0.png", "region": {"mode": "rect", "rect": {"x": 1, "y": 2, "width": 3, "height": 4}}, "threshold": 0.8, "color_check": null},
                    {"id": "operator_1", "template": "assets/PAGE_OPERATOR_1.png", "region": {"mode": "rect", "rect": {"x": 5, "y": 6, "width": 7, "height": 8}}, "threshold": 0.8, "color_check": null},
                    {"id": "middle", "template": "assets/MIDDLE.png", "region": {"mode": "rect", "rect": {"x": 9, "y": 10, "width": 11, "height": 12}}, "threshold": 0.8, "color_check": null},
                    {"id": "mall", "template": "assets/MALL.png", "region": {"mode": "rect", "rect": {"x": 13, "y": 14, "width": 15, "height": 16}}, "threshold": 0.8, "color_check": null}
                ],
                "entry_page": "operator",
                "target_page": "mall",
                "operations": [
                    {"id": "operator_to_middle", "purpose": "go middle", "from": "operator", "to": "middle", "click": {"kind": "rect", "x": 100, "y": 100, "width": 20, "height": 20}, "verify_template": null, "guard": {"page_id": "operator", "target_id": "page/operator_0", "expected_rect": {"x": 100, "y": 100, "width": 20, "height": 20}, "verify_template": "assets/PAGE_OPERATOR_0.png"}, "consumes": [], "produces": ["credit"]},
                    {"id": "middle_to_mall", "purpose": "go mall", "from": "middle", "to": "mall", "click": {"kind": "rect", "x": 200, "y": 100, "width": 20, "height": 20}, "verify_template": "assets/MALL.png", "guard": {"page_id": "middle", "target_id": "page/middle", "expected_rect": {"x": 200, "y": 100, "width": 20, "height": 20}, "verify_template": "assets/MIDDLE.png"}, "consumes": ["sanity"], "produces": []}
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            root.join("operations/return_home/task.json"),
            serde_json::to_string_pretty(&json!({
                "schema_version": "0.3",
                "task_id": "return_home",
                "game": "arknights",
                "server_scope": ["cn"],
                "goal": "fixture",
                "coordinate_space": {"width": 1280, "height": 720},
                "defaults": {"template_threshold": 0.9, "color_max_distance": 20.0},
                "anchors": [
                    {"id": "home", "template": "assets/HOME.png", "region": {"mode": "rect", "rect": {"x": 20, "y": 20, "width": 30, "height": 30}}, "threshold": 0.8, "color_check": null}
                ],
                "entry_page": "home",
                "target_page": "home",
                "operations": [
                    {"id": "home_noop", "purpose": "noop", "from": "home", "to": null, "click": {"kind": "point", "x": 1, "y": 1}, "verify_template": null, "guard": {"page_id": "home", "target_id": "page/home", "expected_rect": {"x": 1, "y": 1, "width": 1, "height": 1}, "verify_template": "assets/HOME.png"}, "consumes": [], "produces": []}
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            root.join("navigation/arknights.cn.navigation.json"),
            serde_json::to_string_pretty(&json!({
                "schema_version": "0.3",
                "control_points": [{"name": "home", "point": [1, 1]}]
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn add_recruit_fixture(root: &Path) {
        fs::create_dir_all(root.join("operations/recruit_task/assets")).unwrap();
        fs::write(
            root.join("operations/recruit_task/assets/RECRUIT.png"),
            one_pixel_png(),
        )
        .unwrap();
        fs::write(
            root.join("operations/recruit_task/task.json"),
            serde_json::to_string_pretty(&json!({
                "schema_version": "0.3",
                "task_id": "recruit_task",
                "game": "arknights",
                "server_scope": ["cn"],
                "goal": "fixture",
                "coordinate_space": {"width": 1280, "height": 720},
                "defaults": {"template_threshold": 0.9, "color_max_distance": 20.0},
                "anchors": [
                    {"id": "recruit", "template": "assets/RECRUIT.png", "region": {"mode": "rect", "rect": {"x": 40, "y": 40, "width": 50, "height": 50}}, "threshold": 0.8, "color_check": null}
                ],
                "entry_page": "recruit",
                "target_page": "recruit",
                "operations": []
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn add_return_home_recruit_rule(root: &Path) {
        let path = root.join("operations/return_home/task.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value.as_object_mut().unwrap().insert(
            "page_rules".to_string(),
            json!({
                "home": {"required": ["page/home"]},
                "recruit": {"required": ["page/recruit"], "forbidden": ["page/home"]}
            }),
        );
        fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    }

    fn one_pixel_png() -> &'static [u8] {
        &[
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 15, 4,
            0, 9, 251, 3, 253, 167, 89, 75, 221, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
        ]
    }
}
