// SPDX-License-Identifier: AGPL-3.0-only

//! W3: the per-user scheduled task that runs `<root>\tools\actingwatch.exe` every minute.
//! It is registered, checked and removed through `schtasks.exe`: actingctl forbids unsafe
//! code, so COM's task service is out of reach. The definition is written as UTF-16LE XML
//! with a BOM, the form `schtasks /Query /XML` declares (`contracts/runtime-watchdog.md`).

use super::Failure;
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use super::powershell::CREATE_NO_WINDOW;

pub(crate) const INTERVAL: &str = "PT1M";
pub(crate) const LOGON_TYPE: &str = "InteractiveToken";
pub(crate) const RUN_LEVEL: &str = "LeastPrivilege";
pub(crate) const TASK_XML: &str = "task.xml";

/// `ActingCommand Runtime watchdog <fp12>` in the root folder `\`: the first 12 hex digits of
/// SHA-256 over the plain install root, lowercased.
pub(crate) fn task_name(root_plain: &Path) -> String {
    let digest = Sha256::digest(root_plain.to_string_lossy().to_lowercase().as_bytes());
    let fingerprint = digest
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("ActingCommand Runtime watchdog {fingerprint}")
}

pub(crate) fn launcher(root_plain: &Path) -> PathBuf {
    root_plain.join("tools").join("actingwatch.exe")
}

fn system32(program: &str) -> PathBuf {
    let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    PathBuf::from(system_root).join("System32").join(program)
}

fn run(program: &str, arguments: &[&OsStr]) -> Result<Output, String> {
    Command::new(system32(program))
        .args(arguments)
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| format!("{program} did not run: {error}"))
}

/// What a tool printed, stdout then stderr, as one trimmed text.
fn printed(output: &Output) -> String {
    format!(
        "{} {}",
        decode(&output.stdout).trim(),
        decode(&output.stderr).trim()
    )
    .trim()
    .to_owned()
}

/// `schtasks` writes in the console code page when redirected, UTF-16 when asked for it;
/// the watchdog's own paths are compared after this decoding.
fn decode(bytes: &[u8]) -> String {
    let utf16 =
        bytes.starts_with(&[0xff, 0xfe]) || bytes.iter().skip(1).step_by(2).any(|b| *b == 0);
    if utf16 && bytes.len().is_multiple_of(2) {
        let units = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&units)
            .trim_start_matches('\u{feff}')
            .to_owned()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// The current user's SID, the task's principal.
pub(crate) fn current_user_sid() -> Result<String, Failure> {
    let output = run(
        "whoami.exe",
        &["/user", "/fo", "csv", "/nh"].map(OsStr::new),
    )
    .map_err(|detail| Failure::misconfigured("watchdog_user_unavailable", detail))?;
    let text = decode(&output.stdout);
    let sid = csv_fields(text.lines().next().unwrap_or_default())
        .pop()
        .filter(|sid| sid.starts_with("S-1-"));
    match (output.status.success(), sid) {
        (true, Some(sid)) => Ok(sid),
        _ => Err(Failure::misconfigured(
            "watchdog_user_unavailable",
            format!("whoami /user printed no SID: {}", printed(&output)),
        )),
    }
}

/// One CSV line of a Windows tool: quoted fields, `""` inside quotes.
fn csv_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut characters = line.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '"' if quoted && characters.peek() == Some(&'"') => {
                field.push('"');
                characters.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut field)),
            other => field.push(other),
        }
    }
    fields.push(field);
    fields
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The task definition (`contracts/runtime-watchdog.md`, "Task"): a time trigger from
/// `start_boundary` repeating every minute without end, the user's interactive token at the
/// least privilege, no new instance while one runs, five minutes at most, no battery stops,
/// no restart on failure, visible in Task Scheduler.
pub(crate) fn definition_xml(root_plain: &Path, user_sid: &str, start_boundary: &str) -> String {
    let root = escape(&root_plain.display().to_string());
    let command = escape(&launcher(root_plain).display().to_string());
    let user = escape(user_sid);
    let start = escape(start_boundary);
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>ActingCommand Runtime watchdog of {root}: every minute tools\actingwatch.exe runs actingctl watchdog run-once (Workflow #374). Remove it with actingctl watchdog uninstall --root {root}.</Description>
  </RegistrationInfo>
  <Triggers>
    <TimeTrigger>
      <Repetition>
        <Interval>{INTERVAL}</Interval>
        <StopAtDurationEnd>false</StopAtDurationEnd>
      </Repetition>
      <StartBoundary>{start}</StartBoundary>
      <Enabled>true</Enabled>
    </TimeTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>{LOGON_TYPE}</LogonType>
      <RunLevel>{RUN_LEVEL}</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT5M</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>
      <WorkingDirectory>{root}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#
    )
}

/// UTF-16LE with a byte order mark, matching the declared `encoding="UTF-16"`.
pub(crate) fn utf16le_with_bom(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xff, 0xfe];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    bytes
}

pub(crate) fn register(name: &str, xml: &Path) -> Result<(), Failure> {
    let output = run(
        "schtasks.exe",
        &[
            OsStr::new("/Create"),
            OsStr::new("/TN"),
            OsStr::new(name),
            OsStr::new("/XML"),
            xml.as_os_str(),
            OsStr::new("/F"),
        ],
    )
    .map_err(|detail| Failure::misconfigured("watchdog_task_register_failed", detail))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Failure::misconfigured(
            "watchdog_task_register_failed",
            format!("schtasks /Create {}: {}", output.status, printed(&output)),
        ))
    }
}

pub(crate) fn delete(name: &str) -> Result<(), Failure> {
    let output = run(
        "schtasks.exe",
        &["/Delete", "/TN", name, "/F"].map(OsStr::new),
    )
    .map_err(|detail| Failure::misconfigured("watchdog_task_delete_failed", detail))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Failure::misconfigured(
            "watchdog_task_delete_failed",
            format!("schtasks /Delete {}: {}", output.status, printed(&output)),
        ))
    }
}

/// The parts of a registered definition the watchdog depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Definition {
    pub(crate) command: Option<String>,
    pub(crate) interval: Option<String>,
    pub(crate) logon_type: Option<String>,
    pub(crate) run_level: Option<String>,
    /// No `<Enabled>false</Enabled>` in the task or its trigger.
    pub(crate) enabled: bool,
}

pub(crate) enum Registration {
    /// `schtasks /Query` found no task of that name, or could not read it; its text.
    Absent(String),
    Present(Definition),
}

pub(crate) fn query(name: &str) -> Result<Registration, Failure> {
    let output = run(
        "schtasks.exe",
        &["/Query", "/TN", name, "/XML"].map(OsStr::new),
    )
    .map_err(|detail| Failure::misconfigured("watchdog_task_query_failed", detail))?;
    if !output.status.success() {
        return Ok(Registration::Absent(printed(&output)));
    }
    Ok(Registration::Present(parse_definition(&decode(
        &output.stdout,
    ))))
}

pub(crate) fn parse_definition(xml: &str) -> Definition {
    Definition {
        command: element(xml, "Command"),
        interval: element(xml, "Interval"),
        logon_type: element(xml, "LogonType"),
        run_level: element(xml, "RunLevel"),
        enabled: !xml.contains("<Enabled>false</Enabled>"),
    }
}

fn element(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = start + xml[start..].find(&format!("</{tag}>"))?;
    Some(unescape(xml[start..end].trim()))
}

/// What differs from the definition `install` writes; empty when it matches.
pub(crate) fn mismatches(definition: &Definition, root_plain: &Path) -> Vec<String> {
    let expected = launcher(root_plain).display().to_string();
    let mut differences = Vec::new();
    if !definition
        .command
        .as_deref()
        .is_some_and(|command| command.eq_ignore_ascii_case(&expected))
    {
        differences.push(format!(
            "command {:?}, expected {expected:?}",
            definition.command
        ));
    }
    for (name, value, expected) in [
        ("interval", &definition.interval, INTERVAL),
        ("logon_type", &definition.logon_type, LOGON_TYPE),
    ] {
        if value.as_deref() != Some(expected) {
            differences.push(format!("{name} {value:?}, expected {expected:?}"));
        }
    }
    // Task Scheduler omits the default run level, which is the least privilege.
    if definition
        .run_level
        .as_deref()
        .is_some_and(|level| level != RUN_LEVEL)
    {
        differences.push(format!(
            "run_level {:?}, expected {RUN_LEVEL:?}",
            definition.run_level
        ));
    }
    if !definition.enabled {
        differences.push("disabled".to_owned());
    }
    differences
}

/// Status, last run time and last result from `schtasks /Query /FO CSV /V /NH`, by column
/// (4, 6 and 7), raw: Task Scheduler localises them.
pub(crate) fn run_state(name: &str) -> Option<(String, String, String)> {
    let output = run(
        "schtasks.exe",
        &["/Query", "/TN", name, "/FO", "CSV", "/V", "/NH"].map(OsStr::new),
    )
    .ok()
    .filter(|output| output.status.success())?;
    let text = decode(&output.stdout);
    let fields = csv_fields(text.lines().find(|line| !line.trim().is_empty())?);
    Some((
        fields.get(3)?.clone(),
        fields.get(5)?.clone(),
        fields.get(6)?.clone(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_is_utf16_xml_with_every_setting_escaped() {
        let root = Path::new(r"D:\A&B <Install>");
        let xml = definition_xml(root, "S-1-5-21-1-2-3-1001", "2026-10-07T06:50:13Z");
        for setting in [
            r#"<?xml version="1.0" encoding="UTF-16"?>"#,
            "<Interval>PT1M</Interval>",
            "<StopAtDurationEnd>false</StopAtDurationEnd>",
            "<StartBoundary>2026-10-07T06:50:13Z</StartBoundary>",
            "<UserId>S-1-5-21-1-2-3-1001</UserId>",
            "<LogonType>InteractiveToken</LogonType>",
            "<RunLevel>LeastPrivilege</RunLevel>",
            "<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>",
            "<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>",
            "<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>",
            "<AllowHardTerminate>true</AllowHardTerminate>",
            "<StartWhenAvailable>false</StartWhenAvailable>",
            "<Hidden>false</Hidden>",
            "<ExecutionTimeLimit>PT5M</ExecutionTimeLimit>",
            "<Priority>7</Priority>",
            r"<Command>D:\A&amp;B &lt;Install&gt;\tools\actingwatch.exe</Command>",
            r"<WorkingDirectory>D:\A&amp;B &lt;Install&gt;</WorkingDirectory>",
        ] {
            assert!(xml.contains(setting), "missing {setting}");
        }
        assert!(!xml.contains("<Duration>"));
        assert!(!xml.contains("RestartOnFailure"));
        assert!(!xml.contains("<Arguments>"));
        let bytes = utf16le_with_bom(&xml);
        assert_eq!(&bytes[..4], &[0xff, 0xfe, b'<', 0]);
        assert_eq!(decode(&bytes), xml);
        // What Task Scheduler reads back is what install wrote.
        let definition = parse_definition(&xml);
        assert_eq!(
            definition.command.as_deref(),
            Some(r"D:\A&B <Install>\tools\actingwatch.exe")
        );
        assert!(mismatches(&definition, root).is_empty());
        let disabled = parse_definition(&xml.replacen(
            "<Enabled>true</Enabled>",
            "<Enabled>false</Enabled>",
            1,
        ));
        assert_eq!(mismatches(&disabled, root), ["disabled"]);
    }

    #[test]
    fn names_csv_and_console_output_are_read_as_windows_prints_them() {
        assert_eq!(
            task_name(Path::new(r"F:\AC")),
            task_name(Path::new(r"f:\ac"))
        );
        assert!(task_name(Path::new(r"F:\AC")).starts_with("ActingCommand Runtime watchdog "));
        assert_eq!(task_name(Path::new(r"F:\AC")).len(), 31 + 12);
        assert_eq!(
            csv_fields(
                r#""HOST","\Task, one","N/A","Ready","Interactive only","2026/10/7 6:50:13","0""#
            ),
            [
                "HOST",
                r"\Task, one",
                "N/A",
                "Ready",
                "Interactive only",
                "2026/10/7 6:50:13",
                "0"
            ]
        );
        assert_eq!(csv_fields(r#""a ""b""","c""#), [r#"a "b""#, "c"]);
        assert_eq!(decode(b"<Task>\r\r\n</Task>"), "<Task>\r\r\n</Task>");
    }
}
