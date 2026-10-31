// SPDX-License-Identifier: AGPL-3.0-only

//! A hub with one instance `E1`, its fake N and S backends open and one lease, on a virtual
//! clock (no real clock, no sleep).

#![allow(dead_code)]

use actingcommand_capture_bench::backend::{BackendFactory, OpenOptions};
use actingcommand_capture_bench::clock::{Clock, VirtualClock};
use actingcommand_capture_bench::fake::{FakeCheckScript, FakeDevice, FakeScript};
use actingcommand_capture_bench::hub::{BudgetConfig, Hub, MemorySink};
use actingcommand_capture_bench::locks::LockOwner;
use actingcommand_capture_bench::model::BackendKind;
use std::sync::Arc;

pub struct Fixture {
    pub clock: Arc<VirtualClock>,
    pub hub: Arc<Hub>,
    pub sink: Arc<MemorySink>,
    pub device: Arc<FakeDevice>,
    pub owner: Arc<LockOwner>,
    pub lease: u64,
    participant: u64,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.participant != 0 {
            self.clock.retire(self.participant);
        }
    }
}

/// A zero-latency fake: nothing in a step ever waits on the clock.
pub fn instant_script() -> FakeScript {
    FakeScript {
        nemu_latency_us: 0,
        scrcpy_latency_us: 0,
        open_latency_us: 0,
        ..FakeScript::default()
    }
}

/// `participant`: the test thread joins the virtual clock, so its own sleeps move time.
pub fn fixture(script: FakeScript, participant: bool) -> Fixture {
    let clock = Arc::new(VirtualClock::new(1_800_000_000_000));
    let dyn_clock: Arc<dyn Clock> = clock.clone();
    let id = if participant {
        let id = clock.register();
        clock.adopt(id);
        id
    } else {
        0
    };
    let sink = Arc::new(MemorySink::default());
    let hub = Arc::new(Hub::new(
        Arc::clone(&dyn_clock),
        &["E1".to_owned()],
        7,
        BudgetConfig::default(),
        sink.clone(),
    ));
    let device = FakeDevice::new(dyn_clock, script, FakeCheckScript::default());
    for kind in [BackendKind::Nemu, BackendKind::Scrcpy] {
        let backend = device
            .open("E1", kind, &OpenOptions::default())
            .expect("the fake backend opens");
        hub.backends[0].lock().expect("the slots").put(backend);
    }
    let owner = LockOwner::new("test");
    let lease = hub.subscribe(&owner, 0, 1, 3_000_000).lease_id;
    Fixture {
        clock,
        hub,
        sink,
        device,
        owner,
        lease,
        participant: id,
    }
}
