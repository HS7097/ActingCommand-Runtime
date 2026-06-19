// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_device::{
    CaptureBackend, DeviceError, DeviceResult, InputBackend, MaaTouchBackend,
    MaaTouchValidationConfig, ScreencapBackend, combine_operation_and_close,
};
use actingcommand_page_detector::{
    PageDetector, PageEvaluation, PageTargetRole, load_page_set_from_json_str,
};
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::{
    PackRect, RecognitionEvaluator, RecognitionPack, RecognitionTarget, TargetEvaluation,
    TargetKind, load_pack_from_json_str,
};
use actingcommand_task_loop::{
    DryRunAction, DryRunResult, DryRunStatus, DryRunTaskLoop, load_task_plan_from_json_str,
};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
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
    Capture {
        out: PathBuf,
    },
    Recognize {
        options: RecognizeOptions,
    },
    DetectPage {
        options: DetectPageOptions,
    },
    TaskDryRun {
        options: TaskDryRunOptions,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecognizeOptions {
    pack: PathBuf,
    pack_root: PathBuf,
    target: Option<String>,
    scene: Option<PathBuf>,
    capture: bool,
    check_pack: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DetectPageOptions {
    pack: PathBuf,
    pack_root: PathBuf,
    pages: PathBuf,
    page: Option<String>,
    all: bool,
    scene: Option<PathBuf>,
    capture: bool,
    check_pages: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskDryRunOptions {
    pack: PathBuf,
    pack_root: PathBuf,
    pages: PathBuf,
    task: PathBuf,
    scene: Option<PathBuf>,
    capture: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliTargetKind {
    Template { has_click: bool },
    Color { has_click: bool },
    ClickOnly,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("FATAL: {err}");
        std::process::exit(1);
    }
}

fn run() -> DeviceResult<()> {
    let (config, commands) = parse_args(env::args().skip(1))?;
    match commands.as_slice() {
        [DeviceCommand::Capture { .. }] => {
            return run_capture_command(config, &commands);
        }
        [DeviceCommand::Recognize { options }] => {
            print!("{}", run_recognize_command(config, options)?);
            return Ok(());
        }
        [DeviceCommand::DetectPage { options }] => {
            print!("{}", run_detect_page_command(config, options)?);
            return Ok(());
        }
        [DeviceCommand::TaskDryRun { options }] => {
            print!("{}", run_task_dry_run_command(config, options)?);
            return Ok(());
        }
        _ if has_read_only_command(&commands) => {
            return Err(DeviceError::fatal(
                "read-only commands cannot be combined with MaaTouch input commands",
            ));
        }
        _ => {}
    }

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

fn has_read_only_command(commands: &[DeviceCommand]) -> bool {
    commands.iter().any(|command| {
        matches!(
            command,
            DeviceCommand::Capture { .. }
                | DeviceCommand::Recognize { .. }
                | DeviceCommand::DetectPage { .. }
                | DeviceCommand::TaskDryRun { .. }
        )
    })
}

fn run_capture_command(
    config: MaaTouchValidationConfig,
    commands: &[DeviceCommand],
) -> DeviceResult<()> {
    let [DeviceCommand::Capture { out }] = commands else {
        return Err(DeviceError::fatal(
            "capture cannot be combined with MaaTouch input commands",
        ));
    };

    let mut backend = ScreencapBackend::new(config.adb, config.target);
    let frame = backend.capture()?;
    if let Some(parent) = out.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|err| {
            DeviceError::fatal(format!(
                "failed to create capture output directory {}: {err}",
                parent.display()
            ))
        })?;
    }
    fs::write(out, &frame.png).map_err(|err| {
        DeviceError::fatal(format!(
            "failed to write capture output {}: {err}",
            out.display()
        ))
    })?;
    println!(
        "captured {}x{} -> {}",
        frame.width,
        frame.height,
        out.display()
    );
    Ok(())
}

fn run_recognize_command(
    config: MaaTouchValidationConfig,
    options: &RecognizeOptions,
) -> DeviceResult<String> {
    let pack_json = fs::read_to_string(&options.pack).map_err(|err| {
        DeviceError::fatal(format!(
            "failed to read recognition pack {}: {err}",
            options.pack.display()
        ))
    })?;
    let pack = load_pack_from_json_str(&pack_json).map_err(pack_error)?;
    let target_kind = match (&options.target, options.check_pack) {
        (_, true) => None,
        (Some(target), false) => Some(resolve_cli_target_kind(&pack, target)?),
        (None, false) => {
            return Err(DeviceError::fatal(
                "recognize requires --target <id> unless --check-pack is used",
            ));
        }
    };
    let evaluator =
        RecognitionEvaluator::new(options.pack_root.clone(), pack).map_err(pack_error)?;

    if options.check_pack {
        return Ok("check_pack=passed\n".to_string());
    }

    let target = options
        .target
        .as_deref()
        .ok_or_else(|| DeviceError::fatal("recognize target is missing"))?;
    let target_kind = target_kind.expect("target_kind is set for non-check-pack recognize");

    if target_kind == CliTargetKind::ClickOnly {
        let click = evaluator.get_click_target(target).map_err(pack_error)?;
        return Ok(format!(
            "id={target}\nkind=click_only\nclick={}\nevaluated=false\n",
            format_rect(click)
        ));
    }

    let scene = load_recognition_scene(config, options)?;
    let evaluation = evaluator
        .evaluate_target(&scene, target)
        .map_err(pack_error)?;
    format_evaluation(&evaluator, target, target_kind, evaluation)
}

fn run_detect_page_command(
    config: MaaTouchValidationConfig,
    options: &DetectPageOptions,
) -> DeviceResult<String> {
    let (evaluator, detector) =
        load_evaluator_and_detector(&options.pack, &options.pack_root, &options.pages)?;
    detector.validate(&evaluator).map_err(page_error)?;

    if options.check_pages {
        return Ok("check_pages=passed\n".to_string());
    }

    let scene = load_scene(
        config,
        options.scene.as_ref(),
        options.capture,
        "detect-page",
    )?;

    if options.all {
        let evaluations = detector
            .evaluate_all(&evaluator, &scene)
            .map_err(page_error)?;
        return Ok(evaluations
            .iter()
            .map(format_page_evaluation)
            .collect::<Vec<_>>()
            .join("\n"));
    }

    let page = options
        .page
        .as_deref()
        .ok_or_else(|| DeviceError::fatal("detect-page requires --page <id> or --all"))?;
    let evaluation = detector
        .evaluate_page(&evaluator, &scene, page)
        .map_err(page_error)?;
    Ok(format_page_evaluation(&evaluation))
}

fn run_task_dry_run_command(
    config: MaaTouchValidationConfig,
    options: &TaskDryRunOptions,
) -> DeviceResult<String> {
    let (evaluator, detector) =
        load_evaluator_and_detector(&options.pack, &options.pack_root, &options.pages)?;
    let task_json = fs::read_to_string(&options.task).map_err(|err| {
        DeviceError::fatal(format!(
            "failed to read task plan {}: {err}",
            options.task.display()
        ))
    })?;
    let task_plan = load_task_plan_from_json_str(&task_json).map_err(task_error)?;
    let task_loop = DryRunTaskLoop::new(task_plan).map_err(task_error)?;
    task_loop
        .validate(&detector, &evaluator)
        .map_err(task_error)?;

    let scene = load_scene(
        config,
        options.scene.as_ref(),
        options.capture,
        "task-dry-run",
    )?;
    let result = task_loop
        .dry_run(&detector, &evaluator, &scene)
        .map_err(task_error)?;
    Ok(format_dry_run_result(&result))
}

fn load_recognition_scene(
    config: MaaTouchValidationConfig,
    options: &RecognizeOptions,
) -> DeviceResult<Scene> {
    load_scene(config, options.scene.as_ref(), options.capture, "recognize")
}

fn load_scene(
    config: MaaTouchValidationConfig,
    scene: Option<&PathBuf>,
    capture: bool,
    command: &str,
) -> DeviceResult<Scene> {
    let scene_png = if let Some(scene) = scene {
        fs::read(scene).map_err(|err| {
            DeviceError::fatal(format!(
                "failed to read scene PNG {}: {err}",
                scene.display()
            ))
        })?
    } else if capture {
        let mut backend = ScreencapBackend::new(config.adb, config.target);
        backend.capture()?.png
    } else {
        return Err(DeviceError::fatal(format!(
            "{command} requires exactly one of --scene <png> or --capture"
        )));
    };

    Scene::from_png(&scene_png).map_err(|err| DeviceError::fatal(err.to_string()))
}

fn load_evaluator_and_detector(
    pack_path: &Path,
    pack_root: &Path,
    pages_path: &Path,
) -> DeviceResult<(RecognitionEvaluator, PageDetector)> {
    let pack_json = fs::read_to_string(pack_path).map_err(|err| {
        DeviceError::fatal(format!(
            "failed to read recognition pack {}: {err}",
            pack_path.display()
        ))
    })?;
    let pack = load_pack_from_json_str(&pack_json).map_err(pack_error)?;
    let evaluator = RecognitionEvaluator::new(pack_root.to_path_buf(), pack).map_err(pack_error)?;

    let pages_json = fs::read_to_string(pages_path).map_err(|err| {
        DeviceError::fatal(format!(
            "failed to read page set {}: {err}",
            pages_path.display()
        ))
    })?;
    let page_set = load_page_set_from_json_str(&pages_json).map_err(page_error)?;
    let detector = PageDetector::new(page_set).map_err(page_error)?;
    Ok((evaluator, detector))
}

fn resolve_cli_target_kind(pack: &RecognitionPack, target_id: &str) -> DeviceResult<CliTargetKind> {
    let target = pack
        .targets
        .iter()
        .find(|target| match target {
            RecognitionTarget::Template(target) => target.id == target_id,
            RecognitionTarget::Color(target) => target.id == target_id,
            RecognitionTarget::ClickOnly(target) => target.id == target_id,
        })
        .ok_or_else(|| DeviceError::fatal(format!("target id not found: {target_id}")))?;

    Ok(match target {
        RecognitionTarget::Template(target) => CliTargetKind::Template {
            has_click: target.click.is_some(),
        },
        RecognitionTarget::Color(target) => CliTargetKind::Color {
            has_click: target.click.is_some(),
        },
        RecognitionTarget::ClickOnly(_) => CliTargetKind::ClickOnly,
    })
}

fn format_evaluation(
    evaluator: &RecognitionEvaluator,
    target: &str,
    target_kind: CliTargetKind,
    evaluation: TargetEvaluation,
) -> DeviceResult<String> {
    let click = match target_kind {
        CliTargetKind::Template { has_click } | CliTargetKind::Color { has_click } => {
            if has_click {
                format_rect(evaluator.get_click_target(target).map_err(pack_error)?)
            } else {
                "missing".to_string()
            }
        }
        CliTargetKind::ClickOnly => "missing".to_string(),
    };

    match evaluation.kind {
        TargetKind::Template => {
            let template = evaluation.template.ok_or_else(|| {
                DeviceError::fatal(format!(
                    "template target '{target}' returned no template result"
                ))
            })?;
            let mut output = format!(
                "id={target}\nkind=template\npassed={}\nraw_score={:.6}\nscore={:.6}\nthreshold={:.6}\nmessage={}\n",
                evaluation.passed,
                template.raw_score,
                template.score,
                template.threshold,
                evaluation.message
            );
            if let Some(color) = evaluation.color {
                output.push_str(&format!(
                    "color_distance={:.6}\ncolor_max_distance={:.6}\ncolor_mean={}\ncolor_expected={}\n",
                    color.distance,
                    color.max_distance,
                    format_rgb(color.mean),
                    format_rgb(color.expected)
                ));
            }
            output.push_str(&format!("click={click}\n"));
            Ok(output)
        }
        TargetKind::Color => {
            let color = evaluation.color.ok_or_else(|| {
                DeviceError::fatal(format!("color target '{target}' returned no color result"))
            })?;
            Ok(format!(
                "id={target}\nkind=color\npassed={}\ndistance={:.6}\nmax_distance={:.6}\nmessage={}\ncolor_mean={}\ncolor_expected={}\nclick={click}\n",
                evaluation.passed,
                color.distance,
                color.max_distance,
                evaluation.message,
                format_rgb(color.mean),
                format_rgb(color.expected)
            ))
        }
        TargetKind::ClickOnly => Err(DeviceError::fatal(
            "click-only target cannot return evaluation output",
        )),
    }
}

fn format_rect(rect: PackRect) -> String {
    format!("{},{},{},{}", rect.x, rect.y, rect.width, rect.height)
}

fn format_rgb(value: [u8; 3]) -> String {
    format!("{},{},{}", value[0], value[1], value[2])
}

fn format_page_evaluation(evaluation: &PageEvaluation) -> String {
    let mut output = format!(
        "page_id={}\nmatched={}\nrequired_passed={}\nrequired_total={}\noptional_passed={}\noptional_total={}\nforbidden_passed={}\nforbidden_total={}\nmessage={}\n",
        evaluation.page_id,
        evaluation.matched,
        evaluation.required_passed,
        evaluation.required_total,
        evaluation.optional_passed,
        evaluation.optional_total,
        evaluation.forbidden_passed,
        evaluation.forbidden_total,
        evaluation.message
    );

    for target in &evaluation.target_results {
        output.push_str(&format!(
            "target={},role={},passed={},message={}\n",
            target.target_id,
            format_page_role(target.role),
            target.passed,
            target.message
        ));
    }

    output
}

fn format_page_role(role: PageTargetRole) -> &'static str {
    match role {
        PageTargetRole::Required => "required",
        PageTargetRole::Optional => "optional",
        PageTargetRole::Forbidden => "forbidden",
    }
}

fn format_dry_run_result(result: &DryRunResult) -> String {
    let mut output = format!(
        "task_id={}\nstatus={}\nmatched_step={}\nmatched_page={}\n",
        result.task_id,
        format_dry_run_status(result.status),
        result.matched_step_id.as_deref().unwrap_or("missing"),
        result.matched_page_id.as_deref().unwrap_or("missing")
    );

    match &result.action {
        Some(DryRunAction::Complete) => {
            output.push_str("action=complete\n");
        }
        Some(DryRunAction::Click { target_id, click }) => {
            output.push_str(&format!(
                "action=click\ntarget={target_id}\nclick={}\n",
                format_rect(*click)
            ));
        }
        None => {}
    }

    output.push_str(&format!("executed=false\nmessage={}\n", result.message));
    output
}

fn format_dry_run_status(status: DryRunStatus) -> &'static str {
    match status {
        DryRunStatus::NoPageMatched => "no_page_matched",
        DryRunStatus::WouldComplete => "would_complete",
        DryRunStatus::WouldClick => "would_click",
    }
}

fn pack_error(err: actingcommand_recognition_pack::RecognitionPackError) -> DeviceError {
    DeviceError::fatal(err.to_string())
}

fn page_error(err: actingcommand_page_detector::PageDetectorError) -> DeviceError {
    DeviceError::fatal(err.to_string())
}

fn task_error(err: actingcommand_task_loop::TaskLoopError) -> DeviceError {
    DeviceError::fatal(err.to_string())
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
            DeviceCommand::Capture { .. } => {
                return Err(DeviceError::fatal(
                    "capture cannot run through MaaTouchBackend",
                ));
            }
            DeviceCommand::Recognize { .. } => {
                return Err(DeviceError::fatal(
                    "recognize cannot run through MaaTouchBackend",
                ));
            }
            DeviceCommand::DetectPage { .. } => {
                return Err(DeviceError::fatal(
                    "detect-page cannot run through MaaTouchBackend",
                ));
            }
            DeviceCommand::TaskDryRun { .. } => {
                return Err(DeviceError::fatal(
                    "task-dry-run cannot run through MaaTouchBackend",
                ));
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
            "capture" => {
                index += 1;
                commands.push(DeviceCommand::Capture {
                    out: parse_capture_out(&tokens, &mut index)?,
                });
            }
            "recognize" => {
                index += 1;
                commands.push(DeviceCommand::Recognize {
                    options: parse_recognize_options(&tokens, &mut index)?,
                });
            }
            "detect-page" => {
                index += 1;
                commands.push(DeviceCommand::DetectPage {
                    options: parse_detect_page_options(&tokens, &mut index)?,
                });
            }
            "task-dry-run" => {
                index += 1;
                commands.push(DeviceCommand::TaskDryRun {
                    options: parse_task_dry_run_options(&tokens, &mut index)?,
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
            "missing command: expected reset, tap, longtap, swipe, capture, recognize, detect-page, or task-dry-run",
        ));
    }

    Ok((cfg, commands))
}

fn parse_recognize_options(tokens: &[String], index: &mut usize) -> DeviceResult<RecognizeOptions> {
    let mut pack = None;
    let mut pack_root = None;
    let mut target = None;
    let mut scene = None;
    let mut capture = false;
    let mut check_pack = false;

    while *index < tokens.len() {
        match tokens[*index].as_str() {
            "--pack" => {
                pack = Some(PathBuf::from(next_token(tokens, index, "--pack")?));
            }
            "--pack-root" => {
                pack_root = Some(PathBuf::from(next_token(tokens, index, "--pack-root")?));
            }
            "--target" => {
                target = Some(next_token(tokens, index, "--target")?);
            }
            "--scene" => {
                scene = Some(PathBuf::from(next_token(tokens, index, "--scene")?));
            }
            "--capture" => {
                capture = true;
                *index += 1;
            }
            "--check-pack" => {
                check_pack = true;
                *index += 1;
            }
            other => {
                return Err(DeviceError::fatal(format!(
                    "unknown recognize argument: {other}"
                )));
            }
        }
    }

    if scene.is_some() && capture {
        return Err(DeviceError::fatal(
            "recognize accepts --scene <png> or --capture, not both",
        ));
    }
    if !check_pack && target.is_none() {
        return Err(DeviceError::fatal(
            "recognize requires --target <id> unless --check-pack is used",
        ));
    }
    Ok(RecognizeOptions {
        pack: pack.ok_or_else(|| DeviceError::fatal("recognize requires --pack <pack.json>"))?,
        pack_root: pack_root
            .ok_or_else(|| DeviceError::fatal("recognize requires --pack-root <dir>"))?,
        target,
        scene,
        capture,
        check_pack,
    })
}

fn parse_detect_page_options(
    tokens: &[String],
    index: &mut usize,
) -> DeviceResult<DetectPageOptions> {
    let mut pack = None;
    let mut pack_root = None;
    let mut pages = None;
    let mut page = None;
    let mut all = false;
    let mut scene = None;
    let mut capture = false;
    let mut check_pages = false;

    while *index < tokens.len() {
        match tokens[*index].as_str() {
            "--pack" => {
                pack = Some(PathBuf::from(next_token(tokens, index, "--pack")?));
            }
            "--pack-root" => {
                pack_root = Some(PathBuf::from(next_token(tokens, index, "--pack-root")?));
            }
            "--pages" => {
                pages = Some(PathBuf::from(next_token(tokens, index, "--pages")?));
            }
            "--page" => {
                page = Some(next_token(tokens, index, "--page")?);
            }
            "--all" => {
                all = true;
                *index += 1;
            }
            "--scene" => {
                scene = Some(PathBuf::from(next_token(tokens, index, "--scene")?));
            }
            "--capture" => {
                capture = true;
                *index += 1;
            }
            "--check-pages" => {
                check_pages = true;
                *index += 1;
            }
            other => {
                return Err(DeviceError::fatal(format!(
                    "unknown detect-page argument: {other}"
                )));
            }
        }
    }

    if scene.is_some() && capture {
        return Err(DeviceError::fatal(
            "detect-page accepts --scene <png> or --capture, not both",
        ));
    }
    if page.is_some() && all {
        return Err(DeviceError::fatal(
            "detect-page accepts --page <id> or --all, not both",
        ));
    }
    if check_pages && (page.is_some() || all || scene.is_some() || capture) {
        return Err(DeviceError::fatal(
            "detect-page --check-pages cannot be combined with --page, --all, --scene, or --capture",
        ));
    }
    if !check_pages && page.is_none() && !all {
        return Err(DeviceError::fatal(
            "detect-page requires --page <id> or --all unless --check-pages is used",
        ));
    }
    if !check_pages && scene.is_none() && !capture {
        return Err(DeviceError::fatal(
            "detect-page requires --scene <png> or --capture unless --check-pages is used",
        ));
    }

    Ok(DetectPageOptions {
        pack: pack.ok_or_else(|| DeviceError::fatal("detect-page requires --pack <pack.json>"))?,
        pack_root: pack_root
            .ok_or_else(|| DeviceError::fatal("detect-page requires --pack-root <dir>"))?,
        pages: pages
            .ok_or_else(|| DeviceError::fatal("detect-page requires --pages <pages.json>"))?,
        page,
        all,
        scene,
        capture,
        check_pages,
    })
}

fn parse_task_dry_run_options(
    tokens: &[String],
    index: &mut usize,
) -> DeviceResult<TaskDryRunOptions> {
    let mut pack = None;
    let mut pack_root = None;
    let mut pages = None;
    let mut task = None;
    let mut scene = None;
    let mut capture = false;

    while *index < tokens.len() {
        match tokens[*index].as_str() {
            "--pack" => {
                pack = Some(PathBuf::from(next_token(tokens, index, "--pack")?));
            }
            "--pack-root" => {
                pack_root = Some(PathBuf::from(next_token(tokens, index, "--pack-root")?));
            }
            "--pages" => {
                pages = Some(PathBuf::from(next_token(tokens, index, "--pages")?));
            }
            "--task" => {
                task = Some(PathBuf::from(next_token(tokens, index, "--task")?));
            }
            "--scene" => {
                scene = Some(PathBuf::from(next_token(tokens, index, "--scene")?));
            }
            "--capture" => {
                capture = true;
                *index += 1;
            }
            other => {
                return Err(DeviceError::fatal(format!(
                    "unknown task-dry-run argument: {other}"
                )));
            }
        }
    }

    if scene.is_some() && capture {
        return Err(DeviceError::fatal(
            "task-dry-run accepts --scene <png> or --capture, not both",
        ));
    }
    if scene.is_none() && !capture {
        return Err(DeviceError::fatal(
            "task-dry-run requires --scene <png> or --capture",
        ));
    }

    Ok(TaskDryRunOptions {
        pack: pack.ok_or_else(|| DeviceError::fatal("task-dry-run requires --pack <pack.json>"))?,
        pack_root: pack_root
            .ok_or_else(|| DeviceError::fatal("task-dry-run requires --pack-root <dir>"))?,
        pages: pages
            .ok_or_else(|| DeviceError::fatal("task-dry-run requires --pages <pages.json>"))?,
        task: task.ok_or_else(|| DeviceError::fatal("task-dry-run requires --task <task.json>"))?,
        scene,
        capture,
    })
}

fn parse_capture_out(tokens: &[String], index: &mut usize) -> DeviceResult<PathBuf> {
    let mut out = None;
    while *index < tokens.len() {
        match tokens[*index].as_str() {
            "--out" => {
                out = Some(PathBuf::from(next_token(tokens, index, "--out")?));
            }
            other => {
                return Err(DeviceError::fatal(format!(
                    "unknown capture argument: {other}"
                )));
            }
        }
    }

    out.ok_or_else(|| DeviceError::fatal("capture requires --out <path>"))
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
         cargo run -p actingcommand-device-test -- [options] capture --out <path>\n\
         cargo run -p actingcommand-device-test -- [options] recognize --pack <pack.json> --pack-root <dir> --target <id> --scene <png>\n\
         cargo run -p actingcommand-device-test -- [options] recognize --pack <pack.json> --pack-root <dir> --target <id> --capture\n\
         cargo run -p actingcommand-device-test -- [options] recognize --pack <pack.json> --pack-root <dir> --check-pack\n\
         cargo run -p actingcommand-device-test -- [options] detect-page --pack <pack.json> --pack-root <dir> --pages <pages.json> --check-pages\n\
         cargo run -p actingcommand-device-test -- [options] detect-page --pack <pack.json> --pack-root <dir> --pages <pages.json> --page <page_id> --scene <png>\n\
         cargo run -p actingcommand-device-test -- [options] detect-page --pack <pack.json> --pack-root <dir> --pages <pages.json> --all --capture\n\
         cargo run -p actingcommand-device-test -- [options] task-dry-run --pack <pack.json> --pack-root <dir> --pages <pages.json> --task <task.json> --scene <png>\n\
         \n\
         Multiple commands may be provided in one invocation and will reuse one MaaTouch session.\n\
         Capture is a single-shot adb exec-out screencap command and cannot be combined with touch commands.\n\
         Recognize, detect-page, and task-dry-run are read-only: offline scene mode does not connect to a device; capture mode only uses ScreencapBackend.\n\
         Options: --adb --serial --host --port --local --remote --no-connect --no-push \\\n\
         --command-timeout-ms --handshake-timeout-ms --shutdown-timeout-ms"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

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

    #[test]
    fn parses_capture_out() {
        let (_, commands) = parse_args([
            "--port".to_string(),
            "16384".to_string(),
            "capture".to_string(),
            "--out".to_string(),
            "frame.png".to_string(),
        ])
        .expect("parse");

        assert_eq!(
            commands,
            vec![DeviceCommand::Capture {
                out: PathBuf::from("frame.png")
            }]
        );
    }

    #[test]
    fn rejects_capture_without_out() {
        let err = parse_args(["capture".to_string()]).expect_err("missing out");
        assert!(err.message().contains("--out"));
    }

    #[test]
    fn parses_recognize_scene_form() {
        let (_, commands) = parse_args([
            "recognize".to_string(),
            "--pack".to_string(),
            "pack.json".to_string(),
            "--pack-root".to_string(),
            "resources".to_string(),
            "--target".to_string(),
            "fixture/button".to_string(),
            "--scene".to_string(),
            "scene.png".to_string(),
        ])
        .expect("parse");

        assert_eq!(
            commands,
            vec![DeviceCommand::Recognize {
                options: RecognizeOptions {
                    pack: PathBuf::from("pack.json"),
                    pack_root: PathBuf::from("resources"),
                    target: Some("fixture/button".to_string()),
                    scene: Some(PathBuf::from("scene.png")),
                    capture: false,
                    check_pack: false,
                }
            }]
        );
    }

    #[test]
    fn parses_recognize_capture_form_with_global_port() {
        let (config, commands) = parse_args([
            "--port".to_string(),
            "16384".to_string(),
            "recognize".to_string(),
            "--pack".to_string(),
            "pack.json".to_string(),
            "--pack-root".to_string(),
            "resources".to_string(),
            "--target".to_string(),
            "fixture/button".to_string(),
            "--capture".to_string(),
        ])
        .expect("parse");

        assert_eq!(config.target.port, 16_384);
        assert!(matches!(
            commands.as_slice(),
            [DeviceCommand::Recognize {
                options: RecognizeOptions { capture: true, .. }
            }]
        ));
    }

    #[test]
    fn parses_recognize_check_pack_without_target_or_scene() {
        let (_, commands) = parse_args([
            "recognize".to_string(),
            "--pack".to_string(),
            "pack.json".to_string(),
            "--pack-root".to_string(),
            "resources".to_string(),
            "--check-pack".to_string(),
        ])
        .expect("parse");

        assert!(matches!(
            commands.as_slice(),
            [DeviceCommand::Recognize {
                options: RecognizeOptions {
                    target: None,
                    scene: None,
                    capture: false,
                    check_pack: true,
                    ..
                }
            }]
        ));
    }

    #[test]
    fn rejects_recognize_scene_and_capture_together() {
        let err = parse_args([
            "recognize".to_string(),
            "--pack".to_string(),
            "pack.json".to_string(),
            "--pack-root".to_string(),
            "resources".to_string(),
            "--target".to_string(),
            "fixture/button".to_string(),
            "--scene".to_string(),
            "scene.png".to_string(),
            "--capture".to_string(),
        ])
        .expect_err("scene and capture conflict");

        assert!(err.message().contains("--scene"));
        assert!(err.message().contains("--capture"));
    }

    #[test]
    fn rejects_recognize_without_target_unless_check_pack() {
        let err = parse_args([
            "recognize".to_string(),
            "--pack".to_string(),
            "pack.json".to_string(),
            "--pack-root".to_string(),
            "resources".to_string(),
            "--scene".to_string(),
            "scene.png".to_string(),
        ])
        .expect_err("target required");

        assert!(err.message().contains("--target"));
    }

    #[test]
    fn offline_recognize_scene_passes_without_device_connection() {
        let fixture = write_template_fixture("offline-recognize");
        let mut config = MaaTouchValidationConfig::default();
        config.target.host = "device.invalid.local".to_string();
        config.target.port = 1;

        let output = run_recognize_command(
            config,
            &RecognizeOptions {
                pack: fixture.pack,
                pack_root: fixture.root.clone(),
                target: Some("fixture/button".to_string()),
                scene: Some(fixture.scene),
                capture: false,
                check_pack: false,
            },
        )
        .expect("recognize");

        assert!(output.contains("id=fixture/button"));
        assert!(output.contains("kind=template"));
        assert!(output.contains("passed=true"));
        assert!(output.contains("threshold=0.900000"));
        assert!(output.contains("message=template passed"));
        assert!(output.contains("click=30,20,18,14"));
        let _ = fs::remove_dir_all(fixture.root);
    }

    #[test]
    fn template_with_color_check_outputs_color_diagnostics() {
        let fixture = write_template_with_color_fixture("template-color-pass", [24, 28, 36]);
        let output = run_recognize_command(
            MaaTouchValidationConfig::default(),
            &RecognizeOptions {
                pack: fixture.pack,
                pack_root: fixture.root.clone(),
                target: Some("fixture/button".to_string()),
                scene: Some(fixture.scene),
                capture: false,
                check_pack: false,
            },
        )
        .expect("recognize");

        assert!(output.contains("passed=true"));
        assert!(output.contains("message=template passed"));
        assert!(output.contains("color_distance=0.000000"));
        assert!(output.contains("color_max_distance=20.000000"));
        assert!(output.contains("color_mean=24,28,36"));
        assert!(output.contains("color_expected=24,28,36"));
        let _ = fs::remove_dir_all(fixture.root);
    }

    #[test]
    fn template_with_failing_color_check_explains_failure() {
        let fixture = write_template_with_color_fixture("template-color-fail", [255, 0, 0]);
        let output = run_recognize_command(
            MaaTouchValidationConfig::default(),
            &RecognizeOptions {
                pack: fixture.pack,
                pack_root: fixture.root.clone(),
                target: Some("fixture/button".to_string()),
                scene: Some(fixture.scene),
                capture: false,
                check_pack: false,
            },
        )
        .expect("recognize");

        assert!(output.contains("passed=false"));
        assert!(output.contains("message=color check failed"));
        assert!(output.contains("color_distance="));
        assert!(output.contains("color_expected=255,0,0"));
        let _ = fs::remove_dir_all(fixture.root);
    }

    #[test]
    fn color_target_outputs_message() {
        let fixture = write_color_fixture("color-target", [24, 28, 36]);
        let output = run_recognize_command(
            MaaTouchValidationConfig::default(),
            &RecognizeOptions {
                pack: fixture.pack,
                pack_root: fixture.root.clone(),
                target: Some("fixture/color".to_string()),
                scene: Some(fixture.scene),
                capture: false,
                check_pack: false,
            },
        )
        .expect("recognize");

        assert!(output.contains("kind=color"));
        assert!(output.contains("passed=true"));
        assert!(output.contains("message=color passed"));
        assert!(output.contains("color_mean=24,28,36"));
        assert!(output.contains("color_expected=24,28,36"));
        let _ = fs::remove_dir_all(fixture.root);
    }

    #[test]
    fn parses_click_only_recognize_without_scene_or_capture() {
        let (_, commands) = parse_args([
            "recognize".to_string(),
            "--pack".to_string(),
            "pack.json".to_string(),
            "--pack-root".to_string(),
            "resources".to_string(),
            "--target".to_string(),
            "fixture/click".to_string(),
        ])
        .expect("parse click-only candidate");

        assert!(matches!(
            commands.as_slice(),
            [DeviceCommand::Recognize {
                options: RecognizeOptions {
                    target: Some(_),
                    scene: None,
                    capture: false,
                    check_pack: false,
                    ..
                }
            }]
        ));
    }

    #[test]
    fn click_only_recognize_prints_click_without_evaluation() {
        let fixture = write_click_only_fixture("click-only");
        let output = run_recognize_command(
            MaaTouchValidationConfig::default(),
            &RecognizeOptions {
                pack: fixture.pack,
                pack_root: fixture.root.clone(),
                target: Some("fixture/click".to_string()),
                scene: None,
                capture: false,
                check_pack: false,
            },
        )
        .expect("click-only");

        assert_eq!(
            output,
            "id=fixture/click\nkind=click_only\nclick=3,4,5,6\nevaluated=false\n"
        );
        let _ = fs::remove_dir_all(fixture.root);
    }

    #[test]
    fn template_recognize_without_scene_or_capture_is_fatal() {
        let fixture = write_template_fixture("template-missing-input");
        let err = run_recognize_command(
            MaaTouchValidationConfig::default(),
            &RecognizeOptions {
                pack: fixture.pack,
                pack_root: fixture.root.clone(),
                target: Some("fixture/button".to_string()),
                scene: None,
                capture: false,
                check_pack: false,
            },
        )
        .expect_err("template input required");

        assert!(err.message().contains("--scene"));
        assert!(err.message().contains("--capture"));
        let _ = fs::remove_dir_all(fixture.root);
    }

    #[test]
    fn color_recognize_without_scene_or_capture_is_fatal() {
        let fixture = write_color_fixture("color-missing-input", [24, 28, 36]);
        let err = run_recognize_command(
            MaaTouchValidationConfig::default(),
            &RecognizeOptions {
                pack: fixture.pack,
                pack_root: fixture.root.clone(),
                target: Some("fixture/color".to_string()),
                scene: None,
                capture: false,
                check_pack: false,
            },
        )
        .expect_err("color input required");

        assert!(err.message().contains("--scene"));
        assert!(err.message().contains("--capture"));
        let _ = fs::remove_dir_all(fixture.root);
    }

    #[test]
    fn check_pack_accepts_valid_pack() {
        let fixture = write_template_fixture("check-pack-valid");
        let output = run_recognize_command(
            MaaTouchValidationConfig::default(),
            &RecognizeOptions {
                pack: fixture.pack,
                pack_root: fixture.root.clone(),
                target: None,
                scene: None,
                capture: false,
                check_pack: true,
            },
        )
        .expect("check pack");

        assert_eq!(output, "check_pack=passed\n");
        let _ = fs::remove_dir_all(fixture.root);
    }

    #[test]
    fn check_pack_rejects_missing_template() {
        let fixture = write_missing_template_fixture("check-pack-missing-template");
        let err = run_recognize_command(
            MaaTouchValidationConfig::default(),
            &RecognizeOptions {
                pack: fixture.pack,
                pack_root: fixture.root.clone(),
                target: None,
                scene: None,
                capture: false,
                check_pack: true,
            },
        )
        .expect_err("missing template");

        assert!(err.message().contains("does not exist"));
        let _ = fs::remove_dir_all(fixture.root);
    }

    #[test]
    fn parses_detect_page_check_pages() {
        let (_, commands) = parse_args([
            "detect-page".to_string(),
            "--pack".to_string(),
            "pack.json".to_string(),
            "--pack-root".to_string(),
            "resources".to_string(),
            "--pages".to_string(),
            "pages.json".to_string(),
            "--check-pages".to_string(),
        ])
        .expect("parse");

        assert!(matches!(
            commands.as_slice(),
            [DeviceCommand::DetectPage {
                options: DetectPageOptions {
                    check_pages: true,
                    page: None,
                    scene: None,
                    capture: false,
                    ..
                }
            }]
        ));
    }

    #[test]
    fn parses_detect_page_scene_form() {
        let (_, commands) = parse_args([
            "detect-page".to_string(),
            "--pack".to_string(),
            "pack.json".to_string(),
            "--pack-root".to_string(),
            "resources".to_string(),
            "--pages".to_string(),
            "pages.json".to_string(),
            "--page".to_string(),
            "fixture/home_page".to_string(),
            "--scene".to_string(),
            "scene.png".to_string(),
        ])
        .expect("parse");

        assert!(matches!(
            commands.as_slice(),
            [DeviceCommand::DetectPage {
                options: DetectPageOptions {
                    page: Some(_),
                    scene: Some(_),
                    capture: false,
                    check_pages: false,
                    ..
                }
            }]
        ));
    }

    #[test]
    fn parses_detect_page_capture_form() {
        let (config, commands) = parse_args([
            "--port".to_string(),
            "16384".to_string(),
            "detect-page".to_string(),
            "--pack".to_string(),
            "pack.json".to_string(),
            "--pack-root".to_string(),
            "resources".to_string(),
            "--pages".to_string(),
            "pages.json".to_string(),
            "--page".to_string(),
            "fixture/home_page".to_string(),
            "--capture".to_string(),
        ])
        .expect("parse");

        assert_eq!(config.target.port, 16_384);
        assert!(matches!(
            commands.as_slice(),
            [DeviceCommand::DetectPage {
                options: DetectPageOptions { capture: true, .. }
            }]
        ));
    }

    #[test]
    fn rejects_detect_page_scene_and_capture_together() {
        let err = parse_args([
            "detect-page".to_string(),
            "--pack".to_string(),
            "pack.json".to_string(),
            "--pack-root".to_string(),
            "resources".to_string(),
            "--pages".to_string(),
            "pages.json".to_string(),
            "--page".to_string(),
            "fixture/home_page".to_string(),
            "--scene".to_string(),
            "scene.png".to_string(),
            "--capture".to_string(),
        ])
        .expect_err("scene/capture conflict");

        assert!(err.message().contains("--scene"));
        assert!(err.message().contains("--capture"));
    }

    #[test]
    fn rejects_detect_page_without_page_or_all() {
        let err = parse_args([
            "detect-page".to_string(),
            "--pack".to_string(),
            "pack.json".to_string(),
            "--pack-root".to_string(),
            "resources".to_string(),
            "--pages".to_string(),
            "pages.json".to_string(),
            "--scene".to_string(),
            "scene.png".to_string(),
        ])
        .expect_err("page required");

        assert!(err.message().contains("--page"));
        assert!(err.message().contains("--all"));
    }

    #[test]
    fn rejects_detect_page_without_scene_or_capture() {
        let err = parse_args([
            "detect-page".to_string(),
            "--pack".to_string(),
            "pack.json".to_string(),
            "--pack-root".to_string(),
            "resources".to_string(),
            "--pages".to_string(),
            "pages.json".to_string(),
            "--page".to_string(),
            "fixture/home_page".to_string(),
        ])
        .expect_err("scene required");

        assert!(err.message().contains("--scene"));
        assert!(err.message().contains("--capture"));
    }

    #[test]
    fn rejects_detect_page_check_pages_mixed_with_page_or_scene() {
        let err = parse_args([
            "detect-page".to_string(),
            "--pack".to_string(),
            "pack.json".to_string(),
            "--pack-root".to_string(),
            "resources".to_string(),
            "--pages".to_string(),
            "pages.json".to_string(),
            "--check-pages".to_string(),
            "--page".to_string(),
            "fixture/home_page".to_string(),
        ])
        .expect_err("mixed check-pages");

        assert!(err.message().contains("--check-pages"));
        assert!(err.message().contains("--page"));
    }

    #[test]
    fn check_pages_accepts_synthetic_pages() {
        let fixture = write_page_fixture("check-pages-valid", [24, 28, 36]);
        let output = run_detect_page_command(
            MaaTouchValidationConfig::default(),
            &DetectPageOptions {
                pack: fixture.pack.clone(),
                pack_root: fixture.root.clone(),
                pages: fixture.pages.clone(),
                page: None,
                all: false,
                scene: None,
                capture: false,
                check_pages: true,
            },
        )
        .expect("check pages");

        assert_eq!(output, "check_pages=passed\n");
        let _ = fs::remove_dir_all(fixture.root);
    }

    #[test]
    fn detect_page_scene_matches_synthetic_page() {
        let fixture = write_page_fixture("detect-page-match", [24, 28, 36]);
        let output = run_detect_page_command(
            MaaTouchValidationConfig::default(),
            &DetectPageOptions {
                pack: fixture.pack.clone(),
                pack_root: fixture.root.clone(),
                pages: fixture.pages.clone(),
                page: Some("fixture/home_page".to_string()),
                all: false,
                scene: Some(fixture.scene.clone()),
                capture: false,
                check_pages: false,
            },
        )
        .expect("detect page");

        assert!(output.contains("page_id=fixture/home_page"));
        assert!(output.contains("matched=true"));
        assert!(
            output.contains("target=fixture/color,role=required,passed=true,message=color passed")
        );
        let _ = fs::remove_dir_all(fixture.root);
    }

    #[test]
    fn detect_page_required_failure_reports_target_line() {
        let fixture = write_page_fixture("detect-page-fail", [255, 0, 0]);
        let output = run_detect_page_command(
            MaaTouchValidationConfig::default(),
            &DetectPageOptions {
                pack: fixture.pack.clone(),
                pack_root: fixture.root.clone(),
                pages: fixture.pages.clone(),
                page: Some("fixture/home_page".to_string()),
                all: false,
                scene: Some(fixture.scene.clone()),
                capture: false,
                check_pages: false,
            },
        )
        .expect("detect page");

        assert!(output.contains("matched=false"));
        assert!(output.contains("message=required target failed: fixture/color"));
        assert!(
            output.contains("target=fixture/color,role=required,passed=false,message=color failed")
        );
        let _ = fs::remove_dir_all(fixture.root);
    }

    #[test]
    fn detect_page_click_only_page_is_fatal() {
        let fixture = write_click_only_fixture("detect-page-click-only");
        let pages = write_pages_file(&fixture.root, "fixture/home_page", "fixture/click");
        let err = run_detect_page_command(
            MaaTouchValidationConfig::default(),
            &DetectPageOptions {
                pack: fixture.pack.clone(),
                pack_root: fixture.root.clone(),
                pages,
                page: None,
                all: false,
                scene: None,
                capture: false,
                check_pages: true,
            },
        )
        .expect_err("click-only page");

        assert!(err.message().contains("click-only target"));
        let _ = fs::remove_dir_all(fixture.root);
    }

    #[test]
    fn detect_page_missing_page_id_is_fatal() {
        let fixture = write_page_fixture("detect-page-missing-page", [24, 28, 36]);
        let err = run_detect_page_command(
            MaaTouchValidationConfig::default(),
            &DetectPageOptions {
                pack: fixture.pack.clone(),
                pack_root: fixture.root.clone(),
                pages: fixture.pages.clone(),
                page: Some("fixture/missing".to_string()),
                all: false,
                scene: Some(fixture.scene.clone()),
                capture: false,
                check_pages: false,
            },
        )
        .expect_err("missing page");

        assert!(err.message().contains("page id not found"));
        let _ = fs::remove_dir_all(fixture.root);
    }

    #[test]
    fn detect_page_coordinate_mismatch_is_fatal() {
        let fixture = write_page_fixture("detect-page-coordinate", [24, 28, 36]);
        let wrong_scene = fixture.root.join("scenes/wrong.png");
        fs::write(&wrong_scene, encode_png(32, 24, |_x, _y| [24, 28, 36])).expect("wrong scene");
        let err = run_detect_page_command(
            MaaTouchValidationConfig::default(),
            &DetectPageOptions {
                pack: fixture.pack.clone(),
                pack_root: fixture.root.clone(),
                pages: fixture.pages.clone(),
                page: Some("fixture/home_page".to_string()),
                all: false,
                scene: Some(wrong_scene),
                capture: false,
                check_pages: false,
            },
        )
        .expect_err("coordinate mismatch");

        assert!(err.message().contains("coordinate_space"));
        let _ = fs::remove_dir_all(fixture.root);
    }

    #[test]
    fn detect_page_is_treated_as_read_only_command() {
        let (_, commands) = parse_args([
            "tap".to_string(),
            "1".to_string(),
            "2".to_string(),
            "detect-page".to_string(),
            "--pack".to_string(),
            "pack.json".to_string(),
            "--pack-root".to_string(),
            "resources".to_string(),
            "--pages".to_string(),
            "pages.json".to_string(),
            "--check-pages".to_string(),
        ])
        .expect("parse mixed commands");

        assert!(has_read_only_command(&commands));
    }

    #[test]
    fn parses_task_dry_run_scene_form() {
        let (_, commands) = parse_args([
            "task-dry-run".to_string(),
            "--pack".to_string(),
            "pack.json".to_string(),
            "--pack-root".to_string(),
            "resources".to_string(),
            "--pages".to_string(),
            "pages.json".to_string(),
            "--task".to_string(),
            "task.json".to_string(),
            "--scene".to_string(),
            "scene.png".to_string(),
        ])
        .expect("parse");

        assert!(matches!(
            commands.as_slice(),
            [DeviceCommand::TaskDryRun {
                options: TaskDryRunOptions {
                    scene: Some(_),
                    capture: false,
                    ..
                }
            }]
        ));
    }

    #[test]
    fn parses_task_dry_run_capture_form() {
        let (_, commands) = parse_args([
            "task-dry-run".to_string(),
            "--pack".to_string(),
            "pack.json".to_string(),
            "--pack-root".to_string(),
            "resources".to_string(),
            "--pages".to_string(),
            "pages.json".to_string(),
            "--task".to_string(),
            "task.json".to_string(),
            "--capture".to_string(),
        ])
        .expect("parse");

        assert!(matches!(
            commands.as_slice(),
            [DeviceCommand::TaskDryRun {
                options: TaskDryRunOptions { capture: true, .. }
            }]
        ));
    }

    #[test]
    fn rejects_task_dry_run_scene_and_capture_together() {
        let err = parse_args([
            "task-dry-run".to_string(),
            "--pack".to_string(),
            "pack.json".to_string(),
            "--pack-root".to_string(),
            "resources".to_string(),
            "--pages".to_string(),
            "pages.json".to_string(),
            "--task".to_string(),
            "task.json".to_string(),
            "--scene".to_string(),
            "scene.png".to_string(),
            "--capture".to_string(),
        ])
        .expect_err("scene/capture conflict");

        assert!(err.message().contains("--scene"));
        assert!(err.message().contains("--capture"));
    }

    #[test]
    fn rejects_task_dry_run_without_task() {
        let err = parse_args([
            "task-dry-run".to_string(),
            "--pack".to_string(),
            "pack.json".to_string(),
            "--pack-root".to_string(),
            "resources".to_string(),
            "--pages".to_string(),
            "pages.json".to_string(),
            "--scene".to_string(),
            "scene.png".to_string(),
        ])
        .expect_err("missing task");

        assert!(err.message().contains("--task"));
    }

    #[test]
    fn task_dry_run_complete_outputs_would_complete() {
        let fixture = write_page_fixture("task-complete", [24, 28, 36]);
        let output = run_task_dry_run_command(
            MaaTouchValidationConfig::default(),
            &TaskDryRunOptions {
                pack: fixture.pack.clone(),
                pack_root: fixture.root.clone(),
                pages: fixture.pages.clone(),
                task: fixture.task_complete.clone(),
                scene: Some(fixture.scene.clone()),
                capture: false,
            },
        )
        .expect("task dry run");

        assert!(output.contains("task_id=fixture.task"));
        assert!(output.contains("status=would_complete"));
        assert!(output.contains("matched_step=home_step"));
        assert!(output.contains("matched_page=fixture/home_page"));
        assert!(output.contains("action=complete"));
        assert!(output.contains("executed=false"));
        let _ = fs::remove_dir_all(fixture.root);
    }

    #[test]
    fn task_dry_run_click_outputs_click_rect() {
        let fixture = write_page_fixture("task-click", [24, 28, 36]);
        let output = run_task_dry_run_command(
            MaaTouchValidationConfig::default(),
            &TaskDryRunOptions {
                pack: fixture.pack.clone(),
                pack_root: fixture.root.clone(),
                pages: fixture.pages.clone(),
                task: fixture.task_click.clone(),
                scene: Some(fixture.scene.clone()),
                capture: false,
            },
        )
        .expect("task dry run");

        assert!(output.contains("status=would_click"));
        assert!(output.contains("action=click"));
        assert!(output.contains("target=fixture/color"));
        assert!(output.contains("click=30,20,18,14"));
        assert!(output.contains("executed=false"));
        let _ = fs::remove_dir_all(fixture.root);
    }

    #[test]
    fn task_dry_run_coordinate_mismatch_is_fatal() {
        let fixture = write_page_fixture("task-coordinate", [24, 28, 36]);
        let wrong_scene = fixture.root.join("scenes/wrong.png");
        fs::write(&wrong_scene, encode_png(32, 24, |_x, _y| [24, 28, 36])).expect("wrong scene");
        let err = run_task_dry_run_command(
            MaaTouchValidationConfig::default(),
            &TaskDryRunOptions {
                pack: fixture.pack.clone(),
                pack_root: fixture.root.clone(),
                pages: fixture.pages.clone(),
                task: fixture.task_complete.clone(),
                scene: Some(wrong_scene),
                capture: false,
            },
        )
        .expect_err("coordinate mismatch");

        assert!(err.message().contains("coordinate_space"));
        let _ = fs::remove_dir_all(fixture.root);
    }

    #[test]
    fn task_dry_run_is_treated_as_read_only_command() {
        let (_, commands) = parse_args([
            "tap".to_string(),
            "1".to_string(),
            "2".to_string(),
            "task-dry-run".to_string(),
            "--pack".to_string(),
            "pack.json".to_string(),
            "--pack-root".to_string(),
            "resources".to_string(),
            "--pages".to_string(),
            "pages.json".to_string(),
            "--task".to_string(),
            "task.json".to_string(),
            "--scene".to_string(),
            "scene.png".to_string(),
        ])
        .expect("parse mixed commands");

        assert!(has_read_only_command(&commands));
    }

    struct Fixture {
        root: PathBuf,
        pack: PathBuf,
        scene: PathBuf,
    }

    struct PageFixture {
        root: PathBuf,
        pack: PathBuf,
        pages: PathBuf,
        scene: PathBuf,
        task_complete: PathBuf,
        task_click: PathBuf,
    }

    fn write_template_fixture(label: &str) -> Fixture {
        let root = temp_fixture_dir(label);
        fs::create_dir_all(root.join("templates")).expect("templates dir");
        fs::create_dir_all(root.join("scenes")).expect("scenes dir");
        fs::write(root.join("templates/button.png"), button_png(12, 10)).expect("template");
        fs::write(root.join("scenes/home_scene.png"), scene_png(64, 48)).expect("scene");
        fs::write(root.join("recognition-pack.json"), template_pack_json()).expect("pack");
        Fixture {
            pack: root.join("recognition-pack.json"),
            scene: root.join("scenes/home_scene.png"),
            root,
        }
    }

    fn write_template_with_color_fixture(label: &str, expected: [u8; 3]) -> Fixture {
        let root = temp_fixture_dir(label);
        fs::create_dir_all(root.join("templates")).expect("templates dir");
        fs::create_dir_all(root.join("scenes")).expect("scenes dir");
        fs::write(root.join("templates/button.png"), button_png(12, 10)).expect("template");
        fs::write(root.join("scenes/home_scene.png"), scene_png(64, 48)).expect("scene");
        fs::write(
            root.join("recognition-pack.json"),
            template_with_color_pack_json(expected),
        )
        .expect("pack");
        Fixture {
            pack: root.join("recognition-pack.json"),
            scene: root.join("scenes/home_scene.png"),
            root,
        }
    }

    fn write_color_fixture(label: &str, expected: [u8; 3]) -> Fixture {
        let root = temp_fixture_dir(label);
        fs::create_dir_all(root.join("scenes")).expect("scenes dir");
        fs::write(root.join("scenes/home_scene.png"), scene_png(64, 48)).expect("scene");
        fs::write(
            root.join("recognition-pack.json"),
            color_pack_json(expected),
        )
        .expect("pack");
        Fixture {
            pack: root.join("recognition-pack.json"),
            scene: root.join("scenes/home_scene.png"),
            root,
        }
    }

    fn write_page_fixture(label: &str, expected: [u8; 3]) -> PageFixture {
        let root = temp_fixture_dir(label);
        fs::create_dir_all(root.join("scenes")).expect("scenes dir");
        fs::write(root.join("scenes/home_scene.png"), scene_png(64, 48)).expect("scene");
        fs::write(
            root.join("recognition-pack.json"),
            color_pack_json(expected),
        )
        .expect("pack");
        let pages = write_pages_file(&root, "fixture/home_page", "fixture/color");
        let task_complete = write_task_file(&root, "task-complete.json", task_complete_json());
        let task_click =
            write_task_file(&root, "task-click.json", task_click_json("fixture/color"));

        PageFixture {
            pack: root.join("recognition-pack.json"),
            pages,
            scene: root.join("scenes/home_scene.png"),
            task_complete,
            task_click,
            root,
        }
    }

    fn write_click_only_fixture(label: &str) -> Fixture {
        let root = temp_fixture_dir(label);
        fs::write(root.join("recognition-pack.json"), click_only_pack_json()).expect("pack");
        Fixture {
            pack: root.join("recognition-pack.json"),
            scene: root.join("unused.png"),
            root,
        }
    }

    fn write_pages_file(root: &std::path::Path, page_id: &str, target_id: &str) -> PathBuf {
        let pages = root.join("pages.json");
        fs::write(&pages, pages_json(page_id, target_id)).expect("pages");
        pages
    }

    fn write_task_file(root: &std::path::Path, name: &str, content: String) -> PathBuf {
        let task = root.join(name);
        fs::write(&task, content).expect("task");
        task
    }

    fn write_missing_template_fixture(label: &str) -> Fixture {
        let root = temp_fixture_dir(label);
        fs::write(
            root.join("recognition-pack.json"),
            missing_template_pack_json(),
        )
        .expect("pack");
        Fixture {
            pack: root.join("recognition-pack.json"),
            scene: root.join("unused.png"),
            root,
        }
    }

    fn temp_fixture_dir(label: &str) -> PathBuf {
        let index = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "actingcommand-device-test-{label}-{}-{index}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("fixture root");
        root
    }

    fn template_pack_json() -> &'static str {
        r#"{
            "schema_version": "0.1",
            "game": "fixture",
            "server": "test",
            "locale": "test",
            "coordinate_space": { "width": 64, "height": 48 },
            "defaults": { "template_threshold": 0.90, "color_max_distance": 20.0 },
            "targets": [
              {
                "type": "template",
                "id": "fixture/button",
                "template_path": "templates/button.png",
                "region": { "x": 20, "y": 14, "width": 12, "height": 10 },
                "click": { "x": 30, "y": 20, "width": 18, "height": 14 }
              }
            ]
        }"#
    }

    fn template_with_color_pack_json(expected: [u8; 3]) -> String {
        format!(
            r#"{{
                "schema_version": "0.1",
                "game": "fixture",
                "server": "test",
                "locale": "test",
                "coordinate_space": {{ "width": 64, "height": 48 }},
                "defaults": {{ "template_threshold": 0.90, "color_max_distance": 20.0 }},
                "targets": [
                  {{
                    "type": "template",
                    "id": "fixture/button",
                    "template_path": "templates/button.png",
                    "region": {{ "x": 20, "y": 14, "width": 12, "height": 10 }},
                    "color_check": {{
                      "region": {{ "x": 0, "y": 0, "width": 4, "height": 4 }},
                      "expected": [{}, {}, {}]
                    }},
                    "click": {{ "x": 30, "y": 20, "width": 18, "height": 14 }}
                  }}
                ]
            }}"#,
            expected[0], expected[1], expected[2]
        )
    }

    fn color_pack_json(expected: [u8; 3]) -> String {
        format!(
            r#"{{
                "schema_version": "0.1",
                "game": "fixture",
                "server": "test",
                "locale": "test",
                "coordinate_space": {{ "width": 64, "height": 48 }},
                "defaults": {{ "template_threshold": 0.90, "color_max_distance": 20.0 }},
                "targets": [
                  {{
                    "type": "color",
                    "id": "fixture/color",
                    "region": {{ "x": 0, "y": 0, "width": 4, "height": 4 }},
                    "expected": [{}, {}, {}],
                    "click": {{ "x": 30, "y": 20, "width": 18, "height": 14 }}
                  }}
                ]
            }}"#,
            expected[0], expected[1], expected[2]
        )
    }

    fn pages_json(page_id: &str, target_id: &str) -> String {
        format!(
            r#"{{
                "schema_version": "0.1",
                "pages": [
                  {{
                    "id": "{page_id}",
                    "required": ["{target_id}"],
                    "optional": [],
                    "forbidden": []
                  }}
                ]
            }}"#
        )
    }

    fn task_complete_json() -> String {
        r#"{
            "schema_version": "0.1",
            "id": "fixture.task",
            "steps": [
              {
                "id": "home_step",
                "page_id": "fixture/home_page",
                "on_match": { "type": "complete" }
              }
            ]
        }"#
        .to_string()
    }

    fn task_click_json(target_id: &str) -> String {
        format!(
            r#"{{
                "schema_version": "0.1",
                "id": "fixture.task",
                "steps": [
                  {{
                    "id": "home_step",
                    "page_id": "fixture/home_page",
                    "on_match": {{ "type": "click", "target_id": "{target_id}" }}
                  }}
                ]
            }}"#
        )
    }

    fn click_only_pack_json() -> &'static str {
        r#"{
            "schema_version": "0.1",
            "targets": [
              {
                "type": "click_only",
                "id": "fixture/click",
                "click": { "x": 3, "y": 4, "width": 5, "height": 6 }
              }
            ]
        }"#
    }

    fn missing_template_pack_json() -> &'static str {
        r#"{
            "schema_version": "0.1",
            "targets": [
              {
                "type": "template",
                "id": "fixture/missing",
                "template_path": "templates/missing.png",
                "region": { "x": 20, "y": 14, "width": 12, "height": 10 }
              }
            ]
        }"#
    }

    fn scene_png(width: u32, height: u32) -> Vec<u8> {
        encode_png(width, height, |x, y| {
            if (20..32).contains(&x) && (14..24).contains(&y) {
                button_pixel(x - 20, y - 14)
            } else {
                [24, 28, 36]
            }
        })
    }

    fn button_png(width: u32, height: u32) -> Vec<u8> {
        encode_png(width, height, button_pixel)
    }

    fn button_pixel(x: u32, y: u32) -> [u8; 3] {
        let stripe = ((x * 11 + y * 17) % 97) as u8;
        [
            70 + stripe,
            120 + (stripe / 2),
            190_u8.saturating_sub(stripe / 3),
        ]
    }

    fn encode_png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
        let mut scanlines = Vec::with_capacity((height * (1 + width * 3)) as usize);
        for y in 0..height {
            scanlines.push(0);
            for x in 0..width {
                scanlines.extend_from_slice(&pixel(x, y));
            }
        }

        let mut zlib = Vec::new();
        zlib.extend_from_slice(&[0x78, 0x01]);
        write_stored_deflate_blocks(&mut zlib, &scanlines);
        zlib.extend_from_slice(&adler32(&scanlines).to_be_bytes());

        let mut png = Vec::new();
        png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        append_chunk(&mut png, b"IHDR", &ihdr);
        append_chunk(&mut png, b"IDAT", &zlib);
        append_chunk(&mut png, b"IEND", &[]);
        png
    }

    fn write_stored_deflate_blocks(out: &mut Vec<u8>, data: &[u8]) {
        let block_count = data.len().div_ceil(65_535);
        for (index, chunk) in data.chunks(65_535).enumerate() {
            out.push(if index + 1 == block_count { 1 } else { 0 });
            let len = chunk.len() as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(chunk);
        }
    }

    fn append_chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        png.extend_from_slice(kind);
        png.extend_from_slice(data);
        let mut crc_data = Vec::with_capacity(kind.len() + data.len());
        crc_data.extend_from_slice(kind);
        crc_data.extend_from_slice(data);
        png.extend_from_slice(&crc32(&crc_data).to_be_bytes());
    }

    fn adler32(data: &[u8]) -> u32 {
        let mut a = 1_u32;
        let mut b = 0_u32;
        for byte in data {
            a = (a + u32::from(*byte)) % 65_521;
            b = (b + a) % 65_521;
        }
        (b << 16) | a
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffff_u32;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
        !crc
    }
}
