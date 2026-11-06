// SPDX-License-Identifier: AGPL-3.0-only

//! `ac-vision-ep-bench` (Workflow #407 GPU-0a, experiment branch only). See the crate docs and
//! `cli.rs` for the command line.

#[cfg(windows)]
fn main() {
    use actingcommand_vision_ep_bench::cli::{Command, parse_args};
    use actingcommand_vision_ep_bench::error::{BenchCode, BenchError};
    use actingcommand_vision_ep_bench::report::ReportStatus;
    use actingcommand_vision_ep_bench::{run, win};
    use std::time::Instant;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match parse_args(&args) {
        Ok(command) => command,
        Err(error) => fail(&error),
    };
    let options = match command {
        Command::ListAdapters => {
            let adapters = win::dxgi_adapters().unwrap_or_else(|error| fail(&error));
            match serde_json::to_string_pretty(&adapters) {
                Ok(text) => {
                    println!("{text}");
                    return;
                }
                Err(error) => fail(&BenchError::new(
                    BenchCode::OutputUnwritable,
                    format!("the adapter list does not serialise: {error}"),
                )),
            }
        }
        Command::Run(options) => *options,
    };
    if let Err(error) = run::check_footprint(&options) {
        fail(&error);
    }
    let started = Instant::now();
    let mut report = run::empty_report(&options);
    let outcome = run::run(&options, &mut report);
    report.wall_clock_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
    if let Err(error) = &outcome {
        report.status = ReportStatus::Failed;
        report.error = Some(error.clone());
    }
    if let Err(error) = run::write_report(&options.out, &report) {
        if let Err(run_error) = &outcome {
            eprintln!("ac-vision-ep-bench: {run_error}");
        }
        fail(&error);
    }
    match outcome {
        Ok(()) => eprintln!(
            "ac-vision-ep-bench: {} finished; report {}",
            options.label,
            options.out.display()
        ),
        Err(error) => {
            eprintln!(
                "ac-vision-ep-bench: {} failed in phase {}; report {}",
                options.label,
                report.phase_at_failure.as_deref().unwrap_or("unknown"),
                options.out.display()
            );
            fail(&error);
        }
    }
}

#[cfg(windows)]
fn fail(error: &actingcommand_vision_ep_bench::error::BenchError) -> ! {
    eprintln!("ac-vision-ep-bench: {error}");
    std::process::exit(error.code.exit_status())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("ac-vision-ep-bench: bench_cli_invalid: the bench runs on Windows only");
    std::process::exit(2)
}
