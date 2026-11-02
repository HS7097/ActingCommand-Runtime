// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #407 LOAD-3 (register §1.260; design `rd-1031/check-speed/model-loading.md` §3, §4,
//! §6; `contracts/resource-declarations.md`, "Loading pages"): how a page-graph run's waits treat
//! the package's loading pages, under `execution.loading.mode` as the runtime hands it with each
//! step's pacing ([`StepPacing::loading`]).
//!
//! - **`off`** (the default, and every Lab operation): no loading page is evaluated and every
//!   decision is the one before LOAD-3, a confirmation's wait page included.
//! - **`shadow`**: each frame of a wait is evaluated against the wait's loading pages and recorded
//!   (phase `loading`), the allowance is counted and its transitions recorded, and no decision
//!   changes.
//! - **`on`**: the one frame judgement below decides.
//!
//! **One judgement** ([`PreparedContainedTask::judge_frame`]), asked by the settled path, the
//! confirmation, the failure decision's confirmation, a wait step's wait, a page wait and an
//! observation window, row by row:
//!
//! 1. two or more pages, or a destination that is an error page: the recognition conflict;
//! 2. an error page;
//! 3. one or more loading pages held (several are no ambiguity), or the page is one the
//!    confirmation adds as one (`loading.add`, its wait page): a **transition frame**. It is
//!    offered to the shared interrupt pages first; a handled one pauses the wait as a handled
//!    interrupt does and spends nothing. Otherwise it spends the allowance and pauses the wait. It
//!    is never the step's final state (ACT-4's judgement is not asked on it: no `present` or
//!    `gone` evidence, no flip evidence), never an entry or a decision frame, never fed to the
//!    run's collectors and never still;
//! 4. what the wait waits for: the step's final state (ACT-4), a page wait's page;
//! 5. anything else, as before.
//!
//! **The allowance** ([`LoadingAllowance`]): one per input, shared by its settled path, its
//! confirmation with the extra rounds and observe-only retries, and its failure decision's
//! confirmation; one per wait step and one per page wait (the package's loading pages and
//! allowance). A transition frame spends the budget time since the wait's frame before it, at most
//! what is left. A transition frame after the allowance is spent ends the wait on it through the
//! wait's own path and code, with `loading_page_id`, `loading_ms` and `loading_limit_ms`: no
//! further time, no extra round. An observation window only suppresses: its transition frames
//! are frames of no destination and spend nothing.
//!
//! **Records** (`ContainedTaskRuntime::record_loading`): each evaluation of a frame's loading pages
//! with its time, and `entered` (the first transition frame of a stretch), `left` (the first frame
//! after it, with the time since its last transition frame) and `limit_reached` (once per
//! allowance), in both `shadow` and `on`.

use super::final_state::FinalState;
use super::{
    ContainedTaskError, ContainedTaskEvaluationTiming, ContainedTaskRunError, ContainedTaskRuntime,
    ContainedTaskTimingContext, PageObservation, PreparedContainedTask, Scene, StepPacing,
    TaskOperation, observe_instant_span,
};
use crate::codes::LoadingEventKind;
use actingcommand_contract::TaskTimingResult;
use actingcommand_contract::outcome::Outcome;
use actingcommand_page_detector::{
    PageBatchResult, PageDetectorError, require_all_page_evaluations,
};
use std::time::{Duration, Instant};

/// Workflow #407 LOAD-3: `execution.loading.mode` as the runtime hands it with a step's pacing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LoadingMode {
    /// No loading page is evaluated; every decision is the one before LOAD-3.
    #[default]
    Off,
    /// Loading pages are evaluated and recorded; no decision changes.
    Shadow,
    /// A transition frame pauses the wait within its allowance.
    On,
}

/// Workflow #407 LOAD-3: one transition of a wait's loading pages, handed to the runtime
/// ([`LoadingRecord::Event`]), which writes it as `contained_task_loading_recorded`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadingEvent {
    /// `entered`, `left` or `limit_reached`.
    pub kind: LoadingEventKind,
    /// The loading page of the stretch: the first of the wait's loading pages that held on its
    /// first frame, or the page the confirmation adds as one.
    pub loading_page_id: String,
    /// The budget time spent of the allowance: before the frame (`entered`), up to it (`left`,
    /// `limit_reached`).
    pub spent: Duration,
    /// The allowance in effect.
    pub limit: Duration,
    /// The operation whose input the allowance belongs to; none for a page wait.
    pub pack_operation_id: Option<String>,
    /// `entered` and `limit_reached`: the page of the package the frame suppresses; `left`: the
    /// page that held on the frame; none without one.
    pub page_id: Option<String>,
    /// `left`: the budget time since the stretch's last transition frame.
    pub exit_gap: Option<Duration>,
}

impl LoadingEvent {
    /// `outcome` with the event's values.
    pub fn values(&self, outcome: Outcome) -> Outcome {
        let mut outcome = outcome
            .loading_event(self.kind)
            .loading_page_id(self.loading_page_id.as_str())
            .loading_ms(self.spent)
            .loading_limit_ms(self.limit);
        if let Some(operation) = &self.pack_operation_id {
            outcome = outcome.pack_operation_id(operation.as_str());
        }
        if let Some(page) = &self.page_id {
            outcome = outcome.page_id(page.as_str());
        }
        if let Some(gap) = self.exit_gap {
            outcome = outcome.loading_exit_gap_ms(gap);
        }
        outcome
    }
}

/// Workflow #407 LOAD-3: what the kernel hands the runtime about a wait's loading pages
/// (`ContainedTaskRuntime::record_loading`).
#[derive(Debug, Clone, Copy)]
pub enum LoadingRecord<'a> {
    /// One frame's evaluation of the wait's loading pages (phase `loading`), with its time.
    Evaluated {
        results: &'a PageBatchResult,
        timing: ContainedTaskEvaluationTiming,
    },
    /// A transition of the wait's loading pages.
    Event(&'a LoadingEvent),
}

/// A wait that ended on its allowance: the values its failure carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LoadingLimit {
    page: String,
    spent: Duration,
    limit: Duration,
}

impl LoadingLimit {
    /// `outcome` with `loading_page_id`, `loading_ms` and `loading_limit_ms`.
    pub(super) fn values(&self, outcome: Outcome) -> Outcome {
        outcome
            .loading_page_id(self.page.as_str())
            .loading_ms(self.spent)
            .loading_limit_ms(self.limit)
    }
}

/// What a frame of a wait does with its allowance.
#[derive(Debug)]
pub(super) enum Charge {
    /// No transition frame, or one under `shadow`: the wait goes on as without loading pages.
    None,
    /// A transition frame within the allowance: the wait pauses for this budget time.
    Paused(Duration),
    /// A transition frame after the allowance was spent: the wait ends on it.
    Spent(LoadingLimit),
}

/// One frame of a wait as its allowance sees it.
pub(super) struct LoadingFrame<'a> {
    /// Whether row 3 of the judgement holds on it (in both modes).
    pub(super) transition: bool,
    /// The page of the package that held on it.
    pub(super) page: Option<&'a str>,
    /// Its budget instant, and that of the wait's frame before it (the wait's start for its
    /// first).
    pub(super) now: Instant,
    pub(super) previous: Instant,
}

/// The stretch of transition frames the latest frames are in.
#[derive(Debug)]
struct Stretch {
    page: String,
    /// The budget instant of its latest transition frame.
    last: Instant,
}

/// Workflow #407 LOAD-3 §3.2: the loading pages of one input's waits (or one wait step's, or one
/// page wait's) and the allowance they share.
#[derive(Debug)]
pub(super) struct LoadingAllowance {
    mode: LoadingMode,
    /// The loading pages of the package's loading copy each frame is evaluated against, as the
    /// detector names them, in the copy's order.
    probed: Vec<String>,
    /// The pages of the package the confirmation adds as loading pages (`loading.add`, its wait
    /// page), as the package declares them.
    added: Vec<String>,
    /// The probed pages that held on the latest frame evaluated.
    held: Vec<String>,
    limit: Duration,
    spent: Duration,
    operation: Option<String>,
    stretch: Option<Stretch>,
    limit_recorded: bool,
}

impl LoadingAllowance {
    /// No loading page: `off`, `loading: "off"`, or a package without any.
    pub(super) const fn off() -> Self {
        Self {
            mode: LoadingMode::Off,
            probed: Vec::new(),
            added: Vec::new(),
            held: Vec::new(),
            limit: Duration::ZERO,
            spent: Duration::ZERO,
            operation: None,
            stretch: None,
            limit_recorded: false,
        }
    }

    /// The allowance of one input of `operation` (or of a wait step's wait), under `pacing`'s mode
    /// (§2.4): `loading: "off"` takes no page; otherwise the package's loading pages less
    /// `except`, with `add` and the wait page. Its limit is `loading.max_ms`, else the wait page's
    /// `max_ms`, else the package's. A set with no limit anywhere (pages added to a package that
    /// carries no loading copy, without a `max_ms`) has an allowance of zero: its first transition
    /// frame ends the wait, named.
    pub(super) fn for_input(
        task: &PreparedContainedTask,
        operation: &TaskOperation,
        pacing: StepPacing,
    ) -> Self {
        if pacing.loading == LoadingMode::Off {
            return Self::off();
        }
        let declared = operation.loading();
        if declared.is_some_and(|loading| loading.off) {
            return Self::off();
        }
        let game = task.control.game.as_str();
        let except = declared.map_or(&[][..], |loading| loading.except.as_slice());
        let probed = task
            .detector
            .loading_page_ids()
            .filter(|page| {
                !except
                    .iter()
                    .any(|excepted| crate::page_anchor_matches(game, page, excepted))
            })
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let added = operation
            .loading_added_pages()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if probed.is_empty() && added.is_empty() {
            return Self::off();
        }
        let limit_ms = declared
            .and_then(|loading| loading.max_ms)
            .or_else(|| operation.wait_page().map(|wait_page| wait_page.max_ms))
            .or_else(|| task.detector.loading_max_ms())
            .unwrap_or(0);
        Self {
            mode: pacing.loading,
            probed,
            added,
            limit: Duration::from_millis(limit_ms),
            operation: Some(operation.id.clone()),
            ..Self::off()
        }
    }

    /// The allowance of one page wait under `pacing`'s mode: the package's loading pages and its
    /// allowance.
    pub(super) fn for_page_wait(task: &PreparedContainedTask, pacing: StepPacing) -> Self {
        let Some(limit_ms) = task.detector.loading_max_ms() else {
            return Self::off();
        };
        if pacing.loading == LoadingMode::Off {
            return Self::off();
        }
        Self {
            mode: pacing.loading,
            probed: task
                .detector
                .loading_page_ids()
                .map(str::to_owned)
                .collect(),
            limit: Duration::from_millis(limit_ms),
            ..Self::off()
        }
    }

    /// Whether row 3 decides (`on`).
    pub(super) fn decides(&self) -> bool {
        self.mode == LoadingMode::On
    }

    /// The pages of the package's loading copy the confirmation evaluates, in their order.
    #[cfg(test)]
    pub(super) fn probed(&self) -> &[String] {
        &self.probed
    }

    /// The pages of the package the confirmation adds as loading pages, as declared.
    #[cfg(test)]
    pub(super) fn added(&self) -> &[String] {
        &self.added
    }

    /// The allowance.
    #[cfg(test)]
    pub(super) const fn limit(&self) -> Duration {
        self.limit
    }

    /// Whether `page_label` is a page the confirmation adds as a loading page.
    fn adds(&self, game: &str, page_label: &str) -> bool {
        self.added
            .iter()
            .any(|added| crate::page_anchor_matches(game, page_label, added))
    }

    /// Whether row 3 holds on the latest frame evaluated, whose page is `page` (none without
    /// one): a loading page held, or the page is one the confirmation adds. Never under `off`.
    fn holds(&self, game: &str, page: Option<&str>) -> bool {
        self.mode != LoadingMode::Off
            && (!self.held.is_empty() || page.is_some_and(|page| self.adds(game, page)))
    }

    /// Under `on`, whether a frame whose page is `page` is a transition frame, which feeds nothing.
    pub(super) fn suppresses(&self, game: &str, page: Option<&str>) -> bool {
        self.decides() && self.holds(game, page)
    }

    /// A frame of a wait that spends the allowance (§3.2).
    pub(super) fn frame<R: ContainedTaskRuntime>(
        &mut self,
        runtime: &mut R,
        game: &str,
        frame: LoadingFrame<'_>,
    ) -> Result<Charge, ContainedTaskRunError<R::Error>> {
        self.account(runtime, game, frame, true)
    }

    /// A frame of an observation window, which only suppresses: its transitions are recorded and
    /// nothing is spent.
    pub(super) fn watch<R: ContainedTaskRuntime>(
        &mut self,
        runtime: &mut R,
        game: &str,
        frame: LoadingFrame<'_>,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        self.account(runtime, game, frame, false).map(drop)
    }

    fn account<R: ContainedTaskRuntime>(
        &mut self,
        runtime: &mut R,
        game: &str,
        frame: LoadingFrame<'_>,
        spends: bool,
    ) -> Result<Charge, ContainedTaskRunError<R::Error>> {
        if self.mode == LoadingMode::Off {
            return Ok(Charge::None);
        }
        if !frame.transition {
            if let Some(stretch) = self.stretch.take() {
                self.record(
                    runtime,
                    LoadingEventKind::Left,
                    stretch.page,
                    frame.page.map(str::to_owned),
                    Some(frame.now.saturating_duration_since(stretch.last)),
                )?;
            }
            return Ok(Charge::None);
        }
        let suppressed = frame
            .page
            .filter(|page| !self.adds(game, page))
            .map(str::to_owned);
        let page = if let Some(stretch) = self.stretch.as_mut() {
            stretch.last = frame.now;
            stretch.page.clone()
        } else {
            let page = self
                .held
                .first()
                .cloned()
                .or_else(|| frame.page.map(str::to_owned))
                .unwrap_or_default();
            self.record(
                runtime,
                LoadingEventKind::Entered,
                page.clone(),
                suppressed.clone(),
                None,
            )?;
            self.stretch = Some(Stretch {
                page: page.clone(),
                last: frame.now,
            });
            page
        };
        if !spends {
            return Ok(Charge::None);
        }
        let left = self.limit.saturating_sub(self.spent);
        if left.is_zero() {
            if !self.limit_recorded {
                self.limit_recorded = true;
                self.record(
                    runtime,
                    LoadingEventKind::LimitReached,
                    page.clone(),
                    suppressed,
                    None,
                )?;
            }
            return Ok(if self.decides() {
                Charge::Spent(LoadingLimit {
                    page,
                    spent: self.spent,
                    limit: self.limit,
                })
            } else {
                Charge::None
            });
        }
        let span = frame
            .now
            .saturating_duration_since(frame.previous)
            .min(left);
        self.spent += span;
        Ok(if self.decides() {
            Charge::Paused(span)
        } else {
            Charge::None
        })
    }

    fn record<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        kind: LoadingEventKind,
        loading_page_id: String,
        page_id: Option<String>,
        exit_gap: Option<Duration>,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        let event = LoadingEvent {
            kind,
            loading_page_id,
            spent: self.spent,
            limit: self.limit,
            pack_operation_id: self.operation.clone(),
            page_id,
            exit_gap,
        };
        runtime
            .record_loading(LoadingRecord::Event(&event))
            .map_err(ContainedTaskRunError::Boundary)
    }
}

/// Rows 1 to 5 of the one judgement (§3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FrameRow {
    /// Row 1: two or more pages, or a destination that is an error page.
    Conflict,
    /// Row 2: an error page.
    ErrorPage,
    /// Row 3 under `on`: a transition frame.
    Transition,
    /// Row 4: what the wait waits for.
    Reached,
    /// Row 5: anything else.
    Other,
}

/// The one judgement of a frame of a wait.
#[derive(Debug)]
pub(super) struct FrameJudgement {
    /// The row that decides: row 3 only under `on`; otherwise as without loading pages.
    pub(super) row: FrameRow,
    /// Whether row 3 holds on the frame (in `shadow` too), for the allowance.
    pub(super) transition: bool,
    /// ACT-4's final-state judgement, asked only on a frame of rows 4 and 5 that has a page.
    pub(super) final_state: Option<FinalState>,
}

impl PreparedContainedTask {
    /// Workflow #407 LOAD-3 §3.1: the one judgement of `observation` (none: a frame that gave no
    /// page) after `operation`'s input, or of a page wait's frame without `operation`, within the
    /// run's `timing`. A page wait takes any page, an error page included (row 2 before row 3).
    pub(super) fn judge_frame<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        operation: Option<&TaskOperation>,
        observation: Option<&PageObservation>,
        loading: &LoadingAllowance,
        timing: ContainedTaskTimingContext,
    ) -> Result<FrameJudgement, ContainedTaskRunError<R::Error>> {
        let game = self.control.game.as_str();
        let judged = |row, transition| FrameJudgement {
            row,
            transition,
            final_state: None,
        };
        let Some(observation) = observation else {
            let transition = loading.holds(game, None);
            let row = if transition && loading.decides() {
                FrameRow::Transition
            } else {
                FrameRow::Other
            };
            return Ok(judged(row, transition));
        };
        let error_page = self
            .program
            .is_error_page(&self.control, &observation.page_label);
        if let Some(operation) = operation {
            let destinations = operation.matching_destination_count(&self.control, observation)?;
            if destinations > 1 || (destinations == 1 && error_page) {
                return Ok(judged(FrameRow::Conflict, false));
            }
        }
        if error_page {
            return Ok(judged(FrameRow::ErrorPage, false));
        }
        let transition = loading.holds(game, Some(&observation.page_label));
        if transition && loading.decides() {
            return Ok(judged(FrameRow::Transition, true));
        }
        let Some(operation) = operation else {
            return Ok(judged(FrameRow::Reached, transition));
        };
        // ACT-4: the final-state judgement is row 4's.
        let final_state = self.final_state(runtime, operation, observation, timing)?;
        Ok(FrameJudgement {
            row: if final_state.reached {
                FrameRow::Reached
            } else {
                FrameRow::Other
            },
            transition,
            final_state: Some(final_state),
        })
    }

    /// Workflow #407 LOAD-3 §6: `loading`'s pages evaluated on `scene`, the frame just recognized,
    /// recorded as the phase `loading` with the time the evaluation took, and the pages that held
    /// kept as the frame's. The evaluation reads the frame's scene in a context of its own: the
    /// page recognition's context holds the run's sample recorder, and a loading copy's targets
    /// are its own (no result of the page recognition's is theirs). A failed evaluation fails the
    /// run under `on` with the code page recognition gives it; under `shadow`, where nothing may
    /// change, its row records the failure and no loading page holds.
    pub(super) fn probe_loading<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        scene: &Scene,
        timing: ContainedTaskTimingContext,
        loading: &mut LoadingAllowance,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        loading.held.clear();
        if loading.mode == LoadingMode::Off || loading.probed.is_empty() {
            return Ok(());
        }
        let pages = loading
            .probed
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let started = runtime.monotonic_now();
        let results = {
            let context = self.evaluator.scene_context(scene);
            self.detector
                .evaluate_pages_outcomes_in_context(&context, &pages)
        };
        let ended = runtime.monotonic_now();
        let measured = ContainedTaskEvaluationTiming {
            elapsed_us: observe_instant_span(started, ended),
            budget_before: timing.budget_at(started),
            result: if results.is_ok() {
                TaskTimingResult::Ok
            } else {
                TaskTimingResult::Err
            },
        };
        runtime
            .record_loading(LoadingRecord::Evaluated {
                results: &results,
                timing: measured,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        let failure_code = crate::page_batch_failure_code(&results);
        let evaluations = results
            .map_err(|error| PageDetectorError::fatal(error.to_string()))
            .and_then(require_all_page_evaluations);
        match evaluations {
            Ok(evaluations) => {
                loading.held = evaluations
                    .into_iter()
                    .filter(|evaluation| evaluation.matched)
                    .map(|evaluation| evaluation.page_id)
                    .collect();
                Ok(())
            }
            Err(error) if loading.decides() => {
                Err(ContainedTaskError::with_detail(failure_code, error.to_string()).into())
            }
            Err(_) => Ok(()),
        }
    }
}
