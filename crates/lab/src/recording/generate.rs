// SPDX-License-Identifier: AGPL-3.0-only

//! `record stop` package generation (Workflow #336 L4, frozen model section 4): the effective
//! steps numbered again 1..n, one page per step and per page transition, the `linear_steps`
//! task, control and resources documents, the guard of every click, and the timeouts with
//! their clamp warnings. Everything here is computed in memory; nothing is written.

use super::model::{
    LabRecording, MarkFamily, RecordRect, RecordSize, RecordedFrame, RecordedMark,
    RecordingDefaults, RecordingStep, SelfTestMargin, StepOptional, StepTransition,
};
use super::store::{blocked, invalid, with_details};
use actingcommand_contract::{LabError, LabResult};
use actingcommand_execution_kernel::{linear_main_interface, prerequisite_package_id_valid};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// The largest task timeout and transition bound (`taskflow.rs`).
pub(crate) const MAX_TIMEOUT_MS: u64 = 1_800_000;
/// The largest control `step_timeout_ms`.
pub(crate) const MAX_STEP_TIMEOUT_MS: u64 = 60_000;
pub(crate) const DEFAULT_ARRIVAL_TIMEOUT_MS: u64 = 15_000;
pub(crate) const DEFAULT_APPLICATION_ARRIVAL_TIMEOUT_MS: u64 = 90_000;
/// The kernel's largest step count.
pub(crate) const MAX_EFFECT_STEPS: usize = 1_000;
const CLICK_POST_DELAY_MS: u64 = 200;
const APPLICATION_POST_DELAY_MS: u64 = 1_000;
const ARRIVAL_INTERVAL_MS: u64 = 500;
/// The margin of one application operation itself (R24 section 4.3).
const APPLICATION_MARGIN_MS: u64 = 10_000;
const TIMEOUT_MARGIN_MS: u64 = 10_000;
const CONTROL_SCHEMA: &str = "Lab-1y.control.v2";
const TASK_SCHEMA: &str = "0.9";

/// The settings of one `record stop`, resolved from its options and the recording.
#[derive(Debug, Clone)]
pub(crate) struct StopSettings {
    pub(crate) game: Option<String>,
    pub(crate) server: Option<String>,
    pub(crate) locale: Option<String>,
    pub(crate) package_id: Option<String>,
    pub(crate) requires: Option<String>,
    pub(crate) timeout_ms: Option<u64>,
    pub(crate) arrival_timeout_ms: u64,
    pub(crate) application_arrival_timeout_ms: u64,
}

/// One effective step with its package number and pages.
#[derive(Debug, Clone)]
pub(crate) struct PlanStep {
    pub(crate) number: u32,
    pub(crate) step: RecordingStep,
    /// `None` for the application entry step (`from:"any"`).
    pub(crate) page: Option<String>,
    pub(crate) required: Vec<String>,
    /// The page of a recognizable transition after this step's effect.
    pub(crate) transition_page: Option<String>,
    pub(crate) transition_required: Vec<String>,
}

impl PlanStep {
    pub(crate) fn is_entry(&self) -> bool {
        self.step.is_application_entry()
    }

    pub(crate) fn has_effect(&self) -> bool {
        self.step.click.is_some() || self.step.application.is_some()
    }

    /// The live (not superseded) frames of the step: its primary frame and samples.
    pub(crate) fn live_frames(&self) -> Vec<&RecordedFrame> {
        self.step
            .frames
            .iter()
            .filter(|frame| !frame.superseded)
            .collect()
    }

    pub(crate) fn primary(&self) -> Option<&RecordedFrame> {
        self.step
            .frames
            .iter()
            .rev()
            .find(|frame| frame.role == "primary" && !frame.superseded)
    }

    /// The live frames of the step's page transition (`--to-transition` keeps the
    /// `superseded` flag of the frames it moves).
    pub(crate) fn transition_live_frames(&self) -> Vec<&RecordedFrame> {
        match &self.step.transition {
            Some(StepTransition::Page { frames, .. }) => {
                frames.iter().filter(|frame| !frame.superseded).collect()
            }
            _ => Vec::new(),
        }
    }

    pub(crate) fn window(&self) -> Option<(u64, u64)> {
        match &self.step.transition {
            Some(StepTransition::Window { min_ms, max_ms }) => Some((*min_ms, *max_ms)),
            _ => None,
        }
    }

    /// The settle of an optional step (Workflow #339); `None` for a required step.
    pub(crate) fn settle_ms(&self) -> Option<u64> {
        self.step
            .optional
            .as_ref()
            .map(|optional| optional.settle_ms)
    }

    /// The step's page; every step but the application entry step has one.
    pub(crate) fn page_id(&self) -> LabResult<&str> {
        self.page.as_deref().ok_or_else(|| {
            blocked(
                "record_step_frame_missing",
                format!("step {} has no page", self.step.index),
            )
        })
    }
}

/// A maximal run of consecutive optional steps (Workflow #339 section 3.1), as indices into
/// `Plan::steps`: the required step before it, its members in path order, and the step whose
/// page follows the run (its skip target N).
#[derive(Debug, Clone)]
pub(crate) struct OptionalRun {
    pub(crate) previous: usize,
    pub(crate) members: Vec<usize>,
    pub(crate) skip: usize,
}

impl OptionalRun {
    /// A run settles at most once, so it adds its largest settle (section 2.5).
    fn settle_ms(&self, steps: &[PlanStep]) -> u64 {
        self.members
            .iter()
            .filter_map(|&member| steps[member].settle_ms())
            .max()
            .unwrap_or(0)
    }
}

/// The runs of optional steps in path order. The pre-checks keep step 1 and the last step
/// required, so every run has a required step before it and a skip target after it.
fn optional_runs(steps: &[PlanStep]) -> LabResult<Vec<OptionalRun>> {
    let mut runs = Vec::new();
    let mut index = 0;
    while index < steps.len() {
        if steps[index].settle_ms().is_none() {
            index += 1;
            continue;
        }
        let start = index;
        while steps
            .get(index)
            .is_some_and(|step| step.settle_ms().is_some())
        {
            index += 1;
        }
        let previous = start
            .checked_sub(1)
            .filter(|_| index < steps.len())
            .ok_or_else(|| {
                invalid(
                    "validation_failed",
                    "a run of optional steps has no required step before or after it",
                )
            })?;
        runs.push(OptionalRun {
            previous,
            members: (start..index).collect(),
            skip: index,
        });
    }
    Ok(runs)
}

/// The package a recording generates, before it is encoded.
#[derive(Debug, Clone)]
pub(crate) struct Plan {
    pub(crate) record_id: String,
    pub(crate) task_id: String,
    pub(crate) game: String,
    pub(crate) server: String,
    pub(crate) locale: String,
    pub(crate) package_id: String,
    pub(crate) requires: Option<String>,
    pub(crate) size: RecordSize,
    pub(crate) defaults: RecordingDefaults,
    pub(crate) generated_at_unix_ms: u64,
    pub(crate) steps: Vec<PlanStep>,
    /// The runs of optional steps (Workflow #339), in path order.
    pub(crate) runs: Vec<OptionalRun>,
    /// Every emitted mark (own marks of the effective steps and their transitions), by id.
    pub(crate) marks: BTreeMap<String, RecordedMark>,
    /// The emitted mark ids in package order.
    pub(crate) emitted: Vec<String>,
    pub(crate) width: usize,
    pub(crate) arrival_timeout_ms: u64,
    pub(crate) application_arrival_timeout_ms: u64,
    pub(crate) step_timeout_ms: u64,
    pub(crate) timeout_ms: u64,
    /// Warnings known before the self-checks: clamps, the requires prefix, stop targets.
    pub(crate) warnings: Vec<Value>,
    /// Steps with a click or application operation marked `needs_review`.
    pub(crate) needs_review: Vec<Value>,
}

impl Plan {
    pub(crate) fn numbered(&self, number: u32) -> String {
        format!("{number:0width$}", width = self.width)
    }

    pub(crate) fn operation_id(&self, step: &PlanStep) -> String {
        let kind = if step.step.application.is_some() {
            "app"
        } else {
            "click"
        };
        format!("step_{}_{kind}", self.numbered(step.number))
    }

    /// The page the effect of step `index` (0-based) must reach: the next step's page.
    pub(crate) fn next_page(&self, index: usize) -> LabResult<&str> {
        self.steps
            .get(index + 1)
            .and_then(|step| step.page.as_deref())
            .ok_or_else(|| invalid("validation_failed", "a step after an effect has no page"))
    }

    /// The first page a run waits for after the effect of step `index`: the transition page,
    /// or the next step's page.
    pub(crate) fn gate(&self, index: usize) -> LabResult<(String, Vec<String>)> {
        let step = &self.steps[index];
        if let Some(page) = &step.transition_page {
            return Ok((page.clone(), step.transition_required.clone()));
        }
        let next = self
            .steps
            .get(index + 1)
            .ok_or_else(|| invalid("validation_failed", "a step after an effect is missing"))?;
        Ok((self.next_page(index)?.to_string(), next.required.clone()))
    }

    pub(crate) fn effect_steps(&self) -> usize {
        self.steps.iter().filter(|step| step.has_effect()).count()
    }

    /// The run of optional steps that step `index` (0-based) comes right before.
    pub(crate) fn run_after(&self, index: usize) -> Option<&OptionalRun> {
        self.runs.iter().find(|run| run.previous == index)
    }

    /// The run of optional steps that step `index` (0-based) belongs to.
    pub(crate) fn run_of(&self, index: usize) -> Option<&OptionalRun> {
        self.runs.iter().find(|run| run.members.contains(&index))
    }

    pub(crate) fn has_application_step(&self) -> bool {
        self.steps
            .iter()
            .any(|step| step.step.application.is_some())
    }

    pub(crate) fn mark(&self, id: &str) -> LabResult<&RecordedMark> {
        self.marks.get(id).ok_or_else(|| {
            with_details(
                blocked(
                    "record_mark_rejected",
                    format!("mark '{id}' is not a mark of an effective step"),
                ),
                json!({"marks": [{"id": id, "reason": "reuse_source_missing"}]}),
            )
        })
    }

    /// Whether the marks `required` of a page involve OCR: an OCR mark, or a check with an
    /// OCR member.
    pub(crate) fn involves_ocr(&self, required: &[String]) -> LabResult<bool> {
        for id in required {
            let mark = self.mark(id)?;
            match mark.family {
                MarkFamily::Ocr => return Ok(true),
                MarkFamily::Check => {
                    for member in mark.check_members() {
                        if self.mark(member)?.family == MarkFamily::Ocr {
                            return Ok(true);
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(false)
    }

    /// The template marks whose crops the package carries, as `(mark id, asset path)`.
    pub(crate) fn template_assets(&self) -> Vec<(String, String)> {
        self.emitted
            .iter()
            .filter_map(|id| self.marks.get(id))
            .filter_map(|mark| {
                mark.crop
                    .as_ref()
                    .filter(|_| mark.family == MarkFamily::Template)
                    .map(|crop| (mark.id.clone(), crop.asset.clone()))
            })
            .collect()
    }

    pub(crate) fn task_dir(&self) -> String {
        format!("resources/operations/{}", self.task_id)
    }
}

/// The effective steps (neither dropped nor converted), by serial number.
pub(crate) fn effective_steps(recording: &LabRecording) -> Vec<&RecordingStep> {
    let mut steps = recording
        .steps
        .iter()
        .filter(|step| step.is_effective())
        .collect::<Vec<_>>();
    steps.sort_by_key(|step| step.index);
    steps
}

fn stop_blocked(code: &str, message: String, details: Value) -> LabError {
    with_details(blocked(code, message), details)
}

/// Workflow #339 section 2.5: where an optional step cannot be. Step 1 is the only page the
/// entry gate checks, every run ends on the last step, an optional application step would be a
/// conditional restart, and a launch or restart is complete only on the main interface (R25),
/// which every run must therefore reach.
fn refuse_optional_positions(steps: &[PlanStep], game: &str) -> LabResult<()> {
    let refused = |code: &str, step: &PlanStep, message: String| {
        stop_blocked(
            code,
            message,
            json!({"step": step.step.index, "artifact_step": step.number}),
        )
    };
    if let Some(first) = steps.first().filter(|step| step.settle_ms().is_some()) {
        return Err(refused(
            "record_optional_first_step",
            first,
            format!(
                "step {} is optional but is the first step of the package: the package starts \
                 from its page and a run checks only that page before its first effect; clear it \
                 with `record mark --step {} --not-optional` or record from one screen earlier",
                first.step.index, first.step.index
            ),
        ));
    }
    if let Some(last) = steps.last().filter(|step| step.settle_ms().is_some()) {
        return Err(refused(
            "record_optional_final_step",
            last,
            format!(
                "the last step {} is optional: the package ends on its page, which every run must \
                 reach; clear it with `record mark --step {} --not-optional`",
                last.step.index, last.step.index
            ),
        ));
    }
    if let Some(step) = steps
        .iter()
        .find(|step| step.settle_ms().is_some() && step.step.application.is_some())
    {
        return Err(refused(
            "record_optional_application",
            step,
            format!(
                "step {} is optional and has an application operation: an application step \
                 cannot be optional (there is no conditional restart); clear it with `record \
                 mark --step {} --not-optional`",
                step.step.index, step.step.index
            ),
        ));
    }
    if let Some(start) = steps.iter().rposition(|step| {
        step.step
            .application
            .as_ref()
            .is_some_and(|application| matches!(application.action.as_str(), "launch" | "restart"))
    }) && let Some(home) = steps[start + 1..].iter().find(|step| {
        step.page
            .as_deref()
            .is_some_and(|page| linear_main_interface(game, page))
    }) && home.settle_ms().is_some()
    {
        return Err(refused(
            "record_optional_restart_segment_end",
            home,
            format!(
                "step {} ({}) is the main interface after the last launch or restart and is \
                 optional: a launch or restart is complete only on the main interface, which \
                 every run must reach; mark the screens before it optional and clear it with \
                 `record mark --step {} --not-optional`",
                home.step.index,
                home.page.as_deref().unwrap_or_default(),
                home.step.index
            ),
        ));
    }
    Ok(())
}

/// A timeout given on the command line must lie in 1..=1800000 (`validation_failed`, exit 2).
pub(crate) fn validate_timeout_option(name: &str, value: Option<u64>) -> LabResult<()> {
    if value.is_some_and(|value| !(1..=MAX_TIMEOUT_MS).contains(&value)) {
        return Err(with_details(
            invalid(
                "validation_failed",
                format!("{name} must be in 1..={MAX_TIMEOUT_MS}"),
            ),
            json!({"option": name, "value": value, "max": MAX_TIMEOUT_MS}),
        ));
    }
    Ok(())
}

fn requires_invalid(requires: &str, reason: &str, message: String) -> LabError {
    with_details(
        invalid("record_requires_invalid", message),
        json!({"requires": requires, "reason": reason}),
    )
}

/// The marks of a page: own marks then reused ids, without the members of its `any_of`
/// checks (a member of an `any_of` check is not required on its own).
fn required_ids(
    own: &[RecordedMark],
    reused: &[String],
    marks: &BTreeMap<String, RecordedMark>,
) -> Vec<String> {
    let ids = own
        .iter()
        .map(|mark| mark.id.clone())
        .chain(reused.iter().cloned())
        .collect::<Vec<_>>();
    let any_of_members = ids
        .iter()
        .filter_map(|id| marks.get(id))
        .filter_map(|mark| mark.any_of.as_ref())
        .flatten()
        .cloned()
        .collect::<BTreeSet<_>>();
    ids.into_iter()
        .filter(|id| !any_of_members.contains(id))
        .collect()
}

/// Section 4.1 pre-checks (with the R24 and R25 rules), section 4.2 pages and the timeouts of
/// section 4.3. Any refusal leaves the recording unchanged.
pub(crate) fn plan(recording: &LabRecording, settings: &StopSettings) -> LabResult<Plan> {
    validate_timeout_option("--timeout-ms", settings.timeout_ms)?;
    let effective = effective_steps(recording);
    let n = effective.len();
    if n < 2 {
        return Err(stop_blocked(
            "record_no_steps",
            format!(
                "the recording has {n} effective step(s); a package needs at least two: a \
                 screen with its effect and the screen it arrives at"
            ),
            json!({"effective_steps": n}),
        ));
    }
    for (position, step) in effective.iter().enumerate() {
        if position > 0 && step.is_application_entry() {
            return Err(stop_blocked(
                "record_application_entry_not_first",
                format!(
                    "step {} is an application entry step (no frame) but not the first \
                     effective step",
                    step.index
                ),
                json!({"step": step.index}),
            ));
        }
    }
    for (position, step) in effective.iter().enumerate() {
        let effect = step.click.is_some() || step.application.is_some();
        let number = position + 1;
        if number < n && !effect {
            return Err(stop_blocked(
                "record_step_click_missing",
                format!(
                    "step {} (package step {number}) has no effect: every step before the last \
                     needs a click or an application operation; if it shows a loading screen, \
                     turn it into a transition with `record mark --to-transition {}`",
                    step.index, step.index
                ),
                json!({"step": step.index, "artifact_step": number}),
            ));
        }
        if number == n && effect {
            return Err(stop_blocked(
                "record_final_step_has_click",
                format!(
                    "the last step {} has an effect; the last step only recognizes the screen \
                     the package arrives at: capture and mark the screen after its effect",
                    step.index
                ),
                json!({"step": step.index, "artifact_step": number}),
            ));
        }
    }
    for step in &effective {
        if !step.is_application_entry() && step.marks.is_empty() && step.reused.is_empty() {
            return Err(stop_blocked(
                "record_step_without_recognition",
                format!("step {} has no recognition mark", step.index),
                json!({"step": step.index}),
            ));
        }
        if let Some(StepTransition::Page { marks, reused, .. }) = &step.transition
            && marks.is_empty()
            && reused.is_empty()
        {
            return Err(stop_blocked(
                "record_step_without_recognition",
                format!(
                    "the page transition of step {} has no recognition mark",
                    step.index
                ),
                json!({"step": step.index, "transition": "page"}),
            ));
        }
    }
    let effect_steps = n - 1;
    if effect_steps > MAX_EFFECT_STEPS {
        return Err(stop_blocked(
            "record_too_many_steps",
            format!(
                "the recording has {effect_steps} steps with an effect; a package has at most \
                 {MAX_EFFECT_STEPS}"
            ),
            json!({"effect_steps": effect_steps, "max": MAX_EFFECT_STEPS}),
        ));
    }
    let missing = [
        ("game", settings.game.is_none()),
        ("server", settings.server.is_none()),
        ("locale", settings.locale.is_none()),
    ]
    .into_iter()
    .filter(|(_, missing)| *missing)
    .map(|(name, _)| name)
    .collect::<Vec<_>>();
    let (Some(game), Some(server), Some(locale)) = (
        settings.game.clone(),
        settings.server.clone(),
        settings.locale.clone(),
    ) else {
        return Err(with_details(
            invalid(
                "record_locale_missing",
                format!(
                    "the package needs its game, server and locale; missing: {}; give them on \
                     record start or record stop (--game, --server, --locale)",
                    missing.join(", ")
                ),
            ),
            json!({"missing": missing}),
        ));
    };
    let package_id = settings
        .package_id
        .clone()
        .unwrap_or_else(|| format!("{game}.{server}.{}", recording.task_id));
    if !prerequisite_package_id_valid(&package_id) {
        return Err(with_details(
            invalid(
                "validation_failed",
                "--package-id must be non-empty, at most 256 bytes and without control \
                 characters",
            ),
            json!({"package_id": package_id}),
        ));
    }
    let mut warnings = Vec::new();
    if let Some(requires) = &settings.requires {
        if !prerequisite_package_id_valid(requires) {
            return Err(requires_invalid(
                requires,
                "prerequisite_id_invalid",
                "--requires must be non-empty, at most 256 bytes and without control characters"
                    .to_string(),
            ));
        }
        if *requires == package_id {
            return Err(requires_invalid(
                requires,
                "prerequisite_self",
                format!("--requires names this package itself ({package_id})"),
            ));
        }
        if effective[0].is_application_entry() {
            return Err(requires_invalid(
                requires,
                "application_entry",
                "the first step is an application entry step: it recognizes nothing before its \
                 operation, so there is no first step a prerequisite package could lead to"
                    .to_string(),
            ));
        }
        let prefix = format!("{game}.{server}.");
        if !requires.starts_with(&prefix) {
            warnings.push(json!({
                "code": "requires_prefix_mismatch",
                "requires": requires,
                "expected_prefix": prefix,
                "message": format!(
                    "--requires {requires} does not start with {prefix}: a prerequisite package \
                     runs on the same game and server"
                )
            }));
        }
    }
    for pair in effective.windows(2) {
        let after_stop = pair[0]
            .application
            .as_ref()
            .is_some_and(|application| application.action == "stop");
        let next_invalid = pair[1].click.is_some()
            || pair[1]
                .application
                .as_ref()
                .is_some_and(|application| application.action == "stop");
        if after_stop && next_invalid {
            return Err(stop_blocked(
                "record_step_after_stop_invalid",
                format!(
                    "step {} stops the application, so the effect of step {} can only be a \
                     launch or a restart: the foreground gate refuses every pointer input while \
                     the assigned application is not in the foreground",
                    pair[0].index, pair[1].index
                ),
                json!({"step": pair[0].index, "next_step": pair[1].index}),
            ));
        }
    }

    // Section 4.2: pages, numbered to one width (at least two digits).
    let width = n.to_string().len().max(2);
    let numbered = |number: usize| format!("{number:0width$}");
    let mut marks = BTreeMap::new();
    let mut emitted = Vec::new();
    for step in &effective {
        for mark in &step.marks {
            emitted.push(mark.id.clone());
            marks.insert(mark.id.clone(), mark.clone());
        }
    }
    for step in &effective {
        if let Some(StepTransition::Page {
            marks: transition_marks,
            ..
        }) = &step.transition
        {
            for mark in transition_marks {
                emitted.push(mark.id.clone());
                marks.insert(mark.id.clone(), mark.clone());
            }
        }
    }
    let mut steps = Vec::with_capacity(n);
    for (position, step) in effective.iter().enumerate() {
        let number = position + 1;
        let page = (!step.is_application_entry()).then(|| match &step.page {
            Some(name) => format!("step_{}_{name}", numbered(number)),
            None => format!("step_{}", numbered(number)),
        });
        let required = required_ids(&step.marks, &step.reused, &marks);
        let (transition_page, transition_required) = match &step.transition {
            Some(StepTransition::Page {
                marks: transition_marks,
                reused,
                ..
            }) => (
                Some(format!("transition_{}", numbered(number))),
                required_ids(transition_marks, reused, &marks),
            ),
            _ => (None, Vec::new()),
        };
        for id in required.iter().chain(&transition_required) {
            if !marks.contains_key(id) {
                return Err(with_details(
                    blocked(
                        "record_mark_rejected",
                        format!(
                            "step {} names mark '{id}', which is not a mark of an effective step",
                            step.index
                        ),
                    ),
                    json!({"marks": [{"id": id, "reason": "reuse_source_missing"}]}),
                ));
            }
        }
        steps.push(PlanStep {
            number: u32::try_from(number).unwrap_or(u32::MAX),
            step: (*step).clone(),
            page,
            required,
            transition_page,
            transition_required,
        });
    }

    // R25: a launch or restart is complete only on the main interface; one such page must
    // follow the last of them (the kernel predicate `linear_main_interface`).
    if let Some(start) = steps.iter().rposition(|step| {
        step.step
            .application
            .as_ref()
            .is_some_and(|application| matches!(application.action.as_str(), "launch" | "restart"))
    }) {
        let later = steps[start + 1..]
            .iter()
            .filter_map(|step| step.page.clone())
            .collect::<Vec<_>>();
        if !later.iter().any(|page| linear_main_interface(&game, page)) {
            let step = &steps[start];
            return Err(stop_blocked(
                "record_application_without_home",
                format!(
                    "step {} ({}) launches or restarts the application, but no later step is the \
                     main interface: a launch or restart is complete only on the main interface; \
                     mark that step with `record mark --step <k> --page home` (its page \
                     step_<nn>_home is the main interface)",
                    step.step.index,
                    step.step
                        .application
                        .as_ref()
                        .map_or("application", |application| application.action.as_str())
                ),
                json!({"step": step.step.index, "artifact_step": step.number, "later_pages": later}),
            ));
        }
    }
    refuse_optional_positions(&steps, &game)?;
    let runs = optional_runs(&steps)?;

    let size = recording.coordinate_space.ok_or_else(|| {
        blocked(
            "record_step_frame_missing",
            "the recording has no frame, so it has no coordinate space",
        )
    })?;

    // Timeouts (section 4.3 with the R24 formula).
    validate_timeout_option("--arrival-timeout-ms", Some(settings.arrival_timeout_ms))?;
    validate_timeout_option(
        "--application-arrival-timeout-ms",
        Some(settings.application_arrival_timeout_ms),
    )?;
    let arrival = settings.arrival_timeout_ms;
    let application_arrival = settings.application_arrival_timeout_ms;
    let step_timeout = arrival.min(MAX_STEP_TIMEOUT_MS);
    if arrival > MAX_STEP_TIMEOUT_MS {
        warnings.push(json!({
            "code": "step_timeout_clamped",
            "arrival_timeout_ms": arrival,
            "step_timeout_ms": step_timeout,
            "message": format!(
                "control step_timeout_ms is min(arrival timeout, {MAX_STEP_TIMEOUT_MS}): \
                 {arrival} was clamped to {step_timeout}; operations keep expect_after \
                 timeout_ms {arrival}"
            )
        }));
    }
    let timeout_ms = match settings.timeout_ms {
        Some(timeout) => timeout,
        None => {
            let computed =
                default_timeout(&steps, &runs, step_timeout, arrival, application_arrival);
            if computed > MAX_TIMEOUT_MS {
                warnings.push(json!({
                    "code": "task_timeout_clamped",
                    "computed_ms": computed,
                    "timeout_ms": MAX_TIMEOUT_MS,
                    "message": format!(
                        "the default task timeout {computed} ms exceeds {MAX_TIMEOUT_MS} ms and \
                         was clamped; give --timeout-ms to choose another value"
                    )
                }));
                MAX_TIMEOUT_MS
            } else {
                computed
            }
        }
    };

    // R24: the page after a stop is outside the game.
    for step in steps.iter().filter(
        |step| matches!(&step.step.application, Some(application) if application.action == "stop"),
    ) {
        warnings.push(json!({
            "code": "application_stop_target_external",
            "step": step.number,
            "message": "the screen after a stop is the desktop or the launcher: it changes with the \
                        instance's launcher and wallpaper and is reliable only on the instance it \
                        was recorded on"
        }));
    }
    let needs_review = steps
        .iter()
        .filter(|step| {
            step.step
                .click
                .as_ref()
                .is_some_and(|click| click.needs_review)
                || step
                    .step
                    .application
                    .as_ref()
                    .is_some_and(|application| application.needs_review)
        })
        .map(|step| json!({"step": step.number, "record_index": step.step.index}))
        .collect();

    Ok(Plan {
        record_id: recording.record_id.clone(),
        task_id: recording.task_id.clone(),
        game,
        server,
        locale,
        package_id,
        requires: settings.requires.clone(),
        size,
        defaults: recording.defaults.clone(),
        generated_at_unix_ms: recording.updated_at_unix_ms,
        steps,
        runs,
        marks,
        emitted,
        width,
        arrival_timeout_ms: arrival,
        application_arrival_timeout_ms: application_arrival,
        step_timeout_ms: step_timeout,
        timeout_ms,
        warnings,
        needs_review,
    })
}

/// The default task timeout (R24 section 4.3 with Workflow #339 section 2.5):
/// `S·[step 1 is no application entry] + Σ a·(w + (b − w)⁺ + p + T) + Σ_app E
///  + Σ (a − 1)·(r + S) + Σ_runs max(settle) + 10000`. The sums before the settles take every
/// optional step as shown, the longest path; a run settles only when it is skipped, at most
/// once, so it adds its largest settle.
fn default_timeout(
    steps: &[PlanStep],
    runs: &[OptionalRun],
    step_timeout: u64,
    arrival: u64,
    application_arrival: u64,
) -> u64 {
    let mut total = if steps.first().is_some_and(PlanStep::is_entry) {
        0
    } else {
        step_timeout
    };
    for step in steps.iter().filter(|step| step.has_effect()) {
        let application = step.step.application.is_some();
        let (attempts, retry_interval) = match step
            .step
            .click
            .as_ref()
            .and_then(|click| click.retry.as_ref())
        {
            Some(retry) if !application => (u64::from(retry.max_attempts), retry.interval_ms),
            _ => (1, 0),
        };
        let (post_delay, arrival_k) = if application {
            (APPLICATION_POST_DELAY_MS, application_arrival)
        } else {
            (CLICK_POST_DELAY_MS, arrival)
        };
        let (wait, upper) = match step.window() {
            Some((min, max)) => (post_delay.max(min), max),
            None => (post_delay, 0),
        };
        let page_timeout = match &step.step.transition {
            Some(StepTransition::Page { timeout_ms, .. }) => timeout_ms.unwrap_or(arrival_k),
            _ => 0,
        };
        let attempt = wait
            .saturating_add(upper.saturating_sub(wait))
            .saturating_add(page_timeout)
            .saturating_add(arrival_k);
        total = total.saturating_add(attempts.saturating_mul(attempt));
        if application {
            total = total.saturating_add(APPLICATION_MARGIN_MS);
        }
        total = total.saturating_add(
            attempts
                .saturating_sub(1)
                .saturating_mul(retry_interval.saturating_add(step_timeout)),
        );
    }
    for run in runs {
        total = total.saturating_add(run.settle_ms(steps));
    }
    total.saturating_add(TIMEOUT_MARGIN_MS)
}

fn rect_json(rect: RecordRect) -> Value {
    json!({"x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height})
}

fn region_json(rect: RecordRect) -> Value {
    json!({"mode": "rect", "rect": rect_json(rect)})
}

fn mark_region(mark: &RecordedMark) -> LabResult<RecordRect> {
    mark.region.ok_or_else(|| {
        invalid(
            "validation_failed",
            format!("mark '{}' has no region", mark.id),
        )
    })
}

/// The rectangle a guard expects for a mark: a template's search area, a color or digest
/// region, the bounding box of a check's members.
fn guard_rect(plan: &Plan, mark: &RecordedMark) -> LabResult<RecordRect> {
    match mark.family {
        MarkFamily::Template => Ok(mark.search.unwrap_or(mark_region(mark)?)),
        MarkFamily::Check => {
            let mut bounds: Option<(i64, i64, i64, i64)> = None;
            for member in mark.check_members() {
                let member = plan.mark(member)?;
                let rect = match member.family {
                    MarkFamily::Template => member.search.unwrap_or(mark_region(member)?),
                    _ => mark_region(member)?,
                };
                let (left, top) = (i64::from(rect.x), i64::from(rect.y));
                let (right, bottom) = (left + i64::from(rect.width), top + i64::from(rect.height));
                bounds = Some(match bounds {
                    None => (left, top, right, bottom),
                    Some((l, t, r, b)) => (l.min(left), t.min(top), r.max(right), b.max(bottom)),
                });
            }
            let (left, top, right, bottom) = bounds.ok_or_else(|| {
                invalid(
                    "validation_failed",
                    format!("check '{}' has no members", mark.id),
                )
            })?;
            let field = |value: i64| {
                i32::try_from(value)
                    .map_err(|_| invalid("validation_failed", "check bounding box overflow"))
            };
            Ok(RecordRect {
                x: field(left)?,
                y: field(top)?,
                width: field(right - left)?,
                height: field(bottom - top)?,
            })
        }
        _ => mark_region(mark),
    }
}

/// The guard target of a click step (section 4.3), only from the page's required marks:
/// the declared click guard, the click source (not OCR), the first template, the first color
/// or color digest, the first check. `None` when the page has only OCR marks: the click is
/// then a trusted coordinate.
pub(crate) fn guard_target<'p>(
    plan: &'p Plan,
    step: &PlanStep,
) -> LabResult<Option<&'p RecordedMark>> {
    let required = step
        .required
        .iter()
        .map(|id| plan.mark(id))
        .collect::<LabResult<Vec<_>>>()?;
    let guardable = |mark: &&RecordedMark| mark.family != MarkFamily::Ocr;
    let named = |id: Option<&String>| {
        id.and_then(|id| required.iter().copied().find(|mark| &mark.id == id))
            .filter(guardable)
    };
    if let Some(mark) = named(step.step.click_guard.as_ref()) {
        return Ok(Some(mark));
    }
    if let Some(mark) = named(
        step.step
            .click
            .as_ref()
            .and_then(|click| click.from.as_ref()),
    ) {
        return Ok(Some(mark));
    }
    for families in [
        &[MarkFamily::Template][..],
        &[MarkFamily::Color, MarkFamily::ColorDigest][..],
        &[MarkFamily::Check][..],
    ] {
        if let Some(mark) = required
            .iter()
            .copied()
            .find(|mark| families.contains(&mark.family))
        {
            return Ok(Some(mark));
        }
    }
    Ok(None)
}

fn guard_json(plan: &Plan, step: &PlanStep, page: &str) -> LabResult<Option<Value>> {
    let Some(mark) = guard_target(plan, step)? else {
        return Ok(None);
    };
    let mut guard = Map::new();
    guard.insert("page_id".into(), json!(page));
    guard.insert("target_id".into(), json!(mark.id));
    guard.insert("expected_rect".into(), rect_json(guard_rect(plan, mark)?));
    match mark.family {
        MarkFamily::Template => {
            let crop = mark.crop.as_ref().ok_or_else(|| {
                invalid(
                    "validation_failed",
                    format!("template '{}' has no crop", mark.id),
                )
            })?;
            guard.insert("verify_template".into(), json!(crop.asset));
        }
        MarkFamily::Color | MarkFamily::ColorDigest => {
            guard.insert("color_probe".into(), json!(mark.id));
        }
        MarkFamily::Check => {
            guard.insert("check".into(), json!(mark.id));
        }
        MarkFamily::Ocr => return Ok(None),
    }
    Ok(Some(Value::Object(guard)))
}

fn margin_json(margin: &Option<SelfTestMargin>) -> Value {
    match margin {
        None => Value::Null,
        Some(SelfTestMargin::Value(value)) => json!(value),
        Some(SelfTestMargin::Digest {
            mean_milli,
            max_cell,
        }) => json!({"mean_milli": mean_milli, "max_cell": max_cell}),
    }
}

fn mark_summaries(plan: &Plan, own: &[RecordedMark], reused: &[String]) -> LabResult<Vec<Value>> {
    let mut summaries = Vec::new();
    for mark in own {
        summaries.push(json!({
            "id": mark.id,
            "family": mark.family.as_str(),
            "self_test": mark.self_test.status.as_str(),
            "margin": margin_json(&mark.self_test.margin)
        }));
    }
    for id in reused {
        let mark = plan.mark(id)?;
        summaries.push(json!({
            "id": mark.id,
            "family": mark.family.as_str(),
            "reused": true,
            "self_test": mark.self_test.status.as_str(),
            "margin": margin_json(&mark.self_test.margin)
        }));
    }
    Ok(summaries)
}

fn transition_json(step: &PlanStep) -> Option<Value> {
    match &step.step.transition {
        None => None,
        Some(StepTransition::Window { min_ms, max_ms }) => {
            Some(json!({"kind": "window", "min_ms": min_ms, "max_ms": max_ms}))
        }
        Some(StepTransition::Page { timeout_ms, .. }) => {
            let mut transition = Map::new();
            transition.insert("kind".into(), json!("page"));
            transition.insert("page_id".into(), json!(step.transition_page));
            if let Some(timeout) = timeout_ms {
                transition.insert("timeout_ms".into(), json!(timeout));
            }
            Some(Value::Object(transition))
        }
    }
}

/// The `transitions` summary of the output.
pub(crate) fn transitions_summary(plan: &Plan) -> Vec<Value> {
    plan.steps
        .iter()
        .filter_map(|step| match &step.step.transition {
            None => None,
            Some(StepTransition::Window { min_ms, max_ms }) => Some(json!({
                "step": step.number, "kind": "window", "min_ms": min_ms, "max_ms": max_ms
            })),
            Some(StepTransition::Page {
                timeout_ms, source, ..
            }) => Some(json!({
                "step": step.number, "kind": "page", "page": step.transition_page,
                "timeout_ms": timeout_ms, "source": source
            })),
        })
        .collect()
}

/// The `application_steps` summary of the output (R24 section 4.8).
pub(crate) fn application_steps_summary(plan: &Plan) -> Vec<Value> {
    plan.steps
        .iter()
        .filter_map(|step| {
            step.step.application.as_ref().map(|application| {
                json!({
                    "step": step.number,
                    "op": plan.operation_id(step),
                    "action": application.action,
                    "source": application.source
                })
            })
        })
        .collect()
}

fn step_provenance(plan: &Plan, step: &PlanStep) -> LabResult<Value> {
    let mut entry = Map::new();
    entry.insert("step".into(), json!(step.number));
    entry.insert("record_index".into(), json!(step.step.index));
    entry.insert("page".into(), json!(step.page));
    if let Some(primary) = step.primary() {
        entry.insert("frame_sha256".into(), json!(primary.sha256));
        entry.insert(
            "sample_sha256".into(),
            json!(
                step.live_frames()
                    .iter()
                    .filter(|frame| frame.role == "sample")
                    .map(|frame| frame.sha256.clone())
                    .collect::<Vec<_>>()
            ),
        );
        entry.insert("frame_size".into(), json!([primary.width, primary.height]));
        entry.insert(
            "marks".into(),
            json!(mark_summaries(plan, &step.step.marks, &step.step.reused)?),
        );
    }
    if let Some(click) = &step.step.click {
        entry.insert(
            "click".into(),
            json!({
                "rect": rect_json(click.rect),
                "source": click.source,
                "from": click.from,
                "retry": click.retry.as_ref().map(|retry| json!({
                    "max_attempts": retry.max_attempts, "interval_ms": retry.interval_ms
                })),
                "executed": click.execution.as_ref().map(|execution| json!({
                    "point": {"x": execution.point.x, "y": execution.point.y},
                    "point_rule": execution.point_rule,
                    "effect": execution.effect
                })),
                "attempts": click.attempts.len(),
                "needs_review": click.needs_review
            }),
        );
    }
    if let Some(application) = &step.step.application {
        entry.insert(
            "application".into(),
            json!({
                "action": application.action,
                "cli_verb": application.cli_verb,
                "source": application.source,
                "request_id": application.executed.as_ref().map(|executed| executed.request_id.clone())
            }),
        );
    }
    if let Some(optional) = &step.step.optional {
        entry.insert("optional".into(), optional_provenance(optional));
    }
    match &step.step.transition {
        None => {}
        Some(StepTransition::Window { min_ms, max_ms }) => {
            entry.insert(
                "transition".into(),
                json!({"kind": "window", "min_ms": min_ms, "max_ms": max_ms}),
            );
        }
        Some(StepTransition::Page {
            marks,
            reused,
            timeout_ms,
            source,
            converted_step,
            ..
        }) => {
            let frames = step.transition_live_frames();
            entry.insert(
                "transition".into(),
                json!({
                    "kind": "page",
                    "page": step.transition_page,
                    "frame_sha256": frames
                        .iter()
                        .rev()
                        .find(|frame| frame.role == "transition")
                        .map(|frame| frame.sha256.clone()),
                    "sample_sha256": frames
                        .iter()
                        .filter(|frame| frame.role == "transition_sample")
                        .map(|frame| frame.sha256.clone())
                        .collect::<Vec<_>>(),
                    "marks": mark_summaries(plan, marks, reused)?,
                    "timeout_ms": timeout_ms,
                    "source": source,
                    "converted_step": converted_step
                }),
            );
        }
    }
    Ok(Value::Object(entry))
}

/// The provenance of an optional step (Workflow #339 section 2.5).
fn optional_provenance(optional: &StepOptional) -> Value {
    json!({"settle_ms": optional.settle_ms, "marked_at_unix_ms": optional.marked_at_unix_ms})
}

/// The `optional_steps` summary of the output (Workflow #339 section 2.5): every optional step
/// with its operation, settle, skip target and the earlier step of the same pop-up (`same_as`,
/// section 4.7c), by step index.
pub(crate) fn optional_steps_summary(
    plan: &Plan,
    same_as: &BTreeMap<usize, usize>,
) -> LabResult<Vec<Value>> {
    let mut summary = Vec::new();
    for run in &plan.runs {
        let skip_to = plan.steps[run.skip].page_id()?;
        for &member in &run.members {
            let step = &plan.steps[member];
            summary.push(json!({
                "step": step.number,
                "op": plan.operation_id(step),
                "settle_ms": step.settle_ms(),
                "skip_to": skip_to,
                "same_as": same_as.get(&member).map(|&other| plan.steps[other].number)
            }));
        }
    }
    Ok(summary)
}

fn mark_provenance(plan: &Plan, mark: &RecordedMark) -> Value {
    let mut provenance = Map::new();
    provenance.insert("source".into(), json!("lab_record"));
    provenance.insert("record_id".into(), json!(plan.record_id));
    if let Some(step) = plan.steps.iter().find(|step| step.step.index == mark.step) {
        provenance.insert("step".into(), json!(step.number));
    }
    provenance.insert("record_index".into(), json!(mark.step));
    if mark.transition_of.is_some() {
        provenance.insert("transition".into(), json!(true));
    }
    provenance.insert("self_test".into(), json!(mark.self_test.status.as_str()));
    Value::Object(provenance)
}

/// The target families of the task: every emitted mark in its family.
fn target_families(plan: &Plan) -> LabResult<Map<String, Value>> {
    let (mut templates, mut probes, mut ocr, mut checks) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for id in &plan.emitted {
        let mark = plan.mark(id)?;
        match mark.family {
            MarkFamily::Template => {
                let crop = mark.crop.as_ref().ok_or_else(|| {
                    invalid(
                        "validation_failed",
                        format!("template '{}' has no crop", mark.id),
                    )
                })?;
                let mut target = Map::new();
                target.insert("id".into(), json!(mark.id));
                target.insert("template".into(), json!(crop.asset));
                target.insert(
                    "region".into(),
                    region_json(mark.search.unwrap_or(mark_region(mark)?)),
                );
                if let Some(threshold) = mark.threshold {
                    target.insert("threshold".into(), json!(threshold));
                }
                target.insert("provenance".into(), mark_provenance(plan, mark));
                templates.push(Value::Object(target));
            }
            MarkFamily::Color => {
                let mut target = Map::new();
                target.insert("id".into(), json!(mark.id));
                target.insert("region".into(), region_json(mark_region(mark)?));
                target.insert("expected".into(), json!(mark.expected));
                if let Some(distance) = mark.max_distance {
                    target.insert("max_distance".into(), json!(distance));
                }
                target.insert("provenance".into(), mark_provenance(plan, mark));
                probes.push(Value::Object(target));
            }
            MarkFamily::ColorDigest => {
                let mut digest = Map::new();
                digest.insert("algorithm".into(), json!("color_digest.v1"));
                digest.insert("columns".into(), json!(mark.columns));
                digest.insert("rows".into(), json!(mark.rows));
                digest.insert("cells".into(), json!(mark.cells));
                if let Some(exclude) = &mark.exclude_cells {
                    digest.insert("exclude_cells".into(), json!(exclude));
                }
                digest.insert("max_mean_milli".into(), json!(mark.max_mean_milli));
                if let Some(max_cell) = mark.max_cell {
                    digest.insert("max_cell".into(), json!(max_cell));
                }
                let mut target = Map::new();
                target.insert("id".into(), json!(mark.id));
                target.insert("region".into(), region_json(mark_region(mark)?));
                target.insert("digest".into(), Value::Object(digest));
                target.insert("provenance".into(), mark_provenance(plan, mark));
                probes.push(Value::Object(target));
            }
            MarkFamily::Ocr => {
                let spec = mark.ocr.as_ref().ok_or_else(|| {
                    invalid(
                        "validation_failed",
                        format!("ocr '{}' has no OCR fields", mark.id),
                    )
                })?;
                ocr.push(json!({
                    "id": mark.id,
                    "region": region_json(mark_region(mark)?),
                    "languages": spec.languages,
                    "timeout_ms": spec.timeout_ms,
                    "match_mode": spec.match_mode,
                    "expected": spec.expected,
                    "case_sensitive": spec.case_sensitive,
                    "minimum_confidence": spec.minimum_confidence,
                    "model_ref": spec.model_ref,
                    "model_sha256": spec.model_sha256
                }));
            }
            MarkFamily::Check => {
                let mut target = Map::new();
                target.insert("id".into(), json!(mark.id));
                match (&mark.all_of, &mark.any_of) {
                    (Some(members), _) => target.insert("all_of".into(), json!(members)),
                    (None, Some(members)) => target.insert("any_of".into(), json!(members)),
                    (None, None) => {
                        return Err(invalid(
                            "validation_failed",
                            format!("check '{}' has no members", mark.id),
                        ));
                    }
                };
                checks.push(Value::Object(target));
            }
        }
    }
    let mut families = Map::new();
    for (name, targets) in [
        ("verify_templates", templates),
        ("color_probes", probes),
        ("ocr_targets", ocr),
        ("checks", checks),
    ] {
        if !targets.is_empty() {
            families.insert(name.into(), Value::Array(targets));
        }
    }
    Ok(families)
}

fn operation_json(plan: &Plan, index: usize) -> LabResult<Value> {
    let step = &plan.steps[index];
    let next_page = plan.next_page(index)?;
    let id = plan.operation_id(step);
    let mut operation = Map::new();
    operation.insert("id".into(), json!(id));
    let mut provenance = Map::new();
    provenance.insert("source".into(), json!("lab_record"));
    provenance.insert("record_id".into(), json!(plan.record_id));
    provenance.insert("step".into(), json!(step.number));
    provenance.insert("record_index".into(), json!(step.step.index));
    if let Some(primary) = step.primary() {
        provenance.insert("frame_sha256".into(), json!(primary.sha256));
    }
    if let Some(application) = &step.step.application {
        operation.insert(
            "purpose".into(),
            json!(format!(
                "Lab recording step {} (application {})",
                step.number, application.action
            )),
        );
        operation.insert(
            "from".into(),
            json!(step.page.clone().unwrap_or_else(|| "any".to_string())),
        );
        operation.insert("to".into(), json!(next_page));
        operation.insert("application".into(), json!({"action": application.action}));
        operation.insert(
            "expect_after".into(),
            json!({
                "page_id": next_page,
                "timeout_ms": plan.application_arrival_timeout_ms,
                "interval_ms": ARRIVAL_INTERVAL_MS
            }),
        );
        operation.insert("post_delay_ms".into(), json!(APPLICATION_POST_DELAY_MS));
        provenance.insert(
            "application".into(),
            json!({
                "source": application.source,
                "request_id": application.executed.as_ref().map(|executed| executed.request_id.clone())
            }),
        );
    } else {
        let click = step.step.click.as_ref().ok_or_else(|| {
            blocked(
                "record_step_click_missing",
                format!("step {} has no effect", step.step.index),
            )
        })?;
        let page = step.page.clone().ok_or_else(|| {
            blocked(
                "record_step_frame_missing",
                format!("step {} has a click but no page", step.step.index),
            )
        })?;
        operation.insert(
            "purpose".into(),
            json!(format!("Lab recording step {}", step.number)),
        );
        operation.insert("from".into(), json!(page));
        operation.insert("to".into(), json!(next_page));
        operation.insert(
            "click".into(),
            json!({
                "kind": "rect",
                "x": click.rect.x,
                "y": click.rect.y,
                "width": click.rect.width,
                "height": click.rect.height
            }),
        );
        match guard_json(plan, step, &page)? {
            Some(guard) => {
                operation.insert("guard".into(), guard);
            }
            None => {
                operation.insert("unguarded_trusted_coordinate".into(), json!(true));
            }
        }
        operation.insert(
            "expect_after".into(),
            json!({
                "page_id": next_page,
                "timeout_ms": plan.arrival_timeout_ms,
                "interval_ms": ARRIVAL_INTERVAL_MS
            }),
        );
        operation.insert("post_delay_ms".into(), json!(CLICK_POST_DELAY_MS));
        if let Some(retry) = &click.retry {
            operation.insert("retryable".into(), json!(true));
            operation.insert("max_attempts".into(), json!(retry.max_attempts));
            operation.insert("retry_interval_ms".into(), json!(retry.interval_ms));
        }
    }
    // Workflow #339: the step's page may not appear; the operation is then skipped.
    if let Some(optional) = &step.step.optional {
        operation.insert("optional".into(), json!({"settle_ms": optional.settle_ms}));
        provenance.insert("optional".into(), optional_provenance(optional));
    }
    if let Some(transition) = transition_json(step) {
        operation.insert("transition".into(), transition);
    }
    operation.insert("provenance".into(), Value::Object(provenance));
    Ok(Value::Object(operation))
}

fn page_rules(plan: &Plan) -> Map<String, Value> {
    let mut rules = Map::new();
    for step in &plan.steps {
        if let Some(page) = &step.page {
            rules.insert(page.clone(), json!({"required": step.required}));
        }
        if let Some(page) = &step.transition_page {
            rules.insert(page.clone(), json!({"required": step.transition_required}));
        }
    }
    rules
}

fn pretty(value: &Value) -> LabResult<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        invalid(
            "validation_failed",
            format!("failed to encode a package document: {error}"),
        )
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// The content table of the package (section 4.5): control.json, resources.json, task.json
/// and the template crops. `checks` carries the self-check results the provenance records
/// (`warnings`, `arrival_by_time_window`, `cross_check`); the first pass has none.
pub(crate) fn render(
    plan: &Plan,
    checks: Option<&Map<String, Value>>,
    crops: &BTreeMap<String, Vec<u8>>,
) -> LabResult<BTreeMap<String, Vec<u8>>> {
    let first = plan
        .steps
        .first()
        .ok_or_else(|| invalid("validation_failed", "the package has no step"))?;
    let last = plan
        .steps
        .last()
        .ok_or_else(|| invalid("validation_failed", "the package has no step"))?;
    let target = last
        .page
        .clone()
        .ok_or_else(|| blocked("record_step_frame_missing", "the last step has no page"))?;
    let effect_steps = plan.effect_steps();
    let application_steps = plan
        .steps
        .iter()
        .filter(|step| step.step.application.is_some())
        .count();
    let click_steps = effect_steps - application_steps;

    let mut control = Map::new();
    control.insert("schema_version".into(), json!(CONTROL_SCHEMA));
    control.insert("package_id".into(), json!(plan.package_id));
    control.insert("execution_mode".into(), json!("linear_steps"));
    control.insert("game".into(), json!(plan.game));
    control.insert("server".into(), json!(plan.server));
    control.insert(
        "resolution".into(),
        json!({"width": plan.size.width, "height": plan.size.height}),
    );
    control.insert("entry_task_id".into(), json!(plan.task_id));
    if let Some(requires) = &plan.requires {
        control.insert("prerequisite_package_id".into(), json!(requires));
    }
    control.insert("timeout_ms".into(), json!(plan.timeout_ms));
    control.insert("step_timeout_ms".into(), json!(plan.step_timeout_ms));
    control.insert("max_steps".into(), json!(effect_steps));

    let resources = json!({"schema_version": "1.0", "resources": [], "resource_count": 0});

    let mut provenance = Map::new();
    provenance.insert("source".into(), json!("lab_record"));
    provenance.insert("record_id".into(), json!(plan.record_id));
    provenance.insert(
        "recording_schema".into(),
        json!(super::model::LAB_RECORDING_SCHEMA),
    );
    provenance.insert("generator".into(), json!("actinglab record stop"));
    provenance.insert(
        "generated_at_unix_ms".into(),
        json!(plan.generated_at_unix_ms),
    );
    provenance.insert(
        "steps".into(),
        Value::Array(
            plan.steps
                .iter()
                .map(|step| step_provenance(plan, step))
                .collect::<LabResult<Vec<_>>>()?,
        ),
    );
    if let Some(checks) = checks {
        for (key, value) in checks {
            provenance.insert(key.clone(), value.clone());
        }
    }

    let mut task = Map::new();
    task.insert("schema_version".into(), json!(TASK_SCHEMA));
    task.insert("task_id".into(), json!(plan.task_id));
    task.insert("game".into(), json!(plan.game));
    task.insert("server_scope".into(), json!([plan.server]));
    task.insert("locale".into(), json!(plan.locale));
    let goal = if application_steps == 0 {
        format!(
            "Lab recording {}: {click_steps} click step(s)",
            plan.record_id
        )
    } else {
        format!(
            "Lab recording {}: {click_steps} click step(s), {application_steps} application \
             step(s)",
            plan.record_id
        )
    };
    task.insert("goal".into(), json!(goal));
    task.insert("provenance".into(), Value::Object(provenance));
    task.insert(
        "coordinate_space".into(),
        json!({"width": plan.size.width, "height": plan.size.height}),
    );
    task.insert(
        "defaults".into(),
        json!({
            "template_threshold": plan.defaults.template_threshold,
            "color_max_distance": plan.defaults.color_max_distance,
            "match_metric": plan.defaults.match_metric
        }),
    );
    task.insert("timeout_ms".into(), json!(plan.timeout_ms));
    task.insert("max_steps".into(), json!(effect_steps));
    task.insert(
        "entry_page".into(),
        json!(first.page.clone().unwrap_or_else(|| "any".to_string())),
    );
    task.insert("target_page".into(), json!(target));
    task.insert(
        "scheduling_outcome".into(),
        json!({"mappings": [{
            "outcome_key": format!("{}_done", plan.task_id),
            "effect": "no_designated_effect",
            "terminal_pages": [target]
        }]}),
    );
    for (name, targets) in target_families(plan)? {
        task.insert(name, targets);
    }
    task.insert("page_rules".into(), Value::Object(page_rules(plan)));
    let mut operations = Vec::new();
    for (index, step) in plan.steps.iter().enumerate() {
        if step.has_effect() {
            operations.push(operation_json(plan, index)?);
        }
    }
    task.insert("operations".into(), Value::Array(operations));

    let task_dir = plan.task_dir();
    let mut entries = BTreeMap::new();
    entries.insert("control.json".to_string(), pretty(&Value::Object(control))?);
    entries.insert(
        "resources/operations/resources.json".to_string(),
        pretty(&resources)?,
    );
    entries.insert(
        format!("{task_dir}/task.json"),
        pretty(&Value::Object(task))?,
    );
    for (id, asset) in plan.template_assets() {
        let bytes = crops.get(&id).ok_or_else(|| {
            invalid(
                "validation_failed",
                format!("the crop of template '{id}' was not loaded"),
            )
        })?;
        entries.insert(format!("{task_dir}/{asset}"), bytes.clone());
    }
    Ok(entries)
}
