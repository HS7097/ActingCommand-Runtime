// SPDX-License-Identifier: AGPL-3.0-only

//! #503 NOISE-A (0.12.102; register §1.552): the process sampler counts the processes the Runtime
//! starts and the images the host hands over as owned; every other process stays third-party at
//! the same thresholds; a sample lists at most 32 owned processes, the Runtime first, and an owned
//! process left out of the list is never third-party. Pure: the record's ownership, the rule for
//! the Runtime's children and the split, on made-up processes and neutral image names; nothing is
//! enumerated, no clock is read.

use super::*;

const RUNTIME: u32 = 100;
/// The Runtime's creation time (Windows `FILETIME`).
const RUNTIME_CREATED: u64 = 133_000_000_000_000_000;
/// At or above the default process CPU threshold (4 000 basis points).
const HEAVY_CPU: u16 = 5_000;

fn thresholds() -> ProcessLoadThresholds {
    ProcessLoadThresholds {
        cpu_basis_points: 4_000,
        io_bytes_per_second: 64 * 1024 * 1024,
        min_coverage_basis_points: 9_900,
    }
}

fn sampled(
    pid: u32,
    owner: ProcessOwnership,
    cpu_basis_points: u16,
    created: u64,
) -> ProcessSample {
    ProcessSample {
        pid,
        process_name: format!("process-{pid}"),
        ownership: owner,
        cpu_basis_points,
        working_set_bytes: 1,
        peak_working_set_bytes: Some(1),
        process_created_at_windows_100ns: Some(created),
        io_bytes_per_second: 0,
    }
}

/// The handed-over images of the tests: a debug bridge at its path, a device process of any one
/// version under its versions' directory, and a virtual machine by its name.
fn handed_over() -> OwnedImageSet {
    OwnedImageSet::default()
        .with_path("C:/Fixture/tools/bridge.exe")
        .with_open_level_path(r"C:\Fixture\versions", r"shell\device.exe")
        .with_name("Fixture-VM.exe")
}

/// The record's ownership of `pid` named `name` at `path`, with the Runtime's registry alone.
fn classified(
    pid: u32,
    images: &OwnedImageSet,
    name: &str,
    path: Option<&str>,
) -> ProcessOwnership {
    let registry = BTreeMap::from([(RUNTIME, "actingcommand-runtime".to_owned())]);
    record_ownership(pid, RUNTIME, &registry, images, name, || {
        path.map(str::to_owned)
    })
}

#[test]
fn noise_a_an_owned_image_is_a_handed_over_path_one_open_level_or_a_name() {
    let images = handed_over();
    assert!(!images.is_empty());
    // A path, ignoring ASCII case, `/` and `\` alike, with or without `\\?\`.
    assert!(images.owns("BRIDGE.EXE", || Some(
        r"\\?\c:\FIXTURE\Tools\bridge.exe".to_owned()
    )));
    // The same file name elsewhere, or a path that cannot be read, is not owned.
    assert!(!images.owns("bridge.exe", || Some(
        r"C:\Other\tools\bridge.exe".to_owned()
    )));
    assert!(!images.owns("bridge.exe", || None));
    // One open directory level: any version, but exactly one level.
    assert!(images.owns("device.exe", || Some(
        r"C:\Fixture\versions\17.0\shell\device.exe".to_owned()
    )));
    assert!(images.owns("device.exe", || Some(
        r"C:\Fixture\versions\18.2\shell\device.exe".to_owned()
    )));
    assert!(!images.owns("device.exe", || Some(
        r"C:\Fixture\versions\shell\device.exe".to_owned()
    )));
    assert!(!images.owns("device.exe", || Some(
        r"C:\Fixture\versions\17.0\extra\shell\device.exe".to_owned()
    )));
    assert!(!images.owns("device.exe", || Some(
        r"C:\Fixture\other\17.0\shell\device.exe".to_owned()
    )));
    // A name decides alone, and a name no image has reads no path.
    assert!(images.owns("fixture-vm.EXE", || panic!("an owned name reads no path")));
    assert!(!images.owns("unrelated.exe", || panic!(
        "a name no image has reads no path"
    )));
    // The empty set owns nothing.
    let empty = OwnedImageSet::default();
    assert!(empty.is_empty());
    assert!(!empty.owns("bridge.exe", || Some(
        r"C:\Fixture\tools\bridge.exe".to_owned()
    )));
    assert!(!empty.owns("fixture-vm.exe", || None));
    // The handle: empty before the host puts a set; every clone reads the set put last.
    let handle = OwnedImages::default();
    let sampler_side = handle.clone();
    assert_eq!(sampler_side.current(), OwnedImageSet::default());
    handle.replace(images.clone());
    assert_eq!(sampler_side.current(), images);
}

#[test]
fn noise_a_a_child_an_image_and_an_unrelated_heavy_process_only_the_third_is_third_party() {
    let images = handed_over();
    let mut processes = BTreeMap::new();
    processes.insert(
        RUNTIME,
        sampled(RUNTIME, ProcessOwnership::Runtime, 100, RUNTIME_CREATED),
    );
    // A child of the Runtime: its record gives third-party, the rule for children owns it.
    let child = classified(201, &images, "worker.exe", None);
    assert_eq!(child, ProcessOwnership::ThirdParty);
    processes.insert(201, sampled(201, child, HEAVY_CPU, RUNTIME_CREATED + 10));
    // A process at a handed-over image path.
    let at_image = classified(
        301,
        &images,
        "bridge.exe",
        Some(r"C:\Fixture\tools\bridge.exe"),
    );
    assert_eq!(at_image, ProcessOwnership::Owned);
    processes.insert(301, sampled(301, at_image, HEAVY_CPU, RUNTIME_CREATED - 10));
    // The same file name at an unrelated path.
    let unrelated = classified(401, &images, "bridge.exe", Some(r"C:\Unrelated\bridge.exe"));
    assert_eq!(unrelated, ProcessOwnership::ThirdParty);
    processes.insert(
        401,
        sampled(401, unrelated, HEAVY_CPU, RUNTIME_CREATED + 20),
    );
    // A process created before the Runtime whose parent id the Runtime took over is no child.
    processes.insert(
        501,
        sampled(501, ProcessOwnership::ThirdParty, 0, RUNTIME_CREATED - 20),
    );
    let parents = ProcessParents::from([(201, RUNTIME), (301, 7), (401, 7), (501, RUNTIME)]);

    own_runtime_children(RUNTIME, &mut processes, &parents);

    assert_eq!(processes[&201].ownership, ProcessOwnership::Owned);
    assert_eq!(processes[&501].ownership, ProcessOwnership::ThirdParty);
    let ProcessSplit {
        owned,
        third_party_high_load,
    } = split_processes(&processes, thresholds(), 5);
    assert_eq!(
        owned.iter().map(|process| process.pid).collect::<Vec<_>>(),
        [RUNTIME, 201, 301]
    );
    assert_eq!(
        third_party_high_load
            .iter()
            .map(|process| (process.pid, process.ownership))
            .collect::<Vec<_>>(),
        [(401, ProcessOwnership::ThirdParty)]
    );
}

#[test]
fn noise_a_more_than_32_owned_processes_list_32_the_runtime_first_and_none_is_third_party() {
    let mut processes = BTreeMap::new();
    // The Runtime idle, 39 owned processes each above the threshold, one third-party one.
    processes.insert(
        RUNTIME,
        sampled(RUNTIME, ProcessOwnership::Runtime, 0, RUNTIME_CREATED),
    );
    for index in 0..39_u16 {
        let pid = 1_000 + u32::from(index);
        processes.insert(
            pid,
            sampled(
                pid,
                ProcessOwnership::Owned,
                HEAVY_CPU + index % 4,
                RUNTIME_CREATED,
            ),
        );
    }
    processes.insert(
        9_000,
        sampled(
            9_000,
            ProcessOwnership::ThirdParty,
            HEAVY_CPU,
            RUNTIME_CREATED,
        ),
    );

    let ProcessSplit {
        owned,
        third_party_high_load,
    } = split_processes(&processes, thresholds(), 5);

    assert_eq!(owned.len(), MAX_OWNED_PROCESSES);
    assert_eq!(owned[0].pid, RUNTIME);
    // Then by CPU descending and pid ascending.
    assert!(owned[1..].windows(2).all(|pair| {
        pair[0].cpu_basis_points > pair[1].cpu_basis_points
            || (pair[0].cpu_basis_points == pair[1].cpu_basis_points && pair[0].pid < pair[1].pid)
    }));
    assert!(
        owned
            .iter()
            .all(|process| process.ownership != ProcessOwnership::ThirdParty)
    );
    // The owned processes left out of the list are not third-party either.
    assert_eq!(
        third_party_high_load
            .iter()
            .map(|process| process.pid)
            .collect::<Vec<_>>(),
        [9_000]
    );
}

#[test]
fn noise_a_an_empty_handle_owns_only_the_runtime_and_its_children() {
    let empty = OwnedImages::default().current();
    assert_eq!(
        classified(RUNTIME, &empty, "actingd.exe", None),
        ProcessOwnership::Runtime
    );
    assert_eq!(
        classified(
            301,
            &empty,
            "bridge.exe",
            Some(r"C:\Fixture\tools\bridge.exe")
        ),
        ProcessOwnership::ThirdParty
    );
    assert_eq!(
        classified(302, &empty, "fixture-vm.exe", None),
        ProcessOwnership::ThirdParty
    );
    let mut processes = BTreeMap::from([
        (
            RUNTIME,
            sampled(RUNTIME, ProcessOwnership::Runtime, 0, RUNTIME_CREATED),
        ),
        (
            201,
            sampled(201, ProcessOwnership::ThirdParty, 0, RUNTIME_CREATED + 1),
        ),
    ]);
    own_runtime_children(
        RUNTIME,
        &mut processes,
        &ProcessParents::from([(201, RUNTIME)]),
    );
    assert_eq!(processes[&201].ownership, ProcessOwnership::Owned);
}
