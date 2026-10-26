// SPDX-License-Identifier: AGPL-3.0-only

//! #422 LN-7c O (`rd-1026/ln7c/final-query.md` §2, §7 row O): the open reads the owner records
//! of the two latest owner epochs, and of each older owner a start may still close, instead of
//! every owner record of the history.
//!
//! A start writes the first records of its owner epoch before its start record: its capacity
//! sample, its prior-epoch imports and closes and its startup recovery come first, then
//! `runtime.started` or `runtime.takeover`. So the read takes the owner records after the start
//! before the latest one (the type index, `sequence > ?`), then reads back, a chunk at a time
//! and each chunk a forward read of the same index, to the first record of the owner that window
//! begins with. The read is then a suffix of the history that begins where an owner took over,
//! with the owner record right before it (the owner that ended there). When a start still closes
//! the scopes of that ended owner, or it is the owner before the latest one, it is read back the
//! same way, from its last record to its first, and so on. Reading an owner back never passes the
//! second start before the place it starts from: an owner's records begin only once its own
//! start opened the ledger.
//!
//! Applied in ledger order, the suffix gives each owner whose records all lie in it the state the
//! whole history gives it: its epochs, end and reopening, its import, the closes of its scopes and
//! their writer, its quiescences, and through its epochs its scope sources and releases. Each
//! retained owner (`retained_owners`) is such an owner when its first block in the suffix holds a
//! start record: no record of it precedes that start's own records. A retained owner the read
//! cannot place so (an owner in no start's epoch, as a start that stopped before it wrote its
//! start record leaves) makes the open read every owner record, as before, and say so
//! (`LedgerOpenTiming::owner_read_full`, which actingd prints as one stdout line).
//!
//! A ledger dump an earlier run left (at the dump path or the fallback path) makes the open read
//! every owner record too, and say so: the open imports the dump after this read, and the owner
//! the dump names may precede the bounded read (review F2). Dumps follow only a failed write, so
//! this costs one full read on those starts.

use super::open::OWNER_TYPES;
use super::*;
use crate::global::history::HistorySelection;
use crate::global::storage::{DurableStorage, EventStore};
use actingcommand_contract::EventQuery;
use std::time::Instant;

/// The owner records the open reads, in ledger order, and why they are every owner record of the
/// history: the bounded read could not place an owner (a retained owner whose first block holds
/// no start record, an owner block that reaches past the start before its own, or no owner record
/// after the start before the latest one), or a dump was present.
pub(super) struct OwnerRead {
    pub(super) records: Vec<PersistedEvent>,
    pub(super) full: Option<OwnerReadFull>,
}

/// One owner's block read back from a sequence: its owner records before that sequence, in
/// ledger order, and the owner record right before them (`None` at the history's start).
struct Block {
    records: Vec<PersistedEvent>,
    before: Option<PersistedEvent>,
}

impl RetentionIndex {
    /// #422 LN-7c O: why the open read every owner record (it could not place a retained owner
    /// in a start's epoch, or a dump was present), `None` when it did not.
    pub(in crate::global) fn owner_read_full(&self) -> Option<OwnerReadFull> {
        self.owner_read_full
    }

    /// The owner changes, ends, imports, closes and writers of `records` alone, in ledger order.
    fn owners_of<'a>(records: impl IntoIterator<Item = &'a PersistedEvent>) -> Self {
        let mut index = Self::default();
        for event in records {
            index.apply_selected(event);
        }
        index
    }

    /// Whether the first block of `owner` (from its first change to the next change) holds one
    /// of `starts`.
    fn first_block_holds_start(&self, owner: OwnerEpoch, starts: &[u64]) -> bool {
        let mut changes = self.owner_epochs.iter();
        let Some((first, _)) = changes.find(|(_, epoch)| **epoch == owner) else {
            return false;
        };
        let end = changes.next().map_or(u64::MAX, |(next, _)| *next);
        starts.iter().any(|start| first < start && *start < end)
    }
}

impl<B: DurableStorage> EventStore<B> {
    /// #422 LN-7c O: the owner records of the open (the module comment): those of the two latest
    /// owner epochs and of each older owner a start may still close, or, when a retained owner is
    /// in no start's epoch or `dump_present` (a dump this open imports), every owner record.
    /// `starts` are the sequences of the starts (`runtime.started`, `runtime.takeover`) in ledger
    /// order. Every read stops at `deadline`.
    pub(super) fn owner_records(
        &self,
        starts: &[u64],
        dump_present: bool,
        deadline: Option<Instant>,
    ) -> GlobalLedgerResult<OwnerRead> {
        let head = self.backend.head();
        #[cfg(test)]
        if FULL_OWNER_READ.with(std::cell::Cell::get) {
            return Ok(OwnerRead {
                records: self.owner_range(1, head, deadline)?,
                full: None,
            });
        }
        // Without a start before the latest one, the window is the whole history.
        let Some(after) = starts.len().checked_sub(2).map(|index| starts[index]) else {
            return Ok(OwnerRead {
                records: self.owner_range(1, head, deadline)?,
                full: None,
            });
        };
        // Review F2: the dump's owner may precede the bounded read.
        if dump_present {
            return Ok(OwnerRead {
                records: self.owner_range(1, head, deadline)?,
                full: Some(OwnerReadFull::DumpPresent),
            });
        }
        let mut from = after.saturating_add(1);
        let mut records = self.owner_range(from, head, deadline)?;
        let mut owner = records.first().and_then(recorded_owner);
        while let Some(current) = owner {
            let Some(block) = self.owner_block(current, from, starts, deadline)? else {
                break;
            };
            from = block.records.first().map_or(from, PersistedEvent::sequence);
            let mut suffix = block.records;
            suffix.append(&mut records);
            records = suffix;
            let index = RetentionIndex::owners_of(block.before.iter().chain(&records));
            let retained = index.retained_owners();
            let ended = block.before.as_ref().and_then(recorded_owner);
            if let Some(ended) = ended.filter(|ended| retained.contains(ended)) {
                // A start may still close the scopes of the owner the suffix's first one ended,
                // or it is the owner before the latest: its block is read back too.
                from = block
                    .before
                    .as_ref()
                    .map_or(from, |before| before.sequence().saturating_add(1));
                owner = Some(ended);
                continue;
            }
            if retained
                .iter()
                .all(|kept| index.first_block_holds_start(*kept, starts))
            {
                if let Some(before) = block.before {
                    records.insert(0, before);
                }
                return Ok(OwnerRead {
                    records,
                    full: None,
                });
            }
            owner = None;
        }
        Ok(OwnerRead {
            records: self.owner_range(1, head, deadline)?,
            full: Some(OwnerReadFull::OwnerInNoStartEpoch),
        })
    }

    /// The block of `owner` that ends right before `from`, read back a chunk at a time, each a
    /// forward read of the type index: the first chunk the `ledger_observer.history_page_rows`
    /// sequences before `from`, each next chunk twice as many. The block ends at the first owner
    /// record of another owner; it reads back no further than the second start before `from` (the
    /// start before the owner's own one), and reaching it is `None`: the owner is in no start's
    /// epoch. Without a second start before `from` it may reach the history's start.
    fn owner_block(
        &self,
        owner: OwnerEpoch,
        from: u64,
        starts: &[u64],
        deadline: Option<Instant>,
    ) -> GlobalLedgerResult<Option<Block>> {
        let under = starts.partition_point(|start| *start < from);
        let floor = under.checked_sub(2).map_or(1, |index| starts[index]);
        let mut span = u64::try_from(self.history_page_rows)
            .unwrap_or(u64::MAX)
            .max(1);
        let mut chunks = Vec::new();
        let mut through = from;
        while through > floor {
            let lo = through.saturating_sub(span).max(floor);
            let mut chunk = self.owner_range(lo, through - 1, deadline)?;
            if let Some(position) = chunk
                .iter()
                .rposition(|event| recorded_owner(event) != Some(owner))
            {
                chunks.push(chunk.split_off(position + 1));
                return Ok(Some(Block {
                    records: chunks.into_iter().rev().flatten().collect(),
                    before: chunk.pop(),
                }));
            }
            chunks.push(chunk);
            through = lo;
            span = span.saturating_mul(2);
        }
        Ok((floor == 1).then(|| Block {
            records: chunks.into_iter().rev().flatten().collect(),
            before: None,
        }))
    }

    /// The owner records (`OWNER_TYPES` records that name an owner) from `from` through
    /// `through`, in ledger order, read by the type index.
    fn owner_range(
        &self,
        from: u64,
        through: u64,
        deadline: Option<Instant>,
    ) -> GlobalLedgerResult<Vec<PersistedEvent>> {
        let query = EventQuery {
            event_types: OWNER_TYPES.to_vec(),
            from_sequence: Some(from),
            to_sequence: Some(through),
            ..EventQuery::default()
        };
        self.history_until(deadline)
            .select_where(&HistorySelection::of(&query), |event| {
                recorded_owner(event).is_some()
            })
    }
}

#[cfg(test)]
thread_local! {
    /// Tests: the opens on this thread read every owner record, as before LN-7c O.
    static FULL_OWNER_READ: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Tests: the owner state of the latest open on this thread, and whether it read every owner
    /// record.
    static OPENED: std::cell::RefCell<Option<(OwnerState, bool)>> =
        const { std::cell::RefCell::new(None) };
}

/// Tests: whether the opens on this thread read every owner record (today's read before
/// LN-7c O), the reference the bounded read is compared with.
#[cfg(test)]
pub(in crate::global) fn read_every_owner_record_for_test(every: bool) {
    FULL_OWNER_READ.with(|cell| cell.set(every));
}

/// Tests: the owner state of the latest open on this thread, and whether it read every owner
/// record.
#[cfg(test)]
pub(in crate::global) fn take_opened_owner_state_for_test() -> Option<(OwnerState, bool)> {
    OPENED.with(|cell| cell.borrow_mut().take())
}

/// Tests: what an open keeps of the owners a start may still close.
#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
pub(in crate::global) struct OwnerState {
    /// `retained_owners`.
    pub(in crate::global) retained: BTreeSet<OwnerEpoch>,
    /// The owners whose scopes a writer that has written nothing yet closes
    /// (`prior_epoch_owners`).
    pub(in crate::global) next_closes: BTreeSet<OwnerEpoch>,
    pub(in crate::global) owners: BTreeMap<OwnerEpoch, OwnerFacts>,
}

/// Tests: what an open keeps of one retained owner.
#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
pub(in crate::global) struct OwnerFacts {
    /// Its epochs: the sequence ranges in which it is the recorded owner.
    pub(in crate::global) epochs: Vec<(u64, u64)>,
    pub(in crate::global) end: Option<u64>,
    pub(in crate::global) reopened: bool,
    pub(in crate::global) import:
        Option<(TerminalEvent, actingcommand_contract::PriorEpochOwnerImport)>,
    pub(in crate::global) closes: BTreeMap<
        actingcommand_contract::PriorEpochScope,
        (TerminalEvent, actingcommand_contract::PriorEpochScopeClose),
    >,
    pub(in crate::global) writer: Option<OwnerEpoch>,
    pub(in crate::global) quiescences: BTreeMap<InstanceId, TerminalEvent>,
    pub(in crate::global) scope_sources:
        BTreeMap<actingcommand_contract::PriorEpochScope, (TerminalEvent, u64)>,
    pub(in crate::global) released_leases: BTreeMap<(InstanceId, LeaseId), TerminalEvent>,
}

#[cfg(test)]
impl RetentionIndex {
    /// Tests: keeps this open's owner state for `take_opened_owner_state_for_test`.
    pub(super) fn keep_owner_state_for_test(&self, head: u64) {
        let state = self.owner_state(head);
        OPENED.with(|cell| *cell.borrow_mut() = Some((state, self.owner_read_full.is_some())));
    }

    fn owner_state(&self, head: u64) -> OwnerState {
        let subject = |scope: &ClosureScope| actingcommand_contract::PriorEpochScope {
            owner: scope.owner,
            instance: scope.instance,
            run: scope.run,
            lease: scope.lease,
            request: scope.request,
            correlation: scope.correlation,
        };
        let retained = self.retained_owners();
        let owners = retained
            .iter()
            .map(|owner| {
                let facts = OwnerFacts {
                    epochs: self.owner_ranges(*owner, head),
                    end: self.owner_ends.get(owner).copied(),
                    reopened: self.reopened_owners.contains(owner),
                    import: self.owner_imports.get(owner).cloned(),
                    closes: self
                        .prior_epoch_closes
                        .iter()
                        .filter(|(scope, _)| scope.owner == *owner)
                        .map(|(scope, close)| (subject(scope), close.clone()))
                        .collect(),
                    writer: self.prior_epoch_writers.get(owner).copied(),
                    quiescences: self
                        .quiescent_instances
                        .iter()
                        .filter(|((epoch, _), _)| epoch == owner)
                        .map(|((_, instance), close)| (*instance, *close))
                        .collect(),
                    scope_sources: self
                        .scope_sources
                        .iter()
                        .filter(|(scope, _)| scope.owner == *owner)
                        .map(|(scope, source)| (subject(scope), *source))
                        .collect(),
                    released_leases: self
                        .released_leases
                        .iter()
                        .filter(|((epoch, _, _), _)| epoch == owner)
                        .map(|((_, instance, lease), release)| ((*instance, *lease), *release))
                        .collect(),
                };
                (*owner, facts)
            })
            .collect();
        let writer = *actingcommand_contract::IdentifierIssuer::new()
            .expect("identifier issuer")
            .mint_owner_epoch()
            .expect("a writer epoch with no record")
            .transport();
        OwnerState {
            retained,
            next_closes: self.prior_epoch_owners(writer),
            owners,
        }
    }
}
