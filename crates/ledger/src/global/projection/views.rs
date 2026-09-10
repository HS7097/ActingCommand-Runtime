// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    EventSeverity, InputPayload, LedgerEventPosition, LedgerFailureResolution, LedgerPageLimit,
    LedgerReadScope, LedgerRecoveryGap, LedgerRecoveryState, LedgerRunRecovery, RecognitionPayload,
    RecognitionVerdict, RuntimeEventQueryCursor, RuntimeEventQueryPage,
    RuntimeEventQueryPageRequest,
};

// The existing query bound also caps recovery context. Truncation is an explicit Unknown.
const MAX_RECOVERY_CONTEXT_EVENTS: usize = super::super::MAX_QUERY_PAGE_EVENTS;

pub(in crate::global) struct PageSelection<'a> {
    pub through_sequence: u64,
    pub sequences: Option<&'a [u64]>,
}

impl From<u64> for PageSelection<'_> {
    fn from(through_sequence: u64) -> Self {
        Self {
            through_sequence,
            sequences: None,
        }
    }
}

pub(in crate::global) fn page_bounds(
    query: &EventQuery,
    profile: ProjectionProfile,
    request: &RuntimeEventQueryPageRequest,
    latest: u64,
) -> Result<(u64, u64), GlobalLedgerError> {
    let invalid = |code| GlobalLedgerError::request(code, "project_ledger_view_page");
    query
        .validate()
        .map_err(|_| invalid("invalid_event_query_bounds"))?;
    request.validate().map_err(|error| invalid(error.code()))?;
    let (snapshot, after) = match request.cursor() {
        Some(cursor) => {
            if !cursor
                .matches(query, profile)
                .map_err(|error| invalid(error.code()))?
            {
                return Err(invalid("runtime_event_query_cursor_invalid"));
            }
            (cursor.snapshot_ledger_position(), cursor.after_sequence())
        }
        None => (request.snapshot_position().unwrap_or(latest), 0),
    };
    if snapshot > latest {
        return Err(invalid("invalid_runtime_event_query_snapshot"));
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
        let invalid = |code| GlobalLedgerError::request(code, "project_ledger_view_page");
        let latest = selection.through_sequence;
        let (snapshot, after) = page_bounds(query, profile, request, latest)?;
        if events.last().map_or(0, E::sequence) != latest {
            return Err(GlobalLedgerError::fatal(
                "ledger_snapshot_boundary_mismatch",
                "project_ledger_view_page",
            ));
        }
        let mut selected = if let Some(sequences) = selection.sequences {
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
                        "ledger_sql_selection_mismatch",
                        "project_ledger_view_page",
                    ));
                };
                previous = *sequence;
                selected.push(event.clone());
            }
            if selected.len() > usize::from(request.limit()) + 1 {
                return Err(GlobalLedgerError::fatal(
                    "ledger_sql_selection_mismatch",
                    "project_ledger_view_page",
                ));
            }
            selected
        } else {
            self.query_page(
                events,
                query,
                after,
                snapshot,
                usize::from(request.limit()) + 1,
            )
        };
        let count_limited = selected.len() > usize::from(request.limit());
        let scanned_through = if count_limited {
            selected.pop().expect("lookahead exists").sequence()
        } else {
            snapshot
        };
        let mut rows: Vec<_> = selected
            .iter()
            .map(|event| {
                let mut row = project(event, profile);
                row.views = LedgerView::memberships(
                    event.event_type(),
                    event.severity(),
                    event.origin().source(),
                    self.lab_related(event, snapshot),
                );
                row
            })
            .collect();
        let mut groups = self.run_recovery(events, &rows, snapshot, scope.read_complete);
        let mut byte_limited = false;
        loop {
            let has_more = count_limited || byte_limited;
            let cursor = if has_more {
                let last = rows
                    .last()
                    .ok_or_else(|| invalid("runtime_event_query_response_too_large"))?;
                Some(
                    RuntimeEventQueryCursor::new(snapshot, last.sequence, query, profile)
                        .map_err(|error| invalid(error.code()))?,
                )
            } else {
                None
            };
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
            let page = RuntimeEventQueryPage::new(
                rows.clone(),
                snapshot,
                request.limit(),
                has_more,
                cursor,
            )
            .and_then(|page| {
                page.with_projection_context(
                    LedgerReadScope {
                        scanned_through_position: scanned_through,
                        limits,
                        ..scope.clone()
                    },
                    groups.clone(),
                )
            });
            match page {
                Ok(page) => return Ok(page),
                Err(error)
                    if error.code() == "runtime_event_query_response_too_large"
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
        &self,
        events: &[E],
        rows: &[ProjectedEvent],
        snapshot: u64,
        complete: bool,
    ) -> Vec<LedgerRunRecovery> {
        let runs: BTreeSet<_> = rows
            .iter()
            .filter_map(|row| row.links.run_id().copied())
            .collect();
        runs.into_iter()
            .filter_map(|run| {
                let positions = self.run_ids.get(&run)?;
                let context: Vec<_> = positions
                    .iter()
                    .map(|position| &events[*position])
                    .take_while(|event| event.sequence() <= snapshot)
                    .take(MAX_RECOVERY_CONTEXT_EVENTS + 1)
                    .collect();
                let context_limited = context.len() > MAX_RECOVERY_CONTEXT_EVENTS;
                let context = &context[..context.len().min(MAX_RECOVERY_CONTEXT_EVENTS)];
                let failures: Vec<_> = context
                    .iter()
                    .copied()
                    .filter(|event| {
                        event.severity() >= EventSeverity::Warning || failure_kind(*event).is_some()
                    })
                    .collect();
                if failures.is_empty() && !context_limited && complete {
                    return None;
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
                            context.iter().copied().find(|candidate| {
                                candidate.sequence() > failure.sequence()
                                    && same_recovery_relation(failure, candidate)
                                    && success_kind(*candidate) == kind
                            })
                        })
                        .flatten();
                    if let Some(success) = success {
                        let conflicting = context.iter().copied().any(|candidate| {
                            candidate.sequence() > success.sequence()
                                && same_recovery_relation(failure, candidate)
                                && failure_kind(candidate) == kind
                        });
                        if conflicting
                            && !group.gaps.contains(&LedgerRecoveryGap::ConflictingOutcome)
                        {
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
                Some(group)
            })
            .collect()
    }
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
            (
                TaskSemanticFact::EntryRecoveryFailed {
                    package_sha256: left,
                    ..
                },
                TaskSemanticFact::EntryRecoveryCompleted {
                    package_sha256: right,
                    ..
                }
                | TaskSemanticFact::EntryRecoveryFailed {
                    package_sha256: right,
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
