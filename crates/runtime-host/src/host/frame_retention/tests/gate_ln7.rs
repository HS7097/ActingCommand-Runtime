// SPDX-License-Identifier: AGPL-3.0-only

//! LN-7 (#422, LEAN-L §3.5; A6, the host side). At a sweep start the cleaner reads the frame view
//! of the frames present in the live frame store (`frame_retention_view_live`): a frame whose file
//! left the store before the sweep is not listed, so it is neither removed nor counted absent; a
//! frame held by a viewer is busy and retried by a later sweep; the frames this process removed
//! leave its `handled` set at the next sweep start. A formal writer ledger and its artifact store
//! share one temporary state root (the fixture of `gate_h5g13.rs`); every timestamp is a constant.

use super::super::{FrameRetention, SweepReport};
use super::gate_h5g13::{CAPTURED, Fixture, NOW, artifact_store, formal_writer};
use actingcommand_contract::IdentifierIssuer;
use actingcommand_ledger::{FrameRetentionSwitches, GlobalLedger, Sha256SecretFingerprinter};
use std::path::Path;

/// One whole sweep: the cleaner's rounds from a sweep start until the sweep completes, and the
/// report it printed (none for an idle sweep after the first).
fn sweep(
    retention: &mut FrameRetention,
    ledger: &GlobalLedger,
    root: &Path,
) -> Option<SweepReport> {
    for _ in 0..8 {
        let report = retention
            .maintain(ledger, root, NOW, &|| false, &mut |warning| {
                panic!("the cleaner warns about no frame: {warning:?}")
            })
            .expect("every round returns Ok");
        if retention.sweep.is_none() {
            return report;
        }
    }
    panic!("the sweep did not complete within 8 rounds");
}

#[test]
fn ln7_a_sweep_start_lists_only_the_frames_present_in_the_store() {
    let temp = tempfile::tempdir().expect("state root");
    let ledger = formal_writer(temp.path());
    let fingerprinter = Sha256SecretFingerprinter::new(b"gate-ln7-fixture").expect("fingerprinter");
    let ids = IdentifierIssuer::new().expect("issuer");
    let owner = *ids.mint_owner_epoch().expect("owner").transport();
    let fixture = Fixture {
        ledger: &ledger,
        fingerprinter: &fingerprinter,
        ids: &ids,
        owner,
        instance: ids.mint_instance_id().expect("instance"),
    };
    let artifacts = artifact_store(temp.path(), &ids, owner);
    fixture.record_owner();
    let [absent, held, due] = [
        fixture.observe(&artifacts, CAPTURED[0], b"frame that left the store"),
        fixture.observe(&artifacts, CAPTURED[1], b"frame held by a viewer"),
        fixture.observe(&artifacts, CAPTURED[2], b"frame due for removal"),
    ];
    std::fs::remove_file(&absent.material).expect("the frame leaves the store");
    #[cfg(windows)]
    let viewer = {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&held.material)
            .expect("viewer")
    };

    let mut retention = FrameRetention::new(NOW, FrameRetentionSwitches::default());
    let first =
        sweep(&mut retention, &ledger, artifacts.root()).expect("the first sweep prints its line");
    // The frame that left the store is not listed: neither among the sweep's frames nor absent.
    assert_eq!(
        (first.frames, first.absent, first.rescanned_absent),
        (2, 0, 0)
    );
    #[cfg(windows)]
    assert_eq!((first.busy, first.deleted), (1, 1));
    #[cfg(not(windows))]
    assert_eq!((first.busy, first.deleted), (0, 2));
    assert!(!due.material.exists(), "the due frame is removed");
    assert!(retention.handled.contains(&due.reference.artifact_id));

    // The next sweep start lists the store again: the removed frame left it and leaves `handled`,
    // and the frame the viewer held is removed now.
    #[cfg(windows)]
    drop(viewer);
    retention.last_sweep_start = None;
    let second = sweep(&mut retention, &ledger, artifacts.root());
    assert!(!retention.handled.contains(&due.reference.artifact_id));
    #[cfg(windows)]
    {
        let second = second.expect("the retry prints its line");
        assert_eq!((second.frames, second.deleted, second.busy), (1, 1, 0));
    }
    #[cfg(not(windows))]
    assert!(second.is_none(), "nothing is left to sweep");
    assert!(
        !held.material.exists(),
        "the held frame is removed by a later sweep"
    );
    ledger
        .check_writer_health()
        .expect("the writer stays healthy");
}
