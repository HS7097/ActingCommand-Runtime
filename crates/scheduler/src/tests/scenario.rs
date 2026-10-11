// SPDX-License-Identifier: AGPL-3.0-only

//! POL-I1, the scheduler `Scenario` harness (test plan P-1, `test-ci-plan.md:162`;
//! `inventory-policy.md:119`). Used by the P-1 gate tests G1, G2, G4 and the R3 rows.
//!
//! Every step goes through the public `SeedScheduler` API at an explicit monotonic millisecond
//! that only the harness advances: no clock, no sleep, no thread. After every step the
//! invariant checker reads `active_lease`, `queued_on`, `queued_count`, `active_tokens` and
//! `tokens_for_connection`, and nothing else, and requires:
//! 1. at most one active token per instance, and it is the instance's lease (key 2; c3a:65);
//! 2. `queued_count` equals the length of `queued_on` for every instance: one queue, no second
//!    state (Alice 10-07);
//! 3. `queued_on` lists the holder-owned continuation first, then High before Normal, then
//!    arrival (model-369-queue.md Q-3 (0), (2), (3)), against the arrival order the harness
//!    recorded itself;
//! 4. every token a connection owns is the active token of its instance (c3a:66).

use super::{claim, claim_config, epoch, holder, ids, instance, queued_request, request};
use crate::{
    ClaimGate, ClaimKind, ClaimRequest, ConnectionId, LeaseReleaseReason, LeaseTransferReason,
    QueueAdmissionDecision, SchedulerResult, SeedScheduler, TransferPreparation,
};
use actingcommand_contract::{
    HolderId, IdentifierIssuer, InstanceId, LeasePriority, LeaseToken, RequestId,
};
use std::cmp::Reverse;
use std::collections::BTreeMap;

/// One holder's current token and the connection that owns it (the one current-token cell of
/// model-369-queue.md Q-5), with the request the key was granted to.
#[derive(Debug, Clone)]
pub(super) struct Hold {
    pub(super) token: LeaseToken,
    pub(super) connection_id: ConnectionId,
    pub(super) request_id: RequestId,
}

/// A client: one request under one holder on one connection.
#[derive(Debug, Clone, Copy)]
pub(super) struct Client {
    pub(super) request_id: RequestId,
    holder_id: HolderId,
    connection_id: ConnectionId,
}

/// The answer to a client `QueueLease`: an immediate grant, committed, or a queued entry.
pub(super) enum ClientQueue {
    Granted(Hold),
    Waiting,
}

/// A lease end that goes through the pump (model-369-queue.md Q-4). `HostShutdown`, the one end
/// that does not hand the key on, is not a step of this harness.
#[derive(Debug, Clone, Copy)]
pub(super) enum LeaseEnd {
    /// The holder releases (`ExplicitRelease`).
    Explicit,
    /// The run failed (`BackendFailure`).
    BackendFailure,
}

pub(super) struct Scenario {
    issuer: IdentifierIssuer,
    scheduler: SeedScheduler,
    monotonic_ms: u64,
    instances: Vec<InstanceId>,
    connections: Vec<ConnectionId>,
    next_connection: u64,
    arrivals: BTreeMap<RequestId, u64>,
}

impl Scenario {
    /// A scheduler with no takeover cooldown, at monotonic 0.
    pub(super) fn new() -> Self {
        Self::start(ids(), Vec::new())
    }

    /// A scheduler that has just taken over one instance: its takeover cooldown runs from 0.
    pub(super) fn after_takeover() -> (Self, InstanceId) {
        let issuer = ids();
        let taken_over = instance(&issuer);
        (Self::start(issuer, vec![taken_over]), taken_over)
    }

    fn start(issuer: IdentifierIssuer, taken_over: Vec<InstanceId>) -> Self {
        let scheduler = SeedScheduler::new(
            epoch(&issuer),
            claim_config(),
            taken_over.iter().copied(),
            0,
        )
        .expect("scheduler");
        let scenario = Self {
            issuer,
            scheduler,
            monotonic_ms: 0,
            instances: taken_over,
            connections: Vec::new(),
            next_connection: 1,
            arrivals: BTreeMap::new(),
        };
        scenario.check();
        scenario
    }

    pub(super) fn scheduler(&self) -> &SeedScheduler {
        &self.scheduler
    }

    pub(super) fn monotonic_ms(&self) -> u64 {
        self.monotonic_ms
    }

    pub(super) fn new_instance(&mut self) -> InstanceId {
        let instance_id = instance(&self.issuer);
        self.instances.push(instance_id);
        instance_id
    }

    fn new_connection(&mut self) -> ConnectionId {
        let connection_id = ConnectionId::new(self.next_connection).expect("connection");
        self.next_connection += 1;
        self.connections.push(connection_id);
        connection_id
    }

    pub(super) fn client(&mut self) -> Client {
        Client {
            request_id: request(&self.issuer),
            holder_id: holder(&self.issuer).1,
            connection_id: self.new_connection(),
        }
    }

    /// A Runtime-internal claim with its own holder and connection (TTL 900 ms, `claim`).
    pub(super) fn claim(
        &mut self,
        instance_id: InstanceId,
        kind: ClaimKind,
        priority: LeasePriority,
    ) -> ClaimRequest {
        let holder_id = holder(&self.issuer).1;
        let connection_id = self.new_connection();
        claim(
            &self.issuer,
            instance_id,
            holder_id,
            connection_id,
            kind,
            priority,
        )
    }

    /// The holder's own continuation, queued before a rung run (model-369-queue.md H-3; high,
    /// model §6 item 3).
    pub(super) fn continuation(&mut self, hold: &Hold) -> ClaimRequest {
        claim(
            &self.issuer,
            hold.token.instance_id(),
            hold.token.holder_id(),
            hold.connection_id,
            ClaimKind::LadderContinuation,
            LeasePriority::High,
        )
    }

    /// Advances the monotonic clock.
    pub(super) fn tick(&mut self, elapsed_ms: u64) {
        self.monotonic_ms += elapsed_ms;
        self.check();
    }

    /// A client `AcquireLease`: an immediate try with commit.
    pub(super) fn acquire(
        &mut self,
        client: Client,
        instance_id: InstanceId,
    ) -> SchedulerResult<Hold> {
        let held = self
            .scheduler
            .acquire(
                client.request_id,
                instance_id,
                client.holder_id,
                client.connection_id,
                self.monotonic_ms,
            )
            .map(|token| Hold {
                token,
                connection_id: client.connection_id,
                request_id: client.request_id,
            });
        self.check();
        held
    }

    /// A client `QueueLease`; an immediate grant is committed at once.
    pub(super) fn queue_lease(
        &mut self,
        client: Client,
        instance_id: InstanceId,
        priority: LeasePriority,
        timeout_ms: u64,
    ) -> ClientQueue {
        let now = self.monotonic_ms;
        let decision = self
            .scheduler
            .request_queued(
                queued_request(
                    client.request_id,
                    instance_id,
                    client.holder_id,
                    client.connection_id,
                    priority,
                    timeout_ms,
                ),
                now,
            )
            .expect("a client QueueLease is admitted")
            .into_decision();
        let answer = match decision {
            QueueAdmissionDecision::Lease(preparation) => ClientQueue::Granted(Hold {
                token: self
                    .scheduler
                    .commit_acquire(preparation, now)
                    .expect("commit the immediate grant"),
                connection_id: client.connection_id,
                request_id: client.request_id,
            }),
            QueueAdmissionDecision::Queued(_) => {
                self.arrived(client.request_id);
                ClientQueue::Waiting
            }
        };
        self.check();
        answer
    }

    /// An immediate try of `claim` with commit (`prepare_claim_acquire` + `commit_acquire`).
    pub(super) fn try_claim(
        &mut self,
        claim: ClaimRequest,
        gate: ClaimGate,
    ) -> SchedulerResult<Hold> {
        let now = self.monotonic_ms;
        let held = self
            .scheduler
            .prepare_claim_acquire(claim, gate, now)
            .and_then(|preparation| self.scheduler.commit_acquire(preparation, now))
            .map(|token| Hold {
                token,
                connection_id: claim.connection_id,
                request_id: claim.request_id,
            });
        self.check();
        held
    }

    /// A probe: would a policy dispatch get the key on `instance_id` now? It prepares a
    /// `PolicyDispatch` claim and never commits it.
    pub(super) fn probe(
        &mut self,
        instance_id: InstanceId,
        gate: ClaimGate,
    ) -> SchedulerResult<()> {
        let claim = self.claim(
            instance_id,
            ClaimKind::PolicyDispatch,
            LeasePriority::Normal,
        );
        let answer = self
            .scheduler
            .prepare_claim_acquire(claim, gate, self.monotonic_ms)
            .map(drop);
        self.check();
        answer
    }

    /// Queues a Runtime-internal claim (`enqueue_claim`).
    pub(super) fn enqueue(&mut self, claim: ClaimRequest) -> SchedulerResult<()> {
        let queued = self
            .scheduler
            .enqueue_claim(claim, self.monotonic_ms)
            .map(drop);
        if queued.is_ok() {
            self.arrived(claim.request_id);
        }
        self.check();
        queued
    }

    /// The trigger's lease end handed to `claim` (model-369-queue.md H-1), as the host does it
    /// (runtime-host `lease.rs:2029-2043`, reason `BackendFailure`). `None`: no hand-off took
    /// place and the hold is unchanged.
    pub(super) fn hand_off(
        &mut self,
        hold: &Hold,
        claim: ClaimRequest,
        gate: ClaimGate,
    ) -> Option<Hold> {
        let now = self.monotonic_ms;
        let prepared = self
            .scheduler
            .prepare_hand_off(
                &hold.token,
                hold.connection_id,
                LeaseTransferReason::BackendFailure,
                claim,
                gate,
                now,
            )
            .expect("prepare the hand-off");
        let handed = match prepared {
            TransferPreparation::Ready(prepared) => Some(Hold {
                token: self
                    .scheduler
                    .commit_transfer(prepared, now)
                    .expect("commit the hand-off"),
                connection_id: claim.connection_id,
                request_id: claim.request_id,
            }),
            TransferPreparation::NoCandidate | TransferPreparation::Deferred => None,
        };
        self.check();
        handed
    }

    /// A lease end through the pump (model-369-queue.md Q-4): a transfer to the first eligible
    /// entry when one waits (the receiver's hold is returned), otherwise a release (`None`).
    pub(super) fn end_lease(
        &mut self,
        hold: &Hold,
        end: LeaseEnd,
        gate: ClaimGate,
    ) -> Option<Hold> {
        let now = self.monotonic_ms;
        let (reason, release_request) = match end {
            LeaseEnd::Explicit => (
                LeaseTransferReason::ExplicitRelease,
                Some(request(&self.issuer)),
            ),
            LeaseEnd::BackendFailure => (LeaseTransferReason::BackendFailure, None),
        };
        let prepared = self
            .scheduler
            .prepare_transfer_gated(
                &hold.token,
                hold.connection_id,
                reason,
                release_request,
                gate,
                now,
            )
            .expect("prepare the lease end");
        let next = match prepared {
            TransferPreparation::Ready(prepared) => {
                let connection_id = prepared.to_connection_id();
                let request_id = prepared.queued_request_id();
                let token = self
                    .scheduler
                    .commit_transfer(prepared, now)
                    .expect("commit the lease end's transfer");
                Some(Hold {
                    token,
                    connection_id,
                    request_id,
                })
            }
            TransferPreparation::NoCandidate => {
                let released = match release_request {
                    Some(release_request) => self.scheduler.release(
                        release_request,
                        &hold.token,
                        hold.connection_id,
                        now,
                    ),
                    None => self.scheduler.release_owned(
                        &hold.token,
                        hold.connection_id,
                        LeaseReleaseReason::BackendFailure,
                    ),
                };
                released.expect("release with nothing waiting");
                None
            }
            TransferPreparation::Deferred => panic!("no destructive step is open in a scenario"),
        };
        self.check();
        next
    }

    /// The moment between a lease end and its pump: a dedicated pause close lease's
    /// `InstancePaused` release, which names no transfer, leaves the waiting entries standing
    /// (as `tests.rs` `an_immediate_try_never_jumps_an_eligible_entry` builds it).
    pub(super) fn release_before_pump(&mut self, hold: &Hold) {
        self.scheduler
            .release_owned(
                &hold.token,
                hold.connection_id,
                LeaseReleaseReason::InstancePaused,
            )
            .expect("release without a transfer");
        self.check();
    }

    /// Renews a Runtime-held lease (`renew_with_ttl`); the hold keeps the renewed token.
    pub(super) fn renew_with_ttl(
        &mut self,
        hold: &mut Hold,
        lease_ttl_ms: u64,
    ) -> SchedulerResult<()> {
        let renewed = self.scheduler.renew_with_ttl(
            &hold.token,
            hold.connection_id,
            lease_ttl_ms,
            self.monotonic_ms,
        );
        if let Ok(token) = &renewed {
            hold.token = token.clone();
        }
        self.check();
        renewed.map(drop)
    }

    /// An install drain of `instance_id` (`remove_queued_for_drain`): the cancelled requests.
    pub(super) fn drain(&mut self, instance_id: InstanceId) -> Vec<RequestId> {
        let cancelled = self
            .scheduler
            .remove_queued_for_drain(instance_id)
            .expect("install drain")
            .iter()
            .map(|cancelled| cancelled.queued().request_id())
            .collect();
        self.check();
        cancelled
    }

    fn arrived(&mut self, request_id: RequestId) {
        let arrival = u64::try_from(self.arrivals.len()).expect("arrival count");
        self.arrivals.insert(request_id, arrival);
    }

    fn check(&self) {
        let active = self.scheduler.active_tokens();
        let mut held = BTreeMap::new();
        for token in &active {
            *held.entry(token.instance_id()).or_insert(0_usize) += 1;
        }
        assert!(
            held.values().all(|count| *count == 1),
            "invariant 1: at most one active token per instance"
        );
        for instance_id in &self.instances {
            assert_eq!(
                self.scheduler
                    .active_lease(*instance_id)
                    .map(|lease| lease.token().clone()),
                active
                    .iter()
                    .find(|token| token.instance_id() == *instance_id)
                    .cloned(),
                "invariant 1: the instance's lease is its one active token"
            );
            let queued = self
                .scheduler
                .queued_on(*instance_id)
                .expect("queue status");
            assert_eq!(
                self.scheduler.queued_count(*instance_id),
                queued.len(),
                "invariant 2: one queue per instance"
            );
            let order = queued
                .iter()
                .map(|entry| {
                    let arrival = self
                        .arrivals
                        .get(&entry.request_id())
                        .copied()
                        .expect("invariant 3: every queued entry was queued by a step");
                    (
                        !entry.kind().holder_owned(),
                        Reverse(entry.priority()),
                        arrival,
                    )
                })
                .collect::<Vec<_>>();
            assert!(
                order.windows(2).all(|pair| pair[0] <= pair[1]),
                "invariant 3: continuation first, then High before Normal, then arrival"
            );
        }
        for connection_id in &self.connections {
            for token in self.scheduler.tokens_for_connection(*connection_id) {
                assert_eq!(
                    self.scheduler
                        .active_lease(token.instance_id())
                        .map(|lease| lease.token().clone()),
                    Some(token),
                    "invariant 4: a connection owns only active tokens"
                );
            }
        }
    }
}
