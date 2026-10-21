// SPDX-License-Identifier: AGPL-3.0-only

//! Rust mainline data structures for declarative task-flow contracts.

use crate::codes::{SharedCode, TaskflowCode};
use crate::types::*;
use crate::{SchedulingEffectCondition, SchedulingOutcomeDeclaration};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Explicit operation postcondition polling budget; independent of step timeout.
pub const MAX_POSTCONDITION_TIMEOUT_MS: u64 = MAX_CONTAINED_TASK_TIMEOUT_MS;
pub const MAX_CONTAINED_TASK_TIMEOUT_MS: u64 = 1_800_000;
pub const PHASED_CONTROL_SCHEMA: &str = "Lab-1y.control.v2";
pub const MAX_TASK_PHASES: usize = 32;
/// The most pages one stage's `end_pages` names (#411 B5).
pub const MAX_PHASE_END_PAGES: usize = 16;

/// One linear stage inside the same task, run and lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskPhase {
    pub id: String,
    pub operations: Vec<String>,
    pub target_pages: Vec<String>,
    /// #411 B5/B24: pages on which the stage ends the run, Completed there: a page an operation
    /// of this stage confirms, or (B20) an entry page the run enters on. 1..=16 pages, none of
    /// them a target page of the stage or an error page. Absent, the stage serializes as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_pages: Option<Vec<String>>,
    /// #411 B14: pages of the task's entry page set that start this stage, besides the stage an
    /// operation's `from` infers. Absent, the stage serializes as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_pages: Option<Vec<String>>,
}

impl TaskPhase {
    /// #411 B5/B24/B20: whether `page` is one of the stage's `end_pages`, both compared without
    /// the exact `<game>/` prefix. The one end-page test of the static walk, the run (a page an
    /// operation of the stage confirms) and the run's entry observation.
    pub fn is_end_page(&self, game: &str, page: &str) -> bool {
        let prefix = format!("{game}/");
        let page = page.strip_prefix(&prefix).unwrap_or(page);
        self.end_pages.iter().flatten().any(|end| {
            let end = end.as_str();
            end.strip_prefix(&prefix).unwrap_or(end) == page
        })
    }
}

pub fn validate_task_phases(
    phases: &[TaskPhase],
    operation_ids: &[&str],
) -> Result<(), crate::outcome::Code> {
    locate_task_phases(phases, operation_ids, &|page: &str| page.to_owned())
        .map_err(|refusal| refusal.code)
}

// --- #411 A20: located phase refusals (begin) ---

/// The rule a refused member of a phased route breaks (#411 A20): the `phase_problem` token of
/// the refusal's developer detail only, neither an outcome value nor a vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskPhaseProblem {
    PhaseCount,
    PhaseId,
    OperationCount,
    OperationRepeated,
    OperationUnknown,
    TargetCount,
    TargetRepeated,
    TargetInvalid,
    EntryInvalid,
    FinalTargets,
    NoOperation,
    NoTerminal,
    /// #411 B5: `end_pages` is empty or names more than 16 pages.
    EndPagesCount,
    /// #411 B5: an end page is named twice.
    EndPagesRepeated,
    /// #411 B5: an end page is blank, `any`, a target page of its stage or an error page.
    EndPagesInvalid,
    /// #411 B14: `entry_pages` is empty or names more than 16 pages.
    EntryPagesCount,
    /// #411 B14: an entry page is named twice.
    EntryPagesRepeated,
    /// #411 B14: an entry page is blank, `any`, or no page of the task's entry that starts the
    /// stage.
    EntryPagesInvalid,
}

impl TaskPhaseProblem {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PhaseCount => "phase_count",
            Self::PhaseId => "phase_id",
            Self::OperationCount => "operation_count",
            Self::OperationRepeated => "operation_repeated",
            Self::OperationUnknown => "operation_unknown",
            Self::TargetCount => "target_count",
            Self::TargetRepeated => "target_repeated",
            Self::TargetInvalid => "target_invalid",
            Self::EntryInvalid => "entry_invalid",
            Self::FinalTargets => "final_targets",
            Self::NoOperation => "no_operation",
            Self::NoTerminal => "no_terminal",
            Self::EndPagesCount => "end_pages_count",
            Self::EndPagesRepeated => "end_pages_repeated",
            Self::EndPagesInvalid => "end_pages_invalid",
            Self::EntryPagesCount => "entry_pages_count",
            Self::EntryPagesRepeated => "entry_pages_repeated",
            Self::EntryPagesInvalid => "entry_pages_invalid",
        }
    }
}

/// Where a refusal of a phased route lies (#411 A20): the rule, the refused member as a JSON
/// pointer into the task (`/phases/<index>…` or `/entry_page`) and, where the rule concerns
/// one, the stage id, the page (as the walk compares it: without the `<game>/` prefix), the
/// operation and, for a stage that cannot leave a page, the start the walk came from. A name
/// that is blank or could not be an outcome value (a control character, over 256 bytes) is
/// left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskPhaseLocation {
    pub problem: TaskPhaseProblem,
    pub json_pointer: String,
    pub phase_id: Option<String>,
    pub page_id: Option<String>,
    pub pack_operation_id: Option<String>,
    pub entry_page: Option<String>,
}

impl TaskPhaseLocation {
    /// The developer detail of the refusal, the same for the Runtime and the source build:
    /// `phase_problem=<token> json_pointer=<pointer>[ phase_id=<id>][ page_id=<page>]
    /// [ pack_operation_id=<id>][ entry_page=<page>]`.
    pub fn detail(&self) -> String {
        let mut detail = vec![
            format!("phase_problem={}", self.problem.as_str()),
            format!("json_pointer={}", self.json_pointer),
        ];
        detail.extend(
            [
                ("phase_id", &self.phase_id),
                ("page_id", &self.page_id),
                ("pack_operation_id", &self.pack_operation_id),
                ("entry_page", &self.entry_page),
            ]
            .into_iter()
            .filter_map(|(key, value)| value.as_ref().map(|value| format!("{key}={value}"))),
        );
        detail.join(" ")
    }

    fn page(mut self, page: String) -> Self {
        self.page_id = shown(page);
        self
    }

    fn pack_operation(mut self, id: Option<&str>) -> Self {
        self.pack_operation_id = id.map(str::to_owned).and_then(shown);
        self
    }
}

/// A refusal of a phased route's checks (#411 A20): its code and, for `task_phases_invalid` and
/// `task_phases_unreachable`, where it lies. The other codes carry no location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskPhaseRefusal {
    pub code: crate::outcome::Code,
    pub location: Option<Box<TaskPhaseLocation>>,
}

impl From<crate::outcome::Code> for TaskPhaseRefusal {
    fn from(code: crate::outcome::Code) -> Self {
        Self {
            code,
            location: None,
        }
    }
}

impl TaskPhaseRefusal {
    fn located(code: TaskflowCode, location: TaskPhaseLocation) -> Self {
        Self {
            code: code.code(),
            location: Some(Box::new(location)),
        }
    }
}

/// The label rule of a stage id and a stage target page.
fn phase_label(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

/// A name a location shows: not blank, and fit for an outcome value of an id key.
fn shown(value: String) -> Option<String> {
    (!value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control))
        .then_some(value)
}

/// The location of the refused member `json_pointer`; under `/phases/<index>` it names the
/// stage id, when that id passes the label rule.
fn phase_location(
    problem: TaskPhaseProblem,
    json_pointer: String,
    phase: Option<&TaskPhase>,
) -> TaskPhaseLocation {
    TaskPhaseLocation {
        problem,
        json_pointer,
        phase_id: phase
            .filter(|phase| phase_label(&phase.id))
            .map(|phase| phase.id.clone()),
        page_id: None,
        pack_operation_id: None,
        entry_page: None,
    }
}

/// [`validate_task_phases`] with the refused member located (#411 A20); `game` only shapes how
/// a page is named.
pub fn validate_task_phases_located(
    game: &str,
    phases: &[TaskPhase],
    operation_ids: &[&str],
) -> Result<(), TaskPhaseRefusal> {
    let prefix = format!("{game}/");
    locate_task_phases(phases, operation_ids, &|page: &str| {
        page.strip_prefix(&prefix).unwrap_or(page).to_owned()
    })
}

/// The stage declarations' check (#411 A20): members in declaration order (stage count; then
/// stage by stage its id, operation count, each operation, target count, each target page and,
/// #411 B5/B14, its end pages and its entry pages); the first refused member is named.
fn locate_task_phases(
    phases: &[TaskPhase],
    operation_ids: &[&str],
    canonical: &dyn Fn(&str) -> String,
) -> Result<(), TaskPhaseRefusal> {
    let refused = |location| TaskPhaseRefusal::located(TaskflowCode::TaskPhasesInvalid, location);
    if phases.is_empty() || phases.len() > MAX_TASK_PHASES {
        return Err(refused(phase_location(
            TaskPhaseProblem::PhaseCount,
            "/phases".to_owned(),
            None,
        )));
    }
    let mut ids = std::collections::BTreeSet::new();
    for (index, phase) in phases.iter().enumerate() {
        let at = |problem, member: String| {
            phase_location(problem, format!("/phases/{index}/{member}"), Some(phase))
        };
        if !phase_label(&phase.id) || !ids.insert(&phase.id) {
            return Err(refused(at(TaskPhaseProblem::PhaseId, "id".to_owned())));
        }
        if phase.operations.is_empty() || phase.operations.len() > operation_ids.len() {
            return Err(refused(at(
                TaskPhaseProblem::OperationCount,
                "operations".to_owned(),
            )));
        }
        let mut operations = std::collections::BTreeSet::new();
        for (member, id) in phase.operations.iter().enumerate() {
            let problem = if !operations.insert(id) {
                TaskPhaseProblem::OperationRepeated
            } else if !operation_ids.contains(&id.as_str()) {
                TaskPhaseProblem::OperationUnknown
            } else {
                continue;
            };
            return Err(refused(
                at(problem, format!("operations/{member}")).pack_operation(Some(id.as_str())),
            ));
        }
        if phase.target_pages.is_empty() || phase.target_pages.len() > 256 {
            return Err(refused(at(
                TaskPhaseProblem::TargetCount,
                "target_pages".to_owned(),
            )));
        }
        let mut targets = std::collections::BTreeSet::new();
        for (member, page) in phase.target_pages.iter().enumerate() {
            let problem = if !targets.insert(page) {
                TaskPhaseProblem::TargetRepeated
            } else if !phase_label(page) || page == "any" {
                TaskPhaseProblem::TargetInvalid
            } else {
                continue;
            };
            return Err(refused(
                at(problem, format!("target_pages/{member}")).page(canonical(page)),
            ));
        }
        // #411 B5: 1..=16 end pages, none repeated, blank, `any` or a target page of the stage.
        if let Some(ends) = &phase.end_pages {
            if ends.is_empty() || ends.len() > MAX_PHASE_END_PAGES {
                return Err(refused(at(
                    TaskPhaseProblem::EndPagesCount,
                    "end_pages".to_owned(),
                )));
            }
            let mut seen = std::collections::BTreeSet::new();
            for (member, page) in ends.iter().enumerate() {
                let problem = if !seen.insert(canonical(page)) {
                    TaskPhaseProblem::EndPagesRepeated
                } else if !phase_label(page)
                    || page == "any"
                    || phase
                        .target_pages
                        .iter()
                        .any(|target| canonical(target) == canonical(page))
                {
                    TaskPhaseProblem::EndPagesInvalid
                } else {
                    continue;
                };
                return Err(refused(
                    at(problem, format!("end_pages/{member}")).page(canonical(page)),
                ));
            }
        }
        // #411 B14: 1..=16 entry pages, none repeated, blank or `any`.
        if let Some(starts) = &phase.entry_pages {
            if starts.is_empty() || starts.len() > MAX_ENTRY_PAGES {
                return Err(refused(at(
                    TaskPhaseProblem::EntryPagesCount,
                    "entry_pages".to_owned(),
                )));
            }
            let mut seen = std::collections::BTreeSet::new();
            for (member, page) in starts.iter().enumerate() {
                let problem = if !seen.insert(canonical(page)) {
                    TaskPhaseProblem::EntryPagesRepeated
                } else if !phase_label(page) || page == "any" {
                    TaskPhaseProblem::EntryPagesInvalid
                } else {
                    continue;
                };
                return Err(refused(
                    at(problem, format!("entry_pages/{member}")).page(canonical(page)),
                ));
            }
        }
    }
    Ok(())
}

/// #411 B5: no stage's end page is one of the task's error pages; a refusal is
/// `task_phases_invalid` at the end page (rule `end_pages_invalid`). Source parsing, build and
/// Runtime admission share this check; `game` only shapes how a page is named and compared.
pub fn validate_phase_end_pages_located(
    game: &str,
    phases: &[TaskPhase],
    error_pages: &[String],
) -> Result<(), TaskPhaseRefusal> {
    let prefix = format!("{game}/");
    let canonical = |page: &str| page.strip_prefix(&prefix).unwrap_or(page).to_owned();
    for (index, phase) in phases.iter().enumerate() {
        for (member, page) in phase.end_pages.iter().flatten().enumerate() {
            if error_pages
                .iter()
                .any(|error| canonical(error) == canonical(page))
            {
                return Err(TaskPhaseRefusal::located(
                    TaskflowCode::TaskPhasesInvalid,
                    phase_location(
                        TaskPhaseProblem::EndPagesInvalid,
                        format!("/phases/{index}/end_pages/{member}"),
                        Some(phase),
                    )
                    .page(canonical(page)),
                ));
            }
        }
    }
    Ok(())
}

// --- #411 A20: located phase refusals (end) ---

/// Recorded on the existing step chain; completed means this step advances the cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskPhaseEvidence {
    pub index: u32,
    pub id: String,
    pub completed: bool,
}

/// A view of already parsed operations for the common phase reachability check.
pub struct TaskPhaseOperation<'a> {
    pub id: &'a str,
    pub from: &'a str,
    pub destinations: Vec<String>,
    pub retryable: bool,
    /// Workflow #390 EXEC-1: the operation declares `when` or `on_guard_refused`, so at run time
    /// the page's next candidate may run instead of it.
    pub may_be_passed_over: bool,
    /// Workflow #308 SEL-L4b (frozen selector model S0 §1.4 (c)): the outcome key of the select
    /// step's `on_empty`, when it declares one; the run may end there, on the step's own page.
    pub on_empty_key: Option<&'a str>,
}

// --- Workflow #373 NAV-3b: entry page sets (begin) ---

/// The most pages one task entry page set names (Workflow #373 NAV-3b).
pub const MAX_ENTRY_PAGES: usize = 16;

crate::outcome_vocabulary! {
    /// Why a task's entry page set is refused at admission (vocabulary `entry_problem`,
    /// Workflow #373 NAV-3b).
    pub enum EntryProblem: "entry_problem" {
        Empty => "empty",
        TooMany => "too_many",
        Duplicate => "duplicate",
        AnyInSet => "any_in_set",
        Unresolved => "unresolved",
        PhaseStartMissing => "phase_start_missing",
        PhaseStartAmbiguous => "phase_start_ambiguous",
    }
}

/// One start of a phased route: the stage the route enters at and the page it enters on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskPhaseEntry<'a> {
    pub phase: usize,
    pub page: &'a str,
}

/// The shape of a task's entry page set before its pages are resolved (Workflow #373 NAV-3b):
/// 1..=16 labels, none of them `any` or blank, and no page named twice once the exact game
/// prefix is removed. Source parsing, build and Runtime admission share this check.
pub fn validate_entry_page_set(game: &str, pages: &[String]) -> Result<(), EntryProblem> {
    if pages.is_empty() {
        return Err(EntryProblem::Empty);
    }
    if pages.len() > MAX_ENTRY_PAGES {
        return Err(EntryProblem::TooMany);
    }
    let prefix = format!("{game}/");
    let canonical = |page: &str| page.strip_prefix(&prefix).unwrap_or(page).to_owned();
    if pages.iter().any(|page| canonical(page) == "any") {
        return Err(EntryProblem::AnyInSet);
    }
    if pages.iter().any(|page| canonical(page).trim().is_empty()) {
        return Err(EntryProblem::Unresolved);
    }
    let mut seen = std::collections::BTreeSet::new();
    if !pages.iter().all(|page| seen.insert(canonical(page))) {
        return Err(EntryProblem::Duplicate);
    }
    Ok(())
}

/// The stage a phased route enters at for one page of its entry page set (Workflow #373
/// NAV-3b): the one stage that lists an operation whose `from` is that page, or (#411 B14) that
/// lists the page in its `entry_pages`, or (#411 B20) that lists it in its `end_pages`, where the
/// run then ends at once. No start, or more than one (a page that is an end page and also an
/// operation's `from` has two), refuses the set.
pub fn phase_entry_start(
    game: &str,
    page: &str,
    phases: &[TaskPhase],
    operations: &[TaskPhaseOperation<'_>],
) -> Result<usize, EntryProblem> {
    match entry_starts(game, page, phases, operations).as_slice() {
        [index] => Ok(*index),
        [] => Err(EntryProblem::PhaseStartMissing),
        _ => Err(EntryProblem::PhaseStartAmbiguous),
    }
}

/// Every stage that starts `page`, by index in declaration order, each stage once (Workflow
/// #373 NAV-3b, #411 A17, B14, B20): the stages behind the starts `phase_entry_start` decides
/// on, so that a refused stage start can name the stages that start the page.
pub fn phase_entry_starts(
    game: &str,
    page: &str,
    phases: &[TaskPhase],
    operations: &[TaskPhaseOperation<'_>],
) -> Vec<usize> {
    let mut stages = entry_starts(game, page, phases, operations);
    stages.dedup();
    stages
}

/// The starts of `page` in declaration order: a stage that lists an operation whose `from` is
/// the page or (#411 B14) lists the page in its `entry_pages` is one start; (#411 B20) a stage
/// that lists the page in its `end_pages` is another one of its own, the run's end at once.
fn entry_starts(
    game: &str,
    page: &str,
    phases: &[TaskPhase],
    operations: &[TaskPhaseOperation<'_>],
) -> Vec<usize> {
    let prefix = format!("{game}/");
    let canonical = |page: &str| page.strip_prefix(&prefix).unwrap_or(page).to_owned();
    let page = canonical(page);
    let mut starts = Vec::new();
    for (index, phase) in phases.iter().enumerate() {
        let inferred = operations.iter().any(|operation| {
            phase.operations.iter().any(|id| id == operation.id)
                && operation.from != "any"
                && canonical(operation.from) == page
        });
        let listed = phase
            .entry_pages
            .iter()
            .flatten()
            .any(|listed| canonical(listed) == page);
        if inferred || listed {
            starts.push(index);
        }
        if phase.is_end_page(game, &page) {
            starts.push(index);
        }
    }
    starts
}

// --- Workflow #373 NAV-3b: entry page sets (end) ---

/// The static reachability check of a phased route. Workflow #373 NAV-3b: a route may enter at
/// several starts (an entry page set); the check runs once from each start, and each must
/// reach a mapped terminal.
pub fn validate_phased_route(
    game: &str,
    entries: &[TaskPhaseEntry<'_>],
    phases: &[TaskPhase],
    target_pages: &[String],
    operations: &[TaskPhaseOperation<'_>],
    scheduling: &SchedulingOutcomeDeclaration,
) -> Result<(), crate::outcome::Code> {
    validate_phased_route_located(game, entries, phases, target_pages, operations, scheduling)
        .map_err(|refusal| refusal.code)
}

/// [`validate_phased_route`] with the refused member located (#411 A20): after the stage
/// declarations, the entries, then (#411 B14) each stage's entry pages, then the last stage's
/// target pages, then the walk from each start in order; a refusal of `task_phases_invalid` or
/// `task_phases_unreachable` names the first refused member.
pub fn validate_phased_route_located(
    game: &str,
    entries: &[TaskPhaseEntry<'_>],
    phases: &[TaskPhase],
    target_pages: &[String],
    operations: &[TaskPhaseOperation<'_>],
    scheduling: &SchedulingOutcomeDeclaration,
) -> Result<(), TaskPhaseRefusal> {
    let prefix = format!("{game}/");
    let canonical = |page: &str| page.strip_prefix(&prefix).unwrap_or(page).to_owned();
    locate_task_phases(
        phases,
        &operations
            .iter()
            .map(|operation| operation.id)
            .collect::<Vec<_>>(),
        &canonical,
    )?;
    scheduling
        .validate()
        .map_err(|_| TaskflowCode::TaskPhasesOutcomeInvalid.code())?;
    let pages = |values: &[String]| {
        values
            .iter()
            .map(|page| canonical(page))
            .collect::<std::collections::BTreeSet<_>>()
    };
    let entry_invalid = |entry: Option<&TaskPhaseEntry<'_>>| {
        let location = phase_location(
            TaskPhaseProblem::EntryInvalid,
            "/entry_page".to_owned(),
            None,
        );
        TaskPhaseRefusal::located(
            TaskflowCode::TaskPhasesInvalid,
            match entry {
                Some(entry) => location.page(canonical(entry.page)),
                None => location,
            },
        )
    };
    if entries.is_empty() {
        return Err(entry_invalid(None));
    }
    if let Some(entry) = entries.iter().find(|entry| {
        entry.page.trim().is_empty() || entry.page == "any" || entry.phase >= phases.len()
    }) {
        return Err(entry_invalid(Some(entry)));
    }
    // #411 B14 (D4): a page a stage lists in `entry_pages` is a page of the task's entry (its
    // entry page set) that starts that stage; a single entry page starts the first stage.
    for (index, phase) in phases.iter().enumerate() {
        for (member, page) in phase.entry_pages.iter().flatten().enumerate() {
            if !entries
                .iter()
                .any(|entry| entry.phase == index && canonical(entry.page) == canonical(page))
            {
                return Err(TaskPhaseRefusal::located(
                    TaskflowCode::TaskPhasesInvalid,
                    phase_location(
                        TaskPhaseProblem::EntryPagesInvalid,
                        format!("/phases/{index}/entry_pages/{member}"),
                        Some(phase),
                    )
                    .page(canonical(page)),
                ));
            }
        }
    }
    let last = phases.len() - 1;
    if pages(target_pages) != pages(&phases[last].target_pages) {
        return Err(TaskPhaseRefusal::located(
            TaskflowCode::TaskPhasesInvalid,
            phase_location(
                TaskPhaseProblem::FinalTargets,
                format!("/phases/{last}/target_pages"),
                Some(&phases[last]),
            ),
        ));
    }
    // Workflow #390 OUT-2: every designated operation of either form names one operation and is
    // not retryable.
    let designated = scheduling.designated_operations();
    if designated.iter().any(|id| {
        operations
            .iter()
            .filter(|operation| operation.id == *id)
            .count()
            != 1
    }) || operations
        .iter()
        .any(|operation| designated.contains(&operation.id) && operation.retryable)
    {
        return Err(TaskflowCode::TaskPhasesDesignatedEffectInvalid
            .code()
            .into());
    }
    for entry in entries {
        validate_phased_route_from(game, entry, phases, operations, scheduling, &canonical)?;
    }
    Ok(())
}

/// Today's reachability check of a phased route, from one start. Workflow #390 OUT-2: the state
/// carries the designated effects that completed on the way (a bit mask); a member of a
/// designated effect set that may have reached its cap is passed over, and the stage's next
/// operation from the page is followed too. Workflow #390 EXEC-1: so is a candidate that may be
/// passed over (`when`, `on_guard_refused`); a page left without an operation after such
/// candidates only ends that path (the run's `contained_task_operation_candidates_exhausted`),
/// while one left without an operation once a member may be capped stays unreachable. #411 A20:
/// each queued state carries the operation whose destination produced it (none at the start),
/// so the first path that reaches a state names it; an unreachable refusal names the stage, the
/// page, that operation and the start. #411 B5: a destination an operation of a stage confirms
/// that is one of the stage's end pages is a terminal there, and the path ends; #411 B20: so is
/// a start whose page is an end page of the stage it starts, under no designated effect. A stage
/// entered by advancing does not test its own end pages on the advancing page (D5).
fn validate_phased_route_from(
    game: &str,
    entry: &TaskPhaseEntry<'_>,
    phases: &[TaskPhase],
    operations: &[TaskPhaseOperation<'_>],
    scheduling: &SchedulingOutcomeDeclaration,
    canonical: &dyn Fn(&str) -> String,
) -> Result<(), TaskPhaseRefusal> {
    let reach = scheduling.designated_effect_reach();
    let start = canonical(entry.page);
    if phases
        .get(entry.phase)
        .is_some_and(|phase| phase.is_end_page(game, &start))
    {
        return end_page_terminal(scheduling, 0, &start, canonical).map_err(TaskPhaseRefusal::from);
    }
    let stage_location = |problem, index: usize| {
        phase_location(problem, format!("/phases/{index}"), phases.get(index))
    };
    let mut pending =
        std::collections::VecDeque::from([(entry.phase, start.clone(), 0_u16, None::<&str>)]);
    let mut visited = std::collections::BTreeSet::new();
    let mut terminals = 0;
    while let Some((index, page, completed, via)) = pending.pop_front() {
        if !visited.insert((index, page.clone(), completed)) {
            continue;
        }
        if index == phases.len() {
            let condition = crate::DesignatedEffectReach::condition(completed);
            if scheduling
                .mappings()
                .iter()
                .filter(|mapping| {
                    mapping.effect() == condition
                        && mapping
                            .terminal_pages()
                            .iter()
                            .any(|target| canonical(target) == page)
                })
                .count()
                != 1
            {
                return Err(TaskflowCode::TaskPhasesOutcomeIncomplete.code().into());
            }
            terminals += 1;
            continue;
        }
        let phase = &phases[index];
        let mut passed_over: Vec<&str> = Vec::new();
        let mut capped = false;
        loop {
            let eligible = |operation: &&TaskPhaseOperation<'_>| {
                phase.operations.iter().any(|id| id == operation.id)
                    && !passed_over.contains(&operation.id)
            };
            let Some(operation) = operations
                .iter()
                .filter(eligible)
                .find(|operation| operation.from != "any" && canonical(operation.from) == page)
                .or_else(|| {
                    operations
                        .iter()
                        .filter(eligible)
                        .find(|operation| operation.from == "any")
                })
            else {
                if passed_over.is_empty() || capped {
                    let mut location = stage_location(TaskPhaseProblem::NoOperation, index)
                        .page(page)
                        .pack_operation(via);
                    location.entry_page = shown(start);
                    return Err(TaskPhaseRefusal::located(
                        TaskflowCode::TaskPhasesUnreachable,
                        location,
                    ));
                }
                break;
            };
            let member = reach.index_of(operation.id);
            if let Some(member) = member
                && reach.is_single(member)
                && completed & crate::DesignatedEffectReach::bit(member) != 0
            {
                return Err(TaskflowCode::TaskPhasesDesignatedEffectRepeated
                    .code()
                    .into());
            }
            // Workflow #308 SEL-L4b (S0 §1.4 (c)): a select step that declares `on_empty` may end
            // the run on its own page, so this state is a terminal as well; the walk goes on past
            // the step, because the select may also succeed.
            if let Some(key) = operation.on_empty_key {
                on_empty_terminal(scheduling, completed, &page, key, canonical)?;
                terminals += 1;
            }
            if operation.destinations.is_empty() {
                return Err(TaskflowCode::TaskPhasesPostconditionMissing.code().into());
            }
            let next_completed = member.map_or(completed, |member| {
                completed | crate::DesignatedEffectReach::bit(member)
            });
            for destination in &operation.destinations {
                if destination == "any" {
                    return Err(TaskflowCode::TaskPhasesPostconditionMissing.code().into());
                }
                let destination = canonical(destination);
                let next_index = index
                    + usize::from(
                        phase
                            .target_pages
                            .iter()
                            .any(|target| canonical(target) == destination),
                    );
                if next_index == index && phase.is_end_page(game, &destination) {
                    end_page_terminal(scheduling, next_completed, &destination, canonical)?;
                    terminals += 1;
                    continue;
                }
                pending.push_back((next_index, destination, next_completed, Some(operation.id)));
            }
            let may_be_capped = member.is_some_and(|member| reach.may_be_capped(member, completed));
            if !may_be_capped && !operation.may_be_passed_over {
                break;
            }
            capped |= may_be_capped;
            passed_over.push(operation.id);
        }
    }
    if terminals == 0 {
        return Err(TaskPhaseRefusal::located(
            TaskflowCode::TaskPhasesUnreachable,
            stage_location(TaskPhaseProblem::NoTerminal, entry.phase).page(start),
        ));
    }
    Ok(())
}

/// Workflow #308 SEL-L4b (S0 §1.4 (c)): the terminal an `on_empty` select step adds at its state
/// (`page`, the designated effects `completed` on the way): exactly one mapping has that state's
/// effect condition and lists the page, and its outcome key is the step's `on_empty` key. A step
/// reachable under both effect conditions needs two mappings with that one key, which unique
/// keys forbid, so it is refused.
fn on_empty_terminal(
    scheduling: &SchedulingOutcomeDeclaration,
    completed: u16,
    page: &str,
    key: &str,
    canonical: &dyn Fn(&str) -> String,
) -> Result<(), crate::outcome::Code> {
    let condition = crate::DesignatedEffectReach::condition(completed);
    let mappings = scheduling
        .mappings()
        .iter()
        .filter(|mapping| {
            mapping.effect() == condition
                && mapping
                    .terminal_pages()
                    .iter()
                    .any(|target| canonical(target) == page)
        })
        .collect::<Vec<_>>();
    match mappings.as_slice() {
        [mapping] if mapping.outcome_key() == key => Ok(()),
        _ => Err(TaskflowCode::TaskPhasesOutcomeIncomplete.code()),
    }
}

/// #411 B5/B20: the terminal an end page adds at its state (`page`, the designated effects
/// `completed` on the way; none at a start): exactly one mapping has that state's effect
/// condition and lists the page.
fn end_page_terminal(
    scheduling: &SchedulingOutcomeDeclaration,
    completed: u16,
    page: &str,
    canonical: &dyn Fn(&str) -> String,
) -> Result<(), crate::outcome::Code> {
    let condition = crate::DesignatedEffectReach::condition(completed);
    let mapped = scheduling
        .mappings()
        .iter()
        .filter(|mapping| {
            mapping.effect() == condition
                && mapping
                    .terminal_pages()
                    .iter()
                    .any(|target| canonical(target) == page)
        })
        .count();
    if mapped == 1 {
        Ok(())
    } else {
        Err(TaskflowCode::TaskPhasesOutcomeIncomplete.code())
    }
}

pub const fn postcondition_timeout_is_valid(timeout_ms: Option<u64>) -> bool {
    match timeout_ms {
        Some(value) => value > 0 && value <= MAX_POSTCONDITION_TIMEOUT_MS,
        None => true,
    }
}

/// Operation 0.8 post-admission fields. Values and their meaning belong to the package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldsDeclaration {
    pub mode: OcrFieldsMode,
    pub page_ids: Vec<String>,
    pub fields: Vec<OcrFieldDeclaration>,
    pub limits: OcrFieldsLimits,
    pub outcome_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrFieldsMode {
    FieldsV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldsLimits {
    pub max_frames: u32,
    pub max_items: u32,
    pub max_string_bytes: u32,
    pub max_total_bytes: u64,
    pub max_truth_entries: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldDeclaration {
    pub id: String,
    pub group: String,
    pub target_id: String,
    pub required: bool,
    pub privacy: OcrFieldPrivacy,
    pub trim: OcrFieldTrim,
    pub value: OcrFieldType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_extraction: Option<OcrFieldTextExtraction>,
}

/// A single bounded suffix, interpreted identically by execution and projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum OcrFieldTextExtraction {
    StripDeclaredSuffixV1 { suffix: Vec<OcrFieldSuffixSegment> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OcrFieldSuffixSegment {
    AsciiDigits { count: u8 },
    Literal { value: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrFieldExtractionVersion {
    StripDeclaredSuffixV1,
}

/// Half-open byte offsets in the original normalized text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldTextRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldExtractionEvidence {
    pub rule_version: OcrFieldExtractionVersion,
    pub matched_suffix: OcrFieldTextRange,
    pub extracted_text: String,
    pub extracted_range: OcrFieldTextRange,
}

impl OcrFieldTextExtraction {
    pub fn validate(&self) -> Result<(), crate::outcome::Code> {
        let Self::StripDeclaredSuffixV1 { suffix } = self;
        if !(1..=16).contains(&suffix.len()) {
            return Err(TaskflowCode::OcrFieldsTextExtractionInvalid.code());
        }
        let mut bytes = 0;
        for segment in suffix {
            bytes += match segment {
                OcrFieldSuffixSegment::AsciiDigits { count } if (1..=8).contains(count) => {
                    usize::from(*count)
                }
                OcrFieldSuffixSegment::Literal { value } if (1..=64).contains(&value.len()) => {
                    value.len()
                }
                _ => return Err(TaskflowCode::OcrFieldsTextExtractionInvalid.code()),
            };
        }
        if bytes > 256 {
            return Err(TaskflowCode::OcrFieldsTextExtractionInvalid.code());
        }
        Ok(())
    }

    /// Call only after the complete dictionary input has no exact or alias match.
    pub fn extract(
        &self,
        normalized_text: &str,
    ) -> Result<Option<OcrFieldExtractionEvidence>, crate::outcome::Code> {
        self.validate()?;
        let Self::StripDeclaredSuffixV1 { suffix } = self;
        let mut end = normalized_text.len();
        for segment in suffix.iter().rev() {
            let length = match segment {
                OcrFieldSuffixSegment::AsciiDigits { count } => usize::from(*count),
                OcrFieldSuffixSegment::Literal { value } => value.len(),
            };
            let Some(start) = end.checked_sub(length) else {
                return Ok(None);
            };
            let Some(text) = normalized_text.get(start..end) else {
                return Ok(None);
            };
            let matches = match segment {
                OcrFieldSuffixSegment::AsciiDigits { .. } => {
                    text.bytes().all(|byte| byte.is_ascii_digit())
                }
                OcrFieldSuffixSegment::Literal { value } => text == value,
            };
            if !matches {
                return Ok(None);
            }
            end = start;
        }
        let prefix = &normalized_text[..end];
        let extracted_text = prefix.trim();
        if extracted_text.is_empty() {
            return Ok(None);
        }
        let start = prefix.len() - prefix.trim_start().len();
        Ok(Some(OcrFieldExtractionEvidence {
            rule_version: OcrFieldExtractionVersion::StripDeclaredSuffixV1,
            matched_suffix: OcrFieldTextRange {
                start: end,
                end: normalized_text.len(),
            },
            extracted_text: extracted_text.to_owned(),
            extracted_range: OcrFieldTextRange {
                start,
                end: start + extracted_text.len(),
            },
        }))
    }

    pub fn verify(
        &self,
        normalized_text: &str,
        evidence: &OcrFieldExtractionEvidence,
    ) -> Result<(), crate::outcome::Code> {
        if self.extract(normalized_text)?.as_ref() != Some(evidence) {
            return Err(TaskflowCode::OcrFieldsTextExtractionEvidenceInvalid.code());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrFieldPrivacy {
    Public,
    Personal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrFieldTrim {
    WhitespaceV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OcrFieldType {
    UnsignedInteger {
        min: u64,
        max: u64,
        #[serde(
            default,
            skip_serializing_if = "OcrUnsignedIntegerFormat::is_ascii_decimal"
        )]
        format: OcrUnsignedIntegerFormat,
    },
    DictionaryEntry {
        dictionary: OcrFieldDictionaryReference,
    },
}

// Interface anchor `package` revision 2 (contracts/component-interfaces.md): raise its revision in
// distribution/windows/component-interfaces.json when packs need what older readers lack.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrUnsignedIntegerFormat {
    #[default]
    AsciiDecimal,
    CommaGrouped,
    CurrentCapacity,
    /// ASCII decimal digits after at most one optional `x`, `X` or `×` (U+00D7) prefix.
    TimesPrefixed,
    /// Workflow #396 REC-6: `H:MM:SS`, 1 to 3 ASCII digits of hours, then two digits each of
    /// minutes and seconds in `00..=59`; the value is the number of seconds.
    ClockHms,
}

/// Both parts of one parsed `unsigned_integer` observation (Workflow #396 REC-6): the value, and
/// for `current_capacity` the capacity (the right-hand part), which only a value test reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OcrUnsignedIntegerParts {
    pub value: u64,
    pub capacity: Option<u64>,
}

impl OcrUnsignedIntegerFormat {
    fn is_ascii_decimal(&self) -> bool {
        *self == Self::AsciiDecimal
    }

    /// Parses the complete already-trimmed observation without changing its text.
    pub fn parse(self, text: &str, min: u64, max: u64) -> Result<u64, OcrFieldReason> {
        let value = self.parse_parts(text)?.value;
        if value < min || value > max {
            return Err(OcrFieldReason::OutOfRange);
        }
        Ok(value)
    }

    /// Parses the complete already-trimmed observation into both its parts, without a range
    /// check (Workflow #396 REC-6). [`Self::parse`] checks the value against its range; a value
    /// test checks the part it declares.
    pub fn parse_parts(self, text: &str) -> Result<OcrUnsignedIntegerParts, OcrFieldReason> {
        if text.is_empty() {
            return Err(OcrFieldReason::Empty);
        }
        let decimal = |digits: &str| {
            if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(OcrFieldReason::InvalidInteger);
            }
            digits.parse::<u64>().map_err(|_| OcrFieldReason::Overflow)
        };
        let mut capacity = None;
        let value = match self {
            Self::AsciiDecimal => decimal(text)?,
            Self::CommaGrouped => {
                let mut groups = text.split(',');
                let first = groups.next().ok_or(OcrFieldReason::InvalidInteger)?;
                if first.is_empty()
                    || first.len() > 3
                    || !first.bytes().all(|byte| byte.is_ascii_digit())
                    || (first.starts_with('0') && (first.len() > 1 || text.contains(',')))
                    || !groups.all(|group| {
                        group.len() == 3 && group.bytes().all(|byte| byte.is_ascii_digit())
                    })
                {
                    return Err(OcrFieldReason::InvalidInteger);
                }
                text.bytes()
                    .filter(|byte| *byte != b',')
                    .try_fold(0_u64, |value, byte| {
                        value
                            .checked_mul(10)
                            .and_then(|value| value.checked_add(u64::from(byte - b'0')))
                            .ok_or(OcrFieldReason::Overflow)
                    })?
            }
            Self::CurrentCapacity => {
                let (current, declared) =
                    text.split_once('/').ok_or(OcrFieldReason::InvalidInteger)?;
                let current = decimal(current)?;
                capacity = Some(decimal(declared)?);
                current
            }
            Self::TimesPrefixed => decimal(
                text.strip_prefix(|prefix: char| matches!(prefix, 'x' | 'X' | '\u{00D7}'))
                    .unwrap_or(text),
            )?,
            Self::ClockHms => {
                let mut parts = text.split(':');
                let (Some(hours), Some(minutes), Some(seconds), None) =
                    (parts.next(), parts.next(), parts.next(), parts.next())
                else {
                    return Err(OcrFieldReason::InvalidInteger);
                };
                let sixty = |digits: &str| {
                    if digits.len() != 2 {
                        return Err(OcrFieldReason::InvalidInteger);
                    }
                    decimal(digits).and_then(|value| {
                        if value < 60 {
                            Ok(value)
                        } else {
                            Err(OcrFieldReason::InvalidInteger)
                        }
                    })
                };
                if !(1..=3).contains(&hours.len()) {
                    return Err(OcrFieldReason::InvalidInteger);
                }
                let hours = decimal(hours)?;
                hours * 3_600 + sixty(minutes)? * 60 + sixty(seconds)?
            }
        };
        Ok(OcrUnsignedIntegerParts { value, capacity })
    }
}

/// How a value test trims an observed text before parsing it (Workflow #396 REC-6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrValueTrim {
    /// Unicode whitespace at both ends, as `str::trim` (the fields' `whitespace_v1`).
    WhitespaceV1,
    /// Every Unicode whitespace character, wherever it stands (`1 / 3` reads as `1/3`).
    WhitespaceAllV1,
}

impl OcrValueTrim {
    pub fn apply(self, text: &str) -> std::borrow::Cow<'_, str> {
        match self {
            Self::WhitespaceV1 => std::borrow::Cow::Borrowed(text.trim()),
            Self::WhitespaceAllV1 => std::borrow::Cow::Owned(
                text.chars()
                    .filter(|character| !character.is_whitespace())
                    .collect(),
            ),
        }
    }
}

/// Workflow #411 B1 (register §1.163): the most confusion pairs one dictionary declares.
pub const MAX_INTEGER_CONFUSIONS: usize = 32;
/// Workflow #411 B1: the longest `prefix` or `suffix` of a dictionary, in bytes.
pub const MAX_INTEGER_AFFIX_BYTES: usize = 64;
/// Workflow #411 B1: the most ASCII digits a dictionary's `digits` names (`u64::MAX` has 20).
pub const MAX_INTEGER_DIGITS: u64 = 20;
/// Workflow #411 B1: the fewest and the most frames of one agreement.
pub const MIN_AGREEMENT_FRAMES: u64 = 2;
pub const MAX_AGREEMENT_FRAMES: u64 = 5;
/// Workflow #411 B1: the longest interval between two frames of one agreement.
pub const MAX_AGREEMENT_INTERVAL_MS: u64 = 1_000;

/// Workflow #411 B1 (B-train plan §2.1 item 1, register §1.163): the dictionary of one number
/// read from text, declared on the `value` object of one value-test side or one reading mark and
/// never shared between them. After the trim and before the format's parser, a declared `prefix`
/// and `suffix` are stripped when the text carries them, each `confusions` character becomes its
/// ASCII digit, and the text must then hold `digits.min..=digits.max` ASCII digits (else the text
/// is not an integer). The read text stays verbatim in evidence; the parsed value shows the
/// substitution. There is no global table. Absent, the text is parsed as before.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegerDictionary {
    /// `[from, to]` pairs: `from` one character, `to` one ASCII digit, no `from` twice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confusions: Option<Vec<(String, String)>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affix: Option<IntegerAffix>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digits: Option<IntegerDigits>,
}

/// Workflow #411 B1: the text a dictionary strips before and after the number, when present.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegerAffix {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,
}

/// Workflow #411 B1: how many ASCII digits the text holds after the substitution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegerDigits {
    pub min: u64,
    pub max: u64,
}

impl IntegerDictionary {
    /// The member the declaration rules refuse, as a JSON pointer below the dictionary, or
    /// `None` when every rule holds: `confusions` 1..=32 pairs (`/confusions`), each `from` one
    /// character not declared before (`/confusions/<i>/0`), each `to` one ASCII digit
    /// (`/confusions/<i>/1`); a `prefix` or `suffix` of 1..=64 bytes (`/affix/prefix`,
    /// `/affix/suffix`); `digits.min` in 1..=20 (`/digits/min`) and `digits.max` in
    /// `min..=20` (`/digits/max`).
    pub fn refused_member(&self) -> Option<String> {
        if let Some(confusions) = &self.confusions {
            if !(1..=MAX_INTEGER_CONFUSIONS).contains(&confusions.len()) {
                return Some("/confusions".to_owned());
            }
            for (index, (from, to)) in confusions.iter().enumerate() {
                if from.chars().count() != 1
                    || confusions[..index]
                        .iter()
                        .any(|(earlier, _)| earlier == from)
                {
                    return Some(format!("/confusions/{index}/0"));
                }
                if to.len() != 1 || !to.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Some(format!("/confusions/{index}/1"));
                }
            }
        }
        if let Some(affix) = &self.affix {
            for (member, value) in [("prefix", &affix.prefix), ("suffix", &affix.suffix)] {
                if value
                    .as_ref()
                    .is_some_and(|value| !(1..=MAX_INTEGER_AFFIX_BYTES).contains(&value.len()))
                {
                    return Some(format!("/affix/{member}"));
                }
            }
        }
        if let Some(digits) = self.digits {
            if !(1..=MAX_INTEGER_DIGITS).contains(&digits.min) {
                return Some("/digits/min".to_owned());
            }
            if !(digits.min..=MAX_INTEGER_DIGITS).contains(&digits.max) {
                return Some("/digits/max".to_owned());
            }
        }
        None
    }

    /// Applies the dictionary to one trimmed text: strips the declared `prefix` and `suffix`
    /// when the text carries them, turns each `confusions` character into its digit, then
    /// checks the declared digit count (`InvalidInteger` outside it). The result goes to the
    /// format's parser; the caller keeps the read text as it was.
    pub fn apply<'a>(&self, text: &'a str) -> Result<std::borrow::Cow<'a, str>, OcrFieldReason> {
        let mut text = text;
        if let Some(affix) = &self.affix {
            if let Some(prefix) = affix.prefix.as_deref() {
                text = text.strip_prefix(prefix).unwrap_or(text);
            }
            if let Some(suffix) = affix.suffix.as_deref() {
                text = text.strip_suffix(suffix).unwrap_or(text);
            }
        }
        let text = match &self.confusions {
            None => std::borrow::Cow::Borrowed(text),
            Some(confusions) => std::borrow::Cow::Owned(
                text.chars()
                    .map(|character| {
                        confusions
                            .iter()
                            .find(|(from, _)| from.chars().next() == Some(character))
                            .and_then(|(from, _)| from.chars().next())
                            .unwrap_or(character)
                    })
                    .collect(),
            ),
        };
        if let Some(digits) = self.digits {
            let count = text.bytes().filter(u8::is_ascii_digit).count() as u64;
            if !(digits.min..=digits.max).contains(&count) {
                return Err(OcrFieldReason::InvalidInteger);
            }
        }
        Ok(text)
    }
}

/// Workflow #411 B1 (B-train plan §2.1 item 2, register §1.163): the frames that must read one
/// value before it counts as read, declared on one value-test side or one reading mark: `frames`
/// (2..=5) frames `interval_ms` (1..=1000) apart, of which `k` (1..=`frames`, `frames` when
/// absent) must read the same value. A reading writes a fact, so its `k` is `frames`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegerAgreement {
    pub frames: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k: Option<u64>,
    pub interval_ms: u64,
}

impl IntegerAgreement {
    /// The member the declaration rules refuse, as a JSON pointer below the agreement, or
    /// `None` when every rule holds: `/frames` outside 2..=5, `/k` outside 1..=`frames` or, for a
    /// reading (`reading`), other than `frames`, and `/interval_ms` outside 1..=1000.
    pub fn refused_member(&self, reading: bool) -> Option<&'static str> {
        if !(MIN_AGREEMENT_FRAMES..=MAX_AGREEMENT_FRAMES).contains(&self.frames) {
            return Some("/frames");
        }
        if let Some(k) = self.k
            && (!(1..=self.frames).contains(&k) || (reading && k != self.frames))
        {
            return Some("/k");
        }
        if !(1..=MAX_AGREEMENT_INTERVAL_MS).contains(&self.interval_ms) {
            return Some("/interval_ms");
        }
        None
    }

    /// The number of frames, for an admitted agreement.
    pub fn frame_count(&self) -> usize {
        usize::try_from(self.frames).unwrap_or(usize::MAX)
    }

    /// How many frames must read the same value: `k`, or every frame.
    pub fn required(&self) -> usize {
        usize::try_from(self.k.unwrap_or(self.frames)).unwrap_or(usize::MAX)
    }
}

/// Workflow #411 B1: the refusal of one declared `dictionary` (a JSON value): the member it
/// refuses, as a JSON pointer below the dictionary (empty for the dictionary itself), and why;
/// `None` when it reads as an [`IntegerDictionary`] whose rules hold. The shape first (an object
/// of `confusions`, `affix` and `digits`; pairs of two strings; string affixes; unsigned digit
/// bounds), then [`IntegerDictionary::refused_member`].
pub fn integer_dictionary_refusal(
    value: &serde_json::Value,
) -> Option<(String, crate::ResourceDeclarationReason)> {
    use crate::ResourceDeclarationReason::{InvalidType, InvalidValue, MissingField, UnknownField};
    let Some(object) = value.as_object() else {
        return Some((String::new(), InvalidType));
    };
    if let Some(member) = object
        .keys()
        .find(|member| !matches!(member.as_str(), "confusions" | "affix" | "digits"))
    {
        return Some((format!("/{member}"), UnknownField));
    }
    if let Some(confusions) = object.get("confusions") {
        let Some(pairs) = confusions.as_array() else {
            return Some(("/confusions".to_owned(), InvalidType));
        };
        for (index, pair) in pairs.iter().enumerate() {
            let Some(pair) = pair.as_array() else {
                return Some((format!("/confusions/{index}"), InvalidType));
            };
            if pair.len() != 2 {
                return Some((format!("/confusions/{index}"), InvalidValue));
            }
            if let Some(side) = pair.iter().position(|member| !member.is_string()) {
                return Some((format!("/confusions/{index}/{side}"), InvalidType));
            }
        }
    }
    if let Some(affix) = object.get("affix") {
        let Some(affix) = affix.as_object() else {
            return Some(("/affix".to_owned(), InvalidType));
        };
        for (member, value) in affix {
            if !matches!(member.as_str(), "prefix" | "suffix") {
                return Some((format!("/affix/{member}"), UnknownField));
            }
            if !value.is_string() {
                return Some((format!("/affix/{member}"), InvalidType));
            }
        }
    }
    if let Some(digits) = object.get("digits") {
        let Some(digits) = digits.as_object() else {
            return Some(("/digits".to_owned(), InvalidType));
        };
        if let Some(member) = digits
            .keys()
            .find(|member| !matches!(member.as_str(), "min" | "max"))
        {
            return Some((format!("/digits/{member}"), UnknownField));
        }
        for member in ["min", "max"] {
            match digits.get(member) {
                None => return Some((format!("/digits/{member}"), MissingField)),
                Some(bound) if !bound.is_u64() => {
                    return Some((format!("/digits/{member}"), InvalidType));
                }
                Some(_) => {}
            }
        }
    }
    match serde_json::from_value::<IntegerDictionary>(value.clone()) {
        Ok(dictionary) => dictionary
            .refused_member()
            .map(|member| (member, InvalidValue)),
        Err(_) => Some((String::new(), InvalidValue)),
    }
}

/// Workflow #411 B1: the refusal of one declared `agreement` (a JSON value) of a value-test side
/// or, with `reading`, of a reading mark: the member it refuses, as a JSON pointer below the
/// agreement (empty for the agreement itself), and why; `None` when it reads as an
/// [`IntegerAgreement`] whose rules hold. The shape first (an object of the unsigned integers
/// `frames`, `k` and `interval_ms`, `frames` and `interval_ms` required), then
/// [`IntegerAgreement::refused_member`].
pub fn integer_agreement_refusal(
    value: &serde_json::Value,
    reading: bool,
) -> Option<(String, crate::ResourceDeclarationReason)> {
    use crate::ResourceDeclarationReason::{InvalidType, InvalidValue, MissingField, UnknownField};
    let Some(object) = value.as_object() else {
        return Some((String::new(), InvalidType));
    };
    if let Some(member) = object
        .keys()
        .find(|member| !matches!(member.as_str(), "frames" | "k" | "interval_ms"))
    {
        return Some((format!("/{member}"), UnknownField));
    }
    for member in ["frames", "interval_ms"] {
        if !object.contains_key(member) {
            return Some((format!("/{member}"), MissingField));
        }
    }
    if let Some((member, _)) = object.iter().find(|(_, value)| !value.is_u64()) {
        return Some((format!("/{member}"), InvalidType));
    }
    match serde_json::from_value::<IntegerAgreement>(value.clone()) {
        Ok(agreement) => agreement
            .refused_member(reading)
            .map(|member| (member.to_owned(), InvalidValue)),
        Err(_) => Some((String::new(), InvalidValue)),
    }
}

/// The comparison of a value test (Workflow #396 REC-6): `parses` needs only the left side to
/// read; the others compare the left value with the right one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueTestOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Parses,
}

impl ValueTestOp {
    /// Whether `left op right` holds; `parses` always holds once the left side reads.
    pub fn holds(self, left: i128, right: i128) -> bool {
        match self {
            Self::Eq => left == right,
            Self::Ne => left != right,
            Self::Lt => left < right,
            Self::Le => left <= right,
            Self::Gt => left > right,
            Self::Ge => left >= right,
            Self::Parses => true,
        }
    }
}

/// Why one side of a value test could not be read (Workflow #396 REC-6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueTestProblem {
    Empty,
    Invalid,
    Overflow,
    OutOfRange,
    RegionUnresolved,
    LowConfidence,
    /// Ledger revision 4 (Workflow #411 B1, register §1.153 LR-5; writer BT-B1): the frames of
    /// a declared agreement did not read one value often enough.
    NoAgreement,
}

/// A dictionary a field reads, by its task-local path. Workflow #336 PKG-S2 (option 丙, M-PKG
/// §3.2, D5): a `sha256` the reference may still carry is an [`IgnoredPin`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldDictionaryReference {
    pub path: String,
    #[serde(default, rename = "sha256", skip_serializing)]
    pub ignored_sha256: IgnoredPin,
}

/// Workflow #336 PKG-S2 (option 丙, M-PKG §3.2, D5): the `sha256` key a resource file may still
/// carry on a select step's policy, an OCR dictionary reference or a post-admission truth set.
/// The key is optional and its value, whatever it is, is ignored when read: it is neither
/// validated nor compared, and never written or echoed. PKG-S1b refuses the key again once the
/// resource repositories no longer carry it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IgnoredPin;

impl<'de> Deserialize<'de> for IgnoredPin {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        serde::de::IgnoredAny::deserialize(deserializer).map(|_| Self)
    }
}

/// Readings one task may declare (`contracts/resource-readings.md`).
pub const MAX_RESOURCE_READINGS: usize = 16;
/// The largest value a reading may declare: 2^53 - 1, exact in every JSON consumer.
pub const MAX_RESOURCE_READING_VALUE: u64 = (1 << 53) - 1;

/// One operation `resource_readings` entry (schema 0.8 and 0.9, Workflow #335 S5), a reading
/// mark: the integer the Runtime reads from `target_id` on each frame a successful run reaches
/// the task's page `page_id` on (Workflow #384 D2-S2: any page of the task) and writes as the
/// instance fact `fact_key`. Workflow #390 READ-1 (FCM O-08): a `current_capacity` mark that
/// declares `capacity_fact_key` also writes the capacity (the right-hand part) as that second
/// instance fact. The declaration never enters an event, artifact or report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceReadingDeclaration {
    pub id: String,
    pub fact_key: String,
    /// Workflow #390 READ-1: the second reading key of a `current_capacity` mark, which the
    /// capacity is written as; absent, the capacity is validated and not written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_fact_key: Option<String>,
    pub page_id: String,
    pub target_id: String,
    pub trim: OcrFieldTrim,
    pub value: ResourceReadingType,
    pub minimum_confidence_milli: u16,
    pub valid_for_ms: u64,
    /// Workflow #411 B1 (register §1.163): the frames that must all read the same value before
    /// the mark writes its fact: the frame its page was reached on and the next `frames - 1`
    /// captures, `interval_ms` apart (`k` is `frames`). Absent, the mark reads the frame its page
    /// was reached on, as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agreement: Option<IntegerAgreement>,
}

/// The closed set of reading value types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceReadingType {
    UnsignedInteger {
        min: u64,
        max: u64,
        #[serde(
            default,
            skip_serializing_if = "OcrUnsignedIntegerFormat::is_ascii_decimal"
        )]
        format: OcrUnsignedIntegerFormat,
        /// Workflow #411 B1: the number's dictionary; absent, the text is parsed as before.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dictionary: Option<IntegerDictionary>,
    },
}

impl ResourceReadingDeclaration {
    /// The shared rule of one declaration. The declaration gate checks the same rules field by
    /// field first; each reason names the field it rejects.
    pub fn validate(&self) -> Result<(), crate::outcome::Code> {
        let identifier = |value: &str| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-/.:".contains(&c))
        };
        if !identifier(&self.id) {
            return Err(TaskflowCode::ResourceReadingIdInvalid.code());
        }
        if !(self.fact_key.starts_with("resource.") || self.fact_key.starts_with("inventory."))
            || crate::fact::validate_fact_key(&self.fact_key).is_err()
        {
            return Err(TaskflowCode::ResourceReadingFactKeyInvalid.code());
        }
        // Workflow #390 READ-1: only a `current_capacity` mark has a capacity to write, under a
        // key of the mark key rule.
        if let Some(capacity_fact_key) = &self.capacity_fact_key {
            let ResourceReadingType::UnsignedInteger { format, .. } = &self.value;
            if *format != OcrUnsignedIntegerFormat::CurrentCapacity
                || !resource_fact_key(capacity_fact_key)
            {
                return Err(TaskflowCode::ResourceReadingCapacityFactKeyInvalid.code());
            }
        }
        if !identifier(&self.page_id) || self.page_id == "any" {
            return Err(TaskflowCode::ResourceReadingPageIdInvalid.code());
        }
        if !identifier(&self.target_id) {
            return Err(TaskflowCode::ResourceReadingTargetIdInvalid.code());
        }
        let ResourceReadingType::UnsignedInteger {
            min,
            max,
            dictionary,
            ..
        } = &self.value;
        if *max > MAX_RESOURCE_READING_VALUE {
            return Err(TaskflowCode::ResourceReadingValueMaxInvalid.code());
        }
        if min > max {
            return Err(TaskflowCode::ResourceReadingValueMinInvalid.code());
        }
        if !(1..=1_000).contains(&self.minimum_confidence_milli) {
            return Err(TaskflowCode::ResourceReadingMinimumConfidenceInvalid.code());
        }
        if !(crate::MIN_FACT_TTL_MS..=crate::MAX_FACT_TTL_MS).contains(&self.valid_for_ms) {
            return Err(TaskflowCode::ResourceReadingValidForInvalid.code());
        }
        // Workflow #411 B1: a dictionary and an agreement follow their declaration rules; a mark
        // writes a fact, so every frame of its agreement must read the value. No code of their
        // own (register §1.163): the declaration gate names the refused member.
        if dictionary
            .as_ref()
            .is_some_and(|dictionary| dictionary.refused_member().is_some())
            || self
                .agreement
                .is_some_and(|agreement| agreement.refused_member(true).is_some())
        {
            return Err(SharedCode::ResourceDeclarationInvalid.code());
        }
        Ok(())
    }

    /// Trims and parses one observed text with the declared value type and range. This is the
    /// only reading parser; Workflow #308 slice 2 reuses it.
    pub fn parse(&self, text: &str) -> Result<u64, OcrFieldReason> {
        self.parse_parts(text).map(|parts| parts.value)
    }

    /// Workflow #390 READ-1: parses one observed text as [`Self::parse`] does and, for a mark
    /// that declares `capacity_fact_key`, also returns the capacity, which must fit an exact
    /// JSON integer (`0..=2^53-1`, else `OutOfRange`); without that key, `capacity` is `None`.
    pub fn parse_parts(&self, text: &str) -> Result<OcrUnsignedIntegerParts, OcrFieldReason> {
        let text = match self.trim {
            OcrFieldTrim::WhitespaceV1 => text.trim(),
        };
        let ResourceReadingType::UnsignedInteger {
            min,
            max,
            format,
            dictionary,
        } = &self.value;
        // Workflow #411 B1: the mark's dictionary, after the trim and before the parser.
        let text = match dictionary {
            Some(dictionary) => dictionary.apply(text)?,
            None => std::borrow::Cow::Borrowed(text),
        };
        let parts = format.parse_parts(&text)?;
        if parts.value < *min || parts.value > *max {
            return Err(OcrFieldReason::OutOfRange);
        }
        let capacity = match (&self.capacity_fact_key, parts.capacity) {
            (None, _) => None,
            (Some(_), Some(capacity)) if capacity > MAX_RESOURCE_READING_VALUE => {
                return Err(OcrFieldReason::OutOfRange);
            }
            (Some(_), capacity) => capacity,
        };
        Ok(OcrUnsignedIntegerParts {
            value: parts.value,
            capacity,
        })
    }

    /// Workflow #390 READ-1: the instance fact keys the mark writes: `fact_key`, then
    /// `capacity_fact_key` when it declares one.
    pub fn fact_keys(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.fact_key.as_str()).chain(self.capacity_fact_key.as_deref())
    }
}

/// One task's `resource_readings`: 1..=16 valid entries with unique ids and fact keys; every key
/// a mark writes (Workflow #390 READ-1: its capacity key too) is unique in the task.
pub fn validate_resource_readings(
    readings: &[ResourceReadingDeclaration],
) -> Result<(), crate::outcome::Code> {
    if readings.is_empty() || readings.len() > MAX_RESOURCE_READINGS {
        return Err(TaskflowCode::ResourceReadingsCountInvalid.code());
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut fact_keys = std::collections::BTreeSet::new();
    for reading in readings {
        reading.validate()?;
        if !ids.insert(reading.id.as_str()) {
            return Err(TaskflowCode::ResourceReadingIdDuplicate.code());
        }
        if !reading.fact_keys().all(|key| fact_keys.insert(key)) {
            return Err(TaskflowCode::ResourceReadingFactKeyDuplicate.code());
        }
    }
    Ok(())
}

/// An OCR confidence as integer milli, `floor(clamp(confidence, 0, 1) * 1000)`. A missing or
/// non-finite confidence has none. This is the only conversion; Workflow #308 slice 2 reuses it.
pub fn ocr_confidence_milli(confidence: Option<f32>) -> Option<u16> {
    confidence
        .filter(|confidence| confidence.is_finite())
        .map(|confidence| (confidence.clamp(0.0, 1.0) * 1_000.0).floor() as u16)
}

/// The identifier rule of reading marks and adjustments: `[A-Za-z0-9_\-/.:]`, `1..=128` bytes.
fn resource_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-/.:".contains(&c))
}

/// The key rule of reading marks and adjustments: a realistic instance fact key.
fn resource_fact_key(value: &str) -> bool {
    (value.starts_with("resource.") || value.starts_with("inventory."))
        && crate::fact::validate_fact_key(value).is_ok()
}

/// Adjustments one task may declare (Workflow #384 D2-S3), as many as reading marks.
pub const MAX_RESOURCE_ADJUSTMENTS: usize = MAX_RESOURCE_READINGS;

/// One operation `resource_adjustments` entry (Workflow #384 D2-S3, D2 model §3.6): once the
/// task's operation `operation_id` is confirmed, the instance fact `fact_key` has changed. The
/// new value comes only from live recognition. Without `amount` it is a reading mark of the key
/// that the run reads after the confirmation; with `amount` it is the change the screen shows
/// on the confirmation frame, added to or subtracted from the key's current record. A package
/// never declares an amount itself. The declaration never enters an event, artifact or report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceAdjustmentDeclaration {
    pub id: String,
    pub fact_key: String,
    pub operation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<ResourceAdjustmentAmount>,
}

/// Where an adjustment reads the change the screen shows: one OCR target of the task, and
/// whether the change is spent or gained. No other source of an amount exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceAdjustmentAmount {
    pub target_id: String,
    pub direction: ResourceAdjustmentDirection,
}

/// Whether an adjustment's amount is subtracted from or added to the key's current value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceAdjustmentDirection {
    Consume,
    Gain,
}

impl ResourceAdjustmentDeclaration {
    /// The shared rule of one declaration. The declaration gate checks the same rules field by
    /// field first; each reason names the field it rejects.
    pub fn validate(&self) -> Result<(), crate::outcome::Code> {
        if !resource_identifier(&self.id) {
            return Err(TaskflowCode::ResourceAdjustmentIdInvalid.code());
        }
        if !resource_fact_key(&self.fact_key) {
            return Err(TaskflowCode::ResourceAdjustmentFactKeyInvalid.code());
        }
        if !resource_identifier(&self.operation_id) {
            return Err(TaskflowCode::ResourceAdjustmentOperationIdInvalid.code());
        }
        if self
            .amount
            .as_ref()
            .is_some_and(|amount| !resource_identifier(&amount.target_id))
        {
            return Err(TaskflowCode::ResourceAdjustmentTargetIdInvalid.code());
        }
        Ok(())
    }
}

impl ResourceAdjustmentAmount {
    /// Trims one observed change and parses it as ASCII decimal digits after at most one sign
    /// of its direction (`+` for a gain, `-` for a consumption), within `0..=2^53-1`. The
    /// digits are read by the reading parser (`OcrUnsignedIntegerFormat`).
    pub fn parse(&self, text: &str) -> Result<u64, OcrFieldReason> {
        let text = text.trim();
        let sign = match self.direction {
            ResourceAdjustmentDirection::Consume => '-',
            ResourceAdjustmentDirection::Gain => '+',
        };
        let digits = text.strip_prefix(sign).unwrap_or(text);
        OcrUnsignedIntegerFormat::AsciiDecimal.parse(digits, 0, MAX_RESOURCE_READING_VALUE)
    }
}

/// One task's `resource_adjustments` (Workflow #384 D2-S3): `1..=16` valid entries with unique
/// ids, each naming one of the task's `operation_ids` and adjusting a key at most once per
/// operation. An adjustment without `amount` needs a reading mark of its key in `readings`
/// (Workflow #390 READ-1: a mark whose `fact_key` or `capacity_fact_key` it is).
pub fn validate_resource_adjustments(
    adjustments: &[ResourceAdjustmentDeclaration],
    readings: Option<&[ResourceReadingDeclaration]>,
    operation_ids: &[&str],
) -> Result<(), crate::outcome::Code> {
    if adjustments.is_empty() || adjustments.len() > MAX_RESOURCE_ADJUSTMENTS {
        return Err(TaskflowCode::ResourceAdjustmentsCountInvalid.code());
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut changes = std::collections::BTreeSet::new();
    for adjustment in adjustments {
        adjustment.validate()?;
        if !operation_ids.contains(&adjustment.operation_id.as_str()) {
            return Err(TaskflowCode::ResourceAdjustmentOperationIdInvalid.code());
        }
        // Workflow #390 READ-1: a mark of the key is one that writes it, its capacity key
        // included.
        if adjustment.amount.is_none()
            && !readings.unwrap_or_default().iter().any(|reading| {
                reading
                    .fact_keys()
                    .any(|key| key == adjustment.fact_key.as_str())
            })
        {
            return Err(TaskflowCode::ResourceAdjustmentMarkMissing.code());
        }
        if !ids.insert(adjustment.id.as_str()) {
            return Err(TaskflowCode::ResourceAdjustmentIdDuplicate.code());
        }
        if !changes.insert((
            adjustment.operation_id.as_str(),
            adjustment.fact_key.as_str(),
        )) {
            return Err(TaskflowCode::ResourceAdjustmentFactKeyDuplicate.code());
        }
    }
    Ok(())
}

impl OcrFieldsDeclaration {
    /// A task without input can only collect fields on the terminal pages of its fields outcome.
    /// Workflow #390 OUT-1 (FCM O-05): it may declare several outcomes, each mapping terminal
    /// pages without a designated effect; its target pages are all of those terminal pages, and
    /// a run that ends on another outcome's page collects no field.
    pub fn validate_zero_input_task(
        &self,
        game: &str,
        execution_mode: &str,
        stop_on_confirmation: bool,
        target_pages: &[String],
        scheduling: &SchedulingOutcomeDeclaration,
    ) -> Result<(), crate::outcome::Code> {
        self.validate()?;
        scheduling
            .validate()
            .map_err(|_| TaskflowCode::OcrFieldsZeroInputOutcomeInvalid.code())?;
        // Outcome keys are unique in a valid declaration.
        let Some(fields_mapping) = scheduling
            .mappings()
            .iter()
            .find(|mapping| mapping.outcome_key() == self.outcome_key)
        else {
            return Err(TaskflowCode::OcrFieldsZeroInputOutcomeInvalid.code());
        };
        if execution_mode != "navigable_route"
            || !stop_on_confirmation
            || scheduling.has_designated_effect()
            || scheduling
                .mappings()
                .iter()
                .any(|mapping| mapping.effect() != SchedulingEffectCondition::NoDesignatedEffect)
        {
            return Err(TaskflowCode::OcrFieldsZeroInputOutcomeInvalid.code());
        }
        let prefix = format!("{game}/");
        let canonical = |pages: &[String]| {
            pages
                .iter()
                .map(|page| page.strip_prefix(&prefix).unwrap_or(page).to_owned())
                .collect::<std::collections::BTreeSet<_>>()
        };
        let targets = canonical(target_pages);
        let fields = canonical(&self.page_ids);
        let fields_terminals = canonical(fields_mapping.terminal_pages());
        let declared_terminals = scheduling
            .mappings()
            .iter()
            .map(|mapping| mapping.terminal_pages().len())
            .sum::<usize>();
        let terminals = scheduling
            .mappings()
            .iter()
            .flat_map(|mapping| canonical(mapping.terminal_pages()))
            .collect::<std::collections::BTreeSet<_>>();
        if targets.is_empty()
            || targets.iter().any(|page| page.is_empty() || page == "any")
            || targets.len() != target_pages.len()
            || fields.len() != self.page_ids.len()
            || terminals.len() != declared_terminals
            || fields != fields_terminals
            || targets != terminals
        {
            return Err(TaskflowCode::OcrFieldsZeroInputPagesInvalid.code());
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), crate::outcome::Code> {
        let identifier = |value: &str| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-/.:".contains(&c))
        };
        let mut pages = std::collections::BTreeSet::new();
        let mut ids = std::collections::BTreeSet::new();
        let mut targets = std::collections::BTreeSet::new();
        let limits = &self.limits;
        if self.page_ids.is_empty()
            || self.page_ids.len() > 2
            || self
                .page_ids
                .iter()
                .any(|page| !identifier(page) || !pages.insert(page))
            || self.fields.is_empty()
            || self.fields.len() > 32
            || self.outcome_key != "fields_recorded"
            || !(1..=256).contains(&limits.max_frames)
            || !(1..=4096).contains(&limits.max_items)
            || !(1..=4096).contains(&limits.max_string_bytes)
            || !(1..=4 * 1024 * 1024).contains(&limits.max_total_bytes)
            || !(1..=4096).contains(&limits.max_truth_entries)
        {
            return Err(SharedCode::OcrFieldsDeclarationInvalid.code());
        }
        for field in &self.fields {
            if let Some(extraction) = &field.text_extraction {
                if !matches!(field.value, OcrFieldType::DictionaryEntry { .. }) {
                    return Err(TaskflowCode::OcrFieldsTextExtractionTypeInvalid.code());
                }
                extraction.validate()?;
            }
            if !identifier(&field.id)
                || !identifier(&field.group)
                || !identifier(&field.target_id)
                || !ids.insert(&field.id)
                || !targets.insert(&field.target_id)
            {
                return Err(TaskflowCode::OcrFieldsBindingInvalid.code());
            }
            match &field.value {
                OcrFieldType::UnsignedInteger { min, max, .. } if min > max => {
                    return Err(TaskflowCode::OcrFieldsRangeInvalid.code());
                }
                OcrFieldType::DictionaryEntry { dictionary }
                    if dictionary.path.is_empty()
                        || dictionary.path.len() > 256
                        || dictionary.path.contains(['\\', ':'])
                        || dictionary
                            .path
                            .split('/')
                            .any(|p| p.is_empty() || p == "." || p == "..") =>
                {
                    return Err(SharedCode::OcrFieldsDictionaryReferenceInvalid.code());
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// The same hash-bound truth-set format used by collection OCR. Mapping is exact after
/// trim/lowercase normalization; canonical output retains the package's spelling.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldDictionary {
    pub schema_version: String,
    pub items: Vec<String>,
    #[serde(default)]
    pub aliases: Option<Vec<OcrFieldDictionaryAlias>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldDictionaryAlias {
    pub observed: String,
    pub canonical: String,
}

impl OcrFieldDictionary {
    pub fn validate(&self, limits: &OcrFieldsLimits) -> Result<(), crate::outcome::Code> {
        if !matches!(
            self.schema_version.as_str(),
            "actingcommand.ocr-truth-set.v2"
        ) && !(self.schema_version == "actingcommand.ocr-truth-set.v1" && self.aliases.is_none())
        {
            return Err(TaskflowCode::OcrFieldsDictionarySchemaInvalid.code());
        }
        if self.items.is_empty()
            || self.items.len() > limits.max_truth_entries as usize
            || self.aliases.as_ref().is_some_and(|v| v.len() > 1024)
        {
            return Err(SharedCode::OcrFieldsDictionaryLimitExceeded.code());
        }
        let valid = |v: &str| {
            !v.trim().is_empty()
                && v.len() <= limits.max_string_bytes as usize
                && v.trim().to_lowercase().len() <= limits.max_string_bytes as usize
        };
        let mut canonical = std::collections::BTreeSet::new();
        for item in &self.items {
            if !valid(item) || !canonical.insert(item.trim().to_lowercase()) {
                return Err(TaskflowCode::OcrFieldsDictionaryAmbiguous.code());
            }
        }
        let mut aliases = std::collections::BTreeSet::new();
        for alias in self.aliases.iter().flatten() {
            let observed = alias.observed.trim().to_lowercase();
            if !valid(&alias.observed)
                || !valid(&alias.canonical)
                || canonical.contains(&observed)
                || !aliases.insert(observed)
                || !self.items.contains(&alias.canonical)
            {
                return Err(TaskflowCode::OcrFieldsDictionaryAmbiguous.code());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum OcrFieldValue {
    UnsignedInteger(u64),
    DictionaryEntry(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrFieldReason {
    Resolved,
    Empty,
    InvalidInteger,
    Overflow,
    OutOfRange,
    UnknownEntry,
    AmbiguousEntry,
    LimitExceeded,
    ProviderFailed,
    RegionUnresolved,
    NotCollected,
    /// Workflow R-11 (OCR-CONF): the text resolved, but the observation's confidence is missing
    /// or below the `minimum_confidence` its OCR target declares, so no value is accepted.
    LowConfidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrRegionRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrRegionOffset {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrAnchorMatch {
    pub rect: OcrRegionRect,
    pub raw_score: f32,
    pub score: f32,
    pub threshold: f32,
    pub passed: bool,
}

// Recognition and transport admission require finite scores.
impl Eq for OcrAnchorMatch {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrRegionUnresolvedReason {
    AnchorNotMatched,
    CoordinateOverflow,
    OutOfFrame,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrRegionEvidence {
    pub frame_width: u32,
    pub frame_height: u32,
    pub anchor_target_id: Option<String>,
    pub anchor_match: Option<OcrAnchorMatch>,
    pub offset: Option<OcrRegionOffset>,
    pub width: i32,
    pub height: i32,
    pub roi: Option<OcrRegionRect>,
    pub unresolved: Option<OcrRegionUnresolvedReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldResult {
    pub field_id: String,
    pub target_id: String,
    pub raw_text: Option<String>,
    pub normalized_text: Option<String>,
    pub value: Option<OcrFieldValue>,
    pub reason: OcrFieldReason,
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<OcrRegionEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extraction: Option<OcrFieldExtractionEvidence>,
    pub redacted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldRecord {
    pub frame_index: u32,
    pub page_id: String,
    pub group: String,
    pub fields: Vec<OcrFieldResult>,
}

pub const OCR_FIELDS_REPORT_SCHEMA: &str = "actingcommand.runtime.post-admission-ocr-fields.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldsReport {
    pub schema_version: String,
    pub declaration: OcrFieldsDeclaration,
    pub frames_collected: u32,
    pub items_collected: u32,
    pub total_observed_utf8_bytes: u64,
    pub records: Vec<OcrFieldRecord>,
    pub failure: Option<OcrFieldReason>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskFlow {
    pub schema_version: String,
    pub id: String,
    pub name: String,
    pub game: GameKey,
    pub servers: Vec<ServerKey>,
    pub resolutions: Vec<Resolution>,
    pub entrypoint: String,
    pub tasks: Vec<TaskDefinition>,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskDefinition {
    pub id: TaskId,
    pub name: String,
    pub steps: Vec<TaskStep>,
    pub on_failure: FailurePolicy,
    pub produces: Vec<String>,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskStep {
    pub id: String,
    pub description: Option<String>,
    pub primitive: String,
    pub params: BTreeMap<String, TaskParamValue>,
    pub when: Option<String>,
    pub next: Option<String>,
    pub on_failure: Option<FailurePolicy>,
    pub timeout_ms: Option<DurationMillis>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FailurePolicy {
    pub severity: Severity,
    pub retry_limit: Option<i32>,
    pub retry_delay_ms: Option<DurationMillis>,
    pub fallback_step: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TaskParamValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<TaskParamValue>),
    Object(BTreeMap<String, TaskParamValue>),
}
