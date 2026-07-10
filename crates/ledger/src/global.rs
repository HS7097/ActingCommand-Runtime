// SPDX-License-Identifier: AGPL-3.0-only

//! Recoverable single-writer storage for the global Runtime event ledger.

mod projection;
mod storage;

use crate::PersistedEvent;
use actingcommand_contract::{
    EventQuery, ProjectedEvent, ProjectionProfile, SanitizationError, SanitizedEventDraft,
    SecretField, SecretFingerprinter, Sha256Fingerprint, SubscriptionCursor,
};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Weak,
    mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use storage::SegmentStore;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_SEGMENT_MAX_BYTES: u64 = 16 * 1024 * 1024;
const DEFAULT_INGRESS_CAPACITY: usize = 256;
const DEFAULT_SUBSCRIPTION_CAPACITY: usize = 64;
const DEFAULT_REPLAY_PAGE_EVENTS: usize = 256;
const MAX_REPLAY_PAGE_EVENTS: usize = 1024;

pub type GlobalLedgerResult<T> = Result<T, GlobalLedgerError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalLedgerError {
    code: &'static str,
    operation: &'static str,
    detail: Option<String>,
    terminal: bool,
}

impl GlobalLedgerError {
    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn operation(&self) -> &'static str {
        self.operation
    }

    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    pub fn is_fatal(&self) -> bool {
        self.terminal
    }

    fn fatal(code: &'static str, operation: &'static str) -> Self {
        Self {
            code,
            operation,
            detail: None,
            terminal: true,
        }
    }

    fn request(code: &'static str, operation: &'static str) -> Self {
        Self {
            code,
            operation,
            detail: None,
            terminal: false,
        }
    }

    fn io(code: &'static str, operation: &'static str, error: &std::io::Error) -> Self {
        Self {
            code,
            operation,
            detail: Some(error.to_string()),
            terminal: true,
        }
    }

    fn json(code: &'static str, operation: &'static str, error: &serde_json::Error) -> Self {
        Self {
            code,
            operation,
            detail: Some(format!("line {}, column {}", error.line(), error.column())),
            terminal: true,
        }
    }

    fn terminal(&self) -> bool {
        self.terminal
    }
}

impl fmt::Display for GlobalLedgerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "global ledger error {} during {}",
            self.code, self.operation
        )
    }
}

impl Error for GlobalLedgerError {}

#[derive(Clone)]
pub struct GlobalLedgerConfig {
    root: PathBuf,
    owner_id: String,
    segment_max_bytes: u64,
    ingress_capacity: usize,
    subscription_capacity: usize,
}

impl fmt::Debug for GlobalLedgerConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GlobalLedgerConfig")
            .field("root", &"<redacted-root>")
            .field("owner_id", &"<redacted-owner-id>")
            .field("segment_max_bytes", &self.segment_max_bytes)
            .field("ingress_capacity", &self.ingress_capacity)
            .field("subscription_capacity", &self.subscription_capacity)
            .finish()
    }
}

impl GlobalLedgerConfig {
    pub fn new(root: impl AsRef<Path>, owner_id: impl Into<String>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            owner_id: owner_id.into(),
            segment_max_bytes: DEFAULT_SEGMENT_MAX_BYTES,
            ingress_capacity: DEFAULT_INGRESS_CAPACITY,
            subscription_capacity: DEFAULT_SUBSCRIPTION_CAPACITY,
        }
    }

    pub fn with_segment_max_bytes(mut self, bytes: u64) -> Self {
        self.segment_max_bytes = bytes;
        self
    }

    pub fn with_ingress_capacity(mut self, capacity: usize) -> Self {
        self.ingress_capacity = capacity;
        self
    }

    pub fn with_subscription_capacity(mut self, capacity: usize) -> Self {
        self.subscription_capacity = capacity;
        self
    }

    fn validate(&self) -> GlobalLedgerResult<()> {
        if self.root.as_os_str().is_empty() {
            return Err(GlobalLedgerError::fatal(
                "invalid_ledger_config",
                "validate_root",
            ));
        }
        if !is_identifier(&self.owner_id) {
            return Err(GlobalLedgerError::fatal(
                "invalid_ledger_config",
                "validate_owner_id",
            ));
        }
        if self.segment_max_bytes < 128 {
            return Err(GlobalLedgerError::fatal(
                "invalid_ledger_config",
                "validate_segment_size",
            ));
        }
        if self.ingress_capacity == 0 {
            return Err(GlobalLedgerError::fatal(
                "invalid_ledger_config",
                "validate_ingress_capacity",
            ));
        }
        if self.subscription_capacity == 0 {
            return Err(GlobalLedgerError::fatal(
                "invalid_ledger_config",
                "validate_subscription_capacity",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubscriptionOptions {
    replay_page_events: usize,
}

impl SubscriptionOptions {
    pub fn new(replay_page_events: usize) -> GlobalLedgerResult<Self> {
        if !(1..=MAX_REPLAY_PAGE_EVENTS).contains(&replay_page_events) {
            return Err(GlobalLedgerError::fatal(
                "invalid_replay_page_size",
                "configure_subscription",
            ));
        }
        Ok(Self { replay_page_events })
    }
}

impl Default for SubscriptionOptions {
    fn default() -> Self {
        Self {
            replay_page_events: DEFAULT_REPLAY_PAGE_EVENTS,
        }
    }
}

#[derive(Clone)]
pub struct Sha256SecretFingerprinter {
    private_salt: Vec<u8>,
}

impl fmt::Debug for Sha256SecretFingerprinter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Sha256SecretFingerprinter")
            .field("private_salt", &"<redacted>")
            .finish()
    }
}

impl Sha256SecretFingerprinter {
    pub fn new(salt: impl AsRef<[u8]>) -> GlobalLedgerResult<Self> {
        let salt = salt.as_ref();
        if salt.is_empty() {
            return Err(GlobalLedgerError::fatal(
                "invalid_redactor_config",
                "configure_redactor",
            ));
        }
        Ok(Self {
            private_salt: salt.to_vec(),
        })
    }
}

impl SecretFingerprinter for Sha256SecretFingerprinter {
    fn fingerprint(
        &self,
        field: SecretField,
        original: &str,
    ) -> Result<Sha256Fingerprint, SanitizationError> {
        if self.private_salt.is_empty() {
            return Err(SanitizationError::fingerprinter_failure());
        }
        let mut digest = Sha256::new();
        digest.update(&self.private_salt);
        digest.update([0]);
        digest.update(match field {
            SecretField::AccountIdentity => b"account_identity".as_slice(),
            SecretField::AuthenticationMaterial => b"authentication_material".as_slice(),
        });
        digest.update([0]);
        digest.update(original.as_bytes());
        Sha256Fingerprint::new(format!("sha256:{:x}", digest.finalize()), original)
    }
}

enum WriterCommand {
    Append {
        draft: Box<SanitizedEventDraft>,
        response: SyncSender<GlobalLedgerResult<PersistedEvent>>,
    },
    Query {
        query: EventQuery,
        response: SyncSender<GlobalLedgerResult<Vec<PersistedEvent>>>,
    },
    Subscribe {
        cursor: SubscriptionCursor,
        response: SyncSender<GlobalLedgerResult<SubscriptionRegistration>>,
    },
    ReplayPage {
        after_sequence: u64,
        through_sequence: u64,
        page_events: usize,
        response: SyncSender<GlobalLedgerResult<Vec<PersistedEvent>>>,
    },
    Project {
        query: EventQuery,
        profile: ProjectionProfile,
        response: SyncSender<GlobalLedgerResult<Vec<ProjectedEvent>>>,
    },
    Shutdown {
        response: SyncSender<GlobalLedgerResult<()>>,
    },
    #[cfg(test)]
    TestTerminalFailure { error: GlobalLedgerError },
    #[cfg(test)]
    TestSubscriberCount { response: SyncSender<usize> },
}

pub struct GlobalLedger {
    sender: Option<SyncSender<WriterCommand>>,
    writer: Option<JoinHandle<GlobalLedgerResult<()>>>,
}

pub struct LedgerSubscription {
    sender: SyncSender<WriterCommand>,
    replay: VecDeque<PersistedEvent>,
    replay_fetch_after_sequence: u64,
    last_delivered_sequence: u64,
    replay_through_sequence: u64,
    replay_page_events: usize,
    live: Receiver<PersistedEvent>,
    terminal: Receiver<GlobalLedgerError>,
    terminal_error: Option<GlobalLedgerError>,
    liveness: Option<Arc<()>>,
}

impl LedgerSubscription {
    pub fn recv_timeout(&mut self, timeout: Duration) -> GlobalLedgerResult<PersistedEvent> {
        loop {
            self.check_terminal()?;
            if let Some(event) = self.replay.pop_front() {
                self.check_terminal()?;
                return self.deliver(event);
            }
            if self.replay_fetch_after_sequence < self.replay_through_sequence {
                self.fetch_replay_page()?;
                continue;
            }
            let result = self.live.recv_timeout(timeout);
            self.check_terminal()?;
            return match result {
                Ok(event) => self.deliver(event),
                Err(mpsc::RecvTimeoutError::Timeout) => Err(GlobalLedgerError::request(
                    "subscription_timeout",
                    "receive_subscription",
                )),
                Err(mpsc::RecvTimeoutError::Disconnected) => Err(self.latch_after_terminal_check(
                    GlobalLedgerError::fatal("writer_unavailable", "receive_subscription"),
                )),
            };
        }
    }

    pub fn resume_cursor(&self) -> SubscriptionCursor {
        SubscriptionCursor {
            after_sequence: self.last_delivered_sequence,
        }
    }

    pub const fn replay_through_sequence(&self) -> u64 {
        self.replay_through_sequence
    }

    fn fetch_replay_page(&mut self) -> GlobalLedgerResult<()> {
        self.check_terminal()?;
        let after_sequence = self.replay_fetch_after_sequence;
        let (response, receiver) = mpsc::sync_channel(1);
        if let Err(error) = send_command(
            &self.sender,
            WriterCommand::ReplayPage {
                after_sequence,
                through_sequence: self.replay_through_sequence,
                page_events: self.replay_page_events,
                response,
            },
            "replay_subscription",
        ) {
            return Err(self.latch_after_terminal_check(error));
        }
        let page = match receive_response(receiver, "replay_subscription") {
            Ok(Ok(page)) => page,
            Ok(Err(error)) | Err(error) => {
                return Err(self.latch_after_terminal_check(error));
            }
        };
        self.check_terminal()?;
        if page.is_empty() || page.len() > self.replay_page_events {
            return Err(self.latch_terminal(GlobalLedgerError::fatal(
                "subscription_replay_invalid",
                "validate_replay_page",
            )));
        }
        let mut expected_sequence = after_sequence.checked_add(1).ok_or_else(|| {
            self.latch_terminal(GlobalLedgerError::fatal(
                "subscription_replay_invalid",
                "validate_replay_page",
            ))
        })?;
        for event in &page {
            if event.sequence() != expected_sequence
                || event.sequence() > self.replay_through_sequence
            {
                return Err(self.latch_terminal(GlobalLedgerError::fatal(
                    "subscription_replay_invalid",
                    "validate_replay_page",
                )));
            }
            expected_sequence = expected_sequence.saturating_add(1);
        }
        self.replay_fetch_after_sequence = page
            .last()
            .expect("validated replay page is non-empty")
            .sequence();
        self.replay.extend(page);
        Ok(())
    }

    fn deliver(&mut self, event: PersistedEvent) -> GlobalLedgerResult<PersistedEvent> {
        let expected_sequence = self.last_delivered_sequence.checked_add(1).ok_or_else(|| {
            self.latch_terminal(GlobalLedgerError::fatal(
                "subscription_sequence_invalid",
                "deliver_subscription",
            ))
        })?;
        if event.sequence() != expected_sequence {
            return Err(self.latch_terminal(GlobalLedgerError::fatal(
                "subscription_sequence_invalid",
                "deliver_subscription",
            )));
        }
        self.last_delivered_sequence = event.sequence();
        Ok(event)
    }

    fn check_terminal(&mut self) -> GlobalLedgerResult<()> {
        if let Some(error) = &self.terminal_error {
            return Err(error.clone());
        }
        match self.terminal.try_recv() {
            Ok(error) => Err(self.latch_terminal(error)),
            Err(TryRecvError::Empty) => Ok(()),
            Err(TryRecvError::Disconnected) => Err(self.latch_terminal(GlobalLedgerError::fatal(
                "writer_unavailable",
                "receive_subscription_terminal",
            ))),
        }
    }

    fn latch_after_terminal_check(&mut self, fallback: GlobalLedgerError) -> GlobalLedgerError {
        match self.check_terminal() {
            Err(error) => error,
            Ok(()) => self.latch_terminal(fallback),
        }
    }

    fn latch_terminal(&mut self, error: GlobalLedgerError) -> GlobalLedgerError {
        self.replay.clear();
        while self.live.try_recv().is_ok() {}
        self.liveness.take();
        self.terminal_error = Some(error.clone());
        error
    }
}

struct SubscriptionRegistration {
    replay_through_sequence: u64,
    live: Receiver<PersistedEvent>,
    terminal: Receiver<GlobalLedgerError>,
    liveness: Arc<()>,
}

struct ActiveSubscription {
    after_sequence: u64,
    live: SyncSender<PersistedEvent>,
    terminal: SyncSender<GlobalLedgerError>,
    liveness: Weak<()>,
}

impl fmt::Debug for GlobalLedger {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GlobalLedger")
            .field("writer_alive", &self.writer.is_some())
            .finish()
    }
}

impl GlobalLedger {
    pub fn open(config: GlobalLedgerConfig) -> GlobalLedgerResult<Self> {
        Self::open_with_store(config, SegmentStore::open)
    }

    fn open_with_store<F>(config: GlobalLedgerConfig, open_store: F) -> GlobalLedgerResult<Self>
    where
        F: FnOnce(GlobalLedgerConfig) -> GlobalLedgerResult<SegmentStore>,
    {
        config.validate()?;
        let capacity = config.ingress_capacity;
        let subscription_capacity = config.subscription_capacity;
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let (store_sender, store_receiver) = mpsc::sync_channel(0);
        let writer = thread::Builder::new()
            .name("actingcommand-global-ledger".to_string())
            .spawn(move || {
                let store = store_receiver.recv().map_err(|_| {
                    GlobalLedgerError::fatal("writer_start_cancelled", "start_writer")
                })?;
                writer_loop(store, receiver, subscription_capacity)
            })
            .map_err(|error| {
                GlobalLedgerError::io("writer_spawn_failed", "spawn_writer", &error)
            })?;

        let store = match open_store(config) {
            Ok(store) => store,
            Err(store_error) => {
                drop(store_sender);
                drop(sender);
                join_cancelled_writer(writer)?;
                return Err(store_error);
            }
        };
        match store_sender.send(store) {
            Ok(()) => Ok(Self {
                sender: Some(sender),
                writer: Some(writer),
            }),
            Err(mpsc::SendError(store)) => {
                drop(sender);
                let close_result = store.close();
                let join_result = join_cancelled_writer(writer);
                close_result?;
                join_result?;
                Err(GlobalLedgerError::fatal(
                    "writer_unavailable",
                    "start_writer",
                ))
            }
        }
    }

    pub fn append(&self, draft: SanitizedEventDraft) -> GlobalLedgerResult<PersistedEvent> {
        let (response, receiver) = mpsc::sync_channel(1);
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| GlobalLedgerError::fatal("writer_unavailable", "append_event"))?;
        send_command(
            sender,
            WriterCommand::Append {
                draft: Box::new(draft),
                response,
            },
            "append_event",
        )?;
        receive_response(receiver, "append_event")?
    }

    pub fn query(&self, query: EventQuery) -> GlobalLedgerResult<Vec<PersistedEvent>> {
        let (response, receiver) = mpsc::sync_channel(1);
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| GlobalLedgerError::fatal("writer_unavailable", "query_events"))?;
        send_command(
            sender,
            WriterCommand::Query { query, response },
            "query_events",
        )?;
        receive_response(receiver, "query_events")?
    }

    pub fn subscribe(&self, cursor: SubscriptionCursor) -> GlobalLedgerResult<LedgerSubscription> {
        self.subscribe_with_options(cursor, SubscriptionOptions::default())
    }

    pub fn subscribe_with_options(
        &self,
        cursor: SubscriptionCursor,
        options: SubscriptionOptions,
    ) -> GlobalLedgerResult<LedgerSubscription> {
        let (response, receiver) = mpsc::sync_channel(1);
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| GlobalLedgerError::fatal("writer_unavailable", "subscribe_events"))?;
        send_command(
            sender,
            WriterCommand::Subscribe { cursor, response },
            "subscribe_events",
        )?;
        let registration = receive_response(receiver, "subscribe_events")??;
        Ok(LedgerSubscription {
            sender: sender.clone(),
            replay: VecDeque::new(),
            replay_fetch_after_sequence: cursor.after_sequence,
            last_delivered_sequence: cursor.after_sequence,
            replay_through_sequence: registration.replay_through_sequence,
            replay_page_events: options.replay_page_events,
            live: registration.live,
            terminal: registration.terminal,
            terminal_error: None,
            liveness: Some(registration.liveness),
        })
    }

    pub fn project(
        &self,
        query: EventQuery,
        profile: ProjectionProfile,
    ) -> GlobalLedgerResult<Vec<ProjectedEvent>> {
        let (response, receiver) = mpsc::sync_channel(1);
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| GlobalLedgerError::fatal("writer_unavailable", "project_events"))?;
        send_command(
            sender,
            WriterCommand::Project {
                query,
                profile,
                response,
            },
            "project_events",
        )?;
        receive_response(receiver, "project_events")?
    }

    pub fn close(mut self) -> GlobalLedgerResult<()> {
        self.shutdown()
    }

    fn shutdown(&mut self) -> GlobalLedgerResult<()> {
        let Some(writer) = self.writer.take() else {
            return Ok(());
        };
        let Some(sender) = self.sender.take() else {
            return writer
                .join()
                .map_err(|_| GlobalLedgerError::fatal("writer_panicked", "join_writer"))?;
        };
        let (response, receiver) = mpsc::sync_channel(1);
        let send_result = sender
            .send(WriterCommand::Shutdown { response })
            .map_err(|_| GlobalLedgerError::fatal("writer_unavailable", "shutdown_writer"));
        drop(sender);
        let response_result =
            send_result.and_then(|()| receive_response(receiver, "shutdown_writer")?);
        let join_result = writer
            .join()
            .map_err(|_| GlobalLedgerError::fatal("writer_panicked", "join_writer"))?;
        response_result.and(join_result)
    }
}

fn join_cancelled_writer(writer: JoinHandle<GlobalLedgerResult<()>>) -> GlobalLedgerResult<()> {
    match writer.join() {
        Ok(Err(error)) if error.code() == "writer_start_cancelled" => Ok(()),
        Ok(Ok(())) => Err(GlobalLedgerError::fatal(
            "writer_unavailable",
            "join_cancelled_writer",
        )),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(GlobalLedgerError::fatal(
            "writer_panicked",
            "join_cancelled_writer",
        )),
    }
}

impl Drop for GlobalLedger {
    fn drop(&mut self) {
        if self.writer.is_none() {
            return;
        }
        if let Err(error) = self.shutdown()
            && !thread::panicking()
        {
            panic!("{error}");
        }
    }
}

fn writer_loop(
    mut store: SegmentStore,
    receiver: Receiver<WriterCommand>,
    subscription_capacity: usize,
) -> GlobalLedgerResult<()> {
    let mut subscribers = Vec::new();
    while let Ok(command) = receiver.recv() {
        match command {
            WriterCommand::Append { draft, response } => {
                let result = store.append(*draft);
                let terminal = result.as_ref().is_err_and(GlobalLedgerError::terminal);
                if let Ok(event) = &result {
                    let _ = response.send(Ok(event.clone()));
                    deliver_live_event(&mut subscribers, event);
                }
                if terminal {
                    let error = result.expect_err("terminal append result must be an error");
                    notify_terminal_failure(&mut subscribers, error.clone());
                    let _ = response.send(Err(error.clone()));
                    return Err(error);
                }
                if let Err(error) = result {
                    let _ = response.send(Err(error));
                }
            }
            WriterCommand::Query { query, response } => {
                let _ = response.send(Ok(store.query(&query)));
            }
            WriterCommand::Subscribe { cursor, response } => {
                let replay_through_sequence = store.latest_sequence();
                let (live, live_receiver) = mpsc::sync_channel(subscription_capacity);
                let (terminal, terminal_receiver) = mpsc::sync_channel(1);
                let liveness = Arc::new(());
                let registration = SubscriptionRegistration {
                    replay_through_sequence,
                    live: live_receiver,
                    terminal: terminal_receiver,
                    liveness: Arc::clone(&liveness),
                };
                if response.send(Ok(registration)).is_ok() {
                    subscribers.push(ActiveSubscription {
                        after_sequence: cursor.after_sequence.max(replay_through_sequence),
                        live,
                        terminal,
                        liveness: Arc::downgrade(&liveness),
                    });
                }
            }
            WriterCommand::ReplayPage {
                after_sequence,
                through_sequence,
                page_events,
                response,
            } => {
                let result = if (1..=MAX_REPLAY_PAGE_EVENTS).contains(&page_events) {
                    Ok(store.replay_page(after_sequence, through_sequence, page_events))
                } else {
                    Err(GlobalLedgerError::fatal(
                        "invalid_replay_page_size",
                        "replay_subscription",
                    ))
                };
                let _ = response.send(result);
            }
            WriterCommand::Project {
                query,
                profile,
                response,
            } => {
                let projected = store
                    .query(&query)
                    .iter()
                    .map(|event| projection::project(event, profile))
                    .collect();
                let _ = response.send(Ok(projected));
            }
            WriterCommand::Shutdown { response } => {
                let result = store.close();
                match result {
                    Ok(()) => {
                        notify_terminal_failure(
                            &mut subscribers,
                            GlobalLedgerError::request("subscription_closed", "shutdown_writer"),
                        );
                        let _ = response.send(Ok(()));
                        return Ok(());
                    }
                    Err(error) => {
                        notify_terminal_failure(&mut subscribers, error.clone());
                        let _ = response.send(Err(error.clone()));
                        return Err(error);
                    }
                }
            }
            #[cfg(test)]
            WriterCommand::TestTerminalFailure { error } => {
                notify_terminal_failure(&mut subscribers, error.clone());
                return Err(error);
            }
            #[cfg(test)]
            WriterCommand::TestSubscriberCount { response } => {
                let _ = response.send(subscribers.len());
            }
        }
    }
    let result = store.close();
    match &result {
        Ok(()) => notify_terminal_failure(
            &mut subscribers,
            GlobalLedgerError::request("subscription_closed", "close_writer"),
        ),
        Err(error) => notify_terminal_failure(&mut subscribers, error.clone()),
    }
    result
}

fn deliver_live_event(subscribers: &mut Vec<ActiveSubscription>, event: &PersistedEvent) {
    subscribers.retain(|subscriber| {
        if subscriber.liveness.upgrade().is_none() {
            return false;
        }
        if event.sequence() <= subscriber.after_sequence {
            return true;
        }
        match subscriber.live.try_send(event.clone()) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                let _ = subscriber.terminal.try_send(GlobalLedgerError::fatal(
                    "subscription_lagged",
                    "deliver_subscription",
                ));
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    });
}

fn notify_terminal_failure(subscribers: &mut Vec<ActiveSubscription>, error: GlobalLedgerError) {
    subscribers.retain(|subscriber| subscriber.terminal.try_send(error.clone()).is_ok());
}

fn send_command(
    sender: &SyncSender<WriterCommand>,
    command: WriterCommand,
    operation: &'static str,
) -> GlobalLedgerResult<()> {
    match sender.try_send(command) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => Err(GlobalLedgerError::fatal("ingress_full", operation)),
        Err(TrySendError::Disconnected(_)) => {
            Err(GlobalLedgerError::fatal("writer_unavailable", operation))
        }
    }
}

fn receive_response<T>(
    receiver: Receiver<GlobalLedgerResult<T>>,
    operation: &'static str,
) -> GlobalLedgerResult<GlobalLedgerResult<T>> {
    match receiver.recv_timeout(COMMAND_TIMEOUT) {
        Ok(result) => Ok(result),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(GlobalLedgerError::fatal(
            "writer_response_timeout",
            operation,
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(GlobalLedgerError::fatal("writer_unavailable", operation))
        }
    }
}

fn is_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod recovery_tests;

#[cfg(test)]
#[path = "global/v2_tests.rs"]
mod v2_tests;
