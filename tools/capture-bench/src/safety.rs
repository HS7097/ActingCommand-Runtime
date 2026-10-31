// SPDX-License-Identifier: AGPL-3.0-only

//! §3.10: the production-isolation start check and §3.3.4: the single bench lock. Every item
//! that fails refuses the run (`refused_start`, exit 13) and says which.

use crate::plan::{Plan, PriorityClass};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

/// The single bench lock's fixed path (§3.3.4, written at build time; no plan changes it).
pub const BENCH_LOCK_PATH: &str = r"D:\.Claude\exp402\bench.lock";
/// The fixed runs root (§3.3.4, §3.10 item 8).
pub const RUNS_ROOT: &str = r"D:\.Claude\exp402\runs";

/// The lock path and the runs root. Production uses [`Roots::fixed`]; the tests put both under
/// their own temporary root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots {
    pub lock_path: PathBuf,
    pub runs_root: PathBuf,
}

impl Roots {
    pub fn fixed() -> Self {
        Self {
            lock_path: PathBuf::from(BENCH_LOCK_PATH),
            runs_root: PathBuf::from(RUNS_ROOT),
        }
    }

    /// Whether `run_dir` is a directory directly or deeper under the runs root, without `..`.
    pub fn contains_run_dir(&self, run_dir: &Path) -> bool {
        run_dir.is_absolute()
            && !run_dir
                .components()
                .any(|component| matches!(component, Component::ParentDir))
            && run_dir.starts_with(&self.runs_root)
            && run_dir != self.runs_root
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        text.push(char::from(HEX[usize::from(byte >> 4)]));
        text.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    text
}

/// The bench lock (§3.3.4): opened for writing with read-only sharing for the whole process, so
/// a second bench meets a sharing violation and refuses; the system closes the handle when the
/// process ends, forced or not.
pub struct BenchLock {
    file: File,
}

impl BenchLock {
    pub fn acquire(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("bench_lock: create {}: {error}", parent.display()))?;
        }
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(false);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // FILE_SHARE_READ only: others may read the identity, nobody may open it for writing.
            options.share_mode(0x0000_0001);
        }
        let file = options
            .open(path)
            .map_err(|error| format!("bench_lock_held: {}: {error}", path.display()))?;
        #[cfg(not(windows))]
        {
            // Without Windows share modes the lock is advisory only (the bench runs on Windows).
        }
        Ok(Self { file })
    }

    /// Writes the identity the forced stop checks (§3.11.5).
    pub fn write_identity(&self, identity: &Value) -> Result<(), String> {
        let text = serde_json::to_vec_pretty(identity)
            .map_err(|error| format!("bench_lock: encode the identity: {error}"))?;
        self.file
            .set_len(0)
            .map_err(|error| format!("bench_lock: truncate: {error}"))?;
        let mut file = &self.file;
        file.write_all(&text)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("bench_lock: write the identity: {error}"))
    }
}

/// `{pid, process creation time, run_id, run directory, plan SHA-256, executable SHA-256, start
/// time}` (§3.3.4).
pub fn process_identity(run_dir: &Path, run_id: &str, plan_sha256: &str) -> Value {
    let executable = std::env::current_exe().ok();
    let executable_sha256 = executable
        .as_ref()
        .and_then(|path| std::fs::read(path).ok())
        .map(|bytes| sha256_hex(&bytes));
    json!({
        "pid": std::process::id(),
        "process_created_unix_ms": crate::winproc::own_creation_unix_ms(),
        "run_id": run_id,
        "run_dir": run_dir.display().to_string(),
        "plan_sha256": plan_sha256,
        "executable": executable.map(|path| path.display().to_string()),
        "executable_sha256": executable_sha256,
        "started_unix_ms": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as u64),
    })
}

/// The host-side items of the start check.
pub trait StartChecks: Send {
    fn check(&mut self, plan: &Plan, run_dir: &Path) -> Result<(), String>;
    /// What the check read, for `header.json`.
    fn facts(&self) -> Value;
}

/// The production start check: the adb version, the process priority class read back, `TEMP`
/// and `TMP` under the run directory.
#[derive(Default)]
pub struct HostStartChecks {
    facts: Value,
}

#[derive(Debug, Clone, Serialize)]
struct AdbVersion {
    path: String,
    line: Option<String>,
}

impl StartChecks for HostStartChecks {
    fn check(&mut self, plan: &Plan, run_dir: &Path) -> Result<(), String> {
        let mut facts = serde_json::Map::new();
        // §3.10 item 1: exactly the Runtime's adb, by its version line. `adb version` reads the
        // client only.
        let adb = &plan.safety.adb_path;
        if !Path::new(adb).is_absolute() {
            return Err(format!("adb_path is not absolute: {adb}"));
        }
        let output = std::process::Command::new(adb)
            .arg("version")
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|error| format!("adb version did not run: {error}"))?;
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        let line = text
            .lines()
            .find_map(|line| line.trim().strip_prefix("Version ").map(str::to_owned));
        facts.insert(
            "adb".to_owned(),
            json!(AdbVersion {
                path: adb.clone(),
                line: line.clone(),
            }),
        );
        if line.as_deref() != Some(plan.safety.adb_version.as_str()) {
            return Err(format!(
                "adb_version_mismatch: {adb} reports {line:?}, the plan requires {}",
                plan.safety.adb_version
            ));
        }
        // §3.10 item 4: TEMP and TMP under the run directory.
        let temp = run_dir.join("temp");
        for variable in ["TEMP", "TMP"] {
            let value = std::env::var_os(variable).map(PathBuf::from);
            facts.insert(
                variable.to_owned(),
                json!(value.as_ref().map(|path| path.display().to_string())),
            );
            if value.as_deref() != Some(temp.as_path()) {
                return Err(format!(
                    "temp_not_in_run_dir: {variable} must be {}",
                    temp.display()
                ));
            }
        }
        std::fs::create_dir_all(&temp)
            .map_err(|error| format!("create {}: {error}", temp.display()))?;
        // §3.10 item 5: the production actingd's priority class, set and read back.
        let wanted = plan.safety.priority_class;
        crate::winproc::set_own_priority(wanted)?;
        let read = crate::winproc::own_priority()?;
        facts.insert("priority_class".to_owned(), json!(read));
        if read != wanted {
            return Err(format!(
                "priority_mismatch: set {wanted:?}, read back {read:?}"
            ));
        }
        // §3.10 item 1: the scrcpy server this build embeds matches its pin (else S is not
        // ready, which the run records; it does not refuse the start).
        facts.insert(
            "scrcpy_server".to_owned(),
            json!({
                "version": actingcommand_device::SCRCPY_SERVER_VERSION,
                "sha256": actingcommand_device::SCRCPY_SERVER_SHA256,
            }),
        );
        self.facts = Value::Object(facts);
        Ok(())
    }

    fn facts(&self) -> Value {
        self.facts.clone()
    }
}

/// The test start check: accepts and records nothing.
pub struct AcceptStartChecks;

impl StartChecks for AcceptStartChecks {
    fn check(&mut self, _plan: &Plan, _run_dir: &Path) -> Result<(), String> {
        Ok(())
    }

    fn facts(&self) -> Value {
        json!({ "start_checks": "fake" })
    }
}

/// A test start check that refuses with `reason`.
pub struct RefuseStartChecks(pub String);

impl StartChecks for RefuseStartChecks {
    fn check(&mut self, _plan: &Plan, _run_dir: &Path) -> Result<(), String> {
        Err(self.0.clone())
    }

    fn facts(&self) -> Value {
        json!({ "start_checks": "fake-refuse" })
    }
}

/// The priority class names (for the plan's value).
pub fn priority_name(class: PriorityClass) -> &'static str {
    match class {
        PriorityClass::Idle => "idle",
        PriorityClass::BelowNormal => "below_normal",
        PriorityClass::Normal => "normal",
        PriorityClass::AboveNormal => "above_normal",
    }
}
