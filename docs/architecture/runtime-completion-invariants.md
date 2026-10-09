# Runtime Completion Invariants

This document defines the constructive Runtime evidence for the scheduling and resident-runtime
completion boundary. The evidence uses neutral data, fake backends, accelerated clocks, real child
processes, and persisted local state. It does not claim real-device, game-client, UI, or 48-hour
wall-clock validation.

## Nine invariants

| Invariant | Constructive evidence | Counterexample and red condition |
| --- | --- | --- |
| Deterministic replay | `same_inputs_produce_byte_stable_decisions` and `accelerated_48h_policy_replay_is_deterministic_and_recoverable` compare complete typed evaluations and 48-hour transcripts. | A hidden clock, random source, unstable iteration order, or changed identity input changes the byte transcript and fails equality. |
| Replay has zero second side effect | `policy_host_revalidates_admission_pins_versions_and_replays_without_side_effects` and `safe_reset_replay_without_connection_cache_does_not_repeat_input` compare durable event/backend counters before and after exact replay. | Removing durable replay lookup produces another lease/input/event and changes the asserted counters. |
| Loops are budgeted | `zero_loop_budget_rejects_the_complete_catalog` and `dispatch_intent_pins_fact_freshness_and_loop_budget` enforce nonzero compile-time budgets and pin the admitted daily, window, and runtime limits on each intent. | Zero budget is rejected; dropping budget fields makes the focused tests fail. |
| Clock jumps force full recomputation | `policy_cadence_is_explicit_and_clock_jumps_force_full_recompute` checks normal event, cooldown, jump, and reconciliation directives. | Treating a threshold-crossing clock observation as incremental changes `Full/ClockJump` to another directive and fails. |
| Crash recovery rebuilds the same pending set | `nonempty_pending_policy_work_is_rebuilt_after_real_process_crash` records one admitted and one pending intent in a child process before its forced exit, reopens Runtime from durable state, requires the rebuilt pending set to contain the recorded pending intent, and suppresses replay of the admitted one under a single lease. `policy_dispatch_accepts_one_late_outcome_after_process_crash` reopens after a crash with one admitted dispatch, creates no second dispatch, suppresses its replay, and records its late outcome once across a second restart. | Losing seen-dispatch state reintroduces an admitted intent; losing a pending intent or duplicating a lease also fails event counts. |
| Eligible work does not starve | `aging_eventually_prevents_lower_priority_starvation` advances eligibility age until lower-priority work wins deterministically. | Removing aging leaves the original high-priority choice selected and fails the late-round assertion. |
| Invalid input fails loud | `invalid_snapshots_fail_loud_before_any_intent`, `legacy_schema_version_rejects_the_complete_catalog`, and compiler corruption tests require typed rejection before IR or intent creation. | Partial loading, default insertion, or warning-only handling returns an IR/intent and fails `expect_err` plus diagnostic-path assertions. |
| Unknown is not silently false | `unknown_fact_stays_unknown_and_requests_detection` and `stale_fact_is_not_silently_false` require `EligibilityState::Unknown` and a detection suggestion. | Coercing missing or stale evidence to false changes the typed state and removes the required suggestion. |
| Every dispatch has a complete reason chain | The neutral activity evaluation checks a one-to-one intent/chain mapping, matching decision IDs, and nonempty reasons; `policy_host_revalidates_admission_pins_versions_and_replays_without_side_effects` rejects tampered chains. | Omitting, relabeling, or detaching a chain fails before host admission. |

These tests are constructive rather than string-presence checks. Each one supplies an invalid,
unknown, replayed, aged, jumped, crashed, or tampered input that becomes incorrectly successful if
the corresponding guard is reverted.

## Completion criteria

### Neutral activity integration

`contracts/scheduling/examples/h1-neutral-activity` is a data-only activity addition. It introduces
no Runtime, scheduler, client, or UI branch for a game identity. The production compiler accepts it
through the same four scheduling documents used by every catalog.

`neutral_activity_diff_compiles_and_expresses_all_requirement_shapes` proves:

- a near-cap regenerating-resource consumer using clock plus resource projection;
- a material-balancing task using one consumed and one produced typed pool effect;
- an expiring-item consumer driven by a record-deadline fact, with no pool effect;
- deterministic catalog hash and dry-run bytes;
- all three tasks enter the generic evaluator using only declared instance capabilities.

### Accelerated 48-hour replay

`accelerated_48h_policy_replay_is_deterministic_and_recoverable` performs 48 hourly policy rounds
without sleeping. It applies only catalog-declared effects, requires a complete reason chain for
every intent, serializes the midpoint state, reconstructs it, and compares the recovered second
half with the uninterrupted transcript and final state.

The host-level clock-jump test separately proves that wall-clock discontinuity changes cadence to a
full recomputation. The child-process crash tests prove durable recovery rather than merely cloning
in-memory test state.

### Real process with fake backends

The following tests cross a real process boundary while retaining fake device backends:

- `policy_dispatch_accepts_one_late_outcome_after_process_crash` covers Runtime startup, catalog
  decision, approval, lease, ledger persistence, abrupt child exit, replay suppression, and one late
  outcome after recovery;
- `nonempty_pending_policy_work_is_rebuilt_after_real_process_crash` covers pending-set
  reconstruction after an abrupt child exit;
- `detachable_agent_sidecar_recovers_and_escalates_without_device_authority` covers Dispatcher
  wake, session receipt, Runtime restart, sidecar resume, response receipt, and escalation while all
  device/capture/input counters remain zero;
- `agent_session_start_and_completion_are_idempotent` covers successful Dispatcher response receipt
  and reconnect replay.

### Performance, publication, migration, and rollback

Quantified counterexamples remain owned by their production modules:

- `performance_stutter_is_ledger_visible_and_enriches_policy_failure` injects measured frame gaps
  and requires a bounded pressure context on the failure event;
- `release_pointer_transaction_rolls_back_at_each_sqlite_write_boundary` exercises every declared
  SQLite transition failpoint and requires the old pointer after failure;
- `concurrent_release_switches_commit_exactly_one_pointer_revision` proves one authoritative winner;
- `legacy_migration_is_idempotent_and_conflicts_fail_loudly` proves repeatability and explicit
  conflict rejection;
- `release_generations_switch_atomically_and_rollback_only_to_history` and
  `state_document_rollback_creates_a_new_monotonic_revision` prove all-old/all-new visibility and
  monotonic rollback history.

### Genericity guard

`c2_runtime_code_contracts_defaults_and_fixtures_are_project_neutral` scans the Runtime-domain
workspace members (code, defaults, and fixtures) and the non-Cargo roots `contracts` and `tests`
(`GENERIC_NON_CARGO_ROOTS`) for project-specific identities. The guard's own
`generic_runtime_guard_rejects_project_specific_branch` counterexample injects a hard-coded project
branch and must report a violation. External resource/provider data remains the only permitted place
for project identities.

## Verification commands

Each command runs one named test by its full path:

```text
cargo test -p actingcommand-policy --test h1_completion neutral_activity_diff_compiles_and_expresses_all_requirement_shapes -- --exact
cargo test -p actingcommand-policy --test h1_completion accelerated_48h_policy_replay_is_deterministic_and_recoverable -- --exact
cargo test -p actingcommand-policy --lib evaluator::tests::same_inputs_produce_byte_stable_decisions -- --exact
cargo test -p actingcommand-policy --lib evaluator::tests::unknown_fact_stays_unknown_and_requests_detection -- --exact
cargo test -p actingcommand-policy --lib evaluator::tests::aging_eventually_prevents_lower_priority_starvation -- --exact
cargo test -p actingcommand-policy --lib evaluator::tests::invalid_snapshots_fail_loud_before_any_intent -- --exact
cargo test -p actingcommand-runtime-host --lib tests::policy_admission::policy_cadence_is_explicit_and_clock_jumps_force_full_recompute -- --exact
cargo test -p actingcommand-runtime-host --lib tests::policy_admission::policy_host_revalidates_admission_pins_versions_and_replays_without_side_effects -- --exact
cargo test -p actingcommand-runtime-host --lib tests::crash_recovery::policy_dispatch_accepts_one_late_outcome_after_process_crash -- --exact
cargo test -p actingcommand-runtime-host --lib tests::crash_recovery::nonempty_pending_policy_work_is_rebuilt_after_real_process_crash -- --exact
cargo test -p actingcommand-runtime-host --lib tests::agent_sessions::detachable_agent_sidecar_recovers_and_escalates_without_device_authority -- --exact
cargo test -p actingcommand-runtime-host --lib tests::monitor::performance_stutter_is_ledger_visible_and_enriches_policy_failure -- --exact
cargo test -p actingcommand-runtime-state --lib store::tests::release_pointer_transaction_rolls_back_at_each_sqlite_write_boundary -- --exact
cargo test -p actingcommand-actinglab-architecture --test workspace_guards c2_runtime_code_contracts_defaults_and_fixtures_are_project_neutral -- --exact
```

Reading the result: a verified test prints `running 1 test`, its `... ok` line and
`test result: ok. 1 passed`. `running 0 tests` means the name matched nothing; cargo still exits 0,
but the command verified nothing and does not count as a pass. A test listed as `ignored` was
skipped and does not count as a pass either. A failure prints `FAILED` and cargo exits non-zero.
None of these tests is behind a Cargo feature.

The whole-workspace run is the module test jobs of `.github/workflows/ci.yml`, summarized by the
`rust` check: every workspace member runs in exactly one job, `actingcommand-runtime-client` runs
single-threaded (`-- --test-threads=1`), and its `test-observation` run covers the recorder tests,
`tests::runtime_input_proxy_renews_before_short_lease_expiry` and
`tests::long_input_extends_only_its_response_wait`. The registered flaky tests of
`ci/flaky-tests.toml` are skipped there and run in the non-gating `flaky` job. The workflow file
lists the remaining steps.
