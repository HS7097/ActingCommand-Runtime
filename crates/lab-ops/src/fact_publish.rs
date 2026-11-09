// SPDX-License-Identifier: AGPL-3.0-only

//! `facts publish` (Workflow #434 LAB-1b; register §1.287), moved from actingctl's
//! `agent-publish-facts --record-file`: one bounded, valid fact observation read from a file
//! (LIVE-FACT-POOL-v1) and published through the Runtime's formal entry, `PublishFacts`. actingctl
//! sends it with the agent's (Agent, Adapter) origin.

use crate::{InputProblem, read_bounded};
use actingcommand_contract::{EventId, FactObservation, MAX_FACT_OBSERVATION_BYTES};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientError};
use std::path::Path;

/// The observation in the file at `path`: at most `MAX_FACT_OBSERVATION_BYTES`, JSON the decoder
/// takes, and valid; otherwise why it cannot be used.
pub fn read_observation(path: &Path) -> Result<FactObservation, InputProblem> {
    let bytes = read_bounded(path, MAX_FACT_OBSERVATION_BYTES)?;
    let observation: FactObservation =
        serde_json::from_slice(&bytes).map_err(InputProblem::Json)?;
    observation.validate().map_err(InputProblem::Invalid)?;
    Ok(observation)
}

/// `facts publish`: `observation` published; the event that records it.
pub fn publish(
    client: &RuntimeClient,
    observation: FactObservation,
) -> Result<EventId, RuntimeClientError> {
    client.publish_facts(observation)
}
