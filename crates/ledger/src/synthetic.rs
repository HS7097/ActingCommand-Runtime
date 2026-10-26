// SPDX-License-Identifier: AGPL-3.0-only

//! LN-1 (Workflow #422, LEAN-L model §7.2): a deterministic synthetic ledger for the scale
//! harness, and the process measures the harness reads. Test hooks only.
//!
//! [`write_synthetic_ledger`] appends `records` records to a closed formal root, straight into its
//! tables, one transaction per batch (`SyntheticAppender`); no list of the history is held in
//! memory, so it writes a million records as easily as ten thousand. The mix follows production's
//! (#407: most records come from screenshots):
//! - runs of a contained task, each requested by a command (`command.received`,
//!   `command.validated`), admitted and started (`task.requested`, `task.started`), and ended by a
//!   terminal intent and its terminal, which share an action id, so the intent's `open` index row
//!   closes with its result;
//! - in every run, frames, each a capture and recognition cycle with its task facts
//!   (`capture.requested`, `capture.completed`, `task.evidence_indexed`,
//!   `recognition.requested`, `task.recognition_started`, `recognition.completed`,
//!   `task.recognition_completed`);
//! - every few frames a step whose effect is an input (`task.step_started`, `task.effect_intent`,
//!   `input.intent`, `input.committed`, `task.effect_completed`, `task.step_finished`); the
//!   commit moves the instance's input boundary row;
//! - now and then an instance fact (`fact.published`, tagged `instance_facts/...`) and a
//!   workbench record (`runtime.fact_recorded`, tagged when its key matches the default
//!   `tag.workbench_patterns`, as each run's `task.<label>.last_duration_ms` does).
//!
//! The tagged keys, the task labels, the pages and the instances are the same at every size;
//! only the history's length follows `records`.
//!
//! Deterministic: the same spec writes the same canonical records. Identifiers come from one
//! issuer in a fixed order, and its namespace is replaced by a fixed one; times start at a fixed
//! instant and advance by a fixed interval; every label is a neutral fixture word. Only the keyed
//! integrity tags differ between roots, since each root has its own key.
//!
//! #422 LN-8d: with `frame_artifact_every_frames` set, every that many frames of a run the
//! capture's frame is stored through the artifact store of the root
//! (`ArtifactStore::put`, `artifacts/<shard>/artifact_<hex>.png`), as the capture store stores a
//! frame: its `artifact.created` and `artifact.verified` records carry the frame's `capture.frame`
//! reference, linked to its run, frame and correlation, and the frame stays in the root's live
//! frame store, as production's frame store holds frames from the whole history. The store issues
//! the references, so their artifact identifiers (and the object keys that name them) are the
//! store's and differ between roots; the records' own identifiers come from the generator.
//!
//! Not in the mix: policy dispatch records (the start's dispatch recovery needs their catalog
//! generation in the state store), the decision memory's value records (LN-3 writes them),
//! other artifact references, workbench snapshot sets and lifecycle records (each start writes
//! its own).

use crate::PersistedEvent;
use crate::codes::{LedgerCode, LedgerLocation};
use crate::fact::StoredEventRecord;
use crate::global::{
    AppendedBatch, GlobalLedgerError, GlobalLedgerResult, LedgerMaintenance,
    Sha256SecretFingerprinter, SyntheticAppender,
};
use actingcommand_artifact_store::{
    ArtifactCapacityAdmission, ArtifactEventSink, ArtifactStore, ArtifactStoreError,
    ArtifactStoreResult, ArtifactWriteContext, ArtifactWriteRequest,
};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Code, Location};
use actingcommand_contract::page_projection::Rect;
use actingcommand_contract::{
    ArtifactIssuePolicy, ArtifactKind, ArtifactLinksDraft, ArtifactProducer,
    ArtifactRedactionState, AuditInput, CapacityAdmissionOutcome, CapacityAdmissionReason,
    CapacityDecision, CapturePayloadDraft, CommandPayloadDraft, EffectDisposition, EventAction,
    EventActor, EventDraft, EventId, EventLinksDraft, EventOrigin, EventPayloadDraft,
    EventSeverity, EventSource, FactContent, FactPayloadDraft, FactRecord, FactScope, FactValue,
    IdentifierIssuer, InputAction, InputPayloadDraft, InstanceId, IssuedCorrelationId,
    IssuedFrameId, IssuedInstanceId, IssuedRunId, OriginModule, OwnerEpoch,
    RecognitionCandidateScore, RecognitionCandidateScores, RecognitionPayloadDraft,
    RecognitionVerdict, RecognizedTarget, RecognizedTargetRole, RetentionClass, RuntimeFactRecord,
    RuntimeFactScope, RuntimePayloadDraft, TASK_RECOGNITION_CANDIDATE_SCORE_LIMIT,
    TASK_RECOGNITION_TARGET_LIMIT, TaskOutcome, TaskPayloadDraft, TaskSemanticFact,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

/// The issuer namespace every synthetic identifier carries: `SYNTHETI` in hex.
const FIXED_NAMESPACE: &str = "53594e5448455449";
/// The salt of the generator's secret fingerprints (its records carry no secret field).
const FINGERPRINT_SALT: &[u8] = b"ln1-synthetic-ledger";
/// The frame size every capture and recognition record names.
const FRAME_WIDTH: u32 = 1_280;
const FRAME_HEIGHT: u32 = 720;
/// The records a run needs besides its frames: command, admission and start; terminal intent,
/// terminal and the run's duration record.
const RUN_OPEN_RECORDS: u64 = 4;
const RUN_CLOSE_RECORDS: u64 = 3;
/// The most records one frame writes: its cycle, a step, an instance fact and a workbench record.
const FRAME_MAX_RECORDS: u64 = 7 + 6 + 1 + 1;
/// LN-8d: the records a stored frame adds (`artifact.created`, `artifact.verified`).
const FRAME_ARTIFACT_RECORDS: u64 = 2;

/// One instance of the synthetic history: the alias its instance facts name and its identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticInstance {
    pub alias: String,
    pub instance_id: InstanceId,
}

/// The shape of the history; it does not depend on its length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticMix {
    /// Distinct task labels; with the instances, the dispatch pairs of the history.
    pub task_labels: u64,
    /// Candidate pages of each task, all listed in each recognition.
    pub pages_per_task: u64,
    /// Targets recorded for the matched page of each recognition.
    pub targets_per_page: u64,
    /// Frames of a run: from `frames_per_run_min` to `frames_per_run_max`, by run.
    pub frames_per_run_min: u64,
    pub frames_per_run_max: u64,
    /// A step with an input every this many frames.
    pub step_every_frames: u64,
    /// An instance fact every this many frames.
    pub fact_every_frames: u64,
    /// A workbench record every this many frames.
    pub workbench_every_frames: u64,
    /// Distinct instance fact keys of each instance.
    pub fact_keys: u64,
    /// #422 LN-8d: a capture frame stored in the root's frame store every this many frames of a
    /// run; 0 stores none.
    pub frame_artifact_every_frames: u64,
}

impl Default for SyntheticMix {
    fn default() -> Self {
        Self {
            task_labels: 12,
            pages_per_task: 12,
            targets_per_page: 6,
            frames_per_run_min: 32,
            frames_per_run_max: 64,
            step_every_frames: 4,
            fact_every_frames: 8,
            workbench_every_frames: 16,
            fact_keys: 24,
            frame_artifact_every_frames: 0,
        }
    }
}

/// What [`write_synthetic_ledger`] writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticLedgerSpec {
    /// The history's length in records; the root gets exactly this many.
    pub records: u64,
    /// The instances the runs, inputs and facts name, in turn.
    pub instances: Vec<SyntheticInstance>,
    /// Records per transaction.
    pub batch_records: usize,
    /// The first record's time; each later record is `record_interval_ms` later.
    pub first_timestamp_unix_ms: u64,
    pub record_interval_ms: u64,
    pub mix: SyntheticMix,
}

impl SyntheticLedgerSpec {
    /// `records` records for `instances`: batches of 10,000 records, from 2025-06-15 00:00 UTC
    /// (1,749,945,600,000 ms), one record every 500 ms (about 170,000 a day, production's rate),
    /// in the default mix.
    pub fn new(records: u64, instances: Vec<SyntheticInstance>) -> Self {
        Self {
            records,
            instances,
            batch_records: 10_000,
            first_timestamp_unix_ms: 1_749_945_600_000,
            record_interval_ms: 500,
            mix: SyntheticMix::default(),
        }
    }

    fn validate(&self) -> GlobalLedgerResult<()> {
        let mix = &self.mix;
        let invalid = [
            (self.instances.is_empty(), "instances"),
            (self.batch_records == 0, "batch_records"),
            (self.first_timestamp_unix_ms == 0, "first_timestamp_unix_ms"),
            (self.record_interval_ms == 0, "record_interval_ms"),
            (mix.task_labels == 0, "task_labels"),
            (
                mix.pages_per_task == 0 || mix.pages_per_task > 1_024,
                "pages_per_task",
            ),
            (
                mix.targets_per_page > TASK_RECOGNITION_TARGET_LIMIT as u64,
                "targets_per_page",
            ),
            (
                mix.frames_per_run_min == 0 || mix.frames_per_run_max < mix.frames_per_run_min,
                "frames_per_run",
            ),
            (mix.step_every_frames == 0, "step_every_frames"),
            (mix.fact_every_frames == 0, "fact_every_frames"),
            (mix.workbench_every_frames == 0, "workbench_every_frames"),
            (mix.fact_keys == 0, "fact_keys"),
        ];
        match invalid.iter().find(|(refused, _)| *refused) {
            Some((_, field)) => Err(GlobalLedgerError::request(
                LedgerCode::InvalidLedgerConfig.code(),
                LedgerLocation::ValidateRoot.location(),
            )
            .with_detail(format!("synthetic ledger spec: {field}"))),
            None => Ok(()),
        }
    }
}

/// What [`write_synthetic_ledger`] wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyntheticLedgerReport {
    pub records: u64,
    pub first_sequence: u64,
    pub head_sequence: u64,
    pub batches: u64,
    /// The bytes of the canonical records.
    pub canonical_bytes: u64,
    /// SHA-256 over the records' `record_sha256` values in sequence order: equal specs written
    /// from the same head give equal values (LN-8d: when the mix stores no frame).
    pub content_sha256: String,
    /// Records per event type.
    pub record_types: BTreeMap<String, u64>,
    pub runs: u64,
    pub frames: u64,
    pub steps: u64,
    pub instance_facts: u64,
    pub workbench_records: u64,
    /// #422 LN-8d: the capture frames stored in the root's frame store.
    pub stored_frames: u64,
    pub first_timestamp_unix_ms: u64,
    pub last_timestamp_unix_ms: u64,
    /// The generator's own time (not deterministic).
    pub elapsed_ms: u64,
}

/// Appends a synthetic history of `spec.records` records to the closed formal root of
/// `database` (a root as acsetup leaves it, or one a Runtime closed), opened by the caller. The
/// root's writer lock is held while it writes; every batch is authenticated as the writer's own
/// batches are.
pub fn write_synthetic_ledger(
    database: &RuntimeDatabase,
    spec: &SyntheticLedgerSpec,
) -> GlobalLedgerResult<SyntheticLedgerReport> {
    spec.validate()?;
    let started = Instant::now();
    let limits = MaintenanceLimits::default();
    let maintenance =
        LedgerMaintenance::acquire(database.root(), false, limits, limits.deadline()?)?;
    let written: GlobalLedgerResult<SyntheticLedgerReport> = (|| {
        if !maintenance.formal_ready(database)? {
            return Err(GlobalLedgerError::fatal(
                SharedCode::LedgerMigrationRequired.code(),
                LedgerLocation::OpenRuntimeLedger.location(),
            ));
        }
        // LN-8d: the root's artifact store, when the mix stores frames.
        let frame_store = if spec.mix.frame_artifact_every_frames == 0 {
            None
        } else {
            Some(open_frame_store(
                database.root(),
                spec.first_timestamp_unix_ms,
            )?)
        };
        let mut generator = Generator::new(spec, SyntheticAppender::open(database)?, frame_store)?;
        generator.write()?;
        generator.report(started)
    })();
    match (written, maintenance.close()) {
        (Ok(report), Ok(())) => Ok(report),
        (Ok(_), Err(error)) | (Err(error), Ok(())) => Err(error),
        // The write's failure stays the error; the close's failure joins its detail.
        (Err(error), Err(close)) => {
            let detail = format!(
                "{}; close failure {} during {}",
                error.detail().unwrap_or("-"),
                close.code(),
                close.operation()
            );
            Err(error.with_detail(detail))
        }
    }
}

/// The run's identifiers and labels.
struct Run {
    index: u64,
    instance: IssuedInstanceId,
    alias: String,
    task_label: String,
    pages: Vec<String>,
    /// The run's own links: instance, request, correlation, task, run and lease.
    links: EventLinksDraft,
    /// The command's links: instance, request and correlation.
    request_links: EventLinksDraft,
    /// LN-8d: the run and correlation a stored frame's reference names.
    run_id: IssuedRunId,
    correlation: IssuedCorrelationId,
    steps: u32,
    page: usize,
}

struct Generator<'a> {
    spec: &'a SyntheticLedgerSpec,
    issuer: IdentifierIssuer,
    /// `_` and the issuer's namespace, as it appears in every identifier the issuer mints.
    issuer_namespace: String,
    /// LN-8d: the root's artifact store, which stores the frames the mix stores.
    frame_store: Option<ArtifactStore>,
    fingerprinter: Sha256SecretFingerprinter,
    appender: SyntheticAppender<'a>,
    first_sequence: u64,
    next_sequence: u64,
    written: u64,
    batch: Vec<PersistedEvent>,
    batches: u64,
    digest: Sha256,
    canonical_bytes: u64,
    record_types: BTreeMap<String, u64>,
    runs: u64,
    frames: u64,
    steps: u64,
    instance_facts: u64,
    workbench_records: u64,
    stored_frames: u64,
    last_timestamp_unix_ms: u64,
}

impl<'a> Generator<'a> {
    fn new(
        spec: &'a SyntheticLedgerSpec,
        appender: SyntheticAppender<'a>,
        frame_store: Option<ArtifactStore>,
    ) -> GlobalLedgerResult<Self> {
        let issuer = IdentifierIssuer::new().map_err(|error| issue_failure(error.code()))?;
        // The first identifier names the issuer's namespace: the 16 hex digits after `evt_`.
        let probe = issuer
            .mint_event_id()
            .map_err(|error| issue_failure(error.code()))?;
        let probe = serde_json::to_value(probe.transport()).map_err(|error| {
            json_failure(
                LedgerCode::EventSerializationFailed.code(),
                LedgerLocation::SerializeEvent.location(),
                &error,
            )
        })?;
        let namespace = probe
            .as_str()
            .and_then(|text| text.strip_prefix("evt_"))
            .and_then(|hex| hex.get(..16))
            .ok_or_else(|| {
                record_failure(
                    LedgerCode::InvalidEventLink.code(),
                    LedgerLocation::SerializeEvent.location(),
                    "issuer namespace".to_owned(),
                )
            })?;
        let first_sequence = appender.next_sequence()?;
        Ok(Self {
            spec,
            issuer_namespace: format!("_{namespace}"),
            issuer,
            frame_store,
            fingerprinter: Sha256SecretFingerprinter::new(FINGERPRINT_SALT)?,
            appender,
            first_sequence,
            next_sequence: first_sequence,
            written: 0,
            batch: Vec::with_capacity(spec.batch_records),
            batches: 0,
            digest: Sha256::new(),
            canonical_bytes: 0,
            record_types: BTreeMap::new(),
            runs: 0,
            frames: 0,
            steps: 0,
            instance_facts: 0,
            workbench_records: 0,
            stored_frames: 0,
            last_timestamp_unix_ms: 0,
        })
    }

    fn remaining(&self) -> u64 {
        self.spec.records - self.written
    }

    /// The most records one frame writes (LN-8d: with its stored frame when the mix stores some).
    fn frame_max_records(&self) -> u64 {
        if self.spec.mix.frame_artifact_every_frames == 0 {
            FRAME_MAX_RECORDS
        } else {
            FRAME_MAX_RECORDS + FRAME_ARTIFACT_RECORDS
        }
    }

    /// Runs while a whole run with one frame fits; the last few records are workbench records.
    fn write(&mut self) -> GlobalLedgerResult<()> {
        let mut run = 0;
        while self.remaining() > 0 {
            if self.remaining() >= RUN_OPEN_RECORDS + self.frame_max_records() + RUN_CLOSE_RECORDS {
                self.run(run)?;
                run += 1;
            } else {
                self.filler()?;
            }
        }
        self.flush()
    }

    fn report(mut self, started: Instant) -> GlobalLedgerResult<SyntheticLedgerReport> {
        self.flush()?;
        let digest = std::mem::take(&mut self.digest);
        Ok(SyntheticLedgerReport {
            records: self.written,
            first_sequence: self.first_sequence,
            head_sequence: self.next_sequence - 1,
            batches: self.batches,
            canonical_bytes: self.canonical_bytes,
            content_sha256: format!("sha256:{:x}", digest.finalize()),
            record_types: self.record_types,
            runs: self.runs,
            frames: self.frames,
            steps: self.steps,
            instance_facts: self.instance_facts,
            workbench_records: self.workbench_records,
            stored_frames: self.stored_frames,
            first_timestamp_unix_ms: self.spec.first_timestamp_unix_ms,
            last_timestamp_unix_ms: self.last_timestamp_unix_ms,
            elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }

    /// The next record's time.
    fn timestamp(&self) -> GlobalLedgerResult<u64> {
        self.spec
            .record_interval_ms
            .checked_mul(self.written)
            .and_then(|offset| self.spec.first_timestamp_unix_ms.checked_add(offset))
            .ok_or_else(|| {
                GlobalLedgerError::request(
                    LedgerCode::InvalidLedgerConfig.code(),
                    LedgerLocation::ValidateRoot.location(),
                )
                .with_detail("synthetic ledger spec: timestamps overflow".to_owned())
            })
    }

    fn run(&mut self, index: u64) -> GlobalLedgerResult<()> {
        let spec = self.spec;
        let mix = &spec.mix;
        let instance_count = spec.instances.len() as u64;
        let instance = &spec.instances[usize::try_from(index % instance_count).unwrap_or(0)];
        let alias = instance.alias.clone();
        let issued_instance = self.issuer.issue_registered_instance(instance.instance_id);
        let task = index % mix.task_labels;
        let task_label = format!("synthetic-task-{task:02}");
        let pages = (0..mix.pages_per_task)
            .map(|page| format!("synthetic-page-{task:02}-{page:02}"))
            .collect::<Vec<_>>();
        let request = self
            .issuer
            .mint_request_id()
            .map_err(|error| issue_failure(error.code()))?;
        let correlation = self
            .issuer
            .mint_correlation_id()
            .map_err(|error| issue_failure(error.code()))?;
        let task_id = self
            .issuer
            .mint_task_id()
            .map_err(|error| issue_failure(error.code()))?;
        let run_id = self
            .issuer
            .mint_run_id()
            .map_err(|error| issue_failure(error.code()))?;
        let lease = self
            .issuer
            .mint_lease_id()
            .map_err(|error| issue_failure(error.code()))?;
        let request_links = EventLinksDraft::default()
            .with_instance_id(issued_instance)
            .with_request_id(request)
            .with_correlation_id(correlation);
        let links = request_links
            .clone()
            .with_task_id(task_id)
            .with_run_id(run_id)
            .with_lease_id(lease);
        let mut run = Run {
            index,
            instance: issued_instance,
            alias,
            task_label,
            pages,
            links,
            request_links,
            run_id,
            correlation,
            steps: 0,
            page: 0,
        };
        self.runs += 1;
        let runtime = (EventSource::Runtime, OriginModule::Runtime);
        self.push(
            runtime,
            run.request_links.clone(),
            CommandPayloadDraft::received(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
        )?;
        self.push(
            runtime,
            run.request_links.clone(),
            CommandPayloadDraft::validated(
                EventAction::RuntimeTaskRun,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            )
            .into(),
        )?;
        self.push(
            runtime,
            run.links.clone(),
            TaskPayloadDraft::semantic(
                TaskSemanticFact::PackageAdmitted {
                    package_label: format!("synthetic-pack-{task:02}"),
                    task_label: run.task_label.clone(),
                    response_deadline_monotonic_ms: None,
                },
                AuditInput::new(),
            )
            .into(),
        )?;
        self.push(
            runtime,
            run.links.clone(),
            TaskPayloadDraft::semantic(TaskSemanticFact::RunStarted, AuditInput::new()).into(),
        )?;
        let frames = mix.frames_per_run_min
            + (index * 7) % (mix.frames_per_run_max - mix.frames_per_run_min + 1);
        for frame in 0..frames {
            if self.remaining() < self.frame_max_records() + RUN_CLOSE_RECORDS {
                break;
            }
            self.frame(&mut run, frame)?;
        }
        self.close(&run)
    }

    fn frame(&mut self, run: &mut Run, frame: u64) -> GlobalLedgerResult<()> {
        let frame_id = self
            .issuer
            .mint_frame_id()
            .map_err(|error| issue_failure(error.code()))?;
        let recognition = self
            .issuer
            .mint_recognition_id()
            .map_err(|error| issue_failure(error.code()))?;
        let frame_links = run.links.clone().with_frame_id(frame_id);
        let recognition_links = frame_links.clone().with_recognition_id(recognition);
        let page_count = run.pages.len();
        run.page = usize::try_from((frame + run.index) % page_count as u64).unwrap_or(0);
        let matched = run.pages[run.page].clone();
        let runtime = (EventSource::Runtime, OriginModule::Runtime);
        let recognizer = (EventSource::Runtime, OriginModule::Recognition);
        self.frames += 1;
        self.push(
            (EventSource::Device, OriginModule::Capture),
            frame_links.clone(),
            CapturePayloadDraft::requested(EventAction::CaptureObserve, AuditInput::new()).into(),
        )?;
        self.push(
            (EventSource::Device, OriginModule::Capture),
            frame_links.clone(),
            CapturePayloadDraft::completed_with_capture_acquire(
                EventAction::CaptureObserve,
                EffectDisposition::NotPerformed,
                FRAME_WIDTH,
                FRAME_HEIGHT,
                Some(18_000 + (frame * 37) % 9_000),
                AuditInput::new(),
            )
            .into(),
        )?;
        if stores_frame(self.spec, frame) {
            self.stored_frame(run, frame_id, &frame_links)?;
        }
        self.push(
            runtime,
            frame_links.clone(),
            TaskPayloadDraft::semantic(
                TaskSemanticFact::EvidenceIndexed {
                    frame_width: FRAME_WIDTH,
                    frame_height: FRAME_HEIGHT,
                },
                AuditInput::new(),
            )
            .into(),
        )?;
        self.push(
            recognizer,
            recognition_links.clone(),
            RecognitionPayloadDraft::requested(EventAction::RecognitionObserve, AuditInput::new())
                .into(),
        )?;
        self.push(
            runtime,
            recognition_links.clone(),
            TaskPayloadDraft::semantic(
                TaskSemanticFact::RecognitionStarted {
                    candidate_pages: run.pages.clone(),
                    frame_width: FRAME_WIDTH,
                    frame_height: FRAME_HEIGHT,
                },
                AuditInput::new(),
            )
            .into(),
        )?;
        self.push(
            recognizer,
            recognition_links.clone(),
            RecognitionPayloadDraft::completed(
                EventAction::RecognitionObserve,
                EffectDisposition::NotPerformed,
                FRAME_WIDTH,
                FRAME_HEIGHT,
                RecognitionVerdict::PageMatched,
                AuditInput::new(),
            )
            .into(),
        )?;
        let spec = self.spec;
        let targets = targets(run.page, spec.mix.targets_per_page);
        let scores = candidate_scores(&run.pages, &matched);
        self.push(
            runtime,
            recognition_links,
            TaskPayloadDraft::semantic(
                TaskSemanticFact::RecognitionCompleted {
                    candidate_pages: run.pages.clone(),
                    matched_page: Some(matched.clone()),
                    frame_width: FRAME_WIDTH,
                    frame_height: FRAME_HEIGHT,
                    targets,
                    candidate_scores: Some(Box::new(scores)),
                },
                AuditInput::new(),
            )
            .into(),
        )?;
        let mix = &spec.mix;
        let step = frame % mix.step_every_frames == mix.step_every_frames - 1;
        let fact = frame % mix.fact_every_frames == mix.fact_every_frames - 1;
        let workbench = frame % mix.workbench_every_frames == mix.workbench_every_frames - 1;
        if step {
            self.step(run, frame, &frame_links, frame_id)?;
        }
        if fact {
            self.instance_fact(run, frame)?;
        }
        if workbench {
            let key = format!("task.{}.attempts", run.task_label);
            let value = i64::try_from(frame + 1).unwrap_or(i64::MAX);
            self.workbench(
                RuntimeFactScope::Instance {
                    instance_id: *run.instance.transport(),
                },
                key,
                value,
            )?;
        }
        Ok(())
    }

    fn step(
        &mut self,
        run: &mut Run,
        frame: u64,
        frame_links: &EventLinksDraft,
        frame_id: actingcommand_contract::IssuedFrameId,
    ) -> GlobalLedgerResult<()> {
        let step_index = run.steps;
        let operation_label = format!("synthetic-operation-{:02}", step_index % 8);
        let from_page = run.pages[run.page].clone();
        let to_page = run.pages[(run.page + 1) % run.pages.len()].clone();
        let action = InputAction::Tap {
            x: i32::try_from(100 + (frame * 53) % 1_000).unwrap_or(100),
            y: i32::try_from(80 + (frame * 31) % 560).unwrap_or(80),
        };
        let input = self
            .issuer
            .mint_action_id()
            .map_err(|error| issue_failure(error.code()))?;
        let input_links = frame_links.clone().with_action_id(input);
        let runtime = (EventSource::Runtime, OriginModule::Runtime);
        let device = (EventSource::Device, OriginModule::DeviceProxy);
        self.steps += 1;
        self.push(
            runtime,
            frame_links.clone(),
            TaskPayloadDraft::semantic(
                TaskSemanticFact::StepStarted {
                    step_index,
                    operation_label: operation_label.clone(),
                    from_page,
                    phase: None,
                },
                AuditInput::new(),
            )
            .into(),
        )?;
        self.push(
            runtime,
            frame_links.clone(),
            TaskPayloadDraft::semantic(
                TaskSemanticFact::EffectIntent {
                    step_index,
                    operation_label: operation_label.clone(),
                    action: action.clone(),
                    frame_extent: None,
                },
                AuditInput::new(),
            )
            .into(),
        )?;
        self.push(
            device,
            input_links.clone(),
            InputPayloadDraft::intent_with_provenance(
                action,
                None,
                None,
                Some(*frame_id.transport()),
                AuditInput::new(),
            )
            .into(),
        )?;
        self.push(
            device,
            input_links,
            InputPayloadDraft::committed_with_touch_response(
                EventAction::InputTap,
                EffectDisposition::Performed,
                Some(4_000 + (frame * 11) % 3_000),
                AuditInput::new(),
            )
            .into(),
        )?;
        self.push(
            runtime,
            frame_links.clone(),
            TaskPayloadDraft::semantic(
                TaskSemanticFact::EffectCompleted {
                    step_index,
                    operation_label: operation_label.clone(),
                },
                AuditInput::new(),
            )
            .into(),
        )?;
        self.push(
            runtime,
            frame_links.clone(),
            TaskPayloadDraft::semantic(
                TaskSemanticFact::StepFinished {
                    step_index,
                    operation_label,
                    page_label: to_page,
                    phase: None,
                },
                AuditInput::new(),
            )
            .into(),
        )?;
        run.steps += 1;
        Ok(())
    }

    /// LN-8d: the frame stored as the capture store stores one: the root's artifact store writes
    /// its material (a few fixture bytes) at its object key and gives its `capture.frame`
    /// reference (run, frame and correlation links) in `artifact.created` and `artifact.verified`
    /// with the frame's links. Each record gets the generator's next event id, as every record
    /// does.
    fn stored_frame(
        &mut self,
        run: &Run,
        frame_id: IssuedFrameId,
        frame_links: &EventLinksDraft,
    ) -> GlobalLedgerResult<()> {
        let timestamp = self.timestamp()?;
        let frame = self.frames;
        let material = format!("synthetic-frame-{frame}");
        let Some(store) = self.frame_store.as_ref() else {
            return Err(record_failure(
                LedgerCode::InvalidLedgerConfig.code(),
                LedgerLocation::ValidateRoot.location(),
                format!("synthetic frame {frame}: the mix stores frames without a frame store"),
            ));
        };
        let mut drafts = FrameDrafts::default();
        store
            .put(
                ArtifactWriteRequest::new(
                    ArtifactKind::CaptureFrame,
                    material.as_bytes(),
                    ArtifactWriteContext::new(
                        ArtifactLinksDraft::default()
                            .with_run_id(run.run_id)
                            .with_frame_id(frame_id)
                            .with_correlation_id(run.correlation),
                        frame_links.clone(),
                        timestamp,
                    ),
                    ArtifactIssuePolicy::new(
                        ArtifactProducer::CaptureStore,
                        RetentionClass::DebugFull,
                        ArtifactRedactionState::NotRequired,
                    ),
                ),
                &mut drafts,
            )
            .map_err(|error| store_failure(&format!("synthetic frame {frame}"), &error))?;
        // The records the mix reserved for a stored frame (`frame_max_records`).
        if drafts.0.len() as u64 != FRAME_ARTIFACT_RECORDS {
            return Err(record_failure(
                SharedCode::ArtifactWriteFailed.code(),
                LedgerLocation::ValidateRoot.location(),
                format!(
                    "synthetic frame {frame}: {} artifact records, not {FRAME_ARTIFACT_RECORDS}",
                    drafts.0.len()
                ),
            ));
        }
        for draft in drafts.0 {
            let event_id = *self
                .issuer
                .mint_event_id()
                .map_err(|error| issue_failure(error.code()))?
                .transport();
            self.push_draft(draft, Some(event_id))?;
        }
        self.stored_frames += 1;
        Ok(())
    }

    /// One instance fact: a key of the instance's fixed set, observed at the record's time.
    fn instance_fact(&mut self, run: &Run, frame: u64) -> GlobalLedgerResult<()> {
        let key = (run.index * 5 + frame) % self.spec.mix.fact_keys;
        let observed_at_unix_ms = self.timestamp()?;
        let record = FactRecord {
            scope: FactScope::Instance {
                instance_id: run.alias.clone(),
            },
            key: format!("resource.synthetic_counter_{key:02}"),
            content: FactContent::Inline {
                value: FactValue::Integer(i64::try_from(run.index + frame).unwrap_or(i64::MAX)),
            },
            observed_at_unix_ms,
            expires_at_unix_ms: None,
            ttl_policy: None,
            confidence_milli: 1_000,
            source_detector: "synthetic-detector".to_owned(),
            source_snapshot_id: format!("snapshot:synthetic:{}", self.next_sequence),
            schema_version: "fact.v1".to_owned(),
            resource_bundle_hash: "5".repeat(64),
            invalidate_on: Vec::new(),
        };
        self.instance_facts += 1;
        self.push(
            (EventSource::Runtime, OriginModule::FactStore),
            run.request_links.clone(),
            FactPayloadDraft::published(record, AuditInput::new()).into(),
        )
    }

    /// One workbench record (`runtime.fact_recorded`), observed at the record's time.
    fn workbench(
        &mut self,
        scope: RuntimeFactScope,
        key: String,
        value: i64,
    ) -> GlobalLedgerResult<()> {
        let record = RuntimeFactRecord {
            scope,
            key,
            value: FactValue::Integer(value),
            observed_at_unix_ms: self.timestamp()?,
            source: OriginModule::Runtime,
            ttl_ms: None,
        };
        self.workbench_records += 1;
        self.push(
            (EventSource::Runtime, OriginModule::RuntimeFacts),
            EventLinksDraft::default(),
            RuntimePayloadDraft::fact_recorded(record, AuditInput::new()).into(),
        )
    }

    /// The run's terminal intent and terminal, which share an action id, and its duration.
    fn close(&mut self, run: &Run) -> GlobalLedgerResult<()> {
        let terminal = self
            .issuer
            .mint_action_id()
            .map_err(|error| issue_failure(error.code()))?;
        let terminal_links = run.links.clone().with_action_id(terminal);
        let runtime = (EventSource::Runtime, OriginModule::Runtime);
        self.push(
            runtime,
            terminal_links.clone(),
            TaskPayloadDraft::semantic(
                TaskSemanticFact::Finalizing {
                    task_result: TaskOutcome::Success,
                },
                AuditInput::new(),
            )
            .into(),
        )?;
        self.push(
            runtime,
            terminal_links,
            TaskPayloadDraft::semantic(
                TaskSemanticFact::TerminalCommitted {
                    task_result: TaskOutcome::Success,
                    final_page: Some(run.pages[run.page].clone()),
                    executed_steps: Some(run.steps),
                    outcome: None,
                    failure_severe: false,
                    scheduling_disposition: None,
                    task_timing: None,
                },
                AuditInput::new(),
            )
            .into(),
        )?;
        let duration_ms = i64::try_from(30_000 + (run.index * 7_919) % 600_000).unwrap_or(0);
        self.workbench(
            RuntimeFactScope::Instance {
                instance_id: *run.instance.transport(),
            },
            format!("task.{}.last_duration_ms", run.task_label),
            duration_ms,
        )
    }

    /// The history's last records, when no whole run fits any more.
    fn filler(&mut self) -> GlobalLedgerResult<()> {
        let value = i64::try_from(self.written).unwrap_or(i64::MAX);
        self.workbench(
            RuntimeFactScope::Runtime,
            "task.synthetic-filler.attempts".to_owned(),
            value,
        )
    }

    /// Builds one record as the Runtime's own drafts are built (an issued event id, a sanitized
    /// draft, the persisted record), then gives its identifiers the fixed namespace.
    fn push(
        &mut self,
        (source, module): (EventSource, OriginModule),
        links: EventLinksDraft,
        payload: EventPayloadDraft,
    ) -> GlobalLedgerResult<()> {
        let timestamp = self.timestamp()?;
        let event_id = self
            .issuer
            .mint_event_id()
            .map_err(|error| issue_failure(error.code()))?;
        let draft = EventDraft::new(
            event_id,
            timestamp,
            EventSeverity::Info,
            EventOrigin::new(source, module, EventActor::Runtime),
            links,
            payload,
        );
        self.push_draft(draft, None)
    }

    /// The record of `draft` (LN-8d: with `event_id` as its event id when given, for a draft the
    /// artifact store built), sanitized and persisted at the next sequence, its identifiers in
    /// the fixed namespace.
    fn push_draft(
        &mut self,
        draft: EventDraft,
        event_id: Option<EventId>,
    ) -> GlobalLedgerResult<()> {
        let draft = draft.sanitize(&self.fingerprinter).map_err(|error| {
            record_failure(
                error.code(),
                LedgerLocation::ValidateSanitizedEvent.location(),
                format!("synthetic record {}: {}", self.next_sequence, error.field()),
            )
        })?;
        let event = match event_id {
            Some(event_id) => {
                PersistedEvent::from_sanitized_with_event_id(self.next_sequence, draft, event_id)
            }
            None => PersistedEvent::from_sanitized(self.next_sequence, draft),
        }
        .map_err(|error| {
            record_failure(
                error.code(),
                LedgerLocation::ValidatePersistedEvent.location(),
                format!("synthetic record {}", self.next_sequence),
            )
        })?;
        let timestamp = event.timestamp_unix_ms();
        let event = self.with_fixed_namespace(&event)?;
        let event_type = serde_json::to_value(event.event_type())
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| {
                record_failure(
                    LedgerCode::EventSerializationFailed.code(),
                    LedgerLocation::SerializeEvent.location(),
                    format!("synthetic record {} type", self.next_sequence),
                )
            })?;
        *self.record_types.entry(event_type).or_default() += 1;
        self.batch.push(event);
        self.last_timestamp_unix_ms = timestamp;
        self.written += 1;
        self.next_sequence = self.next_sequence.checked_add(1).ok_or_else(|| {
            GlobalLedgerError::fatal(
                LedgerCode::SequenceExhausted.code(),
                LedgerLocation::IncrementSequence.location(),
            )
        })?;
        if self.batch.len() >= self.spec.batch_records {
            self.flush()?;
        }
        Ok(())
    }

    /// The record with every identifier of the generator's issuer in the fixed namespace; it is
    /// decoded and validated as a stored record is.
    fn with_fixed_namespace(&self, event: &PersistedEvent) -> GlobalLedgerResult<PersistedEvent> {
        let text =
            serde_json::to_string(&StoredEventRecord::from_event(event)).map_err(|error| {
                json_failure(
                    LedgerCode::EventSerializationFailed.code(),
                    LedgerLocation::SerializeEvent.location(),
                    &error,
                )
            })?;
        let text = text.replace(&self.issuer_namespace, &format!("_{FIXED_NAMESPACE}"));
        let stored: StoredEventRecord = serde_json::from_str(&text).map_err(|error| {
            json_failure(
                LedgerCode::CorruptLedgerRecord.code(),
                LedgerLocation::DecodeSqliteRecord.location(),
                &error,
            )
        })?;
        // LN-8d: a stored frame's record carries its reference as a persisted record does, its
        // material not read (`Unrecorded`).
        stored
            .into_event_with_artifact_availability(&mut |_| Ok(None))
            .map_err(|error| {
                record_failure(
                    error.code(),
                    LedgerLocation::ValidatePersistedEvent.location(),
                    format!("synthetic record {}", event.sequence()),
                )
            })
    }

    fn flush(&mut self) -> GlobalLedgerResult<()> {
        if self.batch.is_empty() {
            return Ok(());
        }
        let appended: AppendedBatch = self.appender.append(&self.batch)?;
        for hash in &appended.record_hashes {
            self.digest.update(hash.as_bytes());
            self.digest.update(b"\n");
        }
        self.canonical_bytes += appended.canonical_bytes;
        self.batches += 1;
        self.batch.clear();
        Ok(())
    }
}

/// LN-8d: whether `spec`'s mix stores the capture frame of a run's frame `frame`.
fn stores_frame(spec: &SyntheticLedgerSpec, frame: u64) -> bool {
    let every = spec.mix.frame_artifact_every_frames;
    every != 0 && frame % every == every - 1
}

/// LN-8d: the artifact store of the state root `root`, with a capacity owner that admits every
/// write, so the generator stores its frames as the capture store does.
fn open_frame_store(root: &Path, at_ms: u64) -> GlobalLedgerResult<ArtifactStore> {
    let store = ArtifactStore::open(root)
        .map_err(|error| store_failure("synthetic frame store", &error))?;
    let owner_epoch = *IdentifierIssuer::new()
        .map_err(|error| issue_failure(error.code()))?
        .mint_owner_epoch()
        .map_err(|error| issue_failure(error.code()))?
        .transport();
    store
        .install_capacity_admission(Arc::new(SyntheticCapacity {
            owner_epoch,
            decided_at_unix_ms: at_ms,
        }))
        .map_err(|error| store_failure("synthetic frame store", &error))?;
    Ok(store)
}

/// LN-8d: the capacity owner of the synthetic frame store: every write is admitted, as a fresh
/// sample with room admits it.
struct SyntheticCapacity {
    owner_epoch: OwnerEpoch,
    decided_at_unix_ms: u64,
}

impl ArtifactCapacityAdmission for SyntheticCapacity {
    fn decide(&self, _path: &Path, bytes: u64) -> ArtifactStoreResult<CapacityDecision> {
        Ok(CapacityDecision {
            owner_epoch: self.owner_epoch,
            decided_at_unix_ms: self.decided_at_unix_ms,
            decided_at_monotonic_ms: 0,
            requested_bytes: bytes,
            target_volume: None,
            admission_result: CapacityAdmissionOutcome::Allowed,
            reason: CapacityAdmissionReason::FreshSample,
            fact: None,
        })
    }
}

/// LN-8d: the records the artifact store gives while it stores one synthetic frame, in order.
#[derive(Default)]
struct FrameDrafts(Vec<EventDraft>);

impl ArtifactEventSink for FrameDrafts {
    fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
        self.0.push(draft);
        Ok(())
    }
}

/// LN-8d: the artifact store's failure while the generator stores `what`, with the store's code,
/// operation and detail.
fn store_failure(what: &str, error: &ArtifactStoreError) -> GlobalLedgerError {
    record_failure(
        SharedCode::ArtifactWriteFailed.code(),
        LedgerLocation::ValidateRoot.location(),
        format!(
            "{what}: {} during {}: {}",
            error.code(),
            error.operation(),
            error.detail()
        ),
    )
}

/// The matched page's targets: required, any-of, optional and forbidden ones, each with its
/// region inside the frame.
fn targets(page: usize, count: u64) -> Vec<RecognizedTarget> {
    const ROLES: [RecognizedTargetRole; 6] = [
        RecognizedTargetRole::Required,
        RecognizedTargetRole::AnyOf,
        RecognizedTargetRole::Optional,
        RecognizedTargetRole::Forbidden,
        RecognizedTargetRole::Required,
        RecognizedTargetRole::Optional,
    ];
    (0..count)
        .map(|target| {
            let slot = u32::try_from(target % 6).unwrap_or(0);
            let row = u32::try_from((target / 6) % 3).unwrap_or(0);
            let role = ROLES[usize::try_from(target % 6).unwrap_or(0)];
            RecognizedTarget {
                target_id: format!("synthetic-target-{page:02}-{target:02}"),
                role,
                passed: role != RecognizedTargetRole::Forbidden,
                region: Some(Rect {
                    x: 40 + slot * 180,
                    y: 60 + row * 200,
                    width: 160,
                    height: 90,
                }),
            }
        })
        .collect()
}

/// Each listed candidate page's verdict: the matched page passed, every other failed on its first
/// target.
fn candidate_scores(pages: &[String], matched: &str) -> RecognitionCandidateScores {
    let listed = pages.len().min(TASK_RECOGNITION_CANDIDATE_SCORE_LIMIT);
    RecognitionCandidateScores {
        pages: pages[..listed]
            .iter()
            .enumerate()
            .map(|(index, page)| {
                let passed = page == matched;
                RecognitionCandidateScore {
                    page_id: page.clone(),
                    passed,
                    best_score_milli: Some(if passed {
                        930
                    } else {
                        u32::try_from(200 + (index * 41) % 500).unwrap_or(200)
                    }),
                    first_failed_target_id: (!passed)
                        .then(|| format!("synthetic-target-{index:02}-00")),
                }
            })
            .collect(),
        omitted: u32::try_from(pages.len() - listed).unwrap_or(u32::MAX),
    }
}

fn issue_failure(code: Code) -> GlobalLedgerError {
    GlobalLedgerError::fatal(code, LedgerLocation::CreateIdentifierIssuer.location())
}

fn record_failure(code: Code, operation: Location, detail: String) -> GlobalLedgerError {
    GlobalLedgerError::fatal(code, operation).with_detail(detail)
}

fn json_failure(code: Code, operation: Location, error: &serde_json::Error) -> GlobalLedgerError {
    GlobalLedgerError::fatal(code, operation).with_detail(format!(
        "line {}, column {}",
        error.line(),
        error.column()
    ))
}

/// The calling process's memory (Windows `K32GetProcessMemoryInfo`), for the scale harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProcessMemory {
    /// Private bytes: the process's private commit (`PrivateUsage`).
    pub private_bytes: u64,
    /// The highest private commit of the process's life (`PeakPagefileUsage`).
    pub peak_private_bytes: u64,
    pub working_set_bytes: u64,
    pub peak_working_set_bytes: u64,
}

/// The calling process's memory counters.
#[cfg(windows)]
pub fn process_memory() -> std::io::Result<ProcessMemory> {
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let size = u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>())
        .map_err(|_| std::io::Error::other("memory counter size"))?;
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: size,
        ..PROCESS_MEMORY_COUNTERS_EX::default()
    };
    // SAFETY: the pseudo handle of the calling process needs no closing, and `counters` is a
    // writable PROCESS_MEMORY_COUNTERS_EX of `size` bytes, which the call fills in place of the
    // base structure it is declared with.
    let filled = unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
            size,
        )
    };
    if filled == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(ProcessMemory {
        private_bytes: counters.PrivateUsage as u64,
        peak_private_bytes: counters.PeakPagefileUsage as u64,
        working_set_bytes: counters.WorkingSetSize as u64,
        peak_working_set_bytes: counters.PeakWorkingSetSize as u64,
    })
}

/// The calling process's memory counters: read on Windows only.
#[cfg(not(windows))]
pub fn process_memory() -> std::io::Result<ProcessMemory> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "process memory counters are read on Windows only",
    ))
}

/// Milliseconds since the calling process was created.
#[cfg(windows)]
pub fn process_age_ms() -> std::io::Result<u64> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    let ticks =
        |time: FILETIME| u64::from(time.dwLowDateTime) | (u64::from(time.dwHighDateTime) << 32);
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: the pseudo handle of the calling process needs no closing, and each pointer is a
    // writable FILETIME.
    let read = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    };
    if read == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut now = FILETIME::default();
    // SAFETY: `now` is a writable FILETIME.
    unsafe { GetSystemTimeAsFileTime(&mut now) };
    ticks(now)
        .checked_sub(ticks(created))
        .map(|age| age / 10_000)
        .ok_or_else(|| std::io::Error::other("the process was created after the current time"))
}

/// Milliseconds since the calling process was created: read on Windows only.
#[cfg(not(windows))]
pub fn process_age_ms() -> std::io::Result<u64> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "the process creation time is read on Windows only",
    ))
}

#[cfg(test)]
#[path = "tests/synthetic_ledger.rs"]
mod tests;
