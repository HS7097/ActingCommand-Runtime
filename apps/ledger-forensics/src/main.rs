// SPDX-License-Identifier: AGPL-3.0-only

fn main() {
    // One-off (to be reverted): Workflow #375 R5c evidence.
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() == 5 && args[1] == "--state-root" && args[3] == "r5c-frames-debug" {
        let now = args[4].parse::<u64>().unwrap_or(0);
        match actingcommand_ledger_forensics::r5c_frames_debug(std::path::Path::new(&args[2]), now)
        {
            Ok(text) => {
                println!("{text}");
                return;
            }
            Err(error) => {
                eprintln!("r5c-frames-debug: {error}");
                std::process::exit(1);
            }
        }
    }
    if let Err(error) = actingcommand_contract::process_installation() {
        eprintln!("FATAL actingledger: {error}");
        std::process::exit(1);
    }
    if let Err(error) = actingledger::run_env() {
        eprintln!("actingledger: {error}");
        std::process::exit(1);
    }
}
