// SPDX-License-Identifier: AGPL-3.0-only

//! #463 S2-1 (`contracts/lab-recording.md`, "What a recording still lacks: `record ready`"): the
//! one read that lists everything a recording still lacks before `record stop` can make its
//! package. Stage one leaves material; stage two writes the business's meaning on it offline;
//! `record stop` refuses at the first pre-check the recording fails, so this answer lists every
//! one at once (`generate::survey`, the generator's own checks, collected), then, when none is
//! failed, the self-checks `record stop` runs (`recording_api::self_checks`), and the recording's
//! state: `material`, `draft`, `ready` or `packaged`. Here the answer is put together from what the
//! generator found; the reading itself is `recording_api::record_ready`, offline, under no lock,
//! writing nothing.
//!
//! Fix round 2: an item carries what its refusal's details say ([`ReadyDetails`]), and the
//! warnings are typed ([`ReadyWarningItem`]), with the JSON they had.
//!
//! #463 S2-1b: the read takes the package's game, server and locale as `record stop` takes them
//! (its options, then the recording, then for game and server the Lab's instance configuration,
//! then the configuration of the installation the program runs from), and says where each came
//! from ([`ReadySources`]); it writes none of them on the recording. Fix round 1: the
//! installation's row gives both parts or neither; a packaged recording answers what its package
//! holds, and a given part that is not its package's is a warning ([`ReadyOptionConflict`]).
//!
//! #463 S2-1c: every failure of each self-check, not only the first (`check_items`), and for each
//! self-check that ran whether that list is all it has ([`ReadyCheckRun`]).

use super::crosscheck::{CheckFailure, CheckRun};
use super::generate::{Lack, effective_steps};
use super::model::{LabRecording, OpaqueJson, RecordRect, RecordingStep};
use super::steps::{POINT_SOURCE, SWIPE_SOURCE};
use super::store::invalid;
use crate::codes::{ReadyCheck, ReadyMissing, ReadySource, ReadyWarning};
use actingcommand_contract::{LabError, LabResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// #463 S2-1 fix round 2: what the refusal of a failed pre-check says of its item beside the
/// item's own members (its `step` and `artifact_step` are the item's; its `select_problem`,
/// `ending_problem`, `transition` and `outcome` say again the item's word and code), so that two
/// items of one word are told apart: the marks it names, an ending's outcome key and page name,
/// the step after a stop, the pages after the last launch or restart, the parts of the package's
/// identity that are not known, and the counts of a recording with too few or too many steps.
/// Read from the refusal's `details`; for `ending_mark_missing`, whose refusal names the mark
/// alone, `outcome_key` is the ending's. #463 S2-1c: for a self-check's failure, `outcome_key`,
/// `page`, `frames` and `target` say where the check met it, never read from its refusal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReadyDetails {
    /// The marks the refusal names (`reused_mark_missing`, `forbidden_mark_missing`,
    /// `anchor_not_own_template`, `ending_mark_missing`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<ReadyMark>,
    /// The ending's outcome key (the ending words; #463 S2-1c: and a self-check's failure of an
    /// ending).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome_key: Option<String>,
    /// The ending's page name (`ending_page_taken`); #463 S2-1c: or the page a self-check's failure
    /// is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    /// #463 S2-1c: the frames a self-check's failure is on (a page that does not match its own
    /// frame, a page that passes on another screen's frames, a frame that cannot be read back).
    /// Never read from a pre-check's refusal.
    #[serde(skip_deserializing, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<String>,
    /// #463 S2-1c: the target a self-check's failure is about: a template whose crop cannot be
    /// read back, the mark a click follows; fix round 1: the target that reads the name of a
    /// select's slot. Never read from a pre-check's refusal.
    #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// The serial number of the step after a stop (`effect_after_stop`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_step: Option<u32>,
    /// The pages after the last launch or restart (`application_without_home`), none among them
    /// the main interface.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub later_pages: Option<Vec<String>>,
    /// The parts of the package's identity that are not known (`game`, `server`, `locale`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    /// How many effective steps the recording has (`too_few_steps`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_steps: Option<usize>,
    /// How many steps have an effect, and how many a package holds (`too_many_effects`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect_steps: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<usize>,
}

impl ReadyDetails {
    /// Whether the refusal says nothing beside the item's own members.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// #463 S2-1 fix round 2: a mark a refusal names: its id, why it is refused (`reuse_source_missing`,
/// `forbidden_source_missing`, `anchor_invalid`) and, for an anchor, what is wrong with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadyMark {
    pub id: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// One thing a recording still lacks: a pre-check of `record stop` it fails, or (#463 S2-1c) one
/// failure of a self-check (`what` the check's name).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReadyItem {
    /// 1..n, in the answer's order.
    pub order: usize,
    /// The step's serial number, as `record mark --step` takes it; `None` for an item of the whole
    /// recording.
    pub step: Option<u32>,
    /// The step's number in the package; `None` with `step`.
    pub artifact_step: Option<u32>,
    /// The frame the step's press happened on, as stage one recorded it (`before_frame`; fix round
    /// 2: on a step whose press `--reopen-step` moved aside, its click's last attempt's); `None`
    /// when the step has none.
    pub frame_id: Option<String>,
    /// A word of `ready_missing`, or for a self-check's refusal a word of `ready_check`.
    pub what: String,
    /// The code `record stop` answers for it.
    pub code: String,
    /// The one command that settles it, where there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Fix round 2: what its refusal says beside these members; absent when nothing.
    #[serde(skip_serializing_if = "ReadyDetails::is_empty")]
    pub details: ReadyDetails,
}

/// #463 S2-1 fix round 2: `press_point_only` for one step (`{step, artifact_step, frame_id,
/// what}`, as before).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PressPointOnly {
    /// The step's serial number, as `record mark --step` takes it.
    pub step: u32,
    /// The step's number in the package.
    pub artifact_step: u32,
    /// The frame the step's press happened on ([`ReadyItem::frame_id`]).
    pub frame_id: Option<String>,
    /// `press_point_only`.
    pub what: String,
}

/// #463 S2-1b fix round 1: on a packaged recording, a part of the package's identity given to
/// `record ready` other than the one its package was generated with: `record stop` given it
/// refuses `record_stop_option_conflict` with `{field, recorded, given}` as its details; `record
/// ready` says the same as a warning and answers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReadyOptionConflict {
    /// `record_stop_option_conflict`, the code `record stop` refuses with.
    pub code: String,
    /// The part: `game`, `server` or `locale`.
    pub field: String,
    /// What the package was generated with; `null` when it recorded none.
    pub recorded: Option<OpaqueJson>,
    /// What `record ready` was given.
    pub given: OpaqueJson,
}

/// #463 S2-1 fix round 2: one warning of `record ready` (it never blocks), with the JSON it had: a
/// step's `press_point_only`, or a warning of the generator or of a self-check as `record stop`
/// prints it (its word in `code`, its other members its own; a dozen shapes, carried as they are);
/// #463 S2-1b fix round 1: or, on a packaged recording, a given part of its identity that is not
/// its package's ([`ReadyOptionConflict`]).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ReadyWarningItem {
    PressPointOnly(PressPointOnly),
    Generator(OpaqueJson),
    OptionConflict(ReadyOptionConflict),
}

/// #463 S2-1c: one self-check `record ready` ran: whether the failures `missing` lists of it are all
/// it has (`complete`), or it stopped at the last of them (`reason`, a word of `ready_check_stop`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReadyCheckRun {
    /// A word of `ready_check`.
    pub check: String,
    pub complete: bool,
    /// Why it stopped; absent when it is complete.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The answer of `record ready`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordReadyOutcome {
    /// `material`, `draft`, `ready` or `packaged`, by the recording alone.
    pub status: String,
    pub record_id: String,
    /// The session's state as `record status` says it: `active`, `archived` or `stopped`.
    pub session: String,
    /// Every pre-check the recording fails, the recording's own first, then by step, each step's
    /// in the order the generator checks them; or, when none, the failures of the self-checks
    /// (#463 S2-1c: every one each check found, in the order the checks run, each check's by step
    /// and then by frame).
    pub missing: Vec<ReadyItem>,
    pub missing_total: usize,
    /// `press_point_only` for each step whose press is a recorded point without a press region,
    /// then the generator's own warnings as `record stop` prints them; on a packaged recording
    /// (#463 S2-1b fix round 1), then each given part of its identity that is not its package's.
    pub warnings: Vec<ReadyWarningItem>,
    /// #463 S2-1c: each self-check that ran, in the order they ran, and whether its failures in
    /// `missing` are all it has; empty when none ran.
    pub checks: Vec<ReadyCheckRun>,
    /// The self-checks that did not run, in the order they run; empty when every one ran.
    pub checks_after: Vec<String>,
    /// #463 S2-1b: where the package's game, server and locale this answer took came from; on a
    /// packaged recording (fix round 1), what its package holds.
    pub sources: ReadySources,
}

/// #463 S2-1b: where each part of the package's identity `record ready` took came from, a word of
/// `ready_source` (`command_line`, `recording`, `lab_config`, `installation_config`); `None`
/// (`null`) for a part that is not known, which `missing` then lists. On a packaged recording
/// (fix round 1) `recording` for each part its package holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReadySources {
    pub game: Option<String>,
    pub server: Option<String>,
    pub locale: Option<String>,
}

impl ReadySources {
    /// The words of where the game, the server and the locale came from.
    pub(crate) fn of(
        game: Option<ReadySource>,
        server: Option<ReadySource>,
        locale: Option<ReadySource>,
    ) -> Self {
        let word = |source: Option<ReadySource>| source.map(|source| source.as_str().to_string());
        Self {
            game: word(game),
            server: word(server),
            locale: word(locale),
        }
    }
}

/// #463 S2-1b: a part of the package's identity: its value and the place it came from, `(None,
/// None)` when it is not known.
pub(crate) type Known = (Option<String>, Option<ReadySource>);

/// #463 S2-1b: the first of `candidates` (a value where its place holds one, and that place) that
/// is known, as its value and its place; `(None, None)` when none is.
pub(crate) fn known(candidates: &[(Option<&String>, ReadySource)]) -> Known {
    candidates
        .iter()
        .find_map(|&(value, source)| value.map(|value| (value.clone(), source)))
        .unzip()
}

/// The state of a recording, by the recording alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadyState {
    /// Nothing of stage two is written yet.
    Material,
    /// Something is written, and a pre-check fails or a self-check refuses.
    Draft,
    /// Every pre-check passes and every self-check: `record stop` would write the package.
    Ready,
    /// Stopped, its package written, and nothing changed since.
    Packaged,
}

impl ReadyState {
    const fn word(self) -> &'static str {
        match self {
            Self::Material => "material",
            Self::Draft => "draft",
            Self::Ready => "ready",
            Self::Packaged => "packaged",
        }
    }
}

/// Whether `recording` is stopped with its package written and nothing changed since (no mark
/// after the stop, `amended_at_unix_ms`).
pub(crate) fn packaged(recording: &LabRecording) -> bool {
    recording.status == "stopped"
        && recording.artifact.is_some()
        && recording.amended_at_unix_ms.is_none()
}

/// Whether stage two wrote anything on `recording`: a step carries a page name, a mark (own,
/// reused or forbidden, an anchor, a click guard), a press region (a click that is no press stage
/// one recorded: a tap or a long press by its point, a swipe; or one that follows a mark or
/// declares a retry; #463 stage two, train 1: or a press box, a press recorded by its point whose
/// rectangle is no longer that one pixel, the test [`point_press_warnings`] makes), a choice (a
/// select; #463 S2-6: or a step marked as holding a choice, refined or not), an optional flag or a
/// transition; or the recording took an ending.
fn stage_two_written(recording: &LabRecording) -> bool {
    !recording.endings.is_empty()
        || recording.steps.iter().any(|step| {
            step.page.is_some()
                || !step.marks.is_empty()
                || !step.reused.is_empty()
                || !step.forbidden.is_empty()
                || step.anchor.is_some()
                || step.click_guard.is_some()
                || step.select.is_some()
                || step.choice.is_some()
                || step.optional.is_some()
                || step.transition.is_some()
                || step.click.as_ref().is_some_and(|click| {
                    (click.source != POINT_SOURCE && click.source != SWIPE_SOURCE)
                        || (click.source == POINT_SOURCE && !one_pixel(click.rect))
                        || click.follow.is_some()
                        || click.retry.is_some()
                })
        })
}

/// The frame the press of `step` happened on, as stage one recorded it: its click's or its
/// application operation's `before_frame`. Fix round 2: a click without an execution, whose press
/// `--reopen-step` moved aside, names its last attempt's (an application operation moved aside
/// keeps no frame). #463 S2-2: `record review` names a step's screen before by it.
pub(crate) fn press_frame(step: &RecordingStep) -> Option<String> {
    step.click
        .as_ref()
        .and_then(|click| click.execution.as_ref().or_else(|| click.attempts.last()))
        .and_then(|execution| execution.before_frame.clone())
        .or_else(|| {
            step.application
                .as_ref()
                .and_then(|application| application.executed.as_ref())
                .and_then(|executed| executed.before_frame.clone())
        })
}

/// [`press_frame`] of the step with the serial number `serial`.
fn press_frame_of(recording: &LabRecording, serial: u32) -> Option<String> {
    recording
        .steps
        .iter()
        .find(|step| step.index == serial)
        .and_then(press_frame)
}

/// The one command that settles `what` on the step `step`, where there is one. #463 S2-1b: a part
/// of the package's identity is settled two ways, both named: given to `record ready`, which only
/// checks with it and writes nothing, and given to `record stop`, which builds with it.
fn hint(what: ReadyMissing, step: Option<u32>) -> Option<String> {
    let identity = |flag: &str| {
        Some(format!(
            "record ready {flag} to check, record stop {flag} to build"
        ))
    };
    let step = step.map_or_else(|| "<k>".to_string(), |step| step.to_string());
    match what {
        ReadyMissing::Game => identity("--game <g>"),
        ReadyMissing::Server => identity("--server <s>"),
        ReadyMissing::Locale => identity("--locale <l>"),
        ReadyMissing::PageRecognition => {
            Some(format!("record mark --step {step} --template <id>=x,y,w,h"))
        }
        ReadyMissing::OptionalFirstStep
        | ReadyMissing::OptionalFinalStep
        | ReadyMissing::OptionalApplication
        | ReadyMissing::OptionalRestartHome => {
            Some(format!("record mark --step {step} --not-optional"))
        }
        _ => None,
    }
}

/// Fix round 2: the [`ReadyDetails`] of the failed pre-check `lack`, read from its refusal's
/// details, with the ending it is about where the refusal does not name it. Details that do not
/// read so are the generator's own fault, `validation_failed`.
fn lack_details(lack: &Lack) -> LabResult<ReadyDetails> {
    let mut details = match &lack.error.details {
        Some(refused) => {
            serde_json::from_value::<ReadyDetails>(refused.clone()).map_err(|problem| {
                invalid(
                    "validation_failed",
                    format!(
                        "the details of the refusal {} do not read as an item's: {problem}",
                        lack.error.code
                    ),
                )
            })?
        }
        None => ReadyDetails::default(),
    };
    if details.outcome_key.is_none() {
        details.outcome_key.clone_from(&lack.outcome_key);
    }
    Ok(details)
}

/// The items of the failed pre-checks `lacks` (in the order the generator met them): the
/// recording's own first, then by the step's serial number, each step's in the generator's order.
pub(crate) fn lack_items(recording: &LabRecording, lacks: &[Lack]) -> LabResult<Vec<ReadyItem>> {
    let mut ordered = lacks.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|lack| lack.step.map(|(serial, _)| serial));
    ordered
        .into_iter()
        .enumerate()
        .map(|(position, lack)| {
            let step = lack.step.map(|(serial, _)| serial);
            Ok(ReadyItem {
                order: position + 1,
                step,
                artifact_step: lack.step.map(|(_, number)| number),
                frame_id: step.and_then(|serial| press_frame_of(recording, serial)),
                what: lack.what.as_str().to_string(),
                code: lack.error.code.clone(),
                hint: hint(lack.what, step),
                details: lack_details(lack)?,
            })
        })
        .collect()
}

/// #463 S2-1c: the items of the self-checks that ran (`runs`, in the order they ran): every failure
/// each found, each check's by the step's serial number (none first), then by its first frame
/// (`f0009` before `f0010`), failures of one step and frame in the order the check met them. An
/// item names its step (the serial number and the package's number) and the frame the step's press
/// happened on, as a pre-check's item does, and in its `details` the frames, the page or the target
/// and the ending the failure is about; an item of the package as a whole names none. Fix round 1:
/// the failure a check stopped at (its last, `CheckRun::stopped`) is its last item, where it
/// stopped, after every failure it met before it.
pub(crate) fn check_items(recording: &LabRecording, runs: &[CheckRun]) -> Vec<ReadyItem> {
    runs.iter()
        .flat_map(|run| {
            let judged = if run.stopped.is_some() {
                run.failures.len().saturating_sub(1)
            } else {
                run.failures.len()
            };
            let (met, stop) = run.failures.split_at(judged);
            let mut failures = met.iter().collect::<Vec<&CheckFailure>>();
            failures.sort_by_key(|failure| {
                (
                    failure.locus.step.map(|(serial, _)| serial),
                    failure
                        .locus
                        .frames
                        .first()
                        .map(|frame| (frame.len(), frame.clone())),
                )
            });
            failures
                .into_iter()
                .chain(stop)
                .map(move |failure| (run.check, failure))
        })
        .enumerate()
        .map(|(position, (check, failure))| {
            let locus = &failure.locus;
            let step = locus.step.map(|(serial, _)| serial);
            ReadyItem {
                order: position + 1,
                step,
                artifact_step: locus.step.map(|(_, number)| number),
                frame_id: step.and_then(|serial| press_frame_of(recording, serial)),
                what: check.as_str().to_string(),
                code: failure.error.code.clone(),
                hint: None,
                details: ReadyDetails {
                    outcome_key: locus.outcome_key.clone(),
                    page: locus.page.clone(),
                    frames: locus.frames.clone(),
                    target: locus.target.clone(),
                    ..ReadyDetails::default()
                },
            }
        })
        .collect()
}

/// #463 S2-1c: `checks` of the self-checks that ran (`runs`): each complete unless it stopped, with
/// the word of why.
pub(crate) fn check_runs(runs: &[CheckRun]) -> Vec<ReadyCheckRun> {
    runs.iter()
        .map(|run| ReadyCheckRun {
            check: run.check.as_str().to_string(),
            complete: run.stopped.is_none(),
            reason: run.stopped.map(|stop| stop.as_str().to_string()),
        })
        .collect()
}

/// #463 S2-1c: `checks` when every self-check ran to its end and found nothing.
pub(crate) fn every_check_complete() -> Vec<ReadyCheckRun> {
    ReadyCheck::ALL
        .iter()
        .map(|check| ReadyCheckRun {
            check: check.as_str().to_string(),
            complete: true,
            reason: None,
        })
        .collect()
}

/// Workflow #392 DA-5cd: `missing` with, first when `size` is the refusal of a recording whose
/// frames are not the base size, its item (an item of the whole recording; fix round 1: `what`
/// `size_not_base`, `code` the one `record stop` answers, `record_size_not_base`), the others
/// numbered after it.
pub(crate) fn with_size_item(size: Option<&LabError>, missing: Vec<ReadyItem>) -> Vec<ReadyItem> {
    let Some(refusal) = size else {
        return missing;
    };
    let item = ReadyItem {
        order: 1,
        step: None,
        artifact_step: None,
        frame_id: None,
        what: ReadyMissing::SizeNotBase.as_str().to_string(),
        code: refusal.code.clone(),
        hint: None,
        details: ReadyDetails::default(),
    };
    std::iter::once(item)
        .chain(missing)
        .enumerate()
        .map(|(position, mut item)| {
            item.order = position + 1;
            item
        })
        .collect()
}

/// #463 stage two, train 1: whether `rect` is one pixel, the rectangle of a press recorded by its
/// point until a press box (#463 S2-5) gives it a region. #463 S2-2: `record review` draws a press
/// region that is no such point.
pub(crate) const fn one_pixel(rect: RecordRect) -> bool {
    rect.width == 1 && rect.height == 1
}

/// `press_point_only` for each effective step, in the order a run meets them, whose press is a
/// single recorded point (a tap or a long press `tap`, `long-tap --record` declared by its point,
/// a box of one pixel) with no press region set and no mark it follows: the package would press
/// that exact pixel.
pub(crate) fn point_press_warnings(recording: &LabRecording) -> Vec<ReadyWarningItem> {
    effective_steps(recording)
        .into_iter()
        .enumerate()
        .filter(|(_, step)| {
            step.click.as_ref().is_some_and(|click| {
                click.source == POINT_SOURCE && one_pixel(click.rect) && click.follow.is_none()
            })
        })
        .map(|(position, step)| {
            ReadyWarningItem::PressPointOnly(PressPointOnly {
                step: step.index,
                artifact_step: u32::try_from(position + 1).unwrap_or(u32::MAX),
                frame_id: press_frame(step),
                what: ReadyWarning::PressPointOnly.as_str().to_string(),
            })
        })
        .collect()
}

/// Fix round 2: the generator's and the self-checks' `warnings`, each as `record stop` prints it.
pub(crate) fn generator_warnings(warnings: &[Value]) -> LabResult<Vec<ReadyWarningItem>> {
    warnings
        .iter()
        .map(|warning| OpaqueJson::from_serializable(warning).map(ReadyWarningItem::Generator))
        .collect()
}

/// The answer for `recording` and its session's state `session`. A recording that is not
/// [`packaged`] is `material` while stage two wrote nothing on it, otherwise `ready` when nothing
/// is `missing` and `draft` when something is. #463 S2-1b: `sources`, where the package's game,
/// server and locale came from. #463 S2-1c: `checks`, the self-checks that ran.
pub(crate) fn ready_outcome(
    recording: &LabRecording,
    session: String,
    missing: Vec<ReadyItem>,
    warnings: Vec<ReadyWarningItem>,
    checks: Vec<ReadyCheckRun>,
    checks_after: &[ReadyCheck],
    sources: ReadySources,
) -> RecordReadyOutcome {
    let state = if packaged(recording) {
        ReadyState::Packaged
    } else if !stage_two_written(recording) {
        ReadyState::Material
    } else if missing.is_empty() {
        ReadyState::Ready
    } else {
        ReadyState::Draft
    };
    RecordReadyOutcome {
        status: state.word().to_string(),
        record_id: recording.record_id.clone(),
        session,
        missing_total: missing.len(),
        missing,
        warnings,
        checks,
        checks_after: checks_after
            .iter()
            .map(|check| check.as_str().to_string())
            .collect(),
        sources,
    }
}
