// SPDX-License-Identifier: AGPL-3.0-only

//! LN-7 (#422, LEAN-L §3.5; A6, the host side). At a pass start the cleaner lists the frames
//! present in the live frame store and asks the ledger about those only (#422 LN-7c Q: by its
//! query, adapted from the writer's view `frame_retention_view_live`, which Q removed): a frame
//! whose file left the store before the pass is not listed, so it is neither removed nor counted
//! absent; a frame held by a viewer is busy and retried by a later pass; the frames this process
//! removed leave its `handled` set at the next pass start. A formal writer ledger and its artifact
//! store share one temporary state root (the fixture of `gate_h5g13.rs`); every timestamp is a
//! constant.

use super::super::{FrameRetention, PassEnd, SweepReport};
use super::gate_h5g13::{
    CAPTURED, Fixture, NOW, SETTINGS, artifact_store, formal_writer, round_clock,
};
use actingcommand_contract::IdentifierIssuer;
use actingcommand_ledger::{FrameRetentionSwitches, GlobalLedger, Sha256SecretFingerprinter};
use std::path::Path;

/// One whole pass: the cleaner's rounds from a pass start until the pass completes, and the
/// report it printed (none for an idle pass after the first). Each pass's clock starts an hour
/// after the one before, past the pass interval.
fn sweep(
    retention: &mut FrameRetention,
    ledger: &GlobalLedger,
    root: &Path,
    first_round: u64,
) -> Option<SweepReport> {
    for round in first_round..first_round + 8 {
        let ended = retention
            .maintain(
                ledger,
                root,
                round_clock(round),
                SETTINGS,
                std::sync::Arc::new(|| false),
                &|| false,
                &mut |warning| panic!("the cleaner warns about no frame: {warning:?}"),
            )
            .expect("every round returns Ok");
        if retention.pass.is_none() {
            return match ended {
                Some(PassEnd::Completed(report)) => Some(report),
                Some(PassEnd::Skipped) => panic!("the frame index covers the ledger"),
                None => None,
            };
        }
    }
    panic!("the pass did not complete within 8 rounds");
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
    assert!(!absent.material.exists(), "the frame left the store");
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
    let first = sweep(&mut retention, &ledger, artifacts.root(), 0)
        .expect("the first sweep prints its line");
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
    let second = sweep(&mut retention, &ledger, artifacts.root(), 60);
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
