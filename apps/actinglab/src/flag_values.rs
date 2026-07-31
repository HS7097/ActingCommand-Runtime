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

pub(super) fn required_non_empty_flag(flags: &FlagArgs, name: &str) -> CliOutcome<String> {
    let value = flags.required(name)?;
    if value.trim().is_empty() {
        return Err(CliError::usage(format!("{name} must not be empty")));
    }
    Ok(value)
}

pub(super) fn parse_optional_unit_f64(flags: &FlagArgs, name: &str) -> CliOutcome<Option<f64>> {
    let Some(value) = flags.optional(name) else {
        return Ok(None);
    };
    if value == "true" {
        return Err(CliError::usage(format!("missing {name} <value>")));
    };
    let parsed = value
        .parse::<f64>()
        .map_err(|err| CliError::usage(format!("failed to parse {name} '{value}': {err}")))?;
    if !parsed.is_finite() || !(0.0..=1.0).contains(&parsed) {
        return Err(CliError::usage(format!(
            "{name} must be a finite number between 0 and 1"
        )));
    }
    Ok(Some(parsed))
}

pub(super) fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}
