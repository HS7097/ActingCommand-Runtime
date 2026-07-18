// SPDX-License-Identifier: AGPL-3.0-only

//! Exact provenance and scope validation for isolated external compatibility data.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const EXTERNAL_COMPAT_SCHEMA_VERSION: &str = "actingcommand.external-compat.v2";
pub const EXTERNAL_COMPAT_MANIFEST_PATH: &str = "tests/external-compat/manifest-v2.toml";
const EXTERNAL_COMPAT_DATA_ROOT: &str = "tests/external-compat/data/";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExternalCompatManifest {
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
pub enum ExternalCompatScope {
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
        generator_path: String,
        generator_revision: String,
        generator_sha256: String,
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

fn parse_external_compat_manifest(source: &str) -> Result<ExternalCompatManifest, String> {
    toml::from_str(source).map_err(|error| format!("invalid external-compat manifest: {error}"))
}

fn load_external_compat_manifest(path: &Path) -> Result<ExternalCompatManifest, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    parse_external_compat_manifest(&source)
}

pub struct ExternalCompatReader {
    root: PathBuf,
    manifest: ExternalCompatManifest,
}

impl ExternalCompatReader {
    /// Opens the validated containment zone without exposing raw manifest paths to consumers.
    pub fn open(root: &Path) -> Result<Self, String> {
        let manifest = load_external_compat_manifest(&root.join(EXTERNAL_COMPAT_MANIFEST_PATH))?;
        validate_external_compat_manifest(root, &manifest)?;
        Ok(Self {
            root: root.to_path_buf(),
            manifest,
        })
    }

    /// Authorizes scope before file I/O and verifies the bytes returned to the caller.
    pub fn read(&self, scope: ExternalCompatScope, path: &str) -> Result<Vec<u8>, String> {
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
        let resolved = resolve_regular_file(&self.root, path)?;
        let bytes = fs::read(&resolved)
            .map_err(|error| format!("failed to read {}: {error}", resolved.display()))?;
        let actual = format!("{:x}", Sha256::digest(&bytes));
        if actual != entry.sha256 {
            return Err(format!(
                "external-compat entry {} content hash drifted during scoped read: registered {}, actual {actual}",
                entry.id, entry.sha256
            ));
        }
        Ok(bytes)
    }

    pub(crate) fn registered_paths(&self) -> impl Iterator<Item = &str> {
        self.manifest.entry.iter().map(|entry| entry.path.as_str())
    }
}

pub fn load_and_validate_external_compat(root: &Path) -> Result<ExternalCompatReader, String> {
    ExternalCompatReader::open(root)
}

fn validate_external_compat_manifest(
    root: &Path,
    manifest: &ExternalCompatManifest,
) -> Result<(), String> {
    let mut errors = Vec::new();
    if manifest.schema_version != EXTERNAL_COMPAT_SCHEMA_VERSION {
        errors.push(format!(
            "unsupported schema_version {}; expected {EXTERNAL_COMPAT_SCHEMA_VERSION}",
            manifest.schema_version
        ));
    }

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
        validate_entry_source(root, entry, &mut errors);
        match resolve_regular_file(root, &entry.path) {
            Ok(path) => match sha256_file(&path) {
                Ok(actual) if actual != entry.sha256 => errors.push(format!(
                    "entry {} content hash drifted: registered {}, actual {actual}",
                    entry.id, entry.sha256
                )),
                Ok(_) => {}
                Err(error) => errors.push(error),
            },
            Err(error) => errors.push(format!("entry {} {error}", entry.id)),
        }
    }

    match external_compat_files(root) {
        Ok(files) => {
            for file in &files {
                if !paths.contains(file.as_str()) {
                    errors.push(format!("unregistered external-compat file {file}"));
                }
            }
            for path in paths {
                if !files.contains(&path.to_string()) {
                    errors.push(format!("registered external-compat file is missing {path}"));
                }
            }
        }
        Err(error) => errors.push(error),
    }

    finish_errors(errors)
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

fn validate_entry_source(root: &Path, entry: &ExternalCompatEntry, errors: &mut Vec<String>) {
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
            generator_path,
            generator_revision,
            generator_sha256,
            parameters,
            input,
        } => {
            validate_hashed_workspace_file(
                root,
                &entry.id,
                "generator",
                generator_path,
                generator_sha256,
                errors,
            );
            if !is_git_sha(generator_revision) {
                errors.push(format!("entry {} has invalid generator revision", entry.id));
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
            let mut previous = None;
            let mut input_paths = HashSet::new();
            for item in input {
                if !input_paths.insert(item.path.as_str()) {
                    errors.push(format!(
                        "entry {} repeats generated input {}",
                        entry.id, item.path
                    ));
                }
                if previous.is_some_and(|left: &str| left >= item.path.as_str()) {
                    errors.push(format!(
                        "entry {} generated inputs are not strictly sorted at {}",
                        entry.id, item.path
                    ));
                }
                previous = Some(item.path.as_str());
                validate_hashed_workspace_file(
                    root,
                    &entry.id,
                    "input",
                    &item.path,
                    &item.sha256,
                    errors,
                );
            }
        }
    }
}

fn validate_hashed_workspace_file(
    root: &Path,
    entry_id: &str,
    role: &str,
    path: &str,
    expected: &str,
    errors: &mut Vec<String>,
) {
    if !is_sha256(expected) {
        errors.push(format!("entry {entry_id} has invalid {role} sha256"));
        return;
    }
    match resolve_regular_file(root, path) {
        Ok(resolved) => match sha256_file(&resolved) {
            Ok(actual) if actual != expected => errors.push(format!(
                "entry {entry_id} {role} hash drifted: registered {expected}, actual {actual}"
            )),
            Ok(_) => {}
            Err(error) => errors.push(error),
        },
        Err(error) => errors.push(format!("entry {entry_id} {role} {error}")),
    }
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

fn resolve_regular_file(root: &Path, relative: &str) -> Result<PathBuf, String> {
    validate_repo_path(relative)?;
    let mut current = root.to_path_buf();
    for component in Path::new(relative).components() {
        let Component::Normal(part) = component else {
            return Err(format!("has unsafe path {relative}"));
        };
        current.push(part);
        let metadata = fs::symlink_metadata(&current)
            .map_err(|error| format!("failed to inspect {}: {error}", current.display()))?;
        if is_link_or_reparse(&metadata) {
            return Err(format!("path is a link or reparse point: {relative}"));
        }
    }
    if !current.is_file() {
        return Err(format!("path is not a regular file: {relative}"));
    }
    let canonical_root = fs::canonicalize(root)
        .map_err(|error| format!("failed to resolve {}: {error}", root.display()))?;
    let canonical_file = fs::canonicalize(&current)
        .map_err(|error| format!("failed to resolve {}: {error}", current.display()))?;
    if !canonical_file.starts_with(canonical_root) {
        return Err(format!("path escapes workspace root: {relative}"));
    }
    Ok(current)
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

fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
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

    #[test]
    fn empty_containment_zone_is_valid() {
        let root = fixture_root("empty");
        write_manifest(&root);
        load_and_validate_external_compat(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn upstream_entry_rejects_unregistered_file_and_hash_drift() {
        let root = fixture_root("upstream");
        let path = "tests/external-compat/data/sample.json";
        write_file(&root, path, b"{\"value\":1}\n");
        let hash = sha256_file(&root.join(path)).unwrap();
        let manifest = ExternalCompatManifest {
            schema_version: EXTERNAL_COMPAT_SCHEMA_VERSION.to_string(),
            entry: vec![ExternalCompatEntry {
                id: "compat.sample".to_string(),
                path: path.to_string(),
                sha256: hash.clone(),
                purpose: "parser compatibility".to_string(),
                allowed_scope: vec![ExternalCompatScope::ParserSchema],
                source: ExternalCompatSource::Upstream {
                    repository_url: "https://github.com/example/upstream".to_string(),
                    commit_sha: "1".repeat(40),
                    upstream_path: "fixtures/sample.json".to_string(),
                    sha256: hash,
                },
            }],
        };
        validate_external_compat_manifest(&root, &manifest).unwrap();

        write_file(&root, "tests/external-compat/unregistered.json", b"{}\n");
        let error = validate_external_compat_manifest(&root, &manifest).unwrap_err();
        assert!(error.contains("unregistered external-compat file"));
        fs::remove_file(root.join("tests/external-compat/unregistered.json")).unwrap();

        write_file(&root, path, b"{\"value\":2}\n");
        let error = validate_external_compat_manifest(&root, &manifest).unwrap_err();
        assert!(error.contains("content hash drifted"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generated_entry_rejects_generator_and_input_drift() {
        let root = fixture_root("generated");
        let output = "tests/external-compat/data/generated.json";
        let generator = "tools/generate.rs";
        let input = "inputs/source.json";
        write_file(&root, output, b"{\"value\":1}\n");
        write_file(&root, generator, b"fn main() {}\n");
        write_file(&root, input, b"{\"source\":1}\n");
        let manifest = ExternalCompatManifest {
            schema_version: EXTERNAL_COMPAT_SCHEMA_VERSION.to_string(),
            entry: vec![ExternalCompatEntry {
                id: "compat.generated".to_string(),
                path: output.to_string(),
                sha256: sha256_file(&root.join(output)).unwrap(),
                purpose: "generated parser compatibility".to_string(),
                allowed_scope: vec![ExternalCompatScope::ParserGenerated],
                source: ExternalCompatSource::Generated {
                    generator_path: generator.to_string(),
                    generator_revision: "2".repeat(40),
                    generator_sha256: sha256_file(&root.join(generator)).unwrap(),
                    parameters: BTreeMap::from([(
                        "format.version".to_string(),
                        GeneratedParameter::Integer(1),
                    )]),
                    input: vec![GeneratedInput {
                        path: input.to_string(),
                        sha256: sha256_file(&root.join(input)).unwrap(),
                    }],
                },
            }],
        };
        validate_external_compat_manifest(&root, &manifest).unwrap();

        write_file(&root, generator, b"fn main() { panic!() }\n");
        let error = validate_external_compat_manifest(&root, &manifest).unwrap_err();
        assert!(error.contains("generator hash drifted"));
        write_file(&root, generator, b"fn main() {}\n");

        write_file(&root, input, b"{\"source\":2}\n");
        let error = validate_external_compat_manifest(&root, &manifest).unwrap_err();
        assert!(error.contains("input hash drifted"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manifest_rejects_wildcards_and_traversal() {
        let root = fixture_root("invalid");
        let path = "tests/external-compat/data/sample.json";
        write_file(&root, path, b"{}\n");
        write_file(&root, "tools/generate.rs", b"fn main() {}\n");
        write_file(&root, "inputs/source.json", b"{}\n");
        let manifest = ExternalCompatManifest {
            schema_version: EXTERNAL_COMPAT_SCHEMA_VERSION.to_string(),
            entry: vec![ExternalCompatEntry {
                id: "compat.invalid".to_string(),
                path: "tests/external-compat/data/*.json".to_string(),
                sha256: sha256_file(&root.join(path)).unwrap(),
                purpose: "invalid fixture".to_string(),
                allowed_scope: vec![ExternalCompatScope::ParserSchema],
                source: ExternalCompatSource::Generated {
                    generator_path: "../generate.rs".to_string(),
                    generator_revision: "3".repeat(40),
                    generator_sha256: "0".repeat(64),
                    parameters: BTreeMap::from([(
                        "source.url".to_string(),
                        GeneratedParameter::Identifier(
                            "https://example.invalid/floating".to_string(),
                        ),
                    )]),
                    input: vec![GeneratedInput {
                        path: "inputs/*.json".to_string(),
                        sha256: "0".repeat(64),
                    }],
                },
            }],
        };
        let error = validate_external_compat_manifest(&root, &manifest).unwrap_err();
        assert!(error.contains("invalid exact path"));
        assert!(error.contains("invalid generator parameter source.url"));
        assert!(error.contains("unregistered external-compat file"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generated_provenance_rejects_arbitrary_command_field() {
        let source = r#"
schema_version = "actingcommand.external-compat.v2"

[[entry]]
id = "compat.generated"
path = "tests/external-compat/data/generated.json"
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
purpose = "generated parser compatibility"
allowed_scope = ["parser.generated"]

[entry.source]
kind = "generated"
generator_path = "tools/generate.rs"
generator_revision = "2222222222222222222222222222222222222222"
generator_sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
command = "git fetch origin main"

[[entry.source.input]]
path = "inputs/source.json"
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
"#;

        let error = parse_external_compat_manifest(source).unwrap_err();
        assert!(error.contains("unknown field `command`"));
    }

    #[test]
    fn scoped_reader_authorizes_before_io_and_hashes_returned_bytes() {
        let root = fixture_root("scoped-reader");
        let path = "tests/external-compat/data/sample.json";
        let expected = b"{\"value\":1}\n";
        write_file(&root, path, expected);
        let hash = sha256_file(&root.join(path)).unwrap();
        write_file(
            &root,
            EXTERNAL_COMPAT_MANIFEST_PATH,
            format!(
                r#"schema_version = "{EXTERNAL_COMPAT_SCHEMA_VERSION}"

[[entry]]
id = "compat.sample"
path = "{path}"
sha256 = "{hash}"
purpose = "parser compatibility"
allowed_scope = ["parser.schema"]

[entry.source]
kind = "upstream"
repository_url = "https://github.com/example/upstream"
commit_sha = "1111111111111111111111111111111111111111"
upstream_path = "fixtures/sample.json"
sha256 = "{hash}"
"#
            )
            .as_bytes(),
        );
        let reader = ExternalCompatReader::open(&root).unwrap();

        fs::remove_file(root.join(path)).unwrap();
        let error = reader
            .read(ExternalCompatScope::ParserGenerated, path)
            .unwrap_err();
        assert!(error.contains("does not allow scope parser.generated"));

        write_file(&root, path, b"{\"value\":2}\n");
        let error = reader
            .read(ExternalCompatScope::ParserSchema, path)
            .unwrap_err();
        assert!(error.contains("content hash drifted during scoped read"));

        write_file(&root, path, expected);
        assert_eq!(
            reader
                .read(ExternalCompatScope::ParserSchema, path)
                .unwrap(),
            expected
        );
        fs::remove_dir_all(root).unwrap();
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
}
