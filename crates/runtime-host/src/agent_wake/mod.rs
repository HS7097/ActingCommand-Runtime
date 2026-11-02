// SPDX-License-Identifier: AGPL-3.0-only

//! The pure parts of the Runtime's messages to registered channels (Workflow #432 AGT-1a;
//! `contracts/agent-wake.md`): the message a channel receives ([`summary`]) and the body file it
//! points at ([`body`]). No thread, no file and no send here; AGT-1b's worker calls these.

pub(crate) mod body;
pub(crate) mod summary;
