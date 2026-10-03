// SPDX-License-Identifier: AGPL-3.0-only

//! One-off (to be reverted), Workflow #336 L6 evidence item 2: `compare_failure_frames` called
//! directly on 1280x720 frames the one-off workflow synthesizes. Copied into
//! `crates/execution-kernel/tests/` by the workflow only; every printed line starts with
//! `FRAMES|`.

use actingcommand_execution_kernel::{
    FAILURE_FRAME_CELL_DELTA, FAILURE_FRAME_MAX_CHANGED_MILLI, compare_failure_frames,
    failure_frame_ccoeff,
};

#[test]
fn oneoff_336l6_compare_failure_frames_on_synthesized_frames() {
    let directory = std::env::var("ONEOFF_L6_FRAMES").expect("ONEOFF_L6_FRAMES");
    let cases = std::fs::read_to_string(format!("{directory}/cases.txt")).expect("cases.txt");
    println!(
        "FRAMES|constants|cell_delta={FAILURE_FRAME_CELL_DELTA}|max_changed_milli={FAILURE_FRAME_MAX_CHANGED_MILLI}"
    );
    let mut count = 0;
    for line in cases.lines().filter(|line| !line.trim().is_empty()) {
        let mut parts = line.split('|');
        let (Some(name), Some(previous), Some(current)) = (parts.next(), parts.next(), parts.next())
        else {
            panic!("case line {line}");
        };
        let previous = std::fs::read(format!("{directory}/{previous}")).expect("previous frame");
        let current = std::fs::read(format!("{directory}/{current}")).expect("current frame");
        let compared = compare_failure_frames(&previous, &current);
        let ccoeff = failure_frame_ccoeff(&previous, &current);
        match compared {
            Ok(compared) => println!(
                "FRAMES|{name}|{}|{}|{}|{}|{}",
                compared.verdict.as_str(),
                compared.reason.unwrap_or("-"),
                compared
                    .changed_cells_milli
                    .map_or_else(|| "-".to_owned(), |value| value.to_string()),
                compared
                    .digest_mean_milli
                    .map_or_else(|| "-".to_owned(), |value| value.to_string()),
                match &ccoeff {
                    Ok(value) => format!("{value:.3}"),
                    Err(error) => format!("error:{}", error.reason()),
                }
            ),
            Err(error) => println!("FRAMES|{name}|unavailable|{}|-|-|-", error.reason()),
        }
        count += 1;
    }
    let broken = compare_failure_frames(b"not a png", b"not a png either");
    println!(
        "FRAMES|png_invalid|{}",
        broken.map_or_else(|error| error.reason().to_owned(), |_| "compared".to_owned())
    );
    assert!(count > 0, "no frame cases");
}
