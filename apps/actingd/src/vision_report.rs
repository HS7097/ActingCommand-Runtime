// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #453 S7 (`contracts/vision-model-folders.md`, "The manifest"): `actingd vision
//! manifest --provider cpu|directml` and `actingd vision check --config <file> [--install-root
//! <root>]`. Both only read: no state root, no owner lock, no ledger, no socket, no network, and
//! no file is written.
//!
//! The manifest is what a vision root of the provider must hold, built from the tool registry this
//! build embeds (`scripts/windows-tools/windows-tool-sources.v1.json`, the same text the install
//! adb's pin is read from) and the DirectML runtime pin (`DIRECTML_RUNTIME_PIN`): the runtime's
//! libraries, the pin's (`DirectML.dll` for `directml` only); the license files of the components
//! they come from (each one's `pinned_license_files`); and the files of every model component (one
//! that names `model_ref` and `family`, each artifact at its `vision_root_path`), with the model's
//! identity computed as the engine computes it (`ppocr_model_set_sha256`, or the classifier's own
//! SHA-256). It names no address. A registry this build cannot read so is `serialization_failed`
//! with the reason as its `foreign_data_error` link: a defect of the build, never of a root.
//!
//! The check resolves the configured vision root as a start does (`ConfiguredVision::resolve_root`:
//! a root that does not resolve is the start's own refusal, `vision_root_unavailable` or
//! `vision_root_in_program_slot`, at stage `config_vision`), compares every manifest file by its
//! size and then by its SHA-256 read in bounded chunks (the states of `PinnedFileCheck`: `ok`,
//! `missing`, `sha256_mismatch`, `unreadable`), lists `models\` as a start lists it (none when the
//! folder does not exist), and computes each model folder's identity from its files, each file
//! read once. A configuration without a `vision` section, or whose `execution_provider` is
//! `cuda`, has no manifest: `vision_config_invalid` at stage `config_vision`.

use super::*;
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{FileState, RawSource, VocabularyToken};
use actingcommand_vision_ffi::{
    DIRECTML_RUNTIME_PIN, DirectmlRuntimePin, OcrModelSpec, OnnxExecutionProvider,
    VISION_MODELS_DIRECTORY, VISION_RUNTIME_DIRECTORY, VisionListingCode, VisionModelFamily,
    list_vision_models, ppocr_model_set_sha256,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs;
use std::io::{ErrorKind, Read};

/// The schema of both reports (`{schema_version, outcome, report?}`); the outcome's code says
/// which command answered.
pub(super) const SCHEMA_VERSION: &str = "actingcommand.actingd.vision.v1";
/// The developer text of a command line this subcommand does not take.
const USAGE: &str = "actingd vision takes manifest --provider cpu|directml, or check --config \
     <file> [--install-root <root>], each option once with a value";
/// The providers a manifest is built for; a vision root of `cuda` has none.
const MANIFEST_PROVIDERS: [OnnxExecutionProvider; 2] =
    [OnnxExecutionProvider::Cpu, OnnxExecutionProvider::Directml];
/// The chunk a file is hashed in.
const CHUNK_BYTES: usize = 64 * 1024;
/// An OCR model's files in its folder, flat and nested (`contracts/vision-model-folders.md`,
/// "Folder layouts").
const OCR_DETECTOR: [&str; 2] = ["det.onnx", "det/inference.onnx"];
const OCR_RECOGNIZER: [&str; 2] = ["rec.onnx", "rec/inference.onnx"];
const OCR_DICTIONARY: [&str; 2] = ["keys.txt", "rec/keys.txt"];
/// An NN classifier's file in its folder.
const NN_CLASSIFIER: &str = "model.onnx";

/// What a manifest is built from: the tool registry's text and the DirectML runtime pin. A test
/// gives its own.
#[derive(Clone, Copy)]
pub(super) struct PinTable<'a> {
    pub(super) registry: &'a str,
    pub(super) pin: &'a DirectmlRuntimePin,
}

/// The pin table this build carries.
const EMBEDDED: PinTable<'static> = PinTable {
    registry: config::ac_adb::TOOL_SOURCES_PIN,
    pin: &DIRECTML_RUNTIME_PIN,
};

/// One file a vision root of the provider must hold.
#[derive(Debug, Clone, Serialize)]
pub(super) struct ManifestFile {
    /// Its place below the vision root, `/`-separated.
    pub(super) vision_root_path: String,
    pub(super) size: u64,
    pub(super) sha256: String,
    /// The registry component that pins it.
    pub(super) component: String,
    /// Its entry in the component's archive; `null` for a file the component pins on its own.
    pub(super) archive_path: Option<String>,
    /// Whether it is a license file.
    pub(super) license: bool,
    /// Whether it has a size and a SHA-256 to compare (every file of this build's manifest).
    pub(super) pinned: bool,
}

/// One model a vision root of the provider must hold.
#[derive(Debug, Clone, Serialize)]
pub(super) struct ManifestModel {
    pub(super) model_ref: String,
    pub(super) family: String,
    /// The `vision_root_path` of each of its files, in the registry's order.
    pub(super) files: Vec<String>,
    /// Its identity, computed from its files' pins as the engine computes it from their bytes.
    pub(super) model_sha256: String,
}

/// What a vision root of `provider` must hold.
#[derive(Debug, Clone, Serialize)]
pub(super) struct Manifest {
    pub(super) provider: &'static str,
    /// In byte order of `vision_root_path`.
    pub(super) files: Vec<ManifestFile>,
    pub(super) models: Vec<ManifestModel>,
}

/// The members of the registry a manifest reads; every other member is ignored.
#[derive(Deserialize)]
struct Registry {
    components: BTreeMap<String, RegistryComponent>,
}

#[derive(Deserialize)]
struct RegistryComponent {
    #[serde(default)]
    model_ref: Option<String>,
    #[serde(default)]
    family: Option<String>,
    #[serde(default)]
    artifacts: Vec<RegistryArtifact>,
    #[serde(default)]
    pinned_license_files: Vec<RegistryPinnedFile>,
}

#[derive(Deserialize)]
struct RegistryArtifact {
    #[serde(default)]
    vision_root_path: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    sha256: Option<String>,
}

#[derive(Deserialize)]
struct RegistryPinnedFile {
    path: String,
    vision_root_path: String,
    size: u64,
    sha256: String,
}

/// `actingd vision manifest|check` on the pin table this build carries.
pub(super) fn run(arguments: Vec<OsString>) -> Result<Reported, ActingdError> {
    run_with(&arguments, EMBEDDED)
}

/// `actingd vision manifest|check` on `table`; a command line it does not take is
/// `invalid_arguments`.
pub(super) fn run_with(
    arguments: &[OsString],
    table: PinTable<'_>,
) -> Result<Reported, ActingdError> {
    match arguments.get(1).and_then(|word| word.to_str()) {
        Some("manifest") => manifest_report(arguments, table),
        Some("check") => check_report(arguments, table),
        _ => Err(invalid()),
    }
}

fn invalid() -> ActingdError {
    ActingdError::config(SharedCode::InvalidArguments).with_detail(USAGE.to_owned())
}

fn staged(mut error: ActingdError, stage: ActingdLocation) -> ActingdError {
    error.stage = Some(stage.location());
    error
}

fn encode_failed() -> ActingdError {
    ActingdError::process(SharedCode::SerializationFailed)
}

/// A registry this build cannot read as a manifest: `serialization_failed`, the reason its
/// `foreign_data_error` link.
fn defect(reason: String) -> ActingdError {
    let code = SharedCode::SerializationFailed;
    ActingdError::refused(
        code,
        Outcome::new(code).caused_by(
            Outcome::new(ContractCode::ForeignDataError)
                .raw_source(RawSource::Json)
                .raw_text(reason),
        ),
    )
}

/// `vision manifest --provider cpu|directml`: `vision_manifest_reported` with the manifest as
/// `report`.
fn manifest_report(arguments: &[OsString], table: PinTable<'_>) -> Result<Reported, ActingdError> {
    let provider = match arguments {
        [_, _, flag, word] if flag == "--provider" => MANIFEST_PROVIDERS
            .into_iter()
            .find(|provider| word == provider.as_str())
            .ok_or_else(invalid)?,
        _ => return Err(invalid()),
    };
    let manifest = manifest(table, provider)?;
    let report = serde_json::to_value(&manifest).map_err(|_| encode_failed())?;
    Ok(Reported::new(Outcome::new(ActingdCode::VisionManifestReported)).with("report", report))
}

/// What a vision root of `provider` must hold (module doc), from `table`.
pub(super) fn manifest(
    table: PinTable<'_>,
    provider: OnnxExecutionProvider,
) -> Result<Manifest, ActingdError> {
    let registry = serde_json::from_str::<Registry>(table.registry)
        .map_err(|error| defect(error.to_string()))?;
    let mut runtime = vec![&table.pin.onnxruntime, &table.pin.providers_shared];
    if provider == OnnxExecutionProvider::Directml {
        runtime.push(&table.pin.directml);
    }
    let mut files = runtime
        .iter()
        .map(|pinned| ManifestFile {
            vision_root_path: format!("{VISION_RUNTIME_DIRECTORY}/{}", pinned.file_name),
            size: pinned.size,
            sha256: pinned.sha256.to_owned(),
            component: pinned.tool_source.to_owned(),
            archive_path: Some(pinned.archive_path.to_owned()),
            license: false,
            pinned: true,
        })
        .collect::<Vec<_>>();
    // The license files of each component a runtime file comes from, once per component.
    let sources = runtime
        .iter()
        .map(|pinned| pinned.tool_source)
        .collect::<BTreeSet<_>>();
    for source in sources {
        let component = registry
            .components
            .get(source)
            .ok_or_else(|| defect(format!("the registry has no component {source}")))?;
        files.extend(
            component
                .pinned_license_files
                .iter()
                .map(|license| ManifestFile {
                    vision_root_path: license.vision_root_path.clone(),
                    size: license.size,
                    sha256: license.sha256.clone(),
                    component: source.to_owned(),
                    archive_path: Some(license.path.clone()),
                    license: true,
                    pinned: true,
                }),
        );
    }
    let mut models = Vec::new();
    for (id, component) in &registry.components {
        let (model_ref, family) = match (&component.model_ref, &component.family) {
            (Some(model_ref), Some(family)) => (model_ref, family),
            (None, None) => continue,
            _ => {
                return Err(defect(format!(
                    "component {id} names one of model_ref and family without the other"
                )));
            }
        };
        let mut model_files = Vec::new();
        for artifact in &component.artifacts {
            let Some(path) = &artifact.vision_root_path else {
                continue;
            };
            let (Some(size), Some(sha256)) = (artifact.size, &artifact.sha256) else {
                return Err(defect(format!(
                    "component {id}: {path} has no size or no sha256"
                )));
            };
            model_files.push(ManifestFile {
                vision_root_path: path.clone(),
                size,
                sha256: sha256.clone(),
                component: id.clone(),
                archive_path: None,
                license: false,
                pinned: true,
            });
        }
        models.push(ManifestModel {
            model_ref: model_ref.clone(),
            family: family.clone(),
            files: model_files
                .iter()
                .map(|file| file.vision_root_path.clone())
                .collect(),
            model_sha256: model_identity(model_ref, family, &model_files)?,
        });
        files.extend(model_files);
    }
    files.sort_by(|left, right| left.vision_root_path.cmp(&right.vision_root_path));
    let mut places = BTreeSet::new();
    for file in &files {
        if !relative_path(&file.vision_root_path)
            || !places.insert(file.vision_root_path.to_ascii_lowercase())
        {
            return Err(defect(format!(
                "{} is no place of its own below a vision root",
                file.vision_root_path
            )));
        }
    }
    Ok(Manifest {
        provider: provider.as_str(),
        files,
        models,
    })
}

/// Whether `path` is relative, `/`-separated, with no empty, `.` or `..` segment and no `\` or
/// `:`.
fn relative_path(path: &str) -> bool {
    !path.contains(['\\', ':'])
        && path
            .split('/')
            .all(|segment| !matches!(segment, "" | "." | ".."))
}

/// The identity of the model `model_ref` of `family` whose files are `files` (each below
/// `models/<model_ref>/`), from their pins: for `ppocr-ctc` `ppocr_model_set_sha256` over the
/// detector (when there is one), the recognizer and the dictionary, flat or nested; for
/// `onnx-classify` the SHA-256 of `model.onnx`.
fn model_identity(
    model_ref: &str,
    family: &str,
    files: &[ManifestFile],
) -> Result<String, ActingdError> {
    let folder = format!("{VISION_MODELS_DIRECTORY}/{model_ref}/");
    let mut named = BTreeMap::new();
    for file in files {
        let name = file
            .vision_root_path
            .strip_prefix(folder.as_str())
            .ok_or_else(|| {
                defect(format!(
                    "model {model_ref}: {} lies outside {folder}",
                    file.vision_root_path
                ))
            })?;
        named.insert(name, file.sha256.as_str());
    }
    if family == VisionModelFamily::PpocrCtc.as_str() {
        let missing = |role: &str| defect(format!("model {model_ref} has no {role} file"));
        let recognizer = pick(&named, OCR_RECOGNIZER).ok_or_else(|| missing("recognizer"))?;
        let dictionary = pick(&named, OCR_DICTIONARY).ok_or_else(|| missing("dictionary"))?;
        ppocr_model_set_sha256(pick(&named, OCR_DETECTOR), recognizer, dictionary, None)
            .map_err(|error| defect(format!("model {model_ref}: {}", error.message())))
    } else if family == VisionModelFamily::OnnxClassify.as_str() {
        named
            .get(NN_CLASSIFIER)
            .map(|sha256| (*sha256).to_owned())
            .ok_or_else(|| defect(format!("model {model_ref} has no {NN_CLASSIFIER}")))
    } else {
        Err(defect(format!(
            "model {model_ref}: {family} is no family of a model folder"
        )))
    }
}

/// The pin of the first of `names` (a file's flat and nested name) that `named` holds.
fn pick<'a>(named: &BTreeMap<&str, &'a str>, names: [&str; 2]) -> Option<&'a str> {
    names.into_iter().find_map(|name| named.get(name).copied())
}

/// `vision check --config <file> [--install-root <root>]`: `vision_check_completed` with the
/// comparison as `report` `{provider, vision_root, files, folders, models}`.
fn check_report(arguments: &[OsString], table: PinTable<'_>) -> Result<Reported, ActingdError> {
    let (config_path, install_root) = parse_check(arguments)?;
    let install_root = install_root
        .as_deref()
        .map(check_config::install_root_of)
        .transpose()?;
    let file = config::load_for(
        &config_path,
        actingcommand_contract::InstallConfigPurpose::CandidateCheck,
    )
    .map_err(|code| staged(ActingdError::config(code), ActingdLocation::ConfigLoad))?;
    let file = match install_root {
        Some(root) => file.with_install_root(root),
        None => file,
    };
    let vision = file
        .vision_section()
        .map_err(|code| staged(ActingdError::config(code), ActingdLocation::ConfigAssemble))?
        .ok_or_else(unmanifested)?;
    let provider = vision.execution_provider();
    if !MANIFEST_PROVIDERS.contains(&provider) {
        return Err(unmanifested());
    }
    let root = vision.resolve_root().map_err(|refused| {
        staged(
            ActingdError::refused(refused.code, refused.outcome())
                .with_detail(refused.message.clone()),
            ActingdLocation::ConfigVision,
        )
    })?;
    let manifest = manifest(table, provider)?;
    let mut digests = Digests::default();
    let files = manifest
        .files
        .iter()
        .map(|file| checked_file(&root, file, &mut digests))
        .collect::<Result<Vec<_>, _>>()?;
    let folders = model_folders(&root, &mut digests)?;
    let models = manifest
        .models
        .iter()
        .map(|model| checked_model(model, &folders))
        .collect::<Vec<_>>();
    let report = json!({
        "provider": provider.as_str(),
        "vision_root": root.to_string_lossy(),
        "files": files,
        "folders": folders,
        "models": models,
    });
    Ok(Reported::new(Outcome::new(ActingdCode::VisionCheckCompleted)).with("report", report))
}

/// The command line after `vision check`: `--config` once and `--install-root` at most once,
/// each with a non-empty value.
fn parse_check(arguments: &[OsString]) -> Result<(PathBuf, Option<PathBuf>), ActingdError> {
    let (pairs, remaining) = arguments.get(2..).unwrap_or_default().as_chunks::<2>();
    if !remaining.is_empty() {
        return Err(invalid());
    }
    let mut config = None;
    let mut install_root = None;
    for pair in pairs {
        let slot = if pair[0] == "--config" {
            &mut config
        } else if pair[0] == "--install-root" {
            &mut install_root
        } else {
            return Err(invalid());
        };
        if slot.is_some() || pair[1].is_empty() {
            return Err(invalid());
        }
        *slot = Some(PathBuf::from(&pair[1]));
    }
    Ok((config.ok_or_else(invalid)?, install_root))
}

/// A configuration whose vision root has no manifest: no `vision` section, or `cuda`.
fn unmanifested() -> ActingdError {
    staged(
        ActingdError::config(ActingdCode::VisionConfigInvalid).with_detail(
            "actingd vision check compares a vision root of execution_provider cpu or directml; \
             the configuration names no vision section or another provider"
                .to_owned(),
        ),
        ActingdLocation::ConfigVision,
    )
}

/// The SHA-256 of each file read whole, by its path, so that no file is read twice.
#[derive(Default)]
struct Digests(BTreeMap<PathBuf, String>);

impl Digests {
    /// The SHA-256 of the file at `path`, read in bounded chunks once; the OS failure, with the
    /// path, when it cannot be read.
    fn of(&mut self, path: &Path) -> Result<String, Outcome> {
        if let Some(digest) = self.0.get(path) {
            return Ok(digest.clone());
        }
        let digest = read_digest(path)
            .map_err(|(error, op)| Outcome::from_io_error(&error, op).path(path))?;
        self.0.insert(path.to_path_buf(), digest.clone());
        Ok(digest)
    }
}

fn read_digest(path: &Path) -> Result<String, (std::io::Error, IoOp)> {
    let mut file = fs::File::open(path).map_err(|error| (error, IoOp::Open))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; CHUNK_BYTES];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => hasher.update(&buffer[..read]),
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err((error, IoOp::Read)),
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// How the file at `path` compares with `file`'s pin, as `PinnedRuntimeFile::check` compares: no
/// file is `missing`; one whose metadata cannot be read `unreadable`; no regular file or another
/// size `sha256_mismatch` without reading it; then its SHA-256, `ok` or `sha256_mismatch`, or
/// `unreadable` when it cannot be read. The OS failure goes with `unreadable`.
fn compare(
    path: &Path,
    file: &ManifestFile,
    digests: &mut Digests,
) -> (FileState, Option<Outcome>) {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return (FileState::Missing, None),
        Err(error) => {
            return (
                FileState::Unreadable,
                Some(Outcome::from_io_error(&error, IoOp::Metadata).path(path)),
            );
        }
    };
    if !metadata.is_file() || metadata.len() != file.size {
        return (FileState::Sha256Mismatch, None);
    }
    match digests.of(path) {
        Ok(digest) if digest == file.sha256 => (FileState::Ok, None),
        Ok(_) => (FileState::Sha256Mismatch, None),
        Err(cause) => (FileState::Unreadable, Some(cause)),
    }
}

/// An outcome as a report member: its envelope.
fn envelope(outcome: &Outcome) -> Result<Value, ActingdError> {
    serde_json::to_value(outcome.envelope(Detail::Full)).map_err(|_| encode_failed())
}

/// One manifest file of the root at `root`: the file's manifest entry with its `state` and, for
/// `unreadable`, the OS failure as `cause`.
fn checked_file(
    root: &Path,
    file: &ManifestFile,
    digests: &mut Digests,
) -> Result<Value, ActingdError> {
    let path = file
        .vision_root_path
        .split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part));
    let (state, cause) = compare(&path, file, digests);
    let mut entry = serde_json::to_value(file).map_err(|_| encode_failed())?;
    entry["state"] = Value::from(state.token());
    if let Some(cause) = cause {
        entry["cause"] = envelope(&cause)?;
    }
    Ok(entry)
}

/// The folders of `<root>\models` as a start lists them, by name: a model folder
/// `{model_ref, family, layout?, identity}` (`identity` `null` with the OS failure as `cause`
/// when a file cannot be read), a folder that breaks the folder rule `{model_ref, invalid}` with
/// its reason. None when `models\` does not exist; one that cannot be listed is the start's
/// refusal, `vision_root_unavailable` at stage `config_vision`.
fn model_folders(root: &Path, digests: &mut Digests) -> Result<Vec<Value>, ActingdError> {
    let models_dir = root.join(VISION_MODELS_DIRECTORY);
    if matches!(fs::symlink_metadata(&models_dir), Err(error) if error.kind() == ErrorKind::NotFound)
    {
        return Ok(Vec::new());
    }
    let listing = list_vision_models(&models_dir).map_err(|error| {
        let code = VisionListingCode::VisionRootUnavailable;
        let outcome = match error.os_cause() {
            Some(cause) => Outcome::new(code).caused_by(cause.clone()),
            None => Outcome::new(code),
        };
        staged(
            ActingdError::refused(code, outcome).with_detail(error.message().to_owned()),
            ActingdLocation::ConfigVision,
        )
    })?;
    let mut folders = BTreeMap::new();
    for spec in &listing.ocr {
        let identity = ocr_identity(spec, digests);
        let entry = folder(
            &spec.model_ref,
            VisionModelFamily::PpocrCtc,
            Some(spec.layout.as_str()),
            identity,
        )?;
        folders.insert(spec.model_ref.clone(), entry);
    }
    for spec in &listing.nn {
        let identity = digests.of(&spec.model_path);
        let entry = folder(
            &spec.model_ref,
            VisionModelFamily::OnnxClassify,
            None,
            identity,
        )?;
        folders.insert(spec.model_ref.clone(), entry);
    }
    for invalid in &listing.invalid {
        folders.insert(
            invalid.name.clone(),
            json!({"model_ref": invalid.name, "invalid": invalid.reason}),
        );
    }
    Ok(folders.into_values().collect())
}

/// The identity of an OCR folder from the bytes of its files, as the engine computes it.
fn ocr_identity(spec: &OcrModelSpec, digests: &mut Digests) -> Result<String, Outcome> {
    let detector = match &spec.detector_path {
        Some(path) => Some(digests.of(path)?),
        None => None,
    };
    let recognizer = digests.of(&spec.recognizer_path)?;
    let dictionary = digests.of(&spec.dictionary_path)?;
    ppocr_model_set_sha256(detector.as_deref(), &recognizer, &dictionary, None).map_err(|error| {
        Outcome::new(ContractCode::ForeignDataError)
            .raw_source(RawSource::Vision)
            .raw_text(error.message().to_owned())
    })
}

/// One model folder's report entry.
fn folder(
    model_ref: &str,
    family: VisionModelFamily,
    layout: Option<&str>,
    identity: Result<String, Outcome>,
) -> Result<Value, ActingdError> {
    let mut entry = json!({"model_ref": model_ref, "family": family.as_str()});
    if let Some(layout) = layout {
        entry["layout"] = Value::from(layout);
    }
    match identity {
        Ok(identity) => entry["identity"] = Value::from(identity),
        Err(cause) => {
            entry["identity"] = Value::Null;
            entry["cause"] = envelope(&cause)?;
        }
    }
    Ok(entry)
}

/// One manifest model against the folders: whether a folder of its name is listed (`present`),
/// that folder's identity (`null` without one, or for an invalid folder or one not read), and
/// whether it equals the model's (`identity_matches`).
fn checked_model(model: &ManifestModel, folders: &[Value]) -> Value {
    let folder = folders
        .iter()
        .find(|folder| folder["model_ref"] == model.model_ref.as_str());
    let identity = folder.and_then(|folder| folder["identity"].as_str());
    json!({
        "model_ref": model.model_ref,
        "family": model.family,
        "model_sha256": model.model_sha256,
        "present": folder.is_some(),
        "identity": identity,
        "identity_matches": identity == Some(model.model_sha256.as_str()),
    })
}
