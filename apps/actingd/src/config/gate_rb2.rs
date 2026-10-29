// SPDX-License-Identifier: AGPL-3.0-only

// RB-2 (Alice's ruling of 2026-10-29 10:2x, 「设回正常」; brief rd-1029/brief-rb2.md;
// `contracts/runtime-config.md`, "The `host` section"): the start sets its own memory priority
// and I/O priority to `host.process_memory_priority` and `host.process_io_priority` (default
// `normal`, `low` allowed, any other value refused naming its key), reads both back and records
// one outcome; a failed call is a warning and the start goes on; the CPU priority class is never
// touched. The operating system is a fake that starts as a below-normal scheduled task leaves a
// process: memory priority 2, I/O priority Low (1), CPU class below normal.

use super::*;
use crate::process_priority::{self, ProcessPriorityApi};
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{Category, CauseRelation, RawSource, Relation};
use actingcommand_contract::{ConfigParameter, ConfigParameterSource, FactScalar};
use actingcommand_host_metrics::HostMetricsLocation;
use actingcommand_runtime_config::host::{PROCESS_IO_PRIORITY, PROCESS_MEMORY_PRIORITY};
use actingcommand_runtime_config::{HostProcessConfig, ProcessPriorityLevel};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};

/// `BELOW_NORMAL_PRIORITY_CLASS`, the CPU priority class of a process a below-normal scheduled
/// task starts.
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x4000;

/// An actingd configuration without instances, with this `host` section when one is given.
fn configuration(host: Option<Value>) -> ActingdConfigFile {
    let mut config = json!({
        "schema_version": CONFIG_SCHEMA_VERSION,
        "state_root": "state",
        "bind_host": "127.0.0.1",
        "secret_fingerprint_salt": "0123456789abcdef",
        "instances": []
    });
    if let Some(host) = host {
        config["host"] = host;
    }
    let mut config = serde_json::from_value::<ActingdConfigFile>(config).expect("typed config");
    config.source_root = PathBuf::from("fixture-root");
    config
}

/// The manifest row of `key`.
fn row<'a>(assembly: &'a RuntimeAssembly, key: &str) -> &'a ConfigParameter {
    assembly
        .manifest
        .parameters
        .iter()
        .find(|parameter| parameter.key == key)
        .unwrap_or_else(|| panic!("the manifest reports {key}"))
}

fn levels(memory: ProcessPriorityLevel, io: ProcessPriorityLevel) -> HostProcessConfig {
    HostProcessConfig {
        memory_priority: memory,
        io_priority: io,
    }
}

#[test]
fn rb2_the_host_keys_default_to_normal_take_low_and_refuse_any_other_value_by_name() {
    use ProcessPriorityLevel::{Low, Normal};
    // A file that does not name the section: both `normal`, from the defaults file, and no
    // instruction may change them.
    let assembly = configuration(None).assemble().expect("runtime assembly");
    assert_eq!(assembly.process_priority, levels(Normal, Normal));
    for key in [PROCESS_MEMORY_PRIORITY, PROCESS_IO_PRIORITY] {
        let row = row(&assembly, key);
        assert_eq!(row.value, FactScalar::String("normal".to_owned()), "{key}");
        assert_eq!(row.source, ConfigParameterSource::Default, "{key}");
        assert!(!row.settable, "{key} is applied before the ledger opens");
    }

    // `low` is taken for either key; a key the file does not name stays `normal`.
    for (section, expected) in [
        (
            json!({ "process_memory_priority": "low" }),
            levels(Low, Normal),
        ),
        (json!({ "process_io_priority": "low" }), levels(Normal, Low)),
        (
            json!({ "process_memory_priority": "low", "process_io_priority": "low" }),
            levels(Low, Low),
        ),
        (
            json!({ "process_memory_priority": "normal", "process_io_priority": "normal" }),
            levels(Normal, Normal),
        ),
    ] {
        let assembly = configuration(Some(section.clone()))
            .assemble()
            .unwrap_or_else(|code| panic!("{section} is taken: {code:?}"));
        assert_eq!(assembly.process_priority, expected, "{section}");
        for (key, member) in [
            (PROCESS_MEMORY_PRIORITY, "process_memory_priority"),
            (PROCESS_IO_PRIORITY, "process_io_priority"),
        ] {
            let row = row(&assembly, key);
            match section.get(member) {
                Some(named) => {
                    assert_eq!(
                        row.value,
                        FactScalar::String(named.as_str().expect("a word").to_owned()),
                        "{section} {key}"
                    );
                    assert_eq!(
                        row.source,
                        ConfigParameterSource::Explicit,
                        "{section} {key}"
                    );
                }
                None => {
                    assert_eq!(
                        row.value,
                        FactScalar::String("normal".to_owned()),
                        "{section} {key}"
                    );
                    assert_eq!(
                        row.source,
                        ConfigParameterSource::Default,
                        "{section} {key}"
                    );
                }
            }
        }
    }

    // Anything else refuses the start (and check-config, which assembles the same way) as
    // `config_value_invalid` naming the key: another word, another spelling or an empty word is
    // `out_of_range`, a value that is no string `wrong_type`.
    for (key, member) in [
        (PROCESS_MEMORY_PRIORITY, "process_memory_priority"),
        (PROCESS_IO_PRIORITY, "process_io_priority"),
    ] {
        for (value, problem) in [
            (json!("high"), "out_of_range"),
            (json!("below_normal"), "out_of_range"),
            (json!("Normal"), "out_of_range"),
            (json!(""), "out_of_range"),
            (json!(5), "wrong_type"),
            (json!(true), "wrong_type"),
            (json!(["low"]), "wrong_type"),
        ] {
            let section = json!({ member: value });
            let Err((code, outcome)) = configuration(Some(section.clone())).assemble_or_refusal()
            else {
                panic!("{section} is refused");
            };
            assert_eq!(
                code,
                actingcommand_runtime_config::ConfigCode::ConfigValueInvalid.code(),
                "{section}"
            );
            assert_eq!(outcome.value("config_key"), Some(&json!(key)), "{section}");
            assert_eq!(
                outcome.value("config_value_problem"),
                Some(&json!(problem)),
                "{section}"
            );
        }
    }
}

/// One call the start made on the fake system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    SetMemory(u32),
    ReadMemory,
    SetIo(u32),
    ReadIo,
}

/// The operating system as the start sees it: the process's memory and I/O priority (`None`
/// on a system without them), its CPU priority class, which no call of the seam can reach, and
/// the calls that fail or that the system takes without changing anything.
struct FakeSystem {
    calls: RefCell<Vec<Call>>,
    memory: Cell<Option<u32>>,
    io: Cell<Option<u32>>,
    cpu_class: Cell<u32>,
    refuse_set_memory: Option<Outcome>,
    refuse_read_memory: Option<Outcome>,
    refuse_set_io: Option<Outcome>,
    refuse_read_io: Option<Outcome>,
    keep_levels: bool,
}

impl FakeSystem {
    /// A process a below-normal scheduled task started.
    fn inherited() -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            memory: Cell::new(Some(2)),
            io: Cell::new(Some(1)),
            cpu_class: Cell::new(BELOW_NORMAL_PRIORITY_CLASS),
            refuse_set_memory: None,
            refuse_read_memory: None,
            refuse_set_io: None,
            refuse_read_io: None,
            keep_levels: false,
        }
    }

    /// A system without these priorities: nothing to set, nothing to read.
    fn without_priorities() -> Self {
        let system = Self::inherited();
        system.memory.set(None);
        system.io.set(None);
        system
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.borrow().clone()
    }

    fn set(
        &self,
        call: Call,
        refusal: &Option<Outcome>,
        level: &Cell<Option<u32>>,
        to: u32,
    ) -> Result<(), Outcome> {
        self.calls.borrow_mut().push(call);
        if let Some(refusal) = refusal {
            return Err(refusal.clone());
        }
        if !self.keep_levels && level.get().is_some() {
            level.set(Some(to));
        }
        Ok(())
    }

    fn read(
        &self,
        call: Call,
        refusal: &Option<Outcome>,
        level: &Cell<Option<u32>>,
    ) -> Result<Option<u32>, Outcome> {
        self.calls.borrow_mut().push(call);
        match refusal {
            Some(refusal) => Err(refusal.clone()),
            None => Ok(level.get()),
        }
    }
}

impl ProcessPriorityApi for FakeSystem {
    fn set_memory_priority(&self, level: u32) -> Result<(), Outcome> {
        self.set(
            Call::SetMemory(level),
            &self.refuse_set_memory,
            &self.memory,
            level,
        )
    }

    fn memory_priority(&self) -> Result<Option<u32>, Outcome> {
        self.read(Call::ReadMemory, &self.refuse_read_memory, &self.memory)
    }

    fn set_io_priority(&self, level: u32) -> Result<(), Outcome> {
        self.set(Call::SetIo(level), &self.refuse_set_io, &self.io, level)
    }

    fn io_priority(&self) -> Result<Option<u32>, Outcome> {
        self.read(Call::ReadIo, &self.refuse_read_io, &self.io)
    }
}

/// The system's refusal of one call, as the operating-system boundary reports it.
fn refusal(operation: HostMetricsLocation) -> Outcome {
    Outcome::new(ContractCode::ForeignOsError)
        .raw_source(RawSource::Os)
        .raw_code("0xC0000022")
        .os_error(5u32)
        .raw_text("Access is denied. (os error 5)")
        .operation(operation)
}

/// The record's four values: memory asked and read, I/O asked and read.
fn record(outcome: &Outcome) -> [Option<&Value>; 4] {
    [
        outcome.value("memory_priority_requested"),
        outcome.value("memory_priority_read"),
        outcome.value("io_priority_requested"),
        outcome.value("io_priority_read"),
    ]
}

/// The operations of the record's `caused_by` links, each a `foreign_os_error`.
fn failed_operations(outcome: &Outcome) -> Vec<Value> {
    outcome
        .causes()
        .iter()
        .map(|link| {
            assert!(link.relation() == &Relation::from(CauseRelation::CausedBy));
            assert!(link.outcome().is(ContractCode::ForeignOsError));
            link.outcome()
                .value("operation")
                .cloned()
                .expect("operation")
        })
        .collect()
}

#[test]
fn rb2_the_start_applies_the_configured_levels_reads_them_back_and_leaves_the_cpu_class() {
    for (section, memory, io) in [
        (None, 5u32, 2u32),
        (
            Some(json!({ "process_memory_priority": "low", "process_io_priority": "low" })),
            2,
            1,
        ),
        (Some(json!({ "process_io_priority": "low" })), 5, 1),
    ] {
        let assembly = configuration(section.clone())
            .assemble()
            .expect("runtime assembly");
        let system = FakeSystem::inherited();
        let outcome = process_priority::apply(assembly.process_priority, &system);
        // Exactly these calls, in this order: memory set and read back, then I/O set and read
        // back. Nothing else is called, and the CPU class is as the task started the process.
        assert_eq!(
            system.calls(),
            vec![
                Call::SetMemory(memory),
                Call::ReadMemory,
                Call::SetIo(io),
                Call::ReadIo
            ],
            "{section:?}"
        );
        assert_eq!(
            (system.memory.get(), system.io.get()),
            (Some(memory), Some(io)),
            "{section:?}"
        );
        assert_eq!(
            system.cpu_class.get(),
            BELOW_NORMAL_PRIORITY_CLASS,
            "{section:?}"
        );
        // The one record: what was asked for and what was read back.
        assert!(
            outcome.is(ActingdCode::HostProcessPriorityApplied),
            "{section:?}: {outcome:?}"
        );
        assert_eq!(outcome.category(), Category::Info, "{section:?}");
        assert_eq!(
            record(&outcome),
            [
                Some(&json!(memory)),
                Some(&json!(memory)),
                Some(&json!(io)),
                Some(&json!(io))
            ],
            "{section:?}"
        );
        assert!(outcome.causes().is_empty(), "{section:?}");
    }

    // A system without these priorities (not Windows): nothing changes, and the record has the
    // same code with what was asked for and nothing read.
    let system = FakeSystem::without_priorities();
    let outcome = process_priority::apply(
        levels(ProcessPriorityLevel::Normal, ProcessPriorityLevel::Normal),
        &system,
    );
    assert!(
        outcome.is(ActingdCode::HostProcessPriorityApplied),
        "{outcome:?}"
    );
    assert_eq!(
        record(&outcome),
        [Some(&json!(5)), None, Some(&json!(2)), None]
    );
    assert_eq!(system.cpu_class.get(), BELOW_NORMAL_PRIORITY_CLASS);
}

#[test]
fn rb2_a_failing_call_is_a_warning_with_its_failure_and_the_start_goes_on() {
    let normal = levels(ProcessPriorityLevel::Normal, ProcessPriorityLevel::Normal);

    // The I/O set is refused: the memory priority is still set, the I/O priority still read
    // back (still Low), and the record is the warning with the refusal as its link.
    let system = FakeSystem {
        refuse_set_io: Some(refusal(HostMetricsLocation::ProcessIoPrioritySet)),
        ..FakeSystem::inherited()
    };
    let outcome = process_priority::apply(normal, &system);
    assert_eq!(
        system.calls(),
        vec![
            Call::SetMemory(5),
            Call::ReadMemory,
            Call::SetIo(2),
            Call::ReadIo
        ]
    );
    assert!(
        outcome.is(ActingdCode::HostProcessPriorityFailed),
        "{outcome:?}"
    );
    assert_eq!(outcome.category(), Category::Warning);
    assert_eq!(
        record(&outcome),
        [
            Some(&json!(5)),
            Some(&json!(5)),
            Some(&json!(2)),
            Some(&json!(1))
        ]
    );
    assert_eq!(
        failed_operations(&outcome),
        vec![json!("process_io_priority_set")]
    );
    assert_eq!(
        outcome.causes()[0].outcome().value("raw_code"),
        Some(&json!("0xC0000022"))
    );

    // The memory set and its read are both refused: the I/O priority is still applied, the
    // record has no memory level read, and both refusals are links in call order.
    let system = FakeSystem {
        refuse_set_memory: Some(refusal(HostMetricsLocation::ProcessMemoryPrioritySet)),
        refuse_read_memory: Some(refusal(HostMetricsLocation::ProcessMemoryPriorityRead)),
        ..FakeSystem::inherited()
    };
    let outcome = process_priority::apply(normal, &system);
    assert_eq!(
        system.calls(),
        vec![
            Call::SetMemory(5),
            Call::ReadMemory,
            Call::SetIo(2),
            Call::ReadIo
        ]
    );
    assert_eq!(system.io.get(), Some(2));
    assert!(
        outcome.is(ActingdCode::HostProcessPriorityFailed),
        "{outcome:?}"
    );
    assert_eq!(outcome.category(), Category::Warning);
    assert_eq!(
        record(&outcome),
        [Some(&json!(5)), None, Some(&json!(2)), Some(&json!(2))]
    );
    assert_eq!(
        failed_operations(&outcome),
        vec![
            json!("process_memory_priority_set"),
            json!("process_memory_priority_read")
        ]
    );

    // The system takes both calls but keeps its levels: no call failed, yet the levels read
    // back are not the ones asked for, so the record is the warning, with no link.
    let system = FakeSystem {
        keep_levels: true,
        ..FakeSystem::inherited()
    };
    let outcome = process_priority::apply(normal, &system);
    assert!(
        outcome.is(ActingdCode::HostProcessPriorityFailed),
        "{outcome:?}"
    );
    assert_eq!(outcome.category(), Category::Warning);
    assert_eq!(
        record(&outcome),
        [
            Some(&json!(5)),
            Some(&json!(2)),
            Some(&json!(2)),
            Some(&json!(1))
        ]
    );
    assert!(outcome.causes().is_empty());

    // A warning ends nothing: its exit category is that of a start that goes on, and the CPU
    // class was never touched by any of the three.
    assert_eq!(outcome.category().exit_code(), 0);
    assert_eq!(system.cpu_class.get(), BELOW_NORMAL_PRIORITY_CLASS);
}
