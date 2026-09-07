// SPDX-License-Identifier: AGPL-3.0-only

use super::{VisionFfiError, VisionFfiResult, argument_value};
use actingcommand_ledger_forensics::{
    ForensicEventFilter, ForensicEventsRequest, ForensicOutput, ForensicReport, ForensicRequest,
    MAX_FORENSIC_EVENTS,
};
use std::path::PathBuf;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Options {
    pub state_root: PathBuf,
    pub after: u64,
    pub through: Option<u64>,
    pub limit: usize,
}

pub(super) fn parse(args: impl IntoIterator<Item = String>) -> VisionFfiResult<Options> {
    let mut args = args.into_iter();
    let mut state_root = None;
    let mut after = 0;
    let mut through = None;
    let mut limit = MAX_FORENSIC_EVENTS;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--state-root" => state_root = Some(PathBuf::from(argument_value(&mut args, &arg)?)),
            "--after" => after = argument_value(&mut args, &arg)?.parse().map_err(error)?,
            "--through" => through = Some(argument_value(&mut args, &arg)?.parse().map_err(error)?),
            "--limit" => limit = argument_value(&mut args, &arg)?.parse().map_err(error)?,
            _ => return Err(error(format!("unknown argument: {arg}"))),
        }
    }
    let options = Options {
        state_root: state_root.ok_or_else(|| error("--state-root is required"))?,
        after,
        through,
        limit,
    };
    request(&options)?;
    Ok(options)
}

fn request(options: &Options) -> VisionFfiResult<ForensicRequest> {
    let filter =
        ForensicEventFilter::new(Some("provider".into()), None, None, None).map_err(error)?;
    let page = ForensicEventsRequest::new(filter, options.after, options.through, options.limit)
        .map_err(error)?;
    Ok(ForensicRequest::events(&options.state_root, page))
}

pub(super) fn run(args: impl IntoIterator<Item = String>) -> VisionFfiResult<()> {
    let options = parse(args)?;
    let report = actingcommand_ledger_forensics::run(request(&options)?).map_err(error)?;
    let ForensicOutput::Machine(ForensicReport::Events(page)) = report else {
        return Err(error("B returned an unexpected report kind"));
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "observation": "runtime_ledger",
            "state_root": options.state_root,
            "page_has_provider_facts": !page.events.is_empty(),
            "inference": "not_observed_by_startup",
            "lazy_initialization": "not_observed_by_startup",
            "page": page,
        }))
        .map_err(error)?
    );
    Ok(())
}

fn error(error: impl std::fmt::Display) -> VisionFfiError {
    VisionFfiError::fatal("vision-provider-check", error.to_string())
}
