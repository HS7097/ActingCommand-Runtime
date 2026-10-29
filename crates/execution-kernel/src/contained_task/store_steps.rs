// SPDX-License-Identifier: AGPL-3.0-only

//! STORE-FG-1 (since 0.12.29; design `rd-1029/design-store-fg.md` §2, §3; contracts
//! `resource-declarations.md`, "Store update packages"): kernel admission's cross-checks of a
//! control declaration's `store_update` against the package's task and pages. The member's own
//! shape is checked where it is read (`parse_task_control_declaration`); the routing of a
//! declared store step's tap is the run's (`ContainedTaskRuntime::store_input`).
//!
//! - Each store operation names one operation of the task whose effect is a click that draws a
//!   tap, from one page of the package (`store_operation_invalid`).
//! - Each of its proof targets is an OCR target with an `expected` text that stands directly in
//!   the `required` list of that `from` page (`store_proof_invalid`), so the kernel, which
//!   dispatches an operation only after its `from` page passed, taps only on a frame that showed
//!   the proof.
//! - The package brings the assigned application back with an `application` `launch` or
//!   `restart` operation (`store_way_back_missing`).
//!
//! Each refusal is `contained_task_program_invalid` with its `program_problem`, the path
//! `control.json` and the refused member as `json_pointer`.

use super::{
    ContainedTaskError, PreparedContainedTask, TaskControl, TaskProgram, linear, program_refusal,
    recognition_target_id, resolve_page_reference,
};
use crate::codes::ProgramProblem;
use actingcommand_contract::ApplicationLifecycleAction;
use actingcommand_page_detector::PageDetector;
use actingcommand_recognition_pack::{RecognitionEvaluator, RecognitionTarget};

/// The package file that holds `store_update`, as a refusal names it.
const CONTROL_PATH: &str = "control.json";

/// The click kinds whose input is one `Tap` (design §2.3: only taps).
const TAP_KINDS: [&str; 6] = [
    "point",
    "rect",
    "specific_rect",
    "target",
    "target_center",
    "offset",
];

/// The tap kinds whose place the package fixes, not the frame (design §8.1).
const FIXED_TAP_KINDS: [&str; 3] = ["point", "rect", "specific_rect"];

impl TaskProgram {
    /// The cross-checks of the control's `store_update`, when it declares one (module doc).
    pub(super) fn validate_store_operations(
        &self,
        control: &TaskControl,
        detector: &PageDetector,
        evaluator: &RecognitionEvaluator,
    ) -> Result<(), ContainedTaskError> {
        let Some(store_update) = &control.store_update else {
            return Ok(());
        };
        let game = control.game.as_str();
        for (index, declared) in store_update.store_operations.iter().enumerate() {
            let pointer = format!("/store_update/store_operations/{index}");
            let refuse = |problem: ProgramProblem, member: &str| {
                program_refusal(problem, CONTROL_PATH, &format!("{pointer}/{member}"))
            };
            let operation = self
                .operations
                .iter()
                .find(|operation| operation.id == declared.operation_id)
                .filter(|operation| {
                    operation.application.is_none()
                        && operation.select.is_none()
                        && operation.wait.is_none()
                        && operation
                            .click
                            .as_ref()
                            .is_some_and(|click| TAP_KINDS.contains(&click.kind.as_str()))
                })
                .ok_or_else(|| refuse(ProgramProblem::StoreOperationInvalid, "operation"))?;
            let resolved = if operation.from == linear::ANY_PAGE {
                None
            } else {
                resolve_page_reference(game, &operation.from, detector).ok()
            };
            let from = resolved
                .ok_or_else(|| refuse(ProgramProblem::StoreOperationInvalid, "operation"))?;
            let required = detector
                .page_definitions()
                .iter()
                .find(|page| page.id == from)
                .map(|page| page.required.as_slice())
                .unwrap_or_default();
            for (target_index, target_id) in declared.proof.iter().enumerate() {
                let target_id = target_id.as_str();
                let text = evaluator.pack().targets.iter().any(|target| {
                    recognition_target_id(target) == target_id
                        && matches!(target, RecognitionTarget::Ocr(ocr) if !ocr.expected.is_empty())
                });
                if !text || !required.iter().any(|entry| entry == target_id) {
                    return Err(refuse(
                        ProgramProblem::StoreProofInvalid,
                        &format!("proof/{target_index}"),
                    ));
                }
            }
        }
        let way_back = self.operations.iter().any(|operation| {
            operation.application.as_ref().is_some_and(|effect| {
                matches!(
                    effect.action,
                    ApplicationLifecycleAction::Launch | ApplicationLifecycleAction::Restart
                )
            })
        });
        if !way_back {
            return Err(program_refusal(
                ProgramProblem::StoreWayBackMissing,
                CONTROL_PATH,
                "/store_update",
            ));
        }
        Ok(())
    }
}

impl PreparedContainedTask {
    /// Design §8.1 (the resource side's field note: the store's button moves between devices):
    /// the ids of the declared store operations whose click taps a fixed place (`point`, `rect`,
    /// `specific_rect`) rather than a target located on the frame, in declaration order. The
    /// standard-pack check warns on each; admission still takes them.
    pub(crate) fn store_fixed_taps(&self) -> Vec<String> {
        let Some(store_update) = &self.control.store_update else {
            return Vec::new();
        };
        store_update
            .store_operations
            .iter()
            .filter(|declared| {
                self.program.operations.iter().any(|operation| {
                    operation.id == declared.operation_id
                        && operation
                            .click
                            .as_ref()
                            .is_some_and(|click| FIXED_TAP_KINDS.contains(&click.kind.as_str()))
                })
            })
            .map(|declared| declared.operation_id.clone())
            .collect()
    }
}
