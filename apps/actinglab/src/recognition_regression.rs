// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #331 RSU-1 (additions-v012 §1.5b row RSU-1; forced-compromises mapping T-01):
//! `actinglab recognition-regression --state-root <Runtime state root> --evidence <file>
//! [--evidence <file> ...] (--package <path> | --zip <path>)`, the frame-replay recognition
//! regression of a new package or a new version of one.
//!
//! The frames are the ones the run's ledger kept, named by the per-step frame references of
//! MCP-6 (`actingctl run evidence` / `ac_run_evidence`, saved as they answered; several pages of
//! one run may be given). Each reference (`<sequence>:<event_id>:<artifact_id>`) is found among
//! the run's verified capture frames in the state root's ledger, read offline, and evaluated
//! with the given package's recognition. The original record of a frame is the ledger's
//! recognition fact recorded on that frame: each target with whether it passed. For every
//! target of that record, the replay gives whether the package now finds it, its score and
//! whether that agrees with the original record; per target, the counts and the error rate
//! against the original record follow (disagreements per mille of the frames compared).
//!
//! The command only reads: it opens the ledger read-only, never connects to a Runtime, touches
//! no device and writes nothing. A frame whose material the retention rules removed (#375) is
//! listed as `material_missing`; a frame without a recognition fact is listed as
//! `no_original_record`; neither is compared. It reports and decides nothing: it exits 0
//! whenever the comparison was made, whatever the error rate (whether it becomes a gate is an
//! open policy question), and it is not wired into any CI.

use super::codes::ActinglabCode;
use super::{CliError, CliOutcome, ErrorKind, FlagArgs, GlobalOptions};
use actingcommand_artifact_store::{read_projected_verified, verify_projected_read_only};
use actingcommand_contract::{
    ArtifactKind, ArtifactMediaType, ArtifactProducer, EventPayload, EventQuery, EventType,
    FrameId, ProjectedArtifactReference, ProjectedEvent, ProjectionPayload, ProjectionProfile,
    RecognizedTarget, RequestId, TaskPayload, TaskSemanticFact,
};
use actingcommand_ledger::{
    GlobalLedger, GlobalLedgerEvidence, GlobalLedgerEvidenceConfig, project_subscription_event,
};
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::{RecognitionEvaluator, TargetEvaluation, TargetKind};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// One page of a run's evidence is at most 100 steps of frame references; a saved page stays
/// well inside this bound.
const MAX_EVIDENCE_BYTES: u64 = 16 * 1024 * 1024;
/// The ledger is read in pages of this many events.
const EVENT_PAGE: usize = 256;

pub(super) fn run_recognition_regression(
    global: &GlobalOptions,
    flags: &FlagArgs,
) -> CliOutcome<Value> {
    let args = RegressionArgs::parse(global, flags)?;
    let evidence = read_evidence(&args.evidence)?;
    let bundle = crate::contained_resources::load(flags, "recognition-regression")?;
    let evaluator = bundle.loaded_bundle().evaluator().ok_or_else(|| {
        CliError::package_invalid("the package declares no recognition pack to replay")
    })?;
    let kept = read_kept_frames(&args.state_root, &evidence)?;
    // Workflow #396 LAB-3: an OCR target of the comparison needs the Runtime's OCR provider.
    let compared_targets = kept
        .iter()
        .filter_map(|frame| match &frame.source {
            FrameSource::Kept { original, .. } => Some(original),
            _ => None,
        })
        .flatten()
        .map(|target| target.target_id.as_str())
        .filter(|target| has_target(evaluator, target))
        .collect::<Vec<_>>();
    crate::offline_ocr::lab_ocr().require(evaluator.pack(), compared_targets)?;
    let mut report = compare(evaluator, &kept);
    report.insert("mode".to_owned(), json!("offline_recognition_regression"));
    report.insert("executed".to_owned(), json!(false));
    report.insert("request_id".to_owned(), json!(evidence.request_id));
    report.insert(
        "package_id".to_owned(),
        json!(
            bundle
                .loaded_bundle()
                .control()
                .and_then(|control| control.get("package_id"))
                .cloned()
        ),
    );
    report.insert("ocr".to_owned(), crate::offline_ocr::status());
    report.insert("production_global_ledger_written".to_owned(), json!(false));
    Ok(Value::Object(report))
}

struct RegressionArgs {
    state_root: PathBuf,
    evidence: Vec<PathBuf>,
}

impl RegressionArgs {
    fn parse(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Self> {
        flags.expect_positionals("recognition-regression", 0)?;
        if global.run_root.is_some()
            || global.instance.is_some()
            || global.instances.is_some()
            || global.profile.is_some()
            || global.resource_root.is_some()
            || global.dry_run
            || global.verbose
            || global.quiet
            || global.game.is_some()
            || global.server.is_some()
            || global.runtime_endpoint.is_some()
            || global.capture_backend.is_some()
            || global.touch_backend.is_some()
        {
            return Err(CliError::new(
                ErrorKind::UsageValidation,
                "offline_device_scope_forbidden",
                "recognition-regression accepts no global Runtime, device, selector, backend, resource, or execution flags",
                &[],
            ));
        }
        if let Some(name) = flags.flags.keys().find(|name| {
            !matches!(
                name.as_str(),
                "--state-root" | "--evidence" | "--package" | "--zip"
            )
        }) {
            return Err(CliError::usage(format!(
                "recognition-regression does not accept {name}"
            )));
        }
        let state_root = match flags.values("--state-root").as_slice() {
            [value] if value != "true" && !value.trim().is_empty() => PathBuf::from(value),
            _ => {
                return Err(CliError::usage(
                    "recognition-regression requires one --state-root <Runtime state root>",
                ));
            }
        };
        let evidence = flags.values("--evidence");
        if evidence.is_empty()
            || evidence
                .iter()
                .any(|value| value == "true" || value.trim().is_empty())
        {
            return Err(CliError::usage(
                "recognition-regression requires --evidence <file>, once per evidence page",
            ));
        }
        Ok(Self {
            state_root,
            evidence: evidence.into_iter().map(PathBuf::from).collect(),
        })
    }
}

/// One page of a run's evidence as `actingctl run evidence` or `ac_run_evidence` answered it.
/// Only the run and the steps' frame references are read; every other member (the CLI line's
/// `outcome`, `run_id`, `task_id`, a step's `page`, `targets` and `action`, `final`,
/// `next_step`, `overlay`, `truncated`) is left unread.
#[derive(Deserialize)]
struct EvidencePage {
    request_id: RequestId,
    steps: Vec<EvidenceStep>,
}

#[derive(Deserialize)]
struct EvidenceStep {
    step_index: u32,
    frames: Vec<EvidenceFrame>,
}

#[derive(Deserialize)]
struct EvidenceFrame {
    #[serde(rename = "ref")]
    reference: String,
}

/// The evidence of one run: its request and every frame reference, once, with the steps that
/// name it, in the order the pages give them.
struct Evidence {
    request_id: RequestId,
    frames: Vec<(String, Vec<u32>)>,
}

fn read_evidence(paths: &[PathBuf]) -> CliOutcome<Evidence> {
    let mut request = None;
    let mut frames: Vec<(String, Vec<u32>)> = Vec::new();
    for path in paths {
        let page = read_evidence_page(path)?;
        match request {
            None => request = Some(page.request_id),
            Some(request_id) if request_id == page.request_id => {}
            Some(_) => {
                return Err(evidence_invalid(
                    path,
                    "a page of another run than the first page's".to_owned(),
                ));
            }
        }
        for step in page.steps {
            for frame in step.frames {
                match frames
                    .iter_mut()
                    .find(|(reference, _)| *reference == frame.reference)
                {
                    Some((_, steps)) if steps.contains(&step.step_index) => {}
                    Some((_, steps)) => steps.push(step.step_index),
                    None => frames.push((frame.reference, vec![step.step_index])),
                }
            }
        }
    }
    let request_id = request.ok_or_else(|| CliError::usage("no --evidence page was given"))?;
    if frames.is_empty() {
        return Err(evidence_invalid(
            &paths[0],
            "the evidence names no frame".to_owned(),
        ));
    }
    Ok(Evidence { request_id, frames })
}

fn read_evidence_page(path: &Path) -> CliOutcome<EvidencePage> {
    let unreadable = |error: std::io::Error| {
        CliError::new(
            ErrorKind::UsageValidation,
            ActinglabCode::RecognitionRegressionEvidenceUnreadable.as_str(),
            format!("failed to read --evidence {}: {error}", path.display()),
            &[],
        )
    };
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(unreadable)?
        .take(MAX_EVIDENCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    if bytes.len() as u64 > MAX_EVIDENCE_BYTES {
        return Err(evidence_invalid(
            path,
            format!("larger than {MAX_EVIDENCE_BYTES} bytes"),
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| evidence_invalid(path, format!("not a run evidence answer: {error}")))
}

fn evidence_invalid(path: &Path, problem: String) -> CliError {
    CliError::new(
        ErrorKind::UsageValidation,
        ActinglabCode::RecognitionRegressionEvidenceInvalid.as_str(),
        format!("--evidence {}: {problem}", path.display()),
        &[],
    )
}

/// One referenced frame and what the replay compares on it.
struct KeptFrame {
    reference: String,
    steps: Vec<u32>,
    frame_id: Option<FrameId>,
    source: FrameSource,
}

enum FrameSource {
    /// The frame and the targets of the recognition fact recorded on it.
    Kept {
        scene: Box<Scene>,
        original: Vec<RecognizedTarget>,
    },
    /// The retention rules removed the frame's material.
    MaterialMissing,
    /// The run recorded no recognition on the frame.
    NoOriginalRecord,
}

/// How a referenced frame was replayed (`frames[].status`).
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum FrameStatus {
    Compared,
    MaterialMissing,
    NoOriginalRecord,
}

/// How a target of a frame's original record was replayed (`frames[].targets[].status`).
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum TargetStatus {
    /// Evaluated with the package; `matched` is whether it passed now.
    Compared,
    /// The package declares no target of this id.
    Absent,
    /// The package's evaluation failed; `failure` is its error.
    Failed,
}

/// Every referenced frame of the run, read from the state root's ledger: the run's verified
/// capture frames by their references, and the last recognition fact recorded on each frame.
fn read_kept_frames(root: &Path, evidence: &Evidence) -> CliOutcome<Vec<KeptFrame>> {
    let ledger_failure = |problem: String| {
        CliError::new(
            ErrorKind::UsageValidation,
            ActinglabCode::RecognitionRegressionLedgerUnreadable.as_str(),
            format!("--state-root {}: {problem}", root.display()),
            &[],
        )
    };
    // Only a segment root calls the verifier: a frame deleted by hand stays unrecorded, any
    // other artifact failure fails the reading.
    let mut artifact_failure = None;
    let snapshot = GlobalLedger::open_evidence(
        GlobalLedgerEvidenceConfig::new(root).sqlite_material_not_read(),
        |reference| match verify_projected_read_only(root, reference) {
            Ok(verified) => Some(verified),
            Err(error) if error.is_material_missing() => None,
            Err(error) => {
                artifact_failure.get_or_insert(error.code());
                None
            }
        },
    )
    .map_err(|error| ledger_failure(format!("the ledger cannot be read: {error}")))?;
    if let Some(code) = artifact_failure {
        return Err(ledger_failure(format!(
            "an artifact does not verify: {code}"
        )));
    }
    if !snapshot.is_complete() {
        return Err(ledger_failure(
            "the ledger is incomplete or corrupt".to_owned(),
        ));
    }
    let events = run_events(&snapshot, evidence.request_id)
        .map_err(|error| ledger_failure(format!("the run's events cannot be read: {error}")))?;
    let mut captures: Vec<(String, Option<FrameId>, &ProjectedArtifactReference)> = Vec::new();
    let mut recognitions: Vec<(Option<FrameId>, &[RecognizedTarget])> = Vec::new();
    for event in &events {
        if event.event_type == EventType::ArtifactVerified {
            for artifact in &event.artifacts {
                if artifact.kind == ArtifactKind::CaptureFrame
                    && artifact.producer == ArtifactProducer::CaptureStore
                {
                    captures.push((
                        material_ref(event, artifact),
                        artifact
                            .frame_id
                            .or_else(|| event.links.frame_id().copied()),
                        artifact,
                    ));
                }
            }
        }
        if let ProjectionPayload::Full(payload) = &event.payload
            && let EventPayload::Task(TaskPayload::Semantic(semantic)) = payload.as_ref()
            && let TaskSemanticFact::RecognitionCompleted { targets, .. } = semantic.fact()
        {
            recognitions.push((event.links.frame_id().copied(), targets.as_slice()));
        }
    }
    let mut kept = Vec::with_capacity(evidence.frames.len());
    for (reference, steps) in &evidence.frames {
        let Some((_, frame_id, artifact)) = captures
            .iter()
            .find(|(candidate, _, _)| candidate == reference)
        else {
            return Err(CliError::new(
                ErrorKind::UsageValidation,
                ActinglabCode::RecognitionRegressionFrameUnknown.as_str(),
                format!(
                    "the frame reference {reference} names no verified capture frame of the run in {}",
                    root.display()
                ),
                &[],
            ));
        };
        let original = frame_id.and_then(|frame| {
            recognitions
                .iter()
                .rev()
                .find(|(candidate, _)| *candidate == Some(frame) && candidate.is_none())
                .map(|(_, targets)| targets.to_vec())
        });
        let source = match original {
            None => FrameSource::NoOriginalRecord,
            Some(original) => match read_frame(root, reference, artifact)? {
                Some(scene) => FrameSource::Kept {
                    scene: Box::new(scene),
                    original,
                },
                None => FrameSource::MaterialMissing,
            },
        };
        kept.push(KeptFrame {
            reference: reference.clone(),
            steps: steps.clone(),
            frame_id: *frame_id,
            source,
        });
    }
    Ok(kept)
}

/// Every event of the run's request, oldest first, in its forensic projection.
fn run_events(
    snapshot: &GlobalLedgerEvidence,
    request_id: RequestId,
) -> Result<Vec<ProjectedEvent>, String> {
    let query = EventQuery {
        request_id: Some(request_id),
        ..EventQuery::default()
    };
    let through = snapshot.latest_sequence();
    let mut events = Vec::new();
    let mut after = 0;
    loop {
        let page = snapshot
            .query_page(&query, after, through, EVENT_PAGE)
            .map_err(|error| error.to_string())?;
        for event in &page {
            let projected = project_subscription_event(event, &query, ProjectionProfile::Forensic)
                .ok_or_else(|| "a requested event was not projected".to_owned())?;
            after = projected.sequence;
            events.push(projected);
        }
        if page.len() < EVENT_PAGE {
            return Ok(events);
        }
    }
}

/// `<sequence>:<event_id>:<artifact_id>`, the material reference MCP gives a frame.
fn material_ref(event: &ProjectedEvent, artifact: &ProjectedArtifactReference) -> String {
    let text = |value: Value| match value {
        Value::String(text) => text,
        other => other.to_string(),
    };
    format!(
        "{}:{}:{}",
        event.sequence,
        text(json!(event.event_id)),
        text(json!(artifact.artifact_id))
    )
}

/// The frame's image; `None` when its material is gone.
fn read_frame(
    root: &Path,
    reference: &str,
    artifact: &ProjectedArtifactReference,
) -> CliOutcome<Option<Scene>> {
    let invalid = |problem: String| {
        CliError::new(
            ErrorKind::UsageValidation,
            ActinglabCode::RecognitionRegressionFrameInvalid.as_str(),
            format!("the frame {reference}: {problem}"),
            &[],
        )
    };
    if artifact.media_type != ArtifactMediaType::ImagePng {
        return Err(invalid("its material is not a PNG image".to_owned()));
    }
    let bytes = match read_projected_verified(root, artifact) {
        Ok(bytes) => bytes,
        Err(error) if error.is_material_missing() => return Ok(None),
        Err(error) => {
            return Err(invalid(format!(
                "its material does not read: {}",
                error.code()
            )));
        }
    };
    Scene::from_png(&bytes)
        .map(Some)
        .map_err(|error| invalid(format!("its PNG does not decode: {error}")))
}

fn has_target(evaluator: &RecognitionEvaluator, target_id: &str) -> bool {
    evaluator
        .pack()
        .targets
        .iter()
        .any(|target| target.id() == target_id)
}

/// The comparison of every kept frame with `evaluator`: `frames`, `targets` and `summary`.
fn compare(evaluator: &RecognitionEvaluator, kept: &[KeptFrame]) -> Map<String, Value> {
    let mut tallies: BTreeMap<String, Tally> = BTreeMap::new();
    let mut summary = Tally::default();
    let (mut missing, mut unrecorded, mut replayed) = (0_u64, 0_u64, 0_u64);
    let mut frames = Vec::with_capacity(kept.len());
    for frame in kept {
        let mut row = Map::new();
        row.insert("ref".to_owned(), json!(frame.reference));
        row.insert("steps".to_owned(), json!(frame.steps));
        row.insert("frame_id".to_owned(), json!(frame.frame_id));
        let (scene, original) = match &frame.source {
            FrameSource::Kept { scene, original } => (scene, original),
            FrameSource::MaterialMissing => {
                missing += 1;
                row.insert("status".to_owned(), json!(FrameStatus::MaterialMissing));
                frames.push(Value::Object(row));
                continue;
            }
            FrameSource::NoOriginalRecord => {
                unrecorded += 1;
                row.insert("status".to_owned(), json!(FrameStatus::NoOriginalRecord));
                frames.push(Value::Object(row));
                continue;
            }
        };
        replayed += 1;
        let mut targets = Vec::with_capacity(original.len());
        for target in original {
            let tally = tallies.entry(target.target_id.clone()).or_default();
            let mut entry = Map::new();
            entry.insert("target".to_owned(), json!(target.target_id));
            entry.insert("original_matched".to_owned(), json!(target.passed));
            let verdict = if has_target(evaluator, &target.target_id) {
                Some(evaluator.evaluate_target(scene, &target.target_id))
            } else {
                None
            };
            let status = match verdict {
                None => TargetStatus::Absent,
                Some(Err(error)) => {
                    entry.insert("failure".to_owned(), json!(error));
                    TargetStatus::Failed
                }
                Some(Ok(evaluation)) => {
                    entry.insert("matched".to_owned(), json!(evaluation.passed));
                    if let Some(score) = score(&evaluation) {
                        entry.insert("score".to_owned(), score);
                    }
                    entry.insert(
                        "agrees".to_owned(),
                        json!(evaluation.passed == target.passed),
                    );
                    TargetStatus::Compared
                }
            };
            let matched = entry.get("matched").and_then(Value::as_bool);
            tally.count(status, target.passed, matched);
            summary.count(status, target.passed, matched);
            entry.insert("status".to_owned(), json!(status));
            targets.push(Value::Object(entry));
        }
        row.insert("status".to_owned(), json!(FrameStatus::Compared));
        row.insert("targets".to_owned(), Value::Array(targets));
        frames.push(Value::Object(row));
    }
    let mut summary_value = summary.value();
    // The whole run's counts are over the target entries of the original records.
    if let Some(records) = summary_value.remove("frames") {
        summary_value.insert("records".to_owned(), records);
    }
    summary_value.insert("frames_referenced".to_owned(), json!(kept.len()));
    summary_value.insert("frames_replayed".to_owned(), json!(replayed));
    summary_value.insert("frames_material_missing".to_owned(), json!(missing));
    summary_value.insert(
        "frames_without_original_record".to_owned(),
        json!(unrecorded),
    );
    let mut report = Map::new();
    report.insert("frames".to_owned(), Value::Array(frames));
    report.insert(
        "targets".to_owned(),
        Value::Array(
            tallies
                .into_iter()
                .map(|(target, tally)| {
                    let mut value = tally.value();
                    value.insert("target".to_owned(), json!(target));
                    Value::Object(value)
                })
                .collect(),
        ),
    );
    report.insert("summary".to_owned(), Value::Object(summary_value));
    report
}

/// One target's (or the whole run's) counts over the frames that recorded it.
#[derive(Default)]
struct Tally {
    /// Frames whose original record names it.
    frames: u64,
    /// Of those, the frames it was evaluated on.
    compared: u64,
    absent: u64,
    failed: u64,
    original_hits: u64,
    hits: u64,
    /// Passed in the original record, not now.
    lost: u64,
    /// Not passed in the original record, passed now.
    gained: u64,
}

impl Tally {
    fn count(&mut self, status: TargetStatus, original: bool, matched: Option<bool>) {
        self.frames += 1;
        match (status, matched) {
            (TargetStatus::Compared, Some(matched)) => {
                self.compared += 1;
                self.original_hits += u64::from(original);
                self.hits += u64::from(matched);
                self.lost += u64::from(original && !matched);
                self.gained += u64::from(!original && !matched);
            }
            (TargetStatus::Absent, _) => self.absent += 1,
            _ => self.failed += 1,
        }
    }

    fn value(&self) -> Map<String, Value> {
        let disagreements = self.lost + self.gained;
        let mut value = Map::new();
        value.insert("frames".to_owned(), json!(self.frames));
        value.insert("compared".to_owned(), json!(self.compared));
        value.insert("absent".to_owned(), json!(self.absent));
        value.insert("failed".to_owned(), json!(self.failed));
        value.insert("original_hits".to_owned(), json!(self.original_hits));
        value.insert("hits".to_owned(), json!(self.hits));
        value.insert("lost".to_owned(), json!(self.lost));
        value.insert("gained".to_owned(), json!(self.gained));
        value.insert("disagreements".to_owned(), json!(disagreements));
        // Per mille of the frames compared, rounded down; `null` when none was compared.
        value.insert(
            "error_rate_milli".to_owned(),
            json!((self.compared > 0).then(|| disagreements.saturating_mul(1000) / self.compared)),
        );
        value
    }
}

/// The score the evaluation reports for its target's kind, in milli: a template's match score
/// and threshold, a colour's distance and its limit, an OCR read's confidence, a classifier's
/// selected score; `None` for a kind without one.
fn score(evaluation: &TargetEvaluation) -> Option<Value> {
    let milli = |value: f32| (f64::from(value) * 1000.0).round() as i64;
    let mut score = Map::new();
    score.insert("kind".to_owned(), json!(evaluation.kind));
    match evaluation.kind {
        TargetKind::Template => {
            let template = evaluation.template.as_ref()?;
            score.insert("score_milli".to_owned(), json!(milli(template.score)));
            score.insert(
                "threshold_milli".to_owned(),
                json!(milli(template.threshold)),
            );
        }
        TargetKind::Color => {
            let color = evaluation.color.as_ref()?;
            score.insert("distance_milli".to_owned(), json!(milli(color.distance)));
            score.insert(
                "max_distance_milli".to_owned(),
                json!(milli(color.max_distance)),
            );
        }
        TargetKind::Ocr => {
            let confidence = evaluation.ocr.as_ref()?.confidence?;
            score.insert("confidence_milli".to_owned(), json!(milli(confidence)));
        }
        TargetKind::Nn => {
            let selected = evaluation.nn.as_ref()?.selected_score?;
            score.insert("score_milli".to_owned(), json!(milli(selected)));
        }
        _ => return None,
    }
    Some(Value::Object(score))
}

// Workflow #331 RSU-1: the comparison of kept frames with a new version of a package, against
// the frames' original records. Each frame is 2 x 1 pixels; the new version's targets are colour
// targets of one pixel.
#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::RecognizedTargetRole;
    use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
    use actingcommand_recognition_pack::RecognitionPack;

    const RED: [u8; 3] = [255, 0, 0];
    const GREEN: [u8; 3] = [0, 255, 0];
    const BLUE: [u8; 3] = [0, 0, 255];
    const BLACK: [u8; 3] = [0, 0, 0];

    // The new version: the home page is still red, the terminal page now expects green where the
    // run saw blue, a mark expects black, and the guard target is gone.
    fn new_version() -> RecognitionEvaluator {
        let color = |id: &str, x: u32, expected: [u8; 3]| {
            json!({"type": "color", "id": id,
                   "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
        };
        let pack: RecognitionPack = serde_json::from_value(json!({
            "schema_version": "0.3",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": 2, "height": 1},
            "defaults": {"color_max_distance": 0.0},
            "targets": [
                color("page/home", 0, RED),
                color("page/terminal", 0, GREEN),
                color("mark/alt", 1, BLACK)
            ]
        }))
        .expect("recognition pack");
        RecognitionEvaluator::new(PathBuf::new(), pack).expect("recognition evaluator")
    }

    fn scene(left: [u8; 3], right: [u8; 3]) -> Box<Scene> {
        let png = Frame::from_pixels(
            2,
            1,
            [left, right].concat(),
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
        .expect("frame")
        .png_for_artifact()
        .expect("frame PNG");
        Box::new(Scene::from_png(&png).expect("scene"))
    }

    fn recorded(target_id: &str, passed: bool) -> RecognizedTarget {
        RecognizedTarget {
            target_id: target_id.to_owned(),
            role: RecognizedTargetRole::Required,
            passed,
            region: None,
        }
    }

    fn frame(reference: &str, step: u32, source: FrameSource) -> KeptFrame {
        KeptFrame {
            reference: reference.to_owned(),
            steps: vec![step],
            frame_id: None,
            source,
        }
    }

    fn tally(report: &Map<String, Value>, id: &str) -> Value {
        report["targets"]
            .as_array()
            .expect("targets")
            .iter()
            .find(|target| target["target"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("no tally of {id}"))
    }

    // ADD row RSU-1: per target of the original records, whether the new version finds it, its
    // score, and the error rate against the original record. On the home frame the home page
    // still agrees and the guard target is absent from the new version (not compared); on the
    // terminal frame the terminal page is lost and the mark, missed by the original record, is
    // gained. A frame whose material is gone and a frame without a recognition fact are listed
    // and not compared.
    #[test]
    fn rsu1_each_target_is_compared_with_its_original_record() {
        let kept = vec![
            frame(
                "1:home",
                0,
                FrameSource::Kept {
                    scene: scene(RED, GREEN),
                    original: vec![recorded("page/home", true), recorded("guard/ready", true)],
                },
            ),
            frame(
                "2:terminal",
                1,
                FrameSource::Kept {
                    scene: scene(BLUE, BLACK),
                    original: vec![recorded("page/terminal", true), recorded("mark/alt", false)],
                },
            ),
            frame("3:gone", 1, FrameSource::MaterialMissing),
            frame("4:unrecorded", 1, FrameSource::NoOriginalRecord),
        ];
        let report = compare(&new_version(), &kept);

        let home = tally(&report, "page/home");
        assert_eq!(
            (
                &home["frames"],
                &home["compared"],
                &home["original_hits"],
                &home["hits"]
            ),
            (&json!(1), &json!(1), &json!(1), &json!(1))
        );
        assert_eq!(home["disagreements"], 0);
        assert_eq!(home["error_rate_milli"], 0);

        let guard = tally(&report, "guard/ready");
        assert_eq!(
            (&guard["absent"], &guard["compared"]),
            (&json!(1), &json!(0))
        );
        assert_eq!(guard["error_rate_milli"], Value::Null);

        let terminal = tally(&report, "page/terminal");
        assert_eq!(
            (
                &terminal["original_hits"],
                &terminal["hits"],
                &terminal["lost"],
                &terminal["gained"]
            ),
            (&json!(1), &json!(0), &json!(1), &json!(0))
        );
        assert_eq!(terminal["error_rate_milli"], 1000);

        let mark = tally(&report, "mark/alt");
        assert_eq!(
            (
                &mark["original_hits"],
                &mark["hits"],
                &mark["lost"],
                &mark["gained"]
            ),
            (&json!(0), &json!(1), &json!(0), &json!(1))
        );
        assert_eq!(mark["error_rate_milli"], 1000);

        let summary = &report["summary"];
        assert_eq!(summary["frames_referenced"], 4);
        assert_eq!(summary["frames_replayed"], 2);
        assert_eq!(summary["frames_material_missing"], 1);
        assert_eq!(summary["frames_without_original_record"], 1);
        assert_eq!(summary["records"], 4);
        assert_eq!(summary["compared"], 3);
        assert_eq!(summary["absent"], 1);
        assert_eq!(summary["disagreements"], 2);
        assert_eq!(summary["error_rate_milli"], 666);

        let frames = report["frames"].as_array().expect("frames");
        assert_eq!(
            frames
                .iter()
                .map(|frame| frame["status"].clone())
                .collect::<Vec<_>>(),
            [
                json!("compared"),
                json!("compared"),
                json!("material_missing"),
                json!("no_original_record")
            ]
        );
        let lost = &frames[1]["targets"][0];
        assert_eq!(lost["target"], "page/terminal");
        assert_eq!(
            (&lost["original_matched"], &lost["matched"], &lost["agrees"]),
            (&json!(true), &json!(false), &json!(false))
        );
        assert_eq!(lost["score"]["kind"], "color");
        assert!(lost["score"]["distance_milli"].as_i64() > Some(0), "{lost}");
        assert_eq!(lost["score"]["max_distance_milli"], 0);
        assert_eq!(frames[0]["targets"][1]["status"], "absent");
    }
}
