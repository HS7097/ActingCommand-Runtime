// SPDX-License-Identifier: AGPL-3.0-only

use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

type AppResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Clone)]
struct Config {
    adb_path: String,
    serial: Option<String>,
    host: String,
    port: u16,
    local_maatouch: PathBuf,
    remote_maatouch: String,
    connect: bool,
    push: bool,
    tap: bool,
    wake_first: bool,
    wake_x: i32,
    wake_y: i32,
    x: i32,
    y: i32,
    pressure: i32,
    command_timeout: Duration,
    handshake_timeout: Duration,
    shutdown_timeout: Duration,
    post_command_delay: Duration,
    between_tap_delay: Duration,
}

#[derive(Debug, Clone)]
struct HandshakeInfo {
    max_contacts: i32,
    max_x: i32,
    max_y: i32,
    max_pressure: i32,
    pid: String,
}

#[derive(Debug)]
struct CommandOutput {
    stdout: String,
    stderr: String,
}

#[derive(Debug)]
struct CliError(String);

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for CliError {}

fn main() {
    if let Err(err) = run() {
        eprintln!("maatouch-test-rs failed: {err}");
        std::process::exit(1);
    }
}

fn run() -> AppResult<()> {
    let mut cfg = parse_args(env::args().skip(1))?;
    let serial = match cfg.serial.take() {
        Some(value) => value,
        None => format!("{}:{}", cfg.host, cfg.port),
    };

    println!("Target device: {serial}");
    println!("Local MaaTouch: {}", cfg.local_maatouch.display());
    println!("Remote MaaTouch: {}", cfg.remote_maatouch);

    if cfg.push {
        require_file(&cfg.local_maatouch)?;
    }

    if cfg.connect {
        let output =
            run_adb_with_timeout(&cfg.adb_path, &["connect", &serial], cfg.command_timeout)?;
        print_command_output("adb connect", &output);
    }

    verify_device(&cfg, &serial)?;

    if cfg.push {
        push_maatouch(&cfg, &serial)?;
    }

    let info = run_maatouch_session(&cfg, &serial)?;
    println!(
        "MaaTouch handshake OK: contacts={} size={}x{} pressure={} pid={}",
        info.max_contacts, info.max_x, info.max_y, info.max_pressure, info.pid
    );
    println!("PASS");
    Ok(())
}

fn parse_args<I>(args: I) -> AppResult<Config>
where
    I: IntoIterator<Item = String>,
{
    let mut cfg = Config {
        adb_path: "adb".to_string(),
        serial: None,
        host: "127.0.0.1".to_string(),
        port: 16384,
        local_maatouch: default_maatouch_path(),
        remote_maatouch: "/data/local/tmp/maatouch".to_string(),
        connect: true,
        push: true,
        tap: false,
        wake_first: false,
        wake_x: 1200,
        wake_y: 360,
        x: 640,
        y: 360,
        pressure: 50,
        command_timeout: Duration::from_secs(12),
        handshake_timeout: Duration::from_secs(8),
        shutdown_timeout: Duration::from_secs(1),
        post_command_delay: Duration::from_millis(250),
        between_tap_delay: Duration::from_secs(1),
    };

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--adb" => cfg.adb_path = next_value(&mut iter, "--adb")?,
            "--serial" => cfg.serial = Some(next_value(&mut iter, "--serial")?),
            "--host" => cfg.host = next_value(&mut iter, "--host")?,
            "--port" => cfg.port = next_value(&mut iter, "--port")?.parse()?,
            "--local" => cfg.local_maatouch = PathBuf::from(next_value(&mut iter, "--local")?),
            "--remote" => cfg.remote_maatouch = next_value(&mut iter, "--remote")?,
            "--no-connect" => cfg.connect = false,
            "--no-push" => cfg.push = false,
            "--tap" => cfg.tap = true,
            "--wake-first" => cfg.wake_first = true,
            "--wake-x" => cfg.wake_x = next_value(&mut iter, "--wake-x")?.parse()?,
            "--wake-y" => cfg.wake_y = next_value(&mut iter, "--wake-y")?.parse()?,
            "--x" => cfg.x = next_value(&mut iter, "--x")?.parse()?,
            "--y" => cfg.y = next_value(&mut iter, "--y")?.parse()?,
            "--pressure" => cfg.pressure = next_value(&mut iter, "--pressure")?.parse()?,
            "--command-timeout-ms" => {
                cfg.command_timeout =
                    Duration::from_millis(next_value(&mut iter, "--command-timeout-ms")?.parse()?)
            }
            "--handshake-timeout-ms" => {
                cfg.handshake_timeout =
                    Duration::from_millis(next_value(&mut iter, "--handshake-timeout-ms")?.parse()?)
            }
            "--shutdown-timeout-ms" => {
                cfg.shutdown_timeout =
                    Duration::from_millis(next_value(&mut iter, "--shutdown-timeout-ms")?.parse()?)
            }
            "--post-command-delay-ms" => {
                cfg.post_command_delay = Duration::from_millis(
                    next_value(&mut iter, "--post-command-delay-ms")?.parse()?,
                )
            }
            "--between-tap-delay-ms" => {
                cfg.between_tap_delay =
                    Duration::from_millis(next_value(&mut iter, "--between-tap-delay-ms")?.parse()?)
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            _ => return Err(cli_error(format!("unknown argument: {arg}"))),
        }
    }

    Ok(cfg)
}

fn next_value<I>(iter: &mut I, name: &str) -> AppResult<String>
where
    I: Iterator<Item = String>,
{
    iter.next()
        .ok_or_else(|| cli_error(format!("missing value for {name}")))
}

fn print_help() {
    println!(
        "Usage: maatouch-test-rs [--port 16384] [--wake-first --tap --x 1160 --y 541]\n\
         Defaults to reset-only validation on 127.0.0.1:16384."
    );
}

fn default_maatouch_path() -> PathBuf {
    [
        "..",
        "upstream-sources",
        "AzurPilot",
        "bin",
        "MaaTouch",
        "maatouch",
    ]
    .iter()
    .collect()
}

fn require_file(path: &PathBuf) -> AppResult<()> {
    let meta = fs::metadata(path).map_err(|err| {
        cli_error(format!(
            "required MaaTouch file is unavailable at {}: {err}",
            path.display()
        ))
    })?;
    if meta.is_dir() {
        return Err(cli_error(format!(
            "required MaaTouch path is a directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn verify_device(cfg: &Config, serial: &str) -> AppResult<()> {
    let state = run_adb_with_timeout(
        &cfg.adb_path,
        &["-s", serial, "get-state"],
        cfg.command_timeout,
    )
    .map_err(|err| {
        let devices = run_adb_with_timeout(&cfg.adb_path, &["devices", "-l"], cfg.command_timeout)
            .map(|out| out.stdout)
            .unwrap_or_else(|list_err| format!("adb devices -l also failed: {list_err}"));
        cli_error(format!(
            "target device {serial} is not available: {err}\nadb devices -l:\n{devices}"
        ))
    })?;
    let trimmed = state.stdout.trim();
    if trimmed != "device" {
        return Err(cli_error(format!(
            "target device {serial} is not in device state: {trimmed:?}"
        )));
    }

    let size = run_adb_with_timeout(
        &cfg.adb_path,
        &["-s", serial, "shell", "wm", "size"],
        cfg.command_timeout,
    )?;
    println!("Device state: {trimmed}");
    println!("Device screen: {}", size.stdout.trim());
    Ok(())
}

fn push_maatouch(cfg: &Config, serial: &str) -> AppResult<()> {
    let local = cfg.local_maatouch.to_string_lossy().to_string();
    let output = run_adb_with_timeout(
        &cfg.adb_path,
        &["-s", serial, "push", &local, &cfg.remote_maatouch],
        cfg.command_timeout,
    )?;
    print_command_output("adb push", &output);

    let output = run_adb_with_timeout(
        &cfg.adb_path,
        &["-s", serial, "shell", "chmod", "755", &cfg.remote_maatouch],
        cfg.command_timeout,
    )?;
    print_command_output("adb chmod", &output);
    Ok(())
}

fn run_maatouch_session(cfg: &Config, serial: &str) -> AppResult<HandshakeInfo> {
    let mut child = Command::new(&cfg.adb_path)
        .args([
            "-s",
            serial,
            "shell",
            &format!("CLASSPATH={}", cfg.remote_maatouch),
            "app_process",
            "/",
            "com.shxyke.MaaTouch.App",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| cli_error(format!("failed to start MaaTouch app_process: {err}")))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| cli_error("failed to open MaaTouch stdout".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| cli_error("failed to open MaaTouch stderr".to_string()))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| cli_error("failed to open MaaTouch stdin".to_string()))?;

    let stderr_text = Arc::new(Mutex::new(String::new()));
    let stderr_copy = Arc::clone(&stderr_text);
    let stderr_thread = thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut text = String::new();
        let _ = reader.read_to_string(&mut text);
        if let Ok(mut target) = stderr_copy.lock() {
            *target = text;
        }
    });

    let stdout_reader = Arc::new(Mutex::new(BufReader::new(stdout)));
    let handshake_reader = Arc::clone(&stdout_reader);
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = match handshake_reader.lock() {
            Ok(mut reader) => read_handshake(&mut reader),
            Err(err) => Err(cli_error(format!(
                "failed to lock MaaTouch stdout reader: {err}"
            ))),
        };
        let _ = tx.send(result);
    });

    let info = match rx.recv_timeout(cfg.handshake_timeout) {
        Ok(result) => result.map_err(|err| attach_stderr(err, &stderr_text))?,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            stop_child(&mut child, cfg.shutdown_timeout);
            let _ = stderr_thread.join();
            return Err(attach_stderr(
                cli_error(format!(
                    "timed out after {:?} waiting for MaaTouch handshake",
                    cfg.handshake_timeout
                )),
                &stderr_text,
            ));
        }
        Err(err) => {
            stop_child(&mut child, cfg.shutdown_timeout);
            let _ = stderr_thread.join();
            return Err(attach_stderr(
                cli_error(format!("failed to receive MaaTouch handshake: {err}")),
                &stderr_text,
            ));
        }
    };

    send_reset(&mut stdin)?;
    if cfg.tap {
        if cfg.wake_first {
            send_tap(&mut stdin, cfg.wake_x, cfg.wake_y, cfg.pressure)?;
            println!(
                "Wake tap sent: x={} y={} pressure={}",
                cfg.wake_x, cfg.wake_y, cfg.pressure
            );
            thread::sleep(cfg.between_tap_delay);
        }
        send_tap(&mut stdin, cfg.x, cfg.y, cfg.pressure)?;
        println!(
            "Tap sent: x={} y={} pressure={}",
            cfg.x, cfg.y, cfg.pressure
        );
    } else {
        println!("Tap skipped: pass --tap to send one down/up touch event.");
    }

    thread::sleep(cfg.post_command_delay);
    drop(stdin);
    stop_child(&mut child, cfg.shutdown_timeout);
    drop(stdout_reader);
    let _ = stderr_thread.join();

    let stderr = stderr_text
        .lock()
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if !stderr.is_empty() {
        if stderr == "Killed" {
            println!("MaaTouch process stopped after validation (stderr: Killed).");
        } else {
            eprintln!("MaaTouch stderr:\n{stderr}");
        }
    }

    Ok(info)
}

fn read_handshake<R: Read>(reader: &mut BufReader<R>) -> AppResult<HandshakeInfo> {
    loop {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .map_err(|err| cli_error(format!("failed to read MaaTouch handshake: {err}")))?;
        if read == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "Aborted" {
            return Err(cli_error(
                "MaaTouch reported Aborted during startup".to_string(),
            ));
        }
        if let Some(rest) = line.strip_prefix("^ ") {
            return parse_version_and_pid(rest, reader);
        }
    }
    Err(cli_error(
        "MaaTouch stdout ended before handshake was received".to_string(),
    ))
}

fn parse_version_and_pid<R: Read>(
    version: &str,
    reader: &mut BufReader<R>,
) -> AppResult<HandshakeInfo> {
    let values = version
        .split_whitespace()
        .map(str::parse::<i32>)
        .collect::<Result<Vec<_>, _>>()?;
    if values.len() != 4 {
        return Err(cli_error(format!(
            "invalid MaaTouch version line: ^ {version}"
        )));
    }

    let mut pid_line = String::new();
    reader.read_line(&mut pid_line).map_err(|err| {
        cli_error(format!(
            "failed to read MaaTouch pid line after version: {err}"
        ))
    })?;
    let pid_line = pid_line.trim();
    let pid = pid_line
        .strip_prefix("$ ")
        .ok_or_else(|| cli_error(format!("unexpected MaaTouch pid line: {pid_line:?}")))?
        .trim()
        .to_string();

    Ok(HandshakeInfo {
        max_contacts: values[0],
        max_x: values[1],
        max_y: values[2],
        max_pressure: values[3],
        pid,
    })
}

fn send_reset(stdin: &mut ChildStdin) -> AppResult<()> {
    stdin.write_all(b"r\nc\n")?;
    stdin.flush()?;
    println!("Reset command sent.");
    Ok(())
}

fn send_tap(stdin: &mut ChildStdin, x: i32, y: i32, pressure: i32) -> AppResult<()> {
    writeln!(stdin, "d 0 {x} {y} {pressure}")?;
    writeln!(stdin, "c")?;
    stdin.flush()?;
    thread::sleep(Duration::from_millis(80));
    writeln!(stdin, "u 0")?;
    writeln!(stdin, "c")?;
    stdin.flush()?;
    Ok(())
}

fn run_adb_with_timeout(
    adb_path: &str,
    args: &[&str],
    timeout: Duration,
) -> AppResult<CommandOutput> {
    let mut child = Command::new(adb_path)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| cli_error(format!("failed to spawn adb {}: {err}", args.join(" "))))?;

    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            let stdout = read_pipe_to_string(child.stdout.take())?;
            let stderr = read_pipe_to_string(child.stderr.take())?;
            if status.success() {
                return Ok(CommandOutput { stdout, stderr });
            }
            return Err(cli_error(format!(
                "adb {} failed with {status}\nstdout:\n{stdout}\nstderr:\n{stderr}",
                args.join(" ")
            )));
        }
        if started.elapsed() >= timeout {
            stop_child(&mut child, Duration::from_millis(500));
            let stdout = read_pipe_to_string(child.stdout.take())?;
            let stderr = read_pipe_to_string(child.stderr.take())?;
            return Err(cli_error(format!(
                "adb {} timed out after {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
                args.join(" "),
                timeout
            )));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn read_pipe_to_string<R: Read>(pipe: Option<R>) -> io::Result<String> {
    let mut text = String::new();
    if let Some(mut reader) = pipe {
        reader.read_to_string(&mut text)?;
    }
    Ok(text)
}

fn stop_child(child: &mut Child, timeout: Duration) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    let _ = child.kill();
    let started = Instant::now();
    while started.elapsed() < timeout {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    eprintln!("warning: child process did not exit within {:?}", timeout);
}

fn print_command_output(label: &str, output: &CommandOutput) {
    let stdout = output.stdout.trim();
    if !stdout.is_empty() {
        println!("{label} stdout: {stdout}");
    }
    let stderr = output.stderr.trim();
    if !stderr.is_empty() {
        eprintln!("{label} stderr: {stderr}");
    }
}

fn attach_stderr(
    err: Box<dyn Error + Send + Sync>,
    stderr: &Arc<Mutex<String>>,
) -> Box<dyn Error + Send + Sync> {
    let stderr = stderr
        .lock()
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if stderr.is_empty() {
        err
    } else {
        cli_error(format!("{err}\nMaaTouch stderr:\n{stderr}"))
    }
}

fn cli_error(message: String) -> Box<dyn Error + Send + Sync> {
    Box::new(CliError(message))
}
