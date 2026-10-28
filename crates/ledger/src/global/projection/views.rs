// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    EventSeverity, InputPayload, LedgerEventPosition, LedgerFailureResolution, LedgerPageLimit,
    LedgerReadScope, LedgerRecoveryGap, LedgerRecoveryState, LedgerRunRecovery, RecognitionPayload,
    RecognitionVerdict, RunId, RuntimeEventQueryCursor, RuntimeEventQueryPage,
    RuntimeEventQueryPageRequest,
};

// The existing query bound also caps recovery context. Truncation is an explicit Unknown.
const MAX_RECOVERY_CONTEXT_EVENTS: usize = super::super::MAX_QUERY_PAGE_EVENTS;

pub(in crate::global) struct PageSelection<'a> {
    pub through_sequence: u64,
    pub sequences: Option<&'a [u64]>,
    pub retention: Option<&'a crate::global::retention::RetentionIndex>,
    /// #411 A51 (R2): the read's work budget when this page selects its rows itself
    /// (`sequences` absent); the page stops where the budget is spent.
    pub work: Option<&'a crate::global::read_work::ReadWork>,
}

impl From<u64> for PageSelection<'_> {
    fn from(through_sequence: u64) -> Self {
        Self {
            through_sequence,
            sequences: None,
            retention: None,
            work: None,
        }
    }
}

pub(in crate::global) fn page_bounds(
    query: &EventQuery,
    profile: ProjectionProfile,
    request: &RuntimeEventQueryPageRequest,
    latest: u64,
) -> Result<(u64, u64), GlobalLedgerError> {
    let invalid =
        |code| GlobalLedgerError::request(code, LedgerLocation::ProjectLedgerViewPage.location());
    query.validate().map_err(|error| invalid(error.code()))?;
    request.validate().map_err(|error| invalid(error.code()))?;
    let (snapshot, after) = match request.cursor() {
        Some(cursor) => {
            if !cursor
                .matches(query, profile)
                .map_err(|error| invalid(error.code()))?
            {
                return Err(invalid(LedgerCode::RuntimeEventQueryCursorInvalid.code()));
            }
            (cursor.snapshot_ledger_position(), cursor.after_sequence())
        }
        None => (request.snapshot_position().unwrap_or(latest), 0),
    };
    if snapshot > latest {
        return Err(invalid(SharedCode::InvalidRuntimeEventQuerySnapshot.code()));
    }
    Ok((snapshot, after))
}

impl EventIndexes {
    pub(in crate::global) fn project_view_page<E: LedgerEventRead>(
        &self,
        events: &[E],
        query: &EventQuery,
        profile: ProjectionProfile,
        request: &RuntimeEventQueryPageRequest,
        scope: LedgerReadScope,
        selection: PageSelection<'_>,
    ) -> Result<RuntimeEventQueryPage, GlobalLedgerError> {
        let latest = selection.through_sequence;
        let (snapshot, after) = page_bounds(query, profile, request, latest)?;
        if events.last().map_or(0, E::sequence) != latest {
            return Err(GlobalLedgerError::fatal(
                LedgerCode::LedgerSnapshotBoundaryMismatch.code(),
                LedgerLocation::ProjectLedgerViewPage.location(),
            ));
        }
        // #411 A51 (R2): the last candidate a budgeted selection decided before its budget ran
        // out.
        let mut budget_stop = None;
        let selected = if let Some(sequences) = selection.sequences {
            let mut previous = after;
            let mut selected = Vec::with_capacity(sequences.len());
            for sequence in sequences {
                let event = events
                    .binary_search_by_key(sequence, E::sequence)
                    .ok()
                    .map(|index| &events[index]);
                let Some(event) = event.filter(|event| {
                    *sequence > previous
                        && *sequence <= snapshot
                        && self.matches(query, *event, snapshot)
                }) else {
                    return Err(GlobalLedgerError::fatal(
                        LedgerCode::LedgerSqlSelectionMismatch.code(),
                        LedgerLocation::ProjectLedgerViewPage.location(),
                    ));
                };
                previous = *sequence;
                selected.push(event.clone());
            }
            if selected.len() > usize::from(request.limit()) + 1 {
                return Err(GlobalLedgerError::fatal(
                    LedgerCode::LedgerSqlSelectionMismatch.code(),
                    LedgerLocation::ProjectLedgerViewPage.location(),
                ));
            }
            selected
        } else {
            let scan = self.query_page_with_observer(
                events,
                query,
                after,
                snapshot,
                usize::from(request.limit()) + 1,
                &mut || selection.work.is_none_or(|work| work.visit()),
            );
            budget_stop = scan.stopped_at;
            scan.events
        };
        let lab_related = |keys: &LabKeys| self.lab_related(keys, snapshot);
        let mut run_context = |run: &RunId| -> Result<(Vec<E>, bool), GlobalLedgerError> {
            Ok((
                self.run_ids
                    .get(run)
                    .into_iter()
                    .flatten()
                    .map(|position| &events[*position])
                    .take_while(|event| event.sequence() <= snapshot)
                    .take(MAX_RECOVERY_CONTEXT_EVENTS + 1)
                    .cloned()
                    .collect(),
                false,
            ))
        };
        project_selected(
            selected,
            budget_stop,
            query,
            profile,
            request,
            scope,
            snapshot,
            PageContext {
                lab_related: &lab_related,
                run_context: &mut run_context,
                retention: selection
                    .retention
                    .map(crate::global::retention::PageProofs::Index),
            },
        )
    }
}

/// #422 LN-WS: the records of one run at or before a page's snapshot, and whether the read's budget cut
/// them short.
pub(in crate::global) type RunContextRecords<E> = Result<(Vec<E>, bool), GlobalLedgerError>;

/// LN-6 (#422, LEAN-L §3.3): what a page reads besides its selected records: whether a record
/// with given links relates to a Lab anchor at the page's snapshot, the records of one run at or
/// before the snapshot (at most `MAX_RECOVERY_CONTEXT_EVENTS + 1` of them, in ledger order) and
/// the eviction proofs of the records' artifacts. #422 LN-WS: a run's records come with whether
/// the read's budget cut them short; the run's context is then limited.
pub(in crate::global) struct PageContext<'a, E> {
    pub lab_related: &'a dyn Fn(&LabKeys) -> bool,
    pub run_context: &'a mut dyn FnMut(&RunId) -> RunContextRecords<E>,
    pub retention: Option<crate::global::retention::PageProofs<'a>>,
}

/// One page of `selected` (in ledger order, with one record of lookahead when the page is cut
/// by its count) at `snapshot`. `budget_stop` is the last candidate a budgeted selection decided
/// before its budget ran out (#411 A51). The in-memory indexes and the SQLite store share it.
#[allow(clippy::too_many_arguments)]
pub(in crate::global) fn project_selected<E: LedgerEventRead>(
    mut selected: Vec<E>,
    budget_stop: Option<u64>,
    query: &EventQuery,
    profile: ProjectionProfile,
    request: &RuntimeEventQueryPageRequest,
    scope: LedgerReadScope,
    snapshot: u64,
    context: PageContext<'_, E>,
) -> Result<RuntimeEventQueryPage, GlobalLedgerError> {
    let invalid =
        |code| GlobalLedgerError::request(code, LedgerLocation::ProjectLedgerViewPage.location());
    let count_limited = selected.len() > usize::from(request.limit());
    let scanned_through = if count_limited {
        selected.pop().expect("lookahead exists").sequence()
    } else {
        budget_stop.unwrap_or(snapshot)
    };
    let budget_limited = !count_limited && budget_stop.is_some();
    let mut rows: Vec<_> = selected
        .iter()
        .map(|event| {
            let mut row = project_at(event, profile, snapshot);
            if let Some(retention) = context.retention {
                row.artifact_evictions = retention.observations(event, snapshot);
            }
            row.views = LedgerView::memberships(
                event.event_type(),
                event.severity(),
                event.origin().source(),
                (context.lab_related)(&LabKeys::of(event.links())),
            );
            row
        })
        .collect();
    let mut groups = run_recovery(&rows, snapshot, scope.read_complete, context.run_context)?;
    let mut byte_limited = false;
    loop {
        let has_more = count_limited || byte_limited || budget_limited;
        // A page cut by its count or its bytes goes on after its last row; one cut only by
        // the work budget goes on after the last candidate its read decided (#411 A51).
        let continues_after = if count_limited || byte_limited {
            Some(
                rows.last()
                    .ok_or_else(|| invalid(SharedCode::RuntimeEventQueryResponseTooLarge.code()))?
                    .sequence,
            )
        } else if budget_limited {
            Some(scanned_through)
        } else {
            None
        };
        let cursor = continues_after
            .map(|after| {
                RuntimeEventQueryCursor::new(snapshot, after, query, profile)
                    .map_err(|error| invalid(error.code()))
            })
            .transpose()?;
        let mut limits = Vec::new();
        if count_limited {
            limits.push(LedgerPageLimit::EventCount);
        }
        if byte_limited {
            limits.push(LedgerPageLimit::ResponseBytes);
        }
        if !scope.read_complete {
            limits.push(LedgerPageLimit::SourceIncomplete);
        }
        if budget_limited {
            limits.push(LedgerPageLimit::ReadBudget);
        }
        let page = RuntimeEventQueryPage::projected(
            rows.clone(),
            snapshot,
            request.limit(),
            has_more,
            cursor,
            LedgerReadScope {
                scanned_through_position: scanned_through,
                read_complete: scope.read_complete && !budget_limited,
                limits,
                ..scope.clone()
            },
            groups.clone(),
        );
        match page {
            Ok(page) => return Ok(page),
            Err(error)
                if error.code().as_str()
                    == SharedCode::RuntimeEventQueryResponseTooLarge.as_str()
                    && rows.len() > 1 =>
            {
                rows.pop();
                groups.retain(|group| {
                    rows.iter()
                        .any(|row| row.links.run_id() == Some(&group.run_id))
                });
                byte_limited = true;
            }
            Err(error) => return Err(invalid(error.code())),
        }
    }
}

fn run_recovery<E: LedgerEventRead>(
    rows: &[ProjectedEvent],
    snapshot: u64,
    complete: bool,
    run_context: &mut dyn FnMut(&RunId) -> RunContextRecords<E>,
) -> Result<Vec<LedgerRunRecovery>, GlobalLedgerError> {
    let runs: BTreeSet<_> = rows
        .iter()
        .filter_map(|row| row.links.run_id().copied())
        .collect();
    let mut groups = Vec::new();
    for run in runs {
        let (context, cut_short) = run_context(&run)?;
        let context: Vec<_> = context
            .into_iter()
            .take_while(|event| event.sequence() <= snapshot)
            .take(MAX_RECOVERY_CONTEXT_EVENTS + 1)
            .collect();
        // #422 LN-WS: a context the read's budget cut short is limited as one the bound cut.
        let context_limited = cut_short || context.len() > MAX_RECOVERY_CONTEXT_EVENTS;
        let context = &context[..context.len().min(MAX_RECOVERY_CONTEXT_EVENTS)];
        let failures: Vec<_> = context
            .iter()
            .filter(|event| {
                event.severity() >= EventSeverity::Warning || failure_kind(*event).is_some()
            })
            .collect();
        if failures.is_empty() && !context_limited && complete {
            continue;
        }
        let mut group = LedgerRunRecovery {
            run_id: run,
            state: LedgerRecoveryState::Unresolved,
            evidence: Vec::new(),
            gaps: Vec::new(),
        };
        if !complete {
            group.gaps.push(LedgerRecoveryGap::SourceIncomplete);
        }
        if context_limited {
            group.gaps.push(LedgerRecoveryGap::ContextLimit);
        }
        for failure in failures {
            let kind = failure_kind(failure);
            let related = kind.is_some() && has_recovery_relation(failure);
            if !related && !group.gaps.contains(&LedgerRecoveryGap::MissingRelation) {
                group.gaps.push(LedgerRecoveryGap::MissingRelation);
            }
            let success = related
                .then(|| {
                    context.iter().find(|candidate| {
                        candidate.sequence() > failure.sequence()
                            && same_recovery_relation(failure, *candidate)
                            && success_kind(*candidate) == kind
                    })
                })
                .flatten();
            if let Some(success) = success {
                let conflicting = context.iter().any(|candidate| {
                    candidate.sequence() > success.sequence()
                        && same_recovery_relation(failure, candidate)
                        && failure_kind(candidate) == kind
                });
                if conflicting && !group.gaps.contains(&LedgerRecoveryGap::ConflictingOutcome) {
                    group.gaps.push(LedgerRecoveryGap::ConflictingOutcome);
                }
            }
            group.evidence.push(LedgerFailureResolution {
                failure: position(failure),
                success: success.map(position),
            });
        }
        group.state = if !group.gaps.is_empty() {
            LedgerRecoveryState::Unknown
        } else if !group.evidence.is_empty()
            && group.evidence.iter().all(|item| item.success.is_some())
        {
            LedgerRecoveryState::Recovered
        } else {
            LedgerRecoveryState::Unresolved
        };
        groups.push(group);
    }
    Ok(groups)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RecoveryKind {
    Input,
    Recognition,
    EntryRecovery,
}

fn failure_kind<E: LedgerEventRead>(event: &E) -> Option<RecoveryKind> {
    match event.payload() {
        EventPayload::Input(InputPayload::Failed(_)) => Some(RecoveryKind::Input),
        EventPayload::Recognition(RecognitionPayload::Failed(_)) => Some(RecoveryKind::Recognition),
        EventPayload::Task(TaskPayload::Semantic(payload))
            if matches!(payload.fact(), TaskSemanticFact::EntryRecoveryFailed { .. }) =>
        {
            Some(RecoveryKind::EntryRecovery)
        }
        _ => None,
    }
}

fn success_kind<E: LedgerEventRead>(event: &E) -> Option<RecoveryKind> {
    match event.payload() {
        EventPayload::Input(InputPayload::Completed(_)) => Some(RecoveryKind::Input),
        EventPayload::Recognition(RecognitionPayload::Completed(payload))
            if payload.recognition_verdict() == Some(RecognitionVerdict::PageMatched) =>
        {
            Some(RecoveryKind::Recognition)
        }
        EventPayload::Task(TaskPayload::Semantic(payload))
            if matches!(
                payload.fact(),
                TaskSemanticFact::EntryRecoveryCompleted { .. }
            ) =>
        {
            Some(RecoveryKind::EntryRecovery)
        }
        _ => None,
    }
}

fn has_recovery_relation<E: LedgerEventRead>(event: &E) -> bool {
    event.links().action_id().is_some()
        || event.links().recognition_id().is_some()
        || failure_kind(event) == Some(RecoveryKind::EntryRecovery)
}

fn same_recovery_relation<E: LedgerEventRead>(failure: &E, candidate: &E) -> bool {
    if failure.links().run_id().is_none() || failure.links().run_id() != candidate.links().run_id()
    {
        return false;
    }
    let action = failure.links().action_id();
    let recognition = failure.links().recognition_id();
    if action.is_some() || recognition.is_some() {
        return action.is_none_or(|id| candidate.links().action_id() == Some(id))
            && recognition.is_none_or(|id| candidate.links().recognition_id() == Some(id));
    }
    match (failure.payload(), candidate.payload()) {
        (
            EventPayload::Task(TaskPayload::Semantic(left)),
            EventPayload::Task(TaskPayload::Semantic(right)),
        ) => match (left.fact(), right.fact()) {
            // Workflow #336 PKG-S2: the same layer's package, by `package_id`; a failure whose
            // package was refused before its id was read pairs with nothing.
            (
                TaskSemanticFact::EntryRecoveryFailed {
                    package_label: Some(left),
                    ..
                },
                TaskSemanticFact::EntryRecoveryCompleted {
                    package_label: right,
                    ..
                }
                | TaskSemanticFact::EntryRecoveryFailed {
                    package_label: Some(right),
                    ..
                },
            ) => left == right,
            _ => false,
        },
        _ => false,
    }
}

fn position<E: LedgerEventRead>(event: &E) -> LedgerEventPosition {
    LedgerEventPosition {
        event_id: *event.event_id(),
        sequence: event.sequence(),
    }
}
