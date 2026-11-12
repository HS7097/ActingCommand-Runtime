// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #437 MAPP-1 (`contracts/device-group.md`, "The configuration form", "Undeclared
//! sharing", "Derived, never stored"; register §1.323, §1.328): the device groups of a
//! configuration, derived at assembly from the entries' `device_group` and never stored.
//!
//! - **The name** comes first, on every entry that names a group: 1 to 64 bytes under the alias
//!   rules (`name_invalid`, [`name_refusal`]).
//! - **The rules of a group of several**, at assembly, in check-config and at start alike, each a
//!   refusal `device_group_config_invalid` with its `device_group_problem`: no member is a handset
//!   (`handset_member`) or a fixture (`fixture_member`); every member names the device by the same
//!   key with the same value (`device_mismatch`); every member has its own `application_id`
//!   (`application_shared`) and a `startup_package` (`startup_package_missing`); no member turns
//!   on a device-level action the first slice refuses (`device_action_unsupported`).
//! - **Undeclared sharing**: two entries outside one group that name the same `instance_index`,
//!   `instance_name`, `serial`, or `host` and `port` (`undeclared_shared_device`). After the
//!   start's discovery the provider startup checks the same of the endpoints it resolved
//!   (`provider_startup.rs`).
//! - **Until the slice "order within one device"**, every group of several is refused after these
//!   rules (`several_members_unsupported`); a group of one is accepted and behaves as an entry
//!   without `device_group`.
//! - **The member → device key map**: a group's device key is the smallest instance id of its
//!   members, a group of one's the member's own id; the host hands it to the scheduler and the
//!   evaluator, in memory only.

use super::{InstanceConfig, default_device_host, default_device_port};
use crate::codes::{ActingdCode, DeviceGroupProblem};
use actingcommand_contract::InstanceId;
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_runtime_config::{CheckContext, ConfigFailure, EffectiveConfig};
use std::collections::BTreeMap;

/// The longest `device_group` name, in bytes (`contracts/device-group.md`, "The configuration
/// form": 1 to 64 bytes under the alias rules, whose own bound is longer). A bound of the form,
/// not a tunable.
const MAX_DEVICE_GROUP_BYTES: usize = 64;
/// The entry's member a refusal names for a handset member: its `device` (kind).
const DEVICE_KEY: &str = "device";
/// The entry's member a refusal names for a fixture member.
const FIXTURE_BACKEND_KEY: &str = "fixture_backend";
/// The entry's member a refusal names for two members with one application.
const APPLICATION_ID_KEY: &str = "application_id";
/// The entry's member a refusal names for a member without a startup package.
const STARTUP_PACKAGE_KEY: &str = "startup_package";
/// The discovery keys and the endpoint's members, as a refusal names them.
pub(super) const INSTANCE_INDEX_KEY: &str = "instance_index";
const INSTANCE_NAME_KEY: &str = "instance_name";
const HOST_KEY: &str = "host";
pub(super) const PORT_KEY: &str = "port";
const SERIAL_KEY: &str = "serial";

/// How one entry names its device, as the group rules compare it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DeviceNaming {
    /// A fixture or a handset: the entry names no device the rules compare (a handset's serial
    /// is the private binding file's, which binds one serial to one alias).
    Unnamed,
    /// `instance_index`, resolved by the start's discovery.
    Index(u16),
    /// `instance_name`, resolved by the start's discovery.
    Name(String),
    /// An explicit ADB endpoint: `host` and `port` as the registration takes them (the daemon's
    /// defaults where the entry leaves them out), and `serial` when the entry names one.
    Endpoint {
        host: String,
        port: u16,
        serial: Option<String>,
    },
}

impl DeviceNaming {
    /// The key this naming uses, as a refusal names it.
    const fn naming_member(&self) -> &'static str {
        match self {
            Self::Unnamed => DEVICE_KEY,
            Self::Index(_) => INSTANCE_INDEX_KEY,
            Self::Name(_) => INSTANCE_NAME_KEY,
            Self::Endpoint { .. } => PORT_KEY,
        }
    }

    /// The key a mismatch between this naming and `other` names: the first member of the
    /// endpoint that differs, or the key `other` uses when the two use different keys.
    fn differing_key(&self, other: &Self) -> &'static str {
        match (self, other) {
            (
                Self::Endpoint { host, port, serial },
                Self::Endpoint {
                    host: other_host,
                    port: other_port,
                    serial: other_serial,
                },
            ) => {
                if host != other_host {
                    HOST_KEY
                } else if port == other_port && serial != other_serial {
                    SERIAL_KEY
                } else {
                    PORT_KEY
                }
            }
            _ => other.naming_member(),
        }
    }

    /// The key two entries share when they name the same device by the same key and value:
    /// `instance_index`, `instance_name`, `serial`, or `host` and `port` (named `port`). `None`
    /// for entries that name no device or different devices.
    fn shared_key(&self, other: &Self) -> Option<&'static str> {
        match (self, other) {
            (Self::Index(index), Self::Index(other)) if index == other => Some(INSTANCE_INDEX_KEY),
            (Self::Name(name), Self::Name(other)) if name == other => Some(INSTANCE_NAME_KEY),
            (
                Self::Endpoint {
                    serial: Some(serial),
                    ..
                },
                Self::Endpoint {
                    serial: Some(other),
                    ..
                },
            ) if serial == other => Some(SERIAL_KEY),
            (
                Self::Endpoint { host, port, .. },
                Self::Endpoint {
                    host: other_host,
                    port: other_port,
                    ..
                },
            ) if host == other_host && port == other_port => Some(PORT_KEY),
            _ => None,
        }
    }
}

/// What the group rules read of one entry, taken before the assembly consumes the entry.
#[derive(Debug, Clone)]
pub(super) struct GroupEntry {
    alias: String,
    instance_id: InstanceId,
    /// The registry id as the configuration keys name it (`instance.<id>.<member>`).
    instance_key: String,
    device_group: Option<String>,
    naming: DeviceNaming,
    handset: bool,
    fixture: bool,
    application_id: Option<String>,
    startup_package: bool,
}

impl GroupEntry {
    /// The entry as the rules read it; `handset` is the device kind `assemble` checked.
    pub(super) fn of(instance: &InstanceConfig, instance_key: String, handset: bool) -> Self {
        let fixture = instance.fixture_backend.is_some();
        let naming = if fixture || handset {
            DeviceNaming::Unnamed
        } else {
            match (instance.instance_index, instance.instance_name.as_deref()) {
                (Some(index), _) => DeviceNaming::Index(index),
                (None, Some(name)) => DeviceNaming::Name(name.to_owned()),
                (None, None) => DeviceNaming::Endpoint {
                    host: instance.host.clone().unwrap_or_else(default_device_host),
                    port: instance.port.unwrap_or_else(default_device_port),
                    serial: instance.serial.clone(),
                },
            }
        };
        Self {
            alias: instance.alias.clone(),
            instance_id: instance.instance_id,
            instance_key,
            device_group: instance.device_group.clone(),
            naming,
            handset,
            fixture,
            application_id: instance.application_id.clone(),
            startup_package: instance.startup_package.is_some(),
        }
    }
}

/// A refusal of the device group rules: `device_group_config_invalid` with the entries
/// concerned, the member that breaks the rule and the rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceGroupRefusal {
    pub(crate) aliases: Vec<String>,
    pub(crate) config_key: &'static str,
    pub(crate) problem: DeviceGroupProblem,
}

impl DeviceGroupRefusal {
    pub(super) fn new(
        aliases: impl IntoIterator<Item = String>,
        config_key: &'static str,
        problem: DeviceGroupProblem,
    ) -> Self {
        Self {
            aliases: aliases.into_iter().collect(),
            config_key,
            problem,
        }
    }

    pub(crate) const fn code(&self) -> Code {
        ActingdCode::DeviceGroupConfigInvalid.code()
    }

    pub(crate) fn outcome(&self) -> Outcome {
        Outcome::new(ActingdCode::DeviceGroupConfigInvalid)
            .aliases(&self.aliases)
            .config_key(self.config_key)
            .device_group_problem(self.problem)
    }
}

/// The first of the group checks, on every entry that names a group, before the kind rules: the
/// refusal of an entry `alias` whose `device_group` `name` is not 1 to 64 bytes under the alias
/// rules (`name_invalid`, `aliases` the entry, `config_key` `device_group`). The alias is right;
/// the refusal names the member that is wrong.
pub(super) fn name_refusal(alias: &str, name: &str) -> Option<DeviceGroupRefusal> {
    (name.len() > MAX_DEVICE_GROUP_BYTES
        || actingcommand_contract::validate_instance_alias(name).is_err())
    .then(|| {
        DeviceGroupRefusal::new(
            [alias.to_owned()],
            actingcommand_runtime_config::DEVICE_GROUP,
            DeviceGroupProblem::NameInvalid,
        )
    })
}

/// Why the rules refused the configuration: a rule of device groups, or the configuration
/// mechanism's own refusal while it read a member's values in effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum GroupCheckFailure {
    Group(DeviceGroupRefusal),
    Config(ConfigFailure),
}

/// The device groups of one configuration, derived at assembly.
#[derive(Debug, Clone, Default)]
pub(crate) struct DeviceGroups {
    entries: Vec<GroupEntry>,
}

impl DeviceGroups {
    pub(super) fn new(entries: Vec<GroupEntry>) -> Self {
        Self { entries }
    }

    /// Every group the entries name, by name, its members in alias order. A value one entry
    /// names alone is a group of one.
    fn groups(&self) -> BTreeMap<&str, Vec<&GroupEntry>> {
        let mut groups = BTreeMap::<&str, Vec<&GroupEntry>>::new();
        for entry in &self.entries {
            if let Some(name) = entry.device_group.as_deref() {
                groups.entry(name).or_default().push(entry);
            }
        }
        for members in groups.values_mut() {
            members.sort_by(|left, right| left.alias.cmp(&right.alias));
        }
        groups
    }

    /// The groups of several, each as its members' aliases in alias order.
    pub(crate) fn several(&self) -> Vec<Vec<String>> {
        self.groups()
            .into_values()
            .filter(|members| members.len() > 1)
            .map(|members| {
                members
                    .into_iter()
                    .map(|member| member.alias.clone())
                    .collect()
            })
            .collect()
    }

    /// The rules of the device groups with the values in effect, in order: each group of
    /// several by name against its rules (`handset_member`, `fixture_member`, `device_mismatch`,
    /// `application_shared`, `startup_package_missing`, `device_action_unsupported`), then
    /// undeclared sharing over every two entries outside one group, in configuration order, then
    /// the first group of several (`several_members_unsupported`). A group of one meets only
    /// the sharing rule.
    pub(super) fn check(
        &self,
        effective: &EffectiveConfig,
        context: &CheckContext,
    ) -> Result<(), GroupCheckFailure> {
        let refused = |aliases: Vec<String>, key: &'static str, problem| {
            Err(GroupCheckFailure::Group(DeviceGroupRefusal::new(
                aliases, key, problem,
            )))
        };
        let groups = self.groups();
        for members in groups.values().filter(|members| members.len() > 1) {
            if let Some(member) = members.iter().find(|member| member.handset) {
                return refused(
                    vec![member.alias.clone()],
                    DEVICE_KEY,
                    DeviceGroupProblem::HandsetMember,
                );
            }
            if let Some(member) = members.iter().find(|member| member.fixture) {
                return refused(
                    vec![member.alias.clone()],
                    FIXTURE_BACKEND_KEY,
                    DeviceGroupProblem::FixtureMember,
                );
            }
            let first = members[0];
            if let Some(member) = members[1..]
                .iter()
                .find(|member| member.naming != first.naming)
            {
                return refused(
                    vec![first.alias.clone(), member.alias.clone()],
                    first.naming.differing_key(&member.naming),
                    DeviceGroupProblem::DeviceMismatch,
                );
            }
            for (at, member) in members.iter().enumerate() {
                if let Some(other) = members[at + 1..].iter().find(|other| {
                    member.application_id.is_some() && other.application_id == member.application_id
                }) {
                    return refused(
                        vec![member.alias.clone(), other.alias.clone()],
                        APPLICATION_ID_KEY,
                        DeviceGroupProblem::ApplicationShared,
                    );
                }
            }
            if let Some(member) = members.iter().find(|member| !member.startup_package) {
                return refused(
                    vec![member.alias.clone()],
                    STARTUP_PACKAGE_KEY,
                    DeviceGroupProblem::StartupPackageMissing,
                );
            }
            for member in members {
                if let Some((action, _)) = effective
                    .device_action_in_effect(&member.instance_key, context)
                    .map_err(GroupCheckFailure::Config)?
                {
                    return refused(
                        vec![member.alias.clone()],
                        action.member(),
                        DeviceGroupProblem::DeviceActionUnsupported,
                    );
                }
            }
        }
        for (at, entry) in self.entries.iter().enumerate() {
            for other in &self.entries[at + 1..] {
                if entry.device_group.is_some() && entry.device_group == other.device_group {
                    continue;
                }
                if let Some(key) = entry.naming.shared_key(&other.naming) {
                    return refused(
                        vec![entry.alias.clone(), other.alias.clone()],
                        key,
                        DeviceGroupProblem::UndeclaredSharedDevice,
                    );
                }
            }
        }
        if let Some(members) = groups.values().find(|members| members.len() > 1) {
            return refused(
                members.iter().map(|member| member.alias.clone()).collect(),
                actingcommand_runtime_config::DEVICE_GROUP,
                DeviceGroupProblem::SeveralMembersUnsupported,
            );
        }
        Ok(())
    }

    /// The member → device key map of every entry: a group's device key is the smallest
    /// instance id of its members, an entry without a group (or alone in its group) keys its
    /// own id. Memory only.
    pub(crate) fn device_keys(&self) -> BTreeMap<InstanceId, InstanceId> {
        let groups = self.groups();
        self.entries
            .iter()
            .map(|entry| {
                let device = entry
                    .device_group
                    .as_deref()
                    .and_then(|name| groups.get(name))
                    .and_then(|members| members.iter().map(|member| member.instance_id).min())
                    .unwrap_or(entry.instance_id);
                (entry.instance_id, device)
            })
            .collect()
    }

    /// The group each entry that names one names, by alias: check-config's report member.
    pub(crate) fn names(&self) -> BTreeMap<String, String> {
        self.entries
            .iter()
            .filter_map(|entry| {
                entry
                    .device_group
                    .clone()
                    .map(|name| (entry.alias.clone(), name))
            })
            .collect()
    }

    /// The group an entry names, by alias; `None` for an entry without one.
    pub(super) fn group_of(&self, alias: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| entry.alias == alias)
            .and_then(|entry| entry.device_group.as_deref())
    }

    /// The explicit endpoints the start's discovery compares with the endpoints it resolves:
    /// each entry bound by its `host` and `port`, with its alias.
    pub(super) fn explicit_endpoints(&self) -> Vec<(String, String, u16)> {
        self.entries
            .iter()
            .filter_map(|entry| match &entry.naming {
                DeviceNaming::Endpoint { host, port, .. } => {
                    Some((entry.alias.clone(), host.clone(), *port))
                }
                _ => None,
            })
            .collect()
    }
}
