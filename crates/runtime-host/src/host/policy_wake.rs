// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #383 AO-7 (model-ledger-observer v1.4.1 §3.7): the host's business notifications
//! that wake the daemon's policy driver.
//!
//! Before AO-7 the daemon's policy monitor subscribed to the whole ledger over its own Runtime
//! connection and mapped each event it read to a driver trigger. The host now raises the same
//! triggers itself, right after it has committed the business change at today's append points
//! (model §14.1, AO-7 row), and the monitor takes them from memory. Nothing on the wake path
//! subscribes to or reads the ledger, so a slow or paused ledger flush never holds a wake back
//! (D2 A-2). AO-3 moves the raise to the fan-out point; this interface stays.
//!
//! The mapping is the one the monitor applied to ledger events, unchanged: a lease release,
//! transfer or expiry, and a performance or resource-authoring record, raise
//! `ResourcesChanged`; a catalog transition record raises `CatalogChanged`; an instance fact
//! publication or invalidation and an approval decision raise `FactsChanged`. Nothing else
//! wakes the driver. The append points that raise are the ones that can commit a record of a
//! waking type: every `append_event*` path (`facts.rs`), the fact invalidations of the store's
//! synchronization, approval decisions (`governance.rs`), catalog transitions
//! (`policy_catalog.rs`), the lease critical executions that end a lease (`lease.rs`, through
//! [`PolicyWakingLedger`]) and the capacity monitor's records (`performance/capacity.rs`).

use super::*;
use actingcommand_contract::{EventFamily, SanitizedEventDraft};
use actingcommand_ledger::GlobalLedgerResult;
use std::sync::PoisonError;

/// The policy driver's wakes the host has raised and its policy monitor has not taken yet,
/// each at most once. There is one consumer: the daemon's policy monitor.
#[derive(Default)]
pub(crate) struct PolicyWakeSignal {
    pending: Mutex<Vec<PolicyTrigger>>,
    raised: Condvar,
}

impl PolicyWakeSignal {
    /// Raises the wake `event_type` maps to, if any, for a record the host has just committed.
    pub(crate) fn raise(&self, event_type: EventType) {
        let Some(trigger) = policy_trigger_for_event(event_type) else {
            return;
        };
        let mut pending = self.pending();
        if !pending.contains(&trigger) {
            pending.push(trigger);
        }
        self.raised.notify_all();
    }

    fn take(&self, wait: Duration) -> Vec<PolicyTrigger> {
        let pending = self.pending();
        let (mut pending, _) = self
            .raised
            .wait_timeout_while(pending, wait, |pending| pending.is_empty())
            .unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut *pending)
    }

    fn clear(&self) {
        self.pending().clear();
    }

    /// The guarded sections only push to, take or clear the list and never panic, so a poisoned
    /// lock still holds a consistent list.
    fn pending(&self) -> MutexGuard<'_, Vec<PolicyTrigger>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Workflow #383 AO-7: the daemon policy monitor's side of the host's business notifications
/// ([`RuntimeHost::open_policy_wakes`]).
pub struct PolicyWakes {
    signal: Arc<PolicyWakeSignal>,
}

impl PolicyWakes {
    /// The wakes raised since the last take, each once; waits at most `wait` for the first.
    /// Empty when none was raised in time.
    pub fn take(&self, wait: Duration) -> Vec<PolicyTrigger> {
        self.signal.take(wait)
    }
}

impl RuntimeHost {
    /// Workflow #383 AO-7: the host's business notifications for the daemon's policy monitor.
    /// Opening them is the monitor's starting point: the wakes raised before are dropped, as
    /// the monitor's ledger subscription used to start at the ledger head, because the initial
    /// (`Recovery`) policy cycle has already evaluated what they report.
    pub fn open_policy_wakes(&self) -> RuntimeHostResult<PolicyWakes> {
        let shared = self.shared_ref(HostLocation::OpenPolicyWakes)?;
        shared.policy_wakes.clear();
        Ok(PolicyWakes {
            signal: Arc::clone(&shared.policy_wakes),
        })
    }

    /// Workflow #383 AO7-c, test builds and the `test-hooks` feature only: the event
    /// subscriptions (`SubscribeEvents`) this host has served.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn event_subscriptions_for_test(&self) -> u64 {
        self.shared
            .as_ref()
            .expect("test Runtime host is open")
            .event_subscriptions_for_test
            .load(Ordering::Acquire)
    }
}

/// Workflow #383 AO-7: the ledger as a critical execution's appender that raises the policy
/// driver's wake for each record it commits. The lease critical executions that end a lease
/// (an explicit release, a cleanup and an expiry) append through it.
pub(super) struct PolicyWakingLedger<'a> {
    shared: &'a HostShared,
}

impl EventAppender for PolicyWakingLedger<'_> {
    fn append_durable(&self, draft: SanitizedEventDraft) -> GlobalLedgerResult<PersistedEvent> {
        let event = self.shared.ledger.append_durable(draft)?;
        self.shared.policy_wakes.raise(event.event_type());
        Ok(event)
    }
}

impl HostShared {
    pub(super) fn policy_waking_ledger(&self) -> PolicyWakingLedger<'_> {
        PolicyWakingLedger { shared: self }
    }
}

/// The policy driver's wake for a committed record of type `event_type`, as the daemon's
/// monitor mapped the ledger's events before AO-7. Workflow #369 W-1: a lease end returns a
/// key, so the driver wakes on `lease.released`, `lease.transferred` and `lease.expired`, and
/// on no other lease, scheduler, capture or runtime record.
pub fn policy_trigger_for_event(event_type: EventType) -> Option<PolicyTrigger> {
    match event_type {
        EventType::LeaseReleased | EventType::LeaseTransferred | EventType::LeaseExpired => {
            Some(PolicyTrigger::ResourcesChanged)
        }
        other => policy_trigger_for_family(other.family()),
    }
}

/// The policy driver's wake for a committed record of `family`, a lease end aside
/// ([`policy_trigger_for_event`]).
pub fn policy_trigger_for_family(family: EventFamily) -> Option<PolicyTrigger> {
    match family {
        EventFamily::Catalog => Some(PolicyTrigger::CatalogChanged),
        EventFamily::Performance | EventFamily::ResourceAuthoring => {
            Some(PolicyTrigger::ResourcesChanged)
        }
        EventFamily::Fact | EventFamily::Approval => Some(PolicyTrigger::FactsChanged),
        EventFamily::Runtime
        | EventFamily::Provider
        | EventFamily::Monitor
        | EventFamily::Command
        | EventFamily::Scheduler
        | EventFamily::Policy
        | EventFamily::Lease
        | EventFamily::Task
        | EventFamily::Application
        | EventFamily::Input
        | EventFamily::Capture
        | EventFamily::Recognition
        | EventFamily::Artifact
        | EventFamily::Client
        | EventFamily::State
        | EventFamily::Release
        | EventFamily::Agent
        | EventFamily::Ledger => None,
    }
}
