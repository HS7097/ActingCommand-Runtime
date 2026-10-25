// SPDX-License-Identifier: AGPL-3.0-only

//! Bounded startup reads (Workflow #381 R4a). Every writer command waits at most the Ledger's
//! reply bound, so a startup read is split into pages pinned to one ledger position: no single
//! command grows with the whole Ledger, and the events read are the ones one query at that
//! position would have returned, in the same order.

use actingcommand_contract::{EventQuery, EventType};
use actingcommand_ledger::{
    GlobalLedger, GlobalLedgerError, GlobalLedgerResult, LedgerCode, PersistedEvent,
};

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
    read_pages_after(ledger, query, 0, through, page_events)
}

/// Workflow #422 LN-4: [`read_pages`] of the sequences after `after` through `through`.
pub(crate) fn read_pages_after(
    ledger: &GlobalLedger,
    query: &EventQuery,
    after: u64,
    through: u64,
    page_events: usize,
) -> GlobalLedgerResult<Vec<PersistedEvent>> {
    let mut events = Vec::new();
    let mut after = after;
    if after >= through {
        return Ok(events);
    }
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

/// Workflow #422 LN-4: [`read_event_types`] of the sequences after `after` through `through`.
pub(crate) fn read_event_types_after(
    ledger: &GlobalLedger,
    event_types: &[EventType],
    after: u64,
    through: u64,
    page_events: usize,
) -> GlobalLedgerResult<Vec<PersistedEvent>> {
    let mut events = Vec::new();
    for event_type in event_types {
        events.extend(read_pages_after(
            ledger,
            &EventQuery {
                event_type: Some(*event_type),
                to_sequence: Some(through),
                ..EventQuery::default()
            },
            after,
            through,
            page_events,
        )?);
    }
    events.sort_by_key(PersistedEvent::sequence);
    Ok(events)
}

/// #411 A65 (R1): the events `query` selects with a sequence after `after` through `through`, in
/// ledger order, one page of at most `page_events` events at a time. A page that the writer's
/// read budget stopped (`ledger_read_budget_exceeded`) is read again with half as many events,
/// down to one event, and the read goes on after the last event that page returned; every
/// other failure ends the read.
pub(crate) struct LedgerPages<'a> {
    ledger: &'a GlobalLedger,
    query: EventQuery,
    after: u64,
    through: u64,
    page_events: usize,
    exhausted: bool,
}

impl<'a> LedgerPages<'a> {
    pub(crate) fn new(
        ledger: &'a GlobalLedger,
        query: EventQuery,
        after: u64,
        through: u64,
        page_events: usize,
    ) -> Self {
        Self {
            ledger,
            query,
            after,
            through,
            page_events: page_events.max(1),
            exhausted: after >= through,
        }
    }

    /// The next page; `None` once the range is read.
    pub(crate) fn next_page(&mut self) -> GlobalLedgerResult<Option<Vec<PersistedEvent>>> {
        if self.exhausted {
            return Ok(None);
        }
        let mut page_events = self.page_events;
        let page = loop {
            match self
                .ledger
                .query_page(self.query.clone(), self.after, self.through, page_events)
            {
                Ok(page) => break page,
                Err(error) if page_events > 1 && stopped_by_read_budget(&error) => {
                    page_events /= 2;
                }
                Err(error) => return Err(error),
            }
        };
        if let Some(last) = page.last() {
            self.after = last.sequence();
        }
        self.exhausted = page.len() < page_events || self.after >= self.through;
        Ok(Some(page))
    }

    /// Every remaining page, in ledger order.
    pub(crate) fn read_all(mut self) -> GlobalLedgerResult<Vec<PersistedEvent>> {
        let mut events = Vec::new();
        while let Some(page) = self.next_page()? {
            events.extend(page);
        }
        Ok(events)
    }
}

/// A read the writer's read budget stopped: not fatal, read again (#411 A51 R2).
fn stopped_by_read_budget(error: &GlobalLedgerError) -> bool {
    !error.is_fatal() && error.code() == LedgerCode::LedgerReadBudgetExceeded.as_str()
}
