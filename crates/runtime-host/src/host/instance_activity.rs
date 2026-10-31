// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #429 R-0b (register §1.238, since 0.12.36; contracts/runtime-interface.md,
//! "Workbench request kinds", the row `RuntimeInstanceStatus.activity?`): what each instance does
//! now. An activity registers itself here when it begins and leaves when its guard drops, so it
//! is gone however it ends: a success, a failure, a cancel, an interruption (an error returned
//! early, a panic's unwinding) or the shutdown that ends its thread. Memory only: the table holds
//! no ledger handle, and building the status member reads nothing but this table. A leaf lock:
//! nothing else is taken while it is held.
//!
//! When an instance has more than one activity at once (a ladder's rung run inside its ladder, an
//! emulator control's start inside a ladder's restart rung), the status shows one of them: the
//! activity that took the instance's key itself ([`KeyHold::Own`]) before one that runs on
//! another activity's key ([`KeyHold::Borrowed`]), and among those the one that began last.

use super::lock;
use crate::RuntimeHostResult;
use crate::codes::HostLocation;
use actingcommand_contract::{InstanceActivity, InstanceActivityKind, InstanceId, RequestId};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

/// Whether an activity took the instance's key (its lease or its claim) itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KeyHold {
    /// It took the key: a policy run (its dispatch's lease), a direct run, a startup package, a
    /// reconnect, a self-check, an emulator control on its own claim, a ladder.
    Own,
    /// It runs on the key another activity holds: a ladder's rung run, a ladder's emulator
    /// restart.
    Borrowed,
}

/// An activity that begins, with the members its kind carries. A policy run and a direct run
/// always name their request and their package (R-0b), so the type demands both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ActivityStart {
    /// A run the scheduler dispatched: the request of the run itself, the key `contained_runs`
    /// tracks it under.
    PolicyRun {
        request_id: RequestId,
        package_id: String,
    },
    /// A run a client started (`RunContainedTask`): the client's request.
    DirectRun {
        request_id: RequestId,
        package_id: String,
    },
    /// A run the host started for itself: a startup package on its claim, or a ladder rung's
    /// package on the ladder's key.
    HostRun {
        request_id: RequestId,
        package_id: String,
        key: KeyHold,
    },
    /// An instance resume's reconnect: the resume's request.
    Reconnect { request_id: RequestId },
    /// A self-check: a person's `SelfCheckInstance` (its request), or the daemon start's
    /// preparation of the instance (no request).
    Selfcheck { request_id: Option<RequestId> },
    /// An emulator start, stop or restart: the request of the claim it runs on.
    EmulatorControl { request_id: RequestId, key: KeyHold },
    /// A recovery ladder: the request of its claim.
    Ladder { request_id: RequestId },
}

impl ActivityStart {
    fn into_activity(self, since_unix_ms: u64) -> (InstanceActivity, KeyHold) {
        let (kind, request_id, package_id, key) = match self {
            Self::PolicyRun {
                request_id,
                package_id,
            } => (
                InstanceActivityKind::PolicyRun,
                Some(request_id),
                Some(package_id),
                KeyHold::Own,
            ),
            Self::DirectRun {
                request_id,
                package_id,
            } => (
                InstanceActivityKind::DirectRun,
                Some(request_id),
                Some(package_id),
                KeyHold::Own,
            ),
            Self::HostRun {
                request_id,
                package_id,
                key,
            } => (
                InstanceActivityKind::HostRun,
                Some(request_id),
                Some(package_id),
                key,
            ),
            Self::Reconnect { request_id } => (
                InstanceActivityKind::Reconnect,
                Some(request_id),
                None,
                KeyHold::Own,
            ),
            Self::Selfcheck { request_id } => (
                InstanceActivityKind::Selfcheck,
                request_id,
                None,
                KeyHold::Own,
            ),
            Self::EmulatorControl { request_id, key } => (
                InstanceActivityKind::EmulatorControl,
                Some(request_id),
                None,
                key,
            ),
            Self::Ladder { request_id } => (
                InstanceActivityKind::Ladder,
                Some(request_id),
                None,
                KeyHold::Own,
            ),
        };
        (
            InstanceActivity {
                kind,
                request_id,
                package_id,
                since_unix_ms,
            },
            key,
        )
    }
}

struct ActivityEntry {
    /// The guard's identity: the entry leaves with the guard that holds the same allocation.
    token: Arc<()>,
    key: KeyHold,
    activity: InstanceActivity,
}

/// The activities of every instance, each instance's in the order they began.
#[derive(Default)]
pub(super) struct InstanceActivities {
    table: Mutex<BTreeMap<InstanceId, Vec<ActivityEntry>>>,
}

impl InstanceActivities {
    /// Registers `start` on `instance_id`, begun at `since_unix_ms` (the host's clock). The
    /// activity shows until the returned guard drops.
    pub(super) fn begin(
        &self,
        instance_id: InstanceId,
        start: ActivityStart,
        since_unix_ms: u64,
    ) -> RuntimeHostResult<InstanceActivityGuard<'_>> {
        let (activity, key) = start.into_activity(since_unix_ms);
        let token = Arc::new(());
        lock(&self.table, HostLocation::BeginInstanceActivity)?
            .entry(instance_id)
            .or_default()
            .push(ActivityEntry {
                token: Arc::clone(&token),
                key,
                activity,
            });
        Ok(InstanceActivityGuard {
            activities: self,
            instance_id,
            token,
        })
    }

    /// The activity each instance shows now, by instance; an instance doing none of them has no
    /// entry. Reads only this table.
    pub(super) fn shown(&self) -> RuntimeHostResult<BTreeMap<InstanceId, InstanceActivity>> {
        Ok(
            lock(&self.table, HostLocation::ProjectRuntimeControlPlaneStatus)?
                .iter()
                .filter_map(|(instance_id, entries)| {
                    shown_of(entries).map(|activity| (*instance_id, activity.clone()))
                })
                .collect(),
        )
    }

    /// Every activity of `instance_id` now, in the order they began, each with whether it took
    /// the key itself.
    #[cfg(test)]
    pub(super) fn registered_for_test(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<Vec<(InstanceActivityKind, bool)>> {
        Ok(
            lock(&self.table, HostLocation::ProjectRuntimeControlPlaneStatus)?
                .get(&instance_id)
                .map(|entries| {
                    entries
                        .iter()
                        .map(|entry| (entry.activity.kind, entry.key == KeyHold::Own))
                        .collect()
                })
                .unwrap_or_default(),
        )
    }

    fn end(&self, instance_id: InstanceId, token: &Arc<()>) {
        // A poisoned table still loses the entry; the next status read reports the poison.
        let mut table = self.table.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(entries) = table.get_mut(&instance_id) {
            entries.retain(|entry| !Arc::ptr_eq(&entry.token, token));
            if entries.is_empty() {
                table.remove(&instance_id);
            }
        }
    }
}

impl super::HostShared {
    /// Begins `start` on `instance_id`, its `since_unix_ms` the host clock's reading now (the
    /// clock seam, a fake one in tests).
    pub(super) fn begin_instance_activity(
        &self,
        instance_id: InstanceId,
        start: ActivityStart,
    ) -> RuntimeHostResult<InstanceActivityGuard<'_>> {
        let since_unix_ms = self.clock.sample()?.unix_ms;
        self.instance_activities
            .begin(instance_id, start, since_unix_ms)
    }

    /// The activity each instance shows now (`RuntimeInstanceStatus.activity`), from memory only.
    pub(super) fn instance_activities_shown(
        &self,
    ) -> RuntimeHostResult<BTreeMap<InstanceId, InstanceActivity>> {
        self.instance_activities.shown()
    }
}

/// The precedence rule: of `entries` (in the order they began), the last one that took the key
/// itself; when none did, the last one.
fn shown_of(entries: &[ActivityEntry]) -> Option<&InstanceActivity> {
    entries
        .iter()
        .rev()
        .find(|entry| entry.key == KeyHold::Own)
        .or_else(|| entries.last())
        .map(|entry| &entry.activity)
}

/// One registered activity; dropping it ends the activity.
#[must_use = "the activity ends when its guard drops"]
pub(super) struct InstanceActivityGuard<'a> {
    activities: &'a InstanceActivities,
    instance_id: InstanceId,
    token: Arc<()>,
}

impl Drop for InstanceActivityGuard<'_> {
    fn drop(&mut self) {
        self.activities.end(self.instance_id, &self.token);
    }
}

#[cfg(test)]
mod tests {
    // Workflow #429 R-0b (register §1.238): the table on its own, with the clock's readings
    // given: each kind's members, the end of every guard, the member built from the table alone
    // (no ledger), and the overlap rule. The host's wiring is `tests::gate_429_r0b`'s.

    use super::*;
    use actingcommand_contract::IdentifierIssuer;

    struct Ids(IdentifierIssuer);

    impl Ids {
        fn new() -> Self {
            Self(IdentifierIssuer::new().expect("identifier issuer"))
        }

        fn instance(&self) -> InstanceId {
            *self.0.mint_instance_id().expect("instance id").transport()
        }

        fn request(&self) -> RequestId {
            *self.0.mint_request_id().expect("request id").transport()
        }
    }

    fn shown(table: &InstanceActivities, instance_id: InstanceId) -> Option<InstanceActivity> {
        table.shown().expect("the table").remove(&instance_id)
    }

    #[test]
    fn r0b_each_kind_carries_its_members_and_policy_and_direct_runs_their_request_and_package() {
        let ids = Ids::new();
        let table = InstanceActivities::default();
        let instance = ids.instance();
        let request = ids.request();
        let cases = [
            (
                ActivityStart::PolicyRun {
                    request_id: request,
                    package_id: "package.example".to_owned(),
                },
                InstanceActivityKind::PolicyRun,
                Some(request),
                Some("package.example"),
            ),
            (
                ActivityStart::DirectRun {
                    request_id: request,
                    package_id: "package.example".to_owned(),
                },
                InstanceActivityKind::DirectRun,
                Some(request),
                Some("package.example"),
            ),
            (
                ActivityStart::HostRun {
                    request_id: request,
                    package_id: "package.startup".to_owned(),
                    key: KeyHold::Own,
                },
                InstanceActivityKind::HostRun,
                Some(request),
                Some("package.startup"),
            ),
            (
                ActivityStart::Reconnect {
                    request_id: request,
                },
                InstanceActivityKind::Reconnect,
                Some(request),
                None,
            ),
            (
                ActivityStart::Selfcheck {
                    request_id: Some(request),
                },
                InstanceActivityKind::Selfcheck,
                Some(request),
                None,
            ),
            (
                ActivityStart::Selfcheck { request_id: None },
                InstanceActivityKind::Selfcheck,
                None,
                None,
            ),
            (
                ActivityStart::EmulatorControl {
                    request_id: request,
                    key: KeyHold::Own,
                },
                InstanceActivityKind::EmulatorControl,
                Some(request),
                None,
            ),
            (
                ActivityStart::Ladder {
                    request_id: request,
                },
                InstanceActivityKind::Ladder,
                Some(request),
                None,
            ),
        ];
        for (index, (start, kind, request_id, package_id)) in cases.into_iter().enumerate() {
            let since_unix_ms = 1_000 + u64::try_from(index).expect("index");
            assert_eq!(shown(&table, instance), None, "{kind:?} before");
            let guard = table
                .begin(instance, start, since_unix_ms)
                .expect("an activity begins");
            assert_eq!(
                shown(&table, instance),
                Some(InstanceActivity {
                    kind,
                    request_id,
                    package_id: package_id.map(str::to_owned),
                    since_unix_ms,
                }),
                "{kind:?} while it runs"
            );
            drop(guard);
            assert_eq!(shown(&table, instance), None, "{kind:?} after");
            assert!(table.shown().expect("the table").is_empty());
        }
    }

    /// An activity's function: it begins `start`, sees it shown, and returns `end` (its guard
    /// drops on the way out, with the value or with the error).
    fn activity_returning(
        table: &InstanceActivities,
        instance: InstanceId,
        start: ActivityStart,
        end: Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        let _activity = table.begin(instance, start, 1).expect("begins");
        assert!(shown(table, instance).is_some(), "shown while it runs");
        end
    }

    #[test]
    fn r0b_every_end_clears_the_activity() {
        let ids = Ids::new();
        let table = InstanceActivities::default();
        let instance = ids.instance();
        let run = |request_id| ActivityStart::DirectRun {
            request_id,
            package_id: "package.example".to_owned(),
        };
        // A success, a failure and a cancel return from the activity's function.
        for (end, name) in [
            (Ok(()), "a success"),
            (Err("failed"), "a failure"),
            (Err("cancelled"), "a cancel"),
        ] {
            assert_eq!(
                activity_returning(&table, instance, run(ids.request()), end),
                end
            );
            assert_eq!(shown(&table, instance), None, "after {name}");
        }
        // An interruption: the activity's thread unwinds.
        let interrupted = run(ids.request());
        std::thread::scope(|scope| {
            let unwound = scope
                .spawn(|| {
                    let _activity = table.begin(instance, interrupted, 3).expect("begins");
                    assert!(shown(&table, instance).is_some());
                    panic!("interrupted");
                })
                .join();
            assert!(unwound.is_err());
        });
        assert_eq!(shown(&table, instance), None, "after an interruption");
        // A shutdown: the threads that hold activities end, each dropping its guard.
        let ladder = table
            .begin(
                instance,
                ActivityStart::Ladder {
                    request_id: ids.request(),
                },
                4,
            )
            .expect("begins");
        let rung = table
            .begin(
                instance,
                ActivityStart::HostRun {
                    request_id: ids.request(),
                    package_id: "package.home".to_owned(),
                    key: KeyHold::Borrowed,
                },
                5,
            )
            .expect("begins");
        std::thread::scope(|scope| {
            scope.spawn(move || drop(rung));
            scope.spawn(move || drop(ladder));
        });
        assert!(
            table.shown().expect("the table").is_empty(),
            "after a shutdown"
        );
    }

    // Test 4 of the work order: building `activity` reads no ledger. The table is built and read
    // here with no host and no ledger in existence; its read takes nothing but the table; and the
    // table's only member is its map, so the destructuring below stops compiling the day a ledger
    // handle (or anything else) joins it.
    #[test]
    fn r0b_the_status_member_is_built_from_the_table_alone_without_any_ledger() {
        let ids = Ids::new();
        let table = InstanceActivities::default();
        let InstanceActivities { table: _ } = &table;
        let instances = [ids.instance(), ids.instance()];
        let request = ids.request();
        let _first = table
            .begin(
                instances[0],
                ActivityStart::PolicyRun {
                    request_id: request,
                    package_id: "package.example".to_owned(),
                },
                11,
            )
            .expect("begins");
        let _second = table
            .begin(
                instances[1],
                ActivityStart::Selfcheck { request_id: None },
                12,
            )
            .expect("begins");
        let shown = table.shown().expect("the table alone");
        assert_eq!(shown.len(), 2);
        assert_eq!(
            shown.get(&instances[0]).map(|activity| activity.kind),
            Some(InstanceActivityKind::PolicyRun)
        );
        assert_eq!(
            shown.get(&instances[1]).map(|activity| activity.kind),
            Some(InstanceActivityKind::Selfcheck)
        );
    }

    #[test]
    fn r0b_the_key_holder_shows_then_the_activity_that_began_last() {
        let ids = Ids::new();
        let table = InstanceActivities::default();
        let instance = ids.instance();
        let other = ids.instance();
        let ladder_request = ids.request();
        let rung_request = ids.request();
        let control_request = ids.request();

        // A ladder on its key, then its rung's run on the ladder's key: the ladder shows.
        let ladder = table
            .begin(
                instance,
                ActivityStart::Ladder {
                    request_id: ladder_request,
                },
                10,
            )
            .expect("begins");
        let rung = table
            .begin(
                instance,
                ActivityStart::HostRun {
                    request_id: rung_request,
                    package_id: "package.home".to_owned(),
                    key: KeyHold::Borrowed,
                },
                20,
            )
            .expect("begins");
        let ladder_shown = shown(&table, instance).expect("an activity");
        assert_eq!(ladder_shown.kind, InstanceActivityKind::Ladder);
        assert_eq!(ladder_shown.request_id, Some(ladder_request));
        assert_eq!(ladder_shown.since_unix_ms, 10);
        // The ladder's restart rung after it: the ladder still shows.
        let restart = table
            .begin(
                instance,
                ActivityStart::EmulatorControl {
                    request_id: ladder_request,
                    key: KeyHold::Borrowed,
                },
                30,
            )
            .expect("begins");
        assert_eq!(
            shown(&table, instance).map(|activity| activity.kind),
            Some(InstanceActivityKind::Ladder)
        );
        // Another instance is not affected.
        assert_eq!(shown(&table, other), None);
        // The ladder ends before its borrowers: of these two, which hold no key, the one that
        // began last shows.
        drop(ladder);
        let last = shown(&table, instance).expect("an activity");
        assert_eq!(last.kind, InstanceActivityKind::EmulatorControl);
        assert_eq!(last.since_unix_ms, 30);
        drop(restart);
        assert_eq!(
            shown(&table, instance).map(|activity| activity.kind),
            Some(InstanceActivityKind::HostRun)
        );
        drop(rung);
        assert_eq!(shown(&table, instance), None);

        // Two that took the key (an emulator control, then its startup package's run after the
        // hand-over): the one that began last shows, whatever its clock reading says.
        let control = table
            .begin(
                instance,
                ActivityStart::EmulatorControl {
                    request_id: control_request,
                    key: KeyHold::Own,
                },
                50,
            )
            .expect("begins");
        let startup = table
            .begin(
                instance,
                ActivityStart::HostRun {
                    request_id: rung_request,
                    package_id: "package.startup".to_owned(),
                    key: KeyHold::Own,
                },
                40,
            )
            .expect("begins");
        assert_eq!(
            shown(&table, instance).map(|activity| activity.kind),
            Some(InstanceActivityKind::HostRun)
        );
        drop(startup);
        assert_eq!(
            shown(&table, instance).map(|activity| activity.kind),
            Some(InstanceActivityKind::EmulatorControl)
        );
        drop(control);
        assert!(table.shown().expect("the table").is_empty());
    }
}
