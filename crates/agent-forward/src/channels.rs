// SPDX-License-Identifier: AGPL-3.0-only

//! The channel registry (Workflow #432; `contracts/agent-wake.md`): the registered channels'
//! names and addresses, `<state_root>\agent-forward\channels.json`. It is program state and not
//! configuration, and it holds no secret. AGT-1a gives its shape, its validation and its read; the
//! command that writes it comes with the first transport. AGT-T1b adds a Claude Code channel's
//! `restore` switch.

use serde::Deserialize;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// The `schema` the registry file carries.
pub const CHANNELS_SCHEMA: &str = "actingcommand.agent-channels.v1";
/// The longest channel name, in Unicode scalars. A name goes into every message's first line.
pub const MAX_CHANNEL_CHARS: usize = 16;
/// The longest native id, in bytes.
pub const MAX_NATIVE_ID_BYTES: usize = 256;
/// The most channels.
pub const MAX_CHANNELS: usize = 256;

/// The client a channel lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelClient {
    Claude,
    Codex,
    Dsh,
    Antigravity,
}

/// The three paths a DSH channel needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DshPaths {
    pub app_root: String,
    pub home_root: String,
    pub node_executable: String,
}

/// One registered channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub name: String,
    pub client: ChannelClient,
    pub native_id: String,
    /// `Some` exactly for a DSH channel.
    pub dsh: Option<DshPaths>,
    /// Whether a stopped session of the channel may be restored before a send (AGT-T1b): a Claude
    /// Code channel's switch, on unless its line says `false`. Always false for another client.
    pub restore: bool,
}

/// The registry, checked whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelRegistry {
    pub channels: Vec<Channel>,
}

/// Why a registry is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryProblem {
    /// Not JSON of the registry's shape, or another `schema`.
    Shape,
    /// More than [`MAX_CHANNELS`] channels.
    TooManyChannels,
    /// A name that is empty, longer than [`MAX_CHANNEL_CHARS`] or holds a control character.
    NameInvalid,
    /// Two channels with one name.
    NameRepeated,
    /// A native id that is empty, too long or holds a control character.
    NativeIdInvalid,
    /// A DSH channel without its three absolute paths, or another client's channel with any.
    PathsInvalid,
    /// `restore` on a channel of another client than Claude Code.
    RestoreInvalid,
}

/// A registry refused, with the channel it stopped at when there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryInvalid {
    pub problem: RegistryProblem,
    pub channel: Option<String>,
}

/// Why a registry file could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryReadFailure {
    /// The file exists and cannot be read.
    Read(ErrorKind),
    /// The file holds no valid registry.
    Invalid(RegistryInvalid),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChannelFile {
    name: String,
    client: ChannelClient,
    native_id: String,
    #[serde(default)]
    app_root: Option<String>,
    #[serde(default)]
    home_root: Option<String>,
    #[serde(default)]
    node_executable: Option<String>,
    #[serde(default)]
    restore: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryFile {
    schema: String,
    channels: Vec<ChannelFile>,
}

/// An absolute path as Windows reads it, on any build host: a drive path or a UNC path, with no
/// control character.
fn absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    let drive = bytes.len() > 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    (drive || path.starts_with(r"\\")) && !path.chars().any(char::is_control)
}

fn name_valid(name: &str) -> bool {
    (1..=MAX_CHANNEL_CHARS).contains(&name.chars().count())
        && !name.trim().is_empty()
        && !name.chars().any(char::is_control)
}

fn channel(file: ChannelFile) -> Result<Channel, RegistryInvalid> {
    let refused = |problem| RegistryInvalid {
        problem,
        channel: Some(file.name.clone()),
    };
    if !name_valid(&file.name) {
        return Err(RegistryInvalid {
            problem: RegistryProblem::NameInvalid,
            channel: None,
        });
    }
    if file.native_id.is_empty()
        || file.native_id.len() > MAX_NATIVE_ID_BYTES
        || file.native_id.chars().any(char::is_control)
    {
        return Err(refused(RegistryProblem::NativeIdInvalid));
    }
    let dsh = match (
        file.client,
        file.app_root.as_deref(),
        file.home_root.as_deref(),
        file.node_executable.as_deref(),
    ) {
        (ChannelClient::Dsh, Some(app_root), Some(home_root), Some(node_executable))
            if absolute(app_root) && absolute(home_root) && absolute(node_executable) =>
        {
            Some(DshPaths {
                app_root: app_root.to_owned(),
                home_root: home_root.to_owned(),
                node_executable: node_executable.to_owned(),
            })
        }
        (ChannelClient::Dsh, _, _, _)
        | (_, Some(_), _, _)
        | (_, _, Some(_), _)
        | (_, _, _, Some(_)) => {
            return Err(refused(RegistryProblem::PathsInvalid));
        }
        (_, None, None, None) => None,
    };
    let restore = match (file.client, file.restore) {
        (ChannelClient::Claude, restore) => restore.unwrap_or(true),
        (_, None) => false,
        (_, Some(_)) => return Err(refused(RegistryProblem::RestoreInvalid)),
    };
    Ok(Channel {
        name: file.name,
        client: file.client,
        native_id: file.native_id,
        dsh,
        restore,
    })
}

impl ChannelRegistry {
    /// The registry from the text of its file, checked whole.
    pub fn parse(text: &str) -> Result<Self, RegistryInvalid> {
        let shape = || RegistryInvalid {
            problem: RegistryProblem::Shape,
            channel: None,
        };
        let file = serde_json::from_str::<RegistryFile>(text).map_err(|_| shape())?;
        if file.schema != CHANNELS_SCHEMA {
            return Err(shape());
        }
        if file.channels.len() > MAX_CHANNELS {
            return Err(RegistryInvalid {
                problem: RegistryProblem::TooManyChannels,
                channel: None,
            });
        }
        let mut channels = Vec::with_capacity(file.channels.len());
        for entry in file.channels {
            let parsed = channel(entry)?;
            if channels
                .iter()
                .any(|known: &Channel| known.name == parsed.name)
            {
                return Err(RegistryInvalid {
                    problem: RegistryProblem::NameRepeated,
                    channel: Some(parsed.name),
                });
            }
            channels.push(parsed);
        }
        Ok(Self { channels })
    }

    /// The channel named `name`.
    pub fn channel(&self, name: &str) -> Option<&Channel> {
        self.channels.iter().find(|channel| channel.name == name)
    }

    /// Whether a program a channel names is not there: a note for `check-config`, since a moved
    /// program never stops a start.
    pub fn program_missing(&self) -> bool {
        self.channels.iter().any(|channel| {
            channel
                .dsh
                .as_ref()
                .is_some_and(|paths| !Path::new(&paths.node_executable).is_file())
        })
    }
}

/// The registry file under `state_root`.
pub fn registry_path(state_root: &Path) -> PathBuf {
    state_root.join("agent-forward").join("channels.json")
}

/// The registry under `state_root`; `None` when the file does not exist.
pub fn read_registry(state_root: &Path) -> Result<Option<ChannelRegistry>, RegistryReadFailure> {
    match std::fs::read_to_string(registry_path(state_root)) {
        Ok(text) => ChannelRegistry::parse(&text)
            .map(Some)
            .map_err(RegistryReadFailure::Invalid),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(RegistryReadFailure::Read(error.kind())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn registry(channels: Value) -> Result<ChannelRegistry, RegistryInvalid> {
        ChannelRegistry::parse(
            &json!({"schema": CHANNELS_SCHEMA, "channels": channels}).to_string(),
        )
    }

    fn problem(channels: Value) -> RegistryProblem {
        registry(channels)
            .expect_err("the registry is refused")
            .problem
    }

    fn plain(name: &str) -> Value {
        json!({"name": name, "client": "claude", "native_id": "00000000-0000-4000-8000-000000000001"})
    }

    #[test]
    fn channels_refuse_control_characters_long_names_and_duplicates() {
        // Chinese names count by scalar: sixteen pass, seventeen do not.
        let sixteen = "频".repeat(16);
        let parsed = registry(json!([plain("总览"), plain(&sixteen)])).expect("two names pass");
        assert_eq!(parsed.channels.len(), 2);
        assert_eq!(
            parsed.channel("总览").map(|channel| channel.client),
            Some(ChannelClient::Claude)
        );
        for name in [
            "频".repeat(17),
            String::new(),
            "  ".to_owned(),
            "line\nbreak".to_owned(),
            "tab\there".to_owned(),
        ] {
            assert_eq!(
                problem(json!([plain(&name)])),
                RegistryProblem::NameInvalid,
                "{name:?}"
            );
        }
        assert_eq!(
            registry(json!([plain("总览"), plain("总览")])),
            Err(RegistryInvalid {
                problem: RegistryProblem::NameRepeated,
                channel: Some("总览".to_owned()),
            })
        );
        // The native id, the client word, an unknown member, another schema.
        for (entry, expected) in [
            (
                json!({"name": "a", "client": "claude", "native_id": ""}),
                RegistryProblem::NativeIdInvalid,
            ),
            (
                json!({"name": "a", "client": "claude", "native_id": "x\ny"}),
                RegistryProblem::NativeIdInvalid,
            ),
            (
                json!({"name": "a", "client": "neutral", "native_id": "x"}),
                RegistryProblem::Shape,
            ),
            (
                json!({"name": "a", "client": "claude", "native_id": "x", "token": "y"}),
                RegistryProblem::Shape,
            ),
        ] {
            assert_eq!(problem(json!([entry])), expected);
        }
        assert_eq!(
            ChannelRegistry::parse(&json!({"schema": "other", "channels": []}).to_string())
                .map_err(|invalid| invalid.problem),
            Err(RegistryProblem::Shape)
        );
    }

    #[test]
    fn channels_dsh_requires_three_absolute_paths() {
        let entry = |app_root: Value, home_root: Value, node_executable: Value| {
            json!([{
                "name": "a", "client": "dsh", "native_id": "neutral-session",
                "app_root": app_root, "home_root": home_root, "node_executable": node_executable,
            }])
        };
        let parsed = registry(entry(
            json!(r"C:\neutral\app"),
            json!(r"\\neutral\home"),
            json!("C:/neutral/node.exe"),
        ))
        .expect("three absolute paths pass");
        assert_eq!(
            parsed.channels[0].dsh,
            Some(DshPaths {
                app_root: r"C:\neutral\app".to_owned(),
                home_root: r"\\neutral\home".to_owned(),
                node_executable: "C:/neutral/node.exe".to_owned(),
            })
        );
        // One left out, each one relative.
        for channels in [
            entry(Value::Null, json!(r"C:\h"), json!(r"C:\n.exe")),
            entry(json!(r"C:\a"), Value::Null, json!(r"C:\n.exe")),
            entry(json!(r"C:\a"), json!(r"C:\h"), Value::Null),
            entry(json!(r"neutral\app"), json!(r"C:\h"), json!(r"C:\n.exe")),
            entry(json!(r"C:\a"), json!("home"), json!(r"C:\n.exe")),
            entry(json!(r"C:\a"), json!(r"C:\h"), json!("node.exe")),
        ] {
            assert_eq!(problem(channels), RegistryProblem::PathsInvalid);
        }
        // Another client's channel names none of them.
        assert_eq!(
            problem(json!([{
                "name": "a", "client": "codex", "native_id": "x", "app_root": r"C:\a",
            }])),
            RegistryProblem::PathsInvalid
        );
    }
}
