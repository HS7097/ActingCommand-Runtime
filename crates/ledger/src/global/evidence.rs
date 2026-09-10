// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
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
impl GlobalLedger {
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
