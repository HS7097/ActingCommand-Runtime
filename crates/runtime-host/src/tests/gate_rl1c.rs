// SPDX-License-Identifier: AGPL-3.0-only

// ROUT-L RL-1c (#411; coordinator work order `rd-1027/brief-rl1c.md`, tests C-2 and C-5; board
// #411 6432236193 and 6435964241): the readers of a composer-mode Runtime. The host runs in
// composer mode (no configured catalog) over one neutral game root on disk whose `schedule.json`
// names one rule of a task package that this Runtime's kernel refuses with a suspend code
// (`recognition_sample_budget_invalid`, the SR-3 fixture package as a ZIP container), a manual
// Runtime clock and a fake instance.
//
// - C-5: the task package admission after start runs against the composed procedure bindings:
//   it waits for the scheduler workbench's first check and then suspends the composed package,
//   so the next evaluation defers the rule's pair with `pack_admission_suspended`. Before RL-1c it
//   ran at once over the bindings the start had (none composed) and suspended nothing.
// - C-2: `actingd suspended` answers in composer mode: the report of the composition the
//   configuration names (`compose_configured_standard_packs`) lists the suspended package under
//   `packs`, with its content hash measured now at the place the composed procedure binds.

use super::gate_386_sr3::{
    COMMIT, REFUSED_ID, SUSPEND_CODE, refused_package, suspension_of, write_package,
};
use super::gate_rl1b::{GAME_A, POOL_A, SERVER_A, bundle, resources, schedule_a, write_json};
use super::*;
use crate::pack_admission::PackAdmissionHold;
use serde_json::json;

/// The composed rule and its package: a package file, so the registry's directory (the package
/// id) names a ZIP container.
const RULE: &str = "fixture.refused";
const PACKAGE: &str = "fixture.refused.zip";
const INSTANCE: &str = POLICY_INSTANCE_ALIAS;
/// Bounds only a broken run: the background admission answers on a channel.
const GATE_WAIT: Duration = Duration::from_secs(30);

/// The fixture's facts with the one instance bound to game A on server A, able to run the
/// Runtime's package operation, and no recorded outcome.
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

/// One composer-mode host over its game root, with its clock and the package's content hash.
struct Scene {
    host: RuntimeHost,
    clock: Arc<ManualRuntimeClock>,
    state: TempDir,
    packs: TempDir,
    facts: EvaluationFacts,
    resources: EvaluationResources,
    /// The SHA-256 of the package file, in lowercase hex.
    hash: String,
}

impl Scene {
    /// Places game A's root (its bundle, `schedule.json` with the one rule, and the package) and
    /// starts the host in composer mode with the admission after start held by `hold`.
    fn start(hold: Arc<PackAdmissionHold>) -> Self {
        let state = TempDir::new().expect("state root");
        let packs = TempDir::new().expect("standard-pack root");
        let game = packs.path().join(GAME_A);
        write_json(
            &game.join("bundle.json"),
            &bundle(GAME_A, SERVER_A, &[PACKAGE]),
        );
        write_json(
            &game.join("schedule.json"),
            &schedule_a(&[(RULE, PACKAGE, 100)]),
        );
        let hash = write_package(&game.join(PACKAGE), &refused_package(100));
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let facts = facts();
        let host = RuntimeHost::start(
            host_config(&state, &packs, &facts, &clock).with_pack_admission_hold_for_test(hold),
            Arc::new(FakeProvider::from_entries([(
                INSTANCE.to_owned(),
                instance_id(),
                Arc::new(FakeState::default()),
            )])),
        )
        .expect("start the composer-mode host");
        Self {
            host,
            clock,
            state,
            packs,
            facts,
            resources: resources(&[POOL_A]),
            hash,
        }
    }

    /// The configuration of the host: composer mode over the standard-pack root.
    fn config(&self) -> RuntimeHostConfig {
        host_config(&self.state, &self.packs, &self.facts, &self.clock)
    }

    /// One cycle at the fixture's time with the monotonic clock at `monotonic_ms`.
    fn cycle(&self, monotonic_ms: u64) -> PolicyCycle {
        self.clock.set_unix_ms(POLICY_NOW_UNIX_MS);
        self.clock.set_monotonic_ms(monotonic_ms);
        let sample_capacity = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample_capacity().expect("capacity sample");
        self.host
            .evaluate_policy_cycle_with_test_inputs(
                &self.facts,
                &self.resources,
                EvaluationTime {
                    unix_ms: POLICY_NOW_UNIX_MS,
                    monotonic_ms: POLICY_NOW_UNIX_MS,
                },
                11,
                PolicyTrigger::Recovery,
            )
            .unwrap_or_else(|error| panic!("cycle at {monotonic_ms}: {error}"))
    }

    /// The first cycle composes the rules; once the admission after start has finished, the
    /// second cycle's evaluation.
    fn suspend(&self, release: &mpsc::Sender<()>, finished: &mpsc::Receiver<()>) -> PolicyCycle {
        release.send(()).expect("release the admission after start");
        self.cycle(0);
        finished
            .recv_timeout(GATE_WAIT)
            .expect("the admission after start finishes after the first check");
        self.cycle(1_000)
    }
}

fn host_config(
    state: &TempDir,
    packs: &TempDir,
    facts: &EvaluationFacts,
    clock: &Arc<ManualRuntimeClock>,
) -> RuntimeHostConfig {
    config(state)
        .with_policy_inputs(PolicyInputSnapshot::new(facts.clone(), policy_resources()))
        .with_runtime_clock(clock.clone())
        .with_runtime_commit(COMMIT)
        .with_standard_packs_root(Some(packs.path().to_owned()))
        .with_standard_pack_schedule(true)
}

/// C-5 (work order item 4): in composer mode the task package admission after start waits for
/// the workbench's first check and admits the procedure bindings it installs: the composed
/// package, refused with a suspend code, is suspended, and the next evaluation defers the rule's
/// pair with `pack_admission_suspended` naming the package and its refusal; no intent is made.
#[test]
fn rl1c_c5_the_admission_after_start_admits_the_composed_procedures() {
    let (hold, release, finished) = PackAdmissionHold::open();
    let scene = Scene::start(hold);
    let second = scene.suspend(&release, &finished);
    let evaluation = second.evaluation.as_ref().expect("an evaluation");
    assert!(
        !evaluation
            .dispatch_intents
            .iter()
            .any(|intent| intent.task_id == RULE),
        "the suspended package's rule is not dispatched: {evaluation:#?}"
    );
    let suspension = suspension_of(evaluation, RULE, INSTANCE).unwrap_or_else(|| {
        panic!("the admission after start suspended the composed package: {evaluation:#?}")
    });
    assert_eq!(
        suspension.value("package_id"),
        Some(&json!(REFUSED_ID)),
        "{suspension}"
    );
    assert_eq!(
        suspension.value("failure_code"),
        Some(&json!(SUSPEND_CODE)),
        "{suspension}"
    );
    assert_eq!(
        suspension.value("actual_digest"),
        Some(&json!(format!("sha256:{}", scene.hash))),
        "the composed package was measured after its refusal: {suspension}"
    );
    scene.host.close().expect("close the host");
}

/// C-2 (work order item 2): `actingd suspended` in composer mode reads the composition the
/// configuration names instead of answering `suspended_policy_unconfigured`. After the scene of
/// C-5, the report of `compose_configured_standard_packs` lists the suspended package under
/// `packs` with its content hash measured now at the place the composed procedure binds, which
/// only the composition's catalog and bindings name.
#[test]
fn rl1c_c2_the_suspended_report_answers_against_the_composition() {
    let (hold, release, finished) = PackAdmissionHold::open();
    let scene = Scene::start(hold);
    let second = scene.suspend(&release, &finished);
    let evaluation = second.evaluation.as_ref().expect("an evaluation");
    assert!(
        suspension_of(evaluation, RULE, INSTANCE).is_some(),
        "{evaluation:#?}"
    );
    let config = scene.config();
    // The roots stay until the report has measured the package.
    let Scene {
        host,
        hash,
        state,
        packs,
        ..
    } = scene;
    host.close().expect("close the host");

    let schedule = crate::compose_configured_standard_packs(&config)
        .unwrap_or_else(|error| panic!("compose the configuration's rules: {error}"));
    assert!(schedule.problems().is_empty(), "{:?}", schedule.problems());
    let report = crate::composed_suspension_report(&crate::ComposedSuspensionReportRequest {
        config_path: Path::new("actingd.config.json"),
        config_modified_unix_ms: None,
        schedule: &schedule,
    })
    .unwrap_or_else(|error| panic!("actingd suspended in composer mode: {error}"));
    let prefix = hash.chars().take(12).collect::<String>();
    assert_eq!(
        report["packs"],
        json!([{
            "package_id": REFUSED_ID,
            "failure_code": SUSPEND_CODE,
            "runtime_commit": COMMIT,
            "paused_hash_prefix": prefix,
            "current_hash_prefix": prefix,
            "instances": [INSTANCE],
        }]),
        "{report:#}"
    );
    drop(state);
    drop(packs);
}
