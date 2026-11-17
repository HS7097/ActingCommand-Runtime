// SPDX-License-Identifier: AGPL-3.0-only

//! The command line of `acib` (board #479, ACIB-0):
//!
//! ```text
//! acib devices                                   [--timeout-ms <ms>]
//! acib info     --device <n>                     [--timeout-ms <ms>]
//! acib services --device <n>                     [--timeout-ms <ms>]
//! acib images   --device <n>                     [--timeout-ms <ms>]
//! acib apps     --device <n> --contains <text>   [--timeout-ms <ms>]
//! ```
//!
//! `<n>` is the device's usbmuxd session number, as `acib devices` lists it. `<text>` is what the
//! bundle identifiers asked for hold, at least three characters. Anything else is refused with
//! `command_line_invalid`.

use crate::answer::CommandName;

/// How long a command may take when `--timeout-ms` does not say.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
/// The fewest characters `--contains` takes: `apps` answers what was asked for, never the
/// whole list.
pub const SHORTEST_CONTAINS: usize = 3;

/// What a command line asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Devices,
    Info(u32),
    Services(u32),
    Images(u32),
    Apps { device: u32, contains: String },
}

impl Request {
    /// The command's name.
    pub fn name(&self) -> CommandName {
        match self {
            Self::Devices => CommandName::Devices,
            Self::Info(_) => CommandName::Info,
            Self::Services(_) => CommandName::Services,
            Self::Images(_) => CommandName::Images,
            Self::Apps { .. } => CommandName::Apps,
        }
    }

    /// The device number the command names, if it names one.
    pub fn device(&self) -> Option<u32> {
        match self {
            Self::Devices => None,
            Self::Info(device)
            | Self::Services(device)
            | Self::Images(device)
            | Self::Apps { device, .. } => Some(*device),
        }
    }
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub request: Request,
    /// The time the whole command may take, in milliseconds (at least 1).
    pub timeout_ms: u64,
}

/// The command line after the program's name; `None` when it is not one of the five commands
/// with their options (an option given twice, an unknown option, a number that is not one, a
/// text too short).
pub fn parse(arguments: &[String]) -> Option<Invocation> {
    let (command, options) = arguments.split_first()?;
    let mut device = None;
    let mut timeout_ms = None;
    let mut contains = None;
    for pair in options.chunks(2) {
        let [option, value] = pair else {
            return None;
        };
        match option.as_str() {
            "--device" if device.is_none() => device = Some(number::<u32>(value)?),
            "--timeout-ms" if timeout_ms.is_none() => {
                timeout_ms = Some(number::<u64>(value).filter(|ms| *ms > 0)?);
            }
            "--contains" if contains.is_none() => contains = Some(text(value)?),
            _ => return None,
        }
    }
    let request = match (command.as_str(), device, contains) {
        ("devices", None, None) => Request::Devices,
        ("info", Some(device), None) => Request::Info(device),
        ("services", Some(device), None) => Request::Services(device),
        ("images", Some(device), None) => Request::Images(device),
        ("apps", Some(device), Some(contains)) => Request::Apps { device, contains },
        _ => return None,
    };
    Some(Invocation {
        request,
        timeout_ms: timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS),
    })
}

/// A decimal number of ASCII digits only (no sign, no space).
fn number<T: std::str::FromStr>(text: &str) -> Option<T> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// The text of `--contains`, when it has at least [`SHORTEST_CONTAINS`] characters.
fn text(value: &str) -> Option<String> {
    if value.chars().count() >= SHORTEST_CONTAINS {
        Some(value.to_owned())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(line: &[&str]) -> Option<Invocation> {
        let arguments: Vec<String> = line.iter().map(|argument| (*argument).to_owned()).collect();
        parse(&arguments)
    }

    #[test]
    fn the_four_commands_parse_with_their_options() {
        assert_eq!(
            parsed(&["devices"]),
            Some(Invocation {
                request: Request::Devices,
                timeout_ms: DEFAULT_TIMEOUT_MS,
            })
        );
        assert_eq!(
            parsed(&["info", "--device", "3"]),
            Some(Invocation {
                request: Request::Info(3),
                timeout_ms: DEFAULT_TIMEOUT_MS,
            })
        );
        assert_eq!(
            parsed(&["services", "--timeout-ms", "45000", "--device", "12"]),
            Some(Invocation {
                request: Request::Services(12),
                timeout_ms: 45_000,
            })
        );
        assert_eq!(
            parsed(&["images", "--device", "0", "--timeout-ms", "1"]),
            Some(Invocation {
                request: Request::Images(0),
                timeout_ms: 1,
            })
        );
        assert_eq!(
            parsed(&["devices", "--timeout-ms", "500"]).map(|invocation| invocation.timeout_ms),
            Some(500)
        );
    }

    #[test]
    fn every_other_command_line_is_refused() {
        let lines: &[&[&str]] = &[
            &[],
            &["help"],
            &["DEVICES"],
            &["devices", "--device", "3"],
            &["info"],
            &["info", "--device"],
            &["info", "--device", ""],
            &["info", "--device", "-1"],
            &["info", "--device", "+3"],
            &["info", "--device", "0x3"],
            &["info", "--device", " 3"],
            &["info", "--device", "4294967296"],
            &["info", "--device", "3", "--device", "3"],
            &["info", "--device", "3", "--timeout-ms", "0"],
            &[
                "info",
                "--device",
                "3",
                "--timeout-ms",
                "1",
                "--timeout-ms",
                "1",
            ],
            &["info", "--device", "3", "--verbose"],
            &["info", "--device=3"],
            &["services", "3"],
            &["images", "--device", "3", "extra"],
        ];
        for line in lines {
            assert_eq!(parsed(line), None, "{line:?}");
        }
    }

    /// `apps` needs `--device` and `--contains` of at least three characters (counted as
    /// characters, not bytes); no other command takes `--contains`.
    #[test]
    fn the_apps_command_takes_a_text_of_three_characters_or_more() {
        assert_eq!(
            parsed(&["apps", "--device", "3", "--contains", "game"]),
            Some(Invocation {
                request: Request::Apps {
                    device: 3,
                    contains: "game".to_owned(),
                },
                timeout_ms: DEFAULT_TIMEOUT_MS,
            })
        );
        assert_eq!(
            parsed(&[
                "apps",
                "--contains",
                "Abc",
                "--timeout-ms",
                "9000",
                "--device",
                "4"
            ]),
            Some(Invocation {
                request: Request::Apps {
                    device: 4,
                    contains: "Abc".to_owned(),
                },
                timeout_ms: 9_000,
            })
        );
        assert_eq!(
            parsed(&["apps", "--device", "3", "--contains", "äöü"])
                .map(|invocation| invocation.request),
            Some(Request::Apps {
                device: 3,
                contains: "äöü".to_owned(),
            })
        );
        let refused: &[&[&str]] = &[
            &["apps"],
            &["apps", "--device", "3"],
            &["apps", "--contains", "game"],
            &["apps", "--device", "3", "--contains"],
            &["apps", "--device", "3", "--contains", ""],
            &["apps", "--device", "3", "--contains", "ga"],
            &["apps", "--device", "3", "--contains", "äö"],
            &[
                "apps",
                "--device",
                "3",
                "--contains",
                "game",
                "--contains",
                "game",
            ],
            &["devices", "--contains", "game"],
            &["info", "--device", "3", "--contains", "game"],
            &["services", "--device", "3", "--contains", "game"],
            &["images", "--device", "3", "--contains", "game"],
        ];
        for line in refused {
            assert_eq!(parsed(line), None, "{line:?}");
        }
    }

    #[test]
    fn a_request_names_its_command_and_its_device() {
        assert_eq!(Request::Devices.name(), CommandName::Devices);
        assert_eq!(Request::Devices.device(), None);
        assert_eq!(Request::Info(3).name(), CommandName::Info);
        assert_eq!(Request::Services(4).device(), Some(4));
        assert_eq!(Request::Images(5).name(), CommandName::Images);
        assert_eq!(Request::Images(5).device(), Some(5));
    }
}
