// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #373 NAV-T (model M-373 v1.1 §3.1, §3.9 second row, §4.4, §11.1 and §11.2 rows NT-1
// to NT-4; wave-18 ruling 1; exec key 80): the standard-pack cross-check of `package bundle` and
// `standard-pack check`.
//
// NT-1: `package bundle` lays the root files it is given beside `bundle.json` byte for byte, and
// refuses a root file the Runtime's reader refuses (`standard_pack_root_invalid`), writing no
// `--out`. NT-2: every package a root file names is admitted and checked for its part as the
// Runtime checks it; one that does not fit is `package_bundle_navigation_package_invalid` with
// the Runtime's `navigation_role` and `navigation_problem`, by `package bundle` and by the check
// of an installed game root alike. NT-3: both report each pack whose entry page the page list
// does not hold, and the check refuses a listed package it cannot admit. NT-4: a pack named by
// a name the game root reserves is refused by `package bundle` and by the bundle index the
// Runtime reads.
//
// A synthetic `neutral` game, server `test`, 16x9. Each page is told by the colour of the first
// pixel: `home` blue, `shop` red, `notice` white, `stage` green, `done` yellow. The page list
// holds `home`, `shop` and `notice`; the feature package recognises exactly those; the edge
// `home` to `shop` has its package; the recovery chain has one step; the interrupt page
// `notice` has its handler; and one more pack starts on `stage`, which the list does not hold.

use crate::{
    PackageBundleRequest, PackageBundleResponse, StandardPackCheckRequest,
    StandardPackCheckResponse, StandardPackRootFiles, check_standard_pack_root, package_bundle,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    LabError, LabResult, StandardPackProblem, StandardPackRegistry, StandardPackRootBytes,
    read_standard_pack_root,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";
const FEATURES: &str = "neutral.test.features";
const HOME_TO_SHOP: &str = "neutral.test.home_to_shop";
const RETURN_HOME: &str = "neutral.test.return_home";
const CLOSE_NOTICE: &str = "neutral.test.close_notice";
const STAGE: &str = "neutral.test.stage";
const BAD: &str = "neutral.test.bad";

const NAVIGATION_PACKAGE_INVALID: &str = "package_bundle_navigation_package_invalid";
const ENTRY_PAGE_UNLISTED: &str = "package_bundle_entry_page_unlisted";
const ROOT_INVALID: &str = "standard_pack_root_invalid";
const INDEX_INVALID: &str = "package_bundle_index_invalid";

const PAGES: &str = "pages.json";
const INTERRUPTS: &str = "interrupts.json";
const NAVIGATION: &str = "navigation.json";

// ---- Packs.

// A pack of the game: its execution mode, its prerequisite package, its entry (a page or `any`),
// its target and one click per operation `(id, from, to)`; its pages are the ones these name.
#[derive(Clone)]
struct Pack {
    package_id: &'static str,
    mode: &'static str,
    prerequisite: Option<&'static str>,
    entry: &'static str,
    target: &'static str,
    operations: Vec<(&'static str, &'static str, &'static str)>,
    width: u32,
}

// A `navigable_route` pack from `entry` to `target`.
fn graph(
    package_id: &'static str,
    entry: &'static str,
    target: &'static str,
    operations: &[(&'static str, &'static str, &'static str)],
) -> Pack {
    Pack {
        package_id,
        mode: "navigable_route",
        prerequisite: None,
        entry,
        target,
        operations: operations.to_vec(),
        width: 16,
    }
}

// The colour of the first pixel that tells `page`.
fn colour(page: &str) -> [u8; 3] {
    match page {
        "home" => [0, 0, 255],
        "shop" => [255, 0, 0],
        "notice" => [255, 255, 255],
        "stage" => [0, 255, 0],
        "done" => [255, 255, 0],
        other => panic!("no page {other}"),
    }
}

impl Pack {
    // Writes the pack as a source package directory: its control, its resource table and its
    // one task.
    fn write(&self, directory: &Path) {
        let mut pages = BTreeSet::from([self.entry, self.target]);
        for (_, from, to) in &self.operations {
            pages.insert(*from);
            pages.insert(*to);
        }
        pages.remove("any");
        let height = self.width * 9 / 16;
        let mut control = json!({
            "schema_version": "Lab-1y.control.v2",
            "package_id": self.package_id,
            "execution_mode": self.mode,
            "game": GAME,
            "server": SERVER,
            "resolution": {"width": self.width, "height": height},
            "entry_task_id": "task",
            "capture_interval_ms": 10,
            "step_timeout_ms": 600,
            "timeout_ms": 8000,
            "max_steps": 2
        });
        if let Some(prerequisite) = self.prerequisite {
            control["prerequisite_package_id"] = json!(prerequisite);
        }
        let task = json!({
            "schema_version": "0.9",
            "task_id": "task",
            "game": GAME,
            "server_scope": [SERVER],
            "locale": "en-US",
            "goal": "NAV-T fixture",
            "coordinate_space": {"width": self.width, "height": height},
            "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
            "color_probes": pages
                .iter()
                .map(|page| json!({
                    "id": format!("{page}_colour"),
                    "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
                    "expected": colour(page)
                }))
                .collect::<Vec<_>>(),
            "page_rules": pages
                .iter()
                .map(|page| ((*page).to_owned(), json!({"required": [format!("{page}_colour")]})))
                .collect::<serde_json::Map<_, _>>(),
            "entry_page": self.entry,
            "target_page": self.target,
            "operations": self
                .operations
                .iter()
                .map(|(id, from, to)| json!({
                    "id": id,
                    "from": from,
                    "to": to,
                    "click": {"kind": "point", "x": 1, "y": 0},
                    "unguarded_trusted_coordinate": true,
                    "retryable": false
                }))
                .collect::<Vec<_>>()
        });
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
            fs::write(path, serde_json::to_vec_pretty(&value).expect("pack JSON"))
                .expect("pack file");
        }
    }
}

// The feature package: a `recognize_only` pack whose pages are exactly the page list's.
fn features() -> Pack {
    Pack {
        mode: "recognize_only",
        ..graph(
            FEATURES,
            "home",
            "home",
            &[
                ("see_shop", "shop", "home"),
                ("see_notice", "notice", "home"),
            ],
        )
    }
}

// ---- The standard pack.

// The packs and the root files of one game root; a root file that is `None` is not given.
#[derive(Clone)]
struct Root {
    packs: Vec<Pack>,
    pages: Option<Value>,
    navigation: Option<Value>,
    interrupts: Option<Value>,
}

impl Root {
    // `pack` in place of the pack of its id.
    fn with(mut self, pack: Pack) -> Self {
        self.packs
            .retain(|listed| listed.package_id != pack.package_id);
        self.packs.push(pack);
        self
    }

    fn edges(mut self, edges: Value) -> Self {
        if let Some(navigation) = self.navigation.as_mut() {
            navigation["edges"] = edges;
        }
        self
    }

    fn interrupt(mut self, member: &str, value: Value) -> Self {
        if let Some(interrupts) = self.interrupts.as_mut() {
            interrupts["interrupts"][0][member] = value;
        }
        self
    }
}

// The game root that keeps every rule.
fn valid() -> Root {
    Root {
        packs: vec![
            features(),
            graph(HOME_TO_SHOP, "home", "shop", &[("go", "home", "shop")]),
            graph(RETURN_HOME, "shop", "home", &[("back", "shop", "home")]),
            graph(
                CLOSE_NOTICE,
                "notice",
                "home",
                &[("close", "notice", "home")],
            ),
            graph(STAGE, "stage", "done", &[("finish", "stage", "done")]),
        ],
        pages: Some(json!({
            "schema_version": "actingcommand.pages.v1",
            "game": GAME,
            "features": [FEATURES],
            "pages": [
                {"id": "home", "kind": "page"},
                {"id": "shop", "kind": "page"},
                {"id": "notice", "kind": "overlay"}
            ]
        })),
        navigation: Some(json!({
            "schema_version": "actingcommand.navigation.v1",
            "game": GAME,
            "edges": [{"from": "home", "to": "shop", "package_id": HOME_TO_SHOP}],
            "recovery_chain": [RETURN_HOME]
        })),
        interrupts: Some(json!({
            "schema_version": "actingcommand.interrupts.v1",
            "game": GAME,
            "interrupts": [
                {"page": "notice", "handlers": [CLOSE_NOTICE], "then": "resume", "max_per_run": 1}
            ]
        })),
    }
}

// Each root file given, by its name. A source is written pretty, with a last line feed, so its
// bytes are not the ones a reader would write back.
fn root_files(root: &Root) -> Vec<(&'static str, Vec<u8>)> {
    [
        (PAGES, &root.pages),
        (INTERRUPTS, &root.interrupts),
        (NAVIGATION, &root.navigation),
    ]
    .into_iter()
    .filter_map(|(name, value)| {
        value.as_ref().map(|value| {
            let mut bytes = serde_json::to_vec_pretty(value).expect("root file JSON");
            bytes.push(b'\n');
            (name, bytes)
        })
    })
    .collect()
}

// One `package bundle` call over `root`: the applications table, one source directory per pack
// and the root files, each in a temporary directory.
fn bundle(
    root: &Root,
) -> (
    TempDir,
    PackageBundleRequest,
    LabResult<PackageBundleResponse>,
) {
    let temp = TempDir::new().expect("temporary directory");
    let applications = temp.path().join("applications.json");
    fs::write(
        &applications,
        serde_json::to_vec_pretty(&json!({
            "schema_version": "actingcommand.applications.v1",
            "game": GAME,
            "servers": {SERVER: {"application_id": "com.example.neutral", "label": "Test"}}
        }))
        .expect("applications JSON"),
    )
    .expect("applications table");
    let packs_root = temp.path().join("sources");
    for pack in &root.packs {
        pack.write(&packs_root.join(pack.package_id));
    }
    let sources = temp.path().join("root");
    fs::create_dir_all(&sources).expect("root file directory");
    let mut files = StandardPackRootFiles::default();
    for (name, bytes) in root_files(root) {
        let path = sources.join(name);
        fs::write(&path, bytes).expect("root file");
        match name {
            PAGES => files.pages = Some(path),
            INTERRUPTS => files.interrupts = Some(path),
            _ => files.navigation = Some(path),
        }
    }
    let request = PackageBundleRequest {
        applications,
        packs_root,
        out: temp.path().join("bundle"),
        source: None,
        maintenance: None,
        root_files: files,
    };
    let result = package_bundle(request.clone());
    (temp, request, result)
}

// An installed game root of `root`, `<temporary>/packages/neutral/`: its bundle index v4 listing
// every pack and every package of `listed_only`, which has no directory; one directory per pack;
// and the root files.
fn installed(root: &Root, listed_only: &[&'static str]) -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("temporary directory");
    let game_root = temp.path().join("packages").join(GAME);
    fs::create_dir_all(&game_root).expect("game root");
    for pack in &root.packs {
        pack.write(&game_root.join(pack.package_id));
    }
    let listed = root
        .packs
        .iter()
        .map(|pack| pack.package_id)
        .chain(listed_only.iter().copied())
        .map(|package_id| {
            json!({
                "package_id": package_id,
                "server": SERVER,
                "entry_task_id": "task",
                "path": format!("packs/{package_id}"),
                "file_count": 3,
                "byte_count": 64
            })
        })
        .collect::<Vec<_>>();
    fs::write(
        game_root.join("bundle.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": "actingcommand.bundle.v4",
            "game": GAME,
            "packs": listed
        }))
        .expect("bundle JSON"),
    )
    .expect("bundle index");
    for (name, bytes) in root_files(root) {
        fs::write(game_root.join(name), bytes).expect("root file");
    }
    (temp, game_root)
}

fn check(game_root: &Path) -> LabResult<StandardPackCheckResponse> {
    check_standard_pack_root(StandardPackCheckRequest {
        root: game_root.to_owned(),
    })
}

// A refusal's code and the values of the outcome it carries.
fn refusal(error: &LabError) -> (String, Value) {
    let Some(details) = error.details.as_ref() else {
        panic!("{}: {} carries no outcome", error.code, error.message);
    };
    let outcome = &details["outcome"];
    assert_eq!(outcome["code"], error.code.as_str(), "{}", error.message);
    (error.code.clone(), outcome["values"].clone())
}

// Each warning's code, package and pages.
fn report(warnings: &[Outcome]) -> Vec<(String, Value, Value)> {
    warnings
        .iter()
        .map(|warning| {
            (
                warning.code().as_str().to_owned(),
                warning.value("package_id").cloned().unwrap_or(Value::Null),
                warning.value("page_ids").cloned().unwrap_or(Value::Null),
            )
        })
        .collect()
}

// ---- NT-1.

// NT-1: the root files given are laid beside `bundle.json` byte for byte, and the written game
// root is one the Runtime's reader reads, with the page list, the navigation data and the
// interrupts it was given.
#[test]
fn nt_1_package_bundle_lays_the_root_files_beside_bundle_json_byte_for_byte() {
    let root = valid();
    let (_temp, request, result) = bundle(&root);
    let response = result.unwrap_or_else(|error| panic!("{}: {}", error.code, error.message));
    assert_eq!(response.status, "written");
    for (name, source) in [
        (PAGES, &request.root_files.pages),
        (INTERRUPTS, &request.root_files.interrupts),
        (NAVIGATION, &request.root_files.navigation),
    ] {
        let source = source.as_ref().expect("a given root file");
        assert_eq!(
            fs::read(request.out.join(name)).expect("written root file"),
            fs::read(source).expect("source root file"),
            "{name}"
        );
    }
    let read = |name: &str| fs::read(request.out.join(name)).expect("written file");
    let (bundle, pages, interrupts, navigation) = (
        read("bundle.json"),
        read(PAGES),
        read(INTERRUPTS),
        read(NAVIGATION),
    );
    let written = read_standard_pack_root(
        StandardPackRootBytes {
            bundle: Some(&bundle),
            pages: Some(&pages),
            interrupts: Some(&interrupts),
            navigation: Some(&navigation),
        },
        GAME,
    )
    .expect("a game root the Runtime reads")
    .expect("a game root");
    assert_eq!(written.registry.len(), root.packs.len());
    assert_eq!(
        written.pages.expect("the page list").features,
        [FEATURES.to_owned()]
    );
    assert_eq!(
        written
            .navigation
            .expect("the navigation data")
            .recovery_chain,
        [RETURN_HOME.to_owned()]
    );
    assert_eq!(
        written.interrupts.expect("the interrupts").interrupts[0].handlers,
        [CLOSE_NOTICE.to_owned()]
    );
}

// NT-1: a root file the Runtime's reader refuses is refused by `package bundle` with the reader's
// `standard_pack_root_invalid`, naming the given file, its problem and where; `--out` is not
// written.
#[test]
fn nt_1_a_root_file_the_runtime_refuses_is_refused_and_no_out_is_written() {
    let mut other_game = valid();
    if let Some(pages) = other_game.pages.as_mut() {
        pages["game"] = json!("other");
    }
    let without_pages = Root {
        pages: None,
        ..valid()
    };
    let cases = [
        (
            "an edge from a page to itself",
            valid().edges(json!([{"from": "home", "to": "home", "package_id": HOME_TO_SHOP}])),
            NAVIGATION,
            StandardPackProblem::EdgeSelfLoop,
            "/edges/0/to",
        ),
        (
            "a page list of another game",
            other_game,
            PAGES,
            StandardPackProblem::GameMismatch,
            "/game",
        ),
        (
            "interrupts without a page list",
            without_pages,
            INTERRUPTS,
            StandardPackProblem::PagesMissing,
            "",
        ),
        (
            "a handler the bundle does not hold",
            valid().interrupt("handlers", json!(["neutral.test.missing"])),
            INTERRUPTS,
            StandardPackProblem::PackageNotListed,
            "/interrupts/0/handlers/0",
        ),
    ];
    for (case, root, file, problem, pointer) in cases {
        let (_temp, request, result) = bundle(&root);
        let error = result.expect_err(case);
        let (code, values) = refusal(&error);
        assert_eq!(code, ROOT_INVALID, "{case}: {}", error.message);
        let source = match file {
            PAGES => &request.root_files.pages,
            INTERRUPTS => &request.root_files.interrupts,
            _ => &request.root_files.navigation,
        };
        assert_eq!(
            values,
            json!({
                "game": GAME,
                "path": source.as_ref().expect("the refused file was given").to_string_lossy(),
                "standard_pack_problem": problem.as_str(),
                "json_pointer": pointer
            }),
            "{case}"
        );
        assert!(!request.out.exists(), "{case}: --out was written");
    }
}

// ---- NT-2.

// How a case changes the valid game root.
type Arrange = fn(Root) -> Root;

// NT-2: each package a root file names is checked for its part as the Runtime checks it, by
// `package bundle` and by the check of the installed root alike: the feature package's pages are
// exactly the list's and it is a `recognize_only` package; an edge's package starts on its `from`
// and ends on its `to`, is no application entry and can be a layer of a run of its server (of the
// resolution of the server's feature package); a recovery chain, handler or `then.run` package
// declares no prerequisite package; a handler starts on its interrupt page.
#[test]
fn nt_2_a_package_unfit_for_its_part_is_refused_by_package_bundle_and_by_the_check() {
    let cases: [(&str, Arrange, &str, &str, &str); 11] = [
        (
            "a feature package without a listed page",
            |root| {
                root.with(Pack {
                    operations: vec![("see_shop", "shop", "home")],
                    ..features()
                })
            },
            FEATURES,
            "features",
            "page_set_mismatch",
        ),
        (
            "a feature package that is no recognize_only package",
            |mut root| {
                if let Some(pages) = root.pages.as_mut() {
                    pages["features"] = json!([HOME_TO_SHOP]);
                }
                root
            },
            HOME_TO_SHOP,
            "features",
            "role_ineligible",
        ),
        (
            "an edge whose package does not start on its from page",
            |root| {
                root.edges(json!([{"from": "notice", "to": "shop", "package_id": HOME_TO_SHOP}]))
            },
            HOME_TO_SHOP,
            "edge",
            "entry_mismatch",
        ),
        (
            "an edge whose package does not end on its to page",
            |root| {
                root.edges(json!([{"from": "home", "to": "notice", "package_id": HOME_TO_SHOP}]))
            },
            HOME_TO_SHOP,
            "edge",
            "terminal_mismatch",
        ),
        (
            "an edge whose package is an application entry",
            |root| {
                root.with(graph(BAD, "any", "shop", &[("go", "home", "shop")]))
                    .edges(json!([{"from": "home", "to": "shop", "package_id": BAD}]))
            },
            BAD,
            "edge",
            "application_entry",
        ),
        (
            "an edge whose package cannot be a layer",
            |root| root.edges(json!([{"from": "home", "to": "shop", "package_id": FEATURES}])),
            FEATURES,
            "edge",
            "role_ineligible",
        ),
        (
            "an edge whose package has another resolution than the feature package",
            |root| {
                root.with(Pack {
                    width: 32,
                    ..graph(BAD, "home", "shop", &[("go", "home", "shop")])
                })
                .edges(json!([{"from": "home", "to": "shop", "package_id": BAD}]))
            },
            BAD,
            "edge",
            "identity_mismatch",
        ),
        (
            "a recovery chain step that declares a prerequisite package",
            |root| {
                root.with(Pack {
                    prerequisite: Some(HOME_TO_SHOP),
                    ..graph(RETURN_HOME, "shop", "home", &[("back", "shop", "home")])
                })
            },
            RETURN_HOME,
            "chain",
            "prerequisite_declared",
        ),
        (
            "a handler whose entry does not hold its interrupt page",
            |root| root.interrupt("handlers", json!([RETURN_HOME])),
            RETURN_HOME,
            "handler",
            "entry_mismatch",
        ),
        (
            "a handler that declares a prerequisite package",
            |root| {
                root.with(Pack {
                    prerequisite: Some(HOME_TO_SHOP),
                    ..graph(
                        CLOSE_NOTICE,
                        "notice",
                        "home",
                        &[("close", "notice", "home")],
                    )
                })
            },
            CLOSE_NOTICE,
            "handler",
            "prerequisite_declared",
        ),
        (
            "a then.run package that declares a prerequisite package",
            |root| {
                root.with(Pack {
                    prerequisite: Some(HOME_TO_SHOP),
                    ..graph(BAD, "home", "shop", &[("go", "home", "shop")])
                })
                .interrupt("then", json!({"run": [BAD]}))
            },
            BAD,
            "then_run",
            "prerequisite_declared",
        ),
    ];
    for (case, arrange, package_id, role, problem) in cases {
        let expected = json!({
            "package_id": package_id,
            "navigation_role": role,
            "navigation_problem": problem
        });

        let (_temp, request, result) = bundle(&arrange(valid()));
        let error = result.expect_err(case);
        let (code, values) = refusal(&error);
        assert_eq!(
            code, NAVIGATION_PACKAGE_INVALID,
            "{case}: {}",
            error.message
        );
        assert_eq!(values, expected, "{case}: package bundle");
        assert!(!request.out.exists(), "{case}: --out was written");

        let (_temp, game_root) = installed(&arrange(valid()), &[]);
        let error = check(&game_root).expect_err(case);
        let (code, values) = refusal(&error);
        assert_eq!(
            code, NAVIGATION_PACKAGE_INVALID,
            "{case}: {}",
            error.message
        );
        assert_eq!(values, expected, "{case}: standard-pack check");
    }
}

// ---- NT-3.

// NT-3: both tools report each pack whose entry page the page list does not hold, one
// `package_bundle_entry_page_unlisted` naming the pack and the pages; a list that holds the page
// leaves nothing to report. The check answers the game, the root files it read and the packs.
#[test]
fn nt_3_both_tools_report_each_pack_whose_entry_page_the_list_does_not_hold() {
    let unlisted = vec![(
        ENTRY_PAGE_UNLISTED.to_owned(),
        json!(STAGE),
        json!(["stage"]),
    )];
    let (_temp, _request, result) = bundle(&valid());
    let response = result.unwrap_or_else(|error| panic!("{}: {}", error.code, error.message));
    assert_eq!(report(&response.warnings), unlisted, "package bundle");

    let (_temp, game_root) = installed(&valid(), &[]);
    let checked =
        check(&game_root).unwrap_or_else(|error| panic!("{}: {}", error.code, error.message));
    assert_eq!(checked.status, "checked");
    assert_eq!(checked.game, GAME);
    assert_eq!(
        checked.root_files,
        ["bundle.json", PAGES, INTERRUPTS, NAVIGATION]
    );
    assert_eq!(checked.pack_count, 5);
    assert_eq!(report(&checked.warnings), unlisted, "standard-pack check");

    // The page list holds `stage` too, and the feature package recognises it.
    let mut listed = valid().with(Pack {
        operations: vec![
            ("see_shop", "shop", "home"),
            ("see_notice", "notice", "home"),
            ("see_stage", "stage", "home"),
        ],
        ..features()
    });
    if let Some(pages) = listed.pages.as_mut() {
        pages["pages"]
            .as_array_mut()
            .expect("the pages")
            .push(json!({"id": "stage", "kind": "page"}));
    }
    let (_temp, _request, result) = bundle(&listed);
    let response = result.unwrap_or_else(|error| panic!("{}: {}", error.code, error.message));
    assert!(response.warnings.is_empty(), "{:?}", response.warnings);
    let (_temp, game_root) = installed(&listed, &[]);
    let checked =
        check(&game_root).unwrap_or_else(|error| panic!("{}: {}", error.code, error.message));
    assert!(checked.warnings.is_empty(), "{:?}", checked.warnings);
}

// NT-3: an edge's package the installed root lists but cannot admit (its directory is not
// there) is refused as the Runtime refuses it, `admission_failed`, with the loader's refusal as
// its cause.
#[test]
fn nt_3_the_check_refuses_a_listed_package_it_cannot_admit() {
    let root = valid().edges(json!([{"from": "home", "to": "shop", "package_id": BAD}]));
    let (_temp, game_root) = installed(&root, &[BAD]);
    let error = check(&game_root).expect_err("an edge package that is not installed");
    let (code, values) = refusal(&error);
    assert_eq!(code, NAVIGATION_PACKAGE_INVALID, "{}", error.message);
    assert_eq!(
        values,
        json!({
            "package_id": BAD,
            "navigation_role": "edge",
            "navigation_problem": "admission_failed"
        })
    );
    let causes = error.details.as_ref().expect("an outcome")["outcome"]["causes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(causes.len(), 1, "the loader's refusal is the cause");
}

// ---- NT-4.

// NT-4: a pack whose `package_id` is a name the game root reserves, a root file's `<kind>.json`
// or what a replacement in place leaves (`<name>.part`, `<name>.old-<unix_ms>`), in any case, is
// refused by `package bundle`, which writes no `--out`, and a bundle index that lists one is no
// registry the Runtime reads.
#[test]
fn nt_4_a_reserved_package_id_is_refused() {
    for package_id in [
        "pages.json",
        "Navigation.JSON",
        "neutral.test.part",
        "neutral.test.old-1760000000000",
    ] {
        let root = Root {
            packs: vec![graph(package_id, "home", "shop", &[("go", "home", "shop")])],
            pages: None,
            navigation: None,
            interrupts: None,
        };
        let (_temp, request, result) = bundle(&root);
        let error = result.expect_err(package_id);
        assert_eq!(error.code, INDEX_INVALID, "{package_id}: {}", error.message);
        assert!(!request.out.exists(), "{package_id}: --out was written");

        let index = json!({
            "schema_version": "actingcommand.bundle.v4",
            "game": GAME,
            "packs": [{
                "package_id": package_id,
                "server": SERVER,
                "entry_task_id": "task",
                "path": format!("packs/{package_id}"),
                "file_count": 3,
                "byte_count": 64
            }]
        });
        let refused = StandardPackRegistry::from_bundle_bytes(
            &serde_json::to_vec(&index).expect("bundle JSON"),
            GAME,
        )
        .expect_err(package_id);
        assert_eq!(
            refused.problem,
            StandardPackProblem::BundleInvalid,
            "{package_id}"
        );
    }
}
