// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #411 RV-1: the read-only Git plumbing `resource verify` runs (contracts
//! `resource-verify.md`, "Git"). `git` comes from `PATH`, runs in `--repo` with
//! `GIT_OPTIONAL_LOCKS=0` (no index refresh) and `-c core.quotepath=off`, and only the commands
//! of [`GitCommand`] run: nothing writes the index, a ref or the work tree. The variables that
//! would point Git at another repository, work tree or index are not passed on, so `--repo`
//! alone names the repository.

use crate::codes::{ActinglabCode, GitCommand};
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{IoOp, Outcome, RawSource};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// Bytes of git's stderr one `raw_text` keeps (the key table's bound).
const RAW_TEXT_LIMIT: usize = 4096;
/// The variables that redirect Git away from `--repo`.
const REDIRECTING_VARIABLES: [&str; 4] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
];

pub(super) struct Git {
    repo: PathBuf,
}

impl Git {
    pub(super) fn new(repo: &Path) -> Self {
        Self {
            repo: repo.to_path_buf(),
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        command
            .current_dir(&self.repo)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .args(["-c", "core.quotepath=off"])
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for variable in REDIRECTING_VARIABLES {
            command.env_remove(variable);
        }
        command
    }

    /// Runs one command. An exit code in `answers` is an answer of the command, as 0 always is;
    /// any other ends in `resource_verify_git_failed`.
    pub(super) fn run(
        &self,
        which: GitCommand,
        args: &[&str],
        answers: &[i32],
    ) -> Result<(i32, Vec<u8>), Outcome> {
        let output = self
            .command(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| failed_to_start(which, &error))?;
        answer(which, output, answers)
    }

    /// Runs one command that reads `input` on its stdin. The input is written beside the read
    /// of the answer, so neither pipe can fill while the other waits.
    pub(super) fn run_with_input(
        &self,
        which: GitCommand,
        args: &[&str],
        input: &[u8],
    ) -> Result<Vec<u8>, Outcome> {
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|error| failed_to_start(which, &error))?;
        let stdin = child.stdin.take();
        let (output, written) = std::thread::scope(|scope| {
            let writer = scope.spawn(move || match stdin {
                Some(mut stdin) => stdin.write_all(input),
                None => Ok(()),
            });
            let output = child.wait_with_output();
            (output, writer.join())
        });
        let output = output.map_err(|error| {
            Outcome::new(ActinglabCode::ResourceVerifyGitFailed)
                .git_command(which)
                .caused_by(Outcome::from_io_error(&error, IoOp::Wait))
        })?;
        let (_, stdout) = answer(which, output, &[])?;
        match written {
            Ok(Ok(())) => Ok(stdout),
            Ok(Err(error)) => Err(Outcome::new(ActinglabCode::ResourceVerifyGitFailed)
                .git_command(which)
                .caused_by(Outcome::from_io_error(&error, IoOp::Write))),
            Err(_) => Err(Outcome::new(ActinglabCode::ResourceVerifyGitFailed).git_command(which)),
        }
    }

    /// `git rev-parse --verify --quiet <rev>`: the object it names, or `None` when it names
    /// none (exit 1).
    pub(super) fn resolve(&self, rev: &str) -> Result<Option<String>, Outcome> {
        let (code, stdout) = self.run(
            GitCommand::RevParse,
            &["rev-parse", "--verify", "--quiet", rev],
            &[1],
        )?;
        Ok((code == 0).then(|| text(&stdout).trim().to_owned()))
    }

    /// `git merge-base --is-ancestor <ancestor> <descendant>` (exit 1: it is not).
    pub(super) fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool, Outcome> {
        let (code, _) = self.run(
            GitCommand::MergeBase,
            &["merge-base", "--is-ancestor", ancestor, descendant],
            &[1],
        )?;
        Ok(code == 0)
    }
}

/// The NUL-separated fields of a `-z` answer, as text.
pub(super) fn nul_fields(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .map(text)
        .collect()
}

pub(super) fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn answer(which: GitCommand, output: Output, answers: &[i32]) -> Result<(i32, Vec<u8>), Outcome> {
    match output.status.code() {
        Some(code) if code == 0 || answers.contains(&code) => Ok((code, output.stdout)),
        code => Err(exited(which, code, &output.stderr)),
    }
}

fn failed_to_start(which: GitCommand, error: &std::io::Error) -> Outcome {
    Outcome::new(ActinglabCode::ResourceVerifyGitFailed)
        .git_command(which)
        .caused_by(Outcome::from_io_error(error, IoOp::Spawn))
}

/// git ended with a code that is no answer: its stderr is the cause's `raw_text`, cut at the
/// bound (the whole text goes to the developer log).
fn exited(which: GitCommand, code: Option<i32>, stderr: &[u8]) -> Outcome {
    let stderr = text(stderr);
    let mut end = stderr.len().min(RAW_TEXT_LIMIT);
    while !stderr.is_char_boundary(end) {
        end -= 1;
    }
    let mut cause = Outcome::new(ContractCode::ForeignOtherError)
        .raw_source(RawSource::Process)
        .raw_text(&stderr[..end]);
    if end < stderr.len() {
        eprintln!("WARN actinglab: resource verify git {which}: {stderr}");
        cause = cause.raw_text_truncated(true);
    }
    let mut outcome = Outcome::new(ActinglabCode::ResourceVerifyGitFailed).git_command(which);
    if let Some(code) = code {
        cause = cause.exit_code(code);
        outcome = outcome.exit_code(code);
    }
    outcome.caused_by(cause)
}
