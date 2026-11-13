// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #453 S10 (`contracts/emulator-control.md`, "Offline instance discovery"): `actingd
//! discover [--config <file> [--install-root <root>]] [--mumu-root <dir>]`, the emulator instances
//! of this machine while no Runtime runs. Read only: no state root, no owner lock, no ledger, no
//! socket, and no file is written; of the configuration file it reads only `mumu_root`,
//! `allow_env_overrides` and the entries' binding keys, each by the rule a start applies, and
//! `--install-root` is checked by check-config's rule. It resolves the emulator installation as
//! check-config does (the configured `mumu_root`, the environment rung only under the file's
//! `allow_env_overrides`, then the resolver's own rungs), runs only the vendor tool's two reads the
//! Runtime runs at its start (`version`, then `info -v all`, each within
//! `MUMU_MANAGER_COMMAND_TIMEOUT`), and answers the host's instance shape
//! (`RuntimeDiscoveredInstance`), each instance with the alias of the configuration file's entry
//! its binding key names, plus the installation it found and how (`mumu_root {path, source}`) and
//! the provider's version.

use super::*;
use actingcommand_contract::{RuntimeDiscoveredInstance, RuntimeInstanceDiscovery};
use actingcommand_device::{
    DeviceError, DiscoveredMumuInstance, MumuManagerReads, MumuManagerSource, NemuResolutionReason,
    SystemMumuManagerReads, discover_mumu_installation,
};
use config::InstanceBindingKey;
use serde_json::json;

/// The report's schema (`{schema_version, outcome, report?}`).
pub(super) const SCHEMA_VERSION: &str = "actingcommand.actingd.discover.v1";

/// `mumu_root.source` of an installation the configuration file's `mumu_root` names.
const CONFIG_SOURCE: &str = "config";
/// `mumu_root.source` of an installation `--mumu-root` names.
const ARGUMENT_SOURCE: &str = "argument";
/// The developer text of a command line this subcommand does not take.
const USAGE: &str = "actingd discover takes --config <file>, --install-root <root> only with \
     --config, and --mumu-root <absolute folder>, each at most once with a value";

/// What the command line names.
#[derive(Default)]
struct Options {
    config: Option<PathBuf>,
    install_root: Option<PathBuf>,
    mumu_root: Option<PathBuf>,
}

/// `actingd discover` on the vendor tool itself.
pub(super) fn run(arguments: Vec<std::ffi::OsString>) -> Result<Reported, ActingdError> {
    run_with(&arguments, &mut SystemMumuManagerReads)
}

/// `actingd discover` with the vendor tool's two reads `reads` (a test injects its own).
/// `discover_completed` with the report (`report`); a refusal is the configuration's (check-config's
/// codes and stages), `invalid_arguments` for a command line it does not take, or the discovery's:
/// `mumu_manager_version_unsupported` for a version the policy floor refuses or cannot read,
/// `instance_discovery_unavailable` for anything else (no installation, a tool that did not run,
/// exited non-zero, ran past its bound, or answered what the contract refuses), with the device
/// failure's outcome (check-config's `config::device_failure_outcome`) as its cause.
pub(super) fn run_with(
    arguments: &[std::ffi::OsString],
    reads: &mut dyn MumuManagerReads,
) -> Result<Reported, ActingdError> {
    let options = parse(arguments)?;
    let inputs = match &options.config {
        Some(config_path) => read_config(config_path, options.install_root.as_deref())?,
        // No file, no override rule: the environment rung takes no part.
        None => config::DiscoveryInputs {
            mumu_root: None,
            env_nemu_folder: None,
            bindings: Vec::new(),
        },
    };
    let config::DiscoveryInputs {
        mumu_root: configured,
        env_nemu_folder,
        bindings,
    } = inputs;
    // `--mumu-root` is the first rung, before the file's `mumu_root`.
    let (explicit, explicit_source) = match options.mumu_root {
        Some(root) => (Some(root), ARGUMENT_SOURCE),
        None => (configured, CONFIG_SOURCE),
    };
    let discovered =
        discover_mumu_installation(explicit.as_deref(), env_nemu_folder.as_deref(), reads)
            .map_err(|error| refused(&error, explicit.as_deref()))?;
    let source = match discovered.report.source {
        MumuManagerSource::ExplicitRoot => explicit_source,
        other => check_config::mumu_root_source(other),
    };
    let listed = &discovered.report.instances;
    let instances = listed
        .iter()
        .map(|instance| RuntimeDiscoveredInstance {
            bound_alias: bound_alias(&bindings, listed, instance),
            instance_index: instance.instance_index,
            instance_name: instance.instance_name.clone(),
            adb_host: instance.adb_host.clone(),
            adb_port: instance.adb_port,
            running: instance.running,
            android_version: instance.android_version.clone(),
        })
        .collect::<Vec<_>>();
    // The host's rule for an answered discovery (`host/instance_discovery.rs`): a list the
    // contract refuses is `instance_discovery_unavailable`.
    RuntimeInstanceDiscovery::validate_instances(&instances).map_err(|error| {
        let code = SharedCode::InstanceDiscoveryUnavailable;
        ActingdError::refused(
            code,
            Outcome::new(code).caused_by(Outcome::new(error.code())),
        )
    })?;
    let instances = serde_json::to_value(&instances)
        .map_err(|_| ActingdError::process(SharedCode::SerializationFailed))?;
    let report = json!({
        "mumu_root": {
            "path": discovered.install_root.to_string_lossy(),
            "source": source,
        },
        "provider_version": discovered.report.version.to_string(),
        "instances": instances,
    });
    Ok(Reported::new(Outcome::new(ActingdCode::DiscoverCompleted)).with("report", report))
}

/// The configuration file at `config_path`, loaded as check-config loads it (`config_load`'s
/// refusals), `install_root` checked by check-config's rule (`check_config_install_root_invalid`),
/// and the members discovery reads, each by its rule (`config_assemble`'s refusals); see
/// `ActingdConfigFile::discovery_inputs`. Nothing under the state root is read.
fn read_config(
    config_path: &Path,
    install_root: Option<&Path>,
) -> Result<config::DiscoveryInputs, ActingdError> {
    if let Some(root) = install_root {
        check_config::install_root_of(root)?;
    }
    let staged = |code: Code, stage: ActingdLocation| {
        let mut error = ActingdError::config(code);
        error.stage = Some(stage.location());
        error
    };
    let file = config::load_for(
        config_path,
        actingcommand_contract::InstallConfigPurpose::CandidateCheck,
    )
    .map_err(|code| staged(code, ActingdLocation::ConfigLoad))?;
    file.discovery_inputs()
        .map_err(|code| staged(code, ActingdLocation::ConfigAssemble))
}

/// The command line after `discover`: `--config`, `--install-root` (only with `--config`) and
/// `--mumu-root` (an absolute folder), each at most once with a non-empty value; anything else is
/// `invalid_arguments`.
fn parse(arguments: &[std::ffi::OsString]) -> Result<Options, ActingdError> {
    let invalid =
        || ActingdError::config(SharedCode::InvalidArguments).with_detail(USAGE.to_owned());
    let (pairs, remaining) = arguments.get(1..).unwrap_or_default().as_chunks::<2>();
    if !remaining.is_empty() {
        return Err(invalid());
    }
    let mut options = Options::default();
    for pair in pairs {
        let slot = if pair[0] == "--config" {
            &mut options.config
        } else if pair[0] == "--install-root" {
            &mut options.install_root
        } else if pair[0] == "--mumu-root" {
            &mut options.mumu_root
        } else {
            return Err(invalid());
        };
        if slot.is_some() || pair[1].is_empty() {
            return Err(invalid());
        }
        *slot = Some(PathBuf::from(&pair[1]));
    }
    if options.install_root.is_some() && options.config.is_none() {
        return Err(invalid());
    }
    if options
        .mumu_root
        .as_ref()
        .is_some_and(|root| !root.is_absolute())
    {
        return Err(invalid());
    }
    Ok(options)
}

/// The configuration file's entry bound to `instance` by its binding key, as a start binds it
/// (`config/provider_startup.rs`): `instance_index` names the index; `instance_name` names the one
/// listed instance of that exact name, none when several carry it (a start refuses that entry as
/// ambiguous). Of several entries naming one instance, the first by alias, as a start names the
/// index of a device group. `None` without a file.
fn bound_alias(
    bindings: &[(String, InstanceBindingKey)],
    listed: &[DiscoveredMumuInstance],
    instance: &DiscoveredMumuInstance,
) -> Option<String> {
    bindings
        .iter()
        .filter(|(_, key)| match key {
            InstanceBindingKey::Index(index) => *index == instance.instance_index,
            InstanceBindingKey::Name(name) => {
                *name == instance.instance_name
                    && listed
                        .iter()
                        .filter(|other| other.instance_name == *name)
                        .count()
                        == 1
            }
        })
        .map(|(alias, _)| alias)
        .min()
        .cloned()
}

/// A discovery the device layer refused, classified as the Runtime's discovery surface classifies
/// it (`DiscoverySpec::discover`, `crates/runtime-host/src/provider.rs`), with the device failure's
/// outcome (`explicit_root` the folder named, when one was) as its cause.
fn refused(error: &DeviceError, explicit_root: Option<&Path>) -> ActingdError {
    let version_refused = matches!(
        error
            .nemu_resolution_context()
            .map(|context| context.reason()),
        Some(
            NemuResolutionReason::ProviderVersionBelowMinimum
                | NemuResolutionReason::ProviderVersionUnparseable
        )
    );
    let code = if version_refused {
        HostCode::MumuManagerVersionUnsupported.code()
    } else {
        SharedCode::InstanceDiscoveryUnavailable.code()
    };
    let cause = config::device_failure_outcome(error, explicit_root);
    ActingdError::refused(code, Outcome::new(code).caused_by(cause))
}
