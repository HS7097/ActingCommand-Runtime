// SPDX-License-Identifier: AGPL-3.0-only

// LAB-WAIT (0.12.29; the resource side's report of 10-29; register §1.210, provisional). Offline
// `observe` reads a wait step as pack containment and the kernel do
// (resource-declarations.md, "Wait steps and wait pages"): `wait` and none of `click`,
// `application` and `select` (pack containment's `is_wait_step`) is a step without input, which
// the generated navigation leaves out, so observe lists no action for it, whether its
// destinations are a page-set `to` or `expect_after.page_id`; the package loads, and the step's
// destinations are the kernel's (`PreparedContainedTask::operation_destinations`). Before 0.12.29
// a page-set `to` on a wait step refused the package with `missing field 'click'`. A click step
// whose `to` is a page set is listed as before and still requires its `click`. Packages are
// synthetic and neutral: a 2 x 1 frame whose left pixel names the page.

use actingcommand_lab::{ExternallyVerifiedBundle, PreparedContainedTask};
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 2;
const PAGES: [(&str, [u8; 3]); 3] = [
    ("home", [255, 0, 0]),
    ("terminal", [0, 0, 255]),
    ("alternate", [0, 255, 255]),
];

fn zip_entries(entries: &[(&str, Value)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in entries {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec(value).expect("JSON"))
            .expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

// The package of a task schema `0.9` page-graph task from `home` to `terminal` or `alternate`
// with `operations`, under a `Lab-1y.control.v2` control, with an empty generated navigation.
fn package(operations: Vec<Value>) -> Vec<u8> {
    let targets = PAGES
        .iter()
        .map(|(page, colour)| {
            json!({"type": "color", "id": format!("page/{page}"),
                   "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": colour})
        })
        .collect::<Vec<_>>();
    let pages = PAGES
        .iter()
        .map(|(page, _)| {
            json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")],
                   "optional": [], "forbidden": []})
        })
        .collect::<Vec<_>>();
    zip_entries(&[
        (
            "control.json",
            json!({
                "schema_version": "Lab-1y.control.v2",
                "package_id": "neutral.lab_wait.task",
                "execution_mode": "navigable_route",
                "game": "neutral",
                "server": "test",
                "resolution": {"width": WIDTH, "height": 1},
                "entry_task_id": "task",
                "capture_interval_ms": 1_000,
                "step_timeout_ms": 3_000,
                "timeout_ms": 30_000
            }),
        ),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        (
            "resources/operations/task/task.json",
            json!({
                "schema_version": "0.9",
                "task_id": "task",
                "game": "neutral",
                "server_scope": ["test"],
                "coordinate_space": {"width": WIDTH, "height": 1},
                "entry_page": "home",
                "target_page": ["terminal", "alternate"],
                "operations": operations
            }),
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            json!({
                "schema_version": "0.7",
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": WIDTH, "height": 1},
                "defaults": {"color_max_distance": 0.0},
                "targets": targets
            }),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json!({"schema_version": "0.3", "pages": pages}),
        ),
        (
            "resources/navigation/neutral.test.navigation.json",
            json!({"schema_version": "0.3", "game": "neutral", "navigation": [],
                   "destructive_actions": []}),
        ),
    ])
}

// The package as offline `observe --zip` admits it.
fn observed(case: &str, bytes: &[u8]) -> ExternallyVerifiedBundle {
    ExternallyVerifiedBundle::load("semantic_observe", bytes)
        .unwrap_or_else(|error| panic!("{case}: the package is admitted: {error}"))
}

// A wait step `settle` from `home` that makes no input, its destinations `terminal` and
// `alternate` declared as a page-set `to` (the shape of the report) or as `expect_after.page_id`
// (the resource side's workaround): observe loads the package and lists no action for the step,
// and the kernel admits the same package with the step's destinations the declared set.
#[test]
fn lab_wait_a_wait_step_to_a_page_set_loads_and_its_destinations_are_the_kernels() {
    for (case, destination) in [
        ("to", json!({"to": ["terminal", "alternate"]})),
        (
            "expect_after",
            json!({"expect_after": {"page_id": ["terminal", "alternate"]}}),
        ),
    ] {
        let mut settle = json!({"id": "settle", "from": "home", "wait": {"max_ms": 60_000}});
        for (member, value) in destination.as_object().expect("destination members") {
            settle[member] = value.clone();
        }
        let bytes = package(vec![settle]);

        let bundle = observed(case, &bytes);
        let view =
            crate::contained_resources::observation_resources(&bundle).unwrap_or_else(|error| {
                panic!("{case}: observe admits the package: {}", error.message)
            });
        let listed = view
            .multi_destination
            .iter()
            .map(|operation| operation.id.as_str())
            .collect::<Vec<_>>();
        assert!(
            listed.is_empty(),
            "{case}: a wait step is no click operation: {listed:?}"
        );

        let task = PreparedContainedTask::load("neutral.instance", &bytes)
            .unwrap_or_else(|error| panic!("{case}: the kernel admits the package: {error}"));
        let destinations = task
            .operation_destinations("settle")
            .unwrap_or_else(|error| panic!("{case}: the step's destinations: {error}"));
        assert_eq!(destinations, ["alternate", "terminal"], "{case}");
    }
}

// A click step `branch` from `home` whose `to` is the page set is listed with every page it may
// reach and its declared click; the same step without its click (and without `wait`) is refused
// with `missing field 'click'`, as before.
#[test]
fn lab_wait_a_click_step_to_a_page_set_still_requires_its_click() {
    let click = json!({"kind": "point", "x": 1, "y": 0});
    let branch = json!({"id": "branch", "from": "home", "to": ["terminal", "alternate"],
                        "click": click, "unguarded_trusted_coordinate": true});

    let bundle = observed("with its click", &package(vec![branch.clone()]));
    let view = crate::contained_resources::observation_resources(&bundle)
        .unwrap_or_else(|error| panic!("observe admits the package: {}", error.message));
    let listed = view
        .multi_destination
        .iter()
        .map(|operation| {
            (
                operation.id.as_str(),
                operation.task_id.as_str(),
                operation.from_page.as_str(),
                operation.to_pages.clone(),
                operation.click.clone(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        listed,
        [(
            "branch",
            "task",
            "neutral/home",
            vec![
                "neutral/terminal".to_string(),
                "neutral/alternate".to_string()
            ],
            click
        )]
    );

    let mut without_click = branch;
    without_click
        .as_object_mut()
        .expect("operation object")
        .remove("click");
    let bundle = observed("without its click", &package(vec![without_click]));
    let Err(error) = crate::contained_resources::observation_resources(&bundle) else {
        panic!("a click step to a page set without its click is admitted");
    };
    assert!(
        error.message.contains("missing field 'click'"),
        "{}",
        error.message
    );
}
