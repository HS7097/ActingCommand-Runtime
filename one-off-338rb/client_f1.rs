// SPDX-License-Identifier: AGPL-3.0-only

//! One-off (to be reverted), Workflow #338 Rb F1 evidence against a real RuntimeHost: the
//! old-daemon branch of `resume_scheduling_expected` / `resource_target_view` is taken only when
//! the call's own frame was dropped before its receipt. Each case puts a TCP proxy in front of
//! the host that decides per request frame:
//! (1) EMULATED a9a1845d decoder: drops the connection at a frame it could not decode (a
//!     `resume_scheduling` with fields other than `scope`, any `resource_target_view`);
//! (2) the first `status` frame is held 1.5 s before it is forwarded, so a client with a 300 ms
//!     IO timeout latches its connection on that earlier call; the conditional resume then meets
//!     the already-failed connection;
//! (3) a current daemon that has not answered yet: the conditional resume frame is held 1.5 s
//!     before it is forwarded (the host then lifts the pause), so the client's receipt read times
//!     out first.
//! Copied under the runtime-client test module by the one-off workflow only; lines start `RB|`.

use super::*;
use actingcommand_contract::{SchedulingPauseExpectation, SchedulingPauseScope};
use std::net::SocketAddr;

#[derive(Clone, Copy)]
enum ProxyAction {
    Forward,
    Drop,
    HoldThenForward(Duration),
}

fn f1_read_frame(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).ok()?;
    let mut body = vec![0_u8; u32::from_be_bytes(header) as usize];
    stream.read_exact(&mut body).ok()?;
    Some(body)
}

fn f1_write_frame(stream: &mut TcpStream, body: &[u8]) -> bool {
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .and_then(|()| stream.write_all(body))
        .and_then(|()| stream.flush())
        .is_ok()
}

fn operation_of(frame: &[u8]) -> Value {
    serde_json::from_slice::<Value>(frame).expect("frame JSON")["operation"].clone()
}

/// Starts a proxy in front of `upstream` and returns a state root naming it.
fn f1_proxy(
    upstream: &RuntimeInfo,
    decide: Arc<dyn Fn(&Value) -> ProxyAction + Send + Sync>,
) -> TempDir {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind proxy");
    let address = listener.local_addr().expect("proxy address");
    let target: SocketAddr = upstream.socket_addr().expect("upstream address");
    thread::spawn(move || {
        for incoming in listener.incoming() {
            let Ok(mut client) = incoming else { return };
            let decide = Arc::clone(&decide);
            thread::spawn(move || {
                let Ok(mut runtime) = TcpStream::connect(target) else {
                    return;
                };
                while let Some(frame) = f1_read_frame(&mut client) {
                    match decide(&operation_of(&frame)) {
                        ProxyAction::Drop => {
                            let _ = client.shutdown(std::net::Shutdown::Both);
                            let _ = runtime.shutdown(std::net::Shutdown::Both);
                            return;
                        }
                        ProxyAction::HoldThenForward(hold) => thread::sleep(hold),
                        ProxyAction::Forward => {}
                    }
                    if !f1_write_frame(&mut runtime, &frame) {
                        return;
                    }
                    let Some(receipt) = f1_read_frame(&mut runtime) else {
                        return;
                    };
                    if !f1_write_frame(&mut client, &receipt) {
                        return;
                    }
                }
            });
        }
    });
    let root = TempDir::new().expect("proxy state root");
    let info = RuntimeInfo::new(
        upstream.pid(),
        address.ip().to_string(),
        address.port(),
        upstream.owner_epoch(),
        upstream.started_at_unix_ms(),
    )
    .expect("proxy runtime info");
    fs::write(
        root.path().join(RUNTIME_INFO_FILE),
        serde_json::to_vec(&info).expect("proxy info JSON"),
    )
    .expect("write proxy runtime info");
    root
}

fn f1_error(label: &str, error: &RuntimeClientError) {
    println!(
        "RB|F1|{label}|code={}|operation={}|disposition={}|header_io_kind={:?}|{error}",
        error.code(),
        error.operation(),
        error.disposition().as_str(),
        error.receipt_header_io().map(|io| io.kind()),
    );
}

fn f1_revision(client: &RuntimeClient) -> Option<u64> {
    client
        .status()
        .expect("status")
        .scheduling_pause()
        .map(|pause| pause.revision)
}

fn f1_pause(client: &RuntimeClient) -> u64 {
    match client
        .pause_scheduling(SchedulingPauseScope::Global, "oneoff.rb", 1_000)
        .expect("global pause")
    {
        RuntimeResult::SchedulingPaused { revision, .. } => revision,
        other => panic!("pause result: {other:?}"),
    }
}

#[test]
fn oneoff_338rb_f1_old_runtime_branch_needs_this_calls_dropped_frame() {
    let root = TempDir::new().expect("state root");
    let host = host(&root, Arc::new(FakeState::default()), 5_000);
    let direct = client(&root);
    let epoch = direct.runtime_info().owner_epoch();
    let global = SchedulingPauseScope::Global;
    let revision = f1_pause(&direct);
    let expected = SchedulingPauseExpectation {
        owner_epoch: epoch,
        revision,
    };
    println!("RB|F1|PAUSED|revision={revision}");

    // (1) The emulated a9a1845d decoder drops the frames it cannot decode.
    let baseline = f1_proxy(
        direct.runtime_info(),
        Arc::new(|operation: &Value| match operation["operation"].as_str() {
            Some("resume_scheduling")
                if !operation.as_object().is_some_and(|fields| {
                    fields.keys().all(|key| key == "operation" || key == "scope")
                }) =>
            {
                ProxyAction::Drop
            }
            Some("resource_target_view") => ProxyAction::Drop,
            _ => ProxyAction::Forward,
        }),
    );
    let old = client(&baseline);
    let error = old
        .resume_scheduling_expected(global.clone(), expected)
        .expect_err("dropped conditional resume");
    f1_error("CASE1_BASELINE_RESUME", &error);
    assert_eq!(error.code(), "runtime_operation_unsupported");
    assert_eq!(error.disposition(), RuntimeClientErrorClass::Runtime);
    let error = old
        .resource_target_view("node.a")
        .expect_err("dropped view");
    f1_error("CASE1_BASELINE_VIEW", &error);
    assert_eq!(error.code(), "runtime_operation_unsupported");
    println!(
        "RB|F1|CASE1|status_revision={:?}|unchanged={}",
        f1_revision(&direct),
        f1_revision(&direct) == Some(revision)
    );

    // (2) A connection already failed by an earlier call (a status receipt that timed out).
    let held_status = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&held_status);
    let slow_status = f1_proxy(
        direct.runtime_info(),
        Arc::new(move |operation: &Value| {
            if operation["operation"].as_str() == Some("status")
                && counter.fetch_add(1, Ordering::AcqRel) == 0
            {
                ProxyAction::HoldThenForward(Duration::from_millis(1_500))
            } else {
                ProxyAction::Forward
            }
        }),
    );
    let latched = client_with_timeout(&slow_status, Duration::from_millis(300));
    let earlier = latched.status().expect_err("earlier status receipt times out");
    f1_error("CASE2_EARLIER_CALL", &earlier);
    let error = latched
        .resume_scheduling_expected(global.clone(), expected)
        .expect_err("already failed connection");
    f1_error("CASE2_PRE_LATCHED_RESUME", &error);
    assert_ne!(error.code(), "runtime_operation_unsupported");
    assert_eq!(error.disposition(), RuntimeClientErrorClass::Uncertain);
    let error = latched
        .resource_target_view("node.a")
        .expect_err("already failed connection");
    f1_error("CASE2_PRE_LATCHED_VIEW", &error);
    assert_ne!(error.code(), "runtime_operation_unsupported");
    assert_eq!(error.disposition(), RuntimeClientErrorClass::Uncertain);
    println!(
        "RB|F1|CASE2|status_revision={:?}",
        f1_revision(&direct)
    );

    // (3) A current daemon that has not answered within the client's receipt wait.
    let slow_resume = f1_proxy(
        direct.runtime_info(),
        Arc::new(|operation: &Value| {
            if operation["operation"].as_str() == Some("resume_scheduling") {
                ProxyAction::HoldThenForward(Duration::from_millis(1_500))
            } else {
                ProxyAction::Forward
            }
        }),
    );
    let impatient = client_with_timeout(&slow_resume, Duration::from_millis(300));
    let before = f1_revision(&direct);
    let error = impatient
        .resume_scheduling_expected(global.clone(), expected)
        .expect_err("receipt read times out");
    f1_error("CASE3_TIMEOUT_RESUME", &error);
    assert_ne!(error.code(), "runtime_operation_unsupported");
    assert_eq!(error.disposition(), RuntimeClientErrorClass::Uncertain);
    let at_error = f1_revision(&direct);
    thread::sleep(Duration::from_millis(2_000));
    let later = f1_revision(&direct);
    println!(
        "RB|F1|CASE3|status_revision_before={before:?}|at_client_error={at_error:?}|after_the_held_frame_arrived={later:?}"
    );
    drop((direct, old, latched, impatient));
    host.close().expect("close host");
}
