// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #383 AO-2 (`contracts/ledger-observer.md` §2, §2.1): the calls a running Runtime
//! makes on the ledger. A gate hands one business event to the capture interface and goes on at
//! once; the ledger numbers it, queues it, and later assembles, sanitizes and commits it on its
//! flush thread. The flush barrier waits for one captured record's commit and writes nothing;
//! the tier handle reads the capture queue's watermark tier without a lock or I/O.
//!
//! These are interface shapes only; `crates/ledger` implements them. The host receives them at
//! start (AO-3 guards where they may appear).

use crate::outcome::Outcome;
use crate::{
    EventId, EventLinksDraft, EventOrigin, EventPayloadDraft, EventSeverity, IssuedEventId,
    SourceRevision,
};

/// One business event as a gate sees it: an existing payload draft and its links. Draft
/// assembly, sanitizing, category, ordering, watermarks, flushing and tagging all happen on the
/// ledger side.
///
/// As built by AO-2: the gate issues the record's event id (`event_id`), so the id exists before
/// anything is written and may be linked at once, and `severity` is the severity the draft
/// requests (only `debug` survives sanitizing; every other record takes its category).
pub struct CaptureItem {
    pub event_id: IssuedEventId,
    pub severity: EventSeverity,
    pub payload: EventPayloadDraft,
    pub links: EventLinksDraft,
    pub origin: EventOrigin,
    /// Set by a registered revisioned source (§8.3): the store that applied the change.
    pub source: Option<SourceRevision>,
}

/// Issued at capture, before anything is written: the event id may be linked at once;
/// `capture_seq` orders the record; there is no ledger sequence yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureTicket {
    pub event_id: EventId,
    pub capture_seq: u64,
}

/// A captured record once it is committed: its event id and its ledger sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommittedRecord {
    pub event_id: EventId,
    pub sequence: u64,
}

/// The capture queue's watermark tier (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CaptureTier {
    BelowTier1,
    Tier1,
    Tier2,
    Tier3,
}

/// The capture interface (§2 call 1). Never blocks, never fails, never drops (§7).
pub trait LedgerCapture: Send + Sync {
    /// The state-document half of a joint capture (G15); opaque to the host.
    type JointWork: Send + 'static;
    /// Queues one record in capture order and returns at once.
    fn capture(&self, item: CaptureItem) -> CaptureTicket;
    /// G15: the record and its state-document work commit in one transaction on the flush
    /// thread.
    fn capture_joint(&self, item: CaptureItem, work: Self::JointWork) -> CaptureTicket;
}

/// The flush barrier (§2 call 2, §8.4): waits until one captured record is committed; writes
/// nothing.
pub trait LedgerFlushBarrier: Send + Sync {
    /// Returns once the record is committed. A caller never receives success for an
    /// uncommitted record: a record the flush thread refused ends the wait with
    /// `ledger_record_rejected` (its gap record stands in its place), and a ledger that stopped
    /// fatally ends it with `ledger_write_failed`.
    fn wait_committed(&self, ticket: &CaptureTicket) -> Result<CommittedRecord, Outcome>;
}

/// The watermark tier read (§2 call 4): lock-free, no I/O.
pub trait LedgerBacklogTier: Send + Sync {
    fn tier(&self) -> CaptureTier;
}
