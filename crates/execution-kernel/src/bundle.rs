// SPDX-License-Identifier: AGPL-3.0-only

//! Production package ingress over the pack-containment capability. Workflow #336 PKG-S1
//! (option 丙): every entry takes a path or bytes and admits the package structurally; the
//! caller's reference, where an entry takes one, is only recorded on the bundle and compared
//! with nothing.

use actingcommand_contract::PackageRef;
use actingcommand_pack_containment::{Containment, ContainmentError, InstanceId, LoadedBundle};
use actingcommand_recognition_pack::VisionProvider;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

#[derive(Debug)]
pub enum ExecutionBundleError {
    Containment(ContainmentError),
    MissingLoadedBundle,
}

impl fmt::Display for ExecutionBundleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Containment(error) => error.fmt(formatter),
            Self::MissingLoadedBundle => formatter.write_str(
                "fatal execution bundle error: containment did not retain the loaded bundle",
            ),
        }
    }
}

impl Error for ExecutionBundleError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Containment(error) => Some(error),
            Self::MissingLoadedBundle => None,
        }
    }
}

impl From<ContainmentError> for ExecutionBundleError {
    fn from(error: ContainmentError) -> Self {
        Self::Containment(error)
    }
}

/// Immutable production resource capability: a package admitted by containment, by its path or
/// its bytes, with the reference its caller gave recorded on it.
#[derive(Debug)]
pub struct ExternallyVerifiedBundle {
    bundle: LoadedBundle,
}

impl ExternallyVerifiedBundle {
    pub(crate) fn load_metadata_path(
        instance_label: &str,
        locator: &std::path::Path,
        recorded: &PackageRef,
        deadline: std::time::Instant,
    ) -> Result<Self, ExecutionBundleError> {
        let instance = InstanceId::new(instance_label)?;
        let mut containment = Containment::for_metadata_validation();
        containment.load_path(&instance, locator, recorded, false, deadline)?;
        let bundle = containment
            .take_loaded(&instance)
            .ok_or(ExecutionBundleError::MissingLoadedBundle)?;
        Ok(Self { bundle })
    }

    pub fn load_path(
        instance_label: &str,
        locator: &std::path::Path,
        recorded: &PackageRef,
        observation: bool,
        vision_provider: Option<Arc<dyn VisionProvider>>,
        deadline: std::time::Instant,
    ) -> Result<Self, ExecutionBundleError> {
        let instance = InstanceId::new(instance_label)?;
        let mut containment = match vision_provider {
            Some(provider) => Containment::with_vision_provider(provider),
            None => Containment::new(),
        };
        containment.load_path(&instance, locator, recorded, observation, deadline)?;
        let bundle = containment
            .take_loaded(&instance)
            .ok_or(ExecutionBundleError::MissingLoadedBundle)?;
        Ok(Self { bundle })
    }

    /// Workflow #336: `load_path` for a content table already in memory; `recorded` is kept as
    /// `load_path` keeps it.
    pub fn load_content_entries(
        instance_label: &str,
        entries: std::collections::BTreeMap<String, Vec<u8>>,
        recorded: &PackageRef,
        observation: bool,
        vision_provider: Option<Arc<dyn VisionProvider>>,
        deadline: std::time::Instant,
    ) -> Result<Self, ExecutionBundleError> {
        let instance = InstanceId::new(instance_label)?;
        let mut containment = match vision_provider {
            Some(provider) => Containment::with_vision_provider(provider),
            None => Containment::new(),
        };
        containment.load_content_entries(&instance, entries, recorded, observation, deadline)?;
        let bundle = containment
            .take_loaded(&instance)
            .ok_or(ExecutionBundleError::MissingLoadedBundle)?;
        Ok(Self { bundle })
    }

    pub fn load_observation(
        instance_label: &str,
        zip_bytes: &[u8],
        recorded: &PackageRef,
        vision_provider: Option<Arc<dyn VisionProvider>>,
    ) -> Result<Self, ExecutionBundleError> {
        let instance = InstanceId::new(instance_label)?;
        let mut containment = match vision_provider {
            Some(provider) => Containment::with_vision_provider(provider),
            None => Containment::new(),
        };
        containment.load_observation(&instance, zip_bytes, recorded)?;
        let bundle = containment
            .take_loaded(&instance)
            .ok_or(ExecutionBundleError::MissingLoadedBundle)?;
        Ok(Self { bundle })
    }

    pub fn load(
        instance_label: &str,
        zip_bytes: &[u8],
        recorded: &PackageRef,
    ) -> Result<Self, ExecutionBundleError> {
        Self::load_with_optional_vision_provider(instance_label, zip_bytes, recorded, None)
    }

    pub fn load_with_vision_provider(
        instance_label: &str,
        zip_bytes: &[u8],
        recorded: &PackageRef,
        vision_provider: Arc<dyn VisionProvider>,
    ) -> Result<Self, ExecutionBundleError> {
        Self::load_with_optional_vision_provider(
            instance_label,
            zip_bytes,
            recorded,
            Some(vision_provider),
        )
    }

    fn load_with_optional_vision_provider(
        instance_label: &str,
        zip_bytes: &[u8],
        recorded: &PackageRef,
        vision_provider: Option<Arc<dyn VisionProvider>>,
    ) -> Result<Self, ExecutionBundleError> {
        let instance = InstanceId::new(instance_label)?;
        let mut containment = match vision_provider {
            Some(provider) => Containment::with_vision_provider(provider),
            None => Containment::new(),
        };
        containment.load(&instance, zip_bytes, recorded)?;
        let bundle = containment
            .take_loaded(&instance)
            .ok_or(ExecutionBundleError::MissingLoadedBundle)?;
        Ok(Self { bundle })
    }

    pub fn loaded_bundle(&self) -> &LoadedBundle {
        &self.bundle
    }

    pub fn into_loaded_bundle(self) -> LoadedBundle {
        self.bundle
    }
}
