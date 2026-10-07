// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #361 C3: the startup catalog plan, previewed for `actingd check-config` without
//! touching the state root. The ledger is read through the lock-free evidence reader (SQLite
//! read-only, no owner lock, no referenced material), the State documents through a read-only
//! view of the same database and the catalog generations from their immutable directories,
//! so the preview runs beside a running daemon and creates, locks and writes nothing. The
//! decision and the approval plan are the functions startup uses.

use super::catalog_transaction::catalog_projection_events;
use super::*;
use crate::approval::ApprovalProjection;
use crate::catalog_plan::{
    CatalogApprovalPlan, CatalogLineage, CatalogTransitionPlanKind, CatalogTransitionRequest,
    decide_catalog_transition, plan_catalog_approvals,
};
use actingcommand_ledger::GlobalLedgerEvidenceConfig;
use std::time::Duration;

const OPERATION: &str = "preview_policy_catalog";
/// How long the preview may spend reading the ledger (a check is not allowed to stall a
/// configuration commit indefinitely).
const PREVIEW_LEDGER_DEADLINE: Duration = Duration::from_secs(120);
/// The materials whose absence makes a state root fresh, as at startup.
const STATE_MATERIALS: [&str; 5] = [
    "runtime-state.sqlite",
    "runtime-state.key",
    "ledger",
    "release-blobs",
    "artifacts",
];

/// What the preview reads: the configured state root and policy.
pub struct CatalogPreviewRequest<'a> {
    pub state_root: &'a Path,
    /// The configured catalog; `None` when the configuration has no `policy` (driver off).
    pub catalog: Option<&'a CatalogSources>,
    pub transition: Option<&'a CatalogTransitionRequest>,
    pub approval_ids: &'a [String],
}

/// The startup plan as the preview found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogPreview {
    state_root_present: bool,
    active: Option<CatalogGeneration>,
    configured: Option<CatalogGeneration>,
    plan: Option<CatalogTransitionPlanKind>,
    approvals: Option<CatalogApprovalPlan>,
}

impl CatalogPreview {
    /// Whether the state root exists. An existing root without any state material reads as
    /// a fresh one: nothing is active and nothing was decided.
    pub fn state_root_present(&self) -> bool {
        self.state_root_present
    }

    /// The generation the ledger projects as active.
    pub fn active(&self) -> Option<&CatalogGeneration> {
        self.active.as_ref()
    }

    /// The configured generation, compiled in memory; `None` without a `policy` section.
    pub fn configured(&self) -> Option<&CatalogGeneration> {
        self.configured.as_ref()
    }

    /// The plan startup would make; `None` without a `policy` section.
    pub fn plan(&self) -> Option<CatalogTransitionPlanKind> {
        self.plan
    }

    /// The approvals startup would record; `None` without a `policy` section.
    pub fn approvals(&self) -> Option<&CatalogApprovalPlan> {
        self.approvals.as_ref()
    }
}

/// The ledger-derived inputs of the plan.
struct StateSnapshot {
    active: Option<CatalogGeneration>,
    lineage: CatalogLineage,
    approvals: ApprovalProjection,
}

/// `actingd check-config`'s preview of startup's catalog plan (Workflow #361 C3). A refusal
/// is the error startup would fail with: the plan's code (`catalog_activation_not_newer`,
/// `catalog_transition_expectation_mismatch`, `catalog_replace_version_not_newer`,
/// `policy_catalog_approval_conflict`, ...) or the ledger's own.
pub fn preview_policy_catalog_transition(
    preview: &CatalogPreviewRequest<'_>,
) -> RuntimeHostResult<CatalogPreview> {
    let configured = preview
        .catalog
        .map(|sources| {
            compile_catalog(sources)
                .map(|compiled| generation_from(&compiled, sources))
                .map_err(|_| request("catalog_compile_failed", OPERATION))
        })
        .transpose()?;
    let state_root_present = preview
        .state_root
        .try_exists()
        .map_err(|_| fatal("state_root_inspect_failed", OPERATION))?;
    let snapshot = if state_root_present {
        read_state(preview.state_root)?
    } else {
        None
    };
    let empty = CatalogLineage::default();
    let (active, lineage, approvals) = match &snapshot {
        Some(snapshot) => (
            snapshot.active.as_ref(),
            &snapshot.lineage,
            Some(&snapshot.approvals),
        ),
        None => (None, &empty, None),
    };
    let (plan, planned_approvals) = match &configured {
        None => (None, None),
        Some(configured) => {
            let kind = decide_catalog_transition(active, lineage, configured, preview.transition)?;
            let planned =
                plan_catalog_approvals(approvals, lineage, configured, preview.approval_ids)?;
            (Some(kind), Some(planned))
        }
    };
    Ok(CatalogPreview {
        state_root_present,
        active: active.cloned(),
        configured,
        plan,
        approvals: planned_approvals,
    })
}

/// Reads the active generation, the lineage and the approval projection of an existing state
/// root; `None` for a root without any state material.
fn read_state(state_root: &Path) -> RuntimeHostResult<Option<StateSnapshot>> {
    let mut fresh = true;
    for material in STATE_MATERIALS {
        if state_root
            .join(material)
            .try_exists()
            .map_err(|_| fatal("state_root_inspect_failed", OPERATION))?
        {
            fresh = false;
        }
    }
    if fresh {
        return Ok(None);
    }
    let ledger = GlobalLedger::open_evidence(
        GlobalLedgerEvidenceConfig::new(state_root)
            .sqlite_material_not_read()
            .with_deadline(Instant::now() + PREVIEW_LEDGER_DEADLINE),
        |_| None,
    )
    .map_err(|error| catalog_ledger_error(&error))?;
    if !ledger.is_complete() {
        return Err(fatal("policy_state_ledger_incomplete", OPERATION));
    }
    let database = actingcommand_runtime_database::RuntimeDatabase::open_existing(state_root, true)
        .map_err(|error| {
            RuntimeHostError::fatal(
                error.code(),
                error.operation(),
                RuntimeErrorCode::LedgerFailure,
            )
            .with_native_detail(format!("{error:?}"))
        })?;
    let state = Arc::new(
        RuntimeStateStore::from_database(Arc::new(database))
            .map_err(|error| RuntimeHostError::state(&error))?,
    );
    let store = CatalogStore::open_read_only(state_root, Arc::clone(&state));
    // Startup migrates a legacy active-pointer file before it projects; a preview cannot.
    if store
        .legacy_active_pointer
        .try_exists()
        .map_err(|_| fatal("state_root_inspect_failed", OPERATION))?
    {
        return Err(fatal("catalog_legacy_pointer_pending", OPERATION));
    }
    let events =
        catalog_projection_events(ledger.latest_sequence(), |query| Ok(ledger.query(&query)))?;
    let (current, lineage) = store.fold_catalog_events(&events)?;
    let projected = match current {
        None => None,
        Some((id, version, hash)) => {
            let loaded = store.load_generation(&hash)?;
            if loaded.generation.catalog_id() != id
                || loaded.generation.catalog_version() != version
            {
                return Err(fatal("catalog_projection_identity_mismatch", OPERATION));
            }
            Some(loaded.generation.clone())
        }
    };
    let pointer = store.load_active()?.map(|catalog| catalog.generation);
    if pointer != projected {
        return Err(fatal("catalog_active_source_mismatch", OPERATION));
    }
    let mut approval_events = ledger.query(&EventQuery {
        event_type: Some(EventType::ApprovalDecision),
        ..EventQuery::default()
    });
    approval_events.sort_by_key(PersistedEvent::sequence);
    let approvals = ApprovalProjection::from_events(&approval_events, state)?;
    Ok(Some(StateSnapshot {
        active: pointer,
        lineage,
        approvals,
    }))
}
