// SPDX-License-Identifier: AGPL-3.0-only

//! `<root>\watchdog\watchdog.log`: one UTF-8 line per decision change, start, error or
//! install, `<RFC3339 UTC> <unix_ms> <LEVEL> <code> key=value…`. The launcher appends its
//! own failures and the tick's stderr to the same file.

use super::Failure;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

pub(crate) const LOG_FILE: &str = "watchdog.log";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

pub(crate) fn append(
    directory: &Path,
    now_unix_ms: u64,
    level: Level,
    code: &str,
    fields: &[(&str, String)],
) -> Result<(), Failure> {
    let path = directory.join(LOG_FILE);
    let line = format_line(now_unix_ms, level, code, fields);
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(line.as_bytes()))
        .map_err(|error| {
            Failure::misconfigured(
                "watchdog_log_unwritable",
                format!("{}: {error}", path.display()),
            )
        })
}

/// Whether the log's last line is `<code>` with `code=<value>`; a misconfiguration found
/// before the state is readable is written once, not every minute.
pub(crate) fn last_line_matches(directory: &Path, code: &str, value: &str) -> bool {
    let mut tail = Vec::new();
    let read = File::open(directory.join(LOG_FILE)).and_then(|mut file| {
        let length = file.metadata()?.len();
        file.seek(SeekFrom::Start(length.saturating_sub(4096)))?;
        file.read_to_end(&mut tail)
    });
    if read.is_err() {
        return false;
    }
    let wanted = format!("code={value}");
    String::from_utf8_lossy(&tail)
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| {
            line.split(' ').nth(3) == Some(code) && line.split(' ').any(|field| field == wanted)
        })
}

fn format_line(now_unix_ms: u64, level: Level, code: &str, fields: &[(&str, String)]) -> String {
    let mut line = format!(
        "{} {now_unix_ms} {} {code}",
        rfc3339_utc(now_unix_ms),
        level.as_str()
    );
    for (key, value) in fields {
        line.push(' ');
        line.push_str(key);
        line.push('=');
        if value.is_empty() || value.contains(|c: char| c.is_whitespace() || c == '"' || c == '=') {
            line.push('"');
            line.push_str(&value.replace('"', "\\\"").replace(['\r', '\n'], " "));
            line.push('"');
        } else {
            line.push_str(value);
        }
    }
    line.push('\n');
    line
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ`, from days since the epoch to the proleptic Gregorian date.
pub(crate) fn rfc3339_utc(unix_ms: u64) -> String {
    let seconds = unix_ms / 1000;
    let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX / 2);
    let second_of_day = seconds % 86_400;
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        second_of_day / 3_600,
        second_of_day % 3_600 / 60,
        second_of_day % 60,
        unix_ms % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_carry_utc_time_level_code_and_quoted_fields() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339_utc(1_759_800_000_123), "2025-10-07T01:20:00.123Z");
        assert_eq!(rfc3339_utc(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(rfc3339_utc(4_102_444_799_999), "2099-12-31T23:59:59.999Z");
        assert_eq!(
            format_line(
                1_759_800_000_123,
                Level::Warn,
                "watchdog_started_runtime",
                &[
                    ("method", "breakaway".to_owned()),
                    ("log", r"C:\Install Root\watchdog\actingd-1.log".to_owned()),
                    ("line", "FATAL actingd: \"x\"".to_owned()),
                ],
            ),
            "2025-10-07T01:20:00.123Z 1759800000123 WARN watchdog_started_runtime method=breakaway log=\"C:\\Install Root\\watchdog\\actingd-1.log\" line=\"FATAL actingd: \\\"x\\\"\"\n"
        );
    }
}
