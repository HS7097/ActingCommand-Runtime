# Runtime Ledger Issue #28 L8 Acceptance

Date: 2026-07-09

Source task: `C:\合作工作区\ActingCommand\TASK-runtime-ledger-chain.md`

## Scope

This report closes the current issue #28 implementation slice for already-existing Runtime modules:

- `crates/ledger`
- `lab run`
- Lab result.zip projection
- Session request / response / journal compatibility views
- Lab-2 `observe` / `do` / `ensure` / `wait`
- direct Lab-1 semantic commands `detect-page` / `tap-target` / `navigate`
- current CLI JSON projections
- FrameStore / screenshot / recognition evidence references
- package containment light events

Excluded future modules remain out of the completion denominator: UI, full scheduler task system, agent-only protocols, database, encrypted log service, and new game logic.

## Acceptance Result

The current implemented-module fact surfaces meet the issue #28 95 percent gate.

Result: 23 / 23 current fact surfaces have a runtime-ledger path, projection path, or explicit compatibility boundary.

The only remaining compatibility artifact is the Session `request-journal.jsonl` sidecar. It is no longer treated as the sole source for completed/failed Session facts in the covered query/status paths: default journal reads, request-state, session events, queue/status diagnostics, response get/wait, and daemon-routed output consume or cross-check runtime-ledger receipts. Physical removal of the sidecar is intentionally deferred until compatibility is no longer needed.

## Fact Surface Matrix

| # | Fact surface | Status | Evidence |
|---|---|---|---|
| 1 | lab run events/reco/steps/dispatch/receipt writes | Pass | `lab run` records dispatch, drive, receipt, reco_id, action_id, evidence_id |
| 2 | lab run finish/output commit order | Pass | `finalizing` before terminal artifact work; `finish_ok` only after zip + sha256 |
| 3 | lab run output projection | Pass | result output reads completed run projection from runtime-ledger |
| 4 | Lab-2 `observe` | Pass | Lab-2 ledger receipt projection |
| 5 | Lab-2 `do` | Pass | guard miss, dry-run, real-branch, receipt reconstruction tests |
| 6 | Lab-2 `ensure` | Pass | Lab-2 receipt-backed command path |
| 7 | Lab-2 `wait` | Pass | Lab-2 receipt-backed command path |
| 8 | direct `tap-target` | Pass | semantic runtime-ledger receipt projection |
| 9 | direct `navigate` | Pass | semantic runtime-ledger receipt projection |
| 10 | direct `detect-page` | Pass | semantic runtime-ledger receipt projection |
| 11 | recognition details | Pass | `reco_id` and recognition evidence refs projected into result logs |
| 12 | action/tap/swipe step facts | Pass | `action_id` in step ledger id chain and projected step payload |
| 13 | Session request/response journal convergence | Pass with compatibility sidecar | runtime-ledger receipts drive default projection; legacy conflicts fail loudly |
| 14 | Session events/cancel/recovery/stream convergence | Pass | Session event/status surfaces use projected runtime-ledger receipts and dispatch records |
| 15 | package validate light event | Pass | containment package event ledger write |
| 16 | package inspect light event | Pass | containment package event ledger write |
| 17 | package run blocked light event | Pass | blocked package run emits ledger light event before safety error |
| 18 | screenshot/evidence persistence facts | Pass | screenshot evidence index and degradation records |
| 19 | record/step facts | Pass | step/action id ledger records |
| 20 | CLI status/queue output projection | Pass | session status diagnostics and queue consistency read runtime-ledger projections |
| 21 | CLI detect-page / semantic projection | Pass | direct semantic command outputs project from receipts |
| 22 | result.zip logs/screenshots/evidence refs | Pass | result.zip logs/evidence projections read from runtime-ledger/evidence refs |
| 23 | diagnostics and ledger query commands | Pass | `ledger show/events/receipts/diagnose/evidence` query existing ledger/evidence files only |

## Adversarial Checks

| Required counterexample | Covered by |
|---|---|
| `--out` parent path is a regular file | `zip_failure_after_success_does_not_record_finish_ok` |
| `write_logs` failure | `write_logs_failure_does_not_record_finish_ok` |
| zip write failure before terminal receipt | `zip_failure_after_success_does_not_record_finish_ok` |
| zip exists but sha256 mismatches | `completed_projection_rejects_finish_ok_with_output_zip_sha256_mismatch` |
| ledger/run-root unavailable or no run root | explicit `run_root_not_configured` and fail-loud ledger write paths |
| corrupt ledger tail | `ledger_writes_flushes_and_skips_corrupt_tail` and ledger query diagnostics |
| CLI success but missing receipt | `completed_projection_requires_terminal_receipt`, `session_response_get_requires_runtime_ledger_receipt` |
| `finish_ok` but zip missing | `completed_projection_rejects_finish_ok_with_missing_output_zip_file` |
| Session journal/ledger conflict | `session_request_state_rejects_journal_ledger_status_conflict`, `session_journal_and_events_reject_legacy_ledger_status_conflict` |
| Lab-2 `do` blocked without fake success | `lab2_do_guard_miss_returns_actionable_error_details` |
| old direct semantic entry bypasses ledger | `lab1_direct_semantic_commands_write_runtime_ledger_receipts` |
| ledger/evidence query visibility | `ledger_query_commands_read_records_events_receipts_diagnostics_and_evidence` |

## Verification Commands

- `cargo test -p actingcommand-actinglab completed_projection_requires_finalizing_record -- --nocapture`
- `cargo test -p actingcommand-actinglab completed_projection_requires_terminal_receipt -- --nocapture`
- `cargo test -p actingcommand-actinglab completed_projection_rejects_finish_ok_with_missing_output_zip_file -- --nocapture`
- `cargo test -p actingcommand-actinglab completed_projection_rejects_finish_ok_with_output_zip_sha256_mismatch -- --nocapture`
- `cargo test -p actingcommand-actinglab zip_failure_after_success_does_not_record_finish_ok -- --nocapture`
- `cargo test -p actingcommand-actinglab write_logs_failure_does_not_record_finish_ok -- --nocapture`
- `cargo test -p actingcommand-actinglab session_response_get_requires_runtime_ledger_receipt -- --nocapture`
- `cargo test -p actingcommand-actinglab lab2_do_guard_miss_returns_actionable_error_details -- --nocapture`
- `cargo test -p actingcommand-actinglab lab1_direct_semantic_commands_write_runtime_ledger_receipts -- --nocapture`
- `cargo test -p actingcommand-actinglab ledger_query_commands_read_records_events_receipts_diagnostics_and_evidence -- --nocapture`
- `cargo test -p actingcommand-actinglab session_status_diagnostics_projects_ledger_only_receipts -- --nocapture`
- `cargo fmt --all -- --check`
- `git diff --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- `cargo build --release`

All listed verification commands passed.

## Remaining Tail

- Keep the Session `request-journal.jsonl` compatibility sidecar until a later compatibility-removal milestone.
- Future modules that were explicitly excluded from issue #28 still need their own runtime-ledger integration when implemented.
