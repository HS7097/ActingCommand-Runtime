// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #453 S12 (`contracts/state-root-installation.md`, "Handing a state root over"):
//! offline `actingd claim-state-root`, the explicit act of a person that hands the configuration's
//! state root over to the installation the command runs from. Besides a start it is the one
//! program that mints an identity and claims a state root.
//!
//! Its order: no installation root, no claim (`state_root_claim_failed`); the identity's refusals,
//! read before anything is written (`state_root_installation::check_hand_over`); the state root's
//! folder made when missing, as a start makes it; the owner taken with the start's acquire, which a
//! running Runtime refuses with the existing `owner_conflict` and which records the command's owner
//! epoch in the owner journal; the hand-over under that lock
//! (`state_root_installation::hand_over`); the epoch closed, also after a failure. It never opens
//! the ledger: the fact of the new claim is the next start's to record.

use crate::codes::{HostCode, HostLocation};
use crate::owner::{OwnerGuard, OwnerStartup};
use crate::state_root_installation::{self, HandOverRequest, StateRootHandOver};
use crate::{RuntimeClock, RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{IdentifierIssuer, RuntimeErrorCode};
use std::fs;
use std::path::Path;

/// What `actingd claim-state-root` works on: the configuration's state root, the installation root
/// the command runs from (none when the configuration's folder holds no installer state file), the
/// clock, the commit the build was made from and whether `--new-identity` was given.
pub(crate) struct ClaimStateRoot<'a> {
    pub(crate) state_root: &'a Path,
    pub(crate) installation_root: Option<&'a Path>,
    pub(crate) clock: &'a dyn RuntimeClock,
    pub(crate) build_commit: Option<&'a str>,
    pub(crate) new_identity: bool,
    /// Test only: the step after which the hand-over stops.
    #[cfg(test)]
    pub(crate) crash_after: Option<state_root_installation::HandOverStep>,
    /// Test only: every hard link fails with "unsupported".
    #[cfg(test)]
    pub(crate) hard_link_fails: bool,
}

/// Runs `actingd claim-state-root` as the module documentation orders it.
pub(crate) fn run(request: &ClaimStateRoot<'_>) -> RuntimeHostResult<StateRootHandOver> {
    let Some(installation_root) = request.installation_root else {
        return Err(RuntimeHostError::fatal(
            HostCode::StateRootClaimFailed,
            HostLocation::RecordStateRootInstallation,
            RuntimeErrorCode::InvalidRequest,
        )
        .with_outcome_values(|outcome| outcome.path(request.state_root))
        .with_native_detail(
            "the configuration's folder holds no installer state file install\\state.json: the \
             command runs from no installation, and only an installation claims a state root"
                .to_owned(),
        ));
    };
    state_root_installation::check_hand_over(installation_root, request.new_identity)?;
    fs::create_dir_all(request.state_root).map_err(|_| {
        RuntimeHostError::fatal(
            SharedCode::StateRootCreateFailed,
            HostLocation::RecordStateRootInstallation,
            RuntimeErrorCode::RuntimeHalted,
        )
    })?;
    let at_unix_ms = request.clock.sample()?.unix_ms;
    let issuer = IdentifierIssuer::new().map_err(|_| {
        RuntimeHostError::fatal(
            HostCode::OwnerEpochIssueFailed,
            HostLocation::AcquireOwnerFile,
            RuntimeErrorCode::RuntimeHalted,
        )
    })?;
    let OwnerStartup {
        guard: mut owner,
        owner_epoch,
        ..
    } = OwnerGuard::acquire(request.state_root, &issuer, at_unix_ms)?;
    let handed = state_root_installation::hand_over(&HandOverRequest {
        installation_root,
        state_root: request.state_root,
        owner_epoch,
        at_unix_ms,
        build_commit: request.build_commit,
        new_identity: request.new_identity,
        #[cfg(test)]
        crash_after: request.crash_after,
        #[cfg(test)]
        hard_link_fails: request.hard_link_fails,
    });
    // The command's owner epoch is closed whatever the hand-over answered, as a failed start's is.
    let closed = request
        .clock
        .sample()
        .and_then(|sample| owner.close(sample.unix_ms));
    match (handed, closed) {
        (Ok(handed), Ok(())) => Ok(handed),
        (Ok(_), Err(closed)) => Err(closed),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(closed)) => Err(error.with_note(closed.daemon_outcome())),
    }
}
