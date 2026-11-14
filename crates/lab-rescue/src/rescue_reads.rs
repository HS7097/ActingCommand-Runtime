// SPDX-License-Identifier: AGPL-3.0-only

//! The parameters the code of each row [`crate::rescue`] serves reads, and the command line each
//! row whose answer is one hands on (Workflow #434 LAB-5B1b; register §1.287, "Slice LAB-5B1b";
//! `contracts/lab-two-entries.md` §9.1): one constant per served row, of the shapes Workflow #434
//! LAB-5B1 made for the core rows (`actingcommand_lab_ops::core_reads`), which that code reads the
//! row's parameters through ([`Params`], with its debug assertion on a read outside the constant)
//! and builds the command line from. The slice's tests hold each equal to its row's declared
//! parameters in both directions. Workflow #434 LAB-5B1c (register §1.287, "Slice LAB-5B1c"): and
//! the flags each command line writes held to what its receiver takes (the installer's lists,
//! `crate::install::FlagLists` beside each command, and the agent channel's,
//! `actingcommand_agent_forward::cli::*_FLAGS`, each one constant its parse reads), each receiver
//! fed every declared parameter; nothing is left for a later slice.
//!
//! - **Read in this process** ([`READ_HERE`]): the rows whose parameters the module reads itself
//!   ([`Reads`]): the composites, the reads, the starts and the MCP rows.
//! - **Handed to the installer** ([`INSTALLER`]): the rows whose answer is an `install` command
//!   (`crate::install::execute_as`), each with its command line ([`Handed`]).
//! - **Handed to the agent channel** ([`AGENT`]): the `agent` rows, each with the words after
//!   `agent-channel` (`crate::agent_channel::run`).
//! - **Exceptions** ([`EXCEPTIONS`]): the names outside the operation table that the code reads,
//!   each with its reason; none since LAB-5B1c.
//!
//! A handed row's parts are written in their place, but its `Written::Each` parameters and
//! `Part::Further`: those come last, together, member by member in the order the caller gave them
//! (`rescue::further_flags`, as before the slice), every member no other part writes under the flag
//! of its name. A constant without `Part::Further` hands nothing more on (`up`, `down`).

use actingcommand_lab_ops::core_reads::{Exception, Handed, Params, Part, Reads, Written};
use serde_json::{Map, Value};

const fn reads(row: &'static str, names: &'static [&'static str]) -> Reads {
    Reads { row, names }
}

/// The rows whose parameters the module reads in its process.
pub static READ_HERE: [Reads; 20] = [
    reads("restart", &[]),
    reads("alive", &["wait"]),
    reads("deploy", &["target", "plan", "detach", "dir"]),
    reads("rollback", &["plan", "detach", "to"]),
    reads("config replace", &["file", "sha"]),
    // Workflow #434 LAB-5B1c: the row declares `sha`, which `rescue::config_swap` reads for it.
    reads("config restore", &["backup", "sha"]),
    reads(
        "instance add",
        &[
            "alias",
            "game",
            "server",
            "mumu_index",
            "touch_backend",
            "capture_backend",
            // Workflow #452: optional; without it the id the game's root declares.
            "application_id",
        ],
    ),
    reads("place", &["bundle", "sums", "facts", "plan"]),
    reads("unlock", &[]),
    // Workflow #453 S12: the live build's `actingd claim-state-root`, `--new-identity` when given.
    reads("state-root claim", &["new_identity"]),
    reads("check-config", &["file"]),
    reads("ledger verify", &[]),
    // Workflow #453 S10: the live build's `actingd discover`; the row declares no parameter.
    reads("emulators", &[]),
    // Workflow #453 S7: the live build's `actingd vision manifest --provider <provider>`, and its
    // `actingd vision check`, which declares no parameter.
    reads("vision manifest", &["provider"]),
    reads("vision check", &[]),
    reads("watchdog install", &[]),
    reads("watchdog uninstall", &[]),
    reads("mcp plan", &["entry_protocol", "current_build"]),
    reads("mcp entries", &["all"]),
    reads("mcp config", &["client", "list"]),
];

const fn handed(
    row: &'static str,
    command: &'static [&'static str],
    parts: &'static [Part],
) -> Handed {
    Handed {
        row,
        command,
        only_with: None,
        parts,
    }
}

const fn flag(name: &'static str, flag: &'static str) -> Part {
    Part::Param(name, Written::Flag(flag))
}

const fn each(name: &'static str, flag: &'static str) -> Part {
    Part::Param(name, Written::Each(flag))
}

/// `op`, an installer operation's id, as the installer takes it.
const OPERATION: Part = flag("op", "--operation");
/// `dir`, a release folder, as the installer takes it.
const DIR: Part = flag("dir", "--dir");
/// A release's facts and what it is expected to be, as the installer takes them.
const FACTS: Part = each("facts", "--facts");
const EXPECT_TAG: Part = each("expect_tag", "--expect-tag");
const EXPECT_COMMIT: Part = each("expect_commit", "--expect-commit");
/// A channel's name, as the agent channel takes it.
const NAME: Part = each("name", "--name");

/// The rows whose answer is an `install` command: the command's words, then `--root` and the
/// module's root where it takes one.
pub static INSTALLER: [Handed; 11] = [
    handed("up", &["start"], &[Part::Root]),
    // The hold's reason is the module's own when the caller names none (`rescue::down`).
    handed("down", &["stop"], &[Part::Root, flag("reason", "--hold")]),
    handed("undo", &["undo"], &[Part::Root, OPERATION, Part::Further]),
    handed(
        "op status",
        &["status"],
        &[
            Part::Root,
            OPERATION,
            each("check_rollback", "--check-rollback"),
            Part::Further,
        ],
    ),
    handed(
        "op resume",
        &["resume"],
        &[Part::Root, OPERATION, Part::Further],
    ),
    handed("leftovers", &["leftovers"], &[Part::Root, Part::Further]),
    handed(
        "guards",
        &["guards"],
        &[Part::Root, flag("for", "--for"), Part::Further],
    ),
    handed(
        "release-hold",
        &["release-hold"],
        &[Part::Root, flag("reason", "--reason"), Part::Further],
    ),
    handed(
        "release verify",
        &["release", "verify"],
        &[DIR, FACTS, EXPECT_TAG, EXPECT_COMMIT, Part::Further],
    ),
    handed(
        "release stage",
        &["release", "stage"],
        &[
            Part::Root,
            DIR,
            FACTS,
            EXPECT_TAG,
            EXPECT_COMMIT,
            each("local_build", "--local-build"),
            Part::Further,
        ],
    ),
    handed("adopt", &["adopt"], &[Part::Root, Part::Further]),
];

/// The `agent` rows: the agent channel's command, then `--state-root` and the module's state root
/// where it takes one.
pub static AGENT: [Handed; 6] = [
    handed(
        "agent add",
        &["add"],
        &[
            Part::StateRoot,
            NAME,
            each("client", "--client"),
            each("native_id", "--native-id"),
            each("dsh_app_root", "--dsh-app-root"),
            each("dsh_home", "--dsh-home"),
            each("dsh_node", "--dsh-node"),
            each("restore", "--restore"),
            Part::Further,
        ],
    ),
    handed(
        "agent remove",
        &["remove"],
        &[Part::StateRoot, NAME, Part::Further],
    ),
    handed("agent list", &["list"], &[Part::StateRoot, Part::Further]),
    handed(
        "agent claude-hook",
        &["claude-hook"],
        &[
            Part::Param("action", Written::Word),
            Part::StateRoot,
            Part::Further,
        ],
    ),
    handed(
        "agent claude-register",
        &["claude-register"],
        &[Part::StateRoot, NAME, Part::Further],
    ),
    handed("agent notice", &["notice"], &[Part::Further]),
];

/// The exceptions, each with its reason: none. Workflow #434 LAB-5B1c: LAB-5B1b's one exception,
/// `config restore`'s `sha`, is gone, since the row declares the parameter (the coordinator's
/// decision on LAB-5B1b's open point; the table follows the code).
pub static EXCEPTIONS: [Exception; 0] = [];

/// The parameters `map` of the row `name` the module reads in its process, read through its
/// constant; `None` for another row.
pub(crate) fn read_here<'a>(name: &str, map: &'a Map<String, Value>) -> Option<Params<'a>> {
    READ_HERE
        .iter()
        .find(|reads| reads.row == name)
        .map(|reads| Params::reading(reads, &EXCEPTIONS, map))
}

/// The constant of the installer row `name`.
pub(crate) fn installer(name: &str) -> Option<&'static Handed> {
    INSTALLER.iter().find(|handed| handed.row == name)
}

/// The constant of the agent row `name`.
pub(crate) fn agent(name: &str) -> Option<&'static Handed> {
    AGENT.iter().find(|handed| handed.row == name)
}

/// The parameters `map` of the handed row `handed`, read through its constant.
pub(crate) fn handing<'a>(handed: &'static Handed, map: &'a Map<String, Value>) -> Params<'a> {
    Params::handing(handed, &EXCEPTIONS, map)
}
