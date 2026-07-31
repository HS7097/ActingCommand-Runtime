use super::{CliError, CliOutcome, FlagArgs};
use std::time::Duration;

pub(super) fn parse_optional_duration_ms(
    flags: &FlagArgs,
    name: &str,
    default_ms: u64,
) -> CliOutcome<Duration> {
    let Some(value) = flags.optional(name).filter(|value| value != "true") else {
        return Ok(Duration::from_millis(default_ms));
    };
    let ms = value
        .parse::<u64>()
        .map_err(|err| CliError::usage(format!("failed to parse {name} '{value}': {err}")))?;
    Ok(Duration::from_millis(ms))
}

pub(super) fn parse_optional_usize(
    flags: &FlagArgs,
    name: &str,
    default_value: usize,
) -> CliOutcome<usize> {
    let Some(value) = flags.optional(name).filter(|value| value != "true") else {
        return Ok(default_value);
    };
    value
        .parse::<usize>()
        .map_err(|err| CliError::usage(format!("failed to parse {name} '{value}': {err}")))
}

pub(super) fn parse_optional_string_value(
    flags: &FlagArgs,
    name: &str,
) -> CliOutcome<Option<String>> {
    match flags.optional(name) {
        None => Ok(None),
        Some(value) if value == "true" => Err(CliError::usage(format!("missing {name} <value>"))),
        Some(value) if value.trim().is_empty() => {
            Err(CliError::usage(format!("{name} must not be empty")))
        }
        Some(value) => Ok(Some(value)),
    }
}

pub(super) fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}
