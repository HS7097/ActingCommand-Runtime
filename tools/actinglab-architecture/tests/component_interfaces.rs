// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #378 A8 (order-v012 §3.6; contracts/v012-register.md §6; model-378 §10 as amended
//! by §16 A-7): the Runtime's interface declaration `distribution/windows/component-interfaces.json`
//! declares each v0.12.0 revision once, and the Windows exact-SHA build validates every interface
//! the declaration names before it copies the declaration into the Runtime and Tools manifests.
//!
//! The ranges follow the bump rule of `contracts/component-interfaces.md`: a writer of a new
//! revision raises `max`, and `min` rises where v0.12.0 no longer reads the older revision (no
//! old-version compatibility, Alice 2026-10-11).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const DECLARATION: &str = "distribution/windows/component-interfaces.json";
const BUILD_WORKFLOW: &str = ".github/workflows/windows-remote-build.yml";
const SCHEMA_VERSION: &str = "actingcommand.component-interfaces.v1";

/// The v0.12.0 declaration, one row per interface:
/// - `actingd-config` 5 (register §5): the one revision v0.12.0 raises; the keys option 丙 and
///   NAV-3a removed are refused, so no revision-4 file is read. Workflow #411 B15 R3 (v0.12.3,
///   register §1.113): revision 6 adds the optional remedy keys; a revision-5 file is still read,
///   so [5, 6]. Workflow #406 T12 (register §1.136): revision 7 adds the optional `touch`
///   section; a revision-5 or revision-6 file is still read, so [5, 7]. #411 A38 (register
///   §1.148): revision 8 adds the optional `recovery_ladder.fence_recheck` member; revisions 5
///   to 7 are still read, so [5, 8]. The 0.12.x release after v0.12.9 (#411 wave 3, register
///   §1.168 A35-5): revision 9 adds that release's configuration keys; revisions 5 to 8 are
///   still read, so [5, 9]. P0-LO (v0.12.11): revision 10 adds
///   `ledger_observer.open_timeout_ms` and `ledger_observer.open_warn_ratio_milli`; revisions 5
///   to 9 are still read, so [5, 10]. #411 A64 (v0.12.12, ruling C-43): revision 11 adds
///   `host_claims.run_end_grace_ms`, #411 A66 (same release) adds to it the `watchdog` section
///   (`start_window_ms`), PERF-B1 (Workflow #407 B1, register §1.177, same release) the
///   optional `execution` section, SCH-M ED-1 (register §1.178, same release) the
///   expected-duration keys and emu-watch the emulator watch's `emulator_control` keys;
///   revisions 5 to 10 are still read, so [5, 11]. IDLE-1 (v0.12.13, Alice's ruling 41):
///   revision 12 adds the `idle` section and an instance entry's `idle` member; ROUT-L RL-1b
///   (same release) adds to it the `standard_packs` keys `check_interval_ms` and
///   `cached_generations`, and a `policy` section without `catalog` (composer mode), LN-5 (#422,
///   same release) `ledger_observer.history_page_rows` and LN-TUNE (#422, same release)
///   `ledger_observer.writer_reply_timeout_ms`; revisions 5 to 11 are still read, so [5, 12].
///   LN-8 (#422, the release after v0.12.13): revision 13 adds
///   `ledger_observer.open_verify_tail_records`, and LN-9 (#422, same release) the scrub's
///   `ledger_observer` keys `scrub_rows_per_second`, `scrub_chunk_rows` and
///   `scrub_pass_interval_ms`; revisions 5 to 12 are still read, so [5, 13]. LN-WS (#422,
///   v0.12.15): revision 14 adds `ledger_observer.read_check_vm_steps` and
///   `ledger_observer.history_scan_rows`; revisions 5 to 13 are still read, so [5, 14].
///   Workflow #392 DA-4b-D (v0.12.16, Alice's ruling 69): revision 15 adds the `display_safe_area`
///   section and an instance entry's `display_safe_area` member; revisions 5 to 14 are still read,
///   so [5, 15]. #422 LN-7c Q (v0.12.17, Alice's ruling 70): revision 16 adds the top-level
///   `frame_retention_evidence_max_age_days` and the `ledger_observer` keys
///   `frame_query_steps_per_second` and `frame_query_chunk_frames`; revisions 5 to 15 are still
///   read, so [5, 16]. Workflow #421 INST-1 (v0.12.18): revision 17 adds the `install` section
///   (`actingctl install`'s waits, attempts, margins and guard values); revisions 5 to 16 are
///   still read, so [5, 17]. Workflow #421 INST-2 (v0.12.19, Alice's decision 75 of 10-27):
///   revision 18 adds `install.start.sample_interval_ms`, `install.start.stall_after_ms`,
///   `install.start.stall_read_bytes`, `install.start.memory_wait_ms` and
///   `install.stop.busy_wait_ms` and retires `install.guard.blocked_windows`; revisions 5 to 17
///   are still read, so [5, 18]. Workflow #427-A and #427-B (one release): revision 19 adds
///   `scheduling.parallel.enabled` (#427-A) and `performance.pacing.max_concurrent_runs`
///   (#427-B); revisions 5 to 18 are still read, so [5, 19]. RB-2 (0.12.28, Alice's ruling of
///   2026-10-29): revision 20 adds the `host` section (`host.process_memory_priority`,
///   `host.process_io_priority`), and ISO-PE (#419, same release) `scheduling.evaluation.retry_ms`
///   and `scheduling.evaluation.escalate_after`; revisions 5 to 19 are still read, so [5, 20].
///   STORE-FG-2 (0.12.29, Alice's ruling of 2026-10-29): revision 21 adds the `store_foreground`
///   section (`store_foreground.allowed_application_ids`); revisions 5 to 20 are still read, so
///   [5, 21]. Workflow #429 R-2 (0.12.31, Alice's rulings 98 and 99): revision 22 adds
///   `performance.presets`; revisions 5 to 21 are still read, so [5, 22]. SS-4 (0.12.34, Alice's
///   ruling 105 A, register §1.231): revision 23 adds `scheduling.no_progress.enabled` and an
///   instance entry's `scheduling.no_progress`; revisions 5 to 22 are still read, so [5, 23].
///   Workflow #432 AGT-1a (0.12.37, Alice's words of 2026-11-02, register §1.245): revision 24 adds
///   the optional `agent_wake` section, and ACT-1 and ACT-2 (same release, register §1.250)
///   `execution.action_gap` and an instance entry's `execution`; revisions 5 to 23 are still
///   read, so [5, 24]. Workflow #432 AGT-2b (0.12.38, Alice's words of 2026-11-02 19:4x, register
///   §1.263): revision 25 adds a rule's `on.at_minute_of_day` and `on.utc_offset_minutes`, the time
///   of a `daily_summary` rule; revisions 5 to 24 are still read (only a `daily_summary` rule, which
///   had no emitter, now needs its time), so [5, 25]. #402 HS-1 (0.12.39, Alice's words of
///   2026-11-03 11:4x, register §1.267): revision 26 relaxes `nemu_app_index` (any emulator entry
///   may name it, and Nemu touch no longer needs it); revisions 5 to 25 are still read, so
///   [5, 26]. Workflow #407 GPU-1 D (0.12.44, Alice's approval of 166, register §1.286): revision
///   27 adds `vision.execution_provider` `directml` with `vision.directml_device` and
///   `vision.gpu_fallback`; revisions 5 to 26 are still read, so [5, 27]. Revision 28 (0.12.54)
///   is one raise for two slices: #428 SCHED-C1e (register §1.299 SC1-12) adds an instance
///   entry's `scheduling.pool_lines`, and IDLE-2 (register §1.304) adds the `idle.action` value
///   `auto` and the keys `idle.return_home_min_gap_ms` and `idle.stop_emulator_min_gap_ms`
///   (section and per instance); revisions 5 to 27 are still read, so [5, 28]. Revision 29
///   (0.12.56) is one raise for two slices: Workflow #438 HELD-1 (register §1.322, §1.327) adds
///   the member `task_held` (`enabled`, `after_ms`) of the `scheduling` section and of an instance
///   entry's `scheduling`, and Workflow #437 MAPP-1 (register §1.328) adds an instance entry's
///   `device_group` and check-config's report member `device_group`; revisions 5 to 28 are still
///   read, so [5, 29]. Workflow #437 MAPP-2b (0.12.60, register §1.353) raises it to 30: the
///   member `device_group.switch_failure_cooldown_ms` of the `scheduling` section; revisions 5 to
///   29 are still read, so [5, 30]. Workflow #453 S2 (0.12.60, register §1.362) raises it to 31:
///   the member `start_paused` of an instance entry; revisions 5 to 30 are still read, so [5, 31].
/// - `install-control` stays [0, 1] (model-378 §16 A-7; U-4 raises it in 0.12.x).
/// - `install-selection` is unchanged (model-378 §10).
/// - `ledger` [3, 3]: SQLite format 2, `actingcommand.event.v3`; format 1 is refused. Workflow
///   #411 BT-LR (register §1.153) raises `max` to 4 once for the 0.12.x B train (Alice
///   2026-10-20): additive members and two task record types; a revision-3 ledger is still read,
///   so [3, 4], with no fresh state root and no migration. Workflow 114-A (0.12.36, register
///   §1.236) raises `max` to 5: the record type `policy.not_run_changed` and its payloads, shapes
///   only (no Runtime of 114-A writes one); revision-3 and revision-4 ledgers are still read, so
///   [3, 5], one way. #402 HS-2b (0.12.39, register §1.274) raises `max` to 6: HS-2's closed
///   action `device.backend_switched` and its payload, with the vocabulary
///   `nemu_app_index_source`; revision-3 to revision-5 ledgers are still read, so [3, 6], one way.
///   Workflow #404 FS-3 (0.12.43, register §1.284) raises `max` to 7: the select confirmations
///   `tracked` and `track_lost`; revision-3 to revision-6 ledgers are still read, so [3, 7], one
///   way. Workflow #428 S168-LB1a (0.12.55, register §1.309, §1.311) raises `max` to 8: a
///   `fact.published` `record_list` record of 257 to 1024 rows; revision-3 to revision-7 ledgers
///   are still read, so [3, 8], one way.
/// - `package` 3: the one `package` bump of v0.12.0 (register §6); only bundle index v4 is read.
///   Workflow #411 B15 R1 (register §1.111) raises `max` to 4 in v0.12.3: a pack's `remedy` and
///   `provides`; a revision-3 pack is still read. Workflow #406 T3 (register §1.137) raises it to
///   5: the click `single_touch_path_drag_v1`; a revision-3 or revision-4 pack is still read.
///   Workflow #357 DB-2 (0.12.37, register §1.244) raises it to 6: a selection document's `tables`
///   and the value source `table`; a document without them keeps its bytes, so earlier packs are
///   still read. Workflow #404 FS-1 and FS-2 (0.12.43, register §1.282, §1.283) raise it to 7: a
///   `repeated_anchor` anchor's `mask` and `scales` and the feature value `anchor_milli`; a pack
///   without them is read as before, so [3, 7]. FS-3 (same release, register §1.284) adds a
///   select step's `confirm: "track"` and `track_px` to revision 7, no second raise. Workflow #428
///   S168-4 and S168-5a (0.12.50, register §1.291) raise it to 8: a task operation's effect
///   `read_list` and a selection document's value source `table_max_over_list`; a pack without
///   them keeps its revision and a document without the source its bytes, so [3, 8]. Workflow
///   #428 S168-LB1b (0.12.55, register §1.312) raises it to 9: `table_max_over_list`'s optional
///   member `lower_bound_column`; a document without it keeps its bytes, so [3, 9]. Workflow #444
///   NOTICES-2 (0.12.58, register §1.335) raises it to 10: a bundle zip's notices directory
///   `notices/`, placed by the installer and opened by no reader of the Runtime; a bundle without
///   it keeps its revision, so [3, 10]. Workflow #450 FL-1 (0.12.59, register §1.343) raises it to
///   11: a task's `field_lists`, OCR fields grouped into one list fact; a pack without it keeps
///   its revision, so [3, 11]. Workflow #455 G-R1 (0.12.60, register §1.356) raises it to 12: a
///   game root's goal declaration `goals.json`, placed by the installer and opened by no reader of
///   the Runtime yet; a bundle without it keeps its revision, so [3, 12]. Workflow #463 K1
///   (0.12.60, register §1.374) raises it to 13: a `linear_steps` task's select step, which every
///   Runtime before it refuses at admission; a pack without one keeps its revision, so [3, 13].
/// - `runtime-client` [2, 2]: request v4, receipt v2; requests v1 to v3 are refused. Workflow
///   #429 R-0 (0.12.30, register §1.213) raises `max` to 3: the workbench's request kinds and
///   members; every revision-2 request is still answered as before, so [2, 3]. Workflow #429
///   R-7a (0.12.35, register §1.233) raises `max` to 4: the player hold's request kinds, results
///   and members; R-7a fills none and still answers every revision-2 and revision-3 request as
///   before, so [2, 4]. Workflow #429 R-0b (0.12.36, register §1.238) raises `min` to 3: it fills
///   the status member `RuntimeInstanceStatus.activity`, which a revision-2 reader refuses (the
///   status denies unknown members), so [3, 4]. Workflow #432 AGT-1b (0.12.37, register §1.246)
///   raises `max` to 5: the request kind `AgentWakeTest`, its result and the status member
///   `agent_wake`, which is absent with no `agent_wake` section; every earlier request is answered
///   as before, so [3, 5]. Workflow #407 PERF-READ (0.12.41, register §1.280) raises `max` to 6:
///   the `PerformanceStatus` answer's members `memory` and `vision`, which a reader whose `max` is
///   below 6 refuses (the status denies unknown members); no request changes and every earlier
///   request is answered as before, so [3, 6]. Workflow #428 S168-LB1a (0.12.55, register §1.309,
///   §1.311) raises `max` to 7: an `InstanceFactSnapshot` view with a record list of 257 to 1024
///   rows, the request's optional `key_prefix` and the answer's optional `records_total`; a
///   request without the member is answered as before, so [3, 7]. Workflow #434 LAB-6S1a
///   (0.12.58, register §1.287, "Slice LAB-6S1a") raises `max` to 8: the optional `initiator` of
///   `BindChannel`, `ChannelBound` and the receipt; a client of 3 to 7 neither sends nor receives
///   it, so [3, 8].
/// - `runtime-outcomes` [1, 1]: the new interface of model-378 §10.
const V0120: &[(&str, [u64; 2])] = &[
    ("actingd-config", [5, 31]),
    ("install-control", [0, 1]),
    ("install-selection", [1, 1]),
    ("ledger", [3, 8]),
    ("package", [3, 13]),
    ("runtime-client", [3, 8]),
    ("runtime-outcomes", [1, 1]),
];

#[test]
fn the_runtime_declares_each_v0120_interface_revision_once() {
    let declaration = read_declaration();
    assert_eq!(
        declaration
            .get("schema_version")
            .and_then(serde_json::Value::as_str),
        Some(SCHEMA_VERSION),
        "{DECLARATION}: schema_version"
    );
    let speaks = declaration
        .get("speaks")
        .and_then(serde_json::Value::as_object)
        .unwrap_or_else(|| panic!("{DECLARATION}: `speaks` is an object"));
    let declared = speaks
        .iter()
        .map(|(name, range)| {
            let range = range
                .as_array()
                .and_then(|range| match range.as_slice() {
                    [min, max] => Some([min.as_u64()?, max.as_u64()?]),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{DECLARATION}: speaks.{name} is [min, max]"));
            (name.as_str(), range)
        })
        .collect::<Vec<_>>();
    let mut required = V0120.to_vec();
    required.sort_unstable();
    let mut declared = declared;
    declared.sort_unstable();
    assert_eq!(
        declared, required,
        "{DECLARATION} declares the v0.12.0 revisions of order-v012 §3.6 and register §6"
    );
}

#[test]
fn the_exact_sha_build_validates_every_declared_interface() {
    let workflow = fs::read_to_string(workspace_root().join(BUILD_WORKFLOW))
        .unwrap_or_else(|error| panic!("read {BUILD_WORKFLOW}: {error}"));
    // The validator is the one-line `foreach ($name in @('…', …))` that follows the line
    // reading the declaration (`$interfacesPath`).
    let validated = workflow
        .lines()
        .skip_while(|line| !line.contains("$interfacesPath = "))
        .find(|line| line.trim_start().starts_with("foreach ($name in @("))
        .unwrap_or_else(|| {
            panic!("{BUILD_WORKFLOW}: no validator loop after the declaration is read")
        })
        .split('\'')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let declared = read_declaration()
        .get("speaks")
        .and_then(serde_json::Value::as_object)
        .map(|speaks| speaks.keys().cloned().collect::<BTreeSet<_>>())
        .unwrap_or_default();
    let required = V0120
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        validated, required,
        "{BUILD_WORKFLOW} validates every v0.12.0 interface (model-378 §10: runtime-outcomes joins the loop)"
    );
    assert_eq!(
        validated, declared,
        "{BUILD_WORKFLOW} validates exactly the interfaces {DECLARATION} names"
    );
}

fn read_declaration() -> serde_json::Value {
    let path = workspace_root().join(DECLARATION);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {DECLARATION}: {error}"))
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the architecture tool sits two levels below the workspace root")
        .to_path_buf()
}
