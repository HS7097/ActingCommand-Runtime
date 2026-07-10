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
    mpsc::{self, Receiver, SyncSender, TrySendError},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use storage::SegmentStore;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_SEGMENT_MAX_BYTES: u64 = 16 * 1024 * 1024;
const DEFAULT_INGRESS_CAPACITY: usize = 256;
const DEFAULT_SUBSCRIPTION_CAPACITY: usize = 64;

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
            .field("owner_id", &self.owner_id)
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
        response: SyncSender<GlobalLedgerResult<LedgerSubscription>>,
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
    replay: VecDeque<PersistedEvent>,
    live: Receiver<PersistedEvent>,
    terminal: Receiver<GlobalLedgerError>,
    _liveness: Arc<()>,
}

impl LedgerSubscription {
    pub fn recv_timeout(&mut self, timeout: Duration) -> GlobalLedgerResult<PersistedEvent> {
        if let Some(error) = self.terminal_error() {
            return Err(error);
        }
        if let Some(event) = self.replay.pop_front() {
            return Ok(event);
        }
        let result = match self.live.recv_timeout(timeout) {
            Ok(event) => Ok(event),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(GlobalLedgerError::request(
                "subscription_timeout",
                "receive_subscription",
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(GlobalLedgerError::request(
                "subscription_closed",
                "receive_subscription",
            )),
        };
        if let Some(error) = self.terminal_error() {
            return Err(error);
        }
        result
    }

    fn terminal_error(&self) -> Option<GlobalLedgerError> {
        self.terminal.try_recv().ok()
    }
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
        Self::open_with_store(config, COMMAND_TIMEOUT, SegmentStore::open)
    }

    fn open_with_store<F>(
        config: GlobalLedgerConfig,
        startup_timeout: Duration,
        open_store: F,
    ) -> GlobalLedgerResult<Self>
    where
        F: FnOnce(GlobalLedgerConfig) -> GlobalLedgerResult<SegmentStore> + Send + 'static,
    {
        config.validate()?;
        let capacity = config.ingress_capacity;
        let subscription_capacity = config.subscription_capacity;
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
        let writer = thread::Builder::new()
            .name("actingcommand-global-ledger".to_string())
            .spawn(move || {
                let store = match open_store(config) {
                    Ok(store) => {
                        let _ = startup_sender.send(Ok(()));
                        store
                    }
                    Err(error) => {
                        let _ = startup_sender.send(Err(error.clone()));
                        return Err(error);
                    }
                };
                writer_loop(store, receiver, subscription_capacity)
            })
            .map_err(|error| {
                GlobalLedgerError::io("writer_spawn_failed", "spawn_writer", &error)
            })?;
        match startup_receiver.recv_timeout(startup_timeout) {
            Ok(Ok(())) => Ok(Self {
                sender: Some(sender),
                writer: Some(writer),
            }),
            Ok(Err(error)) => {
                let _ = writer.join();
                Err(error)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                drop(sender);
                drop(writer);
                Err(GlobalLedgerError::fatal(
                    "writer_start_timeout",
                    "start_writer",
                ))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = writer.join();
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
        receive_response(receiver, "subscribe_events")?
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
                let replay = VecDeque::from(store.events_after(cursor.after_sequence));
                let (live, live_receiver) = mpsc::sync_channel(subscription_capacity);
                let (terminal, terminal_receiver) = mpsc::sync_channel(1);
                let liveness = Arc::new(());
                let subscription = LedgerSubscription {
                    replay,
                    live: live_receiver,
                    terminal: terminal_receiver,
                    _liveness: Arc::clone(&liveness),
                };
                if response.send(Ok(subscription)).is_ok() {
                    subscribers.push(ActiveSubscription {
                        after_sequence: cursor.after_sequence,
                        live,
                        terminal,
                        liveness: Arc::downgrade(&liveness),
                    });
                }
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
    if let Err(error) = &result {
        notify_terminal_failure(&mut subscribers, error.clone());
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
#[path = "global/v2_tests.rs"]
mod v2_tests;
