// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #361 C2: the catalog transition a configured catalog asks for, decided before
//! anything is recorded. Planning stages the immutable generation directory, as activation
//! does, and writes no ledger record; applying the plan is the first catalog write.

use crate::{CatalogGeneration, RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::RuntimeErrorCode;

/// What applying a plan records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogTransitionPlanKind {
    /// The configured catalog is already active: nothing is recorded.
    Unchanged,
    /// No catalog is active yet: `catalog.activated` without a previous generation.
    First,
    /// The active catalog id at a higher version: `catalog.activated`.
    Forward,
}

impl CatalogTransitionPlanKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::First => "first",
            Self::Forward => "forward",
        }
    }
}

/// A decided catalog transition: the configured generation (already staged), the active one
/// the decision was made against, and what applying it records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogTransitionPlan {
    pub(crate) kind: CatalogTransitionPlanKind,
    pub(crate) generation: CatalogGeneration,
    pub(crate) active: Option<CatalogGeneration>,
}

impl CatalogTransitionPlan {
    pub fn kind(&self) -> CatalogTransitionPlanKind {
        self.kind
    }

    /// The configured generation the plan activates or keeps.
    pub fn generation(&self) -> &CatalogGeneration {
        &self.generation
    }

    /// The generation that was active when the plan was made; applying the plan requires it
    /// to still be active.
    pub fn active(&self) -> Option<&CatalogGeneration> {
        self.active.as_ref()
    }
}

/// The existing activation rule as one decision: the same hash is unchanged, nothing active is
/// a first activation, the active id at a higher version is a forward step, and anything else
/// is refused with `catalog_activation_not_newer`.
pub(crate) fn decide_catalog_transition(
    active: Option<&CatalogGeneration>,
    configured: &CatalogGeneration,
) -> RuntimeHostResult<CatalogTransitionPlanKind> {
    let Some(active) = active else {
        return Ok(CatalogTransitionPlanKind::First);
    };
    if active.catalog_hash() == configured.catalog_hash() {
        return Ok(CatalogTransitionPlanKind::Unchanged);
    }
    if active.catalog_id() == configured.catalog_id()
        && configured.catalog_version() > active.catalog_version()
    {
        return Ok(CatalogTransitionPlanKind::Forward);
    }
    Err(RuntimeHostError::request(
        "catalog_activation_not_newer",
        "plan_policy_catalog_transition",
        RuntimeErrorCode::InvalidRequest,
    ))
}
