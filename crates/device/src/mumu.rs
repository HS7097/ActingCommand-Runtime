// SPDX-License-Identifier: AGPL-3.0-only

use crate::{DeviceError, DeviceResult};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

const NEMU_IPC_DLL_NAME: &str = "external_renderer_ipc.dll";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MumuInstallSource {
    ExplicitFolder,
    ConfiguredBackendPath,
    RunningProcess,
    VendorEnumeration,
}

impl MumuInstallSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ExplicitFolder => "explicit_folder",
            Self::ConfiguredBackendPath => "configured_backend_path",
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MumuBackendPaths {
    pub(crate) installation: MumuInstallation,
    pub(crate) adb_path: PathBuf,
    pub(crate) capture_dll_path: PathBuf,
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
        let root = canonicalize_install_root(&root, MumuInstallSource::ExplicitFolder)?;
        return Ok(Some(MumuInstallation {
            root,
            source: MumuInstallSource::ExplicitFolder,
        }));
    }

    let mut running_roots = Vec::new();
    for executable in running_executables {
        let executable = canonicalize_backend_file(executable, "running MuMu executable")?;
        let root = mumu_root_from_path(&executable).ok_or_else(|| {
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

pub(crate) fn resolve_mumu_backend_paths(
    configured_adb: Option<PathBuf>,
    explicit_root: Option<PathBuf>,
    explicit_dll: Option<PathBuf>,
) -> DeviceResult<Option<MumuBackendPaths>> {
    let configured_adb = configured_adb
        .filter(|path| !path.as_os_str().is_empty())
        .map(|path| canonicalize_backend_file(&path, "configured ADB executable"))
        .transpose()?;
    let explicit_dll = explicit_dll
        .filter(|path| !path.as_os_str().is_empty())
        .map(|path| canonicalize_backend_file(&path, "configured Nemu IPC DLL"))
        .transpose()?;
    let adb_root = configured_adb.as_deref().and_then(mumu_root_from_path);
    let dll_root = explicit_dll.as_deref().and_then(mumu_root_from_capture_dll);

    let installation = if let Some(root) = explicit_root {
        let installation = explicit_installation(root, MumuInstallSource::ExplicitFolder)?;
        let adb_label = configured_adb
            .as_deref()
            .map(|path| format!("configured ADB {}", path.display()))
            .unwrap_or_else(|| "configured ADB".to_string());
        let dll_label = explicit_dll
            .as_deref()
            .map(|path| format!("configured Nemu IPC DLL {}", path.display()))
            .unwrap_or_else(|| "configured Nemu IPC DLL".to_string());
        ensure_optional_root_matches(&adb_label, adb_root.as_deref(), &installation)?;
        ensure_optional_root_matches(&dll_label, dll_root.as_deref(), &installation)?;
        installation
    } else if let Some(root) = adb_root.clone() {
        let installation = explicit_installation(root, MumuInstallSource::ConfiguredBackendPath)?;
        let dll_label = explicit_dll
            .as_deref()
            .map(|path| format!("configured Nemu IPC DLL {}", path.display()))
            .unwrap_or_else(|| "configured Nemu IPC DLL".to_string());
        ensure_optional_root_matches(&dll_label, dll_root.as_deref(), &installation)?;
        installation
    } else if let Some(root) = dll_root.clone() {
        explicit_installation(root, MumuInstallSource::ConfiguredBackendPath)?
    } else {
        let Some(installation) = resolve_mumu_installation(None)? else {
            return Ok(None);
        };
        installation
    };

    let adb_path = match configured_adb {
        Some(path) => {
            let root = adb_root.ok_or_else(|| {
                DeviceError::fatal(format!(
                    "configured ADB {} does not identify the selected MuMu installation root {}; ADB and Nemu capture must share one installation identity",
                    path.display(),
                    installation.root.display()
                ))
            })?;
            ensure_same_install_root("configured ADB", &root, &installation)?;
            path
        }
        None => resolve_mumu_adb(&installation)?,
    };
    let capture_dll_path = match explicit_dll {
        Some(path) => {
            if !path_is_within_mumu_root(&path, &installation.root) {
                return Err(DeviceError::fatal(format!(
                    "configured Nemu IPC DLL {} is outside selected MuMu installation root {}",
                    path.display(),
                    installation.root.display()
                )));
            }
            if let Some(root) = dll_root {
                ensure_same_install_root("configured Nemu IPC DLL", &root, &installation)?;
            }
            path
        }
        None => resolve_mumu_capture_dll(&installation)?,
    };

    Ok(Some(MumuBackendPaths {
        installation,
        adb_path,
        capture_dll_path,
    }))
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
        let path = canonicalize_backend_file(path, &format!("MuMu {label}"))?;
        let root = canonicalize_install_root(&installation.root, installation.source)?;
        if !path_is_within_mumu_root(&path, &root) {
            return Err(DeviceError::fatal(format!(
                "MuMu {label} resolved outside selected installation root {}: {}",
                root.display(),
                path.display()
            )));
        }
        return Ok(path);
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
    let roots = roots
        .into_iter()
        .map(|root| canonicalize_install_root(&root, source))
        .collect::<DeviceResult<Vec<_>>>()?;
    let roots = stable_unique_paths(roots);
    if roots.len() != 1 {
        return Err(DeviceError::fatal(format!(
            "MuMu installation discovery is ambiguous for source={}: {}; configure ACTINGCOMMAND_NEMU_FOLDER, ACTINGCOMMAND_ADB_PATH, or an explicit backend path",
            source.as_str(),
            display_paths(&roots)
        )));
    }
    let root = roots.into_iter().next().expect("one root");
    Ok(MumuInstallation { root, source })
}

fn explicit_installation(
    root: PathBuf,
    source: MumuInstallSource,
) -> DeviceResult<MumuInstallation> {
    let root = canonicalize_install_root(&root, source)?;
    Ok(MumuInstallation { root, source })
}

fn ensure_optional_root_matches(
    label: &str,
    root: Option<&Path>,
    installation: &MumuInstallation,
) -> DeviceResult<()> {
    if let Some(root) = root {
        ensure_same_install_root(label, root, installation)?;
    }
    Ok(())
}

fn ensure_same_install_root(
    label: &str,
    root: &Path,
    installation: &MumuInstallation,
) -> DeviceResult<()> {
    if same_mumu_install_root(root, &installation.root) {
        return Ok(());
    }
    Err(DeviceError::fatal(format!(
        "{label} belongs to MuMu installation root {}, not selected root {}; ADB and Nemu capture must share one installation identity",
        root.display(),
        installation.root.display()
    )))
}

fn canonicalize_backend_file(path: &Path, label: &str) -> DeviceResult<PathBuf> {
    let canonical = std::fs::canonicalize(path).map_err(|err| {
        DeviceError::fatal(format!(
            "failed to canonicalize {label} {}: {err}",
            path.display()
        ))
    })?;
    if canonical.is_file() {
        return Ok(canonical);
    }
    Err(DeviceError::fatal(format!(
        "{label} does not exist or is not a file: {}",
        canonical.display()
    )))
}

fn canonicalize_install_root(root: &Path, source: MumuInstallSource) -> DeviceResult<PathBuf> {
    let canonical = std::fs::canonicalize(root).map_err(|err| {
        DeviceError::fatal(format!(
            "failed to canonicalize MuMu installation root from source={} at {}: {err}",
            source.as_str(),
            root.display()
        ))
    })?;
    if canonical.is_dir() {
        return Ok(canonical);
    }
    Err(DeviceError::fatal(format!(
        "MuMu installation root from source={} does not exist or is not a directory: {}",
        source.as_str(),
        canonical.display()
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
    #[cfg(windows)]
    use std::process::Command;
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

        assert_eq!(
            selected.root,
            fs::canonicalize(explicit).expect("canonical root")
        );
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
        fs::write(&executable, b"fixture").expect("process executable");
        fs::create_dir_all(&other_root).expect("other root");

        let selected = resolve_mumu_installation_from_sources(None, &[executable], &[vendor])
            .expect("running selection")
            .expect("installation");

        assert_eq!(
            selected.root,
            fs::canonicalize(running_root).expect("canonical root")
        );
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

        let resolved_adb = resolve_mumu_adb(&installation).expect("ADB");
        let resolved_dll = resolve_mumu_capture_dll(&installation).expect("DLL");
        assert_eq!(resolved_adb, fs::canonicalize(adb).expect("canonical ADB"));
        assert_eq!(resolved_dll, fs::canonicalize(dll).expect("canonical DLL"));
        assert_eq!(
            mumu_root_from_capture_dll(&resolved_dll).expect("DLL root"),
            installation.root
        );
    }

    #[test]
    fn coordinated_backend_paths_reject_cross_installation_inputs() {
        let temp = TempRoot::new("coordinated-mismatch");
        let first = temp.path().join("MuMu Player First");
        let second = temp.path().join("MuMuPlayer-Second");
        let adb = first.join("nx_main/adb.exe");
        let dll = second.join("nx_device/15.1/shell/sdk/external_renderer_ipc.dll");
        for file in [&adb, &dll] {
            fs::create_dir_all(file.parent().expect("parent")).expect("candidate parent");
            fs::write(file, b"fixture").expect("candidate file");
        }

        let err = resolve_mumu_backend_paths(Some(adb.clone()), None, Some(dll.clone()))
            .expect_err("cross-installation inputs must fail");

        assert!(err.message().contains("one installation identity"));
        assert!(err.message().contains(&first.display().to_string()));
        assert!(err.message().contains(&second.display().to_string()));
    }

    #[test]
    fn coordinated_backend_paths_preserve_one_installation_identity() {
        let temp = TempRoot::new("coordinated-same-root");
        let root = temp.path().join("MuMuPlayer-Future");
        let adb = root.join("nx_device/16.0/shell/adb.exe");
        let dll = root.join("nx_device/16.0/shell/sdk/external_renderer_ipc.dll");
        for file in [&adb, &dll] {
            fs::create_dir_all(file.parent().expect("parent")).expect("candidate parent");
            fs::write(file, b"fixture").expect("candidate file");
        }

        let paths = resolve_mumu_backend_paths(Some(adb.clone()), None, Some(dll.clone()))
            .expect("coordinated resolution")
            .expect("MuMu paths");

        assert_eq!(
            paths.installation.root,
            fs::canonicalize(root).expect("canonical root")
        );
        assert_eq!(
            paths.adb_path,
            fs::canonicalize(adb).expect("canonical ADB")
        );
        assert_eq!(
            paths.capture_dll_path,
            fs::canonicalize(dll).expect("canonical DLL")
        );
    }

    #[test]
    fn coordinated_backend_paths_reject_unassociated_adb() {
        let temp = TempRoot::new("coordinated-unassociated");
        let root = temp.path().join("MuMu Player Configured");
        let adb = temp.path().join("platform-tools/adb.exe");
        let dll = root.join("nx_device/16.0/shell/sdk/external_renderer_ipc.dll");
        for file in [&adb, &dll] {
            fs::create_dir_all(file.parent().expect("parent")).expect("candidate parent");
            fs::write(file, b"fixture").expect("candidate file");
        }

        let err = resolve_mumu_backend_paths(Some(adb.clone()), Some(root.clone()), Some(dll))
            .expect_err("unassociated ADB must fail");

        assert!(err.message().contains("does not identify"));
        assert!(
            err.message().contains(
                &fs::canonicalize(adb)
                    .expect("canonical ADB")
                    .display()
                    .to_string()
            )
        );
        assert!(
            err.message().contains(
                &fs::canonicalize(root)
                    .expect("canonical root")
                    .display()
                    .to_string()
            )
        );
    }

    #[test]
    fn coordinated_backend_paths_reject_parent_traversal_escape() {
        let temp = TempRoot::new("coordinated-parent-traversal");
        let selected = temp.path().join("MuMu Player Selected");
        let escaped = temp.path().join("MuMuPlayer-Escaped");
        let configured_adb = selected
            .join("nx_main")
            .join("..")
            .join("..")
            .join("MuMuPlayer-Escaped")
            .join("nx_main")
            .join("adb.exe");
        let escaped_adb = escaped.join("nx_main/adb.exe");
        let dll = selected.join("nx_device/17.0/shell/sdk/external_renderer_ipc.dll");
        for file in [&escaped_adb, &dll] {
            fs::create_dir_all(file.parent().expect("parent")).expect("candidate parent");
            fs::write(file, b"fixture").expect("candidate file");
        }
        fs::create_dir_all(selected.join("nx_main")).expect("lexical ADB parent");

        let err =
            resolve_mumu_backend_paths(Some(configured_adb), Some(selected.clone()), Some(dll))
                .expect_err("parent traversal must not escape the selected installation");

        assert!(err.message().contains("not selected root"));
        assert!(err.message().contains(&selected.display().to_string()));
    }

    #[test]
    fn coordinated_backend_paths_reject_directory_reparse_escape() {
        let temp = TempRoot::new("coordinated-reparse-escape");
        let selected = temp.path().join("MuMu Player Selected");
        let escaped = temp.path().join("MuMuPlayer-Escaped");
        let escaped_adb_dir = escaped.join("nx_main");
        let configured_adb = selected.join("nx_main/adb.exe");
        let escaped_adb = escaped_adb_dir.join("adb.exe");
        let dll = selected.join("nx_device/17.0/shell/sdk/external_renderer_ipc.dll");
        for file in [&escaped_adb, &dll] {
            fs::create_dir_all(file.parent().expect("parent")).expect("candidate parent");
            fs::write(file, b"fixture").expect("candidate file");
        }
        create_directory_link(&selected.join("nx_main"), &escaped_adb_dir);

        let err =
            resolve_mumu_backend_paths(Some(configured_adb), Some(selected.clone()), Some(dll))
                .expect_err("reparse escape must not leave the selected installation");

        assert!(err.message().contains("not selected root"));
        assert!(err.message().contains(&selected.display().to_string()));
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

    #[cfg(windows)]
    fn create_directory_link(link: &Path, target: &Path) {
        let output = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .expect("create junction");
        assert!(
            output.status.success(),
            "failed to create junction: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(unix)]
    fn create_directory_link(link: &Path, target: &Path) {
        std::os::unix::fs::symlink(target, link).expect("create directory symlink");
    }
}
