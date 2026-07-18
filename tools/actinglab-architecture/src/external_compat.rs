// SPDX-License-Identifier: AGPL-3.0-only

//! Exact provenance, capability, and I/O validation for external compatibility data.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const EXTERNAL_COMPAT_SCHEMA_VERSION: &str = "actingcommand.external-compat.v2";
pub const EXTERNAL_COMPAT_MANIFEST_PATH: &str = "tests/external-compat/manifest-v2.toml";
const EXTERNAL_COMPAT_DATA_ROOT: &str = "tests/external-compat/data/";
const MAX_EXTERNAL_COMPAT_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ExternalCompatManifest {
    schema_version: String,
    #[serde(default)]
    entry: Vec<ExternalCompatEntry>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ExternalCompatEntry {
    id: String,
    path: String,
    sha256: String,
    purpose: String,
    allowed_scope: Vec<ExternalCompatScope>,
    source: ExternalCompatSource,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum ExternalCompatScope {
    #[serde(rename = "parser.generated")]
    ParserGenerated,
    #[serde(rename = "parser.schema")]
    ParserSchema,
}

impl ExternalCompatScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::ParserGenerated => "parser.generated",
            Self::ParserSchema => "parser.schema",
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ExternalCompatSource {
    Upstream {
        repository_url: String,
        commit_sha: String,
        upstream_path: String,
        sha256: String,
    },
    Generated {
        generator_id: String,
        #[serde(default)]
        parameters: BTreeMap<String, GeneratedParameter>,
        input: Vec<GeneratedInput>,
    },
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
enum GeneratedParameter {
    Boolean(bool),
    Integer(i64),
    Identifier(String),
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct GeneratedInput {
    path: String,
    sha256: String,
}

struct GeneratedReplayRequest<'a> {
    parameters: &'a BTreeMap<String, GeneratedParameter>,
    inputs: &'a [(String, Vec<u8>)],
}

type ReplayGenerator = for<'a> fn(&GeneratedReplayRequest<'a>) -> Result<Vec<u8>, String>;
type ResolveGeneratorRevision = fn(&Path, &str, &[u8]) -> Result<String, String>;

#[derive(Clone, Copy)]
struct RegisteredGenerator {
    id: &'static str,
    source_path: &'static str,
    replay: ReplayGenerator,
}

fn replay_identity(request: &GeneratedReplayRequest<'_>) -> Result<Vec<u8>, String> {
    if !request.parameters.is_empty() || request.inputs.len() != 1 {
        return Err("generator.identity requires one input and no parameters".to_string());
    }
    Ok(request.inputs[0].1.clone())
}

const REGISTERED_GENERATORS: &[RegisteredGenerator] = &[RegisteredGenerator {
    id: "generator.identity",
    source_path: "tools/actinglab-architecture/src/external_compat.rs",
    replay: replay_identity,
}];

fn parse_external_compat_manifest(source: &str) -> Result<ExternalCompatManifest, String> {
    toml::from_str(source).map_err(|error| format!("invalid external-compat manifest: {error}"))
}

fn load_external_compat_manifest(path: &Path) -> Result<ExternalCompatManifest, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    parse_external_compat_manifest(&source)
}

struct ExternalCompatReader<'a> {
    root: PathBuf,
    manifest: ExternalCompatManifest,
    generators: &'a [RegisteredGenerator],
    revision_resolver: ResolveGeneratorRevision,
}

impl ExternalCompatReader<'static> {
    fn open(root: &Path) -> Result<Self, String> {
        Self::open_with(root, REGISTERED_GENERATORS, resolve_generator_revision)
    }
}

impl<'a> ExternalCompatReader<'a> {
    /// Loading the catalog parses and authorizes metadata only; entry bytes are
    /// opened later, after a typed use has selected its private capability.
    fn open_with(
        root: &Path,
        generators: &'a [RegisteredGenerator],
        revision_resolver: ResolveGeneratorRevision,
    ) -> Result<Self, String> {
        let manifest = load_external_compat_manifest(&root.join(EXTERNAL_COMPAT_MANIFEST_PATH))?;
        validate_manifest_structure(&manifest, generators)?;
        Ok(Self {
            root: root.to_path_buf(),
            manifest,
            generators,
            revision_resolver,
        })
    }

    #[cfg(test)]
    fn read_parser_generated(&self, path: &str) -> Result<Vec<u8>, String> {
        self.read_scoped(ExternalCompatScope::ParserGenerated, path)
    }

    #[cfg(test)]
    fn read_parser_schema(&self, path: &str) -> Result<Vec<u8>, String> {
        self.read_scoped(ExternalCompatScope::ParserSchema, path)
    }

    fn read_scoped(&self, scope: ExternalCompatScope, path: &str) -> Result<Vec<u8>, String> {
        let entry = self.authorized_entry(scope, path)?;
        read_hashed_workspace_file(&self.root, &entry.id, "entry", path, &entry.sha256)
    }

    #[cfg(test)]
    fn read_scoped_with_hook<F>(
        &self,
        scope: ExternalCompatScope,
        path: &str,
        after_open: F,
    ) -> Result<Vec<u8>, String>
    where
        F: FnOnce() -> Result<(), String>,
    {
        let entry = self.authorized_entry(scope, path)?;
        read_hashed_workspace_file_with_hook(
            &self.root,
            &entry.id,
            "entry",
            path,
            &entry.sha256,
            after_open,
        )
    }

    fn authorized_entry(
        &self,
        scope: ExternalCompatScope,
        path: &str,
    ) -> Result<&ExternalCompatEntry, String> {
        let entry = self
            .manifest
            .entry
            .iter()
            .find(|entry| entry.path == path)
            .ok_or_else(|| format!("external-compat access uses unregistered path {path}"))?;
        if !entry.allowed_scope.contains(&scope) {
            return Err(format!(
                "external-compat entry {} does not allow scope {}",
                entry.id,
                scope.as_str()
            ));
        }
        Ok(entry)
    }

    fn audit_all(&self) -> Result<(), String> {
        let mut errors = Vec::new();
        let registered_paths = self
            .manifest
            .entry
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<HashSet<_>>();

        match external_compat_files(&self.root) {
            Ok(files) => {
                for file in &files {
                    if !registered_paths.contains(file.as_str()) {
                        errors.push(format!("unregistered external-compat file {file}"));
                    }
                }
                for path in &registered_paths {
                    if !files.iter().any(|file| file == path) {
                        errors.push(format!("registered external-compat file is missing {path}"));
                    }
                }
            }
            Err(error) => errors.push(error),
        }

        for entry in &self.manifest.entry {
            match self.read_scoped(entry.allowed_scope[0], &entry.path) {
                Ok(output) => {
                    if let Err(error) = self.audit_source(entry, &output) {
                        errors.push(error);
                    }
                }
                Err(error) => errors.push(error),
            }
        }
        finish_errors(errors)
    }

    fn audit_source(&self, entry: &ExternalCompatEntry, output: &[u8]) -> Result<(), String> {
        let ExternalCompatSource::Generated {
            generator_id,
            parameters,
            input,
        } = &entry.source
        else {
            return Ok(());
        };
        let generator = registered_generator(self.generators, generator_id).ok_or_else(|| {
            format!(
                "entry {} references unregistered generator {generator_id}",
                entry.id
            )
        })?;
        let generator_source =
            read_workspace_file(&self.root, &entry.id, "generator", generator.source_path)?;
        let revision =
            (self.revision_resolver)(&self.root, generator.source_path, &generator_source)?;
        if !is_git_sha(&revision) {
            return Err(format!(
                "entry {} generator registry resolved an invalid Git revision",
                entry.id
            ));
        }

        let mut inputs = Vec::with_capacity(input.len());
        for item in input {
            let bytes = read_hashed_workspace_file(
                &self.root,
                &entry.id,
                "input",
                &item.path,
                &item.sha256,
            )?;
            inputs.push((item.path.clone(), bytes));
        }
        let replayed = (generator.replay)(&GeneratedReplayRequest {
            parameters,
            inputs: &inputs,
        })
        .map_err(|error| format!("entry {} generator replay failed: {error}", entry.id))?;
        if replayed != output {
            return Err(format!(
                "entry {} generated output does not match deterministic replay",
                entry.id
            ));
        }
        Ok(())
    }

    fn registered_paths(&self) -> impl Iterator<Item = &str> {
        self.manifest.entry.iter().map(|entry| entry.path.as_str())
    }
}

/// Performs the full isolated audit. Raw scopes and raw-byte readers are not
/// public capabilities.
///
/// ~~~compile_fail
/// use actingcommand_actinglab_architecture::external_compat::ExternalCompatScope;
/// let _ = ExternalCompatScope::ParserGenerated;
/// ~~~
pub fn audit_external_compat(root: &Path) -> Result<(), String> {
    ExternalCompatReader::open(root)?.audit_all()
}

pub(crate) fn validated_external_compat_paths(root: &Path) -> Result<Vec<String>, String> {
    let reader = ExternalCompatReader::open(root)?;
    reader.audit_all()?;
    Ok(reader.registered_paths().map(str::to_string).collect())
}

fn validate_manifest_structure(
    manifest: &ExternalCompatManifest,
    generators: &[RegisteredGenerator],
) -> Result<(), String> {
    let mut errors = Vec::new();
    if manifest.schema_version != EXTERNAL_COMPAT_SCHEMA_VERSION {
        errors.push(format!(
            "unsupported schema_version {}; expected {EXTERNAL_COMPAT_SCHEMA_VERSION}",
            manifest.schema_version
        ));
    }
    validate_generator_registry(generators, &mut errors);

    let mut ids = HashSet::new();
    let mut paths = HashSet::new();
    let mut previous_id = None;
    for entry in &manifest.entry {
        if !is_registry_id(&entry.id) {
            errors.push(format!("entry has invalid id {}", entry.id));
        }
        if !ids.insert(entry.id.as_str()) {
            errors.push(format!("duplicate external-compat id {}", entry.id));
        }
        if previous_id.is_some_and(|previous: &str| previous >= entry.id.as_str()) {
            errors.push(format!(
                "external-compat ids are not strictly sorted at {}",
                entry.id
            ));
        }
        previous_id = Some(entry.id.as_str());
        if let Err(error) = validate_data_path(&entry.path) {
            errors.push(format!("entry {} {error}", entry.id));
        }
        if !paths.insert(entry.path.as_str()) {
            errors.push(format!("duplicate external-compat path {}", entry.path));
        }
        if !is_sha256(&entry.sha256) {
            errors.push(format!("entry {} has invalid sha256", entry.id));
        }
        if entry.purpose.trim().is_empty() {
            errors.push(format!("entry {} has empty purpose", entry.id));
        }
        validate_scopes(entry, &mut errors);
        validate_source_structure(entry, generators, &mut errors);
    }
    finish_errors(errors)
}

fn validate_generator_registry(generators: &[RegisteredGenerator], errors: &mut Vec<String>) {
    let mut ids = HashSet::new();
    let mut source_paths = HashSet::new();
    for generator in generators {
        if !is_registry_id(generator.id) || !ids.insert(generator.id) {
            errors.push(format!(
                "generator registry has invalid or duplicate id {}",
                generator.id
            ));
        }
        if let Err(error) = validate_repo_path(generator.source_path) {
            errors.push(format!("generator {} source {error}", generator.id));
        }
        if !source_paths.insert(generator.source_path) {
            errors.push(format!(
                "generator registry repeats source path {}",
                generator.source_path
            ));
        }
    }
}

fn validate_scopes(entry: &ExternalCompatEntry, errors: &mut Vec<String>) {
    if entry.allowed_scope.is_empty() {
        errors.push(format!("entry {} has no allowed_scope", entry.id));
        return;
    }
    let mut seen = HashSet::new();
    let mut previous = None;
    for scope in &entry.allowed_scope {
        let scope = scope.as_str();
        if !seen.insert(scope) {
            errors.push(format!("entry {} repeats scope {scope}", entry.id));
        }
        if previous.is_some_and(|left: &str| left >= scope) {
            errors.push(format!(
                "entry {} scopes are not strictly sorted at {scope}",
                entry.id
            ));
        }
        previous = Some(scope);
    }
}

fn validate_source_structure(
    entry: &ExternalCompatEntry,
    generators: &[RegisteredGenerator],
    errors: &mut Vec<String>,
) {
    match &entry.source {
        ExternalCompatSource::Upstream {
            repository_url,
            commit_sha,
            upstream_path,
            sha256,
        } => {
            if !is_pinned_repository_url(repository_url) {
                errors.push(format!(
                    "entry {} has invalid upstream repository URL",
                    entry.id
                ));
            }
            if !is_git_sha(commit_sha) {
                errors.push(format!(
                    "entry {} has invalid upstream commit SHA",
                    entry.id
                ));
            }
            if let Err(error) = validate_repo_path(upstream_path) {
                errors.push(format!("entry {} upstream path {error}", entry.id));
            }
            if !is_sha256(sha256) || sha256 != &entry.sha256 {
                errors.push(format!(
                    "entry {} upstream raw-byte hash does not match entry sha256",
                    entry.id
                ));
            }
        }
        ExternalCompatSource::Generated {
            generator_id,
            parameters,
            input,
        } => {
            let generator = registered_generator(generators, generator_id);
            if generator.is_none() {
                errors.push(format!(
                    "entry {} references unregistered generator {generator_id}",
                    entry.id
                ));
            }
            for (key, value) in parameters {
                let value_is_valid = match value {
                    GeneratedParameter::Boolean(_) | GeneratedParameter::Integer(_) => true,
                    GeneratedParameter::Identifier(value) => is_registry_id(value),
                };
                if !is_registry_id(key) || !value_is_valid {
                    errors.push(format!(
                        "entry {} has invalid generator parameter {key}",
                        entry.id
                    ));
                }
            }
            if input.is_empty() {
                errors.push(format!("entry {} has no generated inputs", entry.id));
            }

            let mut roles = BTreeMap::from([(entry.path.as_str(), "output")]);
            if let Some(generator) = generator {
                insert_provenance_role(
                    &entry.id,
                    &mut roles,
                    generator.source_path,
                    "generator",
                    errors,
                );
            }
            let mut previous = None;
            for item in input {
                if previous.is_some_and(|left: &str| left >= item.path.as_str()) {
                    errors.push(format!(
                        "entry {} generated inputs are not strictly sorted at {}",
                        entry.id, item.path
                    ));
                }
                previous = Some(item.path.as_str());
                if let Err(error) = validate_repo_path(&item.path) {
                    errors.push(format!("entry {} input {error}", entry.id));
                }
                if !is_sha256(&item.sha256) {
                    errors.push(format!("entry {} has invalid input sha256", entry.id));
                }
                insert_provenance_role(&entry.id, &mut roles, &item.path, "input", errors);
            }
        }
    }
}

fn insert_provenance_role<'a>(
    entry_id: &str,
    roles: &mut BTreeMap<&'a str, &'static str>,
    path: &'a str,
    role: &'static str,
    errors: &mut Vec<String>,
) {
    if let Some(previous) = roles.insert(path, role) {
        errors.push(format!(
            "entry {entry_id} provenance_cycle: {path} is both {previous} and {role}"
        ));
    }
}

fn registered_generator<'a>(
    generators: &'a [RegisteredGenerator],
    id: &str,
) -> Option<&'a RegisteredGenerator> {
    generators.iter().find(|generator| generator.id == id)
}

fn resolve_generator_revision(root: &Path, path: &str, source: &[u8]) -> Result<String, String> {
    let tracked = git_output(root, &["ls-files", "--error-unmatch", "--", path])?;
    if tracked.trim().is_empty() {
        return Err(format!(
            "registered generator source is not tracked: {path}"
        ));
    }
    let revision = git_output(root, &["rev-list", "-n", "1", "HEAD", "--", path])?;
    if !is_git_sha(revision.trim()) {
        return Err(format!(
            "registered generator source has no trusted Git revision: {path}"
        ));
    }
    let committed = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["show", &format!("HEAD:{path}")])
        .output()
        .map_err(|error| format!("failed to read committed generator source {path}: {error}"))?;
    if !committed.status.success() {
        return Err(format!(
            "failed to read committed generator source {path}: {}",
            String::from_utf8_lossy(&committed.stderr).trim()
        ));
    }
    if committed.stdout != source {
        return Err(format!(
            "registered generator source differs from its trusted Git revision: {path}"
        ));
    }
    Ok(revision.trim().to_string())
}

fn git_output(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|error| format!("failed to run git {}: {error}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("git {} returned non-UTF-8 output: {error}", args.join(" ")))
}

fn read_hashed_workspace_file(
    root: &Path,
    entry_id: &str,
    role: &str,
    path: &str,
    expected: &str,
) -> Result<Vec<u8>, String> {
    read_hashed_workspace_file_with_hook(root, entry_id, role, path, expected, || Ok(()))
}

fn read_hashed_workspace_file_with_hook<F>(
    root: &Path,
    entry_id: &str,
    role: &str,
    path: &str,
    expected: &str,
    after_open: F,
) -> Result<Vec<u8>, String>
where
    F: FnOnce() -> Result<(), String>,
{
    if !is_sha256(expected) {
        return Err(format!("entry {entry_id} has invalid {role} sha256"));
    }
    let mut file = open_verified_beneath(root, path)
        .map_err(|error| format!("entry {entry_id} {role} {error}"))?;
    after_open()?;
    let bytes =
        read_bounded(&mut file).map_err(|error| format!("entry {entry_id} {role} {error}"))?;
    if format!("{:x}", Sha256::digest(&bytes)) != expected {
        return Err(format!("entry {entry_id} {role} content hash drifted"));
    }
    Ok(bytes)
}

fn read_workspace_file(
    root: &Path,
    entry_id: &str,
    role: &str,
    path: &str,
) -> Result<Vec<u8>, String> {
    let mut file = open_verified_beneath(root, path)
        .map_err(|error| format!("entry {entry_id} {role} {error}"))?;
    read_bounded(&mut file).map_err(|error| format!("entry {entry_id} {role} {error}"))
}

fn read_bounded(file: &mut File) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    file.take(MAX_EXTERNAL_COMPAT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read verified file handle: {error}"))?;
    if bytes.len() as u64 > MAX_EXTERNAL_COMPAT_BYTES {
        return Err(format!(
            "verified file exceeds {MAX_EXTERNAL_COMPAT_BYTES} byte limit"
        ));
    }
    Ok(bytes)
}

fn external_compat_files(root: &Path) -> Result<Vec<String>, String> {
    let data_root = root.join("tests/external-compat");
    let mut files = Vec::new();
    collect_regular_files(root, &data_root, &mut files)?;
    files.retain(|path| path != EXTERNAL_COMPAT_MANIFEST_PATH);
    files.sort();
    Ok(files)
}

fn collect_regular_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<String>,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(directory)
        .map_err(|error| format!("failed to inspect {}: {error}", directory.display()))?;
    if is_link_or_reparse(&metadata) {
        return Err(format!(
            "external-compat path is a link or reparse point: {}",
            directory.display()
        ));
    }
    for entry in fs::read_dir(directory)
        .map_err(|error| format!("failed to read {}: {error}", directory.display()))?
    {
        let path = entry
            .map_err(|error| format!("failed to read external-compat entry: {error}"))?
            .path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
        if is_link_or_reparse(&metadata) {
            return Err(format!(
                "external-compat path is a link or reparse point: {}",
                path.display()
            ));
        }
        if metadata.is_dir() {
            collect_regular_files(root, &path, files)?;
        } else if metadata.is_file() {
            files.push(normalize_workspace_path(root, &path)?);
        }
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn open_verified_beneath(root: &Path, relative: &str) -> Result<File, String> {
    use std::ffi::{CString, c_char, c_int};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    unsafe extern "C" {
        fn openat(directory: c_int, path: *const c_char, flags: c_int, mode: u32) -> c_int;
    }

    const O_RDONLY: c_int = 0;
    const O_DIRECTORY: c_int = 0x0001_0000;
    const O_NOFOLLOW: c_int = 0x0002_0000;
    const O_CLOEXEC: c_int = 0x0008_0000;

    validate_repo_path(relative)?;
    let root_path = CString::new(root.as_os_str().as_bytes())
        .map_err(|_| format!("workspace root contains a null byte: {}", root.display()))?;
    let root_descriptor = unsafe {
        openat(
            -100,
            root_path.as_ptr(),
            O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
            0,
        )
    };
    if root_descriptor < 0 {
        return Err(format!(
            "failed to open workspace root {}: {}",
            root.display(),
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: openat returned a new owned descriptor on success.
    let mut current = unsafe { File::from_raw_fd(root_descriptor) };
    if !current
        .metadata()
        .map_err(|error| format!("failed to inspect workspace root handle: {error}"))?
        .is_dir()
    {
        return Err(format!(
            "workspace root is not a directory: {}",
            root.display()
        ));
    }
    let components = Path::new(relative).components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(part) = component else {
            return Err(format!("has unsafe path {relative}"));
        };
        let last = index + 1 == components.len();
        let part = CString::new(part.as_bytes())
            .map_err(|_| format!("verified path contains a null byte: {relative}"))?;
        let mut flags = O_RDONLY | O_NOFOLLOW | O_CLOEXEC;
        if !last {
            flags |= O_DIRECTORY;
        }
        // SAFETY: the directory descriptor and null-terminated component remain live for the call.
        let descriptor = unsafe { openat(current.as_raw_fd(), part.as_ptr(), flags, 0) };
        if descriptor < 0 {
            return Err(format!(
                "failed to open verified path {relative}: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: openat returned a new owned descriptor on success.
        current = unsafe { File::from_raw_fd(descriptor) };
        let metadata = current
            .metadata()
            .map_err(|error| format!("failed to inspect verified handle {relative}: {error}"))?;
        if (!last && !metadata.is_dir()) || (last && !metadata.is_file()) {
            return Err(format!("path is not a regular file: {relative}"));
        }
    }
    Ok(current)
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
fn open_verified_beneath(_root: &Path, _relative: &str) -> Result<File, String> {
    Err("external-compat verified handle I/O is unsupported on this Unix target".to_string())
}

#[cfg(windows)]
fn open_verified_beneath(root: &Path, relative: &str) -> Result<File, String> {
    use std::fs::OpenOptions;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;

    validate_repo_path(relative)?;
    let open = |path: &Path, directory: bool| -> Result<File, String> {
        let mut options = OpenOptions::new();
        options
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(
                FILE_FLAG_OPEN_REPARSE_POINT
                    | if directory {
                        FILE_FLAG_BACKUP_SEMANTICS
                    } else {
                        0
                    },
            );
        options
            .open(path)
            .map_err(|error| format!("failed to open {}: {error}", path.display()))
    };

    let root_file = open(root, true)?;
    let root_metadata = root_file
        .metadata()
        .map_err(|error| format!("failed to inspect workspace root handle: {error}"))?;
    if !root_metadata.is_dir()
        || root_metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(format!(
            "workspace root is not a regular directory handle: {}",
            root.display()
        ));
    }

    let candidate = root.join(relative);
    let file = open(&candidate, false)?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to inspect verified handle {relative}: {error}"))?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(format!(
            "path is not a regular non-reparse file: {relative}"
        ));
    }
    let root_final = windows_final_path(&root_file)?;
    let file_final = windows_final_path(&file)?;
    if !windows_path_is_strictly_beneath(&root_final, &file_final) {
        return Err(format!("path escapes workspace root: {relative}"));
    }
    Ok(file)
}

#[cfg(windows)]
fn windows_final_path(file: &File) -> Result<PathBuf, String> {
    use std::ffi::{OsString, c_void};
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFinalPathNameByHandleW(
            handle: *mut c_void,
            path: *mut u16,
            length: u32,
            flags: u32,
        ) -> u32;
    }

    let mut buffer = vec![0u16; 32_768];
    // SAFETY: the file handle is live and buffer is writable for its reported length.
    let mut written = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            0,
        )
    };
    if written == 0 {
        return Err(format!(
            "failed to resolve verified file handle: {}",
            std::io::Error::last_os_error()
        ));
    }
    if written as usize >= buffer.len() {
        buffer.resize(written as usize + 1, 0);
        // SAFETY: the resized buffer is writable and the file handle remains live.
        written = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                0,
            )
        };
        if written == 0 || written as usize >= buffer.len() {
            return Err("failed to resolve complete verified file handle path".to_string());
        }
    }
    Ok(OsString::from_wide(&buffer[..written as usize]).into())
}

#[cfg(windows)]
fn windows_path_is_strictly_beneath(root: &Path, child: &Path) -> bool {
    let root = root.components().collect::<Vec<_>>();
    let child = child.components().collect::<Vec<_>>();
    child.len() > root.len()
        && root.iter().zip(&child).all(|(left, right)| {
            left.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
        })
}

#[cfg(not(any(unix, windows)))]
fn open_verified_beneath(_root: &Path, _relative: &str) -> Result<File, String> {
    Err("external-compat verified handle I/O is unsupported on this platform".to_string())
}

fn validate_data_path(path: &str) -> Result<(), String> {
    validate_repo_path(path)?;
    if !path.starts_with(EXTERNAL_COMPAT_DATA_ROOT) {
        return Err(format!(
            "path must be under {EXTERNAL_COMPAT_DATA_ROOT}: {path}"
        ));
    }
    Ok(())
}

fn validate_repo_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.contains(['\\', '*', '?'])
        || path.starts_with('/')
        || path.ends_with('/')
        || Path::new(path)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("has invalid exact path {path}"));
    }
    Ok(())
}

fn normalize_workspace_path(root: &Path, path: &Path) -> Result<String, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| format!("{} escaped workspace root", path.display()))?;
    let value = relative
        .to_str()
        .ok_or_else(|| format!("path {} is not UTF-8", relative.display()))?
        .replace('\\', "/");
    validate_repo_path(&value)?;
    Ok(value)
}

fn is_pinned_repository_url(value: &str) -> bool {
    value.starts_with("https://github.com/")
        && !value.contains([' ', '\t', '\n', '\r', '?', '#', '*'])
        && value.trim_end_matches('/').split('/').count() == 5
}

fn is_registry_id(value: &str) -> bool {
    !value.is_empty()
        && value.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        })
}

fn is_git_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn finish_errors(mut errors: Vec<String>) -> Result<(), String> {
    if errors.is_empty() {
        Ok(())
    } else {
        errors.sort();
        errors.dedup();
        Err(errors.join("\n"))
    }
}

#[cfg(windows)]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn replay_first_input(request: &GeneratedReplayRequest<'_>) -> Result<Vec<u8>, String> {
        let _parameters = request.parameters;
        request
            .inputs
            .first()
            .map(|(_, bytes)| bytes.clone())
            .ok_or_else(|| "missing replay input".to_string())
    }

    fn fixture_revision(_root: &Path, path: &str, source: &[u8]) -> Result<String, String> {
        if path != "tools/generate.rs" || source != b"fn main() {}\n" {
            return Err("generator source drifted from trusted fixture revision".to_string());
        }
        Ok("2".repeat(40))
    }

    fn fixture_generator() -> RegisteredGenerator {
        RegisteredGenerator {
            id: "generator.fixture",
            source_path: "tools/generate.rs",
            replay: replay_first_input,
        }
    }

    #[test]
    fn empty_containment_zone_is_valid() {
        let root = fixture_root("empty");
        write_manifest(&root);
        audit_external_compat(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn upstream_entry_rejects_unregistered_file_and_hash_drift() {
        let root = fixture_root("upstream");
        let path = "tests/external-compat/data/sample.json";
        write_file(&root, path, b"{\"value\":1}\n");
        let hash = sha256_file(&root.join(path));
        let manifest = upstream_manifest(path, &hash, ExternalCompatScope::ParserSchema);
        write_parsed_manifest(&root, &manifest);
        ExternalCompatReader::open(&root)
            .unwrap()
            .audit_all()
            .unwrap();

        write_file(&root, "tests/external-compat/unregistered.json", b"{}\n");
        let error = ExternalCompatReader::open(&root)
            .unwrap()
            .audit_all()
            .unwrap_err();
        assert!(error.contains("unregistered external-compat file"));
        fs::remove_file(root.join("tests/external-compat/unregistered.json")).unwrap();

        write_file(&root, path, b"{\"value\":2}\n");
        let error = ExternalCompatReader::open(&root)
            .unwrap()
            .audit_all()
            .unwrap_err();
        assert!(error.contains("content hash drifted"));
        assert!(!error.contains(&sha256_file(&root.join(path))));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generated_entry_uses_registered_replay_and_trusted_revision() {
        let root = fixture_root("generated");
        let output = "tests/external-compat/data/generated.json";
        let input = "inputs/source.json";
        write_file(&root, output, b"{\"source\":1}\n");
        write_file(&root, "tools/generate.rs", b"fn main() {}\n");
        write_file(&root, input, b"{\"source\":1}\n");
        let manifest = generated_manifest(&root, output, input, "generator.fixture");
        write_parsed_manifest(&root, &manifest);
        let generators = [fixture_generator()];
        ExternalCompatReader::open_with(&root, &generators, fixture_revision)
            .unwrap()
            .audit_all()
            .unwrap();

        write_file(&root, "tools/generate.rs", b"fn main() { panic!() }\n");
        let error = ExternalCompatReader::open_with(&root, &generators, fixture_revision)
            .unwrap()
            .audit_all()
            .unwrap_err();
        assert!(error.contains("trusted fixture revision"));
        write_file(&root, "tools/generate.rs", b"fn main() {}\n");

        write_file(&root, input, b"{\"source\":2}\n");
        let error = ExternalCompatReader::open_with(&root, &generators, fixture_revision)
            .unwrap()
            .audit_all()
            .unwrap_err();
        assert!(error.contains("input content hash drifted"));
        write_file(&root, input, b"{\"source\":1}\n");

        write_file(&root, output, b"{\"different\":true}\n");
        let output_drift = generated_manifest(&root, output, input, "generator.fixture");
        write_parsed_manifest(&root, &output_drift);
        let error = ExternalCompatReader::open_with(&root, &generators, fixture_revision)
            .unwrap()
            .audit_all()
            .unwrap_err();
        assert!(error.contains("does not match deterministic replay"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generated_provenance_rejects_cycles_and_manifest_claimed_revision() {
        let root = fixture_root("generated-cycle");
        let output = "tests/external-compat/data/generated.json";
        write_file(&root, output, b"{}\n");
        write_file(&root, "tools/generate.rs", b"fn main() {}\n");
        let manifest = generated_manifest(&root, output, output, "generator.fixture");
        let generators = [fixture_generator()];
        let error = validate_manifest_structure(&manifest, &generators).unwrap_err();
        assert!(error.contains("provenance_cycle"));

        let source = format!(
            r#"
schema_version = "{EXTERNAL_COMPAT_SCHEMA_VERSION}"

[[entry]]
id = "compat.generated"
path = "{output}"
sha256 = "{hash}"
purpose = "generated parser compatibility"
allowed_scope = ["parser.generated"]

[entry.source]
kind = "generated"
generator_id = "generator.fixture"
generator_revision = "2222222222222222222222222222222222222222"

[[entry.source.input]]
path = "inputs/source.json"
sha256 = "{hash}"
"#,
            hash = "0".repeat(64),
        );
        let error = parse_external_compat_manifest(&source).unwrap_err();
        assert!(error.contains("generator_revision"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manifest_rejects_wildcards_traversal_and_unknown_generator() {
        let manifest = ExternalCompatManifest {
            schema_version: EXTERNAL_COMPAT_SCHEMA_VERSION.to_string(),
            entry: vec![ExternalCompatEntry {
                id: "compat.invalid".to_string(),
                path: "tests/external-compat/data/*.json".to_string(),
                sha256: "0".repeat(64),
                purpose: "invalid fixture".to_string(),
                allowed_scope: vec![ExternalCompatScope::ParserSchema],
                source: ExternalCompatSource::Generated {
                    generator_id: "generator.unknown".to_string(),
                    parameters: BTreeMap::from([(
                        "source.url".to_string(),
                        GeneratedParameter::Identifier(
                            "https://example.invalid/floating".to_string(),
                        ),
                    )]),
                    input: vec![GeneratedInput {
                        path: "../source.json".to_string(),
                        sha256: "0".repeat(64),
                    }],
                },
            }],
        };
        let error = validate_manifest_structure(&manifest, &[]).unwrap_err();
        assert!(error.contains("invalid exact path"));
        assert!(error.contains("invalid generator parameter source.url"));
        assert!(error.contains("unregistered generator"));
    }

    #[test]
    fn generated_provenance_rejects_arbitrary_command_field() {
        let source = format!(
            r#"
schema_version = "{EXTERNAL_COMPAT_SCHEMA_VERSION}"

[[entry]]
id = "compat.generated"
path = "tests/external-compat/data/generated.json"
sha256 = "{hash}"
purpose = "generated parser compatibility"
allowed_scope = ["parser.generated"]

[entry.source]
kind = "generated"
generator_id = "generator.fixture"
command = "git fetch origin main"

[[entry.source.input]]
path = "inputs/source.json"
sha256 = "{hash}"
"#,
            hash = "0".repeat(64),
        );

        let error = parse_external_compat_manifest(&source).unwrap_err();
        assert!(error.contains("command"));
    }

    #[test]
    fn scoped_reader_authorizes_before_entry_io_and_hides_actual_hash() {
        let root = fixture_root("scoped-reader");
        let path = "tests/external-compat/data/sample.json";
        let expected = b"{\"value\":1}\n";
        write_file(&root, path, expected);
        let manifest = upstream_manifest(
            path,
            &sha256_file(&root.join(path)),
            ExternalCompatScope::ParserSchema,
        );
        write_parsed_manifest(&root, &manifest);
        let reader = ExternalCompatReader::open(&root).unwrap();

        fs::remove_file(root.join(path)).unwrap();
        let error = reader.read_parser_generated(path).unwrap_err();
        assert!(error.contains("does not allow scope parser.generated"));

        write_file(&root, path, b"{\"value\":2}\n");
        let actual = sha256_file(&root.join(path));
        let error = reader.read_parser_schema(path).unwrap_err();
        assert!(error.contains("content hash drifted"));
        assert!(!error.contains(&actual));

        write_file(&root, path, expected);
        assert_eq!(reader.read_parser_schema(path).unwrap(), expected);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn verified_handle_does_not_follow_path_replacement_after_open() {
        let root = fixture_root("same-handle");
        let path = "tests/external-compat/data/sample.json";
        let moved = "tests/external-compat/data/opened.json";
        let replacement = "replacement.json";
        let expected = b"{\"trusted\":true}\n";
        let outside = b"{\"outside\":true}\n";
        write_file(&root, path, expected);
        write_file(&root, replacement, outside);
        let manifest = upstream_manifest(
            path,
            &sha256_file(&root.join(path)),
            ExternalCompatScope::ParserSchema,
        );
        write_parsed_manifest(&root, &manifest);
        let reader = ExternalCompatReader::open(&root).unwrap();

        let bytes = reader
            .read_scoped_with_hook(ExternalCompatScope::ParserSchema, path, || {
                fs::rename(root.join(path), root.join(moved))
                    .map_err(|error| format!("move opened fixture: {error}"))?;
                fs::rename(root.join(replacement), root.join(path))
                    .map_err(|error| format!("replace opened fixture: {error}"))?;
                Ok(())
            })
            .unwrap();
        assert_eq!(bytes, expected);
        assert_eq!(fs::read(root.join(path)).unwrap(), outside);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn verified_open_rejects_symlink_before_consuming_outside_bytes() {
        use std::os::unix::fs::symlink;

        let root = fixture_root("symlink");
        let path = "tests/external-compat/data/sample.json";
        let outside = root.parent().unwrap().join(format!(
            "actingcommand-external-outside-{}",
            std::process::id()
        ));
        fs::write(&outside, b"outside-secret").unwrap();
        write_file(&root, path, b"trusted");
        let manifest = upstream_manifest(
            path,
            &sha256_file(&root.join(path)),
            ExternalCompatScope::ParserSchema,
        );
        write_parsed_manifest(&root, &manifest);
        let reader = ExternalCompatReader::open(&root).unwrap();
        fs::remove_file(root.join(path)).unwrap();
        symlink(&outside, root.join(path)).unwrap();

        let error = reader.read_parser_schema(path).unwrap_err();
        assert!(error.contains("failed to open verified path"));
        assert!(!error.contains(&sha256_file(&outside)));
        fs::remove_dir_all(root).unwrap();
        fs::remove_file(outside).unwrap();
    }

    fn upstream_manifest(
        path: &str,
        hash: &str,
        scope: ExternalCompatScope,
    ) -> ExternalCompatManifest {
        ExternalCompatManifest {
            schema_version: EXTERNAL_COMPAT_SCHEMA_VERSION.to_string(),
            entry: vec![ExternalCompatEntry {
                id: "compat.sample".to_string(),
                path: path.to_string(),
                sha256: hash.to_string(),
                purpose: "parser compatibility".to_string(),
                allowed_scope: vec![scope],
                source: ExternalCompatSource::Upstream {
                    repository_url: "https://github.com/example/upstream".to_string(),
                    commit_sha: "1".repeat(40),
                    upstream_path: "fixtures/sample.json".to_string(),
                    sha256: hash.to_string(),
                },
            }],
        }
    }

    fn generated_manifest(
        root: &Path,
        output: &str,
        input: &str,
        generator_id: &str,
    ) -> ExternalCompatManifest {
        ExternalCompatManifest {
            schema_version: EXTERNAL_COMPAT_SCHEMA_VERSION.to_string(),
            entry: vec![ExternalCompatEntry {
                id: "compat.generated".to_string(),
                path: output.to_string(),
                sha256: sha256_file(&root.join(output)),
                purpose: "generated parser compatibility".to_string(),
                allowed_scope: vec![ExternalCompatScope::ParserGenerated],
                source: ExternalCompatSource::Generated {
                    generator_id: generator_id.to_string(),
                    parameters: BTreeMap::from([(
                        "format.version".to_string(),
                        GeneratedParameter::Integer(1),
                    )]),
                    input: vec![GeneratedInput {
                        path: input.to_string(),
                        sha256: sha256_file(&root.join(input)),
                    }],
                },
            }],
        }
    }

    fn fixture_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "actingcommand-external-compat-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("tests/external-compat")).unwrap();
        root
    }

    fn write_file(root: &Path, path: &str, bytes: &[u8]) {
        let target = root.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, bytes).unwrap();
    }

    fn write_manifest(root: &Path) {
        write_file(
            root,
            EXTERNAL_COMPAT_MANIFEST_PATH,
            format!("schema_version = {EXTERNAL_COMPAT_SCHEMA_VERSION:?}\n").as_bytes(),
        );
    }

    fn write_parsed_manifest(root: &Path, manifest: &ExternalCompatManifest) {
        let mut source = format!("schema_version = {:?}\n", EXTERNAL_COMPAT_SCHEMA_VERSION);
        for entry in &manifest.entry {
            source.push_str(&format!(
                "\n[[entry]]\nid = {:?}\npath = {:?}\nsha256 = {:?}\npurpose = {:?}\nallowed_scope = [{:?}]\n",
                entry.id,
                entry.path,
                entry.sha256,
                entry.purpose,
                entry.allowed_scope[0].as_str(),
            ));
            match &entry.source {
                ExternalCompatSource::Upstream {
                    repository_url,
                    commit_sha,
                    upstream_path,
                    sha256,
                } => source.push_str(&format!(
                    "\n[entry.source]\nkind = \"upstream\"\nrepository_url = {repository_url:?}\ncommit_sha = {commit_sha:?}\nupstream_path = {upstream_path:?}\nsha256 = {sha256:?}\n"
                )),
                ExternalCompatSource::Generated {
                    generator_id,
                    parameters,
                    input,
                } => {
                    source.push_str(&format!(
                        "\n[entry.source]\nkind = \"generated\"\ngenerator_id = {generator_id:?}\n"
                    ));
                    if !parameters.is_empty() {
                        source.push_str("\n[entry.source.parameters]\n");
                        for (key, value) in parameters {
                            let value = match value {
                                GeneratedParameter::Boolean(value) => value.to_string(),
                                GeneratedParameter::Integer(value) => value.to_string(),
                                GeneratedParameter::Identifier(value) => format!("{value:?}"),
                            };
                            source.push_str(&format!("{key:?} = {value}\n"));
                        }
                    }
                    for item in input {
                        source.push_str(&format!(
                            "\n[[entry.source.input]]\npath = {:?}\nsha256 = {:?}\n",
                            item.path, item.sha256
                        ));
                    }
                }
            }
        }
        write_file(root, EXTERNAL_COMPAT_MANIFEST_PATH, source.as_bytes());
    }

    fn sha256_file(path: &Path) -> String {
        let bytes = fs::read(path).unwrap();
        format!("{:x}", Sha256::digest(bytes))
    }
}
