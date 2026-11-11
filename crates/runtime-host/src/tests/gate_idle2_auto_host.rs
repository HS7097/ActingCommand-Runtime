// SPDX-License-Identifier: AGPL-3.0-only

//! IDLE-2 (Alice's words of 11-11: 「根据任务间隔时间自动选择是关闭模拟器还是关闭应用还是回到主页面」; the
//! coordinator's rulings on Workflow #386, comment 6787682270; #419, Alice: 「一个模拟器故障之后不能崩掉
//! 整个 acting command」): `idle.action` `auto` through the host, on a manual clock, with the scripted
//! emulator of `gate_idle1p` (X: `start_emulator`, its configured emulator start and a startup
//! package) and the neutral standard-pack root of `gate_373_nav3a` (game `neutral`, server `test`,
//! the policy identity the test gives X).
//!
//! - A gap of 20 min rests X: the return-home package its game declares runs once on the idle
//!   action's claim, one `instance_idle_action` (`run_idle_action`, `idle_action` `return_home`)
//!   records it, nothing is kept in `host.idle_state`, and a fresh plan of the same gap takes no
//!   second claim.
//! - A rest whose package fails is recorded as the host run's failure, climbs no ladder (X's stuck
//!   recovery on) and is not tried again in its stretch; the Runtime goes on.
//! - A game root that declares no return-home package for the server: no run, and one
//!   `instance_idle_action_unsupported` (`return_home`, `maintenance`) across two stretches.
//! - A gap of 4 h stops X's emulator; `host.idle_state` names `stop_emulator`, never `auto`; a
//!   restart under `auto` restores X closed and does not wake it at once; its wake comes at its
//!   time.
//!
//! Every wait returns as soon as what it waits for has happened; `GATE_WAIT` only bounds a broken
//! run.

use super::gate_369_s6c::{claims_after, connectivity, head};
use super::gate_373_nav3a::{
    AWAY, LEAD, LEAD_STEPS, MENU, RETURN_HOME, StandardRoot, route, screens,
};
use super::gate_idle1p::{
    NOW_UNIX_MS, WAKE_UNIX_MS, X_ALIAS, kept_rows, no_plan, stops, wake_records, x_plan_at, x_row,
    x_state,
};
use super::instance_workers::{all_events, phases};
use super::*;
use crate::codes::{IdleCode, IdleLocation};
use crate::idle_action::IdlePlan;
use actingcommand_contract::{InstanceStuckRecovery, RuntimeConfigManifest, RuntimeLifecyclePhase};
use actingcommand_host_metrics::NetworkConnectivity;
use actingcommand_runtime_config::{
    DefaultsFile, annotate_settable, mechanism_rows, render_parameters,
};
use serde_json::{Value, json};

const GATE_WAIT: Duration = Duration::from_secs(60);
const MINUTE_MS: u64 = 60_000;
/// The rest's tier: 20 min, between `idle.return_home_min_gap_ms` and `idle.min_gap_ms`.
const REST_GAP_MS: u64 = 20 * MINUTE_MS;

fn instance_text(instance: InstanceId) -> String {
    match serde_json::to_value(instance).expect("instance id encodes") {
        Value::String(text) => text,
        other => panic!("an instance id is a string: {other}"),
    }
}

/// The configuration actingd assembles for X over the defaults file: X's entry names
/// `start_emulator` and the idle action `auto` with the default thresholds.
fn auto_manifest(x: InstanceId) -> RuntimeConfigManifest {
    let id = instance_text(x);
    let file = json!({"instances": [{
        "instance_id": id,
        "start_emulator": true,
        "idle": {"action": "auto"}
    }]});
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = render_parameters(
        &mechanism_rows(),
        std::slice::from_ref(&id),
        &defaults,
        &file,
        &|_| None,
    )
    .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// A start of the Runtime on `root` with X alone under `auto`, on `clock`, with the standard-pack
/// root `packs` when given; X's stuck recovery on only when `stuck_recovery`.
fn start_auto(
    root: &TempDir,
    clock: &Arc<ManualRuntimeClock>,
    x: InstanceId,
    state: &Arc<FakeState>,
    packs: Option<PathBuf>,
    stuck_recovery: bool,
) -> RuntimeHost {
    let package_path = root.path().join("idle2-startup-package.zip");
    fs::write(&package_path, neutral_contained_task_package(true)).expect("write startup package");
    let startup = ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
        .and_then(|request| request.with_response_deadline_ms(60_000))
        .expect("startup package request");
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let config = config(root)
        .with_runtime_clock(clock.clone())
        .with_stuck_recovery(BTreeMap::from([(
            X_ALIAS.to_owned(),
            InstanceStuckRecovery {
                enabled: stuck_recovery,
                cooldown_secs: 600,
            },
        )]))
        .with_emulator_autostart(vec![X_ALIAS.to_owned()])
        .with_network_connectivity_for_test(read)
        .with_startup_packages(BTreeMap::from([(X_ALIAS.to_owned(), startup)]))
        .with_standard_packs_root(packs)
        .with_config_manifest(auto_manifest(x));
    RuntimeHost::start(
        config,
        Arc::new(FakeProvider::from_entries([(
            X_ALIAS.to_owned(),
            x,
            Arc::clone(state),
        )])),
    )
    .expect("the start")
}

/// X's plan evaluated at `at`: no task due, the next one `gap` away.
fn x_plan_in(at: u64, gap: u64) -> BTreeMap<String, IdlePlan> {
    BTreeMap::from([(
        X_ALIAS.to_owned(),
        IdlePlan {
            evaluated_at_unix_ms: at,
            due_now: false,
            next_wake_unix_ms: Some(at + gap),
            reading_due_unix_ms: None,
        },
    )])
}

/// X's policy identity: the neutral game and server of the standard-pack root.
fn x_identity() -> BTreeMap<String, (String, String)> {
    BTreeMap::from([(
        X_ALIAS.to_owned(),
        ("neutral".to_owned(), "test".to_owned()),
    )])
}

/// A standard-pack root under `base` whose game declares the return-home package of the server
/// `test` when `declared` (otherwise it lists another package and declares none).
fn packs(base: &Path, declared: bool) -> PathBuf {
    let mut root = StandardRoot::new(base);
    if declared {
        route(RETURN_HOME, "away", "menu", LEAD_STEPS, None).write(&root.list(RETURN_HOME));
        root.return_home(RETURN_HOME);
    } else {
        route(LEAD, "away", "menu", LEAD_STEPS, None).write(&root.list(LEAD));
    }
    root.write_bundle();
    root.root
}

/// X's idle records after `after`, in order: each `instance_idle_action` and
/// `instance_idle_action_unsupported` with its operation and `idle_action`, and for the warning
/// its `member`.
fn idle_outcomes(host: &RuntimeHost, x: InstanceId, after: u64) -> Vec<String> {
    let events = all_events(host);
    phases(&events)
        .into_iter()
        .filter(|(event, _)| event.sequence() > after && event.links().instance_id() == Some(&x))
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::DeviceStateObserved { outcome }
                if outcome.is(IdleCode::Taken) || outcome.is(IdleCode::Unsupported) =>
            {
                let text = |key: &str| {
                    outcome
                        .value(key)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned()
                };
                Some(
                    [
                        outcome.code().as_str().to_owned(),
                        text("operation"),
                        text("idle_action"),
                        text("member"),
                    ]
                    .join(" ")
                    .trim_end()
                    .to_owned(),
                )
            }
            _ => None,
        })
        .collect()
}

/// The rest's record: `instance_idle_action` at `run_idle_action`, `idle_action` `return_home`.
fn rest_record() -> String {
    format!(
        "{} {} return_home",
        IdleCode::Taken.as_str(),
        IdleLocation::Action.as_str()
    )
}

/// Whether X's events after `after` admitted the package `package_id`.
fn admitted(host: &RuntimeHost, after: u64, package_id: &str) -> bool {
    all_events(host).iter().any(|event| {
        event.sequence() > after
            && matches!(
                event.payload(),
                EventPayload::Task(TaskPayload::Semantic(payload))
                    if matches!(
                        payload.fact(),
                        TaskSemanticFact::PackageAdmitted { package_label, .. }
                            if package_label == package_id
                    )
            )
    })
}

/// Whether a recovery ladder started, or was suppressed, after `after`.
fn ladder_after(host: &RuntimeHost, after: u64) -> bool {
    let events = all_events(host);
    phases(&events).into_iter().any(|(event, phase)| {
        event.sequence() > after
            && matches!(
                phase,
                RuntimeLifecyclePhase::RecoveryLadderStarted { .. }
                    | RuntimeLifecyclePhase::RecoveryLadderSuppressed { .. }
            )
    })
}

#[test]
fn idle2_auto_g_a_rest_runs_the_declared_return_home_package_once_and_records_it() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let x = instance_id();
    let state = x_state();
    screens(&state, vec![AWAY, MENU]);
    let host = start_auto(
        &root,
        &clock,
        x,
        &state,
        Some(packs(root.path(), true)),
        false,
    );
    let before = head(&host);

    host.check_idle_instances_with_identities_for_test(
        &x_plan_in(NOW_UNIX_MS, REST_GAP_MS),
        &x_identity(),
    )
    .expect("the idle check");
    assert_eq!(
        host.await_idle_action_ends_for_test(1, GATE_WAIT),
        Some(1),
        "X's rest ended"
    );
    assert_eq!(idle_outcomes(&host, x, before), [rest_record()]);
    assert!(
        admitted(&host, before, RETURN_HOME),
        "the rest ran the return-home package the game declares"
    );
    assert_eq!(stops(&state), 0, "a rest stops nothing");
    assert_eq!(
        kept_rows(&host),
        Some(Vec::new()),
        "a rested instance is not kept in host.idle_state"
    );

    // A fresh plan with the same task: one rest per idle stretch, so no second claim.
    let after_rest = head(&host);
    let later = NOW_UNIX_MS + MINUTE_MS;
    clock.set_unix_ms(later);
    host.check_idle_instances_with_identities_for_test(
        &x_plan_in(later, REST_GAP_MS - MINUTE_MS),
        &x_identity(),
    )
    .expect("the idle check after the rest");
    assert!(
        claims_after(&host, x, after_rest).is_empty(),
        "no second rest in the stretch"
    );
    assert!(
        host.fatal_error().expect("runtime health").is_none(),
        "the Runtime goes on"
    );
    host.close().expect("close the host");
}

#[test]
fn idle2_auto_h_a_failed_rest_is_recorded_climbs_no_ladder_and_is_not_retried() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let x = instance_id();
    let state = x_state();
    // The screen never leaves `away`: the package's one step never reaches its page.
    screens(&state, vec![AWAY]);
    let host = start_auto(
        &root,
        &clock,
        x,
        &state,
        Some(packs(root.path(), true)),
        true,
    );
    let before = head(&host);

    host.check_idle_instances_with_identities_for_test(
        &x_plan_in(NOW_UNIX_MS, REST_GAP_MS),
        &x_identity(),
    )
    .expect("the idle check");
    assert_eq!(
        host.await_idle_action_ends_for_test(1, GATE_WAIT),
        Some(1),
        "X's rest ended"
    );
    let events = all_events(&host);
    let failed = events.iter().any(|event| {
        event.sequence() > before
            && event.links().instance_id() == Some(&x)
            && matches!(
                event.event_type(),
                EventType::TaskFailed | EventType::RuntimeFailed
            )
    });
    assert!(failed, "the rest's failure is recorded under X");
    assert!(
        idle_outcomes(&host, x, before).is_empty(),
        "no instance_idle_action says a failed rest ran"
    );
    assert!(
        !ladder_after(&host, before),
        "a failed rest climbs no ladder"
    );

    // A fresh plan of the same task: the failed rest is the stretch's rest.
    let later = NOW_UNIX_MS + MINUTE_MS;
    clock.set_unix_ms(later);
    host.check_idle_instances_with_identities_for_test(
        &x_plan_in(later, REST_GAP_MS - MINUTE_MS),
        &x_identity(),
    )
    .expect("the idle check after the failed rest");
    assert_eq!(
        claims_after(&host, x, before).len(),
        1,
        "the rest's own claim only: no retry and no ladder's claim"
    );
    assert!(
        host.fatal_error().expect("runtime health").is_none(),
        "X's failed rest latched the Runtime"
    );
    assert_eq!(kept_rows(&host), Some(Vec::new()));
    host.close().expect("close the host");
}

#[test]
fn idle2_auto_i_without_a_declared_return_home_package_no_rest_runs_and_one_warning_says_so() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let x = instance_id();
    let state = x_state();
    screens(&state, vec![AWAY, MENU]);
    let host = start_auto(
        &root,
        &clock,
        x,
        &state,
        Some(packs(root.path(), false)),
        false,
    );
    let before = head(&host);

    host.check_idle_instances_with_identities_for_test(
        &x_plan_in(NOW_UNIX_MS, REST_GAP_MS),
        &x_identity(),
    )
    .expect("the first stretch's check");
    assert_eq!(host.await_idle_action_ends_for_test(1, GATE_WAIT), Some(1));

    // Work holds X (its admission guard, as a keyless observe does), which ends the stretch; a
    // plan made after it asks for a rest again.
    let admission = host
        .instance_admission_for_test(X_ALIAS)
        .expect("X's admission guard");
    let worked = NOW_UNIX_MS + MINUTE_MS;
    clock.set_unix_ms(worked);
    {
        let _work = admission.lock().expect("hold X's admission guard");
        host.check_idle_instances_with_identities_for_test(
            &x_plan_in(worked, REST_GAP_MS),
            &x_identity(),
        )
        .expect("the check during the work");
    }
    let settled = worked + MINUTE_MS;
    clock.set_unix_ms(settled);
    host.check_idle_instances_with_identities_for_test(
        &x_plan_in(settled, REST_GAP_MS),
        &x_identity(),
    )
    .expect("the second stretch's check");
    assert_eq!(host.await_idle_action_ends_for_test(2, GATE_WAIT), Some(2));

    let warning = format!(
        "{} {} return_home maintenance",
        IdleCode::Unsupported.as_str(),
        IdleLocation::Action.as_str()
    );
    assert_eq!(
        idle_outcomes(&host, x, before),
        [warning],
        "one warning per start and instance, and no rest recorded"
    );
    assert!(
        !admitted(&host, before, LEAD),
        "no package runs for a rest the game does not declare"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

#[test]
fn idle2_auto_j_a_restart_keeps_the_tier_taken_and_does_not_wake_it_at_once() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let x = instance_id();
    // The first start: a gap of 4 h stops X's emulator (an unscripted Stop is confirmed).
    let state = x_state();
    let first = start_auto(&root, &clock, x, &state, None, false);
    first
        .check_idle_instances_for_test(&x_plan_at(NOW_UNIX_MS))
        .expect("the idle check");
    assert_eq!(first.await_idle_action_ends_for_test(1, GATE_WAIT), Some(1));
    assert_eq!(stops(&state), 1, "auto took stop_emulator for a gap of 4 h");
    assert_eq!(
        kept_rows(&first),
        Some(vec![x_row(Some(WAKE_UNIX_MS))]),
        "host.idle_state names the tier taken, stop_emulator, never auto"
    );
    first.close().expect("close the first host");

    // The restart, well before the wake, still under `auto`.
    let restarted_at = NOW_UNIX_MS + 30 * MINUTE_MS;
    clock.set_unix_ms(restarted_at);
    let state = x_state();
    let host = start_auto(&root, &clock, x, &state, None, false);
    let after_start = head(&host);
    assert_eq!(
        kept_rows(&host),
        Some(vec![x_row(Some(WAKE_UNIX_MS))]),
        "the restart restores X closed with its wake"
    );
    host.check_idle_instances_for_test(&no_plan())
        .expect("the check before any plan");
    host.check_idle_instances_for_test(&x_plan_at(restarted_at))
        .expect("the check with a plan");
    assert!(
        idle_outcomes(&host, x, after_start).is_empty(),
        "auto beside the kept stop_emulator is no switch: no wake at once, no action"
    );
    assert_eq!(stops(&state), 0);

    // The wake at its time.
    clock.set_unix_ms(WAKE_UNIX_MS);
    host.check_idle_instances_for_test(&no_plan())
        .expect("the check at the wake");
    let wake = wake_records()[0].clone();
    assert_eq!(
        idle_outcomes(&host, x, after_start),
        [format!("{wake} stop_emulator")],
        "one wake, at its time"
    );
    host.close().expect("close the restarted host");
}
