// SPDX-License-Identifier: AGPL-3.0-only

//! One-off (to be reverted), Workflow #338 Rc evidence for the scheduler chain: a physical
//! scheduled contained run (policy dispatch on the existing fixture catalog) whose Runtime
//! process is killed while its input blocks, then a restart on the same state root. The
//! previous-epoch settlement must leave the run to `reconcile_policy_dispatches`. Copied into the
//! runtime-host test module by the one-off workflow only; every printed line starts with `RC|`.

use super::*;

fn rc_config(root: &Path, package: &[u8]) -> RuntimeHostConfig {
    RuntimeHostConfig::new(root, b"runtime-host-test-salt")
        .with_policy_inputs(PolicyInputSnapshot::new(policy_facts(), policy_resources()))
        .with_procedure_manifest(procedure_manifest_with_primary(
            package,
            vec!["after_observation".to_owned()],
        ))
        .with_io_timeout(Duration::from_millis(500))
        .with_scheduler(SchedulerConfig {
            maximum_client_heartbeat_interval_ms: 20,
            takeover_cooldown_ms: 40,
            lease_ttl_ms: 60_000,
            ..SchedulerConfig::default()
        })
}

#[test]
fn oneoff_338rc_scheduled_child_process() {
    let Ok(root) = std::env::var("ACTINGCOMMAND_RC_SCHEDULED_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let instance_id: InstanceId = serde_json::from_slice(
        &fs::read(root.join("instance.json")).expect("instance bytes"),
    )
    .expect("instance identifier");
    let package = neutral_contained_task_package(true);
    let package_path = root.join("scheduled-task.zip");
    fs::write(&package_path, &package).expect("scheduled package");
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    state.block_input.store(true, Ordering::Release);
    let host = RuntimeHost::start(
        rc_config(&root, &package),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id,
            Arc::clone(&state),
        )),
    )
    .expect("child runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("child catalog activation");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let admission = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("child policy admission");
    let PolicyDispatchAdmission::Granted { context } = admission else {
        panic!("expected a scheduled run context")
    };
    fs::write(
        root.join("scheduled-run.json"),
        serde_json::to_vec(&(context.run_id(), context.task_id())).expect("run identity"),
    )
    .expect("run identity file");
    let watched = Arc::clone(&state);
    let marker = root.join("scheduled-input-started");
    thread::spawn(move || {
        while !watched.input_started.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(10));
        }
        fs::write(&marker, b"started").expect("input marker");
    });
    let request = ContainedTaskRequest::new(
        package_path.to_string_lossy().into_owned(),
        format!("{:x}", Sha256::digest(&package)),
    )
    .expect("scheduled request");
    let _ = host.run_scheduled_contained_task(&context, &request);
    panic!("the scheduled run must block until the parent kills this child");
}

#[test]
fn oneoff_338rc_scheduled_chain_run_keeps_its_settlement() {
    let root = TempDir::new().expect("tempdir");
    let shared_instance_id = instance_id();
    fs::write(
        root.path().join("instance.json"),
        serde_json::to_vec(&shared_instance_id).expect("instance bytes"),
    )
    .expect("instance file");
    let marker = root.path().join("scheduled-input-started");
    let mut child = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "tests::oneoff_338rc::oneoff_338rc_scheduled_child_process",
            "--nocapture",
        ])
        .env("ACTINGCOMMAND_RC_SCHEDULED_ROOT", root.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn scheduled child");
    let deadline = Instant::now() + Duration::from_secs(30);
    while !marker.is_file() {
        assert!(Instant::now() < deadline, "scheduled input marker timeout");
        assert!(
            child.try_wait().expect("poll scheduled child").is_none(),
            "scheduled child exited before its input"
        );
        thread::sleep(Duration::from_millis(10));
    }
    child.kill().expect("kill scheduled child");
    let status = child.wait().expect("wait scheduled child");
    println!("RC|SCHEDULED|child killed during its blocked input status={status}");
    let (run_id, task_id): (RunId, TaskId) = serde_json::from_slice(
        &fs::read(root.path().join("scheduled-run.json")).expect("run identity bytes"),
    )
    .expect("run identity");

    let package = neutral_contained_task_package(true);
    let host = RuntimeHost::start(
        rc_config(root.path(), &package),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            shared_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("restarted runtime host");
    let mut client = TestClient::connect(&host);
    let events = projected_events(
        &mut client,
        EventQuery {
            run_id: Some(run_id),
            ..EventQuery::default()
        },
    );
    let mut recovered = 0;
    let mut admitted = 0;
    for event in &events {
        let fact = match &event.payload {
            ProjectionPayload::Full(payload) => match payload.as_ref() {
                EventPayload::Task(TaskPayload::Semantic(semantic)) => {
                    if matches!(semantic.fact(), TaskSemanticFact::PackageAdmitted { .. }) {
                        admitted += 1;
                    }
                    if let TaskSemanticFact::TerminalCommitted { failure_code, .. } =
                        semantic.fact()
                        && failure_code.as_deref() == Some("contained_task_recovered_after_restart")
                    {
                        recovered += 1;
                    }
                    serde_json::to_string(semantic.fact()).expect("fact JSON")
                }
                _ => String::from("not_a_task_fact"),
            },
            _ => String::from("-"),
        };
        println!(
            "RC|SCHEDULED|seq={}|type={}|origin={}/{}|task_link={}|fact={}",
            event.sequence,
            serde_json::to_string(&event.event_type).expect("type JSON"),
            serde_json::to_string(&event.origin.source()).expect("source JSON"),
            serde_json::to_string(&event.origin.actor()).expect("actor JSON"),
            event.links.task_id() == Some(&task_id),
            fact
        );
    }
    println!(
        "RC|SCHEDULED|run_events={}|package_admitted={admitted}|recovered_after_restart_terminals={recovered}",
        events.len()
    );
    assert_eq!(admitted, 1, "the scheduled run was admitted before the kill");
    assert_eq!(recovered, 0, "the scheduler chain is not settled by R3");
    drop(client);
    host.close().expect("close restarted host");
}
