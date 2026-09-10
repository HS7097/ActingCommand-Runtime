// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::fact::{LedgerEventMetadata, LedgerEventRead};
use actingcommand_contract::{
    LedgerMaterialReadState, LedgerReadScope, LedgerReadSource, RuntimeEventQueryPage,
    RuntimeEventQueryPageRequest,
};
use actingcommand_runtime_database::RuntimeDatabase;

/// An explicit Runtime state root, independent of the stored ledger medium.
#[derive(Debug, Clone)]
pub struct GlobalLedgerEvidenceConfig {
    root: PathBuf,
    budget: Option<(u64, usize, Instant)>,
}
impl GlobalLedgerEvidenceConfig {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            budget: None,
        }
    }
    pub fn with_budget(mut self, bytes: u64, events: usize, deadline: Instant) -> Self {
        self.budget = Some((bytes, events, deadline));
        self
    }
}

/// Verified facts and the actual writer observation, with physical fields kept separate.
pub struct GlobalLedgerEvidence {
    source: EvidenceSource,
    writer: GlobalLedgerWriterMetadataObservation,
}
enum EvidenceSource {
    Segment(Box<GlobalLedgerReadOnly>),
    Sqlite(Box<SqliteLedgerReadOnly>),
}
impl GlobalLedgerEvidence {
    pub fn events(&self) -> &[PersistedEvent] {
        match &self.source {
            EvidenceSource::Segment(source) => source.events(),
            EvidenceSource::Sqlite(source) => source.events(),
        }
    }
    pub fn query(&self, query: &EventQuery) -> Vec<PersistedEvent> {
        match &self.source {
            EvidenceSource::Segment(source) => source.query(query),
            EvidenceSource::Sqlite(source) => source.query(query),
        }
    }
    pub fn query_page(
        &self,
        query: &EventQuery,
        after: u64,
        through: u64,
        limit: usize,
    ) -> GlobalLedgerResult<Vec<PersistedEvent>> {
        match &self.source {
            EvidenceSource::Segment(source) => source.query_page(query, after, through, limit),
            EvidenceSource::Sqlite(source) => source.query_page(query, after, through, limit),
        }
    }
    pub fn latest_sequence(&self) -> u64 {
        self.events().last().map_or(0, PersistedEvent::sequence)
    }

    pub fn segment(&self) -> Option<&GlobalLedgerReadOnly> {
        match &self.source {
            EvidenceSource::Segment(source) => Some(source),
            EvidenceSource::Sqlite(_) => None,
        }
    }
    pub fn backend(&self) -> &'static str {
        match self.source {
            EvidenceSource::Segment(_) => "segment",
            EvidenceSource::Sqlite(_) => "sqlite",
        }
    }
    pub fn writer_metadata(&self) -> &GlobalLedgerWriterMetadataObservation {
        &self.writer
    }
    pub fn read_complete(&self) -> bool {
        self.segment()
            .is_none_or(|source| source.storage_snapshot().read_complete)
    }
    pub fn corrupt_tail(&self) -> Option<&GlobalLedgerCorruptTail> {
        self.segment().and_then(GlobalLedgerReadOnly::corrupt_tail)
    }
    pub fn is_complete(&self) -> bool {
        self.read_complete() && self.corrupt_tail().is_none()
    }
}
/// An immutable, authenticated ledger snapshot with reference metadata only.
/// This type exposes projected pages and cannot issue artifact material authority.
pub struct GlobalLedgerMetadata {
    events: Vec<LedgerEventMetadata>,
    indexes: projection::EventIndexes,
    through_sequence: u64,
    writer: GlobalLedgerWriterMetadataObservation,
    backend: &'static str,
    read_complete: bool,
    corrupt_tail: Option<GlobalLedgerCorruptTail>,
}

impl GlobalLedgerMetadata {
    pub fn project_view_page(
        &self,
        query: &EventQuery,
        profile: ProjectionProfile,
        request: &RuntimeEventQueryPageRequest,
    ) -> GlobalLedgerResult<RuntimeEventQueryPage> {
        self.indexes.project_view_page(
            &self.events,
            query,
            profile,
            request,
            LedgerReadScope {
                source: LedgerReadSource::Offline,
                material_read: LedgerMaterialReadState::NotRequested,
                scanned_through_position: self.through_sequence,
                read_complete: self.read_complete,
                limits: Vec::new(),
            },
            self.through_sequence,
        )
    }
    pub fn latest_sequence(&self) -> u64 {
        self.through_sequence
    }
    pub fn read_complete(&self) -> bool {
        self.read_complete
    }
    pub fn writer_metadata(&self) -> &GlobalLedgerWriterMetadataObservation {
        &self.writer
    }
    pub fn backend(&self) -> &'static str {
        self.backend
    }
    pub fn corrupt_tail(&self) -> Option<&GlobalLedgerCorruptTail> {
        self.corrupt_tail.as_ref()
    }
}

impl GlobalLedger {
    /// Reads ledger records and their integrity data without opening referenced artifacts.
    pub fn open_metadata(
        config: GlobalLedgerEvidenceConfig,
    ) -> GlobalLedgerResult<GlobalLedgerMetadata> {
        let ledger_root = config.root.join("ledger");
        let database_exists = config
            .root
            .join(actingcommand_runtime_database::DATABASE_FILE)
            .try_exists()
            .map_err(|error| {
                GlobalLedgerError::io("ledger_io", "inspect_evidence_database", &error)
            })?;
        let key_exists = config
            .root
            .join(actingcommand_runtime_database::INTEGRITY_KEY_FILE)
            .try_exists()
            .map_err(|error| GlobalLedgerError::io("ledger_io", "inspect_evidence_key", &error))?;
        if database_exists || key_exists {
            let database = RuntimeDatabase::open_existing(&config.root, true)?;
            if sqlite::has_schema(&database)? {
                let (events, through_sequence) = sqlite::open_metadata(&database, config.budget)?;
                let writer = read_only::read_writer_metadata(&ledger_root)?;
                return Ok(GlobalLedgerMetadata {
                    indexes: projection::EventIndexes::from_events(&events),
                    events,
                    through_sequence,
                    writer,
                    backend: "sqlite",
                    read_complete: true,
                    corrupt_tail: None,
                });
            }
        }
        let mut segment_config = GlobalLedgerReadOnlyConfig::new(ledger_root);
        segment_config.budget = config.budget;
        let source = read_only::open_metadata(segment_config)?;
        Ok(GlobalLedgerMetadata {
            through_sequence: source
                .events
                .last()
                .map_or(0, LedgerEventMetadata::sequence),
            indexes: projection::EventIndexes::from_events(&source.events),
            events: source.events,
            writer: source.writer_metadata,
            backend: "segment",
            read_complete: source.storage_snapshot.read_complete && source.corrupt_tail.is_none(),
            corrupt_tail: source.corrupt_tail,
        })
    }

    /// Select the medium from formal metadata in an explicitly supplied state root.
    pub fn open_evidence<F>(
        config: GlobalLedgerEvidenceConfig,
        mut verifier: F,
    ) -> GlobalLedgerResult<GlobalLedgerEvidence>
    where
        F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
    {
        let ledger_root = config.root.join("ledger");
        let database_path = config
            .root
            .join(actingcommand_runtime_database::DATABASE_FILE);
        let database_exists = database_path.try_exists().map_err(|error| {
            GlobalLedgerError::io("ledger_io", "inspect_evidence_database", &error)
        })?;
        let key_exists = config
            .root
            .join(actingcommand_runtime_database::INTEGRITY_KEY_FILE)
            .try_exists()
            .map_err(|error| GlobalLedgerError::io("ledger_io", "inspect_evidence_key", &error))?;
        if database_exists || key_exists {
            let database = RuntimeDatabase::open_existing(&config.root, true)?;
            if sqlite::has_schema(&database)? {
                let source =
                    SqliteLedgerReadOnly::open_formal(&database, config.budget, &mut verifier)?;
                let writer = read_only::read_writer_metadata(&ledger_root)?;
                return Ok(GlobalLedgerEvidence {
                    source: EvidenceSource::Sqlite(Box::new(source)),
                    writer,
                });
            }
        }
        let mut segment_config = GlobalLedgerReadOnlyConfig::new(ledger_root);
        segment_config.budget = config.budget;
        let source = Self::open_read_only(segment_config, verifier)?;
        let writer = source.writer_metadata().clone();
        Ok(GlobalLedgerEvidence {
            source: EvidenceSource::Segment(Box::new(source)),
            writer,
        })
    }
}
