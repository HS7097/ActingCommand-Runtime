// SPDX-License-Identifier: AGPL-3.0-only

//! #422 W1-8 (P27; QR-1 review L-3): the eviction proofs the writer publishes for its reader
//! pool, kept in a persistent structure so that a publication never copies the whole proof set
//! on the writer thread.
//!
//! The set is a few immutable levels, oldest first, each shared (`Arc`) by the writer's set and
//! every point published from it; a newer level's proof of an artifact replaces an older level's.
//! A change adds a level of its one proof and merges it into the levels before it while the
//! level before is at most twice its size, so each level ends more than twice as large as the
//! next newer one: `n` proofs make at most about `log2(n) + 1` levels, a proof is copied into a
//! new level about `log2(n)` times over its life, and a publication copies only the levels'
//! handles. A merge copies a level a published point still shares and leaves the point its own,
//! so a point keeps the proofs as of its head while the writer's set moves on.

use actingcommand_contract::{ArtifactEvictionProof, ArtifactId};
use std::collections::BTreeMap;
use std::sync::Arc;

type Level = BTreeMap<ArtifactId, ArtifactEvictionProof>;

/// #422 W1-8 (P27): the eviction proofs by artifact, as a few shared immutable levels.
#[derive(Clone, Default)]
pub(in crate::global) struct ProofLevels {
    /// Oldest first; each level more than twice as large as the next newer one.
    levels: Vec<Arc<Level>>,
}

impl ProofLevels {
    /// Sets `artifact`'s proof (a new one, or its outcome on its intent): a level of this one
    /// proof, merged into the levels before it while the level before is at most twice its size.
    pub(in crate::global) fn set(&mut self, artifact: ArtifactId, proof: ArtifactEvictionProof) {
        let mut newest = Level::from([(artifact, proof)]);
        while let Some(older) = self.levels.pop() {
            if older.len() > newest.len().saturating_mul(2) {
                self.levels.push(older);
                break;
            }
            // The older level's own map when nothing else shares it; a copy when a published
            // point still does, which keeps that point's level as it was.
            let mut merged = Arc::unwrap_or_clone(older);
            merged.extend(newest);
            newest = merged;
        }
        self.levels.push(Arc::new(newest));
    }

    /// `artifact`'s proof: the newest level's that holds one.
    pub(in crate::global) fn get(&self, artifact: &ArtifactId) -> Option<&ArtifactEvictionProof> {
        self.levels
            .iter()
            .rev()
            .find_map(|level| level.get(artifact))
    }

    /// Tests: the levels the set is kept in.
    #[cfg(test)]
    pub(in crate::global) fn level_count(&self) -> usize {
        self.levels.len()
    }

    /// Tests: the artifacts the set holds a proof of.
    #[cfg(test)]
    pub(in crate::global) fn artifacts(&self) -> std::collections::BTreeSet<ArtifactId> {
        self.levels
            .iter()
            .flat_map(|level| level.keys().copied())
            .collect()
    }
}
