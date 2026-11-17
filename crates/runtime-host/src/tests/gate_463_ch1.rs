// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 CH-1 (0.12.71; register §1.455; contracts/package-variables.md, "The run's
// admission record"; contracts/policy-suspension.md, "Content hash"; coordinator work order
// rd-1117/brief-463-ch1-admitted-content.md, after the owner's ruling of 11-16 on board #463,
// option A of question 220): the run's admission record (`task.requested`, `PackageAdmitted`)
// carries the content hash its admission measured on the bytes it read (`content_hash`) and the
// values handed to the run from outside (`variables`); both are only recorded, a record without
// them reads as before, and the public projection carries neither (ledger revision 9).
//
// Neutral fixtures. The kernel's admission by path of a package directory and of an assembled
// package ZIP, each written below a temporary directory. The direct runs are built as
// `gate_463_s38` builds its own: the host in composer mode over one game root on disk, a manual
// Runtime clock and one fake instance on a physical provider whose display is observed (a direct
// run needs it, Workflow #392 DA-A), after one evaluation so that the workbench judged the root's
// variables; each run is sent on a connection of its own, goes from `home` to `result` in one tap
// and completes. The scheduled runs are built as `gate_463_v2` builds its own (a fixture
// simulation; one rule due at every evaluation; a value set through `ApplyResourceTargets` on a
// connection bound `lab`). The package of the root declares the variables each test names.
// - `admitted_hash_equals_measure_content_hash_of_directory`: the kernel's admission of a
//   directory answers the content hash, 64 lowercase hexadecimal digits, equal to containment's
//   `measure_content_hash` of the same directory.
// - `assembled_zip_hash_is_file_sha256`: for an assembled package ZIP the measure is the SHA-256
//   of the whole file (D3), and the admission answers exactly that, as `measure_content_hash`
//   does.
// - `package_admitted_reads_back_with_and_without_hash`: the admission record of a direct run
//   given a value carries both members and reads back unchanged; the same record without them
//   (a revision-8 record) reads back unchanged and carries neither.
// - `hash_not_64_lower_hex_refused`: that record with a content hash of uppercase digits, 63 or 65
//   digits, a digit that is no hexadecimal one, none, or a `sha256:` prefix is refused where its
//   members are validated (`invalid_task_semantic_payload` at `task_semantic_fact`, the stored
//   reader's form); as written it validates. Fix round 1: so is that record with `variables` of no
//   entry, of one entry more than the 16 a package declares at most, of names not in strictly
//   ascending order (descending, or one name twice), with an entry of both forms or of neither,
//   or with a `value_sha256` that is not 64 lowercase hexadecimal digits; with 16 entries in
//   ascending order, or one entry of a SHA-256 and a length above 1024, it validates.
// - `direct_run_records_admitted_content`: a direct run's record carries the content hash of its
//   package directory as containment measures it; after the package's control declaration is
//   rewritten, the next run's record carries the new content's hash, another one.
// - `direct_run_records_given_values`: given `pick` and `order`, the record lists both, sorted by
//   name, each value's compact JSON text whole.
// - `value_over_1024_recorded_by_sha256_and_length`: an `ordered` value of 2145 bytes is recorded
//   by its text's SHA-256 and length only, one of exactly 1024 bytes whole.
// - `run_without_values_writes_the_bytes_of_the_hash_alone`: a run given no value writes no
//   `variables` member: its fact is byte for byte the fact with the content hash alone.
// - `scheduled_run_records_values_it_read`: a scheduled run's record lists the value its dispatch
//   handed down and the run read (the default when nothing is set, the set value after), and,
//   when the package's declaration was replaced between the dispatch and the run so that it no
//   longer takes the dispatched value, no `variables` member while the dispatch's reason still
//   says the value was set: the run read its default. Its content hash is the replaced content's.
// - `public_projection_carries_neither_member`: the public projection of a record with both
//   members is byte for byte the projection of the same record without them.
// - `startup_package_run_records_admitted_content` (fix round 1): a run the host starts itself,
//   the instance's startup package, run on a key the test holds as a granted startup claim runs it
//   (as `gate_463_u1` runs one), completes, and its admission record carries the content hash of
//   its package directory as containment measures it, and no `variables`.

use super::gate_442_rem1::write_package;
use super::gate_rl1b::{
    GAME_A, POOL_A, SERVER_A, bundle, pool, profiles, resources, rule, write_json,
};
use super::*;
use actingcommand_contract::codes::SanitizationCode;
use actingcommand_contract::{DeclaredInitiator, Initiator, RuntimeChannel};
use actingcommand_policy::PolicyEvaluation;
use serde_json::{Value, json};

const TASK: &str = "fixture.ch1";
const INSTANCE: &str = POLICY_INSTANCE_ALIAS;
const RESULT: [u8; 3] = [255, 255, 255];
const HOME: [u8; 3] = [255, 0, 0];
const PARAMETERS: &str = "policy_dispatch_parameters";

// ---- The game root.

// The variable `pick`: a `choice` of `item_a` and `item_b`, default `item_b`.
fn pick() -> Value {
    json!({"name": "pick", "type": "choice", "title": {"en": "Item"},
           "options": [{"value": "item_a", "title": {"en": "Item A"}},
                       {"value": "item_b", "title": {"en": "Item B"}}],
           "default": "item_b"})
}

// `pick` as a replaced package declares it: `item_a` is no option any more.
fn pick_without_item_a() -> Value {
    json!({"name": "pick", "type": "choice", "title": {"en": "Item"},
           "options": [{"value": "item_b", "title": {"en": "Item B"}},
                       {"value": "item_c", "title": {"en": "Item C"}}],
           "default": "item_b"})
}

// The variable `order`: `ordered`, the options `item_a` and `item_b`, none to both.
fn order() -> Value {
    json!({"name": "order", "type": "ordered", "title": {"en": "Order"},
           "options": [{"value": "item_a", "title": {"en": "Item A"}},
                       {"value": "item_b", "title": {"en": "Item B"}}],
           "min_count": 0, "max_count": 2, "default": ["item_b"]})
}

// `count` option values of `bytes` bytes each: a letter, two digits, then `x`.
fn keys(letter: char, count: usize, bytes: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("{letter}{index:02}{}", "x".repeat(bytes - 3)))
        .collect()
}

// An `ordered` variable `name` whose options are `values`, none to all of them, default the
// first alone.
fn listed(name: &str, values: &[String]) -> Value {
    let options = values
        .iter()
        .map(|value| json!({"value": value, "title": {"en": "Key"}}))
        .collect::<Vec<_>>();
    json!({"name": name, "type": "ordered", "title": {"en": "Keys"},
           "options": options, "min_count": 0, "max_count": values.len(),
           "default": [values[0]]})
}

// Changes the control declaration of the package in `package` in place.
fn patch_control(package: &Path, change: impl FnOnce(&mut Value)) {
    let control_path = package.join("control.json");
    let mut control: Value =
        serde_json::from_slice(&fs::read(&control_path).expect("the control declaration"))
            .expect("control JSON");
    change(&mut control);
    write_json(&control_path, &control);
}

// Places the game root below `packs`: its bundle, its `schedule.json` with the one rule, due at
// every evaluation, and the package, whose control declaration declares `declared`.
fn place_game_root(packs: &Path, declared: Value) {
    let game = packs.join(GAME_A);
    write_json(
        &game.join("bundle.json"),
        &bundle(GAME_A, SERVER_A, &[TASK]),
    );
    let mut due = rule(TASK, TASK, 100, Some(POOL_A));
    due["trigger"] = json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"}, "every_ms": 1,
                     "anchor_ms": 0}
    });
    due["loop_budget"]["window_iteration_limit"] = json!(8);
    due["loop_budget"]["max_runtime_ms"] = json!(3_600_000);
    let mut activity = profiles("", Some(POOL_A));
    for profile in &mut activity {
        profile["max_window_iterations"] = json!(8);
    }
    write_json(
        &game.join("schedule.json"),
        &json!({
            "schema_version": "actingcommand.schedule.v1",
            "requires": [],
            "rules": [due],
            "pools": [pool(POOL_A)],
            "activity": activity,
            "timeline": [],
            "held": []
        }),
    );
    let package = game.join(TASK);
    write_package(&package, TASK, None, None);
    patch_control(&package, |control| control["variables"] = declared);
}

// The fixture's facts: the one instance bound to game A on server A, able to run the Runtime's
// package operation, and no recorded outcome.
fn facts() -> EvaluationFacts {
    let mut facts = policy_facts();
    facts.outcomes.clear();
    facts.instances = vec![InstanceSnapshot {
        instance_id: INSTANCE.to_owned(),
        server_id: SERVER_A.to_owned(),
        game_id: GAME_A.to_owned(),
        host_id: "fixture-host-a".to_owned(),
        available: true,
        capability_operation_ids: vec![actingcommand_policy::STANDARD_PACK_OPERATION_ID.to_owned()],
        preferred_task_ids: Vec::new(),
    }];
    facts
}

// ---- The fixture.

struct Scene {
    _state_root: TempDir,
    packs: TempDir,
    device: Arc<FakeState>,
    clock: Arc<ManualRuntimeClock>,
    host: RuntimeHost,
}

impl Scene {
    // The host for direct runs, as `gate_463_s38` builds it, after one evaluation: the workbench
    // judged the root and its variables apply.
    fn direct(declared: Value) -> Self {
        let scene = Self::start(declared, false);
        scene.sample_capacity();
        // The workbench checks the game roots at the start of a policy cycle; what the cycle
        // dispatches is not this test's.
        let _cycle = scene
            .host
            .evaluate_policy_cycle_with_test_inputs(
                &facts(),
                &resources(&[POOL_A]),
                EvaluationTime {
                    unix_ms: POLICY_NOW_UNIX_MS,
                    monotonic_ms: POLICY_NOW_UNIX_MS,
                },
                7,
                PolicyTrigger::FactsChanged,
            )
            .unwrap_or_else(|error| panic!("evaluate the policy: {error}"));
        scene
    }

    // The host for scheduled runs, as `gate_463_v2` builds it: a fixture simulation.
    fn scheduled(declared: Value) -> Self {
        Self::start(declared, true)
    }

    fn start(declared: Value, simulation: bool) -> Self {
        let state_root = TempDir::new().expect("state root");
        let packs = TempDir::new().expect("standard-pack root");
        place_game_root(packs.path(), declared);
        let device = Arc::new(FakeState::default());
        let provider = FakeProvider::one(INSTANCE, instance_id(), Arc::clone(&device));
        let provider = if simulation {
            provider.fixture_simulation()
        } else {
            // A direct run needs a device instance whose display is observed (DA-A).
            device.physical_task_geometry.store(true, Ordering::Release);
            *device
                .physical_task_extent
                .lock()
                .expect("fake physical task extent") = Some((2, 1));
            provider
        };
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let host = RuntimeHost::start(
            config(&state_root)
                .with_policy_inputs(PolicyInputSnapshot::new(facts(), policy_resources()))
                .with_runtime_clock(clock.clone())
                .with_standard_packs_root(Some(packs.path().to_owned()))
                .with_standard_pack_schedule(true),
            Arc::new(provider),
        )
        .expect("start the composer-mode host");
        Self {
            _state_root: state_root,
            packs,
            device,
            clock,
            host,
        }
    }

    // The package's directory, the path its runs name.
    fn package(&self) -> PathBuf {
        self.packs.path().join(GAME_A).join(TASK)
    }

    fn request(&self) -> ContainedTaskRequest {
        ContainedTaskRequest::new(self.package().to_string_lossy().into_owned())
            .expect("contained task request")
    }

    fn sample_capacity(&self) {
        self.host
            .capacity_sampler_for_test()
            .expect("capacity owner")()
        .expect("capacity sample");
    }

    // The device shows `home` until the next run's tap, then `result`.
    fn script_device(&self) {
        let taps = self.device.input_count.load(Ordering::Acquire);
        *self.device.pixel_script.lock().expect("pixel script") =
            Some(Arc::new(move |_: usize, inputs: usize| {
                vec![(0, if inputs <= taps { HOME } else { RESULT })]
            }));
    }

    // One direct run of the package with the values `given`, on a connection of its own; it
    // completes after its one tap.
    fn run(&self, given: &[(&str, Value)]) {
        self.script_device();
        self.sample_capacity();
        let request = self
            .request()
            .with_variables(
                given
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), value.clone()))
                    .collect(),
            )
            .expect("the values' names are variables' names");
        let mut client = TestClient::connect(&self.host);
        client.set_receipt_read_timeout();
        let run = client.request(RuntimeOperation::run_contained_task(
            INSTANCE,
            client.ids.mint_holder_id().expect("holder"),
            request,
        ));
        let receipt = client.send(&run);
        drop(client);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
    }

    // One full evaluation at `at`, the Runtime clock moved there first.
    fn evaluate(&self, at: u64, seed: u64) -> PolicyEvaluation {
        advance_manual_clock_to(&self.clock, at);
        self.sample_capacity();
        self.host
            .evaluate_policy_cycle_with_test_inputs(
                &facts(),
                &resources(&[POOL_A]),
                EvaluationTime {
                    unix_ms: at,
                    monotonic_ms: at,
                },
                seed,
                PolicyTrigger::FactsChanged,
            )
            .unwrap_or_else(|error| panic!("evaluate the policy at {at}: {error}"))
            .evaluation
            .expect("a full evaluation")
    }

    // Evaluates at `at`, admits the dispatch of the rule, calls `before_run` with the package's
    // directory (between the dispatch and its run), runs it (`home`, one tap, `result`) and
    // settles it: the reason chain the dispatch carried and its run's context.
    fn evaluate_and_run(
        &self,
        at: u64,
        seed: u64,
        before_run: impl FnOnce(&Path),
    ) -> (DecisionReasonChain, Box<PolicyRunContext>) {
        let evaluation = self.evaluate(at, seed);
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == TASK && intent.instance_id == INSTANCE)
            .unwrap_or_else(|| panic!("a dispatch of {TASK}: {evaluation:#?}"))
            .clone();
        let chain = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("the intent's reason chain")
            .clone();
        self.sample_capacity();
        let admission = PolicyAdmissionContext {
            fact_ledger_position: intent.input_ledger_position,
            fact_snapshot_id: intent.fact_snapshot_id.clone(),
            approval_fact_ids: BTreeSet::new(),
            fencing_owner_epoch: self.host.runtime_info().owner_epoch(),
            now_unix_ms: intent.prerequisites.evaluated_at_unix_ms,
        };
        let PolicyDispatchAdmission::Granted { context } = self
            .host
            .admit_scheduled_policy_dispatch(&intent, &chain, &admission, &self.request())
            .unwrap_or_else(|error| panic!("admit {TASK}: {error}"))
        else {
            panic!("expected one policy run context")
        };
        before_run(self.package().as_path());
        self.script_device();
        let receipt = self
            .host
            .run_scheduled_contained_task(&context, &self.request())
            .expect("the scheduled run");
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
        self.host
            .complete_scheduled_policy_run(&context, &receipt)
            .expect("the run settles");
        (chain, context)
    }

    // `ApplyResourceTargets` of a v3 document of the instance with no target and the one entry
    // `pick` = `value`, on a connection bound `lab` with `Lab-CLI` and the Lab's envelope, the
    // Runtime clock at `at` first. It is applied.
    fn set_pick(&self, value: Value, at: u64) {
        advance_manual_clock_to(&self.clock, at);
        let mut client = TestClient::connect(&self.host);
        let bind = client.request(RuntimeOperation::BindChannel {
            channel: RuntimeChannel::Lab,
            via: None,
            initiator: Some(DeclaredInitiator::from(Initiator::LabCli)),
        });
        let bound = client.send(&bind);
        assert_eq!(bound.state(), RuntimeReceiptState::Completed, "{bound:?}");
        let document = json!({
            "schema_version": "actingcommand.resource-targets.v3",
            "instance": INSTANCE,
            "targets": [],
            "entries": [{"name": "pick", "value": value}]
        });
        let request = RuntimeRequest::new(
            client.ids.mint_request_id().expect("request id"),
            client.ids.mint_correlation_id().expect("correlation id"),
            None,
            EventActor::Lab,
            EventSource::Lab,
            1,
            RuntimeOperation::ApplyResourceTargets {
                document_json: document.to_string(),
                expected_version: None,
            },
        )
        .expect("lab runtime request");
        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "the value is set: {receipt:?}"
        );
    }

    // The payload of the last admission record (`task.requested`) the ledger holds.
    fn last_admission(&self) -> EventPayload {
        last_admission(&self.host)
    }

    fn close(self) {
        self.host.close().expect("close the composer-mode host");
    }
}

// The payload of the last admission record (`task.requested`) the ledger of `host` holds.
fn last_admission(host: &RuntimeHost) -> EventPayload {
    let mut events = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::TaskRequested),
            ..EventQuery::default()
        })
        .expect("the ledger's admission records");
    events.sort_by_key(PersistedEvent::sequence);
    events
        .last()
        .map(|event| event.payload().clone())
        .expect("an admission record")
}

// The admission fact of `payload`.
fn fact(payload: &EventPayload) -> &TaskSemanticFact {
    match payload {
        EventPayload::Task(TaskPayload::Semantic(semantic))
            if matches!(semantic.fact(), TaskSemanticFact::PackageAdmitted { .. }) =>
        {
            semantic.fact()
        }
        other => panic!("a package admission: {other:?}"),
    }
}

// What the admission fact of `payload` carries of CH-1's: its content hash and its values as
// JSON (`null` without them).
fn admitted(payload: &EventPayload) -> (Option<String>, Value) {
    let TaskSemanticFact::PackageAdmitted {
        content_hash,
        variables,
        ..
    } = fact(payload)
    else {
        panic!("a package admission: {payload:?}")
    };
    (
        content_hash.clone(),
        serde_json::to_value(variables).expect("the values encode"),
    )
}

// The content hash of the package at `path` as containment measures it (`measure_content_hash`).
fn measured(path: &Path) -> String {
    crate::suspension_lift::measure_package_hash(&path.to_string_lossy())
        .expect("the package is measured")
}

// The fact's object in the stored record `stored`.
fn fact_object(stored: &mut Value) -> &mut serde_json::Map<String, Value> {
    stored
        .pointer_mut("/payload/data/fact")
        .and_then(Value::as_object_mut)
        .expect("the record's fact")
}

// The stored record `stored`, decoded.
fn read(stored: &Value) -> EventPayload {
    serde_json::from_value(stored.clone()).expect("the record decodes")
}

// The stored record `stored` without the two members of ledger revision 9.
fn without_members(stored: &Value) -> Value {
    let mut stored = stored.clone();
    let object = fact_object(&mut stored);
    object.remove("content_hash");
    object.remove("variables");
    stored
}

// The stored record `stored` with its fact's member `member` set to `value`.
fn with_member(stored: &Value, member: &str, value: &Value) -> Value {
    let mut changed = stored.clone();
    fact_object(&mut changed).insert(member.to_owned(), value.clone());
    changed
}

// `stored` with its fact's `member` set to `value` validates.
fn assert_in_form(stored: &Value, member: &str, value: &Value) {
    if let Err(error) = read(&with_member(stored, member, value)).validate() {
        panic!("{member} {value} is in its form: {error:?}");
    }
}

// `stored` with its fact's `member` set to `value` is refused where the record's members are
// validated: `invalid_task_semantic_payload` at `task_semantic_fact`, the stored reader's form.
fn assert_refused(stored: &Value, member: &str, value: &Value) {
    let error = read(&with_member(stored, member, value))
        .validate()
        .expect_err("a member out of its form is refused");
    assert_eq!(
        error.code(),
        SanitizationCode::InvalidTaskSemanticPayload.code(),
        "{member} {value}"
    );
    assert_eq!(error.field(), "task_semantic_fact", "{member} {value}");
}

#[test]
fn admitted_hash_equals_measure_content_hash_of_directory() {
    let root = TempDir::new().expect("packages");
    let package = root.path().join(TASK);
    write_package(&package, TASK, None, None);
    let (_, _, content_hash) = PreparedContainedTask::load_path_with_resident_bytes(
        INSTANCE,
        &package,
        None,
        admission_deadline(),
    )
    .expect("the package directory is admitted");
    let content_hash = content_hash.expect("its admission measured the content hash");
    assert!(
        content_hash.len() == 64
            && content_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')),
        "{content_hash}"
    );
    assert_eq!(content_hash, measured(&package));
}

#[test]
fn assembled_zip_hash_is_file_sha256() {
    let root = TempDir::new().expect("packages");
    let package = root.path().join("neutral-task.zip");
    let bytes = neutral_contained_task_package(true);
    fs::write(&package, &bytes).expect("write the package");
    let (_, _, content_hash) = PreparedContainedTask::load_path_with_resident_bytes(
        INSTANCE,
        &package,
        None,
        admission_deadline(),
    )
    .expect("the assembled package is admitted");
    let file_sha256 = format!("{:x}", Sha256::digest(&bytes));
    assert_eq!(content_hash.as_deref(), Some(file_sha256.as_str()));
    assert_eq!(measured(&package), file_sha256);
}

#[test]
fn package_admitted_reads_back_with_and_without_hash() {
    let scene = Scene::direct(json!([pick()]));
    scene.run(&[("pick", json!("item_a"))]);
    let written = scene.last_admission();
    let (content_hash, variables) = admitted(&written);
    assert_eq!(content_hash, Some(measured(&scene.package())));
    assert_eq!(
        variables,
        json!([{"variable_name": "pick", "value_json": "\"item_a\""}])
    );
    let stored = serde_json::to_value(&written).expect("the record encodes");
    let with = read(&stored);
    with.validate().expect("the record validates");
    assert_eq!(
        serde_json::to_value(&with).expect("the record encodes"),
        stored,
        "the record reads back unchanged"
    );
    let stored = without_members(&stored);
    let without = read(&stored);
    without
        .validate()
        .expect("a record without the members validates");
    assert_eq!(
        serde_json::to_value(&without).expect("the record encodes"),
        stored,
        "a record without the members reads back unchanged"
    );
    assert_eq!(admitted(&without), (None, Value::Null));
    scene.close();
}

#[test]
fn hash_not_64_lower_hex_refused() {
    let scene = Scene::direct(json!([pick()]));
    scene.run(&[]);
    let stored = serde_json::to_value(scene.last_admission()).expect("the record encodes");
    read(&stored)
        .validate()
        .expect("the record as written validates");
    for hash in [
        "A".repeat(64),
        "a".repeat(63),
        "a".repeat(65),
        format!("{}g", "a".repeat(63)),
        String::new(),
        format!("sha256:{}", "a".repeat(64)),
    ] {
        assert_refused(&stored, "content_hash", &Value::String(hash));
    }

    // Fix round 1: `variables` out of its form. An entry of a value's text whole, and one of a
    // longer text's SHA-256 and length.
    let whole = |name: &str| json!({"variable_name": name, "value_json": "\"item_a\""});
    let hashed = |sha256: String| {
        json!({"variable_name": "pick",
               "value_sha256": sha256, "value_bytes": 2000})
    };
    // As many entries as a package declares variables at most, in strictly ascending order.
    let most = (0..actingcommand_contract::MAX_PACKAGE_VARIABLES)
        .map(|index| whole(&format!("v{index:02}")))
        .collect::<Vec<_>>();
    let mut one_more = most.clone();
    one_more.push(whole("v99"));
    for variables in [json!(most), json!([hashed("a".repeat(64))])] {
        assert_in_form(&stored, "variables", &variables);
    }
    for variables in [
        // No entry.
        json!([]),
        // More than the largest number.
        json!(one_more),
        // Names not in strictly ascending order: descending, or one name twice.
        json!([whole("pick"), whole("order")]),
        json!([whole("pick"), whole("pick")]),
        // An entry with both forms, and one with neither.
        json!([{"variable_name": "pick", "value_json": "\"item_a\"",
                "value_sha256": "a".repeat(64), "value_bytes": 2000}]),
        json!([{"variable_name": "pick"}]),
        // A `value_sha256` that is not 64 lowercase hexadecimal digits.
        json!([hashed("A".repeat(64))]),
        json!([hashed("a".repeat(63))]),
        json!([hashed("a".repeat(65))]),
        json!([hashed(format!("{}g", "a".repeat(63)))]),
    ] {
        assert_refused(&stored, "variables", &variables);
    }
    scene.close();
}

// Fix round 1: a run the host starts itself, the instance's startup package (the same package
// directory), run on a key the test holds as a granted startup claim runs it
// (`run_startup_package_on_key_for_test`, as `gate_463_u1` runs one), from `home` to `result` in
// one tap: its admission record carries the content hash of its directory as containment measures
// it, and no `variables` (it was handed none).
#[test]
fn startup_package_run_records_admitted_content() {
    let root = TempDir::new().expect("state root");
    let package = root.path().join(TASK);
    write_package(&package, TASK, None, None);
    let device = Arc::new(FakeState::default());
    // A run on a device instance whose display is observed (Workflow #392 DA-A).
    device.physical_task_geometry.store(true, Ordering::Release);
    *device
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((2, 1));
    // `home` until the run's tap, then `result`.
    *device.pixel_script.lock().expect("pixel script") =
        Some(Arc::new(|_: usize, inputs: usize| {
            vec![(0, if inputs == 0 { HOME } else { RESULT })]
        }));
    let startup =
        ContainedTaskRequest::new(package.to_string_lossy().into_owned()).expect("startup package");
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0)))
            .with_startup_packages(BTreeMap::from([(INSTANCE.to_owned(), startup)])),
        Arc::new(FakeProvider::one(
            INSTANCE,
            instance_id(),
            Arc::clone(&device),
        )),
    )
    .expect("runtime host");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let acquire = runtime_request(
        &ids,
        RuntimeOperation::acquire_lease(INSTANCE, ids.mint_holder_id().expect("holder")),
    );
    let connection = ConnectionId::new(463).expect("connection");
    let granted = host
        .process_request_for_test(&acquire, connection)
        .expect("the key's receipt");
    let Some(RuntimeResult::LeaseGranted { token }) = granted.result() else {
        panic!("the key: {granted:?}");
    };
    let token = token.clone();
    let ended = host
        .run_startup_package_on_key_for_test(INSTANCE, acquire, token, connection)
        .expect("no Runtime failure");
    assert_eq!(ended, Ok(()), "the startup package completes");
    let (content_hash, variables) = admitted(&last_admission(&host));
    assert_eq!(content_hash, Some(measured(&package)));
    assert_eq!(
        variables,
        Value::Null,
        "a startup package is handed no value"
    );
    host.close().expect("close runtime host");
}

#[test]
fn direct_run_records_admitted_content() {
    let scene = Scene::direct(json!([pick()]));
    scene.run(&[]);
    let (first, _) = admitted(&scene.last_admission());
    assert_eq!(first, Some(measured(&scene.package())));
    // The package's content changes: its control declaration allows one step more (2 to 3),
    // which the run's one step does not reach.
    patch_control(&scene.package(), |control| {
        assert_eq!(control["max_steps"], json!(2), "setup: the fixture's steps");
        control["max_steps"] = json!(3);
    });
    scene.run(&[]);
    let (second, _) = admitted(&scene.last_admission());
    assert_eq!(second, Some(measured(&scene.package())));
    assert_ne!(
        second, first,
        "the record names the content the run was admitted with"
    );
    scene.close();
}

#[test]
fn direct_run_records_given_values() {
    let scene = Scene::direct(json!([pick(), order()]));
    scene.run(&[
        ("pick", json!("item_a")),
        ("order", json!(["item_b", "item_a"])),
    ]);
    let (_, variables) = admitted(&scene.last_admission());
    assert_eq!(
        variables,
        json!([
            {"variable_name": "order", "value_json": "[\"item_b\",\"item_a\"]"},
            {"variable_name": "pick", "value_json": "\"item_a\""}
        ])
    );
    scene.close();
}

#[test]
fn value_over_1024_recorded_by_sha256_and_length() {
    let long_keys = keys('k', 32, 64);
    let edge_keys = keys('e', 31, 30);
    let scene = Scene::direct(json!([
        listed("long_list", &long_keys),
        listed("edge_list", &edge_keys)
    ]));
    let long_value = json!(long_keys);
    let edge_value = json!(edge_keys);
    let long_text = serde_json::to_string(&long_value).expect("the long value's text");
    let edge_text = serde_json::to_string(&edge_value).expect("the edge value's text");
    assert_eq!(long_text.len(), 2145, "32 keys of 64 bytes");
    assert_eq!(edge_text.len(), 1024, "31 keys of 30 bytes");
    scene.run(&[("long_list", long_value), ("edge_list", edge_value)]);
    let (_, variables) = admitted(&scene.last_admission());
    assert_eq!(
        variables,
        json!([
            {"variable_name": "edge_list", "value_json": edge_text},
            {"variable_name": "long_list",
             "value_sha256": format!("{:x}", Sha256::digest(long_text.as_bytes())),
             "value_bytes": long_text.len()}
        ])
    );
    scene.close();
}

#[test]
fn run_without_values_writes_the_bytes_of_the_hash_alone() {
    let scene = Scene::direct(json!([pick()]));
    scene.run(&[]);
    let written = scene.last_admission();
    let recorded = fact(&written);
    let TaskSemanticFact::PackageAdmitted {
        response_deadline_monotonic_ms,
        ..
    } = recorded
    else {
        panic!("a package admission: {recorded:?}")
    };
    let alone = TaskSemanticFact::PackageAdmitted {
        package_label: TASK.to_owned(),
        task_label: "task".to_owned(),
        response_deadline_monotonic_ms: *response_deadline_monotonic_ms,
        content_hash: Some(measured(&scene.package())),
        variables: None,
    };
    assert_eq!(
        serde_json::to_vec(recorded).expect("the fact encodes"),
        serde_json::to_vec(&alone).expect("the fact encodes")
    );
    let members = serde_json::to_value(recorded).expect("the fact encodes");
    assert!(members.get("variables").is_none(), "{members}");
    scene.close();
}

#[test]
fn scheduled_run_records_values_it_read() {
    let scene = Scene::scheduled(json!([pick()]));
    // Nothing set: the dispatch hands its run the default `item_b`, which the run reads.
    let (_, context) = scene.evaluate_and_run(POLICY_NOW_UNIX_MS, 7, |_| {});
    let (content_hash, variables) = admitted(&scene.last_admission());
    assert_eq!(content_hash, Some(measured(&scene.package())));
    assert_eq!(
        variables,
        json!([{"variable_name": "pick", "value_json": "\"item_b\""}])
    );

    // `item_a` set through the Lab: the next dispatch hands it down and its run reads it.
    let set_at = context.admission().activity.next_eligible_unix_ms;
    scene.set_pick(json!("item_a"), set_at);
    let (_, context) = scene.evaluate_and_run(set_at, 8, |_| {});
    let (_, variables) = admitted(&scene.last_admission());
    assert_eq!(
        variables,
        json!([{"variable_name": "pick", "value_json": "\"item_a\""}])
    );

    // The package replaced between the next dispatch and its run: its declaration no longer
    // offers `item_a`. The dispatch's reason says it handed down `item_a`, set; the run read its
    // default, so its record lists no value.
    let at = context.admission().activity.next_eligible_unix_ms;
    let (chain, _) = scene.evaluate_and_run(at, 9, |package| {
        patch_control(package, |control| {
            control["variables"] = json!([pick_without_item_a()]);
        });
    });
    let parameters = chain
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == PARAMETERS)
        .expect("the dispatch's parameters");
    let handed = parameters
        .outcome
        .value("variable_values")
        .expect("the dispatch's variable values");
    assert_eq!(handed[0]["variable_source"], json!("set"), "{handed}");
    assert_eq!(handed[0]["value_json"], json!("\"item_a\""), "{handed}");
    let (content_hash, variables) = admitted(&scene.last_admission());
    assert_eq!(
        variables,
        Value::Null,
        "the run read no value of its dispatch"
    );
    assert_eq!(content_hash, Some(measured(&scene.package())));
    scene.close();
}

#[test]
fn public_projection_carries_neither_member() {
    let scene = Scene::direct(json!([pick()]));
    scene.run(&[("pick", json!("item_a"))]);
    let written = scene.last_admission();
    let (content_hash, variables) = admitted(&written);
    assert!(
        content_hash.is_some() && variables.is_array(),
        "the record carries both members"
    );
    let stored = serde_json::to_value(&written).expect("the record encodes");
    let without = read(&without_members(&stored));
    let projected = serde_json::to_vec(&written.public_projection()).expect("projection");
    assert_eq!(
        projected,
        serde_json::to_vec(&without.public_projection()).expect("projection"),
        "the projection of the record is the projection without its two members"
    );
    let text = String::from_utf8(projected).expect("the projection is text");
    assert!(text.contains("package_admitted"), "{text}");
    scene.close();
}
