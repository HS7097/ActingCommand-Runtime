// SPDX-License-Identifier: AGPL-3.0-only

//! #463 S2-8' (`contracts/lab-recording.md`, "Folding swipes into a scrolling search"): a choice in
//! a scrolling list is recorded in stage one as what the agent did, several swipes, each a step of
//! its own with its screens, then the press on the wanted row. A package does not replay those
//! swipes: its select declares one scan and the kernel moves the list itself. `fold_scan` says,
//! after the fact, that the swipes from `first` to the press step - 1 and the press are one
//! scrolling search: each folded swipe stays in the file whole, marked as folded into the press
//! step (`folded_into`), which records the fold (`scan_fold`); nothing is moved, renumbered or
//! deleted, and a folded swipe is no effective step. `unfold_scan` undoes it. This slice declares
//! no scan and generates nothing new: a fold no scrolling select reads is a failed pre-check of
//! `record stop` (`scan_without_select`).
//!
//! Every refusal is `record_scan_invalid` with its `scan_problem` and writes nothing. The agent
//! names the steps; the tool finds no range by itself.

use super::model::{
    ClickGesture, FoldAxis, FoldDirection, FoldedMove, LabRecording, RecordPoint, RecordingStep,
    StepScanFold,
};
use super::steps::{effective_step, step_ref};
use super::store::{blocked, now_unix_ms, with_details};
use crate::codes::{LabCode, ScanProblem};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{LabError, LabResult};
use serde_json::{Value, json};

/// A swipe runs clearly along one axis when its move along that axis, in pixels between its own
/// two points, is not zero and at least this many times its move across it.
const AXIS_RATIO: u64 = 2;

/// `record_scan_invalid` on `step` with why, its details carrying the outcome with `scan_problem`
/// and the members of `extra`.
pub(crate) fn scan_invalid(
    step: u32,
    problem: ScanProblem,
    message: String,
    extra: Value,
) -> LabError {
    let mut details = json!({
        "step": step,
        "scan_problem": problem.as_str(),
        "outcome": Outcome::new(LabCode::RecordScanInvalid)
            .scan_problem(problem)
            .envelope(Detail::Full)
    });
    if let (Some(object), Some(extra)) = (details.as_object_mut(), extra.as_object()) {
        for (name, value) in extra {
            object.insert(name.clone(), value.clone());
        }
    }
    with_details(
        blocked(LabCode::RecordScanInvalid.as_str(), message),
        details,
    )
}

/// The press step whose fold `step` is part of: the step it is folded into, or its own serial
/// number when it holds a fold; `None` for a step of no fold.
pub(crate) fn fold_of(step: &RecordingStep) -> Option<u32> {
    step.folded_into
        .or_else(|| step.scan_fold.as_ref().map(|_| step.index))
}

/// `step_in_fold`: a step action or a mark names step `step`, part of the fold of the press step
/// `press` (a folded swipe, or the press step itself); the fold is undone first.
pub(crate) fn in_fold(step: u32, press: u32) -> LabError {
    let what = if step == press {
        format!("step {step} holds the fold of the swipes recorded before it")
    } else {
        format!(
            "step {step} is a swipe folded into the scrolling search of step {press}: it is no \
             effective step, and record review lists its screens in the sheet of step {press}"
        )
    };
    scan_invalid(
        step,
        ScanProblem::StepInFold,
        format!(
            "{what}; undo the fold first (step_action unfold_scan of step {press}, record mark \
             --unfold-scan {press}); nothing was written"
        ),
        json!({"fold_step": press}),
    )
}

/// #463 S2-13 with S2-8': the step `index` `record frames` lists the frames around: an effective
/// step, or a swipe folded into a press step's scrolling search, whose screens are those of the
/// list; any other is `effective_step`'s refusal.
pub(crate) fn effective_or_folded(
    recording: &LabRecording,
    index: u32,
) -> LabResult<&RecordingStep> {
    match step_ref(recording, index) {
        Some(step) if step.folded_into.is_some() && step.kept_on_path() => Ok(step),
        _ => effective_step(recording, index),
    }
}

/// The step whose screen a run meets `step` on: for a press step that holds a fold, its first
/// folded swipe, on whose screen before its scrolling search starts; `step` otherwise. #463 S2-7's
/// drop of a press before the last compares the next step's screen so.
pub(crate) fn met_on<'r>(
    recording: &'r LabRecording,
    step: &'r RecordingStep,
) -> &'r RecordingStep {
    step.scan_fold
        .as_ref()
        .and_then(|fold| step_ref(recording, fold.first))
        .unwrap_or(step)
}

/// `fold_scan {step: press, first}`: the swipes recorded from `first` to `press` - 1 are folded
/// into the scrolling search of the press `press`. Each folded step keeps every member and frame
/// and takes `folded_into: press`; the press step takes the fold (`scan_fold`): its first step,
/// the axis, each folded step with its direction against the first swipe's, and the time. The
/// recording is changed in memory only when every check holds ([`planned_fold`]).
pub(crate) fn fold_scan(recording: &mut LabRecording, press: u32, first: u32) -> LabResult<()> {
    let (axis, moves) = planned_fold(recording, press, first)?;
    let mut fold = Some(StepScanFold {
        first,
        axis,
        moves,
        folded_at_unix_ms: now_unix_ms(),
    });
    for step in &mut recording.steps {
        if step.index == press {
            step.scan_fold = fold.take();
        } else if (first..press).contains(&step.index) {
            step.folded_into = Some(press);
        }
    }
    Ok(())
}

/// `unfold_scan {step: press}`: the fold of the press `press` is undone: its `scan_fold` and every
/// folded step's `folded_into` are taken off, and nothing else, so that the step table is byte for
/// byte what it was before the fold when nothing else changed in between (but for
/// `updated_at_unix_ms`, which every write sets). A step that holds no fold is `not_folded`.
pub(crate) fn unfold_scan(recording: &mut LabRecording, press: u32) -> LabResult<()> {
    let Some(step) = step_ref(recording, press) else {
        return Err(not_folded(press, None));
    };
    if step.scan_fold.is_none() {
        return Err(not_folded(press, step.folded_into));
    }
    for step in &mut recording.steps {
        if step.index == press {
            step.scan_fold = None;
        }
        if step.folded_into == Some(press) {
            step.folded_into = None;
        }
    }
    Ok(())
}

/// `not_folded`: `unfold_scan` names step `press`, which holds no fold (`into`: the press step it
/// is folded into, when it is a folded swipe).
fn not_folded(press: u32, into: Option<u32>) -> LabError {
    let hint = into.map_or_else(
        || "record status shows the fold on the press step it was made on (scan_fold)".to_owned(),
        |into| format!("it is a swipe folded into step {into}: name step {into}"),
    );
    scan_invalid(
        press,
        ScanProblem::NotFolded,
        format!("step {press} holds no fold to undo; {hint}; nothing was written"),
        json!({"folded_into": into}),
    )
}

/// `scan_not_selected`, the refusal `record stop` and its dry run give a press step `press` that
/// holds the fold `fold` and declares no scrolling select (`record ready`'s item
/// `scan_without_select`): this build declares none, so a recording with a fold makes no package.
pub(crate) fn scan_not_selected(press: u32, fold: &StepScanFold) -> LabError {
    let last = press.saturating_sub(1);
    scan_invalid(
        press,
        ScanProblem::ScanNotSelected,
        format!(
            "the swipes of steps {}..{last} are folded into the scrolling search of step {press}, \
             which declares no scrolling select yet: its package would press without scrolling. \
             This build declares no scrolling select, so a recording with a fold makes no package \
             yet; to make the package without the fold, undo it (step_action unfold_scan of step \
             {press}); nothing was written",
            fold.first
        ),
        json!({"first": fold.first}),
    )
}

/// Every check of `fold_scan {step: press, first}`, in order, before anything is changed; the fold's
/// axis and its folded steps with their directions when they all hold. `fold_range_invalid`: `first`
/// is not below `press`; a step of `first..=press` is unknown, dropped, converted into a transition,
/// folded or holding a fold already, or inserted after the stop; the press step's effect is no
/// executed tap ([`one_tap`]); a step between is no executed swipe with its two points on record
/// ([`swipe_ends`]). Then, step by step, what a folded step would lose ([`would_lose`]). Then the
/// swipes' run: each along one axis clearly, the first's ([`run_of`], `move_axis_unclear`), and no
/// swipe in the first's direction after one against it (`move_after_back`).
fn planned_fold(
    recording: &LabRecording,
    press: u32,
    first: u32,
) -> LabResult<(FoldAxis, Vec<FoldedMove>)> {
    let range = |at: Option<u32>, why: String| {
        scan_invalid(
            press,
            ScanProblem::FoldRangeInvalid,
            format!(
                "fold_scan folds the swipes recorded from step {first} to step {press} - 1 into the \
                 press of step {press}, and {why}; name the press of the wanted row as step and \
                 the first swipe before it as first; nothing was written"
            ),
            json!({"first": first, "at": at}),
        )
    };
    if first >= press {
        return Err(range(
            None,
            format!("first {first} is not below step {press}"),
        ));
    }
    let step_of = |index: u32| {
        step_ref(recording, index)
            .ok_or_else(|| range(Some(index), format!("step {index} does not exist")))
    };
    let pressed = step_of(press)?;
    let swipes = (first..press).map(step_of).collect::<LabResult<Vec<_>>>()?;
    for step in swipes.iter().chain(std::iter::once(&pressed)) {
        if let Some(state) = off_path(step) {
            return Err(range(
                Some(step.index),
                format!("step {} is {state}", step.index),
            ));
        }
    }
    if !one_tap(pressed) {
        return Err(range(
            Some(press),
            format!("the effect of step {press} is no executed tap"),
        ));
    }
    let mut ends = Vec::with_capacity(swipes.len());
    for step in &swipes {
        let Some(points) = swipe_ends(step) else {
            return Err(range(
                Some(step.index),
                format!(
                    "the effect of step {} is no executed swipe with its two points on record",
                    step.index
                ),
            ));
        };
        ends.push((step.index, points));
    }
    for step in &swipes {
        if let Some((problem, what)) = would_lose(step) {
            return Err(scan_invalid(
                press,
                problem,
                format!(
                    "step {} {what}, which the fold would lose: a folded step is no step of the \
                     package; take it off first, or fold another range; nothing was written",
                    step.index
                ),
                json!({"first": first, "at": step.index}),
            ));
        }
    }
    let mut first_run = None;
    let mut back_seen = false;
    let mut moves = Vec::with_capacity(ends.len());
    for (index, (from, to)) in ends {
        let moved = json!({"first": first, "at": index, "from": from, "to": to});
        let Some((axis, positive)) = run_of(from, to) else {
            return Err(scan_invalid(
                press,
                ScanProblem::MoveAxisUnclear,
                format!(
                    "the swipe of step {index} from {},{} to {},{} runs clearly along neither axis \
                     (its move along one axis is not at least {AXIS_RATIO} times its move across \
                     it); nothing was written",
                    from.x, from.y, to.x, to.y
                ),
                moved,
            ));
        };
        let (first_axis, first_positive) = *first_run.get_or_insert((axis, positive));
        if axis != first_axis {
            return Err(scan_invalid(
                press,
                ScanProblem::MoveAxisUnclear,
                format!(
                    "the swipe of step {index} runs along another axis than the first swipe of \
                     step {first}; nothing was written"
                ),
                moved,
            ));
        }
        let direction = if positive == first_positive {
            FoldDirection::Forward
        } else {
            FoldDirection::Back
        };
        if direction == FoldDirection::Forward && back_seen {
            return Err(scan_invalid(
                press,
                ScanProblem::MoveAfterBack,
                format!(
                    "the swipe of step {index} runs forward again after a swipe back: the \
                     demonstration goes forward as often as needed, then back as often as needed, \
                     then presses; nothing was written"
                ),
                moved,
            ));
        }
        back_seen |= direction == FoldDirection::Back;
        moves.push(FoldedMove {
            step: index,
            direction,
        });
    }
    let Some((axis, _)) = first_run else {
        return Err(range(None, "the range holds no swipe".to_owned()));
    };
    Ok((axis, moves))
}

/// Why `step` cannot be in a fold's range: dropped, converted into a transition, folded into
/// another press step or holding a fold already, or inserted after the stop; `None` when it can.
fn off_path(step: &RecordingStep) -> Option<String> {
    if step.dropped {
        return Some("dropped".to_owned());
    }
    if step.converted_to_transition {
        return Some("converted into a transition".to_owned());
    }
    if let Some(press) = fold_of(step) {
        return Some(format!("in the fold of step {press} already"));
    }
    step.inserted_after
        .map(|after| format!("an optional step inserted after step {after} after the stop"))
}

/// Whether the effect of `step` is one executed tap: a performed click without a gesture, and no
/// application operation.
fn one_tap(step: &RecordingStep) -> bool {
    step.application.is_none()
        && step
            .click
            .as_ref()
            .is_some_and(|click| click.executed() && click.gesture.is_none())
}

/// The two points of the executed swipe of `step`, as its execution holds them: a performed drag
/// with its end on record, and no application operation; `None` for any other effect.
fn swipe_ends(step: &RecordingStep) -> Option<(RecordPoint, RecordPoint)> {
    if step.application.is_some() {
        return None;
    }
    let click = step.click.as_ref().filter(|click| click.executed())?;
    if !matches!(click.gesture, Some(ClickGesture::Drag { .. })) {
        return None;
    }
    let execution = click.execution.as_ref()?;
    Some((execution.point, execution.to_point?))
}

/// What a folded `step` would lose, with its word: marks of its own, its page's name, its
/// optional flag, a choice (marked or refined by a select), a transition after its effect.
fn would_lose(step: &RecordingStep) -> Option<(ScanProblem, &'static str)> {
    [
        (
            !step.marks.is_empty(),
            ScanProblem::WouldLoseMarks,
            "has marks of its own",
        ),
        (
            step.page.is_some(),
            ScanProblem::WouldLosePage,
            "names its page",
        ),
        (
            step.optional.is_some(),
            ScanProblem::WouldLoseOptional,
            "is optional",
        ),
        (
            step.choice.is_some() || step.select.is_some(),
            ScanProblem::WouldLoseChoice,
            "holds a choice",
        ),
        (
            step.transition.is_some(),
            ScanProblem::WouldLoseTransition,
            "has a transition after its effect",
        ),
    ]
    .into_iter()
    .find(|(lost, _, _)| *lost)
    .map(|(_, problem, what)| (problem, what))
}

/// The axis a swipe from `from` to `to` runs clearly along and whether it runs towards larger
/// coordinates: vertical when its vertical move is not zero and at least [`AXIS_RATIO`] times its
/// horizontal move, horizontal the other way round; `None` when neither holds.
fn run_of(from: RecordPoint, to: RecordPoint) -> Option<(FoldAxis, bool)> {
    let across = i64::from(to.x) - i64::from(from.x);
    let down = i64::from(to.y) - i64::from(from.y);
    let (moved_x, moved_y) = (across.unsigned_abs(), down.unsigned_abs());
    if moved_y > 0 && moved_y >= moved_x.saturating_mul(AXIS_RATIO) {
        Some((FoldAxis::Vertical, down > 0))
    } else if moved_x > 0 && moved_x >= moved_y.saturating_mul(AXIS_RATIO) {
        Some((FoldAxis::Horizontal, across > 0))
    } else {
        None
    }
}
