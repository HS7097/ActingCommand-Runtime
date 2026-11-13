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
//! folder does not exist), and computes the identity of the model folders the manifest or a
//! `--model` names from their files, each file read once. A configuration without a `vision`
//! section, or whose `execution_provider` is `cuda`, has no manifest: `vision_config_invalid` at
//! stage `config_vision`.
//!
//! Fix 1 (review): what the check reads is bounded. A manifest file of another size than its pin
//! is a mismatch and is not read, and none is read past its pinned size. Only the model files of
//! the folders the manifest or a `--model` names are hashed; every other folder is listed by name
//! and none of its model files is read. A model folder file is hashed only within
//! `FILE_BOUND_BYTES` (256 MiB), and the check hashes no more than `TOTAL_BOUND_BYTES` (512 MiB)
//! in all, the manifest's files included; a file over either bound is reported as such
//! (`over_bound`) and not read. Fix 2 (review): listing `models\` as a start lists it reads, of
//! every folder there, asked or not, its entries' metadata and its `model.json` when it holds one
//! (at most 64 KiB, as a start reads it); those reads hash nothing and are not counted in
//! `TOTAL_BOUND_BYTES`, which counts the bytes hashed. Each `--model` is a model folder's name by
//! the loader's own rule (`invalid_model_folder_name`), at most `MAX_ASKED_MODELS` (64) of them,
//! refused `invalid_arguments` before the configuration is read; a name asked about that no folder
//! has is listed `asked` and `absent`.

use super::*;
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{FileState, RawSource, VocabularyToken};
use actingcommand_vision_ffi::{
    DIRECTML_RUNTIME_PIN, DirectmlRuntimePin, OcrModelSpec, OnnxExecutionProvider,
    VISION_MODELS_DIRECTORY, VISION_RUNTIME_DIRECTORY, VisionListingCode, VisionModelFamily,
    invalid_model_folder_name, list_vision_models, ppocr_model_set_sha256,
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
     <file> [--install-root <root>] [--model <model_ref>]..., --config and --install-root once, \
     --model at most 64 times, each a model folder's name, each option with a value";
/// The providers a manifest is built for; a vision root of `cuda` has none.
const MANIFEST_PROVIDERS: [OnnxExecutionProvider; 2] =
    [OnnxExecutionProvider::Cpu, OnnxExecutionProvider::Directml];
/// The chunk a file is hashed in.
const CHUNK_BYTES: usize = 64 * 1024;
/// Workflow #453 S7 fix 1 (review): the most of one model folder file the check hashes; a larger
/// one is reported over the bound and not read. 256 MiB, more than three times the largest model
/// file this build's registry pins (the OCR model's recognizer, about 77 MB). A bound of what the
/// check reads, not a tunable.
const FILE_BOUND_BYTES: u64 = 256 * 1024 * 1024;
/// Workflow #453 S7 fix 1 (review): the most the check hashes in all, the manifest's files
/// included (their pins add up to about 175 MB for `directml` with this build's registry, 156 MB
/// for `cpu`); a model folder file that would pass it is reported over the bound and not read.
/// 512 MiB; the rescue row's bound of `actingd vision check` is this read at 4 MiB/s. Not a
/// tunable.
const TOTAL_BOUND_BYTES: u64 = 512 * 1024 * 1024;
/// Workflow #453 S7 fix 2 (review): the most `--model` options `vision check` takes; one more is
/// `invalid_arguments`. The rescue row names each model the packs of every game root of a root
/// need, once: this build's registry pins one model, a game's packs name an OCR model and a few
/// classifiers, and the engine holds four loaded at once; 64 is far above that, and 64 names of the
/// longest a folder may have (255 bytes) with their `--model` are 16 896 characters, about half of
/// the 32 767 a Windows command line holds, the rest left to the program, `--config` and
/// `--install-root`. A bound of what the check takes, not a tunable.
const MAX_ASKED_MODELS: usize = 64;

/// What the check hashes at most: of one model folder file, and in all. A test gives its own.
#[derive(Clone, Copy)]
pub(super) struct Bounds {
    pub(super) file_bytes: u64,
    pub(super) total_bytes: u64,
}

/// The bounds this build checks within.
pub(super) const BOUNDS: Bounds = Bounds {
    file_bytes: FILE_BOUND_BYTES,
    total_bytes: TOTAL_BOUND_BYTES,
};
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

/// `actingd vision manifest|check` on the pin table this build carries, within its bounds.
pub(super) fn run(arguments: Vec<OsString>) -> Result<Reported, ActingdError> {
    run_with(&arguments, EMBEDDED, BOUNDS)
}

/// `actingd vision manifest|check` on `table`, `check` hashing within `bounds`; a command line it
/// does not take is `invalid_arguments`.
pub(super) fn run_with(
    arguments: &[OsString],
    table: PinTable<'_>,
    bounds: Bounds,
) -> Result<Reported, ActingdError> {
    match arguments.get(1).and_then(|word| word.to_str()) {
        Some("manifest") => manifest_report(arguments, table),
        Some("check") => check_report(arguments, table, bounds),
        _ => Err(invalid()),
    }
}

fn invalid() -> ActingdError {
    ActingdError::config(SharedCode::InvalidArguments).with_detail(USAGE.to_owned())
}

/// `error` at `stage`. Workflow #453 S7 fix 2: `vision` names a stage only here, and only
/// `config_load`, `config_assemble` and `config_vision`, where it loads, assembles and resolves
/// the vision configuration; the rescue row `vision check` reads a refusal with a stage as a root
/// that was not resolved, and one without as no usable report.
fn staged(mut error: ActingdError, stage: ActingdLocation) -> ActingdError {
    error.stage = Some(stage.location());
    error
}

fn encode_failed() -> ActingdError {
    ActingdError::process(SharedCode::SerializationFailed)
}

/// A registry this build cannot read as a manifest: `serialization_failed`, `detail` (what is
/// wrong with it) its `foreign_data_error` link.
fn defect(detail: String) -> ActingdError {
    let code = SharedCode::SerializationFailed;
    ActingdError::refused(
        code,
        Outcome::new(code).caused_by(
            Outcome::new(ContractCode::ForeignDataError)
                .raw_source(RawSource::Json)
                .raw_text(detail),
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

/// `vision check --config <file> [--install-root <root>] [--model <model_ref>]...`:
/// `vision_check_completed` with the comparison as `report` `{provider, vision_root, files,
/// folders, models}`, hashing within `bounds`.
fn check_report(
    arguments: &[OsString],
    table: PinTable<'_>,
    bounds: Bounds,
) -> Result<Reported, ActingdError> {
    let CheckLine {
        config: config_path,
        install_root,
        models: mut asked,
    } = parse_check(arguments)?;
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
    // The manifest's files are read within the total bound; a build whose pins do not fit it is
    // a defect of the build.
    let pinned_bytes = manifest
        .files
        .iter()
        .fold(0_u64, |bytes, file| bytes.saturating_add(file.size));
    if pinned_bytes > bounds.total_bytes {
        return Err(defect(format!(
            "the manifest's files hold {pinned_bytes} bytes, more than the {} bytes vision check \
             hashes in all",
            bounds.total_bytes
        )));
    }
    let mut reader = Reader::new(bounds);
    let files = manifest
        .files
        .iter()
        .map(|file| checked_file(&root, file, &mut reader))
        .collect::<Result<Vec<_>, _>>()?;
    asked.extend(manifest.models.iter().map(|model| model.model_ref.clone()));
    let folders = model_folders(&root, &asked, &mut reader)?;
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

/// The command line of `vision check`.
struct CheckLine {
    config: PathBuf,
    install_root: Option<PathBuf>,
    /// Workflow #453 S7 fix 1: the model folders whose identity is computed besides the
    /// manifest's (the models the installed packs need, as the rescue row names them).
    models: BTreeSet<String>,
}

/// The command line after `vision check`: `--config` once, `--install-root` at most once and
/// (fix 1) `--model` with a UTF-8 value, each with a non-empty value. Fix 2 (review): each
/// `--model` a model folder's name by the loader's own rule ([`invalid_model_folder_name`]), at
/// most [`MAX_ASKED_MODELS`] of them; anything else is `invalid_arguments`, before anything of the
/// configuration or the vision root is read.
fn parse_check(arguments: &[OsString]) -> Result<CheckLine, ActingdError> {
    let (pairs, remaining) = arguments.get(2..).unwrap_or_default().as_chunks::<2>();
    if !remaining.is_empty() {
        return Err(invalid());
    }
    let mut config = None;
    let mut install_root = None;
    let mut models = BTreeSet::new();
    let mut asked = 0_usize;
    for pair in pairs {
        if pair[1].is_empty() {
            return Err(invalid());
        }
        if pair[0] == "--model" {
            asked += 1;
            let model = pair[1]
                .to_str()
                .filter(|model| asked <= MAX_ASKED_MODELS && !invalid_model_folder_name(model))
                .ok_or_else(invalid)?;
            models.insert(model.to_owned());
            continue;
        }
        let slot = if pair[0] == "--config" {
            &mut config
        } else if pair[0] == "--install-root" {
            &mut install_root
        } else {
            return Err(invalid());
        };
        if slot.is_some() {
            return Err(invalid());
        }
        *slot = Some(PathBuf::from(&pair[1]));
    }
    Ok(CheckLine {
        config: config.ok_or_else(invalid)?,
        install_root,
        models,
    })
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

/// Workflow #453 S7 fix 1 (review): the SHA-256 of `file` over at most `limit` bytes, read in
/// bounded chunks from where it stands, and how many bytes that was. Nothing past `limit` is
/// read.
pub(super) fn hash_at_most(file: &mut fs::File, limit: u64) -> std::io::Result<(String, u64)> {
    let mut bounded = file.take(limit);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; CHUNK_BYTES];
    let mut hashed = 0_u64;
    loop {
        match bounded.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                hasher.update(&buffer[..read]);
                hashed = hashed.saturating_add(read as u64);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok((format!("{:x}", hasher.finalize()), hashed))
}

/// Which bound a model folder file is over.
#[derive(Debug, Clone, Copy)]
enum Bound {
    /// [`Bounds::file_bytes`].
    File,
    /// What [`Bounds::total_bytes`] left after the files hashed before it.
    Total,
}

impl Bound {
    fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Total => "total",
        }
    }
}

/// Why a model folder the check was asked about has no identity in the report.
enum Unidentified {
    /// A file of it could not be read (the OS failure, with the path), or the engine's rule
    /// refused its files' digests.
    Failed(Outcome),
    /// A file of it is over a bound and was not read.
    OverBound {
        path: PathBuf,
        size: u64,
        bound: Bound,
        bound_bytes: u64,
    },
}

/// The files the check hashed, by path, so that none is read twice, and the bytes it hashed in
/// all, within its [`Bounds`].
struct Reader {
    bounds: Bounds,
    digests: BTreeMap<PathBuf, String>,
    hashed_bytes: u64,
}

impl Reader {
    fn new(bounds: Bounds) -> Self {
        Self {
            bounds,
            digests: BTreeMap::new(),
            hashed_bytes: 0,
        }
    }

    /// The SHA-256 of the manifest file at `path`, whose metadata gave it its pin's `size`, read
    /// at most `size` bytes: `None` when it did not hold `size` bytes while it was read (it shrank,
    /// or grew past its pin); the OS failure, with the path, when it cannot be read.
    fn pinned(&mut self, path: &Path, size: u64) -> Result<Option<String>, Outcome> {
        if let Some(digest) = self.digests.get(path) {
            return Ok(Some(digest.clone()));
        }
        let failed =
            |error: std::io::Error, op: IoOp| Outcome::from_io_error(&error, op).path(path);
        let mut file = fs::File::open(path).map_err(|error| failed(error, IoOp::Open))?;
        let (digest, hashed) =
            hash_at_most(&mut file, size).map_err(|error| failed(error, IoOp::Read))?;
        self.hashed_bytes = self.hashed_bytes.saturating_add(hashed);
        let length = file
            .metadata()
            .map_err(|error| failed(error, IoOp::Metadata))?
            .len();
        if hashed != size || length != size {
            return Ok(None);
        }
        self.digests.insert(path.to_path_buf(), digest.clone());
        Ok(Some(digest))
    }

    /// The SHA-256 of the model folder file at `path`, read at most to its length when it was
    /// opened; a file larger than the file bound, or than what the total bound leaves, is
    /// [`Unidentified::OverBound`] and is not read; the OS failure, with the path, when it cannot
    /// be read.
    fn digest(&mut self, path: &Path) -> Result<String, Unidentified> {
        if let Some(digest) = self.digests.get(path) {
            return Ok(digest.clone());
        }
        let failed = |error: std::io::Error, op: IoOp| {
            Unidentified::Failed(Outcome::from_io_error(&error, op).path(path))
        };
        let mut file = fs::File::open(path).map_err(|error| failed(error, IoOp::Open))?;
        let size = file
            .metadata()
            .map_err(|error| failed(error, IoOp::Metadata))?
            .len();
        let left = self.bounds.total_bytes.saturating_sub(self.hashed_bytes);
        let over = if size > self.bounds.file_bytes {
            Some((Bound::File, self.bounds.file_bytes))
        } else if size > left {
            Some((Bound::Total, self.bounds.total_bytes))
        } else {
            None
        };
        if let Some((bound, bound_bytes)) = over {
            return Err(Unidentified::OverBound {
                path: path.to_path_buf(),
                size,
                bound,
                bound_bytes,
            });
        }
        let (digest, hashed) =
            hash_at_most(&mut file, size).map_err(|error| failed(error, IoOp::Read))?;
        self.hashed_bytes = self.hashed_bytes.saturating_add(hashed);
        self.digests.insert(path.to_path_buf(), digest.clone());
        Ok(digest)
    }
}

/// How the file at `path` compares with `file`'s pin, as `PinnedRuntimeFile::check` compares: no
/// file is `missing`; one whose metadata cannot be read `unreadable`; no regular file or another
/// size `sha256_mismatch` without reading it; then its SHA-256 over at most its pinned size, `ok`
/// or `sha256_mismatch` (also when it changed its size while read), or `unreadable` when it cannot
/// be read. The OS failure goes with `unreadable`.
fn compare(path: &Path, file: &ManifestFile, reader: &mut Reader) -> (FileState, Option<Outcome>) {
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
    match reader.pinned(path, file.size) {
        Ok(Some(digest)) if digest == file.sha256 => (FileState::Ok, None),
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
    reader: &mut Reader,
) -> Result<Value, ActingdError> {
    let path = file
        .vision_root_path
        .split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part));
    let (state, cause) = compare(&path, file, reader);
    let mut entry = serde_json::to_value(file).map_err(|_| encode_failed())?;
    entry["state"] = Value::from(state.token());
    if let Some(cause) = cause {
        entry["cause"] = envelope(&cause)?;
    }
    Ok(entry)
}

/// The folders of `<root>\models` as a start lists them, by name: a model folder
/// `{model_ref, family, layout?, asked, identity?}`, a folder that breaks the folder rule
/// `{model_ref, invalid}` with its reason. Fix 1 (review): only the model files of a model folder
/// `asked` names (the manifest's models and each `--model`) are hashed, `asked` true with its
/// `identity` (`null` with the OS failure as `cause` when a file cannot be read, or with
/// `over_bound` for a file over a bound, which is not read); every other one is `asked` false,
/// with no identity, and none of its model files is read. Fix 2 (review): the listing itself reads
/// every folder's entries' metadata and its `model.json` (at most 64 KiB), asked or not, as a start
/// does, and none of that is counted in the bounds of `reader`; each name `asked` names that no
/// folder has is `{model_ref, asked: true, absent: true}`, in name order with the others. No
/// folder but those absent ones when `models\` does not exist; one that cannot be listed is the
/// start's refusal, `vision_root_unavailable` at stage `config_vision`.
fn model_folders(
    root: &Path,
    asked: &BTreeSet<String>,
    reader: &mut Reader,
) -> Result<Vec<Value>, ActingdError> {
    let mut folders = BTreeMap::new();
    let models_dir = root.join(VISION_MODELS_DIRECTORY);
    if !matches!(fs::symlink_metadata(&models_dir), Err(error) if error.kind() == ErrorKind::NotFound)
    {
        listed_folders(&models_dir, asked, reader, &mut folders)?;
    }
    for model_ref in asked {
        folders
            .entry(model_ref.clone())
            .or_insert_with(|| json!({"model_ref": model_ref, "asked": true, "absent": true}));
    }
    Ok(folders.into_values().collect())
}

/// The folders `list_vision_models` lists in `models_dir`, each as [`model_folders`] writes it,
/// into `folders` by name.
fn listed_folders(
    models_dir: &Path,
    asked: &BTreeSet<String>,
    reader: &mut Reader,
    folders: &mut BTreeMap<String, Value>,
) -> Result<(), ActingdError> {
    let listing = list_vision_models(models_dir).map_err(|error| {
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
    for spec in &listing.ocr {
        let identity = asked
            .contains(&spec.model_ref)
            .then(|| ocr_identity(spec, reader));
        let entry = folder(
            &spec.model_ref,
            VisionModelFamily::PpocrCtc,
            Some(spec.layout.as_str()),
            identity,
        )?;
        folders.insert(spec.model_ref.clone(), entry);
    }
    for spec in &listing.nn {
        let identity = asked
            .contains(&spec.model_ref)
            .then(|| reader.digest(&spec.model_path));
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
    Ok(())
}

/// The identity of an OCR folder from the bytes of its files, as the engine computes it; the
/// first file that cannot be read or is over a bound stops it.
fn ocr_identity(spec: &OcrModelSpec, reader: &mut Reader) -> Result<String, Unidentified> {
    let detector = match &spec.detector_path {
        Some(path) => Some(reader.digest(path)?),
        None => None,
    };
    let recognizer = reader.digest(&spec.recognizer_path)?;
    let dictionary = reader.digest(&spec.dictionary_path)?;
    ppocr_model_set_sha256(detector.as_deref(), &recognizer, &dictionary, None).map_err(|error| {
        Unidentified::Failed(
            Outcome::new(ContractCode::ForeignDataError)
                .raw_source(RawSource::Vision)
                .raw_text(error.message()),
        )
    })
}

/// One model folder's report entry; `identity` is `None` for a folder the check was not asked
/// about.
fn folder(
    model_ref: &str,
    family: VisionModelFamily,
    layout: Option<&str>,
    identity: Option<Result<String, Unidentified>>,
) -> Result<Value, ActingdError> {
    let mut entry = json!({"model_ref": model_ref, "family": family.as_str()});
    if let Some(layout) = layout {
        entry["layout"] = Value::from(layout);
    }
    entry["asked"] = Value::from(identity.is_some());
    match identity {
        None => {}
        Some(Ok(identity)) => entry["identity"] = Value::from(identity),
        Some(Err(Unidentified::Failed(cause))) => {
            entry["identity"] = Value::Null;
            entry["cause"] = envelope(&cause)?;
        }
        Some(Err(Unidentified::OverBound {
            path,
            size,
            bound,
            bound_bytes,
        })) => {
            entry["identity"] = Value::Null;
            entry["over_bound"] = json!({
                "path": path.to_string_lossy(),
                "size": size,
                "bound": bound.as_str(),
                "bound_bytes": bound_bytes,
            });
        }
    }
    Ok(entry)
}

/// One manifest model against the folders: whether a folder of its name is listed (`present`;
/// fix 2: an `absent` entry is none), that folder's identity (`null` without one, or for an invalid
/// folder or one not read), and whether it equals the model's (`identity_matches`).
fn checked_model(model: &ManifestModel, folders: &[Value]) -> Value {
    let folder = folders
        .iter()
        .find(|folder| folder["model_ref"] == model.model_ref.as_str() && folder["absent"] != true);
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
