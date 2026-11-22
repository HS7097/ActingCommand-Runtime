// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 V1 (register §1.378; contracts/package-variables.md; contracts/remedies.md
// section 2.4): a package's `control.json` may declare `variables` beside `provides`. Kernel
// admission reads them with the contract crate's one reader: a rule broken is refused as
// `contained_task_control_invalid` with the rule as its `reason` and the pointer in
// `control.json` as its `json_pointer`; a name a parameter of `provides` has is
// `variable_name_taken`. A package that declares variables is admitted, and answers its
// descriptor, its parameter defaults and the parameters of a run without a dispatch as the same
// package without them: nothing resolves or overlays a variable in this slice. Every package is
// built in memory; the metadata reader by path is read by `standard-pack check`
// (`resource-tooling`, `gate_463_v1`).
//
// Workflow #455 G-SLx (0.12.78; register §1.501): a package that provides a capability it marks
// irreversible declares variables as any other package, where V1 refused them
// (`variables_on_irreversible`): the first test takes them beside such a `provides`, and the
// third admits such a package for real and shows that its runs read its variables as the same
// package's beside a reversible `provides` do.
//
// A synthetic `neutral` game, server `test`, 16x9, as `gate_411_b15p` builds it: page `home` is
// told by a blue first pixel and page `page_b` by a red one; the package's one operation goes from
// `home` to `page_b`. Capabilities, parameters, variables and options are made-up names.

use super::tests::admission_deadline;
use super::*;
use serde_json::{Value, json};

const LABEL: &str = "v1-test";
const PACKAGE_ID: &str = "neutral.test.variables";
const CONTROL_INVALID: &str = "contained_task_control_invalid";

// The control declaration of the package, with `provides` and `variables` when given.
fn control(provides: Option<Value>, variables: Option<Value>) -> Value {
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
    if let Some(variables) = variables {
        control["variables"] = variables;
    }
    control
}

// One capability with the parameter `param_p`, irreversible when `irreversible`.
fn provides(irreversible: bool) -> Value {
    json!([{"capability": "cap_x", "irreversible": irreversible, "parameters": [
        {"name": "param_p", "options": ["opt_1", "opt_2"], "default": "opt_2"}]}])
}

// A variable of each kind.
fn variables() -> Value {
    json!([
        {"name": "pick", "type": "choice", "title": {"en": "Item"},
         "options": [{"value": "item_a", "title": {"en": "Item A"}},
                     {"value": "item_b", "title": {"en": "Item B"}}],
         "default": "item_b", "default_origin": "model"},
        {"name": "floor", "type": "integer", "title": {"en": "Keep at least"},
         "min": 0, "max": 200, "step": 10, "default": 0},
        {"name": "order", "type": "ordered", "title": {"en": "Order"},
         "options": [{"value": "item_a", "title": {"en": "Item A"}},
                     {"value": "item_b", "title": {"en": "Item B"}}],
         "min_count": 0, "max_count": 2, "default": ["item_b"]},
        {"name": "extra", "type": "switch", "title": {"en": "Also the extra"}, "default": false}
    ])
}

// The package's one task: from `home` to `page_b`, which its mapping `done_k` maps.
fn task() -> Value {
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
        "goal": "V1 fixture",
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
        "scheduling_outcome": {"mappings": [{
            "outcome_key": "done_k",
            "effect": "no_designated_effect",
            "terminal_pages": ["page_b"]
        }]}
    })
}

// The content table of the package's source directory, in memory.
fn entries(control: &Value) -> BTreeMap<String, Vec<u8>> {
    [
        ("control.json", control.clone()),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        ("resources/operations/task/task.json", task()),
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

fn refusal(error: &ContainedTaskError) -> (&str, Option<&str>) {
    (error.code().as_str(), error.detail())
}

// [`variables`] changed by `change`.
fn with(change: impl Fn(&mut Value)) -> Value {
    let mut value = variables();
    change(&mut value);
    value
}

// Each rule of `variables` refuses the control declaration with `contained_task_control_invalid`,
// its reason and its pointer; the variables of each kind, beside a reversible `provides`, without
// one and (G-SLx) beside a `provides` that marks its capability irreversible, pass.
#[test]
fn v1_variables_violations_are_refused_with_their_reason_and_pointer() {
    let cases = [
        (
            "no entry",
            None,
            json!([]),
            "variables_invalid",
            "/variables",
        ),
        (
            "a member a variable does not have",
            None,
            with(|value| value[0]["governs"] = json!({"packages": [PACKAGE_ID]})),
            "variables_invalid",
            "/variables/0/governs",
        ),
        (
            "a name outside the token form",
            None,
            with(|value| value[1]["name"] = json!("Floor")),
            "variables_invalid",
            "/variables/1/name",
        ),
        (
            "a default off the step",
            None,
            with(|value| value[1]["default"] = json!(5)),
            "variables_invalid",
            "/variables/1/default",
        ),
        (
            "an ordered default that names no option",
            None,
            with(|value| value[2]["default"] = json!(["item_c"])),
            "variables_invalid",
            "/variables/2/default/0",
        ),
        (
            "a name an earlier variable has",
            None,
            with(|value| value[3]["name"] = json!("pick")),
            "variable_name_taken",
            "/variables/3/name",
        ),
        (
            "a name a parameter of provides has",
            Some(provides(false)),
            with(|value| value[1]["name"] = json!("param_p")),
            "variable_name_taken",
            "/variables/1/name",
        ),
        (
            "a name a parameter of an irreversible capability has",
            Some(provides(true)),
            with(|value| value[1]["name"] = json!("param_p")),
            "variable_name_taken",
            "/variables/1/name",
        ),
    ];
    for (case, provided, declared, reason, pointer) in cases {
        let error =
            validate_control_declaration(control(provided, Some(declared))).expect_err(case);
        assert_eq!(
            refusal(&error),
            (
                CONTROL_INVALID,
                Some(format!("reason={reason} json_pointer={pointer}").as_str())
            ),
            "{case}"
        );
    }

    for (case, provided) in [
        ("without provides", None),
        ("beside a reversible provides", Some(provides(false))),
        ("beside an irreversible provides", Some(provides(true))),
    ] {
        validate_control_declaration(control(provided, Some(variables())))
            .unwrap_or_else(|error| panic!("{case}: {error}"));
    }
}

// A package that declares variables is admitted for real (the source parser's gate and kernel
// admission), and answers as the same package without them: the same descriptor, the same
// parameter defaults and the same parameters of a run without a dispatch, so nothing is overlaid
// for a variable in this slice.
#[test]
fn v1_a_package_with_variables_is_admitted_and_answers_as_without_them() {
    let prepare = |name: &str, variables: Option<Value>| {
        PreparedContainedTask::load_content_entries(
            LABEL,
            entries(&control(Some(provides(false)), variables)),
            None,
            admission_deadline(),
        )
        .unwrap_or_else(|error| panic!("{name}: admission refused the package: {error}"))
    };
    let declaring = prepare("with variables", Some(variables()));
    let plain = prepare("without variables", None);
    assert_eq!(
        serde_json::to_value(declaring.package_descriptor()).expect("descriptor JSON"),
        serde_json::to_value(plain.package_descriptor()).expect("descriptor JSON")
    );
    assert_eq!(
        declaring.declared_parameter_defaults(),
        [("param_p".to_owned(), "opt_2".to_owned())]
    );
    assert_eq!(
        declaring.declared_parameter_defaults(),
        plain.declared_parameter_defaults()
    );
    assert_eq!(
        declaring.default_run_parameters(),
        plain.default_run_parameters()
    );
}

// Workflow #455 G-SLx (0.12.78; register §1.501; the brief's test 1): a package whose own
// `provides` marks its capability irreversible and that declares the four variables is admitted
// for real (the source parser's gate and kernel admission; every kernel before 0.12.78 refused it
// `variables_on_irreversible`), declares them as the same package beside a reversible `provides`,
// and its runs read them as that package's do: without a dispatch at their defaults, from a
// dispatch that carries `pick` item_a and `floor` 100 those values (the others at their defaults),
// with the dispatch's parameter beside them, and given the same values (a direct run, the Lab's
// scan replay) the same.
#[test]
fn gslx_a_package_with_an_irreversible_capability_declares_variables_and_its_runs_read_them() {
    use actingcommand_contract::FactValue;
    let prepare = |irreversible: bool| {
        PreparedContainedTask::load_content_entries(
            LABEL,
            entries(&control(Some(provides(irreversible)), Some(variables()))),
            None,
            admission_deadline(),
        )
        .unwrap_or_else(|error| {
            panic!("irreversible {irreversible}: admission refused the package: {error}")
        })
    };
    let marked = prepare(true);
    let reversible = prepare(false);
    assert_eq!(marked.declared_variables().len(), 4);
    assert_eq!(marked.declared_variables(), reversible.declared_variables());
    assert_eq!(
        marked.declared_parameter_defaults(),
        [("param_p".to_owned(), "opt_2".to_owned())]
    );
    assert_eq!(marked.default_run_values(), reversible.default_run_values());
    assert_eq!(
        marked.default_variable_values(),
        reversible.default_variable_values()
    );

    let given = [
        ("pick".to_owned(), json!("item_a")),
        ("floor".to_owned(), json!(100)),
    ];
    let read = marked.dispatched_variable_values(&given);
    assert_eq!(read, reversible.dispatched_variable_values(&given));
    let value_of = |name: &str| {
        read.iter()
            .find(|(declared, _)| declared == name)
            .map(|(_, value)| value.clone())
    };
    assert_eq!(
        value_of("pick"),
        Some(FactValue::String("item_a".to_owned()))
    );
    assert_eq!(value_of("floor"), Some(FactValue::Integer(100)));
    assert_eq!(value_of("extra"), Some(FactValue::Boolean(false)));
    let parameter = vec![("param_p".to_owned(), "opt_1".to_owned())];
    assert_eq!(
        marked
            .dispatched_run_parameters(parameter.clone(), 1_000)
            .with_variables(read.clone()),
        reversible
            .dispatched_run_parameters(parameter, 1_000)
            .with_variables(reversible.dispatched_variable_values(&given))
    );
    assert_eq!(
        marked
            .given_variable_values(&given)
            .expect("the given values"),
        read
    );
}
