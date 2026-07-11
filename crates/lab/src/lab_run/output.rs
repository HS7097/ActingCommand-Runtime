// SPDX-License-Identifier: AGPL-3.0-only

struct CapturedScene {
    scene: Scene,
    matched_page: Option<String>,
    page_evaluations: Vec<PageEvaluation>,
    verify_template_matched: bool,
    width: u32,
    height: u32,
}

impl CapturedScene {
    fn matched_anchor(&self, game: &str) -> Option<String> {
        self.matched_page
            .as_deref()
            .map(|page| canonical_page_anchor(game, page))
    }
}

fn scene_from_frame(frame: &Frame) -> CliOutcome<Scene> {
    let pixel_format = match frame.pixel_format {
        PixelFormat::Rgb8 => ScenePixelFormat::Rgb8,
        PixelFormat::Rgba8 => ScenePixelFormat::Rgba8,
    };
    Scene::from_pixels(frame.width, frame.height, &frame.pixels, pixel_format)
        .map_err(|err| CliError::device(err.to_string()))
}

#[derive(Debug)]
struct ArchiveResult {
    path: PathBuf,
    sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalOutputZip {
    path: String,
    sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LabCompletedProjection {
    run_id: String,
    status: String,
    ok: bool,
    record_type: String,
    output_zip: Option<TerminalOutputZip>,
    ledger_path: PathBuf,
}

impl LabCompletedProjection {
    fn require_output_zip(&self) -> CliOutcome<&TerminalOutputZip> {
        self.output_zip.as_ref().ok_or_else(|| {
            CliError::package_invalid("runtime ledger completed projection missing output_zip")
        })
    }
}

struct LabLogProjection {
    events: Vec<Value>,
    recognition: Vec<Value>,
    evidence: Vec<Value>,
    summary: Value,
    diagnostics: Value,
    environment: Value,
}

#[derive(Debug, Clone, Copy)]
struct IntervalStats {
    min: u64,
    median: u64,
    max: u64,
    count: usize,
}

fn interval_stats(values: &[u64]) -> Option<IntervalStats> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    Some(IntervalStats {
        min: sorted[0],
        median: sorted[sorted.len() / 2],
        max: *sorted.last().expect("non-empty"),
        count: sorted.len(),
    })
}

#[cfg(test)]
fn path_is_inside(path: &Path, parent: &Path) -> bool {
    path_is_inside_from(path, parent, None)
}

fn path_is_inside_from(path: &Path, parent: &Path, current_dir: Option<&Path>) -> bool {
    let path = normalized_absolute_path(path, current_dir);
    let parent = normalized_absolute_path(parent, current_dir);
    path != parent && path.starts_with(parent)
}

fn normalized_absolute_path(path: &Path, current_dir: Option<&Path>) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        current_dir.unwrap_or_else(|| Path::new(".")).join(path)
    };
    normalize_path_components(&absolute)
}

fn normalize_path_components(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(value) => normalized.push(value),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn write_json(path: &Path, value: &Value) -> CliOutcome<()> {
    let text = serde_json::to_vec_pretty(value).map_err(|err| {
        CliError::package_invalid(format!("failed to serialize {}: {err}", path.display()))
    })?;
    fs::write(path, text).map_err(|err| {
        CliError::package_invalid(format!("failed to write {}: {err}", path.display()))
    })
}

fn write_json_lines(path: &Path, values: &[Value]) -> CliOutcome<()> {
    let mut file = File::create(path).map_err(|err| {
        CliError::package_invalid(format!("failed to create {}: {err}", path.display()))
    })?;
    for value in values {
        let line = serde_json::to_string(value).map_err(|err| {
            CliError::package_invalid(format!("failed to serialize {}: {err}", path.display()))
        })?;
        writeln!(file, "{line}").map_err(|err| {
            CliError::package_invalid(format!("failed to write {}: {err}", path.display()))
        })?;
    }
    Ok(())
}

fn write_output_zip(output_dir: &Path, out_path: &Path) -> CliOutcome<()> {
    let result = write_output_zip_inner(output_dir, out_path);
    if result.is_err() {
        let _ = fs::remove_file(out_path);
    }
    result
}

fn write_output_zip_inner(output_dir: &Path, out_path: &Path) -> CliOutcome<()> {
    if let Some(parent) = out_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::package_invalid(format!("failed to create {}: {err}", parent.display()))
        })?;
    }
    let file = File::create(out_path).map_err(|err| {
        CliError::package_invalid(format!("failed to create {}: {err}", out_path.display()))
    })?;
    let mut zip = ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.add_directory("logs/", options)
        .map_err(|err| CliError::package_invalid(format!("failed to add logs directory: {err}")))?;
    zip.add_directory("screenshots/", options).map_err(|err| {
        CliError::package_invalid(format!("failed to add screenshots directory: {err}"))
    })?;
    add_zip_dir(&mut zip, output_dir, &output_dir.join("logs"), options)?;
    add_zip_dir(
        &mut zip,
        output_dir,
        &output_dir.join("screenshots"),
        options,
    )?;
    let file = zip
        .finish()
        .map_err(|err| CliError::package_invalid(format!("failed to finish output zip: {err}")))?;
    file.sync_all().map_err(|err| {
        CliError::package_invalid(format!(
            "failed to sync output zip {}: {err}",
            out_path.display()
        ))
    })?;
    Ok(())
}

fn add_zip_dir(
    zip: &mut ZipWriter<File>,
    root: &Path,
    dir: &Path,
    options: FileOptions,
) -> CliOutcome<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir).map_err(|err| {
        CliError::package_invalid(format!("failed to list {}: {err}", dir.display()))
    })? {
        let entry = entry.map_err(|err| {
            CliError::package_invalid(format!("failed to read directory entry: {err}"))
        })?;
        let path = entry.path();
        if path.is_dir() {
            add_zip_dir(zip, root, &path, options)?;
        } else {
            let relative = path.strip_prefix(root).map_err(|err| {
                CliError::package_invalid(format!("failed to relativize {}: {err}", path.display()))
            })?;
            let name = path_to_zip_name(relative)?;
            zip.start_file(name, options).map_err(|err| {
                CliError::package_invalid(format!("failed to start zip file: {err}"))
            })?;
            let bytes = fs::read(&path).map_err(|err| {
                CliError::package_invalid(format!("failed to read {}: {err}", path.display()))
            })?;
            zip.write_all(&bytes).map_err(|err| {
                CliError::package_invalid(format!("failed to write output zip: {err}"))
            })?;
        }
    }
    Ok(())
}

fn path_to_zip_name(path: &Path) -> CliOutcome<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(value.to_string_lossy().to_string()),
            _ => {
                return Err(CliError::package_invalid(format!(
                    "invalid output zip path {}",
                    path.display()
                )));
            }
        }
    }
    Ok(parts.join("/"))
}

fn page_evaluation_json(evaluation: &PageEvaluation) -> Value {
    json!({
        "page": evaluation.page_id,
        "matched": evaluation.matched,
        "message": evaluation.message,
        "required_passed": evaluation.required_passed,
        "required_total": evaluation.required_total,
        "any_of_passed": evaluation.any_of_passed,
        "any_of_total": evaluation.any_of_total,
        "optional_passed": evaluation.optional_passed,
        "optional_total": evaluation.optional_total,
        "forbidden_passed": evaluation.forbidden_passed,
        "forbidden_total": evaluation.forbidden_total,
        "targets": evaluation.target_results.iter().map(|target| json!({
            "id": target.target_id,
            "role": format!("{:?}", target.role),
            "passed": target.passed,
            "message": target.message
        })).collect::<Vec<_>>()
    })
}

fn rect_json(rect: PackRect) -> Value {
    json!({"x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height})
}

fn file_sha256(path: &Path) -> CliOutcome<String> {
    let bytes = fs::read(path).map_err(|err| {
        CliError::package_invalid(format!("failed to read {}: {err}", path.display()))
    })?;
    Ok(hex_sha256(&bytes))
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hash_text(text: &str) -> u64 {
    let digest = Sha256::digest(text.as_bytes());
    u64::from_be_bytes([
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7],
    ])
}

fn now_system_time(clock: &dyn Clock) -> CliOutcome<SystemTime> {
    UNIX_EPOCH
        .checked_add(Duration::from_millis(clock.now_unix_ms()?))
        .ok_or_else(|| CliError::device("Lab clock value exceeds SystemTime range"))
}

fn timestamp_iso(time: SystemTime) -> String {
    let (date, h, m, s, ms) = timestamp_parts(time);
    format!("{date}T{h:02}:{m:02}:{s:02}.{ms:03}Z")
}

fn timestamp_file_stem(time: SystemTime) -> String {
    let (date, h, m, s, ms) = timestamp_parts(time);
    format!("{}_{h:02}{m:02}{s:02}_{ms:03}", date.replace('-', ""))
}

fn timestamp_parts(time: SystemTime) -> (String, u64, u64, u64, u32) {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = duration.as_secs();
    let days = seconds / 86_400;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    (
        format!("{year:04}-{month:02}-{day:02}"),
        seconds_of_day / 3600,
        (seconds_of_day % 3600) / 60,
        seconds_of_day % 60,
        duration.subsec_millis(),
    )
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year, m, d)
}
