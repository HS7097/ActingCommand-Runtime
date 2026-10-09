// SPDX-License-Identifier: AGPL-3.0-only

fn main() {
    if let Err(error) = actingcommand_contract::process_installation() {
        eprintln!("FATAL actingledger: {error}");
        std::process::exit(1);
    }
    if let Err(error) = actingledger::run_env() {
        eprintln!("actingledger: {error}");
        std::process::exit(1);
    }
}
