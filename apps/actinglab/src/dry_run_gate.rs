// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #341: the global `--dry-run` flag is never silently ignored. A side-effecting command
// either previews (sends, controls and writes nothing) or refuses with `dry_run_unsupported`
// before its first Runtime connection and its first write. Every command declares its mode in
// the table below; `contracts/actinglab-dry-run.md` is the contract.

use crate::{CliError, CliOutcome, ErrorKind, GlobalOptions};
use serde_json::{Map, Value, json};

pub(crate) const REFUSAL_CODE: &str = "dry_run_unsupported";

/// `record` / `session record` actions refused under `--dry-run`.
pub(crate) const RECORD_REFUSED_ACTIONS: &[&str] = &["start", "step", "amend"];

/// Fails with `dry_run_unsupported` (exit 2) when the global `--dry-run` flag is set; otherwise
/// returns `Ok(())` and the command continues. `extra` (an object) is merged into the details.
pub(crate) fn refuse(
    global: &GlobalOptions,
    form: &str,
    read_only_alternative: Option<&str>,
    extra: Value,
) -> CliOutcome<()> {
    if !global.dry_run {
        return Ok(());
    }
    let next = match read_only_alternative {
        Some(alternative) => format!("Run `{alternative}` for a read-only view"),
        None => "There is no read-only alternative".to_string(),
    };
    let message = format!(
        "{form} has no dry run; this is not an argument error and nothing was sent or written. \
         {next}; drop --dry-run only if the user asked for this action."
    );
    let mut details = Map::new();
    details.insert("form".to_string(), json!(form));
    details.insert("dry_run".to_string(), json!(true));
    details.insert("executed".to_string(), json!(false));
    details.insert(
        "read_only_alternative".to_string(),
        json!(read_only_alternative),
    );
    if let Value::Object(extra) = extra {
        for (key, value) in extra {
            details.entry(key).or_insert(value);
        }
    }
    Err(
        CliError::new(ErrorKind::UsageValidation, REFUSAL_CODE, message, &[])
            .with_details(Value::Object(details)),
    )
}

/// The preview of a direct input command (tap, swipe, long-tap, key, text): nothing is sent and
/// no Runtime connection is opened, so the instance, the lease and the foreground application are
/// not checked.
pub(crate) fn input_preview(instance: &str, action: Value) -> Value {
    json!({
        "status": "planned",
        "mode": "dry_run",
        "dry_run": true,
        "executed": false,
        "backend": "runtime_proxy",
        "control_mode": "direct_trusted_manual",
        "instance": instance,
        "action": action,
        "checked": ["arguments", "instance_selection"],
        "not_checked": ["runtime_instance", "lease", "foreground_application"],
        "input_outcome": {"input_stage": "not_submitted", "input_receipt": Value::Null}
    })
}

#[derive(Clone, Copy)]
enum Mode {
    /// Previews; the value is the `dry_run_marker` its data carries.
    Preview(&'static str),
    /// Rejects the flag; the value is the `dry_run_refusal_code`.
    Refused(&'static str),
    NoEffect,
    /// The forms differ; each form declares its own mode.
    Mixed(&'static [(&'static str, Mode)]),
}

const DRY_RUN: Mode = Mode::Preview("dry_run");
const EXECUTED_FALSE: Mode = Mode::Preview("executed_false");
const UNSUPPORTED: Mode = Mode::Refused(REFUSAL_CODE);

const CAPTURE_FORMS: &[(&str, Mode)] = &[
    ("--out <path>", UNSUPPORTED),
    ("--record", UNSUPPORTED),
    ("diagnose", Mode::NoEffect),
];
const OBSERVE_FORMS: &[(&str, Mode)] = &[
    (
        "--capture --record",
        Mode::Refused("record_flag_unsupported"),
    ),
    ("--with-frame <path>", UNSUPPORTED),
    ("without --with-frame <path> or --record", Mode::NoEffect),
];
const STREAM_FORMS: &[(&str, Mode)] = &[
    ("check", Mode::NoEffect),
    ("other forms", Mode::Preview("capture_dry_run")),
];
const MONITOR_POLICY_FORMS: &[(&str, Mode)] = &[
    ("status", Mode::NoEffect),
    ("set", UNSUPPORTED),
    ("clear", UNSUPPORTED),
];
const SESSION_INSTANCE_FORMS: &[(&str, Mode)] = &[
    ("list", Mode::NoEffect),
    ("registry", Mode::NoEffect),
    (
        "connect|health|keep-alive|reconnect (retired)",
        Mode::NoEffect,
    ),
    ("app <launch|stop|force-stop|restart>", UNSUPPORTED),
];
const RECORD_FORMS: &[(&str, Mode)] = &[
    ("start", UNSUPPORTED),
    ("stop", DRY_RUN),
    ("mark", DRY_RUN),
    ("step", UNSUPPORTED),
    ("amend", UNSUPPORTED),
    ("status", Mode::NoEffect),
    ("candidates", Mode::NoEffect),
    ("build-task", DRY_RUN),
    ("promote", DRY_RUN),
];

/// The declaration table of `commands[].dry_run_mode`. A capability entry not listed here is
/// `no_effect` when its status is retired, reserved or unavailable, and `undeclared` otherwise.
const DECLARATIONS: &[(&str, Mode)] = &[
    // preview
    ("config set", DRY_RUN),
    ("resource convert", DRY_RUN),
    ("package build-task", DRY_RUN),
    ("package build-pack", DRY_RUN),
    ("tap", DRY_RUN),
    ("swipe", DRY_RUN),
    ("long-tap", DRY_RUN),
    ("key", DRY_RUN),
    ("text", DRY_RUN),
    ("detect", DRY_RUN),
    ("record build-task", DRY_RUN),
    ("record promote", DRY_RUN),
    ("record stop", DRY_RUN),
    ("record mark", DRY_RUN),
    ("session record build-task", DRY_RUN),
    ("session record promote", DRY_RUN),
    ("session record stop", DRY_RUN),
    ("session record mark", DRY_RUN),
    ("do", EXECUTED_FALSE),
    ("ensure", EXECUTED_FALSE),
    ("tap-target", EXECUTED_FALSE),
    ("navigate", EXECUTED_FALSE),
    ("session recover", EXECUTED_FALSE),
    ("session stream", Mode::Preview("capture_dry_run")),
    // refused
    ("capture --record", UNSUPPORTED),
    ("session capture --record", UNSUPPORTED),
    (
        "session app --record",
        Mode::Refused("record_flag_unsupported"),
    ),
    (
        "session instance app --record",
        Mode::Refused("record_flag_unsupported"),
    ),
    (
        "observe --capture --record",
        Mode::Refused("record_flag_unsupported"),
    ),
    (
        "do --capture --record",
        Mode::Refused("record_flag_unsupported"),
    ),
    ("session app", UNSUPPORTED),
    ("session app launch", UNSUPPORTED),
    ("session app stop", UNSUPPORTED),
    ("session app force-stop", UNSUPPORTED),
    ("session app restart", UNSUPPORTED),
    ("session instance app", UNSUPPORTED),
    ("session instance app launch", UNSUPPORTED),
    ("session instance app stop", UNSUPPORTED),
    ("session instance app force-stop", UNSUPPORTED),
    ("session instance app restart", UNSUPPORTED),
    ("touch-probe", UNSUPPORTED),
    ("record start", UNSUPPORTED),
    ("record step", UNSUPPORTED),
    ("record amend", UNSUPPORTED),
    ("session record start", UNSUPPORTED),
    ("session record step", UNSUPPORTED),
    ("session record amend", UNSUPPORTED),
    ("lab signatures register", UNSUPPORTED),
    ("lab signatures match", UNSUPPORTED),
    ("lab signatures retire", UNSUPPORTED),
    ("lab unpin", UNSUPPORTED),
    ("lab debug-package", UNSUPPORTED),
    ("lab export-evidence", UNSUPPORTED),
    ("package bundle", UNSUPPORTED),
    ("package run", UNSUPPORTED),
    ("resource restore", UNSUPPORTED),
    ("lab run", Mode::Refused("explicit_offline_entry_required")),
    (
        "package dry-run",
        Mode::Refused("offline_device_scope_forbidden"),
    ),
    ("scheduling compile", Mode::Refused("validation_failed")),
    ("scheduling timeline", Mode::Refused("validation_failed")),
    // mixed
    ("capture", Mode::Mixed(CAPTURE_FORMS)),
    ("session capture", Mode::Mixed(CAPTURE_FORMS)),
    ("observe", Mode::Mixed(OBSERVE_FORMS)),
    ("stream", Mode::Mixed(STREAM_FORMS)),
    ("session monitor-policy", Mode::Mixed(MONITOR_POLICY_FORMS)),
    ("session instance", Mode::Mixed(SESSION_INSTANCE_FORMS)),
    ("record", Mode::Mixed(RECORD_FORMS)),
    ("session record", Mode::Mixed(RECORD_FORMS)),
    // no_effect
    ("version", Mode::NoEffect),
    ("doctor", Mode::NoEffect),
    ("paths", Mode::NoEffect),
    ("config get", Mode::NoEffect),
    ("schema", Mode::NoEffect),
    ("list", Mode::NoEffect),
    ("capabilities", Mode::NoEffect),
    ("status", Mode::NoEffect),
    ("resource validate", Mode::NoEffect),
    ("resource catalog", Mode::NoEffect),
    ("resource compile-maa", Mode::NoEffect),
    ("resource check-release", Mode::NoEffect),
    ("wait", Mode::NoEffect),
    ("package validate", Mode::NoEffect),
    ("package preflight", Mode::NoEffect),
    ("package inspect", Mode::NoEffect),
    ("package digest", Mode::NoEffect),
    ("operation validate", Mode::NoEffect),
    ("operation inspect", Mode::NoEffect),
    ("operation explain", Mode::NoEffect),
    ("run summary", Mode::NoEffect),
    ("session status", Mode::NoEffect),
    ("session throat-policy", Mode::NoEffect),
    ("session capture-policy", Mode::NoEffect),
    ("session record-policy", Mode::NoEffect),
    ("session self-heal-policy", Mode::NoEffect),
    ("session contract", Mode::NoEffect),
    ("session api", Mode::NoEffect),
    ("session transport", Mode::NoEffect),
    ("session transport plan", Mode::NoEffect),
    ("session transport check", Mode::NoEffect),
    ("session stream check", Mode::NoEffect),
    ("session instance list", Mode::NoEffect),
    ("session instance registry", Mode::NoEffect),
    ("session capture diagnose", Mode::NoEffect),
    ("session recover --stale-capture", Mode::NoEffect),
    ("record status", Mode::NoEffect),
    ("record candidates", Mode::NoEffect),
    ("session record status", Mode::NoEffect),
    ("session record candidates", Mode::NoEffect),
    ("current-page", Mode::NoEffect),
    ("is-visible", Mode::NoEffect),
    ("locate", Mode::NoEffect),
    ("lab validate", Mode::NoEffect),
    ("lab status", Mode::NoEffect),
    ("lab receipt", Mode::NoEffect),
    ("lab watch", Mode::NoEffect),
    ("lab replay-evidence", Mode::NoEffect),
    ("lab evidence", Mode::NoEffect),
    ("capture diagnose", Mode::NoEffect),
    ("env resolve", Mode::NoEffect),
    ("env status", Mode::NoEffect),
    ("detect-page", Mode::NoEffect),
    ("recognize", Mode::NoEffect),
    ("recognize-artifact", Mode::NoEffect),
];

/// Adds `dry_run_mode` and its conditional field (`dry_run_marker`, `dry_run_refusal_code` or
/// `dry_run_forms`) to one capability entry built by `command_cap`.
pub(crate) fn annotate_capability(capability: &mut Value) {
    let declared = capability["command"].as_str().and_then(|command| {
        DECLARATIONS
            .iter()
            .find(|(name, _)| *name == command)
            .map(|(_, mode)| *mode)
    });
    let mode = declared.or_else(|| {
        matches!(
            capability["status"].as_str(),
            Some("retired" | "reserved" | "unavailable")
        )
        .then_some(Mode::NoEffect)
    });
    let fields = match mode {
        Some(mode) => mode_fields(mode),
        None => Map::from_iter([("dry_run_mode".to_string(), json!("undeclared"))]),
    };
    if let Value::Object(capability) = capability {
        capability.extend(fields);
    }
}

fn mode_fields(mode: Mode) -> Map<String, Value> {
    let mut fields = Map::new();
    match mode {
        Mode::Preview(marker) => {
            fields.insert("dry_run_mode".to_string(), json!("preview"));
            fields.insert("dry_run_marker".to_string(), json!(marker));
        }
        Mode::Refused(code) => {
            fields.insert("dry_run_mode".to_string(), json!("refused"));
            fields.insert("dry_run_refusal_code".to_string(), json!(code));
        }
        Mode::NoEffect => {
            fields.insert("dry_run_mode".to_string(), json!("no_effect"));
        }
        Mode::Mixed(forms) => {
            fields.insert("dry_run_mode".to_string(), json!("mixed"));
            let forms = forms
                .iter()
                .map(|(form, mode)| {
                    let mut entry = mode_fields(*mode);
                    entry.insert("form".to_string(), json!(form));
                    Value::Object(entry)
                })
                .collect::<Vec<_>>();
            fields.insert("dry_run_forms".to_string(), Value::Array(forms));
        }
    }
    fields
}
