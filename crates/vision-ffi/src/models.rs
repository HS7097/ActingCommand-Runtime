// SPDX-License-Identifier: AGPL-3.0-only

//! Vision model folders and the loader contract of the in-process engine (Workflow #360).
//!
//! A vision root holds `ort\` (ONNX Runtime) and `models\<name>\`, one folder per model,
//! named by the `model_ref` packs use. An OCR folder has the flat layout `det.onnx`
//! (optional), `rec.onnx`, `keys.txt`, or the nested layout `det\inference.onnx` (optional),
//! `rec\inference.onnx`, `rec\keys.txt`; an NN classification folder has `model.onnx`. An
//! optional `model.json` describes the model. Listing reads only directory entries, file
//! metadata and descriptions; model files are read and hashed by the engine on first use.

use crate::{
    CudaDeviceSelector, NnEngine, OcrEngine, OcrSessionId, OnnxExecutionProvider, VisionFfiError,
    VisionFfiErrorCode, VisionFfiResult,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Domain separator of the engine binding digest recorded as an OCR execution's
/// `provider_binary_sha256`.
pub const OCR_ENGINE_BINDING_SCHEMA: &str = "actingcommand.ocr-engine-binding.v1";
/// The schema of a model folder's `model.json`.
pub const VISION_MODEL_DESCRIPTION_SCHEMA: &str = "actingcommand.vision_model.v1";
pub const VISION_MODEL_DESCRIPTION_FILE: &str = "model.json";
pub const VISION_MODELS_DIRECTORY: &str = "models";
pub const VISION_RUNTIME_DIRECTORY: &str = "ort";
pub const ONNXRUNTIME_LIBRARY: &str = "onnxruntime.dll";
pub const ONNXRUNTIME_SHARED_PROVIDERS_LIBRARY: &str = "onnxruntime_providers_shared.dll";
pub const ONNXRUNTIME_CUDA_PROVIDER_LIBRARY: &str = "onnxruntime_providers_cuda.dll";

const MODULE: &str = "vision-models";
const MAX_DESCRIPTION_BYTES: u64 = 64 * 1024;
const MAX_MODEL_NAME_BYTES: usize = 255;
const MAX_DESCRIPTION_LANGUAGES: usize = 64;
const MAX_LANGUAGE_BYTES: usize = 64;

/// The ONNX Runtime an engine initialises once per process, on its first model build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionRuntimeSpec {
    /// The exact ONNX Runtime library the engine initialises.
    pub onnxruntime_library: PathBuf,
    /// The libraries loaded first, in order, through the process runtime-library closure;
    /// it contains `onnxruntime_library` exactly once.
    pub runtime_library_closure: Vec<PathBuf>,
    /// The SHA-256 the ONNX Runtime library must have, when the configuration pins one.
    pub expected_onnxruntime_sha256: Option<String>,
    pub execution_provider: OnnxExecutionProvider,
    pub cuda_device: Option<CudaDeviceSelector>,
}

impl VisionRuntimeSpec {
    /// The ONNX Runtime of a vision root: `ort\onnxruntime_providers_shared.dll` when present,
    /// then `ort\onnxruntime.dll` and, for CUDA, `ort\onnxruntime_providers_cuda.dll`, loaded
    /// in that order. Only file metadata is read; a required library that is missing fails.
    pub fn from_vision_root(
        vision_root: &Path,
        execution_provider: OnnxExecutionProvider,
        cuda_device: Option<CudaDeviceSelector>,
    ) -> VisionFfiResult<Self> {
        let directory = vision_root.join(VISION_RUNTIME_DIRECTORY);
        let onnxruntime_library = directory.join(ONNXRUNTIME_LIBRARY);
        let mut runtime_library_closure = Vec::new();
        let shared = directory.join(ONNXRUNTIME_SHARED_PROVIDERS_LIBRARY);
        if regular_file(&shared).map_err(runtime_unavailable)? {
            runtime_library_closure.push(shared);
        }
        if !regular_file(&onnxruntime_library).map_err(runtime_unavailable)? {
            return Err(runtime_unavailable(format!(
                "{} is missing",
                onnxruntime_library.display()
            )));
        }
        runtime_library_closure.push(onnxruntime_library.clone());
        if execution_provider == OnnxExecutionProvider::Cuda {
            let cuda = directory.join(ONNXRUNTIME_CUDA_PROVIDER_LIBRARY);
            if !regular_file(&cuda).map_err(runtime_unavailable)? {
                return Err(runtime_unavailable(format!(
                    "{} is missing; it is required for execution_provider cuda",
                    cuda.display()
                )));
            }
            runtime_library_closure.push(cuda);
        }
        Ok(Self {
            onnxruntime_library,
            runtime_library_closure,
            expected_onnxruntime_sha256: None,
            execution_provider,
            cuda_device,
        })
    }
}

/// The model families a folder can declare; only these need engine code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisionModelFamily {
    /// PP-OCR detection plus CTC recognition (PP-OCR v3 to v6 and models retrained from them).
    PpocrCtc,
    /// One ONNX image classifier.
    OnnxClassify,
}

impl VisionModelFamily {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PpocrCtc => "ppocr-ctc",
            Self::OnnxClassify => "onnx-classify",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "ppocr-ctc" => Some(Self::PpocrCtc),
            "onnx-classify" => Some(Self::OnnxClassify),
            _ => None,
        }
    }
}

/// The file layout of an OCR model folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrModelLayout {
    /// `det.onnx`, `rec.onnx`, `keys.txt`.
    Flat,
    /// `det\inference.onnx`, `rec\inference.onnx`, `rec\keys.txt`.
    Nested,
}

impl OcrModelLayout {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Flat => "flat",
            Self::Nested => "nested",
        }
    }
}

/// A model folder's description: its `model.json`, or the family defaults when the folder
/// has none. Its canonical form fills every default, so a missing file and a file that
/// spells out the defaults describe the same model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VisionModelDescription {
    pub schema_version: String,
    pub family: String,
    /// Informational; recorded at startup and never compared with a target's languages.
    #[serde(default)]
    pub languages: Vec<String>,
}

impl VisionModelDescription {
    pub fn default_for(family: VisionModelFamily) -> Self {
        Self {
            schema_version: VISION_MODEL_DESCRIPTION_SCHEMA.to_string(),
            family: family.as_str().to_string(),
            languages: Vec::new(),
        }
    }

    fn validate(&self, family: VisionModelFamily) -> Result<(), String> {
        if self.schema_version != VISION_MODEL_DESCRIPTION_SCHEMA {
            return Err(format!(
                "model.json schema_version must be '{VISION_MODEL_DESCRIPTION_SCHEMA}'"
            ));
        }
        match VisionModelFamily::parse(&self.family) {
            Some(declared) if declared == family => {}
            Some(declared) => {
                return Err(format!(
                    "model.json declares family '{}' but the folder holds a '{}' model",
                    declared.as_str(),
                    family.as_str()
                ));
            }
            None => {
                return Err(format!(
                    "model.json family '{}' is unsupported (ppocr-ctc or onnx-classify)",
                    self.family
                ));
            }
        }
        if self.languages.len() > MAX_DESCRIPTION_LANGUAGES
            || self.languages.iter().any(|language| {
                language.trim().is_empty()
                    || language.len() > MAX_LANGUAGE_BYTES
                    || language.chars().any(char::is_control)
            })
        {
            return Err(format!(
                "model.json languages must hold at most {MAX_DESCRIPTION_LANGUAGES} non-blank values of at most {MAX_LANGUAGE_BYTES} bytes"
            ));
        }
        Ok(())
    }

    /// SHA-256 of the canonical JSON form.
    pub fn canonical_sha256(&self) -> VisionFfiResult<String> {
        let bytes = serde_json::to_vec(self).map_err(|error| {
            VisionFfiError::fatal(
                MODULE,
                format!("model description cannot be serialised: {error}"),
            )
        })?;
        Ok(sha256_hex(&bytes))
    }
}

/// One OCR model folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrModelSpec {
    pub model_ref: String,
    pub layout: OcrModelLayout,
    pub detector_path: Option<PathBuf>,
    pub recognizer_path: PathBuf,
    pub dictionary_path: PathBuf,
    pub description: VisionModelDescription,
    pub description_sha256: String,
}

/// One NN classification model folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NnModelSpec {
    pub model_ref: String,
    pub model_path: PathBuf,
    pub description: VisionModelDescription,
    pub description_sha256: String,
}

/// A model folder that breaks the folder rule; only requests naming it fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidModelFolder {
    pub name: String,
    pub path: PathBuf,
    pub reason: String,
}

/// The folders under one `models` directory, sorted by name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VisionModelListing {
    pub ocr: Vec<OcrModelSpec>,
    pub nn: Vec<NnModelSpec>,
    pub invalid: Vec<InvalidModelFolder>,
}

enum FolderModel {
    Ocr(OcrModelSpec),
    Nn(NnModelSpec),
}

/// Lists the model folders of `models_dir`. Plain files directly under it (the flat layout
/// of earlier releases, manifests) are not models and are skipped. A folder that breaks the
/// rule is listed as invalid with its reason; only an unreadable `models_dir` fails.
pub fn list_vision_models(models_dir: &Path) -> VisionFfiResult<VisionModelListing> {
    let unreadable = |error: std::io::Error| {
        VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::ProviderUnavailable,
            MODULE,
            format!(
                "vision models directory {} is unavailable: {error}",
                models_dir.display()
            ),
        )
    };
    let mut entries = Vec::new();
    for entry in fs::read_dir(models_dir).map_err(unreadable)? {
        let entry = entry.map_err(unreadable)?;
        let file_type = entry.file_type().map_err(unreadable)?;
        if file_type.is_file() {
            continue;
        }
        entries.push((entry.file_name(), entry.path(), file_type));
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut listing = VisionModelListing::default();
    for (name, path, file_type) in entries {
        let display_name = name.to_string_lossy().into_owned();
        let classified = if file_type.is_symlink() {
            Err("the folder is a symbolic link or junction".to_string())
        } else if !file_type.is_dir() {
            Err("the entry is neither a file nor a folder".to_string())
        } else {
            match name.to_str() {
                Some(name) => classify_folder(name, &path),
                None => Err("the folder name is not UTF-8".to_string()),
            }
        };
        match classified {
            Ok(FolderModel::Ocr(spec)) => listing.ocr.push(spec),
            Ok(FolderModel::Nn(spec)) => listing.nn.push(spec),
            Err(reason) => listing.invalid.push(InvalidModelFolder {
                name: display_name,
                path,
                reason,
            }),
        }
    }
    Ok(listing)
}

fn classify_folder(name: &str, folder: &Path) -> Result<FolderModel, String> {
    if name.trim().is_empty()
        || name.len() > MAX_MODEL_NAME_BYTES
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', ':'])
        || name.chars().any(char::is_control)
    {
        return Err(format!(
            "the folder name is not a valid model_ref (at most {MAX_MODEL_NAME_BYTES} bytes, no path separators, colons or control characters)"
        ));
    }
    if regular_file(&folder.join("cls.onnx"))? || directory(&folder.join("cls"))? {
        return Err("an angle classifier (cls) is not supported".to_string());
    }
    let flat_detector = regular_file(&folder.join("det.onnx"))?;
    let flat_recognizer = regular_file(&folder.join("rec.onnx"))?;
    let flat_dictionary = regular_file(&folder.join("keys.txt"))?;
    let nested_detector = directory(&folder.join("det"))?;
    let nested_recognizer = directory(&folder.join("rec"))?;
    let classifier = regular_file(&folder.join("model.onnx"))?;
    let flat = flat_detector || flat_recognizer || flat_dictionary;
    let nested = nested_detector || nested_recognizer;
    match (flat, nested, classifier) {
        (false, false, false) => Err(
            "the folder holds no model: expected rec.onnx and keys.txt, rec\\inference.onnx and rec\\keys.txt, or model.onnx"
                .to_string(),
        ),
        (true, false, false) => {
            if !flat_recognizer || !flat_dictionary {
                return Err("the flat OCR layout needs both rec.onnx and keys.txt".to_string());
            }
            let (description, description_sha256) =
                read_description(folder, VisionModelFamily::PpocrCtc)?;
            Ok(FolderModel::Ocr(OcrModelSpec {
                model_ref: name.to_string(),
                layout: OcrModelLayout::Flat,
                detector_path: flat_detector.then(|| folder.join("det.onnx")),
                recognizer_path: folder.join("rec.onnx"),
                dictionary_path: folder.join("keys.txt"),
                description,
                description_sha256,
            }))
        }
        (false, true, false) => {
            let recognizer = folder.join("rec").join("inference.onnx");
            let dictionary = folder.join("rec").join("keys.txt");
            if !nested_recognizer || !regular_file(&recognizer)? || !regular_file(&dictionary)? {
                return Err(
                    "the nested OCR layout needs rec\\inference.onnx and rec\\keys.txt"
                        .to_string(),
                );
            }
            let detector = folder.join("det").join("inference.onnx");
            if nested_detector && !regular_file(&detector)? {
                return Err(
                    "det\\ exists but det\\inference.onnx is missing in the nested OCR layout"
                        .to_string(),
                );
            }
            let (description, description_sha256) =
                read_description(folder, VisionModelFamily::PpocrCtc)?;
            Ok(FolderModel::Ocr(OcrModelSpec {
                model_ref: name.to_string(),
                layout: OcrModelLayout::Nested,
                detector_path: nested_detector.then_some(detector),
                recognizer_path: recognizer,
                dictionary_path: dictionary,
                description,
                description_sha256,
            }))
        }
        (false, false, true) => {
            let (description, description_sha256) =
                read_description(folder, VisionModelFamily::OnnxClassify)?;
            Ok(FolderModel::Nn(NnModelSpec {
                model_ref: name.to_string(),
                model_path: folder.join("model.onnx"),
                description,
                description_sha256,
            }))
        }
        _ => Err(
            "the folder mixes model layouts: use the flat OCR layout, the nested OCR layout or model.onnx, not more than one"
                .to_string(),
        ),
    }
}

fn read_description(
    folder: &Path,
    family: VisionModelFamily,
) -> Result<(VisionModelDescription, String), String> {
    let path = folder.join(VISION_MODEL_DESCRIPTION_FILE);
    let description = match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == ErrorKind::NotFound => {
            VisionModelDescription::default_for(family)
        }
        Err(error) => return Err(format!("model.json is unavailable: {error}")),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err("model.json is not a regular file".to_string());
        }
        Ok(metadata) if metadata.len() > MAX_DESCRIPTION_BYTES => {
            return Err(format!("model.json exceeds {MAX_DESCRIPTION_BYTES} bytes"));
        }
        Ok(_) => {
            let bytes =
                fs::read(&path).map_err(|error| format!("model.json is unreadable: {error}"))?;
            let description: VisionModelDescription = serde_json::from_slice(&bytes)
                .map_err(|error| format!("model.json is invalid: {error}"))?;
            description.validate(family)?;
            description
        }
    };
    let description_sha256 = description
        .canonical_sha256()
        .map_err(|error| error.message().to_string())?;
    Ok((description, description_sha256))
}

/// True for a regular file, false when nothing is there; anything else is an error.
fn regular_file(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(format!("{} is a symbolic link", path.display()))
        }
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(format!("{} is not a regular file", path.display())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("{} is unavailable: {error}", path.display())),
    }
}

/// True for a directory, false when nothing is there; anything else is an error.
fn directory(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(format!("{} is a symbolic link or junction", path.display()))
        }
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => Err(format!("{} is not a folder", path.display())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("{} is unavailable: {error}", path.display())),
    }
}

fn runtime_unavailable(message: impl Into<String>) -> VisionFfiError {
    VisionFfiError::fatal_with_code(VisionFfiErrorCode::ProviderUnavailable, MODULE, message)
}

/// The outcome of one OCR model load.
pub enum OcrModelLoad {
    /// The content identity equals the requested one and the engine is ready.
    Loaded {
        engine: Box<dyn OcrEngine + Send>,
        model_sha256: String,
    },
    /// The files hash to a different content identity; no session was built.
    Mismatch { model_sha256: String },
}

/// The outcome of one NN model load.
pub enum NnModelLoad {
    /// The content identity equals the requested one and the engine is ready.
    Loaded {
        engine: Box<dyn NnEngine + Send>,
        model_sha256: String,
    },
    /// The file hashes to a different content identity; no session was built.
    Mismatch { model_sha256: String },
}

/// Builds the engine of one model on first use. Implemented by the in-process engine and
/// injected by the composition root, so the Runtime never links ONNX Runtime itself.
pub trait VisionModelLoader: Send + Sync {
    /// Reads the model's files once, computes their content identity and, when it equals
    /// `expected_model_sha256`, builds the engine whose executions carry `session_id`.
    /// Waiting for another request's runtime initialisation ends at `wait_deadline`.
    fn load_ocr(
        &self,
        model: &OcrModelSpec,
        expected_model_sha256: &str,
        session_id: &OcrSessionId,
        wait_deadline: Instant,
    ) -> VisionFfiResult<OcrModelLoad>;

    /// Reads `model.onnx` once, computes its SHA-256 and, when it equals
    /// `expected_model_sha256`, builds the classifier.
    fn load_nn(
        &self,
        model: &NnModelSpec,
        expected_model_sha256: &str,
        wait_deadline: Instant,
    ) -> VisionFfiResult<NnModelLoad>;
}

/// The engine binding digest: the running executable, the ONNX Runtime library and the
/// model description of one OCR session.
pub fn ocr_engine_binding_sha256(
    executable_sha256: &str,
    onnxruntime_sha256: &str,
    description_sha256: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(OCR_ENGINE_BINDING_SCHEMA.as_bytes());
    hasher.update(b"\0");
    for value in [executable_sha256, onnxruntime_sha256, description_sha256] {
        hasher.update(value.as_bytes());
        hasher.update(b"\0");
    }
    lower_hex(&hasher.finalize())
}

/// Lowercase hexadecimal SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    lower_hex(&Sha256::digest(bytes))
}

/// Lowercase hexadecimal SHA-256 of the file at `path`, read in bounded chunks.
pub fn sha256_file_hex(module: &'static str, path: &Path) -> VisionFfiResult<String> {
    let mut file = fs::File::open(path).map_err(|error| {
        VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::ProviderUnavailable,
            module,
            format!("failed to open {} for hashing: {error}", path.display()),
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::ProviderUnavailable,
                module,
                format!("failed to read {} for hashing: {error}", path.display()),
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(lower_hex(&hasher.finalize()))
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(HEX[usize::from(byte >> 4)]));
        value.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    value
}
