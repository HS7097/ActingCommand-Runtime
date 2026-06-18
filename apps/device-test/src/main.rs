// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_device::{
    DeviceError, DeviceResult, InputBackend, MaaTouchBackend, MaaTouchValidationConfig,
    combine_operation_and_close,
};
use std::env;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
enum DeviceCommand {
    Reset,
    Tap {
        x: i32,
        y: i32,
    },
    LongTap {
        x: i32,
        y: i32,
        duration_ms: u64,
    },
    Swipe {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        duration_ms: u64,
    },
}

fn main() {
    if let Err(err) = run() {
        eprintln!("FATAL: {err}");
        std::process::exit(1);
    }
}

fn run() -> DeviceResult<()> {
    let (config, commands) = parse_args(env::args().skip(1))?;
    let mut backend = MaaTouchBackend::new(config.adb, config.target, config.maatouch);

    println!("Target device: {}", backend.serial());
    let device = backend.connect()?;
    println!("Device state: {}", device.state);
    println!("Device screen: {}", device.screen_size);
    if let Some(handshake) = backend.handshake_info() {
        println!(
            "MaaTouch handshake OK: contacts={} size={}x{} pressure={} pid={}",
            handshake.max_contacts,
            handshake.max_x,
            handshake.max_y,
            handshake.max_pressure,
            handshake.pid
        );
    }

    let operation_result = run_commands(&mut backend, &commands);
    let close_result = backend.close();
    combine_operation_and_close(operation_result, close_result)?;

    println!("PASS");
    Ok(())
}

fn run_commands(backend: &mut MaaTouchBackend, commands: &[DeviceCommand]) -> DeviceResult<()> {
    for command in commands {
        match *command {
            DeviceCommand::Reset => {
                backend.reset()?;
                println!("reset sent");
            }
            DeviceCommand::Tap { x, y } => {
                backend.tap(x, y)?;
                println!("tap sent: x={x} y={y}");
            }
            DeviceCommand::LongTap { x, y, duration_ms } => {
                backend.long_tap(x, y, duration_ms)?;
                println!("longtap sent: x={x} y={y} duration_ms={duration_ms}");
            }
            DeviceCommand::Swipe {
                x1,
                y1,
                x2,
                y2,
                duration_ms,
            } => {
                backend.swipe(x1, y1, x2, y2, duration_ms)?;
                println!("swipe sent: x1={x1} y1={y1} x2={x2} y2={y2} duration_ms={duration_ms}");
            }
        }
    }
    Ok(())
}

fn parse_args<I>(args: I) -> DeviceResult<(MaaTouchValidationConfig, Vec<DeviceCommand>)>
where
    I: IntoIterator<Item = String>,
{
    let mut cfg = MaaTouchValidationConfig::default();
    let mut commands = Vec::new();
    let tokens = args.into_iter().collect::<Vec<_>>();
    let mut index = 0;

    while index < tokens.len() {
        match tokens[index].as_str() {
            "--adb" => {
                cfg.adb.adb_path = next_token(&tokens, &mut index, "--adb")?;
            }
            "--serial" => {
                cfg.target.serial = Some(next_token(&tokens, &mut index, "--serial")?);
            }
            "--host" => {
                cfg.target.host = next_token(&tokens, &mut index, "--host")?;
            }
            "--port" => {
                cfg.target.port = parse_token(&tokens, &mut index, "--port")?;
            }
            "--local" => {
                cfg.maatouch.local_path =
                    PathBuf::from(next_token(&tokens, &mut index, "--local")?);
            }
            "--remote" => {
                cfg.maatouch.remote_path = next_token(&tokens, &mut index, "--remote")?;
            }
            "--no-connect" => {
                cfg.target.connect = false;
                index += 1;
            }
            "--no-push" => {
                cfg.maatouch.push = false;
                index += 1;
            }
            "--command-timeout-ms" => {
                cfg.adb.command_timeout = Duration::from_millis(parse_token(
                    &tokens,
                    &mut index,
                    "--command-timeout-ms",
                )?);
            }
            "--handshake-timeout-ms" => {
                cfg.maatouch.handshake_timeout = Duration::from_millis(parse_token(
                    &tokens,
                    &mut index,
                    "--handshake-timeout-ms",
                )?);
            }
            "--shutdown-timeout-ms" => {
                cfg.maatouch.shutdown_timeout = Duration::from_millis(parse_token(
                    &tokens,
                    &mut index,
                    "--shutdown-timeout-ms",
                )?);
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            "reset" => {
                commands.push(DeviceCommand::Reset);
                index += 1;
            }
            "tap" => {
                index += 1;
                let x = parse_positional(&tokens, &mut index, "tap x")?;
                let y = parse_positional(&tokens, &mut index, "tap y")?;
                commands.push(DeviceCommand::Tap { x, y });
            }
            "longtap" => {
                index += 1;
                let x = parse_positional(&tokens, &mut index, "longtap x")?;
                let y = parse_positional(&tokens, &mut index, "longtap y")?;
                let duration_ms = parse_positional(&tokens, &mut index, "longtap duration_ms")?;
                commands.push(DeviceCommand::LongTap { x, y, duration_ms });
            }
            "swipe" => {
                index += 1;
                let x1 = parse_positional(&tokens, &mut index, "swipe x1")?;
                let y1 = parse_positional(&tokens, &mut index, "swipe y1")?;
                let x2 = parse_positional(&tokens, &mut index, "swipe x2")?;
                let y2 = parse_positional(&tokens, &mut index, "swipe y2")?;
                let duration_ms = parse_positional(&tokens, &mut index, "swipe duration_ms")?;
                commands.push(DeviceCommand::Swipe {
                    x1,
                    y1,
                    x2,
                    y2,
                    duration_ms,
                });
            }
            other => {
                return Err(DeviceError::fatal(format!(
                    "unknown argument or command: {other}"
                )));
            }
        }
    }

    if commands.is_empty() {
        return Err(DeviceError::fatal(
            "missing command: expected reset, tap, longtap, or swipe",
        ));
    }

    Ok((cfg, commands))
}

fn next_token(tokens: &[String], index: &mut usize, name: &str) -> DeviceResult<String> {
    let value_index = *index + 1;
    let value = tokens
        .get(value_index)
        .ok_or_else(|| DeviceError::fatal(format!("missing value for {name}")))?
        .clone();
    *index += 2;
    Ok(value)
}

fn parse_token<T>(tokens: &[String], index: &mut usize, name: &str) -> DeviceResult<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let value = next_token(tokens, index, name)?;
    parse_value(&value, name)
}

fn parse_positional<T>(tokens: &[String], index: &mut usize, name: &str) -> DeviceResult<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let value = tokens
        .get(*index)
        .ok_or_else(|| DeviceError::fatal(format!("missing positional value for {name}")))?;
    let parsed = parse_value(value, name)?;
    *index += 1;
    Ok(parsed)
}

fn parse_value<T>(value: &str, name: &str) -> DeviceResult<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse()
        .map_err(|err| DeviceError::fatal(format!("invalid value for {name}: {err}")))
}

fn print_help() {
    println!(
        "Usage:\n\
         cargo run -p actingcommand-device-test -- [options] reset\n\
         cargo run -p actingcommand-device-test -- [options] tap <x> <y>\n\
         cargo run -p actingcommand-device-test -- [options] longtap <x> <y> <duration_ms>\n\
         cargo run -p actingcommand-device-test -- [options] swipe <x1> <y1> <x2> <y2> <duration_ms>\n\
         \n\
         Multiple commands may be provided in one invocation and will reuse one MaaTouch session.\n\
         Options: --adb --serial --host --port --local --remote --no-connect --no-push \\\n\
         --command-timeout-ms --handshake-timeout-ms --shutdown-timeout-ms"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiple_commands_for_one_session() {
        let (_, commands) = parse_args([
            "reset".to_string(),
            "tap".to_string(),
            "100".to_string(),
            "200".to_string(),
            "longtap".to_string(),
            "300".to_string(),
            "400".to_string(),
            "500".to_string(),
        ])
        .expect("parse");

        assert_eq!(
            commands,
            vec![
                DeviceCommand::Reset,
                DeviceCommand::Tap { x: 100, y: 200 },
                DeviceCommand::LongTap {
                    x: 300,
                    y: 400,
                    duration_ms: 500
                },
            ]
        );
    }
}
