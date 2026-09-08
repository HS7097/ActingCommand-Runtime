// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract as ledger;
use actingcommand_device as device;

pub(super) fn convert(value: &device::VendorStdioFacts) -> ledger::VendorStdioFacts {
    ledger::VendorStdioFacts {
        process_id: value.process_id,
        process_created_filetime: fact(&value.process_created_filetime, |v| *v),
        started_filetime: value.started_filetime,
        steps: value
            .steps
            .iter()
            .map(|step| ledger::StdioStep {
                phase: phase(step.phase),
                api: api(step.api),
                target: reference(step.target),
                source: step.source.map(reference),
                completed_filetime: step.completed_filetime,
                returned: step.returned,
                error: step.error.as_ref().map(error),
                before: step.before.as_ref().map(reference_fact),
                after: step.after.as_ref().map(reference_fact),
                related: step.related.as_ref().map(reference_fact),
            })
            .collect(),
        dropped_count: value.dropped_count,
    }
}

fn fact<T, U>(value: &device::StdioFact<T>, convert: impl FnOnce(&T) -> U) -> ledger::StdioFact<U> {
    match value {
        device::StdioFact::Known(value) => ledger::StdioFact::Known(convert(value)),
        device::StdioFact::Unknown(reason) => ledger::StdioFact::Unknown(match reason {
            device::StdioUnknown::Borrowed => ledger::StdioUnknown::Borrowed,
            device::StdioUnknown::Invalid => ledger::StdioUnknown::Invalid,
            device::StdioUnknown::QueryFailed(value) => {
                ledger::StdioUnknown::QueryFailed(error(value))
            }
        }),
    }
}

fn error(value: &device::StdioNativeError) -> ledger::StdioNativeError {
    match value {
        device::StdioNativeError::Win32 { code } => ledger::StdioNativeError::Win32 { code: *code },
        device::StdioNativeError::Crt { errno, dos_errno } => ledger::StdioNativeError::Crt {
            errno: *errno,
            dos_errno: *dos_errno,
        },
        device::StdioNativeError::Io { code } => ledger::StdioNativeError::Io { code: *code },
    }
}

fn reference_fact(value: &device::StdioReferenceFact) -> ledger::StdioReferenceFact {
    ledger::StdioReferenceFact {
        reference: reference(value.reference),
        observed_filetime: value.observed_filetime,
        fd: value.fd,
        handle: fact(&value.handle, |v| *v),
        metadata_from: value.metadata_from.map(reference),
        flags: fact(&value.flags, |v| *v),
        file_identity: fact(&value.file_identity, |v| ledger::StdioFileIdentity {
            volume_serial: v.volume_serial,
            file_id: v.file_id,
        }),
    }
}

fn reference(value: device::StdioReference) -> ledger::StdioReference {
    match value {
        device::StdioReference::Stdout => ledger::StdioReference::Stdout,
        device::StdioReference::Stderr => ledger::StdioReference::Stderr,
        device::StdioReference::SavedStdout => ledger::StdioReference::SavedStdout,
        device::StdioReference::SavedStderr => ledger::StdioReference::SavedStderr,
        device::StdioReference::CaptureStdout => ledger::StdioReference::CaptureStdout,
        device::StdioReference::CaptureStderr => ledger::StdioReference::CaptureStderr,
        device::StdioReference::Win32Stdout => ledger::StdioReference::Win32Stdout,
        device::StdioReference::Win32Stderr => ledger::StdioReference::Win32Stderr,
        device::StdioReference::All => ledger::StdioReference::All,
    }
}

fn phase(value: device::StdioPhase) -> ledger::StdioPhase {
    match value {
        device::StdioPhase::Acquire => ledger::StdioPhase::Acquire,
        device::StdioPhase::Install => ledger::StdioPhase::Install,
        device::StdioPhase::Restore => ledger::StdioPhase::Restore,
        device::StdioPhase::Close => ledger::StdioPhase::Close,
        device::StdioPhase::AcquisitionCleanup => ledger::StdioPhase::AcquisitionCleanup,
    }
}

fn api(value: device::StdioApi) -> ledger::StdioApi {
    match value {
        device::StdioApi::Dup => ledger::StdioApi::Dup,
        device::StdioApi::Dup2 => ledger::StdioApi::Dup2,
        device::StdioApi::Open => ledger::StdioApi::Open,
        device::StdioApi::GetStdHandle => ledger::StdioApi::GetStdHandle,
        device::StdioApi::GetOsfhandle => ledger::StdioApi::GetOsfhandle,
        device::StdioApi::SetStdHandle => ledger::StdioApi::SetStdHandle,
        device::StdioApi::Flush => ledger::StdioApi::Flush,
        device::StdioApi::Close => ledger::StdioApi::Close,
        device::StdioApi::Unlink => ledger::StdioApi::Unlink,
    }
}
