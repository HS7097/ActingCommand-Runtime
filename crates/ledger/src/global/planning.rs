// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_runtime_database::{RuntimeDatabase, RuntimeTransaction};

const PLANNING_RECOVERY_PAGE_EVENTS: usize = 256;

/// A complete, bounded interval of original facts, anchored to one recovery high-water mark.
/// Fields are private so callers cannot omit facts or manufacture a different interval.
pub struct PlanningSignalRecoveryPage {
    after_sequence: u64,
    through_sequence: u64,
    upper: PersistedEvent,
    events: Vec<PersistedEvent>,
}

impl PlanningSignalRecoveryPage {
    pub fn after_sequence(&self) -> u64 {
        self.after_sequence
    }

    pub fn through_sequence(&self) -> u64 {
        self.through_sequence
    }

    pub fn planning_events(&self) -> impl Iterator<Item = &PersistedEvent> {
        self.events
            .iter()
            .filter(|event| event.event_type() == EventType::PolicyPlanningSignalObserved)
    }
}

impl GlobalLedger {
    /// Reads original verified facts, including non-planning gaps, before State takes its lock.
    pub fn planning_signal_recovery_page(
        &self,
        after_sequence: u64,
        upper_sequence: u64,
    ) -> GlobalLedgerResult<PlanningSignalRecoveryPage> {
        if after_sequence >= upper_sequence {
            return Err(planning_page_error("planning_recovery_interval_invalid"));
        }
        let mut upper =
            self.query_page(EventQuery::default(), upper_sequence - 1, upper_sequence, 1)?;
        if upper.len() != 1 || upper[0].sequence() != upper_sequence {
            return Err(planning_page_error("planning_recovery_upper_missing"));
        }
        let events = self.query_page(
            EventQuery::default(),
            after_sequence,
            upper_sequence,
            PLANNING_RECOVERY_PAGE_EVENTS,
        )?;
        let through_sequence = after_sequence
            .saturating_add(PLANNING_RECOVERY_PAGE_EVENTS as u64)
            .min(upper_sequence);
        let page = PlanningSignalRecoveryPage {
            after_sequence,
            through_sequence,
            upper: upper.remove(0),
            events,
        };
        page.validate_interval(after_sequence)?;
        Ok(page)
    }
}

/// Proves the entire page and its fixed upper anchor against the borrowed database transaction.
/// The State owner supplies its checked current checkpoint; this function never takes a lock,
/// contacts the writer, opens material, changes a row, or commits the caller's transaction.
pub fn verify_transaction_planning_page(
    database: &RuntimeDatabase,
    transaction: &RuntimeTransaction<'_, '_>,
    page: &PlanningSignalRecoveryPage,
    current_checkpoint: u64,
) -> GlobalLedgerResult<()> {
    page.validate_interval(current_checkpoint)?;
    verify_transaction_event(database, transaction, &page.upper)?;
    for event in &page.events {
        verify_transaction_event(database, transaction, event)?;
    }
    Ok(())
}

impl PlanningSignalRecoveryPage {
    fn validate_interval(&self, current_checkpoint: u64) -> GlobalLedgerResult<()> {
        if current_checkpoint != self.after_sequence
            || self.after_sequence >= self.through_sequence
            || self.through_sequence > self.upper.sequence()
            || self.events.len() > PLANNING_RECOVERY_PAGE_EVENTS
            || self.events.len() as u64 != self.through_sequence - self.after_sequence
        {
            return Err(planning_page_error("planning_recovery_interval_invalid"));
        }
        let mut expected = self.after_sequence;
        for event in &self.events {
            expected = expected
                .checked_add(1)
                .ok_or_else(|| planning_page_error("planning_recovery_sequence_overflow"))?;
            if event.sequence() != expected {
                return Err(planning_page_error("planning_recovery_interval_incomplete"));
            }
        }
        Ok(())
    }
}

fn planning_page_error(code: &'static str) -> GlobalLedgerError {
    GlobalLedgerError::fatal(code, "recover_planning_signal_page")
}
