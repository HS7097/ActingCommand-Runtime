// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B15 R1 (contracts/remedies.md sections 2.2 and 2.4; model-b15 sections 4.2 and
// 4.4; exec key 95): a package's `control.json` may declare what it `provides`, each capability
// with the parameters whose options and default state its scope. Kernel admission takes such a
// package; a prepared package answers the default of every parameter it declares. A `provides`
// that breaks a rule is refused as `contained_task_control_invalid` with the rule as its
// `reason`, and so is a package that provides a capability one of its own remedies names. Every
// package here is built in memory; the readers that take a package by its path (`describe_path`
// and `read_remedy_declarations`) are tested in runtime-host (`tests/gate_411_b15p_host.rs`,
// host key 119).
//
// A synthetic `neutral` game, server `test`, 16x9: page `home` is told by a blue first pixel and
// page `page_b` by a red one; the package's one operation goes from `home` to `page_b`, where its
// run ends on the key `blocked_k`. Needs, capabilities, parameters and options are neutral names.

use super::tests::admission_deadline;
use super::*;
use serde_json::{Value, json};

const LABEL: &str = "b15p-test";
const PACKAGE_ID: &str = "neutral.test.provider";
const CONTROL_INVALID: &str = "contained_task_control_invalid";

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

// The content table of the package's source directory, in memory: its control declaration, its
// resource list and its one task.
fn entries(control: &Value, task: &Value) -> BTreeMap<String, Vec<u8>> {
    [
        ("control.json", control.clone()),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        ("resources/operations/task/task.json", task.clone()),
    ]
    .into_iter()
    .map(|(path, value)| {
        (
            path.to_owned(),
            serde_json::to_vec_pretty(&value).expect("package JSON"),
        )
    })
    .collect()
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

fn refusal(error: &ContainedTaskError) -> (&str, Option<&str>) {
    (error.code().as_str(), error.detail())
}

// Requirement 2: each rule of `provides` refuses the control declaration with
// `contained_task_control_invalid` and its reason: the list, a capability or its marker
// `provides_invalid`; a capability's parameters `provides_parameters_invalid`, an irreversible
// capability without parameters included. A list that keeps every rule passes.
#[test]
fn b15p_provides_violations_are_refused_with_their_reason() {
    let parameter = |name: &str| json!({"name": name, "options": ["opt_1"], "default": "opt_1"});
    let options = |count: usize| {
        (0..count)
            .map(|index| format!("opt_{index}"))
            .collect::<Vec<_>>()
    };
    let with_parameters =
        |parameters: Value| json!([{"capability": "cap_x", "parameters": parameters}]);
    let cases = [
        ("no capability", json!([]), "provides_invalid"),
        (
            "nine capabilities",
            Value::Array(
                (0..9)
                    .map(|index| json!({"capability": format!("cap_{index}")}))
                    .collect(),
            ),
            "provides_invalid",
        ),
        (
            "no list",
            json!({"capability": "cap_x"}),
            "provides_invalid",
        ),
        (
            "a capability provided twice",
            json!([{"capability": "cap_x"}, {"capability": "cap_x"}]),
            "provides_invalid",
        ),
        (
            "the capability none",
            json!([{"capability": "none"}]),
            "provides_invalid",
        ),
        (
            "a capability that is no token",
            json!([{"capability": "Cap_x"}]),
            "provides_invalid",
        ),
        (
            "an irreversible marker that is no boolean",
            json!([{"capability": "cap_x", "irreversible": "yes"}]),
            "provides_invalid",
        ),
        (
            "a member a provided capability does not have",
            json!([{"capability": "cap_x", "scope": "opt_1"}]),
            "provides_invalid",
        ),
        (
            "an irreversible capability without parameters",
            json!([{"capability": "cap_x", "irreversible": true}]),
            "provides_parameters_invalid",
        ),
        (
            "an irreversible capability with an empty parameter list",
            json!([{"capability": "cap_x", "irreversible": true, "parameters": []}]),
            "provides_parameters_invalid",
        ),
        (
            "nine parameters",
            with_parameters(Value::Array(
                (0..9)
                    .map(|index| parameter(&format!("param_{index}")))
                    .collect(),
            )),
            "provides_parameters_invalid",
        ),
        (
            "a parameter name that is no token",
            with_parameters(json!([parameter("Param_p")])),
            "provides_parameters_invalid",
        ),
        (
            "a parameter name two capabilities declare",
            json!([
                {"capability": "cap_x", "parameters": [parameter("param_p")]},
                {"capability": "cap_y", "parameters": [parameter("param_p")]}
            ]),
            "provides_parameters_invalid",
        ),
        (
            "a parameter without options",
            with_parameters(json!([{"name": "param_p", "options": [], "default": "opt_1"}])),
            "provides_parameters_invalid",
        ),
        (
            "a parameter with seventeen options",
            with_parameters(
                json!([{"name": "param_p", "options": options(17), "default": "opt_1"}]),
            ),
            "provides_parameters_invalid",
        ),
        (
            "an option named twice",
            with_parameters(
                json!([{"name": "param_p", "options": ["opt_1", "opt_1"], "default": "opt_1"}]),
            ),
            "provides_parameters_invalid",
        ),
        (
            "an option that is no token",
            with_parameters(json!([{"name": "param_p", "options": ["opt-1"], "default": "opt-1"}])),
            "provides_parameters_invalid",
        ),
        (
            "a default that is no option",
            with_parameters(json!([{"name": "param_p", "options": ["opt_1"], "default": "opt_2"}])),
            "provides_parameters_invalid",
        ),
        (
            "a parameter without its default",
            with_parameters(json!([{"name": "param_p", "options": ["opt_1"]}])),
            "provides_parameters_invalid",
        ),
    ];
    for (case, provides, reason) in cases {
        let error = validate_control_declaration(control(Some(provides))).expect_err(case);
        assert_eq!(
            refusal(&error),
            (CONTROL_INVALID, Some(format!("reason={reason}").as_str())),
            "{case}"
        );
    }

    for provides in [
        provides(),
        json!([{"capability": "cap_x", "irreversible": false}]),
        with_parameters(json!([{"name": "param_p", "options": options(16), "default": "opt_15"}])),
    ] {
        validate_control_declaration(control(Some(provides.clone())))
            .unwrap_or_else(|error| panic!("{provides}: {error}"));
    }
}

// Requirement 3: a package that provides a capability one of its own remedies names is refused
// as `contained_task_control_invalid` `reason=provides_own_remedy`; a remedy that names only
// capabilities other packages provide is not.
#[test]
fn b15p_a_package_providing_its_own_remedy_capability_is_refused() {
    let control = parse_task_control_declaration(control(Some(provides())))
        .unwrap_or_else(|error| panic!("the control declaration: {error}"));
    let scheduling = |capabilities: Value| -> SchedulingOutcomeDeclaration {
        let declaration: SchedulingOutcomeDeclaration = serde_json::from_value(json!({
            "mappings": [{
                "outcome_key": "blocked_k",
                "effect": "no_designated_effect",
                "terminal_pages": ["page_b"],
                "remedy": {"need": "need_a", "capabilities": capabilities}
            }]
        }))
        .expect("the scheduling declaration");
        declaration.validate().expect("a valid declaration");
        declaration
    };

    for capabilities in [json!(["cap_y"]), json!(["cap_z", "cap_x"])] {
        let error = super::remedies::validate_own_remedies(
            &control,
            Some(&scheduling(capabilities.clone())),
        )
        .expect_err("a package's remedy names a capability it provides");
        assert_eq!(
            refusal(&error),
            (CONTROL_INVALID, Some("reason=provides_own_remedy")),
            "{capabilities}"
        );
    }

    super::remedies::validate_own_remedies(&control, Some(&scheduling(json!(["cap_z"]))))
        .unwrap_or_else(|error| panic!("a remedy another package serves: {error}"));
    super::remedies::validate_own_remedies(&control, None)
        .unwrap_or_else(|error| panic!("a package without remedies: {error}"));
}

// Requirement 5: a prepared package answers every parameter its `provides` declares with the
// default it declares, in declaration order; a package that provides nothing answers none.
#[test]
fn b15p_declared_parameter_defaults_are_answered_by_a_prepared_task() {
    let prepare = |name: &str, provides: Option<Value>| {
        PreparedContainedTask::load_content_entries(
            LABEL,
            entries(&control(provides), &task(Some(remedy()))),
            None,
            admission_deadline(),
        )
        .unwrap_or_else(|error| panic!("{name}: admission refused the package: {error}"))
    };

    let declaring = prepare("defaults", Some(provides()));
    assert_eq!(
        declaring.declared_parameter_defaults(),
        [
            ("param_p".to_owned(), "opt_2".to_owned()),
            ("param_q".to_owned(), "opt_3".to_owned()),
        ]
    );

    let none = prepare("no-provides", None);
    assert!(
        none.declared_parameter_defaults().is_empty(),
        "{:?}",
        none.declared_parameter_defaults()
    );
}
