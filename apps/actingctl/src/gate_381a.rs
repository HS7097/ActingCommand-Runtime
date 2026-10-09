// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #381 A, test plan A-2 G2a (R1′, R2 client half): one deadline for each
//! `install-transition` run, inside acsetup's limit, and no identity declaration for Query.

use super::{Command, InstallTransitionRun, client_config, install_transition_run};
use actingcommand_contract::{
    IdentifierIssuer, InstallTransitionAction, InstallTransitionTicket, RuntimeShutdownTarget,
};
use std::path::Path;
use std::time::{Duration, Instant};

/// acsetup gives each actingctl run 75 s (`CONTROL_TIMEOUT`) and never kills it.
const ACSETUP_CONTROL_TIMEOUT: Duration = Duration::from_secs(75);
/// The `--wait` acsetup adds to `commit_shutdown`.
const ACSETUP_COMMIT_WAIT: Duration = Duration::from_secs(60);
const RUN_BUDGET: Duration = Duration::from_secs(60);
const COMMIT_EXCHANGE_BUDGET: Duration = Duration::from_secs(10);

fn actions() -> [(InstallTransitionAction, Duration); 5] {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let ticket = InstallTransitionTicket {
        target: RuntimeShutdownTarget {
            owner_epoch: *issuer.mint_owner_epoch().expect("owner epoch").transport(),
            pid: 4_242,
            started_at_unix_ms: 1_800_000_000_000,
        },
        transition_id: "transition-381a".to_owned(),
        request_id: *issuer.mint_request_id().expect("request id").transport(),
    };
    [
        (
            InstallTransitionAction::Query {
                transition_id: ticket.transition_id.clone(),
            },
            RUN_BUDGET,
        ),
        (
            InstallTransitionAction::Release {
                ticket: ticket.clone(),
                timeout_ms: 60_000,
            },
            RUN_BUDGET,
        ),
        (
            InstallTransitionAction::BeginDrain {
                transition_id: ticket.transition_id.clone(),
                timeout_ms: 60_000,
            },
            RUN_BUDGET,
        ),
        (
            InstallTransitionAction::Abort {
                ticket: ticket.clone(),
            },
            RUN_BUDGET,
        ),
        (
            InstallTransitionAction::CommitShutdown { ticket },
            COMMIT_EXCHANGE_BUDGET,
        ),
    ]
}

/// #672 review M3: the client of each run carries the run's budget as its exchange timeout and
/// one deadline at the run's start plus that budget; other commands carry no deadline.
#[test]
fn gate_the_run_client_carries_one_deadline_from_the_runs_start() {
    // Only an anchor for the expected deadlines; nothing measures elapsed time.
    let started = Instant::now();
    for (action, budget) in actions() {
        let config = client_config(
            Path::new("state"),
            &Command::InstallTransition {
                action: action.clone(),
            },
            started,
        );
        assert_eq!(
            (config.io_timeout(), config.deadline()),
            (budget, Some(started + budget)),
            "{action:?}"
        );
    }
    assert_eq!(
        client_config(Path::new("state"), &Command::Status, started).deadline(),
        None
    );
}

#[test]
fn gate_each_install_transition_run_has_one_deadline_and_query_declares_no_identity() {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let ticket = InstallTransitionTicket {
        target: RuntimeShutdownTarget {
            owner_epoch: *issuer.mint_owner_epoch().expect("owner epoch").transport(),
            pid: 4_242,
            started_at_unix_ms: 1_800_000_000_000,
        },
        transition_id: "transition-381a".to_owned(),
        request_id: *issuer.mint_request_id().expect("request id").transport(),
    };
    let rows = [
        (
            InstallTransitionAction::Query {
                transition_id: ticket.transition_id.clone(),
            },
            RUN_BUDGET,
            false,
        ),
        (
            InstallTransitionAction::Release {
                ticket: ticket.clone(),
                timeout_ms: 60_000,
            },
            RUN_BUDGET,
            true,
        ),
        (
            InstallTransitionAction::BeginDrain {
                transition_id: ticket.transition_id.clone(),
                timeout_ms: 60_000,
            },
            RUN_BUDGET,
            true,
        ),
        (
            InstallTransitionAction::Abort {
                ticket: ticket.clone(),
            },
            RUN_BUDGET,
            true,
        ),
        (
            InstallTransitionAction::CommitShutdown {
                ticket: ticket.clone(),
            },
            COMMIT_EXCHANGE_BUDGET,
            true,
        ),
    ];
    for (action, budget, declares_identity) in rows {
        let run = install_transition_run(&action);
        // One deadline for the whole run; a single exchange may use all of it.
        assert_eq!(
            run,
            InstallTransitionRun {
                budget: Some(budget),
                exchange_timeout: budget,
                declares_identity,
            },
            "{action:?}"
        );
        let wait = if matches!(action, InstallTransitionAction::CommitShutdown { .. }) {
            ACSETUP_COMMIT_WAIT
        } else {
            Duration::ZERO
        };
        let run_budget = run.budget.expect("a run deadline");
        assert!(
            run_budget + wait < ACSETUP_CONTROL_TIMEOUT,
            "{action:?}: {run_budget:?} + {wait:?}"
        );
    }
}
