# v0.12.0 register: persisted shapes, removals, wire shapes and configuration keys

Workflow #383 slice AO-0. Order-v012 rule R5 asks that every persisted shape the 0.12 series
writes is registered once for the v0.12.0 ledger (revision 3), so that a 0.12.x build never adds a
field to a record an earlier 0.12 build must read (the 2026-09-27 lesson), with one exception:
ledger revision 4, once, by Alice's ruling of 2026-10-20 (§1.153). This register collects
them from every frozen model in one place, with the slice that writes the types and readers and
the slice that first writes the record. It also lists the removals of option 丙, the wire shapes
handed to AO-0, every configuration key of the one v0.12.0 `actingd-config` revision, and the
row capacity of `config.parameters`.

**Sources** (the coordinator's workspace; each model is frozen on its board issue): LEDG = `rd-1009/arch/model-ledger-observer.md`
v1.4.1; D2M = `rd-1009/d2/model-d2-fact-refresh.md` v1.4.2; PERF = `rd-1009/perf/model-perf-pacing.md`
v1.4.2; SCHED = `rd-1009/sched/model-sched-rules.md` v1.4.2; M387 = `rd-1010/m387/model-387-ladder-config.md`
v1.2; UIM = `rd-1009/perm/model-unified-interface.md` v2.2.3; M-PKG = `rd-1011/docs/model-pkg.md`
v1.1; ADD = `rd-1010/plan-v012/additions-v012.md` v3.1; M378 = `rd-1008/errors/model-378.md` v2.2;
ORD = `rd-1010/plan-v012/order-v012.md`.

**How to read the "code slice" column.** SH-1 is the v0.12.0 shape slice (ORD §3.5 item 2): it
writes the types, the readers, the validation and a round-trip test, never a writer. A shape whose
owner slice is in v0.12.0 and brings its own shape says so. A writer that arrives in 0.12.x writes
a shape that v0.12.0 already reads.

**Rules every shape below follows.** No member carries a task-package content digest (option 丙;
the package identity is `package_id`, recorded as `package_label`). No reader accepts a ledger or
configuration written before v0.12.0 (no old-version compatibility); every 0.12 build reads what
earlier 0.12 builds wrote on the same root, which is why this register exists
([ledger-observer.md](ledger-observer.md), "Later rulings applied", item 2). Field and type
names are fixed here unless a row says "reserved", in which case the named slice may rename it
before SH-1 merges.

## 1 New and changed persisted ledger shapes (ledger revision 3)

### 1.1 Ledger observer (#383, LEDG §11)

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| L1 | `ledger.gap_recorded` | new event type, family `ledger`, action `ledger.recovery`, origin module `global-ledger`; severity = outcome category | SH-1 | AO-1 (dump gaps; built, §1.26), AO-2 (tail, single record; built, §1.38 OB4) | payload `{gap_kind (dump_incomplete, dump_invalid, tail_unconfirmed, record_rejected), gap_start_unix_ms, gap_end_unix_ms, record_count? (absent = unknown), event_id?, source? {source, lineage, revision}, outcome}`; [ledger-observer.md §10](ledger-observer.md#10-gap-records-ao-1-ao-2) |
| L2 | `ledger.health_observed` | new event type, family `ledger`, action `ledger.probe` (new action value), origin module `global-ledger` | SH-1 | AO-2 (watermark and spill segments, revision skips; built, §1.38 OB5), AO-8 (mismatch, flush lag) | payload `{outcome, segment_edge? (started, ended)}`; the finding's values travel in `outcome` |
| L3 | `ledger.dump_imported` | new event type, family `ledger`, action `ledger.recovery`, origin module `global-ledger`, info | SH-1 | AO-1 (built, §1.26) | payload `{outcome}` with `ledger_dump_imported` and its values |
| L4 | record envelope `capture` | new optional envelope member of every stored event | SH-1 | AO-2 (`capture_seq`, `captured_at_unix_ms`; built, §1.38 OB2), AO-3 (`source`) | `{capture_seq, captured_at_unix_ms, source? {source: instance_facts \| runtime_facts \| decision_memory \| fact_refresh_transitions, lineage, revision}}`; `timestamp` = capture time |
| L5 | `runtime.lifecycle_observed` phase `tagged_state_loaded` | new phase | SH-1 | AO-6 | `{stores: [{source, previous_lineage?, previous_revision?, lineage, loaded_count}]}`; the base record of a new lineage |
| L6 | `runtime.lifecycle_observed` phase `tagged_state_enabled` | new phase | SH-1 | AO-5 / AO-6 | no members |
| L7 | `policy.control_state_recorded` | new event type, family `policy`, action `policy.control_state` (new), info; tagged | SH-1 | LN-3 (§1.182; was AO-4) | one dispatch pair's complete decision memory: `{instance_id, task_id (or the reserved D2 recognition identity), daily_counts, window_counts, window_runtime_ms, active_window?, failure_streak? {failure_code, failure_identity? (failure identity v2; §1.15 Q1), class, count, escalation_streak, disposition, retry_at_unix_ms?, decision_id}, retry_round?, dispatch_clock?, condition_reruns? {failure_code, count, expires_at_unix_ms}}`; `active_window` = the `window_id` of the task's activity profile that `window_counts` and `window_runtime_ms` count in; `window_runtime_ms` = the runtime the task has reserved or used in that window (`PolicyControlState.task_runtime`, `policy_control.rs:87`); `paused_on` is rebuilt from `failure_identity` and never stored; the activity profile's own state is L14 |
| L8 | `agent.wake_obligation_recorded` | new event type, family `agent`, action `agent.wake`, info; tagged | SH-1 | LN-3 (§1.182; was AO-4) | one wake obligation (`AgentWakeRecord` at AO-4's head) |
| L9 | `policy.dispatch_intent`, `task.selection_evaluated`: `input_fact_revisions` | new optional member | SH-1 | AO-3 | `{instance_facts {lineage, revision}, runtime_facts {lineage, revision}}`; readers accept either it or `input_ledger_position` (still non-zero when present); AO-4 stops writing the old member (ORD §3.5 item 11) |
| L10 | `runtime.fact_snapshot` sealed by the ledger | existing type, origin module `global-ledger` | — | AO-8 (T2) | the core stops writing it (AO-4) |
| L11 | `ledger_index` table | new table in `runtime-state.sqlite` | AO-2 | AO-2 (built, §1.38 OB1) | [ledger-observer.md §11.1](ledger-observer.md#111-the-index-table-ledger_index) |
| L12 | ledger dump file | file format, `dump_version` 1 | AO-1 (built, §1.26 AO1) | AO-1 | [ledger-observer.md §9.3](ledger-observer.md#93-dump-file) |
| L13 | view lists | Health gains `ledger.gap_recorded`, `ledger.health_observed`, `ledger.dump_imported`; Changes gains `policy.control_state_recorded`, `policy.activity_state_recorded`, `policy.fact_refresh_transition`, `agent.wake_obligation_recorded` | SH-1 (view schema upgrade, `views.rs:40-43`) | — | the new `perf.*` types join Health through their family |
| L14 | `policy.activity_state_recorded` | new event type, family `policy`, action `policy.control_state`, info; tagged | SH-1 | LN-3 (§1.182; was AO-4) | one (instance, activity profile)'s complete activity state, index key `activity_memory/<instance>/<activity profile>`: `{instance_id, profile_id, local_day, daily_count, window_id, window_count, window_runtime_ms, next_eligible_unix_ms?}` = today's `activity_daily`, `activity_window`, `activity_runtime` and `next_activity_eligible` (`policy_control.rs:88-91`); without it a restart after AO-6 would reset the activity budgets and the activity interval |

### 1.2 Fact refresh (#384, D2M §5.3 A-5, N3)

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| D1 | `policy.fact_refresh_transition` | new event type, family `policy`, action `policy.fact_refresh` (new), info; G5 copies it; not tagged | SH-1 | AO-4 (gate) after D2-S5 (recorder; built, §1.68, in memory only) | payload `{cycle_seq, instance_id, fact_key, outcome}` (`outcome` = the transition's code); envelope `capture.source` = `fact_refresh_transitions`, lineage = owner epoch, revision = transition number (from 1 per epoch, +1 per transition, never reset); a skip is `ledger_store_revision_skipped` |

### 1.3 Scheduling rules (#386, SCHED §11 X2)

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| S1 | policy execution record, failure part: `condition` | new optional member | SH-1 | SR-2b (SR-2a reads) | `{refunded, count, expires_at_unix_ms}`; replay only — the rerun state loads from L7; **done (SR-2a):** the settlement takes it from the record, as it takes `perf_context` (`policy_control.rs`, `commit_execution` and `preview_settlement`), never from the configuration. A run with it adds to no failure streak and no escalation (the record keeps the pair's previous counters, 0 when there are none, and `failure_streaks` keeps the latest real failure), continues (retry attempt 0, no retry time), is never re-judged `policy_runtime_budget_exceeded`, adds no runtime to the task or activity window, and with `refunded` gives back the task daily, task window, activity daily and activity window counts its admission took. Only a recoverable failure of a task that is not sensitive takes it; any other pairing is `policy_execution_record_mismatch`. `count` and `expires_at_unix_ms` are SR-2b's (the rerun state); the live settlement writes no `condition` before SR-2b; **done (SR-2b-1, 0.12.27):** the live settlement and the reconciliation of a run from its terminal write it, and the owed rerun is kept in L7's `condition_reruns`; SR-2b-2 (the promotion, the stage gate and the expiry) is still to come |
| S2 | `ContainedTaskCancellationReason::BlackoutStop` | new enum value (wire and ledger) | SH-1 (runtime-client 2) | SR-4b | `from_failure_code`: `blackout_run_stopped` → `BlackoutStop` |
| S3 | `RecoveryLadderSuppression::Blackout` | new enum value | SH-1 | SR-4a (SR-2a decodes) | **done (SR-2a):** a v0.12.0 start reads a `recovery_ladder_suppressed` record with `reason: "blackout"` (SH-1's decoder; no Runtime reader branches on the reason); [emulator-control.md](emulator-control.md) lists the value. The host sets no blackout before SR-4a |

No other new event type (`disposition` gains no value). The pack schedule `session.pack.schedule`
and the feature locks `session.feature.<key>` are ordinary `fact.published` records (§4).

### 1.4 Performance pacing (#385, PERF §12.2)

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| P1 | `perf.pacing_changed` | new event type, family `performance`, action `performance.observe`, observation | SH-1 | P-m | the stage transition record (`perf_stage_changed`); **written (P-m):** §1.61 PM1 |
| P2 | `perf.dispatch_share_changed` | new event type, decision | SH-1 | P-s | `perf_scheduling_paused` / `perf_scheduling_resumed` |
| P3 | `perf.run_hold` | new event type, decision | SH-1 | P-h | held and resumed at `pace_checkpoint` |
| P4 | `perf.summary` | new members | SH-1 | P-m | `responsiveness_bp`, `host_responsiveness_bp`, `emulator_responsiveness_bp`, `pacing_stage`, `factor_milli`, `dispatch_share_milli`, `pacing_mode`, `learning`; host and per instance: `learning_view_stage`, `learning_view_share_milli`, `learning_view_allowed`, `normal_view_stage`, `normal_view_share_milli`, `normal_view_allowed`; `pacing_phase` (`learning`, `normal`; only when `mode = full`); `learn_days`, `learn_active_ms`; **written (P-m):** §1.61 PM2 |
| P5 | task timing evidence, per step | new members | SH-1 | P-m (`page_settle_ms`, `pacing_band`, `pressure_level`), P-k (`pacing_factor_milli`, `extra_wait_rounds`) | `page_settle_ms` keyed by (instance, `package_id`, step index) |
| P6 | policy execution record | new members | SH-1 | P-k | `wall_elapsed_ms`, `budget_elapsed_ms` |
| P7 | `ContainedTaskCancellationReason::PerformanceYield` | new enum value | SH-1 (runtime-client 2) | P-k / P-h | `from_failure_code`: `perf_run_yielded`, `perf_hold_expired` → `PerformanceYield` (one revision with S2) |
| P8 | task timing evidence, per run (MEM-1, ADD §1.6) | reserved members | SH-1 | P-m | `package_load_ms`, `package_load_bytes`, only if the run record does not already carry them; **written (P-m, MEM-1):** §1.61 PM3 |

The calibration statistics, curve and phase are workbench keys (`host.perf_calibration.stats`,
`.curve`, `.phase`) written as `runtime.fact_recorded`; no `perf.calibration_*` event type exists.

### 1.5 Recovery ladder configuration (#387, M387 §6.1) — brought by D1, not SH-1 (ORD §3.5 item 2)

| # | Shape | Code slice |
|---|---|---|
| R1 | `RecoveryTriggerStage` + `stability_warnings` (ladder records only) | D1c |
| R2 | `RecoveryLadderTrigger` + `rule`, `matched` (`code` / `class{ladder_class}` / `warning_burst{count, window_ms, first_unix_ms, last_unix_ms}`), `start` (`immediately` / `after_performance_release`) | D1c |
| R3 | `RecoveryLadderStarted`: `rungs` 1..=3 in configured order, + `config_digest` (the configuration's hash, not a package digest) | D1c |
| R4 | `RecoveryRungSkipReason` + `before_start_rung`; `RecoveryLadderSuppressed` + optional `rule` | D1c |
| R5 | `ConfigParameterSource` + `instruction`; `ConfigParameter` + `settable` | D1b |
| R6 | runtime fact keys `host.config_overrides` (tagged, §4) and `config.selection` | D1b |

### 1.6 Unified interface (#389, UIM §7.1)

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| U1 | G14 command records: `channel`, `via` | new members of `command.received/validated/rejected` | SH-1 | U-2 | `channel` ∈ `cli`, `mcp`, `ui`, `lab`; `via` ∈ `mcp`, `ui`, only on `lab` |
| U2 | G14 action values | no new value | — | U-2, U-3, D1b | `InstallTransition`, `UpgradeState`, `LoadPack`, `UnloadPack`, `ConfigureSettings` → `runtime.action`; `ClearKeptFolder` → `artifact.retention`; task-run keeps `runtime.task_run` |
| U3 | pack registry workbench keys | runtime fact records (G3), not tagged (§4) | SH-1 (key constructors) | U-3 | `host.pack_registry.<package_id>` (procedure bindings, runtime scope), `host.pack_registry.startup` (instance scope), `host.pack_registry.prerequisite.<package_id>` (runtime scope); value `{state: loaded \| unloaded, binding? {procedure_ref, operation_id, yield_points, scheduled_execution? {mode, package_path}}, stopped_request_ids?, withdrawn_dispatches?}`; no digest (M-PKG §5.1); an unload stops the running manual runs of the same `package_id` (M-PKG D10, decided by Alice 10-12) |
| U4 | `lab_module_authoring`, `lab_module_debugging` | configuration keys reported as `config.parameters` rows | U-2 | U-2 | not settable (UIM §4.1) |

The owner journal's upgrade-state close marker is not a ledger shape (UIM §7.1 item 6; U-1, U-4).
The vocabularies `channel` and `lab_option` belong to U-0's code registration (UIM §7.5) and are
not registered by AO-0.

### 1.7 Additions plan (ADD §3 wave 2 row and line 363)

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A1 | `AgentWakeKind` + `periodic_review`, `ladder_exhausted`, `task_paused` | reserved enum values (persisted in `agent.wake_requested`) | SH-1 (decodable) | AGT-1 (0.12.x) | M-AGT may rename before SH-1 merges |
| A2 | `agent.response_recorded`: `report_artifact` | reserved optional member (artifact reference) | SH-1 | AGT-3 | the report body is a ledger material, never a board post |
| A3 | instruction lines `session.instructions` | reserved instance fact key, one per scope, `record_list` | GOAL-1 | GOAL-1 (7b) | rows `{kind, task_id?, target?, value, channel, set_at_unix_ms, expires_at_unix_ms?, basis_artifact?}`; `kind` covers a task weight, a special goal and the one-shot `run_once` of MCP-17; the frozen `session.resource_targets` stays the special-goal section (Alice 10-10 17:0x); GOAL-M fixes the rows |
| A4 | `policy.dispatch_intent`: `priority_locked` | reserved optional member | SH-1 | RT335-S4 | only if S4 cannot take the lock from an existing record |
| A5 | `config.catalog` | runtime fact key (runtime scope, re-observed under `config.`) | MCP-9a | MCP-9a | `{catalog_id, catalog_version, catalog_hash, activated_event_id, approval_id?, approval_disposition?, previous_catalog_id?, previous_catalog_version?}`; the configuration generation is `config.selection` (R6); the watchdog block is read client-side; a bound package path that is missing or unreadable is reported as one overview line computed at read time (coordinator 10-12); **done (MCP-9a):** one row of text fields (`catalog_version` and `previous_catalog_version` decimal text, as `config.selection`'s generation; `approval_disposition` `approved`, `rejected`, `pinned` or `revoked`); `activated_event_id` names the last `catalog.activated` or `catalog.rolled_back` record of the generation's hash, the approval members the latest approval decision whose target is that generation (by hash and version), the previous members the generation of that record's `previous_catalog_hash`; recorded at start, after each committed transition and after each approval decision when it changed; contract type `ConfigCatalog`; [runtime-config.md](runtime-config.md) "Facts" |
| A6 | `actingctl packs --instance` ↔ `ac_packs` rows | read output of a general read (no persisted record and no runtime-client shape: it reads the configuration facts before U-3 and the loaded pack registry after it) | MCP-11 | MCP-11 | the instance's bound standard pack and, per package, `{package_id, path, role}` with `role` ∈ routine, maintenance, prerequisite, startup, recognition maintenance (token spellings reserved for MCP-11); no digest (M-PKG §5.5); a shared-table path (DB-1) and a residency column (MEM-3) come later |

### 1.8 Wave 2 slices (train 3)

Each slice brings its own shape, written by the slice that brings it.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| V1 | `OcrFieldReason::LowConfidence` (`low_confidence`) | new enum value of the post-admission OCR fields report (`actingcommand.runtime.post-admission-ocr-fields.v1`): `OcrFieldResult.reason` and `OcrFieldsReport.failure` | OCR-CONF (#390 R-11) | OCR-CONF | a resolved text whose observation's confidence is missing or below its OCR target's `minimum_confidence`; the field keeps no value, and a failed required field fails the task with `ocr_field_low_confidence`; [ocr-fields.md](ocr-fields.md) |
| V2 | `TaskSemanticFact::EntryRecognition` (`task.entry_preflight`, kind `entry_recognition`) | changed members: `required_page` is replaced by `required_pages`, and `matched_page` is a new optional member | NAV-3b (#373, M-373 v1.1 §8.3) | NAV-3b | `{phase, required_pages [1..=16 distinct pages], matched_page? (present exactly when matched, one of required_pages), matched}`; a record with the old `required_page` is not read |
| V3 | task diagnostic stream (`actingcommand.runtime.task-diagnostic.v2`): `TaskDiagnosticTargetData.failure`, `TaskDiagnosticOcrData::Evaluated.located`, `TaskDiagnosticRecognitionError.outcome`; the recognition error rows' error code `TaskDiagnosticRecognitionError.code` gains the value `TextReadRefused` (A2-P removes `code` and makes `outcome` required, §1.15 Q18) | three new optional members, absent when unused, and one new value | REC-1 (#396, REC-M v1.2.1 §2) | REC-1 | `failure` = `text_locate_not_found` / `text_locate_outside_cells`; `located` = `TaskDiagnosticOcrLocatedData {block_index, rect, confidence_milli, cell_index?, cell_rect?, truncated}`; `outcome` = `text_read_budget_exceeded` / `text_read_blocks_truncated`, under the code `TextReadRefused`; no ledger record type is added |
| V4 | `TaskGeometryAspectMismatch::FrameNotSixteenByNine` (`frame_not_sixteen_by_nine`) of `task.geometry_observed` | removed persisted value | DA-A (#392, model 392 §8.3, §12.1; model 398 §8) | — | the frame's own aspect ratio is recorded, never refused; `frame_and_logical_display_differ` is the one value left; a record that carries the removed value is not read (no old-version compatibility) |
| V5 | `AdbCommandEvidence.raw_text_redacted` in the ADB target recovery report (`adb_recovery` of the runtime lifecycle and recovery records) | new optional member, omitted when false | RD-1a (#398, model 398 v1.1 §3.5 rule 4) | RD-1a | `true` when a device serial the Runtime holds a binding for was replaced by `[device:<label>]` in the command's `stdout` or `stderr`; outcomes carry the same fact as the boolean key `raw_text_redacted` (on `foreign_adb_error`); [adb-target-recovery.md](adb-target-recovery.md) |

### 1.9 Ledger revision 3 itself (#378 A3, M378 §8.1)

Workflow #378 slice A3 (PR #721) writes the types, the readers and the writers of revision 3 in one
PR, so it is both the code slice and the first writer of every row below. Later rulings override
M378 where they differ: no format-1 reader and no migration (Alice 10-11; ORD §10). The other
sections' shapes come on top of these. A3's new codes and locations are catalog entries, not rows
(§7).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| E1 | SQLite format 2 and `actingcommand.event.v3` | format | A3 | A3 | `user_version` 2 (`FORMAL_FORMAT_VERSION`, `ledger/src/global/sqlite.rs`); every record's `schema_version` is `actingcommand.event.v3` (`GLOBAL_EVENT_SCHEMA_VERSION`), and the readers accept no other. A database whose ledger tables sit under `user_version` 1 is refused with `ledger_format_unsupported` (fatal) at every entry: open, schema inspection, snapshot, prefix and selected reads, import, initialization and the view upgrade; each reads only `user_version` and the ledger table count before it refuses, and writes nothing. On the Runtime's start the refusal comes first: `RuntimeHost::start` probes an existing database read-only (`refuse_unsupported_ledger_format`) before it writes the owner journal or takes the writer lock, so a format-1 root gets neither; a root without the database is fresh and is not created by the probe. The views' `SEVERITIES` gain `success` |
| E2 | record `severity` | changed meaning; new value `success` | A3 | A3 | `EventSeverity::Success` (between `debug` and `info`). A severity is never set on its own (`record_severity`, `contract/src/event/envelope.rs`): it is the category of the record's `outcome`, else the catalog category of its event type (vocabulary `event_type`); a producer chooses only `debug`. So `fatal` has only the category meaning (M378 F1). The ledger's storage checks compare with `record_severity`. The policy class of a failed scheduled run no longer comes from the severity (E10) |
| E3 | `outcome` | new member of every result-bearing record | A3 | A3 | record form `{code, category, values, causes?, causes_total?}`: the envelope without `schema_version` and `detail`, always full, `values` always present; a link adds `relation` (M378 §4.1). Carried by: every `DiagnosticOutcomePayload` (`runtime.failed`, `input.failed`, `capture.failed` and the other `*.failed` / `*.rejected` records) and `DiagnosticPayload`, required (the producer's outcome, else the lifecycle failure's, else the artifact failure's, else the diagnostic code itself; a failed cleanup is its `cleanup` link); `ArtifactExportFailurePayload` (required); `CatalogTransitionPayload` and `ReleaseTransitionPayload` (with their `diagnostic_code`); `runtime.lifecycle_observed` `RecoveryRungFinished` (a failed or skipped rung; a skip is `recovery_rung_skipped_<reason>`, info), `RecoveryLadderFinished` (E6) and `InstancePreparationFinished`; the task facts `TerminalCommitted` (failed or cancelled), `TerminalRejected`, `EntryRecoveryFailed`, `EntryTargetDisposition` (fail-closed), the selection confirmation `guard_failed` / `capture_failed` and `TaskGeometryFailure`; `PolicyDispatchRejection`; `ProviderNativeFailure` (a device failure without a registered code is `provider_native_failed`; `module` stays a member, see E5) and the provider startup observation `model_folder_invalid` (new: `vision_model_folder_invalid` with `path` and `model_name`, replacing the `invalid_model:` binding whose `configured` held vision-ffi's reason); `ArtifactEvictionOutcomeRecord` (a failed eviction's `foreign_os_error`); `CapacityVolumeSample.cause` (an outcome under the member's old name, with `operation`); the performance monitor state (degraded only); `ResourceAuthoringPayload` (promote failed); the task diagnostic record's terminal `task_error` / `operation_error`, its error page and its unexecuted page and guard; the `config.subsystems` rows (E8). The fields it replaces are removed (E5) |
| E4 | the 14 serde members named `outcome` | renamed | A3 | A3 | `RecoveryRungFinished.rung_result`, `RecoveryLadderFinished.ladder_result`, `PolicyExecutionPayload.policy_result`, `TaskSemanticFact::Finalizing.task_result` and `::TerminalCommitted.task_result`, `SchedulingOutcomeProjection.scheduling_result`, `TaskDiagnosticTerminalData::Returned.task_result`, `FailedRunRetentionEvidence.task_result`, `ArtifactEvictionProof.terminal_event` and `ArtifactEvictionObservation.terminal_event`, `CapacityDecision.admission_result`, and `selection_result` of `TaskSelectionRecord`, `TaskSelectionGateResult` and `TaskSelectionTermResult`. Types and values are unchanged; in a record `outcome` now means only E3 (M378 M3). The two receipt members are A4's; `outcome_key` and `policy.facts.outcomes` keep their names |
| E5 | fields removed | removal | A3 | A3 | the `runtime.failed` lifecycle record's `stage`, `code`, `operation`, `fatal`, `raw_os_error`, `native_detail`, `cleanup_cause` and `runtime_code` (now the outcome's values and links, `runtime_code` the value of that name, which every code accepts while pending; `cleanup_detail` keeps the cleanup's typed detail), and its `cause`'s `code`, `severity`, `native_detail` and `last_native_detail`; `DiagnosticOutcomePayload.cleanup_cause` (now `cleanup_detail`; `CleanupCauseRecord` is gone); `ProviderNativeFailure` `code`, `severity`, `message`; `ArtifactFailureRecord` `primary`, `secondary` (the outcome, with `secondary` links); `ArtifactEvictionOutcomeRecord.io` (`ArtifactEvictionIo {kind, raw_os_error, message, message_truncated}`); `CapacityNativeCause {operation, raw_os_error, detail}`; `PolicyDispatchRejection` `code`, `operation`, `fatal`; `PolicyReasonRecord.detail`; the performance monitor state's `failure_code`; `ResourceAuthoringPayload.failure_code`; the task facts' `failure_code` (terminal, entry recovery, entry disposition), `TerminalRejected.reason` and `TaskGeometryFailure.code`; `TaskSelectionOutcome::Unknown.detail` and the confirmation `code`; `RecoveryRungFinished.reason`; `InstancePreparationFinished.failure_code`; `DiagnosticDetailRecord` `message` and `declared_sensitivity` (E7); the task diagnostic record's page `message` and `error`, recognition error `message`, target `message`, target source `message`, error page `message`, unexecuted page and guard `reason`, and terminal `code` and `detail`; `ConfigSubsystem.reason` (E8). Kept, against M378 §8.1's move to the value `origin_module` (acceptance CL-M3, CA-L5): the lifecycle record's `source` (`runtime_host`, `runtime_client`, `actingd`) and `ProviderNativeFailure.module` (`actingd.config`, `execution-kernel`, `actingcommand_device::mumu_manager`, `actingcommand_runtime_host::provider`, `actingcommand_runtime_host::emulator_control`): the vocabulary `origin_module` has a token for none of them but `runtime_host` (`runtime`) and the host's provider (`provider`), so both stay required string members of revision 3 |
| E6 | `RecoveryLadderFinished.outcome` | new optional member | A3 | A3 | present exactly when `ladder_result` is `exhausted` (the payload check refuses anything else): `recovery_ladder_exhausted` (error), caused by the last failed rung's outcome (no link when every rung was skipped), or `recovery_ladder_interrupted` (warning), caused by the holding interruption. `recovered` and `environment_ready` carry none. Its category is the record's severity (E2), so review L6 (#666) holds: own failures end at `error`, an interruption at `warning` |
| E7 | `DiagnosticDetailRecord` | changed | A3 | A3 | typed tokens only: `{category, stage, backend, operation}`; `stage` takes 1 to 4 dotted segments (was 2 to 4), so a registered location fits. It is the `detail`, `primary_detail` and `cleanup_detail` of failure records, the lifecycle cause's `detail` and the device diagnostic samples' `detail`. No persisted Runtime English (M378 G6): third-party text is `raw_text` on a `foreign_*` link of the outcome (`foreign_os_error` with `os_error` when the OS gave a number, `foreign_mumu_manager_error` with `exit_code` for MuMuManager's output once its command exited, never an OS error, else `foreign_other_error`) with its `raw_text_sensitivity`, and the Runtime's own text goes only to the developer log. A record is at least `internal` when it has a diagnostic code or detail, and at least the highest `raw_text_sensitivity` in its outcome (none declared counts as `sensitive`). Text members still bridged: `LifecycleNativeDetail.text` (ADB recovery evidence, B5), the OCR texts and `InputAction::Text.text` (B4); A2-P typed `EmulatorVersionEvidence::Unavailable.reason` (§1.15 Q21) and made `stage` exactly a registered location (§1.15 Q10) |
| E8 | `config.subsystems` rows | changed runtime fact rows | A3 | A3 | the manifest's `ConfigSubsystem {name, enabled, outcome}` (was `reason`); the fact row is `{name, enabled, code}` plus each scalar value of the outcome under its own key (today only `count`), and no links; the `env_overrides` row names the `variable` of each `env_override_ignored` note under `ignored_variables`, comma-separated in link order, so an ignored override is never silent (coordinator ruling 11). Codes `config_subsystem_configured`, `_configured_off`, `_flag_absent`, `_section_absent`, `_always_on`, `_discovery_bound`, `_discovery_unbound`, all `info`; [actingd-check-config.md](actingd-check-config.md), "Result" |
| E9 | public projection | codes only (a read surface, not a stored record) | A3 | A3 | `PublicPayload.outcome` is the record's outcome codes only (`Outcome::codes_only`, M378 §4.3: every node keeps its code, category and relation and only the values whose type is a code, a location or a vocabulary token; no `raw_text`, no data value); it replaces `cleanup_cause` and `failure_code`. The projected task semantic fact (terminal, entry, selection confirmation and geometry failures) and the retention `EvictionOutcome` (was `io_kind`, `raw_os_error`) carry codes-only outcomes; the lifecycle failure's summary drops `primary_detail` and `cleanup_detail`; `ProjectDiagnosticView` drops `fatal` |
| E10 | `TaskSemanticFact::TerminalCommitted.failure_severe` | new member, `bool` | A3 | A3 | right after `outcome`; serde default `false`, skipped when `false`. The failure point's own verdict that this failure must pause the task: `true` when the run poisoned the Runtime or its error was fatal (poison or `error.is_fatal()`), except `false` for a non-poisoning `input_backend_operation_failed` / `application_backend_operation_failed` of a `linear_steps` main package (R25-2), and `false` for the first-capture failures the host deliberately makes nonfatal (ADB baseline lost, a stale kept session). Only a failed terminal sets it (`invalid_task_terminal` otherwise). The policy class of a failed scheduled run is `Severe` iff `failure_severe` or the code is on the contract's static always-severe list (`is_severe_policy_failure`), through the contract fn `scheduled_failure_class`, used alike by live settlement, startup recovery and the ledger's re-derivation (coordinator ruling 9, 2026-10-13). The policy failure code is `scheduled_failure_code_str(outcome)`: the outcome's code, or the `raw_code` of `outcome_relay_unregistered` (coordinator ruling 7) |

A rung skip reason added later (R4's `before_start_rung`) records its own
`recovery_rung_skipped_*` code, which its slice registers.

Revision 3 freezes each event type's category: a record without an outcome takes it as its
severity (E2) and the ledger's checks compare stored records with it, so recategorising an
`event_type` token needs a ledger revision bump.

Follow-ups A3's acceptance (2026-10-13) leaves to later slices, beside the PR's deviations:
- **B2:** the Runtime-made `with_native_detail(format!(…))` texts (about 50 sites) and the
  Runtime errors' `Display` written as `foreign_other_error.raw_text` at `events.rs:157`,
  `approval.rs:409`, `fact_store.rs:259`, `host/input.rs:108`, `host/client_events.rs:32`,
  `host/contained_task.rs:1844`, `host/lab_operation.rs:614`, `:655`, `:692`,
  `host/online_observation.rs:428`, `:442` and `host/instance_discovery.rs:200`; the relaxed
  spec rules (relayed outcomes exempt from required keys, the context keys `operation`,
  `stage`, `boundary` and `runtime_code` declared for every pending code, `lease_id` and
  `timeout_ms` optional; A4 restores `lease_id`).
- **A2-P (done, §1.15):** the recognition error's `severity`/`code` strings (Q18),
  `EmulatorVersionEvidence::Unavailable.reason` (Q21), and the failed
  `policy.execution_recorded`'s outcome (Q1, Q2).
- **B4:** coding vision-ffi's individual folder reasons (check-config prints them; the record
  carries `vision_model_folder_invalid` only); the R25-2 verdict at the failure point
  (`contained_task.rs` terminal) against the rerun-only identity at settlement
  (`failure_settlement.rs`) when a cleanup poisons after the terminal.
- **A8 (documents done):** the PPOCR failure source (`ppocr_source`, `ppocr_message`) and the
  connection `timing` are measured but no longer persisted;
  [task-diagnostic-stream.md](task-diagnostic-stream.md) says so. actingd's dead
  `client_message` argument (`RuntimeLifecycleFailure::Client.message`, matched as `message: _`
  in `runtime-host/src/host/lifecycle.rs`) is still there: removing it is a code change, outside
  A8's declarations and documents, and is left to the coordinator.
- **A6a / A8 (done):** check-config v1's `subsystems[].reason` became `outcome` without a version
  bump; [actingd-check-config.md](actingd-check-config.md) says so, and check-config v2 is part
  of `runtime-outcomes` revision 1 (§6).

### 1.10 Real-device capture (#398 R1 §5.1, §5.3; #402 RD-3)

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| C1 | `CaptureBackendName::Scrcpy`, spelled `scrcpy` | new enum value of `task.geometry_observed` `frame.backend` and of `CaptureGeometry.backend` (`observation`, `frame.producer`) | #402 RD-3 (brings its own shape) | #402 RD-3, once RD-1b lets an instance select it | the backend-open reports and `device.self_check` carry the selected backend as a string (`selected`), which gains the same spelling; no new member |
| C2 | `CaptureFrameTransform::Crop(CaptureFrameCrop)`, serialised `{"crop": {"x", "y", "width", "height"}}` | new enum value of `CaptureGeometry.frame_transform` (`task.geometry_observed`: `observation`, `frame.producer`) | #402 RD-3 (brings its own shape) | #402 RD-3 | the frame is exactly this region of the logical display, in logical display pixels (`deny_unknown_fields`); a recorded crop is compared exactly, every other transform proportionally as before (`CaptureGeometry::frame_matches_display`); for example a 1080×2340 display streamed as 1080×2336 records `{x: 0, y: 2, width: 1080, height: 2336}` |

The stream stamp of a frame (`FrameStreamStamp`: sequence, device presentation time, arrival
time) stays in the device layer's memory and is not recorded. The model's values
`fresh_wait_ms`, `frame_sequence` and `stream_end` are outcome values (registry entries, LEDG §11),
attached when B5 types the device layer.

### 1.11 Wave 2 slices (REC-2)

Each slice brings its own shape, written by the slice that brings it.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| T1 | task diagnostic stream (`actingcommand.runtime.task-diagnostic.v2`): `TaskDiagnosticTargetData.uniqueness` (`TaskDiagnosticUniquenessData`) | new optional member, absent when unused | REC-2 (#396, REC-M v1.2.1 §2, §4) | REC-2 | present on the row of a target that declares `unique` and otherwise passed: `TaskDiagnosticUniquenessData {score_milli, margin_milli, second_score_milli?, second_rect?}` (`deny_unknown_fields`; `second_rect` an `OcrRegionRect`), the two optional members present exactly when a second place lies within the margin; REC-1's `failure` (V3) then carries `target_match_not_unique`, and a unique search past its limit is a recognition error row (with its timing) whose `outcome` (V3, §1.15 Q18) is `target_match_search_incomplete`; no new member beyond `uniqueness`, no ledger record type is added; [resource-declarations.md](resource-declarations.md), section Task schema `0.9` unique matches |

### 1.12 Wave 3: NAV-6 (#373, M-373 v1.1 §8.3, §8.4, §8.6)

NAV-6 brings its own shapes and writes them. Without an `interrupts.json` in a game root, a run
writes none of them and its records are those of v0.12.0 before NAV-6; the startup record N4
appears only once `standard_packs.root` is set.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| N1 | `TaskSemanticFact::EntryRecoveryPackageAdmitted`, `::EntryRecoveryCompleted`, `::EntryRecoveryFailed` (`task.entry_preflight`) | new optional member `role` (`TaskEntryRecoveryRole`) | NAV-6 | NAV-6 | value `interrupt_handler` (a shared interrupt page's handler or `then.run` package run as a layer); serde default, omitted when absent. Prerequisite, return-home and bound recovery layers carry none until NAV-3a (`prerequisite`, `return_home`) and NAV-5 (`navigation_hop`, `recovery_chain`) add their values (M-373 §8.3) |
| N2 | `TaskSemanticFact::InterruptHandled` (`task.entry_preflight`, kind `interrupt_handled`) | new record | NAV-6 | NAV-6 | `{page, handler_package_label, then, count, outcome}` (`deny_unknown_fields` as every task fact): `page` the `pages.json` id, `then` `"resume"`, `"end_run"` or `{"run": <package label>}` (`TaskInterruptThen`), `count` 1..=8 (`invalid_task_interrupt` otherwise), `outcome` `contained_task_interrupt_handled` (info) with `page_id`, `count` |
| N3 | `TaskSemanticFact::InterruptAmbiguous` (`task.entry_preflight`, kind `interrupt_ambiguous`) | new record | NAV-6 | NAV-6 | `{page_ids [2..=32 pages.json ids], outcome}`, `outcome` `contained_task_interrupt_ambiguous` (warning) with `page_ids`; the record M-373 §6.3 asks for (N6-8), a shape §8.3 does not name |
| N4 | `RuntimeLifecyclePhase::StandardPackRead` (`runtime.lifecycle_observed`, phase `standard_pack_read`) | new lifecycle phase | NAV-6 | NAV-6 | `{outcome}`: `standard_pack_registered` (info; `game`, `count`, optional `added_package_ids`, `removed_package_ids`), `standard_pack_root_invalid` (`game`, `path`, `standard_pack_problem`, `json_pointer`) or `standard_pack_root_unreadable` (`path`, optional `game`; `foreign_os_error` cause), all error but the first; system links; durable (`CommitSync::Full`). At startup one per game root, plus (Workflow #357 DB-1, §1.65 TB2) one per refused table of a sound root (`standard_pack_table_invalid` or `standard_pack_table_unreadable`); in a run's preparation one when the registry changed (M-373 §8.4) |
| N5 | `standard_packs.root` (actingd configuration, section `standard_packs`) | new configuration key | NAV-6 | NAV-6 | a path, relative to the configuration file's directory; empty refused (`config_decode_failed`); no default (absent = no root is read); startup key (`settable = false`, M-373 §8.6). Parsed the existing way; D1b/D1c move it into `crates/runtime-config` as a section file. Not in `config.parameters` |
| N6 | check-config v1 report: member `standard_packs` `{root, root_problem, games: [{game, bundle_version, pack_count, root_files, problem}]}` and `warnings` entries `<code>:<game>` | interface | NAV-6 | NAV-6 | always present; a broken game root keeps `status: ok` ([actingd-check-config.md](actingd-check-config.md), "Standard packs") |
| N7 | root files `pages.json` (`actingcommand.pages.v1`) and `interrupts.json` (`actingcommand.interrupts.v1`) | resource file formats, not ledger shapes | NAV-6 (reader, contract checks) | resource side (NAV-1, NAV-2) | [package-reference.md](package-reference.md), "Standard package root"; `actinglab resource validate` families `standard_pack_pages`, `standard_pack_interrupts` |
| N8 | `ContainedTaskRuntime::shared_interrupt` (execution kernel) | new trait method, default `NotHandled` | NAV-6 | — | not persisted; every other runtime keeps today's behaviour |

### 1.13 Resource fence (#403, approved 2026-10-13)

An unconfirmed close of one instance's own resources fences that instance instead of ending the
whole Runtime. The slice brings its own shapes and writes them. The fence itself is memory only
(a restart prepares and self-checks every instance again, as every start does), and the owner
journal's record shape does not change: while any fence holds the owner's disposition stays
`in_use`, and only unconfirmed resources that no single instance owns, or an unconfirmed close
once the Runtime is shutting down, still write `unconfirmed`.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| F1 | `runtime.failed` of `instance_resources_fenced` | existing record type and lifecycle failure, new code | #403 | #403 | one record when an instance is fenced: stage `session_close`, linked to the instance and to the request whose work met the failure; outcome `instance_resources_fenced` (error) with `instance_id`, `caused_by` the unconfirmed failure; no resource dispositions of its own (the close's own `runtime.failed` before it carries them) |
| F2 | `runtime.lifecycle_observed` phase `resource_fence_lifted` | new phase | #403 | #403 | `{instance_id, preparation, outcome}`: `preparation` is the `instance_preparation_finished` whose passing self-check proved the close, `outcome` is `instance_resources_fence_lifted` (info) with `instance_id`; one record when the fence is lifted |
| F3 | `RecoveryTriggerStage::ResourceFence` (`resource_fence`) | new enum value of `recovery_ladder_started.trigger.stage` | #403 | #403 | a fence's ladder: no `run_id`, `task_id` or `preparation`; `failure_code` is the unconfirmed failure's code; a fence that manual work met (a direct task run, a client lease, an application or input command, a Lab operation) starts none (M387 B1) |
| F4 | `RecoveryRungSkipReason::ResourceFence` (`resource_fence`) | new enum value of the rung plan's skip reason | #403 | #403 | the return-home rung of a fence's ladder, which never re-prepares the instance; its `recovery_rung_finished` outcome is `recovery_rung_skipped_resource_fence` (info) |
| F5 | `RuntimeInstanceStatus.resource_fence` (`InstanceResourceFence`) | new optional member of the status projection (also in the runtime state observation of `command.validated`) | #403 | #403 | `{cause, since_unix_ms, awaiting_person}` (`deny_unknown_fields`), absent when the instance is not fenced; `cause` is the unconfirmed failure's code; `awaiting_person` once the fence's ladder ended without proof or none could start; `ac_overview` shows it as the row's `resource_fence` |

### 1.14 Wave 3 slices (D1b, #387 M387 §5–§6)

D1b brings its own shapes (rows R5, R6 of §1.5 and W3 of §3, written here as built) and writes
them; [runtime-config.md](runtime-config.md) is the contract.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| G1 | `ConfigParameter.settable` (boolean); `ConfigParameterSource::Instruction`, spelled `instruction` | new member, new enum value (R5) | D1b | D1b | `settable` from the key table of `crates/runtime-config`; `instruction` only in the running Runtime's facts, never in `check-config` |
| G2 | `config.parameters` → one runtime fact `config.parameters.<section>` per section (prefix `CONFIG_PARAMETERS_FACT_PREFIX`); rows `{key, value, source, settable}` | key change of a runtime fact (§5.1 split) | D1b | D1b | section = the key's first segment, `top_level` for a key without a dot; at most 256 rows each; a section an earlier start had and this one has not is recorded empty; readers: `actingctl status --config` (all sections, key order), not the suspension report (its start time now reads `config.subsystems`) |
| G3 | `config.selection` | new runtime fact (R6), one row `{config_sha256, defaults_sha256, config_generation?}` | D1b | D1b | `sha256:<hex>` of the configuration file and of the embedded `actingd.defaults.json` (configuration files, not packages); `config_generation` decimal text, installed Runtime only; contract type `ConfigSelection` |
| G4 | `host.config_overrides` | new runtime fact (R6), rows `{config_key, value_json, base_json, base_source, set_unix_ms, set_in_owner_epoch, request_id, channel?}` | D1b | D1b | `value_json` / `base_json` canonical JSON text; `base_source` `explicit` or `default`; `set_unix_ms` a `timestamp_ms` scalar; `channel` written from U-2 on; tagged (§4) |
| G5 | `RuntimeOperation::ConfigureSettings { changes: [ConfigSettingChange {key, value?}] }` → `RuntimeResult::SettingsConfigured { settings: [ConfiguredSetting {key, value, source}] }` | new request and result kind (W3, runtime-client 2) | D1b | D1b | 1..=64 changes, keys unique, value ≤ 8 KiB serialized (`invalid_configure_settings`); general instruction, G14-copied; command records `command.received` / `validated` / `rejected` with action `runtime.action` (§1.6 U2) |
| G6 | `CommandPayloadDraft::rejected_with_outcome` | writer API, no new member | D1b | D1b | the `command.rejected` record's `outcome` is the refusal (`config_*`) rather than `command_rejected` |
| G7 | restore notes at start: `runtime.failed` with outcome `config_override_superseded` (info) / `config_override_base_changed` / `config_override_dropped` (warning) and `config_key` | existing record type, new outcome codes | D1b | D1b | severity = the code's category; system links |
| G8 | `check-config` v1 report: member `config_selection`; `not_checked` token `instruction_values`; `config_manifest.parameters[].settable` | interface members | D1b | D1b | schema version stays `v1` (additive) |
| G9 | `actingctl config set <key>=<json>…` / `config clear <key>…`; MCP `ac_config_set {changes: [{key, value?}]}` (observer tier until U-3) | CLI and MCP surface | D1b | D1b | the same request through both (D1-M8); MCP `INSTRUCTIONS` names `ac_config_set` (UIF:1013 → M387 §8.4) |
| G10 | Runtime artifact payload `actingd.defaults.json` | distribution file | D1b | D1b | layout name stays `distribution-v1`; `Get-ExactBuildArtifact.ps1` accepts a Runtime artifact with or without it; the build compares its SHA-256 with `check-config`'s `config_selection.defaults_sha256` |

Configuration keys (§5): D1b adds none. The per-instance `stuck_recovery` and
`stuck_recovery_cooldown_secs` join the key table (settable, `default = code` until D1c gives
them `section:recovery_ladder.*`); every key the manifest reported before D1b has a row
(not settable). The defaults file ships empty (`{}`) until a section with `default = file`
keys arrives (D1c on).

### 1.15 A2-P: emitted spellings and dynamic families (#378 A2-P, M378 §2.1-§2.2)

Workflow #378 slice A2-P writes the types, the readers and the writers in one PR, so it is the code
slice and the first writer of every row below. Renamed spellings and changed members have no alias
(no old-version compatibility): a record that carries a removed member or an old spelling is not read.
The new codes, locations, keys and vocabulary tokens are catalog entries, not rows (§7).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| Q1 | `PolicyFailureRecord` (`policy.execution_recorded` `policy_result.failure`) | changed | A2-P | A2-P | `error_code` removed. `outcome` (record form, required, first member) is the run's failure: the terminal outcome's top node (`Outcome::without_causes`: code, category, every value, no links), the request error's top node for a run without a terminal, `policy_runtime_budget_exceeded` for the budget rewrite, or `policy_settlement_interrupted` for the startup interruption. `failure_identity` (optional, skipped when absent) is the #336 identity, byte for byte the former `error_code` of a live `linear_steps` settlement (format v1 until PKG-S3's v2, §2), validated as a policy token. Streaks compare the failure code (`scheduled_failure_code_str`), the identity and the class, never the whole outcome; replay and the ledger's recovery lanes compare the same. `deny_unknown_fields` kept |
| Q2 | `policy.execution_recorded` severity | changed | A2-P | A2-P | a failed execution's severity is its failure outcome's category (E2): `error` for almost every failure code, `warning` for a warning code; it was the event type's `info` (A3 deviation 5). A success stays `info`. A failed execution record is therefore in the errors view, a retention error point and run-recovery evidence again, as before revision 3 |
| Q3 | public projection: `PublicPayload.policy_execution_outcome`, `PublicPayload.policy_signal_fact_code` | changed, removed (read surface) | A2-P | A2-P | the nested failure `outcome` is codes only (E9); `policy_signal_fact_code` is removed: the planning signal's outcome is the record's `PublicPayload.outcome`, codes only, so `fact_key` and times (data values) are not projected |
| Q4 | run summary `actingcommand.run-summary.v1` | changed members (wire) | A2-P | A2-P | `outcome.policy.failure` follows Q1 (`outcome`, `failure_identity` instead of `error_code`); `reason_chain.reasons[]` follow Q7 |
| Q5 | `policy.planning_signal_observed` | `fact_code` replaced by `outcome` | A2-P | A2-P | `PolicyPlanningSignalPayload.outcome` (record form, E3; the record's severity is its category, E2): detection `planning_fact_unknown` {`fact_key`, `unknown_reason`}; clock slots `clock_slot_refused` / `_deferred` / `_owed` with `task_id`, `alias`, `gate_code`, `budget_dimension`, their times and `first_run` (written only when true), a refused slot recording at `warning`; strategic `strategic_feasibility_red` / `strategic_timeline_reached`; drift `maintenance_recheck_suggested` (now `info`). Signal and reservation ids are derived as before |
| Q6 | runtime-state planning-signal projection row | schema `actingcommand.policy-planning-signal.v2` | A2-P | A2-P | `{schema_version, signal_id, instance_id, task_id, kind, outcome, observed_at_unix_ms, detection_budget}` (was `fact_code`); namespace `policy.planning-signal.v1` unchanged; a v1 row is not read |
| Q7 | `PolicyReasonRecord` | changed: each reason is its outcome | A2-P | A2-P | every entry of `policy.dispatch_*` `reasons` and of `task.selection_evaluated` `reasons` / verdict `reasons` is `{code, category, values}` (was `{code}`; M378 §8.1). Scheduling reasons: `task_eligible`, `rank_assigned`, `score_computed` (were `eligible`, `ranked`, `scored`), `score_unknown` {`rule_id`}, `score_cost_unknown`, `decision_candidate_not_selected` {`task_id`}, `resource_target_unmapped` / `_override` / `_pending` / `_satisfied` / `_applied` {`resource_target_id`} (were `<code>:<value>`); selection reasons: 20 `selection_*`, `candidate_*`, `gate_*`, `term_*`, `tie_break_unknown` codes (were dotted, e.g. `gate.rejected` → `gate_rejected` {`rule_id`}). Replay compares chains exactly, so values are deterministic. Amended by §1.145 (A26, 0.12.5): `decision_candidate_not_selected` {`task_id`, `why`, `decision_state`, `eligibility`}, `decision_record` {`omitted_why`?} |
| Q8 | `ProjectDecisionView.reason_codes` (IPC `ProjectInterface`) | changed values (wire) | A2-P | A2-P | the same string list, now the registered spellings of Q7 |
| Q9 | `actingd suspended` `clock_slots` | changed unparsed entry (wire) | A2-P | A2-P | parsed entries keep their JSON (read from the signal's outcome); an unparsed entry is `{parse_state, signal_id, code}` (was `raw_token`). `suspended[].error_code` keeps its value (the failure identity, else the failure's code) |
| Q10 | `DiagnosticDetailRecord.stage` | changed values and validation | A2-P | A2-P | every stage is a registered location (`^[a-z][a-z0-9]*(_[a-z0-9]+)+$`, at most 64 bytes; was 1 to 4 dotted segments, E7): the device stages are the former dotted tokens with `.` → `_` (`adb_ensure_device_get_state`, `maatouch_stdin_write`, …), lifecycle details carry `runtime_lifecycle_*`, connection failures `runtime_ipc_*`, others `frame_retention_move` / `_counter` / `_remove`, `runtime_monitor_restore`, `observation_failed`, `instance_discovery_failed`, `instance_discovery_unsupported`, `application_foreground_unsupported`, `emulator_control_binding` / `_registration` / `_support`, `mumu_manager_control`, `mumu_manager_process_list`, `material_read_attempt`, `resource_declaration_prepare`, `adb_endpoint_pending`, `adb_endpoint_unresolved`, `device_registry_input_operation`. The member stays a string (no received-location type yet, B5) |
| Q11 | `DiagnosticDetailRecord.operation` | changed values | A2-P | A2-P | the input operations are `input_tap`, `input_swipe`, `input_long_tap`, `input_key`, `input_text`, `input_reset`, `input_close` (were one word); the member stays a string (it relays other crates' operations until the A2b/A2c merge follow-up and B5) |
| Q12 | restart failure value `stop_result` (vocabulary `emulator_stop_result`) | unchanged tokens | A2-P | A2-P | the host maps the stage location to the token by an explicit table (was the last dotted segment); the 19 tokens and their values are unchanged |
| Q13 | `runtime.failed` outcome of a receipt-build failure | changed code | A2-P | A2-P | `runtime_receipt_build_failed` (runtime-host) replaces the client's `runtime_receipt_invalid`, which means a receipt the client cannot read |
| Q14 | `RuntimeMaterialReadFailure.operation` (wire); connection and lifecycle detail `operation` | changed values | A2-P | A2-P | `material_read_deadline_compute`, `material_read_length_convert` (were the shared code spellings `material_read_deadline_overflow`, `material_read_length_overflow`); the size refusal of an effective configuration is `effective_configuration_size_check` (was the code spelling `effective_configuration_too_large`) |
| Q15 | check-config v1 `mumu_root_unresolved.reason` | changed value (wire) | A2-P | A2-P | `<category>.<stage>` with the new stage spelling, e.g. `native.mumu_manager_registry`; A6a's check-config v2 replaces it |
| Q16 | unlock-owner v1 `error.stage` | unchanged bytes | A2-P | A2-P | the host's unlock stages are locations (`unlock_owner_file`, `unlock_owner_ledger`); actingd prints `journal` / `ledger` as before until A6a's v2 |
| Q17 | MuMu capability profile `EmulatorCapabilityEvidence.failure_semantics` (Status capabilities) | changed text (wire) | A2-P | A2-P | the stage names inside the text follow Q10; `evidence_ref` stays `mumu_manager.info` / `mumu_manager.control` |
| Q18 | task diagnostic stream (`task-diagnostic.v2`): `TaskDiagnosticRecognitionError` | changed members | A2-P | A2-P | `severity` and `code` removed (M378 §8.1: the outcome's category replaces a copied severity); `outcome` required: the REC-1/REC-2 outcome when the evaluation refused to conclude (V3, T1), else the recognition pack's registered code (`recognition_pack_invalid`, `recognition_pack_target_unsupported`, `recognition_pack_provider_missing`, `recognition_pack_provider_failed`, `recognition_pack_provider_response_invalid`, `ocr_region_unresolved`, `recognition_pack_text_read_refused`); `deny_unknown_fields` kept; schema not bumped |
| Q19 | task diagnostic stream: the unexecuted guard row's `outcome` | changed values | A2-P | A2-P | `contained_task_guard_skipped` (info, a guard the operation declares unguarded) or `contained_task_guard_page_mismatch` (warning), registered kernel codes; was `outcome_relay_unregistered` with `raw_code` `trusted_coordinate` / `page_mismatch`; the phase is unchanged |
| Q20 | `perf.monitor_degraded` `outcome` | changed values | A2-P | A2-P | a rejected observation event is `performance_event_rejected` {`event_type`} (new key, `vocab:event_type`) caused by the contract's sanitization outcome (was `outcome_relay_unregistered` with `raw_code` `<code>.<event type>`); a pipeline failure carries its error's outcome (code, `operation`, links), a sampler failure its code; severity unchanged (`error`); codes mode keeps `event_type` |
| Q21 | `RuntimeInstanceStatus.capabilities.version`: `EmulatorVersionEvidence::Unavailable.reason` | changed type (wire; persisted in every Status record's `runtime_state`) | A2-P | A2-P | the closed token `not_probed` (was the English text "Backend versions have not been probed."); the member is kept (the structure of `Status`) |
| Q22 | PPOCR node placement diagnostic `nodes[].placement_reason` | changed type | A2-P | A2-P | a one-token closed enum serialised `unavailable`; zero byte change |
| Q23 | the kernel's online-observation rows (observation artifact; `ContainedPageObserved` facts) | changed values | A2-P | A2-P | a recognition pack error's `code` is the registered spelling of Q18 (was PascalCase); `page_batch_failure.unexecuted[].reason` is `batch_validation_failed` / `batch_terminated` (was PascalCase); `severity` and `role` keep their PascalCase (B4) |
| Q24 | `actingledger export --task-evidence` `gaps` | renamed token (wire) | A2-P | A2-P | `artifact_unverifiable` (was `artifact_verification_failed`, also a contract code) |
| Q25 | actinglab `resource restore` report `data.gaps[].codes`; a draft `task.json` `provenance` gap | renamed token (wire, file) | A2-P | A2-P | `lab_input_failed` (was `input_failed`, the contract code's spelling; CD-8) |
| Q26 | resource-tooling `package validate` `recognition_pack_diagnostics[].unsupported_targets[]` | changed members (wire) | A2-P | A2-P | `{id, method?, mask}` (`method` the pack spelling `rgb_count` / `hsv_count`, absent for `ncc`; `mask` boolean); was `{id, reason}` English. The Lab's `recognition_unsupported_targets[].reason` keeps its member (B4) and reads `method=rgb_count, mask` |
| Q27 | selection-policy document `gates[].gate_id`, `scoring[].term_id` | tightened rule (resource file format, not a ledger shape) | A2-P (coordinator ruling 5) | resource side | a token (lowercase ASCII letters, digits, `_`, `.`, `:`, `-`; at most 128 bytes as before), the form of the `rule_id` value its reasons carry (Q7); the declaration gate and contained task admission refuse another id with `resource_declaration_invalid`, reason `invalid_value`, at `/gates/<j>/gate_id` or `/scoring/<k>/term_id` of the policy document ([selection-graph.md](selection-graph.md), Select step) |

### 1.16 A5: actingctl, MCP and the watchdog speak outcomes (#378 A5, M378 §7.1, §7.3, §7.5, §7.6)

Workflow #378 slice A5 writes the types, the readers and the writers in one PR, so it is the code
slice and the first writer of every row below. None of them is a ledger record; they are the
`runtime-outcomes` interface (M378 §10; revision 1, declared for v0.12.0, §6) and the watchdog's own files. No
old-version reader: a v1 report, a v1 `state.json` or the v1 MCP error shape is not read. The new
codes, keys (`argument`, `argument_problem`, `instance_selector`, `mcp_tool`, `material_state`,
`material_limit`, `req_id`, `owner_closed`, `process_alive`, `run_state`,
`resource_targets_rejection_reason`, `media_type`; `owner_epoch` retyped from integer to `id`),
vocabularies (`argument_problem`, `material_state`, `material_limit`, `run_state`,
`resource_targets_rejection_reason`; `mcp_blocker` gains `state_root`, `scheduling_pause`,
`runtime_upgrade`) and the domain `instances_` are catalog entries, not rows (§7).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| O1 | actingctl stdout: exactly one JSON line per exit, `{…result, "outcome": <envelope>}` on success, `{"outcome": …}` on failure; `status --config` prints its records under `records`; a refused receipt keeps `{receipt, outcome}` | CLI interface | A5 | A5 | every subcommand but `mcp-serve` / `mcp-config`; one success code per subcommand; stderr carries only the developer line `<LEVEL> actingctl: <code> …` of a warning, error or fatal outcome (and `DIAG actingctl:` lines of the diagnostic sink); warnings beside a result are `warnings: [{outcome}]` (D3); `ACTINGCOMMAND_DETAIL=codes` keeps only registered names |
| O2 | actingctl and `actingwatch` exit codes: 0 success/info/warning, 1 error, 2 fatal; a denied or failed receipt exits at least 1 | process interface | A5 | A5 | replaces the watchdog's 10–17 and the launcher's 20–23 (their conditions are codes now); actingd's own exits stay A6a's |
| O3 | MCP tool error `{class, outcome}`; warning `{outcome}`; JSON-RPC transport error `{code, message: <code spelling>, data: {outcome}}` (-32022 keeps `data.supported`, `data.requested`) | MCP interface | A5 | A5 | no `code`, `message`, `blocked_by`, `details` or `explain` member; what must change is the value `mcp_blocker`; a relayed actinglab or Runtime code is a link; a Runtime refusal keeps the receipt's outcome on top with the receipt's `receipt_state` and `request_id`; an actinglab error's details are a `foreign_other_error` link (`raw_source` `process`, their JSON as `raw_text`); `mcp-serve --detail` (`codes` or `full`) |
| O4 | MCP job snapshot `job.outcome` → `job.job_result` (the answer, or `{error {class, outcome}}`) | renamed member (M378 L6) | A5 | A5 | `ac_get_run` of a job handle |
| O5 | `actingcommand.watchdog.report.v2`: `{schema_version, outcome, attention: [envelope…], …data}` for `status`, `run-once`, `install`, `uninstall` and the usage exit | report (stdout) | A5 | A5 | v1's `decision`, `code`, `exit_code`, `detail` are gone; members that held prose or a code spelling are outcomes or tokens; `live_owner.started_by_watchdog` |
| O6 | `<root>\watchdog\state.json` `actingcommand.runtime-watchdog-state.v2`: `last_decision {code, values}`; `starts[].start_result {code, values}` (was `starts[].outcome`, M378 L6) | watchdog file | A5 | A5 | a file of another schema is set aside (`state.json.other-schema-<ms>`, `watchdog_state_reset`) and the budget restarts; start results `watchdog_start_pending`, `watchdog_started_runtime`, `start_timeout` or the failure's code |

### 1.17 Wave 4 slice D1c (#387 M387 §3, §4, §6, §7)

D1c brings its own shapes (rows R1–R4 of §1.5, written here as built, and M398 §6.4's two
items) and writes them; [emulator-control.md](emulator-control.md), "Stuck-recovery ladder",
and [runtime-config.md](runtime-config.md), "The recovery ladder section", are the contract.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| LD1 | `RecoveryTriggerStage::StabilityWarnings` (`stability_warnings`) | new enum value (R1) | D1c | D1c | ladder records only: `instance_preparation_finished` refuses it (and `task`) |
| LD2 | `RecoveryLadderTrigger` + `rule?` (token, the trigger item's `id`), `matched?` (`{by: "code"}` / `{by: "class", ladder_class}` / `{by: "warning_burst", count, window_ms, first_unix_ms, last_unix_ms}`), `start` (`immediately` / `after_performance_release`, required) | new members (R2) | D1c | D1c | `rule` and `matched` absent exactly on a `resource_fence` trigger (#403: a fence matches no item); validation checks the record agrees with itself, no code table |
| LD3 | `recovery_ladder_started` `rungs` one to three in the configured order, no action twice; + `config_digest` (64 lowercase hex, the SHA-256 of the section's values; a configuration's hash, not a package digest); + `profile?` (token) | changed and new members (R3; M398 §6.4) | D1c | D1c | `profile` stays absent until RD-4's ladder profiles exist |
| LD4 | `RecoveryRungSkipReason` + `before_start_rung`, + `device_not_ready` (M398 §6.4); outcomes `recovery_rung_skipped_before_start_rung`, `recovery_rung_skipped_device_not_ready` (info) | new enum values, new codes (R4) | D1c | D1c (`before_start_rung`); RD-5 (`device_not_ready`) | |
| LD5 | `recovery_ladder_suppressed` + `rule?` | new member (R4) | D1c | D1c | absent for a fence's ladder |
| LD6 | `recovery_ladder_finished` `rungs_tried` ≤ 3 | rule against the action vocabulary (`RecoveryRung::ALL`), not a fixed ladder | D1c | D1c | `RecoveryRung::LADDER` retired |
| LD7 | `runtime.failed` of `recovery_ladder_code_unclassified` (warning, `raw_code`) | existing record type, new values on a D1b code | D1c | D1c | once per process and code; system links |
| LD8 | `config.parameters.recovery_ladder` (15 rows), `config.parameters.host_claims` (1), `config.parameters.standard_packs` (`root`, when the file names it) | new sections of an existing fact family | D1c | D1c | lists as canonical JSON text; `standard_packs.root` moved from actingd's own configuration (NAV-6 deviation 5), not settable |
| LD9 | `config_value_problem` + `duplicate_id`, `stage_not_allowed`, `match_missing`, `code_unknown`, `code_retired`, `burst_invalid`, `start_rung_out_of_range`, `rungs_invalid`, `bound_out_of_range`, `renewal_cap_exceeded`, `claim_ttl_out_of_range`; `recovery_ladder_config_invalid` + `config_value_problem?` | vocabulary tokens, an optional value | D1c | D1c | the same key and problem on `config_value_invalid` through an instruction |
| LD10 | `InstanceStuckRecovery` loses `Default` and `DEFAULT_COOLDOWN_SECS`; `is_stuck_recovery_trigger` and `is_preparation_recovery_trigger` removed | contract API removal (M387 §6.3) | D1c | — | an instance's defaults are the section's `enabled` / `cooldown_secs` in effect |

Configuration keys (§5): the `recovery_ladder.*` and `host_claims.*` rows of §5, as listed, in
`distribution/windows/actingd.defaults.json`; the per-instance `stuck_recovery` and
`stuck_recovery_cooldown_secs` take `section:recovery_ladder.enabled` / `.cooldown_secs`.
Deviations from §5's defaults: rung 3's `stop_bound_ms` is 210 000 (today's bound since #381 B),
not 190 000; no `perf_hold` item (its code `perf_hold_expired` arrives with P-h); the item
`navigation_exhausted` (M-373 §15 ruling 1, `start_rung` 3) comes first, its code
`contained_task_navigation_recovery_exhausted` registered pending by D1c (runtime-host, error,
`pack_logic_mismatch`) for NAV-5 to emit.

### 1.18 Touch module (#406, approved 2026-10-14)

The touch module's 0.12.x slice T-3 adds a box-confined, multi-turn, single-touch drag. `InputAction`
is a wire shape (`runtime.rs`) and a ledger-validated one (an input intent's `provenance`, a task
effect's action), so by ORD §3.5 item 12 v0.12.0 already decodes it. Its other parts add no
persisted shape: centre-hotspot sampling records the same sampled region and landing point as
today, and the least-recently-used landing-point history lives in memory only. Workflow #406
T12 (§1.136) writes centre-hotspot sampling with the algorithms and parameters of §1.25 X11 and
X12, keeps the history in memory only, and adds the `touch` configuration section
(`actingd-config` revision 7). Workflow #406 T3 (§1.137) is H1's first writer: the kernel
generates it from the click `single_touch_path_drag_v1` (`package` revision 5) without sampling
evidence, the input intent records no execution plan for it, every backend that performs
segmented swipes performs it, and the host no longer refuses it (`input_action_unsupported` stays
registered, and nothing emits it).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| H1 | `InputAction::SingleTouchPathDragV1 {box_x, box_y, box_width, box_height, points: [{x, y}], segment_duration_ms: [ms], corner_hold_ms}` | reserved enum value (wire and ledger) | SH-1 (decodable) | #406 T-3 (0.12.x) | the box lies on the screen and is not empty; 2 to 32 points (`MAX_PATH_DRAG_POINTS`), each on the screen and inside the box; exactly `points.len() - 1` segment durations, each 1..=60 000 ms (the input duration bound), and `corner_hold_ms` at most 60 000; event action `input.swipe`, as the existing drag's; `Debug` redacted; before T-3 nothing writes or executes it: the host refuses an input request that names it with `input_action_unsupported` (runtime-host, error, `none`, pending), and the kernel's and the Lab port's paths refuse it too |

### 1.19 SEL-L4a: the selector's record shapes (#308, selector model S0 v2.1 §1.9, §3.4)

SEL-L4a writes the types, the readers, the validation and the round-trip tests of these shapes,
never a writer (S0 §6 row L4a). They are part of revision 3 because they merge before the v0.12.0
tag (S0 §1.9, revision rule): `task.selection_evaluated` and its projection deny unknown fields, so
no 0.12.x build may add a member to them. The scan session writes them: SEL-L3 (`dry_run_scan`)
and SEL-L4b (select over a scan, and a single-frame select's `on_empty`).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| SL1 | `task.selection_evaluated` `projection` | changed member: tagged by `schema_version`, `actingcommand.candidate-projection.v1` (bytes unchanged) or `actingcommand.candidate-projection.v2` (`ScanCandidateProjection`, new) | SEL-L4a | SEL-L3, SEL-L4b | v2 is `{schema_version, page_id, layout_id, layout_kind: repeated_anchor, axis (vertical, horizontal), frame, coverage {state (not_started, incomplete, early_stopped, complete), start? (start_check, rewind_check), end? (end_check), extent_px?}, viewports [≤256] {motion (start, forward, corrective), content_offset_px, tracked_instances, readable_instances, unknown_instances, frame_id}, reposition {rewind_viewports, return_viewports}, candidates [≤64] {id, instance_index, actionable: true, content_rect, fixed_viewport, features}, unknown [≤64] {position_index, content_rect, first_read_viewport, cause (identity_unread, feature_unread, contradicted, unknown_dropped), identity?, identity_unknown?}, gated_out {count, identities_sha256}, candidate_set_sha256}`. The coverage state decides its members (`not_started` none, `incomplete` `start`, `early_stopped` `start` and `extent_px`, `complete` all three); `identity` is present exactly when the cause is not `identity_unread`; viewport indices name held viewports. Reader bounds 256 / 64 / 64, refused as `candidate_projection_budget_exceeded` (`scan_viewports`, `scan_candidates`, `scan_unknown`); the writer's byte budget is 32 KiB (`projection_bytes`). `candidate_set_sha256` covers `schema_version`, `page_id`, `layout_id`, `layout_kind`, `axis`, `frame`, `coverage.state`, `candidates`, `unknown` and `gated_out`, features as in v1, and leaves out the scan's path: `viewports`, `reposition`, the other coverage members, `fixed_viewport` and `first_read_viewport` |
| SL2 | `TaskSelectionConfirmation` `identity_matched {candidate_id, frame_id, content_offset_px}`, `identity_unconfirmed {identity_state (absent, unknown, different), observed_identity?, identity_unknown?}`, `reposition_failed {outcome, stage (return, confirm)}` | new variants, v2 only | SEL-L4a | SEL-L4b | `reposition_failed` carries its failure as `outcome`, as `guard_failed` and `capture_failed` do (E3), where S0 wrote `code`; it is the record's outcome and codes only in the public projection (E9). `identity_matched` names the selected candidate; `observed_identity` is present exactly when the state is `different`; `matched` and `mismatched` stay v1 only |
| SL3 | `task.selection_evaluated` `unknown_check` | new optional member | SEL-L4a | SEL-L4b | `{unknown_instances, blocking (none, unknown_instances, outranked_possible), upper_bound {kind: none_unknown \| none_eligible \| max {milli} \| unbounded}}`; present exactly when the step declares `scan` or `on_empty`, so every v2 record has it (`invalid_task_selection_record`, field `unknown_check`) |
| SL4 | `task.selection_evaluated` validation | the seven path rules of S0 §1.9 (review-2 M3) | SEL-L4a | — | refused as `invalid_task_selection_record` with the field `path_rule_1` … `path_rule_6`, and `confirmation` for rule 7; rule 2 is the v2 projection's own (`invalid_candidate_projection`, `viewports`). A record with `unknown_check` chooses exactly one candidate (`selected`). A v1 record keeps rules 3–7 without the `unknown[]` and `verdicts` clauses, and without `unknown_check` only "attempted iff selected" |

### 1.20 Wave 6 slice RD-1b (#398 model-398 v1.1 §3, §11, §12)

RD-1b brings its own shapes and writes them; [device-binding.md](device-binding.md) is the
contract. No member carries a device serial: a handset is recorded by its fingerprint and shown
by its label `handset:<alias>`.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| RB1 | `runtime.instance_bound` + `device?` (`RuntimeInstanceDeviceBinding`) | new optional member | RD-1b | RD-1b | `{kind, fingerprint?, bound_at_unix_ms?, bound_via?}` (`deny_unknown_fields`): `kind` `handset` (`InstanceDeviceKind`, `emulator` reserved for the record); `fingerprint` the ledger's salted SHA-256 of the serial (`sha256:<64 hex>`, field `device_serial`); `bound_via` `cli`, `mcp`, `ui` or `manual` (`DeviceBoundVia`); the last three present together for a bound handset and absent for an unbound one; only a handset records `device`, so an emulator's record is unchanged |
| RB2 | `runtime.instance_bound` of an instance addressed by a serial | changed rule | RD-1b | RD-1b | no `adb_host` and no `adb_port` (model-398 §3.4: a configured `serial` recorded the default `127.0.0.1:16384` before); a `device` requires `serial_configured`, `binding_source` `explicit` and physical provenance; refused otherwise with `invalid_instance_binding_device` (contract, error) |
| RB3 | `SecretField::DeviceSerial` | new enum value of the fingerprint fields | RD-1b | RD-1b | the fingerprint's domain label `device_serial` (`Sha256SecretFingerprinter`); not itself persisted |
| RB4 | `session.instance.available` = `false` with `source_detector` `runtime.device-link`, `source_snapshot_id` `device_link:unbound` | new source of an existing instance fact record | RD-1b | RD-1b | a handset the private binding file binds no device to; RD-2 adds the other link states under the same prefix ([instance-fact-store.md](instance-fact-store.md), "Backend self-check availability") |
| RB5 | `<state root>\private\device-bindings.json`, `actingcommand.device-bindings.v1` | private file format, not a ledger shape | RD-1b (reader) | RD-1c (`actingctl devices bind`), or a person | `{schema, bindings: [{alias, serial, bound_at_unix_ms, bound_via}]}`; never exported, archived or recorded (architecture guard RD-1b-T10) |
| RB6 | `config.parameters.instance`: `instance.<instance id>.device_kind` | new per-instance configuration row | RD-1b | RD-1b | string `emulator` / `handset` from the entry's `device.kind`; default `none`, so only an instance that names `device` has the row (no row for an emulator-only configuration); not settable; key-table `file_path` `InstanceMemberAt("device.kind")` |
| RB7 | check-config v1: `error.detail` `{alias, device_config_problem}` / `{path, device_binding_problem}` / `{alias}`; warning `device_binding_unused:<alias>`; a handset's `instances[]` `adb_host` / `adb_port` `null` | interface members | RD-1b | RD-1b | schema version stays `v1` (additive); [actingd-check-config.md](actingd-check-config.md), "Devices" |
| RB8 | `ledger-forensics::InstanceBindings.devices` (fingerprint to instance ids, first-binding order) and `InstanceBinding.device_fingerprint` | read surface | RD-1b | RD-1b | beside the port map ([global-ledger-query.md](global-ledger-query.md)) |

Configuration keys (§5): per instance `device { kind, transport }` (`kind` `emulator` or
`handset`, absent = emulator; `transport` a handset's only, `usb` or absent; `tcp` reserved and
refused), parsed, validated and reported by RD-1b, read by RD-1b; no default value (an instance
without `device` is an emulator). With it the §5 row of `capture_backend` `scrcpy` takes effect:
a handset may select `scrcpy`. A handset's alias is at most 248 bytes (coordinator ruling
2026-10-13). The codes `device_config_invalid` (values added), `device_binding_missing`,
`device_binding_file_invalid`, `device_binding_unused` (actingd), `invalid_instance_binding_device`
(contract), the location `device_endpoint_unbound` (runtime-host), the keys
`device_config_problem`, `device_binding_problem` and their vocabularies are catalog entries,
not rows (§7). The `config.parameters` capacity (§5.1) gains one row per handset instance.

### 1.21 Wave 6 slice REC-7 (#396, REC-M v1.2.1 §9)

REC-7 brings and reads both rows; neither is a ledger shape, and no ledger record or persisted
member changes ([resource-declarations.md](resource-declarations.md), section Task schema `0.9`
template masks, `verify_templates` color checks and the threshold floor). The new location
`target_match_exact` (the stage of a masked search past its limit) is a catalog entry, not a row.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| RC1 | task schema `0.9` `anchors[]` and `verify_templates[]`: `mask` (`{"type": "range", "lower", "upper"}`) or `maskRange` (`[lower, upper]`); `verify_templates[].color_check`; recognition pack schema `0.7` template target `mask` (`type` `range` only) | new members (resource file format, not a ledger shape) | REC-7 | resource side | gray bounds 0..=255, `lower <= upper`; both source forms derive the same pack `mask` and pack schema `0.7`; pack schema `0.6` and task schemas before `0.9` refuse them (`UnconsumedField`); operations still refuse a mask |
| RC2 | admission of a package with its bytes (Runtime, `actinglab package digest`, `package bundle`): a mask selects at least 16 template pixels, not all of one value under `ccoeff_normed`; a `ccorr_normed` threshold above 0 is at least the template's flat-field score in milli plus 50 | tightened rule (resource file format, not a ledger shape) | REC-7 | resource side | refusal `resource_declaration_invalid`, `InvalidValue` at the derived pack's `/targets/<i>/mask`, `/targets/<i>/threshold` or `/defaults/template_threshold`; packages admitted before may be refused, so the resource repositories are checked with `package bundle` before a build with REC-7 is deployed |

### 1.23 Wave 6 slice SR-1 (#386 SCHED §2.1, §2.2, §2.4, §9; #405 random delay)

SR-1 brings its own shapes and writes them; [runtime-config.md](runtime-config.md), "The
`scheduling` section", and [business-identity-consensus.md](business-identity-consensus.md),
"Observe-only retries", are the contract. No new event type and no new member of a persisted
record: the sampling codes carry their registered values (spec §10, values not structure).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| SQ1 | `config.parameters.scheduling` (21 rows: §5's 17 keys, `recognition.sample_timeout_retries` and `random_delay.{enabled, min_ms, max_ms}`); `config.parameters.instance` + `instance.<id>.scheduling.random_delay.{enabled, min_ms, max_ms}` (3 rows per instance) | new section of an existing fact family; new rows | SR-1 | SR-1 | lists as canonical JSON text; every key settable; a per-instance random-delay key defaults to `section:scheduling.random_delay.<member>` |
| SQ2 | the outcomes of `recognition_sample_deadline_exceeded` and `candidate_sample_deadline_exceeded` carry `attempts`, `timeout_ms`, `stage`; `recognition_sample_budget_insufficient` / `candidate_sample_budget_insufficient` carry `required_ms`, `remaining_ms`; `recognition_sample_budget_invalid` carries `required_ms`, `limit_ms`, `pack_operation_id?` | registered values on the `task.failed` terminal, the run's receipt and an admission refusal | SR-1 | SR-1 | `stage` is one of the kernel's new locations `observation_entry`, `observation_before_input`, `observation_after_input`, `observation_page_wait` |
| SQ3 | `blackout_config_invalid` at start and in `check-config`, with `field` (a JSON pointer into the configuration) and `blackout_id?` | existing code (SR-0), first emitted | SR-1 | SR-1 | through an instruction the same entries are `config_value_invalid` (`section_check`) |
| SQ4 | `recognition_sample_deadline_or_permission_lost` | code retired, `replaced_by` `recognition_sample_deadline_exceeded` | SR-1 | — | the kernel's checkpoint returns `Result<(), E>`; `sampling_supported()` says whether a runtime samples |
| SQ5 | the `task.failed` terminal of a wait under target consensus whose screen never meets the consensus: a page wait, a step's postcondition and a `linear_steps` wait (first step, intermediate page, next step, retry decision) | new rule of existing outcomes (shapes unchanged) | TEST-FU (wave-17 acceptance rulings 1-2) | TEST-FU | after a frame that gave no awaited page, a window whose rest can no longer hold the next sampled recognition while the task's remaining time still can is spent, as an elapsed one is: the wait ends with its own miss (`contained_task_page_unknown`, `contained_task_step_unconfirmed`; `contained_task_linear_entry_unmatched`, `contained_task_linear_intermediate_unobserved`, `page_confirmation_failed`), class `pack_logic_mismatch`, so it reaches the ladder; it was `recognition_sample_budget_insufficient` (SQ2), which stays for a task whose remaining time cannot hold the recognition and for every first observation of a window; found by TEST-FU's G3 tests (exec key 21); a run of optional operations' wait (`linear_wait_set`) is not covered |
| SQ6 | the admission of a policy dispatch intent: its refusal `policy_facts_stale` | new rule of an existing refusal (shapes unchanged) | TEST-FU (coordinator ruling, 10-16 22:0x) | TEST-FU | Workflow #369 model v3.1 §3.3 W-1 (the other instances' intents run in the same cycle): admission refuses an intent `policy_facts_stale` only when the evaluation's `fact_snapshot_id` moved and the intent's instance view moved too; the view is that instance's snapshot, the facts and catalog pools whose scope selects it (its instance, server or game; a pool the catalog does not declare selects every instance), the priority offsets for it or for every instance, its own task states and outcomes, and every host; it was the whole snapshot alone, which the cycle's earlier dispatch on another instance always moves, so a cycle's intent on a second instance was refused before it was written; a change on the intent's own instance, server or game still refuses it, the decision identity and the recorded `fact_snapshot_id` stay the evaluation's, and the view lives in memory with the pending intent (no persisted or wire member, no code); [instance-fact-store.md](instance-fact-store.md), "Admission staleness per instance"; found by TEST-FU's A-4 G5 row 2 test (apps key 6) |

Configuration keys (§5): the `scheduling.recognition.*` row and the five sections of the next
row, as listed, in `distribution/windows/actingd.defaults.json`; Workflow #405's random delay
(§5 row `scheduling.random_delay.*`, coordinator's order of 2026-10-14).
`scheduling.rerun.condition_codes` lists SCHED §9's eight codes; `perf_run_yielded` and
`perf_hold_expired` are registered by SH-1 (pending), since a code list entry must be a listed,
active code (train 6). Since 0.12.28 the default also lists `contained_task_paused` (Alice's
decision 92, 2026-10-29; a changed default, no new key and no `actingd-config` revision;
[policy-suspension.md](policy-suspension.md), "Which runs").

### 1.24 Wave 6 slice P-c (#385 PERF §10, §11)

P-c brings the `performance` section of `crates/runtime-config` (one file per subsection,
`src/performance/{monitor,responsiveness,pacing,calibration}.rs`) and writes it;
[runtime-config.md](runtime-config.md), "The performance section", is the contract.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| PC1 | `config.parameters.performance` (97 rows) | new section of an existing fact family | P-c | P-c | every key of PERF §10 (§5 rows below), in row order monitor, responsiveness, pacing, calibration; lists and objects as canonical JSON text; settable: `performance.pacing.curve` and `performance.pacing.hold.*` only |
| PC2 | `config.parameters.performance_monitor` gone (recorded empty once after an earlier start had it); `config.parameters.performance_control` keeps `escalation_samples`, `recovery_samples`, `transition_cooldown_ms` | rows of existing sections removed | P-c | P-c | `performance_monitor.sample_interval_ms` → `performance.monitor.sample_interval_ms`; `performance_monitor.pressure_start_samples` / `_end_samples` → `performance.pressure_start_samples` / `_end_samples` (file members unchanged); `performance_control.clock_jump_threshold_ms` → `performance.pacing.clock_jump_ms`; `performance_control.normal_heavy_dispatch_limit` / `pressured_heavy_dispatch_limit` → `performance.pacing.heavy_dispatch_limit.normal` / `.paused` |
| PC3 | `perf_config_invalid` (error, ladder class `none`, `config_key`, `config_value_problem?`; domain `perf_`, owner `runtime-config`) | new code | P-c | P-c | a start and `check-config` refuse with it before the ledger opens (exit 1); through an instruction the same values are `config_value_invalid` |

Configuration keys (§5): the `performance.*` rows of §5, every key of PERF §10, parsed, checked and
defaulted in `distribution/windows/actingd.defaults.json`. §5's §10.4 count is corrected to 19
(`bounds.*` are four keys). `performance.pacing.curve` holds at most 70 points (coordinator
ruling 2026-10-14: the 8 KiB `ConfigureSettings` value limit). Deviations from §5's spellings: the pressure streaks keep their
file members, so their keys are `performance.pressure_start_samples` / `_end_samples` rather than
`performance.pressure.start_samples` / `.end_samples`; the stage boundaries are
`performance.calibration.bounds.s1` to `.s4` (PERF writes `bounds.S1`; a configuration key is a
lowercase token). `execution.defaults.*` is not in P-c (see the PR). Read in v0.12.0 by P-c: the
streaks (as before), `performance.pacing.clock_jump_ms` and `.heavy_dispatch_limit.*` (the
controller; before, code constants); every other key by its later slice.

### 1.25 Shape re-check additions (10-14)

Workflow #383 SH-2, the second v0.12.0 shape slice (ORD §3.5 item 12, rule R5). The coordinator's
re-check of 2026-10-14 found 0.12.x shapes that later models and later rulings bring (the #331
ruling of 10-13, #405 and #406 approved 10-14, B5's v0.12.1 change). SH-2 writes their types,
readers, validation and round-trip tests as SH-1 did (§1.1-§1.7), never a writer; the "First
writer" column names the slice that writes them later. Every new member is omitted when absent,
so today's records keep their bytes. X11 and X12 supersede §1.18's sentence that centre-hotspot
sampling adds no persisted shape: a draw must name its real algorithm. Coordinator rulings
applied: M392's rename of `FrameAndLogicalDisplayDiffer` is dropped (V4 keeps today's spelling);
no later ledger revision for DA or REC, their shapes are here; a drawn random delay is not kept
across a restart, so L7 gains no member. Left to the v0.12.0 slices that bring their own shapes:
NAV-5, SEL-L3b, OUT-2, EXEC-1/2, DRILL-1, the W rows of D2-S5 and P-m, and the keys of RD-2,
RD-5b and RD-4.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| X1 | `PerformancePressureKind::FullscreenContention` (`fullscreen_contention`), `::LedgerBacklog` (`ledger_backlog`); peak `PerformancePressureValue::LedgerBacklog {tier, buffer_bytes, buffer_records}` (`ledger_backlog`) | new enum values of `PerformancePressureRecord.kind` and `.peak` (`perf.pressure_started`, `perf.pressure_ended`, the `pressures` of a performance context) | SH-2 | P-s | the reader checks kind and peak in pairs: `fullscreen_contention` with a `process_load` peak (as `third_party`), `ledger_backlog` with a `ledger_backlog` peak whose `tier` (1 to 3) names the record's severity (1 elevated, 2 high, 3 critical) and whose buffer holds bytes and records; otherwise `invalid_performance_pressure_value`; PERF §3.4, §8 D11, §13 P-s; the spellings are the ones `performance.pacing.pressure_floor.kinds` already accepts (P-c, §1.24) |
| X2 | `AgentWakeKind::DailySummary` (`daily_summary`) | reserved enum value (`agent.wake_requested`) | SH-2 (decodable) | AGT-1 | #331 ruling 10-13: the first wake triggers are the daily results summary and recovery ladder exhausted (A1's `ladder_exhausted`); A1's other reserved values stay |
| X3 | `AgentWakeData.trigger_event_id`, `.trigger_sequence` | changed: both optional; `trigger_sequence` never 0 when present (`invalid_agent_wake`) | SH-2 | AGT-1 (a timer's wake names neither), AO-4 (stops writing `trigger_sequence`, [ledger-observer.md §13](ledger-observer.md#13-identity-without-ledger-positions-ao-3-ao-4-ao-4q-in-v0120)) | today's writer still writes both; the host's agent replay compares the sequence only while a record carries it and indexes wakes by trigger only when one is named |
| X4 | `runtime.lifecycle_observed` phases `tagged_state_loaded` and `tagged_state_enabled` (L5, L6): `outcome` | new optional member of each | SH-2 | AO-6 (`ledger_tagged_state_loaded`, with a `ledger_tagged_entries_dropped` link), AO-5 (`ledger_tagged_state_seeded`) | an info outcome ([ledger-observer.md §11.5, §11.6, §16](ledger-observer.md#16-outcome-codes)); another category is refused; the record's severity is its category (E2) |
| X5 | `ledger.health_observed` (L2): finding `ledger_tag_key_unclassified` {`fact_key`} | allowed finding | — (L2's reader restricts the category to warning or error, not the code, so it already reads it; SH-2 adds the round-trip test) | AO-5 | [ledger-observer.md §11.2](ledger-observer.md#112-which-keys-are-tagged-configuration-ledg-82) |
| X6 | `CaptureGeometry.safe_insets` (`CaptureInsets {left, top, right, bottom}`, device-frame pixels) | new optional member (`task.geometry_observed`: `observation`, `frame.producer`) | SH-2 | DA-4 | model 392 §8.2; absent = not read, or no cutout section; left + right and top + bottom stay below the logical display's width and height (`invalid_sanitized_payload`, field `safe_insets`) |
| X7 | `TaskGeometryObservation.display_map` (`TaskGeometryDisplayMap {kind: identity \| scale \| translate, scale_num, scale_den, scale_width, scale_height, base_width, base_height, scale_by: height \| width, scale_area: screen \| safe_area, anchor_area: screen \| safe_area, insets_source?: reported \| declared, declared}`) | new optional member | SH-2 | DA-4 | model 392 §4, §8.1, §8.3: sizes non-zero; `scale_num / scale_den` reduced and at least 1; `identity` = factor 1 on the base size, `scale` = a larger factor on the base size, `translate` = any other scale frame and only with a declaration (§11); a map measured on the safe area names its insets' source; otherwise `invalid_sanitized_payload`, field `display_map` |
| X8 | `TaskGeometryPhase::Changed` (`changed`); `TaskGeometryRecheckTrigger::ExtentChanged`, `::RotationChanged`, `::InsetsChanged` (`extent_changed`, `rotation_changed`, `insets_changed`) | new enum values | SH-2 | DA-4 (DA-2's map rebuild) | model 392 §7.3, §8.3: a `changed` observation has a frame, one of the three change triggers and no original failure; a `recheck` keeps a failure trigger and its original failure; otherwise `invalid_task_geometry_phase`. V4 keeps `frame_and_logical_display_differ` (the rename is dropped) |
| X9 | `TaskDiagnosticTargetData.correction` (`TaskDiagnosticCorrectionData {dx, dy}`) | new optional member of the task diagnostic stream (`task-diagnostic.v2`) | SH-2 | DA-2 | model 392 §5.3, §8.4: the match's top-left corner minus the mapped region's, in scale-frame (base) pixels like REC-5's `scale_milli`; `i16` each, the correction radius (at most 64) bounds them; unknown members refused; no ledger record type is added |
| X10 | §4 rows `display.logical_extent`, `display.safe_area.reported`, `display.safe_area.declared`, `display.map` | instance-store fact keys, tagged | — (fact values) | DA-4 (`declared`: a person or an agent) | register only; model 392 §8.1; DA-4 books them in the [capacity ledger](instance-fact-store.md#capacity-ledger-workflow-383-ao-0) when it adds them |
| X11 | `InputSamplingAlgorithm::CenterHotspotNormalV1` (`center_hotspot_normal_v1`), `::CenterHotspotLeastRecentV1` (`center_hotspot_least_recent_v1`) | reserved enum values of `InputSamplingEvidence.algorithm` (`TaskSemanticPayload.sampling`) | SH-2 (decodable) | #406 T-1, T-2 | a draw names its real algorithm; today's writer records `xorshift64_uniform_rect_v1` |
| X12 | `InputSamplingEvidence.parameters` (`InputSamplingParameters {sigma_ratio_milli, max_redraws, candidate_count?, chosen_candidate?}`) | reserved optional member | SH-2 | T-1 (`sigma_ratio_milli`, `max_redraws`), T-2 (the candidates) | the configured values at the draw, so T-4 replays from the seed; T-2's pick also depends on the in-memory landing history, so `chosen_candidate` (0-based, in draw order) names the winner. Present exactly for a centre-hotspot algorithm; `sigma_ratio_milli` > 0; the two candidate members exactly for `center_hotspot_least_recent_v1`, `chosen_candidate` < `candidate_count`; otherwise `invalid_sanitized_payload`, field `sampling_parameters` |
| X13 | the ADB recovery evidence (`adb_recovery`): `AdbTargetRecovery.endpoint`, `.initial_error` optional; `outcome` on `AdbTargetRecovery`, `AdbRecoveryStep` and `AdbCommandEvidence` | changed members, new optional members (`runtime.lifecycle_observed` phase `adb_target_recovery`, a `runtime.failed` lifecycle failure) | SH-2 | B5 (v0.12.1) | E7 and the payload's own note: B5 moves the device text to `foreign_adb_error` links with `raw_text`; the error that started the recovery stays recorded, as `initial_error` or as `outcome` (`invalid_adb_target_recovery`); a record is at least as sensitive as the highest `raw_text_sensitivity` on the evidence's outcomes (E7); today's writer keeps the text members and writes no outcome |
| X14 | `policy.dispatch_intent`: `random_delay` (`PolicyRandomDelay {due_at_unix_ms, planned_start_unix_ms, delay_ms}`) | new optional member | SH-2 | #405 | only a dispatch intent carries it (`invalid_policy_payload`, field `random_delay`); `due_at_unix_ms` non-zero; `delay_ms` is the delay applied, at most 86 400 000 (the configuration's bound, §5), 0 when the allowed period left none; `planned_start_unix_ms` = `due_at_unix_ms` + `delay_ms`; not kept across a restart (no L7 member) |

The register-only rows of the re-check are in §3 (W7, W13) and §4 (X10's keys,
`host.kept_folder_cleared`).

### 1.26 Wave 8 slice AO-1 (#383 ledger-observer.md §9, §15)

AO-1 is the first writer of L1's dump gaps (`dump_incomplete`, `dump_invalid`), of L3 and of the
dump file (L12), and brings the `ledger_observer` section of `crates/runtime-config`
(`src/ledger_observer.rs`); [runtime-config.md](runtime-config.md), "The `ledger_observer`
section", is that section's contract.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| AO1 | ledger dump file (L12) | the members L12 leaves open | AO-1 | AO-1 | header `owner_epoch` is the owner epoch's id (`epoch_…`) and `runtime_commit` is present when the build knows its commit; a record's `draft` holds the sanitized draft's members, `artifacts` as projected references with their object keys; `captured_at_unix_ms` is the draft's time until AO-2 captures; `capture_seq` counts from 1 in each dump; every hash is 64 lowercase hex digits; a place that already holds a dump file is not written to ([ledger-observer.md §9.3](ledger-observer.md#93-dump-file)) |
| AO2 | `config.parameters.ledger_observer` (21 rows) | new section of an existing fact family | AO-1 | AO-1 | the keys of §5's `ledger_observer.*` row in §15's order; decimals and lists as canonical JSON text; settable: `flush_lag_warn_ms`, `probe_interval_ms`, `probe_sample_keys`, `status_gap_window_ms` |

### 1.27 Wave 8 slice MCP-4 (#389, MCP-0 command table v1.1 §4.4)

MCP-4 brings the read kind of §3 W2 and the one read output below; neither is a ledger shape, and
no persisted record changes ([runtime-interface.md](runtime-interface.md), the actingctl and MCP
tables; [instance-fact-store.md](instance-fact-store.md), "Reading the seeds").

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| M41 | `actingctl facts --program`, `facts --instance <selector>` and `status --config` ↔ `ac_facts` (`scope` program, instance or config; `instance`; `key_prefix`): `{scope, read_at_unix_ms, snapshot \| records, records_total, records_kept}` | read output of a general read (no persisted record; the CLI line carries it beside its outcome) | MCP-4 | MCP-4 | `snapshot` is the `RuntimeFactSnapshot` (program) or the instance's `InstanceFactView` (W2); `records` the `config.*` records (config); `read_at_unix_ms` is the Runtime's clock at the read (the snapshot's `taken_at_unix_ms`, the instance read's `sampled_completed_at_unix_ms`); `records_total` counts the records read, `records_kept` those answered after `key_prefix` (program and instance only); the selector is the alias, instance_id or ADB port, resolved with one status read; MCP exports an answer past its output budget (`answer_exported`); CLI success codes `runtime_facts_reported`, `instance_facts_reported` (new) and `runtime_config_reported` |

### 1.28 Wave 8 slice D2-S2 (#384 D2M §3.2, §3.4, §3.5, §6 S2; v1.4.2)

D2-S2 brings its own shapes and writes them; [resource-readings.md](resource-readings.md)
("Kernel reading", "Host", "Recognition maintenance packages") and
[runtime-config.md](runtime-config.md) are the contract. No new event type and no new member of a
persisted record; the reading facts keep their shape.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| DS1 | `config.parameters.instance`: `instance.<instance id>.recognition_package` | new per-instance configuration row | D2-S2 | D2-S2 | string, the location of the instance's recognition maintenance package from the entry's `recognition_package.package`, resolved against the configuration file's directory; a location only, never a digest (option 丙); default `none`, so only an instance that names it has the row; not settable (read once at assembly); key-table `file_path` `InstanceMemberAt("recognition_package.package")` |
| DS2 | `config.parameters.scheduling`: `scheduling.fact_refresh.recognition_join_timeout_ms` | new configuration row | D2-S2 | D2-S2 | milliseconds `0..=1800000`, default `10000` (`actingd.defaults.json`), settable; the one `scheduling.fact_refresh.*` key D2-S2 reads (the join, D2M §3.5), parsed here ahead of D2-S4's other six keys of the §5 row |
| DS3 | `config.subsystems` row `recognition_packages` | new row of an existing fact | D2-S2 | D2-S2 | only when the configuration binds a recognition package; `enabled` when the start kept one; outcome `config_subsystem_configured` with one `note` link `fact_reader_binding_ignored` (caused by `fact_reader_binding_invalid`) per binding the start runs without |
| DS4 | task diagnostic stream (`task-diagnostic.v2`): a reading mark that gave no reading is one `error` row (`TaskDiagnosticErrorData::Recognition`, phase `post_admission`, the mark's `target_id`) whose `outcome` is `contained_task_resource_reading_unresolved` (warning; `reading_id`, `fact_key`, `reading_problem`) | new value of an existing member | D2-S2 | D2-S2 | written on the frame the mark was read on; a joined reading's OCR rows also carry that frame's `frame_id`, not the last captured frame's; schema not bumped |
| DS5 | `fact.published` of a reading | changed rule of an existing record (shape unchanged) | D2-S2 | D2-S2 | `observed_at_unix_ms` and the `frame:` part of `source_snapshot_id` are those of the frame the reading was taken on, on any page of the task (they were the terminal frame's); readings are published in the order of their frames' capture times |
| DS6 | package format: bundle `maintenance[].uses` value `recognition` (at most one per server); task `resource_readings[].page_id` any page the task declares; `linear_steps` tasks may carry `resource_readings` | new value and relaxed rules (resource file format, not a ledger shape) | D2-S2 | resource side (D2-R1, D2-S6) | a reader that does not know `recognition` refuses the declaration; it is part of `package` revision 3, declared once for v0.12.0 (§6) |
| DS7 | check-config v1: `error.code` `fact_reader_binding_invalid`, stage `config_recognition_binding`, `error.detail` `{alias, path, reader_problem, package_id, fact_key}`; `instances[].recognition_package` `{package, fact_keys}` for an instance that binds one; warning `fact_reader_missing:<instance>:<fact key>` | interface members | D2-S2 | D2-S2 | schema version stays `v1` (additive) |
| DS8 | `actinglab package bundle` output: `warnings` (absent when empty), one `recognition_pack_undeclared` outcome per pack that qualifies as a reader but is not declared `recognition` | interface member | D2-S2 | D2-S2 | resource-tooling `PackageBundleResponse.warnings` |

Configuration keys (§5): per instance `recognition_package` `{package}` (built, DS1) and
`scheduling.fact_refresh.recognition_join_timeout_ms` (built, DS2). The codes
`fact_reader_binding_invalid`, `fact_reader_binding_ignored`, `fact_reader_missing`,
`fact_reader_document_mismatch` (runtime-host; registered, emitted once actingd loads a
scheduling-layer document and D2-S4 reads `min_interval_ms`), `recognition_pack_undeclared`
(resource-tooling, domain `recognition_pack_` gains its owner), the reclassification of
`contained_task_resource_reading_unresolved` to warning (ladder class `none`), the keys
`reading_id`, `reading_problem` and the vocabulary `reading_problem`, the `reader_problem` token
`package_unreadable` and the actingd location `config_recognition_binding` are catalog entries,
not rows (§7).

### 1.29 Wave 8 slice REC-6 (#396, REC-M v1.2.1 §8)

REC-6 brings and writes both members of R6-1, in the task diagnostic stream; R6-2 is resource file
format, not a ledger shape. No ledger record type is added. The retired code
`contained_task_post_admission_ocr_target_in_page_gate` and the new codes, locations, keys and
vocabularies are catalog entries, not rows.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| R6-1 | task diagnostic stream (`actingcommand.runtime.task-diagnostic.v2`): `TaskDiagnosticTargetData.value_test` (`TaskDiagnosticValueTestData`) and `TaskDiagnosticTargetData.skipped` (`TaskDiagnosticTargetSkip`) | two new optional members, absent when unused | REC-6 (#396, REC-M v1.2.1 §2, §8.3, §8.4) | REC-6 | `value_test` = `{left: TaskDiagnosticValueOperand, op, right?, passed}` with `TaskDiagnosticValueOperand {target_id, raw_text?, value?, problem?}` (`deny_unknown_fields`), `op` one of `eq`, `ne`, `lt`, `le`, `gt`, `ge`, `parses`, `problem` one of `empty`, `invalid`, `overflow`, `out_of_range`, `region_unresolved`, `low_confidence`, and `right` = `{"kind": "const", "const"}` or `{"kind": "operand", "operand", "mul", "add"}`, absent for `parses`; REC-1's `failure` (V3) then carries `value_test_unresolved` or `value_test_not_met`. `skipped` = `page_unmatched` on a page target row the page recognition left unevaluated (not passed, no child rows); the page detector's `PageTargetEvaluation.skipped` carries the same value |
| R6-2 | task schema `0.9` `checks[].value_test` (`{left, op, right?}`); recognition pack schema `0.7` target type `value_test`; `OcrUnsignedIntegerFormat::ClockHms` (`clock_hms`, for fields, readings and value tests); a value test side's `trim` `whitespace_all_v1`; the page gate of a page that collects or reads OCR | new members and values, one rule relaxed (resource file format, not a ledger shape) | REC-6 | resource side | older task and pack schemas refuse the new members (`UnconsumedField`); a value test in a page rule's `forbidden`, directly or through a check, is refused at `/pages/<i>/forbidden/<j>`; a page that collects fields, collection-mode targets or readings may gate on any backend and on those targets themselves (the gate's observation is the collection's); they are part of `package` revision 3, declared once for v0.12.0 (§6) |

### 1.30 Wave 8 slice L-5b (#382: the K/T remnants L-5 left)

L-5b removes what is left of the retired K/T policy (Workflow #382 L-5, after #375 R5d) and
writes no new shape; [ledger-store.md](ledger-store.md) and
[actingd-check-config.md](actingd-check-config.md) are the contract. No 0.12 build writes a
`failed_run` intent (L-5), and no reader accepts a ledger or a configuration written before
v0.12.0, so nothing written by a 0.12 build becomes unreadable.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| L5-1 | `ArtifactEvictionIntentRecord.failed_run` and its types `FailedRunRetentionEvidence`, `FailedRunRetentionBasis`, `FailedRunRetentionPolicy` | removed persisted member | L-5b | — | an intent names its `success` source; a stored intent that still carries `failed_run` does not decode, so replay refuses it (`corrupt_ledger_record`, location `decode_sqlite_record`) and the ledger does not open |
| L5-2 | `ArtifactRetentionPublicFact::EvictionIntent.failed_run` (the public read data of `artifact.eviction_intent`) | removed wire member | L-5b | — | it was never present on a record a 0.12 build wrote |
| L5-3 | actingd keys `frame_retention_failed_run_successes`, `frame_retention_failed_run_days`; their `config.parameters.top_level` rows and key table rows | removed configuration keys and rows | L-5b | — | a configuration that names either key is refused when loaded (`config_decode_failed`, start and `check-config`); an instruction naming one is `config_key_unknown` |
| L5-4 | `invalid_failed_run_retention_policy` (contract), `artifact_eviction_failed_run_basis_conflict` (ledger) | codes retired, no replacement | L-5b | — | no longer emitted; the fragment entries stay, marked `retired` |

### 1.31 Wave 9 slice PKG-S2 (#336 option 丙, M-PKG v1.1 §4.3)

PKG-S2 removes the package digests of §2's PKG-S2 row from the ledger and the evidence and
names every package by its `package_id` (`package_label`). It adds no record type. A stored
record that still carries a removed member does not decode, so replay refuses it
(`corrupt_ledger_record`, location `decode_sqlite_record`) and the ledger does not open; no
reader of the old form exists. The rows below fix what §2 leaves open: which members are
optional, and what replaces each removed one.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| P2-1 | `TaskSemanticFact::PackageAdmitted {package_label, task_label, response_deadline_monotonic_ms?}` | removed persisted member `package_sha256` | PKG-S2 | PKG-S2 | `package_label` is the package's `package_id` (a task semantic label, `invalid_task_semantic_label`); the code `invalid_task_package_fingerprint` is deleted (pending) |
| P2-2 | `EntryRecoveryPackageAdmitted.package_label`, `EntryRecoveryCompleted.package_label` (required), `EntryRecoveryFailed.package_label` (optional, omitted when absent) | changed persisted members (`package_sha256` → `package_label`) | PKG-S2 | PKG-S2 | each the layer's own `package_id`; a failure has none only when the layer's package was refused before its `package_id` was read (a request's own recovery binding names a path only); the run recovery view pairs a failure with a later completion of the same label; the failure identity's `pre:` scope is `pre:<I>`, `I` the id hash of that label (§2 format v2 scope; the rest of the format stays with PKG-S3) |
| P2-3 | `EffectiveConfigurationFacts::EntryRecovery.package_label` | changed artifact member (`package_sha256` → `package_label`) | PKG-S2 | PKG-S2 | ledger-forensics refuses an empty, overlong or control-character label (`effective_configuration_fields_invalid`) |
| P2-4 | `TaskSelectionPolicy {path, policy_sha256, policy_id}` | removed persisted member `package_sha256` | PKG-S2 | PKG-S2 | `policy_sha256` is the evaluator's identity of the document, not a package digest; it stays |
| P2-5 | `ResourceDeclarationRejection {package_label?, program_version, issue}` | removed members `declared_package`, `verified_package`; new optional `package_label` | PKG-S2 | PKG-S2 | present when preparation read the refused package's control `package_id`; validated as a label (`invalid_resource_declaration_identity`) |
| P2-6 | `LabOperationPrepared.package_label?`, `ContainedObservationEvidence.package_label?`, `ContainedPageObservation.package_label?` | removed members `expected_package_sha256`, `actual_package_sha256`; new optional `package_label` | PKG-S2 | PKG-S2 | the package's control `package_id`, absent when the control declares none (a Lab observation package may omit it); the client checks that the evidence and the receipt name the same label; `resource restore` pairs records with the original package by it and refuses a package that declares none or another one; the restore provenance and report carry `package_label` |
| P2-7 | `EvidencePackage {file_name, verification}` | removed member `sha256` | PKG-S2 | PKG-S2 | evidence archive manifest (`actingcommand.evidence.v2`) |
| P2-8 | post-admission OCR comparison report (`truth_set_path`, …) and runtime-client `RuntimeOfficialOcrComparison` | removed member `truth_set_sha256` | PKG-S2 | PKG-S2 | the reader refuses a report that carries it (`deny_unknown_fields`) |
| P2-9 | run status `actingcommand.run-status.v1` `{package_label?, recovery_package_labels}`; run summary `actingcommand.run-summary.v1` `package_label` | changed wire members (`package_ref`, `recovery_packages`, `package_digest` → labels) | PKG-S2 | PKG-S2 | the summary names the one admitted package (`run_summary_package_fact_count_invalid` stays); `run_summary_package_digest_invalid` and `_mismatch` are deleted (pending); a settlement-interrupted summary names no package; the MCP status schema follows |
| P2-10 | reading facts' `resource_bundle_hash` | changed value, shape unchanged | PKG-S2 | PKG-S2 | lowercase hex SHA-256 of `package:<package_id>` (D8) |
| P2-11 | resource files: a select step's `policy.sha256`, an OCR dictionary reference's `sha256`, a truth set's `sha256` | optional and ignored, whatever the value | PKG-S2 | resource side | contract type `IgnoredPin`; never validated, compared, recorded or echoed; PKG-S1b refuses the keys again |

### 1.35 Wave 9 slice REC-8 (#396, REC-M v1.2.1 §10)

REC-8 brings and reads both rows; neither is a ledger shape. No ledger record type or task
diagnostic member is added: a target the shared read serves keeps the OCR evaluation row it had,
with the read's blocks. The new location `text_read_shared` is a catalog entry, not a row.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| R8-1 | task schema `0.9` `candidate_layouts[].shared_ocr` and recognition pack schema `0.7` `candidate_layouts[].shared_ocr`: `{region, languages, timeout_ms, model_ref, model_sha256, blocks, line_merge?}` | new member of a `fixed_slots` layout (resource file format, not a ledger shape) | REC-8 | resource side | [selection-graph.md](selection-graph.md), section Shared OCR read; the region is a rectangle or `full_frame` (the source's `rect` / `full_frame` region form, derived into the pack's), `blocks` must be `detect` with a `line_merge`; a `repeated_anchor` layout refuses it (`UnknownField`), task schemas before `0.9` too (`UnconsumedField`); a shared read no slot or feature target uses is refused; one projection counts it as one provider evaluation per frame it serves, and the targets it serves none; it is part of `package` revision 3, declared once for v0.12.0 (§6) |
| R8-2 | the text read budget of a page recognition (`ocr-fields.md`, section Located text) | tightened rule (resource file format, not a ledger shape) | REC-8 | resource side | REC-6 left open how the field, reading and `0.7` collection targets a page gate names count: each such target a page names, directly or through a check, is one read of its own region (its observation is kept by target), at admission (`resource_declaration_invalid`, `InvalidValue` at `/pages/<i>` of the page that exceeds the budget) and at run time (`text_read_budget_exceeded` before the gate's provider call); a task whose gates name more than four of them, or more than one frame of their summed area with the page set's other reads, is refused where it was admitted since REC-6 |

### 1.36 Wave 9 slice S6a (#369 model v3.1 §7 row 6, H-5; ORD §3.5 item 17)

S6a brings and writes the configuration key `start_emulator` and its report;
[runtime-config.md](runtime-config.md) and [actingd-check-config.md](actingd-check-config.md) are
the contract. No ledger record type or persisted member is added. host-metrics'
`network_connectivity()` is an in-memory read with no persisted shape; its reason vocabulary
`connectivity_reason`, the code `start_emulator_requires_discovery_binding` and the domain
`start_emulator_` are catalog entries, not rows (§7). Nothing acts on the key before S6c.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| E6-1 | `config.parameters.instance`: `instance.<instance id>.start_emulator` | new per-instance configuration row | S6a | S6a | boolean from the entry's `start_emulator`; default `none`, so an entry that does not name it has no row and is off; not settable (read once at assembly); `true` only on a discovery-bound instance, otherwise the start and `check-config` refuse with `start_emulator_requires_discovery_binding` (actingd, error, ladder class `none`, `alias`) |
| E6-2 | check-config v1: `instances[]` member `start_emulator` (boolean) on a `discovery_pending` entry; `error.detail` `{alias}` of `start_emulator_requires_discovery_binding` | interface members | S6a | S6a | schema version stays `v1` (additive); an `explicit` entry has no such member |

Configuration keys (§5): the per-instance `start_emulator` row, parsed, checked and reported by
S6a; S6c reads it.

### 1.37 Wave 10 slice A6a: actingd speaks outcomes (#378 A6a, M378 §2.3, §2.4, §7.4-§7.6, §8.2, §11)

A6a writes the types, the readers and the writers in one PR, so it is the code slice and the
first writer of every row below. Rows A6-1 to A6-7 and A6-10 are the `runtime-outcomes`
interface (M378 §10; revision 1, declared for v0.12.0, §6) or actingd's and the watchdog's own files, not ledger
records; A6-5 and A6-8 reach persisted bytes. No old-version reader: a v1 report, a FATAL line
or a v1 backup manifest is not read. The new codes (`runtime_ready`, `runtime_stopped`,
`ledger_opened`, `ledger_open_measured`, `owner_released`, `scheduling_pause_restored`,
`scheduling_pause_dropped`, `check_config_passed`, `ledger_maintenance_completed`,
`unlock_owner_completed`, `suspended_reported`, `actingd_fallback_log_failed`,
`adb_install_pin_invalid`, `suspended_frame_missing`, `suspended_frame_unreadable`,
`panic_unrecovered`, `runtime_halted`), the retired `runtime_fatal` and location
`resource_package`, the vocabulary enums `file_state` and `owner_probe_state`, and the context key
`install_root` are catalog entries, not rows (§7).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A6-1 | actingd daemon lines on stderr: per outcome the developer line `<INFO\|WARN\|ERROR\|FATAL> actingd: <code> <text>` and `OUTCOME actingd: <envelope>` (full) in one write; `ledger_opened`, `owner_released`, `scheduling_pause_restored` / `_dropped`, `runtime_ready`, then `runtime_stopped` and, when a condition stopped actingd, that condition as the last line (the terminal outcome) | process interface | A6a | A6a | replaces the stdout lines `actingd ready …`, `actingd ledger_open …`, `actingd owner_resources_released_by_exit …`, `actingd scheduling_pause_*` and the `FATAL actingd:` line; an accepted shutdown of a held start ends as `runtime_stopped` with the shutdown as its link; `startup_recovery`, `frame_retention` and `policy_monitor` stdout lines are unchanged |
| A6-2 | actingd exit codes: the terminal or report outcome's category, 0 success/info/warning, 1 error, 2 fatal | process interface | A6a | A6a | was 0 or 1 (the ledger dump stop already exited 2) |
| A6-3 | start refusal before the ledger opens: the same daemon lines, also appended to `%LOCALAPPDATA%\ActingCommand\logs\actingd-<ms>.log` when stderr is a console or absent; the outcome carries `install_root` once actingd resolved its installation | process interface, log file | A6a | A6a | a failed append is one `WARN actingd: actingd_fallback_log_failed` developer line; std's console check stands in for `GetFileType` (actingd forbids unsafe code), so a `NUL` stderr is not detected |
| A6-4 | check-config v2 `actingcommand.actingd.check-config.v2`: `{schema_version, outcome, report?}`; `report` holds v1's ok members less `schema_version`, `status`, `warnings` and `mumu_root_unresolved`; a pass is `check_config_passed` with `note` links (each ignored override, the MuMu resolution, unused device bindings, standard-pack problems, missing fact readers); a refusal is its code with `stage` (`config_load`, `config_assemble`, `config_vision`, `config_validate`, `config_resource_package`, `config_recognition_binding`, `policy_state`) and its values | report (stdout) | A6a | A6a | v1's `error {code, stage, detail}`, `remedy`, `loader_code`, `loader_message` and the stage tokens `load`, `assemble`, `vision`, `validate` are gone; the adb refusal carries `adb_path`, `aliases`, `tools_dir`, `files`; `catalog_activation_not_newer` the `active_` and `configured_` catalog values; runtime-config refusals `config_key`; `start_emulator_requires_discovery_binding` its instance as `alias` at check-config and at the start (register E6-2's `error.detail`) |
| A6-5 | ledger-maintenance v2 `actingcommand.ledger-maintenance.v2`: the receipt's members with `outcome` (`ledger_maintenance_completed`); a failure is `{schema_version, outcome, activated: false}`; `MaintenanceWarning` is `{outcome}` in the receipt's `warnings` and in the backup manifest (`DatabaseBackup.warnings`) | report (stdout), backup manifest | A6a | A6a | v1's `status: "failed"` and `error {code, operation, cause, cleanup, warnings}` are gone; cleanup failures are `cleanup` links, warnings `note` links, a failed verify read's phases a `ledger_open_measured` note link; a v1 backup manifest does not decode |
| A6-6 | unlock-owner v2 `actingcommand.actingd.unlock-owner.v2`: `{schema_version, outcome, …}`, the receipt's members on success (`unlock_owner_completed`), `journal_appended` on a failure, whose `stage` is a location (`config_load`, `unlock_owner_file`, `unlock_owner_ledger`) | report (stdout) | A6a | A6a | v1's `status` and `error {code, stage}` are gone (register Q16); `owner_resource_unconfirmed` carries `owner_pid` and `owner_probe_state` |
| A6-7 | suspended v2 `actingcommand.actingd.suspended.v2`: the report's members with `outcome` (`suspended_reported`); a row's `error_code` and `failure_code` are `failure` (the recorded outcome), `failure_identity` and `run_failure`; a frame comparison's `reason` is `outcome` and `ccoeff_error` is `ccoeff_failure` | report (stdout) | A6a | A6a | v1's `status`, `code`, `operation` and `detail` are gone; actingctl `diagnose` reads the outcome's category |
| A6-8 | the Runtime error code `runtime_fatal` is `runtime_halted` (receipt projections, the `runtime_code` value of receipts and of `runtime.failed` outcomes); a latched host answers a later request with an error condition under `runtime_halted` (its `caused_by` link) and with a fatal condition as itself | changed value (wire, persisted) | A6a | A6a | M378 §2.3 M1a; `runtime_restart_required` is the contract's (shared) |
| A6-9 | watchdog: the terminal outcome (the last `OUTCOME actingd:` or `OUTCOME acforward:` line) decides: fatal holds (`watchdog_fatal_hold`, its link), an error considers a start, anything else leaves the journal rows; a fallback-log outcome naming another `install_root` is ignored; a run-out budget is `panic_unrecovered` when every start in the window ended in `runtime_restart_required`, else `watchdog_budget_exhausted` with the last terminal outcome as its link | watchdog decision | A6a | A6a | the FATAL-line scan, the restartable stop codes and the `owner_conflict` exception are gone; report v2's data member `fatal {log, line}` is `terminal {log, outcome}` |
| A6-10 | MCP `ac_binding_draft` `check_config {config, exit_code, outcome, report_export}` | MCP output | A6a | A6a | `status` and `error` were copied from check-config v1 |

### 1.38 Wave 10 slice AO-2 (#383 ledger-observer.md §7, §8, §10, §11.1)

AO-2 is the first writer of L11, L4 (`capture_seq`, `captured_at_unix_ms`), L1's `tail_unconfirmed`
and `record_rejected` gaps, and L2's segment and revision findings. The rows below fix what those
rows leave open. The capture interface (`actingcommand-contract` `ledger_capture.rs`) and the
shared watermark module (`artifact-store` `watermark.rs`) are in-memory shapes, not rows. The
seven new locations (`configure_ledger_capture`, `ledger_capture_record`, `ledger_spill_records`,
`ledger_flush_captured`, `ledger_index_rows`, `ledger_tail_gap`, `ledger_read_spilled`) are
catalog entries (§7). No configuration key is added: the queue reads §15's existing
`ledger_observer.*` keys.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| OB1 | `ledger_index` (L11) | the members L11 leaves open | AO-2 | AO-2 | `STRICT, WITHOUT ROWID`; `sequence` and `source_revision` in the ledger's ordered integer encoding; `value_sha256` = the record's `record_sha256`; source columns from the record's capture. Key formats, the `open`/`request`/`lifecycle` rules and the transaction: [ledger-observer.md §11.1](ledger-observer.md#111-the-index-table-ledger_index), "As built by AO-2". No migration: a database created before AO-2 has no table, so its first batch fails (§9.2) |
| OB2 | record envelope `capture` (L4) | which records carry it, numbering | AO-2 | AO-2 | carried by every record that passes the capture queue: captures, direct appends (capture time = the draft's own time), segment and revision findings, gaps of refused records; an imported dump record keeps its dump line's capture. `capture_seq` counts from 1 for each writer that opens (per owner epoch), contiguous in capture order; it is unique only within an epoch. Not carried until AO-3 and AO-4: the records the writer drafts itself inside a command (retention and eviction, Lab pin, prior-epoch closes, settlement continuation, recovery, the import's own records, the tail gap). `source` is written when an item carries one; the stores set it from AO-3 |
| OB3 | ledger dump file record line (L12) | changed members | AO-2 | AO-2 | `draft.source` (new, optional): a captured record's source revision; `capture_seq` is the record's capture number in its writer's queue, no longer counted from 1 in each dump; `captured_at_unix_ms` is its capture time. Spill lines are appended without a trailer; at the seal, the writer's own records in flight take the next capture numbers and the fatal record the last. A refused capture is spilled as its gap |
| OB4 | `ledger.gap_recorded` `record_rejected` and `tail_unconfirmed` (L1) | values | AO-2 | AO-2 | `record_rejected`: its own event id; range and `timestamp` = the refused record's capture time; `record_count` 1; `event_id` and `source` of the refused record; `outcome` `ledger_record_rejected` caused by the refusal's code, with `event_id`, `ledger_source`, `source_lineage`, `source_revision`. `tail_unconfirmed`: at the running writer's open after the import, when the previous owner was not closed cleanly and no imported dump ended sealed; range = last record's time → the open |
| OB5 | `ledger.health_observed` (L2) | values | AO-2 | AO-2 | `ledger_capture_buffer_busy` and `_overloaded`: `segment_edge`, `count` and `buffer_bytes` in memory; `ledger_capture_spilled`: `segment_edge`, records and bytes spilled (started: the first spill; ended: the whole segment, when the dump file is deleted); every end adds `elapsed_ms`. `ledger_store_revision_skipped`: no edge; `ledger_source`, `source_lineage`, `expected_revision`, `source_revision`. Only the running Runtime's writer reports them; no edge is reported once the close has begun |

### 1.39 Wave 10 slice D2-S3 (#384 D2M §3.6, §6 S3; v1.4.2)

D2-S3 brings its own shapes and writes them; [resource-readings.md](resource-readings.md)
("Adjustments") and [instance-fact-store.md](instance-fact-store.md) ("Adjustment
invalidations") are the contract. No new event type and no new member of a persisted record.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| D31 | `fact.published` of a resource adjustment write | new rule of an existing record (shape unchanged) | D2-S3 | D2-S3 | one record: the adjusted value at the confirmation frame's capture time, the adjusted record's `expires_at_unix_ms` (and a `ttl_policy` of `expires - observed`, `detector_contract`), the lower of the two confidences, `source_detector` `resource_adjustment:<entry task id>/<adjustment id>`, `source_snapshot_id` `run:<run>/frame:<frame>/<adjustment id>/base:<event id>` (the base's `fact.published`); published in a run's terminal stage 2 in capture-time order with its readings, through `publish_facts` with the `ResourceAdjustment` purpose |
| D32 | `fact.invalidated` of an adjustment | changed rule of an existing record (shape unchanged) | D2-S3 | D2-S3 | an instance-scoped `resource.*` / `inventory.*` record may be invalidated by a `task.step_finished` it does not list in `invalidate_on` (`is_adjustment_invalidation`): a confirmed adjustment that read no new value drops the key's record from before the run; the other source rules (published before the trigger, trigger before the invalidation, trigger time, trigger instance in the record's scope, tombstone projection) are unchanged and checked by the store, its replay, the history at a position and the state projection alike |
| D33 | task diagnostic stream (`task-diagnostic.v2`): a confirmed adjustment's outcome is one `error` row (`TaskDiagnosticErrorData::Recognition`, phase `post_admission`, the amount's `target_id` or none) on its confirmation frame: `fact_adjustment_skipped` (info), `fact_adjustment_underflow` (warning), and for an amount that gave no reading `contained_task_resource_reading_unresolved` (warning, `reading_id` = the adjustment's id) | new values of an existing member | D2-S3 | D2-S3 | as DS4; an amount's OCR evaluation rows carry the confirmation frame's `frame_id`; schema not bumped |
| D34 | package format: task `resource_adjustments[] {id, fact_key, operation_id, amount? {target_id, direction: consume \| gain}}` (task schema `0.8`, `0.9`) | new family (resource file format, not a ledger shape) | D2-S3 | resource side (D2-S6) | an older reader refuses the family (`UnknownField`); it is part of `package` revision 3, declared once for v0.12.0 (§6) |

The codes `fact_adjustment_skipped` (info) and `fact_adjustment_underflow` (warning)
(runtime-host, ladder class `none`), the contract's declaration reasons `resource_adjustments_*`
and `resource_adjustment_*` (`declaration_error`), the key and vocabulary `adjustment_reason`
(`base_missing`, `base_expired`, `newer_observation`, `amount_unresolved`, `after_read_missing`)
and the wider description of the key `reading_id` are catalog entries, not rows (§7).

### 1.40 Wave 10 slice OUT-1 (#390, forced compromises O-03, O-04, O-05)

OUT-1 writes no ledger record, wire shape or configuration key, and adds or retires no code.
One resource file format rule is relaxed (O1-1). The catalog rule (a catalog references a
subset of its package's declared outcomes, [scheduling/README.md](scheduling/README.md)
"Runtime Boundary") and the policy input records of declared outcomes a successful run did
not produce ("Settlement Outcome Records") are not persisted; `fact_snapshot_id` hashes the
projected outcomes, so it follows them.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| O1-1 | task schema `0.8` zero-input fields task (`operations: []`): `scheduling_outcome.mappings` | relaxed rule (resource file format, not a ledger shape) | OUT-1 | resource side | one `fields_recorded` mapping whose terminal pages are the fields pages, and any other mappings; every mapping `no_designated_effect`; the target pages are every mapping's terminal pages, no page in two mappings ([ocr-fields.md](ocr-fields.md)); a run that ends on another mapping's page has no OCR observation and no fields report; it is part of `package` revision 3, declared once for v0.12.0 (§6) |

### 1.41 Wave 10 slice EXEC-2 (#390, forced-compromises mapping E-05, E-06)

EXEC-2 brings the observation window and the input recheck of page-graph operations;
[resource-declarations.md](resource-declarations.md), "Task schema `0.9` observation windows and
input rechecks", is the contract. No new event type and no new member of a persisted record: the
frames of a window or a recheck are recorded as every capture, recognition and guard evaluation
is, with the existing timing boundaries (`PostconditionWait`, `PageRecognitionWait`), and a step
finishes with the existing `StepFinished` on the page its window ended on. The kernel uses the
runtime seams it has (`observation_now`, `step_pacing`, `shared_interrupt`); none is added. The
codes
`contained_task_input_stability_unconfirmed` and
`contained_task_observation_stability_unconfirmed` (execution-kernel, error,
`pack_logic_mismatch`) are catalog entries, not rows.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| EX1 | task schema `0.9` operation members `observation_window` (`{until: "window_end", window_ms}` or `{until: "stable", window_ms, frames}`) and `input_recheck` (`{kind: "fresh_frame"}` or `{kind: "stable_frames", frames}`) | new members (resource file format, not a ledger shape) | EXEC-2 | resource side | `window_ms` 1..=60000, `frames` 2..=32 (Runtime admission, `contained_task_operation_invalid`); an older task schema refuses both (`UnconsumedField`), a `linear_steps` task refuses both (`contained_task_linear_invalid`); it is part of `package` revision 3, declared once for v0.12.0 (§6) |

### 1.42 Wave 10 slice MCP-14 (#389, MCP-0 command table v1.1 §4.15)

MCP-14 brings the one read output below; it reads the outcome catalog the build embeds, sends no
Runtime request and changes no persisted record or wire shape ([runtime-interface.md](runtime-interface.md),
the actingctl and MCP tables). Its codes (`outcome_codes_reported`, `outcome_code_unknown`,
`outcome_catalog_unreadable`), the key `code_name` and actingctl's share of the domain `outcome_`
are catalog entries, not rows (§7).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| M14-1 | `actingctl codes <code>` ↔ `ac_outcome_code` with `code`: the code's catalog entry as the catalog holds it, with `code`; `actingctl codes [--prefix] [--owner] [--limit] [--cursor]` ↔ `ac_outcome_code` with `prefix`, `owner`, `limit`, `cursor`: `{codes: [{code, category, ladder_class, status}], next_cursor?}` (MCP adds `truncated` when it cuts a list to its output budget) | read output of a general read (offline; no persisted record; the CLI line carries it beside its outcome) | MCP-14 | MCP-14 | codes in name order; `prefix` matches the start of a code's name, `owner` an entry's `owner`; `limit` 1–100, default 20; `next_cursor` is present while codes past the page remain and holds across processes and both sides for the same `prefix` and `owner` (`cursor_invalid` otherwise); `code` excludes the list arguments; the entry text is the catalog's English (command table §0.4 item 1: the Chinese names stay in the UI) |

### 1.45 Slice D1c-H (#387, Alice 10-14: one failure counts once toward direction ①)

D1c-H changes what direction ① counts, which is memory only, and writes one existing optional
member on one more record; no record type, member, code or key is added.
[emulator-control.md](emulator-control.md), "Stuck-recovery ladder", is the contract.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| CH1 | `runtime.failed` of `instance_resources_fenced` (F1): `RuntimeLifecycleFailureRecord.entered_event_id` | existing optional member, now written on this record | D1c-H | D1c-H | names the record of the unconfirmed failure that fenced the instance, when that failure was recorded; absent otherwise, as before. Direction ① follows it to count the fence and its failure once where the fence's links name the instance alone (a failed lease end) |

### 1.46 Slice U-2 (#389, UIM §3.2, §4.1, §4.2, §5.2; register §1.6 U1, U4)

U-2 binds each connection to its channel and marks, never refuses: no request is refused by
channel or by Lab option before U-6. The connection-to-channel table is memory only, is not part
of any status read and counts no connection ([runtime-interface.md](runtime-interface.md),
"Channels"). Its codes (`request_bind_channel_completed`, `invalid_bind_channel`, and the first
emissions of `channel_binding_exists` and `client_channel_unsupported`) are catalog entries, not
rows (§7).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| U2-1 | `command.received` (G14, §1.6 U1): `channel`, `via` | existing optional members, now written | SH-1 | U-2 | written on the `command.received` the C4 path writes and on `DebugPackage`'s; `channel` ∈ `cli`, `mcp`, `ui`, `lab`, `via` ∈ `mcp`, `ui` and only with `lab`; absent on the Runtime's own command records (scheduled runs, the ladder, the emulator control phases); before AO-4 the other writes show their channel in their receipt only (UIM §5.2) |
| U2-2 | receipt `actingcommand.runtime.receipt.v2`: `channel`, `via` | new optional members (runtime-client 2, W9) | U-2 | U-2 | the door's tag of the connection the request came on; absent on the handshake of a connection not bound yet; a `BindChannel` receipt carries the binding it made (or, refused, the one the connection keeps) |
| U2-3 | `BindChannel {channel, via?}` → `RuntimeResult::ChannelBound {channel, via?}` | behaviour of a W11 kind | U-1 | U-2 | sent once per new connection after `Health` (connect, the `send_or_dropped` reconnect, the contained-task timeout recovery); `via` on another channel than `lab` is refused `invalid_bind_channel` before any binding; a second one is refused `channel_binding_exists` (`channel` = the kept one); records nothing; accepted while an install transition drains |
| U2-4 | transition binding (UIM §3.2 "Transition") | memory only | U-2 | U-2 | until U-6 a connection that never binds is bound at its first request other than the handshake from that request's envelope source: `Ui` → `ui`, `Lab` → `lab`, `Cli` and `Adapter` → `cli`; U-6 deletes it |
| U2-5 | `host.config_overrides` rows: `channel` | existing optional member, now written | D1b | U-2 | the channel of the `ConfigureSettings` that set the value; a restored row whose `channel` is not one of the four values stops the start (`host_config_overrides_fact_invalid`) |
| U2-6 | `lab_module_authoring`, `lab_module_debugging` (§1.6 U4, §5) | configuration keys reported as `config.parameters.top_level` rows | U-2 | U-2 | booleans, `false` in the defaults file (not installed); `source` `explicit` when the file names one; never settable (`read_at_assembly`, not in D1's set); read once at start; nothing is refused by them before U-6 |
| U2-7 | `approval.decision` written by actingd's start-up transcription | existing record, new writer path | — | U-2 | in process instead of a `(User, Ui)` connection with an identity card: provenance stays `(Governance, User, Ui)` (the approval projection requires it); the record links a request id and a correlation id the host mints; actingd declares no governance identity card any more |

### 1.47 Wave 12 slice PKG-S3 (#336 option 丙, M-PKG v1.1 §3.3, §3.4, §4.4)

PKG-S3 removes the package digest from the scheduling binding and the dispatch, writes the
failure identity in format v2 (§2) and lifts a pause by the content hashes that identity
records. It adds no record type and no persisted member; one warning code and one
configuration key. A stored dispatch intent that still carries `package_digest` does not
decode (`corrupt_ledger_record`, location `decode_sqlite_record`) and the ledger does not open;
a v1 failure identity string is not read. [policy-suspension.md](policy-suspension.md) is the
contract.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| P3-1 | `policy.dispatch_intent` (`PolicyDispatchEventData`) | removed persisted member `package_digest` | PKG-S3 | PKG-S3 | the ledger's replay equality of two intents (`same_dispatch_definition`) no longer compares it; `procedure_binding_digest` stays |
| P3-2 | `DispatchIntent.package_digest` (policy evaluation) and `ProcedureBinding.package_digest` | removed members | PKG-S3 | PKG-S3 | `procedure_binding_digest` is SHA-256 over the tuple v3 (`actingcommand.procedure-binding.v3`, `procedure_ref`, `operation_id`, `yield_points`): a package changed in place or found at another path keeps its binding and decisions. The binding carries its package's location (`scheduled_execution.package_path`) outside the tuple; a scheduled run naming another path is `policy_run_identity_mismatch`. The codes `procedure_package_digest_mismatch`, `procedure_package_digest_missing`, `policy_run_package_digest_invalid` and `policy_run_package_digest_missing` are deleted (pending) |
| P3-3 | `policy.execution_recorded` `failure.failure_identity` | changed value (format v2, §2), shape unchanged | PKG-S3 | PKG-S3 | at most 214 bytes; the main package and each layer as `<I><H>`; the pause's recorded hashes are rebuilt from it alone, never from the dispatch |
| P3-4 | `actingd suspended` (`actingcommand.actingd.suspended.v2`) `package` `{paused_hash_prefix, current_hash_prefix, layers[{kind, package_id, paused_hash_prefix, current_hash_prefix, changed}]}`; `lifts_when` `content_hash_changes`; `lifted[].lifted_by` `main_hash \| layer:<n> \| restart`; `lifted[].effective` `next_evaluation \| active` | changed report members (not persisted) | PKG-S3 | PKG-S3 | replaces `paused`/`current` package references, `paused_digest_prefix`/`current_digest_prefix`, `main_digest_changes_or_any_layer_changes`, `main_digest` and `pending_restart`; the report measures each package it names once, read-only; warning `lift_check_hash_unmeasured:<task>/<instance>`. The `takeover` hint keeps `--package-ref` until PKG-S4 removes the flag |
| P3-5 | `lift_check_hash_unmeasured` (warning; `path?`, `package_id?`, `caused_by` the read's refusal), location `check_suspension_lift` | new code | PKG-S3 | PKG-S3 | a note link of the `policy_task_paused` reason a lift check could not judge (M-PKG D18): the pause stays and the package is measured again at a later check |
| P3-6 | `scheduling.suspension.lift_check_min_interval_ms` | new configuration key (§5) | PKG-S3 | PKG-S3 | duration, default 60 000 (`actingd.defaults.json`); read at each lift check |

### 1.48 Wave 12 slice OUT-2 (#390, forced compromises O-01, O-02, R-03, S-01)

OUT-2 brings its own shapes and writes them; [resource-declarations.md](resource-declarations.md)
("Task schema `0.9` designated effect sets") is the contract. A package's designated effects may
be a set whose members repeat up to an explicit cap (a constant, or an instance fact read before
the run's first step), and the run's terminal records each member's count beside its cap. No new
event type, no new member of a persisted record and no configuration key; the policy inputs are
unchanged (a catalog reads the mapped outcome key). The code
`contained_task_designated_effect_cap_unresolved` (execution-kernel, error, `declaration_error`)
is a catalog entry, not a row (§7).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| O2-1 | package format: task `scheduling_outcome.designated_effects[] {operation_id, max_repeats?}`, `max_repeats` an integer or `{fact_key}` (task schema `0.9`) | new member (resource file format, not a ledger shape) | OUT-2 | resource side | 1 to 8 members, each operation once; a constant cap from 1 to 1,000, absent = 1; a fact cap is an unexpired integer instance fact from 0 to 1,000, read once before the run's first step; not with `designated_operation`; an older task schema refuses it (`UnconsumedField`); it is part of `package` revision 3, declared once for v0.12.0 (§6) |
| O2-2 | `task.completed` `TerminalCommitted.scheduling_disposition.effect` (and the same disposition in a scheduling outcome projection's `scheduling_result`): `SchedulingEffectEvidence::DesignatedEffects` (`{"kind": "designated_effects", "effects": [DesignatedEffectTally {operation_label, count, max_repeats}]}`) | new enum value of an existing member | OUT-2 | OUT-2 | written for a run of a package that declares a set: one tally per member in declaration order, `count` the times its effect completed (0 when none), `max_repeats` the cap the run held it to; 1 to 8 tallies, each operation once, `max_repeats` at most 1,000 and `count` at most `max_repeats` (`invalid_scheduling_outcome_declaration`, field `designated_effects`); its mapping condition is `designated_effect_completed` when any count is above 0. A run of a `designated_operation` package keeps `designated_effect_completed` or `no_designated_effect` |

### 1.49 Wave 12 slice SEL-L3 (#308, frozen selector model S0 v2.1 §2.1–§2.6, §3.3, §6 row L3)

SEL-L3 brings the scan session engine of a select step in the execution kernel and its offline
`dry_run_scan`; it writes no new record type, member, wire shape or configuration key. The
candidate projection v2 and the record's scan members are SEL-L4a's (merged); the record of a
scan select, its admission, `Decide`, `Return`, `Confirm` and the tap are SEL-L4b's, which also
reads `select.scan` from a package (L3 reads it only as given). A scan's drags are recorded as
the existing `task.effect_intent` of the select step, as the row below states. The codes
`selection_scan_start_unconfirmed`, `selection_scan_settle_incomplete`,
`selection_scan_alignment_rejected`, `selection_scan_gap_detected`,
`selection_scan_instance_missing`, `selection_scan_identity_mismatch`,
`selection_scan_end_unconfirmed`, `selection_scan_position_unread` (execution-kernel, error,
`pack_logic_mismatch`), `selection_scan_bounds_exceeded` (`budget_or_permission`) and
`selection_scan_replay_frame_missing` (dry run only, `none`), the locations `select_scan_start`,
`select_scan_settle`, `select_scan_align`, `select_scan_observe`, `select_scan_end` and
`select_scan_decide` (the domain `select_` that execution-kernel already holds), and the keys
`viewport_index`, `position_index`, `gap_px`, `expected_identity`, `observed_identity`,
`scan_alignment_failure`, `scan_spacing_failure` and `scan_bound` with their vocabularies are
catalog entries, not rows (§7).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| L3-1 | `task.effect_intent` of a select step: several per step action, one per scan drag (advance, corrective, rewind; SEL-L4b adds the return drags), each linked to the frame it was sampled on, each followed by its input; none followed by `task.effect_completed` | existing record, written more than once per select step | SEL-L3 (kernel); online from SEL-L4b, which first lets a package declare `scan` | SEL-L4b | the step's one `task.effect_completed` follows its selecting tap, so the host's count of designated effects per step (one) is unchanged; the host already keys every intent by `(step_index, operation_label)` and links it to the last captured frame, so it needs no change |

### 1.50 Wave 12 slice REC-5 (#396, REC-M v1.2.1 §7)

REC-5 brings and writes R5-1 in the task diagnostic stream; R5-2 is resource file format, not a
ledger shape. No ledger record type, code, location or key is added
([resource-declarations.md](resource-declarations.md), section Task schema `0.9` multi-scale
templates).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| R5-1 | task diagnostic stream (`actingcommand.runtime.task-diagnostic.v2`): `TaskDiagnosticTemplateData.scale_milli` | new optional member, absent when unused | REC-5 (#396, REC-M v1.2.1 §2, §7) | REC-5 | the factor in milli (500 to 2000) the matched template was resampled by, present exactly on the template data of a target that declares `scales`; the row's box is then the resampled template's. The recognition evaluation's `TemplateEvaluation.scale_milli` carries the same value. In base (scale-frame) pixels, as X9's correction is |
| R5-2 | task schema `0.9` `anchors[].scales` and `verify_templates[].scales`; recognition pack schema `0.7` template target `scales` (`{"factors_milli": [...]}`) | new member (resource file format, not a ledger shape) | REC-5 | resource side | 1 to 4 distinct integers from 500 to 2000, no default; older task and pack schemas refuse it (`UnconsumedField`); a scaled template is refused as a `target_consensus` target, a `repeated_anchor` anchor and an offset feature; where a package is admitted with its bytes, each resampled template keeps at least 4 pixels a side and fits its region (`InvalidValue` at `/targets/<i>/scales/factors_milli/<k>`), and the mask minimum and threshold floor hold per factor; it is part of `package` revision 3, declared once for v0.12.0 (§6) |

### 1.52 Wave 12 slice RD-2 (#398 model-398 v1.1 §4, §10.1, §12; #387 ruling A9)

RD-2 brings its own shapes and writes them. The link of a handset is watched by one
`adb track-devices` process per Runtime, only while a bound handset is configured; the
`device_link` section of `crates/runtime-config` (`src/device_link.rs`) is its configuration and
[runtime-config.md](runtime-config.md), "The `device_link` section", that section's contract. No
member carries a device serial: a handset is its label `handset:<alias>`.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| RD2-1 | `runtime.lifecycle_observed` phase `device_link_observed` `{outcome}` | new lifecycle phase | RD-2 | RD-2 | `outcome` is `device_link_waiting` (warning, values `device_label`, `device_link_state`; once per state a handset enters away from `device`), `device_link_restored` (info, `device_label`) under the handset's instance links, or `device_watch_restarted` (warning) / `device_watch_failed` (error, a spawn failure's OS text its `caused_by` link) with `device_watch_problem` under system links; normal commit sync |
| RD2-2 | `device.link` (instance scope, string, `device_link_state`) | new runtime fact | RD-2 | RD-2 | `device`, `absent`, `offline`, `unauthorized`, `no_permissions`, `other`, `unwatched`; written for a bound handset only, when its link changes |
| RD2-3 | `device.connected` of a handset | new producer of an existing runtime fact | RD-2 | RD-2 | `false` when the handset leaves `device` for a state adb reports; `true` when its first list finds it prepared at `device` and when a restore preparation passes |
| RD2-4 | `session.instance.available` = `false` with `source_detector` `runtime.device-link`, `source_snapshot_id` `device_link:<state>` | the other link states under RB4's prefix (§1.20) | RD-2 | RD-2 | a handset away from `device` is unavailable whatever its self-check says; the link back at `device` and a passing preparation restore the seed, as a self-check withdrawal is restored |
| RD2-5 | `device_link_lost` on a run's failure | new top code of an existing failure | RD-2 | RD-2 | a run whose device failure (backend open, backend operation or capture) met its handset away from `device`: values `device_label`, `device_link_state`; the device failure its `caused_by` link; the error's projection unchanged |
| RD2-6 | the ladder's holding interruption `device_link_lost` (A9) | new interruption reason | RD-2 | RD-2 | a climbing ladder whose handset left `device` is interrupted: the rung fails with `recovery_ladder_interrupted` caused by `device_link_lost`, and `recovery_ladder_finished` `exhausted` is recorded at warning (§2.2b A8) |
| RD2-7 | a self-check claim of the handset's queue (`ClaimKind::SelfCheck`, host claim work `device_link_restore`) | new use of an existing claim kind | RD-2 | RD-2 | the restore preparation after `device_link.settle_ms`; no new claim kind (model-398 §4.3) |
| RD2-8 | `config.parameters.device_link` (6 rows) | new section of an existing fact family | RD-2 | RD-2 | the keys of §5's `device_link.*` row in model-398 §4.6's order; the two wait lists as canonical JSON text; every key settable |

Codes (runtime-host): `device_link_lost` (error, `environment_fault`), `device_link_waiting`,
`device_watch_restarted`, `device_link_reconnect_failed` (warning, `none`),
`device_link_restored` (info, `none`), `device_watch_failed` (error, `none`); keys
`device_link_state`, `device_watch_problem` and their vocabularies; seven `device_link_*`
locations. `device_link_reconnect_failed` is not in model-398 §10.1: it records the failure of
the one `adb reconnect` that `offline_action = kick_once` sends (Fail Loud), and the Runtime then
waits as under `wait`. The defaults file adds `device_link_lost` to
`recovery_ladder.exclude_codes` (coordinator ruling on model-398 §16 item 5).

### 1.54 Wave 12 slice READ-1 (#390, forced-compromises mapping O-08)

READ-1 lets a fraction reading write its capacity as a second reading key;
[resource-readings.md](resource-readings.md), "Second reading key", is the contract. No new event
type and no new member of a persisted record: the capacity is one more `fact.published` of the
existing shape. The contract's declaration reason `resource_reading_capacity_fact_key_invalid`
(`declaration_error`) is a catalog entry, not a row (§7).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| RE1 | package format: task `resource_readings[].capacity_fact_key` (task schema `0.8`, `0.9`) | new optional member (resource file format, not a ledger shape) | READ-1 | resource side | only with `value.format` `current_capacity`; the key rule of `fact_key`; unique among every key the task's marks write; an older reader refuses it (`UnknownField` @ `/resource_readings/<i>/capacity_fact_key`); it is part of `package` revision 3, declared once for v0.12.0 (§6) |
| RE2 | `fact.published` of a mark's capacity | new rule of an existing record (shape unchanged) | READ-1 | READ-1 | one record per capacity read, right after the mark's value record: `key` = `capacity_fact_key`, `content` the capacity (`0..=2^53-1`), `source_snapshot_id` `run:<run>/frame:<frame>/<reading id>/capacity`, an observation of its own (one per reading, as for two marks on one frame; the value record keeps `run:<run>/frame:<frame>/<reading id>`); every other field (observation and expiry, `ttl_policy`, `confidence_milli`, `source_detector` `resource_reading:<entry task id>/<reading id>`) as the mark's value record of the same frame |

### 1.55 Wave 13 slice PKG-S4 (#336 option 丙, M-PKG v1.1 §4.5)

PKG-S4 removes the package digest from every input surface: the IPC requests, the actingd
configuration, actingctl, MCP and the actinglab flags. A package is named by its path only; the
path chooses how it is read (PKG-S1) and its admission is structural. No record type and no
persisted member: the rows are wire, configuration and output shapes. A request or a
configuration that still carries a removed member does not decode (`deny_unknown_fields`); no
reader of the old form exists.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| P4-1 | `ContainedTaskRequest {package_path, recovery?, response_deadline_ms}`, `ContainedTaskRecoveryBinding {package_path}`, `PackageDebugRequest {package_path}`, `ContainedObservationRequest {package_path, targets}`, `ContainedLabOperationRequest {package_path, selection, projection_hint, after?}`, `SavedArtifactOcrRequest {source, package_path, target_id}` | removed wire member `expected_sha256` | PKG-S4 | PKG-S4 | a request that carries it does not decode; the codes `invalid_contained_task_hash`, `invalid_contained_task_recovery_hash` and `invalid_debug_package_hash` are deleted (pending); the host admits each package by its path; the saved-artifact OCR diagnostic artifact no longer echoes `package_sha256` |
| P4-2 | `PackageDebugSummary {task_id, layout, entry_count, resident_bytes, task_count, has_recognition_pack, has_pages, has_navigation}` | removed wire member `verified_sha256` | PKG-S4 | PKG-S4 | |
| P4-3 | actingd configuration: `instances[].startup_package {package}`, `prerequisite_packages[] {package_id, package_path}`, `policy.procedure_manifest[] {procedure_ref, operation_id, yield_points, scheduled_execution?}` (and the resource declaration's procedure binding) | removed configuration keys `expected_sha256`, `package_digest` (§5) | PKG-S4 | PKG-S4 | a file that names one is `config_decode_failed`; the codes `startup_package_digest_invalid`, `prerequisite_package_digest_invalid` and `procedure_package_digest_invalid` are deleted (pending); a binding's or prerequisite package's path chooses its reading: a directory, or a `.zip` / `.json` file (another file is `procedure_package_container_unsupported`); check-config reports `instances[].startup_package` as `{package}` |
| P4-4 | a configured prerequisite package's identity (M-PKG D7) | rule | PKG-S4 | PKG-S4 | the package at a `prerequisite_packages` entry's path must declare that entry's `package_id`, or it is refused with `contained_task_prerequisite_mismatch` (existing code): as a chain layer (as before), as the configured return-home package of a page-graph entry, and on the recovery ladder's first rung |
| P4-5 | actingctl `task-run --package <path> [--recovery-package <path>]`; MCP `ac_run_pack {instance, package, recovery_package?, deadline_s?}` and `ac_pack_check {package}` → `{status, package_id, entry_task_id}` (PKG-S4's interim `{preflight}` until #411 A5, §1.97 PC2); actinglab `--package <path>` or `--zip <path>` | removed flags and arguments `--expected-sha256`, `--package-ref`, `--recovery-expected-sha256`, `--recovery-package-ref`, `package_ref`, `recovery_package_ref`; removed answer members `digest`, `package_ref_check` | PKG-S4 | PKG-S4 | actingctl refuses a removed flag as unknown (`actingctl_usage_invalid`, `argument_problem` `unknown`); MCP refuses a removed argument (`arguments_invalid`, `unknown`) and computes no digest; actinglab refuses the removed flags by name (`validation_failed`) except `lab replay-evidence --expected-sha256`, the evidence archive's own hash (M-PKG §1.3); `actingd suspended`'s `takeover` hint names `--package <package>` only |
| P4-6 | actinglab outputs: `package preflight` `package_label` (was `package_ref`); `package validate` and `lab validate` without `input_sha256`, `hash_source` and `externally_verified`; `package dry-run` result record and answer `package_label` (was `package_sha256`), taking `--package` or `--zip`; capabilities `package_locator {by: path, readings}` (was `package_reference`) | changed output members | PKG-S4 | PKG-S4 | `package dry-run` reads its package by its path, so a content directory is simulated (forced compromise T-01); the dry-run record's `package_label` comes ahead of PKG-S5, which listed it, because its input reference is gone |

### 1.56 Wave 13 slice D2-S4 (#384 D2M §3.3, §3.4, §3.7, §3.9, §6 S4; v1.4.2)

D2-S4 brings its own shapes and writes them; [scheduling/README.md](scheduling/README.md),
"Fact Refresh", and [runtime-config.md](runtime-config.md) are the contract. No new event type
and no new member of a persisted record: a fact refresh is recorded as every policy dispatch, run
and reading is, under a reserved task identity.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| D41 | `config.parameters.scheduling`: `scheduling.fact_refresh.enabled`, `.min_interval_ms`, `.daily_limit`, `.failure_backoff_ms`, `.max_consecutive_failures` | new configuration rows | D2-S4 | D2-S4 | boolean, milliseconds, integer `0..=4294967295`, milliseconds, integer `0..=4294967295`; defaults `true`, `300000`, `96`, `600000`, `3` (`actingd.defaults.json`); settable, read at every policy cycle; `overdue_ms` of the §5 row is left to D2-S5, its reader (a settable key without a default has no value an instruction can replace yet) |
| D42 | `policy.dispatch` records of a fact refresh (intent, admitted, rejected, completed) and its `policy.execution_recorded` | new values of existing members (shape unchanged) | D2-S4 | D2-S4 | `task_id` `_fact_refresh` (a catalog task id never starts with `_`), `operation_id` `_fact_refresh`, the procedure binding digest of the Runtime-owned procedure `_fact_refresh` (operation `_fact_refresh`, no yield points); the reason chain carries one `fact_refresh_requested` per requested key, which is the run's requested key set; the admission's budget receipt counts the Runtime-owned task spec (task daily and window limits `4294967295`, task runtime limit `4294967295` ms, 60 000 ms reserved per run) and the instance's activity profile; a settled run writes the four settlement runtime facts `task._fact_refresh.*` as any settled dispatch does |
| D43 | a fact refresh run's `runtime.request` (`RunContainedTask`) | new value of an existing member (shape unchanged) | D2-S4 | D2-S4 | `package_path` the instance's kept recognition maintenance package, the request naming it by its path only (§1.55 P4-1) |

The codes `fact_refresh_requested` (info), `fact_refresh_pending` (info),
`fact_refresh_unresolved` (warning) and `fact_refresh_exhausted` (warning) (policy, ladder class
`none`), the key and vocabulary `refresh_limit` (`daily_limit`, `failure_pause`,
`schedulable_state`, `activity_window`, `placement`), and `fact_reader_binding_invalid` as a
fact refresh run's failure code are catalog entries, not rows (§7).

### 1.57 Wave 13 slice EXEC-1 (#390, forced-compromises mapping E-04, E-13)

EXEC-1 brings its own shapes and writes them; [resource-declarations.md](resource-declarations.md)
("Task schema `0.9` candidate operations") is the contract. One page of a page-graph task may
carry ordered candidate operations with a `when`, and a click may let a refusal of its guard fall
to the page's next candidate. No new event type, no new member of a persisted record and no
configuration key. The code `contained_task_operation_candidates_exhausted` (execution-kernel,
error, `pack_logic_mismatch`; values `page_id`, `count`) is a catalog entry, not a row (§7).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| EC1 | package format: task operation members `when` (`{"present": <target>}` or `{"absent": <target>}`) and `on_guard_refused` (`"next_candidate"`) (task schema `0.9`) | new members (resource file format, not a ledger shape) | EXEC-1 | resource side | `when` names a target judged on one frame (not `click_only`, not under target consensus), not on the entry application; `on_guard_refused` only on a click with a declared guard (Runtime admission, `contained_task_operation_invalid`); an older task schema refuses both (`UnconsumedField`), a `linear_steps` task refuses both (`contained_task_linear_invalid`); it is part of `package` revision 3, declared once for v0.12.0 (§6) |
| EC2 | `task.step_finished` of a candidate passed over after its guard refused | new rule of an existing record (shape unchanged) | EXEC-1 | EXEC-1 | written right after the step's `task.step_started` and its guard evaluation, with no `task.effect_intent` or `task.effect_completed` between, `page_label` the page of the frame the guard refused on and no phase; the step stays counted in the run's executed steps, and the next candidate starts the next step index on the same frame |
| EC3 | task diagnostic stream: the `target` and `error` rows' guard `phase` value `when` | new value of an existing member | EXEC-1 | EXEC-1 | the evaluation of a candidate's `when` target on the frame its step would start on, written before that step's `step_started` row (a withdrawn candidate starts no step) |

### 1.58 Wave 13 slice SEL-L3b (#308, forced compromises E-10, E-11, E-12; scan session of S0 v2.1 §2)

SEL-L3b widens how a scan moves: a list along either axis, a drag that ends with a pause and a
brake across the axis, and the page flip, aligned by its overlap or by page-order evidence. It
brings one input primitive, the drag with a pause and a brake, which a backend performs from the
same prepared segmented plan as the vertical-brake drag, through the same provider operation
(`segmented_swipe`) and error chain. It adds no code, location or key: a page-order viewport that
shows no single admitted page fails the existing `selection_scan_alignment_rejected` (its catalog
entry now names that case), and a broken action is refused with the existing
`invalid_input_coordinate`, `invalid_input_duration` and `invalid_segmented_swipe_contract`.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| L3B-1 | `InputAction::SingleTouchDragWithPauseAndBrakeV1 {x1, y1, x2, y2, drag_duration_ms, pause_ms, x3, y3, brake_duration_ms}` (`single_touch_drag_with_pause_and_brake_v1`) | new enum value (wire `Input` request; ledger: an input intent's `provenance.input_action`, a `task.effect_intent`'s `action`) | SEL-L3b | SEL-L3b (a scan drag that declares a pause or a brake; online from SEL-L4b, which lets a package declare `scan`) | one press at (`x1`, `y1`) dragged to (`x2`, `y2`) over `drag_duration_ms`, held there `pause_ms`, moved to (`x3`, `y3`) over `brake_duration_ms`, released; every point on the screen, `drag_duration_ms` 1..=60 000, the others at most 60 000; without a brake `brake_duration_ms` is 0 and (`x3`, `y3`) is (`x2`, `y2`), with one it moves; a pause or a brake is present (else it is a swipe); event action `input.swipe`; `Debug` redacted; its sampling evidence is the swipe's (the press in the first region, the drag's end in the second); the input intent records no `execution_plan` (the `maa_2_0` record is the vertical-brake drag's): the action in the provenance determines the plan; the foreground gate and the Nemu input take it as they take the vertical-brake drag; the adb shell backend refuses it as it refuses every segmented swipe |
| L3B-2 | `task.effect_intent` of a select step's scan move | existing record, written as row L3-1 states | SEL-L3b | SEL-L4b | a scan move is also a page-flip tap (`InputAction::Tap`, sampled in `rect`, or in `back_rect` for a corrective or a rewind) or the drag of row L3B-1; each is followed by its input and by no `task.effect_completed` |
| L3B-3 | package format: a select step's `scan.advance` and `scan.reposition` | new members (resource file format, not a ledger shape) | SEL-L3b (types); SEL-L4b (parse and admission) | resource side | `kind: drag` gains the optional `pause_ms` (at least 1) and `brake {cross_px, duration_ms}` (`cross_px` not 0, signed, across the axis; `duration_ms` at least 1); `kind: rect` is new: `advance {rect, back_rect}` with `min_content_step_px` and `max_content_step_px` (aligned by overlap) or with `page_order {checks: [check target ids, page 0 first], page_step_px}` (at least one check and 1 px; aligned by the one page check that passes, the same page or the next one), never both; `reposition {rect, back_rect, max_content_step_px}`, whose rewind taps `back_rect`; `axis` takes `vertical` or `horizontal` |

### 1.59 Wave 13 slice RD-5b (#398 model-398 v1.1 §7, §10.1, §12)

RD-5b brings its own shapes and writes them. The device state of a bound handset is read through
the device layer's closed command sets (RD-5a): by one sampler thread per Runtime, only while a
bound handset is configured and its link is `device`, and by the start gate before the first step
of every package run on it; the `device_state` section of `crates/runtime-config`
(`src/device_state.rs`) is its configuration and [runtime-config.md](runtime-config.md), "The
`device_state` section", that section's contract; [application-lifecycle.md](application-lifecycle.md),
"The handset start gate", is the gate's. No member carries a device serial: a handset is its
label `handset:<alias>`.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| RD5-1 | `runtime.lifecycle_observed` phase `device_state_observed` `{outcome}` | new lifecycle phase | RD-5b | RD-5b | under the handset's instance links: `device_thermal_hold` (warning, `thermal_status`) or `device_battery_low` (warning, `battery_percent`) when a sample finds the handset held; `device_hold_lifted` (info, `device_hold`) for each hold that lifts; `device_screen_woken` (info) when the wake key lit the screen; `device_state_unreadable` (error, `device_state_probe`, adb's text its `caused_by` link) for the first failing read of a sampling streak and for a start gate that could not read the screen-off timeout; each with `device_label` where its code declares it; normal commit sync |
| RD5-2 | `device.screen`, `device.keyguard`, `device.battery`, `device.thermal`, `device.screen_off_timeout_ms` (instance scope, a bound handset only) | new runtime facts | RD-5b | RD-5b | written by a sample, source `runtime`, each with the lifetime `device_state.fact_ttl_ms`: strings (`awake`/`asleep`/`dozing`/`dreaming`; `none`/`insecure`/`secure`; a `thermal_status` word), one `record_list` row `{level, plugged, status, temperature_deci_c}`, a duration; a changed value at once, an unchanged one when its record would expire before the next sample |
| RD5-3 | `session.instance.available` = `false` with `source_detector` `runtime.device-state`, `source_snapshot_id` `device_state:<hold>` | new source of an existing instance fact record | RD-5b | RD-5b | `locked`, `thermal`, `battery_low`, `in_use` (the first held, in that order); a device-link withdrawal comes first; with no hold left a passing self-check restores the seed, as a self-check withdrawal is restored |
| RD5-4 | `device_wake_failed`, `device_keyguard_dismiss_failed`, `device_locked`, `device_state_unreadable` (device codes, RD-5a), `device_thermal_hold`, `device_battery_low`, `device_in_use` on a run's failure | new top codes of an existing failure | RD-5b | RD-5b | the start gate's refusal before the run's first step (operation `device_state_gate`), or the keepalive's before an input (`device_state_keepalive`); projection `backend_operation_failed`, not fatal; no input reaches the device; a run whose handset left `device` still ends with `device_link_lost` (RD-2) |
| RD5-5 | a step of the run's lease for the screen check (`begin_destructive_step`, Business) | new use of an existing scheduler step | RD-5b | RD-5b | the wake key and the keyguard dismissal run under its witness (`Adb::run_write`); the step is finished right after the check; no ledger record of its own |
| RD5-6 | `config.parameters.device_state` (10 rows) | new section of an existing fact family | RD-5b | RD-5b | the keys of §5's `device_state.*` row in model-398 §7.8's order; every key settable |

Codes (runtime-host): `device_thermal_hold`, `device_battery_low`, `device_in_use` (warning,
`planned_stop`), `device_screen_woken`, `device_hold_lifted` (info, `none`),
`device_state_sampling_failed` (error, `none`; the sampler thread could not be spawned, not in
model-398 §10.1); keys `device_hold`, `thermal_status` and their vocabularies, `battery_percent`;
six `device_state_*` locations. `device_wake_failed`, `device_keyguard_dismiss_failed`,
`device_locked` and `device_state_unreadable` stay the device layer's (RD-5a).

### 1.60 Wave 13 slice S6c (#369 model v3.1 §7 row 8, H-5; ORD §3.5 item 17)

S6c reads S6a's per-instance `start_emulator` (§1.36 E6-1) and starts a configured, stopped
emulator at daemon start on an autostart claim; [emulator-control.md](emulator-control.md),
"Configured start at daemon start", is the contract. No event type, persisted member, wire
field or configuration key is added, and the claim, its start turn and its report lines live in
memory only, as every claim kind does (model §4.1). The thirteen `emulator_autostart_*` codes,
the locations `enqueue_emulator_autostart`, `run_emulator_autostart` and
`emulator_autostart_reports`, the context key `connectivity_reason` (S6a's vocabulary) and the
domain `emulator_autostart_` are catalog entries, not rows (§7).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| E6C-1 | actingd daemon lines on stderr: the autostart report lines, `emulator_autostart_queued` (`count`) once the claims are enqueued, before `runtime_ready`; per claim `…_network_ready` or `…_network_status_unknown`, then one of `…_started`, `…_preparation_failed`, `…_failed`, `…_network_unready`, `…_superseded`, `…_yielded`, `…_skipped`, `…_interrupted`, each with `alias`; one developer line and one `OUTCOME actingd:` line each (A6-1's writer) | process interface | S6c | S6c | none when no instance names `start_emulator` (the default, off); the lines of claims the host's close ended come before `runtime_stopped`; a line that cannot be written never stops actingd; #643's stdout lines `actingd emulator_autostart_* …` were never shipped |
| E6C-2 | an autostart claim's records under its own `AcquireLease` request (origin `(Agent, Adapter)`, the host's synthesized connection): `lease.requested` and `scheduler.admitted` + `lease.granted`, or `scheduler.queued` (deadline `u64::MAX`, priority `normal`) while a pause holds it; the renew set; `lease.released`; at its Start `command.received` (Runtime origin) and H-4's start records; a refusal before the Start: `command.received`, `command.rejected`, `runtime.failed` (`emulator_autostart_network_unready` with `alias`, `elapsed_ms`; or the `lease_expired` / `lease_missing` of a refused renewal) | new use of existing records (shapes unchanged) | S6c | S6c | a claim that yields to a pause, is superseded by an operator control, finds its instance running, or is interrupted by a drain or a shutdown writes its release only (and, for a pause, its new claim's `lease.requested` + `scheduler.queued`), no `command.*`; #411 A3 (v0.12.1): a gated claim does not refuse a shutdown, and an accepted shutdown disconnects it with `scheduler.denied` `lease.queue_disconnected` only (no report line, no `command.*`) |

### 1.61 Wave 13 slice P-m with MEM-1 (#385 PERF v1.4.2 §3, §4, §4.6, §10.3, §12; ADD §1.6)

P-m brings the shadow measurement ([performance-control.md](performance-control.md), "Shadow
measurement"): it writes `perf.pacing_changed` (P1) and the `pacing` members of `perf.summary`
(P4), and MEM-1 the run's package load in the task timing evidence (P8: the run record did not
carry it). It adds the read kind and the read output below; neither is a ledger shape. No action
takes effect: `performance.pacing.mode` stays `observe` (coordinator ruling 7 of wave 13).
Not written by this slice: the baselines' keys of §4 (`health.perf_baseline.*`,
`host.perf_baseline.*`) wait for AO-5 and AO-6 (PERF §3.5: until then in memory); the hourly
`host.perf_calibration.stats` and P5's per-step members (`page_settle_ms`, `pacing_band`,
`pressure_level`) are left to a later slice (see the PR).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| W15 | `RuntimeOperation::PerformanceStatus` → `RuntimeResult::PerformanceStatus { status }` | runtime-client 2 (§3) | P-m | P-m | a general read from memory that reads and records nothing in the ledger (U-1 row: general read, no G14 copy); `status` is a `RuntimePerformanceStatus {observed_at_unix_ms, pacing_mode, pacing_phase? (only under full), learning, monitor_health, host, instances (one per registered instance, at most 256), warnings (at most 64 warning outcomes: perf_probe_unavailable, perf_baseline_clamped), ledger_read_pages? (#422 W1-8, W20, since 0.12.25: {pages, total_ms, longest_ms, last_ms}, the view pages the ledger's reader pool read since the Runtime started; absent without a reader pool)}`; `host` = `{host_responsiveness_bp?, responsiveness_bp, pacing_stage, factor_milli, learning_view, learning_view_allowed (count), normal_view, normal_view_allowed (count), effective_stage, effective_factor_milli}`; an instance row = `{instance_alias, emulator_responsiveness_bp?, responsiveness_bp, pacing_stage, factor_milli, learning_view, learning_view_allowed (bool), normal_view, normal_view_allowed (bool), effective_stage, effective_factor_milli}`; a view = `{pacing_stage, share_milli, dwell_24h, dwell_total, stops_24h, stops_total}`, a dwell = `{s0_ms, s1_ms, s2_ms, s3_ms, s4_ms}`; a completed receipt carries `request_performance_status_completed`; a wrong shape is `invalid_performance_status` |
| W16 | `actingctl perf-status [--instance <alias\|instance_id\|ADB port>]` ↔ `ac_overview` block `performance` (`sections` gains `performance`) | read output of a general read (no persisted record; the CLI line carries it beside its outcome) | P-m | P-m | the W15 status, its `instances` narrowed to the selected one, with `ledger_read_pages {pages, total_ms, longest_ms, last_ms}` as it is when the status carries it (#422 W1-8, W20; absent without a reader pool); CLI success `runtime_performance_reported`; actingd absent: the client's failure, exit 1; in `ac_overview` a failed read leaves a warning and `incomplete`, never an empty block; the block's place in the cut order was registered by MCP-8 |
| PM1 | `perf.pacing_changed` (P1) | first writer | SH-1 | P-m | outcome `perf_stage_changed` (info): `pacing_stage_from`, `pacing_stage_to`, `responsiveness_bp`, `factor_milli`, `pacing_cause`, `pacing_mode`, optional `instance_id` (alias), `host_responsiveness_bp`, `emulator_responsiveness_bp`, `pacing_learning_capped`; one record per change of a scope's stage, none for a factor change within a stage (PERF §12.1) |
| PM2 | `perf.summary` `pacing` (P4) | first writer | SH-1 | P-m | the last evaluation's: `responsiveness_bp` the host's `R_eff`, `host_responsiveness_bp` `R_host`, `emulator_responsiveness_bp` the lowest instance `R_emu`, the host's curve stage, factor and normal share, `pacing_mode`, `learning` (under `full` while the learning view caps), both views with their `allowed`, `pacing_phase` only under `full`, `learn_days`/`learn_active_ms` absent (P-a), one row per instance (at most 64) |
| PM3 | task timing evidence `package_load_ms`, `package_load_bytes` (P8, MEM-1) | first writer | SH-1 | P-m | the run's main package: the time `prepare` took to read and admit it, and the bytes it holds in memory (the loaded bundle's resident bytes); a prerequisite or recovery package is not counted |

### 1.62 Wave 13 slice RD-1c (#398 model-398 v1.1 §3.3, §10.1, §12)

RD-1c writes the private binding file RD-1b reads (RB5) and adds no ledger shape: `actingctl
devices` runs no Runtime request and records nothing, and the next start records a binding as
RB1 says. [device-binding.md](device-binding.md), "Choosing a device", is the contract.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| D1C1 | `<state root>\private\device-bindings.json` (RB5) | first writer of an existing private file format, not a ledger shape | RD-1c | RD-1c (`actingctl devices bind\|unbind`, `ac_device_bind`, `ac_device_unbind`) | the file as RB5 defines it, `bound_via` `cli` or `mcp`; written whole through a temporary sibling after the file's own rules accept the new content (one alias and one serial each); never read by an export, archive or diagnostic (guard RD-1b-T10 names `apps/actingctl/src/devices.rs` as its writer) |
| D1C2 | `actingctl devices list` ↔ `ac_devices` rows `{handle?, transport, state, model?, bound_alias?, endpoint?}`; `devices bind` ↔ `ac_device_bind` `{alias, handle, model?, restart_required}`; `devices unbind` ↔ `ac_device_unbind` `{alias, unbound, restart_required}` | client read and instruction outputs (no persisted record, no runtime-client shape) | RD-1c | RD-1c | `handle` `<transport>:<transport_id>`, never a device value; no output names a serial |

Codes (catalog entries, not rows, §7): `device_list_reported`, `device_binding_changed`
(success), `device_handle_unknown`, `device_list_failed`, `device_kind_unsupported`,
`device_bindings_invalid`, `device_bindings_write_failed` (error), all actingctl, `ladder_class`
`none`; the key `handle` (token); actingctl joins the owners of the prefix `device_`.

### 1.63 Wave 13 slice DRILL-1 (#390, forced-compromises mapping T-08; coordinator ruling 9 of wave 13)

DRILL-1 lets a person drill one instance's stuck-recovery ladder; a drill is recorded like a ladder
a trigger started and tagged `drill`, and never acts on the device unless the request says so.
[emulator-control.md](emulator-control.md), "Drills", is the contract;
[runtime-interface.md](runtime-interface.md) has the request, CLI and MCP rows. Its codes
(`recovery_ladder_drill_requested`, `recovery_rung_skipped_drill_plan`,
`recovery_ladder_drill_disabled`, `recovery_ladder_drill_already_running`,
`recovery_ladder_drill_cooling_down`, `request_drill_recovery_ladder_completed`,
`recovery_ladder_drill_queued`) and its locations are catalog entries, not rows (§7). No
configuration key.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| DR1 | `runtime.lifecycle_observed` payload member `drill` (boolean) | new optional member | DRILL-1 | DRILL-1 | present (`true`) only on a drill's own ladder phases (`recovery_ladder_started`, `recovery_rung_finished`, `recovery_instance_stopped`, `recovery_environment_ready`, `recovery_ladder_finished`, `recovery_ladder_suppressed`), absent on every other record; the payload check refuses it on any other phase, and a `recovery_ladder_started` carries it exactly when its trigger's stage is `drill` |
| DR2 | `RecoveryTriggerStage::Drill`, spelled `drill`: `recovery_ladder_started.trigger.stage` | new enum value | DRILL-1 | DRILL-1 | a drill's trigger names no run, task, preparation, rule or match, `failure_code` `recovery_ladder_drill_requested`, `start` `immediately`; an `instance_preparation_finished` never names it; a `recovery_ladder.triggers[]` item that names it is refused `stage_not_allowed` |
| DR3 | `RecoveryRungSkipReason::DrillPlan`, spelled `drill_plan`: `recovery_ladder_started.rungs[].reason` and the skip outcome `recovery_rung_skipped_drill_plan` of `recovery_rung_finished` | new enum value | DRILL-1 | DRILL-1 | written by a plan drill for each rung the ladder would have run (the started plan lists it `pending`) |
| DR4 | `RecoveryLadderOutcome::Planned`, spelled `planned`: `recovery_ladder_finished.ladder_result` | new enum value | DRILL-1 | DRILL-1 | only on a drill's record (`drill: true`), with `rungs_tried` 0 and no `outcome`; info |
| DR5 | `RuntimeOperation::DrillRecoveryLadder { instance_alias, mode }` (`mode` `plan` or `device`) → `RuntimeResult::RecoveryLadderDrillQueued { drill: {instance_alias, mode, ladder_request_id} }` | wire shape, runtime-client 2 (request kind of the option-two block, U-1's classification table gains the row; G14 copies it) | DRILL-1 | DRILL-1 | the emulator control origin gate (User+Ui, Cli+Cli, else `invalid_emulator_control_origin`); a physical instance only; a completed receipt carries `request_drill_recovery_ladder_completed`; `ladder_request_id` is the request id of every record of the drill's claim and ladder |
| DR6 | `actingctl ladder-drill --instance <selector> [--device]` ↔ `ac_ladder_drill` (`instance`, `device`): `{instance_alias, mode, ladder_request_id}` | operation output (no persisted record; the CLI line carries it beside its outcome) | DRILL-1 | DRILL-1 | the selector is the alias, instance_id or ADB port, resolved with one status read; `--device` absent (`device` false) is a plan drill; CLI success `recovery_ladder_drill_queued`; MCP operator tier before U-3, marked destructive |

### 1.64 Wave 14 slice PKG-S5 (#336 option 丙, M-PKG v1.1 §3.5, §4.6; M-357a §7 TB12, §1.65)

PKG-S5 names every pack by its `package_id` where the tools lay packages out: the bundle index
v4 and the Lab's `record stop` output. It retires `actinglab package digest`, the package
reference type `PackageRef` (with its content-directory, legacy ZIP and source-tree forms and the
`sha256:`-prefixed serializers) and the authoring receipt's package hash. No record type and no
persisted member: the rows are file formats and output shapes. A bundle index v1, v2 or v3 is
not read (no old version is read, M-PKG §7). The content hash keeps its one entry,
containment's `measure_content_hash`, for the suspension rule only (§2, "Failure identity").
[package-reference.md](package-reference.md), "Content-directory tools and the bundle index",
and [lab-recording.md](lab-recording.md), "Writing", are the contract.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| P5-1 | bundle index v4 `actingcommand.bundle.v4` (`BundleIndex`): `{schema_version, game, source?, packs[] {package_id, server, entry_task_id, path, file_count, byte_count}, maintenance?, tables?}` | resource file format (the game root's `bundle.json`), replaces v2 and v3 | PKG-S5 | PKG-S5 (`package bundle`); resource side (PKG-S7) | `path` is exactly `packs/<package_id>`; `package_id` is one safe path segment, never `tables` in any ASCII case (`invalid_bundle_pack_path`), unique (`duplicate_bundle_pack`); no member carries a digest; `maintenance` (the bundle-level maintenance declarations, by `package_id`) is present when `package bundle --maintenance` gave it; a v2 or v3 `bundle.json` is `standard_pack_root_invalid` `malformed`; check-config's `standard_packs.games[].bundle_version` is 4 |
| P5-2 | bundle v4 `tables` (M-357a §7 TB12) | optional member of P5-1 | PKG-S5 | resource side (PKG-S7 bundle script) | at most 64 unique table ids of `[a-z][a-z0-9_]{0,63}` (`invalid_bundle_index`); read and checked, otherwise ignored by the Runtime; the updater (#364 installer, PKG-S6 acsetup) replaces only the listed tables and leaves the root's other tables alone, tables before package directories; `package bundle` writes none; §1.65 TB12 is the shape's register row, this is its code |
| P5-3 | `package bundle` layout `packs/<package_id>/` | tool output layout | PKG-S5 | PKG-S5 | named by the `package_id` the pack's `control.json` declares; the copy is admitted in full by its path, no digest computed; two packs of one `package_id`, or one that cannot name its directory, are `package_bundle_index_invalid` |
| P5-4 | `record stop` output `<package_id>.<zip\|json>` (in the recording's `lab/out/` and in `--lab-dir`), replaced in place (M-PKG §3.5, D9) | tool output layout | PKG-S5 | PKG-S5 | written as `<name>.part`, an existing file set aside as `<name>.old-<unix_ms>`, the `.part` renamed into place, the old one deleted; a failed step is `record_state_io_failed` naming the `.part` file and any file left (`restored`, `left_files`); `record_artifact_name_conflict` is no longer written; a `--package-id` that is not one path segment is `validation_failed` |
| P5-5 | `record stop` answer `lab {…, lab_dir_status: "written" \| "replaced" \| "to_write" \| "to_replace", …}` and its binding examples `prerequisite_entry_example {package_id, package_path}`, `binding_example {procedure_ref, operation_id, yield_points, scheduled_execution}`, `task_run_example` (`--package` only) | changed output members (removed `digest`, `sha256`, `package_ref`, `package_digest`; `present` gone) | PKG-S5 | PKG-S5 | MCP `ac_record_stop` keeps the binding parts inline without `package_ref`; the round trip compares the expanded table with the generated one entry by entry |
| P5-6 | Lab recording state `RecordingArtifact {container, path, lab_dir_path?, byte_count, package_id, …}` (`actingcommand.lab-recording.v1`) | removed members `digest`, `sha256` (Lab state file, not the ledger) | PKG-S5 | PKG-S5 | a stopped recording given `--lab-dir` again reads its copy and replaces the `--lab-dir` file the same way |
| P5-7 | `AuthoringValidationReport {checks}` | removed member `package_sha256` | PKG-S5 | PKG-S5 | the code `authoring_validation_package_hash_invalid` is deleted (pending) |
| P5-8 | `actinglab package digest`; `PackageRef`, `ContentDirectory`, `ContentDirectoryVersion`, `GitSourceTree`, `GitOid`, `prefixed_reference`, `optional_prefixed_reference`; `describe_path`, the loaders' `recorded` parameter, `TaskPackageDescriptor.reference` | removed command and types | PKG-S5 | PKG-S5 | `package digest` is an unknown package command (`validation_failed`); the codes `invalid_git_object_id`, `invalid_package_repository`, `invalid_source_tree_reference`, `invalid_content_directory_reference`, `invalid_package_sha256` and `source_tree_loader_retired` are deleted (pending); guards `pkg_s5_no_package_reference_is_left` and `pkg_s5_the_content_hash_has_one_entry_and_no_admission_path_reaches_it` (S5-T5) |

### 1.65 DB-M: shared tables and the `table` value (#357, M-357a v1.1)

M-357a adds no ledger shape and no wire shape: a table read is recorded through the existing members of `task.selection_evaluated` (C357 constraint, for M-357a: no new ledger type, field or enum value; M-357b changes the selection record for multi-select, Alice 2026-10-15, §1.66). The rows below are resource file formats, a bundle index member, interface members and confirmations that a persisted shape stays as it is. They land before the v0.12.0 tag so that DB-1 and DB-2 (0.12.x) add nothing a v0.12.0 reader must know. New codes, keys and vocabulary tokens are catalog entries, not rows.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| TB1 | game-root file `tables/<table_id>.json` (`actingcommand.table.v1`) | resource file format, not a ledger shape | DB-1 (reader; contract checks `parse_standard_pack_table`; canonical-JSON reader moves into the contract crate) | resource side (DB-4) | `{schema_version, game, table_id, row_key [1..=8], columns [2..=32] {name, type: key \| integer {min, max} \| boolean \| enum_string {allowed}, nullable?, valid_until_unix_s? (nullable columns only)}, rows [1..=8192] [cells]}`; one `key` column = `row_key[0]`; rows unique on `row_key`; canonical JSON; ≤2 MiB; ≤64 tables per game root (beyond 64, by file name, each is `table_too_large`); `tables` is a reserved directory of the game root; `tables/` without `bundle.json` is `bundle_missing`; in a resource repository, `tables/*.json` at the repository root only; [package-reference.md](package-reference.md), "Standard package root" |
| TB2 | `runtime.lifecycle_observed` phase `standard_pack_read` (N4): one more record per refused table at startup | existing record and phase, new outcome codes `standard_pack_table_invalid`, `standard_pack_table_unreadable` | DB-1 | DB-1 | REG §1.12 N4 "At startup one per game root" becomes "one per game root, plus one per refused table"; the root's own record is unchanged; a refused table never fails the root or the start; tables of a broken root are not read |
| TB3 | check-config v1 report: `standard_packs.games[].tables` `[{table_id, path, row_count?, problem?, expired_columns?}]`; `warnings` entries `standard_pack_table_invalid:<game>`, `standard_pack_table_column_expired:<game>` | interface member | DB-1 | DB-1 | schema stays `v1` (additive, as G8); present for every sound game root, empty when it has no `tables/`; not listed for a broken root |
| TB4 | `actingctl packs` ↔ `ac_packs`: top-level `standard_packs.games[].tables`, same shape as TB3 | read output (no persisted record, no Runtime request) | DB-1 | DB-1 | per game, not per instance (`actingctl packs` resolves no game for an instance); `result_schema` updated (additive); fills A6's "a shared-table path (DB-1) … come later" |
| TB5 | `actinglab resource validate` family `standard_pack_tables` | tool family | DB-1 (files); DB-2 (bind cross-check, identity coverage) | — | files: TB1 checks; cross-check: every package policy of the root that declares `tables` is bound with `SelectionTables::bind` (K2); coverage: identities of the layout's catalog with no row are a warning, an error for a document that reads the table under `abort_evaluation`; per-document rules and K3 stay in the `selection_policy` family; the coverage code uses the tool's own domain prefix |
| TB6 | selection-policy document (`actingcommand.selection-policy.v1`): optional `tables [≤8] {table_id, path, row_key, columns}`; `ValueRef::Table` (`source: "table"`) `{table_id, key_field, pins [≤7] {column, equals: ScalarValue \| equals_fact, otherwise? {equals \| equals_fact}}, value_column, fallback_column?, absent?}` | pack format (package interface), additive | DB-2 | resource side | `schema_version` unchanged; a document without `tables` keeps its canonical bytes, `policy_sha256` and `input_sha256`; `table_id` equals the file's id and `path` is `tables/<table_id>.json`; `key_field` is the layout's identity field; readers of `table` declare `drop_candidate` or `abort_evaluation`; `absent` in gates only; `fallback_column` in terms only; a pinned read needs an `absent` gate on the same table, key and pins first; a tie-break may read a table only where a drop or abort gate or term reads the same table, key and pins; a v0.12.0 build refuses such a document (`contained_task_select_policy_invalid`) |
| TB7 | `ContainedTaskRuntime::standard_pack_table` (execution kernel) | new trait method, default "unavailable" | DB-2 | — | not persisted; answers from the tables the host bound in the run's preparation (K1/K2); reads no file; offline runtimes answer from the dry run's root |
| TB8 | `task.selection_evaluated` | **unchanged by M-357a** | — | DB-2 | table reads use `gates[]`, `terms[]` (`unknown_dropped {reason: lookup_miss \| type_mismatch}`), `score_milli`, `rank`, `reasons[]` (`term_fallback_used`), `unknown_check`; a static table drop in a scan is a `candidates[]` entry with an `unknown_dropped` verdict, which the existing reader accepts (`evaluates` and path rules 3–4 unchanged); M-357a adds no member and no variant. **Superseded for multi-select:** M-357b (Alice 2026-10-15, #357/6182436190) adds the optional member `multi_select` and six confirmation variants to this record and amends path rules 1, 4 and 5 for it (§1.66 MS1–MS5, slice DB-3s, merged with or after this section and before the v0.12.0 tag); a record without `multi_select` is unchanged |
| TB9 | `TaskSelectionUnknownReason` / `UnknownReason` | **unchanged** | — | — | a key with no row at all, or a null cell, is `lookup_miss`; an enum cell outside the declared `allowed` is `type_mismatch`; a key with rows but none matching the pins reads the declared `absent` (gates only); a pinned fact keeps its own reason |
| TB10 | scheduling consumer: catalog compile refuses a document that declares `tables` | behaviour, no shape | DB-2 | — | `type_mismatch` (`"tables=declared consumer=scheduling"`), as for record lists; 367-D1 lifts it when it wires tables (§5) |
| TB11 | instance and workbench fact stores | **no key** | — | — | tables are never published as facts; REG §4 gains no row; the capacity ledger books nothing |
| TB12 | bundle index v4 (PKG-S5): optional `tables [≤64] <table_id>` | bundle index member, additive to v4 | PKG-S5 (before A8) | resource side (PKG-S7 bundle script) | v0.12.0 readers parse and check it (grammar, unique, count) and otherwise ignore it; the updater (#364 installer, PKG-S6 acsetup) replaces only the listed tables and leaves other tables of the root alone, tables before package directories; bundle v4 also refuses a pack whose `package_id` is `tables` (`bundle_invalid`); never added to bundle v2/v3 (`deny_unknown_fields`) |

### 1.66 M-357b: multi-select in one step (#357, M-357b v1.0)

A select step whose policy declares more than one candidate chooses up to N of them in one attempt and records them in one `task.selection_evaluated` with one confirmation per pick (Alice 2026-10-15, #357/6182436190). DB-3s writes the types, the readers, the validation and the tests of MS1–MS5, never a writer; they are part of revision 3 because they merge before the v0.12.0 tag, which waits for them. The package-format and behaviour rows MS6–MS10 may land before or after the tag; their code is 0.12.x (DB-3a1, DB-3a2, DB-3b), where `package` is bumped once. M-357a's TB8 ("unchanged") does not cover these rows (§1.65 TB8). New codes, keys and vocabularies are catalog entries, not rows.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| MS1 | `task.selection_evaluated` `multi_select` | new optional member | DB-3s | DB-3b (single frame), DB-3c (scan) | `{declared_count (2..=16), required_count (1..=declared_count), free_slots? (1..=65535), order (rank, content), mark_feature, pick_set_sha256? (v1 projection only), picks [≤16] {candidate_id, confirmation, tap_check?}}`; present exactly when the step's policy declares more than one candidate; absent on every single select, whose bytes are unchanged; `required_count` = min(`declared_count`, `free_slots`) when `free_slots` is present, else `declared_count`; `selected` keeps the evaluator's rank order and `picks` carries the tap order; `pick_set_sha256` is checked for presence and grammar only (its definition is writer-side); `deny_unknown_fields` throughout |
| MS2 | `TaskSelectionConfirmation` `pick_matched {frame_id}`, `pick_moved {frame_id, pick_set_sha256}`, `mark_unconfirmed {candidate_id, mark_expectation (marked, unmarked), mark_state (marked, unmarked, unknown), frame_id}`, `tap_marked {frame_id}`, `picks_confirmed {count}`, `picks_stopped {count}`; `TaskSelectionRepositionStage` `tap_check` | new variants and value | DB-3s | DB-3b, DB-3c | `pick_matched`, `mark_unconfirmed` and `tap_marked` appear only inside `multi_select.picks`; `pick_moved` is a v1 pick's confirmation or tap check; `picks_confirmed` and `picks_stopped` only as the record's top-level `confirmation`; `reposition_failed {tap_check}` only as a v2 tap check; `mark_unconfirmed` with `mark_expectation` equal to `mark_state` is refused; a record without `multi_select` never carries any of them |
| MS3 | `task.selection_evaluated` validation | rules M1–M8 (M-357b §5.2) | DB-3s | — | refused as `invalid_task_selection_record` with the fields `multi_select`, `picks`, `confirmation`, `path_rule_1`, `path_rule_4`, `path_rule_5`; with `multi_select`, the top-level `confirmation` is `not_attempted` (no picks), `picks_confirmed {count}`, a verbatim copy of the first failing pick confirmation or tap check, or `picks_stopped {count}` when the sequence stopped outside the record (deadline, task timeout, input failure, run end) after `count` picks were tapped and checked; "a record with `unknown_check` chooses exactly one candidate" (SL4) holds only without `multi_select`; path rule 4 also blocks a partial `selected` (count below `required_count`) with an unknown instance; path rule 5 applies only to a full `selected`, with the lowest selected score; an early-stopped scan has a full `selected` and no `selection_quota_unfilled` |
| MS4 | public projection of `task.selection_evaluated` | read surface | DB-3s | — | every outcome inside `multi_select.picks` (confirmation and tap check) is codes only, as the top-level confirmation's (E9); `TaskSemanticFact::outcome()` keeps reading the top-level confirmation, which carries a multi-select's failure verbatim when it has an `outcome` |
| MS5 | `task.effect_intent` of a multi-select step: one tap per pick, linked to its pick's confirmation frame, before the step's one `task.selection_evaluated`; one `task.effect_completed` after the record | existing records, new order within a step | DB-3s (reader test) | DB-3b | extends L3-1 and L3B-2 (several intents per step action); pick k's tap intent is the intent of the step action linked to `picks[k-1].confirmation.frame_id` (scan moves never link to a confirmation frame); the record's envelope frame is the last tap-check or failing frame; no reader of revision 3 requires the selection record to precede the step's first intent (pinned by a DB-3s ledger test); ledger-forensics pairing of several intents per step is SEL-L4b's fix |
| MS6 | task file: select step `select.picks {order, mark {feature}, ignore_features [0..=5], settle {interval_ms, timeout_ms}}` | new member (resource file format, task schema `0.9` or DB-3b's) | DB-3b | resource side | required exactly when the policy declares more than one candidate; the mark is a `passed` feature without consensus; `ignore_features` single-frame only, never identity, the mark or a policy-read feature; a multi-select layout allows single-frame jitter consensus only; `expect_identity.choice_of` never names a multi-select step; a scan multi-select's last tie-break is `candidate_id lowest_first`; `on_empty` only when `empty` is reachable; a v0.12.0 build refuses it (`UnknownField` at `…/select/picks`) |
| MS7 | selection-policy document: `combination {quotas [≤8] {constraint_id, value, on_unknown, bounds [1..=32] {equals, min, max}, unlisted (unbounded, excluded)}, exclusive [≤8] {constraint_id, value, on_unknown, except [≤32]}, budgets [≤4] {constraint_id, value, on_unknown, max {equals \| equals_fact}}}`; `selection.min_count`; `selection.free_slots` (`ValueRef::Fact`) | pack format (package interface), additive | DB-3a2 | resource side | `schema_version` unchanged; a document without them keeps its canonical bytes, `policy_sha256` and `input_sha256`; `on_unknown` is `{"kind": "drop_candidate"}` or `{"kind": "abort_evaluation"}`, and only the latter when the value is a `keyed_fact`; constraint values never carry `absent`, are read as gates named by `constraint_id` and recorded in `gates[]`; a budget's `equals_fact` and `free_slots` are resolved in the up-front check; the multi-select policy's last tie-break key is `candidate_id`; `min_count` only with `none_allowed`; `free_slots` excludes `min_count` and any quota minimum; a v0.12.0 build refuses it (`contained_task_select_policy_invalid`) |
| MS8 | `ValueRef::Table` `aggregate {free_column, take (highest, max_value), at_most [≤4] {column, value, null_passes?}}` | pack format, additive to TB6 | DB-3a1 | resource side | the pins cover every row-key column but the key and `free_column`; `free_column` compares by integer or by the document's `allowed` order; no row left after the filter reads as the declared `absent` (gates); a `field` or `fact` filter value that is Unknown makes the read Unknown with that reason; a `keyed_fact` filter value follows S0 §4.2 (list checked up front, `absent` required); the class gate of an aggregated read carries the same `aggregate` |
| MS9 | scheduling consumer: catalog compile refuses a document that declares `combination` or `equals_ref` | behaviour, no shape | DB-3a2 | — | `type_mismatch` (`"combination=declared consumer=scheduling"`, `"equals_ref=declared consumer=scheduling"`), as for record lists; 367-D1/D2 decide multi-dispatch |
| MS10 | TB6 pin: `equals_ref` (a `field` or an unpinned single-key `table` read) beside `equals` and `equals_fact` | pack format, additive to TB6 | DB-3a2 | resource side | the pin takes the candidate's own value; no `otherwise`; only in a policy that declares more than one candidate; every distinct `equals_ref` needs one quota whose `value` is the same reference, with `min` = `max` on every bound, `unlisted: excluded` and the minimums summing to the declared count; the rows read through it enter `input_sha256`; the per-identity bound stays exact |

### 1.67 Wave 14 slice NAV-3a (#373, M-373 v1.1 §4.2, §4.3, §8.3, §9.2)

NAV-3a lets a page-graph package declare a prerequisite package and gives it the return-home
fallback, and finds every prerequisite and return-home package in the standard-pack registry of
its game (`<standard_packs.root>/<game>/bundle.json`, read when a run uses it) instead of the
actingd configuration. [linear-steps.md](linear-steps.md), "Prerequisite packages" and
"Return-home fallback", and [package-reference.md](package-reference.md), "Standard package
root", are the contract. The vocabulary `registry_problem` and its key are catalog entries
(§7); the deleted codes are listed in N3A-3.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| N3A-1 | `TaskEntryRecoveryRole` values `prerequisite` and `return_home`: the `role` of `EntryRecoveryPackageAdmitted`, `EntryRecoveryCompleted`, `EntryRecoveryFailed` (`task.entry_preflight`) | new enum values of §1.12 N1's member | NAV-3a | NAV-3a | `prerequisite` on a declared layer of the prerequisite gate, `return_home` on its return-home layer and on the registry's return-home package of a page-graph home entry; a request's own recovery binding still carries none (M-373 §8.3). A replayed request takes a layer with a role as a registry package of the request's package's game |
| N3A-2 | `contained_task_prerequisite_unbound` value `registry_problem` (`root_unset`, `game_root_missing`, `not_listed`) | new outcome value of an existing code | NAV-3a | NAV-3a | on the preparation refusal of a declared prerequisite package the registry does not bind (M-373 §4.3, §9.2) |
| N3A-3 | actingd configuration `prerequisite_packages[] {package_id, package_path}` and `return_home_packages[] {game, server, package_id}` | removed configuration keys | NAV-3a | — | a configuration that carries either is `config_decode_failed` at `config_load`; no reader of the old form. Deleted (pending) with them: `prerequisite_packages_size_invalid`, `prerequisite_package_id_invalid`, `prerequisite_package_unavailable`, `prerequisite_package_not_regular`, `prerequisite_package_container_unsupported`, `prerequisite_package_request_invalid`, `prerequisite_package_duplicate`, `return_home_packages_size_invalid`, `return_home_package_key_invalid`, `return_home_package_duplicate` (actingd) and `return_home_package_unbound` (contract; `prerequisite_package_digest_invalid` left with PKG-S4), and the location `resolve_return_home_package` |
| N3A-4 | the registry of a game root: package id → directory and server → return-home package (`bundle.json` v4 `maintenance[]` use `return_home`) | rule | NAV-3a | NAV-3a | `standard_pack_registered` (§1.12 N4) is written when either part changed since the last read; every listed package can be a prerequisite package; bundle v4 refuses a `maintenance[]` entry with the retired use `prerequisite` (M-373 §3.8, test row N3a-9: the NAV-3a follow-up, §1.73 N3F-1; PKG-S5's v4 still accepted it), so a game root whose `bundle.json` holds one is `standard_pack_root_invalid` `malformed` and registers nothing |
| N3A-5 | `navigable_route` control `prerequisite_package_id` | package declaration rule | NAV-3a | — | admitted for `navigable_route` (phased or not); `in_page_guard` and `recognize_only` are `contained_task_control_invalid` (`reason=prerequisite_execution_mode`, the retired `reason=prerequisite_requires_linear_steps` no longer appears); a `navigable_route` package declaring one needs an entry page (`reason=prerequisite_requires_page_entry`), which is then always checked first |
| N3A-6 | `actingctl packs` and `ac_packs`: roles `prerequisite` and `return_home` | removed output rows and argument values (client only) | NAV-3a | NAV-3a | they named the retired keys; the roles left are `procedure`, `startup`, `resource_package`; the warning `return_home_package_unbound` is gone with its code |

### 1.68 Wave 14 slice D2-S5 (#384 D2M §5.1 MP-3, §5.3 A-5, §6 S5, §7; v1.4.2)

D2-S5 brings the fact refresh's transition recorder and its read;
[scheduling/README.md](scheduling/README.md) ("Fact Refresh"),
[runtime-interface.md](runtime-interface.md) and [runtime-config.md](runtime-config.md) are the
contract. It writes no ledger record: the transitions stay in memory until AO-4's G5 gate copies
them as D1 (§1.2), whose shape is unchanged.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| D51 | `RuntimeOperation::FactRefreshStatus` → `RuntimeResult::FactRefreshStatus { status }` (W17) | runtime-client 2 (§3) | D2-S5 | D2-S5 | a general read from memory that reads and records nothing in the ledger (U-1 row: general read, no G14 copy); `status` is a `RuntimeFactRefreshStatus {cycle_seq, transition_seq, instances}`: the numbers of the latest cycle the recorder observed and of its latest transition in this owner epoch (0 before the first), and one row `{instance_alias, demands}` per instance with a waiting key (at most 256), each demand `{fact_key, outcome, package_id?, since_unix_ms, overdue}` (at most 256 per row): the latest cycle's reason (`fact_refresh_pending`, `_unresolved`, `_exhausted` or `_unbound` with its values), the recognition maintenance package that serves the key, when the wait began and whether `fact_reader_overdue` was reported for it; a completed receipt carries `request_fact_refresh_status_completed`; a wrong shape is `invalid_fact_refresh_status`; a reply past one IPC frame is refused `runtime_fact_refresh_status_too_large_for_frame` |
| D52 | `actingctl fact-refresh [--instance <alias\|instance_id\|ADB port>]` ↔ `ac_overview` block `fact_refresh` (`sections` gains `fact_refresh`) (W18) | read output of a general read (no persisted record; the CLI line carries it beside its outcome) | D2-S5 | D2-S5 | the D51 status, its `instances` narrowed to the selected one; CLI success `runtime_fact_refresh_reported`; actingd absent: the client's failure, exit 1; in `ac_overview` a failed read leaves a warning and `incomplete`, never an empty block; the block's place in the cut order (first) was registered by MCP-8 |
| D53 | `config.parameters.scheduling`: `scheduling.fact_refresh.overdue_ms` | new configuration row | D2-S5 | D2-S5 | milliseconds, no default (§5: unset = not reported), so the row exists only when the configuration file names the key; settable, and an instruction can change it only then (`config_key_unknown` otherwise); read at every policy cycle's yield point |
| D54 | actingd daemon lines on stderr: per fact refresh transition the developer line `<INFO\|WARN> actingd: <code> <values> cycle_seq=<n> transition_seq=<n>` and `OUTCOME actingd: <envelope>` (full) in one write (A6-1's writer) | process interface | D2-S5 | D2-S5 | the transitions of each evaluated cycle, in (instance, key) order; none for a deferred cycle; a line that cannot be written never stops the driver; a transition is an info or a warning, so it never becomes a terminal outcome the watchdog acts on (A6-9) |

The codes `fact_refresh_resolved` (policy, info) and `fact_reader_overdue` (runtime-host,
warning), both ladder class `none` and only transitions, never decision reasons; the read's
`request_fact_refresh_status_completed`, `invalid_fact_refresh_status` (contract) and
`runtime_fact_refresh_status_too_large_for_frame` (runtime-host); `runtime_fact_refresh_reported`
(actingctl); and the locations `read_fact_refresh_status`, `record_fact_refresh_transitions`
(runtime-host) and `runtime_fact_refresh_status` (runtime-client) are catalog entries, not rows
(§7).

### 1.69 Wave 14 slice SR-3 (#386 SCHED v1.4.2 §2.4, §10, §11, §12; M-PKG v1.1 §3.3, §3.4, §5.2; coordinator ruling 4 of wave 14)

SR-3 suspends a task package that admission refuses with a code of
`scheduling.pack_admission.suspend_codes`, by its `package_id` under the Runtime build, with the
content hash PKG-S3's `measure_package_hash` measures right after the refusal (the S3-T6 guard
lists the suspension point). The suspension is memory of the daemon process; no record type
and no persisted member is new. [policy-suspension.md](policy-suspension.md), "Task package
suspension", is the contract. The code `runtime_pack_admission_spawn_failed` (runtime-host,
error, ladder class `none`), the locations `check_pack_admission` and
`join_runtime_pack_admission`, and `actual_digest` and `runtime_commit` of
`pack_admission_suspended` and `pack_admission_lifted` turning optional (pending entries: absent
for a package that could not be measured, M-PKG D18, and for a build without a known commit)
are catalog entries, not rows (§7).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| SR3-1 | `policy.planning_signal_observed` of kind `goal_missed` whose outcome is `pack_admission_suspended` or `pack_admission_lifted` | new values of existing members (shape unchanged) | SR-3 | SR-3 | the evaluation's copy (the G5 choke point, no direct write): a suspension once per instance it defers a pair of (`task_id` the pair's task), a lift once per instance the suspension was copied for (no `task_id`); `signal_id` `signal:pack_admission:<suspended\|lifted>:<hex>`, `<hex>` the SHA-256 of the package, the instance, the evaluation time and the process's count of copies, so each copy has its own; values `package_id`, `actual_digest` (`sha256:<content hash>`), `failure_code` (a suspension only), `runtime_commit`; selected by kind and prefix, never by outcome, like the clock-slot records |
| SR3-2 | `actingd suspended` (`actingcommand.actingd.suspended.v2`) top-level `packs[{package_id, failure_code, runtime_commit, paused_hash_prefix, current_hash_prefix, instances}]` | new report member (not persisted; `schema_version` unchanged) | SR-3 | SR-3 | read from the SR3-1 copies since the latest daemon start, a lift removing its instance; `current_hash_prefix` measured read-only at the location the copied task's procedure binds |
| SR3-3 | `actingctl diagnose` and `ac_diagnose` member `packs` (with `packs_truncated` when cut) | new output member (A5's shapes kept; parity with the CLI unchanged) | SR-3 | SR-3 | the SR3-2 rows whose `instances` name the diagnosed instance, every row without one, verbatim; cut after `suspended`, before `report_warnings` |
| SR3-4 | `config.parameters.scheduling`: `scheduling.pack_admission.check_in_check_config`, `.check_after_start`, `.suspend_codes` | new configuration rows (keys of the existing `scheduling` section, no new section) | SR-3 | SR-3 | boolean, boolean, list of codes the catalog lists and has not retired; defaults `true`, `true`, `["recognition_sample_budget_invalid"]` (`actingd.defaults.json`); settable, read where they are used (check-config, the background admission after start, each refusal of a scheduled run's preparation) |

### 1.70 Wave 14 slice EXEC-3 (#390, forced-compromises mapping E-02; coordinator ruling 5 of wave 14)

EXEC-3 lets a page-graph task use what R22 and R24 gave `linear_steps` tasks.
[policy-suspension.md](policy-suspension.md) ("Immediate rerun", "Failure identity") and
[application-lifecycle.md](application-lifecycle.md) ("In a run the host starts itself") are the
contract. It adds no event type, no member, no enum value, no code and no configuration key: two
existing records get new rules.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| X31 | `policy.execution_recorded` `failure.failure_identity` of a scheduled page-graph run | new rule of an existing member (shape unchanged) | EXEC-3 | EXEC-3 | the live settlement writes it for every run whose main package was admitted, `linear_steps` or page graph (format v2 unchanged); a page-graph run without a prerequisite chain that checked its required entry (an `EntryRecoveryDecision`) takes that check as its gate and its entry recovery package as its layer for the scope (`gate`, `pre:<I>`, `main`); its restart segment ends on the literal `home`; R25-2 stays a `linear_steps` rule; the startup reconciliation still writes none. So a page-graph failure scheduled for a retry is rerun at once (`failure_retry_immediate`) and its pause is lifted by its package's content hash. Wave 14 ruling 4 (fix round): `contained_task_step_unconfirmed` in the `main` scope, outside the restart segment, joins R21's failures that pause by repeating, as the graph counterpart of `contained_task_linear_intermediate_unobserved` (a coordinator reading of R21, reported to Alice) |
| X32 | the order of a host package run's first records: `task.step_started` (from `<unrecognized>`), `application.intent`, `application.completed`, `task.effect_completed`, before the run's first `capture.completed` | new rule of existing records (shapes unchanged) | EXEC-3 | EXEC-3 | only in a startup package's and a recovery ladder rung's run, and only for a page-graph package whose `entry_page` is `any` and whose first operation is a `from: "any"` application effect (R24); every other run keeps the #316-B3 order (its initial frame first) |

### 1.71 Wave 14 slice SEL-L6b (#308, forced-compromises mapping row SEL-L6b, T-01)

SEL-L6b replays one select step's scan over a saved frame sequence through the kernel's own scan
path (`dry_run_scan`, SEL-L3), with the facts SEL-L6a's whole-package replay gives a select
step. It writes no ledger record, wire shape or configuration key; its only new shape is an
actinglab operation output (L6B-1). [selection-graph.md](selection-graph.md), "Offline", is the
contract. The actinglab codes `replay_scan_refused`, `replay_scan_unreadable` and
`replay_scan_invalid` (error, ladder class `none`) are catalog entries, not rows (§7); the
kernel's existing `selection_scan_replay_frame_missing`, `selection_state_unavailable` and
`selection_fact_context_mismatch` are reused.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| L6B-1 | `actinglab replay --package <path> --frames <directory> --select <operation> --scan <file> [--facts <file>]`: `{mode: "offline_scan_replay", executed: false, package_id, task_id, operation_id, frames_given, facts, ocr, production_global_ledger_written: false, end: {status, coverage?, outcome?}, scan?}` | operation output (no persisted record; an actinglab result, read by a person or a CI job) | SEL-L6b | SEL-L6b | `end.status` `covered` (coverage `complete` or `early_stopped`, exit 0), `aborted` (the evaluator's decision ended the scan with coverage `not_started` or `incomplete`) or `failed` (`end.outcome`, the full outcome envelope); `scan` is the kernel's `ScanDryRun` (coverage, start evidence, extent, coverage viewports, rewind viewports, positions, members, unknown and gated-out positions, decision), absent when the scan failed; every end other than `covered` is `replay_scan_refused` with the output in the details. `--scan` is the step's scan declaration (S0 §2.1), the shape SEL-L4b admits from a package (§1.58 L3B-3), taken here as given |

### 1.72 Wave 14 slice RSU-1 (#331, additions-v012 §1.5b row RSU-1; forced-compromises mapping T-01)

RSU-1 is an actinglab offline command, `recognition-regression`. It replays the frames a run's
ledger kept, named by MCP-6's per-step frame references, with the recognition of a new package
or version, and compares each frame with its original record. It reads the state root
read-only and writes no ledger record, wire shape or configuration key; its only new shape is
its operation output (RS1-1). It is not wired into any CI. [recognition-regression.md](recognition-regression.md)
is the contract. The actinglab codes `recognition_regression_evidence_unreadable`,
`recognition_regression_evidence_invalid`, `recognition_regression_ledger_unreadable`,
`recognition_regression_frame_unknown` and `recognition_regression_frame_invalid` (error, ladder
class `none`) are catalog entries, not rows (§7).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| RS1-1 | `actinglab recognition-regression --state-root <root> --evidence <file>... (--package <path> \| --zip <path>)`: `{mode: "offline_recognition_regression", executed: false, request_id, package_id, frames: [{ref, steps, frame_id, status, targets?: [{target, original_matched, status, matched?, agrees?, score?, failure?}]}], targets: [{target, frames, compared, absent, failed, original_hits, hits, lost, gained, disagreements, error_rate_milli}], summary: {records, compared, absent, failed, original_hits, hits, lost, gained, disagreements, error_rate_milli, frames_referenced, frames_replayed, frames_material_missing, frames_without_original_record}, ocr, production_global_ledger_written: false}` | operation output (no persisted record; an actinglab result, read by a person or an agent) | RSU-1 | RSU-1 | a frame's `status` is `compared`, `material_missing` (#375 retention removed it) or `no_original_record`; a target's `status` is `compared`, `absent` (not in the package) or `failed`; `score` is `{kind, score_milli, threshold_milli}` (template), `{kind, distance_milli, max_distance_milli}` (colour), `{kind, confidence_milli}` (OCR) or `{kind, score_milli}` (classifier); `error_rate_milli` is the disagreements (`lost` + `gained`) per mille of the frames compared, rounded down, `null` when none was compared; the original record is the run's last `task.recognition_completed` fact on the frame; the input is MCP-6's evidence ([runtime-interface.md](runtime-interface.md), `run evidence`), of which only `request_id`, `steps[].step_index` and `steps[].frames[].ref` are read; exit 0 whenever the comparison was made |

### 1.73 Wave 15 slice NAV-3a follow-up (#373, M-373 v1.1 §3.8, §4.3; wave 14 rulings, NAV-3a items 3 and 5)

The NAV-3a follow-up retires the bundle maintenance use `prerequisite`, which NAV-3a left to
whichever of NAV-3a and PKG-S5 merged later (M-373 test row N3a-9), and changes the Lab's
`prerequisite_entry_example` from an entry of the retired actingd key `prerequisite_packages` to
the package's place in its game's standard-pack registry. No record type, wire shape,
configuration key or outcome code: the rows are a file-format vocabulary, a rule of the bundle
check and an output member. [package-reference.md](package-reference.md), "Maintenance
declarations", and [lab-recording.md](lab-recording.md), "Output", are the contract.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| N3F-1 | `MaintenanceUse` (`maintenance[].uses` of bundle v4 and of the source declaration array): `startup`, `return_home`, `recognition` | removed enum value `prerequisite` (resource file format) | NAV-3a follow-up | — | an entry that names `prerequisite` is an unknown use: the bundle index and the source array do not decode, a game root holding it is `standard_pack_root_invalid` `malformed` (§1.67 N3A-4), `package bundle --maintenance` and `resource validate` refuse the declaration file; no compatibility reading (M-373 §3.8). The UI's acsetup still names the value and drops it with PKG-S6 |
| N3F-2 | `validate_bundle_maintenance` (`package bundle --maintenance`, installers): the chains of a bundle are checked against every pack it holds | rule | NAV-3a follow-up | — | the bundle is its game's registry (M-373 §4.3): every pack binds a prerequisite package and no declaration binds one; a declared prerequisite the bundle does not hold is `contained_task_prerequisite_unbound`; `return_home` binds each server's fallback, as before |
| N3F-3 | `record stop` answer member `prerequisite_entry_example`: `{bundle: "<standard_packs.root>/<game>/bundle.json", pack: {package_id, server, entry_task_id, path: "packs/<package_id>"}, directory: "<standard_packs.root>/<game>/<package_id>"}` | changed output member (Lab), replaces §1.64 P5-5's `{package_id, package_path}` | NAV-3a follow-up | NAV-3a follow-up | the package's registry entry (`package bundle` adds `file_count` and `byte_count` to `pack`); `<standard_packs.root>` stays a placeholder. MCP `ac_record_stop` and `ac_binding_draft` pass it on verbatim, unchanged |

### 1.74 Wave 15 slice LAB-2 (#390, forced-compromises mapping row LAB-2; compromises L-02, T-03)

LAB-2 completes the Lab recording's generated package and the navigation projection. It writes
no ledger record, wire shape or configuration key: the rows are the Lab state file, the Lab's
tool input and output, and the resource files the tools write.
[lab-recording.md](lab-recording.md) ("Forbidden marks and anchors", "Drags and long presses",
"Marking after stop", "`--record` on device commands", "Pages and documents") and
[resource-declarations.md](resource-declarations.md) (the navigation paragraph) are the
contract. No code is new: the refusals reuse `validation_failed`, `record_mark_rejected` (new
reasons `forbidden_source_missing`, `forbidden_mark_passes`, `anchor_invalid` with `problem`
`not_own_template` or `not_required`), `record_mark_in_use`, `record_to_transition_invalid`
(new reason `page_rule`), `record_click_rect_conflict`, `record_click_outside_step_rect`,
`record_step_click_missing` and `record_session_not_active` (new `details.reason`
`recording_stopped`).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| L2-1 | Lab recording state (`actingcommand.lab-recording.v1`): step `forbidden: [ids]` and `anchor`, click `gesture` (`{"kind": "drag", "to", "duration_ms"}` \| `{"kind": "long_press", "duration_ms"}`), execution `to_point` and `duration_ms`, recording `amended_at_unix_ms` | new members (Lab state file, not the ledger) | LAB-2 | LAB-2 | absent when unused, so a recording without them is written as before; a template mark recorded without a search area keeps the default one it was given (L2-2) |
| L2-2 | `record mark` input: `--template <id>=x,y,w,h@x,y,w,h`; `--forbid <id>`; `--anchor <id>` \| `--no-anchor`; `--click x,y,w,h --drag-to x,y,w,h --duration-ms <ms>`; `--long-press x,y --duration-ms <ms>`; request members `forbid`, `anchor`, `clear_anchor`, `click.drag_to`, `click.long_press`, `click.duration_ms` (`actingcommand.lab-record-mark.v1`) | tool input (MCP `ac_record_mark` hands the request on unchanged) | LAB-2 | LAB-2 | a template mark without a declared search area searches its crop grown by 48 px on each side, clamped to the frame (coordinator input of 10-15: a game update shifted a menu bar by about 40 px and every search area equal to its template box failed); durations 1..=60000 ms |
| L2-3 | `record mark` output `forbidden[{id, family, status: "absent" \| "not_evaluated", frames}]`, `after_stop`, `step_state{forbidden?, anchor?}`; `record status` steps `forbidden?`, `anchor?`, click `gesture?`, `amended_at_unix_ms?` | output members (no persisted record) | LAB-2 | LAB-2 | absent when unused |
| L2-4 | the package `record stop` generates: page rule `forbidden`; the anchoring template as `anchors[] {id: <page>, …}` (target `page/<page>`, named by every reference); clicks `{"kind": "drag", "from", "to", "duration_ms"}` and `{"kind": "long_press", "x", "y", "duration_ms"}`; the default task timeout counting a gesture's duration | resource file format the tool writes (task schema `0.9`, unchanged grammar) | LAB-2 | LAB-2 | a recording that uses none of them generates the same bytes as before |
| L2-5 | `record stop` on a stopped recording marked again: generated again, `lab.status: "regenerated"` | output value and rule | LAB-2 | LAB-2 | options not given keep the generated package's values, except the task timeout, computed again; `--lab-dir` is written only when given |
| L2-6 | `do --capture --record --swipe x1,y1,x2,y2,ms`, and `do --capture --record` on a drag or long press step (one swipe; the plan's `gesture` and `to_point`) | tool input and output | LAB-2 | LAB-2 | was `record_flag_unsupported`; the Runtime operation is the existing coordinates swipe |
| L2-7 | the generated navigation's edge `{id, from_page, to_pages: [...], click, source}` for an operation whose `to` is a page set | resource file format (`navigation`, schema `0.6` unchanged) | LAB-2 | LAB-2 (source compilation) | exactly one of `to_page` and `to_pages`; a catalog navigation action and page projection element like any edge; the drive graph and actinglab's route tools leave it out; a `long_press` navigation click reads as a drag that does not move |

### 1.75 Wave 15 slice TOOL-1 (#390, forced-compromises mapping row TOOL-1, P-03; coordinator ruling 3 of wave 15)

TOOL-1 adds two actinglab authoring commands that take a content directory by its path,
`package fmt` and `package edit`. They write no ledger record, wire shape or configuration key;
their new shapes are two operation outputs (TL1-1, TL1-3) and the edit list an author gives
(TL1-2). [package-reference.md](package-reference.md), "Formatting and editing a content
directory", is the contract. Nothing computes, compares or reports a digest (option 丙). The
actinglab codes `package_fmt_check_failed`, `package_fmt_write_failed`,
`package_edit_patch_invalid`, `package_edit_pointer_unresolved`, `package_edit_admission_refused`
and `package_edit_write_failed` (error, ladder class `none`, in the new domains `package_fmt_` and
`package_edit_` of actinglab) are catalog entries, not rows (§7); a read or parse failure reuses
`package_invalid`.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| TL1-1 | `actinglab package fmt --package <directory>`: `{mode: "package_fmt", package, json_files, reformatted: [path]}`; with `--check`: `{mode: "package_fmt_check", package, json_files, unformatted: []}` | operation output (no persisted record; an actinglab result, read by a person or a CI job) | TOOL-1 | TOOL-1 | the canonical serialization of a JSON file: UTF-8 without a byte-order mark, two-space indentation, one member or element per line, members in their document order, LF line endings with a last LF (the coordinator's wave-15 ruling over FCM P-03's CRLF); other files are left byte for byte; paths are the directory's `/`-separated paths; `--check` writes nothing, and a JSON file that is not canonical makes it `package_fmt_check_failed` (exit 2) with details `{package, json_files, unformatted}` |
| TL1-2 | `package edit --patch <file>`: a JSON array of `{file, op: add \| replace, path, value}`, no other member, at most 32 MiB | input document (an author's file; the Runtime never stores it) | TOOL-1 | — | `op` is an RFC 6902 operation, `path` an RFC 6901 JSON pointer into the JSON file `file` of the package; the edits apply in order; `--file <path> --add <pointer> --value <JSON>` (or `--replace`) gives one edit instead |
| TL1-3 | `actinglab package edit --package <directory> (--patch <file> \| --file <path> (--add \| --replace) <pointer> --value <JSON>)`: `{mode: "package_edit", package, package_id, task_id, edits, files: [path], admission: "passed", written}`; with the global `--dry-run` also `status: "validated", dry_run: true, persisted: false` and `written: false` | operation output (no persisted record) | TOOL-1 | TOOL-1 | the edited content table is admitted as actingd admits a content directory before any file is written, and `files` (the files the edits changed, written in the canonical serialization of TL1-1) are written only after it passes; a refusal is `package_edit_admission_refused` with details `{package, code, detail, declaration, written: false}`; a write failure (`package_fmt_write_failed`, `package_edit_write_failed`) carries `{package, file, written, part_left}` |

### 1.76 Wave 15 slice POOL-1 (#390, forced-compromises mapping row POOL-1, compromise S-04)

POOL-1 lets a `ledger_fact` pool take its data from readings: a reading of the pool's key is the
pool's base, which the next reading or adjustment of the key resets, and the pool's value is
that base projected by the pool's regeneration rule. [resource-readings.md](resource-readings.md)
("Live pools"), [scheduling/README.md](scheduling/README.md) ("Pool Projection", "Fact
Refresh"), [resource-targets.md](resource-targets.md) and `docs/live-fact-pools.md` are the
contract. It adds no event type, no member, no enum value, no code and no configuration key:
existing records and outputs get new rules. The code
`contained_task_resource_reading_live_pool_unsupported` and the location
`check_resource_reading_pools` retire (runtime-host); the `reader_problem` token
`live_pool_key` is no longer given and stays in the vocabulary for what earlier builds wrote.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| PL1 | `fact.published` of a run's reading or adjustment write of the key a `ledger_fact` pool observes in the run's instance scope | new rule of an existing record (shape unchanged) | POOL-1 | POOL-1 | published as every reading is (the successful terminal's stages 1-3; stage 0 is gone), `invalidate_on` `[]`; it is the pool's base until the next reading or adjustment of the key; no run fails with `contained_task_resource_reading_live_pool_unsupported` any more |
| PL2 | the live pool source check of the policy evaluation and the forward projection (`pool_fact_invalidation_binding_missing`) | new rule (no persisted shape) | POOL-1 | POOL-1 | an active record of the pool's key that the Runtime published (event actor `runtime`) with a `source_detector` starting `resource_reading:` or `resource_adjustment:` needs no input invalidation; every record still needs its current native instance binding, and any other record still needs `input.committed` and `input.failed` |
| PL3 | the unknown reason of a `resource_projection` predicate on a `ledger_fact` pool without a usable record (`trigger_unknown` / `feedback_stop_unknown`, the detection suggestion and `planning_fact_unknown`) | new values of existing members | POOL-1 | POOL-1 | the key's own fact reason, `fact_missing`, `fact_expired`, `fact_low_confidence` (below the pool's minimum) or `type_mismatch` (not an integer within `0..=capacity`), instead of `resource_observation_missing`; with the pool in exactly the instance's scope the task raises the fact refresh's demand for the key (§1.56, D2-S4's records unchanged); a static snapshot pool keeps `resource_observation_missing` and raises none |
| PL4 | a resource target's inventory: `ApplyResourceTargets` `conditions[].state` (`computed.current`), `targetable_resources` `observation.current`, `active_resource_targets` and the evaluator's target terms | new rule of existing output members | POOL-1 | POOL-1 | for a `ledger_fact` pool, the record projected to the evaluation instant by the pool's regeneration rule (each complete period adds `amount`, up to `capacity`; a record above `capacity` is read as observed); `observed_at_unix_ms` stays the record's; a static snapshot pool's target reads the record as observed |
| PL5 | actingd `check-config` and start: the recognition maintenance package binding's key check (D2M §3.2 check 3, T14) | removed rule | POOL-1 | POOL-1 | a mark whose key a `ledger_fact` pool of the configured catalog observes no longer refuses the binding (`fact_reader_binding_invalid` with `live_pool_key`) and no longer makes a start ignore it; the `policy_fact_authority` check stays |

### 1.77 #410 item 1 `labobs`: observe and do say what their frame is (board #410; Alice 2026-10-15, the 0.12 end)

The Lab's `observe` and `do` answers, and MCP's `ac_lab_observe` and `ac_lab_do` that pass them
on unchanged, state the frame's coordinate system, give an optional JPEG preview by its path, and
`do` attaches its result after an action. No image is ever inlined (Alice: no item 7). No ledger
record, wire shape or configuration key changes; the rows are output members and local files.
[contained-lab-operation.md](contained-lab-operation.md) (the last section) and
[contained-page-observation.md](contained-page-observation.md) are the contract. The actinglab
code `lab_frame_export_failed` (warning, ladder class `none`) is a catalog entry, not a row (§7).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| LO-1 | `frame_coordinates {device_pixels: [width, height], orientation, applies_to: ["tap", "tap_rect", "swipe"]}` on every `observe` and `do` answer | output member (no persisted record) | labobs | labobs | `orientation` `landscape` (wider than high), `portrait` (higher than wide) or `square`; `do`'s is the frame its coordinates refer to (online the before frame); Min keeps it and counts it toward `observe`'s budget |
| LO-2 | `--preview` on `observe` (offline and `--capture`) and online `do`: `preview {path, width, height, scale}` or `{failure {code, path?, raw_text?}}` | flag and output member | labobs | labobs | a JPEG (quality 75), long edge at most 1024, `scale` = frame pixels per preview pixel (1 for a frame no larger); `observe --preview` under the global `--dry-run` is `dry_run_unsupported`; `do --preview` without `--capture` or under any dry run is `validation_failed` |
| LO-3 | the Lab frame directory `%LOCALAPPDATA%\ActingCommand\actinglab\frames`: `<sha256>.png`, `<sha256>.preview.jpg` | local files (client side, not the state root) | labobs | labobs | named by the SHA-256 of the frame's PNG, written through a partial file of the process renamed into place; never read back by the Lab, the Runtime or an export |
| LO-4 | `do` (online, after an action with an after frame) `after_attachment`: `{kind: "page_projection", observation, preview?}` for a recognized after page, else `{kind: "frame", frame_coordinates, frame_path, preview}` | output member | labobs | labobs | added after the Min projection of the operation members (which keep their budget); the known page's projection is cut on its own as `observe` cuts its own under Min, whole under `--verbose`/`--pretty`; also in a failure's details; a part not written is `{failure}` and the answer still succeeds |
| LO-5 | MCP `ac_lab_observe`, `ac_lab_do`: argument `preview` ↔ `--preview`; an answer exported past the output budget keeps `frame_coordinates`, `frame_path`, `preview` and `after_attachment` inline, each in that order while it fits | MCP argument and overflow rule (apps 41 parity) | labobs | labobs | the answer is otherwise actinglab's data verbatim (A5); `{req_id, export, overflowed: true}` and the warning `answer_exported` as before |

### 1.78 #410 item 6 `gaplog`: a local capability-gap log (board #410; Alice 2026-10-15, the 0.12 end)

An agent, or a person, records a capability it needed and no command offered, with a short why.
The record stays on the computer: one JSON line in the user's own local log, never sent anywhere
and never written to the ledger. A missing capability is not something the Runtime did (iron
rules 3 and 13), the client action record retires at U-6, and the log works without actingd.
[runtime-interface.md](runtime-interface.md) (the actingctl and MCP tables) is the contract. The
codes `actingctl_capability_gap_recorded` (success) and `actingctl_capability_gap_write_failed`
(error), both actingctl with ladder class `none`, are catalog entries, not rows (§7). The tool
table goes from 34 to 35 tools (observer 19 to 20).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| GL-1 | `%LOCALAPPDATA%\ActingCommand\actingctl\capability-gaps.jsonl` (below `APPDATA` without `LOCALAPPDATA`), one line per gap `{schema_version: "actingcommand.capability-gap.v1", recorded_at_unix_ms, via, capability, why}` | local file (client side, not the state root, not the ledger) | gaplog | gaplog (`actingctl capability-gap`, `ac_capability_gap`) | appended with one write per line; `via` `cli` or `mcp`; nothing reads it back, exports it or sends it |
| GL-2 | `actingctl capability-gap --capability <text> --why <text>` ↔ `ac_capability_gap` (`capability`, `why`): `{path, gap}`, the log and the line written | client instruction output (no persisted record beyond GL-1, no runtime-client shape) | gaplog | gaplog | each text one line, `capability` 1–120 and `why` 1–500 characters, else refused before writing (`actingctl_usage_invalid` / `arguments_invalid` with `value_missing`, `not_recognised` or `out_of_range`); MCP observer tier before U-3, a general tool after it; CLI success `actingctl_capability_gap_recorded` |

### 1.79 Wave 16 slice R7 + K4 (#361 R7, #372 K4; M361 and K372 addenda of 2026-10-15; forced-compromises mapping S-08)

R7 lets a V2 clock predicate skip an occurrence that was not served in time, so a missed slot
no longer takes a later day's budget; K4 records the slot losses no gate shows, the occurrence
R7 skipped and the one a later occurrence absorbed. [scheduling/README.md](scheduling/README.md)
("Clock Sources", "Clock-slot records"), [scheduling/v2/README.md](scheduling/v2/README.md) and
[policy-suspension.md](policy-suspension.md) (`clock_slots`) are the contract. No ledger event
type, no payload member and no configuration key is added (the actingd-config revision is
untouched); the records are the existing `goal_missed` planning signal. The codes
`clock_occurrence_expired` (policy, info, domain `clock_occurrence_`), `clock_slot_expired`
(runtime-host, info) and `clock_slot_superseded` (runtime-host, warning), all ladder class
`none`, and the key `grace_ms` (`duration_ms`) are catalog entries, not rows (§7).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| RK1 | `missed: {kind: "skip_after", grace_ms}` on a `clock` predicate of an `actingcommand.scheduling.v2` catalog | catalog member (resource file format, not a ledger shape) | R7+K4 | the W2 generator, after the v0.12.0 deploy (Q10) | optional and skipped when absent, so a catalog without it keeps its serialization, hash and approval; `grace_ms` 60 000..=604 800 000 (`limit_exceeded`); refused under `not` (`predicate_uncomputable`) and in a V1 document (`unknown_field`); an unknown `kind` is `type_mismatch` |
| RK2 | the decision reason `clock_occurrence_expired` {`occurrence_unix_ms`, `grace_ms`} | new code in existing reason chains | R7+K4 | R7+K4 | added by a clock whose unserved occurrence is past its grace; the decision stays `trigger_false` when that clock decides the trigger |
| RK3 | `policy.planning_signal_observed` of kind `goal_missed` with `clock_slot_expired` {`task_id`, `alias`, `occurrence_unix_ms`, `grace_ms`, `next_occurrence_unix_ms`?} or `clock_slot_superseded` {`task_id`, `alias`, `occurrence_unix_ms`, `next_occurrence_unix_ms`} | new codes of an existing record (shape unchanged) | R7+K4 | R7+K4 | `signal_id` `signal:clock_slot:<expired\|superseded>:<sha256 of task, alias and occurrence>`, once per (pair, occurrence); only for a pair with a dispatch history that is live (its trigger true with its counted clocks due); superseded occurrences within 48 h, at most 32 per pair |
| RK4 | `actingd suspended` `clock_slots`: entries of both codes; `grace_ms`; `slot_fate` `expired` | new values of existing members (`schema_version` unchanged) | R7+K4 | R7+K4 | an expired entry is `expired`, a superseded one `collapsed`, both settled; a refused or deferred entry of an occurrence with an expired record settles `expired` |
| RK5 | `ac_diagnose` / `actingctl diagnose` `clock_slots` order | new rule of an existing output member | R7+K4 | R7+K4 | refused and superseded rows (the `warning` codes) first, then deferred and expired rows, then owed rows, newest first in each; still at most 10, cut before `errors_page`, with `clock_slots_total` |

### 1.80 Wave 16 slice R-10 (#390, forced-compromises mapping R-10; REC-M §8.6, option 2)

R-10 decides what an OCR target located relative to an anchor template is while that anchor
does not pass: it did not pass, instead of an error that failed the whole page recognition. It
adds no event type, member, enum value, code or configuration key: an existing code becomes a
target's `failure`, and the page set gets one admission rule.
[resource-declarations.md](resource-declarations.md) (the R-10 paragraph after the value tests)
and [ocr-fields.md](ocr-fields.md) (the unresolved-region paragraph) are the contract.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| R10-1 | a target evaluation (the task diagnostic `target` row and its `ocr` row) of an OCR target with a `template_relative` region whose anchor did not pass | new rule of existing members (shapes unchanged) | R-10 | R-10 | `passed: false`, `failure` `ocr_region_unresolved` (no values), the `ocr` row's `region` with `unresolved: anchor_not_matched` and no `roi`, empty `raw_text` and `derived_text`, no `confidence`, `block_count` 0, and no provider call; in a page rule, a check member, `guard.target_id` and `guard.check` alike (a guard refuses its input with `contained_task_guard_refused`, its `cause=` token `ocr_region_unresolved` when the guard target is the OCR target). `out_of_frame` and `coordinate_overflow` stay evaluation errors; fields and readings keep `region_unresolved` |
| R10-2 | the page set's `forbidden` (`recognition.pages.json`): an OCR target with a `template_relative` region, directly or as a member of a check | new admission rule (resource file format) | R-10 | — | admitted only when its anchor template is directly in the same page's `required`; otherwise `resource_declaration_invalid` at `/pages/<i>/forbidden/<j>` (`InvalidValue`). No pack in the three resource repositories at their 10-15 heads names such a target in a page rule |

### 1.81 Wave 16 slice RD-4 (#398 model-398 v1.1 §6, option B; coordinator ruling 3 of wave 16)

RD-4 gives the recovery ladder per-device profiles and the rule for a trigger's start rung beyond
a profile's rungs (model-398 §6.6, option B, coordinator ruling of 2026-10-15: default
`exhaust`). [emulator-control.md](emulator-control.md), "Ladder profiles", and
[runtime-config.md](runtime-config.md), "The recovery ladder section", are the contract. No
code, location, vocabulary value, event type or record member is added: the member `profile`
(LD3) and the skip reason `device_not_ready` with `recovery_rung_skipped_device_not_ready` (LD4)
exist since D1c, and RD-4 is their first writer. `mechanism_sections()` keeps its sections: the
keys join the existing `recovery_ladder` and per-instance sections.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| RD4-1 | `recovery_ladder.profiles` (object, profile name to `{rungs}`), `recovery_ladder.kind_profiles` (object, device kind to profile name), `recovery_ladder.rules.start_beyond_profile` (`exhaust` / `last_rung` / `refuse`) | new configuration keys (§5) | RD-4 | RD-4 | defaults `{"handset": {"rungs": [return_home, application_restart]}}`, `{"handset": "handset"}`, `exhaust`; settable, in the section's digest, checked with the section (a profile's rungs as `rungs`, refusals naming `recovery_ladder.profiles.<name>`; a kind or name that does not resolve `out_of_range`; under `refuse` every start rung within every profile, else `start_rung_out_of_range`) |
| RD4-2 | `instance.<instance id>.recovery_ladder_profile` (the `instances[]` entry's `recovery_ladder_profile`) | new per-instance configuration key (§5) | RD-4 | RD-4 | default `none` (no row unless the entry names it); settable; it must name a profile (`recovery_ladder_config_invalid` at start and in check-config, `config_value_invalid` by instruction, `out_of_range`); it comes before `kind_profiles` |
| RD4-3 | `recovery_ladder_started` `profile` | first writer of an existing member (LD3) | D1c | RD-4 | the name of the profile the instance climbs by, taken at admission with the section's snapshot; absent when it climbs the section's `rungs` (every emulator under the defaults) |
| RD4-4 | `recovery_rung_finished` `skipped` with `recovery_rung_skipped_device_not_ready` | first writer of an existing skip reason and code (LD4) | D1c | RD-4 | a handset's start gate held the rung's run before its first step (`device_locked`, `device_thermal_hold`, `device_battery_low`, `device_in_use`) under `rules.entry_unavailable = skip`; under `fail` the rung fails with that code, and after the application restart's stop it fails |
| RD4-5 | the plan of `recovery_ladder_started` for a start beyond a profile | new rule of an existing member | RD-4 | RD-4 | `exhaust`: every rung `before_start_rung`, then `recovery_ladder_finished` `exhausted` with `rungs_tried` 0 as any all-skipped climb; `last_rung`: the rungs before the profile's last `before_start_rung`, the last one climbs |

### 1.82 Wave 16 slice lpbox: a long press takes a click box (#390; coordinator ruling 6 of wave 16; Alice 2026-10-09)

Alice's rule of 10-09 is that pack taps use click boxes (a point picked inside a box sized to
the control, the whole box harmless), never fixed points. The kernel's `long_press` (alias
`long_tap`) was the one click of a task package with no box form: `point` has `rect` and
`target_center` has `target`, and every drag already takes rectangles. It now takes a click
box, and the Lab records a long press as the box around the point it presses (wave-15 rulings,
LAB-2 item 2). [resource-declarations.md](resource-declarations.md) (the lpbox paragraph) and
[lab-recording.md](lab-recording.md) ("Drags and long presses") are the contract. No code, no
configuration key and no ledger member is new; the refusals reuse
`contained_task_operation_invalid`, `contained_task_input_out_of_bounds`, the declaration
reason `MissingField`, `invalid_input_sampling_action` and `validation_failed`.

Migration: none needed. The point form stays valid, so a package with a point long press keeps
loading. No package declares one today: none of the three resource repositories' `main`, nor
their open pull requests (checked on 2026-10-15), holds a `long_press` or `long_tap`
click. A point long press written from now on needs the reason its target has no stable extent
(Alice 10-09); otherwise it is a box.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| LB-1 | a task operation's or navigation edge's click `{"kind": "long_press" \| "long_tap", "x", "y", "width", "height", "duration_ms"}` | resource file format (an optional member pair; task schemas unchanged) | lpbox | lpbox (Lab generation) | `width` and `height` together (one alone is `MissingField` at the other, `contained_task_operation_invalid` in the kernel), each at least 1, inside the resolution (`contained_task_input_out_of_bounds`); the point form `{kind, x, y, duration_ms}` is unchanged; the navigation projection and a guard synthesized from the click keep the box |
| LB-2 | the run's held point: a point of the box drawn with the step's action seed (`xorshift64_uniform_rect_v1`, as a `rect` tap's), the box's center without a seed; `task.semantic` `EffectIntent` `sampling {algorithm, action_seed, source_regions: [box]}` | new rule of an existing optional member (ledger shape unchanged) | lpbox | lpbox | the sanitizer took `sampling` on `Tap`, `Swipe` and the two drags; it now also takes it on `LongTap`, with one region holding the point (else `invalid_input_sampling_action`); a point long press records none, as before |
| LB-3 | drive navigation and actinglab's navigation reader on a boxed `long_press` click: a drag that does not move, from the box to the box, at its center | new rule (no persisted shape) | lpbox | lpbox | as they press a `rect` tap at its rectangle's center; a box needs both members (`drive_plan_invalid` / `validation_failed`) |
| LB-4 | `record mark --long-press x,y,w,h`, request member `click.long_press {x, y, width, height}` (`actingcommand.lab-record-mark.v1`) | tool input (changes L2-2's point) | lpbox | lpbox | the box is the step's click rectangle; a bare point is `validation_failed`; MCP `ac_record_mark` hands the request on unchanged |
| LB-5 | the package `record stop` generates for a long press: `{"kind": "long_press", "x", "y", "width", "height", "duration_ms"}` with the step's click rectangle; `do --capture --record` on it holds the box's center (`point_rule` `rect_center`, was `declared_point`) | resource file format the tool writes; output value | lpbox | lpbox | changes L2-4 and L2-6; the first decision must hold a point inside the box; a recording made before keeps its 1x1 click rectangle and generates a box of one pixel |

### 1.83 Wave 16 slice navoffset: navigation readers accept REC-4 `offset` clicks (#390, wave-15 rulings LAB-2 item 7; coordinator ruling 7 of wave 16)

LAB-2 found that neither navigation input reader accepted an `offset` click, so a package whose
navigation holds one (REC-4's `--click` with `--click-follow`, or a source `rect_move`) failed
online observation and could not be a carrier package. Both readers now take it. No ledger
record, wire shape, configuration key or outcome code: the rows are a reading rule of an existing
resource format and two output values. [resource-declarations.md](resource-declarations.md) (the
navigation paragraphs) is the contract. A refusal keeps the reader's existing code
(`drive_plan_invalid` in the kernel, actinglab's usage refusal, `observation_resources_invalid`
for the online observation's admission).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| NO-1 | the click `{"kind": "offset", "target_id", "offset": {x, y, width, height}}` of a navigation edge, page operation or control point, read by the kernel's drive reader and actinglab's navigation reader | new reading rule of an existing resource format (`navigation`, schema `0.6` unchanged) | navoffset | — (the Lab and source compilation write it since REC-4) | the box at the top-left corner of the box `target_id` matches plus the offset, each number scaled by the factor a template with `scales` matched at (REC-5, as a task's `offset` click); a width or height that is not positive, or no string `target_id`, is refused; was `unsupported navigation click kind` |
| NO-2 | the page projection element of an offset click: the online observation (`ObserveContainedPage`, which admits the carrier package of `observe` and `do`) and actinglab `observe` | new rule of an existing output (page projection shape unchanged) | navoffset | navoffset | `recognition_basis: target_evaluation`, `input` the tap of that box on the frame observed; no geometry when the target did not pass or matched no template box (`target_not_matched`, `matched_template_rect_unavailable`); a package holding one is admitted (was `observation_resources_invalid`) |
| NO-3 | the unresolved semantic input `{"type": "target_offset", "target_id", "offset"}` (actinglab's navigation listings and the Lab's `SemanticInputResponse`) | new value of an existing output member | navoffset | navoffset | a route resolves it on the current frame like a `target` edge and taps the center of the box |

### 1.84 Wave 17 slice NAV-5: full-match entry routing (#373, M-373 v1.1 §5, §4.4, §8.2, §8.3, §8.5, §9; wave-17 rulings 1, 19 to 21)

NAV-5 brings its own shapes and writes them. A run of a game whose root holds no
`navigation.json` writes none of them and is as before NAV-5 (N5-9: an entry the page list
does not hold adds one warning record, NV5). [package-reference.md](package-reference.md),
"Standard package root" ("Navigation"), [policy-suspension.md](policy-suspension.md) ("Failure
identity", "Which failures accumulate", "Lifting"), [linear-steps.md](linear-steps.md)
("Return-home fallback") and [resource-declarations.md](resource-declarations.md) are the
contract. No configuration key or section: the root is NAV-6's `standard_packs.root`.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| NV1 | root file `navigation.json` (`actingcommand.navigation.v1`) | resource file format, not a ledger shape | NAV-5 (reader, contract checks) | resource side (NAV-1) | `{schema_version, game, edges [{from, to, package_id}] (0..=1024), recovery_chain [package ids] (1..=8)}`, `deny_unknown_fields`; refused as `standard_pack_root_invalid` with the new `standard_pack_problem` values `edges_count`, `edge_page_invalid`, `edge_page_unlisted`, `edge_self_loop`, `edge_duplicate`, `recovery_chain_count`, `recovery_chain_duplicate` (and `pages_missing`, `bundle_missing`, `package_not_listed`); `actinglab resource validate` family `standard_pack_navigation`; check-config's `root_files` may list it |
| NV2 | `TaskSemanticFact::NavigationLocated` (`task.entry_preflight`, kind `navigation_located`) | new record | NAV-5 | NAV-5 | `{attempt 1..=16, matched_pages [<=8 pages.json ids], matched_total <=256, interrupt_page?, outcome}`, `invalid_task_navigation` otherwise; `outcome` `contained_task_navigation_located` (info; `page_id`, `count`), `_navigation_page_unrecognized` (warning; `count`), `_navigation_page_ambiguous` (warning; `page_ids`), `contained_task_interrupt_ambiguous` or, with `interrupt_page`, `contained_task_interrupt_handled` (M-373 §8.3 names the record; the two interrupt outcomes are the locate's verdicts on an interrupt page) |
| NV3 | `TaskSemanticFact::NavigationRoutePlanned` (kind `navigation_route_planned`) | new record | NAV-5 | NAV-5 | `{from_page, hops [{from, to, package_label}] (<=16, none when the located page is an entry page)}` (`TaskNavigationHop`, `deny_unknown_fields`) |
| NV4 | `TaskSemanticFact::NavigationWarning` (`task.entry_preflight`, kind `navigation_warning`) | new record | NAV-5 | NAV-5 | `{outcome}` (`deny_unknown_fields`; a public projection keeps its codes only): `contained_task_navigation_route_missing` (`operation` `navigation_route`, `page_id`), `_navigation_hop_unconfirmed` (`operation` `navigation_hop`, `package_id`, `page_id`) or `_navigation_entry_unlisted` (`operation` `navigation_route`, `page_ids`), all warning; ends no run; the records M-373 §5.6 asks for, a shape §8.3 does not name (as NAV-6's N3; wave-17 acceptance ruling 1) |
| NV5 | `TaskEntryRecoveryRole` values `navigation_hop`, `recovery_chain`; `TaskEntryRecognitionPhase` value `navigation_target` | new enum values of existing members | NAV-5 | NAV-5 | the roles of an edge's and a chain step's layer on `EntryRecovery*` (N1); the phase of the feature package's check of a hop's `to` page on `EntryRecognition`; a replay counts neither role's layer as a gate package |
| NV6 | `runtime.fact_recorded` `task.page` | existing fact, new writer | NAV-5 | NAV-5 | `<game>/<pages.json id>` when a locate finds exactly one page (M-373 §8.2); written only when it changes, never on an unrecognised or ambiguous frame |
| NV7 | failure identity (`policy.execution_recorded` `failure.failure_identity`) segments `~n<I><H>` and `~g<H>` | new optional segments of format v2 | NAV-5 | NAV-5 | after `p`/`r`, in the order `n`, `g`, on a failure whose code starts with `contained_task_navigation_` or `contained_task_interrupt_`, or that happened in a navigation, chain or interrupt layer; `n` the layer's package (or the one a `contained_task_navigation_package_invalid` names), `g` the hash of the game root's `pages.json`, `navigation.json`, `interrupts.json`; such failures accumulate in every scope but `prepare` (M-373 §8.5, §15 ruling 4; the `contained_task_interrupt_*` failures by wave-17 acceptance ruling 2, which M-373 does not say); a lift check lifts when the layer's package or the data files changed; the prefix grows to at most 250 bytes |
| NV8 | outcome codes `contained_task_navigation_located`, `_page_unrecognized`, `_page_ambiguous`, `_route_missing`, `_hop_unconfirmed`, `_entry_unlisted` (`none`), `_entry_unconfirmed` (error, `pack_logic_mismatch`), `_limit_reached` (error, `budget_or_permission`); `invalid_task_navigation` (contract); locations `navigation_locate`, `navigation_route`, `navigation_hop`, `navigation_chain`; key `navigation_limit` and vocabulary `navigation_limit` (`locates`, `hops`, `steps`, `interrupts`; `interrupts` has no writer yet, wave-17 acceptance ruling 3, see the follow-up below); `navigation_problem` values `terminal_mismatch`, `application_entry` | registry (fragments) | NAV-5 | NAV-5 | `contained_task_navigation_recovery_exhausted` (registered pending by D1c) gets its first writer; `contained_task_navigation_package_invalid` its roles `edge` and `chain`; `contracts/outcome-codes.json` is regenerated by the train (G9); `_entry_unconfirmed` is not in D1c's default `exclude_codes` (wave-17 acceptance ruling 7) |
| NV9 | kernel `PreparedContainedTask::run_as_host_layer`, `recognize_navigation_entry`, `await_navigation_entry`, `capture_scene`, `navigation_entry_pages`, `has_application_entry`, `terminal_holds_page`; `Scene` re-exported (and `Clone`) | new kernel interface, not persisted | NAV-5 | — | a recovery chain step runs as a run the host starts itself (wave-17 ruling 19, EXEC-3 ruling 2's open point): a page-graph application entry begins with its effect; every other layer and run keeps initial-frame-first |
| NV10 | the drive and actinglab navigation readers' `destructive_actions` in every click form | new reading rule of an existing resource format | NAV-5 | — | wave-17 ruling 21: `target`, `offset`, `drag`, `long_press` were unreadable (`unsupported navigation click kind for tap rect`); fixed boxes block a route's taps without a frame, a target's box on the frame the route resolves its edge on |

Follow-up (wave-17 acceptance ruling 3): [ ] the `navigation_limit` token `interrupts` gets its
writer in the NAV-5 locate's interrupt branch (`handle_navigation_interrupt` in
`host/standard_pack.rs`, called by the locate in `host/navigation.rs`): when the run's total
of 16 interrupt handlings (M-373 §5.5) is spent while a gate navigates, the run ends
`contained_task_navigation_limit_reached` with `navigation_limit: interrupts`. Until that row
lands, NAV-6's `contained_task_interrupt_limit_reached` ends it (N6-7).

### 1.85 Wave 17 slice SEL-L4b: select over a scan (#308, frozen selector model S0 v2.1 §6 row L4b; wave-17 rulings 2 and 22)

SEL-L4b lets a package declare a select step's `scan`, a select over a list read across
viewports, and its `on_empty`, a normal end of the run when nobody listed is present, and lets
an operation's `expect_after` read back the chosen business ID (`expect_identity`). It brings
the scan select's `Decide`, `Return`, `Confirm` and selecting tap, the record of every path once
the decision exists, the on-empty end of a run with its admission in both route checks, and the
ledger-forensics pairing of several effect intents under one step action (M-357b D34).
[selection-graph.md](selection-graph.md) (section Select step, through Expected identity),
[README.md](README.md) (phases) and
[business-identity-consensus.md](business-identity-consensus.md) are the contract. It adds no
record type and no member: the record shapes are SEL-L4a's (§1.19) and the scan moves' intents
SEL-L3's and SEL-L3b's (§1.49, §1.58); SEL-L4b is their first online writer. The codes
`selection_result_blocked`, `selection_identity_unconfirmed` and `selection_identity_mismatch`
(execution-kernel, error, `pack_logic_mismatch`), the locations `select_decide`,
`select_return`, `select_confirm` and `select_expect_identity` (the domain `select_` that
execution-kernel already holds), and the keys `selection_kind`, `outcome_key`,
`selection_blocking` and `identity_state` with their vocabularies are catalog entries, not rows
(§7). The admission refusals reuse `contained_task_select_invalid`,
`contained_task_outcome_declaration_incomplete` and `task_phases_outcome_incomplete`. The
coordinator's device run of the model's row (a neutral smoke pack on a real list: the drags,
one tap, the identity on the detail page; once with a listed member present and once with
nobody listed) is a VAL line, not part of the merge gate.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| L4B-1 | package format: a select step's `select.scan` and `select.on_empty {outcome_key}`, and an operation's `expect_after.expect_identity {target_id, choice_of}` | new members (resource file format, not a ledger shape; task schemas unchanged) | SEL-L4b (parse and admission; the scan's types SEL-L3 and SEL-L3b) | resource side | the declaration gate: `scan` holds `identity_feature`, `axis`, `advance`, `reposition`, `settle`, `track`, `start`, `end` and `bounds` (`MissingField`) and optionally `early_stop`, nothing else (`UnknownField`); `on_empty` and `expect_identity` hold their members as strings (`InvalidType`). Admission (`contained_task_select_invalid`): the static checks of S0 §2.1 (a `repeated_anchor` layout with `unknown_identity: reject` and one-frame consensus; the policy's last tie-break key `candidate_id lowest_first`; the settle, viewport and `max_ms` bounds, `max_ms` within the task's `timeout_ms`; the tracking area and its core; each move's steps against the band; checks that are targets of the pack); every policy field has a domain from the layout; `on_empty` needs mode `none_allowed`, a `scheduling_outcome`, a `fixed_slots` layout when there is no `scan`, no post-admission OCR comparison, and post-admission fields whose `page_ids` include the step's `from` page; `expect_identity` needs a `choice_of` select step at or before the declaring one with an `ocr_aliases` identity, a declaring operation that is not retryable, and an OCR `target_id`; the route's terminal for `on_empty` is row L4B-5's |
| L4B-2 | `task.selection_evaluated` of a scan select | first writer of existing shapes (SL1–SL3) | SEL-L4a | SEL-L4b | written online once the decision exists, before the step's tap or failure return (a failure before the decision writes none, S0 §3.4 P1): the v2 projection, whose `gated_out.identities_sha256` is the SHA-256 (64 lowercase hex) of the compact JSON array of the gated-out business IDs in ascending order; `unknown_check` always; the confirmation per path, `not_attempted` (P2–P7), `capture_failed`, `page_changed` or `guard_failed` (P8), `reposition_failed` at `return` (P9) or `confirm` (P12), `identity_unconfirmed` (P10), `identity_matched` (P11); an abort keeps no verdict; a failure of the runtime itself after the decision is recorded only when the runtime classifies it as fatal |
| L4B-3 | `task.selection_evaluated` of a single-frame select that declares `on_empty` | first writer of an existing member (SL3) | SEL-L4a | SEL-L4b | the v1 record adds `unknown_check`: its unknown instances are the actionable candidates the kernel keeps from the evaluator and those the evaluator drops as unknown; rules 2–3 of S0 §3.2 block before the confirmation (`not_attempted`, `selection_result_blocked`); otherwise its confirmation is `matched` or `mismatched` as before |
| L4B-4 | `task.effect_intent` of a scan select's moves, the return moves included | existing record, written as rows L3-1 and L3B-2 state | SEL-L3 (kernel), SEL-L3b | SEL-L4b | written online from this slice on: the advances, correctives and rewinds, and the return's `reposition` and `advance` moves, each linked to the frame it was sampled on and followed by its input, none followed by `task.effect_completed`; every return viewport counts toward `max_reposition_viewports`; the selecting tap follows, bound to F2, with the step's one `task.effect_completed` |
| L4B-5 | the on-empty end of a run | new rule of existing records (`StepFinished`, the run's terminal) | SEL-L4b | SEL-L4b | no tap and no `task.effect_intent` or `task.effect_completed` for it; the step's `StepFinished` with `page_label` the step's page as recognized on its last frame and, in a phased task, `phase` `completed: false`; the terminal Completed on that page in the middle of a stage, `final_page` that page and `selected_scheduling_outcome` the `on_empty` key; the host derives the disposition from that last `StepFinished` unchanged. Admission: the state at the step is a terminal of the route (exactly one mapping with its effect condition lists the page, and its key is the `on_empty` key; a step reachable under both effect conditions is refused), `contained_task_outcome_declaration_incomplete` without phases (Runtime admission) and `task_phases_outcome_incomplete` with them (source parsing, build and Runtime admission) |
| L4B-6 | `contained_task_prerequisite_incompatible` detail `reason=select_on_empty` | new value of an existing refusal detail | SEL-L4b | SEL-L4b | a package with an `on_empty` step serves neither as a prerequisite package nor for return-home use, because an early end on the step's page would leave the dependent package short of its entry page |
| L4B-7 | `actingledger export --task-evidence`: the pairing of an input with its step's effect intent | new rule of an existing output (report shape unchanged) | SEL-L4b | SEL-L4b | M-357b D34 (§5.4): an input's `source_step` is the intent of its step action linked to the input's `before_frame_id`, judged unique after that narrowing, and a step row's `physical_inputs` holds only the inputs whose `before_frame_id` is its frame; it covers a scan's moves and a multi select's taps under one step action, where every input was `ambiguous` before |

### 1.86 Wave 18 slice 372-K5: a same-day catalog change accounts by the new catalog's task identity (#372 K5; K372 addendum of 2026-10-17; forced-compromises mapping S-08)

K5 lets a V2 task declare the task ids of an earlier catalog it continues, so a rename that
activates during a day keeps that day's budgets, slot, outcome and failure state instead of
running again, stalling its followers or silently lifting a pause. Under the active catalog a
task's accounting group is its own id and the ids it declares; the host reads every
per-(task, instance) state it derives from the ledger over that group, at read time only:
nothing in the ledger, the projections or the control state is re-keyed, and replay stays per
dispatch against its own generation. The Runtime never infers a rename (coordinator ruling Q6).
[scheduling/README.md](scheduling/README.md) (the paragraph after "Task ids are global"),
[scheduling/v2/README.md](scheduling/v2/README.md) and [policy-suspension.md](policy-suspension.md)
(`clock_slots`, `next_dispatch`) are the contract. No ledger event type, no payload member, no
code, key, domain, vocabulary token and no configuration key is added (the actingd-config
revision is untouched); the catalog diagnostics reuse `duplicate_id`, `limit_exceeded`, the task
id rule's `type_mismatch` and the unknown-member refusal (`unknown_field`).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| K5-1 | `continues_from: [task id, ...]` on a task of an `actingcommand.scheduling.v2` tasks document | catalog member (resource file format, not a ledger shape) | 372-K5 | the W2 generator, after the v0.12.0 deploy (K372 Q9) | optional and skipped when absent, so a catalog without it keeps its serialization, hash and approval; 1..=8 ids (`limit_exceeded` at the member), each by the task id rule; an id that is a task of the same catalog, one listed twice in one task, and one two tasks claim (a split) are `duplicate_id` at the item; refused in a V1 document (`unknown_field`); never checked against the ledger |
| K5-2 | `policy.dispatch_admitted`'s budget receipt: `task_daily_used`, `task_window_used`, `task_runtime_reserved_ms` | new meaning of existing values (shape unchanged) | 372-K5 | 372-K5 | for a task that declares `continues_from` they count its accounting group on the instance and day or window; the host's own count of each id keeps only that id's runs, so a replay against the dispatch's own generation rebuilds the same receipt; for any other task they are unchanged; payload validation (`used` between 1 and the limit) holds |

### 1.87 Wave 17 slice PM-CFG: the monitor reads its row (#385, PERF v1.4.2 §10.1; wave-17 ruling 10)

Every key of §5's `performance.monitor.*` / `.pressure.*` / `.load.*` row, parsed, checked and
defaulted since P-c (§1.24), now takes effect: the start-up assembly reads the row once
(`MonitorSettings` in runtime-config, refusing a value that breaks a rule as `perf_config_invalid`
naming the key) and hands it to the performance monitor, its host sampler and the performance
controller; the controller hands `load.heavy_threshold_milli` on to the policy evaluator, so both
judge heavy work by that one key. Each default is the constant the monitor ran with before, so a
configuration that names none behaves as before; no pacing mode changes
(`performance.pacing.mode` stays `observe`). No persisted record, wire shape, outcome code or
configuration key changes;
[runtime-config.md](runtime-config.md) ("The performance section") and
[performance-control.md](performance-control.md) are the contract.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| PMC-1 | the 15 keys of PERF §10.1 in effect → the monitor (sampling period, context window, summary period, sample bounds, failures in a row, freshness multiplier, stutter gap, pressure streaks, thresholds, severity, GPU aggregate), its host sampler (`ProcessLoadThresholds.min_coverage_basis_points` beside the process thresholds) and the controller (`load.heavy_threshold_milli`), whose resource overlay hands the heavy threshold to the policy evaluator (`HostResourceSnapshot.heavy_threshold_milli`, in memory only: no member of the resources document or a request) | readers of existing keys (no new key) | PM-CFG | PM-CFG | read at assembly only (not settable); the severity's `elevated_bp` is the record's `high` level, below it `elevated`; the controller and the evaluator judge heavy work by the one key (was the evaluator's own 1 500) |

### 1.88 Wave 18 slice NAV-T: the standard-pack cross-check (#373, M-373 v1.1 §3.1, §3.9 second row, §11.1, §11.2 rows NT-1 to NT-4; wave-18 ruling 1)

NAV-T makes `package bundle` carry and cross-check a standard pack's root files and adds
`actinglab standard-pack check`, which runs the same checks on an installed game root. No
record type, ledger member or configuration key: the rows are tool inputs and outputs, a
resource file rule and in-process interfaces. [package-reference.md](package-reference.md)
("Content-directory tools and the bundle index", "Standard-pack root files"; "Standard package
root") and [actinglab-dry-run.md](actinglab-dry-run.md) are the contract.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| NT1 | `package bundle --pages <file> --interrupts <file> --navigation <file>`; output `pages.json`, `interrupts.json`, `navigation.json` beside `bundle.json` | new tool flags and output layout (`PackageBundleRequest.root_files`, `StandardPackRootFiles`) | NAV-T | resource side (PKG-S7 bundle script, NAV-4) | each copied byte for byte once the game root the bundle makes passes `read_standard_pack_root` (`standard_pack_root_invalid` with `game`, the given file's `path`, `standard_pack_problem`, `json_pointer`) and the cross-check; a flag without its path, or a file that cannot be read, is `validation_failed`; with any root file every pack is admitted with `describe_navigation_path` |
| NT2 | the cross-check (M-373 §3.9 second row, checks 1 to 4) | rule of `package bundle` and `standard-pack check` | NAV-T | — | feature packages (`recognize_only`, game and bundle server, pages exactly the list's; REC-M budget by admission), edges (layer-eligible, entry holds `from`, ends on `to`, no application entry), recovery chain, handler and `then.run` packages (layer-eligible, no prerequisite of their own), handlers' entries (hold their page); layer eligibility is `prerequisite_incompatibility_with` with the game, the bundle server and that server's feature package resolution; a refusal is `package_bundle_navigation_package_invalid` (`package_id`, `navigation_role`, `navigation_problem` as the Runtime's `contained_task_navigation_package_invalid`; `admission_failed` caused by the loader's refusal); an `on_empty` package is `role_ineligible` |
| NT3 | `actinglab standard-pack check --root <game root>` answer `{status: "checked", root, game, root_files, pack_count, warnings}` (`StandardPackCheckResponse`) | new command and output shape | NAV-T | NAV-T | reads only (dry run `no_effect`); capability `standard-pack check` (`offline`, `read_only`); the game is the root directory's name; a root without root files is `standard_pack_root_invalid` `bundle_missing`; unknown flags and positionals are `validation_failed` |
| NT4 | the report "entry page not in the list": `package_bundle_entry_page_unlisted` (warning; `package_id`, `page_ids`) in `warnings` of both answers | new output member contents | NAV-T | NAV-T | one per pack whose `navigation_entry_pages` (without the game prefix) are not all pages of `pages.json`; none without `pages.json`; NAV-4 (resource CI) reads it |
| NT5 | bundle v4 reserved `package_id`s (`reserved_package_id`, any ASCII case): `tables`, `<kind>.json`, `<name>.part`, `<name>.old-<unix_ms>` | resource file rule (bundle index v4, §1.64 P5-1) | NAV-T (M-373 §3.1, §3.8 item 2) | — | `BundleIndex::validate` refuses one as `invalid_bundle_pack_path`, so a game root whose `bundle.json` lists one is `standard_pack_root_invalid` `bundle_invalid`; `package bundle` refuses a pack that declares one as `package_bundle_index_invalid`; every root file is named `<kind>.json` (§3.1), so every such name is reserved |
| NT6 | contract `read_standard_pack_root`, `StandardPackRootBytes`, `StandardPackRoot`; kernel `PreparedContainedTask::describe_navigation_path`, `NavigationPackageDescriptor` | new in-process interfaces, not persisted | NAV-T | — | the game root check the Runtime's reader (`host/standard_pack.rs` `read_game_root`, unchanged in behaviour), `package bundle` and `standard-pack check` share (M-373 §3.9 "三处共用"); the descriptor carries the answers of `page_ids`, `entry_holds_page`, `linear_entry_page`, `terminal_holds_page`, `has_application_entry` and `navigation_entry_pages`, which now share their page sets with the descriptor |
| NT7 | outcome codes `package_bundle_navigation_package_invalid` (error, `none`), `package_bundle_entry_page_unlisted` (warning, `none`) | registry (fragment `resource-tooling.json`) | NAV-T | NAV-T | `standard_pack_root_invalid` (contract, shared) gets the tools as writers; `contracts/outcome-codes.json` is regenerated by the train (G9) |

### 1.89 Wave 18 slice DISC-1: a start retries the instance discovery while it is unavailable (#390; coordinator ruling 2 of wave 18)

After the 10-16 boot the Runtime started before the emulator, its discovery answered
`instance_discovery_unavailable` and the start ended in `fatal_hold` until a person restarted
the machine; v0.12.0 has no watchdog (WD-1 is 0.12.1). From DISC-1 on the start tries the
discovery again, at most `startup_discovery.attempts` times and
`startup_discovery.retry_interval_ms` apart on the Runtime clock, records a warning for each
failed attempt that it tries again, goes on once an attempt succeeds and ends exactly as before
when the last attempt fails. Only the startup path retries. [provider-startup.md](provider-startup.md)
("Discovery unavailable at start") and [runtime-config.md](runtime-config.md) ("The
`startup_discovery` section") are the contract. The code `instance_discovery_retried` (actingd,
warning, `ladder_class` `none`: a process start, row 7) is a catalog entry, not a row (§7).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| DI1 | `provider.startup_observed` observation `discovery_retried` (`ProviderStartupObservation::DiscoveryRetried`) | new value of an existing record's `phase` | DISC-1 | DISC-1 | payload `{outcome}` (`deny_unknown_fields`), backend `mumu_manager`, inside the `instance_discovery` bracket, written before each wait: `instance_discovery_retried` with `attempts` (the number of the attempt that failed), `limit_count` (`startup_discovery.attempts`) and `retry_after_ms` (`startup_discovery.retry_interval_ms`), all required, and the attempt's failure as its `caused_by` link (`instance_discovery_unavailable` over the device failure, `provider_native_failed` or a MuMu resolution reason); the record's severity is the code's category, `warning`; the same warning goes to actingd's stderr as its daemon lines (developer line and `OUTCOME actingd:` line; coordinator 10-16 23:0x); none is written for an attempt that succeeds, for the last attempt or for any other refusal (`failed` as before) |
| DI2 | `startup_discovery.attempts`, `startup_discovery.retry_interval_ms` | configuration keys (§5) | DISC-1 | DISC-1 | integer `1..=100`, default 25; milliseconds `1000..=300000`, default 10 000 (24 waits of 10 s, about four minutes, cover a normal cold start of the emulator, about three minutes); default `file`, not settable (read once at assembly, before any instruction value is restored); out of range: `config_value_invalid` (`out_of_range`) for the key at start and in `check-config` |
| DI3 | `ProviderStartup::startup_discovery()` and `ProviderStartup::pause(duration)` | new host interface, not persisted | DISC-1 | — | the section in effect (the host reads it from the manifest, or the defaults file without one) and a wait on the Runtime clock followed by the install preparation boundary |

### 1.91 Wave 18 slice 九处-1: overview and status show each emulator's running state (#390 comment 6095942731, item 1; wave-18 ruling 4 as corrected by the coordinator, 10-16 22:5x)

`actingctl status` (and so `actingctl emulator status`) and each `ac_overview` /
`actingctl overview` instance row show whether the instance's emulator runs, as the Runtime last
read it, with the time of that reading, or `unknown` while it holds no reading; never `off` by
default. The Runtime adds no poll: it keeps, in memory with the registered instance, the reading
it already takes from `MuMuManager info` at the start's discovery and on-demand discovery and at
each emulator control; [emulator-control.md](emulator-control.md) ("Client and CLI") and
[runtime-interface.md](runtime-interface.md) are the contract. No persisted record, fact,
configuration key or outcome code changes.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| N1-1 | `RuntimeInstanceStatus.emulator_state` (`EmulatorRunningState`): `{state: "on" \| "off", observed_at_unix_ms}` or `{state: "unknown"}`, `deny_unknown_fields` | new member of the status projection, always serialized (runtime-client wire; read back as `unknown` when absent) | 九处-1 | 九处-1 (host 101) | the reading: the start's discovery (bound to its port `on`, bound pending `off`, at the start's registration), each (re)binding after a confirmed start or stop and the state read after a failed control (`unknown` when nothing settled after the vendor command ran), an on-demand `DiscoverInstances` (an unlisted instance `unknown`); an instance that is not discovery-bound stays `unknown` |
| N1-2 | `emulator_state` on each `ac_overview` / `actingctl overview` instance row | output member (apps 41 parity, apps 69) | 九处-1 | 九处-1 | the status' member copied; never cut by MCP's output budget |

### 1.92 Wave 18 slice nine6: a pool nothing publishes gets a tool warning (#390, Alice's nine additions of 2026-10-10, item 6; wave-18 ruling 5)

`actinglab resource validate`, the tool that already reads a scheduling `pools.json` (and the
operations' readings and adjustments), warns about a `ledger_fact` pool that nothing in the
repository publishes. It writes no ledger record, wire shape or configuration key; its one new
shape is a member of an operation output (NN6-1).
[resource-declarations.md](resource-declarations.md), "A pool nothing publishes", is the
contract. The actinglab code `resource_pool_producer_missing` (warning, ladder class `none`, in
actinglab's domain `resource_`) is a catalog entry, not a row (§7); no game content enters the
check, which compares keys and scopes only.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| NN6-1 | `actinglab resource validate` answer (`actinglab.resource-declarations.v1`): new member `warnings: [{code: "resource_pool_producer_missing", path, json_pointer, fact_key}]`, empty when there is none | operation output (no persisted record; an actinglab result, read by a person or a resource repository's CI job) | nine6 | nine6 | one element per `ledger_fact` pool of a valid pools document the run read that is not of instance scope or whose key no reading mark (`fact_key`, `capacity_fact_key`) or adjustment of an operation `task.json` of the repository (outside `.git` and the excluded material) writes; `path` is the document, `json_pointer` `/pools/<i>`; the document's entry stays `valid` and the command succeeds (exit 0); a `static_snapshot` pool is not checked; the same warning is a `WARN actinglab:` developer line on stderr |

### 1.94 Slice SEL-L4b-T: `expect_identity` reads template identities (#308, SEL L4b-T model row of 2026-10-17; forced-compromises mapping R-07; wave-17 acceptance ruling 1)

SEL-L4b-T gives `expect_identity` a second path besides the OCR `target_id` of §1.85 (row
L4B-1): a pool of template targets of the pack in one rect, the destination's identity region.
On every destination frame each entry passes by its own target's declaration, admitted with
REC-7's floors; exactly one business ID among the passing entries is a reading, none is
`absent` and two or more are `unknown`. The path reads a choice of either recognition kind, so
a select whose identity is told by icons can now bind its destination to its choice.
[selection-graph.md](selection-graph.md) (sections Declaration and Expected identity) and
[business-identity-consensus.md](business-identity-consensus.md) are the contract. It adds no
code, key, token, location or configuration key: the failures are §1.85's
`selection_identity_mismatch` at `select_expect_identity`, and `absent` is a token the
`identity_state` vocabulary already holds; the catalog changes the descriptions of the code and
of that token only. The admission refusals reuse `contained_task_select_invalid` with new detail
tokens. The resource side's VAL line (a live select pack binds its destination to the chosen
identity through title templates it already ships; one natural selection on the device passes
the step) is not part of the merge gate.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| L4BT-1 | package format: an operation's `expect_after.expect_identity.templates`, an array of `{id, target_id}` | new optional member (resource file format, not a ledger shape; task schemas unchanged) | SEL-L4b-T | resource side | the declaration gate: `expect_identity` holds `choice_of` and optionally `target_id` and `templates`, nothing else (`UnknownField`); each entry is `{id, target_id}`, both strings (`InvalidType` at `…/templates/<j>/<member>`). Admission (`contained_task_select_invalid`): the `choice_of` step has an identity feature (`choice_of=no_identity`); exactly one of `target_id` and `templates` (`expect_identity=target_and_templates`, `=neither`); the text path as row L4B-1; the pool holds 1..=16 entries (`templates=count`), each a template target (`template=not_template`) named once (`template=duplicate`), with an `id` of the step's identity domain (`template=out_of_domain`), an effective threshold above 0 (`template=threshold_zero`) and a `rect` region (`template=region_not_rect`), one rect for all (`template=regions_differ`) |
| L4BT-2 | `selection_identity_mismatch` with `identity_state` `absent` | new value combination of an existing record (shape unchanged) | SEL-L4b-T | SEL-L4b-T | a template `expect_identity` on whose last destination frame before `expect_after.timeout_ms` no pool entry passed; `expected_identity` the chosen ID, no `observed_identity`; `unknown` keeps its text-path meaning and also covers two or more business IDs passing |

### 1.95 Slice A8b: the v0.12.0 tag-time slice (#378, model-378 §5.1, §5.3, §9 G10; A8 ruling 1 of 2026-10-17)

A8b brings the three items A8 left for the tag (A8's PR #899, "Not done"): the read command
`actingledger codes`, the release snapshot `released/0.12.0.json` and the guard check G10. It
writes no ledger record, wire shape or configuration key and changes no interface revision of
§6. [runtime-outcomes.md](runtime-outcomes.md) ("Release snapshot", "Reading the catalog:
`actingledger codes`" and the G10 row) is the contract. Its catalog changes are entries, not
rows (§7): the actingledger location `read_outcome_catalog`, and the codes
`outcome_code_unknown` and `outcome_catalog_unreadable` of §1.42, which move from actingctl's
fragment to the contract's (`SharedCode`, spec D3, because actingledger emits them too) with
their spelling, category, ladder class, values and review unchanged.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A8B-1 | `actingledger codes [--code <code>] [--json \| --markdown]`: the default form one tab-separated line per code (code, category, ladder_class, review, status, description); `--markdown` the catalog's SHA-256 line and a table of the same columns; `--json` one line `actingcommand.actingledger.codes.v1` `{schema_version, catalog_sha256, codes: {name: entry}}` | read output (offline; no persisted record, no state root) | A8b | A8b | codes in name order, or the one `--code` names (`outcome_code_unknown` otherwise); each entry as the embedded catalog holds it; `catalog_sha256` is the SHA-256 of the embedded `contracts/outcome-codes.json` (model-378 §5.3 "Version": the release half lives only in the tag); a failure is one `actingledger: <code> during <location>: <text>` stderr line and exit 1 |
| A8B-2 | `contracts/outcome-codes/released/0.12.0.json`, `actingcommand.outcome-codes.release.v1` | release snapshot (contract file, written once by the release PR) | A8b | A8b | every code (with `category`, `ladder_class`, `review`, `status`, `values: {key: {type, required}}`) and every location (`review`, `status`) of the catalog at this commit, retired ones included; G9 compares every later catalog with its settled entries; never edited afterwards (spec §11) |
| A8B-3 | `outcome-guard release <version> [<root>]` | guard command of the release build (no persisted shape) | A8b | A8b | G10 for `X.Y.Z` or `X.Y.Z-rc.N`: settled entries have their text; `X.Y.Z` needs `released/X.Y.Z.json`, and from v0.13.0 on no pending code or location (a final of the 0.12 series may carry pending entries: Alice 2026-10-19, the 0.12 series releases plain Z versions, and the coordinator's decision of that day moves the boundary from v0.12.1 to v0.13.0); a release candidate needs only the text; the exact-SHA build runs it before it builds when it releases (`windows-remote-build.yml`, step "Outcome catalog release gate (G10)") and a violation stops the release before the tag exists |

### 1.96 Slice WD-1: the watchdog supervises a single-location root (#390 WD-1; deploy plan Q1 (b); Alice 2026-10-16)

From v0.12.1 `actingctl watchdog` also supervises a single-location root: a root without an
install selection whose `runtime\BUILD-MANIFEST.json` is a file, configured by
`<root>\actingd.config.json` (the root config `packs` and `mcp-serve` already read). An A/B
selection wins when a root has both; a root with neither stays `watchdog_layout_unknown`. The
decisions, the budget, the grace, the holds, the start methods, the task, the launcher and
`state.json` are unchanged; a single root's start runs `<root>\runtime\actingcommand-actingd.exe
--config <root>\actingd.config.json` in `<root>`, the logon script's command line.
`actingctl start` and `restart` without `--config` still need an A/B selection.
[runtime-watchdog.md](runtime-watchdog.md) ("Layout", "Observation" item 5, "Start", "Task",
"Exit codes") is the contract. WD-1 adds no code, key, location or configuration key; its
catalog change is the text of `watchdog_layout_unknown` (an entry, not a row, §7). It changes no
interface revision of §6: the watchdog report and `state.json` are no declared interface, and the
report's change is additive. A recorded or arbitrary `--config` path is not part of it (coordinator
decision 5 of 2026-10-18: B).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| WD1-1 | `actingcommand.watchdog.report.v2` (O5): new data members `layout` (`"ab"` \| `"single"`) and `config` (the plain path of the config the fixed entry gets) on every tick report (`status`, `run-once`); `generation` and `slot` are null for a single-location root | additive report members (stdout; schema name unchanged; coordinator decision 6 of 2026-10-18) | WD-1 | WD-1 | an A/B installation keeps its `generation` and `slot`; `config` is the selected generation's config there; a report without tick data (`install`, `uninstall`, the usage exit, a misconfiguration found before the installation resolved) has neither member |
| WD1-2 | `state.json` v2 (O6), the task definition and the launcher | unchanged | WD-1 | — | a single-location root's start records omit `generation` (already optional); its `watchdog_started_runtime` and `watchdog_start_failed` log lines carry no `generation` field; a deploy or successor script that replaces `runtime\` or `tools\` of a single-location root holds `install\writer.lock` exclusively, or disables the task, while it does (coordinator decision 5 of 2026-10-18), so the launcher runs nothing and a tick is `watchdog_installer_busy` (row 7a) |
| WD1-3 | §6 interface revisions | no interface revision | WD-1 | — | `component-interfaces.json` is unchanged |

### 1.97 Slice A5: `actinglab package check` (#411 A5; M-PKG §3.2, S5-T4; UIM :489)

A5 writes the command the interface promised (runtime-interface.md, `ac_pack_check` and the
actinglab table) and points `ac_pack_check` at it, the one `package check` piece of U-3 (the
rest of U-3 stays later). No record type, ledger member, configuration key or outcome code: the
rows are a tool input and output and an MCP answer. No interface revision of §6 (as NT3).
[package-reference.md](package-reference.md) ("Consumers"),
[actinglab-dry-run.md](actinglab-dry-run.md) and
[actinglab-capabilities.md](actinglab-capabilities.md) are the contract.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| PC1 | `actinglab package check --package <path>` answer `{status: "admitted", package_id, entry_task_id}` (`PackageCheckResponse`) | new command and output shape | A5 | A5 | the Runtime's structural admission, `PreparedContainedTask::describe_path` with the 60 s budget of pack admission, so it admits exactly what check-config and the daemon admit and a declared OCR model need not be installed; both names from the descriptor (`TaskPackageDescriptor`, unchanged); a refusal is `package_invalid` (exit 2) with `details.code` the admission's own code and `details.declaration` (`null` unless `resource_declaration_invalid`); reads only (dry run `no_effect`); capability `package check` (`offline`, `read_only`); a flag other than `--package`, a positional argument or a missing `--package` is `validation_failed` (exit 2); a relative path is read below the working directory, as every `--package`; `package preflight` is unchanged |
| PC2 | MCP `ac_pack_check {package}` → `{status, package_id, entry_task_id}` | changed answer (was `{preflight}`, P4-5) | A5 | A5 | runs `actinglab --json --via mcp package check --package <package>` and answers its data verbatim; tier (observer), input schema and the one overflow rule unchanged; a refusal is `lab_tool_failed` caused by `package_invalid` and by the details as `foreign_other_error`; `ac_binding_draft` keeps running `package preflight` |

### 1.98 Slice A7: the overview shows the last backend self-check; a failed capture open in a preparation is recoverable (#411 A7)

A7 answers the 10-18 incident (#411 comment 6251046694): with the emulator windows closed by
hand and adb up, every self-check failed its capture open (`capture_backend_open_failed`,
`nemu_ipc`, no diagnostic category, confirmed disposal), while `actingctl status` still showed
`emulator_state on` (correct: it is the running state, N1-1) and no ladder started although
stuck recovery was on. It writes no ledger record, fact, wire shape, configuration key or
outcome code and changes no interface revision of §6: `actingctl status`,
`RuntimeInstanceStatus` and `emulator_state` are unchanged (a status member is a runtime-client
wire change, a later release). [runtime-interface.md](runtime-interface.md) (the `ac_overview`
row) and [emulator-control.md](emulator-control.md) ("Client and CLI"; "Preparation stages")
are the contract. A fresh-backend capture failure on the task path (#316-P4,
[application-lifecycle.md](application-lifecycle.md)) is not changed here; §1.106 (A7T-1)
rules it.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A7-1 | `backend_selfcheck {<entry>: {status, checked_at_unix_ms}}` on each `ac_overview` / `actingctl overview` instance row | output member (apps 41 parity, apps 72) | A7 | A7 | one entry per backend entry (`input`, `capture`, `nemu`) whose `backend.selfcheck.<entry>.status` fact the instance holds, its value verbatim (`passed`, `failed`, `unknown`) with `.checked_at_unix_ms`; read from the RuntimeFactSnapshot the overview already reads, nothing added to the facts; an absent or invalidated entry is omitted, `{}` while none was checked, the member absent only when the snapshot read failed; it changes only when an open is recorded (no poll); never cut by MCP's output budget |
| A7-2 | the preparation's recoverable check | rule (no shape change) | A7 | A7 (host 105) | a failed capture-backend open (`capture_backend_open_failed`) with confirmed temporary-resource disposal, a successful cleanup and no failed input-parameter observation is recoverable whatever its diagnostic category; the other categories' rule is unchanged; which codes start a ladder stays the configuration's (`preparation_backend`); an unconfirmed disposal (a Nemu worker that timed out) stays outside it |

### 1.99 Slice A4 (v0.12.1): `actingledger events` reads only its range (#411 A4; coordinator ruling (a) of 2026-10-18)

On an SQLite root the offline `actingledger events` page and `export --performance` open the
ledger with a range opening (`GlobalLedger::open_range`), not the whole opening: the head, the
rows of the range and, for `events`, the eviction rows of the page's artifacts, each checked on
its own. [ledger-store.md](ledger-store.md) and [global-ledger-query.md](global-ledger-query.md)
are the contract. It adds no code, location, key, wire or ledger shape, and the reports are
unchanged on a valid ledger; the refusals keep the ledger's existing codes.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A4-1 | the `events` report's `artifact_evictions` (and each event's eviction proofs) | read behaviour of an existing member (shape unchanged) | A4 | A4 | built from the `artifact.eviction_intent` and `artifact.eviction_outcome` rows after the last row read, each checked on its own, not against a whole-ledger retention replay; each proof carries the head of the read transaction, as before, so `--through` below an eviction still lists its proof. `open`, `chain` and `export` keep the full verification, and `chain` is the full check; a segment root is read whole |

### 1.100 Slice A8 + A14 (v0.12.1-rc.2): MCP names the missing package and the doubled tap; `fields` says what it does (#411 A8, A14)

MCP `ac_lab_do` and `ac_lab_observe` refuse, before the install is located and before any
actinglab child runs, two calls they can already tell are wrong, and name the argument; the
tool descriptions say which arguments a capture needs. A tap without a package is not made
possible: every online Lab request carries its package
([contained-lab-operation.md](contained-lab-operation.md)). No code, wire, ledger, configuration
or interface revision (§6) changes: the refusals use the existing `arguments_invalid` with its
`argument` / `argument_problem` values (apps 73).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A8-1 | `ac_lab_do` / `ac_lab_observe` with `capture: true`, `dry_run` not `true` and neither `package` nor `zip` → `arguments_invalid {argument: package, argument_problem: required}` (class `usage`) | changed answer (was `lab_tool_failed {exit_code: 2}` caused by `package_invalid`) | A8 | A8 | the condition is actinglab's own branch to `PackageInput::open` (do: `--capture` without `--dry-run`; observe: `--capture`); no child, nothing recorded (no `client.action`); with `package` or `zip`, without capture, or with `dry_run` the call runs actinglab as before (both locators together stay actinglab's `validation_failed`); the descriptions say "package or zip is required" (do: without dry_run) |
| A8-2 | the `fields` argument of `ac_lab_do` and `ac_lab_observe` | description only (no behaviour change) | A8 | A8 | the members, comma separated, that the Min answer keeps whole (never removed or shortened) when it is reduced to its byte budget; it selects nothing; on observe the named ones of the members Min leaves out (`instance`, `arbitration`, `matched`, `standby`, `frame_age_ms`, `frame_source`) are kept; still passed to actinglab as `--fields` (CLI parity) |
| A14-1 | `ac_lab_do` with `target: "tap"` and `tap` → `arguments_invalid {argument: tap, argument_problem: conflicting}` (class `usage`) | changed answer (was `lab_tool_failed` caused by `validation_failed`) | A8 | A8 | `target` is a Runtime element id; the description says the action takes exactly one of `target`, `tap` or `swipe`; no child, nothing recorded |

### 1.101 Slice A9 (v0.12.1-rc.2): offline `observe` of an unknown page answers `unknown` in Min (#411 A9)

When no page matches, offline `observe`'s near-miss `candidates` are a transport member, not
the identity, so Min cuts them to fit its hard limit instead of failing with
`observe_identity_limit_exceeded`. [page-projection.md](page-projection.md) ("Shared input and
output") is the contract. It adds no code, wire or ledger shape; the pending description of
`observe_identity_limit_exceeded` says the code now means the identity itself does not fit.
An answer that succeeds today is unchanged, and the Normal view (`--verbose`) keeps the whole
list.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A9-1 | `candidates_truncated: true` and `candidates_omitted_count` beside `candidates` in offline `observe`'s Min answer | two optional output members (exec 86) | A9 | A9 | present only when Min removed near-miss entries: after every existing reduction (fact rows, projection entries, the identity alone) the answer is still over the hard limit, so Min removes `candidates` entries from the end (sorted by score, the lowest last) one at a time until it fits, and the suspicion's copy of the list with them; `candidates` stays an array; the count is the entries removed; both members are kept by Min like `candidates`; with every entry removed and the identity still over the limit, `observe_identity_limit_exceeded` as before |

### 1.102 Slice A10 (v0.12.1-rc.2): an undecodable Lab recording makes only the Lab part unavailable (#411 A10; coordinator decision 2 of 2026-10-19)

A `recording.json` that was read and does not decode as `actingcommand.lab-recording.v1` is
treated as an identity mismatch is: the Lab part is unavailable and the session answers on.
[lab-recording.md](lab-recording.md) ("Files", "record status") is the contract. It adds no
member, code or command, and changes no interface revision of §6; the file is never repaired,
rewritten, moved or deleted and is not decoded leniently.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A10-1 | `lab.reason` of `record status` / `ac_record_status` gains the value `lab_recording_unreadable` | new value of an existing member (shape unchanged) | A10 | A10 | `lab: {status: "unavailable", reason: "lab_recording_unreadable"}` with the session's `record`, exit 0, where the answer was `record_lab_unavailable` (exit 3); step actions refuse with `record_lab_unavailable` and that reason, and `record stop` stops the session with no Lab output, both as for `lab_recording_mismatch`; a read error other than a missing file stays `record_state_io_failed`; the reason stays a Lab-crate string until B4 (exec 85) |

### 1.103 Slice A12 (v0.12.1-rc.2): the Lab's OCR loader reads a single-location root's configuration (#411 A12 part 1)

From v0.12.1 actinglab and the Lab, started outside an install selection from a single-location
root (the WD-1 layout of §1.96: `runtime\BUILD-MANIFEST.json` is a file two levels above the
executable, and that folder is no `A`/`B` slot folder), load the OCR provider over the vision
root that `<root>\actingd.config.json` names (`vision.root`, else `<root>\vision`).
[lab-recording.md](lab-recording.md) ("OCR in the Lab") is the contract. A12 adds no code, key,
option, environment variable, location or shape and changes no interface revision of §6;
`process_installation` and actingctl are unchanged, and so is which command refuses without OCR
(part 2 of the report is a B item, coordinator decision 3 of 2026-10-19). Its catalog change is
the text of `lab_ocr_unavailable` and `lab_ocr_vision_unconfigured` (entries, not rows, §7).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A12-1 | the configuration the offline tools' OCR loader reads on a single-location root | rule (no shape change) | A12 | A12 (exec 87) | an A/B installation's selected configuration and a refused installation are unchanged; a single-location root's configuration that is missing, unreadable, no regular file or over 1 MiB is `config_read_failed`, one that does not decode `config_decode_failed`, each the cause of `lab_ocr_unavailable` and never `lab_ocr_vision_unconfigured` (coordinator decision 6 of 2026-10-19); outside both an installation and a single-location root the cause stays `lab_ocr_vision_unconfigured` |

### 1.104 Slice Lab L2 (v0.12.x): the Lab loads every click kind the Runtime runs; offline `observe` projects page-set operations (#411 Lab L2, with A13's A part)

Alice's principle of 2026-10-19 (「Lab的资源制作应当包含所有可用后端的识别以及触控」): a package the
Runtime runs is a package the Lab can load. Both navigation readers read the segmented drag
`single_touch_drag_with_vertical_brake_v1`, so a package holding one is no longer refused by the
online observation (and so by `observe --capture` and `do --capture`) or by actinglab's
`observe`, `do`, `ensure` and `wait`; the Lab lists the gesture and does not perform it. A13's A
part: offline `observe` puts an operation whose `to` is a page set into `observation.elements`,
as L2-7 (§1.74) and the online observation already do. [resource-declarations.md](resource-declarations.md)
(the navigation paragraphs) and [page-projection.md](page-projection.md) ("Shared input and
output") are the contract. One new outcome code; no persisted ledger shape, no wire or record
shape, no configuration key and no interface revision of §6 changes. Executing the gesture from
the Lab (B-1, B-3) and `input` on `actions[]` page-set entries (A13's B part) are not in this slice.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| LL2-1 | the click `{"kind": "single_touch_drag_with_vertical_brake_v1", "from_rect", "corner_rect", "horizontal_duration_ms", "corner_hold_ms", "brake_distance_px", "brake_duration_ms"}` of a navigation edge (to a page or a page set) or a page operation, read by the kernel's drive reader and actinglab's navigation reader | new reading rule of an existing resource format (`navigation`, schema `0.6` unchanged) (exec 90) | Lab L2 | — (source compilation writes it) | its shape as pack containment checks it: exactly these members, two rectangles of integer `x`, `y`, `width`, `height`, an integer `brake_distance_px`, unsigned durations; another form keeps the reader's refusal (`drive_plan_invalid`, actinglab's usage refusal, `observation_resources_invalid`); was `unsupported navigation click kind`; no route of the drive graph or of actinglab's navigation tools passes it; as a destructive page operation it covers `from_rect`, `corner_rect` and `corner_rect` moved up by `brake_distance_px` |
| LL2-2 | `gesture_not_projectable` (`error`, `ladder_class` `none`, `since` 0.12.3, pending), a `blocked_reason` of the page projection | new outcome code, new value of an existing member (page projection shape unchanged) (exec 90) | Lab L2 | Lab L2 | the element of a segmented drag in the online observation and actinglab `observe`: `recognized: true`, `recognition_basis: matched_page_declaration`, `actionable: false`, `input: null`; `do <element-id>` on it is `lab_element_unavailable` as for any element that is not actionable; the online observation checks its two boxes against the coordinate space, as a drag's; its class is that of its siblings (a projection reason, row 7) |
| LL2-3 | offline `observe`'s `actions` entry of an edge whose click is a segmented drag | new value of an existing member (`input: null`) | Lab L2 | Lab L2 | `{id, from_page, to_page, input: null, source}`, the edge entry's members; every other entry unchanged |
| A13-1 | offline `observe`'s `observation.elements` include each operation from the observed page whose `to` is a page set | rule (no shape change) (exec 90) | Lab L2 | Lab L2 | from the generated navigation row (`to_pages`, its compiled `click` and `source`), resolved as an edge is: a `point` click a tap of its 1×1 box, a `rect` or `specific_rect` click its box and centre, a `target` click on the frame (`target_not_matched` with no input when it fails), geometry outside the frame `invalid_frame_geometry`; a segmented drag as LL2-2; the online observation already projects them; `actions` is unchanged; the projection's limits apply as to any element, so Min may keep a different element for such packages |

### 1.105 Slice A16 (v0.12.1): an entry recovery package that cannot end on the entry is refused before it runs (#411 A16)

NAV-3b's rule (A1) is unchanged: an entry recovery package must end on a page of the required
entry set, and the registry's `linear_steps` return-home package is not compared. A16 decides the
case that is certain before the package runs. [README.md](README.md) (entry page sets) and
[linear-steps.md](linear-steps.md) ("Ladder and page-graph home entry") are the contract. No
record type, ledger member, key, configuration key or wire shape changes; which runs go to entry
recovery is unchanged.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A16-1 | `contained_task_home_recovery_terminal_outside_entry` (`error`, `ladder_class` `pack_logic_mismatch`, `since` 0.12.1, pending) with `page_ids` (required), `package_id` (required), `page_id` (optional) | new outcome code and an earlier refusal (host 107) | A16 | A16 | after the recovery package's existing checks and before `EntryRecoveryPackageAdmitted`: when none of the package's declared final pages (its `target_page` or target page set, resolved to its pages) is a page of the required entry set, the run fails closed with no input, no `EntryRecoveryPackageAdmitted` and no `EntryRecoveryCompleted`; the fail-closed `entry_target_disposition`, the terminal and the receipt carry the values (`page_ids` the required entry pages as recorded, `package_id` the recovery package, `page_id` its declared final page when it declares exactly one); the registry's `linear_steps` return-home package is not compared and still runs to the recheck; a package with a declared final page in the set runs as before and A1's `contained_task_home_recovery_terminal_non_home` still fails it when it ends elsewhere; the class is the one this case gave until now (A1's code) |

### 1.106 Slice A7T (v0.12.2): a contained task's capture failure is that instance's failure, not the Runtime's (#411 A7 task path; Alice 2026-10-19, pending item 13 = A)

A7 parts 1 and 2 (§1.98) left the task path's capture failure to a ruling. Alice ruled that an
instance whose capture cannot be opened during a task fails that task, is marked abnormal and
goes to the recovery ladder, which restarts that emulator, while the other instances go on;
only a broken capture service as a whole is fatal. "The capture service as a whole" is every
cause outside the instance's four capture codes with confirmed disposal (coordinator decision 1
of 2026-10-19). [application-lifecycle.md](application-lifecycle.md) (the contained task's
capture), [emulator-control.md](emulator-control.md) (the defaults) and
[runtime-config.md](runtime-config.md) are the contract. No record, wire shape, configuration
key, outcome code or interface revision of §6 changes; the four codes' pending catalog text is
written in full (`contract.json`, category, `ladder_class` and values unchanged).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A7T-1 | a contained task's capture failure of the instance | rule (no shape change; replaces §1.98's "waits for a ruling") | A7T | A7T (host 108) | after the unconfirmed-resource settlement answered confirmed, a failure whose kernel code is `capture_backend_open_failed`, `paired_backend_open_failed`, `capture_backend_operation_failed` or `capture_frame_invalid` fails only that run, on every capture of the run and every execution provenance: the receipt is `Failed` with the kernel code, projected as a nonfatal `capture_failed`; `capture.failed`, then `task.failed` (not severe, its severity the code's category) and the lease released; the host is not fatal and later requests are not answered `runtime_halted`; a failed open has written the instance's `backend.selfcheck.capture.status = failed`; any other code and resources released unconfirmed that no registered instance owns keep their fatality |
| A7T-2 | the shipped trigger item `task_capture_backend` | default value (no new key) | A7T | A7T (host 108) | `{"id": "task_capture_backend", "stages": ["task"], "codes": [the four codes]}` after `preparation_backend` in `actingd.defaults.json`, with the trigger defaults (`immediately`, rung 1, cool-down respected); a scheduled run on an instance with stuck recovery on that fails by A7T-1 starts a ladder by it; with the capture self-check failed its `return_home` and `application_restart` rungs are skipped `capture_unavailable` and `emulator_restart` is the first rung tried; a direct run starts none (`rules.direct_runs = exempt`) |
| A7T-3 | the first capture's ADB probe child | rule (no shape change) | A7T | A7T (host 108) | the one ADB baseline probe of a physical instance's first capture (task deadline) still runs for the four codes; a failed probe with confirmed cleanup invalidates `device.connected`, `application.foreground` and `task.page` with `adb_unreachable` as before; a probe whose adb child is unconfirmed is settled by the #403 rule (`settle_unconfirmed_adb`): the instance is fenced and the run fails nonfatal with `instance_resources_fenced`, and only where no registered instance owns it is the owner retained and the Runtime fatal; the probe's answer no longer decides the capture failure's fatality |

### 1.107 Slice PK1b (v0.12.x): the host hands each step its instance's pacing factor, settles on budget time and records both (#385 P-k; PERF v1.4.2 §4.6, §5.3, §5.6, §10.3, §12; Alice 2026-10-19, pending item 4)

With `performance.pacing.mode = pace` (or `full`) a contained run's step waits and kernel limits
stretch by its instance's pacing factor in effect; under `observe`, the release default, every
factor is 1 000 and every wait, limit, deadline, settlement and record is byte-identical to
before. [performance-control.md](performance-control.md) ("Effect", "Step pacing") and
[runtime-config.md](runtime-config.md) (the `scheduling` section) are the contract. No interface
revision of §6, configuration key, record type or outcome code changes; the members below are
the ones SH-1 registered (P5, P6), first written here.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| PK1-1 | register P5 `pacing_factor_milli` of the task timing evidence's `steps[]` | first write of a registered member (host 110) | PK1b | PK1b | one entry per step index whose factor was above 1 000, `pacing_factor_milli` the factor the step started with; the attempts of one step share its entry; entries in step order (strictly increasing `step_index`); no entry for a step at 1 000, so `steps` stays absent under `observe`; `page_settle_ms`, `pacing_band`, `pressure_level` and `extra_wait_rounds` are not written here |
| PK1-2 | register P6 `wall_elapsed_ms`, `budget_elapsed_ms` of the policy execution record, and the settlement rule | first write of registered members and a rule (host 110) | PK1b | PK1b | the host keeps a run's factor segments (one `step_pacing()` answer to the next or to the end of its execution); budget time = wall time − the sum over its segments of segment wall × (f − 1000) / f; a policy run with a segment above 1 000 is compared with its admitted runtime budget (`PolicyBudgetReceipt::runtime_exceeded`) on its budget time and its record carries `wall_elapsed_ms` (the wall time, as `runtime_ms`) and `budget_elapsed_ms` (never above the wall time); a `policy_runtime_budget_exceeded` rewrite records the number compared as its `runtime_ms`; the host's replay, its recovery check and the ledger's settlement check compare `budget_elapsed_ms` when the record carries it, else the wall time, and the ledger's recovered draft of a record that carries them carries them again; a run settled from its terminal after a restart is settled on its wall time; every other run's record has neither member |
| PK1-3 | `StepPacing.factor_milli` (the kernel seam, PK1a) | in-process value (no wire) (host 110) | PK1b | PK1b | `step_pacing()` (and through it entry recovery's) hands `StepPacing::new(retries).with_factor_milli(f)`, `f` the instance's `effective_factor_milli` of the shadow measurement's last evaluation, read under the performance lock alone (a poisoned lock is fatal): 1 000 under `observe`, before the first evaluation, without counters and for an instance without a row; the curve's factor (pressure floor included) under `pace` and `full`; the host's deadline, lease TTL, active checks and `contained_task_deadline_exceeded` stay wall-clock at every factor; dispatch, shares, holds and the seven-level controller are unchanged |

### 1.108 Slice PK2 (v0.12.x): the stage's extra wait rounds, extra resamples and the pre-input wait, never an extra click (#385 P-k S5; PERF v1.4.2 §4.1, §4.4, §5.2, §5.4; Alice 2026-10-19, pending item 4)

With `performance.pacing.mode = pace` (or `full`) a contained run's step also takes its stage's
extra wait rounds and a pre-input wait; under `observe`, the release default, no round or wait
is added and every wait, input, limit and record is byte-identical to before.
[performance-control.md](performance-control.md) ("Step pacing") is the contract, with
[business-identity-consensus.md](business-identity-consensus.md),
[policy-suspension.md](policy-suspension.md) and [runtime-config.md](runtime-config.md) (the
`performance.pacing.*` row). No interface revision of §6, configuration key, record type or
outcome code changes; the member below is the one SH-1 registered (P5), first written here, and
`pre_input_wait_base_ms` is an existing key, now read.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| PK2-1 | register P5 `extra_wait_rounds` of the task timing evidence's `steps[]` | first write of a registered member (host 111) | PK2 | PK2 | the rounds the step's waits started (rounds before the run's first step count for that step), written when at least one; an entry exists for a step whose factor was above 1 000 (PK1-1) or that used a round, `pacing_factor_milli` absent at 1 000; a step at 1 000 that used none has no entry, so `steps` stays absent under `observe`; the attempts of one step share its entry, in step order |
| PK2-2 | the extra-round rule | rule (no shape change) (exec 92, host 111) | PK2 | PK2 | the shadow measurement keeps per instance the lower curve neighbour's `extra_retries` (under `linear` and `step`), at least `pressure_floor.extra_retries` under an active floor pressure at or above `min_severity`; in effect 0 under `observe`, the reading under `pace` and `full`; `step_pacing()` hands it as `StepPacing.extra_wait_rounds` and `sample_timeout_retries` = the configured base + it (§5.4 item 2); a wait without an input (page wait, post-input confirmation, entry waits, observation window, `linear_steps` transition and arrival waits) that reached its limit on the budget clock without an error page or a terminal page starts one more round of the same length and interval, captures only, at most that many times, while the task's budget is not spent, then fails as before; the post-input confirmation's rounds precede the failure decision; the failure decision's confirmation and a `linear_steps` retry decision take none; inputs never exceed those at 1 000 with no round (`max_attempts` and every clicking retry unchanged); an entry check takes its own `step_pacing()` answer; no status, summary or record shape carries the rounds in effect |
| PK2-3 | the pre-input wait | rule; existing key `performance.pacing.pre_input_wait_base_ms` now read (exec 92, host 111) | PK2 | PK2 | `StepPacing.pre_input_wait` = `pre_input_wait_base_ms` × (f − 1000) / 1000 (zero at 1 000), the base read with the `performance` section at start-up and only for a step above 1 000; on the page-graph and `linear_steps` step paths the kernel pauses it as given after the action and its sampling check, before `EffectIntent`, on the budget clock (a task budget it spends ends the run `contained_task_timeout` before the effect intent); nothing is recognized again after it; at 1 000 no pause and the order unchanged; a scan's drags take none; the key's row and `pressure_floor.extra_retries`' row name P-k as reader |
| PK2-4 | paced waits end at the host deadline | rule (no shape change) (host 111) | PK2 | PK2 | once a run had a factor segment above 1 000, every wait of the kernel (`ContainedTaskRuntime::pause`: post-input and retry delays, capture, arrival and confirmation waits, sampling intervals and resamples, the pre-input wait, extra rounds, entry and `linear_steps` waits, observation windows) is cut by the host to the Runtime-clock time left before the run's host deadline, so the run's next check ends it `contained_task_deadline_exceeded` (a direct run cancelled `deadline_exceeded`, a scheduled run through its failure settlement) inside the heartbeat reserve, while its lease still holds: the lease lasts the reserve past that deadline and is not renewed while the run runs (direct, scheduled and host package runs alike); a run whose every factor is 1 000 waits exactly as before and reads no clock for it; no code, key, record or wire changes |

### 1.109 Slice A19 (v0.12.2): a timed-out arrival wait answers "not reached" in its own code; a cut takes samples only (#411 A19)

actinglab answers an online `do --capture --after-page` whose arrival wait times out as it
answers a navigation step whose page did not arrive: `lab_arrival_timeout`, exit 3, instead of
`device_error`, exit 4. The call stays a failure with a nonzero CLI result
([contained-lab-operation.md](contained-lab-operation.md), "Timeout and abort … produce a failed
receipt and nonzero CLI result"). No code, key, wire, ledger, configuration or interface
revision (§6) changes; the catalog change is the text of the pending `lab_arrival_timeout`. Every
other failed record, and the `--record` path's handling of a failure, keep their answers.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A19-1 | online `do --capture --after-page` whose wait times out → CLI error `lab_arrival_timeout`, exit 3, `blocked_by: [arrival_page]`, details unchanged; MCP `ac_lab_do` class `safety`, `lab_tool_failed {exit_code: 3}` caused by `lab_arrival_timeout` | changed answer (was `device_error`, exit 4; MCP class `device`) (exec 93) | A19 | A19 | chosen by the failure's stage `arrival` and code `lab_arrival_timeout`; the details are the operation's answer as before, `after_attachment` included; `lab_element_unavailable` stays `capability_insufficient` (exit 3) and any other failure `device_error` (exit 4); a script that tests exit 4 on a timeout reads exit 3 |
| A19-2 | every online `do` answer has `arrival` as its last member | member order (no member added or removed) (exec 93) | A19 | A19 | success and failure, Min and Normal: `arrival` moves after `after_attachment` (and after `record` on a `--record` answer); its own members stay `status`, `elapsed_ms`, `samples`; every other member keeps its order and value; so a reader that cuts the text, such as MCP's `raw_text` (4096 bytes), keeps `state`, `failure`, `after_attachment` and the arrival's status and elapsed time, and loses samples only |

### 1.110 Slice R0 b15d (v0.12.3): "needs further processing" with declared remedies, its contract text and codes ahead of their code (#411 B15; model-b15; coordinator decisions of 2026-10-19)

A mapped named outcome key whose mapping declares a `remedy` is "needs further processing": the
run is a success with that key, and the scheduler holds the blocked task, requests an allowed
provider, retries once, follows a chain of at most three layers and stops for a person when a round
is used up. Needs, capabilities, parameters and options are opaque tokens the packages declare;
the Runtime pairs and orders nothing. [remedies.md](remedies.md) is the contract, with its parts in
[resource-declarations.md](resource-declarations.md), [package-reference.md](package-reference.md),
[scheduling/README.md](scheduling/README.md), [scheduling-blackout.md](scheduling-blackout.md),
[policy-suspension.md](policy-suspension.md), [instance-fact-store.md](instance-fact-store.md),
[runtime-interface.md](runtime-interface.md), [runtime-config.md](runtime-config.md) and
[selection-policy.md](selection-policy.md). R0 writes the text and registers every code, key and
vocabulary of the train; slices R1–R7 (§1.111–§1.117) implement it. **B15 adds no persisted ledger
shape** (model-b15 §10): every carrier below is an existing record or member; the codes, keys and
vocabularies are catalog entries, not rows (§7); the package members ride `package` revision 4
(§6) and the configuration keys `actingd-config` revision 6 (§5).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B15-1 | codes `policy_remedy_waiting` (info), `policy_remedy_requested` (info), `policy_remedy_retry` (info), `policy_remedy_unchosen` (warning), `policy_remedy_unavailable` (error), `policy_remedy_exhausted` (warning), `policy_remedy_lifted` (info), `policy_remedy_expired` (warning), `policy_dispatch_parameters` (info) (owner policy); `policy_remedy_round_closed` (info), `policy_remedy_declaration_unusable` (warning), `session_remedy_choices_invalid` (error), `session_dispatch_key_reserved` (error) (owner runtime-host, registry `RemedyHostCode`); `package_bundle_remedy_invalid` (warning) (owner resource-tooling) | new catalog entries (no shape): `ladder_class` `none`, `since` 0.12.3, pending, full text | R0 | R1, R2, R4–R7 | values as [remedies.md](remedies.md) section 12 lists them; no new domain: `policy_`, `session_` and `package_` already cover the owners; no emission site in R0 |
| B15-2 | keys `need`, `capability`, `parameter_name`, `parameter_value` (token), `capabilities_allowed`, `capabilities_declared` (list of token), `remedy_source` (vocabulary `remedy_source`: `target`, `configuration`, `package`), `remedy_problem` (vocabulary `remedy_problem`: `no_choice`, `choice_none`, `choice_invalid`, `parameter_invalid`, `capability_unbound`, `cycle`, `parameters_mismatch`, `provider_unavailable`, `round_limit`, `depth_limit`, `capability_unprovided`), `remedy_depth` (integer), `decision_id` (id), `irreversible` (boolean), `remedy_inputs_digest` (hash), `run_parameters` (records of `parameter_name`, `parameter_value`, `remedy_source`) | new key-table and vocabulary entries (no shape) | R0 | R1, R2, R4–R7 | the two vocabularies are built from the contract's enums `RemedySource` and `RemedyProblem` (G7); `remedy_inputs_digest` hashes declarations and versions, never a package's content |
| B15-3 | the reasons `policy_remedy_requested`, `policy_remedy_retry` and `policy_dispatch_parameters` in the chains of `policy.dispatch_intent`, `policy.dispatch_admitted`, `policy.dispatch_rejected` and `policy.dispatch_completed` | new values of existing members (shape unchanged) | R2 | R4 | each reason persisted as its outcome (code and values), no detail; at most 16 `policy_remedy_requested` per dispatch; `run_parameters` one record per declared parameter |
| B15-4 | `policy.planning_signal_observed` of kind `goal_missed` whose outcome is `policy_remedy_unchosen`, `policy_remedy_unavailable`, `policy_remedy_exhausted`, `policy_remedy_lifted` or `policy_remedy_expired` | new values of existing members (shape unchanged) | R2 | R4 | `signal_id` `signal:remedy:<stopped\|lifted\|expired>:<hex>`, `<hex>` the lowercase SHA-256 of `task \0 instance \0 decision_id` of the round; checked against the planning-signal projection before writing, once per round across restarts; `instance_id` the alias, `task_id` the root task; selected by kind and prefix, never by outcome |
| B15-5 | `policy.dispatch_rejected` with `policy_remedy_round_closed` | new value of an existing member (shape unchanged) | R4 | R4 | an intent whose remedy round or layer closed between evaluation and admission; no lease, no run |
| B15-6 | `fact.published` of the instance fact `session.remedy.choices` | ordinary instance fact (§4 row; shape unchanged) | R6 | R6 | rows `allow`, `none`, `parameter`; scope instance, server or game; the request's origin on the event; `source_snapshot_id` `remedy-choices:<sha256 hex>` |
| B15-7 | `session.dispatch.parameter.<name>` | overlay of a run's instance fact snapshots, never stored (§4 row) | R5 | R5 | the prefix `session.dispatch.` refused to publishers (`session_dispatch_key_reserved`); `task.selection_evaluated`'s `input_sha256` covers the overlay (shape unchanged) |
| B15-8 | `config.parameters.scheduling` rows `scheduling.remedy.enabled`, `.max_per_round`, `.choices`; `config.parameters.instance` rows `instance.<id>.scheduling.remedy.choices` | new configuration rows (§5 row; no new section) | R3 | R3 | `actingd-config` revision 6, raised once in the release |
| B15-9 | `actingd suspended` (`actingcommand.actingd.suspended.v2`) top-level `remedies[{code, remedy_problem, task_id, instance_id, outcome_key, need, recorded_sequence, recorded_at_unix_ms, lifted?}]` with `remedies_total` when cut; MCP `ac_diagnose` rows `remedies` | new report and output members (not persisted; `schema_version` unchanged) | R7 | R7 | read from the B15-4 stop and lift records; an unreadable record `{parse_state: "unparsed", signal_id, code}` |
| B15-10 | `task.json` mapping member `remedy {need, capabilities, default?}`; `control.json` member `provides [{capability, irreversible?, parameters?}]` | package members (`package` revision 4, §6; not ledger) | R1 | R1 | absent members serialize byte for byte as before; refusals `invalid_scheduling_outcome_declaration` (field) and `contained_task_control_invalid` (`provides_invalid`, `provides_parameters_invalid`, `provides_own_remedy`) |
| B15-11 | the terminal's `scheduling_disposition` (`task.completed`) | read only (shape unchanged) | R4 | — | the settlement reads its `outcome_key` whatever the catalog references; a remedy key is a success: no failure identity, the streak cleared |

### 1.111 Slice R1 b15p (v0.12.3): packages declare remedies and provided capabilities; the bundle check reports remedies nothing provides (#411 B15; model-b15 §4, §13; coordinator decisions of 2026-10-19)

R1 implements §1.110 B15-10 and the report of `package_bundle_remedy_invalid` (§1.110 B15-1):
[remedies.md](remedies.md) sections 2 and 10, [resource-declarations.md](resource-declarations.md)
("Task schema `0.9` remedies and provided capabilities") and
[package-reference.md](package-reference.md) (item 5 of the cross-check). **No persisted ledger
shape** (model-b15 §10): `remedy` and `provides` are package data (`task.json`, `control.json`),
never a member of a persisted event; the bundle report is an answer member that already holds
outcomes; the codes, keys and vocabularies are R0's catalog entries. The members ride `package`
revision 4: R1 is the release's first slice that needs it and raises the Runtime's declaration
to [3, 4] (§6). No code, contract, default or test names a game, a need, a capability or an
order of capabilities; the names are compared for equality only.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| R1-1 | `task.json` `scheduling_outcome.mappings[].remedy {need, capabilities, default?}` | package member (`package` 4; not ledger) (exec 95) | R1 | R1 | `need` a token `^[a-z][a-z0-9_]{0,63}$`; `capabilities` 1 to 8 unique tokens, never `none`; `default` unique, a subset, absent or `[]` = no package default; refused `invalid_scheduling_outcome_declaration` with field `remedy.need`, `remedy.capabilities` or `remedy.default` (kernel admission: `contained_task_outcome_declaration_invalid`); a member the remedy does not have is refused when read; a mapping without `remedy` serializes byte for byte as before |
| R1-2 | `control.json` `provides [{capability, irreversible?, parameters?: [{name, options, default}]}]` | package member (`package` 4; not ledger) (exec 95) | R1 | R1 | 1 to 8 entries, capabilities unique tokens, never `none`; at most 8 parameters, at least one when `irreversible`; names unique across the whole `provides`; 1 to 16 unique option tokens; `default` one of them; refused `contained_task_control_invalid` `reason=provides_invalid` (the list, a capability, its marker, a member an entry does not have) or `reason=provides_parameters_invalid` (a capability's parameters, a shape error inside them included) |
| R1-3 | a package that provides a capability one of its own remedies names | new admission rule (exec 95) | R1 | R1 | `contained_task_control_invalid` `reason=provides_own_remedy` |
| R1-4 | the source parser (the declaration gate of every admission, the source build and the Lab's declaration check) | accepts `remedy` and `provides` (exec 95) | R1 | R1 | refuses the same violations as `resource_declaration_invalid` at the member's JSON pointer (`unknown_field`, `invalid_type`, `missing_field`, `invalid_value`; a self-provided capability at `/provides/<i>/capability` of `control.json`); its refusal comes first in admission, as for every declaration it checks |
| R1-5 | `PreparedContainedTask::read_remedy_declarations(label, locator, deadline) -> RemedyDeclarations {package_id, remedies: [(outcome_key, remedy)], provides}`; `PreparedContainedTask::declared_parameter_defaults() -> [(name, default)]` | kernel interface (not persisted) (the defaults exec 95; the reader by path host 119) | R1 | R4, R5 | the reader admits by path through the metadata-only containment without preparing the task, as `read_reading_marks`; the defaults in declaration order |
| R1-6 | `package bundle` with any root file and `actinglab standard-pack check`: `warnings` gains `package_bundle_remedy_invalid` `{package_id, remedy_problem, need?, capability?, outcome_key?}` | report entries (not persisted; answer shape unchanged) (exec 102) | R1 | R1 | after the cross-check, per game root, every registry package read by the metadata reader: `capability_unprovided` one per remedy capability no package of the root provides (with `need`, `capability`, `outcome_key`); `cycle` one per package on a cycle of remedies and provided capabilities (with the remedy leading into it); `parameters_mismatch` one per provider of a capability whose providers declare different parameters (compared by name, option set and default; with `capability`); `standard-pack check` reports them with or without `pages.json`; nothing is refused |
| R1-7 | `distribution/windows/component-interfaces.json` `package` [3, 3] → [3, 4] | interface declaration (§6; contract K27 adapted) | R1 | R1 | a revision-3 pack is still read; [component-interfaces.md](component-interfaces.md) gains the revision-4 row |

### 1.112 Slice R2 b15e (v0.12.3): the evaluator plans declared remedies (#411 B15; model-b15 §5.3, §6, §7, §8, §9.2, §11; [remedies.md](remedies.md) sections 3.3, 4, 5 and 6)

The pure policy crate plans the remedies of section 1.110: `plan_remedies` folds every round from
the pairs' journal, resolves the allowed capabilities (target record, configuration, package
default) and the run parameters, and returns per pair a trigger override (a request or the one
retry), a deferral (a hold or a stop) and the stop, lift and expiry signals;
`evaluate_with_remedies` applies the plan. The host fills the inputs and appends the signals
(R4); the overlay is R5's. **No persisted ledger shape** (model-b15 §10): the reasons ride the
existing dispatch chains (B15-3), the signals are the existing `goal_missed` planning signals
(B15-4), `PolicyEvaluation` keeps its members, and the slice adds no code, key or vocabulary token
(all are R0's, §1.110 B15-1, B15-2). Gate: policy key 52.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B15E-1 | `plan_remedies(catalog, facts, time, &RemedyInputs) -> Result<RemedyPlan>`; `RemedyInputs {enabled, max_per_round, table {remedies, providers, refused_edges}, journal, stops, signals_seen, configuration, persistent_gates}` (`crates/policy/src/remedy.rs`) | new pure interface (memory only, never persisted) | R2 | R4 | remedies off or an empty table: an empty plan; `max_per_round` outside 1..=8, a journal over 64 runs or given twice, a remedy without capabilities or a table name that is not a token refuse the evaluation (`policy_evaluation_input_invalid`); `persistent_gates` carries the host's persistent gate codes (`policy_task_paused`, `pack_admission_suspended`), since the policy crate may not spell another owner's codes (outcome guard G5); the three placement codes are the crate's own |
| B15E-2 | `evaluate_with_remedies(…, fact_refresh, &RemedyPlan, eligibility)` | new entry point (output shape unchanged) | R2 | R4 | a request or retry is triggered and past its cooldown and promoted; its reasons join the decision and the chain; a selected pair with run parameters carries `policy_dispatch_parameters` before `placement_selected`; a plan deferral applies when the callback answered `Eligible`; after the allocation a requesting layer's hold names its provider's gate, or its root stops `provider_unavailable`; with an empty plan the evaluation equals `evaluate_with_fact_refresh` byte for byte; the existing entries are unchanged |
| B15E-3 | `RemedyPlan::signals_after(&PolicyEvaluation)` | new function | R2 | R4 | the plan's signals plus the stops the evaluation decided (every requested provider behind a persistent gate), none whose id `signals_seen` holds, ordered by id |
| B15E-4 | the plan's values | rules (no shape) | R2 | R4 | a stop names the round's root task and the key and need of the layer that stopped; an expiry the top layer's; `policy_remedy_requested.count` is the request's number in the layer, `policy_remedy_retry.count` the layer's requests so far, both with `limit_count` = `max_per_round`; a hold wakes at its round's window end, a stop never |
| B15E-5 | the plan's decisions where the model leaves room | rules (no shape) | R2 | R4 | an unreadable target record or configuration list decides every need `choice_invalid`; a provider whose package's parameters do not all resolve (an undeclared parameter, two values in one source, a value outside the options) is not requestable and its dispatches carry no `policy_dispatch_parameters`; a provider held by another round makes the layer wait; nothing requestable: `parameter_invalid`, then `parameters_mismatch`, then `cycle` (a provider exists but only in the chain or on a refused edge), else the table's problem for the mapping or `capability_unbound`; a recorded stop whose round is no longer the pair's latest root is lifted; `remedy_inputs_digest` hashes the target records that apply (scope, observation, expiry, rows), the configuration rows, the root key's remedy, its capabilities' providers on the instance with their declarations, the edges refused from the root task and the catalog hash |

### 1.113 Slice R3 b15c (v0.12.3): the remedy configuration keys (#411 B15; model-b15 §5.2; coordinator decisions 5 and 10 of 2026-10-19)

The `scheduling` section parses, checks and defaults the three remedy keys, and the per-instance
section each instance's own choice list (contract text by R0, §1.110 B15-8;
[runtime-config.md](runtime-config.md)). **No persisted ledger shape** (model-b15 §10): the keys
are rows of the existing `config.parameters.scheduling` and `config.parameters.instance` facts and
of `host.config_overrides`; no code, key, vocabulary token or domain is added (a broken row is the
existing `config_value_invalid` with existing `config_value_problem` tokens). `actingd-config`
rises to revision 6, declared [5, 6] (§6): a revision-5 file is still read.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B15C-1 | `scheduling.remedy.enabled`, `.max_per_round`, `.choices` | configuration keys (§5 row; `config.parameters.scheduling` rows) | R3 | R3 (host 112) | defaults `true`, 2, `[]`, only in `actingd.defaults.json`; `max_per_round` `1..=8`; all settable; reader R4 |
| B15C-2 | `instance.<id>.scheduling.remedy.choices` (in the file, the entry's `scheduling.remedy.choices`) | per-instance key (`config.parameters.instance` row) | R3 | R3 (host 112) | default `section:scheduling.remedy.choices`; when set (an empty list too) it replaces the section's list as a whole, never merged; settable; `EffectiveConfig::remedy(instance_id)` gives `{enabled, max_per_round, rows}` with the list in effect |
| B15C-3 | the choice row rules (the `session.remedy.choices` shape) | rule (no shape change) | R3 | R3 (host 112) | `config_value_invalid` (`config_key`, `config_value_problem`) at load, in check-config and in `ConfigureSettings`: `wrong_type` for a row that is no object, a `row` or field missing or not a string, or a member its kind lacks; `out_of_range` for an unknown `row`, a name outside `^[a-z][a-z0-9_]{0,63}$`, `none` as a capability, or more than 256 rows; `section_check`, at the later row, for a row twice, a `none` with an `allow` of the same need, or two `parameter` rows of the same capability and parameter with different values (which value wins would be unsaid). The failure carries the JSON pointer of the row (and member) in the key's value; it is not an outcome value, because `config_value_invalid` declares no pointer key. Whether the packages declare the names is R7's |
| B15C-4 | `actingd-config` [5, 6] | interface revision (§6; K27) | R3 | R3 | revision 6 is revision 5 plus the optional `scheduling.remedy` object and the instance entry member `scheduling.remedy.choices`; a Runtime before v0.12.3 refuses a file that names either (the section and the entry deny unknown fields); raised once for the release |

### 1.114 Slice R4 b15h (v0.12.3): the host runs declared remedies (#411 B15; model-b15 §7, §9, §10, §12, §14; [remedies.md](remedies.md) sections 5 and 7)

The Runtime host fills the plan of §1.112 and acts on it: the remedy table from the bound packages,
the pairs' journal from the ledger, the plan in each evaluation with its deferrals at position 8a,
the admission check of a request or retry, and the stop, lift and expiry records. **No persisted
ledger shape** (model-b15 §10): the records written are B15-4 and B15-5 of §1.110, the chains carry
B15-3, the terminal is read (B15-11); the table, the journal's outcome keys and the recorded stops
are memory only, rebuilt at start; no code, key, vocabulary token, location or configuration key
is added. Gate: host key 113.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B15H-1 | the remedy table (`crates/runtime-host/src/remedy.rs`) | memory only (no shape) | R4 | R4 | built at start (once the dispatches are settled) and at the first evaluation under a newly activated catalog, again at most once per `scheduling.suspension.lift_check_min_interval_ms` while a round is open or a stop is recorded; each catalog task's package is read at its procedure binding's location by `read_remedy_declarations`, each location once; it records per (task, key) the remedy, per task the provided capabilities, every edge "blocked task to a provider of its remedies' capabilities" on a cycle over tasks (refused), and the findings `capability_unbound` (no provider task covers an instance the blocked task covers; with no instance known, none in the catalog), `cycle` and `parameters_mismatch` (providers of one capability on one instance declare different parameters); a location that cannot be read contributes nothing; building never fails a start or an activation; each new finding and each unreadable location is a developer line (`actingd remedy_table finding ...`, `actingd remedy_table unreadable ...`); R7 reports them as notes |
| B15H-2 | the remedy journal | memory only (no shape) | R4 | R4 | per (task, instance) of every task in the table, its admitted runs in ledger order since the pair's latest root blocked run, at most 64: the policy host's dispatch projection (`policy.dispatch_admitted`: decision, admission time, the activity window's end from the dispatch's own catalog, the `policy_remedy_requested` / `policy_remedy_retry` reasons; `policy.execution_recorded`: success or failure, settlement time), with the outcome key a success's terminal names; the key is read at settlement in the one terminal read the settlement makes, whenever the task's package declares a remedy and whether or not the catalog references keys (a task that declares none reads nothing more than before), and at start from the terminals of each such pair's latest runs back to its latest root blocked run; the `signal:remedy:` records are read in ledger order at start (seen ids; stops no lift followed) only when a bound package declares a remedy, and live as each is recorded |
| B15H-3 | the evaluation | rule (no shape change) | R4 | R4 | `plan_remedies` with the table, the journal, the recorded stops, the seen ids, `EffectiveConfig::remedy` of each instance by its registry id (an alias the registry does not hold takes the section's list), `scheduling.remedy.enabled`, `max_per_round` and the persistent gates `policy_task_paused`, `pack_admission_suspended`; then `evaluate_with_remedies` in place of `evaluate_with_fact_refresh` (with no remedy declared or remedies off the plan is empty and the evaluation is unchanged); in the eligibility callback a pair the plan defers gets the plan's reason after positions 1 to 7 and before availability (8a); the stops, lifts and expiries of `signals_after` are appended as B15-4 after the cycle's clock-slot records, each only when the planning-signal projection does not hold its id |
| B15H-4 | `policy.dispatch_rejected` with `policy_remedy_round_closed` (`task_id` the round's blocked task, `instance_id` the alias, `decision_id` the round, `operation` `validate_policy_dispatch`) | first writer of B15-5 (no shape change) | R4 | R4 | an intent whose chain carries `policy_remedy_requested` or `policy_remedy_retry` is admitted only while the plan at the admission time, on the admission's facts, still gives its pair a trigger override with a reason of the same code, blocked task, round and layer (`remedy_depth`); checked after the dispatch validation and before the budget preview: no lease, no run; an intent without such a reason is not checked; a served remedy also moves the instance's task states, which `policy_facts_stale` refuses first |
| B15H-5 | clock-slot gate classes | rule (no shape change) | R4 | R4 | `policy_remedy_unchosen`, `policy_remedy_unavailable`, `policy_remedy_exhausted` are persistent gates (`clock_slot_refused`), `policy_remedy_waiting` a waiting gate (`clock_slot_deferred` only once the window passed), as `scheduling/README.md` "Remedies" and `scheduling-blackout.md` "Order of the deferrals" state; a hold the evaluation turned into its round's stop after the allocation is traced as the stop |
| B15H-6 | EXEC-3 and failure identities | unchanged | R4 | — | a run ending on a remedy key settles as a success (no failure identity, streak cleared); `PolicyControlState`'s failure handling is untouched |

### 1.115 Slice R5 b15r (v0.12.3): a run reads its parameters as read-only facts (#411 B15; model-b15 §6; [remedies.md](remedies.md) section 4; [instance-fact-store.md](instance-fact-store.md), "Run parameters")

R5 implements B15-7 of §1.110 and is the first emitter of `session_dispatch_key_reserved` (B15-1):
the channel Workflow #411 B8 shares. **No persisted ledger shape** (model-b15 §10): the records
`session.dispatch.parameter.<name>` are an overlay of a run's instance fact snapshots, never
stored (§4 row; 0 rows in the capacity ledger); the chosen values are in the dispatch chain's
`policy_dispatch_parameters` (B15-3, R2 and R4); `task.selection_evaluated` keeps its shape, its
`input_sha256` covering the overlay; no code, key, vocabulary token, location or configuration key
is added (the code is R0's). No code, contract, default or test names a game, a parameter or an
option of a real package. Gate: host key 114.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B15R-1 | `RunParameters`, `PreparedContainedTask::default_run_parameters()`, `PreparedContainedTask::dispatched_run_parameters(values, admitted_at_unix_ms)`, `RunParameters::overlay(snapshot, now_unix_ms)`, `RUN_PARAMETERS_KEY_PREFIX` (`session.dispatch.`) | kernel interface (memory only, not persisted) | R5 | R5 | one record per parameter, in declaration or reason order, after the stored records: key `session.dispatch.parameter.<name>`, an inline string, the snapshot's instance as scope, confidence 1000, no expiry, `source_detector` `runtime.run-parameters`, `source_snapshot_id` `run-parameters`, `schema_version` `fact.v1`, `resource_bundle_hash` the SHA-256 of `package:<package_id>` (no package content is hashed); the snapshot keeps its id, the stored projection's; a package that declares no parameter adds nothing |
| B15R-2 | the host's select-step snapshot (`selection_state`, a run's one in-run read of instance facts, the entry-recovery layer's included) | rule (no shape change) | R5 | R5 | a policy run holds the `(parameter_name, parameter_value)` entries of its own chain's `policy_dispatch_parameters`, read as the fact refresh reads its requested keys, observed at the dispatch's `admitted_at_unix_ms`; a run without a dispatch (a direct run, a run the host starts itself) holds the declared defaults, observed at the snapshot's instant; a policy run whose chain carries no such reason (remedies off, or a parameter that does not resolve, §1.112 B15E-5) holds the declared defaults too, so a provider package still runs with its own scope |
| B15R-3 | the offline Lab providers (`simulate_contained_task`, `simulate_contained_task_at`, `replay_contained_task`) | rule (no shape change) | R5 | R5 | the declared defaults, observed at the run's instant, on the empty offline snapshot or the snapshot given; the single-step evaluations `dry_run_select` and the scan replay are not runs and are unchanged |
| B15R-4 | `PublishFact` / `PublishFacts` and the Runtime's own publications of a record under `session.dispatch.` | new refusal (first emission of B15-1) | R5 | R5 | `session_dispatch_key_reserved` with `fact_key` (the first such key), a non-fatal request error (`invalid_request`, receipt `denied`) before the fact write gate and every other publication rule; the whole observation is refused and nothing is appended; a person's (`cli`, `ui`) publication of a non-offset key is refused earlier by the existing origin rule (`invalid_agent_dispatcher_origin`) |

### 1.118 Slice Lab L1 (v0.12.x): a Lab2 verb refuses an option its path does not read (#411 Lab L1; G7; coordinator decision 11 of 2026-10-19)

Each path of actinglab `observe`, `do`, `ensure` and `wait` (offline with `--scene`; online with
`--capture`; for `do` and `ensure` also `--capture --dry-run`) reads a fixed set of options and
refuses every other one before it loads, captures or presses anything, instead of accepting and
ignoring it. [actinglab-capabilities.md](actinglab-capabilities.md) and
[contained-lab-operation.md](contained-lab-operation.md) are the contract. No persisted ledger
shape changes, and no code, key, wire, configuration or interface revision (§6): the CLI refusal
is the existing `validation_failed` (exit 2) and MCP's the existing `arguments_invalid` with its
`argument` / `argument_problem` values. Global options are not verb options and are unchanged.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| L1-1 | the refusal: `validation_failed`, exit 2, details `{argument, argument_problem, arguments: [{argument, argument_problem}]}` | new details of an existing code (exec 96) | Lab L1 | Lab L1 | `argument` is the first refused option in name order; `argument_problem` is `unknown` when no path of the verb reads the option and `conflicting` when another path does |
| L1-2 | online `do --capture` (not `--dry-run`) with `--destructive` or `--allow-destructive` | changed answer: refused as `conflicting` (was accepted and ignored) (exec 96) | Lab L1 | Lab L1 | before any Runtime connection; offline (`--scene`) and `--dry-run` planning read both as before; the declaration says so |
| L1-3 | declared options: `do` loses `--no-wait`, `--recovery-timeout-ms`, `--recovery-poll-ms`, `--priority`, `--lease-id`; `ensure` loses `--no-wait`, `--recovery-timeout-ms`, `--recovery-poll-ms`, `--lease-id`; `observe` loses `--require-fresh` | removed CLI options (`schema`, `capabilities`) (exec 96) | Lab L1 | Lab L1 | no path read them; each is now refused as `unknown` on every path; `ensure --lease-id` by decision 11 (plan R2 named `--no-wait` and `--recovery-*`) |
| L1-4 | any other option a path does not read | changed answer: refused (was accepted and ignored) (exec 96) | Lab L1 | Lab L1 | for example the loose declaration flags `--pack`, `--pack-root`, `--pages`, `--navigation` (these verbs read the package only), `--state-dir` without `--record`, `--test-capture-delay-ms` with `--capture`, `--tap` / `--swipe` / `--projection-*` without online `do --capture`, `--step-timeout-ms` / `--poll-ms` on `ensure` without online execution; a script passing one now fails with exit 2 |
| L1-5 | MCP `ac_lab_do` without `priority`, `lease_id`, `no_wait`, `recovery_timeout_ms`, `recovery_poll_ms`; `ac_lab_observe` without `require_fresh` | removed tool arguments: `arguments_invalid {argument, argument_problem: unknown}` (class `usage`) (apps 76) | Lab L1 | Lab L1 | refused before the install is located and before any actinglab child runs (the schema's `additionalProperties: false`) |
| L1-6 | MCP `ac_lab_do` with `capture: true`, `dry_run` not `true`, and `destructive` or `allow_destructive` `true` | changed answer: `arguments_invalid {argument, argument_problem: conflicting}` (class `usage`) (was passed to actinglab and ignored) (apps 76) | Lab L1 | Lab L1 | before any child, nothing recorded; offline, with `dry_run` or with both `false` the call goes on; any other path conflict is actinglab's refusal relayed as `lab_tool_failed` caused by `validation_failed` (the descriptions say so) |

### 1.119 Slice Lab L3 (v0.12.x): the Lab presses gestures as the kernel does (#411 Lab L3; G3, G4; Alice 2026-10-19, 「Lab的资源制作应当包含所有可用后端的识别以及触控」)

A long press is sent as one `InputAction::LongTap` wherever the Lab presses, as the kernel presses a
pack's `long_press` click, instead of a swipe that does not move; and `ensure --capture` names the
Runtime frame its guard used with each input, as online `do` names its before frame. No persisted
ledger shape: no ledger record, wire shape, configuration key, outcome code or interface revision
(§6) changes. `LabOperationSelection::Coordinates` already decodes `long_tap` (request v4); only
its validation widens, and the persisted prepared record keeps its shape (its `geometry` stays a
`drag`, its `action` is a long tap). [contained-lab-operation.md](contained-lab-operation.md),
[lab-recording.md](lab-recording.md) ("`--record` on device commands") and
[resource-declarations.md](resource-declarations.md) are the contract.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| L3-1 | `LabOperationSelection::Coordinates { action }` | accepted value widened: `long_tap` beside `tap` and `swipe` (exec 97) | Lab L3 | Lab L3 | it resolves to the geometry `drag` with `from` equal to `to` (a 1x1 rectangle at the point at both ends) and to the long tap itself; `key`, `text`, `reset` and the segmented drags stay `invalid_lab_operation_selection` |
| L3-2 | the action a resolved Lab selection presses | rule (no shape change) (exec 97) | Lab L3 | Lab L3 | a `drag` geometry whose `from` equals `to` in the same rectangle is `LongTap` at that point over `duration_ms`, every other drag one `Swipe`; the Runtime's preparation and the evidence verifier resolve by this rule; a pack `drag` with two equal rectangles and `do --capture --swipe x,y,x,y,ms` press a long tap too (on adb the same command) |
| L3-3 | the input of `do --capture --record` on a long-press step | rule (no shape change) (exec 97) | Lab L3 | Lab L3 | a long tap at the planned point (the centre of its click box) over the declared duration, where it was a swipe that does not move; a drag stays one swipe and a tap a tap; the click record (`to_point`, `duration_ms`) is unchanged |
| L3-4 | the input a Lab route presses for a navigation edge | rule (no shape change) (exec 97) | Lab L3 | Lab L3 | a `long_press` edge is a long tap at the point the reader holds (a box's centre), any other drag one swipe: `ensure --capture`, `navigate` (the kernel drive input `resolved_input_action`, which only the Lab uses) and `session recover` |
| L3-5 | the input request of an `ensure --capture` route | `frame` set (an existing optional member of the Runtime input request) (exec 97) | Lab L3 | Lab L3 | the frame reference (`frame_id`, `width`, `height`) of the observation its edge's guard was decided on, the Runtime's last committed frame; an instance whose input is `nemu_ipc` presses the tap instead of refusing it `nemu_input_frame_required`; on any other instance the Runtime resolves the frame first (`input_frame_unavailable` if another capture committed a later frame in between) |

### 1.120 Slice Lab L4 (v0.12.x): backend facts where the shape already exists (#411 Lab L4; Alice 2026-10-19, 「Lab的资源制作应当包含所有可用后端的识别以及触控」)

A Lab recording names the backend that captured a Runtime frame, and the offline capability
listing names the capture and input backends the Runtime can use instead of the choices actingd
refuses. No persisted ledger shape: no ledger record, wire shape, configuration key, outcome code
or interface revision (§6) changes. The rows are a value of an existing `recording.json` member
(a Lab file, not the ledger) and offline answer members.
[lab-recording.md](lab-recording.md) ("`--record` on device commands") and
[actinglab-capabilities.md](actinglab-capabilities.md) are the contract.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| L4-1 | `recording.json` frame `capture_backend` of an `observe --capture --record` frame (source `runtime_observation`) | new value of an existing member (Lab file, shape unchanged) (exec 98) | Lab L4 | Lab L4 | the backend of the Runtime frame (`ReadonlyObservation.capture_backend`), named as `capture --record` names it (`adb_screencap`, `droidcast_raw`, `nemu_ipc`, `scrcpy`), where it was `null`; the answer's `record.frame` carries the same value |
| L4-2 | `capabilities` `capture_backends` | changed values and one entry (exec 98) | Lab L4 | Lab L4 | a `scrcpy` entry (`backend: scrcpy`, `required_assets: ["adb"]`, `device_kinds: ["handset"]`, declared support, `unverified`); `auto` and `auto-fastest` `declared_support: false`, status `unavailable`, `reason_code` `capture_backend_must_be_explicit` (was declared support, `unverified`); every entry keeps `available: false`, `availability_checked: false` and `execution_authority: runtime` |
| L4-3 | `capabilities` `touch_backends` beside `capture_backends`: `nemu_ipc`, `maatouch`, `minitouch`, `adb_shell_input`, each `{id, gestures, declared_support: true, status: unverified, available: false, reason_code, execution_authority: runtime, availability_checked: false}` | output member (no persisted record) (exec 98) | Lab L4 | Lab L4 | `gestures` are the Runtime input action kinds the backend performs: `nemu_ipc` `tap` and the two segmented drags; `maatouch` also `long_tap`, `swipe`, `key`, `text`; `minitouch` `tap`, `long_tap`, `swipe` and the segmented drags; `adb_shell_input` `tap`, `long_tap`, `swipe`; `reason_code` `runtime_backend_configuration_and_assets_unverified` as the capture entries |
| L4-4 | `schema control` `capture_backend` / `touch_backend` and `session instance registry` `capture_backends` / `touch_backends` | changed values of existing members (exec 98) | Lab L4 | Lab L4 | capture `adb`, `droidcast_raw`, `nemu_ipc`, `scrcpy`; touch `nemu_ipc`, `maatouch`, `minitouch`, `adb_shell_input`; `auto` and `auto-fastest` are no longer listed |

### 1.121 Slice Lab L6 (v0.12.x): offline provider and evaluation parity; the replay reports readings and fields (#411 Lab L6; G10, G11; coordinator decision 10 of 2026-10-19)

Alice's principle of 2026-10-19 (「Lab的资源制作应当包含所有可用后端的识别以及触控」): the offline
tools evaluate with the provider the Runtime builds from the same vision root, and offline
`observe` projects what the online observation projects. A vision root with NN models and no OCR
model gives the offline tools a provider; offline `observe` evaluates an action's NN target and
fills the values of every target it evaluated; and (decision 10) the whole-package replay
reports the readings and fields its run took. [lab-recording.md](lab-recording.md) ("OCR in the
Lab"), [page-projection.md](page-projection.md) ("Shared input and output") and
[selection-graph.md](selection-graph.md) (the `actinglab replay` paragraph) are the contract. No
persisted ledger shape: no ledger record, wire shape, configuration key, outcome code or
interface revision (§6) changes. The rows are a rule of the Lab's provider, values of existing
page-projection members and members of an actinglab answer. Existing codes only
(`lab_ocr_unavailable`, `lab_ocr_model_missing`, `dynamic_target_not_evaluated`,
`target_not_matched`, `matched_template_rect_unavailable`); no catalog change.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| L6-1 | the provider of the offline tools (`LabOcr`) on a vision root whose model folders hold NN models and no OCR model | rule (no shape change) (exec 99) | Lab L6 | Lab L6 | was no provider; now the provider over the root's listing, as actingd builds it (`provider_startup`), given to every offline evaluation (the package loads of `observe`, `do`, `ensure`, `wait`, `package dry-run`, `replay`, the package edit check, `record stop`'s admission and cross-check); OCR stays unavailable with the same outcome (`lab_ocr_unavailable` caused by `lab_ocr_model_missing`), so a command that would evaluate an OCR target refuses as before, the mark self-test and `record stop`'s first decision report `lab_ocr_unavailable`, and the `ocr` state reports `available: false`; a `package dry-run` that evaluates an OCR target on such a root gets the refusal actingd's provider gives on it; a root with neither model, or only invalid folders, is unchanged |
| L6-2 | offline `observe`'s element of an action whose target is an NN target | changed values of existing members (exec 99) | Lab L6 | Lab L6 | evaluated when the provider holds the target's model (`require_nn_model`, the evaluator's own admission): `recognition_basis: target_evaluation`, `recognized` its `passed`, `blocked_reason` `matched_template_rect_unavailable` when it passed (an NN target matches no box) and `target_not_matched` when it did not, `input: null`; was always `not_evaluated` with `dynamic_target_not_evaluated`, which stays the answer when the provider does not hold the model |
| L6-3 | offline `observe`'s `observation.fields` | new values of an existing member (page projection shape unchanged) (exec 99) | Lab L6 | Lab L6 | was always empty; now, for each target of every kind in catalog order that the observation evaluated (every page's targets in page order, then each `--targets` target no page evaluated, as an optional target of no page), one entry `{target_id, field: null, parsed: false, value: {evaluations: [{evaluation_index, page, group_index, target_index, role, evaluation}]}}` and one more per `fields_v1` field an operation declares on it, with privacy and redaction as online (coordinator decision of 18:4x: the entries online gives, every kind); the online observation's construction, now one function (`observation_fields`) for both; the projection's limits apply, so Min may drop them (the `observe_success` golden: `entry_count` 1 → 4, `omitted_count` 1 → 4, a new `content_sha256`); without OCR no OCR target is evaluated, so none has an entry, and `ocr.not_evaluated` holds as before |
| L6-4 | `actinglab replay --package <path> --frames <directory> [--facts <file>]`: `readings?: [{frame, readings: [{reading_id, fact_key, value, confidence_milli}], unresolved: [{reading_id, fact_key, reading_problem}]}]`, `fields?` (the `actingcommand.runtime.post-admission-ocr-fields.v1` report) | operation output members (no persisted record; an actinglab result, read by a person or a CI job) (exec 99) | Lab L6 | Lab L6 | the replay ran the readings and fields and dropped them; `readings` holds one entry per frame the run handed to recognition, in hand-off order, `frame` its position in the sequence from 1, `reading_problem` the vocabulary token; `fields` is the run's report with every field its declaration classifies `personal` stripped of `raw_text`, `normalized_text`, `value`, `detail` and `extraction` and marked `redacted: true`; each is absent when the run took none; resource adjustments are not reported; `package dry-run` runs neither, as before |

### 1.122 Slice Lab L7 (v0.12.x): Lab page detection and guard as the Runtime's (#411 Lab L7; G12; Alice 2026-10-19, 「Lab的资源制作应当包含所有可用后端的识别以及触控」)

The Lab decides a frame's page as `current-page` (the kernel's read-only detection) decides it,
and offline `do` checks the page of the guard an operation declares on its target before it
evaluates the target, as the kernel guards that operation before its input (`guard_outcome`).
Rules and one catalog entry; no persisted ledger shape: no ledger record, wire shape,
configuration key or interface revision (§6) changes, and no answer member is added. The codes
are `page_recognition_conflict`, which `current-page` and `detect-page` already answered and
which this slice registers (L7-3), `contained_task_guard_refused`, the kernel's, and
`target_not_visible`.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| L7-1 | the page `do`, `ensure` and `wait` decide on a frame, offline (`--scene`) and in their `--capture` client forms (`do --capture --dry-run`, `ensure --capture` with every guard and arrival frame of its route, `wait --capture`) | rule (no shape change) (exec 100) | Lab L7 | Lab L7 | a frame that two or more pages match is refused with `page_recognition_conflict` (exit 2, details `{matched_pages}` in page order), as `current-page` refuses it, where the first matched page was taken; nothing is planned or pressed; one matched page is the page and none is `standby` as before; the same detection serves `session recover` (stale capture and startup login), which refuses such a frame the same way |
| L7-2 | offline `do <target> --scene --dry-run` on a target that the guard of a click operation of the package names (`guard.target_id`, any task file of the package) | rule (no shape change) (exec 100) | Lab L7 | Lab L7 | the frame must show a page one of those guards names (`page_id` read in its task's `game` as the kernel reads it; `any` names every page; a frame no page matches is no guard's page), else the plan is refused with `contained_task_guard_refused` (exit 3, safety; details `{req_id, error: resource_drift, state: <the frame's page>, hint, authority: isolated_offline}`) before the target is evaluated; on such a page the target is evaluated as before (`target_not_visible` when it does not pass); the guard's `expected_rect` is not compared, as the kernel compares it only at admission; a bare target, which no guard names, is evaluated as before; online `do --capture --dry-run` keeps the named-target evaluation |
| L7-3 | catalog entry `page_recognition_conflict` (`SharedCode`, fragment `contract.json`) | registered code (no shape change) (exec 100) | Lab L7 | Lab L7 | error, `ladder_class` `none` (row 7: a Lab CLI refusal), review `pending`, since 0.12.4, with description, common causes and handling; prefix `page_` was already the contract's; no value key added (the pages travel in the Lab envelope's `details.matched_pages`); the Lab crate's read-only commands and actinglab emit it through `SharedCode::PageRecognitionConflict` instead of a string |

### 1.123 Slice A21 (0.12.x): admission priority of online observation facts; actinglab observe failure details (#411 A21)

A21: admission priority of online observation facts; actinglab observe failure details. Rule
and answer details only; no persisted ledger shape, no wire, key or code change. The 256-row /
64-KiB bound of each fact collection is unchanged; offline `observe` has no fact budget.
[contained-page-observation.md](contained-page-observation.md) and
[business-identity-consensus.md](business-identity-consensus.md) are the contract. Existing codes
only (`observation_fact_failed`, `candidate_sample_record_failed`, `device_error`,
`lab_tool_failed`); no catalog change. `do --capture`, which evaluates the same observation
without explicit targets, follows the same rule.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A21-1 | the rows of `facts` and `private_facts` of an online observation | rule (no shape change) (exec 101) | A21 | A21 | rows are admitted by tier, while each fits: (1) the matched page's candidate evidence (`candidate_sample`, `candidate_projection`, `candidate_projection_failure`) and, when the pack declares target consensus, `recognition_coverage` and every `recognition_sample`; (2) `explicit_target`, `explicit_target_uses`, `explicit_target_uses_failure`, `target_failure` and the `target_evaluation` rows an `explicit_target_uses` row names; (3) every other row; kept rows stay in production order and omitted rows are counted as before (`item_count`, `omitted_count`, `target_evaluation_count`, `omitted_target_evaluation_count`, `truncated`); both collections, each with its own row sizes; when every row fits both collections are as before; a candidate sample is kept only while its projection's failure row still fits after it; explicit targets never change `status`, the projection page, the candidate sets or the elements by the budget |
| A21-2 | the failure of an online observation over the budget | rule (no shape change) (exec 101) | A21 | A21 | only a tier-1 row that does not fit either collection fails: a candidate sample ends its projection with `candidate_sample_record_failed` (partial), a candidate projection row or a consensus sample fails the observation with `observation_fact_failed` (`operation: encode_observation_fact`), with today's cause texts; until now an optional page or target row could crowd out a required one, and the check read the controlled collection only |
| A21-3 | a failed `actinglab observe --capture` → `device_error`, exit 4, with `details {error, state: "failed", req_id, receipt_state, runtime_outcome, correlation_id, hint: "inspect-runtime-ledger"}` | answer details added (exec 101) | A21 | A21 | `error` the Runtime outcome's code, `req_id` and `receipt_state` from the failed receipt when the client error carries one (absent otherwise), `runtime_outcome` the client error's typed outcome; the failed terminal ledger append keeps its message and gains the same details; the code and exit code are unchanged (lab-recording.md); MCP `ac_lab_observe` then answers `lab_tool_failed {exit_code: 4, req_id}` caused by `device_error` and a `foreign_other_error` holding the details, and records its `client.action` (no MCP change) |

### 1.124 Slice A22 (v0.12.3): a direct run refuses a recovery binding no path can use (#411 A22)

A22: a direct run refuses a recovery binding no path can use; new code
`contained_task_recovery_binding_unused` and location `check_recovery_binding`. No persisted
ledger shape, no wire, key or configuration change; the request shape is unchanged.
[linear-steps.md](linear-steps.md) ("Prerequisite packages", "Resolution"),
[package-reference.md](package-reference.md) ("Consumers") and
[runtime-state-observation.md](runtime-state-observation.md) are the contract.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A22-1 | `contained_task_recovery_binding_unused` (`error`, `ladder_class` `declaration_error`, `since` 0.12.3, pending) with `package_id` (required) at the new location `check_recovery_binding`, `invalid_request` (MCP class `usage`), receipt `Denied`, native detail `reason=entry_not_checked` or `reason=prerequisite_gate` | new outcome code and location, an earlier refusal (host 117) | A22 | A22 | a direct run (`run_contained_task`: CLI and console task-run, MCP `ac_run_pack`) that binds a recovery package is refused after its prerequisite chain is resolved and before any record, claim or lease when neither (a) its home entry can use the binding (empty chain, required entry pages, not a zero-step package with post-admission reading) nor (b) a ladder after its failed end can (`recovery_ladder.rules.direct_runs = trigger`, the instance's stuck recovery on, a physical instance; the configuration in effect); it writes no `task.*` record, takes no lease, sends no input and starts no ladder; `reason=prerequisite_gate` when the chain is not empty, else `entry_not_checked`; a request without a binding, a binding (a) or (b) can use, scheduled runs, startup packages, ladder rungs and Lab runs are unchanged; the MCP `ac_run_pack` description and its `recovery_package` schema text say so (no argument, member or class change) |

### 1.125 Slice A23 (v0.12.x): a run whose own package dispatched no step after other layers is judged on its own recognition (#411 A23)

A23: the scheduling disposition chooses its final-page rule by the run's own package's
dispatched steps, not by the run's total; a run that dispatched none after other layers is
judged on its last recognition. No persisted ledger shape, no code, no wire, key or
configuration change; `executed_steps` is unchanged. [linear-steps.md](linear-steps.md) (the
prerequisite gate and the Ledger table's scheduling outcome row) and
[ocr-fields.md](ocr-fields.md) (zero-input fields tasks) are the contract.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A23-1 | the terminal's `scheduling_disposition` (`task.completed`) after a prerequisite, return-home, navigation, entry recovery or interrupt-handler layer | rule (no shape change) (host 118) | A23 | A23 | the run's own package's dispatched steps choose the rule: with one or more, the last attempt's `StepFinished` of step `executed_steps - 1` (the package's own last step) must name the final page, as before; with none, the last `RecognitionCompleted` with a matched page must match it, whatever steps the other layers ran; a layer's page is never compared with the run's terminal pages; `executed_steps` in `TerminalCommitted`, the receipt and the run status stays the run's total; the effect bound, the designated-effect counts, the mapping choice and every outcome code are unchanged; a run with no layer is judged as before |

### 1.116 Slice R6 b15w1 (v0.12.3): a person or an agent writes the instance's remedy choices, checked at the write (#411 B15; model-b15 §5.1, §7.6; [instance-fact-store.md](instance-fact-store.md), "Remedy choice facts")

R6 implements §1.110 B15-6: the family, the write rule and the writers of the instance fact
`session.remedy.choices`, which R4 reads. **No persisted ledger shape** (model-b15 §10): a write
is an ordinary `fact.published` of an instance fact (§4 row; shape unchanged) that keeps the
request's origin; the refusal `session_remedy_choices_invalid` and its keys are R0's catalog
entries (§1.110 B15-1, B15-2); no code, key, vocabulary token, location, configuration key or
interface revision is added; the CLI and MCP answers are output only. Gates: host key 115, apps
key 74.

**Scopes (coordinator ruling on #993).** Remedy choices are written at instance or game scope
only. The fact store's server scope (`FactScope::Server { server_id }`) names a server without
its game and covers every instance whose server id is equal, so a server id two games share would
share one record of remedy choices across both games, which is never intended; the instance and
game scopes are the only ones because the server scope is not game-qualified. The write rule
refuses a server-scoped record from every writer (`field` `/scope`), `remedy set` has no
`--server` and `ac_remedy_set` no `server` (both refused as arguments the command or tool does not
take). No code, key or token is added: the refusals are `session_remedy_choices_invalid`,
`actingctl_usage_invalid` and `arguments_invalid` with `argument_problem` `unknown`.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B15W-1 | `FactKeyFamily::RemedyChoices` (`fact_key_family("session.remedy.choices")`); `FactKeyFamily::takes_origin_exception` | contract rule (no shape) (host 115) | R6 | R6 | `RuntimeRequest::validate` refuses any `PublishFact(s)` whose keys belong to two families as `fact_origin_mixed` (for offsets and other keys as before); until U-6 an observation of a family with the origin exception (offsets, remedy choices) is taken from `(agent, adapter)`, `(user, ui)`, `(cli, cli)` and every other key from `(agent, adapter)` only (`invalid_agent_dispatcher_origin`); the family is a general instruction (§2.6) |
| B15W-2 | `fact.published` of `session.remedy.choices` (§1.110 B15-6) | first writer (shape unchanged) (host 115) | R6 | R6 | the event's source and actor are the request's, as for offsets; `source_snapshot_id` `remedy-choices:<sha256 hex of the JSON of (scope, rows, observed_at_unix_ms)>` (`remedy_choices_snapshot_id`, `FactRecord::remedy_choices`), so the same rows written later are a new version; `resource_bundle_hash` the same hex; the writers give no expiry and full confidence |
| B15W-3 | the write rule | rule (no shape) (host 115) | R6 | R6 | checked whole before the fact write gate, nothing appended on refusal, `session_remedy_choices_invalid` (`fact_key`, `field`, `instance_id` for a meaning error; request error, receipt `Denied`); shape first (scope `instance` or `game`: a `server` scope is refused as `/scope` from every writer, through `PublishFact` and `PublishFacts`, whatever the rows, the empty list included; an inline `record_list`; `row` `allow`, `none` or `parameter` with exactly its fields, each a string token `^[a-z][a-z0-9_]{0,63}$`, `none` never a capability; no row twice; no `none` with an `allow` of the same need; no two `parameter` rows of one capability and parameter; at most 256 rows, which the request's validation refuses first; no `invalidate_on`), then the meaning against the remedy table built from the active catalog's bound packages when the write arrives (`remedy::build_table`), on every instance the scope covers (an instance scope its instance, a game scope the policy inputs' instances of that game): an `allow` whose need no bound remedy declares (`/<row>/need`) or whose capability that need's remedies do not list (`/<row>/capability`); a `parameter` row whose capability no bound provider declares (`/<row>/capability`), whose parameter it does not declare (`/<row>/parameter`) or whose value is not an option (`/<row>/value`); `none` rows and an empty list read no package; `field` `""` for a value that is not a row list, `/<row>` for a row twice or a conflict (the later row), `/invalidate_on` for invalidating events |
| B15W-4 | `actingctl remedy set (--instance <selector> \| --game <id>) --rows <file>`, `remedy get --instance <selector>`, `remedy lift --instance <selector>` | new CLI commands (output only) (apps 74) | R6 | R6 | (cli, cli); `set` publishes the scope's whole list (a bounded JSON list of objects with string members) and answers `{event_id, scope, key, source_snapshot_id, observed_at_unix_ms, rows_total}`; `get` answers `{instance_alias, read_at_unix_ms, records, in_effect}` (the records that apply, most specific first, each with `in_effect`; `in_effect` the scope of the most specific unexpired record, `null` without); `lift` republishes the instance's own record unchanged (its rows and the length of its lifetime) or an empty one, strictly newer, and adds `previous_source_snapshot_id`; the line's outcome is the completed receipt's code (`request_publish_facts_completed`, `request_instance_fact_snapshot_completed`: no actingctl success code is registered for these commands); a Runtime refusal prints the denied receipt and exits 1; a flag the action does not take (`--server` included: no server scope), a repeated flag, a missing target or `--rows`, conflicting targets and an unreadable or ill-shaped rows file are `actingctl_usage_invalid` (`argument`, `argument_problem`) |
| B15W-5 | MCP `ac_remedy_set {instance \| game, rows \| lift: true}`, `ac_remedy_get {instance}` | new general tools (observer until U-3) (apps 74) | R6 | R6 | the CLI's answers without the outcome; `ac_remedy_set` writes on a fresh (agent, adapter) connection after the provenance record (`client.action` `ac_remedy_set`); `lift` only with `instance` and without `rows`; arguments refused as `arguments_invalid` (`server` is no argument: `unknown`); a `get` answer past the output budget is exported (`answer_exported`) |

### 1.117 Slice R7 b15w2 (v0.12.3): a stopped remedy is visible where a person looks; check-config finds unusable remedies (#411 B15; model-b15 §7.5, §9.1, §13; [remedies.md](remedies.md) section 10)

`actingd suspended` and MCP `ac_diagnose` show the remedy rounds that stopped for a person, read
from R4's stop and lift records, and `actingd check-config` reports the remedies the bound packages
or the configuration make unusable. [policy-suspension.md](policy-suspension.md) (`remedies`) and
[actingd-check-config.md](actingd-check-config.md) (the notes) are the contract. **No persisted
ledger shape** (model-b15 §10): the records read are B15-4 of §1.110; the report and output
members are B15-9, not persisted, the `schema_version`s unchanged; no code, key, vocabulary
token, location, configuration key or interface revision is added; nothing is written. Gates:
host key 116, apps key 75.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B15W2-1 | `actingd suspended` top-level `remedies` and `remedies_total` (B15-9) | new report members (not persisted) (host 116) | R7 | R7 | every `policy.planning_signal_observed` of kind `goal_missed` whose `signal_id` starts with `signal:remedy:stopped:`, selected by kind and prefix, never by outcome; kept while in effect (no later `signal:remedy:lifted:` of the same `<hex>`), whatever its age, or while that lift's event time is within 48 h of the ledger's latest event; newest first by sequence, at most 200; `code`, `remedy_problem`, `outcome_key`, `need` from the outcome, `task_id` and `instance_id` the record's own, `recorded_sequence` / `recorded_at_unix_ms` the event's, `lifted {sequence, at_unix_ms}` the lift's event; an outcome that is not one of the three stop codes, holds a value its code does not declare, misses a required one, has one of the wrong type (`remedy_problem`, `remedy_source` outside their vocabularies) or a record without a task is `{parse_state: "unparsed", signal_id, code}`, no warning |
| B15W2-2 | MCP `ac_diagnose` / `actingctl diagnose` member `remedies` (and `remedies_truncated`) | new output member (not persisted) (apps 75) | R7 | R7 | the report's rows whose `instance_id` is the diagnosed instance's alias or id (every row without an instance; an unparsed row names none), verbatim; no window filter; cut for the output budget after `packs`, before `report_warnings`; the tool description and result schema name it |
| B15W2-3 | `check_config_passed` `note` links `policy_remedy_declaration_unusable` (warning) | first emission of a B15-1 code (no shape) (host 116) | R7 | R7 | R4's table built offline over the configured catalog and `policy.procedure_manifest`'s locations for the policy inputs' instances: each finding (`capability_unbound`, `cycle`, `parameters_mismatch`) names `task_id`, `outcome_key`, `need`, `capability`, `instance_id` (the alias) and `package_id` where known; then the rows of `scheduling.remedy.choices` and of each `instance.<id>.scheduling.remedy.choices` the file sets: an `allow` / `none` of a need no bound remedy declares is `choice_invalid` with `need`, an `allow` of a capability that need's remedies do not list `choice_invalid` with `need` and `capability`, a `parameter` row whose capability, parameter or option no bound provider declares `parameter_invalid` with `capability`; a per-instance list's note adds its registry `instance_id`; each row's note is also a developer line on stderr naming the list's key and the row; a package that cannot be read gives no note (the dispatch reports it) but, as at run time, the developer line `actingd remedy_table unreadable code=<code> tasks=<tasks> path=<path>` on stderr; a catalog that does not compile gives none (the preview refuses it); the outcome stays `check_config_passed`, exit 0 |

### 1.130 Slice Lab L5a (v0.12.x): every online Lab observation takes the frames its package's target consensus needs, as one transaction (#411 Lab L5a; G8 online; coordinator decisions Q2–Q5 of 2026-10-19; Alice 2026-10-19, 「Lab的资源制作应当包含所有可用后端的识别以及触控」)

`observe --capture` and each observation of `do --capture` (before, after, every arrival
sample) capture the frames the package's target consensus needs and evaluate them as the
kernel's page transaction does. Rules, one fact row kind, one Rust API parameter and one
catalog entry; [contained-page-observation.md](contained-page-observation.md),
[contained-lab-operation.md](contained-lab-operation.md) and
[business-identity-consensus.md](business-identity-consensus.md) are the contract. The
records keep their shapes: the do's prepared and terminal records still reference one before
frame, one after frame and the arrival samples, and the observation evidence keeps its
members; the earlier frames are ordinary capture events and `CaptureFrame` artifacts.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| L5-1 | an online Lab observation's frames (`observe --capture`; `do --capture`'s before, after and arrival observations) | rule (no shape change) (host 120) | Lab L5a | Lab L5a | with F = `target_sample_frames()` > 1 it captures F − 1 sample frames and then the current frame, `target_sample_interval_ms()` apart (unscaled, decision Q2), each a capture of its own (capture events, verified `CaptureFrame` artifact, frame ID, dedup marker, committed input frame), and the kernel evaluates pages, explicit targets, elements and fields over the set, oldest first (`scene_context_with_samples`); the last frame is the observation's frame (`frame`, the projection's `FrameIdentity`, `rgb8_sha256`, do's `before_frame` / `after_frame` / arrival sample frame and its input frame); `recognition_coverage` reads `provided_frames` = `required_frames` = F, `aggregation: production_owner`, and consensus no longer makes the observation partial; frames of different sizes fail it with the kernel's `recognition_sample_geometry_changed`; the waits hold no lock; a package without target consensus captures one frame and its rows, events and answers are unchanged; an observation stores at most F ≤ 5 frames, a do at most 2·F, or 21·F ≤ 105 with arrival (decision Q5); the DiagnosticJson bounds are unchanged |
| L5-2 | fact row `{"kind": "candidate_projection_skipped", "layout_id", "coverage": {"provided_frames": F, "required_frames": F_l, "aggregation": "not_requested"}}` in `facts` and `private_facts` of a `do --capture` observation | fact row kind added (no shape change) (host 120) | Lab L5a | Lab L5a | decision Q4: a Lab `do`'s observations are page observations; like the kernel's page waits and guards they do not project a candidate layout that needs F_l > 1 frames, and the skipped layout does not make them partial; single-frame layouts are projected as before; the row is required evidence (#411 A21 tier 1); `observe --capture` keeps its layout handling (L5b changes it) |
| L5-3 | the shared Lab operation evidence verifier (`verify_lab_operation_evidence`, the client's and `resource restore`'s) | rule (no shape change) (host 120) | Lab L5a | Lab L5a | each observation (before, then each arrival sample, or the after observation without an arrival condition) is preceded by exactly `provided_frames − 1` further verified `CaptureFrame` artifacts of the request, `provided_frames` from that observation's `recognition_coverage` row (one frame without the row), after the lease grant and the previous observation's projection and before its own frame, each with its own artifact lifecycle; the observation the operation failed in (a frame or projection stage) may lack its projection, and a transaction it failed in before its frame leaves its earlier frames after the last projection, fewer than five in either case; any other frame is `runtime_lab_frame_budget_mismatch`; the client's observe verification is unchanged (it binds the current frame) |
| L5-4 | `RuntimeDebugSession::observe_contained_page` and `run_contained_lab_operation` take an `ObservationFramePlan` (frames 1..=5, interval) | Rust API parameter added (no wire change) (host 120) | Lab L5a | Lab L5a | actinglab computes it from the package it sends with the kernel's `PreparedPageObservation::frame_plan` (`observation_frame_plan`), the host's helper, and refuses a package the kernel does not admit for observation with `package_invalid` before any Runtime request; the receipt wait adds, for each observation a request can make (observe 1, do 2 or 21 with arrival), F − 1 backend-open allowances and (F − 1) × interval; MCP runs actinglab without a deadline and changes no argument, only the `ac_lab_observe` and `ac_lab_do` descriptions |
| L5-5 | catalog entry `observation_samples_interrupted` (runtime-host, `HostCode`, fragment `runtime-host.json`) at the new location `check_observation_sample_inputs` | registered code and location (no shape change) (host 120) | Lab L5a | Lab L5a | error, `ladder_class` `none` (row 7: a Lab request refusal), review `pending`, since 0.12.4, with description, common causes and handling; decision Q3: after the current frame of a keyless `observe --capture` transaction one native `EventQuery` (type `InputIntent`, the instance, from the first sample frame's capture request to the current frame's terminal, limit 1) finds whether an input reached the instance; one did: the request fails (`recognition_failed`, receipt `failed`) through the observation failure record before anything is recognised, and actinglab answers `device_error` (exit 4, `details.error` the code) as #411 A21 does; `do` holds its lease across its transactions and makes no query |
| L5-6 | an online `observe`'s candidate layout series; the `candidate_projection` row of a layout projected over its series gains `coverage {"provided_frames": F_l, "required_frames": F_l, "aggregation": "production_owner"}`, and a `candidate_projection_failure` row's `provided_frames` counts the frames its projection was given | rule; fact row member added (no shape change) (host 121) | Lab L5b | Lab L5b | when `observe --capture` is complete and recognises exactly one page, each candidate layout of that page that needs F_l > 1 frames, in catalog order, takes F_l − 1 further page transactions (L5-1), the layout's `sample_interval_ms` (unscaled, decision Q2) apart after the current frame, each a keyless transaction of its own (capture events, verified `CaptureFrame` artifacts, frame IDs), the waits holding no lock; each must recognise the same single page on the same frame size, else the layout's row is `candidate_projection_failure` with the kernel's `candidate_sample_page_or_geometry_changed` (item `frame`) and the observation is partial; the layout is projected with `project_candidates_with_samples` over the current frame and then each further transaction's current frame, the last one's scene the projection's, as the kernel's select step does; the observation's frame, projection, elements, fields, explicit targets and every other row stay those of its own transaction; a single-frame layout's row is unchanged; a `do`'s observations still skip such layouts (L5-2); L5a's input check (L5-5) runs from the observation's first capture to the last capture of its series, after the evaluation and before anything is persisted; a series capture that fails ends the request with its own failure and terminal, the PP-OCR reports of the evaluations before it archived |
| L5-7 | at most 25 frames per online `observe` (`MAX_OBSERVATION_SERIES_FRAMES`) | rule (no shape change) (host 121) | Lab L5b | Lab L5b | decision Q5: the bound counts the observation's own transaction and each admitted series whole ((F_l − 1) × F frames), so which layouts get a series depends on the package alone; a layout whose series would exceed it is projected over the current frame alone, which the recognition pack refuses itself with `candidate_samples_unavailable` (coverage `provided_frames` 1, `not_completed`), and the observation is partial; a later layout is still tried within the bound; restates L5-1's five-frame bound for `observe` (its own transaction stays at most five frames) |
| L5-8 | `RuntimeDebugSession::observe_contained_page` takes an `ObservationSeriesPlan` (frames 1..=25, waits), or anything that converts into one (an `ObservationFramePlan` converts to its one transaction) | Rust API parameter changed (no wire change) (host 121) | Lab L5b | Lab L5b | actinglab computes it from the package it sends with the kernel's `PreparedPageObservation::series_plan`: over the package's pages, the most frames and the longest waits an `observe` takes, its transaction ((F − 1) × interval) and, per page, its admitted series (each further transaction the layout's interval plus its own transaction's waits); the receipt wait allows one backend-open allowance per frame plus the waits; `do --capture` keeps its `ObservationFramePlan` allowance (L5-4) |

No persisted ledger shape: no ledger record, wire shape, configuration key or interface revision changes; one outcome code.

### 1.131 Slice Lab B-2 (v0.12.x): offline multi-frame input; a Lab2 option is taken once (#411 Lab B-2; G8 offline; coordinator decisions Q1, Q7, Q9, Q11 of 2026-10-19; Alice 2026-10-19, 「Lab的资源制作应当包含所有可用后端的识别以及触控」)

The offline Lab takes the frame set a package's multi-frame agreement needs, as replay and the
task path do: `--scene` is repeatable on the offline paths, and target consensus, page detection
and candidate layouts decide over the given frames.
[business-identity-consensus.md](business-identity-consensus.md),
[page-projection.md](page-projection.md), [contained-lab-operation.md](contained-lab-operation.md),
[actinglab-capabilities.md](actinglab-capabilities.md) and
[runtime-interface.md](runtime-interface.md) (MCP rows) are the contract. Codes are the existing
`validation_failed` (exit 2) with the existing `argument_problem` tokens and keys (`range_min`,
`range_max`) and MCP's existing `arguments_invalid`.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B2-1 | actinglab `--scene <png>` given 1 to 5 times, oldest first, on offline `observe`, `do --dry-run`, `ensure`, `wait` and on `detect-page`, `current-page`, `recognize`, `is-visible` | changed CLI option (repeatable) (exec 107) | Lab B-2 | Lab B-2 | the last frame is the current frame: `frame_source`, `frame_coordinates`, `observation.frame`, `--with-frame`, `--preview`, the destructive-region check and every geometry are its; page detection and every target (page targets, `--targets`, do's target, recognize's and is-visible's targets, guard checks) are evaluated over all the frames, so target consensus reads its last required frames; a candidate layout of offline `observe`'s page is projected over its last `required_frames()` frames, without a page check per frame (decision Q11); single-frame targets read the current frame; the interval is not checked; paths with `--capture` still refuse `--scene`; one `--scene` reads as before; `tap-target` and `navigate` read one `--scene` and refuse a repeated one (B2-6); the other `--scene` commands are unchanged |
| B2-2 | the refusal of the frames: `validation_failed`, exit 2, details `{argument: "--scene", argument_problem, ...}`, before anything is evaluated | new details of an existing code (exec 107) | Lab B-2 | Lab B-2 | more than 5: `out_of_range` with `range_min` 1, `range_max` 5; fewer than the package's target consensus requires (F_t): `out_of_range` with `range_min` F_t, `range_max` 5, `required_frames`, `provided_frames`; offline `observe` whose matched page has a layout requiring more frames than given: `out_of_range` with `range_min` its frames, `range_max` 5 and `layout_id`, after the page is detected and before anything else is evaluated, nothing answered; more than one frame for a package without multi-frame agreement (no target consensus and every layout over one frame): `repeated`; the same path twice: `repeated`; frames of different sizes: `unusable` with `index`, the position (0 the oldest) of the first frame whose size differs from the current one's; in a set, a `--scene` without its value: `value_missing`; these replace offline `observe`'s `package_invalid` refusals of target consensus and of a layout's frames and the detection paths' prose `validation_failed` ("requires N frames, got 1") |
| B2-3 | `recognition_coverage {provided_frames, required_frames, aggregation: "production_owner"}` on the answers of `detect-page`, `current-page`, offline `do`, `recognize` and `is-visible` | new answer member (exec 107) | Lab B-2 | Lab B-2 | present only when the package declares target consensus, so a single-frame answer without it is unchanged; `provided_frames` the frames given, `required_frames` the most any target consensus requires; the kernel's `PageDetectionResponse`, `RecognizeEvaluatedResponse` and `IsVisibleResponse` gain the optional member (Rust API; no wire) |
| B2-4 | offline `observe`'s `recognition_coverage.provided_frames` and `candidate_coverage.provided_frames` | changed answer values (exec 107) | Lab B-2 | Lab B-2 | the frames given (was always 1), and the most frames a projected layout used (was always 1); `observation.metrics.sample_scope` is unchanged (`single_offline_observation`) |
| B2-5 | MCP `ac_lab_observe` and `ac_lab_do` `sample_scenes`: a list of 1 to 4 strings, none flag-like | new tool argument (apps 78) | Lab B-2 | Lab B-2 | the earlier frames, oldest first; actinglab receives `--scene <s1> ... --scene <scene>`; without `scene` it is `arguments_invalid {argument: sample_scenes, argument_problem: conflicting}` (class `usage`), a list outside 1..4 `out_of_range` and a flag-like item `not_recognised`, each before the install is located and before any child runs; the tool descriptions say so |
| B2-6 | an option a path of `observe`, `do`, `ensure` or `wait`, or `recognize`, `is-visible`, `detect-page`, `current-page`, `tap-target` or `navigate`, takes once, given more than once | changed answer: refused (was last-wins) (exec 107) | Lab B-2 | Lab B-2 | decision Q9 (R7): `validation_failed`, exit 2, details `{argument, argument_problem: "repeated", arguments: [{argument, argument_problem}]}` (the first in name order, then every one), before anything is loaded, captured or pressed, on the Lab2 verbs after the L1 check of unread options; only the `--scene` of the offline Lab2 paths and of `recognize`, `is-visible`, `detect-page` and `current-page` repeats; `tap-target` and `navigate` read one frame and refuse a repeated `--scene`; a script that passed an option twice now fails |

No persisted ledger shape; Lab interfaces (CLI, answers, MCP) only; no outcome code, wire,
configuration key or interface revision (§6) changes.

### 1.132 Slice Lab B-9 (v0.12.x): Lab recordings mark value tests (#411 Lab B-9; G17; coordinator decision 7 of 2026-10-19 and decision Q10 of the L5 plan; Alice 2026-10-19, 「Lab的资源制作应当包含所有可用后端的识别以及触控」)

A Lab recording marks a value test (task schema `0.9`, Workflow #396 REC-6) whose sides are
inline OCR reads, self-tests it with the Runtime's evaluator and the Lab's OCR provider, and
`record stop` declares it as the Runtime reads it. The rows are a Lab request and file member,
values of an existing actinglab answer list and the generated package's declarations, which
are the task schema's own. [lab-recording.md](lab-recording.md) ("Value tests", "Pages and
documents", "Self-checks") is the contract. Existing codes only (`validation_failed`,
`record_mark_rejected` with `self_test_failed`, `self_test_error` and `region_outside_frame`,
`anchor_invalid`, `record_click_source_invalid`, `record_click_follow_invalid`,
`record_mark_id_conflict`, `lab_ocr_unavailable`; the recognition pack's `value_test_unresolved`
and `value_test_not_met` in details); no catalog change.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B9-1 | `actingcommand.lab-record-mark.v1` `add[]` entry of family `value_test`: `{id, family: "value_test", left, op, right?}`, each side `{region, languages, timeout_ms, minimum_confidence, model_ref, model_sha256, trim, value: {type: "unsigned_integer", min, max, format?}, part?, mul?, add?}`, `right` `{const}` or a side | new mark family and request members (Lab request schema) (exec 108) | Lab B-9 | Lab B-9 | request form only; the Runtime's value-test rules (`parses` without `right`, every other `op` with one; `const` in 0..=2^53-1; `mul` 1..=1000000 and `add` within ±(2^53-1) on the right side only, both there; `part` exactly with `current_capacity`; `min <= max`); a shape error is `validation_failed` (exit 2) and a side region outside the frame `region_outside_frame` inside `record_mark_rejected`; the self-test reads the sides with the Lab's OCR provider on every live frame (failure details `{frame_id, failure, value_test}`), `not_evaluated` (`lab_ocr_unavailable`) without a model; forbidding it, directly or through a check, is `validation_failed` (`{mark_id, family}`), and it is never an anchor, click source or followed template |
| B9-2 | `recording.json` `RecordedMark.value_test: {left, op, right?}` | new optional member (Lab file, `skip_serializing_if`) (exec 108) | Lab B-9 | Lab B-9 | present only on a value-test mark, which has no `region`; the mark output shows the mark and `record status` its `value_test` (`marks[].value_test?`); the side target ids `<id>/left` and `<id>/right` share the recording's mark id space (`record_mark_id_conflict`) |
| B9-3 | the package `record stop` generates for a value-test mark | generation rule (task schema `0.9` declarations, no shape change) (exec 108) | Lab B-9 | Lab B-9 | one `ocr_targets[]` entry per side (`<id>/left`, `<id>/right`) with the side's region, languages, timeout, minimum confidence and model, `match_mode: "contains"`, `expected` `"0"`..`"9"`, `case_sensitive: false`; one `checks[]` entry `{id, value_test: {left: {target_id, trim, value, part?}, op, right?}}`; the page rule requires the value test (not as an `any_of` member), never its sides; as the click guard `guard.check` is its id and `expected_rect` the bounding box of its sides; the first decision and the cross-check count it as OCR (undetermined without a model, the admitted evaluator with one) |
| B9-4 | `session record-policy` `lab_recording.families` | changed value of an existing answer member (exec 108) | Lab B-9 | Lab B-9 | `value_test` after `check` |

Lab file format, no persisted ledger shape, no outcome code.

### 1.133 Slice Lab BD (v0.12.x): Lab recordings declare a drag with a vertical brake and generate it (#411 Lab BD; G1, G17; coordinator decisions 6 of 2026-10-19 and Q6 of the L5 plan; Alice 2026-10-19, 「Lab的资源制作应当包含所有可用后端的识别以及触控」)

A Lab recording step declares the Runtime's segmented drag `single_touch_drag_with_vertical_brake_v1`
on its click rectangle and a corner box, `record stop` generates it as the task's source click and
simulates it as the kernel does, and a Lab operation does not press it (that is B-3, Alice's
pending item 14-0; decision 6: no coordinate form). The rows are a Lab request member and CLI
flag, a Lab file member, the generated package's click, which is the task schema's own source
form, and one catalog entry. [lab-recording.md](lab-recording.md) ("record mark", "Drags and
long presses", "`--record` on device commands", "Pages and documents", "Self-checks", "Error
codes") and [resource-declarations.md](resource-declarations.md) (the segmented drag paragraph)
are the contract. Existing codes otherwise (`validation_failed`, `record_step_click_not_executed`,
`record_artifact_admission_failed`).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| BD-1 | `actingcommand.lab-record-mark.v1` `click.brake_corner` (`{x, y, width, height}`) and `record mark --brake-corner x,y,w,h` | new request member (Lab request schema) and CLI flag (exec 109) | Lab BD | Lab BD | with `click.region` (`--click x,y,w,h`), the rectangle the drag presses in; both boxes inside the frame with width and height ≥ 1 and the corner's `y` ≥ 100 (pack containment's bound); with `duration_ms`, `drag_to`, `long_press`, `from` or `follow` (their flags) it is `validation_failed` (exit 2; details `{argument, argument_problem: "conflicting", arguments}`, and `{argument: "brake_corner", argument_problem: "out_of_range", range_min: 100}` for the corner bound); guard, retry and `replace_click` as for a tap; the shortcut and the request form give the same click; `record mark`'s schema summary lists the flag |
| BD-2 | `recording.json` `StepClick.gesture` `{"kind": "single_touch_drag_with_vertical_brake_v1", "corner": {x, y, width, height}}` | new value of an existing optional member (Lab file, `skip_serializing_if`) (exec 109) | Lab BD | Lab BD | the click's `rect` is the rectangle it presses in; the mark output and `record status` (`click.gesture?`) show it; its duration is 550 ms (200 + 150 + 200); `do --capture --record` on its step refuses before any Runtime connection, capture or recording change and records nothing (BD-4); an offline frame or `--close-step` closes the step, and a device frame before that is `record_step_click_not_executed` |
| BD-3 | the package `record stop` generates for a brake drag | generation rule (task schema `0.9` source click, no shape change) (exec 109) | Lab BD | Lab BD | the step's `click` is `{"kind": "single_touch_drag_with_vertical_brake_v1", "from": <click rect>, "corner": <corner>, "horizontal_duration_ms": 200, "corner_hold_ms": 150, "brake_distance_px": 100, "brake_duration_ms": 200}`, which source compilation writes as `from_rect` and `corner_rect`; the default task timeout counts g = 550; the task provenance records the gesture on the step's click (`provenance.steps[].click.gesture`); the kernel admits the package; on step 1 the first decision must be the kernel's `SingleTouchDragWithVerticalBrakeV1` of `step_01_click` with (x1, y1) in the click rectangle, (x2, y2) in the corner box, x3 = x2, y3 = y2 − 100 and the four constants, anything else `record_artifact_admission_failed` |
| BD-4 | catalog entry `record_gesture_execution_unsupported` (lab, `LabCode`, fragment `lab.json`) | registered code (no shape change) (exec 109) | Lab BD | Lab BD | decision Q6 (a): error, `ladder_class` `none` (row 7: a CLI refusal), layer `process`, no values, review `pending`, since 0.12.5, with description, common causes and handling (how to close the step); `do --capture --record` on a brake-drag step, exit 3, actinglab details `{step, gesture}`; it travels typed (`LabCode::RecordGestureExecutionUnsupported`) |

Lab file format, no persisted ledger shape; one outcome code; no wire, configuration key or
interface revision (§6) changes.

### 1.135 Slice D1 (0.12.5): the scheduler-side random delay of every task an instance is dispatched (#405 D1; plan-405-406 §6, §8 Q6–Q12; coordinator decisions of 2026-10-19; Alice 2026-10-14, 「这个随机延迟是调度器侧的…覆盖这个实例下所有的任务」)

D1 reads the random delay keys SR-1 parses (§5): once an instance has its delay enabled, every
task the policy dispatches there waits a random time first. **No persisted ledger shape**: the
record written is X14 of §1.25 (`policy.dispatch_intent` `random_delay`), whose first writer D1
is; the draws are memory only and not kept across a restart; one code and two value keys are
added (catalog entries, not rows); no wire, configuration key or vocabulary is added.
[scheduling-blackout.md](scheduling-blackout.md) ("Order of the deferrals", position 9),
[scheduling/README.md](scheduling/README.md) ("Why A Candidate Does Not Run") and
[runtime-config.md](runtime-config.md) (the per-instance random delay) are the contract. Gate:
host key 122.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| D1-1 | `EffectiveConfig::random_delay(instance_id) -> Result<RandomDelay, ConfigFailure>` | configuration accessor (no shape) (host 122) | D1 | D1 | each of `enabled`, `min_ms`, `max_ms` the instance's own `instance.<id>.scheduling.random_delay.<member>` when the configuration carries it, otherwise the section's; the pair checked as SR-1 checks it (`config_value_invalid`, `section_check`, for the `min_ms` key read); the host reads it at every authoritative policy cycle for every instance of the snapshot by its registry id (an alias the registry does not hold takes the section's keys) and hands it to the evaluation |
| D1-2 | the random delay table of `PolicyHost` | memory only (no shape) (host 122) | D1 | D1 | per (task, instance alias): `due_at`, the delay, the planned start and the end of the allowed period; kept while an earlier gate (positions 1 to 8) defers the pair or it waits, and through a refused admission; ended when the pair's latest admitted dispatch (its task snapshot's `last_dispatched_unix_ms`) is at or after `due_at`, when an evaluation leaves the pair not eligible (trigger false or unknown, feedback stop, placement block, or no decision for it), when an evaluation passes the allowed period's end, when the instance's delay is off, and when the active catalog generation changes; a host starts with none |
| D1-3 | position 9 of the eligibility closure | rule (no shape change) (host 122) | D1 | D1 | after availability, for an instance whose delay is on: a pair without an entry draws (`due_at` the evaluation time; `min_ms` plus a value uniform in `0..=max_ms - min_ms` from the first eight bytes of the SHA-256 of the cycle's seed, the task, the alias and `due_at`; cut to the earliest of `facts_fresh_until_unix_ms` and the activity window's `until_unix_ms - 1`, 0 when nothing is left); a planned start after now defers the candidate with `dispatch_random_delay_waiting` and a wake at the planned start (a fact refresh in its refresh form, the gate named, no limit); at or before now it is eligible; an instance whose delay is off is not gated and records nothing; a waiting pair does not take the instance |
| D1-4 | `policy.dispatch_intent` `random_delay` `{due_at_unix_ms, planned_start_unix_ms, delay_ms}` | first writer of X14 (no shape change) (host 122) | D1 | D1 | written on the intent of a pair dispatched under an enabled delay (`delay_ms` 0 when the cut left nothing), never on `policy.dispatch_admitted`, `_rejected` or `_completed`, and on no intent of an instance with the delay off; carried from the evaluation to the admission in the trusted dispatch store; the contract adds the draft member and `PolicyPayloadDraft::dispatch_intent_with_random_delay` only; `PolicyDispatchEventData`, `PolicyEvaluation` and `DispatchIntent` are unchanged |
| D1-5 | code `dispatch_random_delay_waiting` (`instance_id`, `task_id`, `due_at_unix_ms`, `random_delay_ms`, `next_eligible_unix_ms`); keys `due_at_unix_ms` (`unix_ms`), `random_delay_ms` (`duration_ms`) | new catalog entries (no shape) (host 122) | D1 | D1 | owner runtime-host (`HostCode`), category info, `ladder_class` `none` (row 7: a scheduling decision reason, not how work on an instance ended), layer host, review pending, since 0.12.5, full text; a waiting gate for the clock-slot records (the default class of a host code in `closure_gate`): a slot waiting for its delay is recorded deferred only once its window has passed |
| D1-6 | direct runs (`actingctl task-run`, MCP `ac_run_pack`), Lab operations, the recovery ladder, startup packages, emulator autostart | unchanged (host 122) | D1 | — | they do not pass the policy evaluation, so they are not delayed; every policy dispatch is (dependency follow-ups, remedy requests and their retries, immediate failure retries, fact refresh reads) |

### 1.136 Slice T12 (0.12.x): centre-hotspot touch sampling, the least-recent landing point and the `touch` keys (#406 T-1, T-2; plan-405-406 Q13 to Q16; Alice 2026-10-13 「…默认都是中心热点…」, 2026-10-14 「…我们选择距离上一次被选择的点击点最久的一个就行…」)

Every box a Runtime run presses is drawn around the box's center; a single-region press draws
`touch.sampling.candidate_count` candidates and lands on the one its instance's landing history
used longest ago (an unused one first). [resource-declarations.md](resource-declarations.md)
("Touch sampling") is the draw's normative text, [runtime-config.md](runtime-config.md) ("The
`touch` section") the keys'. **No persisted ledger shape**: the draws write the registered X11
and X12 (§1.25) for the first time, inside today's `task.semantic` `EffectIntent` `sampling`;
the landing history is memory only, never read from or written to the ledger, and empty after a
restart; the keys are rows of a new section fact `config.parameters.touch` of the existing
family (as P-c's `performance`, §1.24), and of `host.config_overrides`. No code, key, vocabulary
token, location or domain is added: an out-of-range key is the existing `config_value_invalid`
(`out_of_range`), and a one-point box records no code (Alice's ruling).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| T12-1 | `InputSamplingAlgorithm::CenterHotspotNormalV1`, `::CenterHotspotLeastRecentV1` with `InputSamplingEvidence.parameters` (§1.25 X11, X12) | registered values, first written (exec 110) | SH-2 | T12 | built by the contract constructors `InputSamplingEvidence::center_hotspot` (`{sigma_ratio_milli, max_redraws}`, one region per point) and `::center_hotspot_least_recent` (one region, `{sigma_ratio_milli, max_redraws, candidate_count, chosen_candidate}`), both validated as X12 says; a single-region press with `candidate_count` ≥ 2 writes the least-recent draw, with 1 the normal draw; a drag writes the normal draw with its two regions; the seed derivation and its `xorshift64_uniform_rect_v1/run` and `/action` labels are unchanged (domain separators) |
| T12-2 | the kernel's runtime seam `touch_sampling()` (default none) and `choose_touch_candidate(region, candidates)` (default the first) | in-process interface, not persisted (exec 110) | T12 | T12 | asked at each step as `step_pacing()` is; a runtime without touch keys (kernel fakes, the offline Lab until T4) keeps `xorshift64_uniform_rect_v1` (plan Q14); the rule `least_recent_candidate` (first unused pixel, else the oldest use, ties to the lowest index) is the kernel's, the history the runtime's |
| T12-3 | the landing history | memory only (host 123) | T12 | T12 | per instance alias and per box (`x`, `y`, `width`, `height` of the sampling region, device pixels; DA-4's map applies first when it lands); "used" is the same pixel; ordered by a per-instance use counter updated when a point is chosen (plan Q16); at most `points_per_box` pixels per box (the newest) and `max_boxes` boxes per instance (the box unused longest forgotten); policy and direct runs share it; a leaf lock taken under the location `derive_contained_task_action_seed` (no new location) |
| T12-4 | `touch.sampling.sigma_ratio_milli` (`1..=1000`, 250), `.max_redraws` (`0..=64`, 8), `.candidate_count` (`1..=16`, 4), `touch.history.points_per_box` (`1..=256`, 16), `touch.history.max_boxes` (`1..=65536`, 1024) | configuration keys (§5 row; `config.parameters.touch` rows) (host 123) | T12 | T12 | host-wide, every key settable, defaults only in `actingd.defaults.json`, read at each step; `config_value_invalid` (`out_of_range`) at load, in check-config and in `ConfigureSettings` |
| T12-5 | `actingd-config` [5, 7] | interface revision (§6; K27) | T12 | T12 | revision 7 is revision 6 plus the optional `touch` section; a Runtime before T12 refuses a file that names it (the file denies unknown members); raised once for #406's release |

### 1.137 Slice T3 (0.12.x): the box-confined multi-turn drag, generated by the kernel; adb shell input performs segmented plans through `input motionevent` (#406 T-3; plan-405-406 §6, Q17 to Q19; coordinator decisions of 2026-10-19, Q18 (a); Alice 2026-10-13 「…同时支持在特定方框里在一个拖动下拐很多弯的拖动方法」)

A package declares a path drag by its box, its turns, its shortest segment, its duration and its
corner hold; the kernel generates the points from the step's action seed and every backend that
performs segmented swipes performs the drag as one touch; adb shell input becomes such a backend
from Android SDK level 23, so both brake drags run on adb too. **No persisted ledger shape**: the
record is H1 of §1.18 (`InputAction::SingleTouchPathDragV1`), written for the first time, in the
existing `task.semantic` `EffectIntent` action and the input intent's provenance (event action
`input.swipe`); it carries no sampling evidence (an earlier 0.12 build's
`InputSamplingEvidence::validate` refuses sampling on a path drag, plan Q19) and no execution plan
record (as the drag with a pause and a brake). One device location is added (catalog entry, not a
row); no code, key, vocabulary token, wire or configuration key is added; `package` rises to
revision 5. [resource-declarations.md](resource-declarations.md) ("The path drag") and
[component-interfaces.md](component-interfaces.md) (`package` 5) are the contract. Gate: exec key
111, host key 124.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| T3-1 | click `single_touch_path_drag_v1` `{kind, box, turns, min_segment_px, duration_ms, corner_hold_ms}` in a task file, compiled `box_rect`, in a task operation, a navigation edge (to a page or a page set) and a page operation | resource file format (task schema, no version change) (exec 111) | T3 | T3 | the box inside the task's resolution, `x`, `y` ≥ 0, `width`, `height` ≥ 1; `turns` 1..=30; `min_segment_px` 1..=⌊(max(width, height) − 1) / 2⌋; `duration_ms` `turns + 1 ..= (turns + 1) × 60 000`; `corner_hold_ms` 0..=60 000; refused `resource_declaration_invalid` (`InvalidValue` at the member) by the declaration gate in both forms and `contained_task_operation_invalid` by the kernel's click check; source compilation writes `box` as `box_rect`, and a guard synthesized from the click takes the box |
| T3-2 | `InputAction::SingleTouchPathDragV1` (§1.18 H1) | registered value, first written (exec 111, host 124) | SH-1 | T3 | generated from the step's action seed (seed 0's state without one): the first point one draw in the box (hotspot with the `touch` keys, uniform without), each next point the first of up to `max_redraws + 1` draws (one without the keys) at least `min_segment_px` from the previous point, else the box's corner pixel farthest from it (top left, top right, bottom left, bottom right among equals); `duration_ms` split by the segments' whole-pixel lengths (floor, each kept within 1..=60 000 and leaving the later ones 1..=60 000, the remainder to the last); no `sampling` on its effect intent and no `execution_plan` on its input intent |
| T3-3 | the path plan (`SegmentedDragAction::PathDrag`, `prepare_path_drag`) | in-process plan, not persisted (exec 111) | T3 | T3 | press at the first point, each segment along the MAA 2.0 curve over its duration, a hold of `corner_hold_ms` between segments (none when 0), one release at the last point; performed by `nemu_ipc` (which admits it on a frame the Runtime captured, as the brake drags), MaaTouch, minitouch and adb shell input; any other backend refuses it with the segmented swipe capability error; the device replay recorder records it as `single_touch_path_drag_v1` |
| T3-4 | adb shell input's segmented plans (`input motionevent`) and the connect step `input_sdk_level` | device behaviour and location (no shape) (exec 111, host 124) | T3 | T3 | the connect reads `getprop ro.build.version.sdk` after the rotation (an adb failure fails the connect, stage `input_sdk_level`); from level 23 a segmented plan (the path drag and both brake drags) is one `adb shell` invocation of `input motionevent DOWN/MOVE/UP x y` with the plan's waits as `sleep s.mmm`, every point passing the bounds check, its timeout the waits plus 1 s per command plus 2 s; below 23 or with an answer that is no decimal level `supports_segmented_swipe` is false and the plan is refused before any command, never sent as `input swipe`; the timing on a device is checked after deploy (plan §7) |
| T3-5 | the drive reader's `DriveNavigationClick::PathDrag(DrivePathDrag {box_rect})` | in-process reader (no shape) (exec 111) | T3 | T3 | read as a segmented drag (#411 Lab L2's rule, §1.104): exactly its six members and its bounds, else `drive_plan_invalid`; no route passes it, the online observation lists it `gesture_not_projectable` after checking its box, and as a destructive page operation it covers its box; actinglab's reader and the Lab's are T4's |
| T3-6 | location `input_sdk_level` (device, `DeviceStage`, fragment `device.json` `locations`) | registered location (no shape) (exec 111) | T3 | T3 | owner and layer `device`, review pending, since 0.12.6 (the release after v0.12.5, whose catalog snapshot was taken without it), the description of plan §6 |
| T3-7 | `package` [3, 5] | interface revision (§6; K27) | T3 | T3 | revision 5 is revision 4 plus the click; a reader before T3 refuses a pack that names it (unknown click kind); a pack without it is a revision-4 pack; raised once for #406's release |

The host no longer refuses the path drag (H1's `input_action_unsupported`; its test retired, host
key 58 names the replacements), and the foreground gate checks it as a pointer input.

### 1.138 Slice T4 (0.12.x): the Lab path of the touch module: offline runs draw as the kernel from their seed, the Lab presses a box at its seeded hotspot point, the Lab reads and records the path drag (#406 T-4; plan-405-406 Q14, Q20; coordinator decisions of 2026-10-19; Alice 2026-10-19 「Lab的资源制作应当包含所有可用后端的识别以及触控」)

The Lab draws the points of the boxes it presses as a package run draws them, from its own seed and
the touch keys of the root configuration it reads; offline outputs carry the sampling; actinglab's
navigation reader and the Lab bundle read the path drag, recording declares it, and the online Lab
still does not perform it (B-3). **No persisted ledger shape**: every member below is a Lab output,
a Lab file member or an MCP argument; the `sampling` they carry is the effect intent's evidence
(X11, X12 of §1.25, first written by T12) serialized into Lab answers and Lab files only, never
into the ledger. No code, key, vocabulary token, wire or configuration key is added, and no
interface revision rises. [resource-declarations.md](resource-declarations.md) ("The Lab's draw",
"The path drag"), [lab-recording.md](lab-recording.md) ("Drags and long presses", the point rule),
[selection-graph.md](selection-graph.md) ("Offline"),
[contained-lab-operation.md](contained-lab-operation.md) and
[actinglab-capabilities.md](actinglab-capabilities.md) are the contract. Gate: exec key 112, apps
key 79.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| T4-1 | the offline action seed `offline_action_seed(seed, step_index, operation_label)` | rule (no shape) (exec 112) | T4 | T4 | the first eight bytes, big-endian, of SHA-256 over `ActingCommand offline action seed v1`, NUL, the seed (8 bytes BE), the step index (4 bytes BE), the label's length (8 bytes BE) and its bytes; the offline runtimes give it for every step, and the Lab derives each press's seed by it (the press's place in its command, the pressed target, element or edge id; the empty label for a recorded click) |
| T4-2 | `--seed <u64>` of `package dry-run` and a whole `replay` (0 without it), and the touch keys of the root configuration (its `touch` section over the embedded defaults; none outside every root: uniform) | CLI option, rule (exec 112) | T4 | T4 | an offline run draws as the kernel with one candidate (`center_hotspot_normal_v1`, no landing history) or uniformly without touch keys; an unreadable, undecodable or out-of-range configuration refuses the draw (`config_read_failed`, `config_decode_failed`, `config_value_invalid`, exit 2); a scan replay refuses `--seed` (`conflicting`) |
| T4-3 | `OfflineDecision::WouldClick.sampling`, `OfflineReplayEffect.sampling`, `OfflineSimulationResult.seed`, `OfflineReplayResult.seed`, `package dry-run`'s answer `seed` | output members, optional (exec 112) | T4 | T4 | `sampling` is the evidence as the ledger records an effect intent's, absent for a click that draws no box point, so such a decision serializes (and fingerprints) as before; `seed` is present when a click drew from the run's seed (a path drag included); a run given that seed draws every point again |
| T4-4 | `--seed <u64>` of `do` (`--scene`, `--capture --dry-run`, `--capture --record`), `ensure --capture`, `navigate`, `tap-target` and `session recover` | CLI option (exec 112) | T4 | T4 | given, the Lab presses each box at the point drawn from it (`DriveDraw`), not its centre; without it the centre, as before, so every answer without it is unchanged; refused (`validation_failed`, `conflicting`) where nothing is pressed or drawn: online `do <element-id>` (the Runtime presses its projection's point), `--tap` / `--swipe`, a planned route (`ensure` without `--capture`, `--dry-run`), `session recover` with `--stale-capture` or `--startup-login`; the online press stays a plain input |
| T4-5 | `actual_click.seed` and `actual_click.sampling` of `do`; `sampling` of `tap-target`'s answer and of each route step (`ensure`, `navigate`, `session recover`) | output members, optional (exec 112) | T4 | T4 | present only for a drawn press |
| T4-6 | `click.execution.sampling` (`recording.json`), `ClickPlan.sampling`, and the `point_rule` values `center_hotspot_normal_v1` / `xorshift64_uniform_rect_v1` | Lab file member, optional; rule (exec 112) | T4 | T4 | a recorded press's rule names how the Lab pressed: `rect_center` (the centre, no seed), `explicit` (`--tap`, `--swipe`) or the draw's algorithm with its evidence |
| T4-7 | gesture `{"kind": "single_touch_path_drag_v1", "turns", "min_segment_px", "duration_ms", "corner_hold_ms"}` (`recording.json` `click.gesture`) and the request member `click.path_drag {turns, min_segment_px, duration_ms, corner_hold_ms}` | Lab file format (exec 112) | T4 | T4 | the box is the click rectangle, inside the frame; the numbers within T3-1's bounds; conflicting members and bounds `validation_failed`; `record stop` generates the source click (`box`, compiled `box_rect`) and checks its first decision as a path inside the box; a Lab operation does not press it (`record_gesture_execution_unsupported`); it lasts `duration_ms + turns × corner_hold_ms` in the task timeout |
| T4-8 | actinglab's `NavigationClick::PathDrag` and the Lab bundle's `single_touch_path_drag_v1` | in-process readers (no shape) (exec 112) | T4 | T4 | read as T3-5 reads it; the offline page-projection element is `gesture_not_projectable` (`do` → `lab_element_unavailable`), offline `observe` lists its edge with `input: null`, it covers its box as a destructive page operation, the Lab port keeps refusing it |
| T4-9 | MCP `ac_lab_do` argument `seed` (integer ≥ 0) | MCP argument (apps 79) | T4 | T4 | passed as `--seed`; the answer is actinglab's, `actual_click.seed` and `actual_click.sampling` included; the description says so |

The capability listing's touch-backend gestures (L4-3 of §1.120) were stale since T3: every
segmented backend performs the path drag, and `adb_shell_input` performs the three segmented drags
from Android SDK level 23. T4 corrects the listing and its test (exec 98); the L4-3 row stands for
its shape.

### 1.139 Slice A24 (0.12.x): ledger event time is not monotonic in sequence; the latest time is the maximum timestamp (#411 A24)

A24: `timestamp_unix_ms` is the producer's capture or occurrence time, not the append time, so it
is not monotonic in `sequence` (a run's diagnostic `artifact.created`/`verified` carry its start
time and are appended at its end); order by `sequence`, let time bounds filter records but never
end a scan, and take the latest time as the maximum timestamp, not the last record's. The
readers that took the last record's time as the latest time follow the rule. Rule only: no
persisted ledger shape, no wire, key, configuration or code change.
[ledger-observer.md](ledger-observer.md) (§8.1, §10), [global-ledger-query.md](global-ledger-query.md),
[ledger-store.md](ledger-store.md), [policy-suspension.md](policy-suspension.md)
(`clock_slots`), [emulator-control.md](emulator-control.md) (direction ①) and
[runtime-fact-store.md](runtime-fact-store.md) (offline read) are the contract.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A24-1 | the reference time of `clock_slots` in `actingd suspended` | rule (no shape change) (host 125) | A24 | A24 | the maximum `timestamp_unix_ms` of the snapshot, not the last event's; the 48 h window and the settling of a record without a next dispatch use it, so a last record stamped earlier (a run's diagnostic `artifact.created`) no longer leaves a slot `pending` that the latest time has passed |
| A24-2 | direction ①'s failure-burst window (memory only) | rule (no shape change) (host 125) | A24 | A24 | the window keeps its counted failures in time order and counts those within the item's `window_ms` of its latest failure, whatever order their records were committed in; the burst's `first_unix_ms` and `last_unix_ms` are the earliest and latest of them (never first after last); the same records in time order and out of it give the same count and the same burst; records in time order count exactly as before |
| A24-3 | `gap_start_unix_ms` of `ledger.gap_recorded` `tail_unconfirmed`, `dump_incomplete` and `dump_invalid` | rule (no shape change) (ledger 50) | A24 | A24 | a tail gap starts at the maximum timestamp of the committed (or imported) records, a dump gap at the maximum capture time of the verified dump records (the dump's start when none verified), not at the last record's |
| A24-4 | `taken_at_unix_ms` of the offline `actingledger facts --at <position>` | rule (no shape change) (ledger 50) | A24 | A24 | the maximum timestamp of the events at or before `position`, not the timestamp of the event at it; read as the position's event and then the events through it stamped later (a time filter, the scan bounded by the position) |
| A24-5 | the scripted Runtime's ledger (`runtime-client` `test_support`, feature `test-support`) | test fake (no shape change) | A24 | A24 | its `to_timestamp_unix_ms` bound is exclusive, as the contract and the ledger's query are; no current test of it queries with an upper time bound |

### 1.140 Slice A25 (0.12.x): `standard-pack check` admits every registry package, with or without the navigation root files (#411 A25)

A25: the standard-pack cross-check admits every package of the registry (`bundle.json`) once,
by its directory, with or without `pages.json`, `interrupts.json` and `navigation.json`. No
persisted ledger shape, no code, no wire, key or configuration change; the answer shape is
unchanged. [package-reference.md](package-reference.md) ("Standard-pack root files") is the
contract.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A25-1 | `actinglab standard-pack check --root <game root>` and the cross-check `package bundle` runs with root files | rule (no shape change) (exec 113) | A25 | A25 | every registry package is admitted once by its directory with the cross-check's admission (`PreparedContainedTask::describe_navigation_path`, its deadline and instance label), also on a root whose only root file is `bundle.json`; a package that fails admission, or whose directory is missing, is refused `package_invalid` naming its directory (exit 2), as a registry package no root file names is refused with `pages.json`; `status: checked` is answered only once every package was admitted, and `pack_count` stays the registry's length; with `pages.json` the order, the refusals and the warnings are unchanged and each package is admitted once; `package bundle` admits each registry package or takes its admission by path, never skipping one; before, a root without `pages.json` was answered `checked` with only the remedy report's metadata read of its packages, which is no admission |

### 1.143 Slice MCP-19a (0.12.x): the latest evaluation says why each candidate did not run (`ScheduleExplanation`, memory only) (#386 SR-6b, MCP-19; plan-mcp19 §2, §6, §7 Q1, Q5–Q11)

MCP-19a: the policy host keeps the latest evaluated cycle's not-run explanation in memory, and the
general read `ScheduleExplanation` (§3 W5) answers it under `runtime-client` 2 (§6).
[scheduling/README.md](scheduling/README.md) ("Why A Candidate Does Not Run", "Where a person sees
it") and [runtime-interface.md](runtime-interface.md) ("New request kinds") are the contract. The
table and the read write nothing to the ledger and no decision reads them; `TaskDecision`'s new
member is never serialized, so every serialized evaluation is unchanged; the wire gains one
request kind and its result and changes no existing shape. Gates: policy key 56, host keys 126 and
127. The actingctl and MCP outputs are MCP-19b's (§1.144).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| M19A-1 | `TaskDecision.next_wake_unix_ms: Option<u64>` (`#[serde(skip)]`) | Rust API, not serialized (policy 56) | MCP-19a | MCP-19a | the decision's own wake as the evaluator computes it (`TaskWork.next_wake_unix_ms`); the evaluation's `next_wake_unix_ms` stays the least of the decisions' wakes and the catalog's own (timeline, pool freshness); the host's decision for an unknown priority-offset task has none, and so does a decision read back |
| M19A-2 | the not-run explanation table (`crates/runtime-host/src/schedule_explanation.rs`, held by `PolicyHost`) | memory-only state, no shape (host 126) | MCP-19a | MCP-19a | replaced by every evaluated cycle from the bound evaluation, before the host adds its notes (`eligible_since_unknown`, `policy_instance_fact_missing` and the unknown-offset decision are not in it); a deferred cycle or a failed evaluation leaves it; a start begins with none. Per instance with a decision, by alias: its earliest decision wake; the selected decision `{task_id, decision_id, dispatch_state}`, the state read when the request is answered from the dispatch projection (`pending` when it holds none, `intent`, `admitted`, `rejected`, `completed`); every other decision as a candidate row `{task_id, state, eligibility, why, reasons, reasons_omitted?, next_wake_unix_ms?, since_unix_ms}` (`state` `blocked` for a blocked decision, `deferred` for every other, an unselected eligible one included; `why` the full outcome of its last reason; `reasons` its reason codes in order, the last 32, ending with `why`'s), due rows (eligibility `true`) first by total score, highest first, then unranked, ties by task id, then the rest by wake, none last, then task id; and one not-running row, the first due row's `{task_id, why, next_wake_unix_ms?, since_unix_ms}` or, with no due row, `{why: due_task_missing {instance_id, next_eligible_unix_ms?}, since_unix_ms}`. `since_unix_ms` is the first evaluation of an unbroken run with the same (instance, task, `why` code), for the not-running row the same (instance, task or none, `why` code) |
| M19A-3 | `RuntimeOperation::ScheduleExplanation { instance?, candidates }` → `RuntimeResult::ScheduleExplanation { explanation }` (W5) | runtime-client 2 (§3) (host 126, 127) | MCP-19a | MCP-19a | a general read from the policy host's memory under the policy lock (U-1 row: General, Read, no G14 copy); nothing read from or appended to the ledger. `explanation` is `RuntimeScheduleExplanation {evaluated_at_unix_ms?, catalog_hash?, instances}` (both optional members absent and no entry before the first evaluated cycle after a start), each entry a `ScheduleExplanationInstance {instance_alias, next_wake_unix_ms?, selected?, not_running, candidates, candidates_total}` with M19A-2's rows (`candidates` omitted when empty or not asked for, `candidates_total` counting every row); `instance` is checked with the alias rule and must be an alias the registry holds (else `instance_unknown`, denied), and the answer then holds its entry only (none when the latest evaluation had no decision for it); bounds 256 instances, 1 024 candidates per instance (the rest counted), 32 reason codes per row; a not-running row has no task exactly when its `why` is `due_task_missing`; a wrong shape is `invalid_schedule_explanation`; a reply past one IPC frame is refused `runtime_schedule_explanation_too_large_for_frame` (denied); a completed receipt carries `request_schedule_explanation_completed`; client `RuntimeClient::schedule_explanation(instance, candidates)` |
| M19A-4 | the six catalog entries below | new catalog entries, no shape (host 127) | MCP-19a | MCP-19a | review `pending`, ladder class `none` (row 7: a read's success or refusal, a contract refusal), since 0.12.5, full text |

The codes `request_schedule_explanation_completed` (success) and `invalid_schedule_explanation`
(error) (contract), `runtime_schedule_explanation_too_large_for_frame` (error) (runtime-host) and
`runtime_schedule_reported` (success; actingctl, emitted from MCP-19b on), and the locations
`read_schedule_explanation` (runtime-host) and `runtime_schedule_explanation` (runtime-client), are
catalog entries, not rows (§7). `due_task_missing` (info, pending, pre-registered by SR-0 under
runtime-host and never emitted before) moves to the contract registry
(`ScheduleExplanationRowCode`, domain `due_task_` with it; coordinator decision of 2026-10-20), so
the answer's validation can name it; the host emits that typed code from MCP-19a on. No settled
entry changes. No persisted ledger shape.

### 1.126 Slice RV-1 (0.12.x): `actinglab resource verify` checks a resource repository commit as its CI does (#411 RV-1; plan-rv §4, §5, §8, §9; coordinator decisions of 2026-10-19 D1, D2)

RV-1: one actinglab command checks one commit of a resource repository in one process, with this
build's checks, and writes a report bound to the commit, its tree, its base and the build;
`resource check-release`, a stub that answered `checked` without checking, is retired (O5).
[resource-verify.md](resource-verify.md) is the contract, with one sentence each in
[resource-declarations.md](resource-declarations.md), [package-reference.md](package-reference.md),
[actinglab-capabilities.md](actinglab-capabilities.md) and a row in
[actinglab-dry-run.md](actinglab-dry-run.md). No persisted ledger shape, no wire, key or
configuration change: the report is operation output and no persisted record; the codes, value
keys and vocabularies are catalog entries, not rows (§7). The only new dependency is the `git`
executable on `PATH`, at run time, for this one command.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| RV1-1 | `actinglab resource verify --repo --source-repository --work --report [--commit] [--base]` | new command (exec 103) | RV-1 | RV-1 | the six flags only (`validation_failed` otherwise, a positional argument too); the device, Runtime and instance global options `offline_device_scope_forbidden`; global `--dry-run` `dry_run_unsupported` after flag parsing; an existing `--work`, `--report` or `<report>.part`, or a missing parent, `resource_verify_output_exists` before anything is read; steps `tools`, `checkout`, `declarations`, `pins`, `admission`, `format`, `bundle` in that order, every step runs, only a failed `checkout` leaves the content steps `not_run`; git read-only from `PATH` with `GIT_OPTIONAL_LOCKS=0`; writes only below `--work` and the report, deletes nothing; exit 0 on success, exit 2 otherwise |
| RV1-2 | the report `actinglab.resource-verify.v1` `{schema_version, result, outcome, started_at_unix_ms, finished_at_unix_ms, repository {root, commit, tree, base, source_repository, game}, tools {cli_version, runtime_version, runtime_head, executable, executable_sha256, build_manifest}, work, steps [{step, result, duration_ms, counts, failures, warnings}], failed_steps}` | operation output (no persisted record) (exec 103) | RV-1 | RV-1 | written to `<report>.part`, then renamed, after the last step, on success and on failure, the precondition's failure included; the CLI answer's data (success) or `details` (failure) is the same object; failures and warnings are `actingcommand.outcome.v1` envelopes, full detail; the counts of each step as the contract lists them |
| RV1-3 | the work directory `<work>/packs-root/`, `bundle/`, `out/<game>-resources-<sha7>.zip` and `out/SHA256SUMS`, `read-back/<game>/`, `installed/<game>/` | operation output (no persisted record) (exec 103) | RV-1 | RV-1 | the zip's entries in ordinal order with `interfaces.json` `package` [3, 3] validated with the build's head; `standard-pack check` runs on the installed root |
| RV1-4 | capability entry `resource verify` (`offline`, `available`, dry run `refused` `dry_run_unsupported`) with the member `report_schema: "actinglab.resource-verify.v1"`; `resource check-release` `retired` with `reason_code` `resource_check_release_retired` | capability member (no shape change) (exec 103) | RV-1 | RV-1 | the wrapper reads `report_schema` to know the pinned Tools can verify; help `command_options` and `list commands` list the command; `ratchet/actinglab_commands.json` gains `resource verify` |
| RV1-5 | codes `resource_verify_passed` (success; `step_count`), `resource_verify_failed` (`failed_step_count`), `resource_verify_output_exists` (`path`), `resource_verify_git_failed` (`git_command`, `exit_code?`), `resource_verify_checkout_invalid` (`repo`, `checkout_problem`), `resource_verify_checkout_dirty` (`path_count`, `first_path`, `dirty_problem`), `resource_verify_base_invalid` (`base`, `base_problem`), `resource_verify_tools_mismatch` (`manifest_path`, `tools_problem`), `resource_verify_layout_invalid` (`path`, `layout_problem`), `resource_verify_pin_committed` (`path`, `json_pointer`, `member`), `resource_verify_bundle_mismatch` (`path`, `bundle_problem`), `resource_verify_readback_mismatch` (`entry`, `readback_problem`), `resource_verify_report_write_failed` (`path`), `resource_check_release_retired` (no value); every one but the first and the last two with `step?` | new catalog entries (no shape) (exec 103) | RV-1 | RV-1 | owner actinglab (registry `ActinglabCode`, block `#411 RV-1`), category error unless noted, `ladder_class` `none` (row 7: a Lab CLI command's results), layer `process`, review `pending`, since 0.12.4, full text; the prefix `resource_` already covers actinglab |
| RV1-6 | keys `step` (vocabulary `resource_verify_step`), `step_count`, `failed_step_count`, `path_count` (integer), `git_command`, `checkout_problem`, `dirty_problem`, `base_problem`, `tools_problem`, `layout_problem`, `bundle_problem`, `readback_problem` (vocabularies of the same names), `repo`, `manifest_path` (path), `first_path` (relative path), `base`, `member` (name), `found_after_first` (boolean), `unformatted` (list of relative path) | new key-table and vocabulary entries (no shape) (exec 103) | RV-1 | RV-1 | the vocabularies are built from actinglab's enums in `codes.rs` (G7); `tools_problem` adds `manifest_invalid` to the plan's two tokens |
| RV1-7 | the optional keys the reused codes carry: `resource_declaration_invalid` (`path`, `json_pointer`, `found_after_first`, `step`), `package_invalid` (`path`, `step`), `package_fmt_check_failed` (`path`, `unformatted`, `step`), and `step` on `validation_failed`, `standard_pack_root_invalid` and the nine `package_bundle_*` errors | optional values of pending entries (no shape) (exec 103) | RV-1 | RV-1 | `resource verify` attaches only the values the embedded catalog declares for a reused code, so another code a step relays carries no value it does not declare |
| RV1-8 | `resource check-release` | retired command (exec 103) | RV-1 | RV-1 | refused with `resource_check_release_retired` (exit 2), whose catalog text names `resource verify`; dry run `no_effect` (retired) |

### 1.134 Slice W1 (0.12.x): the catalog predicate `fact_time_reached`, true once the time a fact holds plus an offset has come; the scheduler wakes at it (#405 W1; plan-405-406 §2, §6, §8 Q1–Q5)

W1: a V2 catalog predicate reads the `timestamp_ms` value of one fact and is true once that time
plus a signed offset has come; before it the evaluation wakes at that instant, and a fact that
gives no time makes the predicate false with its reason, never a fact refresh.
[scheduling/README.md](scheduling/README.md) ("Fact-time wake", "Why A Candidate Does Not Run")
and [scheduling/v2/README.md](scheduling/v2/README.md) are the contract. **No persisted ledger
shape**: the predicate is a catalog format addition, and the ledger holds only the catalog's
`catalog_hash`; a Runtime before W1 refuses a catalog that names the kind, so the resource side
ships such a catalog only after W1 is deployed; the reasons are catalog codes carried by the
evaluation's decisions and the existing dispatch decision records; no wire, configuration key or
interface revision changes. Gate: policy key 53.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| W1-1 | predicate `{"kind": "fact_time_reached", "scope", "fact_key", "offset_ms", "max_age_ms"}` (`PredicateSpec::FactTimeReached`, `contracts/scheduling/v2/common.schema.json`) | catalog format addition (no ledger shape) (policy 53) | W1 | W1 | V2 only (a V1 document naming the kind: `unknown_field` at the predicate's `/kind`); every member required, `max_age_ms` nullable; `scope` and `fact_key` checked as `fact`'s; `offset_ms` within `MIN_FACT_TIME_OFFSET_MS..=MAX_FACT_TIME_OFFSET_MS` (−86 400 000..=86 400 000) and `max_age_ms` null or `1..=MAX_FACT_MAX_AGE_MS`, else `limit_exceeded` at the member; a catalog without it compiles, serializes and hashes as before |
| W1-2 | the predicate's truth and wake | rule (no shape) (policy 53) | W1 | W1 | true when the fact exists, confidence above 0, fresh as `fact` (`max_age_ms`, its own expiry), value `timestamp_ms` T, and now ≥ T + `offset_ms` (signed, saturating at 0); its freshness and wake are `fact`'s (freshness end, +1 for the wake); not reached: false, wake at min(T + `offset_ms`, freshness end + 1); unusable: false, no suggestion, no fact refresh, no wake; under `any`, `all` and `not` as any predicate; no counted clock, no outcome key; a template's predicate needs the report game's scope; an instance-scope read for `fact_reader_missing` |
| W1-3 | codes `fact_time_pending` (info; `fact_key`, `next_eligible_unix_ms`), `fact_time_unavailable` (info; `fact_key`, `fact_time_problem`) | new catalog entries (no shape) (policy 53) | W1 | W1 | owner policy (registry `PolicyCode`, block `#405 W1`), `ladder_class` `none` (row 7: a scheduling decision reason, not how work on an instance ended), layer `policy`, review `pending`, since 0.12.5, full text; the prefix `fact_` already covers policy |
| W1-4 | key `fact_time_problem` (vocabulary `fact_time_problem`: `missing`, `low_confidence`, `stale`, `expired`, `not_a_time`) | new key-table and vocabulary entries (no shape) (policy 53) | W1 | W1 | the vocabulary is built from the policy enum `FactTimeProblem` in `codes.rs` (G7) |

### 1.141 Slice Lab L5c (v0.12.x): the Lab's `--capture` client forms take the frames target consensus needs (#411 Lab L5c; G8; coordinator decision Q8 (a) of the L5 plan, Q2; Alice 2026-10-19, 「Lab的资源制作应当包含所有可用后端的识别以及触控」)

`do --capture --dry-run`, `ensure --capture` and `wait --capture` decide in the client on raw
Runtime frames. They take the frame set a package's target consensus needs, as offline `--scene`
(§1.131) and online observations (§1.130) do, and check the set as a keyless `observe` is
checked. [contained-lab-operation.md](contained-lab-operation.md) and
[business-identity-consensus.md](business-identity-consensus.md) are the contract. Rules, one
answer member and one Rust API method; the codes are existing ones: `observation_samples_interrupted`
(runtime-host, reused in actinglab's `details.error`), `validation_failed` with its existing
`argument_problem` token and keys, and `device_error`.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| L5C-1 | the frames of each observation of a `--capture` client form: do's one, ensure's start, every route guard and every arrival poll, every poll of `wait --page` and `wait --stable` | rule (no shape change) (exec 114) | Lab L5c | Lab L5c | with F = `target_sample_frames()` > 1, F read-only Runtime captures (`ObserveReadonly`, each a capture of its own), `target_sample_interval_ms()` apart (unscaled, decision Q2), oldest first, the last the current frame; page detection, every target (do's target, a route edge's target, the stable target) and the destructive regions are evaluated over the set as offline frames are (`detect_current_page_over`, `evaluate_target_over`, `destructive_frame_regions_over`), so target consensus reads its last required frames; the click, the coordinates, `frame_coordinates`, `frame_age_ms`, `backend` and the input's frame (L3-5) are the current frame's; a wait poll takes one whole set and the timeout is checked between sets, so an observation takes at most F ≤ 5 frames (decision Q5's per-observation bound); a package without target consensus takes one capture and its answers are unchanged |
| L5C-2 | the input check of a client frame set | rule (no shape change) (exec 114) | Lab L5c | Lab L5c | the client holds no lease across its captures; after the last capture it makes two queries through `RuntimeDebugSession::query_event_page`: the first capture's `capture.requested` event (by its request ID; its sequence and instance), then one `input.intent` on that instance from that sequence to the last capture's terminal sequence, limit 1, as the host's `check_observation_sample_inputs` (L5-5); one found: the form fails `device_error` (exit 4) with `details {error: "observation_samples_interrupted", state: "failed", hint}` before anything is decided on the frames; no new code |
| L5C-3 | frames of a client set whose sizes differ | rule (no shape change) (exec 114) | Lab L5c | Lab L5c | after the input check, as offline frames (B2-2): `validation_failed`, exit 2, details `{argument: "--capture", argument_problem: "unusable", index}`, the position (0 the oldest) of the first earlier frame whose size differs from the current frame's |
| L5C-4 | `recognition_coverage {provided_frames, required_frames, aggregation: "production_owner"}` on the answer of `do --capture --dry-run` | new answer member (exec 114) | Lab L5c | Lab L5c | as offline `do` (B2-3): present only when the package declares target consensus, `provided_frames` = `required_frames` = F; absent otherwise, so a single-frame answer is unchanged |
| L5C-5 | `RuntimeDebugSession::query_event_page(query, profile, page)` | Rust API method added (no wire change) (exec 114) | Lab L5c | Lab L5c | one page of the shared ledger view as `RuntimeClient::query_event_page` reads it, not bound to the session's correlation; the existing `QueryEvents` request |

No persisted ledger shape: no ledger record, wire shape, configuration key, interface revision
(§6) or outcome code changes.

### 1.142 Slice Lab RP (v0.12.x): the Lab's single-step select evaluations read the run parameters as a run does (#411 Lab RP; model-b15 §6; [remedies.md](remedies.md) section 4; Alice 2026-10-19, 「Lab的资源制作应当包含所有可用后端的识别以及触控」)

Lab RP closes R5's accepted deviation 3 (§1.115 B15R-3): the single-step evaluations
`dry_run_select` and the scan replay read `session.dispatch.parameter.<name>` through R5's own
overlay (`RunParameters::overlay`, B15R-1), the package's declared defaults as a run without a
dispatch reads them, and the scan replay takes given values in their place.
[remedies.md](remedies.md) section 4, [instance-fact-store.md](instance-fact-store.md) ("Run
parameters") and [selection-graph.md](selection-graph.md) ("Offline", where `actinglab replay`
is specified) are the contract. **No persisted ledger shape**: the overlay is never stored, and
nothing is recorded for a Lab evaluation. No wire, configuration key, outcome code, key or
vocabulary token: the refusals are actinglab's existing `validation_failed` (exit 2) with the
existing `argument_problem` tokens. A package that declares no parameter evaluates as before.
Gate: exec key 115.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| RP-1 | `PreparedContainedTask::given_run_parameters(given) -> Result<RunParameters, GivenParameterRefusal>`, `GivenParameterRefusal {name, problem}`, `GivenParameterProblem` (`Undeclared`, `NotAnOption`, `Repeated`) | kernel interface (memory only, not persisted) (exec 115) | Lab RP | Lab RP | every declared parameter with its declared default, each given `(name, value)` replacing its default as a dispatch's chosen value does, observed at the snapshot's instant; without values it equals `default_run_parameters()`; a name the package does not declare (any name, on a package that declares none), a value outside the parameter's declared options and a name given twice are refused, never ignored |
| RP-2 | `dry_run_select(task, operation_id, frame, (snapshot, now_unix_ms), run_parameters, policy_override)` | changed kernel interface (Rust API; no wire) (exec 115) | Lab RP | Lab RP | takes the instance fact snapshot (was a `SelectionFactSnapshot`) and the run parameters, overlays them, and evaluates the rows of the record lists the evaluated policy declares; no Lab verb calls it yet (the `actinglab select` tool of a later slice) |
| RP-3 | `replay_scan(task, operation_id, scan, frames, facts, run_parameters)` | changed kernel interface (Rust API; no wire) (exec 115) | Lab RP | Lab RP | the overlay is added to the given or the empty offline snapshot before it is checked, as a run's select-step snapshot holds it |
| RP-4 | actinglab `replay --select <operation> --scan <file> --parameter <name>=<value>` | new CLI option (repeatable, one flag per distinct name) (exec 115) | Lab RP | Lab RP | the value replaces that parameter's declared default; without `--parameter` the declared defaults; listed in help `command_options` `replay`; the answer's shape is unchanged |
| RP-5 | the refusal of `--parameter`: `validation_failed`, exit 2, details `{argument: "--parameter", argument_problem, arguments: [{argument, argument_problem}]}` | new details of an existing code (exec 115) | Lab RP | Lab RP | before anything is read: a flag without its value `value_missing`, a value that is not `<name>=<value>` `not_recognised`, a name given twice `repeated` (#411 Lab B-2's rule, decision Q9), `--parameter` on a whole replay (without `--select`) `conflicting`; once the package is admitted and before any frame is read: a name the package does not declare (any name, on a package that declares none) or a value outside its declared options `not_recognised` |
| RP-6 | MCP `ac_lab_observe` / `ac_lab_do` | unchanged (no shape change) | Lab RP | — | neither reaches a single-step select evaluation (offline `observe` projects candidate layouts and evaluates no selection policy), so neither gains a `parameters` member |

No persisted ledger shape; Rust API and Lab CLI only; no outcome code, wire, configuration key
or interface revision (§6) changes.

### 1.144 Slice MCP-19b (0.12.x): a person and an agent see why each task did not run (`actingctl schedule` ↔ `ac_schedule`, overview `not_running`, diagnose `not_run`) (#386 SR-6b, MCP-19; plan-mcp19 §2, §7 Q2, Q4, Q8, Q9)

MCP-19b: actingctl and MCP show MCP-19a's read `ScheduleExplanation` (§1.143, §3 W5).
[runtime-interface.md](runtime-interface.md) (actingctl and MCP tables) and
[runtime-config.md](runtime-config.md) (`scheduling.not_run.diagnose_rows`) are the contract.
Output only: every read records nothing in the ledger, `actingctl status` and
`RuntimeInstanceStatus` are unchanged (W13 stays reserved, Q2). No persisted ledger shape; no
code, key, wire or interface revision added (the one code emitted, `runtime_schedule_reported`, is
MCP-19a's; `config_facts_missing`, `invalid_fact_snapshot_record`, `output_budget_exceeded` and
the selector refusals exist). Gates: apps keys 80 and 81.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| M19B-1 | `actingctl schedule [--instance <alias\|instance_id\|ADB port>] --state-root <path>` | new command (no shape) (apps 80) | MCP-19b | MCP-19b | one ScheduleExplanation read with `candidates: true`, after one status read that resolves a given `--instance` to its alias (`instance_unknown`, `instance_ambiguous` as `fact-refresh`); stdout is the answer verbatim with `outcome` `runtime_schedule_reported`, exit 0; actingd absent: the client's failure, exit 1; any other flag `actingctl_usage_invalid` |
| M19B-2 | MCP `ac_schedule {instance?}` | new observer tool (no shape) (apps 80) | MCP-19b | MCP-19b | read-only, idempotent; the CLI's object without `outcome`; past the output budget it cuts candidate rows only, from the last row of the last entry backwards, each entry that lost rows marked `candidates_truncated: true`; `evaluated_at_unix_ms`, `catalog_hash` and each entry's `instance_alias`, `next_wake_unix_ms`, `selected`, `not_running` and `candidates_total` are never cut; still too large: `output_budget_exceeded`; another argument `arguments_invalid` |
| M19B-3 | `not_running` on each `ac_overview` / `actingctl overview` instance row | new output member (no shape) (apps 81) | MCP-19b | MCP-19b | when `instances` is answered, one ScheduleExplanation read with `candidates: false` (the selected instance or every one); a row whose alias has an entry gains that entry's `not_running` verbatim, a row without one has no member; a failed read is one warning (the client's failure) and `incomplete`, and no row has the member; never cut; no new section |
| M19B-4 | `not_run`, `not_run_total` of `ac_diagnose` / `actingctl diagnose` | new output members (no shape) (apps 81) | MCP-19b | MCP-19b | one ScheduleExplanation read with `candidates: true` (the diagnosed instance or every one) and one RuntimeFactSnapshot read: per entry in the answer's order, the candidate rows whose `state` is `deferred`, at most N = `scheduling.not_run.diagnose_rows` of `config.parameters.scheduling` per instance, each with `instance_alias`; `not_run_total` counts them all when one was left out (N = 0 gives an empty list); a failed read, or a section without the row (`config_facts_missing`; a value that does not read back `invalid_fact_snapshot_record`), is a warning (the CLI line's `warnings`, MCP's warnings) and `incomplete`, with no `not_run`; MCP cuts `not_run` from its end after `clock_slots` and before `errors_page` (`not_run_truncated`) |

### 1.145 Slice A26 (0.12.x): a dispatch decision record says why each candidate it lists did not run, as values (#411 A26; #386 SR-6a)

A26: since A2-P (Q7) a dispatch decision record persisted `decision_candidate_not_selected`
with `task_id` only, and the `why` that #386 SR-6a names was developer text that nothing
persisted (there is no developer log of it). Each listed candidate now carries `why`,
`decision_state` and `eligibility` as values, and `decision_record` counts the unlisted ones by
`why` in `omitted_why`; this amends row Q7. No persisted ledger shape change: the record
envelope `{code, category, values}` and every event payload member are unchanged, and earlier
0.12 readers accept any `values` map (`PolicyReasonRecord` is `#[serde(transparent)]` over
`Outcome`, whose reader keeps `values` as a JSON map without a key check, and a received
outcome is never checked against the catalog). The new value keys and vocabularies are catalog
entries, not rows (§7). [scheduling/README.md](scheduling/README.md) ("Dispatch Decision
Record", "Why A Candidate Does Not Run") is the contract.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A26-1 | `decision_candidate_not_selected` in the `reasons` of `policy.dispatch_intent`, `_admitted`, `_rejected` and `_completed` (Q7) | new values of a pending entry (no shape change) (policy 57) | A26 | A26 | `why` (code, required): the candidate's last reason, the one that settled it, never `none`; `decision_state` (vocabulary `decision_state`: `eligible`, `deferred`, `blocked`, `selected`, required): its decision's state; `eligibility` (vocabulary `eligibility`: `true`, `false`, `unknown`, required): its decision's eligibility; for every listed candidate, ranked or not, equal to the detail's `why=`, `state=` and, for a candidate that was not ranked, `eligibility=`; a `why` the embedded catalog does not list fails the evaluation (`policy_evaluation_input_invalid`) instead of a record without it; the ranking terms of a ranked candidate (`total`, `behind_by`, `priority`, `aging_ms`, `strategic_milli`, `urgency_milli`, `contention`, `effective`, `score`, `utility`, `offset`, `target`, `promoted`, `tie_breaker`) have no keys in the catalog and stay developer text |
| A26-2 | `decision_record` in the same `reasons` (Q7) | new optional value of a pending entry (no shape change) (policy 57) | A26 | A26 | `omitted_why` (records `{why, count}`, optional): the related candidates the record does not list, counted by `why`, one record per code in ascending code order, without the detail's 1010-byte fold (64 records, then `omitted_why_total`); left out when every related candidate is listed, so a record that lists them all is unchanged |
| A26-3 | keys `why` (code), `decision_state`, `eligibility` (vocabularies of the same names), `omitted_why` (records of `why`, `count`) | new key-table and vocabulary entries (no shape) (policy 57) | A26 | A26 | the vocabularies are built from the policy crate's enums in `codes.rs` (G7), review `pending`; the two codes keep `since` 0.12.0, their text names 0.12.5 |
| A26-4 | the developer detail of the decision record's reasons | contract text (no shape change) | A26 | A26 | unchanged; it is kept only in the in-memory decision. The sentence that it goes to a developer log is removed: no such log line exists |

### 1.146 Slice BT-B2 (0.12.x): two values compared in a selection policy; `ocr_integer` reads the current or the capacity of `a/b` (#411 B2; b-plan §2.2)

B2: a selection policy may compare two integer values with the predicate `integer_compare`, and
an `ocr_integer` candidate feature may read the right-hand part of a `current_capacity` text. Both
are package members of `package` revision 6, which BT-PK6 declares at the end of the B train;
until then this Runtime admits them by code and the declaration stays [3, 5]. No persisted ledger
shape, no code, no wire, key or configuration change: validation uses existing
`SelectionErrorCode`s and the existing layout refusals. B1's dictionary on `ocr_integer` (b-plan
D1) is not part of this slice. [selection-policy.md](selection-policy.md) ("Two values compared",
"Upper bound") and [business-identity-consensus.md](business-identity-consensus.md) are the
contract.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B2-1 | selection-policy document predicate `integer_compare {left, op, right, mul?, add?}`, `op` one of `eq`, `ne`, `lt`, `le`, `gt`, `ge` | new predicate (resource file format, not a ledger shape) (policy 58) | BT-B2 | resource side | holds when `left op right × mul + add`, compared in 128 bits as a value test compares; both sides declared `integer` values (`type_mismatch` otherwise); `mul` 1..=1000000, default 1, and `add` within ±(2^53 − 1), default 0 (`integer_out_of_range` otherwise); an unknown side leaves the predicate unknown and the gate's `on_unknown` decides, the left side's reason first; the collect option lists each unknown side; the upper bound compares the sides' ranges; a document that leaves `mul` and `add` out is serialised without them, and one without `integer_compare` keeps its canonical bytes, `policy_sha256` and `input_sha256` (so the scheduling consumer's `catalog_hash` too); a reader before B2 refuses it (unknown predicate kind); `package` revision 6 |
| B2-2 | candidate layout feature `ocr_integer`: `integer.part` (`current` or `capacity`) | new optional member (resource file format, not a ledger shape) (exec 116) | BT-B2 | resource side | only with `format` `current_capacity` (otherwise the layout is refused, `InvalidValue`, at parse and pack load); `capacity` reads the right-hand part of `a/b`, `current` and no `part` the left-hand part as before; the range applies to the part read, so a scan's field domain stays `min..=max`; the derived pack copies `integer` as declared; a reader before B2 refuses it (`integer` denies unknown members); `package` revision 6 |

### 1.147 Slice BT-LAB (0.12.x B train, group G0): a page-set entry of offline `observe`'s `actions` carries a normalised `input`; Lab arrival samples start on a fixed period (#411 A13 B part, A18 option ④; plan-B §2.16, §2.17)

A13 B part: offline `observe` lists an operation whose `to` is a page set in `actions` with
`input` next to its declared `click`, built from its generated navigation row as an edge's
`input` is; A13's A part (§1.104 A13-1) left `actions` unchanged.
[resource-declarations.md](resource-declarations.md) (the navigation paragraph of Workflow #390
LAB-2) is the contract. A18 option ④ (Alice's ruling on item 14, question 13; Lab B-3, #412,
answers A18 fully later): online `do`'s arrival samples start on a fixed period instead of one
fixed sleep after each evaluation; [contained-lab-operation.md](contained-lab-operation.md) (the
arrival paragraph) is the contract. No persisted ledger shape, no wire, key, configuration or
interface revision (§6), no outcome code. Gates: exec key 117, host key 128.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| L13-1 | offline `observe`'s `actions` entry of an operation whose `to` is a page set: `{id, task_id, from_page, to_pages, click, input}` | new member of a Lab answer entry (operation output, no persisted record) (exec 117) | BT-LAB | BT-LAB | `input` from the operation's row in the generated navigation (same `id` and `from_page`, a page set), built as an edge entry's `input` (`semantic_input_json` of the navigation reader's click; no coordinate resolved on the frame), so it equals the `input` of an edge with the same click; `null` for a segmented or path drag, as an edge's, and when the navigation holds no such row (a package compiled from source always holds one); `click` stays the task file's declared click; every other entry unchanged |
| L18-1 | the wait between two arrival samples of online `do` | rule (no shape change) (host 128) | BT-LAB | BT-LAB | a sample starts `LAB_ARRIVAL_INTERVAL_MS` (500 ms) after the previous one started: after its capture and evaluation the request waits on the Runtime clock (`kernel_pause`, as the transaction's own interval) for the rest of the 500 ms, none when they took longer, never past the deadline; was a 500 ms sleep after each evaluation, bounded by the deadline; the 20-sample budget, the deadline, `timed_out` and `elapsed_ms` are unchanged |

### 1.148 Slice A38 (0.12.9): a fence that waits for a person re-checks itself; an emulator control command that timed out reads the status first; the wait is visible (#411 A38; coordinator 10-20 21:4x)

The 10-20 incident: one instance's capture channel could not open for a moment, its fence's
ladder could not restart the emulator because the shutdown command passed its 60 s bound, and
the instance waited two hours for a person although one self-check would have passed at once.
[emulator-control.md](emulator-control.md) ("Resource fence"; "State after a failure") is the
contract, [runtime-config.md](runtime-config.md) ("The recovery ladder section") the keys'.
**Persisted shapes:** one new optional member of an existing projection (A38-1); the records are
existing record types with new codes (A38-2 to A38-4); the wait itself is memory only, like the
fence.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A38-1 | `InstanceResourceFence.person_wait` (`InstanceFencePersonWait`, F5) | new optional member of the status projection (also in the runtime state observation of `command.validated`) | A38 | A38 | `{since_unix_ms, recheck_attempts, recheck_max_attempts, next_recheck_unix_ms?}` (`deny_unknown_fields`), present exactly while `awaiting_person`; `next_recheck_unix_ms` absent while a recheck runs, after the last one and while `max_attempts` is 0; `ac_overview` shows the status member unchanged as the row's `resource_fence` |
| A38-2 | `runtime.failed` with `instance_resources_fence_awaiting_person` (warning, `none`) | new code of an existing record | A38 | A38 | system links with the instance; once when a fence begins to wait for a person (its ladder ended without proof, none could start, or manual work met it); values `instance_id`, `failure_code` (the fence's cause), `limit_count`, `retry_after_ms`, `next_occurrence_unix_ms?` |
| A38-3 | `runtime.failed` with `instance_resources_fence_recheck_failed` (warning, `none`) | new code of an existing record | A38 | A38 | the recheck claim's request links; one per failed recheck; values `instance_id`, `attempts`, `limit_count`, `failure_code?` (the self-check's), `next_occurrence_unix_ms?` (absent after the last); the recheck's own `instance_preparation_finished` has stage `recovery_preparation` (the Runtime's own recovery preparation: no ladder starts from it, A2) |
| A38-4 | `resource_fence_lifted` (F2) `outcome` | new value of an existing member | A38 | A38 | `instance_resources_fence_recheck_passed` (info, `none`; `instance_id`, `attempts`, `limit_count`) when the fence's own recheck lifted it; every other lift keeps `instance_resources_fence_lifted` |
| A38-5 | `runtime.failed` with `emulator_control_timeout_settled` (warning, `stability_warning`) | new code of an existing record | A38 | A38 | the control request's links, before `command.validated`; when a `stop` or `start` control command passed its own bound (`mumu_manager_timeout`) and the one status read after it showed the requested state; values `instance_id`, `elapsed_ms`, `process_started`, `android_started`, `adb_port_present`; `caused_by` the timeout (`emulator_control_failed`); otherwise `emulator_control_failed` as before, now with the read's three values when the read answered |
| A38-6 | `recovery_ladder.fence_recheck.interval_ms` (`10000..=86400000`, 300 000), `recovery_ladder.fence_recheck.max_attempts` (`0..=1000`, 12; `0` turns the recheck off) | configuration keys (§5 row; `config.parameters.recovery_ladder` rows) | A38 | A38 | settable, read by the lease sweep at each tick; defaults only in `actingd.defaults.json`; `config_value_invalid` (`out_of_range`) through an instruction, `recovery_ladder_config_invalid` at load and in check-config |
| A38-7 | `actingd-config` [5, 8] | interface revision (§6; K27) | A38 | A38 | revision 8 is revision 7 plus the optional member `recovery_ladder.fence_recheck`; a Runtime before A38 refuses a file that names it (the section denies unknown members); a file that does not name it keeps its revision |

### 1.149 Slice BT-B3 (0.12.x): several expected colors (#411 B3; B-train plan §2.3; Alice's reading (a), 2026-10-20)

A color probe, a template's `color_check` and with it the `repeated_anchor` anchor's color
predicate may list up to seven `alternatives` besides `expected`; one acceptance decides all of
them: the nearest declared color, the earliest among equal distances, within the one
`max_distance`. **No persisted ledger shape**: `TaskDiagnosticColorData` is unchanged and keeps
`expected` and `distance`, which are now the matched color's; `expected_index` is read-only and
Lab output only. No code, location, key, vocabulary token, wire or configuration key is added;
the refusals reuse `resource_declaration_invalid` reasons and the recognition pack's existing
load refusal. The member belongs to `package` revision 6, which BT-PK6 declares at the end of
the B train (plan §4); until then it is admitted by code. [resource-declarations.md](resource-declarations.md)
("Task schema `0.9` several expected colors"), [template-relative-color.md](template-relative-color.md)
and [selection-graph.md](selection-graph.md) are the contract. Gate: exec key 118.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B3-1 | `alternatives: [[r, g, b], …]` on a `color_probes[]` entry with `expected` and on the `color_check` of `anchors[]` and `verify_templates[]`; derived onto the `color` target and the `color_check` | resource file format (task schema `0.9`, no version change; pack schema `0.7`) (exec 118) | BT-B3 | resource side | 1..=7 colors, none repeated, none equal to `expected`; refused `resource_declaration_invalid` at `…/alternatives` (`UnconsumedField` before task schema `0.9` and on a digest entry, `InvalidType` for no array, `InvalidValue` otherwise); a source that uses it derives pack schema `0.7`; a `0.6` pack refuses it as an unknown field and the evaluator refuses the same counts, repeats and copies at load |
| B3-2 | the color acceptance of a probe, a `color_check` (absolute, relative and in the joint search) and the `repeated_anchor` predicate | rule (no shape change) (exec 118) | BT-B3 | BT-B3 | the region's mean against `expected`, then each alternative in order; the nearest, the earliest among equals, is the evaluation's `expected` and `distance`, and passes within `max_distance` (else the package default); in the joint search any declared color qualifies a candidate; without `alternatives` evaluation is unchanged |
| B3-3 | `expected_index` of the recognition evaluation's `color` and of the read-only `ColorEvaluationResponse` | in-process and output member (not persisted) (exec 118) | BT-B3 | BT-B3 | 0 for `expected`, `n` for the `n`th alternative; present only when the probe or check declares `alternatives`; the task diagnostic record does not carry it |

### 1.150 Slice BT-B8 (0.12.x): the scheduler passes a run its schedule values (#411 B8; plan `rd-1021/train-0129/b-plan/plan.md` §2.8, Q12, D7; [remedies.md](remedies.md) section 4)

B8 adds its typed values to the channel B15 R5 opened (§1.115). **No persisted ledger shape**:
the values are new optional values of the pending entry `policy_dispatch_parameters` in the
existing dispatch reason chains (`PolicyReasonRecord` is `#[serde(transparent)]` over `Outcome`;
earlier 0.12 readers keep `values` as a map without a key check), and the records
`session.dispatch.schedule.<key>` are an overlay of a run's instance fact snapshots, never stored.
The catalog member is V2 only and optional; a catalog without it keeps its bytes and its hash. No
package format change. Gates: policy key 59, host key 129.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B8-1 | `TaskSpec.dispatch_parameters: Option<bool>` (V2 `tasks.schema.json` `dispatch_parameters`, boolean) | catalog member (V2 only; absent serializes as nothing) (policy 59) | BT-B8 | BT-B8 | `true` opts the task in; `false` or absent carries nothing; a V1 document that names it is refused `unknown_field` at `/tasks/<i>/dispatch_parameters` |
| B8-2 | `policy_dispatch_parameters` values `next_occurrence_unix_ms`, `window_end_unix_ms` (existing keys), `last_in_window`, `last_daily_slot` (booleans), `daily_remaining` (integer, new keys); `run_parameters` becomes optional | new optional values of a pending entry (no shape change) (policy 59) | BT-B8 | BT-B8 | on every selected dispatch of an opted-in task, with or without remedies (Q12), merged into the remedy plan's reason or alone; each only when known: the least next occurrence of the clocks `clock_slot_view` counts (none without such a clock, an interval under one hour, or when none occurs again), the end of the instance's activity window at the evaluation (the intent's profile), whether that next occurrence is absent or at or after the window end, the fewer of the task's (accounting group) and the activity profile's remaining daily runs after this dispatch as the host's admission preview counts them, and whether that is 0; a schedule with no known value adds nothing |
| B8-3 | `CandidateEligibility::EligibleWithDailyRemaining { daily_remaining }` | policy interface (memory only) (policy 59) | BT-B8 | BT-B8 | the host's eligibility closure answers it for an admissible candidate (positions 1 to 9 passed), from the admission preview's `PolicyBudgetReceipt` (`task_daily_limit − task_daily_used`, `activity_daily_limit − activity_daily_used`, the fewer); the evaluator treats it as `Eligible` |
| B8-4 | `RunParameters::with_dispatch_schedule(values, admitted_at_unix_ms)` and the overlay's `session.dispatch.schedule.<key>` records | kernel interface (memory only, not persisted) (host 129) | BT-B8 | BT-B8 | after the parameter records: the key under `session.dispatch.schedule.` (D7), the value typed (`timestamp_ms` for the two instants, `boolean`, `integer`), the other members as B15R-1's, observed at the dispatch's admission; a run without a dispatch (direct, Lab, the Lab's single-step evaluations) and a dispatch without the values hold none, so a policy that reads one meets an unknown fact and its `on_unknown` decides; a reason without `run_parameters` leaves the parameters at their declared defaults (B15R-2 unchanged) |
| B8-5 | keys `last_in_window`, `last_daily_slot` (boolean), `daily_remaining` (integer) | new key-table entries (no shape) (policy 59) | BT-B8 | BT-B8 | review of the code stays `pending`; no new code, vocabulary token, location or configuration key |

### 1.151 Slice A51 (0.12.9): one ledger read no longer stops the Runtime: the Lab view selects by index, a writer read has a work budget, a read timeout is not fatal, the close is bounded (#411 A51; brief A51, coordinator freeze 10-21 19:3x)

The 10-21 incident: one `view: lab` page ran as one correlated SQLite statement on the ledger
writer for minutes; nothing committed meanwhile, the reads behind it timed out, the host mapped
those timeouts to its fatal state and the close then waited for the writer without a bound.
**No persisted ledger shape**: the stored view DDL, the ledger format and every view's answer
are unchanged; the writer's Lab facts are memory only. [ledger-observer.md](ledger-observer.md)
§15 and [runtime-config.md](runtime-config.md) give the keys, [global-ledger-query.md](global-ledger-query.md)
("read_scope") the page. Gates: ledger key 51, host key 130.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A51-1 | `LedgerPageLimit::ReadBudget` (`read_budget`) in a page's `read_scope.limits` | new value of an existing wire enum (runtime-client 2, folded in as §1.91 N1-1) | A51 | A51 | a Runtime page whose read stopped at its budget: `read_complete` false, `has_more` true, `scanned_through_position` below the snapshot, the cursor after it (after the last row when the count or byte limit cut the page too); the page may hold no row; the client's, actingctl's and MCP's page loops go on from the cursor |
| A51-2 | `ledger_close_timeout` (error, `none`) | new code (ledger fragment) | A51 | A51 | the close waited `close_timeout_ms` for the `Shutdown`'s place in the queue, its reply and the writer's end; values `timeout_ms`, `stage` (the writer's command or flush in progress), `elapsed_ms`, `operation` `shutdown_writer`; terminal for the ledger (the writer is left behind); the host's close carries it as the `caused_by` of `ledger_failure` at `close_global_ledger`, actingd exits non-zero and its watchdog starts it again |
| A51-3 | `ledger_observer.read_budget_ms` (`1..=9999`, 1 000), `ledger_observer.close_timeout_ms` (above `read_budget_ms`, 60 000) | configuration keys (§5 row; `config.parameters.ledger_observer` rows) | A51 | A51 | not settable, read when the ledger opens; defaults only in `actingd.defaults.json`; a broken rule is `config_value_invalid` (`section_check`); the writer's own check refuses the same as `invalid_ledger_config` |
| A51-4 | `actingd-config` [5, 8] | interface revision (§6; K27), extended, not raised (coordinator freeze) | A51 | A51 | revision 8 (A38, §1.148 A38-7) also holds the two optional `ledger_observer` members; a Runtime before A51 refuses a file that names either (the section denies unknown members) |
| A51-5 | the writer's Lab facts per run (the Lab-anchored request and correlation of each run, and the runs of each request and correlation) and `LedgerProjectViewObservation.selection_work` | writer memory and a process-local observation (not persisted) | A51 | A51 | a Runtime page of every view selects its rows in memory from the writer's indexes, each Lab relation found by lookup; the offline reader keeps the stored SQL views; the observation counts the candidates the index path visited, or the SQLite VM steps of an offline selection |
| A51-6 | a writer read's work budget and its answers | rule (no shape) | A51 | A51 | every read command (queries, pages, projections, replay pages, the frame retention view, the scheduling outcomes) looks at the writer's clock every 256 rows; a page answers A51-1, any other read the existing `ledger_read_budget_exceeded` (not fatal); the artifact lookup keeps its own 4 s material deadline; the appends that queued behind a read commit before the writer's next command |
| A51-7 | the host's reads that the writer did not answer in time (`writer_response_timeout`) or stopped at its budget | rule (no shape) | A51 | A51 | relayed as request errors (`runtime_unavailable`), the request answered `failed` with the read's code, nothing latched: the status and overview head read, the fact snapshots' position reads, the event and diagnostics pages, subscriptions, the evidence, lease, contained-task, safe-reset, lifecycle, client-fact, observation-input, artifact-index and agent-context reads; the performance loop's seal skips its tick; every other read failure stays `ledger_failure` |
| A51-8 | `RuntimeDaemonReporter` (host interface; actingd's `DaemonReporter`) | host interface (memory only) | A51 | A51 | the first fatal condition the host latches goes to actingd's daemon lines at the moment it latches, before the close; a skipped background step is a developer line |

### 1.152 Slice A35-S1 (0.12.x): a pack can mark a mapping `busy`; the recovery ladder holds instead of restarting while it is busy (#411 A35, B37; design `rd-1021/train-0129/a35-design.md` §2.2, §3, §4.3, §5.1 S1; coordinator decisions Q1 to Q3 and Q11 of 2026-10-21)

The A35 incident: an instance was in a long activity that ends by itself, its return-home run
ended on the activity page, and the ladder went on to the application restart and closed the game
in the middle of it. A `scheduling_outcome` mapping may now say `busy`; a rung run that completes
on it is not recovered, and the ladder holds its key and runs the return-home rung again within a
configured limit. [emulator-control.md](emulator-control.md) ("Busy ends", "Records") is the
contract, [resource-declarations.md](resource-declarations.md) ("Busy outcome mappings") the
member's, [runtime-config.md](runtime-config.md) the keys'. **No persisted ledger shape** (design
§7): the holds are recorded with the existing `recovery_rung_finished` and
`recovery_ladder_finished` phases and two new codes; the busy end is memory only. Semantic changes
a reader meets (not shapes): one rung may have several `recovery_rung_finished` records in one
ladder (the return-home rung's, once per run after a hold), `rungs_tried` counts distinct rungs,
and a `failed` rung record may carry an `info` outcome. Gates: host keys 133 to 137, contract key
K29.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B37-1 | `scheduling_outcome.mappings[].busy` (boolean, default `false`) | new optional member of a `task.json` mapping (resource file format, not a ledger shape) (contract K29) | A35-S1 | resource side | only on a `no_designated_effect` mapping and never with `remedy` (`invalid_scheduling_outcome_declaration`, field `busy`; the source parser checks the type at `…/busy`); a mapping without it serializes byte for byte as before; joins `package` revision 6, which BT-PK6 declares, admitted by code until then; never a member of a persisted event |
| B37-2 | `recovery_rung_busy_outcome` (info, `planned_stop`), `recovery_ladder_hold_exhausted` (warning, `planned_stop`) | new codes of existing records (host 133, 135) | A35-S1 | A35-S1 | the first is the outcome of a `failed` `recovery_rung_finished` whose run completed on a busy mapping (`outcome_key`, `held_ms`, `limit_ms` required; `page_id`, `retry_after_ms` optional, the latter absent when the hold ends); the second is the outcome of `recovery_ladder_finished` `exhausted` when the hold reached `max_wait_ms` under `on_exhausted = stop` (`held_ms`, `limit_ms`; caused by the last busy outcome); neither matches a default trigger item, and direction ① ignores a climbing instance's records |
| B37-3 | the ladder's busy hold | rule (no shape change) (host 133 to 136) | A35-S1 | A35-S1 | after a busy end, while the climb has a return-home package and has held less than `max_wait_ms` since its first busy end: renew the key to the hold plus `hold.margin_ms`, hold `recheck_ms` (no longer than what is left) making the holding check every `poll_ms` (an interruption ends the climb at the next poll, the return-home rung recorded failed with it), then run the return-home rung again; at the limit `on_exhausted` (`stop`: end without a restart; `escalate`: the next rung); without a return-home package as `escalate`; a rung is escalated past at most once (amends "a rung is tried at most once", coordinator decision Q2) |
| B37-4 | `recovery_ladder.busy.poll_ms` (100..=`outcome.recheck_ms`, 500), `recovery_ladder.busy.outcome.recheck_ms` (plus the margin within the longest renewal, 60 000), `recovery_ladder.busy.outcome.max_wait_ms` (0..=86 400 000, 3 600 000), `recovery_ladder.busy.outcome.on_exhausted` (`stop` \| `escalate`, `stop`) | configuration keys (§5 row; `config.parameters.recovery_ladder` rows) (host 137) | A35-S1 | A35-S1 | settable (coordinator decision Q3), part of the section's digest, a ladder holds by its admission snapshot; defaults only in `actingd.defaults.json`; at load and in check-config `recovery_ladder_config_invalid` (`bound_out_of_range`, `renewal_cap_exceeded`), through an instruction `config_value_invalid` with the same key and why |
| B37-5 | `actingd-config` [5, 8] | interface revision (§6; K27), extended, not raised (wave-2 frame) | A35-S1 | A35-S1 | revision 8 (A38, §1.148 A38-7) also holds the optional `recovery_ladder.busy` member; a Runtime before A35-S1 refuses a file that names it (the section denies unknown members) |

### 1.153 Ledger revision 4 (0.12.x, Workflow #411 B train)

Alice's ruling of 2026-10-20 (Workflow #411 item 14: every B item per the recommendation, in the
0.12 series) takes the persisted shapes of the B train into one ledger revision, the one
exception to the rule at the head of this register. Slice BT-LR writes them shape first, as SH-1
did for v0.12.0: the types, every reader (contract validation, the ledger writer's admission of
a run's facts, the task diagnostic stream's rows), the validation and one round trip per shape,
and **no writer**; each row names the slice that first writes it (B-train plan
`rd-1021/train-0129/b-plan/plan.md` §3; coordinator decisions Q1 to Q3 of 2026-10-21). Additive
only: SQLite format 2, `actingcommand.event.v3`, the tables, the indexes and the view type lists
are unchanged (the two new record types are in the `events` view, in `errors` by severity and in
no type list, since a type list change is a view schema upgrade). Interface `ledger` rises from
[3, 3] to [3, 4] (§6; [component-interfaces.md](component-interfaces.md) revision 4): the Runtime
writes revision 4 and reads revision 3, with no fresh state root and no migration; the first
release that contains it is the point of no rollback, and acsetup combines it only with a UI
whose `ledger` range holds 4. No code, location, key or configuration key is added; the
vocabularies gain `event_type` `task.effect_refused` and `task.operation_capped` (info) and
`value_problem` `no_agreement`, each with its text. Gates: contract keys K31 and K32 (K27
adapted), ledger keys 54 and 55.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| LR-1 | `decision_matched {candidate_set_sha256, decision_sha256, projection?}` and `decision_mismatched {…}` (`TaskSelectionConfirmation::DecisionMatched`, `DecisionMismatched`) | new variants of a persisted enum, in `task.selection_evaluated` (K31, K32) | BT-LR | BT-B4 | a single-frame select that confirms by its decision (`select.confirm: decision`, b-plan §2.4): only as the top-level confirmation of a record with a v1 projection and no `multi_select`, never a pick's or a tap check's; both hashes 64 lowercase hex digits; `decision_sha256` covers the actionable candidates' `{id, click rect, status}`, the selected ids and the outcome key, and a reader checks its grammar, never recomputes it; `candidate_set_sha256` is F2's and may differ from the evaluated projection's; an embedded F2 projection seals it and names the record's layout and page, as for `matched`; the record's own validation (run directly, and by a writer's draft) refuses it `invalid_task_selection_record` at `confirmation`; the stored reader refuses the fact `invalid_task_semantic_payload` at `task_semantic_fact` and keeps no inner cause (X8) |
| LR-2 | `TaskDiagnosticTemplateData.luma {mean_ratio_milli, contrast_ratio_milli}` | new optional member of a task diagnostic `target` row (K31) | BT-LR | BT-B13 | present only when the target declares `luma_floor`: the window's mean and standard deviation over the scored pixels, each divided by the template's and times 1000 (b-plan §2.10, D9); absent keeps the revision-3 bytes |
| LR-3 | `task.effect_refused {step_index, operation_label, target_id}` (`TaskSemanticFact::EffectRefused`) | new event type, family `task`, a task fact without an outcome (info) (K31, K32; ledger 54, 55) | BT-LR | BT-B18 | the refusal marker `target_id` passed after the step's input, so the effect is not done (b-plan §2.13); it follows an earlier `task.effect_completed` of its step (the same step index and operation) and is the step's only one: the ledger's writer refuses it otherwise (`invalid_task_effect` at `effect_refused`) and writes nothing; durable; the host's designated effect derivation leaves the refused effect out of the disposition (BT-B18), which no reader recomputes |
| LR-4 | guard phase `refusal` | new value of the `phase` string of a `target` row's guard source and of an `error` row (K31) | BT-LR | BT-B18 | a refusal marker's evaluation (`TASK_DIAGNOSTIC_PHASE_REFUSAL`); [task-diagnostic-stream.md](task-diagnostic-stream.md) |
| LR-5 | `ValueTestProblem::NoAgreement` (`no_agreement`) | new variant of `TaskDiagnosticValueOperand.problem` (K31) | BT-LR | BT-B1 | the frames of a declared agreement did not read one value often enough (b-plan §2.1, Q6); the recognition pack carries it onto the outcome vocabulary `value_problem`, which gains the token `no_agreement`; `reading_problem` `no_agreement` stays with BT-B1 |
| LR-6 | guard phase `agreement_sample` | new value of the `phase` string of `target` and `error` rows (K31) | BT-LR | BT-B1 | each agreement frame's value-test evaluation (`TASK_DIAGNOSTIC_PHASE_AGREEMENT_SAMPLE`) |
| LR-7 | `task.operation_capped {step_index, operation_label, count, limit_count, outcome}` (`TaskSemanticFact::OperationCapped`) | new event type, family `task`; its severity is the outcome's category (K31, K32; ledger 54, 55) | BT-LR | BT-KEY | `outcome` is `contained_task_operation_runs_capped` (info, `ladder_class` none), which BT-KEY registers with its values `pack_operation_id`, `count`, `limit_count`, `page_id`, `outcome_key` (b-plan §2.11 item 5, Q5); `limit_count` 1..=1000 (the fact's own validation, as a writer's draft runs it: `invalid_task_semantic_payload` at `limit_count`; the stored reader: `invalid_task_semantic_payload` at `task_semantic_fact`, no inner cause, X8); a run records at most one: the ledger's writer refuses a second (`invalid_task_semantic_payload` at `operation_capped`) and writes nothing; durable; the public projection keeps its outcome's codes only |
| LR-8 | `candidate_scores {pages: [{page_id, passed, best_score_milli?, first_failed_target_id?}], omitted}` of `task.recognition_completed` (`RecognitionCandidateScores`, `RecognitionCandidateScore`) | new optional member (K31, K32; ledger 54) | BT-LR | A44 writer (§1.173) | #411 A44 (stopped in E2, PR #1090): one entry per candidate page in candidate order, the first 16 (`TASK_RECOGNITION_CANDIDATE_SCORE_LIMIT`), `omitted` the candidate pages after them; `passed` is the page matched, `best_score_milli` the highest target score evaluated on it, `first_failed_target_id` why it did not match, only on a page that did not pass; the matched page's entry passed, and without a matched page none did; the fact's own validation, as a writer's draft runs it, refuses `invalid_task_recognition_result` at `candidate_scores` (`invalid_task_semantic_label` for a malformed target id); the stored reader refuses the fact `invalid_task_semantic_payload` at `task_semantic_fact` and keeps no inner cause (X8); absent keeps the revision-3 bytes; the paired `recognition.completed` payload is unchanged |
| LR-9 | `package_version` of `entry_recovery_package_admitted` and `entry_recovery_completed` (`task.entry_preflight`) | new optional member (K31, K32; ledger 54) | BT-LR | a later B26 slice | #411 B26: the version the layer's package declares, a label (the fact's own validation, as a writer's draft runs it: `invalid_task_semantic_label` at `package_version`; the stored reader: `invalid_task_semantic_payload` at `task_semantic_fact`, no inner cause, X8); never a digest or a hash (option 丙); absent keeps the revision-3 bytes; `entry_recovery_failed` has none |
| LR-10 | `LabOperationSelection::Operation {operation_id, seed?}`, `LabOperationPrepared.operation` (`LabDeclaredOperation`) and `LabOperationRecord.operation_result` (`LabDeclaredOperationResult` with `LabOperationAttempt`, `LabOperationFrameEntry`, `LabGuardOutcome`) in `actingcommand.runtime.lab-operation-prepared.v1` and `-terminal.v1` (#412 B-3) | new variant of a persisted enum and two new optional members (contract K39 to K42) | Lab B-3 S0 | Lab B-3 S2 | a Lab operation that runs an operation its package declares, as the kernel runs it (design `rd-1021/train-0129/lab-b3-design.md` §3; §1.159); absent in element and coordinate records, which keep their bytes; the schema strings, event types, tables and indexes are unchanged; until S2 the Runtime refuses the selection at `selection`, before any input; amended by S2 before its first writer, 10-22 (no ledger held an LR-10 record; `ledger` stays [3, 4]): the guard `not_evaluated` (no other members) exactly when no attempt is recorded, and the verifier accepts a complete record without attempts (§1.162) |

### 1.154 Slice BT-B5 (0.12.x): stage end pages and entry pages; an entry that ends at once (#411 B5, B14, B20, B24, B38; B-train plan `rd-1021/train-0129/b-plan/plan.md` §2.5, D4, D5)

A stage of a phased route may list its end pages and its entry pages. **No persisted ledger
shape**: a run that ends on an end page writes the records a Completed run writes today
(`StepFinished` with its stage not completed, or none at all for an entry that ends at once), and
the host maps it as before. No code, location, key, vocabulary token, wire or configuration key
is added; the refusals reuse `task_phases_invalid` (new `phase_problem` tokens of its developer
detail only: `end_pages_count`, `end_pages_repeated`, `end_pages_invalid`, `entry_pages_count`,
`entry_pages_repeated`, `entry_pages_invalid`), `task_phases_outcome_incomplete`,
`contained_task_entry_pages_invalid` (`phase_start_ambiguous`) and
`contained_task_prerequisite_incompatible`. Both members join `package` revision 6, which BT-PK6
declares at the end of the B train; until then they are admitted by code. There is no implicit
end on arrival at a mapped page. [README.md](README.md) ("Stage end pages and entry pages",
"Entry page sets") and [linear-steps.md](linear-steps.md) (prerequisite checks) are the contract.
Gates: exec keys 120 to 124, contract key K37.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B5-1 | `TaskPhase.end_pages` (`phases[]` of task schema `0.9` and of control `Lab-1y.control.v2`) | new optional member (resource file format, not a ledger shape) (exec 120, 123) | BT-B5 | resource side | 1..=16 pages of the package, none repeated, blank, `any`, a target page of the stage or an error page (`task_phases_invalid` at `/phases/<i>/end_pages/<j>`); a page an operation of the stage confirms that is one of them ends the run Completed on that page, with no further operation and the stage not completed (`TaskPhase::is_end_page`, the one test of the walk, the run and the entry); a stage entered by advancing does not test its own end pages on the advancing frame (D5); absent, the stage serializes as before and a reader before BT-B5 refuses the member (`deny_unknown_fields`) |
| B5-2 | the static walk at an end page | rule (no shape change) (contract K37) | BT-B5 | BT-B5 | a confirmed end page is a terminal that needs exactly one mapping of its effect condition and page (`task_phases_outcome_incomplete` otherwise), and the path ends there; source parsing, build and Runtime admission share the check |
| B5-3 | `contained_task_prerequisite_incompatible` detail `reason=phase_end_pages` | new value of an existing refusal detail (exec 124) | BT-B5 | BT-B5 | a package with a stage that lists `end_pages` serves neither as a prerequisite package nor for return-home use: an end page ends its run short of its target pages |
| B14-1 | `TaskPhase.entry_pages` | new optional member (resource file format, not a ledger shape) (exec 121) | BT-B5 | resource side | 1..=16 pages, none repeated, blank or `any`; each a page of the task's entry page set that starts the listing stage (D4; `task_phases_invalid` at `/phases/<i>/entry_pages/<j>` otherwise; a single entry page starts the first stage); a listed page starts the stage where no operation's `from` infers one, and a page listed by two stages or also inferred to another stage is `phase_start_ambiguous` |
| B20-1 | a start on an entry page a stage lists in `end_pages` | rule (no shape change) (exec 122) | BT-B5 | BT-B5 | a start of that stage of its own (`phase_entry_start`); the run ends on its entry observation before any step: Completed, 0 steps, final page that page, no designated effect, judged by the host on the last recognition (#411 A23); the walk needs the one mapping of that page under `no_designated_effect`; a page that is also an operation's `from` (or another start) has two starts and is refused `phase_start_ambiguous`; B38 is this rule's fixture |

### 1.155 Slice BT-B12 (0.12.x): tap only after a still screen (#411 B12; plan `rd-1021/train-0129/b-plan/plan.md` §2.9, D8, Q7, Q9)

A click's `input_recheck` gains the kind `still_frames`, and an input recheck of any kind is
admitted under `linear_steps`. The colour-digest distance between consecutive frames is the
Runtime's one frame-change measure ([color-digest.md](color-digest.md), section Frame change);
later rules that ask whether the screen is still use it. **No persisted ledger shape**: a refused
recheck ends with an existing terminal record carrying the new code; the frames are recorded as
every capture, recognition and guard evaluation is. No location, key, vocabulary token, wire or
configuration key is added. The new members join `package` revision 6, which BT-PK6 declares at
the end of the B train (plan §4); until then they are admitted by code. Gate: exec key 126.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B12-1 | `input_recheck: {kind: "still_frames", frames, region?, max_distance_milli?}` | resource file format (task schema `0.9`, no version change) (exec 126) | BT-B12 | resource side | `frames` 2..=32 and `frames - 1` control capture intervals within the control step timeout; `region` `{x, y, width, height}` with a positive size inside the coordinate space; `max_distance_milli` an unsigned integer (Runtime admission, `contained_task_operation_invalid`); the declaration gate refuses a wrong shape (`resource_declaration_invalid`: `InvalidType`, `UnknownField`, `MissingField`) and an older task schema `UnconsumedField` |
| B12-2 | the `still_frames` recheck | rule (no shape change) (exec 126) | BT-B12 | BT-B12 | fresh frames one control capture interval apart; the count restarts at 0 when the guard fails or no page passes, and at 1 when the region's frame change since the frame before exceeds the threshold (default 1500 milli, D8: the whole frame on a 16×9 grid); the input is drawn from and bound to the last frame |
| B12-3 | `contained_task_input_still_unconfirmed` (error, `ladder_class` `pack_logic_mismatch`, values `pack_operation_id`, `timeout_ms`) | new code (pending) (exec 126) | BT-B12 | BT-B12 | the step timeout ended before the frames held and stayed still; the input is refused through the operation's failure decision (page graph) or ends the run (`linear_steps`) |
| B12-4 | an `input_recheck` of any kind under `linear_steps` | admission rule (no shape change) (exec 126) | BT-B12 | resource side | `contained_task_linear_invalid` `reason=input_recheck` is retired; each fresh frame is evaluated against the step's page only; a refusal ends the run as a guard refusal does |
| B12-5 | `frame_change_digest`, `frame_change_milli`, `FRAME_CHANGE_MAX_MILLI` (`actingcommand_recognition::color_digest`, re-exported by the execution kernel) | Runtime-internal interface (memory only) | BT-B12 | BT-B12 | the one frame-change measure: the `color_digest.v1` digest of a region (the whole frame by default) on 16×9 cells, each axis at most the region's pixels; the change is the digests' `mean_milli`, no cell excluded; still at most 1500 by default |

### 1.156 Slice E4 (0.12.x): four refusals and warnings say what is wrong (#411 B27, B28, B34, B39 A part; B-train plan §6, Q15)

Each item says what is wrong as typed values under the outcome-code spec; nothing that was
accepted is refused, nor the other way round, and B34 only adds a warning. **No persisted ledger
shape**: the new values are optional values of three pending entries
(`arguments_invalid`, `resource_declaration_invalid`,
`contained_task_prerequisite_entry_unmatched`), and the one new code, a pending warning, is
answered only by `actinglab package check`. No wire, configuration key, location or package
format changes. [runtime-interface.md](runtime-interface.md) (`ac_events`),
[resource-declarations.md](resource-declarations.md) (page rules),
[package-reference.md](package-reference.md) (`package check`) and
[linear-steps.md](linear-steps.md) (prerequisite gate) are the contract. Gates: apps keys 87, 88;
exec key 131; host key 139.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| E4-1 | `ac_events` given an `event_type`, or an element of `event_types` or `exclude_event_types`, that names no event type: `arguments_invalid {argument, argument_problem: not_recognised, argument_value}` (class `usage`) | changed answer: a new optional value (apps 87) | E4 | E4 | `argument_value` (`name`) is the value given, kept when it is at most 256 bytes without a control character; refused before any Runtime read, as before; the input schema is unchanged; `actingctl events` keeps its `actingctl_usage_invalid` without the value |
| E4-2 | `resource_declaration_invalid` of a page rule's `any_of` that is no array, or of an entry of it that is no array (`invalid_type` at that member) | new optional value `expected_shape` (`vocab:expected_shape`, token `array_of_arrays`) on the kernel admission's outcome (exec 131) | E4 | E4 | the same refusal and the same `details.declaration`; `package check`'s and `package preflight`'s `details.outcome` and the Runtime's refusal carry it (`declaration_expected_shape` reads the refused member's pointer); a target id inside a group that is no string gets no value; the `resource_declaration_rejected` record's own outcome is unchanged, so on the Runtime the refusal's lifecycle record carries the value (ledger revision 3, as for every failure with values) |
| E4-3 | `actinglab package check` answer member `warnings` | changed output (not persisted) (apps 88) | E4 | E4 | the envelope of one `package_check_click_box_single_point` warning per operation click whose declared box is one point (a `point` click and a long press without a box, each a box of 1×1 at its point, or a `rect`, `specific_rect` or long-press box of 1×1), in declaration order, with `pack_operation_id`, `box_x`, `box_y`, `box_width`, `box_height`; empty when there is none; a click located on the frame (`target`, `target_center`, `offset`) and a drag are not judged; the package still passes (exit 0); `ac_pack_check` answers it verbatim |
| E4-4 | `contained_task_prerequisite_entry_unmatched` after a declared prerequisite package whose run ended on a page exactly one of its `scheduling_outcome` mappings lists | new optional value `layer_outcome_key` (`name`) on the failure's outcome, the terminal's and the receipt's (host 139) | E4 | E4 | the mapping's `outcome_key`; a prerequisite package declares no designated effect, so the condition is `no_designated_effect`; none when the layer's run did not succeed, ended on an unmapped page or selected a key of its own; not on `contained_task_return_home_entry_unmatched`; the run still fails with this code (B39's B part, a mapped layer key that ends the main run, is not in this slice) |
| E4-5 | keys `argument_value`, `layer_outcome_key` (`name`), `expected_shape` (`vocab:expected_shape`), `box_x`, `box_y`, `box_width`, `box_height` (integer); vocabulary `expected_shape`; domain `package_check_` (actinglab); code `package_check_click_box_single_point` (warning, ladder class `none`) | new key-table, vocabulary, domain and catalog entries (no shape) | E4 | E4 | every entry stays `pending`; no location or configuration key |

### 1.157 Slice B41 (0.12.x): a chain step that hands over on its page goes first (#411 B41; coordinator triage Q15; [scheduling/README.md](scheduling/README.md), "Chain Handover")

The 10-20 incident: a chain's predecessor ended on a result page that is its successor's entry
page; a fact refresh reading ranked ahead of the successor ran first, its return-home
prerequisite layer left the page, and the successor failed with
`contained_task_home_recovery_terminal_outside_entry`. **No persisted ledger shape**: the new
reason travels in the existing decisions and dispatch reason chains (`PolicyReasonRecord` is
`#[serde(transparent)]` over `Outcome`), and a dispatch decision record lists a waiting candidate
with it as `why` (§1.145). No catalog or package format change, no key, vocabulary token,
location or configuration key. Gates: policy keys 60 to 63, T5 in key 63's row (host keys 141 to 143 unused).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B41-1 | `policy_chain_handover_waiting` (info, `none`; `instance_id`, `task_id` the successor, `page_id` the page) | new code of existing decision reasons (policy 60 to 63) | B41 | B41 | the last reason of each candidate that ranked ahead of a dispatched successor on its instance, a fact refresh reading included (its decision deferred, no wake); the affected candidates of that reading carry it as `fact_refresh_pending`'s `gate_code`; review `pending` |
| B41-2 | `evaluate_with_chain_handovers` with `ChainHandoverInputs { resting_pages, successor_wait_codes }` (`RestingPage { page_id, observed_at_unix_ms }` per instance alias; the deferral codes under which a successor still goes first, the host's `dispatch_random_delay_waiting`) and the entry answer `(task, instance alias, page id) -> bool` | policy interface (memory only) (policy 60 to 63) | B41 | B41 | the host passes each instance's `task.page` runtime fact and answers from the successor's package (its declared entry page or set, or its entry gate's pages, without the game prefix; a package it cannot read holds no page); none of it enters a snapshot or a decision identity |
| B41-3 | the allocation order on a handover instance | rule (no shape change) (policy 60 to 63) | B41 | B41 | the successor moves ahead of the candidates that ranked ahead of it; they wait while it is dispatched or waits for its random delay (part of its dispatch, coordinator ruling 10-21), and take their turn in rank order when any other gate defers it (an admission gate at positions 1 to 8a, or the host budget); a successor that reads a key the fact refresh reads is no candidate, so the refresh goes first (model item 4) |

### 1.158 Slice A50 (0.12.x): a capture that cannot open says why; an operator start or restart queues the startup package; a restart waits for the old VM; the recovery preparation reads the emulator status first (#411 A50 and the A38 follow-up; coordinator 10-21 17:2x)

The production host on 10-21 at 17:00, after an emulator restart: the game was not running, the
Nemu capture looked up the game's display by its package and found none, and the Runtime reported
only `capture_backend_open_failed`; the operator's `emulator restart` answered
`startup_package: none`, so the game never started; and the old VM still held the port during the
restart, so the new instance came up on the port after it. [emulator-control.md](emulator-control.md)
and [runtime-config.md](runtime-config.md) ("The `emulator_control` section") are the contract.
**Persisted shapes:** none new. The records are existing record types with new optional values
(A50-1) or a new code (A50-4); the answer's new member is a receipt result's (A50-2), never stored.
No ledger change. Gates: host keys 144 to 147 (148 to 151 unused).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A50-1 | `capture_backend_open_failed` and `paired_backend_open_failed`: values `capture_open_cause` (new key, vocabulary `capture_open_cause`: `display_missing`, `connection_failed`, `other`), `application_id`, `instance_index` (existing keys) | new optional values of pending entries, a new key and vocabulary (no shape change) (host 144) | A50 | A50 | every emission through the host's execution failure carries the cause; `display_missing` when a Nemu display lookup by package answered no display (the application is not running), `connection_failed` when a Nemu connect answered no handle, both with the package (paired session only) and the instance index the backend used; `other` for every other failure and every backend that cannot tell, the backend's own text being the outcome's foreign link (`raw_text`) |
| A50-2 | `RuntimeResult::EmulatorInstanceControlled.startup_package_withheld: Option<StartupPackageWithheld>` (`instance_paused`, `scheduling_paused`) | new optional member of a receipt result (wire, served under runtime-client 2; absent unless a package was withheld; never stored) (host 145) | A50 | A50 | beside `startup_package: none` when the instance has a startup package and its own scheduling pause (`instance_paused`) or the global one (`scheduling_paused`) holds it; an earlier reader whose result denies unknown members refuses an answer that carries it |
| A50-3 | an operator `start` / `restart` (and the autostart's start, the same path) of an instance that has a startup package queues it whether or not the connection preparation passed; a pause that holds the instance queues none and cancels the instance's queued startup claims; a failed preparation's admitted ladder takes the key and the startup claim waits queued behind it | rule (no shape change; replaces S6b M6's gated startup claim of a paused start and the preparation gate of §1.36) (host 145) | A50 | A50 | the run is the ordinary startup run (`startup_package_scheduled`, the claim, `hand_off_startup_package`); the ladder's own rungs are unchanged |
| A50-4 | `runtime.failed` with `emulator_restart_previous_instance_running` (error, `environment_fault`) | new code of an existing record (host 146) | A50 | A50 | a restart (the operator's, or the ladder's emulator-restart rung) whose Stop's readiness wait expired reads the emulator status (the bounded read after the Stop's failure, else its last observation, then one read every 500 ms on the Runtime clock) until it reads the old VM stopped, at most `emulator_control.restart_exit_wait_ms`; still running there, the request's `command.rejected` and this record under the restart's links, values `instance_id`, `elapsed_ms`, `timeout_ms`, `config_key`, and the last read's `process_started`, `android_started`, `adb_port_present`; `caused_by` the Stop's `emulator_control_wait_timeout`; nothing is launched (the rung fails with the code); a Stop the device confirmed, or one whose read already shows it stopped, waits for nothing; the key is renewed to the wait's bound first |
| A50-5 | `emulator_control.restart_exit_wait_ms` (`1000..=300000`, 60 000) | configuration key of a new section (§5 row; `config.parameters.emulator_control` row) (host 146) | A50 | A50 | settable, read at every such restart; default only in `actingd.defaults.json`; `config_value_invalid` (`out_of_range`) at load, in check-config and through an instruction |
| A50-6 | `actingd-config` [5, 8] | interface revision (§6; no raise) | A50 | A50 | revision 8 (A38) also carries the optional `emulator_control` section; a Runtime before A50 refuses a file that names it (the file denies unknown members); a file that does not name it keeps its revision |
| A50-7 | `prepare_recovery_connection` (a ladder's readiness wait, the application restart rung's preparation, a fence's recheck) reads the emulator status first, as A48's self-check does | rule (no shape change; A48's codes and records) (host 147) | A50 | A50 | a running emulator the Runtime holds stopped or on another port is recognized again (port bound, `runtime.instance_bound`, the ADB baseline awaited, `device.connected`) before the opens; a stopped one is `instance_not_running`; a failed read is `emulator_state_read_failed` and the check goes on from the Runtime's own state |
| A50-8 | the client's receipt wait of a restart: 745 s (was 445 s) | client constant (no shape) | A50 | A50 | 445 s plus the largest `restart_exit_wait_ms` (300 s); start and stop keep 235 s |
| A71-1 | #411 A71 (0.12.25; #411 comment 6380024768; emu-watch, Alice's ruling 35: a pause holds only dispatches): an operator `start` / `restart` of an instance that has a startup package queues it also while a scheduling pause holds the instance (its own or the global one); `startup_package_withheld` then names that pause beside `startup_package: scheduled` | rule (no shape change; replaces A50-3's "a pause that holds the instance queues none and cancels the instance's queued startup claims"; A50-2's member keeps its shape and values) (host gate row "#411 A71") | A71 | A71 | the claim waits queued behind the pause as a gated entry, as a configured start's does, and runs once after the resume (the resume's reconnect first); 10-24 12:50 a start under an instance pause answered `startup_package_withheld: instance_paused` and the resume ran no startup package |
| W2B-1 | W2-B (0.12.26; review W1-6 M1, L1, L3; coordinator work order `rd-1028/brief-wave2a.md` W2-B): a `host.idle_state` row `{instance_alias, startup_package_queued: true}` for an instance whose startup package an emulator start (operator or configured) queued behind a scheduling pause, kept from the hand-off until the claim is granted or an emulator control cancels it; the next start restores it and schedules the package again behind the restored pause | new row form of an existing runtime fact (persisted; [runtime-fact-store.md](runtime-fact-store.md); no new key, code or configuration key) (host gate row "W2-B") | W2-B | W2-B | the same record and restore as #405 IDLE-1P / O-2 (no second mechanism); a woken instance is not kept so (its wake brings the package back); a Runtime stop disconnects the claim (`lease.queue_disconnected`) but keeps the row, so the package runs once after the resume, never twice; a build before 0.12.26 reads a record with such a row as `invalid_runtime_state_fact` (`restore_idle_state`) |
| IDLE-WG | IDLE-WG (0.12.27; production v0.12.26 on 10-29 02:43, #421 6476382014; coordinator work order `rd-1029/brief-wave2c.md` IDLE-WG): the idle plan counts a task due only when it could be dispatched now: on an instance a blackout covers or whose activity window is closed at the evaluation, no unselected task is due, also when its decision names a hold that comes before those gates (a scheduling pause, `dispatch_instance_held` for a claim on the key such as a resume's reconnect); such a task moves the idle wake only to the time the gates let it through (the blackout's end, then the window's next opening) minus the lead | rule (no shape change; [runtime-config.md](runtime-config.md), "The `idle` section") (host gate row "IDLE-WG") | IDLE-WG | IDLE-WG | read only by the idle check's plan (`schedule_explanation::idle_held_back`); the evaluation's decisions, the not-run explanation and the scheduler's wake are unchanged; a really due task, a switched action, a wake whose time has come (also inside a hold) and the start's owed wake still wake the instance; no new code, key or configuration key |
| UNSET-1 | UNSET-1 (0.12.28; production v0.12.26 on 10-29 05:24, #421 6485126610, 6485372261; coordinator work order `rd-1029/brief-unset1.md`): a start that cannot settle a scheduled run holds its instance only the first time: before recording, it reads the run's `runtime.failed` records once, by the run's links, up to the first `policy_settlement_dispatch_unsettled`; a run that already holds one is recorded as `policy_settlement_dispatch_left_open` (Warning) with the refusal as `failure_code` (new optional value of that code) and its instance is not paused again; a run that holds none is recorded and held as before; a read the writer did not answer in time, or stopped at its budget, holds as before and the Error carries the read's failure as a `note` link | rule and a new optional value of an existing code (no shape change; `runtime-host.json`; location `read_unsettled_dispatch_records`) (host gate row "UNSET-1") | UNSET-1 | UNSET-1 | read only by the start's `hold_unsettled_dispatches`; the refusal itself is still not persisted and the dispatch stays open (settling it is UNSET-2, which changes #670's rule); a pause still in place stays as it is; any other read failure fails the start, as the start's other reads of the run do; no new code, key or configuration key |
| B35-1 | #411 B35 (0.12.28; coordinator triage `rd-1028/triage-p71.md` B35, item 88 「全部放行」; coordinator work order `rd-1029/brief-wave2d.md` B35-warn): `package bundle` (with `--interrupts`) and `actinglab standard-pack check` report, after the report "entry page not in the list", one `package_bundle_interrupt_page_shadowed` warning `{package_id, page_id}` in `warnings` per pack of the registry and shared interrupt page of `interrupts.json` it declares itself, in registry order and then `interrupts.json` order; a feature package of `pages.json` and a handler of that page are not reported | new catalog entry (warning, ladder class `none`, `since` 0.12.28, pending; owner resource-tooling) and report entries (not persisted; answer shape unchanged) (exec gate row "#411 B35") | B35-warn | B35-warn | the Runtime's rule is unchanged: a page the running package declares is its own and the shared interrupt page does not act for it ([README.md](README.md), "Shared interrupt pages"; [package-reference.md](package-reference.md), the cross-check); nothing is refused; no new key; the "yield to the shared interrupt" declaration is P69, not here |
| EMU-BIND-1 | EMU-BIND (0.12.28; production 10-29 after the 05:23 reboot; coordinator work order `rd-1029/brief-emu-bind.md`): the emulator watch recognizes an instance's running emulator (Android started, an ADB port reported) whose endpoint the Runtime holds pending or bound to another port, on one autostart claim marked `recognize` (the self-check's recognition of #411 A48 on its key: the port bound, `runtime.instance_bound`, the ADB baseline, the opens; then the startup package as after a start); a watch or idle wake claim whose read before its Start finds the emulator so recognizes it instead of ending skipped | rule (no shape change; [emulator-control.md](emulator-control.md), "Emulator watch") (host gate row "EMU-BIND") | EMU-BIND | EMU-BIND | never launches or stops an emulator; any discovery-bound instance, configured or not; paused or not, inside a blackout it waits for the end; at most one claim per instance, bounded by `emulator_control.restart_attempts` within `restart_window_ms` counted apart from restarts; a failure stays on its instance (#419); no new code or configuration key |
| EMU-BIND-2 | `watch_kind` (new key, vocabulary `emulator_watch_kind`: `restart`, `recognize`) on `emulator_autostart_queued`, `emulator_autostart_skipped`, `emulator_autostart_preparation_failed`, `emulator_watch_restart_exhausted` | new optional value of existing codes, a new key and vocabulary (no shape change) (host gate row "EMU-BIND") | EMU-BIND | EMU-BIND | the watch's restart records carry `restart`, its recognitions and their ends `recognize`; `emulator_watch_restart_exhausted` is recorded once per streak of each kind |
| EMU-BIND-3 | a Start's readiness wait takes a refused `info` poll as no observation and waits to its deadline (`mumu_manager_wait_timeout`, carrying the last refusal); a Stop's refused poll still ends the wait; a failure's summary carries the refused poll's own text after the control output | rule (no shape change; [emulator-control.md](emulator-control.md), "Tool dispatch, timeouts and wait criteria", coordinator-approved change) (host gate row "EMU-BIND") | EMU-BIND | EMU-BIND | `launch_err_code != 0` still fails at once; MuMuManager 6.8.4.0 refused one `info` poll while the VM of the first launch after a reboot came up; `exit_code` stays the control command's |
| EMU-BIND-4 | EMU-BIND fix 1 (0.12.28; review `rd-1029/review-w2-emu-bind.md` M1, L1, L2; coordinator work order `rd-1029/brief-emu-bind-fix1.md`): the watch also recognizes an instance bound to the reported port that holds no `backend.selfcheck.*` fact, whose key is idle and that neither its idle action, a `released` instance pause nor a #403 fence holds; the recognition claim reads the status again on its key and ends `emulator_autostart_skipped` (`watch_kind` `recognize`, `process_started`, `android_started`, `adb_port_present`; also a `device_state_observed` record) without preparing when none is needed any more; `watch_kind` also on `emulator_autostart_failed`, `_interrupted`, `_superseded` | rule, and new optional values of existing codes (no shape change; [emulator-control.md](emulator-control.md), "Emulator watch") (host gate row "EMU-BIND") | EMU-BIND | EMU-BIND | the M1 case: a Start that bound the port and failed at its ADB baseline (`emulator_control_adb_not_ready`) or at its deadline with a read that saw the emulator running; same bound and claim as EMU-BIND-1; no new code, key or configuration key |

### 1.159 Slice Lab B-3 S0 (0.12.x): the LR-10 record shape of a Lab operation that runs a declared operation; contract and readers, no writer (#412 B-3; design `rd-1021/train-0129/lab-b3-design.md` §1.1, §3, §4, §5 S0; coordinator 10-21 22:2x: the design's twelve open questions as recommended)

`actinglab do --operation` (Lab B-3 S2) runs one operation its package declares as the kernel runs
it. This slice writes its shape as BT-LR wrote the B train's: the types, the record validation, the
shared evidence verifier, the client's and `resource restore`'s readers and one round trip per
shape, and **no writer**. The shape is LR-10 of §1.153 and joins ledger revision 4: `ledger` stays
[3, 4]; `actingcommand.event.v3`, SQLite format 2, the tables, the indexes and the schema strings
are unchanged; `runtime-client` stays [2, 2] (only actinglab and actingctl of the same distribution
send the selection, as for L3-1 and W5). No code, location or configuration key is added; no pack
member, so `package` is unaffected. [contained-lab-operation.md](contained-lab-operation.md)
("A declared operation") is the contract. Gates: contract keys K39 to K42.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B3-1 | the selection `{mode: "operation", operation_id, seed?}`; prepared `operation {phase_id?, from_page, destination_pages, max_attempts, recheck_frames, guard, sampling?}`; terminal `operation_result {end, final_page?, outcome_key?, attempts, frames}`; `guard` `{status: trusted_coordinate}` or `{status: passed, page_id, target_id, target_kind}` | LR-10 (contract K39) | Lab B-3 S0 | Lab B-3 S2 | new structs deny unknown members; the optional members are absent when empty, so element and coordinate records serialize byte for byte as before; `destination_pages` at most 16, `frames` at most 256; amended by S2 before its first writer, 10-22: the guard form `{status: not_evaluated}` (no other members), only when `attempts` is empty, which validation requires (`not_evaluated` with attempts, or `passed` / `trusted_coordinate` without, is `invalid_lab_operation_record`) |
| B3-2 | validation of a record with the two members | rule (contract K40) | Lab B-3 S0 | Lab B-3 S2 | `invalid_lab_operation_record`: the members together and only with the selection `operation` (one without them is a request that failed before any input); no `geometry`, `selected_element`, `before_projection`, `after` or `arrival`; attempts 1, 2, … at most `max_attempts`, each intent after the previous attempt's outcome (the first after the verified prepared record) and before its own outcome; each input bound to a frame of its attempt before the input; frames in capture order; the record's `input_*` the last attempt's, `effect` the aggregate; `end` `confirmed` or `run_completed` exactly without a failure or with one at `release`; the prepared record states attempt 1 (input frame, action, guard, draw); the after frame is the last frame. A request with the selection and an arrival condition, and an empty or NUL operation id, is `invalid_lab_operation_selection`; `resolve` refuses the selection with that code |
| B3-3 | the shared verifier's declared-operation branch | rule (contract K41) | Lab B-3 S0 | Lab B-3 S2 | every capture frame of the request is a listed frame (`runtime_lab_frame_budget_mismatch`, in place of the transaction accounting); the diagnostics are the prepared and terminal records and at most the after projection; one `InputIntent` and one outcome per attempt that reached its input, matching its action, and no other input event of the request (`runtime_lab_unreported_input`); no resolution of the selection; the final event's effect is the aggregate; a complete operation kept its lease to its last observation; amended by S2 before LR-10's first writer, 10-22: a complete record without attempts (a keyed end before any input) needs no before frame, `runtime_lab_before_missing` only for one whose attempts recorded an input |
| B3-4 | `resource restore` of such a record | rule (contract K42) | Lab B-3 S0 | Lab B-3 S0 | the gap `lab_declared_operation`; no operation is restored from it; its provenance names `end` (decision Q12) |
| B3-5 | the action events of such a record | reader (no shape) | Lab B-3 S0 | Lab B-3 S0 | `RuntimeClient` and actinglab's `resource restore` query each attempt's `input_action_id` |
| B3-6 | key `lab_operation_problem` (`vocab:lab_operation_problem`); vocabularies `lab_operation_end` (`confirmed`, `run_completed`, `passed_over`, `recovery_requested`, `failed`) and `lab_operation_problem` (`application_effect`, `select_step`, `linear_steps`, `recognize_only`, `chosen_identity`) | new key-table and vocabulary entries (no shape) | Lab B-3 S0 | Lab B-3 S1 (the problem), S2 (the end) | both vocabularies `pending`; the enums `LabOperationEnd` and `LabOperationProblem` are the contract's; S1's `contained_task_declared_operation_unsupported` carries the key |

### 1.160 Slice A43b (0.12.x): a wait that sees a still screen it cannot recognise is not stretched by the pacing factor (#411 A43; coordinator 10-21, Q8 of 10-21 20:2x)

The production run of 10-21 00:2x waited for a page that never came at the pressure floor's
factor 1250 with its extra round; the factor only made the empty wait longer. The rule uses the
one frame-change measure of §1.155 B12-5 at its default; it adds no captured frame.
[performance-control.md](performance-control.md) ("Step pacing") is the contract. **No persisted
ledger shape**: no code, key, vocabulary token, configuration key, location or package member.
Gates: exec keys 133 to 135.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A43b-1 | a page wait (`capture_until_page`) or a step's confirmation (`await_postcondition`) on a still, unrecognised screen | rule (no shape change) (exec 133 to 135) | A43b | A43b | "still and unrecognised" holds while the wait's latest two frames are unchanged by the frame-change measure (the whole frame, still at most 1500 milli) and neither matched a page the wait accepts (any page of the package for the page wait, the step's destination for the confirmation); a wait's frame is the frame its observation recognised; while it holds, the rest of the wait is counted at the factor 1000 (its pauses last their declared length and its wall time counts in full against its limit), otherwise it is paced as before; a page it accepts or a changed frame returns to the step's pacing at once; a handled shared interrupt page or a failed observation clears it; the run's budget clock, the task deadline, the extra wait rounds and the outcome of a wait that runs out (its code, `elapsed_ms` as the wait counted it, `limit_ms`) are unchanged; at the factor 1000 nothing changes |

### 1.161 Slice Lab B-3 S1 (0.12.x): the kernel runs one declared operation as a run would, and returns after that one step (#412 B-3; design `rd-1021/train-0129/lab-b3-design.md` §0 item 2, §1.1, §2.1, §2.2, §4, §5 S1; coordinator 10-21 22:2x: the design's twelve open questions as recommended, Q2 page-graph clicks only)

`actinglab do --operation` (Lab B-3 S2) runs one operation its package declares as the kernel runs
it. This slice is the kernel's half: a declared-operation mode of the page-graph run, not a copy of
its step, so the operation's `when`, guard, input recheck (`still_frames` included), point draw and
landing-history choice, gesture, `post_delay`, `expect_after` and retries run unchanged. The entry
waits for a page with `capture_until_page` as a run does, A43b's rule included (§1.160). **No
persisted shape**: the mode writes no record of its own (LR-10's writer is S2), adds no location,
key, vocabulary token, configuration key or package member; `runtime-client` and `ledger` are
unchanged. The Runtime-internal interface is the kernel's. Gates: exec keys 136 to 143 and 180.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B3S1-1 | `PreparedContainedTask::run_declared_operation(runtime, operation_id) -> DeclaredOperationOutcome` (`Confirmed {page}`, `RunCompleted {page, outcome_key}`, `PassedOver {refusal}`, `RecoveryRequested {failure_reason, after_page}`) | Runtime-internal interface (memory only) (exec 136, 138 to 142, 180) | Lab B-3 S1 | Lab B-3 S1 | the six points of design §2.1: no post-admission OCR, OCR fields or reading relay; the entry always `capture_until_page` (no required entry, no application entry); the declared operation is the only candidate, with no target page, stage, designated cap (no fact is read) or per-run cap, and one step; the run returns once that step ended without ending the run (confirmed on its page, passed over on a guard refusal with `on_guard_refused: next_candidate`, which keeps the refusal, or a failure decision that asked for recovery, which it does not run); no stability termination; a success that ends the package's run (a keyed end) is `RunCompleted`; a failure is the run's error with its outcome, as a run's |
| B3S1-2 | `PreparedContainedTask::declared_operation(operation_id) -> DeclaredOperation {phase_id?, from_page, destination_pages, max_attempts}` | Runtime-internal interface (memory only) (exec 143) | Lab B-3 S1 | Lab B-3 S1 | admission before any lease: an operation of the entry task; the stage that lists it, its `from` as declared, its `to` or `expect_after` pages, its retry policy's attempts (1 without one): what LR-10's prepared `operation` states before the run |
| B3S1-3 | `contained_task_declared_operation_unknown` (`pack_operation_id`), `contained_task_declared_operation_unsupported` (`pack_operation_id`, `lab_operation_problem`), `contained_task_declared_operation_page_mismatch` (`pack_operation_id`, `page_id`, `page_ids`); all `error`, `ladder_class` `none` (decision Q7), every value required | new codes (pending) (exec 137, 143) | Lab B-3 S1 | Lab B-3 S1 | unsupported: `application_effect`, `select_step`, `linear_steps`, `recognize_only`, `chosen_identity` (the key and vocabulary are S0's, §1.159 B3-6), and `wait`, a wait step (added with A35-S3, §1.176 S3-6, exec 179); page mismatch: the run's first page is not the operation's `from`, no step starts and nothing is input |

### 1.163 Slice BT-B1 (0.12.x): a number's dictionary and multi-frame agreement; a value the frames do not agree on is no value (#411 B1, B40; B-train plan `rd-1021/train-0129/b-plan/plan.md` §2.1 and §6 B40; coordinator decisions Q6 and Q8 D1 of 2026-10-21)

A value-test side and a reading mark read their number with a dictionary of their own and may
require several frames to read one value. This slice is the first writer of ledger revision 4
(§1.153): LR-5 (`no_agreement` in a value-test side's `problem`) and LR-6 (the guard phase
`agreement_sample`, one row per agreement frame), as registered there; it adds no other persisted
member. No code, location, key or configuration key is added; the vocabulary `reading_problem`
gains `no_agreement` with its text (`value_problem` `no_agreement` came with BT-LR). Declaration
refusals reuse `resource_declaration_invalid` (the declaration gate, the recognition pack) and
`contained_task_resource_reading_invalid` (Runtime admission of a mark, detail
`resource_declaration_invalid`). A side its frames do not agree on is unread:
`value_test_unresolved` with `value_problem` `no_agreement`, so a page that requires the test
does not match and a guard on it refuses (`contained_task_guard_refused`,
`cause=value_test_unresolved`); a mark writes no fact and records the warning
`contained_task_resource_reading_unresolved` with `reading_problem` `no_agreement`. No default
value and no last value is ever used. Both members join `package` revision 6, which BT-PK6
declares at the end of the B train; until then they are admitted by code. Without a dictionary
and an agreement, behaviour and records are byte for byte as before. Not covered (D1): the
`ocr_integer` candidate feature's dictionary (a follow-up) and operation `0.8` fields. B40 is
covered by the per-field confusions (plan §6) and has no test of its own. The contract:
[ocr-fields.md](ocr-fields.md) (the Workflow #411 B1 paragraph),
[resource-declarations.md](resource-declarations.md) (value tests),
[resource-readings.md](resource-readings.md) (Declaration, Kernel reading),
[business-identity-consensus.md](business-identity-consensus.md) (Finite consensus). Gates: exec
keys 144 to 146 and host keys 340 to 342 (the host family is not in the slice's row: 300 + 10 × 4,
wave-3 frame; 342 is follow-up BT-B1-FIX: a recognition whose sampling deadline passed latches no
fatal and is observed again); contract keys 43 to 46, exec keys 147 to 151 and apps keys 93 and 94
unused.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B1-1 | `dictionary` `{confusions?, affix?, digits?}` on the `unsigned_integer` `value` of a value-test side and of a reading mark (`IntegerDictionary`) | new optional member (resource file format, not a ledger shape) (exec 144, 145) | BT-B1 | resource side | `confusions` 1..=32 `[from, to]` pairs (`from` one character named once, `to` one ASCII digit), `prefix` and `suffix` 1..=64 bytes, `1 <= digits.min <= digits.max <= 20`; after the trim and before the parser the affixes are stripped when present, the confusions mapped and the digit count checked (else `invalid` / `invalid_integer`); `raw_text` stays verbatim and the value shows the substitution; `IntegerDictionary::apply` is the one implementation; no global table |
| B1-2 | `agreement` `{frames, interval_ms, k?}` on a value-test side and on a reading mark (`IntegerAgreement`) | new optional member (resource file format) (exec 145, 146) | BT-B1 | resource side | `frames` 2..=5, `interval_ms` 1..=1000, `k` 1..=`frames` (`frames` when absent); a mark's `k` is its `frames`, refused otherwise at `…/agreement/k`; a value test's agreements share the pack's one frame interval with its multi-frame target consensus (refused at `…/agreement/interval_ms`) |
| B1-3 | a value test with an agreement | rule (writes LR-5 and LR-6) (host 340) | BT-B1 | BT-B1 | sampled as a target under target consensus is, through an entry the evaluator holds (one unperturbed sample on each of its frames, never declared; a declared `target_consensus` on a value test stays refused), so each observation takes its frames; read on its last F frames (F the largest `frames` of its sides), both sides on each, each frame's evaluation recorded as a sample with the guard phase `agreement_sample`; a side with an agreement has the one value read on at least `k` of its frames (`k_of_n_value`, the `k_of_n` rule of candidate features), else `problem` `no_agreement`; a side without one has its current-frame read; not a `when` target |
| B1-4 | a reading mark with an agreement | rule (no ledger shape) (host 341) | BT-B1 | BT-B1 | after the frame its page was reached on, `frames - 1` more captures (the largest of the page's marks), each the largest declared interval after the one before, each recorded as a capture and a frame handed to recognition and joined after it with no readings of its own; the mark holds only when each of its frames gives the same readings, at their lowest confidence, else the warning with `no_agreement` and no fact; a frame's provider failure or join timeout keeps that problem; a runtime that does not sample refuses with `recognition_sampling_runtime_unsupported` |
| B1-5 | `reading_problem` `no_agreement` | new vocabulary token (catalog text) | BT-B1 | BT-B1 | the frames of the mark's agreement did not all read the same value; the mark writes no fact |
| B1-6 | a recognition whose observation's sampling timed out (follow-up BT-B1-FIX, coordinator ruling 2026-10-22) | rule (no ledger shape: the existing `recognition.failed` and its revision-3 `outcome`; no new code) (host 342) | BT-B1-FIX | BT-B1-FIX | the deadline after a frame's capture is checked before its recognition starts; a recognition whose evaluation finished records its completion before the timeout; one the timeout cut short (or that matched more than one page of an exclusive set) has no result and is closed, before the observation is made again or the run ends with the timeout, by its `recognition.failed` (`diagnostic_code` `recognition_failed`, `outcome` `recognition_sample_deadline_exceeded` with the retries made so far, the window and the stage) under its own frame and recognition; the host's check that no recognition starts while one is open is unchanged |

### 1.162 Slice Lab B-3 S2 (0.12.x): the host runs one declared operation over the Lab lease and writes its LR-10 records; client, CLI (`do --operation`) and MCP (`ac_lab_do.operation`) (#412 B-3; design `rd-1021/train-0129/lab-b3-design.md` §1.2, §1.3, §2.3, §2.4, §2.5, §3, §4, §5 S2; coordinator 10-21 22:2x: the design's twelve open questions as recommended)

`actinglab do --operation` runs one operation its package declares as the kernel runs it (§1.161),
over the Lab lease. This slice is LR-10's writer (§1.153, §1.159): the records are written as S0
registered them, with the two amendments S2 made before LR-10's first writer (coordinator 10-22:
the guard `not_evaluated`, and a complete record without attempts verifies), the prepared record
persisted and verified before attempt 1's input; `ledger`
stays [3, 4] and `runtime-client` [2, 2] (decision Q8); no other persisted member, no
configuration key, location, key or vocabulary token, and no package member. One code is new.
Element and coordinate selections are unchanged. [contained-lab-operation.md](contained-lab-operation.md)
("Running a declared operation") is the contract. Gates: host keys 152 to 155, apps keys 90 to 92.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B3S2-1 | the Runtime's answer to a request with the selection `operation` | rule (host 152) | Lab B-3 S2 | Lab B-3 S2 | the selection is no longer refused at `selection`: the kernel admits the package (with the Runtime's provider) and the operation before any lease, a refusal a Denied receipt with no record; the lease lasts the package's task timeout and the longest input (`MAX_INPUT_DURATION_MS`), at most the longest response deadline, plus the heartbeat reserve; a request whose lease was not granted captured nothing and its records state no declared operation; the kernel then runs `run_declared_operation`, and once the operation ended `confirmed` or `run_completed` its last frame is projected after the run |
| B3S2-2 | `LabOperationRuntime` and the LR-10 records it writes | rule (host 152, 153, 155) | Lab B-3 S2 | Lab B-3 S2 | captures are the lease's, fenced as an element operation's and each listed as a frame entry (attempt: the attempt started last, 0 before the first; an attempt's input frame, the frame captured last at its effect intent, is that attempt's; `after_input` after its `EffectCompleted`; `page` the page the kernel matched); inputs follow the handset keepalive (Q4), go through `HostShared::input` with the kernel's frame (a frame other than the last captured one is the fatal `contained_task_effect_frame_identity_mismatch`), and each attempt's intent and outcome are resolved as an element operation's, from after the previous attempt's events; the draw uses the touch keys in effect and the instance's landing history shared with runs (Q1); attempt 1 draws from the request's seed or one derived from the request, attempt k > 1 from (that seed, k) (Q9); factor 1000 and the configured observe-only retries; no shared interrupt (Q3), application effect, selection state or task, fact or scheduling record; attempt 1's effect intent persists and verifies the prepared record (operation, input frame, action, guard, draw, recheck frames) before it returns, and a failed persist stops the operation before its input without a record; with no attempt the prepared record states the operation as admitted (no input frame, action or draw; its `guard` `not_evaluated`, coordinator ruling 1 of 10-22) and is persisted after the run, and a keyed end before any input is such a `run_completed` record that verifies (ruling 2; host 152); stages: before the current attempt's input `selection`, after it `arrival`, a capture's `before_frame` or `after_frame`, an input's `input`; `passed_over` answers the guard's refusal, `recovery_requested` its failure reason |
| B3S2-3 | `lab_operation_frame_limit_reached` (`count`, `limit_count`), error, `ladder_class` `none` (decision Q7), both values required | new code (pending) (host 154) | Lab B-3 S2 | Lab B-3 S2 | the capture after the 256th of a declared operation is refused before anything is captured (`count` 257, `limit_count` 256) and ends the operation at `before_frame` or `after_frame`; no input follows it |
| B3S2-4 | `RuntimeDebugSession::run_contained_lab_declared_operation(alias, request, frames, task_timeout)` and its receipt wait | client interface (Rust API, no wire change) (apps 90) | Lab B-3 S2 | Lab B-3 S2 | the receipt wait is the package's task timeout (the client's kernel admission), four backend-open allowances, `MAX_INPUT_DURATION_MS`, the I/O allowance and one projection transaction; `run_contained_lab_operation` takes the element and coordinate selections, and each method refuses the other's (`runtime_lab_operation_request_invalid`) |
| B3S2-5 | `actinglab do --operation <id> --capture` | CLI (apps 91) | Lab B-3 S2 | Lab B-3 S2 | refuses before any connection an element id, `--tap`, `--swipe`, `--after-page`, `--after-timeout-ms`, `--record`, `--tap-rect`, `--projection-sequence`, `--projection-hash`, `--dry-run` (do's and the global one), `--scene` and `--operation` without `--capture`: `validation_failed`, exit 2, `argument_problem` `conflicting` (Q6, Q10); runs the kernel's admission of the package and the operation first (an operation it does not run this way is its own code, exit 2); answers `operation`, `end` and `attempts`; exits 0 on `confirmed` or `run_completed`, 3 blocked by `page`, `guard` or `arrival_page` as design §1.2's table says, else 4 |
| B3S2-6 | `ac_lab_do.operation` | MCP argument (apps 92) | Lab B-3 S2 | Lab B-3 S2 | a text passed as `--operation`, `seed` read with it; with target, tap, swipe, after_page, after_timeout_ms, record, tap_rect, projection_sequence, projection_hash, scene, sample_scenes or dry_run, or without `capture: true`, refused `arguments_invalid {argument, argument_problem: conflicting}` before any child runs |

### 1.164 Slice BT-KEY (0.12.x): a per-run operation cap and a guard refusal end on a named key (#411 B16 part 1, B22, B33; B-train plan `rd-1021/train-0129/b-plan/plan.md` §2.11, D10, Q5, Q9)

An operation of a task schema `0.9` page-graph task may cap how often one run dispatches it
(`max_runs` with `on_max_runs`), and a click's guard refusal may end the run on a named key (the
keyed `on_guard_refused`; B33 is merged into B22). `on_empty`, a keyed `on_max_runs` and a keyed
`on_guard_refused` follow **one keyed-terminal rule** (`keyed_terminal_holds`), which later B23 and
B18 reuse. **Persisted shape:** this slice is the first writer of ledger revision 4 row LR-7
(§1.153), `task.operation_capped`, written as registered; it adds no persisted member. B22 adds
no code and no record. The new members join `package` revision 6, which BT-PK6 declares at the
end of the B train; until then the Runtime admits them by code. No location, key, vocabulary
token, wire or configuration key is added. [resource-declarations.md](resource-declarations.md)
("Per-run operation caps and keyed guard refusals"), [README.md](README.md) and
[linear-steps.md](linear-steps.md) are the contract. Gates: contract key K47, exec keys 152 to 157
(contract 48 to 50 and exec 158, 159 unused).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| KEY-1 | `max_runs` (1..=1000) and `on_max_runs` (`"next_candidate"` \| `{"outcome_key": K}`) of a task operation | new optional members (resource file format, not a ledger shape) (exec 152, 153; contract K47) | BT-KEY | resource side | declared together, task schema `0.9`, page-graph tasks only (`linear_steps`: `contained_task_linear_invalid`, `reason=max_runs`); the run counts per operation the dispatches that started a step, a guard pass-over counting and a `when` withdrawal not (D10); capped with `next_candidate` the operation is passed over like a capped designated member; capped with a key, when it is the operation the page would dispatch, no step starts and the run ends Completed on the current page with K; never on a designated operation or a member of a designated effect set (`task_phases_designated_effect_invalid` in the walk, `contained_task_outcome_declaration_invalid` in the page-graph check); a keyed one needs a `scheduling_outcome`; Runtime admission `contained_task_operation_invalid` at the member; the declaration gate refuses a wrong shape (`resource_declaration_invalid`) and an older task schema (`UnconsumedField`) |
| KEY-2 | `on_guard_refused: {"outcome_key": K, "failed"?: <target>}` | new form of an existing member (the string `next_candidate` stays) (exec 154 to 156) | BT-KEY | resource side | on a click with a declared guard; `failed` is the guard target or one member of it (a composite), absent the guard target; at attempt 1, before any input, a guard refusal with a definite fail of the named target (`value_test_not_met`, or not passed with no registered reason; a composite by its members' definite fails, one of `all_of`, every one of `any_of`) ends the step without an input on the frame's page (as a pass-over, EC2) and the run Completed there with K; a page mismatch, `value_test_unresolved` and every other registered reason, an evaluation error, a sampling timeout, a later attempt, an input recheck's refusal and another member failing alone keep the failure decision; the member is read from the guard target's typed evaluation, never from `cause=` |
| KEY-3 | the keyed-terminal rule | rule (no shape change) (contract K47) | BT-KEY | BT-KEY | a keyed end's state (its page and effect condition) is a terminal needing exactly one mapping with that condition listing the page whose key is K; reachable under both conditions it is refused; the phased walk (`task_phases_outcome_incomplete`) and the page-graph coverage check (`contained_task_outcome_declaration_incomplete`, detail `operation=<id> <member> page=<page> …`) share `keyed_terminal_holds`, generalised from `on_empty`'s |
| KEY-4 | `contained_task_operation_runs_capped` (info, `ladder_class` `none`; values `pack_operation_id`, `count`, `limit_count`, `page_id`, `outcome_key`, all required) | new code (pending) (exec 152) | BT-KEY | BT-KEY | the outcome of `task.operation_capped` only (coordinator decision Q5) |
| KEY-5 | `task.operation_capped {step_index, operation_label, count, limit_count, outcome}` | first writer of LR-7 (§1.153) (exec 152) | BT-LR | BT-KEY | written once, before the run's `Finalizing`, when a keyed cap ends the run: `step_index` is the index the capped dispatch would have taken (the run's executed steps), `count` the operation's dispatches, `limit_count` its `max_runs`; an operation capped with `next_candidate` writes none, so a run has at most one |
| KEY-6 | `contained_task_prerequisite_incompatible` detail `reason=max_runs_end`, `reason=guard_refused_end` | new values of an existing refusal detail (exec 157) | BT-KEY | BT-KEY | a package with a keyed `on_max_runs` or a keyed `on_guard_refused` is neither a prerequisite package nor a return-home package (coordinator decision Q9); a `busy` mapping (§1.152) is no keyed end and is unchanged |
| KEY-7 | `TaskPhaseOperation.max_runs` (`TaskPhaseRunCap {limit, outcome_key}`), `TaskPhaseOperation.on_guard_refused_key`, `keyed_terminal_holds`, `MAX_TASK_OPERATION_RUNS` (now public) | Runtime-internal interface (memory only) | BT-KEY | BT-KEY | the walk's view of the new members, shared by source parsing, build and Runtime admission |

### 1.165 Slice BT-B13 (0.12.x): brightness and contrast floors for template targets, relative to the template itself (#411 B13; B-train plan `rd-1021/train-0129/b-plan/plan.md` §2.10, D9; coordinator decision Q8 D9 of 2026-10-21)

A template target may declare floors on how bright and how contrasted the window it matched is,
each relative to the template itself; a candidate below them does not qualify in the one joint
acceptance of the template search, so the target does not pass. **Persisted shape:** this slice
is the first writer of ledger revision 4 row LR-2 (§1.153), `TaskDiagnosticTemplateData.luma`,
as registered there; no other persisted member. No code, location, key, vocabulary token, wire
or configuration key is added; the refusals reuse `resource_declaration_invalid` reasons and the
recognition pack's existing load refusal. The member belongs to `package` revision 6, which
BT-PK6 declares at the end of the B train (plan §4); until then it is admitted by code.
[resource-declarations.md](resource-declarations.md) ("Task schema `0.9` brightness and contrast
floors") and [template-relative-color.md](template-relative-color.md) are the contract. Gate:
exec key 160 (161 to 164 unused).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B13-1 | `luma_floor: {mean_ratio_milli?, contrast_ratio_milli?}` on `anchors[]` and `verify_templates[]` entries; derived onto the template target | resource file format (task schema `0.9`, no version change; pack schema `0.7`) (exec 160) | BT-B13 | resource side | integers, each optional; refused `resource_declaration_invalid` at `…/luma_floor` (`UnconsumedField` before task schema `0.9`, `InvalidType` for no object, `UnknownField` for another member, `InvalidType` at a member that is no non-negative integer); a source that uses it derives pack schema `0.7`; a `0.6` pack refuses it (`UnconsumedField`), and the pack refuses a member outside `0..=u32::MAX` (`InvalidValue`); a `repeated_anchor` anchor or an offset feature that declares it is refused when the evaluator is built, and template-region batch evaluation refuses it |
| B13-2 | the floor in the joint acceptance of a template search | rule (no shape change) (exec 160) | BT-B13 | BT-B13 | the window's mean and standard deviation over the template's scored pixels (its mask's, at its factor), each over the template's own, times 1000, rounded down; a candidate qualifies when each declared ratio reaches its floor, beside its relative color check; with `unique`, a second place counts only when it qualifies; a target without a floor evaluates as before; a template with no brightness or no contrast over its scored pixels is a recognition error when its floor is evaluated; not a frame-change measure |
| B13-3 | `TemplateEvaluation.luma` → `TaskDiagnosticTemplateData.luma {mean_ratio_milli, contrast_ratio_milli}` | writer of LR-2 (§1.153) (exec 160) | BT-LR | BT-B13 | the chosen candidate's ratios (the accepted one, else the best template), present only when the target declares `luma_floor`; absent keeps the revision-3 bytes |

### 1.166 Slice BT-B4 (0.12.x): a select step may confirm by its decision (#411 B4; B-train plan `rd-1021/train-0129/b-plan/plan.md` §2.4 with D3, coordinator Q8 of 2026-10-21; ledger revision 4, §1.153 LR-1)

A single-frame select step may declare `confirm: "decision"`: its confirmation frame F2 is
projected and decided again with the decision's fact snapshot and instant, and the step compares
the two decision hashes instead of the candidate-set hashes, so a feature value that changes on
F2 without changing the decision no longer stops the tap. Scan selects keep their identity
confirmation and multi-select picks their own path (D3). **Persisted shape:** this slice is the
first writer of LR-1 (§1.153), `decision_matched` and `decision_mismatched`, written as
registered there and only as the top-level confirmation of a v1 record without `multi_select`;
no other member. A step without `confirm`, or with `"candidate_set"`, records exactly what it
recorded before. One new code; no location, key, vocabulary token, wire or configuration key.
`select.confirm` joins `package` revision 6, which BT-PK6 declares at the end of the B train;
until then this Runtime admits it by code and the declaration stays [3, 5].
[selection-graph.md](selection-graph.md) (sections Declaration, Execution, Confirming by the
decision, Records) and [candidate-projection.md](candidate-projection.md) are the contract.
Gate: exec key 165 (166 to 170 unused).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B4-1 | `select.confirm`: `"candidate_set"` (default) or `"decision"` | new optional member (resource file format, not a ledger shape) (exec 165) | BT-B4 | resource side | a single-frame select's: admission refuses `"decision"` beside `scan` (`contained_task_select_invalid`, detail `confirm=decision scan=declared`); the source gate refuses a non-string (`InvalidType`) and any other string (`InvalidValue`) at `/operations/<i>/select/confirm`, and the Runtime's typed reading refuses any other value as it refuses a malformed `select`; `null` is absent; a reader before BT-B4 refuses the member (`select` denies unknown members); joins `package` revision 6 |
| B4-2 | the decision hash `decision_sha256` | rule (the writer's definition; no reader recomputes it) (exec 165) | BT-B4 | BT-B4 | the SHA-256, 64 lowercase hex digits, of the compact JSON `{"candidates":[{"id","click":{"x","y","width","height"},"status"}],"selected":[…],"outcome_key"}`: the projection's actionable candidates in projection order, each with its click rectangle and its verdict's status (`ranked`, `gate_rejected`, `unknown_dropped`, or `null` for an actionable candidate kept from the evaluator), the decision's selected IDs in its order and its outcome key; no feature value, score, rank or reason; the step's own comparison hash, never a package digest (option 丙) |
| B4-3 | the decision confirmation | rule (no shape change beyond LR-1) (exec 165) | BT-B4 | BT-B4 | after F2's capture, page and guard checks and projection as before, F2 is decided with the decision's snapshot and instant; equal decision hashes record `decision_matched {candidate_set_sha256 (F2's), decision_sha256 (F2's), projection?}` and the tap follows inside the chosen candidate's click rectangle on F2, bound to F2; different ones record `decision_mismatched` with F2's hashes and fail `selection_decision_mismatch` before any input; a failure of F2's decision or of a hash records `capture_failed` with that failure's code (`selection_evaluation_failed`), as a projection failure of F2 does; the evaluated decision's own hash is not stored (a `decision_matched` hash equals it) |
| B4-4 | `selection_decision_mismatch` (error, `ladder_class` `pack_logic_mismatch`) | new code (pending) (exec 165) | BT-B4 | BT-B4 | values as `selection_projection_mismatch`'s, all optional (`operation`, `runtime_code`, `host_halted`, `receipt_state`, `request_id`, filled where the host relays a failure); the developer detail names the operation and both decision hashes; text per the B-train working notes |

### 1.167 Slice BT-B16b (0.12.x): a task's reserves gate it; of two tasks that fit alone but not together the higher score runs first (#411 B16 part 2; plan `rd-1021/train-0129/b-plan/plan.md` §2.12; [scheduling/README.md](scheduling/README.md), "Task Reserves")

**No persisted ledger shape**: the new reason travels in the existing decisions and dispatch reason
chains (`PolicyReasonRecord` is `#[serde(transparent)]` over `Outcome`), and a dispatch decision
record lists a held candidate with it as `why` (§1.145). The catalog member is V2 only and
optional; a catalog without it keeps its bytes and its hash. No package format change, no
configuration key, vocabulary token or location. Several instances that share one pool belong to
Workflow #367 (multi-dispatch) and are not handled. Gates: policy keys 64 to 66 (67 unused).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B16b-1 | `TaskSpec.reserves: Option<Vec<ReserveSpec>>`, `ReserveSpec { pool_id, at_least }` (V2 `tasks.schema.json` `reserves`, 1 to 128 entries) | catalog member (V2 only; absent serializes as nothing) (policy 64) | BT-B16b | BT-B16b | `at_least` in `0..=capacity` of the named pool (`limit_exceeded` at `at_least`); an undeclared pool `dangling_reference` and a pool named twice `duplicate_id` at the entry's `pool_id`; an empty or longer list `limit_exceeded` at the member; a V1 document that names it is refused `unknown_field` at `/tasks/<i>/reserves` |
| B16b-2 | the selection document's candidate fields `task.consumes.<pool>`, `task.after.<pool>`, `task.reserve_gap.<pool>` (integers) | evaluator interface (memory only) (policy 66) | BT-B16b | BT-B16b | per reserved pool of the candidate's task: its declared consumption (the sum of its `consumes` amounts for the pool, 0 without one), `after = projected − consumes` (the pool's projection at the evaluation, as `resource_projection` reads it) and `after − at_least`; the last two only while the pool's amount is known; a catalog with reserves runs the score stage even without a document, value, offset or target, and every candidate without reserves keeps its rank, reasons and fields |
| B16b-3 | `resource_reserve_insufficient` (info, `none`; `pool_id`, `reserve_at_least`; optional `amount_after`, `task_id`) | new code of existing decision reasons (policy 64, 65) | BT-B16b | BT-B16b | the last reason of a candidate the reserve gate holds this round, its decision deferred: on each instance the ranked candidates are taken by `effective_milli`, the higher first, and a candidate with reserves is held when for one reserved pool the amount is unknown (no `amount_after`), or `after < at_least`, or `after` less the consumption of other tasks dispatched on the instance at or after the pool's latest observation is below it (`task_id` the first such task), or that less the consumption of a candidate taken before it and not held is below it (`task_id` that candidate's task); a regenerating pool wakes the evaluation at its next projection step; review `pending` |
| B16b-4 | keys `pool_id` (name), `reserve_at_least`, `amount_after` (integers) | new key-table entries (no shape) (policy 64) | BT-B16b | BT-B16b | used by `resource_reserve_insufficient` only |

### 1.168 Slice A35-S2 (0.12.x): settle gate: after a listed return-home failure the ladder probes the screen, and while it still changes it holds instead of restarting (#411 A35; design `rd-1021/train-0129/a35-design.md` §2.1, §3, §4.3, §5.1 S2, §5.3; coordinator decisions of 2026-10-21, Q8 revised: the frame-change measure of §1.155)

The A35 incident: an instance was in a long activity that ends by itself, its return-home package
did not know the activity's screen and failed `contained_task_page_unknown`, and the ladder went
on to the application restart at once and closed the application in the middle of it. After a
return-home failure of a listed ladder class, the ladder now probes the screen with read-only
frames and compares consecutive frames by the Runtime's one frame-change measure (§1.155 B12,
[color-digest.md](color-digest.md), "Frame change"; no second measure, no thumbnail similarity):
a settled screen is climbed past as before, a changing one is held within a configured limit.
[emulator-control.md](emulator-control.md) ("Settle gate", "Records") is the contract,
[runtime-config.md](runtime-config.md) the keys'. **No persisted ledger shape** (design §7): the
verdicts are recorded with the existing `recovery_rung_finished` and `recovery_ladder_finished`
phases and one new code; the probe's frames are ordinary read-only captures; the frame-change
digest is memory only. Gates: host keys 156 to 161.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A35-1 | `recovery_rung_screen_changing` (info, `planned_stop`) | new code of an existing record (host 156, 158, 161) | A35-S2 | A35-S2 | the outcome of a `failed` return-home `recovery_rung_finished` whose probe found the screen changing: `count` (frames probed), `score_milli` (the largest frame change between two consecutive frames), `held_ms` (since the climb's first changing verdict), `limit_ms` (`busy.screen.max_wait_ms`) required, `retry_after_ms` optional (absent when the hold ends); caused by the run's own failure code; matches no default trigger item, and direction ① ignores a climbing instance's records. `recovery_ladder_hold_exhausted` (§1.152 B37-2) also ends a screen hold under `on_exhausted = stop`, caused by the last `recovery_rung_screen_changing` (its pending description says so) |
| A35-2 | the screen hold | rule (no shape change) (host 156, 158) | A35-S2 | A35-S2 | after a changing verdict, while `held_ms` is below `max_wait_ms`: S1's hold (key renewed to the hold plus `hold.margin_ms`, holding check every `busy.poll_ms`) for `screen.recheck_ms`, no longer than what is left, then a new probe; settled, the return-home rung runs again; at the limit `screen.on_exhausted` (`escalate`: the next rung; `stop`: end without a restart); `max_wait_ms` 0 probes only; one record per changing verdict, so the return-home rung may have several records in one ladder |
| A35-3 | the settle gate and its probe | rule (no shape change) (host 156 to 161) | A35-S2 | A35-S2 | a return-home failure (not a holding interruption) whose code's ladder class `screen.classes` lists, on a climb with a return-home package: holding check, key renewed to `(probe_frames - 1) * probe_interval_ms` plus `hold.margin_ms`, one host-minted `ObserveReadonly` request under the ladder's causation id (`command.received`, `command.validated`), `probe_frames` keyless observer captures `probe_interval_ms` apart (§1.130 L5's transaction, #411 A49's observer frame); two consecutive frames are unchanged when the measure is at most `change_max_milli`; settled: recorded and climbed past as before; a probe capture that fails or is refused (its own capture records), or frames the measure cannot compare (`capture_frame_invalid` caused by the measure's refusal, a lifecycle failure record): climbed past as before; a climb that starts past `return_home` is not gated (Q4) |
| A35-4 | `recovery_ladder.busy.screen.classes` (list of ladder class tokens, each once, `["pack_logic_mismatch"]`), `.probe_frames` (2..=5, 5), `.probe_interval_ms` (100..=10 000, 1 000), `.change_max_milli` (unsigned 32-bit, 1 500), `.recheck_ms` (at least `busy.poll_ms`; plus the margin within the longest renewal, 15 000), `.max_wait_ms` (0..=86 400 000, 900 000), `.on_exhausted` (`escalate` \| `stop`, `escalate`) | configuration keys (§5 row; `config.parameters.recovery_ladder` rows) (host 159, 161) | A35-S2 | A35-S2 | settable, part of the section's digest, a ladder holds by its admission snapshot; defaults only in `actingd.defaults.json`; at load and in check-config `recovery_ladder_config_invalid` (`code_unknown`, `wrong_type`, `duplicate_id`, `bound_out_of_range`, `renewal_cap_exceeded`, `out_of_range`), through an instruction `config_value_invalid` with the same key and why; `busy.poll_ms`'s range now ends at the shorter of `busy.outcome.recheck_ms` and `busy.screen.recheck_ms` |
| A35-5 | `actingd-config` [5, 9] | interface revision (§6; K27), raised (wave-3 frame) | A35-S2 | A35-S2 | revision 9 is revision 8 plus the configuration keys of the 0.12.x release after v0.12.9; a Runtime before that release refuses a file that names `recovery_ladder.busy.screen` (the section denies unknown members); a file that names none keeps its revision, so revisions 5 to 8 are still read |
| A35-6 | `CompletedReadonlyObservation.change_digest` (the frame's frame-change digest) | memory only (no wire, no record) | A35-S2 | A35-S2 | taken from every read-only capture's frame before it is persisted; a frame the measure cannot digest fails the observation `capture_frame_invalid` with its capture and recognition failure records, as an unpublished frame layout does |
| A56-1 | entry pages on a recheck | rule (no shape change; since 0.12.25) | W1-3 (#411 A56, P10) | W1-3 | #411 comment 6355995369 fix 1, Friday review item 8 ([emulator-control.md](emulator-control.md), "Entry pages on a recheck"): on a recheck (a probe after a hold; the first probe makes none), a changing probe's last frame is compared with the return-home package's entry pages (its declared `entry_page`, one page or a set; a `linear_steps` package's first step page): the package the rung runs, admitted again, the frame read back from its artifact through the display adapter's frame gate under the game's declaration; a page shown: the return-home rung runs again as after a settled probe, no changing verdict recorded for that recheck; none shown, or a package whose entry lists no page: held as A35-2; a comparison that cannot be made: a lifecycle failure record (`runtime.failed`, its own code, under the frame's links) and held as A35-2; no new code, key or record. The live stop-loss instruction `recovery_ladder.busy.screen.max_wait_ms` 120000 is cleared by the coordinator after the deploy |
| A56-2 | `PreparedContainedTask::entry_pages_on_frame` | library API addition (since 0.12.25) | W1-3 | the recovery ladder | the pages of the package's entry (`navigation_entry_pages`) a frame shows, through the frame gate (`Resolution::validate_frame`) and one scene evaluation (`matching_pages_on_scene`); empty, the frame untouched, for an entry that lists no page; a gate refusal or evaluation error is the kernel's own code |

### 1.169 Slice A35-S4 (0.12.x): a busy end of an entry layer ends the run (#411 A35, B37 entry part; design `rd-1021/train-0129/a35-design.md` §2.2 "Entry extension", §5.1 S4, §5.3; coordinator decision Q5 of 2026-10-21)

A35 seen from a run's entry: a run that starts while its instance shows an activity that ends by
itself runs its home entry's return-home layer, or a step of its navigation recovery chain, and
that layer ends on the activity page under a `busy` mapping (§1.152 B37-1). Before, the home entry
failed `contained_task_home_recovery_terminal_non_home` (`pack_logic_mismatch`, which starts the
ladder) and the navigation located again and ran the chain's next step, which may restart the
application. Now such an end ends the run. [linear-steps.md](linear-steps.md) ("Ladder and
page-graph home entry") and [package-reference.md](package-reference.md) (the navigation's
"Recovery chain") are the contract. **No persisted ledger shape**: the existing
`EntryRecoveryCompleted`, fail-closed `EntryTargetDisposition` and terminal records carry one new
code; no configuration key, no location, no package member (S1's `busy` is read). Gates: host
keys 162, 163 (164 unused).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B37E-1 | `contained_task_entry_busy` (warning, `planned_stop`; `package_id`, `page_id`, `outcome_key` required) | new code of existing records (host 162, 163) | A35-S4 | A35-S4 | the outcome of the fail-closed `EntryTargetDisposition`, the run's `task.failed` terminal and its receipt; `package_id` the layer's package, `page_id` the page it ended on, `outcome_key` the busy mapping's key; it matches no default trigger item, so no ladder starts; not in `scheduling.rerun.condition_codes` (Q5: only after SR-2b's writer); review `pending` |
| B37E-2 | a home entry's return-home layer that completed on a `busy` mapping | rule (no shape change) (host 162) | A35-S4 | A35-S4 | the run ends with B37E-1 after the layer's `EntryRecoveryCompleted`, in place of `contained_task_home_recovery_terminal_non_home`: no recheck, no navigation; the mapping is the one `no_designated_effect` mapping that lists the final page (the condition a layer reaches); a `linear_steps` return-home package too; a request's own recovery package declares no `scheduling_outcome`, so only the registry's package can end so |
| B37E-3 | a navigation recovery chain step that completed on a `busy` mapping | rule (no shape change) (host 163) | A35-S4 | A35-S4 | the run ends with B37E-1 after the step's `EntryRecoveryCompleted`, the gate closed fail-closed: the run does not locate again and the chain's next step is neither admitted nor run; a hop's layer and every other layer are unchanged |

### 1.170 Slice A35-S5 (0.12.x): the status shows a recovery ladder that holds instead of climbing: what it waits for, how long it has held, its limit (#411 A35; design `rd-1021/train-0129/a35-design.md` §5.1 S5; coordinator decision Q7 of 2026-10-21)

A hold may keep an instance for up to an hour (A35-S1's default `busy.outcome.max_wait_ms`), and
until now nothing but the records said so. While a ladder's hold waits, `Status` (and so
`actingctl status`) and the instance's `ac_overview` row show it.
[emulator-control.md](emulator-control.md) ("Busy ends") is the contract,
[runtime-interface.md](runtime-interface.md) (`status`, `ac_overview`) the reads'. **Persisted
shapes:** none. The member is the ladder's state in memory, read at the status read; no record,
no configuration key, no code. Gate: host key 410 (wave-3 row 11 listed apps 95 and 96, which stay
unused: the status block is built in the host crate, and the frame gives a family the row does
not list the numbers `300 + 10 × row`).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A35S5-1 | `RuntimeInstanceStatus.ladder_busy_hold` (`InstanceLadderBusyHold {kind, held_ms, limit_ms}`, `deny_unknown_fields`; `kind` `LadderBusyHoldKind`: `busy_outcome`, `screen_changing`) | new optional member of the status projection (wire, served under runtime-client 2; absent unless a hold waits); the same type is the status inside the runtime state observation of `command.validated`, which no writer records since AO-4q (as F5 and A38-1) (host 410) | A35-S5 | A35-S5 | present exactly while the ladder's hold waits (`show_ladder_busy_hold` around the hold's wait), absent while the return-home rung runs again; `held_ms` is the time since the climb's first hold of the kind began, on the Runtime clock at the read; `limit_ms` the kind's configured `max_wait_ms` (the ladder's admission snapshot); A35-S1's busy hold shows `busy_outcome`; `screen_changing` is the vocabulary's second value, for A35-S2's changing-screen hold once it passes it (the coordinator re-checks it when A35-S2 has merged); `ac_overview` shows the status member verbatim as the row's `ladder_busy_hold` |

### 1.173 Slice A44 writer (0.12.x): the recognition record lists every candidate page's best score and verdict (#411 A44; coordinator 10-21 00:2x; ledger revision 4, §1.153 LR-8)

The recognition record listed the targets of one page only (the matched page, else the first
candidate page), so it could not show why the other candidate pages did not match. **Persisted
shape:** this slice is the writer of ledger revision 4 row LR-8 (§1.153), `candidate_scores` of
`task.recognition_completed`, written as registered there; no other persisted member. The paired
`recognition.completed` payload is unchanged and `ledger` stays [3, 4]. The kernel hands the
verdicts to the host in memory, on its in-process recognition trace. It reports only what the
recognition already evaluates: every candidate page is evaluated today (the page detector's batch
evaluates every listed page and a recognition needs every page's evaluation; none stops at the
first match), and inside a page a lazy target is skipped once the page is ruled out (Workflow
#396 REC-6). No code, location, key, vocabulary token, wire, package member or configuration key
is added. Gates: exec keys 171 and 172, host key 174 (exec 173 and 174 unused).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A44-1 | `ContainedTaskTrace::RecognitionCompleted.candidate_scores` (`Box<RecognitionCandidateScores>`) | Runtime-internal interface (memory only) (exec 171, 172) | A44 writer | A44 writer | built where each of the kernel's page recognitions records its trace (a home entry check of one page, of an entry page set, and a run's page evaluation), from the page evaluations that recognition made; nothing is evaluated for it |
| A44-2 | the verdicts | rule (no shape change beyond LR-8) (exec 171, 172) | A44 writer | A44 writer | one entry per candidate page in candidate order, the first 16 (`TASK_RECOGNITION_CANDIDATE_SCORE_LIMIT`), `omitted` the candidate pages after them; `passed` is the page's own match, so the matched page's entry passed and without a matched page none did; `best_score_milli` is the highest score among the targets evaluated on the page, passed or not: a template's normalised score (`score_milli`, rounded down, 0 to 1000), an OCR read's confidence (`ocr_confidence_milli`) and an NN target's selected score (as a template's); a colour, colour digest, composite or value test target, a target the page recognition skipped and a read without a confidence give none, and a page with none has no `best_score_milli`; `first_failed_target_id`, only on a page that did not pass, is the first target in the page's declared order (`required`, the `any_of` groups, `forbidden`) that ruled it out: a `required` target that did not pass, the first target of an `any_of` group none of whose targets passed (and none skipped), or a `forbidden` target that passed; a skipped target rules nothing out; a candidate page without an evaluation would have neither |
| A44-3 | `task.recognition_completed.candidate_scores` | writer of LR-8 (§1.153) (host 174) | BT-LR | A44 writer | the host writes the kernel's verdicts unchanged into every `task.recognition_completed` it records; the fact's own validation runs on the writer's draft as registered for LR-8 |

### 1.175 Slice FLK07-FIX (0.12.x): a resource-close lease the instance's queue refuses fails only the capture's work and no longer halts the Runtime (#411 A52; FLK-07's cause, board #71; coordinator 10-22 11:1x)

CI run 39708910434 ran FLK-07 (`capture_failure::readonly_failures_are_visible_and_terminal_without_fake_success`)
20 times: 4 rounds latched `lease_transfer_not_safe` (`prepare_resource_close_lease`,
`host_halted=true`). A read-only observe holds the instance's admission guard while it captures,
so a Runtime claim that arrives meanwhile waits in the instance's queue; when the capture failed,
its close asked for a dedicated resource-close lease, the waiting entry refused it (Workflow #369
§5.1) and the close path made the refusal the Runtime's fatal. In production one instance's failing
observes could stop the whole Runtime while another request waited.
[read-session-resource-close.md](read-session-resource-close.md) is the contract. **No persisted
shape**: the refusal is an existing record (`runtime.failed`) of an existing code
(`lease_transfer_not_safe`) at an existing location (`prepare_resource_close_lease`); no code,
key, vocabulary token, configuration key, location or package member. Gates: host keys 171, 172
(173 unused).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| A52-1 | a capture failure whose dedicated close lease is refused `lease_transfer_not_safe` (a read-only observe's or a monitor's; a run closes under its own lease) | rule (no shape change) (host 171) | FLK07-FIX | FLK07-FIX | no close is attempted; the refusal is one `runtime.failed` (stage `runtime_lifecycle_session_close`, the instance, the failed work's links), not fatal; the work fails as its capture failure; the instance is not fenced (Workflow #403 fences an unconfirmed close; nothing is unconfirmed); the session must close and is closed at the waiting entry's lease end or by its preparation (`read-session-resource-close.md`, "Device session lifetime"; `runtime-fact-store.md`, "Connection preparation phase") |
| A52-2 | every other failure of that close | rule (unchanged) (host 172) | FLK07-FIX | — | keeps its fatality: a close attempted under an expired or otherwise invalid lease or an unresolved destructive step retains failure and owner protection, a ledger failure ends the Runtime; an unconfirmed close of the instance's own resources fences it (#403) |

### 1.176 Slice A35-S3 (0.12.x): a wait step without input, and a confirmation's wait page (#411 B36, B19; design `rd-1021/train-0129/a35-design.md` §2.3, §5.1 S3, §7; Alice's B19 decision of 2026-10-20; coordinator decisions Q9 to Q11 of 2026-10-21)

A run that starts on a page showing an activity that ends by itself could only fail or end at once
there (B19 problem 1, B36), and a confirmation waited out its whole timeout on any unexpected page
(B19 problem 2). Now a page-graph operation may be a wait step that makes no input and waits on its
page for its destinations, and a confirmation may declare a wait page that pauses its clock. One
kernel wait serves both, the confirmation's (`await_postcondition`), and keeps A43b's rule
(§1.160). [resource-declarations.md](resource-declarations.md) ("Wait steps and wait pages"),
[README.md](README.md) and [linear-steps.md](linear-steps.md) are the contract. **No persisted
ledger shape** (design §7): no code, no record, no configuration key, no location, no key; the step
writes the existing `StepStarted`/`StepFinished` without effect records, as a pass-over does. The
new members join `package` revision 6, which BT-PK6 declares; until then the Runtime admits them by
code. Gates: exec keys 175 to 179.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| S3-1 | `wait: {"max_ms", "on_timeout"?}` of a task operation | new optional member (resource file format, not a ledger shape) (exec 175 to 177, 179) | A35-S3 | resource side | joins package revision 6; task schema `0.9`, page-graph tasks only (`linear_steps`: `contained_task_linear_invalid`, `reason=wait`); the fourth effect beside `click`, `application` and `select`: no guard, no input, no effect record; frames every `expect_after.interval_ms`, else the capture interval (Q10); a frame on `from` keeps it waiting up to `max_ms` from the step's start on the run's budget clock (Q9) and restarts the away clock (`expect_after.timeout_ms`, else the step timeout), which runs on frames of neither page after the shared interrupt pages; a destination finishes the step and the run goes on; an error page fails `contained_task_error_page_reached`; the away clock fails `contained_task_step_unconfirmed`; `max_ms` on `from` ends the run there Completed with `on_timeout` (a keyed end), else `contained_task_step_unconfirmed` with `page_id` the wait page |
| S3-2 | `expect_after.wait_page: {"page_id", "max_ms"}` | new optional member (resource file format) (exec 178, 179) | A35-S3 | resource side | joins package revision 6; task schema `0.9`, page-graph tasks only (`reason=wait` in `linear_steps`); frames on the page pause the confirmation clock up to `max_ms` of time on it in all, then count as frames of no destination; retry policy, failure decision and extra rounds unchanged |
| S3-3 | `program_problem` tokens `wait_member_refused`, `wait_from_invalid`, `wait_destination_missing`, `wait_max_invalid`, `wait_designated`, `wait_timeout_unmapped`, `wait_timeout_not_busy`, `wait_page_invalid` | new tokens of an existing vocabulary (slice E1, #411 A39: `operations_empty`) on the existing `contained_task_program_invalid` (exec 179) | A35-S3 | A35-S3 | Runtime admission's refusal of a broken wait step or wait page, with `path` (the task declaration) and `json_pointer` (the refused member), checked before the operation's other checks; the source parser refuses a wrong shape (`resource_declaration_invalid`) and an older task schema (`UnconsumedField`) |
| S3-4 | `on_timeout` in the static checks | rule (no shape change) (exec 179) | A35-S3 | A35-S3 | a keyed end under §1.164 KEY-3 (`keyed_terminal_holds`): the phased walk (`task_phases_outcome_incomplete`) and the page-graph coverage (`contained_task_outcome_declaration_incomplete`); its mapping declares `busy` (Q11, `wait_timeout_mapping_busy`); unlike KEY-6 the package stays a prerequisite or return-home layer |
| S3-5 | `TaskPhaseOperation.on_timeout_key`, `wait_timeout_mapping_busy` | Runtime-internal interface (memory only) | A35-S3 | A35-S3 | the walk's view of the new member, shared by source parsing, build and Runtime admission |
| S3-6 | `lab_operation_problem` token `wait` | new token of an existing vocabulary (Lab B-3 S0, §1.159 B3-6) on the existing `contained_task_declared_operation_unsupported` (exec 179) | A35-S3 | A35-S3 | Lab B-3 S1's admission of a declared operation (`PreparedContainedTask::declared_operation`, §1.161 B3S1-2, B3S1-3) refuses a wait step before any lease, with `pack_operation_id` and `lab_operation_problem` `wait` (Lab decision Q2: page-graph clicks only); checked after the package's execution mode, the application effect and the select step; no new code or key |

### 1.177 Slice PERF-B1 (0.12.x): a wait's own capture interval, the configured ceiling of a declared interval, and the back-off on unchanged frames (Workflow #407 B1; coordinator brief 10-24 `rd-1024/brief-perf-b1-capture.md`; Alice 10-24: performance first, #405)

A package that declares no interval captures every 50 ms plus the capture's own time (about every
0.6 s in production), and a package could declare at most 5 000 ms: a 25-minute wait took about
2 700 captures, each with its ledger events and its frame. Now a step's `expect_after.interval_ms`
(its confirmation, or a wait step's wait) may declare a slow interval for that one wait (R1; the
kernel already had the unit, only the bound held it at 5 000 ms), the bound is the configuration's
(R2), and a step's wait for a page lengthens its pause over unchanged frames (R3); a package that
declares nothing keeps its 50 ms (R4). [resource-declarations.md](resource-declarations.md)
("Capture cadence"), [runtime-config.md](runtime-config.md) ("The `execution` section"),
[README.md](README.md) and [linear-steps.md](linear-steps.md) are the contract. **No persisted
ledger shape**: no code, location, value key or vocabulary token; the refusal above the ceiling
reuses the codes admission gives a broken interval, and the effective intervals stay where they
were recorded (`EffectiveTaskTiming.capture_interval`, each operation's `interval`). The new
section adds one section fact of an existing family (B1-2). Gates: the `gate_407_b1` tests (a) to
(d), in rows of exec.toml and host.toml.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B1-1 | control `capture_interval_ms`, `expect_after.interval_ms` and a `linear_steps` transition's `interval_ms` admitted in `1..=1800000` (were `1..=5000`) | rule of an existing member (resource file format, not a ledger shape) (tests (a), (d)) | PERF-B1 | resource side | the bound is the longest wait a step may declare and the range maximum of B1-2; refused above it as before (`contained_task_control_invalid`; `contained_task_linear_invalid`, `reason=transition_interval`); `post_delay_ms` and a scan's `settle.interval_ms` keep their 5 000 ms bound |
| B1-2 | `execution.capture.max_interval_ms` (`5000..=1800000`, 60 000) | configuration key of a new section (§5 row; `config.parameters.execution` row) (tests (d)) | PERF-B1 | PERF-B1 | settable; handed to the kernel with each step's pacing; a run (and an entry check) refuses a package that declares a longer interval before its first capture, with B1-1's codes and `reason=capture_interval_ceiling` in the detail (`transition_interval` for a transition); `config_value_invalid` (`out_of_range`) at load, in check-config and through an instruction |
| B1-3 | `execution.capture.backoff_enabled` (`true`), `execution.capture.idle_ceiling_ms` (`50..=60000`, 5 000) | configuration keys of the same section (tests (b), (c), (d)) | PERF-B1 | PERF-B1 | settable, read at each step; `wrong_type` for a value of another type, `out_of_range` outside the range |
| B1-4 | the back-off of a step's wait for a page (the page wait and the confirmation of the page-graph path, a wait step's wait, the waits of a `linear_steps` step) | rule (no shape change) (tests (b), (c)) | PERF-B1 | PERF-B1 | a frame of the wait unchanged from the one before it by #411 B12's whole-frame measure doubles the next pause up to the idle ceiling or the declared interval, whichever is longer; the first changed frame, a handled shared interrupt page or a failed observation returns it to the declared interval; the pause is cut at the wait's remaining time and the task's deadline; entry checks, observation windows, input rechecks, samples and a scan's settle keep their interval |
| B1-5 | `StepPacing.capture_cadence`, `CaptureCadence` (`execution-kernel`) | Runtime-internal interface (memory only) | PERF-B1 | PERF-B1 | the host hands the section with each step's pacing; a runtime that hands none (offline simulation, fixtures, the Lab's declared operation) gets the default ceiling and no back-off |

### 1.178 Slice ED-1 (0.12.x): the expected duration E, pure, and its configuration keys (#405; SCH-M frozen model v1.0 `rd-1024/sched-design/model.md` §3.4 D-12, D-15, §6 row ED-1; Alice 10-22 16:4x, #405 comment 6335497930; ruling 39 R6.2, R6.3)

Wave 1 groundwork of v35: behaviour-neutral until ED-2 fills the snapshot members.
[scheduling/README.md](scheduling/README.md) ("Settlement Feedback And Eligibility Age") and
[runtime-config.md](runtime-config.md) ("Expected duration") are the contract. **No persisted
ledger shape.** The configuration joins `actingd-config` revision 11. Gates: the `gate_ed1` tests t1 to
t6 (policy) and `ed1_tests` (runtime-config, host).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| ED1-1 | `expected_runtime(samples_newest_first, ExpectedRuntimeSettings {enabled, runs, margin_ms}, declared_ms, max_runtime_ms) -> ExpectedRuntime {expected_ms, source: Declared \| Runs(n)}`; `ExpectedRuntime::snapshot_members` | policy interface (memory only) (tests t1 to t4) | ED-1 | ED-1 | D-12: E = min(⌈mean of the latest min(runs, k)⌉ + margin, M); k = 0 or the switch off: D, no snapshot member |
| ED1-2 | `TaskRuntimeSnapshot.expected_runtime_ms`, `.expected_runtime_runs` | new optional members of the evaluation input (memory and the input identity; absent: today's bytes) (tests t5, t6) | ED-1 | ED-2 (built, §1.190 ED2-4) | the evaluator uses E (at most `max_runtime_ms`) for the cost, the intent's `expected_duration_ms`, forward planning and the production estimate; absent, the declared value (and the cost's `last_duration_ms` fallback) as before |
| ED1-3 | `scheduling.expected_runtime.{enabled, runs, margin_ms}` (`true`, 5 in `1..=16`, 300 000 in `0..=3600000`) and per instance `instance.<id>.scheduling.expected_runtime.*` (default the section's); `scheduling.fact_refresh.expected_duration_ms` (60 000 in `1..=1800000`) | configuration keys (`actingd-config` revision 11) (test c1) | ED-1 | person | all settable; accessor `EffectiveConfig::expected_runtime(instance_id)`; `FactRefreshConfig.expected_duration_ms`; read by ED-2 |

### 1.179 Slice SR-4a1 (0.12.x): the pure blackout and day-boundary judgements, per-instance and `once` blackout entries (#386; SCH-M frozen model v1.0 `rd-1024/sched-design/model.md` §3.2 D-1, D-4, D-6, §6 row SR-4a1; Alice 10-24 12:1x, #386 comment 6379206178)

Wave 1 groundwork of v35: behaviour-neutral until SR-4a2 reads it.
[scheduling-blackout.md](scheduling-blackout.md) ("Configuration blackouts", "Pure judgement",
"Day-boundary fit", "Configuration", "Codes") and [runtime-config.md](runtime-config.md) are the
contract. **No persisted ledger shape.** The configuration joins `actingd-config` revision 11.
Gates: the `gate_sr4a1` tests t1 to t4 (policy.toml) and the `sr4a1_tests` c1 and c2 (host.toml).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| SR4A1-1 | `blackout_at`, `next_blackout_start` over `BlackoutSources {host, instances, catalog}`; `Blackout {id, source, start_unix_ms, end_unix_ms}`; `BlackoutEntry {id, window}`, `BlackoutWindow::{Weekly, Once}` | policy interface (memory only) (policy tests t1 to t4) | SR-4a1 | SR-4a1 | the union of overlapping or adjacent intervals of the three sources; its end the union's end; the reported entry covers the time and ends last; catalog `maintenance` events with V2 validity and the instance's scope; no reader yet (SR-4a2) |
| SR4A1-2 | `day_boundary_fit(profile, now, required_ms) -> Option<DayBoundaryDeferral {window_end_unix_ms, required_ms, next_eligible_unix_ms}>` | policy interface (memory only) | SR-4a1 | SR-4a1 | SCH-M D-6; the wake is the profile's next window start; its code `policy_day_boundary_approaching` and its tests are SR-4a2's |
| SR4A1-3 | blackout entry `kind` (`weekly`, `once`); at most 64 per list; `instance.<id>.scheduling.blackout` (list, no default, settable) | configuration keys (`actingd-config` revision 11) (host test c2) | SR-4a1 | person / deploy command | an entry without `kind` is `weekly` (the host shape); `once` `[from_unix_ms, until_unix_ms)` non-empty; ids unique per list; pointer `/instances/<i>/scheduling/blackout/<n>/…` for an instance entry; accessor `EffectiveConfig::instance_blackouts` |
| SR4A1-4 | `scheduling.blackout.quiet_horizon` (`deadline` \| `expected`, default `deadline`); `scheduling.day_boundary.quiet` (boolean, default `true`) and per instance `instance.<id>.scheduling.day_boundary.quiet` (default the section's) | configuration keys (`actingd-config` revision 11) (host test c1) | SR-4a1 | person | all settable; read by SR-4a2; SR-4b changes the default of `quiet_horizon` to `expected`; accessor `EffectiveConfig::day_boundary_quiet` |
| SR4A1-5 | key `blackout_source` (`vocab:blackout_source`); vocabulary `blackout_source` (`host`, `instance`, `catalog`; contract crate `BlackoutSource`) | new key and vocabulary (host test c2) | SR-4a1 | SR-4a1 | `blackout_config_invalid` gains `blackout_source` (required; first written by SR-4a1); `blackout_active` and `blackout_approaching` gain it (required; first written by SR-4a2) |

### 1.180 Slice CF-1 (0.12.x): the edge members `since_own_run` and `once_per_reading`: a chain step runs once per new result of its predecessor, a completion time is served once (SCH-M frozen model v1.0 `rd-1024/sched-design/model.md` §3.3 D-8, §6 row CF-1; #405, Alice 10-22, comment 6335384240: condition-first, not a timer)

CF-1: two optional V2 catalog members turn a level into an edge, so condition-first catalogs (v34)
need no pass, round or slot reset events to make a successor or a completion time fire again.
[scheduling/README.md](scheduling/README.md) ("Condition edges", "Why A Candidate Does Not Run")
and [scheduling/v2/README.md](scheduling/v2/README.md) are the contract. **No persisted ledger
shape**: the members are a catalog format addition (the ledger holds only the catalog's
`catalog_hash`, unchanged for a catalog without them); the reasons are catalog codes carried by the
evaluation's decisions and the existing dispatch decision records; the subsystem row is a value of
the existing `config.subsystems` fact. No wire, configuration key or interface revision changes. A
Runtime before CF-1 refuses a catalog that writes either member (`unknown_field`), so the generator
emits them only when check-config names `condition_edges`.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| CF1-1 | `dependency_completed.since_own_run` (optional boolean; `PredicateSpec::DependencyCompleted`, `contracts/scheduling/v2/common.schema.json`) | catalog format addition (no ledger shape) | CF-1 | GEN-34 | V2 only (a V1 document writing it, either value: `unknown_field` at the member); `true`: true when the predecessor's latest terminal is accepted and this task has no admitted dispatch on the instance, or its latest dispatch is before the predecessor's latest dispatch, or its latest run did not succeed; otherwise false with `dependency_already_served`, no wake; absent or `false`: the level as before; absent serializes as nothing |
| CF1-2 | `fact_time_reached.once_per_reading` (optional boolean; `PredicateSpec::FactTimeReached`) | catalog format addition (no ledger shape) | CF-1 | GEN-34 | V2 only with its predicate; `true`: a time T is served when this task's latest run succeeded and was dispatched at or after max(T + `offset_ms`, the fact's `observed_at_unix_ms`), then false with `fact_time_served`, no wake, until a reading observed after that dispatch; absent or `false`: §1.134 W1-2 as before; absent serializes as nothing |
| CF1-3 | codes `dependency_already_served` (info; `task_id`, `predecessor_task_id`), `fact_time_served` (info; `fact_key`, `served_by_dispatch_unix_ms`) | new catalog entries (no shape) | CF-1 | CF-1 | owner policy (registry `PolicyCode`, block `SCH-M CF-1`), `ladder_class` `none` (row 7: a scheduling decision reason, not how work on an instance ended), layer `policy`, review `pending`; the prefixes `dependency_` and `fact_` already cover policy |
| CF1-4 | keys `predecessor_task_id` (name), `served_by_dispatch_unix_ms` (unix_ms) | new key-table entries (no shape) | CF-1 | CF-1 | — |
| CF1-5 | `config.subsystems` row `condition_edges` (always on, `config_subsystem_always_on`) | new row value of an existing fact (no shape) | CF-1 | CF-1 | [actingd-check-config.md](actingd-check-config.md); the generator's capability check for both members |

### 1.181 Slice CF-2 (0.12.x): a reading mark publishes a completion time, its frame time plus the countdown it shows (SCH-M frozen model v1.0 `rd-1024/sched-design/model.md` §3.3 D-9, §6 row CF-2; #405)

CF-2: a package can publish the time some work completes, `*_ready_at`, read from the screen as a
countdown, so the catalog predicate `fact_time_reached` (§1.134) has a time to wake at.
[resource-readings.md](resource-readings.md) ("Completion times", "Version gate", "Kernel
reading", "Host") and [ocr-fields.md](ocr-fields.md) are the contract. **No persisted ledger
shape**: the record is the existing `fact.published` with an inline `timestamp_ms` content, a
value type the fact record already holds; no code, key, location or configuration key is added
(declaration refusals reuse `resource_declaration_invalid`, and a frame disagreement the reading
problem `no_agreement`). The value joins `package` revision 6, which BT-PK6 declares; until then
it is admitted by code. The subsystem row is a value of the existing `config.subsystems` fact.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| CF2-1 | `resource_readings[].value` `{"type": "timestamp_ms", "format": "clock_hms", "base": "observation"}` (`ResourceReadingType::TimestampMs`, `ResourceReadingTimeBase`) | new value type (resource file format, not a ledger shape) | CF-2 | CF-P (resource side) | `format` and `base` required; another format `InvalidValue` @ `…/value/format`, another base `InvalidValue` @ `…/value/base`; `min`, `max`, `dictionary` `UnknownField` at the member (and `base` on `unsigned_integer`); `capacity_fact_key` refused as for every non-fraction mark; the shared `validate` refuses another format with `resource_declaration_invalid`; a Runtime before CF-2 refuses the value (`UnknownField` @ `…/value/base`), never silently |
| CF2-2 | the record of a completion-time reading | existing `fact.published`, inline `timestamp_ms` content (no shape change) | CF-2 | CF-2 | `observed_at_unix_ms` + countdown seconds × 1000, the frame's capture time as `observed_at_unix_ms`, `expires_at_unix_ms` = `observed_at_unix_ms` + `valid_for_ms`, every other field as for a reading; the kernel trace carries the countdown in seconds |
| CF2-3 | a completion-time mark with an agreement | rule (no shape) | CF-2 | CF-2 | holds when each frame gives its reading and the instants (frame capture time + countdown) lie within (`frames` − 1) × `interval_ms` + 1000 ms, latest minus earliest; else `no_agreement`; the reading is the handed-off frame's |
| CF2-4 | `config.subsystems` row `resource_reading_time` (always on, `config_subsystem_always_on`) | new row value of an existing fact (no shape) | CF-2 | CF-2 | [actingd-check-config.md](actingd-check-config.md); packs that declare the value are placed only on a Runtime that names it |
| CF2-5 | a `resource_adjustments` entry naming a completion-time mark's key | rule (no shape) | CF-2 | CF-2 | coordinator ruling of 2026-10-24: refused at declaration, with or without `amount`, `InvalidValue` @ `/resource_adjustments/<i>`; the shared `validate_resource_adjustments` gives the existing `resource_declaration_invalid` (no new code) |

### 1.182 Slice LN-3 (0.12.x): the value records of the decision memory, the wake obligations and the monitor registry (#422 LEAN-L; frozen model v1.0 `rd-1024/lean-ledger/model.md` §1.2 H-c, H-e, §3.6, §4, §5.1 row LN-3; Alice 10-24 14:0x, #422: the start loads only tagged content such as the scheduler workbench)

LN-3 is the first writer of L7, L8 and L14, which the register gave to AO-4; LEAN-L §4 takes
them onto the #422 path, written through the existing append path (AO-2's capture queue and
flush) with no gate machinery. The only new shape is one `state` key rule of L11,
`monitor/<alias>`. Loading from these records at start is LN-4's: until then the start still
replays, and the records are written only.
[ledger-observer.md §11.1 and §11.3](ledger-observer.md#113-value-records-the-latest-record-is-the-current-value),
"As built by LN-3", are the contract. No code, location, value key, vocabulary token or
configuration key is added. Gates: the `gate_422_ln3` tests, `monitor::tests::ln3_a_…` and
`policy_control::tests::ln3_b_…`, one row of host.toml. The number 1.182 is provisional for the
parallel slices.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| LN3-1 | `policy.control_state_recorded` (L7) | first writer of a registered type | LN-3 | LN-3 | one record per change of a dispatch pair's state, after the admission's outcome, after the settlement's completion, and after the start's and the online settlement reconciliations, under the policy guard; a change is a difference between the pair's complete state and its latest record, so a replayed admission or a repeated settlement writes none. `daily_counts`: the newest 32 days; `active_window`: the window of the pair's latest admission, with `window_counts` and `window_runtime_ms` in it; `dispatch_clock`: the admission time of the pair's latest admitted dispatch until its completion; `condition_reruns`: absent until SR-2b keeps a rerun state. Origin `scheduler` / `policy` / `scheduler`, system links, info. At the open the replayed state counts as recorded; seeding one record per key is LN-4's |
| LN3-2 | `policy.activity_state_recorded` (L14) | first writer | LN-3 | LN-3 | the same occasions and rule; `local_day` and `window_id` are those of the latest admission of the profile on the instance |
| LN3-3 | `agent.wake_obligation_recorded` (L8) | first writer | LN-3 | LN-3 | right after each wake's `agent.wake_requested`, the same `AgentWakeData`; origin `scheduler` / `agent_dispatcher` / `runtime`, system links. Never replayed: the agent dispatcher's whole-ledger fold now takes only the replayed types, as its paged read does |
| LN3-4 | `ledger_index` `state` key `monitor/<alias>` (L11) | new key rule | LN-3 | LN-3 | the monitor registry's records already carry an alias's complete state (`runtime_state` `monitor_changed`: status, revision, configuration version; `monitor_imported`: every instance it brings). The key names the alias's latest applied change, or the import for each alias it brings; a change that was not applied is not tagged. Tagged records commit on arrival (§11.4). An import of no instance tags nothing |
| LN3-5 | `PolicyControlState::value_records`, `apply_value_record`; `PolicyHost::changed_value_records` | Runtime-internal interface (memory only) | LN-3 | — | the reducer is test-only until LN-4 wires the start load. The records carry no settlement order, so Workflow #372 K5's `failure_settled` (whose failure state a continued id reads) and a retry round's `opened` cannot be rebuilt from them (LN-4) |

### 1.183 Slice LN-2 (0.12.x): the instance fact store loads from the tagged `state` rows; the start's whole-ledger replay goes (#422 LEAN-L; frozen model v1.0 `rd-1024/lean-ledger/model.md` §1.2 H-a, H-g, H-j, §3.1, §3.2 step 5, §5.1 row LN-2, §7 A3 and A6; Alice 10-24 14:0x, #422: 「账本仅向内存加载带有特定标签的内容，不要全量加载」)

LN-2 is the first reader of L11's `state` rows (§11.5 steps 1–4) for the instance store.
[ledger-observer.md §11.5](ledger-observer.md#115-start-loading-ao-6), "As built by LN-2", is the
contract. No code, location, value key, vocabulary token or configuration key is added: the load
reuses the registered `ledger_tag_index_mismatch` (fatal, §16) and the location
`ledger_index_rows`. Gates: the `gate_422_ln2` tests, one row of host.toml. The number 1.183 is
provisional for the parallel slices.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| LN2-1 | `GlobalLedger::tagged_state(prefixes)` → `LedgerTaggedState {through_sequence, records: [LedgerTaggedRecord {key, event}]}`; `instance_fact_state_key(scope, key)` | ledger interface (read only) | LN-2 | — | the `state` rows whose key starts with a prefix, each record read by the row's sequence (a point read of the store through the sequence key, the record authenticated on its own: LN-5's history read; no other record is read), sorted by (sequence, key), at the writer's head. Each row is checked: a record has that sequence and the row's event id, carries the row's key (under any workbench pattern) and its stored `record_sha256` is the row's `value_sha256`; otherwise `ledger_tag_index_mismatch` with `fact_key` (the row key as a token: `/` → `:`), `event_id`, `expected_sha256` (the index's) and `actual_sha256` (the record's) when they fit the key table, the raw row in the detail. Its writer command kind is `Query`, never a whole-ledger query; the read count is the records returned. A store without an index (the test-only segment store) answers `ledger_joint_transaction_unsupported` |
| LN2-2 | `InstanceFactStore::recover` | Runtime-internal (memory only) | LN-2 | — | loads from `instance_facts/` and `input_boundary/`: a publication loads only the records its rows name; for each loaded publication the first later event of a type in its `invalidate_on` whose scope matches (an input type only when a scope instance's boundary row of that type lies after the publication, read by type and instances; any other type by its type index) is applied as replay would apply it, so a fact whose invalidation was not written yet is the same pending invalidation; records and triggers go through today's reducer in ledger order; the position is the writer's head. A key whose latest record is an invalidation has its tombstone and `latest_observed` = its invalidation time (§11.3). The whole-ledger replay, the start's `HistoricalFactProjection` pass and `RuntimeStateStore::recover_fact_projections` (its only caller was the start; a `fact.invalidated` and its tombstone commit in one transaction and are never dumped) are gone. `HistoricalFactProjection` stays for the on-demand history reads (`at_position`, `active_records_at`). Other read failures stop the start as `fact_store_recovery_failed` with the ledger's failure as the detail |
| LN2-3 | `ApprovalRecordsAt` | Runtime-internal (memory only) | LN-2 | — | the resident read projection keeps the latest record only of the approvals in `active` and `recent` (at most 512); a later decision of a dropped id is folded without the target check against its earlier record, which the governance path's own full projection keeps |
| LN2-4 | `VerifiedArtifactIndex` removed | Runtime-internal (memory only) | LN-2 | — | `artifact.verified` is read on demand, by its type index in pages through the ledger's latest position at the call; the three callers read the same references in ledger order |

### 1.184 Slice LN-4 (0.12.x): the decision memory, the agent dispatcher and the monitor registry load from their tagged records; seeding once plus the marker (#422 LEAN-L; frozen model v1.0 `rd-1024/lean-ledger/model.md` §1.2 H-c, H-e, H-i, §3.2 step 5, §3.6, §5.1 row LN-4; Alice 10-24 14:0x, #422)

LN-4 is the first loader of L7, L8 and L14 and the first writer of L6 (`tagged_state_enabled`).
[ledger-observer.md §11.1, §11.3 and §11.5](ledger-observer.md#115-start-loading-ao-6), "As built
by LN-4", are the contract. No code, location, vocabulary token or configuration key is added;
the load and the seeding use the registered `ledger_tag_index_mismatch` and
`ledger_tagged_state_seeded` (L6's report, value `count`). Gates: `gate_422_ln4`,
`agent_sessions::ln4_d_…`, one row of host.toml. The number 1.184 is provisional for the
parallel slices.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| LN4-1 | `ledger_index` `state` key `monitor_import`; `wake/<wake id>` also for the five session record types; `lifecycle` key `tagged_state_enabled` (L11) | new key rules | LN-4 | LN-4 | `monitor_import` settles LN3-4's import of no instance; a `wake/` row naming a completed session is an ended obligation, one naming an escalated session keeps it for the person; the marker row is one read at start |
| LN4-2 | L7 `failure_settled?` (u64 ≥ 1); L7 `retry_round.opened?` (u64 ≥ 1) | new optional members of a registered type | LN-4 | LN-4 | settles LN3-5 (register L7 as written carries no settlement order): the Workflow #372 K5 order of the pair's latest settlement that set or cleared its failure state, and of the round's opening; absent in LN-3's records, which the seeding replaces; the smallest addition, since neither order can be rebuilt from L7's other members (a success leaves no failure record, and two rounds of one group may open in one millisecond) |
| LN4-3 | L6 `runtime.lifecycle_observed` phase `tagged_state_enabled` | first writer of a registered phase | LN-4 | LN-4 | written once by a start without the marker, after the value records it seeds; its report `ledger_tagged_state_seeded` `{count}` is the keys seeded. A root's first start (no `runtime.started` or `runtime.takeover` before it, the `lifecycle` row `last_start`) has no earlier epoch to seed and writes no marker; the root's next start seeds what that epoch recorded |
| LN4-4 | `GlobalLedger::index_records(role, prefixes)` | Runtime-internal interface | LN-4 | — | one writer command: the rows of `state`, `open` or `lifecycle` with a key prefix, a point read of each row's record, the §11.5 step 3 checks; a store that keeps no index answers `ledger_selection_query_unsupported`; reads count as `Query` in the writer's peak |
| LN4-5 | the agent dispatcher prunes an ended session | Runtime-internal behaviour | LN-4 | — | §11.2 "sessions are not kept": at a session's end its request records leave memory; a completed session's wake, trigger and session leave too, an escalated one keeps its status and wake for the person; a resent response of an ended session is answered from the ledger; the wake reconciliation reads only the previous epoch's planning signals and wakes |
| LN4-6 | `refresh_dispatches` | Runtime-internal behaviour | LN-4 | — | rebuilds the decision memory from the latest value records and reads only the dispatch records after the records' position (each dispatch change writes its pair's record under its guard, so a dispatch with a later record is open or its pair's latest completed one, which the journal keeps); before the seeding wrote the records it replays as before |
| LN4-7 | L7 `dispatches[]?` of `{intent_sequence, admitted_sequence?, completed_sequence?, failures?}` (≤ 128 entries, strictly increasing intents; `admitted` > `intent`; `completed` > `admitted` only with `failures`) | new optional member of a registered type (coordinator ruling 3, 10-26) | LN-4 | LN-4 | the smallest member that lets a start find a pair's latest completed dispatch by indexed reads: the sequences of the dispatches the journal keeps of the pair, which the start point-reads (the execution is read among the executions between the admission and the completion). It names the open dispatches, the latest completed one and, for a task of the remedy journal (one that declares a remedy or provides a capability), its latest runs (at most 64). `failures` is the pair's consecutive failed runs through that dispatch, the per-pair consecutive-failure count of ruling 2, kept at each completion. Empty while the pair has none; absent in earlier records (no old-version compatibility, Alice 10-11) |
| LN4-8 | the policy host's dispatch journal | Runtime-internal behaviour (coordinator rulings 1 and 2, 10-26) | LN-4 | — | bounded, never by the event count: per (task, instance) the open dispatches, the latest intent and the latest 64 admitted dispatches (the latest completed one and the remedy journal's runs). A pair's settlement reads its latest executed run and the count its completion keeps, no fold over all runs. Decision pages, the task runtime projection at a position the journal does not answer, and a replayed admission of a decision it no longer holds read the dispatch history on demand, page by page (LN-5's SQLite reads under the read budget) |

Deviations, as the coordinator ruled them (10-26 ruling 4): a session escalated to a person keeps
its status and its wake until it completes (LN4-5), and the wake reconciliation looks back one
epoch only (LN4-5). Two more of LN4-8: a Workflow #372 K5 group's failure streak is the count of
the group's latest run's own pair (a continued id's trailing failures before the rename are not
added), and a resent execution or completion of a dispatch the journal no longer holds (older
than the pair's latest 64 runs) is refused `policy_dispatch_unknown` instead of being replayed.

### 1.185 Slice RL-1b (0.12.x): the scheduler workbench in the host: load when used, unload when not, no approval (#420 ROUT-L; frozen model v1.0 `rd-1024/rout-lean/model.md` §3.3–§3.6, §3.8, §3.9, §6.1 row RL-1b, §7.1 A2–A6, A8, A9; Alice's ruling 51, #386 6380164487: 「直接动态加载，用到的时候再加载，不用了直接从内存卸载，这个应该是要放到调度器工作台的」)

[standard-pack-schedule.md](standard-pack-schedule.md), "The scheduler workbench (RL-1b)", is the
contract. Pending decision 58 takes its recommended answer A (a pause does not unload a game's
rules). One code is added (`policy_pool_reading_unused`), four optional values of
`standard_pack_registered`, two configuration keys and one subsystem; `actingd-config` revision
12, shared with IDLE-1. Gates: the `gate_rl1b` tests, eight rows of host.toml. The number 1.185
is provisional for the parallel slices.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| RL1B-1 | a `policy` section without `catalog`, `catalog_approval_ids` and `procedure_manifest` (composer mode) | configuration (revision 12) | RL-1b | the person | requires `standard_packs.root` (`config_value_invalid`, `config_key` `standard_packs.root`); approval ids, a manifest or a `catalog_transition` without `catalog` are refused; with `catalog` the configured path is unchanged for this release |
| RL1B-2 | `standard_packs.check_interval_ms` (1000..=3600000, default 60000), `standard_packs.cached_generations` (0..=8, default 1), both settable | configuration keys (revision 12) | RL-1b | the person | read at each check; `cached_generations` replaces the memo's fixed bound of 8 once the workbench runs |
| RL1B-3 | a composed generation directory: the four documents, `fragments/<game>.json`, `standard-packs.json` (`actingcommand.standard-pack-composition.v1`: `rules_digest`, `games`, `procedures`, `pools`), `manifest.json` | Runtime state (immutable, by content hash) | RL-1b | RL-1b | activated with the existing `catalog.transition_intent` / `catalog.activated`, no promotion, no approval decision; read back with the 4 MiB catalog bound |
| RL1B-4 | `standard_pack_registered` values `schedule_sha256`, `rules`, `held`, `unclassified` | outcome values (optional) | RL-1b | RL-1b | one record per changed `schedule.json` of a game in use |
| RL1B-5 | `policy_pool_reading_unused` (warning, `pool_id`) | outcome code | RL-1b | RL-1b | a configured static pool reading for a pool no composed game declares is left out of the evaluation; recorded once per pool (RL-1a review item 1) |
| RL1B-6 | `config.subsystems` row `standard_pack_schedule` (always on) | new row value of an existing fact | RL-1b | RL-1b | a `schedule.json` that requires it is composed only by a Runtime that names it |
| RL1B-7 | check-config `report.standard_pack_schedule` (`composer_mode`, `games`, `rules`, `pools`, `profiles`, `events`, `unclassified`, `problems`, `left_out`) | check-config report member | RL-1b | — | the composition the workbench would make for the configured instances, in either mode; each problem is a note |

### 1.186 Slice LN-7 (0.12.x): retention without resident history: the facts of the live frames only, read by type and by index (#422 LEAN-L; frozen model v1.0 `rd-1024/lean-ledger/model.md` §3.5, §5.1 row LN-7, §7 A6; Alice 10-24 14:0x, #422 R1 and P2: nothing resident grows with the history)

[ledger-store.md](ledger-store.md#open-ownership-and-append-durability), the LN-7 paragraphs of
"Open, ownership and append durability", "Prior-epoch scope close" and "Frame cleaner", is the
contract. No code, location, value key, vocabulary token or configuration key is added: a listing
failure of the frame store reuses `ledger_io` (fatal at the open, a request error at a sweep
start), an old artifact's other reference reuses `artifact_identity_conflict`. Part 1 (#1338): the
prior-epoch closes come from the previous epoch only and `owner_epochs` keeps owner changes only.
Gates: the `gate_ln7` tests of the ledger and of the frame cleaner. The number 1.186 is
provisional for the parallel slices.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| LN7-1 | `GlobalLedger::frame_retention_view_live(now_unix_ms, switches, root)` | ledger interface (read) | LN-7 | — | lists `root`'s live frame store on the caller's thread, then the writer keeps only the facts of the frames present there and answers the frame view of `frame_retention_view`; its writer command kind stays `RetentionCandidates` |
| LN7-2 | the writer's resident retention state | Runtime-internal behaviour | LN-7 | — | the eviction proof set; the owner changes, ends, imports and closes; the scopes of the latest owner, the one before it and the owners whose import or close they wrote; the facts of the live frames and what their classes read (every frame of their runs, the open runs, the runs with a frame at most 30 s after a live frame, the marker neighbours, the error points whose window can hold a live frame, the Lab scopes). Kept frames that left the store are classed beside the live ones and not listed. A record naming a run or a capture frame the writer no longer holds loads it by the run link, the `artifact_id` index and the frame link before it is checked; `classify` and `apply` are fed the records unchanged, in ledger order |
| LN7-3 | the open's retention read | Runtime-internal behaviour | LN-7 | — | by type (owner records, starts, bindings, eviction intents and outcomes, `Performed` releases of the retained owners' epochs), by index (the live and proven frames' records and what they name) and the retained owners' scopes grouped in SQL over the link index; no `validate` replay of every record. The owner records scale with their types (lifecycle observations, capacity samples, runtime-state facts) |
| LN7-4 | `actingcommand_artifact_store::list_live_frames(root)` | artifact store interface (read only) | LN-7 | — | the canonical artifact ids of `artifacts\<shard>\artifact_<hex>.png` under the state root; no file contents, never `kept\`; a missing `artifacts\` or a shard that vanishes lists nothing |
| LN7-5 | the frame cleaner's sweep start | Runtime-internal behaviour | LN-7 | — | reads `frame_retention_view_live`; `handled` and `failed` keep only the ids its view lists, so they are bounded by the frames on disk; `absent` and `rescanned_absent` count only frames that left the store after the listing |
| LN7-6 | the non-recovery branch of `retention_candidates` and eviction admission | Runtime-internal behaviour | LN-7 | — | lists and judges the resident objects: every admissible object has its material present, so it is live and resident; the host reads only `recovery_pending` outside recovery (startup recovery is the only caller) |

### 1.187 Slice LN-8 (0.12.x): the start's ledger open reads the meta row, `MAX(sequence)`, the newest records and the indexes only; `quick_check` leaves the start (#422 LEAN-L; frozen model v1.0 `rd-1024/lean-ledger/model.md` §3.2 steps 1 and 2, §3.7, §6, §5.1 row LN-8, §7.1 A1 to A3; Alice's rulings 56 and 57)

[ledger-store.md](ledger-store.md#open-ownership-and-append-durability), the LN-8 paragraph of
"Open, ownership and append durability", [ledger-observer.md §15](ledger-observer.md#15-configuration-keys-ledger_observer)
(the LN-8 note) and [runtime-database.md](runtime-database.md) are the contract. No code or
location is added: damage in the open's tail reuses the integrity codes a complete read gives for
it. `ledger-maintenance verify` keeps its complete read and the whole-file check; old rows that no
reader reads are re-checked by LN-9 (the same release). Gates: the `gate_ln8` tests of the
ledger and the scale harness (`Lean scale`), which gates model §7.1 A1 to A3 from this slice on.
The number 1.187 is provisional for the parallel slices.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| LN8-1 | `ledger_observer.open_verify_tail_records` | configuration key (`actingd-config` revision 13) | LN-8 | — | how many of the newest records the start's ledger open reads and authenticates; 4 096, `1..=1000000`, not settable (read when the ledger opens); `LedgerFailurePlan::with_open_verify_tail_records`, `GlobalLedgerConfig::with_open_verify_tail_records` |
| LN8-2 | `ledger_opened`, `ledger_open_measured`: the optional values `meta_ms`, `tail_ms`, `index_ms`, `import_ms` | outcome value keys (duration) | LN-8 | — | the start's open phases: format, marker, meta row and `MAX(sequence)`; the tail; the missing views and indexes with the retention read by type and by index; the dump import, tail gap and stale-owner record. `event_count` is the tail records authenticated; `sql_read_ms`, `verify_ms`, `restore_ms` stay the phases of `ledger-maintenance verify` and are 0 for the start; `ledger_open_slow`'s `elapsed_ms` sums all eight |
| LN8-3 | the formal writer open's verification | Runtime-internal behaviour | LN-8 | — | one read transaction: format and marker, keyed meta row, `MAX(sequence)` = head, then the newest rows in chunks, each authenticated on its own with its link and artifact rows, contiguous through the head, the oldest chained to its predecessor's stored hash and the newest's hash the meta row's; no other row and no migrated-prefix digest; the views a database lacks are created against the meta row and `MAX(sequence)`, without a second read |
| LN8-4 | `RuntimeDatabase::open_existing_for_start(root)` | database interface | LN-8 | — | the Runtime start's open of existing storage: read-write with every check of `open_existing(root, false)` except the whole-file `quick_check(1)` |

### 1.188 Slice CF-4 (0.12.x): a `fact` predicate compares with another fact of its scope plus an offset (#386, condition-first; Alice 10-26: 「上限-1000才可能当做红线，并且这个红线绝对不是固定的」)

[scheduling/README.md](scheduling/README.md) ("Fact references", "Why A Candidate Does Not
Run"), [scheduling/v2/README.md](scheduling/v2/README.md) and
[standard-pack-schedule.md](standard-pack-schedule.md) (`requires`) are the contract. **No
persisted ledger shape**: the reference is a catalog format addition (the ledger holds only the
catalog's `catalog_hash`, unchanged for a catalog without one); the reason is a catalog code
carried by the evaluation's decisions and the existing dispatch decision records; the subsystem
row is a value of the existing `config.subsystems` fact. No wire, configuration key or interface
revision changes. The number 1.188 is provisional for the parallel slices.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| CF4-1 | `fact.value` may be `{"type": "fact", "fact_key", "offset"}` (`FactComparand::Fact`, `FactReference`; schema `factReference` in `contracts/scheduling/v2/common.schema.json`) | catalog format addition (no ledger shape) | CF-4 | the standard packs' `schedule.json` | V2 only (V1: `unknown_field` at `value/type`); the referenced fact is read in the predicate's scope with its `max_age_ms`; both facts integers, compared as `a C b + offset`; `contains` refused; a typed value serializes and hashes as before |
| CF4-2 | code `fact_reference_unavailable` (info; `fact_key`, `unknown_reason`) | new catalog entry (no shape) | CF-4 | CF-4 | owner policy (registry `PolicyCode`, block `#386 CF-4`), `ladder_class` `none` (row 7: a scheduling decision reason), layer `policy`, review `pending`, since 0.12.14; the existing vocabulary `unknown_reason` (`fact_missing`, `fact_low_confidence`, `fact_stale`, `fact_expired`, `type_mismatch`); no new key |
| CF4-3 | `config.subsystems` row `fact_reference` (always on, `config_subsystem_always_on`) | new row value of an existing fact (no shape) | CF-4 | CF-4 | [actingd-check-config.md](actingd-check-config.md); a `schedule.json` whose rule writes a reference must list it in `requires`, else the composer refuses the file (`schedule_invalid` at the reference) |

### 1.189 Slice SR-4a3 (0.12.x): complete silence at the ladder, the startup package, the emulator autostart and the monitor probe (#386; SCH-M frozen model v1.0 `rd-1024/sched-design/model.md` §3.2 D-3, §6 row SR-4a3; frozen scheduling model v1.4.2 §5.2a, §5.3 rule 3, C-3; Alice 10-09: 「黑名单就是完全静默，连接断开了也不管」)

[scheduling-blackout.md](scheduling-blackout.md), "Complete silence" ("How the Runtime holds
them"), is the contract. No code, configuration key, subsystem or persisted shape is added: the
ladder suppression `blackout` (register S3) is first written here, `blackout_config_invalid` gains
its first host writer with `blackout_source` `catalog`, and the scheduler's routine gate gains a
value in memory. With both blackout lists empty and no catalog `maintenance` event nothing
behaves differently. Gates: the `gate_386_sr4a3` tests, one row of host.toml. The number 1.189
is provisional for the parallel slices.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| SR4A3-1 | `ClaimGate::Silenced {routine_held, autostart}`; `ClaimGate::{routine_held, blackout, holds, silenced}`; `SeedScheduler::waiting_total_gated` | scheduler interface (memory only) | SR-4a3 | SR-4a3 | holds `startup_package` (a blackout covers the instance, or the next starts within the package's deadline plus `quiet_margin_ms`) and `autostart` (a blackout covers it); every client and operator kind passes |
| SR4A3-2 | `recovery_ladder_suppressed` reason `blackout`, `until_unix_ms` the blackout's end (Info) | persisted enum value (registered by AO-0, S3) | SR-4a3 | SR-4a3 | at admission (every trigger stage), at a granted claim's climb, for a queued ladder claim at the lease sweep, before a rung or a hold's rerun its time limit would carry into a blackout |
| SR4A3-3 | `blackout_config_invalid` with `blackout_source` `catalog`, `field` `/events/<n>/validity`, `blackout_id` the event's id | outcome values (existing) | SR-4a3 | SR-4a3 | a catalog `maintenance` event with a duration and no V2 validity, once per active generation; the event is left out of the host's blackouts |

### 1.190 Slice ED-2 (0.12.x): the run-duration samples and the expected duration E in force (#386; SCH-M frozen model v1.0 `rd-1024/sched-design/model.md` §3.4 D-13, D-14, D-15, §6 row ED-2, and the coordinator's ruling of 10-24 14:1x after the freeze; Alice 10-22 16:4x, #405 comment 6335497930: 「每个任务执行所需的时间可以动态的从账本读，近5次该任务包的平均时间加5分钟」; ruling 39 R6.2, R6.3)

E takes effect when this slice deploys; `scheduling.expected_runtime.enabled` false (section or per
instance) rolls it back without a restart. [instance-fact-store.md](instance-fact-store.md) ("Run
duration facts", the capacity ledger), [scheduling/README.md](scheduling/README.md) ("Settlement
Feedback And Eligibility Age"), [runtime-config.md](runtime-config.md) ("Expected duration") and
[actingd-check-config.md](actingd-check-config.md) are the contract. The ruling after the freeze
overrides the model's "dispatch intent shape unchanged" (§5): the intent records the duration it
reserved (ED2-3). **No configuration key** (ED-1's keys are read now). Codes: the model's two,
`run_duration_key_reserved` and `run_duration_row_dropped` (#420 S4: one domain warning with
values covers the row bound, an unreadable row and a refused publication). Gates: the `gate_ed2`
tests t1 to t9 (host). The number 1.190 is provisional for the parallel slices (1.189 is SR-4a3's).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| ED2-1 | `health.run_duration` (one per instance): `record_list` rows `{package_id, n, d1 … dn, at}` | instance fact key (tagged, as every instance key) (tests t1 to t5, t7) | ED-2 | ED-2 (the settlement of a successful scheduled run) | newest duration `d1`, at most `runs` (≤ 16) per row, ≤ 256 rows (the oldest `at` dropped with `run_duration_row_dropped`); source `runtime.run-duration`, no expiry; loaded by LN-2's tagged start; +1 active fact per instance in the capacity ledger; kept by package updates and upgrades (R6.2) |
| ED2-2 | `run_duration_key_reserved` (error; `fact_key`), `run_duration_row_dropped` (warning; `instance_id`, `package_id`, the refusal as `caused_by`); locations `record_run_duration_sample`, `project_expected_runtime` | outcome codes and locations (tests t6, t7) | ED-2 | ED-2 | a client's publication of the key, or any other path but the settlement, is refused before the fact write gate with nothing appended; the warning is one `runtime.failed` with system links |
| ED2-3 | `policy.dispatch_intent`: `expected_duration_ms` (milliseconds, not 0) | new optional member of a persisted record (tests t4, t9) | ED-2 | ED-2 | written only when the intent's duration is not its task spec's declared `expected_duration_ms` (E, or a configured fact refresh duration); only an intent carries it (`invalid_policy_payload`, field `expected_duration_ms`); recovery and replay reserve it (`control_intent`), absent the spec's declared one; `PolicyPayloadDraft::dispatch_intent_with(data, random_delay, expected_duration_ms, audit)`, `PolicyDispatchPayload::expected_duration_ms()` |
| ED2-4 | the projection of E into `TaskRuntimeSnapshot.expected_runtime_ms`, `.expected_runtime_runs` (ED1-2) | Runtime-internal behaviour (memory and the input identity) (tests t1, t3, t4, t8, t9) | ED-2 | — | every authoritative projection; a pair's package is the `package_id` a scheduled run's preparation admitted at its procedure binding's location in this process (`PackAdmission::learn_prepared`; never the admission after start, so E never turns on between an evaluation and its admission), the fact refresh's the reader binding's; the record is never an ordinary fact; admission compares the intent's duration with its own projection's (`policy_facts_stale` with E there) |
| ED2-5 | `ScheduleExplanationCandidate.expected_runtime_ms`, `.expected_runtime_source` (`declared` \| `runs:<n>`) | new optional members of a wire shape (runtime-client; `ac_schedule`, `actingctl schedule`) (tests t1, t9) | ED-2 | ED-2 | every candidate row the Runtime answers carries both; the MCP result schema names them |
| ED2-6 | `config.subsystems` row `expected_runtime` (always on, `config_subsystem_always_on`) | new row value of an existing fact (no shape) | ED-2 | ED-2 | the catalog generator emits v35 only for a Runtime that names it (model §3.8) |
| ED2-7 | `FactRefreshSettings.expected_duration_ms` (policy) | policy interface (memory only) | ED-2 | ED-2 | the fact refresh's declared duration D is `scheduling.fact_refresh.expected_duration_ms`; the spec's `FACT_REFRESH_EXPECTED_DURATION_MS` (60 000) stays as its nominal value, the key's default |

### 1.191 Slice SR-4a2 (0.12.x): the policy closes over blackouts: positions 4 and 7, the day boundary, the admission re-check, the random delay's cut (#386; SCH-M frozen model v1.0 `rd-1024/sched-design/model.md` §3.2 D-3, D-4, D-6, §6 row SR-4a2; Alice's ruling 50, #386 6380164487: the day-boundary rule as recommended, per-instance switch, default on)

The gate of catalog v35 on the Runtime side, over SR-4a1's pure judgements (§1.179).
[scheduling-blackout.md](scheduling-blackout.md) ("Day-boundary fit", "Gradual quiet", "Order of
the deferrals", "Configuration", "Codes") and [actingd-check-config.md](actingd-check-config.md)
are the contract. **No persisted ledger shape and no configuration key**: every key it reads is
SR-4a1's (revision 11); the refusals are existing `policy.dispatch_rejected` records with the
code's outcome. One code is added. Gates: the `gate_386_sr4a2` tests, ten rows of host.toml. The
number 1.191 is provisional for the parallel slices (1.189 is SR-4a3's, 1.190 ED-2's).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| SR4A2-1 | `blackout_active` and `blackout_approaching` with `blackout_source` (registered by SR-0, the value by SR-4a1) | outcome codes, first emitted | SR-4a2 | SR-4a2 | evaluation reasons (the alias as `instance_id`) and admission refusals at Info (the opaque `instance_id`, the wake as the rejection's `next_eligible_unix_ms`); waiting gates of the #372 K2 clock-slot records |
| SR4A2-2 | `policy_day_boundary_approaching` (info, `instance_id`, `window_end_unix_ms`, `required_ms`, `next_eligible_unix_ms`) | outcome code (new) | SR-4a2 | SR-4a2 | SCH-M D-6: E does not fit the activity window it would be admitted in; wake at the next window start; a waiting gate of the clock-slot records |
| SR4A2-3 | `config.subsystems` row `scheduling_blackout` (always on, `count` the host entries) | new row value of an existing fact | SR-4a2 | SR-4a2 | a catalog generator emits `maintenance` events only for a Runtime that names it (GEN-35) |
| SR4A2-4 | the random delay's cut gains the quiet point (the next blackout start minus the lead; where the day boundary applies, the window's end minus E) | behaviour (memory only) | SR-4a2 | — | the draw's `allowed_until` is the earliest of the four; `PolicyRandomDelay` unchanged |

### 1.192 Slice DA-4 (0.12.x): a game root's display declaration `display-layout.json` (#392 / #401; model 392 v1.2 `rd-1012/m392/model-392-adapter-shell-v1.2.md` §4.1, §0.2 C9, §12.1 row DA-4; the coordinator's ruling of 10-13, §17 item 2: the declaration sits at the standard package's root and the NAV-5 root reader reads it)

[package-reference.md](package-reference.md) ("Content-directory tools and the bundle index",
"Standard package root", `display-layout.json`) and [actingd-check-config.md](actingd-check-config.md)
are the contract. **No persisted ledger shape, no configuration key and no new code**: a broken
file is the existing `standard_pack_root_invalid` with one more `standard_pack_problem` token
(#420 S4); the display adapter that uses the declaration comes with DA-2a. The SH-2 rows X6 to
X8 and X10 (§1.25) name DA-4 their first writer by model v1.1's slicing; under v1.2 their
writers are DA-2a (X7, X8 and X10's `display.map`) and DA-4b (X6, the safe area), and this slice
writes none of them. A game root without `display-layout.json` behaves as before, and no
resource repository carries one in this wave. Gates: the contract's `standard_pack_display`
tests (DA4-T1, DA4-T2), the host's `gate_392_da4` tests (DA4-T3 and the identity test) and
actinglab's `gate_392_da4` test (DA4-T4). The number 1.192 is provisional for the parallel
slices.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| DL-1 | root file `display-layout.json` (`actingcommand.display-layout.v1`): `{schema_version, game, scale {by: height \| width, area: screen \| safe_area}, anchor_area: screen \| safe_area, correction {radius_px}}` (`StandardPackDisplayLayout`, `parse_standard_pack_display_layout`, `STANDARD_PACK_DISPLAY_LAYOUT_FILE`; `StandardPackRootBytes.display_layout`, `StandardPackRoot.display_layout`) | resource file format, not a ledger shape (tests DA4-T1, DA4-T2) | DA-4 (reader, contract checks) | resource side (DA-6) | every member required, unknown members refused, no Runtime default; `malformed` at the first unknown, missing or misfit member's pointer (empty for the whole document), `game_mismatch` at `/game`, `display_layout_invalid` at `/correction/radius_px` above 64 (the shape's bound, not a tunable); without `bundle.json` `bundle_missing`; no `pages.json` needed; read by `read_standard_pack_root` for every reader; the spellings are SH-2's `DisplayScaleBy` and `DisplayArea` (X7) |
| DL-2 | `standard_pack_problem` token `display_layout_invalid` | vocabulary token of an existing code (`standard_pack_root_invalid`, contract) | DA-4 | DA-4 | model 392 §9.2: v1.1's code `display_layout_invalid` becomes this token; the code's description lists `display-layout.json` among the root files |
| DL-3 | `package bundle --display-layout <file>`; output `display-layout.json` beside `bundle.json` (`StandardPackRootFiles.display_layout`); `resource verify`'s bundle step lays a repository root's `display-layout.json` the same way | new tool flag and output layout (test DA4-T4) | DA-4 | resource side (DA-6) | laid byte for byte, under its root file name whatever the given file is named, once the game root the bundle makes passes `read_standard_pack_root` (`standard_pack_root_invalid` with the given file's `path`); with it every pack is admitted for real as with any root file; `standard-pack check` lists it in `root_files` |
| DL-4 | the Runtime's reading of the file: at startup with the other root files (a broken one only recorded); in a run's preparation, the game root read when it holds the file (a broken one refuses the run before any lease and task record); check-config's `root_files`; `actingctl packs`' sound root; the scheduler workbench's and `standard-pack compose`'s root read | behaviour (memory only) (tests DA4-T3, identity) | DA-4 | — | what a preparation read is not kept yet (DA-2a keeps it for the run's display map); a file replaced in place is read by the next run; a 1280x720 package on a 1280x720 device records and sends byte for byte what it does without the file and without a root |

### 1.193 Slice DA-2a (0.12.x): the display adapter's capture side for the identity and a pure scale (#392 / #401; model 392 v1.2 `rd-1012/m392/model-392-adapter-shell-v1.2.md` §0.2 C1 to C3, C10, C11, §5.1, §6.6, §8.3, §12.1 row DA-2a; coordinator work order `rd-1026/brief-da2a.md`: the identity and a pure scale only, a translation waits for DA-2b)

[display-adapter.md](display-adapter.md) section 8 is the contract. **No new persisted shape, no
configuration key and no new code**: the SH-2 shapes X7 and X8 (§1.25) get their writer, the
vocabulary `display_problem` gains three tokens, `contained_task_geometry_unavailable` an optional
value, and `contained_task_frame_resolution_mismatch` is retired (`replaced_by`
`display_map_unresolved`, retired in 0.12.15). X10's `display.map` and `display.logical_extent`
keep no writer in this slice. A 1280x720 package on a 1280x720 device records and sends byte for
byte what it did. Gates: the kernel's `gate_392_da2a` tests (DA2-T1, T2, T3, T6, T12, T14, T15 and
the identity test), the host's `gate_392_da2a` tests (DA2-T9, the input prerequisite and the safe
area); the measurement DA2-B is no gate. The number 1.193 is provisional for parallel slices.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| DA2a-1 | the frame gate `Resolution::validate_frame` at the kernel's six capture entries (entry page, navigation locate, sample, page frame, reading agreement frame, Lab dry-run select) | behaviour (memory only) | DA-2a | — | the map from the frame's size and its producer's geometry, the package's base and the run's declaration; the identity uses the frame's own buffer, a pure scale is resampled once to the base's size (REC-5); a translation, a cropped frame or a turned frame is `display_map_unresolved` (`translation_unsupported`, a turned one `orientation_mismatch`) until DA-2b; a refusal of the map is `display_map_unresolved` with its problem (a frame not showing its display `contained_task_frame_invalid`); the frames of one observation keep one map (`recognition_sample_geometry_changed`, a candidate sample `candidate_sample_page_or_geometry_changed`); a recognition record's regions are device boxes (outward) |
| DA2a-2 | `ContainedTaskRuntime::display_layout` (default `None`) and the run's kept declaration (`RunStandardPack`, from `prepare_run_interrupts`) | kernel seam, behaviour (memory only) | DA-2a | — | the declaration a run's preparation read (DA-4, §1.192 DL-4) holds for the whole run and all its layers; `DisplayRule::from(&StandardPackDisplayLayout)` is the mapping input, no second copy of the shape |
| DA2a-3 | `TaskGeometryObservation.display_map` (X7) | first writer | SH-2 | DA-2a | written on a passed `initial` or `changed` observation whose map is not the plain identity (the identity leaves the record as it was); undeclared, it names the rule the map was built by (`width`, `screen`, `screen`, `declared: false`); the validation now admits a declared `translate` of the base's size (insets narrow its anchor area only) |
| DA2a-4 | `TaskGeometryPhase::Changed` with `extent_changed`, `rotation_changed`, `insets_changed` (X8) | first writer | SH-2 | DA-2a | a frame whose size or whose producer's rotation, crop or insets differ from those the map was built on reads the geometry again and is recorded `changed` (the first change its trigger); the map is rebuilt, the display facts are published again when they changed, and the run goes on: a size change no longer fails the input prerequisite |
| DA2a-5 | the input prerequisite under a map that is not the plain identity | behaviour | DA-2a | — | until DA-3 maps input, `display_map_unresolved` (`input_scale_unsupported`, stage `display_input_map`, the frame's and the base's sizes), no input |
| DA2a-6 | `display_problem` tokens `translation_unsupported`, `input_scale_unsupported`, `safe_area_unavailable`; `contained_task_geometry_unavailable` value `display_problem` (optional); `contained_task_frame_resolution_mismatch` retired | vocabulary tokens, an optional value, a retirement (#420 S4) | DA-2a | DA-2a | a declaration measured on the safe area ends the run at its first frame `contained_task_geometry_unavailable` (`safe_area_unavailable`) before DA-4b reads insets |

### 1.194 Slice LN-WS (0.12.x): no read can make a write fail: a request's read is bounded inside a SQLite statement, its history statements span bounded ranges from the window's first record, and an append keeps the ledger's code (#422; root cause `rd-1026/writer-stall/root-cause.md` §5; the production stop of 2026-10-26 19:12:43)

[ledger-observer.md §15](ledger-observer.md#15-configuration-keys-ledger_observer) (the A51 and
LN-WS notes) and [runtime-config.md](runtime-config.md) ("The `ledger_observer` section") are the
contract. **No new persisted shape and no new code**: `ledger_read_budget_exceeded`,
`writer_response_timeout` and a page's `read_budget` limit carry every outcome. Two
configuration keys join `actingd-config` revision 14. Gates: the ledger's `gate_ln_ws` tests (T1
to T5) and the host's (T6, T7); the scale harness's six read forms are a measurement, no gate.
The number 1.194 is provisional for parallel slices.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| LNWS-1 | `ledger_observer.read_check_vm_steps` | configuration key (`actingd-config` revision 14) | LN-WS | — | SQLite virtual-machine steps between two looks at the clock of a read that serves a Runtime request; 10 000, `100..=1000000`, not settable (read when the ledger opens); `LedgerFailurePlan::with_read_check_vm_steps`, `GlobalLedgerConfig::with_read_check_vm_steps` |
| LNWS-2 | `ledger_observer.history_scan_rows` | configuration key (`actingd-config` revision 14) | LN-WS | — | sequences one history statement of a view page spans at most; 65 536, `1024..=1048576`, not settable (read when the ledger opens); `LedgerFailurePlan::with_history_scan_rows`, `GlobalLedgerConfig::with_history_scan_rows` |
| LNWS-3 | the statement bound of `ProjectViewPage` and `ReplayPage` | Runtime-internal behaviour | LN-WS | — | a progress handler on the shared connection, installed while the read holds it and removed before it lets it go; it interrupts the statement in progress once the read's budget is spent: a page answers incomplete, a replay page and an interrupted head check `ledger_read_budget_exceeded`; the Runtime's own reads keep A51's bound between rows |
| LNWS-4 | a view page's history statements | Runtime-internal behaviour | LN-WS | — | spans of at most `history_scan_rows` sequences, the first of one sequence (completed whatever the budget), each later twice the one before; a first page with a window starts at `MIN(sequence)` of the time index for its start, minus one; a run's recovery context the budget cuts short is `context_limit` |
| LNWS-5 | `append_runtime_event`, `record_policy_values`, `linked_policy_run_events` | host failure mapping | LN-WS | — | the fatal `ledger_failure` keeps the ledger's code as its `caused_by` link (`writer_response_timeout`, ...); fatality unchanged |
| LNWS-6 | a reader-pool page's Lab relations (#422 RB-1, 0.12.28; production v0.12.26 after the machine restart of 2026-10-29 05:23; coordinator work order `rd-1029/brief-rb1.md`) | Runtime-internal behaviour (memory only; no key, code, format or table) (ledger gate row "RB-1") | RB-1 | — | the pool builds them once at its start on a connection of its own within the open's deadline (`ledger_observer.open_timeout_ms`), each id with the sequence it became Lab-related at; a page at or before their end filters by its snapshot (the same rows and views as the whole history's read, also pinned to an older snapshot), a page past it reads only the records after it under its own budget and carries them forward; before the build ends, and when it failed, the whole history's read as before; a failed build is a warning of the new location `build_lab_relations` (its deadline `ledger_read_budget_exceeded`), never fatal, not tried again before the next start ([ledger-observer.md §15](ledger-observer.md#15-configuration-keys-ledger_observer), the RB-1 note) |

### 1.195 Slice DA-4b-D (0.12.16): the declared safe area (#392 / #401; Alice's ruling 69 of 2026-10-26, board #392 6414416615 and 6414691189: 「只有安全区适配不做，但是需要留出来声明式的安全区接口，也就是说可以依照声明的文件直接切换安全区形态」, 「一份文件对应一个机型或多个机型，并且修改设置后实时生效」; coordinator work order `rd-1026/brief-da4bd.md`, which replaces model 392 v1.2's DA-4b row)

[display-adapter.md](display-adapter.md) section 9 is the contract. **No new persisted shape and
no new code**: SH-2's X6 `CaptureGeometry.safe_insets` gets its first writer, X7's
`insets_source` its `declared`, the vocabulary `display_problem` four tokens, and
`display_map_unresolved` four optional values; `actingd-config` rises to revision 15 (§6). Not
done, by the ruling: no display's cutout is read (model 392 §8.2), X10's
`display.safe_area.reported` and `display.safe_area.declared` keep no writer, and no device's
model is detected. A 1280x720 package on a 1280x720 emulator records and sends byte for byte what
it did: only a run whose game measures on the safe area is touched. Gates: the host's
`gate_392_da4bd` tests (work order tests 1 to 4) and DA-2a's adapted safe-area test (test 4's
handset). The number 1.195 is provisional for parallel slices.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| DA4bD-1 | declaration file `actingcommand.display-safe-area.v1`: `{schema_version, models: [string], forms: [{display {width, height}, rotation: r0 \| r90 \| r180 \| r270, insets {left, top, right, bottom}}]}`, every `*.json` of the declaration directory | Runtime-side declaration file, not a ledger shape (tests 1 to 3) | DA-4b-D | a person or an agent | every member required, unknown members refused; models non-empty and distinct, forms non-empty, displays at least 1, insets that leave a width and a height, no two forms of one display and rotation; one model in one file only; parsed in the host (`crate::display_safe_area`), its one reader |
| DA4bD-2 | keys `display_safe_area.directory` (string, default `display-safe-area`, relative to the state root, non-empty), `display_safe_area.check_interval_ms` (milliseconds `0..=3600000`, default 1 000), `display_safe_area.model` (string, default empty: none) and per instance `display_safe_area.model` (default `section:display_safe_area.model`; the entry's `display_safe_area.model`) | configuration keys, **`actingd-config` revision 15** | DA-4b-D | — | all settable, read at every frame of a run whose game measures on the safe area; the defaults only in `actingd.defaults.json`; `config.parameters.display_safe_area` and `config.parameters.instance` |
| DA4bD-3 | X6 `CaptureGeometry.safe_insets` on a delivered frame's own geometry; X7 `insets_source` `declared` | first writer | SH-2 | DA-4b-D | the host stamps them before the frame reaches the kernel: the declared model's form for the frame's logical display and rotation, an emulator without a model 0, a handset without one none (`contained_task_geometry_unavailable`, `safe_area_unavailable`, as DA-2a); a change of them is DA-2a's `changed` record with `insets_changed` and the map's rebuild, from the next frame (DA2a-4); a map measured on the safe area names `declared` (DA2a-3) |
| DA4bD-4 | live reload | behaviour (memory only) (test 2) | DA-4b-D | — | the directory is read again once the interval has passed (0: every frame); files of the same names and bytes (SHA-256) keep their catalog; the model setting is read at every frame; nothing needs a restart |
| DA4bD-5 | `display_problem` tokens `safe_area_declaration_invalid`, `safe_area_model_repeated`, `safe_area_model_undeclared`, `safe_area_form_undeclared`; `display_map_unresolved` optional values `path`, `other_path`, `json_pointer`, `config_key` (existing keys) | vocabulary tokens, optional values (#420 S4: one domain code with values) | DA-4b-D | DA-4b-D | each ends the run at stage `display_map_build` with the frame's size and no input (test 3); `safe_area_unavailable`'s meaning narrows to a handset that declares no model |

### 1.196 Slice LN-7c Q (0.12.17): the frame cleaner by ledger query, the evidence-frame rule, and no frame read at the open (#422; Alice's ruling 70; design `rd-1026/ln7c/final-query.md` §1, §2, §4, §5; coordinator work order `rd-1026/brief-ln7c-q.md`; the 14-day question answered A)

[ledger-store.md](ledger-store.md) ("Frame cleaner", "Frame listing", the LN-7c Q paragraph of
the open) and [ledger-observer.md §15](ledger-observer.md#15-configuration-keys-ledger_observer)
are the contract. **No new persisted shape and no new code**: `ledger_index_mismatch`, `ledger_io`
and the cleaner's existing warnings carry every outcome; two ledger locations are added
(`query_frame_retention`, `frame_retention_live_objects`). Three configuration keys join
`actingd-config` revision 16 (§6). Gates: the ledger's `ln7c_q_t_plan_*`, `ln7c_q_teq_*`,
`ln7c_q_t_evidence_*`, `ln7c_q_t_open_*` and `ln7c_q_t_pace_*` tests, the host cleaner's
`ln7c_q_t_pace_*`, `ln7c_q_t_schedule_*` and `ln7c_q_t_evidence_*` tests, and runtime-config's
`ln7c_q_t_evidence_*`. The number 1.196 is provisional for parallel slices.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| LN7CQ-1 | `frame_retention_evidence_max_age_days` | configuration key (top level, **`actingd-config` revision 16**) | LN-7c Q | — | 15, `1..=3650`, settable, taken at each pass start; joins `config.parameters.top_level`; no 14-day key |
| LN7CQ-2 | `ledger_observer.frame_query_steps_per_second` | configuration key (**`actingd-config` revision 16**) | LN-7c Q | — | 500 000, `1000..=100000000`, settable, read after each statement; the pace is in SQLite steps, not rows |
| LN7CQ-3 | `ledger_observer.frame_query_chunk_frames` | configuration key (**`actingd-config` revision 16**) | LN-7c Q | — | 256, `1..=4096`, read at start (not settable) |
| LN7CQ-4 | statements W, L, K and the owner statement (`crates/ledger/src/global/sqlite/frame_query/*.sql`) | Runtime-internal behaviour | LN-7c Q | — | on a read-only connection of the cleaner's own, one read transaction per statement, run only while `ledger_frame_refs` covers `1..=` the snapshot's head; golden plans `plan_w.txt`, `plan_l.txt`, `plan_k.txt` |
| LN7CQ-5 | the pass line `actingd frame_retention pass …` | daemon line | LN-7c Q | — | gains `kept_examined`, `removed_confirmed`, `removed_expired`, `kept_unknown`, `query_steps`; a pass without a complete frame index prints `actingd frame_retention pass skipped=frame_refs_incomplete` |
| LN7CQ-6 | `ac_evidence_confirm_read`, `actingctl evidence confirm-read` | MCP and CLI answer member | LN-7c Q | — | `covers {evidence_frames, in_kept}` (C's answer gains it); a count that cannot be made leaves it out with a warning |
| LN7CQ-7 | `actingledger frames` | offline tool output | LN-7c Q | — | live classes from statement L; `evidence {max_age_days, kept_staying, kept_remove_confirmed, kept_remove_expired, kept_unknown, live_remove_confirmed, confirmed[]}` with `confirmed_by {actor, source, surface, client, at_unix_ms, sequence}` |
| LN7CQ-8 | the writer's open and its retention state | Runtime-internal behaviour | LN-7c Q | — | the open reads no frame and does not list the live frame store; the writer keeps no frame facts and answers no frame view (`GlobalLedger::frame_retention_view`, `frame_retention_view_live` removed); `GlobalLedger::retain_live_frames` trims the frame objects at each pass start |

### 1.197 Slice LN-7c V (0.12.17): `ledger-maintenance verify` checks the frame cleaner's statements against the full view (#422; Alice's ruling 70; design `rd-1026/ln7c/final-query.md` §7 row V, §6; coordinator work order `rd-1026/brief-ln7c-v.md`)

[ledger-store.md](ledger-store.md) ("Frame cleaner", the offline check) and
[ledger-maintenance.md](ledger-maintenance.md) are the contract. **No new code, key, revision or
persisted shape**; one ledger location is added (`verify_frame_query`). Offline only: the Runtime
never runs the check. Opt-in (fix round 1): only `verify --frame-query` runs it; without the flag
`verify` is the complete read alone, as before this slice. Gates: the ledger's `ln7c_v_*` tests.
The number 1.197 is provisional for parallel slices.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| LN7CV-1 | `ledger_index_mismatch` optional values `artifact_id`, `field` (existing keys) | optional values (#420 S4: one domain code with values) | LN-7c V | LN-7c V | operation `verify_frame_query`: the frame whose statement answer differs from the full view, and the answer's member as a JSON pointer (`/class`, `/action`, `/created_at_unix_ms`, `/run_id`, `/folder`, empty for a frame one side does not answer); the ruled (E1, E11) and safe (E3, E4, E5) differences are not reported |
| LN7CV-2 | `INFO actingd: ledger_maintenance_completed verify frame_query_live=<n> frame_query_kept=<n> frame_query_accounted=<n>` | daemon line (stderr) | LN-7c V | — | after a passing `verify --frame-query` of a formal root; never without the flag; the receipt is unchanged |
| LN7CV-3 | `actingd ledger-maintenance` reads `frame_retention_dedup_error` and `frame_retention_dedup_lab` | behaviour | LN-7c V | — | for `verify --frame-query`'s check, with the defaults a start uses when the file names neither |
| LN7CV-4 | `actingd ledger-maintenance verify --frame-query`; `LedgerMaintenanceRequest.frame_query` (default `false`) | command-line flag; host request member | LN-7c V | — | opt-in: runs the check after the complete read; without it `verify` builds no retention index, runs no statement and prints no count line; never ignored (fix round 1b): another operation with it, or `verify` of a root that is not formal (Segment, candidate), is `maintenance_arguments_invalid` |

### 1.198 Slice CF-5 (0.12.18): a pool's capacity and a `resource_projection`'s comparand can be facts (#386, condition-first; Alice 10-26, memory `no-fixed-account-thresholds`: an account's cap is read and a rule writes "≥ cap − margin"; coordinator work order `rd-1026/brief-cf5.md`)

[scheduling/README.md](scheduling/README.md) ("Pool fact references", with "Pool Projection",
"Task Reserves", "Fact Refresh" and "Why A Candidate Does Not Run"),
[scheduling/v2/README.md](scheduling/v2/README.md), [resource-targets.md](resource-targets.md)
and [standard-pack-schedule.md](standard-pack-schedule.md) (`requires`) are the contract. **No
persisted ledger shape**: both forms are catalog format additions (the ledger holds only the
catalog's `catalog_hash`, unchanged for a catalog whose capacities and projection values are
numbers); no new code (the existing `fact_reference_unavailable` is also emitted by a
`resource_projection`; its pending entry's description says so), no configuration key, no wire
or interface revision. The number 1.198 is provisional for the parallel slices.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| CF5-1 | a pool's `capacity` may be `{"type": "fact", "fact_key", "offset"}` (`PoolCapacity::Fact`, `FactReference`; schema `factReference` in `contracts/scheduling/v2/common.schema.json`, used by `v2/pools.schema.json`) | catalog format addition (no ledger shape) | CF-5 | the standard packs' `schedule.json` | V2 only (V1: `unknown_field` at `capacity/type`); read at each evaluation in the pool's scope as the pool reads its value (the pool's minimum confidence, its own expiry), capacity = integer + offset, at least 1; otherwise the pool has no value and its projection is Unknown with the fact's detection suggestion (`fact_missing`, `fact_low_confidence`, `fact_expired`, `type_mismatch`), no constant fallback; a reading above it stays as read, regeneration stops at it; the compiler's reserve and effect bounds apply to a constant only; a constant serializes and hashes as before |
| CF5-2 | a `resource_projection`'s `value` may be `{"type": "fact", "fact_key", "offset"}` (`ProjectionComparand::Fact`) | catalog format addition (no ledger shape) | CF-5 | the standard packs' `schedule.json` | V2 only (V1: `unknown_field` at `value/type`); the referenced fact is read in the pool's scope with its own expiry and a confidence above 0; compared as `projected C b + offset` in `i128`; a referenced fact that gives no integer makes the predicate false with `fact_reference_unavailable {fact_key, unknown_reason}` (CF4-2), no wake, no fact refresh; an integer serializes and hashes as before |
| CF5-3 | `config.subsystems` row `pool_fact_reference` (always on, `config_subsystem_always_on`) | new row value of an existing fact (no shape) | CF-5 | CF-5 | [actingd-check-config.md](actingd-check-config.md); a `schedule.json` that writes CF5-1 or CF5-2 must list it in `requires`, else the composer refuses the file (`schedule_invalid` at the first reference: rules, then pools) |

### 1.199 Slice #427-B (0.12.22): no virtual CPU/GPU/IO deduction across instances, heavy-scene slots with their numbers, optional maximum of concurrent runs (Workflow #427 plan r2 point 5, comment 6460013989; Alice 10-28 12:4x: 「427整单批准并放行，允许进入最近的预发行版」; coordinator work order `rd-1028/brief-427b-budget.md`)

[scheduling/README.md](scheduling/README.md) ("Shared Run Limits"),
[performance-control.md](performance-control.md), [runtime-config.md](runtime-config.md) ("The
performance section") and the policy fragment of the outcome catalog are the contract. **No
persisted ledger shape**: the reasons travel in decisions, the schedule explanation and, as
`why`, in dispatch decision records whose shape is unchanged; a decision reason's outcome gains
values only. The number 1.199 is provisional for the parallel slices (#427-A).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| 427B-1 | the round deducts no CPU, GPU or IO for a chosen dispatch; `host_budget_deferred` retired (`retired_in` 0.12.22) | rule; outcome code retired | #427-B | the policy evaluator | placement against the host snapshot stays (`host_capacity_insufficient`); no budget pool per instance |
| 427B-2 | `heavy_scene_budget_deferred` and `heavy_scene_budget_exhausted` carry `used`, `limit_count`, `admitted_count`, `round_selected_count` (required; the last two new keys of the key table) | outcome values added to pending entries | #427-B | the policy evaluator | slots in use = heavy policy dispatches admitted and not settled (the larger of the controller's capped count and the policy host's own) + heavy ones chosen earlier in the round; each dispatch once |
| 427B-3 | new code `policy_concurrent_runs_deferred` (info, `ladder_class` none; the same four values) | new outcome code | #427-B | the policy evaluator | given only when `performance.pacing.max_concurrent_runs` is set and reached; checked before the heavy-scene slots |
| 427B-4 | `performance.pacing.max_concurrent_runs` (integer `1..=65535`, no default, read once at assembly) | new optional configuration key (**`actingd-config` revision 19**) | #427-B | actingd's start-up assembly | absent: no limit and no `config.parameters` row; `1`: one policy dispatch at a time across every instance |
| 427B-5 | `actingcommand_policy::RunSlots` and `evaluate_with_run_slots` | library API addition | #427-B | the policy host | the admitted dispatches' load profiles and the maximum, read from the dispatch journal at each evaluation; every other entry point passes none |

### 1.200 Slice #427-A (0.12.22): the policy round's scheduled runs on the instance workers, behind a switch that is off by default (Workflow #427 plan r2 points 1, 2, 3, 4 and 6 with C1 to C5, comment 6460013989; Alice 10-28 12:4x: 「427整单批准并放行，允许进入最近的预发行版」; coordinator work orders `rd-1028/brief-427a-lanes.md`, `rd-1028/brief-427a-fix1.md` and `rd-1028/brief-427a-fix2.md`)

[scheduling/README.md](scheduling/README.md) ("Per-Instance Lanes"), [runtime-config.md](runtime-config.md)
and [component-interfaces.md](component-interfaces.md) (`actingd-config` revision 19) are the
contract. **No persisted ledger shape**: a handed-off run writes the records the driver's own run
writes, and an abandoned run is settled online as the start settles a run whose lease already
ended: no release is written and its lease keeps its one end, then its terminal or the
interrupted settlement (fix 2, re-review M-1); a run refused at the stop gets a live
interrupted settlement, with the elapsed `runtime_ms` (not the start's recovered shape), and is
lifted after the restart like the start's (fix 2, re-review L-2; wording, re-review 2 L-A); no
new code. The number 1.200 is provisional for the parallel slices (#427-B holds 1.199).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| 427A-1 | `scheduling.parallel.enabled` (boolean, default `false`) | new configuration key (**`actingd-config` revision 19**) | #427-A | a person (`ac_config_set`, the configuration file) | settable; read by actingd at the start of every policy round, so an instruction takes effect at the next round; on: each admitted scheduled run goes to its instance's worker (one run per instance at a time, different instances at once); off: the round runs them itself, as before |
| 427A-2 | locations `hand_off_scheduled_policy_run`, `run_handed_policy_run`, `read_policy_runs_in_flight`, `update_policy_runs_in_flight` (`since` 0.12.22, `review` pending) | new outcome locations | #427-A | the Runtime host | existing codes only (`runtime_stopping`, `instance_worker_unavailable`, `policy_instance_dispatch_conflict`, `policy_run_identity_mismatch`) |
| 427A-3 | a `policy_outcome_snapshot_changed` evaluation is sampled again at most three times, then the round is given up and deferred to its next trigger (daemon lines `actingd policy_cycle_resampled`, `actingd policy_cycle_given_up`) | behaviour (C1; not behind the switch) | #427-A | actingd's policy driver | each re-sample judges the candidates again on the new sample |
| 427A-4 | a run no worker holds any more (a worker panic, a settlement that failed without stopping the Runtime) is settled online by the start's rule once its lease ended: as at start, a lease that already ended gets no release and no `policy_settlement_release_recovered` (fix 2, re-review M-1; #670 rulings 2 and 3), so it keeps its one end; the run is settled from its terminal or, without one, as interrupted from that end; its registration is then forgotten | behaviour (fix 1, review M1; fix 2, re-review M-1) | #427-A fix 1, fix 2 | the reconciliation | the instance stays held while the run's lease is active; a lease that ended in more than one way stays held and the next start records it |
| 427A-5 | a run handed off before an install drain or a scheduling pause began goes ahead, as the synchronous round's run admitted before them does; only a run not yet taken when the Runtime's stop was requested is refused (`runtime_stopping`): never started, its lease released, given a live interrupted settlement (`policy_settlement_interrupted`, with the elapsed `runtime_ms`, not the start's recovered shape), which the restart lifts, the first one of its pair, like the start's (fix 2, re-review L-2; wording, re-review 2 L-A) | behaviour (fix 1, review L1; fix 2, re-review L-2) | #427-A fix 1, fix 2 | an instance worker | the drain waits for the run and the pause drains it; no failure is recorded for the pause or the drain |

### 1.201 Slice W1-4 (0.12.25): a reset may exclude key prefixes and never clears a task's own last result (Workflow #411 P19, comment 6455013768 items 1 and 2; coordinator work order `rd-1028/brief-wave1.md` W1-4, the declarative way)

[scheduling/README.md](scheduling/README.md) ("Reset exclusions and the task's own last result",
with "Runtime Enforcement" and "Condition edges"), [scheduling/v2/README.md](scheduling/v2/README.md),
`scheduling/v2/timeline.schema.json` and [standard-pack-schedule.md](standard-pack-schedule.md)
(`requires`) are the contract. **No persisted ledger shape**: the member is a catalog format
addition (the ledger holds only the catalog's `catalog_hash`, unchanged for a catalog without
it), and the own last result is the `TaskRuntimeSnapshot` the evaluation already receives; no new
code (`dangling_reference`, `unknown_field` and `type_mismatch` are reused), no configuration
key, no wire or interface revision. The number 1.201 is provisional for the parallel slices of
wave 1.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| P19-1 | a timeline event's optional `excludes_fact_prefixes` (`TimelineEvent::excludes_fact_prefixes`, `TimelineEvent::invalidates_key`; schema `v2/timeline.schema.json`) | catalog format addition (no ledger shape) | W1-4 | the standard packs' `schedule.json` | V2 only (V1: `unknown_field` at the member); at most 128 identifiers, each below one of the event's `invalidates_fact_prefixes` (`dangling_reference` at the entry otherwise); a key below one keeps its observation through the event and its expiry is not capped at the event's next occurrence; absent, the catalog serializes and hashes as before |
| P19-2 | a reset never clears a task's own last result: its own `dependency_completed`, `since_own_run`'s own run and `once_per_reading`'s served check read the task's state as the input holds it | behaviour | W1-4 | the policy evaluator | the window result before `feedback_stop` and another task's `dependency_completed` still read the state as the resets leave it; `clock_slot_live` reads the trigger the same way, so the K4 clock-slot liveness changes with it: a pair whose own dependency was Unknown after a reset may now be live, and more `clock_slot_superseded` or `clock_slot_expired` records are written (diagnostics only, not dispatch) |
| P19-3 | `config.subsystems` row `reset_exclusions` (always on, `config_subsystem_always_on`) | new row value of an existing fact (no shape) | W1-4 | W1-4 | [actingd-check-config.md](actingd-check-config.md); a `schedule.json` whose event writes P19-1 must list it in `requires`, else the composer refuses the file (`schedule_invalid` at the first `/timeline/<i>/excludes_fact_prefixes`); a file that relies on P19-2 names it too |

### 1.202 Slice RB-2 (0.12.28): actingd sets its own memory and I/O priority at start (Alice's ruling of 2026-10-29 10:2x: 「设回正常」; investigation `rd-1029/inv-reboot/rb.md` §3 and RB-2; coordinator work order `rd-1029/brief-rb2.md`)

[runtime-config.md](runtime-config.md) ("The `host` section") and the actingd and host-metrics
fragments of the outcome catalog are the contract. **No persisted ledger shape**: the record is
a daemon line, and the section's rows are new rows of the existing `config.parameters.<section>`
fact. The number 1.202 is provisional for the parallel slices.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| RB2-1 | `host.process_memory_priority`, `host.process_io_priority` (string `normal` \| `low`, default `normal` from the defaults file, not settable: applied before the ledger opens) | new configuration section (**`actingd-config` revision 20**) | RB-2 | actingd's start | another value refuses the start and check-config as `config_value_invalid` naming the key; the CPU priority class is no key and is never changed |
| RB2-2 | `host_process_priority_applied` (info) and `host_process_priority_failed` (warning), `ladder_class` none, values `memory_priority_requested`, `io_priority_requested` (required), `memory_priority_read`, `io_priority_read` (optional; the four new keys of the key table) | new outcome codes, one daemon line per start | RB-2 | actingd's start | a failed call is a `caused_by` `foreign_os_error` with one of the four new host-metrics locations `process_memory_priority_set`, `_read`, `process_io_priority_set`, `_read`; the start goes on |

### 1.203 Slice B43 (0.12.28): a shared interrupt page can declare that reaching its limit is an environment problem (Workflow #411 B43; triage `rd-1028/triage-p71.md` B43, Alice's item 88 of 10-29: 「全部放行」; coordinator work order `rd-1029/brief-wave2d.md` B43)

[package-reference.md](package-reference.md) ("Standard package root", `interrupts.json` and
"Shared interrupt pages") and [policy-suspension.md](policy-suspension.md) ("Which failures
accumulate") are the contract. **No persisted ledger shape**: the member is a root-file format
addition, and the new code rides the existing `task.failed` terminal and
`policy.execution_recorded` failure as any code does (a catalog entry, not a row, §7); no
configuration key, no wire or interface revision raised here. The number 1.203 is provisional
for the parallel slices of wave 2.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| B43-1 | an `interrupts.json` entry's optional `limit_cause`: `"pack_logic"` (default) or `"environment"` (`StandardPackInterrupt::limit_cause`, `InterruptLimitCause`) | new optional member (resource file format, not a ledger shape) | B43 | resource side | read by `parse_standard_pack_interrupts`, so the Runtime, `package bundle` and `standard-pack check` accept and refuse it alike; another value or type is `malformed` (pointer empty, as every decoding refusal of the file); absent, the entry serializes as before; joins `package` revision 6, which BT-PK6 declares, admitted by code until then; it is the first root-file member folded into a `package` revision (the earlier ones are pack members), so BT-PK6 must name `interrupts.json` `limit_cause` when it declares revision 6; a Runtime before 0.12.28 refuses a root that names it (`standard_pack_root_invalid`) |
| B43-2 | a run that meets an `environment` page once more than its `max_per_run` ends with `contained_task_interrupt_environment_limit_reached` (error, `budget_or_permission`; `page_id`, `count`, `limit_count`) | behaviour | B43 | runtime-host | in place of `contained_task_interrupt_limit_reached`; the run's 16 handlings of all pages together keep the old code; the failure is rerun only in every scope (`failure_accumulates`), its identity keeps `g`; the recovery ladder follows its configuration: no default trigger matches the class, so by default the code starts no ladder; an operator who wants one adds a trigger item that names the code |

### 1.204 Slice WD-MT (0.12.28): the watchdog dates a log by the file's own time, and actingd's children do not inherit its log (Workflow #421; investigation `rd-1029/inv-1119.md` §2 and §4 rows C and D; coordinator work order `rd-1029/brief-wd-mt.md`)

[runtime-watchdog.md](runtime-watchdog.md) (Observation 4 and "Logs held open"),
[install-cli.md](install-cli.md) (the close's terminal and the rollback gate's start logs) and
the actingd and host-metrics fragments of the outcome catalog are the contract. **No persisted
ledger shape, no configuration key, no wire or interface revision**: the new code is a daemon
line. The number 1.204 is settled (coordinator, 10-29): ISO-PE, also 0.12.28, takes 1.205.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| WDMT-1 | a candidate log's modification time is read from the file (`fs::metadata`), not from its directory entry (Rust's std falls back to the entry only when the open is refused as access denied); an observation lists the logs once (F, the closed-epoch log test of rows 6 and 6′, row 6′'s look-back), and `status` lists them once more after it decides a formal close, for its close evidence; a log whose time cannot be read is `watchdog_log_unreadable` (`path`, a `caused_by` `foreign_os_error` with `io_op` `metadata`), a log gone since the listing is passed over | behaviour | WD-MT | actingctl watchdog, `actingctl install` | before 0.12.28 the listing's entry time was used and an unreadable entry was passed over; the install rollback gate dates a start log without `<ms>` in its name the same way |
| WDMT-2 | `host_stdio_inheritance_failed` (warning, `ladder_class` none, no values; each failed call a `caused_by` `foreign_os_error` with one of the two new host-metrics locations `process_stdout_inheritance_clear`, `process_stderr_inheritance_clear`) | new outcome code, a daemon line only on failure | WD-MT | actingd's start | the daemon clears `HANDLE_FLAG_INHERIT` on its own stdout and stderr before anything can start a child; nothing is written on success; the start goes on |
| WDMT-3 | the vendor stdio session's saved duplicates of stdout and stderr are not inheritable (their `Dup` step's `after.flags` shows it); a refusal adds one `SetHandleInformation` step with its error and ends the acquisition | behaviour | WD-MT | device (vendor stdio) | the successful path keeps its 32 steps; the restore gives the standard descriptors the originals' inherit flag, cleared since WDMT-2 |

### 1.205 Slice ISO-PE (0.12.28): a policy evaluation failure that is not global no longer stops the Runtime, and its detail is kept (#419; investigation `rd-1029/inv-1119.md`, production v0.12.26 on 10-29 11:19: one `policy_evaluation_input_invalid` ended the policy driver and the monitor stopped all five instances; coordinator work order `rd-1029/brief-iso-pe.md`)

[scheduling/README.md](scheduling/README.md) ("Evaluation failures") and
[runtime-config.md](runtime-config.md) ("Skipped evaluations") are the contract. **No persisted
ledger shape**: the two new codes ride the existing `runtime.failed` record (a catalog entry each,
§7), the refused input's values ride the existing `caused_by` link of `policy_evaluation_rejected`,
and the policy's text the existing native detail; the two keys join **`actingd-config` revision
20**, which RB-2 raised in this release (not raised again). The number 1.205 is the
coordinator's (1.204 is WD-MT's, #1430). Fix round 1 (`rd-1029/brief-iso-pe-fix1.md`, review
`rd-1029/review-iso-pe.md` M1) bounds `outcome_projection_not_ready` (PE-6).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| PE-1 | `policy_evaluation_input_invalid` gains the optional values `policy_input` (vocabulary `policy_input`: `fact`, `outcome`, `task_state`, `completed_window`, `pool`, `priority_offset`), `policy_input_problem` (vocabulary `policy_input_problem`: `observed_after_evaluation`, `unknown_reference`, `duplicate`, `value_invalid`), `fact_key`, `instance_id`, `task_id`, `pool_id`, `observed_at_unix_ms`, `evaluated_at_unix_ms` (two vocabularies and four keys new in the key table) | values of a pending code | ISO-PE | policy crate (`validate_inputs`) | set by the evaluator's input checks (the time checks with both times); `policy_evaluation_rejected` carries it as its `caused_by` link and the policy's code and developer text as its native detail (a `foreign_other_error` link), as `scheduling/README.md` already said; until 0.12.28 only the bare code was kept |
| PE-2 | `policy_evaluation_skipped` (warning, `none`; `count`, `limit_count`, `retry_after_ms`, `next_eligible_unix_ms`, the failure as `caused_by`) | new outcome code, a `runtime.failed` record with system links and a daemon line | ISO-PE | runtime-host (`evaluate_policy_cycle`) | one per skipped evaluation; the host answers a deferred cycle with no evaluation, due `retry_after_ms` later, which the driver applies as any deferred cycle |
| PE-3 | `policy_evaluation_awaiting_person` (error, `none`; `count`, `limit_count`, `retry_after_ms`, the latest failure as `caused_by`) | new outcome code, a `runtime.failed` record with system links and a daemon line | ISO-PE | runtime-host | once per run of skips, at the skip that reaches `scheduling.evaluation.escalate_after`; the run counts the evaluations in a row that did not evaluate, the waits of PE-6 included; an evaluation that evaluates ends the run; the Runtime keeps running |
| PE-4 | `scheduling.evaluation.retry_ms` (milliseconds `1..=3600000`, default `15000`) and `scheduling.evaluation.escalate_after` (integer `1..=4294967295`, default `3`), settable | new configuration keys (**`actingd-config` revision 20**) | ISO-PE | read by the host at every skipped evaluation | a Runtime before 0.12.28 refuses a file that names `scheduling.evaluation`, because the section denies unknown members |
| PE-5 | the evaluation's time is sampled under `fact_write_gate` (and `policy_outcome_gate`), after the evaluation waited for them, not as it starts | behaviour | ISO-PE | runtime-host | an input a concurrent settlement observed and committed while the evaluation waited for its gates is no longer "later than evaluation time" |
| PE-6 | actingd defers `outcome_projection_not_ready` by one debounce also when the trigger has no recompute wake; the host returns it only while fewer than `scheduling.evaluation.escalate_after` evaluations in a row have not evaluated (each wait counts as one, with no record), and skips the next one (PE-2, PE-3) | behaviour | ISO-PE (fix 1 for the bound) | actingd's driver; runtime-host (`evaluate_policy_cycle`) | until 0.12.28 such a trigger ended the driver and the Runtime, and one with recompute wakes was deferred every debounce with no record and no bound |

### 1.206 Slice CFL-1 (0.12.29): recognition preprocessing, the pure chain and its contract (Workflow #411 A58, comment 6371351838; #428 comment 6485683314; design `rd-1029/design-cf.md` §4.2, §4.3, §4.5, §4.6, §6 CFL-1, revised by `rd-1029/design-cf-v2.md` §5.3, §5.7 and §8 (the CFL-1 revision round); review `rd-1029/review-cfl1.md` L1–L3; coordinator work orders `rd-1029/brief-cfl1.md` and `rd-1029/brief-cfl1-fix1.md`, with the names, the hue scale, the tolerance and "kept is white" settled on 10-29)

[recognition-preprocess.md](recognition-preprocess.md) is the contract. **No persisted ledger
shape**, no code, no configuration key, no wire or interface revision: the chain is a library
addition that no package can declare before CFL-2, which adds the member `preprocess` (into
`package` revision 6, as the design says). The number 1.206 is the coordinator's (1.204 is
WD-MT's, 1.205 is ISO-PE's).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| CFL1-1 | `actingcommand_recognition::preprocess`: the validated chain `Preprocess` (`new`, `steps`, `apply`), `PreprocessStep`, `PreprocessKind` (names and members), `ColorSpace`, `ThresholdValue`, `PreprocessOutput` with `PreprocessEvidence {kept_milli, inverted, threshold_value}`, `PreprocessError {step, member, reason}` with `pointer()`, `PreprocessApplyError {EmptyRegion, RegionTooLarge, Internal}`, `check_region_size`, the constants `PREPROCESS_ALGORITHM` (`"preprocess.v1"`) and `MAX_REGION_PIXELS` (2^24), `rgb_to_hsv`, and `Scene::preprocess_region` | library API addition (memory only, since 0.12.29) | CFL-1 | — (CFL-2 hands the processed region to the OCR reads) | the two shapes of at most 4 steps, at most 8 colors; every step integer arithmetic except `gray`, which is the scene's own gray (image 0.25 `to_luma8`, `f32`), so that `[gray]` equals the scene's gray byte for byte; the design's `floor((2126 R + 7152 G + 722 B) / 10000)` is not that gray (it differs on about half of all colors) and is not used |
| CFL1-2 | the region domain: `apply` refuses a region of more than 2^24 pixels (`RegionTooLarge`) before any step, `Scene::preprocess_region` before it crops; within it Otsu compares the exact scores quotient first, remainder second, with every intermediate at most 108 bits (the contract's bound table) | behaviour (library) | CFL-1 revision round | — | a domain of the algorithm like the 4 steps and 8 colors, no configuration key; the `checked_mul` guards stay and cannot fail inside the domain; the tie rule (smallest `t`) is unchanged |
| CFL1-3 | `kept_milli` is counted before `invert`, and `inverted` says whether the chain has it (in place of the first round's `foreground_milli`, counted after `invert`) | evidence definition (memory only) | CFL-1 revision round | — | golden values re-recorded with the coordinator's approval: `keep_range hsv` then `invert` 750 → 250, `gray` then `invert` 625 → 375; every other value unchanged |

### 1.207 Slice STORE-FG-1 (0.12.29): a package declares the steps it runs while the store application is in front (Alice 10-29 10:1x: 「从外部更新的可以单独做成一个独立包，只用官方应用商店更新就行」; design `rd-1029/design-store-fg.md` §2, §3, §6 and the resource side's §8; coordinator work order `rd-1029/brief-store-fg-1.md`)

[resource-declarations.md](resource-declarations.md) ("Store update packages") and
[package-reference.md](package-reference.md) (the layer predicate and the cross-check) are the
contract. **No persisted ledger shape**: the member is a pack format addition, the refusals reuse
existing codes, and the run's records are the existing ones; no configuration key (the operator's
allowlist `store_foreground.allowed_application_ids` is STORE-FG-2's), no wire or interface
revision raised here. The number 1.207 is the coordinator's (1.204 is WD-MT's, 1.205 is ISO-PE's,
1.206 is CFL-1's).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| SF-1 | `control.json` `store_update` {`store_application_id`, `store_operations[]` {`operation`, `proof[]`}} (`actingcommand_contract::StoreUpdateDeclaration`, `validate_store_update`) | new optional member (pack format, not a ledger shape) | STORE-FG-1 | resource side | the source gate refuses a wrong shape or a broken rule as `resource_declaration_invalid` at the member; kernel admission as `contained_task_control_invalid` `reason=store_update_invalid`, also outside `navigable_route` and `linear_steps`; joins `package` revision 6, which BT-PK6 declares, admitted by code until then (BT-PK6 must name `control.json` `store_update`); a Runtime before 0.12.29 refuses a package that names it (`UnknownField`) |
| SF-2 | `program_problem` tokens `store_operation_invalid`, `store_proof_invalid`, `store_way_back_missing` | new tokens of an existing vocabulary on the existing `contained_task_program_invalid` | STORE-FG-1 | STORE-FG-1 | the cross-checks against the task (a store step is a guarded tap from one page, its `guard` on that page and never `unguarded_trusted_coordinate`, so every frame its tap is drawn for, retries and rechecks included, passes the page and with it the proof; each proof an OCR target directly in that page's `required`; a `launch` or `restart` way back), with the path `control.json` and the refused member as `json_pointer` |
| SF-3 | the prerequisite and startup incompatibility token `store_update` | new value of existing refusal details (`contained_task_prerequisite_incompatible` `reason`, the navigation `role_ineligible`, the startup refusal) | STORE-FG-1 | STORE-FG-1 | a store update package is no prerequisite, return-home, edge, chain, handler, `then.run` or startup package and no ladder rung |
| SF-4 | `ContainedTaskRuntime::admit_store_foreground` and `ContainedTaskRuntime::store_input` | new kernel boundary methods with default implementations (admit every store; a store tap is any input) | STORE-FG-1 | STORE-FG-2 (host overrides) | asked and used only in the package's own run (entry `Ordinary`, no declared operation): the admission once before any capture, the store input for a declared store operation's `Tap` at the page-graph and the `linear_steps` input sites; every other input stays `input` |
| SF-5 | `package_bundle_store_tap_unlocated` (warning, `ladder_class` none; `package_id`, `pack_operation_id`) | new outcome code (resource-tooling), design §8.1 | STORE-FG-1 | `package bundle`, `actinglab standard-pack check` | one per package and store step that taps a fixed place (`point`, `rect`, `specific_rect`); nothing is refused |

### 1.208 Slice CFL-2 (0.12.29): recognition preprocessing wired into the OCR reads (Workflow #411 A58; #428 comment 6485683314; design `rd-1029/design-cf-v2.md` §0, §5.4 to §5.8, §5.12, §8 CFL-2, with the five points the resource audit confirmed; question 94 built for option A, one fixed chain per target read once; coordinator work order `rd-1029/brief-cfl2-v2.md`)

[recognition-preprocess.md](recognition-preprocess.md) (section Hand-over) and
[resource-declarations.md](resource-declarations.md) (section Task schema `0.9` recognition
preprocessing) are the contract; [ocr-fields.md](ocr-fields.md), [selection-graph.md](selection-graph.md)
(Shared OCR read, Preprocessing) and [saved-artifact-ocr.md](saved-artifact-ocr.md) name the rest.
**No persisted ledger shape** (the task diagnostic maps OCR evaluations member by member;
ledger revision stays [3, 4]), no configuration key, no new error code. One new warning code. The
package member joins `package` interface revision 6, which BT-PK6 declares; until then it is
admitted by code (`component-interfaces.json` stays `package` [3, 5]). The number 1.208 is the
coordinator's (1.204 is WD-MT's, 1.205 is ISO-PE's, 1.206 is CFL-1's, 1.207 is STORE-FG-1's).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| CFL2-1 | `preprocess` `{"steps": [...]}` on task schema `0.9` `ocr_targets[]` and `candidate_layouts[].shared_ocr`, and on recognition pack schema `0.7` `ocr` targets and `shared_ocr`; passed to the pack as declared (a source that uses it derives pack `0.7`; one without it derives every document byte for byte as before) | new optional package member (joins `package` revision 6) | CFL-2 | resource side (no production package before CF-RF1 passes, design §7) | one parser for source and pack (`OcrPreprocess::from_json`), refusals at their own pointers; pack `0.6` `UnconsumedField`; another target type `UnknownField`; a whole-frame OCR target without `locate` and a shared read's `otsu` `InvalidValue`; a Runtime before 0.12.29 refuses the member as unknown |
| CFL2-2 | the hand-over of every OCR read: without a chain the frame itself as before; with one only the processed region `(0, 0, w, h)` as R = G = B pixels, `Implicit` read as `Region`, blocks moved back onto the region's origin | behaviour | CFL-2 | — | the chain runs once before each provider call, never on a remembered read, a kept observation or a shared read's targets; after it the sample deadline is checked again (`recognition sample budget insufficient`) and a saved frame's OCR timeout loses the chain's time (`OCR request budget exhausted` at zero) |
| CFL2-3 | the chain is part of a text read's request (`TextReadKey`) and of the static read budget (`StaticTextRead`); a shared read serves only targets with an equal chain, and its "serves at least one target" admission counts with chains | behaviour | CFL-2 | — | two chains over one region are two reads; colors of `keep_colors` compare in their declared order |
| CFL2-4 | `OcrEvaluation.preprocess`, `OcrEvaluation.shared_read`, `OcrObservationEvaluation.preprocess`: `{algorithm, steps, scope, rect, source_rect?, kept_milli, inverted, threshold_value}` and `{rect, detected?, limit?, kept_milli?, threshold_value?}` | read-only evidence (memory and read-only answers only) | CFL-2 | — | absent without a chain (`shared_read` on every evaluation a shared read serves); direct serializers carry them: the read-only page responses' `actual` evaluations, a saved frame's OCR report `observation`, and the online observation's `candidate_sample` rows (`ac_observe`, `ac_lab_observe` and the Lab's online `observe`); explicit mappings do not: the task diagnostic (ledger), the read-only target response, the field report artifact, the runtime client's DTOs |
| CFL2-5 | `package_bundle_preprocess_divergent` (warning, ladder class `none`, since 0.12.29, pending; `package_id`, `target_id`, `json_pointer`): `package bundle` and `actinglab standard-pack check` report one per package, OCR target and shared read whose chains differ though the read would serve the target | new catalog entry and report entries (not persisted) | CFL-2 | resource tooling | no existing warning names a target and a shared read (the five package-bundle warnings are entry pages, remedies, shadowed interrupt pages, undeclared recognition packs and unlocated store taps); nothing is refused |

### 1.209 Slice STORE-FG-2 (0.12.29): the store allowlist, and the host taps a declared store step only with exactly that store in front (Alice 10-29 10:1x; her questions 95 to 97, option A; design `rd-1029/design-store-fg.md` §2, §3, §5, §6; review `rd-1029/review-store-fg-1.md` L5; coordinator work order `rd-1029/brief-store-fg-2.md`)

[application-lifecycle.md](application-lifecycle.md) ("Store foreground allowance"),
[runtime-config.md](runtime-config.md) ("The `store_foreground` section") and the runtime-host
fragment of the outcome catalog are the contract. **No persisted ledger shape**: the refusals are
the existing `command.rejected`, `runtime.failed` and the run's terminal, the store in front is the
existing `application.foreground` fact, and the key's row is a new row of the existing
`config.parameters.<section>` fact. The number 1.209 is the coordinator's (1.204 is WD-MT's,
1.205 is ISO-PE's, 1.206 is CFL-1's, 1.207 is STORE-FG-1's, 1.208 is CFL-2's).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| SF2-1 | `store_foreground.allowed_application_ids` (list of 0 to 16 unique Android package names, default `[]` from the defaults file, not settable) | new configuration section (**`actingd-config` revision 21**) | STORE-FG-2 | the operator, in the configuration file | a value that is no list or an entry that is no string is `config_value_invalid` `wrong_type`; more than 16 entries or a malformed entry `out_of_range`; a duplicate or an instance's own `application_id` `section_check`; at start and in `check-config` |
| SF2-2 | `application_store_not_allowed` (error, `declaration_error`; `alias`, `application_id`); location `admit_store_foreground` | new outcome code and location (runtime-host), design §3's one new code | STORE-FG-2 | runtime-host | the run of a store update package whose store the list does not name, before any capture or input (`denied`); a store tap that meets the same at the foreground gate (`command.rejected` + `runtime.failed`) |
| SF2-3 | the foreground gate's expected application: the declared store for a store step's plain `Tap` (`RuntimeInputContext.foreground`, `InputForeground::Store`), else the assigned application | behaviour | STORE-FG-2 | runtime-host | a mismatch is the existing `application_not_foreground`, its detail naming `expected=` the store; client `Input`, Lab operations and layers always expect the assigned application; the host overrides both SF-4 methods in the one runtime that runs a package's own run (`RuntimeContainedTask`), neither in the layer and Lab runtimes |
| SF2-4 | the handset start gate's `device_in_use` for a store update package's own run | behaviour | STORE-FG-2 | runtime-host | the declared store in front is no other application for that run; every other run unchanged |
| SF2-5 | the startup refusal of a store update package: `startup_package_admission_failed` with `reason=store_update` | existing code, new detail (review L5) | STORE-FG-2 | runtime-host | before, `contained_task_resource_reading_run_kind_unsupported`; a resource-reading startup package keeps that code |

### 1.210 Slice LAB-WAIT (0.12.29): offline `observe` loads a wait step whose `to` is a page set (the resource side's report of 10-29: such a step refused the package with `missing field 'click'`; coordinator work order LAB-WAIT)

[resource-declarations.md](resource-declarations.md) (offline `observe`'s page-set operations,
and "Wait steps and wait pages") is the contract. **No persisted ledger shape**, no code, no
configuration key, no wire or interface revision, no new member: actinglab reads an existing
shape that pack containment and the kernel already admit. The number 1.210 is the coordinator's
(1.204 is WD-MT's, 1.205 is ISO-PE's, 1.206 is CFL-1's, 1.207 is STORE-FG-1's, 1.208 is CFL-2's,
1.209 is STORE-FG-2's).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| LW-1 | offline `observe`'s page-set operations (`ObservationResources::multi_destination`, read from the task files) leave out a wait step, `wait` and none of `click`, `application` and `select` (pack containment's `is_wait_step`), as they leave out an application effect and a select step | behaviour (actinglab, since 0.12.29) | LAB-WAIT | — | the generated navigation holds no row for a wait step, so `actions` lists none for it, whether its destinations are a page-set `to` or `expect_after.page_id`; its destinations stay the kernel's (`to`, else `expect_after`); a step that declares `wait` beside a `click` is still read as a click step (pack containment's reading; the kernel refuses it, `wait_member_refused`); a click step whose `to` is a page set still requires its `click` |

### 1.211 Slice SS-1 (0.12.30): a pool's base valuation counts on every instance, and the resource layer reads no task state (Alice's ruling 100, #386 comment 6490879952: 「没有玩家目标时，跑建模算出的基准最优解」, resource weight feeds task weight and never the reverse; ruling 102: a met goal only stops adding score; design `rd-1029/design-sch-state.md` §3, §4.1, §4.2, §5 SS-1 and `rd-1029/design-acm-fit.md` C1 to C3; coordinator work order `rd-1029/brief-wave3a.md` SS-1)

[resource-targets.md](resource-targets.md) ("Base valuation on every instance", "Evaluation" and
"Resource weights (v2)") and [scheduling/README.md](scheduling/README.md) ("Pool Valuation" and
the dispatch decision record) are the contract. **No persisted ledger shape**: the term rides the
existing `resource_weights` reason and `score_computed` detail, and the decision record's
existing fields; no new code (`resource_weights` is reused, its catalog description updated), no
configuration key, no wire or interface revision. In production no pool declares a `valuation`
and no task declares `produces` yet (`rd-1029/design-sch-state.md` §2.1), so neither the base
term nor the v1 `U` change (SS1-2) can fire live until the resource side writes them. The number
1.211 is the coordinator's.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| SS1-1 | without an active v2 policy (no record, a withdrawal, an expired or unreadable record, a v1 policy) every candidate that produces a valued pool covering its instance carries the base term `R` (`W = B`, `Γ = 0`): it is added to the effective score, enters the score stage when `R > 0`, and shows as `resource_weights` `policy=none …`, ` resources=<R>` in `score_computed` and, unless a v1 target applies, `target=<R> target_id=* mode=adjust … offset_kept=1` in the dispatch decision record | behaviour | SS-1 | the policy evaluator | until 0.12.29 the base weight counted only on an instance with an active non-empty v2 policy; a v1 target's score adds on top (`adjust`: `base + s + R`; `override`: `utility + s + R`), its record keeps `target=<s>`; a base term bounds no freshness and wakes nothing; a candidate that produces no valued pool, and every catalog without a valuation, is byte for byte as before (SS1-2's v1 `U` aside); v2 has no paused target yet, so "every target paused" (v3) needs nothing here |
| SS1-2 | the resource layer (`weigh_production`) reads the pools' valuation, the stored targets, the inventory readings and each task's own `produces` only; a v2 target's `no_producing_task` lapse and a named task's `task_disabled` are reports only (the target resolves and keeps covering the disabled task); a v1 target's `U` includes the named tasks an instance override disables | behaviour (one-way guard) | SS-1 | the policy evaluator | no observable v2 change: a disabled task has no candidate; v1: disabling a named task by instance override no longer raises the other named tasks' `s` (before, `U` dropped the disabled task's contribution); a v2 target's `tasks` list still narrows who receives `Γ` (it is target data, not task state; #429 A.5 v3 removes `tasks`) |

### 1.212 Slice SS-3 (0.12.30): run-count quotas become optional (Alice's ruling 100, #386 comment 6490879952: dispatch follows live account state, there is no "run task X N times"; design `rd-1029/design-sch-state.md` §1.1, §4, §5 SS-3; coordinator work order `rd-1029/brief-wave3a.md` SS-3)

[scheduling/README.md](scheduling/README.md) ("Optional run-count bounds"),
[scheduling/v2/README.md](scheduling/v2/README.md) and the V2 `tasks` and `activity` schemas are
the contract; [remedies.md](remedies.md) (B8's `daily_remaining`, `last_daily_slot`) and
[runtime-config.md](runtime-config.md) (the ceiling of E) name the rest. **A persisted shape
changes** (SS3-3): the members of the budget receipt become optional. Every reader of the receipt
sees the change: the admission record of `policy.dispatch_admitted` and of
`policy.dispatch_completed`, the retry round of `policy.control_state_recorded`, and the global
ledger query's public projection `PublicPayload.policy_admission`, which reaches clients through
runtime-client. A receipt of a catalog that writes every bound is unchanged byte for byte, and so
is every existing catalog's identity; `ledger` stays [3, 4], the record type, its other members,
the tables and the schema strings are unchanged.

A receipt that leaves members out is written only after the resource side first leaves a bound
out. From then on a reader before 0.12.30 cannot decode it (no downgrade, Alice 10-11):

- **A Runtime before 0.12.30** refuses the ledger with `corrupt_ledger_record`. It does so at open
  when such a record lies among the newest `ledger_observer.open_verify_tail_records` rows it
  verifies. Otherwise it does so later, when the background recheck reaches the record; the
  recheck latches a Global failure and stops the Runtime with the same code. It also refuses a V2
  catalog that leaves `loop_budget` out.
- **Tools and the UI before 0.12.30** fail to decode a ledger event query page that carries such
  a receipt in `PublicPayload.policy_admission`.

No ledger revision is bumped now (the coordinator's decision of 10-29). The 0.12.30 release notes
and Workflow #428 say both:

- after the resource side first leaves a bound out, no rollback below 0.12.30 is possible;
- the resource side leaves bounds out only once every deployment runs 0.12.30 or later.

The bump comes with SS-9, which deletes the members.

No code, location, configuration key, wire or interface revision is added. The refusals reuse
`missing_required_field`, `type_mismatch` (an explicit `null`), `loop_budget_missing` and
`policy_budget_exhausted`. The number 1.212 is the coordinator's. Safety bounds are untouched:
`on_failure` retries and escalation, `scheduling.rerun.max_per_occurrence`, the recovery ladder,
the per-run deadline, in-pack loop caps and concurrency. The recognition maintenance daily
throttle (safety bound S4, `scheduling.fact_refresh.daily_limit`) relies on the `task_daily`
counter, which the Runtime keeps only because the fact refresh task writes `daily_limit`
(`fact_refresh_task`); SS-9 must keep that item or give the throttle a counter of its own. The
no-progress guard (SS-4), batch allowances (SS-6) and deleting the members (SS-9) are later
slices.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| SS3-1 | V2 `tasks[].loop_budget` and each of its items `daily_limit`, `window_iteration_limit`, `max_runtime_ms`; V2 `profiles[]` `daily_budget`, `max_window_iterations`, `session_max_ms` | required members become optional (catalog format, not a ledger shape) | SS-3 | resource side (each rule migrates; RS-1 to RS-7 of the design) | an absent one serializes and hashes as nothing; a written one is checked as before (`loop_budget_missing` for 0, `limit_exceeded` above 1 000 000); V1 still requires all six, now as `missing_required_field` at the member from the validator instead of the parser; in V1 and V2 an explicit `null` is refused as `type_mismatch`, as before SS-3 and as the V2 schemas declare (no file in the repository writes one) |
| SS3-2 | an unwritten bound is no dimension: admission reserves and refuses nothing on it, the retry round does not judge it, settlement never rewrites a run to `policy_runtime_budget_exceeded` on it, a refund gives back only the counts taken, E has no ceiling without `max_runtime_ms`, B8 derives `daily_remaining` and `last_daily_slot` only from the written daily bounds (both absent with neither), the strategy estimate takes the bounds written and the period | behaviour | SS-3 | runtime-host, policy crate | a written bound applies exactly as before; the Runtime keeps no count for an unwritten dimension and does not clear the counts it kept, so a bound written back within the same day (or window) reads back the count recorded before it was left out, and the runs admitted while it was absent are not counted; the fact refresh task keeps `daily_limit` written for S4 (section head) |
| SS3-3 | `PolicyBudgetReceipt`: each of `task_daily_used`/`task_daily_limit`, `task_window_used`/`task_window_limit`, `task_runtime_reserved_ms`/`task_runtime_limit_ms`, `activity_daily_used`/`activity_daily_limit`, `activity_window_used`/`activity_window_limit`, `activity_runtime_reserved_ms`/`activity_runtime_limit_ms` optional, a pair present together or not at all (`PolicyBudgetReceipt::is_valid`; payload validation `invalid_policy_budget_receipt` otherwise); `runtime_exceeded` never exceeds an absent runtime pair | persisted shape: required members become optional (the admission record of `policy.dispatch_admitted` and `policy.dispatch_completed`, the retry round of `policy.control_state_recorded`), and the public projection of the global ledger query (`PublicPayload.policy_admission`, to clients through runtime-client) with them | SS-3 | runtime-host (admission; the dispatch's completion carries it into `policy.dispatch_completed`) | written exactly as before for a catalog that writes every bound; a carried pair keeps the rule `used` between 1 and the limit; a reader before 0.12.30 cannot decode a receipt with members left out, which is written only after the resource side first leaves a bound out (section head) |
| SS3-4 | `DispatchPrerequisites.daily_limit`, `window_iteration_limit`, `max_runtime_ms` optional; the intent check requires each one carried to be above 0 and the intent's expected duration within a carried `max_runtime_ms` | policy interface (memory only; the intent is rebuilt from the record) | SS-3 | policy crate | a dispatch of a catalog that writes all three pins them with the bytes of before |

### 1.213 Slice R-0 (0.12.30): the player workbench's wire shapes first (Workflow #429 A.2 R-0 and A.8, with the 10-29 17:0x revision: Alice's rulings 100 and 102, targets name no task and carry no `on_met`; coordinator work order `rd-1029/brief-wave3a.md` R-0)

[runtime-interface.md](runtime-interface.md) ("Workbench request kinds", "Blocks", "Lab
options") and [component-interfaces.md](component-interfaces.md) (`runtime-client` revision 3)
are the contract. **No persisted ledger shape**: every shape is a wire shape, the routine summary
and the activity live in memory, and the refusals record nothing. **No new outcome code**: the
kinds without behaviour answer the shared `not_implemented`, as `LoadPack` does before U-3; five
runtime-host locations are new. No configuration key. The number 1.213 is the coordinator's
(1.211 is SS-1's, 1.212 SS-3's, 1.214 R-15a's). The review round (`rd-1029/review-r0.md` M1 to
M8 and L5, coordinator brief `rd-1029/brief-r0-fix1.md`) added R0-8 and R0-9 and settled the
one name of the edit-version token: `expected_version` (`ConfigureSettings.expected_revision`
before it). Fix round 2 (coordinator brief `rd-1029/brief-r0-fix2.md`) added R0-10, so that the
console pins A.5's stale-schema mark with the rest.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| R0-1 | `RuntimeOperation::RunInstanceTask {instance_alias, package_id, holder_id, response_deadline_ms?}`, `StopInstanceRun {instance_alias, request_id, wait_ms}`, `EditPackSchedule {scope, package_id, rows: [PackScheduleEditRow {from_unix_ms, until_unix_ms?, schedulable}], expected_version?}`, `EditResourceTarget {instance_alias, expected_version, op: ResourceTargetEdit}` (`{kind: upsert, target_json}`, `{kind: set_paused, id, paused}`, `{kind: delete, id}`) and `RoutineSummary {instance?, granularity: hour \| day, since_revision?}` | new request kinds (**`runtime-client` revision 3**) | R-0 | the behaviour slices: R-5, R-6, R-8, R-13c, R-12 | each Denied `not_implemented` (`operation` `run_instance_task`, `stop_instance_run`, `edit_pack_schedule`, `edit_resource_target`, `routine_summary`; new runtime-host locations) and recording nothing until its slice; only an instance alias is checked before (`EditPackSchedule`: its scope's `instance_id` when the scope is an instance); `EditResourceTarget.expected_version` is `ResourceTargetView`'s `active.version`, `0` when the view has no `active`; `target_json` keeps `ApplyResourceTargets`' style (a v3 target as JSON text: `id`, `maintain{at_least}` or `gain{amount}`, the pool, `paused?`, `valid_until_unix_ms?`, no `tasks`, no `on_met`), parsed by R-13a |
| R0-2 | `RuntimeResult::InstanceRunStopped {stopped: InstanceRunStop {request_id, settled, reset?: done \| not_needed \| failed}}`, `PackScheduleEdited {edited: {version, event_id}}`, `RoutineSummary {summary: RoutineSummaryPage {owner_epoch, granularity, revision, instances: [{instance_alias, coverage_from_unix_ms, buckets: [{start_unix_ms, end_unix_ms, revision, tasks: [{task_id, successes, failures?: [{code, count}], interruptions}]}]}]}}` | new results (**`runtime-client` revision 3**) | R-0 | R-6, R-8, R-12 | `RunInstanceTask` answers as `RunContainedTask`, `EditResourceTarget` as `ApplyResourceTargets`; no member counts runs done or allowed (ruling 100): a bucket's task row holds health statistics only; `interruptions` counts the runs that ended `contained_task_cancelled` (a stop, a drain's cancel), never in `failures`; the summary's `revision` lives in its `owner_epoch`, and a client whose `since_revision` came with another epoch drops it; `PackScheduleEdited.version` is an opaque token compared only for equality |
| R0-3 | `ConfigureSettings.expected_version?`, `ApplyResourceTargets.expected_version?` | new optional request members (**`runtime-client` revision 3**) | R-0 | R-4, R-13c | absent when unused, so a request without them serializes as before; a request that names one is Denied `not_implemented` (`operation` `configure_settings`, `apply_resource_targets`) and records nothing until its slice; the console does not send them before R-4 and R-13c deploy, and may send such a refused request again without the member; each value is an opaque version token compared only for equality (R0-9) |
| R0-4 | `RuntimeInstanceStatus.activity? {kind: policy_run \| direct_run \| host_run \| reconnect \| selfcheck \| emulator_control \| ladder, request_id?, package_id?, since_unix_ms}` | new optional status member (**`runtime-client` revision 3**) | R-0 | R-0b, in 0.12.31, fills all seven kinds at once; `policy_run` and `direct_run` always carry `request_id` and `package_id`; the same release raises `runtime-client` `min` to 3 (§1.216 R0C-4; before it: the slice that fills each kind) | memory only, never read from the ledger; absent in every 0.12.30 answer, so a revision-2 reader still reads the status; the release that first fills it raises `runtime-client` `min` to 3 |
| R0-5 | `ControlEmulatorInstance`, `SelfCheckInstance`, `ApplicationLifecycle` move from option two to the general instructions; the five new kinds are general (four instructions recorded through G14, `RoutineSummary` a read that records nothing) | request table (`interface_class`) | R-0 | — | no behaviour before U-6: the gate marks and never refuses, and the origin gates of `validate()` stay until U-6 |
| R0-6 | the Lab options' writer: `actingctl install` only (`install new`, `install config lab-options`), acsetup through it | contract wording | R-0 | — | settles `runtime-interface.md` against `component-interfaces.md` and `install-cli.md` |
| R0-7 | `runtime-client` [2, 2] → [2, 3] in `distribution/windows/component-interfaces.json` | interface revision | R-0 | — | revision 3 is revision 2 plus R0-1 to R0-4, R0-8, R0-10, §3 W5 and, from R-0c (§1.216 R0C-3, R0C-4), `PackScheduleView` and `StartReport`; W6, W7 and W13 are not built and are not part of it |
| R0-8 | `RuntimeResult::SettingsConfigured.config_version?`, `RuntimeResult::RuntimeFactSnapshot.config_version?` (beside `snapshot`, never in the persisted `RuntimeFactSnapshot`), `ActiveResourceTargets.rows?: [ResourceTargetRow {id, kind: maintain \| gain, resource, goal, baseline?, current?, remaining, progress_milli, state: active \| met \| completed \| paused \| awaiting_baseline \| unknown \| unmapped, reading: {observed_at_unix_ms?, fresh_until_unix_ms?, pending_reason?}, completed_at_unix_ms?, updated_at_unix_ms}]` (#429 A.5's read model; the table's version is the existing `active.version`) | new optional answer members (**`runtime-client` revision 3**) | R-0 | R-4 (`config_version`), R-13e (`rows`) | absent in every 0.12.30 answer, so each answer serializes as before and a revision-2 reader still reads it; the release that first fills any of them raises `runtime-client` `min` to 3, as for R0-4 |
| R0-9 | the edit-version token: `expected_version` in a request, the answer's own member after a write (`config_version`, `ResourceTargetsApplied.version`, `PackScheduleEdited.version`), `current_version` in a `fact_observation_version_conflict`; opaque, compared only for equality, never ordered, never related to a `QueryEvents` sequence; and the console's revision rules (a console that sends any revision-3 shape declares `min` 3) | contract wording | R-0 | R-4, R-8, R-13c | [runtime-interface.md](runtime-interface.md), "Workbench request kinds" |
| R0-10 | `ResourceTargetView.stale_schema?` (a boolean at the view's top level, beside `active`) | new optional answer member (**`runtime-client` revision 3**) | R-0 | R-13a | #429 A.5: `true` when the instance's stored policy is a v1 or v2 record, which the v3 reader reads as no table (no `active`) instead of refusing the view; absent in every 0.12.30 answer, so the view serializes as before and a revision-2 reader still reads it; the release that first fills it raises `runtime-client` `min` to 3, as for R0-4 and R0-8 |

### 1.214 Slice R-15a (0.12.30): the UI side of the installer's interface edges, and the two Lab flags from the graphical wizard (Workflow #429 A.4 increments a and h; design `rd-1029/429-A-runtime.md` §A.4, the R-15a entry; coordinator work order `rd-1029/brief-wave3a.md` R-15a)

[install-cli.md](install-cli.md) ("Interface checks", "UI edges"; "Lab options from the graphical
wizard") and [component-interfaces.md](component-interfaces.md) ("Checks per operation") are the
contract. **No persisted ledger shape**, no outcome code, no configuration key, no wire or
interface revision: the edges are the installer's own checks of declarations that already exist,
and no anchor of a declared interface changes. The number 1.214 is the coordinator's.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| R15A-1 | party U\*: a UI build's declaration, read from its manifest's `interfaces` (the UI's `component-interfaces.json`) by the parse rules, a missing `runtime-outcomes` counting as [0, 0] (`UiDeclaration`) | behaviour (`actingctl install`) | R-15a | — (the release reader R-15b and the layout R-15c bring a UI build) | until then only the gate tests read one; the UI manifest's `installation_selection_schema` agreement stays with R-15b's reader |
| R15A-2 | the U\* edge rows `actingd-config` `installer_ui_next_common` (I ∩ U\* ∩ R\*), `ledger` `previous_max_in_ui` (Rp.max ∈ U\*) and `next_max_in_ui` (R\*.max ∈ U\*), `runtime-client` `ui_meets_next` (U\* ∩ R\*), `runtime-outcomes` `next_max_in_ui` (R\*.max ∈ U\*), each naming every party with its source and range; U\*'s source `manifest`, or `default` for a `runtime-outcomes` it does not name | behaviour (edge rows of the plan document's existing shape) | R-15a | — (R-15d adds them to the update and rollback plans and the rollback gate's part 1) | `install-selection` has no edge on a single-location root (no `install\active.json`); `component-interfaces.md` defines no edge of the UI being replaced; a failed row will count in the existing refusal `interface_edge` |
| R15A-3 | the graphical wizard always passes `--lab-authoring true\|false` and `--lab-debugging true\|false` to `install new` and shows the result's `lab_options_enabled` as returned; a flag given decides its option, interactive or not | contract rule | R-15a (contract only) | acsetup (UI U-2), `actingctl install new` (R-15e) | no code: `install new` and `config lab-options` are not built yet, and the contract's flags already take an explicit value |

### 1.215 Slice UPD-1 (0.12.30): a "client out of date" dialog ends the run by name, and the Runtime runs the store update package (Alice's rulings 95 and 96 of 2026-10-29 17:1x, 96 read as automatic; 97 stays open and is not built; design `rd-1029/design-upd1.md`; coordinator work order `rd-1029/brief-upd1.md`)

[package-reference.md](package-reference.md) (`interrupts.json` `result`, "Shared interrupt
pages", the cross-check), [instance-fact-store.md](instance-fact-store.md) ("Application update
facts", the capacity ledger), [policy-suspension.md](policy-suspension.md) ("Which runs"),
[runtime-config.md](runtime-config.md) (the changed default),
[emulator-control.md](emulator-control.md) (the ladder rule),
[scheduling-blackout.md](scheduling-blackout.md) (position 4a, the clock-slot class),
[scheduling/README.md](scheduling/README.md) ("Application Update Hold"),
[standard-pack-schedule.md](standard-pack-schedule.md) ("The store update rule") and the
runtime-host fragment of the outcome catalog are the contract. **No persisted ledger shape**: the
fact rides the existing `fact.published`, the end the existing `task.failed`, `runtime.failed` and
`policy.execution_recorded`, the ladder's stop the existing `recovery_rung_finished` and
`recovery_ladder_finished`; no configuration key, no `actingd-config`, `ledger`, `runtime-client`
or `runtime-outcomes` revision. The number 1.215 is the coordinator's (1.211 is SS-1's, 1.212
SS-3's, 1.213 R-0's, 1.214 R-15a's). Game-neutral: no store, game or page is named.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| UPD-1 | an `interrupts.json` entry's optional `result`: `"application_update_required"` (`StandardPackInterrupt::result`, `InterruptResult`), only with `then: "end_run"` | new optional member (resource file format, not a ledger shape) | UPD-1a | resource side | read by `parse_standard_pack_interrupts`, so the Runtime, `package bundle` and `standard-pack check` accept and refuse it alike; on another `then` it is the new `standard_pack_problem` token `interrupt_result_invalid` at `/interrupts/<i>/result`; another value or type is `malformed`; absent, the entry serializes as before; joins `package` revision 6 beside `limit_cause` (BT-PK6 names it), admitted by code until then; a Runtime before 0.12.30 refuses a root that names it |
| UPD-2 | `contained_task_application_update_required` (warning, `none`; `page_id`) | new outcome code (runtime-host): no existing code is a non-accumulating, ladder-free end that names the dialog | UPD-1a | runtime-host | ends a run the page ended, after its handler; not under `contained_task_interrupt_`, so it never accumulates; class `none` matches no default trigger and `stability_burst` does not count it |
| UPD-3 | the default `scheduling.rerun.condition_codes` lists `contained_task_application_update_required` | **changed default** (no new key, no `actingd-config` revision; the release notes say so) | UPD-1a | `distribution/windows/actingd.defaults.json` | a scheduled run that met the dialog ends by a condition: no streak, no R22, counts given back, its rerun owed (option A of Alice's possible question 109; a sensitive task's stays an ordinary failure) |
| UPD-4 | instance fact `health.application.update_required` (boolean, instance scope, `source_detector` `runtime.application-update`, no expiry, no `invalidate_on`) | new fact key of the existing instance store | UPD-1a | runtime-host only | true before the terminal of a run that met the dialog (`application_update:run:<run_id>`); false before the successful terminal of a `store_update` package's own run, scheduled or direct; false as a start's seed for a configured instance without a record (`application_update:seed`), a record already there kept; a store refusal fails that run (Fail Loud); booked in the capacity ledger (1 per instance) |
| UPD-5 | `application_update_key_reserved` (error, `none`; `fact_key`) | new outcome code (runtime-host) | UPD-1a | runtime-host | a client's `PublishFact`/`PublishFacts` of the key, and any other Runtime path, refused before the fact write gate with nothing appended |
| UPD-6 | a ladder rung's package run that ends with UPD-2 stops the ladder: the rung `failed` with `recovery_ladder_interrupted` caused by it, the ladder `exhausted` at warning, no further rung | behaviour (a rule that stays in code) | UPD-1a | runtime-host | no emulator restart for an old client; the startup package that meets the dialog is one `runtime.failed` at warning and opens no ladder (as every startup package) |
| UPD-7 | `package_bundle_interrupt_page_shadowed` is not reported for a store update package (`store_update`) that declares a page whose entry names a `result` | behaviour (resource tooling) | UPD-1a | resource-tooling | the page is the store update package's entry by its part; a package without `store_update` that declares it is still reported |
| UPD-8 | position 4a of the eligibility closure: while an instance's UPD-4 fact is true, every candidate of it except a store update task (its bound package declares `store_update`, read by the remedy table's metadata-only scan, `RemedyDeclarations::store_update`) is deferred `application_update_pending` (info, `none`; `instance_id`, `fact_key`) with no wake; the fact refresh's `fact_refresh_pending` names it as `gate_code` | new outcome code and behaviour (runtime-host, kernel's `RemedyDeclarations` gains `store_update`) | UPD-1b | runtime-host | owed reruns, R22, remedy requests and chain successors included; other instances untouched (#419); the table rebuilds at `scheduling.suspension.lift_check_min_interval_ms` while an instance waits, so an in-place store package counts from the next rebuild; a package the scan cannot read is no store task; a waiting gate for the clock-slot records; admission relies on `policy_facts_stale` |
| UPD-9 | the idle plan counts a decision UPD-8 held (its reason, or a `fact_refresh_pending` whose `gate_code` is it) neither due nor as a wake; fix round 1 (review M1): nor any other decision of an instance whose UPD-4 fact was true in the same evaluation but a store update task's, whatever its reason (the policy host hands `record_held_back` the waiting instances and the store update tasks, `IdleUpdateHold`) | behaviour | UPD-1b | runtime-host | the plan never wakes the instance for a held task, nor for one not yet a candidate (its cooldown, its clock trigger, the fact refresh); the store update task's own decision, its cooldown's wake included, still counts; other instances keep their wakes |
| UPD-10 | a shared interrupt page with UPD-1's `result` whose handler fails short of fatal still ends the run with UPD-2 (`page_id`), the handler's failure its `related` link, UPD-4 true before the terminal; the handler layer is still `EntryRecoveryFailed` with its own code and no `interrupt_handled` follows | behaviour (fix round 1, review M2, the coordinator's decision of 2026-10-29) | UPD-1 fix 1 | runtime-host | the page seen is what matters, and the store update package starts from the dialog page itself; a fatal handler failure poisons the run as before, and a stop of the run during the handler (cancellation, operator pause, deadline) ends it as that stop; before, any handler failure ended the run with the handler's own code, accumulating toward a pause and opening a `pack_logic` ladder |
| UPD-11 | `package bundle --schedule` and `standard-pack check` refuse a `schedule.json` rule whose package declares `store_update` and whose `cooldown_ms` is 0: `standard_pack_root_invalid`, `schedule_invalid` at `/rules/<i>/cooldown_ms` | behaviour (resource tooling; fix round 1, review L6) | UPD-1 fix 1 | resource-tooling | the cooldown is the only pace of the store's "no new version yet" loop; a validity rule, no key; existing codes and problem token; a rule without the member was refused already (required); the Runtime's composer does not repeat it |
| UPD-12 | the start's seed of UPD-4 (`application_update:seed`, false) is observed at 1, the earliest time a fact record may carry, never at the start; a run's record stays at max(now on the Runtime clock, the previous record's observation + 1) | behaviour (fix round 2: CI run 40852041172) | UPD-1 fix 2 | runtime-host | the seed is the default, not an observation; dated at the start it is an evaluation input observed after the time of any evaluation or forward projection asked about an earlier time, which the policy evaluator refuses whole (`observed_after_evaluation`: `policy_evaluation_rejected`, `policy_evaluation_input_invalid` for a forward projection) |
| UPD-13 | UPD-8 holds whatever the ranking: a candidate of a waiting instance that is no store update task and that the round settles before it asks the eligibility closure (behind the instance's selected candidate, behind the shared run limits, held by a chain handover) is deferred `application_update_pending` with no wake in place of that settlement's code; the fact refresh's names it as its gate (policy crate `InstanceHolds`, `evaluate_with_instance_holds`; the host passes the waiting instances with their store update tasks exempt) | behaviour (fix round 3: CI run 40858239951) | UPD-1 fix 3 | runtime-host, policy crate | before, a store update task that ranked first took the instance and the ordinary candidate behind it stayed eligible with `instance_already_selected`, never asked; a candidate the closure is asked about keeps the order of the deferrals (positions 1 to 4 first); memory only, no decision identity or snapshot changes; every other evaluator entry point holds nothing |
| UPD-14 | Alice's ruling 109 (2026-10-29 20:4x, revised; 「识别特定特征，并使用一个特殊的退出码标记这个任务，再去进行更新，完成更新后可以直接重跑更新前的任务以确保达到预期」) answers UPD-3's question: every run the update dialog interrupts, wherever it was, ends with UPD-2, which stays a condition code (no failure, no streak, no pause), and is rerun once a store update package's run has cleared UPD-4 | ruling recorded (no shape, code, key or behaviour change) | — | — | option A as built since UPD-1a; no split by the run's progress and no second code: the first answer of 20:3x (a run interrupted midway counts as a failure) concerned another situation and is withdrawn |
| UPD-15 | a `schedule.json` timeline event whose `invalidates_fact_prefixes` covers `health.application.update_required` (the key itself or any prefix of it, `health.` included), unless one of its `excludes_fact_prefixes` keeps the key, is refused: `standard_pack_root_invalid`, `schedule_invalid` at `/timeline/<i>/invalidates_fact_prefixes/<j>` (the first such entry in file order); the contract crate's `APPLICATION_UPDATE_REQUIRED_FACT_KEY` names the key for the composer, and runtime-host's key is it | behaviour (resource file validity; fix round 5, the note of review `rd-1029/review-upd1-r3.md` beside L1 to L3, coordinator brief `rd-1029/brief-upd1-fix5.md`) | UPD-1 fix 5 | policy crate (`compose_standard_pack_rules`), so the Runtime's workbench and check-config, `package bundle --schedule` and `actinglab standard-pack check` alike | no new code or problem token (the composer's refusals of `schedule.json` are all `schedule_invalid` at their pointer) and no key; the gate and the holds read the fact as recorded while the evaluation projects it through the timeline, so such a reset made the store update rule's trigger read the key missing (Unknown) and never run while the gate kept holding the instance; before, the contract only said such an event must not be written; a catalog given whole (the configuration's catalog section) is not composed and not checked |

### 1.216 Slice R-0c (0.12.30): the runtime client's workbench methods, the contract wrap-up and `PackScheduleView` (Workflow #429 A.2, A.8 and A.7.1 item 6; the coordinator's decision on #429 UI item 4: SR-11 resolves per package across the layers, and the read view comes now so that the console pins `runtime-client` 3 once; coordinator work order `rd-1029/brief-r0c.md`, with `rd-1029/429-answers-25.md` section 三)

[runtime-interface.md](runtime-interface.md) ("New request kinds", "Workbench request kinds"),
[component-interfaces.md](component-interfaces.md) (`runtime-client` revision 3),
[instance-fact-store.md](instance-fact-store.md) ("Pack schedule facts") and
[ledger-observer.md](ledger-observer.md) §13 are the contract. **No persisted ledger shape**: the
new read records nothing and its refusal records nothing. **No new outcome code**: a deadline out
of range reuses `invalid_contained_task_response_deadline`, the read refused before R-8 the shared
`not_implemented`; six runtime-client locations and one runtime-host location are new (`since`
0.12.30), and the catalog domain `pack_schedule_` gains the owner `runtime-host`. No
configuration key. It adds members to revision 3, so it merges before the 0.12.30 tag. The number
1.216 is the coordinator's (1.213 is R-0's, 1.214 R-15a's, 1.215 UPD-1's).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| R0C-1 | `RuntimeClient::run_instance_task(instance_alias, package_id, response_deadline) -> RuntimeFlowOutput`, `stop_instance_run(instance_alias, request_id, wait) -> StoppedRun {stopped: InstanceRunStop, outcome}`, `edit_pack_schedule(scope, package_id, rows, expected_version?) -> PackScheduleEdited`, `edit_resource_target(instance_alias, expected_version, op) -> ResourceTargetsApplied`, `routine_summary(instance?, granularity, since_revision?) -> RoutineSummaryPage`, `pack_schedule_view(instance_alias) -> PackScheduleView`; `configure_settings(changes, expected_version?) -> ConfiguredSettings {settings, config_version?}`, `apply_resource_targets(document_json, expected_version?)`, `runtime_fact_snapshot_versioned() -> (snapshot, config_version?)` | client methods (Rust API of `runtime-client`; no wire change) | R-0c | — | `expected_version` travels as given, absent when `None`; `run_instance_task` mints the holder on its connection, always names `response_deadline_ms` (1 ms to `ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS`, else `invalid_contained_task_response_deadline` unsent), waits deadline plus the reset's wait (an `Input` `reset`'s) plus IO because the Runtime resets the device before it writes a cancelled run's receipt, answers a completed run's flow, maps a cancelled run by its reason as `submit_prepared` does and never sends `SafeReset` or `CancelContainedTask` (a receipt timeout is returned as it is and latches the connection: the caller reads `Status` and sends `StopInstanceRun` on a new one); the result's `task_request_id` is the request's `request_id`; `stop_instance_run` waits `wait` plus the reset's wait plus IO because the Runtime resets the device before it writes a settled stop's receipt, returns the completed receipt's outcome unchanged beside the stop (a failed reset's code travels there, wherever R-6 puts it) and refuses another run or a `reset` that contradicts `settled`; `edit_resource_target`, `routine_summary` and `pack_schedule_view` refuse an answer for another instance; the two reads report a dropped connection as `runtime_operation_unsupported`; `runtime_fact_snapshot` reads through `runtime_fact_snapshot_versioned` and answers as before. Callers adapted mechanically (`None`, or `.settings`): actingctl `config set|clear`, `agent-apply-resource-targets`, MCP `ac_config_set`, `ac_targets_set`, and two tests |
| R0C-2 | client locations `runtime_run_instance_task`, `runtime_stop_instance_run`, `runtime_edit_pack_schedule`, `runtime_edit_resource_target`, `runtime_routine_summary`, `runtime_pack_schedule_view`; host location `pack_schedule_view` | outcome locations (`since` 0.12.30) | R-0c | runtime-client, runtime-host | the catalog domain `pack_schedule_` lists `policy` and `runtime-host` |
| R0C-3 | `RuntimeOperation::PackScheduleView {instance_alias}` → `RuntimeResult::PackScheduleViewed {view: PackScheduleView {instance_alias, scopes: [{scope, version}], packages: [{package_id, schedulable, source: instance \| server \| game \| pack_default \| none, row?: {from_unix_ms, until_unix_ms?, schedulable}, next_change_unix_ms?}]}}` | new request kind and result (**`runtime-client` revision 3**) | R-0c | R-8 (`pack_default` R-9) | a general read that records nothing (`interface_class` general read, no G14); every level denies unknown members; `version` is each scope's list version, `0` while the scope has no list, the next `EditPackSchedule.expected_version` of that scope; Denied `not_implemented` (`operation` `pack_schedule_view`) recording nothing until R-8; open to every origin like `ResourceTargetView` |
| R0C-4 | R0-4's first writer (`activity`): R-0b in 0.12.31 fills all seven kinds at once, `policy_run` and `direct_run` always with `request_id` and `package_id`, and raises `runtime-client` `min` to 3; `StartReport` is part of revision 3 (R0-7, §3 W21, §6), as W5 is; ledger-observer §13's receipts of gate-class operations take a later revision (W6); in codes mode `fact_observation_version_conflict` keeps its `current_version` value, as `host_halted` travels (R-4) | contract wording | R-0c | R-0b, R-4 | `StartReport` reads only memory, has served since v0.12.19, and the console reads its `instructions_dropped` |
| R0C-5 | SR-11 resolves per package: instance over server over game over the pack's default row, each package on its own; a layer without a row of a package decides nothing for it; `EditPackSchedule` replaces only that package's rows in its scope | contract wording ([instance-fact-store.md](instance-fact-store.md), "Pack schedule facts") | R-0c | R-8 | replaces "an instance record replaces the server and game records as a whole"; SR-11 had no code, so nothing migrates; Alice's order "instance > server > game" is unchanged |

### 1.217 Slice R-0d (0.12.30): the layer below in `PackScheduleView` (Workflow #429 UI item 5, `rd-1029/ui-check-13.md` item 5: with only the deciding layer visible, turning a package back on takes withdraw, read again, write; coordinator work order `rd-1029/brief-r0d.md`)

[runtime-interface.md](runtime-interface.md) ("Workbench request kinds", the `PackScheduleView`
row), [component-interfaces.md](component-interfaces.md) (`runtime-client` revision 3) and
[instance-fact-store.md](instance-fact-store.md) ("Pack schedule facts") are the contract. **No
persisted ledger shape**, no outcome code, no configuration key, no new interface revision: one
optional answer member joins R0C-3's result in revision 3 before the 0.12.30 tag. The
`runtime-client` method `pack_schedule_view` passes it through unchanged. The number 1.217 is the
coordinator's (1.215 is UPD-1's, 1.216 R-0c's).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| R0D-1 | `PackSchedulePackage.below?: PackScheduleBelow {schedulable, source: server \| game \| pack_default \| none}` in R0C-3's `PackScheduleViewed` | new optional answer member (**`runtime-client` revision 3**) | R-0d | R-8 (`pack_default` R-9) | the package's decision now if the deciding layer had no row of it: SR-11 resolved again without that layer's rows of the package, ending at the pack default or `none`; never `instance`; absent when `source` is `pack_default` or `none`, so such a package serializes as under R-0c; both levels deny unknown members; Runtime 0.12.30 refuses the read `not_implemented`, so no 0.12.30 answer carries it and `min` stays 2; a console built before R-0d refuses a view that carries it, so the console pins a commit with it |

### 1.218 Slice SS-6 (0.12.31): a dispatch hands its run the spend allowance, the remaining gap and a run cap (Alice's ruling 100, #386 comment 6490879952: dispatch follows live account state, there is no "run task X N times"; ruling 106 of 10-29 20:3x: do the highest-weight consuming task, time joins the model later, until then the reserve line is the backstop; design `rd-1029/design-sch-state.md` §4.3 and the SS-6 entry of §5; coordinator work order `rd-1029/brief-ss6.md`)

[remedies.md](remedies.md) (section 4, "Spend values"; section 12) is the contract;
[scheduling/v2/README.md](scheduling/v2/README.md), [scheduling/README.md](scheduling/README.md)
("Task Reserves"), [instance-fact-store.md](instance-fact-store.md) ("Run parameters") and
[selection-policy.md](selection-policy.md) (the in-task consumer) name it. The Runtime can work
out how much may be spent and how much is still missing, the package can read the unit price
and the dialog's cap on its screen; so the Runtime hands its part to the package at dispatch, on
Workflow #411 B8's `session.dispatch.schedule.*` channel (§1.150), with no opt-in. B8's
`daily_remaining` and `last_daily_slot` are unchanged; counts retire in SS-9. **No new persisted
ledger shape**: the values are new optional values of the pending entry
`policy_dispatch_parameters` in the existing dispatch reason chain of `policy.dispatch_intent`
(`PolicyReasonRecord` is `#[serde(transparent)]` over `Outcome`, whose `values` every 0.12 reader
keeps as a map without a key check), and the records `session.dispatch.schedule.<key>` are an
overlay of the run's instance fact snapshots, never stored. No catalog member, no package format
change, no configuration key, no outcome code, no wire or interface revision. One-way: the values
come from the pools' state (the reserve gate's projection, the resolved goals) and the task's own
`consumes`, `produces` and `reserves`; nothing reads a task's enablement, override, priority,
offset or run state. Game-neutral. The number 1.218 is the coordinator's. Review round 2 (the
coordinator's decisions on PR #1446): the recording is B8's typed values, the 64-record limit is
documented, a met goal and a task without a reserved consumed pool give no cap (as built); the
allowance is the reserve gate's own available amount, its step 3 included; `run_cap` is at most
1000; the goal term applies only when no produced pool carries a base weight and is the largest
need over the open gaps.

**Recorded bytes.** A selected dispatch of a task that reserves a pool it consumes (its allowance
is then always known and at least one run: the gate holds the candidate otherwise), or that
produces a pool an active v1 or v2 goal covers, now carries `policy_dispatch_parameters` in its
`policy.dispatch_intent` chain: a new reason when it carried none (no package parameter, no B8
opt-in), or the same reason with `spend_allowances`, `gaps_remaining` and `run_cap` added (its
developer detail rebuilt as the outcome's text, as B8 does). Every other dispatch, an opted-in B8
task's included, carries the bytes of before. In production no task declares `reserves` and none
produces toward a goal yet (`rd-1029/design-sch-state.md` §2.1; RS-2 of the plan writes them), so
nothing changes live until the resource side writes them.

**Rollback.** Runtime 0.12.30 reads the chains 0.12.31 writes: the reason decodes as every
`policy_dispatch_parameters` does, unknown values kept and never checked. A 0.12.30 host resuming
such a dispatch overlays only B8's keys, so a package that reads an SS-6 key meets an unknown fact
(`max_repeats` ends the run with `contained_task_designated_effect_cap_unresolved` before any
input). Rolling back below 0.12.31 is therefore possible; only the packages that read the new keys
lose them. The gate itself is unchanged: its step 3 now reads its pending consumption through the
helper the allowance shares (`resource_targets::pending_consumption`), with the same entries,
order and reasons.

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| SS6-1 | `policy_dispatch_parameters` values `spend_allowances` (records `{effect_index, spend_allowance}`), `gaps_remaining` (records `{effect_index, gap_remaining}`) and `run_cap` (integer); keys `run_cap`, `effect_index`, `spend_allowance`, `gap_remaining` (integers), `spend_allowances`, `gaps_remaining` (records) | new optional values of a pending entry and new key-table entries (no shape change) | SS-6 | the policy evaluator | on every selected dispatch, with or without remedies and B8's opt-in, merged into that reason or alone, each only when known: per `consumes` entry `i` whose pool the task reserves, the reserve gate's own available amount (the projection less the declared consumption of the instance's other tasks dispatched since the pool's latest reading, the gate's step 3) less `at_least`, at least 0 (absent while unknown), so the package is never told it may spend below the reserve line; per `produces` entry `j`, the gap of the active goal on its pool covering the task (v2: as its shortfall weight covers; v1: the target naming the task), 0 when met, absent without an active goal or while unknown; `run_cap` = the spend term, min over reserved consumed pools of ⌊allowance / declared per-run consumption⌋, and, only when no produced pool carries a base weight, min with the goal term, the **largest** ⌈gap / declared per-run `amount`⌉ over the produced pools with an open gap (gaps of 0 add nothing; with none left, e.g. every goal met, no goal term: ruling 102); then at most 1000, the `max_repeats` fact-cap ceiling (`MAX_DESIGNATED_EFFECT_REPEATS`), so a package reading it never meets that cap's range error; no `run_cap` without a reserved consumed pool, with an unknown projection, or when a goal-driven produced pool with an open gap is declared by `expected_amount_milli`; saturating integers, allowances and gaps recorded at most `i64::MAX`; a record list keeps 64 records (`<key>_total` beyond) |
| SS6-2 | the overlay records `session.dispatch.schedule.spend_allowance.<i>`, `session.dispatch.schedule.gap_remaining.<j>`, `session.dispatch.schedule.run_cap` (`integer`) | kernel overlay of a policy run's snapshots (memory only, not persisted) | SS-6 | runtime-host (`fact_refresh::dispatch_schedule_values`) | after B8's values, in record order; the longest key is 45 bytes (`…spend_allowance.127`, at most 128 entries per list), within the 128-byte fact key limit of a selection-policy document and an outcome key (the instance fact store allows 192); indices keep pool ids out of the keys; a package reads them as B8's: `max_repeats: {"fact_key": "session.dispatch.schedule.run_cap"}` (an integer from 0 to 1,000, which `run_cap` always is; a missing key ends the run with the existing named error before any input) or an integer fact of a selection strategy |
| SS6-3 | `ReserveView.projected`; `resource_targets::pending_consumption` and `pending_total` (the gate's step 3 entries, now shared); `InstanceTargets::goal_gap`; v1 `TargetEffect::Satisfied.resource`; the evaluator computes the pending consumption once (`reserve_unshown`) for the gate and the allowances | policy internals (memory only) | SS-6 | policy crate | the allowance reads the projection the gate's `after` is taken from and the pending consumption its step 3 takes, at the same instant, so an allowance below the declared consumption, 0 included, is a candidate the gate already holds; no reason or record of the gate changes |

### 1.219 Slice R-1 (0.12.31): a key the configuration file does not write can be set online (Workflow #429 A.6 rule 4, A.8's R-1 row and the R-1 entry of `rd-1029/429-A-runtime.md`: the console sets everything that may change at run time through `ConfigureSettings`, never through the file; coordinator work order `rd-1029/brief-r1.md`)

[runtime-config.md](runtime-config.md) ("`ConfigureSettings`", "Keys the file does not write",
"Restore at start", "Rollback") and [scheduling-blackout.md](scheduling-blackout.md) are the
contract. **No new persisted ledger shape**: the rows are `host.config_overrides` rows of the
existing shape (D1b), whose `base_json` is the JSON text `null` for a key the file did not write.
No outcome code, no configuration key, no `actingd-config` or other interface revision (the file
format and the wire are unchanged). Only the barrier "the file does not write the key" goes; each
key's kind stays as its row says. This supersedes D53's "an instruction can change it only then
(`config_key_unknown` otherwise)" for `scheduling.fact_refresh.overdue_ms`. The number 1.219 is
the coordinator's.

**Rollback.** Runtime 0.12.30 reads the new rows with its row parser (unchanged since then) and
drops each, `config_override_dropped`, listed in `StartReport.instructions_dropped`, because its
configuration has no setting of a key the file does not write; it never refuses the record, so
its start does not fail. The installer's post-start `instructions` check lists those keys as
dropped, as it lists any instruction a rollback target cannot keep (A.6 rule 5: a rollback
resets them and the console says so); an update from 0.12.30 meets no such row, since 0.12.30
never writes one. Tested on a fixed sample of the rows (`runtime-config` `r1_tests::r1_t6`).

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| R1-1 | `ConfigureSettings` of a key whose default is `none` (`DefaultSource::None`) that the file does not write: accepted once its instance (a per-instance key) or its section (an exact key) is in the configuration; the key's own checks unchanged (type, range, whole-section check, `config_key_not_settable` for a key that is not settable); the row records `base_source` `default`, `base_json` `null`; the key then has a `config.parameters` row (source `instruction`); a change without a value returns it to no value, no row, and the receipt reports `{key, value: "null" (string), source: default}` | behaviour (configuration instruction layer) | R-1 | runtime-config (`EffectiveConfig::apply`) | before, `config_key_unknown`; settable keys with no default: the per-instance `scheduling.blackout` and `recovery_ladder_profile`, and `scheduling.fact_refresh.overdue_ms`; `performance.pacing.max_concurrent_runs` is read once at assembly (`Settable::No(ReadAtAssembly)`), so a file-silent instruction naming it is now `config_key_not_settable` (was `config_key_unknown`), as it already was with the file naming it; R-1 does not make it settable; `standard_packs.root` and the per-instance `device_kind`, `recognition_package`, `start_emulator` likewise; the receipt member keeps the wire shape (`ConfiguredSetting.value` is a scalar, and a receipt's `settings` is never empty) |
| R1-2 | restore at start: a row over a key the file did not write (`default`, `null`) is kept while the file still does not write the key and superseded (`config_override_superseded`) once it does; a row over an explicit value of a key with no default that the file no longer writes is superseded | behaviour (restore rule) | R-1 | runtime-config (`EffectiveConfig::restore`) | before, a key with no default that the file did not write had no setting, so every such row was `config_override_dropped`; a key whose instance or section is gone is still dropped; the installer's instructions check counts dropped and superseded alike |

## 2 Removals and changes of option 丙 (M-PKG §4.3, §4.4)

Deleted outright, no old-version reader (M-PKG §7). The slice named deletes the field and its
readers in one PR.

| Slice | Removed or changed |
|---|---|
| PKG-S2 | `PackageAdmitted.package_sha256` (keep `package_label`, `task_label`); `EntryRecoveryPackageAdmitted/Completed/Failed.package_sha256` → `package_label`; effective configuration `EntryRecovery.package_sha256` → `package_label`; `TaskSelectionPolicy.package_sha256`; Lab declared refusal `declared_package`, `verified_package` → optional `package_label`; Lab evidence `expected_package_sha256` / `actual_package_sha256` → `package_label`; `EvidencePackage.sha256`; OCR comparison report `truth_set_sha256` and its runtime-client reader; run status `package_ref` / `recovery_packages` → `package_label` / `recovery_package_labels`; run summary digest checks; reading facts' `resource_bundle_hash` = SHA-256(`"package:" + package_id`) (shape unchanged). **Done (PKG-S2):** all of them, §1.31 P2-1 to P2-11; also the run summary's `package_digest` (now `package_label`), the in-package `sha256` keys made optional and ignored (D5), and request replay by `package_label` (a recorded label must be the request's package's, or a configured prerequisite package's or the recovery binding's `package_id`, each admitted again) |
| PKG-S3 | `DispatchIntent.package_digest` (payload and storage equality); `ProcedureBinding.package_digest`; `binding_digest` tuple v3 = (`procedure_ref`, `operation_id`, `yield_points`); `PausedOn.package_digest` → content hashes parsed from the failure identity; suspension report members `paused_hash_prefix`, `current_hash_prefix`, `lifts_when: content_hash_changes`, `lifted_by: main_hash \| layer:<n> \| restart`. **Done (PKG-S3):** all of them, §1.47 P3-1 to P3-6; also the run identity check by path (the request's path against the binding's location) and `lifted[].effective` `next_evaluation` in place of `pending_restart` |
| PKG-S4 | IPC `expected_sha256` of `PackageDebugRequest`, `ContainedTaskRecoveryBinding`, `ContainedTaskRequest`, the Lab operation, online observation and saved-artifact OCR requests; `PackageDebugSummary.verified_sha256`; configuration `startup_package.expected_sha256`, `prerequisite_packages[].package_digest`, `policy.procedure_manifest[].package_digest`, the resource declaration's `package_digest`. **Done (PKG-S4):** all of them, §1.55 P4-1 to P4-6; also the actingctl, MCP and actinglab digest flags and arguments, the `takeover` hint's `--package-ref`, the `package validate` / `lab validate` digest outputs, the dry-run record's `package_label` and the identity check of a configured prerequisite package (D7) |
| PKG-S5 | bundle v4 `packs[] {package_id, server, entry_task_id, path = "packs/<package_id>", file_count, byte_count}` (the bundle-level maintenance declaration stays, coordinator 10-12); `RecordingArtifact.digest` / `.sha256` → `package_id`; `PackageRef`, `ContentDirectory` and the `sha256:` serializers. **Done (PKG-S5):** all of them, §1.64 P5-1 to P5-8; also bundle v4's optional `tables` (M-357a TB12) and its reserved `package_id` `tables`, the Lab output named `<package_id>.<ext>` and replaced in place (D9), `actinglab package digest` and the authoring receipt's `package_sha256` |

**Failure identity, format v2** (PKG-S3, M-PKG §3.4):
`<base>~v2~k<K>~m<I><H>[~p<I><H>]…[~r<Kh><I><H>]~f<F>`. `I` = the first 8 hex digits of the
SHA-256 of `package_id`; `H` = the first 12 hex digits of the package's content hash measured at
suspension (`x` when it could not be measured, D18); the `pre:<digest>` scope becomes `pre:<I>`.
A v1 string is not read. The content hash is measured only at suspension and at lift checks
(never on admission) and is stored only inside this string (the failure record's
`failure_identity`, §1.15 Q1) and in SR-3's suspension event key
`actual_digest`. A lift check of one package runs at most once per
`scheduling.suspension.lift_check_min_interval_ms` (§5).

## 3 Wire shapes handed to AO-0 (runtime-client revisions)

| # | Shape | Revision | Slice | Notes |
|---|---|---|---|---|
| W1 | `RuntimeStateSource` = `{instance_facts? {lineage, revision}, runtime_facts? {lineage, revision}, ledger_committed_head, sampled_started_at_unix_ms, sampled_completed_at_unix_ms}` | runtime-client 2 | AO-4q | replaces `{event_id, sequence}`; store fields absent before AO-3 and readers accept that; status queries stop appending records; **done (AO-4q):** each store field is a `RuntimeStateStoreRevision {lineage (owner epoch), revision}`; `Status`, `MonitorStatus`, `ProjectInterface` and `DiscoverInstances` append nothing on success and read the committed head inside the sampling interval ([runtime-state-observation.md](runtime-state-observation.md)); a discovery refusal still records its refusal |
| W2 | `RuntimeOperation::InstanceFactSnapshot { instance }` → `RuntimeResult::InstanceFactSnapshot { snapshot, source }` | runtime-client 2 | MCP-4 | general read, one instance's facts with observation and expiry times and the resolved scope; a new kind, not a parameter of `RuntimeFactSnapshot` (that kind stays field-less); U-1's classification table gains the row; **done (MCP-4):** `instance` is the registered instance alias; `snapshot` is an `InstanceFactView {instance_alias, server_id?, game_id?, records}`: the active records of the instance's own scope and of the server and game its configuration binds it to (`server_id` and `game_id` both absent when it binds none, and then only the instance's own records), ordered by key, then scope; the host projection's `ledger_position` and `snapshot_id` never go on the wire (the view is built from the store's records; `contracts/ledger-observer.md` §13); `source` is W1's `RuntimeStateSource` sampled around the read; nothing is appended (U-1 row: general read, no record); a reply past one IPC frame is refused `instance_fact_snapshot_too_large_for_frame`; a completed receipt carries `request_instance_fact_snapshot_completed` |
| W3 | `ConfigureSettings { changes: [{key, value?}] }` and its receipt `{key, value, source}` | runtime-client 2 | D1b | M387 §5.6 |
| W4 | `PerformanceYield`, `BlackoutStop` | runtime-client 2 | SH-1 | P7, S2 |
| W5 | read request `ScheduleExplanation { instance }` | runtime-client 3 | MCP-19 | reserved; per task: trigger, last dispatch, next due, not-run reason, produces, consumes, pools; **served (MCP-19a, §1.143) under runtime-client 2**: `ScheduleExplanation { instance?, candidates }` answers per instance the latest evaluation's not-running row, selected decision and candidates that did not run (why, reason codes, wake, since); only actingctl and MCP of the same Runtime send it and every revision-2 shape is unchanged, so revision 3 includes it; the trigger, last dispatch, produces, consumes and pools of the reserved row come later (MCP-19c) |
| W6 | gate-class receipts carry `event_id` and store revisions, never a ledger sequence; install receipts after the commit | a later revision (not 3; §1.213 R0-7) | AO-4 | [ledger-observer.md §13](ledger-observer.md#13-identity-without-ledger-positions-ao-3-ao-4-ao-4q-in-v0120) |
| W7 | status blocks `ledger`, `tagged_state`; `startup`, `host` | a later revision (not 3; §1.213 R0-7) | AO-8; MCP-9b (`startup`, `host`) | [ledger-observer.md §14.2](ledger-observer.md#142-status-blocks); `startup` and `host` (MCP-9b) are held in memory and leave through the same status read as AO-8's blocks (re-check 10-14, §1.25) |
| W8 | request `actingcommand.runtime.request.v4`: optional `detail` (`codes` \| `full`) | runtime-client 2 | A4 | #378 §7.1; absent, the source's class default applies (codes only for `source: ui`, the full outcome for every other source); requests v1–v3 are refused; [client-interactions.md](client-interactions.md) |
| W9 | receipt `actingcommand.runtime.receipt.v2`: `outcome` (an `actingcommand.outcome.v1` envelope) on every receipt; the v1 member `error` (`RuntimeErrorProjection`) is removed | runtime-client 2 | A4 | #378 §7.2; admitted, observed, queued and cancelled carry `request_admitted`, `request_observed`, `request_queued`, `request_cancelled`; completed carries its kind's `request_<kind>_completed`; denied and failed carry the refusal or failure on top, with the values `runtime_code`, `operation`, `host_halted`, `holder_id`, `lease_id`, `retry_after_ms` where they apply; `host_halted` travels in codes mode too; a consumer judges refusal by `state` (denied/failed), the category only says how bad it is; codes mode is applied where the receipt leaves the host |
| W10 | `RuntimeResult::ContainedTaskCompleted.outcome` → `task_result`; `ContainedTaskCancellationStatus::Terminal.outcome` → `task_result` | runtime-client 2 | A4 | #378 §8.1: renames in receipt v2 (same value), because `outcome` is reserved for the envelope |
| W11 | request kinds `BindChannel`, `UpgradeState`, `UpgradeStateStatus`, `LoadPack`, `UnloadPack` | runtime-client 2 | U-1 | UIM §8.1; their success codes come with the behaviour (U-2, U-3, U-4); **done (U-2):** `BindChannel` answers `ChannelBound` with `request_bind_channel_completed` (§1.46 U2-3) |
| W12 | `RuntimeCaptureBackend::Scrcpy`, spelled `scrcpy`: `ReadonlyObservation.capture_backend` (read-only observation results and `CaptureSequence.observations`) | runtime-client 2 | #402 RD-3 | new enum value (R1 §5.1); the Lab maps it one to one to `CaptureBackendName::Scrcpy` (C1); W8–W11 are A4's (#718) |
| W13 | `RuntimeInstanceStatus.not_running` | a later revision (not 3; §1.213 R0-7) | SR-6b | the per-instance not-run rows; they live in the workbench's memory only, and AO-4q (#746) stopped persisting the status (status reads append nothing), so the member is a wire shape, not a ledger one (re-check 10-14, §1.25); not built in 0.12.x: a status member breaks the deployed UI; the rows are W5's (§1.143) |
| W14 | `EventQuery.event_types` (a set of event types, OR within it) and `EventQuery.fact_key` (an instance or runtime fact key) | runtime-client 2 | MCP-2 | two optional members of the `QueryEvents` query (`deny_unknown_fields`), absent when unused, so a query without them serializes, and fingerprints its cursor, as before; `event_types` is the alternative of `event_type` (at most 32, each once, none excluded, else `invalid_event_query_event_type_filter`); `fact_key` keeps only the records that name it (`fact.published`, `fact.invalidated`, `runtime.fact_recorded`, `runtime.fact_invalidated`), else `invalid_event_query_fact_key_filter`; [global-ledger-query.md](global-ledger-query.md) |
| W15 | `RuntimeOperation::PerformanceStatus` → `RuntimeResult::PerformanceStatus { status }` | runtime-client 2 | P-m | the shadow measurement's status read; **done (P-m):** §1.61 W15, with its read output W16 |
| W17 | `RuntimeOperation::FactRefreshStatus` → `RuntimeResult::FactRefreshStatus { status }` | runtime-client 2 | D2-S5 | the D2 S5 transition query (UIM §2.3); **done (D2-S5):** §1.68 D51, with its read output D52 (W18) |
| W19 | `RuntimeEventQueryPageRequest.without_run_recovery` (a boolean, `since` 0.12.23) | runtime-client 2 | #422 QR-2 | one optional member of the `QueryEvents` page request (`deny_unknown_fields`), absent (`false`) when unused, so a request without it serializes, and its page answers, as before; `true`: the page derives no `run_recovery` group and reads no run's context for one, its rows, views, cursor and read scope its own; no outcome code; sent by `runtime-client`'s run listing and run status reads, and by every page of `actingctl diagnose` (#422 QR-2b, since 0.12.24); [global-ledger-query.md](global-ledger-query.md), [component-interfaces.md](component-interfaces.md) (`runtime-client` revision 2) |
| W20 | `RuntimePerformanceStatus.ledger_read_pages` = `LedgerReadPagesStatus {pages, total_ms, longest_ms, last_ms}` (`since` 0.12.25) | runtime-client 2 | #422 W1-8 (P29) | one optional member of W15's status (`deny_unknown_fields`), absent when the Runtime's ledger has no reader pool: the view pages the pool read since the Runtime started (a page it answered without reading is not counted), their summed, longest and last time in milliseconds, read from the ledger's memory, no writer command; a wrong shape (times out of order, or times without a page) is `invalid_performance_status`; no outcome code; `actingctl perf-status` and `ac_overview`'s `performance` block show it as it is; on PerformanceStatus, not on Status, because a status member breaks the deployed UI (W13) |
| W21 | the player workbench's request kinds, results and members (§1.213 R0-1 to R0-4, R0-8, R0-10; §1.216 R0C-3, `PackScheduleView`), and the read `StartReport` (§1.216 R0C-4) | runtime-client 3 | #429 R-0, R-0c | declared [2, 3] in 0.12.30; each kind answers `not_implemented` until its slice; `RuntimeInstanceStatus.activity` is the status member W13 warned about: it and R0-8's answer members stay absent until a release raises `min` to 3 with the console that reads them |

U-1 owns the shapes of `BindChannel`, `UpgradeState`, `UpgradeStateStatus`, `LoadPack` and
`UnloadPack` (W11; UIM §8.1; LoadPack and UnloadPack without any digest, M-PKG §5.1).

## 4 Fact keys new in the 0.12 series and their tag class

| Key | Store | Tagged | Writer slice |
|---|---|---|---|
| `health.perf_baseline.<signal>.<backend>` (per instance, one per backend in use) | instance | yes (every instance key) | P-m |
| `health.run_duration` (per instance, one record of at most 256 package rows; [instance-fact-store.md](instance-fact-store.md), "Run duration facts"; §1.190 ED2-1) | instance | yes (every instance key) | SCH-M ED-2 |
| `session.pack.schedule` (one per scope) | instance | yes | SR-11 |
| `session.feature.<feature_key>` | instance | yes | SR-5a |
| `session.instructions` (reserved, one per scope) | instance | yes | GOAL-1 |
| `host.perf_baseline.<signal>` | workbench | yes (`tag.workbench_patterns`) | P-m |
| `host.perf_calibration.stats`, `.curve`, `.phase` | workbench | yes | P-m (`stats`), P-a |
| `host.config_overrides` | workbench | yes (added to `tag.workbench_patterns` by M387 §5.7) | D1b |
| `host.contained_settlement` (an integer: the start event of the last start that completed the settlement of open direct runs; [runtime-fact-store.md](runtime-fact-store.md)) | workbench | yes (added to `tag.workbench_patterns` by #411 A65) | #411 A65 |
| `host.idle_state` (a record list: the instances the idle action closed or is closing, with their action and wake; since 0.12.25 its writer row names the start anchor, not the owner epoch (#405 N-2), and a woken instance stays in it, `woken`, until its startup package has run (#405 O-2); [runtime-fact-store.md](runtime-fact-store.md)) | workbench | yes (added to `tag.workbench_patterns` by #405 IDLE-1P) | #405 IDLE-1P |
| `config.selection`, `config.catalog` | workbench | no (`config.` is re-observed) | D1b, MCP-9a |
| `config.parameters.touch` (a section fact of the `config.parameters.<section>` family, five rows; §1.136 T12-4) | workbench | no (`config.` is re-observed) | #406 T12 |
| `config.parameters.emulator_control` (a section fact of the `config.parameters.<section>` family, one row; §1.158 A50-5) | workbench | no (`config.` is re-observed) | #411 A50 |
| `config.parameters.execution` (a section fact of the `config.parameters.<section>` family, three rows; §1.177 B1-2, B1-3) | workbench | no (`config.` is re-observed) | PERF-B1 |
| `host.pack_registry.*` | workbench | no (added to `tag.reobserve_prefixes`, UIM §6.5) | U-3 |
| `display.frame_extent`, `display.rotation`, `display.orientation`, `display.aspect` (per instance, only for an instance bound to a device; [instance-fact-store.md](instance-fact-store.md#display-facts)) | instance | yes (every instance key) | DA-A |
| `display.logical_extent` (one row `{width, height}`), `display.safe_area.reported` (one row `{left, top, right, bottom}`, device-frame pixels), `display.safe_area.declared` (the same row), `display.map` (one row, model 392 §8.1) (per instance; §1.25 X10) | instance | yes (every instance key) | DA-4 (`display.safe_area.declared`: a person or an agent) |
| `host.kept_folder_cleared` (re-check 10-14, §1.25) | workbench | no (`tag.reobserve_prefixes`) | 375-R5e-clear (U-3) |
| `device.link` (per instance, a bound handset only; §1.52 RD2-2) | instance | no (`device.` is in `tag.reobserve_prefixes`) | RD-2 |
| `device.screen`, `device.keyguard`, `device.battery`, `device.thermal`, `device.screen_off_timeout_ms` (per instance, a bound handset only, with a lifetime; §1.59 RD5-2) | instance | no (`device.` is in `tag.reobserve_prefixes`) | RD-5b |
| `session.remedy.choices` (one per scope: instance, server or game; §1.110 B15-6) | instance | yes | #411 B15 R6 |
| `session.dispatch.parameter.<name>` (an overlay of a run's snapshots only, never stored; the prefix `session.dispatch.` is refused to publishers; §1.110 B15-7) | — (never stored) | — | #411 B15 R5 |

The capacity of the instance store is booked in
[Instance fact store](instance-fact-store.md#capacity-ledger-workflow-383-ao-0).

## 5 Configuration keys of `actingd-config` revision 5

One revision for v0.12.0, declared [5, 5] by #378 A8 (ORD §3.6). Every key designed by the frozen models is
parsed, validated and defaulted in v0.12.0 even when its reader comes in 0.12.x (ORD R4); the
defaults live in `distribution/windows/actingd.defaults.json` (#387 D1b), never in the example
configuration and never as code constants; each parsing slice brings defaults and parsing in the
same PR (ORD R3). "Parsed by" is the slice that adds the key to the configuration; "read by" is
when it takes effect.

| Section | Keys | Defaults and meaning | Parsed by | Read by (takes effect) |
|---|---|---|---|---|
| `ledger_observer.*` | the 21 keys of [ledger-observer.md §15](ledger-observer.md#15-configuration-keys-ledger_observer); #411 A51 (**`actingd-config` revision 8**): `read_budget_ms`, `close_timeout_ms`; P0-LO (**`actingd-config` revision 10**): `open_timeout_ms`, `open_warn_ratio_milli`; #422 LN-5 (**`actingd-config` revision 12**): `history_page_rows`; #422 LN-TUNE (**`actingd-config` revision 12**): `writer_reply_timeout_ms`; #422 LN-8 (**`actingd-config` revision 13**): `open_verify_tail_records`; #422 LN-9 (**`actingd-config` revision 13**): `scrub_rows_per_second`, `scrub_chunk_rows`, `scrub_pass_interval_ms`; #422 LN-WS (**`actingd-config` revision 14**): `read_check_vm_steps`, `history_scan_rows`; #422 LN-7c Q (**`actingd-config` revision 16**): `frame_query_steps_per_second`, `frame_query_chunk_frames` | there; A51: 1 000 (`1..=9999`, below `writer_reply_timeout_ms`), 60 000 (above `read_budget_ms`), not settable (§1.151 A51-3); P0-LO: 600 000 (`10000..=600000`), 500 (`100..=1000`), not settable; LN-5: 256 (`1..=4096`), not settable; LN-TUNE: 10 000 (`1000..=600000`), not settable; LN-8: 4 096 (`1..=1000000`), not settable; LN-9: 625 (`1..=100000`, settable), 256 (`1..=4096`), 2 400 000 (`60000..=86400000`), the last two not settable; LN-WS: 10 000 (`100..=1000000`), 65 536 (`1024..=1048576`), not settable (§1.194); LN-7c Q: 500 000 (`1000..=100000000`, settable), 256 (`1..=4096`, not settable) (§1.196) | AO-1 (built, §1.26); A51 (§1.151); P0-LO; LN-5; LN-TUNE; LN-8; LN-9; LN-WS; LN-7c Q | AO-1, AO-2 (v0.12.0); AO-5, AO-8 (0.12.x); A51 (0.12.9); P0-LO (0.12.11); LN-5 (0.12.13); LN-TUNE (0.12.13); LN-8 (0.12.14); LN-9 (0.12.14); LN-WS (0.12.15); LN-7c Q (0.12.17) |
| `recovery_ladder.*` | `enabled`, `cooldown_secs`, `trigger_defaults.start`, `.start_rung`, `.respect_cooldown`, `triggers`, `exclude_codes`, `rules.direct_runs`, `.preparation_superseded`, `.already_running`, `.cooldown_opens_on`, `.entry_unavailable`, `rungs`, `hold.claim_ttl_ms`, `hold.margin_ms`; Workflow #398 RD-4: `rules.start_beyond_profile`, `profiles`, `kind_profiles`; #411 A38 (**`actingd-config` revision 8**): `fence_recheck.interval_ms`, `fence_recheck.max_attempts`; #411 B37 (A35-S1, revision 8): `busy.poll_ms`, `busy.outcome.recheck_ms`, `.max_wait_ms`, `.on_exhausted`; #411 A35 (A35-S2, **`actingd-config` revision 9**): `busy.screen.classes`, `.probe_frames`, `.probe_interval_ms`, `.change_max_milli`, `.recheck_ms`, `.max_wait_ms`, `.on_exhausted` | M387 §7.1; model-398 §6.3, §6.6 (`exhaust`, the `handset` profile of return home and application restart for the kind `handset`); A38: 300 000 (`10000..=86400000`), 12 (`0..=1000`), settable (§1.148 A38-6); A35-S1: 500, 60 000, 3 600 000, `stop`, settable (§1.152 B37-4); A35-S2: `["pack_logic_mismatch"]`, 5, 1 000, 1 500, 15 000, 900 000, `escalate`, settable (§1.168 A35-4) | D1c; RD-4 (§1.81); A38 (§1.148); A35-S1 (§1.152); A35-S2 (§1.168) | D1c (v0.12.0); RD-4 (v0.12.0); A38 (0.12.9); A35-S1 (0.12.x); A35-S2 (0.12.x) |
| `host_claims.*` | `renewal_slack_ms` | 1 000 | D1c | D1c |
| per instance | `stuck_recovery`, `stuck_recovery_cooldown_secs` (existing keys, defaults now from the section) | M387 §5.5 | D1b | D1c |
| per instance | `recovery_ladder_profile` | model-398 §6.2: default `none` (the kind table, then the section's `rungs`); must name a profile; settable | RD-4 (§1.81) | RD-4 (v0.12.0) |
| `performance.monitor.*`, `performance.pressure.*`, `performance.load.*` | PERF §10.1 (15 keys; the three existing `performance_monitor` keys keep their file paths) | PERF §10.1 | P-c (built, §1.24) | P-m `gpu_aggregate`; PM-CFG every other key (v0.12.0, §1.87) |
| `performance.responsiveness.*` | PERF §10.2 (37 keys) | PERF §10.2 | P-c (built, §1.24) | P-m (v0.12.0) |
| `performance.pacing.*` | PERF §10.3 (26 keys; `mode` stays `observe` in every release until Alice approves); Workflow #427-B (**`actingd-config` revision 19**): the optional `max_concurrent_runs` | PERF §10.3; #427-B: no default (unset, no limit), `1..=65535`, not settable (§1.199 427B-4) | P-c (built, §1.24); #427-B | P-k, P-s, P-h (0.12.x); `clock_jump_ms`, `heavy_dispatch_limit.*` P-c; `max_concurrent_runs` #427-B |
| `performance.calibration.*` | PERF §10.4 (19 keys) | PERF §10.4 | P-c (built, §1.24) | P-a (0.12.x) |
| `execution.defaults.*` | the kernel's fallback defaults (PERF §10.4 last notes) | today's constants | **not built:** P-c left it out (PR #748 deviation 5, "after SR-1"), and no later slice parses it, so it is not part of revision 5 (open for the coordinator, A8 report) | — (the kernel's fallbacks stay code constants in v0.12.0) |
| `execution.capture.*` | `max_interval_ms`, `backoff_enabled`, `idle_ceiling_ms` (**`actingd-config` revision 11**) | 60 000 (`5000..=1800000`), `true`, 5 000 (`50..=60000`), settable (§1.177 B1-2, B1-3) | PERF-B1 | PERF-B1 (0.12.x) |
| `scheduling.recognition.*` | `sample_timeout_retries` | 2 | SR-1 (built, §1.23) | SR-1 (v0.12.0) |
| `scheduling.rerun.*`, `scheduling.cost_commit.*`, `scheduling.blackout.*`, `scheduling.feature_lock.*`, `scheduling.not_run.*` | SCHED §9 (17 keys) | SCHED §9 | SR-1 (built, §1.23) | SR-2b, SR-4a, SR-5a, SR-10 (0.12.x); `not_run.diagnose_rows` MCP-19b |
| `scheduling.expected_runtime.*` and per instance `instance.<id>.scheduling.expected_runtime.*`; `scheduling.fact_refresh.expected_duration_ms` (`actingd-config` revision 11) | `enabled`, `runs`, `margin_ms`; the declared duration of a fact refresh read | `true`, 5 (`1..=16`), 300 000 (`0..=3600000`); 60 000 (`1..=1800000`); an instance's default is the section's (SCH-M §3.4) | ED-1 (built, §1.178) | ED-2 |
| `scheduling.blackout.quiet_horizon`, `scheduling.day_boundary.quiet`, and per instance `instance.<id>.scheduling.blackout`, `instance.<id>.scheduling.day_boundary.quiet` (`actingd-config` revision 11) | SCH-M §3.2 D-1, D-4, D-6 | `deadline`, `true`; no instance list by default; an instance's `day_boundary.quiet` defaults to the section's | SR-4a1 (built, §1.179) | SR-4a2 |
| `scheduling.random_delay.*`, and per instance `instance.<id>.scheduling.random_delay.*` | `enabled`, `min_ms`, `max_ms` | `false`, 60 000, 3 600 000; `0 <= min_ms <= max_ms <= 86400000` on the pair in effect; an instance's default is the section's (#405, coordinator's order of 2026-10-14) | SR-1 (built, §1.23) | #405 D1 (0.12.5, §1.135): the pair in effect per instance, read at every policy cycle |
| `scheduling.pack_admission.*` | `check_in_check_config`, `check_after_start`, `suspend_codes` | SCHED §9 | SR-3 (built, §1.69 SR3-4) | SR-3 (v0.12.0) |
| `scheduling.parallel.*` | Workflow #427-A (**`actingd-config` revision 19**): `enabled` | `false`, settable (§1.200 427A-1): whether the policy driver hands each admitted scheduled run to its instance's worker | #427-A | #427-A (0.12.22): read at the start of every policy round |
| `scheduling.fact_refresh.*` | `enabled`, `min_interval_ms`, `daily_limit`, `failure_backoff_ms`, `max_consecutive_failures`, `recognition_join_timeout_ms`, `overdue_ms` (no default value: unset, no `fact_reader_overdue` is reported; that is its declared default, coordinator ruling 9 of 2026-10-16, so R4 holds) | D2M §3.4 | D2-S4 (built, §1.56 D41, all but `overdue_ms`); `recognition_join_timeout_ms` D2-S2 (built, §1.28); `overdue_ms` D2-S5 (built, §1.68 D53) | D2-S4 (v0.12.0); `recognition_join_timeout_ms` D2-S2; `overdue_ms` D2-S5 |
| per instance | `recognition_package` `{package}` (a location only, no digest; no default) | D2M §3.2 v1.4.2 | D2-S2 (built, §1.28) | D2-S2 |
| per instance | `start_emulator` | ORD §3.5 item 17: default `none` (off unless the entry names it); discovery-bound instances only | S6a (built, §1.36) | S6c (built, §1.60; v0.12.0) |
| per instance | `capture_backend` gains the value `scrcpy` (no new key) | explicit only, never part of `auto` or `auto-fastest`; refused on an emulator instance (`device_config_invalid`), and every instance is an emulator until RD-1b adds `device.kind` (R1 §5.1); the stream's parameters are device-layer defaults, not keys, until D1b's mechanism | #402 RD-3 | #402 RD-3, after RD-1b |
| top level | `lab_module_authoring`, `lab_module_debugging` | UIM §4.1 | U-2 | U-2 (reported only in v0.12.0) |
| `scheduling.suspension.*` | `lift_check_min_interval_ms` | 60 000: the least time between two lift checks of the same package (M-PKG §3.4, D1) | PKG-S3 (built, §1.47) | PKG-S3 (v0.12.0); SR-3 |
| `device_link.*` | `watch.restart_backoff_ms`, `watch.restart_limit`, `watch.restart_window_ms`, `settle_ms`, `offline_action`, `restore_retry_ms` | `[1000, 5000, 30000]`, 5 (1..=100), 600 000 (at least the first restart wait), 3 000 (0..=60 000), `wait` (`wait` / `kick_once`), `[5000, 15000, 60000]` (model-398 §4.6; both lists non-empty); every key settable | RD-2 (built, §1.52) | RD-2 (v0.12.0) |
| `device_state.*` | `sample_interval_ms`, `fact_ttl_ms`, `wake_settle_ms`, `keepalive_margin_ms`, `keyguard_insecure`, `battery_min_percent`, `thermal_hold_at`, `thermal_resume_below`, `in_use_gate`, `in_use_backoff_ms` | 60 000 (5 000..=3 600 000), 180 000 (at least the sampling interval), 1 000 (100..=10 000), 60 000, `dismiss` (`dismiss` / `wait`), 15 (0..=100), `severe`, `moderate` (below `thermal_hold_at`), `false` (a dedicated handset, Alice 2026-10-13), 300 000 (10 000..=3 600 000) (model-398 §7.8); every key settable | RD-5b (built, §1.59) | RD-5b (v0.12.0) |
| `startup_discovery.*` | `attempts`, `retry_interval_ms` | 25 (`1..=100`), 10 000 (`1000..=300000`); not settable, read once at assembly (Workflow #390 DISC-1, coordinator ruling 2 of wave 18) | DISC-1 (§1.89 DI2) | DISC-1 (v0.12.0) |
| `scheduling.remedy.*`, and per instance `instance.<id>.scheduling.remedy.choices` (**`actingd-config` revision 6**, Workflow #411 B15; §1.110 B15-8) | `enabled`, `max_per_round`, `choices` | `true`, 2 (`1..=8`), `[]`; an instance's list replaces the section's as a whole, default the section's; every key settable; rows of the `session.remedy.choices` shape ([runtime-config.md](runtime-config.md)) | #411 B15 R3 (built, §1.113) | #411 B15 R4 (0.12.3); R7 (check-config notes) |
| `touch.*` (**`actingd-config` revision 7**, Workflow #406; §1.136) | `sampling.sigma_ratio_milli`, `sampling.max_redraws`, `sampling.candidate_count`, `history.points_per_box`, `history.max_boxes` | 250 (`1..=1000`), 8 (`0..=64`), 4 (`1..=16`), 16 (`1..=256`), 1024 (`1..=65536`); host-wide, every key settable (plan Q13; [runtime-config.md](runtime-config.md)) | #406 T12 (§1.136) | #406 T12 (0.12.x) |
| `emulator_control.*` (**`actingd-config` revision 8**, #411 A50; §1.158) | `restart_exit_wait_ms` | 60 000 (`1000..=300000`); host-wide, settable ([runtime-config.md](runtime-config.md)) | A50 (§1.158 A50-5) | A50 (0.12.x) |
| `host.*` (**`actingd-config` revision 20**, RB-2; §1.202) | `process_memory_priority`, `process_io_priority` | `normal` (`normal` \| `low`); not settable, applied by actingd to its own process at start ([runtime-config.md](runtime-config.md), "The `host` section") | RB-2 (§1.202 RB2-1) | RB-2 (0.12.28) |
| `store_foreground.*` (**`actingd-config` revision 21**, STORE-FG-2; §1.209) | `allowed_application_ids` | `[]` (0 to 16 unique Android package names, none an instance's `application_id`); not settable, only the configuration file widens it ([runtime-config.md](runtime-config.md), "The `store_foreground` section") | STORE-FG-2 (§1.209 SF2-1) | STORE-FG-2 (0.12.29) |

- Removed by option 丙 in the same revision (PKG-S4): `startup_package.expected_sha256`,
  `prerequisite_packages[].package_digest`, `policy.procedure_manifest[].package_digest`. A
  configuration that still carries them is refused (`config_decode_failed`); no reader of the old
  form exists. **Done (PKG-S4, §1.55 P4-3).**
- Removed by Workflow #373 NAV-3a in the same revision: the top-level `prerequisite_packages`
  and `return_home_packages` (§1.67 N3A-3); the standard-pack registry replaces them.
- `governance.allowed_clients` is still accepted in v0.12.0; U-6 retires it.
- Keys designed after v0.12.0 (#367 D1 decisions and later) bump the revision in their own release.

### 5.1 Capacity of `config.parameters`

`config.parameters` is one runtime fact whose `record_list` holds at most 256 rows (M387 §5.9,
`runtime_fact.rs`). One row per key; list, object and decimal keys are one row each.

| Rows | Count |
|---|---|
| today (`apps/actingd/src/config/manifest.rs` at 706ead94) | about 55, plus 2 per instance |
| `ledger_observer.*` | 35 (21, #411 A51's two, P0-LO's two, one each of #422 LN-5, LN-TUNE and LN-8, #422 LN-9's three, #422 LN-WS's two and #422 LN-7c Q's two) |
| `recovery_ladder.*`, `host_claims.*` | 16 (and #411 A38's two, #411 B37's four, #411 A35's seven) |
| `performance.*`, `execution.defaults.*` | about 100 (some replace today's `performance_monitor.*` / `performance_control.*` rows) |
| `scheduling.*` (recognition, rerun, cost_commit, blackout, feature_lock, not_run, pack_admission, fact_refresh, suspension, random_delay) | 32 |
| Lab options | 2 |
| per instance: `recognition_package`, `start_emulator`, `scheduling.random_delay.*` (#405) | 5 per instance |
| **total** | **about 226 + 7 per instance** (three instances ≈ 247; five ≈ 261) |

The total stayed under 256 for up to eight instances (255 rows) before #405's three per-instance
keys (SR-1, §1.23); with them it passes 256 at five instances. So D1b implements the split (coordinator ruling, 2026-10-12): `config.parameters`
becomes one `config.parameters.<section>` fact per section (M387 §5.9), written and read that way
from D1b on, and each section fact holds at most 256 rows.

## 6 Interface revisions (declared once for v0.12.0 by #378 A8, ORD §3.6)

Declared in `distribution/windows/component-interfaces.json`; the meaning of each revision and
what v0.12.0 moved are in [component-interfaces.md](component-interfaces.md), and contract key
K27 asserts the declaration. Every moving interface rises once for the release.

| Interface | v0.12.0 | Sources in this register |
|---|---|---|
| `actingd-config` | [5, 5] | §5: every key it lists as parsed (not `execution.defaults.*`, see there); the removals of PKG-S4 §1.55 P4-3 and NAV-3a §1.67 N3A-3, which no v0.12.0 reader accepts. Workflow #411 B15 R3 raises it to [5, 6] in v0.12.3 (§1.113 B15C-4): revision 6 adds the remedy keys, and a revision-5 file is still read. Workflow #406 T12 raises it to [5, 7] in #406's release (§1.136 T12-5): revision 7 adds the `touch` section, and revision-5 and revision-6 files are still read. #411 A38 raises it to [5, 8] (§1.148 A38-7): revision 8 adds `recovery_ladder.fence_recheck`, and revisions 5 to 7 are still read. #411 A51 adds `ledger_observer.read_budget_ms` and `close_timeout_ms` to the same revision 8 (§1.151 A51-4). #411 B37 (A35-S1) adds `recovery_ladder.busy` to the same revision 8 (§1.152 B37-5). The 0.12.x release after v0.12.9 raises it to [5, 9] (§1.168 A35-5): revision 9 adds that release's configuration keys (#411 A35: `recovery_ladder.busy.screen`), and revisions 5 to 8 are still read. P0-LO raises it to [5, 10] in v0.12.11: revision 10 adds `ledger_observer.open_timeout_ms` and `open_warn_ratio_milli`, and revisions 5 to 9 are still read. #411 A64, #411 A66, PERF-B1 and SCH-M ED-1 raise it to [5, 11] in v0.12.12: revision 11 adds `host_claims.run_end_grace_ms` (A64, ruling C-43), the `watchdog` section (`start_window_ms`, A66), the `execution` section (`capture.max_interval_ms`, `capture.backoff_enabled`, `capture.idle_ceiling_ms`; PERF-B1, §1.177 B1-2) and SCH-M ED-1's `scheduling.expected_runtime.*` (also per instance) and `scheduling.fact_refresh.expected_duration_ms` (§1.178 ED1-3), and revisions 5 to 10 are still read. IDLE-1 (Alice's ruling 41) raises it to [5, 12] in the release after v0.12.12: revision 12 adds the `idle` section (`action`, `min_gap_ms`, `close_app_wake_lead_ms`, `stop_emulator_wake_lead_ms`) and an instance entry's `idle` member; ROUT-L RL-1b (same release, §1.185 RL1B-1, RL1B-2) adds to revision 12 the `standard_packs` keys `check_interval_ms` and `cached_generations` and a `policy` section without `catalog` (composer mode), #422 LN-5 `ledger_observer.history_page_rows` and #422 LN-TUNE `ledger_observer.writer_reply_timeout_ms`; and revisions 5 to 11 are still read. #422 LN-8 raises it to [5, 13] in the release after v0.12.13: revision 13 adds `ledger_observer.open_verify_tail_records`, and revisions 5 to 12 are still read. #422 LN-WS raises it to [5, 14] in v0.12.15 (§1.194): revision 14 adds `ledger_observer.read_check_vm_steps` and `history_scan_rows`, and revisions 5 to 13 are still read Workflow #392 DA-4b-D (Alice's ruling 69) raises it to [5, 15] in v0.12.16 (§1.195 DA4bD-2): revision 15 adds the `display_safe_area` section (`directory`, `check_interval_ms`, `model`) and an instance entry's `display_safe_area` member (`model`), and revisions 5 to 14 are still read. #422 LN-7c Q raises it to [5, 16] in v0.12.17 (§1.196 LN7CQ-1 to LN7CQ-3): revision 16 adds the top-level `frame_retention_evidence_max_age_days` and `ledger_observer.frame_query_steps_per_second` and `frame_query_chunk_frames`, and revisions 5 to 15 are still read. Workflow #421 INST-1 raises it to [5, 17] in v0.12.18: revision 17 adds the `install` section (the tunables of `actingctl install`, [install-cli.md](install-cli.md)), and revisions 5 to 16 are still read |
| `ledger` | [3, 3] | A3 §1.9, SH-1, SR-2a reads, D1 ladder records, L4a §1.19, AO-1 dump import §1.26, AO-2 index table §1.38, U-2's G14 members §1.46, PKG-S2 and PKG-S3 removals (§2), NAV-6 §1.12, #403 §1.13, D1b §1.14, A2-P §1.15, SH-2 §1.25, OCR-CONF §1.8 V1, M-357b DB-3s §1.66. Workflow #411 BT-LR raises it to [3, 4] once for the 0.12.x B train (§1.153, Alice 2026-10-20): revision 4 adds the train's shapes, and a revision-3 ledger is still read |
| `runtime-client` | [2, 2] | §3 W1–W4, W8–W12, W14, W15, W17 (AO-4q, MCP-4, D1b, SH-1, A4, U-1, RD-3, MCP-2, P-m, D2-S5); A2-P's wire spellings §1.15; 九处-1's status member `emulator_state` §1.91 N1-1. `runtime-client` 3 comes with its own later release (§3 W5–W7 and W13: MCP-19, AO-4, AO-8 with MCP-9b's `startup` and `host` in W7, SR-6b's `not_running` in W13; re-check 10-14, §1.25). Runtime v0.12.x serves the read `ScheduleExplanation` (§3 W5) under revision 2: only actingctl and MCP of the same Runtime send it and every revision-2 shape is unchanged; revision 3 includes it. #411 A51's page limit `read_budget` (§1.151 A51-1) is folded into revision 2 as §1.91 N1-1 was. Workflow #429 R-0 raises it to [2, 3] in 0.12.30 (§1.213 R0-7, §3 W21): revision 3 is revision 2 plus the workbench's shapes (R-0c's `PackScheduleView` included, §1.216 R0C-3), W5 and `StartReport` (§1.216 R0C-4); W6, W7 and W13 are not built and take a later revision |
| `package` | [3, 3] | PKG-S1; PKG-S5 §1.64 P5-1 with M-357a TB12 §1.65; D2-S2 §1.28 DS6; D2-S3 §1.39 D34; READ-1 §1.54 RE1; the NAV-3a follow-up §1.73 N3F-1; REC-6 §1.29 R6-2; REC-8 §1.35 R8-1; OUT-1 §1.40 O1-1; EXEC-2 §1.41 EX1; OUT-2 §1.48 O2-1; REC-5 §1.50 R5-2; EXEC-1 §1.57 EC1. M-357a TB1 and TB6 (DB-1, DB-2) and M-357b MS6–MS10 (DB-3a1, DB-3a2, DB-3b) bump it once more in the 0.12.x release that carries them, once only when they share a release. Workflow #411 B15's `remedy` (a `task.json` mapping member) and `provides` (a `control.json` member) ride revision 4 (writer R1, §1.110 B15-10), raised once in their release by its first slice that needs it. Workflow #406 T3 raises it to [3, 5] in #406's release (§1.137 T3-7): revision 5 adds the click `single_touch_path_drag_v1`, and revision-3 and revision-4 packs are still read |
| `runtime-outcomes` | [1, 1] (new) | A5 §1.16, A6a §1.37 |
| `install-control` | [0, 1] (unchanged) | model-378 §16 A-7 dropped revision 2 (`carried`) from v0.12.0; U-4 raises it to [0, 2] in 0.12.x |
| `install-selection` | [1, 1] (unchanged) | — |

AO-0 changes no declaration.

## 7 What AO-0 does not register, and why

| Item | Why | Who |
|---|---|---|
| the codes of M387 §6.4 item 2 (`config_key_unknown`, `config_key_not_settable`, `config_value_invalid`, `config_override_superseded`, `config_override_base_changed`, `config_override_dropped`, `recovery_ladder_config_invalid`, `recovery_ladder_code_unclassified`) | their owner crate `crates/runtime-config` does not exist yet, and a fragment must belong to a workspace member | D1b (with D1c) |
| keys `config_key`, `ladder_rule` and the vocabularies `ladder_trigger_start`, `rules.*` (M387 §6.4 items 3–4); `ladder_class` | in flight with D1a / D1b | D1a, D1b |
| vocabularies `channel`, `lab_option` and the codes of UIM §7.5 | U-0's code registration | U-0 |
| the codes of PERF §11, SCHED §10, D2M | each model's own slices register them; LEDG §11: codes and keys are registry entries, not persisted structure | P-*, SR-0, D2 slices |
| the event-type and action enum variants of §1 | the `event_type` vocabulary is built from the Rust enum (guard G7), so the types arrive with the code | SH-1 |
