// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 S3-15a (register §1.451; contracts/standard-pack-schedule.md, "Where a pack stands:
// `standard-pack status`"; contracts/install-cli.md, "What the placements enabled"; coordinator
// work order `rd-1116/brief-463-s315a-status-installation.md`): `standard-pack status
// --installation` answers the authorization each pack was enabled under, from the installation's
// placement journals, and each pair's suspension and break-in, from one run of its `actingd
// suspended`.
// - `s315_a_pack_placed_under_a_journal_that_names_it_answers_its_authorization`: the journal's
//   reference, initiator, finishing time and operation, and the root's count of the journals read;
// - `s315_of_two_placements_that_name_a_pack_the_later_answers`;
// - `s315_a_later_content_only_placement_changes_nothing`: a placement whose `enabled` names no
//   pack leaves the earlier one standing;
// - `s315_an_undone_placement_and_one_that_placed_nothing_do_not_count`: the earlier one answers,
//   and a pack only a placement that ended before placing names answers `no_journal_names_it`;
// - `s315_a_pack_no_journal_names_answers_not_recorded_no_journal_names_it`: a pack a rule enables,
//   named only by an unfinished placement and by one of another game;
// - `s315_an_unreadable_journal_is_listed_and_hides_no_other`: the root lists it, the other packs
//   are answered, a pack no readable journal names answers `journals_unreadable`;
// - `s315_without_installation_the_answer_is_s314s_byte_for_byte`: the command line without
//   `--installation` answers S3-14's function's answer, byte for byte, and none of the new members;
// - `s315_pairs_show_a_suspension_and_a_break_in_and_a_pack_without_pairs_is_empty`: from a report
//   as `actingd suspended` prints it and the composed bindings;
// - `s315_a_report_not_read_leaves_the_word_the_outcome_and_the_authorization`: the child that
//   cannot be started (the installation holds no actingd), the outcome a diagnosis gives a child
//   that failed and one it ended at its deadline, and bindings that could not be read;
// - `s315_a_root_not_the_installations_and_a_build_without_the_rescue_module_are_refused`;
// - `s315_the_row_writes_nothing`.
// The installation is a made-up single-location root whose `pack_place` journals are written as
// the rescue module's `place` writes them; the child is the installation's own `actingd
// suspended`, started as `diagnose` starts it, which this root does not hold. No Runtime, no
// device, no clock, no sleep; made-up packs only.

use super::env_lock;
use super::gate_434_lab5b4::Isolated;
use crate::standard_pack_cli::read_installation;
use crate::{ErrorKind, run_cli};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_lab_ops::codes::ActingctlCode;
use actingcommand_lab_ops::locate::Location;
use actingcommand_resource_tooling::{
    AuthorizationUnrecorded, EnabledAuthorization, InstallationJournals, InstallationPairs,
    PackAuthorization, PackPairs, PackStatus, PairsUnread, PlacementAuthorization,
    StandardPackStatus, StandardPackStatusRequest, StatusInstallation, standard_pack_status,
    standard_pack_status_with, status_installation_unbuilt,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
const OTHER_GAME: &str = "neutral-other";
const SERVER: &str = "test";
/// The return-home package of the server, a supporting part no rule dispatches.
const HOME: &str = "neutral.test.home";
/// Packs a rule enables each.
const PLACED: &str = "neutral.test.placed";
const TWICE: &str = "neutral.test.twice";
const UNDONE: &str = "neutral.test.undone";
const RUNS: &str = "neutral.test.runs";
/// The pool the rules produce into, with a valuation.
const VALUED: &str = "pool.valued";
/// The initiators the journals record.
const LAB_CLI: &str = "Lab-CLI";
const LAB_MCP: &str = "Lab-MCP";
/// The references of the authorizations the placements named (made-up board references).
const FIRST: &str = "board item 463, comment 0000000001";
const SECOND: &str = "board item 463, comment 0000000002";
const CONTENT_ONLY: &str = "board item 463, comment 0000000003";
const THEN_UNDONE: &str = "board item 463, comment 0000000004";
const UNFINISHED: &str = "board item 463, comment 0000000005";
const PLACED_NOTHING: &str = "board item 463, comment 0000000006";
const OF_ANOTHER_GAME: &str = "board item 463, comment 0000000007";
/// A placement's journal records its end this long after its own milliseconds.
const FINISHED_AFTER_MS: u64 = 5;
/// The operations the fixture's journals record, the two of the first two placements.
const FIRST_MS: u64 = 1_000;
const SECOND_MS: u64 = 2_000;
/// The journal that cannot be read.
const UNREADABLE_MS: u64 = 8_000;
/// The instances of the report's pairs.
const NODE_A: &str = "node-a";
const NODE_B: &str = "node-b";
const NODE_C: &str = "node-c";

/// How a placement's journal ends.
#[derive(Debug, Clone, Copy)]
enum End {
    /// It finished with every item placed.
    Placed,
    /// It finished before it placed an item (its instances not idle in time): its item planned.
    PlacedNothing,
    /// It placed its item and was undone.
    Undone,
    /// It never finished: no end recorded.
    Unfinished,
}

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("directory");
    fs::write(path, serde_json::to_vec_pretty(value).expect("JSON")).expect("file");
}

fn probe(page: &str, expected: [u8; 3]) -> Value {
    json!({
        "id": format!("{page}_colour"),
        "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
        "expected": expected
    })
}

/// The pack `package_id` in `directory`, as S3-14's gate writes it: a 64x24 page graph whose page
/// `home` is told by a blue first pixel and `page_b` by a red one, one trusted click between them.
fn write_pack(directory: &Path, package_id: &str) {
    write_json(
        &directory.join("control.json"),
        &json!({
            "schema_version": "Lab-1y.control.v2",
            "package_id": package_id,
            "execution_mode": "navigable_route",
            "game": GAME,
            "server": SERVER,
            "resolution": {"width": 64, "height": 24},
            "entry_task_id": "task",
            "capture_interval_ms": 10,
            "step_timeout_ms": 600,
            "timeout_ms": 8000,
            "max_steps": 2
        }),
    );
    write_json(
        &directory.join("resources/operations/resources.json"),
        &json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
    );
    write_json(
        &directory.join("resources/operations/task/task.json"),
        &json!({
            "schema_version": "0.9",
            "task_id": "task",
            "game": GAME,
            "server_scope": [SERVER],
            "locale": "en-US",
            "goal": "S3-15a fixture",
            "coordinate_space": {"width": 64, "height": 24},
            "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
            "color_probes": [probe("home", [0, 0, 255]), probe("page_b", [255, 0, 0])],
            "page_rules": {
                "home": {"required": ["home_colour"]},
                "page_b": {"required": ["page_b_colour"]}
            },
            "entry_page": "home",
            "target_page": "page_b",
            "operations": [{
                "id": "go", "from": "home", "to": "page_b",
                "click": {"kind": "point", "x": 1, "y": 0},
                "unguarded_trusted_coordinate": true, "retryable": false
            }]
        }),
    );
}

/// The id of the rule of `package_id`, the catalog task one instance per server composes for it.
fn rule_id(package_id: &str) -> String {
    format!("{package_id}.rule")
}

/// The rule of `package_id` on an hourly clock that produces one unit into the valued pool.
fn rule(package_id: &str) -> Value {
    let id = rule_id(package_id);
    json!({
        "id": id,
        "package_id": package_id,
        "priority": 100,
        "trigger": {
            "kind": "clock",
            "schedule": {
                "kind": "interval",
                "clock_source": {"kind": "local"},
                "every_ms": 3600000,
                "anchor_ms": 0
            }
        },
        "feedback_stop": {
            "kind": "outcome",
            "task_id": id,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [],
        "produces": [{
            "pool_id": VALUED,
            "direction": "produce",
            "amount": 1,
            "observation_source": "scan_verified",
            "confidence_milli": 1000
        }],
        "on_failure": {
            "action": "continue",
            "retry_limit": 2,
            "retry_backoff_ms": 30000,
            "escalation_threshold": 3
        },
        "sensitive": false,
        "next_run_clamp_ms": 86400000,
        "yield_points": [],
        "expected_duration_ms": 60000,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "light"},
        "strategic_weight_milli": 1000
    })
}

/// The root's `schedule.json`: a rule for each pack but the return-home one, and the valued pool.
fn schedule() -> Value {
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "rules": [rule(PLACED), rule(TWICE), rule(UNDONE), rule(RUNS)],
        "pools": [{
            "id": VALUED,
            "capacity": 1000,
            "projection": {"kind": "none"},
            "observation": {"kind": "fact", "fact_key": "resource.valued"},
            "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
            "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
            "valuation": {
                "name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000
            }
        }],
        "activity": [],
        "timeline": []
    })
}

/// The `pack_place` journal of the operation `op-<ms>` of the root `root`, as the rescue module's
/// `place` writes one with no Runtime (the inputs `cold` and `bound`, the authorization and the
/// packages it enables), its one item and its check after the placement, ended as `end` says.
fn journal(root: &Path, ms: u64, game: &str, reference: &str, enabled: &[&str], end: End) -> Value {
    let (item, check) = match end {
        End::Placed => ("done", "done"),
        End::PlacedNothing => ("planned", "planned"),
        End::Undone => ("undone", "undone"),
        End::Unfinished => ("done", "started"),
    };
    let finished = match end {
        End::Unfinished => Value::Null,
        End::Placed | End::PlacedNothing | End::Undone => json!(ms + FINISHED_AFTER_MS),
    };
    let undo = match end {
        End::Undone => json!({
            "started_at_unix_ms": ms + 10,
            "finished_at_unix_ms": ms + 12,
            "initiator": LAB_CLI
        }),
        End::Placed | End::PlacedNothing | End::Unfinished => Value::Null,
    };
    let initiator = if ms == SECOND_MS { LAB_MCP } else { LAB_CLI };
    json!({
        "schema_version": "actingcommand.install-operation.v1",
        "operation_id": format!("op-{ms}"),
        "kind": "pack_place",
        "root": root.display().to_string(),
        "installer": {
            "commit": null,
            "path": root.join("tools").join("actinglab.exe").display().to_string()
        },
        "initiator": initiator,
        "inputs": {
            "commit": null,
            "game": game,
            "zip": "neutral-bundle.zip",
            "zip_sha256": "0".repeat(64),
            "config_sha256": format!("sha256:{}", "0".repeat(64)),
            "cold": true,
            "bound": [],
            "authorization": reference,
            "enabled": enabled
        },
        "plan_id": "plan-0123456789abcdef",
        "plan": null,
        "guards": [],
        "acknowledged": [],
        "s0": null,
        "previous": {"live": null, "state": null},
        "steps": [
            {
                "n": 1,
                "kind": "pack_item",
                "target": root.join("packages").join(game).display().to_string(),
                "intent": {},
                "state": item,
                "evidence": null
            },
            {
                "n": 2,
                "kind": "post_check",
                "target": "runtime",
                "intent": {"place": true},
                "state": check,
                "evidence": null
            }
        ],
        "undo": undo,
        "result": null,
        "finished_at_unix_ms": finished
    })
}

/// The made-up installation `<temp>/installation`, its configuration naming the standard-pack
/// root `packages` and a state root that does not exist; its game root `packages/neutral` with
/// five packs (the return-home one and four a rule enables); its journals, oldest first:
/// - `op-1000` placed [`PLACED`], [`TWICE`] and [`UNDONE`] under [`FIRST`] (`Lab-CLI`);
/// - `op-2000` placed [`TWICE`] under [`SECOND`] (`Lab-MCP`);
/// - `op-3000` replaced a content under [`CONTENT_ONLY`], enabling nothing;
/// - `op-4000` placed [`UNDONE`] under [`THEN_UNDONE`], and was undone;
/// - `op-5000` names [`RUNS`] under [`UNFINISHED`] and never finished;
/// - `op-6000` names [`HOME`] under [`PLACED_NOTHING`] and ended before it placed anything;
/// - `op-7000` placed [`RUNS`] of another game under [`OF_ANOTHER_GAME`].
///
/// And the Lab's state directory beside it, empty. The installation holds no program, so its
/// `actingd suspended` cannot be started.
struct Fixture {
    temp: TempDir,
    root: PathBuf,
    game_root: PathBuf,
    state: PathBuf,
}

impl Fixture {
    fn write() -> Self {
        let temp = TempDir::new().expect("temporary directory");
        let root = temp.path().join("installation");
        let packages = root.join("packages");
        let game_root = packages.join(GAME);
        let mut listed = Vec::new();
        for package_id in [HOME, PLACED, TWICE, UNDONE, RUNS] {
            write_pack(&game_root.join(package_id), package_id);
            listed.push(json!({
                "package_id": package_id, "server": SERVER, "entry_task_id": "task",
                "path": format!("packs/{package_id}"), "file_count": 3, "byte_count": 64
            }));
        }
        write_json(
            &game_root.join("bundle.json"),
            &json!({
                "schema_version": "actingcommand.bundle.v4",
                "game": GAME,
                "packs": listed,
                "maintenance": [{"package_id": HOME, "server": SERVER, "uses": ["return_home"]}]
            }),
        );
        write_json(&game_root.join("schedule.json"), &schedule());
        write_json(
            &root.join("actingd.config.json"),
            &json!({
                "state_root": temp.path().join("state-root"),
                "instances": [],
                "standard_packs": {"root": packages}
            }),
        );
        let placements = [
            (
                FIRST_MS,
                GAME,
                FIRST,
                vec![PLACED, TWICE, UNDONE],
                End::Placed,
            ),
            (SECOND_MS, GAME, SECOND, vec![TWICE], End::Placed),
            (3_000, GAME, CONTENT_ONLY, Vec::new(), End::Placed),
            (4_000, GAME, THEN_UNDONE, vec![UNDONE], End::Undone),
            (5_000, GAME, UNFINISHED, vec![RUNS], End::Unfinished),
            (6_000, GAME, PLACED_NOTHING, vec![HOME], End::PlacedNothing),
            (7_000, OTHER_GAME, OF_ANOTHER_GAME, vec![RUNS], End::Placed),
        ];
        for (ms, game, reference, enabled, end) in placements {
            write_json(
                &Self::journal_path(&root, ms),
                &journal(&root, ms, game, reference, &enabled, end),
            );
        }
        let state = temp.path().join("lab-state");
        fs::create_dir_all(&state).expect("the state directory");
        Self {
            temp,
            root,
            game_root,
            state,
        }
    }

    fn journal_path(root: &Path, ms: u64) -> PathBuf {
        root.join("install")
            .join("operations")
            .join(format!("op-{ms}.json"))
    }

    /// Adds `op-8000`, a file of a journal's name that is no journal.
    fn add_unreadable_journal(&self) {
        write_json(
            &Self::journal_path(&self.root, UNREADABLE_MS),
            &json!({"neutral": "not a journal"}),
        );
    }

    /// The installation as the rescue module locates one, its configuration `config`.
    fn location_with(&self, config: PathBuf) -> Location {
        Location {
            root: self.root.clone(),
            installation: None,
            config,
            state_root: self.temp.path().join("state-root"),
        }
    }

    fn location(&self) -> Location {
        self.location_with(self.root.join("actingd.config.json"))
    }

    fn request(&self) -> StandardPackStatusRequest {
        StandardPackStatusRequest {
            root: self.game_root.clone(),
            repo: None,
            package_id: None,
            state_dir: self.state.clone(),
        }
    }

    /// What `--installation` reads of the installation for its game root, as the row reads it.
    fn installation(&self) -> StatusInstallation {
        read_installation(&self.location(), &self.game_root)
            .unwrap_or_else(|error| panic!("the installation: {}: {}", error.code, error.message))
    }

    /// The status of the game root with `installation`, which must pass.
    fn status_with(&self, installation: &StatusInstallation) -> StandardPackStatus {
        standard_pack_status_with(&self.request(), Some(installation))
            .unwrap_or_else(|error| panic!("the status: {}: {}", error.code, error.message))
    }

    /// The row's answer with `--installation`.
    fn installed(&self) -> StandardPackStatus {
        self.status_with(&self.installation())
    }
}

/// The row of the pack `package_id`.
fn row<'a>(answer: &'a StandardPackStatus, package_id: &str) -> &'a PackStatus {
    answer
        .packs
        .iter()
        .find(|row| row.package_id == package_id)
        .unwrap_or_else(|| panic!("{package_id} is answered: {answer:?}"))
}

/// The authorization the journal of the placement `op-<ms>` records, under `reference`.
fn recorded(reference: &str, initiator: &str, ms: u64) -> EnabledAuthorization {
    EnabledAuthorization::Recorded(PlacementAuthorization {
        reference: reference.to_owned(),
        initiator: Some(initiator.to_owned()),
        at_unix_ms: ms + FINISHED_AFTER_MS,
        operation_id: format!("op-{ms}"),
    })
}

/// The pack's authorization and the word that says why none is recorded.
fn unrecorded(status: &PackStatus) -> (&EnabledAuthorization, Option<AuthorizationUnrecorded>) {
    (
        &status.enabled.authorization,
        status.enabled.authorization_unrecorded,
    )
}

const NOT_RECORDED: EnabledAuthorization =
    EnabledAuthorization::Unrecorded(PackAuthorization::NotRecorded);

/// The member names of a JSON object.
fn names(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .map(|members| members.keys().cloned().collect())
        .unwrap_or_default()
}

/// Every directory and file below `directory`, each file with its bytes.
fn tree(directory: &Path, into: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
    for entry in fs::read_dir(directory).expect("a directory") {
        let path = entry.expect("an entry").path();
        if path.is_dir() {
            tree(&path, into);
            into.insert(path, None);
        } else {
            let bytes = fs::read(&path).expect("a file");
            into.insert(path, Some(bytes));
        }
    }
}

#[test]
fn s315_a_pack_placed_under_a_journal_that_names_it_answers_its_authorization() {
    let _guard = env_lock();
    let fixture = Fixture::write();

    let answer = fixture.installed();

    let placed = row(&answer, PLACED);
    assert_eq!(
        placed.enabled.authorization,
        recorded(FIRST, LAB_CLI, FIRST_MS)
    );
    assert_eq!(placed.enabled.authorization_unrecorded, None);
    let serialized = serde_json::to_value(placed).expect("the row");
    assert_eq!(
        serialized["enabled"],
        json!({
            "state": "enabled",
            "authorization": {
                "reference": FIRST,
                "initiator": LAB_CLI,
                "at_unix_ms": FIRST_MS + FINISHED_AFTER_MS,
                "operation": "op-1000"
            }
        })
    );
    let installation = answer.installation.as_ref().expect("the root's part");
    assert_eq!(installation.operations_read, 7, "{installation:?}");
    assert!(
        installation.operations_unread.is_empty(),
        "{installation:?}"
    );
}

#[test]
fn s315_of_two_placements_that_name_a_pack_the_later_answers() {
    let _guard = env_lock();
    let fixture = Fixture::write();

    let answer = fixture.installed();

    assert_eq!(
        row(&answer, TWICE).enabled.authorization,
        recorded(SECOND, LAB_MCP, SECOND_MS)
    );
    assert_eq!(
        serde_json::to_value(row(&answer, TWICE)).expect("the row")["enabled"]["authorization"]["operation"],
        "op-2000"
    );
}

#[test]
fn s315_a_later_content_only_placement_changes_nothing() {
    let _guard = env_lock();
    let fixture = Fixture::write();
    // `op-3000` finished with its item placed after `op-1000`, and enables nothing.
    let bytes = fs::read(Fixture::journal_path(&fixture.root, 3_000)).expect("op-3000");
    let content_only: Value = serde_json::from_slice(&bytes).expect("op-3000 reads");
    assert_eq!(content_only["inputs"]["enabled"], json!([]));
    assert_eq!(content_only["inputs"]["authorization"], CONTENT_ONLY);

    let answer = fixture.installed();

    for package_id in [PLACED, UNDONE] {
        assert_eq!(
            row(&answer, package_id).enabled.authorization,
            recorded(FIRST, LAB_CLI, FIRST_MS),
            "{package_id}"
        );
    }
}

#[test]
fn s315_an_undone_placement_and_one_that_placed_nothing_do_not_count() {
    let _guard = env_lock();
    let fixture = Fixture::write();

    let answer = fixture.installed();

    // `op-4000` named it later and was undone: the earlier placement answers.
    assert_eq!(
        row(&answer, UNDONE).enabled.authorization,
        recorded(FIRST, LAB_CLI, FIRST_MS)
    );
    // `op-6000` alone names the return-home pack and placed nothing: no journal names it.
    assert_eq!(
        unrecorded(row(&answer, HOME)),
        (
            &NOT_RECORDED,
            Some(AuthorizationUnrecorded::NoJournalNamesIt)
        )
    );
}

#[test]
fn s315_a_pack_no_journal_names_answers_not_recorded_no_journal_names_it() {
    let _guard = env_lock();
    let fixture = Fixture::write();

    let answer = fixture.installed();

    let runs = row(&answer, RUNS);
    assert_eq!(
        runs.enabled.state,
        actingcommand_resource_tooling::EnabledState::Enabled
    );
    assert_eq!(
        unrecorded(runs),
        (
            &NOT_RECORDED,
            Some(AuthorizationUnrecorded::NoJournalNamesIt)
        )
    );
    let serialized = serde_json::to_value(runs).expect("the row");
    assert_eq!(
        serialized["enabled"],
        json!({
            "state": "enabled",
            "authorization": "not_recorded",
            "authorization_unrecorded": "no_journal_names_it"
        })
    );
}

#[test]
fn s315_an_unreadable_journal_is_listed_and_hides_no_other() {
    let _guard = env_lock();
    let fixture = Fixture::write();
    fixture.add_unreadable_journal();

    let answer = fixture.installed();

    let installation = answer.installation.as_ref().expect("the root's part");
    assert_eq!(installation.operations_read, 7, "{installation:?}");
    assert_eq!(
        installation.operations_unread,
        vec![format!("op-{UNREADABLE_MS}")]
    );
    assert_eq!(
        row(&answer, PLACED).enabled.authorization,
        recorded(FIRST, LAB_CLI, FIRST_MS)
    );
    assert_eq!(
        row(&answer, TWICE).enabled.authorization,
        recorded(SECOND, LAB_MCP, SECOND_MS)
    );
    for package_id in [RUNS, HOME] {
        assert_eq!(
            unrecorded(row(&answer, package_id)),
            (
                &NOT_RECORDED,
                Some(AuthorizationUnrecorded::JournalsUnreadable)
            ),
            "{package_id}"
        );
    }
    let serialized = serde_json::to_value(&answer).expect("the answer");
    assert_eq!(
        serialized["installation"]["operations_unread"],
        json!(["op-8000"])
    );
}

#[test]
fn s315_without_installation_the_answer_is_s314s_byte_for_byte() {
    let _guard = env_lock();
    let _isolated = Isolated::new();
    let fixture = Fixture::write();
    let root = fixture.game_root.display().to_string();
    let state = fixture.state.display().to_string();
    let line = [
        "--json",
        "standard-pack",
        "status",
        "--root",
        root.as_str(),
        "--state-dir",
        state.as_str(),
    ];

    let result = run_cli(line, true);

    let shown = result.envelope_json();
    assert_eq!(result.exit_code(), 0, "{shown}");
    let data = result
        .envelope
        .data
        .clone()
        .unwrap_or_else(|| panic!("the status answers: {shown}"));
    let answer = standard_pack_status(&fixture.request())
        .unwrap_or_else(|error| panic!("the status: {}: {}", error.code, error.message));
    let expected = serde_json::to_value(&answer).expect("the answer");
    assert_eq!(
        serde_json::to_vec(&data).expect("the line's answer"),
        serde_json::to_vec(&expected).expect("the function's answer"),
        "{shown}"
    );
    assert_eq!(
        names(&data),
        [
            "enabled_states",
            "game",
            "packs",
            "root",
            "stage_states",
            "state_dir"
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>(),
        "{shown}"
    );
    let rows = data["packs"].as_array().expect("the rows");
    assert_eq!(rows.len(), 5, "{shown}");
    for pack in rows {
        assert!(pack.get("pairs").is_none(), "{pack}");
        assert_eq!(pack["enabled"]["authorization"], "not_recorded", "{pack}");
        assert_eq!(
            pack["enabled"]["authorization_unrecorded"], "journals_not_read",
            "{pack}"
        );
        assert_eq!(
            names(&pack["enabled"]),
            ["authorization", "authorization_unrecorded", "state"]
                .into_iter()
                .map(str::to_owned)
                .collect::<BTreeSet<_>>(),
            "{pack}"
        );
    }
}

#[test]
fn s315_pairs_show_a_suspension_and_a_break_in_and_a_pack_without_pairs_is_empty() {
    let fixture = Fixture::write();
    let break_in = json!({
        "content_hash_prefix": "0123456789ab",
        "runs": 3,
        "runs_needed": 20,
        "first_settled_at_unix_ms": 1000,
        "since_first_ms": 5000,
        "ms_needed": 604800000,
        "in_break_in": true
    });
    let past = json!({
        "content_hash_prefix": "ba9876543210",
        "runs": 20,
        "runs_needed": 20,
        "first_settled_at_unix_ms": 1000,
        "since_first_ms": 700000000,
        "ms_needed": 604800000,
        "in_break_in": false
    });
    let with_pair = |row: &Value, task_id: &str, instance_id: &str| {
        let mut row = row.clone();
        row["task_id"] = json!(task_id);
        row["instance_id"] = json!(instance_id);
        row
    };
    let twice_task = format!("{}@{NODE_B}", rule_id(TWICE));
    let report = json!({
        "schema_version": "actingcommand.actingd.suspended.v2",
        "outcome": {
            "schema_version": "actingcommand.outcome.v1",
            "code": "suspended_reported",
            "category": "success",
            "values": {}
        },
        "state_root": fixture.temp.path().join("state-root"),
        "through_sequence": 42,
        "suspended": [
            {
                "task_id": rule_id(PLACED),
                "instance_id": NODE_A,
                "procedure_ref": format!("package.{PLACED}"),
                "pause_cause": "unrecorded_screen",
                "effective_class": "severe"
            },
            {
                "task_id": rule_id(RUNS),
                "instance_id": NODE_C,
                "procedure_ref": format!("package.{RUNS}")
            }
        ],
        "break_ins": [
            with_pair(&break_in, rule_id(PLACED).as_str(), NODE_A),
            with_pair(&past, twice_task.as_str(), NODE_B),
            with_pair(&break_in, "neutral.test.ghost.rule", NODE_A)
        ],
        "warnings": ["config_newer_than_daemon_start", "ledger_read_incomplete"]
    });
    let bindings = BTreeMap::from([
        (rule_id(PLACED), PLACED.to_owned()),
        (twice_task.clone(), TWICE.to_owned()),
        (rule_id(RUNS), RUNS.to_owned()),
    ]);
    let installation = StatusInstallation {
        journals: InstallationJournals::default(),
        pairs: InstallationPairs::Read {
            report,
            bindings: Ok(bindings),
        },
    };

    let answer = fixture.status_with(&installation);

    let pairs = |package_id: &str| {
        serde_json::to_value(&row(&answer, package_id).pairs).expect("the pairs")
    };
    // A suspended pair in its break-in period: its pause cause and its break-in row, the report's
    // members as the report names them.
    assert_eq!(
        pairs(PLACED),
        json!([{
            "task_id": rule_id(PLACED),
            "instance_id": NODE_A,
            "suspended": {"pause_cause": "unrecorded_screen"},
            "break_in": break_in
        }])
    );
    // A pair past its break-in, composed for two instances, not suspended.
    assert_eq!(
        pairs(TWICE),
        json!([{"task_id": twice_task, "instance_id": NODE_B, "break_in": past}])
    );
    // A suspended pair whose row names no pause cause, and no break-in.
    assert_eq!(
        pairs(RUNS),
        json!([{"task_id": rule_id(RUNS), "instance_id": NODE_C, "suspended": {}}])
    );
    // No pair of the report is the pack's: an empty list.
    for package_id in [UNDONE, HOME] {
        assert_eq!(
            row(&answer, package_id).pairs,
            Some(PackPairs::Read(Vec::new())),
            "{package_id}"
        );
    }
    assert_eq!(answer.warnings, vec!["ledger_read_incomplete".to_owned()]);
    let installation = answer.installation.as_ref().expect("the root's part");
    assert!(installation.suspension_report_read, "{installation:?}");
    assert_eq!(installation.pairs_unread, None);
    assert!(installation.pairs_unread_outcome.is_none());
    let serialized = serde_json::to_value(&answer).expect("the answer");
    assert_eq!(
        names(&serialized["installation"]),
        [
            "operations_read",
            "operations_unread",
            "suspension_report_read"
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
    );
    assert_eq!(serialized["warnings"], json!(["ledger_read_incomplete"]));
}

#[test]
fn s315_a_report_not_read_leaves_the_word_the_outcome_and_the_authorization() {
    let _guard = env_lock();
    let fixture = Fixture::write();

    // The installation holds no actingd: the child cannot be started.
    let read = fixture.installation();
    let InstallationPairs::ReportNotRead(outcome) = &read.pairs else {
        panic!("the report was read: {:?}", read.pairs);
    };
    assert!(
        outcome.is(ActingctlCode::SuspendedReportSpawnFailed),
        "{outcome:?}"
    );
    let failed = || Outcome::new(ActingctlCode::SuspendedReportFailed);
    let ended = || Outcome::new(ActingctlCode::SuspendedReportTimeout);
    let cases = [
        (
            "not started",
            read.clone(),
            ActingctlCode::SuspendedReportSpawnFailed,
        ),
        (
            "failed",
            StatusInstallation {
                journals: read.journals.clone(),
                pairs: InstallationPairs::ReportNotRead(failed()),
            },
            ActingctlCode::SuspendedReportFailed,
        ),
        (
            "ended at its deadline",
            StatusInstallation {
                journals: read.journals.clone(),
                pairs: InstallationPairs::ReportNotRead(ended()),
            },
            ActingctlCode::SuspendedReportTimeout,
        ),
    ];
    for (case, installation, code) in cases {
        let answer = fixture.status_with(&installation);
        let part = answer.installation.as_ref().expect("the root's part");
        assert!(!part.suspension_report_read, "{case}: {part:?}");
        assert_eq!(
            part.pairs_unread,
            Some(PairsUnread::SuspensionReportNotRead),
            "{case}"
        );
        assert!(
            part.pairs_unread_outcome
                .as_ref()
                .is_some_and(|outcome| outcome.is(code)),
            "{case}: {part:?}"
        );
        assert!(answer.warnings.is_empty(), "{case}");
        for pack in &answer.packs {
            assert_eq!(
                pack.pairs,
                Some(PackPairs::NotRead(PairsUnread::SuspensionReportNotRead)),
                "{case}: {}",
                pack.package_id
            );
        }
        // The journals do not depend on the report.
        assert_eq!(
            row(&answer, PLACED).enabled.authorization,
            recorded(FIRST, LAB_CLI, FIRST_MS),
            "{case}"
        );
        let serialized = serde_json::to_value(&answer).expect("the answer");
        assert_eq!(
            serialized["packs"][0]["pairs"], "suspension_report_not_read",
            "{case}"
        );
        assert_eq!(
            serialized["installation"]["pairs_unread"], "suspension_report_not_read",
            "{case}"
        );
    }

    // The report read, the composed bindings not: their own word, the report's warning passed on.
    let installation = StatusInstallation {
        journals: read.journals.clone(),
        pairs: InstallationPairs::Read {
            report: json!({"warnings": ["ledger_read_incomplete"]}),
            bindings: Err(Outcome::new(SharedCode::ConfigReadFailed)),
        },
    };
    let answer = fixture.status_with(&installation);
    let part = answer.installation.as_ref().expect("the root's part");
    assert!(part.suspension_report_read, "{part:?}");
    assert_eq!(part.pairs_unread, Some(PairsUnread::BindingsNotRead));
    assert!(
        part.pairs_unread_outcome
            .as_ref()
            .is_some_and(|outcome| outcome.is(SharedCode::ConfigReadFailed)),
        "{part:?}"
    );
    assert_eq!(
        row(&answer, TWICE).pairs,
        Some(PackPairs::NotRead(PairsUnread::BindingsNotRead))
    );
    assert_eq!(answer.warnings, vec!["ledger_read_incomplete".to_owned()]);
}

#[test]
fn s315_a_root_not_the_installations_and_a_build_without_the_rescue_module_are_refused() {
    let _guard = env_lock();
    let fixture = Fixture::write();
    fixture.add_unreadable_journal();
    let refused_code = "package_status_root_not_installed";

    // A game root of that name outside the installation's standard-pack root, which does not
    // exist: refused before anything is read (the readiness would refuse a root that is no
    // directory otherwise).
    let elsewhere = fixture.temp.path().join("elsewhere").join(GAME);
    let error = read_installation(&fixture.location(), &elsewhere)
        .expect_err("a root that is not the installation's");
    assert_eq!(error.code, refused_code, "{}", error.message);
    assert!(
        matches!(error.class, ErrorKind::UsageValidation),
        "{error:?}"
    );
    let details = error.details.clone().unwrap_or(Value::Null);
    assert_eq!(details["outcome"]["code"], refused_code, "{details}");
    assert!(!elsewhere.exists());

    // A configuration that names no standard-pack root places no game root.
    let bare = fixture.root.join("bare.config.json");
    write_json(
        &bare,
        &json!({"state_root": fixture.temp.path().join("state-root"), "instances": []}),
    );
    let error = read_installation(&fixture.location_with(bare), &fixture.game_root)
        .expect_err("an installation without a standard-pack root");
    assert_eq!(error.code, refused_code, "{}", error.message);

    // On the command line, `--installation` reaches the installation before the root is read: a
    // program that runs from no installation is refused by the location's own outcome, not by the
    // readiness's refusal of a root that is no directory.
    let isolated = Isolated::new();
    let missing = isolated.path("no-game-root");
    let state = isolated.path("no-state");
    let line = [
        "--json",
        "standard-pack",
        "status",
        "--root",
        missing.as_str(),
        "--state-dir",
        state.as_str(),
        "--installation",
    ];
    let result = run_cli(line, true);
    let shown = result.envelope_json();
    assert_eq!(result.exit_code(), 4, "{shown}");
    let error = result
        .envelope
        .error
        .as_ref()
        .unwrap_or_else(|| panic!("the line was not refused: {shown}"));
    assert_eq!(error.code, "install_root_unresolved", "{shown}");
    assert!(!Path::new(&state).exists(), "{shown}");

    // The build without the rescue module (not built on Windows, where CI runs): its refusal.
    let error = status_installation_unbuilt();
    assert_eq!(error.code, "package_status_installation_unbuilt");
    assert!(
        matches!(error.class, ErrorKind::UsageValidation),
        "{error:?}"
    );
    let details = error.details.clone().unwrap_or(Value::Null);
    assert_eq!(
        details["outcome"]["code"], "package_status_installation_unbuilt",
        "{details}"
    );
}

#[test]
fn s315_the_row_writes_nothing() {
    let _guard = env_lock();
    let fixture = Fixture::write();
    fixture.add_unreadable_journal();
    let mut before = BTreeMap::new();
    tree(fixture.temp.path(), &mut before);

    fixture.installed();
    standard_pack_status(&fixture.request())
        .unwrap_or_else(|error| panic!("the status: {}: {}", error.code, error.message));

    let mut after = BTreeMap::new();
    tree(fixture.temp.path(), &mut after);
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>()
    );
    assert!(before == after, "a file changed");
}
