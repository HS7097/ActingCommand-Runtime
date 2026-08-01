use super::{
    CliError, CliOutcome, FlagArgs, MatchMetric, SessionRecordRect, SessionRecordRegion,
    TouchBackendChoice,
};
use std::path::PathBuf;
use std::time::Duration;

pub(super) fn parse_optional_duration_ms(
    flags: &FlagArgs,
    name: &str,
    default_ms: u64,
) -> CliOutcome<Duration> {
    let Some(value) = flags.optional(name).filter(|value| value != "true") else {
        return Ok(Duration::from_millis(default_ms));
    };
    let ms = value
        .parse::<u64>()
        .map_err(|err| CliError::usage(format!("failed to parse {name} '{value}': {err}")))?;
    Ok(Duration::from_millis(ms))
}

pub(super) fn parse_optional_usize(
    flags: &FlagArgs,
    name: &str,
    default_value: usize,
) -> CliOutcome<usize> {
    let Some(value) = flags.optional(name).filter(|value| value != "true") else {
        return Ok(default_value);
    };
    value
        .parse::<usize>()
        .map_err(|err| CliError::usage(format!("failed to parse {name} '{value}': {err}")))
}

pub(super) fn parse_optional_string_value(
    flags: &FlagArgs,
    name: &str,
) -> CliOutcome<Option<String>> {
    match flags.optional(name) {
        None => Ok(None),
        Some(value) if value == "true" => Err(CliError::usage(format!("missing {name} <value>"))),
        Some(value) if value.trim().is_empty() => {
            Err(CliError::usage(format!("{name} must not be empty")))
        }
        Some(value) => Ok(Some(value)),
    }
}

pub(super) fn required_non_empty_flag(flags: &FlagArgs, name: &str) -> CliOutcome<String> {
    let value = flags.required(name)?;
    if value.trim().is_empty() {
        return Err(CliError::usage(format!("{name} must not be empty")));
    }
    Ok(value)
}

pub(super) fn parse_optional_unit_f64(flags: &FlagArgs, name: &str) -> CliOutcome<Option<f64>> {
    let Some(value) = flags.optional(name) else {
        return Ok(None);
    };
    if value == "true" {
        return Err(CliError::usage(format!("missing {name} <value>")));
    };
    let parsed = value
        .parse::<f64>()
        .map_err(|err| CliError::usage(format!("failed to parse {name} '{value}': {err}")))?;
    if !parsed.is_finite() || !(0.0..=1.0).contains(&parsed) {
        return Err(CliError::usage(format!(
            "{name} must be a finite number between 0 and 1"
        )));
    }
    Ok(Some(parsed))
}

pub(super) fn parse_record_duration_ms(flags: &FlagArgs, default_ms: u64) -> CliOutcome<u64> {
    let duration_ms = flags
        .optional("--duration-ms")
        .filter(|value| value != "true")
        .map(|value| {
            value.parse::<u64>().map_err(|err| {
                CliError::usage(format!("failed to parse --duration-ms '{value}': {err}"))
            })
        })
        .transpose()?
        .unwrap_or(default_ms);
    if duration_ms == 0 {
        return Err(CliError::usage("--duration-ms must be positive"));
    }
    Ok(duration_ms)
}

pub(super) fn record_amend_step_id(flags: &FlagArgs) -> CliOutcome<String> {
    let value = flags
        .optional("--step-id")
        .filter(|value| value != "true")
        .or_else(|| flags.positionals.first().cloned())
        .ok_or_else(|| CliError::usage("session record amend requires <step-id> or --step-id"))?;
    if value.trim().is_empty() {
        return Err(CliError::usage("record amend step id must not be empty"));
    }
    Ok(value)
}

pub(super) fn record_candidates_step_id(flags: &FlagArgs) -> CliOutcome<String> {
    let value = flags
        .optional("--step-id")
        .filter(|value| value != "true")
        .or_else(|| flags.positionals.first().cloned())
        .ok_or_else(|| {
            CliError::usage("session record candidates requires <step-id> or --step-id")
        })?;
    if value.trim().is_empty() {
        return Err(CliError::usage(
            "record candidates step id must not be empty",
        ));
    }
    Ok(value)
}

pub(super) fn stream_input_relay_action(
    flags: &FlagArgs,
) -> CliOutcome<Option<(String, Vec<String>)>> {
    let Some(value) = flags
        .optional("--input-relay")
        .or_else(|| flags.optional("--interactive-input"))
    else {
        return Ok(None);
    };
    if value == "true" {
        let action = flags.positionals.first().cloned().ok_or_else(|| {
            CliError::usage("stream --input-relay expects an action: tap|swipe|long-tap|key|text")
        })?;
        return Ok(Some((
            action,
            flags.positionals.iter().skip(1).cloned().collect(),
        )));
    }
    Ok(Some((value, flags.positionals.clone())))
}

pub(super) fn stream_check_requested(flags: &FlagArgs) -> bool {
    flags.positionals.first().map(String::as_str) == Some("check")
}

pub(super) fn target_argument(flags: &FlagArgs, command: &str) -> CliOutcome<String> {
    if let Some(target) = flags.optional("--target").filter(|value| value != "true") {
        return Ok(target);
    }
    flags
        .positionals
        .first()
        .cloned()
        .ok_or_else(|| CliError::usage(format!("{command} requires <target> or --target <id>")))
}

#[rustfmt::skip]
pub(super) fn session_record_drift_diagnostics_path(flags: &FlagArgs) -> CliOutcome<Option<PathBuf>> {
    let Some(value) = flags.optional("--from-drift-diagnostics") else {
        return Ok(None);
    };
    if value == "true" {
        return Err(CliError::usage(
            "session record amend --from-drift-diagnostics requires <path>",
        ));
    }
    Ok(Some(PathBuf::from(value)))
}

pub(super) fn parse_touch_backend_override(
    flags: &FlagArgs,
) -> CliOutcome<Option<TouchBackendChoice>> {
    let Some(value) = flags.optional("--touch-backend") else {
        return Ok(None);
    };
    if value == "true" {
        return Err(CliError::usage(
            "--touch-backend expects auto, auto-fastest, maatouch, minitouch, or adb_shell_input",
        ));
    }
    TouchBackendChoice::parse(&value)
        .map(Some)
        .map_err(|err| CliError::usage(err.to_string()))
}

pub(super) fn parse_match_metric_flag(flags: &FlagArgs) -> CliOutcome<MatchMetric> {
    match flags
        .optional("--metric")
        .unwrap_or_else(|| "ccorr_normed".to_string())
        .as_str()
    {
        "ccorr_normed" => Ok(MatchMetric::CrossCorrelationNormalized),
        "ccoeff_normed" => Ok(MatchMetric::CorrelationCoefficientNormalized),
        other => Err(CliError::usage(format!(
            "unsupported --metric '{other}', expected ccorr_normed or ccoeff_normed"
        ))),
    }
}

pub(super) fn parse_record_build_resolution(flags: &FlagArgs) -> CliOutcome<Option<(u32, u32)>> {
    let Some(value) = flags
        .optional("--resolution")
        .filter(|value| value != "true")
    else {
        return Ok(None);
    };
    let normalized = value.replace(['X', '*'], "x");
    let Some((width, height)) = normalized.split_once('x') else {
        return Err(CliError::usage(format!(
            "--resolution must use <width>x<height>, got {value}"
        )));
    };
    let width = width.trim().parse::<u32>().map_err(|err| {
        CliError::usage(format!(
            "failed to parse --resolution width '{width}': {err}"
        ))
    })?;
    let height = height.trim().parse::<u32>().map_err(|err| {
        CliError::usage(format!(
            "failed to parse --resolution height '{height}': {err}"
        ))
    })?;
    if width == 0 || height == 0 {
        return Err(CliError::usage(
            "--resolution width and height must be non-zero",
        ));
    }
    Ok(Some((width, height)))
}

pub(super) fn parse_session_record_region(value: &str) -> CliOutcome<SessionRecordRegion> {
    if value == "auto" {
        return Ok(SessionRecordRegion::Auto);
    }
    let parts = value.split(',').map(str::trim).collect::<Vec<_>>();
    if parts.len() != 4 {
        return Err(CliError::usage(format!(
            "record anchor region must be auto or x,y,width,height: {value}"
        )));
    }
    let parse_part = |index: usize, name: &str| {
        parts[index].parse::<i32>().map_err(|err| {
            CliError::usage(format!(
                "failed to parse record anchor region {name} '{}': {err}",
                parts[index]
            ))
        })
    };
    let rect = SessionRecordRect {
        x: parse_part(0, "x")?,
        y: parse_part(1, "y")?,
        width: parse_part(2, "width")?,
        height: parse_part(3, "height")?,
    };
    if rect.width <= 0 || rect.height <= 0 {
        return Err(CliError::usage(
            "record anchor region width and height must be positive",
        ));
    }
    Ok(SessionRecordRegion::Rect { rect })
}

#[rustfmt::skip]
pub(super) fn parse_session_record_rect(value: &str, label: &str) -> CliOutcome<SessionRecordRect> {
    let parts = value.split(',').map(str::trim).collect::<Vec<_>>();
    if parts.len() != 4 {
        return Err(CliError::usage(format!(
            "{label} must be formatted as x,y,width,height: {value}"
        )));
    }
    let parse = |index: usize, name: &str| {
        parts[index].parse::<i32>().map_err(|err| {
            CliError::usage(format!(
                "failed to parse {label} {name} '{}': {err}",
                parts[index]
            ))
        })
    };
    let rect = SessionRecordRect {
        x: parse(0, "x")?,
        y: parse(1, "y")?,
        width: parse(2, "width")?,
        height: parse(3, "height")?,
    };
    if rect.width <= 0 || rect.height <= 0 {
        return Err(CliError::usage(format!(
            "{label} dimensions must be positive: {}x{}",
            rect.width, rect.height
        )));
    }
    Ok(rect)
}

pub(super) fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(args: &[&str]) -> FlagArgs {
        FlagArgs::parse(
            &args
                .iter()
                .map(|value| (*value).to_string())
                .collect::<Vec<_>>(),
        )
        .expect("parse test flags")
    }

    #[test]
    fn match_metric_flag_preserves_default_values_and_rejection() {
        assert_eq!(
            parse_match_metric_flag(&FlagArgs::default()).expect("default metric"),
            MatchMetric::CrossCorrelationNormalized
        );
        assert_eq!(
            parse_match_metric_flag(&flags(&["--metric", "ccorr_normed"]))
                .expect("ccorr_normed metric"),
            MatchMetric::CrossCorrelationNormalized
        );
        assert_eq!(
            parse_match_metric_flag(&flags(&["--metric", "ccoeff_normed"]))
                .expect("ccoeff_normed metric"),
            MatchMetric::CorrelationCoefficientNormalized
        );

        let error = parse_match_metric_flag(&flags(&["--metric", "unsupported"]))
            .expect_err("unsupported metric must fail");
        assert_eq!(
            error.message,
            "unsupported --metric 'unsupported', expected ccorr_normed or ccoeff_normed"
        );
    }

    #[test]
    fn parse_record_build_resolution_preserves_absence_bare_true_normalization_parsing_errors_and_valid_values()
     {
        assert_eq!(
            parse_record_build_resolution(&FlagArgs::default()).expect("absent resolution"),
            None
        );
        assert_eq!(
            parse_record_build_resolution(&flags(&["--resolution"])).expect("bare true resolution"),
            None
        );
        assert_eq!(
            parse_record_build_resolution(&flags(&["--resolution", "1280X720"]))
                .expect("uppercase X resolution"),
            Some((1280, 720))
        );
        assert_eq!(
            parse_record_build_resolution(&flags(&["--resolution", "1280*720"]))
                .expect("asterisk resolution"),
            Some((1280, 720))
        );
        assert_eq!(
            parse_record_build_resolution(&flags(&["--resolution", " 1280 x 720 "]))
                .expect("trimmed resolution"),
            Some((1280, 720))
        );
        assert_eq!(
            parse_record_build_resolution(&flags(&["--resolution", "1920x1080"]))
                .expect("valid resolution"),
            Some((1920, 1080))
        );

        let malformed = parse_record_build_resolution(&flags(&["--resolution", "1280"]))
            .expect_err("missing resolution separator must fail");
        assert_eq!(
            malformed.message,
            "--resolution must use <width>x<height>, got 1280"
        );

        let width_error = parse_record_build_resolution(&flags(&["--resolution", "oopsx720"]))
            .expect_err("invalid width must fail");
        let expected_width_error = "oops".parse::<u32>().expect_err("invalid test width");
        assert_eq!(
            width_error.message,
            format!("failed to parse --resolution width 'oops': {expected_width_error}")
        );

        let height_error = parse_record_build_resolution(&flags(&["--resolution", "1280xoops"]))
            .expect_err("invalid height must fail");
        let expected_height_error = "oops".parse::<u32>().expect_err("invalid test height");
        assert_eq!(
            height_error.message,
            format!("failed to parse --resolution height 'oops': {expected_height_error}")
        );

        for value in ["0x720", "1280x0"] {
            let zero = parse_record_build_resolution(&flags(&["--resolution", value]))
                .expect_err("zero resolution dimension must fail");
            assert_eq!(
                zero.message,
                "--resolution width and height must be non-zero"
            );
        }
    }

    #[test]
    fn parse_session_record_region_preserves_auto_rect_whitespace_parse_errors_and_positive_dimensions()
     {
        assert!(matches!(
            parse_session_record_region("auto").expect("automatic record region"),
            SessionRecordRegion::Auto
        ));

        for (value, expected) in [
            ("10,20,30,40", (10, 20, 30, 40)),
            (" 10 , 20 , 30 , 40 ", (10, 20, 30, 40)),
        ] {
            let SessionRecordRegion::Rect { rect } =
                parse_session_record_region(value).expect("rectangular record region")
            else {
                panic!("record region must be rectangular");
            };
            assert_eq!((rect.x, rect.y, rect.width, rect.height), expected);
        }

        let malformed = parse_session_record_region("1,2,3")
            .expect_err("record region with the wrong part count must fail");
        assert_eq!(
            malformed.message,
            "record anchor region must be auto or x,y,width,height: 1,2,3"
        );

        let expected_parse_error = "oops".parse::<i32>().expect_err("invalid test component");
        for (value, name) in [
            ("oops,2,3,4", "x"),
            ("1,oops,3,4", "y"),
            ("1,2,oops,4", "width"),
            ("1,2,3,oops", "height"),
        ] {
            let error = parse_session_record_region(value)
                .expect_err("invalid record region component must fail");
            assert_eq!(
                error.message,
                format!(
                    "failed to parse record anchor region {name} 'oops': {expected_parse_error}"
                )
            );
        }

        for value in ["1,2,0,4", "1,2,3,0", "1,2,-1,4", "1,2,3,-1"] {
            let error = parse_session_record_region(value)
                .expect_err("non-positive record region dimension must fail");
            assert_eq!(
                error.message,
                "record anchor region width and height must be positive"
            );
        }
    }

    #[test]
    fn parse_session_record_rect_preserves_whitespace_parse_order_labels_errors_and_positive_dimensions()
     {
        for (value, expected) in [
            ("10,20,30,40", (10, 20, 30, 40)),
            (" 10 , 20 , 30 , 40 ", (10, 20, 30, 40)),
        ] {
            let rect = parse_session_record_rect(value, "--swipe from")
                .expect("valid session record rectangle");
            assert_eq!((rect.x, rect.y, rect.width, rect.height), expected);
        }

        let malformed = parse_session_record_rect("1,2,3", "--swipe from")
            .expect_err("record rectangle with the wrong part count must fail");
        assert_eq!(
            malformed.message,
            "--swipe from must be formatted as x,y,width,height: 1,2,3"
        );

        let expected_parse_error = "oops"
            .parse::<i32>()
            .expect_err("invalid test rectangle component");
        for (value, name) in [
            ("oops,2,3,4", "x"),
            ("1,oops,3,4", "y"),
            ("1,2,oops,4", "width"),
            ("1,2,3,oops", "height"),
        ] {
            let error = parse_session_record_rect(value, "--swipe to")
                .expect_err("invalid record rectangle component must fail");
            assert_eq!(
                error.message,
                format!("failed to parse --swipe to {name} 'oops': {expected_parse_error}")
            );
        }

        for (value, width, height) in [
            ("1,2,0,4", 0, 4),
            ("1,2,3,0", 3, 0),
            ("1,2,-1,4", -1, 4),
            ("1,2,3,-1", 3, -1),
        ] {
            let error = parse_session_record_rect(value, "--swipe from")
                .expect_err("non-positive record rectangle dimension must fail");
            assert_eq!(
                error.message,
                format!("--swipe from dimensions must be positive: {width}x{height}")
            );
        }
    }

    #[test]
    fn record_candidates_step_id_preserves_precedence_fallback_errors_and_original_value() {
        assert_eq!(
            record_candidates_step_id(&flags(&["positional", "--step-id", "flagged"]))
                .expect("flag value takes precedence"),
            "flagged"
        );
        assert_eq!(
            record_candidates_step_id(&flags(&["positional"])).expect("first positional fallback"),
            "positional"
        );

        let missing =
            record_candidates_step_id(&FlagArgs::default()).expect_err("missing step id must fail");
        assert_eq!(
            missing.message,
            "session record candidates requires <step-id> or --step-id"
        );

        let empty = record_candidates_step_id(&flags(&["--step-id", "  "]))
            .expect_err("trim-empty step id must fail");
        assert_eq!(empty.message, "record candidates step id must not be empty");

        assert_eq!(
            record_candidates_step_id(&flags(&["--step-id", "  original  "]))
                .expect("original step id"),
            "  original  "
        );
    }

    #[test]
    fn stream_input_relay_action_preserves_precedence_fallback_absence_literal_true_errors_and_arguments()
     {
        assert_eq!(
            stream_input_relay_action(&flags(&[
                "--interactive-input",
                "swipe",
                "--input-relay",
                "tap",
                "primary-arg",
            ]))
            .expect("input-relay precedence"),
            Some(("tap".to_string(), vec!["primary-arg".to_string()]))
        );
        assert_eq!(
            stream_input_relay_action(&flags(&[
                "--interactive-input",
                "swipe",
                "fallback-arg-1",
                "fallback-arg-2",
            ]))
            .expect("interactive-input fallback"),
            Some((
                "swipe".to_string(),
                vec!["fallback-arg-1".to_string(), "fallback-arg-2".to_string(),],
            ))
        );
        assert_eq!(
            stream_input_relay_action(&FlagArgs::default()).expect("absent input relay"),
            None
        );

        let literal_true_flags = flags(&["tap", "10", "20", "--input-relay"]);
        assert_eq!(
            stream_input_relay_action(&literal_true_flags).expect("literal true input relay"),
            Some(("tap".to_string(), vec!["10".to_string(), "20".to_string()],))
        );
        assert_eq!(
            literal_true_flags.positionals,
            vec!["tap".to_string(), "10".to_string(), "20".to_string()],
            "literal-true handling must clone rather than consume positionals"
        );

        let missing = stream_input_relay_action(&flags(&["--input-relay"]))
            .expect_err("literal true without a positional action must fail");
        assert_eq!(
            missing.message,
            "stream --input-relay expects an action: tap|swipe|long-tap|key|text"
        );

        let explicit_flags = flags(&["arg-1", "arg-2", "--input-relay", "text"]);
        assert_eq!(
            stream_input_relay_action(&explicit_flags).expect("explicit relay action"),
            Some((
                "text".to_string(),
                vec!["arg-1".to_string(), "arg-2".to_string()],
            ))
        );
        assert_eq!(
            explicit_flags.positionals,
            vec!["arg-1".to_string(), "arg-2".to_string()],
            "explicit-action handling must clone all positional arguments"
        );
    }
}
