// SPDX-License-Identifier: AGPL-3.0-only

//! §3.8 and §3.4: the preview frame identity, the latest-frame slot per instance and the
//! subscribers' one-frame mailboxes. Everything here sits under one leaf lock (rank 9): it is
//! taken under the scheduler lock when a sample publishes, and nothing is taken under it.

use crate::backend::PixelLayout;
use crate::clock::Micros;
use crate::model::PathKind;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A preview frame's validity (§3.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Validity {
    Live,
    Withdrawn,
    Stale,
}

/// The preview's own frame type: never a `Frame`, never an input frame of a session (§3.8).
#[derive(Debug, Clone, Serialize)]
pub struct PreviewFrame {
    pub instance: usize,
    pub path: PathKind,
    pub owner_epoch: u64,
    pub generation: u64,
    pub preview_sequence: u64,
    pub grant_sequence: u64,
    pub width: u32,
    pub height: u32,
    pub layout: PixelLayout,
    /// The pixels arrive bottom-up from Nemu and are flipped by the device layer; S arrives
    /// upright. The transform the viewer must apply is always `identity` here.
    pub transform: &'static str,
    pub sample_start_us: Micros,
    pub sample_end_us: Micros,
    pub published_us: Micros,
    pub stream_sequence: Option<u64>,
    pub device_pts_us: Option<u64>,
    pub received_us: Option<Micros>,
    pub validity: Validity,
    #[serde(skip)]
    pub pixels: Arc<Vec<u8>>,
}

/// What a subscriber's mailbox holds next.
#[derive(Debug, Clone)]
pub enum ViewMessage {
    Frame(Arc<PreviewFrame>),
    /// The picture is withdrawn (a gate).
    Withdrawn {
        instance: usize,
        gate: String,
    },
    /// The cell ended; a new cell may follow.
    CellEnded,
    /// The run ended.
    Ended,
}

struct Mailbox {
    instance: usize,
    pending: Option<ViewMessage>,
    /// Frames replaced before the subscriber took them (`dropped_slow_consumer`).
    dropped: u64,
    delivered: u64,
    closed: bool,
}

#[derive(Default)]
struct ViewState {
    latest: BTreeMap<usize, Arc<PreviewFrame>>,
    mailboxes: BTreeMap<u64, Mailbox>,
    producers: BTreeMap<usize, u32>,
}

/// The slots and mailboxes (leaf lock).
#[derive(Default)]
pub struct ViewHub {
    state: Mutex<ViewState>,
    changed: Condvar,
}

impl ViewHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens subscriber `subscriber`'s mailbox for `instance`.
    pub fn open_mailbox(&self, subscriber: u64, instance: usize) {
        lock(&self.state).mailboxes.insert(
            subscriber,
            Mailbox {
                instance,
                pending: None,
                dropped: 0,
                delivered: 0,
                closed: false,
            },
        );
    }

    pub fn close_mailbox(&self, subscriber: u64) {
        if let Some(mailbox) = lock(&self.state).mailboxes.get_mut(&subscriber) {
            mailbox.closed = true;
        }
        self.changed.notify_all();
    }

    /// Publishes a frame: the instance's slot keeps only it and every subscriber of the
    /// instance gets it in place of what it has not taken yet (§3.4 背压).
    pub fn publish(&self, frame: PreviewFrame) {
        let frame = Arc::new(frame);
        let mut state = lock(&self.state);
        state.latest.insert(frame.instance, Arc::clone(&frame));
        for mailbox in state.mailboxes.values_mut() {
            if mailbox.closed || mailbox.instance != frame.instance {
                continue;
            }
            if matches!(mailbox.pending, Some(ViewMessage::Frame(_))) {
                mailbox.dropped += 1;
            }
            mailbox.pending = Some(ViewMessage::Frame(Arc::clone(&frame)));
        }
        drop(state);
        self.changed.notify_all();
    }

    /// Sends `message` to every open mailbox of `instance` (all instances with `None`).
    pub fn broadcast(&self, instance: Option<usize>, message: ViewMessage) {
        let mut state = lock(&self.state);
        if let ViewMessage::Withdrawn {
            instance: target, ..
        } = &message
        {
            state.latest.remove(target);
        }
        for mailbox in state.mailboxes.values_mut() {
            if mailbox.closed || instance.is_some_and(|instance| instance != mailbox.instance) {
                continue;
            }
            mailbox.pending = Some(message.clone());
        }
        drop(state);
        self.changed.notify_all();
    }

    /// Takes subscriber `subscriber`'s pending message without waiting.
    pub fn take(&self, subscriber: u64) -> Option<ViewMessage> {
        let mut state = lock(&self.state);
        let mailbox = state.mailboxes.get_mut(&subscriber)?;
        let message = mailbox.pending.take();
        if matches!(message, Some(ViewMessage::Frame(_))) {
            mailbox.delivered += 1;
        }
        message
    }

    /// Waits (a production thread, not a clock participant) for subscriber `subscriber`'s next
    /// message; `None` once the mailbox is closed.
    pub fn wait_take(&self, subscriber: u64) -> Option<ViewMessage> {
        let mut state = lock(&self.state);
        loop {
            let mailbox = state.mailboxes.get_mut(&subscriber)?;
            if let Some(message) = mailbox.pending.take() {
                if matches!(message, ViewMessage::Frame(_)) {
                    mailbox.delivered += 1;
                }
                return Some(message);
            }
            if mailbox.closed {
                return None;
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// The newest frame of `instance`.
    pub fn latest(&self, instance: usize) -> Option<Arc<PreviewFrame>> {
        lock(&self.state).latest.get(&instance).cloned()
    }

    /// `(delivered, dropped_slow_consumer)` of a subscriber.
    pub fn counts(&self, subscriber: u64) -> Option<(u64, u64)> {
        lock(&self.state)
            .mailboxes
            .get(&subscriber)
            .map(|mailbox| (mailbox.delivered, mailbox.dropped))
    }

    /// Registers a producer of `instance`'s preview; at most one exists (§3.3.4). Returns
    /// false (and registers nothing) when one is there already.
    pub fn claim_producer(&self, instance: usize) -> bool {
        let mut state = lock(&self.state);
        let producers = state.producers.entry(instance).or_insert(0);
        if *producers > 0 {
            return false;
        }
        *producers = 1;
        true
    }

    pub fn release_producer(&self, instance: usize) {
        lock(&self.state).producers.remove(&instance);
    }

    /// The number of producers of `instance` (always 0 or 1).
    pub fn producers(&self, instance: usize) -> u32 {
        lock(&self.state)
            .producers
            .get(&instance)
            .copied()
            .unwrap_or(0)
    }
}
