// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #396 LAB-3 (FCM T-02, L-02): the OCR provider of the offline tools. actinglab and
//! the Lab load the provider the Runtime uses: the in-process `ppocr-ctc` engine over the
//! model folders of the vision root that the Runtime configuration's `vision` section names
//! (Workflow #360), always on the CPU. As in the Runtime, listing reads only directory entries
//! and descriptions; model files are read and ONNX Runtime is initialised on the first OCR
//! request. Without a usable OCR model, OCR is unavailable under `lab_ocr_unavailable` with
//! the reason as its cause, and the offline tools report that code wherever they would
//! evaluate an OCR target; they never skip one silently.
//!
//! Workflow #411 A12: a process outside an A/B installation that runs from a single-location
//! root (`runtime\BUILD-MANIFEST.json` two levels above it, no install selection; register §1.96)
//! reads that root's `actingd.config.json`.
//!
//! #411 Lab L6: a vision root with NN models and no OCR model gives the offline tools a provider,
//! as the Runtime accepts such a root: NN targets evaluate with it, while OCR stays unavailable
//! under `lab_ocr_unavailable` caused by `lab_ocr_model_missing`, as before.

use crate::codes::LabCode;
use actingcommand_contract::codes::{InstallationCode, SharedCode};
use actingcommand_contract::outcome::{Detail, IoOp, Outcome, OutcomeEnvelope};
use actingcommand_contract::{InstalledProcess, LabError, LabResult};
use actingcommand_execution_kernel::VisionFfiProvider;
use actingcommand_page_detector::PageDetector;
use actingcommand_ppocr_onnx_json_provider::PpocrCtcLoader;
use actingcommand_recognition_pack::{
    RecognitionPack, RecognitionTarget, ValueTestRight, VisionProvider,
};
use actingcommand_vision_ffi::{
    ONNXRUNTIME_LIBRARY, OnnxExecutionProvider, VISION_MODELS_DIRECTORY, VISION_RUNTIME_DIRECTORY,
    VisionListingCode, VisionRuntimeSpec, list_vision_models,
};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The default vision root, beside the install root or else beside the configuration file, as
/// the Runtime resolves it.
const DEFAULT_VISION_ROOT: &str = "vision";

/// The Runtime configuration of a single-location root, directly in the root (WD-1, register
/// §1.96).
const ROOT_CONFIGURATION_FILE: &str = "actingd.config.json";

/// The largest Runtime configuration read, as an installation's selected configuration
/// (`InstalledProcess::config_bytes`) and mcp-serve's.
const MAX_CONFIGURATION_BYTES: u64 = 1024 * 1024;

/// The vision provider of one offline tool process, and why OCR is unavailable to it.
#[derive(Debug, Clone)]
pub struct LabOcr {
    /// The vision root the provider was looked for in, once it is known.
    vision_root: Option<PathBuf>,
    /// The provider every offline evaluation is given; `None` when the root gives none.
    provider: Option<Arc<dyn VisionProvider>>,
    /// `lab_ocr_unavailable` with its cause when the provider reads no text: there is none, or
    /// (#411 Lab L6) its root holds NN models and no OCR model.
    ocr_unavailable: Option<Outcome>,
}

/// The parts of the Runtime configuration the offline tools read: only the vision root.
#[derive(Deserialize)]
struct RuntimeConfiguration {
    #[serde(default)]
    vision: Option<VisionSection>,
}

/// `vision.execution_provider` and `vision.cuda_device` are not read: the offline tools run
/// every model on the CPU.
#[derive(Deserialize)]
struct VisionSection {
    #[serde(default)]
    root: Option<PathBuf>,
}

/// The details of a `lab_ocr_unavailable` Lab error.
#[derive(Serialize)]
struct UnavailableDetails {
    outcome: OutcomeEnvelope,
}

impl LabOcr {
    /// The provider of this process's installation (`process_installation`): the vision root
    /// of its selected Runtime configuration. Outside an installation, a process that runs from
    /// a single-location root reads that root's configuration (Workflow #411 A12); a process
    /// outside both has no Runtime configuration to read.
    pub fn from_process_installation() -> Self {
        match actingcommand_contract::process_installation() {
            Ok(Some(installation)) => Self::from_installation(Some(installation)),
            Ok(None) => match process_single_location_root() {
                Ok(Some(root)) => Self::from_single_location_root(&root),
                Ok(None) => Self::from_installation(None),
                Err(cause) => Self::unavailable(None, cause),
            },
            Err(error) => Self::unavailable(None, Outcome::new(error.code())),
        }
    }

    /// The provider of the single-location root `root` (Workflow #411 A12): its configuration
    /// `<root>\actingd.config.json`, a regular file of at most 1 MiB, names the vision root
    /// (`vision.root`, else `<root>\vision`). A configuration that is missing, unreadable or
    /// larger is `config_read_failed`, one that does not decode `config_decode_failed`: such a
    /// root is never "unconfigured".
    pub fn from_single_location_root(root: &Path) -> Self {
        match read_root_configuration(&root.join(ROOT_CONFIGURATION_FILE)) {
            Ok(configuration) => Self::from_runtime_configuration(&configuration, root, Some(root)),
            Err(cause) => Self::unavailable(None, cause),
        }
    }

    /// The provider of an installation's selected Runtime configuration; `None` has none.
    pub fn from_installation(installation: Option<&InstalledProcess>) -> Self {
        let Some(installation) = installation else {
            return Self::unavailable(None, Outcome::new(LabCode::LabOcrVisionUnconfigured));
        };
        let configuration = match installation.config_bytes() {
            Ok(bytes) => bytes,
            Err(error) => return Self::unavailable(None, Outcome::new(error.code())),
        };
        let path = installation.config_path();
        let directory = path.parent().unwrap_or_else(|| installation.root());
        Self::from_runtime_configuration(&configuration, directory, Some(installation.root()))
    }

    /// The provider a Runtime configuration names: `vision.root` (absolute, or relative to the
    /// configuration's directory) or, without it, `vision` beside the install root, else beside
    /// the configuration. A configuration without a `vision` section configures no OCR.
    pub fn from_runtime_configuration(
        configuration: &[u8],
        configuration_directory: &Path,
        install_root: Option<&Path>,
    ) -> Self {
        let configuration = match serde_json::from_slice::<RuntimeConfiguration>(configuration) {
            Ok(configuration) => configuration,
            Err(_) => {
                return Self::unavailable(None, Outcome::new(SharedCode::ConfigDecodeFailed));
            }
        };
        let Some(vision) = configuration.vision else {
            return Self::unavailable(None, Outcome::new(LabCode::LabOcrVisionUnconfigured));
        };
        let root = match vision.root {
            Some(root) if root.as_os_str().is_empty() => {
                return Self::unavailable(None, Outcome::new(SharedCode::ConfigDecodeFailed));
            }
            Some(root) if root.is_absolute() => root,
            Some(root) => configuration_directory.join(root),
            None => install_root
                .unwrap_or(configuration_directory)
                .join(DEFAULT_VISION_ROOT),
        };
        Self::from_vision_root(&root)
    }

    /// The provider over the model folders of the vision root `root`, on the CPU: the root and
    /// its `models` folder must be readable, `ort\onnxruntime.dll` present, and at least one
    /// folder an OCR model. #411 Lab L6: a root whose folders hold NN models and no OCR model
    /// gives the provider too, for NN targets; OCR is then unavailable, caused by
    /// `lab_ocr_model_missing`.
    pub fn from_vision_root(root: &Path) -> Self {
        let root = match std::fs::canonicalize(root) {
            Ok(root) => root,
            Err(error) => {
                return Self::unavailable(
                    Some(root.to_path_buf()),
                    Outcome::new(VisionListingCode::VisionRootUnavailable)
                        .caused_by(Outcome::from_io_error(&error, IoOp::Canonicalize)),
                );
            }
        };
        let models = root.join(VISION_MODELS_DIRECTORY);
        let listing = match list_vision_models(&models) {
            Ok(listing) => listing,
            Err(error) => {
                let cause = with_os_cause(
                    Outcome::new(VisionListingCode::VisionRootUnavailable),
                    error.os_cause(),
                );
                return Self::unavailable(Some(root), cause);
            }
        };
        let runtime =
            match VisionRuntimeSpec::from_vision_root(&root, OnnxExecutionProvider::Cpu, None) {
                Ok(runtime) => runtime,
                Err(error) => {
                    let library = root
                        .join(VISION_RUNTIME_DIRECTORY)
                        .join(ONNXRUNTIME_LIBRARY);
                    let cause = with_os_cause(
                        Outcome::new(LabCode::LabOcrRuntimeMissing).path(library),
                        error.os_cause(),
                    );
                    return Self::unavailable(Some(root), cause);
                }
            };
        if listing.ocr.is_empty() && listing.nn.is_empty() && listing.invalid.is_empty() {
            return Self::unavailable(
                Some(root),
                Outcome::new(VisionListingCode::VisionModelsEmpty),
            );
        }
        if listing.ocr.is_empty() && listing.nn.is_empty() {
            let cause = Outcome::new(LabCode::LabOcrModelMissing).path(&models);
            return Self::unavailable(Some(root), cause);
        }
        let ocr_missing = listing
            .ocr
            .is_empty()
            .then(|| Outcome::new(LabCode::LabOcrModelMissing).path(&models));
        let loader = Arc::new(PpocrCtcLoader::new(runtime, None));
        match VisionFfiProvider::from_listing(listing, loader) {
            // #411 Lab L6: without an OCR model the provider serves the root's NN models.
            Ok(provider) => Self {
                ocr_unavailable: ocr_missing.map(|cause| unavailable_outcome(Some(&root), cause)),
                vision_root: Some(root),
                provider: Some(Arc::new(provider)),
            },
            // The provider refuses only a listing without any folder, refused above.
            Err(_) => Self::unavailable(
                Some(root),
                Outcome::new(VisionListingCode::VisionModelsEmpty),
            ),
        }
    }

    /// A provider the caller built: a tool with its own engine, or a test's scripted one.
    pub fn with_provider(provider: Arc<dyn VisionProvider>) -> Self {
        Self {
            vision_root: None,
            provider: Some(provider),
            ocr_unavailable: None,
        }
    }

    fn unavailable(vision_root: Option<PathBuf>, cause: Outcome) -> Self {
        Self {
            ocr_unavailable: Some(unavailable_outcome(vision_root.as_deref(), cause)),
            vision_root,
            provider: None,
        }
    }

    /// The provider every offline evaluation is given; `None` when the root gives none. #411
    /// Lab L6: a root with NN models and no OCR model gives one, which evaluates NN targets
    /// while OCR stays unavailable ([`Self::ocr_provider`]).
    pub fn provider(&self) -> Option<Arc<dyn VisionProvider>> {
        self.provider.clone()
    }

    /// The provider when it reads text; `None` when OCR is unavailable.
    pub fn ocr_provider(&self) -> Option<Arc<dyn VisionProvider>> {
        self.provider
            .clone()
            .filter(|_| self.ocr_unavailable.is_none())
    }

    /// `lab_ocr_unavailable` with its cause, when OCR is unavailable.
    pub fn unavailable_outcome(&self) -> Option<&Outcome> {
        self.ocr_unavailable.as_ref()
    }

    /// The vision root the provider was looked for in, once it is known.
    pub fn vision_root(&self) -> Option<&Path> {
        self.vision_root.as_deref()
    }

    /// Refuses with [`Self::unavailable_error`] when OCR is unavailable and evaluating one of
    /// `target_ids` reaches an OCR target: the target itself, a side of a value test (Workflow
    /// #396 REC-6), or either as a member of a composite.
    pub fn require<'a>(
        &self,
        pack: &RecognitionPack,
        target_ids: impl IntoIterator<Item = &'a str>,
    ) -> LabResult<()> {
        if self.ocr_unavailable.is_none() {
            return Ok(());
        }
        for target_id in target_ids {
            if let Some(ocr) = reached_ocr_target(pack, target_id)
                && let Some(error) = self.unavailable_error(Some(ocr))
            {
                return Err(error);
            }
        }
        Ok(())
    }

    /// [`Self::require`] for every target a page set evaluates: page detection evaluates every
    /// target of every page.
    pub fn require_pages(&self, pack: &RecognitionPack, detector: &PageDetector) -> LabResult<()> {
        self.require(
            pack,
            detector.page_definitions().iter().flat_map(|page| {
                page.required
                    .iter()
                    .chain(page.any_of.iter().flatten())
                    .chain(page.optional.iter())
                    .chain(page.forbidden.iter())
                    .map(String::as_str)
            }),
        )
    }

    /// `lab_ocr_unavailable` as a Lab error, naming the OCR target that needed it; `None`
    /// when OCR is available. Its details carry the outcome with its cause.
    pub fn unavailable_error(&self, target_id: Option<&str>) -> Option<LabError> {
        let outcome = self.ocr_unavailable.as_ref()?;
        // The key table's id rule: at most 256 bytes, no control character.
        let outcome = match target_id {
            Some(target_id)
                if target_id.len() <= 256 && !target_id.chars().any(char::is_control) =>
            {
                outcome.clone().target_id(target_id)
            }
            _ => outcome.clone(),
        };
        let error = LabError::safety_blocked(
            LabCode::LabOcrUnavailable.as_str(),
            format!("OCR is unavailable to the offline tools: {outcome}"),
            &["ocr_model"],
        );
        Some(
            match serde_json::to_value(UnavailableDetails {
                outcome: outcome.envelope(Detail::Full),
            }) {
                Ok(details) => error.with_details(details),
                Err(_) => error,
            },
        )
    }
}

/// `lab_ocr_unavailable`, with the vision root as its `path` once it is known, caused by `cause`.
fn unavailable_outcome(vision_root: Option<&Path>, cause: Outcome) -> Outcome {
    let outcome = Outcome::new(LabCode::LabOcrUnavailable);
    let outcome = match vision_root {
        Some(root) => outcome.path(root),
        None => outcome,
    };
    outcome.caused_by(cause)
}

/// `outcome` with the OS failure behind it, when the OS reported one.
fn with_os_cause(outcome: Outcome, cause: Option<&Outcome>) -> Outcome {
    match cause {
        Some(cause) => outcome.caused_by(cause.clone()),
        None => outcome,
    }
}

/// Workflow #406 T4: the Runtime configuration of this process's root, read as the OCR loader
/// reads it: an installation's selected configuration, else a single-location root's
/// `actingd.config.json` (Workflow #411 A12); `None` outside both. A configuration that cannot be
/// read is its failure (`config_read_failed` with its cause, or the installation's code).
pub(crate) fn process_root_configuration() -> Result<Option<Vec<u8>>, Outcome> {
    match actingcommand_contract::process_installation() {
        Ok(Some(installation)) => installation
            .config_bytes()
            .map(Some)
            .map_err(|error| Outcome::new(error.code())),
        Ok(None) => match process_single_location_root()? {
            Some(root) => read_root_configuration(&root.join(ROOT_CONFIGURATION_FILE)).map(Some),
            None => Ok(None),
        },
        Err(error) => Err(Outcome::new(error.code())),
    }
}

/// The single-location root this process runs from (Workflow #411 A12): [`single_location_root`]
/// of the canonical executable. An executable that cannot be resolved is
/// `install_executable_unavailable`, as `process_installation` answers it.
fn process_single_location_root() -> Result<Option<PathBuf>, Outcome> {
    let unresolved = |error: &std::io::Error, op: IoOp| {
        Outcome::new(InstallationCode::InstallExecutableUnavailable)
            .caused_by(Outcome::from_io_error(error, op))
    };
    let executable = std::env::current_exe().map_err(|error| unresolved(&error, IoOp::Locate))?;
    let executable = std::fs::canonicalize(&executable)
        .map_err(|error| unresolved(&error, IoOp::Canonicalize))?;
    Ok(single_location_root(&executable))
}

/// Workflow #411 A12: the single-location root of an executable at the canonical path
/// `executable`: the folder two levels above it, when its `runtime\BUILD-MANIFEST.json` is a file
/// and it is no `A`/`B` slot folder (the rule of `process_installation` and of actingctl's install
/// root); otherwise none.
fn single_location_root(executable: &Path) -> Option<PathBuf> {
    let root = executable.parent()?.parent()?;
    let slot_folder = matches!(
        root.file_name().and_then(|name| name.to_str()),
        Some("A" | "B")
    );
    if slot_folder || !root.join("runtime").join("BUILD-MANIFEST.json").is_file() {
        return None;
    }
    Some(root.to_path_buf())
}

/// Reads a single-location root's Runtime configuration `path`: a regular file of at most
/// [`MAX_CONFIGURATION_BYTES`]. A missing, unreadable or larger file is `config_read_failed` with
/// the `path`, caused by the OS failure when the OS reported one.
fn read_root_configuration(path: &Path) -> Result<Vec<u8>, Outcome> {
    let unreadable = || Outcome::new(SharedCode::ConfigReadFailed).path(path);
    let failed = |error: &std::io::Error, op: IoOp| {
        unreadable().caused_by(Outcome::from_io_error(error, op))
    };
    let file = std::fs::File::open(path).map_err(|error| failed(&error, IoOp::Open))?;
    let metadata = file
        .metadata()
        .map_err(|error| failed(&error, IoOp::Metadata))?;
    if !metadata.is_file() {
        return Err(unreadable());
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONFIGURATION_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| failed(&error, IoOp::Read))?;
    if bytes.len() as u64 > MAX_CONFIGURATION_BYTES {
        return Err(unreadable());
    }
    Ok(bytes)
}

/// The OCR target evaluating `target_id` reaches: the target itself, the OCR target a side of a
/// value test reads (Workflow #396 REC-6; the left side first), or the first of those a member
/// of a composite reaches.
fn reached_ocr_target<'p>(pack: &'p RecognitionPack, target_id: &str) -> Option<&'p str> {
    let ocr = |id: &str| {
        pack.targets.iter().find_map(|target| match target {
            RecognitionTarget::Ocr(target) if target.id == id => Some(target.id.as_str()),
            _ => None,
        })
    };
    let reached = |id: &str| {
        ocr(id).or_else(|| {
            pack.targets.iter().find_map(|target| match target {
                RecognitionTarget::ValueTest(test) if test.id == id => {
                    ocr(test.left.target_id.as_str()).or_else(|| match &test.right {
                        Some(ValueTestRight::Operand(operand)) => ocr(operand.target_id.as_str()),
                        _ => None,
                    })
                }
                _ => None,
            })
        })
    };
    reached(target_id).or_else(|| {
        pack.targets.iter().find_map(|target| match target {
            RecognitionTarget::Composite(composite) if composite.id == target_id => composite
                .members
                .iter()
                .find_map(|member| reached(member.as_str())),
            _ => None,
        })
    })
}

#[cfg(test)]
mod tests;
// Workflow #411 A12 part 1 (exec key 87): a single-location root's configuration names the vision
// root of the offline tools.
#[cfg(test)]
mod gate_rc2_a12;
// #411 Lab L6 (exec key 99): a vision root with NN models and no OCR model gives the offline tools
// a provider for NN targets; OCR stays unavailable, caused by `lab_ocr_model_missing`.
#[cfg(test)]
mod gate_lab_l6;
