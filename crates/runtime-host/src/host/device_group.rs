// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #437 MAPP-1b (`contracts/device-group.md`; register §1.328, "MAPP-1b", and §1.330):
//! the host's half of a device group, the instances that declare one device.
//!
//! - **The members.** The member → device key map of the start's configuration
//!   (`RuntimeHostConfig::with_device_keys`), as the scheduler keys its state: a group's device
//!   key is its smallest member id, and an instance the map does not name is alone on its device.
//!   Every answer here for an instance alone on its device is the one before MAPP-1b.
//! - **The device's holder.** A member's occupancy is read across its group: the lease of the
//!   device (whichever member its token names), a registered policy run of any member, or the
//!   first eligible waiting claim of the device (each judged by its own member's routine gate).
//! - **The waits of a member** for another member (`device_group_waiting`): another member's player
//!   hold (`player_hold`), its fence (`fenced`), and its holder (`held`). A member's own gates and
//!   its own holder keep their own codes.
//! - **The refusals of device-level actions** (`device_group_action_refused`): an operator's
//!   emulator control of a member, and a keyless observation of a member while another member
//!   holds the device.

use super::*;
use crate::codes::{HostCode, HostLocation};
use crate::policy_host::{DeviceGroupHold, InstanceOccupancy};
use actingcommand_policy::DeviceGroupWait;

/// The member a device's holder serves: the member the lease's token names, a member whose
/// policy run is registered, or the member the first eligible waiting claim serves; with the
/// claim kind and, for a policy run, its catalog task.
struct DeviceHolder {
    member: InstanceId,
    kind: ClaimKind,
    task_id: Option<String>,
}

/// A member's device as a policy cycle or an admission reads it: the device's occupancy (held,
/// or in its takeover cooldown) and the member that has it.
pub(super) struct DeviceOccupancy {
    pub(super) occupancy: Option<InstanceOccupancy>,
    holder: Option<DeviceHolder>,
}

/// `device_group_action_refused` (register §1.330): a device-level action `operation` on the
/// member `instance_alias` of a device group of several, refused before any device step.
fn device_group_action_refused(
    instance_alias: &str,
    operation: impl Into<Location>,
) -> RuntimeHostError {
    RuntimeHostError::request(
        HostCode::DeviceGroupActionRefused,
        operation,
        RuntimeErrorCode::InvalidRequest,
    )
    .with_outcome_values(|outcome| outcome.alias(instance_alias))
}

impl HostShared {
    /// The device key of `instance_id`: its group's smallest member id, its own id when it is
    /// alone on its device.
    pub(super) fn device_key(&self, instance_id: InstanceId) -> InstanceId {
        self.device_keys
            .get(&instance_id)
            .copied()
            .unwrap_or(instance_id)
    }

    /// The members of `instance_id`'s device, itself included, in id order: every instance the
    /// map gives its device key, and the key itself (the group's smallest member id).
    pub(super) fn device_members(&self, instance_id: InstanceId) -> Vec<InstanceId> {
        let device = self.device_key(instance_id);
        let mut members = self
            .device_keys
            .iter()
            .filter(|(_, key)| **key == device)
            .map(|(member, _)| *member)
            .collect::<BTreeSet<_>>();
        members.insert(device);
        members.insert(instance_id);
        members.into_iter().collect()
    }

    /// Whether `instance_id` is a member of a device group of several.
    pub(super) fn in_group_of_several(&self, instance_id: InstanceId) -> bool {
        self.device_members(instance_id).len() > 1
    }

    /// The registered aliases of `members`, by id. A member the registry does not hold is not a
    /// member the host runs: it is refused, as an unknown instance is.
    fn member_aliases(
        &self,
        members: &[InstanceId],
    ) -> RuntimeHostResult<BTreeMap<InstanceId, String>> {
        let registered = lock(
            &self.registered_instances,
            HostLocation::ReadInstanceRegistry,
        )?;
        members
            .iter()
            .map(|member| {
                registered
                    .get(member)
                    .map(|instance| (*member, instance.instance_alias.clone()))
                    .ok_or_else(|| {
                        RuntimeHostError::request(
                            RuntimeErrorCode::InstanceUnknown,
                            HostLocation::ReadInstanceRegistry,
                            RuntimeErrorCode::InstanceUnknown,
                        )
                    })
            })
            .collect()
    }

    /// The occupancy of `instance_id`'s device read across its group (`contracts/device-group.md`,
    /// "One queue, one worker, one lease per device"): while any member's run is registered or any
    /// member's claim holds the device or waits for it first, the device is held for every member
    /// (`InstanceOccupancy::Held`, the holder's kind, the device's eligible waiting claims, each
    /// judged by its own member's routine gate); a free device in its takeover cooldown is
    /// `Cooldown`, as an instance's is. The holder names the member it serves.
    pub(super) fn device_occupancy(
        &self,
        instance_id: InstanceId,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<DeviceOccupancy> {
        let members = self.device_members(instance_id);
        let gates = members
            .iter()
            .map(|member| Ok((*member, self.routine_gate(*member)?)))
            .collect::<RuntimeHostResult<BTreeMap<_, _>>>()?;
        let own_gate = gates.get(&instance_id).copied().unwrap_or(ClaimGate::Open);
        let gate_of = |member: InstanceId| gates.get(&member).copied().unwrap_or(own_gate);
        let now_monotonic_ms = self.monotonic_ms()?;
        // Workflow #427-A (C3), per member: a run handed to the device's worker holds the device
        // from its hand-off to its settlement, also once its lease is released.
        let run = self.policy_run_of_members(&members)?;
        let scheduler = lock(&self.scheduler, HostLocation::ReadPolicyInstanceOccupancy)?;
        let count = scheduler.eligible_count(instance_id, &gate_of, now_monotonic_ms);
        // A lapsed holder is no hold: the sweep, or the admission's own expiry, ends it.
        let lease = scheduler
            .active_lease(instance_id)
            .filter(|lease| lease.token().expires_at_monotonic_ms() > now_monotonic_ms);
        let holder = match (lease, &run) {
            (Some(lease), _) => {
                let member = lease.token().instance_id();
                Some(DeviceHolder {
                    member,
                    kind: lease.kind(),
                    task_id: run
                        .as_ref()
                        .filter(|(run_member, _)| *run_member == member)
                        .map(|(_, task_id)| task_id.clone()),
                })
            }
            (None, Some((member, task_id))) => Some(DeviceHolder {
                member: *member,
                kind: ClaimKind::PolicyDispatch,
                task_id: Some(task_id.clone()),
            }),
            (None, None) => scheduler
                .first_eligible_claim(instance_id, &gate_of, now_monotonic_ms)
                .map(|(member, kind)| DeviceHolder {
                    member,
                    kind,
                    task_id: None,
                }),
        };
        if let Some(holder) = holder {
            return Ok(DeviceOccupancy {
                occupancy: Some(InstanceOccupancy::Held {
                    holder_kind: holder.kind,
                    count,
                }),
                holder: Some(holder),
            });
        }
        let remaining_ms = scheduler.cooldown_remaining_ms(instance_id, now_monotonic_ms);
        drop(scheduler);
        let occupancy = match remaining_ms {
            Some(remaining_ms) => Some(InstanceOccupancy::Cooldown {
                until_unix_ms: now_unix_ms.checked_add(remaining_ms).ok_or_else(|| {
                    policy_admission_fatal(
                        HostCode::PolicyAdmissionClockOverflow,
                        HostLocation::ReadPolicyInstanceOccupancy,
                    )
                })?,
            }),
            None => None,
        };
        Ok(DeviceOccupancy {
            occupancy,
            holder: None,
        })
    }

    /// What keeps the member `instance_id` of a device group of several from its device because
    /// of another member (`contracts/device-group.md`, "One pick per device per round", "A
    /// failure stays where it belongs", "Pause and hold"), in this order: another member's player
    /// hold (`player_hold`), another member's fence (`fenced`), then another member's holder of
    /// the device (`held`). `None` for an instance alone on its device, for a member that holds
    /// the device itself (its own holder keeps its own code) and for a free device. The other
    /// members are read in alias order; `player_held` and `fenced` answer by alias, read by the
    /// caller at one moment with the member's own control gates, which come before this.
    pub(super) fn device_group_hold(
        &self,
        instance_id: InstanceId,
        now_unix_ms: u64,
        player_held: &dyn Fn(&str) -> bool,
        fenced: &dyn Fn(&str) -> bool,
    ) -> RuntimeHostResult<Option<DeviceGroupHold>> {
        let members = self.device_members(instance_id);
        if members.len() < 2 {
            return Ok(None);
        }
        let device = self.device_occupancy(instance_id, now_unix_ms)?;
        if device
            .holder
            .as_ref()
            .is_some_and(|holder| holder.member == instance_id)
        {
            return Ok(None);
        }
        let aliases = self.member_aliases(&members)?;
        let mut others = aliases
            .iter()
            .filter(|(member, _)| **member != instance_id)
            .map(|(_, alias)| alias.as_str())
            .collect::<Vec<_>>();
        others.sort_unstable();
        let gate = |wait: DeviceGroupWait, alias: &str| DeviceGroupHold {
            wait,
            alias: alias.to_owned(),
            task_id: None,
            holder_kind: None,
        };
        if let Some(alias) = others.iter().copied().find(|alias| player_held(*alias)) {
            return Ok(Some(gate(DeviceGroupWait::PlayerHold, alias)));
        }
        if let Some(alias) = others.iter().copied().find(|alias| fenced(*alias)) {
            return Ok(Some(gate(DeviceGroupWait::Fenced, alias)));
        }
        let Some(holder) = device.holder else {
            return Ok(None);
        };
        let alias = aliases.get(&holder.member).cloned().ok_or_else(|| {
            RuntimeHostError::request(
                RuntimeErrorCode::InstanceUnknown,
                HostLocation::ReadPolicyInstanceOccupancy,
                RuntimeErrorCode::InstanceUnknown,
            )
        })?;
        Ok(Some(DeviceGroupHold {
            wait: DeviceGroupWait::Held,
            alias,
            task_id: holder.task_id,
            holder_kind: Some(holder.kind),
        }))
    }

    /// Every registered member of a device group of several that another member keeps from its
    /// device as a policy cycle starts, by alias ([`Self::device_group_hold`]), with the player
    /// holds and fences the cycle read at the same step.
    pub(super) fn policy_device_group_holds(
        &self,
        scheduling_pause: &SchedulingPauseTable,
        fenced_aliases: &BTreeSet<String>,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<BTreeMap<String, DeviceGroupHold>> {
        let instances = lock(
            &self.registered_instances,
            HostLocation::ReadPolicyInstanceRegistry,
        )?
        .values()
        .map(|instance| (instance.instance_id(), instance.instance_alias.clone()))
        .collect::<Vec<_>>();
        let player_held = |alias: &str| scheduling_pause.player_holds(alias);
        let fenced = |alias: &str| fenced_aliases.contains(alias);
        let mut holds = BTreeMap::new();
        for (instance_id, instance_alias) in instances {
            if let Some(hold) =
                self.device_group_hold(instance_id, now_unix_ms, &player_held, &fenced)?
            {
                holds.insert(instance_alias, hold);
            }
        }
        Ok(holds)
    }

    /// [`Self::device_group_hold`] as the policy admission reads it now: the other members'
    /// player holds from the pause table and their fences, read here.
    pub(super) fn admission_device_group_hold(
        &self,
        instance_id: InstanceId,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<Option<DeviceGroupHold>> {
        let members = self.device_members(instance_id);
        if members.len() < 2 {
            return Ok(None);
        }
        let aliases = self.member_aliases(&members)?;
        let player_held = {
            let pauses = lock(
                &self.scheduling_pause,
                HostLocation::GatePolicySchedulingPause,
            )?;
            aliases
                .values()
                .filter(|alias| pauses.player_holds(alias.as_str()))
                .cloned()
                .collect::<BTreeSet<_>>()
        };
        let mut fenced = BTreeSet::new();
        for (member, alias) in &aliases {
            if self.resource_fenced(*member)? {
                fenced.insert(alias.clone());
            }
        }
        self.device_group_hold(
            instance_id,
            now_unix_ms,
            &|alias: &str| player_held.contains(alias),
            &|alias: &str| fenced.contains(alias),
        )
    }

    /// `contracts/device-group.md`, "Device-level actions", row 5: an operator's emulator control
    /// (start, stop, restart) of a member of a device group of several is refused before any
    /// device step and before its intent, `device_group_action_refused` (`alias` the member,
    /// `operation` `control_emulator_instance`): it would rebind and refresh that member only and
    /// leave the other members' endpoints and device facts as they were. An instance alone on its
    /// device is not refused here.
    pub(super) fn refuse_emulator_control_of_group(
        &self,
        instance_alias: &str,
    ) -> Result<(), RequestFailure> {
        let instance_id = self.resolve_instance(instance_alias)?.instance_id();
        if !self.in_group_of_several(instance_id) {
            return Ok(());
        }
        Err(RequestFailure::request(
            device_group_action_refused(instance_alias, ContractLocation::ControlEmulatorInstance),
            RuntimeReceiptState::Denied,
            None,
        ))
    }

    /// `contracts/device-group.md`, "Device-level actions", row 15: a keyless read-only
    /// observation of a member of a device group of several, under the device's admission guard
    /// (`_admission`), is refused while another member holds the device (its lease, lapsed or
    /// not, or the device held for the member by another member's run or waiting claim, as its
    /// occupancy reads it), because it would open a second member's session on the device:
    /// `scheduler.denied` and the failure's record under `links`, then a denied receipt,
    /// `device_group_action_refused` (`alias` the member, `operation` `observe_readonly`).
    /// Nothing is captured. A member that holds the device itself, a free device and an instance
    /// alone on its device are not refused here.
    pub(super) fn refuse_keyless_observation_of_group(
        &self,
        instance_alias: &str,
        instance_id: InstanceId,
        links: EventLinksDraft,
        _admission: &MutexGuard<'_, ()>,
    ) -> Result<(), RequestFailure> {
        if !self.in_group_of_several(instance_id) {
            return Ok(());
        }
        let lease_of_another = lock(&self.scheduler, HostLocation::ReadInstanceAdmission)?
            .active_lease(instance_id)
            .is_some_and(|lease| lease.token().instance_id() != instance_id);
        let now_unix_ms = self.runtime_clock_sample()?.unix_ms;
        let held_by_another = lease_of_another
            || self
                .device_occupancy(instance_id, now_unix_ms)?
                .holder
                .is_some_and(|holder| holder.member != instance_id);
        if !held_by_another {
            return Ok(());
        }
        let error = device_group_action_refused(instance_alias, ContractLocation::ObserveReadonly);
        let denied = self
            .append_event_raw(
                EventSeverity::Warning,
                EventSource::Scheduler,
                OriginModule::Scheduler,
                EventActor::Scheduler,
                links.clone(),
                SchedulerPayloadDraft::denied(
                    EventAction::ScheduleAdmit,
                    DiagnosticCode::RuntimeDiagnostic,
                    AuditInput::new(),
                ),
            )
            .map_err(RequestFailure::poison_without_terminal)?;
        self.record_required_failure(&error, &denied, links)
            .map_err(RequestFailure::poison_without_terminal)?;
        Err(RequestFailure::request(
            error,
            RuntimeReceiptState::Denied,
            Some(terminal(&denied)),
        ))
    }
}
