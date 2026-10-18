// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 A5 (M-PKG S5-T4; runtime-interface.md "ac_pack_check": one tool = one command,
// parameter names = flag names; register §1.97 PC2; apps key 71): MCP `ac_pack_check {package}`
// runs actinglab `package check --package <package>`, after `--json --via mcp`, and answers that
// command's data verbatim, `{status, package_id, entry_task_id}`, not the interim
// `{preflight}` of PKG-S4.

use super::super::{PACK_CHECK, Program, command_end, command_line, interpret};
use crate::child::Captured;
use serde_json::{Map, json};
use std::path::PathBuf;
use std::process::ExitStatus;

const PACKAGE: &str = "neutral-packs/neutral.check.source";

#[cfg(windows)]
fn exited(code: u8) -> ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    ExitStatus::from_raw(u32::from(code))
}

#[cfg(unix)]
fn exited(code: u8) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    ExitStatus::from_raw(i32::from(code) << 8)
}

#[test]
fn ac_pack_check_runs_actinglab_package_check_and_answers_its_data_verbatim() {
    let program = Program {
        actinglab: PathBuf::from("neutral-install")
            .join("tools")
            .join("actinglab.exe"),
        root: PathBuf::from("neutral-install"),
        config: PathBuf::from("neutral-install").join("actingd.config.json"),
        installation: None,
        state_root: Ok(PathBuf::from("neutral-state")),
    };
    let mut arguments = Map::new();
    arguments.insert("package".to_owned(), json!(PACKAGE));
    let line = command_line(&PACK_CHECK, &arguments).unwrap_or_else(|error| {
        panic!(
            "ac_pack_check with a package was refused: {}",
            error.to_value()
        )
    });
    let command = program.command(&line);
    assert_eq!(command.get_program(), program.actinglab.as_os_str());
    let words = command
        .get_args()
        .map(|word| word.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        words,
        [
            "--json",
            "--via",
            "mcp",
            "package",
            "check",
            "--package",
            PACKAGE
        ]
    );

    // The answer is the command's data, unchanged: no `preflight` wrapper.
    let data = json!({
        "status": "admitted",
        "package_id": "neutral.check.source",
        "entry_task_id": "task",
    });
    let envelope = json!({"ok": true, "command": "package check", "data": data.clone()});
    let answer = interpret(
        &Captured {
            status: exited(0),
            stdout: envelope.to_string().into_bytes(),
            stderr: Vec::new(),
        },
        "package check",
    );
    let (result, warnings) = command_end(
        &PACK_CHECK,
        "lab_job_gate_0121_pkgcheck",
        answer.result,
        answer.req_id.as_deref(),
        Vec::new(),
    );
    let result = result
        .unwrap_or_else(|error| panic!("ac_pack_check answered an error: {}", error.to_value()));
    assert_eq!(result, data);
    assert!(warnings.is_empty(), "{warnings:?}");
}
