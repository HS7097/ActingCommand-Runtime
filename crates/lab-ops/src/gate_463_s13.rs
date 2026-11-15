// SPDX-License-Identifier: AGPL-3.0-only

// #463 S1-3 (register §1.404; contracts/lab-recording.md, "Application steps"): the rows `app
// launch|restart|stop` take `record`, `after_ms` and `still_max_ms`, and `apps::App` passes them on
// the line of its actinglab child. Without `recorded` the line is what it was; recorded, it ends with
// `--record`, then `--after-ms` and `--still-max-ms` when they are given, for each action. No child
// is started.

use crate::apps::{self, ACTIONS, App};
use crate::lab_child::Actinglab;
use std::path::Path;

const ALIAS: &str = "neutral.a";

// The arguments of `app`'s actinglab child, as MCP's child (`--via mcp`) starts it.
fn line(app: &App) -> Vec<String> {
    let lab = Actinglab {
        program: Path::new("actinglab"),
        installation: None,
        state_root: None,
        via: "mcp",
    };
    apps::command(&lab, app)
        .get_args()
        .map(|word| word.to_string_lossy().into_owned())
        .collect()
}

// `app` for `action` on the alias, as `App::new` makes it.
fn app(action: &str) -> App {
    App::new(action, ALIAS).unwrap_or_else(|| panic!("{action} is an action of session app"))
}

#[test]
fn s13_the_app_line_carries_record_and_its_waits_when_asked() {
    for action in ACTIONS {
        let plain = [
            "--json",
            "--via",
            "mcp",
            "session",
            "app",
            action,
            "--instance",
            ALIAS,
        ];
        assert_eq!(line(&app(action)), plain, "{action}");
        let mut recorded = plain.to_vec();
        recorded.push("--record");
        assert_eq!(
            line(&app(action).recorded(None, None)),
            recorded,
            "{action}"
        );
        let mut waits = recorded.clone();
        waits.extend(["--after-ms", "1500", "--still-max-ms", "0"]);
        assert_eq!(
            line(&app(action).recorded(Some(1500), Some(0))),
            waits,
            "{action}"
        );
        let mut after_only = recorded;
        after_only.extend(["--after-ms", "2500"]);
        assert_eq!(
            line(&app(action).recorded(Some(2500), None)),
            after_only,
            "{action}"
        );
    }
}
