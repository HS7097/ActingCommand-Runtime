// SPDX-License-Identifier: AGPL-3.0-only

//! The report's first entry, a page a person reads first (`SUMMARY.md`), and its data
//! (`summary.json`) (Workflow #467 RPT-1; `contracts/problem-report.md`, "The summary"). Fixed
//! labels in English and Chinese; every value is data. Fix round 2, item 1: the page masks each
//! value as it writes it ([`Mask::member`]), so that its labels, codes, ids and the entries'
//! names stay whole.

use super::scores::{ScoreSource, TargetScore};
use super::source::Geometry;
use super::{
    ERRORS_ON_SUMMARY, ERRORS_PER_INSTANCE, InstanceErrors, MEMBERS, MIB, Mask, NOT_YET, Page,
    RunReport, SCHEMA, VIEW_ERRORS, not_yet, row_json, window_json,
};
use crate::cli_line;
use crate::code_lookup::{self, Answer, Query};
use crate::codes::{ReportFit, ReportPartReason};
use actingcommand_contract::outcome::{CodeStr, Outcome};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The catalog's description of the code spelled `spelling`, when the embedded catalog lists it.
pub(super) fn description(spelling: &str) -> Option<String> {
    if let Ok(Answer::Entry(entry)) = code_lookup::read_embedded(&Query::One(spelling.to_owned())) {
        return entry["description"].as_str().map(str::to_owned);
    }
    None
}

/// The days since 1970-01-01 as a civil date (year, month, day).
fn civil(days: u64) -> (u64, u64, u64) {
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

/// One UTC time, to the second.
struct Moment {
    year: u64,
    month: u64,
    day: u64,
    hour: u64,
    minute: u64,
    second: u64,
}

impl Moment {
    fn of(unix_ms: u64) -> Self {
        let seconds = unix_ms / 1000;
        let (year, month, day) = civil(seconds / 86_400);
        let of_day = seconds % 86_400;
        Self {
            year,
            month,
            day,
            hour: of_day / 3600,
            minute: of_day / 60 % 60,
            second: of_day % 60,
        }
    }
}

/// `YYYY-MM-DD HH:MM:SSZ`.
pub(super) fn utc(unix_ms: u64) -> String {
    let moment = Moment::of(unix_ms);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}Z",
        moment.year, moment.month, moment.day, moment.hour, moment.minute, moment.second
    )
}

/// `YYYYMMDD-HHMMSS`, the archive's name.
pub(super) fn compact(unix_ms: u64) -> String {
    let moment = Moment::of(unix_ms);
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        moment.year, moment.month, moment.day, moment.hour, moment.minute, moment.second
    )
}

/// The display line of a run: the device frame's size as stored, the package's coordinate
/// space and how the one was mapped onto the other.
pub(super) fn display_line(geometry: Option<&Geometry>) -> String {
    let Some(geometry) = geometry else {
        return "no geometry observation recorded".to_owned();
    };
    match (geometry.device, &geometry.map) {
        (Some((width, height)), Some(map)) => {
            let how = match map.kind.as_str() {
                "scale" => "recognition on the reduced frame".to_owned(),
                "translate" => format!(
                    "regions moved by anchors on a {}×{} frame",
                    map.scale_width, map.scale_height
                ),
                other => other.to_owned(),
            };
            format!(
                "device {width}×{height}, package {}×{}, scale {}/{}, {how}",
                map.base_width, map.base_height, map.scale_num, map.scale_den
            )
        }
        (Some((width, height)), None) if geometry.passed => {
            format!("device {width}×{height}, package {width}×{height}, not scaled")
        }
        (Some((width, height)), None) => format!(
            "device {width}×{height}, no display map recorded (geometry {})",
            geometry.conclusion
        ),
        (None, _) => format!(
            "device frame not recorded (geometry {})",
            geometry.conclusion
        ),
    }
}

/// A value as one cell of a table: no `|` and no line break.
fn cell(text: &str) -> String {
    text.replace('|', "/").replace(['\n', '\r'], " ")
}

/// A text on one line: its control characters escaped, as an outcome's `Display` writes them.
fn one_line(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                character.escape_default().to_string()
            } else {
                character.to_string()
            }
        })
        .collect()
}

fn number(value: Option<f64>) -> String {
    value.map_or_else(String::new, |value| format!("{value:.3}"))
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

/// What a target read, for the table's last column, masked as the scores' data is.
fn read(target: &TargetScore, removed: bool, mask: &Mask) -> String {
    if let Some(text) = &target.text {
        let text = cell(&mask.member("text", text));
        return match target.confidence {
            Some(confidence) => format!("`{text}` ({confidence:.2})"),
            None => format!("`{text}`"),
        };
    }
    if removed && target.kind == "ocr" {
        return "(removed with the frames)".to_owned();
    }
    if let Some((distance, largest)) = target.color {
        return format!("colour distance {distance:.1} of at most {largest:.1}");
    }
    if let Some(test) = &target.value_test {
        let mut test = test.clone();
        mask.value(&mut test);
        return format!("value test {}", cell(&test.to_string()));
    }
    String::new()
}

/// Fix round 2, item 1: `outcome` in one line as its `Display` writes it (its code, each value,
/// its causes), each value masked by its key as an outcome's `values` are in the data, the
/// entries' names kept.
fn outcome_line(outcome: &Outcome, page: &Page<'_>, mask: &Mask) -> String {
    let mut line = outcome.code().as_str().to_owned();
    for (key, value) in outcome.values() {
        let mut value = value.clone();
        mask.outcome_value(key, &mut value, page.entries);
        match value {
            Value::String(text) => line.push_str(&format!(" {key}={}", one_line(&text))),
            other => line.push_str(&format!(" {key}={other}")),
        }
    }
    if !outcome.causes().is_empty() {
        let causes = outcome
            .causes()
            .iter()
            .map(|link| {
                format!(
                    "{}: {}",
                    link.relation(),
                    outcome_line(link.outcome(), page, mask)
                )
            })
            .collect::<Vec<_>>();
        line.push_str(&format!(" [{}]", causes.join("; ")));
    }
    line
}

fn ended_with_text(code: Option<&CodeStr>) -> &str {
    code.map_or("", CodeStr::as_str)
}

/// The page the run was left on: the last page recognised, else the terminal's page.
fn last_page(report: &RunReport) -> &str {
    report
        .recognition
        .as_ref()
        .and_then(|recognition| recognition.matched_page.as_deref())
        .or_else(|| {
            report
                .evidence
                .as_ref()
                .and_then(|evidence| evidence.final_["page"].as_str())
        })
        .or(report.row.final_page.as_deref())
        .unwrap_or("unknown")
}

/// The page the run started from: its first step's.
fn first_page(report: &RunReport) -> Option<&str> {
    report
        .evidence
        .as_ref()
        .and_then(|evidence| evidence.steps.first())
        .map(|step| step.page.as_str())
}

fn table_row(label: &str, row: &super::RunRow, mask: &Mask) -> String {
    format!(
        "| {label} | {} | {} | {} | {} | {} | {} |\n",
        cell(&mask.member("alias", &row.alias)),
        row.state.as_str(),
        cell(&mask.member("package", row.package.as_deref().unwrap_or(""))),
        ended_with_text(row.ended_with.as_ref()),
        cell(&mask.member("final_page", row.final_page.as_deref().unwrap_or(""))),
        utc(row.first_unix_ms)
    )
}

/// The versions in one line: each member's commit (a hash, kept), this program's version, each
/// game root's bundle source (its game a member name, kept; its repository masked).
fn versions_line(versions: &Value, mask: &Mask) -> String {
    let short = |commit: &str| commit.get(..12).unwrap_or(commit).to_owned();
    let mut shown = Vec::new();
    for member in MEMBERS {
        let build = &versions[member];
        let text = match build["commit_sha"].as_str() {
            Some(commit) => short(commit),
            None if build.is_null() => "none".to_owned(),
            None => "unknown".to_owned(),
        };
        shown.push(format!("{member} {text}"));
    }
    shown.push(format!(
        "lab-ops {}",
        versions["lab_ops"].as_str().unwrap_or("?")
    ));
    if let Some(bundles) = versions["bundles"].as_object() {
        for (game, source) in bundles {
            shown.push(format!(
                "{game} {}@{}",
                mask.member("repository", source["repository"].as_str().unwrap_or("?")),
                short(source["commit"].as_str().unwrap_or("?"))
            ));
        }
    }
    shown.join(" · ")
}

fn targets_table(report: &RunReport, mask: &Mask) -> String {
    let scores = &report.scores;
    let from = match scores.source {
        ScoreSource::Stream => "from the diagnostic stream / 来自诊断流",
        ScoreSource::Recognition => {
            "from the recognition record: matched or not and each page's best score, no \
             threshold / 来自识别记录"
        }
        ScoreSource::Nothing => "none recorded / 没有记录",
    };
    let mut out = format!("- Targets / 目标 ({from})\n\n");
    if !scores.targets.is_empty() {
        out.push_str(
            "| target | kind | matched | last | best | threshold | margin | frames | read |\n",
        );
        out.push_str("|---|---|---|---|---|---|---|---|---|\n");
        for target in &scores.targets {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                cell(&mask.member("target", &target.target)),
                cell(&mask.member("kind", &target.kind)),
                yes_no(target.matched),
                number(target.last),
                number(target.best),
                number(target.threshold),
                number(target.margin()),
                target.frames,
                read(target, scores.text_removed, mask)
            ));
        }
        out.push('\n');
    }
    if !scores.pages.is_empty() {
        out.push_str("| page | matched | evaluated | best (milli) | first failed target |\n");
        out.push_str("|---|---|---|---|---|\n");
        for page in &scores.pages {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                cell(&mask.member("page", &page.page)),
                yes_no(page.matched),
                page.evaluated,
                page.best_milli
                    .map_or_else(String::new, |best| best.to_string()),
                cell(&mask.member(
                    "first_failed_target",
                    page.first_failed.as_deref().unwrap_or("")
                ))
            ));
        }
        out.push('\n');
    }
    out
}

fn frames_line(report: &RunReport, page: &Page<'_>) -> String {
    if !page.request.frames {
        return "- Frames / 截图: none: what the screen showed is missing, and the text and \
                values read from it are removed / 未带截图：画面内容缺失，从截图读出的文字和数值已去掉\n"
            .to_owned();
    }
    if report.frames.is_empty() {
        return "- Frames / 截图: none read / 没有读到\n".to_owned();
    }
    let names = report
        .frames
        .iter()
        .map(|frame| {
            if page.kept.contains(&frame.name) {
                frame.name.clone()
            } else {
                format!("{} (left out: size bound)", frame.name)
            }
        })
        .collect::<Vec<_>>();
    format!("- Frames / 截图: {}\n", names.join(", "))
}

fn run_section(number: usize, report: &RunReport, page: &Page<'_>, mask: &Mask) -> String {
    let row = &report.row;
    let mut out = format!(
        "## Run {number} / 运行 {number}: {} ({})\n\n",
        mask.member("run_id", row.run_id.as_deref().unwrap_or("-")),
        mask.member("request_id", &row.request_id)
    );
    let ended = match &row.ended_with {
        Some(code) => format!(
            "{}, `{}`: {}",
            row.state.as_str(),
            code.as_str(),
            description(code.as_str()).unwrap_or_else(|| "no catalog entry".to_owned())
        ),
        None => row.state.as_str().to_owned(),
    };
    out.push_str(&format!("- Ended / 结束: {ended}\n"));
    out.push_str(&format!(
        "- Package / 包: {}\n",
        mask.member("package", row.package.as_deref().unwrap_or("unknown"))
    ));
    match report.step() {
        Some(step) => out.push_str(&format!(
            "- Step / 步骤: {} (started on page `{}`; the run started on page `{}`)\n",
            step.step_index,
            mask.member("step_page", &step.page),
            mask.member(
                "run_started_on_page",
                first_page(report).unwrap_or("unknown")
            )
        )),
        None => out.push_str(
            "- Step / 步骤: none recorded (the run ended before its first step, or its steps \
             could not be read)\n",
        ),
    }
    out.push_str(&format!(
        "- Last page recognised / 最后识别的页: `{}`\n",
        mask.member("last_page", last_page(report))
    ));
    out.push_str(&format!(
        "- Display / 显示: {}\n",
        display_line(report.geometry())
    ));
    out.push_str(&targets_table(report, mask));
    out.push_str(&frames_line(report, page));
    if report.excerpt_cut {
        out.push_str(
            "- The failing step's stream records were cut to their bound, the newest kept \
             / 失败那一步的诊断记录超出上限，只留最新的\n",
        );
    }
    for warning in &report.warnings {
        out.push_str(&format!(
            "- Missing / 缺: {}\n",
            outcome_line(warning, page, mask)
        ));
    }
    out.push('\n');
    out
}

/// What one instance's errors view holds, in one sentence (fix round 1, item 7: its newest rows,
/// and whether the search reached the window's start; fix round 2, item 5: why it did not).
fn errors_line(instance: &InstanceErrors) -> String {
    let Some(read) = &instance.read else {
        let why = instance.unread.map_or("", |why| why.as_str());
        return format!("Not read (`{why}`); see the warnings / 没有读到，见警告");
    };
    let count = read.rows.len();
    let file = &instance.file;
    match (read.stopped, read.searched_from) {
        (Some(ReportPartReason::NotReadToEnd), _) => {
            return format!(
                "{count} rows in {file}, the newest found; the Runtime cut the search short \
                 (`not_read_to_end`), so older rows of the window were not searched / Runtime \
                 中途截断了搜索，更早的记录没有读到"
            );
        }
        (Some(why), Some(from)) => {
            return format!(
                "{count} rows in {file}, the newest found; the search spent its windows \
                 (`{}`) and searched from ledger sequence {from} on, so older rows of the window \
                 were not searched / 搜索用完了它的窗口，从账本序号 {from} 起搜索，更早的记录没有读到",
                why.as_str()
            );
        }
        (Some(why), None) => {
            return format!(
                "{count} rows in {file}, the newest found; the search stopped above the \
                 window's start (`{}`), so older rows of the window were not searched / 搜索没到时间窗开头，更早的记录没有读到",
                why.as_str()
            );
        }
        (None, _) => {}
    }
    if count >= ERRORS_PER_INSTANCE {
        return format!(
            "{count} rows in {file}, the newest of the window; older rows were not read / \
             时间窗里最新的 {count} 行，更早的没有读"
        );
    }
    if count == 0 {
        return "No row in the window / 时间窗里没有记录".to_owned();
    }
    format!("{count} rows in {file}, every row of the window / 时间窗里的全部记录")
}

fn errors_section(page: &Page<'_>, mask: &Mask) -> String {
    let mut out = String::new();
    let mut descriptions: BTreeMap<String, Option<String>> = BTreeMap::new();
    for instance in page.errors {
        out.push_str(&format!(
            "## Errors view / 错误视图: {}\n\n",
            cell(&mask.member("alias", &instance.alias))
        ));
        out.push_str(&errors_line(instance));
        let Some(read) = &instance.read else {
            out.push_str(".\n\n");
            continue;
        };
        if !read.rows.is_empty() && !page.kept.contains(&instance.file) {
            out.push_str(" (the file was left out to keep the size bound / 为守大小上限未放入)");
        }
        out.push_str(".\n\n");
        if read.rows.is_empty() {
            continue;
        }
        out.push_str("| time | severity | code | module | run |\n|---|---|---|---|---|\n");
        let shown = read.rows.len().saturating_sub(ERRORS_ON_SUMMARY);
        for row in &read.rows[shown..] {
            let code = row["code"].as_str().unwrap_or("");
            let described = descriptions
                .entry(code.to_owned())
                .or_insert_with(|| description(code));
            let code_text = match described {
                Some(text) => format!("`{code}`: {}", cell(text)),
                None => format!("`{code}`"),
            };
            let run = match (row["run_id"].as_str(), row["request_id"].as_str()) {
                (Some(run_id), _) => mask.member("run_id", run_id),
                (None, Some(request_id)) => mask.member("request_id", request_id),
                (None, None) => String::new(),
            };
            out.push_str(&format!(
                "| {} | {} | {code_text} | {} | {run} |\n",
                row["ts"].as_u64().map_or_else(String::new, utc),
                mask.member("severity", row["severity"].as_str().unwrap_or("")),
                mask.member("module", row["module"].as_str().unwrap_or(""))
            ));
        }
        out.push('\n');
    }
    out
}

/// The bound `bytes` as people read it: whole MiB, else bytes.
fn bound_text(bytes: u64) -> String {
    if bytes.is_multiple_of(MIB) {
        format!("{} MiB", bytes / MIB)
    } else {
        format!("{bytes} bytes")
    }
}

/// Fix round 1, item 10: what the screenshots can show, and how to export without them; or that
/// this report has none. Fix round 2, item 8: the way named is the command's own option, which
/// exists today (the console's control comes with its own slice).
fn privacy_line(frames: bool) -> &'static str {
    if frames {
        "Screenshots / 截图: the screenshots in this report can show a player's name and numbers. \
         To send a report without them, export again with `--no-frames` (the command's own \
         option); the text and values read from them are left out too / \
         本报告里的截图可能显示玩家的名字和数字；不想带截图时，加上命令自己的选项 --no-frames 重新导出，\
         从截图读出的文字和数值也会一起去掉。\n\n"
    } else {
        "Screenshots / 截图: none; the text and values read from them are left out too / \
         不带截图，从截图读出的文字和数值也已去掉。\n\n"
    }
}

/// Fix round 1, item 4: whether the archive fits its bound.
fn size_line(page: &Page<'_>) -> String {
    let bound = bound_text(page.request.max_bytes);
    match page.fit {
        ReportFit::Fits => {
            format!("Size / 大小: within the bound of {bound} / 在 {bound} 上限内\n\n")
        }
        ReportFit::StillOver => format!(
            "Size / 大小: over the bound of {bound}, with every part that may be left out left \
             out (see Left out) / 去掉所有可以去掉的部分后仍超过 {bound} 上限（见“未放入”）\n\n"
        ),
    }
}

/// Fix round 1, item 3: for each instance, whether the listing of its failed and unfinished runs
/// was read to its end.
fn listings_lines(page: &Page<'_>, mask: &Mask) -> String {
    let mut out = String::new();
    for listing in page.listings {
        let alias = cell(&mask.member("alias", &listing.alias));
        match listing.cut {
            None => out.push_str(&format!(
                "- Listing of {alias} / {alias} 的运行列表: read to its end / 读完\n"
            )),
            Some(why) => out.push_str(&format!(
                "- Listing of {alias} / {alias} 的运行列表: not read to its end (`{}`): the \
                 window may hold failed or unfinished runs it does not show / 没读完，时间窗里可能还有没列出的运行\n",
                why.as_str()
            )),
        }
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// `SUMMARY.md`: its first line says the view, the instances and the window; then the
/// versions, the screenshots, the size, the runs' listings and table, each run, the errors view
/// of each instance, what was left out, what is not in this report yet, and the warnings. Each
/// value is masked as it is written (fix round 2, item 1).
pub(super) fn page(page: &Page<'_>, mask: &Mask) -> String {
    let request = page.request;
    let until = request
        .until_unix_ms
        .map_or_else(|| utc(request.now_unix_ms), utc);
    let instances = if page.instances.is_empty() {
        "none".to_owned()
    } else {
        page.instances
            .iter()
            .map(|alias| mask.member("alias", alias))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let frames = if request.frames {
        "included / 带截图"
    } else {
        "left out / 不带截图"
    };
    let mut out = String::from("# Problem report / 问题报告\n\n");
    out.push_str(&format!(
        "View / 视图: {VIEW_ERRORS} · instances / 实例: {} · window / 时间窗: {} to {until} · \
         Runtime online / Runtime 在运行 · frames: {frames}\n\n",
        cell(&instances),
        utc(request.since_unix_ms)
    ));
    out.push_str(&format!(
        "Versions / 版本: {}\n\n",
        versions_line(page.versions, mask)
    ));
    out.push_str(privacy_line(request.frames));
    out.push_str(&size_line(page));
    out.push_str(&format!(
        "## Runs / 运行: {} failed or unfinished, newest first / 失败或未结束的运行，新的在前\n\n",
        page.reports.len()
    ));
    out.push_str(&listings_lines(page, mask));
    let unread = page
        .listings
        .iter()
        .filter(|listing| listing.cut.is_some())
        .map(|listing| cell(&mask.member("alias", &listing.alias)))
        .collect::<Vec<_>>();
    if page.reports.is_empty() && unread.is_empty() {
        out.push_str(
            "No failed or unfinished run in the window / 时间窗里没有失败或未结束的运行。\n\n",
        );
    } else if page.reports.is_empty() {
        let names = unread.join(", ");
        out.push_str(&format!(
            "No failed or unfinished run was found, but the listing of {names} was not read to \
             its end: the window may hold some / 没找到失败或未结束的运行，但 {names} 的运行列表没读完，时间窗里可能有。\n\n"
        ));
    } else {
        out.push_str(
            "| # | instance | state | package | ended with | last page | first recorded |\n",
        );
        out.push_str("|---|---|---|---|---|---|---|\n");
        for (index, report) in page.reports.iter().enumerate() {
            out.push_str(&table_row(&(index + 1).to_string(), &report.row, mask));
        }
        if let Some(before) = page.previous {
            out.push_str(&table_row("before 1 / 1 之前", before, mask));
        }
        out.push('\n');
    }
    for (index, report) in page.reports.iter().enumerate() {
        out.push_str(&run_section(index + 1, report, page, mask));
    }
    out.push_str(&errors_section(page, mask));
    if !page.left_out.is_empty() {
        out.push_str("## Left out / 未放入\n\n");
        for left in page.left_out {
            out.push_str(&format!(
                "- {}: {}\n",
                left["file"].as_str().unwrap_or(""),
                left["why"].as_str().unwrap_or("")
            ));
        }
        out.push('\n');
    }
    out.push_str("## Not in this report yet / 这份报告还没有\n\n");
    for (part, slice) in NOT_YET {
        out.push_str(&format!("- {part} ({slice})\n"));
    }
    out.push('\n');
    if !page.warnings.is_empty() {
        out.push_str("## Warnings / 警告\n\n");
        for warning in page.warnings {
            out.push_str(&format!("- {}\n", outcome_line(warning, page, mask)));
        }
        out.push('\n');
    }
    out
}

fn run_data(report: &RunReport, page: &Page<'_>) -> Value {
    let ended = report.row.ended_with.as_ref();
    json!({
        "listing": row_json(&report.row),
        "ended_with_description": ended.and_then(|code| description(code.as_str())),
        "step": report.step().map(|step| step.step_index),
        "step_page": report.step().map(|step| step.page.as_str()),
        "run_started_on_page": first_page(report),
        "last_page": last_page(report),
        "display": display_line(report.geometry()),
        "scores": report.scores.json(),
        "frames": report.frames.iter().map(|frame| json!({
            "file": frame.name,
            "kept": page.kept.contains(&frame.name),
        })).collect::<Vec<_>>(),
        "excerpt_cut": report.excerpt_cut,
        "missing": report.warnings.iter().map(cli_line::warning).collect::<Vec<_>>(),
    })
}

/// `summary.json`: the summary's data.
pub(super) fn data(page: &Page<'_>) -> Value {
    let request = page.request;
    json!({
        "schema_version": SCHEMA,
        "view": VIEW_ERRORS,
        "generated_unix_ms": request.now_unix_ms,
        "instances": page.instances,
        "window": window_json(super::Window {
            since_unix_ms: request.since_unix_ms,
            until_unix_ms: request.until_unix_ms,
        }),
        "runtime_online": true,
        "frames": request.frames,
        "within_bound": page.fit == ReportFit::Fits,
        "versions": page.versions,
        "listings": page.listings.iter().map(|listing| json!({
            "alias": listing.alias,
            "read_to_end": listing.cut.is_none(),
            "why": listing.cut.map(|why| why.as_str()),
        })).collect::<Vec<_>>(),
        "runs": page.reports.iter().map(|report| run_data(report, page)).collect::<Vec<_>>(),
        "succeeded_before_newest": page.previous.map(row_json),
        "errors": page.errors.iter().map(|instance| json!({
            "alias": instance.alias,
            "file": instance.file,
            "read": instance.read.is_some(),
            "why": instance.unread.map(|why| why.as_str()),
            "kept": instance.read.as_ref().map(|read| read.rows.len()),
            "newest_only": instance.read.as_ref().map(|read| read.rows.len() >= ERRORS_PER_INSTANCE),
            "searched_to_start": instance.read.as_ref().map(super::Errors::searched_to_start),
            "stopped": instance.read.as_ref().and_then(|read| read.stopped).map(|why| why.as_str()),
            "searched_from": instance.read.as_ref().and_then(|read| read.searched_from),
            "in_archive": page.kept.contains(&instance.file),
        })).collect::<Vec<_>>(),
        "left_out": page.left_out,
        "not_in_this_report_yet": not_yet(),
        "warnings": page.warnings.iter().map(cli_line::warning).collect::<Vec<_>>(),
    })
}
