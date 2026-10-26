// SPDX-License-Identifier: AGPL-3.0-only

//! #422 LN-7c V (`rd-1026/ln7c/final-query.md` §7 row V, §6): `ledger-maintenance verify` runs the
//! frame cleaner's statements W, L and K, as the cleaner runs them, against the full view (LN-7b's
//! `classify` over every verified record) over the same root, the same `artifacts\` and `kept\`
//! listings and one clock. Offline only: the Runtime never runs it.
//!
//! The reference is the full view with the rules the ruling changed:
//! - **E1** (ruling 70): a default frame is due 1 day, a resource frame 7 days after its ingestion
//!   time, so the reference's action is computed from `created_at_unix_ms`, not from the entry;
//! - **E11** (ruling 70): a Lab or error frame that an agent or a person confirmed reading at or
//!   after its capture is removed (`remove_confirmed`), and a kept frame is removed once confirmed
//!   (`remove_confirmed`) or at `frame_retention_evidence_max_age_days` (`remove_expired`). The
//!   confirmations are read from the verified records themselves, not from the frame index.
//!
//! The differences §6 calls safe are not reported when the records show their cause, and each is
//! counted ([`FrameQueryCheck::accounted`]):
//! - **E3**: the query classes Lab a frame the view does not, and a Lab pin of the frame was
//!   released (the view's release comparison);
//! - **E4**: the query classes error a frame the view classes resource, duplicate or default, and
//!   a Warning-or-higher or `task.failed` record names the frame (by link, artifact or index row);
//! - **E5**: the view classes duplicate a frame the query settles otherwise, and a marker conflict
//!   (a second marker on the frame or on its neighbour's side) touches the frame.
//!
//! **E12** cannot arise: the check examines every listed frame at one clock, without the cleaner's
//! schedule. Any other difference is `ledger_index_mismatch` of `verify_frame_query` with the
//! frame's `artifact_id` and the answer's member that differs (`field`, a JSON pointer: `/class`,
//! `/action`, `/created_at_unix_ms`, `/run_id`, `/folder`, or empty when only one side answers).

use super::super::frame_refs::{ARTIFACT, FRAME, READ_CONFIRMED, RUN, frame_refs};
use super::super::views;
use super::{FrameQuery, FrameQueryAction, FrameQueryOutcome, FrameQuerySettings, identifier};
use crate::PersistedEvent;
use crate::codes::{LedgerCode, LedgerLocation};
use crate::global::retention::{
    DAY_MS, READING_DAYS, SETTLE_AGE_MS, full_frame_view, live_frame_ids,
};
use crate::global::{
    FrameRetentionClass, FrameRetentionFrame, FrameRetentionSwitches, GlobalLedgerError,
    GlobalLedgerResult,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ArtifactId, ArtifactKind, ArtifactPinReason, ArtifactRetentionFact, CapturePayload,
    EventPayload, EventSeverity, EventType, FrameId, RunId,
};
use actingcommand_runtime_database::RuntimeDatabase;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

/// #422 LN-7c V: what `ledger-maintenance verify` runs the statements and the full view with: one
/// clock, the two dedup switches, `frame_retention_evidence_max_age_days`, and
/// `ledger_observer.frame_query_chunk_frames` and `ledger_observer.read_check_vm_steps`, as the
/// caller has them in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameQueryCheckSettings {
    pub now_unix_ms: u64,
    pub switches: FrameRetentionSwitches,
    pub evidence_max_age_days: u64,
    pub chunk_frames: usize,
    pub check_vm_steps: u32,
}

/// #422 LN-7c V: what the check compared.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameQueryCheck {
    /// Frames of the live listing that statement L or the full view knows.
    pub live_frames: u64,
    /// Frames of the kept listing that statement K or the verified records know.
    pub kept_frames: u64,
    /// Differences §6 calls safe (E3, E4, E5) whose cause the records show: not reported.
    pub accounted: u64,
}

fn location() -> LedgerLocation {
    LedgerLocation::VerifyFrameQuery
}

fn deadline_reached() -> GlobalLedgerError {
    GlobalLedgerError::request(
        LedgerCode::LedgerReadBudgetExceeded.code(),
        location().location(),
    )
    .with_detail("phase=verify_frame_query deadline reached".to_owned())
}

/// A statement's answer; a frame index that does not cover the ledger is a finding (the cleaner
/// would decide nothing), and a stopped statement the deadline's.
fn answered<T>(outcome: FrameQueryOutcome<T>) -> GlobalLedgerResult<T> {
    match outcome {
        FrameQueryOutcome::Answered { value, .. } => Ok(value),
        FrameQueryOutcome::Incomplete => Err(GlobalLedgerError::request(
            LedgerCode::LedgerIndexMismatch.code(),
            location().location(),
        )
        .with_detail(
            "phase=verify_frame_query table=ledger_frame_refs frame_refs_incomplete".to_owned(),
        )),
        FrameQueryOutcome::Stopped => Err(deadline_reached()),
    }
}

/// What the check reads from the verified records besides the full view: each verified capture
/// frame's ingestion times and runs (K's rows), the read confirmations (E11), the artifacts whose
/// Lab pin was released (E3), what Warning-or-higher and `task.failed` records name (E4), and the
/// markers (E5).
#[derive(Default)]
struct Records {
    verified: BTreeMap<ArtifactId, BTreeSet<(u64, Option<RunId>)>>,
    /// `(target kind, target id)` of a confirmation and the latest confirmation's time.
    confirmed: BTreeMap<(&'static str, String), u64>,
    lab_released: BTreeSet<ArtifactId>,
    named: BTreeSet<(&'static str, String)>,
    /// A representative's preserved frames, and a preserved frame's representatives.
    successors: BTreeMap<FrameId, Vec<FrameId>>,
    predecessors: BTreeMap<FrameId, Vec<FrameId>>,
}

impl Records {
    fn read(events: &[PersistedEvent], deadline: Instant) -> GlobalLedgerResult<Self> {
        let mut records = Self::default();
        let mut lab_pins = BTreeSet::new();
        for event in events {
            if Instant::now() >= deadline {
                return Err(deadline_reached());
            }
            if event.event_type() == EventType::ArtifactVerified {
                for artifact in event.artifacts() {
                    if artifact.kind() == ArtifactKind::CaptureFrame {
                        records
                            .verified
                            .entry(*artifact.artifact_id())
                            .or_default()
                            .insert((artifact.created_at_unix_ms(), artifact.run_id().copied()));
                    }
                }
            }
            match event.payload().artifact_retention() {
                Some(ArtifactRetentionFact::PinRecorded(pin))
                    if pin.reason == ArtifactPinReason::Lab =>
                {
                    lab_pins.insert(*event.event_id());
                }
                Some(ArtifactRetentionFact::PinReleased(release))
                    if lab_pins.contains(&release.pin.event_id) =>
                {
                    records
                        .lab_released
                        .insert(release.identity.artifact.artifact_id);
                }
                _ => {}
            }
            if let EventPayload::Capture(CapturePayload::DedupWindow(window)) = event.payload()
                && let (Some(preserved), Some(representative)) =
                    (window.preserved_frame_id(), event.links().frame_id())
            {
                records
                    .successors
                    .entry(*representative)
                    .or_default()
                    .push(*preserved);
                records
                    .predecessors
                    .entry(*preserved)
                    .or_default()
                    .push(*representative);
            }
            let naming = (event.event_type() == EventType::TaskFailed
                || event.severity() >= EventSeverity::Warning)
                && event.payload().artifact_retention().is_none();
            if naming {
                if let Some(frame) = event.links().frame_id() {
                    records.named.insert((FRAME, views::key(frame)?));
                }
                for artifact in event.artifacts() {
                    records
                        .named
                        .insert((ARTIFACT, views::key(artifact.artifact_id())?));
                }
            }
            for frame_ref in frame_refs(event)? {
                let target = (frame_ref.target_kind, frame_ref.target_id);
                if frame_ref.pointer == READ_CONFIRMED {
                    let latest = records.confirmed.entry(target).or_insert(0);
                    *latest = (*latest).max(event.timestamp_unix_ms());
                } else if naming {
                    records.named.insert(target);
                }
            }
        }
        Ok(records)
    }

    /// Whether a confirmation of `artifact` or of its `run` was recorded at or after `at`.
    fn confirmed(
        &self,
        artifact: ArtifactId,
        run: Option<RunId>,
        at: u64,
    ) -> GlobalLedgerResult<bool> {
        let by = |target: (&'static str, String)| {
            self.confirmed
                .get(&target)
                .is_some_and(|latest| *latest >= at)
        };
        Ok(by((ARTIFACT, views::key(artifact)?))
            || match run {
                Some(run) => by((RUN, views::key(run)?)),
                None => false,
            })
    }

    /// A marker conflict touches `frame`: it is preserved or represented twice, or a frame it
    /// is marked with is represented or preserved twice on the side that faces it.
    fn contested(&self, frame: FrameId) -> bool {
        let twice = |sides: &BTreeMap<FrameId, Vec<FrameId>>, id: &FrameId| {
            sides.get(id).is_some_and(|ids| ids.len() > 1)
        };
        twice(&self.predecessors, &frame)
            || twice(&self.successors, &frame)
            || self
                .predecessors
                .get(&frame)
                .into_iter()
                .flatten()
                .any(|representative| twice(&self.successors, representative))
            || self
                .successors
                .get(&frame)
                .into_iter()
                .flatten()
                .any(|preserved| twice(&self.predecessors, preserved))
    }

    /// Whether E3, E4 or E5 explains the query's class `query` of `frame`, which the view classes
    /// otherwise, by a cause the records show.
    fn accounted(
        &self,
        query: FrameRetentionClass,
        frame: &FrameRetentionFrame,
    ) -> GlobalLedgerResult<bool> {
        use FrameRetentionClass as C;
        let artifact = frame.reference.artifact_id;
        let frame_id = frame.reference.frame_id;
        if query == C::Lab
            && !matches!(frame.class, C::Lab | C::Running)
            && self.lab_released.contains(&artifact)
        {
            return Ok(true);
        }
        if query == C::Error && matches!(frame.class, C::Resource | C::Duplicate | C::Default) {
            let named = self.named.contains(&(ARTIFACT, views::key(artifact)?))
                || match frame_id {
                    Some(frame_id) => self.named.contains(&(FRAME, views::key(frame_id)?)),
                    None => false,
                };
            if named {
                return Ok(true);
            }
        }
        Ok(frame.class == C::Duplicate
            && !matches!(query, C::Duplicate | C::Running)
            && frame_id.is_some_and(|frame_id| self.contested(frame_id)))
    }
}

/// The view's action for `frame` at `now` with E1's due times and E11's confirmed evidence frames.
fn reference_action(frame: &FrameRetentionFrame, confirmed: bool, now: u64) -> FrameQueryAction {
    let at = frame.reference.created_at_unix_ms;
    match frame.class {
        FrameRetentionClass::Running => FrameQueryAction::Running,
        FrameRetentionClass::Lab | FrameRetentionClass::Error if confirmed => {
            FrameQueryAction::RemoveConfirmed
        }
        FrameRetentionClass::Lab => FrameQueryAction::MoveLab,
        FrameRetentionClass::Error => FrameQueryAction::MoveError,
        FrameRetentionClass::Resource if at.saturating_add(READING_DAYS * DAY_MS) <= now => {
            FrameQueryAction::Remove
        }
        FrameRetentionClass::Resource => FrameQueryAction::KeepResource,
        FrameRetentionClass::Duplicate => FrameQueryAction::RemoveDuplicate,
        FrameRetentionClass::Default if at.saturating_add(DAY_MS) <= now => {
            FrameQueryAction::Remove
        }
        FrameRetentionClass::Default => FrameQueryAction::Keep,
    }
}

/// One difference: the statement, the frame, the answer's member and both answers.
struct Difference {
    statement: &'static str,
    artifact: ArtifactId,
    field: &'static str,
    query: String,
    reference: String,
}

/// The differences found, the first kept for the finding.
#[derive(Default)]
struct Found {
    first: Option<Difference>,
    differences: u64,
    check: FrameQueryCheck,
}

impl Found {
    fn differ(
        &mut self,
        statement: &'static str,
        artifact: ArtifactId,
        field: &'static str,
        query: String,
        reference: String,
    ) {
        self.differences += 1;
        if self.first.is_none() {
            self.first = Some(Difference {
                statement,
                artifact,
                field,
                query,
                reference,
            });
        }
    }

    fn finish(self) -> GlobalLedgerResult<FrameQueryCheck> {
        let Some(Difference {
            statement,
            artifact,
            field,
            query,
            reference,
        }) = self.first
        else {
            return Ok(self.check);
        };
        let artifact = views::key(artifact)?;
        Err(GlobalLedgerError::request(
            LedgerCode::LedgerIndexMismatch.code(),
            location().location(),
        )
        .with_detail(format!(
            "phase=verify_frame_query statement={statement} artifact_id={artifact} field={field} query={query} reference={reference} differences={}",
            self.differences
        ))
        .with_outcome(
            Outcome::new(LedgerCode::LedgerIndexMismatch)
                .operation(location())
                .artifact_id(artifact)
                .field(field),
        ))
    }
}

/// The artifact ids of the kept listing, each once.
fn kept_frames(root: &Path) -> GlobalLedgerResult<BTreeSet<ArtifactId>> {
    Ok(actingcommand_artifact_store::list_kept_folders(root)
        .map_err(|error| {
            GlobalLedgerError::request(LedgerCode::LedgerIo.code(), location().location())
                .with_detail(error.to_string())
        })?
        .into_iter()
        .flat_map(|leaf| leaf.files)
        .filter_map(|file| {
            file.object_file
                .strip_suffix(".png")
                .and_then(identifier::<ArtifactId>)
        })
        .collect())
}

/// #422 LN-7c V: the check of `ledger-maintenance verify` on `database`, whose verified records
/// are `events`, within `deadline` (module documentation). Reads no artifact material: the
/// listings are names only.
pub(in crate::global) fn check_frame_queries(
    database: Arc<RuntimeDatabase>,
    events: &[PersistedEvent],
    check: &FrameQueryCheckSettings,
    deadline: Instant,
) -> GlobalLedgerResult<FrameQueryCheck> {
    let now = check.now_unix_ms;
    let root = database.root().to_path_buf();
    let view = full_frame_view(events, now, check.switches, &mut |_| {
        if Instant::now() >= deadline {
            return Err(deadline_reached());
        }
        Ok(())
    })?;
    let records = Records::read(events, deadline)?;
    let reference = view
        .frames
        .iter()
        .map(|frame| (frame.reference.artifact_id, frame))
        .collect::<BTreeMap<_, _>>();
    let live = live_frame_ids(&root)
        .map_err(|error| {
            GlobalLedgerError::io(LedgerCode::LedgerIo.code(), location().location(), &error)
        })?
        .into_iter()
        .collect::<Vec<_>>();
    let kept = kept_frames(&root)?.into_iter().collect::<Vec<_>>();
    let settings = FrameQuerySettings {
        now_unix_ms: now,
        switches: check.switches,
        evidence_max_age_ms: check.evidence_max_age_days.saturating_mul(DAY_MS),
    };
    let mut query = FrameQuery::open(
        database,
        Arc::new(move || Instant::now() >= deadline),
        check.check_vm_steps,
    )?;
    let mut found = Found::default();
    let chunk_frames = check.chunk_frames.max(1);

    // W once, then L per chunk of the live listing, all at `now`.
    let waits = answered(query.waits()?)?;
    for chunk in live.chunks(chunk_frames) {
        let mut answers = BTreeMap::<ArtifactId, Vec<_>>::new();
        for answer in answered(query.live(chunk, &waits, &settings)?)? {
            answers.entry(answer.artifact_id).or_default().push(answer);
        }
        for id in chunk {
            let frame = reference.get(id).copied();
            let answers = answers.remove(id).unwrap_or_default();
            if frame.is_none() && answers.is_empty() {
                continue;
            }
            found.check.live_frames += 1;
            let Some(frame) = frame else {
                found.differ(
                    "L",
                    *id,
                    "",
                    answers[0].action.as_str().to_owned(),
                    "none".to_owned(),
                );
                continue;
            };
            if answers.is_empty() {
                // Statement L gives no row for a frame before its settle age; the view classes it
                // running then.
                let settling = frame.class == FrameRetentionClass::Running
                    && frame
                        .reference
                        .created_at_unix_ms
                        .saturating_add(SETTLE_AGE_MS)
                        > now;
                if !settling {
                    found.differ(
                        "L",
                        *id,
                        "",
                        "none".to_owned(),
                        format!("{:?}", frame.class),
                    );
                }
                continue;
            }
            for answer in answers {
                if answer.class != frame.class {
                    if records.accounted(answer.class, frame)? {
                        found.check.accounted += 1;
                    } else {
                        found.differ(
                            "L",
                            *id,
                            "/class",
                            format!("{:?}", answer.class),
                            format!("{:?}", frame.class),
                        );
                    }
                    continue;
                }
                let at = frame.reference.created_at_unix_ms;
                if answer.created_at_unix_ms != at {
                    found.differ(
                        "L",
                        *id,
                        "/created_at_unix_ms",
                        answer.created_at_unix_ms.to_string(),
                        at.to_string(),
                    );
                    continue;
                }
                let confirmed = records.confirmed(*id, frame.run_id, at)?;
                let expected = reference_action(frame, confirmed, now);
                if answer.action != expected {
                    found.differ(
                        "L",
                        *id,
                        "/action",
                        answer.action.as_str().to_owned(),
                        expected.as_str().to_owned(),
                    );
                    continue;
                }
                if answer.action.moves() {
                    let folder = match query.frame(*id)? {
                        Some(verified) => Some(
                            query.folder(&verified, answer.action == FrameQueryAction::MoveLab)?,
                        ),
                        None => None,
                    };
                    if folder != frame.folder {
                        found.differ(
                            "L",
                            *id,
                            "/folder",
                            format!("{folder:?}"),
                            format!("{:?}", frame.folder),
                        );
                    }
                }
            }
        }
    }

    // K per chunk of the kept listing: E11 over the verified records.
    for chunk in kept.chunks(chunk_frames) {
        let mut answers = BTreeMap::<ArtifactId, Vec<_>>::new();
        for answer in answered(query.kept(chunk, &settings)?)? {
            answers.entry(answer.artifact_id).or_default().push(answer);
        }
        for id in chunk {
            let verified = records.verified.get(id);
            let answers = answers.remove(id).unwrap_or_default();
            // A kept file this ledger does not know: neither side touches it.
            if verified.is_none() && answers.is_empty() {
                continue;
            }
            found.check.kept_frames += 1;
            let Some(verified) = verified else {
                found.differ(
                    "K",
                    *id,
                    "",
                    answers[0].action.as_str().to_owned(),
                    "none".to_owned(),
                );
                continue;
            };
            if answers.is_empty() {
                found.differ("K", *id, "", "none".to_owned(), "verified".to_owned());
                continue;
            }
            for answer in answers {
                let at = answer.created_at_unix_ms;
                if !verified.contains(&(at, answer.run_id)) {
                    let field = if verified.iter().any(|(known, _)| *known == at) {
                        "/run_id"
                    } else {
                        "/created_at_unix_ms"
                    };
                    found.differ(
                        "K",
                        *id,
                        field,
                        format!("{at} {:?}", answer.run_id),
                        format!("{verified:?}"),
                    );
                    continue;
                }
                let expected = if at.saturating_add(settings.evidence_max_age_ms) <= now {
                    FrameQueryAction::RemoveExpired
                } else if records.confirmed(*id, answer.run_id, at)? {
                    FrameQueryAction::RemoveConfirmed
                } else {
                    FrameQueryAction::Keep
                };
                if answer.action != expected {
                    found.differ(
                        "K",
                        *id,
                        "/action",
                        answer.action.as_str().to_owned(),
                        expected.as_str().to_owned(),
                    );
                }
            }
        }
    }
    found.finish()
}
