// SPDX-License-Identifier: AGPL-3.0-only

//! One-off (to be reverted), Workflow #338 Rb evidence for the runtime-client paths against a
//! real RuntimeHost: the conditional resume (match, revision and epoch refusals) and the
//! old-daemon detection of `resume_scheduling_expected` and `resource_target_view`. The older
//! daemon is EMULATED: a TCP proxy in front of the host applies the a9a1845d decoder's rule to
//! each frame (the baseline `ResumeScheduling` variant has only `scope`, `RuntimeOperation`
//! denies unknown fields, and `ResourceTargetView` does not exist) and drops the connection
//! without a receipt exactly where that decoder fails (`runtime_request_decode_failed`); every
//! other frame is forwarded unchanged. Copied under the runtime-client test module by the one-off
//! workflow only; every printed line starts `RB|`.

use super::*;
use actingcommand_contract::{SchedulingPauseExpectation, SchedulingPauseScope};
use std::net::SocketAddr;

fn rb_read_frame(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).ok()?;
    let length = u32::from_be_bytes(header) as usize;
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body).ok()?;
    Some(body)
}

fn rb_write_frame(stream: &mut TcpStream, body: &[u8]) -> bool {
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .and_then(|()| stream.write_all(body))
        .and_then(|()| stream.flush())
        .is_ok()
}

/// The a9a1845d decoder's verdict on one request frame.
fn baseline_decoder_accepts(frame: &[u8]) -> bool {
    let Ok(request) = serde_json::from_slice::<Value>(frame) else {
        return false;
    };
    let operation = &request["operation"];
    match operation["operation"].as_str() {
        Some("resume_scheduling") => operation
            .as_object()
            .is_some_and(|fields| fields.keys().all(|key| key == "operation" || key == "scope")),
        Some("resource_target_view") => false,
        _ => true,
    }
}

/// Starts the emulated baseline daemon in front of `upstream` and returns a state root whose
/// runtime info names it (same pid, owner epoch and start time as the real host).
fn baseline_proxy(upstream: &RuntimeInfo, drops: Arc<AtomicUsize>) -> TempDir {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind baseline proxy");
    let address = listener.local_addr().expect("proxy address");
    let target: SocketAddr = upstream.socket_addr().expect("upstream address");
    thread::spawn(move || {
        for incoming in listener.incoming() {
            let Ok(mut client) = incoming else { return };
            let drops = Arc::clone(&drops);
            thread::spawn(move || {
                let Ok(mut runtime) = TcpStream::connect(target) else {
                    return;
                };
                while let Some(frame) = rb_read_frame(&mut client) {
                    if !baseline_decoder_accepts(&frame) {
                        drops.fetch_add(1, Ordering::AcqRel);
                        let _ = client.shutdown(std::net::Shutdown::Both);
                        let _ = runtime.shutdown(std::net::Shutdown::Both);
                        return;
                    }
                    if !rb_write_frame(&mut runtime, &frame) {
                        return;
                    }
                    let Some(receipt) = rb_read_frame(&mut runtime) else {
                        return;
                    };
                    if !rb_write_frame(&mut client, &receipt) {
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

fn rb_error(label: &str, error: &RuntimeClientError) {
    println!(
        "RB|CLIENT|{label}|code={}|operation={}|host_failure={:?}|runtime_code={:?}|disposition={}|{error}",
        error.code(),
        error.operation(),
        error.host_failure(),
        error.projection().map(|projection| projection.code),
        error.disposition().as_str()
    );
}

fn global_revision(client: &RuntimeClient) -> Option<u64> {
    client
        .status()
        .expect("status")
        .scheduling_pause()
        .map(|pause| pause.revision)
}

fn pause_global(client: &RuntimeClient) -> u64 {
    match client
        .pause_scheduling(SchedulingPauseScope::Global, "oneoff.rb", 1_000)
        .expect("global pause")
    {
        RuntimeResult::SchedulingPaused { revision, .. } => revision,
        other => panic!("pause result: {other:?}"),
    }
}

#[test]
fn oneoff_338rb_client_conditional_resume_and_old_runtime_detection() {
    let root = TempDir::new().expect("state root");
    let host = host(&root, Arc::new(FakeState::default()), 5_000);
    let direct = client(&root);
    let epoch = direct.runtime_info().owner_epoch();
    let other_epoch = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_owner_epoch()
        .expect("other epoch")
        .transport();
    let global = SchedulingPauseScope::Global;

    let revision = pause_global(&direct);
    println!("RB|CLIENT|PAUSED|revision={revision}");
    let error = direct
        .resume_scheduling_expected(
            global.clone(),
            SchedulingPauseExpectation {
                owner_epoch: epoch,
                revision: revision + 1,
            },
        )
        .expect_err("revision mismatch");
    rb_error("REVISION_MISMATCH", &error);
    assert_eq!(
        error.host_failure(),
        Some(("scheduling_pause_revision_mismatch", "resume_scheduling"))
    );
    let error = direct
        .resume_scheduling_expected(
            global.clone(),
            SchedulingPauseExpectation {
                owner_epoch: other_epoch,
                revision,
            },
        )
        .expect_err("epoch mismatch");
    rb_error("EPOCH_MISMATCH", &error);
    assert_eq!(
        error.host_failure(),
        Some(("scheduling_pause_owner_epoch_mismatch", "resume_scheduling"))
    );
    println!(
        "RB|CLIENT|AFTER_REFUSALS|status_revision={:?}",
        global_revision(&direct)
    );

    let drops = Arc::new(AtomicUsize::new(0));
    let proxy_root = baseline_proxy(direct.runtime_info(), Arc::clone(&drops));
    let old = client(&proxy_root);
    let before = global_revision(&direct);
    let error = old
        .resume_scheduling_expected(
            global.clone(),
            SchedulingPauseExpectation {
                owner_epoch: epoch,
                revision,
            },
        )
        .expect_err("baseline decoder drops the conditional resume");
    rb_error("BASELINE_CONDITIONAL_RESUME", &error);
    let after = global_revision(&direct);
    println!(
        "RB|CLIENT|BASELINE_CONDITIONAL_RESUME|dropped_connections={}|status_revision_before={before:?}|after={after:?}|unchanged={}",
        drops.load(Ordering::Acquire),
        before == after
    );
    assert_eq!(error.code(), "runtime_operation_unsupported");
    assert_eq!(error.disposition(), RuntimeClientErrorClass::Runtime);
    assert_eq!(before, after);
    assert_eq!(after, Some(revision));

    let error = old
        .resource_target_view("node.a")
        .expect_err("baseline decoder drops the view");
    rb_error("BASELINE_VIEW", &error);
    assert_eq!(error.code(), "runtime_operation_unsupported");
    println!(
        "RB|CLIENT|BASELINE_VIEW|dropped_connections={}",
        drops.load(Ordering::Acquire)
    );
    match direct.resource_target_view("node.a") {
        Ok(view) => println!(
            "RB|CLIENT|DIRECT_VIEW_NO_POLICY_INPUTS|ok|{}",
            serde_json::to_string(&view).expect("view JSON")
        ),
        Err(error) => rb_error("DIRECT_VIEW_NO_POLICY_INPUTS", &error),
    }

    let resumed = old
        .resume_scheduling(global.clone())
        .expect("an unconditional resume still decodes under the baseline");
    println!(
        "RB|CLIENT|BASELINE_UNCONDITIONAL_RESUME|{}",
        serde_json::to_string(&resumed).expect("result JSON")
    );
    println!(
        "RB|CLIENT|BASELINE_UNCONDITIONAL_RESUME|status_revision={:?}",
        global_revision(&direct)
    );

    let stale = pause_global(&direct);
    direct
        .resume_scheduling(global.clone())
        .expect("someone else resumes");
    let error = old
        .resume_scheduling_expected(
            global.clone(),
            SchedulingPauseExpectation {
                owner_epoch: epoch,
                revision: stale,
            },
        )
        .expect_err("dropped after the pause it named was lifted");
    rb_error("BASELINE_STALE_EXPECTATION", &error);
    assert_eq!(error.code(), "runtime_scheduling_resume_unconfirmed");
    assert_eq!(error.disposition(), RuntimeClientErrorClass::Uncertain);

    let revision = pause_global(&direct);
    let matched = direct
        .resume_scheduling_expected(
            global.clone(),
            SchedulingPauseExpectation {
                owner_epoch: epoch,
                revision,
            },
        )
        .expect("matching conditional resume");
    println!(
        "RB|CLIENT|MATCH|{}",
        serde_json::to_string(&matched).expect("result JSON")
    );
    println!(
        "RB|CLIENT|MATCH|status_revision={:?}",
        global_revision(&direct)
    );
    drop((direct, old));
    host.close().expect("close host");
}
