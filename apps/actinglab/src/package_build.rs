// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome, FlagArgs, GlobalOptions};
use actingcommand_contract::BundleSource;
use actingcommand_lab::{DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES, PackageBundleRequest};
use actingcommand_resource_tooling::{
    PackageLoadingMode, PackageLoadingRequest, StandardPackRootFiles, package_loading,
};
use serde::Serialize;
use serde_json::{Value, json};

/// The flags of `package loading` (Workflow #407 LOAD-1).
pub(crate) const LOADING_FLAGS: [&str; 4] = ["--root-file", "--packs-root", "--write", "--check"];
/// Workflow #434 LAB-5B2: the flags of `package loading` that take no value (Workflow #434
/// LAB-5B4: nor from the next word, but `true` or `false`, `FlagArgs::parse_switches`).
pub(crate) const LOADING_SWITCHES: [&str; 2] = ["--write", "--check"];

/// Workflow #434 LAB-5B3: the flags `package bundle` reads, each with its value. Workflow #434
/// LAB-5B4: its command line is parsed by it (`FlagArgs::parse_reading`); no positional.
pub(crate) const BUNDLE_READS: crate::flag_args::reads::FlagReads =
    crate::flag_args::reads::FlagReads {
        row: "package bundle",
        parts: &[&crate::flag_args::reads::ReadPart {
            options: &[
                "--maintenance",
                "--source-repository",
                "--source-commit",
                "--applications",
                "--packs-root",
                "--out",
                "--pages",
                "--interrupts",
                "--navigation",
                "--schedule",
                "--display-layout",
                "--loading",
                // Workflow #444 NOTICES-3: the package author's notices directory.
                "--notices",
                // Workflow #455 G-R1: the game's goal declaration.
                "--goals",
            ],
            switches: &[],
            refused: &[],
        }],
        positionals: &[],
    };

/// Workflow #288 A2b: `package bundle --applications <file> --packs-root <directory> --out
/// <directory>`, with `--source-repository` and `--source-commit` together or not at all; it
/// writes bundle v4 (Workflow #336 PKG-S5). Workflow #373 NAV-T: `--pages`, `--interrupts` and
/// `--navigation` name the standard-pack root files it checks, cross-checks and lays beside
/// `bundle.json`; ROUT-L RL-1a: so does `--schedule` (`schedule.json`); Workflow #392 DA-4: and
/// `--display-layout` (`display-layout.json`). Workflow #407 LOAD-1: `--loading` names the
/// game's `loading.json`: every pack's loading copy is checked against it, and it is laid beside
/// `bundle.json` with its templates. Workflow #444 NOTICES-3: `--notices` names a directory of
/// the package author's notice texts: every file is held to the notices rules before anything is
/// written, then laid byte for byte into `notices/` beside `bundle.json`. Workflow #455 G-R1:
/// `--goals` names the game's `goals.json`: read after the composition and held to the bundle's
/// root, then laid beside `bundle.json`; a refused file refuses the bundle.
///
/// Workflow #434 LAB-5B3: whether a flag was given is read through `FlagArgs::given`.
pub(super) fn run_bundle(flags: &FlagArgs) -> CliOutcome<Value> {
    let maintenance = if flags.given("--maintenance") {
        use std::io::Read;
        let path = flags.required_path("--maintenance")?;
        let mut bytes = Vec::new();
        std::fs::File::open(&path)
            .and_then(|file| {
                file.take(DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
            })
            .map_err(|error| CliError::usage(format!("{}: {error}", path.display())))?;
        if bytes.len() > DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES {
            return Err(CliError::usage(
                "--maintenance exceeds the package input byte limit",
            ));
        }
        Some(
            serde_json::from_slice(&bytes)
                .map_err(|error| CliError::usage(format!("{}: {error}", path.display())))?,
        )
    } else {
        None
    };
    let source = match (
        flags.given("--source-repository"),
        flags.given("--source-commit"),
    ) {
        (false, false) => None,
        (true, true) => Some(BundleSource {
            repository: flags.required("--source-repository")?,
            commit: flags.required("--source-commit")?,
        }),
        _ => {
            return Err(CliError::usage(
                "--source-repository and --source-commit are given together or not at all",
            ));
        }
    };
    // Workflow #373 NAV-T: a root file flag given without its path is refused, not ignored.
    let root_file = |flag: &str| -> CliOutcome<Option<std::path::PathBuf>> {
        if flags.given(flag) {
            flags.required_path(flag).map(Some)
        } else {
            Ok(None)
        }
    };
    let request = PackageBundleRequest {
        applications: flags.required_path("--applications")?,
        packs_root: flags.required_path("--packs-root")?,
        out: flags.required_path("--out")?,
        source,
        maintenance,
        root_files: StandardPackRootFiles {
            pages: root_file("--pages")?,
            interrupts: root_file("--interrupts")?,
            navigation: root_file("--navigation")?,
            schedule: root_file("--schedule")?,
            display_layout: root_file("--display-layout")?,
            loading: root_file("--loading")?,
            notices: root_file("--notices")?,
            goals: root_file("--goals")?,
        },
    };
    let mut lab = super::env_detection::build_readonly_lab()?;
    serialize_response(lab.package_bundle(request)?)
}

/// Workflow #407 LOAD-1: `package loading --root-file <loading.json> --packs-root <directory>
/// (--write | --check)`, exactly one of the two. `--check` writes nothing; `--write` under the
/// global `--dry-run` is a preview that writes nothing (`dry_run: true`, `written: false`).
pub(super) fn run_loading(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    if let Some(flag) = flags
        .flags
        .keys()
        .find(|flag| !LOADING_FLAGS.contains(&flag.as_str()))
    {
        return Err(CliError::usage(format!(
            "package loading does not accept {flag}"
        )));
    }
    flags.expect_positionals("package loading", 0)?;
    for flag in LOADING_FLAGS {
        if flags.flags.get(flag).is_some_and(|values| values.len() > 1) {
            return Err(CliError::usage(format!(
                "package loading takes {flag} once"
            )));
        }
    }
    for flag in LOADING_SWITCHES {
        if flags.flags.contains_key(flag) && !flags.bool(flag) {
            return Err(CliError::usage(format!("{flag} takes no value")));
        }
    }
    let mode = match (flags.bool("--write"), flags.bool("--check")) {
        (true, false) if global.dry_run => PackageLoadingMode::Preview,
        (true, false) => PackageLoadingMode::Write,
        (false, true) => PackageLoadingMode::Check,
        _ => {
            return Err(CliError::usage(
                "package loading takes exactly one of --write and --check",
            ));
        }
    };
    let response = package_loading(PackageLoadingRequest {
        root_file: flags.required_path("--root-file")?,
        packs_root: flags.required_path("--packs-root")?,
        mode,
    })?;
    let mut data = serialize_response(response)?;
    if mode == PackageLoadingMode::Preview {
        data["dry_run"] = json!(true);
        data["written"] = json!(false);
    }
    Ok(data)
}

fn serialize_response<T: Serialize>(response: T) -> CliOutcome<Value> {
    serde_json::to_value(response)
        .map_err(|error| CliError::device(format!("failed to serialize Lab response: {error}")))
}
