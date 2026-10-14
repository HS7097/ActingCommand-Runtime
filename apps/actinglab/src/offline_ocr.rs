// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #396 LAB-3: actinglab's one OCR provider, the Runtime's, loaded once per process
//! from the installation actinglab runs from (`LabOcr`). Every offline package load gets it;
//! without an OCR model a command that would evaluate an OCR target refuses with
//! `lab_ocr_unavailable` before evaluating anything.

use super::CliOutcome;
use actingcommand_lab::{ExternallyVerifiedBundle, LabOcr};
use actingcommand_recognition_pack::{RecognitionEvaluator, RecognitionTarget};
use serde_json::{Value, json};
use std::sync::OnceLock;

/// The process's OCR provider, or why OCR is unavailable to it.
pub(super) fn lab_ocr() -> &'static LabOcr {
    static OCR: OnceLock<LabOcr> = OnceLock::new();
    OCR.get_or_init(LabOcr::from_process_installation)
}

/// Refuses when evaluating `target` reaches an OCR target and OCR is unavailable.
pub(super) fn require_target(evaluator: &RecognitionEvaluator, target: &str) -> CliOutcome<()> {
    lab_ocr().require(evaluator.pack(), [target])
}

/// Refuses when the bundle's page detection reaches an OCR target and OCR is unavailable:
/// page detection evaluates every target of every page.
pub(super) fn require_pages(resources: &ExternallyVerifiedBundle) -> CliOutcome<()> {
    let bundle = resources.loaded_bundle();
    match (bundle.evaluator(), bundle.detector()) {
        (Some(evaluator), Some(detector)) => lab_ocr().require_pages(evaluator.pack(), detector),
        _ => Ok(()),
    }
}

/// Workflow #308 SEL-L6a: refuses when OCR is unavailable and the bundle's recognition pack
/// declares an OCR target. A whole-package replay may evaluate any target of its pack: page
/// detection, guards, select steps, post-admission fields and resource readings.
pub(super) fn require_package(resources: &ExternallyVerifiedBundle) -> CliOutcome<()> {
    let Some(evaluator) = resources.loaded_bundle().evaluator() else {
        return Ok(());
    };
    let pack = evaluator.pack();
    lab_ocr().require(
        pack,
        pack.targets.iter().filter_map(|target| match target {
            RecognitionTarget::Ocr(target) => Some(target.id.as_str()),
            _ => None,
        }),
    )
}

/// The OCR state an offline command reports: available on the CPU with its vision root, or
/// unavailable with `lab_ocr_unavailable` and its cause.
pub(super) fn status() -> Value {
    let ocr = lab_ocr();
    let mut status = json!({
        "available": ocr.provider().is_some(),
        "execution_provider": "cpu",
        "vision_root": ocr.vision_root().map(|root| root.display().to_string()),
    });
    if let Some(details) = ocr.unavailable_error(None).and_then(|error| error.details) {
        status["unavailable"] = details;
    }
    status
}
