// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #382 A-1 (test plan v2.2 §3.7, G13; apps 27): actingwatch, the program the
// watchdog's task runs (contracts/runtime-watchdog.md "Launcher"; rd-1007/v0113/model-374.md
// §2.1). While acsetup holds the installation writer lock it exits 0 and runs nothing. A program
// that is not `<root>\tools\actingwatch.exe`, a root without the watchdog directory and a writer
// lock it cannot probe each end the launch with exit 1: model v2.2 §7.5 and §7.6 replaced the
// launcher's 20-23 with registered codes and the exit of their category, error.
//
// Workflow #434 LAB-4: the launch runs the tick in its own process instead of a fixed entry, so
// the injected launch keeps each tick it was asked to run where it kept each command; the gate
// of a fixed entry that cannot run left with the fixed entry (`ci/retired-gates.toml`), and
// `actingwatch_tick_in_process` runs a whole tick in the program.

use super::{Launch, WriterLock, probe_writer_lock, run};
use std::fs::{self, File};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// How the injected launch answers the writer lock probe.
#[derive(Clone, Copy)]
enum LockProbe {
    /// The launcher's own probe of the lock file.
    File,
    /// The OS refuses the probe.
    Refused,
}

/// A launch of the program at `exe`; it keeps the root of each tick it was asked to run and
/// runs none.
struct InjectedLaunch {
    exe: PathBuf,
    lock: LockProbe,
    runs: Vec<PathBuf>,
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

    fn tick(&mut self, root: &Path, _log: &Path) -> i32 {
        self.runs.push(root.to_path_buf());
        0
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
