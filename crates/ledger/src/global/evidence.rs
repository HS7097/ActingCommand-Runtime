// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::codes::{LedgerCode, LedgerLocation};
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
    material: EvidenceMaterial,
    prefix: Option<u64>,
}
/// How an SQLite opening treats referenced artifact material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EvidenceMaterial {
    /// Every unevicted artifact must verify, or the whole opening fails.
    Required,
    /// Each artifact is verified; a verifier `None` leaves only that artifact Unrecorded.
    PerArtifact,
    /// The verifier is never called.
    NotRead,
}
impl GlobalLedgerEvidenceConfig {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            budget: None,
            material: EvidenceMaterial::Required,
            prefix: None,
        }
    }
    /// SQLite only: authenticates records exactly as the default opening, then verifies
    /// each unevicted artifact separately. When the verifier returns `None`, that artifact
    /// stays `ArtifactAvailability::Unrecorded` and the opening continues; the caller is
    /// responsible for recording and reporting every `None` it returns. Segment roots still
    /// scan all material as before. `GlobalLedgerEvidence::material_checked()` and
    /// `Unrecorded` are the only external signals.
    pub fn sqlite_material_per_artifact(mut self) -> Self {
        self.material = EvidenceMaterial::PerArtifact;
        self
    }
    /// SQLite only: authenticates records exactly as the default opening and never calls
    /// the verifier; unevicted artifacts stay `ArtifactAvailability::Unrecorded` and
    /// `GlobalLedgerEvidence::material_checked()` is false. Segment roots still scan all
    /// material as before.
    pub fn sqlite_material_not_read(mut self) -> Self {
        self.material = EvidenceMaterial::NotRead;
        self
    }
    /// SQLite only (Workflow #363): authenticates the keyed head row, then reads and
    /// authenticates only events `1..=through_sequence` (a migrated ledger also reads
    /// through its cutover completion). Later rows are never read, so this opening does
    /// not establish their integrity, and artifact availability is as of the prefix.
    /// Requires a record path (`sqlite_material_not_read` or
    /// `sqlite_material_per_artifact`); a segment root is read whole, as before. A
    /// `through_sequence` beyond the authenticated head is the request error
    /// `ledger_prefix_beyond_head`.
    pub fn sqlite_prefix(mut self, through_sequence: u64) -> Self {
        self.prefix = Some(through_sequence);
        self
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
    Records(Box<RecordEvidence>),
}
/// SQLite records authenticated by the metadata opening, with per-artifact material state.
struct RecordEvidence {
    events: Vec<PersistedEvent>,
    indexes: projection::EventIndexes,
    material_checked: bool,
    extent: GlobalLedgerReadExtent,
}

/// What one evidence opening read (Workflow #363). In memory only; never persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalLedgerReadExtent {
    /// The last authenticated event: for an SQLite prefix read the declared prefix (or a
    /// migrated root's cutover completion), otherwise the head.
    pub through_sequence: u64,
    /// The source's authenticated head when it was opened.
    pub head_sequence: u64,
    /// Events read and authenticated.
    pub event_count: usize,
    /// Ledger bytes counted by the reader; `None` where the opening does not count them.
    pub ledger_bytes: Option<u64>,
    /// Wall time per phase of an SQLite record-path opening; `None` for other paths.
    pub phases: Option<GlobalLedgerReadPhases>,
}

/// Wall time per phase of an SQLite record-path opening (Workflow #363).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalLedgerReadPhases {
    /// Head authentication and the row reads.
    pub sql_read: Duration,
    /// Row, chain, tag, relation, head and marker authentication.
    pub verify: Duration,
    /// Eviction annotation, retention replay and per-artifact restore.
    pub retention_restore: Duration,
}
impl GlobalLedgerEvidence {
    pub fn events(&self) -> &[PersistedEvent] {
        match &self.source {
            EvidenceSource::Segment(source) => source.events(),
            EvidenceSource::Sqlite(source) => source.events(),
            EvidenceSource::Records(source) => &source.events,
        }
    }
    pub fn query(&self, query: &EventQuery) -> Vec<PersistedEvent> {
        match &self.source {
            EvidenceSource::Segment(source) => source.query(query),
            EvidenceSource::Sqlite(source) => source.query(query),
            EvidenceSource::Records(source) => source.indexes.query(&source.events, query),
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
            EvidenceSource::Records(source) => {
                if !(1..=MAX_QUERY_PAGE_EVENTS).contains(&limit) || after > through {
                    return Err(GlobalLedgerError::request(
                        "invalid_query_page",
                        "query_read_only_event_page",
                    ));
                }
                Ok(source
                    .indexes
                    .query_page(&source.events, query, after, through, limit))
            }
        }
    }
    /// The sequence of the last event read. For an `sqlite_prefix` opening this is the prefix
    /// end, not the source's head; use `read_extent().head_sequence` for the head.
    pub fn latest_sequence(&self) -> u64 {
        self.events().last().map_or(0, PersistedEvent::sequence)
    }

    pub fn segment(&self) -> Option<&GlobalLedgerReadOnly> {
        match &self.source {
            EvidenceSource::Segment(source) => Some(source),
            EvidenceSource::Sqlite(_) | EvidenceSource::Records(_) => None,
        }
    }
    pub fn backend(&self) -> &'static str {
        match self.source {
            EvidenceSource::Segment(_) => "segment",
            EvidenceSource::Sqlite(_) | EvidenceSource::Records(_) => "sqlite",
        }
    }
    /// False only for an SQLite opening with `sqlite_material_not_read`, whose artifacts
    /// were never verified. Otherwise every artifact without an eviction proof is either
    /// Available or, with `sqlite_material_per_artifact`, Unrecorded after a verifier `None`.
    pub fn material_checked(&self) -> bool {
        match &self.source {
            EvidenceSource::Segment(_) | EvidenceSource::Sqlite(_) => true,
            EvidenceSource::Records(source) => source.material_checked,
        }
    }
    /// The extent of this opening: an SQLite prefix read stops at its declared prefix
    /// below the authenticated head; every other opening reads through the head.
    pub fn read_extent(&self) -> GlobalLedgerReadExtent {
        match &self.source {
            EvidenceSource::Records(source) => source.extent,
            EvidenceSource::Segment(source) => GlobalLedgerReadExtent {
                through_sequence: source.latest_sequence(),
                head_sequence: source.latest_sequence(),
                event_count: source.events().len(),
                ledger_bytes: Some(source.storage_snapshot().read_bytes),
                phases: None,
            },
            EvidenceSource::Sqlite(source) => GlobalLedgerReadExtent {
                through_sequence: source.latest_sequence(),
                head_sequence: source.latest_sequence(),
                event_count: source.events().len(),
                ledger_bytes: None,
                phases: None,
            },
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
/// The events of some event types from `GlobalLedger::open_selected` (Workflow #375 R375-3),
/// each authenticated on its own. Rows of other types were never read.
pub struct GlobalLedgerSelection {
    types: Vec<EventType>,
    events: Vec<PersistedEvent>,
    indexes: projection::EventIndexes,
    head_sequence: u64,
    complete: bool,
    writer: GlobalLedgerWriterMetadataObservation,
}
impl GlobalLedgerSelection {
    /// The authenticated head when opened (not the last selected event).
    pub fn head_sequence(&self) -> u64 {
        self.head_sequence
    }
    /// `query.event_type` must be one of the selected types and `query.view` must be absent;
    /// otherwise the request error `ledger_selection_query_unsupported` (never an empty or
    /// partial result). A view is refused because its Lab relation is decided from events of
    /// other types.
    pub fn query(&self, query: &EventQuery) -> GlobalLedgerResult<Vec<PersistedEvent>> {
        let selected = query
            .event_type
            .is_some_and(|event_type| self.types.contains(&event_type));
        if !selected || query.view.is_some() {
            return Err(GlobalLedgerError::request(
                LedgerCode::SelectionQueryUnsupported.as_str(),
                LedgerLocation::QuerySelection.as_str(),
            ));
        }
        Ok(self.indexes.query(&self.events, query))
    }
    /// False only for a segment root with an incomplete read or a corrupt tail.
    pub fn is_complete(&self) -> bool {
        self.complete
    }
    pub fn writer_metadata(&self) -> &GlobalLedgerWriterMetadataObservation {
        &self.writer
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
    /// Its projected views verify through the head, so `sqlite_prefix` is refused.
    pub fn open_metadata(
        config: GlobalLedgerEvidenceConfig,
    ) -> GlobalLedgerResult<GlobalLedgerMetadata> {
        if config.prefix.is_some() {
            return Err(GlobalLedgerError::request(
                "ledger_prefix_unsupported",
                "open_ledger_metadata",
            ));
        }
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
                let (events, sqlite, _) =
                    sqlite::open_metadata(Arc::new(database), config.budget, None)?;
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
        // Workflow #363: a prefix leaves later material facts unread, so it needs a record path.
        if config.prefix.is_some() && config.material == EvidenceMaterial::Required {
            return Err(GlobalLedgerError::request(
                "invalid_evidence_prefix",
                "open_runtime_evidence",
            ));
        }
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
                if config.material == EvidenceMaterial::Required {
                    let source =
                        SqliteLedgerReadOnly::open_formal(&database, config.budget, &mut verifier)?;
                    let writer = read_only::read_writer_metadata(&ledger_root)?;
                    return Ok(GlobalLedgerEvidence {
                        source: EvidenceSource::Sqlite(Box::new(source)),
                        writer,
                    });
                }
                // The same record authentication, ready marker and eviction annotation as
                // `open_metadata`; material is then restored one artifact at a time. With a
                // prefix, records, retention and availability stop at the prefix.
                let (metadata, _view, read) =
                    sqlite::open_metadata(Arc::new(database), config.budget, config.prefix)?;
                let restore_started = Instant::now();
                let mut check = |count| read_only::check_read_budget(config.budget, 0, count);
                let retention =
                    retention::RetentionIndex::from_events_checked(&metadata, &mut check)?;
                let material_checked = config.material == EvidenceMaterial::PerArtifact;
                let mut material_verifier = material_checked.then_some(&mut verifier);
                let mut events = Vec::with_capacity(metadata.len());
                for event in metadata {
                    check(events.len() + 1)?;
                    events.push(retention.restore_metadata(event, &mut material_verifier)?);
                }
                let extent = GlobalLedgerReadExtent {
                    through_sequence: read.through_sequence,
                    head_sequence: read.head_sequence,
                    event_count: events.len(),
                    ledger_bytes: Some(read.bytes),
                    phases: Some(GlobalLedgerReadPhases {
                        sql_read: read.sql_read,
                        verify: read.verify,
                        retention_restore: read.annotate + restore_started.elapsed(),
                    }),
                };
                let writer = read_only::read_writer_metadata(&ledger_root)?;
                return Ok(GlobalLedgerEvidence {
                    source: EvidenceSource::Records(Box::new(RecordEvidence {
                        indexes: projection::EventIndexes::from_events(&events),
                        events,
                        material_checked,
                        extent,
                    })),
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

    /// Workflow #375 R375-3: the events of `event_types`, each authenticated on its own,
    /// with the keyed head row and sequence contiguity. Rows of other types are never read,
    /// so this opening does not establish their integrity; artifacts are restored
    /// Unrecorded and no retention state is derived. A segment root is read whole, as
    /// `open_evidence` with `sqlite_material_not_read`.
    pub fn open_selected(
        root: impl Into<PathBuf>,
        event_types: &[EventType],
        deadline: Instant,
    ) -> GlobalLedgerResult<GlobalLedgerSelection> {
        let root = root.into();
        let budget = Some((u64::MAX, usize::MAX, deadline));
        let ledger_root = root.join("ledger");
        let database_exists = root
            .join(actingcommand_runtime_database::DATABASE_FILE)
            .try_exists()
            .map_err(|error| {
                GlobalLedgerError::io("ledger_io", "inspect_evidence_database", &error)
            })?;
        let key_exists = root
            .join(actingcommand_runtime_database::INTEGRITY_KEY_FILE)
            .try_exists()
            .map_err(|error| GlobalLedgerError::io("ledger_io", "inspect_evidence_key", &error))?;
        if database_exists || key_exists {
            let database = RuntimeDatabase::open_existing(&root, true)?;
            if sqlite::has_schema(&database)? {
                let read = sqlite::read_selected(&database, event_types, budget)?;
                let mut events = Vec::with_capacity(read.events.len());
                for event in read.events {
                    read_only::check_read_budget(budget, 0, events.len() + 1)?;
                    events.push(
                        event
                            .into_event_with_artifact_availability(&mut |_| Ok(None))
                            .map_err(|error| {
                                GlobalLedgerError::fatal(error.code(), "validate_persisted_event")
                            })?,
                    );
                }
                read_only::check_read_budget(budget, 0, events.len())?;
                let writer = read_only::read_writer_metadata(&ledger_root)?;
                return Ok(GlobalLedgerSelection {
                    types: event_types.to_vec(),
                    indexes: projection::EventIndexes::from_events(&events),
                    events,
                    head_sequence: read.head_sequence,
                    complete: true,
                    writer,
                });
            }
        }
        let source = Self::open_evidence(
            GlobalLedgerEvidenceConfig::new(root)
                .sqlite_material_not_read()
                .with_deadline(deadline),
            |_| None,
        )?;
        let events = source
            .events()
            .iter()
            .filter(|event| event_types.contains(&event.event_type()))
            .cloned()
            .collect::<Vec<_>>();
        Ok(GlobalLedgerSelection {
            types: event_types.to_vec(),
            indexes: projection::EventIndexes::from_events(&events),
            events,
            head_sequence: source.latest_sequence(),
            complete: source.is_complete(),
            writer: source.writer_metadata().clone(),
        })
    }
}
