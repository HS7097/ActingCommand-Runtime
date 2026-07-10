// SPDX-License-Identifier: AGPL-3.0-only

//! Recoverable single-writer storage for the global Runtime event ledger.

mod storage;

use actingcommand_contract::{
    EventContractError, FieldRedactor, PersistedEvent, SanitizationError, SanitizedEventDraft,
    SanitizedPayload,
};
use sha2::{Digest, Sha256};
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use storage::SegmentStore;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_SEGMENT_MAX_BYTES: u64 = 16 * 1024 * 1024;
const DEFAULT_INGRESS_CAPACITY: usize = 256;

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
        true
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
            "global ledger fatal error {} during {}",
            self.code, self.operation
        )
    }
}

impl Error for GlobalLedgerError {}

impl From<EventContractError> for GlobalLedgerError {
    fn from(error: EventContractError) -> Self {
        let code = error.code();
        Self::request(code, "erase_sanitized_event")
    }
}

#[derive(Clone)]
pub struct GlobalLedgerConfig {
    root: PathBuf,
    owner_id: String,
    segment_max_bytes: u64,
    ingress_capacity: usize,
}

impl fmt::Debug for GlobalLedgerConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GlobalLedgerConfig")
            .field("root", &"<redacted-root>")
            .field("owner_id", &self.owner_id)
            .field("segment_max_bytes", &self.segment_max_bytes)
            .field("ingress_capacity", &self.ingress_capacity)
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
        Ok(())
    }
}

#[derive(Clone)]
pub struct Sha256FieldRedactor {
    private_salt: Vec<u8>,
}

impl fmt::Debug for Sha256FieldRedactor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Sha256FieldRedactor")
            .field("private_salt", &"<redacted>")
            .finish()
    }
}

impl Sha256FieldRedactor {
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

impl FieldRedactor for Sha256FieldRedactor {
    fn fingerprint(&self, field_name: &str, value: &str) -> Result<String, SanitizationError> {
        if self.private_salt.is_empty() {
            return Err(SanitizationError::redactor_failure());
        }
        let mut digest = Sha256::new();
        digest.update(&self.private_salt);
        digest.update([0]);
        digest.update(field_name.as_bytes());
        digest.update([0]);
        digest.update(value.as_bytes());
        Ok(format!("sha256:{:x}", digest.finalize()))
    }
}

enum WriterCommand {
    Append {
        draft: Box<actingcommand_contract::ErasedSanitizedEventDraft>,
        response: SyncSender<GlobalLedgerResult<PersistedEvent>>,
    },
    Shutdown {
        response: SyncSender<GlobalLedgerResult<()>>,
    },
}

pub struct GlobalLedger {
    sender: Option<SyncSender<WriterCommand>>,
    writer: Option<JoinHandle<GlobalLedgerResult<()>>>,
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
                writer_loop(store, receiver)
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

    pub fn append<P: SanitizedPayload>(
        &self,
        draft: SanitizedEventDraft<P>,
    ) -> GlobalLedgerResult<PersistedEvent> {
        let draft = draft.erase()?;
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
) -> GlobalLedgerResult<()> {
    while let Ok(command) = receiver.recv() {
        match command {
            WriterCommand::Append { draft, response } => {
                let result = store.append(*draft);
                let terminal = result.as_ref().is_err_and(GlobalLedgerError::terminal);
                let _ = response.send(result.clone());
                if terminal {
                    return result.map(|_| ());
                }
            }
            WriterCommand::Shutdown { response } => {
                let result = store.close();
                let _ = response.send(result.clone());
                return result;
            }
        }
    }
    store.close()
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
