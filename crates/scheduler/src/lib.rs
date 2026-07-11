// SPDX-License-Identifier: AGPL-3.0-only

//! Per-instance C3a write admission, lease lifetime, and fencing authority.

#![forbid(unsafe_code)]

use actingcommand_contract::{
    HolderId, IdentifierIssuer, InstanceId, LeaseId, LeaseToken, OwnerEpoch, RequestId,
    RuntimeErrorCode, RuntimeErrorProjection,
};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

pub const DEFAULT_MAX_CLIENT_HEARTBEAT_INTERVAL_MS: u64 = 5_000;
pub const DEFAULT_TAKEOVER_COOLDOWN_MS: u64 = 6_000;
pub const DEFAULT_LEASE_TTL_MS: u64 = 120_000;

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
}

impl SchedulerConfig {
    pub fn validate(self) -> SchedulerResult<Self> {
        if self.maximum_client_heartbeat_interval_ms == 0
            || self.takeover_cooldown_ms <= self.maximum_client_heartbeat_interval_ms
            || self.lease_ttl_ms <= self.maximum_client_heartbeat_interval_ms
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
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseReleaseReason {
    Explicit,
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
        }
    }

    pub const fn is_fatal(&self) -> bool {
        matches!(
            self,
            Self::InvalidConfig
                | Self::InvalidConnection
                | Self::IdentifierIssuance
                | Self::ExpiryOverflow
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
        }
    }
}

impl fmt::Display for SchedulerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "scheduler error {}", self.code())
    }
}

impl Error for SchedulerError {}

#[derive(Clone)]
struct LeaseEntry {
    token: LeaseToken,
    connection_id: ConnectionId,
    acquire_request_id: RequestId,
    last_renew: Option<RenewRecord>,
}

#[derive(Clone)]
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

#[derive(Default)]
struct InstanceState {
    lease: Option<LeaseEntry>,
    last_release: Option<ReleaseRecord>,
    cooldown_until_monotonic_ms: u64,
}

pub struct SeedScheduler {
    owner_epoch: OwnerEpoch,
    config: SchedulerConfig,
    lease_issuer: IdentifierIssuer,
    instances: BTreeMap<InstanceId, InstanceState>,
    lease_locations: BTreeMap<LeaseId, InstanceId>,
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
        });
        state.last_release = None;
        self.lease_locations.insert(lease_id, instance_id);
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
            LeaseReleaseReason::Explicit | LeaseReleaseReason::Expired
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
