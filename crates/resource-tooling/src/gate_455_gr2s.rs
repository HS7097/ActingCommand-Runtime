// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 G-R2s fix round 3 (register §1.369; contracts/standard-pack-goals.md,
// "Irreversibility"; contracts/standard-pack-schedule.md, "The irreversible mark"): the resource
// tools' goal check (`goal_issues`) and the composer's input they read (`root_irreversibility`),
// over real packages on disk read by the Runtime's metadata reader.
// - C.4: the goal check's half of the policy gate's t7 goes through `goal_issues`: a `tier` entry
//   over four chain heads whose followers are linked three ways the evaluator reads as a chain
//   link (a plain link, a link that also accepts a failed end, a link behind a complete rule),
//   each follower running an irreversible package, is refused at those three heads and passes the
//   fourth, whose one follower runs a package nobody marks.
// - Part B (the coordinator's ruling of 11-14): a package whose remedy lists a capability another
//   package of the root provides marked irreversible counts as irreversible, for the goal check
//   (a `tier` over its rule, over the head of a chain it follows, and an entry that governs it are
//   refused) and for the composer's input the tools read; a package whose remedy lists only a
//   capability nobody marks does not.
// A made-up game `neutral`, server `test`, 16x9; every package, rule and capability is a
// placeholder.

use crate::standard_pack_goals::{goal_issues, root_irreversibility};
use actingcommand_contract::{
    StandardPackProblem, StandardPackRoot, StandardPackRootBytes, StandardPackRootIssue,
    read_standard_pack_root,
};
use actingcommand_policy::BENEFIT_FIXED_VALUE_SUBSYSTEM;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";

/// An irreversible capability, with the parameters the reader requires of one.
fn irreversible(capability: &str) -> Value {
    json!([{"capability": capability, "irreversible": true, "parameters": [
        {"name": "param_p", "options": ["opt_1", "opt_2"], "default": "opt_1"}]}])
}

/// A remedy of the need `need_a` listing `capabilities`.
fn remedy(capabilities: &[&str]) -> Value {
    json!({"need": "need_a", "capabilities": capabilities})
}

/// Writes the package `package_id` as a package directory: its control (with `provides` when
/// given) and its one task, from page `home` (a blue first pixel) to page `page_b` (a red one),
/// where its run ends on the key `blocked_k`, whose mapping declares `remedy` when given.
fn write_pack(directory: &Path, package_id: &str, provides: Option<Value>, remedy: Option<Value>) {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": "navigable_route",
        "game": GAME,
        "server": SERVER,
        "resolution": {"width": 16, "height": 9},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    if let Some(provides) = provides {
        control["provides"] = provides;
    }
    let probe = |page: &str, expected: [u8; 3]| {
        json!({
            "id": format!("{page}_colour"),
            "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
            "expected": expected
        })
    };
    let mut task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": [SERVER],
        "locale": "en-US",
        "goal": "G-R2s fixture",
        "coordinate_space": {"width": 16, "height": 9},
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
    });
    if let Some(remedy) = remedy {
        task["scheduling_outcome"] = json!({"mappings": [{
            "outcome_key": "blocked_k",
            "effect": "no_designated_effect",
            "terminal_pages": ["page_b"],
            "remedy": remedy
        }]});
    }
    for (path, value) in [
        ("control.json", control),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        ("resources/operations/task/task.json", task),
    ] {
        let path = directory.join(path);
        fs::create_dir_all(path.parent().expect("pack file parent")).expect("pack directory");
        fs::write(path, serde_json::to_vec_pretty(&value).expect("pack JSON")).expect("pack file");
    }
}

/// A `schedule.json` rule running the package of its own id on `trigger`, writing a fixed value.
fn rule(id: &str, trigger: Value) -> Value {
    json!({
        "id": id,
        "package_id": id,
        "priority": 100,
        "trigger": trigger,
        "feedback_stop": {
            "kind": "outcome",
            "task_id": id,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [],
        "produces": [],
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
        "strategic_weight_milli": 1000,
        "fixed_value_milli": 12000
    })
}

/// An hourly clock.
fn clock() -> Value {
    json!({
        "kind": "clock",
        "schedule": {
            "kind": "interval",
            "clock_source": {"kind": "local"},
            "every_ms": 3600000,
            "anchor_ms": 0
        }
    })
}

/// A chain link: `dependency_completed` of `task_id` accepting `terminal_states`.
fn link(task_id: &str, terminal_states: &[&str]) -> Value {
    json!({
        "kind": "dependency_completed",
        "task_id": task_id,
        "terminal_states": terminal_states,
        "since_own_run": true
    })
}

/// One package of a root: its id, what it provides and the remedy of its mapping.
type Pack = (&'static str, Option<Value>, Option<Value>);

/// A game root in a new temporary directory: `bundle.json` registering `packs`, each in its
/// directory, and `schedule.json` of `rules`; with the root as the contract crate reads it.
fn installed(packs: Vec<Pack>, rules: Vec<Value>) -> (TempDir, PathBuf, StandardPackRoot, Vec<u8>) {
    let temp = TempDir::new().expect("temporary directory");
    let game_root = temp.path().join(GAME);
    let mut listed = Vec::new();
    for (package_id, provides, remedy) in packs {
        write_pack(&game_root.join(package_id), package_id, provides, remedy);
        listed.push(json!({
            "package_id": package_id, "server": SERVER, "entry_task_id": "task",
            "path": format!("packs/{package_id}"), "file_count": 3, "byte_count": 64
        }));
    }
    let bundle = serde_json::to_vec_pretty(
        &json!({"schema_version": "actingcommand.bundle.v4", "game": GAME, "packs": listed}),
    )
    .expect("bundle JSON");
    fs::write(game_root.join("bundle.json"), &bundle).expect("bundle.json");
    let root = read_standard_pack_root(
        StandardPackRootBytes {
            bundle: Some(bundle.as_slice()),
            ..StandardPackRootBytes::default()
        },
        GAME,
    )
    .unwrap_or_else(|issue| panic!("the root reads: {issue}"))
    .expect("the root holds bundle.json");
    let schedule = serde_json::to_vec_pretty(&json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [BENEFIT_FIXED_VALUE_SUBSYSTEM],
        "rules": rules,
        "pools": [],
        "activity": [],
        "timeline": [],
        "held": []
    }))
    .expect("schedule JSON");
    (temp, game_root, root, schedule)
}

/// `goals.json` with `entries`.
fn goals(entries: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema_version": "actingcommand.goals.v1",
        "game": GAME,
        "entries": entries
    }))
    .expect("goals JSON")
}

/// A `tier` entry over `rules`.
fn tier(rules: &[&str]) -> Value {
    json!({
        "name": "weight", "type": "tier", "governs": {"rules": rules},
        "title": {"en": "Weight"}, "default": "normal",
        "options": [{"value": "normal", "title": {"en": "Normal"}, "scale_milli": 1000},
                    {"value": "more", "title": {"en": "More"}, "scale_milli": 2000}]
    })
}

/// The problems `goal_issues` gives of `goals` against the installed root.
fn issues(
    game_root: &Path,
    root: &StandardPackRoot,
    schedule: &[u8],
    goals: &[u8],
) -> Vec<StandardPackRootIssue> {
    let schedule_path = game_root.join("schedule.json");
    goal_issues(
        GAME,
        root,
        Some((schedule, schedule_path.as_path())),
        goals,
        &|directory: &str| game_root.join(directory),
        None,
    )
    .unwrap_or_else(|error| panic!("the check runs: {}: {}", error.code, error.message))
}

/// Each problem of `issues` as (word, pointer).
fn refusals(issues: &[StandardPackRootIssue]) -> Vec<(StandardPackProblem, String)> {
    issues
        .iter()
        .map(|issue| (issue.problem, issue.pointer.clone()))
        .collect()
}

// C.4: t7's goal half through `goal_issues`. Four heads on an hourly clock, each writing a fixed
// value and running a package that provides nothing: H1's follower B1 by a plain link, H2's
// follower B2 by a link that also accepts a `failed` end, H3's follower C3 (a complete rule) and
// C3's follower B3; B1, B2 and B3 run a package that marks `cap_y` irreversible. H4's one follower
// F4 runs a package that provides `cap_p`, which nobody marks. A `tier` over the four heads is
// `goal_governs_irreversible` at H1, H2 and H3 and nothing else.
#[test]
fn gr2s_t7_the_goal_check_reads_the_same_chain_links() {
    let packs: Vec<Pack> = vec![
        ("neutral.h1", None, None),
        ("neutral.b1", Some(irreversible("cap_y")), None),
        ("neutral.h2", None, None),
        ("neutral.b2", Some(irreversible("cap_y")), None),
        ("neutral.h3", None, None),
        ("neutral.c3", None, None),
        ("neutral.b3", Some(irreversible("cap_y")), None),
        ("neutral.h4", None, None),
        ("neutral.f4", Some(json!([{"capability": "cap_p"}])), None),
    ];
    let rules = vec![
        rule("neutral.h1", clock()),
        rule("neutral.b1", link("neutral.h1", &["succeeded"])),
        rule("neutral.h2", clock()),
        rule("neutral.b2", link("neutral.h2", &["succeeded", "failed"])),
        rule("neutral.h3", clock()),
        rule("neutral.c3", link("neutral.h3", &["succeeded"])),
        rule("neutral.b3", link("neutral.c3", &["succeeded"])),
        rule("neutral.h4", clock()),
        rule("neutral.f4", link("neutral.h4", &["succeeded"])),
    ];
    let (_temp, game_root, root, schedule) = installed(packs, rules);
    let goals = goals(json!([tier(&[
        "neutral.h1",
        "neutral.h2",
        "neutral.h3",
        "neutral.h4"
    ])]));
    assert_eq!(
        refusals(&issues(&game_root, &root, &schedule, &goals)),
        (0..3)
            .map(|index| {
                (
                    StandardPackProblem::GoalGovernsIrreversible,
                    format!("/entries/0/governs/rules/{index}"),
                )
            })
            .collect::<Vec<_>>()
    );
}

// Part B: BLOCKED's package provides nothing and its mapping's remedy lists `cap_r` and `cap_q`;
// PROVIDER's package provides `cap_r` marked irreversible; F5, the follower of H5 by a plain link,
// runs a package whose remedy lists `cap_r`; KEEPS's package's remedy lists `cap_q` alone, which
// SPARE's package provides unmarked. The composer's input the tools read counts BLOCKED, PROVIDER
// and F5 irreversible and KEEPS, SPARE and H5 not, every package read. The goal check reads the
// same rule: a `tier` over BLOCKED, KEEPS and H5 is `goal_governs_irreversible` at BLOCKED and H5,
// and a `switch` that governs the packages BLOCKED and KEEPS is refused at BLOCKED, nothing else.
#[test]
fn gr2s_r3_a_remedy_that_lists_an_irreversible_capability_makes_its_package_irreversible() {
    let packs: Vec<Pack> = vec![
        ("neutral.blocked", None, Some(remedy(&["cap_r", "cap_q"]))),
        ("neutral.provider", Some(irreversible("cap_r")), None),
        ("neutral.h5", None, None),
        ("neutral.f5", None, Some(remedy(&["cap_r"]))),
        ("neutral.keeps", None, Some(remedy(&["cap_q"]))),
        (
            "neutral.spare",
            Some(json!([{"capability": "cap_q"}])),
            None,
        ),
    ];
    let rules = vec![
        rule("neutral.blocked", clock()),
        rule("neutral.provider", clock()),
        rule("neutral.h5", clock()),
        rule("neutral.f5", link("neutral.h5", &["succeeded"])),
        rule("neutral.keeps", clock()),
        rule("neutral.spare", clock()),
    ];
    let (_temp, game_root, root, schedule) = installed(packs, rules);

    let read = root_irreversibility(&root, &|directory: &str| game_root.join(directory));
    assert!(read.packages.unreadable().is_empty(), "every package reads");
    for (package_id, expected) in [
        ("neutral.blocked", true),
        ("neutral.provider", true),
        ("neutral.f5", true),
        ("neutral.keeps", false),
        ("neutral.spare", false),
        ("neutral.h5", false),
    ] {
        assert_eq!(
            read.packages.package(package_id),
            Some(expected),
            "{package_id}"
        );
    }

    let goals = goals(json!([
        tier(&["neutral.blocked", "neutral.keeps", "neutral.h5"]),
        {"name": "enabled", "type": "switch",
         "governs": {"packages": ["neutral.blocked", "neutral.keeps"]},
         "title": {"en": "Enabled"}, "default": false}
    ]));
    assert_eq!(
        refusals(&issues(&game_root, &root, &schedule, &goals)),
        [
            (
                StandardPackProblem::GoalGovernsIrreversible,
                "/entries/0/governs/rules/0".to_owned()
            ),
            (
                StandardPackProblem::GoalGovernsIrreversible,
                "/entries/0/governs/rules/2".to_owned()
            ),
            (
                StandardPackProblem::GoalGovernsIrreversible,
                "/entries/1/governs/packages/0".to_owned()
            ),
        ]
    );
}
