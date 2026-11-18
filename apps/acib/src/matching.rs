// SPDX-License-Identifier: AGPL-3.0-only

//! Which processes of the phone are of one application (board #479, ACIB-3): the pure matching
//! of `running` and `stop`. (ACIB-3c) The bundle's record comes from the installation proxy's
//! lookup of that one bundle, and the processes from the instruments server's list of running
//! processes ([`listed_processes`]).
//!
//! The installation proxy's record gives the application its bundle identifier and the path of
//! its bundle, a directory (its `Path`); the instruments server's list gives each process its pid
//! and, in `realAppName`, what is read here as the location of its executable, and no bundle
//! identifier. A process is **of the bundle** when its executable lies
//! inside the bundle's directory (Apple's documentation, "Placing content in a bundle": on iOS
//! the main executable is at the root of the bundle and app extensions in `PlugIns/`), and the
//! two kinds are answered apart (the coordinator's ruling on board #479, fix round 1 of ACIB-3):
//! a process whose executable lies **directly** in the directory is the application itself, and
//! only it counts for `running` and is ended by `stop`; a process whose executable lies deeper
//! (an app extension in `PlugIns/`, a helper inside a nested bundle) is the bundle's, but is not
//! the application running: a widget the system runs for it does not make the application run,
//! and `stop` leaves it, as a person who swipes the application away does.
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
//! the bundle asked for is installed and the pids of its own processes, by kind.

use idevice::services::dvt::message::Message;

/// The members of an entry of the instruments server's list of running processes that this
/// program reads: its pid, and `realAppName`, read as the location of its executable. The
/// library's own reader of that list (`DeviceInfoClient::running_processes`, idevice 0.1.68,
/// `services/dvt/device_info.rs` 41-92, under a feature `acib` does not turn on) reads these two
/// beside `name`, `isApplication` and `startPageCount`, which are not read here; it documents
/// none of them, so that `realAppName` is the path of the executable is read from no description
/// and is to be seen on a phone.
const PID: &str = "pid";
const REAL_APP_NAME: &str = "realAppName";

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

/// The pids of a bundle's processes, each list ascending and each pid in it once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pids {
    /// The application itself: the processes whose executable lies directly in the bundle's
    /// directory.
    pub application: Vec<u32>,
    /// The processes whose executable lies deeper in the bundle's directory: its extensions'
    /// and its nested bundles' (the bundle's, not the application running).
    pub extensions: Vec<u32>,
}

/// Where a path lies in a directory it is inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Depth {
    /// In the directory itself: `<directory>/<name>`.
    Directly,
    /// In a directory below it.
    Deeper,
}

/// The pids of the processes whose executable lies inside one of `directories`, by kind
/// ([`Pids`]); a process without a location, or with one read as no place, is of none.
pub fn pids_inside<'a>(
    directories: &[String],
    processes: impl IntoIterator<Item = Process<'a>>,
) -> Pids {
    let mut pids = Pids::default();
    for process in processes {
        let depth = process.executable.and_then(place).and_then(|path| {
            directories
                .iter()
                .find_map(|directory| depth_in(directory, &path))
        });
        match depth {
            Some(Depth::Directly) => pids.application.push(process.pid),
            Some(Depth::Deeper) => pids.extensions.push(process.pid),
            None => {}
        }
    }
    for list in [&mut pids.application, &mut pids.extensions] {
        list.sort_unstable();
        list.dedup();
    }
    pids
}

/// (ACIB-3c) The processes of the instruments server's answer `reply` to `runningProcesses`, as
/// [`pids_inside`] reads them: of each entry its pid and, as the location of its executable, its
/// `realAppName` when that is a text (an entry without one is of no application). `None` when the
/// answer is no list, or one of its entries is no dictionary or has no pid that is a whole number
/// from 0 to `u32::MAX`: a process whose pid cannot be read could be the bundle's, so the whole
/// list is unreadable. Nothing of an entry but these two is read, and what is returned borrows
/// from `reply`, which stays with the command.
pub fn listed_processes(reply: &Message) -> Option<Vec<Process<'_>>> {
    let entries = reply.data.as_ref()?.as_array()?;
    entries
        .iter()
        .map(|entry| {
            let members = entry.as_dictionary()?;
            let pid = u32::try_from(members.get(PID)?.as_unsigned_integer()?).ok()?;
            let executable = members
                .get(REAL_APP_NAME)
                .and_then(|location| location.as_string());
            Some(Process { pid, executable })
        })
        .collect()
}

/// Where `path` lies in `directory`, when it lies inside it: below it, at a component's
/// boundary (`place` leaves no empty component and no slash at the end).
fn depth_in(directory: &str, path: &str) -> Option<Depth> {
    let rest = path.strip_prefix(directory)?.strip_prefix('/')?;
    Some(if rest.contains('/') {
        Depth::Deeper
    } else {
        Depth::Directly
    })
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
    use idevice::services::dvt::message::{MessageHeader, PayloadHeader};
    use idevice::xpc::{Dictionary, XPCObject};

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

    /// The pids of `processes` that are of [`BUNDLE`] in the list of [`applications`], by kind.
    fn of_the_bundle(processes: &[Process<'_>]) -> Pids {
        match installation(BUNDLE, applications()) {
            Installation::Installed(directories) => {
                pids_inside(&directories, processes.iter().copied())
            }
            other => panic!("the bundle is installed: {other:?}"),
        }
    }

    /// The application itself, `pids` alone.
    fn application(pids: &[u32]) -> Pids {
        Pids {
            application: pids.to_vec(),
            extensions: Vec::new(),
        }
    }

    /// A bundle no process of the list is of has no pid of either kind; processes of another
    /// application, one without a location and the system's own are of no bundle.
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
        assert_eq!(of_the_bundle(&processes), Pids::default());
        assert_eq!(of_the_bundle(&[]), Pids::default());
    }

    /// The one process of the bundle, its main executable directly in its directory, is the
    /// application; another application's process beside it is of no bundle.
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
        assert_eq!(of_the_bundle(&processes), application(&[412]));
    }

    /// An application and its extensions both alive: the main executable, directly in the
    /// bundle's directory, is the application; each executable nested deeper (an app extension
    /// in `PlugIns/`, a helper inside a nested bundle) is answered apart, as the bundle's
    /// extensions; each list ascending and each pid once; another application's extension is
    /// neither.
    #[test]
    fn the_application_and_its_extensions_are_answered_apart() {
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
                640,
                "file:///private/var/containers/Bundle/Application/TESTUUID-0001/Example.app/Frameworks/Helper.framework/Helper.app/Helper",
            ),
            process(
                412,
                "file:///private/var/containers/Bundle/Application/TESTUUID-0001/Example.app/Example",
            ),
        ];
        assert_eq!(
            of_the_bundle(&processes),
            Pids {
                application: vec![412],
                extensions: vec![415, 530, 640],
            }
        );
    }

    /// An application of which only an extension is alive (a widget the system runs for it)
    /// does not run: its application list is empty, and the extension is answered apart.
    #[test]
    fn an_application_with_only_an_extension_alive_does_not_run() {
        let processes = [
            process(
                201,
                "file:///private/var/containers/Bundle/Application/TESTUUID-0002/Other.app/Other",
            ),
            process(
                530,
                "file:///private/var/containers/Bundle/Application/TESTUUID-0001/Example.app/PlugIns/Widget.appex/Widget",
            ),
        ];
        assert_eq!(
            of_the_bundle(&processes),
            Pids {
                application: Vec::new(),
                extensions: vec![530],
            }
        );
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
        assert_eq!(of_the_bundle(&processes), Pids::default());
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
        assert_eq!(of_the_bundle(&processes), application(&[704]));
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

    /// (ACIB-3c) The bundle's directory as the installation proxy's record gives it, a plain path.
    const RECORD_PATH: &str =
        "/private/var/containers/Bundle/Application/TESTUUID-0001/Example.app";
    /// Where the tests' applications lie, in the shape of the instruments server's `realAppName`.
    const BUNDLES: &str = "/private/var/containers/Bundle/Application";

    /// An entry of the instruments server's list of running processes, as the library's reader
    /// knows one: `pid`, `realAppName` when given, and the members that are not read, its `name`
    /// the bundle's executable's in every entry (a name is not read, so it makes no process the
    /// bundle's).
    fn listed(pid: XPCObject, real_app_name: Option<XPCObject>) -> XPCObject {
        let mut members = Dictionary::new();
        members.insert("pid".to_owned(), pid);
        members.insert("name".to_owned(), XPCObject::String("Example".to_owned()));
        if let Some(location) = real_app_name {
            members.insert("realAppName".to_owned(), location);
        }
        members.insert("isApplication".to_owned(), XPCObject::Bool(true));
        members.insert("startPageCount".to_owned(), XPCObject::UInt64(0));
        XPCObject::Dictionary(members)
    }

    /// The entry of the process `pid` whose `realAppName` is `location`.
    fn at(pid: u64, location: &str) -> XPCObject {
        listed(
            XPCObject::UInt64(pid),
            Some(XPCObject::String(location.to_owned())),
        )
    }

    /// The instruments server's answer holding `data` (as the library's reader decodes it, a
    /// property list value), on a channel of its own.
    fn reply(data: Option<XPCObject>) -> Message {
        Message::new(
            MessageHeader::new(0, 1, 7, 1, 1, false),
            PayloadHeader::method_invocation(),
            None,
            data.map(|value| value.to_plist()),
        )
    }

    /// The answer listing `entries`.
    fn answer(entries: Vec<XPCObject>) -> Message {
        reply(Some(XPCObject::Array(entries)))
    }

    /// The pids of [`BUNDLE`]'s processes in the answer `reply`, its directory the installation
    /// proxy's record's [`RECORD_PATH`], by kind.
    fn in_reply(reply: &Message) -> Pids {
        let record = [Application {
            bundle_identifier: BUNDLE,
            path: RECORD_PATH,
        }];
        match installation(BUNDLE, record) {
            Installation::Installed(directories) => {
                pids_inside(&directories, listed_processes(reply).expect("a list"))
            }
            other => panic!("the bundle is installed: {other:?}"),
        }
    }

    /// (ACIB-3c) The cases above, on the shape of the new sources: the installation proxy's
    /// record gives the directory as a plain path, the instruments server's list each process's
    /// `realAppName`. No process of the bundle (the system's, another application's, one without
    /// a location, an empty list); one; the application and its extensions (each list ascending,
    /// each pid once, another application's extension neither); only an extension alive; paths
    /// that merely start alike; a path compared exactly, case included, `/var` and `/private/var`
    /// alike and a file URL read as its path. Every entry's `name` is the bundle's executable's,
    /// and none of them is the bundle's by it.
    #[test]
    fn the_instruments_list_gives_the_bundles_pids_by_kind() {
        let inside = |rest: &str| format!("{BUNDLES}/TESTUUID-0001/Example.app/{rest}");
        let other = format!("{BUNDLES}/TESTUUID-0002/Other.app/Other");

        let none = answer(vec![
            at(1, "/sbin/launchd"),
            at(201, &other),
            listed(XPCObject::UInt64(202), None),
        ]);
        assert_eq!(in_reply(&none), Pids::default());
        assert_eq!(in_reply(&answer(Vec::new())), Pids::default());

        let one = answer(vec![at(201, &other), at(412, &inside("Example"))]);
        assert_eq!(in_reply(&one), application(&[412]));

        let with_extensions = answer(vec![
            at(530, &inside("PlugIns/Widget.appex/Widget")),
            at(412, &inside("Example")),
            at(
                201,
                &format!("{BUNDLES}/TESTUUID-0002/Other.app/PlugIns/Share.appex/Share"),
            ),
            at(415, &inside("PlugIns/Notify.appex/Notify")),
            at(
                640,
                &inside("Frameworks/Helper.framework/Helper.app/Helper"),
            ),
            at(412, &inside("Example")),
        ]);
        assert_eq!(
            in_reply(&with_extensions),
            Pids {
                application: vec![412],
                extensions: vec![415, 530, 640],
            }
        );

        let only_an_extension = answer(vec![
            at(201, &other),
            at(530, &inside("PlugIns/Widget.appex/Widget")),
        ]);
        assert_eq!(
            in_reply(&only_an_extension),
            Pids {
                application: Vec::new(),
                extensions: vec![530],
            }
        );

        let alike = answer(vec![
            at(
                601,
                &format!("{BUNDLES}/TESTUUID-0001/Example.appendix/Example"),
            ),
            at(
                602,
                &format!("{BUNDLES}/TESTUUID-0001/Example.app2/Example"),
            ),
            at(
                603,
                &format!("{BUNDLES}/TESTUUID-00011/Example.app/Example"),
            ),
            at(604, RECORD_PATH),
            at(605, &inside("../Other.app/Other")),
        ]);
        assert_eq!(in_reply(&alike), Pids::default());

        let cased = answer(vec![
            at(701, &format!("{BUNDLES}/TESTUUID-0001/example.app/Example")),
            at(702, &format!("{BUNDLES}/testuuid-0001/Example.app/Example")),
            at(
                703,
                "/PRIVATE/var/containers/Bundle/Application/TESTUUID-0001/Example.app/Example",
            ),
            at(
                704,
                "/var/containers/Bundle/Application/TESTUUID-0001/Example.app/Example",
            ),
            at(705, &format!("file://{}", inside("Example"))),
        ]);
        assert_eq!(in_reply(&cased), application(&[704, 705]));
    }

    /// (ACIB-3c) Of the instruments server's answer only each entry's pid and `realAppName` are
    /// read, a `realAppName` that is no text read as no location; the pids 0 and `u32::MAX` are
    /// pids. The answer is unreadable (`None`) when it holds no data, is no list, or one entry is
    /// no dictionary, has no pid, or a pid that is negative, larger than `u32::MAX` or a text.
    #[test]
    fn the_instruments_list_is_read_whole_or_not_at_all() {
        let read = answer(vec![
            at(412, "/var/a/Example.app/Example"),
            listed(XPCObject::UInt64(530), None),
            listed(XPCObject::UInt64(531), Some(XPCObject::UInt64(9))),
            at(0, "/sbin/launchd"),
            at(u64::from(u32::MAX), "/var/b/Other.app/Other"),
        ]);
        let processes: Vec<(u32, Option<&str>)> = listed_processes(&read)
            .expect("a list")
            .iter()
            .map(|process| (process.pid, process.executable))
            .collect();
        assert_eq!(
            processes,
            [
                (412, Some("/var/a/Example.app/Example")),
                (530, None),
                (531, None),
                (0, Some("/sbin/launchd")),
                (u32::MAX, Some("/var/b/Other.app/Other")),
            ]
        );

        let mut without_pid = Dictionary::new();
        without_pid.insert(
            "realAppName".to_owned(),
            XPCObject::String("/var/a/Example.app/Example".to_owned()),
        );
        let unreadable = [
            reply(None),
            reply(Some(XPCObject::Dictionary(Dictionary::new()))),
            reply(Some(XPCObject::String("runningProcesses".to_owned()))),
            answer(vec![
                at(412, "/var/a/Example.app/Example"),
                XPCObject::String("412".to_owned()),
            ]),
            answer(vec![XPCObject::Dictionary(without_pid)]),
            answer(vec![listed(XPCObject::Int64(-1), None)]),
            answer(vec![listed(
                XPCObject::UInt64(u64::from(u32::MAX) + 1),
                None,
            )]),
            answer(vec![listed(XPCObject::String("412".to_owned()), None)]),
        ];
        for (case, message) in unreadable.iter().enumerate() {
            assert!(listed_processes(message).is_none(), "case {case}");
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
