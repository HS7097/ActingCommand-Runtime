// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B15 R1 (contracts/remedies.md sections 2.2 and 2.4; model-b15 sections 4.2 and
// 4.4; register §1.111 R1-5; host key 119): the readers that take a package by its path. Kernel
// admission by path (`PreparedContainedTask::describe_path`) takes a package whose
// `control.json` declares what it `provides`, each capability with the parameters whose options
// and default state its scope; the host's metadata reader
// (`PreparedContainedTask::read_remedy_declarations`) reads its declarations by path without
// preparing it. The kernel's own tests of `provides` build their packages in memory
// (`execution-kernel` `contained_task/gate_411_b15p.rs`, exec key 95).
//
// A synthetic `neutral` game, server `test`, 16x9: page `home` is told by a blue first pixel and
// page `page_b` by a red one; the package's one operation goes from `home` to `page_b`, where its
// run ends on the key `blocked_k`. Needs, capabilities, parameters and options are neutral names.
// The package is a source package directory under a temporary directory.

use super::*;
use actingcommand_contract::{ProvidedCapability, RemedyDeclaration};
use actingcommand_execution_kernel::RemedyDeclarations;
use serde_json::{Value, json};

const LABEL: &str = "b15p-test";
const PACKAGE_ID: &str = "neutral.test.provider";

// The control declaration of the package, with `provides` when one is given.
fn control(provides: Option<Value>) -> Value {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": PACKAGE_ID,
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
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
    control
}

// The package's one task: from `home` to `page_b`, which its mapping `blocked_k` maps, with
// `remedy` on that mapping when one is given.
fn task(remedy: Option<Value>) -> Value {
    let mut mapping = json!({
        "outcome_key": "blocked_k",
        "effect": "no_designated_effect",
        "terminal_pages": ["page_b"]
    });
    if let Some(remedy) = remedy {
        mapping["remedy"] = remedy;
    }
    let probe = |page: &str, expected: [u8; 3]| {
        json!({
            "id": format!("{page}_colour"),
            "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
            "expected": expected
        })
    };
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "B15 R1 fixture",
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
            "id": "go",
            "from": "home",
            "to": "page_b",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }],
        "scheduling_outcome": {"mappings": [mapping]}
    })
}

// The package's source directory under `root`: its control declaration, its resource list and
// its one task.
fn write_package(root: &TempDir, control: &Value, task: &Value) -> PathBuf {
    let path = root.path().join("package");
    for (file, value) in [
        ("control.json", control.clone()),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        ("resources/operations/task/task.json", task.clone()),
    ] {
        let file = path.join(file);
        fs::create_dir_all(file.parent().expect("a package file's directory"))
            .expect("package directory");
        fs::write(
            file,
            serde_json::to_vec_pretty(&value).expect("package JSON"),
        )
        .expect("package file");
    }
    path
}

// Two capabilities, the second irreversible with the two parameters that state its scope.
fn provides() -> Value {
    json!([
        {"capability": "cap_x"},
        {"capability": "cap_y", "irreversible": true, "parameters": [
            {"name": "param_p", "options": ["opt_1", "opt_2"], "default": "opt_2"},
            {"name": "param_q", "options": ["opt_3", "opt_4", "opt_5"], "default": "opt_3"}
        ]}
    ])
}

// A remedy whose capability the package itself does not provide.
fn remedy() -> Value {
    json!({"need": "need_a", "capabilities": ["cap_z"], "default": ["cap_z"]})
}

// Requirements 2 and 5: a package that provides two capabilities, one irreversible with its
// parameters, and declares a remedy is admitted by kernel admission; the metadata reader, which
// admits it by its path without preparing it, answers its package id, its remedy with the
// mapping's key and every capability it provides with its marker and parameters, as declared.
#[test]
fn b15p_provides_with_parameters_is_admitted_and_read_without_preparing() {
    let root = TempDir::new().expect("tempdir");
    let package = write_package(&root, &control(Some(provides())), &task(Some(remedy())));

    let descriptor = PreparedContainedTask::describe_path(LABEL, &package, admission_deadline())
        .unwrap_or_else(|error| panic!("kernel admission refused the package: {error}"));
    assert_eq!(descriptor.package_id(), PACKAGE_ID);

    let read =
        PreparedContainedTask::read_remedy_declarations(LABEL, &package, admission_deadline())
            .unwrap_or_else(|error| panic!("the reader refused the package: {error}"));
    let declared: RemedyDeclaration = serde_json::from_value(remedy()).expect("the remedy");
    let provided: Vec<ProvidedCapability> =
        serde_json::from_value(provides()).expect("the capabilities");
    assert_eq!(
        read,
        RemedyDeclarations {
            package_id: PACKAGE_ID.to_owned(),
            remedies: vec![("blocked_k".to_owned(), declared)],
            provides: provided,
        }
    );
    let (reversible, irreversible) = (&read.provides[0], &read.provides[1]);
    assert_eq!(
        (reversible.capability(), reversible.irreversible()),
        ("cap_x", false)
    );
    assert!(reversible.parameters().is_empty());
    assert_eq!(
        (irreversible.capability(), irreversible.irreversible()),
        ("cap_y", true)
    );
    let strings = |values: &[&str]| -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    };
    assert_eq!(
        irreversible
            .parameters()
            .iter()
            .map(|parameter| (
                parameter.name().to_owned(),
                parameter.options().to_vec(),
                parameter.default_option().to_owned()
            ))
            .collect::<Vec<(String, Vec<String>, String)>>(),
        [
            (
                "param_p".to_owned(),
                strings(&["opt_1", "opt_2"]),
                "opt_2".to_owned()
            ),
            (
                "param_q".to_owned(),
                strings(&["opt_3", "opt_4", "opt_5"]),
                "opt_3".to_owned()
            ),
        ]
    );
}
