// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 G-R2a, host half (0.12.60; register §1.361; contracts/resource-targets.md,
// "Document `actingcommand.resource-targets.v3`" and "Storage"; contracts/instance-fact-store.md,
// "Resource target policy facts"; coordinator work order rd-1114/brief-455-gr2a-v3-store.md):
// `ApplyResourceTargets` writes the goal document's value store through the host, on a manual
// clock and a fake device with the fixture catalog active (its one task produces the fixture
// pool, which declares no valuation, so every target states its step and importance):
// - a v3 document writes a v3 record whose rows name who set them, from the request's connection
//   and the Runtime's clock, never from the document; a later v3 document without `entries` and a
//   v2 document keep the stored entries as they are, and the read shows the v3 record;
// - `entries` replace the stored ones, a record of entries alone has no target, a document with
//   neither withdraws as a v2 document with `targets: []` does, a v1 document over a v3 record is
//   refused at its version, a replay appends nothing and `expected_version` is compared as T1-R's;
// - v1 and v2 documents over v1 or v2 records write exactly the rows they wrote before;
// - a start reloads a v3 record unchanged;
// - the version a writer reads is the version the gate compares, a withdrawal's `0` included,
//   and a receipt's `version` is not a value to send back (the coordinator's rulings of 11-14 on
//   #1702);
// - a request that names no channel applies a v1 or v2 document as any request does, and its v3
//   document is refused at request level, the Runtime's health untouched;
// - a v3 record's view decodes through the contract crate's strict types and passes `validate`,
//   as a console built against the released client decodes it.
// Every alias, pool and id is the fixture's or neutral.

use super::gate_383_ao4q::ledger_head;
use super::*;
use actingcommand_contract::{
    ChannelBinding, DeclaredInitiator, FactContent, FactPayload, FactRecord,
    FactScalar as ContractFactScalar, FactValue as ContractFactValue, Initiator,
    RESOURCE_TARGETS_FACT_KEY, ResourceTargetView, ResourceTargetsApplied,
    ResourceTargetsRejectionReason, RuntimeChannel,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::collections::BTreeMap;

const ALIAS: &str = POLICY_INSTANCE_ALIAS;
const START: u64 = POLICY_NOW_UNIX_MS;
const DAY_MS: u64 = 86_400_000;
/// The fixture catalog's pool, observed through `resource.primary`; its one task produces it.
const POOL: &str = "fixture-pool-a";
/// The second pool of `two_pools`, observed through `resource.secondary`; the fixture task produces
/// it too.
const POOL_B: &str = "fixture-pool-b";
const TASK: &str = "fixture.observe";
const V1: &str = "actingcommand.resource-targets.v1";
const V2: &str = "actingcommand.resource-targets.v2";
const V3: &str = "actingcommand.resource-targets.v3";
const APPLIED: &str = "request_apply_resource_targets_completed";
const REJECTED: &str = "resource_targets_rejected";
const CONFLICT: &str = "fact_observation_version_conflict";

/// A request's envelope on a connection.
type Envelope = fn(&TestClient, RuntimeOperation) -> RuntimeRequest;

/// A request from the Lab: the envelope `(lab, lab)`.
fn lab_request(client: &TestClient, operation: RuntimeOperation) -> RuntimeRequest {
    RuntimeRequest::new(
        client.ids.mint_request_id().expect("request id"),
        client.ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Lab,
        EventSource::Lab,
        1,
        operation,
    )
    .expect("lab runtime request")
}

/// The fixture pool's target, at least `amount`, with its own step and importance.
fn floor(amount: u64) -> Value {
    json!({
        "id": "neutral.floor",
        "resource": POOL,
        "condition": {"kind": "at_least", "amount": amount},
        "scale": 10,
        "importance_milli": 500,
        "apply": {"mode": "adjust", "weight": "score_stage"}
    })
}

/// `target` with its own lifetime.
fn lasting(mut target: Value, valid_until: u64) -> Value {
    target["valid_until_unix_ms"] = json!(valid_until);
    target
}

/// A v3 document of the fixture instance; `entries` absent when `None`.
fn v3(targets: Vec<Value>, entries: Option<Vec<Value>>) -> String {
    let mut document = json!({"schema_version": V3, "instance": ALIAS, "targets": targets});
    if let Some(entries) = entries {
        document["entries"] = Value::Array(entries);
    }
    document.to_string()
}

/// A v1 document of the fixture instance: the fixture pool's target naming the fixture task.
fn v1(amount: u64) -> String {
    let mut target = floor(amount);
    target["rule"] = json!("shortfall_linear");
    target["tasks"] = json!([TASK]);
    json!({
        "schema_version": V1,
        "instance": ALIAS,
        "valid_until_unix_ms": START + DAY_MS,
        "targets": [target]
    })
    .to_string()
}

fn entry(name: &str, value: Value) -> Value {
    json!({"name": name, "value": value})
}

/// The target `neutral.second` on pool B, at least `amount`, with its own step and importance.
fn second_floor(amount: u64) -> Value {
    let mut target = floor(amount);
    target["id"] = json!("neutral.second");
    target["resource"] = json!(POOL_B);
    target
}

/// The fixture catalog with a second pool, so that a document may hold two targets (one per
/// resource): pool B, a copy of the fixture pool observed through `resource.secondary`, which the
/// fixture task also produces.
fn two_pools() -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut pools: Value = serde_json::from_slice(&sources.pools.bytes).expect("the pools");
    let mut second = pools["pools"][0].clone();
    second["id"] = json!(POOL_B);
    second["observation"] = json!({"kind": "fact", "fact_key": "resource.secondary"});
    pools["pools"]
        .as_array_mut()
        .expect("the pool list")
        .push(second);
    sources.pools.bytes = serde_json::to_vec_pretty(&pools).expect("the pool bytes");
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("the tasks");
    let mut produced = tasks["tasks"][0]["produces"][0].clone();
    produced["pool_id"] = json!(POOL_B);
    tasks["tasks"][0]["produces"]
        .as_array_mut()
        .expect("the task's production")
        .push(produced);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("the task bytes");
    sources
}

/// The identity of T1-R's v2 document at least `amount` (`gate_455_t1r::document`), from its
/// canonical text written out by hand, as `gr2a_h3` pins it.
fn t1r_identity(amount: u64) -> String {
    let valid_until = START + DAY_MS;
    identity(&format!(
        r#"{{"instance":"{ALIAS}","schema_version":"{V2}","targets":[{{"apply":{{"mode":"adjust","weight":"score_stage"}},"condition":{{"amount":{amount},"kind":"at_least"}},"id":"neutral.floor","importance_milli":500,"resource":"{POOL}","scale":10}}],"valid_until_unix_ms":{valid_until}}}"#
    ))
}

/// The identity of `v1(amount)`, from its canonical text written out by hand, as `gr2a_h3` pins it.
fn v1_identity(amount: u64) -> String {
    let valid_until = START + DAY_MS;
    identity(&format!(
        r#"{{"instance":"{ALIAS}","schema_version":"{V1}","targets":[{{"apply":{{"mode":"adjust","weight":"score_stage"}},"condition":{{"amount":{amount},"kind":"at_least"}},"id":"neutral.floor","importance_milli":500,"resource":"{POOL}","rule":"shortfall_linear","scale":10,"tasks":["{TASK}"]}}],"valid_until_unix_ms":{valid_until}}}"#
    ))
}

fn text(value: &str) -> ContractFactScalar {
    ContractFactScalar::String(value.to_owned())
}

fn integer(value: u64) -> ContractFactScalar {
    ContractFactScalar::Integer(i64::try_from(value).expect("a fixture integer"))
}

fn row(fields: &[(&str, ContractFactScalar)]) -> BTreeMap<String, ContractFactScalar> {
    fields
        .iter()
        .map(|(field, value)| ((*field).to_owned(), value.clone()))
        .collect()
}

/// The `policy` header of a record of `version` with the identity `policy_sha256`.
fn header(version: &str, policy_sha256: &str) -> BTreeMap<String, ContractFactScalar> {
    row(&[
        ("row", text("policy")),
        ("schema_version", text(version)),
        ("instance", text(ALIAS)),
        ("policy_sha256", text(policy_sha256)),
    ])
}

/// The fields of the fixture pool's target row at least `amount`, as a v2 or v3 row has them.
fn floor_fields(amount: u64) -> Vec<(&'static str, ContractFactScalar)> {
    vec![
        ("row", text("target")),
        ("id", text("neutral.floor")),
        ("resource", text(POOL)),
        ("at_least", integer(amount)),
        ("scale", integer(10)),
        ("importance_milli", integer(500)),
        ("mode", text("adjust")),
        ("weight", text("score_stage")),
    ]
}

/// Who set a row: `set_unix_ms`, `channel` and, when given, `initiator`.
fn set_by(
    at: u64,
    channel: RuntimeChannel,
    initiator: Option<Initiator>,
) -> Vec<(&'static str, ContractFactScalar)> {
    let mut fields = vec![
        ("set_unix_ms", integer(at)),
        ("channel", text(channel.as_str())),
    ];
    if let Some(initiator) = initiator {
        fields.push(("initiator", text(initiator.as_str())));
    }
    fields
}

/// `sha256:<hex>` of a canonical text written out by hand.
fn identity(canonical: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))
}

/// The rows a record holds.
fn rows_of(record: &FactRecord) -> Vec<BTreeMap<String, ContractFactScalar>> {
    match &record.content {
        FactContent::Inline {
            value: ContractFactValue::RecordList(rows),
        } => rows.clone(),
        other => panic!("an inline record list, not {other:?}"),
    }
}

struct Fixture {
    root: TempDir,
    clock: Arc<ManualRuntimeClock>,
    now: Cell<u64>,
    host: RuntimeHost,
}

impl Fixture {
    /// A host on the fixture instance with the fixture catalog active and the Runtime clock at the
    /// fixture's time.
    fn start() -> Self {
        Self::start_with(&policy_sources(1))
    }

    /// A host on the fixture instance with `sources` active and the Runtime clock at the
    /// fixture's time.
    fn start_with(sources: &CatalogSources) -> Self {
        let root = TempDir::new().expect("tempdir");
        let clock = Arc::new(ManualRuntimeClock::new(START, 0));
        let host = Self::open(&root, &clock);
        host.activate_policy_catalog(sources)
            .expect("activate the catalog");
        Self {
            root,
            clock,
            now: Cell::new(START),
            host,
        }
    }

    fn open(root: &TempDir, clock: &Arc<ManualRuntimeClock>) -> RuntimeHost {
        RuntimeHost::start(
            config(root).with_runtime_clock(clock.clone()),
            Arc::new(FakeProvider::one(
                ALIAS,
                instance_id(),
                Arc::new(FakeState::default()),
            )),
        )
        .expect("runtime host")
    }

    /// A clean stop and a new start on the same state root, the Runtime clock at `at`; the active
    /// catalog is the ledger's.
    fn restart(self, at: u64) -> Self {
        let Self { root, host, .. } = self;
        host.close().expect("close the host");
        let clock = Arc::new(ManualRuntimeClock::new(at, 0));
        let host = Self::open(&root, &clock);
        Self {
            root,
            clock,
            now: Cell::new(at),
            host,
        }
    }

    /// A connection bound to `binding` by its `BindChannel`.
    fn bound(&self, binding: ChannelBinding) -> TestClient {
        let mut client = TestClient::connect(&self.host);
        let bind = client.request(RuntimeOperation::BindChannel {
            channel: binding.channel,
            via: binding.via,
            initiator: binding.initiator.map(DeclaredInitiator::from),
        });
        let bound = client.send(&bind);
        assert_eq!(bound.state(), RuntimeReceiptState::Completed, "{bound:?}");
        client
    }

    /// `ApplyResourceTargets` of `document` with `expected_version`, sent on `client`'s connection
    /// with `envelope`, the clock one millisecond on first; the receipt and the Runtime's time at
    /// the request.
    fn apply(
        &self,
        client: &mut TestClient,
        envelope: Envelope,
        document: &str,
        expected_version: Option<u64>,
    ) -> (RuntimeReceipt, u64) {
        self.clock.advance(1);
        self.now.set(self.now.get() + 1);
        let request = envelope(
            client,
            RuntimeOperation::ApplyResourceTargets {
                document_json: document.to_owned(),
                expected_version,
            },
        );
        (client.send(&request), self.now.get())
    }

    /// The Runtime clock `ms` milliseconds on.
    fn wait(&self, ms: u64) {
        self.clock.advance(ms);
        self.now.set(self.now.get() + ms);
    }

    /// The instance's `ResourceTargetView`.
    fn view(&self) -> ResourceTargetView {
        let mut client = TestClient::connect(&self.host);
        let request = client.request(RuntimeOperation::ResourceTargetView {
            instance_alias: ALIAS.to_owned(),
        });
        let receipt = client.send(&request);
        let Some(RuntimeResult::ResourceTargetView { view }) = receipt.result() else {
            panic!("a resource target view: {receipt:?}");
        };
        (**view).clone()
    }

    /// The instance's stored `session.resource_targets` record, as the fact store holds it.
    fn stored(&self) -> FactRecord {
        let mut client = TestClient::connect(&self.host);
        let request = client.request(RuntimeOperation::InstanceFactSnapshot {
            instance: ALIAS.to_owned(),
            key_prefix: Some(RESOURCE_TARGETS_FACT_KEY.to_owned()),
        });
        let receipt = client.send(&request);
        let Some(RuntimeResult::InstanceFactSnapshot { snapshot, .. }) = receipt.result() else {
            panic!("an instance fact snapshot: {receipt:?}");
        };
        snapshot
            .records
            .iter()
            .find(|record| record.key == RESOURCE_TARGETS_FACT_KEY)
            .cloned()
            .unwrap_or_else(|| panic!("a record of the key: {snapshot:?}"))
    }

    /// How many `fact.published` of the instance's resource targets the ledger holds.
    fn policy_events(&self) -> usize {
        self.host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::FactPublished),
                ..EventQuery::default()
            })
            .expect("ledger records")
            .into_iter()
            .filter(|event| {
                matches!(
                    event.payload(),
                    EventPayload::Fact(FactPayload::Published(payload))
                        if payload.record().key == RESOURCE_TARGETS_FACT_KEY
                )
            })
            .count()
    }

    fn head(&self) -> u64 {
        ledger_head(&mut TestClient::connect(&self.host))
    }

    fn close(self) {
        assert!(self.host.fatal_error().expect("runtime health").is_none());
        self.host.close().expect("close host");
    }
}

/// `lab` with the word `Lab-CLI`, as `actinglab` binds.
const fn lab_cli() -> ChannelBinding {
    ChannelBinding {
        channel: RuntimeChannel::Lab,
        via: None,
        initiator: Some(Initiator::LabCli),
    }
}

/// The policy a completed receipt answers.
#[track_caller]
fn applied(receipt: &RuntimeReceipt) -> ResourceTargetsApplied {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "the write is taken: {receipt:?}"
    );
    assert_eq!(receipt.outcome().code().as_str(), APPLIED, "{receipt:?}");
    let Some(RuntimeResult::ResourceTargetsApplied { applied }) = receipt.result() else {
        panic!("an applied policy: {receipt:?}");
    };
    (**applied).clone()
}

/// A refused document: Denied `resource_targets_rejected`, `reason` at `field_path`.
#[track_caller]
fn refused(receipt: &RuntimeReceipt, reason: ResourceTargetsRejectionReason, field_path: &str) {
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    assert_eq!(receipt.outcome().code().as_str(), REJECTED, "{receipt:?}");
    let rejection = receipt
        .resource_targets_rejection()
        .unwrap_or_else(|| panic!("a rejection: {receipt:?}"));
    assert_eq!(
        (rejection.reason, rejection.field_path.as_str()),
        (reason, field_path),
        "{receipt:?}"
    );
}

// The rules' rows that keep entries, and who set each row. On `lab` with `Lab-CLI` and the Lab's
// envelope, a v3 document (a target with its own lifetime, two entries) writes a v3 record: the
// header, the target row with its lifetime and with `set_unix_ms` the Runtime's time at the
// request, `channel` `lab` and `initiator` `Lab-CLI`, and the two entry rows, their values'
// canonical texts, set alike; the receipt of a v3 document names no lifetime and no condition. A
// document cannot name who set it: an entry with `set_unix_ms` is `unknown_field` and appends
// nothing. On `ui` with the console's envelope and the view's version, a v3 document without
// `entries` replaces the target (now without a lifetime, set at its own time by `ui`, no
// initiator) and keeps both entry rows byte for byte; its receipt names no lifetime and no
// condition. On `cli`, a v2 document replaces the target again, the document's lifetime its own,
// no one named as its setter, and the entry rows are still those of the first write; its receipt
// names the document's lifetime and its condition, as a v2 document's does. The view then
// answers the v3 record: its version, the target in document form with its own lifetime, no
// lifetime of the record, not expired.
#[test]
fn gr2a_h1_a_v3_record_names_who_set_its_rows_and_later_documents_keep_its_entries() {
    let fixture = Fixture::start();
    let mut lab = fixture.bound(lab_cli());
    let lab_envelope: Envelope = lab_request;
    let valid_until = START + DAY_MS;
    let first_entries = vec![entry("route", json!("a")), entry("rounds", json!(3))];

    let head = fixture.head();
    let (receipt, _) = fixture.apply(
        &mut lab,
        lab_envelope,
        &v3(
            vec![lasting(floor(50), valid_until)],
            Some(vec![
                json!({"name": "route", "value": "a", "set_unix_ms": 1}),
            ]),
        ),
        None,
    );
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    assert_eq!(
        receipt
            .resource_targets_rejection()
            .map(|rejection| rejection.reason),
        Some(ResourceTargetsRejectionReason::UnknownField),
        "{receipt:?}"
    );
    assert_eq!(fixture.head(), head, "a refusal appends nothing");

    let (receipt, first_at) = fixture.apply(
        &mut lab,
        lab_envelope,
        &v3(vec![lasting(floor(50), valid_until)], Some(first_entries)),
        None,
    );
    let first = applied(&receipt);
    assert_eq!(
        (first.valid_until_unix_ms, first.conditions.len()),
        (None, 0),
        "a v3 document's receipt names no lifetime and no condition"
    );
    let lab_set = set_by(first_at, RuntimeChannel::Lab, Some(Initiator::LabCli));
    let entry_row = |name: &str, value_json: &str| {
        let mut fields = vec![
            ("row", text("entry")),
            ("name", text(name)),
            ("value_json", text(value_json)),
        ];
        fields.extend(lab_set.clone());
        row(&fields)
    };
    let entry_rows = vec![entry_row("route", "\"a\""), entry_row("rounds", "3")];
    let stored = fixture.stored();
    assert_eq!(stored.expires_at_unix_ms, None, "a v3 record has no expiry");
    let mut target = floor_fields(50);
    target.push(("valid_until_unix_ms", integer(valid_until)));
    target.extend(lab_set.clone());
    let mut expected = vec![header(V3, &first.policy_sha256), row(&target)];
    expected.extend(entry_rows.clone());
    assert_eq!(rows_of(&stored), expected, "the first write");

    let mut console = fixture.bound(ChannelBinding::new(RuntimeChannel::Ui));
    let console_envelope: Envelope = TestClient::governance_request;
    let version = fixture.view().active.map_or(0, |active| active.version);
    assert_eq!(version, first.version);
    let (receipt, second_at) = fixture.apply(
        &mut console,
        console_envelope,
        &v3(vec![floor(60)], None),
        Some(version),
    );
    let second = applied(&receipt);
    assert_eq!(
        (second.valid_until_unix_ms, second.conditions.len()),
        (None, 0),
        "a target without a lifetime: the receipt names none"
    );
    let mut target = floor_fields(60);
    target.extend(set_by(second_at, RuntimeChannel::Ui, None));
    let mut expected = vec![header(V3, &second.policy_sha256), row(&target)];
    expected.extend(entry_rows.clone());
    assert_eq!(
        rows_of(&fixture.stored()),
        expected,
        "the target replaced, the entries kept"
    );

    let mut cli = TestClient::connect(&fixture.host);
    let cli_envelope: Envelope = TestClient::request;
    let v2 = json!({
        "schema_version": V2,
        "instance": ALIAS,
        "valid_until_unix_ms": valid_until,
        "targets": [floor(70)]
    })
    .to_string();
    let (receipt, _) = fixture.apply(&mut cli, cli_envelope, &v2, None);
    let third = applied(&receipt);
    assert_eq!(
        (third.valid_until_unix_ms, third.conditions.len()),
        (Some(valid_until), 1)
    );
    let mut target = floor_fields(70);
    target.push(("valid_until_unix_ms", integer(valid_until)));
    let mut expected = vec![header(V3, &third.policy_sha256), row(&target)];
    expected.extend(entry_rows);
    let stored = fixture.stored();
    assert_eq!(rows_of(&stored), expected, "a v2 document over a v3 record");
    assert_eq!(
        Some(stored.resource_bundle_hash.as_str()),
        third.policy_sha256.strip_prefix("sha256:"),
        "the receipt's identity names the v3 record written"
    );

    let view = fixture.view();
    let active = view.active.as_ref().expect("the v3 record is active");
    assert_eq!(
        (
            active.schema_version.as_str(),
            active.version,
            active.event_id,
            active.valid_until_unix_ms,
            active.expired,
        ),
        (V3, third.version, third.event_id, None, false)
    );
    assert_eq!(active.targets, vec![lasting(floor(70), valid_until)]);
    drop(lab);
    drop(console);
    drop(cli);
    fixture.close();
}

// The other rows of the rules, on `lab` with `Lab-CLI`. A v3 document with an entry is written; the
// same document again is a replay (its version, nothing appended), and with another version it is
// `fact_observation_version_conflict` naming the record's, nothing appended. A v1 document over the
// v3 record is refused `unsupported_schema_version` at `/schema_version`, nothing appended. A v3
// document with `entries: []` replaces the entries with none. A v3 document of an entry and no
// target writes a record of that entry alone, which the view does not show (no `active`); the same
// with no `entries` member keeps that entry, the same policy, so it is a replay. A v3 document of
// neither withdraws: the header of a v2 document with `targets: []` alone (its identity, the
// canonical text of that document), no expiry; the v2 document with `targets: []` is then a replay
// of it.
#[test]
fn gr2a_h2_entries_replaced_a_record_of_entries_alone_the_withdrawal_and_the_refusals() {
    let fixture = Fixture::start();
    let mut lab = fixture.bound(lab_cli());
    let send: Envelope = lab_request;
    let valid_until = START + DAY_MS;
    let document = v3(
        vec![lasting(floor(50), valid_until)],
        Some(vec![entry("route", json!("a"))]),
    );
    let first = applied(&fixture.apply(&mut lab, send, &document, None).0);
    let written = fixture.policy_events();
    let head = fixture.head();

    let replay = applied(&fixture.apply(&mut lab, send, &document, None).0);
    assert_eq!(
        (replay.version, replay.previous_version, replay.replayed),
        (first.version, Some(first.version), true)
    );
    let (conflict, _) = fixture.apply(&mut lab, send, &document, Some(first.version + 1));
    assert_eq!(
        conflict.state(),
        RuntimeReceiptState::Denied,
        "{conflict:?}"
    );
    assert_eq!(conflict.outcome().code().as_str(), CONFLICT, "{conflict:?}");
    assert_eq!(
        conflict.outcome().value("current_version"),
        Some(&json!(first.version)),
        "{conflict:?}"
    );
    refused(
        &fixture.apply(&mut lab, send, &v1(50), None).0,
        ResourceTargetsRejectionReason::UnsupportedSchemaVersion,
        "/schema_version",
    );
    assert_eq!(
        (fixture.policy_events(), fixture.head()),
        (written, head),
        "nothing appended"
    );

    applied(
        &fixture
            .apply(
                &mut lab,
                send,
                &v3(vec![lasting(floor(50), valid_until)], Some(Vec::new())),
                None,
            )
            .0,
    );
    let rows = rows_of(&fixture.stored());
    assert_eq!(rows.len(), 2, "the header and the target: {rows:?}");
    assert!(
        rows.iter()
            .all(|row| row.get("row") != Some(&text("entry")))
    );

    let (receipt, alone_at) = fixture.apply(
        &mut lab,
        send,
        &v3(Vec::new(), Some(vec![entry("route", json!("b"))])),
        None,
    );
    let alone = applied(&receipt);
    assert_eq!(
        (alone.valid_until_unix_ms, alone.conditions.len()),
        (None, 0)
    );
    let mut entry_row = vec![
        ("row", text("entry")),
        ("name", text("route")),
        ("value_json", text("\"b\"")),
    ];
    entry_row.extend(set_by(
        alone_at,
        RuntimeChannel::Lab,
        Some(Initiator::LabCli),
    ));
    assert_eq!(
        rows_of(&fixture.stored()),
        vec![header(V3, &alone.policy_sha256), row(&entry_row)]
    );
    assert!(
        fixture.view().active.is_none(),
        "no target, no active policy"
    );
    let kept = applied(&fixture.apply(&mut lab, send, &v3(Vec::new(), None), None).0);
    assert_eq!(
        (kept.version, kept.replayed),
        (alone.version, true),
        "the kept entry is the same policy"
    );

    let (receipt, _) = fixture.apply(&mut lab, send, &v3(Vec::new(), Some(Vec::new())), None);
    let withdrawn = applied(&receipt);
    let withdrawal = identity(&format!(
        r#"{{"instance":"{ALIAS}","schema_version":"{V2}","targets":[]}}"#
    ));
    assert_eq!(withdrawn.policy_sha256, withdrawal);
    let stored = fixture.stored();
    assert_eq!(rows_of(&stored), vec![header(V2, &withdrawal)]);
    assert_eq!(
        (stored.expires_at_unix_ms, withdrawn.valid_until_unix_ms),
        (None, None)
    );
    let v2_withdrawal = json!({"schema_version": V2, "instance": ALIAS, "targets": []}).to_string();
    let again = applied(&fixture.apply(&mut lab, send, &v2_withdrawal, None).0);
    assert_eq!(
        (again.version, again.replayed),
        (withdrawn.version, true),
        "the v2 withdrawal is the same record"
    );
    drop(lab);
    fixture.close();
}

// v1 and v2 records untouched. T1-R's v2 document (gate_455_t1r), and a v1 document of the same
// target naming the fixture task, applied over no record and over each other's records, write the
// rows the contract gives a v1 and a v2 policy, pinned byte for byte with each identity computed
// here from its canonical text, with the document's lifetime as the record's expiry, and the view
// answers each as before G-R2a: its version, its lifetime, not expired, the target in document form.
#[test]
fn gr2a_h3_v1_and_v2_documents_over_v1_or_v2_records_write_the_rows_they_wrote_before() {
    let fixture = Fixture::start();
    let mut client = TestClient::connect(&fixture.host);
    let send: Envelope = TestClient::agent_request;
    let valid_until = START + DAY_MS;
    let v2_identity = |amount: u64| {
        identity(&format!(
            r#"{{"instance":"{ALIAS}","schema_version":"{V2}","targets":[{{"apply":{{"mode":"adjust","weight":"score_stage"}},"condition":{{"amount":{amount},"kind":"at_least"}},"id":"neutral.floor","importance_milli":500,"resource":"{POOL}","scale":10}}],"valid_until_unix_ms":{valid_until}}}"#
        ))
    };
    let v1_identity = identity(&format!(
        r#"{{"instance":"{ALIAS}","schema_version":"{V1}","targets":[{{"apply":{{"mode":"adjust","weight":"score_stage"}},"condition":{{"amount":40,"kind":"at_least"}},"id":"neutral.floor","importance_milli":500,"resource":"{POOL}","rule":"shortfall_linear","scale":10,"tasks":["{TASK}"]}}],"valid_until_unix_ms":{valid_until}}}"#
    ));
    let v2_rows = |amount: u64| vec![header(V2, &v2_identity(amount)), row(&floor_fields(amount))];
    let mut v1_target = floor_fields(40);
    v1_target.push(("rule", text("shortfall_linear")));
    let v1_rows = vec![
        header(V1, &v1_identity),
        row(&v1_target),
        row(&[
            ("row", text("task")),
            ("target", text("neutral.floor")),
            ("task", text(TASK)),
        ]),
    ];
    for (label, document, identity, rows, schema_version) in [
        (
            "v2 over none",
            super::gate_455_t1r::document(50),
            v2_identity(50),
            v2_rows(50),
            V2,
        ),
        ("v1 over v2", v1(40), v1_identity.clone(), v1_rows, V1),
        (
            "v2 over v1",
            super::gate_455_t1r::document(60),
            v2_identity(60),
            v2_rows(60),
            V2,
        ),
        (
            "v2 over v2",
            super::gate_455_t1r::document(50),
            v2_identity(50),
            v2_rows(50),
            V2,
        ),
    ] {
        let written = applied(&fixture.apply(&mut client, send, &document, None).0);
        assert_eq!(
            (
                written.policy_sha256.as_str(),
                written.valid_until_unix_ms,
                written.conditions.len()
            ),
            (identity.as_str(), Some(valid_until), 1),
            "{label}"
        );
        let stored = fixture.stored();
        assert_eq!(rows_of(&stored), rows, "{label}");
        assert_eq!(
            (
                stored.expires_at_unix_ms,
                stored.resource_bundle_hash.as_str()
            ),
            (
                Some(valid_until),
                identity.strip_prefix("sha256:").expect("an identity")
            ),
            "{label}"
        );
        let view = fixture.view();
        let active = view.active.as_ref().expect("an active policy");
        let target: Value =
            serde_json::from_str::<Value>(&document).expect("the document")["targets"][0].clone();
        assert_eq!(
            (
                active.schema_version.as_str(),
                active.version,
                active.valid_until_unix_ms,
                active.expired,
                active.targets.clone(),
            ),
            (
                schema_version,
                written.version,
                Some(valid_until),
                false,
                vec![target]
            ),
            "{label}"
        );
    }
    drop(client);
    fixture.close();
}

// A start reloads a v3 record unchanged. A v3 document of a target with its own lifetime and two
// entries, written on `lab` with `Lab-CLI`, is read back from the fact store and through the view;
// after a clean stop and a new start on the same state root a minute later, the store holds the
// same record (every member, its rows byte for byte) and the view answers the same active policy
// (version, event, targets, lifetime, conditions); the same document is then a replay that
// appends nothing.
#[test]
fn gr2a_h4_a_start_reloads_a_v3_record_unchanged() {
    let fixture = Fixture::start();
    let mut lab = fixture.bound(lab_cli());
    let document = v3(
        vec![lasting(floor(50), START + DAY_MS)],
        Some(vec![
            entry("route", json!("a")),
            entry("order", json!(["x", "y"])),
        ]),
    );
    let written = applied(&fixture.apply(&mut lab, lab_request, &document, None).0);
    let stored = fixture.stored();
    let active = fixture.view().active.expect("the v3 record is active");
    drop(lab);

    let fixture = fixture.restart(START + 60_000);
    assert_eq!(fixture.stored(), stored, "the record reloaded unchanged");
    assert_eq!(
        fixture.view().active,
        Some(active),
        "the same resolved policy"
    );
    let events = fixture.policy_events();
    let mut lab = fixture.bound(lab_cli());
    let replay = applied(&fixture.apply(&mut lab, lab_request, &document, None).0);
    assert_eq!(
        (replay.version, replay.replayed),
        (written.version, true),
        "a replay after the start"
    );
    assert_eq!(fixture.policy_events(), events, "nothing appended");
    drop(lab);
    fixture.close();
}

// The version a writer reads is the version the gate compares, in every state (the coordinator's
// rulings of 11-14 on #1702, which correct T1-R's comparison): `active.version` when the view
// shows `active`, `0` when it shows none. In each state the view's version is read, a v2 document
// written with it is applied, and a v3 document written with a version read before is
// `fact_observation_version_conflict` whose `current_version` is what a new read answers, nothing
// appended. The states: no record, a withdrawn record (a v2 document with `targets: []`) and a v3
// record of entries alone, which the view answers without `active`, so `0`; an active v2 record;
// a v3 record with targets. Over the record of entries alone the applied v2 document keeps the
// entry (it carries none), and the stale version sent after it is that record's own, not `0`.
// A receipt's `version` is not a value to send back (the coordinator's ruling of 11-14 on #1702):
// over a withdrawn record and over a record of entries alone, which the view answers without
// `active`, a write that sends the `version` of the receipt that wrote the record is the conflict
// naming `0` and appends nothing, and the write with the read's `0` is then applied.
#[test]
fn gr2a_h5_the_version_a_writer_reads_is_the_version_the_gate_compares_in_every_state() {
    let valid_until = START + DAY_MS;
    let v2_document = |amount: u64| {
        json!({
            "schema_version": V2,
            "instance": ALIAS,
            "valid_until_unix_ms": valid_until,
            "targets": [floor(amount)]
        })
        .to_string()
    };
    let withdrawal = json!({"schema_version": V2, "instance": ALIAS, "targets": []}).to_string();
    let v3_document = |amount: u64| v3(vec![lasting(floor(amount), valid_until)], None);
    let entries_alone = v3(Vec::new(), Some(vec![entry("route", json!("a"))]));
    // Each state: its label, the documents that make it, whether the view shows no `active`, and
    // whether it is the record of entries alone.
    let states = [
        ("no record", Vec::new(), true, false),
        ("an active v2 record", vec![v2_document(40)], false, false),
        (
            "a withdrawn record",
            vec![v2_document(40), withdrawal],
            true,
            false,
        ),
        (
            "a v3 record of entries alone",
            vec![entries_alone],
            true,
            true,
        ),
        (
            "a v3 record with targets",
            vec![v3_document(40)],
            false,
            false,
        ),
    ];
    for (label, setup, read_as_none, entries_only) in states {
        let fixture = Fixture::start();
        let mut client = TestClient::connect(&fixture.host);
        let send: Envelope = TestClient::request;
        let mut setup_version = 0;
        for document in &setup {
            setup_version = applied(&fixture.apply(&mut client, send, document, None).0).version;
        }
        let read = fixture.view().active.map_or(0, |active| active.version);
        assert_eq!(
            read == 0,
            read_as_none,
            "{label}: the version read is {read}"
        );
        if read_as_none && setup_version > 0 {
            let head = fixture.head();
            let (sent_back, _) =
                fixture.apply(&mut client, send, &v2_document(50), Some(setup_version));
            assert_eq!(
                (sent_back.state(), sent_back.outcome().code().as_str()),
                (RuntimeReceiptState::Denied, CONFLICT),
                "{label}: the receipt's version sent back: {sent_back:?}"
            );
            assert_eq!(
                sent_back.outcome().value("current_version"),
                Some(&json!(0)),
                "{label}: {sent_back:?}"
            );
            assert_eq!(fixture.head(), head, "{label}: a conflict appends nothing");
        }
        let written = applied(
            &fixture
                .apply(&mut client, send, &v2_document(50), Some(read))
                .0,
        );
        let stale = if entries_only {
            assert!(
                setup_version > 0,
                "{label}: the record has a version of its own"
            );
            assert!(
                rows_of(&fixture.stored())
                    .iter()
                    .any(|row| row.get("name") == Some(&text("route"))),
                "{label}: the entry kept"
            );
            setup_version
        } else {
            read
        };
        let head = fixture.head();
        let (refused, _) = fixture.apply(&mut client, send, &v3_document(60), Some(stale));
        let now = fixture.view().active.map_or(0, |active| active.version);
        assert_eq!(now, written.version, "{label}");
        assert_eq!(
            refused.state(),
            RuntimeReceiptState::Denied,
            "{label}: {refused:?}"
        );
        assert_eq!(
            refused.outcome().code().as_str(),
            CONFLICT,
            "{label}: {refused:?}"
        );
        assert_eq!(
            refused.outcome().value("current_version"),
            Some(&json!(now)),
            "{label}: {refused:?}"
        );
        assert_eq!(fixture.head(), head, "{label}: a conflict appends nothing");
        drop(client);
        fixture.close();
    }
}

// A request that names no channel (the coordinator's ruling of 11-14 on #1702). The door tags every
// client request with its connection's channel, so no request of these fixtures arrives without
// one: the host takes the request from the door on with none
// (`process_request_without_channel_for_test`, the function that takes the channel). T1-R's v2
// document over no record and a v1 document over that v2 record are applied as on any request:
// completed, the identity, lifetime and condition of the receipt and the rows and expiry of the
// record those `gr2a_h3` pins. A v3 document is refused at request level: Denied
// `channel_binding_missing` at `apply_resource_targets` over `invalid_request`, the host not halted,
// no result, no terminal and no document rejection; nothing is appended, the v1 record stays in
// force, the Runtime's fatal state stays empty and its connections are served.
#[test]
fn gr2a_h6_a_request_without_a_channel_applies_v1_and_v2_and_its_v3_document_is_refused() {
    let fixture = Fixture::start();
    let client = TestClient::connect(&fixture.host);
    let connection = ConnectionId::new(901).expect("a connection id");
    let valid_until = START + DAY_MS;
    let without_channel = |document: &str| {
        fixture.wait(1);
        let request = client.request(RuntimeOperation::ApplyResourceTargets {
            document_json: document.to_owned(),
            expected_version: None,
        });
        fixture
            .host
            .process_request_without_channel_for_test(&request, connection)
            .expect("a receipt")
    };
    let mut v1_target = floor_fields(40);
    v1_target.push(("rule", text("shortfall_linear")));
    for (label, document, identity, rows) in [
        (
            "v2 over none",
            super::gate_455_t1r::document(50),
            t1r_identity(50),
            vec![header(V2, &t1r_identity(50)), row(&floor_fields(50))],
        ),
        (
            "v1 over v2",
            v1(40),
            v1_identity(40),
            vec![
                header(V1, &v1_identity(40)),
                row(&v1_target),
                row(&[
                    ("row", text("task")),
                    ("target", text("neutral.floor")),
                    ("task", text(TASK)),
                ]),
            ],
        ),
    ] {
        let written = applied(&without_channel(&document));
        assert_eq!(
            (
                written.policy_sha256.as_str(),
                written.valid_until_unix_ms,
                written.conditions.len()
            ),
            (identity.as_str(), Some(valid_until), 1),
            "{label}"
        );
        let stored = fixture.stored();
        assert_eq!(rows_of(&stored), rows, "{label}");
        assert_eq!(stored.expires_at_unix_ms, Some(valid_until), "{label}");
    }

    let head = fixture.head();
    let events = fixture.policy_events();
    let in_force = fixture.stored();
    let refused = without_channel(&v3(
        vec![lasting(floor(50), valid_until)],
        Some(vec![entry("route", json!("a"))]),
    ));
    assert_eq!(refused.state(), RuntimeReceiptState::Denied, "{refused:?}");
    let outcome = refused.outcome();
    assert_eq!(
        outcome.code().as_str(),
        "channel_binding_missing",
        "{refused:?}"
    );
    assert_eq!(
        (
            outcome.value("operation"),
            outcome.value("runtime_code"),
            outcome.value("host_halted")
        ),
        (
            Some(&json!("apply_resource_targets")),
            Some(&json!("invalid_request")),
            Some(&json!(false))
        ),
        "{refused:?}"
    );
    assert!(
        refused.result().is_none()
            && refused.terminal().is_none()
            && refused.resource_targets_rejection().is_none(),
        "{refused:?}"
    );
    assert_eq!(
        (fixture.head(), fixture.policy_events()),
        (head, events),
        "nothing appended"
    );
    assert_eq!(fixture.stored(), in_force, "the v1 record stays in force");
    assert!(
        fixture
            .host
            .fatal_error()
            .expect("runtime health")
            .is_none(),
        "the Runtime's health stays clean"
    );
    drop(client);
    fixture.close();
}

// What a reader of the previous revision decodes (the coordinator's ruling of 11-14 on #1702): a
// console built against the released client decodes `ResourceTargetView` with the contract crate's
// strict types (the view, `active`, its conditions and rows refuse unknown members) and runs
// `ResourceTargetView::validate`; G-R2a changes none of them. With two pools active, the read's
// answer for each v3 record below serialises, decodes through `ResourceTargetView` into the answer
// itself and passes `validate`; the existing members carry the v3 record so:
// - a v3 record with targets, one with its own lifetime and one without: `active.schema_version`
//   `actingcommand.resource-targets.v3`, `active.targets` the targets as untyped values in v3
//   document form, the first with its `valid_until_unix_ms`, the second without one, no
//   `valid_until_unix_ms` of `active`, `expired` false;
// - the same record kind once every target's own lifetime has passed: the targets still listed,
//   `expired` true, no `valid_until_unix_ms` of `active`;
// - a v3 record of entries alone: no `active`.
// The entries are not shown in any answer (the entry's name appears nowhere in it).
#[test]
fn gr2a_h7_a_v3_records_view_decodes_through_the_contracts_strict_types_and_passes_validate() {
    let fixture = Fixture::start_with(&two_pools());
    let mut client = TestClient::connect(&fixture.host);
    let send: Envelope = TestClient::request;
    // The read's answer as JSON, after it decoded through the strict types into itself and passed
    // `validate`.
    let decoded = |label: &str| -> Value {
        let view = fixture.view();
        let wire = serde_json::to_value(&view).expect("the view serialises");
        let read: ResourceTargetView = serde_json::from_value(wire.clone())
            .unwrap_or_else(|error| panic!("{label}: the strict types decode it: {error}"));
        read.validate()
            .unwrap_or_else(|error| panic!("{label}: the answer passes validate: {error:?}"));
        assert_eq!(read, view, "{label}");
        assert!(
            !wire.to_string().contains("route"),
            "{label}: entries are not shown: {wire}"
        );
        wire
    };

    let lifetime = START + DAY_MS;
    let with_targets = applied(
        &fixture
            .apply(
                &mut client,
                send,
                &v3(
                    vec![lasting(floor(50), lifetime), second_floor(20)],
                    Some(vec![entry("route", json!("a"))]),
                ),
                None,
            )
            .0,
    );
    let label = "a v3 record with targets";
    let wire = decoded(label);
    let active = &wire["active"];
    assert_eq!(
        (
            &active["schema_version"],
            &active["version"],
            &active["expired"]
        ),
        (&json!(V3), &json!(with_targets.version), &json!(false)),
        "{label}: {wire}"
    );
    assert_eq!(
        active["targets"],
        json!([lasting(floor(50), lifetime), second_floor(20)]),
        "{label}"
    );
    assert_eq!(
        (
            active["targets"][0].get("valid_until_unix_ms"),
            active["targets"][1].get("valid_until_unix_ms"),
            active.get("valid_until_unix_ms")
        ),
        (Some(&json!(lifetime)), None, None),
        "{label}: {wire}"
    );

    let (first_lapse, last_lapse) = (START + 30_000, START + 60_000);
    applied(
        &fixture
            .apply(
                &mut client,
                send,
                &v3(
                    vec![
                        lasting(floor(50), last_lapse),
                        lasting(second_floor(20), first_lapse),
                    ],
                    None,
                ),
                None,
            )
            .0,
    );
    fixture.wait(120_000);
    let label = "a v3 record whose targets are all expired";
    let wire = decoded(label);
    let active = &wire["active"];
    assert_eq!(
        (&active["schema_version"], &active["expired"]),
        (&json!(V3), &json!(true)),
        "{label}: {wire}"
    );
    assert_eq!(
        active["targets"],
        json!([
            lasting(floor(50), last_lapse),
            lasting(second_floor(20), first_lapse)
        ]),
        "{label}"
    );
    assert!(
        active.get("valid_until_unix_ms").is_none(),
        "{label}: {wire}"
    );

    applied(
        &fixture
            .apply(
                &mut client,
                send,
                &v3(Vec::new(), Some(vec![entry("route", json!("b"))])),
                None,
            )
            .0,
    );
    let label = "a v3 record of entries alone";
    let wire = decoded(label);
    assert!(wire.get("active").is_none(), "{label}: {wire}");
    drop(client);
    fixture.close();
}
