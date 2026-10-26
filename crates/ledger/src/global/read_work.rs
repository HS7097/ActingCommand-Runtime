// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A51 (R2): the work budget of one read on the writer thread. A read takes the writer's
//! clock when it starts and looks at it again after every `READ_BUDGET_CHECK_ROWS` rows it
//! visits; once its budget (`ledger_observer.read_budget_ms`) is spent it stops at the row it
//! reached. A page then answers incomplete from that row on (`LedgerPageLimit::ReadBudget`);
//! every other read answers `ledger_read_budget_exceeded`. The appends that waited meanwhile
//! commit before the writer's next command. A read without a budget (an offline reader, or a
//! writer opened without one) never stops.
//!
//! #422 LN-WS (R1; the production stop of 2026-10-26 19:12:43): a read that serves a Runtime
//! request (a view page, a subscription's replay page) is bounded inside its SQLite statements
//! too. While it holds the shared connection, its statements run under a progress handler that
//! looks at the same clock every `ledger_observer.read_check_vm_steps` virtual-machine steps
//! ([`StatementBound`]); once the budget is spent the handler interrupts the statement in
//! progress, which ends only that read: a page answers incomplete, any other read
//! `ledger_read_budget_exceeded`. The read also looks at the clock before each further statement
//! of its own. A read ends within its budget and one check interval, also inside a step.

use super::GlobalLedgerError;
use super::write_failure::LedgerMonotonicClock;
use crate::codes::LedgerCode;
use actingcommand_contract::outcome::{IoKind, Location};
use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

/// Rows a read visits between two looks at the clock.
pub(in crate::global) const READ_BUDGET_CHECK_ROWS: usize = 256;
/// #422 LN-WS (R1): the default of `ledger_observer.read_check_vm_steps`, the SQLite
/// virtual-machine steps between two looks at the clock of a read that serves a Runtime request.
pub(in crate::global) const DEFAULT_READ_CHECK_VM_STEPS: u32 = 10_000;
/// #422 LN-WS (R1): the largest `ledger_observer.read_check_vm_steps`.
pub(in crate::global) const MAX_READ_CHECK_VM_STEPS: u32 = 1_000_000;

/// One read's budget on the writer's clock (milliseconds) and the rows it visited.
pub(in crate::global) struct ReadWork {
    clock: Option<LedgerMonotonicClock>,
    started_ms: u64,
    budget_ms: u64,
    visited: Cell<usize>,
    spent: Cell<bool>,
    /// #422 LN-WS (R1): the bound inside the statements of a read that serves a Runtime request.
    statements: Option<Arc<StatementBound>>,
}

/// #422 LN-WS (R1): what the progress handler of one read knows: the read's budget on the
/// writer's clock, how often it looks at the clock, and what it saw. The handler runs on the
/// writer's thread, inside the read's statements; it is shared with the connection the read
/// holds, so it is `Send` and its state atomic.
pub(in crate::global) struct StatementBound {
    clock: LedgerMonotonicClock,
    started_ms: u64,
    budget_ms: u64,
    check_vm_steps: u32,
    /// The budget is spent (the handler, a look between statements or a visited row saw it).
    spent: AtomicBool,
    /// The statement in progress goes on past the budget: a view page's first statement, of
    /// one sequence, which every page completes so its cursor is past its start.
    exempt: AtomicBool,
    /// The handler interrupted a statement of this read.
    interrupted: AtomicBool,
    /// Tests: the handler's callbacks, and those at or after the budget point.
    callbacks: AtomicU64,
    late_callbacks: AtomicU64,
}

impl StatementBound {
    /// The handler's period in SQLite virtual-machine steps.
    pub(in crate::global) fn check_vm_steps(&self) -> i32 {
        i32::try_from(self.check_vm_steps).unwrap_or(i32::MAX)
    }

    /// The progress handler: `true` interrupts the statement in progress. It looks at the clock
    /// until the budget is spent, then interrupts every statement except an exempt one.
    pub(in crate::global) fn progress(&self) -> bool {
        self.callbacks.fetch_add(1, Ordering::Relaxed);
        if !self.spent.load(Ordering::Relaxed) {
            if (self.clock)().saturating_sub(self.started_ms) < self.budget_ms {
                return false;
            }
            self.spent.store(true, Ordering::Relaxed);
        }
        self.late_callbacks.fetch_add(1, Ordering::Relaxed);
        if self.exempt.load(Ordering::Relaxed) {
            return false;
        }
        self.interrupted.store(true, Ordering::Relaxed);
        true
    }
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
            statements: None,
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
            statements: None,
        }
    }

    /// #422 LN-WS (R1): this read bounded inside its statements too, looking at the clock every
    /// `check_vm_steps` virtual-machine steps; a read without a budget stays unbounded.
    pub(in crate::global) fn within_statements(mut self, check_vm_steps: u32) -> Self {
        if let Some(clock) = &self.clock {
            self.statements = Some(Arc::new(StatementBound {
                clock: Arc::clone(clock),
                started_ms: self.started_ms,
                budget_ms: self.budget_ms,
                check_vm_steps,
                spent: AtomicBool::new(false),
                exempt: AtomicBool::new(false),
                interrupted: AtomicBool::new(false),
                callbacks: AtomicU64::new(0),
                late_callbacks: AtomicU64::new(0),
            }));
        }
        self
    }

    /// #422 LN-WS (R1): the bound the read's statements run under, when it has one.
    pub(in crate::global) fn statement_bound(&self) -> Option<&Arc<StatementBound>> {
        self.statements.as_ref()
    }

    fn is_spent(&self) -> bool {
        self.spent.get()
            || self
                .statements
                .as_ref()
                .is_some_and(|bound| bound.spent.load(Ordering::Relaxed))
    }

    fn mark_spent(&self) {
        self.spent.set(true);
        if let Some(bound) = &self.statements {
            bound.spent.store(true, Ordering::Relaxed);
        }
    }

    /// Counts one visited row. After every `READ_BUDGET_CHECK_ROWS` rows it looks at the clock:
    /// `false` once the budget is spent, and from then on (LN-WS: also once its statement bound
    /// saw it spent).
    pub(in crate::global) fn visit(&self) -> bool {
        if self.is_spent() {
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
            self.mark_spent();
            return false;
        }
        true
    }

    /// #422 LN-WS (R1): whether the budget is spent, looking at the clock (before a further
    /// statement of a request's read); a read without a budget never spends it.
    pub(in crate::global) fn spent_now(&self) -> bool {
        if self.is_spent() {
            return true;
        }
        let Some(clock) = self.clock.as_ref() else {
            return false;
        };
        if clock().saturating_sub(self.started_ms) >= self.budget_ms {
            self.mark_spent();
            return true;
        }
        false
    }

    /// #422 LN-WS (R1): while `exempt`, the statement bound lets the statement in progress run
    /// past the budget.
    pub(in crate::global) fn exempt(&self, exempt: bool) {
        if let Some(bound) = &self.statements {
            bound.exempt.store(exempt, Ordering::Relaxed);
        }
    }

    /// #422 LN-WS (R1): whether `error` is a statement of this read that its bound interrupted
    /// (SQLite's `SQLITE_INTERRUPT` after the handler returned `true`). Any other interrupt stays
    /// the SQL failure it is.
    pub(in crate::global) fn interrupted(&self, error: &GlobalLedgerError) -> bool {
        error.code == LedgerCode::LedgerSqliteFailed.as_str()
            && error.write_failure == Some(IoKind::Interrupted)
            && self
                .statements
                .as_ref()
                .is_some_and(|bound| bound.interrupted.load(Ordering::Relaxed))
    }

    /// The rows visited so far.
    pub(in crate::global) fn visited(&self) -> usize {
        self.visited.get()
    }

    /// #422 LN-WS, tests: the progress callbacks of the read's in-statement bound, and those at or
    /// after its budget point.
    #[cfg(any(test, feature = "test-hooks"))]
    pub(in crate::global) fn bound_callbacks(&self) -> (u64, u64) {
        self.statements.as_ref().map_or((0, 0), |bound| {
            (
                bound.callbacks.load(Ordering::Relaxed),
                bound.late_callbacks.load(Ordering::Relaxed),
            )
        })
    }

    /// `ledger_read_budget_exceeded` of `operation`: a read that is no page stopped at its
    /// budget. Not fatal: the caller reads again at its next tick or request.
    pub(in crate::global) fn exceeded(operation: Location) -> GlobalLedgerError {
        GlobalLedgerError::request(LedgerCode::LedgerReadBudgetExceeded.code(), operation)
    }
}
