// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{LabError, LabResult};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct LabState {
    root: PathBuf,
    arbitrator: ArbitratorStore,
    environment: EnvStore,
    sessions: SessionStore,
}

impl LabState {
    pub fn open(root: impl AsRef<Path>) -> LabResult<Self> {
        let root = root.as_ref();
        if root.as_os_str().is_empty() {
            return Err(LabError::usage("Lab state root must not be empty"));
        }
        let root = root.to_path_buf();
        Ok(Self {
            arbitrator: ArbitratorStore::new(root.join("lab2")),
            environment: EnvStore::new(root.join("env-detection")),
            sessions: SessionStore::new(root.join("session")),
            root,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn arbitrator(&self) -> &ArbitratorStore {
        &self.arbitrator
    }

    pub fn environment(&self) -> &EnvStore {
        &self.environment
    }

    pub fn sessions(&self) -> &SessionStore {
        &self.sessions
    }
}

#[derive(Debug, Clone)]
pub struct ArbitratorStore {
    root: PathBuf,
}

impl ArbitratorStore {
    fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[derive(Debug, Clone)]
pub struct EnvStore {
    root: PathBuf,
}

impl EnvStore {
    fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lab_state_is_the_only_domain_store_constructor() {
        let state = LabState::open("state").expect("state");
        assert_eq!(state.arbitrator().root(), Path::new("state/lab2"));
        assert_eq!(state.environment().root(), Path::new("state/env-detection"));
        assert_eq!(state.sessions().root(), Path::new("state/session"));
    }

    #[test]
    fn empty_state_root_fails_loudly() {
        let error = LabState::open("").expect_err("empty root");
        assert_eq!(error.code, "validation_failed");
    }
}
