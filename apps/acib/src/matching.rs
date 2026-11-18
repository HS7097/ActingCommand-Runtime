// SPDX-License-Identifier: AGPL-3.0-only

//! Which processes of the phone are of one application (board #479, ACIB-3): the pure matching
//! of `running` and `stop`, on the application service's two lists.
//!
//! The list of applications gives each application its bundle identifier and the path of its
//! bundle, a directory; the list of processes gives each process its pid and the location of its
//! executable, and no bundle identifier. A process is **of the bundle** when its executable lies
//! inside the bundle's directory: the application's main executable, at the top of the
//! directory, and every executable nested deeper in it, as an app extension's in `PlugIns/` is
//! (Apple's documentation, "Placing content in a bundle": on iOS the main executable is at the
//! root of the bundle and app extensions in `PlugIns/`). Everything in that directory is the
//! application's own code, signed with it, installed and removed with it; an extension runs for
//! it; and no other application's executable can lie in it. So `running` counts an extension
//! that runs, and `stop` ends it too, so that nothing the application runs is left.
//!
//! Bundle identifiers are compared without regard to ASCII case (Apple's documentation of
//! `CFBundleIdentifier`: "Bundle IDs are case-insensitive"); paths are compared exactly, case
//! included (iOS's file system is case-sensitive, Apple's APFS guide says). A location is read
//! as a path: an absolute path as it is, a `file://` URL (empty host or `localhost`) as its
//! percent-decoded path; `/private/var/…` and `/var/…` are read alike (on iOS the one is the
//! other); a slash at its end is dropped; one with an empty, `.` or `..` component, or that is
//! not absolute, is read as no place at all (a process there is of no application).
//!
//! Nothing here keeps or gives anything of another application: the functions answer whether
//! the bundle asked for is installed and the pids of its own processes.

/// The scheme of a file URL.
const FILE_SCHEME: &str = "file://";
/// The host a file URL may name for this device.
const LOCAL_HOST: &str = "localhost";
/// What `/var` is reached through on iOS: `/private/var/…` is read as `/var/…`.
const PRIVATE: &str = "/private";
/// The place `/private` leads to that is read without it.
const VAR: &str = "/var/";

/// An application of the service's list, as the matching reads it.
#[derive(Debug, Clone, Copy)]
pub struct Application<'a> {
    pub bundle_identifier: &'a str,
    /// The path of the application's bundle, a directory.
    pub path: &'a str,
}

/// A process of the service's list, as the matching reads it.
#[derive(Debug, Clone, Copy)]
pub struct Process<'a> {
    pub pid: u32,
    /// The location of the process's executable, when the list gives one.
    pub executable: Option<&'a str>,
}

/// What the list of applications says of one bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installation {
    /// No application of the list has the bundle's identifier.
    NotInstalled,
    /// The directories of the applications with the bundle's identifier (one, as a rule), as
    /// `place` reads them.
    Installed(Vec<String>),
    /// An application with the bundle's identifier has a path that cannot be read as a place:
    /// which processes are of it cannot be told.
    PathUnreadable,
}

/// What the list of applications says of `bundle`: the entries whose identifier is `bundle`
/// without regard to ASCII case, and their directories.
pub fn installation<'a>(
    bundle: &str,
    applications: impl IntoIterator<Item = Application<'a>>,
) -> Installation {
    let mut directories = Vec::new();
    for application in applications {
        if !application.bundle_identifier.eq_ignore_ascii_case(bundle) {
            continue;
        }
        match place(application.path) {
            Some(directory) => directories.push(directory),
            None => return Installation::PathUnreadable,
        }
    }
    if directories.is_empty() {
        Installation::NotInstalled
    } else {
        Installation::Installed(directories)
    }
}

/// The pids, ascending and each once, of the processes whose executable lies inside one of
/// `directories` (a process without a location, or with one read as no place, is of none).
pub fn pids_inside<'a>(
    directories: &[String],
    processes: impl IntoIterator<Item = Process<'a>>,
) -> Vec<u32> {
    let mut pids: Vec<u32> = processes
        .into_iter()
        .filter(|process| {
            process
                .executable
                .and_then(place)
                .is_some_and(|path| directories.iter().any(|directory| inside(directory, &path)))
        })
        .map(|process| process.pid)
        .collect();
    pids.sort_unstable();
    pids.dedup();
    pids
}

/// Whether `path` lies inside `directory`: below it, at a component's boundary.
fn inside(directory: &str, path: &str) -> bool {
    path.strip_prefix(directory)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// The place a location names, as the matching compares it (see the module's text); `None`
/// when it names none.
fn place(location: &str) -> Option<String> {
    let path = match location.strip_prefix(FILE_SCHEME) {
        Some(rest) => percent_decoded(rest.strip_prefix(LOCAL_HOST).unwrap_or(rest))?,
        None => location.to_owned(),
    };
    let below_private = path
        .strip_prefix(PRIVATE)
        .filter(|rest| rest.starts_with(VAR))
        .map(str::to_owned);
    let path = below_private.unwrap_or(path);
    let path = path.trim_end_matches('/');
    let mut components = path.split('/');
    let absolute = components.next() == Some("");
    let mut rest = components.peekable();
    if !absolute || rest.peek().is_none() {
        return None;
    }
    if rest.any(|component| component.is_empty() || component == "." || component == "..") {
        return None;
    }
    Some(path.to_owned())
}

/// `text` with each `%` and two hexadecimal digits read as the byte they write, when the bytes
/// are UTF-8; `None` for a `%` without two digits or bytes that are not UTF-8.
fn percent_decoded(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let high = hex_digit(*bytes.get(at + 1)?)?;
            let low = hex_digit(*bytes.get(at + 2)?)?;
            decoded.push(high * 16 + low);
            at += 3;
        } else {
            decoded.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

/// The value of a hexadecimal digit.
fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bundle the tests ask for, and its directory as the list of applications gives it.
    const BUNDLE: &str = "com.example.app";
    const DIRECTORY: &str = "/private/var/containers/Bundle/Application/TESTUUID-0001/Example.app";
    /// Another application and its directory.
    const OTHER_BUNDLE: &str = "com.example.other";
    const OTHER_DIRECTORY: &str =
        "/private/var/containers/Bundle/Application/TESTUUID-0002/Other.app";

    /// The two applications of the tests' list.
    fn applications() -> Vec<Application<'static>> {
        vec![
            Application {
                bundle_identifier: OTHER_BUNDLE,
                path: OTHER_DIRECTORY,
            },
            Application {
                bundle_identifier: BUNDLE,
                path: DIRECTORY,
            },
        ]
    }

    /// A process `pid` whose executable is at `executable`.
    fn process(pid: u32, executable: &str) -> Process<'_> {
        Process {
            pid,
            executable: Some(executable),
        }
    }

    /// The pids of `processes` that are of [`BUNDLE`] in the list of [`applications`].
    fn of_the_bundle(processes: &[Process<'_>]) -> Vec<u32> {
        match installation(BUNDLE, applications()) {
            Installation::Installed(directories) => {
                pids_inside(&directories, processes.iter().copied())
            }
            other => panic!("the bundle is installed: {other:?}"),
        }
    }

    /// A bundle no process of the list is of has no pid; processes of another application, one
    /// without a location and the system's own are of no bundle.
    #[test]
    fn a_bundle_with_no_process_has_no_pid() {
        let processes = [
            process(1, "file:///sbin/launchd"),
            process(
                201,
                "file:///private/var/containers/Bundle/Application/TESTUUID-0002/Other.app/Other",
            ),
            Process {
                pid: 202,
                executable: None,
            },
        ];
        assert_eq!(of_the_bundle(&processes), Vec::<u32>::new());
        assert_eq!(of_the_bundle(&[]), Vec::<u32>::new());
    }

    /// The one process of the bundle, its main executable at the top of its directory, has its
    /// pid; another application's process beside it does not.
    #[test]
    fn a_bundle_with_one_process_has_its_pid() {
        let processes = [
            process(
                201,
                "file:///private/var/containers/Bundle/Application/TESTUUID-0002/Other.app/Other",
            ),
            process(
                412,
                "file:///private/var/containers/Bundle/Application/TESTUUID-0001/Example.app/Example",
            ),
        ];
        assert_eq!(of_the_bundle(&processes), vec![412]);
    }

    /// An application and its extensions: the main executable and each executable nested in
    /// the bundle's directory (an app extension in `PlugIns/`) are all of the bundle, the pids
    /// ascending and each once; another application's extension is not.
    #[test]
    fn an_application_and_its_extension_processes_are_all_of_the_bundle() {
        let processes = [
            process(
                530,
                "file:///private/var/containers/Bundle/Application/TESTUUID-0001/Example.app/PlugIns/Widget.appex/Widget",
            ),
            process(
                412,
                "file:///private/var/containers/Bundle/Application/TESTUUID-0001/Example.app/Example",
            ),
            process(
                201,
                "file:///private/var/containers/Bundle/Application/TESTUUID-0002/Other.app/PlugIns/Share.appex/Share",
            ),
            process(
                415,
                "file:///private/var/containers/Bundle/Application/TESTUUID-0001/Example.app/PlugIns/Notify.appex/Notify",
            ),
            process(
                412,
                "file:///private/var/containers/Bundle/Application/TESTUUID-0001/Example.app/Example",
            ),
        ];
        assert_eq!(of_the_bundle(&processes), vec![412, 415, 530]);
    }

    /// A process of another application whose path merely starts as the bundle's directory
    /// does (a longer directory name, a sibling) is not of the bundle: a path is inside a
    /// directory only below it, at a component's boundary.
    #[test]
    fn a_process_whose_path_merely_starts_alike_is_not_of_the_bundle() {
        let processes = [
            process(
                601,
                "file:///private/var/containers/Bundle/Application/TESTUUID-0001/Example.appendix/Example",
            ),
            process(
                602,
                "file:///private/var/containers/Bundle/Application/TESTUUID-0001/Example.app2/Example",
            ),
            process(
                603,
                "file:///private/var/containers/Bundle/Application/TESTUUID-00011/Example.app/Example",
            ),
            process(
                604,
                "file:///private/var/containers/Bundle/Application/TESTUUID-0001/Example.app",
            ),
            process(
                605,
                "file:///private/var/containers/Bundle/Application/TESTUUID-0001/Example.app/../Other.app/Other",
            ),
        ];
        assert_eq!(of_the_bundle(&processes), Vec::<u32>::new());
    }

    /// A bundle identifier is matched without regard to ASCII case; a path only exactly, case
    /// included: a process in a directory whose name differs from the bundle's in case is of
    /// another directory, not of the bundle.
    #[test]
    fn a_bundle_identifier_matches_without_regard_to_case_and_a_path_only_exactly() {
        for asked in ["com.example.app", "COM.EXAMPLE.APP", "com.Example.App"] {
            assert_eq!(
                installation(asked, applications()),
                Installation::Installed(vec![
                    "/var/containers/Bundle/Application/TESTUUID-0001/Example.app".to_owned()
                ]),
                "{asked}"
            );
        }
        assert_eq!(
            installation("com.example.ap", applications()),
            Installation::NotInstalled
        );
        assert_eq!(
            installation("com.example.app.extra", applications()),
            Installation::NotInstalled
        );
        let processes = [
            process(
                701,
                "file:///private/var/containers/Bundle/Application/TESTUUID-0001/example.app/Example",
            ),
            process(
                702,
                "file:///private/var/containers/Bundle/Application/testuuid-0001/Example.app/Example",
            ),
            process(
                703,
                "file:///PRIVATE/var/containers/Bundle/Application/TESTUUID-0001/Example.app/Example",
            ),
            process(
                704,
                "file:///private/var/containers/Bundle/Application/TESTUUID-0001/Example.app/Example",
            ),
        ];
        assert_eq!(of_the_bundle(&processes), vec![704]);
    }

    /// A location is read as a path: a plain absolute path, a `file://` URL with an empty host
    /// or `localhost`, percent-decoded; `/private/var/…` as `/var/…`; a slash at its end
    /// dropped. A relative path, another scheme or host, a bad escape and a path with an empty,
    /// `.` or `..` component are read as no place.
    #[test]
    fn a_location_is_read_as_its_path() {
        let read = [
            ("/var/a/Example.app", "/var/a/Example.app"),
            ("/private/var/a/Example.app", "/var/a/Example.app"),
            ("/private/var/a/Example.app/", "/var/a/Example.app"),
            ("file:///private/var/a/Example.app/", "/var/a/Example.app"),
            ("file://localhost/var/a/Example.app", "/var/a/Example.app"),
            (
                "file:///var/a/My%20App.app/My%20App",
                "/var/a/My App.app/My App",
            ),
            (
                "file:///var/a/%C3%84pp.app/%c3%84pp",
                "/var/a/\u{c4}pp.app/\u{c4}pp",
            ),
            ("/Applications/Example.app", "/Applications/Example.app"),
            ("/private/etc/x", "/private/etc/x"),
        ];
        for (location, path) in read {
            assert_eq!(place(location).as_deref(), Some(path), "{location}");
        }
        for location in [
            "",
            "/",
            "var/a/Example.app",
            "Example.app",
            "file://",
            "file:///",
            "file://host/var/a/Example.app",
            "http:///var/a/Example.app",
            "file:///var/a/Example%2.app",
            "file:///var/a/Example%zz.app",
            "file:///var/a/%FF.app",
            "/var//a/Example.app",
            "/var/./a/Example.app",
            "/var/a/../b/Example.app",
        ] {
            assert_eq!(place(location), None, "{location:?}");
        }
    }

    /// The list of applications says whether the bundle is installed and where: not installed
    /// when no entry has its identifier, the directories of the entries that have it, and
    /// `PathUnreadable` when one of them has a path read as no place.
    #[test]
    fn the_list_of_applications_says_whether_and_where_the_bundle_is_installed() {
        assert_eq!(installation(BUNDLE, Vec::new()), Installation::NotInstalled);
        assert_eq!(
            installation(
                BUNDLE,
                [Application {
                    bundle_identifier: OTHER_BUNDLE,
                    path: OTHER_DIRECTORY,
                }]
            ),
            Installation::NotInstalled
        );
        assert_eq!(
            installation(
                BUNDLE,
                [Application {
                    bundle_identifier: BUNDLE,
                    path: "Example.app",
                }]
            ),
            Installation::PathUnreadable
        );
        assert_eq!(
            installation(
                BUNDLE,
                [
                    Application {
                        bundle_identifier: BUNDLE,
                        path: "/Applications/Example.app",
                    },
                    Application {
                        bundle_identifier: BUNDLE,
                        path: "",
                    },
                ]
            ),
            Installation::PathUnreadable
        );
    }
}
