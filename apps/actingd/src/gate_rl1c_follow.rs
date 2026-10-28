// SPDX-License-Identifier: AGPL-3.0-only

// ROUT-L RL-1c follow-up (#411 6444982862 L2; 0.12 wave 1 slice W1-9): `actingd suspended` run by
// hand, without `--deadline-ms`, reads the ledger under a deadline too: the default one from the
// process's start, where a caller's run reads under the deadline it passes. Before, a run by hand
// had none and its read was bounded by nothing.

use super::*;
use std::ffi::OsString;
use std::time::Duration;

fn arguments(words: &[&str]) -> Vec<OsString> {
    words.iter().map(OsString::from).collect()
}

fn options(words: &[&str]) -> suspended::Options {
    let Ok(options) = suspended::options(&arguments(words)) else {
        panic!("actingd {words:?} is refused");
    };
    options
}

#[test]
fn rl1c_follow_l2_a_run_by_hand_reads_under_the_default_deadline() {
    let by_hand = options(&["suspended", "--config", "a.json"]);
    assert_eq!(by_hand.config, Some(PathBuf::from("a.json")));
    assert_eq!(
        by_hand.read_for,
        Duration::from_millis(suspended::DEFAULT_READ_MS),
        "a run by hand reads under the default deadline"
    );

    let called = options(&["suspended", "--deadline-ms", "22000", "--config", "a.json"]);
    assert_eq!(called.config, Some(PathBuf::from("a.json")));
    assert_eq!(
        called.read_for,
        Duration::from_millis(22_000),
        "a caller's run reads under its own deadline"
    );
}
