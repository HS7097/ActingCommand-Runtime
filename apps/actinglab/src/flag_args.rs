// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub(super) struct FlagArgs {
    pub(super) flags: BTreeMap<String, Vec<String>>,
    pub(super) positionals: Vec<String>,
}

impl FlagArgs {
    pub(super) fn parse(args: &[String]) -> CliOutcome<Self> {
        Self::parse_with_required_values(args, false)
    }

    pub(super) fn parse_values(args: &[String]) -> CliOutcome<Self> {
        Self::parse_with_required_values(args, true)
    }

    fn parse_with_required_values(args: &[String], require_values: bool) -> CliOutcome<Self> {
        let mut parsed = Self::default();
        let mut index = 0usize;
        while index < args.len() {
            let arg = &args[index];
            if arg.starts_with("--") {
                if index + 1 < args.len() && !args[index + 1].starts_with("--") {
                    parsed
                        .flags
                        .entry(arg.clone())
                        .or_default()
                        .push(args[index + 1].clone());
                    index += 2;
                } else {
                    if require_values {
                        return Err(CliError::usage(format!("missing {arg} <value>")));
                    }
                    parsed
                        .flags
                        .entry(arg.clone())
                        .or_default()
                        .push("true".to_string());
                    index += 1;
                }
            } else {
                parsed.positionals.push(arg.clone());
                index += 1;
            }
        }
        Ok(parsed)
    }

    pub(super) fn bool(&self, name: &str) -> bool {
        self.flags
            .get(name)
            .and_then(|values| values.last())
            .is_some_and(|value| value == "true")
    }

    pub(super) fn optional(&self, name: &str) -> Option<String> {
        self.flags
            .get(name)
            .and_then(|values| values.last())
            .cloned()
    }

    pub(super) fn values(&self, name: &str) -> Vec<String> {
        self.flags.get(name).cloned().unwrap_or_default()
    }

    pub(super) fn without_first_positional(&self) -> Self {
        let mut next = self.clone();
        if !next.positionals.is_empty() {
            next.positionals.remove(0);
        }
        next
    }

    pub(super) fn required(&self, name: &str) -> CliOutcome<String> {
        self.optional(name)
            .filter(|value| value != "true")
            .ok_or_else(|| CliError::usage(format!("missing {name} <value>")))
    }

    pub(super) fn optional_path(&self, name: &str) -> Option<PathBuf> {
        self.optional(name)
            .filter(|value| value != "true")
            .map(PathBuf::from)
    }

    pub(super) fn required_path(&self, name: &str) -> CliOutcome<PathBuf> {
        self.required(name).map(PathBuf::from)
    }

    pub(super) fn reject_flags(&self, command: &str) -> CliOutcome<()> {
        if self.flags.is_empty() {
            return Ok(());
        }
        let names = self.flags.keys().cloned().collect::<Vec<_>>();
        Err(CliError::usage(format!(
            "{command} takes positional arguments only; unexpected flags: {}",
            names.join(", ")
        )))
    }

    pub(super) fn expect_positionals(&self, command: &str, expected: usize) -> CliOutcome<()> {
        if self.positionals.len() == expected {
            return Ok(());
        }
        Err(CliError::usage(format!(
            "{command} expects {expected} positional argument(s), got {}",
            self.positionals.len()
        )))
    }

    pub(super) fn required_positional(&self, index: usize, name: &str) -> CliOutcome<&str> {
        self.positionals
            .get(index)
            .map(String::as_str)
            .ok_or_else(|| CliError::usage(format!("missing {name}")))
    }

    pub(super) fn required_i32(&self, index: usize, name: &str) -> CliOutcome<i32> {
        let value = self.required_positional(index, name)?;
        value
            .parse::<i32>()
            .map_err(|err| CliError::usage(format!("failed to parse {name} '{value}': {err}")))
    }

    pub(super) fn required_u64(&self, index: usize, name: &str) -> CliOutcome<u64> {
        let value = self.required_positional(index, name)?;
        value
            .parse::<u64>()
            .map_err(|err| CliError::usage(format!("failed to parse {name} '{value}': {err}")))
    }
}
