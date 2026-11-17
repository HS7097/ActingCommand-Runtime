// SPDX-License-Identifier: AGPL-3.0-only

//! The one JSON line `acib` prints for each command (board #479, ACIB-0).
//!
//! These types are the whole of what the program shows. None of them has a member for a
//! device's serial number, UDID, name or address, or for anything of a pairing record: the
//! line cannot carry them. A library error is shown through [`crate::redact`], which cuts a
//! text that could carry such a value to the error's kind.

use crate::redact;
use serde::Serialize;

/// The command a line answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandName {
    Devices,
    Info,
    Services,
    Images,
}

/// Why a command was refused or failed: this program's own words, not Runtime outcome codes.
/// Each word has one cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Problem {
    /// The command line is not one of the four commands with their options.
    CommandLineInvalid,
    /// The async runtime of this process could not be built.
    AsyncRuntimeFailed,
    /// The command did not finish within `--timeout-ms`.
    DeadlinePassed,
    /// No connection to usbmuxd.
    UsbmuxdUnreachable,
    /// usbmuxd did not answer the device list.
    UsbmuxdListFailed,
    /// usbmuxd lists no device with the given number.
    DeviceNotListed,
    /// No connection to the device's lockdown service.
    LockdownUnreachable,
    /// A lockdown value could not be read, and the device did not refuse it by a word.
    LockdownReadFailed,
    /// A lockdown value is not of the type its key has.
    LockdownValueUnexpected,
    /// usbmuxd's answer to the pairing record request could not be read.
    PairRecordUnreadable,
    /// usbmuxd holds no pairing record of this host for the device.
    NotPaired,
    /// The device refused the lockdown session with this host's pairing record.
    SessionRefused,
    /// The device refused to start the service.
    ServiceRefused,
    /// The started service could not be connected to.
    ServiceUnreachable,
    /// The CoreDeviceProxy tunnel or the TCP stack inside this process failed.
    TunnelFailed,
    /// The remote service discovery port inside the tunnel could not be connected to.
    RsdUnreachable,
    /// The remote service discovery handshake failed.
    RsdHandshakeFailed,
    /// The image mounter's lookup failed other than by finding no image.
    ImageLookupFailed,
}

/// A refusal or failure: the word, and the library's error text where the library gave one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Failure {
    pub problem: Problem,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Failure {
    /// A failure this program finds itself; there is no library text.
    pub const fn own(problem: Problem) -> Self {
        Self {
            problem,
            error: None,
        }
    }

    /// A failure the library (or the OS beneath it) reported: its text, or its kind alone where
    /// the text could carry an identifier.
    pub fn library<E>(problem: Problem, error: &E) -> Self
    where
        E: std::fmt::Debug + std::fmt::Display,
    {
        let kind = redact::kind_of(&format!("{error:?}"));
        Self {
            problem,
            error: Some(redact::shown_error(&kind, &error.to_string())),
        }
    }
}

/// How usbmuxd reaches a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Link {
    Usb,
    Network,
    /// A connection usbmuxd names otherwise; its description is not shown.
    Unknown,
}

/// One device of `acib devices`: its usbmuxd session number and its connection, nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceEntry {
    pub device: u32,
    pub connection: Link,
}

/// A lockdown value read in a session: the value, or the word the device refused it with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reading {
    Value(bool),
    Refused(String),
}

/// `acib devices`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Devices {
    pub command: CommandName,
    pub devices: Vec<DeviceEntry>,
}

/// `acib info --device <n>`: the values read without a session, whether this host is paired,
/// and the two values read in a session. A command that stops in its second half keeps what
/// its first half read and names its problem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Info {
    pub command: CommandName,
    pub device: u32,
    pub product_version: String,
    pub product_type: String,
    pub build_version: String,
    pub device_class: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paired: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_protected: Option<Reading>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub developer_mode: Option<Reading>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<Problem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Info {
    /// This answer, stopped by `failure`.
    pub fn stopped(mut self, failure: Failure) -> Self {
        self.problem = Some(failure.problem);
        self.error = failure.error;
        self
    }
}

/// `acib services --device <n>`: the remote services' names, sorted, and their count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Services {
    pub command: CommandName,
    pub device: u32,
    pub count: usize,
    pub services: Vec<String>,
}

/// `acib images --device <n>`: whether a developer image is mounted, and its kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Images {
    pub command: CommandName,
    pub device: u32,
    pub mounted: bool,
    pub kind: Option<String>,
}

/// A refused or failed command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Refused {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<u32>,
    pub problem: Problem,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The line a command prints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Line {
    Devices(Devices),
    Info(Info),
    Services(Services),
    Images(Images),
    Refused(Refused),
}

impl Line {
    /// A refusal of `command` on `device`.
    pub fn refused(command: Option<CommandName>, device: Option<u32>, failure: Failure) -> Self {
        Self::Refused(Refused {
            command,
            device,
            problem: failure.problem,
            error: failure.error,
        })
    }

    /// Whether the command was answered (status 0); a line with a problem is not (status 1).
    pub fn answered(&self) -> bool {
        match self {
            Self::Refused(_) => false,
            Self::Info(info) => info.problem.is_none(),
            Self::Devices(_) | Self::Services(_) | Self::Images(_) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn json(line: &Line) -> String {
        serde_json::to_string(line).expect("an answer serializes")
    }

    fn first_half() -> Info {
        Info {
            command: CommandName::Info,
            device: 3,
            product_version: "27.0.1".to_owned(),
            product_type: "TESTPRODUCT1,1".to_owned(),
            build_version: "TESTBUILD1".to_owned(),
            device_class: "iPhone".to_owned(),
            paired: None,
            password_protected: None,
            developer_mode: None,
            problem: None,
            error: None,
        }
    }

    /// One line of every shape the program prints.
    fn every_shape() -> Vec<(Line, &'static str, bool)> {
        let paired = Info {
            paired: Some(true),
            password_protected: Some(Reading::Value(false)),
            developer_mode: Some(Reading::Refused("GetProhibited".to_owned())),
            ..first_half()
        };
        let not_paired = Info {
            paired: Some(false),
            ..first_half()
        }
        .stopped(Failure::own(Problem::NotPaired));
        let session_refused = Info {
            paired: Some(true),
            ..first_half()
        }
        .stopped(Failure {
            problem: Problem::SessionRefused,
            error: Some("InvalidHostID".to_owned()),
        });
        vec![
            (
                Line::Devices(Devices {
                    command: CommandName::Devices,
                    devices: vec![
                        DeviceEntry {
                            device: 3,
                            connection: Link::Usb,
                        },
                        DeviceEntry {
                            device: 7,
                            connection: Link::Network,
                        },
                        DeviceEntry {
                            device: 9,
                            connection: Link::Unknown,
                        },
                    ],
                }),
                r#"{"command":"devices","devices":[{"device":3,"connection":"usb"},{"device":7,"connection":"network"},{"device":9,"connection":"unknown"}]}"#,
                true,
            ),
            (
                Line::Devices(Devices {
                    command: CommandName::Devices,
                    devices: Vec::new(),
                }),
                r#"{"command":"devices","devices":[]}"#,
                true,
            ),
            (
                Line::Info(paired),
                r#"{"command":"info","device":3,"product_version":"27.0.1","product_type":"TESTPRODUCT1,1","build_version":"TESTBUILD1","device_class":"iPhone","paired":true,"password_protected":{"value":false},"developer_mode":{"refused":"GetProhibited"}}"#,
                true,
            ),
            (
                Line::Info(not_paired),
                r#"{"command":"info","device":3,"product_version":"27.0.1","product_type":"TESTPRODUCT1,1","build_version":"TESTBUILD1","device_class":"iPhone","paired":false,"problem":"not_paired"}"#,
                false,
            ),
            (
                Line::Info(session_refused),
                r#"{"command":"info","device":3,"product_version":"27.0.1","product_type":"TESTPRODUCT1,1","build_version":"TESTBUILD1","device_class":"iPhone","paired":true,"problem":"session_refused","error":"InvalidHostID"}"#,
                false,
            ),
            (
                Line::Info(first_half().stopped(Failure {
                    problem: Problem::PairRecordUnreadable,
                    error: Some("device socket io failed".to_owned()),
                })),
                r#"{"command":"info","device":3,"product_version":"27.0.1","product_type":"TESTPRODUCT1,1","build_version":"TESTBUILD1","device_class":"iPhone","problem":"pair_record_unreadable","error":"device socket io failed"}"#,
                false,
            ),
            (
                Line::Services(Services {
                    command: CommandName::Services,
                    device: 3,
                    count: 2,
                    services: vec!["com.example.one".to_owned(), "com.example.two".to_owned()],
                }),
                r#"{"command":"services","device":3,"count":2,"services":["com.example.one","com.example.two"]}"#,
                true,
            ),
            (
                Line::Images(Images {
                    command: CommandName::Images,
                    device: 3,
                    mounted: true,
                    kind: Some("Personalized".to_owned()),
                }),
                r#"{"command":"images","device":3,"mounted":true,"kind":"Personalized"}"#,
                true,
            ),
            (
                Line::Images(Images {
                    command: CommandName::Images,
                    device: 3,
                    mounted: false,
                    kind: None,
                }),
                r#"{"command":"images","device":3,"mounted":false,"kind":null}"#,
                true,
            ),
            (
                Line::refused(
                    Some(CommandName::Services),
                    Some(3),
                    Failure {
                        problem: Problem::TunnelFailed,
                        error: Some("TLS error".to_owned()),
                    },
                ),
                r#"{"command":"services","device":3,"problem":"tunnel_failed","error":"TLS error"}"#,
                false,
            ),
            (
                Line::refused(
                    Some(CommandName::Images),
                    Some(3),
                    Failure::own(Problem::NotPaired),
                ),
                r#"{"command":"images","device":3,"problem":"not_paired"}"#,
                false,
            ),
            (
                Line::refused(
                    Some(CommandName::Devices),
                    None,
                    Failure::own(Problem::DeadlinePassed),
                ),
                r#"{"command":"devices","problem":"deadline_passed"}"#,
                false,
            ),
            (
                Line::refused(None, None, Failure::own(Problem::CommandLineInvalid)),
                r#"{"problem":"command_line_invalid"}"#,
                false,
            ),
        ]
    }

    #[test]
    fn each_answer_is_one_json_line_of_its_shape_and_its_status() {
        for (line, expected, answered) in every_shape() {
            let text = json(&line);
            assert_eq!(text, expected);
            assert!(!text.contains('\n'), "one line: {text}");
            assert_eq!(line.answered(), answered, "status of {text}");
        }
    }

    /// The answer types have no member for a serial number, a UDID, a name, an address or a
    /// pairing record: every member any line holds is one of these.
    #[test]
    fn the_answers_hold_no_member_for_an_identifier() {
        const MEMBERS: &[&str] = &[
            "build_version",
            "command",
            "connection",
            "count",
            "developer_mode",
            "device",
            "device_class",
            "devices",
            "error",
            "kind",
            "mounted",
            "paired",
            "password_protected",
            "problem",
            "product_type",
            "product_version",
            "refused",
            "services",
            "value",
        ];
        fn collect(value: &serde_json::Value, found: &mut BTreeSet<String>) {
            match value {
                serde_json::Value::Object(map) => {
                    for (key, member) in map {
                        found.insert(key.clone());
                        collect(member, found);
                    }
                }
                serde_json::Value::Array(items) => {
                    for item in items {
                        collect(item, found);
                    }
                }
                _ => {}
            }
        }
        let mut found = BTreeSet::new();
        for (line, _, _) in every_shape() {
            let value = serde_json::to_value(&line).expect("an answer serializes");
            collect(&value, &mut found);
        }
        let allowed: BTreeSet<String> = MEMBERS.iter().map(|member| (*member).to_owned()).collect();
        assert_eq!(found, allowed);
    }

    #[test]
    fn every_problem_word_is_this_programs_own_snake_case_word() {
        let words = [
            (Problem::CommandLineInvalid, "command_line_invalid"),
            (Problem::AsyncRuntimeFailed, "async_runtime_failed"),
            (Problem::DeadlinePassed, "deadline_passed"),
            (Problem::UsbmuxdUnreachable, "usbmuxd_unreachable"),
            (Problem::UsbmuxdListFailed, "usbmuxd_list_failed"),
            (Problem::DeviceNotListed, "device_not_listed"),
            (Problem::LockdownUnreachable, "lockdown_unreachable"),
            (Problem::LockdownReadFailed, "lockdown_read_failed"),
            (
                Problem::LockdownValueUnexpected,
                "lockdown_value_unexpected",
            ),
            (Problem::PairRecordUnreadable, "pair_record_unreadable"),
            (Problem::NotPaired, "not_paired"),
            (Problem::SessionRefused, "session_refused"),
            (Problem::ServiceRefused, "service_refused"),
            (Problem::ServiceUnreachable, "service_unreachable"),
            (Problem::TunnelFailed, "tunnel_failed"),
            (Problem::RsdUnreachable, "rsd_unreachable"),
            (Problem::RsdHandshakeFailed, "rsd_handshake_failed"),
            (Problem::ImageLookupFailed, "image_lookup_failed"),
        ];
        let mut seen = BTreeSet::new();
        for (problem, word) in words {
            assert_eq!(
                serde_json::to_value(problem).expect("a word serializes"),
                serde_json::Value::String(word.to_owned())
            );
            assert!(
                word.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                "{word} is snake_case"
            );
            assert!(seen.insert(word), "{word} is one word for one cause");
        }
    }

    #[test]
    fn a_library_error_shows_its_text_or_only_its_kind() {
        #[derive(Debug)]
        enum FakeError {
            Plain,
            Carrying(String),
        }
        impl std::fmt::Display for FakeError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    Self::Plain => f.write_str("device socket io failed"),
                    Self::Carrying(text) => write!(f, "unexpected response from device: {text}"),
                }
            }
        }
        assert_eq!(
            Failure::library(Problem::UsbmuxdUnreachable, &FakeError::Plain),
            Failure {
                problem: Problem::UsbmuxdUnreachable,
                error: Some("device socket io failed".to_owned()),
            }
        );
        let carrying = FakeError::Carrying("0123456789abcdef0123456789abcdef01234567".to_owned());
        assert_eq!(
            Failure::library(Problem::PairRecordUnreadable, &carrying),
            Failure {
                problem: Problem::PairRecordUnreadable,
                error: Some("Carrying".to_owned()),
            }
        );
    }
}
