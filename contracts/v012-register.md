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
| D1 | `policy.fact_refresh_transition` | new event type, family `policy`, action `policy.fact_refresh` (new), info; G5 copies it; not tagged | SH-1 | AO-4 (gate) after D2-S5 (recorder; built, §1.68, in memory only) | payload `{cycle_seq, instance_id, fact_key, outcome}` (`outcome` = the transition's code); envelope `capture.source` = `fact_refresh_transitions`, lineage = owner epoch, revision = transition number (from 1 per epoch, +1 per transition, never reset); a skip is `ledger_store_revision_skipped` |

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
| SQ5 | the `task.failed` terminal of a wait under target consensus whose screen never meets the consensus: a page wait, a step's postcondition and a `linear_steps` wait (first step, intermediate page, next step, retry decision) | new rule of existing outcomes (shapes unchanged) | TEST-FU (wave-17 acceptance rulings 1-2) | TEST-FU | after a frame that gave no awaited page, a window whose rest can no longer hold the next sampled recognition while the task's remaining time still can is spent, as an elapsed one is: the wait ends with its own miss (`contained_task_page_unknown`, `contained_task_step_unconfirmed`; `contained_task_linear_entry_unmatched`, `contained_task_linear_intermediate_unobserved`, `page_confirmation_failed`), class `pack_logic_mismatch`, so it reaches the ladder; it was `recognition_sample_budget_insufficient` (SQ2), which stays for a task whose remaining time cannot hold the recognition and for every first observation of a window; found by TEST-FU's G3 tests (exec key 21); a run of optional operations' wait (`linear_wait_set`) is not covered |
| SQ6 | the admission of a policy dispatch intent: its refusal `policy_facts_stale` | new rule of an existing refusal (shapes unchanged) | TEST-FU (coordinator ruling, 10-16 22:0x) | TEST-FU | Workflow #369 model v3.1 §3.3 W-1 (the other instances' intents run in the same cycle): admission refuses an intent `policy_facts_stale` only when the evaluation's `fact_snapshot_id` moved and the intent's instance view moved too; the view is that instance's snapshot, the facts and catalog pools whose scope selects it (its instance, server or game; a pool the catalog does not declare selects every instance), the priority offsets for it or for every instance, its own task states and outcomes, and every host; it was the whole snapshot alone, which the cycle's earlier dispatch on another instance always moves, so a cycle's intent on a second instance was refused before it was written; a change on the intent's own instance, server or game still refuses it, the decision identity and the recorded `fact_snapshot_id` stay the evaluation's, and the view lives in memory with the pending intent (no persisted or wire member, no code); [instance-fact-store.md](instance-fact-store.md), "Admission staleness per instance"; found by TEST-FU's A-4 G5 row 2 test (apps key 6) |

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
| W15 | `RuntimeOperation::PerformanceStatus` → `RuntimeResult::PerformanceStatus { status }` | runtime-client 2 (§3) | P-m | P-m | a general read from memory that reads and records nothing in the ledger (U-1 row: general read, no G14 copy); `status` is a `RuntimePerformanceStatus {observed_at_unix_ms, pacing_mode, pacing_phase? (only under full), learning, monitor_health, host, instances (one per registered instance, at most 256), warnings (at most 64 warning outcomes: perf_probe_unavailable, perf_baseline_clamped)}`; `host` = `{host_responsiveness_bp?, responsiveness_bp, pacing_stage, factor_milli, learning_view, learning_view_allowed (count), normal_view, normal_view_allowed (count), effective_stage, effective_factor_milli}`; an instance row = `{instance_alias, emulator_responsiveness_bp?, responsiveness_bp, pacing_stage, factor_milli, learning_view, learning_view_allowed (bool), normal_view, normal_view_allowed (bool), effective_stage, effective_factor_milli}`; a view = `{pacing_stage, share_milli, dwell_24h, dwell_total, stops_24h, stops_total}`, a dwell = `{s0_ms, s1_ms, s2_ms, s3_ms, s4_ms}`; a completed receipt carries `request_performance_status_completed`; a wrong shape is `invalid_performance_status` |
| W16 | `actingctl perf-status [--instance <alias\|instance_id\|ADB port>]` ↔ `ac_overview` block `performance` (`sections` gains `performance`) | read output of a general read (no persisted record; the CLI line carries it beside its outcome) | P-m | P-m | the W15 status, its `instances` narrowed to the selected one; CLI success `runtime_performance_reported`; actingd absent: the client's failure, exit 1; in `ac_overview` a failed read leaves a warning and `incomplete`, never an empty block; the block's place in the cut order was registered by MCP-8 |
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
| W5 | read request `ScheduleExplanation { instance }` | runtime-client 3 | MCP-19 | reserved; per task: trigger, last dispatch, next due, not-run reason, produces, consumes, pools |
| W6 | gate-class receipts carry `event_id` and store revisions, never a ledger sequence; install receipts after the commit | runtime-client 3 | AO-4 | [ledger-observer.md §13](ledger-observer.md#13-identity-without-ledger-positions-ao-3-ao-4-ao-4q-in-v0120) |
| W7 | status blocks `ledger`, `tagged_state`; `startup`, `host` | runtime-client 3 | AO-8; MCP-9b (`startup`, `host`) | [ledger-observer.md §14.2](ledger-observer.md#142-status-blocks); `startup` and `host` (MCP-9b) are held in memory and leave through the same status read as AO-8's blocks (re-check 10-14, §1.25) |
| W8 | request `actingcommand.runtime.request.v4`: optional `detail` (`codes` \| `full`) | runtime-client 2 | A4 | #378 §7.1; absent, the source's class default applies (codes only for `source: ui`, the full outcome for every other source); requests v1–v3 are refused; [client-interactions.md](client-interactions.md) |
| W9 | receipt `actingcommand.runtime.receipt.v2`: `outcome` (an `actingcommand.outcome.v1` envelope) on every receipt; the v1 member `error` (`RuntimeErrorProjection`) is removed | runtime-client 2 | A4 | #378 §7.2; admitted, observed, queued and cancelled carry `request_admitted`, `request_observed`, `request_queued`, `request_cancelled`; completed carries its kind's `request_<kind>_completed`; denied and failed carry the refusal or failure on top, with the values `runtime_code`, `operation`, `host_halted`, `holder_id`, `lease_id`, `retry_after_ms` where they apply; `host_halted` travels in codes mode too; a consumer judges refusal by `state` (denied/failed), the category only says how bad it is; codes mode is applied where the receipt leaves the host |
| W10 | `RuntimeResult::ContainedTaskCompleted.outcome` → `task_result`; `ContainedTaskCancellationStatus::Terminal.outcome` → `task_result` | runtime-client 2 | A4 | #378 §8.1: renames in receipt v2 (same value), because `outcome` is reserved for the envelope |
| W11 | request kinds `BindChannel`, `UpgradeState`, `UpgradeStateStatus`, `LoadPack`, `UnloadPack` | runtime-client 2 | U-1 | UIM §8.1; their success codes come with the behaviour (U-2, U-3, U-4); **done (U-2):** `BindChannel` answers `ChannelBound` with `request_bind_channel_completed` (§1.46 U2-3) |
| W12 | `RuntimeCaptureBackend::Scrcpy`, spelled `scrcpy`: `ReadonlyObservation.capture_backend` (read-only observation results and `CaptureSequence.observations`) | runtime-client 2 | #402 RD-3 | new enum value (R1 §5.1); the Lab maps it one to one to `CaptureBackendName::Scrcpy` (C1); W8–W11 are A4's (#718) |
| W13 | `RuntimeInstanceStatus.not_running` | runtime-client 3 | SR-6b | the per-instance not-run rows; they live in the workbench's memory only, and AO-4q (#746) stopped persisting the status (status reads append nothing), so the member is a wire shape, not a ledger one (re-check 10-14, §1.25) |
| W14 | `EventQuery.event_types` (a set of event types, OR within it) and `EventQuery.fact_key` (an instance or runtime fact key) | runtime-client 2 | MCP-2 | two optional members of the `QueryEvents` query (`deny_unknown_fields`), absent when unused, so a query without them serializes, and fingerprints its cursor, as before; `event_types` is the alternative of `event_type` (at most 32, each once, none excluded, else `invalid_event_query_event_type_filter`); `fact_key` keeps only the records that name it (`fact.published`, `fact.invalidated`, `runtime.fact_recorded`, `runtime.fact_invalidated`), else `invalid_event_query_fact_key_filter`; [global-ledger-query.md](global-ledger-query.md) |
| W15 | `RuntimeOperation::PerformanceStatus` → `RuntimeResult::PerformanceStatus { status }` | runtime-client 2 | P-m | the shadow measurement's status read; **done (P-m):** §1.61 W15, with its read output W16 |
| W17 | `RuntimeOperation::FactRefreshStatus` → `RuntimeResult::FactRefreshStatus { status }` | runtime-client 2 | D2-S5 | the D2 S5 transition query (UIM §2.3); **done (D2-S5):** §1.68 D51, with its read output D52 (W18) |

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
| `ledger_observer.*` | the 21 keys of [ledger-observer.md §15](ledger-observer.md#15-configuration-keys-ledger_observer) | there | AO-1 (built, §1.26) | AO-1, AO-2 (v0.12.0); AO-5, AO-8 (0.12.x) |
| `recovery_ladder.*` | `enabled`, `cooldown_secs`, `trigger_defaults.start`, `.start_rung`, `.respect_cooldown`, `triggers`, `exclude_codes`, `rules.direct_runs`, `.preparation_superseded`, `.already_running`, `.cooldown_opens_on`, `.entry_unavailable`, `rungs`, `hold.claim_ttl_ms`, `hold.margin_ms`; Workflow #398 RD-4: `rules.start_beyond_profile`, `profiles`, `kind_profiles` | M387 §7.1; model-398 §6.3, §6.6 (`exhaust`, the `handset` profile of return home and application restart for the kind `handset`) | D1c; RD-4 (§1.81) | D1c (v0.12.0); RD-4 (v0.12.0) |
| `host_claims.*` | `renewal_slack_ms` | 1 000 | D1c | D1c |
| per instance | `stuck_recovery`, `stuck_recovery_cooldown_secs` (existing keys, defaults now from the section) | M387 §5.5 | D1b | D1c |
| per instance | `recovery_ladder_profile` | model-398 §6.2: default `none` (the kind table, then the section's `rungs`); must name a profile; settable | RD-4 (§1.81) | RD-4 (v0.12.0) |
| `performance.monitor.*`, `performance.pressure.*`, `performance.load.*` | PERF §10.1 (15 keys; the three existing `performance_monitor` keys keep their file paths) | PERF §10.1 | P-c (built, §1.24) | P-m `gpu_aggregate`; PM-CFG every other key (v0.12.0, §1.87) |
| `performance.responsiveness.*` | PERF §10.2 (37 keys) | PERF §10.2 | P-c (built, §1.24) | P-m (v0.12.0) |
| `performance.pacing.*` | PERF §10.3 (26 keys; `mode` stays `observe` in every release until Alice approves) | PERF §10.3 | P-c (built, §1.24) | P-k, P-s, P-h (0.12.x); `clock_jump_ms`, `heavy_dispatch_limit.*` P-c |
| `performance.calibration.*` | PERF §10.4 (19 keys) | PERF §10.4 | P-c (built, §1.24) | P-a (0.12.x) |
| `execution.defaults.*` | the kernel's fallback defaults (PERF §10.4 last notes) | today's constants | **not built:** P-c left it out (PR #748 deviation 5, "after SR-1"), and no later slice parses it, so it is not part of revision 5 (open for the coordinator, A8 report) | — (the kernel's fallbacks stay code constants in v0.12.0) |
| `scheduling.recognition.*` | `sample_timeout_retries` | 2 | SR-1 (built, §1.23) | SR-1 (v0.12.0) |
| `scheduling.rerun.*`, `scheduling.cost_commit.*`, `scheduling.blackout.*`, `scheduling.feature_lock.*`, `scheduling.not_run.*` | SCHED §9 (17 keys) | SCHED §9 | SR-1 (built, §1.23) | SR-2b, SR-4a, SR-5a, SR-10 (0.12.x); `not_run.diagnose_rows` SR-6 |
| `scheduling.random_delay.*`, and per instance `instance.<id>.scheduling.random_delay.*` | `enabled`, `min_ms`, `max_ms` | `false`, 60 000, 3 600 000; `0 <= min_ms <= max_ms <= 86400000` on the pair in effect; an instance's default is the section's (#405, coordinator's order of 2026-10-14) | SR-1 (built, §1.23) | #405 (0.12.x) |
| `scheduling.pack_admission.*` | `check_in_check_config`, `check_after_start`, `suspend_codes` | SCHED §9 | SR-3 (built, §1.69 SR3-4) | SR-3 (v0.12.0) |
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

## 6 Interface revisions (declared once for v0.12.0 by #378 A8, ORD §3.6)

Declared in `distribution/windows/component-interfaces.json`; the meaning of each revision and
what v0.12.0 moved are in [component-interfaces.md](component-interfaces.md), and contract key
K27 asserts the declaration. Every moving interface rises once for the release.

| Interface | v0.12.0 | Sources in this register |
|---|---|---|
| `actingd-config` | [5, 5] | §5: every key it lists as parsed (not `execution.defaults.*`, see there); the removals of PKG-S4 §1.55 P4-3 and NAV-3a §1.67 N3A-3, which no v0.12.0 reader accepts. Workflow #411 B15 R3 raises it to [5, 6] in v0.12.3 (§1.113 B15C-4): revision 6 adds the remedy keys, and a revision-5 file is still read |
| `ledger` | [3, 3] | A3 §1.9, SH-1, SR-2a reads, D1 ladder records, L4a §1.19, AO-1 dump import §1.26, AO-2 index table §1.38, U-2's G14 members §1.46, PKG-S2 and PKG-S3 removals (§2), NAV-6 §1.12, #403 §1.13, D1b §1.14, A2-P §1.15, SH-2 §1.25, OCR-CONF §1.8 V1, M-357b DB-3s §1.66 |
| `runtime-client` | [2, 2] | §3 W1–W4, W8–W12, W14, W15, W17 (AO-4q, MCP-4, D1b, SH-1, A4, U-1, RD-3, MCP-2, P-m, D2-S5); A2-P's wire spellings §1.15; 九处-1's status member `emulator_state` §1.91 N1-1. `runtime-client` 3 comes with its own later release (§3 W5–W7 and W13: MCP-19, AO-4, AO-8 with MCP-9b's `startup` and `host` in W7, SR-6b's `not_running` in W13; re-check 10-14, §1.25) |
| `package` | [3, 3] | PKG-S1; PKG-S5 §1.64 P5-1 with M-357a TB12 §1.65; D2-S2 §1.28 DS6; D2-S3 §1.39 D34; READ-1 §1.54 RE1; the NAV-3a follow-up §1.73 N3F-1; REC-6 §1.29 R6-2; REC-8 §1.35 R8-1; OUT-1 §1.40 O1-1; EXEC-2 §1.41 EX1; OUT-2 §1.48 O2-1; REC-5 §1.50 R5-2; EXEC-1 §1.57 EC1. M-357a TB1 and TB6 (DB-1, DB-2) and M-357b MS6–MS10 (DB-3a1, DB-3a2, DB-3b) bump it once more in the 0.12.x release that carries them, once only when they share a release. Workflow #411 B15's `remedy` (a `task.json` mapping member) and `provides` (a `control.json` member) ride revision 4 (writer R1, §1.110 B15-10), raised once in their release by its first slice that needs it |
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
