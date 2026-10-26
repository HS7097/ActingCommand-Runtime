// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A51 (R2): the work budget of one read on the writer thread. A read takes the writer's
//! clock when it starts and looks at it again after every `READ_BUDGET_CHECK_ROWS` rows it
//! visits; once its budget (`ledger_observer.read_budget_ms`) is spent it stops at the row it
//! reached. A page then answers incomplete from that row on (`LedgerPageLimit::ReadBudget`);
//! every other read answers `ledger_read_budget_exceeded`. The appends that waited meanwhile
//! commit before the writer's next command. A read without a budget (an offline reader, or a
//! writer opened without one) never stops.

use super::GlobalLedgerError;
use super::write_failure::LedgerMonotonicClock;
use crate::codes::LedgerCode;
use actingcommand_contract::outcome::Location;
use std::cell::Cell;
use std::time::Duration;

/// Rows a read visits between two looks at the clock.
pub(in crate::global) const READ_BUDGET_CHECK_ROWS: usize = 256;
/// #422 LN-WS (R1): the default of `ledger_observer.read_check_vm_steps`, the SQLite
/// virtual-machine steps between two looks at the clock of a read that serves a Runtime request.
pub(in crate::global) const DEFAULT_READ_CHECK_VM_STEPS: u32 = 10_000;

/// One read's budget on the writer's clock (milliseconds) and the rows it visited.
pub(in crate::global) struct ReadWork {
    clock: Option<LedgerMonotonicClock>,
    started_ms: u64,
    budget_ms: u64,
    visited: Cell<usize>,
    spent: Cell<bool>,
}

impl ReadWork {
    /// A read that never stops.
    pub(in crate::global) fn unbounded() -> Self {
        Self {
            clock: None,
            started_ms: 0,
            budget_ms: 0,
            visited: Cell::new(0),
            spent: Cell::new(false),
        }
    }

    /// A read the writer started at `started_ms` on `clock`, which stops once `budget` is
    /// spent; without a budget it never stops.
    pub(in crate::global) fn started(
        clock: &LedgerMonotonicClock,
        started_ms: u64,
        budget: Option<Duration>,
    ) -> Self {
        let Some(budget) = budget else {
            return Self::unbounded();
        };
        Self {
            clock: Some(clock.clone()),
            started_ms,
            budget_ms: u64::try_from(budget.as_millis()).unwrap_or(u64::MAX),
            visited: Cell::new(0),
            spent: Cell::new(false),
        }
    }

    /// Counts one visited row. After every `READ_BUDGET_CHECK_ROWS` rows it looks at the clock:
    /// `false` once the budget is spent, and from then on.
    pub(in crate::global) fn visit(&self) -> bool {
        if self.spent.get() {
            return false;
        }
        let visited = self.visited.get().saturating_add(1);
        self.visited.set(visited);
        let Some(clock) = self.clock.as_ref() else {
            return true;
        };
        if !visited.is_multiple_of(READ_BUDGET_CHECK_ROWS) {
            return true;
        }
        if clock().saturating_sub(self.started_ms) >= self.budget_ms {
            self.spent.set(true);
            return false;
        }
        true
    }

    /// The rows visited so far.
    pub(in crate::global) fn visited(&self) -> usize {
        self.visited.get()
    }

    /// #422 LN-WS, tests: the progress callbacks of the read's in-statement bound, and those at or
    /// after its budget point.
    #[cfg(any(test, feature = "test-hooks"))]
    pub(in crate::global) fn bound_callbacks(&self) -> (u64, u64) {
        (0, 0)
    }

    /// `ledger_read_budget_exceeded` of `operation`: a read that is no page stopped at its
    /// budget. Not fatal: the caller reads again at its next tick or request.
    pub(in crate::global) fn exceeded(operation: Location) -> GlobalLedgerError {
        GlobalLedgerError::request(LedgerCode::LedgerReadBudgetExceeded.code(), operation)
    }
}
