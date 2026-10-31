// SPDX-License-Identifier: AGPL-3.0-only

//! The A/B install selection v2 and the slot document (Workflow #429 R-15b; frozen design
//! `rd-1031/design-r15b-ab-v4.1.md` §2.3 and §2.4; `contracts/install-cli.md`, "The A/B
//! layout").
//!
//! `install\active.json` names one selection: a slot and its three members, the Runtime, the
//! Tools of the same build and the UI. Unlike v1 it names no configuration, no provider and no
//! configuration generation: the one file configuration is `<root>\actingd.config.json`.
//! `<slot>\SLOT.json` records what `prepare` placed in the slot. Both are parsed and checked
//! here; R-15b has no consumer of either (R-15c switches the readers to v2), so nothing reads
//! or writes them yet.

use super::{InstallFileReference, InstallSlot, MAX_INSTALL_SELECTION_BYTES, canonical_sha256};
use crate::codes::RuntimeContractCode;
use crate::{RuntimeContractError, RuntimeContractResult};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// Interface anchor `install-selection` revision 2 (contracts/component-interfaces.md): the
// readers declare [2, 2] from R-15c on; raise the revision when a v2 reader can no longer read
// what the installer writes.
pub const INSTALL_SELECTION_V2_SCHEMA: &str = "actingcommand.install-selection.v2";
/// The slot document's schema (`install-root` revision 1, the installers' private files).
pub const INSTALL_SLOT_SCHEMA: &str = "actingcommand.install-slot.v1";
/// The slot document's name in its slot.
pub const INSTALL_SLOT_DOCUMENT_FILE: &str = "SLOT.json";
/// The bound of a slot document, 64 KiB.
pub const MAX_INSTALL_SLOT_DOCUMENT_BYTES: usize = 64 * 1024;
/// The environment variable with which the installer lets the actingd entry start the Runtime
/// while its own operation is unfinished: the operation id, which the entry checks against
/// `install\current.json` and removes from the child's environment (`contracts/stable-entry.md`).
pub const INSTALL_OPERATION_ENV: &str = "ACTINGCOMMAND_INSTALL_OPERATION";
/// The environment variable that keeps the graphical entry from showing a dialog or opening
/// Notepad: `1` (`contracts/stable-entry.md`). The entry removes it from the child's
/// environment.
pub const ENTRY_NO_DIALOG_ENV: &str = "ACTINGCOMMAND_ENTRY_NO_DIALOG";

/// The longest digit run of a selection or operation id (`sel-<ms>`, `op-<ms>`).
const MAX_ID_DIGITS: usize = 20;
/// The longest tag.
const MAX_TAG_CHARS: usize = 128;
/// The longest repository name (`owner/name`).
const MAX_REPOSITORY_BYTES: usize = 200;

fn refused(code: RuntimeContractCode) -> RuntimeContractError {
    RuntimeContractError::new(code)
}

/// Whether `value` is `prefix` followed by 1 to 20 ASCII digits.
fn is_numbered_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|digits| {
        !digits.is_empty()
            && digits.len() <= MAX_ID_DIGITS
            && digits.bytes().all(|byte| byte.is_ascii_digit())
    })
}

/// Whether `value` is a selection id: `sel-` and 1 to 20 digits.
pub fn is_selection_id(value: &str) -> bool {
    is_numbered_id(value, "sel-")
}

/// Whether `value` is an installer operation id: `op-` and 1 to 20 digits.
pub fn is_operation_id(value: &str) -> bool {
    is_numbered_id(value, "op-")
}

fn is_commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// `owner/name`, each part of `[A-Za-z0-9._-]`, not `.` or `..`.
fn is_repository(value: &str) -> bool {
    let part = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    };
    value.len() <= MAX_REPOSITORY_BYTES
        && value
            .split_once('/')
            .is_some_and(|(owner, name)| part(owner) && part(name))
}

/// No tag, or 1 to 128 visible ASCII characters.
fn is_tag(value: Option<&str>) -> bool {
    value.is_none_or(|tag| {
        !tag.is_empty()
            && tag.len() <= MAX_TAG_CHARS
            && tag.bytes().all(|byte| byte.is_ascii_graphic())
    })
}

/// One member of a selection: which build of which repository, and its manifest's SHA-256.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallMemberIdentity {
    /// `owner/name`, the manifest's `repository`.
    pub repository: String,
    /// The manifest's `commit_sha`: 40 lowercase hex digits.
    pub commit: String,
    /// The release tag of the kept build, `null` for a local build.
    pub tag: Option<String>,
    /// The lowercase, unprefixed SHA-256 of the member's `BUILD-MANIFEST.json`.
    pub manifest_sha256: String,
}

impl InstallMemberIdentity {
    /// `install_member_identity_invalid` when a member breaks its form.
    pub fn validate(&self) -> RuntimeContractResult<()> {
        if !is_repository(&self.repository)
            || !is_commit(&self.commit)
            || !is_tag(self.tag.as_deref())
            || !canonical_sha256(&self.manifest_sha256)
        {
            return Err(refused(RuntimeContractCode::InstallMemberIdentityInvalid));
        }
        Ok(())
    }
}

/// The three members of a selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallSelectionMembers {
    pub runtime: InstallMemberIdentity,
    pub tools: InstallMemberIdentity,
    pub ui: InstallMemberIdentity,
}

impl InstallSelectionMembers {
    /// Every member in its form, and the Runtime paired with the Tools of its own build
    /// (`install_member_pairing_mismatch`: the same repository and commit).
    pub fn validate(&self) -> RuntimeContractResult<()> {
        self.runtime.validate()?;
        self.tools.validate()?;
        self.ui.validate()?;
        if self.runtime.repository != self.tools.repository
            || self.runtime.commit != self.tools.commit
        {
            return Err(refused(RuntimeContractCode::InstallMemberPairingMismatch));
        }
        Ok(())
    }
}

/// `install\active.json`, `actingcommand.install-selection.v2` (design §2.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallSelectionV2 {
    pub schema_version: String,
    /// `sel-<ms>`, the name of the immutable copy `install\selections\<id>.json`.
    pub selection_id: String,
    /// 1 for a root's first selection, the baseline's plus 1 after it.
    pub generation: u64,
    pub slot: InstallSlot,
    /// `<slot>/SLOT.json` and its SHA-256.
    pub slot_document: InstallFileReference,
    pub members: InstallSelectionMembers,
    /// The operation that wrote it, `op-<ms>`.
    pub made_by: String,
}

impl InstallSelectionV2 {
    /// Every rule of the document on its own; a refusal names the first rule broken, in this
    /// order: the schema, the selection id and the generation and `made_by`
    /// (`install_selection_v2_invalid`), the slot document's reference
    /// (`invalid_install_file_reference`) and path (`install_slot_document_mismatch`), the
    /// members (`install_member_identity_invalid`, then `install_member_pairing_mismatch`).
    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.schema_version != INSTALL_SELECTION_V2_SCHEMA
            || !is_selection_id(&self.selection_id)
            || self.generation == 0
            || !is_operation_id(&self.made_by)
        {
            return Err(refused(RuntimeContractCode::InstallSelectionV2Invalid));
        }
        self.slot_document.validate()?;
        if self.slot_document.path != slot_document_path(self.slot) {
            return Err(refused(RuntimeContractCode::InstallSlotDocumentMismatch));
        }
        self.members.validate()
    }

    /// The selection a writer commits after `baseline` (none for a root's first): its
    /// generation is 1 then, else the baseline's plus 1 (`install_selection_v2_invalid`).
    pub fn validate_successor(&self, baseline: Option<&Self>) -> RuntimeContractResult<()> {
        self.validate()?;
        let expected = match baseline {
            None => Some(1),
            Some(baseline) => baseline.generation.checked_add(1),
        };
        if Some(self.generation) != expected {
            return Err(refused(RuntimeContractCode::InstallSelectionV2Invalid));
        }
        Ok(())
    }

    /// Parses and checks the bytes of a selection: 1 byte to 16 KiB
    /// (`install_selection_size_invalid`), JSON of exactly this shape
    /// (`install_selection_json_invalid`: an unknown, missing or mistyped member), then
    /// [`Self::validate`].
    pub fn from_json(bytes: &[u8]) -> RuntimeContractResult<Self> {
        if bytes.is_empty() || bytes.len() > MAX_INSTALL_SELECTION_BYTES {
            return Err(refused(RuntimeContractCode::InstallSelectionSizeInvalid));
        }
        let selection: Self = serde_json::from_slice(bytes)
            .map_err(|_| refused(RuntimeContractCode::InstallSelectionJsonInvalid))?;
        selection.validate()?;
        Ok(selection)
    }

    /// The selected slot's folder below `root`.
    pub fn slot_root(&self, root: &Path) -> PathBuf {
        root.join(self.slot.as_str())
    }
}

/// `<slot>/SLOT.json` of `slot`, as a selection references it.
pub fn slot_document_path(slot: InstallSlot) -> String {
    format!("{}/{INSTALL_SLOT_DOCUMENT_FILE}", slot.as_str())
}

/// One member as `prepare` placed it in a slot: its identity, the kept build it was copied from
/// and how many files and bytes it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallSlotMember {
    pub repository: String,
    pub commit: String,
    pub tag: Option<String>,
    pub manifest_sha256: String,
    /// `install/builds/<commit>/runtime` or `/tools`, or `install/ui-builds/<commit>/ui`.
    pub source: String,
    /// The files the member's manifest lists, at least 1.
    pub files: u64,
    pub bytes: u64,
}

impl InstallSlotMember {
    /// The member's identity, as a selection names it.
    pub fn identity(&self) -> InstallMemberIdentity {
        InstallMemberIdentity {
            repository: self.repository.clone(),
            commit: self.commit.clone(),
            tag: self.tag.clone(),
            manifest_sha256: self.manifest_sha256.clone(),
        }
    }

    /// The identity in its form, the source the kept build of its own commit, and at least one
    /// file (`install_member_identity_invalid`).
    fn validate(&self, builds: &str, member: &str) -> RuntimeContractResult<()> {
        self.identity().validate()?;
        if self.source != format!("install/{builds}/{}/{member}", self.commit) || self.files == 0 {
            return Err(refused(RuntimeContractCode::InstallMemberIdentityInvalid));
        }
        Ok(())
    }
}

/// The three members of a slot document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallSlotMembers {
    pub runtime: InstallSlotMember,
    pub tools: InstallSlotMember,
    pub ui: InstallSlotMember,
}

/// `<slot>\SLOT.json`, `actingcommand.install-slot.v1` (design §2.4): written once into the
/// slot's `.part` by `prepare` and never changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallSlotDocument {
    pub schema_version: String,
    pub slot: InstallSlot,
    /// The `prepare` operation that wrote it, `op-<ms>`.
    pub prepared_by: String,
    pub members: InstallSlotMembers,
}

impl InstallSlotDocument {
    /// The schema and `prepared_by` (`install_slot_document_invalid`), each member
    /// (`install_member_identity_invalid`) and the Runtime paired with the Tools of its build
    /// (`install_member_pairing_mismatch`).
    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.schema_version != INSTALL_SLOT_SCHEMA || !is_operation_id(&self.prepared_by) {
            return Err(refused(RuntimeContractCode::InstallSlotDocumentInvalid));
        }
        self.members.runtime.validate("builds", "runtime")?;
        self.members.tools.validate("builds", "tools")?;
        self.members.ui.validate("ui-builds", "ui")?;
        self.identities().validate()
    }

    /// Parses and checks the bytes of a slot document: 1 byte to 64 KiB and JSON of exactly
    /// this shape (`install_slot_document_invalid`), then [`Self::validate`].
    pub fn from_json(bytes: &[u8]) -> RuntimeContractResult<Self> {
        if bytes.is_empty() || bytes.len() > MAX_INSTALL_SLOT_DOCUMENT_BYTES {
            return Err(refused(RuntimeContractCode::InstallSlotDocumentInvalid));
        }
        let document: Self = serde_json::from_slice(bytes)
            .map_err(|_| refused(RuntimeContractCode::InstallSlotDocumentInvalid))?;
        document.validate()?;
        Ok(document)
    }

    /// The members' identities, as a selection of this slot names them.
    pub fn identities(&self) -> InstallSelectionMembers {
        InstallSelectionMembers {
            runtime: self.members.runtime.identity(),
            tools: self.members.tools.identity(),
            ui: self.members.ui.identity(),
        }
    }

    /// The reader's third step (design §2.3): the document's slot and its members equal the
    /// selection's (`install_slot_document_mismatch`). The SHA-256 of the document's bytes is
    /// the reader's own check against `selection.slot_document`.
    pub fn matches_selection(&self, selection: &InstallSelectionV2) -> RuntimeContractResult<()> {
        if self.slot != selection.slot || self.identities() != selection.members {
            return Err(refused(RuntimeContractCode::InstallSlotDocumentMismatch));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "gate_r15b_selection.rs"]
mod gate_r15b_selection;
