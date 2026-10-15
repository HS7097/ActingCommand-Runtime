// SPDX-License-Identifier: AGPL-3.0-only

// #382 C-4 (test plan v2.2 section 3.2 C-4 and section 3.10 C-4; inventory-contract G7 and I2;
// inventory key R8): the input proxy's heartbeat on the manual timer (CON-I2). One beat is one
// passed heartbeat interval, so no test waits on the wall clock.
// - C4-T1: each beat renews exactly once, with the token the previous renewal returned; without
//   a beat nothing is renewed.
// - C4-T2: a failed renewal ends the heartbeat, which renews no more, and the next proxy call
//   returns that failure.
// - C4-T3: dropping the proxy releases its lease exactly once, and a later beat renews nothing.
// - C4-T4, the wire row and test plan section 4 item 5's replacement: over `ScriptedRuntime`
//   with a real `RuntimeClient`, one beat is exactly one renewal frame, and the renewed token,
//   with its new expiry, is the token of the proxy's next input and of its release. Under
//   `test-observation` it carries the heartbeat proxy's trace assertion.
// Every wait returns as soon as its message arrives; `GATE_WAIT` only bounds a broken run.

use crate::input::{HeartbeatTimer, RuntimeInputAuthority, TestInputAuthority};
use crate::test_support::{ScriptedAnswer, ScriptedConnection, ScriptedGate, ScriptedRuntime};
use crate::{
    ClientLocation, DEFAULT_RUNTIME_HEARTBEAT_INTERVAL, RuntimeClient, RuntimeClientConfig,
    RuntimeClientError, RuntimeClientResult, RuntimeInputProxy,
};
use actingcommand_contract::{
    EventActor, EventSource, IdentifierIssuer, InputAction, InputFrameReference, LeaseToken,
    RuntimeErrorCode, RuntimeErrorProjection, RuntimeOperation, RuntimeReceipt,
    RuntimeReceiptState, RuntimeResult,
};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::Duration;
use tempfile::TempDir;

#[cfg(feature = "test-observation")]
use super::{assert_before, assert_client_observation_trace};
#[cfg(feature = "test-observation")]
use crate::test_observation::{
    ObservationOutcome, ObservationRecord, ObservationStage, TestObservationCapture,
};

const GATE_WAIT: Duration = Duration::from_secs(120);
const INSTANCE: &str = "node-01";
// The granted lease's expiry, and what each renewal adds to the expiry it renews.
const GRANTED_EXPIRY_MS: u64 = 60_000;
const RENEWAL_MS: u64 = 60_000;
// An IO budget no answered exchange of the wire row comes near.
const IO: Duration = Duration::from_secs(10);

// The lease `token` renewed: the same lease, instance and holder, its expiry `RENEWAL_MS`
// later, as a Runtime's renewal answers.
fn renewed(token: &LeaseToken) -> LeaseToken {
    LeaseToken::new(
        token.owner_epoch(),
        token.lease_id(),
        token.instance_id(),
        token.holder_id(),
        token.expires_at_monotonic_ms() + RENEWAL_MS,
    )
    .expect("renewed token")
}

fn granted_token() -> LeaseToken {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    LeaseToken::new(
        *issuer.mint_owner_epoch().expect("owner epoch").transport(),
        *issuer.mint_lease_id().expect("lease id").transport(),
        *issuer.mint_instance_id().expect("instance id").transport(),
        *issuer.mint_holder_id().expect("holder id").transport(),
        GRANTED_EXPIRY_MS,
    )
    .expect("granted token")
}

// The Runtime refuses a renewal because the lease expired: not fatal to the connection, but the
// lease is gone.
fn renewal_refused() -> RuntimeClientError {
    let projection = RuntimeErrorProjection::new(RuntimeErrorCode::LeaseExpired, false);
    RuntimeClientError::rejected(
        ClientLocation::RenewRuntimeLease,
        projection.clone(),
        projection.receipt_outcome(None, None),
    )
}

#[derive(Default)]
struct AuthorityCalls {
    acquisitions: usize,
    // The token each renewal was asked to renew, in order.
    renewed: Vec<LeaseToken>,
    released: Vec<LeaseToken>,
}

// CON-I2's fake lease authority: it grants `granted`, renews each token as `renewed` does,
// refuses the renewal numbered `failing_renewal` with its error, records every call and reports
// the number of each renewal to the test. No input may reach it.
struct ScriptedAuthority {
    granted: LeaseToken,
    failing_renewal: Option<(usize, RuntimeClientError)>,
    calls: Mutex<AuthorityCalls>,
    renewals: Mutex<mpsc::Sender<usize>>,
}

impl ScriptedAuthority {
    fn new(
        failing_renewal: Option<(usize, RuntimeClientError)>,
    ) -> (Arc<Self>, mpsc::Receiver<usize>) {
        let (renewals, reports) = mpsc::channel();
        let authority = Arc::new(Self {
            granted: granted_token(),
            failing_renewal,
            calls: Mutex::new(AuthorityCalls::default()),
            renewals: Mutex::new(renewals),
        });
        (authority, reports)
    }

    fn calls(&self) -> MutexGuard<'_, AuthorityCalls> {
        self.calls.lock().expect("authority calls")
    }
}

impl TestInputAuthority for ScriptedAuthority {
    fn acquire_lease(&self, instance_alias: &str) -> RuntimeClientResult<LeaseToken> {
        assert_eq!(instance_alias, INSTANCE);
        self.calls().acquisitions += 1;
        Ok(self.granted.clone())
    }

    fn renew_lease(&self, token: &LeaseToken) -> RuntimeClientResult<LeaseToken> {
        let number = {
            let mut calls = self.calls();
            calls.renewed.push(token.clone());
            calls.renewed.len()
        };
        let answer = match &self.failing_renewal {
            Some((failing, failure)) if *failing == number => Err(failure.clone()),
            _ => Ok(renewed(token)),
        };
        self.renewals
            .lock()
            .expect("renewal reports")
            .send(number)
            .expect("report the renewal");
        answer
    }

    fn input(
        &self,
        _token: &LeaseToken,
        _action: InputAction,
        _frame: Option<InputFrameReference>,
    ) -> RuntimeClientResult<RuntimeReceipt> {
        panic!("an input reached the scripted lease authority")
    }

    fn release_lease(&self, token: &LeaseToken) -> RuntimeClientResult<()> {
        self.calls().released.push(token.clone());
        Ok(())
    }
}

// A proxy on `authority` whose heartbeat waits on the manual timer.
fn connect(authority: &Arc<ScriptedAuthority>) -> RuntimeInputProxy {
    let fake: Arc<dyn TestInputAuthority> = authority.clone();
    RuntimeInputProxy::connect_for_test(
        RuntimeInputAuthority::Fake(fake),
        INSTANCE,
        DEFAULT_RUNTIME_HEARTBEAT_INTERVAL,
        HeartbeatTimer::Manual,
    )
    .expect("the input proxy")
}

// C4-T1: without a beat nothing is renewed and the release names the granted token; three beats
// are exactly three renewals, each of the token the one before returned, and the release names
// the third renewal's token.
#[test]
fn gate_each_heartbeat_beat_renews_once_with_the_token_the_previous_renewal_returned() {
    let (authority, reports) = ScriptedAuthority::new(None);
    let mut proxy = connect(&authority);
    proxy.close().expect("close the proxy before any beat");
    {
        let calls = authority.calls();
        assert_eq!(calls.acquisitions, 1);
        assert!(calls.renewed.is_empty(), "a renewal without a beat");
        assert_eq!(calls.released, [authority.granted.clone()]);
    }
    assert!(reports.try_recv().is_err(), "a renewal without a beat");

    let (authority, reports) = ScriptedAuthority::new(None);
    let mut proxy = connect(&authority);
    let heartbeat = proxy
        .manual_heartbeat()
        .expect("the proxy's manual heartbeat");
    for beat in 1..=3 {
        assert!(heartbeat.beat(), "beat {beat} reaches the heartbeat");
        assert_eq!(
            reports.recv_timeout(GATE_WAIT).expect("the beat's renewal"),
            beat
        );
    }
    proxy.close().expect("close the proxy after three beats");
    let first = renewed(&authority.granted);
    let second = renewed(&first);
    let third = renewed(&second);
    let calls = authority.calls();
    assert_eq!(calls.acquisitions, 1);
    assert_eq!(calls.renewed, [authority.granted.clone(), first, second]);
    assert_eq!(calls.released, [third]);
    drop(calls);
    assert!(reports.try_recv().is_err(), "more renewals than beats");
}

// C4-T2: the second renewal is refused. The heartbeat ends there: closing the proxy reports the
// failure and joins it, a later beat finds it gone, and nothing is renewed after the refusal. The
// next proxy call, an input, returns the refusal itself and reaches no authority.
#[test]
fn gate_a_failed_renewal_ends_the_heartbeat_and_the_next_proxy_call_returns_it() {
    let failure = renewal_refused();
    let (authority, reports) = ScriptedAuthority::new(Some((2, failure.clone())));
    let mut proxy = connect(&authority);
    let heartbeat = proxy
        .manual_heartbeat()
        .expect("the proxy's manual heartbeat");
    for beat in 1..=2 {
        assert!(heartbeat.beat(), "beat {beat} reaches the heartbeat");
        assert_eq!(
            reports.recv_timeout(GATE_WAIT).expect("the beat's renewal"),
            beat
        );
    }

    let input = proxy
        .input(InputAction::Tap { x: 10, y: 20 })
        .expect_err("an input after the refused renewal");
    assert_eq!(input, failure, "the next proxy call returns the refusal");

    proxy
        .close()
        .expect_err("closing reports the failed heartbeat");
    assert!(!heartbeat.beat(), "the ended heartbeat takes a beat");
    let first = renewed(&authority.granted);
    let calls = authority.calls();
    assert_eq!(calls.renewed, [authority.granted.clone(), first]);
    drop(calls);
    assert!(reports.try_recv().is_err(), "a renewal after the refusal");
}

// C4-T3: dropping the proxy releases its lease exactly once, with the granted token; a beat
// after the drop finds no heartbeat and renews nothing.
#[test]
fn gate_dropping_the_proxy_releases_its_lease_once_and_a_later_beat_renews_nothing() {
    let (authority, reports) = ScriptedAuthority::new(None);
    let proxy = connect(&authority);
    let heartbeat = proxy
        .manual_heartbeat()
        .expect("the proxy's manual heartbeat");
    drop(proxy);
    {
        let calls = authority.calls();
        assert_eq!(calls.released, [authority.granted.clone()]);
    }

    assert!(
        !heartbeat.beat(),
        "the dropped proxy's heartbeat takes a beat"
    );
    let calls = authority.calls();
    assert!(calls.renewed.is_empty(), "a renewal after the drop");
    assert_eq!(calls.released.len(), 1, "the lease is released once");
    drop(calls);
    assert!(reports.try_recv().is_err(), "a renewal after the drop");
}

// The tokens of the `select`ed requests a connection received, in order.
fn tokens(
    requests: &[actingcommand_contract::RuntimeRequest],
    select: impl Fn(&RuntimeOperation) -> Option<&LeaseToken>,
) -> Vec<LeaseToken> {
    requests
        .iter()
        .filter_map(|request| select(request.operation()).cloned())
        .collect()
}

// C4-T4, the wire row: a real `RuntimeClient` over `ScriptedRuntime`. The Runtime grants a
// lease; one beat sends exactly one renewal frame, of the granted token; the receipt's renewed
// token, with its expiry `RENEWAL_MS` later, is the token of the proxy's next input and of its
// release.
#[test]
fn gate_one_beat_sends_one_renewal_frame_and_the_renewed_token_carries_the_next_input() {
    #[cfg(feature = "test-observation")]
    let observation = TestObservationCapture::start().expect("start test observation");
    let root = TempDir::new().expect("tempdir");
    let renewal_answered = ScriptedGate::new();
    let runtime = ScriptedRuntime::start(root.path(), |owner_epoch| {
        vec![
            ScriptedConnection::new()
                .request(move |request| {
                    let RuntimeOperation::AcquireLease { holder_id, .. } = request.operation()
                    else {
                        panic!("an acquisition, got {:?}", request.operation())
                    };
                    let issuer = IdentifierIssuer::new().expect("identifier issuer");
                    let token = LeaseToken::new(
                        owner_epoch,
                        *issuer.mint_lease_id().expect("lease id").transport(),
                        *issuer.mint_instance_id().expect("instance id").transport(),
                        *holder_id,
                        GRANTED_EXPIRY_MS,
                    )
                    .expect("granted token");
                    ScriptedAnswer::answered(
                        request,
                        RuntimeReceiptState::Admitted,
                        RuntimeResult::LeaseGranted { token },
                    )
                })
                .request(|request| {
                    let RuntimeOperation::RenewLease { token } = request.operation() else {
                        panic!("a renewal, got {:?}", request.operation())
                    };
                    ScriptedAnswer::completed(
                        request,
                        RuntimeResult::LeaseRenewed {
                            token: renewed(token),
                        },
                    )
                })
                .open(&renewal_answered)
                .request(|request| {
                    assert!(
                        matches!(request.operation(), RuntimeOperation::Input { .. }),
                        "an input, got {:?}",
                        request.operation()
                    );
                    let action_id = *IdentifierIssuer::new()
                        .expect("identifier issuer")
                        .mint_action_id()
                        .expect("action id")
                        .transport();
                    ScriptedAnswer::completed(request, RuntimeResult::InputCommitted { action_id })
                })
                .request(|request| {
                    let RuntimeOperation::ReleaseLease { token } = request.operation() else {
                        panic!("a release, got {:?}", request.operation())
                    };
                    ScriptedAnswer::completed(
                        request,
                        RuntimeResult::LeaseReleased {
                            instance_id: token.instance_id(),
                            lease_id: token.lease_id(),
                        },
                    )
                }),
        ]
    });
    let client = RuntimeClient::connect(
        RuntimeClientConfig::new(
            root.path(),
            crate::RuntimeChannel::Lab,
            EventActor::Lab,
            EventSource::Lab,
        )
        .with_io_timeout(IO),
    )
    .expect("Lab runtime client");
    let mut proxy = RuntimeInputProxy::connect_for_test(
        RuntimeInputAuthority::Client(client.clone()),
        INSTANCE,
        DEFAULT_RUNTIME_HEARTBEAT_INTERVAL,
        HeartbeatTimer::Manual,
    )
    .expect("the input proxy");
    let heartbeat = proxy
        .manual_heartbeat()
        .expect("the proxy's manual heartbeat");
    assert!(heartbeat.beat(), "the beat reaches the heartbeat");
    renewal_answered.wait();
    let receipt = proxy
        .input(InputAction::Tap { x: 10, y: 20 })
        .expect("the input after the renewal");
    assert!(
        matches!(receipt.result(), Some(RuntimeResult::InputCommitted { .. })),
        "{receipt:?}"
    );
    proxy.close().expect("close the proxy");
    drop(heartbeat);
    drop(client);
    let record = runtime.finish();
    assert!(
        record.unscripted.is_empty(),
        "the client reconnected: {:?}",
        record.unscripted
    );

    let requests = &record.connections[0].requests;
    let renewals = tokens(requests, |operation| match operation {
        RuntimeOperation::RenewLease { token } => Some(token),
        _ => None,
    });
    let [renewal] = renewals.as_slice() else {
        panic!("one renewal frame for one beat: {renewals:?}")
    };
    assert_eq!(
        renewal.expires_at_monotonic_ms(),
        GRANTED_EXPIRY_MS,
        "the renewal renews the granted token"
    );
    let renewed_token = renewed(renewal);
    assert_eq!(
        renewed_token.expires_at_monotonic_ms(),
        GRANTED_EXPIRY_MS + RENEWAL_MS
    );
    let inputs = tokens(requests, |operation| match operation {
        RuntimeOperation::Input { token, .. } => Some(token),
        _ => None,
    });
    assert_eq!(
        inputs,
        [renewed_token.clone()],
        "the next input carries the renewed token"
    );
    let releases = tokens(requests, |operation| match operation {
        RuntimeOperation::ReleaseLease { token } => Some(token),
        _ => None,
    });
    assert_eq!(
        releases,
        [renewed_token],
        "the release carries the renewed token"
    );

    #[cfg(feature = "test-observation")]
    {
        let records = observation.finish().expect("seal test observation");
        assert_heartbeat_proxy_trace(&records);
    }
}

// Test plan section 4 item 5: the heartbeat proxy's trace, the client half of the retired
// test's assertion (the scripted Runtime has no host or backend half). The proxy acquires, the
// heartbeat's wait ends on its interval and renews, the proxy's input runs, the stop is sent
// before the heartbeat is joined and the release follows the join; the trace holds the granted
// token and its one renewal.
#[cfg(feature = "test-observation")]
fn assert_heartbeat_proxy_trace(records: &[ObservationRecord]) {
    let token_lineage = assert_client_observation_trace(records);
    assert_before(
        records,
        ObservationStage::ProxyAcquireStart,
        ObservationStage::ProxyAcquireResult,
    );
    assert!(records.iter().any(|record| {
        record.stage == ObservationStage::HeartbeatWaitResult
            && record.outcome == ObservationOutcome::Timeout
    }));
    assert_before(
        records,
        ObservationStage::HeartbeatRenewStart,
        ObservationStage::HeartbeatRenewResult,
    );
    assert_before(
        records,
        ObservationStage::ProxyInputStart,
        ObservationStage::ProxyInputResult,
    );
    assert_before(
        records,
        ObservationStage::ProxyStopSend,
        ObservationStage::HeartbeatJoinStart,
    );
    assert_before(
        records,
        ObservationStage::HeartbeatJoinStart,
        ObservationStage::HeartbeatJoinResult,
    );
    assert_before(
        records,
        ObservationStage::HeartbeatJoinResult,
        ObservationStage::ProxyReleaseStart,
    );
    assert_before(
        records,
        ObservationStage::ProxyReleaseStart,
        ObservationStage::ProxyReleaseResult,
    );
    assert_eq!(
        token_lineage.len(),
        2,
        "the granted token and its one renewal: {token_lineage:?}"
    );
}
