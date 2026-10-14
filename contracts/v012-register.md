# v0.12.0 register: persisted shapes, removals, wire shapes and configuration keys

Workflow #383 slice AO-0. Order-v012 rule R5 asks that every persisted shape the 0.12 series
writes is registered once for the v0.12.0 ledger (revision 3), so that a 0.12.x build never adds a
field to a record an earlier 0.12 build must read (the 2026-09-27 lesson). This register collects
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
| L7 | `policy.control_state_recorded` | new event type, family `policy`, action `policy.control_state` (new), info; tagged | SH-1 | AO-4 | one dispatch pair's complete decision memory: `{instance_id, task_id (or the reserved D2 recognition identity), daily_counts, window_counts, window_runtime_ms, active_window?, failure_streak? {failure_code, failure_identity? (failure identity v2; §1.15 Q1), class, count, escalation_streak, disposition, retry_at_unix_ms?, decision_id}, retry_round?, dispatch_clock?, condition_reruns? {failure_code, count, expires_at_unix_ms}}`; `active_window` = the `window_id` of the task's activity profile that `window_counts` and `window_runtime_ms` count in; `window_runtime_ms` = the runtime the task has reserved or used in that window (`PolicyControlState.task_runtime`, `policy_control.rs:87`); `paused_on` is rebuilt from `failure_identity` and never stored; the activity profile's own state is L14 |
| L8 | `agent.wake_obligation_recorded` | new event type, family `agent`, action `agent.wake`, info; tagged | SH-1 | AO-4 | one wake obligation (`AgentWakeRecord` at AO-4's head) |
| L9 | `policy.dispatch_intent`, `task.selection_evaluated`: `input_fact_revisions` | new optional member | SH-1 | AO-3 | `{instance_facts {lineage, revision}, runtime_facts {lineage, revision}}`; readers accept either it or `input_ledger_position` (still non-zero when present); AO-4 stops writing the old member (ORD §3.5 item 11) |
| L10 | `runtime.fact_snapshot` sealed by the ledger | existing type, origin module `global-ledger` | — | AO-8 (T2) | the core stops writing it (AO-4) |
| L11 | `ledger_index` table | new table in `runtime-state.sqlite` | AO-2 | AO-2 (built, §1.38 OB1) | [ledger-observer.md §11.1](ledger-observer.md#111-the-index-table-ledger_index) |
| L12 | ledger dump file | file format, `dump_version` 1 | AO-1 (built, §1.26 AO1) | AO-1 | [ledger-observer.md §9.3](ledger-observer.md#93-dump-file) |
| L13 | view lists | Health gains `ledger.gap_recorded`, `ledger.health_observed`, `ledger.dump_imported`; Changes gains `policy.control_state_recorded`, `policy.activity_state_recorded`, `policy.fact_refresh_transition`, `agent.wake_obligation_recorded` | SH-1 (view schema upgrade, `views.rs:40-43`) | — | the new `perf.*` types join Health through their family |
| L14 | `policy.activity_state_recorded` | new event type, family `policy`, action `policy.control_state`, info; tagged | SH-1 | AO-4 | one (instance, activity profile)'s complete activity state, index key `activity_memory/<instance>/<activity profile>`: `{instance_id, profile_id, local_day, daily_count, window_id, window_count, window_runtime_ms, next_eligible_unix_ms?}` = today's `activity_daily`, `activity_window`, `activity_runtime` and `next_activity_eligible` (`policy_control.rs:88-91`); without it a restart after AO-6 would reset the activity budgets and the activity interval |

### 1.2 Fact refresh (#384, D2M §5.3 A-5, N3)

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| D1 | `policy.fact_refresh_transition` | new event type, family `policy`, action `policy.fact_refresh` (new), info; G5 copies it; not tagged | SH-1 | AO-4 (gate) after D2-S5 (recorder) | payload `{cycle_seq, instance_id, fact_key, outcome}` (`outcome` = the transition's code); envelope `capture.source` = `fact_refresh_transitions`, lineage = owner epoch, revision = transition number (from 1 per epoch, +1 per transition, never reset); a skip is `ledger_store_revision_skipped` |

### 1.3 Scheduling rules (#386, SCHED §11 X2)

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| S1 | policy execution record, failure part: `condition` | new optional member | SH-1 | SR-2b (SR-2a reads) | `{refunded, count, expires_at_unix_ms}`; replay only — the rerun state loads from L7; **done (SR-2a):** the settlement takes it from the record, as it takes `perf_context` (`policy_control.rs`, `commit_execution` and `preview_settlement`), never from the configuration. A run with it adds to no failure streak and no escalation (the record keeps the pair's previous counters, 0 when there are none, and `failure_streaks` keeps the latest real failure), continues (retry attempt 0, no retry time), is never re-judged `policy_runtime_budget_exceeded`, adds no runtime to the task or activity window, and with `refunded` gives back the task daily, task window, activity daily and activity window counts its admission took. Only a recoverable failure of a task that is not sensitive takes it; any other pairing is `policy_execution_record_mismatch`. `count` and `expires_at_unix_ms` are SR-2b's (the rerun state); the live settlement writes no `condition` before SR-2b |
| S2 | `ContainedTaskCancellationReason::BlackoutStop` | new enum value (wire and ledger) | SH-1 (runtime-client 2) | SR-4b | `from_failure_code`: `blackout_run_stopped` → `BlackoutStop` |
| S3 | `RecoveryLadderSuppression::Blackout` | new enum value | SH-1 | SR-4a (SR-2a decodes) | **done (SR-2a):** a v0.12.0 start reads a `recovery_ladder_suppressed` record with `reason: "blackout"` (SH-1's decoder; no Runtime reader branches on the reason); [emulator-control.md](emulator-control.md) lists the value. The host sets no blackout before SR-4a |

No other new event type (`disposition` gains no value). The pack schedule `session.pack.schedule`
and the feature locks `session.feature.<key>` are ordinary `fact.published` records (§4).

### 1.4 Performance pacing (#385, PERF §12.2)

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| P1 | `perf.pacing_changed` | new event type, family `performance`, action `performance.observe`, observation | SH-1 | P-m | the stage transition record (`perf_stage_changed`) |
| P2 | `perf.dispatch_share_changed` | new event type, decision | SH-1 | P-s | `perf_scheduling_paused` / `perf_scheduling_resumed` |
| P3 | `perf.run_hold` | new event type, decision | SH-1 | P-h | held and resumed at `pace_checkpoint` |
| P4 | `perf.summary` | new members | SH-1 | P-m | `responsiveness_bp`, `host_responsiveness_bp`, `emulator_responsiveness_bp`, `pacing_stage`, `factor_milli`, `dispatch_share_milli`, `pacing_mode`, `learning`; host and per instance: `learning_view_stage`, `learning_view_share_milli`, `learning_view_allowed`, `normal_view_stage`, `normal_view_share_milli`, `normal_view_allowed`; `pacing_phase` (`learning`, `normal`; only when `mode = full`); `learn_days`, `learn_active_ms` |
| P5 | task timing evidence, per step | new members | SH-1 | P-m (`page_settle_ms`, `pacing_band`, `pressure_level`), P-k (`pacing_factor_milli`, `extra_wait_rounds`) | `page_settle_ms` keyed by (instance, `package_id`, step index) |
| P6 | policy execution record | new members | SH-1 | P-k | `wall_elapsed_ms`, `budget_elapsed_ms` |
| P7 | `ContainedTaskCancellationReason::PerformanceYield` | new enum value | SH-1 (runtime-client 2) | P-k / P-h | `from_failure_code`: `perf_run_yielded`, `perf_hold_expired` → `PerformanceYield` (one revision with S2) |
| P8 | task timing evidence, per run (MEM-1, ADD §1.6) | reserved members | SH-1 | P-m | `package_load_ms`, `package_load_bytes`, only if the run record does not already carry them |

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
| A5 | `config.catalog` | runtime fact key (runtime scope, re-observed under `config.`) | MCP-9a | MCP-9a | `{catalog_id, catalog_version, catalog_hash, activated_event_id, approval_id?, approval_disposition?, previous_catalog_id?, previous_catalog_version?}`; the configuration generation is `config.selection` (R6); the watchdog block is read client-side; a bound package path that is missing or unreadable is reported as one overview line computed at read time (coordinator 10-12) |
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
- **A8:** the PPOCR failure source (`ppocr_source`, `ppocr_message`) and the connection
  `timing`, measured but no longer persisted; actingd's dead `client_message` argument
  (`apps/actingd/src/main.rs`, `RuntimeLifecycleFailure::Client.message`).
- **A6a / A8:** check-config v1's `subsystems[].reason` became `outcome` without a version
  bump; A6a's check-config v2 and A8's declaration list it.

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
| N4 | `RuntimeLifecyclePhase::StandardPackRead` (`runtime.lifecycle_observed`, phase `standard_pack_read`) | new lifecycle phase | NAV-6 | NAV-6 | `{outcome}`: `standard_pack_registered` (info; `game`, `count`, optional `added_package_ids`, `removed_package_ids`), `standard_pack_root_invalid` (`game`, `path`, `standard_pack_problem`, `json_pointer`) or `standard_pack_root_unreadable` (`path`, optional `game`; `foreign_os_error` cause), all error but the first; system links; durable (`CommitSync::Full`). At startup one per game root; in a run's preparation one when the registry changed (M-373 §8.4) |
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
| Q7 | `PolicyReasonRecord` | changed: each reason is its outcome | A2-P | A2-P | every entry of `policy.dispatch_*` `reasons` and of `task.selection_evaluated` `reasons` / verdict `reasons` is `{code, category, values}` (was `{code}`; M378 §8.1). Scheduling reasons: `task_eligible`, `rank_assigned`, `score_computed` (were `eligible`, `ranked`, `scored`), `score_unknown` {`rule_id`}, `score_cost_unknown`, `decision_candidate_not_selected` {`task_id`}, `resource_target_unmapped` / `_override` / `_pending` / `_satisfied` / `_applied` {`resource_target_id`} (were `<code>:<value>`); selection reasons: 20 `selection_*`, `candidate_*`, `gate_*`, `term_*`, `tie_break_unknown` codes (were dotted, e.g. `gate.rejected` → `gate_rejected` {`rule_id`}). Replay compares chains exactly, so values are deterministic |
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
`runtime-outcomes` interface (M378 §10, declared by A8) and the watchdog's own files. No
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
today, and the least-recently-used landing-point history lives in memory only.

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

Configuration keys (§5): the `scheduling.recognition.*` row and the five sections of the next
row, as listed, in `distribution/windows/actingd.defaults.json`; Workflow #405's random delay
(§5 row `scheduling.random_delay.*`, coordinator's order of 2026-10-14).
`scheduling.rerun.condition_codes` lists SCHED §9's eight codes; `perf_run_yielded` and
`perf_hold_expired` are registered by SH-1 (pending), since a code list entry must be a listed,
active code (train 6).

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
| DS6 | package format: bundle `maintenance[].uses` value `recognition` (at most one per server); task `resource_readings[].page_id` any page the task declares; `linear_steps` tasks may carry `resource_readings` | new value and relaxed rules (resource file format, not a ledger shape) | D2-S2 | resource side (D2-R1, D2-S6) | a reader that does not know `recognition` refuses the declaration; the `package` interface revision is declared once by A8 |
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
| R6-2 | task schema `0.9` `checks[].value_test` (`{left, op, right?}`); recognition pack schema `0.7` target type `value_test`; `OcrUnsignedIntegerFormat::ClockHms` (`clock_hms`, for fields, readings and value tests); a value test side's `trim` `whitespace_all_v1`; the page gate of a page that collects or reads OCR | new members and values, one rule relaxed (resource file format, not a ledger shape) | REC-6 | resource side | older task and pack schemas refuse the new members (`UnconsumedField`); a value test in a page rule's `forbidden`, directly or through a check, is refused at `/pages/<i>/forbidden/<j>`; a page that collects fields, collection-mode targets or readings may gate on any backend and on those targets themselves (the gate's observation is the collection's); the `package` interface revision for these is declared once by A8 |

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
| R8-1 | task schema `0.9` `candidate_layouts[].shared_ocr` and recognition pack schema `0.7` `candidate_layouts[].shared_ocr`: `{region, languages, timeout_ms, model_ref, model_sha256, blocks, line_merge?}` | new member of a `fixed_slots` layout (resource file format, not a ledger shape) | REC-8 | resource side | [selection-graph.md](selection-graph.md), section Shared OCR read; the region is a rectangle or `full_frame` (the source's `rect` / `full_frame` region form, derived into the pack's), `blocks` must be `detect` with a `line_merge`; a `repeated_anchor` layout refuses it (`UnknownField`), task schemas before `0.9` too (`UnconsumedField`); a shared read no slot or feature target uses is refused; one projection counts it as one provider evaluation per frame it serves, and the targets it serves none; the `package` interface revision for it is declared once by A8 |
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
interface (M378 §10, declared by A8) or actingd's and the watchdog's own files, not ledger
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
| D34 | package format: task `resource_adjustments[] {id, fact_key, operation_id, amount? {target_id, direction: consume \| gain}}` (task schema `0.8`, `0.9`) | new family (resource file format, not a ledger shape) | D2-S3 | resource side (D2-S6) | an older reader refuses the family (`UnknownField`); the `package` interface revision is declared once by A8 |

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
| O1-1 | task schema `0.8` zero-input fields task (`operations: []`): `scheduling_outcome.mappings` | relaxed rule (resource file format, not a ledger shape) | OUT-1 | resource side | one `fields_recorded` mapping whose terminal pages are the fields pages, and any other mappings; every mapping `no_designated_effect`; the target pages are every mapping's terminal pages, no page in two mappings ([ocr-fields.md](ocr-fields.md)); a run that ends on another mapping's page has no OCR observation and no fields report; the `package` interface revision is declared once by A8 |

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
| EX1 | task schema `0.9` operation members `observation_window` (`{until: "window_end", window_ms}` or `{until: "stable", window_ms, frames}`) and `input_recheck` (`{kind: "fresh_frame"}` or `{kind: "stable_frames", frames}`) | new members (resource file format, not a ledger shape) | EXEC-2 | resource side | `window_ms` 1..=60000, `frames` 2..=32 (Runtime admission, `contained_task_operation_invalid`); an older task schema refuses both (`UnconsumedField`), a `linear_steps` task refuses both (`contained_task_linear_invalid`); the `package` interface revision is declared once by A8 |

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

## 2 Removals and changes of option 丙 (M-PKG §4.3, §4.4)

Deleted outright, no old-version reader (M-PKG §7). The slice named deletes the field and its
readers in one PR.

| Slice | Removed or changed |
|---|---|
| PKG-S2 | `PackageAdmitted.package_sha256` (keep `package_label`, `task_label`); `EntryRecoveryPackageAdmitted/Completed/Failed.package_sha256` → `package_label`; effective configuration `EntryRecovery.package_sha256` → `package_label`; `TaskSelectionPolicy.package_sha256`; Lab declared refusal `declared_package`, `verified_package` → optional `package_label`; Lab evidence `expected_package_sha256` / `actual_package_sha256` → `package_label`; `EvidencePackage.sha256`; OCR comparison report `truth_set_sha256` and its runtime-client reader; run status `package_ref` / `recovery_packages` → `package_label` / `recovery_package_labels`; run summary digest checks; reading facts' `resource_bundle_hash` = SHA-256(`"package:" + package_id`) (shape unchanged). **Done (PKG-S2):** all of them, §1.31 P2-1 to P2-11; also the run summary's `package_digest` (now `package_label`), the in-package `sha256` keys made optional and ignored (D5), and request replay by `package_label` (a recorded label must be the request's package's, or a configured prerequisite package's or the recovery binding's `package_id`, each admitted again) |
| PKG-S3 | `DispatchIntent.package_digest` (payload and storage equality); `ProcedureBinding.package_digest`; `binding_digest` tuple v3 = (`procedure_ref`, `operation_id`, `yield_points`); `PausedOn.package_digest` → content hashes parsed from the failure identity; suspension report members `paused_hash_prefix`, `current_hash_prefix`, `lifts_when: content_hash_changes`, `lifted_by: main_hash \| layer:<n> \| restart` |
| PKG-S4 | IPC `expected_sha256` of `PackageDebugRequest`, `ContainedTaskRecoveryBinding`, `ContainedTaskRequest`, the Lab operation, online observation and saved-artifact OCR requests; `PackageDebugSummary.verified_sha256`; configuration `startup_package.expected_sha256`, `prerequisite_packages[].package_digest`, `policy.procedure_manifest[].package_digest`, the resource declaration's `package_digest` |
| PKG-S5 | bundle v4 `packs[] {package_id, server, entry_task_id, path = "packs/<package_id>", file_count, byte_count}` (the bundle-level maintenance declaration stays, coordinator 10-12); `RecordingArtifact.digest` / `.sha256` → `package_id`; `PackageRef`, `ContentDirectory` and the `sha256:` serializers |

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
| W5 | read request `ScheduleExplanation { instance }` | runtime-client 3 | MCP-19 | reserved; per task: trigger, last dispatch, next due, not-run reason, produces, consumes, pools |
| W6 | gate-class receipts carry `event_id` and store revisions, never a ledger sequence; install receipts after the commit | runtime-client 3 | AO-4 | [ledger-observer.md §13](ledger-observer.md#13-identity-without-ledger-positions-ao-3-ao-4-ao-4q-in-v0120) |
| W7 | status blocks `ledger`, `tagged_state`; `startup`, `host` | runtime-client 3 | AO-8; MCP-9b (`startup`, `host`) | [ledger-observer.md §14.2](ledger-observer.md#142-status-blocks); `startup` and `host` (MCP-9b) are held in memory and leave through the same status read as AO-8's blocks (re-check 10-14, §1.25) |
| W8 | request `actingcommand.runtime.request.v4`: optional `detail` (`codes` \| `full`) | runtime-client 2 | A4 | #378 §7.1; absent, the source's class default applies (codes only for `source: ui`, the full outcome for every other source); requests v1–v3 are refused; [client-interactions.md](client-interactions.md) |
| W9 | receipt `actingcommand.runtime.receipt.v2`: `outcome` (an `actingcommand.outcome.v1` envelope) on every receipt; the v1 member `error` (`RuntimeErrorProjection`) is removed | runtime-client 2 | A4 | #378 §7.2; admitted, observed, queued and cancelled carry `request_admitted`, `request_observed`, `request_queued`, `request_cancelled`; completed carries its kind's `request_<kind>_completed`; denied and failed carry the refusal or failure on top, with the values `runtime_code`, `operation`, `host_halted`, `holder_id`, `lease_id`, `retry_after_ms` where they apply; `host_halted` travels in codes mode too; a consumer judges refusal by `state` (denied/failed), the category only says how bad it is; codes mode is applied where the receipt leaves the host |
| W10 | `RuntimeResult::ContainedTaskCompleted.outcome` → `task_result`; `ContainedTaskCancellationStatus::Terminal.outcome` → `task_result` | runtime-client 2 | A4 | #378 §8.1: renames in receipt v2 (same value), because `outcome` is reserved for the envelope |
| W11 | request kinds `BindChannel`, `UpgradeState`, `UpgradeStateStatus`, `LoadPack`, `UnloadPack` | runtime-client 2 | U-1 | UIM §8.1; their success codes come with the behaviour (U-2, U-3, U-4) |
| W12 | `RuntimeCaptureBackend::Scrcpy`, spelled `scrcpy`: `ReadonlyObservation.capture_backend` (read-only observation results and `CaptureSequence.observations`) | runtime-client 2 | #402 RD-3 | new enum value (R1 §5.1); the Lab maps it one to one to `CaptureBackendName::Scrcpy` (C1); W8–W11 are A4's (#718) |
| W13 | `RuntimeInstanceStatus.not_running` | runtime-client 3 | SR-6b | the per-instance not-run rows; they live in the workbench's memory only, and AO-4q (#746) stopped persisting the status (status reads append nothing), so the member is a wire shape, not a ledger one (re-check 10-14, §1.25) |

U-1 owns the shapes of `BindChannel`, `UpgradeState`, `UpgradeStateStatus`, `LoadPack` and
`UnloadPack` (W11; UIM §8.1; LoadPack and UnloadPack without any digest, M-PKG §5.1).

## 4 Fact keys new in the 0.12 series and their tag class

| Key | Store | Tagged | Writer slice |
|---|---|---|---|
| `health.perf_baseline.<signal>.<backend>` (per instance, one per backend in use) | instance | yes (every instance key) | P-m |
| `session.pack.schedule` (one per scope) | instance | yes | SR-11 |
| `session.feature.<feature_key>` | instance | yes | SR-5a |
| `session.instructions` (reserved, one per scope) | instance | yes | GOAL-1 |
| `host.perf_baseline.<signal>` | workbench | yes (`tag.workbench_patterns`) | P-m |
| `host.perf_calibration.stats`, `.curve`, `.phase` | workbench | yes | P-m (`stats`), P-a |
| `host.config_overrides` | workbench | yes (added to `tag.workbench_patterns` by M387 §5.7) | D1b |
| `config.selection`, `config.catalog` | workbench | no (`config.` is re-observed) | D1b, MCP-9a |
| `host.pack_registry.*` | workbench | no (added to `tag.reobserve_prefixes`, UIM §6.5) | U-3 |
| `display.frame_extent`, `display.rotation`, `display.orientation`, `display.aspect` (per instance, only for an instance bound to a device; [instance-fact-store.md](instance-fact-store.md#display-facts)) | instance | yes (every instance key) | DA-A |
| `display.logical_extent` (one row `{width, height}`), `display.safe_area.reported` (one row `{left, top, right, bottom}`, device-frame pixels), `display.safe_area.declared` (the same row), `display.map` (one row, model 392 §8.1) (per instance; §1.25 X10) | instance | yes (every instance key) | DA-4 (`display.safe_area.declared`: a person or an agent) |
| `host.kept_folder_cleared` (re-check 10-14, §1.25) | workbench | no (`tag.reobserve_prefixes`) | 375-R5e-clear (U-3) |

The capacity of the instance store is booked in
[Instance fact store](instance-fact-store.md#capacity-ledger-workflow-383-ao-0).

## 5 Configuration keys of `actingd-config` revision 5

One revision for v0.12.0, declared by A8 (ORD §3.6). Every key designed by the frozen models is
parsed, validated and defaulted in v0.12.0 even when its reader comes in 0.12.x (ORD R4); the
defaults live in `distribution/windows/actingd.defaults.json` (#387 D1b), never in the example
configuration and never as code constants; each parsing slice brings defaults and parsing in the
same PR (ORD R3). "Parsed by" is the slice that adds the key to the configuration; "read by" is
when it takes effect.

| Section | Keys | Defaults and meaning | Parsed by | Read by (takes effect) |
|---|---|---|---|---|
| `ledger_observer.*` | the 21 keys of [ledger-observer.md §15](ledger-observer.md#15-configuration-keys-ledger_observer) | there | AO-1 (built, §1.26) | AO-1, AO-2 (v0.12.0); AO-5, AO-8 (0.12.x) |
| `recovery_ladder.*` | `enabled`, `cooldown_secs`, `trigger_defaults.start`, `.start_rung`, `.respect_cooldown`, `triggers`, `exclude_codes`, `rules.direct_runs`, `.preparation_superseded`, `.already_running`, `.cooldown_opens_on`, `.entry_unavailable`, `rungs`, `hold.claim_ttl_ms`, `hold.margin_ms` | M387 §7.1 | D1c | D1c (v0.12.0) |
| `host_claims.*` | `renewal_slack_ms` | 1 000 | D1c | D1c |
| per instance | `stuck_recovery`, `stuck_recovery_cooldown_secs` (existing keys, defaults now from the section) | M387 §5.5 | D1b | D1c |
| `performance.monitor.*`, `performance.pressure.*`, `performance.load.*` | PERF §10.1 (15 keys; the three existing `performance_monitor` keys keep their file paths) | PERF §10.1 | P-c (built, §1.24) | P-m (v0.12.0) |
| `performance.responsiveness.*` | PERF §10.2 (37 keys) | PERF §10.2 | P-c (built, §1.24) | P-m (v0.12.0) |
| `performance.pacing.*` | PERF §10.3 (26 keys; `mode` stays `observe` in every release until Alice approves) | PERF §10.3 | P-c (built, §1.24) | P-k, P-s, P-h (0.12.x); `clock_jump_ms`, `heavy_dispatch_limit.*` P-c |
| `performance.calibration.*` | PERF §10.4 (19 keys) | PERF §10.4 | P-c (built, §1.24) | P-a (0.12.x) |
| `execution.defaults.*` | the kernel's fallback defaults (PERF §10.4 last notes) | today's constants | P-c | P-c |
| `scheduling.recognition.*` | `sample_timeout_retries` | 2 | SR-1 (built, §1.23) | SR-1 (v0.12.0) |
| `scheduling.rerun.*`, `scheduling.cost_commit.*`, `scheduling.blackout.*`, `scheduling.feature_lock.*`, `scheduling.not_run.*` | SCHED §9 (17 keys) | SCHED §9 | SR-1 (built, §1.23) | SR-2b, SR-4a, SR-5a, SR-10 (0.12.x); `not_run.diagnose_rows` SR-6 |
| `scheduling.random_delay.*`, and per instance `instance.<id>.scheduling.random_delay.*` | `enabled`, `min_ms`, `max_ms` | `false`, 60 000, 3 600 000; `0 <= min_ms <= max_ms <= 86400000` on the pair in effect; an instance's default is the section's (#405, coordinator's order of 2026-10-14) | SR-1 (built, §1.23) | #405 (0.12.x) |
| `scheduling.pack_admission.*` | `check_in_check_config`, `check_after_start`, `suspend_codes` | SCHED §9 | SR-3 | SR-3 |
| `scheduling.fact_refresh.*` | `enabled`, `min_interval_ms`, `daily_limit`, `failure_backoff_ms`, `max_consecutive_failures`, `recognition_join_timeout_ms`, `overdue_ms` (no default: unset = not reported) | D2M §3.4 | D2-S4; `recognition_join_timeout_ms` D2-S2 (built, §1.28) | D2-S4 (v0.12.0); `recognition_join_timeout_ms` D2-S2 |
| per instance | `recognition_package` `{package}` (a location only, no digest; no default) | D2M §3.2 v1.4.2 | D2-S2 (built, §1.28) | D2-S2 |
| per instance | `start_emulator` | ORD §3.5 item 17: default `none` (off unless the entry names it); discovery-bound instances only | S6a (built, §1.36) | S6c (v0.12.0) |
| per instance | `capture_backend` gains the value `scrcpy` (no new key) | explicit only, never part of `auto` or `auto-fastest`; refused on an emulator instance (`device_config_invalid`), and every instance is an emulator until RD-1b adds `device.kind` (R1 §5.1); the stream's parameters are device-layer defaults, not keys, until D1b's mechanism | #402 RD-3 | #402 RD-3, after RD-1b |
| top level | `lab_module_authoring`, `lab_module_debugging` | UIM §4.1 | U-2 | U-2 (reported only in v0.12.0) |
| `scheduling.suspension.*` | `lift_check_min_interval_ms` | 60 000: the least time between two lift checks of the same package (M-PKG §3.4, D1) | PKG-S3 | PKG-S3; SR-3 |

- Removed by option 丙 in the same revision (PKG-S4): `startup_package.expected_sha256`,
  `prerequisite_packages[].package_digest`, `policy.procedure_manifest[].package_digest`. A
  configuration that still carries them is refused (`config_decode_failed`); no reader of the old
  form exists.
- `governance.allowed_clients` is still accepted in v0.12.0; U-6 retires it.
- Keys designed after v0.12.0 (#367 D1 decisions and later) bump the revision in their own release.

### 5.1 Capacity of `config.parameters`

`config.parameters` is one runtime fact whose `record_list` holds at most 256 rows (M387 §5.9,
`runtime_fact.rs`). One row per key; list, object and decimal keys are one row each.

| Rows | Count |
|---|---|
| today (`apps/actingd/src/config/manifest.rs` at 706ead94) | about 55, plus 2 per instance |
| `ledger_observer.*` | 21 |
| `recovery_ladder.*`, `host_claims.*` | 16 |
| `performance.*`, `execution.defaults.*` | about 100 (some replace today's `performance_monitor.*` / `performance_control.*` rows) |
| `scheduling.*` (recognition, rerun, cost_commit, blackout, feature_lock, not_run, pack_admission, fact_refresh, suspension, random_delay) | 32 |
| Lab options | 2 |
| per instance: `recognition_package`, `start_emulator`, `scheduling.random_delay.*` (#405) | 5 per instance |
| **total** | **about 226 + 7 per instance** (three instances ≈ 247; five ≈ 261) |

The total stayed under 256 for up to eight instances (255 rows) before #405's three per-instance
keys (SR-1, §1.23); with them it passes 256 at five instances. So D1b implements the split (coordinator ruling, 2026-10-12): `config.parameters`
becomes one `config.parameters.<section>` fact per section (M387 §5.9), written and read that way
from D1b on, and each section fact holds at most 256 rows.

## 6 Interface revisions (declared once by A8, ORD §3.6)

`ledger` [3, 3] (A3 §1.9, SH-1, SR-2a reads, D1 ladder records, L4a, AO-1 dump import, AO-2 index
table, U-2's G14 members, PKG-S2 and PKG-S3 removals, NAV-6 §1.12, #403 §1.13, D1b §1.14, A2-P §1.15,
SH-2 §1.25);
`runtime-client` [2, 2] (§3 W1–W4, W8–W12: A4, U-1, D1b, D2-S5, P-m, RD-3; A2-P's wire
spellings §1.15); `runtime-client` 3 comes with its own later release, not with A8's
declaration (§3 W5–W7 and W13: MCP-19, AO-4, AO-8 with MCP-9b's `startup` and `host` in W7,
SR-6b's `not_running` in W13; re-check 10-14, §1.25); `actingd-config` 5
(§5); `runtime-outcomes` [1, 1] (A5 §1.16); `package` (PKG-S1, PKG-S5, D2-S2/S3); `install-control` stays
[0, 1] until U-4. AO-0 changes no declaration.

## 7 What AO-0 does not register, and why

| Item | Why | Who |
|---|---|---|
| the codes of M387 §6.4 item 2 (`config_key_unknown`, `config_key_not_settable`, `config_value_invalid`, `config_override_superseded`, `config_override_base_changed`, `config_override_dropped`, `recovery_ladder_config_invalid`, `recovery_ladder_code_unclassified`) | their owner crate `crates/runtime-config` does not exist yet, and a fragment must belong to a workspace member | D1b (with D1c) |
| keys `config_key`, `ladder_rule` and the vocabularies `ladder_trigger_start`, `rules.*` (M387 §6.4 items 3–4); `ladder_class` | in flight with D1a / D1b | D1a, D1b |
| vocabularies `channel`, `lab_option` and the codes of UIM §7.5 | U-0's code registration | U-0 |
| the codes of PERF §11, SCHED §10, D2M | each model's own slices register them; LEDG §11: codes and keys are registry entries, not persisted structure | P-*, SR-0, D2 slices |
| the event-type and action enum variants of §1 | the `event_type` vocabulary is built from the Rust enum (guard G7), so the types arrive with the code | SH-1 |
