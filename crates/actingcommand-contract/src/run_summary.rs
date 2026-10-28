// SPDX-License-Identifier: AGPL-3.0-only

//! P32 B-1 (#422 6473873443; design rd-1028/design-p32.md §2 B, §3 B-1; since 0.12.27): one
//! contained run as its facts show it, folded by one pure accumulator.
//!
//! The run status read of runtime-client folds a run's facts with [`RunFold`] and a run
//! listing folds its listing facts with [`RunListingFacts`]; the Runtime's in-memory run summary
//! (P32 B-2) folds the same facts with the same accumulators, so the listing it answers
//! (`QueryRuns`, [`crate::QueryRunsPage`]) and the client's own reads can never disagree.
//!
//! **Commutative.** A fold reads one fact with its sequence at a time and keeps only what does
//! not depend on the order the facts come in: the least or greatest sequence of a kind, a set
//! keyed by sequence (the same fact twice is once), and for the facts a run records once (its
//! request's origin, its admission, its terminal, its lease's terminal) the two of the lowest
//! sequences, which tell none, one and more than one apart. Feeding the same facts in any order,
//! each any number of times, leaves the same fold. A sequence names one record, so two facts with
//! one sequence are one fact.
//!
//! **Anomalies stay anomalies.** Two terminals, two admissions, two origins, a semantic fact
//! without its run's identity or with another run's, and links that disagree are kept and
//! reported ([`RunFactAnomaly`]), never resolved by picking one: the run status read refuses such
//! a run, as it did before B-1. Of several anomalies of the facts themselves (two terminals,
//! admissions or origins, a missing or another run's identity) the fold reports the one found at
//! the lowest sequence, so a reader that folds the facts in sequence order and asks after each
//! fact refuses at the same fact and with the same code as the read that checked each fact as it
//! came. Links that disagree, the lease's among them, are reported only after those: once every
//! fact is folded and none of those is found, whatever their sequence ([`RunFold::links`],
//! [`RunFold::lease_id`]), as the read always did.

use crate::outcome::CodeStr;
use crate::{
    ContainedTaskCancellationReason, ContainedTaskLeaseTerminal, CorrelationId, EventId,
    EventLinks, EventSource, EventType, InstanceId, LeaseId, RequestId, RunId, TaskId, TaskOutcome,
    TaskRunIdentity, TaskSemanticFact,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The facts a run is listed by (Workflow #391 item 3): its main admission, its entry facts and
/// its terminal. A run without an entry check starts with its main admission, a run with one
/// records an entry fact once that check is done, and a run whose first check fails ends with
/// its terminal.
pub const RUN_LISTING_EVENT_TYPES: [EventType; 5] = [
    EventType::TaskRequested,
    EventType::TaskEntryPreflight,
    EventType::TaskCompleted,
    EventType::TaskFailed,
    EventType::TaskCancelled,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunDispatch {
    Manual,
    Scheduled,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOrigin {
    Cli,
    Ui,
    Lab,
    Scheduler,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainedRunState {
    NotFound,
    Admitted,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    /// A run fact precedes a later Runtime start or takeover without a terminal; uncertain.
    InterruptedUnterminated,
}

/// A run's terminal as `actingcommand.run-status.v1` shows it. `failure_code` is the code of the
/// terminal's committed outcome (ledger revision 3). A relayed spelling the catalog does not
/// register was committed as `outcome_relay_unregistered`, and that is the code this member
/// reads; the original spelling (a token) stays in that outcome's `raw_code` value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainedRunTerminal {
    pub outcome: TaskOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<CodeStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_page: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executed_steps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancellation_reason: Option<ContainedTaskCancellationReason>,
    pub sequence: u64,
    pub event_id: EventId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainedRunLease {
    pub lease_id: LeaseId,
    pub terminal: Option<ContainedTaskLeaseTerminal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainedRunProgress {
    pub step_index: u32,
    pub operation_label: String,
    pub page: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainedRunEvidence {
    pub snapshot_ledger_position: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admitted_sequence: Option<u64>,
    pub restarted_after_admission: bool,
}

/// One run's brief status (`RunStatusMode::Brief` of runtime-client, without its schema string):
/// what [`RunFold::brief`] answers and what one row of a `QueryRuns` page carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunBriefStatus {
    pub request_id: RequestId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<CorrelationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<InstanceId>,
    pub dispatch: RunDispatch,
    pub origin: RunOrigin,
    /// Workflow #336 PKG-S2 (option 丙): the admitted package's `package_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_label: Option<String>,
    /// The `package_id` of each prerequisite or recovery package the run admitted, in order.
    pub recovery_package_labels: Vec<String>,
    pub state: ContainedRunState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<ContainedRunTerminal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<ContainedRunLease>,
    pub evidence: ContainedRunEvidence,
}

/// One fact of a run as the fold reads it: one record's sequence, identity, time, type, source
/// and links and, for a semantic task fact whose payload was read, that fact. A record that is
/// no semantic task fact (a request's origin, a lease's release or expiry), and a listing page's
/// record, which carries no payload, has none.
#[derive(Debug, Clone, Copy)]
pub struct RunFact<'a> {
    pub sequence: u64,
    pub event_id: EventId,
    pub timestamp_unix_ms: u64,
    pub event_type: EventType,
    pub source: EventSource,
    pub links: &'a EventLinks,
    pub semantic: Option<&'a TaskSemanticFact>,
}

/// Why a run's facts are no one run's status. Each maps to the run status read's code of the
/// same name (`run_status_*`); the fold reports, never resolves, them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RunFactAnomaly {
    /// A semantic fact lacks one of its run's identity links.
    IdentityMissing,
    /// A semantic fact names another run, or a link disagrees with the run's.
    IdentityMismatch,
    /// The request has two origin records.
    OriginInconsistent,
    /// The run has two main admissions.
    PackageFactCountInvalid,
    /// The run has two terminals.
    TerminalStateInconsistent,
    /// The run's lease has two terminals.
    LeaseTerminalInconsistent,
}

/// The facts of one kind at the two lowest sequences: enough to tell none, one and more than
/// one apart in any order; the same sequence twice is once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LowestTwo<T> {
    first: Option<(u64, T)>,
    second: Option<(u64, T)>,
}

impl<T> LowestTwo<T> {
    const fn new() -> Self {
        Self {
            first: None,
            second: None,
        }
    }

    fn note(&mut self, sequence: u64, value: T) {
        let first = self.first.as_ref().map(|(found, _)| *found);
        let second = self.second.as_ref().map(|(found, _)| *found);
        // The same fact again.
        if first == Some(sequence) || second == Some(sequence) {
            return;
        }
        match (first, second) {
            (None, _) => self.first = Some((sequence, value)),
            (Some(found), _) if sequence < found => {
                self.second = self.first.take();
                self.first = Some((sequence, value));
            }
            (Some(_), None) => self.second = Some((sequence, value)),
            (Some(_), Some(found)) if sequence < found => self.second = Some((sequence, value)),
            (Some(_), Some(_)) => {}
        }
    }

    /// The one fact of the kind; `None` with none or more than one.
    fn only(&self) -> Option<&(u64, T)> {
        match (&self.first, &self.second) {
            (Some(first), None) => Some(first),
            _ => None,
        }
    }

    /// The sequence at which a second fact of the kind was found, in sequence order.
    fn second_sequence(&self) -> Option<u64> {
        self.second.as_ref().map(|(sequence, _)| *sequence)
    }

    const fn is_empty(&self) -> bool {
        self.first.is_none()
    }
}

/// One link over a run's records: none yet, one value, or two different values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SingleLink<T> {
    Absent,
    One(T),
    Conflict,
}

impl<T: Copy + PartialEq> SingleLink<T> {
    fn note(&mut self, value: Option<&T>) {
        let Some(value) = value else {
            return;
        };
        *self = match *self {
            Self::Absent => Self::One(*value),
            Self::One(found) if found == *value => Self::One(found),
            Self::One(_) | Self::Conflict => Self::Conflict,
        };
    }

    fn value(self) -> Result<Option<T>, RunFactAnomaly> {
        match self {
            Self::Absent => Ok(None),
            Self::One(value) => Ok(Some(value)),
            Self::Conflict => Err(RunFactAnomaly::IdentityMismatch),
        }
    }

    /// Whether the link agrees with `expected`: absent, or that one value.
    fn agrees(self, expected: T) -> bool {
        match self {
            Self::Absent => true,
            Self::One(value) => value == expected,
            Self::Conflict => false,
        }
    }
}

/// The links of a run's records, each one value over all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RunLinkValues {
    request: SingleLink<RequestId>,
    correlation: SingleLink<CorrelationId>,
    instance: SingleLink<InstanceId>,
    task: SingleLink<TaskId>,
    run: SingleLink<RunId>,
    lease: SingleLink<LeaseId>,
}

impl RunLinkValues {
    const fn new() -> Self {
        Self {
            request: SingleLink::Absent,
            correlation: SingleLink::Absent,
            instance: SingleLink::Absent,
            task: SingleLink::Absent,
            run: SingleLink::Absent,
            lease: SingleLink::Absent,
        }
    }

    fn note(&mut self, links: &EventLinks) {
        self.request.note(links.request_id());
        self.correlation.note(links.correlation_id());
        self.instance.note(links.instance_id());
        self.task.note(links.task_id());
        self.run.note(links.run_id());
        self.lease.note(links.lease_id());
    }
}

/// The run's identity links, each one value over all its records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunFoldLinks {
    pub request_id: Option<RequestId>,
    pub correlation_id: Option<CorrelationId>,
    pub instance_id: Option<InstanceId>,
    pub task_id: Option<TaskId>,
    pub run_id: Option<RunId>,
}

/// A lease's release or expiry records (`lease.released`, `lease.expired`), folded by sequence:
/// none is no terminal, one is that terminal, two are an anomaly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunLeaseTerminals {
    terminals: LowestTwo<ContainedTaskLeaseTerminal>,
}

impl RunLeaseTerminals {
    pub const fn new() -> Self {
        Self {
            terminals: LowestTwo::new(),
        }
    }

    /// One record of the lease; a record of another type is no terminal.
    pub fn fold(&mut self, sequence: u64, event_type: EventType) {
        let terminal = match event_type {
            EventType::LeaseReleased => ContainedTaskLeaseTerminal::Released,
            EventType::LeaseExpired => ContainedTaskLeaseTerminal::Expired,
            _ => return,
        };
        self.terminals.note(sequence, terminal);
    }

    pub const fn is_empty(&self) -> bool {
        self.terminals.is_empty()
    }

    /// The lease's terminal: `None` without one; two are `LeaseTerminalInconsistent`.
    pub fn terminal(&self) -> Result<Option<ContainedTaskLeaseTerminal>, RunFactAnomaly> {
        match (self.terminals.first, self.terminals.second) {
            (None, _) => Ok(None),
            (Some((_, terminal)), None) => Ok(Some(terminal)),
            (Some(_), Some(_)) => Err(RunFactAnomaly::LeaseTerminalInconsistent),
        }
    }
}

impl Default for RunLeaseTerminals {
    fn default() -> Self {
        Self::new()
    }
}

/// What a run listing knows of one run from its listing facts in the listing's window (#422
/// QR-2b): the first of them (the listing's order), whether the window holds its main admission
/// and the terminal facts the window holds of it. Fed the run's listing facts only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunListingFacts {
    order_sequence: u64,
    opened: bool,
    terminals: LowestTwo<EventType>,
}

impl RunListingFacts {
    /// The run's first listing fact read.
    pub fn first(sequence: u64, event_type: EventType) -> Self {
        let mut facts = Self {
            order_sequence: sequence,
            opened: false,
            terminals: LowestTwo::new(),
        };
        facts.fold(sequence, event_type);
        facts
    }

    /// One more listing fact of the run: it can only move the first one earlier.
    pub fn fold(&mut self, sequence: u64, event_type: EventType) {
        self.order_sequence = self.order_sequence.min(sequence);
        match event_type {
            EventType::TaskRequested => self.opened = true,
            EventType::TaskCompleted | EventType::TaskFailed | EventType::TaskCancelled => {
                self.terminals.note(sequence, event_type);
            }
            _ => {}
        }
    }

    /// The sequence of the run's first listing fact read: what a listing orders by.
    pub const fn order_sequence(&self) -> u64 {
        self.order_sequence
    }

    /// Whether the facts read hold the run's main admission (`task.requested`).
    pub const fn opened(&self) -> bool {
        self.opened
    }

    /// The state the listing facts alone prove: succeeded or cancelled when they hold the run's
    /// one terminal and that terminal is `task.completed` or `task.cancelled` (a terminal's
    /// record type is its `task_result`'s, so the run's status read would answer that state). A
    /// failed terminal, no terminal or more than one terminal proves nothing.
    pub fn settled(&self) -> Option<ContainedRunState> {
        match self.terminals.only() {
            Some((_, EventType::TaskCompleted)) => Some(ContainedRunState::Succeeded),
            Some((_, EventType::TaskCancelled)) => Some(ContainedRunState::Cancelled),
            _ => None,
        }
    }
}

/// What a run's own reads add to its facts: whether the Runtime started or took over after the
/// run's first fact and after its admission, and its lease's terminal when it has a lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunReads {
    pub restarted_after_first_fact: bool,
    pub restarted_after_admission: bool,
    pub lease_terminal: Option<ContainedTaskLeaseTerminal>,
}

/// The fold of one run's facts (the record types runtime-client's run status reads: its
/// request's origin, its semantic facts, its lease's release or expiry). Pure and commutative:
/// see the module documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunFold {
    origins: LowestTwo<(RunDispatch, RunOrigin)>,
    /// The lowest sequence of a semantic fact without its run's identity.
    identity_missing: Option<u64>,
    /// Each identity the run's semantic facts carry with its lowest sequence, by that sequence.
    identities: Vec<(TaskRunIdentity, u64)>,
    /// The run's first semantic fact: its sequence and time.
    first_fact: Option<(u64, u64)>,
    admissions: LowestTwo<String>,
    recovery_package_labels: BTreeMap<u64, String>,
    /// The latest step fact: its sequence and the progress it shows.
    progress: Option<(u64, ContainedRunProgress)>,
    step_started: bool,
    terminals: LowestTwo<(ContainedRunTerminal, u64)>,
    links: RunLinkValues,
    lease_terminals: RunLeaseTerminals,
    listing: Option<RunListingFacts>,
}

impl Default for RunFold {
    fn default() -> Self {
        Self::new()
    }
}

impl RunFold {
    pub const fn new() -> Self {
        Self {
            origins: LowestTwo::new(),
            identity_missing: None,
            identities: Vec::new(),
            first_fact: None,
            admissions: LowestTwo::new(),
            recovery_package_labels: BTreeMap::new(),
            progress: None,
            step_started: false,
            terminals: LowestTwo::new(),
            links: RunLinkValues::new(),
            lease_terminals: RunLeaseTerminals::new(),
            listing: None,
        }
    }

    /// Folds one fact.
    pub fn fold(&mut self, fact: RunFact<'_>) {
        let sequence = fact.sequence;
        self.links.note(fact.links);
        if let Some(origin) = request_origin(fact.event_type, fact.source) {
            self.origins.note(sequence, origin);
        }
        if fact.links.lease_id().is_some() {
            self.lease_terminals.fold(sequence, fact.event_type);
        }
        if RUN_LISTING_EVENT_TYPES.contains(&fact.event_type) {
            self.listing = Some(match self.listing {
                Some(mut listing) => {
                    listing.fold(sequence, fact.event_type);
                    listing
                }
                None => RunListingFacts::first(sequence, fact.event_type),
            });
        }
        let Some(semantic) = fact.semantic else {
            return;
        };
        let Some(identity) = semantic_identity(fact.links) else {
            self.identity_missing = Some(
                self.identity_missing
                    .map_or(sequence, |lowest| lowest.min(sequence)),
            );
            return;
        };
        match self
            .identities
            .iter_mut()
            .find(|(found, _)| *found == identity)
        {
            Some((_, lowest)) => *lowest = (*lowest).min(sequence),
            None => self.identities.push((identity, sequence)),
        }
        // Distinct identities have distinct lowest sequences (each its own fact), so this order
        // is one order whatever the order the facts came in.
        self.identities.sort_by_key(|(_, lowest)| *lowest);
        if self.first_fact.is_none_or(|(first, _)| sequence < first) {
            self.first_fact = Some((sequence, fact.timestamp_unix_ms));
        }
        match semantic {
            TaskSemanticFact::PackageAdmitted { package_label, .. } => {
                self.admissions.note(sequence, package_label.clone());
            }
            TaskSemanticFact::EntryRecoveryPackageAdmitted { package_label, .. } => {
                self.recovery_package_labels
                    .entry(sequence)
                    .or_insert_with(|| package_label.clone());
            }
            TaskSemanticFact::StepStarted {
                step_index,
                operation_label,
                from_page,
                ..
            } => {
                self.step_started = true;
                self.note_progress(sequence, *step_index, operation_label, from_page);
            }
            TaskSemanticFact::StepFinished {
                step_index,
                operation_label,
                page_label,
                ..
            } => self.note_progress(sequence, *step_index, operation_label, page_label),
            TaskSemanticFact::TerminalCommitted {
                task_result,
                final_page,
                executed_steps,
                outcome,
                ..
            } => {
                let failure_code = outcome.as_ref().map(|outcome| outcome.code().clone());
                let terminal = ContainedRunTerminal {
                    outcome: *task_result,
                    final_page: final_page.clone(),
                    executed_steps: *executed_steps,
                    cancellation_reason: (*task_result == TaskOutcome::Cancelled).then_some(
                        ContainedTaskCancellationReason::from_failure_code(failure_code.as_ref()),
                    ),
                    failure_code,
                    sequence,
                    event_id: fact.event_id,
                };
                self.terminals
                    .note(sequence, (terminal, fact.timestamp_unix_ms));
            }
            _ => {}
        }
    }

    fn note_progress(&mut self, sequence: u64, step_index: u32, operation_label: &str, page: &str) {
        if self
            .progress
            .as_ref()
            .is_none_or(|(latest, _)| sequence > *latest)
        {
            self.progress = Some((
                sequence,
                ContainedRunProgress {
                    step_index,
                    operation_label: operation_label.to_owned(),
                    page: page.to_owned(),
                },
            ));
        }
    }

    /// The anomaly of the facts folded that a read checking each fact in sequence order meets
    /// first: the one found at the lowest sequence (a second origin, admission or terminal at
    /// the second's sequence, a semantic fact without its run's identity or with another one's
    /// at its own) and, at one sequence, the fact's identity before what it records. `None` when
    /// the facts are one run's facts so far. The run's links and its lease are checked apart
    /// ([`Self::links`], [`Self::lease_id`]), after every fact.
    pub fn anomaly_found(&self) -> Option<RunFactAnomaly> {
        // A semantic fact that names another run than the run's first one is found at the
        // lowest sequence of a fact with another identity.
        let mismatch = self.identities.get(1).map(|(_, lowest)| *lowest);
        [
            (self.identity_missing, RunFactAnomaly::IdentityMissing),
            (mismatch, RunFactAnomaly::IdentityMismatch),
            (
                self.origins.second_sequence(),
                RunFactAnomaly::OriginInconsistent,
            ),
            (
                self.admissions.second_sequence(),
                RunFactAnomaly::PackageFactCountInvalid,
            ),
            (
                self.terminals.second_sequence(),
                RunFactAnomaly::TerminalStateInconsistent,
            ),
        ]
        .into_iter()
        .filter_map(|(found, anomaly)| found.map(|sequence| (sequence, anomaly)))
        .min()
        .map(|(_, anomaly)| anomaly)
    }

    /// The identity of the run's first semantic fact.
    pub fn identity(&self) -> Option<TaskRunIdentity> {
        self.identities.first().map(|(identity, _)| *identity)
    }

    /// The sequence and time of the run's first semantic fact.
    pub const fn first_fact(&self) -> Option<(u64, u64)> {
        self.first_fact
    }

    /// The run's identity links, each one value over every record folded and each the value of
    /// the run's first semantic fact when it has one (a request's audit may omit links, but any
    /// it carries agrees with the run); else `IdentityMismatch`.
    pub fn links(&self) -> Result<RunFoldLinks, RunFactAnomaly> {
        let links = RunFoldLinks {
            request_id: self.links.request.value()?,
            correlation_id: self.links.correlation.value()?,
            instance_id: self.links.instance.value()?,
            task_id: self.links.task.value()?,
            run_id: self.links.run.value()?,
        };
        if let Some(identity) = self.identity()
            && !(self.links.request.agrees(identity.request_id)
                && self.links.correlation.agrees(identity.correlation_id)
                && self.links.instance.agrees(identity.instance_id)
                && self.links.task.agrees(identity.task_id)
                && self.links.run.agrees(identity.run_id))
        {
            return Err(RunFactAnomaly::IdentityMismatch);
        }
        Ok(links)
    }

    /// The run's lease, one value over every record folded; else `IdentityMismatch`.
    pub fn lease_id(&self) -> Result<Option<LeaseId>, RunFactAnomaly> {
        self.links.lease.value()
    }

    /// The release or expiry records of the run's lease among the facts folded (the run's own,
    /// linked to its request). Without one, the run's status read reads its lease's.
    pub const fn lease_terminals(&self) -> RunLeaseTerminals {
        self.lease_terminals
    }

    /// The sequence of the run's main admission.
    pub fn admitted_sequence(&self) -> Option<u64> {
        self.admissions.only().map(|(sequence, _)| *sequence)
    }

    /// The run's terminal and when it was recorded.
    pub fn terminal(&self) -> Option<(&ContainedRunTerminal, u64)> {
        self.terminals
            .only()
            .map(|(_, (terminal, timestamp))| (terminal, *timestamp))
    }

    /// The latest step fact's progress.
    pub fn progress(&self) -> Option<&ContainedRunProgress> {
        self.progress.as_ref().map(|(_, progress)| progress)
    }

    /// What the run's listing facts among the facts folded show.
    pub const fn listing(&self) -> Option<RunListingFacts> {
        self.listing
    }

    /// The run's state: by its terminal; without one, `not_found` without a semantic fact,
    /// `interrupted_unterminated` when the Runtime started or took over after its first fact,
    /// `running` once a step started or without a main admission, else `admitted`.
    pub fn state(&self, restarted_after_first_fact: bool) -> ContainedRunState {
        match self.terminal() {
            Some((terminal, _)) => match terminal.outcome {
                TaskOutcome::Success => ContainedRunState::Succeeded,
                TaskOutcome::Failure => ContainedRunState::Failed,
                TaskOutcome::Cancelled => ContainedRunState::Cancelled,
            },
            None if self.first_fact.is_none() => ContainedRunState::NotFound,
            None if restarted_after_first_fact => ContainedRunState::InterruptedUnterminated,
            None if self.step_started || self.admitted_sequence().is_none() => {
                ContainedRunState::Running
            }
            None => ContainedRunState::Admitted,
        }
    }

    /// The run's brief status at `snapshot`, with what its own reads add (`reads`): the first
    /// anomaly of the facts, of their links or of their lease when there is one.
    pub fn brief(
        &self,
        request_id: RequestId,
        snapshot: u64,
        reads: RunReads,
    ) -> Result<RunBriefStatus, RunFactAnomaly> {
        if let Some(anomaly) = self.anomaly_found() {
            return Err(anomaly);
        }
        let links = self.links()?;
        if self
            .identity()
            .is_some_and(|identity| identity.request_id != request_id)
        {
            return Err(RunFactAnomaly::IdentityMismatch);
        }
        let lease = self.lease_id()?.map(|lease_id| ContainedRunLease {
            lease_id,
            terminal: reads.lease_terminal,
        });
        let (dispatch, origin) = self
            .origins
            .only()
            .map_or((RunDispatch::Unknown, RunOrigin::Unknown), |(_, found)| {
                *found
            });
        Ok(RunBriefStatus {
            request_id,
            correlation_id: links.correlation_id,
            run_id: links.run_id,
            task_id: links.task_id,
            instance_id: links.instance_id,
            dispatch,
            origin,
            package_label: self
                .admissions
                .only()
                .map(|(_, package_label)| package_label.clone()),
            recovery_package_labels: self.recovery_package_labels.values().cloned().collect(),
            state: self.state(reads.restarted_after_first_fact),
            terminal: self.terminal().map(|(terminal, _)| terminal.clone()),
            lease,
            evidence: ContainedRunEvidence {
                snapshot_ledger_position: snapshot,
                admitted_sequence: self.admitted_sequence(),
                restarted_after_admission: reads.restarted_after_admission,
            },
        })
    }
}

/// The request's origin record: a client command is manual, the scheduler's receipt scheduled.
pub const fn request_origin(
    event_type: EventType,
    source: EventSource,
) -> Option<(RunDispatch, RunOrigin)> {
    match event_type {
        EventType::CliCommand => Some((RunDispatch::Manual, RunOrigin::Cli)),
        EventType::UiAction => Some((RunDispatch::Manual, RunOrigin::Ui)),
        EventType::LabRequest => Some((RunDispatch::Manual, RunOrigin::Lab)),
        EventType::CommandReceived if matches!(source, EventSource::Scheduler) => {
            Some((RunDispatch::Scheduled, RunOrigin::Scheduler))
        }
        _ => None,
    }
}

/// A semantic fact's run identity, from its links.
fn semantic_identity(links: &EventLinks) -> Option<TaskRunIdentity> {
    Some(TaskRunIdentity {
        request_id: *links.request_id()?,
        correlation_id: *links.correlation_id()?,
        instance_id: *links.instance_id()?,
        task_id: *links.task_id()?,
        run_id: *links.run_id()?,
    })
}

#[cfg(test)]
#[path = "run_summary/gate_422_p32b1.rs"]
mod gate_422_p32b1;
