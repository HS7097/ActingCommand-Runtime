// SPDX-License-Identifier: AGPL-3.0-only

//! Per-instance C3a write admission, lease lifetime, and fencing authority.

#![forbid(unsafe_code)]

use actingcommand_contract::{
    HolderId, IdentifierIssuer, InstanceId, LeaseId, LeasePriority, LeaseQueueStatus, LeaseToken,
    MAX_LEASE_QUEUE_TIMEOUT_MS, OwnerEpoch, RequestId, RuntimeErrorCode, RuntimeErrorProjection,
};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

pub const DEFAULT_MAX_CLIENT_HEARTBEAT_INTERVAL_MS: u64 = 5_000;
pub const DEFAULT_TAKEOVER_COOLDOWN_MS: u64 = 6_000;
pub const DEFAULT_LEASE_TTL_MS: u64 = 120_000;
pub const DEFAULT_MAXIMUM_QUEUE_TIMEOUT_MS: u64 = 60_000;
pub const DEFAULT_MAX_QUEUE_DEPTH_PER_INSTANCE: usize = 64;
pub const MAX_QUEUE_DEPTH_PER_INSTANCE: usize = 4_096;

pub type SchedulerResult<T> = Result<T, SchedulerError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConnectionId(u64);

impl ConnectionId {
    pub fn new(value: u64) -> SchedulerResult<Self> {
        if value == 0 {
            return Err(SchedulerError::InvalidConnection);
        }
        Ok(Self(value))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerConfig {
    pub maximum_client_heartbeat_interval_ms: u64,
    pub takeover_cooldown_ms: u64,
    pub lease_ttl_ms: u64,
    pub maximum_queue_timeout_ms: u64,
    pub max_queue_depth_per_instance: usize,
}

impl SchedulerConfig {
    pub fn validate(self) -> SchedulerResult<Self> {
        if self.maximum_client_heartbeat_interval_ms == 0
            || self.takeover_cooldown_ms <= self.maximum_client_heartbeat_interval_ms
            || self.lease_ttl_ms <= self.maximum_client_heartbeat_interval_ms
            || self.maximum_queue_timeout_ms == 0
            || self.maximum_queue_timeout_ms > MAX_LEASE_QUEUE_TIMEOUT_MS
            || self.max_queue_depth_per_instance == 0
            || self.max_queue_depth_per_instance > MAX_QUEUE_DEPTH_PER_INSTANCE
        {
            return Err(SchedulerError::InvalidConfig);
        }
        Ok(self)
    }
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            maximum_client_heartbeat_interval_ms: DEFAULT_MAX_CLIENT_HEARTBEAT_INTERVAL_MS,
            takeover_cooldown_ms: DEFAULT_TAKEOVER_COOLDOWN_MS,
            lease_ttl_ms: DEFAULT_LEASE_TTL_MS,
            maximum_queue_timeout_ms: DEFAULT_MAXIMUM_QUEUE_TIMEOUT_MS,
            max_queue_depth_per_instance: DEFAULT_MAX_QUEUE_DEPTH_PER_INSTANCE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseReleaseReason {
    Explicit,
    Preempted,
    Disconnect,
    Expired,
    BackendFailure,
    HostShutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseTransferReason {
    Preempted,
    ExplicitRelease,
    Disconnect,
    Expired,
    BackendFailure,
    HostShutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleasedLease {
    pub token: LeaseToken,
    pub reason: LeaseReleaseReason,
}

pub enum LeasePreparation {
    Existing(LeaseToken),
    New(PreparedLease),
}

impl LeasePreparation {
    pub fn token(&self) -> &LeaseToken {
        match self {
            Self::Existing(token) => token,
            Self::New(prepared) => prepared.token(),
        }
    }

    pub const fn is_existing(&self) -> bool {
        matches!(self, Self::Existing(_))
    }
}

impl fmt::Debug for LeasePreparation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Existing(_) => "LeasePreparation::Existing(<opaque-token>)",
            Self::New(_) => "LeasePreparation::New(<opaque-token>)",
        })
    }
}

pub struct PreparedLease {
    token: LeaseToken,
    connection_id: ConnectionId,
    acquire_request_id: RequestId,
    priority: LeasePriority,
}

impl PreparedLease {
    pub const fn token(&self) -> &LeaseToken {
        &self.token
    }
}

impl fmt::Debug for PreparedLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PreparedLease(<opaque-token>)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchedulerError {
    InvalidConfig,
    InvalidConnection,
    IdentifierIssuance,
    ExpiryOverflow,
    Busy {
        holder_id: HolderId,
        lease_id: LeaseId,
        expires_at_monotonic_ms: u64,
    },
    Cooldown {
        retry_after_ms: u64,
    },
    StaleOwnerEpoch,
    LeaseMismatch,
    InstanceMismatch,
    HolderMismatch,
    ConnectionMismatch,
    LeaseExpired,
    LeaseNotExpired,
    LeaseMissing,
    QueueFull,
    QueueExpired,
    QueueMissing,
    QueueConnectionMismatch,
    QueueRequestMismatch,
    QueueTimeoutInvalid,
    QueueSequenceOverflow,
    TransferNotSafe,
    DestructiveStateMismatch,
}

impl SchedulerError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidConfig => "invalid_scheduler_config",
            Self::InvalidConnection => "invalid_connection_id",
            Self::IdentifierIssuance => "identifier_issuance_failed",
            Self::ExpiryOverflow => "lease_expiry_overflow",
            Self::Busy { .. } => "lease_busy",
            Self::Cooldown { .. } => "lease_cooldown",
            Self::StaleOwnerEpoch => "stale_owner_epoch",
            Self::LeaseMismatch => "lease_mismatch",
            Self::InstanceMismatch => "instance_mismatch",
            Self::HolderMismatch => "holder_mismatch",
            Self::ConnectionMismatch => "connection_mismatch",
            Self::LeaseExpired => "lease_expired",
            Self::LeaseNotExpired => "lease_not_expired",
            Self::LeaseMissing => "lease_missing",
            Self::QueueFull => "lease_queue_full",
            Self::QueueExpired => "lease_queue_expired",
            Self::QueueMissing => "lease_queue_missing",
            Self::QueueConnectionMismatch => "lease_queue_connection_mismatch",
            Self::QueueRequestMismatch => "lease_queue_request_mismatch",
            Self::QueueTimeoutInvalid => "lease_queue_timeout_invalid",
            Self::QueueSequenceOverflow => "lease_queue_sequence_overflow",
            Self::TransferNotSafe => "lease_transfer_not_safe",
            Self::DestructiveStateMismatch => "destructive_state_mismatch",
        }
    }

    pub const fn is_fatal(&self) -> bool {
        matches!(
            self,
            Self::InvalidConfig
                | Self::InvalidConnection
                | Self::IdentifierIssuance
                | Self::ExpiryOverflow
                | Self::QueueSequenceOverflow
        )
    }

    pub const fn projection(&self) -> RuntimeErrorProjection {
        match self {
            Self::InvalidConfig
            | Self::InvalidConnection
            | Self::IdentifierIssuance
            | Self::ExpiryOverflow => {
                RuntimeErrorProjection::new(RuntimeErrorCode::RuntimeFatal, true)
            }
            Self::Busy {
                holder_id,
                lease_id,
                ..
            } => RuntimeErrorProjection::new(RuntimeErrorCode::LeaseBusy, false)
                .with_holder(*holder_id, *lease_id),
            Self::Cooldown { retry_after_ms } => {
                RuntimeErrorProjection::new(RuntimeErrorCode::LeaseCooldown, false)
                    .with_retry_after(*retry_after_ms)
            }
            Self::StaleOwnerEpoch => {
                RuntimeErrorProjection::new(RuntimeErrorCode::StaleOwnerEpoch, false)
            }
            Self::LeaseMismatch => {
                RuntimeErrorProjection::new(RuntimeErrorCode::LeaseMismatch, false)
            }
            Self::InstanceMismatch => {
                RuntimeErrorProjection::new(RuntimeErrorCode::InstanceMismatch, false)
            }
            Self::HolderMismatch => {
                RuntimeErrorProjection::new(RuntimeErrorCode::HolderMismatch, false)
            }
            Self::ConnectionMismatch => {
                RuntimeErrorProjection::new(RuntimeErrorCode::ConnectionMismatch, false)
            }
            Self::LeaseExpired => {
                RuntimeErrorProjection::new(RuntimeErrorCode::LeaseExpired, false)
            }
            Self::LeaseNotExpired => {
                RuntimeErrorProjection::new(RuntimeErrorCode::LeaseMismatch, false)
            }
            Self::LeaseMissing => {
                RuntimeErrorProjection::new(RuntimeErrorCode::LeaseMissing, false)
            }
            Self::QueueFull => RuntimeErrorProjection::new(RuntimeErrorCode::QueueFull, false),
            Self::QueueExpired => {
                RuntimeErrorProjection::new(RuntimeErrorCode::QueueExpired, false)
            }
            Self::QueueMissing => {
                RuntimeErrorProjection::new(RuntimeErrorCode::QueueMissing, false)
            }
            Self::QueueConnectionMismatch | Self::QueueRequestMismatch => {
                RuntimeErrorProjection::new(RuntimeErrorCode::QueueConnectionMismatch, false)
            }
            Self::QueueTimeoutInvalid => {
                RuntimeErrorProjection::new(RuntimeErrorCode::InvalidRequest, false)
            }
            Self::QueueSequenceOverflow => {
                RuntimeErrorProjection::new(RuntimeErrorCode::RuntimeFatal, true)
            }
            Self::TransferNotSafe | Self::DestructiveStateMismatch => {
                RuntimeErrorProjection::new(RuntimeErrorCode::TransferNotSafe, false)
            }
        }
    }
}

impl fmt::Display for SchedulerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "scheduler error {}", self.code())
    }
}

impl Error for SchedulerError {}

#[derive(Clone, PartialEq, Eq)]
struct LeaseEntry {
    token: LeaseToken,
    connection_id: ConnectionId,
    acquire_request_id: RequestId,
    last_renew: Option<RenewRecord>,
    priority: LeasePriority,
    destructive_step_active: bool,
    preempt_requested: bool,
}

#[derive(Clone, PartialEq, Eq)]
struct QueueEntry {
    request_id: RequestId,
    holder_id: HolderId,
    connection_id: ConnectionId,
    priority: LeasePriority,
    deadline_monotonic_ms: u64,
    arrival_sequence: u64,
}

#[derive(Clone, PartialEq, Eq)]
struct RenewRecord {
    request_id: RequestId,
    submitted_token: LeaseToken,
    renewed_token: LeaseToken,
}

#[derive(Clone)]
struct ReleaseRecord {
    request_id: RequestId,
    submitted_token: LeaseToken,
    connection_id: ConnectionId,
    released: ReleasedLease,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueLeaseRequest {
    pub request_id: RequestId,
    pub instance_id: InstanceId,
    pub holder_id: HolderId,
    pub connection_id: ConnectionId,
    pub priority: LeasePriority,
    pub timeout_ms: u64,
}

impl QueueLeaseRequest {
    pub const fn new(
        request_id: RequestId,
        instance_id: InstanceId,
        holder_id: HolderId,
        connection_id: ConnectionId,
        priority: LeasePriority,
        timeout_ms: u64,
    ) -> Self {
        Self {
            request_id,
            instance_id,
            holder_id,
            connection_id,
            priority,
            timeout_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedLease {
    request_id: RequestId,
    instance_id: InstanceId,
    holder_id: HolderId,
    connection_id: ConnectionId,
    priority: LeasePriority,
    position: u32,
    deadline_monotonic_ms: u64,
    preempt_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveLease {
    token: LeaseToken,
    connection_id: ConnectionId,
    priority: LeasePriority,
    destructive_step_active: bool,
    preempt_requested: bool,
}

impl ActiveLease {
    pub const fn token(&self) -> &LeaseToken {
        &self.token
    }

    pub const fn connection_id(&self) -> ConnectionId {
        self.connection_id
    }

    pub const fn priority(&self) -> LeasePriority {
        self.priority
    }

    pub const fn destructive_step_active(&self) -> bool {
        self.destructive_step_active
    }

    pub const fn preempt_requested(&self) -> bool {
        self.preempt_requested
    }
}

impl QueuedLease {
    pub const fn request_id(&self) -> RequestId {
        self.request_id
    }

    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub const fn holder_id(&self) -> HolderId {
        self.holder_id
    }

    pub const fn connection_id(&self) -> ConnectionId {
        self.connection_id
    }

    pub const fn priority(&self) -> LeasePriority {
        self.priority
    }

    pub const fn position(&self) -> u32 {
        self.position
    }

    pub const fn deadline_monotonic_ms(&self) -> u64 {
        self.deadline_monotonic_ms
    }

    pub const fn preempt_requested(&self) -> bool {
        self.preempt_requested
    }

    pub fn status(&self) -> SchedulerResult<LeaseQueueStatus> {
        LeaseQueueStatus::new(
            self.request_id,
            self.instance_id,
            self.priority,
            self.position,
            self.deadline_monotonic_ms,
            self.preempt_requested,
        )
        .map_err(|_| SchedulerError::InvalidConfig)
    }
}

pub enum QueueAdmissionDecision {
    Lease(LeasePreparation),
    Queued(QueuedLease),
}

impl fmt::Debug for QueueAdmissionDecision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Lease(_) => "QueueAdmissionDecision::Lease(<opaque>)",
            Self::Queued(_) => "QueueAdmissionDecision::Queued(<opaque>)",
        })
    }
}

#[derive(Debug)]
pub struct QueueAdmissionOutcome {
    decision: QueueAdmissionDecision,
    expired: Vec<QueuedLease>,
}

impl QueueAdmissionOutcome {
    pub fn decision(&self) -> &QueueAdmissionDecision {
        &self.decision
    }

    pub fn into_decision(self) -> QueueAdmissionDecision {
        self.decision
    }

    pub fn expired(&self) -> &[QueuedLease] {
        &self.expired
    }

    pub fn into_parts(self) -> (QueueAdmissionDecision, Vec<QueuedLease>) {
        (self.decision, self.expired)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueuePoll {
    Granted(LeaseToken),
    Pending(QueuedLease),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelledQueuedLease {
    queued: QueuedLease,
}

impl CancelledQueuedLease {
    pub const fn queued(&self) -> &QueuedLease {
        &self.queued
    }
}

pub enum TransferPreparation {
    NoCandidate,
    Deferred,
    Ready(Box<PreparedLeaseTransfer>),
}

impl fmt::Debug for TransferPreparation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NoCandidate => "TransferPreparation::NoCandidate",
            Self::Deferred => "TransferPreparation::Deferred",
            Self::Ready(_) => "TransferPreparation::Ready(<opaque>)",
        })
    }
}

pub struct PreparedLeaseTransfer {
    from: LeaseEntry,
    queued: QueueEntry,
    to_token: LeaseToken,
    reason: LeaseTransferReason,
    release_request_id: Option<RequestId>,
}

impl PreparedLeaseTransfer {
    pub const fn from_token(&self) -> &LeaseToken {
        &self.from.token
    }

    pub const fn to_token(&self) -> &LeaseToken {
        &self.to_token
    }

    pub const fn queued_request_id(&self) -> RequestId {
        self.queued.request_id
    }

    pub const fn from_connection_id(&self) -> ConnectionId {
        self.from.connection_id
    }

    pub const fn to_connection_id(&self) -> ConnectionId {
        self.queued.connection_id
    }

    pub const fn reason(&self) -> LeaseTransferReason {
        self.reason
    }

    pub const fn priority(&self) -> LeasePriority {
        self.queued.priority
    }

    pub const fn release_request_id(&self) -> Option<RequestId> {
        self.release_request_id
    }
}

impl fmt::Debug for PreparedLeaseTransfer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PreparedLeaseTransfer(<opaque>)")
    }
}

#[derive(Default)]
struct InstanceState {
    lease: Option<LeaseEntry>,
    last_release: Option<ReleaseRecord>,
    cooldown_until_monotonic_ms: u64,
    queue: Vec<QueueEntry>,
}

pub struct SeedScheduler {
    owner_epoch: OwnerEpoch,
    config: SchedulerConfig,
    lease_issuer: IdentifierIssuer,
    instances: BTreeMap<InstanceId, InstanceState>,
    lease_locations: BTreeMap<LeaseId, InstanceId>,
    next_arrival_sequence: u64,
}

impl fmt::Debug for SeedScheduler {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SeedScheduler")
            .field("owner_epoch", &self.owner_epoch)
            .field("config", &self.config)
            .field("instance_count", &self.instances.len())
            .field("active_lease_count", &self.lease_locations.len())
            .finish()
    }
}

impl SeedScheduler {
    pub fn new(
        owner_epoch: OwnerEpoch,
        config: SchedulerConfig,
        takeover_instances: impl IntoIterator<Item = InstanceId>,
        now_monotonic_ms: u64,
    ) -> SchedulerResult<Self> {
        let config = config.validate()?;
        let cooldown_until = now_monotonic_ms
            .checked_add(config.takeover_cooldown_ms)
            .ok_or(SchedulerError::ExpiryOverflow)?;
        let mut instances = BTreeMap::new();
        for instance_id in takeover_instances {
            instances.entry(instance_id).or_insert(InstanceState {
                cooldown_until_monotonic_ms: cooldown_until,
                ..InstanceState::default()
            });
        }
        Ok(Self {
            owner_epoch,
            config,
            lease_issuer: IdentifierIssuer::new()
                .map_err(|_| SchedulerError::IdentifierIssuance)?,
            instances,
            lease_locations: BTreeMap::new(),
            next_arrival_sequence: 1,
        })
    }

    pub const fn owner_epoch(&self) -> OwnerEpoch {
        self.owner_epoch
    }

    pub const fn config(&self) -> SchedulerConfig {
        self.config
    }

    pub fn acquire(
        &mut self,
        request_id: RequestId,
        instance_id: InstanceId,
        holder_id: HolderId,
        connection_id: ConnectionId,
        now_monotonic_ms: u64,
    ) -> SchedulerResult<LeaseToken> {
        let preparation = self.prepare_acquire(
            request_id,
            instance_id,
            holder_id,
            connection_id,
            now_monotonic_ms,
        )?;
        self.commit_acquire(preparation, now_monotonic_ms)
    }

    pub fn prepare_acquire(
        &mut self,
        request_id: RequestId,
        instance_id: InstanceId,
        holder_id: HolderId,
        connection_id: ConnectionId,
        now_monotonic_ms: u64,
    ) -> SchedulerResult<LeasePreparation> {
        self.prepare_acquire_with_priority(
            request_id,
            instance_id,
            holder_id,
            connection_id,
            LeasePriority::Normal,
            now_monotonic_ms,
        )
    }

    fn prepare_acquire_with_priority(
        &mut self,
        request_id: RequestId,
        instance_id: InstanceId,
        holder_id: HolderId,
        connection_id: ConnectionId,
        priority: LeasePriority,
        now_monotonic_ms: u64,
    ) -> SchedulerResult<LeasePreparation> {
        {
            let state = self.instances.entry(instance_id).or_default();
            if state.cooldown_until_monotonic_ms > now_monotonic_ms {
                return Err(SchedulerError::Cooldown {
                    retry_after_ms: state.cooldown_until_monotonic_ms - now_monotonic_ms,
                });
            }
            state.cooldown_until_monotonic_ms = 0;
            if let Some(lease) = &state.lease {
                if lease.token.expires_at_monotonic_ms() <= now_monotonic_ms {
                    return Err(SchedulerError::LeaseExpired);
                }
                if lease.acquire_request_id == request_id
                    && lease.connection_id == connection_id
                    && lease.token.holder_id() == holder_id
                {
                    return Ok(LeasePreparation::Existing(lease.token.clone()));
                }
                return Err(SchedulerError::Busy {
                    holder_id: lease.token.holder_id(),
                    lease_id: lease.token.lease_id(),
                    expires_at_monotonic_ms: lease.token.expires_at_monotonic_ms(),
                });
            }
        }
        let expires_at_monotonic_ms = self.expiry_from(now_monotonic_ms)?;
        let lease_id = *self
            .lease_issuer
            .mint_lease_id()
            .map_err(|_| SchedulerError::IdentifierIssuance)?
            .transport();
        let token = LeaseToken::new(
            self.owner_epoch,
            lease_id,
            instance_id,
            holder_id,
            expires_at_monotonic_ms,
        )
        .map_err(|_| SchedulerError::InvalidConfig)?;
        Ok(LeasePreparation::New(PreparedLease {
            token,
            connection_id,
            acquire_request_id: request_id,
            priority,
        }))
    }

    pub fn commit_acquire(
        &mut self,
        preparation: LeasePreparation,
        now_monotonic_ms: u64,
    ) -> SchedulerResult<LeaseToken> {
        let LeasePreparation::New(prepared) = preparation else {
            let LeasePreparation::Existing(token) = preparation else {
                unreachable!();
            };
            return Ok(token);
        };
        self.validate_epoch(&prepared.token)?;
        if prepared.token.expires_at_monotonic_ms() <= now_monotonic_ms {
            return Err(SchedulerError::LeaseExpired);
        }
        let instance_id = prepared.token.instance_id();
        let state = self
            .instances
            .get_mut(&instance_id)
            .ok_or(SchedulerError::LeaseMissing)?;
        if state.cooldown_until_monotonic_ms > now_monotonic_ms {
            return Err(SchedulerError::Cooldown {
                retry_after_ms: state.cooldown_until_monotonic_ms - now_monotonic_ms,
            });
        }
        if let Some(lease) = &state.lease {
            return Err(SchedulerError::Busy {
                holder_id: lease.token.holder_id(),
                lease_id: lease.token.lease_id(),
                expires_at_monotonic_ms: lease.token.expires_at_monotonic_ms(),
            });
        }
        let lease_id = prepared.token.lease_id();
        let token = prepared.token;
        state.lease = Some(LeaseEntry {
            token: token.clone(),
            connection_id: prepared.connection_id,
            acquire_request_id: prepared.acquire_request_id,
            last_renew: None,
            priority: prepared.priority,
            destructive_step_active: false,
            preempt_requested: false,
        });
        state.last_release = None;
        self.lease_locations.insert(lease_id, instance_id);
        Ok(token)
    }

    pub fn request_queued(
        &mut self,
        request: QueueLeaseRequest,
        now_monotonic_ms: u64,
    ) -> SchedulerResult<QueueAdmissionOutcome> {
        let QueueLeaseRequest {
            request_id,
            instance_id,
            holder_id,
            connection_id,
            priority,
            timeout_ms,
        } = request;
        if timeout_ms == 0 || timeout_ms > self.config.maximum_queue_timeout_ms {
            return Err(SchedulerError::QueueTimeoutInvalid);
        }
        if let Some(decision) =
            self.replay_queue_request(request_id, instance_id, holder_id, connection_id, priority)?
        {
            if matches!(
                &decision,
                QueueAdmissionDecision::Queued(queued)
                    if queued.deadline_monotonic_ms() <= now_monotonic_ms
            ) {
                self.remove_queued_request(request_id)?;
                return Err(SchedulerError::QueueExpired);
            }
            return Ok(QueueAdmissionOutcome {
                decision,
                expired: Vec::new(),
            });
        }
        let expired = self.take_expired_for_instance(instance_id, now_monotonic_ms)?;
        let has_active_lease = self
            .instances
            .get(&instance_id)
            .and_then(|state| state.lease.as_ref())
            .is_some();
        if !has_active_lease {
            let preparation = self.prepare_acquire_with_priority(
                request_id,
                instance_id,
                holder_id,
                connection_id,
                priority,
                now_monotonic_ms,
            )?;
            return Ok(QueueAdmissionOutcome {
                decision: QueueAdmissionDecision::Lease(preparation),
                expired,
            });
        }
        let deadline_monotonic_ms = now_monotonic_ms
            .checked_add(timeout_ms)
            .ok_or(SchedulerError::ExpiryOverflow)?;
        let arrival_sequence = self.next_arrival_sequence;
        self.next_arrival_sequence = self
            .next_arrival_sequence
            .checked_add(1)
            .ok_or(SchedulerError::QueueSequenceOverflow)?;
        let state = self
            .instances
            .get_mut(&instance_id)
            .ok_or(SchedulerError::LeaseMissing)?;
        if state.queue.len() >= self.config.max_queue_depth_per_instance {
            return Err(SchedulerError::QueueFull);
        }
        state.queue.push(QueueEntry {
            request_id,
            holder_id,
            connection_id,
            priority,
            deadline_monotonic_ms,
            arrival_sequence,
        });
        sort_queue(&mut state.queue);
        refresh_preempt_requested(state);
        let queued = queued_by_request(state, instance_id, request_id)?;
        Ok(QueueAdmissionOutcome {
            decision: QueueAdmissionDecision::Queued(queued),
            expired,
        })
    }

    pub fn poll_queued(
        &mut self,
        request_id: RequestId,
        connection_id: ConnectionId,
        now_monotonic_ms: u64,
    ) -> SchedulerResult<QueuePoll> {
        for state in self.instances.values() {
            if let Some(lease) = &state.lease
                && lease.acquire_request_id == request_id
            {
                if lease.connection_id != connection_id {
                    return Err(SchedulerError::QueueConnectionMismatch);
                }
                return Ok(QueuePoll::Granted(lease.token.clone()));
            }
        }
        let location = self
            .instances
            .iter()
            .find_map(|(instance_id, state)| {
                state
                    .queue
                    .iter()
                    .position(|entry| entry.request_id == request_id)
                    .map(|position| (*instance_id, position))
            })
            .ok_or(SchedulerError::QueueMissing)?;
        let (instance_id, position) = location;
        let state = self
            .instances
            .get_mut(&instance_id)
            .ok_or(SchedulerError::QueueMissing)?;
        if state.queue[position].connection_id != connection_id {
            return Err(SchedulerError::QueueConnectionMismatch);
        }
        if state.queue[position].deadline_monotonic_ms <= now_monotonic_ms {
            state.queue.remove(position);
            refresh_preempt_requested(state);
            return Err(SchedulerError::QueueExpired);
        }
        Ok(QueuePoll::Pending(queued_at_position(
            state,
            instance_id,
            position,
        )?))
    }

    pub fn cancel_queued(
        &mut self,
        request_id: RequestId,
        connection_id: ConnectionId,
    ) -> SchedulerResult<CancelledQueuedLease> {
        let location = self
            .instances
            .iter()
            .find_map(|(instance_id, state)| {
                state
                    .queue
                    .iter()
                    .position(|entry| entry.request_id == request_id)
                    .map(|position| (*instance_id, position))
            })
            .ok_or(SchedulerError::QueueMissing)?;
        let (instance_id, position) = location;
        let state = self
            .instances
            .get_mut(&instance_id)
            .ok_or(SchedulerError::QueueMissing)?;
        if state.queue[position].connection_id != connection_id {
            return Err(SchedulerError::QueueConnectionMismatch);
        }
        let queued = queued_at_position(state, instance_id, position)?;
        state.queue.remove(position);
        refresh_preempt_requested(state);
        Ok(CancelledQueuedLease { queued })
    }

    pub fn take_expired_queued(
        &mut self,
        now_monotonic_ms: u64,
    ) -> SchedulerResult<Vec<QueuedLease>> {
        let instance_ids = self.instances.keys().copied().collect::<Vec<_>>();
        let mut expired = Vec::new();
        for instance_id in instance_ids {
            expired.extend(self.take_expired_for_instance(instance_id, now_monotonic_ms)?);
        }
        Ok(expired)
    }

    pub fn remove_queued_for_connection(
        &mut self,
        connection_id: ConnectionId,
    ) -> SchedulerResult<Vec<CancelledQueuedLease>> {
        let mut removed = Vec::new();
        for (instance_id, state) in &mut self.instances {
            let mut position = 0;
            while position < state.queue.len() {
                if state.queue[position].connection_id == connection_id {
                    removed.push(CancelledQueuedLease {
                        queued: queued_at_position(state, *instance_id, position)?,
                    });
                    state.queue.remove(position);
                } else {
                    position += 1;
                }
            }
            refresh_preempt_requested(state);
        }
        Ok(removed)
    }

    pub fn queued_instance_ids_for_connection(
        &self,
        connection_id: ConnectionId,
    ) -> Vec<InstanceId> {
        self.instances
            .iter()
            .filter_map(|(instance_id, state)| {
                state
                    .queue
                    .iter()
                    .any(|entry| entry.connection_id == connection_id)
                    .then_some(*instance_id)
            })
            .collect()
    }

    pub fn remove_queued_for_connection_on_instance(
        &mut self,
        instance_id: InstanceId,
        connection_id: ConnectionId,
    ) -> SchedulerResult<Vec<CancelledQueuedLease>> {
        let Some(state) = self.instances.get_mut(&instance_id) else {
            return Ok(Vec::new());
        };
        let mut removed = Vec::new();
        let mut position = 0;
        while position < state.queue.len() {
            if state.queue[position].connection_id == connection_id {
                removed.push(CancelledQueuedLease {
                    queued: queued_at_position(state, instance_id, position)?,
                });
                state.queue.remove(position);
            } else {
                position += 1;
            }
        }
        refresh_preempt_requested(state);
        Ok(removed)
    }

    pub fn remove_queued_for_instance(
        &mut self,
        instance_id: InstanceId,
    ) -> SchedulerResult<Vec<CancelledQueuedLease>> {
        let Some(state) = self.instances.get_mut(&instance_id) else {
            return Ok(Vec::new());
        };
        let mut removed = Vec::with_capacity(state.queue.len());
        while !state.queue.is_empty() {
            removed.push(CancelledQueuedLease {
                queued: queued_at_position(state, instance_id, 0)?,
            });
            state.queue.remove(0);
        }
        refresh_preempt_requested(state);
        Ok(removed)
    }

    pub fn begin_destructive_step(
        &mut self,
        token: &LeaseToken,
        connection_id: ConnectionId,
        now_monotonic_ms: u64,
    ) -> SchedulerResult<()> {
        self.validate_write(token, connection_id, now_monotonic_ms)?;
        let state = self
            .instances
            .get_mut(&token.instance_id())
            .ok_or(SchedulerError::LeaseMissing)?;
        let lease = state.lease.as_mut().ok_or(SchedulerError::LeaseMissing)?;
        if lease.destructive_step_active {
            return Err(SchedulerError::DestructiveStateMismatch);
        }
        if lease.preempt_requested {
            return Err(SchedulerError::TransferNotSafe);
        }
        lease.destructive_step_active = true;
        Ok(())
    }

    pub fn finish_destructive_step(
        &mut self,
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> SchedulerResult<()> {
        self.validate_epoch(token)?;
        let instance_id = self.locate_token_instance(token)?;
        let state = self
            .instances
            .get_mut(&instance_id)
            .ok_or(SchedulerError::LeaseMissing)?;
        let lease = state.lease.as_mut().ok_or(SchedulerError::LeaseMissing)?;
        if lease.token != *token || lease.connection_id != connection_id {
            return Err(SchedulerError::ConnectionMismatch);
        }
        if !lease.destructive_step_active {
            return Err(SchedulerError::DestructiveStateMismatch);
        }
        lease.destructive_step_active = false;
        Ok(())
    }

    pub fn prepare_transfer(
        &mut self,
        token: &LeaseToken,
        connection_id: ConnectionId,
        reason: LeaseTransferReason,
        release_request_id: Option<RequestId>,
        now_monotonic_ms: u64,
    ) -> SchedulerResult<TransferPreparation> {
        if (reason == LeaseTransferReason::ExplicitRelease) != release_request_id.is_some() {
            return Err(SchedulerError::QueueRequestMismatch);
        }
        self.validate_epoch(token)?;
        let instance_id = self.locate_token_instance(token)?;
        let state = self
            .instances
            .get(&instance_id)
            .ok_or(SchedulerError::LeaseMissing)?;
        let lease = state.lease.as_ref().ok_or(SchedulerError::LeaseMissing)?;
        if lease.token != *token {
            return Err(SchedulerError::LeaseMismatch);
        }
        if lease.connection_id != connection_id {
            return Err(SchedulerError::ConnectionMismatch);
        }
        if lease.destructive_step_active {
            return Ok(TransferPreparation::Deferred);
        }
        let Some(queued) = state.queue.first() else {
            return Ok(TransferPreparation::NoCandidate);
        };
        if queued.deadline_monotonic_ms <= now_monotonic_ms {
            return Err(SchedulerError::QueueExpired);
        }
        if reason == LeaseTransferReason::Preempted && queued.priority <= lease.priority {
            return Ok(TransferPreparation::NoCandidate);
        }
        let expires_at_monotonic_ms = self.expiry_from(now_monotonic_ms)?;
        let lease_id = *self
            .lease_issuer
            .mint_lease_id()
            .map_err(|_| SchedulerError::IdentifierIssuance)?
            .transport();
        let to_token = LeaseToken::new(
            self.owner_epoch,
            lease_id,
            instance_id,
            queued.holder_id,
            expires_at_monotonic_ms,
        )
        .map_err(|_| SchedulerError::InvalidConfig)?;
        Ok(TransferPreparation::Ready(Box::new(
            PreparedLeaseTransfer {
                from: lease.clone(),
                queued: queued.clone(),
                to_token,
                reason,
                release_request_id,
            },
        )))
    }

    pub fn commit_transfer(
        &mut self,
        prepared: Box<PreparedLeaseTransfer>,
        now_monotonic_ms: u64,
    ) -> SchedulerResult<LeaseToken> {
        let prepared = *prepared;
        self.validate_epoch(&prepared.from.token)?;
        self.validate_epoch(&prepared.to_token)?;
        if prepared.to_token.expires_at_monotonic_ms() <= now_monotonic_ms {
            return Err(SchedulerError::LeaseExpired);
        }
        let instance_id = prepared.from.token.instance_id();
        let state = self
            .instances
            .get_mut(&instance_id)
            .ok_or(SchedulerError::LeaseMissing)?;
        if state.lease.as_ref() != Some(&prepared.from) {
            return Err(SchedulerError::LeaseMismatch);
        }
        if state.queue.first() != Some(&prepared.queued) {
            return Err(SchedulerError::QueueRequestMismatch);
        }
        let queued = state.queue.remove(0);
        let released = ReleasedLease {
            token: prepared.from.token.clone(),
            reason: transfer_release_reason(prepared.reason),
        };
        state.last_release = prepared.release_request_id.map(|request_id| ReleaseRecord {
            request_id,
            submitted_token: prepared.from.token.clone(),
            connection_id: prepared.from.connection_id,
            released,
        });
        let token = prepared.to_token;
        state.lease = Some(LeaseEntry {
            token: token.clone(),
            connection_id: queued.connection_id,
            acquire_request_id: queued.request_id,
            last_renew: None,
            priority: queued.priority,
            destructive_step_active: false,
            preempt_requested: false,
        });
        refresh_preempt_requested(state);
        self.lease_locations.remove(&prepared.from.token.lease_id());
        self.lease_locations.insert(token.lease_id(), instance_id);
        Ok(token)
    }

    pub fn renew(
        &mut self,
        request_id: RequestId,
        submitted_token: &LeaseToken,
        connection_id: ConnectionId,
        now_monotonic_ms: u64,
    ) -> SchedulerResult<LeaseToken> {
        if let Some(renewed) = self.replayed_renew(request_id, submitted_token, connection_id)? {
            return Ok(renewed);
        }
        self.validate_epoch(submitted_token)?;
        let instance_id = self.locate_token_instance(submitted_token)?;
        let expires_at = self.expiry_from(now_monotonic_ms)?;
        let state = self
            .instances
            .get_mut(&instance_id)
            .ok_or(SchedulerError::LeaseMissing)?;
        let lease = state.lease.as_mut().ok_or(SchedulerError::LeaseMissing)?;
        validate_active_lease(lease, submitted_token, connection_id, now_monotonic_ms)?;
        let renewed = LeaseToken::new(
            self.owner_epoch,
            lease.token.lease_id(),
            instance_id,
            lease.token.holder_id(),
            expires_at,
        )
        .map_err(|_| SchedulerError::InvalidConfig)?;
        lease.last_renew = Some(RenewRecord {
            request_id,
            submitted_token: submitted_token.clone(),
            renewed_token: renewed.clone(),
        });
        lease.token = renewed.clone();
        Ok(renewed)
    }

    /// Recovers the most recent matching renew result without mutating lease state.
    pub fn replayed_renew(
        &self,
        request_id: RequestId,
        submitted_token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> SchedulerResult<Option<LeaseToken>> {
        self.validate_epoch(submitted_token)?;
        let Some(lease) = self
            .instances
            .get(&submitted_token.instance_id())
            .and_then(|state| state.lease.as_ref())
        else {
            return Ok(None);
        };
        let Some(record) = lease
            .last_renew
            .as_ref()
            .filter(|record| record.request_id == request_id)
        else {
            return Ok(None);
        };
        if record.submitted_token != *submitted_token {
            return Err(SchedulerError::LeaseMismatch);
        }
        if lease.connection_id != connection_id {
            return Err(SchedulerError::ConnectionMismatch);
        }
        Ok(Some(record.renewed_token.clone()))
    }

    pub fn release(
        &mut self,
        request_id: RequestId,
        submitted_token: &LeaseToken,
        connection_id: ConnectionId,
        now_monotonic_ms: u64,
    ) -> SchedulerResult<ReleasedLease> {
        if let Some(released) = self.replayed_release(request_id, submitted_token, connection_id)? {
            return Ok(released);
        }
        self.validate_epoch(submitted_token)?;
        let instance_id = self.locate_token_instance(submitted_token)?;
        let state = self
            .instances
            .get_mut(&instance_id)
            .ok_or(SchedulerError::LeaseMissing)?;
        let lease = state.lease.as_ref().ok_or(SchedulerError::LeaseMissing)?;
        validate_active_lease(lease, submitted_token, connection_id, now_monotonic_ms)?;
        let released = ReleasedLease {
            token: lease.token.clone(),
            reason: LeaseReleaseReason::Explicit,
        };
        state.lease = None;
        state.last_release = Some(ReleaseRecord {
            request_id,
            submitted_token: submitted_token.clone(),
            connection_id,
            released: released.clone(),
        });
        self.lease_locations.remove(&submitted_token.lease_id());
        Ok(released)
    }

    /// Recovers the most recent matching release result without mutating lease state.
    pub fn replayed_release(
        &self,
        request_id: RequestId,
        submitted_token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> SchedulerResult<Option<ReleasedLease>> {
        self.validate_epoch(submitted_token)?;
        let Some(record) = self
            .instances
            .get(&submitted_token.instance_id())
            .and_then(|state| state.last_release.as_ref())
            .filter(|record| record.request_id == request_id)
        else {
            return Ok(None);
        };
        if record.submitted_token != *submitted_token {
            return Err(SchedulerError::LeaseMismatch);
        }
        if record.connection_id != connection_id {
            return Err(SchedulerError::ConnectionMismatch);
        }
        Ok(Some(record.released.clone()))
    }

    pub fn validate_write(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
        now_monotonic_ms: u64,
    ) -> SchedulerResult<()> {
        self.validate_epoch(token)?;
        if let Some(state) = self.instances.get(&token.instance_id())
            && state.cooldown_until_monotonic_ms > now_monotonic_ms
        {
            return Err(SchedulerError::Cooldown {
                retry_after_ms: state.cooldown_until_monotonic_ms - now_monotonic_ms,
            });
        }
        let instance_id = self.locate_token_instance(token)?;
        let state = self
            .instances
            .get(&instance_id)
            .ok_or(SchedulerError::LeaseMissing)?;
        if state.cooldown_until_monotonic_ms > now_monotonic_ms {
            return Err(SchedulerError::Cooldown {
                retry_after_ms: state.cooldown_until_monotonic_ms - now_monotonic_ms,
            });
        }
        let lease = state.lease.as_ref().ok_or(SchedulerError::LeaseMissing)?;
        validate_active_lease(lease, token, connection_id, now_monotonic_ms)
    }

    pub fn due_tokens(&self, now_monotonic_ms: u64) -> Vec<LeaseToken> {
        self.instances
            .values()
            .filter_map(|state| {
                state
                    .lease
                    .as_ref()
                    .filter(|lease| lease.token.expires_at_monotonic_ms() <= now_monotonic_ms)
                    .map(|lease| lease.token.clone())
            })
            .collect()
    }

    pub fn tokens_for_connection(&self, connection_id: ConnectionId) -> Vec<LeaseToken> {
        self.instances
            .values()
            .filter_map(|state| {
                state
                    .lease
                    .as_ref()
                    .filter(|lease| lease.connection_id == connection_id)
                    .map(|lease| lease.token.clone())
            })
            .collect()
    }

    pub fn connection_for_token(&self, token: &LeaseToken) -> SchedulerResult<ConnectionId> {
        self.validate_epoch(token)?;
        let instance_id = self.locate_token_instance(token)?;
        let state = self
            .instances
            .get(&instance_id)
            .ok_or(SchedulerError::LeaseMissing)?;
        let lease = state.lease.as_ref().ok_or(SchedulerError::LeaseMissing)?;
        if lease.token != *token {
            return Err(SchedulerError::LeaseMismatch);
        }
        Ok(lease.connection_id)
    }

    pub fn expire_token(
        &mut self,
        submitted_token: &LeaseToken,
        now_monotonic_ms: u64,
    ) -> SchedulerResult<ReleasedLease> {
        self.validate_epoch(submitted_token)?;
        let instance_id = self.locate_token_instance(submitted_token)?;
        let state = self
            .instances
            .get_mut(&instance_id)
            .ok_or(SchedulerError::LeaseMissing)?;
        let lease = state.lease.as_ref().ok_or(SchedulerError::LeaseMissing)?;
        if lease.token != *submitted_token {
            return Err(SchedulerError::LeaseMismatch);
        }
        if lease.token.expires_at_monotonic_ms() > now_monotonic_ms {
            return Err(SchedulerError::LeaseNotExpired);
        }
        let lease = state.lease.take().ok_or(SchedulerError::LeaseMissing)?;
        self.lease_locations.remove(&lease.token.lease_id());
        state.last_release = None;
        Ok(ReleasedLease {
            token: lease.token,
            reason: LeaseReleaseReason::Expired,
        })
    }

    pub fn release_owned(
        &mut self,
        submitted_token: &LeaseToken,
        connection_id: ConnectionId,
        reason: LeaseReleaseReason,
    ) -> SchedulerResult<ReleasedLease> {
        if matches!(
            reason,
            LeaseReleaseReason::Explicit
                | LeaseReleaseReason::Preempted
                | LeaseReleaseReason::Expired
        ) {
            return Err(SchedulerError::LeaseMismatch);
        }
        self.validate_epoch(submitted_token)?;
        let instance_id = self.locate_token_instance(submitted_token)?;
        let state = self
            .instances
            .get_mut(&instance_id)
            .ok_or(SchedulerError::LeaseMissing)?;
        let lease = state.lease.as_ref().ok_or(SchedulerError::LeaseMissing)?;
        if lease.token != *submitted_token {
            return Err(SchedulerError::LeaseMismatch);
        }
        if lease.connection_id != connection_id {
            return Err(SchedulerError::ConnectionMismatch);
        }
        let lease = state.lease.take().ok_or(SchedulerError::LeaseMissing)?;
        self.lease_locations.remove(&lease.token.lease_id());
        state.last_release = None;
        Ok(ReleasedLease {
            token: lease.token,
            reason,
        })
    }

    pub fn rollback_lease(&mut self, token: &LeaseToken) -> SchedulerResult<()> {
        self.validate_epoch(token)?;
        let instance_id = self.locate_token_instance(token)?;
        let state = self
            .instances
            .get_mut(&instance_id)
            .ok_or(SchedulerError::LeaseMissing)?;
        let lease = state.lease.as_ref().ok_or(SchedulerError::LeaseMissing)?;
        if lease.token != *token {
            return Err(SchedulerError::LeaseMismatch);
        }
        state.lease = None;
        state.last_release = None;
        self.lease_locations.remove(&token.lease_id());
        Ok(())
    }

    pub fn active_tokens(&self) -> Vec<LeaseToken> {
        self.instances
            .values()
            .filter_map(|state| state.lease.as_ref().map(|lease| lease.token.clone()))
            .collect()
    }

    pub fn active_lease(&self, instance_id: InstanceId) -> Option<ActiveLease> {
        self.instances
            .get(&instance_id)
            .and_then(|state| state.lease.as_ref())
            .map(|lease| ActiveLease {
                token: lease.token.clone(),
                connection_id: lease.connection_id,
                priority: lease.priority,
                destructive_step_active: lease.destructive_step_active,
                preempt_requested: lease.preempt_requested,
            })
    }

    pub fn active_instance_ids(&self) -> Vec<InstanceId> {
        self.instances
            .iter()
            .filter_map(|(instance_id, state)| state.lease.as_ref().map(|_| *instance_id))
            .collect()
    }

    pub fn protected_instance_ids(&self, now_monotonic_ms: u64) -> Vec<InstanceId> {
        self.instances
            .iter()
            .filter_map(|(instance_id, state)| {
                (state.lease.is_some() || state.cooldown_until_monotonic_ms > now_monotonic_ms)
                    .then_some(*instance_id)
            })
            .collect()
    }

    pub fn clear_elapsed_cooldowns(&mut self, now_monotonic_ms: u64) -> bool {
        let mut changed = false;
        for state in self.instances.values_mut() {
            if state.cooldown_until_monotonic_ms != 0
                && state.cooldown_until_monotonic_ms <= now_monotonic_ms
            {
                state.cooldown_until_monotonic_ms = 0;
                changed = true;
            }
        }
        changed
    }

    fn replay_queue_request(
        &self,
        request_id: RequestId,
        instance_id: InstanceId,
        holder_id: HolderId,
        connection_id: ConnectionId,
        priority: LeasePriority,
    ) -> SchedulerResult<Option<QueueAdmissionDecision>> {
        for (candidate_instance, state) in &self.instances {
            if let Some(lease) = &state.lease
                && lease.acquire_request_id == request_id
            {
                if *candidate_instance != instance_id
                    || lease.token.holder_id() != holder_id
                    || lease.priority != priority
                {
                    return Err(SchedulerError::QueueRequestMismatch);
                }
                if lease.connection_id != connection_id {
                    return Err(SchedulerError::QueueConnectionMismatch);
                }
                return Ok(Some(QueueAdmissionDecision::Lease(
                    LeasePreparation::Existing(lease.token.clone()),
                )));
            }
            if let Some((position, queued)) = state
                .queue
                .iter()
                .enumerate()
                .find(|(_, queued)| queued.request_id == request_id)
            {
                if *candidate_instance != instance_id
                    || queued.holder_id != holder_id
                    || queued.priority != priority
                {
                    return Err(SchedulerError::QueueRequestMismatch);
                }
                if queued.connection_id != connection_id {
                    return Err(SchedulerError::QueueConnectionMismatch);
                }
                return Ok(Some(QueueAdmissionDecision::Queued(queued_at_position(
                    state,
                    *candidate_instance,
                    position,
                )?)));
            }
        }
        Ok(None)
    }

    pub fn take_expired_for_instance(
        &mut self,
        instance_id: InstanceId,
        now_monotonic_ms: u64,
    ) -> SchedulerResult<Vec<QueuedLease>> {
        let Some(state) = self.instances.get_mut(&instance_id) else {
            return Ok(Vec::new());
        };
        let mut expired = Vec::new();
        let mut position = 0;
        while position < state.queue.len() {
            if state.queue[position].deadline_monotonic_ms <= now_monotonic_ms {
                expired.push(queued_at_position(state, instance_id, position)?);
                state.queue.remove(position);
            } else {
                position += 1;
            }
        }
        refresh_preempt_requested(state);
        Ok(expired)
    }

    fn remove_queued_request(&mut self, request_id: RequestId) -> SchedulerResult<()> {
        let (instance_id, position) = self
            .instances
            .iter()
            .find_map(|(instance_id, state)| {
                state
                    .queue
                    .iter()
                    .position(|entry| entry.request_id == request_id)
                    .map(|position| (*instance_id, position))
            })
            .ok_or(SchedulerError::QueueMissing)?;
        let state = self
            .instances
            .get_mut(&instance_id)
            .ok_or(SchedulerError::QueueMissing)?;
        state.queue.remove(position);
        refresh_preempt_requested(state);
        Ok(())
    }

    fn expiry_from(&self, now_monotonic_ms: u64) -> SchedulerResult<u64> {
        now_monotonic_ms
            .checked_add(self.config.lease_ttl_ms)
            .ok_or(SchedulerError::ExpiryOverflow)
    }

    fn validate_epoch(&self, token: &LeaseToken) -> SchedulerResult<()> {
        if token.owner_epoch() != self.owner_epoch {
            return Err(SchedulerError::StaleOwnerEpoch);
        }
        Ok(())
    }

    fn locate_token_instance(&self, token: &LeaseToken) -> SchedulerResult<InstanceId> {
        let located = self
            .lease_locations
            .get(&token.lease_id())
            .copied()
            .ok_or_else(|| {
                let state = self.instances.get(&token.instance_id());
                if state.is_some_and(|state| state.cooldown_until_monotonic_ms > 0) {
                    SchedulerError::LeaseMissing
                } else {
                    SchedulerError::LeaseMismatch
                }
            })?;
        if located != token.instance_id() {
            return Err(SchedulerError::InstanceMismatch);
        }
        Ok(located)
    }
}

fn sort_queue(queue: &mut [QueueEntry]) {
    queue.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| left.arrival_sequence.cmp(&right.arrival_sequence))
    });
}

fn refresh_preempt_requested(state: &mut InstanceState) {
    let preempt_requested = state
        .lease
        .as_ref()
        .zip(state.queue.first())
        .is_some_and(|(lease, queued)| queued.priority > lease.priority);
    if let Some(lease) = state.lease.as_mut() {
        lease.preempt_requested = preempt_requested;
    }
}

fn queued_by_request(
    state: &InstanceState,
    instance_id: InstanceId,
    request_id: RequestId,
) -> SchedulerResult<QueuedLease> {
    let position = state
        .queue
        .iter()
        .position(|entry| entry.request_id == request_id)
        .ok_or(SchedulerError::QueueMissing)?;
    queued_at_position(state, instance_id, position)
}

fn queued_at_position(
    state: &InstanceState,
    instance_id: InstanceId,
    position: usize,
) -> SchedulerResult<QueuedLease> {
    let entry = state
        .queue
        .get(position)
        .ok_or(SchedulerError::QueueMissing)?;
    let position = u32::try_from(position)
        .ok()
        .and_then(|position| position.checked_add(1))
        .ok_or(SchedulerError::QueueSequenceOverflow)?;
    let preempt_requested = state
        .lease
        .as_ref()
        .is_some_and(|lease| entry.priority > lease.priority);
    Ok(QueuedLease {
        request_id: entry.request_id,
        instance_id,
        holder_id: entry.holder_id,
        connection_id: entry.connection_id,
        priority: entry.priority,
        position,
        deadline_monotonic_ms: entry.deadline_monotonic_ms,
        preempt_requested,
    })
}

const fn transfer_release_reason(reason: LeaseTransferReason) -> LeaseReleaseReason {
    match reason {
        LeaseTransferReason::Preempted => LeaseReleaseReason::Preempted,
        LeaseTransferReason::ExplicitRelease => LeaseReleaseReason::Explicit,
        LeaseTransferReason::Disconnect => LeaseReleaseReason::Disconnect,
        LeaseTransferReason::Expired => LeaseReleaseReason::Expired,
        LeaseTransferReason::BackendFailure => LeaseReleaseReason::BackendFailure,
        LeaseTransferReason::HostShutdown => LeaseReleaseReason::HostShutdown,
    }
}

fn validate_active_lease(
    lease: &LeaseEntry,
    submitted_token: &LeaseToken,
    connection_id: ConnectionId,
    now_monotonic_ms: u64,
) -> SchedulerResult<()> {
    if lease.token.instance_id() != submitted_token.instance_id() {
        return Err(SchedulerError::InstanceMismatch);
    }
    if lease.token.lease_id() != submitted_token.lease_id() {
        return Err(SchedulerError::LeaseMismatch);
    }
    if lease.token.holder_id() != submitted_token.holder_id() {
        return Err(SchedulerError::HolderMismatch);
    }
    if lease.connection_id != connection_id {
        return Err(SchedulerError::ConnectionMismatch);
    }
    if lease.token.expires_at_monotonic_ms() <= now_monotonic_ms {
        return Err(SchedulerError::LeaseExpired);
    }
    if lease.token != *submitted_token {
        return Err(SchedulerError::LeaseMismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
