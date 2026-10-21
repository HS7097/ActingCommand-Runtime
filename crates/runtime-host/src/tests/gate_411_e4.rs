// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 slice E4, T4 (B39 A part; register §1.156 E4-4; host key 139), in the world of
// `gate_373_nav5`, whose fixtures it reuses as its child module. A page-graph package declares the
// prerequisite package `neutral.test.lead`, which leads `menu` to `shop` and maps its terminal
// page `shop` to an outcome key. The prerequisite runs and ends on `shop`; the package's own entry
// `home` still does not pass, and the game root has no navigation data, so the run fails as before
// with `contained_task_prerequisite_entry_unmatched`. Its outcome also names the outcome key the
// layer's run ended on, as the typed value `layer_outcome_key`.

use super::*;

/// The outcome key the layer's package maps its terminal page `shop` to.
const LAYER_KEY: &str = "lead_reached_shop";

#[test]
fn e4_t4_an_unmatched_entry_after_a_prerequisite_names_the_layer_outcome_key() {
    let state = Arc::new(FakeState::default());
    screens(&state, vec![MENU, SHOP, HOME, DONE]);
    let temp = TempDir::new().expect("tempdir");
    let mut root = world(temp.path());
    root.add(&graph(LEAD, "menu", "shop", &[("lead", "menu", "shop")]));
    // The layer's package maps its terminal page to an outcome key (a fake layer that ends on a
    // mapped key).
    let task_file = root
        .game
        .join(LEAD)
        .join("resources/operations/task/task.json");
    let mut task: Value =
        serde_json::from_slice(&fs::read(&task_file).expect("read the layer's task"))
            .expect("the layer's task JSON");
    task["scheduling_outcome"] = json!({
        "mappings": [{
            "outcome_key": LAYER_KEY,
            "effect": "no_designated_effect",
            "terminal_pages": ["shop"]
        }]
    });
    fs::write(
        &task_file,
        serde_json::to_vec_pretty(&task).expect("the layer's task JSON"),
    )
    .expect("write the layer's task");
    root.bundle();
    let host = start(&temp, &state, &root);
    let main = Pack {
        prerequisite: Some(LEAD),
        ..graph_main()
    };
    let run = run_direct(&host, &temp, "main", &main);
    host.close().expect("close runtime host");

    assert_eq!(
        run.admitted(),
        [(LEAD.to_owned(), Some(TaskEntryRecoveryRole::Prerequisite))]
    );
    let failure = run.failure();
    assert_eq!(
        failure.code().as_str(),
        "contained_task_prerequisite_entry_unmatched",
        "{failure}"
    );
    // B39 A part: the outcome key the layer's run ended on, as a typed value.
    assert_eq!(
        text(&failure, "layer_outcome_key"),
        Some(LAYER_KEY),
        "{failure}"
    );
}
