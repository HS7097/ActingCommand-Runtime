// SPDX-License-Identifier: AGPL-3.0-only
// Adapted from HS7097/codex-goal-lifecycle@c9538cc5 scripts/runtime/src/bridge/security.rs. Copyright (c) 2026 HS7097. MIT: see ../LICENSE-codex-goal-lifecycle.txt.

//! Which process owns a Claude session, and whose it is (Workflow #432;
//! `contracts/agent-forward.md`): the Claude Code process above a hook or a command, and whether a
//! process belongs to the current Windows user.

use actingcommand_host_metrics::{ProcessProbe, probe_process};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_FILES, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::{
    EqualSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// How many parents are looked at.
const MAX_ANCESTORS: usize = 32;

/// A process by id and creation time: an id alone can be reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProcessIdentity {
    pub(crate) pid: u32,
    pub(crate) created_at_unix_ms: u64,
}

fn identity_of(pid: u32) -> Result<ProcessIdentity, String> {
    match probe_process(pid) {
        ProcessProbe::Running { created_at_unix_ms } => Ok(ProcessIdentity {
            pid,
            created_at_unix_ms,
        }),
        ProcessProbe::NotRunning => Err(format!("process {pid} is not running")),
        ProcessProbe::Unknown(code) => Err(format!("cannot inspect process {pid}: {code}")),
    }
}

/// Every process now: id, parent id and executable name.
fn process_table() -> Result<BTreeMap<u32, (u32, String)>, String> {
    // SAFETY: the call takes no pointers.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(format!(
            "cannot list processes: {}",
            io::Error::last_os_error()
        ));
    }
    // SAFETY: the snapshot was just opened, is valid and has no other owner.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: the snapshot is valid; `entry` is valid for the call and carries its size.
    if unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } == 0 {
        return Err(format!(
            "cannot read the process list: {}",
            io::Error::last_os_error()
        ));
    }
    let mut table = BTreeMap::new();
    loop {
        let end = entry
            .szExeFile
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(entry.szExeFile.len());
        table.insert(
            entry.th32ProcessID,
            (
                entry.th32ParentProcessID,
                String::from_utf16_lossy(&entry.szExeFile[..end]),
            ),
        );
        // SAFETY: as for `Process32FirstW`.
        if unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
                return Err(format!("cannot finish the process list: {error}"));
            }
            return Ok(table);
        }
    }
}

/// The Claude Code process above this one, the owner of the session a hook or a command runs in.
/// It belongs to this Windows user and is older than this process.
pub(crate) fn claude_ancestor() -> Result<ProcessIdentity, String> {
    let table = process_table()?;
    let this = identity_of(std::process::id())?;
    let mut seen = BTreeSet::new();
    let mut pid = this.pid;
    for _ in 0..MAX_ANCESTORS {
        if !seen.insert(pid) {
            return Err("the process ancestry has a cycle".to_owned());
        }
        let Some((parent, _)) = table.get(&pid) else {
            break;
        };
        pid = *parent;
        let Some((_, name)) = table.get(&pid) else {
            break;
        };
        if name.eq_ignore_ascii_case("claude.exe") || name.eq_ignore_ascii_case("claude-code.exe") {
            let identity = identity_of(pid)?;
            if identity.created_at_unix_ms > this.created_at_unix_ms {
                return Err("the parent's process id was reused".to_owned());
            }
            return match same_windows_user(pid) {
                Ok(true) => Ok(identity),
                Ok(false) => Err("the Claude Code process belongs to another user".to_owned()),
                Err(error) => Err(format!(
                    "cannot read the Claude Code process' user: {error}"
                )),
            };
        }
    }
    Err("this process is not a child of a Claude Code process".to_owned())
}

/// The `TOKEN_USER` of a process, in a buffer aligned for it: the SID it points to is inside.
fn token_user(process: HANDLE) -> io::Result<Vec<usize>> {
    let mut token = null_mut();
    // SAFETY: `process` is valid for the call and the out pointer is valid.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the token was just opened, is valid and has no other owner.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut size = 0_u32;
    // SAFETY: a null buffer of length 0 asks for the size; the out pointer is valid.
    let first =
        unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut size) };
    if first != 0 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
        return Err(error);
    }
    let mut buffer = vec![0_usize; (size as usize).div_ceil(size_of::<usize>())];
    // SAFETY: the buffer holds at least `size` bytes, is aligned for TOKEN_USER and outlives the
    // call; the out pointer is valid.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            size,
            &mut size,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(buffer)
}

/// The SID of a buffer [`token_user`] filled.
fn sid(buffer: &[usize]) -> *mut core::ffi::c_void {
    // SAFETY: the buffer is aligned for TOKEN_USER and `GetTokenInformation` filled it with one.
    unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid }
}

/// Whether process `pid` runs as the same Windows user as this process.
pub(crate) fn same_windows_user(pid: u32) -> io::Result<bool> {
    // SAFETY: the call takes no pointers.
    let peer = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if peer.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the handle was just opened, is valid and has no other owner.
    let peer = unsafe { OwnedHandle::from_raw_handle(peer) };
    let peer = token_user(peer.as_raw_handle())?;
    // SAFETY: the pseudo handle of the current process needs no closing.
    let current = token_user(unsafe { GetCurrentProcess() })?;
    // SAFETY: both pointers point into live buffers holding valid SIDs.
    Ok(unsafe { EqualSid(sid(&current), sid(&peer)) } != 0)
}
