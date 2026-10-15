// SPDX-License-Identifier: AGPL-3.0-only

// The formal roots of the gate tests, built with public APIs only: the State database, the formal
// SQLite ledger marker and its writer, as acsetup leaves a root (the assembly of the ledger
// crate's `global/tests/fixture.rs`). Events are drafted the way the runtime host drafts them:
// `runtime.instance_bound` with system links and the instance link (runtime-host
// `host/lifecycle.rs`), and the `runtime.fact_*` events with origin `runtime` /
// `runtime-facts` / `runtime`, severity `info`, system links, plus the instance link of an
// instance scope (contracts/runtime-fact-store.md, "Ledger events"). Every event time is a fixed
// constant; nothing here reads a clock.

use actingcommand_contract::{
    AuditInput, EventActor, EventDraft, EventLinksDraft, EventOrigin, EventPayloadDraft,
    EventSeverity, EventSource, ExecutionBackendProvenance, IdentifierIssuer,
    InstanceBindingSource, IssuedInstanceId, OriginModule, RuntimeFactInvalidation,
    RuntimeFactRecord, RuntimeFactScope, RuntimeFactSnapshot, RuntimePayloadDraft,
};
use actingcommand_ledger::{
    GlobalLedger, GlobalLedgerEvidenceConfig, LedgerMaintenance, PersistedEvent,
    Sha256SecretFingerprinter,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use actingcommand_runtime_state::RuntimeStateStore;
use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

// The State integrity key, the writer's owner id and the fingerprint salt: neutral fixture words.
const STATE_KEY: &[u8] = b"actingledger-gate-fixture";
const OWNER_ID: &str = "actingledger-gate-fixture";
const SALT: &[u8] = b"actingledger-gate-salt";
// The fixtures' event times are whole seconds after this fixed instant.
const BASE_UNIX_MS: u64 = 1_700_000_000_000;

// The fixed event time `seconds` after the fixtures' base instant.
pub const fn at(seconds: u64) -> u64 {
    BASE_UNIX_MS + seconds * 1_000
}

// A formal root with its writer open. `close` closes the writer and the State database, as an
// offline reader finds a root.
pub struct OpenRoot {
    ledger: GlobalLedger,
    database: Arc<RuntimeDatabase>,
    ids: IdentifierIssuer,
    fingerprinter: Sha256SecretFingerprinter,
}

impl OpenRoot {
    // State tables, the formal ledger marker and the writer in `root`.
    pub fn create(root: &Path) -> Self {
        let database =
            Arc::new(RuntimeStateStore::open_database(root, STATE_KEY).expect("State database"));
        let limits = MaintenanceLimits::default();
        let maintenance = LedgerMaintenance::acquire(
            root,
            true,
            limits,
            limits.deadline().expect("maintenance deadline"),
        )
        .expect("maintenance owner");
        maintenance
            .initialize_empty(&database)
            .expect("formal ledger");
        let ledger = maintenance
            .open_writer(Arc::clone(&database), OWNER_ID.into())
            .expect("formal writer");
        Self {
            ledger,
            database,
            ids: IdentifierIssuer::new().expect("identifier issuer"),
            fingerprinter: Sha256SecretFingerprinter::new(SALT).expect("fingerprinter"),
        }
    }

    // The root's writer.
    pub fn ledger(&self) -> &GlobalLedger {
        &self.ledger
    }

    // The root's identifier issuer.
    pub fn ids(&self) -> &IdentifierIssuer {
        &self.ids
    }

    // The root's sanitizer.
    pub fn fingerprinter(&self) -> &Sha256SecretFingerprinter {
        &self.fingerprinter
    }

    // A new instance id, issued as an event link.
    pub fn instance(&self) -> IssuedInstanceId {
        self.ids.mint_instance_id().expect("instance id")
    }

    // One sanitized event of severity `info` at `at_unix_ms`, appended by the writer.
    pub fn append(
        &self,
        at_unix_ms: u64,
        origin: EventOrigin,
        links: EventLinksDraft,
        payload: EventPayloadDraft,
    ) -> PersistedEvent {
        let draft = EventDraft::new(
            self.ids.mint_event_id().expect("event id"),
            at_unix_ms,
            EventSeverity::Info,
            origin,
            links,
            payload,
        )
        .sanitize(&self.fingerprinter)
        .expect("a sanitized event");
        self.ledger.append(draft).expect("append the event")
    }

    // `runtime.instance_bound` of `instance` at `127.0.0.1:<port>`, as the host records each
    // registered instance at its start.
    pub fn bind(
        &self,
        at_unix_ms: u64,
        instance: IssuedInstanceId,
        alias: &str,
        port: u16,
    ) -> PersistedEvent {
        self.append(
            at_unix_ms,
            runtime_origin(OriginModule::Runtime),
            self.system_links().with_instance_id(instance),
            RuntimePayloadDraft::instance_bound(
                alias,
                ExecutionBackendProvenance::PhysicalDevice,
                Some("127.0.0.1".to_owned()),
                Some(port),
                false,
                InstanceBindingSource::Explicit,
                None,
                None,
                None,
                None,
                AuditInput::new(),
            )
            .into(),
        )
    }

    // `runtime.fact_recorded` of `record`, appended when it was observed.
    pub fn record_fact(&self, record: &RuntimeFactRecord) -> PersistedEvent {
        self.append(
            record.observed_at_unix_ms,
            runtime_origin(OriginModule::RuntimeFacts),
            self.scope_links(&record.scope),
            RuntimePayloadDraft::fact_recorded(record.clone(), AuditInput::new()).into(),
        )
    }

    // `runtime.fact_invalidated` of `invalidation`, appended when the key was dropped.
    pub fn invalidate_fact(&self, invalidation: &RuntimeFactInvalidation) -> PersistedEvent {
        self.append(
            invalidation.at_unix_ms,
            runtime_origin(OriginModule::RuntimeFacts),
            self.scope_links(&invalidation.scope),
            RuntimePayloadDraft::fact_invalidated(invalidation.clone(), AuditInput::new()).into(),
        )
    }

    // `runtime.fact_snapshot` of one sealed part, appended when it was taken.
    pub fn seal_facts(&self, part: RuntimeFactSnapshot) -> PersistedEvent {
        self.append(
            part.taken_at_unix_ms,
            runtime_origin(OriginModule::RuntimeFacts),
            self.system_links(),
            RuntimePayloadDraft::fact_snapshot(part, AuditInput::new()).into(),
        )
    }

    // Closes the writer, then the State database.
    pub fn close(self) {
        let Self {
            ledger, database, ..
        } = self;
        ledger.close().expect("close the writer");
        drop(database);
    }

    // The host's system links: a request, a correlation and an action.
    fn system_links(&self) -> EventLinksDraft {
        EventLinksDraft::default()
            .with_request_id(self.ids.mint_request_id().expect("request id"))
            .with_correlation_id(self.ids.mint_correlation_id().expect("correlation id"))
            .with_action_id(self.ids.mint_action_id().expect("action id"))
    }

    // System links, plus the instance link of an instance scope.
    fn scope_links(&self, scope: &RuntimeFactScope) -> EventLinksDraft {
        let links = self.system_links();
        match scope {
            RuntimeFactScope::Runtime => links,
            RuntimeFactScope::Instance { instance_id } => {
                links.with_instance_id(self.ids.issue_registered_instance(*instance_id))
            }
        }
    }
}

// The origin of an event the Runtime itself records from `module`.
fn runtime_origin(module: OriginModule) -> EventOrigin {
    EventOrigin::new(EventSource::Runtime, module, EventActor::Runtime)
}

// One in-process `actingledger --state-root <root> <command...>`: its result and what it printed.
pub fn cli(root: &Path, command: &[&str]) -> (Result<(), actingledger::CliError>, Vec<u8>) {
    let mut args = vec![OsString::from("--state-root"), root.as_os_str().to_owned()];
    args.extend(command.iter().copied().map(OsString::from));
    let mut output = Vec::new();
    let result = actingledger::run(args, &mut output);
    (result, output)
}

// The one JSON report the command printed.
pub fn report(output: &[u8]) -> serde_json::Value {
    serde_json::from_slice(output).unwrap_or_else(|error| {
        panic!(
            "one JSON report ({error}): {}",
            String::from_utf8_lossy(output)
        )
    })
}

// The latest sequence of a closed root's ledger.
pub fn latest_sequence(root: &Path) -> u64 {
    GlobalLedger::open_metadata(GlobalLedgerEvidenceConfig::new(root))
        .expect("ledger metadata")
        .latest_sequence()
}
