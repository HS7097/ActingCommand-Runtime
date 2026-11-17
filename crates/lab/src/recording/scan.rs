// SPDX-License-Identifier: AGPL-3.0-only

//! #463 S2-9a (`contracts/lab-recording.md`, "A scrolling choice: reading the list to its end"):
//! the first kind of scrolling choice. Stage one recorded the demonstration as it was played:
//! swipes forward to the list's end, swipes back to the wanted row, the press; S2-8' folded those
//! swipes into the press step (`scan_fold`). A request's `select` with `rows` (one row, its
//! repeating anchor, its name region and its click box, all on the list's first screen) and `scan`
//! (the list's area, the start mark on the first screen, the end mark on the last screen after a
//! forward swipe, and `stop: "end"`) declares the step's choice over the list read across its
//! screens; `record stop` generates it as a select over a scan (`contracts/selection-graph.md`,
//! "Select over a scan").
//!
//! - **Every region is the agent's.** The Lab measures numbers from those regions and from the
//!   demonstration (the content window, the spacing of the rows, the settle, the bounds, the scan's
//!   time; [`scan_numbers`]); each is kept with what it was measured from, and each may be given in
//!   the request instead (`scan.numbers`), marked as given. No tool proposes a region, a choice or
//!   a value.
//! - **The self-test at mark time** ([`check_scan_select`]) reads every screen of the demonstration
//!   with the kernel's own dry run of the step (`dry_run_select`, through `select::decide`), checks
//!   the start and end marks there, presses the demonstrated name on the step's own frames, and runs
//!   the kernel's offline entry (`dry_run_scan_decide`) over the forward screens, once for the pressed
//!   name and once for a name no screen shows.
//! - **The self-checks of `record stop`** run the same on the package it generates
//!   ([`scan_select_checks`], [`scan_first_decision`]).
//!
//! #463 S2-9b (`contracts/lab-recording.md`, "A scrolling choice: straight to the wanted item"):
//! the second kind, `stop: "wanted"`. Stage one swiped forward until the wanted row showed and
//! pressed it, so the list's end may never be on a recorded screen: such a select may declare no
//! end mark (the owner's ruling on board #463, option A of question 221). Its scan stops early
//! where the wanted item shows (`early_stop: "upper_bound"`); without an end it declares no `end`
//! and no `on_empty` ([`ends_empty`]): found, it presses; where the list stops moving first, the
//! run fails with the kernel's `selection_scan_early_stop_unreached`. The offline entry must stop
//! early on the pressed name; a name no screen shows is not judged offline then.

use super::crosscheck::{Failures, FirstDecision, FrameCache, Locus, evaluator_bundle};
use super::fold::scan_invalid;
use super::frames::{LoadedFrame, crop_png, read_stored_frame, rect_contains, rect_inside};
use super::generate::{
    Plan, PlanStep, SELECT_FEATURE, render, select_policy_document, select_probe_plan,
};
use super::marks::MarkRejection;
use super::model::{
    AfterCaptureRecord, ClickGesture, FoldAxis, FoldDirection, LabRecording, RecordPoint,
    RecordRect, RecordedFrame, RecordedMark, RecordingStep, ScanAbsentRun, ScanDecision,
    ScanMeasuredStep, ScanNumber, ScanNumberOrigin, ScanNumberSource, ScanNumberValue, ScanRegion,
    ScanScreen, ScanScreenRole, ScanSelfTestOutcome, ScanStop, SelectFrameDecision, SelectRowsSpec,
    SelectScanSpec, SelectScanState, SelectSelfTest, SelectSpec, SelfTestStatus, StepScanFold,
    StepSelect,
};
use super::ready::press_frame;
use super::select::{
    Decided, NAME_UNREAD, PRESS_MISMATCH, PRESSED_ABSENT, SELECT_REJECTION_ID, SELF_TEST_ERROR,
    SelectChecked, SelectFrame, SelectProbe, decide, mirrored_outcome, on_empty_key,
    select_invalid,
};
use super::steps::{live_primary, step_ref};
use super::store::{blocked, invalid, with_details};
use crate::LabOcr;
use crate::codes::{LabCode, ScanProblem, SelectProblem};
use actingcommand_contract::{
    InstanceFactContext, InstanceFactSnapshot, LabError, LabResult, ScanCoverageState,
    TaskSelectionBlocking, TaskSelectionOutcome,
};
use actingcommand_device::{CaptureBackendName, Frame};
use actingcommand_execution_kernel::{
    ContainedTaskError, ExecutionKernelCode, PreparedContainedTask, ScanDryRunDecided,
    dry_run_scan_decide,
};
use actingcommand_recognition::color_digest::FRAME_CHANGE_MAX_MILLI;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// The instance the probe package is admitted for, as a fixed-slot select's probe.
const PROBE_INSTANCE: &str = "lab.record.select";
/// The time the probe's admission may take, as `record stop`'s.
const PROBE_BUDGET: Duration = Duration::from_secs(120);
/// The parts of a scrolling select's targets: the row's anchor, its name, the two ends.
const ANCHOR: &str = "anchor";
const NAME: &str = "name";
const START: &str = "start";
const END: &str = "end";
/// The most `expected` values an OCR target lists (the recognition pack's limit).
const EXPECTED_LIMIT: usize = 64;
/// The most anchors a `repeated_anchor` layout admits, and the most viewports of a scan.
const MAX_INSTANCES: u64 = 64;
const MAX_VIEWPORTS: u64 = 128;
/// The words of the self-test's refusals that only a scrolling select has (`record_mark_rejected`
/// with `{id: "select", reason}`; `record stop`'s `record_step_self_mismatch` with `reason`).
const ROWS_NOT_FOUND: &str = "scan_rows_not_found";
const START_MARK_MISSING: &str = "scan_start_mark_missing";
const START_MARK_REPEATED: &str = "scan_start_mark_repeated";
const END_MARK_MISSING: &str = "scan_end_mark_missing";
const END_MARK_LOST: &str = "scan_end_mark_lost";
const STEP_OUTSIDE_WINDOW: &str = "scan_step_outside_window";
const COVERAGE_INCOMPLETE: &str = "scan_coverage_incomplete";
const POSITION_UNREAD: &str = "scan_position_unread";
const DECISION_MISMATCH: &str = "scan_decision_mismatch";
const ABSENT_NOT_EMPTY: &str = "scan_absent_not_empty";
/// #463 S2-9b: the words of a scrolling select straight to the wanted item without an end mark: its
/// smallest content step above twice its position tolerance plus one (mark time), and its wanted
/// default on no screen after the first and the forward swipes (the self-check `selects`).
const MIN_STEP_TOO_LARGE: &str = "scan_min_step_too_large";
const DEFAULT_NOT_SHOWN: &str = "scan_default_not_shown";

/// #463 S2-9a: a scrolling select's sheet in `record review`: its rows and scan as declared, the
/// screens of the demonstration in order (the regions taken from each, the names read on each,
/// the content offset of each screen the offline entry aligned), every number with its origin, and
/// the offline entry's outcome, as the select's self-test left them.
/// #463 S2-9b: the sheet adds `when_absent`, what a run does when the wanted item is in no row.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordReviewScan {
    pub rows: SelectRowsSpec,
    pub scan: SelectScanSpec,
    pub screens: Vec<ScanScreen>,
    pub numbers: Vec<ScanNumberValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ScanSelfTestOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_absent: Option<ScanWhenAbsent>,
}

/// #463 S2-9b: how a run of a scrolling select ends when the wanted item is in no row of its list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanAbsentEnd {
    /// The scan reads the list to its declared end, decides empty and the run ends normally with
    /// the select's `on_empty` key.
    EndsNormally,
    /// The scan declares no end: the list stops moving before the early stop holds and the run
    /// fails with the kernel's code.
    Fails,
}

/// #463 S2-9b: what `record review` says of a scrolling select: its kind (`kind`, the request's
/// `scan.stop`), whether it declares an end mark (`end_declared`), and what a run does when the
/// wanted item is in no row (`run`): it ends normally with `outcome_key`, or it fails with the
/// kernel's code `fails_with`. Absent for a select of the kind `end` without an end mark, which
/// `record stop` refuses (`end_not_recorded`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScanWhenAbsent {
    pub kind: ScanStop,
    pub end_declared: bool,
    pub run: ScanAbsentEnd,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fails_with: Option<String>,
}

/// Whether `step`'s select scrolls: it declares `rows` (and `scan`).
pub(crate) fn scrolls(step: &RecordingStep) -> bool {
    step.select
        .as_ref()
        .is_some_and(|select| select.declaration.rows.is_some())
}

/// #463 S2-9b: whether `scan` goes straight to the wanted item and declares no end mark: it never
/// reads its list to an end, so it decides only once its early stop holds.
fn without_end(scan: &SelectScanSpec) -> bool {
    scan.stop == Some(ScanStop::Wanted) && scan.end.is_none()
}

/// #463 S2-9b: whether `select` can end a run normally when the wanted name is in no row or slot
/// (`on_empty`): every select but a scrolling one straight to the wanted item without an end mark,
/// beside whose scan the kernel refuses `on_empty` (`contracts/selection-graph.md`, "A scan
/// without an end").
pub(crate) fn ends_empty(select: &SelectSpec) -> bool {
    !select.scan.as_ref().is_some_and(without_end)
}

/// #463 S2-9b: the coverage the kernel's offline entry must give the pressed name: complete for
/// the first kind; for the second, early stopped where the item shows, or complete when an end is
/// declared and its check passes on that screen (the end check is made first on each screen).
fn decided_coverage(scan: &SelectScanSpec) -> &'static [ScanCoverageState] {
    match (scan.stop, scan.end) {
        (Some(ScanStop::Wanted), None) => &[ScanCoverageState::EarlyStopped],
        (Some(ScanStop::Wanted), Some(_)) => {
            &[ScanCoverageState::EarlyStopped, ScanCoverageState::Complete]
        }
        _ => &[ScanCoverageState::Complete],
    }
}

/// The rows and the scan of a scrolling select.
fn rows_and_scan(spec: &SelectSpec) -> LabResult<(SelectRowsSpec, &SelectScanSpec)> {
    match (spec.rows, spec.scan.as_ref()) {
        (Some(rows), Some(scan)) => Ok((rows, scan)),
        _ => Err(invalid(
            "validation_failed",
            "a scrolling select declares rows and scan together",
        )),
    }
}

/// The target of `part` of the scrolling select of `step`: `select/step_ii/<part>`.
fn target_id(plan: &Plan, step: &PlanStep, part: &str) -> String {
    format!("select/step_{}/{part}", plan.numbered(step.number))
}

/// The asset of the template `part` of the scrolling select of `step`, in a directory of its own
/// (`assets/select/`), where no mark's asset is: a mark's asset name holds no `/`.
fn asset_of(plan: &Plan, step: &PlanStep, part: &str) -> String {
    format!(
        "assets/select/step_{}_{part}.png",
        plan.numbered(step.number)
    )
}

/// The templates a scrolling select declares: the row's anchor and the start mark, and the end
/// mark when it is declared.
fn template_parts(spec: &SelectSpec) -> Vec<&'static str> {
    let mut parts = vec![ANCHOR, START];
    if spec.scan.as_ref().is_some_and(|scan| scan.end.is_some()) {
        parts.push(END);
    }
    parts
}

fn rect_json(rect: RecordRect) -> Value {
    json!({"x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height})
}

fn region_json(rect: RecordRect) -> Value {
    json!({"mode": "rect", "rect": rect_json(rect)})
}

/// The start along `axis` of `rect` and its extent along it.
fn along(axis: FoldAxis, rect: RecordRect) -> (i64, i64) {
    match axis {
        FoldAxis::Vertical => (i64::from(rect.y), i64::from(rect.height)),
        FoldAxis::Horizontal => (i64::from(rect.x), i64::from(rect.width)),
    }
}

/// The start across `axis` of `rect` and its extent across it.
fn across(axis: FoldAxis, rect: RecordRect) -> (i64, i64) {
    match axis {
        FoldAxis::Vertical => (i64::from(rect.x), i64::from(rect.width)),
        FoldAxis::Horizontal => (i64::from(rect.y), i64::from(rect.height)),
    }
}

fn unsigned(value: i64) -> u64 {
    u64::try_from(value.max(0)).unwrap_or(0)
}

fn holds(point: RecordPoint, rect: RecordRect) -> bool {
    i64::from(point.x) >= i64::from(rect.x)
        && i64::from(point.y) >= i64::from(rect.y)
        && i64::from(point.x) < i64::from(rect.x) + i64::from(rect.width)
        && i64::from(point.y) < i64::from(rect.y) + i64::from(rect.height)
}

/// The word of a region, as the answers name it.
fn region_word(region: ScanRegion) -> &'static str {
    match region {
        ScanRegion::List => "list",
        ScanRegion::Row => "row",
        ScanRegion::Anchor => "anchor",
        ScanRegion::Name => "name",
        ScanRegion::Click => "click",
        ScanRegion::Start => "start",
        ScanRegion::End => "end",
    }
}

/// The word of a number, as the request's `numbers` and the answers name it.
fn number_word(name: ScanNumber) -> String {
    match serde_json::to_value(name) {
        Ok(Value::String(word)) => word,
        _ => String::new(),
    }
}

/// #463 S2-9a: the shape of a scrolling select's request, before any recording is read: `rows`
/// excludes `slots` (`rows_with_slots`) and `confirm` (`confirm_with_rows`: the kernel confirms a
/// scrolled choice itself); `rows` and `scan` go together; `scan.stop` is stated
/// (`scan_stop_missing`). The Lab's own reading (a rectangle without width or height, a number of
/// `numbers` the Lab does not know or of 0) is `validation_failed`; another word of `stop` does not
/// decode (`validation_failed`). #463 S2-9b: a select straight to the wanted item without an end
/// mark gives no `on_empty_key` (`on_empty_without_end`): it can end no run normally.
pub(crate) fn validate_scan_shape(step: Option<u32>, select: &SelectSpec) -> LabResult<()> {
    let refuse = |problem: SelectProblem, message: &str| {
        select_invalid(step, problem, format!("{message}; nothing was written"))
    };
    let shape = |message: String| invalid("validation_failed", format!("select {message}"));
    if select.rows.is_some() && !select.slots.is_empty() {
        return Err(refuse(
            SelectProblem::RowsWithSlots,
            "the request declares rows and slots: a scrolling select reads its rows across the \
             list's screens, a fixed-slot select its slots on one screen; give one of them",
        ));
    }
    if select.rows.is_some() && select.confirm.is_some() {
        return Err(refuse(
            SelectProblem::ConfirmWithRows,
            "the request declares confirm beside rows: the kernel confirms a scrolled choice itself, \
             on a fresh frame after its return to the chosen row; leave confirm out",
        ));
    }
    let (rows, scan) = rows_and_scan(select)
        .map_err(|_| shape("declares rows and scan together, or neither".to_owned()))?;
    if scan.stop.is_none() {
        return Err(refuse(
            SelectProblem::ScanStopMissing,
            "the request's scan states no stop: say which kind of scrolling choice the step holds \
             (\"end\": the list is read to its end, then chosen; \"wanted\": the list is scrolled \
             straight to the wanted item)",
        ));
    }
    if without_end(scan) && select.on_empty_key.is_some() {
        return Err(refuse(
            SelectProblem::OnEmptyWithoutEnd,
            "the request's select scrolls straight to the wanted item, declares no end mark and \
             gives an on_empty_key: such a scan never reads the list to its end, so a run cannot \
             end normally there; where the item is not found the run fails. Leave on_empty_key \
             out, or record the list's end once and declare the end mark",
        ));
    }
    let rects = [
        rows.row,
        rows.anchor,
        rows.name,
        rows.click_rect(),
        scan.list,
        scan.start,
    ];
    if rects
        .into_iter()
        .chain(scan.end)
        .any(|rect| rect.width < 1 || rect.height < 1)
    {
        return Err(shape(
            "rows and scan: a rectangle needs width and height >= 1".to_owned(),
        ));
    }
    for (word, value) in &scan.numbers {
        if serde_json::from_value::<ScanNumber>(json!(word)).is_err() {
            return Err(shape(format!(
                "scan.numbers: '{word}' is no number of a scan"
            )));
        }
        if *value == 0 {
            return Err(shape(format!("scan.numbers: '{word}' must be at least 1")));
        }
    }
    Ok(())
}

/// #463 S2-9a: `record_scan_invalid` with `end_not_recorded` for a step whose scrolling select
/// reads its list to its end and declares no end mark (`record ready`'s item
/// `scan_end_not_recorded`); `None` for any other step (#463 S2-9b: a select straight to the
/// wanted item may declare none).
pub(crate) fn end_not_recorded(step: &RecordingStep) -> Option<LabError> {
    let scan = step.select.as_ref()?.declaration.scan.as_ref()?;
    if scan.end.is_some() || scan.stop == Some(ScanStop::Wanted) {
        return None;
    }
    let index = step.index;
    Some(scan_invalid(
        index,
        ScanProblem::EndNotRecorded,
        format!(
            "the scrolling select of step {index} reads its list to its end and declares no end \
             mark: the demonstration never reached the list's end, so the scan would have no end \
             to stop at. Record the business again and scroll to the list's end once (then back \
             to the wanted row), fold the swipes, and declare the end mark on the last screen \
             after a forward swipe; nothing was written"
        ),
        json!({}),
    ))
}

/// The screens of a scrolling select kept with it.
fn screens_of_step(step: &RecordingStep) -> &[ScanScreen] {
    step.select
        .as_ref()
        .and_then(|select| select.scan.as_ref())
        .map(|scan| scan.screens.as_slice())
        .unwrap_or_default()
}

/// #463 S2-9a: the stored frames of every screen of the demonstration of `step`'s scrolling select
/// (none for any other step), which `record stop` reads back as it reads a step's frames.
pub(crate) fn scan_screen_frames(step: &PlanStep) -> Vec<&RecordedFrame> {
    screens_of_step(&step.step)
        .iter()
        .map(|screen| &screen.frame)
        .collect()
}

/// #463 S2-9a: the list's first screen of `step`'s scrolling select, on which a run meets the
/// step's page (its regression case's frame); `None` for any other step.
pub(crate) fn first_screen(step: &PlanStep) -> Option<&RecordedFrame> {
    screens_of_step(&step.step)
        .first()
        .map(|screen| &screen.frame)
}

/// #463 S2-9a: `record review`'s sheet of `step`'s scrolling select, of the recording of the task
/// `task_id`; `None` for any other step. #463 S2-9b: with `when_absent`.
pub(crate) fn review_sheet(step: &RecordingStep, task_id: &str) -> Option<RecordReviewScan> {
    let select = step.select.as_ref()?;
    let state = select.scan.as_ref()?;
    let (rows, scan) = rows_and_scan(&select.declaration).ok()?;
    Some(RecordReviewScan {
        rows,
        scan: scan.clone(),
        screens: state.screens.clone(),
        numbers: state.numbers.clone(),
        outcome: state.outcome.clone(),
        when_absent: when_absent(task_id, &select.declaration, scan),
    })
}

/// #463 S2-9b: what a run of the scrolling select `spec` (its scan `scan`) does when the wanted item
/// is in no row: with an end mark it ends normally with the `on_empty` key; straight to the wanted
/// item without one it fails `selection_scan_early_stop_unreached`; `None` for the kind `end`
/// without an end mark, which generates no package.
fn when_absent(task_id: &str, spec: &SelectSpec, scan: &SelectScanSpec) -> Option<ScanWhenAbsent> {
    let kind = scan.stop?;
    let end_declared = scan.end.is_some();
    if end_declared {
        return Some(ScanWhenAbsent {
            kind,
            end_declared,
            run: ScanAbsentEnd::EndsNormally,
            outcome_key: Some(on_empty_key(task_id, spec)),
            fails_with: None,
        });
    }
    (kind == ScanStop::Wanted).then(|| ScanWhenAbsent {
        kind,
        end_declared,
        run: ScanAbsentEnd::Fails,
        outcome_key: None,
        fails_with: Some(
            ExecutionKernelCode::SelectionScanEarlyStopUnreached
                .as_str()
                .to_owned(),
        ),
    })
}

// ---- Generation -----------------------------------------------------------------------------

/// #463 S2-9a: the `repeated_anchor` layout of a scrolling select on its step's page: the anchor
/// template, every rectangle relative to the anchor's top-left corner on the first screen (the
/// row as `instance_rect`, the click box, the name read at its offset), the list as the readable
/// band, rows ordered along the fold's axis, and the one identity feature `name` over the names.
pub(crate) fn layout_json(
    plan: &Plan,
    step: &PlanStep,
    spec: &SelectSpec,
    state: &SelectScanState,
) -> LabResult<Value> {
    let page = step.page_id()?;
    let (rows, scan) = rows_and_scan(spec)?;
    let entries = spec
        .names
        .iter()
        .map(|name| json!({"id": name.id, "aliases": name.aliases}))
        .collect::<Vec<_>>();
    Ok(json!({
        "id": plan.select_layout_id(step),
        "page_id": page,
        "kind": "repeated_anchor",
        "anchor": target_id(plan, step, ANCHOR),
        "max_instances": state.number(ScanNumber::MaxInstances),
        "order": match state.axis {
            FoldAxis::Vertical => "top_to_bottom",
            FoldAxis::Horizontal => "left_to_right",
        },
        "instance_rect": relative(rows, rows.row),
        "click": relative(rows, rows.click_rect()),
        "readable_band": rect_json(scan.list),
        "features": [{
            "name": SELECT_FEATURE,
            "value": "identity",
            "target": target_id(plan, step, NAME),
            "offset": {"x": rows.name.x - rows.anchor.x, "y": rows.name.y - rows.anchor.y},
            "identity": {
                "entries": entries,
                "recognition": {
                    "kind": "ocr_aliases",
                    "max_distance": spec.matching.max_distance,
                    "minimum_margin": spec.matching.minimum_margin,
                    "minimum_confidence_milli": spec.read.minimum_confidence_milli
                }
            }
        }]
    }))
}

/// `rect` relative to the top-left corner of the anchor of `rows`.
fn relative(rows: SelectRowsSpec, rect: RecordRect) -> Value {
    json!({
        "x": rect.x - rows.anchor.x,
        "y": rect.y - rows.anchor.y,
        "width": rect.width,
        "height": rect.height
    })
}

/// #463 S2-9a: the `scan` member of a scrolling select's operation, every member the declaration
/// requires (`contracts/selection-graph.md`, "Declaration"), each number the select's own: the
/// advance a drag between one-pixel boxes at the first forward swipe's two points over its
/// duration, the reposition the same drag, the tracked area the list, the signature the row, the
/// start mark with `rewind: true` and the end mark. A scan that does not stop early declares no
/// `early_stop`. Without an end mark (only the mark-time probe meets one: `record stop` refuses it
/// first, `end_not_recorded`) the start mark stands in its place. #463 S2-9b: a select straight to
/// the wanted item adds `early_stop: "upper_bound"` after `bounds` and, without an end mark,
/// declares no `end`.
pub(crate) fn add_scan_member(
    declared: &mut Value,
    plan: &Plan,
    step: &PlanStep,
    spec: &SelectSpec,
    state: &SelectScanState,
) -> LabResult<()> {
    let (rows, scan) = rows_and_scan(spec)?;
    let number = |name: ScanNumber| state.number(name);
    let point = |at: RecordPoint| json!({"x": at.x, "y": at.y, "width": 1, "height": 1});
    let end = if scan.end.is_some() { END } else { START };
    let Value::Object(mut member) = json!({
        "identity_feature": SELECT_FEATURE,
        "axis": match state.axis {
            FoldAxis::Vertical => "vertical",
            FoldAxis::Horizontal => "horizontal",
        },
        "advance": {
            "kind": "drag",
            "from": point(state.advance_from),
            "to": point(state.advance_to),
            "duration_ms": number(ScanNumber::AdvanceDurationMs),
            "min_content_step_px": number(ScanNumber::AdvanceMinContentStepPx),
            "max_content_step_px": number(ScanNumber::AdvanceMaxContentStepPx)
        },
        "reposition": {
            "kind": "drag",
            "from": point(state.advance_from),
            "to": point(state.advance_to),
            "duration_ms": number(ScanNumber::AdvanceDurationMs),
            "max_content_step_px": number(ScanNumber::RepositionMaxContentStepPx)
        },
        "settle": {
            "interval_ms": number(ScanNumber::SettleIntervalMs),
            "timeout_ms": number(ScanNumber::SettleTimeoutMs)
        },
        "track": {
            "area": rect_json(scan.list),
            "edge_margin_px": number(ScanNumber::EdgeMarginPx),
            "signature": {
                "rect": relative(rows, rows.row),
                "columns": number(ScanNumber::SignatureColumns),
                "rows": number(ScanNumber::SignatureRows)
            },
            "signature_max_distance_milli": number(ScanNumber::SignatureMaxDistanceMilli),
            "position_tolerance_px": number(ScanNumber::PositionTolerancePx),
            "min_overlap_instances": number(ScanNumber::MinOverlapInstances),
            "spacing": {
                "max_axis_gap_px": number(ScanNumber::SpacingMaxAxisGapPx),
                "columns": number(ScanNumber::SpacingColumns),
                "max_trailing_gap_px": number(ScanNumber::SpacingMaxTrailingGapPx),
                "max_cross_lead_px": number(ScanNumber::SpacingMaxCrossLeadPx),
                "max_cross_gap_px": number(ScanNumber::SpacingMaxCrossGapPx)
            }
        },
        "start": {"check": target_id(plan, step, START), "rewind": true}
    }) else {
        return Err(invalid("validation_failed", "a scan member is no object"));
    };
    if !without_end(scan) {
        member.insert(
            "end".to_owned(),
            json!({"check": target_id(plan, step, end)}),
        );
    }
    member.insert(
        "bounds".to_owned(),
        json!({
            "max_viewports": number(ScanNumber::MaxViewports),
            "max_reposition_viewports": number(ScanNumber::MaxRepositionViewports),
            "max_ms": number(ScanNumber::MaxMs)
        }),
    );
    if scan.stop == Some(ScanStop::Wanted) {
        member.insert("early_stop".to_owned(), json!("upper_bound"));
    }
    declared
        .as_object_mut()
        .ok_or_else(|| invalid("validation_failed", "a select's declaration is no object"))?
        .insert("scan".to_owned(), Value::Object(member));
    Ok(())
}

/// #463 S2-9a: the OCR target that reads a row's name, at the name region of the first screen
/// (the layout moves it to each row's anchor), with the select's read, as a fixed slot's target.
pub(crate) fn name_target(plan: &Plan, step: &PlanStep, spec: &SelectSpec) -> LabResult<Value> {
    let (rows, _) = rows_and_scan(spec)?;
    let mut expected = Vec::new();
    let mut seen = BTreeSet::new();
    for alias in spec.names.iter().flat_map(|name| name.aliases.iter()) {
        if expected.len() < EXPECTED_LIMIT && seen.insert(alias.as_str()) {
            expected.push(alias.clone());
        }
    }
    Ok(json!({
        "id": target_id(plan, step, NAME),
        "region": region_json(rows.name),
        "languages": spec.read.languages,
        "timeout_ms": spec.read.timeout_ms,
        "match_mode": "contains",
        "expected": expected,
        "case_sensitive": false,
        "minimum_confidence": f64::from(spec.read.minimum_confidence_milli) / 1000.0,
        "model_ref": spec.read.model_ref,
        "model_sha256": spec.read.model_sha256
    }))
}

/// #463 S2-9a: the template targets of a scrolling select: the row's anchor, searched in the list;
/// the start mark and the end mark, each searched at its own region, unmoved, so that it passes
/// only where the list stands at that end.
pub(crate) fn template_targets(
    plan: &Plan,
    step: &PlanStep,
    spec: &SelectSpec,
) -> LabResult<Vec<Value>> {
    let (_, scan) = rows_and_scan(spec)?;
    let mut parts = vec![(ANCHOR, scan.list), (START, scan.start)];
    if let Some(end) = scan.end {
        parts.push((END, end));
    }
    Ok(parts
        .into_iter()
        .map(|(part, region)| {
            json!({
                "id": target_id(plan, step, part),
                "template": asset_of(plan, step, part),
                "region": region_json(region),
                "provenance": {
                    "source": "lab_record",
                    "record_id": plan.record_id,
                    "step": step.number,
                    "record_index": step.step.index,
                    "select": part
                }
            })
        })
        .collect())
}

/// #463 S2-9a: every template a scrolling select of the package carries, `(target id, asset)`.
pub(crate) fn template_assets(plan: &Plan) -> Vec<(String, String)> {
    let mut assets = Vec::new();
    for (step, select) in plan.select_steps() {
        if select.scan.is_none() {
            continue;
        }
        for part in template_parts(&select.declaration) {
            assets.push((target_id(plan, step, part), asset_of(plan, step, part)));
        }
    }
    assets
}

/// The frame of `frame` in `cache`.
fn loaded_of<'c>(cache: &'c FrameCache, frame: &RecordedFrame) -> LabResult<&'c LoadedFrame> {
    cache.get(&frame.sha256).ok_or_else(|| {
        invalid(
            "validation_failed",
            format!("frame {} was not loaded", frame.frame_id),
        )
    })
}

/// #463 S2-9a: the templates of `step`'s scrolling select cut from its screens in `cache`: the
/// anchor and the start mark from the first screen, the end mark from the screen it is taken from
/// (the last after a forward swipe), each by its declared region.
pub(crate) fn scan_crops(
    plan: &Plan,
    step: &PlanStep,
    spec: &SelectSpec,
    state: &SelectScanState,
    cache: &FrameCache,
) -> LabResult<Vec<(String, Vec<u8>)>> {
    let (rows, scan) = rows_and_scan(spec)?;
    let first = state
        .screens
        .first()
        .ok_or_else(|| invalid("validation_failed", "a scrolling select has no screen"))?;
    let first = loaded_of(cache, &first.frame)?;
    let mut crops = vec![
        (target_id(plan, step, ANCHOR), crop_png(first, rows.anchor)?),
        (target_id(plan, step, START), crop_png(first, scan.start)?),
    ];
    if let Some(end) = scan.end {
        let last = state
            .screens
            .iter()
            .find(|screen| screen.regions.contains(&ScanRegion::End))
            .ok_or_else(|| {
                invalid(
                    "validation_failed",
                    "a scrolling select's end mark has no screen",
                )
            })?;
        crops.push((
            target_id(plan, step, END),
            crop_png(loaded_of(cache, &last.frame)?, end)?,
        ));
    }
    Ok(crops)
}

// ---- The numbers ----------------------------------------------------------------------------

/// The rows one screen showed, as the dry run read them: how many anchors matched there, and the
/// rectangle of each actionable row.
struct ScreenRows {
    role: ScanScreenRole,
    anchors: usize,
    rows: Vec<RecordRect>,
}

/// The spacing of the rows the screens showed, before the tolerance is added: the most anchors on
/// one screen, the most rows across the axis at one place, the largest gap along the axis between
/// two rows of a screen (and the first screen's first row from the list's leading edge), the last
/// screen after a forward swipe's last row from the list's trailing edge, and the largest lead and
/// gap across the axis.
#[derive(Default)]
struct RowMeasure {
    anchors: u64,
    columns: u64,
    axis_gap: u64,
    trailing: u64,
    cross_lead: u64,
    cross_gap: u64,
}

/// The rectangles `rects` grouped into rows: those whose start along `axis` lies within
/// `tolerance` of a row's first, in axis order, each row in cross order (the kernel's grouping).
fn grouped(axis: FoldAxis, rects: &[RecordRect], tolerance: i64) -> Vec<Vec<RecordRect>> {
    let mut sorted = rects.to_vec();
    sorted.sort_by_key(|rect| (along(axis, *rect).0, across(axis, *rect).0));
    let mut rows: Vec<Vec<RecordRect>> = Vec::new();
    for rect in sorted {
        let start = along(axis, rect).0;
        match rows.last_mut() {
            Some(row)
                if row
                    .first()
                    .is_some_and(|first| start - along(axis, *first).0 <= tolerance) =>
            {
                row.push(rect);
            }
            _ => rows.push(vec![rect]),
        }
    }
    for row in &mut rows {
        row.sort_by_key(|rect| across(axis, *rect).0);
    }
    rows
}

fn measure_rows(
    axis: FoldAxis,
    list: RecordRect,
    screens: &[ScreenRows],
    tolerance: i64,
) -> RowMeasure {
    let (list_start, list_length) = along(axis, list);
    let (list_cross, _) = across(axis, list);
    let last_forward = screens
        .iter()
        .rposition(|screen| screen.role != ScanScreenRole::Back);
    let mut measure = RowMeasure::default();
    for (index, screen) in screens.iter().enumerate() {
        measure.anchors = measure
            .anchors
            .max(u64::try_from(screen.anchors).unwrap_or(u64::MAX));
        let rows = grouped(axis, &screen.rows, tolerance);
        for row in &rows {
            measure.columns = measure
                .columns
                .max(u64::try_from(row.len()).unwrap_or(u64::MAX));
            if let Some(first) = row.first() {
                measure.cross_lead = measure
                    .cross_lead
                    .max(unsigned(across(axis, *first).0 - list_cross));
            }
            for pair in row.windows(2) {
                measure.cross_gap = measure
                    .cross_gap
                    .max(unsigned(across(axis, pair[1]).0 - across(axis, pair[0]).0));
            }
        }
        let starts = rows
            .iter()
            .filter_map(|row| row.first().map(|first| along(axis, *first).0))
            .collect::<Vec<_>>();
        for pair in starts.windows(2) {
            measure.axis_gap = measure.axis_gap.max(unsigned(pair[1] - pair[0]));
        }
        if screen.role == ScanScreenRole::First
            && let Some(first) = starts.first()
        {
            measure.axis_gap = measure.axis_gap.max(unsigned(first - list_start));
        }
        if Some(index) == last_forward
            && let Some(last) = starts.last()
        {
            measure.trailing = unsigned(list_start + list_length - last);
        }
    }
    measure
}

/// The numbers taken so far, each the request's given value or the Lab's measured one.
struct Numbers<'a> {
    index: u32,
    given: &'a BTreeMap<String, u64>,
    taken: Vec<ScanNumberValue>,
}

impl Numbers<'_> {
    /// The number `name`: the request's when it gives one (`given`, with the measured value beside
    /// it), else `measured` (from `from`); one the Lab cannot measure and the request does not give
    /// is `scan_number_not_measured`.
    fn take(
        &mut self,
        name: ScanNumber,
        from: ScanNumberSource,
        measured: Option<u64>,
    ) -> LabResult<u64> {
        self.take_labelled(name, from, measured, ScanNumberOrigin::Measured)
    }

    /// #463 S2-9b: [`Self::take`] of a number the Lab computes as `label` says (`derived` from a
    /// stated formula over measured numbers, `default` a constant of the generator), which is its
    /// origin when the request does not give it.
    fn take_labelled(
        &mut self,
        name: ScanNumber,
        from: ScanNumberSource,
        measured: Option<u64>,
        label: ScanNumberOrigin,
    ) -> LabResult<u64> {
        let word = number_word(name);
        let given = self.given.get(word.as_str()).copied();
        let (value, origin) = match (given, measured) {
            (Some(value), _) => (value, ScanNumberOrigin::Given),
            (None, Some(value)) => (value, label),
            (None, None) => {
                let index = self.index;
                let mut refused = select_invalid(
                    Some(index),
                    SelectProblem::ScanNumberNotMeasured,
                    format!(
                        "the number '{word}' of the scrolling select of step {index} is measured \
                         from the demonstration's swipes, which do not hold what it is measured \
                         from (a swipe recorded without its after loop); give it in the request's \
                         scan.numbers; nothing was written"
                    ),
                );
                if let Some(details) = refused.details.as_mut().and_then(Value::as_object_mut) {
                    details.insert("number".to_owned(), json!(word));
                }
                return Err(refused);
            }
        };
        self.taken.push(ScanNumberValue {
            name,
            value,
            origin,
            from,
            measured: given.and(measured),
        });
        Ok(value)
    }
}

/// The after loop's record of a swipe.
fn after_of(swipe: &RecordingStep) -> Option<&AfterCaptureRecord> {
    swipe
        .click
        .as_ref()
        .and_then(|click| click.execution.as_ref())
        .and_then(|execution| execution.after_capture.as_ref())
}

/// The recorded duration of a swipe: its execution's, else its drag's.
fn swipe_duration(swipe: &RecordingStep) -> Option<u64> {
    let click = swipe.click.as_ref()?;
    click
        .execution
        .as_ref()
        .and_then(|execution| execution.duration_ms)
        .or(match click.gesture {
            Some(ClickGesture::Drag { duration_ms, .. }) => Some(duration_ms),
            _ => None,
        })
}

/// #463 S2-9a: every number of a scrolling select's scan and layout, in this order, each the
/// request's given value or the one the Lab measures (`b` and `e` the list's and the row's extents
/// along the axis, `V` the screens after the forward swipes and the first one):
/// the advance's window, 1 to `b − e`, and the reposition's largest step `b − e`, from the list and
/// the row; the advance's duration, the first forward swipe's; the settle's interval, the first
/// forward swipe's after loop's, and its timeout, twice the longest time a swipe's screen took to
/// be captured after it plus the interval, from the fold's swipes; the position tolerance,
/// `max(2, e / 10)`, and the edge margin, the tolerance, from the row; the signature's grid, the row
/// in cells as square as its shape allows (at most 8) along its long side and one across; the
/// signature distance, the Runtime's frame-change bound; one overlapping row, since every pair
/// matches by its name read; the spacing (the rows across the axis at one place, the gaps along and
/// across the axis, the last row from the list's trailing edge on the last screen after a forward
/// swipe, each plus the tolerance) and the most anchors (that plus 2, at most 64), from the rows
/// the screens show (`screens`; `None`, before they are read: the widest the list allows); the
/// viewports, `2V` (at least 2, at most 128), and the return viewports, `2V` (at most 128), from
/// the fold's swipes; and the scan's time, the viewports and the return viewports and one more,
/// each an advance and its settle, and one screen's reads of every anchor. #463 S2-9b: for a scan
/// without an end mark, whose demonstration stops at the wanted row, the numbers taken from the
/// screens up to the list's end are `derived` instead: the trailing gap `b` (the kernel checks it
/// only where an end check passes), the most anchors `(b / e + 2)` per column and no fewer than a
/// screen showed plus 2 (at most 64), the viewports `2V + 2` (two more screens tell that the list
/// stopped), the return viewports `2V` and the scan's time from those.
fn scan_numbers(
    index: u32,
    spec: &SelectSpec,
    axis: FoldAxis,
    swipes: &[(&RecordingStep, FoldDirection)],
    screens: Option<&[ScreenRows]>,
) -> LabResult<Vec<ScanNumberValue>> {
    let (rows, scan) = rows_and_scan(spec)?;
    let mut numbers = Numbers {
        index,
        given: &scan.numbers,
        taken: Vec::new(),
    };
    let (_, list_length) = along(axis, scan.list);
    let (_, list_cross) = across(axis, scan.list);
    let (_, row_length) = along(axis, rows.row);
    let (_, row_cross) = across(axis, rows.row);
    let room = unsigned(list_length - row_length).max(1);
    let forward = swipes
        .iter()
        .filter(|(_, direction)| *direction == FoldDirection::Forward)
        .map(|(swipe, _)| *swipe)
        .collect::<Vec<_>>();
    let first_forward = forward.first().copied();
    let waited = swipes
        .iter()
        .map(|(swipe, _)| after_of(swipe).map(|record| record.waited_ms))
        .collect::<Option<Vec<_>>>()
        .and_then(|waited| waited.into_iter().max());
    let screen_count = u64::try_from(forward.len()).unwrap_or(u64::MAX) + 1;

    numbers.take(
        ScanNumber::AdvanceMinContentStepPx,
        ScanNumberSource::ListAndRow,
        Some(1),
    )?;
    numbers.take(
        ScanNumber::AdvanceMaxContentStepPx,
        ScanNumberSource::ListAndRow,
        Some(room),
    )?;
    let duration = numbers.take(
        ScanNumber::AdvanceDurationMs,
        ScanNumberSource::FirstForwardSwipe,
        first_forward.and_then(swipe_duration),
    )?;
    numbers.take(
        ScanNumber::RepositionMaxContentStepPx,
        ScanNumberSource::ListAndRow,
        Some(room),
    )?;
    let interval = numbers.take(
        ScanNumber::SettleIntervalMs,
        ScanNumberSource::FirstForwardSwipe,
        first_forward
            .and_then(after_of)
            .and_then(|record| record.interval_ms),
    )?;
    let timeout = numbers.take(
        ScanNumber::SettleTimeoutMs,
        ScanNumberSource::FoldSwipes,
        waited.map(|waited| waited.saturating_mul(2).saturating_add(interval)),
    )?;
    let tolerance = numbers.take(
        ScanNumber::PositionTolerancePx,
        ScanNumberSource::Row,
        Some((unsigned(row_length) / 10).max(2)),
    )?;
    numbers.take(
        ScanNumber::EdgeMarginPx,
        ScanNumberSource::Row,
        Some(tolerance),
    )?;
    let cells = (unsigned(row_cross) / unsigned(row_length).max(1)).clamp(1, 8);
    let (columns, signature_rows) = match axis {
        FoldAxis::Vertical => (cells, 1),
        FoldAxis::Horizontal => (1, cells),
    };
    numbers.take(
        ScanNumber::SignatureColumns,
        ScanNumberSource::Row,
        Some(columns),
    )?;
    numbers.take(
        ScanNumber::SignatureRows,
        ScanNumberSource::Row,
        Some(signature_rows),
    )?;
    numbers.take(
        ScanNumber::SignatureMaxDistanceMilli,
        ScanNumberSource::FrameChangeBound,
        Some(u64::from(FRAME_CHANGE_MAX_MILLI)),
    )?;
    numbers.take(
        ScanNumber::MinOverlapInstances,
        ScanNumberSource::NamesRead,
        Some(1),
    )?;
    let measure = screens.map(|screens| {
        measure_rows(
            axis,
            scan.list,
            screens,
            i64::try_from(tolerance).unwrap_or(i64::MAX),
        )
    });
    let spaced = |pick: fn(&RowMeasure) -> u64, widest: u64| {
        Some(
            measure
                .as_ref()
                .map_or(widest, |found| pick(found).saturating_add(tolerance)),
        )
    };
    // #463 S2-9b: a scan without an end mark was demonstrated up to the wanted row, not to the
    // list's end, so the numbers the first kind measures on the screens up to the end are derived
    // from what its screens do show (`derived`), each by its stated formula below.
    let endless = without_end(scan);
    let label = if endless {
        ScanNumberOrigin::Derived
    } else {
        ScanNumberOrigin::Measured
    };
    let columns = numbers.take(
        ScanNumber::SpacingColumns,
        ScanNumberSource::Screens,
        Some(measure.as_ref().map_or(1, |found| found.columns.max(1))),
    )?;
    numbers.take(
        ScanNumber::SpacingMaxAxisGapPx,
        ScanNumberSource::Screens,
        spaced(|measure| measure.axis_gap, unsigned(list_length)),
    )?;
    // #463 S2-9b: the kernel checks the trailing gap only on the screen where the end check passes,
    // which a scan without an end never reaches: the widest the list allows, `b`.
    if endless {
        numbers.take_labelled(
            ScanNumber::SpacingMaxTrailingGapPx,
            ScanNumberSource::ListAndRow,
            Some(unsigned(list_length)),
            label,
        )?;
    } else {
        numbers.take(
            ScanNumber::SpacingMaxTrailingGapPx,
            ScanNumberSource::Screens,
            spaced(|measure| measure.trailing, unsigned(list_length)),
        )?;
    }
    numbers.take(
        ScanNumber::SpacingMaxCrossLeadPx,
        ScanNumberSource::Screens,
        spaced(|measure| measure.cross_lead, unsigned(list_cross)),
    )?;
    numbers.take(
        ScanNumber::SpacingMaxCrossGapPx,
        ScanNumberSource::Screens,
        spaced(|measure| measure.cross_gap, unsigned(list_cross)),
    )?;
    let shown_anchors = measure.as_ref().map_or(MAX_INSTANCES, |found| {
        found.anchors.saturating_add(2).min(MAX_INSTANCES)
    });
    let instances = if endless {
        // #463 S2-9b: rows not shown may be shorter than those shown: as many rows as the list's
        // length holds whole and the two its edges cut, `b / e + 2`, in each column, and no fewer
        // than a screen showed plus 2, at most 64.
        let fitting = (unsigned(list_length) / unsigned(row_length).max(1))
            .saturating_add(2)
            .saturating_mul(columns);
        numbers.take_labelled(
            ScanNumber::MaxInstances,
            ScanNumberSource::ListAndRow,
            Some(fitting.max(shown_anchors).min(MAX_INSTANCES)),
            label,
        )?
    } else {
        numbers.take(
            ScanNumber::MaxInstances,
            ScanNumberSource::Screens,
            Some(shown_anchors),
        )?
    };
    // #463 S2-9b: a scan without an end mark tells that the list stopped moving only on two more
    // screens than its last new one, so its viewports allow two more beyond twice the screens shown
    // up to the wanted row: `2V + 2`; its return viewports `2V`.
    let stopping = if endless { 2 } else { 0 };
    let viewports = numbers.take_labelled(
        ScanNumber::MaxViewports,
        ScanNumberSource::FoldSwipes,
        Some(
            screen_count
                .saturating_mul(2)
                .saturating_add(stopping)
                .clamp(2, MAX_VIEWPORTS),
        ),
        label,
    )?;
    let returns = numbers.take_labelled(
        ScanNumber::MaxRepositionViewports,
        ScanNumberSource::FoldSwipes,
        Some(screen_count.saturating_mul(2).clamp(1, MAX_VIEWPORTS)),
        label,
    )?;
    numbers.take_labelled(
        ScanNumber::MaxMs,
        ScanNumberSource::ScanMoves,
        Some(
            viewports
                .saturating_add(returns)
                .saturating_add(1)
                .saturating_mul(duration.saturating_add(timeout))
                .saturating_add(instances.saturating_mul(spec.read.timeout_ms)),
        ),
        label,
    )?;
    Ok(numbers.taken)
}

// ---- The screens ----------------------------------------------------------------------------

/// The stored frame `id` of the recording.
fn frame_of(recording: &LabRecording, press: u32, id: &str) -> LabResult<RecordedFrame> {
    recording
        .steps
        .iter()
        .flat_map(|step| step.frames.iter())
        .find(|frame| frame.frame_id == id)
        .cloned()
        .ok_or_else(|| screen_missing(press, Some(id)))
}

fn screen_missing(press: u32, frame_id: Option<&str>) -> LabError {
    with_details(
        blocked(
            "record_step_frame_missing",
            format!(
                "a screen of the swipes folded into step {press} is not in the recording: the \
                 scrolling select reads the screen before the first swipe and the screen after \
                 each swipe"
            ),
        ),
        json!({"step": press, "frame_id": frame_id}),
    )
}

/// The screens of the demonstration of the fold `fold` of the press step `press`, in order: the
/// first screen (the first swipe's screen before it), then the screen after each swipe, forward or
/// back; with the regions each image is taken from (the list on every one; the row, its anchor,
/// its name, its click box and the start mark on the first; the end mark on the last screen after
/// a forward swipe, when it is declared).
fn screens_of(
    recording: &LabRecording,
    press: &RecordingStep,
    fold: &StepScanFold,
    rows: SelectRowsSpec,
    scan: &SelectScanSpec,
) -> LabResult<Vec<ScanScreen>> {
    let index = press.index;
    let first = step_ref(recording, fold.first).ok_or_else(|| screen_missing(index, None))?;
    let first_id = press_frame(first)
        .or_else(|| live_primary(first).map(|frame| frame.frame_id.clone()))
        .ok_or_else(|| screen_missing(index, None))?;
    let mut regions = vec![
        ScanRegion::List,
        ScanRegion::Row,
        ScanRegion::Anchor,
        ScanRegion::Name,
    ];
    if rows.click.is_some() {
        regions.push(ScanRegion::Click);
    }
    regions.push(ScanRegion::Start);
    let mut screens = vec![ScanScreen {
        frame: frame_of(recording, index, &first_id)?,
        step: fold.first,
        role: ScanScreenRole::First,
        regions,
        names: Vec::new(),
        content_offset_px: None,
    }];
    for moved in &fold.moves {
        let swipe = step_ref(recording, moved.step).ok_or_else(|| screen_missing(index, None))?;
        let after = swipe
            .click
            .as_ref()
            .and_then(|click| click.execution.as_ref())
            .and_then(|execution| execution.after_frame.clone())
            .ok_or_else(|| screen_missing(index, None))?;
        screens.push(ScanScreen {
            frame: frame_of(recording, index, &after)?,
            step: moved.step,
            role: match moved.direction {
                FoldDirection::Forward => ScanScreenRole::Forward,
                FoldDirection::Back => ScanScreenRole::Back,
            },
            regions: vec![ScanRegion::List],
            names: Vec::new(),
            content_offset_px: None,
        });
    }
    if scan.end.is_some()
        && let Some(last) = screens
            .iter_mut()
            .rev()
            .find(|screen| screen.role != ScanScreenRole::Back)
    {
        last.regions.push(ScanRegion::End);
    }
    Ok(screens)
}

/// The screens met going forward: the first one and those after a forward swipe.
fn forward_screens(screens: &[ScanScreen]) -> Vec<&ScanScreen> {
    screens
        .iter()
        .filter(|screen| screen.role != ScanScreenRole::Back)
        .collect()
}

// ---- The kernel's dry runs ------------------------------------------------------------------

/// A probe package admitted by the kernel: its plan, its files, the kernel's task and the select
/// step's operation.
struct Admitted {
    plan: Plan,
    entries: BTreeMap<String, Vec<u8>>,
    prepared: PreparedContainedTask,
    operation: String,
}

/// `kernel_refused`: the kernel's admission refuses the probe of step `index`.
fn kernel_refused(index: u32, error: &ContainedTaskError) -> LabError {
    let mut refused = select_invalid(
        Some(index),
        SelectProblem::KernelRefused,
        format!(
            "the kernel's admission refuses the scrolling select of step {index} as record stop \
             would generate it: {error}; nothing was written"
        ),
    );
    if let Some(details) = refused.details.as_mut().and_then(Value::as_object_mut) {
        details.insert("loader_code".to_owned(), json!(error.code().as_str()));
        details.insert("detail".to_owned(), json!(error.detail()));
    }
    refused
}

/// The probe package of the step `step` (its select holding the scan `state`) with the template
/// crops `crops` of its marks and those of its scan cut from `cache`, admitted by the kernel.
fn admitted_probe(
    recording: &LabRecording,
    step: &RecordingStep,
    marks: Vec<RecordedMark>,
    crops: &BTreeMap<String, Vec<u8>>,
    cache: &FrameCache,
    ocr: &LabOcr,
) -> LabResult<Admitted> {
    let plan = select_probe_plan(recording, step, marks)?;
    let first = plan
        .steps
        .first()
        .ok_or_else(|| invalid("validation_failed", "the probe package has no step"))?;
    let (select, state) = first
        .step
        .select
        .as_ref()
        .and_then(|select| select.scan.as_ref().map(|state| (select, state)))
        .ok_or_else(|| invalid("validation_failed", "the probe's step holds no scan"))?;
    let mut all = crops.clone();
    all.extend(scan_crops(&plan, first, &select.declaration, state, cache)?);
    let entries = render(&plan, None, &all)?;
    let operation = plan.operation_id(first);
    let prepared = PreparedContainedTask::load_content_entries(
        PROBE_INSTANCE,
        entries.clone(),
        ocr.provider(),
        Instant::now() + PROBE_BUDGET,
    )
    .map_err(|error| kernel_refused(step.index, &error))?;
    Ok(Admitted {
        plan,
        entries,
        prepared,
        operation,
    })
}

/// A frame of its own for the kernel, decoded from the stored bytes (a frame is not cloned).
fn owned_frame(loaded: &LoadedFrame) -> LabResult<Frame> {
    Frame::from_png(loaded.png.clone(), CaptureBackendName::AdbScreencap).map_err(|error| {
        invalid(
            "record_frame_unreadable",
            format!("failed to decode a stored frame: {error}"),
        )
    })
}

/// The kernel's offline entry (`dry_run_scan_decide`, called, never copied) of the select
/// `operation` of `prepared` over the forward screens `forward`, each fed twice as a forward run
/// captures it (the frame dispatched and its settle, then each viewport and its settle), with the
/// package's variables at their defaults, an empty fact snapshot at `now_unix_ms`, and the step's
/// sealed policy or `document`. A failure of the run is its outcome, with the kernel's code and
/// detail.
fn offline_entry(
    prepared: &PreparedContainedTask,
    operation: &str,
    forward: &[&LoadedFrame],
    document: Option<&Value>,
    now_unix_ms: u64,
) -> LabResult<Result<ScanDryRunDecided, Value>> {
    let mut frames = Vec::with_capacity(forward.len() * 2);
    for loaded in forward {
        frames.push(owned_frame(loaded)?);
        frames.push(owned_frame(loaded)?);
    }
    let snapshot = InstanceFactSnapshot {
        snapshot_id: "lab.record.select".to_owned(),
        ledger_position: 0,
        context: InstanceFactContext {
            instance_id: PROBE_INSTANCE.to_owned(),
            server_id: String::new(),
            game_id: String::new(),
        },
        records: Vec::new(),
    };
    let parameters = prepared.default_run_values();
    let run = match document {
        None => dry_run_scan_decide(
            prepared,
            operation,
            frames,
            (&snapshot, now_unix_ms),
            &parameters,
            None,
        ),
        Some(document) => {
            let policy = serde_json::from_value(document.clone()).map_err(|error| {
                invalid(
                    "validation_failed",
                    format!("the select's policy document does not decode: {error}"),
                )
            })?;
            dry_run_scan_decide(
                prepared,
                operation,
                frames,
                (&snapshot, now_unix_ms),
                &parameters,
                Some(&policy),
            )
        }
    };
    Ok(run.map_err(|error| json!({"code": error.code().as_str(), "detail": error.detail()})))
}

/// The decision of an offline run by its kind: `selected` with the chosen member's name and
/// position when nothing blocks it, `blocked` when an unknown position blocks it (rule 2 or 3),
/// else the evaluator's kind.
fn scan_decision(decided: &ScanDryRunDecided) -> LabResult<ScanDecision> {
    let blocking = decided.unknown_check.blocking != TaskSelectionBlocking::None;
    Ok(match mirrored_outcome(&decided.decision.outcome)? {
        TaskSelectionOutcome::Selected { .. } => match &decided.chosen {
            Some(member) if !blocking => ScanDecision::Selected {
                name: member.identity.clone(),
                position: member.position_index,
            },
            _ => ScanDecision::Blocked,
        },
        TaskSelectionOutcome::Empty | TaskSelectionOutcome::Insufficient { .. } if blocking => {
            ScanDecision::Blocked
        }
        TaskSelectionOutcome::Empty => ScanDecision::Empty,
        TaskSelectionOutcome::Insufficient { .. } => ScanDecision::Insufficient,
        TaskSelectionOutcome::Ambiguous { .. } => ScanDecision::Ambiguous,
        TaskSelectionOutcome::Unknown { .. } => ScanDecision::Unknown,
    })
}

/// The actionable rows of a screen's dry run whose name read no known name, with why.
fn unread_rows(decided: &Decided) -> Vec<(u32, Value)> {
    decided
        .instances
        .iter()
        .filter(|instance| instance.actionable)
        .filter_map(|instance| {
            instance
                .unread
                .as_ref()
                .map(|cause| (instance.index, cause.clone()))
        })
        .collect()
}

/// The names read on a screen's actionable rows, in the order of the rows along `axis`.
fn names_read(decided: &Decided, axis: FoldAxis) -> Vec<String> {
    let mut rows = decided
        .instances
        .iter()
        .filter(|instance| instance.actionable)
        .collect::<Vec<_>>();
    rows.sort_by_key(|instance| (along(axis, instance.rect).0, across(axis, instance.rect).0));
    rows.into_iter()
        .filter_map(|instance| instance.name.clone())
        .collect()
}

/// What one offline run gives the select's outcome and its refusals: its coverage, positions,
/// decision and unknown positions.
struct Offline {
    coverage: ScanCoverageState,
    positions: u32,
    decision: ScanDecision,
    unknown: Vec<Value>,
    offsets: Vec<i64>,
}

fn offline_of(decided: &ScanDryRunDecided) -> LabResult<Offline> {
    Ok(Offline {
        coverage: decided.scan.coverage,
        positions: decided.scan.positions,
        decision: scan_decision(decided)?,
        unknown: decided
            .scan
            .unknown
            .iter()
            .map(|unknown| json!({"position": unknown.position_index, "cause": unknown.cause}))
            .collect(),
        offsets: decided
            .scan
            .viewports
            .iter()
            .map(|viewport| viewport.content_offset_px)
            .collect(),
    })
}

/// The content step of each forward swipe, as the offline run aligned the screens around it: the
/// difference of the content offsets of the viewports before and after it.
fn content_steps(screens: &[ScanScreen], offsets: &[i64]) -> Vec<ScanMeasuredStep> {
    forward_screens(screens)
        .iter()
        .skip(1)
        .zip(offsets.windows(2))
        .map(|(screen, pair)| ScanMeasuredStep {
            step: screen.step,
            content_step_px: pair[1] - pair[0],
        })
        .collect()
}

/// The first name of the select that no screen shows, for the run that must end empty.
fn absent_name(spec: &SelectSpec, screens: &[ScanScreen]) -> Option<String> {
    let shown = screens
        .iter()
        .flat_map(|screen| screen.names.iter().map(String::as_str))
        .collect::<BTreeSet<_>>();
    spec.names
        .iter()
        .map(|name| name.id.as_str())
        .find(|id| !shown.contains(id))
        .map(str::to_owned)
}

// ---- The self-test at mark time -------------------------------------------------------------

/// The step `step` with `current` as its select, holding the scan `state`.
fn with_scan(step: &RecordingStep, current: &StepSelect, state: &SelectScanState) -> RecordingStep {
    let mut probe = step.clone();
    probe.select = Some(StepSelect {
        scan: Some(state.clone()),
        ..current.clone()
    });
    probe
}

/// `state` with the advance's window the widest the list allows, 1 to `room`: the window the
/// content steps are measured in before they are held to the declared one.
fn widest(state: &SelectScanState, room: u64) -> SelectScanState {
    let mut wide = state.clone();
    for number in &mut wide.numbers {
        match number.name {
            ScanNumber::AdvanceMinContentStepPx => number.value = 1,
            ScanNumber::AdvanceMaxContentStepPx => number.value = room,
            _ => {}
        }
    }
    wide
}

/// #463 S2-9a: the self-test of a scrolling select at mark time (its rules shared with a fixed-slot
/// select already held, `select::check_step_select`): the step holds a fold (`scan_without_fold`);
/// every region lies inside the frame (`scan_region_outside_frame`), the row inside the list
/// (`scan_region_outside_list`), the anchor, the name region and the click box inside the row
/// (`scan_region_outside_row`); an OCR model reads the names (`lab_ocr_unavailable`); the numbers
/// ([`scan_numbers`]). Then, on probe packages the kernel admits (`kernel_refused`): every screen of
/// the demonstration read by the kernel's dry run of the step, each row the anchor finds there
/// reading a known name (`scan_rows_not_found`, `select_name_unread`, `self_test_error`); the start
/// mark passing on the first screen and on no later forward screen (`scan_start_mark_missing`,
/// `scan_start_mark_repeated`), the end mark on the last screen after a forward swipe and, from the
/// first forward screen it passes on, on every later one (`scan_end_mark_missing`,
/// `scan_end_mark_lost`); the content step of every forward swipe, measured in the widest window,
/// inside the declared window (`scan_step_outside_window`); the kernel's offline entry over the
/// forward screens deciding the pressed name over complete coverage with every position read
/// (`scan_coverage_incomplete`, `scan_position_unread`, `scan_decision_mismatch`), and a name no
/// screen shows empty over complete coverage (`scan_absent_not_empty`); and on the step's own live
/// frames the pressed name selecting a row whose click box holds the press (`select_pressed_absent`,
/// `select_press_mismatch`). Without an end mark the offline entry is not run: `record ready`
/// lists it (`scan_end_not_recorded`). #463 S2-9b, straight to the wanted item: the offline entry
/// over the forward screens must stop early on the pressed name ([`decided_coverage`]); without an
/// end mark no name no screen shows is run (a scan without an end judges an absent item only where
/// the list stops moving, on screens no demonstration recorded), and the smallest content step is at
/// most twice the position tolerance plus one (`scan_min_step_too_large`), so that where the list
/// stops on the device the run meets its own failure before the alignment's.
pub(crate) fn check_scan_select(
    recording: &LabRecording,
    probe: SelectProbe,
    frames: &[SelectFrame<'_>],
    ocr: &LabOcr,
    current: StepSelect,
) -> LabResult<SelectChecked> {
    let index = probe.step.index;
    let spec = current.declaration.clone();
    let (rows, scan) = rows_and_scan(&spec)?;
    let refuse = |problem: SelectProblem, message: String| {
        select_invalid(
            Some(index),
            problem,
            format!("{message}; nothing was written"),
        )
    };
    let Some(fold) = probe.step.scan_fold.clone() else {
        return Err(refuse(
            SelectProblem::ScanWithoutFold,
            format!(
                "step {index} holds no fold of the swipes recorded before it, so its scrolling \
                 select has no screens of the list to read; fold them first (step_action fold_scan \
                 of step {index})"
            ),
        ));
    };
    let size = recording.coordinate_space.ok_or_else(|| {
        blocked(
            "record_step_frame_missing",
            "the recording has no frame, so it has no coordinate space",
        )
    })?;
    let mut regions = vec![
        (ScanRegion::List, scan.list),
        (ScanRegion::Row, rows.row),
        (ScanRegion::Anchor, rows.anchor),
        (ScanRegion::Name, rows.name),
        (ScanRegion::Click, rows.click_rect()),
        (ScanRegion::Start, scan.start),
    ];
    if let Some(end) = scan.end {
        regions.push((ScanRegion::End, end));
    }
    if let Some((region, _)) = regions.iter().find(|(_, rect)| !rect_inside(*rect, size)) {
        return Err(refuse(
            SelectProblem::ScanRegionOutsideFrame,
            format!(
                "the {} region of the scrolling select of step {index} does not lie inside the \
                 {}x{} frame",
                region_word(*region),
                size.width,
                size.height
            ),
        ));
    }
    if !rect_contains(scan.list, rows.row) {
        return Err(refuse(
            SelectProblem::ScanRegionOutsideList,
            format!(
                "the row of the scrolling select of step {index} does not lie inside its list: the \
                 row is one row of the list as its first screen shows it"
            ),
        ));
    }
    for (region, rect) in [
        (ScanRegion::Anchor, rows.anchor),
        (ScanRegion::Name, rows.name),
        (ScanRegion::Click, rows.click_rect()),
    ] {
        if !rect_contains(rows.row, rect) {
            return Err(refuse(
                SelectProblem::ScanRegionOutsideRow,
                format!(
                    "the {} region of the scrolling select of step {index} does not lie inside its \
                     row",
                    region_word(region)
                ),
            ));
        }
    }
    let mut screens = screens_of(recording, &probe.step, &fold, rows, scan)?;
    if ocr.ocr_provider().is_none() {
        return Err(ocr.unavailable_error(None).unwrap_or_else(|| {
            blocked(
                LabCode::LabOcrUnavailable.as_str(),
                format!(
                    "the scrolling select of step {index} reads the names of the list's rows with \
                     the OCR model, and measures its numbers on them"
                ),
            )
        }));
    }
    let mut cache = FrameCache::new();
    for screen in &screens {
        if !cache.contains_key(&screen.frame.sha256) {
            cache.insert(
                screen.frame.sha256.clone(),
                read_stored_frame(&screen.frame)?,
            );
        }
    }
    let mut swipes = Vec::with_capacity(fold.moves.len());
    for moved in &fold.moves {
        let swipe = step_ref(recording, moved.step).ok_or_else(|| screen_missing(index, None))?;
        swipes.push((swipe, moved.direction));
    }
    let (advance_from, advance_to) = swipes
        .first()
        .and_then(|(swipe, _)| swipe.click.as_ref())
        .and_then(|click| click.execution.as_ref())
        .and_then(|execution| Some((execution.point, execution.to_point?)))
        .ok_or_else(|| screen_missing(index, None))?;
    let mut state = SelectScanState {
        axis: fold.axis,
        advance_from,
        advance_to,
        screens: screens.clone(),
        numbers: scan_numbers(index, &spec, fold.axis, &swipes, None)?,
        outcome: None,
    };
    let instant = recording.record_started_at_unix_ms;
    let mut rejections = Vec::new();
    let reject = |rejections: &mut Vec<MarkRejection>, reason: &str, detail: Value| {
        rejections.push(MarkRejection::new(SELECT_REJECTION_ID, reason, detail));
    };

    // Every screen read by the kernel's dry run of the step, on a probe whose numbers are the
    // widest the list allows.
    let reading = admitted_probe(
        recording,
        &with_scan(&probe.step, &current, &state),
        probe.marks.clone(),
        &probe.crops,
        &cache,
        ocr,
    )?;
    let mut read_rows = Vec::with_capacity(screens.len());
    for screen in &mut screens {
        let frame_id = screen.frame.frame_id.clone();
        let loaded = loaded_of(&cache, &screen.frame)?;
        let decided = match decide(
            &reading.prepared,
            &reading.operation,
            &loaded.frame,
            None,
            instant,
        )? {
            Ok(decided) => decided,
            Err(failure) => {
                reject(
                    &mut rejections,
                    SELF_TEST_ERROR,
                    json!({"frame_id": frame_id, "failure": failure}),
                );
                continue;
            }
        };
        for (instance, cause) in unread_rows(&decided) {
            reject(
                &mut rejections,
                NAME_UNREAD,
                json!({"frame_id": frame_id, "row": instance, "cause": cause}),
            );
        }
        let actionable = decided
            .instances
            .iter()
            .filter(|instance| instance.actionable)
            .map(|instance| instance.rect)
            .collect::<Vec<_>>();
        if screen.role == ScanScreenRole::First && actionable.is_empty() {
            reject(
                &mut rejections,
                ROWS_NOT_FOUND,
                json!({"frame_id": frame_id}),
            );
        }
        screen.names = names_read(&decided, fold.axis);
        read_rows.push(ScreenRows {
            role: screen.role,
            anchors: decided.instances.len(),
            rows: actionable,
        });
    }
    if !rejections.is_empty() {
        return Ok(SelectChecked {
            select: current,
            click_rect: None,
            rejections,
        });
    }
    state.screens.clone_from(&screens);
    state.numbers = scan_numbers(index, &spec, fold.axis, &swipes, Some(read_rows.as_slice()))?;
    if let Some(rejection) = stop_window_rejection(scan, &state) {
        return Ok(SelectChecked {
            select: current,
            click_rect: None,
            rejections: vec![rejection],
        });
    }

    // The start and the end marks on the forward screens, by the package's own evaluator.
    let checked = admitted_probe(
        recording,
        &with_scan(&probe.step, &current, &state),
        probe.marks.clone(),
        &probe.crops,
        &cache,
        ocr,
    )?;
    let forward = forward_screens(&screens);
    {
        let bundle = evaluator_bundle(&checked.entries, ocr)?;
        let loaded_bundle = bundle.loaded_bundle();
        let evaluator = loaded_bundle.evaluator().ok_or_else(|| {
            invalid(
                "validation_failed",
                "the probe package gives no recognition evaluator",
            )
        })?;
        let first = checked
            .plan
            .steps
            .first()
            .ok_or_else(|| invalid("validation_failed", "the probe package has no step"))?;
        let mut marks = vec![(START, target_id(&checked.plan, first, START))];
        if scan.end.is_some() {
            marks.push((END, target_id(&checked.plan, first, END)));
        }
        for (part, target) in marks {
            let mut passes = Vec::with_capacity(forward.len());
            for screen in &forward {
                let loaded = loaded_of(&cache, &screen.frame)?;
                match evaluator.evaluate_target(&loaded.scene, &target) {
                    Ok(evaluation) => passes.push(evaluation.passed),
                    Err(error) => {
                        reject(
                            &mut rejections,
                            SELF_TEST_ERROR,
                            json!({"frame_id": screen.frame.frame_id, "target": target,
                                   "error": error.to_string()}),
                        );
                        passes.push(false);
                    }
                }
            }
            let frame_at = |at: usize| forward.get(at).map(|screen| screen.frame.frame_id.clone());
            if part == START {
                if passes.first() != Some(&true) {
                    reject(
                        &mut rejections,
                        START_MARK_MISSING,
                        json!({"frame_id": frame_at(0)}),
                    );
                }
                if let Some(at) = passes.iter().skip(1).position(|passed| *passed) {
                    reject(
                        &mut rejections,
                        START_MARK_REPEATED,
                        json!({"frame_id": frame_at(at + 1)}),
                    );
                }
            } else {
                let last = passes.len().saturating_sub(1);
                if passes.last() != Some(&true) {
                    reject(
                        &mut rejections,
                        END_MARK_MISSING,
                        json!({"frame_id": frame_at(last)}),
                    );
                } else if let Some(from) = passes.iter().position(|passed| *passed)
                    && let Some(at) = passes.iter().skip(from).position(|passed| !*passed)
                {
                    reject(
                        &mut rejections,
                        END_MARK_LOST,
                        json!({"frame_id": frame_at(from + at), "passed_on": frame_at(from)}),
                    );
                }
            }
        }
    }

    // The pressed name on the step's own live frames, as a fixed-slot select's self-test presses
    // it, its rows read as on the screens.
    let first = checked
        .plan
        .steps
        .first()
        .ok_or_else(|| invalid("validation_failed", "the probe package has no step"))?;
    let pressed_document = (spec.pressed_name() != spec.wanted.default)
        .then(|| select_policy_document(&checked.plan, first, &spec, spec.pressed_name()));
    let executed = probe
        .step
        .click
        .as_ref()
        .filter(|click| click.executed())
        .and_then(|click| click.execution.as_ref())
        .map(|execution| execution.point);
    let mut decisions = Vec::new();
    let mut click_rect = None;
    for frame in frames {
        let frame_id = frame.entry.frame_id.as_str();
        let default = match decide(
            &checked.prepared,
            &checked.operation,
            &frame.loaded.frame,
            None,
            instant,
        )? {
            Ok(decided) => decided,
            Err(failure) => {
                reject(
                    &mut rejections,
                    SELF_TEST_ERROR,
                    json!({"frame_id": frame_id, "failure": failure}),
                );
                continue;
            }
        };
        let unread = unread_rows(&default);
        if !unread.is_empty() {
            for (instance, cause) in unread {
                reject(
                    &mut rejections,
                    NAME_UNREAD,
                    json!({"frame_id": frame_id, "row": instance, "cause": cause}),
                );
            }
            continue;
        }
        let pressed = match &pressed_document {
            None => default.kind,
            Some(document) => match decide(
                &checked.prepared,
                &checked.operation,
                &frame.loaded.frame,
                Some(document),
                instant,
            )? {
                Ok(decided) => decided.kind,
                Err(failure) => {
                    reject(
                        &mut rejections,
                        SELF_TEST_ERROR,
                        json!({"frame_id": frame_id, "failure": failure}),
                    );
                    continue;
                }
            },
        };
        if frame.entry.role == "primary" {
            let Some(row) = pressed.slot() else {
                reject(
                    &mut rejections,
                    PRESSED_ABSENT,
                    json!({"frame_id": frame_id, "pressed": spec.pressed_name(),
                           "decision": pressed}),
                );
                continue;
            };
            let rect = default.clicks.get(&row).copied().ok_or_else(|| {
                invalid(
                    "validation_failed",
                    format!("the dry run of step {index} selected row {row}, which it lacks"),
                )
            })?;
            if let Some(point) = executed
                && !holds(point, rect)
            {
                reject(
                    &mut rejections,
                    PRESS_MISMATCH,
                    json!({"frame_id": frame_id, "pressed": spec.pressed_name(), "row": row,
                           "click_rect": rect, "point": point}),
                );
                continue;
            }
            click_rect = Some(rect);
        }
        decisions.push(SelectFrameDecision {
            frame_id: frame_id.to_owned(),
            pressed,
            default: default.kind,
        });
    }

    // The kernel's offline entry over the forward screens, once the end of the list is declared
    // (#463 S2-9b: or the scan goes straight to the wanted item, and decides where it stops early):
    // the content steps in the widest window, then the pressed name and, with an end, a name no
    // screen shows.
    if (scan.end.is_some() || scan.stop == Some(ScanStop::Wanted)) && rejections.is_empty() {
        let loaded = forward
            .iter()
            .map(|screen| loaded_of(&cache, &screen.frame))
            .collect::<LabResult<Vec<_>>>()?;
        let (_, list_length) = along(fold.axis, scan.list);
        let (_, row_length) = along(fold.axis, rows.row);
        let wide = widest(&state, unsigned(list_length - row_length).max(1));
        let measuring = if wide == state {
            None
        } else {
            Some(admitted_probe(
                recording,
                &with_scan(&probe.step, &current, &wide),
                probe.marks.clone(),
                &probe.crops,
                &cache,
                ocr,
            )?)
        };
        let measured_on = measuring.as_ref().unwrap_or(&checked);
        let measured = offline_entry(
            &measured_on.prepared,
            &measured_on.operation,
            &loaded,
            pressed_document.as_ref(),
            instant,
        )?;
        let steps = match &measured {
            Ok(decided) => content_steps(&screens, &offline_of(decided)?.offsets),
            Err(failure) => {
                reject(
                    &mut rejections,
                    SELF_TEST_ERROR,
                    json!({"frame_id": forward.first().map(|screen| screen.frame.frame_id.clone()),
                           "failure": failure}),
                );
                Vec::new()
            }
        };
        let window = (
            state.number(ScanNumber::AdvanceMinContentStepPx),
            state.number(ScanNumber::AdvanceMaxContentStepPx),
        );
        for step in &steps {
            let inside = u64::try_from(step.content_step_px)
                .is_ok_and(|content| (window.0..=window.1).contains(&content));
            if !inside {
                reject(
                    &mut rejections,
                    STEP_OUTSIDE_WINDOW,
                    json!({"swipe": step.step, "content_step_px": step.content_step_px,
                           "window": [window.0, window.1]}),
                );
            }
        }
        if rejections.is_empty() {
            let pressed = match measuring {
                None => measured,
                Some(_) => offline_entry(
                    &checked.prepared,
                    &checked.operation,
                    &loaded,
                    pressed_document.as_ref(),
                    instant,
                )?,
            };
            match pressed {
                Err(failure) => reject(
                    &mut rejections,
                    SELF_TEST_ERROR,
                    json!({"frame_id": forward.first().map(|screen| screen.frame.frame_id.clone()),
                           "failure": failure}),
                ),
                Ok(decided) => {
                    let run = offline_of(&decided)?;
                    judge_pressed(&mut rejections, &run, spec.pressed_name(), scan);
                    for (screen, offset) in screens
                        .iter_mut()
                        .filter(|screen| screen.role != ScanScreenRole::Back)
                        .zip(&run.offsets)
                    {
                        screen.content_offset_px = Some(*offset);
                    }
                    // #463 S2-9b: without an end mark a name no screen shows is not judged here.
                    let absent_run = if without_end(scan) {
                        None
                    } else {
                        absent_name(&spec, &screens)
                    };
                    let absent = match absent_run {
                        None => None,
                        Some(name) => {
                            let document =
                                select_policy_document(&checked.plan, first, &spec, &name);
                            match offline_entry(
                                &checked.prepared,
                                &checked.operation,
                                &loaded,
                                Some(&document),
                                instant,
                            )? {
                                Err(failure) => {
                                    reject(
                                        &mut rejections,
                                        SELF_TEST_ERROR,
                                        json!({"absent": name, "failure": failure}),
                                    );
                                    None
                                }
                                Ok(decided) => {
                                    let absent = offline_of(&decided)?;
                                    if absent.coverage != ScanCoverageState::Complete
                                        || absent.decision != ScanDecision::Empty
                                    {
                                        reject(
                                            &mut rejections,
                                            ABSENT_NOT_EMPTY,
                                            json!({"name": name, "coverage": absent.coverage,
                                                   "decision": absent.decision}),
                                        );
                                    }
                                    Some(ScanAbsentRun {
                                        name,
                                        coverage: absent.coverage,
                                        decision: absent.decision,
                                    })
                                }
                            }
                        }
                    };
                    state.outcome = Some(ScanSelfTestOutcome {
                        coverage: run.coverage,
                        positions: run.positions,
                        pressed: run.decision,
                        steps,
                        absent,
                    });
                }
            }
        }
    }
    if rejections.is_empty() && click_rect.is_none() {
        return Err(invalid(
            "validation_failed",
            format!("step {index} has no live primary frame for its select"),
        ));
    }
    state.screens = screens;
    Ok(SelectChecked {
        select: StepSelect {
            self_test: SelectSelfTest {
                status: SelfTestStatus::Passed,
                reason: None,
                frames: decisions,
            },
            scan: Some(state),
            ..current
        },
        click_rect,
        rejections,
    })
}

/// #463 S2-9b: the refusal `scan_min_step_too_large` of a scan without an end mark whose smallest
/// content step `m` exceeds `2t + 1`, `t` its position tolerance (both of `state`'s numbers). The
/// kernel, where the list stops moving, meets the failure of a scan without an end
/// (`selection_scan_early_stop_unreached`) when the last displacement is at most `t` or at least
/// `m − t`, and the alignment's failure in between: with `m ≤ 2t + 1` there is no in between.
/// `None` for every other scan.
fn stop_window_rejection(scan: &SelectScanSpec, state: &SelectScanState) -> Option<MarkRejection> {
    if !without_end(scan) {
        return None;
    }
    let smallest = state.number(ScanNumber::AdvanceMinContentStepPx);
    let tolerance = state.number(ScanNumber::PositionTolerancePx);
    let most = tolerance.saturating_mul(2).saturating_add(1);
    (smallest > most).then(|| {
        MarkRejection::new(
            SELECT_REJECTION_ID,
            MIN_STEP_TOO_LARGE,
            json!({"min_content_step_px": smallest, "position_tolerance_px": tolerance,
                   "most": most}),
        )
    })
}

/// The refusals of the offline run for the pressed name `pressed` of the scan `scan`: its coverage
/// not complete (#463 S2-9b: not the one [`decided_coverage`] gives the kind), positions it could
/// not read, a decision other than that name.
fn judge_pressed(
    rejections: &mut Vec<MarkRejection>,
    run: &Offline,
    pressed: &str,
    scan: &SelectScanSpec,
) {
    if !decided_coverage(scan).contains(&run.coverage) {
        rejections.push(MarkRejection::new(
            SELECT_REJECTION_ID,
            COVERAGE_INCOMPLETE,
            json!({"coverage": run.coverage}),
        ));
    }
    if !run.unknown.is_empty() {
        rejections.push(MarkRejection::new(
            SELECT_REJECTION_ID,
            POSITION_UNREAD,
            json!({"positions": run.unknown}),
        ));
    }
    let chose_pressed =
        matches!(&run.decision, ScanDecision::Selected { name, .. } if name == pressed);
    if !chose_pressed {
        rejections.push(MarkRejection::new(
            SELECT_REJECTION_ID,
            DECISION_MISMATCH,
            json!({"pressed": pressed, "decision": run.decision}),
        ));
    }
}

// ---- The self-checks of `record stop` -------------------------------------------------------

/// `record_step_self_mismatch` of the scrolling select of `step` on the frame `frame_id`, with why
/// (`reason`) and the members of `detail`.
fn mismatch(step: &PlanStep, frame_id: Option<&str>, reason: &str, detail: Value) -> LabError {
    let mut details = json!({
        "step": step.number,
        "record_index": step.step.index,
        "frame_id": frame_id,
        "reason": reason
    });
    if let (Some(object), Some(extra)) = (details.as_object_mut(), detail.as_object()) {
        for (key, value) in extra {
            object.insert(key.clone(), value.clone());
        }
    }
    with_details(
        blocked(
            "record_step_self_mismatch",
            format!(
                "the scrolling select of step {} does not choose over its recorded screens as \
                 recorded: {reason}",
                step.step.index
            ),
        ),
        details,
    )
}

/// #463 S2-9a, self-check item 8 for a scrolling select: on every live frame of the step the
/// kernel's dry run of the package decides as for a fixed-slot select (its rows actionable in the
/// list each read, the pressed name selecting the row the press hit on the primary frame); on every
/// screen of the demonstration the step's page is the one page of the package that passes (another
/// page passing there is the kernel's `select_page_mismatch`, `self_test_error`) and every row reads
/// a known name; and the kernel's offline entry over the forward screens decides the pressed name
/// over complete coverage with every position read, and a name no screen shows empty over complete
/// coverage. Each failure is `record_step_self_mismatch` with its `reason`, met by `failures`; the
/// step's entry (`item`) gains `frames` and `scan`. #463 S2-9b, straight to the wanted item: the
/// pressed name's run stops early ([`decided_coverage`]); without an end mark no name no screen
/// shows is run, and the wanted default is a name read on the first screen or one after a forward
/// swipe (`scan_default_not_shown`): a run with the package's default values could only find it
/// beyond the recorded screens, where no offline entry can follow it.
pub(crate) fn scan_select_checks(
    prepared: &PreparedContainedTask,
    plan: &Plan,
    (step, select, state): (&PlanStep, &StepSelect, &SelectScanState),
    cache: &FrameCache,
    failures: &mut Failures,
    item: &mut Value,
) -> LabResult<()> {
    let spec = &select.declaration;
    let (_, scan) = rows_and_scan(spec)?;
    let operation = plan.operation_id(step);
    let instant = plan.generated_at_unix_ms;
    let on = |frames: &[String]| Locus {
        page: step.page.clone(),
        ..Locus::on_step(step).with_frames(frames)
    };
    let pressed_document = (spec.pressed_name() != spec.wanted.default)
        .then(|| select_policy_document(plan, step, spec, spec.pressed_name()));
    let executed = step
        .step
        .click
        .as_ref()
        .filter(|click| click.executed())
        .and_then(|click| click.execution.as_ref())
        .map(|execution| execution.point);
    let mut decisions = Vec::new();
    for frame in step.live_frames() {
        let ids = std::slice::from_ref(&frame.frame_id);
        let frame_id = Some(frame.frame_id.as_str());
        let loaded = loaded_of(cache, frame)?;
        let default = match decide(prepared, &operation, &loaded.frame, None, instant)? {
            Ok(decided) => decided,
            Err(failure) => {
                failures.fail(
                    on(ids),
                    mismatch(step, frame_id, SELF_TEST_ERROR, json!({"failure": failure})),
                )?;
                continue;
            }
        };
        let unread = unread_rows(&default);
        if !unread.is_empty() {
            for (row, cause) in unread {
                failures.fail(
                    on(ids).with_target(&target_id(plan, step, NAME)),
                    mismatch(
                        step,
                        frame_id,
                        NAME_UNREAD,
                        json!({"row": row, "cause": cause}),
                    ),
                )?;
            }
            continue;
        }
        let pressed = match &pressed_document {
            None => default.kind,
            Some(document) => {
                match decide(prepared, &operation, &loaded.frame, Some(document), instant)? {
                    Ok(decided) => decided.kind,
                    Err(failure) => {
                        failures.fail(
                            on(ids),
                            mismatch(step, frame_id, SELF_TEST_ERROR, json!({"failure": failure})),
                        )?;
                        continue;
                    }
                }
            }
        };
        if frame.role == "primary" {
            let Some(row) = pressed.slot() else {
                failures.fail(
                    on(ids),
                    mismatch(
                        step,
                        frame_id,
                        PRESSED_ABSENT,
                        json!({"pressed": spec.pressed_name(), "decision": pressed}),
                    ),
                )?;
                continue;
            };
            if let Some(point) = executed
                && let Some(rect) = default.clicks.get(&row).copied()
                && !holds(point, rect)
            {
                failures.fail(
                    on(ids),
                    mismatch(
                        step,
                        frame_id,
                        PRESS_MISMATCH,
                        json!({"row": row, "click_rect": rect, "point": point}),
                    ),
                )?;
                continue;
            }
        }
        decisions.push(json!(SelectFrameDecision {
            frame_id: frame.frame_id.clone(),
            pressed,
            default: default.kind,
        }));
    }
    item["frames"] = json!(decisions);

    let mut screens = Vec::with_capacity(state.screens.len());
    // #463 S2-9b: the names read on the first screen and those after a forward swipe.
    let mut shown = BTreeSet::new();
    for screen in &state.screens {
        let ids = std::slice::from_ref(&screen.frame.frame_id);
        let frame_id = Some(screen.frame.frame_id.as_str());
        let loaded = loaded_of(cache, &screen.frame)?;
        match decide(prepared, &operation, &loaded.frame, None, instant)? {
            Err(failure) => {
                failures.fail(
                    on(ids),
                    mismatch(step, frame_id, SELF_TEST_ERROR, json!({"failure": failure})),
                )?;
            }
            Ok(decided) => {
                for (row, cause) in unread_rows(&decided) {
                    failures.fail(
                        on(ids).with_target(&target_id(plan, step, NAME)),
                        mismatch(
                            step,
                            frame_id,
                            NAME_UNREAD,
                            json!({"row": row, "cause": cause}),
                        ),
                    )?;
                }
                let names = names_read(&decided, state.axis);
                if screen.role != ScanScreenRole::Back {
                    shown.extend(names.iter().cloned());
                }
                screens.push(json!({
                    "frame_id": screen.frame.frame_id,
                    "step": screen.step,
                    "role": screen.role,
                    "names": names
                }));
            }
        }
    }

    let forward = forward_screens(&state.screens);
    let ids = forward
        .iter()
        .map(|screen| screen.frame.frame_id.clone())
        .collect::<Vec<_>>();
    let first_id = ids.first().map(String::as_str);
    let loaded = forward
        .iter()
        .map(|screen| loaded_of(cache, &screen.frame))
        .collect::<LabResult<Vec<_>>>()?;
    let mut result = json!({"screens": screens});
    match offline_entry(
        prepared,
        &operation,
        &loaded,
        pressed_document.as_ref(),
        instant,
    )? {
        Err(failure) => failures.fail(
            on(ids.as_slice()),
            mismatch(step, first_id, SELF_TEST_ERROR, json!({"failure": failure})),
        )?,
        Ok(decided) => {
            let run = offline_of(&decided)?;
            let mut refusals = Vec::new();
            judge_pressed(&mut refusals, &run, spec.pressed_name(), scan);
            for refusal in refusals {
                failures.fail(
                    on(ids.as_slice()),
                    mismatch(step, first_id, &refusal.reason, refusal.detail),
                )?;
            }
            result["coverage"] = json!(run.coverage);
            result["positions"] = json!(run.positions);
            result["pressed"] = json!(run.decision);
            result["steps"] = json!(content_steps(&state.screens, &run.offsets));
        }
    }
    // #463 S2-9b: without an end mark a name no screen shows is not judged offline.
    if !without_end(scan)
        && let Some(name) = absent_name(spec, &state.screens)
    {
        let document = select_policy_document(plan, step, spec, &name);
        match offline_entry(prepared, &operation, &loaded, Some(&document), instant)? {
            Err(failure) => failures.fail(
                on(ids.as_slice()),
                mismatch(
                    step,
                    first_id,
                    SELF_TEST_ERROR,
                    json!({"absent": name, "failure": failure}),
                ),
            )?,
            Ok(decided) => {
                let absent = offline_of(&decided)?;
                if absent.coverage != ScanCoverageState::Complete
                    || absent.decision != ScanDecision::Empty
                {
                    failures.fail(
                        on(ids.as_slice()),
                        mismatch(
                            step,
                            first_id,
                            ABSENT_NOT_EMPTY,
                            json!({"name": name, "coverage": absent.coverage,
                                   "decision": absent.decision}),
                        ),
                    )?;
                }
                result["absent"] = json!(ScanAbsentRun {
                    name,
                    coverage: absent.coverage,
                    decision: absent.decision,
                });
            }
        }
    }
    // #463 S2-9b: without an end mark a run with the package's default values finds its item only
    // on the screens it reaches, which the demonstration must show.
    if without_end(scan) && !shown.contains(spec.wanted.default.as_str()) {
        failures.fail(
            on(ids.as_slice()),
            mismatch(
                step,
                first_id,
                DEFAULT_NOT_SHOWN,
                json!({"default": spec.wanted.default}),
            ),
        )?;
    }
    item["scan"] = result;
    Ok(())
}

/// `record_artifact_admission_failed` at the stage `first_decision`.
fn first_decision_failed(message: String, mut details: Value) -> LabError {
    if let Some(object) = details.as_object_mut() {
        object.insert("stage".to_owned(), json!("first_decision"));
    }
    with_details(
        blocked("record_artifact_admission_failed", message),
        details,
    )
}

/// #463 S2-9a, self-check item 3 for a scrolling select at step 1: the kernel's offline entry over
/// its forward screens with the package's variables at their defaults (the sealed policy) must
/// cover the list completely (#463 S2-9b: straight to the wanted item, stop early there,
/// [`decided_coverage`]) and read every position; a decision that selects is
/// `{status: "would_select", operation_label, name, position, frames}` (its return, confirmation
/// and tap follow on frames no recording holds), an empty one the run's normal end,
/// `{status: "would_end_empty", operation_label, outcome_key, frames}`; anything else is
/// `record_artifact_admission_failed`, met by `failures` (step 1, its forward screens).
pub(crate) fn scan_first_decision(
    prepared: &PreparedContainedTask,
    plan: &Plan,
    (first, select, state): (&PlanStep, &StepSelect, &SelectScanState),
    cache: &FrameCache,
    failures: &mut Failures,
) -> LabResult<FirstDecision> {
    let operation = plan.operation_id(first);
    let forward = forward_screens(&state.screens);
    let ids = forward
        .iter()
        .map(|screen| screen.frame.frame_id.clone())
        .collect::<Vec<_>>();
    let loaded = forward
        .iter()
        .map(|screen| loaded_of(cache, &screen.frame))
        .collect::<LabResult<Vec<_>>>()?;
    let mut refuse = |error: LabError| -> LabResult<FirstDecision> {
        failures.fail(Locus::on_step(first).with_frames(&ids), error)?;
        Ok(FirstDecision {
            value: Value::Null,
            warning: None,
        })
    };
    let decided = match offline_entry(
        prepared,
        &operation,
        &loaded,
        None,
        plan.generated_at_unix_ms,
    )? {
        Ok(decided) => decided,
        Err(failure) => {
            return refuse(first_decision_failed(
                "the offline entry of step 1's scrolling select fails over its screens".to_owned(),
                json!({"frames": ids, "failure": failure}),
            ));
        }
    };
    let run = offline_of(&decided)?;
    // #463 S2-9b: a select straight to the wanted item decides where its scan stops early.
    let (_, scan) = rows_and_scan(&select.declaration)?;
    if !decided_coverage(scan).contains(&run.coverage) || !run.unknown.is_empty() {
        return refuse(first_decision_failed(
            "the offline entry of step 1's scrolling select does not read the whole list"
                .to_owned(),
            json!({"frames": ids, "coverage": run.coverage, "unknown": run.unknown}),
        ));
    }
    let value = match &run.decision {
        ScanDecision::Selected { name, position } => json!({
            "status": "would_select",
            "operation_label": operation,
            "name": name,
            "position": position,
            "frames": ids
        }),
        ScanDecision::Empty => json!({
            "status": "would_end_empty",
            "operation_label": operation,
            "outcome_key": on_empty_key(&plan.task_id, &select.declaration),
            "frames": ids
        }),
        other => {
            return refuse(first_decision_failed(
                "the scrolling select of step 1 selects no row over its screens, and the run \
                 would fail there"
                    .to_owned(),
                json!({"frames": ids, "decision": other}),
            ));
        }
    };
    Ok(FirstDecision {
        value,
        warning: None,
    })
}
