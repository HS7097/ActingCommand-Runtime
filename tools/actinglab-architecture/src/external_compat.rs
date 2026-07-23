// SPDX-License-Identifier: AGPL-3.0-only

//! Exact provenance, capability, and I/O validation for external compatibility data.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const EXTERNAL_COMPAT_SCHEMA_VERSION: &str = "actingcommand.external-compat.v1";
pub const EXTERNAL_COMPAT_MANIFEST_PATH: &str = "tests/external-compat/manifest-v1.toml";
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
        generator_path: String,
        generator_sha256: String,
        generator_blob_sha: String,
        generator_revision: String,
        entrypoint: String,
        command: Vec<String>,
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
type VerifyGeneratorRevision = fn(&Path, &str, &[u8], &str, &str) -> Result<(), String>;

#[derive(Clone, Copy)]
struct RegisteredGenerator {
    id: &'static str,
    source_path: &'static str,
    entrypoint: &'static str,
    command: &'static [&'static str],
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
    entrypoint: "replay_identity",
    command: &["internal:replay_identity"],
    replay: replay_identity,
}];

fn parse_external_compat_manifest(source: &str) -> Result<ExternalCompatManifest, String> {
    toml::from_str(source).map_err(|error| format!("invalid external-compat manifest: {error}"))
}

fn load_external_compat_manifest(root: &Path) -> Result<ExternalCompatManifest, String> {
    let mut file = open_verified_beneath(root, EXTERNAL_COMPAT_MANIFEST_PATH)
        .map_err(|error| format!("external-compat manifest {error}"))?;
    let bytes =
        read_bounded(&mut file).map_err(|error| format!("external-compat manifest {error}"))?;
    let source = std::str::from_utf8(&bytes)
        .map_err(|error| format!("external-compat manifest is not UTF-8: {error}"))?;
    parse_external_compat_manifest(source)
}

struct ExternalCompatReader<'a> {
    root: PathBuf,
    manifest: ExternalCompatManifest,
    generators: &'a [RegisteredGenerator],
    revision_verifier: VerifyGeneratorRevision,
}

impl ExternalCompatReader<'static> {
    fn open(root: &Path) -> Result<Self, String> {
        Self::open_with(root, REGISTERED_GENERATORS, verify_generator_revision)
    }
}

impl<'a> ExternalCompatReader<'a> {
    /// Loading the catalog parses and authorizes metadata only; entry bytes are
    /// opened later, after a typed use has selected its private capability.
    fn open_with(
        root: &Path,
        generators: &'a [RegisteredGenerator],
        revision_verifier: VerifyGeneratorRevision,
    ) -> Result<Self, String> {
        let manifest = load_external_compat_manifest(root)?;
        validate_manifest_structure(&manifest, generators)?;
        Ok(Self {
            root: root.to_path_buf(),
            manifest,
            generators,
            revision_verifier,
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
        if let ExternalCompatSource::Upstream { sha256, .. } = &entry.source {
            if format!("{:x}", Sha256::digest(output)) != *sha256 {
                return Err(format!(
                    "entry {} upstream raw-byte hash drifted",
                    entry.id
                ));
            }
            return Ok(());
        }
        let ExternalCompatSource::Generated {
            generator_id,
            generator_path,
            generator_sha256,
            generator_blob_sha,
            generator_revision,
            entrypoint: _,
            command: _,
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
        let generator_source = read_hashed_workspace_file(
            &self.root,
            &entry.id,
            "generator",
            generator_path,
            generator_sha256,
        )?;
        (self.revision_verifier)(
            &self.root,
            generator.source_path,
            &generator_source,
            generator_blob_sha,
            generator_revision,
        )?;

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
        if entry.purpose.trim().is_empty() || entry.purpose.trim() != entry.purpose {
            errors.push(format!(
                "entry {} purpose must be nonempty and have no surrounding whitespace",
                entry.id
            ));
        }
        validate_scopes(entry, &mut errors);
        validate_source_structure(entry, generators, &mut errors);
    }
    validate_manifest_provenance_graph(manifest, &mut errors);
    finish_errors(errors)
}

fn validate_manifest_provenance_graph(manifest: &ExternalCompatManifest, errors: &mut Vec<String>) {
    let output_paths = manifest
        .entry
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<HashSet<_>>();
    let mut graph = BTreeMap::<&str, BTreeSet<&str>>::new();
    let mut rooted = HashSet::<&str>::new();

    for entry in &manifest.entry {
        let dependencies = match &entry.source {
            ExternalCompatSource::Upstream { .. } => {
                rooted.insert(entry.path.as_str());
                Vec::new()
            }
            ExternalCompatSource::Generated {
                generator_path,
                input,
                ..
            } => std::iter::once(generator_path.as_str())
                .chain(input.iter().map(|item| item.path.as_str()))
                .collect::<Vec<_>>(),
        };
        let edges = graph.entry(entry.path.as_str()).or_default();
        for dependency in dependencies {
            if output_paths.contains(dependency) {
                edges.insert(dependency);
            } else {
                rooted.insert(entry.path.as_str());
            }
        }
    }

    fn visit<'a>(
        node: &'a str,
        graph: &BTreeMap<&'a str, BTreeSet<&'a str>>,
        states: &mut HashMap<&'a str, u8>,
        stack: &mut Vec<&'a str>,
        errors: &mut Vec<String>,
    ) {
        match states.get(node).copied() {
            Some(1) => {
                let start = stack
                    .iter()
                    .position(|candidate| *candidate == node)
                    .unwrap_or(0);
                let mut cycle = stack[start..].to_vec();
                cycle.push(node);
                errors.push(format!("manifest provenance_cycle: {}", cycle.join(" -> ")));
                return;
            }
            Some(2) => return,
            _ => {}
        }
        states.insert(node, 1);
        stack.push(node);
        if let Some(dependencies) = graph.get(node) {
            for dependency in dependencies {
                visit(dependency, graph, states, stack, errors);
            }
        }
        stack.pop();
        states.insert(node, 2);
    }

    let mut states = HashMap::new();
    for node in graph.keys().copied() {
        visit(node, &graph, &mut states, &mut Vec::new(), errors);
    }

    fn reaches_root<'a>(
        node: &'a str,
        graph: &BTreeMap<&'a str, BTreeSet<&'a str>>,
        rooted: &HashSet<&'a str>,
        visiting: &mut HashSet<&'a str>,
    ) -> bool {
        if rooted.contains(node) {
            return true;
        }
        if !visiting.insert(node) {
            return false;
        }
        let result = graph.get(node).is_some_and(|dependencies| {
            dependencies
                .iter()
                .any(|dependency| reaches_root(dependency, graph, rooted, visiting))
        });
        visiting.remove(node);
        result
    }
    for node in graph.keys().copied() {
        if !reaches_root(node, &graph, &rooted, &mut HashSet::new()) {
            errors.push(format!(
                "manifest provenance has no independent root for output {node}"
            ));
        }
    }
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
        if !is_registry_id(generator.entrypoint) {
            errors.push(format!(
                "generator {} has invalid entrypoint {}",
                generator.id, generator.entrypoint
            ));
        }
        if generator.command.is_empty()
            || generator
                .command
                .iter()
                .any(|argument| argument.trim().is_empty())
        {
            errors.push(format!("generator {} has invalid command", generator.id));
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
            generator_path,
            generator_sha256,
            generator_blob_sha,
            generator_revision,
            entrypoint,
            command,
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
            if let Err(error) = validate_repo_path(generator_path) {
                errors.push(format!("entry {} generator path {error}", entry.id));
            }
            if !is_sha256(generator_sha256) {
                errors.push(format!("entry {} has invalid generator_sha256", entry.id));
            }
            if !is_git_sha(generator_blob_sha) {
                errors.push(format!("entry {} has invalid generator_blob_sha", entry.id));
            }
            if !is_git_sha(generator_revision) {
                errors.push(format!("entry {} has invalid generator_revision", entry.id));
            }
            if !is_registry_id(entrypoint) {
                errors.push(format!(
                    "entry {} has invalid generator entrypoint",
                    entry.id
                ));
            }
            if command.is_empty() || command.iter().any(|argument| argument.trim().is_empty()) {
                errors.push(format!("entry {} has invalid generator command", entry.id));
            }
            if let Some(generator) = generator {
                if generator_path != generator.source_path {
                    errors.push(format!(
                        "entry {} generator_path does not match registered generator {}",
                        entry.id, generator_id
                    ));
                }
                if entrypoint != generator.entrypoint {
                    errors.push(format!(
                        "entry {} entrypoint does not match registered generator {}",
                        entry.id, generator_id
                    ));
                }
                if command
                    .iter()
                    .map(String::as_str)
                    .ne(generator.command.iter().copied())
                {
                    errors.push(format!(
                        "entry {} command does not match registered generator {}",
                        entry.id, generator_id
                    ));
                }
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
            insert_provenance_role(&entry.id, &mut roles, generator_path, "generator", errors);
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

fn verify_generator_revision(
    root: &Path,
    path: &str,
    source: &[u8],
    expected_blob_sha: &str,
    expected_revision: &str,
) -> Result<(), String> {
    if !is_git_sha(expected_blob_sha) {
        return Err(format!(
            "registered generator has invalid pinned Git blob: {path}"
        ));
    }
    if !is_git_sha(expected_revision) {
        return Err(format!(
            "registered generator has invalid pinned Git revision: {path}"
        ));
    }
    let tracked = git_output(root, &["ls-files", "--error-unmatch", "--", path])?;
    if tracked.trim().is_empty() {
        return Err(format!(
            "registered generator source is not tracked: {path}"
        ));
    }
    let committed_blob = git_output(root, &["rev-parse", &format!("{expected_revision}:{path}")])?;
    if committed_blob.trim() != expected_blob_sha {
        return Err(format!(
            "registered generator blob differs from pinned revision {expected_revision}: {path}"
        ));
    }
    let committed = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["show", &format!("{expected_revision}:{path}")])
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
            "registered generator source differs from pinned revision {expected_revision}: {path}"
        ));
    }
    let ancestor = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["merge-base", "--is-ancestor", expected_revision, "HEAD"])
        .status()
        .map_err(|error| format!("failed to verify generator revision ancestry: {error}"))?;
    if !ancestor.success() {
        return Err(format!(
            "registered generator revision is not an ancestor of HEAD: {expected_revision}"
        ));
    }
    Ok(())
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
    if !is_sha256(expected) {
        return Err(format!("entry {entry_id} has invalid {role} sha256"));
    }
    let mut file = open_verified_beneath(root, path)
        .map_err(|error| format!("entry {entry_id} {role} {error}"))?;
    let bytes =
        read_bounded(&mut file).map_err(|error| format!("entry {entry_id} {role} {error}"))?;
    if format!("{:x}", Sha256::digest(&bytes)) != expected {
        return Err(format!("entry {entry_id} {role} content hash drifted"));
    }
    Ok(bytes)
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
enum StaticWindowsPathKind {
    Directory,
    File,
}

#[cfg(windows)]
fn validate_windows_non_reparse_component(
    path: &Path,
    expected: StaticWindowsPathKind,
    is_reparse: bool,
    is_directory: bool,
    is_file: bool,
) -> Result<(), String> {
    let (kind, expected_type) = match expected {
        StaticWindowsPathKind::Directory => ("directory", is_directory),
        StaticWindowsPathKind::File => ("file", is_file),
    };
    if is_reparse || !expected_type {
        Err(format!(
            "path component is not a regular non-reparse {kind}: {}",
            path.display()
        ))
    } else {
        Ok(())
    }
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
    validate_windows_non_reparse_component(
        root,
        StaticWindowsPathKind::Directory,
        root_metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0,
        root_metadata.is_dir(),
        root_metadata.is_file(),
    )?;

    let mut candidate = root.to_path_buf();
    let components = Path::new(relative).components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(component) = component else {
            return Err(format!("has unsafe path {relative}"));
        };
        candidate.push(component);
        let last = index + 1 == components.len();
        let metadata = fs::symlink_metadata(&candidate)
            .map_err(|error| format!("failed to inspect {}: {error}", candidate.display()))?;
        validate_windows_non_reparse_component(
            &candidate,
            if last {
                StaticWindowsPathKind::File
            } else {
                StaticWindowsPathKind::Directory
            },
            is_link_or_reparse(&metadata),
            metadata.is_dir(),
            metadata.is_file(),
        )?;
    }

    let file = open(&candidate, false)?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to inspect verified handle {relative}: {error}"))?;
    validate_windows_non_reparse_component(
        &candidate,
        StaticWindowsPathKind::File,
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0,
        metadata.is_dir(),
        metadata.is_file(),
    )?;
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
    let components = Path::new(path)
        .components()
        .map(|component| match component {
            Component::Normal(component) => component
                .to_str()
                .map(ToString::to_string)
                .ok_or(()),
            _ => Err(()),
        })
        .collect::<Result<Vec<_>, _>>();
    let is_canonical = matches!(&components, Ok(components) if components.join("/") == path);
    if path.is_empty()
        || path.contains(['\\', '*', '?'])
        || path.starts_with('/')
        || path.ends_with('/')
        || !is_canonical
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
    let Some(repository) = value.strip_prefix("https://github.com/") else {
        return false;
    };
    if repository.ends_with('/')
        || repository.ends_with(".git")
        || repository.contains([' ', '\t', '\n', '\r', '?', '#', '*', '\\'])
    {
        return false;
    }
    let parts = repository.split('/').collect::<Vec<_>>();
    parts.len() == 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && *part != "."
                && *part != ".."
                && part.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
                })
        })
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

    fn fixture_revision(
        _root: &Path,
        path: &str,
        source: &[u8],
        expected_blob_sha: &str,
        expected_revision: &str,
    ) -> Result<(), String> {
        if path != "tools/generate.rs" || source != b"fn main() {}\n" {
            return Err("generator source drifted from trusted fixture revision".to_string());
        }
        if expected_revision != "2".repeat(40) {
            return Err("generator revision drifted from trusted fixture revision".to_string());
        }
        if expected_blob_sha != "3".repeat(40) {
            return Err("generator blob drifted from trusted fixture revision".to_string());
        }
        Ok(())
    }

    fn fixture_generator() -> RegisteredGenerator {
        RegisteredGenerator {
            id: "generator.fixture",
            source_path: "tools/generate.rs",
            entrypoint: "replay_first_input",
            command: &["internal:replay_first_input"],
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
    fn oversized_manifest_fails_closed_before_parsing() {
        let root = fixture_root("oversized-manifest");
        let path = root.join(EXTERNAL_COMPAT_MANIFEST_PATH);
        let file = File::create(&path).unwrap();
        file.set_len(MAX_EXTERNAL_COMPAT_BYTES + 1).unwrap();

        let error = load_external_compat_manifest(&root).unwrap_err();
        assert!(error.contains("byte limit"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn upstream_entry_validates_the_offline_pinned_protocol() {
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
    fn upstream_entry_rejects_url_commit_path_hash_purpose_and_scope_drift() {
        let root = fixture_root("upstream-metadata-drift");
        let path = "tests/external-compat/data/sample.json";
        write_file(&root, path, b"{\"value\":1}\n");
        let hash = sha256_file(&root.join(path));
        let manifest = upstream_manifest(path, &hash, ExternalCompatScope::ParserSchema);

        let mut url_drift = manifest.clone();
        let ExternalCompatSource::Upstream { repository_url, .. } =
            &mut url_drift.entry[0].source
        else {
            unreachable!()
        };
        repository_url.push('/');
        assert!(
            validate_manifest_structure(&url_drift, REGISTERED_GENERATORS)
                .unwrap_err()
                .contains("invalid upstream repository URL")
        );

        let mut commit_drift = manifest.clone();
        let ExternalCompatSource::Upstream { commit_sha, .. } =
            &mut commit_drift.entry[0].source
        else {
            unreachable!()
        };
        *commit_sha = "main".to_string();
        assert!(
            validate_manifest_structure(&commit_drift, REGISTERED_GENERATORS)
                .unwrap_err()
                .contains("invalid upstream commit SHA")
        );

        let mut upstream_path_drift = manifest.clone();
        let ExternalCompatSource::Upstream { upstream_path, .. } =
            &mut upstream_path_drift.entry[0].source
        else {
            unreachable!()
        };
        *upstream_path = "../fixtures/sample.json".to_string();
        assert!(
            validate_manifest_structure(&upstream_path_drift, REGISTERED_GENERATORS)
                .unwrap_err()
                .contains("upstream path has invalid exact path")
        );

        let mut hash_drift = manifest.clone();
        let ExternalCompatSource::Upstream { sha256, .. } = &mut hash_drift.entry[0].source else {
            unreachable!()
        };
        *sha256 = "f".repeat(64);
        assert!(
            validate_manifest_structure(&hash_drift, REGISTERED_GENERATORS)
                .unwrap_err()
                .contains("raw-byte hash does not match entry sha256")
        );

        let mut exact_path_drift = manifest.clone();
        exact_path_drift.entry[0].path = "fixtures/sample.json".to_string();
        assert!(
            validate_manifest_structure(&exact_path_drift, REGISTERED_GENERATORS)
                .unwrap_err()
                .contains("path must be under tests/external-compat/data/")
        );

        let mut purpose_drift = manifest.clone();
        purpose_drift.entry[0].purpose.clear();
        assert!(
            validate_manifest_structure(&purpose_drift, REGISTERED_GENERATORS)
                .unwrap_err()
                .contains("purpose must be nonempty")
        );

        let mut purpose_whitespace = manifest.clone();
        purpose_whitespace.entry[0].purpose = " parser compatibility ".to_string();
        assert!(
            validate_manifest_structure(&purpose_whitespace, REGISTERED_GENERATORS)
                .unwrap_err()
                .contains("no surrounding whitespace")
        );

        for noncanonical in ["fixtures//sample.json", "fixtures/./sample.json"] {
            let mut path_drift = manifest.clone();
            let ExternalCompatSource::Upstream { upstream_path, .. } =
                &mut path_drift.entry[0].source
            else {
                unreachable!()
            };
            *upstream_path = noncanonical.to_string();
            assert!(
                validate_manifest_structure(&path_drift, REGISTERED_GENERATORS)
                    .unwrap_err()
                    .contains("upstream path has invalid exact path"),
                "accepted noncanonical path {noncanonical}"
            );
        }

        write_parsed_manifest(&root, &manifest);
        let reader = ExternalCompatReader::open(&root).unwrap();
        let error = reader.read_parser_generated(path).unwrap_err();
        assert!(error.contains("does not allow scope parser.generated"));
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

        let mut blob_drift = manifest.clone();
        if let ExternalCompatSource::Generated {
            generator_blob_sha, ..
        } = &mut blob_drift.entry[0].source
        {
            *generator_blob_sha = "4".repeat(40);
        }
        write_parsed_manifest(&root, &blob_drift);
        let error = ExternalCompatReader::open_with(&root, &generators, fixture_revision)
            .unwrap()
            .audit_all()
            .unwrap_err();
        assert!(error.contains("generator blob drifted"));
        write_parsed_manifest(&root, &manifest);

        write_file(&root, "tools/generate.rs", b"fn main() { panic!() }\n");
        let error = ExternalCompatReader::open_with(&root, &generators, fixture_revision)
            .unwrap()
            .audit_all()
            .unwrap_err();
        assert!(error.contains("generator content hash drifted"));

        let synchronized = generated_manifest(&root, output, input, "generator.fixture");
        write_parsed_manifest(&root, &synchronized);
        let error = ExternalCompatReader::open_with(&root, &generators, fixture_revision)
            .unwrap()
            .audit_all()
            .unwrap_err();
        assert!(error.contains("trusted fixture revision"));
        write_file(&root, "tools/generate.rs", b"fn main() {}\n");
        write_parsed_manifest(&root, &manifest);

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
    fn production_revision_verifier_pins_generator_blob_and_commit() {
        let root = fixture_root("generator-git-revision");
        let path = "tools/generate.rs";
        let original = b"fn main() {}\n";
        write_file(&root, path, original);
        git_fixture(&root, &["init"]);
        git_fixture(&root, &["config", "core.autocrlf", "false"]);
        git_fixture(&root, &["config", "user.name", "fixture"]);
        git_fixture(&root, &["config", "user.email", "fixture@example.invalid"]);
        git_fixture(&root, &["add", "--", path]);
        git_fixture(&root, &["commit", "-m", "pin generator"]);

        let revision = git_fixture(&root, &["rev-parse", "HEAD"]);
        let blob = git_fixture(&root, &["rev-parse", &format!("HEAD:{path}")]);
        verify_generator_revision(&root, path, original, &blob, &revision).unwrap();

        let error = verify_generator_revision(&root, path, original, &"0".repeat(40), &revision)
            .unwrap_err();
        assert!(error.contains("blob differs from pinned revision"));

        let modified = b"fn main() { println!(\"changed\"); }\n";
        write_file(&root, path, modified);
        let error = verify_generator_revision(&root, path, modified, &blob, &revision).unwrap_err();
        assert!(error.contains("source differs from pinned revision"));

        git_fixture(&root, &["add", "--", path]);
        git_fixture(&root, &["commit", "-m", "change generator"]);
        let changed_revision = git_fixture(&root, &["rev-parse", "HEAD"]);
        let changed_blob = git_fixture(&root, &["rev-parse", &format!("HEAD:{path}")]);
        verify_generator_revision(&root, path, modified, &changed_blob, &changed_revision).unwrap();
        let error =
            verify_generator_revision(&root, path, modified, &changed_blob, &revision).unwrap_err();
        assert!(error.contains("blob differs from pinned revision"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generated_provenance_rejects_cycles_and_unpinned_revision() {
        let root = fixture_root("generated-cycle");
        let output = "tests/external-compat/data/generated.json";
        write_file(&root, output, b"{}\n");
        write_file(&root, "tools/generate.rs", b"fn main() {}\n");
        let manifest = generated_manifest(&root, output, output, "generator.fixture");
        let generators = [fixture_generator()];
        let error = validate_manifest_structure(&manifest, &generators).unwrap_err();
        assert!(error.contains("provenance_cycle"));

        let input = "inputs/source.json";
        write_file(&root, input, b"{}\n");
        let mut unpinned = generated_manifest(&root, output, input, "generator.fixture");
        if let ExternalCompatSource::Generated {
            generator_revision, ..
        } = &mut unpinned.entry[0].source
        {
            *generator_revision = "latest".to_string();
        }
        let error = validate_manifest_structure(&unpinned, &generators).unwrap_err();
        assert!(error.contains("invalid generator_revision"));

        if let ExternalCompatSource::Generated {
            generator_revision, ..
        } = &mut unpinned.entry[0].source
        {
            *generator_revision = "3".repeat(40);
        }
        write_parsed_manifest(&root, &unpinned);
        let error = ExternalCompatReader::open_with(&root, &generators, fixture_revision)
            .unwrap()
            .audit_all()
            .unwrap_err();
        assert!(error.contains("generator revision drifted"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generated_provenance_rejects_cross_entry_cycle_without_a_root() {
        let root = fixture_root("generated-cross-entry-cycle");
        let first = "tests/external-compat/data/first.json";
        let second = "tests/external-compat/data/second.json";
        write_file(&root, first, b"{}\n");
        write_file(&root, second, b"{}\n");
        write_file(&root, "tools/generate.rs", b"fn main() {}\n");

        let mut first_entry = generated_manifest(&root, first, second, "generator.fixture")
            .entry
            .remove(0);
        first_entry.id = "compat.first".to_string();
        let mut second_entry = generated_manifest(&root, second, first, "generator.fixture")
            .entry
            .remove(0);
        second_entry.id = "compat.second".to_string();
        let manifest = ExternalCompatManifest {
            schema_version: EXTERNAL_COMPAT_SCHEMA_VERSION.to_string(),
            entry: vec![first_entry, second_entry],
        };
        let error = validate_manifest_structure(&manifest, &[fixture_generator()]).unwrap_err();
        assert!(error.contains("manifest provenance_cycle"));
        assert!(error.contains(first));
        assert!(error.contains(second));
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
                    generator_path: "../tools/generate.rs".to_string(),
                    generator_sha256: "0".repeat(64),
                    generator_blob_sha: "latest".to_string(),
                    generator_revision: "2".repeat(40),
                    entrypoint: "replay_first_input".to_string(),
                    command: vec!["internal:replay_first_input".to_string()],
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
        assert!(error.contains("invalid generator_blob_sha"));
        assert!(error.contains("unregistered generator"));
    }

    #[test]
    fn generated_provenance_rejects_generator_identity_and_command_drift() {
        let root = fixture_root("generator-identity-drift");
        let output = "tests/external-compat/data/generated.json";
        let input = "inputs/source.json";
        write_file(&root, output, b"{}\n");
        write_file(&root, input, b"{}\n");
        write_file(&root, "tools/generate.rs", b"fn main() {}\n");
        let generators = [fixture_generator()];
        let mut manifest = generated_manifest(&root, output, input, "generator.fixture");
        let ExternalCompatSource::Generated {
            generator_path,
            generator_sha256,
            generator_blob_sha,
            entrypoint,
            command,
            ..
        } = &mut manifest.entry[0].source
        else {
            unreachable!()
        };

        *generator_path = "tools/other.rs".to_string();
        *generator_sha256 = "f".repeat(64);
        *generator_blob_sha = "not-a-blob".to_string();
        *entrypoint = "other_entrypoint".to_string();
        *command = vec!["git".to_string(), "fetch".to_string()];
        let error = validate_manifest_structure(&manifest, &generators).unwrap_err();
        assert!(error.contains("generator_path does not match"));
        assert!(error.contains("invalid generator_blob_sha"));
        assert!(error.contains("entrypoint does not match"));
        assert!(error.contains("command does not match"));
        fs::remove_dir_all(root).unwrap();
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

    #[cfg(windows)]
    #[test]
    fn windows_static_reparse_facts_fail_closed_at_every_path_layer() {
        let cases = [
            (Path::new("workspace"), StaticWindowsPathKind::Directory),
            (
                Path::new("workspace/tests/external-compat/data"),
                StaticWindowsPathKind::Directory,
            ),
            (
                Path::new("workspace/tests/external-compat/data/sample.json"),
                StaticWindowsPathKind::File,
            ),
        ];
        for (path, expected) in cases {
            let error = validate_windows_non_reparse_component(path, expected, true, true, true)
                .unwrap_err();
            assert!(error.contains("non-reparse"));
        }

        validate_windows_non_reparse_component(
            Path::new("workspace/tests"),
            StaticWindowsPathKind::Directory,
            false,
            true,
            false,
        )
        .unwrap();
        validate_windows_non_reparse_component(
            Path::new("workspace/tests/sample.json"),
            StaticWindowsPathKind::File,
            false,
            false,
            true,
        )
        .unwrap();
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
        fs::remove_file(root.join(path)).unwrap();
        symlink(&outside, root.join(path)).unwrap();
        let reader = ExternalCompatReader::open(&root).unwrap();

        let error = reader.read_parser_schema(path).unwrap_err();
        assert!(error.contains("failed to open verified path"));
        assert!(!error.contains(&sha256_file(&outside)));
        fs::remove_dir_all(root).unwrap();
        fs::remove_file(outside).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn verified_open_rejects_static_symlink_ancestor() {
        use std::os::unix::fs::symlink;

        let root = fixture_root("symlink-ancestor");
        let path = "tests/external-compat/data/sample.json";
        write_file(&root, path, b"trusted");
        let manifest = upstream_manifest(
            path,
            &sha256_file(&root.join(path)),
            ExternalCompatScope::ParserSchema,
        );
        write_parsed_manifest(&root, &manifest);

        let outside = root.with_extension("outside-ancestor");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("sample.json"), b"outside-secret").unwrap();
        fs::remove_dir_all(root.join("tests/external-compat/data")).unwrap();
        symlink(&outside, root.join("tests/external-compat/data")).unwrap();

        let reader = ExternalCompatReader::open(&root).unwrap();
        let error = reader.read_parser_schema(path).unwrap_err();
        assert!(error.contains("failed to open verified path"));
        assert!(!error.contains(&sha256_file(&outside.join("sample.json"))));
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
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
                    generator_path: "tools/generate.rs".to_string(),
                    generator_sha256: sha256_file(&root.join("tools/generate.rs")),
                    generator_blob_sha: "3".repeat(40),
                    generator_revision: "2".repeat(40),
                    entrypoint: "replay_first_input".to_string(),
                    command: vec!["internal:replay_first_input".to_string()],
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

    fn git_fixture(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap_or_else(|error| panic!("run git {}: {error}", args.join(" ")));
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
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
                    generator_path,
                    generator_sha256,
                    generator_blob_sha,
                    generator_revision,
                    entrypoint,
                    command,
                    parameters,
                    input,
                } => {
                    source.push_str(&format!(
                        "\n[entry.source]\nkind = \"generated\"\ngenerator_id = {generator_id:?}\ngenerator_path = {generator_path:?}\ngenerator_sha256 = {generator_sha256:?}\ngenerator_blob_sha = {generator_blob_sha:?}\ngenerator_revision = {generator_revision:?}\nentrypoint = {entrypoint:?}\ncommand = {command:?}\n"
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
