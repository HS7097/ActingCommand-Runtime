// SPDX-License-Identifier: AGPL-3.0-only

mod admission;

use actingcommand_page_detector::{PageDetector, load_page_set_from_json_str};
use actingcommand_recognition_pack::{
    AssetResolver, RecognitionEvaluator, UnsupportedRecognitionTarget, load_pack_from_json_str,
    unsupported_recognition_targets,
};
pub use admission::{
    AdmissionError, AdmissionResult, AdmittedAction, AdmittedAnchor, AdmittedControl,
    AdmittedControlPoint, AdmittedDestructiveRegion, AdmittedExpectation, AdmittedGuard,
    AdmittedNavigation, AdmittedOperation, AdmittedOperationDefaults, AdmittedPackage,
    AdmittedRoute, AdmittedTargetKind, AdmittedTask, AssetKey, BoundedPoint, BoundedRect,
    ExecutionMode, FrameStoreSettings, GuardVerification, InputDuration, OpaqueMetadata,
    OperationKey, PackageResolution, PageKey, PageSelector, TargetKey, TargetOffset, TargetTapMode,
    TaskKey,
};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::io::{Cursor, Read};
use std::path::{Component, Path};
use std::sync::Arc;
use zip::ZipArchive;

pub type ContainmentResult<T> = Result<T, ContainmentError>;

pub const DEFAULT_MAX_COMPRESSED_BYTES: u64 = 512 * 1024 * 1024;
pub const DEFAULT_MAX_TOTAL_DECOMPRESSED_BYTES: u64 = 1024 * 1024 * 1024;
pub const DEFAULT_MAX_ENTRY_BYTES: u64 = 256 * 1024 * 1024;
pub const DEFAULT_MAX_ENTRY_COUNT: usize = 4096;
pub const DEFAULT_MAX_RESIDENT_BYTES_PER_INSTANCE: u64 = 1024 * 1024 * 1024;

const DANGEROUS_EXTENSIONS: &[&str] = &[
    "py", "exe", "bat", "cmd", "ps1", "sh", "js", "vbs", "msi", "dll", "scr", "com", "jar",
];
const LAB_RESOURCE_ROOT: &str = "resources";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceId(String);

impl InstanceId {
    pub fn new(value: impl Into<String>) -> ContainmentResult<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ContainmentError::InvalidInstanceId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for InstanceId {
    type Error = ContainmentError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl fmt::Display for InstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskId(String);

impl TaskId {
    pub fn new(value: impl Into<String>) -> ContainmentResult<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ContainmentError::MissingTaskId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sha256Hash([u8; 32]);

impl Sha256Hash {
    pub fn digest(bytes: &[u8]) -> Self {
        let digest = Sha256::digest(bytes);
        let mut hash = [0_u8; 32];
        hash.copy_from_slice(&digest);
        Self(hash)
    }

    pub fn parse_hex(value: &str) -> ContainmentResult<Self> {
        let value = value.strip_prefix("sha256:").unwrap_or(value);
        if value.len() != 64 {
            return Err(ContainmentError::InvalidHash {
                value: value.to_string(),
            });
        }
        let mut bytes = [0_u8; 32];
        for (index, byte) in bytes.iter_mut().enumerate() {
            let offset = index * 2;
            *byte = u8::from_str_radix(&value[offset..offset + 2], 16).map_err(|_| {
                ContainmentError::InvalidHash {
                    value: value.to_string(),
                }
            })?;
        }
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for Sha256Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

pub trait TrustSource {
    fn expected_hash(&self, task_id: &TaskId) -> ContainmentResult<Sha256Hash>;
}

#[derive(Debug, Clone, Copy)]
pub struct ContainmentLimits {
    pub max_compressed_bytes: u64,
    pub max_total_decompressed_bytes: u64,
    pub max_entry_bytes: u64,
    pub max_entry_count: usize,
    pub max_resident_bytes_per_instance: u64,
}

impl Default for ContainmentLimits {
    fn default() -> Self {
        Self {
            max_compressed_bytes: DEFAULT_MAX_COMPRESSED_BYTES,
            max_total_decompressed_bytes: DEFAULT_MAX_TOTAL_DECOMPRESSED_BYTES,
            max_entry_bytes: DEFAULT_MAX_ENTRY_BYTES,
            max_entry_count: DEFAULT_MAX_ENTRY_COUNT,
            max_resident_bytes_per_instance: DEFAULT_MAX_RESIDENT_BYTES_PER_INSTANCE,
        }
    }
}

#[derive(Debug, Default)]
pub struct Containment {
    limits: ContainmentLimits,
    benches: BTreeMap<InstanceId, Bench>,
}

impl Containment {
    pub fn new() -> Self {
        Self::with_limits(ContainmentLimits::default())
    }

    pub fn with_limits(limits: ContainmentLimits) -> Self {
        Self {
            limits,
            benches: BTreeMap::new(),
        }
    }

    pub fn load(
        &mut self,
        instance: &InstanceId,
        task_zip_bytes: &[u8],
        expected: &Sha256Hash,
    ) -> ContainmentResult<&LoadedBundle> {
        if task_zip_bytes.len() as u64 > self.limits.max_compressed_bytes {
            return Err(ContainmentError::CompressedTooLarge {
                instance: instance.clone(),
                size: task_zip_bytes.len() as u64,
                limit: self.limits.max_compressed_bytes,
            });
        }
        let actual = Sha256Hash::digest(task_zip_bytes);
        if !constant_time_hash_eq(&actual, expected) {
            return Err(ContainmentError::HashMismatch {
                instance: instance.clone(),
                expected: *expected,
                actual,
            });
        }
        let package = MemoryPackage::from_zip(task_zip_bytes, self.limits, instance)?;
        let bundle = LoadedBundle::from_memory_package(package, actual)?;
        let bench = self
            .benches
            .entry(instance.clone())
            .or_insert_with(|| Bench::new(instance.clone()));
        Ok(bench.loaded.insert(bundle))
    }

    pub fn get(&self, instance: &InstanceId) -> Option<&LoadedBundle> {
        self.benches
            .get(instance)
            .and_then(|bench| bench.loaded.as_ref())
    }

    pub fn unload(&mut self, instance: &InstanceId) {
        if let Some(bench) = self.benches.get_mut(instance) {
            bench.loaded = None;
        }
    }

    pub fn take_loaded(&mut self, instance: &InstanceId) -> Option<LoadedBundle> {
        self.benches
            .get_mut(instance)
            .and_then(|bench| bench.loaded.take())
    }
}

/// Validates recognition and page metadata against a caller-owned asset resolver.
pub fn validate_recognition_metadata(
    pack_path: &str,
    pack_json: &str,
    pages_path: &str,
    pages_json: &str,
    asset_resolver: Arc<dyn AssetResolver>,
) -> ContainmentResult<Vec<UnsupportedRecognitionTarget>> {
    let (evaluator, _) =
        build_recognition_pipeline(pack_path, pack_json, pages_path, pages_json, asset_resolver)?;
    Ok(evaluator.unsupported_targets().to_vec())
}

#[derive(Debug)]
struct Bench {
    #[allow(dead_code)]
    instance: InstanceId,
    loaded: Option<LoadedBundle>,
}

impl Bench {
    fn new(instance: InstanceId) -> Self {
        Self {
            instance,
            loaded: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageLayout {
    Lab,
    Module,
}

#[derive(Debug)]
pub struct LoadedBundle {
    task_id: TaskId,
    verified: Sha256Hash,
    layout: PackageLayout,
    entries: Arc<BTreeMap<String, Vec<u8>>>,
    entry_count: usize,
    resident_bytes: u64,
    resource_root: String,
    manifest_path: String,
    manifest: Value,
    operation_path: String,
    recognition_pack_path: Option<String>,
    pages_path: Option<String>,
    navigation_path: Option<String>,
    admitted: Option<AdmittedPackage>,
    recognition_pack_diagnostics: Vec<RecognitionPackDiagnostics>,
}

impl LoadedBundle {
    fn from_memory_package(
        package: MemoryPackage,
        verified: Sha256Hash,
    ) -> ContainmentResult<Self> {
        let entries = Arc::new(package.entries);
        let metadata = PackageMetadata::from_entries(&entries)?;
        validate_manifest_hashes(&metadata.manifest, &entries, &metadata.resource_root)?;
        let closed = admission::parse_package(&entries, &metadata)?
            .map(|parsed| admission::close_package(parsed, &entries, &metadata.resource_root))
            .transpose()
            .map_err(ContainmentError::Admission)?;
        let recognition_pack_diagnostics = collect_recognition_pack_diagnostics(&entries)?;
        let (evaluator, detector) = load_recognition_pipeline(&entries, &metadata)?;
        let admitted = match closed {
            Some(closed) => {
                let evaluator = evaluator.clone().ok_or_else(|| {
                    package_contract_error(
                        "admission",
                        "closed executable package is missing its recognition evaluator",
                    )
                })?;
                let detector = detector.clone().ok_or_else(|| {
                    package_contract_error(
                        "admission",
                        "closed executable package is missing its page detector",
                    )
                })?;
                Some(
                    admission::admit_package(closed, evaluator, detector)
                        .map_err(ContainmentError::Admission)?,
                )
            }
            None => None,
        };
        Ok(Self {
            task_id: metadata.task_id,
            verified,
            layout: metadata.layout,
            entries,
            entry_count: package.entry_count,
            resident_bytes: package.resident_bytes,
            resource_root: metadata.resource_root,
            manifest_path: metadata.manifest_path,
            manifest: metadata.manifest,
            operation_path: metadata.operation_path,
            recognition_pack_path: metadata.recognition_pack_path,
            pages_path: metadata.pages_path,
            navigation_path: metadata.navigation_path,
            admitted,
            recognition_pack_diagnostics,
        })
    }

    pub fn task_id(&self) -> &TaskId {
        &self.task_id
    }

    pub fn verified_hash(&self) -> Sha256Hash {
        self.verified
    }

    pub fn layout(&self) -> PackageLayout {
        self.layout
    }

    pub fn entry_count(&self) -> usize {
        self.entry_count
    }

    pub fn resident_bytes(&self) -> u64 {
        self.resident_bytes
    }

    pub fn resource_root(&self) -> &str {
        &self.resource_root
    }

    pub fn manifest_path(&self) -> &str {
        &self.manifest_path
    }

    pub fn manifest(&self) -> &Value {
        &self.manifest
    }

    pub fn operation_path(&self) -> &str {
        &self.operation_path
    }

    pub fn recognition_pack_path(&self) -> Option<&str> {
        self.recognition_pack_path.as_deref()
    }

    pub fn pages_path(&self) -> Option<&str> {
        self.pages_path.as_deref()
    }

    pub fn navigation_path(&self) -> Option<&str> {
        self.navigation_path.as_deref()
    }

    pub fn admitted_package(&self) -> Option<&AdmittedPackage> {
        self.admitted.as_ref()
    }

    pub fn recognition_pack_diagnostics(&self) -> &[RecognitionPackDiagnostics] {
        &self.recognition_pack_diagnostics
    }

    pub fn entry(&self, path: &str) -> Option<&[u8]> {
        self.entries.get(path).map(Vec::as_slice)
    }

    pub fn entry_paths(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    pub fn task_count(&self) -> usize {
        let prefix = prefixed_path(&self.resource_root, "operations/");
        self.entries
            .keys()
            .filter(|path| path.starts_with(&prefix) && path.ends_with("/task.json"))
            .count()
    }

    pub fn resource_entry(&self, relative_path: &str) -> ContainmentResult<&[u8]> {
        validate_relative_ref(relative_path)?;
        let path = prefixed_path(&self.resource_root, relative_path);
        self.entry(&path)
            .ok_or(ContainmentError::MissingEntry { path })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecognitionPackDiagnostics {
    pub path: String,
    pub unsupported_targets: Vec<UnsupportedRecognitionTarget>,
}

#[derive(Debug)]
struct MemoryPackage {
    entries: BTreeMap<String, Vec<u8>>,
    entry_count: usize,
    resident_bytes: u64,
}

impl MemoryPackage {
    fn from_zip(
        zip_bytes: &[u8],
        limits: ContainmentLimits,
        instance: &InstanceId,
    ) -> ContainmentResult<Self> {
        let cursor = Cursor::new(zip_bytes);
        let mut archive =
            ZipArchive::new(cursor).map_err(|err| ContainmentError::MalformedZip {
                message: err.to_string(),
            })?;
        if archive.len() > limits.max_entry_count {
            return Err(ContainmentError::EntryCountTooLarge {
                instance: instance.clone(),
                count: archive.len(),
                limit: limits.max_entry_count,
            });
        }

        let mut entries = BTreeMap::new();
        let mut seen = BTreeSet::new();
        let mut resident_bytes = 0_u64;

        for index in 0..archive.len() {
            let mut entry =
                archive
                    .by_index(index)
                    .map_err(|err| ContainmentError::MalformedZip {
                        message: err.to_string(),
                    })?;
            let Some(path) = normalize_zip_path(entry.name())? else {
                continue;
            };
            let duplicate_key = path.to_ascii_lowercase();
            if !seen.insert(duplicate_key) {
                return Err(ContainmentError::DuplicateEntry { path });
            }
            if has_dangerous_extension(&path) {
                return Err(ContainmentError::ForbiddenEntry { path });
            }
            let read_limit = limits
                .max_entry_bytes
                .min(
                    limits
                        .max_total_decompressed_bytes
                        .saturating_sub(resident_bytes),
                )
                .min(
                    limits
                        .max_resident_bytes_per_instance
                        .saturating_sub(resident_bytes),
                );
            if entry.size() > read_limit {
                return Err(ContainmentError::DecompressTooLarge {
                    instance: instance.clone(),
                    path,
                    size: entry.size(),
                    limit: read_limit,
                });
            }
            let bytes = read_entry_limited(&mut entry, instance, &path, read_limit)?;
            resident_bytes = resident_bytes.checked_add(bytes.len() as u64).ok_or(
                ContainmentError::DecompressTooLarge {
                    instance: instance.clone(),
                    path: path.clone(),
                    size: u64::MAX,
                    limit: limits.max_total_decompressed_bytes,
                },
            )?;
            if resident_bytes > limits.max_total_decompressed_bytes
                || resident_bytes > limits.max_resident_bytes_per_instance
            {
                return Err(ContainmentError::DecompressTooLarge {
                    instance: instance.clone(),
                    path,
                    size: resident_bytes,
                    limit: limits
                        .max_total_decompressed_bytes
                        .min(limits.max_resident_bytes_per_instance),
                });
            }
            entries.insert(path, bytes);
        }

        Ok(Self {
            entry_count: entries.len(),
            entries,
            resident_bytes,
        })
    }
}

#[derive(Debug)]
struct PackageMetadata {
    layout: PackageLayout,
    task_id: TaskId,
    resource_root: String,
    manifest_path: String,
    manifest: Value,
    operation_path: String,
    recognition_pack_path: Option<String>,
    pages_path: Option<String>,
    navigation_path: Option<String>,
}

impl PackageMetadata {
    fn from_entries(entries: &BTreeMap<String, Vec<u8>>) -> ContainmentResult<Self> {
        if entries.contains_key("control.json") {
            return Self::from_lab_entries(entries);
        }
        Self::from_module_entries(entries)
    }

    fn from_lab_entries(entries: &BTreeMap<String, Vec<u8>>) -> ContainmentResult<Self> {
        let control: LabControl = read_json_entry(entries, "control.json")?;
        let resource_root = match control.resource_root {
            None => LAB_RESOURCE_ROOT.to_string(),
            Some(resource_root) if resource_root == LAB_RESOURCE_ROOT => resource_root,
            Some(resource_root) => {
                return Err(ContainmentError::UnsupportedResourceRoot {
                    value: resource_root,
                });
            }
        };
        let manifest_path = prefixed_path(&resource_root, "manifest.json");
        let manifest = read_json_value_entry(entries, &manifest_path)?;
        if let Some(manifest_task_id) = manifest.get("entry_task_id").and_then(Value::as_str)
            && manifest_task_id != control.entry_task_id
        {
            return Err(ContainmentError::ManifestTaskConflict {
                manifest_path: manifest_path.clone(),
                manifest_task_id: manifest_task_id.to_string(),
                control_task_id: control.entry_task_id,
            });
        }
        let task_id = TaskId::new(control.entry_task_id)?;
        let operation_path =
            prefixed_path(&resource_root, &format!("operations/{task_id}/task.json"));
        let _: Value = read_json_value_entry(entries, &operation_path)?;
        let stem = format!("{}.{}", control.game, control.server);
        let recognition_pack_path =
            prefixed_path(&resource_root, &format!("recognition/{stem}.pack.json"));
        let pages_path = prefixed_path(&resource_root, &format!("recognition/{stem}.pages.json"));
        let navigation_path = prefixed_path(
            &resource_root,
            &format!("navigation/{stem}.navigation.json"),
        );
        Ok(Self {
            layout: PackageLayout::Lab,
            task_id,
            resource_root,
            manifest_path,
            manifest,
            operation_path,
            recognition_pack_path: entries
                .contains_key(&recognition_pack_path)
                .then_some(recognition_pack_path),
            pages_path: entries.contains_key(&pages_path).then_some(pages_path),
            navigation_path: entries
                .contains_key(&navigation_path)
                .then_some(navigation_path),
        })
    }

    fn from_module_entries(entries: &BTreeMap<String, Vec<u8>>) -> ContainmentResult<Self> {
        let module = single_top_level_module(entries)?;
        let manifest_path = prefixed_path(&module, "manifest.json");
        let manifest = read_json_value_entry(entries, &manifest_path)?;
        let task_id = task_id_from_manifest_or_operations(&manifest, entries, &module)?;
        let operation_path = prefixed_path(&module, &format!("operations/{task_id}/task.json"));
        let _: Value = read_json_value_entry(entries, &operation_path)?;
        Ok(Self {
            layout: PackageLayout::Module,
            task_id,
            resource_root: module,
            manifest_path,
            manifest,
            operation_path,
            recognition_pack_path: None,
            pages_path: None,
            navigation_path: None,
        })
    }
}

#[derive(Debug, Deserialize)]
struct LabControl {
    game: String,
    server: String,
    entry_task_id: String,
    #[serde(default)]
    resource_root: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ManifestFile {
    path: String,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default)]
    hash: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ManifestHashes {
    #[serde(default)]
    hashes: BTreeMap<String, String>,
    #[serde(default)]
    files: Vec<ManifestFile>,
}

#[derive(Debug)]
struct MemoryAssetResolver {
    entries: Arc<BTreeMap<String, Vec<u8>>>,
    resource_root: String,
}

impl AssetResolver for MemoryAssetResolver {
    fn read_asset(
        &self,
        path: &str,
    ) -> actingcommand_recognition_pack::RecognitionPackResult<Vec<u8>> {
        validate_relative_ref(path).map_err(|err| {
            actingcommand_recognition_pack::RecognitionPackError::fatal(err.to_string())
        })?;
        let resolved = prefixed_path(&self.resource_root, path);
        if let Some(bytes) = self.entries.get(&resolved) {
            return Ok(bytes.clone());
        }
        Err(actingcommand_recognition_pack::RecognitionPackError::fatal(
            format!("memory asset '{path}' does not exist"),
        ))
    }

    fn contains_asset(&self, path: &str) -> bool {
        validate_relative_ref(path).is_ok()
            && self
                .entries
                .contains_key(&prefixed_path(&self.resource_root, path))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainmentError {
    InvalidInstanceId,
    MissingTaskId,
    InvalidHash {
        value: String,
    },
    HashMismatch {
        instance: InstanceId,
        expected: Sha256Hash,
        actual: Sha256Hash,
    },
    CompressedTooLarge {
        instance: InstanceId,
        size: u64,
        limit: u64,
    },
    EntryCountTooLarge {
        instance: InstanceId,
        count: usize,
        limit: usize,
    },
    DecompressTooLarge {
        instance: InstanceId,
        path: String,
        size: u64,
        limit: u64,
    },
    PathTraversal {
        path: String,
    },
    DuplicateEntry {
        path: String,
    },
    ForbiddenEntry {
        path: String,
    },
    MalformedZip {
        message: String,
    },
    MissingEntry {
        path: String,
    },
    JsonParse {
        path: String,
        message: String,
    },
    ManifestHashMismatch {
        path: String,
        expected: Sha256Hash,
        actual: Sha256Hash,
    },
    UnsafeManifestHashPath,
    UnsupportedResourceRoot {
        value: String,
    },
    ManifestTaskConflict {
        manifest_path: String,
        manifest_task_id: String,
        control_task_id: String,
    },
    PackageContract {
        path: String,
        message: String,
    },
    PackParse {
        path: String,
        message: String,
    },
    Admission(AdmissionError),
}

impl fmt::Display for ContainmentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInstanceId => f.write_str("fatal containment error: instance id is empty"),
            Self::MissingTaskId => f.write_str("fatal containment error: task id is missing"),
            Self::InvalidHash { value } => write!(
                f,
                "fatal containment error: invalid sha256 hash '{value}'; hash mismatch cannot be evaluated"
            ),
            Self::HashMismatch {
                instance,
                expected,
                actual,
            } => write!(
                f,
                "fatal containment error: hash mismatch for instance {instance}: expected {expected}, actual {actual}"
            ),
            Self::CompressedTooLarge {
                instance,
                size,
                limit,
            } => write!(
                f,
                "fatal containment error: compressed package for instance {instance} is {size} bytes, limit {limit}"
            ),
            Self::EntryCountTooLarge {
                instance,
                count,
                limit,
            } => write!(
                f,
                "fatal containment error: package for instance {instance} has {count} entries, limit {limit}"
            ),
            Self::DecompressTooLarge {
                instance,
                path,
                size,
                limit,
            } => write!(
                f,
                "fatal containment error: package for instance {instance} exceeds decompression limit at {path}: {size} > {limit}"
            ),
            Self::PathTraversal { path } => write!(
                f,
                "fatal containment error: package path escapes containment: {path}"
            ),
            Self::DuplicateEntry { path } => {
                write!(
                    f,
                    "fatal containment error: duplicate package entry: {path}"
                )
            }
            Self::ForbiddenEntry { path } => {
                write!(
                    f,
                    "fatal containment error: executable/script entry is forbidden: {path}"
                )
            }
            Self::MalformedZip { message } => {
                write!(f, "fatal containment error: malformed zip: {message}")
            }
            Self::MissingEntry { path } => {
                write!(f, "fatal containment error: missing package entry: {path}")
            }
            Self::JsonParse { path, message } => {
                write!(
                    f,
                    "fatal containment error: failed to parse {path}: {message}"
                )
            }
            Self::ManifestHashMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "fatal containment error: manifest hash mismatch for {path}: expected {expected}, actual {actual}"
            ),
            Self::UnsafeManifestHashPath => {
                f.write_str("fatal containment error: manifest hash path is unsafe")
            }
            Self::UnsupportedResourceRoot { value } => write!(
                f,
                "fatal containment error: Lab control resource_root must be omitted or exactly '{LAB_RESOURCE_ROOT}', got '{value}'"
            ),
            Self::ManifestTaskConflict {
                manifest_path,
                manifest_task_id,
                control_task_id,
            } => write!(
                f,
                "fatal containment error: {manifest_path} entry_task_id '{manifest_task_id}' conflicts with control entry_task_id '{control_task_id}'"
            ),
            Self::PackageContract { path, message } => write!(
                f,
                "fatal containment error: invalid package contract in {path}: {message}"
            ),
            Self::PackParse { path, message } => {
                write!(
                    f,
                    "fatal containment error: failed to parse {path}: {message}"
                )
            }
            Self::Admission(error) => {
                write!(
                    f,
                    "fatal containment error: executable package admission failed: {error}"
                )
            }
        }
    }
}

impl Error for ContainmentError {}

fn package_contract_error(path: impl Into<String>, message: impl Into<String>) -> ContainmentError {
    ContainmentError::PackageContract {
        path: path.into(),
        message: message.into(),
    }
}

fn load_recognition_pipeline(
    entries: &Arc<BTreeMap<String, Vec<u8>>>,
    metadata: &PackageMetadata,
) -> ContainmentResult<(Option<RecognitionEvaluator>, Option<PageDetector>)> {
    let Some(pack_path) = &metadata.recognition_pack_path else {
        return Ok((None, None));
    };
    let pack_json = decode_utf8_entry(entries, pack_path)?;
    let resolver = Arc::new(MemoryAssetResolver {
        entries: Arc::clone(entries),
        resource_root: metadata.resource_root.clone(),
    });
    let Some(pages_path) = &metadata.pages_path else {
        let pack =
            load_pack_from_json_str(pack_json.trim_start_matches('\u{feff}')).map_err(|err| {
                ContainmentError::PackParse {
                    path: pack_path.clone(),
                    message: err.to_string(),
                }
            })?;
        let evaluator =
            RecognitionEvaluator::with_asset_resolver(pack, resolver).map_err(|err| {
                ContainmentError::PackParse {
                    path: pack_path.clone(),
                    message: err.to_string(),
                }
            })?;
        return Ok((Some(evaluator), None));
    };
    let pages_json = decode_utf8_entry(entries, pages_path)?;
    let (evaluator, detector) =
        build_recognition_pipeline(pack_path, pack_json, pages_path, pages_json, resolver)?;
    Ok((Some(evaluator), Some(detector)))
}

fn build_recognition_pipeline(
    pack_path: &str,
    pack_json: &str,
    pages_path: &str,
    pages_json: &str,
    asset_resolver: Arc<dyn AssetResolver>,
) -> ContainmentResult<(RecognitionEvaluator, PageDetector)> {
    let pack =
        load_pack_from_json_str(pack_json.trim_start_matches('\u{feff}')).map_err(|err| {
            ContainmentError::PackParse {
                path: pack_path.to_string(),
                message: err.to_string(),
            }
        })?;
    let evaluator =
        RecognitionEvaluator::with_asset_resolver(pack, asset_resolver).map_err(|err| {
            ContainmentError::PackParse {
                path: pack_path.to_string(),
                message: err.to_string(),
            }
        })?;
    let page_set =
        load_page_set_from_json_str(pages_json.trim_start_matches('\u{feff}')).map_err(|err| {
            ContainmentError::PackParse {
                path: pages_path.to_string(),
                message: err.to_string(),
            }
        })?;
    let detector = PageDetector::new(page_set).map_err(|err| ContainmentError::PackParse {
        path: pages_path.to_string(),
        message: err.to_string(),
    })?;
    detector
        .validate(&evaluator)
        .map_err(|err| ContainmentError::PackParse {
            path: pages_path.to_string(),
            message: err.to_string(),
        })?;
    Ok((evaluator, detector))
}

fn collect_recognition_pack_diagnostics(
    entries: &BTreeMap<String, Vec<u8>>,
) -> ContainmentResult<Vec<RecognitionPackDiagnostics>> {
    let mut diagnostics = Vec::new();
    for (path, bytes) in entries {
        if !path.ends_with(".pack.json") {
            continue;
        }
        let text = std::str::from_utf8(bytes).map_err(|err| ContainmentError::PackParse {
            path: path.clone(),
            message: err.to_string(),
        })?;
        let pack = load_pack_from_json_str(text).map_err(|err| ContainmentError::PackParse {
            path: path.clone(),
            message: err.to_string(),
        })?;
        diagnostics.push(RecognitionPackDiagnostics {
            path: path.clone(),
            unsupported_targets: unsupported_recognition_targets(&pack),
        });
    }
    Ok(diagnostics)
}

fn validate_manifest_hashes(
    manifest: &Value,
    entries: &BTreeMap<String, Vec<u8>>,
    resource_root: &str,
) -> ContainmentResult<()> {
    let hashes: ManifestHashes =
        serde_json::from_value(manifest.clone()).map_err(|err| ContainmentError::JsonParse {
            path: prefixed_path(resource_root, "manifest.json"),
            message: err.to_string(),
        })?;
    for (path, expected) in hashes
        .hashes
        .iter()
        .map(|(path, hash)| (path.as_str(), hash.as_str()))
        .chain(hashes.files.iter().filter_map(|file| {
            file.sha256
                .as_deref()
                .or(file.hash.as_deref())
                .map(|hash| (file.path.as_str(), hash))
        }))
    {
        validate_manifest_hash_entry(entries, resource_root, path, expected)?;
    }
    Ok(())
}

fn validate_manifest_hash_entry(
    entries: &BTreeMap<String, Vec<u8>>,
    resource_root: &str,
    path: &str,
    expected: &str,
) -> ContainmentResult<()> {
    validate_manifest_hash_path(path)?;
    let resolved = prefixed_path(resource_root, path);
    let bytes = entries
        .get(&resolved)
        .ok_or_else(|| ContainmentError::MissingEntry {
            path: resolved.clone(),
        })?;
    let expected = Sha256Hash::parse_hex(expected)?;
    let actual = Sha256Hash::digest(bytes);
    if !constant_time_hash_eq(&actual, &expected) {
        return Err(ContainmentError::ManifestHashMismatch {
            path: resolved,
            expected,
            actual,
        });
    }
    Ok(())
}

fn validate_manifest_hash_path(path: &str) -> ContainmentResult<()> {
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
        return Err(ContainmentError::UnsafeManifestHashPath);
    }
    Ok(())
}

fn task_id_from_manifest_or_operations(
    manifest: &Value,
    entries: &BTreeMap<String, Vec<u8>>,
    resource_root: &str,
) -> ContainmentResult<TaskId> {
    if let Some(entry_task_id) = manifest.get("entry_task_id").and_then(Value::as_str) {
        return TaskId::new(entry_task_id);
    }
    let prefix = prefixed_path(resource_root, "operations/");
    let task = entries
        .keys()
        .filter_map(|path| {
            path.strip_prefix(&prefix)
                .and_then(|rest| rest.strip_suffix("/task.json"))
        })
        .next()
        .ok_or(ContainmentError::MissingTaskId)?;
    TaskId::new(task)
}

fn single_top_level_module(entries: &BTreeMap<String, Vec<u8>>) -> ContainmentResult<String> {
    let roots = entries
        .keys()
        .filter_map(|path| path.split('/').next())
        .collect::<BTreeSet<_>>();
    if roots.len() != 1 {
        return Err(ContainmentError::MalformedZip {
            message: "package must contain exactly one top-level module directory".to_string(),
        });
    }
    roots
        .into_iter()
        .next()
        .map(str::to_string)
        .ok_or_else(|| ContainmentError::MalformedZip {
            message: "package must contain exactly one top-level module directory".to_string(),
        })
}

fn read_json_value_entry(
    entries: &BTreeMap<String, Vec<u8>>,
    path: &str,
) -> ContainmentResult<Value> {
    let bytes = entries
        .get(path)
        .ok_or_else(|| ContainmentError::MissingEntry {
            path: path.to_string(),
        })?;
    serde_json::from_slice(bytes).map_err(|err| ContainmentError::JsonParse {
        path: path.to_string(),
        message: err.to_string(),
    })
}

fn read_json_entry<T: for<'de> Deserialize<'de>>(
    entries: &BTreeMap<String, Vec<u8>>,
    path: &str,
) -> ContainmentResult<T> {
    let bytes = entries
        .get(path)
        .ok_or_else(|| ContainmentError::MissingEntry {
            path: path.to_string(),
        })?;
    serde_json::from_slice(bytes).map_err(|err| ContainmentError::JsonParse {
        path: path.to_string(),
        message: err.to_string(),
    })
}

fn decode_utf8_entry<'a>(
    entries: &'a BTreeMap<String, Vec<u8>>,
    path: &str,
) -> ContainmentResult<&'a str> {
    let bytes = entries
        .get(path)
        .ok_or_else(|| ContainmentError::MissingEntry {
            path: path.to_string(),
        })?;
    std::str::from_utf8(bytes).map_err(|err| ContainmentError::JsonParse {
        path: path.to_string(),
        message: err.to_string(),
    })
}

fn read_entry_limited<R: Read>(
    reader: &mut R,
    instance: &InstanceId,
    path: &str,
    limit: u64,
) -> ContainmentResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut limited = reader.take(limit.saturating_add(1));
    limited
        .read_to_end(&mut bytes)
        .map_err(|err| ContainmentError::MalformedZip {
            message: format!("failed to read zip entry {path} for instance {instance}: {err}"),
        })?;
    if bytes.len() as u64 > limit {
        return Err(ContainmentError::DecompressTooLarge {
            instance: instance.clone(),
            path: path.to_string(),
            size: bytes.len() as u64,
            limit,
        });
    }
    Ok(bytes)
}

fn normalize_zip_path(name: &str) -> ContainmentResult<Option<String>> {
    if name.ends_with('/') {
        return Ok(None);
    }
    validate_relative_ref(name)?;
    Ok(Some(name.to_string()))
}

fn validate_relative_ref(path: &str) -> ContainmentResult<()> {
    if path.ends_with('/') || path.contains('\\') || path.contains(':') || path.starts_with('/') {
        return Err(ContainmentError::PathTraversal {
            path: path.to_string(),
        });
    }
    if Path::new(path).components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(ContainmentError::PathTraversal {
            path: path.to_string(),
        });
    }
    Ok(())
}

fn prefixed_path(prefix: &str, path: &str) -> String {
    format!(
        "{}/{}",
        prefix.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn has_dangerous_extension(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            DANGEROUS_EXTENSIONS
                .iter()
                .any(|dangerous| extension.eq_ignore_ascii_case(dangerous))
        })
}

fn constant_time_hash_eq(actual: &Sha256Hash, expected: &Sha256Hash) -> bool {
    let mut diff = 0_u8;
    for (actual, expected) in actual.as_bytes().iter().zip(expected.as_bytes()) {
        diff |= actual ^ expected;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_recognition::{Scene, ScenePixelFormat};
    use std::io::{self, Write};
    use zip::write::FileOptions;

    fn next_fuzz_word(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    #[test]
    fn bounded_arbitrary_zip_bytes_are_panic_free_and_deterministic() {
        const CASES: usize = 256;
        let mut state = 0x68_5eed_fade_cafe_u64;
        for case in 0..CASES {
            let length = (next_fuzz_word(&mut state) % 513) as usize;
            let mut bytes = vec![0_u8; length];
            for byte in &mut bytes {
                *byte = next_fuzz_word(&mut state) as u8;
            }
            let expected = Sha256Hash::digest(&bytes);
            let evaluate = || {
                let instance =
                    InstanceId::new(format!("fuzz-{case}")).expect("bounded fuzz instance id");
                let mut containment = Containment::new();
                match containment.load(&instance, &bytes, &expected) {
                    Ok(_) => "ok".to_string(),
                    Err(error) => format!("error:{error}"),
                }
            };
            let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(evaluate))
                .unwrap_or_else(|_| panic!("arbitrary ZIP case {case} panicked"));
            let second = std::panic::catch_unwind(std::panic::AssertUnwindSafe(evaluate))
                .unwrap_or_else(|_| panic!("arbitrary ZIP replay case {case} panicked"));
            assert_eq!(first, second, "arbitrary ZIP case {case} drifted");
        }
    }

    #[test]
    fn load_single_lab_package_and_evaluate_from_capability() {
        let zip = lab_package_zip("task_a", [255, 0, 0]);
        let expected = Sha256Hash::digest(&zip);
        let instance = InstanceId::new("127.0.0.1:16384").expect("instance");
        let mut containment = Containment::new();

        let bundle = containment
            .load(&instance, &zip, &expected)
            .expect("bundle loaded");

        assert_eq!(bundle.task_id().as_str(), "task_a");
        assert_eq!(bundle.verified_hash(), expected);
        let evaluator = bundle
            .admitted_package()
            .expect("admitted package")
            .evaluator();
        let scene = Scene::from_pixels(1, 1, &[255, 0, 0], ScenePixelFormat::Rgb8).expect("scene");
        let result = evaluator
            .evaluate_target(&scene, "home_color")
            .expect("target evaluated");
        assert!(result.passed);
    }

    #[test]
    fn benches_are_isolated_by_instance() {
        let zip_a = lab_package_zip("task_a", [255, 0, 0]);
        let zip_b = lab_package_zip("task_b", [0, 255, 0]);
        let a = InstanceId::new("a").expect("a");
        let b = InstanceId::new("b").expect("b");
        let mut containment = Containment::new();

        containment
            .load(&a, &zip_a, &Sha256Hash::digest(&zip_a))
            .expect("a loaded");
        containment
            .load(&b, &zip_b, &Sha256Hash::digest(&zip_b))
            .expect("b loaded");

        assert_eq!(
            containment.get(&a).expect("a bundle").task_id().as_str(),
            "task_a"
        );
        assert_eq!(
            containment.get(&b).expect("b bundle").task_id().as_str(),
            "task_b"
        );
    }

    #[test]
    fn rejects_hash_mismatch_before_decompress() {
        let zip = lab_package_zip("task_a", [255, 0, 0]);
        let wrong = Sha256Hash::digest(b"wrong bytes");
        let instance = InstanceId::new("inst").expect("instance");
        let mut containment = Containment::new();

        let err = containment
            .load(&instance, &zip, &wrong)
            .expect_err("hash mismatch rejected");

        assert!(matches!(err, ContainmentError::HashMismatch { .. }));
    }

    #[test]
    fn rejects_decompression_limit_without_oom() {
        let zip = zip_with_entries(&[("module/manifest.json", br#"{}"#.as_slice())]);
        let expected = Sha256Hash::digest(&zip);
        let instance = InstanceId::new("inst").expect("instance");
        let mut containment = Containment::with_limits(ContainmentLimits {
            max_total_decompressed_bytes: 1,
            max_resident_bytes_per_instance: 1,
            ..ContainmentLimits::default()
        });

        let err = containment
            .load(&instance, &zip, &expected)
            .expect_err("limit rejected");

        assert!(matches!(err, ContainmentError::DecompressTooLarge { .. }));
    }

    #[test]
    fn entry_read_limit_uses_remaining_total_and_resident_budgets() {
        let zip = zip_with_entries(&[
            ("module/a.bin", b"1234".as_slice()),
            ("module/b.bin", b"567".as_slice()),
        ]);
        let expected = Sha256Hash::digest(&zip);
        let instance = InstanceId::new("budget-instance").expect("instance");

        for (total_limit, resident_limit) in [(5, 10), (10, 5)] {
            let mut containment = Containment::with_limits(ContainmentLimits {
                max_total_decompressed_bytes: total_limit,
                max_entry_bytes: 10,
                max_resident_bytes_per_instance: resident_limit,
                ..ContainmentLimits::default()
            });
            let err = containment
                .load(&instance, &zip, &expected)
                .expect_err("second entry must exceed the remaining budget");

            assert_eq!(
                err,
                ContainmentError::DecompressTooLarge {
                    instance: instance.clone(),
                    path: "module/b.bin".to_string(),
                    size: 3,
                    limit: 1,
                }
            );
        }
    }

    #[test]
    fn entry_read_failure_reports_real_instance_and_path() {
        struct FailingReader;
        impl Read for FailingReader {
            fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("fixture read failure"))
            }
        }

        let instance = InstanceId::new("reader-instance").expect("instance");
        let err = read_entry_limited(&mut FailingReader, &instance, "module/failing.bin", 16)
            .expect_err("read failure must propagate");
        let message = err.to_string();

        assert!(message.contains("reader-instance"));
        assert!(message.contains("module/failing.bin"));
        assert!(!message.contains("<unknown>"));
    }

    #[test]
    fn entry_overflow_reports_real_instance_and_path() {
        let instance = InstanceId::new("overflow-instance").expect("instance");
        let err = read_entry_limited(
            &mut Cursor::new(b"too-large"),
            &instance,
            "module/large.bin",
            1,
        )
        .expect_err("bounded reader must reject limit plus one byte");

        assert_eq!(
            err,
            ContainmentError::DecompressTooLarge {
                instance,
                path: "module/large.bin".to_string(),
                size: 2,
                limit: 1,
            }
        );
    }

    #[test]
    fn rejects_zip_slip_path() {
        let zip = zip_with_entries(&[
            ("module/manifest.json", br#"{}"#.as_slice()),
            ("../outside", b"x".as_slice()),
        ]);
        let expected = Sha256Hash::digest(&zip);
        let instance = InstanceId::new("inst").expect("instance");
        let mut containment = Containment::new();

        let err = containment
            .load(&instance, &zip, &expected)
            .expect_err("zip slip rejected");

        assert!(matches!(err, ContainmentError::PathTraversal { .. }));
    }

    #[test]
    fn same_instance_second_load_replaces_and_unload_clears() {
        let zip_a = lab_package_zip("task_a", [255, 0, 0]);
        let zip_b = lab_package_zip("task_b", [0, 255, 0]);
        let instance = InstanceId::new("inst").expect("instance");
        let mut containment = Containment::new();

        containment
            .load(&instance, &zip_a, &Sha256Hash::digest(&zip_a))
            .expect("first load");
        containment
            .load(&instance, &zip_b, &Sha256Hash::digest(&zip_b))
            .expect("second load");

        assert_eq!(
            containment
                .get(&instance)
                .expect("replacement")
                .task_id()
                .as_str(),
            "task_b"
        );
        containment.unload(&instance);
        assert!(containment.get(&instance).is_none());
    }

    #[test]
    fn rejects_manifest_hash_mismatch() {
        let mut entries = lab_package_entries("task_a", [255, 0, 0]);
        entries.insert(
            "resources/manifest.json".to_string(),
            br#"{"entry_task_id":"task_a","files":[{"path":"operations/task_a/task.json","sha256":"sha256:0000000000000000000000000000000000000000000000000000000000000000"}]}"#.to_vec(),
        );
        let zip = zip_from_map(entries);
        let expected = Sha256Hash::digest(&zip);
        let instance = InstanceId::new("inst").expect("instance");
        let mut containment = Containment::new();

        let err = containment
            .load(&instance, &zip, &expected)
            .expect_err("manifest mismatch rejected");

        assert!(matches!(err, ContainmentError::ManifestHashMismatch { .. }));
    }

    #[test]
    fn manifest_hash_cannot_be_satisfied_by_bare_path_shadow() {
        let mut entries = lab_package_entries("task_a", [255, 0, 0]);
        let resource_path = "resources/operations/task_a/task.json";
        let official = entries
            .get(resource_path)
            .expect("official operation")
            .clone();
        let shadow = br#"{"shadow":true}"#.to_vec();
        let shadow_hash = Sha256Hash::digest(&shadow);
        entries.insert("operations/task_a/task.json".to_string(), shadow);
        entries.insert(
            "resources/manifest.json".to_string(),
            format!(
                r#"{{"entry_task_id":"task_a","files":[{{"path":"operations/task_a/task.json","sha256":"sha256:{shadow_hash}"}}]}}"#
            )
            .into_bytes(),
        );
        let zip = zip_from_map(entries);
        let expected = Sha256Hash::digest(&zip);
        let instance = InstanceId::new("shadow-instance").expect("instance");
        let mut containment = Containment::new();

        let err = containment
            .load(&instance, &zip, &expected)
            .expect_err("bare path must not satisfy a resource-root hash");

        assert_eq!(
            err,
            ContainmentError::ManifestHashMismatch {
                path: resource_path.to_string(),
                expected: shadow_hash,
                actual: Sha256Hash::digest(&official),
            }
        );
    }

    #[test]
    fn memory_asset_resolver_never_reads_bare_path_shadow() {
        let entries = Arc::new(BTreeMap::from([
            ("templates/target.bin".to_string(), b"shadow".to_vec()),
            (
                "resources/templates/target.bin".to_string(),
                b"official".to_vec(),
            ),
        ]));
        let resolver = MemoryAssetResolver {
            entries,
            resource_root: LAB_RESOURCE_ROOT.to_string(),
        };

        assert_eq!(
            resolver.read_asset("templates/target.bin").expect("asset"),
            b"official"
        );
        assert!(resolver.contains_asset("templates/target.bin"));

        let bare_only = MemoryAssetResolver {
            entries: Arc::new(BTreeMap::from([(
                "templates/target.bin".to_string(),
                b"shadow".to_vec(),
            )])),
            resource_root: LAB_RESOURCE_ROOT.to_string(),
        };
        assert!(!bare_only.contains_asset("templates/target.bin"));
        assert!(bare_only.read_asset("templates/target.bin").is_err());
    }

    #[test]
    fn lab_resource_root_allows_only_omitted_or_exact_resources() {
        for value in [
            "alternate",
            "./resources",
            "resources/",
            "Resources",
            "../resources",
            "",
        ] {
            let mut entries = lab_package_entries("task_a", [255, 0, 0]);
            entries.insert(
                "control.json".to_string(),
                format!(
                    r#"{{"game":"neutral","server":"test","entry_task_id":"task_a","resource_root":"{value}"}}"#
                )
                .into_bytes(),
            );
            let zip = zip_from_map(entries);
            let expected = Sha256Hash::digest(&zip);
            let instance = InstanceId::new("root-instance").expect("instance");
            let mut containment = Containment::new();

            let err = containment
                .load(&instance, &zip, &expected)
                .expect_err("custom root must be rejected");

            assert_eq!(
                err,
                ContainmentError::UnsupportedResourceRoot {
                    value: value.to_string(),
                }
            );
        }
    }

    #[test]
    fn exact_lab_resource_root_drives_all_metadata_paths() {
        let mut entries = lab_package_entries("task_a", [255, 0, 0]);
        entries.insert(
            "control.json".to_string(),
            br#"{"schema_version":"Lab-1y.control.v1","package_id":"neutral.task_a","execution_mode":"recognize_only","game":"neutral","server":"test","resolution":{"width":1,"height":1},"entry_task_id":"task_a","resource_root":"resources"}"#.to_vec(),
        );
        entries.insert(
            "resources/navigation/neutral.test.navigation.json".to_string(),
            br#"{"schema_version":"0.3","game":"neutral","server":"test","navigation":[],"destructive_actions":[]}"#.to_vec(),
        );
        let zip = zip_from_map(entries);
        let expected = Sha256Hash::digest(&zip);
        let instance = InstanceId::new("root-instance").expect("instance");
        let mut containment = Containment::new();

        let bundle = containment
            .load(&instance, &zip, &expected)
            .expect("exact resource root");

        assert_eq!(bundle.resource_root(), "resources");
        assert_eq!(bundle.manifest_path(), "resources/manifest.json");
        assert_eq!(
            bundle.operation_path(),
            "resources/operations/task_a/task.json"
        );
        assert_eq!(
            bundle.recognition_pack_path(),
            Some("resources/recognition/neutral.test.pack.json")
        );
        assert_eq!(
            bundle.pages_path(),
            Some("resources/recognition/neutral.test.pages.json")
        );
        assert_eq!(
            bundle.navigation_path(),
            Some("resources/navigation/neutral.test.navigation.json")
        );
    }

    #[test]
    fn zip_entry_order_does_not_change_canonical_semantics() {
        let entries = lab_package_entries("task_a", [255, 0, 0]);
        let forward = entries
            .iter()
            .map(|(path, bytes)| (path.clone(), bytes.clone()))
            .collect::<Vec<_>>();
        let reverse = forward.iter().cloned().rev().collect::<Vec<_>>();
        let forward_zip = zip_from_ordered_entries(forward);
        let reverse_zip = zip_from_ordered_entries(reverse);

        let fingerprint = |label: &str, bytes: &[u8]| {
            let instance = InstanceId::new(label).expect("instance");
            let mut containment = Containment::new();
            containment
                .load(&instance, bytes, &Sha256Hash::digest(bytes))
                .expect("admitted package")
                .admitted_package()
                .expect("canonical executable package")
                .semantic_fingerprint()
                .to_string()
        };
        assert_eq!(
            fingerprint("forward", &forward_zip),
            fingerprint("reverse", &reverse_zip)
        );
        assert_ne!(
            Sha256Hash::digest(&forward_zip),
            Sha256Hash::digest(&reverse_zip)
        );
    }

    #[test]
    fn lab_contract_rejects_old_manifest_and_navigation_schemas() {
        for (path, replacement) in [
            (
                "resources/manifest.json",
                br#"{"schema_version":"0.2","entry_task_id":"task_a"}"#.as_slice(),
            ),
            (
                "resources/navigation/neutral.test.navigation.json",
                br#"{"schema_version":"0.2","game":"neutral","server":"test","navigation":[],"destructive_actions":[]}"#.as_slice(),
            ),
        ] {
            let mut entries = lab_package_entries("task_a", [255, 0, 0]);
            entries.insert(path.to_string(), replacement.to_vec());
            let zip = zip_from_map(entries);
            let expected = Sha256Hash::digest(&zip);
            let instance = InstanceId::new("schema-instance").expect("instance");
            let mut containment = Containment::new();

            let error = containment
                .load(&instance, &zip, &expected)
                .expect_err("old schema must fail");

            assert!(matches!(error, ContainmentError::PackageContract { .. }));
        }
    }

    #[test]
    fn lab_contract_rejects_navigation_route_without_packaged_operation() {
        let mut entries = lab_package_entries("task_a", [255, 0, 0]);
        entries.insert(
            "resources/navigation/neutral.test.navigation.json".to_string(),
            br#"{"schema_version":"0.3","game":"neutral","server":"test","navigation":[{"id":"ghost_route","from_page":"neutral/home","to_page":"neutral/home","click":{"kind":"point","x":0,"y":0}}],"destructive_actions":[]}"#.to_vec(),
        );
        let zip = zip_from_map(entries);
        let expected = Sha256Hash::digest(&zip);
        let instance = InstanceId::new("closure-instance").expect("instance");
        let mut containment = Containment::new();

        let error = containment
            .load(&instance, &zip, &expected)
            .expect_err("dangling navigation route must fail");

        assert!(matches!(error, ContainmentError::Admission(_)));
        assert!(error.to_string().contains("ghost_route"));
    }

    fn lab_package_zip(task_id: &str, expected: [u8; 3]) -> Vec<u8> {
        zip_from_map(lab_package_entries(task_id, expected))
    }

    fn lab_package_entries(task_id: &str, expected: [u8; 3]) -> BTreeMap<String, Vec<u8>> {
        let operation = format!(
            r#"{{"schema_version":"0.5","task_id":"{task_id}","game":"neutral","server_scope":["test"],"coordinate_space":{{"width":1,"height":1}},"operations":[]}}"#
        )
        .into_bytes();
        let pack = format!(
            r#"{{"schema_version":"0.5","game":"neutral","server":"test","coordinate_space":{{"width":1,"height":1}},"targets":[{{"type":"color","id":"home_color","region":{{"x":0,"y":0,"width":1,"height":1}},"expected":[{},{},{}]}}]}}"#,
            expected[0], expected[1], expected[2]
        )
        .into_bytes();
        let pages = br#"{"schema_version":"0.5","pages":[{"id":"neutral/home","required":["home_color"]}]}"#.to_vec();
        let operation_hash = Sha256Hash::digest(&operation);
        let pack_hash = Sha256Hash::digest(&pack);
        let pages_hash = Sha256Hash::digest(&pages);
        let manifest = format!(
            r#"{{"schema_version":"0.3","entry_task_id":"{task_id}","files":[{{"path":"operations/{task_id}/task.json","sha256":"sha256:{operation_hash}"}},{{"path":"recognition/neutral.test.pack.json","sha256":"sha256:{pack_hash}"}},{{"path":"recognition/neutral.test.pages.json","sha256":"sha256:{pages_hash}"}}]}}"#
        )
        .into_bytes();

        BTreeMap::from([
            (
                "control.json".to_string(),
                format!(
                    r#"{{"schema_version":"Lab-1y.control.v1","package_id":"neutral.{task_id}","execution_mode":"recognize_only","game":"neutral","server":"test","resolution":{{"width":1,"height":1}},"entry_task_id":"{task_id}","allow_placeholder_coords":true}}"#
                )
                .into_bytes(),
            ),
            ("resources/manifest.json".to_string(), manifest),
            (
                format!("resources/operations/{task_id}/task.json"),
                operation,
            ),
            (
                "resources/recognition/neutral.test.pack.json".to_string(),
                pack,
            ),
            (
                "resources/recognition/neutral.test.pages.json".to_string(),
                pages,
            ),
        ])
    }

    fn zip_with_entries(entries: &[(&str, &[u8])]) -> Vec<u8> {
        zip_from_map(
            entries
                .iter()
                .map(|(path, bytes)| ((*path).to_string(), (*bytes).to_vec()))
                .collect(),
        )
    }

    fn zip_from_map(entries: BTreeMap<String, Vec<u8>>) -> Vec<u8> {
        zip_from_ordered_entries(entries.into_iter().collect())
    }

    fn zip_from_ordered_entries(entries: Vec<(String, Vec<u8>)>) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (path, bytes) in entries {
            writer.start_file(path, options).expect("start file");
            writer.write_all(&bytes).expect("write file");
        }
        writer.finish().expect("finish zip").into_inner()
    }
}
