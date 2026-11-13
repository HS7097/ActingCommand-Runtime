// SPDX-License-Identifier: AGPL-3.0-only

//! The initiator (Workflow #434; `contracts/lab-two-entries.md` §4.1): the one word that says
//! where a request or an operation came from, from a closed list of six (Alice's answer 3 of
//! 2026-11-08: 「只记来自哪个组或者哪个组下的具体模块名就行」). A word is compared byte for byte
//! (UTF-8) and printed exactly as written; nothing else (no client, session or user) is part of
//! it. From LAB-6 the six words are a closed vocabulary of `runtime-client` 7 and `ledger` 8.

use crate::cli_line;
use crate::codes::{ActingctlCode, ArgumentProblem};
use crate::table::Entry;
use actingcommand_contract::outcome::Outcome;
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use std::ffi::OsStr;
use std::fmt;
use std::str::FromStr;

/// The environment variable the UI sets for every Lab process it starts (§4.2): `Lab-制包` for
/// its workbench's `serve`, `Lab-救急` for every other one.
pub const ENVIRONMENT_VARIABLE: &str = "ACTINGCOMMAND_INITIATOR";
/// Workflow #434 LAB-6S0 (§4.2): the marker beside [`ENVIRONMENT_VARIABLE`] that only the rescue
/// module's detached start sets, for a detached run whose initiator is `Lab-MCP`
/// (`lab_rescue::rescue`, its `detach`). With it set to [`DETACHED_RUN_MARK`] a Lab process takes
/// `Lab-MCP` from the variable ([`Initiator::of_lab_process_marked`]).
pub const DETACHED_RUN_VARIABLE: &str = "ACTINGCOMMAND_DETACHED_RUN";
/// The one value of [`DETACHED_RUN_VARIABLE`] that marks a run, compared byte for byte: the module
/// whose detached start sets it.
pub const DETACHED_RUN_MARK: &str = "rescue";

/// Where a request or an operation came from: the group (`UI`) or the Lab module (§4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Initiator {
    /// `UI`: the console (acui) on the `ui` entry; fixed by the binding, never declared.
    Ui,
    /// `Lab-CLI`: `actinglab.exe` started from a command line, by a person or a script.
    LabCli,
    /// `Lab-MCP`: `actinglab-mcp.exe`, for an MCP client.
    LabMcp,
    /// `Lab-制包`: the Lab process the UI starts for its authoring workbench
    /// (`actinglab serve --stdio`).
    LabAuthoring,
    /// `Lab-救急`: a Lab process the UI starts outside its workbench (the launcher, acsetup, the
    /// settings page).
    LabRescue,
    /// `Lab-看门狗`: `actingwatch.exe`.
    LabWatchdog,
}

impl Initiator {
    /// The six words, in the contract's order (§4.1).
    pub const ALL: [Self; 6] = [
        Self::Ui,
        Self::LabCli,
        Self::LabMcp,
        Self::LabAuthoring,
        Self::LabRescue,
        Self::LabWatchdog,
    ];

    /// The word, exactly as the contract writes it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ui => "UI",
            Self::LabCli => "Lab-CLI",
            Self::LabMcp => "Lab-MCP",
            Self::LabAuthoring => "Lab-制包",
            Self::LabRescue => "Lab-救急",
            Self::LabWatchdog => "Lab-看门狗",
        }
    }

    /// The entry a connection with this initiator binds (§2.2): `ui` for `UI`, `lab` for the
    /// five Lab words.
    pub const fn entry(self) -> Entry {
        match self {
            Self::Ui => Entry::Ui,
            Self::LabCli
            | Self::LabMcp
            | Self::LabAuthoring
            | Self::LabRescue
            | Self::LabWatchdog => Entry::Lab,
        }
    }

    /// The initiator `word` names, compared byte for byte: no trimming, no case folding. A word
    /// outside the six is `actingctl_usage_invalid` with `argument_problem` `not_recognised`,
    /// naming the word as `argument` when a name may hold it.
    pub fn parse(word: &str) -> Result<Self, Outcome> {
        Self::ALL
            .into_iter()
            .find(|initiator| initiator.as_str() == word)
            .ok_or_else(|| cli_line::usage_invalid(Some(word), ArgumentProblem::NotRecognised))
    }

    /// Workflow #434 LAB-2 (§4.2): the word of an `actinglab.exe` process, its CLI and `serve`,
    /// given the value of [`ENVIRONMENT_VARIABLE`] (`None` when it is not set): `Lab-CLI`, unless
    /// the variable names `Lab-制包` or `Lab-救急`. Any other value, `UI`, `Lab-MCP` and
    /// `Lab-看门狗` included, is `actingctl_usage_invalid` with `variable`
    /// `ACTINGCOMMAND_INITIATOR`, and the process sends and writes nothing. This is the rule
    /// without the marker of Workflow #434 LAB-6S0 ([`Self::of_lab_process_marked`]).
    pub fn of_lab_process(variable: Option<&OsStr>) -> Result<Self, Outcome> {
        let Some(value) = variable else {
            return Ok(Self::LabCli);
        };
        [Self::LabAuthoring, Self::LabRescue]
            .into_iter()
            .find(|initiator| value.to_str() == Some(initiator.as_str()))
            .ok_or_else(|| {
                Outcome::new(ActingctlCode::ActingctlUsageInvalid).variable(ENVIRONMENT_VARIABLE)
            })
    }

    /// Workflow #434 LAB-6S0 (§4.2): [`Self::of_lab_process`], but `Lab-MCP` in the variable is
    /// taken when `marker`, the value of [`DETACHED_RUN_VARIABLE`] (`None` when it is not set), is
    /// [`DETACHED_RUN_MARK`]: the rescue module's detached run of an operation an MCP server
    /// started. The marker changes nothing else: alone, beside another word, or with another value
    /// the answer is [`Self::of_lab_process`]'s.
    pub fn of_lab_process_marked(
        variable: Option<&OsStr>,
        marker: Option<&OsStr>,
    ) -> Result<Self, Outcome> {
        let marked = marker.and_then(OsStr::to_str) == Some(DETACHED_RUN_MARK);
        if marked && variable.and_then(OsStr::to_str) == Some(Self::LabMcp.as_str()) {
            return Ok(Self::LabMcp);
        }
        Self::of_lab_process(variable)
    }

    /// [`Self::of_lab_process_marked`] with this process's environment.
    pub fn of_this_lab_process() -> Result<Self, Outcome> {
        Self::of_lab_process_marked(
            std::env::var_os(ENVIRONMENT_VARIABLE).as_deref(),
            std::env::var_os(DETACHED_RUN_VARIABLE).as_deref(),
        )
    }
}

impl fmt::Display for Initiator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Initiator {
    type Err = Outcome;

    fn from_str(word: &str) -> Result<Self, Self::Err> {
        Self::parse(word)
    }
}

impl Serialize for Initiator {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Initiator {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let word = String::deserialize(deserializer)?;
        Self::ALL
            .into_iter()
            .find(|initiator| initiator.as_str() == word)
            .ok_or_else(|| de::Error::unknown_variant(&word, &WORDS))
    }
}

/// The six words, for a decoder's refusal.
static WORDS: [&str; 6] = [
    Initiator::Ui.as_str(),
    Initiator::LabCli.as_str(),
    Initiator::LabMcp.as_str(),
    Initiator::LabAuthoring.as_str(),
    Initiator::LabRescue.as_str(),
    Initiator::LabWatchdog.as_str(),
];
