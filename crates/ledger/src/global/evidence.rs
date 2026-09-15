// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::fact::{LedgerEventMetadata, LedgerEventRead};
use actingcommand_contract::{
    ArtifactEvictionObservation, ArtifactId, EventLinks, FrameId, LedgerEventPosition,
    LedgerMaterialReadState, LedgerReadScope, LedgerReadSource, RequestId, RunId,
    RuntimeEventQueryPage, RuntimeEventQueryPageRequest, Sensitivity,
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
    /// Applies one operation deadline while retaining any stricter source limits.
    pub fn with_deadline(mut self, deadline: Instant) -> Self {
        self.budget = Some(artifact_read_budget(self.budget, deadline));
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
/// Selects an artifact only from one committed event's original artifact set.
#[derive(Debug, Clone)]
pub struct LedgerArtifactSelection {
    pub event: LedgerEventPosition,
    pub artifact_id: ArtifactId,
    pub snapshot_position: u64,
    pub sha256: String,
    pub byte_count: u64,
    pub run_id: Option<RunId>,
    pub frame_id: Option<FrameId>,
    pub request_id: Option<RequestId>,
    pub correlation_id: Option<CorrelationId>,
}

impl LedgerArtifactSelection {
    pub(super) fn validate(&self) -> GlobalLedgerResult<()> {
        if self.event.sequence == 0
            || self.event.sequence > self.snapshot_position
            || self.byte_count == 0
        {
            return Err(GlobalLedgerError::request(
                "invalid_artifact_selection",
                "resolve_ledger_artifact",
            ));
        }
        Ok(())
    }
}

/// Ledger-authenticated reference and retention; the referenced bytes remain unverified.
#[derive(Debug, Clone)]
pub struct ResolvedLedgerArtifact {
    event: LedgerEventPosition,
    reference: ProjectedArtifactReference,
    links: EventLinks,
    sensitivity: Sensitivity,
    snapshot_position: u64,
    availability_through: u64,
    eviction: Option<ArtifactEvictionObservation>,
}

impl ResolvedLedgerArtifact {
    pub fn event(&self) -> LedgerEventPosition {
        self.event
    }
    /// The internal object key is for the original ArtifactStore reader, not wire output.
    pub fn reference(&self) -> &ProjectedArtifactReference {
        &self.reference
    }
    pub fn links(&self) -> &EventLinks {
        &self.links
    }
    pub fn sensitivity(&self) -> Sensitivity {
        self.sensitivity
    }
    pub fn snapshot_position(&self) -> u64 {
        self.snapshot_position
    }
    pub fn availability_through(&self) -> u64 {
        self.availability_through
    }
    /// Absence means no eviction fact at this position; material still requires verification.
    pub fn eviction(&self) -> Option<&ArtifactEvictionObservation> {
        self.eviction.as_ref()
    }
}

pub(super) fn artifact_read_budget(
    budget: Option<(u64, usize, Instant)>,
    deadline: Instant,
) -> (u64, usize, Instant) {
    let (bytes, events, source_deadline) = budget.unwrap_or((u64::MAX, usize::MAX, deadline));
    (bytes, events, source_deadline.min(deadline))
}

/// All events and retention proofs must come from the same fully authenticated source.
pub(super) fn resolve_artifact_from_events<E: LedgerEventRead>(
    events: &[E],
    selection: &LedgerArtifactSelection,
    through_sequence: u64,
    read_complete: bool,
    deadline: Instant,
    retention: Option<&retention::RetentionIndex>,
) -> GlobalLedgerResult<ResolvedLedgerArtifact> {
    let budget = Some(artifact_read_budget(None, deadline));
    read_only::check_read_budget(budget, 0, 0)?;
    selection.validate()?;
    let denied = |code| GlobalLedgerError::request(code, "resolve_ledger_artifact");
    if !read_complete {
        return Err(denied("ledger_source_incomplete"));
    }
    if selection.snapshot_position > through_sequence {
        return Err(denied("artifact_snapshot_position_invalid"));
    }
    let position = events
        .binary_search_by_key(&selection.event.sequence, |event| event.sequence())
        .map_err(|_| denied("artifact_event_not_found"))?;
    let event = &events[position];
    if event.event_id() != &selection.event.event_id {
        return Err(denied("artifact_event_not_found"));
    }
    let reference = event
        .projected_artifacts(true)
        .into_iter()
        .find(|reference| reference.artifact_id == selection.artifact_id)
        .ok_or_else(|| denied("artifact_reference_not_found"))?;
    if reference.sha256 != selection.sha256 || reference.byte_count != selection.byte_count {
        return Err(denied("artifact_reference_mismatch"));
    }
    let links = event.links();
    if selection
        .run_id
        .is_some_and(|id| links.run_id() != Some(&id))
        || selection
            .frame_id
            .is_some_and(|id| links.frame_id() != Some(&id))
        || selection
            .request_id
            .is_some_and(|id| links.request_id() != Some(&id))
        || selection
            .correlation_id
            .is_some_and(|id| links.correlation_id() != Some(&id))
    {
        return Err(denied("artifact_event_links_mismatch"));
    }
    let proof = match retention {
        Some(retention) => retention.proof(&reference)?,
        None => event
            .artifact_evictions()
            .iter()
            .find(|proof| proof.identity.artifact.artifact_id == reference.artifact_id)
            .cloned(),
    };
    if proof
        .as_ref()
        .is_some_and(|proof| proof.identity.artifact != reference)
    {
        return Err(GlobalLedgerError::fatal(
            "artifact_identity_conflict",
            "resolve_ledger_artifact",
        ));
    }
    let eviction = proof.and_then(|proof| proof.observation(through_sequence));
    read_only::check_read_budget(budget, 0, 0)?;
    Ok(ResolvedLedgerArtifact {
        event: selection.event,
        reference,
        links: links.clone(),
        sensitivity: event.sensitivity(),
        snapshot_position: selection.snapshot_position,
        availability_through: through_sequence,
        eviction,
    })
}

/// An immutable, authenticated ledger snapshot with reference metadata only.
/// This type exposes projected pages and cannot issue artifact material authority.
pub struct GlobalLedgerMetadata {
    sqlite: Option<sqlite::SqliteViewSnapshot>,
    events: Vec<LedgerEventMetadata>,
    repair_count: Option<usize>,
    indexes: projection::EventIndexes,
    through_sequence: u64,
    writer: GlobalLedgerWriterMetadataObservation,
    backend: &'static str,
    read_complete: bool,
    corrupt_tail: Option<GlobalLedgerCorruptTail>,
    budget: Option<(u64, usize, Instant)>,
}

impl GlobalLedgerMetadata {
    /// Resolves this opening's snapshot. Reopen metadata under the material use guard
    /// before rechecking current retention; an old instance cannot observe new facts.
    pub fn resolve_artifact(
        &self,
        selection: &LedgerArtifactSelection,
        deadline: Instant,
    ) -> GlobalLedgerResult<ResolvedLedgerArtifact> {
        let budget = Some(artifact_read_budget(self.budget, deadline));
        read_only::check_read_budget(budget, 0, self.events.len())?;
        let resolved = resolve_artifact_from_events(
            &self.events,
            selection,
            self.through_sequence,
            self.read_complete && self.corrupt_tail.is_none(),
            deadline,
            None,
        )?;
        read_only::check_read_budget(budget, 0, self.events.len())?;
        Ok(resolved)
    }

    pub fn project_view_page(
        &self,
        query: &EventQuery,
        profile: ProjectionProfile,
        request: &RuntimeEventQueryPageRequest,
    ) -> GlobalLedgerResult<RuntimeEventQueryPage> {
        if let Some(sqlite) = &self.sqlite {
            return sqlite.project_view_page(query, profile, request);
        }
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
            self.through_sequence.into(),
        )
    }
    /// Counts this opening's authenticated, unfiltered events without further I/O.
    /// An incomplete read counts only the verified prefix; inspect `read_complete`,
    /// `corrupt_tail` and `latest_sequence` for its boundary.
    pub fn event_count(&self) -> usize {
        self.events.len()
    }

    /// Counts the segment opening's repair records, including incomplete repairs.
    /// SQLite has no repair-log source and returns `None`. The repair log does not
    /// share the event snapshot's sequence boundary.
    pub fn repair_count(&self) -> Option<usize> {
        self.repair_count
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
    /// Resolves a reference and current retention on the original writer queue.
    /// Queue/system waits retain their existing limits; the deadline is cooperative.
    pub fn resolve_artifact(
        &self,
        selection: LedgerArtifactSelection,
        deadline: Instant,
    ) -> GlobalLedgerResult<ResolvedLedgerArtifact> {
        selection.validate()?;
        let budget = Some(artifact_read_budget(None, deadline));
        read_only::check_read_budget(budget, 0, 0)?;
        let (response, receiver) = mpsc::sync_channel(1);
        let sender = self.sender.as_ref().ok_or_else(|| {
            GlobalLedgerError::fatal("writer_unavailable", "resolve_ledger_artifact")
        })?;
        send_command(
            sender,
            WriterCommand::ResolveArtifact {
                selection: Box::new(selection),
                deadline,
                response,
            },
            "resolve_ledger_artifact",
        )?;
        let resolved = receive_response(receiver, "resolve_ledger_artifact")??;
        read_only::check_read_budget(budget, 0, 0)?;
        Ok(resolved)
    }

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
                let (events, sqlite) = sqlite::open_metadata(Arc::new(database), config.budget)?;
                let through_sequence = sqlite.through_sequence;
                let writer = read_only::read_writer_metadata(&ledger_root)?;
                return Ok(GlobalLedgerMetadata {
                    sqlite: Some(sqlite),
                    indexes: projection::EventIndexes::from_events(&events),
                    events,
                    repair_count: None,
                    through_sequence,
                    writer,
                    backend: "sqlite",
                    read_complete: true,
                    corrupt_tail: None,
                    budget: config.budget,
                });
            }
        }
        let mut segment_config = GlobalLedgerReadOnlyConfig::new(ledger_root);
        segment_config.budget = config.budget;
        let source = read_only::open_metadata(segment_config)?;
        Ok(GlobalLedgerMetadata {
            sqlite: None,
            through_sequence: source
                .events
                .last()
                .map_or(0, LedgerEventMetadata::sequence),
            indexes: projection::EventIndexes::from_events(&source.events),
            events: source.events,
            repair_count: Some(source.repairs.len()),
            writer: source.writer_metadata,
            backend: "segment",
            read_complete: source.storage_snapshot.read_complete && source.corrupt_tail.is_none(),
            corrupt_tail: source.corrupt_tail,
            budget: config.budget,
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
