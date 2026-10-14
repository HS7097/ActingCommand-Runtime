// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #375 R5e: `actingledger frames`, the read-only frame listing (model-375 §2.8).
//!
//! It opens the ledger as `events` does, reading no material, takes the frame retention
//! switches from the latest `config.parameters.top_level` runtime fact (which actingd writes at
//! each start), and evaluates the frame retention view at the tool's clock, so it classes frames
//! as the running actingd does. Then it stats the frames it prints, and only after that walks
//! `kept\` for the folder sizes: a frame moved between the two is found by the walk, and a
//! frame found in both places counts `in_folder`. It writes nothing and reads no file contents.

use super::{ForensicError, ForensicOutput, ForensicReport, ForensicResult, map_ledger_error};
use crate::codes::{ForensicsCode, ForensicsLocation};
use actingcommand_artifact_store::{KEPT_DIRECTORY, list_kept_folders};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::CodeStr;
use actingcommand_contract::{
    ArtifactId, ArtifactKind, CONFIG_TOP_LEVEL_SECTION, EventPayload, EventType, FactScalar,
    FactValue, InstanceId, RunId, RuntimeFactRecord, RuntimePayload, TaskPayload, TaskSemanticFact,
    config_parameters_fact_key,
};
use actingcommand_ledger::{
    FrameErrorPoint, FrameRetentionClass, FrameRetentionFrame, FrameRetentionSwitches,
    FrameRetentionView, GlobalLedger, GlobalLedgerEvidenceConfig, LedgerCode, PersistedEvent,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// The runtime fact `actingctl clear-kept` records for each cleared folder (model-375 §2.7):
/// one row `folder`, `cleared_at_unix_ms`.
pub const KEPT_FOLDER_CLEARED_FACT_KEY: &str = "host.kept_folder_cleared";
/// The configuration keys of the switches the classes read (actingd-config revision 4).
const RETENTION_ENABLED_KEY: &str = "frame_retention_enabled";
const DEDUP_ERROR_KEY: &str = "frame_retention_dedup_error";
const DEDUP_LAB_KEY: &str = "frame_retention_dedup_lab";
const DAY_MS: i64 = 86_400_000;
/// `ERROR_DELETE_PENDING`: a file that is being deleted is no longer there.
const DELETE_PENDING: i32 = 303;

/// `--class`: the frames the listing selects. `All` is every settled class and never includes
/// `Running`; `Due` is every duplicate, default and resource frame whose due time has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameClassFilter {
    All,
    Lab,
    Error,
    Resource,
    Duplicate,
    Default,
    Due,
    Running,
}

impl FrameClassFilter {
    /// `lab`, `error`, `resource`, `duplicate`, `default`, `due`, `running` or `all`.
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "all" => Self::All,
            "lab" => Self::Lab,
            "error" => Self::Error,
            "resource" => Self::Resource,
            "duplicate" => Self::Duplicate,
            "default" => Self::Default,
            "due" => Self::Due,
            "running" => Self::Running,
            _ => return None,
        })
    }
}

/// `--before <YYYY-MM-DD>`: a calendar date, which the listing reads as its local midnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalDate {
    year: i64,
    month: i64,
    day: i64,
}

impl LocalDate {
    /// `YYYY-MM-DD`, a real date of the proleptic Gregorian calendar.
    pub fn parse(value: &str) -> Option<Self> {
        let bytes = value.as_bytes();
        let shaped = bytes.len() == 10
            && bytes.iter().enumerate().all(|(index, byte)| match index {
                4 | 7 => *byte == b'-',
                _ => byte.is_ascii_digit(),
            });
        if !shaped {
            return None;
        }
        // Every byte is ASCII, so these slices are character boundaries.
        let year = value[0..4].parse::<i64>().ok()?;
        let month = value[5..7].parse::<i64>().ok()?;
        let day = value[8..10].parse::<i64>().ok()?;
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let days = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => return None,
        };
        (1..=days)
            .contains(&day)
            .then_some(Self { year, month, day })
    }

    /// The UTC instant of this date's local midnight, at the offset `local` gives for an
    /// instant (milliseconds east of UTC); `None` when the offset is unknown.
    fn midnight_unix_ms(self, local: fn(u64) -> Option<i64>) -> Option<u64> {
        let year = if self.month <= 2 {
            self.year - 1
        } else {
            self.year
        };
        let era = year.div_euclid(400);
        let year_of_era = year - era * 400;
        let day_of_year = (153 * ((self.month + 9) % 12) + 2) / 5 + self.day - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        let days = era * 146_097 + day_of_era - 719_468;
        let wall = days.checked_mul(DAY_MS)?;
        let first = wall.checked_sub(local(u64::try_from(wall).ok()?)?)?;
        let utc = wall.checked_sub(local(u64::try_from(first).ok()?)?)?;
        u64::try_from(utc).ok()
    }
}

/// One `actingledger --state-root <root> frames` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForensicFramesRequest {
    pub state_root: PathBuf,
    pub class: FrameClassFilter,
    /// Only frames captured before this date's local midnight.
    pub before: Option<LocalDate>,
    pub instance: Option<InstanceId>,
    pub run: Option<RunId>,
    /// Only the frames of this error point's window; it must name an error point.
    pub error_sequence: Option<u64>,
    /// Print every selected settled frame's path with its class.
    pub paths: bool,
}

/// A number of frames and their bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct FrameCount {
    pub frames: u64,
    pub bytes: u64,
}

impl FrameCount {
    fn count(&mut self, bytes: u64) {
        self.frames = self.frames.saturating_add(1);
        self.bytes = self.bytes.saturating_add(bytes);
    }
}

/// The selected frames of one settled class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct FrameClassTotals {
    pub frames: u64,
    pub bytes: u64,
    /// What the next sweep deletes: a due duplicate, default or resource frame still at its
    /// object key.
    pub due_now: FrameCount,
    /// A Lab or error frame with its folder, still at its object key.
    pub to_move: FrameCount,
    /// Found in `kept\` by the walk, whatever the frame's class is now.
    pub in_folder: FrameCount,
    /// In neither place: deleted by hand, or already removed.
    pub absent: FrameCount,
}

impl FrameClassTotals {
    fn count(&mut self, bytes: u64) {
        self.frames = self.frames.saturating_add(1);
        self.bytes = self.bytes.saturating_add(bytes);
    }
}

/// The selected frames per class; `running` carries counts only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct FrameTotals {
    pub lab: FrameClassTotals,
    pub error: FrameClassTotals,
    pub resource: FrameClassTotals,
    pub duplicate: FrameClassTotals,
    pub default: FrameClassTotals,
    pub running: FrameCount,
}

/// The switches the view was evaluated with, from the latest `config.parameters.top_level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FrameSwitchesReport {
    pub dedup_error: bool,
    pub dedup_lab: bool,
    pub retention_enabled: bool,
}

/// One error point: its sequence, event type, code, time, run, and its run's package and task
/// labels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FrameErrorPointReport {
    pub sequence: u64,
    pub event_type: EventType,
    pub code: CodeStr,
    pub at_unix_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_label: Option<String>,
}

/// One kept leaf: a leaf the walk found, or one the index places frames in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeptFolderReport {
    /// `kept/<date>/<leaf>`, relative to the state root.
    pub path: String,
    pub date: String,
    pub leaf: String,
    /// The kept frame files the walk found in it.
    pub present: FrameCount,
    /// The frames the index places in it.
    pub placed: u64,
    /// Its owner point, then any later points whose windows hold its error frames.
    pub error_points: Vec<FrameErrorPointReport>,
}

/// One error point with its window (`--class error` or `--error-sequence`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorPointFramesReport {
    #[serde(flatten)]
    pub point: FrameErrorPointReport,
    /// The frames its window holds.
    pub window: FrameCount,
    /// Those of them kept as error frames.
    pub kept: FrameCount,
    /// The folders holding its kept frames.
    pub folders: Vec<String>,
}

/// One `host.kept_folder_cleared` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClearedFolderReport {
    pub sequence: u64,
    pub folder: String,
    pub cleared_at_unix_ms: u64,
}

/// Artifacts that are not capture frames; they are never deleted automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct NonFrameTotals {
    pub artifacts: u64,
    pub bytes: u64,
}

/// One printed frame path (`--paths`): `kept/…` when the walk found it, else its object key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FramePathReport {
    pub path: String,
    pub class: &'static str,
}

/// The listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FramesReport {
    pub through_sequence: u64,
    pub evaluated_at_unix_ms: u64,
    /// `--before`, as the UTC instant of its local midnight.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_unix_ms: Option<u64>,
    pub switches: FrameSwitchesReport,
    pub totals: FrameTotals,
    pub folders: Vec<KeptFolderReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_points: Option<Vec<ErrorPointFramesReport>>,
    /// Every clear event, not only the latest fact.
    pub cleared: Vec<ClearedFolderReport>,
    pub non_frame: NonFrameTotals,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<FramePathReport>>,
}

/// What the listing reads from the ledger besides the frame view.
pub(crate) struct FrameLedgerFacts {
    switches: FrameSwitchesReport,
    cleared: Vec<ClearedFolderReport>,
    /// The package and task labels of each run.
    labels: BTreeMap<RunId, (String, String)>,
    non_frame: NonFrameTotals,
}

/// Collects [`FrameLedgerFacts`] in ledger order.
#[derive(Default)]
pub(crate) struct FrameFactsBuilder {
    /// The latest `config.parameters.top_level` value.
    parameters: Option<FactValue>,
    cleared: Vec<ClearedFolderReport>,
    labels: BTreeMap<RunId, (String, String)>,
    non_frame: BTreeMap<ArtifactId, u64>,
}

impl FrameFactsBuilder {
    /// A runtime fact recorded at `sequence`.
    pub(crate) fn fact(&mut self, sequence: u64, record: &RuntimeFactRecord) -> ForensicResult<()> {
        if record.key == config_parameters_fact_key(CONFIG_TOP_LEVEL_SECTION) {
            self.parameters = Some(record.value.clone());
        } else if record.key == KEPT_FOLDER_CLEARED_FACT_KEY {
            self.cleared.push(cleared_folder(sequence, &record.value)?);
        }
        Ok(())
    }

    /// A run's `PackageAdmitted` labels; the first one stays.
    pub(crate) fn package(&mut self, run: RunId, package_label: &str, task_label: &str) {
        self.labels
            .entry(run)
            .or_insert_with(|| (package_label.to_owned(), task_label.to_owned()));
    }

    /// An artifact an event names.
    pub(crate) fn artifact(&mut self, artifact: ArtifactId, kind: ArtifactKind, byte_count: u64) {
        if kind != ArtifactKind::CaptureFrame {
            self.non_frame.entry(artifact).or_insert(byte_count);
        }
    }

    /// The facts; the three switches must be in the latest `config.parameters.top_level`.
    pub(crate) fn finish(self) -> ForensicResult<FrameLedgerFacts> {
        let switches = self
            .parameters
            .as_ref()
            .and_then(parameter_switches)
            .ok_or_else(|| {
                ForensicError::new(
                    ForensicsCode::FrameSwitchesUnavailable.code(),
                    ForensicsLocation::ListFrames.location(),
                    format!(
                        "the latest {} fact does not name {RETENTION_ENABLED_KEY}, \
                     {DEDUP_ERROR_KEY} and {DEDUP_LAB_KEY}",
                        config_parameters_fact_key(CONFIG_TOP_LEVEL_SECTION)
                    ),
                )
            })?;
        Ok(FrameLedgerFacts {
            switches,
            cleared: self.cleared,
            labels: self.labels,
            non_frame: NonFrameTotals {
                artifacts: self.non_frame.len() as u64,
                bytes: self
                    .non_frame
                    .values()
                    .fold(0_u64, |sum, bytes| sum.saturating_add(*bytes)),
            },
        })
    }
}

/// The listing's selection, with `--before` at its UTC instant.
pub(crate) struct FrameSelection {
    pub(crate) class: FrameClassFilter,
    pub(crate) before_unix_ms: Option<u64>,
    pub(crate) instance: Option<InstanceId>,
    pub(crate) run: Option<RunId>,
    pub(crate) error_sequence: Option<u64>,
    pub(crate) paths: bool,
}

/// `actingledger frames`: the frame listing of one state root, read-only.
pub fn list_frames(request: ForensicFramesRequest) -> ForensicResult<ForensicOutput> {
    if request.state_root.as_os_str().is_empty() {
        return Err(ForensicError::new(
            ForensicsCode::InvalidStateRoot.code(),
            ForensicsLocation::ValidateForensicRequest.location(),
            "state root is empty",
        ));
    }
    // Read-only and without material: no artifact is opened or verified, so the listing
    // holds no recovery authority.
    let config = GlobalLedgerEvidenceConfig::new(&request.state_root);
    let snapshot = GlobalLedger::open_evidence(config.sqlite_material_not_read(), |_| None)
        .map_err(map_ledger_error)?;
    let facts = ledger_facts(snapshot.events())?;
    let now_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .ok_or_else(clock_failed)?;
    let view = snapshot
        .frame_retention_view(
            now_unix_ms,
            FrameRetentionSwitches {
                dedup_error: facts.switches.dedup_error,
                dedup_lab: facts.switches.dedup_lab,
            },
        )
        .map_err(map_ledger_error)?;
    let before_unix_ms = match request.before {
        Some(date) => Some(
            date.midnight_unix_ms(actingcommand_ledger::local_time::machine_local_offset_ms)
                .ok_or_else(clock_failed)?,
        ),
        None => None,
    };
    let selection = FrameSelection {
        class: request.class,
        before_unix_ms,
        instance: request.instance,
        run: request.run,
        error_sequence: request.error_sequence,
        paths: request.paths,
    };
    let report = frames_report(&request.state_root, &view, facts, &selection)?;
    Ok(ForensicOutput::Machine(ForensicReport::Frames(Box::new(
        report,
    ))))
}

fn ledger_facts(events: &[PersistedEvent]) -> ForensicResult<FrameLedgerFacts> {
    let mut builder = FrameFactsBuilder::default();
    for event in events {
        for artifact in event.artifacts() {
            builder.artifact(
                *artifact.artifact_id(),
                artifact.kind(),
                artifact.byte_count(),
            );
        }
        match event.payload() {
            EventPayload::Runtime(RuntimePayload::FactRecorded(payload)) => {
                builder.fact(event.sequence(), payload.record())?;
            }
            EventPayload::Task(TaskPayload::Semantic(payload)) => {
                if let TaskSemanticFact::PackageAdmitted {
                    package_label,
                    task_label,
                    ..
                } = payload.fact()
                    && let Some(run) = event.links().run_id()
                {
                    builder.package(*run, package_label, task_label);
                }
            }
            _ => {}
        }
    }
    builder.finish()
}

/// The three switches of a `config.parameters.top_level` value, `None` when one is missing.
fn parameter_switches(value: &FactValue) -> Option<FrameSwitchesReport> {
    let FactValue::RecordList(rows) = value else {
        return None;
    };
    let flag = |key: &str| {
        rows.iter()
            .find_map(|row| match (row.get("key"), row.get("value")) {
                (Some(FactScalar::String(name)), Some(FactScalar::Boolean(value)))
                    if name == key =>
                {
                    Some(*value)
                }
                _ => None,
            })
    };
    Some(FrameSwitchesReport {
        dedup_error: flag(DEDUP_ERROR_KEY)?,
        dedup_lab: flag(DEDUP_LAB_KEY)?,
        retention_enabled: flag(RETENTION_ENABLED_KEY)?,
    })
}

/// One clear event's row: `folder` and `cleared_at_unix_ms`.
fn cleared_folder(sequence: u64, value: &FactValue) -> ForensicResult<ClearedFolderReport> {
    let invalid = || {
        ForensicError::new(
            ForensicsCode::KeptFolderClearedRecordInvalid.code(),
            ForensicsLocation::ListFrames.location(),
            format!("sequence={sequence}"),
        )
    };
    let FactValue::RecordList(rows) = value else {
        return Err(invalid());
    };
    let [row] = rows.as_slice() else {
        return Err(invalid());
    };
    let Some(FactScalar::String(folder)) = row.get("folder") else {
        return Err(invalid());
    };
    let cleared_at_unix_ms = match row.get("cleared_at_unix_ms") {
        Some(FactScalar::TimestampMs(at)) => *at,
        Some(FactScalar::Integer(at)) => u64::try_from(*at).map_err(|_| invalid())?,
        _ => return Err(invalid()),
    };
    Ok(ClearedFolderReport {
        sequence,
        folder: folder.clone(),
        cleared_at_unix_ms,
    })
}

/// The listing of `view` over the files below `root`. It stats every selected settled frame
/// first, then walks `kept\`.
pub(crate) fn frames_report(
    root: &Path,
    view: &FrameRetentionView,
    facts: FrameLedgerFacts,
    selection: &FrameSelection,
) -> ForensicResult<FramesReport> {
    let now = view.evaluated_at_unix_ms;
    // `--error-sequence` must name an error point; a sequence that names none is refused as
    // the usage error it is, never listed as an empty window.
    let named = match selection.error_sequence {
        Some(sequence) => {
            let named = view
                .error_points
                .iter()
                .enumerate()
                .filter(|(_, point)| point.sequence == sequence)
                .map(|(index, _)| index)
                .collect::<BTreeSet<_>>();
            if named.is_empty() {
                return Err(ForensicError::new(
                    SharedCode::InvalidArguments.code(),
                    ForensicsLocation::ListFrames.location(),
                    format!("no error point has sequence {sequence}"),
                ));
            }
            Some(named)
        }
        None => None,
    };
    let selected = view
        .frames
        .iter()
        .filter(|frame| selects(selection, named.as_ref(), frame, now))
        .collect::<Vec<_>>();
    let mut at_object = Vec::with_capacity(selected.len());
    for frame in &selected {
        at_object.push(frame.class != FrameRetentionClass::Running && object_present(root, frame)?);
    }
    let leaves = list_kept_folders(root).map_err(|error| {
        ForensicError::relayed(
            error.code(),
            ForensicsLocation::ListFrames.as_str(),
            error.to_string(),
        )
    })?;
    let mut kept_paths = BTreeMap::<&str, String>::new();
    for leaf in &leaves {
        for file in &leaf.files {
            kept_paths
                .entry(file.object_file.as_str())
                .or_insert_with(|| {
                    format!(
                        "{KEPT_DIRECTORY}/{}/{}/{}",
                        leaf.date, leaf.leaf, file.file_name
                    )
                });
        }
    }

    let mut totals = FrameTotals::default();
    let mut paths = Vec::new();
    for (frame, at_object) in selected.iter().zip(at_object) {
        let bytes = frame.reference.byte_count;
        let class = match frame.class {
            FrameRetentionClass::Running => {
                totals.running.count(bytes);
                continue;
            }
            FrameRetentionClass::Lab => &mut totals.lab,
            FrameRetentionClass::Error => &mut totals.error,
            FrameRetentionClass::Resource => &mut totals.resource,
            FrameRetentionClass::Duplicate => &mut totals.duplicate,
            FrameRetentionClass::Default => &mut totals.default,
        };
        class.count(bytes);
        let path = match object_file(frame).and_then(|file| kept_paths.get(file)) {
            Some(kept) => {
                class.in_folder.count(bytes);
                kept.clone()
            }
            None => {
                if !at_object {
                    class.absent.count(bytes);
                } else if is_due(frame, now) {
                    class.due_now.count(bytes);
                } else if matches!(
                    frame.class,
                    FrameRetentionClass::Lab | FrameRetentionClass::Error
                ) && frame.folder.is_some()
                {
                    class.to_move.count(bytes);
                }
                frame.reference.object_key.clone().unwrap_or_default()
            }
        };
        if selection.paths {
            paths.push(FramePathReport {
                path,
                class: class_name(frame.class),
            });
        }
    }

    // Every walked leaf, and every leaf the index places a frame in.
    let mut folders = BTreeMap::<(String, String), (FrameCount, Vec<&FrameRetentionFrame>)>::new();
    for leaf in &leaves {
        let (present, _) = folders
            .entry((leaf.date.clone(), leaf.leaf.clone()))
            .or_default();
        for file in &leaf.files {
            present.count(file.byte_count);
        }
    }
    for frame in &view.frames {
        if let Some(folder) = &frame.folder {
            folders
                .entry((folder.date.clone(), folder.leaf.clone()))
                .or_default()
                .1
                .push(frame);
        }
    }
    let folders = folders
        .into_iter()
        .map(|((date, leaf), (present, placed))| {
            let owner = view
                .error_points
                .iter()
                .position(|point| point.date == date && point.leaf == leaf);
            let owner_sequence = owner.map(|index| view.error_points[index].sequence);
            let later = placed
                .iter()
                .filter(|frame| frame.class == FrameRetentionClass::Error)
                .flat_map(|frame| frame.windows.iter().copied())
                .filter(|index| {
                    Some(*index) != owner
                        && view.error_points.get(*index).is_some_and(|point| {
                            owner_sequence.is_none_or(|first| point.sequence > first)
                        })
                })
                .collect::<BTreeSet<_>>();
            let error_points = owner
                .into_iter()
                .chain(later)
                .filter_map(|index| view.error_points.get(index))
                .map(|point| point_report(point, &facts.labels))
                .collect();
            KeptFolderReport {
                path: format!("{KEPT_DIRECTORY}/{date}/{leaf}"),
                date,
                leaf,
                present,
                placed: placed.len() as u64,
                error_points,
            }
        })
        .collect::<Vec<_>>();

    let error_points = (selection.class == FrameClassFilter::Error || named.is_some()).then(|| {
        view.error_points
            .iter()
            .enumerate()
            .filter(|(index, point)| match &named {
                Some(named) => named.contains(index),
                None => {
                    selection
                        .instance
                        .is_none_or(|instance| point.instance_id == instance)
                        && selection.run.is_none_or(|run| point.run_id == Some(run))
                        && selection
                            .before_unix_ms
                            .is_none_or(|before| point.at_unix_ms < before)
                }
            })
            .map(|(index, point)| {
                let mut window = FrameCount::default();
                let mut kept = FrameCount::default();
                let mut holders = BTreeSet::new();
                for frame in view
                    .frames
                    .iter()
                    .filter(|frame| frame.windows.contains(&index))
                {
                    window.count(frame.reference.byte_count);
                    if frame.class == FrameRetentionClass::Error {
                        kept.count(frame.reference.byte_count);
                        if let Some(folder) = &frame.folder {
                            holders.insert(format!(
                                "{KEPT_DIRECTORY}/{}/{}",
                                folder.date, folder.leaf
                            ));
                        }
                    }
                }
                ErrorPointFramesReport {
                    point: point_report(point, &facts.labels),
                    window,
                    kept,
                    folders: holders.into_iter().collect(),
                }
            })
            .collect()
    });

    Ok(FramesReport {
        through_sequence: view.through_sequence,
        evaluated_at_unix_ms: view.evaluated_at_unix_ms,
        before_unix_ms: selection.before_unix_ms,
        switches: facts.switches,
        totals,
        folders,
        error_points,
        cleared: facts.cleared,
        non_frame: facts.non_frame,
        paths: selection.paths.then_some(paths),
    })
}

fn selects(
    selection: &FrameSelection,
    named: Option<&BTreeSet<usize>>,
    frame: &FrameRetentionFrame,
    now: u64,
) -> bool {
    let class = match selection.class {
        FrameClassFilter::All => frame.class != FrameRetentionClass::Running,
        FrameClassFilter::Lab => frame.class == FrameRetentionClass::Lab,
        FrameClassFilter::Error => frame.class == FrameRetentionClass::Error,
        FrameClassFilter::Resource => frame.class == FrameRetentionClass::Resource,
        FrameClassFilter::Duplicate => frame.class == FrameRetentionClass::Duplicate,
        FrameClassFilter::Default => frame.class == FrameRetentionClass::Default,
        FrameClassFilter::Due => is_due(frame, now),
        FrameClassFilter::Running => frame.class == FrameRetentionClass::Running,
    };
    class
        && selection
            .instance
            .is_none_or(|instance| frame.instance_id == instance)
        && selection.run.is_none_or(|run| frame.run_id == Some(run))
        && selection
            .before_unix_ms
            .is_none_or(|before| frame.reference.created_at_unix_ms < before)
        && named.is_none_or(|named| frame.windows.iter().any(|index| named.contains(index)))
}

/// A duplicate, default or resource frame whose due time has come.
fn is_due(frame: &FrameRetentionFrame, now: u64) -> bool {
    matches!(
        frame.class,
        FrameRetentionClass::Resource
            | FrameRetentionClass::Duplicate
            | FrameRetentionClass::Default
    ) && frame.due_unix_ms.is_some_and(|due| due <= now)
}

/// The object's own file name, `artifact_<hex>.png`.
fn object_file(frame: &FrameRetentionFrame) -> Option<&str> {
    frame
        .reference
        .object_key
        .as_deref()
        .and_then(|key| key.rsplit('/').next())
}

/// Whether the frame's file is at its object key. Any error but "not there" fails the listing.
fn object_present(root: &Path, frame: &FrameRetentionFrame) -> ForensicResult<bool> {
    let Some(key) = frame.reference.object_key.as_deref() else {
        return Ok(false);
    };
    match std::fs::symlink_metadata(root.join(key)) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(error)
            if error.kind() == ErrorKind::NotFound
                || error.raw_os_error() == Some(DELETE_PENDING) =>
        {
            Ok(false)
        }
        Err(error) => Err(ForensicError::new(
            ForensicsCode::FrameStatFailed.code(),
            ForensicsLocation::ListFrames.location(),
            format!(
                "path={key} io_kind={:?} os_error={:?}",
                error.kind(),
                error.raw_os_error()
            ),
        )),
    }
}

fn point_report(
    point: &FrameErrorPoint,
    labels: &BTreeMap<RunId, (String, String)>,
) -> FrameErrorPointReport {
    let label = point.run_id.as_ref().and_then(|run| labels.get(run));
    FrameErrorPointReport {
        sequence: point.sequence,
        event_type: point.event_type,
        code: point.code.clone(),
        at_unix_ms: point.at_unix_ms,
        run_id: point.run_id,
        package_label: label.map(|(package, _)| package.clone()),
        task_label: label.map(|(_, task)| task.clone()),
    }
}

const fn class_name(class: FrameRetentionClass) -> &'static str {
    match class {
        FrameRetentionClass::Lab => "lab",
        FrameRetentionClass::Error => "error",
        FrameRetentionClass::Resource => "resource",
        FrameRetentionClass::Duplicate => "duplicate",
        FrameRetentionClass::Default => "default",
        FrameRetentionClass::Running => "running",
    }
}

/// The tool's clock or the local time zone cannot be read.
fn clock_failed() -> ForensicError {
    ForensicError::new(
        LedgerCode::ArtifactRetentionClockFailed.code(),
        ForensicsLocation::ListFrames.location(),
        "the clock or the local offset is unavailable",
    )
}

#[cfg(test)]
mod tests;
