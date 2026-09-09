// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{GitObjectAlgorithm, GitOid, GitSourceTree, safe_source_path};
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Instant;

pub(super) fn source_error(code: &'static str) -> ContainmentError {
    ContainmentError::SourceTree { code }
}

struct GitReader<'a> {
    root: &'a Path,
    deadline: Instant,
    limits: ContainmentLimits,
}

impl GitReader<'_> {
    fn check_time(&self) -> ContainmentResult<()> {
        if Instant::now() >= self.deadline {
            return Err(source_error("source_tree_deadline"));
        }
        Ok(())
    }

    fn command(&self, args: &[&str], limit: u64) -> ContainmentResult<Vec<u8>> {
        self.check_time()?;
        let mut command = Command::new("git");
        // No config-driven filter, hooks, replacement objects or lazy network fetch.
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                command.env_remove(key);
            }
        }
        command
            .args([
                "--no-replace-objects",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "protocol.allow=never",
            ])
            .args(args)
            .current_dir(self.root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command
            .spawn()
            .map_err(|_| source_error("source_git_start_failed"))?;
        let Some(stdout) = child.stdout.take() else {
            terminate_git(&mut child)?;
            return Err(source_error("source_git_stdout_missing"));
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        let reader = std::thread::Builder::new()
            .name("contained-git-read".to_owned())
            .spawn(move || {
                let mut bytes = Vec::new();
                let result = stdout
                    .take(limit.saturating_add(1))
                    .read_to_end(&mut bytes)
                    .map_err(|_| "source_git_read_failed")
                    .and({
                        if bytes.len() as u64 > limit {
                            Err("source_git_output_limit")
                        } else {
                            Ok(bytes)
                        }
                    });
                let _ = sender.send(result);
            });
        let reader = match reader {
            Ok(reader) => reader,
            Err(_) => {
                terminate_git(&mut child)?;
                return Err(source_error("source_git_reader_start_failed"));
            }
        };
        let result = receiver.recv_timeout(self.deadline.saturating_duration_since(Instant::now()));
        let bytes = match result {
            Ok(Ok(bytes)) => bytes,
            failed => {
                terminate_git(&mut child)?;
                reader
                    .join()
                    .map_err(|_| source_error("source_git_reader_panicked"))?;
                return Err(source_error(match failed {
                    Ok(Err(code)) => code,
                    _ => "source_tree_deadline",
                }));
            }
        };
        loop {
            let status = match child.try_wait() {
                Ok(status) => status,
                Err(_) => {
                    terminate_git(&mut child)?;
                    reader
                        .join()
                        .map_err(|_| source_error("source_git_reader_panicked"))?;
                    return Err(source_error("source_git_wait_failed"));
                }
            };
            if let Some(status) = status {
                reader
                    .join()
                    .map_err(|_| source_error("source_git_reader_panicked"))?;
                return if status.success() {
                    Ok(bytes)
                } else {
                    Err(source_error("source_git_command_failed"))
                };
            }
            if self.check_time().is_err() {
                terminate_git(&mut child)?;
                reader
                    .join()
                    .map_err(|_| source_error("source_git_reader_panicked"))?;
                return Err(source_error("source_tree_deadline"));
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    fn object(&self, oid: &GitOid, kind: &str) -> ContainmentResult<Vec<u8>> {
        oid.validate()
            .map_err(|_| source_error("source_git_oid_invalid"))?;
        let bytes = self.command(&["cat-file", kind, &oid.hex], self.limits.max_entry_bytes)?;
        if object_hex(oid.algorithm, kind, &bytes) != oid.hex {
            return Err(source_error("source_git_object_hash_mismatch"));
        }
        Ok(bytes)
    }

    fn tree(&self, oid: &GitOid) -> ContainmentResult<Vec<TreeEntry>> {
        let bytes = self.object(oid, "tree")?;
        let oid_bytes = match oid.algorithm {
            GitObjectAlgorithm::Sha1 => 20,
            GitObjectAlgorithm::Sha256 => 32,
        };
        let mut rest = bytes.as_slice();
        let mut entries = Vec::new();
        let mut names = BTreeSet::new();
        while !rest.is_empty() {
            if entries.len() >= self.limits.max_entry_count {
                return Err(source_error("source_tree_entry_limit"));
            }
            let space = rest
                .iter()
                .position(|b| *b == b' ')
                .ok_or_else(|| source_error("source_tree_mode_invalid"))?;
            let zero = rest
                .iter()
                .position(|b| *b == 0)
                .ok_or_else(|| source_error("source_tree_name_invalid"))?;
            if space >= zero || zero + 1 + oid_bytes > rest.len() {
                return Err(source_error("source_tree_entry_invalid"));
            }
            let mode = std::str::from_utf8(&rest[..space])
                .map_err(|_| source_error("source_tree_mode_invalid"))?;
            let name = std::str::from_utf8(&rest[space + 1..zero])
                .map_err(|_| source_error("source_tree_name_invalid"))?;
            if !safe_source_path(name)
                || name == "."
                || name.contains('/')
                || name.eq_ignore_ascii_case(".git")
                || !names.insert(name.to_ascii_lowercase())
            {
                return Err(source_error("source_tree_name_invalid"));
            }
            let hex = rest[zero + 1..zero + 1 + oid_bytes]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            entries.push(TreeEntry {
                name: name.to_owned(),
                mode: mode.to_owned(),
                oid: GitOid {
                    algorithm: oid.algorithm,
                    hex,
                },
            });
            rest = &rest[zero + 1 + oid_bytes..];
        }
        Ok(entries)
    }

    fn collect(
        &self,
        oid: &GitOid,
        prefix: &str,
        entries: &mut BTreeMap<String, TreeEntry>,
        count: &mut usize,
    ) -> ContainmentResult<()> {
        if prefix.split('/').count() > 64 {
            return Err(source_error("source_tree_depth_limit"));
        }
        for entry in self.tree(oid)? {
            if !matches!(entry.mode.as_str(), "40000" | "100644" | "100755") {
                return Err(source_error("source_tree_link_or_mode"));
            }
            *count = count
                .checked_add(1)
                .ok_or_else(|| source_error("source_tree_entry_limit"))?;
            if *count > self.limits.max_entry_count {
                return Err(source_error("source_tree_entry_limit"));
            }
            let path = if prefix.is_empty() {
                entry.name.clone()
            } else {
                format!("{prefix}/{}", entry.name)
            };
            if entry.mode == "40000" {
                self.collect(&entry.oid, &path, entries, count)?;
            } else {
                if has_dangerous_extension(&path) {
                    return Err(ContainmentError::ForbiddenEntry { path });
                }
                if entries.insert(path, entry).is_some() {
                    return Err(source_error("source_tree_duplicate"));
                }
            }
        }
        Ok(())
    }
}

fn terminate_git(child: &mut Child) -> ContainmentResult<()> {
    if !matches!(child.try_wait(), Ok(Some(_)))
        && child.kill().is_err()
        && !matches!(child.try_wait(), Ok(Some(_)))
    {
        return Err(source_error("source_git_cancel_failed"));
    }
    child
        .wait()
        .map_err(|_| source_error("source_git_reap_failed"))?;
    Ok(())
}

struct TreeEntry {
    name: String,
    mode: String,
    oid: GitOid,
}

fn object_hex(algorithm: GitObjectAlgorithm, kind: &str, bytes: &[u8]) -> String {
    let header = format!("{kind} {}\0", bytes.len());
    match algorithm {
        GitObjectAlgorithm::Sha1 => {
            let mut hash = sha1::Sha1::new();
            hash.update(header.as_bytes());
            hash.update(bytes);
            format!("{:x}", hash.finalize())
        }
        GitObjectAlgorithm::Sha256 => {
            let mut hash = Sha256::new();
            hash.update(header.as_bytes());
            hash.update(bytes);
            format!("{:x}", hash.finalize())
        }
    }
}

fn regular_path(path: &Path, directory: bool) -> ContainmentResult<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        let metadata =
            fs::symlink_metadata(&current).map_err(|_| source_error("source_material_missing"))?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(source_error("source_material_link"));
            }
        }
        if metadata.file_type().is_symlink() {
            return Err(source_error("source_material_link"));
        }
    }
    let metadata =
        fs::symlink_metadata(path).map_err(|_| source_error("source_material_missing"))?;
    if if directory {
        !metadata.is_dir()
    } else {
        !metadata.is_file()
    } {
        return Err(source_error("source_material_type"));
    }
    Ok(())
}

pub(super) fn snapshot(
    locator: &Path,
    reference: &GitSourceTree,
    limits: ContainmentLimits,
    deadline: Instant,
) -> ContainmentResult<BTreeMap<String, Vec<u8>>> {
    reference
        .validate()
        .map_err(|_| source_error("source_reference_invalid"))?;
    if !locator.is_absolute() {
        return Err(source_error("source_locator_not_absolute"));
    }
    regular_path(locator, true)?;
    let locator = fs::canonicalize(locator).map_err(|_| source_error("source_material_missing"))?;
    let reader = GitReader {
        root: &locator,
        deadline,
        limits,
    };
    let format = reader.command(&["rev-parse", "--show-object-format=storage"], 16)?;
    let format = std::str::from_utf8(&format)
        .map_err(|_| source_error("source_object_format_invalid"))?
        .trim_end_matches(['\r', '\n']);
    if format
        != match reference.commit.algorithm {
            GitObjectAlgorithm::Sha1 => "sha1",
            GitObjectAlgorithm::Sha256 => "sha256",
        }
    {
        return Err(source_error("source_object_format_mismatch"));
    }
    let top = reader.command(&["rev-parse", "--show-toplevel"], 4096)?;
    let top = PathBuf::from(
        std::str::from_utf8(&top)
            .map_err(|_| source_error("source_repository_path_invalid"))?
            .trim_end_matches(['\r', '\n']),
    );
    regular_path(&top, true)?;
    let top = fs::canonicalize(top).map_err(|_| source_error("source_repository_missing"))?;
    if fs::canonicalize(top.join(&reference.bundle_path))
        .map_err(|_| source_error("source_bundle_path_missing"))?
        != locator
    {
        return Err(source_error("source_bundle_path_mismatch"));
    }
    let remote = reader.command(&["config", "--local", "--get", "remote.origin.url"], 1024)?;
    let remote = std::str::from_utf8(&remote)
        .map_err(|_| source_error("source_repository_invalid"))?
        .trim_end_matches(['\r', '\n']);
    if remote.strip_suffix(".git").unwrap_or(remote) != reference.repository {
        return Err(source_error("source_repository_mismatch"));
    }
    let commit = reader.object(&reference.commit, "commit")?;
    let line = commit
        .split(|b| *b == b'\n')
        .next()
        .ok_or_else(|| source_error("source_commit_invalid"))?;
    let root_hex = std::str::from_utf8(line)
        .ok()
        .and_then(|line| line.strip_prefix("tree "))
        .ok_or_else(|| source_error("source_commit_tree_missing"))?;
    let mut tree = GitOid {
        algorithm: reference.commit.algorithm,
        hex: root_hex.to_owned(),
    };
    tree.validate()
        .map_err(|_| source_error("source_commit_tree_invalid"))?;
    if reference.bundle_path != "." {
        for component in reference.bundle_path.split('/') {
            let entries = reader.tree(&tree)?;
            let child = entries
                .into_iter()
                .find(|entry| entry.name == component && entry.mode == "40000")
                .ok_or_else(|| source_error("source_commit_bundle_missing"))?;
            tree = child.oid;
        }
    }
    if tree != reference.tree {
        return Err(source_error("source_commit_tree_mismatch"));
    }
    let mut declared = BTreeMap::new();
    reader.collect(&tree, "", &mut declared, &mut 0)?;
    let mut snapshot = BTreeMap::new();
    let mut total = 0_u64;
    for (path, entry) in &declared {
        reader.check_time()?;
        let blob = reader.object(&entry.oid, "blob")?;
        let material = locator.join(path);
        regular_path(&material, false)?;
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(0x00200000);
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(0x20000);
        }
        let file = options
            .open(&material)
            .map_err(|_| source_error("source_material_open_failed"))?;
        let metadata = file
            .metadata()
            .map_err(|_| source_error("source_material_metadata_failed"))?;
        if !metadata.is_file() {
            return Err(source_error("source_material_type"));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(source_error("source_material_link"));
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if (metadata.permissions().mode() & 0o111 != 0) != (entry.mode == "100755") {
                return Err(source_error("source_material_mode_mismatch"));
            }
        }
        let limit = limits
            .max_entry_bytes
            .min(limits.max_total_decompressed_bytes.saturating_sub(total))
            .min(limits.max_resident_bytes_per_instance.saturating_sub(total));
        if metadata.len() > limit {
            return Err(source_error("source_material_size_limit"));
        }
        let mut bytes = Vec::new();
        file.take(limit.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| source_error("source_material_read_failed"))?;
        if bytes.len() as u64 > limit {
            return Err(source_error("source_material_size_limit"));
        }
        if blob.starts_with(b"version https://git-lfs.github.com/spec/") {
            let pointer = std::str::from_utf8(&blob)
                .map_err(|_| source_error("source_lfs_pointer_invalid"))?;
            let lines: Vec<_> = pointer.lines().collect();
            if lines.len() != 3 || lines[0] != "version https://git-lfs.github.com/spec/v1" {
                return Err(source_error("source_lfs_pointer_invalid"));
            }
            let hash = lines[1]
                .strip_prefix("oid sha256:")
                .ok_or_else(|| source_error("source_lfs_oid_invalid"))?;
            let size = lines[2]
                .strip_prefix("size ")
                .and_then(|size| size.parse::<u64>().ok())
                .ok_or_else(|| source_error("source_lfs_size_invalid"))?;
            if bytes == blob {
                return Err(source_error("source_lfs_not_materialized"));
            }
            if bytes.len() as u64 != size || Sha256Hash::digest(&bytes).to_string() != hash {
                return Err(source_error("source_lfs_material_mismatch"));
            }
        } else if object_hex(entry.oid.algorithm, "blob", &bytes) != entry.oid.hex {
            return Err(source_error("source_material_blob_mismatch"));
        }
        regular_path(&material, false)?;
        total = total
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| source_error("source_material_size_limit"))?;
        snapshot.insert(path.clone(), bytes);
    }
    reader.check_time()?;
    // Extra worktree files cannot become undeclared inputs to conversion.
    let mut pending = vec![locator.clone()];
    let mut visited = 0;
    let mut observed_files = BTreeSet::new();
    while let Some(directory) = pending.pop() {
        reader.check_time()?;
        regular_path(&directory, true)?;
        for entry in
            fs::read_dir(directory).map_err(|_| source_error("source_directory_read_failed"))?
        {
            let path = entry
                .map_err(|_| source_error("source_directory_read_failed"))?
                .path();
            if path == top.join(".git") {
                continue;
            }
            visited += 1;
            if visited > limits.max_entry_count {
                return Err(source_error("source_tree_entry_limit"));
            }
            let metadata =
                fs::symlink_metadata(&path).map_err(|_| source_error("source_material_missing"))?;
            if metadata.is_dir() {
                regular_path(&path, true)?;
                if path.components().count() > locator.components().count() + 64 {
                    return Err(source_error("source_tree_depth_limit"));
                }
                pending.push(path);
            } else {
                regular_path(&path, false)?;
                let relative = path
                    .strip_prefix(&locator)
                    .map_err(|_| source_error("source_path_escape"))?
                    .to_str()
                    .ok_or_else(|| source_error("source_path_encoding"))?
                    .replace('\\', "/");
                if !declared.contains_key(&relative) {
                    return Err(source_error("source_undeclared_material"));
                }
                if !observed_files.insert(relative) {
                    return Err(source_error("source_material_alias"));
                }
            }
        }
    }
    if observed_files.len() != declared.len() {
        return Err(source_error("source_material_set_mismatch"));
    }
    Ok(snapshot)
}

/// The source directory uses the existing control/resource layout. Only original
/// operation declarations and their explicit dependencies enter pure conversion.
pub(super) fn compile(
    mut entries: BTreeMap<String, Vec<u8>>,
    limits: ContainmentLimits,
    deadline: Instant,
) -> ContainmentResult<(MemoryPackage, Value)> {
    use source::{Bundle, ConversionFiles, OperationConverter, SourceFile, SourceRead};
    if Instant::now() >= deadline {
        return Err(source_error("source_tree_deadline"));
    }
    let control: LabControl = read_json_entry(&entries, "control.json")?;
    let resources = read_json_value_entry(&entries, "resources/operations/resources.json")?;
    let mut bundles = Vec::new();
    for (path, bytes) in &entries {
        if let Some(task) = path
            .strip_prefix("resources/operations/")
            .and_then(|path| path.strip_suffix("/task.json"))
        {
            if task.contains('/') || !safe_source_path(task) {
                return Err(source_error("source_operation_path_invalid"));
            }
            let data = serde_json::from_slice(bytes)
                .map_err(|_| source_error("source_operation_invalid"))?;
            bundles.push(Bundle {
                task_id: task.to_owned(),
                dir: PathBuf::from(format!("resources/operations/{task}")),
                data,
            });
        }
    }
    let entry = bundles
        .iter()
        .find(|bundle| bundle.task_id == control.entry_task_id)
        .ok_or_else(|| source_error("source_entry_operation_missing"))?;
    let required = |key: &str| {
        entry
            .data
            .get(key)
            .cloned()
            .ok_or_else(|| source_error("source_conversion_metadata_missing"))
    };
    let locale = entry
        .data
        .get("locale")
        .and_then(Value::as_str)
        .ok_or_else(|| source_error("source_locale_missing"))?;
    let stem = format!("{}.{}", control.game, control.server);
    let projection_path = format!("resources/navigation/{stem}.projection.json");
    let files = ConversionFiles {
        files: Arc::new(
            source::source_file_requests(&bundles)
                .into_iter()
                .map(|(path, read)| {
                    let bytes = path
                        .to_str()
                        .and_then(|path| entries.get(&path.replace('\\', "/")));
                    let file = match bytes {
                        Some(bytes) => SourceFile {
                            is_file: true,
                            length: Ok(bytes.len() as u64),
                            bytes: match read {
                                SourceRead::Metadata => {
                                    Err("metadata-only conversion input".to_owned())
                                }
                                SourceRead::BoundedBytes(limit) if bytes.len() as u64 > limit => {
                                    Err("conversion input exceeds declared limit".to_owned())
                                }
                                _ => Ok(bytes.clone()),
                            },
                        },
                        None => SourceFile {
                            is_file: false,
                            length: Err("source dependency missing".to_owned()),
                            bytes: Err("source dependency missing".to_owned()),
                        },
                    };
                    (path, file)
                })
                .collect(),
        ),
        projection_bytes: Ok(entries.get(&projection_path).cloned()),
        projection_exists: Ok(entries.contains_key(&projection_path)),
    };
    let converter = OperationConverter {
        root: PathBuf::from("resources"),
        game: source::canonical_game(&control.game)
            .map_err(|_| source_error("source_game_invalid"))?,
        server: source::canonical_server(&control.server)
            .map_err(|_| source_error("source_server_invalid"))?,
        locale: source::canonical_locale(locale)
            .map_err(|_| source_error("source_locale_invalid"))?,
        coordinate_space: required("coordinate_space")?,
        defaults: required("defaults")?,
        resource_ids: source::resource_ids(&resources)
            .map_err(|_| source_error("source_resources_invalid"))?,
        existing_navigation: Some(
            serde_json::json!({"control_points": resources.get("control_points").cloned().unwrap_or_else(|| serde_json::json!([]))}),
        ),
        bundles,
        maa_task_overlays: Default::default(),
    };
    converter
        .validate_bundles(&files)
        .map_err(|error| ContainmentError::PackParse {
            path: "resources/operations".to_owned(),
            message: error.to_string(),
        })?;
    let outputs = converter
        .build_all(&files)
        .map_err(|error| ContainmentError::PackParse {
            path: "resources/operations".to_owned(),
            message: error.to_string(),
        })?;
    let operation = converter
        .canonical_task(&control.entry_task_id)
        .map_err(|error| ContainmentError::PackParse {
            path: format!("resources/operations/{}/task.json", control.entry_task_id),
            message: error.to_string(),
        })?;
    let operation_bytes = serde_json::to_vec(&operation)
        .map_err(|_| source_error("source_conversion_encode_failed"))?
        .len() as u64;
    if operation_bytes > limits.max_entry_bytes {
        return Err(source_error("source_compiled_entry_limit"));
    }
    drop(files);
    let generated = [
        (
            format!("resources/recognition/{stem}.pack.json"),
            outputs.pack,
        ),
        (
            format!("resources/recognition/{stem}.pages.json"),
            outputs.pages,
        ),
        (
            format!("resources/navigation/{stem}.navigation.json"),
            outputs.navigation,
        ),
        (
            "resources/operations/operations.index.json".to_owned(),
            outputs.index,
        ),
        (
            "resources/operations/operations.primitives.json".to_owned(),
            outputs.primitives,
        ),
    ];
    for (path, value) in generated {
        if Instant::now() >= deadline {
            return Err(source_error("source_tree_deadline"));
        }
        if entries.contains_key(&path) {
            return Err(source_error("source_contains_derived_output"));
        }
        entries.insert(
            path,
            serde_json::to_vec(&value)
                .map_err(|_| source_error("source_conversion_encode_failed"))?,
        );
    }
    if entries.contains_key("resources/manifest.json") {
        return Err(source_error("source_contains_derived_output"));
    }
    // This in-memory index preserves the existing projection dependency guards;
    // the admitted package identity remains the verified Git reference.
    let hashes: BTreeMap<_, _> = entries
        .iter()
        .filter_map(|(path, bytes)| {
            path.strip_prefix("resources/")
                .map(|path| (path.to_owned(), Sha256Hash::digest(bytes).to_string()))
        })
        .collect();
    entries.insert(
        "resources/manifest.json".to_owned(),
        serde_json::to_vec(
            &serde_json::json!({"entry_task_id":control.entry_task_id,"hashes":hashes}),
        )
        .map_err(|_| source_error("source_conversion_encode_failed"))?,
    );
    // Projection declarations remain the exact verified source bytes; build_all
    // has already checked them against the generated catalog.
    let mut resident_bytes = operation_bytes;
    for bytes in entries.values() {
        if bytes.len() as u64 > limits.max_entry_bytes {
            return Err(source_error("source_compiled_entry_limit"));
        }
        resident_bytes = resident_bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| source_error("source_compiled_size_limit"))?;
    }
    if entries.len() > limits.max_entry_count
        || resident_bytes > limits.max_total_decompressed_bytes
        || resident_bytes > limits.max_resident_bytes_per_instance
    {
        return Err(source_error("source_compiled_size_limit"));
    }
    Ok((
        MemoryPackage {
            entry_count: entries.len(),
            entries,
            resident_bytes,
        },
        operation,
    ))
}
