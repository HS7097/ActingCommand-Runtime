// SPDX-License-Identifier: AGPL-3.0-only

use crate::{DeviceError, DeviceResult};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// A memory owner's restricted, in-memory allocation capability. It grants no device effect.
#[derive(Clone)]
pub struct FrameMemoryBudget(Arc<FrameMemoryState>);

struct FrameMemoryState {
    limit: Box<dyn Fn() -> DeviceResult<u64> + Send + Sync>,
    live: AtomicU64,
}

impl fmt::Debug for FrameMemoryBudget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrameMemoryBudget")
            .field("live", &self.live_bytes())
            .finish()
    }
}

impl FrameMemoryBudget {
    pub fn new(limit: impl Fn() -> DeviceResult<u64> + Send + Sync + 'static) -> Self {
        Self(Arc::new(FrameMemoryState {
            limit: Box::new(limit),
            live: AtomicU64::new(0),
        }))
    }

    pub fn live_bytes(&self) -> u64 {
        self.0.live.load(Ordering::Acquire)
    }

    pub fn same_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub fn reserve(&self, bytes: u64) -> DeviceResult<FrameMemoryCharge> {
        let limit = (self.0.limit)()
            .map_err(|error| error.with_frame_memory_failure(FrameMemoryFailure::BudgetSource))?;
        self.0
            .live
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                live.checked_add(bytes).filter(|total| *total <= limit)
            })
            .map_err(|_| DeviceError::frame_memory(FrameMemoryFailure::Capacity))?;
        Ok(FrameMemoryCharge {
            owner: self.clone(),
            bytes,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameMemoryFailure {
    Capacity,
    Owner,
    Accounting,
    BudgetSource,
}

/// One allocation's charge; moving it preserves the charge, cloning it is forbidden.
#[derive(Debug)]
pub struct FrameMemoryCharge {
    owner: FrameMemoryBudget,
    bytes: u64,
}

impl PartialEq for FrameMemoryCharge {
    fn eq(&self, other: &Self) -> bool {
        self.owner.same_owner(&other.owner) && self.bytes == other.bytes
    }
}

impl FrameMemoryCharge {
    pub fn owner(&self) -> &FrameMemoryBudget {
        &self.owner
    }
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn split_off(&mut self, bytes: u64) -> DeviceResult<Self> {
        self.bytes = self
            .bytes
            .checked_sub(bytes)
            .ok_or_else(|| DeviceError::frame_memory(FrameMemoryFailure::Accounting))?;
        Ok(Self {
            owner: self.owner.clone(),
            bytes,
        })
    }

    pub fn shrink_to(&mut self, bytes: u64) -> DeviceResult<()> {
        let excess = self
            .bytes
            .checked_sub(bytes)
            .ok_or_else(|| DeviceError::frame_memory(FrameMemoryFailure::Accounting))?;
        drop(self.split_off(excess)?);
        Ok(())
    }

    pub fn transfer_to(&mut self, target: &mut Self, bytes: u64) -> DeviceResult<()> {
        if !self.owner.same_owner(&target.owner) || bytes > self.bytes {
            return Err(DeviceError::frame_memory(FrameMemoryFailure::Accounting));
        }
        target.bytes = target
            .bytes
            .checked_add(bytes)
            .ok_or_else(|| DeviceError::frame_memory(FrameMemoryFailure::Accounting))?;
        self.bytes -= bytes;
        Ok(())
    }
}

impl Drop for FrameMemoryCharge {
    fn drop(&mut self) {
        // Unique, private charges cannot underflow unless accounting itself is broken.
        self.owner
            .0
            .live
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                live.checked_sub(self.bytes)
            })
            .expect("frame memory charge underflow");
    }
}
