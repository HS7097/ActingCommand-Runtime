// SPDX-License-Identifier: AGPL-3.0-only

//! Test plan v2.2 L-2, I6 (inventory-ledger): the shared formal root without runtime-host. Its
//! State tables, its formal SQLite ledger with the writer and its ArtifactStore share one
//! tempdir, opened the way acsetup leaves a root (the assembly of `missing_material.rs`). The
//! only deadline is the one `MaintenanceLimits` computes; nothing here reads a clock.

use crate::global::tests::sealed_global_ledger::GlobalLedgerSink;
use crate::global::{GlobalLedger, LedgerMaintenance};
use actingcommand_artifact_store::ArtifactStore;
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use actingcommand_runtime_state::RuntimeStateStore;
use std::sync::Arc;
use tempfile::TempDir;

/// The State integrity key and the writer's owner id: neutral fixture words.
const STATE_KEY: &[u8] = b"ledger-gate-fixture";
const OWNER_ID: &str = "ledger-gate-fixture";

/// One formal root. Its fields drop in order: the writer, the ArtifactStore, the State
/// database, then the directory.
pub(super) struct FormalRoot {
    ledger: GlobalLedger,
    artifacts: ArtifactStore,
    /// Kept open for the writer's lifetime.
    _database: Arc<RuntimeDatabase>,
    _directory: TempDir,
}

impl FormalRoot {
    /// State tables, the formal ledger marker, the writer and the ArtifactStore with the
    /// fixture capacity admission of `GlobalLedgerSink`.
    pub(super) fn create() -> Self {
        let directory = tempfile::tempdir().expect("formal root directory");
        let database = Arc::new(
            RuntimeStateStore::open_database(directory.path(), STATE_KEY).expect("State database"),
        );
        let limits = MaintenanceLimits::default();
        let maintenance = LedgerMaintenance::acquire(
            directory.path(),
            true,
            limits,
            limits.deadline().expect("maintenance deadline"),
        )
        .expect("maintenance owner");
        maintenance
            .initialize_empty(&database)
            .expect("formal ledger");
        let ledger = maintenance
            .open_writer(Arc::clone(&database), OWNER_ID.into())
            .expect("formal writer");
        let artifacts = ArtifactStore::open(directory.path()).expect("artifact owner");
        let mut admission = GlobalLedgerSink::new(None);
        admission.capacity_root = Some(artifacts.root().to_path_buf());
        artifacts
            .install_capacity_admission(Arc::new(admission))
            .expect("fixture capacity owner");
        Self {
            ledger,
            artifacts,
            _database: database,
            _directory: directory,
        }
    }

    /// The root's writer.
    pub(super) fn ledger(&self) -> &GlobalLedger {
        &self.ledger
    }

    /// The root's ArtifactStore.
    pub(super) fn artifacts(&self) -> &ArtifactStore {
        &self.artifacts
    }
}
