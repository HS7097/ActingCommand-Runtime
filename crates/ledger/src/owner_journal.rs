// SPDX-License-Identifier: AGPL-3.0-only

//! The native owner journal reader shared with OwnerGuard. This is an in-memory
//! capability, not a serialized request or a second persistence path.
//! Workflow #191 O: the guard may fold every record but the last into one leading
//! checkpoint line (`contracts/owner-journal.md`); the reader derives the same state.
use actingcommand_contract::{
    InstanceId, OWNER_JOURNAL_LIMIT, OWNER_JOURNAL_SCHEMA, OwnerClosedRevision, OwnerEpoch,
    OwnerEpochCloseEvidence, OwnerResourceDisposition,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    ops::Range,
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

/// Workflow #191 O: the schema of the checkpoint line, only ever the journal's first line.
pub const OWNER_JOURNAL_CHECKPOINT_SCHEMA: &str = "actingcommand.runtime-owner-checkpoint.v1";
/// Workflow #191 O: the seal line that ends the side file, never written to the journal.
const OWNER_JOURNAL_COMPACTION_SCHEMA: &str = "actingcommand.runtime-owner-compaction.v1";

/// Workflow #191 O: the leading line that replaces every folded record with the reader's
/// derived state for them. The reader validates it on every read; the compaction proves
/// before writing it that the replacement derives the same state as the replaced file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerJournalCheckpoint {
    schema_version: String,
    /// Length and SHA-256 of the replaced file, for audit only.
    predecessor_bytes: u64,
    predecessor_sha256: String,
    /// The last folded record, verbatim; its revision is the base of the physical records.
    last_record: RuntimeOwnerRecord,
    /// One block summary per folded epoch, strictly ascending by epoch.
    epochs: Vec<CheckpointEpoch>,
    /// Every carried complete-read bound, strictly ascending by (bytes, sha256).
    prefixes: Vec<CheckpointPrefix>,
}

/// The fields of `OwnerEpochBlock` and, when the folded records proved one, the close proof.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointEpoch {
    owner_epoch: OwnerEpoch,
    pid: u32,
    started_at_unix_ms: u64,
    first_revision: u64,
    final_revision: u64,
    legacy: bool,
    consistent: bool,
    confirmed_closed: bool,
    last_active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_disposition: Option<OwnerResourceDisposition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    proof: Option<CheckpointProof>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointProof {
    confirmed_revision: u64,
    suffix: Vec<OwnerClosedRevision>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointPrefix {
    bytes: u64,
    sha256: String,
}

/// Workflow #191 O: the last line of the side file; it names the image before it and the
/// file the image replaces, so a crash anywhere in the in-place write is recoverable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerJournalCompactionSeal {
    schema_version: String,
    image_bytes: u64,
    image_sha256: String,
    image_through_revision: u64,
    predecessor_bytes: u64,
    predecessor_sha256: String,
}

/// Only a line that is no owner record is probed, and only for its schema.
#[derive(Deserialize)]
struct SchemaProbe {
    schema_version: String,
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
    /// Every record-boundary prefix of this file, and the block-start prefixes a
    /// checkpoint carries from the files it replaced.
    prefixes: BTreeSet<(u64, String)>,
    blocks: BTreeMap<OwnerEpoch, OwnerEpochBlock>,
    through_revision: u64,
    bytes: u64,
    sha256: String,
    uncompacted_bytes: u64,
}

impl RuntimeOwnerJournal {
    pub fn last(&self) -> Option<&RuntimeOwnerRecord> {
        self.last.as_ref()
    }

    /// The physical bytes after the checkpoint line (the whole file when there is none).
    pub fn uncompacted_bytes(&self) -> u64 {
        self.uncompacted_bytes
    }

    pub(crate) fn supports(&self, sealed: &OwnerEpochCloseEvidence) -> bool {
        self.prefixes
            .contains(&(sealed.journal_bytes, sealed.journal_sha256.clone()))
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
        self.prefixes
            .contains(&(sealed.journal_bytes, sealed.journal_sha256.clone()))
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
        let mut content = read_journal_file(file, "read_owner_file")?;
        if content.is_empty() {
            let empty = format!("{:x}", Sha256::new().finalize());
            return Ok(Self {
                last: None,
                proofs: BTreeMap::new(),
                prefixes: BTreeSet::from([(0, empty.clone())]),
                blocks: BTreeMap::new(),
                through_revision: 0,
                bytes: 0,
                sha256: empty,
                uncompacted_bytes: 0,
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
        let parsed = parse_journal(&content)?;
        let derived = derive_journal(
            parsed.checkpoint.as_ref(),
            &parsed.records,
            parsed.bytes,
            &parsed.sha256,
        )?;
        let mut prefixes = parsed.prefixes;
        prefixes.extend(derived.carried);
        Ok(Self {
            through_revision: derived.last.revision,
            last: Some(derived.last),
            proofs: derived.proofs,
            prefixes,
            blocks: derived.blocks,
            bytes: parsed.bytes,
            sha256: parsed.sha256,
            uncompacted_bytes: parsed.bytes - parsed.checkpoint_end,
        })
    }

    /// Workflow #191 O: the checked replacement of this journal that folds every physical
    /// record but the last into one leading checkpoint, or `None` with fewer than two
    /// physical records. Nothing is written here: the image must derive the same last
    /// record, revision, blocks and proofs as the file, and carry every block start the
    /// ledger may have sealed, or the call fails before the caller writes a byte.
    pub fn compaction_locked(
        file: &mut File,
    ) -> Result<Option<OwnerJournalCompaction>, OwnerJournalError> {
        const OPERATION: &str = "compact_owner_file";
        let error = |code| OwnerJournalError {
            code,
            operation: OPERATION,
        };
        let content = read_journal_file(file, OPERATION)?;
        if !content.is_empty() && content.last() != Some(&b'\n') {
            return Err(error("owner_compaction_incomplete_tail"));
        }
        let original = parse_journal(&content)?;
        let Some((kept, folded)) = original.records.split_last() else {
            return Ok(None);
        };
        if folded.is_empty() {
            return Ok(None);
        }
        let full = derive_journal(
            original.checkpoint.as_ref(),
            &original.records,
            original.bytes,
            &original.sha256,
        )?;
        let part = derive_journal(
            original.checkpoint.as_ref(),
            folded,
            original.bytes,
            &original.sha256,
        )?;
        let needed = full
            .carried
            .union(&full.block_starts)
            .cloned()
            .collect::<BTreeSet<_>>();
        let checkpoint = OwnerJournalCheckpoint {
            schema_version: OWNER_JOURNAL_CHECKPOINT_SCHEMA.to_owned(),
            predecessor_bytes: original.bytes,
            predecessor_sha256: original.sha256.clone(),
            last_record: part.last.clone(),
            epochs: part
                .blocks
                .iter()
                .map(|(epoch, block)| CheckpointEpoch {
                    owner_epoch: *epoch,
                    pid: block.pid,
                    started_at_unix_ms: block.started_at_unix_ms,
                    first_revision: block.first_revision,
                    final_revision: block.final_revision,
                    legacy: block.legacy,
                    consistent: block.consistent,
                    confirmed_closed: block.confirmed_closed,
                    last_active: block.last_active,
                    last_disposition: block.last_disposition,
                    proof: part.proofs.get(epoch).map(|proof| CheckpointProof {
                        confirmed_revision: proof.confirmed_revision,
                        suffix: proof.suffix.clone(),
                    }),
                })
                .collect(),
            prefixes: needed
                .iter()
                .map(|(bytes, sha256)| CheckpointPrefix {
                    bytes: *bytes,
                    sha256: sha256.clone(),
                })
                .collect(),
        };
        let mut image =
            serde_json::to_vec(&checkpoint).map_err(|_| error("owner_compaction_mismatch"))?;
        image.push(b'\n');
        image.extend_from_slice(&content[kept.line.clone()]);
        if image.len() as u64 > OWNER_JOURNAL_LIMIT {
            return Err(error("owner_journal_too_large"));
        }
        let replaced = parse_journal(&image).map_err(|_| error("owner_compaction_mismatch"))?;
        let derived = derive_journal(
            replaced.checkpoint.as_ref(),
            &replaced.records,
            replaced.bytes,
            &replaced.sha256,
        )
        .map_err(|_| error("owner_compaction_mismatch"))?;
        let mut supported = replaced.prefixes.clone();
        supported.extend(derived.carried.iter().cloned());
        if !same_state(&full, &derived) || !needed.is_subset(&supported) {
            return Err(error("owner_compaction_mismatch"));
        }
        let seal = OwnerJournalCompactionSeal {
            schema_version: OWNER_JOURNAL_COMPACTION_SCHEMA.to_owned(),
            image_bytes: replaced.bytes,
            image_sha256: replaced.sha256,
            image_through_revision: derived.last.revision,
            predecessor_bytes: original.bytes,
            predecessor_sha256: original.sha256,
        };
        let mut side_file = image.clone();
        side_file
            .extend(serde_json::to_vec(&seal).map_err(|_| error("owner_compaction_mismatch"))?);
        side_file.push(b'\n');
        Ok(Some(OwnerJournalCompaction {
            image,
            side_file,
            kept_bytes: kept.line.len() as u64,
        }))
    }

    /// Workflow #191 O: finishes or discards an interrupted compaction from its side file
    /// before any read. Stale: the in-place write never began and the journal is left as it
    /// is. Completed: the journal starts with the image; later appends are kept. Truncated:
    /// the replaced file's residue after the image was cut off. Rewritten: an interrupted
    /// in-place write was redone. Any other state is `owner_compaction_unrecoverable`; the
    /// journal is never cut on a guess nor overwritten outside the interrupted-write window.
    pub fn recover_compaction_locked(
        file: &mut File,
        side: &[u8],
    ) -> Result<OwnerCompactionRecovery, OwnerJournalError> {
        const OPERATION: &str = "recover_owner_compaction";
        let error = |code| OwnerJournalError {
            code,
            operation: OPERATION,
        };
        let current = read_journal_file(file, OPERATION)?;
        // R1: an incomplete side file means the in-place write never started.
        let Some(seal) = side_seal(side) else {
            return Ok(OwnerCompactionRecovery::Stale);
        };
        let length = current.len() as u64;
        let prefix_is = |bytes: u64, sha256: &str| {
            length >= bytes && sha256_hex(&current[..bytes as usize]) == sha256
        };
        // R2: the journal still starts with the replaced file; appends after it are kept.
        if prefix_is(seal.predecessor_bytes, &seal.predecessor_sha256) {
            return Ok(OwnerCompactionRecovery::Stale);
        }
        let repair = |_| error("owner_compaction_repair_failed");
        // R3: the image is in place; classify what follows it.
        if prefix_is(seal.image_bytes, &seal.image_sha256) {
            let rest = &current[seal.image_bytes as usize..];
            let appended = rest
                .iter()
                .position(|byte| *byte == b'\n')
                .and_then(|end| serde_json::from_slice::<RuntimeOwnerRecord>(&rest[..=end]).ok())
                .is_some_and(|record| {
                    seal.image_through_revision.checked_add(1) == Some(record.revision)
                });
            if rest.is_empty() || appended {
                return Ok(OwnerCompactionRecovery::Completed);
            }
            if length == seal.predecessor_bytes {
                file.set_len(seal.image_bytes).map_err(repair)?;
                file.sync_all().map_err(repair)?;
                return Ok(OwnerCompactionRecovery::Truncated);
            }
            if !rest.contains(&b'\n') {
                // The torn tail of a later append goes to the ordinary tail recovery.
                return Ok(OwnerCompactionRecovery::Completed);
            }
            return Err(error("owner_compaction_unrecoverable"));
        }
        // R4: an interrupted in-place write leaves exactly these lengths.
        if length == seal.image_bytes
            || (seal.predecessor_bytes <= length
                && length <= seal.predecessor_bytes.max(seal.image_bytes))
        {
            file.seek(SeekFrom::Start(0)).map_err(repair)?;
            file.write_all(&side[..seal.image_bytes as usize])
                .map_err(repair)?;
            file.set_len(seal.image_bytes).map_err(repair)?;
            file.sync_all().map_err(repair)?;
            return Ok(OwnerCompactionRecovery::Rewritten);
        }
        Err(error("owner_compaction_unrecoverable"))
    }
}

/// Workflow #191 O: a checked replacement image for the owner journal and the side file
/// that makes writing it in place recoverable. It carries no derived journal state.
pub struct OwnerJournalCompaction {
    image: Vec<u8>,
    side_file: Vec<u8>,
    kept_bytes: u64,
}

impl OwnerJournalCompaction {
    /// The checkpoint line followed by the unchanged bytes of the last physical record.
    pub fn image(&self) -> &[u8] {
        &self.image
    }

    /// The image followed by its seal line.
    pub fn side_file(&self) -> &[u8] {
        &self.side_file
    }

    /// The bytes of the one record the image keeps after the checkpoint.
    pub fn kept_bytes(&self) -> u64 {
        self.kept_bytes
    }
}

/// What `recover_compaction_locked` found and did (`contracts/owner-journal.md`, R1-R4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerCompactionRecovery {
    Stale,
    Completed,
    Truncated,
    Rewritten,
}

/// One validated physical record, the complete-read bound at its line start and its line.
struct PhysicalRecord {
    record: RuntimeOwnerRecord,
    start: (u64, String),
    line: Range<usize>,
}

/// The line-level parse of one complete journal image.
struct ParsedJournal {
    checkpoint: Option<OwnerJournalCheckpoint>,
    checkpoint_end: u64,
    records: Vec<PhysicalRecord>,
    prefixes: BTreeSet<(u64, String)>,
    bytes: u64,
    sha256: String,
}

/// The state the reader derives from a checkpoint and the physical records after it.
struct DerivedJournal {
    last: RuntimeOwnerRecord,
    blocks: BTreeMap<OwnerEpoch, OwnerEpochBlock>,
    proofs: BTreeMap<OwnerEpoch, OwnerEpochCloseEvidence>,
    /// The prefixes the checkpoint carries.
    carried: BTreeSet<(u64, String)>,
    /// The start of every physical block except a checkpoint's continued epoch.
    block_starts: BTreeSet<(u64, String)>,
}

fn read_journal_file(
    file: &mut File,
    operation: &'static str,
) -> Result<Vec<u8>, OwnerJournalError> {
    let error = |code| OwnerJournalError { code, operation };
    let length = file
        .metadata()
        .map_err(|_| error("owner_metadata_failed"))?
        .len();
    if length > OWNER_JOURNAL_LIMIT {
        return Err(error("owner_journal_too_large"));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| error("owner_seek_failed"))?;
    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .map_err(|_| error("owner_read_failed"))?;
    Ok(content)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// Every per-record rule except revision continuity.
fn record_valid(record: &RuntimeOwnerRecord) -> bool {
    let mut sorted = record.active_instances.clone();
    sorted.sort_unstable();
    sorted.dedup();
    let schema_valid = match record.schema_version.as_str() {
        "actingcommand.runtime-owner.v1" => record.resource_disposition.is_none(),
        OWNER_JOURNAL_SCHEMA => record.resource_disposition.is_some(),
        _ => false,
    };
    schema_valid
        && record.pid != 0
        && record.started_at_unix_ms != 0
        && sorted == record.active_instances
        && record.active != record.closed_at_unix_ms.is_some()
        && !(record.schema_version == OWNER_JOURNAL_SCHEMA
            && !record.active
            && record.resource_disposition != Some(OwnerResourceDisposition::None))
}

fn side_seal(side: &[u8]) -> Option<OwnerJournalCompactionSeal> {
    let body = side.strip_suffix(b"\n")?;
    let seal_start = body
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    let seal = serde_json::from_slice::<OwnerJournalCompactionSeal>(&side[seal_start..]).ok()?;
    (seal.schema_version == OWNER_JOURNAL_COMPACTION_SCHEMA
        && seal.image_bytes == seal_start as u64
        && sha256_hex(&side[..seal_start]) == seal.image_sha256)
        .then_some(seal)
}

/// Parses complete content: an optional leading checkpoint, then records whose revisions
/// continue from its base (0 without one).
fn parse_journal(content: &[u8]) -> Result<ParsedJournal, OwnerJournalError> {
    let invalid = |operation| OwnerJournalError {
        code: "owner_record_invalid",
        operation,
    };
    let checkpoint_invalid = || OwnerJournalError {
        code: "owner_checkpoint_invalid",
        operation: "validate_owner_checkpoint",
    };
    let text = std::str::from_utf8(content).map_err(|_| invalid("read_owner_file"))?;
    let mut checkpoint = None::<OwnerJournalCheckpoint>;
    let mut checkpoint_end = 0;
    let mut records = Vec::new();
    let mut prefixes = BTreeSet::new();
    let mut digest = Sha256::new();
    let mut bytes = 0_u64;
    for line in text.split_inclusive('\n') {
        let blank = line.trim().is_empty();
        // A prior complete read ends immediately before the next acquired owner's
        // append. Keep record boundaries, not an entry for every blank line.
        let start = (!blank).then(|| (bytes, format!("{:x}", digest.clone().finalize())));
        if let Some(start) = &start {
            prefixes.insert(start.clone());
        }
        let line_start = bytes as usize;
        digest.update(line.as_bytes());
        bytes += line.len() as u64;
        let Some(start) = start else {
            continue;
        };
        let record: RuntimeOwnerRecord = match serde_json::from_str(line) {
            Ok(record) => record,
            Err(_) => {
                let probe = serde_json::from_str::<SchemaProbe>(line)
                    .map_err(|_| invalid("read_owner_file"))?;
                if probe.schema_version != OWNER_JOURNAL_CHECKPOINT_SCHEMA {
                    return Err(invalid("read_owner_file"));
                }
                if checkpoint.is_some() || !records.is_empty() {
                    return Err(checkpoint_invalid());
                }
                checkpoint = Some(serde_json::from_str(line).map_err(|_| checkpoint_invalid())?);
                checkpoint_end = bytes;
                continue;
            }
        };
        let base = checkpoint
            .as_ref()
            .map_or(0, |checkpoint| checkpoint.last_record.revision);
        if !record_valid(&record)
            || base.checked_add(records.len() as u64 + 1) != Some(record.revision)
        {
            return Err(invalid("validate_owner_file"));
        }
        records.push(PhysicalRecord {
            record,
            start,
            line: line_start..bytes as usize,
        });
    }
    let sha256 = format!("{:x}", digest.finalize());
    prefixes.insert((bytes, sha256.clone()));
    Ok(ParsedJournal {
        checkpoint,
        checkpoint_end,
        records,
        prefixes,
        bytes,
        sha256,
    })
}

fn closed_revisions(records: &[PhysicalRecord]) -> Vec<OwnerClosedRevision> {
    records
        .iter()
        .map(|physical| OwnerClosedRevision {
            revision: physical.record.revision,
            active: physical.record.active,
            closed_at_unix_ms: physical.record.closed_at_unix_ms,
            disposition: physical
                .record
                .resource_disposition
                .expect("v2 journal validated"),
        })
        .collect()
}

/// The reader's derived state. A checkpoint seeds the folded blocks, proofs and carried
/// prefixes; a first physical block of its last record's epoch continues that block; every
/// other block is derived from its records exactly as without a checkpoint.
fn derive_journal(
    checkpoint: Option<&OwnerJournalCheckpoint>,
    records: &[PhysicalRecord],
    bytes: u64,
    sha256: &str,
) -> Result<DerivedJournal, OwnerJournalError> {
    let last = records
        .last()
        .map(|physical| &physical.record)
        .or(checkpoint.map(|checkpoint| &checkpoint.last_record))
        .ok_or(OwnerJournalError {
            code: "owner_record_invalid",
            operation: "read_owner_file",
        })?
        .clone();
    let through = last.revision;
    let evidence =
        |subject, block: &OwnerEpochBlock, confirmed_revision, suffix| OwnerEpochCloseEvidence {
            schema_version: OWNER_JOURNAL_SCHEMA.to_owned(),
            subject,
            pid: block.pid,
            started_at_unix_ms: block.started_at_unix_ms,
            first_revision: block.first_revision,
            confirmed_revision,
            final_revision: block.final_revision,
            journal_through_revision: through,
            journal_bytes: bytes,
            journal_sha256: sha256.to_owned(),
            suffix,
        };
    let mut proofs = BTreeMap::new();
    let mut blocks = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut carried = BTreeSet::new();
    let mut block_starts = BTreeSet::new();
    if let Some(checkpoint) = checkpoint {
        seed_checkpoint(checkpoint, &evidence, &mut blocks, &mut proofs)?;
        seen.extend(blocks.keys().copied());
        carried.extend(
            checkpoint
                .prefixes
                .iter()
                .map(|prefix| (prefix.bytes, prefix.sha256.clone())),
        );
    }
    let mut start = 0;
    while start < records.len() {
        let first = &records[start].record;
        let end = start
            + records[start..]
                .iter()
                .take_while(|physical| physical.record.owner_epoch == first.owner_epoch)
                .count();
        let epoch = &records[start..end];
        let final_record = &records[end - 1].record;
        let continued = checkpoint
            .filter(|checkpoint| {
                start == 0 && checkpoint.last_record.owner_epoch == first.owner_epoch
            })
            .and_then(|_| blocks.get(&first.owner_epoch).copied());
        let (block, carried_proof) = match continued {
            // M1-M2: the folded part of this block continues in the physical records.
            Some(summary) => (
                OwnerEpochBlock {
                    legacy: summary.legacy
                        || epoch
                            .iter()
                            .any(|physical| physical.record.schema_version != OWNER_JOURNAL_SCHEMA),
                    consistent: summary.consistent
                        && epoch.iter().all(|physical| {
                            physical.record.schema_version == OWNER_JOURNAL_SCHEMA
                                && physical.record.pid == summary.pid
                                && physical.record.started_at_unix_ms == summary.started_at_unix_ms
                        })
                        && (summary.last_active || !first.active)
                        && !epoch
                            .windows(2)
                            .any(|pair| !pair[0].record.active && pair[1].record.active),
                    confirmed_closed: summary.confirmed_closed
                        || epoch.iter().any(|physical| {
                            physical.record.resource_disposition
                                == Some(OwnerResourceDisposition::ConfirmedClosed)
                        }),
                    last_active: final_record.active,
                    last_disposition: final_record.resource_disposition,
                    final_revision: final_record.revision,
                    ..summary
                },
                proofs.remove(&first.owner_epoch),
            ),
            None => {
                block_starts.insert(records[start].start.clone());
                let unique = seen.insert(first.owner_epoch);
                if !unique {
                    proofs.remove(&first.owner_epoch);
                }
                let consistent = unique
                    && epoch.iter().all(|physical| {
                        physical.record.schema_version == OWNER_JOURNAL_SCHEMA
                            && physical.record.pid == first.pid
                            && physical.record.started_at_unix_ms == first.started_at_unix_ms
                    })
                    && !epoch
                        .windows(2)
                        .any(|pair| !pair[0].record.active && pair[1].record.active);
                (
                    OwnerEpochBlock {
                        legacy: epoch
                            .iter()
                            .any(|physical| physical.record.schema_version != OWNER_JOURNAL_SCHEMA),
                        consistent,
                        confirmed_closed: epoch.iter().any(|physical| {
                            physical.record.resource_disposition
                                == Some(OwnerResourceDisposition::ConfirmedClosed)
                        }),
                        last_active: final_record.active,
                        last_disposition: final_record.resource_disposition,
                        pid: first.pid,
                        started_at_unix_ms: first.started_at_unix_ms,
                        first_revision: first.revision,
                        final_revision: final_record.revision,
                    },
                    None,
                )
            }
        };
        blocks.insert(first.owner_epoch, block);
        if block.consistent {
            // M3: the suffix from the last positive close, which may begin in the checkpoint.
            let positive = epoch.iter().rposition(|physical| {
                physical.record.resource_disposition
                    == Some(OwnerResourceDisposition::ConfirmedClosed)
            });
            let candidate = match (positive, carried_proof) {
                (Some(positive), _) => Some((
                    epoch[positive].record.revision,
                    closed_revisions(&epoch[positive..]),
                )),
                (None, Some(proof)) => Some((
                    proof.confirmed_revision,
                    proof
                        .suffix
                        .into_iter()
                        .chain(closed_revisions(epoch))
                        .collect(),
                )),
                (None, None) => None,
            };
            if let Some((confirmed_revision, suffix)) = candidate {
                let proof = evidence(first.owner_epoch, &block, confirmed_revision, suffix);
                // A valid journal with no positive close suffix remains unknown, not permission.
                if proof.validate().is_ok() {
                    proofs.insert(first.owner_epoch, proof);
                }
            }
        }
        start = end;
    }
    Ok(DerivedJournal {
        last,
        blocks,
        proofs,
        carried,
        block_starts,
    })
}

/// Validates a checkpoint against this read and seeds its blocks and proofs. A carried
/// proof takes this read's bounds (M4) and must validate: an invalid one is an error.
fn seed_checkpoint(
    checkpoint: &OwnerJournalCheckpoint,
    evidence: &impl Fn(
        OwnerEpoch,
        &OwnerEpochBlock,
        u64,
        Vec<OwnerClosedRevision>,
    ) -> OwnerEpochCloseEvidence,
    blocks: &mut BTreeMap<OwnerEpoch, OwnerEpochBlock>,
    proofs: &mut BTreeMap<OwnerEpoch, OwnerEpochCloseEvidence>,
) -> Result<(), OwnerJournalError> {
    let invalid = || OwnerJournalError {
        code: "owner_checkpoint_invalid",
        operation: "validate_owner_checkpoint",
    };
    let last = &checkpoint.last_record;
    if checkpoint.schema_version != OWNER_JOURNAL_CHECKPOINT_SCHEMA
        || checkpoint.predecessor_bytes > OWNER_JOURNAL_LIMIT
        || !lowercase_sha256(&checkpoint.predecessor_sha256)
        || !record_valid(last)
        || last.revision == 0
        || checkpoint
            .epochs
            .windows(2)
            .any(|pair| pair[0].owner_epoch >= pair[1].owner_epoch)
        || checkpoint
            .prefixes
            .windows(2)
            .any(|pair| (pair[0].bytes, &pair[0].sha256) >= (pair[1].bytes, &pair[1].sha256))
        || checkpoint
            .prefixes
            .iter()
            .any(|prefix| prefix.bytes > OWNER_JOURNAL_LIMIT || !lowercase_sha256(&prefix.sha256))
    {
        return Err(invalid());
    }
    for entry in &checkpoint.epochs {
        if entry.pid == 0
            || entry.started_at_unix_ms == 0
            || entry.first_revision == 0
            || entry.first_revision > entry.final_revision
            || entry.final_revision > last.revision
            || !entry.legacy && entry.last_disposition.is_none()
        {
            return Err(invalid());
        }
        let block = OwnerEpochBlock {
            legacy: entry.legacy,
            consistent: entry.consistent,
            confirmed_closed: entry.confirmed_closed,
            last_active: entry.last_active,
            last_disposition: entry.last_disposition,
            pid: entry.pid,
            started_at_unix_ms: entry.started_at_unix_ms,
            first_revision: entry.first_revision,
            final_revision: entry.final_revision,
        };
        if let Some(proof) = &entry.proof {
            let evidence = evidence(
                entry.owner_epoch,
                &block,
                proof.confirmed_revision,
                proof.suffix.clone(),
            );
            if !entry.consistent
                || entry.legacy
                || !entry.confirmed_closed
                || proof.suffix.last().is_none_or(|tail| {
                    tail.active != entry.last_active
                        || Some(tail.disposition) != entry.last_disposition
                })
                || evidence.validate().is_err()
            {
                return Err(invalid());
            }
            proofs.insert(entry.owner_epoch, evidence);
        }
        blocks.insert(entry.owner_epoch, block);
    }
    let summary = blocks.get(&last.owner_epoch).ok_or_else(invalid)?;
    if summary.final_revision != last.revision
        || summary.last_active != last.active
        || summary.last_disposition != last.resource_disposition
        || summary.consistent
            && (summary.pid != last.pid || summary.started_at_unix_ms != last.started_at_unix_ms)
    {
        return Err(invalid());
    }
    Ok(())
}

/// The compaction self-check: same last record and revision, same blocks, and the same
/// proofs apart from their read bounds.
fn same_state(left: &DerivedJournal, right: &DerivedJournal) -> bool {
    let unbounded = |proofs: &BTreeMap<OwnerEpoch, OwnerEpochCloseEvidence>| {
        proofs
            .iter()
            .map(|(epoch, proof)| {
                (
                    *epoch,
                    OwnerEpochCloseEvidence {
                        journal_through_revision: 0,
                        journal_bytes: 0,
                        journal_sha256: String::new(),
                        ..proof.clone()
                    },
                )
            })
            .collect::<Vec<_>>()
    };
    matches!(
        (serde_json::to_vec(&left.last), serde_json::to_vec(&right.last)),
        (Ok(left_last), Ok(right_last)) if left_last == right_last
    ) && left.last.revision == right.last.revision
        && left.blocks == right.blocks
        && unbounded(&left.proofs) == unbounded(&right.proofs)
}
