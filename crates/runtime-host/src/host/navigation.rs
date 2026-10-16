// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #373 NAV-5 (model M-373 v1.1 §4.4, §5, §8.2, §8.3, §9): full-match entry routing.
//!
//! A run whose game root holds `pages.json` and `navigation.json`, and whose package's entry
//! pages are all pages of `pages.json`, navigates at its entry gate (§4.4 ②) when its entry does
//! not pass and no explicit layer brought it there (its declared prerequisite chain, or a
//! request's own recovery package, ran first and the entry still did not pass). It takes the
//! place of the return-home fallback (R16, R23). A run the host starts itself (a startup
//! package, a ladder rung) and every layer keep today's gate.
//!
//! - **Locate** (§5.2). The run's feature package evaluates every listed page on one frame: the
//!   last frame of the entry's own check first, a new frame for every later locate. A shared
//!   interrupt page the run's package does not declare itself is handed to its handling (NAV-6)
//!   and the run locates again; two of them are `contained_task_interrupt_ambiguous`. Exactly
//!   one other page is the located page (`contained_task_navigation_located`, and the run's
//!   `task.page` fact); none is `contained_task_navigation_page_unrecognized`, several
//!   `contained_task_navigation_page_ambiguous`. Nothing is guessed.
//! - **Route and hops** (§5.3). The fewest-hops route over the run's server's edges, ties by the
//!   edges' order in the file; page ids are compared as equal or not, never read. Each hop's
//!   package is admitted on its first use and checked for its part, runs as a layer
//!   (`role: navigation_hop`), and the feature package then awaits the edge's `to` page for the
//!   package's step timeout; a hop that did not arrive is
//!   `contained_task_navigation_hop_unconfirmed`, and the run locates again. No route is
//!   `contained_task_navigation_route_missing`.
//! - **Recovery chain** (§5.4). Unrecognised, ambiguous or without a route, the next step of
//!   the run's server's recovery chain runs as a layer (`role: recovery_chain`, a run the host
//!   starts itself: wave-17 ruling 1) and the run locates again. A spent chain ends the run with
//!   `contained_task_navigation_recovery_exhausted`, caused by the last verdict that sent the run
//!   to the chain.
//! - **Arrival** (§5.3). On an entry page the run's package's own recheck decides: it starts, or
//!   the run ends with `contained_task_navigation_entry_unconfirmed`.
//! - **Bounds** (§5.5). 16 locates and 16 hops per gate, 1000 steps per run
//!   (`contained_task_navigation_limit_reached`); the run's deadline as for every run.
//!
//! Packages are named by the standard pack's data only; no request carries a field for them (no
//! parameter passes between packages, C373a).

use super::contained_task::{EntryRecoveryRuntime, PackageIdentity, RuntimeContainedTask};
use super::runtime_facts::TASK_PAGE_FACT_KEY;
use super::standard_pack::{
    GameRoot, RegisteredPack, RunInterrupts, interrupt_failure, package_invalid,
};
use super::*;
use actingcommand_contract::{
    MAX_NAVIGATION_HOPS, MAX_NAVIGATION_LOCATES, MAX_NAVIGATION_RECORDED_PAGES, NavigationLimit,
    NavigationProblem, NavigationRole, StandardPackNavigation, StandardPackPages,
    TaskEntryRecoveryRole, TaskNavigationHop, TaskTimingBudgetOrigin,
};
use actingcommand_execution_kernel::{
    MAX_GATED_STEPS, Scene, SharedInterruptVerdict, canonical_page_anchor,
};

/// One edge of the run's server: run `pack` on the page `from` to reach the page `to` (ids of
/// `pages.json`).
#[derive(Debug, Clone)]
pub(super) struct RunEdge {
    from: String,
    to: String,
    pack: RegisteredPack,
}

/// Workflow #373 NAV-5: a run's navigation data, fixed at its preparation (§7).
pub(super) struct RunNavigation {
    /// The run's package's entry pages as ids of `pages.json`, in its order; when some are not
    /// listed, those (§4.4: the gate does not navigate then).
    target: Result<Vec<String>, Vec<String>>,
    edges: Vec<RunEdge>,
    chain: Vec<RegisteredPack>,
}

/// Workflow #373 NAV-5 (§3.5, §4.4): the navigation data of a run of `prepared` from its game
/// root: the edges and recovery chain steps whose packages are of the run's server, in the
/// file's order. `None` when the package has no page entry (an application entry, `any`, or a
/// mode without one), whose gate never navigates.
pub(super) fn run_navigation(
    root: &GameRoot,
    pages: &StandardPackPages,
    navigation: &StandardPackNavigation,
    prepared: &PreparedContainedTask,
) -> Option<RunNavigation> {
    let game = prepared.game();
    let entry = prepared
        .navigation_entry_pages()?
        .iter()
        .map(|page| canonical_page_anchor(game, page))
        .collect::<Vec<_>>();
    let listed = pages
        .pages
        .iter()
        .map(|page| page.id.as_str())
        .collect::<BTreeSet<_>>();
    let unlisted = entry
        .iter()
        .filter(|page| !listed.contains(page.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let server = prepared.server();
    let of_server = |package_id: &str| {
        root.registry
            .get(package_id)
            .is_some_and(|entry| entry.server == server)
    };
    let edges = navigation
        .edges
        .iter()
        .filter(|edge| of_server(edge.package_id.as_str()))
        .filter_map(|edge| {
            Some(RunEdge {
                from: edge.from.clone(),
                to: edge.to.clone(),
                pack: RegisteredPack::of(root, &edge.package_id)?,
            })
        })
        .collect();
    let chain = navigation
        .recovery_chain
        .iter()
        .filter(|package_id| of_server(package_id.as_str()))
        .filter_map(|package_id| RegisteredPack::of(root, package_id))
        .collect();
    Some(RunNavigation {
        target: if unlisted.is_empty() {
            Ok(entry)
        } else {
            Err(unlisted)
        },
        edges,
        chain,
    })
}

/// Workflow #373 NAV-5 (§4.4 ②): whether a run with these standard-pack data navigates at its
/// entry gate: its game has navigation data and its package's entry pages are all listed.
pub(super) fn navigation_applies(interrupts: Option<&RunInterrupts>) -> bool {
    interrupts
        .and_then(|interrupts| interrupts.navigation.as_ref())
        .is_some_and(|navigation| navigation.target.is_ok())
}

/// §5.3: the fewest-hops route from `from` to a page of `target` over `edges`, ties by the
/// edges' order (a breadth-first search that keeps each page's first way in); `None` without
/// one. Pages are compared as equal or not.
fn find_route(edges: &[RunEdge], from: &str, target: &[String]) -> Option<Vec<RunEdge>> {
    let mut previous = BTreeMap::<&str, usize>::new();
    let mut seen = BTreeSet::from([from]);
    let mut queue = VecDeque::from([from]);
    while let Some(page) = queue.pop_front() {
        if target.iter().any(|entry| entry == page) {
            let mut route = Vec::new();
            let mut cursor = page;
            while cursor != from {
                let index = *previous.get(cursor)?;
                route.push(edges[index].clone());
                cursor = edges[index].from.as_str();
            }
            route.reverse();
            return Some(route);
        }
        for (index, edge) in edges.iter().enumerate() {
            if edge.from == page && seen.insert(edge.to.as_str()) {
                previous.insert(edge.to.as_str(), index);
                queue.push_back(edge.to.as_str());
            }
        }
    }
    None
}

/// What one locate found (§5.2 item 3).
enum Verdict {
    /// A shared interrupt page was handled; the run locates again.
    Interrupt,
    /// Exactly one listed page.
    Located(String),
    /// No page, several pages or several interrupt pages: the outcome that sends the run to
    /// its recovery chain.
    Lost(Outcome),
}

/// One entry gate's navigation: its data and what it has spent so far.
struct NavigationGate {
    navigation: RunNavigation,
    target: Vec<String>,
    /// The steps the gate's layers executed, its explicit layers' before it included.
    executed: u32,
    locates: u32,
    hops: u32,
    /// The index of the recovery chain's next step.
    chain_next: usize,
    /// The outcome of the last verdict that sent the run to its recovery chain.
    last: Option<Outcome>,
    /// The edges' and chain steps' packages, admitted once each per run (§7).
    admitted: BTreeMap<String, PreparedContainedTask>,
}

/// `contained_task_navigation_limit_reached` (§5.5).
fn limit_reached(
    limit: NavigationLimit,
    count: u64,
    limit_count: u64,
) -> ContainedTaskRunError<RequestFailure> {
    interrupt_failure(
        RuntimeHostError::request(
            HostCode::ContainedTaskNavigationLimitReached,
            HostLocation::NavigationRoute,
            RuntimeErrorCode::BackendOperationFailed,
        )
        .with_outcome_values(|outcome| {
            outcome
                .navigation_limit(limit)
                .count(count)
                .limit_count(limit_count)
        }),
    )
}

/// The run's limit of steps (§3.10, `MAX_GATED_STEPS`): `executed` steps so far, the interrupt
/// layers' `interrupt_steps` and the next layer's `max_steps`.
fn check_steps(
    executed: u32,
    interrupt_steps: u32,
    max_steps: u32,
) -> Result<(), ContainedTaskRunError<RequestFailure>> {
    let steps = u64::from(executed) + u64::from(interrupt_steps) + u64::from(max_steps);
    if steps > MAX_GATED_STEPS {
        return Err(limit_reached(
            NavigationLimit::Steps,
            steps,
            MAX_GATED_STEPS,
        ));
    }
    Ok(())
}

/// A capture or cancellation failure of the navigation's own waits, classified as the kernel
/// classifies the runtime's errors.
fn operation_failure(failure: RequestFailure) -> ContainedTaskRunError<RequestFailure> {
    if failure.error.is_fatal() {
        ContainedTaskRunError::Boundary(failure)
    } else {
        ContainedTaskRunError::NonfatalOperation(failure)
    }
}

fn state_invalid() -> ContainedTaskRunError<RequestFailure> {
    ContainedTaskRunError::task(SharedCode::ContainedTaskStateInvalid.as_str())
}

fn step_limit() -> ContainedTaskRunError<RequestFailure> {
    ContainedTaskRunError::task(SharedCode::ContainedTaskPrerequisiteStepLimit.as_str())
}

impl RuntimeContainedTask<'_> {
    /// Workflow #373 NAV-5 (§4.4 ②): whether this run's entry gate navigates.
    pub(super) fn navigates(&self) -> bool {
        navigation_applies(self.interrupts.as_deref())
    }

    /// Workflow #373 NAV-5 (§4.4): `contained_task_navigation_entry_unlisted` when the run's
    /// game has navigation data and its package's entry pages are not all listed: the gate does
    /// not navigate, and the run is as without navigation data.
    pub(super) fn note_unlisted_entry(&self) -> Result<(), RequestFailure> {
        let Some(Err(unlisted)) = self
            .interrupts
            .as_ref()
            .and_then(|interrupts| interrupts.navigation.as_ref())
            .map(|navigation| &navigation.target)
        else {
            return Ok(());
        };
        self.record_entry_fact(TaskSemanticFact::NavigationWarning {
            outcome: Outcome::new(HostCode::ContainedTaskNavigationEntryUnlisted)
                .operation(HostLocation::NavigationRoute)
                .page_ids(unlisted.clone()),
        })
    }

    /// Workflow #373 NAV-5 (§4.4): the entry gate of a run that navigates and has no explicit
    /// layer: the first check of its entry (its unmatched frame offered to the shared interrupt
    /// pages), the navigation when it does not pass, then the run's package, its steps after the
    /// navigation's layers'.
    pub(super) fn run_navigated(
        &mut self,
        prepared: &PreparedContainedTask,
    ) -> Result<ContainedTaskOutcome, ContainedTaskRunError<RequestFailure>> {
        let required_pages = prepared
            .navigation_entry_pages()
            .ok_or_else(state_invalid)?;
        self.retain_entry_scene = true;
        let initial = prepared.recognize_navigation_entry(self);
        self.retain_entry_scene = false;
        // A failure of the first check, before any fact of the gate, is returned unchanged, as
        // the home entry's and a prerequisite gate's first check is.
        let initial = initial?;
        let matched = initial.is_some();
        self.record_entry_fact(TaskSemanticFact::EntryRecognition {
            phase: TaskEntryRecognitionPhase::Initial,
            required_pages,
            matched_page: initial,
            matched,
        })
        .map_err(ContainedTaskRunError::Boundary)?;
        self.record_entry_fact(TaskSemanticFact::EntryRecoveryDecision { required: !matched })
            .map_err(ContainedTaskRunError::Boundary)?;
        self.entry_preflight_recorded = true;
        let executed = if matched {
            self.entry_scene = None;
            0
        } else {
            self.navigate_to_entry(prepared, 0)?
        };
        self.begin_layer(executed).ok_or_else(step_limit)?;
        self.record_entry_fact(TaskSemanticFact::EntryTargetDisposition {
            disposition: TaskEntryTargetDisposition::Started,
            outcome: None,
        })
        .map_err(ContainedTaskRunError::Boundary)?;
        let mut outcome = self.run_main(prepared)?;
        outcome.executed_steps = self
            .layer_end(outcome.executed_steps)
            .ok_or_else(step_limit)?;
        Ok(outcome)
    }

    /// Workflow #373 NAV-5 (§5): the navigation of the run's entry gate after `executed_before`
    /// steps of its explicit layers: locate, route, hops and recovery chain until the run's
    /// package's own recheck passes. The steps the gate's layers executed, those before it
    /// included; a failure ends the gate (`EntryTargetDisposition { FailClosed }`, after a
    /// failed layer's `EntryRecoveryFailed`) and the run with it.
    pub(super) fn navigate_to_entry(
        &mut self,
        prepared: &PreparedContainedTask,
        executed_before: u32,
    ) -> Result<u32, ContainedTaskRunError<RequestFailure>> {
        let scene = self.entry_scene.take();
        let navigation = self
            .interrupts
            .as_mut()
            .and_then(|interrupts| interrupts.navigation.take());
        let Some(navigation) = navigation else {
            return Err(state_invalid());
        };
        let Ok(target) = navigation.target.clone() else {
            return Err(state_invalid());
        };
        let mut gate = NavigationGate {
            navigation,
            target,
            executed: executed_before,
            locates: 0,
            hops: 0,
            chain_next: 0,
            last: None,
            admitted: BTreeMap::new(),
        };
        match self.navigate(prepared, &mut gate, scene) {
            Ok(()) => Ok(gate.executed),
            Err(error) => Err(self.close_navigation(error)),
        }
    }

    /// The navigation's failure closes the gate with its outcome; a poisoning or fatal one is
    /// returned unchanged.
    fn close_navigation(
        &self,
        error: ContainedTaskRunError<RequestFailure>,
    ) -> ContainedTaskRunError<RequestFailure> {
        let severe = matches!(
            &error,
            ContainedTaskRunError::Boundary(failure)
                | ContainedTaskRunError::NonfatalOperation(failure)
                if failure.poison_runtime || failure.error.is_fatal()
        );
        if severe {
            return error;
        }
        let outcome = match &error {
            ContainedTaskRunError::Task(task) => task.outcome(),
            ContainedTaskRunError::Boundary(failure)
            | ContainedTaskRunError::NonfatalOperation(failure) => failure.error.outcome(),
        };
        match self.record_entry_fact(TaskSemanticFact::EntryTargetDisposition {
            disposition: TaskEntryTargetDisposition::FailClosed,
            outcome: Some(outcome),
        }) {
            Ok(()) => error,
            Err(failure) => ContainedTaskRunError::Boundary(failure),
        }
    }

    fn navigate(
        &mut self,
        prepared: &PreparedContainedTask,
        gate: &mut NavigationGate,
        mut scene: Option<Scene>,
    ) -> Result<(), ContainedTaskRunError<RequestFailure>> {
        let features = self.navigation_features()?;
        let game = prepared.game().to_owned();
        // The run's package's own pages, which no shared interrupt page intercepts (§6.1).
        let own = prepared
            .page_ids()
            .into_iter()
            .map(|page| canonical_page_anchor(&game, page))
            .collect::<BTreeSet<_>>();
        let budget = Duration::from_millis(prepared.effective_timing().step_timeout.milliseconds);
        'locate: loop {
            let page = match self.locate(&features, &own, gate, scene.take(), budget)? {
                Verdict::Interrupt => continue 'locate,
                Verdict::Lost(outcome) => {
                    gate.last = Some(outcome);
                    None
                }
                Verdict::Located(page) => Some(page),
            };
            if let Some(page) = page {
                if gate.target.contains(&page) {
                    self.record_entry_fact(TaskSemanticFact::NavigationRoutePlanned {
                        from_page: page,
                        hops: Vec::new(),
                    })
                    .map_err(ContainedTaskRunError::Boundary)?;
                    break 'locate;
                }
                match find_route(&gate.navigation.edges, &page, &gate.target) {
                    None => {
                        let outcome = Outcome::new(HostCode::ContainedTaskNavigationRouteMissing)
                            .operation(HostLocation::NavigationRoute)
                            .page_id(page.as_str());
                        self.record_entry_fact(TaskSemanticFact::NavigationWarning {
                            outcome: outcome.clone(),
                        })
                        .map_err(ContainedTaskRunError::Boundary)?;
                        gate.last = Some(outcome);
                    }
                    Some(route) => {
                        let planned = u64::from(gate.hops) + route.len() as u64;
                        if planned > u64::from(MAX_NAVIGATION_HOPS) {
                            return Err(limit_reached(
                                NavigationLimit::Hops,
                                planned,
                                u64::from(MAX_NAVIGATION_HOPS),
                            ));
                        }
                        self.record_entry_fact(TaskSemanticFact::NavigationRoutePlanned {
                            from_page: page,
                            hops: route
                                .iter()
                                .map(|edge| TaskNavigationHop {
                                    from: edge.from.clone(),
                                    to: edge.to.clone(),
                                    package_label: edge.pack.package_id.clone(),
                                })
                                .collect(),
                        })
                        .map_err(ContainedTaskRunError::Boundary)?;
                        for edge in &route {
                            if !self.hop(prepared, &features, gate, edge)? {
                                continue 'locate;
                            }
                        }
                        break 'locate;
                    }
                }
            }
            self.chain_step(prepared, gate)?;
        }
        self.recheck_entry(prepared, gate)
    }

    /// One locate (§5.2): the frame `scene` (the entry check's last one) or a new one, every
    /// listed page evaluated on it by the feature package in one scene evaluation, and its
    /// verdict recorded (`NavigationLocated`).
    fn locate(
        &mut self,
        features: &PreparedContainedTask,
        own: &BTreeSet<String>,
        gate: &mut NavigationGate,
        scene: Option<Scene>,
        budget: Duration,
    ) -> Result<Verdict, ContainedTaskRunError<RequestFailure>> {
        if gate.locates >= MAX_NAVIGATION_LOCATES {
            return Err(limit_reached(
                NavigationLimit::Locates,
                u64::from(gate.locates) + 1,
                u64::from(MAX_NAVIGATION_LOCATES),
            ));
        }
        gate.locates += 1;
        let attempt = gate.locates;
        let scene = match scene {
            Some(scene) => scene,
            None => {
                let now = self.monotonic_now();
                features.capture_scene(self, now, budget, TaskTimingBudgetOrigin::EntryRecovery)?
            }
        };
        let pages = features.page_ids();
        // An evaluation error (a reading budget past REC-M's, a provider failure) ends the run
        // as a page recognition error does (§5.2 item 3).
        let matched = features
            .matching_pages_on_scene(&scene, &pages)
            .map_err(ContainedTaskRunError::Task)?
            .iter()
            .map(|page| canonical_page_anchor(features.game(), page))
            .collect::<Vec<_>>();
        drop(scene);
        let hits =
            self.navigation_interrupt_hits(&matched.iter().cloned().collect::<BTreeSet<_>>(), own);
        match hits.as_slice() {
            [] => {}
            [(index, page)] => {
                let page = page.clone();
                if self.handle_navigation_interrupt(*index)? == SharedInterruptVerdict::Handled {
                    let count = self.interrupt_count(&page);
                    self.record_located(
                        attempt,
                        &matched,
                        Some(page.clone()),
                        Outcome::new(HostCode::ContainedTaskInterruptHandled)
                            .operation(HostLocation::NavigationLocate)
                            .page_id(page.as_str())
                            .count(count),
                    )?;
                }
                return Ok(Verdict::Interrupt);
            }
            several => {
                let page_ids = several
                    .iter()
                    .map(|(_, page)| page.clone())
                    .collect::<Vec<_>>();
                let outcome = Outcome::new(HostCode::ContainedTaskInterruptAmbiguous)
                    .operation(HostLocation::InterruptCheck)
                    .page_ids(page_ids.clone());
                self.record_entry_fact(TaskSemanticFact::InterruptAmbiguous {
                    page_ids,
                    outcome: outcome.clone(),
                })
                .map_err(ContainedTaskRunError::Boundary)?;
                self.record_located(attempt, &matched, None, outcome.clone())?;
                return Ok(Verdict::Lost(outcome));
            }
        }
        match matched.as_slice() {
            [page] => {
                let page = page.clone();
                self.record_located(
                    attempt,
                    &matched,
                    None,
                    Outcome::new(HostCode::ContainedTaskNavigationLocated)
                        .operation(HostLocation::NavigationLocate)
                        .page_id(page.as_str())
                        .count(attempt),
                )?;
                // §8.2: the run's current page, as the page list names it.
                self.host
                    .record_changed_instance_string_fact(
                        self.control.instance_id,
                        TASK_PAGE_FACT_KEY,
                        &format!("{}/{page}", features.game()),
                    )
                    .map_err(|error| {
                        ContainedTaskRunError::Boundary(RequestFailure::poison_without_terminal(
                            error,
                        ))
                    })?;
                Ok(Verdict::Located(page))
            }
            [] => {
                let outcome = Outcome::new(HostCode::ContainedTaskNavigationPageUnrecognized)
                    .operation(HostLocation::NavigationLocate)
                    .count(attempt);
                self.record_located(attempt, &matched, None, outcome.clone())?;
                Ok(Verdict::Lost(outcome))
            }
            several => {
                let outcome = Outcome::new(HostCode::ContainedTaskNavigationPageAmbiguous)
                    .operation(HostLocation::NavigationLocate)
                    .page_ids(several.to_vec());
                self.record_located(attempt, &matched, None, outcome.clone())?;
                Ok(Verdict::Lost(outcome))
            }
        }
    }

    fn record_located(
        &self,
        attempt: u32,
        matched: &[String],
        interrupt_page: Option<String>,
        outcome: Outcome,
    ) -> Result<(), ContainedTaskRunError<RequestFailure>> {
        self.record_entry_fact(TaskSemanticFact::NavigationLocated {
            attempt,
            matched_pages: matched
                .iter()
                .take(MAX_NAVIGATION_RECORDED_PAGES)
                .cloned()
                .collect(),
            matched_total: u32::try_from(matched.len()).unwrap_or(u32::MAX),
            interrupt_page,
            outcome,
        })
        .map_err(ContainedTaskRunError::Boundary)
    }

    /// One hop (§5.3): the edge's package, admitted once and checked for the edge, runs as a
    /// layer of the run (`role: navigation_hop`), and the feature package awaits the edge's `to`
    /// page for the package's step timeout, a frame every capture interval of the package.
    /// Whether the run arrived.
    fn hop(
        &mut self,
        prepared: &PreparedContainedTask,
        features: &PreparedContainedTask,
        gate: &mut NavigationGate,
        edge: &RunEdge,
    ) -> Result<bool, ContainedTaskRunError<RequestFailure>> {
        gate.hops += 1;
        self.admit_navigation_package(prepared, gate, &edge.pack, Some(edge))?;
        let package = gate
            .admitted
            .get(&edge.pack.package_id)
            .ok_or_else(state_invalid)?;
        check_steps(
            gate.executed,
            self.interrupt_steps,
            package.maximum_executed_steps(),
        )?;
        let timing = package.effective_timing();
        let budget = Duration::from_millis(timing.step_timeout.milliseconds);
        let interval = Duration::from_millis(timing.capture_interval.milliseconds);
        self.planned_steps = gate
            .executed
            .saturating_add(package.maximum_executed_steps())
            .saturating_add(prepared.maximum_executed_steps());
        let executed = self.run_navigation_layer(
            package,
            TaskEntryRecoveryRole::NavigationHop,
            gate.executed,
            false,
        )?;
        gate.executed = gate.executed.checked_add(executed).ok_or_else(step_limit)?;
        // The feature package's page of the edge's `to`.
        let to = features
            .page_ids()
            .into_iter()
            .find(|page| canonical_page_anchor(features.game(), page) == edge.to)
            .map(str::to_owned)
            .ok_or_else(state_invalid)?;
        let arrived = self.await_listed_page(features, &to, budget, interval)?;
        self.record_entry_fact(TaskSemanticFact::EntryRecognition {
            phase: TaskEntryRecognitionPhase::NavigationTarget,
            required_pages: vec![to.clone()],
            matched_page: arrived.then_some(to),
            matched: arrived,
        })
        .map_err(ContainedTaskRunError::Boundary)?;
        if !arrived {
            self.record_entry_fact(TaskSemanticFact::NavigationWarning {
                outcome: Outcome::new(HostCode::ContainedTaskNavigationHopUnconfirmed)
                    .operation(HostLocation::NavigationHop)
                    .package_id(edge.pack.package_id.as_str())
                    .page_id(edge.to.as_str()),
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        }
        Ok(arrived)
    }

    /// §5.3 item 4: the feature package's page `page` (a detector page id), evaluated alone on a
    /// new frame every `interval` until it passes or `budget` is spent.
    fn await_listed_page(
        &mut self,
        features: &PreparedContainedTask,
        page: &str,
        budget: Duration,
        interval: Duration,
    ) -> Result<bool, ContainedTaskRunError<RequestFailure>> {
        let started = self.monotonic_now();
        loop {
            let scene = features.capture_scene(
                self,
                started,
                budget,
                TaskTimingBudgetOrigin::EntryRecovery,
            )?;
            if !features
                .matching_pages_on_scene(&scene, &[page])
                .map_err(ContainedTaskRunError::Task)?
                .is_empty()
            {
                return Ok(true);
            }
            let elapsed = self.monotonic_now().saturating_duration_since(started);
            if elapsed >= budget {
                return Ok(false);
            }
            self.candidate_sampling_checkpoint()
                .map_err(operation_failure)?;
            self.pause(interval.min(budget.saturating_sub(elapsed)));
        }
    }

    /// The recovery chain's next step (§5.4): admitted once and checked for its part, run as a
    /// layer (`role: recovery_chain`) the host starts itself; a spent chain ends the run with
    /// `contained_task_navigation_recovery_exhausted`.
    fn chain_step(
        &mut self,
        prepared: &PreparedContainedTask,
        gate: &mut NavigationGate,
    ) -> Result<(), ContainedTaskRunError<RequestFailure>> {
        let Some(pack) = gate.navigation.chain.get(gate.chain_next).cloned() else {
            let mut error = RuntimeHostError::request(
                HostCode::ContainedTaskNavigationRecoveryExhausted,
                HostLocation::NavigationChain,
                RuntimeErrorCode::BackendOperationFailed,
            )
            .with_outcome_values(|outcome| outcome.count(gate.chain_next));
            if let Some(cause) = gate.last.clone() {
                error = error.with_caused_by(cause);
            }
            return Err(interrupt_failure(error));
        };
        gate.chain_next += 1;
        self.admit_navigation_package(prepared, gate, &pack, None)?;
        let package = gate
            .admitted
            .get(&pack.package_id)
            .ok_or_else(state_invalid)?;
        check_steps(
            gate.executed,
            self.interrupt_steps,
            package.maximum_executed_steps(),
        )?;
        self.planned_steps = gate
            .executed
            .saturating_add(package.maximum_executed_steps())
            .saturating_add(prepared.maximum_executed_steps());
        let executed = self.run_navigation_layer(
            package,
            TaskEntryRecoveryRole::RecoveryChain,
            gate.executed,
            true,
        )?;
        gate.executed = gate.executed.checked_add(executed).ok_or_else(step_limit)?;
        Ok(())
    }

    /// An edge's (`edge`) or a chain step's package, admitted on the run's first use (§7) and
    /// checked for its part every time: eligible as a prerequisite layer of the run (§5.3 item 1;
    /// a chain step declares no prerequisite package of its own, §5.4); an edge's package's entry
    /// holds the edge's `from`, it ends on the edge's `to`, and its entry is no application entry.
    fn admit_navigation_package(
        &mut self,
        prepared: &PreparedContainedTask,
        gate: &mut NavigationGate,
        pack: &RegisteredPack,
        edge: Option<&RunEdge>,
    ) -> Result<(), ContainedTaskRunError<RequestFailure>> {
        let role = if edge.is_some() {
            NavigationRole::Edge
        } else {
            NavigationRole::Chain
        };
        if !gate.admitted.contains_key(&pack.package_id) {
            let admitted = self.admit_layer_package(pack, role)?;
            gate.admitted.insert(pack.package_id.clone(), admitted);
        }
        let package = gate
            .admitted
            .get(&pack.package_id)
            .ok_or_else(state_invalid)?;
        let identity = PackageIdentity::of(prepared);
        let problem = match package
            .package_descriptor()
            .prerequisite_incompatibility_with(
                &identity.game,
                &identity.server,
                identity.resolution,
                edge.is_none(),
            ) {
            Some("game" | "server" | "resolution") => Some(NavigationProblem::IdentityMismatch),
            Some("return_home_declares_prerequisite") => {
                Some(NavigationProblem::PrerequisiteDeclared)
            }
            Some(_) => Some(NavigationProblem::RoleIneligible),
            None => edge.and_then(|edge| {
                let game = package.game();
                if package.has_application_entry() {
                    Some(NavigationProblem::ApplicationEntry)
                } else if !(package.entry_holds_page(&edge.from)
                    || package
                        .linear_entry_page()
                        .is_some_and(|page| canonical_page_anchor(game, page) == edge.from))
                {
                    Some(NavigationProblem::EntryMismatch)
                } else if !package.terminal_holds_page(&edge.to) {
                    Some(NavigationProblem::TerminalMismatch)
                } else {
                    None
                }
            }),
        };
        match problem {
            Some(problem) => Err(package_invalid(&pack.package_id, role, problem, None)),
            None => Ok(()),
        }
    }

    /// Runs `package` as a layer of the run (§5.3 item 2, §5.4 item 2) after `executed_before`
    /// steps of the gate's layers, as a prerequisite layer runs, `host_layer` for a run the host
    /// starts itself (a recovery chain step): its records carry `role`, its own failure is
    /// carried out unchanged. The steps it executed.
    fn run_navigation_layer(
        &mut self,
        package: &PreparedContainedTask,
        role: TaskEntryRecoveryRole,
        executed_before: u32,
        host_layer: bool,
    ) -> Result<u32, ContainedTaskRunError<RequestFailure>> {
        let package_label = package.package_label().to_owned();
        let role = Some(role);
        self.record_entry_fact(TaskSemanticFact::EntryRecoveryPackageAdmitted {
            package_label: package_label.clone(),
            role,
        })
        .map_err(ContainedTaskRunError::Boundary)?;
        // M-373 §8.3: one effective configuration record per distinct layer package.
        if self.configuration_records > 0
            && self
                .interrupts
                .as_mut()
                .is_some_and(|interrupts| interrupts.configured.insert(package_label.clone()))
        {
            self.interrupt_configurations = self.interrupt_configurations.saturating_add(1);
            self.record_configuration(
                EffectiveConfigurationFacts::EntryRecovery {
                    package_label: package_label.clone(),
                    timing: package.effective_timing(),
                },
                None,
                None,
                None,
            )
            .map_err(ContainedTaskRunError::Boundary)?;
        }
        self.begin_layer(executed_before).ok_or_else(step_limit)?;
        let previous_timing = self.task_timing.context();
        let execution = {
            let mut layer_runtime = EntryRecoveryRuntime { inner: self };
            if host_layer {
                package.run_as_host_layer(&mut layer_runtime)
            } else {
                package.run_as_prerequisite(&mut layer_runtime)
            }
        };
        if let Err(ContainedTaskRunError::Task(error)) = &execution {
            self.task_timing
                .task_failure(error.timing(), error.timing_check_position());
        }
        self.task_timing.replace_context(previous_timing);
        let outcome = match execution {
            Ok(outcome) => outcome,
            Err(error) => {
                if let ContainedTaskRunError::Boundary(failure) = &error
                    && (failure.poison_runtime || failure.error.is_fatal())
                {
                    return Err(error);
                }
                let code = match &error {
                    ContainedTaskRunError::Task(error) => error.outcome(),
                    ContainedTaskRunError::Boundary(failure)
                    | ContainedTaskRunError::NonfatalOperation(failure) => failure.error.outcome(),
                };
                self.record_entry_fact(TaskSemanticFact::EntryRecoveryFailed {
                    package_label: Some(package_label),
                    outcome: code,
                    role,
                })
                .map_err(ContainedTaskRunError::Boundary)?;
                return Err(error);
            }
        };
        let Some(final_page) = outcome.final_page else {
            return Err(ContainedTaskRunError::task(
                HostCode::ContainedTaskPrerequisiteFinalPageMissing.as_str(),
            ));
        };
        self.record_entry_fact(TaskSemanticFact::EntryRecoveryCompleted {
            package_label,
            final_page,
            executed_steps: outcome.executed_steps,
            role,
        })
        .map_err(ContainedTaskRunError::Boundary)?;
        Ok(outcome.executed_steps)
    }

    /// §5.3 "到了入口": the run's package's own recheck of its entry for its step timeout; the
    /// run's steps (its package's `max_steps` after the gate's) stay within 1000.
    fn recheck_entry(
        &mut self,
        prepared: &PreparedContainedTask,
        gate: &NavigationGate,
    ) -> Result<(), ContainedTaskRunError<RequestFailure>> {
        let required_pages = prepared
            .navigation_entry_pages()
            .ok_or_else(state_invalid)?;
        let budget = Duration::from_millis(prepared.effective_timing().step_timeout.milliseconds);
        let waited = prepared.await_navigation_entry(self, budget, TaskTimingBudgetOrigin::Task)?;
        self.record_entry_fact(TaskSemanticFact::EntryRecognition {
            phase: TaskEntryRecognitionPhase::PostRecovery,
            required_pages,
            matched: waited.matched.is_some(),
            matched_page: waited.matched,
        })
        .map_err(ContainedTaskRunError::Boundary)?;
        if waited.unmatched.is_some() {
            // The failure's timing belongs to the wait's own budget.
            self.task_timing.replace_context(Some(waited.timing));
            let page_ids = gate.target.clone();
            return Err(interrupt_failure(
                RuntimeHostError::request(
                    HostCode::ContainedTaskNavigationEntryUnconfirmed,
                    HostLocation::NavigationRoute,
                    RuntimeErrorCode::BackendOperationFailed,
                )
                .with_outcome_values(|outcome| outcome.page_ids(page_ids)),
            ));
        }
        check_steps(
            gate.executed,
            self.interrupt_steps,
            prepared.maximum_executed_steps(),
        )?;
        self.planned_steps = gate
            .executed
            .saturating_add(prepared.maximum_executed_steps());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // Workflow #373 NAV-5, test row N5-2 (model M-373 v1.1 §5.3): the route from the located
    // page to the entry has the fewest hops, ties go to the edge the file names first, and page
    // ids are compared only as equal or not: the same graph under other ids gives the same
    // hops under those ids.

    use super::*;

    fn edge(from: &str, to: &str, package_id: &str) -> RunEdge {
        RunEdge {
            from: from.to_owned(),
            to: to.to_owned(),
            pack: RegisteredPack {
                package_id: package_id.to_owned(),
                path: PathBuf::from(package_id),
            },
        }
    }

    fn hops(route: Option<Vec<RunEdge>>) -> Vec<(String, String, String)> {
        route
            .expect("a route")
            .into_iter()
            .map(|edge| (edge.from, edge.to, edge.pack.package_id))
            .collect()
    }

    fn owned(hops: &[(&str, &str, &str)]) -> Vec<(String, String, String)> {
        hops.iter()
            .map(|(from, to, package_id)| {
                (
                    (*from).to_owned(),
                    (*to).to_owned(),
                    (*package_id).to_owned(),
                )
            })
            .collect()
    }

    // From `c` to `home`: three hops through `d` and `e` (named first), and two two-hop routes,
    // through `a` (its first edge named before `b`'s) and through `b` (its second edge named
    // before `a`'s).
    const GRAPH: [(&str, &str, &str); 7] = [
        ("c", "d", "p1"),
        ("d", "e", "p2"),
        ("e", "home", "p3"),
        ("c", "a", "p4"),
        ("c", "b", "p5"),
        ("b", "home", "p6"),
        ("a", "home", "p7"),
    ];

    #[test]
    fn n5_2_the_route_has_the_fewest_hops_and_ties_go_to_the_edge_named_first() {
        let edges = GRAPH
            .iter()
            .map(|(from, to, package_id)| edge(from, to, package_id))
            .collect::<Vec<_>>();
        let home = ["home".to_owned()];
        assert_eq!(
            hops(find_route(&edges, "c", &home)),
            owned(&[("c", "a", "p4"), ("a", "home", "p7")])
        );
        // Of a set of entry pages, `e` and `home` are both two hops away; the route through the
        // edge named first wins.
        assert_eq!(
            hops(find_route(
                &edges,
                "c",
                &["home".to_owned(), "e".to_owned()]
            )),
            owned(&[("c", "d", "p1"), ("d", "e", "p2")])
        );
        assert_eq!(
            hops(find_route(&edges, "d", &home)),
            owned(&[("d", "e", "p2"), ("e", "home", "p3")])
        );
        // No edge leaves `home`: no route back to `c`.
        assert!(find_route(&edges, "home", &["c".to_owned()]).is_none());

        // The same graph under other ids, whose order differs from the first ids' (`a` sorts
        // before `b`, `y` after `e`): the hops are the same edges under the new ids.
        let rename = |page: &str| -> String {
            match page {
                "a" => "y",
                "b" => "e",
                "c" => "n",
                "d" => "k",
                "e" => "w",
                "home" => "r",
                other => panic!("no page {other}"),
            }
            .to_owned()
        };
        let renamed = GRAPH
            .iter()
            .map(|(from, to, package_id)| edge(&rename(from), &rename(to), package_id))
            .collect::<Vec<_>>();
        assert_eq!(
            hops(find_route(&renamed, "n", &["r".to_owned()])),
            owned(&[("n", "y", "p4"), ("y", "r", "p7")])
        );
    }
}
