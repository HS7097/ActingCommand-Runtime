// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #411 A22: a direct run refuses a recovery package that no path of it can use
//! (`contracts/linear-steps.md`, "Prerequisite packages").
//!
//! A request's recovery package acts in two places only: (a) the home entry of a package whose
//! entry is checked and that has no prerequisite chain (`run_preflighted_contained_task`), and
//! (b) the stuck-recovery ladder's `return_home` rung, which a failed direct run starts only under
//! `recovery_ladder.rules.direct_runs = trigger`, with the instance's stuck recovery on and on a
//! physical instance (`ladder_at_lease_end`). A direct run (CLI or console task-run, MCP
//! `ac_run_pack`) that binds one neither can use is denied in its preparation with
//! `contained_task_recovery_binding_unused`: it writes no `task.*` record, takes no lease, sends
//! no input and starts no ladder.

use super::recovery_ladder::{LadderFacts, direct_run_exempt, ladder_invariants};
use super::*;
use crate::codes::{HostCode, HostLocation};

/// Why no path of a direct run can use its recovery binding: the `reason` of the refusal's native
/// detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnusedRecoveryBinding {
    /// The run enters through its prerequisite chain, whose gate ignores the binding.
    PrerequisiteGate,
    /// The run's entry is not checked, so no home entry runs the binding.
    EntryNotChecked,
}

impl UnusedRecoveryBinding {
    /// The `reason=` token of the native detail.
    const fn token(self) -> &'static str {
        match self {
            Self::PrerequisiteGate => "prerequisite_gate",
            Self::EntryNotChecked => "entry_not_checked",
        }
    }
}

/// Whether a run's recovery binding has a use, and why not when it has none.
///
/// (a) The home entry uses it when the run has no prerequisite chain (`prerequisite_chain` is
/// false) and its entry is checked (`entry_checked`: the package's required entry pages, and not a
/// zero-step package with post-admission reading). (b) The ladder uses it when a failed direct
/// run on the instance may start one (`ladder`). The binding is unused only when neither holds;
/// the reason is the prerequisite gate when the chain is not empty, else the unchecked entry.
pub(crate) const fn recovery_binding_use(
    prerequisite_chain: bool,
    entry_checked: bool,
    ladder: bool,
) -> Option<UnusedRecoveryBinding> {
    let entry = !prerequisite_chain && entry_checked;
    if entry || ladder {
        None
    } else if prerequisite_chain {
        Some(UnusedRecoveryBinding::PrerequisiteGate)
    } else {
        Some(UnusedRecoveryBinding::EntryNotChecked)
    }
}

/// The refusal of an unused binding: `contained_task_recovery_binding_unused` at
/// `check_recovery_binding`, `invalid_request`, with `package_id` the run's package and the
/// native detail `reason=<why>`.
pub(crate) fn recovery_binding_unused_error(
    package_id: &str,
    unused: UnusedRecoveryBinding,
) -> RuntimeHostError {
    RuntimeHostError::request(
        HostCode::ContainedTaskRecoveryBindingUnused,
        HostLocation::CheckRecoveryBinding,
        RuntimeErrorCode::InvalidRequest,
    )
    .with_outcome_values(|outcome| outcome.package_id(package_id))
    .with_native_detail(format!("reason={}", unused.token()))
}

impl HostShared {
    /// A direct run's check of its recovery binding, after its prerequisite chain is resolved and
    /// before any record, claim or lease: a binding that neither (a) nor (b) can use is denied.
    /// A request without a binding passes unchanged.
    pub(super) fn check_recovery_binding(
        &self,
        resolved: &RegisteredInstance,
        task_request: &ContainedTaskRequest,
        prepared: &PreparedContainedTask,
        prerequisites: &[PreparedContainedTask],
    ) -> Result<(), RequestFailure> {
        if task_request.recovery().is_none() {
            return Ok(());
        }
        let entry_checked = prepared.required_entry_pages().is_some()
            && !(prepared.has_post_admission_ocr() && prepared.maximum_executed_steps() == 0);
        let ladder = self.direct_run_ladder_possible(resolved)?;
        match recovery_binding_use(!prerequisites.is_empty(), entry_checked, ladder) {
            None => Ok(()),
            Some(unused) => Err(RequestFailure::request(
                recovery_binding_unused_error(prepared.package_label(), unused),
                RuntimeReceiptState::Denied,
                None,
            )),
        }
    }

    /// (b): whether a failed direct run on `resolved` may start a stuck-recovery ladder under the
    /// configuration in effect now, as `ladder_at_lease_end` reads it: `rules.direct_runs` is not
    /// `exempt`, the instance's stuck recovery is on, and the instance is physical.
    fn direct_run_ladder_possible(&self, resolved: &RegisteredInstance) -> RuntimeHostResult<bool> {
        let config = self.ladder_config()?;
        if direct_run_exempt(&config.rules, true) {
            return Ok(false);
        }
        let settings = self.instance_stuck_recovery(resolved.instance_id(), &config)?;
        let physical = resolved.provenance() == ExecutionBackendProvenance::PhysicalDevice;
        Ok(settings.enabled
            && ladder_invariants(&LadderFacts {
                physical,
                ..LadderFacts::PASSING
            })
            .is_none())
    }
}
