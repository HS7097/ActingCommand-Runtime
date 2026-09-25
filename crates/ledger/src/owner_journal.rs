// SPDX-License-Identifier: AGPL-3.0-only

//! The native owner journal reader shared with OwnerGuard. This is an in-memory
//! capability, not a serialized request or a second persistence path.
use actingcommand_contract::{
    InstanceId, OWNER_JOURNAL_LIMIT, OWNER_JOURNAL_SCHEMA, OwnerClosedRevision, OwnerEpoch,
    OwnerEpochCloseEvidence, OwnerResourceDisposition,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOwnerRecord {
    pub schema_version: String,
    pub revision: u64,
    pub owner_epoch: OwnerEpoch,
    pub pid: u32,
    pub started_at_unix_ms: u64,
    pub active: bool,
    pub active_instances: Vec<InstanceId>,
    pub closed_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_disposition: Option<OwnerResourceDisposition>,
}

pub struct OwnerJournalError {
    pub code: &'static str,
    pub operation: &'static str,
}

/// The native reader's observation of one owner epoch's block, whether or not it proves
/// a close. On epoch reuse the latest block is kept and `consistent` is false.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OwnerEpochBlock {
    /// Some record predates the v2 schema.
    pub(crate) legacy: bool,
    /// One unique epoch with one pid/started_at and no inactive-to-active transition.
    pub(crate) consistent: bool,
    /// Some record declares `ConfirmedClosed`.
    pub(crate) confirmed_closed: bool,
    pub(crate) last_active: bool,
    pub(crate) last_disposition: Option<OwnerResourceDisposition>,
    pub(crate) pid: u32,
    pub(crate) started_at_unix_ms: u64,
    pub(crate) first_revision: u64,
    pub(crate) final_revision: u64,
}

/// Only a complete native read constructs this capability; no public proof setter,
/// deserializer or external command accepts evidence supplied by a caller.
pub struct RuntimeOwnerJournal {
    last: Option<RuntimeOwnerRecord>,
    pub(crate) proofs: BTreeMap<OwnerEpoch, OwnerEpochCloseEvidence>,
    prefix_hashes: BTreeMap<u64, String>,
    blocks: BTreeMap<OwnerEpoch, OwnerEpochBlock>,
    through_revision: u64,
    bytes: u64,
    sha256: String,
}

impl RuntimeOwnerJournal {
    pub fn last(&self) -> Option<&RuntimeOwnerRecord> {
        self.last.as_ref()
    }

    pub(crate) fn supports(&self, sealed: &OwnerEpochCloseEvidence) -> bool {
        self.prefix_hashes.get(&sealed.journal_bytes) == Some(&sealed.journal_sha256)
            && self.proofs.get(&sealed.subject).is_some_and(|current| {
                current.first_revision == sealed.first_revision
                    && current.confirmed_revision == sealed.confirmed_revision
                    && current.final_revision == sealed.final_revision
                    && current.journal_through_revision >= sealed.journal_through_revision
            })
    }

    pub(crate) fn block(&self, subject: OwnerEpoch) -> Option<&OwnerEpochBlock> {
        self.blocks.get(&subject)
    }

    fn block_identity(&self, subject: OwnerEpoch) -> (u32, u64, u64, u64) {
        self.blocks.get(&subject).map_or((0, 0, 0, 0), |block| {
            (
                block.pid,
                block.started_at_unix_ms,
                block.first_revision,
                block.final_revision,
            )
        })
    }

    /// The observation an unproven import seals for `subject`: no positive close, the
    /// block identity when one exists, and this complete read's bounds.
    pub(crate) fn observation(&self, subject: OwnerEpoch) -> OwnerEpochCloseEvidence {
        let (pid, started_at_unix_ms, first_revision, final_revision) =
            self.block_identity(subject);
        OwnerEpochCloseEvidence {
            schema_version: OWNER_JOURNAL_SCHEMA.to_owned(),
            subject,
            pid,
            started_at_unix_ms,
            first_revision,
            confirmed_revision: 0,
            final_revision,
            journal_through_revision: self.through_revision,
            journal_bytes: self.bytes,
            journal_sha256: self.sha256.clone(),
            suffix: Vec::new(),
        }
    }

    /// Whether this read still matches a sealed unproven observation: same complete-read
    /// prefix, same block identity and still no proof for the subject.
    pub(crate) fn supports_observation(&self, sealed: &OwnerEpochCloseEvidence) -> bool {
        self.prefix_hashes.get(&sealed.journal_bytes) == Some(&sealed.journal_sha256)
            && self.through_revision >= sealed.journal_through_revision
            && !self.proofs.contains_key(&sealed.subject)
            && self.block_identity(sealed.subject)
                == (
                    sealed.pid,
                    sealed.started_at_unix_ms,
                    sealed.first_revision,
                    sealed.final_revision,
                )
    }

    /// OwnerGuard holds its original exclusive lock throughout this read and startup.
    pub fn read_locked(file: &mut File) -> Result<Self, OwnerJournalError> {
        let error = |code, operation| OwnerJournalError { code, operation };
        let invalid = |operation| error("owner_record_invalid", operation);
        let length = file
            .metadata()
            .map_err(|_| error("owner_metadata_failed", "read_owner_file"))?
            .len();
        if length > OWNER_JOURNAL_LIMIT {
            return Err(error("owner_journal_too_large", "read_owner_file"));
        }
        file.seek(SeekFrom::Start(0))
            .map_err(|_| error("owner_seek_failed", "read_owner_file"))?;
        let mut content = Vec::new();
        file.read_to_end(&mut content)
            .map_err(|_| error("owner_read_failed", "read_owner_file"))?;
        if content.is_empty() {
            let empty = format!("{:x}", Sha256::new().finalize());
            return Ok(Self {
                last: None,
                proofs: BTreeMap::new(),
                prefix_hashes: BTreeMap::from([(0, empty.clone())]),
                blocks: BTreeMap::new(),
                through_revision: 0,
                bytes: 0,
                sha256: empty,
            });
        }
        let complete_length = if content.last() == Some(&b'\n') {
            content.len()
        } else {
            content
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map(|index| index + 1)
                .ok_or_else(|| invalid("recover_owner_file"))?
        };
        if complete_length < content.len() {
            file.set_len(complete_length as u64)
                .map_err(|_| error("owner_tail_truncate_failed", "recover_owner_file"))?;
            file.sync_data()
                .map_err(|_| error("owner_tail_sync_failed", "recover_owner_file"))?;
            content.truncate(complete_length);
        }
        let text = std::str::from_utf8(&content).map_err(|_| invalid("read_owner_file"))?;
        let mut records = Vec::new();
        let mut prefix_hashes = BTreeMap::new();
        let mut digest = Sha256::new();
        let mut bytes = 0_u64;
        for line in text.split_inclusive('\n') {
            // A prior complete read ends immediately before the next acquired owner's
            // append. Keep record boundaries, not an entry for every blank line.
            if !line.trim().is_empty() {
                prefix_hashes.insert(bytes, format!("{:x}", digest.clone().finalize()));
            }
            digest.update(line.as_bytes());
            bytes += line.len() as u64;
            if line.trim().is_empty() {
                continue;
            }
            let record: RuntimeOwnerRecord =
                serde_json::from_str(line).map_err(|_| invalid("read_owner_file"))?;
            let mut sorted = record.active_instances.clone();
            sorted.sort_unstable();
            sorted.dedup();
            let schema_valid = match record.schema_version.as_str() {
                "actingcommand.runtime-owner.v1" => record.resource_disposition.is_none(),
                OWNER_JOURNAL_SCHEMA => record.resource_disposition.is_some(),
                _ => false,
            };
            if !schema_valid
                || record.revision != records.len() as u64 + 1
                || record.pid == 0
                || record.started_at_unix_ms == 0
                || sorted != record.active_instances
                || record.active == record.closed_at_unix_ms.is_some()
                || record.schema_version == OWNER_JOURNAL_SCHEMA
                    && !record.active
                    && record.resource_disposition != Some(OwnerResourceDisposition::None)
            {
                return Err(invalid("validate_owner_file"));
            }
            records.push(record);
        }
        let digest = format!("{:x}", digest.finalize());
        prefix_hashes.insert(bytes, digest.clone());
        let last = records
            .last()
            .ok_or_else(|| invalid("read_owner_file"))?
            .clone();
        let mut proofs = BTreeMap::new();
        let mut blocks = BTreeMap::new();
        let mut seen = BTreeSet::new();
        let mut start = 0;
        while start < records.len() {
            let first = &records[start];
            let end = start
                + records[start..]
                    .iter()
                    .take_while(|r| r.owner_epoch == first.owner_epoch)
                    .count();
            let epoch = &records[start..end];
            let unique = seen.insert(first.owner_epoch);
            if !unique {
                proofs.remove(&first.owner_epoch);
            }
            let consistent = unique
                && epoch.iter().all(|r| {
                    r.schema_version == OWNER_JOURNAL_SCHEMA
                        && r.pid == first.pid
                        && r.started_at_unix_ms == first.started_at_unix_ms
                })
                && !epoch
                    .windows(2)
                    .any(|pair| !pair[0].active && pair[1].active);
            let final_record = &records[end - 1];
            blocks.insert(
                first.owner_epoch,
                OwnerEpochBlock {
                    legacy: epoch
                        .iter()
                        .any(|r| r.schema_version != OWNER_JOURNAL_SCHEMA),
                    consistent,
                    confirmed_closed: epoch.iter().any(|r| {
                        r.resource_disposition == Some(OwnerResourceDisposition::ConfirmedClosed)
                    }),
                    last_active: final_record.active,
                    last_disposition: final_record.resource_disposition,
                    pid: first.pid,
                    started_at_unix_ms: first.started_at_unix_ms,
                    first_revision: first.revision,
                    final_revision: final_record.revision,
                },
            );
            if consistent
                && let Some(positive) = epoch.iter().rposition(|r| {
                    r.resource_disposition == Some(OwnerResourceDisposition::ConfirmedClosed)
                })
            {
                let suffix = epoch[positive..]
                    .iter()
                    .map(|r| OwnerClosedRevision {
                        revision: r.revision,
                        active: r.active,
                        closed_at_unix_ms: r.closed_at_unix_ms,
                        disposition: r.resource_disposition.expect("v2 journal validated"),
                    })
                    .collect();
                let proof = OwnerEpochCloseEvidence {
                    schema_version: OWNER_JOURNAL_SCHEMA.to_owned(),
                    subject: first.owner_epoch,
                    pid: first.pid,
                    started_at_unix_ms: first.started_at_unix_ms,
                    first_revision: first.revision,
                    confirmed_revision: epoch[positive].revision,
                    final_revision: records[end - 1].revision,
                    journal_through_revision: last.revision,
                    journal_bytes: content.len() as u64,
                    journal_sha256: digest.clone(),
                    suffix,
                };
                // A valid journal with no positive close suffix remains unknown, not permission.
                if proof.validate().is_ok() {
                    proofs.insert(first.owner_epoch, proof);
                }
            }
            start = end;
        }
        Ok(Self {
            through_revision: last.revision,
            last: Some(last),
            proofs,
            prefix_hashes,
            blocks,
            bytes: content.len() as u64,
            sha256: digest,
        })
    }
}
