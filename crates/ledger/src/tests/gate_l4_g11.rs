// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #382 L-4 G11 (test plan v2.2 L-4, ledger key 17): the owner journal compaction of
// Workflow #191 O (`contracts/owner-journal.md`, "Checkpoint", "Compaction", "Side file"), on
// the journal's bytes in a tempdir. Journals of 2-6 records over 1-3 epochs, written as
// OwnerGuard writes them:
// - a fold (S1) derives from its image the same last record, revision, blocks and proofs
//   (read bounds aside) as from the file it replaces, keeps the last record's bytes unchanged,
//   writes nothing, and carries every block start, so each bound an earlier startup sealed is
//   still supported; its side file is the image and one seal line; a second fold continues
//   the checkpoint and still carries those bounds;
// - recovering from the side file (R1-R4) before, during and after the image is written
//   leaves a journal that derives the same state: before the write it is left as it is, an
//   interrupted write is redone, a residue is cut, a later append is kept and a torn one goes
//   to the ordinary tail recovery. An empty journal beside a side file is unrecoverable.

use super::*;
use actingcommand_contract::IdentifierIssuer;
use std::fs::OpenOptions;
use std::path::Path;

/// The first epoch's start; later epochs start 10 s apart.
const STARTED_AT_UNIX_MS: u64 = 1_800_000_000_000;
/// How long after its start an epoch closes.
const CLOSED_AFTER_MS: u64 = 1_000;

/// One owner epoch of a fixture journal: the process that held it.
#[derive(Clone, Copy)]
struct Epoch {
    owner_epoch: OwnerEpoch,
    pid: u32,
    started_at_unix_ms: u64,
}

/// A journal built record by record as OwnerGuard appends them (acquire, active instances,
/// confirmed close of the resources, close), revisions from 1.
#[derive(Clone)]
struct Journal {
    /// The instances an owner may hold, sorted as a record keeps them.
    instances: Vec<InstanceId>,
    records: Vec<RuntimeOwnerRecord>,
}

impl Journal {
    fn new(issuer: &IdentifierIssuer) -> Self {
        let mut instances = (0..5)
            .map(|_| *issuer.mint_instance_id().expect("instance id").transport())
            .collect::<Vec<_>>();
        instances.sort_unstable();
        Self {
            instances,
            records: Vec::new(),
        }
    }

    fn append(
        mut self,
        epoch: &Epoch,
        active: bool,
        instances: usize,
        disposition: OwnerResourceDisposition,
    ) -> Self {
        let revision = u64::try_from(self.records.len()).expect("record count") + 1;
        self.records.push(RuntimeOwnerRecord {
            schema_version: OWNER_JOURNAL_SCHEMA.to_owned(),
            revision,
            owner_epoch: epoch.owner_epoch,
            pid: epoch.pid,
            started_at_unix_ms: epoch.started_at_unix_ms,
            active,
            active_instances: self.instances[..instances].to_vec(),
            closed_at_unix_ms: (!active).then_some(epoch.started_at_unix_ms + CLOSED_AFTER_MS),
            resource_disposition: Some(disposition),
            closed_by: None,
        });
        self
    }

    /// The acquire: an active record with the instances it took over.
    fn start(self, epoch: &Epoch, instances: usize) -> Self {
        self.append(epoch, true, instances, OwnerResourceDisposition::None)
    }

    /// An in-epoch change of the active instances.
    fn instances(self, epoch: &Epoch, instances: usize) -> Self {
        self.append(epoch, true, instances, OwnerResourceDisposition::None)
    }

    /// The running owner confirmed its resources closed.
    fn confirmed_closed(self, epoch: &Epoch, instances: usize) -> Self {
        self.append(
            epoch,
            true,
            instances,
            OwnerResourceDisposition::ConfirmedClosed,
        )
    }

    /// The formal close.
    fn close(self, epoch: &Epoch) -> Self {
        self.append(epoch, false, 0, OwnerResourceDisposition::None)
    }

    fn line(record: &RuntimeOwnerRecord) -> Vec<u8> {
        let mut line = serde_json::to_vec(record).expect("encode an owner record");
        line.push(b'\n');
        line
    }

    fn bytes(&self) -> Vec<u8> {
        self.records.iter().flat_map(Self::line).collect()
    }

    /// The bound each later startup read the journal to: the start of every epoch's block but
    /// the first, where that startup's acquire then appended.
    fn startup_bounds(&self) -> Vec<usize> {
        let mut bounds = Vec::new();
        let mut offset = 0;
        for pair in self.records.windows(2) {
            offset += Self::line(&pair[0]).len();
            if pair[0].owner_epoch != pair[1].owner_epoch {
                bounds.push(offset);
            }
        }
        bounds
    }

    /// The running owner's next in-epoch record after the last one.
    fn next_record(&self) -> RuntimeOwnerRecord {
        let last = self.records.last().expect("a last record");
        RuntimeOwnerRecord {
            revision: last.revision + 1,
            active_instances: self.instances[..1].to_vec(),
            ..last.clone()
        }
    }
}

fn epochs(issuer: &IdentifierIssuer) -> [Epoch; 3] {
    [0_u32, 1, 2].map(|index| Epoch {
        owner_epoch: *issuer.mint_owner_epoch().expect("owner epoch").transport(),
        pid: 4_000 + index,
        started_at_unix_ms: STARTED_AT_UNIX_MS + u64::from(index) * 10_000,
    })
}

/// The table: 2-6 records over 1-3 epochs, each ending in the running epoch's latest record.
fn journals(issuer: &IdentifierIssuer) -> Vec<(&'static str, Journal)> {
    let [a, b, c] = epochs(issuer);
    let empty = Journal::new(issuer);
    vec![
        (
            "one epoch, two records",
            empty.clone().start(&a, 0).instances(&a, 2),
        ),
        (
            "one epoch, five records",
            empty
                .clone()
                .start(&a, 0)
                .instances(&a, 1)
                .instances(&a, 3)
                .instances(&a, 5)
                .instances(&a, 4),
        ),
        (
            "two epochs, four records",
            empty
                .clone()
                .start(&a, 0)
                .confirmed_closed(&a, 0)
                .close(&a)
                .start(&b, 0),
        ),
        (
            "three epochs, six records",
            empty
                .start(&a, 5)
                .close(&a)
                .start(&b, 5)
                .confirmed_closed(&b, 5)
                .close(&b)
                .start(&c, 5),
        ),
    ]
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn open(path: &Path) -> File {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap_or_else(|error| panic!("open {}: {error}", path.display()))
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

fn read(path: &Path, row: &str) -> RuntimeOwnerJournal {
    RuntimeOwnerJournal::read_locked(&mut open(path))
        .unwrap_or_else(|error| panic!("{row}: read: {}", error.code.as_str()))
}

fn compact(path: &Path, row: &str) -> OwnerJournalCompaction {
    match RuntimeOwnerJournal::compaction_locked(&mut open(path)) {
        Ok(Some(compaction)) => compaction,
        Ok(None) => panic!("{row}: a journal of two or more records folds"),
        Err(error) => panic!("{row}: fold: {}", error.code.as_str()),
    }
}

fn recover(path: &Path, side: &[u8], row: &str) -> OwnerCompactionRecovery {
    RuntimeOwnerJournal::recover_compaction_locked(&mut open(path), side)
        .unwrap_or_else(|error| panic!("{row}: recovery: {}", error.code.as_str()))
}

/// The proofs with their read bounds set aside.
fn unbounded(journal: &RuntimeOwnerJournal) -> Vec<(OwnerEpoch, OwnerEpochCloseEvidence)> {
    journal
        .proofs
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
        .collect()
}

/// The same last record, revision, blocks and proofs (read bounds aside).
fn assert_same_journal(row: &str, read: &RuntimeOwnerJournal, expected: &RuntimeOwnerJournal) {
    assert_eq!(
        serde_json::to_value(read.last()).expect("encode the last record"),
        serde_json::to_value(expected.last()).expect("encode the last record"),
        "{row}: last record"
    );
    assert_eq!(
        read.through_revision, expected.through_revision,
        "{row}: revision"
    );
    assert_eq!(read.blocks, expected.blocks, "{row}: blocks");
    assert_eq!(unbounded(read), unbounded(expected), "{row}: proofs");
}

/// What each later startup sealed against the journal it read to its bound: the proof of each
/// epoch that read proved, and the observation of each epoch it saw without one.
fn sealed_at_startups(
    directory: &Path,
    journal: &[u8],
    bounds: &[usize],
    row: &str,
) -> Vec<(OwnerEpochCloseEvidence, bool)> {
    let mut sealed = Vec::new();
    for bound in bounds {
        let path = directory.join(format!("startup-{bound}.lock"));
        write(&path, &journal[..*bound]);
        let read = read(&path, row);
        for epoch in read.blocks.keys() {
            match read.proofs.get(epoch) {
                Some(proof) => sealed.push((proof.clone(), true)),
                None => sealed.push((read.observation(*epoch), false)),
            }
        }
    }
    sealed
}

fn assert_supports(
    row: &str,
    read: &RuntimeOwnerJournal,
    sealed: &[(OwnerEpochCloseEvidence, bool)],
) {
    for (evidence, proven) in sealed {
        if *proven {
            assert!(
                read.supports(evidence),
                "{row}: the sealed proof of {:?} at {} bytes",
                evidence.subject,
                evidence.journal_bytes
            );
        } else {
            assert!(
                read.supports_observation(evidence),
                "{row}: the sealed observation of {:?} at {} bytes",
                evidence.subject,
                evidence.journal_bytes
            );
        }
    }
}

#[test]
fn gate_l4_g11_a_fold_derives_the_journal_it_replaces_and_keeps_every_sealed_bound() {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let directory = tempfile::tempdir().expect("journal directory");
    for (index, (row, journal)) in journals(&issuer).into_iter().enumerate() {
        let original = journal.bytes();
        let path = directory.path().join(format!("journal-{index}.lock"));
        write(&path, &original);
        let read_original = read(&path, row);
        let sealed =
            sealed_at_startups(directory.path(), &original, &journal.startup_bounds(), row);
        assert_supports(row, &read_original, &sealed);

        let compaction = compact(&path, row);
        assert_eq!(
            std::fs::read(&path).expect("journal"),
            original,
            "{row}: a fold writes nothing"
        );
        let image = compaction.image();
        let kept = Journal::line(journal.records.last().expect("a last record"));
        assert_eq!(
            image.split_inclusive(|byte| *byte == b'\n').count(),
            2,
            "{row}: a checkpoint line and the last record"
        );
        assert!(image.ends_with(&kept), "{row}: the last record's bytes");
        assert_eq!(compaction.kept_bytes(), kept.len() as u64, "{row}");

        let side = compaction.side_file();
        assert!(side.starts_with(image), "{row}: the side file's image");
        let seal: OwnerJournalCompactionSeal =
            serde_json::from_slice(&side[image.len()..]).expect("the side file's seal line");
        assert!(side.ends_with(b"\n"), "{row}: the seal line is complete");
        assert_eq!(
            seal.schema_version, OWNER_JOURNAL_COMPACTION_SCHEMA,
            "{row}"
        );
        assert_eq!(seal.image_bytes, image.len() as u64, "{row}");
        assert_eq!(seal.image_sha256, sha256(image), "{row}");
        assert_eq!(
            seal.image_through_revision, read_original.through_revision,
            "{row}"
        );
        assert_eq!(seal.predecessor_bytes, original.len() as u64, "{row}");
        assert_eq!(seal.predecessor_sha256, sha256(&original), "{row}");

        let image_path = directory.path().join(format!("journal-{index}-image.lock"));
        write(&image_path, image);
        let read_image = read(&image_path, row);
        assert_same_journal(row, &read_image, &read_original);
        assert_eq!(read_image.uncompacted_bytes(), kept.len() as u64, "{row}");
        assert_supports(row, &read_image, &sealed);
        assert!(
            matches!(
                RuntimeOwnerJournal::compaction_locked(&mut open(&image_path)),
                Ok(None)
            ),
            "{row}: one physical record is not folded"
        );
    }
}

#[test]
fn gate_l4_g11_a_second_fold_continues_the_checkpoint_and_keeps_every_sealed_bound() {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let directory = tempfile::tempdir().expect("journal directory");
    let [a, b, c] = epochs(&issuer);
    let journal = Journal::new(&issuer)
        .start(&a, 5)
        .close(&a)
        .start(&b, 5)
        .confirmed_closed(&b, 5)
        .close(&b)
        .start(&c, 5);
    let original = journal.bytes();
    let sealed = sealed_at_startups(
        directory.path(),
        &original,
        &journal.startup_bounds(),
        "first fold",
    );
    let path = directory.path().join("owner.lock");
    write(&path, &original);
    let first = compact(&path, "first fold");

    // The running epoch goes on after the first fold: its instances change and it confirms
    // its resources closed.
    let after = journal.instances(&c, 3).confirmed_closed(&c, 3);
    let mut folded_once = first.image().to_vec();
    for record in &after.records[6..] {
        folded_once.extend(Journal::line(record));
    }
    write(&path, &folded_once);
    let read_folded_once = read(&path, "folded once");
    assert_supports("folded once", &read_folded_once, &sealed);

    let second = compact(&path, "second fold");
    write(&path, second.image());
    let read_folded_twice = read(&path, "second fold");
    assert_same_journal("second fold", &read_folded_twice, &read_folded_once);
    assert_supports("second fold", &read_folded_twice, &sealed);
    assert!(
        second.image().ends_with(&Journal::line(
            after.records.last().expect("the last record")
        )),
        "second fold: the last record's bytes"
    );
}

#[test]
fn gate_l4_g11_recovery_from_the_side_file_before_and_after_the_image_is_written_derives_the_same_journal()
 {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let directory = tempfile::tempdir().expect("journal directory");
    let mut shorter_images = 0;
    let mut longer_images = 0;
    for (index, (row, journal)) in journals(&issuer).into_iter().enumerate() {
        let original = journal.bytes();
        let path = directory.path().join(format!("journal-{index}.lock"));
        write(&path, &original);
        let read_original = read(&path, row);
        let compaction = compact(&path, row);
        let image = compaction.image().to_vec();
        let side = compaction.side_file().to_vec();
        if image.len() < original.len() {
            shorter_images += 1;
        } else {
            longer_images += 1;
        }

        // S3 writes the image at offset 0 and then sets the length: `written` bytes of it are
        // over the replaced file.
        let interrupted = |written: usize| {
            let mut file = original.clone();
            if file.len() < written {
                file.resize(written, 0);
            }
            file[..written].copy_from_slice(&image[..written]);
            file
        };
        // The image and the replaced file start alike; a write within that is no change.
        let common = original
            .iter()
            .zip(&image)
            .take_while(|(left, right)| left == right)
            .count();
        let mut cases = vec![(
            "the image not yet written",
            original.clone(),
            OwnerCompactionRecovery::Stale,
        )];
        for written in [common + 1, (common + 1 + image.len()) / 2, image.len() - 1] {
            cases.push((
                "the image partly written",
                interrupted(written),
                OwnerCompactionRecovery::Rewritten,
            ));
        }
        cases.push((
            "the image written, the length not yet set",
            interrupted(image.len()),
            if image.len() < original.len() {
                OwnerCompactionRecovery::Truncated
            } else {
                OwnerCompactionRecovery::Completed
            },
        ));
        cases.push((
            "the image written",
            image.clone(),
            OwnerCompactionRecovery::Completed,
        ));
        for (case, before, expected) in cases {
            let label = format!("{row}, {case}");
            write(&path, &before);
            assert_eq!(recover(&path, &side, &label), expected, "{label}");
            let after = std::fs::read(&path).expect("journal");
            if expected == OwnerCompactionRecovery::Stale {
                assert_eq!(after, before, "{label}: the journal is left as it is");
            } else {
                assert_eq!(after, image, "{label}: the journal is the image");
            }
            assert_same_journal(&label, &read(&path, &label), &read_original);
        }

        // The owner appended after the fold: the record is kept.
        let next = Journal::line(&journal.next_record());
        let mut appended = image.clone();
        appended.extend(&next);
        write(&path, &appended);
        let label = format!("{row}, a record appended after the image");
        assert_eq!(
            recover(&path, &side, &label),
            OwnerCompactionRecovery::Completed,
            "{label}"
        );
        assert_eq!(std::fs::read(&path).expect("journal"), appended, "{label}");
        assert_eq!(
            read(&path, &label).last().map(|record| record.revision),
            Some(read_original.through_revision + 1),
            "{label}"
        );

        // A torn append goes to the ordinary tail recovery of the next read.
        let mut torn = image.clone();
        torn.extend(&next[..next.len() / 2]);
        write(&path, &torn);
        let label = format!("{row}, a torn append after the image");
        assert_eq!(
            recover(&path, &side, &label),
            OwnerCompactionRecovery::Completed,
            "{label}"
        );
        assert_eq!(std::fs::read(&path).expect("journal"), torn, "{label}");
        assert_same_journal(&label, &read(&path, &label), &read_original);
        assert_eq!(std::fs::read(&path).expect("journal"), image, "{label}");

        // R1: a side file without its seal line (S2 did not finish) leaves the journal as it is.
        write(&path, &original);
        let label = format!("{row}, a side file without its seal");
        assert_eq!(
            recover(&path, &side[..image.len()], &label),
            OwnerCompactionRecovery::Stale,
            "{label}"
        );
        assert_eq!(std::fs::read(&path).expect("journal"), original, "{label}");
    }
    assert!(
        shorter_images > 0 && longer_images > 0,
        "the table folds to shorter ({shorter_images}) and longer ({longer_images}) images"
    );

    // Anything else, an empty journal beside a side file included, is unrecoverable.
    let (row, journal) = journals(&issuer).remove(0);
    let path = directory.path().join("empty.lock");
    write(&path, &journal.bytes());
    let side = compact(&path, row).side_file().to_vec();
    write(&path, b"");
    match RuntimeOwnerJournal::recover_compaction_locked(&mut open(&path), &side) {
        Err(error) => assert_eq!(
            error.code.as_str(),
            SharedCode::OwnerCompactionUnrecoverable.code().as_str()
        ),
        Ok(recovery) => panic!("an empty journal beside a side file recovered as {recovery:?}"),
    }
    assert_eq!(
        std::fs::read(&path).expect("journal"),
        b"",
        "the journal is never cut or rewritten on a guess"
    );
}
