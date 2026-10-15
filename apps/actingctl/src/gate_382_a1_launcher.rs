// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #382 A-1 (test plan v2.2 §3.7, G13; apps 27): actingwatch, the program the
// watchdog's task runs (contracts/runtime-watchdog.md "Launcher"; rd-1007/v0113/model-374.md
// §2.1). While acsetup holds the installation writer lock it exits 0 and runs nothing. A program
// that is not `<root>\tools\actingwatch.exe`, a root without the watchdog directory, a writer
// lock it cannot probe and a fixed entry it cannot run each end the launch with exit 1: model
// v2.2 §7.5 and §7.6 replaced the launcher's 20-23 with registered codes and the exit of their
// category, error. A fixed entry that cannot run is logged in `watchdog\watchdog.log` with its
// code and path.

use super::{Launch, WriterLock, probe_writer_lock, run};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use tempfile::TempDir;

/// How the injected launch answers the writer lock probe.
#[derive(Clone, Copy)]
enum LockProbe {
    /// The launcher's own probe of the lock file.
    File,
    /// The OS refuses the probe.
    Refused,
}

/// A launch of the program at `exe` whose fixed entry is not deployed, so it cannot run; it
/// keeps each command it was asked to run.
struct InjectedLaunch {
    exe: PathBuf,
    lock: LockProbe,
    runs: Vec<(PathBuf, Vec<OsString>)>,
}

impl InjectedLaunch {
    fn new(exe: PathBuf, lock: LockProbe) -> Self {
        Self {
            exe,
            lock,
            runs: Vec::new(),
        }
    }
}

impl Launch for InjectedLaunch {
    fn current_exe(&self) -> Option<PathBuf> {
        Some(self.exe.clone())
    }

    fn writer_lock(&self, path: &Path) -> WriterLock {
        match self.lock {
            LockProbe::File => probe_writer_lock(path),
            LockProbe::Refused => WriterLock::Failed(io::Error::from(ErrorKind::PermissionDenied)),
        }
    }

    fn status(&mut self, command: &mut Command) -> io::Result<ExitStatus> {
        self.runs.push((
            PathBuf::from(command.get_program()),
            command.get_args().map(OsStr::to_os_string).collect(),
        ));
        Err(io::Error::from(ErrorKind::NotFound))
    }
}

/// `<temp>\<folder>\<name>`, an empty stand-in for this program: the launcher reads only its
/// path. With `watchdog`, `<temp>\watchdog\` exists too.
fn program(temp: &TempDir, folder: &str, name: &str, watchdog: bool) -> PathBuf {
    let directory = temp.path().join(folder);
    fs::create_dir_all(&directory).expect("the program's directory");
    let exe = directory.join(name);
    File::create(&exe).expect("the program's stand-in");
    if watchdog {
        fs::create_dir_all(temp.path().join("watchdog")).expect("the watchdog directory");
    }
    exe
}

#[test]
fn gate_while_the_writer_lock_is_held_the_launcher_exits_0_and_runs_nothing() {
    let temp = TempDir::new().expect("tempdir");
    let exe = program(&temp, "tools", "actingwatch.exe", true);
    let install = temp.path().join("install");
    fs::create_dir_all(&install).expect("the install directory");
    // acsetup's hold: the writer lock taken exclusively through another handle.
    let held = File::create(install.join("writer.lock")).expect("the writer lock file");
    held.lock().expect("hold the writer lock");
    let mut launch = InjectedLaunch::new(exe, LockProbe::File);

    let exit = run(&mut launch);
    drop(held);

    assert_eq!(exit, 0);
    assert!(launch.runs.is_empty(), "it ran {:?}", launch.runs);
}

#[test]
fn gate_a_misplaced_program_a_root_without_watchdog_directory_and_an_unprobed_lock_exit_1() {
    for (case, folder, name, watchdog, lock) in [
        (
            "not under tools",
            "bin",
            "actingwatch.exe",
            true,
            LockProbe::File,
        ),
        (
            "another file name",
            "tools",
            "watcher.exe",
            true,
            LockProbe::File,
        ),
        (
            "no watchdog directory",
            "tools",
            "actingwatch.exe",
            false,
            LockProbe::File,
        ),
        (
            "a writer lock the OS refuses to probe",
            "tools",
            "actingwatch.exe",
            true,
            LockProbe::Refused,
        ),
    ] {
        let temp = TempDir::new().expect("tempdir");
        let mut launch = InjectedLaunch::new(program(&temp, folder, name, watchdog), lock);

        assert_eq!(run(&mut launch), 1, "{case}");
        assert!(launch.runs.is_empty(), "{case}: it ran {:?}", launch.runs);
    }
}

#[test]
fn gate_a_fixed_entry_that_cannot_run_is_logged_with_its_code_and_exits_1() {
    let temp = TempDir::new().expect("tempdir");
    let mut launch = InjectedLaunch::new(
        program(&temp, "tools", "actingwatch.exe", true),
        LockProbe::File,
    );

    assert_eq!(run(&mut launch), 1);
    // It tried the fixed entry `<root>\runtime\actingctl.exe` once.
    let [(entry, _)] = launch.runs.as_slice() else {
        panic!("one run of the fixed entry, not {:?}", launch.runs);
    };
    assert!(
        entry.ends_with("runtime/actingctl.exe"),
        "{}",
        entry.display()
    );
    assert_eq!(
        entry
            .parent()
            .and_then(Path::parent)
            .map(|root| fs::canonicalize(root).expect("the root")),
        Some(fs::canonicalize(temp.path()).expect("the temporary root"))
    );
    let log = fs::read_to_string(temp.path().join("watchdog").join("watchdog.log"))
        .expect("the watchdog log");
    let lines = log.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "{log}");
    assert!(
        lines[0].starts_with("ERROR actingwatch: watchdog_launcher_spawn_failed ")
            && lines[0].contains(&entry.display().to_string()),
        "{log}"
    );
}
