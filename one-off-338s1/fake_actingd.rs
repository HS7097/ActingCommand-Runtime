// SPDX-License-Identifier: AGPL-3.0-only

// One-off (to be reverted): a stand-in for `actingcommand-actingd.exe suspended --config <path>`
// that prints the report file named by ONEOFF_FAKE_REPORT, so the one-off can give ac_diagnose
// report rows of a chosen size and state root. Built with plain rustc inside the job only.

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().map(String::as_str) != Some("suspended") {
        eprintln!("fake actingd: expected `suspended`, got {arguments:?}");
        std::process::exit(2);
    }
    let path = std::env::var("ONEOFF_FAKE_REPORT").expect("ONEOFF_FAKE_REPORT");
    let report = std::fs::read_to_string(path).expect("read the fake report");
    println!("{}", report.trim());
}
