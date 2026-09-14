// SPDX-License-Identifier: AGPL-3.0-only

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};

#[test]
fn actingledger_read_commands_are_thin_and_fail_loud() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_root = temp.path();
    fs::create_dir_all(state_root.join("ledger/segments")).expect("empty ledger segments");

    let args = [
        OsString::from("--state-root"),
        state_root.as_os_str().to_owned(),
        OsString::from("open"),
    ];
    let error = actingledger::run(args, &mut FailingWriter).expect_err("output failure");
    assert_eq!(error.code(), "output_failed");

    let main_source = include_str!("../src/main.rs");
    assert_eq!(main_source.matches("actingledger::run_env()").count(), 1);
    for forbidden in ["GlobalLedger", "EventQuery", "serde_json"] {
        assert!(!main_source.contains(forbidden));
    }
    let cli_source = include_str!("../src/lib.rs");
    assert_eq!(
        cli_source
            .matches("actingcommand_ledger_forensics::run(request)")
            .count(),
        1
    );
    for forbidden in ["GlobalLedger", "EventQuery", "PersistedEvent"] {
        assert!(!cli_source.contains(forbidden));
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;

        let invalid_utf8 = OsString::from_wide(&[0xd800]);
        let error = actingledger::run(
            [
                OsString::from("replay"),
                OsString::from("--zip"),
                invalid_utf8,
                OsString::from("--expected-sha256"),
                OsString::from("0".repeat(64)),
            ],
            &mut Vec::new(),
        )
        .expect_err("non-UTF-8 replay path");
        assert_eq!(error.code(), "invalid_arguments");
    }
}

struct FailingWriter;

impl Write for FailingWriter {
    fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed output"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
