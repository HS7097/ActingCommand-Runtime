// SPDX-License-Identifier: AGPL-3.0-only

const TEST_GOVERNANCE_CAPABILITY: &str = "runtime-host-governance-test-capability";

struct ManualRuntimeClock {
    unix_ms: AtomicU64,
    monotonic_ms: AtomicU64,
    samples: AtomicU64,
}

impl ManualRuntimeClock {
    fn new(unix_ms: u64, monotonic_ms: u64) -> Self {
        Self {
            unix_ms: AtomicU64::new(unix_ms),
            monotonic_ms: AtomicU64::new(monotonic_ms),
            samples: AtomicU64::new(0),
        }
    }

    fn advance(&self, duration_ms: u64) {
        self.unix_ms.fetch_add(duration_ms, Ordering::SeqCst);
        self.monotonic_ms.fetch_add(duration_ms, Ordering::SeqCst);
    }

    fn set_unix_ms(&self, unix_ms: u64) {
        self.unix_ms.store(unix_ms, Ordering::SeqCst);
    }

    fn set_monotonic_ms(&self, monotonic_ms: u64) {
        self.monotonic_ms.store(monotonic_ms, Ordering::SeqCst);
    }

    fn samples(&self) -> u64 {
        self.samples.load(Ordering::Acquire)
    }

    fn monotonic_ms(&self) -> u64 {
        self.monotonic_ms.load(Ordering::SeqCst)
    }
}

impl RuntimeClock for ManualRuntimeClock {
    fn sample(&self) -> RuntimeHostResult<RuntimeClockSample> {
        self.samples.fetch_add(1, Ordering::AcqRel);
        Ok(RuntimeClockSample {
            unix_ms: self.unix_ms.load(Ordering::SeqCst),
            monotonic_ms: self.monotonic_ms.load(Ordering::SeqCst),
        })
    }
}

struct TestClient {
    stream: TcpStream,
    ids: IdentifierIssuer,
}

impl TestClient {
    fn connect(host: &RuntimeHost) -> Self {
        Self::connect_address(host.runtime_info().socket_addr().expect("runtime address"))
    }

    fn connect_state_root(root: &Path) -> Self {
        let bytes = fs::read(root.join(RUNTIME_INFO_FILE)).expect("runtime info bytes");
        let info: actingcommand_contract::RuntimeInfo =
            serde_json::from_slice(&bytes).expect("runtime info");
        Self::connect_address(info.socket_addr().expect("runtime address"))
    }

    fn connect_address(address: std::net::SocketAddr) -> Self {
        let stream = TcpStream::connect(address).expect("connect runtime");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout");
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .expect("write timeout");
        stream.set_nodelay(true).expect("tcp nodelay");
        Self {
            stream,
            ids: IdentifierIssuer::new().expect("identifier issuer"),
        }
    }

    fn set_receipt_read_timeout(&self) {
        self.stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("receipt timeout");
    }

    fn request(&self, operation: RuntimeOperation) -> RuntimeRequest {
        let correlation_id = self.ids.mint_correlation_id().expect("correlation id");
        self.request_with_correlation(correlation_id, operation)
    }

    fn request_with_correlation(
        &self,
        correlation_id: IssuedCorrelationId,
        operation: RuntimeOperation,
    ) -> RuntimeRequest {
        RuntimeRequest::new(
            self.ids.mint_request_id().expect("request id"),
            correlation_id,
            None,
            EventActor::Cli,
            EventSource::Cli,
            unix_ms_now().expect("wall clock"),
            operation,
        )
        .expect("runtime request")
    }

    fn agent_request(&self, operation: RuntimeOperation) -> RuntimeRequest {
        RuntimeRequest::new(
            self.ids.mint_request_id().expect("request id"),
            self.ids.mint_correlation_id().expect("correlation id"),
            None,
            EventActor::Agent,
            EventSource::Adapter,
            unix_ms_now().expect("wall clock"),
            operation,
        )
        .expect("agent runtime request")
    }

    fn governance_request(&self, operation: RuntimeOperation) -> RuntimeRequest {
        RuntimeRequest::new(
            self.ids.mint_request_id().expect("request id"),
            self.ids.mint_correlation_id().expect("correlation id"),
            None,
            EventActor::User,
            EventSource::Ui,
            unix_ms_now().expect("wall clock"),
            operation,
        )
        .expect("governance runtime request")
    }

    fn authenticate_governance(&mut self) {
        let request = self.governance_request(RuntimeOperation::AuthenticateGovernance {
            capability: TEST_GOVERNANCE_CAPABILITY.to_owned(),
        });
        let receipt = self.send(&request);
        assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
        assert!(matches!(
            receipt.result(),
            Some(RuntimeResult::GovernanceAuthenticated)
        ));
    }

    fn send(&mut self, request: &RuntimeRequest) -> RuntimeReceipt {
        self.send_result(request).unwrap_or_else(|error| {
            panic!(
                "runtime receipt: {error:?}; native read context: {}",
                error
                    .lifecycle
                    .native_detail
                    .as_deref()
                    .map_or("unavailable", |detail| detail.text()),
            )
        })
    }

    fn send_result(&mut self, request: &RuntimeRequest) -> RuntimeHostResult<RuntimeReceipt> {
        let mut read_branch = "not_read";
        let result = (|| {
            write_frame(&mut self.stream, request, DEFAULT_RUNTIME_MAX_FRAME_BYTES)?;
            read_branch = "read_error";
            let frame = match read_frame(&mut self.stream, DEFAULT_RUNTIME_MAX_FRAME_BYTES)? {
                FrameRead::Data(frame) => {
                    read_branch = "Data";
                    frame
                }
                missing @ (FrameRead::Idle | FrameRead::Closed) => {
                    read_branch = if matches!(missing, FrameRead::Idle) {
                        "Idle"
                    } else {
                        "Closed"
                    };
                    return Err(RuntimeHostError::request(
                        "test_receipt_missing",
                        "read_test_receipt",
                        RuntimeErrorCode::ProtocolInvalid,
                    ));
                }
            };
            let receipt = serde_json::from_slice::<RuntimeReceipt>(&frame).map_err(|_| {
                RuntimeHostError::request(
                    "test_receipt_invalid",
                    "read_test_receipt",
                    RuntimeErrorCode::ProtocolInvalid,
                )
            })?;
            receipt.validate().map_err(|_| {
                RuntimeHostError::request(
                    "test_receipt_invalid",
                    "read_test_receipt",
                    RuntimeErrorCode::ProtocolInvalid,
                )
            })?;
            Ok(receipt)
        })();
        result.map_err(|error: RuntimeHostError| {
            error.with_native_detail(format!(
                "frame_read={read_branch}; operation={:?}; request_id_json={:?}; correlation_id_json={:?}",
                request.operation(),
                serde_json::to_string(&request.request_id()),
                serde_json::to_string(&request.correlation_id()),
            ))
        })
    }

    fn acquire(&mut self, alias: &str) -> (RuntimeRequest, LeaseToken) {
        let request = self.request(RuntimeOperation::acquire_lease(
            alias,
            self.ids.mint_holder_id().expect("holder id"),
        ));
        let receipt = self.send(&request);
        let RuntimeResult::LeaseGranted { token } = receipt.result().expect("lease result") else {
            panic!("expected lease grant");
        };
        (request, token.clone())
    }

    fn queue(
        &mut self,
        alias: &str,
        priority: LeasePriority,
        timeout_ms: u64,
    ) -> (RuntimeRequest, LeaseQueueStatus) {
        let request = self.request(RuntimeOperation::queue_lease(
            alias,
            self.ids.mint_holder_id().expect("holder id"),
            LeaseQueuePolicy::new(priority, timeout_ms).expect("queue policy"),
        ));
        let receipt = self.send(&request);
        let RuntimeResult::LeaseQueued { status } = receipt.result().expect("queue result") else {
            panic!("expected queued lease");
        };
        (request, status.clone())
    }
}

fn instance_id() -> InstanceId {
    *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport()
}

fn runtime_request(ids: &IdentifierIssuer, operation: RuntimeOperation) -> RuntimeRequest {
    RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        unix_ms_now().expect("wall clock"),
        operation,
    )
    .expect("runtime request")
}

fn event_types_for_request(
    host: &RuntimeHost,
    ids: &IdentifierIssuer,
    connection_id: ConnectionId,
    request_id: actingcommand_contract::RequestId,
) -> Vec<EventType> {
    let mut cursor: Option<RuntimeEventQueryCursor> = None;
    let mut event_types = Vec::new();
    loop {
        let query = runtime_request(
            ids,
            RuntimeOperation::QueryEvents {
                query: EventQuery {
                    request_id: Some(request_id),
                    ..EventQuery::default()
                },
                profile: ProjectionProfile::Forensic,
                page: RuntimeEventQueryPageRequest::new(128, cursor.clone()).expect("event page"),
            },
        );
        let receipt = host
            .process_request_for_test(&query, connection_id)
            .expect("event query");
        let RuntimeResult::EventPage { page } = receipt.result().expect("events result") else {
            panic!("expected event projection");
        };
        event_types.extend(page.events().iter().map(|event| event.event_type));
        cursor = page.next_cursor().cloned();
        if !page.has_more() {
            return event_types;
        }
    }
}

fn event_types_for_correlation(
    client: &mut TestClient,
    correlation_id: actingcommand_contract::CorrelationId,
) -> Vec<EventType> {
    projected_events(
        client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    )
    .into_iter()
    .map(|event| event.event_type)
    .collect()
}

fn projected_events(
    client: &mut TestClient,
    query: EventQuery,
) -> Vec<actingcommand_contract::ProjectedEvent> {
    let mut cursor: Option<RuntimeEventQueryCursor> = None;
    let mut events = Vec::new();
    loop {
        let request = client.request(RuntimeOperation::QueryEvents {
            query: query.clone(),
            profile: ProjectionProfile::Forensic,
            page: RuntimeEventQueryPageRequest::new(128, cursor.clone()).expect("event page"),
        });
        let receipt = client.send(&request);
        let RuntimeResult::EventPage { page } = receipt.result().expect("events result") else {
            panic!("expected event projection");
        };
        events.extend_from_slice(page.events());
        cursor = page.next_cursor().cloned();
        if !page.has_more() {
            return events;
        }
    }
}

fn project_snapshot(host: &RuntimeHost, request: ProjectInterfaceRequest) -> ProjectLedgerSnapshot {
    let mut client = TestClient::connect(host);
    let request = client.request(RuntimeOperation::ProjectInterface { request });
    let receipt = client.send(&request);
    let RuntimeResult::ProjectInterface { response } = receipt.result().expect("project result")
    else {
        panic!("expected project interface response")
    };
    response
        .snapshot()
        .expect("current project snapshot")
        .clone()
}

fn projected_task_semantic_fact(
    event: &actingcommand_contract::ProjectedEvent,
) -> Option<&TaskSemanticFact> {
    match &event.payload {
        ProjectionPayload::Public(projected) => match projected.as_ref() {
            PublicEventPayload::Task(payload) => payload.task_semantic_fact(),
            _ => None,
        },
        ProjectionPayload::Full(projected) => match projected.as_ref() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => Some(payload.fact()),
            _ => None,
        },
        _ => None,
    }
}

fn config(root: &TempDir) -> RuntimeHostConfig {
    RuntimeHostConfig::new(root.path(), b"runtime-host-test-salt")
        .with_policy_inputs(PolicyInputSnapshot::new(policy_facts(), policy_resources()))
        .with_procedure_manifest(procedure_manifest())
        .with_governance_capability(TEST_GOVERNANCE_CAPABILITY)
        .with_io_timeout(Duration::from_millis(500))
        .with_scheduler(SchedulerConfig {
            maximum_client_heartbeat_interval_ms: 20,
            takeover_cooldown_ms: 40,
            lease_ttl_ms: 5_000,
            ..SchedulerConfig::default()
        })
}

fn host_with_state(root: &TempDir, alias: &str, state: Arc<FakeState>) -> RuntimeHost {
    RuntimeHost::start(
        config(root),
        Arc::new(FakeProvider::one(alias, instance_id(), state)),
    )
    .expect("runtime host")
}

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let started = Instant::now();
    while !predicate() {
        assert!(started.elapsed() < timeout, "condition timed out");
        thread::sleep(Duration::from_millis(10));
    }
}

fn assert_input_denied(client: &mut TestClient, token: LeaseToken, expected: RuntimeErrorCode) {
    let request = client.request(RuntimeOperation::Input {
        token,
        action: InputAction::Tap { x: 10, y: 20 },
    });
    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied);
    assert_eq!(receipt.error_projection().expect("denial").code, expected);
}

fn concurrent_acquire(
    mut client: TestClient,
    alias: &'static str,
    start: Arc<Barrier>,
    completed: Arc<Barrier>,
) -> thread::JoinHandle<RuntimeReceipt> {
    thread::spawn(move || {
        let request = client.request(RuntimeOperation::acquire_lease(
            alias,
            client.ids.mint_holder_id().expect("holder id"),
        ));
        start.wait();
        let receipt = client.send(&request);
        completed.wait();
        receipt
    })
}
