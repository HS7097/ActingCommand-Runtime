// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_device::{
    DeviceError, DeviceResult, MaaTouchValidationConfig, TouchAction, validate_maatouch,
};
use std::env;
use std::path::PathBuf;
use std::time::Duration;

fn main() {
    if let Err(err) = run() {
        eprintln!("FATAL: {err}");
        std::process::exit(1);
    }
}

fn run() -> DeviceResult<()> {
    let config = parse_args(env::args().skip(1))?;
    let serial = config.target.resolved_serial();

    println!("Target device: {serial}");
    println!("Local MaaTouch: {}", config.maatouch.local_path.display());
    println!("Remote MaaTouch: {}", config.maatouch.remote_path);

    let result = validate_maatouch(&config)?;
    println!("Device state: {}", result.device.state);
    println!("Device screen: {}", result.device.screen_size);
    println!(
        "MaaTouch handshake OK: contacts={} size={}x{} pressure={} pid={}",
        result.handshake.max_contacts,
        result.handshake.max_x,
        result.handshake.max_y,
        result.handshake.max_pressure,
        result.handshake.pid
    );
    println!("PASS");
    Ok(())
}

fn parse_args<I>(args: I) -> DeviceResult<MaaTouchValidationConfig>
where
    I: IntoIterator<Item = String>,
{
    let mut cfg = MaaTouchValidationConfig::default();
    let mut tap_enabled = false;
    let mut wake_first = false;
    let mut wake = TouchAction::new(1200, 360, 50);
    let mut tap = TouchAction::new(640, 360, 50);

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--adb" => cfg.adb.adb_path = next_value(&mut iter, "--adb")?,
            "--serial" => cfg.target.serial = Some(next_value(&mut iter, "--serial")?),
            "--host" => cfg.target.host = next_value(&mut iter, "--host")?,
            "--port" => cfg.target.port = parse_value(&mut iter, "--port")?,
            "--local" => cfg.maatouch.local_path = PathBuf::from(next_value(&mut iter, "--local")?),
            "--remote" => cfg.maatouch.remote_path = next_value(&mut iter, "--remote")?,
            "--no-connect" => cfg.target.connect = false,
            "--no-push" => cfg.maatouch.push = false,
            "--tap" => tap_enabled = true,
            "--wake-first" => wake_first = true,
            "--wake-x" => wake.x = parse_value(&mut iter, "--wake-x")?,
            "--wake-y" => wake.y = parse_value(&mut iter, "--wake-y")?,
            "--x" => tap.x = parse_value(&mut iter, "--x")?,
            "--y" => tap.y = parse_value(&mut iter, "--y")?,
            "--pressure" => {
                let pressure = parse_value(&mut iter, "--pressure")?;
                wake.pressure = pressure;
                tap.pressure = pressure;
            }
            "--command-timeout-ms" => {
                cfg.adb.command_timeout =
                    Duration::from_millis(parse_value(&mut iter, "--command-timeout-ms")?)
            }
            "--handshake-timeout-ms" => {
                cfg.maatouch.handshake_timeout =
                    Duration::from_millis(parse_value(&mut iter, "--handshake-timeout-ms")?)
            }
            "--shutdown-timeout-ms" => {
                cfg.maatouch.shutdown_timeout =
                    Duration::from_millis(parse_value(&mut iter, "--shutdown-timeout-ms")?)
            }
            "--post-command-delay-ms" => {
                cfg.touch_plan.post_command_delay =
                    Duration::from_millis(parse_value(&mut iter, "--post-command-delay-ms")?)
            }
            "--between-tap-delay-ms" => {
                cfg.touch_plan.between_tap_delay =
                    Duration::from_millis(parse_value(&mut iter, "--between-tap-delay-ms")?)
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            _ => return Err(DeviceError::fatal(format!("unknown argument: {arg}"))),
        }
    }

    if wake_first {
        cfg.touch_plan.wake_first = Some(wake);
    }
    if tap_enabled {
        cfg.touch_plan.tap = Some(tap);
    }

    Ok(cfg)
}

fn next_value<I>(iter: &mut I, name: &str) -> DeviceResult<String>
where
    I: Iterator<Item = String>,
{
    iter.next()
        .ok_or_else(|| DeviceError::fatal(format!("missing value for {name}")))
}

fn parse_value<I, T>(iter: &mut I, name: &str) -> DeviceResult<T>
where
    I: Iterator<Item = String>,
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let value = next_value(iter, name)?;
    value
        .parse()
        .map_err(|err| DeviceError::fatal(format!("invalid value for {name}: {err}")))
}

fn print_help() {
    println!(
        "Usage: cargo run -p actingcommand-device-test -- [--port 16384] [--wake-first --tap --x 1160 --y 541]\n\
         Defaults to reset-only MaaTouch validation on 127.0.0.1:16384.\n\
         MaaTouch binary default: external-tools/maatouch/maatouch"
    );
}
