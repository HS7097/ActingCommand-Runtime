// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #389 U-2 (unified interface model v2.2.3 §3.2 rules 2 and 8, §3.5; §8.2 TU-2, TU-8):
// every connection of a client binds the channel its configuration names, on connect and on
// each reconnect (the `send_or_dropped` fallback and the contained-task timeout recovery). An
// actingd that does not know BindChannel drops the connection without a receipt: the client
// reconnects once in the same owner epoch, goes on unbound, says so
// (`client_channel_unsupported`) and sends no BindChannel on its later reconnects.

use super::*;
use actingcommand_contract::{RuntimeControlPlaneStatus, RuntimeOperationKind};
use std::sync::Mutex;

// A request, or `None` once the client hung up.
fn read_or_end(stream: &mut TcpStream) -> Option<RuntimeRequest> {
    let mut header = [0_u8; 4];
    match stream.read_exact(&mut header) {
        Ok(()) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
            ) =>
        {
            return None;
        }
        Err(error) => panic!("scripted request header: {error}"),
    }
    let mut body = vec![0_u8; u32::from_be_bytes(header) as usize];
    stream.read_exact(&mut body).expect("scripted request body");
    Some(serde_json::from_slice(&body).expect("decode request"))
}

// Health answered, then the BindChannel that follows it answered and returned.
fn answer_handshake(stream: &mut TcpStream, owner_epoch: OwnerEpoch) -> ChannelBinding {
    let health = read_scripted_request(stream);
    assert!(matches!(health.operation(), RuntimeOperation::Health));
    write_scripted_result(stream, &health, RuntimeResult::Health { owner_epoch });
    let bind = read_scripted_request(stream);
    let RuntimeOperation::BindChannel { channel, via } = bind.operation() else {
        panic!("BindChannel after Health, got {:?}", bind.operation());
    };
    let binding = ChannelBinding {
        channel: *channel,
        via: *via,
    };
    write_scripted_result(
        stream,
        &bind,
        RuntimeResult::ChannelBound {
            channel: binding.channel,
            via: binding.via,
        },
    );
    binding
}

fn forwarded_lab_config(root: &TempDir) -> RuntimeClientConfig {
    RuntimeClientConfig::new(
        root.path(),
        RuntimeChannel::Lab,
        EventActor::Lab,
        EventSource::Lab,
    )
    .with_via(ChannelVia::Mcp)
}

const FORWARDED_LAB: ChannelBinding = ChannelBinding {
    channel: RuntimeChannel::Lab,
    via: Some(ChannelVia::Mcp),
};

// TU-2: the `send_or_dropped` fallback reconnects, and the new connection binds the same
// channel and `via`.
#[test]
fn u2_a_reconnect_after_a_dropped_request_binds_the_same_channel() {
    let root = TempDir::new().expect("tempdir");
    let bindings = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&bindings);
    let server = scripted_runtime(&root, move |listener, owner_epoch| {
        let (mut first, _) = listener.accept().expect("accept the client");
        seen.lock()
            .expect("bindings")
            .push(answer_handshake(&mut first, owner_epoch));
        let view = read_scripted_request(&mut first);
        assert!(matches!(
            view.operation(),
            RuntimeOperation::ResourceTargetView { .. }
        ));
        drop(first);
        let (mut second, _) = listener.accept().expect("accept the reconnect");
        seen.lock()
            .expect("bindings")
            .push(answer_handshake(&mut second, owner_epoch));
        assert!(read_or_end(&mut second).is_none());
    });
    let client = RuntimeClient::connect(forwarded_lab_config(&root)).expect("runtime client");
    let error = client
        .resource_target_view("node.a")
        .expect_err("the dropped request is not answered");
    assert_eq!(error.code().as_str(), "runtime_operation_unsupported");
    drop(client);
    server.join().expect("scripted runtime");
    assert_eq!(
        *bindings.lock().expect("bindings"),
        vec![FORWARDED_LAB, FORWARDED_LAB]
    );
}

// TU-2: the contained-task timeout recovery opens a new connection that binds the same channel
// and `via` before it queries, cancels and resets the run.
#[test]
fn u2_the_contained_task_timeout_recovery_binds_the_same_channel() {
    let root = TempDir::new().expect("tempdir");
    let bindings = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&bindings);
    let server = scripted_runtime(&root, move |listener, owner_epoch| {
        let (mut timed_out, _) = listener.accept().expect("accept the task client");
        seen.lock()
            .expect("bindings")
            .push(answer_handshake(&mut timed_out, owner_epoch));
        let task = read_scripted_request(&mut timed_out);
        assert!(matches!(
            task.operation(),
            RuntimeOperation::RunContainedTask { .. }
        ));
        let (mut recovery, _) = listener.accept().expect("accept the recovery client");
        drop(timed_out);
        recovery
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("bounded recovery script");
        seen.lock()
            .expect("bindings")
            .push(answer_handshake(&mut recovery, owner_epoch));
    });
    let client = RuntimeClient::connect(
        forwarded_lab_config(&root)
            .with_io_timeout(Duration::from_millis(50))
            .with_backend_open_timeout(Duration::from_millis(100)),
    )
    .expect("runtime client");
    let _ = client.run_contained_task(
        "node.a",
        contained_task_request()
            .with_response_deadline_ms(100)
            .expect("bounded task deadline"),
    );
    drop(client);
    server.join().expect("scripted runtime");
    assert_eq!(
        *bindings.lock().expect("bindings"),
        vec![FORWARDED_LAB, FORWARDED_LAB]
    );
}

// TU-8: an actingd that cannot decode BindChannel drops the connection without a receipt. The
// client reconnects once, checks the owner epoch, goes on unbound and is usable; it reports
// `client_channel_unsupported` once through the frontend's reporter, and a later reconnect sends
// Health and no BindChannel and reports nothing more.
#[test]
fn u2_an_actingd_before_u2_leaves_the_client_unbound_and_usable() {
    // The frontend's reporter (here: this test's) receives the one report of the process.
    static REPORTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
    assert!(crate::set_channel_unsupported_reporter(|outcome| {
        REPORTS
            .lock()
            .expect("reports")
            .push(outcome.code().as_str().to_owned());
    }));
    let root = TempDir::new().expect("tempdir");
    let after_reconnect = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&after_reconnect);
    let server = scripted_runtime(&root, move |listener, owner_epoch| {
        // The old actingd: Health is answered, BindChannel ends the connection.
        let (mut first, _) = listener.accept().expect("accept the client");
        let health = read_scripted_request(&mut first);
        assert!(matches!(health.operation(), RuntimeOperation::Health));
        write_scripted_result(&mut first, &health, RuntimeResult::Health { owner_epoch });
        let bind = read_scripted_request(&mut first);
        assert!(matches!(
            bind.operation(),
            RuntimeOperation::BindChannel { .. }
        ));
        drop(first);
        // The one reconnect: Health, then the client's own requests.
        let (mut second, _) = listener.accept().expect("accept the reconnect");
        let health = read_scripted_request(&mut second);
        assert!(matches!(health.operation(), RuntimeOperation::Health));
        write_scripted_result(&mut second, &health, RuntimeResult::Health { owner_epoch });
        let status = read_scripted_request(&mut second);
        assert!(matches!(status.operation(), RuntimeOperation::Status));
        write_scripted_result(
            &mut second,
            &status,
            RuntimeResult::Status {
                status: RuntimeControlPlaneStatus::new(owner_epoch, Vec::new())
                    .expect("scripted status"),
            },
        );
        let view = read_scripted_request(&mut second);
        assert!(matches!(
            view.operation(),
            RuntimeOperation::ResourceTargetView { .. }
        ));
        drop(second);
        // A later reconnect: Health only.
        let (mut third, _) = listener.accept().expect("accept the later reconnect");
        let health = read_scripted_request(&mut third);
        assert!(matches!(health.operation(), RuntimeOperation::Health));
        write_scripted_result(&mut third, &health, RuntimeResult::Health { owner_epoch });
        while let Some(request) = read_or_end(&mut third) {
            seen.lock()
                .expect("requests")
                .push(request.operation().kind());
        }
    });
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("the client connects to the old actingd");
    assert!(client.channel_unsupported().expect("the client's state"));
    assert_eq!(
        *REPORTS.lock().expect("reports"),
        vec!["client_channel_unsupported".to_owned()]
    );
    client.status().expect("the unbound client is usable");
    let error = client
        .resource_target_view("node.a")
        .expect_err("the old actingd drops the unknown request");
    assert_eq!(error.code().as_str(), "runtime_operation_unsupported");
    drop(client);
    server.join().expect("scripted runtime");
    assert_eq!(REPORTS.lock().expect("reports").len(), 1);
    assert!(
        !after_reconnect
            .lock()
            .expect("requests")
            .contains(&RuntimeOperationKind::BindChannel)
    );
}
