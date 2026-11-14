// SPDX-License-Identifier: AGPL-3.0-only

//! The `linear_steps` execution mode (Workflow #336; `contracts/linear-steps.md`).
//!
//! A linear task is its recorded path: operation `k` leaves the page of step `k` and must
//! reach the page of step `k + 1`. Every wait evaluates only the page or pages it waits for,
//! as the detector page ids admission resolved; no page is recognized globally and no run
//! state machine is built. An operation may declare an intermediate state after its input: a
//! recognizable page that must be seen before the next step is trusted, or a time window in
//! which nothing is evaluated before its lower bound. A declared retry only repeats an input
//! the screen shows was swallowed. An operation's effect may instead be the application
//! lifecycle action on the instance's assigned application (R24); the first operation may then
//! start from any screen, with no recognition before it. An operation may be optional
//! (Workflow #339): its page may not appear, and the run then skips it. The input before a run
//! of optional operations awaits their pages together with the page after the run, and the
//! page that passes decides which operation runs next.
//!
//! Workflow #407 LOAD-4 (register §1.262; design `rd-1031/check-speed/model-loading.md` §5): under
//! `execution.loading.mode` `on` each frame of a wait is judged by LOAD-3's one judgement
//! (`loading.rs`): the loading pages are evaluated on the frame its candidates are, and one that
//! holds makes it a transition frame whatever candidate passed. A transition frame is never a
//! candidate, the intermediate page or the step's own page; it pauses the wait (its arrival
//! budget, a skip target's settle and first sighting, an extra round's end) within the attempt's
//! allowance, which the attempt's waits and its retry decision's share, and one after the
//! allowance is spent ends the wait with the loading values. The entry wait has an allowance of
//! its own. Under `off` and `shadow` every decision is the one before LOAD-4.
//!
//! Workflow #463 U1 (`contracts/linear-steps.md`, "A screen the package never recorded"; fix
//! round 2, the coordinator's rulings on board #463): where a step's failed wait ends the run with
//! `page_confirmation_failed` (the last attempt's arrival, the next page after a seen
//! intermediate page or skip target, the continued wait after a late gate, a retry decision that
//! saw neither page), a task's own run outside a restart segment first makes one closing
//! decision: it observes for the step timeout, never inputs and never hands a frame to the shared
//! interrupt pages. Only a screen that stayed one the package never recorded for the whole
//! decision changes the end, to `contained_task_linear_unrecorded_screen` with the same detail
//! and timing; every other end, an error of the decision included, is the one it had.
//!
//! Workflow #463 K1 (`contracts/linear-steps.md`, "Select steps"): an operation's effect may be a
//! select step. Its attempt is the page graph's ([`PreparedContainedTask::select_attempt`]): it
//! decides on the step's frame by the step's policy, confirms the choice on a fresh frame and taps
//! the chosen candidate once, bound to that frame; the step then goes on as a click step does. An
//! empty decision of a step that declares `on_empty` taps nothing and ends the run Completed on
//! the step's page with its key, as the page graph's keyed end finishes a run
//! ([`LinearStepEnd::Empty`]). A select step declares no retry, is not optional and its page is
//! mapped to its `on_empty` key; a task without a select step is admitted and run as before.

use super::loading::{Charge, FrameRow, LoadingAllowance, LoadingFrame, LoadingLimit};
use super::observation_window::StillScreen;
use super::{
    ApplicationEffectSupport, CaptureBackoff, ColorDigest, ContainedTaskBoundaryTiming,
    ContainedTaskError, ContainedTaskGuardOutcome, ContainedTaskOutcome, ContainedTaskRunError,
    ContainedTaskRuntime, ContainedTaskTimingContext, ContainedTaskTrace, DEFAULT_TASK_TIMEOUT_MS,
    ExtraRounds, InputRecheck, MAX_DECLARED_INTERVAL_MS, MAX_STEP_TIMEOUT_MS, MAX_STEPS,
    MAX_TASK_TIMEOUT_MS, PaceClock, PageCandidates, PageObservation, PostAdmissionOcrCollector,
    PreparedContainedTask, RecheckedInput, SampleRetries, StepPacing, TaskControl, TaskOperation,
    TaskProgram, budget_elapsed_since, budget_now, budget_observation_now, invariant_broken,
    is_geometry_change, pace_pause, paced_step, resolve_page_reference, runtime_touch_choice,
    sample_budget_invalid, selection, step_point_draw,
};
use crate::codes::{ExecutionKernelCode, ExecutionKernelLocation};
use crate::{InputFrameContext, RunOperationPolicy};
use actingcommand_contract::TaskSchema;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_contract::{
    ApplicationLifecycleAction, InputAction, InputSamplingEvidence, PHASED_CONTROL_SCHEMA,
    SchedulingEffectCondition, TaskTimingBoundary, TaskTimingCheckPosition, TaskTimingFailure,
    TaskTimingScope, TaskTimingStage, keyed_terminal_holds,
};
use actingcommand_page_detector::PageDetector;
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::{RecognitionEvaluator, RememberedOcrObservations};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

/// The control `execution_mode` of a linear task.
pub(super) const LINEAR_STEPS: &str = "linear_steps";
const LINEAR_INVALID: Code = ExecutionKernelCode::ContainedTaskLinearInvalid.code();
const ENTRY_UNMATCHED: Code = ExecutionKernelCode::ContainedTaskLinearEntryUnmatched.code();
const INTERMEDIATE_UNOBSERVED: Code =
    ExecutionKernelCode::ContainedTaskLinearIntermediateUnobserved.code();
const APPLICATION_UNCONFIRMED: Code =
    ExecutionKernelCode::ContainedTaskLinearApplicationUnconfirmed.code();
const PAGE_CONFIRMATION_FAILED: Code = ExecutionKernelCode::PageConfirmationFailed.code();
const UNRECORDED_SCREEN: Code = ExecutionKernelCode::ContainedTaskLinearUnrecordedScreen.code();
const UNRECOGNIZED_PAGE: &str = "<unrecognized>";
/// The `from` / `entry_page` of an application entry: any screen, never a detector page.
pub(super) const ANY_PAGE: &str = "any";

/// An operation's declared intermediate state, as the package writes it (Workflow #336 R13).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum TaskTransition {
    /// A recognizable page that must be seen after the input, before the next step's page.
    Page {
        page_id: String,
        #[serde(default)]
        timeout_ms: Option<u64>,
        #[serde(default)]
        interval_ms: Option<u64>,
    },
    /// Nothing is evaluated before `min_ms`; the next step's page may take `max_ms` plus its
    /// own arrival budget.
    Window { min_ms: u64, max_ms: u64 },
}

/// An operation whose page may not appear, as the package writes it (Workflow #339): the whole
/// operation is then skipped. After the run of optional operations it belongs to, the next
/// page is watched for `settle_ms` for a late optional page.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TaskOptional {
    settle_ms: u64,
}

/// The admitted path; every page is the detector page id admission resolved.
pub(super) struct LinearPlan {
    target: String,
    steps: Vec<LinearStep>,
    /// The runs of optional operations (Workflow #339), in path order.
    runs: Vec<OptionalRun>,
}

/// One operation of the path: its own page, the next step's page, its effect and its
/// intermediate state.
struct LinearStep {
    from: LinearFrom,
    to: String,
    effect: LinearEffect,
    transition: Option<LinearTransition>,
    /// Workflow #339: the settle of an optional operation; `None` for a required one.
    optional: Option<Duration>,
    /// Workflow #339: the run of optional operations that this step's input may reach, for the
    /// operation before a run and for its members; `None` when the input reaches `to` only.
    run: Option<usize>,
    /// Workflow #407 LOAD-4: the pages the step's confirmation adds as loading pages
    /// (`loading.add`, then its wait page), as the detector names them.
    loading_added: Vec<String>,
}

/// A maximal run of consecutive optional operations (Workflow #339 §3.1). The operation before
/// it is required, since the first operation is.
struct OptionalRun {
    /// The run's operation indexes, in path order.
    members: Vec<usize>,
    /// The skip target: the destination of the run's last operation.
    skip_page: String,
    /// The operation after the run; `None` when the run ends the path.
    next: Option<usize>,
}

/// The pages an input may reach (Workflow #339 §3.1), in evaluation order: the next step's
/// page; or, for the operation before a run and its members, the run's optional pages not run
/// yet in path order, then the run's skip target. An optional page that passes on the same frame
/// as the skip target wins.
struct LinearCandidates<'p> {
    pages: Vec<&'p str>,
    /// The largest settle of the optional pages among `pages`; zero without one.
    settle: Duration,
}

impl LinearCandidates<'_> {
    /// Whether `page` is the skip target of candidates that still watch for an optional page.
    fn settles_on(&self, page: &str) -> bool {
        self.pages.len() > 1 && !self.settle.is_zero() && self.pages.last() == Some(&page)
    }

    /// The detail an arrival failure appends (§3.6): the awaited pages, and whether the skip
    /// target was seen. Nothing for one page, so the detail of a path without optional
    /// operations is unchanged.
    fn awaited(&self, skip_target_seen: bool) -> String {
        if self.pages.len() < 2 {
            return String::new();
        }
        format!(
            " awaited={}{}",
            self.pages.join(","),
            if skip_target_seen {
                " skip_target_seen=true"
            } else {
                ""
            }
        )
    }
}

impl LinearPlan {
    /// Account for every page in each observation's largest legal candidate set. Guards
    /// have the separate StepStarted budget checked by the shared preparation owner.
    pub(super) fn validate_sampling_budget(
        &self,
        program: &TaskProgram,
        control: &TaskControl,
        evaluator: &RecognitionEvaluator,
        detector: &PageDetector,
    ) -> Result<(), ContainedTaskError> {
        if evaluator.pack().target_consensus.is_empty() {
            return Ok(());
        }
        // Workflow #386 SR-1 (scheduling model §2.4 item 1): a wait whose sampling cannot fit
        // its budget is `recognition_sample_budget_invalid`, naming the operation whose wait it
        // is (none for the entry).
        let check = |pages: &[&str], limit_ms: u64, phase: &str, operation: Option<&str>| {
            let required_ms =
                selection::page_recognition_budget_ms(evaluator, detector, Some(pages))?;
            let limit_ms = limit_ms.min(control.task_timeout().milliseconds);
            if required_ms > limit_ms {
                return Err(sample_budget_invalid(
                    ContainedTaskError::with_detail(
                        ExecutionKernelCode::RecognitionSampleBudgetInvalid.code(),
                        format!(
                            "phase={phase} pages={} requires {required_ms}ms across page calls and waits; limit={limit_ms}ms",
                            pages.join(",")
                        ),
                    ),
                    required_ms,
                    limit_ms,
                    operation,
                ));
            }
            Ok(())
        };
        let handled = BTreeSet::new();
        for (index, (step, operation)) in self.steps.iter().zip(&program.operations).enumerate() {
            if index == 0
                && let LinearFrom::Page(page) = &step.from
            {
                check(
                    &[page.as_str()],
                    control.step_timeout().milliseconds,
                    "entry",
                    None,
                )?;
            }
            let candidates = self.candidates(index, &handled);
            let arrival_ms = operation.effective_timing(control).timeout.milliseconds;
            let arrival_ms = match &step.transition {
                Some(LinearTransition::Window { min, max }) => {
                    let delay =
                        Duration::from_millis(operation.post_delay_ms.unwrap_or(0)).max(*min);
                    (max.saturating_sub(delay) + Duration::from_millis(arrival_ms)).as_millis()
                        as u64
                }
                _ => arrival_ms,
            };
            let operation_id = Some(operation.id.as_str());
            check(&candidates.pages, arrival_ms, "arrival", operation_id)?;
            if let Some(LinearTransition::Page { page, timeout, .. }) = &step.transition {
                check(
                    &[page.as_str()],
                    timeout.as_millis() as u64,
                    "transition",
                    operation_id,
                )?;
            }
            if let LinearFrom::Page(page) = &step.from
                && matches!(step.effect, LinearEffect::Click)
                && operation
                    .retry_policy(
                        program.defaults,
                        control.timeout_ms.unwrap_or(DEFAULT_TASK_TIMEOUT_MS),
                    )?
                    .is_some_and(|policy| policy.max_attempts() > 1)
            {
                if candidates.pages.len() > 1
                    && matches!(step.transition, Some(LinearTransition::Window { .. }))
                {
                    check(
                        &candidates.pages,
                        operation.effective_timing(control).timeout.milliseconds,
                        "retry_arrival",
                        operation_id,
                    )?;
                }
                let mut retry_pages = match &step.transition {
                    Some(LinearTransition::Page { page, .. }) => vec![page.as_str()],
                    _ => candidates.pages,
                };
                retry_pages.push(page.as_str());
                check(
                    &retry_pages,
                    control.step_timeout().milliseconds,
                    "retry",
                    operation_id,
                )?;
            }
        }
        Ok(())
    }

    /// Workflow #384 D2-S2 (model §3.2 qualification 4, §3.4): the segments of the path, as the
    /// operation index ranges a requested key set may leave out. A path without optional
    /// operations whose first step leaves a page, which ends on that page and returns to it on
    /// the way has one segment per return: each leaves the entry page, reaches its pages and
    /// comes back. Every operation of such a path is in one segment. Any other path is not
    /// segmented and always runs whole.
    fn reading_segments(&self) -> Vec<std::ops::Range<usize>> {
        let Some(LinearFrom::Page(entry)) = self.steps.first().map(|step| &step.from) else {
            return Vec::new();
        };
        if !self.runs.is_empty() || self.target != *entry {
            return Vec::new();
        }
        let mut segments = Vec::new();
        let mut start = 0;
        for (index, step) in self.steps.iter().enumerate() {
            if step.to == *entry {
                segments.push(start..index + 1);
                start = index + 1;
            }
        }
        segments
    }

    /// Workflow #384 D2-S2: the segments that reach no page `reads_on` accepts, as the first
    /// operation of each mapped to the operation after it. The entry page, where every segment
    /// starts and ends, is no segment's own page.
    pub(super) fn segments_without_reads(
        &self,
        reads_on: impl Fn(&str) -> bool,
    ) -> std::collections::BTreeMap<usize, usize> {
        let Some(LinearFrom::Page(entry)) = self.steps.first().map(|step| &step.from) else {
            return std::collections::BTreeMap::new();
        };
        self.reading_segments()
            .into_iter()
            .filter(|segment| {
                !self.steps[segment.clone()]
                    .iter()
                    .any(|step| step.to != *entry && reads_on(step.to.as_str()))
            })
            .map(|segment| (segment.start, segment.end))
            .collect()
    }

    /// The candidates of operation `index` once the members `handled` of its run have run.
    fn candidates(&self, index: usize, handled: &BTreeSet<usize>) -> LinearCandidates<'_> {
        let step = &self.steps[index];
        let Some(run) = step.run.map(|run| &self.runs[run]) else {
            return LinearCandidates {
                pages: vec![step.to.as_str()],
                settle: Duration::ZERO,
            };
        };
        let mut pages = Vec::with_capacity(run.members.len() + 1);
        let mut settle = Duration::ZERO;
        for member in run
            .members
            .iter()
            .filter(|member| **member != index && !handled.contains(*member))
        {
            let member = &self.steps[*member];
            pages.push(member.from.label());
            settle = settle.max(member.optional.unwrap_or_default());
        }
        pages.push(run.skip_page.as_str());
        LinearCandidates { pages, settle }
    }
}

/// The page a step leaves: a detector page id, or the application entry (`from:"any"`), which
/// recognizes nothing before its effect.
#[derive(PartialEq, Eq)]
enum LinearFrom {
    Any,
    Page(String),
}

impl LinearFrom {
    /// `StepStarted.from_page`: the detector page id, or `<unrecognized>` for the application
    /// entry, the literal the page-graph path writes for an unrecognized initial frame.
    fn label(&self) -> &str {
        match self {
            Self::Any => UNRECOGNIZED_PAGE,
            Self::Page(page) => page,
        }
    }
}

/// A step's one effect: a click, an application lifecycle action (Workflow #336 R24), or
/// (Workflow #463 K1) a select step's one tap on the candidate its attempt chose.
#[derive(Clone, Copy)]
enum LinearEffect {
    Click,
    Application(ApplicationLifecycleAction),
    Select,
}

/// How one operation ended without a failure.
enum LinearStepEnd {
    /// A candidate passed on this frame: the next step's page, or a page of a run of optional
    /// operations. Boxed: it holds the frame, and inline it would be the one large variant (clippy
    /// `large_enum_variant`).
    Reached(Box<PageObservation>),
    /// Workflow #463 K1: a select step that declares `on_empty` ended empty.
    Empty(LinearEmptyEnd),
}

/// Workflow #463 K1 (`contracts/linear-steps.md`, "Select steps"): a select step that declares
/// `on_empty` decided `empty` with nothing unknown. No input was made; the step finished on
/// `page_label`, its page as recognized on its last frame, and the run ends there, Completed, with
/// `outcome_key`.
struct LinearEmptyEnd {
    page_label: String,
    outcome_key: String,
}

/// The one input of a step whose decision held: its action and sampling, the guard outcome it
/// was decided with and the input context of the frame that binds it.
struct LinearInput {
    action: InputAction,
    sampling: Option<InputSamplingEvidence>,
    guard: ContainedTaskGuardOutcome,
    input_context: Option<InputFrameContext>,
}

const fn application_action_name(action: ApplicationLifecycleAction) -> &'static str {
    match action {
        ApplicationLifecycleAction::Launch => "launch",
        ApplicationLifecycleAction::Restart => "restart",
        ApplicationLifecycleAction::Stop => "stop",
    }
}

enum LinearTransition {
    Page {
        page: String,
        timeout: Duration,
        interval: Duration,
    },
    Window {
        min: Duration,
        max: Duration,
    },
}

impl LinearStep {
    fn transition_detail(&self) -> String {
        match &self.transition {
            None => "transition=none".to_owned(),
            Some(LinearTransition::Page { .. }) => "transition=page".to_owned(),
            Some(LinearTransition::Window { min, max }) => format!(
                "transition=window min_ms={} max_ms={}",
                min.as_millis(),
                max.as_millis()
            ),
        }
    }
}

/// The run budgets a linear run reads.
#[derive(Clone, Copy)]
pub(super) struct LinearRun {
    pub(super) step_timeout: Duration,
    pub(super) capture_interval: Duration,
    /// The run's timing context; #385 P-k: it carries the run's budget clock, from the run's
    /// entry and then from the step in progress.
    pub(super) timing: ContainedTaskTimingContext,
    /// Workflow #386 SR-1: the pacing of the run's entry, then of the step in progress.
    pub(super) pacing: StepPacing,
    /// STORE-FG-1 (design §2.3, §2.7): whether the run's declared store steps tap in the store:
    /// the package's own run, not a layer or a declared operation's run.
    pub(super) store_routing: bool,
    /// Workflow #463 U1: the run is a task's own run (a scheduled or a direct run of the
    /// package, or a Lab run of it), not a layer's, a recovery entry's or a run the host starts
    /// itself (a startup package, a ladder rung: fix round 2,
    /// [`ContainedTaskRuntime::host_started_run`]); only such a run ends on a screen its package
    /// never recorded with `contained_task_linear_unrecorded_screen`.
    pub(super) own_task_run: bool,
    /// Workflow #463 U1 (R25-1, `contracts/policy-suspension.md`, "Which failures accumulate"):
    /// the run made a `launch` or `restart` effect and no step after it arrived on the main
    /// interface yet; false at the run's start.
    pub(super) restart_segment: bool,
}

/// What a wait is for: its sleep boundary and the stage of a task timeout inside it.
#[derive(Clone, Copy)]
enum LinearWaitPurpose {
    /// The first step's page, before any input.
    Entry,
    /// The intermediate page or the next step's page, after an input.
    AfterInput,
    /// The retry decision between the gate and this step's own page.
    RetryDecision,
}

impl LinearWaitPurpose {
    const fn boundary(self) -> TaskTimingBoundary {
        match self {
            Self::Entry | Self::RetryDecision => TaskTimingBoundary::PageRecognitionWait,
            Self::AfterInput => TaskTimingBoundary::PostconditionWait,
        }
    }

    /// Workflow #386 SR-1: the observation a sampling timeout of this wait names (`stage`).
    const fn observation(self) -> ExecutionKernelLocation {
        match self {
            Self::Entry => ExecutionKernelLocation::ObservationEntry,
            Self::AfterInput | Self::RetryDecision => {
                ExecutionKernelLocation::ObservationAfterInput
            }
        }
    }

    const fn timeout_stage(self) -> TaskTimingStage {
        match self {
            Self::Entry => TaskTimingStage::EntryRecognition,
            Self::AfterInput => TaskTimingStage::Postcondition,
            Self::RetryDecision => TaskTimingStage::PageRecognition,
        }
    }
}

/// Why one attempt did not reach the next step's page.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LinearMissKind {
    /// The declared intermediate page was not seen.
    Intermediate,
    /// The next step's page did not pass, and no intermediate page was seen.
    Arrival,
    /// The intermediate page was seen, then the next step's page did not pass.
    AfterIntermediate,
}

struct LinearMiss {
    kind: LinearMissKind,
    elapsed: Duration,
    limit: Duration,
    /// Workflow #339: the skip target of a run passed during the wait; the input took effect,
    /// so the attempt has no retry decision.
    skip_target_seen: bool,
    /// Workflow #407 LOAD-4: the wait ended on a transition frame after the attempt's loading
    /// allowance was spent.
    loading_limit: Option<LoadingLimit>,
}

/// The budgets of the wait for the next step's page.
#[derive(Clone, Copy)]
struct LinearArrival {
    budget: Duration,
    interval: Duration,
    post_input_delay: Duration,
}

type LinearAttemptResult<E> = Result<Result<PageObservation, LinearMiss>, ContainedTaskRunError<E>>;

/// One wait's end: the frame on which a candidate passed, or none once its budget is spent, with
/// the loading values when it ended on its allowance (Workflow #407 LOAD-4).
type LinearWaitResult<E> =
    Result<Result<PageObservation, Option<LoadingLimit>>, ContainedTaskRunError<E>>;

fn postcondition_timing(elapsed: Duration, limit: Duration) -> Option<TaskTimingFailure> {
    Some(TaskTimingFailure {
        scope: TaskTimingScope::Postcondition,
        stage: TaskTimingStage::Postcondition,
        elapsed_ms: elapsed.as_millis() as u64,
        limit_ms: limit.as_millis() as u64,
        required_delay_ms: None,
    })
}

/// The detail of a step whose next page did not pass: `page_confirmation_failed`'s, which
/// `contained_task_linear_unrecorded_screen` keeps (Workflow #463 U1). `awaited` is the suffix of
/// `LinearCandidates::awaited` (Workflow #339), empty for one page.
fn confirmation_detail(
    operation: &TaskOperation,
    step: &LinearStep,
    attempt: u32,
    intermediate_seen: bool,
    awaited: &str,
) -> String {
    format!(
        "operation={} attempts={attempt} after_page={UNRECOGNIZED_PAGE} hit_error_page=false {}{}{awaited}",
        operation.id,
        step.transition_detail(),
        if intermediate_seen {
            " intermediate_seen=true"
        } else {
            ""
        }
    )
}

fn page_confirmation_failed(
    operation: &TaskOperation,
    step: &LinearStep,
    attempt: u32,
    intermediate_seen: bool,
    awaited: &str,
    elapsed: Duration,
    limit: Duration,
) -> ContainedTaskError {
    ContainedTaskError::with_detail(
        PAGE_CONFIRMATION_FAILED,
        confirmation_detail(operation, step, attempt, intermediate_seen, awaited),
    )
    .with_timing(postcondition_timing(elapsed, limit))
}

/// Workflow #463 U1: the closing decision after attempt `attempt` found a screen the package never
/// recorded on `pages` (the pages the failed wait awaited, then the step's own page). The detail
/// and the timing are those `page_confirmation_failed` gives the same end; the values name the
/// operation, the pages and the attempts, and the host adds the frame of the run's `Failure` pin
/// (`artifact_id`).
fn unrecorded_screen(
    operation: &TaskOperation,
    step: &LinearStep,
    (attempt, intermediate_seen): (u32, bool),
    awaited: &str,
    pages: &[&str],
    elapsed: Duration,
    limit: Duration,
) -> ContainedTaskError {
    let values = Outcome::new(UNRECORDED_SCREEN)
        .pack_operation_id(operation.id.as_str())
        .page_ids(pages.iter().copied())
        .attempts(attempt);
    ContainedTaskError::with_detail(
        UNRECORDED_SCREEN,
        confirmation_detail(operation, step, attempt, intermediate_seen, awaited),
    )
    .with_values(values)
    .with_timing(postcondition_timing(elapsed, limit))
}

/// An application step whose next page did not pass (§5.3 h): never retried. Workflow #407
/// LOAD-4: a wait that ended on the attempt's loading allowance adds its loading values.
fn application_unconfirmed(
    operation: &TaskOperation,
    step: &LinearStep,
    action: ApplicationLifecycleAction,
    miss: LinearMiss,
    awaited: &str,
) -> ContainedTaskError {
    ContainedTaskError::with_detail(
        APPLICATION_UNCONFIRMED,
        format!(
            "operation={} application={} attempts=1 {} intermediate_seen={}{awaited}",
            operation.id,
            application_action_name(action),
            step.transition_detail(),
            miss.kind == LinearMissKind::AfterIntermediate
        ),
    )
    .with_timing(postcondition_timing(miss.elapsed, miss.limit))
    .with_loading_limit(miss.loading_limit)
}

/// Workflow #336 R25 (ruling 5961093808): whether `page` is the main interface of a linear
/// package. Its canonical anchor is `home`, or `step_<digits>_home`, the page Lab records for
/// `--page home`. Only linear packages use this predicate; the page-graph home entry stays the
/// literal `home`.
pub fn linear_main_interface(game: &str, page: &str) -> bool {
    let anchor = crate::canonical_page_anchor(game, page);
    anchor == "home"
        || anchor
            .strip_prefix("step_")
            .and_then(|rest| rest.strip_suffix("_home"))
            .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

impl TaskProgram {
    /// Admission of a `linear_steps` task (§5.2): its operations form one chain from the entry
    /// page to the target page, and every page it names resolves to one detector page.
    pub(super) fn validate_linear(
        &self,
        control: &TaskControl,
        detector: &PageDetector,
    ) -> Result<LinearPlan, ContainedTaskError> {
        let invalid = |reason: &str, operation: Option<&TaskOperation>| {
            ContainedTaskError::with_detail(
                LINEAR_INVALID,
                match operation {
                    Some(operation) => format!("reason={reason} operation={}", operation.id),
                    None => format!("reason={reason}"),
                },
            )
        };
        if !self.schema_at_least(TaskSchema::V0_9)
            || control.schema_version != PHASED_CONTROL_SCHEMA
        {
            return Err(invalid("schema_version", None));
        }
        if self.phases.is_some() || control.phases.is_some() {
            return Err(invalid("phases", None));
        }
        let count = u32::try_from(self.operations.len())
            .ok()
            .filter(|count| (1..=MAX_STEPS).contains(count))
            .ok_or_else(|| invalid("operation_count", None))?;
        if self.max_steps != Some(count) || control.max_steps != Some(count) {
            return Err(invalid("max_steps", None));
        }
        for (reason, declared) in [
            ("error_pages", !self.error_pages.is_empty()),
            ("recovery", self.recovery.is_some()),
            (
                "stability_termination",
                self.stability_termination.is_some() || control.stability_termination.is_some(),
            ),
            ("post_admission_ocr", self.post_admission_ocr.is_some()),
            (
                "stop_on_confirmation",
                control.stop_on_confirmation == Some(false),
            ),
        ] {
            if declared {
                return Err(invalid(reason, None));
            }
        }
        let resolve = |page: &str| resolve_page_reference(&control.game, page, detector);
        // Workflow #336 R24: the application entry `any` is never resolved to a detector page.
        // Workflow #373 NAV-3b: a linear task keeps one entry page; a set is refused.
        let entry = match self
            .single_entry_page()
            .ok_or_else(|| invalid("entry_page", None))?
        {
            ANY_PAGE => LinearFrom::Any,
            page => LinearFrom::Page(resolve(page)?),
        };
        let target = match self.target_pages()?.as_slice() {
            [page] => resolve(page.as_str())?,
            _ => return Err(invalid("target_page", None)),
        };
        let mut operation_ids = BTreeSet::new();
        let mut steps = Vec::with_capacity(self.operations.len());
        for (index, operation) in self.operations.iter().enumerate() {
            // #411 B36/B19 (A35-S3): a wait step is a page-graph declaration. Workflow #407 LOAD-2:
            // a confirmation's wait page and loading are admitted (`validate_loading`).
            if operation.declares_wait() {
                return Err(invalid("wait", Some(operation)));
            }
            // Workflow #428 S168-4: a list reading is a page-graph step.
            if operation.declares_read_list() {
                return Err(invalid(
                    super::list_reading::READ_LIST_MEMBER,
                    Some(operation),
                ));
            }
            // ACT-4: final-state targets are judged by the page-graph confirmation.
            if operation.declares_expected_targets() {
                return Err(invalid("expect_targets", Some(operation)));
            }
            // Workflow #463 K1: so is an `expect_identity`, which reads a select step's choice
            // back on the page-graph destination wait; a linear arrival does not run it.
            if operation
                .expect_after
                .as_ref()
                .and_then(|expectation| expectation.expect_identity.as_ref())
                .is_some_and(|identity| {
                    self.operations
                        .iter()
                        .any(|chooser| chooser.id == identity.choice_of && chooser.select.is_some())
                })
            {
                return Err(invalid("expect_identity", Some(operation)));
            }
            operation.validate(control, self.defaults, &self.schema_version)?;
            if !operation_ids.insert(operation.id.as_str()) {
                return Err(ContainedTaskError::new(
                    ExecutionKernelCode::ContainedTaskProgramInvalid.code(),
                ));
            }
            // Workflow #463 K1: a select step is admitted; `on_error` is a page-graph declaration.
            if operation.on_error.is_some() {
                return Err(invalid("operation_effect", Some(operation)));
            }
            // Workflow #390 EXEC-2: the observation window is a page-graph declaration. Workflow
            // #411 B12 (Q9): an input recheck of any kind is admitted under `linear_steps` too.
            if operation.observation_window.is_some() {
                return Err(invalid("observation_window", Some(operation)));
            }
            // Workflow #390 EXEC-1: candidate operations are page-graph declarations.
            if operation.when.is_some() {
                return Err(invalid("when", Some(operation)));
            }
            if operation.on_guard_refused.is_some() {
                return Err(invalid("on_guard_refused", Some(operation)));
            }
            // #411 B16 part 1: a per-run cap is a page-graph declaration; a step plan runs each
            // operation once.
            if operation.max_runs.is_some() || operation.on_max_runs.is_some() {
                return Err(invalid("max_runs", Some(operation)));
            }
            let effect = match (&operation.click, &operation.application, &operation.select) {
                (Some(_), None, None) => LinearEffect::Click,
                (None, Some(application), None) => LinearEffect::Application(application.action),
                (None, None, Some(_)) => LinearEffect::Select,
                _ => return Err(invalid("operation_effect", Some(operation))),
            };
            if matches!(effect, LinearEffect::Application(_))
                && (operation.retryable.is_some()
                    || operation.max_attempts.is_some()
                    || operation.retry_interval_ms.is_some())
            {
                return Err(invalid("application_retry", Some(operation)));
            }
            // Workflow #463 K1 (the coordinator's ruling 1 on board #463): nor a select step. A
            // retry after a scan would scan again after its moves; a swallowed tap ends as a click
            // step without a retry ends.
            if matches!(effect, LinearEffect::Select)
                && (operation.retryable.is_some()
                    || operation.max_attempts.is_some()
                    || operation.retry_interval_ms.is_some())
            {
                return Err(invalid("select_retry", Some(operation)));
            }
            let from = if operation.from == ANY_PAGE {
                if index != 0 {
                    return Err(invalid("any_from", Some(operation)));
                }
                // Workflow #463 K1: a select step chooses on a page, so it never starts from any
                // screen either.
                if matches!(effect, LinearEffect::Click | LinearEffect::Select) {
                    return Err(invalid("any_requires_application", Some(operation)));
                }
                LinearFrom::Any
            } else {
                LinearFrom::Page(resolve(operation.from.as_str())?)
            };
            let to = match operation.destination_pages()?.as_slice() {
                [page] => resolve(page.as_str())?,
                _ => return Err(invalid("destination", Some(operation))),
            };
            if matches!(&from, LinearFrom::Page(page) if *page == to) {
                return Err(invalid("to_equals_from", Some(operation)));
            }
            let transition = match &operation.transition {
                None => None,
                Some(TaskTransition::Page {
                    page_id,
                    timeout_ms,
                    interval_ms,
                }) => {
                    let page = resolve(page_id.as_str())?;
                    if page == to || matches!(&from, LinearFrom::Page(from) if *from == page) {
                        return Err(invalid("transition_page", Some(operation)));
                    }
                    if timeout_ms.is_some_and(|value| value == 0 || value > MAX_TASK_TIMEOUT_MS) {
                        return Err(invalid("transition_timeout", Some(operation)));
                    }
                    // PERF-B1 (R2): bounded as the control's interval is; the run checks the
                    // ceiling in effect.
                    if interval_ms
                        .is_some_and(|value| value == 0 || value > MAX_DECLARED_INTERVAL_MS)
                    {
                        return Err(invalid("transition_interval", Some(operation)));
                    }
                    let arrival = operation.effective_timing(control);
                    Some(LinearTransition::Page {
                        page,
                        timeout: Duration::from_millis(
                            timeout_ms.unwrap_or(arrival.timeout.milliseconds),
                        ),
                        interval: Duration::from_millis(
                            interval_ms.unwrap_or(control.capture_interval().milliseconds),
                        ),
                    })
                }
                Some(TaskTransition::Window { min_ms, max_ms }) => {
                    if min_ms > max_ms
                        || *max_ms == 0
                        || *max_ms > MAX_TASK_TIMEOUT_MS
                        || *min_ms >= control.task_timeout().milliseconds
                    {
                        return Err(invalid("transition_window", Some(operation)));
                    }
                    Some(LinearTransition::Window {
                        min: Duration::from_millis(*min_ms),
                        max: Duration::from_millis(*max_ms),
                    })
                }
            };
            // Workflow #339: the first step is the only page the entry gate checks, and an
            // optional application step would be a conditional restart.
            let optional = match &operation.optional {
                None => None,
                Some(_) if index == 0 => {
                    return Err(invalid("optional_first_step", Some(operation)));
                }
                Some(_) if matches!(effect, LinearEffect::Application(_)) => {
                    return Err(invalid("optional_application", Some(operation)));
                }
                // Workflow #463 K1: a select step's page is its list, which it reads whatever it
                // shows; skipping it when absent would be no choice.
                Some(_) if matches!(effect, LinearEffect::Select) => {
                    return Err(invalid("select_optional", Some(operation)));
                }
                Some(optional) if optional.settle_ms > MAX_STEP_TIMEOUT_MS => {
                    return Err(invalid("optional_settle", Some(operation)));
                }
                Some(optional) => Some(Duration::from_millis(optional.settle_ms)),
            };
            // Workflow #407 LOAD-2 (design `model-loading.md` §5): a page the step's loading adds,
            // or its wait page, is none of its own pages: a frame of it never proves the input.
            // LOAD-4: the step's waits evaluate them by these ids. LOAD-4b: nor a page of its run,
            // checked once the runs are known.
            let mut loading_added = Vec::new();
            for page in operation.loading_added_pages() {
                let page = resolve(page)?;
                let transition_page = matches!(
                    &transition,
                    Some(LinearTransition::Page { page: passed, .. }) if *passed == page
                );
                if page == to
                    || transition_page
                    || matches!(&from, LinearFrom::Page(from) if *from == page)
                {
                    return Err(invalid("loading_page", Some(operation)));
                }
                loading_added.push(page);
            }
            steps.push(LinearStep {
                from,
                to,
                effect,
                transition,
                optional,
                run: None,
                loading_added,
            });
        }
        if steps[0].from != entry {
            return Err(invalid("entry_page", Some(&self.operations[0])));
        }
        // Workflow #336 L2b (R24): an application entry recognizes nothing before its effect, so
        // there is no first step a prerequisite package could lead to.
        if entry == LinearFrom::Any && control.prerequisite_package_id.is_some() {
            return Err(invalid(
                "prerequisite_with_application_entry",
                Some(&self.operations[0]),
            ));
        }
        for (index, pair) in steps.windows(2).enumerate() {
            if !matches!(&pair[1].from, LinearFrom::Page(from) if *from == pair[0].to) {
                return Err(invalid("chain", Some(&self.operations[index + 1])));
            }
            // After a stop the assigned application is not in the foreground, where the
            // foreground gate refuses every pointer input: only a launch or restart may follow.
            if matches!(
                pair[0].effect,
                LinearEffect::Application(ApplicationLifecycleAction::Stop)
            ) && !matches!(
                pair[1].effect,
                LinearEffect::Application(
                    ApplicationLifecycleAction::Launch | ApplicationLifecycleAction::Restart
                )
            ) {
                return Err(invalid(
                    "input_after_application_stop",
                    Some(&self.operations[index + 1]),
                ));
            }
        }
        let last = steps.len() - 1;
        if steps[last].to != target {
            return Err(invalid("target_page", Some(&self.operations[last])));
        }
        // Workflow #336 R25: a launch or restart is complete only on the main interface
        // (`linear_main_interface`); one such page must follow the last of them.
        let last_start = steps.iter().rposition(|step| {
            matches!(
                step.effect,
                LinearEffect::Application(
                    ApplicationLifecycleAction::Launch | ApplicationLifecycleAction::Restart
                )
            )
        });
        if let Some(start) = last_start
            && !steps[start..]
                .iter()
                .any(|step| linear_main_interface(&control.game, &step.to))
        {
            return Err(invalid(
                "application_without_home",
                Some(&self.operations[start]),
            ));
        }
        // Workflow #339: the first main interface after the last launch or restart is the page
        // of a required operation, so every path through the runs reaches it.
        if let Some(home) = last_start.and_then(|start| {
            steps[start..]
                .iter()
                .position(|step| linear_main_interface(&control.game, &step.to))
                .map(|offset| start + offset + 1)
        }) && steps.get(home).is_some_and(|step| step.optional.is_some())
        {
            return Err(invalid(
                "optional_restart_segment_end",
                Some(&self.operations[home]),
            ));
        }
        let runs = optional_runs(&mut steps)
            .map_err(|(reason, index)| invalid(reason, Some(&self.operations[index])))?;
        let plan = LinearPlan {
            target,
            steps,
            runs,
        };
        // Workflow #407 LOAD-4b (register §1.265): nor any other page the step's waits await, an
        // optional page of its run (Workflow #339) or the run's skip target, in the largest
        // candidate set of each step. Under `execution.loading.mode` `on` such a page is a
        // transition frame on every frame it shows, never a candidate, so the wait could only
        // spend its allowance and fail.
        let none_handled = BTreeSet::new();
        for (index, step) in plan.steps.iter().enumerate() {
            let awaited = plan.candidates(index, &none_handled).pages;
            if step
                .loading_added
                .iter()
                .any(|page| awaited.contains(&page.as_str()))
            {
                return Err(invalid("loading_page", Some(&self.operations[index])));
            }
        }
        if let Some(declaration) = &self.scheduling_outcome {
            declaration.validate().map_err(|_| {
                ContainedTaskError::new(
                    ExecutionKernelCode::ContainedTaskOutcomeDeclarationInvalid.code(),
                )
            })?;
            // Workflow #390 OUT-2: nor a designated effect set.
            if declaration.has_designated_effect() {
                return Err(invalid("designated_operation", None));
            }
            // Workflow #463 K1: the page of each select step that declares `on_empty`, on which
            // its empty decision ends the run.
            let empty_ends = plan
                .steps
                .iter()
                .zip(&self.operations)
                .filter_map(
                    |(step, operation)| match (&step.from, operation.on_empty_key()) {
                        (LinearFrom::Page(page), Some(key)) => {
                            Some((page.as_str(), key, operation))
                        }
                        _ => None,
                    },
                )
                .collect::<Vec<_>>();
            for page in declaration
                .mappings()
                .iter()
                .flat_map(|mapping| mapping.terminal_pages())
            {
                let page = resolve(page.as_str())?;
                if page != plan.target && !empty_ends.iter().any(|(empty, ..)| *empty == page) {
                    return Err(invalid("terminal_page", None));
                }
            }
            // Workflow #463 K1: the host derives a run's key from the page it finished on, and
            // the page-graph coverage check that gives an `on_empty` page its key is not run
            // here: by the one keyed-terminal rule, exactly one mapping lists the step's page,
            // and its key is the step's `on_empty` key. The task has no designated effect.
            for (page, key, operation) in empty_ends {
                // K1 fix round 1: nor is that page the target page. Its one mapping would be the
                // `on_empty` key's, and the host, which keys a run by the page it finished on,
                // would record a run that reached the target as one that found nothing.
                if page == plan.target {
                    return Err(invalid("select_on_empty_target", Some(operation)));
                }
                if !keyed_terminal_holds(
                    declaration,
                    SchedulingEffectCondition::NoDesignatedEffect,
                    key,
                    &|mapped: &str| crate::page_anchor_matches(&control.game, page, mapped),
                ) {
                    return Err(invalid("select_on_empty_terminal", Some(operation)));
                }
            }
        }
        Ok(plan)
    }
}

/// Workflow #339 §3.2: the maximal runs of optional operations, the operation before each run
/// and its members linked to it. A refusal is a `contained_task_linear_invalid` reason and the
/// index of the operation at fault.
fn optional_runs(steps: &mut [LinearStep]) -> Result<Vec<OptionalRun>, (&'static str, usize)> {
    let mut runs = Vec::new();
    let mut index = 0;
    while index < steps.len() {
        if steps[index].optional.is_none() {
            index += 1;
            continue;
        }
        // The first operation is never optional: `start - 1` is the required one before the run.
        let start = index;
        while steps.get(index).is_some_and(|step| step.optional.is_some()) {
            index += 1;
        }
        let members = (start..index).collect::<Vec<_>>();
        let skip_page = steps[index - 1].to.clone();
        {
            // The pages the run's inputs may reach, and the page of the operation before it,
            // which its retry decision adds, are distinct: no candidate list repeats a page.
            let mut reachable = BTreeSet::new();
            for &member in &members {
                if !reachable.insert(steps[member].from.label()) {
                    return Err(("optional_candidates", member));
                }
            }
            if !reachable.insert(skip_page.as_str()) {
                return Err(("optional_candidates", index - 1));
            }
            if reachable.contains(steps[start - 1].from.label()) {
                return Err(("optional_candidates", start - 1));
            }
            // An intermediate page differs from every page its operation's input may reach.
            for (offset, step) in steps[start - 1..index].iter().enumerate() {
                if let Some(LinearTransition::Page { page, .. }) = &step.transition
                    && reachable.contains(page.as_str())
                {
                    return Err(("transition_page", start - 1 + offset));
                }
            }
        }
        for step in &mut steps[start - 1..index] {
            step.run = Some(runs.len());
        }
        runs.push(OptionalRun {
            members,
            skip_page,
            next: (index < steps.len()).then_some(index),
        });
    }
    Ok(runs)
}

impl PreparedContainedTask {
    /// Runs an admitted linear task after `PackageAdmitted` and `RunStarted` (§5.3).
    pub(super) fn run_linear_steps<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        ocr_collector: &mut PostAdmissionOcrCollector<'_>,
        plan: &LinearPlan,
        mut run: LinearRun,
    ) -> Result<ContainedTaskOutcome, ContainedTaskRunError<R::Error>> {
        // Workflow #428 DIAG-SI: an admitted plan always has a first step.
        let first = plan
            .steps
            .first()
            .ok_or_else(|| invariant_broken(|outcome| outcome))?;
        // An application entry captures nothing before its effect (R24): no frame, no wait.
        let mut observation = match &first.from {
            LinearFrom::Any => None,
            LinearFrom::Page(page) => {
                let entry_started = budget_now(runtime, run.timing.pace());
                // Workflow #407 LOAD-4 (§5): the entry wait has an allowance of its own, the
                // package's, as a page wait has.
                let mut loading = LoadingAllowance::for_page_wait(self, run.pacing);
                let entered = self.linear_wait(
                    runtime,
                    &[page.as_str()],
                    run.step_timeout,
                    run.capture_interval,
                    LinearWaitPurpose::Entry,
                    (run, &mut loading),
                )?;
                match entered {
                    Ok(observed) => Some(observed),
                    Err(loading_limit) => {
                        return Err(ContainedTaskError::with_detail(
                            ENTRY_UNMATCHED,
                            format!("page={page}"),
                        )
                        .with_timing(Some(TaskTimingFailure {
                            scope: TaskTimingScope::PageRecognition,
                            stage: TaskTimingStage::EntryRecognition,
                            elapsed_ms: budget_elapsed_since(
                                runtime,
                                run.timing.pace(),
                                entry_started,
                            )
                            .as_millis() as u64,
                            limit_ms: run.step_timeout.as_millis() as u64,
                            required_delay_ms: None,
                        }))
                        .with_loading_limit(loading_limit)
                        .into());
                    }
                }
            }
        };
        // Workflow #384 D2-S2 (model §3.5): the entry frame goes to recognition when the entry
        // page carries marks the run reads. A linear frame is captured without the run's
        // collector, so its page gate keeps no observation for the marks (Workflow #396 REC-6).
        if let Some(observed) = &observation {
            self.hand_off_reading_frame(
                runtime,
                ocr_collector,
                &observed.page_label,
                &observed.scene,
                observed.captured_at,
                RememberedOcrObservations::default(),
                run.timing.pace(),
            )?;
        }
        // Workflow #384 D2-S2 (model §3.4): with a requested key set, a segment that reaches no
        // page marking one of its keys is not run; the run stays on the entry page it starts
        // and ends on.
        let unread_segments = match ocr_collector
            .reading_relay
            .as_ref()
            .filter(|relay| relay.requested().is_some())
        {
            Some(relay) => {
                plan.segments_without_reads(|page| relay.marks_wanted_on(&self.control.game, page))
            }
            None => std::collections::BTreeMap::new(),
        };
        // Workflow #339 §3.3: a cursor over the operations. `dispatched` counts the operations
        // run, as the page-graph path counts its steps, and is the next `step_index`; a skipped
        // optional operation writes nothing. The next operation follows from the page that
        // passed, never from a comparison with the target page. Without an optional operation
        // every operation runs in order and `dispatched` is the operation index.
        // Workflow #428 DIAG-SI: the cursor's own bookkeeping, a Runtime-internal invariant.
        let state_invalid = || invariant_broken(|outcome| outcome);
        let mut current = 0;
        let mut dispatched = 0_u32;
        let mut handled = BTreeSet::new();
        loop {
            if let Some(after) = unread_segments.get(&current) {
                if *after >= plan.steps.len() {
                    break;
                }
                current = *after;
                continue;
            }
            let (operation, step) = self
                .program
                .operations
                .get(current)
                .zip(plan.steps.get(current))
                .ok_or_else(state_invalid)?;
            // Workflow #463 U1 (R25-1): a `launch` or a `restart` opens the restart segment.
            if matches!(
                step.effect,
                LinearEffect::Application(
                    ApplicationLifecycleAction::Launch | ApplicationLifecycleAction::Restart
                )
            ) {
                run.restart_segment = true;
            }
            let candidates = plan.candidates(current, &handled);
            let reached = match self.run_linear_step(
                runtime,
                &mut run,
                dispatched,
                operation,
                step,
                &candidates,
                observation,
            )? {
                LinearStepEnd::Reached(reached) => *reached,
                // Workflow #463 K1 (`contracts/linear-steps.md`, "Select steps"): the run ends
                // Completed on the select step's page with its `on_empty` key, the step counted,
                // as the page graph finishes a keyed end; no run state machine is needed.
                LinearStepEnd::Empty(empty) => {
                    let executed_steps = dispatched.checked_add(1).ok_or_else(state_invalid)?;
                    return self.finish_success_with_key(
                        runtime,
                        ocr_collector,
                        Some(empty.page_label),
                        (executed_steps, &[]),
                        Some(empty.outcome_key),
                        run.timing.pace(),
                    );
                }
            };
            // Workflow #463 U1 (R25-1): a step that arrived on the main interface closes it.
            if linear_main_interface(&self.control.game, &reached.page_label) {
                run.restart_segment = false;
            }
            // Workflow #384 D2-S2 (model §3.5): the frame the step arrived on goes to
            // recognition when its page carries marks the run reads.
            self.hand_off_reading_frame(
                runtime,
                ocr_collector,
                &reached.page_label,
                &reached.scene,
                reached.captured_at,
                RememberedOcrObservations::default(),
                run.timing.pace(),
            )?;
            // Workflow #384 D2-S3 (model §3.6): the step's operation is confirmed on the frame
            // it arrived on.
            self.confirm_resource_adjustments(runtime, ocr_collector, &operation.id, &reached)?;
            dispatched = dispatched.checked_add(1).ok_or_else(state_invalid)?;
            let next = match step.run.map(|run| &plan.runs[run]) {
                None => Some(current + 1).filter(|next| *next < plan.steps.len()),
                Some(optional_run) => {
                    if step.optional.is_some() {
                        handled.insert(current);
                    }
                    // An optional page passed: its operation runs next. The skip target passed:
                    // the operations of the run not run are skipped.
                    match optional_run.members.iter().copied().find(|member| {
                        !handled.contains(member)
                            && plan.steps[*member].from.label() == reached.page_label
                    }) {
                        Some(member) => Some(member),
                        None if reached.page_label == optional_run.skip_page => {
                            handled.clear();
                            optional_run.next
                        }
                        None => {
                            return Err(invariant_broken(|outcome| {
                                outcome
                                    .pack_operation_id(operation.id.as_str())
                                    .page_id(reached.page_label.as_str())
                            })
                            .into());
                        }
                    }
                }
            };
            observation = Some(reached);
            match next {
                Some(next) => current = next,
                None => break,
            }
        }
        self.finish_success(
            runtime,
            ocr_collector,
            Some(plan.target.clone()),
            (dispatched, &[]),
            run.timing.pace(),
        )
    }

    /// One operation (§5.3 a–h, §5.5), from the frame on which its own page passed (none for the
    /// application entry) to the frame on which one of its `candidates` passed (Workflow #339:
    /// the next step's page, or a page of a run of optional operations). #385 P-k: `run` takes
    /// the step's pacing and budget clock, for this step and for the run after it. Workflow #463
    /// K1: or, for a select step that declares `on_empty`, to its empty end on its own page.
    #[allow(clippy::too_many_arguments)]
    fn run_linear_step<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        run: &mut LinearRun,
        step_index: u32,
        operation: &TaskOperation,
        step: &LinearStep,
        candidates: &LinearCandidates<'_>,
        mut observation: Option<PageObservation>,
    ) -> Result<LinearStepEnd, ContainedTaskRunError<R::Error>> {
        let deadline = run.timing.deadline();
        // Workflow #386 SR-1 (scheduling model §2.1 item 4): the step's pacing, taken once at
        // its start; its observation window opens on the observation clock. #385 P-k (§5.6 item
        // 2): its factor holds until the next step's answer.
        run.pacing = runtime
            .step_pacing()
            .map_err(ContainedTaskRunError::operation::<R>)?;
        run.timing = run
            .timing
            .with_pace(paced_step(runtime, run.timing.pace(), run.pacing)?);
        let run = *run;
        let pace = run.timing.pace();
        // Workflow #463 K1: a select over a scan observes within its `max_ms`, capped by the
        // task's, in place of the step timeout, as on the page-graph path (Workflow #308 SEL-L4b);
        // every other step within the step timeout.
        let step_window = operation
            .scan_max_ms()
            .map_or(run.step_timeout, Duration::from_millis);
        let step_timing = run
            .timing
            .with_deadline(budget_observation_now(runtime, pace) + step_window);
        // The dispatched logical step, as the page-graph path counts it; a retry adds none.
        runtime.update_run_progress(step_index.saturating_add(1));
        let policy = operation.retry_policy(
            self.program.defaults,
            self.control.timeout_ms.unwrap_or(DEFAULT_TASK_TIMEOUT_MS),
        )?;
        let max_attempts = policy.as_ref().map_or(1, RunOperationPolicy::max_attempts);
        let retry_interval = Duration::from_millis(
            policy
                .as_ref()
                .map_or(1, RunOperationPolicy::retry_interval_ms),
        );
        let timing = operation.effective_timing(&self.control);
        let window_min = match &step.transition {
            Some(LinearTransition::Window { min, .. }) => *min,
            _ => Duration::ZERO,
        };
        let arrival = LinearArrival {
            budget: Duration::from_millis(timing.timeout.milliseconds),
            interval: Duration::from_millis(timing.interval.milliseconds),
            post_input_delay: Duration::from_millis(operation.post_delay_ms.unwrap_or(0))
                .max(window_min),
        };
        let mut attempt = 1;
        loop {
            if budget_now(runtime, pace) >= deadline {
                return Err(self
                    .task_timeout_error(runtime, TaskTimingStage::Dispatch, deadline, None, pace)
                    .into());
            }
            runtime
                .record(ContainedTaskTrace::StepStarted {
                    step_index,
                    operation_label: operation.id.clone(),
                    from_page: step.from.label().to_owned(),
                    phase: None,
                })
                .map_err(ContainedTaskRunError::Boundary)?;
            match step.effect {
                LinearEffect::Click => {
                    let frame = observation.as_ref().ok_or_else(|| {
                        ContainedTaskError::new(SharedCode::ContainedTaskPageUnknown.code())
                    })?;
                    self.linear_input(
                        runtime,
                        run,
                        step_index,
                        operation,
                        frame,
                        step_timing,
                        attempt,
                    )?;
                }
                // Workflow #463 K1: a select step's attempt replaces the guard and the click; its
                // tap goes on as a click's does. A select step declares no retry, so this is its
                // one attempt.
                LinearEffect::Select => {
                    let frame = observation.as_ref().ok_or_else(|| {
                        ContainedTaskError::new(SharedCode::ContainedTaskPageUnknown.code())
                    })?;
                    if let Some(empty) = self.linear_select_input(
                        runtime,
                        run,
                        step_index,
                        operation,
                        frame,
                        step_timing,
                    )? {
                        // No input was made: the step finishes on its page and the run ends.
                        Self::linear_step_finished(
                            runtime,
                            step_index,
                            operation,
                            &empty.page_label,
                        )?;
                        return Ok(LinearStepEnd::Empty(empty));
                    }
                }
                LinearEffect::Application(action) => {
                    self.linear_application(runtime, run, step_index, operation, action)?;
                }
            }
            // Workflow #407 LOAD-4 (§5): the attempt's one allowance for its loading pages, shared
            // by its waits and its retry decision's; the next input takes a new one.
            let mut loading = LoadingAllowance::for_linear_input(
                self,
                operation,
                run.pacing,
                &step.loading_added,
            );
            self.linear_post_input_delay(runtime, run, arrival.post_input_delay)?;
            let miss = match self.linear_after_input(
                runtime,
                run,
                step,
                candidates,
                arrival,
                &mut loading,
            )? {
                Ok(reached) => {
                    Self::linear_step_finished(
                        runtime,
                        step_index,
                        operation,
                        &reached.page_label,
                    )?;
                    return Ok(LinearStepEnd::Reached(Box::new(reached)));
                }
                Err(miss) => miss,
            };
            let awaited = candidates.awaited(miss.skip_target_seen);
            if let LinearEffect::Application(action) = step.effect {
                // §5.3 h: an application step is never retried and has no retry decision.
                Self::linear_step_finished(runtime, step_index, operation, UNRECOGNIZED_PAGE)?;
                return Err(
                    application_unconfirmed(operation, step, action, miss, &awaited).into(),
                );
            }
            // Workflow #339 §3.5: a seen skip target shows the input took effect; another input
            // would not help, as after a seen intermediate page.
            if miss.kind == LinearMissKind::AfterIntermediate
                || miss.skip_target_seen
                || attempt >= max_attempts
            {
                // Workflow #463 U1 (fix round 2, rule 4): where the end here is
                // `page_confirmation_failed` (the last attempt's arrival, after a seen
                // intermediate page or skip target), the closing decision classifies it first.
                let unrecorded = self.linear_closing(
                    runtime,
                    (run, &mut loading),
                    step,
                    candidates,
                    (miss.kind, miss.loading_limit.is_some()),
                )?;
                Self::linear_step_finished(runtime, step_index, operation, UNRECOGNIZED_PAGE)?;
                if let Some(pages) = unrecorded {
                    return Err(unrecorded_screen(
                        operation,
                        step,
                        (attempt, miss.kind == LinearMissKind::AfterIntermediate),
                        &awaited,
                        &pages,
                        miss.elapsed,
                        miss.limit,
                    )
                    .into());
                }
                return Err(match (miss.kind, &step.transition, miss.loading_limit) {
                    // Workflow #407 LOAD-4 (§5): a wait that ended on the attempt's loading
                    // allowance, the intermediate page's included, is the confirmation's failure
                    // with the loading values.
                    (
                        LinearMissKind::Intermediate,
                        Some(LinearTransition::Page { page, .. }),
                        None,
                    ) => ContainedTaskError::with_detail(
                        INTERMEDIATE_UNOBSERVED,
                        format!(
                            "operation={} attempts={attempt} intermediate_page={page}",
                            operation.id
                        ),
                    )
                    .with_timing(postcondition_timing(miss.elapsed, miss.limit)),
                    (kind, _, loading_limit) => page_confirmation_failed(
                        operation,
                        step,
                        attempt,
                        kind == LinearMissKind::AfterIntermediate,
                        &awaited,
                        miss.elapsed,
                        miss.limit,
                    )
                    .with_loading_limit(loading_limit),
                }
                .into());
            }
            // The retry decision (§5.5): the gate first, then this step's own page. The gate is
            // the intermediate page, or every candidate (Workflow #339 §3.5).
            // Workflow #428 DIAG-SI: admission gives an application entry no retry.
            let LinearFrom::Page(own_page) = &step.from else {
                return Err(invariant_broken(|outcome| {
                    outcome.pack_operation_id(operation.id.as_str())
                })
                .into());
            };
            self.linear_retry_delay(runtime, run, retry_interval)?;
            let decision_started = budget_now(runtime, pace);
            let mut decision_pages = match &step.transition {
                Some(LinearTransition::Page { page, .. }) => vec![page.as_str()],
                _ => candidates.pages.clone(),
            };
            decision_pages.push(own_page.as_str());
            // Workflow #407 LOAD-4 (§5): the decision's waits share the attempt's allowance, and a
            // transition frame is never the gate or the step's own page, so no input follows a
            // loading screen.
            let decided = self.linear_wait(
                runtime,
                &decision_pages,
                run.step_timeout,
                run.capture_interval,
                LinearWaitPurpose::RetryDecision,
                (run, &mut loading),
            )?;
            match decided {
                Ok(frame) if frame.page_label != *own_page => {
                    // The input took effect late: this attempt goes on, without another input.
                    let continued = match &step.transition {
                        Some(LinearTransition::Page { .. }) => self.linear_arrival(
                            runtime,
                            (run, &mut loading),
                            candidates,
                            arrival.budget,
                            arrival.interval,
                            LinearMissKind::AfterIntermediate,
                        )?,
                        // Workflow #339: the skip target is watched from this frame on.
                        _ if candidates.settles_on(&frame.page_label) => {
                            let seen = if frame.sampling.is_some() {
                                pace.budget(frame.frame_started)
                            } else {
                                budget_now(runtime, pace)
                            };
                            self.linear_wait_set(
                                runtime,
                                (run, &mut loading),
                                candidates,
                                arrival.budget,
                                arrival.interval,
                                LinearMissKind::Arrival,
                                Some(seen),
                            )?
                        }
                        _ => {
                            Self::linear_step_finished(
                                runtime,
                                step_index,
                                operation,
                                &frame.page_label,
                            )?;
                            return Ok(LinearStepEnd::Reached(Box::new(frame)));
                        }
                    };
                    match continued {
                        Ok(reached) => {
                            Self::linear_step_finished(
                                runtime,
                                step_index,
                                operation,
                                &reached.page_label,
                            )?;
                            return Ok(LinearStepEnd::Reached(Box::new(reached)));
                        }
                        Err(miss) => {
                            // Workflow #463 U1 (fix round 2, rule 4): the continued wait after a
                            // late gate ends with `page_confirmation_failed`; the closing decision
                            // classifies that end first.
                            let unrecorded = self.linear_closing(
                                runtime,
                                (run, &mut loading),
                                step,
                                candidates,
                                (miss.kind, miss.loading_limit.is_some()),
                            )?;
                            Self::linear_step_finished(
                                runtime,
                                step_index,
                                operation,
                                UNRECOGNIZED_PAGE,
                            )?;
                            let awaited = candidates.awaited(miss.skip_target_seen);
                            let intermediate_seen = miss.kind == LinearMissKind::AfterIntermediate;
                            if let Some(pages) = unrecorded {
                                return Err(unrecorded_screen(
                                    operation,
                                    step,
                                    (attempt, intermediate_seen),
                                    &awaited,
                                    &pages,
                                    miss.elapsed,
                                    miss.limit,
                                )
                                .into());
                            }
                            return Err(page_confirmation_failed(
                                operation,
                                step,
                                attempt,
                                intermediate_seen,
                                &awaited,
                                miss.elapsed,
                                miss.limit,
                            )
                            .with_loading_limit(miss.loading_limit)
                            .into());
                        }
                    }
                }
                Ok(frame) => {
                    // This step's own page: the input was swallowed; input again on this frame.
                    Self::linear_step_finished(runtime, step_index, operation, &frame.page_label)?;
                    attempt += 1;
                    observation = Some(frame);
                }
                Err(loading_limit) => {
                    // Workflow #463 U1 (fix round 2, rules 1 and 2): a retry decision that saw
                    // neither page raises nothing itself. Unless the failed wait missed the
                    // intermediate page (rule 1 (a)) or this decision ended on the attempt's
                    // loading allowance, the closing decision classifies the end first; the
                    // decision's time is read before it, so the end keeps its timing.
                    let closing = self.linear_closing_pages(
                        runtime,
                        run,
                        step,
                        candidates,
                        (miss.kind, loading_limit.is_some()),
                    );
                    let (elapsed, unrecorded) = match closing {
                        Some(pages) => {
                            let elapsed = budget_elapsed_since(runtime, pace, decision_started);
                            let unrecorded =
                                self.linear_closing_decision(runtime, (run, &mut loading), &pages)?;
                            Self::linear_step_finished(
                                runtime,
                                step_index,
                                operation,
                                UNRECOGNIZED_PAGE,
                            )?;
                            (elapsed, unrecorded.then_some(pages))
                        }
                        None => {
                            Self::linear_step_finished(
                                runtime,
                                step_index,
                                operation,
                                UNRECOGNIZED_PAGE,
                            )?;
                            (budget_elapsed_since(runtime, pace, decision_started), None)
                        }
                    };
                    if let Some(pages) = unrecorded {
                        return Err(unrecorded_screen(
                            operation,
                            step,
                            (attempt, false),
                            &candidates.awaited(false),
                            &pages,
                            elapsed,
                            run.step_timeout,
                        )
                        .into());
                    }
                    return Err(page_confirmation_failed(
                        operation,
                        step,
                        attempt,
                        false,
                        &candidates.awaited(false),
                        elapsed,
                        run.step_timeout,
                    )
                    .with_loading_limit(loading_limit)
                    .into());
                }
            }
        }
    }

    /// Workflow #463 U1, fix round 2 (`contracts/linear-steps.md`, "A screen the package never
    /// recorded"): the closing decision of a step whose failed wait of kind `kind` (ended on the
    /// attempt's loading allowance when `loading_ended`) ends the run with
    /// `page_confirmation_failed` ([`Self::linear_closing_pages`],
    /// [`Self::linear_closing_decision`]): its pages when it found a screen the package never
    /// recorded, `None` when that end stands.
    fn linear_closing<'p, R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        (run, loading): (LinearRun, &mut LoadingAllowance),
        step: &'p LinearStep,
        candidates: &LinearCandidates<'p>,
        miss: (LinearMissKind, bool),
    ) -> Result<Option<Vec<&'p str>>, ContainedTaskRunError<R::Error>> {
        let Some(pages) = self.linear_closing_pages(runtime, run, step, candidates, miss) else {
            return Ok(None);
        };
        Ok(self
            .linear_closing_decision(runtime, (run, loading), &pages)?
            .then_some(pages))
    }

    /// Workflow #463 U1, fix round 2: the pages of the closing decision after a failed wait of
    /// kind `kind` (ended on the attempt's loading allowance when `loading_ended`): every page
    /// the wait awaited, then the step's own page. `None` where the decision is not made and the
    /// end stands as it is: a run that is not a task's own run (a layer's, a recovery entry's, or
    /// one the host starts itself, rule 5); a run inside a restart segment (R25-1); a wait that
    /// missed the intermediate page (rule 1 (a): the page may have flashed past onto one the
    /// package knows); a wait that ended on its loading allowance; an application entry; a
    /// package with sample consensus, whose observation gives no result of a frame by itself
    /// (rule 1 (b)); and a run whose time left, before its host deadline or the end of its
    /// admitted runtime budget ([`ContainedTaskRuntime::run_time_left`]), is shorter than twice
    /// the decision's bound, the step timeout (rule 6; fix round 3: the decision lasts its bound,
    /// and the capture and recognition of its last frame, the evaluation of the package's other
    /// pages and the work before the run's settlement come after it).
    fn linear_closing_pages<'p, R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        run: LinearRun,
        step: &'p LinearStep,
        candidates: &LinearCandidates<'p>,
        (kind, loading_ended): (LinearMissKind, bool),
    ) -> Option<Vec<&'p str>> {
        if !run.own_task_run
            || run.restart_segment
            || kind == LinearMissKind::Intermediate
            || loading_ended
            || !self.evaluator.pack().target_consensus.is_empty()
        {
            return None;
        }
        let LinearFrom::Page(own_page) = &step.from else {
            return None;
        };
        let bound = run.timing.pace().wall_span(run.step_timeout);
        if runtime
            .run_time_left()
            .is_some_and(|left| left < bound.saturating_mul(2))
        {
            return None;
        }
        let mut pages = candidates.pages.clone();
        pages.push(own_page.as_str());
        Some(pages)
    }

    /// Workflow #463 U1, fix round 2 (rules 1 to 3): the closing decision on `pages`. It observes
    /// for the step timeout at the capture interval from a first frame taken at once, as a retry
    /// decision observes, and only classifies: it never inputs, never goes on with the attempt
    /// and never hands a frame to the shared interrupt pages. `true` for a screen the package
    /// never recorded, which needs all of these: on every frame up to the bound's end none of
    /// `pages` passed and no shared interrupt page is shown (recognised, never handled); no frame
    /// is a transition frame (LOAD-4, under `on`); the decision's first and last frames, and its
    /// last two captures, are unchanged by the Runtime's one frame-change measure
    /// (`StillScreen::WHOLE_FRAME`); and on its last frame, the one the run pins, no other page of
    /// the package passes. Every other end is `false`, and so is every error raised by its own
    /// capture, recognition or evaluation, whatever its kind (fix round 3: the task's deadline, a
    /// capture or a recognition that failed as a task error or as an operation error of the
    /// runtime, a failed check of the shared interrupt pages): the failed wait's end stands. The
    /// one exception is a failure of the runtime's own bookkeeping, a record it could not write
    /// ([`ContainedTaskRuntime::is_bookkeeping_failure`]), which ends the run as it ends any run.
    fn linear_closing_decision<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        (run, loading): (LinearRun, &mut LoadingAllowance),
        pages: &[&str],
    ) -> Result<bool, ContainedTaskRunError<R::Error>> {
        match self.linear_closing_observation(runtime, (run, loading), pages) {
            Err(ContainedTaskRunError::Boundary(error)) if R::is_bookkeeping_failure(&error) => {
                Err(ContainedTaskRunError::Boundary(error))
            }
            Err(_) => Ok(false),
            observed => observed,
        }
    }

    /// The observation of [`Self::linear_closing_decision`].
    fn linear_closing_observation<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        (run, loading): (LinearRun, &mut LoadingAllowance),
        pages: &[&str],
    ) -> Result<bool, ContainedTaskRunError<R::Error>> {
        let deadline = run.timing.deadline();
        let pace = run.timing.pace();
        let game = self.control.game.as_str();
        let bound = run.step_timeout;
        // PERF-B1 (R3): the pause before the decision's next frame, as a retry decision's.
        let mut backoff = CaptureBackoff::new(run.capture_interval, run.pacing);
        let started = budget_now(runtime, pace);
        // Workflow #407 LOAD-4: the budget instant of the decision's latest frame, its start
        // before the first.
        let mut last_frame_at = started;
        // The frame-change digests of the decision's first and latest frames.
        let mut first: Option<ColorDigest> = None;
        let mut previous: Option<ColorDigest> = None;
        loop {
            if budget_now(runtime, pace) >= deadline {
                return Ok(false);
            }
            let observation =
                self.linear_observe_scene(runtime, pages, run.timing, Some(&mut *loading))?;
            if budget_now(runtime, pace) >= deadline {
                return Ok(false);
            }
            let frame_at = budget_now(runtime, pace);
            let previous_frame_at = std::mem::replace(&mut last_frame_at, frame_at);
            let judged = self.judge_frame(
                runtime,
                None,
                observation.as_ref().ok(),
                loading,
                run.timing,
            )?;
            let page = observation
                .as_ref()
                .ok()
                .map(|observation| observation.page_label.clone());
            // Workflow #407 LOAD-4: the frame counts for the attempt's loading records as a
            // retry decision's frame does; nothing here waits on it.
            loading.frame(
                runtime,
                game,
                LoadingFrame {
                    transition: judged.transition,
                    page: page.as_deref(),
                    now: frame_at,
                    previous: previous_frame_at,
                },
            )?;
            // Rule 1 (b): one of `pages` passed on this frame by itself; LOAD-4: a transition
            // frame. Neither is a screen the package never recorded.
            let scene = match observation {
                Ok(_) => return Ok(false),
                Err(_) if judged.row == FrameRow::Transition => return Ok(false),
                Err(scene) => scene,
            };
            // Rule 3: a shared interrupt page on the frame, recognised and not handled.
            if self.shown_shared_interrupt(runtime, &scene)? {
                return Ok(false);
            }
            // PERF-B1 (R3).
            backoff.frame(&scene)?;
            let digest = StillScreen::WHOLE_FRAME.digest(&scene)?;
            let last_two_still = match &previous {
                Some(previous) => !StillScreen::WHOLE_FRAME.moved(previous, &digest)?,
                None => false,
            };
            let now = budget_now(runtime, pace);
            if now.saturating_duration_since(started) >= bound {
                // Rules 1 (c) and 2: the decision's last two captures, and its first and its last
                // frame, are alike; rule 1 (d): no other page of the package is on the last frame.
                let first_still = match &first {
                    Some(first) => !StillScreen::WHOLE_FRAME.moved(first, &digest)?,
                    None => false,
                };
                return Ok(first_still && last_two_still && !self.another_page_on(pages, &scene));
            }
            if first.is_none() {
                first = Some(digest.clone());
            }
            previous = Some(digest);
            let boundary = LinearWaitPurpose::RetryDecision.boundary();
            let identity = runtime.task_boundary_identity(boundary);
            let wait_started = runtime.monotonic_now();
            let now = pace.budget(wait_started);
            pace_pause(
                runtime,
                pace,
                backoff
                    .interval()
                    .min(bound.saturating_sub(now.saturating_duration_since(started)))
                    .min(deadline.saturating_duration_since(now)),
            );
            let wait_ended = runtime.monotonic_now();
            runtime.observe_task_boundary(ContainedTaskBoundaryTiming {
                boundary,
                identity,
                context: run.timing,
                started: wait_started,
                ended: wait_ended,
                succeeded: true,
            });
        }
    }

    /// Workflow #463 U1, fix round 2 (rule 1 (d)): whether a page the package declares that
    /// `pages` leaves out (another step's page, an intermediate page, a skip target, a loading
    /// page) passes on `scene`, the frame the run would pin, in one scene evaluation that
    /// captures and records nothing. An error of that evaluation counts as such a page.
    fn another_page_on(&self, pages: &[&str], scene: &Scene) -> bool {
        let others = self
            .detector
            .page_ids()
            .chain(self.detector.loading_page_ids())
            .filter(|page| !pages.contains(page))
            .collect::<Vec<_>>();
        if others.is_empty() {
            return false;
        }
        match self.matching_pages_on_scene(scene, &others) {
            Ok(matched) => !matched.is_empty(),
            Err(_) => true,
        }
    }

    /// The guard on the frame that passed this step, then the effect intent and the input
    /// bound to that frame's input context, in the order of the page-graph path; with a declared
    /// input recheck (Workflow #411 B12), to the input context of the recheck's last frame. ACT-2:
    /// the first attempt waits the step's gap and takes a fresh frame of the step's page first.
    #[allow(clippy::too_many_arguments)]
    fn linear_input<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        run: LinearRun,
        step_index: u32,
        operation: &TaskOperation,
        observation: &PageObservation,
        step_timing: ContainedTaskTimingContext,
        attempt: u32,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        let deadline = run.timing.deadline();
        // Workflow #386 SR-1 (scheduling model §2.1 items 1-3): the guard's observation whose
        // sampling timed out is made again in a new window (a fresh frame of the step's page)
        // and guarded again, never repeating an input; the one sampling deadline check before
        // the input precedes the effect intent.
        let mut retries =
            SampleRetries::new(run.pacing, ExecutionKernelLocation::ObservationBeforeInput);
        let mut step_timing = step_timing;
        let mut again: Option<PageObservation> = None;
        if attempt == 1 {
            let gap = runtime
                .step_action_gap()
                .map_err(ContainedTaskRunError::operation::<R>)?;
            if !gap.is_zero() {
                self.wait_pre_input(runtime, gap, run.timing.pace(), deadline)?;
                let (frame, timing) = self.linear_observe_before_input(
                    runtime,
                    run,
                    &observation.page_label,
                    &mut retries,
                )?;
                again = Some(frame);
                step_timing = timing;
            }
        }
        let (guard, action, sampling, input_context) = loop {
            let frame = again.as_ref().unwrap_or(observation);
            let page = frame.page_label.clone();
            let guarded = operation.guard_outcome(
                &self.control,
                frame,
                &self.evaluator,
                runtime,
                Some(step_timing),
            );
            let (guard, target) = match guarded {
                Ok(guarded) => guarded,
                Err(failure) => {
                    retries.retry(runtime, failure, run.step_timeout)?;
                    let (frame, timing) =
                        self.linear_observe_before_input(runtime, run, &page, &mut retries)?;
                    again = Some(frame);
                    step_timing = timing;
                    continue;
                }
            };
            // Workflow #411 B12 (Q9): a click that declares a recheck is checked again on fresh
            // frames of the step's page, as on the page-graph path; its input is drawn from and
            // bound to the last of them. A refusal ends the run as a guard refusal does, with no
            // input and no retry.
            let (guard, target, rechecked) = match operation.input_recheck {
                None => (guard, target, None),
                Some(recheck) => match self.recheck_before_input(
                    runtime,
                    operation,
                    recheck,
                    frame,
                    true,
                    (run.timing, run.step_timeout),
                    &mut retries,
                )? {
                    InputRecheck::Held(held) => {
                        let RecheckedInput {
                            observation: fresh,
                            timing,
                            guard,
                            target,
                        } = *held;
                        step_timing = timing;
                        (guard, target, Some(fresh))
                    }
                    InputRecheck::Refused { error, .. } => return Err(error.into()),
                },
            };
            let frame = rechecked.as_ref().unwrap_or(frame);
            // Workflow #406 T12: the step's seed and the touch keys in effect.
            let draw = step_point_draw(runtime, step_index, &operation.id)?;
            let click = operation.click.as_ref().ok_or_else(|| {
                ContainedTaskError::new(ExecutionKernelCode::ContainedTaskOperationInvalid.code())
            })?;
            let (action, sampling) = click.draw_input::<R::Error>(
                &self.control.resolution,
                target.as_ref(),
                draw,
                &mut runtime_touch_choice(runtime),
            )?;
            if frame.sampling.is_some()
                && budget_observation_now(runtime, step_timing.pace()) >= step_timing.deadline()
            {
                let timeout = ContainedTaskError::new(
                    ExecutionKernelCode::RecognitionSampleDeadlineExceeded.code(),
                );
                retries.retry(
                    runtime,
                    ContainedTaskRunError::<R::Error>::Task(timeout),
                    run.step_timeout,
                )?;
                let (frame, timing) =
                    self.linear_observe_before_input(runtime, run, &page, &mut retries)?;
                again = Some(frame);
                step_timing = timing;
                continue;
            }
            break (guard, action, sampling, frame.input_context.clone());
        };
        self.linear_send_input(
            runtime,
            run,
            step_index,
            operation,
            LinearInput {
                action,
                sampling,
                guard,
                input_context,
            },
        )
    }

    /// Workflow #463 K1 (`contracts/linear-steps.md`, "Select steps"): a select step's attempt on
    /// the frame on which its page passed, the page graph's own
    /// ([`PreparedContainedTask::select_attempt`]): the step's layout is projected and decided by
    /// its policy, the choice is confirmed on a fresh frame, and one tap is drawn inside the chosen
    /// candidate's click box, bound to that frame's input context; the tap is then sent as a
    /// click's is ([`Self::linear_send_input`]). ACT-2: the step's gap and a fresh frame of its
    /// page come first. An observation whose sampling timed out is made again on a fresh frame of
    /// the step's page, never after an input; every other failure, and every failure of a select
    /// over a scan, whose moves may precede it, ends the run with its own code before any tap. An
    /// empty decision of a step that declares `on_empty` taps nothing and is returned.
    fn linear_select_input<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        run: LinearRun,
        step_index: u32,
        operation: &TaskOperation,
        observation: &PageObservation,
        step_timing: ContainedTaskTimingContext,
    ) -> Result<Option<LinearEmptyEnd>, ContainedTaskRunError<R::Error>> {
        let pace = run.timing.pace();
        // As on the page-graph path: a select over a scan observes within its `max_ms` (capped by
        // the task's), and its sampling deadline is checked by the scan itself.
        let window = operation
            .scan_max_ms()
            .map_or(run.step_timeout, Duration::from_millis);
        let sampled = operation.scan_max_ms().is_none()
            && selection::sampling_budget_ms(operation, &self.evaluator, &self.detector)?.is_some();
        let mut retries =
            SampleRetries::new(run.pacing, ExecutionKernelLocation::ObservationBeforeInput);
        let mut step_timing = step_timing;
        let mut again: Option<PageObservation> = None;
        let gap = runtime
            .step_action_gap()
            .map_err(ContainedTaskRunError::operation::<R>)?;
        if !gap.is_zero() {
            self.wait_pre_input(runtime, gap, pace, run.timing.deadline())?;
            let (frame, _) = self.linear_observe_before_input(
                runtime,
                run,
                &observation.page_label,
                &mut retries,
            )?;
            step_timing = run
                .timing
                .with_deadline(budget_observation_now(runtime, pace) + window);
            again = Some(frame);
        }
        let input = loop {
            let frame = again.as_ref().unwrap_or(observation);
            let page = frame.page_label.clone();
            let selected = match self.select_attempt(
                runtime,
                operation,
                frame,
                step_index,
                step_timing,
                (&retries, run.step_timeout),
            ) {
                Ok(selection::SelectAttempt::Selected(selected)) => *selected,
                Ok(selection::SelectAttempt::Empty {
                    outcome_key,
                    page_label,
                }) => {
                    return Ok(Some(LinearEmptyEnd {
                        page_label,
                        outcome_key,
                    }));
                }
                // A scan's failure may follow its own moves: it is never observed again.
                Err(failure) if operation.scan_max_ms().is_some() => return Err(failure),
                Err(failure) => {
                    retries.retry(runtime, failure, run.step_timeout)?;
                    let (frame, timing) =
                        self.linear_observe_before_input(runtime, run, &page, &mut retries)?;
                    again = Some(frame);
                    step_timing = timing;
                    continue;
                }
            };
            // Workflow #308 SEL-L4b: a scan drew the step's seed for its moves; its tap reuses
            // it. Workflow #406 T12: with the touch keys in effect.
            let draw = match selected.draw {
                Some(draw) => draw,
                None => step_point_draw(runtime, step_index, &operation.id)?,
            };
            let (action, sampling) = selected.draw_input::<R::Error>(
                &self.control.resolution,
                draw,
                &mut runtime_touch_choice(runtime),
            )?;
            if sampled
                && budget_observation_now(runtime, step_timing.pace()) >= step_timing.deadline()
            {
                let timeout = ContainedTaskError::new(
                    ExecutionKernelCode::RecognitionSampleDeadlineExceeded.code(),
                );
                retries.retry(
                    runtime,
                    ContainedTaskRunError::<R::Error>::Task(timeout),
                    run.step_timeout,
                )?;
                let (frame, timing) =
                    self.linear_observe_before_input(runtime, run, &page, &mut retries)?;
                again = Some(frame);
                step_timing = timing;
                continue;
            }
            break LinearInput {
                action,
                sampling,
                guard: selected.guard,
                input_context: selected.frame.input_context,
            };
        };
        self.linear_send_input(runtime, run, step_index, operation, input)?;
        Ok(None)
    }

    /// A step's one input once its decision held: the step's pre-input wait, the effect intent,
    /// the input bound to `input`'s frame context and the effect completion, in the order of the
    /// page-graph path.
    fn linear_send_input<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        run: LinearRun,
        step_index: u32,
        operation: &TaskOperation,
        input: LinearInput,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        let deadline = run.timing.deadline();
        let LinearInput {
            action,
            sampling,
            guard,
            input_context,
        } = input;
        // #385 P-k (performance model v1.4.2 §5.2): the step's pre-input wait, after the action
        // and its sampling check, before the effect intent; nothing is recognized again after it.
        self.wait_pre_input(
            runtime,
            run.pacing.pre_input_wait,
            run.timing.pace(),
            deadline,
        )?;
        runtime
            .record(ContainedTaskTrace::EffectIntent {
                step_index,
                operation_label: operation.id.clone(),
                action: action.clone(),
                sampling,
                guard,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        if budget_now(runtime, run.timing.pace()) >= deadline {
            return Err(self
                .task_timeout_error(
                    runtime,
                    TaskTimingStage::BeforeInput,
                    deadline,
                    None,
                    run.timing.pace(),
                )
                .into());
        }
        // STORE-FG-1 (design §2.7): a declared store step's tap goes to the store; every other
        // input as before.
        let sent = match self.store_input_target(run.store_routing, &operation.id, &action) {
            Some(store) => runtime.store_input(action, input_context, store),
            None => runtime.input(action, input_context),
        };
        sent.map_err(ContainedTaskRunError::operation::<R>)?;
        Self::linear_effect_completed(runtime, run, step_index, operation)
    }

    /// Workflow #386 SR-1 (scheduling model §2.1 items 1-2): the guard's observation of a step
    /// whose page is `page`, made again in a new window of the step timeout: a fresh frame on
    /// which only `page` is evaluated. A frame on which it does not pass is
    /// `contained_task_page_unknown`; a sampling timeout uses up another of `retries`.
    fn linear_observe_before_input<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        run: LinearRun,
        page: &str,
        retries: &mut SampleRetries,
    ) -> Result<(PageObservation, ContainedTaskTimingContext), ContainedTaskRunError<R::Error>>
    {
        loop {
            let step_timing = run.timing.with_deadline(
                budget_observation_now(runtime, run.timing.pace()) + run.step_timeout,
            );
            match self.linear_observe_scene(runtime, &[page], step_timing, None) {
                Ok(Ok(observation)) => return Ok((observation, step_timing)),
                Ok(Err(_)) => {
                    return Err(ContainedTaskError::new(
                        SharedCode::ContainedTaskPageUnknown.code(),
                    )
                    .into());
                }
                Err(failure) => retries.retry(runtime, failure, run.step_timeout)?,
            }
        }
    }

    /// An application step's effect (§5.3 b–d): no guard, no effect intent and no input; the
    /// run's application lifecycle path records the `application.*` chain itself, as on the
    /// page-graph path, and the effect completion follows.
    fn linear_application<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        run: LinearRun,
        step_index: u32,
        operation: &TaskOperation,
        action: ApplicationLifecycleAction,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        let deadline = run.timing.deadline();
        if budget_now(runtime, run.timing.pace()) >= deadline {
            return Err(self
                .task_timeout_error(
                    runtime,
                    TaskTimingStage::BeforeInput,
                    deadline,
                    None,
                    run.timing.pace(),
                )
                .into());
        }
        match runtime
            .control_application(action)
            .map_err(ContainedTaskRunError::operation::<R>)?
        {
            ApplicationEffectSupport::Performed => {}
            ApplicationEffectSupport::Unsupported => {
                return Err(ContainedTaskError::with_detail(
                    SharedCode::ApplicationEffectRequiresAssignedApplication.code(),
                    format!("operation={}", operation.id),
                )
                .into());
            }
        }
        Self::linear_effect_completed(runtime, run, step_index, operation)
    }

    /// `EffectCompleted` at the `EffectCompletedRecord` boundary, after a click or an
    /// application effect.
    fn linear_effect_completed<R: ContainedTaskRuntime>(
        runtime: &mut R,
        run: LinearRun,
        step_index: u32,
        operation: &TaskOperation,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        let boundary = TaskTimingBoundary::EffectCompletedRecord;
        let identity = runtime.task_boundary_identity(boundary);
        let effect_started = runtime.monotonic_now();
        let effected = runtime.record(ContainedTaskTrace::EffectCompleted {
            step_index,
            operation_label: operation.id.clone(),
        });
        let effect_ended = runtime.monotonic_now();
        runtime.observe_task_boundary(ContainedTaskBoundaryTiming {
            boundary,
            identity,
            context: run.timing,
            started: effect_started,
            ended: effect_ended,
            succeeded: effected.is_ok(),
        });
        effected.map_err(ContainedTaskRunError::Boundary)
    }

    /// The wait after an input: `post_delay_ms`, or the window's lower bound when larger.
    fn linear_post_input_delay<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        run: LinearRun,
        delay: Duration,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        let deadline = run.timing.deadline();
        // #385 P-k: the remaining time is budget time, the delay lasts delay × f.
        let pace = run.timing.pace();
        let boundary = TaskTimingBoundary::PostInputWait;
        let identity = runtime.task_boundary_identity(boundary);
        let wait_started = runtime.monotonic_now();
        let waited = if delay.is_zero() {
            Ok(())
        } else if deadline
            .checked_duration_since(budget_now(runtime, pace))
            .is_none_or(|remaining| delay >= remaining)
        {
            Err(self.task_timeout_error(
                runtime,
                TaskTimingStage::PostInputDelay,
                deadline,
                Some(delay),
                pace,
            ))
        } else {
            pace_pause(runtime, pace, delay);
            if budget_now(runtime, pace) >= deadline {
                Err(self.task_timeout_error(
                    runtime,
                    TaskTimingStage::PostInputDelay,
                    deadline,
                    None,
                    pace,
                ))
            } else {
                Ok(())
            }
        };
        let wait_ended = runtime.monotonic_now();
        runtime.observe_task_boundary(ContainedTaskBoundaryTiming {
            boundary,
            identity,
            context: run.timing,
            started: wait_started,
            ended: wait_ended,
            succeeded: waited.is_ok(),
        });
        waited.map_err(Into::into)
    }

    /// One attempt after its post-input wait (§5.4): the intermediate state, then the next
    /// step's page, or the candidates of a run of optional operations (Workflow #339). Workflow
    /// #407 LOAD-4: every wait of the attempt spends `loading`, its allowance.
    fn linear_after_input<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        run: LinearRun,
        step: &LinearStep,
        candidates: &LinearCandidates<'_>,
        arrival: LinearArrival,
        loading: &mut LoadingAllowance,
    ) -> LinearAttemptResult<R::Error> {
        match &step.transition {
            None => self.linear_arrival(
                runtime,
                (run, loading),
                candidates,
                arrival.budget,
                arrival.interval,
                LinearMissKind::Arrival,
            ),
            Some(LinearTransition::Page {
                page,
                timeout,
                interval,
            }) => {
                let started = budget_now(runtime, run.timing.pace());
                match self.linear_wait(
                    runtime,
                    &[page.as_str()],
                    *timeout,
                    *interval,
                    LinearWaitPurpose::AfterInput,
                    (run, &mut *loading),
                )? {
                    // The next step's arrival budget starts at the frame that showed the page.
                    Ok(_) => self.linear_arrival(
                        runtime,
                        (run, loading),
                        candidates,
                        arrival.budget,
                        arrival.interval,
                        LinearMissKind::AfterIntermediate,
                    ),
                    Err(loading_limit) => Ok(Err(LinearMiss {
                        kind: LinearMissKind::Intermediate,
                        elapsed: budget_elapsed_since(runtime, run.timing.pace(), started),
                        limit: *timeout,
                        skip_target_seen: false,
                        loading_limit,
                    })),
                }
            }
            Some(LinearTransition::Window { max, .. }) => self.linear_arrival(
                runtime,
                (run, loading),
                candidates,
                max.saturating_sub(arrival.post_input_delay)
                    .saturating_add(arrival.budget),
                arrival.interval,
                LinearMissKind::Arrival,
            ),
        }
    }

    /// The wait for the next step's page; several candidates (Workflow #339) are awaited by
    /// `linear_wait_set`. Workflow #407 LOAD-4: it spends `loading`, the attempt's allowance.
    fn linear_arrival<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        (run, loading): (LinearRun, &mut LoadingAllowance),
        candidates: &LinearCandidates<'_>,
        budget: Duration,
        interval: Duration,
        miss: LinearMissKind,
    ) -> LinearAttemptResult<R::Error> {
        let &[page] = candidates.pages.as_slice() else {
            return self.linear_wait_set(
                runtime,
                (run, loading),
                candidates,
                budget,
                interval,
                miss,
                None,
            );
        };
        let started = budget_now(runtime, run.timing.pace());
        let waited = self.linear_wait(
            runtime,
            &[page],
            budget,
            interval,
            LinearWaitPurpose::AfterInput,
            (run, loading),
        )?;
        Ok(waited.map_err(|loading_limit| LinearMiss {
            kind: miss,
            elapsed: budget_elapsed_since(runtime, run.timing.pace(), started),
            limit: budget,
            skip_target_seen: false,
            loading_limit,
        }))
    }

    /// Workflow #339 §3.4: the wait for several candidates, the optional pages of a run first
    /// and its skip target last. An optional page that passes ends the wait. The skip target
    /// ends it only on a frame captured at least the settle after the capture that first
    /// showed it, so that an optional page arriving late still runs; frames on which nothing
    /// passes are ignored. The wait gives up `budget` after it starts, or, once the skip target
    /// was seen, the settle plus `budget` after that capture. `seen` is a capture that already
    /// showed the skip target (the retry decision's frame). Workflow #407 LOAD-4 (§5): a
    /// transition frame is no candidate; it pauses the wait, the settle and the first sighting of
    /// the skip target within `loading`, the attempt's allowance, and one after the allowance is
    /// spent ends the wait.
    #[allow(clippy::too_many_arguments)]
    fn linear_wait_set<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        (run, loading): (LinearRun, &mut LoadingAllowance),
        candidates: &LinearCandidates<'_>,
        budget: Duration,
        interval: Duration,
        miss: LinearMissKind,
        mut seen: Option<Instant>,
    ) -> LinearAttemptResult<R::Error> {
        let purpose = LinearWaitPurpose::AfterInput;
        let deadline = run.timing.deadline();
        let game = self.control.game.as_str();
        // Workflow #428 DIAG-SI: a wait set always holds its skip target.
        let skip_target = *candidates
            .pages
            .last()
            .ok_or_else(|| invariant_broken(|outcome| outcome))?;
        let settle = candidates.settle;
        // #385 P-k (performance model v1.4.2 §5.1, §5.3): the wait's budget and its settle are
        // budget time; `seen` is a budget instant.
        let pace = run.timing.pace();
        let mut retries =
            SampleRetries::new(run.pacing, ExecutionKernelLocation::ObservationAfterInput);
        // #385 P-k (§5.4 item 1): a wait that gave up starts the step's extra rounds first, each
        // `budget` from where the last one ended; a skip target already seen stays seen.
        let mut rounds = ExtraRounds::of(run.pacing);
        let mut round_end: Option<Instant> = None;
        // PERF-B1 (R3): the pause before the wait's next frame, lengthened over unchanged frames.
        let mut backoff = CaptureBackoff::new(interval, run.pacing);
        let wait_end = |started: Instant, seen: Option<Instant>, round_end: Option<Instant>| {
            let end = seen.map_or(started + budget, |first| first + settle + budget);
            round_end.map_or(end, |round_end| end.max(round_end))
        };
        let mut started = budget_now(runtime, pace);
        // Workflow #407 LOAD-4: the budget instant of the wait's latest frame, its start before
        // the first.
        let mut last_frame_at = started;
        loop {
            if budget_now(runtime, pace) >= deadline {
                return Err(self
                    .linear_task_timeout(
                        runtime,
                        purpose,
                        deadline,
                        TaskTimingCheckPosition::PostconditionBeforeCapture,
                        pace,
                    )
                    .into());
            }
            let observation_deadline = wait_end(started, seen, round_end);
            let timing = if self.evaluator.pack().target_consensus.is_empty() {
                run.timing
            } else {
                run.timing.with_deadline(observation_deadline)
            };
            let captured = budget_now(runtime, pace);
            let observation = match self.linear_observe_scene(
                runtime,
                &candidates.pages,
                timing,
                Some(&mut *loading),
            ) {
                Ok(observation) => observation,
                Err(failure) => {
                    // Workflow #386 SR-1 (scheduling model §2.1 item 1): a sampling timeout
                    // starts the wait over in a new window, without any input.
                    retries.retry(runtime, failure, budget)?;
                    started = budget_now(runtime, pace);
                    seen = None;
                    round_end = None;
                    backoff.reset();
                    continue;
                }
            };
            if budget_now(runtime, pace) >= deadline {
                return Err(self
                    .linear_task_timeout(
                        runtime,
                        purpose,
                        deadline,
                        TaskTimingCheckPosition::PostconditionAfterCapture,
                        pace,
                    )
                    .into());
            }
            // Workflow #407 LOAD-4 (§5): LOAD-3's one judgement, any candidate that passes being
            // what the wait waits for; the frame spends the allowance as it says.
            let frame_at = budget_now(runtime, pace);
            let previous_frame_at = std::mem::replace(&mut last_frame_at, frame_at);
            let judged = self.judge_frame(
                runtime,
                None,
                observation.as_ref().ok(),
                loading,
                run.timing,
            )?;
            let page = observation
                .as_ref()
                .ok()
                .map(|observation| observation.page_label.clone());
            let at = LoadingFrame {
                transition: judged.transition,
                page: page.as_deref(),
                now: frame_at,
                previous: previous_frame_at,
            };
            match observation {
                // A transition frame (`on`) is offered to the shared interrupt pages first; a
                // handled one pauses the wait as below and spends nothing. Otherwise it spends the
                // allowance and pauses the wait, the settle and the skip target's first sighting
                // with it, as a handled interrupt does, or ends the wait once the allowance is
                // spent. It is no candidate, so never the skip target's first sighting.
                observation if judged.row == FrameRow::Transition => {
                    let scene = match &observation {
                        Ok(observation) => &observation.scene,
                        Err(scene) => scene,
                    };
                    let span = match self.offer_shared_interrupt(runtime, scene)? {
                        Some(paused) => {
                            let paused = pace.budget_span(paused);
                            started += paused;
                            if let Some(first) = &mut seen {
                                *first += paused;
                            }
                            if let Some(end) = &mut round_end {
                                *end += paused;
                            }
                            backoff.reset();
                            last_frame_at += paused;
                            continue;
                        }
                        None => match loading.frame(runtime, game, at)? {
                            Charge::Spent(limit) => {
                                return Ok(Err(LinearMiss {
                                    kind: miss,
                                    elapsed: budget_elapsed_since(runtime, pace, started),
                                    limit: wait_end(started, seen, round_end)
                                        .saturating_duration_since(started),
                                    skip_target_seen: seen.is_some(),
                                    loading_limit: Some(limit),
                                }));
                            }
                            Charge::Paused(span) => span,
                            Charge::None => Duration::ZERO,
                        },
                    };
                    started += span;
                    if let Some(first) = &mut seen {
                        *first += span;
                    }
                    if let Some(end) = &mut round_end {
                        *end += span;
                    }
                    backoff.held_frame(scene)?;
                }
                Ok(observation) => {
                    loading.frame(runtime, game, at)?;
                    let captured = if observation.sampling.is_some() {
                        pace.budget(observation.frame_started)
                    } else {
                        captured
                    };
                    if observation.page_label != skip_target || settle.is_zero() {
                        return Ok(Ok(observation));
                    }
                    match seen {
                        Some(first) if captured.saturating_duration_since(first) >= settle => {
                            return Ok(Ok(observation));
                        }
                        Some(_) => {}
                        None => seen = Some(captured),
                    }
                    // PERF-B1 (R3): the skip target while it settles.
                    backoff.frame(&observation.scene)?;
                }
                // Workflow #373 NAV-6 (§6.1 point 3): nothing passed on the frame; it is offered
                // to the shared interrupt pages, and the wait (its settle included) is paused
                // while one is handled.
                Err(scene) => {
                    loading.frame(runtime, game, at)?;
                    // PERF-B1 (R3).
                    backoff.frame(&scene)?;
                    if let Some(paused) = self.offer_shared_interrupt(runtime, &scene)? {
                        let paused = pace.budget_span(paused);
                        started += paused;
                        if let Some(first) = &mut seen {
                            *first += paused;
                        }
                        if let Some(end) = &mut round_end {
                            *end += paused;
                        }
                        backoff.reset();
                        last_frame_at += paused;
                        continue;
                    }
                }
            }
            let mut limit = wait_end(started, seen, round_end).saturating_duration_since(started);
            let elapsed = budget_elapsed_since(runtime, pace, started);
            if elapsed >= limit && rounds.another(runtime, Some((pace, deadline))) {
                // The next round keeps the interval: its first frame comes one interval on.
                round_end = Some(started + elapsed + budget);
                limit = elapsed + budget;
            }
            if elapsed >= limit {
                return Ok(Err(LinearMiss {
                    kind: miss,
                    elapsed,
                    limit,
                    skip_target_seen: seen.is_some(),
                    loading_limit: None,
                }));
            }
            // The settle bounds the sleep only while it lasts; after it, captures keep the
            // interval and never run back to back.
            let now = budget_now(runtime, pace);
            let settle_left = seen
                .and_then(|first| first.checked_add(settle))
                .and_then(|end| end.checked_duration_since(now))
                .filter(|left| !left.is_zero());
            // PERF-B1 (R3): the back-off's pause, cut at the wait's and the task's time left.
            let sleep = backoff
                .interval()
                .min(limit.saturating_sub(elapsed))
                .min(deadline.saturating_duration_since(now));
            let boundary = purpose.boundary();
            let identity = runtime.task_boundary_identity(boundary);
            let wait_started = runtime.monotonic_now();
            pace_pause(
                runtime,
                pace,
                settle_left.map_or(sleep, |left| sleep.min(left)),
            );
            let wait_ended = runtime.monotonic_now();
            runtime.observe_task_boundary(ContainedTaskBoundaryTiming {
                boundary,
                identity,
                context: run.timing,
                started: wait_started,
                ended: wait_ended,
                succeeded: true,
            });
        }
    }

    fn linear_retry_delay<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        run: LinearRun,
        delay: Duration,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        let deadline = run.timing.deadline();
        // #385 P-k: the remaining time is budget time, the delay lasts delay × f.
        let pace = run.timing.pace();
        if deadline
            .checked_duration_since(budget_now(runtime, pace))
            .is_none_or(|remaining| delay > remaining)
        {
            return Err(self
                .task_timeout_error(
                    runtime,
                    TaskTimingStage::RetryDelay,
                    deadline,
                    Some(delay),
                    pace,
                )
                .into());
        }
        let boundary = TaskTimingBoundary::RetryWait;
        let identity = runtime.task_boundary_identity(boundary);
        let wait_started = runtime.monotonic_now();
        pace_pause(runtime, pace, delay);
        let wait_ended = runtime.monotonic_now();
        runtime.observe_task_boundary(ContainedTaskBoundaryTiming {
            boundary,
            identity,
            context: run.timing,
            started: wait_started,
            ended: wait_ended,
            succeeded: true,
        });
        Ok(())
    }

    fn linear_step_finished<R: ContainedTaskRuntime>(
        runtime: &mut R,
        step_index: u32,
        operation: &TaskOperation,
        page_label: &str,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        runtime
            .record(ContainedTaskTrace::StepFinished {
                step_index,
                operation_label: operation.id.clone(),
                page_label: page_label.to_owned(),
                phase: None,
            })
            .map_err(ContainedTaskRunError::Boundary)
    }

    /// Captures until one of `candidates` passes, at most `budget` after the first capture;
    /// `Err(None)` once the budget is spent. A task deadline inside the wait fails the task.
    /// Workflow #386 SR-1 (scheduling model §2.1): an observation whose sampling timed out is
    /// made again in a new window of `budget`, as the run's pacing allows. #385 P-k (§5.4 item
    /// 1): a spent budget starts the step's extra rounds first, one `budget` each, captures only;
    /// the retry decision, whose frame decides whether the step inputs again, takes none.
    /// Workflow #407 LOAD-4 (§5): each frame is judged by LOAD-3's one judgement against
    /// `loading`; a transition frame (`on`) is no candidate, is offered to the shared interrupt
    /// pages first and otherwise spends the allowance and pauses the wait (its budget, its window,
    /// an extra round's end), and one after the allowance is spent ends the wait, `Err` with the
    /// loading values.
    fn linear_wait<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        candidates: &[&str],
        budget: Duration,
        interval: Duration,
        purpose: LinearWaitPurpose,
        (run, loading): (LinearRun, &mut LoadingAllowance),
    ) -> LinearWaitResult<R::Error> {
        let deadline = run.timing.deadline();
        let base_timing = run.timing;
        let game = self.control.game.as_str();
        // #385 P-k (performance model v1.4.2 §5.3): the wait's limits are budget time.
        let pace = run.timing.pace();
        let mut retries = SampleRetries::new(run.pacing, purpose.observation());
        let mut rounds = match purpose {
            LinearWaitPurpose::RetryDecision => ExtraRounds::NONE,
            LinearWaitPurpose::Entry | LinearWaitPurpose::AfterInput => ExtraRounds::of(run.pacing),
        };
        let mut started = budget_now(runtime, pace);
        let mut window_started = budget_observation_now(runtime, pace);
        // Workflow #407 LOAD-4: the budget instant of the wait's latest frame, its start before
        // the first.
        let mut last_frame_at = started;
        // Whether the window has seen a frame on which no awaited page passed.
        let mut missed = false;
        // PERF-B1 (R3): the pause before the wait's next frame, lengthened over unchanged frames.
        let mut backoff = CaptureBackoff::new(interval, run.pacing);
        loop {
            let timing = if self.evaluator.pack().target_consensus.is_empty() {
                base_timing
            } else {
                base_timing.with_deadline(window_started + budget)
            };
            if budget_now(runtime, pace) >= deadline {
                return Err(self
                    .linear_task_timeout(
                        runtime,
                        purpose,
                        deadline,
                        TaskTimingCheckPosition::PostconditionBeforeCapture,
                        pace,
                    )
                    .into());
            }
            // Test plan E-3 G3 (exec key 21) on the linear path (wave-17 acceptance ruling 2):
            // after frames on which no awaited page passed, a window too short for the next
            // sampled frame is spent, as an elapsed one is.
            if missed
                && self.consensus_window_spent(
                    runtime,
                    Some(candidates),
                    window_started + budget,
                    deadline,
                    pace,
                )?
            {
                if !rounds.another(runtime, Some((pace, deadline))) {
                    return Ok(Err(None));
                }
                started = budget_now(runtime, pace);
                window_started = budget_observation_now(runtime, pace);
                missed = false;
                continue;
            }
            let observation =
                match self.linear_observe_scene(runtime, candidates, timing, Some(&mut *loading)) {
                    Ok(observation) => observation,
                    // #402 HS-5d (register §1.279): after an input or an application effect, an
                    // observation whose frames did not keep one geometry (a paired Nemu
                    // session's display that moved from the main display to the application's
                    // between them, HS-5a D3) ends as a frame on which no awaited page passed,
                    // and the wait goes on in its budget: its check and the pause before the next
                    // observation, as after any such frame.
                    Err(failure)
                        if matches!(purpose, LinearWaitPurpose::AfterInput)
                            && is_geometry_change(&failure) =>
                    {
                        missed = true;
                        backoff.reset();
                        let now = budget_now(runtime, pace);
                        if now.saturating_duration_since(started) >= budget {
                            if !rounds.another(runtime, Some((pace, deadline))) {
                                return Ok(Err(None));
                            }
                            started = now;
                            window_started = budget_observation_now(runtime, pace);
                            missed = false;
                        }
                        let boundary = purpose.boundary();
                        let identity = runtime.task_boundary_identity(boundary);
                        let wait_started = runtime.monotonic_now();
                        let now = pace.budget(wait_started);
                        pace_pause(
                            runtime,
                            pace,
                            backoff
                                .interval()
                                .min(budget.saturating_sub(now.saturating_duration_since(started)))
                                .min(deadline.saturating_duration_since(now)),
                        );
                        let wait_ended = runtime.monotonic_now();
                        runtime.observe_task_boundary(ContainedTaskBoundaryTiming {
                            boundary,
                            identity,
                            context: timing,
                            started: wait_started,
                            ended: wait_ended,
                            succeeded: true,
                        });
                        continue;
                    }
                    Err(failure) => {
                        retries.retry(runtime, failure, budget)?;
                        started = budget_now(runtime, pace);
                        window_started = budget_observation_now(runtime, pace);
                        missed = false;
                        backoff.reset();
                        continue;
                    }
                };
            if budget_now(runtime, pace) >= deadline {
                return Err(self
                    .linear_task_timeout(
                        runtime,
                        purpose,
                        deadline,
                        TaskTimingCheckPosition::PostconditionAfterCapture,
                        pace,
                    )
                    .into());
            }
            // Workflow #407 LOAD-4 (§5): LOAD-3's one judgement, any candidate that passes being
            // what the wait waits for; the frame spends the allowance as it says.
            let frame_at = budget_now(runtime, pace);
            let previous_frame_at = std::mem::replace(&mut last_frame_at, frame_at);
            let judged = self.judge_frame(
                runtime,
                None,
                observation.as_ref().ok(),
                loading,
                base_timing,
            )?;
            let page = observation
                .as_ref()
                .ok()
                .map(|observation| observation.page_label.clone());
            let at = LoadingFrame {
                transition: judged.transition,
                page: page.as_deref(),
                now: frame_at,
                previous: previous_frame_at,
            };
            let scene = match observation {
                Ok(observation) if judged.row != FrameRow::Transition => {
                    loading.frame(runtime, game, at)?;
                    return Ok(Ok(observation));
                }
                Ok(observation) => observation.scene,
                Err(scene) => scene,
            };
            missed = true;
            if judged.row == FrameRow::Transition {
                // A transition frame (`on`) is offered to the shared interrupt pages first; a
                // handled one pauses the wait as below and spends nothing. Otherwise it spends the
                // allowance and pauses the wait, or ends it once the allowance is spent.
                match self.offer_shared_interrupt(runtime, &scene)? {
                    Some(paused) => {
                        let paused = pace.budget_span(paused);
                        started += paused;
                        window_started += paused;
                        backoff.reset();
                        last_frame_at += paused;
                        continue;
                    }
                    None => match loading.frame(runtime, game, at)? {
                        Charge::Spent(limit) => return Ok(Err(Some(limit))),
                        Charge::Paused(span) => {
                            started += span;
                            window_started += span;
                        }
                        Charge::None => {}
                    },
                }
                backoff.held_frame(&scene)?;
            } else {
                loading.frame(runtime, game, at)?;
                // PERF-B1 (R3).
                backoff.frame(&scene)?;
                // Workflow #373 NAV-6 (§6.1 point 3): a frame on which no awaited page passed is
                // offered to the shared interrupt pages; the wait's budget is paused while one is
                // handled.
                if let Some(paused) = self.offer_shared_interrupt(runtime, &scene)? {
                    let paused = pace.budget_span(paused);
                    started += paused;
                    window_started += paused;
                    backoff.reset();
                    last_frame_at += paused;
                    continue;
                }
            }
            let now = budget_now(runtime, pace);
            if now.saturating_duration_since(started) >= budget {
                // The next round keeps the interval: its first frame comes one interval on.
                if !rounds.another(runtime, Some((pace, deadline))) {
                    return Ok(Err(None));
                }
                started = now;
                window_started = budget_observation_now(runtime, pace);
                missed = false;
            }
            let boundary = purpose.boundary();
            let identity = runtime.task_boundary_identity(boundary);
            let wait_started = runtime.monotonic_now();
            let now = pace.budget(wait_started);
            // PERF-B1 (R3): the back-off's pause, cut at the wait's and the task's time left.
            pace_pause(
                runtime,
                pace,
                backoff
                    .interval()
                    .min(budget.saturating_sub(now.saturating_duration_since(started)))
                    .min(deadline.saturating_duration_since(now)),
            );
            let wait_ended = runtime.monotonic_now();
            runtime.observe_task_boundary(ContainedTaskBoundaryTiming {
                boundary,
                identity,
                context: timing,
                started: wait_started,
                ended: wait_ended,
                succeeded: true,
            });
        }
    }

    fn linear_task_timeout<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        purpose: LinearWaitPurpose,
        deadline: Instant,
        position: TaskTimingCheckPosition,
        pace: PaceClock,
    ) -> ContainedTaskError {
        let error = self.task_timeout_error(runtime, purpose.timeout_stage(), deadline, None, pace);
        match purpose {
            LinearWaitPurpose::AfterInput => error.with_timing_check_position(position),
            LinearWaitPurpose::Entry | LinearWaitPurpose::RetryDecision => error,
        }
    }

    /// One capture and the evaluation of `candidates` only, recorded as every capture is, at
    /// the `CapturePage` boundary (§5.3). The first passing candidate is the frame's page; with
    /// the frame's input reference the committed input context binds a following input.
    /// Workflow #407 LOAD-4: with `loading`, the frame's scene is also evaluated against its pages
    /// once its candidates' recognition is recorded (LOAD-3's `probe_loading`).
    fn linear_observe_scene<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        candidates: &[&str],
        timing: ContainedTaskTimingContext,
        loading: Option<&mut LoadingAllowance>,
    ) -> Result<Result<PageObservation, Scene>, ContainedTaskRunError<R::Error>> {
        self.capture_frame_gated(
            runtime,
            None,
            None,
            timing,
            Some(PageCandidates::Ordered(candidates)),
            None,
            None,
            loading,
        )
    }
}
