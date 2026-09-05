use super::*;

struct LedgerFile {
    path: PathBuf,
    read: LedgerRead,
}

fn run_legacy_ledger(sub: &str, global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    match sub {
        "show" => run_ledger_show(global, &flags),
        "events" => run_ledger_events(global, &flags),
        "receipts" => run_ledger_receipts(global, &flags),
        "diagnose" => run_ledger_diagnose(global, &flags),
        "evidence" => run_ledger_evidence(global, &flags),
        other => Err(CliError::usage(format!(
            "unknown ledger command: {other}; expected show, events, receipts, diagnose, or evidence"
        ))),
    }
}

fn run_ledger_show(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let filter = LedgerFilter::from_flags(flags)?;
    let run_root = ledger_run_root(global, flags)?;
    let entries = read_ledger_files(&run_root)?;
    let limit = parse_optional_usize(flags, "--limit", 200)?;
    let mut records = Vec::new();
    let mut events = Vec::new();
    for entry in &entries {
        for record in &entry.read.records {
            if filter.matches_record(record, &entry.path, entry.read.header.as_ref()) {
                records.push(json!({
                    "ledger_path": entry.path.display().to_string(),
                    "kind": record.kind.as_str(),
                    "record": record
                }));
            }
        }
        for event in &entry.read.events {
            if filter.matches_event(event, &entry.path, entry.read.header.as_ref()) {
                events.push(json!({
                    "ledger_path": entry.path.display().to_string(),
                    "event": event
                }));
            }
        }
    }
    let record_count = records.len();
    let event_count = events.len();
    records.truncate(limit);
    events.truncate(limit);
    Ok(json!({
        "schema_version": "actingcommand.ledger.show.v0.1",
        "run_root": run_root.display().to_string(),
        "filter": filter.to_json(),
        "ledgers_scanned": entries.len(),
        "skipped_corrupt_lines": skipped_corrupt_lines(&entries),
        "record_count": record_count,
        "event_count": event_count,
        "records_more": record_count.saturating_sub(records.len()),
        "events_more": event_count.saturating_sub(events.len()),
        "records": records,
        "events": events
    }))
}

fn run_ledger_events(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let filter = LedgerFilter::from_flags(flags)?;
    let run_root = ledger_run_root(global, flags)?;
    let entries = read_ledger_files(&run_root)?;
    let limit = parse_optional_usize(flags, "--limit", 200)?;
    let mut events = Vec::new();
    for entry in &entries {
        for event in &entry.read.events {
            if filter.matches_event(event, &entry.path, entry.read.header.as_ref()) {
                events.push(json!({
                    "ledger_path": entry.path.display().to_string(),
                    "event": event
                }));
            }
        }
    }
    let event_count = events.len();
    events.truncate(limit);
    Ok(json!({
        "schema_version": "actingcommand.ledger.events.v0.1",
        "run_root": run_root.display().to_string(),
        "filter": filter.to_json(),
        "ledgers_scanned": entries.len(),
        "skipped_corrupt_lines": skipped_corrupt_lines(&entries),
        "event_count": event_count,
        "events_more": event_count.saturating_sub(events.len()),
        "events": events
    }))
}

fn run_ledger_receipts(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let req_id = flags.required("--req-id")?;
    let filter = LedgerFilter::for_req(req_id.clone());
    let run_root = ledger_run_root(global, flags)?;
    let entries = read_ledger_files(&run_root)?;
    let mut receipts = Vec::new();
    for entry in &entries {
        for record in &entry.read.records {
            if record.kind == LedgerRecordKind::Receipt
                && filter.matches_record(record, &entry.path, entry.read.header.as_ref())
            {
                receipts.push(json!({
                    "ledger_path": entry.path.display().to_string(),
                    "record": record
                }));
            }
        }
    }
    Ok(json!({
        "schema_version": "actingcommand.ledger.receipts.v0.1",
        "run_root": run_root.display().to_string(),
        "req_id": req_id,
        "ledgers_scanned": entries.len(),
        "skipped_corrupt_lines": skipped_corrupt_lines(&entries),
        "receipt_count": receipts.len(),
        "receipts": receipts
    }))
}

fn run_ledger_diagnose(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let filter = LedgerFilter::from_flags(flags)?;
    let run_root = ledger_run_root(global, flags)?;
    let entries = read_ledger_files(&run_root)?;
    let mut matching_records = Vec::new();
    let mut matching_events = Vec::new();
    for entry in &entries {
        for record in &entry.read.records {
            if filter.matches_record(record, &entry.path, entry.read.header.as_ref()) {
                matching_records.push((entry.path.clone(), record.clone()));
            }
        }
        for event in &entry.read.events {
            if filter.matches_event(event, &entry.path, entry.read.header.as_ref()) {
                matching_events.push((entry.path.clone(), event.clone()));
            }
        }
    }
    let receipt_records = matching_records
        .iter()
        .filter(|(_, record)| record.kind == LedgerRecordKind::Receipt)
        .collect::<Vec<_>>();
    let finalizing_count = matching_records
        .iter()
        .filter(|(_, record)| record_type(record) == Some("finalizing"))
        .count();
    let terminal = receipt_records
        .iter()
        .rev()
        .find(|(_, record)| matches!(record_type(record), Some("finish_ok" | "finish_error")))
        .copied();
    let status = terminal
        .and_then(|(_, record)| record.payload.get("status").and_then(Value::as_str))
        .or_else(|| {
            receipt_records
                .iter()
                .rev()
                .find_map(|(_, record)| record.payload.get("state").and_then(Value::as_str))
        })
        .unwrap_or(
            if matching_records.is_empty() && matching_events.is_empty() {
                "not_found"
            } else {
                "incomplete"
            },
        );
    let output_zip = terminal.and_then(|(_, record)| record.payload.get("output_zip").cloned());
    let output_zip_exists = output_zip
        .as_ref()
        .and_then(|zip| zip.get("path"))
        .and_then(Value::as_str)
        .map(|path| Path::new(path).exists());
    Ok(json!({
        "schema_version": "actingcommand.ledger.diagnose.v0.1",
        "run_root": run_root.display().to_string(),
        "filter": filter.to_json(),
        "status": status,
        "ledgers_scanned": entries.len(),
        "skipped_corrupt_lines": skipped_corrupt_lines(&entries),
        "record_count": matching_records.len(),
        "event_count": matching_events.len(),
        "receipt_count": receipt_records.len(),
        "finalizing_count": finalizing_count,
        "terminal_receipt": terminal.map(|(path, record)| json!({
            "ledger_path": path.display().to_string(),
            "record": record
        })),
        "output_zip": output_zip,
        "output_zip_exists": output_zip_exists,
        "diagnostics": ledger_diagnosis_warnings(
            status,
            finalizing_count,
            receipt_records.len(),
            output_zip_exists
        )
    }))
}

fn run_ledger_evidence(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let evidence_id = flags.required("--evidence-id")?;
    let run_root = ledger_run_root(global, flags)?;
    let refs = EvidenceStore::new(&run_root, true)
        .list_by_id(&evidence_id)
        .map_err(|err| CliError::device(err.to_string()))?;
    Ok(json!({
        "schema_version": "actingcommand.ledger.evidence.v0.1",
        "run_root": run_root.display().to_string(),
        "evidence_id": evidence_id,
        "evidence_count": refs.len(),
        "evidence": refs
    }))
}

#[derive(Debug)]
struct LedgerFilter {
    run_id: Option<String>,
    req_id: Option<String>,
    instance_id: Option<String>,
}

impl LedgerFilter {
    fn from_flags(flags: &FlagArgs) -> CliOutcome<Self> {
        let filter = Self {
            run_id: flags.optional("--run-id").filter(|value| value != "true"),
            req_id: flags
                .optional("--req-id")
                .or_else(|| flags.optional("--request-id"))
                .filter(|value| value != "true"),
            instance_id: flags
                .optional("--instance-id")
                .or_else(|| flags.optional("--instance"))
                .filter(|value| value != "true"),
        };
        if filter.run_id.is_none() && filter.req_id.is_none() && filter.instance_id.is_none() {
            return Err(CliError::usage(
                "ledger query requires --run-id, --req-id, or --instance-id",
            ));
        }
        Ok(filter)
    }

    fn for_req(req_id: String) -> Self {
        Self {
            run_id: None,
            req_id: Some(req_id),
            instance_id: None,
        }
    }

    fn matches_record(
        &self,
        record: &LedgerRecord,
        path: &Path,
        header: Option<&SessionHeader>,
    ) -> bool {
        self.run_id
            .as_ref()
            .is_none_or(|run_id| record_contains_id(record, path, "run_id", run_id))
            && self.req_id.as_ref().is_none_or(|req_id| {
                record.req_id.as_deref() == Some(req_id)
                    || record_contains_id(record, path, "req_id", req_id)
            })
            && self.instance_id.as_ref().is_none_or(|instance_id| {
                header.is_some_and(|header| header.instance == *instance_id)
                    || record_contains_id(record, path, "instance", instance_id)
                    || record_contains_id(record, path, "instance_id", instance_id)
            })
    }

    fn matches_event(
        &self,
        event: &LightEvent,
        path: &Path,
        header: Option<&SessionHeader>,
    ) -> bool {
        self.run_id
            .as_ref()
            .is_none_or(|run_id| event_contains_id(event, path, "run_id", run_id))
            && self.req_id.as_ref().is_none_or(|req_id| {
                event.ids.get("req_id").is_some_and(|value| value == req_id)
                    || event_contains_id(event, path, "req_id", req_id)
            })
            && self.instance_id.as_ref().is_none_or(|instance_id| {
                header.is_some_and(|header| header.instance == *instance_id)
                    || event_contains_id(event, path, "instance", instance_id)
                    || event_contains_id(event, path, "instance_id", instance_id)
            })
    }

    fn to_json(&self) -> Value {
        json!({
            "run_id": self.run_id,
            "req_id": self.req_id,
            "instance_id": self.instance_id
        })
    }
}

fn ledger_run_root(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<PathBuf> {
    if let Some(path) = flags.optional_path("--run-root") {
        return Ok(path);
    }
    let config = read_user_config()?;
    effective_run_root(global, &config)
        .ok_or_else(|| CliError::usage("ledger query requires --run-root or config run_root"))
}

fn read_ledger_files(run_root: &Path) -> CliOutcome<Vec<LedgerFile>> {
    let mut paths = Vec::new();
    collect_runtime_ledger_paths(run_root, &mut paths)?;
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let read = LabLedger::read(&path).map_err(|err| {
                CliError::device(format!("failed to read ledger {}: {err}", path.display()))
            })?;
            Ok(LedgerFile { path, read })
        })
        .collect()
}

fn collect_runtime_ledger_paths(root: &Path, paths: &mut Vec<PathBuf>) -> CliOutcome<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root)
        .map_err(|err| CliError::device(format!("failed to read {}: {err}", root.display())))?
    {
        let entry = entry.map_err(|err| CliError::device(err.to_string()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_runtime_ledger_paths(&path, paths)?;
        } else if path.file_name().and_then(|name| name.to_str()) == Some("ledger.jsonl") {
            paths.push(path);
        }
    }
    Ok(())
}

fn skipped_corrupt_lines(entries: &[LedgerFile]) -> usize {
    entries
        .iter()
        .map(|entry| entry.read.skipped_corrupt_lines)
        .sum()
}

fn record_contains_id(record: &LedgerRecord, path: &Path, key: &str, expected: &str) -> bool {
    record
        .id_chain
        .get(key)
        .is_some_and(|value| value == expected)
        || value_contains_id(&record.payload, key, expected)
        || path_contains_segment(path, expected)
}

fn event_contains_id(event: &LightEvent, path: &Path, key: &str, expected: &str) -> bool {
    event.ids.get(key).is_some_and(|value| value == expected)
        || value_contains_id(&event.payload, key, expected)
        || path_contains_segment(path, expected)
}

fn value_contains_id(value: &Value, key: &str, expected: &str) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(item_key, item)| {
            (item_key == key && item.as_str() == Some(expected))
                || value_contains_id(item, key, expected)
        }),
        Value::Array(items) => items
            .iter()
            .any(|item| value_contains_id(item, key, expected)),
        _ => false,
    }
}

fn path_contains_segment(path: &Path, expected: &str) -> bool {
    path.components()
        .any(|component| component.as_os_str().to_string_lossy() == expected)
}

fn record_type(record: &LedgerRecord) -> Option<&str> {
    record.payload.get("record_type").and_then(Value::as_str)
}

fn ledger_diagnosis_warnings(
    status: &str,
    finalizing_count: usize,
    receipt_count: usize,
    output_zip_exists: Option<bool>,
) -> Vec<Value> {
    let mut diagnostics = Vec::new();
    if finalizing_count == 0 {
        diagnostics.push(json!({
            "severity": "warning",
            "code": "missing_finalizing",
            "message": "runtime ledger query did not find a finalizing record"
        }));
    }
    if receipt_count == 0 {
        diagnostics.push(json!({
            "severity": "warning",
            "code": "missing_receipt",
            "message": "runtime ledger query did not find a receipt record"
        }));
    }
    if status == "ok" && output_zip_exists == Some(false) {
        diagnostics.push(json!({
            "severity": "error",
            "code": "terminal_output_missing",
            "message": "ledger reports ok but the recorded output zip path does not exist"
        }));
    }
    diagnostics
}
