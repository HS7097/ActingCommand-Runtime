// SPDX-License-Identifier: AGPL-3.0-only

//! The command line of `acib` (board #479, ACIB-0 to ACIB-3):
//!
//! ```text
//! acib devices     [--key-salt-env <NAME> [--binding-salt-env <NAME2>]] [--timeout-ms <ms>]
//! acib info        <device>                                [--timeout-ms <ms>]
//! acib services    <device>                                [--timeout-ms <ms>]
//! acib images      <device>                                [--timeout-ms <ms>]
//! acib apps        <device> --contains <text> [--max-matching <m>] [--timeout-ms <ms>]
//! acib screenshot  <device> --out <file|-> [--via <way>]   [--timeout-ms <ms>]
//! acib orientation <device> [--via <reader>]               [--timeout-ms <ms>]
//! acib running     <device> --bundle <id>                  [--timeout-ms <ms>]
//! acib launch      <device> --bundle <id> [--relaunch]     [--timeout-ms <ms>]
//! acib stop        <device> --bundle <id>                  [--timeout-ms <ms>]
//! ```
//!
//! `<device>` is `--device <n>`, the device's usbmuxd session number as `acib devices` lists
//! it, or, in its place, `--device-key <key> --key-salt-env <NAME>`: the listed device whose
//! key under the salt of the environment variable `NAME` is `<key>` (`sha256:` and 64 lowercase
//! hexadecimal digits). `devices` takes `--key-salt-env` alone to add each row's key, and
//! `--binding-salt-env` only beside it. `<text>` is what the bundle identifiers asked for hold,
//! at least three characters. `<m>` is the most identifiers `apps` names (1 to
//! [`MAX_MATCHING_LIMIT`]). `<file>` is where the picture is written, a path that must not exist
//! yet, and `-` the standard output ([`Out`]). `<way>` is one of the words of [`Way`] (default
//! [`DEFAULT_WAY`]), `<reader>` one of [`Reader`]'s (default: each in turn). `<id>` is one
//! bundle identifier, written in the characters Apple allows in one (ASCII letters and digits,
//! `-` and `.`). `--relaunch` is a flag without a value, taken by `launch` alone. Anything else
//! is refused with `command_line_invalid`: an option given twice, `--device` beside
//! `--device-key`, either of `--device-key` and `--key-salt-env` without the other, a key or a
//! variable name that cannot be one, a bundle identifier that cannot be one, an option the
//! command does not take.

use crate::answer::{CommandName, Reader, Way};
use crate::key;
use crate::redact;

/// How long a command may take when `--timeout-ms` does not say.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
/// The fewest characters `--contains` takes: `apps` answers what was asked for, never the
/// whole list.
pub const SHORTEST_CONTAINS: usize = 3;
/// The most bundle identifiers `apps` names when `--max-matching` does not say: more matching
/// ones answer `too_many_matching` with the counts alone.
pub const DEFAULT_MAX_MATCHING: usize = 5;
/// The largest bound `--max-matching` takes: a larger one is refused with
/// `command_line_invalid`, so no call names more identifiers than this.
pub const MAX_MATCHING_LIMIT: usize = 20;
/// The way `screenshot` takes its picture when `--via` does not say: the screen capture
/// service, which answers one request with a picture in the format asked for.
pub const DEFAULT_WAY: Way = Way::ScreenCapture;
/// The `--out` that names the standard output in place of a file.
pub const STANDARD_OUTPUT: &str = "-";
/// The flag of `launch` that ends the application first; it takes no value.
const RELAUNCH: &str = "--relaunch";

/// The device a command asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// `--device <n>`: the usbmuxd session number `acib devices` lists.
    Number(u32),
    /// `--device-key <key> --key-salt-env <NAME>`: the listed device whose key under the salt of
    /// the environment variable `salt_variable` is `key`.
    Key { key: String, salt_variable: String },
}

/// The environment variables `devices` reads the salts of its keys from: `device_key` under the
/// first, `binding_key` under the second when it is given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyVariables {
    pub key: String,
    pub binding: Option<String>,
}

/// Where `screenshot` puts the picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    /// A new file at this path.
    File(String),
    /// The standard output, after the line (`--out -`, which the line names as `"out": "-"`):
    /// no file is written.
    Stdout,
}

/// What a command line asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Devices(Option<KeyVariables>),
    Info(Target),
    Services(Target),
    Images(Target),
    Apps {
        device: Target,
        contains: String,
        max_matching: usize,
    },
    Screenshot {
        device: Target,
        out: Out,
        via: Way,
    },
    /// `orientation`: the reader `--via` names, or each reader in turn.
    Orientation {
        device: Target,
        via: Option<Reader>,
    },
    /// `running`: whether the bundle is installed and runs, and its processes' pids.
    Running {
        device: Target,
        bundle: String,
    },
    /// `launch`: the bundle launched, ended first when `relaunch` says so.
    Launch {
        device: Target,
        bundle: String,
        relaunch: bool,
    },
    /// `stop`: the application's own processes ended (not its extensions').
    Stop {
        device: Target,
        bundle: String,
    },
}

impl Request {
    /// The command's name.
    pub fn name(&self) -> CommandName {
        match self {
            Self::Devices(_) => CommandName::Devices,
            Self::Info(_) => CommandName::Info,
            Self::Services(_) => CommandName::Services,
            Self::Images(_) => CommandName::Images,
            Self::Apps { .. } => CommandName::Apps,
            Self::Screenshot { .. } => CommandName::Screenshot,
            Self::Orientation { .. } => CommandName::Orientation,
            Self::Running { .. } => CommandName::Running,
            Self::Launch { .. } => CommandName::Launch,
            Self::Stop { .. } => CommandName::Stop,
        }
    }

    /// The device number the command line names, if it names one (`--device`; a device named
    /// by its key has its number only once it is found).
    pub fn device(&self) -> Option<u32> {
        match self.target()? {
            Target::Number(number) => Some(*number),
            Target::Key { .. } => None,
        }
    }

    /// The device the command asks, if it asks one.
    pub fn target(&self) -> Option<&Target> {
        match self {
            Self::Devices(_) => None,
            Self::Info(device)
            | Self::Services(device)
            | Self::Images(device)
            | Self::Apps { device, .. }
            | Self::Screenshot { device, .. }
            | Self::Orientation { device, .. }
            | Self::Running { device, .. }
            | Self::Launch { device, .. }
            | Self::Stop { device, .. } => Some(device),
        }
    }

    /// The way the command takes a picture, if it takes one.
    pub fn via(&self) -> Option<Way> {
        match self {
            Self::Screenshot { via, .. } => Some(*via),
            Self::Devices(_)
            | Self::Info(_)
            | Self::Services(_)
            | Self::Images(_)
            | Self::Apps { .. }
            | Self::Orientation { .. }
            | Self::Running { .. }
            | Self::Launch { .. }
            | Self::Stop { .. } => None,
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

/// The options of a command line, each given once at most.
#[derive(Default)]
struct Options {
    device: Option<u32>,
    device_key: Option<String>,
    key_salt: Option<String>,
    binding_salt: Option<String>,
    timeout_ms: Option<u64>,
    contains: Option<String>,
    max_matching: Option<usize>,
    out: Option<Out>,
    via: Option<String>,
    bundle: Option<String>,
    relaunch: bool,
}

/// The command line after the program's name; `None` when it is not one of the ten commands
/// with their options (an option given twice, an unknown option, a number that is not one, a
/// text too short, a bound of 0 or above [`MAX_MATCHING_LIMIT`], an empty `--out`, a way or a
/// reader that is not one of its command's words, a key that is not written as a key, a
/// variable name that is empty or holds `=` or NUL, a bundle identifier that is not written as
/// one, `--device` beside `--device-key`, one of `--device-key` and `--key-salt-env` without
/// the other, `--binding-salt-env` without `--key-salt-env` or on another command than
/// `devices`, `--relaunch` on another command than `launch`). Every option but `--relaunch`
/// takes the argument after it as its value, whatever it is.
pub fn parse(arguments: &[String]) -> Option<Invocation> {
    let (command, given) = arguments.split_first()?;
    let mut options = Options::default();
    let mut rest = given.iter();
    while let Some(option) = rest.next() {
        if option.as_str() == RELAUNCH {
            if options.relaunch {
                return None;
            }
            options.relaunch = true;
            continue;
        }
        let value = rest.next()?;
        match option.as_str() {
            "--device" if options.device.is_none() => {
                options.device = Some(number::<u32>(value)?);
            }
            "--device-key" if options.device_key.is_none() => {
                options.device_key = Some(key_text(value)?);
            }
            "--key-salt-env" if options.key_salt.is_none() => {
                options.key_salt = Some(variable(value)?);
            }
            "--binding-salt-env" if options.binding_salt.is_none() => {
                options.binding_salt = Some(variable(value)?);
            }
            "--timeout-ms" if options.timeout_ms.is_none() => {
                options.timeout_ms = Some(number::<u64>(value).filter(|ms| *ms > 0)?);
            }
            "--contains" if options.contains.is_none() => options.contains = Some(text(value)?),
            "--max-matching" if options.max_matching.is_none() => {
                options.max_matching = Some(
                    number::<usize>(value)
                        .filter(|most| (1..=MAX_MATCHING_LIMIT).contains(most))?,
                );
            }
            "--out" if options.out.is_none() => options.out = Some(out(value)?),
            "--via" if options.via.is_none() => options.via = Some(value.clone()),
            "--bundle" if options.bundle.is_none() => options.bundle = Some(bundle(value)?),
            _ => return None,
        }
    }
    let timeout_ms = options.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
    let request = match command.as_str() {
        "devices" => devices(options)?,
        _ => on_a_device(command, options)?,
    };
    Some(Invocation {
        request,
        timeout_ms,
    })
}

/// `devices`, which takes no option but the two salts' variables, the second only beside the
/// first.
fn devices(options: Options) -> Option<Request> {
    let Options {
        device: None,
        device_key: None,
        key_salt,
        binding_salt,
        timeout_ms: _,
        contains: None,
        max_matching: None,
        out: None,
        via: None,
        bundle: None,
        relaunch: false,
    } = options
    else {
        return None;
    };
    match (key_salt, binding_salt) {
        (None, None) => Some(Request::Devices(None)),
        (Some(key), binding) => Some(Request::Devices(Some(KeyVariables { key, binding }))),
        (None, Some(_)) => None,
    }
}

/// A command on one device: `--device`, or `--device-key` with `--key-salt-env`, and the
/// command's own options.
fn on_a_device(command: &str, options: Options) -> Option<Request> {
    let Options {
        device,
        device_key,
        key_salt,
        binding_salt: None,
        timeout_ms: _,
        contains,
        max_matching,
        out,
        via,
        bundle,
        relaunch,
    } = options
    else {
        return None;
    };
    let target = match (device, device_key, key_salt) {
        (Some(number), None, None) => Target::Number(number),
        (None, Some(key), Some(salt_variable)) => Target::Key { key, salt_variable },
        _ => return None,
    };
    let request = match (command, contains, max_matching, out, via, bundle, relaunch) {
        ("info", None, None, None, None, None, false) => Request::Info(target),
        ("services", None, None, None, None, None, false) => Request::Services(target),
        ("images", None, None, None, None, None, false) => Request::Images(target),
        ("apps", Some(contains), max_matching, None, None, None, false) => Request::Apps {
            device: target,
            contains,
            max_matching: max_matching.unwrap_or(DEFAULT_MAX_MATCHING),
        },
        ("screenshot", None, None, Some(out), via, None, false) => Request::Screenshot {
            device: target,
            out,
            via: match via {
                None => DEFAULT_WAY,
                Some(word) => Way::from_word(&word)?,
            },
        },
        ("orientation", None, None, None, via, None, false) => Request::Orientation {
            device: target,
            via: match via {
                None => None,
                Some(word) => Some(Reader::from_word(&word)?),
            },
        },
        ("running", None, None, None, None, Some(bundle), false) => Request::Running {
            device: target,
            bundle,
        },
        ("launch", None, None, None, None, Some(bundle), relaunch) => Request::Launch {
            device: target,
            bundle,
            relaunch,
        },
        ("stop", None, None, None, None, Some(bundle), false) => Request::Stop {
            device: target,
            bundle,
        },
        _ => return None,
    };
    Some(request)
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

/// Where `--out` puts the picture: the standard output for `-`, else the file at the path, when
/// it is not empty (whether it exists is the command's to check; a file named `-` is `./-`).
fn out(value: &str) -> Option<Out> {
    match value {
        "" => None,
        STANDARD_OUTPUT => Some(Out::Stdout),
        path => Some(Out::File(path.to_owned())),
    }
}

/// The bundle identifier of `--bundle`, when it is written as one is.
fn bundle(value: &str) -> Option<String> {
    redact::is_bundle_identifier(value).then(|| value.to_owned())
}

/// The key of `--device-key`, when it is written as a key is.
fn key_text(value: &str) -> Option<String> {
    key::is_key_text(value).then(|| value.to_owned())
}

/// The name of an environment variable a salt is read from, when a variable can have it: not
/// empty, without `=` and NUL. Whether it is set is the command's to read.
fn variable(value: &str) -> Option<String> {
    if value.is_empty() || value.contains(['=', '\0']) {
        None
    } else {
        Some(value.to_owned())
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
                request: Request::Devices(None),
                timeout_ms: DEFAULT_TIMEOUT_MS,
            })
        );
        assert_eq!(
            parsed(&["info", "--device", "3"]),
            Some(Invocation {
                request: Request::Info(Target::Number(3)),
                timeout_ms: DEFAULT_TIMEOUT_MS,
            })
        );
        assert_eq!(
            parsed(&["services", "--timeout-ms", "45000", "--device", "12"]),
            Some(Invocation {
                request: Request::Services(Target::Number(12)),
                timeout_ms: 45_000,
            })
        );
        assert_eq!(
            parsed(&["images", "--device", "0", "--timeout-ms", "1"]),
            Some(Invocation {
                request: Request::Images(Target::Number(0)),
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
    /// characters, not bytes) and takes `--max-matching` of 1 to [`MAX_MATCHING_LIMIT`] (default
    /// [`DEFAULT_MAX_MATCHING`]); no other command takes `--contains` or `--max-matching`.
    #[test]
    fn the_apps_command_takes_a_text_of_three_characters_or_more() {
        assert_eq!(
            parsed(&["apps", "--device", "3", "--contains", "game"]),
            Some(Invocation {
                request: Request::Apps {
                    device: Target::Number(3),
                    contains: "game".to_owned(),
                    max_matching: DEFAULT_MAX_MATCHING,
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
                    device: Target::Number(4),
                    contains: "Abc".to_owned(),
                    max_matching: DEFAULT_MAX_MATCHING,
                },
                timeout_ms: 9_000,
            })
        );
        assert_eq!(
            parsed(&["apps", "--device", "3", "--contains", "äöü"])
                .map(|invocation| invocation.request),
            Some(Request::Apps {
                device: Target::Number(3),
                contains: "äöü".to_owned(),
                max_matching: DEFAULT_MAX_MATCHING,
            })
        );
        for (bound, most) in [("1", 1), ("12", 12)] {
            assert_eq!(
                parsed(&[
                    "apps",
                    "--max-matching",
                    bound,
                    "--device",
                    "3",
                    "--contains",
                    "game"
                ])
                .map(|invocation| invocation.request),
                Some(Request::Apps {
                    device: Target::Number(3),
                    contains: "game".to_owned(),
                    max_matching: most,
                }),
                "--max-matching {bound}"
            );
        }
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
            &[
                "apps",
                "--device",
                "3",
                "--contains",
                "game",
                "--max-matching",
                "0",
            ],
            &[
                "apps",
                "--device",
                "3",
                "--contains",
                "game",
                "--max-matching",
                "-1",
            ],
            &[
                "apps",
                "--device",
                "3",
                "--contains",
                "game",
                "--max-matching",
                "2",
                "--max-matching",
                "2",
            ],
            &["apps", "--device", "3", "--max-matching", "2"],
            &["images", "--device", "3", "--max-matching", "2"],
            &["devices", "--max-matching", "2"],
        ];
        for line in refused {
            assert_eq!(parsed(line), None, "{line:?}");
        }
    }

    /// `--max-matching` takes at most 20: exactly the limit is taken, one more (or any larger
    /// bound) is refused as a command line (`command_line_invalid`).
    #[test]
    fn max_matching_takes_its_limit_of_twenty_and_refuses_one_more() {
        let line = |bound| {
            parsed(&[
                "apps",
                "--device",
                "3",
                "--contains",
                "game",
                "--max-matching",
                bound,
            ])
        };
        assert_eq!(
            line("20").map(|invocation| invocation.request),
            Some(Request::Apps {
                device: Target::Number(3),
                contains: "game".to_owned(),
                max_matching: 20,
            })
        );
        for bound in ["21", "10000", "18446744073709551616"] {
            assert_eq!(line(bound), None, "--max-matching {bound}");
        }
    }

    #[test]
    fn a_request_names_its_command_and_its_device() {
        assert_eq!(Request::Devices(None).name(), CommandName::Devices);
        assert_eq!(Request::Devices(None).device(), None);
        assert_eq!(Request::Info(Target::Number(3)).name(), CommandName::Info);
        assert_eq!(Request::Services(Target::Number(4)).device(), Some(4));
        assert_eq!(
            Request::Images(Target::Number(5)).name(),
            CommandName::Images
        );
        assert_eq!(Request::Images(Target::Number(5)).device(), Some(5));
        assert_eq!(Request::Info(key_target()).device(), None);
        let orientation = Request::Orientation {
            device: Target::Number(7),
            via: None,
        };
        assert_eq!(orientation.name(), CommandName::Orientation);
        assert_eq!(orientation.device(), Some(7));
        assert_eq!(orientation.via(), None);
    }

    /// `screenshot` needs `--device` and a non-empty `--out`, takes `--via` with a word of
    /// [`Way`] (default [`DEFAULT_WAY`]) and `--timeout-ms`, in any order, and names its
    /// command, its device and its way.
    #[test]
    fn the_screenshot_command_takes_a_device_a_file_and_a_way() {
        assert_eq!(
            parsed(&["screenshot", "--device", "3", "--out", "screen.png"]),
            Some(Invocation {
                request: Request::Screenshot {
                    device: Target::Number(3),
                    out: Out::File("screen.png".to_owned()),
                    via: Way::ScreenCapture,
                },
                timeout_ms: DEFAULT_TIMEOUT_MS,
            })
        );
        for (word, way) in [
            ("screen_capture", Way::ScreenCapture),
            ("instruments", Way::Instruments),
        ] {
            assert_eq!(
                parsed(&[
                    "screenshot",
                    "--via",
                    word,
                    "--out",
                    "shots/one.png",
                    "--timeout-ms",
                    "60000",
                    "--device",
                    "4",
                ]),
                Some(Invocation {
                    request: Request::Screenshot {
                        device: Target::Number(4),
                        out: Out::File("shots/one.png".to_owned()),
                        via: way,
                    },
                    timeout_ms: 60_000,
                }),
                "--via {word}"
            );
        }
        let request = Request::Screenshot {
            device: Target::Number(6),
            out: Out::File("screen.png".to_owned()),
            via: Way::Instruments,
        };
        assert_eq!(request.name(), CommandName::Screenshot);
        assert_eq!(request.device(), Some(6));
        assert_eq!(request.via(), Some(Way::Instruments));
        assert_eq!(Request::Info(Target::Number(3)).via(), None);
        assert_eq!(Request::Devices(None).via(), None);
    }

    /// Every other `screenshot` line is refused, no other command takes `--out` or `--via`, and
    /// there is no command for the application in front (the library offers no way to ask).
    #[test]
    fn every_other_screenshot_line_is_refused() {
        let lines: &[&[&str]] = &[
            &["screenshot"],
            &["screenshot", "--device", "3"],
            &["screenshot", "--out", "screen.png"],
            &["screenshot", "--device", "3", "--out"],
            &["screenshot", "--device", "3", "--out", ""],
            &[
                "screenshot",
                "--device",
                "3",
                "--out",
                "a.png",
                "--out",
                "b.png",
            ],
            &[
                "screenshot",
                "--device",
                "3",
                "--out",
                "a.png",
                "--via",
                "instruments",
                "--via",
                "instruments",
            ],
            &[
                "screenshot",
                "--device",
                "3",
                "--out",
                "a.png",
                "--via",
                "SCREEN_CAPTURE",
            ],
            &[
                "screenshot",
                "--device",
                "3",
                "--out",
                "a.png",
                "--via",
                "screenshotr",
            ],
            &["screenshot", "--device", "3", "--out", "a.png", "--via", ""],
            &[
                "screenshot",
                "--device",
                "3",
                "--out",
                "a.png",
                "--contains",
                "game",
            ],
            &[
                "screenshot",
                "--device",
                "3",
                "--out",
                "a.png",
                "--max-matching",
                "2",
            ],
            &["devices", "--out", "a.png"],
            &["info", "--device", "3", "--out", "a.png"],
            &["images", "--device", "3", "--via", "instruments"],
            &[
                "apps",
                "--device",
                "3",
                "--contains",
                "game",
                "--out",
                "a.png",
            ],
            &["foreground", "--device", "3", "--is", "com.example.app"],
        ];
        for line in lines {
            assert_eq!(parsed(line), None, "{line:?}");
        }
    }

    use crate::key::tests::VECTOR_KEY;

    /// The variable name the tests give `--key-salt-env`.
    const SALT_VARIABLE: &str = "ACIB_TEST_KEY_SALT";

    /// The device named by [`VECTOR_KEY`] under the salt of [`SALT_VARIABLE`].
    fn key_target() -> Target {
        Target::Key {
            key: VECTOR_KEY.to_owned(),
            salt_variable: SALT_VARIABLE.to_owned(),
        }
    }

    /// The request of `line`, when it parses.
    fn request(line: &[&str]) -> Option<Request> {
        parsed(line).map(|invocation| invocation.request)
    }

    /// Each command on a device takes `--device-key <key> --key-salt-env <NAME>` in place of
    /// `--device <n>`, the two in any order and among the command's other options.
    #[test]
    fn a_device_key_with_its_salt_variable_names_the_device_in_place_of_its_number() {
        let by_key = ["--device-key", VECTOR_KEY, "--key-salt-env", SALT_VARIABLE];
        let cases: Vec<(Vec<&str>, Request)> = vec![
            (vec!["info"], Request::Info(key_target())),
            (vec!["services"], Request::Services(key_target())),
            (vec!["images"], Request::Images(key_target())),
            (
                vec!["apps", "--contains", "game"],
                Request::Apps {
                    device: key_target(),
                    contains: "game".to_owned(),
                    max_matching: DEFAULT_MAX_MATCHING,
                },
            ),
            (
                vec!["screenshot", "--out", "screen.png"],
                Request::Screenshot {
                    device: key_target(),
                    out: Out::File("screen.png".to_owned()),
                    via: DEFAULT_WAY,
                },
            ),
            (
                vec!["orientation"],
                Request::Orientation {
                    device: key_target(),
                    via: None,
                },
            ),
        ];
        for (line, expected) in cases {
            let mut first = line.clone();
            first.extend(by_key);
            assert_eq!(request(&first), Some(expected.clone()), "{first:?}");
            let mut last = vec![line[0], "--key-salt-env", SALT_VARIABLE];
            last.extend(&line[1..]);
            last.extend(["--device-key", VECTOR_KEY]);
            assert_eq!(request(&last), Some(expected), "{last:?}");
        }
    }

    /// `--device` beside `--device-key` is refused, and so is either of `--device-key` and
    /// `--key-salt-env` without the other, a key not written as one, a variable name a variable
    /// cannot have, a flag given twice, `--binding-salt-env` on a command on a device, and a
    /// command on a device that names none.
    #[test]
    fn every_refused_pair_of_the_device_flags() {
        let digits = &VECTOR_KEY["sha256:".len()..];
        let upper = format!("sha256:{}", digits.to_uppercase());
        let short = format!("sha256:{}", &digits[1..]);
        let lines: Vec<Vec<&str>> = vec![
            vec![
                "info",
                "--device",
                "3",
                "--device-key",
                VECTOR_KEY,
                "--key-salt-env",
                SALT_VARIABLE,
            ],
            vec!["info", "--device", "3", "--device-key", VECTOR_KEY],
            vec!["info", "--device", "3", "--key-salt-env", SALT_VARIABLE],
            vec!["info", "--device-key", VECTOR_KEY],
            vec!["info", "--key-salt-env", SALT_VARIABLE],
            vec!["screenshot", "--device-key", VECTOR_KEY, "--out", "a.png"],
            vec!["orientation", "--key-salt-env", SALT_VARIABLE],
            vec![
                "info",
                "--device-key",
                upper.as_str(),
                "--key-salt-env",
                SALT_VARIABLE,
            ],
            vec![
                "info",
                "--device-key",
                short.as_str(),
                "--key-salt-env",
                SALT_VARIABLE,
            ],
            vec![
                "info",
                "--device-key",
                digits,
                "--key-salt-env",
                SALT_VARIABLE,
            ],
            vec!["info", "--device-key", "", "--key-salt-env", SALT_VARIABLE],
            vec!["info", "--device-key", VECTOR_KEY, "--key-salt-env", ""],
            vec!["info", "--device-key", VECTOR_KEY, "--key-salt-env", "A=B"],
            vec!["info", "--device-key", VECTOR_KEY, "--key-salt-env", "A\0B"],
            vec![
                "info",
                "--device-key",
                VECTOR_KEY,
                "--device-key",
                VECTOR_KEY,
                "--key-salt-env",
                SALT_VARIABLE,
            ],
            vec![
                "info",
                "--device-key",
                VECTOR_KEY,
                "--key-salt-env",
                SALT_VARIABLE,
                "--key-salt-env",
                SALT_VARIABLE,
            ],
            vec![
                "info",
                "--device-key",
                VECTOR_KEY,
                "--key-salt-env",
                SALT_VARIABLE,
                "--binding-salt-env",
                "ACIB_BINDING",
            ],
            vec![
                "info",
                "--device",
                "3",
                "--binding-salt-env",
                "ACIB_BINDING",
            ],
            vec!["orientation"],
        ];
        for line in lines {
            assert_eq!(parsed(&line), None, "{line:?}");
        }
    }

    /// `devices` takes `--key-salt-env` for each row's `device_key`, and `--binding-salt-env`
    /// for its `binding_key` only beside it; it takes neither `--device` nor `--device-key`.
    #[test]
    fn devices_takes_a_key_salt_variable_and_a_binding_salt_variable_beside_it() {
        assert_eq!(
            request(&["devices", "--key-salt-env", SALT_VARIABLE]),
            Some(Request::Devices(Some(KeyVariables {
                key: SALT_VARIABLE.to_owned(),
                binding: None,
            })))
        );
        assert_eq!(
            request(&[
                "devices",
                "--binding-salt-env",
                "ACIB_BINDING",
                "--timeout-ms",
                "900",
                "--key-salt-env",
                SALT_VARIABLE,
            ]),
            Some(Request::Devices(Some(KeyVariables {
                key: SALT_VARIABLE.to_owned(),
                binding: Some("ACIB_BINDING".to_owned()),
            })))
        );
        let refused: &[&[&str]] = &[
            &["devices", "--binding-salt-env", "ACIB_BINDING"],
            &["devices", "--key-salt-env", ""],
            &["devices", "--key-salt-env", "A=B"],
            &[
                "devices",
                "--key-salt-env",
                SALT_VARIABLE,
                "--binding-salt-env",
                "",
            ],
            &[
                "devices",
                "--key-salt-env",
                SALT_VARIABLE,
                "--key-salt-env",
                SALT_VARIABLE,
            ],
            &[
                "devices",
                "--key-salt-env",
                SALT_VARIABLE,
                "--binding-salt-env",
                "ACIB_BINDING",
                "--binding-salt-env",
                "ACIB_BINDING",
            ],
            &["devices", "--device-key", VECTOR_KEY],
            &[
                "devices",
                "--device-key",
                VECTOR_KEY,
                "--key-salt-env",
                SALT_VARIABLE,
            ],
            &[
                "devices",
                "--key-salt-env",
                SALT_VARIABLE,
                "--via",
                "springboard",
            ],
        ];
        for line in refused {
            assert_eq!(parsed(line), None, "{line:?}");
        }
    }

    /// `--out -` is the standard output; any other non-empty `--out` a file, `./-` the file
    /// named `-`.
    #[test]
    fn out_dash_is_the_standard_output() {
        let out_of = |value| match request(&["screenshot", "--device", "3", "--out", value]) {
            Some(Request::Screenshot { out, .. }) => Some(out),
            _ => None,
        };
        assert_eq!(out_of("-"), Some(Out::Stdout));
        assert_eq!(out_of("./-"), Some(Out::File("./-".to_owned())));
        assert_eq!(out_of("--"), Some(Out::File("--".to_owned())));
        assert_eq!(out_of(""), None);
        assert_eq!(
            request(&[
                "screenshot",
                "--out",
                "-",
                "--via",
                "instruments",
                "--device-key",
                VECTOR_KEY,
                "--key-salt-env",
                SALT_VARIABLE,
            ]),
            Some(Request::Screenshot {
                device: key_target(),
                out: Out::Stdout,
                via: Way::Instruments,
            })
        );
        assert_eq!(
            parsed(&["orientation", "--device", "3", "--out", "-"]),
            None
        );
    }

    /// `orientation` takes a device, `--via` with a word of [`Reader`] (without it: each reader
    /// in turn) and `--timeout-ms`; `--via` takes only its own command's words.
    #[test]
    fn the_orientation_command_takes_a_device_and_a_reader() {
        assert_eq!(
            parsed(&["orientation", "--device", "3"]),
            Some(Invocation {
                request: Request::Orientation {
                    device: Target::Number(3),
                    via: None,
                },
                timeout_ms: DEFAULT_TIMEOUT_MS,
            })
        );
        for (word, reader) in [
            ("springboard", Reader::Springboard),
            ("springboard_shim", Reader::SpringboardShim),
        ] {
            assert_eq!(
                parsed(&[
                    "orientation",
                    "--timeout-ms",
                    "8000",
                    "--via",
                    word,
                    "--device",
                    "5",
                ]),
                Some(Invocation {
                    request: Request::Orientation {
                        device: Target::Number(5),
                        via: Some(reader),
                    },
                    timeout_ms: 8_000,
                }),
                "--via {word}"
            );
        }
        let refused: &[&[&str]] = &[
            &["orientation", "--device", "3", "--via", ""],
            &["orientation", "--device", "3", "--via", "screen_capture"],
            &["orientation", "--device", "3", "--via", "SPRINGBOARD"],
            &["orientation", "--device", "3", "--via", "devicecontrol"],
            &[
                "orientation",
                "--device",
                "3",
                "--via",
                "springboard",
                "--via",
                "springboard",
            ],
            &["orientation", "--device", "3", "--contains", "game"],
            &["orientation", "--device", "3", "--max-matching", "2"],
            &["orientation", "--device", "3", "--out", "a.png"],
            &[
                "screenshot",
                "--device",
                "3",
                "--out",
                "a.png",
                "--via",
                "springboard",
            ],
            &["info", "--device", "3", "--via", "springboard"],
        ];
        for line in refused {
            assert_eq!(parsed(line), None, "{line:?}");
        }
    }

    /// The bundle the application commands' tests ask for.
    const BUNDLE: &str = "com.example.app";

    /// `running`, `launch` and `stop` take a device (by its number or by its key) and
    /// `--bundle`, in any order, with `--timeout-ms`; `launch` takes `--relaunch`, a flag
    /// without a value, anywhere among its options; each names its command and its device.
    #[test]
    fn the_application_commands_take_a_device_and_a_bundle() {
        assert_eq!(
            parsed(&["running", "--device", "3", "--bundle", BUNDLE]),
            Some(Invocation {
                request: Request::Running {
                    device: Target::Number(3),
                    bundle: BUNDLE.to_owned(),
                },
                timeout_ms: DEFAULT_TIMEOUT_MS,
            })
        );
        assert_eq!(
            parsed(&[
                "stop",
                "--bundle",
                BUNDLE,
                "--timeout-ms",
                "9000",
                "--device",
                "4"
            ]),
            Some(Invocation {
                request: Request::Stop {
                    device: Target::Number(4),
                    bundle: BUNDLE.to_owned(),
                },
                timeout_ms: 9_000,
            })
        );
        let launch = |relaunch| Request::Launch {
            device: Target::Number(5),
            bundle: BUNDLE.to_owned(),
            relaunch,
        };
        assert_eq!(
            request(&["launch", "--device", "5", "--bundle", BUNDLE]),
            Some(launch(false))
        );
        for line in [
            ["launch", "--relaunch", "--device", "5", "--bundle", BUNDLE],
            ["launch", "--device", "5", "--relaunch", "--bundle", BUNDLE],
            ["launch", "--device", "5", "--bundle", BUNDLE, "--relaunch"],
        ] {
            assert_eq!(request(&line), Some(launch(true)), "{line:?}");
        }
        for (identifier, read) in [
            ("COM.Example.App-2", "COM.Example.App-2"),
            ("a", "a"),
            ("com.example.app.widget", "com.example.app.widget"),
        ] {
            assert_eq!(
                request(&["running", "--bundle", identifier, "--device", "3"]),
                Some(Request::Running {
                    device: Target::Number(3),
                    bundle: read.to_owned(),
                }),
                "{identifier}"
            );
        }
        let by_key = ["--device-key", VECTOR_KEY, "--key-salt-env", SALT_VARIABLE];
        let cases: Vec<(Vec<&str>, Request, CommandName)> = vec![
            (
                vec!["running", "--bundle", BUNDLE],
                Request::Running {
                    device: key_target(),
                    bundle: BUNDLE.to_owned(),
                },
                CommandName::Running,
            ),
            (
                vec!["launch", "--relaunch", "--bundle", BUNDLE],
                Request::Launch {
                    device: key_target(),
                    bundle: BUNDLE.to_owned(),
                    relaunch: true,
                },
                CommandName::Launch,
            ),
            (
                vec!["stop", "--bundle", BUNDLE],
                Request::Stop {
                    device: key_target(),
                    bundle: BUNDLE.to_owned(),
                },
                CommandName::Stop,
            ),
        ];
        for (line, expected, name) in cases {
            let mut keyed = line.clone();
            keyed.extend(by_key);
            assert_eq!(request(&keyed), Some(expected.clone()), "{keyed:?}");
            assert_eq!(expected.name(), name);
            assert_eq!(expected.device(), None);
            assert_eq!(expected.via(), None);
        }
        assert_eq!(launch(true).name(), CommandName::Launch);
        assert_eq!(launch(true).device(), Some(5));
    }

    /// `--bundle` is required by `running`, `launch` and `stop` and taken by no other command;
    /// it takes one bundle identifier written in its own characters, once; `--relaunch` is
    /// taken by `launch` alone, once, and takes no value; the device flags are refused in the
    /// same pairs as before.
    #[test]
    fn every_other_application_command_line_is_refused() {
        let lines: Vec<Vec<&str>> = vec![
            // --bundle is required.
            vec!["running", "--device", "3"],
            vec!["launch", "--device", "3"],
            vec!["launch", "--device", "3", "--relaunch"],
            vec!["stop", "--device", "3"],
            vec!["running", "--device", "3", "--bundle"],
            // --bundle takes a bundle identifier, once.
            vec!["running", "--device", "3", "--bundle", ""],
            vec![
                "running",
                "--device",
                "3",
                "--bundle",
                "com.example.end_game",
            ],
            vec![
                "running",
                "--device",
                "3",
                "--bundle",
                "com.example.app two",
            ],
            vec!["stop", "--device", "3", "--bundle", "com.example.\u{e4}pp"],
            vec!["launch", "--device", "3", "--bundle", "com/example/app"],
            vec![
                "running", "--device", "3", "--bundle", BUNDLE, "--bundle", BUNDLE,
            ],
            // --relaunch is launch's alone, once, without a value.
            vec!["running", "--device", "3", "--bundle", BUNDLE, "--relaunch"],
            vec!["stop", "--relaunch", "--device", "3", "--bundle", BUNDLE],
            vec![
                "launch",
                "--device",
                "3",
                "--bundle",
                BUNDLE,
                "--relaunch",
                "--relaunch",
            ],
            vec![
                "launch",
                "--device",
                "3",
                "--bundle",
                BUNDLE,
                "--relaunch",
                "true",
            ],
            vec!["info", "--device", "3", "--relaunch"],
            vec!["devices", "--relaunch"],
            // No other command takes --bundle; the application commands take no other option.
            vec!["info", "--device", "3", "--bundle", BUNDLE],
            vec![
                "apps",
                "--device",
                "3",
                "--contains",
                "app",
                "--bundle",
                BUNDLE,
            ],
            vec!["devices", "--bundle", BUNDLE],
            vec![
                "running",
                "--device",
                "3",
                "--bundle",
                BUNDLE,
                "--via",
                "springboard",
            ],
            vec![
                "launch", "--device", "3", "--bundle", BUNDLE, "--out", "a.png",
            ],
            vec![
                "stop",
                "--device",
                "3",
                "--bundle",
                BUNDLE,
                "--contains",
                "app",
            ],
            vec!["RUNNING", "--device", "3", "--bundle", BUNDLE],
            // The device flags, refused as before.
            vec!["running", "--bundle", BUNDLE],
            vec![
                "launch",
                "--device",
                "3",
                "--device-key",
                VECTOR_KEY,
                "--key-salt-env",
                SALT_VARIABLE,
                "--bundle",
                BUNDLE,
            ],
            vec!["stop", "--device-key", VECTOR_KEY, "--bundle", BUNDLE],
            vec![
                "running",
                "--key-salt-env",
                SALT_VARIABLE,
                "--bundle",
                BUNDLE,
            ],
            vec![
                "launch",
                "--device",
                "3",
                "--key-salt-env",
                SALT_VARIABLE,
                "--bundle",
                BUNDLE,
            ],
            vec![
                "stop",
                "--device-key",
                VECTOR_KEY,
                "--key-salt-env",
                SALT_VARIABLE,
                "--binding-salt-env",
                "ACIB_BINDING",
                "--bundle",
                BUNDLE,
            ],
            vec![
                "running", "--device", "3", "--device", "3", "--bundle", BUNDLE,
            ],
        ];
        for line in lines {
            assert_eq!(parsed(&line), None, "{line:?}");
        }
    }
}
