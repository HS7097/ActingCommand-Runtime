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
| L1 | `ledger.gap_recorded` | new event type, family `ledger`, action `ledger.recovery`, origin module `global-ledger`; severity = outcome category | SH-1 | AO-1 (dump gaps), AO-2 (tail, single record) | payload `{gap_kind (dump_incomplete, dump_invalid, tail_unconfirmed, record_rejected), gap_start_unix_ms, gap_end_unix_ms, record_count? (absent = unknown), event_id?, source? {source, lineage, revision}, outcome}`; [ledger-observer.md §10](ledger-observer.md#10-gap-records-ao-1-ao-2) |
| L2 | `ledger.health_observed` | new event type, family `ledger`, action `ledger.probe` (new action value), origin module `global-ledger` | SH-1 | AO-2 (watermark and spill segments, revision skips), AO-8 (mismatch, flush lag) | payload `{outcome, segment_edge? (started, ended)}`; the finding's values travel in `outcome` |
| L3 | `ledger.dump_imported` | new event type, family `ledger`, action `ledger.recovery`, origin module `global-ledger`, info | SH-1 | AO-1 | payload `{outcome}` with `ledger_dump_imported` and its values |
| L4 | record envelope `capture` | new optional envelope member of every stored event | SH-1 | AO-2 (`capture_seq`, `captured_at_unix_ms`), AO-3 (`source`) | `{capture_seq, captured_at_unix_ms, source? {source: instance_facts \| runtime_facts \| decision_memory \| fact_refresh_transitions, lineage, revision}}`; `timestamp` = capture time |
| L5 | `runtime.lifecycle_observed` phase `tagged_state_loaded` | new phase | SH-1 | AO-6 | `{stores: [{source, previous_lineage?, previous_revision?, lineage, loaded_count}]}`; the base record of a new lineage |
| L6 | `runtime.lifecycle_observed` phase `tagged_state_enabled` | new phase | SH-1 | AO-5 / AO-6 | no members |
| L7 | `policy.control_state_recorded` | new event type, family `policy`, action `policy.control_state` (new), info; tagged | SH-1 | AO-4 | one dispatch pair's complete decision memory: `{instance_id, task_id (or the reserved D2 recognition identity), daily_counts, window_counts, window_runtime_ms, active_window?, failure_streak? {error_code (failure identity v2), class, count, escalation_streak, disposition, retry_at_unix_ms?, decision_id}, retry_round?, dispatch_clock?, condition_reruns? {failure_code, count, expires_at_unix_ms}}`; `active_window` = the `window_id` of the task's activity profile that `window_counts` and `window_runtime_ms` count in; `window_runtime_ms` = the runtime the task has reserved or used in that window (`PolicyControlState.task_runtime`, `policy_control.rs:87`); `paused_on` is rebuilt from `error_code` and never stored; the activity profile's own state is L14 |
| L8 | `agent.wake_obligation_recorded` | new event type, family `agent`, action `agent.wake`, info; tagged | SH-1 | AO-4 | one wake obligation (`AgentWakeRecord` at AO-4's head) |
| L9 | `policy.dispatch_intent`, `task.selection_evaluated`: `input_fact_revisions` | new optional member | SH-1 | AO-3 | `{instance_facts {lineage, revision}, runtime_facts {lineage, revision}}`; readers accept either it or `input_ledger_position` (still non-zero when present); AO-4 stops writing the old member (ORD §3.5 item 11) |
| L10 | `runtime.fact_snapshot` sealed by the ledger | existing type, origin module `global-ledger` | — | AO-8 (T2) | the core stops writing it (AO-4) |
| L11 | `ledger_index` table | new table in `runtime-state.sqlite` | AO-2 | AO-2 | [ledger-observer.md §11.1](ledger-observer.md#111-the-index-table-ledger_index) |
| L12 | ledger dump file | file format, `dump_version` 1 | AO-1 | AO-1 | [ledger-observer.md §9.3](ledger-observer.md#93-dump-file) |
| L13 | view lists | Health gains `ledger.gap_recorded`, `ledger.health_observed`, `ledger.dump_imported`; Changes gains `policy.control_state_recorded`, `policy.activity_state_recorded`, `policy.fact_refresh_transition`, `agent.wake_obligation_recorded` | SH-1 (view schema upgrade, `views.rs:40-43`) | — | the new `perf.*` types join Health through their family |
| L14 | `policy.activity_state_recorded` | new event type, family `policy`, action `policy.control_state`, info; tagged | SH-1 | AO-4 | one (instance, activity profile)'s complete activity state, index key `activity_memory/<instance>/<activity profile>`: `{instance_id, profile_id, local_day, daily_count, window_id, window_count, window_runtime_ms, next_eligible_unix_ms?}` = today's `activity_daily`, `activity_window`, `activity_runtime` and `next_activity_eligible` (`policy_control.rs:88-91`); without it a restart after AO-6 would reset the activity budgets and the activity interval |

### 1.2 Fact refresh (#384, D2M §5.3 A-5, N3)

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| D1 | `policy.fact_refresh_transition` | new event type, family `policy`, action `policy.fact_refresh` (new), info; G5 copies it; not tagged | SH-1 | AO-4 (gate) after D2-S5 (recorder) | payload `{cycle_seq, instance_id, fact_key, outcome}` (`outcome` = the transition's code); envelope `capture.source` = `fact_refresh_transitions`, lineage = owner epoch, revision = transition number (from 1 per epoch, +1 per transition, never reset); a skip is `ledger_store_revision_skipped` |

### 1.3 Scheduling rules (#386, SCHED §11 X2)

| # | Shape | Kind | Code slice | First writer | Notes |
|---|---|---|---|---|---|
| S1 | policy execution record, failure part: `condition` | new optional member | SH-1 | SR-2b (SR-2a reads) | `{refunded, count, expires_at_unix_ms}`; replay only — the rerun state loads from L7 |
| S2 | `ContainedTaskCancellationReason::BlackoutStop` | new enum value (wire and ledger) | SH-1 (runtime-client 2) | SR-4b | `from_failure_code`: `blackout_run_stopped` → `BlackoutStop` |
| S3 | `RecoveryLadderSuppression::Blackout` | new enum value | SH-1 | SR-4a (SR-2a decodes) | — |

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
| V3 | task diagnostic stream (`actingcommand.runtime.task-diagnostic.v2`): `TaskDiagnosticTargetData.failure`, `TaskDiagnosticOcrData::Evaluated.located`, `TaskDiagnosticRecognitionError.outcome`; the recognition error rows' error code `TaskDiagnosticRecognitionError.code` gains the value `TextReadRefused` | three new optional members, absent when unused, and one new value | REC-1 (#396, REC-M v1.2.1 §2) | REC-1 | `failure` = `text_locate_not_found` / `text_locate_outside_cells`; `located` = `TaskDiagnosticOcrLocatedData {block_index, rect, confidence_milli, cell_index?, cell_rect?, truncated}`; `outcome` = `text_read_budget_exceeded` / `text_read_blocks_truncated`, under the code `TextReadRefused`; no ledger record type is added |
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
| E7 | `DiagnosticDetailRecord` | changed | A3 | A3 | typed tokens only: `{category, stage, backend, operation}`; `stage` takes 1 to 4 dotted segments (was 2 to 4), so a registered location fits. It is the `detail`, `primary_detail` and `cleanup_detail` of failure records, the lifecycle cause's `detail` and the device diagnostic samples' `detail`. No persisted Runtime English (M378 G6): third-party text is `raw_text` on a `foreign_*` link of the outcome (`foreign_os_error` with `os_error` when the OS gave a number, `foreign_mumu_manager_error` with `exit_code` for MuMuManager's output once its command exited, never an OS error, else `foreign_other_error`) with its `raw_text_sensitivity`, and the Runtime's own text goes only to the developer log. A record is at least `internal` when it has a diagnostic code or detail, and at least the highest `raw_text_sensitivity` in its outcome (none declared counts as `sensitive`). Text members still bridged: `LifecycleNativeDetail.text` (ADB recovery evidence, B5), the OCR texts and `InputAction::Text.text` (B4), `EmulatorVersionEvidence::Unavailable.reason` (A2-P) |
| E8 | `config.subsystems` rows | changed runtime fact rows | A3 | A3 | the manifest's `ConfigSubsystem {name, enabled, outcome}` (was `reason`); the fact row is `{name, enabled, code}` plus each scalar value of the outcome under its own key (today only `count`), and no links; the `env_overrides` row names the `variable` of each `env_override_ignored` note under `ignored_variables`, comma-separated in link order, so an ignored override is never silent (coordinator ruling 11). Codes `config_subsystem_configured`, `_configured_off`, `_flag_absent`, `_section_absent`, `_always_on`, `_discovery_bound`, `_discovery_unbound`, all `info`; [actingd-check-config.md](actingd-check-config.md), "Result" |
| E9 | public projection | codes only (a read surface, not a stored record) | A3 | A3 | `PublicPayload.outcome` is the record's outcome codes only (`Outcome::codes_only`, M378 §4.3: every node keeps its code, category and relation and only the values whose type is a code, a location or a vocabulary token; no `raw_text`, no data value); it replaces `cleanup_cause` and `failure_code`. The projected task semantic fact (terminal, entry, selection confirmation and geometry failures) and the retention `EvictionOutcome` (was `io_kind`, `raw_os_error`) carry codes-only outcomes; the lifecycle failure's summary drops `primary_detail` and `cleanup_detail`; `ProjectDiagnosticView` drops `fatal` |
| E10 | `TaskSemanticFact::TerminalCommitted.failure_severe` | new member, `bool` | A3 | A3 | right after `outcome`; serde default `false`, skipped when `false`. The failure point's own verdict that this failure must pause the task: `true` when the run poisoned the Runtime or its error was fatal (poison or `error.is_fatal()`), except `false` for a non-poisoning `input_backend_operation_failed` / `application_backend_operation_failed` of a `linear_steps` main package (R25-2), and `false` for the first-capture failures the host deliberately makes nonfatal (ADB baseline lost, a stale kept session). Only a failed terminal sets it (`invalid_task_terminal` otherwise). The policy class of a failed scheduled run is `Severe` iff `failure_severe` or the code is on the contract's static always-severe list (`is_severe_policy_failure`), through the contract fn `scheduled_failure_class`, used alike by live settlement, startup recovery and the ledger's re-derivation (coordinator ruling 9, 2026-10-13). The policy failure code is `scheduled_failure_code(outcome)`: the outcome's code, or the `raw_code` of `outcome_relay_unregistered` (coordinator ruling 7) |

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
- **A2-P:** the recognition error's `severity`/`code` strings, `EmulatorVersionEvidence::
  Unavailable.reason`, and the failed `policy.execution_recorded`'s outcome (`process.rs`
  pins it at `info` until then).
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
| T1 | task diagnostic stream (`actingcommand.runtime.task-diagnostic.v2`): `TaskDiagnosticTargetData.uniqueness` (`TaskDiagnosticUniquenessData`) | new optional member, absent when unused | REC-2 (#396, REC-M v1.2.1 §2, §4) | REC-2 | present on the row of a target that declares `unique` and otherwise passed: `TaskDiagnosticUniquenessData {score_milli, margin_milli, second_score_milli?, second_rect?}` (`deny_unknown_fields`; `second_rect` an `OcrRegionRect`), the two optional members present exactly when a second place lies within the margin; REC-1's `failure` (V3) then carries `target_match_not_unique`, and a unique search past its limit is a recognition error row (`InvalidPackage`, with its timing) whose REC-1 `outcome` (V3) carries `target_match_search_incomplete`; no new member beyond `uniqueness`, no ledger record type is added; [resource-declarations.md](resource-declarations.md), section Task schema `0.9` unique matches |

## 2 Removals and changes of option 丙 (M-PKG §4.3, §4.4)

Deleted outright, no old-version reader (M-PKG §7). The slice named deletes the field and its
readers in one PR.

| Slice | Removed or changed |
|---|---|
| PKG-S2 | `PackageAdmitted.package_sha256` (keep `package_label`, `task_label`); `EntryRecoveryPackageAdmitted/Completed/Failed.package_sha256` → `package_label`; effective configuration `EntryRecovery.package_sha256` → `package_label`; `TaskSelectionPolicy.package_sha256`; Lab declared refusal `declared_package`, `verified_package` → optional `package_label`; Lab evidence `expected_package_sha256` / `actual_package_sha256` → `package_label`; `EvidencePackage.sha256`; OCR comparison report `truth_set_sha256` and its runtime-client reader; run status `package_ref` / `recovery_packages` → `package_label` / `recovery_package_labels`; run summary digest checks; reading facts' `resource_bundle_hash` = SHA-256(`"package:" + package_id`) (shape unchanged) |
| PKG-S3 | `DispatchIntent.package_digest` (payload and storage equality); `ProcedureBinding.package_digest`; `binding_digest` tuple v3 = (`procedure_ref`, `operation_id`, `yield_points`); `PausedOn.package_digest` → content hashes parsed from the failure identity; suspension report members `paused_hash_prefix`, `current_hash_prefix`, `lifts_when: content_hash_changes`, `lifted_by: main_hash \| layer:<n> \| restart` |
| PKG-S4 | IPC `expected_sha256` of `PackageDebugRequest`, `ContainedTaskRecoveryBinding`, `ContainedTaskRequest`, the Lab operation, online observation and saved-artifact OCR requests; `PackageDebugSummary.verified_sha256`; configuration `startup_package.expected_sha256`, `prerequisite_packages[].package_digest`, `policy.procedure_manifest[].package_digest`, the resource declaration's `package_digest` |
| PKG-S5 | bundle v4 `packs[] {package_id, server, entry_task_id, path = "packs/<package_id>", file_count, byte_count}` (the bundle-level maintenance declaration stays, coordinator 10-12); `RecordingArtifact.digest` / `.sha256` → `package_id`; `PackageRef`, `ContentDirectory` and the `sha256:` serializers |

**Failure identity, format v2** (PKG-S3, M-PKG §3.4):
`<base>~v2~k<K>~m<I><H>[~p<I><H>]…[~r<Kh><I><H>]~f<F>`. `I` = the first 8 hex digits of the
SHA-256 of `package_id`; `H` = the first 12 hex digits of the package's content hash measured at
suspension (`x` when it could not be measured, D18); the `pre:<digest>` scope becomes `pre:<I>`.
A v1 string is not read. The content hash is measured only at suspension and at lift checks
(never on admission) and is stored only inside this string and in SR-3's suspension event key
`actual_digest`. A lift check of one package runs at most once per
`scheduling.suspension.lift_check_min_interval_ms` (§5).

## 3 Wire shapes handed to AO-0 (runtime-client revisions)

| # | Shape | Revision | Slice | Notes |
|---|---|---|---|---|
| W1 | `RuntimeStateSource` = `{instance_facts? {lineage, revision}, runtime_facts? {lineage, revision}, ledger_committed_head, sampled_started_at_unix_ms, sampled_completed_at_unix_ms}` | runtime-client 2 | AO-4q | replaces `{event_id, sequence}`; store fields absent before AO-3 and readers accept that; status queries stop appending records |
| W2 | `RuntimeOperation::InstanceFactSnapshot { instance }` → `RuntimeResult::InstanceFactSnapshot { snapshot, source }` | runtime-client 2 | MCP-4 | general read, one instance's facts with observation and expiry times and the resolved scope; a new kind, not a parameter of `RuntimeFactSnapshot` (that kind stays field-less); U-1's classification table gains the row |
| W3 | `ConfigureSettings { changes: [{key, value?}] }` and its receipt `{key, value, source}` | runtime-client 2 | D1b | M387 §5.6 |
| W4 | `PerformanceYield`, `BlackoutStop` | runtime-client 2 | SH-1 | P7, S2 |
| W5 | read request `ScheduleExplanation { instance }` | runtime-client 3 | MCP-19 | reserved; per task: trigger, last dispatch, next due, not-run reason, produces, consumes, pools |
| W6 | gate-class receipts carry `event_id` and store revisions, never a ledger sequence; install receipts after the commit | runtime-client 3 | AO-4 | [ledger-observer.md §13](ledger-observer.md#13-identity-without-ledger-positions-ao-3-ao-4-ao-4q-in-v0120) |
| W7 | status blocks `ledger`, `tagged_state` | runtime-client 3 | AO-8 | [ledger-observer.md §14.2](ledger-observer.md#142-status-blocks) |
| W8 | request `actingcommand.runtime.request.v4`: optional `detail` (`codes` \| `full`) | runtime-client 2 | A4 | #378 §7.1; absent, the source's class default applies (codes only for `source: ui`, the full outcome for every other source); requests v1–v3 are refused; [client-interactions.md](client-interactions.md) |
| W9 | receipt `actingcommand.runtime.receipt.v2`: `outcome` (an `actingcommand.outcome.v1` envelope) on every receipt; the v1 member `error` (`RuntimeErrorProjection`) is removed | runtime-client 2 | A4 | #378 §7.2; admitted, observed, queued and cancelled carry `request_admitted`, `request_observed`, `request_queued`, `request_cancelled`; completed carries its kind's `request_<kind>_completed`; denied and failed carry the refusal or failure on top, with the values `runtime_code`, `operation`, `host_halted`, `holder_id`, `lease_id`, `retry_after_ms` where they apply; `host_halted` travels in codes mode too; a consumer judges refusal by `state` (denied/failed), the category only says how bad it is; codes mode is applied where the receipt leaves the host |
| W10 | `RuntimeResult::ContainedTaskCompleted.outcome` → `task_result`; `ContainedTaskCancellationStatus::Terminal.outcome` → `task_result` | runtime-client 2 | A4 | #378 §8.1: renames in receipt v2 (same value), because `outcome` is reserved for the envelope |
| W11 | request kinds `BindChannel`, `UpgradeState`, `UpgradeStateStatus`, `LoadPack`, `UnloadPack` | runtime-client 2 | U-1 | UIM §8.1; their success codes come with the behaviour (U-2, U-3, U-4) |
| W12 | `RuntimeCaptureBackend::Scrcpy`, spelled `scrcpy`: `ReadonlyObservation.capture_backend` (read-only observation results and `CaptureSequence.observations`) | runtime-client 2 | #402 RD-3 | new enum value (R1 §5.1); the Lab maps it one to one to `CaptureBackendName::Scrcpy` (C1); W8–W11 are A4's (#718) |

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
| `ledger_observer.*` | the 21 keys of [ledger-observer.md §15](ledger-observer.md#15-configuration-keys-ledger_observer) | there | AO-1 | AO-1, AO-2 (v0.12.0); AO-5, AO-8 (0.12.x) |
| `recovery_ladder.*` | `enabled`, `cooldown_secs`, `trigger_defaults.start`, `.start_rung`, `.respect_cooldown`, `triggers`, `exclude_codes`, `rules.direct_runs`, `.preparation_superseded`, `.already_running`, `.cooldown_opens_on`, `.entry_unavailable`, `rungs`, `hold.claim_ttl_ms`, `hold.margin_ms` | M387 §7.1 | D1c | D1c (v0.12.0) |
| `host_claims.*` | `renewal_slack_ms` | 1 000 | D1c | D1c |
| per instance | `stuck_recovery`, `stuck_recovery_cooldown_secs` (existing keys, defaults now from the section) | M387 §5.5 | D1b | D1c |
| `performance.monitor.*`, `performance.pressure.*`, `performance.load.*` | PERF §10.1 (15 keys; the three existing `performance_monitor` keys keep their file paths) | PERF §10.1 | P-c | P-m (v0.12.0) |
| `performance.responsiveness.*` | PERF §10.2 (37 keys) | PERF §10.2 | P-c | P-m (v0.12.0) |
| `performance.pacing.*` | PERF §10.3 (26 keys; `mode` stays `observe` in every release until Alice approves) | PERF §10.3 | P-c | P-k, P-s, P-h (0.12.x) |
| `performance.calibration.*` | PERF §10.4 (18 keys) | PERF §10.4 | P-c | P-a (0.12.x) |
| `execution.defaults.*` | the kernel's fallback defaults (PERF §10.4 last notes) | today's constants | P-c | P-c |
| `scheduling.recognition.*` | `sample_timeout_retries` | 2 | SR-1 | SR-1 (v0.12.0) |
| `scheduling.rerun.*`, `scheduling.cost_commit.*`, `scheduling.blackout.*`, `scheduling.feature_lock.*`, `scheduling.not_run.*` | SCHED §9 (17 keys) | SCHED §9 | SR-1 | SR-2b, SR-4a, SR-5a, SR-10 (0.12.x); `not_run.diagnose_rows` SR-6 |
| `scheduling.pack_admission.*` | `check_in_check_config`, `check_after_start`, `suspend_codes` | SCHED §9 | SR-3 | SR-3 |
| `scheduling.fact_refresh.*` | `enabled`, `min_interval_ms`, `daily_limit`, `failure_backoff_ms`, `max_consecutive_failures`, `recognition_join_timeout_ms`, `overdue_ms` (no default: unset = not reported) | D2M §3.4 | D2-S4 | D2-S4 (v0.12.0) |
| per instance | `recognition_package` `{package}` (a location only, no digest; no default) | D2M §3.2 v1.4.2 | D2-S2 | D2-S2 |
| per instance | `start_emulator` | ORD §3.5 item 17 | S6a | S6a |
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
| `scheduling.*` (recognition, rerun, cost_commit, blackout, feature_lock, not_run, pack_admission, fact_refresh, suspension) | 29 |
| Lab options | 2 |
| per instance: `recognition_package`, `start_emulator` | 2 per instance |
| **total** | **about 223 + 4 per instance** (three instances ≈ 235; five ≈ 243) |

The total stays under 256 for up to eight instances (255 rows), with little room left; a ninth
instance passes it. So D1b implements the split (coordinator ruling, 2026-10-12): `config.parameters`
becomes one `config.parameters.<section>` fact per section (M387 §5.9), written and read that way
from D1b on, and each section fact holds at most 256 rows.

## 6 Interface revisions (declared once by A8, ORD §3.6)

`ledger` [3, 3] (A3 §1.9, SH-1, SR-2a reads, D1 ladder records, L4a, AO-1 dump import, AO-2 index
table, U-2's G14 members, PKG-S2 and PKG-S3 removals); `runtime-client` [2, 2] (§3 W1–W4, W8–W11: A4, U-1,
D2-S5, P-m); `actingd-config` 5 (§5); `runtime-outcomes` [1, 1]; `package` (PKG-S1, PKG-S5,
D2-S2/S3); `install-control` stays [0, 1] until U-4. AO-0 changes no declaration.

## 7 What AO-0 does not register, and why

| Item | Why | Who |
|---|---|---|
| the codes of M387 §6.4 item 2 (`config_key_unknown`, `config_key_not_settable`, `config_value_invalid`, `config_override_superseded`, `config_override_base_changed`, `config_override_dropped`, `recovery_ladder_config_invalid`, `recovery_ladder_code_unclassified`) | their owner crate `crates/runtime-config` does not exist yet, and a fragment must belong to a workspace member | D1b (with D1c) |
| keys `config_key`, `ladder_rule` and the vocabularies `ladder_trigger_start`, `rules.*` (M387 §6.4 items 3–4); `ladder_class` | in flight with D1a / D1b | D1a, D1b |
| vocabularies `channel`, `lab_option` and the codes of UIM §7.5 | U-0's code registration | U-0 |
| the codes of PERF §11, SCHED §10, D2M | each model's own slices register them; LEDG §11: codes and keys are registry entries, not persisted structure | P-*, SR-0, D2 slices |
| the event-type and action enum variants of §1 | the `event_type` vocabulary is built from the Rust enum (guard G7), so the types arrive with the code | SH-1 |
