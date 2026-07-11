// SPDX-License-Identifier: AGPL-3.0-only

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    OwnerEpoch, RuntimeErrorCode, RuntimeMonitorInstanceStatus, RuntimeMonitorPolicy,
    RuntimeMonitorRegistryStatus, RuntimeMonitorState,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

pub(crate) const MONITOR_FILE_NAME: &str = "monitor.journal";
const MONITOR_SCHEMA_VERSION: &str = "actingcommand.runtime-monitor.v1";
const MAX_MONITOR_JOURNAL_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MonitorRecord {
    schema_version: String,
    revision: u64,
    monitors: Vec<RuntimeMonitorInstanceStatus>,
}

pub(crate) struct MonitorUpdate {
    pub(crate) status: RuntimeMonitorInstanceStatus,
    pub(crate) changed: bool,
}

pub(crate) struct MonitorRegistry {
    file: File,
    revision: u64,
    monitors: BTreeMap<String, RuntimeMonitorInstanceStatus>,
    allowed_aliases: BTreeSet<String>,
}

impl MonitorRegistry {
    pub(crate) fn open(
        state_root: &Path,
        allowed_aliases: impl IntoIterator<Item = String>,
    ) -> RuntimeHostResult<Self> {
        let allowed_aliases = allowed_aliases.into_iter().collect::<BTreeSet<_>>();
        if allowed_aliases.is_empty() {
            return Err(monitor_error(
                "empty_monitor_instance_registry",
                "open_monitor_registry",
            ));
        }
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(state_root.join(MONITOR_FILE_NAME))
            .map_err(|_| monitor_error("monitor_file_open_failed", "open_monitor_registry"))?;
        let last = read_last_record(&mut file, &allowed_aliases)?;
        let (revision, monitors) = last.map_or_else(
            || (0, BTreeMap::new()),
            |record| {
                let monitors = record
                    .monitors
                    .into_iter()
                    .map(|status| (status.instance_alias().to_string(), status))
                    .collect();
                (record.revision, monitors)
            },
        );
        file.seek(SeekFrom::End(0))
            .map_err(|_| monitor_error("monitor_file_seek_failed", "open_monitor_registry"))?;
        Ok(Self {
            file,
            revision,
            monitors,
            allowed_aliases,
        })
    }

    pub(crate) fn configure(
        &mut self,
        instance_alias: &str,
        policy: RuntimeMonitorPolicy,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<MonitorUpdate> {
        self.require_allowed(instance_alias)?;
        policy
            .validate()
            .map_err(|_| monitor_error("monitor_policy_invalid", "configure_monitor_policy"))?;
        if let Some(existing) = self.monitors.get(instance_alias)
            && existing.policy() == Some(&policy)
        {
            return Ok(MonitorUpdate {
                status: existing.clone(),
                changed: false,
            });
        }
        let state = RuntimeMonitorState::scheduled(now_unix_ms)
            .map_err(|_| monitor_error("monitor_state_invalid", "configure_monitor_policy"))?;
        let status = RuntimeMonitorInstanceStatus::configured(instance_alias, policy, state)
            .map_err(|_| monitor_error("monitor_status_invalid", "configure_monitor_policy"))?;
        let mut monitors = self.monitors.clone();
        monitors.insert(instance_alias.to_string(), status.clone());
        self.append_snapshot(&monitors)?;
        self.monitors = monitors;
        Ok(MonitorUpdate {
            status,
            changed: true,
        })
    }

    pub(crate) fn clear(&mut self, instance_alias: &str) -> RuntimeHostResult<MonitorUpdate> {
        self.require_allowed(instance_alias)?;
        let status = RuntimeMonitorInstanceStatus::unconfigured(instance_alias)
            .map_err(|_| monitor_error("monitor_status_invalid", "clear_monitor_policy"))?;
        if !self.monitors.contains_key(instance_alias) {
            return Ok(MonitorUpdate {
                status,
                changed: false,
            });
        }
        let mut monitors = self.monitors.clone();
        monitors.remove(instance_alias);
        self.append_snapshot(&monitors)?;
        self.monitors = monitors;
        Ok(MonitorUpdate {
            status,
            changed: true,
        })
    }

    pub(crate) fn status(
        &self,
        owner_epoch: OwnerEpoch,
    ) -> RuntimeHostResult<RuntimeMonitorRegistryStatus> {
        let instances = self
            .allowed_aliases
            .iter()
            .map(|alias| {
                self.monitors
                    .get(alias)
                    .cloned()
                    .map_or_else(|| RuntimeMonitorInstanceStatus::unconfigured(alias), Ok)
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| monitor_error("monitor_status_invalid", "read_monitor_status"))?;
        RuntimeMonitorRegistryStatus::new(owner_epoch, instances)
            .map_err(|_| monitor_error("monitor_status_invalid", "read_monitor_status"))
    }

    fn require_allowed(&self, instance_alias: &str) -> RuntimeHostResult<()> {
        if !self.allowed_aliases.contains(instance_alias) {
            return Err(monitor_error(
                "monitor_instance_unknown",
                "validate_monitor_instance",
            ));
        }
        Ok(())
    }

    fn append_snapshot(
        &mut self,
        monitors: &BTreeMap<String, RuntimeMonitorInstanceStatus>,
    ) -> RuntimeHostResult<()> {
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| monitor_error("monitor_revision_overflow", "write_monitor_registry"))?;
        let record = MonitorRecord {
            schema_version: MONITOR_SCHEMA_VERSION.to_string(),
            revision,
            monitors: monitors.values().cloned().collect(),
        };
        let mut encoded = serde_json::to_vec(&record)
            .map_err(|_| monitor_error("monitor_record_encode_failed", "write_monitor_registry"))?;
        encoded.push(b'\n');
        let current_length = self
            .file
            .metadata()
            .map_err(|_| monitor_error("monitor_metadata_failed", "write_monitor_registry"))?
            .len();
        let encoded_length = u64::try_from(encoded.len())
            .map_err(|_| monitor_error("monitor_journal_too_large", "write_monitor_registry"))?;
        let next_length = current_length
            .checked_add(encoded_length)
            .ok_or_else(|| monitor_error("monitor_journal_too_large", "write_monitor_registry"))?;
        if next_length > MAX_MONITOR_JOURNAL_BYTES {
            return Err(monitor_error(
                "monitor_journal_too_large",
                "write_monitor_registry",
            ));
        }
        self.file
            .seek(SeekFrom::End(0))
            .map_err(|_| monitor_error("monitor_file_seek_failed", "write_monitor_registry"))?;
        self.file
            .write_all(&encoded)
            .map_err(|_| monitor_error("monitor_file_write_failed", "write_monitor_registry"))?;
        self.file
            .sync_data()
            .map_err(|_| monitor_error("monitor_file_sync_failed", "write_monitor_registry"))?;
        self.revision = revision;
        Ok(())
    }
}

fn read_last_record(
    file: &mut File,
    allowed_aliases: &BTreeSet<String>,
) -> RuntimeHostResult<Option<MonitorRecord>> {
    let length = file
        .metadata()
        .map_err(|_| monitor_error("monitor_metadata_failed", "read_monitor_registry"))?
        .len();
    if length > MAX_MONITOR_JOURNAL_BYTES {
        return Err(monitor_error(
            "monitor_journal_too_large",
            "read_monitor_registry",
        ));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| monitor_error("monitor_file_seek_failed", "read_monitor_registry"))?;
    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .map_err(|_| monitor_error("monitor_file_read_failed", "read_monitor_registry"))?;
    if content.is_empty() {
        return Ok(None);
    }
    if content.last() != Some(&b'\n') {
        return Err(invalid_monitor_record("read_monitor_registry"));
    }
    let content = std::str::from_utf8(&content)
        .map_err(|_| invalid_monitor_record("read_monitor_registry"))?;
    let mut previous_revision = 0;
    let mut last = None;
    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        let record = serde_json::from_str::<MonitorRecord>(line)
            .map_err(|_| invalid_monitor_record("read_monitor_registry"))?;
        validate_record(&record, allowed_aliases)?;
        if record.revision != previous_revision + 1 {
            return Err(invalid_monitor_record("validate_monitor_registry"));
        }
        previous_revision = record.revision;
        last = Some(record);
    }
    last.ok_or_else(|| invalid_monitor_record("read_monitor_registry"))
        .map(Some)
}

fn validate_record(
    record: &MonitorRecord,
    allowed_aliases: &BTreeSet<String>,
) -> RuntimeHostResult<()> {
    if record.schema_version != MONITOR_SCHEMA_VERSION || record.revision == 0 {
        return Err(invalid_monitor_record("validate_monitor_registry"));
    }
    let mut previous_alias = None;
    let mut aliases = BTreeSet::new();
    for status in &record.monitors {
        status
            .validate()
            .map_err(|_| invalid_monitor_record("validate_monitor_registry"))?;
        let alias = status.instance_alias();
        if status.policy().is_none()
            || !allowed_aliases.contains(alias)
            || !aliases.insert(alias)
            || previous_alias.is_some_and(|previous| previous >= alias)
        {
            return Err(invalid_monitor_record("validate_monitor_registry"));
        }
        previous_alias = Some(alias);
    }
    Ok(())
}

fn monitor_error(code: &'static str, operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(code, operation, RuntimeErrorCode::RuntimeFatal)
}

fn invalid_monitor_record(operation: &'static str) -> RuntimeHostError {
    monitor_error("monitor_record_invalid", operation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::IdentifierIssuer;
    use tempfile::TempDir;

    #[test]
    fn monitor_registry_persists_and_idempotent_updates_do_not_append() {
        let root = TempDir::new().expect("tempdir");
        let mut registry =
            MonitorRegistry::open(root.path(), ["ak.cn".to_string()]).expect("registry");
        let policy = RuntimeMonitorPolicy::new(1_000, "home", false).expect("policy");
        assert!(
            registry
                .configure("ak.cn", policy.clone(), 10)
                .expect("configure")
                .changed
        );
        let length = registry.file.metadata().expect("metadata").len();
        assert!(
            !registry
                .configure("ak.cn", policy, 20)
                .expect("idempotent configure")
                .changed
        );
        assert_eq!(registry.file.metadata().expect("metadata").len(), length);
        drop(registry);

        let mut reopened =
            MonitorRegistry::open(root.path(), ["ak.cn".to_string()]).expect("reopen");
        let owner_epoch = *IdentifierIssuer::new()
            .expect("issuer")
            .mint_owner_epoch()
            .expect("epoch")
            .transport();
        assert!(
            reopened.status(owner_epoch).expect("status").instances()[0]
                .policy()
                .is_some()
        );
        assert!(reopened.clear("ak.cn").expect("clear").changed);
        let length = reopened.file.metadata().expect("metadata").len();
        assert!(!reopened.clear("ak.cn").expect("idempotent clear").changed);
        assert_eq!(reopened.file.metadata().expect("metadata").len(), length);
    }

    #[test]
    fn monitor_registry_rejects_corrupt_or_unknown_persisted_state() {
        let root = TempDir::new().expect("tempdir");
        std::fs::write(root.path().join(MONITOR_FILE_NAME), b"not-json\n")
            .expect("write corruption");
        assert_eq!(
            MonitorRegistry::open(root.path(), ["ak.cn".to_string()])
                .err()
                .expect("corrupt registry")
                .code(),
            "monitor_record_invalid"
        );
    }
}
