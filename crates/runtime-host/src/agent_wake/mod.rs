// SPDX-License-Identifier: AGPL-3.0-only

//! The pure parts of the Runtime's messages to registered channels (Workflow #432 AGT-1a;
//! `contracts/agent-wake.md`): the message a channel receives ([`summary`]) and the body file it
//! points at ([`body`]), the rows of `host.agent_wake` ([`rows`], Workflow #432 AGT-1b), which
//! committed record wakes which channel ([`rules`]) unless a cool-down or a day cap holds it
//! ([`guards`], Workflow #432 AGT-2), and when a daily summary is due and what it holds ([`daily`],
//! Workflow #432 AGT-2b). No thread, no file and no send here; the worker (`host/agent_wake.rs`)
//! calls these.

pub(crate) mod body;
pub(crate) mod daily;
pub(crate) mod guards;
pub(crate) mod rows;
pub(crate) mod rules;
pub(crate) mod summary;
