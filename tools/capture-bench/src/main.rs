// SPDX-License-Identifier: AGPL-3.0-only

//! `ac-capture-bench` (#402 step 1, C2). Subcommands:
//!
//! - `run --plan <plan.toml> --run-dir <D:\.Claude\exp402\runs\<run_id>>`: one bench run on the
//!   real backends. `--fake` uses the fake backend (D2, no device), with `--fake-hang-close
//!   <instance>:<N|S>` and `--fake-no-self-exit` for the close-never-returns rehearsal; each fake
//!   S open starts a `fake-child` process outside the job object (the stand-in for S's host-side
//!   `adb shell` child, D2 item 10), and the instances with `apk = true` get the fake APK.
//! - `view --viewer-id <n> --run-dir <dir> --ttl-ms <ms> [--slow-fps <f>]
//!   [--stop-renew-after-ms <ms>] [--unsubscribe-after-ms <ms>] [--gdi]`: a viewer (the bench
//!   starts it); `--gdi` draws each frame in a minimal window (`A_submit`, not presentation).
//! - `fake-child -s <serial> shell <server arguments …> [--max-seconds <s>]`: the stand-in for
//!   S's host-side `adb shell` child in D2 (outside the job object); its command line carries
//!   `-s <serial> shell` and `scid=<hex>` as the real child's does (§3.11.5 checklist item 1).
//!
//! The bench writes nothing to its standard streams (§3.10 item 3); the result is the exit code
//! and the run directory.

use actingcommand_capture_bench::apk::{AdbApkConnector, ApkConnector, FakeApks};
use actingcommand_capture_bench::backend::{BackendFactory, DeviceSideCheck, MaterialFactory};
use actingcommand_capture_bench::clock::{Clock, SystemClock};
use actingcommand_capture_bench::fake::{
    CloseHang, FakeApkSet, FakeCheckScript, FakeDevice, FakeMaterials, FakeScript, FaultAt, Latch,
    ProcessChildren, fake_apks,
};
use actingcommand_capture_bench::metrics::{HostMetrics, HostProbes};
use actingcommand_capture_bench::model::{BackendKind, RunResult};
use actingcommand_capture_bench::plan::Plan;
use actingcommand_capture_bench::real::{RealDevice, TimingSink};
use actingcommand_capture_bench::runner::{Env, run};
use actingcommand_capture_bench::safety::{HostStartChecks, Roots};
use actingcommand_capture_bench::supervisor::{FixedLimits, ProcessExit};
use actingcommand_capture_bench::viewer::{ChildViewers, ViewArgs, view_main};
use actingcommand_capture_bench::winproc::Job;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

fn options(arguments: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut map = BTreeMap::new();
    let mut index = 0;
    while index < arguments.len() {
        let name = arguments[index]
            .strip_prefix("--")
            .ok_or_else(|| format!("unexpected argument {}", arguments[index]))?;
        let flag = matches!(name, "fake" | "fake-no-self-exit" | "gdi");
        if flag {
            map.insert(name.to_owned(), String::new());
            index += 1;
        } else {
            let value = arguments
                .get(index + 1)
                .ok_or_else(|| format!("--{name} needs a value"))?;
            map.insert(name.to_owned(), value.clone());
            index += 2;
        }
    }
    Ok(map)
}

/// The device side of one run, real or fake: what `run` gets from it.
struct Backends {
    backends: Arc<dyn BackendFactory>,
    device_check: Arc<dyn DeviceSideCheck>,
    materials: Arc<dyn MaterialFactory>,
    label: &'static str,
    apk: Option<Arc<dyn ApkConnector>>,
}

fn run_command(arguments: &[String]) -> i32 {
    let refused = RunResult::RefusedStart.exit_code();
    let Ok(options) = options(arguments) else {
        return refused;
    };
    let (Some(plan_path), Some(run_dir)) = (options.get("plan"), options.get("run-dir")) else {
        return refused;
    };
    let run_dir = PathBuf::from(run_dir);
    let Ok(plan_text) = std::fs::read_to_string(plan_path) else {
        return refused;
    };
    let Ok(plan) = Plan::parse(&plan_text) else {
        return refused;
    };
    let clock: Arc<dyn Clock> = Arc::new(SystemClock::new());
    let job = Job::create().ok().map(Arc::new);
    let executable =
        std::env::current_exe().unwrap_or_else(|_| PathBuf::from("ac-capture-bench.exe"));
    let fake = options.contains_key("fake");
    let mut limits = FixedLimits::production();
    let apk_instances: Vec<String> = plan
        .instances
        .iter()
        .filter(|instance| instance.apk)
        .map(|instance| instance.name.clone())
        .collect();
    let Backends {
        backends,
        device_check,
        materials,
        label: backend_label,
        apk,
    } = if fake {
        let mut script = FakeScript::default();
        // #402 INT-1: every APK instance has its own fake APK, on its own device clock (a
        // different boot offset each) and its own display (the plan's orientation): its frames
        // show its own timecode and its taps flip its own APK. (C2d drew the first APK
        // instance's timecode into every instance's frames.)
        let FakeApkSet { apks, patterns } = fake_apks(&clock, &plan);
        script.patterns = patterns;
        if let Some(target) = options.get("fake-hang-close")
            && let Some((instance, kind)) = target.split_once(':')
        {
            script.close_hangs.push(CloseHang {
                at: FaultAt {
                    instance: instance.to_owned(),
                    kind: if kind == "S" {
                        BackendKind::Scrcpy
                    } else {
                        BackendKind::Nemu
                    },
                    from_call: 1,
                    count: 1,
                },
                latch: Latch::new(),
                on_entry: None,
            });
        }
        if options.contains_key("fake-no-self-exit") {
            limits.self_exit = false;
        }
        let device = FakeDevice::new(Arc::clone(&clock), script, FakeCheckScript::default());
        // D2 item 10: the stand-in `adb shell` children, outside the job object, with the
        // command line of the real child (`-s <serial> shell … scid=<hex>`, §3.11.5 item 1).
        let serials: BTreeMap<String, String> = plan
            .instances
            .iter()
            .map(|instance| (instance.name.clone(), instance.serial.clone()))
            .collect();
        device.set_children(ProcessChildren::new(executable.clone(), serials));
        let materials = Arc::new(FakeMaterials {
            writes: AtomicU64::new(0),
        });
        let apk: Option<Arc<dyn ApkConnector>> =
            (!apks.is_empty()).then(|| Arc::new(FakeApks { apks }) as Arc<dyn ApkConnector>);
        Backends {
            backends: Arc::new(Arc::clone(&device)),
            device_check: Arc::new(device),
            materials: Arc::new(materials),
            label: "fake",
            apk,
        }
    } else {
        let sink = TimingSink::install();
        let device = RealDevice::new(&plan, Arc::clone(&clock), sink);
        let apk: Option<Arc<dyn ApkConnector>> = (!apk_instances.is_empty()).then(|| {
            Arc::new(AdbApkConnector {
                adb_path: plan.safety.adb_path.clone(),
                serials: plan
                    .instances
                    .iter()
                    .filter(|instance| {
                        instance.apk && plan.safety.serial_whitelist.contains(&instance.serial)
                    })
                    .map(|instance| (instance.name.clone(), instance.serial.clone()))
                    .collect(),
                timeout: std::time::Duration::from_secs(5),
            }) as Arc<dyn ApkConnector>
        });
        Backends {
            backends: Arc::new(Arc::clone(&device)),
            device_check: Arc::new(device),
            materials: Arc::new(actingcommand_capture_bench::bmodel::PipelineMaterials {
                root: run_dir.join("bmodel"),
            }),
            label: "real",
            apk,
        }
    };
    // §5.5: the bounded per-process GPU counters, in the job object.
    let mut typeperf = None;
    if plan.metrics.typeperf {
        let samples = (plan.total_duration_ms / 1_000).max(1).to_string();
        let output = run_dir.join("gpu-run.csv");
        if std::fs::create_dir_all(&run_dir).is_ok()
            && let Ok(child) = std::process::Command::new("typeperf")
                .args([
                    r"\GPU Engine(*)\Utilization Percentage",
                    r"\GPU Process Memory(*)\Dedicated Usage",
                    "-si",
                    "1",
                    "-sc",
                    samples.as_str(),
                    "-f",
                    "CSV",
                    "-y",
                    "-o",
                ])
                .arg(&output)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
        {
            if let Some(job) = &job {
                let _ = job.assign(&child);
            }
            typeperf = Some(child);
        }
    }
    let env = Env {
        clock: Arc::clone(&clock),
        backends,
        device_check,
        materials,
        metrics: Box::new(HostMetrics::new(run_dir.clone())),
        viewers: Arc::new(ChildViewers {
            executable,
            run_dir: run_dir.clone(),
            job: job.clone(),
        }),
        start_checks: Box::new(HostStartChecks::default()),
        exit: Arc::new(ProcessExit),
        limits,
        roots: Roots::fixed(),
        header_facts: json!({
            "backend": backend_label,
            "bench_timing": actingcommand_device::bench_timing::ENABLED,
            "job_object": job.is_some(),
            "typeperf": typeperf.as_ref().map(std::process::Child::id),
            "apk": apk.is_some(),
        }),
        apk,
        probes: Arc::new(HostProbes {
            temp_dir: std::env::temp_dir(),
            pid: std::process::id(),
        }),
    };
    let outcome = run(&plan_text, &run_dir, env);
    if let Some(mut child) = typeperf {
        let _ = child.kill();
        let _ = child.wait();
    }
    drop(job);
    outcome.exit_code
}

fn view_command(arguments: &[String]) -> i32 {
    let Ok(options) = options(arguments) else {
        return 2;
    };
    let number = |name: &str| {
        options
            .get(name)
            .and_then(|value| value.parse::<u64>().ok())
    };
    let (Some(viewer_id), Some(run_dir), Some(ttl_ms)) = (
        number("viewer-id"),
        options.get("run-dir"),
        number("ttl-ms"),
    ) else {
        return 2;
    };
    view_main(ViewArgs {
        viewer_id,
        run_dir: PathBuf::from(run_dir),
        ttl_ms,
        slow_fps: options
            .get("slow-fps")
            .and_then(|value| value.parse::<f64>().ok()),
        stop_renew_after_ms: number("stop-renew-after-ms"),
        unsubscribe_after_ms: number("unsubscribe-after-ms"),
        gdi: options.contains_key("gdi"),
    })
}

/// #402 INT-1: `fake-child -s <serial> shell <server arguments with scid=<hex>>
/// [--max-seconds <s>]`, the stand-in's command line shaped as the real `adb shell` child's.
fn fake_child_command(arguments: &[String]) -> i32 {
    let Some(seconds) = actingcommand_capture_bench::fake::fake_child_lifetime(arguments) else {
        return 2;
    };
    std::thread::sleep(std::time::Duration::from_secs(seconds));
    0
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let code = match arguments.first().map(String::as_str) {
        Some("run") => run_command(&arguments[1..]),
        Some("view") => view_command(&arguments[1..]),
        Some("fake-child") => fake_child_command(&arguments[1..]),
        _ => 2,
    };
    std::process::exit(code);
}
