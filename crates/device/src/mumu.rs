// SPDX-License-Identifier: AGPL-3.0-only

use crate::{DeviceError, DeviceResult};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

const NEMU_IPC_DLL_NAME: &str = "external_renderer_ipc.dll";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MumuInstallSource {
    ExplicitFolder,
    RunningProcess,
    VendorEnumeration,
}

impl MumuInstallSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ExplicitFolder => "explicit_folder",
            Self::RunningProcess => "running_process",
            Self::VendorEnumeration => "vendor_enumeration",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MumuInstallation {
    pub(crate) root: PathBuf,
    pub(crate) source: MumuInstallSource,
}

pub(crate) fn resolve_mumu_installation(
    explicit_root: Option<PathBuf>,
) -> DeviceResult<Option<MumuInstallation>> {
    if explicit_root.is_some() {
        return resolve_mumu_installation_from_sources(explicit_root, &[], &[]);
    }
    let running_executables = crate::discovery::running_mumu_executable_paths()?;
    let vendor_parents = known_vendor_parent_dirs();
    resolve_mumu_installation_from_sources(None, &running_executables, &vendor_parents)
}

pub(crate) fn resolve_mumu_installation_from_sources(
    explicit_root: Option<PathBuf>,
    running_executables: &[PathBuf],
    vendor_parents: &[PathBuf],
) -> DeviceResult<Option<MumuInstallation>> {
    if let Some(root) = explicit_root {
        require_install_root(&root, MumuInstallSource::ExplicitFolder)?;
        return Ok(Some(MumuInstallation {
            root,
            source: MumuInstallSource::ExplicitFolder,
        }));
    }

    let mut running_roots = Vec::new();
    for executable in running_executables {
        let root = mumu_root_from_path(executable).ok_or_else(|| {
            DeviceError::fatal(format!(
                "running MuMu executable path does not identify an installation root: {}",
                executable.display()
            ))
        })?;
        running_roots.push(root);
    }
    let running_roots = stable_unique_paths(running_roots);
    if !running_roots.is_empty() {
        return select_unique_installation(running_roots, MumuInstallSource::RunningProcess)
            .map(Some);
    }

    let roots = enumerate_vendor_install_roots(vendor_parents)?;
    if roots.is_empty() {
        return Ok(None);
    }
    select_unique_installation(roots, MumuInstallSource::VendorEnumeration).map(Some)
}

pub(crate) fn resolve_mumu_adb(installation: &MumuInstallation) -> DeviceResult<PathBuf> {
    resolve_existing_candidate(
        installation,
        "ADB executable",
        mumu_adb_candidates(&installation.root)?,
    )
}

pub(crate) fn resolve_mumu_capture_dll(installation: &MumuInstallation) -> DeviceResult<PathBuf> {
    resolve_existing_candidate(
        installation,
        "Nemu capture DLL",
        mumu_capture_dll_candidates(&installation.root)?,
    )
}

pub(crate) fn mumu_adb_candidates(root: &Path) -> DeviceResult<Vec<PathBuf>> {
    let mut candidates = vec![root.join("nx_main").join("adb.exe")];
    candidates.extend(
        mumu_version_dirs(root)?
            .into_iter()
            .map(|version| version.join("shell").join("adb.exe")),
    );
    Ok(candidates)
}

pub(crate) fn mumu_capture_dll_candidates(root: &Path) -> DeviceResult<Vec<PathBuf>> {
    let mut candidates = vec![root.join("shell").join("sdk").join(NEMU_IPC_DLL_NAME)];
    candidates.extend(
        mumu_version_dirs(root)?
            .into_iter()
            .map(|version| version.join("shell").join("sdk").join(NEMU_IPC_DLL_NAME)),
    );
    Ok(candidates)
}

pub(crate) fn mumu_root_from_path(path: &Path) -> Option<PathBuf> {
    let mut root = PathBuf::new();
    for component in path.components() {
        let name = component.as_os_str().to_string_lossy();
        if name.eq_ignore_ascii_case("nx_device") || name.eq_ignore_ascii_case("nx_main") {
            return (!root.as_os_str().is_empty()).then_some(root);
        }
        root.push(component.as_os_str());
    }
    None
}

pub(crate) fn mumu_root_from_capture_dll(path: &Path) -> Option<PathBuf> {
    mumu_root_from_path(path).or_else(|| {
        let sdk = path.parent()?;
        let shell = sdk.parent()?;
        if !path_component_eq(sdk, "sdk") || !path_component_eq(shell, "shell") {
            return None;
        }
        shell.parent().map(Path::to_path_buf)
    })
}

pub(crate) fn same_mumu_install_root(left: &Path, right: &Path) -> bool {
    path_key(left) == path_key(right)
}

pub(crate) fn path_is_within_mumu_root(path: &Path, root: &Path) -> bool {
    let path = path_key(path);
    let root = path_key(root);
    path == root
        || path
            .strip_prefix(&root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn resolve_existing_candidate(
    installation: &MumuInstallation,
    label: &str,
    candidates: Vec<PathBuf>,
) -> DeviceResult<PathBuf> {
    if let Some(path) = candidates.iter().find(|path| path.is_file()) {
        return Ok(path.clone());
    }
    Err(DeviceError::fatal(format!(
        "MuMu {label} discovery selected source={} install_root={} but no candidate file exists; checked: {}",
        installation.source.as_str(),
        installation.root.display(),
        display_paths(&candidates)
    )))
}

fn select_unique_installation(
    roots: Vec<PathBuf>,
    source: MumuInstallSource,
) -> DeviceResult<MumuInstallation> {
    if roots.len() != 1 {
        return Err(DeviceError::fatal(format!(
            "MuMu installation discovery is ambiguous for source={}: {}; configure ACTINGCOMMAND_NEMU_FOLDER, ACTINGCOMMAND_ADB_PATH, or an explicit backend path",
            source.as_str(),
            display_paths(&roots)
        )));
    }
    let root = roots.into_iter().next().expect("one root");
    require_install_root(&root, source)?;
    Ok(MumuInstallation { root, source })
}

fn require_install_root(root: &Path, source: MumuInstallSource) -> DeviceResult<()> {
    if root.is_dir() {
        return Ok(());
    }
    Err(DeviceError::fatal(format!(
        "MuMu installation root from source={} does not exist or is not a directory: {}",
        source.as_str(),
        root.display()
    )))
}

fn enumerate_vendor_install_roots(parents: &[PathBuf]) -> DeviceResult<Vec<PathBuf>> {
    let mut roots = Vec::new();
    for parent in stable_unique_paths(parents.to_vec()) {
        let entries = match std::fs::read_dir(&parent) {
            Ok(entries) => entries,
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(DeviceError::fatal(format!(
                    "failed to enumerate MuMu vendor directory {}: {err}",
                    parent.display()
                )));
            }
        };
        for entry in entries {
            let entry = entry.map_err(|err| {
                DeviceError::fatal(format!(
                    "failed to read MuMu vendor directory entry under {}: {err}",
                    parent.display()
                ))
            })?;
            let file_type = entry.file_type().map_err(|err| {
                DeviceError::fatal(format!(
                    "failed to inspect MuMu vendor candidate {}: {err}",
                    entry.path().display()
                ))
            })?;
            if file_type.is_dir() && is_mumu_install_name(&entry.file_name().to_string_lossy()) {
                roots.push(entry.path());
            }
        }
    }
    Ok(stable_unique_paths(roots))
}

fn known_vendor_parent_dirs() -> Vec<PathBuf> {
    let mut parents = Vec::new();
    for root in ["ProgramFiles", "ProgramFiles(x86)"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
    {
        parents.push(root.clone());
        parents.push(root.join("Netease"));
    }
    stable_unique_paths(parents)
}

fn mumu_version_dirs(root: &Path) -> DeviceResult<Vec<PathBuf>> {
    let nx_device = root.join("nx_device");
    let entries = match std::fs::read_dir(&nx_device) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(DeviceError::fatal(format!(
                "failed to enumerate MuMu version directory {}: {err}",
                nx_device.display()
            )));
        }
    };
    let mut versions = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| {
            DeviceError::fatal(format!(
                "failed to read MuMu version directory entry under {}: {err}",
                nx_device.display()
            ))
        })?;
        let file_type = entry.file_type().map_err(|err| {
            DeviceError::fatal(format!(
                "failed to inspect MuMu version candidate {}: {err}",
                entry.path().display()
            ))
        })?;
        if file_type.is_dir() {
            versions.push(entry.path());
        }
    }
    Ok(stable_unique_paths(versions))
}

fn is_mumu_install_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("mumu player") || lower.starts_with("mumuplayer-")
}

fn stable_unique_paths(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.sort_by_key(|path| path_key(path));
    paths.dedup_by(|left, right| path_key(left) == path_key(right));
    paths
}

fn display_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn path_component_eq(path: &Path, expected: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(expected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(label: &str) -> Self {
            let index = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "actingcommand-mumu-discovery-{label}-{}-{index}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("temp root");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn candidates_are_version_independent_and_stably_sorted() {
        let temp = TempRoot::new("versions");
        let root = temp.path().join("MuMu Player Global");
        let nx_main_adb = root.join("nx_main/adb.exe");
        let older_adb = root.join("nx_device/9.7/shell/adb.exe");
        let newer_adb = root.join("nx_device/13.4/shell/adb.exe");
        let newer_dll = root.join("nx_device/13.4/shell/sdk/external_renderer_ipc.dll");
        for file in [&nx_main_adb, &older_adb, &newer_adb, &newer_dll] {
            fs::create_dir_all(file.parent().expect("parent")).expect("candidate parent");
            fs::write(file, b"fixture").expect("candidate file");
        }

        let adb_candidates = mumu_adb_candidates(&root).expect("ADB candidates");
        let dll_candidates = mumu_capture_dll_candidates(&root).expect("DLL candidates");

        assert_eq!(adb_candidates[0], nx_main_adb);
        assert_eq!(adb_candidates[1], newer_adb);
        assert_eq!(adb_candidates[2], older_adb);
        assert_eq!(dll_candidates[1], newer_dll);
    }

    #[test]
    fn explicit_root_overrides_ambiguous_vendor_installations() {
        let temp = TempRoot::new("explicit");
        let vendor = temp.path().join("vendor");
        let first = vendor.join("MuMu Player Alpha");
        let second = vendor.join("MuMuPlayer-Beta");
        let explicit = temp.path().join("CustomMuMuInstall");
        for root in [&first, &second, &explicit] {
            fs::create_dir_all(root).expect("install root");
        }

        let selected =
            resolve_mumu_installation_from_sources(Some(explicit.clone()), &[], &[vendor])
                .expect("explicit selection")
                .expect("installation");

        assert_eq!(selected.root, explicit);
        assert_eq!(selected.source, MumuInstallSource::ExplicitFolder);
    }

    #[test]
    fn running_process_root_precedes_vendor_enumeration() {
        let temp = TempRoot::new("running");
        let vendor = temp.path().join("vendor");
        let running_root = vendor.join("MuMu Player Running");
        let other_root = vendor.join("MuMuPlayer-Other");
        let executable = running_root.join("nx_device/13.4/shell/MuMuNxDevice.exe");
        fs::create_dir_all(executable.parent().expect("process parent")).expect("process root");
        fs::create_dir_all(&other_root).expect("other root");

        let selected = resolve_mumu_installation_from_sources(None, &[executable], &[vendor])
            .expect("running selection")
            .expect("installation");

        assert_eq!(selected.root, running_root);
        assert_eq!(selected.source, MumuInstallSource::RunningProcess);
    }

    #[test]
    fn multiple_vendor_installations_fail_loudly() {
        let temp = TempRoot::new("ambiguous");
        let vendor = temp.path().join("vendor");
        let first = vendor.join("MuMu Player Alpha");
        let second = vendor.join("MuMuPlayer-Beta");
        fs::create_dir_all(&first).expect("first root");
        fs::create_dir_all(&second).expect("second root");

        let err = resolve_mumu_installation_from_sources(None, &[], &[vendor])
            .expect_err("ambiguous installations must fail");
        let message = err.message();

        assert!(message.contains("ambiguous"));
        assert!(message.contains("source=vendor_enumeration"));
        assert!(message.find("MuMu Player Alpha") < message.find("MuMuPlayer-Beta"));
    }

    #[test]
    fn missing_backend_files_fail_with_source_and_root() {
        let temp = TempRoot::new("missing");
        let root = temp.path().join("MuMu Player Empty");
        fs::create_dir_all(&root).expect("install root");
        let installation = MumuInstallation {
            root: root.clone(),
            source: MumuInstallSource::ExplicitFolder,
        };

        let adb_err = resolve_mumu_adb(&installation).expect_err("missing ADB must fail");
        let dll_err = resolve_mumu_capture_dll(&installation).expect_err("missing DLL must fail");

        for message in [adb_err.message(), dll_err.message()] {
            assert!(message.contains("source=explicit_folder"));
            assert!(message.contains(&root.display().to_string()));
        }
    }

    #[test]
    fn adb_and_capture_resolve_from_the_same_installation_root() {
        let temp = TempRoot::new("same-root");
        let root = temp.path().join("MuMuPlayer-Future");
        let adb = root.join("nx_main/adb.exe");
        let dll = root.join("nx_device/15.1/shell/sdk/external_renderer_ipc.dll");
        for file in [&adb, &dll] {
            fs::create_dir_all(file.parent().expect("parent")).expect("candidate parent");
            fs::write(file, b"fixture").expect("candidate file");
        }
        let installation = resolve_mumu_installation_from_sources(Some(root.clone()), &[], &[])
            .expect("selection")
            .expect("installation");

        assert_eq!(resolve_mumu_adb(&installation).expect("ADB"), adb);
        assert_eq!(resolve_mumu_capture_dll(&installation).expect("DLL"), dll);
        assert_eq!(
            mumu_root_from_capture_dll(&dll).expect("DLL root"),
            installation.root
        );
    }

    #[test]
    fn vendor_enumeration_does_not_recurse() {
        let temp = TempRoot::new("bounded");
        let vendor = temp.path().join("vendor");
        fs::create_dir_all(vendor.join("nested/MuMu Player Hidden")).expect("nested root");

        let selected = resolve_mumu_installation_from_sources(None, &[], &[vendor])
            .expect("bounded enumeration");

        assert!(selected.is_none());
    }
}
