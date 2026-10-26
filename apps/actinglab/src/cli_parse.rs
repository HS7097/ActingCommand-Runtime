use crate::flag_values::split_csv;
use crate::{
    CaptureBackendChoice, CliError, ErrorKind, GlobalOptions, Invocation, TouchBackendChoice,
    package_cli,
};
use actingcommand_runtime_client::ChannelVia;
use std::path::PathBuf;

/// ROUT-L RL-T: the options a command declares as its own under the name of a global option. Given
/// after the command's words, such an option and its value stay in the command's arguments; given
/// before them, the option is the global one. `standard-pack compose` takes `--instance
/// <alias>=<game>/<server>` once per bound instance (contracts/standard-pack-schedule.md, "Tools").
const COMMAND_OPTIONS: [(&[&str], &[&str]); 1] = [(&["standard-pack", "compose"], &["--instance"])];

/// Whether `option`, read after the words `rest` gathered so far, is the command's own.
fn command_option(rest: &[String], option: &str) -> bool {
    COMMAND_OPTIONS.iter().any(|(words, options)| {
        options.contains(&option)
            && rest.len() >= words.len()
            && rest.iter().zip(words.iter()).all(|(word, own)| word == own)
    })
}

pub(super) fn parse_invocation<I>(
    args: I,
    json_default: bool,
) -> Result<Invocation, (String, bool, CliError)>
where
    I: IntoIterator,
    I::Item: Into<String>,
{
    let mut global = GlobalOptions {
        json: json_default,
        ..Default::default()
    };
    let raw = args.into_iter().map(Into::into).collect::<Vec<_>>();
    let mut rest = Vec::new();
    let mut index = 0usize;

    while index < raw.len() {
        match raw[index].as_str() {
            option if command_option(&rest, option) => {
                rest.push(option.to_owned());
                index += 1;
                rest.push(require_raw(&raw, index, option)?);
            }
            "--json" => global.json = true,
            "--run-root" => {
                index += 1;
                global.run_root = Some(PathBuf::from(require_raw(&raw, index, "--run-root")?));
            }
            "--instance" => {
                index += 1;
                global.instance = Some(require_raw(&raw, index, "--instance")?);
            }
            "--instances" => {
                index += 1;
                global.instances = Some(split_csv(&require_raw(&raw, index, "--instances")?));
            }
            "--profile" => {
                index += 1;
                global.profile = Some(require_raw(&raw, index, "--profile")?);
            }
            "--resource-root" => {
                index += 1;
                global.resource_root =
                    Some(PathBuf::from(require_raw(&raw, index, "--resource-root")?));
            }
            // Workflow #389 U-2: the counter that forwarded the work (model section 3.2 rule 7).
            "--via" => {
                index += 1;
                let value = require_raw(&raw, index, "--via")?;
                global.via = Some(match value.as_str() {
                    "mcp" => ChannelVia::Mcp,
                    "ui" => ChannelVia::Ui,
                    _ => {
                        return Err((
                            "help".to_string(),
                            global.json,
                            CliError::usage(format!("--via must be mcp or ui, got {value}")),
                        ));
                    }
                });
            }
            "--dry-run" => global.dry_run = true,
            "--verbose" => global.verbose = true,
            "--quiet" => global.quiet = true,
            "--game" => {
                index += 1;
                global.game = Some(require_raw(&raw, index, "--game")?);
            }
            "--server" => {
                index += 1;
                global.server = Some(require_raw(&raw, index, "--server")?);
            }
            "--runtime-endpoint" => {
                index += 1;
                global.runtime_endpoint = Some(require_raw(&raw, index, "--runtime-endpoint")?);
            }
            "--capture-backend" | "--backend" => {
                index += 1;
                let value = require_raw(&raw, index, raw[index - 1].as_str())?;
                global.capture_backend =
                    Some(CaptureBackendChoice::parse(&value).map_err(|err| {
                        (
                            "help".to_string(),
                            global.json,
                            CliError::usage(err.to_string()),
                        )
                    })?);
            }
            "--touch-backend" => {
                index += 1;
                let value = require_raw(&raw, index, "--touch-backend")?;
                global.touch_backend = Some(TouchBackendChoice::parse(&value).map_err(|err| {
                    (
                        "help".to_string(),
                        global.json,
                        CliError::usage(err.to_string()),
                    )
                })?);
            }
            "--require-session" => {
                return Err((
                    "help".to_string(),
                    global.json,
                    CliError::usage(
                        "--require-session was retired; ActingLab clients use the resident Runtime",
                    ),
                ));
            }
            "--version" => global.version = true,
            other => rest.push(other.to_string()),
        }
        index += 1;
    }

    let (command, args) = if rest.is_empty() {
        (vec!["help".to_string()], Vec::new())
    } else {
        command_path_and_args(rest.clone())
    };
    let command_name = match command.as_slice() {
        [group, action]
            if group == "package" && matches!(action.as_str(), "build-task" | "build-pack") =>
        {
            Some(command.join(" "))
        }
        [group, action] if group == "resource" && action == "convert" => Some(command.join(" ")),
        [group] if group == "record" => args
            .first()
            .filter(|action| matches!(action.as_str(), "build-task" | "promote" | "publish"))
            .map(|action| format!("record {action}")),
        [group, action] if group == "session" && action == "record" => args
            .first()
            .filter(|action| matches!(action.as_str(), "build-task" | "promote" | "publish"))
            .map(|action| format!("session record {action}")),
        _ => None,
    };
    if let Some(command_name) = command_name {
        return Err((
            command_name.clone(),
            global.json,
            CliError::new(
                ErrorKind::UsageValidation,
                "resource_production_retired",
                format!(
                    "{command_name} is retired; author a content directory under packs/<package_id>/ or use record start/mark/stop, then package validate/bundle"
                ),
                &[],
            ),
        ));
    }

    let (command, args) = if global.version
        && !package_cli::is_offline_command(&rest)
        && rest.first().is_none_or(|group| group != "scheduling")
    {
        (vec!["version".to_string()], rest)
    } else {
        (command, args)
    };
    let command_name = command.join(" ");
    Ok(Invocation {
        global,
        command,
        args,
        command_name,
    })
}

fn require_raw(
    raw: &[String],
    index: usize,
    name: &str,
) -> Result<String, (String, bool, CliError)> {
    raw.get(index).cloned().ok_or_else(|| {
        (
            "unknown".to_string(),
            true,
            CliError::usage(format!("missing value for {name}")),
        )
    })
}

fn command_path_and_args(rest: Vec<String>) -> (Vec<String>, Vec<String>) {
    let top = rest[0].clone();
    let path_len = match top.as_str() {
        "config" | "env" | "lab" | "package" | "operation" | "control" | "scheduler"
        | "runtime" | "resource" | "run" | "report" | "session" | "ledger" | "scheduling"
        | "standard-pack" => rest.get(1).map(|_| 2).unwrap_or(1),
        _ => 1,
    };
    let command = rest.iter().take(path_len).cloned().collect::<Vec<_>>();
    let args = rest.into_iter().skip(path_len).collect::<Vec<_>>();
    (command, args)
}
