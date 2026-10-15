// SPDX-License-Identifier: AGPL-3.0-only

//! #382 C-2 (test plan v2.2 C-2, inventory-contract I1): `ScriptedRuntime`, a scripted local
//! Runtime for client tests, behind the `test-support` feature.
//!
//! It answers the local IPC of one state root from one script per connection: the n-th
//! connection a client opens runs the n-th script, and a connection beyond the scripts is
//! recorded as unscripted. The handshake of every connection (Workflow #389 U-2: `Health`, then
//! `BindChannel`) is answered for the script. Each request step reads one request and writes a
//! receipt for it, holds it until the client closes the connection, or closes the connection
//! itself; a ledger step answers event queries from in-memory events. Gates order the script
//! against the test instead of sleeps, and every request frame a connection receives is
//! recorded. Every wait of the script ends after `SCRIPT_WAIT` with a panic, so a broken test
//! fails loud instead of hanging; no test relies on that bound.

use crate::ipc::DEFAULT_RUNTIME_MAX_FRAME_BYTES;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    EventQuery, IdentifierIssuer, OwnerEpoch, ProjectedEvent, ProjectionProfile, RUNTIME_INFO_FILE,
    RuntimeEventQueryCursor, RuntimeEventQueryPage, RuntimeEventQueryPageRequest, RuntimeInfo,
    RuntimeOperation, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest, RuntimeResult,
};
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::panic::resume_unwind;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// The longest the script waits at any one point (a request, a close, a gate): a bound for a
/// broken test, never a timing a test relies on.
pub const SCRIPT_WAIT: Duration = Duration::from_secs(120);

/// A gate one side opens and the other waits for (test plan v2.2 §2.1: gates instead of
/// sleeps). Clones share one gate; once open it stays open.
#[derive(Clone, Default)]
pub struct ScriptedGate(Arc<(Mutex<bool>, Condvar)>);

impl ScriptedGate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&self) {
        let (opened, changed) = &*self.0;
        *opened.lock().unwrap_or_else(PoisonError::into_inner) = true;
        changed.notify_all();
    }

    pub fn is_open(&self) -> bool {
        *self.0.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Returns once the gate is open; panics when it stays shut for `SCRIPT_WAIT`.
    pub fn wait(&self) {
        let (opened, changed) = &*self.0;
        let guard = opened.lock().unwrap_or_else(PoisonError::into_inner);
        let (guard, _) = changed
            .wait_timeout_while(guard, SCRIPT_WAIT, |opened| !*opened)
            .unwrap_or_else(PoisonError::into_inner);
        assert!(*guard, "scripted gate still shut after {SCRIPT_WAIT:?}");
    }
}

/// What the script does with one request it read.
pub enum ScriptedAnswer {
    /// Writes this receipt.
    Receipt(Box<RuntimeReceipt>),
    /// Waits until the gate opens, then answers.
    AfterGate(ScriptedGate, Box<ScriptedAnswer>),
    /// Writes nothing and holds the request until the client closes the connection.
    StallUntilClosed,
    /// Writes nothing and closes the connection.
    Close,
}

impl ScriptedAnswer {
    /// A completed receipt of `request` carrying `result`.
    pub fn completed(request: &RuntimeRequest, result: RuntimeResult) -> Self {
        Self::answered(request, RuntimeReceiptState::Completed, result)
    }

    /// A receipt of `request` in `state` carrying `result` (a granted lease, for example, is
    /// `admitted`).
    pub fn answered(
        request: &RuntimeRequest,
        state: RuntimeReceiptState,
        result: RuntimeResult,
    ) -> Self {
        let receipt = RuntimeReceipt::success(request, state, None, result)
            .unwrap_or_else(|error| panic!("scripted receipt: {error:?}"));
        Self::Receipt(Box::new(receipt))
    }

    /// A denied or failed receipt of `request` carrying `outcome` (Workflow #378 A4: the
    /// Runtime's code on top, its Runtime error code as the `runtime_code` value).
    pub fn failed(request: &RuntimeRequest, state: RuntimeReceiptState, outcome: Outcome) -> Self {
        let receipt = RuntimeReceipt::failure(request, state, None, outcome)
            .unwrap_or_else(|error| panic!("scripted failure receipt: {error:?}"));
        Self::Receipt(Box::new(receipt))
    }

    /// This answer, written once `gate` opens.
    pub fn after(self, gate: &ScriptedGate) -> Self {
        Self::AfterGate(gate.clone(), Box::new(self))
    }
}

/// Checks one request (it panics on a request the script does not expect) and says how to
/// answer it.
pub type ScriptedResponder = Box<dyn FnOnce(&RuntimeRequest) -> ScriptedAnswer + Send>;

/// One step of a connection's script, after its handshake.
pub enum ScriptedStep {
    /// Reads the next request and answers it as the responder says.
    Request(ScriptedResponder),
    /// Answers every event query from these events until the client closes the connection, at
    /// one snapshot: the last event's sequence. Any other request fails the script.
    Ledger(Vec<ProjectedEvent>),
    /// Opens the gate.
    Open(ScriptedGate),
}

/// The script of one accepted connection.
#[derive(Default)]
pub struct ScriptedConnection {
    /// `Health` answers this owner epoch instead of the Runtime's, and no `BindChannel` follows.
    other_epoch: Option<OwnerEpoch>,
    steps: Vec<ScriptedStep>,
}

impl ScriptedConnection {
    /// A connection whose handshake is answered in the Runtime's own owner epoch.
    pub fn new() -> Self {
        Self::default()
    }

    /// A connection whose `Health` answers `owner_epoch` instead: the address now belongs to
    /// another Runtime start. A client sends nothing more on it.
    pub fn in_other_epoch(owner_epoch: OwnerEpoch) -> Self {
        Self {
            other_epoch: Some(owner_epoch),
            steps: Vec::new(),
        }
    }

    /// Reads the next request and answers it as `responder` says.
    pub fn request(
        mut self,
        responder: impl FnOnce(&RuntimeRequest) -> ScriptedAnswer + Send + 'static,
    ) -> Self {
        self.steps.push(ScriptedStep::Request(Box::new(responder)));
        self
    }

    /// Answers every further event query from `events` until the client closes.
    pub fn ledger(mut self, events: Vec<ProjectedEvent>) -> Self {
        self.steps.push(ScriptedStep::Ledger(events));
        self
    }

    /// Opens `gate` at this point of the script.
    pub fn open(mut self, gate: &ScriptedGate) -> Self {
        self.steps.push(ScriptedStep::Open(gate.clone()));
        self
    }
}

/// Every request frame one connection received, in order, its handshake included.
#[derive(Debug, Default)]
pub struct ScriptedConnectionRecord {
    pub requests: Vec<RuntimeRequest>,
}

impl ScriptedConnectionRecord {
    /// How many of the received requests `select` selects.
    pub fn count(&self, select: impl Fn(&RuntimeOperation) -> bool) -> usize {
        self.requests
            .iter()
            .filter(|request| select(request.operation()))
            .count()
    }
}

/// What the whole script received.
#[derive(Debug)]
pub struct ScriptedRecord {
    /// One record per scripted connection, in the order they were accepted.
    pub connections: Vec<ScriptedConnectionRecord>,
    /// Connections beyond the scripts (a reconnect the script did not expect): never answered,
    /// recorded until the client closes them.
    pub unscripted: Vec<ScriptedConnectionRecord>,
}

/// A scripted Runtime on a loopback port, found through its state root's runtime-info as a
/// real one is.
pub struct ScriptedRuntime {
    address: SocketAddr,
    owner_epoch: OwnerEpoch,
    scripted: usize,
    /// The local address of `finish`'s own wake-up connection, once it is made.
    wake: Arc<Mutex<Option<SocketAddr>>>,
    acceptor: JoinHandle<Vec<Handler>>,
}

struct Handler {
    scripted: bool,
    thread: JoinHandle<ScriptedConnectionRecord>,
}

impl ScriptedRuntime {
    /// Writes `state_root`'s runtime-info for a new owner epoch on a loopback port and serves
    /// the connections `script` gives for that epoch there, one per accepted connection, in
    /// order.
    pub fn start(
        state_root: &Path,
        script: impl FnOnce(OwnerEpoch) -> Vec<ScriptedConnection>,
    ) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap_or_else(|error| panic!("scripted runtime bind: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("scripted runtime address: {error}"));
        let issuer = IdentifierIssuer::new()
            .unwrap_or_else(|error| panic!("scripted runtime identifiers: {error:?}"));
        let owner_epoch = *issuer
            .mint_owner_epoch()
            .unwrap_or_else(|error| panic!("scripted runtime owner epoch: {error:?}"))
            .transport();
        let info = RuntimeInfo::new(
            std::process::id(),
            address.ip().to_string(),
            address.port(),
            owner_epoch,
            1,
        )
        .unwrap_or_else(|error| panic!("scripted runtime info: {error:?}"));
        let info = serde_json::to_vec(&info)
            .unwrap_or_else(|error| panic!("encode the scripted runtime info: {error}"));
        fs::write(state_root.join(RUNTIME_INFO_FILE), info)
            .unwrap_or_else(|error| panic!("write the scripted runtime info: {error}"));
        let connections = script(owner_epoch);
        let scripted = connections.len();
        let wake = Arc::new(Mutex::new(None));
        let acceptor = {
            let wake = Arc::clone(&wake);
            thread::spawn(move || accept(&listener, owner_epoch, connections, &wake))
        };
        Self {
            address,
            owner_epoch,
            scripted,
            wake,
            acceptor,
        }
    }

    /// The owner epoch of the runtime-info and of each scripted handshake.
    pub const fn owner_epoch(&self) -> OwnerEpoch {
        self.owner_epoch
    }

    /// Stops accepting and returns what every connection received once each has ended. A
    /// connection ends when its client closes it, so drop the clients first. Panics with a
    /// connection's own panic (a request the script did not expect, a gate that stayed shut),
    /// and when a scripted connection never arrived.
    pub fn finish(self) -> ScriptedRecord {
        let Self {
            address,
            scripted,
            wake,
            acceptor,
            ..
        } = self;
        {
            // The acceptor checks each connection against this address under the same lock,
            // so it never takes the wake-up for a client's connection.
            let mut wake = wake.lock().unwrap_or_else(PoisonError::into_inner);
            match TcpStream::connect(address) {
                Ok(stream) => {
                    *wake = Some(stream.local_addr().unwrap_or_else(|error| {
                        panic!("scripted runtime wake-up address: {error}")
                    }));
                }
                // An acceptor that panicked dropped the listener; joining it below raises its
                // panic.
                Err(_) if acceptor.is_finished() => {}
                Err(error) => panic!("wake the scripted runtime: {error}"),
            }
        }
        let handlers = acceptor.join().unwrap_or_else(|panic| resume_unwind(panic));
        let mut record = ScriptedRecord {
            connections: Vec::new(),
            unscripted: Vec::new(),
        };
        for handler in handlers {
            let connection = handler
                .thread
                .join()
                .unwrap_or_else(|panic| resume_unwind(panic));
            if handler.scripted {
                record.connections.push(connection);
            } else {
                record.unscripted.push(connection);
            }
        }
        assert_eq!(
            record.connections.len(),
            scripted,
            "scripted connections that arrived"
        );
        record
    }
}

fn accept(
    listener: &TcpListener,
    owner_epoch: OwnerEpoch,
    connections: Vec<ScriptedConnection>,
    wake: &Mutex<Option<SocketAddr>>,
) -> Vec<Handler> {
    let mut scripts = connections.into_iter();
    let mut handlers = Vec::new();
    loop {
        let (stream, peer) = listener
            .accept()
            .unwrap_or_else(|error| panic!("scripted runtime accept: {error}"));
        if *wake.lock().unwrap_or_else(PoisonError::into_inner) == Some(peer) {
            return handlers;
        }
        let script = scripts.next();
        let scripted = script.is_some();
        let thread = thread::spawn(move || serve(stream, owner_epoch, script));
        handlers.push(Handler { scripted, thread });
    }
}

fn serve(
    stream: TcpStream,
    owner_epoch: OwnerEpoch,
    script: Option<ScriptedConnection>,
) -> ScriptedConnectionRecord {
    stream
        .set_read_timeout(Some(SCRIPT_WAIT))
        .unwrap_or_else(|error| panic!("scripted connection read bound: {error}"));
    let mut connection = Connection {
        stream,
        record: ScriptedConnectionRecord::default(),
    };
    let Some(script) = script else {
        connection.drain();
        return connection.record;
    };
    connection.handshake(
        script.other_epoch.unwrap_or(owner_epoch),
        script.other_epoch.is_none(),
    );
    let mut open = true;
    for step in script.steps {
        match step {
            ScriptedStep::Request(responder) => {
                assert!(open, "a scripted request after the connection closed");
                let request = connection
                    .next_request()
                    .unwrap_or_else(|| panic!("the client closed before a scripted request"));
                open = connection.answer(responder(&request));
            }
            ScriptedStep::Ledger(events) => {
                assert!(open, "a scripted ledger after the connection closed");
                while let Some(request) = connection.next_request() {
                    let RuntimeOperation::QueryEvents {
                        query,
                        profile,
                        page,
                    } = request.operation()
                    else {
                        panic!(
                            "the scripted ledger answers event queries only, got {:?}",
                            request.operation()
                        )
                    };
                    let page = event_page(&events, query, *profile, page);
                    connection.answer(ScriptedAnswer::completed(
                        &request,
                        RuntimeResult::EventPage { page },
                    ));
                }
                open = false;
            }
            ScriptedStep::Open(gate) => gate.open(),
        }
    }
    if open {
        connection.drain();
    }
    connection.record
}

struct Connection {
    stream: TcpStream,
    record: ScriptedConnectionRecord,
}

impl Connection {
    /// `Health` answers `owner_epoch`; a bound handshake then binds the channel `BindChannel`
    /// asks for (Workflow #389 U-2).
    fn handshake(&mut self, owner_epoch: OwnerEpoch, bind: bool) {
        let health = self
            .next_request()
            .unwrap_or_else(|| panic!("the client closed before its handshake"));
        assert!(
            matches!(health.operation(), RuntimeOperation::Health),
            "a handshake starts with Health, got {:?}",
            health.operation()
        );
        self.answer(ScriptedAnswer::completed(
            &health,
            RuntimeResult::Health { owner_epoch },
        ));
        if !bind {
            return;
        }
        let request = self
            .next_request()
            .unwrap_or_else(|| panic!("the client closed before BindChannel"));
        let RuntimeOperation::BindChannel { channel, via } = request.operation() else {
            panic!(
                "BindChannel follows Health in a handshake, got {:?}",
                request.operation()
            )
        };
        let result = RuntimeResult::ChannelBound {
            channel: *channel,
            via: *via,
        };
        self.answer(ScriptedAnswer::completed(&request, result));
    }

    /// Answers the request just read; false once the connection is closed.
    fn answer(&mut self, answer: ScriptedAnswer) -> bool {
        match answer {
            ScriptedAnswer::Receipt(receipt) => {
                self.write(&receipt);
                true
            }
            ScriptedAnswer::AfterGate(gate, answer) => {
                gate.wait();
                self.answer(*answer)
            }
            ScriptedAnswer::StallUntilClosed => {
                self.drain();
                false
            }
            ScriptedAnswer::Close => {
                match self.stream.shutdown(Shutdown::Write) {
                    Ok(()) => {}
                    // The client already closed its end.
                    Err(error) if closed(&error) || error.kind() == ErrorKind::NotConnected => {}
                    Err(error) => panic!("close a scripted connection: {error}"),
                }
                self.drain();
                false
            }
        }
    }

    fn write(&mut self, receipt: &RuntimeReceipt) {
        let body = serde_json::to_vec(receipt)
            .unwrap_or_else(|error| panic!("encode a scripted receipt: {error}"));
        let length = u32::try_from(body.len())
            .unwrap_or_else(|error| panic!("scripted receipt length: {error}"));
        let mut frame = Vec::with_capacity(4 + body.len());
        frame.extend_from_slice(&length.to_be_bytes());
        frame.extend_from_slice(&body);
        self.stream
            .write_all(&frame)
            .and_then(|()| self.stream.flush())
            .unwrap_or_else(|error| panic!("write a scripted receipt: {error}"));
    }

    /// Records every further request until the client closes the connection, answering none.
    fn drain(&mut self) {
        while self.next_request().is_some() {}
    }

    /// The next request, recorded; `None` once the client closed the connection.
    fn next_request(&mut self) -> Option<RuntimeRequest> {
        let mut header = [0_u8; 4];
        match self.stream.read_exact(&mut header) {
            Ok(()) => {}
            Err(error) if closed(&error) => return None,
            Err(error) if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {
                panic!("no request and no close within {SCRIPT_WAIT:?}")
            }
            Err(error) => panic!("read a request header: {error}"),
        }
        let length = usize::try_from(u32::from_be_bytes(header))
            .unwrap_or_else(|error| panic!("request length: {error}"));
        assert!(
            length > 0 && length <= DEFAULT_RUNTIME_MAX_FRAME_BYTES,
            "request frame of {length} bytes"
        );
        let mut body = vec![0_u8; length];
        self.stream
            .read_exact(&mut body)
            .unwrap_or_else(|error| panic!("read a request body: {error}"));
        let request: RuntimeRequest = serde_json::from_slice(&body)
            .unwrap_or_else(|error| panic!("decode a request: {error}"));
        self.record.requests.push(request.clone());
        Some(request)
    }
}

/// The peer closed or reset the connection.
fn closed(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
    )
}

/// One page of the events `query` matches after the request's cursor, oldest first, at the one
/// snapshot of `events` (its last sequence).
fn event_page(
    events: &[ProjectedEvent],
    query: &EventQuery,
    profile: ProjectionProfile,
    request: &RuntimeEventQueryPageRequest,
) -> RuntimeEventQueryPage {
    let snapshot = events.last().map_or(0, |event| event.sequence);
    if let Some(position) = request.snapshot_position() {
        assert_eq!(position, snapshot, "a read at another snapshot");
    }
    let after = request
        .cursor()
        .map_or(0, RuntimeEventQueryCursor::after_sequence);
    let mut matching = events
        .iter()
        .filter(|event| event.sequence > after && matches(query, event));
    let rows = matching
        .by_ref()
        .take(usize::from(request.limit()))
        .cloned()
        .collect::<Vec<_>>();
    let has_more = matching.next().is_some();
    let next_cursor = rows.last().filter(|_| has_more).map(|last| {
        RuntimeEventQueryCursor::new(snapshot, last.sequence, query, profile)
            .unwrap_or_else(|error| panic!("scripted next cursor: {error:?}"))
    });
    RuntimeEventQueryPage::new(rows, snapshot, request.limit(), has_more, next_cursor)
        .unwrap_or_else(|error| panic!("scripted event page: {error:?}"))
}

/// Whether `event` matches `query`. The scripted ledger serves the time and sequence bounds, one
/// event type and the links; a query with any other condition fails the script.
fn matches(query: &EventQuery, event: &ProjectedEvent) -> bool {
    let unserved = EventQuery {
        from_timestamp_unix_ms: None,
        to_timestamp_unix_ms: None,
        from_sequence: None,
        to_sequence: None,
        event_type: None,
        instance_id: None,
        request_id: None,
        correlation_id: None,
        task_id: None,
        run_id: None,
        lease_id: None,
        ..query.clone()
    };
    assert_eq!(
        unserved,
        EventQuery::default(),
        "the scripted ledger serves no such condition"
    );
    let links = &event.links;
    query
        .from_timestamp_unix_ms
        .is_none_or(|from| event.timestamp_unix_ms >= from)
        && query
            .to_timestamp_unix_ms
            .is_none_or(|to| event.timestamp_unix_ms <= to)
        && query
            .from_sequence
            .is_none_or(|from| event.sequence >= from)
        && query.to_sequence.is_none_or(|to| event.sequence <= to)
        && query
            .event_type
            .is_none_or(|event_type| event.event_type == event_type)
        && query
            .instance_id
            .is_none_or(|id| links.instance_id() == Some(&id))
        && query
            .request_id
            .is_none_or(|id| links.request_id() == Some(&id))
        && query
            .correlation_id
            .is_none_or(|id| links.correlation_id() == Some(&id))
        && query.task_id.is_none_or(|id| links.task_id() == Some(&id))
        && query.run_id.is_none_or(|id| links.run_id() == Some(&id))
        && query
            .lease_id
            .is_none_or(|id| links.lease_id() == Some(&id))
}
