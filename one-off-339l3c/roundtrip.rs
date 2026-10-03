// SPDX-License-Identifier: AGPL-3.0-only
// One-off (to be reverted), Workflow #339 L3c: parses each recording.json with this build's
// LabRecording and writes it back the way the recording store does (pretty JSON and a
// newline), then compares the bytes with the file.
use actingcommand_lab::LabRecording;

fn main() {
    let mut failures = 0_u32;
    for path in std::env::args().skip(1) {
        let original = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                println!("ROUNDTRIP|{path}|read_failed|{error}");
                failures += 1;
                continue;
            }
        };
        let text = String::from_utf8_lossy(&original);
        match serde_json::from_str::<LabRecording>(&text) {
            Ok(recording) => match serde_json::to_vec_pretty(&recording) {
                Ok(mut bytes) => {
                    bytes.push(b'\n');
                    let identical = bytes == original;
                    println!(
                        "ROUNDTRIP|{path}|parsed|identical={identical}|bytes={}|rewritten_bytes={}",
                        original.len(),
                        bytes.len()
                    );
                    if !identical {
                        failures += 1;
                    }
                }
                Err(error) => {
                    println!("ROUNDTRIP|{path}|encode_failed|{error}");
                    failures += 1;
                }
            },
            Err(error) => {
                println!("ROUNDTRIP|{path}|parse_failed|{error}");
                failures += 1;
            }
        }
    }
    std::process::exit(if failures == 0 { 0 } else { 1 });
}
