// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl agent-channel` (Workflow #432; `contracts/agent-forward.md`): the channel registry's
//! writer, Claude Code's hook and self-registration, and the notice of the adapted source. The
//! grammar and the client words are this crate's; `actingctl` passes the words after `agent-channel`
//! and prints the answer.
//!
//! None of these needs the Runtime. They touch only `<state_root>\agent-forward\`, under one file
//! lock, and every file goes through `.part`, flush and rename.

use crate::channels::{CHANNELS_SCHEMA, Channel, ChannelClient, ChannelRegistry, registry_path};
use crate::claude;
use crate::dpapi::Secret;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The command's first word on `actingctl`'s command line.
pub const COMMAND: &str = "agent-channel";

const USAGE: &str = "usage: actingctl agent-channel add --state-root <path> --name <name> --client <claude|codex|dsh|antigravity> --native-id <id> [--dsh-app-root <path> --dsh-home <path> --dsh-node <path>]\nusage: actingctl agent-channel remove --state-root <path> --name <name>\nusage: actingctl agent-channel list --state-root <path>\nusage: actingctl agent-channel claude-hook <start|end> --state-root <path>\nusage: actingctl agent-channel claude-register --state-root <path> --name <name>\nusage: actingctl agent-channel notice";

const STATE_ROOT: &str = "--state-root";
const NAME: &str = "--name";
const CLIENT: &str = "--client";
const NATIVE_ID: &str = "--native-id";
const DSH_APP_ROOT: &str = "--dsh-app-root";
const DSH_HOME: &str = "--dsh-home";
const DSH_NODE: &str = "--dsh-node";

/// The file the writers of `<state_root>\agent-forward\` lock.
const LOCK_FILE: &str = ".lock";
/// How long a writer waits for another one.
const LOCK_WAIT: Duration = Duration::from_secs(5);
const LOCK_POLL: Duration = Duration::from_millis(50);

/// The MIT licence of the source this crate adapts, as the binaries carry it.
const NOTICE: &str = concat!(
    "ActingCommand's agent forwarding contains source code adapted from ",
    "HS7097/codex-goal-lifecycle@c9538cc5, under this licence:\n\n",
    include_str!("../LICENSE-codex-goal-lifecycle.txt"),
);

/// What a command answers when it succeeds.
#[derive(Debug, Clone, PartialEq)]
pub enum CliAnswer {
    /// The members of `actingctl`'s one result line.
    Members(Map<String, Value>),
    /// Text for a person, printed as it is.
    Text(&'static str),
    /// Nothing on stdout: Claude Code adds a SessionStart hook's stdout to the session's context.
    Silent,
}

/// What is wrong with a command-line word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageProblem {
    Unknown,
    Required,
    Repeated,
    ValueMissing,
    NotUtf8,
    NotRecognised,
}

/// How a command failed. No text here holds a credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliFailure {
    /// The command line itself; `usage` is the grammar.
    Usage {
        argument: Option<String>,
        problem: UsageProblem,
        usage: &'static str,
    },
    /// The command did not do its work.
    Failed { text: String },
    /// A registered session's hook did not do its work. Its one stderr line is all the session
    /// shows, so nothing goes to stdout.
    Hook { text: String },
}

fn usage(argument: Option<&str>, problem: UsageProblem) -> CliFailure {
    CliFailure::Usage {
        argument: argument.map(str::to_owned),
        problem,
        usage: USAGE,
    }
}

fn utf8(word: &OsStr) -> Result<&str, CliFailure> {
    word.to_str()
        .ok_or_else(|| usage(None, UsageProblem::NotUtf8))
}

/// The flags of one command: each at most once, each with a value.
fn flags(
    words: &[OsString],
    allowed: &[&'static str],
) -> Result<BTreeMap<&'static str, String>, CliFailure> {
    let mut values = BTreeMap::new();
    let mut words = words.iter();
    while let Some(word) = words.next() {
        let word = utf8(word)?;
        let Some(flag) = allowed.iter().find(|flag| **flag == word) else {
            return Err(usage(Some(word), UsageProblem::Unknown));
        };
        let Some(value) = words.next() else {
            return Err(usage(Some(word), UsageProblem::ValueMissing));
        };
        if values.insert(*flag, utf8(value)?.to_owned()).is_some() {
            return Err(usage(Some(word), UsageProblem::Repeated));
        }
    }
    Ok(values)
}

fn required<'a>(
    values: &'a BTreeMap<&'static str, String>,
    flag: &'static str,
) -> Result<&'a str, CliFailure> {
    values
        .get(flag)
        .map(String::as_str)
        .ok_or_else(|| usage(Some(flag), UsageProblem::Required))
}

fn failed(text: String) -> CliFailure {
    CliFailure::Failed { text }
}

fn now_unix_ms() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .ok_or_else(|| "the system clock is unavailable".to_owned())
}

/// Runs `work` holding the lock of `<state_root>\agent-forward\`. The operating system releases the
/// lock if this process dies.
pub(crate) fn locked<T>(
    state_root: &Path,
    work: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let directory = state_root.join("agent-forward");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let path = directory.join(LOCK_FILE);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) if started.elapsed() < LOCK_WAIT => {
                std::thread::sleep(LOCK_POLL);
            }
            Err(TryLockError::WouldBlock) => {
                return Err(format!("another writer holds {}", path.display()));
            }
            Err(TryLockError::Error(error)) => {
                return Err(format!("cannot lock {}: {error}", path.display()));
            }
        }
    }
    work()
}

/// Replaces `path` whole: its `.part` beside it, flushed, then renamed.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)
            .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    }
    let write = || -> std::io::Result<()> {
        let mut file = File::create(&part)?;
        file.write_all(bytes)?;
        file.sync_all()
    };
    write().map_err(|error| format!("cannot write {}: {error}", part.display()))?;
    fs::rename(&part, path).map_err(|error| format!("cannot replace {}: {error}", path.display()))
}

fn client_word(client: ChannelClient) -> &'static str {
    match client {
        ChannelClient::Claude => "claude",
        ChannelClient::Codex => "codex",
        ChannelClient::Dsh => "dsh",
        ChannelClient::Antigravity => "antigravity",
    }
}

fn channel_value(channel: &Channel) -> Value {
    let mut value = json!({
        "name": channel.name,
        "client": client_word(channel.client),
        "native_id": channel.native_id,
    });
    if let Some(paths) = &channel.dsh {
        value["app_root"] = json!(paths.app_root);
        value["home_root"] = json!(paths.home_root);
        value["node_executable"] = json!(paths.node_executable);
    }
    value
}

fn read_channels(state_root: &Path) -> Result<Vec<Channel>, String> {
    match crate::channels::read_registry(state_root) {
        Ok(Some(registry)) => Ok(registry.channels),
        Ok(None) => Ok(Vec::new()),
        Err(failure) => Err(format!("the channel registry is unreadable: {failure:?}")),
    }
}

/// Writes the registry after the same check its reader makes, so no command can leave a registry
/// that later reads as invalid. The caller holds the lock.
fn write_channels(state_root: &Path, channels: &[Value]) -> Result<(), String> {
    let text = serde_json::to_string_pretty(&json!({
        "schema": CHANNELS_SCHEMA,
        "channels": channels,
    }))
    .map_err(|error| format!("cannot encode the registry: {error}"))?;
    ChannelRegistry::parse(&text).map_err(|invalid| {
        format!(
            "the channel is refused: {:?} ({})",
            invalid.problem,
            invalid.channel.as_deref().unwrap_or("no name")
        )
    })?;
    write_atomic(&registry_path(state_root), text.as_bytes())
}

/// The three paths of a DSH channel, as the command line gives them.
pub(crate) struct DshFlags<'a> {
    pub(crate) app_root: Option<&'a str>,
    pub(crate) home_root: Option<&'a str>,
    pub(crate) node_executable: Option<&'a str>,
}

/// Adds one channel to the registry. A Claude channel's id is a session's UUID, kept in lower case.
pub(crate) fn add_channel(
    state_root: &Path,
    name: &str,
    client: &str,
    native_id: &str,
    dsh: Option<DshFlags<'_>>,
) -> Result<Value, String> {
    let native_id = if client == client_word(ChannelClient::Claude) {
        claude::session_id(native_id)
            .ok_or_else(|| "a Claude channel's native id is a session's UUID".to_owned())?
    } else {
        native_id.to_owned()
    };
    let mut entry = json!({"name": name, "client": client, "native_id": native_id});
    if let Some(dsh) = dsh {
        for (member, value) in [
            ("app_root", dsh.app_root),
            ("home_root", dsh.home_root),
            ("node_executable", dsh.node_executable),
        ] {
            if let Some(value) = value {
                entry[member] = json!(value);
            }
        }
    }
    locked(state_root, || {
        let mut channels: Vec<Value> = read_channels(state_root)?
            .iter()
            .map(channel_value)
            .collect();
        channels.push(entry.clone());
        write_channels(state_root, &channels)
    })?;
    Ok(entry)
}

/// Removes one channel. A Claude channel's live record loses its token: it has no more use.
fn remove_channel(state_root: &Path, name: &str) -> Result<(), String> {
    let removed = locked(state_root, || {
        let channels = read_channels(state_root)?;
        let Some(removed) = channels
            .iter()
            .find(|channel| channel.name == name)
            .cloned()
        else {
            return Err("no channel has this name".to_owned());
        };
        let rest: Vec<Value> = channels
            .iter()
            .filter(|channel| channel.name != name)
            .map(channel_value)
            .collect();
        write_channels(state_root, &rest)?;
        Ok(removed)
    })?;
    if removed.client == ChannelClient::Claude {
        claude::clear_removed(state_root, &removed.native_id, now_unix_ms()?)?;
    }
    Ok(())
}

/// This session's own variables. Only a registered channel's session gets as far as this.
fn session_environment() -> claude::SessionEnvironment {
    let text = |name: &str| std::env::var_os(name).and_then(|value| value.into_string().ok());
    claude::SessionEnvironment {
        socket: text(claude::SOCKET_VARIABLE),
        // A token that is not text becomes an empty one, which the check refuses.
        token: std::env::var_os(claude::TOKEN_VARIABLE).map(|value| {
            Secret::new(
                value
                    .into_string()
                    .map(String::into_bytes)
                    .unwrap_or_default(),
            )
        }),
        session_id: text(claude::SESSION_VARIABLE),
    }
}

fn hook(words: &[OsString]) -> Result<CliAnswer, CliFailure> {
    let Some((action, rest)) = words.split_first() else {
        return Err(usage(Some("claude-hook"), UsageProblem::Required));
    };
    let action = utf8(action)?;
    let start = match action {
        "start" => true,
        "end" => false,
        other => return Err(usage(Some(other), UsageProblem::NotRecognised)),
    };
    let values = flags(rest, &[STATE_ROOT])?;
    let state_root = PathBuf::from(required(&values, STATE_ROOT)?);
    let result = now_unix_ms().and_then(|now| {
        if start {
            claude::hook_start(
                &state_root,
                std::io::stdin().lock(),
                &session_environment,
                &crate::ancestor::claude_ancestor,
                now,
            )
        } else {
            claude::hook_end(
                &state_root,
                std::io::stdin().lock(),
                &crate::ancestor::claude_ancestor,
                now,
            )
        }
    });
    result
        .map(|_| CliAnswer::Silent)
        .map_err(|text| CliFailure::Hook { text })
}

/// Runs the words after `agent-channel`.
pub fn run(words: &[OsString]) -> Result<CliAnswer, CliFailure> {
    let Some((action, rest)) = words.split_first() else {
        return Err(usage(None, UsageProblem::Required));
    };
    let action = utf8(action)?;
    match action {
        "add" => {
            let values = flags(
                rest,
                &[
                    STATE_ROOT,
                    NAME,
                    CLIENT,
                    NATIVE_ID,
                    DSH_APP_ROOT,
                    DSH_HOME,
                    DSH_NODE,
                ],
            )?;
            let flag = |name: &'static str| values.get(name).map(String::as_str);
            let dsh = DshFlags {
                app_root: flag(DSH_APP_ROOT),
                home_root: flag(DSH_HOME),
                node_executable: flag(DSH_NODE),
            };
            let channel = add_channel(
                Path::new(required(&values, STATE_ROOT)?),
                required(&values, NAME)?,
                required(&values, CLIENT)?,
                required(&values, NATIVE_ID)?,
                Some(dsh),
            )
            .map_err(failed)?;
            let mut members = Map::new();
            members.insert("channel".to_owned(), channel);
            Ok(CliAnswer::Members(members))
        }
        "remove" => {
            let values = flags(rest, &[STATE_ROOT, NAME])?;
            let name = required(&values, NAME)?;
            remove_channel(Path::new(required(&values, STATE_ROOT)?), name).map_err(failed)?;
            let mut members = Map::new();
            members.insert("removed".to_owned(), json!(name));
            Ok(CliAnswer::Members(members))
        }
        "list" => {
            let values = flags(rest, &[STATE_ROOT])?;
            let channels = read_channels(Path::new(required(&values, STATE_ROOT)?))
                .map_err(failed)?
                .iter()
                .map(channel_value)
                .collect();
            let mut members = Map::new();
            members.insert("channels".to_owned(), Value::Array(channels));
            Ok(CliAnswer::Members(members))
        }
        "claude-hook" => hook(rest),
        "claude-register" => {
            let values = flags(rest, &[STATE_ROOT, NAME])?;
            let state_root = PathBuf::from(required(&values, STATE_ROOT)?);
            let name = required(&values, NAME)?;
            now_unix_ms()
                .and_then(|now| {
                    claude::register(
                        &state_root,
                        name,
                        session_environment(),
                        &crate::ancestor::claude_ancestor,
                        now,
                    )
                })
                .map_err(failed)?;
            let mut members = Map::new();
            members.insert("registered".to_owned(), json!(name));
            Ok(CliAnswer::Members(members))
        }
        "notice" => {
            flags(rest, &[])?;
            Ok(CliAnswer::Text(NOTICE))
        }
        other => Err(usage(Some(other), UsageProblem::Unknown)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notice_prints_the_mit_text() {
        let answer = run(&[OsString::from("notice")]).expect("the notice");
        let CliAnswer::Text(text) = answer else {
            panic!("the notice is text: {answer:?}");
        };
        for line in [
            "HS7097/codex-goal-lifecycle@c9538cc5",
            "MIT License",
            "Copyright (c) 2026 HS7097",
            "Permission is hereby granted, free of charge",
            "The above copyright notice and this permission notice shall be included in all",
            "THE SOFTWARE IS PROVIDED \"AS IS\"",
        ] {
            assert!(text.contains(line), "{line}");
        }
    }
}
