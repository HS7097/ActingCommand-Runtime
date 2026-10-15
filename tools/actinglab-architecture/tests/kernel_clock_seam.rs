// SPDX-License-Identifier: AGPL-3.0-only

// Test plan E-3 (EXEC-I3, test plan v2.2 §3.10): every timer and every wait of the
// contained-task kernel goes through its runtime's clock, `ContainedTaskRuntime::monotonic_now`
// and `ContainedTaskRuntime::pause`. Two clocks in one run would compare instants of different
// clocks, so the production code of `crates/execution-kernel/src/contained_task.rs` and of every
// production module below it calls neither `Instant::now` nor `thread::sleep` (also not through
// an import), except the two default bodies of the seam itself. The modules are found by
// following each file's production `mod name;` declarations; test modules (`#[cfg(test)]`) are
// not read. An allow-list entry that matches nothing fails, and so does a scan that misses one
// of the modules the test plan names.

use actingcommand_actinglab_architecture::inspect_kernel_clock_reads;
use std::fs;
use std::path::{Path, PathBuf};

const KERNEL_ROOT: &str = "crates/execution-kernel/src/contained_task.rs";
// `(file, function, token)`: the seam's own default bodies, each allowed exactly once.
const SEAM_DEFAULTS: &[(&str, &str, &str)] = &[
    (
        "crates/execution-kernel/src/contained_task.rs",
        "ContainedTaskRuntime::monotonic_now",
        "Instant::now",
    ),
    (
        "crates/execution-kernel/src/contained_task.rs",
        "ContainedTaskRuntime::pause",
        "thread::sleep",
    ),
];
// The modules test plan v2.2 §3.10 names besides the root; the scan must reach each one.
const NAMED_MODULES: &[&str] = &[
    "crates/execution-kernel/src/contained_task/linear.rs",
    "crates/execution-kernel/src/contained_task/observation_window.rs",
    "crates/execution-kernel/src/contained_task/readings.rs",
    "crates/execution-kernel/src/contained_task/scan/online.rs",
    "crates/execution-kernel/src/contained_task/selection.rs",
];

#[test]
fn contained_task_kernel_reads_its_clock_and_waits_only_through_the_runtime_seam() {
    let root = workspace_root();
    let mut violations = Vec::new();
    let mut matched = vec![0_usize; SEAM_DEFAULTS.len()];
    let mut scanned = Vec::new();
    let mut pending = vec![root.join(KERNEL_ROOT)];
    while let Some(file) = pending.pop() {
        let relative = file
            .strip_prefix(&root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        let source = fs::read_to_string(&file)
            .unwrap_or_else(|error| panic!("read {}: {error}", file.display()));
        let scan = match inspect_kernel_clock_reads(&relative, &source) {
            Ok(scan) => scan,
            Err(error) => {
                violations.push(error);
                continue;
            }
        };
        for module in &scan.modules {
            match module_file(&file, module) {
                Some(child) => pending.push(child),
                None => violations.push(format!("{relative}: module `{module}` has no file")),
            }
        }
        for finding in scan.findings {
            let entry = SEAM_DEFAULTS.iter().position(|(path, function, token)| {
                relative == *path
                    && finding.function.as_deref() == Some(*function)
                    && finding.token == *token
            });
            if let Some(index) = entry {
                matched[index] += 1;
                if matched[index] == 1 {
                    continue;
                }
            }
            violations.push(format!(
                "{relative}:{} in fn `{}`: {}",
                finding.line,
                finding.function.as_deref().unwrap_or("<none>"),
                finding.token
            ));
        }
        scanned.push(relative);
    }
    for ((path, function, token), count) in SEAM_DEFAULTS.iter().zip(&matched) {
        if *count == 0 {
            violations.push(format!(
                "seam default ({path}, fn `{function}`, `{token}`) matches nothing"
            ));
        }
    }
    for module in NAMED_MODULES {
        if !scanned.iter().any(|path| path == module) {
            violations.push(format!("{module} was not reached from {KERNEL_ROOT}"));
        }
    }

    assert!(
        violations.is_empty(),
        "contained-task kernel clock reads outside the runtime seam (test plan §3.10 E-3):\n{}",
        violations.join("\n")
    );
}

// The file of the child module `module` declared in `parent`: `<dir>/<module>.rs` or
// `<dir>/<module>/mod.rs`, where `<dir>` is the parent's directory for a `mod.rs` and the
// directory named after the parent's file otherwise.
fn module_file(parent: &Path, module: &str) -> Option<PathBuf> {
    let directory = if parent.file_name().is_some_and(|name| name == "mod.rs") {
        parent.parent()?.to_path_buf()
    } else {
        parent.parent()?.join(parent.file_stem()?)
    };
    [
        directory.join(format!("{module}.rs")),
        directory.join(module).join("mod.rs"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the architecture tool sits two levels below the workspace root")
        .to_path_buf()
}
