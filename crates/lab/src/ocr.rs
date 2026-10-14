// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #396 LAB-3 (FCM T-02, L-02): the OCR provider of the offline tools. actinglab and
//! the Lab load the provider the Runtime uses: the in-process `ppocr-ctc` engine over the
//! model folders of the vision root that the Runtime configuration's `vision` section names
//! (Workflow #360), always on the CPU. As in the Runtime, listing reads only directory entries
//! and descriptions; model files are read and ONNX Runtime is initialised on the first OCR
//! request. Without a usable OCR model, OCR is unavailable under `lab_ocr_unavailable` with
//! the reason as its cause, and the offline tools report that code wherever they would
//! evaluate an OCR target; they never skip one silently.

use crate::codes::LabCode;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Detail, IoOp, Outcome, OutcomeEnvelope};
use actingcommand_contract::{InstalledProcess, LabError, LabResult};
use actingcommand_execution_kernel::VisionFfiProvider;
use actingcommand_page_detector::PageDetector;
use actingcommand_ppocr_onnx_json_provider::PpocrCtcLoader;
use actingcommand_recognition_pack::{RecognitionPack, RecognitionTarget, VisionProvider};
use actingcommand_vision_ffi::{
    ONNXRUNTIME_LIBRARY, OnnxExecutionProvider, VISION_MODELS_DIRECTORY, VISION_RUNTIME_DIRECTORY,
    VisionListingCode, VisionRuntimeSpec, list_vision_models,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The default vision root, beside the install root or else beside the configuration file, as
/// the Runtime resolves it.
const DEFAULT_VISION_ROOT: &str = "vision";

/// The OCR provider of one offline tool process, or why OCR is unavailable to it.
#[derive(Debug, Clone)]
pub struct LabOcr {
    /// The vision root the provider was looked for in, once it is known.
    vision_root: Option<PathBuf>,
    state: Result<Arc<dyn VisionProvider>, Outcome>,
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
    /// of its selected Runtime configuration. A process outside an installation has no Runtime
    /// configuration to read.
    pub fn from_process_installation() -> Self {
        match actingcommand_contract::process_installation() {
            Ok(installation) => Self::from_installation(installation),
            Err(error) => Self::unavailable(None, Outcome::new(error.code())),
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
    /// folder an OCR model.
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
        if listing.ocr.is_empty() {
            let cause = Outcome::new(LabCode::LabOcrModelMissing).path(&models);
            return Self::unavailable(Some(root), cause);
        }
        let loader = Arc::new(PpocrCtcLoader::new(runtime, None));
        match VisionFfiProvider::from_listing(listing, loader) {
            Ok(provider) => Self {
                vision_root: Some(root),
                state: Ok(Arc::new(provider)),
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
            state: Ok(provider),
        }
    }

    fn unavailable(vision_root: Option<PathBuf>, cause: Outcome) -> Self {
        let outcome = Outcome::new(LabCode::LabOcrUnavailable);
        let outcome = match &vision_root {
            Some(root) => outcome.path(root),
            None => outcome,
        };
        Self {
            vision_root,
            state: Err(outcome.caused_by(cause)),
        }
    }

    /// The provider every offline evaluation is given; `None` when OCR is unavailable.
    pub fn provider(&self) -> Option<Arc<dyn VisionProvider>> {
        self.state.as_ref().ok().cloned()
    }

    /// `lab_ocr_unavailable` with its cause, when OCR is unavailable.
    pub fn unavailable_outcome(&self) -> Option<&Outcome> {
        self.state.as_ref().err()
    }

    /// The vision root the provider was looked for in, once it is known.
    pub fn vision_root(&self) -> Option<&Path> {
        self.vision_root.as_deref()
    }

    /// Refuses with [`Self::unavailable_error`] when OCR is unavailable and evaluating one of
    /// `target_ids` reaches an OCR target: the target itself, or a member of a composite.
    pub fn require<'a>(
        &self,
        pack: &RecognitionPack,
        target_ids: impl IntoIterator<Item = &'a str>,
    ) -> LabResult<()> {
        if self.state.is_ok() {
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
        let outcome = self.state.as_ref().err()?;
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

/// `outcome` with the OS failure behind it, when the OS reported one.
fn with_os_cause(outcome: Outcome, cause: Option<&Outcome>) -> Outcome {
    match cause {
        Some(cause) => outcome.caused_by(cause.clone()),
        None => outcome,
    }
}

/// The OCR target evaluating `target_id` reaches: the target itself or the first OCR member
/// of a composite.
fn reached_ocr_target<'p>(pack: &'p RecognitionPack, target_id: &str) -> Option<&'p str> {
    let ocr = |id: &str| {
        pack.targets.iter().find_map(|target| match target {
            RecognitionTarget::Ocr(target) if target.id == id => Some(target.id.as_str()),
            _ => None,
        })
    };
    ocr(target_id).or_else(|| {
        pack.targets.iter().find_map(|target| match target {
            RecognitionTarget::Composite(composite) if composite.id == target_id => composite
                .members
                .iter()
                .find_map(|member| ocr(member.as_str())),
            _ => None,
        })
    })
}

#[cfg(test)]
mod tests;
