// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::events::RuntimeEvents;
use actingcommand_contract::{
    AuditInput, EventActor, EventSeverity, EventSource, OriginModule, StateMigrationData,
    StatePayload, StatePayloadDraft,
};
use actingcommand_ledger::{GlobalLedgerError, LedgerTransactionWork, TransactionWorkError};
use actingcommand_runtime_database::RuntimeTransaction;
use actingcommand_runtime_state::PreparedCatalogState;

pub(crate) fn catalog_ledger_error(error: &GlobalLedgerError) -> RuntimeHostError {
    RuntimeHostError::fatal(
        error.code(),
        error.operation(),
        RuntimeErrorCode::LedgerFailure,
    )
    .with_native_detail(format!("{error}; detail={:?}", error.detail()))
}

fn work_error(error: RuntimeHostError) -> TransactionWorkError {
    TransactionWorkError {
        code: error.code(),
        operation: error.operation(),
        fatal: error.is_fatal(),
        detail: format!("{error:?}"),
    }
}

enum CatalogChange {
    Transition {
        generation: CatalogGeneration,
        previous: Option<String>,
        current_matches: bool,
    },
    Migration(StateMigrationData),
}

pub(super) struct CatalogTransaction {
    state: Result<PreparedCatalogState, TransactionWorkError>,
    change: CatalogChange,
}

impl LedgerTransactionWork for CatalogTransaction {
    fn observe(
        &self,
        scope: &RuntimeTransaction<'_, '_>,
    ) -> Result<actingcommand_ledger::TransactionStateObservation, TransactionWorkError> {
        use actingcommand_ledger::TransactionStateObservation as Ledger;
        use actingcommand_runtime_state::CatalogStateObservation as State;
        self.state
            .as_ref()
            .map_err(Clone::clone)?
            .observe(scope)
            .map(|value| match value {
                State::Applied => Ledger::Applied,
                State::Unchanged => Ledger::Unchanged,
                State::Unknown => Ledger::Unknown,
            })
            .map_err(|error| TransactionWorkError {
                code: error.code(),
                operation: error.operation(),
                fatal: error.is_fatal(),
                detail: error.to_string(),
            })
    }

    fn apply(
        &self,
        transaction: &RuntimeTransaction<'_, '_>,
        event: &PersistedEvent,
    ) -> Result<(), TransactionWorkError> {
        if matches!(
            self.change,
            CatalogChange::Transition {
                current_matches: false,
                ..
            }
        ) {
            return Err(work_error(request(
                "catalog_active_generation_changed",
                "switch_active_catalog",
            )));
        }
        let state = self.state.as_ref().map_err(Clone::clone)?;
        let matches = match (&self.change, event.payload()) {
            (
                CatalogChange::Transition {
                    generation,
                    previous,
                    ..
                },
                EventPayload::Catalog(
                    CatalogPayload::Activated(data) | CatalogPayload::RolledBack(data),
                ),
            ) => {
                event.origin().source() == EventSource::Runtime
                    && event.origin().module() == OriginModule::Policy
                    && event.origin().actor() == EventActor::Runtime
                    && data.catalog_id() == generation.catalog_id()
                    && data.catalog_version() == generation.catalog_version()
                    && data.catalog_hash() == generation.catalog_hash()
                    && data.previous_catalog_hash() == previous.as_deref()
            }
            (
                CatalogChange::Migration(expected),
                EventPayload::State(StatePayload::Migrated(data)),
            ) => {
                event.origin().source() == EventSource::Runtime
                    && event.origin().module() == OriginModule::Runtime
                    && event.origin().actor() == EventActor::Runtime
                    && data.migration() == expected
            }
            _ => false,
        };
        if !matches {
            return Err(work_error(fatal(
                "catalog_transaction_fact_mismatch",
                "commit_catalog_state",
            )));
        }
        state.apply(transaction).map_err(|error| {
            let mut mapped = work_error(RuntimeHostError::state(&error));
            mapped.detail = error.to_string();
            mapped
        })
    }
}

impl CatalogStore {
    pub(super) fn prepare_transition(
        &self,
        catalog: &LoadedCatalog,
        previous: Option<&str>,
        current_matches: bool,
    ) -> CatalogTransaction {
        let state = (|| {
            let current = self.load_active()?;
            if current
                .as_ref()
                .map(|value| value.generation.catalog_hash())
                != previous
            {
                return Err(request("state_document_changed", "write_state_document"));
            }
            let document = self
                .state
                .read_json_document(ACTIVE_POINTER_STATE_KEY)
                .map_err(|error| RuntimeHostError::state(&error))?;
            let pointer = CatalogPointer {
                schema_version: CATALOG_STATE_SCHEMA.to_owned(),
                generation: catalog.generation.clone(),
            };
            let bytes = serde_json::to_vec(&pointer)
                .map_err(|_| fatal("catalog_pointer_encode_failed", "switch_active_catalog"))?;
            self.state
                .prepare_catalog_write(
                    CATALOG_STATE_SCHEMA,
                    &bytes,
                    document.as_ref().map(|value| value.payload_sha256()),
                )
                .map_err(|error| RuntimeHostError::state(&error))
        })()
        .map_err(work_error);
        CatalogTransaction {
            state,
            change: CatalogChange::Transition {
                generation: catalog.generation.clone(),
                previous: previous.map(str::to_owned),
                current_matches,
            },
        }
    }

    pub(super) fn migrate_legacy_active_pointer(
        &self,
        ledger: &GlobalLedger,
        events: &RuntimeEvents,
    ) -> RuntimeHostResult<()> {
        if !self.legacy_active_pointer.exists() {
            return Ok(());
        }
        let bytes = read_bounded(&self.legacy_active_pointer, MAX_POINTER_BYTES)?;
        let pointer: CatalogPointer = serde_json::from_slice(&bytes)
            .map_err(|_| fatal("catalog_pointer_invalid", "migrate_active_catalog"))?;
        if pointer.schema_version != CATALOG_STATE_SCHEMA {
            return Err(fatal(
                "catalog_pointer_version_unsupported",
                "migrate_active_catalog",
            ));
        }
        let loaded = self.load_generation(&pointer.generation.catalog_hash)?;
        if loaded.generation != pointer.generation {
            return Err(fatal(
                "catalog_pointer_generation_mismatch",
                "migrate_active_catalog",
            ));
        }
        let canonical = serde_json::to_vec(&pointer)
            .map_err(|_| fatal("catalog_pointer_encode_failed", "migrate_active_catalog"))?;
        let latest = ledger
            .latest_sequence()
            .map_err(|error| catalog_ledger_error(&error))?;
        if self
            .project_catalog_source(ledger, latest)?
            .as_ref()
            .is_some_and(|active| active.generation != pointer.generation)
        {
            return Err(fatal(
                "catalog_migration_source_conflict",
                "migrate_active_catalog",
            ));
        }
        let prepared = self
            .state
            .prepare_catalog_migration(
                LEGACY_CATALOG_POINTER_SCHEMA,
                CATALOG_STATE_SCHEMA,
                &canonical,
            )
            .map_err(|error| RuntimeHostError::state(&error))?;
        let migration = prepared
            .migration()
            .ok_or_else(|| fatal("catalog_migration_missing", "migrate_active_catalog"))?
            .clone();
        let recorded = ledger
            .query(EventQuery {
                event_type: Some(EventType::StateMigrated),
                ..EventQuery::default()
            })
            .map_err(|error| catalog_ledger_error(&error))?;
        let mut matching = 0;
        for event in &recorded {
            if let EventPayload::State(StatePayload::Migrated(data)) = event.payload()
                && data.migration().migration_id() == migration.migration_id()
            {
                if data.migration() != &migration {
                    return Err(fatal(
                        "catalog_migration_source_conflict",
                        "migrate_active_catalog",
                    ));
                }
                matching += 1;
            }
        }
        if matching > 1 {
            return Err(fatal(
                "catalog_migration_source_conflict",
                "migrate_active_catalog",
            ));
        }
        if matching == 0 {
            let draft = events.draft(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                events.system_links()?,
                StatePayloadDraft::migrated(migration.clone(), AuditInput::new()),
            )?;
            let draft = events.sanitize(draft)?;
            ledger
                .append_transaction(
                    draft,
                    Box::new(CatalogTransaction {
                        state: Ok(prepared),
                        change: CatalogChange::Migration(migration.clone()),
                    }),
                )
                .map_err(|error| match error.rolled_back_work() {
                    Some(work) => RuntimeHostError::fatal(
                        work.code,
                        work.operation,
                        RuntimeErrorCode::RuntimeFatal,
                    )
                    .with_native_detail(work.detail.clone()),
                    None => catalog_ledger_error(&error),
                })?;
        }
        let document = self
            .state
            .catalog_migration_document(&migration)
            .map_err(|error| RuntimeHostError::state(&error))?;
        if document.payload() != canonical {
            return Err(fatal(
                "catalog_migration_source_conflict",
                "migrate_active_catalog",
            ));
        }
        // Verify the current chain before retiring the original file. A committed migration
        // may already have later transitions; its historical document remains the source.
        self.load_verified_active(ledger)?;
        fs::remove_file(&self.legacy_active_pointer).map_err(|error| {
            fatal("catalog_pointer_cleanup_failed", "migrate_active_catalog")
                .with_native_detail(error.to_string())
        })?;
        sync_directory(&self.root, "migrate_active_catalog")
    }

    pub(super) fn load_verified_active(
        &self,
        ledger: &GlobalLedger,
    ) -> RuntimeHostResult<Option<LoadedCatalog>> {
        let latest = ledger
            .latest_sequence()
            .map_err(|error| catalog_ledger_error(&error))?;
        let source = self.project_catalog_source(ledger, latest)?;
        let actual = self.load_active()?;
        if actual.as_ref().map(|value| &value.generation)
            != source.as_ref().map(|value| &value.generation)
        {
            return Err(fatal(
                "catalog_active_source_mismatch",
                "load_active_catalog",
            ));
        }
        Ok(actual)
    }

    pub(super) fn project_catalog_source(
        &self,
        ledger: &GlobalLedger,
        through: u64,
    ) -> RuntimeHostResult<Option<LoadedCatalog>> {
        let events = ledger
            .query(EventQuery {
                to_sequence: Some(through),
                ..EventQuery::default()
            })
            .map_err(|error| catalog_ledger_error(&error))?;
        let mut current: Option<(String, u64, String)> = None;
        let mut intents = BTreeMap::new();
        let mut migrations = BTreeSet::new();
        for event in &events {
            if let EventPayload::State(StatePayload::Migrated(payload)) = event.payload() {
                let data = payload.migration();
                if data.state_key() != ACTIVE_POINTER_STATE_KEY {
                    continue;
                }
                if !migrations.insert(data.migration_id())
                    || event.origin().source() != EventSource::Runtime
                    || event.origin().module() != OriginModule::Runtime
                    || event.origin().actor() != EventActor::Runtime
                    || data.to_schema_version() != CATALOG_STATE_SCHEMA
                {
                    return Err(fatal(
                        "catalog_migration_source_conflict",
                        "project_policy_catalog",
                    ));
                }
                let document = self
                    .state
                    .catalog_migration_document(data)
                    .map_err(|error| RuntimeHostError::state(&error))?;
                let pointer: CatalogPointer = serde_json::from_slice(document.payload())
                    .map_err(|_| fatal("catalog_pointer_invalid", "project_policy_catalog"))?;
                if pointer.schema_version != CATALOG_STATE_SCHEMA {
                    return Err(fatal(
                        "catalog_pointer_version_unsupported",
                        "project_policy_catalog",
                    ));
                }
                let loaded = self.load_generation(&pointer.generation.catalog_hash)?;
                if loaded.generation != pointer.generation {
                    return Err(fatal(
                        "catalog_pointer_generation_mismatch",
                        "project_policy_catalog",
                    ));
                }
                if current.as_ref().is_some_and(|(id, version, hash)| {
                    id != pointer.generation.catalog_id()
                        || *version != pointer.generation.catalog_version()
                        || hash != pointer.generation.catalog_hash()
                }) {
                    return Err(fatal(
                        "catalog_migration_source_conflict",
                        "project_policy_catalog",
                    ));
                }
                current = Some((
                    pointer.generation.catalog_id,
                    pointer.generation.catalog_version,
                    pointer.generation.catalog_hash,
                ));
                continue;
            }
            let (payload, success) = match event.payload() {
                EventPayload::Catalog(CatalogPayload::TransitionIntent(payload)) => {
                    let action_id = event.links().action_id().ok_or_else(|| {
                        fatal("catalog_intent_identity_missing", "project_policy_catalog")
                    })?;
                    if event.origin().source() != EventSource::Runtime
                        || event.origin().module() != OriginModule::Policy
                        || event.origin().actor() != EventActor::Runtime
                        || intents.insert(action_id, (event, payload)).is_some()
                    {
                        return Err(fatal(
                            "catalog_intent_source_conflict",
                            "project_policy_catalog",
                        ));
                    }
                    continue;
                }
                EventPayload::Catalog(
                    CatalogPayload::Activated(payload) | CatalogPayload::RolledBack(payload),
                ) => (payload, true),
                EventPayload::Catalog(CatalogPayload::TransitionFailed(payload)) => {
                    (payload, false)
                }
                _ => continue,
            };
            let action_id = event.links().action_id().ok_or_else(|| {
                fatal("catalog_outcome_identity_missing", "project_policy_catalog")
            })?;
            let (intent_event, intent) = intents
                .remove(action_id)
                .ok_or_else(|| fatal("catalog_outcome_intent_missing", "project_policy_catalog"))?;
            if event.origin() != intent_event.origin()
                || event.links() != intent_event.links()
                || event.payload().action() != intent_event.payload().action()
                || payload.catalog_id() != intent.catalog_id()
                || payload.catalog_version() != intent.catalog_version()
                || payload.catalog_hash() != intent.catalog_hash()
                || payload.previous_catalog_hash() != intent.previous_catalog_hash()
                || payload.promotion() != intent.promotion()
            {
                return Err(fatal(
                    "catalog_outcome_source_conflict",
                    "project_policy_catalog",
                ));
            }
            if !success {
                continue;
            }
            if payload.previous_catalog_hash() != current.as_ref().map(|value| value.2.as_str()) {
                return Err(fatal(
                    "catalog_previous_source_mismatch",
                    "project_policy_catalog",
                ));
            }
            if let Some((id, version, _)) = &current {
                if id != payload.catalog_id()
                    || match event.event_type() {
                        EventType::CatalogActivated => payload.catalog_version() <= *version,
                        EventType::CatalogRolledBack => payload.catalog_version() >= *version,
                        _ => true,
                    }
                {
                    return Err(fatal(
                        "catalog_generation_source_conflict",
                        "project_policy_catalog",
                    ));
                }
            } else if event.event_type() != EventType::CatalogActivated {
                return Err(fatal(
                    "catalog_rollback_source_missing",
                    "project_policy_catalog",
                ));
            }
            current = Some((
                payload.catalog_id().to_owned(),
                payload.catalog_version(),
                payload.catalog_hash().to_owned(),
            ));
        }
        let Some((id, version, hash)) = current else {
            return Ok(None);
        };
        let loaded = self.load_generation(&hash)?;
        if loaded.generation.catalog_id() != id || loaded.generation.catalog_version() != version {
            return Err(fatal(
                "catalog_projection_identity_mismatch",
                "project_policy_catalog",
            ));
        }
        Ok(Some(loaded))
    }
}
