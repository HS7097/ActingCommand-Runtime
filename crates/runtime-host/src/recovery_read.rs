// SPDX-License-Identifier: AGPL-3.0-only

//! Bounded startup reads (Workflow #381 R4a). Every writer command waits at most the Ledger's
//! reply bound, so a startup read is split into pages pinned to one ledger position: no single
//! command grows with the whole Ledger, and the events read are the ones one query at that
//! position would have returned, in the same order.

use actingcommand_contract::{EventQuery, EventType};
use actingcommand_ledger::{GlobalLedger, GlobalLedgerResult, PersistedEvent};

/// Events per page of a startup read.
pub(crate) const RECOVERY_PAGE_EVENTS: usize = 256;

/// The events `query` selects with a sequence in `1..=through`, in ledger order, read in pages
/// of at most `page_events` events.
pub(crate) fn read_pages(
    ledger: &GlobalLedger,
    query: &EventQuery,
    through: u64,
    page_events: usize,
) -> GlobalLedgerResult<Vec<PersistedEvent>> {
    let mut events = Vec::new();
    let mut after = 0;
    loop {
        let page = ledger.query_page(query.clone(), after, through, page_events)?;
        let exhausted = page.len() < page_events;
        if let Some(last) = page.last() {
            after = last.sequence();
        }
        events.extend(page);
        if exhausted {
            return Ok(events);
        }
    }
}

/// The events of `event_types` with a sequence in `1..=through`, in ledger order. Each type
/// is read on its own index, in pages of at most `page_events` events.
pub(crate) fn read_event_types(
    ledger: &GlobalLedger,
    event_types: &[EventType],
    through: u64,
    page_events: usize,
) -> GlobalLedgerResult<Vec<PersistedEvent>> {
    let mut events = Vec::new();
    for event_type in event_types {
        events.extend(read_pages(
            ledger,
            &EventQuery {
                event_type: Some(*event_type),
                to_sequence: Some(through),
                ..EventQuery::default()
            },
            through,
            page_events,
        )?);
    }
    events.sort_by_key(PersistedEvent::sequence);
    Ok(events)
}
