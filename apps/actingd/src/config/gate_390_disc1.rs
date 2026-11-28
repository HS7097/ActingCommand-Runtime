// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 DISC-1 (coordinator ruling 2 of wave 18; contracts/v012-register.md §1.89; host
// gate 99): when the instance discovery is unavailable at start, the start tries it again, at
// most `startup_discovery.attempts` times and `startup_discovery.retry_interval_ms` apart on the
// Runtime clock. It records one warning per failed attempt that is tried again, goes on normally
// once an attempt succeeds, and ends exactly as before (the provider startup failure, then the
// fatal `instance_discovery_unavailable`) once the last attempt fails. Each warning is also
// written to actingd's stderr as its daemon lines (coordinator, 10-16 23:0x: Fail Loud for
// people too). Both values come from runtime-config and are refused out of their bounds.
//
// The configuration goes through actingd's own assembly: the file's section, the defaults file
// and the manifest the host starts with. The discovery is scripted, and the Runtime clock is a
// manual one that the provider startup's waits move forward, so the test spends no time and
// reads every wait.

use super::provider_startup::bind_discovered_instances;
use super::*;
use actingcommand_contract::outcome::Category;
use actingcommand_contract::{
    EventPayload, EventSeverity, EventType, ProviderStartupObservation, ProviderStartupStage,
};
use actingcommand_device::{
    DeviceDiscoveryProcess, DeviceError, DeviceResult, DiscoveredMumuInstance, MumuDiscoveryReport,
    MumuManagerReads, MumuManagerSource, MumuManagerVersion,
};
use actingcommand_execution_kernel::InstanceDiscoveryFailure;
use actingcommand_ledger::{GlobalLedger, GlobalLedgerEvidenceConfig, PersistedEvent};
use actingcommand_runtime_host::{
    RuntimeClock, RuntimeClockSample, RuntimeHost, RuntimeHostResult,
};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use tempfile::TempDir;

// The wall-clock reading of the manual clock before any wait, in milliseconds.
const START_UNIX_MS: u64 = 1_760_000_000_000;

type DiscoveryAnswer = Result<MumuDiscoveryReport, Box<InstanceDiscoveryFailure>>;

// The Runtime clock of the start. Only a wait moves it: each wait is kept and moves the clock
// forward by its duration at once (test plan E-3).
#[derive(Default)]
struct ManualClock {
    elapsed_ms: AtomicU64,
    waits: Mutex<Vec<Duration>>,
}

impl ManualClock {
    fn waits(&self) -> Vec<Duration> {
        self.waits.lock().expect("the waits").clone()
    }
}

impl RuntimeClock for ManualClock {
    fn sample(&self) -> RuntimeHostResult<RuntimeClockSample> {
        let elapsed_ms = self.elapsed_ms.load(Ordering::SeqCst);
        Ok(RuntimeClockSample {
            unix_ms: START_UNIX_MS + elapsed_ms,
            monotonic_ms: elapsed_ms,
        })
    }

    fn pause(&self, duration: Duration) {
        self.waits.lock().expect("the waits").push(duration);
        let ms = u64::try_from(duration.as_millis()).expect("a wait in milliseconds");
        self.elapsed_ms.fetch_add(ms, Ordering::SeqCst);
    }
}

// An actingd configuration without instances whose `startup_discovery` section is `section`.
fn configuration(section: Value) -> ActingdConfigFile {
    let mut config = serde_json::from_value::<ActingdConfigFile>(json!({
        "schema_version": CONFIG_SCHEMA_VERSION,
        "state_root": "state",
        "bind_host": "127.0.0.1",
        "secret_fingerprint_salt": "0123456789abcdef",
        "startup_discovery": section,
        "instances": []
    }))
    .expect("typed config");
    config.source_root = PathBuf::from("fixture-root");
    config
}

// The discovery answer of an emulator that has not started yet.
fn unavailable() -> DiscoveryAnswer {
    Err(Box::new(InstanceDiscoveryFailure {
        code: SharedCode::InstanceDiscoveryUnavailable.as_str(),
        error: DeviceError::fatal("fixture MuMuManager answered without an instance list"),
    }))
}

// The discovery answer once the emulator runs: one running instance.
fn discovered() -> DiscoveryAnswer {
    let version = MumuManagerVersion::parse("6.5.7.0").expect("a four-part version");
    Ok(MumuDiscoveryReport {
        source: MumuManagerSource::ExplicitRoot,
        mumu_manager_path: PathBuf::from("C:/fixture/nx_main/MuMuManager.exe"),
        version,
        registry_display_version: None,
        instances: vec![DiscoveredMumuInstance {
            install_root: PathBuf::from("C:/fixture"),
            mumu_manager_path: PathBuf::from("C:/fixture/nx_main/MuMuManager.exe"),
            adb_path: PathBuf::from("C:/fixture/nx_main/adb.exe"),
            instance_index: 0,
            instance_name: "neutral-a".to_owned(),
            adb_host: Some("127.0.0.1".to_owned()),
            adb_port: Some(16384),
            running: true,
            player_state: Some("start_finished".to_owned()),
            android_version: Some("12".to_owned()),
            mumu_version: version,
        }],
    })
}

// What one start did: its result, how many discovery attempts it made, the waits of the provider
// startup, what it wrote to actingd's stderr and the ledger it left.
struct Start {
    result: RuntimeHostResult<()>,
    attempts: usize,
    waits: Vec<Duration>,
    daemon_log: String,
    events: Vec<PersistedEvent>,
}

// Starts a Runtime with the manifest actingd assembles from `section`, whose provider startup runs
// the discovery-bound part with the scripted `answers`, then opens the ledger it left.
fn start(section: Value, answers: Vec<DiscoveryAnswer>) -> Start {
    let root = TempDir::new().expect("tempdir");
    let state_root = root.path().join("state");
    let assembly = configuration(section)
        .assemble()
        .expect("the configuration assembles");
    let clock = Arc::new(ManualClock::default());
    let host = RuntimeHostConfig::new(&state_root, b"0123456789abcdef")
        .with_frame_retention_enabled(false)
        .with_runtime_clock(Arc::clone(&clock) as Arc<dyn RuntimeClock>)
        .with_config_manifest(assembly.manifest);
    let mut answers = VecDeque::from(answers);
    let mut attempts = 0;
    let mut discover = || {
        attempts += 1;
        answers.pop_front().expect("no attempt beyond the script")
    };
    let mut waits = Vec::new();
    let mut daemon_log = Vec::new();
    let started = RuntimeHost::start_with_provider(host, |startup| {
        let specs =
            bind_discovered_instances(startup, None, &mut discover, &mut daemon_log, Vec::new());
        // The waits of the provider startup, before any thread of the Runtime runs.
        waits = clock.waits();
        let registry = ExecutionBackendRegistry::from_assembly(ProviderAssembly {
            instances: specs?,
            vision: None,
            discovery: None,
        })
        .expect("a registry without instances");
        Ok(Arc::new(registry) as Arc<dyn ExecutionBackendProvider>)
    });
    let result = started.map(|host| host.close().expect("close the host"));
    let events =
        GlobalLedger::open_evidence(GlobalLedgerEvidenceConfig::new(&state_root), |_| None)
            .expect("the startup ledger is readable")
            .events()
            .to_vec();
    Start {
        result,
        attempts,
        waits,
        daemon_log: String::from_utf8(daemon_log).expect("daemon lines are text"),
        events,
    }
}

// The provider startup observations of the ledger, in order, with their records.
fn observations(events: &[PersistedEvent]) -> Vec<(&PersistedEvent, &ProviderStartupObservation)> {
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Provider(payload) => Some((event, &payload.record.observation)),
            _ => None,
        })
        .collect()
}

// Each recorded retry warning, in order, is a warning record of `instance_discovery_retried` for
// attempt 1, 2, ... with the configured attempts and wait, caused by the attempt's
// `instance_discovery_unavailable`, and the warnings lie `retry_interval_ms` apart on the clock.
fn assert_retry_warnings(events: &[PersistedEvent], count: u32, attempts: u32, interval_ms: u64) {
    let warnings = observations(events)
        .into_iter()
        .filter_map(|(event, observation)| match observation {
            ProviderStartupObservation::DiscoveryRetried { outcome } => Some((event, outcome)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        warnings.len(),
        usize::try_from(count).expect("a count"),
        "one warning per failed attempt that is tried again"
    );
    for (attempt, (event, outcome)) in (1..=count).zip(&warnings) {
        assert_eq!(
            event.severity(),
            EventSeverity::Warning,
            "attempt {attempt}"
        );
        assert_eq!(outcome.code().as_str(), "instance_discovery_retried");
        assert_eq!(outcome.category(), Category::Warning);
        assert_eq!(outcome.value("attempts"), Some(&json!(attempt)));
        assert_eq!(outcome.value("limit_count"), Some(&json!(attempts)));
        assert_eq!(outcome.value("retry_after_ms"), Some(&json!(interval_ms)));
        let causes = outcome
            .causes()
            .iter()
            .map(|link| link.outcome().code().as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            causes,
            ["instance_discovery_unavailable"],
            "attempt {attempt}"
        );
    }
    for pair in warnings.windows(2) {
        assert_eq!(
            pair[1].0.timestamp_unix_ms() - pair[0].0.timestamp_unix_ms(),
            interval_ms,
            "the attempts lie one wait apart on the Runtime clock"
        );
    }
}

// Each retry warning is also on actingd's stderr, in order: its developer line names the code at
// level WARN with the attempt, the configured attempts and the wait, and its `OUTCOME actingd:`
// line carries the same warning; nothing else is written there.
fn assert_retry_lines(daemon_log: &str, count: u32, attempts: u32, interval_ms: u64) {
    let lines = daemon_log.lines().collect::<Vec<_>>();
    assert_eq!(
        lines.len(),
        usize::try_from(count * 2).expect("a count"),
        "two daemon lines per retry: {daemon_log}"
    );
    for (attempt, pair) in (1..=count).zip(lines.chunks(2)) {
        let values = format!("limit_count={attempts} retry_after_ms={interval_ms}");
        let developer =
            format!("WARN actingd: instance_discovery_retried attempts={attempt} {values}");
        assert!(pair[0].starts_with(&developer), "{}", pair[0]);
        let envelope: Value = serde_json::from_str(
            pair[1]
                .strip_prefix("OUTCOME actingd: ")
                .expect("the OUTCOME line"),
        )
        .expect("the envelope");
        assert_eq!(envelope["code"], "instance_discovery_retried", "{envelope}");
        assert_eq!(envelope["category"], "warning", "{envelope}");
        assert_eq!(envelope["values"]["attempts"], attempt, "{envelope}");
        assert_eq!(envelope["values"]["limit_count"], attempts, "{envelope}");
        assert_eq!(
            envelope["values"]["retry_after_ms"], interval_ms,
            "{envelope}"
        );
    }
}

// Success on attempt k: two attempts find the discovery unavailable and the third succeeds. The
// start waits twice for the configured interval, records a warning for each failed attempt, then
// records the discovery and goes on to `runtime.started`; nothing fails.
#[test]
fn disc1_a_start_whose_discovery_succeeds_on_attempt_three_goes_on_after_two_waits() {
    let started = start(
        json!({ "attempts": 4, "retry_interval_ms": 20_000 }),
        vec![unavailable(), unavailable(), discovered()],
    );
    if let Err(error) = &started.result {
        panic!("the start goes on: {error}");
    }
    assert_eq!(started.attempts, 3);
    assert_eq!(started.waits, [Duration::from_millis(20_000); 2]);
    assert_retry_warnings(&started.events, 2, 4, 20_000);
    assert_retry_lines(&started.daemon_log, 2, 4, 20_000);
    let observations = observations(&started.events);
    assert!(
        !observations.iter().any(|(_, observation)| matches!(
            observation,
            ProviderStartupObservation::Failed { .. }
        )),
        "no provider startup failure is recorded"
    );
    let discovery = observations
        .iter()
        .position(|(_, observation)| {
            matches!(
                observation,
                ProviderStartupObservation::InstanceDiscovery { .. }
            )
        })
        .expect("the successful discovery is recorded");
    let last_warning = observations
        .iter()
        .rposition(|(_, observation)| {
            matches!(
                observation,
                ProviderStartupObservation::DiscoveryRetried { .. }
            )
        })
        .expect("the warnings");
    assert!(last_warning < discovery);
    assert!(
        observations[discovery..]
            .iter()
            .any(|(_, observation)| matches!(
                observation,
                ProviderStartupObservation::Completed {
                    stage: ProviderStartupStage::InstanceDiscovery
                }
            ))
    );
    let started_at = started
        .events
        .iter()
        .find(|event| event.event_type() == EventType::RuntimeStarted)
        .expect("the Runtime started");
    assert!(observations[discovery].0.sequence() < started_at.sequence());
}

// Exhaustion: every attempt finds the discovery unavailable. The start makes exactly the
// configured attempts, waits between them, records a warning for each attempt but the last and
// then ends as before: the provider startup failure of the discovery stage and the fatal
// `instance_discovery_unavailable`, with no `runtime.started`.
#[test]
fn disc1_a_start_whose_every_attempt_is_unavailable_ends_in_the_existing_fatal() {
    let started = start(
        json!({ "attempts": 3, "retry_interval_ms": 15_000 }),
        vec![unavailable(), unavailable(), unavailable()],
    );
    let error = started.result.expect_err("the start fails");
    assert_eq!(error.code(), "instance_discovery_unavailable");
    assert!(error.is_fatal());
    assert_eq!(started.attempts, 3, "no attempt beyond the configured ones");
    assert_eq!(started.waits, [Duration::from_millis(15_000); 2]);
    assert_retry_warnings(&started.events, 2, 3, 15_000);
    assert_retry_lines(&started.daemon_log, 2, 3, 15_000);
    let observations = observations(&started.events);
    let (_, last) = observations.last().expect("provider startup records");
    let ProviderStartupObservation::Failed { stage, failure } = last else {
        panic!("the provider startup ends with its failure: {last:?}");
    };
    assert_eq!(*stage, ProviderStartupStage::InstanceDiscovery);
    assert_eq!(failure.outcome.code().as_str(), "provider_native_failed");
    assert!(!started.events.iter().any(|event| matches!(
        event.event_type(),
        EventType::RuntimeStarted | EventType::RuntimeTakeover
    )));
}

// The bounds: `attempts` within 1..=100 and `retry_interval_ms` within 1000..=300000 assemble; a
// value outside them refuses the start (and check-config, which assembles the same way) as
// `config_value_invalid` naming the key, `out_of_range`, with the key's range.
#[test]
fn disc1_attempts_and_interval_outside_their_bounds_are_refused() {
    for section in [
        json!({ "attempts": 1, "retry_interval_ms": 1_000 }),
        json!({ "attempts": 100, "retry_interval_ms": 300_000 }),
    ] {
        assert!(
            configuration(section.clone()).assemble_or_refusal().is_ok(),
            "{section} is within the bounds"
        );
    }
    let rows = [
        (
            json!({ "attempts": 0 }),
            "startup_discovery.attempts",
            1,
            100,
        ),
        (
            json!({ "attempts": 101 }),
            "startup_discovery.attempts",
            1,
            100,
        ),
        (
            json!({ "retry_interval_ms": 999 }),
            "startup_discovery.retry_interval_ms",
            1_000,
            300_000,
        ),
        (
            json!({ "retry_interval_ms": 300_001 }),
            "startup_discovery.retry_interval_ms",
            1_000,
            300_000,
        ),
    ];
    for (section, key, min, max) in rows {
        let Err((code, outcome)) = configuration(section.clone()).assemble_or_refusal() else {
            panic!("{section} is refused");
        };
        assert_eq!(code.as_str(), "config_value_invalid", "{section}");
        assert_eq!(outcome.value("config_key"), Some(&json!(key)), "{section}");
        assert_eq!(
            outcome.value("config_value_problem"),
            Some(&json!("out_of_range")),
            "{section}"
        );
        assert_eq!(outcome.value("range_min"), Some(&json!(min)), "{section}");
        assert_eq!(outcome.value("range_max"), Some(&json!(max)), "{section}");
    }
}

// Workflow #503 NOISE-B (register §1.553): the device error the process listing answers once its
// bound of 60 s expires, untyped, as the bounded runner words it (`crates/device`'s own test pins
// the runner's words and the bound).
const LISTING_PAST_ITS_BOUND: &str =
    "powershell -NoProfile -ExecutionPolicy Bypass -Command <the listing> timed out after 60s";

// Workflow #503 NOISE-B: discovery's reads with no folder named: the process listing, the first
// read such a discovery makes, fails as the listing fails at its bound; `listed` counts the
// listings asked and `vendor` the reads of the vendor tool, which no test here reaches.
#[derive(Default)]
struct ListingPastItsBound {
    listed: usize,
    vendor: usize,
}

impl MumuManagerReads for ListingPastItsBound {
    fn version(&mut self, _mumu_manager_path: &Path) -> DeviceResult<Value> {
        self.vendor += 1;
        Err(DeviceError::fatal("the vendor tool is not read here"))
    }

    fn instances(&mut self, _mumu_manager_path: &Path) -> DeviceResult<Value> {
        self.vendor += 1;
        Err(DeviceError::fatal("the vendor tool is not read here"))
    }

    fn processes(&mut self) -> DeviceResult<Vec<DeviceDiscoveryProcess>> {
        self.listed += 1;
        Err(DeviceError::fatal(LISTING_PAST_ITS_BOUND))
    }
}

// The outcome a failed listing is recorded as: `provider_native_failed` with `device_category`
// `category` (absent when `None`) and no `mumu_root`, whose `foreign_other_error` link holds the
// listing's own text, which says that it ran out of time and its bound.
fn assert_listing_past_its_bound(failure: &Outcome, category: Option<&str>) {
    assert_eq!(
        failure.code().as_str(),
        "provider_native_failed",
        "{failure}"
    );
    assert_eq!(
        failure.value("device_category"),
        category.map(Value::from).as_ref(),
        "{failure}"
    );
    assert_eq!(failure.value("mumu_root"), None, "{failure}");
    let text = failure
        .causes()
        .iter()
        .map(|link| link.outcome())
        .find(|link| link.code().as_str() == "foreign_other_error")
        .and_then(|link| link.value("raw_text"))
        .and_then(Value::as_str)
        .expect("the listing's own text");
    assert!(text.contains("timed out after 60s"), "{text}");
}

// Workflow #503 NOISE-B, the start (study test of change 3): with no folder named, a start whose
// process listing runs past its bound at every attempt finds the discovery unavailable each time
// (the provider's own classification, `DiscoverySpec::discover_with` over these reads), tries it
// again as for an emulator not started yet, one warning per attempt but the last, and ends in the
// existing fatal; each warning and the provider startup failure carry the failed listing, its
// text saying it ran out of time and its bound. The listing is asked once per attempt and the
// vendor tool never.
#[test]
fn noise_b_a_start_whose_listing_runs_past_its_bound_is_retried_then_ends_in_the_existing_fatal() {
    let mut reads = ListingPastItsBound::default();
    let discovery = DiscoverySpec::new(None);
    let answers = (0..3)
        .map(|_| discovery.discover_with(&mut reads))
        .collect::<Vec<_>>();
    assert_eq!(reads.listed, 3, "one listing per attempt");
    assert_eq!(reads.vendor, 0, "the vendor tool is never read");
    for answer in &answers {
        let Err(failure) = answer else {
            panic!("a listing past its bound is no discovery");
        };
        assert_eq!(
            failure.code,
            SharedCode::InstanceDiscoveryUnavailable.as_str()
        );
    }

    let started = start(
        json!({ "attempts": 3, "retry_interval_ms": 10_000 }),
        answers,
    );
    let error = started.result.expect_err("the start fails");
    assert_eq!(error.code(), "instance_discovery_unavailable");
    assert!(error.is_fatal());
    assert_eq!(started.attempts, 3, "no attempt beyond the configured ones");
    assert_eq!(started.waits, [Duration::from_millis(10_000); 2]);
    assert_retry_warnings(&started.events, 2, 3, 10_000);
    assert_retry_lines(&started.daemon_log, 2, 3, 10_000);
    let records = observations(&started.events);
    for (_, observation) in &records {
        if let ProviderStartupObservation::DiscoveryRetried { outcome } = observation {
            let unavailable = outcome
                .causes()
                .first()
                .expect("the attempt's refusal")
                .outcome();
            let failure = unavailable
                .causes()
                .first()
                .expect("the device failure")
                .outcome();
            assert_listing_past_its_bound(failure, Some("protocol"));
        }
    }
    let (_, last) = records.last().expect("provider startup records");
    let ProviderStartupObservation::Failed { stage, failure } = last else {
        panic!("the provider startup ends with its failure: {last:?}");
    };
    assert_eq!(*stage, ProviderStartupStage::InstanceDiscovery);
    assert_listing_past_its_bound(&failure.outcome, Some("protocol"));
}

// Workflow #503 NOISE-B, check-config (study test of change 3): with no `mumu_root` and no
// environment folder, a process listing past its bound leaves the installation unresolved:
// `mumu_root` `null` and the note, the failed listing (no `device_category`: check-config's
// resolver adds none), its text saying it ran out of time and its bound. The listing is asked
// once and the vendor tool never (check-config never runs it).
#[test]
fn noise_b_check_config_reports_a_listing_past_its_bound_as_an_unresolved_root() {
    let mut reads = ListingPastItsBound::default();

    let (mumu_root, note) = crate::check_config::mumu_root_report(None, None, &mut reads);

    assert_eq!(mumu_root, Value::Null);
    let note = note.expect("the unresolved installation's note");
    assert_listing_past_its_bound(&note, None);
    assert_eq!(reads.listed, 1, "the processes are listed once");
    assert_eq!(reads.vendor, 0, "the vendor tool is never read");
}
