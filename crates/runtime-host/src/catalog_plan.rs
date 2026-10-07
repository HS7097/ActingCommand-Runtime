// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #361 C2 and A: the catalog transition a configured catalog asks for, and the
//! approvals the policy driver records for it, decided before anything is recorded. Planning
//! stages the immutable generation directory, as activation does, and writes no ledger
//! record; applying the plan is the first catalog write.

use crate::approval::ApprovalProjection;
use crate::{CatalogGeneration, RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    ApprovalDecisionRecord, ApprovalDisposition, ApprovalTarget, RuntimeErrorCode,
};
use std::collections::BTreeMap;

/// The reason of the approvals the policy driver records from its configuration.
pub(crate) const CONFIGURED_CATALOG_APPROVAL_REASON: &str = "configured_catalog_approval";
/// The reason of the approvals a transition supersedes.
pub(crate) const CATALOG_SUPERSEDED_REASON: &str = "catalog_superseded";

/// What applying a plan records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogTransitionPlanKind {
    /// The configured catalog is already active: nothing is recorded.
    Unchanged,
    /// No catalog is active yet: `catalog.activated` without a previous generation.
    First,
    /// The active catalog id at a higher version: `catalog.activated`.
    Forward,
    /// Another catalog id that was never active: `catalog.activated` (the id changes).
    Switch,
    /// A generation that was active before: `catalog.rolled_back`.
    Rollback,
}

impl CatalogTransitionPlanKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::First => "first",
            Self::Forward => "forward",
            Self::Switch => "switch",
            Self::Rollback => "rollback",
        }
    }
}

/// The explicit transition a configuration asks for (`policy.catalog_transition`). Its only
/// kind is `replace`: switch to another catalog id or roll back to any generation that was
/// active before, provided `expected_active_catalog_hash` is still the active generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogTransitionRequest {
    expected_active_catalog_hash: String,
}

impl CatalogTransitionRequest {
    pub fn replace(expected_active_catalog_hash: impl Into<String>) -> Self {
        Self {
            expected_active_catalog_hash: expected_active_catalog_hash.into(),
        }
    }

    pub fn expected_active_catalog_hash(&self) -> &str {
        &self.expected_active_catalog_hash
    }
}

/// Every generation that was active in one ledger: the target of a successful
/// `catalog.activated` or `catalog.rolled_back`, or of the legacy active-pointer migration,
/// keyed by catalog hash. Rebuilt from the ledger on every open; never persisted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CatalogLineage {
    previously_active: BTreeMap<String, (String, u64)>,
}

impl CatalogLineage {
    pub(crate) fn record(&mut self, catalog_id: &str, catalog_version: u64, catalog_hash: &str) {
        self.previously_active.insert(
            catalog_hash.to_owned(),
            (catalog_id.to_owned(), catalog_version),
        );
    }

    pub(crate) fn contains(&self, catalog_hash: &str) -> bool {
        self.previously_active.contains_key(catalog_hash)
    }

    /// The catalog id of a generation that was active, by hash.
    pub(crate) fn catalog_id(&self, catalog_hash: &str) -> Option<&str> {
        self.previously_active
            .get(catalog_hash)
            .map(|(catalog_id, _)| catalog_id.as_str())
    }

    /// The highest version of `catalog_id` that was ever active.
    fn highest_version(&self, catalog_id: &str) -> Option<u64> {
        self.previously_active
            .values()
            .filter(|(id, _)| id == catalog_id)
            .map(|(_, version)| *version)
            .max()
    }
}

/// The approvals the policy driver records after the transition, in this order: the
/// revocations of superseded catalog approvals, then the undecided configured approvals and
/// the configured approvals of a restored generation that a transition had superseded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogApprovalPlan {
    pub(crate) revoke: Vec<ApprovalDecisionRecord>,
    pub(crate) record: Vec<ApprovalDecisionRecord>,
    pub(crate) reapprove: Vec<ApprovalDecisionRecord>,
}

impl CatalogApprovalPlan {
    /// Active catalog approvals the planned generation supersedes, as `revoked` records.
    pub fn revoke(&self) -> &[ApprovalDecisionRecord] {
        &self.revoke
    }

    /// Configured approvals with no decision yet.
    pub fn record(&self) -> &[ApprovalDecisionRecord] {
        &self.record
    }

    /// Configured approvals of a generation that was active before and whose latest decision
    /// is the `catalog_superseded` revocation of that same target.
    pub fn reapprove(&self) -> &[ApprovalDecisionRecord] {
        &self.reapprove
    }
}

/// A decided catalog transition: the configured generation (already staged), the active one
/// the decision was made against, what applying it records, and the approvals the driver
/// records afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogTransitionPlan {
    pub(crate) kind: CatalogTransitionPlanKind,
    pub(crate) generation: CatalogGeneration,
    pub(crate) active: Option<CatalogGeneration>,
    pub(crate) approvals: CatalogApprovalPlan,
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

    pub fn approvals(&self) -> &CatalogApprovalPlan {
        &self.approvals
    }
}

fn refused(code: &'static str) -> RuntimeHostError {
    RuntimeHostError::request(
        code,
        "plan_policy_catalog_transition",
        RuntimeErrorCode::InvalidRequest,
    )
}

/// The startup decision table (Workflow #361 A, model §2.A.2), rows in order:
///
/// | Active | Configured | `replace` | Plan |
/// |---|---|---|---|
/// | none | any | absent | `first` |
/// | none | any | present | `catalog_transition_expectation_mismatch` |
/// | A | the hash of A | any | `unchanged` |
/// | A | the id of A, higher version | absent, or expecting A | `forward` |
/// | A | anything else | absent | `catalog_activation_not_newer` |
/// | A | any | expecting another hash | `catalog_transition_expectation_mismatch` |
/// | A | a generation that was active | expecting A | `rollback` |
/// | A | another id, never active | expecting A | `switch`, if its version is above every active version of that id |
/// | A | otherwise | expecting A | `catalog_replace_version_not_newer` |
pub(crate) fn decide_catalog_transition(
    active: Option<&CatalogGeneration>,
    lineage: &CatalogLineage,
    configured: &CatalogGeneration,
    request: Option<&CatalogTransitionRequest>,
) -> RuntimeHostResult<CatalogTransitionPlanKind> {
    let Some(active) = active else {
        return match request {
            None => Ok(CatalogTransitionPlanKind::First),
            Some(_) => Err(refused("catalog_transition_expectation_mismatch")),
        };
    };
    if active.catalog_hash() == configured.catalog_hash() {
        return Ok(CatalogTransitionPlanKind::Unchanged);
    }
    let expects_active =
        request.map(|request| request.expected_active_catalog_hash() == active.catalog_hash());
    if active.catalog_id() == configured.catalog_id()
        && configured.catalog_version() > active.catalog_version()
        && expects_active != Some(false)
    {
        return Ok(CatalogTransitionPlanKind::Forward);
    }
    match expects_active {
        // Review P6: the refusal names the explicit transition that would apply instead.
        None => Err(refused("catalog_activation_not_newer").with_native_detail(format!(
            "active catalog {} v{} ({}); configured catalog {} v{} ({}) is not a higher version \
             of the active catalog id; to switch the catalog id or roll back to a generation \
             that was active, set policy.catalog_transition {{\"kind\":\"replace\",\
             \"expected_active_catalog_hash\":\"{}\"}}",
            active.catalog_id(),
            active.catalog_version(),
            active.catalog_hash(),
            configured.catalog_id(),
            configured.catalog_version(),
            configured.catalog_hash(),
            active.catalog_hash(),
        ))),
        Some(false) => Err(refused("catalog_transition_expectation_mismatch")),
        Some(true) if lineage.contains(configured.catalog_hash()) => {
            Ok(CatalogTransitionPlanKind::Rollback)
        }
        Some(true)
            if configured.catalog_id() != active.catalog_id()
                && lineage
                    .highest_version(configured.catalog_id())
                    .is_none_or(|highest| configured.catalog_version() > highest) =>
        {
            Ok(CatalogTransitionPlanKind::Switch)
        }
        Some(true) => Err(refused("catalog_replace_version_not_newer")),
    }
}

/// Whether an active catalog approval of (`catalog_hash`, `catalog_version`) is superseded by
/// `generation`: an approval of another catalog id that was active is superseded whatever its
/// version; otherwise an older version, or the same version under another hash. A later
/// version of the same catalog (or of a catalog never active here) is kept: it may be approved
/// ahead of its activation (`contracts/client-interactions.md`).
pub(crate) fn catalog_approval_superseded(
    lineage: &CatalogLineage,
    generation: &CatalogGeneration,
    catalog_hash: &str,
    catalog_version: u64,
) -> bool {
    if catalog_hash == generation.catalog_hash() {
        return false;
    }
    if lineage
        .catalog_id(catalog_hash)
        .is_some_and(|catalog_id| catalog_id != generation.catalog_id())
    {
        return true;
    }
    catalog_version <= generation.catalog_version()
}

/// The approvals the policy driver records for `generation` (Workflow #330 H2, #361 A):
///
/// - a configured id with no decision is recorded;
/// - one whose latest decision equals the configured approval is left as it is;
/// - one whose latest decision is the `catalog_superseded` revocation of the same target is
///   approved again when `generation` was active before (a rollback, or a forward or replace
///   step back to a generation a rollback had left);
/// - any other latest decision (a person's rejection or revocation, another reason, another
///   target) fails with `policy_catalog_approval_conflict`;
/// - every active catalog approval `generation` supersedes is revoked with reason
///   `catalog_superseded`.
///
/// `approvals` is `None` for a state root without a ledger: nothing was ever decided.
pub(crate) fn plan_catalog_approvals(
    approvals: Option<&ApprovalProjection>,
    lineage: &CatalogLineage,
    generation: &CatalogGeneration,
    approval_ids: &[String],
) -> RuntimeHostResult<CatalogApprovalPlan> {
    let target = ApprovalTarget::Catalog {
        catalog_hash: generation.catalog_hash().to_owned(),
        catalog_version: generation.catalog_version(),
    };
    let restorable = lineage.contains(generation.catalog_hash());
    let mut plan = CatalogApprovalPlan::default();
    for approval_id in approval_ids {
        let decision = ApprovalDecisionRecord::new(
            approval_id.clone(),
            ApprovalDisposition::Approved,
            target.clone(),
            CONFIGURED_CATALOG_APPROVAL_REASON,
        )
        .map_err(|_| refused("policy_catalog_approval_invalid"))?;
        let latest = match approvals {
            Some(approvals) => approvals.latest_decision(approval_id)?,
            None => None,
        };
        match latest {
            None => plan.record.push(decision),
            Some(existing) if existing == decision => {}
            Some(existing)
                if restorable
                    && existing.disposition() == ApprovalDisposition::Revoked
                    && existing.reason_code() == CATALOG_SUPERSEDED_REASON
                    && existing.target() == decision.target() =>
            {
                plan.reapprove.push(decision);
            }
            // A persisted rejection or revocation cannot be silently replaced by startup config.
            Some(_) => return Err(refused("policy_catalog_approval_conflict")),
        }
    }
    let superseded = approvals
        .map(|approvals| approvals.superseded_catalog_approvals(lineage, generation))
        .unwrap_or_default();
    for superseded in superseded {
        plan.revoke.push(
            ApprovalDecisionRecord::new(
                superseded.approval_id(),
                ApprovalDisposition::Revoked,
                superseded.target().clone(),
                CATALOG_SUPERSEDED_REASON,
            )
            .map_err(|_| refused("policy_catalog_revocation_invalid"))?,
        );
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generation(catalog_id: &str, catalog_version: u64, digit: char) -> CatalogGeneration {
        CatalogGeneration::for_plan_test(
            catalog_id,
            catalog_version,
            &format!("sha256:{}", digit.to_string().repeat(64)),
        )
    }

    /// Workflow #361 A: every row of the decision table.
    #[test]
    fn catalog_transition_decision_table() {
        use CatalogTransitionPlanKind::{First, Forward, Rollback, Switch, Unchanged};
        let current = generation("fixture.lineage-a", 12, 'c');
        let older = generation("fixture.lineage-a", 11, 'b');
        let newer = generation("fixture.lineage-a", 13, 'd');
        let other = generation("fixture.lineage-b", 1, 'e');
        let regressed = generation("fixture.lineage-b", 1, 'f');
        let mut lineage = CatalogLineage::default();
        for value in [&older, &current] {
            lineage.record(
                value.catalog_id(),
                value.catalog_version(),
                value.catalog_hash(),
            );
        }
        let expecting = |hash: &str| CatalogTransitionRequest::replace(hash);
        let decide = |active: Option<&CatalogGeneration>,
                      configured: &CatalogGeneration,
                      request: Option<&CatalogTransitionRequest>| {
            decide_catalog_transition(active, &lineage, configured, request)
                .map_err(|error| error.code())
        };
        let current_hash = expecting(current.catalog_hash());
        let wrong_hash = expecting(other.catalog_hash());
        assert_eq!(decide(None, &current, None), Ok(First));
        assert_eq!(
            decide(None, &current, Some(&current_hash)),
            Err("catalog_transition_expectation_mismatch")
        );
        assert_eq!(decide(Some(&current), &current, None), Ok(Unchanged));
        assert_eq!(
            decide(Some(&current), &current, Some(&wrong_hash)),
            Ok(Unchanged)
        );
        assert_eq!(decide(Some(&current), &newer, None), Ok(Forward));
        assert_eq!(
            decide(Some(&current), &newer, Some(&current_hash)),
            Ok(Forward)
        );
        assert_eq!(
            decide(Some(&current), &older, None),
            Err("catalog_activation_not_newer")
        );
        assert_eq!(
            decide(Some(&current), &other, None),
            Err("catalog_activation_not_newer")
        );
        assert_eq!(
            decide(Some(&current), &older, Some(&current_hash)),
            Ok(Rollback)
        );
        assert_eq!(
            decide(Some(&current), &other, Some(&current_hash)),
            Ok(Switch)
        );
        assert_eq!(
            decide(
                Some(&current),
                &generation("fixture.lineage-a", 10, 'a'),
                Some(&current_hash)
            ),
            Err("catalog_replace_version_not_newer")
        );
        assert_eq!(
            decide(Some(&current), &newer, Some(&wrong_hash)),
            Err("catalog_transition_expectation_mismatch")
        );
        assert_eq!(
            decide(Some(&current), &older, Some(&wrong_hash)),
            Err("catalog_transition_expectation_mismatch")
        );
        // Review L9: once lineage b was active at version 1, a never-active b generation at
        // the same version is no switch target, even from another lineage.
        lineage.record(
            other.catalog_id(),
            other.catalog_version(),
            other.catalog_hash(),
        );
        let from_other = expecting(current.catalog_hash());
        assert_eq!(
            decide_catalog_transition(Some(&current), &lineage, &regressed, Some(&from_other))
                .map_err(|error| error.code()),
            Err("catalog_replace_version_not_newer")
        );
        // A forward step back to a generation that was active stays `forward`.
        lineage.record(
            newer.catalog_id(),
            newer.catalog_version(),
            newer.catalog_hash(),
        );
        assert_eq!(
            decide_catalog_transition(Some(&current), &lineage, &newer, None)
                .map_err(|error| error.code()),
            Ok(Forward)
        );
    }

    /// Workflow #361 A: cross-lineage approvals are superseded whatever their version; a later
    /// version of the same lineage is kept.
    #[test]
    fn catalog_approval_supersession_follows_the_lineage() {
        let current = generation("fixture.lineage-a", 12, 'c');
        let older = generation("fixture.lineage-a", 11, 'b');
        let newer = generation("fixture.lineage-a", 13, 'd');
        let other = generation("fixture.lineage-b", 40, 'e');
        let mut lineage = CatalogLineage::default();
        for value in [&older, &current, &newer, &other] {
            lineage.record(
                value.catalog_id(),
                value.catalog_version(),
                value.catalog_hash(),
            );
        }
        let superseded = |value: &CatalogGeneration| {
            catalog_approval_superseded(
                &lineage,
                &current,
                value.catalog_hash(),
                value.catalog_version(),
            )
        };
        assert!(!superseded(&current));
        assert!(superseded(&older));
        assert!(!superseded(&newer));
        assert!(superseded(&other));
        // A never-active hash keeps the version rule.
        let unknown = generation("fixture.lineage-c", 13, 'f');
        assert!(!superseded(&unknown));
        let unknown_older = generation("fixture.lineage-c", 2, 'a');
        assert!(superseded(&unknown_older));
    }
}
