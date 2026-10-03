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

use super::{
    ApplicationEffectSupport, ContainedTaskBoundaryTiming, ContainedTaskError,
    ContainedTaskOutcome, ContainedTaskRunError, ContainedTaskRuntime, ContainedTaskTimingContext,
    ContainedTaskTrace, DEFAULT_TASK_TIMEOUT_MS, MAX_CAPTURE_INTERVAL_MS, MAX_STEP_TIMEOUT_MS,
    MAX_STEPS, MAX_TASK_TIMEOUT_MS, PageObservation, PostAdmissionOcrCollector,
    PreparedContainedTask, TaskControl, TaskOperation, TaskProgram, resolve_page_reference,
    selection,
};
use crate::RunOperationPolicy;
use actingcommand_contract::{
    ApplicationLifecycleAction, PHASED_CONTROL_SCHEMA, TaskTimingBoundary, TaskTimingCheckPosition,
    TaskTimingFailure, TaskTimingScope, TaskTimingStage,
};
use actingcommand_page_detector::PageDetector;
use actingcommand_recognition_pack::RecognitionEvaluator;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::thread;
use std::time::{Duration, Instant};

/// The control `execution_mode` of a linear task.
pub(super) const LINEAR_STEPS: &str = "linear_steps";
const LINEAR_INVALID: &str = "contained_task_linear_invalid";
const ENTRY_UNMATCHED: &str = "contained_task_linear_entry_unmatched";
const INTERMEDIATE_UNOBSERVED: &str = "contained_task_linear_intermediate_unobserved";
const APPLICATION_UNCONFIRMED: &str = "contained_task_linear_application_unconfirmed";
const PAGE_CONFIRMATION_FAILED: &str = "page_confirmation_failed";
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
        let check = |pages: &[&str], limit_ms: u64, phase: &str| {
            let required_ms =
                selection::page_recognition_budget_ms(evaluator, detector, Some(pages))?;
            let limit_ms = limit_ms.min(control.task_timeout().milliseconds);
            if required_ms > limit_ms {
                return Err(ContainedTaskError::with_detail(
                    "recognition_sample_budget_insufficient",
                    format!(
                        "phase={phase} pages={} requires {required_ms}ms across page calls and waits; limit={limit_ms}ms",
                        pages.join(",")
                    ),
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
            check(&candidates.pages, arrival_ms, "arrival")?;
            if let Some(LinearTransition::Page { page, timeout, .. }) = &step.transition {
                check(&[page.as_str()], timeout.as_millis() as u64, "transition")?;
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
                    )?;
                }
                let mut retry_pages = match &step.transition {
                    Some(LinearTransition::Page { page, .. }) => vec![page.as_str()],
                    _ => candidates.pages,
                };
                retry_pages.push(page.as_str());
                check(&retry_pages, control.step_timeout().milliseconds, "retry")?;
            }
        }
        Ok(())
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

/// A step's one effect: a click, or an application lifecycle action (Workflow #336 R24).
#[derive(Clone, Copy)]
enum LinearEffect {
    Click,
    Application(ApplicationLifecycleAction),
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
    pub(super) timing: ContainedTaskTimingContext,
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
}

/// The budgets of the wait for the next step's page.
#[derive(Clone, Copy)]
struct LinearArrival {
    budget: Duration,
    interval: Duration,
    post_input_delay: Duration,
}

type LinearAttemptResult<E> = Result<Result<PageObservation, LinearMiss>, ContainedTaskRunError<E>>;

fn postcondition_timing(elapsed: Duration, limit: Duration) -> Option<TaskTimingFailure> {
    Some(TaskTimingFailure {
        scope: TaskTimingScope::Postcondition,
        stage: TaskTimingStage::Postcondition,
        elapsed_ms: elapsed.as_millis() as u64,
        limit_ms: limit.as_millis() as u64,
        required_delay_ms: None,
    })
}

/// `awaited` is the suffix of `LinearCandidates::awaited` (Workflow #339), empty for one page.
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
        format!(
            "operation={} attempts={attempt} after_page={UNRECOGNIZED_PAGE} hit_error_page=false {}{}{awaited}",
            operation.id,
            step.transition_detail(),
            if intermediate_seen {
                " intermediate_seen=true"
            } else {
                ""
            }
        ),
    )
    .with_timing(postcondition_timing(elapsed, limit))
}

/// An application step whose next page did not pass (§5.3 h): never retried.
fn application_unconfirmed(
    operation: &TaskOperation,
    step: &LinearStep,
    action: ApplicationLifecycleAction,
    miss: &LinearMiss,
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
        if self.schema_version != "0.9" || control.schema_version != PHASED_CONTROL_SCHEMA {
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
            ("resource_readings", self.resource_readings.is_some()),
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
        let entry = match self
            .entry_page
            .as_deref()
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
            operation.validate(control, self.defaults, &self.schema_version)?;
            if !operation_ids.insert(operation.id.as_str()) {
                return Err(ContainedTaskError::new("contained_task_program_invalid"));
            }
            if operation.select.is_some() || operation.on_error.is_some() {
                return Err(invalid("operation_effect", Some(operation)));
            }
            let effect = match (&operation.click, &operation.application) {
                (Some(_), None) => LinearEffect::Click,
                (None, Some(application)) => LinearEffect::Application(application.action),
                _ => return Err(invalid("operation_effect", Some(operation))),
            };
            if matches!(effect, LinearEffect::Application(_))
                && (operation.retryable.is_some()
                    || operation.max_attempts.is_some()
                    || operation.retry_interval_ms.is_some())
            {
                return Err(invalid("application_retry", Some(operation)));
            }
            let from = if operation.from == ANY_PAGE {
                if index != 0 {
                    return Err(invalid("any_from", Some(operation)));
                }
                if matches!(effect, LinearEffect::Click) {
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
                    if interval_ms
                        .is_some_and(|value| value == 0 || value > MAX_CAPTURE_INTERVAL_MS)
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
                Some(optional) if optional.settle_ms > MAX_STEP_TIMEOUT_MS => {
                    return Err(invalid("optional_settle", Some(operation)));
                }
                Some(optional) => Some(Duration::from_millis(optional.settle_ms)),
            };
            steps.push(LinearStep {
                from,
                to,
                effect,
                transition,
                optional,
                run: None,
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
        if let Some(declaration) = &self.scheduling_outcome {
            declaration.validate().map_err(|_| {
                ContainedTaskError::new("contained_task_outcome_declaration_invalid")
            })?;
            if declaration.designated_operation().is_some() {
                return Err(invalid("designated_operation", None));
            }
            for page in declaration
                .mappings()
                .iter()
                .flat_map(|mapping| mapping.terminal_pages())
            {
                if resolve(page.as_str())? != target {
                    return Err(invalid("terminal_page", None));
                }
            }
        }
        Ok(LinearPlan {
            target,
            steps,
            runs,
        })
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
        run: LinearRun,
    ) -> Result<ContainedTaskOutcome, ContainedTaskRunError<R::Error>> {
        let first = plan
            .steps
            .first()
            .ok_or_else(|| ContainedTaskError::new("contained_task_state_invalid"))?;
        // An application entry captures nothing before its effect (R24): no frame, no wait.
        let mut observation = match &first.from {
            LinearFrom::Any => None,
            LinearFrom::Page(page) => {
                let entry_started = Instant::now();
                let observed = self
                    .linear_wait(
                        runtime,
                        &[page.as_str()],
                        run.step_timeout,
                        run.capture_interval,
                        LinearWaitPurpose::Entry,
                        run.timing,
                    )?
                    .ok_or_else(|| {
                        ContainedTaskError::with_detail(ENTRY_UNMATCHED, format!("page={page}"))
                            .with_timing(Some(TaskTimingFailure {
                                scope: TaskTimingScope::PageRecognition,
                                stage: TaskTimingStage::EntryRecognition,
                                elapsed_ms: entry_started.elapsed().as_millis() as u64,
                                limit_ms: run.step_timeout.as_millis() as u64,
                                required_delay_ms: None,
                            }))
                    })?;
                Some(observed)
            }
        };
        // Workflow #339 §3.3: a cursor over the operations. `dispatched` counts the operations
        // run, as the page-graph path counts its steps, and is the next `step_index`; a skipped
        // optional operation writes nothing. The next operation follows from the page that
        // passed, never from a comparison with the target page. Without an optional operation
        // every operation runs in order and `dispatched` is the operation index.
        let state_invalid = || ContainedTaskError::new("contained_task_state_invalid");
        let mut current = 0;
        let mut dispatched = 0_u32;
        let mut handled = BTreeSet::new();
        loop {
            let (operation, step) = self
                .program
                .operations
                .get(current)
                .zip(plan.steps.get(current))
                .ok_or_else(state_invalid)?;
            let candidates = plan.candidates(current, &handled);
            let reached = self.run_linear_step(
                runtime,
                run,
                dispatched,
                operation,
                step,
                &candidates,
                observation,
            )?;
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
                        None => return Err(state_invalid().into()),
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
            observation.as_ref(),
            Some(plan.target.clone()),
            dispatched,
        )
    }

    /// One operation (§5.3 a–h, §5.5), from the frame on which its own page passed (none for the
    /// application entry) to the frame on which one of its `candidates` passed (Workflow #339:
    /// the next step's page, or a page of a run of optional operations).
    #[allow(clippy::too_many_arguments)]
    fn run_linear_step<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        run: LinearRun,
        step_index: u32,
        operation: &TaskOperation,
        step: &LinearStep,
        candidates: &LinearCandidates<'_>,
        mut observation: Option<PageObservation>,
    ) -> Result<PageObservation, ContainedTaskRunError<R::Error>> {
        let deadline = run.timing.deadline();
        let step_timing = run.timing.with_deadline(Instant::now() + run.step_timeout);
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
            if Instant::now() >= deadline {
                return Err(self
                    .task_timeout_error(TaskTimingStage::Dispatch, deadline, None)
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
                    let frame = observation
                        .as_ref()
                        .ok_or_else(|| ContainedTaskError::new("contained_task_page_unknown"))?;
                    self.linear_input(runtime, run, step_index, operation, frame, step_timing)?;
                }
                LinearEffect::Application(action) => {
                    self.linear_application(runtime, run, step_index, operation, action)?;
                }
            }
            self.linear_post_input_delay(runtime, run, arrival.post_input_delay)?;
            let miss = match self.linear_after_input(runtime, run, step, candidates, arrival)? {
                Ok(reached) => {
                    Self::linear_step_finished(
                        runtime,
                        step_index,
                        operation,
                        &reached.page_label,
                    )?;
                    return Ok(reached);
                }
                Err(miss) => miss,
            };
            let awaited = candidates.awaited(miss.skip_target_seen);
            if let LinearEffect::Application(action) = step.effect {
                // §5.3 h: an application step is never retried and has no retry decision.
                Self::linear_step_finished(runtime, step_index, operation, UNRECOGNIZED_PAGE)?;
                return Err(
                    application_unconfirmed(operation, step, action, &miss, &awaited).into(),
                );
            }
            // Workflow #339 §3.5: a seen skip target shows the input took effect; another input
            // would not help, as after a seen intermediate page.
            if miss.kind == LinearMissKind::AfterIntermediate
                || miss.skip_target_seen
                || attempt >= max_attempts
            {
                Self::linear_step_finished(runtime, step_index, operation, UNRECOGNIZED_PAGE)?;
                return Err(match (miss.kind, &step.transition) {
                    (LinearMissKind::Intermediate, Some(LinearTransition::Page { page, .. })) => {
                        ContainedTaskError::with_detail(
                            INTERMEDIATE_UNOBSERVED,
                            format!(
                                "operation={} attempts={attempt} intermediate_page={page}",
                                operation.id
                            ),
                        )
                        .with_timing(postcondition_timing(miss.elapsed, miss.limit))
                    }
                    (kind, _) => page_confirmation_failed(
                        operation,
                        step,
                        attempt,
                        kind == LinearMissKind::AfterIntermediate,
                        &awaited,
                        miss.elapsed,
                        miss.limit,
                    ),
                }
                .into());
            }
            // The retry decision (§5.5): the gate first, then this step's own page. The gate is
            // the intermediate page, or every candidate (Workflow #339 §3.5).
            let LinearFrom::Page(own_page) = &step.from else {
                return Err(ContainedTaskError::new("contained_task_state_invalid").into());
            };
            self.linear_retry_delay(runtime, run, retry_interval)?;
            let decision_started = Instant::now();
            let mut decision_pages = match &step.transition {
                Some(LinearTransition::Page { page, .. }) => vec![page.as_str()],
                _ => candidates.pages.clone(),
            };
            decision_pages.push(own_page.as_str());
            let decided = self.linear_wait(
                runtime,
                &decision_pages,
                run.step_timeout,
                run.capture_interval,
                LinearWaitPurpose::RetryDecision,
                run.timing,
            )?;
            match decided {
                Some(frame) if frame.page_label != *own_page => {
                    // The input took effect late: this attempt goes on, without another input.
                    let continued = match &step.transition {
                        Some(LinearTransition::Page { .. }) => self.linear_arrival(
                            runtime,
                            run,
                            candidates,
                            arrival.budget,
                            arrival.interval,
                            LinearMissKind::AfterIntermediate,
                        )?,
                        // Workflow #339: the skip target is watched from this frame on.
                        _ if candidates.settles_on(&frame.page_label) => self.linear_wait_set(
                            runtime,
                            run,
                            candidates,
                            arrival.budget,
                            arrival.interval,
                            LinearMissKind::Arrival,
                            Some(if frame.sampling.is_some() {
                                frame.frame_started
                            } else {
                                Instant::now()
                            }),
                        )?,
                        _ => {
                            Self::linear_step_finished(
                                runtime,
                                step_index,
                                operation,
                                &frame.page_label,
                            )?;
                            return Ok(frame);
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
                            return Ok(reached);
                        }
                        Err(miss) => {
                            Self::linear_step_finished(
                                runtime,
                                step_index,
                                operation,
                                UNRECOGNIZED_PAGE,
                            )?;
                            return Err(page_confirmation_failed(
                                operation,
                                step,
                                attempt,
                                miss.kind == LinearMissKind::AfterIntermediate,
                                &candidates.awaited(miss.skip_target_seen),
                                miss.elapsed,
                                miss.limit,
                            )
                            .into());
                        }
                    }
                }
                Some(frame) => {
                    // This step's own page: the input was swallowed; input again on this frame.
                    Self::linear_step_finished(runtime, step_index, operation, &frame.page_label)?;
                    attempt += 1;
                    observation = Some(frame);
                }
                None => {
                    Self::linear_step_finished(runtime, step_index, operation, UNRECOGNIZED_PAGE)?;
                    return Err(page_confirmation_failed(
                        operation,
                        step,
                        attempt,
                        false,
                        &candidates.awaited(false),
                        decision_started.elapsed(),
                        run.step_timeout,
                    )
                    .into());
                }
            }
        }
    }

    /// The guard on the frame that passed this step, then the effect intent and the input
    /// bound to that frame's input context, in the order of the page-graph path.
    fn linear_input<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        run: LinearRun,
        step_index: u32,
        operation: &TaskOperation,
        observation: &PageObservation,
        step_timing: ContainedTaskTimingContext,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        let deadline = run.timing.deadline();
        let (guard, target) = operation.guard_outcome(
            &self.control,
            observation,
            &self.evaluator,
            runtime,
            Some(step_timing),
        )?;
        let action_seed = runtime
            .action_seed(step_index, &operation.id)
            .map_err(ContainedTaskRunError::operation::<R>)?;
        let click = operation
            .click
            .as_ref()
            .ok_or_else(|| ContainedTaskError::new("contained_task_operation_invalid"))?;
        let (action, sampling) =
            click.input_action(&self.control.resolution, target.as_ref(), action_seed)?;
        runtime
            .record(ContainedTaskTrace::EffectIntent {
                step_index,
                operation_label: operation.id.clone(),
                action: action.clone(),
                sampling,
                guard,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        if observation.sampling.is_some() && Instant::now() >= step_timing.deadline() {
            return Err(ContainedTaskError::new("recognition_sample_deadline_exceeded").into());
        }
        if Instant::now() >= deadline {
            return Err(self
                .task_timeout_error(TaskTimingStage::BeforeInput, deadline, None)
                .into());
        }
        runtime
            .input(action, observation.input_context.clone())
            .map_err(ContainedTaskRunError::operation::<R>)?;
        Self::linear_effect_completed(runtime, run, step_index, operation)
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
        if Instant::now() >= deadline {
            return Err(self
                .task_timeout_error(TaskTimingStage::BeforeInput, deadline, None)
                .into());
        }
        match runtime
            .control_application(action)
            .map_err(ContainedTaskRunError::operation::<R>)?
        {
            ApplicationEffectSupport::Performed => {}
            ApplicationEffectSupport::Unsupported => {
                return Err(ContainedTaskError::with_detail(
                    "application_effect_requires_assigned_application",
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
        let effect_started = Instant::now();
        let effected = runtime.record(ContainedTaskTrace::EffectCompleted {
            step_index,
            operation_label: operation.id.clone(),
        });
        let effect_ended = Instant::now();
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
        let boundary = TaskTimingBoundary::PostInputWait;
        let identity = runtime.task_boundary_identity(boundary);
        let wait_started = Instant::now();
        let waited = if delay.is_zero() {
            Ok(())
        } else if deadline
            .checked_duration_since(Instant::now())
            .is_none_or(|remaining| delay >= remaining)
        {
            Err(self.task_timeout_error(TaskTimingStage::PostInputDelay, deadline, Some(delay)))
        } else {
            thread::sleep(delay);
            if Instant::now() >= deadline {
                Err(self.task_timeout_error(TaskTimingStage::PostInputDelay, deadline, None))
            } else {
                Ok(())
            }
        };
        let wait_ended = Instant::now();
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
    /// step's page, or the candidates of a run of optional operations (Workflow #339).
    fn linear_after_input<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        run: LinearRun,
        step: &LinearStep,
        candidates: &LinearCandidates<'_>,
        arrival: LinearArrival,
    ) -> LinearAttemptResult<R::Error> {
        match &step.transition {
            None => self.linear_arrival(
                runtime,
                run,
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
                let started = Instant::now();
                match self.linear_wait(
                    runtime,
                    &[page.as_str()],
                    *timeout,
                    *interval,
                    LinearWaitPurpose::AfterInput,
                    run.timing,
                )? {
                    // The next step's arrival budget starts at the frame that showed the page.
                    Some(_) => self.linear_arrival(
                        runtime,
                        run,
                        candidates,
                        arrival.budget,
                        arrival.interval,
                        LinearMissKind::AfterIntermediate,
                    ),
                    None => Ok(Err(LinearMiss {
                        kind: LinearMissKind::Intermediate,
                        elapsed: started.elapsed(),
                        limit: *timeout,
                        skip_target_seen: false,
                    })),
                }
            }
            Some(LinearTransition::Window { max, .. }) => self.linear_arrival(
                runtime,
                run,
                candidates,
                max.saturating_sub(arrival.post_input_delay)
                    .saturating_add(arrival.budget),
                arrival.interval,
                LinearMissKind::Arrival,
            ),
        }
    }

    /// The wait for the next step's page; several candidates (Workflow #339) are awaited by
    /// `linear_wait_set`.
    fn linear_arrival<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        run: LinearRun,
        candidates: &LinearCandidates<'_>,
        budget: Duration,
        interval: Duration,
        miss: LinearMissKind,
    ) -> LinearAttemptResult<R::Error> {
        let &[page] = candidates.pages.as_slice() else {
            return self.linear_wait_set(runtime, run, candidates, budget, interval, miss, None);
        };
        let started = Instant::now();
        Ok(self
            .linear_wait(
                runtime,
                &[page],
                budget,
                interval,
                LinearWaitPurpose::AfterInput,
                run.timing,
            )?
            .ok_or_else(|| LinearMiss {
                kind: miss,
                elapsed: started.elapsed(),
                limit: budget,
                skip_target_seen: false,
            }))
    }

    /// Workflow #339 §3.4: the wait for several candidates, the optional pages of a run first
    /// and its skip target last. An optional page that passes ends the wait. The skip target
    /// ends it only on a frame captured at least the settle after the capture that first
    /// showed it, so that an optional page arriving late still runs; frames on which nothing
    /// passes are ignored. The wait gives up `budget` after it starts, or, once the skip target
    /// was seen, the settle plus `budget` after that capture. `seen` is a capture that already
    /// showed the skip target (the retry decision's frame).
    #[allow(clippy::too_many_arguments)]
    fn linear_wait_set<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        run: LinearRun,
        candidates: &LinearCandidates<'_>,
        budget: Duration,
        interval: Duration,
        miss: LinearMissKind,
        mut seen: Option<Instant>,
    ) -> LinearAttemptResult<R::Error> {
        let purpose = LinearWaitPurpose::AfterInput;
        let deadline = run.timing.deadline();
        let skip_target = *candidates
            .pages
            .last()
            .ok_or_else(|| ContainedTaskError::new("contained_task_state_invalid"))?;
        let settle = candidates.settle;
        let started = Instant::now();
        loop {
            if Instant::now() >= deadline {
                return Err(self
                    .linear_task_timeout(
                        purpose,
                        deadline,
                        TaskTimingCheckPosition::PostconditionBeforeCapture,
                    )
                    .into());
            }
            let observation_deadline =
                seen.map_or(started + budget, |first| first + settle + budget);
            let timing = if self.evaluator.pack().target_consensus.is_empty() {
                run.timing
            } else {
                run.timing.with_deadline(observation_deadline)
            };
            let captured = Instant::now();
            let observation = self.linear_observe(runtime, &candidates.pages, timing)?;
            if Instant::now() >= deadline {
                return Err(self
                    .linear_task_timeout(
                        purpose,
                        deadline,
                        TaskTimingCheckPosition::PostconditionAfterCapture,
                    )
                    .into());
            }
            if let Some(observation) = observation {
                let captured = if observation.sampling.is_some() {
                    observation.frame_started
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
            }
            let limit = seen
                .map_or(started + budget, |first| first + settle + budget)
                .saturating_duration_since(started);
            let elapsed = started.elapsed();
            if elapsed >= limit {
                return Ok(Err(LinearMiss {
                    kind: miss,
                    elapsed,
                    limit,
                    skip_target_seen: seen.is_some(),
                }));
            }
            // The settle bounds the sleep only while it lasts; after it, captures keep the
            // interval and never run back to back.
            let settle_left = seen
                .and_then(|first| first.checked_add(settle))
                .and_then(|end| end.checked_duration_since(Instant::now()))
                .filter(|left| !left.is_zero());
            let sleep = interval
                .min(limit.saturating_sub(elapsed))
                .min(deadline.saturating_duration_since(Instant::now()));
            let boundary = purpose.boundary();
            let identity = runtime.task_boundary_identity(boundary);
            let wait_started = Instant::now();
            thread::sleep(settle_left.map_or(sleep, |left| sleep.min(left)));
            let wait_ended = Instant::now();
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
        if deadline
            .checked_duration_since(Instant::now())
            .is_none_or(|remaining| delay > remaining)
        {
            return Err(self
                .task_timeout_error(TaskTimingStage::RetryDelay, deadline, Some(delay))
                .into());
        }
        let boundary = TaskTimingBoundary::RetryWait;
        let identity = runtime.task_boundary_identity(boundary);
        let wait_started = Instant::now();
        thread::sleep(delay);
        let wait_ended = Instant::now();
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
    /// `None` once the budget is spent. A task deadline inside the wait fails the task.
    fn linear_wait<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        candidates: &[&str],
        budget: Duration,
        interval: Duration,
        purpose: LinearWaitPurpose,
        timing: ContainedTaskTimingContext,
    ) -> Result<Option<PageObservation>, ContainedTaskRunError<R::Error>> {
        let deadline = timing.deadline();
        let started = Instant::now();
        let timing = if self.evaluator.pack().target_consensus.is_empty() {
            timing
        } else {
            timing.with_deadline(started + budget)
        };
        loop {
            if Instant::now() >= deadline {
                return Err(self
                    .linear_task_timeout(
                        purpose,
                        deadline,
                        TaskTimingCheckPosition::PostconditionBeforeCapture,
                    )
                    .into());
            }
            let observation = self.linear_observe(runtime, candidates, timing)?;
            if Instant::now() >= deadline {
                return Err(self
                    .linear_task_timeout(
                        purpose,
                        deadline,
                        TaskTimingCheckPosition::PostconditionAfterCapture,
                    )
                    .into());
            }
            if observation.is_some() {
                return Ok(observation);
            }
            if started.elapsed() >= budget {
                return Ok(None);
            }
            let boundary = purpose.boundary();
            let identity = runtime.task_boundary_identity(boundary);
            let wait_started = Instant::now();
            thread::sleep(
                interval
                    .min(budget.saturating_sub(started.elapsed()))
                    .min(deadline.saturating_duration_since(Instant::now())),
            );
            let wait_ended = Instant::now();
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

    fn linear_task_timeout(
        &self,
        purpose: LinearWaitPurpose,
        deadline: Instant,
        position: TaskTimingCheckPosition,
    ) -> ContainedTaskError {
        let error = self.task_timeout_error(purpose.timeout_stage(), deadline, None);
        match purpose {
            LinearWaitPurpose::AfterInput => error.with_timing_check_position(position),
            LinearWaitPurpose::Entry | LinearWaitPurpose::RetryDecision => error,
        }
    }

    /// One capture and the evaluation of `candidates` only, recorded as every capture is, at
    /// the `CapturePage` boundary (§5.3). The first passing candidate is the frame's page; with
    /// the frame's input reference the committed input context binds a following input.
    fn linear_observe<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        candidates: &[&str],
        timing: ContainedTaskTimingContext,
    ) -> Result<Option<PageObservation>, ContainedTaskRunError<R::Error>> {
        self.capture_frame(runtime, None, None, timing, Some(candidates), None)
    }
}
