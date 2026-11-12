// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #437 MAPP-1 (register §1.323, §1.328; `contracts/device-group.md`, "The configuration
// form", "Undeclared sharing", "A group of one is today", "Derived, never stored"): an entry's
// `device_group`, the group's rules at assembly (check-config assembles the same way), the
// refusal of every group of several until the slice "order within one device", the refusal of
// undeclared sharing at assembly and after the start's discovery, and the member → device key map.
// The load is tested with a group of one, with each refusal, and with the refusal of a group of
// several that passes every rule (register §1.323, "The code slices"). The ids, names and paths
// are made up; the discovery is scripted.

use super::provider_startup::bind_discovered_instances_of_groups;
use super::*;
use crate::codes::DeviceGroupProblem;
use actingcommand_contract::{
    ConfigParameterSource, EventPayload, FactScalar, IdentifierIssuer, ProviderStartupObservation,
    ProviderStartupStage,
};
use actingcommand_device::{
    DiscoveredMumuInstance, MumuDiscoveryReport, MumuManagerSource, MumuManagerVersion,
};
use actingcommand_ledger::{GlobalLedger, GlobalLedgerEvidenceConfig, PersistedEvent};
use actingcommand_runtime_host::{RuntimeHost, RuntimeHostResult};
use serde_json::{Value, json};
use tempfile::TempDir;

const FIRST: &str = "neutral.a";
const SECOND: &str = "neutral.b";
const GROUP: &str = "device.one";
/// The defaults file's ladder profile without `emulator_restart` (MAPP-1).
const PROFILE: &str = "without_emulator_restart";

fn instance_id() -> InstanceId {
    *IdentifierIssuer::new()
        .expect("issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport()
}

/// One emulator entry bound by its port, of its own application, as a member of a group of
/// several must be written: `device_group`, a startup package, `idle.action` `none` and the
/// profile without `emulator_restart`.
fn member(root: &TempDir, alias: &str, application: &str) -> Value {
    json!({
        "alias": alias, "instance_id": instance_id(),
        "application_id": application, "adb_path": root.path().join("missing-adb.exe"),
        "connect": false, "host": "127.0.0.1", "port": 16384,
        "touch_backend": "adb_shell_input", "capture_backend": "adb",
        "startup_package": {"package": root.path().join(format!("{alias}-startup.zip"))},
        "idle": {"action": "none"},
        "recovery_ladder_profile": PROFILE,
        "device_group": GROUP,
    })
}

/// [`member`] bound by discovery to the emulator of index 2 instead of its port.
fn discovered_member(root: &TempDir, alias: &str, application: &str) -> Value {
    let mut entry = member(root, alias, application);
    let fields = entry.as_object_mut().expect("an entry");
    for endpoint in ["adb_path", "connect", "host", "port"] {
        fields.remove(endpoint);
    }
    fields.insert("instance_index".to_owned(), json!(2));
    entry
}

/// An actingd configuration of these entries.
fn configuration(root: &TempDir, instances: Vec<Value>) -> ActingdConfigFile {
    serde_json::from_value::<ActingdConfigFile>(json!({
        "schema_version": CONFIG_SCHEMA_VERSION,
        "state_root": root.path().join("state"),
        "bind_host": "127.0.0.1",
        "bind_port": 0,
        "secret_fingerprint_salt": "0123456789abcdef",
        "instances": instances,
    }))
    .expect("typed config")
}

/// The refusal of these entries' assembly.
fn refusal(root: &TempDir, instances: Vec<Value>) -> (Code, Outcome) {
    let Err(refused) = configuration(root, instances).assemble_or_refusal() else {
        panic!("the configuration is refused");
    };
    refused
}

/// `outcome` is `device_group_config_invalid` with exactly these values.
fn assert_group_outcome(
    outcome: &Outcome,
    aliases: &[&str],
    config_key: &str,
    problem: DeviceGroupProblem,
) {
    assert!(
        outcome.is(ActingdCode::DeviceGroupConfigInvalid),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.values(),
        json!({
            "aliases": aliases,
            "config_key": config_key,
            "device_group_problem": problem.as_str(),
        })
        .as_object()
        .expect("values"),
        "{outcome:?}"
    );
}

/// The assembly refused with `device_group_config_invalid` and exactly these values.
fn assert_group_refusal(
    (code, outcome): &(Code, Outcome),
    aliases: &[&str],
    config_key: &str,
    problem: DeviceGroupProblem,
) {
    assert_eq!(
        *code,
        ActingdCode::DeviceGroupConfigInvalid.code(),
        "{outcome:?}"
    );
    assert_group_outcome(outcome, aliases, config_key, problem);
}

// Register §1.323, MAPP-1's tests: "a group of one writes what today writes" (the configuration
// half; the scheduler's and the evaluator's halves are their crates' `gate_mapp1`). The same entry
// with and without a lone `device_group` assembles the same: the manifest differs by the one row
// `instance.<id>.device_group` (the file's, not settable), the device key is the entry's own id,
// no group of several is assembled, and check-config's report names the group on the entry's row.
#[test]
fn mapp1_a_lone_device_group_assembles_as_the_entry_without_it() {
    let root = TempDir::new().expect("tempdir");
    let id = instance_id();
    let entry = |group: Option<&str>| {
        let mut entry = json!({
            "alias": FIRST, "instance_id": id,
            "application_id": "com.example.fixture.a",
            "adb_path": root.path().join("missing-adb.exe"),
            "connect": false, "host": "127.0.0.1", "port": 16384,
            "touch_backend": "adb_shell_input", "capture_backend": "adb",
        });
        if let Some(group) = group {
            entry["device_group"] = json!(group);
        }
        entry
    };
    let without = configuration(&root, vec![entry(None)])
        .assemble()
        .unwrap_or_else(|code| panic!("assembles: {code:?}"));
    let with = configuration(&root, vec![entry(Some(GROUP))])
        .assemble()
        .unwrap_or_else(|code| panic!("a group of one assembles: {code:?}"));
    let key = format!(
        "{}.{}.{}",
        actingcommand_runtime_config::INSTANCE_SECTION,
        manifest::instance_id_text(id).expect("the id's text"),
        actingcommand_runtime_config::DEVICE_GROUP
    );
    let row = with
        .manifest
        .parameters
        .iter()
        .find(|parameter| parameter.key == key)
        .expect("the entry's group row");
    assert_eq!(row.value, FactScalar::String(GROUP.to_owned()));
    assert_eq!(row.source, ConfigParameterSource::Explicit);
    assert!(!row.settable);
    assert!(
        !without
            .manifest
            .parameters
            .iter()
            .any(|parameter| parameter.key == key),
        "an entry without the member has no row"
    );
    let others = with
        .manifest
        .parameters
        .iter()
        .filter(|parameter| parameter.key != key)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(others, without.manifest.parameters);
    assert_eq!(with.manifest.subsystems, without.manifest.subsystems);
    for assembly in [&with, &without] {
        assert_eq!(
            assembly.device_groups.device_keys(),
            BTreeMap::from([(id, id)])
        );
        assert!(assembly.device_groups.several().is_empty());
    }
    assert_eq!(
        with.device_groups.names(),
        BTreeMap::from([(FIRST.to_owned(), GROUP.to_owned())])
    );
    assert!(without.device_groups.names().is_empty());
}

// Register §1.323, MAPP-1's tests: "the entry still denies unknown members" (with the declared
// range [5, 29], pinned by the architecture test of the interface declarations, this is what makes
// a build before revision 29 refuse the new form). A name under the alias rules is taken; one that
// breaks them is refused as an alias is.
#[test]
fn mapp1_the_entry_takes_device_group_and_still_denies_unknown_members() {
    let root = TempDir::new().expect("tempdir");
    let mut unknown = member(&root, FIRST, "com.example.fixture.a");
    unknown["device_groups"] = json!(GROUP);
    assert!(
        serde_json::from_value::<ActingdConfigFile>(json!({
            "schema_version": CONFIG_SCHEMA_VERSION,
            "state_root": root.path().join("state"),
            "bind_host": "127.0.0.1",
            "secret_fingerprint_salt": "0123456789abcdef",
            "instances": [unknown],
        }))
        .is_err(),
        "an unknown member of the entry is refused"
    );
    for name in [String::new(), "x".repeat(65)] {
        let mut entry = member(&root, FIRST, "com.example.fixture.a");
        entry["device_group"] = json!(name);
        let (code, _) = refusal(&root, vec![entry]);
        assert_eq!(code, SharedCode::InvalidInstanceAlias.code(), "{name:?}");
    }
}

// Register §1.323, MAPP-1's tests: "each group rule refused with its device_group_problem", and "a
// group of several that breaks no rule refused at load with several_members_unsupported". Each
// row breaks one rule of the form's table on the second member of a group that otherwise keeps
// them all.
#[test]
fn mapp1_each_group_rule_is_refused_with_its_problem_and_a_valid_group_of_several_too() {
    let root = TempDir::new().expect("tempdir");
    let first = || member(&root, FIRST, "com.example.fixture.a");
    let second = || member(&root, SECOND, "com.example.fixture.b");
    let changed = |change: &dyn Fn(&mut Value)| {
        let mut entry = second();
        change(&mut entry);
        entry
    };
    let fixture = |alias: &str| {
        json!({
            "alias": alias, "instance_id": instance_id(), "device_group": GROUP,
            "fixture_backend": {"frames": [{"width": 1, "height": 1, "rgb": [0, 0, 0]}], "max_inputs": 1},
        })
    };
    let rows: Vec<(&str, Vec<Value>, Vec<&str>, &str, DeviceGroupProblem)> = vec![
        (
            "another port",
            vec![first(), changed(&|entry| entry["port"] = json!(16416))],
            vec![FIRST, SECOND],
            "port",
            DeviceGroupProblem::DeviceMismatch,
        ),
        (
            "another key",
            vec![
                discovered_member(&root, FIRST, "com.example.fixture.a"),
                changed(&|entry| {
                    let fields = entry.as_object_mut().expect("an entry");
                    for endpoint in ["adb_path", "connect", "host", "port"] {
                        fields.remove(endpoint);
                    }
                    fields.insert("instance_name".to_owned(), json!("neutral-b"));
                }),
            ],
            vec![FIRST, SECOND],
            "instance_name",
            DeviceGroupProblem::DeviceMismatch,
        ),
        (
            "a handset",
            vec![
                first(),
                changed(&|entry| {
                    let fields = entry.as_object_mut().expect("an entry");
                    for endpoint in ["connect", "host", "port"] {
                        fields.remove(endpoint);
                    }
                    fields.insert("device".to_owned(), json!({"kind": "handset"}));
                }),
            ],
            vec![SECOND],
            "device",
            DeviceGroupProblem::HandsetMember,
        ),
        (
            "fixtures",
            vec![fixture(FIRST), fixture(SECOND)],
            vec![FIRST],
            "fixture_backend",
            DeviceGroupProblem::FixtureMember,
        ),
        (
            "one application",
            vec![
                first(),
                changed(&|entry| entry["application_id"] = json!("com.example.fixture.a")),
            ],
            vec![FIRST, SECOND],
            "application_id",
            DeviceGroupProblem::ApplicationShared,
        ),
        (
            "no startup package",
            vec![
                first(),
                changed(&|entry| {
                    entry
                        .as_object_mut()
                        .expect("an entry")
                        .remove("startup_package");
                }),
            ],
            vec![SECOND],
            "startup_package",
            DeviceGroupProblem::StartupPackageMissing,
        ),
        (
            "the defaults' idle action (close_app)",
            vec![
                first(),
                changed(&|entry| {
                    entry.as_object_mut().expect("an entry").remove("idle");
                }),
            ],
            vec![SECOND],
            "idle.action",
            DeviceGroupProblem::DeviceActionUnsupported,
        ),
        (
            "the section's rungs (emulator_restart)",
            vec![
                first(),
                changed(&|entry| {
                    entry
                        .as_object_mut()
                        .expect("an entry")
                        .remove("recovery_ladder_profile");
                }),
            ],
            vec![SECOND],
            "recovery_ladder_profile",
            DeviceGroupProblem::DeviceActionUnsupported,
        ),
        (
            "start_emulator",
            vec![discovered_member(&root, FIRST, "com.example.fixture.a"), {
                let mut entry = discovered_member(&root, SECOND, "com.example.fixture.b");
                entry["start_emulator"] = json!(true);
                entry
            }],
            vec![SECOND],
            "start_emulator",
            DeviceGroupProblem::DeviceActionUnsupported,
        ),
        (
            "every rule kept",
            vec![first(), second()],
            vec![FIRST, SECOND],
            "device_group",
            DeviceGroupProblem::SeveralMembersUnsupported,
        ),
        (
            "every rule kept, bound by discovery",
            vec![
                discovered_member(&root, FIRST, "com.example.fixture.a"),
                discovered_member(&root, SECOND, "com.example.fixture.b"),
            ],
            vec![FIRST, SECOND],
            "device_group",
            DeviceGroupProblem::SeveralMembersUnsupported,
        ),
    ];
    for (case, instances, aliases, config_key, problem) in rows {
        let refused = refusal(&root, instances);
        assert_eq!(
            refused.1.value("device_group_problem"),
            Some(&json!(problem.as_str())),
            "{case}: {:?}",
            refused.1
        );
        assert_group_refusal(&refused, &aliases, config_key, problem);
    }
}

// Register §1.323, MAPP-1's tests: "undeclared sharing refused, at assembly". Two entries outside
// one group that name one device by the same key and value are refused, `config_key` the shared
// key; entries of other devices, or of one group, are not.
#[test]
fn mapp1_undeclared_sharing_is_refused_at_assembly() {
    let root = TempDir::new().expect("tempdir");
    let plain = |alias: &str, application: &str, binding: Value| {
        let mut entry = json!({
            "alias": alias, "instance_id": instance_id(), "application_id": application,
            "touch_backend": "adb_shell_input", "capture_backend": "adb",
        });
        let fields = entry.as_object_mut().expect("an entry");
        for (key, value) in binding.as_object().expect("a binding") {
            fields.insert(key.clone(), value.clone());
        }
        if !fields.contains_key("instance_index") && !fields.contains_key("instance_name") {
            fields.insert(
                "adb_path".to_owned(),
                json!(root.path().join("missing-adb.exe")),
            );
            fields.insert("connect".to_owned(), json!(false));
        }
        entry
    };
    let endpoint = json!({"host": "127.0.0.1", "port": 16384});
    for (case, binding, key) in [
        ("one host and port", endpoint.clone(), "port"),
        ("one index", json!({"instance_index": 2}), "instance_index"),
        (
            "one name",
            json!({"instance_name": "neutral-a"}),
            "instance_name",
        ),
    ] {
        let refused = refusal(
            &root,
            vec![
                plain(FIRST, "com.example.fixture.a", binding.clone()),
                plain(SECOND, "com.example.fixture.b", binding),
            ],
        );
        assert_eq!(
            refused.1.value("device_group_problem"),
            Some(&json!(DeviceGroupProblem::UndeclaredSharedDevice.as_str())),
            "{case}"
        );
        assert_group_refusal(
            &refused,
            &[FIRST, SECOND],
            key,
            DeviceGroupProblem::UndeclaredSharedDevice,
        );
    }
    // A member of a group of one beside an entry without one, and two groups of one, on one
    // device: refused the same way.
    let mut grouped = plain(FIRST, "com.example.fixture.a", endpoint.clone());
    grouped["device_group"] = json!(GROUP);
    let mut other_group = plain(SECOND, "com.example.fixture.b", endpoint.clone());
    for second in [plain(SECOND, "com.example.fixture.b", endpoint.clone()), {
        other_group["device_group"] = json!("device.two");
        other_group
    }] {
        assert_group_refusal(
            &refusal(&root, vec![grouped.clone(), second]),
            &[FIRST, SECOND],
            "port",
            DeviceGroupProblem::UndeclaredSharedDevice,
        );
    }
    // Two devices: accepted, as before.
    assert!(
        configuration(
            &root,
            vec![
                plain(FIRST, "com.example.fixture.a", endpoint),
                plain(
                    SECOND,
                    "com.example.fixture.b",
                    json!({"host": "127.0.0.1", "port": 16416})
                ),
            ],
        )
        .assemble()
        .is_ok(),
        "two entries of two devices"
    );
}

/// The discovery answer of one running emulator, index 2, named `neutral-a`, at 127.0.0.1:16384.
fn one_emulator() -> MumuDiscoveryReport {
    let version = MumuManagerVersion::parse("6.5.7.0").expect("a four-part version");
    MumuDiscoveryReport {
        source: MumuManagerSource::ExplicitRoot,
        mumu_manager_path: PathBuf::from("C:/fixture/nx_main/MuMuManager.exe"),
        version,
        registry_display_version: None,
        instances: vec![DiscoveredMumuInstance {
            install_root: PathBuf::from("C:/fixture"),
            mumu_manager_path: PathBuf::from("C:/fixture/nx_main/MuMuManager.exe"),
            adb_path: PathBuf::from("C:/fixture/nx_main/adb.exe"),
            instance_index: 2,
            instance_name: "neutral-a".to_owned(),
            adb_host: Some("127.0.0.1".to_owned()),
            adb_port: Some(16384),
            running: true,
            player_state: Some("start_finished".to_owned()),
            android_version: Some("12".to_owned()),
            mumu_version: version,
        }],
    }
}

/// A start of these entries whose provider startup binds the discovery-bound ones with the scripted
/// [`one_emulator`]: its result and the ledger it left.
fn discovery_start(instances: Vec<Value>) -> (RuntimeHostResult<()>, Vec<PersistedEvent>) {
    let root = TempDir::new().expect("tempdir");
    let state_root = root.path().join("state");
    let assembly = configuration(&root, instances)
        .assemble()
        .unwrap_or_else(|code| panic!("the assembly cannot tell: {code:?}"));
    let ConfiguredProvider {
        instances: explicit,
        deferred,
        device_groups,
        ..
    } = assembly.provider;
    let host = RuntimeHostConfig::new(&state_root, b"0123456789abcdef")
        .with_frame_retention_enabled(false)
        .with_config_manifest(assembly.manifest);
    let mut daemon_log = Vec::new();
    let started = RuntimeHost::start_with_provider(host, |startup| {
        let mut specs = explicit;
        specs.extend(bind_discovered_instances_of_groups(
            startup,
            None,
            || Ok(one_emulator()),
            &mut daemon_log,
            deferred,
            &device_groups,
        )?);
        let registry = ExecutionBackendRegistry::from_assembly(ProviderAssembly {
            instances: specs,
            vision: None,
            discovery: None,
        })?;
        Ok(Arc::new(registry) as Arc<dyn ExecutionBackendProvider>)
    });
    let result = started.map(|host| host.close().expect("close the host"));
    let events =
        GlobalLedger::open_evidence(GlobalLedgerEvidenceConfig::new(&state_root), |_| None)
            .expect("the startup ledger is readable")
            .events()
            .to_vec();
    (result, events)
}

/// The provider startup observations of the ledger, in order.
fn observations(events: &[PersistedEvent]) -> Vec<&ProviderStartupObservation> {
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Provider(payload) => Some(&payload.record.observation),
            _ => None,
        })
        .collect()
}

// Register §1.323, MAPP-1's tests: "undeclared sharing refused ... after a fake discovery". An
// index and a name of one emulator, and an explicit HOST:PORT beside a discovery-bound entry,
// pass the assembly (it cannot tell) and are refused once the start's discovery resolves them to
// one device, recorded as every discovery refusal is: the provider startup failure of the
// discovery stage. The discovery record names the index's first entry by alias.
#[test]
fn mapp1_undeclared_sharing_is_refused_after_the_starts_discovery() {
    let root = TempDir::new().expect("tempdir");
    let deferred = |alias: &str, application: &str, binding: Value| {
        let mut entry = json!({
            "alias": alias, "instance_id": instance_id(), "application_id": application,
            "touch_backend": "adb_shell_input", "capture_backend": "adb",
        });
        let fields = entry.as_object_mut().expect("an entry");
        for (key, value) in binding.as_object().expect("a binding") {
            fields.insert(key.clone(), value.clone());
        }
        entry
    };
    let explicit = json!({
        "alias": FIRST, "instance_id": instance_id(), "application_id": "com.example.fixture.a",
        "adb_path": root.path().join("missing-adb.exe"), "connect": false,
        "host": "127.0.0.1", "port": 16384,
        "touch_backend": "adb_shell_input", "capture_backend": "adb",
    });
    for (case, instances, aliases, key) in [
        (
            "an index and a name of one emulator",
            vec![
                deferred(
                    SECOND,
                    "com.example.fixture.b",
                    json!({"instance_index": 2}),
                ),
                deferred(
                    FIRST,
                    "com.example.fixture.a",
                    json!({"instance_name": "neutral-a"}),
                ),
            ],
            [SECOND, FIRST],
            "instance_index",
        ),
        (
            "an explicit endpoint beside a discovery-bound entry",
            vec![
                explicit.clone(),
                deferred(
                    SECOND,
                    "com.example.fixture.b",
                    json!({"instance_index": 2}),
                ),
            ],
            [FIRST, SECOND],
            "port",
        ),
    ] {
        let (result, events) = discovery_start(instances);
        let error = result.expect_err(case);
        assert_eq!(
            error.code(),
            ActingdCode::DeviceGroupConfigInvalid.as_str(),
            "{case}"
        );
        let observed = observations(&events);
        let Some(ProviderStartupObservation::Failed { stage, failure }) = observed.last() else {
            panic!("{case}: the provider startup ends with its failure: {observed:?}");
        };
        assert_eq!(*stage, ProviderStartupStage::InstanceDiscovery, "{case}");
        assert_group_outcome(
            &failure.outcome,
            &aliases,
            key,
            DeviceGroupProblem::UndeclaredSharedDevice,
        );
        let discovered = observed
            .iter()
            .find_map(|observation| match observation {
                ProviderStartupObservation::InstanceDiscovery { instances, .. } => Some(instances),
                _ => None,
            })
            .expect("the discovery is recorded before the failure");
        let expected = if key == "instance_index" {
            FIRST
        } else {
            SECOND
        };
        assert_eq!(
            discovered[0].bound_alias.as_deref(),
            Some(expected),
            "{case}"
        );
    }
}

// Register §1.323 ("Undeclared sharing"): after the discovery, entries of one group that resolve
// to one device are no undeclared sharing (the group's own rules judged them at assembly), and
// entries of different groups, or one without a group, are.
#[test]
fn mapp1_after_the_discovery_only_entries_outside_one_group_share_undeclared() {
    let root = TempDir::new().expect("tempdir");
    let groups = |first: Option<&str>, second: Option<&str>| {
        let entry = |alias: &str, group: Option<&str>| {
            let mut entry = discovered_member(&root, alias, "com.example.fixture.a");
            match group {
                Some(group) => entry["device_group"] = json!(group),
                None => {
                    entry
                        .as_object_mut()
                        .expect("an entry")
                        .remove("device_group");
                }
            }
            let config = serde_json::from_value::<InstanceConfig>(entry).expect("a typed entry");
            device_group::GroupEntry::of(&config, alias.to_owned(), false)
        };
        DeviceGroups::new(vec![entry(FIRST, first), entry(SECOND, second)])
    };
    let resolved: [(String, u16, Option<(String, u16)>); 2] = [
        (FIRST.to_owned(), 2, Some(("127.0.0.1".to_owned(), 16384))),
        (SECOND.to_owned(), 2, Some(("127.0.0.1".to_owned(), 16384))),
    ];
    assert!(
        provider_startup::undeclared_sharing_after_discovery(
            &resolved,
            &groups(Some(GROUP), Some(GROUP))
        )
        .is_none()
    );
    for (first, second) in [
        (Some(GROUP), Some("device.two")),
        (Some(GROUP), None),
        (None, None),
    ] {
        let (code, failure) =
            provider_startup::undeclared_sharing_after_discovery(&resolved, &groups(first, second))
                .unwrap_or_else(|| panic!("{first:?} beside {second:?} is refused"));
        assert_eq!(code, ActingdCode::DeviceGroupConfigInvalid.code());
        assert_group_outcome(
            &failure.outcome,
            &[FIRST, SECOND],
            "instance_index",
            DeviceGroupProblem::UndeclaredSharedDevice,
        );
    }
}
