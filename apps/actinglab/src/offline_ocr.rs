// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #396 LAB-3: actinglab's one OCR provider, the Runtime's, loaded once per process
//! from the installation actinglab runs from (`LabOcr`). Every offline package load gets it;
//! without an OCR model a command that would evaluate an OCR target refuses with
//! `lab_ocr_unavailable` before evaluating anything. #411 Lab L6: a vision root with NN models
//! and no OCR model gives the provider too, for NN targets; OCR is then unavailable.
//!
//! Workflow #428 LAB-OCR-DL (register §1.331): the switch [`OCR_DEADLINE_MAX_FLAG`] of the offline
//! evaluations of saved scenes gives their package load the provider with the OCR inference
//! deadline raised (`actingcommand_lab::with_ocr_deadline_max`), and their answer the member
//! [`OCR_DEADLINE_MEMBER`]; it is refused where the frame is not a saved scene.

use super::{CliError, CliOutcome, FlagArgs};
use actingcommand_lab::{
    ExternallyVerifiedBundle, LabOcr, OCR_DEADLINE_MAX_MS, with_ocr_deadline_max,
};
use actingcommand_recognition_pack::{RecognitionEvaluator, RecognitionTarget, VisionProvider};
use serde_json::{Value, json};
use std::sync::{Arc, OnceLock};

/// The process's OCR provider, or why OCR is unavailable to it.
pub(super) fn lab_ocr() -> &'static LabOcr {
    static OCR: OnceLock<LabOcr> = OnceLock::new();
    OCR.get_or_init(LabOcr::from_process_installation)
}

/// Workflow #428 LAB-OCR-DL: the switch that takes each OCR target's `timeout_ms` as the contract's
/// most for an offline evaluation of saved scenes (the rows `observe`, `do`, `ensure`, `wait`,
/// `recognize`, `detect-page`, `current-page`, `is-visible`, `tap-target` and `navigate`, each on
/// the path that reads `--scene`).
pub(super) const OCR_DEADLINE_MAX_FLAG: &str = "--ocr-deadline-max";

/// Workflow #428 LAB-OCR-DL: the member of an answer the switch raised the OCR inference deadline
/// of: the deadline it was raised to, in milliseconds. No answer without the switch has it.
pub(super) const OCR_DEADLINE_MEMBER: &str = "ocr_deadline_raised_to_ms";

/// Workflow #428 LAB-OCR-DL: whether the command line raises the OCR inference deadline: the switch
/// given where the frame is a saved scene (`saved_scene`: `--scene` names one, and no `--capture`).
/// Given elsewhere it is refused before anything is loaded or read, as a flag that needs another
/// one is: `validation_failed` naming it, `conflicting`. Read only by a command whose flag list
/// holds the switch.
pub(super) fn ocr_deadline_max(flags: &FlagArgs, saved_scene: bool) -> CliOutcome<bool> {
    if !flags.bool(OCR_DEADLINE_MAX_FLAG) {
        return Ok(false);
    }
    if saved_scene {
        return Ok(true);
    }
    Err(CliError::usage(format!(
        "{OCR_DEADLINE_MAX_FLAG} raises the OCR inference deadline of an offline evaluation of saved \
         scenes only: give --scene, without --capture"
    ))
    .with_details(json!({
        "argument": OCR_DEADLINE_MAX_FLAG,
        "argument_problem": "conflicting",
    })))
}

/// Workflow #428 LAB-OCR-DL: the provider an offline package load gives its evaluator: the
/// process's ([`lab_ocr`]), its OCR requests' deadline raised when `raised`.
pub(super) fn evaluation_provider(raised: bool) -> Option<Arc<dyn VisionProvider>> {
    let provider = lab_ocr().provider();
    if raised {
        provider.map(with_ocr_deadline_max)
    } else {
        provider
    }
}

/// Workflow #428 LAB-OCR-DL: `answer` with [`OCR_DEADLINE_MEMBER`] when the switch `raised` the
/// OCR inference deadline, unchanged otherwise. An answer the member cannot go into, one that is
/// no JSON object, is refused, never answered without it.
pub(super) fn with_ocr_deadline(answer: Value, raised: bool) -> CliOutcome<Value> {
    if !raised {
        return Ok(answer);
    }
    let Value::Object(mut members) = answer else {
        return Err(CliError::device(format!(
            "an answer that is no JSON object cannot carry {OCR_DEADLINE_MEMBER}"
        )));
    };
    members.insert(OCR_DEADLINE_MEMBER.to_owned(), json!(OCR_DEADLINE_MAX_MS));
    Ok(Value::Object(members))
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
        "available": ocr.ocr_provider().is_some(),
        "execution_provider": "cpu",
        "vision_root": ocr.vision_root().map(|root| root.display().to_string()),
    });
    if let Some(details) = ocr.unavailable_error(None).and_then(|error| error.details) {
        status["unavailable"] = details;
    }
    status
}
