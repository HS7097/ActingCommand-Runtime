// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #428 RD-BUDGET-1 (0.12.51; register §1.295; exec gate row "#428 RD-BUDGET-1";
// contracts/resource-declarations.md): the declaration validator's reader bounds what a run reads
// from disk, so a file counts once. It keeps each file's bytes for the rest of the run and counts a
// path against the total only at its first read; a later read of the path answers the kept bytes
// and reads nothing from disk. A selection whose distinct files exceed the total is still refused,
// with the same code and message, at the file that crosses the line. Every expected value comes
// from the coordinator's ruling of 11-10 and the slice's requirement lines. The repository is a
// neutral fixture written into a temporary folder: one pack whose task `pick` has three select
// steps, each naming its own selection policy document.

use super::*;
use tempfile::TempDir;

const TASK: &str = "packs/neutral.budget/resources/operations/pick/task.json";
const RESOURCES: &str = "packs/neutral.budget/resources/operations/resources.json";
const POLICIES: [&str; 3] = [
    "packs/neutral.budget/resources/operations/pick/policies/choice_a.json",
    "packs/neutral.budget/resources/operations/pick/policies/choice_b.json",
    "packs/neutral.budget/resources/operations/pick/policies/choice_c.json",
];
const LIST: &str = "resource-changed-paths";

/// One select step of `pick`, from `home` to `result`, that chooses a slot by the policy
/// `policies/<policy>.json`.
fn step(id: &str, policy: &str) -> Value {
    json!({
        "id": id,
        "from": "home",
        "to": "result",
        "expect_after": {"page_id": "result"},
        "guard": {
            "page_id": "home",
            "target_id": "guard/ready",
            "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
            "color_probe": "guard/ready"
        },
        "select": {"layout_id": "choices", "policy": {"path": format!("policies/{policy}.json")}},
        "retryable": false
    })
}

/// A selection policy document of the layout `choices` that keeps the slot whose `first` passed.
fn policy(policy_id: &str) -> Value {
    json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": policy_id,
        "applies_to": {
            "candidate_layout_id": "choices",
            "outcome_keys": {"selected": "slot_selected", "empty": "slot_empty",
                             "insufficient": "slot_insufficient", "ambiguous": "slot_ambiguous",
                             "unknown": "slot_unknown"}
        },
        "fields": [{"name": "first", "value_type": {"type": "boolean"}}],
        "facts": [],
        "gates": [{"gate_id": "choice",
                   "predicate": {"kind": "boolean_equals",
                                 "value": {"source": "field", "field": "first"},
                                 "expected": true},
                   "on_unknown": {"kind": "abort_evaluation"}}],
        "scoring": [],
        "selection": {"mode": "exactly_one", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    })
}

/// The repository: the task, its three policy documents and the operations' resource table.
fn repository() -> TempDir {
    let temp = TempDir::new().unwrap();
    let slot = |x: u32, target: &str| {
        json!({"rect": {"x": x, "y": 0, "width": 1, "height": 1},
               "click": {"x": x, "y": 0, "width": 1, "height": 1},
               "targets": {"first": target}})
    };
    let task = json!({
        "schema_version": "0.9",
        "task_id": "pick",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "target_page": "result",
        "candidate_layouts": [{
            "id": "choices",
            "page_id": "neutral/home",
            "kind": "fixed_slots",
            "features": [{"name": "first", "value": "passed"}],
            "slots": [slot(0, "slot/first"), slot(1, "slot/second")]
        }],
        "operations": [
            step("pick_a", "choice_a"),
            step("pick_b", "choice_b"),
            step("pick_c", "choice_c")
        ]
    });
    let mut files = vec![
        (TASK, task),
        (
            RESOURCES,
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
    ];
    for (path, policy_id) in POLICIES
        .into_iter()
        .zip(["choice_a", "choice_b", "choice_c"])
    {
        files.push((path, policy(policy_id)));
    }
    for (path, document) in files {
        let file = temp.path().join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    }
    temp
}

/// The size on disk of the repository file `path`.
fn size(repo: &Path, path: &str) -> u64 {
    fs::metadata(repo.join(path)).unwrap().len()
}

/// The files a check of the three policy documents reads, each once: the task, its three
/// policies and the operations' resource table.
fn distinct_bytes(repo: &Path) -> u64 {
    [TASK, RESOURCES]
        .into_iter()
        .chain(POLICIES)
        .map(|path| size(repo, path))
        .sum()
}

/// Every policy document selected, none deleted.
fn selection() -> Vec<(PathBuf, bool)> {
    POLICIES
        .into_iter()
        .map(|path| (PathBuf::from(path), false))
        .collect()
}

// Each policy document selected: the run counts the distinct files it reads (the path list, the
// task, its three policies and the resource table) once each, so `read_bytes` is the sum of their
// sizes, not a multiple of it, and `read_files` names each once.
#[test]
fn rd_budget1_a_task_with_several_policies_counts_each_distinct_file_once() {
    let repo = repository();
    let list = POLICIES
        .iter()
        .map(|path| format!("M\0{path}\0"))
        .collect::<String>();
    fs::write(repo.path().join(LIST), list).unwrap();

    let result = crate::run_cli(
        [
            "--json",
            "resource",
            "validate",
            "--repo",
            repo.path().to_str().unwrap(),
            "--changed-paths-file",
            LIST,
        ],
        true,
    );
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let report = result.envelope.data.as_ref().unwrap();
    assert_eq!(report["status"], "valid", "{report}");
    let entries = POLICIES
        .iter()
        .map(|path| json!({"path": path, "status": "valid", "family": "selection_policy"}))
        .collect::<Vec<_>>();
    assert_eq!(report["entries"], json!(entries), "{report}");
    let mut read = vec![LIST, TASK, RESOURCES];
    read.extend(POLICIES);
    read.sort_unstable();
    assert_eq!(report["read_files"], json!(read), "{report}");
    let counted = distinct_bytes(repo.path()) + size(repo.path(), LIST);
    assert_eq!(report["read_bytes"], json!(counted), "{report}");
}

// The same selection with the total set to the distinct files' sizes passes, although checking
// each policy reads the policy, the task, every policy of the task and the resource table, then
// the policy once more, which counted at every read comes to several times the total.
#[test]
fn rd_budget1_a_selection_whose_distinct_files_fit_the_total_passes() {
    let repo = repository();
    let distinct = distinct_bytes(repo.path());
    let policies = POLICIES
        .into_iter()
        .map(|path| size(repo.path(), path))
        .sum::<u64>();
    let every_read = 2 * policies
        + POLICIES.len() as u64
            * (size(repo.path(), TASK) + policies + size(repo.path(), RESOURCES));
    assert!(every_read > distinct, "{every_read} > {distinct}");

    let reader = DeclarationReader::with_total(repo.path(), distinct).expect("reader");
    let validated = match validate_selection(reader, selection()) {
        Ok(validated) => validated,
        Err(failure) => panic!("the selection is refused: {}", failure.error.message),
    };
    assert_eq!(validated.answer["status"], "valid", "{}", validated.answer);
    assert_eq!(validated.read_bytes, distinct);
    assert_eq!(
        validated.answer["read_bytes"],
        json!(distinct),
        "{}",
        validated.answer
    );
}

// A total one byte below the distinct files' sizes refuses the selection as before:
// `resource_declaration_invalid`, "declaration read budget exceeded", naming the file whose first
// read crosses the line (the resource table, the last new file the first policy's check reads),
// at the first selected path; nothing past the line is counted.
#[test]
fn rd_budget1_distinct_files_over_the_total_are_refused_at_the_file_that_crosses() {
    let repo = repository();
    let distinct = distinct_bytes(repo.path());

    let reader = DeclarationReader::with_total(repo.path(), distinct - 1).expect("reader");
    let Err(failure) = validate_selection(reader, selection()) else {
        panic!("the selection over the total is refused");
    };
    assert_eq!(failure.error.code, "resource_declaration_invalid");
    assert_eq!(
        failure.error.message,
        format!("{RESOURCES}: declaration read budget exceeded")
    );
    assert_eq!(
        failure.error.details.as_ref().unwrap()["declaration_file"],
        RESOURCES
    );
    assert_eq!(failure.failed, Some(PathBuf::from(POLICIES[0])));
    assert_eq!(failure.read_bytes, distinct - size(repo.path(), RESOURCES));
    assert!(!failure.read_files.contains(RESOURCES));
}

// A second read of a path answers the bytes of the first and counts nothing: the file changed on
// disk in between, and the answer is still the first read's. A later read keeps its own limit.
#[test]
fn rd_budget1_a_second_read_of_a_path_answers_the_bytes_of_the_first() {
    let repo = repository();
    let mut reader = DeclarationReader::new(repo.path()).expect("reader");
    let path = Path::new(TASK);

    let first = reader.read(path, MAX_DOCUMENT_BYTES).expect("first read");
    assert_eq!(first, fs::read(repo.path().join(TASK)).unwrap());
    fs::write(repo.path().join(TASK), b"{}").unwrap();
    let second = reader.read(path, MAX_DOCUMENT_BYTES).expect("second read");
    assert_eq!(second, first);
    assert_eq!(reader.read_bytes, first.len() as u64);
    assert_eq!(reader.read_files, BTreeSet::from([TASK.to_owned()]));

    let error = reader
        .read(path, first.len() as u64 - 1)
        .expect_err("a read under a smaller limit");
    assert_eq!(
        error.message,
        format!("{TASK}: declaration is not a bounded regular file")
    );
}
