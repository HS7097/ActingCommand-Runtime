// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #375 R5c: the read-only frame retention view. It classes every frame (Lab, error
//! window, resource reading, duplicate, default, or still running), computes its due time and
//! the kept folder of a Lab or error frame, and lists the error points whose 30 s windows
//! keep frames. It writes nothing: every input is a fact the retention index derives from the
//! committed prefix, and the view is evaluated at the ledger head and the caller's clock. It
//! never consults the unlinked-warning latch, the Warning ring or the K/T policy.

use super::super::GlobalLedgerResult;
use super::super::projection::EventIndexes;
use super::{RetentionIndex, material_references, referenced_frames, source};
use crate::fact::LedgerEventRead;
use actingcommand_contract::{
    ArtifactId, ArtifactKind, ArtifactPinReason, CapturePayload, EffectDisposition, EventAction,
    EventFamily, EventPayload, EventSeverity, EventType, FactContent, FactPayload, FrameId,
    InstanceId, LeaseId, LeasePayload, PinnedFrameReason, ProjectedArtifactReference,
    RESOURCE_READING_DETECTOR_PREFIX, RunId, RuntimePayload, TaskOutcome, TaskPayload,
    TaskSemanticFact, parse_resource_reading_snapshot_id,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::PoisonError;

/// A frame is in an error window when its capture time lies in `[t - 30 s, t]`.
const ERROR_WINDOW_MS: u64 = 30_000;
/// A frame settles no earlier than 60 s after its capture.
const SETTLE_AGE_MS: u64 = 60_000;
const DAY_MS: u64 = 86_400_000;
const DAY_MS_I64: i64 = 86_400_000;
/// A resource reading frame is deleted 7 days after its entry time.
const READING_DAYS: u64 = 7;
/// The `<code>` of an error leaf is at most 64 characters.
const LEAF_NAME_CHARS: usize = 64;

/// The two actingd-config switches the classes read (actingd-config revision 4). Ordinary
/// frames are always deduplicated and have no switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameRetentionSwitches {
    /// `frame_retention_dedup_error` (default on): each error window drops its interior
    /// near-duplicates.
    pub dedup_error: bool,
    /// `frame_retention_dedup_lab` (default off): Lab output drops its interior
    /// near-duplicates, except Lab operation evidence.
    pub dedup_lab: bool,
}

impl Default for FrameRetentionSwitches {
    fn default() -> Self {
        Self {
            dedup_error: true,
            dedup_lab: false,
        }
    }
}

/// A UTC instant's local offset in milliseconds east of UTC, `None` when unknown. The view
/// uses [`crate::local_time::machine_local_offset_ms`]; the tests pass a fixed zone.
type LocalOffsetMs = fn(u64) -> Option<i64>;

/// A frame's class, in order of precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FrameRetentionClass {
    /// Class 1: Lab output; moved into its Lab folder at settle and deleted by people.
    Lab,
    /// Class 2: kept by at least one error window; moved into its error folder at settle and
    /// deleted by people.
    Error,
    /// Class 3: a resource reading frame; deleted 7 days after its entry time.
    Resource,
    /// Class 4: an interior near-duplicate that nothing exempts; deleted once settled.
    Duplicate,
    /// Class 5: everything else; deleted 1 day after its entry time.
    Default,
    /// Not settled; nothing touches it.
    Running,
}

/// Where a class 1 or class 2 frame is kept: `<state root>\kept\<date>\<leaf>\<file_name>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptFrameFolder {
    /// `YYYY-MM-DD` in local time: the owner error point's t, or a Lab frame's capture time.
    pub date: String,
    /// `<instance>-<sequence>-<code>` of the owner error point, or `lab-<instance>`.
    pub leaf: String,
    /// `<HHmmss-fff>_<object file name>`, the local capture time first.
    pub file_name: String,
}

/// One frame of the view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameRetentionFrame {
    pub reference: ProjectedArtifactReference,
    pub instance_id: InstanceId,
    pub run_id: Option<RunId>,
    pub class: FrameRetentionClass,
    /// The entry time, once the frame is settled.
    pub entry_unix_ms: Option<u64>,
    /// When a class 3, 4 or 5 frame may be deleted.
    pub due_unix_ms: Option<u64>,
    /// The folder of a settled class 1 or 2 frame.
    pub folder: Option<KeptFrameFolder>,
    /// Indexes into [`FrameRetentionView::error_points`] of every window containing the frame.
    pub windows: Vec<usize>,
}

/// One error point: an error event, or the epoch-end point of a run cut short.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameErrorPoint {
    /// The event's sequence; for an epoch-end point the start event that ended the epoch.
    pub sequence: u64,
    pub event_type: EventType,
    pub epoch_end: bool,
    pub instance_id: InstanceId,
    pub run_id: Option<RunId>,
    /// t, the end of its window.
    pub at_unix_ms: u64,
    pub date: String,
    pub leaf: String,
}

/// The view at one head and one clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameRetentionView {
    pub through_sequence: u64,
    pub evaluated_at_unix_ms: u64,
    /// In artifact id order.
    pub frames: Vec<FrameRetentionFrame>,
    /// In sequence order.
    pub error_points: Vec<FrameErrorPoint>,
}

/// The facts the view reads beyond the retention index's own, derived from the committed
/// prefix by `apply` alone (no clock, store or I/O).
#[derive(Default)]
pub(super) struct FrameFacts {
    /// `runtime.started` and `runtime.takeover` by sequence.
    starts: BTreeMap<u64, Start>,
    /// The aliases bound to each instance, by binding sequence.
    aliases: BTreeMap<InstanceId, BTreeMap<u64, String>>,
    runs: BTreeMap<RunId, RunFacts>,
    /// Every `Performed` lease release: its sequence and its run link.
    releases: BTreeMap<LeaseId, Vec<(u64, Option<RunId>)>>,
    /// Frames of run-less captures that completed.
    completed: BTreeSet<FrameId>,
    /// Marker chains: a marked frame's representative, and a representative's successor.
    predecessors: BTreeMap<FrameId, FrameId>,
    successors: BTreeMap<FrameId, FrameId>,
    /// Frames an `input.intent` names as its before frame.
    before_frames: BTreeSet<FrameId>,
    /// Artifacts and frames a `fact.published` names.
    published_artifacts: BTreeSet<ArtifactId>,
    published_frames: BTreeSet<FrameId>,
    /// Resource reading frames and the run their snapshot id names.
    readings: BTreeMap<FrameId, RunId>,
    /// Artifacts a capture summary pins for a reason other than `recognition_evidence`.
    summary_pins: BTreeSet<ArtifactId>,
    points: Vec<PointFacts>,
}

struct Start {
    at: u64,
    event_type: EventType,
    leaf_name: String,
}

#[derive(Default)]
struct RunFacts {
    first_sequence: u64,
    instance: Option<InstanceId>,
    /// The first terminal, its sequence and ledger timestamp: a `TerminalCommitted`, or the
    /// generic `task.completed`, `task.failed` or `task.cancelled` that ends a Lab debug run,
    /// which commits no `TerminalCommitted`.
    terminal: Option<(u64, u64)>,
    /// The `failure_code` of a failure terminal, with its sequence.
    terminal_failure: Option<(u64, String)>,
    /// The latest capture summary of the run.
    summary: Option<u64>,
    /// A Lab debug-package run (`task.requested` by `runtime.debug_package`); it never holds
    /// back the frames of its instance.
    debug: bool,
}

struct PointFacts {
    sequence: u64,
    at: u64,
    event_type: EventType,
    instance: InstanceId,
    run: Option<RunId>,
    /// The `<code>` of its leaf, read only from events at or before the point.
    leaf_name: String,
    named_artifacts: Vec<ArtifactId>,
    named_frames: Vec<FrameId>,
}

impl FrameFacts {
    pub(super) fn apply<E: LedgerEventRead>(&mut self, event: &E) {
        let sequence = event.sequence();
        let links = event.links();
        if matches!(
            event.event_type(),
            EventType::RuntimeStarted | EventType::RuntimeTakeover
        ) {
            self.starts.insert(
                sequence,
                Start {
                    at: event.timestamp_unix_ms(),
                    event_type: event.event_type(),
                    leaf_name: type_leaf_name(event.event_type()),
                },
            );
        }
        if let EventPayload::Runtime(RuntimePayload::InstanceBound(binding)) = event.payload()
            && let Some(instance) = links.instance_id()
        {
            self.aliases
                .entry(*instance)
                .or_default()
                .insert(sequence, binding.instance_alias().to_owned());
        }
        if let Some(run) = links.run_id() {
            let facts = self.runs.entry(*run).or_insert_with(|| RunFacts {
                first_sequence: sequence,
                ..RunFacts::default()
            });
            if facts.instance.is_none() {
                facts.instance = links.instance_id().copied();
            }
            if let EventPayload::Task(TaskPayload::Requested(payload)) = event.payload()
                && payload.action() == EventAction::RuntimeDebugPackage
            {
                facts.debug = true;
            }
            if facts.terminal.is_none() {
                match event.payload() {
                    EventPayload::Task(TaskPayload::Semantic(payload)) => {
                        if let TaskSemanticFact::TerminalCommitted { outcome, .. } = payload.fact()
                        {
                            facts.terminal = Some((sequence, event.timestamp_unix_ms()));
                            if *outcome == TaskOutcome::Failure {
                                facts.terminal_failure =
                                    payload_text(event.payload(), "failure_code")
                                        .map(|code| (sequence, code));
                            }
                        }
                    }
                    EventPayload::Task(
                        TaskPayload::Completed(_)
                        | TaskPayload::Failed(_)
                        | TaskPayload::Cancelled(_),
                    ) if facts.debug => {
                        facts.terminal = Some((sequence, event.timestamp_unix_ms()));
                    }
                    _ => {}
                }
            }
            if event.event_type() == EventType::CaptureSummaryCommitted {
                facts.summary = Some(sequence);
            }
        }
        if let Some(lease) = links.lease_id()
            && matches!(event.payload(), EventPayload::Lease(LeasePayload::Released(payload))
                if payload.effect_disposition() == EffectDisposition::Performed)
        {
            self.releases
                .entry(*lease)
                .or_default()
                .push((sequence, links.run_id().copied()));
        }
        if event.event_type() == EventType::CaptureCompleted
            && links.run_id().is_none()
            && let Some(frame) = links.frame_id()
        {
            self.completed.insert(*frame);
        }
        if let EventPayload::Capture(CapturePayload::DedupWindow(window)) = event.payload()
            && let (Some(preserved), Some(representative)) =
                (window.preserved_frame_id(), links.frame_id())
        {
            self.note_marker(*representative, *preserved);
        }
        if let EventPayload::Input(actingcommand_contract::InputPayload::Intent(input)) =
            event.payload()
            && let Some(frame) = input
                .provenance()
                .and_then(|provenance| provenance.before_frame_id)
        {
            self.before_frames.insert(frame);
        }
        if let EventPayload::Fact(FactPayload::Published(payload)) = event.payload() {
            for record in payload.records() {
                if let FactContent::Artifact { artifact } = &record.content {
                    self.published_artifacts.insert(artifact.artifact_id);
                }
                self.note_reading(&record.source_detector, &record.source_snapshot_id);
            }
        }
        if let EventPayload::Capture(CapturePayload::SummaryCommitted(payload)) = event.payload() {
            for pin in payload.summary().pinned() {
                if let Some(artifact) = pin.artifact() {
                    self.note_summary_pin(pin.reason(), artifact.artifact_id);
                }
            }
        }
        self.note_error_point(event);
    }

    /// A marker says that `preserved` nearly repeats its predecessor `representative`. Each
    /// frame has at most one predecessor and one successor; a second claim is ignored.
    fn note_marker(&mut self, representative: FrameId, preserved: FrameId) {
        if self.predecessors.contains_key(&preserved)
            || self.successors.contains_key(&representative)
        {
            return;
        }
        self.predecessors.insert(preserved, representative);
        self.successors.insert(representative, preserved);
    }

    /// A published record names the frame its snapshot id names; a resource reading's
    /// detector also makes that frame a reading frame of the named run.
    fn note_reading(&mut self, detector: &str, snapshot: &str) {
        if let Some((run, frame)) = parse_resource_reading_snapshot_id(snapshot) {
            self.published_frames.insert(frame);
            if detector.starts_with(RESOURCE_READING_DETECTOR_PREFIX) {
                self.readings.insert(frame, run);
            }
        }
    }

    /// `recognition_evidence` pins every recognized frame, so it exempts nothing.
    fn note_summary_pin(&mut self, reason: PinnedFrameReason, artifact: ArtifactId) {
        if reason != PinnedFrameReason::RecognitionEvidence {
            self.summary_pins.insert(artifact);
        }
    }

    /// An error point is a non-retention, non-`perf.*` event on a known instance (its own
    /// link, or its run's instance) that is a failure terminal at any severity, a run-linked
    /// event at Warning or higher, or a run-less event at Error or higher.
    fn note_error_point<E: LedgerEventRead>(&mut self, event: &E) {
        if event.payload().artifact_retention().is_some()
            || event.event_type().family() == EventFamily::Performance
        {
            return;
        }
        let links = event.links();
        let run = links.run_id().copied();
        let failure_terminal = matches!(event.payload(), EventPayload::Task(TaskPayload::Semantic(payload))
            if matches!(payload.fact(), TaskSemanticFact::TerminalCommitted { outcome: TaskOutcome::Failure, .. }));
        let point = failure_terminal
            || (run.is_some() && event.severity() >= EventSeverity::Warning)
            || (run.is_none() && event.severity() >= EventSeverity::Error);
        if !point {
            return;
        }
        let Some(instance) = links.instance_id().copied().or_else(|| {
            run.as_ref()
                .and_then(|run| self.runs.get(run))
                .and_then(|facts| facts.instance)
        }) else {
            return;
        };
        let sequence = event.sequence();
        let preceding_failure = run
            .as_ref()
            .and_then(|run| self.runs.get(run))
            .and_then(|facts| facts.terminal_failure.as_ref())
            .filter(|(terminal, _)| *terminal < sequence)
            .map(|(_, code)| code.clone());
        let own = serde_json::to_value(event.payload()).ok();
        let leaf_name = leaf_name(
            own.as_ref()
                .and_then(|value| first_string(value, "failure_code")),
            own.as_ref().and_then(|value| {
                nested_string(value, "rejection", "code").or_else(|| first_string(value, "code"))
            }),
            preceding_failure,
            event.event_type(),
        );
        self.points.push(PointFacts {
            sequence,
            at: event.timestamp_unix_ms(),
            event_type: event.event_type(),
            instance,
            run,
            leaf_name,
            named_artifacts: material_references(event)
                .into_iter()
                .filter(|reference| reference.kind == ArtifactKind::CaptureFrame)
                .map(|reference| reference.artifact_id)
                .collect(),
            named_frames: referenced_frames(event).into_iter().collect(),
        });
    }

    /// The start event that ended the owner epoch of `run`: the first `runtime.started` or
    /// `runtime.takeover` after the run's first event.
    fn ended_by(&self, run: &RunId) -> Option<(u64, &Start)> {
        let first = self.runs.get(run)?.first_sequence;
        self.starts
            .range(first.saturating_add(1)..)
            .next()
            .map(|(sequence, start)| (*sequence, start))
    }

    /// Whether `run` committed a terminal before its owner epoch ended.
    fn terminal_in_own_epoch(&self, run: &RunId) -> bool {
        let Some((terminal, _)) = self.runs.get(run).and_then(|facts| facts.terminal) else {
            return false;
        };
        self.ended_by(run).is_none_or(|(end, _)| terminal < end)
    }

    /// A run that holds back the frames of its instance: no terminal at all and its owner
    /// epoch has not ended, so it can still get an epoch-end point. A Lab debug run never does.
    fn holds_neighbours(&self, run: &RunId) -> bool {
        self.runs
            .get(run)
            .is_some_and(|facts| facts.terminal.is_none() && !facts.debug)
            && self.ended_by(run).is_none()
    }

    /// The entry time of a frame of `run` with `lease` once the run is closed: the terminal's
    /// timestamp when the terminal, the capture summary and a later `Performed` release of
    /// the lease (the run's own, or a run-less one) come first; the timestamp of the start
    /// event that ended the run's owner epoch when that comes first.
    fn run_entry(&self, run: &RunId, lease: Option<LeaseId>) -> Option<u64> {
        let normal = self.runs.get(run).and_then(|facts| {
            let (terminal, terminal_at) = facts.terminal?;
            let summary = facts.summary?;
            let release = match lease {
                Some(lease) => self
                    .releases
                    .get(&lease)?
                    .iter()
                    .find(|(sequence, released)| {
                        *sequence > terminal && released.is_none_or(|released| released == *run)
                    })
                    .map(|(sequence, _)| *sequence)?,
                None => terminal,
            };
            Some((terminal.max(summary).max(release), terminal_at))
        });
        let ended = self
            .ended_by(run)
            .map(|(sequence, start)| (sequence, start.at));
        match (normal, ended) {
            (Some((closed, at)), Some((ended, ended_at))) => {
                Some(if closed <= ended { at } else { ended_at })
            }
            (Some((_, at)), None) | (None, Some((_, at))) => Some(at),
            (None, None) => None,
        }
    }

    /// The alias bound to `instance` at `sequence`, made safe for a folder name; the
    /// instance id when no binding precedes it.
    fn alias(&self, instance: &InstanceId, sequence: u64) -> String {
        let alias = self
            .aliases
            .get(instance)
            .and_then(|bound| bound.range(..=sequence).next_back())
            .map(|(_, alias)| alias.clone())
            .unwrap_or_else(|| id_text(instance));
        leaf_part(&alias, usize::MAX)
    }
}

/// One frame as the view builds it: a `capture.frame` artifact with its recorded identity.
struct ViewFrame {
    reference: ProjectedArtifactReference,
    instance: InstanceId,
    run: Option<RunId>,
    lease: Option<LeaseId>,
    frame: Option<FrameId>,
    /// The capture time.
    at: u64,
    /// The sequence of its `artifact.verified`, for the Lab folder's alias.
    verified_sequence: u64,
    /// An unreleased Lab pin, or the index's Lab protection.
    lab: bool,
    /// Named by a capture summary; an unsummarized frame with a lease is Lab operation
    /// evidence.
    summarized: bool,
}

/// An error point with its t resolved at head.
struct ResolvedPoint {
    sequence: u64,
    event_type: EventType,
    epoch_end: bool,
    instance: InstanceId,
    run: Option<RunId>,
    at: u64,
    alias_sequence: u64,
    leaf_name: String,
    named: BTreeSet<ArtifactId>,
}

/// The frames of one run: first and last capture times, and the last frame's instance and
/// verified sequence.
struct RunSpan {
    first_at: u64,
    last_at: u64,
    last_sequence: u64,
    instance: InstanceId,
}

/// Workflow #375 R5d: the classes at one head and one setting. Only the clock turns them into
/// a view, so a cleaner round at an unchanged head classes nothing again.
pub(super) struct FrameCache {
    through_sequence: u64,
    switches: FrameRetentionSwitches,
    classified: Classified,
}

/// Every frame classed at one head, independent of the clock.
struct Classified {
    frames: Vec<ClassifiedFrame>,
    error_points: Vec<FrameErrorPoint>,
}

/// One frame classed at one head. Its class, entry, due time and folder apply once it settles.
struct ClassifiedFrame {
    reference: ProjectedArtifactReference,
    instance: InstanceId,
    run: Option<RunId>,
    /// 60 s after the capture, once the entry is closed and no unterminated run on the
    /// instance holds the frame; `None` until then.
    settles_at: Option<u64>,
    entry: Option<u64>,
    class: FrameRetentionClass,
    due: Option<u64>,
    folder: Option<KeptFrameFolder>,
    windows: Vec<usize>,
}

impl Classified {
    /// The view at `now`: a frame not yet settled is `Running`, with no entry, due time or
    /// folder.
    fn at(&self, through_sequence: u64, now: u64) -> FrameRetentionView {
        FrameRetentionView {
            through_sequence,
            evaluated_at_unix_ms: now,
            frames: self
                .frames
                .iter()
                .map(|frame| {
                    let settled = frame.settles_at.is_some_and(|at| now >= at);
                    FrameRetentionFrame {
                        reference: frame.reference.clone(),
                        instance_id: frame.instance,
                        run_id: frame.run,
                        class: if settled {
                            frame.class
                        } else {
                            FrameRetentionClass::Running
                        },
                        entry_unix_ms: frame.entry.filter(|_| settled),
                        due_unix_ms: frame.due.filter(|_| settled),
                        folder: frame.folder.clone().filter(|_| settled),
                        windows: frame.windows.clone(),
                    }
                })
                .collect(),
            error_points: self.error_points.clone(),
        }
    }
}

impl RetentionIndex {
    /// Workflow #375 R5c/R5d: the frame view at this index's head and at `now`, named in the
    /// machine's local time. The classes are cached by head and setting.
    pub(in crate::global) fn frame_view<E: LedgerEventRead>(
        &self,
        events: &[E],
        indexes: &EventIndexes,
        now: u64,
        switches: FrameRetentionSwitches,
    ) -> GlobalLedgerResult<FrameRetentionView> {
        // A poisoned cache only loses its cached value, which is built again below.
        let mut cache = self
            .frame_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(cached) = cache.as_ref().filter(|cached| {
            cached.through_sequence == self.through_sequence && cached.switches == switches
        }) {
            return Ok(cached.classified.at(self.through_sequence, now));
        }
        let frames = self.view_frames(events, indexes)?;
        let classified = classify(
            &self.frame_facts,
            &self.frames,
            &frames,
            switches,
            crate::local_time::machine_local_offset_ms,
        )
        // A local time the system cannot convert refuses this view; the writer goes on.
        .ok_or_else(|| {
            super::super::GlobalLedgerError::request(
                "artifact_retention_clock_failed",
                "frame_retention_view",
            )
        })?;
        let view = classified.at(self.through_sequence, now);
        *cache = Some(FrameCache {
            through_sequence: self.through_sequence,
            switches,
            classified,
        });
        Ok(view)
    }

    fn view_frames<E: LedgerEventRead>(
        &self,
        events: &[E],
        indexes: &EventIndexes,
    ) -> GlobalLedgerResult<Vec<ViewFrame>> {
        let mut frames = Vec::new();
        for object in self.objects.values() {
            let Some(identity) = object.identity.as_ref() else {
                continue;
            };
            if object.reference.kind != ArtifactKind::CaptureFrame || object.proof.is_some() {
                continue;
            }
            let lab = object
                .pins
                .values()
                .any(|(_, reason)| *reason == ArtifactPinReason::Lab)
                || match &object.verified {
                    Some(verified) => {
                        self.lab_protected(object, source(events, verified)?, indexes)
                    }
                    None => false,
                };
            frames.push(ViewFrame {
                reference: identity.artifact.clone(),
                instance: identity.instance_id,
                run: identity.run_id,
                lease: identity.lease_id,
                frame: identity.artifact.frame_id,
                at: identity.artifact.created_at_unix_ms,
                verified_sequence: object
                    .verified
                    .map_or(self.through_sequence, |verified| verified.sequence),
                lab,
                summarized: object.summary.is_some(),
            });
        }
        Ok(frames)
    }
}

/// Classes every frame at one head. Pure: every input is a fact of the committed prefix or an
/// argument. `None` when the local offset of a named instant is unknown.
fn classify(
    facts: &FrameFacts,
    frame_artifacts: &BTreeMap<FrameId, BTreeSet<ArtifactId>>,
    frames: &[ViewFrame],
    switches: FrameRetentionSwitches,
    local: LocalOffsetMs,
) -> Option<Classified> {
    let by_frame = frames
        .iter()
        .enumerate()
        .filter_map(|(index, frame)| Some((frame.frame?, index)))
        .collect::<BTreeMap<_, _>>();
    let mut spans = BTreeMap::<RunId, RunSpan>::new();
    for frame in frames {
        let Some(run) = frame.run else {
            continue;
        };
        let span = spans.entry(run).or_insert(RunSpan {
            first_at: frame.at,
            last_at: frame.at,
            last_sequence: frame.verified_sequence,
            instance: frame.instance,
        });
        span.first_at = span.first_at.min(frame.at);
        if frame.at >= span.last_at {
            span.last_at = frame.at;
            span.last_sequence = frame.verified_sequence;
            span.instance = frame.instance;
        }
    }
    // A frame waits while a run on its instance whose first frame is at most 30 s after it
    // has no terminal and its owner epoch has not ended: such a run can still get an
    // epoch-end point dated at its last frame.
    let mut waits = BTreeMap::<InstanceId, u64>::new();
    for (run, span) in &spans {
        if facts.holds_neighbours(run) {
            let first = waits.entry(span.instance).or_insert(span.first_at);
            *first = (*first).min(span.first_at);
        }
    }
    let mut points = Vec::new();
    for point in &facts.points {
        // An error point on a run with no terminal in its own owner epoch, appended after
        // that epoch ended, is dated at the run's last frame.
        let late = point
            .run
            .as_ref()
            .filter(|run| {
                facts
                    .ended_by(run)
                    .is_some_and(|(end, _)| point.sequence > end)
                    && !facts.terminal_in_own_epoch(run)
            })
            .and_then(|run| spans.get(run))
            .map(|span| span.last_at);
        let mut named = point
            .named_artifacts
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        for frame in &point.named_frames {
            if let Some(artifacts) = frame_artifacts.get(frame) {
                named.extend(artifacts.iter().copied());
            }
        }
        points.push(ResolvedPoint {
            sequence: point.sequence,
            event_type: point.event_type,
            epoch_end: false,
            instance: point.instance,
            run: point.run,
            at: late.unwrap_or(point.at),
            alias_sequence: point.sequence,
            leaf_name: point.leaf_name.clone(),
            named,
        });
    }
    // The epoch-end point of a run with frames whose owner epoch ended before its terminal.
    for (run, span) in &spans {
        let Some((end, start)) = facts.ended_by(run) else {
            continue;
        };
        if facts.terminal_in_own_epoch(run) {
            continue;
        }
        points.push(ResolvedPoint {
            sequence: end,
            event_type: start.event_type,
            epoch_end: true,
            instance: span.instance,
            run: Some(*run),
            at: span.last_at,
            alias_sequence: span.last_sequence,
            leaf_name: start.leaf_name.clone(),
            named: BTreeSet::new(),
        });
    }
    points.sort_by_key(|point| (point.sequence, point.at, point.epoch_end));
    let mut by_instance = BTreeMap::<InstanceId, Vec<(u64, usize)>>::new();
    let mut naming = BTreeMap::<ArtifactId, Vec<usize>>::new();
    for (index, point) in points.iter().enumerate() {
        by_instance
            .entry(point.instance)
            .or_default()
            .push((point.at, index));
        for artifact in &point.named {
            naming.entry(*artifact).or_default().push(index);
        }
    }
    for list in by_instance.values_mut() {
        list.sort_unstable();
    }
    let mut out = Vec::with_capacity(frames.len());
    for frame in frames {
        let mut windows = naming
            .get(&frame.reference.artifact_id)
            .into_iter()
            .flatten()
            .copied()
            .collect::<BTreeSet<_>>();
        if let Some(list) = by_instance.get(&frame.instance) {
            let start = list.partition_point(|(at, _)| *at < frame.at);
            for (at, index) in &list[start..] {
                if *at > frame.at.saturating_add(ERROR_WINDOW_MS) {
                    break;
                }
                windows.insert(*index);
            }
        }
        let windows = windows.into_iter().collect::<Vec<_>>();
        let entry = match frame.run.as_ref() {
            Some(run) => facts.run_entry(run, frame.lease),
            None => frame
                .frame
                .filter(|id| facts.completed.contains(id))
                .map(|_| frame.at),
        };
        let waiting = waits
            .get(&frame.instance)
            .is_some_and(|first| *first <= frame.at.saturating_add(ERROR_WINDOW_MS));
        let entry = entry.filter(|_| !waiting);
        let mut class = FrameRetentionClass::Running;
        let mut due = None;
        let mut folder = None;
        if let Some(entry_at) = entry {
            let id = frame.frame;
            let interior = id.is_some_and(|id| {
                facts.predecessors.contains_key(&id) && facts.successors.contains_key(&id)
            });
            // A run closed by the end of its epoch without a capture summary has unknown
            // summary pins: none of its frames is deleted as a duplicate or dropped by a window.
            let no_summary = frame
                .run
                .as_ref()
                .and_then(|run| facts.runs.get(run))
                .is_some_and(|run| run.summary.is_none());
            let exempt = facts.summary_pins.contains(&frame.reference.artifact_id)
                || id.is_some_and(|id| {
                    facts.before_frames.contains(&id) || facts.published_frames.contains(&id)
                })
                || (frame.lease.is_some() && !frame.summarized)
                || facts
                    .published_artifacts
                    .contains(&frame.reference.artifact_id)
                || naming.contains_key(&frame.reference.artifact_id);
            let duplicate = interior && !exempt && !no_summary;
            let previous = neighbour(
                frames,
                &by_frame,
                id.and_then(|id| facts.predecessors.get(&id)),
            );
            let next = neighbour(
                frames,
                &by_frame,
                id.and_then(|id| facts.successors.get(&id)),
            );
            let keeps = |index: &usize| {
                let point = &points[*index];
                !(switches.dedup_error
                    && duplicate
                    && previous.is_some_and(|previous| in_window(point, previous))
                    && next.is_some_and(|next| in_window(point, next)))
            };
            class = if frame.lab && !(switches.dedup_lab && duplicate) {
                FrameRetentionClass::Lab
            } else if windows.iter().any(keeps) {
                FrameRetentionClass::Error
            } else if id.is_some_and(|id| {
                facts
                    .readings
                    .get(&id)
                    .is_some_and(|run| frame.run.as_ref() == Some(run))
            }) {
                FrameRetentionClass::Resource
            } else if duplicate {
                FrameRetentionClass::Duplicate
            } else {
                FrameRetentionClass::Default
            };
            due = match class {
                FrameRetentionClass::Duplicate => Some(entry_at),
                FrameRetentionClass::Default => Some(entry_at.saturating_add(DAY_MS)),
                FrameRetentionClass::Resource => {
                    Some(entry_at.saturating_add(READING_DAYS * DAY_MS))
                }
                _ => None,
            };
            folder = match class {
                FrameRetentionClass::Lab => Some(KeptFrameFolder {
                    date: local_date(frame.at, local)?,
                    leaf: format!(
                        "lab-{}",
                        facts.alias(&frame.instance, frame.verified_sequence)
                    ),
                    file_name: file_name(frame, local)?,
                }),
                // The owner is the lowest-sequence point whose window contains the frame,
                // whichever window keeps it.
                FrameRetentionClass::Error => {
                    match windows
                        .iter()
                        .map(|index| &points[*index])
                        .min_by_key(|point| (point.sequence, point.at))
                    {
                        Some(owner) => Some(KeptFrameFolder {
                            date: local_date(owner.at, local)?,
                            leaf: point_leaf(facts, owner),
                            file_name: file_name(frame, local)?,
                        }),
                        None => None,
                    }
                }
                _ => None,
            };
        }
        out.push(ClassifiedFrame {
            reference: frame.reference.clone(),
            instance: frame.instance,
            run: frame.run,
            settles_at: entry.map(|_| frame.at.saturating_add(SETTLE_AGE_MS)),
            entry,
            class,
            due,
            folder,
            windows,
        });
    }
    let mut error_points = Vec::with_capacity(points.len());
    for point in &points {
        error_points.push(FrameErrorPoint {
            sequence: point.sequence,
            event_type: point.event_type,
            epoch_end: point.epoch_end,
            instance_id: point.instance,
            run_id: point.run,
            at_unix_ms: point.at,
            date: local_date(point.at, local)?,
            leaf: point_leaf(facts, point),
        });
    }
    Some(Classified {
        frames: out,
        error_points,
    })
}

fn neighbour<'f>(
    frames: &'f [ViewFrame],
    by_frame: &BTreeMap<FrameId, usize>,
    of: Option<&FrameId>,
) -> Option<&'f ViewFrame> {
    of.and_then(|id| by_frame.get(id))
        .map(|index| &frames[*index])
}

/// The frames of a point's window: its instance's frames captured in `[t - 30 s, t]`, and
/// the frames its event names.
fn in_window(point: &ResolvedPoint, frame: &ViewFrame) -> bool {
    point.named.contains(&frame.reference.artifact_id)
        || (frame.instance == point.instance
            && (point.at.saturating_sub(ERROR_WINDOW_MS)..=point.at).contains(&frame.at))
}

/// `<instance>-<sequence>-<code>`.
fn point_leaf(facts: &FrameFacts, point: &ResolvedPoint) -> String {
    format!(
        "{}-{}-{}",
        facts.alias(&point.instance, point.alias_sequence),
        point.sequence,
        point.leaf_name
    )
}

/// `<HHmmss-fff>_<object file name>`, in local time.
fn file_name(frame: &ViewFrame, local: LocalOffsetMs) -> Option<String> {
    let object = frame
        .reference
        .object_key
        .as_deref()
        .and_then(|key| key.rsplit('/').next())
        .unwrap_or_default();
    let time = local_time(frame.at, local)?;
    Some(format!(
        "{:02}{:02}{:02}-{:03}_{object}",
        time.hour, time.minute, time.second, time.milli
    ))
}

/// The `<code>` part of an error leaf: the event's own `failure_code`, else its own code (a
/// `rejection.code` before any other `code`), else the failure code of its run's failure
/// terminal when that terminal precedes it, else its event type with `.` as `_`; at most 64
/// characters, outside `[A-Za-z0-9_-]` as `_`.
fn leaf_name(
    own_failure: Option<String>,
    own_value: Option<String>,
    preceding_failure: Option<String>,
    event_type: EventType,
) -> String {
    let name = own_failure
        .or(own_value)
        .or(preceding_failure)
        .unwrap_or_else(|| type_leaf_name(event_type));
    leaf_part(&name, LEAF_NAME_CHARS)
}

/// The first string under `key` in the payload's JSON, depth first in field order.
fn payload_text(payload: &EventPayload, key: &str) -> Option<String> {
    first_string(&serde_json::to_value(payload).ok()?, key)
}

fn first_string(value: &serde_json::Value, key: &str) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => map.iter().find_map(|(name, item)| match item {
            serde_json::Value::String(text) if name == key => Some(text.clone()),
            _ => first_string(item, key),
        }),
        serde_json::Value::Array(items) => items.iter().find_map(|item| first_string(item, key)),
        _ => None,
    }
}

/// The string `key` of the first object named `parent`, depth first in field order.
fn nested_string(value: &serde_json::Value, parent: &str, key: &str) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => map.iter().find_map(|(name, item)| {
            if name == parent
                && let Some(serde_json::Value::String(text)) = item.get(key)
            {
                return Some(text.clone());
            }
            nested_string(item, parent, key)
        }),
        serde_json::Value::Array(items) => items
            .iter()
            .find_map(|item| nested_string(item, parent, key)),
        _ => None,
    }
}

/// The event type's wire name with `.` as `_` (`runtime.takeover` is `runtime_takeover`).
fn type_leaf_name(event_type: EventType) -> String {
    match serde_json::to_value(event_type) {
        Ok(serde_json::Value::String(name)) => name.replace('.', "_"),
        _ => String::new(),
    }
}

fn id_text(instance: &InstanceId) -> String {
    match serde_json::to_value(instance) {
        Ok(serde_json::Value::String(text)) => text,
        _ => String::new(),
    }
}

/// At most `limit` characters, each outside `[A-Za-z0-9_-]` replaced by `_`.
fn leaf_part(text: &str, limit: usize) -> String {
    text.chars()
        .take(limit)
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

struct LocalTime {
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    milli: i64,
}

/// The civil date and time of a UTC instant at the local offset (proleptic Gregorian).
fn local_time(unix_ms: u64, local: LocalOffsetMs) -> Option<LocalTime> {
    let local_ms = i64::try_from(unix_ms).ok()?.checked_add(local(unix_ms)?)?;
    let days = local_ms.div_euclid(DAY_MS_I64);
    let of_day = local_ms.rem_euclid(DAY_MS_I64);
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    Some(LocalTime {
        year: year_of_era + era * 400 + i64::from(month <= 2),
        month,
        day,
        hour: of_day / 3_600_000,
        minute: of_day / 60_000 % 60,
        second: of_day / 1_000 % 60,
        milli: of_day % 1_000,
    })
}

fn local_date(unix_ms: u64, local: LocalOffsetMs) -> Option<String> {
    let time = local_time(unix_ms, local)?;
    Some(format!(
        "{:04}-{:02}-{:02}",
        time.year, time.month, time.day
    ))
}

impl<B: super::super::storage::DurableStorage> super::super::storage::EventStore<B> {
    /// Workflow #375 R5d: the frame view at the writer's head.
    pub(in crate::global) fn frame_retention_view(
        &self,
        now_unix_ms: u64,
        switches: FrameRetentionSwitches,
    ) -> GlobalLedgerResult<FrameRetentionView> {
        self.retention
            .frame_view(&self.events, &self.indexes, now_unix_ms, switches)
    }
}

impl super::super::GlobalLedger {
    /// Workflow #375 R5d: the frame view at the writer's head and at `now_unix_ms`, which the
    /// frame cleaner reads once per sweep. Read-only; the classes are cached by head.
    pub fn frame_retention_view(
        &self,
        now_unix_ms: u64,
        switches: FrameRetentionSwitches,
    ) -> GlobalLedgerResult<FrameRetentionView> {
        let (response, receiver) = std::sync::mpsc::sync_channel(1);
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| super::invalid("writer_unavailable"))?;
        super::super::send_command(
            sender,
            super::super::WriterCommand::FrameRetentionView {
                now_unix_ms,
                switches,
                response,
            },
            "frame_retention_view",
        )?;
        super::super::receive_response(receiver, "frame_retention_view")?
    }
}

impl super::super::GlobalLedgerEvidence {
    /// Workflow #375 R5c: the frame view over this opening's events at `now_unix_ms`: every
    /// frame's class, due time and kept folder, and the error points. Read-only. The retention
    /// index is built from the events once per opening.
    pub fn frame_retention_view(
        &self,
        now_unix_ms: u64,
        switches: FrameRetentionSwitches,
    ) -> GlobalLedgerResult<FrameRetentionView> {
        let events = self.events();
        let (retention, indexes) = match self.frame_index.get() {
            Some(built) => built,
            None => {
                let built =
                    RetentionIndex::from_events_with_indexes_checked(events, &mut |_| Ok(()))?;
                // Another caller may have built it meanwhile; both copies are the same.
                self.frame_index.get_or_init(|| built)
            }
        };
        retention.frame_view(events, indexes, now_unix_ms, switches)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use FrameRetentionClass as C;
    use actingcommand_contract::{
        ArtifactProducer, ArtifactRedactionState, IdentifierIssuer, RetentionClass,
        resource_reading_snapshot_id,
    };

    /// 2026-10-12 06:15:02.117 JST, 21:15:02.117 UTC the day before.
    const T0: u64 = 1_791_753_302_117;
    const SECOND: u64 = 1_000;
    const ON: FrameRetentionSwitches = FrameRetentionSwitches {
        dedup_error: true,
        dedup_lab: false,
    };

    fn at(seconds: u64) -> u64 {
        T0 + seconds * SECOND
    }

    fn utc(_: u64) -> Option<i64> {
        Some(0)
    }

    fn jst(_: u64) -> Option<i64> {
        Some(9 * 3_600_000)
    }

    fn unknown(_: u64) -> Option<i64> {
        None
    }

    fn text<T: serde::Serialize>(id: &T) -> String {
        match serde_json::to_value(id) {
            Ok(serde_json::Value::String(text)) => text,
            other => panic!("identifier text: {other:?}"),
        }
    }

    /// Frames of one instance, built from the facts the view reads.
    struct Fixture {
        ids: IdentifierIssuer,
        instance: InstanceId,
        facts: FrameFacts,
        frames: Vec<ViewFrame>,
        frame_artifacts: BTreeMap<FrameId, BTreeSet<ArtifactId>>,
    }

    impl Fixture {
        fn new() -> Self {
            let ids = IdentifierIssuer::new().expect("issuer");
            let instance = *ids.mint_instance_id().expect("instance").transport();
            let mut facts = FrameFacts::default();
            facts
                .aliases
                .entry(instance)
                .or_default()
                .insert(1, "node.a b".to_owned());
            Self {
                ids,
                instance,
                facts,
                frames: Vec::new(),
                frame_artifacts: BTreeMap::new(),
            }
        }

        /// A run whose first event is `first_sequence`, and the lease its frames carry.
        fn run(&mut self, first_sequence: u64) -> (RunId, LeaseId) {
            let run = *self.ids.mint_run_id().expect("run").transport();
            let lease = *self.ids.mint_lease_id().expect("lease").transport();
            self.facts.runs.insert(
                run,
                RunFacts {
                    first_sequence,
                    instance: Some(self.instance),
                    ..RunFacts::default()
                },
            );
            (run, lease)
        }

        /// The normal close: the terminal, the capture summary, then the lease's release.
        fn close(&mut self, run: (RunId, LeaseId), terminal: u64, terminal_at: u64) {
            let facts = self.facts.runs.get_mut(&run.0).expect("run");
            facts.terminal = Some((terminal, terminal_at));
            facts.summary = Some(terminal + 1);
            self.facts
                .releases
                .entry(run.1)
                .or_default()
                .push((terminal + 2, Some(run.0)));
        }

        /// A frame captured at `captured_at`: a run frame with the run's lease, listed in its
        /// capture summary, or a completed run-less observe frame without a lease.
        fn frame(&mut self, run: Option<(RunId, LeaseId)>, captured_at: u64) -> usize {
            let artifact = *self.ids.mint_artifact_id().expect("artifact").transport();
            let frame = *self.ids.mint_frame_id().expect("frame").transport();
            self.frame_artifacts
                .entry(frame)
                .or_default()
                .insert(artifact);
            self.frames.push(ViewFrame {
                reference: ProjectedArtifactReference {
                    artifact_id: artifact,
                    kind: ArtifactKind::CaptureFrame,
                    run_id: run.map(|(run, _)| run),
                    frame_id: Some(frame),
                    correlation_id: None,
                    object_key: Some(format!("artifacts/ab/{}.png", text(&artifact))),
                    media_type: ArtifactKind::CaptureFrame.media_type(),
                    byte_count: 1,
                    sha256: String::new(),
                    created_at_unix_ms: captured_at,
                    producer: ArtifactProducer::CaptureStore,
                    retention_class: RetentionClass::Adaptive,
                    redaction_state: ArtifactRedactionState::NotRequired,
                },
                instance: self.instance,
                run: run.map(|(run, _)| run),
                lease: run.map(|(_, lease)| lease),
                frame: Some(frame),
                at: captured_at,
                verified_sequence: 2,
                lab: false,
                summarized: run.is_some(),
            });
            if run.is_none() {
                self.facts.completed.insert(frame);
            }
            self.frames.len() - 1
        }

        fn id(&self, index: usize) -> FrameId {
            self.frames[index].frame.expect("frame id")
        }

        fn artifact(&self, index: usize) -> ArtifactId {
            self.frames[index].reference.artifact_id
        }

        /// A similar run: each frame is marked against the frame before it.
        fn chain(&mut self, frames: &[usize]) {
            for pair in frames.windows(2) {
                let (representative, preserved) = (self.id(pair[0]), self.id(pair[1]));
                self.facts.note_marker(representative, preserved);
            }
        }

        fn point(&mut self, sequence: u64, point_at: u64, run: Option<RunId>, leaf_name: &str) {
            self.facts.points.push(PointFacts {
                sequence,
                at: point_at,
                event_type: EventType::TaskFailed,
                instance: self.instance,
                run,
                leaf_name: leaf_name.to_owned(),
                named_artifacts: Vec::new(),
                named_frames: Vec::new(),
            });
        }

        fn start(&mut self, sequence: u64, start_at: u64, event_type: EventType) {
            self.facts.starts.insert(
                sequence,
                Start {
                    at: start_at,
                    event_type,
                    leaf_name: type_leaf_name(event_type),
                },
            );
        }

        fn view(
            &self,
            now: u64,
            switches: FrameRetentionSwitches,
        ) -> (Vec<FrameRetentionFrame>, Vec<FrameErrorPoint>) {
            let view = classify(
                &self.facts,
                &self.frame_artifacts,
                &self.frames,
                switches,
                jst,
            )
            .expect("local time")
            .at(0, now);
            (view.frames, view.error_points)
        }

        fn classes(&self, now: u64, switches: FrameRetentionSwitches) -> Vec<FrameRetentionClass> {
            self.view(now, switches)
                .0
                .iter()
                .map(|frame| frame.class)
                .collect()
        }
    }

    #[test]
    fn an_error_window_keeps_thirty_seconds_and_drops_its_interior_duplicates() {
        let mut fixture = Fixture::new();
        let earlier = fixture.run(5);
        fixture.frame(Some(earlier), T0 - 31 * SECOND);
        fixture.close(earlier, 20, T0 - 30 * SECOND);
        let run = fixture.run(30);
        let frames = (0..5)
            .map(|offset| fixture.frame(Some(run), at(offset)))
            .collect::<Vec<_>>();
        fixture.chain(&frames[..4]);
        let pinned = fixture.artifact(frames[2]);
        fixture
            .facts
            .note_summary_pin(PinnedFrameReason::PreInput, pinned);
        fixture.close(run, 100, at(4));
        fixture.point(100, at(4), Some(run.0), "contained_task_page_unknown");
        let now = T0 + 2 * DAY_MS;

        // Frame 1 is marked, its predecessor and its successor are in the window, and nothing
        // exempts it: the window drops it. Frame 2 is pinned for an input; frames 0 and 3 are
        // the first and the last of the similar run. The frame 31 s before t is in no window.
        assert_eq!(
            fixture.classes(now, ON),
            [
                C::Default,
                C::Error,
                C::Duplicate,
                C::Error,
                C::Error,
                C::Error
            ]
        );
        let off = FrameRetentionSwitches {
            dedup_error: false,
            dedup_lab: false,
        };
        assert_eq!(
            fixture.classes(now, off),
            [C::Default, C::Error, C::Error, C::Error, C::Error, C::Error]
        );
    }

    #[test]
    fn exemptions_readings_due_times_and_running_frames() {
        let mut fixture = Fixture::new();
        let run = fixture.run(10);
        let frames = (0..6)
            .map(|offset| fixture.frame(Some(run), at(offset)))
            .collect::<Vec<_>>();
        fixture.chain(&frames);
        // recognition_evidence alone does not exempt; an input's before frame, a published
        // artifact and a pre-input pin do.
        let (evidence, before, published, pinned) = (
            fixture.artifact(frames[1]),
            fixture.id(frames[2]),
            fixture.artifact(frames[3]),
            fixture.artifact(frames[4]),
        );
        fixture
            .facts
            .note_summary_pin(PinnedFrameReason::RecognitionEvidence, evidence);
        fixture.facts.before_frames.insert(before);
        fixture.facts.published_artifacts.insert(published);
        fixture
            .facts
            .note_summary_pin(PinnedFrameReason::PreInput, pinned);
        // The run's reading frame, and a frame whose reading names another run.
        let reading = fixture.frame(Some(run), at(6));
        let stranger = fixture.frame(Some(run), at(6));
        let other_run = *fixture.ids.mint_run_id().expect("run").transport();
        for (index, named_run) in [(reading, run.0), (stranger, other_run)] {
            let snapshot =
                resource_reading_snapshot_id(&text(&named_run), &text(&fixture.id(index)), "coins");
            fixture
                .facts
                .note_reading("resource_reading:daily/coins", &snapshot);
        }
        fixture.close(run, 50, at(7));
        // Run-less observe captures, compared across requests: the interior ones are duplicates.
        let observe = (20..24)
            .map(|offset| fixture.frame(None, at(offset)))
            .collect::<Vec<_>>();
        fixture.chain(&observe);
        let now = T0 + 2 * DAY_MS;
        // Less than 60 s old: not settled.
        let young = fixture.frame(None, now - 30 * SECOND);

        let (view, points) = fixture.view(now, ON);
        assert!(points.is_empty());
        let classes = view.iter().map(|frame| frame.class).collect::<Vec<_>>();
        assert_eq!(
            classes,
            [
                C::Default,
                C::Duplicate,
                C::Default,
                C::Default,
                C::Default,
                C::Default,
                C::Resource,
                C::Default,
                C::Default,
                C::Duplicate,
                C::Duplicate,
                C::Default,
                C::Running,
            ]
        );
        let due = |index: usize| (view[index].entry_unix_ms, view[index].due_unix_ms);
        assert_eq!(due(frames[0]), (Some(at(7)), Some(at(7) + DAY_MS)));
        assert_eq!(due(frames[1]), (Some(at(7)), Some(at(7))));
        assert_eq!(due(reading), (Some(at(7)), Some(at(7) + 7 * DAY_MS)));
        assert_eq!(due(observe[1]), (Some(at(21)), Some(at(21))));
        assert_eq!(due(young), (None, None));
        assert!(view.iter().all(|frame| frame.folder.is_none()));
    }

    #[test]
    fn lab_output_is_kept_and_lab_dedup_spares_lab_operation_evidence() {
        let mut fixture = Fixture::new();
        // A Lab task run, a lease-less Lab observe burst, two consecutive Lab operations
        // (observe frames that carry the operation's lease), and a burst holding a Lab input's
        // before frame.
        let run = fixture.run(10);
        let task = (0..3)
            .map(|offset| fixture.frame(Some(run), at(offset)))
            .collect::<Vec<_>>();
        fixture.close(run, 40, at(3));
        let burst = (10..13)
            .map(|offset| fixture.frame(None, at(offset)))
            .collect::<Vec<_>>();
        let lease = *fixture.ids.mint_lease_id().expect("lease").transport();
        let operations = (20..24)
            .map(|offset| fixture.frame(None, at(offset)))
            .collect::<Vec<_>>();
        for index in &operations {
            fixture.frames[*index].lease = Some(lease);
        }
        let inputs = (30..33)
            .map(|offset| fixture.frame(None, at(offset)))
            .collect::<Vec<_>>();
        let before = fixture.id(inputs[1]);
        fixture.facts.before_frames.insert(before);
        for frame in &mut fixture.frames {
            frame.lab = true;
        }
        for frames in [&task, &burst, &operations, &inputs] {
            fixture.chain(frames);
        }
        let now = T0 + DAY_MS;

        let (view, _) = fixture.view(now, ON);
        assert!(view.iter().all(|frame| frame.class == C::Lab));
        let folder = view[task[0]].folder.as_ref().expect("Lab folder");
        assert_eq!(folder.date, "2026-10-12");
        assert_eq!(folder.leaf, "lab-node_a_b");
        assert_eq!(
            folder.file_name,
            format!("061502-117_{}.png", text(&fixture.artifact(task[0])))
        );
        let lab = FrameRetentionSwitches {
            dedup_error: true,
            dedup_lab: true,
        };
        let classes = fixture.classes(now, lab);
        let deleted = classes
            .iter()
            .enumerate()
            .filter(|(_, class)| **class == C::Duplicate)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        // Only the interior frames of the task run and of the lease-less burst go; both Lab
        // operations keep every frame, and so does the input's before frame.
        assert_eq!(deleted, [task[1], burst[1]]);
    }

    #[test]
    fn a_run_settles_after_its_release_and_its_neighbours_wait_for_its_terminal() {
        // A failed cli run closes at the run-less Performed release of its lease.
        let mut fixture = Fixture::new();
        let cli = fixture.run(10);
        fixture.frame(Some(cli), T0);
        {
            let facts = fixture.facts.runs.get_mut(&cli.0).expect("run");
            facts.terminal = Some((20, at(1)));
            facts.summary = Some(21);
        }
        fixture
            .facts
            .releases
            .entry(cli.1)
            .or_default()
            .push((15, None));
        let now = T0 + DAY_MS / 2;
        assert_eq!(fixture.classes(now, ON), [C::Running]);
        fixture
            .facts
            .releases
            .entry(cli.1)
            .or_default()
            .push((22, None));
        let (view, _) = fixture.view(now, ON);
        assert_eq!(view[0].class, C::Default);
        assert_eq!(view[0].entry_unix_ms, Some(at(1)));
        assert_eq!(view[0].due_unix_ms, Some(at(1) + DAY_MS));

        // A frame of the previous run 20 s before an unterminated run's first frame waits.
        let mut fixture = Fixture::new();
        let previous = fixture.run(5);
        fixture.frame(Some(previous), at(0));
        fixture.close(previous, 8, at(1));
        let next = fixture.run(10);
        fixture.frame(Some(next), at(20));
        let now = T0 + DAY_MS / 2;
        assert_eq!(fixture.classes(now, ON), [C::Running, C::Running]);
        // A run that has its terminal but no release yet holds nobody.
        fixture.facts.runs.get_mut(&next.0).expect("run").terminal = Some((30, at(21)));
        assert_eq!(fixture.classes(now, ON), [C::Default, C::Running]);

        // A Lab debug-package run without any terminal holds nobody either.
        fixture.facts.runs.get_mut(&next.0).expect("run").terminal = None;
        assert_eq!(fixture.classes(now, ON), [C::Running, C::Running]);
        fixture.facts.runs.get_mut(&next.0).expect("run").debug = true;
        assert_eq!(fixture.classes(now, ON), [C::Default, C::Running]);
    }

    #[test]
    fn a_run_cut_short_keeps_its_last_thirty_seconds_under_the_restart() {
        // A scheduled run with a granted lease and frames, without terminal or release.
        let mut fixture = Fixture::new();
        let previous = fixture.run(5);
        fixture.frame(Some(previous), at(0));
        fixture.close(previous, 8, at(1));
        let cut = fixture.run(10);
        let frames = (2..7)
            .map(|step| fixture.frame(Some(cut), at(step * 10)))
            .collect::<Vec<_>>();
        fixture.chain(&frames);
        let now = at(3_600);
        assert!(
            fixture
                .classes(now, ON)
                .iter()
                .all(|class| *class == C::Running)
        );
        fixture.start(200, at(600), EventType::RuntimeTakeover);

        let (view, points) = fixture.view(now, ON);
        assert_eq!(points.len(), 1);
        assert!(points[0].epoch_end);
        assert_eq!(points[0].sequence, 200);
        assert_eq!(points[0].event_type, EventType::RuntimeTakeover);
        assert_eq!(points[0].at_unix_ms, at(60));
        assert_eq!(points[0].leaf, "node_a_b-200-runtime_takeover");
        // Its window is the run's last 30 s; without a capture summary, nothing in it is
        // dropped and nothing of the run is a duplicate. The neighbour settles at the restart.
        assert_eq!(
            view.iter().map(|frame| frame.class).collect::<Vec<_>>(),
            [
                C::Default,
                C::Default,
                C::Error,
                C::Error,
                C::Error,
                C::Error
            ]
        );
        let folder = view[frames[4]].folder.as_ref().expect("error folder");
        assert_eq!(
            (folder.date.as_str(), folder.leaf.as_str()),
            ("2026-10-12", "node_a_b-200-runtime_takeover")
        );
        assert_eq!(view[frames[1]].entry_unix_ms, Some(at(600)));

        // A direct run settled by the recovered pair, both Warnings stamped at the restart:
        // they fall in the run's last 30 s, which the epoch-end point owns.
        let mut fixture = Fixture::new();
        let direct = fixture.run(10);
        let frames = (0..3)
            .map(|step| fixture.frame(Some(direct), at(step * 10)))
            .collect::<Vec<_>>();
        fixture.start(100, at(900), EventType::RuntimeStarted);
        fixture.point(101, at(900), Some(direct.0), "task_terminal_intent");
        fixture.point(
            102,
            at(900),
            Some(direct.0),
            "contained_task_recovered_after_restart",
        );
        fixture.facts.runs.get_mut(&direct.0).expect("run").terminal = Some((102, at(900)));
        let (view, points) = fixture.view(at(3_600), ON);
        assert_eq!(
            points
                .iter()
                .map(|point| (point.sequence, point.at_unix_ms))
                .collect::<Vec<_>>(),
            [(100, at(20)), (101, at(20)), (102, at(20))]
        );
        for index in frames {
            assert_eq!(view[index].class, C::Error);
            assert_eq!(
                view[index].folder.as_ref().expect("error folder").leaf,
                "node_a_b-100-runtime_started"
            );
        }

        // A run two epochs back gets the point of the start that ended its own epoch.
        let mut fixture = Fixture::new();
        let old = fixture.run(10);
        fixture.frame(Some(old), at(0));
        fixture.start(100, at(500), EventType::RuntimeStarted);
        fixture.start(300, at(5_000), EventType::RuntimeTakeover);
        let (view, points) = fixture.view(at(9_000), ON);
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].leaf, "node_a_b-100-runtime_started");
        assert_eq!(view[0].class, C::Error);
    }

    #[test]
    fn the_lowest_sequence_window_owns_the_frame_and_names_its_folder() {
        let mut fixture = Fixture::new();
        let run = fixture.run(5);
        let early = fixture.frame(Some(run), at(0));
        let middle = fixture.frame(Some(run), at(10));
        let late = fixture.frame(Some(run), at(30));
        fixture.close(run, 200, at(40));
        // Point 120 at 10 s, point 110 at 35 s and point 150 at 40 s.
        fixture.point(120, at(10), Some(run.0), "return_home_failed");
        fixture.point(110, at(35), Some(run.0), "contained_task_page_unknown");
        fixture.point(150, at(40), Some(run.0), "contained_task_step_unconfirmed");

        let (view, points) = fixture.view(T0 + DAY_MS, ON);
        assert_eq!(
            points
                .iter()
                .map(|point| point.sequence)
                .collect::<Vec<_>>(),
            [110, 120, 150]
        );
        let leaf = |index: usize| {
            view[index]
                .folder
                .as_ref()
                .expect("error folder")
                .leaf
                .clone()
        };
        assert_eq!(leaf(early), "node_a_b-120-return_home_failed");
        assert_eq!(leaf(middle), "node_a_b-110-contained_task_page_unknown");
        assert_eq!(leaf(late), "node_a_b-110-contained_task_page_unknown");
        assert_eq!(view[middle].windows, [0, 1, 2]);
        assert_eq!(view[late].windows, [0, 2]);
        assert_eq!(
            view[early].folder.as_ref().expect("error folder").file_name,
            format!("061502-117_{}.png", text(&fixture.artifact(early)))
        );
        assert_eq!(local_date(T0, utc).as_deref(), Some("2026-10-11"));
        assert_eq!(local_date(T0, jst).as_deref(), Some("2026-10-12"));
        // A named instant whose local offset is unknown fails the whole view.
        assert!(
            classify(
                &fixture.facts,
                &fixture.frame_artifacts,
                &fixture.frames,
                ON,
                unknown
            )
            .is_none()
        );
    }

    #[test]
    fn cached_classes_follow_the_clock_only() {
        let mut fixture = Fixture::new();
        let run = fixture.run(5);
        fixture.frame(Some(run), at(0));
        fixture.close(run, 20, at(1));
        let classified = classify(
            &fixture.facts,
            &fixture.frame_artifacts,
            &fixture.frames,
            ON,
            jst,
        )
        .expect("local time");
        // Before it settles the frame runs; after, it is Default and due a day after its entry.
        let early = classified.at(9, at(30));
        assert_eq!(early.through_sequence, 9);
        assert_eq!(early.frames[0].class, C::Running);
        assert_eq!(early.frames[0].due_unix_ms, None);
        let late = classified.at(9, at(61));
        assert_eq!(late.frames[0].class, C::Default);
        assert_eq!(late.frames[0].due_unix_ms, Some(at(1) + DAY_MS));
        assert_eq!(late.evaluated_at_unix_ms, at(61));
    }

    #[test]
    fn the_leaf_code_comes_from_the_owner_event_then_a_preceding_failure_terminal() {
        let value = |text: &str| Some(text.to_owned());
        assert_eq!(
            leaf_name(
                value("own_failure"),
                value("own"),
                value("terminal"),
                EventType::TaskFailed
            ),
            "own_failure"
        );
        assert_eq!(
            leaf_name(
                None,
                value("own"),
                value("terminal"),
                EventType::RuntimeFailed
            ),
            "own"
        );
        assert_eq!(
            leaf_name(
                None,
                None,
                value("terminal"),
                EventType::PolicyExecutionRecorded
            ),
            "terminal"
        );
        assert_eq!(
            leaf_name(None, None, None, EventType::PolicyExecutionRecorded),
            "policy_execution_recorded"
        );
        assert_eq!(
            leaf_name(value("a.b c/d"), None, None, EventType::TaskFailed),
            "a_b_c_d"
        );
        assert_eq!(
            leaf_name(Some("x".repeat(70)), None, None, EventType::TaskFailed).len(),
            64
        );
        assert_eq!(
            type_leaf_name(EventType::RuntimeTakeover),
            "runtime_takeover"
        );
        let payload = serde_json::json!({
            "family": "policy",
            "payload": {
                "diagnostic_code": "diagnostic",
                "rejections": [{ "code": "first" }, { "code": "second" }],
                "failure": { "failure_code": "failed" }
            }
        });
        assert_eq!(first_string(&payload, "code").as_deref(), Some("first"));
        assert_eq!(nested_string(&payload, "rejection", "code"), None);
        // A dispatch rejection names its leaf by `rejection.code`, not by an earlier `code`.
        let rejected = serde_json::json!({
            "family": "policy",
            "payload": {
                "kind": "dispatch_rejected",
                "data": {
                    "candidates": [{ "code": "eligible" }],
                    "rejection": { "code": "instance_busy", "detail": { "code": "inner" } }
                }
            }
        });
        assert_eq!(first_string(&rejected, "code").as_deref(), Some("eligible"));
        assert_eq!(
            nested_string(&rejected, "rejection", "code").as_deref(),
            Some("instance_busy")
        );
        assert_eq!(
            first_string(&payload, "failure_code").as_deref(),
            Some("failed")
        );
        assert_eq!(first_string(&payload, "absent"), None);
    }
}
