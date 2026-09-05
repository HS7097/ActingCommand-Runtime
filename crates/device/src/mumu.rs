// SPDX-License-Identifier: AGPL-3.0-only

use crate::{
    DeviceError, DeviceResult, NemuConfiguredAdbClass, NemuResolutionContext,
    NemuResolutionCountKind, NemuResolutionReason,
};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

const NEMU_IPC_DLL_NAME: &str = "external_renderer_ipc.dll";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MumuInstallSource {
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
        NemuResolutionCountKind::AdbExecutables,
        mumu_adb_candidates(&installation.root)?,
    )
}

pub(crate) fn resolve_mumu_capture_dll(installation: &MumuInstallation) -> DeviceResult<PathBuf> {
    resolve_existing_candidate(
        installation,
        "Nemu capture DLL",
        NemuResolutionCountKind::CaptureDllFiles,
        mumu_capture_dll_candidates(&installation.root)?,
    )
}

fn resolve_mumu_adb_for_capture_dll(
    installation: &MumuInstallation,
    capture_dll: Option<&Path>,
) -> DeviceResult<PathBuf> {
    let Some(version_dir) = capture_dll.and_then(mumu_version_dir_from_path) else {
        return resolve_mumu_adb(installation);
    };
    resolve_existing_candidate(
        installation,
        "ADB executable matching configured Nemu capture DLL version",
        NemuResolutionCountKind::AdbExecutables,
        vec![
            version_dir.join("shell").join("adb.exe"),
            installation.root.join("nx_main").join("adb.exe"),
        ],
    )
}

fn resolve_mumu_capture_dll_for_adb(
    installation: &MumuInstallation,
    adb_path: &Path,
) -> DeviceResult<PathBuf> {
    if let Some(version_dir) = mumu_version_dir_from_path(adb_path) {
        return resolve_existing_candidate(
            installation,
            "Nemu capture DLL matching configured ADB version",
            NemuResolutionCountKind::CaptureDllFiles,
            vec![
                version_dir
                    .join("shell")
                    .join("sdk")
                    .join(NEMU_IPC_DLL_NAME),
            ],
        );
    }

    let candidates = mumu_capture_dll_candidates(&installation.root)?;
    if candidates.first().is_some_and(|path| path.is_file()) {
        return resolve_existing_candidate(
            installation,
            "Nemu capture DLL",
            NemuResolutionCountKind::CaptureDllFiles,
            candidates,
        );
    }
    let version_candidates = candidates
        .iter()
        .skip(1)
        .filter(|path| path.is_file())
        .cloned()
        .collect::<Vec<_>>();
    if version_candidates.len() > 1 {
        return Err(DeviceError::fatal(format!(
            "MuMu Nemu capture DLL discovery is ambiguous for shared ADB {} under install_root={}; configure an explicit Nemu IPC DLL to select one version; candidates: {}",
            adb_path.display(),
            installation.root.display(),
            display_paths(&version_candidates)
        )).with_nemu_resolution_context_if_absent(
            NemuResolutionContext::new(NemuResolutionReason::SharedAdbMultipleDllVersions)
                .with_count(NemuResolutionCountKind::DllVersions, version_candidates.len(), false)
                .with_source(installation.source),
        ));
    }
    resolve_mumu_capture_dll(installation)
}

fn ensure_mumu_backend_version_matches(
    adb_path: &Path,
    capture_dll_path: &Path,
    source: MumuInstallSource,
) -> DeviceResult<()> {
    let Some(adb_version) = mumu_version_dir_from_path(adb_path) else {
        return Ok(());
    };
    let Some(dll_version) = mumu_version_dir_from_path(capture_dll_path) else {
        return Err(DeviceError::fatal(format!(
            "MuMu ADB {} has version identity {}, but Nemu capture DLL {} has no matching nx_device/<version> identity",
            adb_path.display(),
            adb_version.display(),
            capture_dll_path.display()
        )).with_nemu_resolution_context_if_absent(
            NemuResolutionContext::new(NemuResolutionReason::DllVersionMissing).with_source(source),
        ));
    };
    if path_key(&adb_version) == path_key(&dll_version) {
        return Ok(());
    }
    Err(DeviceError::fatal(format!(
        "MuMu ADB {} has version identity {}, but Nemu capture DLL {} has different version identity {}; ADB and capture DLL must use the same nx_device/<version>",
        adb_path.display(),
        adb_version.display(),
        capture_dll_path.display(),
        dll_version.display()
    )).with_nemu_resolution_context_if_absent(
        NemuResolutionContext::new(NemuResolutionReason::VersionMismatch).with_source(source),
    ))
}

pub(crate) fn resolve_mumu_backend_paths(
    configured_adb: Option<PathBuf>,
    explicit_root: Option<PathBuf>,
    explicit_dll: Option<PathBuf>,
) -> DeviceResult<Option<MumuBackendPaths>> {
    let adb_class = configured_adb
        .as_deref()
        .map(|path| nemu_configured_adb_class(Some(path)));
    let has_root = explicit_root.is_some();
    let has_dll = explicit_dll.is_some();
    resolve_mumu_backend_paths_inner(configured_adb, explicit_root, explicit_dll)
        .map_err(|error| error.with_nemu_resolution_provenance(adb_class, has_root, has_dll))
}

fn resolve_mumu_backend_paths_inner(
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
                )).with_nemu_resolution_context_if_absent(
                    NemuResolutionContext::new(NemuResolutionReason::ConfiguredAdbIdentityUnrecognized)
                        .with_source(installation.source),
                )
            })?;
            ensure_same_install_root("configured ADB", &root, &installation)?;
            path
        }
        None => resolve_mumu_adb_for_capture_dll(&installation, explicit_dll.as_deref())?,
    };
    let capture_dll_path = match explicit_dll {
        Some(path) => {
            if !path_is_within_mumu_root(&path, &installation.root) {
                return Err(DeviceError::fatal(format!(
                    "configured Nemu IPC DLL {} is outside selected MuMu installation root {}",
                    path.display(),
                    installation.root.display()
                ))
                .with_nemu_resolution_context_if_absent(
                    NemuResolutionContext::new(NemuResolutionReason::DllOutsideRoot)
                        .with_source(installation.source),
                ));
            }
            if let Some(root) = dll_root {
                ensure_same_install_root("configured Nemu IPC DLL", &root, &installation)?;
            }
            path
        }
        None => resolve_mumu_capture_dll_for_adb(&installation, &adb_path)?,
    };
    ensure_mumu_backend_version_matches(&adb_path, &capture_dll_path, installation.source)?;

    Ok(Some(MumuBackendPaths {
        installation,
        adb_path,
        capture_dll_path,
    }))
}

pub(crate) fn resolve_mumu_backend_paths_for_running_target(
    configured_adb: PathBuf,
    target_serial: &str,
    explicit_instance_id: Option<i32>,
    explicit_root: Option<PathBuf>,
    explicit_dll: Option<PathBuf>,
) -> DeviceResult<MumuBackendPaths> {
    let executable =
        crate::discovery::running_mumu_executable_for_target(target_serial, explicit_instance_id)
            .map_err(|error| {
            error.with_nemu_resolution_provenance(
                Some(nemu_configured_adb_class(Some(&configured_adb))),
                explicit_root.is_some(),
                explicit_dll.is_some(),
            )
        })?;
    resolve_mumu_backend_paths_for_running_executable(
        configured_adb,
        executable,
        explicit_root,
        explicit_dll,
    )
}

fn resolve_mumu_backend_paths_for_running_executable(
    configured_adb: PathBuf,
    running_executable: PathBuf,
    explicit_root: Option<PathBuf>,
    explicit_dll: Option<PathBuf>,
) -> DeviceResult<MumuBackendPaths> {
    let adb_class = Some(nemu_configured_adb_class(Some(&configured_adb)));
    let has_root = explicit_root.is_some();
    let has_dll = explicit_dll.is_some();
    resolve_mumu_backend_paths_for_running_executable_inner(
        configured_adb,
        running_executable,
        explicit_root,
        explicit_dll,
    )
    .map_err(|error| error.with_nemu_resolution_provenance(adb_class, has_root, has_dll))
}

fn resolve_mumu_backend_paths_for_running_executable_inner(
    configured_adb: PathBuf,
    running_executable: PathBuf,
    explicit_root: Option<PathBuf>,
    explicit_dll: Option<PathBuf>,
) -> DeviceResult<MumuBackendPaths> {
    let configured_adb = canonicalize_backend_file(&configured_adb, "configured ADB executable")?;
    let running_executable =
        canonicalize_backend_file(&running_executable, "selected running MuMu executable")?;
    let running_root = mumu_root_from_path(&running_executable).ok_or_else(|| {
        DeviceError::fatal(format!(
            "selected running MuMu executable has invalid topology and does not identify an installation root: {}",
            running_executable.display()
        )).with_nemu_resolution_context_if_absent(
            NemuResolutionContext::new(NemuResolutionReason::RunningExecutableTopologyInvalid)
                .with_source(MumuInstallSource::RunningProcess),
        )
    })?;
    let installation = explicit_installation(running_root, MumuInstallSource::RunningProcess)?;

    if let Some(explicit_root) = explicit_root {
        let explicit = explicit_installation(explicit_root, MumuInstallSource::ExplicitFolder)?;
        ensure_same_install_root("configured MuMu root", &explicit.root, &installation)?;
    }

    let capture_dll_path = match explicit_dll {
        Some(path) => {
            let path = canonicalize_backend_file(&path, "configured Nemu IPC DLL")?;
            if !path_is_within_mumu_root(&path, &installation.root) {
                return Err(DeviceError::fatal(format!(
                    "configured Nemu IPC DLL {} is outside selected MuMu installation root {}",
                    path.display(),
                    installation.root.display()
                ))
                .with_nemu_resolution_context_if_absent(
                    NemuResolutionContext::new(NemuResolutionReason::DllOutsideRoot)
                        .with_source(installation.source),
                ));
            }
            let dll_root = mumu_root_from_capture_dll(&path).ok_or_else(|| {
                DeviceError::fatal(format!(
                    "configured Nemu IPC DLL has invalid topology under selected MuMu installation root {}: {}",
                    installation.root.display(),
                    path.display()
                )).with_nemu_resolution_context_if_absent(
                    NemuResolutionContext::new(NemuResolutionReason::DllVersionMissing)
                        .with_source(installation.source),
                )
            })?;
            ensure_same_install_root("configured Nemu IPC DLL", &dll_root, &installation)?;
            path
        }
        None => {
            let version_dir = mumu_version_dir_from_path(&running_executable).ok_or_else(|| {
                DeviceError::fatal(format!(
                    "selected running MuMu executable has invalid topology and no nx_device/<version> identity: {}",
                    running_executable.display()
                )).with_nemu_resolution_context_if_absent(
                    NemuResolutionContext::new(NemuResolutionReason::RunningVersionMissing)
                        .with_source(installation.source),
                )
            })?;
            resolve_existing_candidate(
                &installation,
                "Nemu capture DLL matching selected running MuMu version",
                NemuResolutionCountKind::CaptureDllFiles,
                vec![
                    version_dir
                        .join("shell")
                        .join("sdk")
                        .join(NEMU_IPC_DLL_NAME),
                ],
            )?
        }
    };
    ensure_running_mumu_version_matches(&running_executable, &capture_dll_path)?;

    Ok(MumuBackendPaths {
        installation,
        adb_path: configured_adb,
        capture_dll_path,
    })
}

fn ensure_running_mumu_version_matches(
    running_executable: &Path,
    capture_dll_path: &Path,
) -> DeviceResult<()> {
    let running_version = mumu_version_dir_from_path(running_executable).ok_or_else(|| {
        DeviceError::fatal(format!(
            "selected running MuMu executable has invalid topology and no nx_device/<version> identity: {}",
            running_executable.display()
        )).with_nemu_resolution_context_if_absent(
            NemuResolutionContext::new(NemuResolutionReason::RunningVersionMissing)
                .with_source(MumuInstallSource::RunningProcess),
        )
    })?;
    let dll_version = mumu_version_dir_from_path(capture_dll_path).ok_or_else(|| {
        DeviceError::fatal(format!(
            "selected running MuMu executable {} has version identity {}, but Nemu capture DLL {} has no matching nx_device/<version> identity",
            running_executable.display(),
            running_version.display(),
            capture_dll_path.display()
        )).with_nemu_resolution_context_if_absent(
            NemuResolutionContext::new(NemuResolutionReason::DllVersionMissing)
                .with_source(MumuInstallSource::RunningProcess),
        )
    })?;
    if path_key(&running_version) == path_key(&dll_version) {
        return Ok(());
    }
    Err(DeviceError::fatal(format!(
        "selected running MuMu executable {} has version identity {}, but Nemu capture DLL {} has different version identity {}; the selected process and capture DLL must use the same nx_device/<version>",
        running_executable.display(),
        running_version.display(),
        capture_dll_path.display(),
        dll_version.display()
    )).with_nemu_resolution_context_if_absent(
        NemuResolutionContext::new(NemuResolutionReason::VersionMismatch)
            .with_source(MumuInstallSource::RunningProcess),
    ))
}

pub(crate) fn nemu_configured_adb_class(path: Option<&Path>) -> NemuConfiguredAdbClass {
    match path.filter(|path| !path.as_os_str().is_empty()) {
        None => NemuConfiguredAdbClass::Absent,
        Some(path) if mumu_version_dir_from_path(path).is_some() => {
            NemuConfiguredAdbClass::VersionedMumu
        }
        Some(path) if mumu_root_from_path(path).is_some() => NemuConfiguredAdbClass::SharedMumu,
        Some(_) => NemuConfiguredAdbClass::Generic,
    }
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

fn mumu_version_dir_from_path(path: &Path) -> Option<PathBuf> {
    let mut prefix = PathBuf::new();
    let mut components = path.components();
    while let Some(component) = components.next() {
        prefix.push(component.as_os_str());
        if component
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case("nx_device")
        {
            prefix.push(components.next()?.as_os_str());
            return Some(prefix);
        }
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
    count_kind: NemuResolutionCountKind,
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
            ))
            .with_nemu_resolution_context_if_absent(
                NemuResolutionContext::new(NemuResolutionReason::CandidateOutsideRoot)
                    .with_source(installation.source),
            ));
        }
        return Ok(path);
    }
    Err(DeviceError::fatal(format!(
        "MuMu {label} discovery selected source={} install_root={} but no candidate file exists; checked: {}",
        installation.source.as_str(),
        installation.root.display(),
        display_paths(&candidates)
    )).with_nemu_resolution_context_if_absent(
        NemuResolutionContext::new(NemuResolutionReason::CandidateAbsent)
            .with_count(count_kind, 0, false)
            .with_source(installation.source),
    ))
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
        )).with_nemu_resolution_context_if_absent(
            NemuResolutionContext::new(if roots.is_empty() {
                NemuResolutionReason::InstallationAbsent
            } else {
                NemuResolutionReason::InstallationAmbiguous
            })
                .with_count(NemuResolutionCountKind::InstallationRoots, roots.len(), false)
                .with_source(source),
        ));
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
    )).with_nemu_resolution_context_if_absent(
        NemuResolutionContext::new(NemuResolutionReason::RootMismatch).with_source(installation.source),
    ))
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
    fn nemu_resolution_diagnostic_preserves_reason_count_and_source() {
        let temp = TempRoot::new("resolution-context");
        let root = temp.path().join("MuMuPlayer-One");
        let other = temp.path().join("MuMuPlayer-Two");
        let shared_adb = root.join("nx_main/adb.exe");
        let versioned_adb = root.join("nx_device/15.0/shell/adb.exe");
        let old_dll = root.join("nx_device/12.0/shell/sdk/external_renderer_ipc.dll");
        let new_dll = root.join("nx_device/15.0/shell/sdk/external_renderer_ipc.dll");
        let executable = root.join("nx_device/15.0/shell/MuMuNxDevice.exe");
        let generic_adb = temp.path().join("platform-tools/adb.exe");
        for file in [
            &shared_adb,
            &versioned_adb,
            &old_dll,
            &new_dll,
            &executable,
            &generic_adb,
        ] {
            fs::create_dir_all(file.parent().expect("parent")).expect("candidate parent");
            fs::write(file, b"fixture").expect("candidate file");
        }
        fs::create_dir_all(&other).expect("other installation");

        let shared = resolve_mumu_backend_paths(Some(shared_adb.clone()), None, None)
            .expect_err("multiple DLL versions");
        assert!(shared.message().contains("ambiguous for shared ADB"));
        assert_eq!(
            shared.nemu_resolution_context(),
            Some(
                NemuResolutionContext::new(NemuResolutionReason::SharedAdbMultipleDllVersions)
                    .with_count(NemuResolutionCountKind::DllVersions, 2, false)
                    .with_source(MumuInstallSource::ConfiguredBackendPath)
                    .with_provenance(Some(NemuConfiguredAdbClass::SharedMumu), false, false),
            )
        );
        let roots = select_unique_installation(
            vec![root.clone(), other.clone()],
            MumuInstallSource::VendorEnumeration,
        )
        .expect_err("multiple installation roots");
        assert_eq!(
            roots.nemu_resolution_context(),
            Some(
                NemuResolutionContext::new(NemuResolutionReason::InstallationAmbiguous)
                    .with_count(NemuResolutionCountKind::InstallationRoots, 2, false)
                    .with_source(MumuInstallSource::VendorEnumeration),
            )
        );
        let mismatch = resolve_mumu_backend_paths(Some(shared_adb), Some(other.clone()), None)
            .expect_err("configured root mismatch");
        assert!(mismatch.message().contains("not selected root"));
        assert_eq!(
            mismatch.nemu_resolution_context(),
            Some(
                NemuResolutionContext::new(NemuResolutionReason::RootMismatch)
                    .with_source(MumuInstallSource::ExplicitFolder)
                    .with_provenance(Some(NemuConfiguredAdbClass::SharedMumu), true, false),
            )
        );
        let version = resolve_mumu_backend_paths(Some(versioned_adb), None, Some(old_dll.clone()))
            .expect_err("configured version mismatch");
        assert_eq!(
            version.nemu_resolution_context(),
            Some(
                NemuResolutionContext::new(NemuResolutionReason::VersionMismatch)
                    .with_source(MumuInstallSource::ConfiguredBackendPath)
                    .with_provenance(Some(NemuConfiguredAdbClass::VersionedMumu), false, true),
            )
        );
        let identity = resolve_mumu_backend_paths(
            Some(generic_adb.clone()),
            Some(root.clone()),
            Some(new_dll),
        )
        .expect_err("configured identity mismatch");
        assert_eq!(
            identity.nemu_resolution_context(),
            Some(
                NemuResolutionContext::new(NemuResolutionReason::ConfiguredAdbIdentityUnrecognized)
                    .with_source(MumuInstallSource::ExplicitFolder)
                    .with_provenance(Some(NemuConfiguredAdbClass::Generic), true, true),
            )
        );
        let target = resolve_mumu_backend_paths_for_running_executable(
            generic_adb,
            executable.clone(),
            None,
            Some(old_dll),
        )
        .expect_err("selected running version mismatch");
        assert_eq!(
            target.nemu_resolution_context(),
            Some(
                NemuResolutionContext::new(NemuResolutionReason::VersionMismatch)
                    .with_source(MumuInstallSource::RunningProcess)
                    .with_provenance(Some(NemuConfiguredAdbClass::Generic), false, true),
            )
        );
        let missing = crate::discovery::running_mumu_executable_for_target_from_processes(
            "127.0.0.1:16448",
            None,
            &[],
        )
        .expect_err("missing selected process");
        assert_eq!(
            missing.nemu_resolution_context(),
            Some(
                NemuResolutionContext::new(NemuResolutionReason::TargetProcessAbsent)
                    .with_count(NemuResolutionCountKind::MatchedTargetProcesses, 0, false)
                    .with_source(MumuInstallSource::RunningProcess),
            )
        );
        let processes = (1..=3)
            .map(|process_id| crate::DeviceDiscoveryProcess {
                process_id,
                name: "MuMuNxDevice.exe".to_string(),
                executable_path: Some(executable.clone()),
                command_line: Some("MuMuNxDevice.exe -v 2".to_string()),
            })
            .collect::<Vec<_>>();
        let ambiguous = crate::discovery::running_mumu_executable_for_target_from_processes(
            "127.0.0.1:16448",
            None,
            &processes,
        )
        .expect_err("ambiguous selected process");
        assert_eq!(
            ambiguous.nemu_resolution_context(),
            Some(
                NemuResolutionContext::new(NemuResolutionReason::TargetProcessAmbiguous)
                    .with_count(NemuResolutionCountKind::MatchedTargetProcesses, 2, true)
                    .with_source(MumuInstallSource::RunningProcess),
            )
        );
        for error in [
            &shared, &roots, &mismatch, &version, &identity, &target, &missing, &ambiguous,
        ] {
            let message = error
                .diagnostic_message()
                .expect("typed diagnostic rendering");
            assert!(message.len() <= 1_024);
            assert!(!message.contains(['/', '\\', ':']));
            assert!(!message.chars().any(char::is_control));
            assert_eq!(error.severity(), crate::DeviceErrorSeverity::Fatal);
            assert!(!error.is_fallback_eligible());
        }
        assert!(
            mismatch
                .diagnostic_message()
                .expect("message")
                .contains("count=unavailable")
        );
        let original = DeviceError::fatal("complete lower detail")
            .with_diagnostic(crate::DeviceErrorCategory::Native, "nemu.native.failure")
            .with_diagnostic_context(
                "nemu_ipc",
                "capture",
                crate::DeviceErrorSensitivity::Sensitive,
            );
        let preserved = original.clone().with_nemu_resolution_context_if_absent(
            shared.nemu_resolution_context().expect("context"),
        );
        assert_eq!(preserved.message(), original.message());
        assert_eq!(preserved.diagnostic(), original.diagnostic());
        assert_eq!(
            preserved.diagnostic_context(),
            original.diagnostic_context()
        );
        assert_eq!(preserved.diagnostic_message(), None);
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
    fn coordinated_backend_paths_keep_configured_adb_version() {
        let temp = TempRoot::new("coordinated-version-match");
        let root = temp.path().join("MuMuPlayer-MultiVersion");
        let old_dll = root.join("nx_device/12.0/shell/sdk/external_renderer_ipc.dll");
        let selected_adb = root.join("nx_device/15.0/shell/adb.exe");
        let selected_dll = root.join("nx_device/15.0/shell/sdk/external_renderer_ipc.dll");
        for file in [&old_dll, &selected_adb, &selected_dll] {
            fs::create_dir_all(file.parent().expect("parent")).expect("candidate parent");
            fs::write(file, b"fixture").expect("candidate file");
        }

        let paths = resolve_mumu_backend_paths(Some(selected_adb.clone()), None, None)
            .expect("coordinated resolution")
            .expect("MuMu paths");

        assert_eq!(
            paths.adb_path,
            fs::canonicalize(selected_adb).expect("canonical ADB")
        );
        assert_eq!(
            paths.capture_dll_path,
            fs::canonicalize(selected_dll).expect("canonical selected DLL")
        );
        assert_ne!(
            paths.capture_dll_path,
            fs::canonicalize(old_dll).expect("canonical old DLL")
        );
    }

    #[test]
    fn coordinated_backend_paths_reject_ambiguous_versions_for_shared_adb() {
        let temp = TempRoot::new("coordinated-shared-adb-ambiguous");
        let root = temp.path().join("MuMuPlayer-MultiVersion");
        let shared_adb = root.join("nx_main/adb.exe");
        let old_dll = root.join("nx_device/12.0/shell/sdk/external_renderer_ipc.dll");
        let new_dll = root.join("nx_device/15.0/shell/sdk/external_renderer_ipc.dll");
        for file in [&shared_adb, &old_dll, &new_dll] {
            fs::create_dir_all(file.parent().expect("parent")).expect("candidate parent");
            fs::write(file, b"fixture").expect("candidate file");
        }

        let err = resolve_mumu_backend_paths(Some(shared_adb), None, None)
            .expect_err("shared ADB must not choose between multiple DLL versions");
        let old_dll = fs::canonicalize(old_dll).expect("canonical old DLL");
        let new_dll = fs::canonicalize(new_dll).expect("canonical new DLL");

        assert!(err.message().contains("ambiguous"));
        assert!(err.message().contains(&old_dll.display().to_string()));
        assert!(err.message().contains(&new_dll.display().to_string()));
    }

    #[test]
    fn coordinated_backend_paths_reject_explicit_cross_version_pair() {
        let temp = TempRoot::new("coordinated-cross-version");
        let root = temp.path().join("MuMuPlayer-MultiVersion");
        let adb = root.join("nx_device/15.0/shell/adb.exe");
        let dll = root.join("nx_device/12.0/shell/sdk/external_renderer_ipc.dll");
        for file in [&adb, &dll] {
            fs::create_dir_all(file.parent().expect("parent")).expect("candidate parent");
            fs::write(file, b"fixture").expect("candidate file");
        }

        let err = resolve_mumu_backend_paths(Some(adb), None, Some(dll))
            .expect_err("different version identities must fail");

        assert!(err.message().contains("different version identity"));
        assert!(err.message().contains("15.0"));
        assert!(err.message().contains("12.0"));
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

    // Task Contract: Workflow #256.
    // Test class: authorized Defect regression.
    #[test]
    fn running_target_paths_keep_generic_adb_and_select_process_version_dll() {
        let temp = TempRoot::new("running-target");
        let generic_adb = temp.path().join("platform-tools/adb.exe");
        let root = temp.path().join("MuMuPlayer-Selected");
        let executable = root.join("nx_device/18.2/shell/MuMuNxDevice.exe");
        let dll = root.join("nx_device/18.2/shell/sdk/external_renderer_ipc.dll");
        for file in [&generic_adb, &executable, &dll] {
            fs::create_dir_all(file.parent().expect("parent")).expect("fixture parent");
            fs::write(file, b"fixture").expect("fixture file");
        }

        let paths = resolve_mumu_backend_paths_for_running_executable(
            generic_adb.clone(),
            executable,
            None,
            None,
        )
        .expect("running target paths");

        assert_eq!(paths.installation.source, MumuInstallSource::RunningProcess);
        assert_eq!(
            paths.installation.root,
            fs::canonicalize(root).expect("canonical root")
        );
        assert_eq!(
            paths.adb_path,
            fs::canonicalize(generic_adb).expect("canonical generic ADB")
        );
        assert_eq!(
            paths.capture_dll_path,
            fs::canonicalize(dll).expect("canonical DLL")
        );
    }

    // Task Contract: Workflow #256.
    // Test class: specification criterion.
    #[test]
    fn running_target_paths_preserve_matching_partial_overrides() {
        let temp = TempRoot::new("running-target-partial-overrides");
        let generic_adb = temp.path().join("platform-tools/adb.exe");
        let root = temp.path().join("MuMuPlayer-Selected");
        let executable = root.join("nx_device/18.2/shell/MuMuNxDevice.exe");
        let dll = root.join("nx_device/18.2/shell/sdk/external_renderer_ipc.dll");
        for file in [&generic_adb, &executable, &dll] {
            fs::create_dir_all(file.parent().expect("parent")).expect("fixture parent");
            fs::write(file, b"fixture").expect("fixture file");
        }

        for (explicit_root, explicit_dll) in [(Some(root.clone()), None), (None, Some(dll.clone()))]
        {
            let paths = resolve_mumu_backend_paths_for_running_executable(
                generic_adb.clone(),
                executable.clone(),
                explicit_root,
                explicit_dll,
            )
            .expect("matching partial override");
            assert_eq!(
                paths.installation.root,
                fs::canonicalize(&root).expect("canonical root")
            );
            assert_eq!(
                paths.capture_dll_path,
                fs::canonicalize(&dll).expect("canonical DLL")
            );
        }
    }

    // Task Contract: Workflow #256.
    // Test class: specification criterion.
    #[test]
    fn running_target_paths_reject_explicit_root_mismatch() {
        let temp = TempRoot::new("running-target-root-mismatch");
        let generic_adb = temp.path().join("platform-tools/adb.exe");
        let selected = temp.path().join("MuMuPlayer-Selected");
        let other = temp.path().join("MuMuPlayer-Other");
        let executable = selected.join("nx_device/18.2/shell/MuMuNxDevice.exe");
        for file in [&generic_adb, &executable] {
            fs::create_dir_all(file.parent().expect("parent")).expect("fixture parent");
            fs::write(file, b"fixture").expect("fixture file");
        }
        fs::create_dir_all(&other).expect("other root");

        let err = resolve_mumu_backend_paths_for_running_executable(
            generic_adb,
            executable,
            Some(other),
            None,
        )
        .expect_err("different explicit root must fail");

        assert!(err.message().contains("configured MuMu root"));
        assert!(err.message().contains("not selected root"));
    }

    // Task Contract: Workflow #256.
    // Test class: specification criterion.
    #[test]
    fn running_target_paths_reject_dll_outside_selected_root() {
        let temp = TempRoot::new("running-target-dll-root");
        let generic_adb = temp.path().join("platform-tools/adb.exe");
        let selected = temp.path().join("MuMuPlayer-Selected");
        let other = temp.path().join("MuMuPlayer-Other");
        let executable = selected.join("nx_device/18.2/shell/MuMuNxDevice.exe");
        let dll = other.join("nx_device/18.2/shell/sdk/external_renderer_ipc.dll");
        for file in [&generic_adb, &executable, &dll] {
            fs::create_dir_all(file.parent().expect("parent")).expect("fixture parent");
            fs::write(file, b"fixture").expect("fixture file");
        }

        let err = resolve_mumu_backend_paths_for_running_executable(
            generic_adb,
            executable,
            None,
            Some(dll),
        )
        .expect_err("DLL outside selected root must fail");

        assert!(
            err.message()
                .contains("outside selected MuMu installation root")
        );
    }

    // Task Contract: Workflow #256.
    // Test class: specification criterion.
    #[test]
    fn running_target_paths_reject_different_dll_version() {
        let temp = TempRoot::new("running-target-version");
        let generic_adb = temp.path().join("platform-tools/adb.exe");
        let root = temp.path().join("MuMuPlayer-Selected");
        let executable = root.join("nx_device/18.2/shell/MuMuNxDevice.exe");
        let dll = root.join("nx_device/17.9/shell/sdk/external_renderer_ipc.dll");
        for file in [&generic_adb, &executable, &dll] {
            fs::create_dir_all(file.parent().expect("parent")).expect("fixture parent");
            fs::write(file, b"fixture").expect("fixture file");
        }

        let err = resolve_mumu_backend_paths_for_running_executable(
            generic_adb,
            executable,
            None,
            Some(dll),
        )
        .expect_err("different DLL version must fail");

        assert!(err.message().contains("different version identity"));
    }

    // Task Contract: Workflow #256.
    // Test class: specification criterion.
    #[test]
    fn running_target_paths_report_missing_same_version_dll() {
        let temp = TempRoot::new("running-target-missing-dll");
        let generic_adb = temp.path().join("platform-tools/adb.exe");
        let root = temp.path().join("MuMuPlayer-Selected");
        let executable = root.join("nx_device/18.2/shell/MuMuNxDevice.exe");
        for file in [&generic_adb, &executable] {
            fs::create_dir_all(file.parent().expect("parent")).expect("fixture parent");
            fs::write(file, b"fixture").expect("fixture file");
        }

        let err =
            resolve_mumu_backend_paths_for_running_executable(generic_adb, executable, None, None)
                .expect_err("missing version DLL must fail");

        assert!(err.message().contains("no candidate file exists"));
        assert!(err.message().contains("selected running MuMu version"));
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
