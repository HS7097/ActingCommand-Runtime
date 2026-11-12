#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Logic tests of ci/gate_check.py, run by the `gate` job of .github/workflows/ci.yml.

    python3 ci/test_gate_check.py -v
"""

import argparse
import contextlib
import io
import json
import tempfile
import tomllib
import unittest
from pathlib import Path

import gate_check

ESC = "\x1b"
TABLE = {
    "compile_fail-0123456789abcdef.exe": ("sample", "compile_fail"),
    "sample-0123456789abcdef.exe": ("sample", "lib"),
}
SUMMARY_ONE = (
    "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s"
)


COMPILE_FAIL_SESSION = [
    "     Running tests\\compile_fail.rs (target\\debug\\deps\\compile_fail-0123456789abcdef.exe)",
    "",
    "running 1 test",
    "test sealed_type_cannot_be_constructed_outside_crate ... ok",
    "",
    SUMMARY_ONE,
    "",
]
SAMPLE_HEADER = "     Running unittests src\\lib.rs (target\\debug\\deps\\sample-0123456789abcdef.exe)"


def read(lines):
    """parse_log of a log of these lines: its binaries, its failures and where it stops."""
    with tempfile.TemporaryDirectory() as folder:
        log = Path(folder) / "job.log"
        log.write_text("\n".join(lines) + "\n", encoding="utf-8")
        return gate_check.parse_log(log, TABLE)


def parse(lines):
    """The binaries and failures of a log that does not stop in the middle."""
    binaries, failures, stop = read(lines)
    assert stop is None, stop
    return binaries, failures


def at(stamp, *lines):
    """The lines as the Test step tees them, each with the time stamp `stamp` (HH:mm:ss.fff)."""
    return [f"{stamp} {line}" for line in lines]


class AttributeTests(unittest.TestCase):
    def test_trybuild_line_after_a_session_summary_parses_and_is_not_attributed(self):
        # #382 CI, run 38122467672 (exec-core): trybuild's stderr line reached the merged log
        # after its compile_fail binary's `test result:`.
        binaries, failures = parse([
            "     Running tests\\compile_fail.rs (target\\debug\\deps\\compile_fail-0123456789abcdef.exe)",
            "",
            "running 1 test",
            "test sealed_type_cannot_be_constructed_outside_crate ... ok",
            "",
            SUMMARY_ONE,
            "",
            f"test {ESC}[0m{ESC}[1mtests/ui/sealed_construct.rs{ESC}[0m ... {ESC}[0m{ESC}[32mok",
            f"{ESC}[0m",
            "",
            "     Running unittests src\\lib.rs (target\\debug\\deps\\sample-0123456789abcdef.exe)",
            "",
            "running 1 test",
            "test tests::second_binary_test ... ok",
            "",
            SUMMARY_ONE,
        ])
        self.assertEqual(failures, [])
        self.assertEqual(
            [(binary.package, binary.target, binary.results) for binary in binaries],
            [
                ("sample", "compile_fail", {"sealed_type_cannot_be_constructed_outside_crate": "ok"}),
                ("sample", "lib", {"tests::second_binary_test": "ok"}),
            ],
        )

    def test_libtest_line_outside_any_session_still_fails(self):
        _, failures = parse([
            "     Running unittests src\\lib.rs (target\\debug\\deps\\sample-0123456789abcdef.exe)",
            "",
            "running 1 test",
            "test tests::first ... ok",
            "",
            SUMMARY_ONE,
            "",
            "test tests::stray ... ok",
        ])
        self.assertEqual(len(failures), 1, failures)
        self.assertTrue(failures[0].startswith("line 8: "), failures)
        self.assertIn("outside any session", failures[0])
        self.assertIn("test tests::stray ... ok", failures[0])

    def test_a_log_that_stops_in_a_multi_threaded_session_names_the_tests_still_running(self):
        # #71 FLK-13, run 43070595705 (host-lab): the Test step ran out of time. host-lab runs
        # libtest multi-threaded, which names a running test only in its 60-second notice.
        binaries, failures, stop = read(
            at("17:47:00.100", *COMPILE_FAIL_SESSION, SAMPLE_HEADER, "", "running 3 tests")
            + at(
                "17:48:01.000",
                "test tests::slow_but_finishes has been running for over 60 seconds",
                "test tests::hangs has been running for over 60 seconds",
            )
            + at("17:48:30.000", "test tests::slow_but_finishes ... ok", "test tests::quick ... ok")
        )
        self.assertEqual(failures, [])
        self.assertEqual(
            [(binary.package, binary.target, binary.results) for binary in binaries],
            [
                ("sample", "compile_fail", {"sealed_type_cannot_be_constructed_outside_crate": "ok"}),
                ("sample", "lib", {"tests::slow_but_finishes": "ok", "tests::quick": "ok"}),
            ],
        )
        self.assertEqual(
            stop,
            {
                "line": 14,
                "stamp": "17:48:30.000",
                "in": "sample::lib",
                "running": 3,
                "finished": 2,
                "unfinished": [
                    {
                        "test": "sample::lib::tests::hangs",
                        "line": 12,
                        "stamp": "17:48:01.000",
                        "seen": "running for over 60 seconds",
                    }
                ],
            },
        )

    def test_a_single_threaded_log_that_stops_in_a_test_names_the_test_it_started(self):
        # exec-core, runtime-client and the apps integration tests run --test-threads=1: libtest
        # writes `test <path> ... ` as the test starts and its result word when it ends.
        _, failures, stop = read(
            at("17:47:00.100", SAMPLE_HEADER, "", "running 2 tests", "test tests::first ... ok")
            + at("17:47:00.200", "test tests::stuck ... ")
        )
        self.assertEqual(failures, [])
        self.assertEqual(
            (stop["line"], stop["stamp"], stop["in"], stop["running"], stop["finished"]),
            (5, "17:47:00.200", "sample::lib", 2, 1),
        )
        self.assertEqual(
            stop["unfinished"],
            [{"test": "sample::lib::tests::stuck", "line": 5, "stamp": "17:47:00.200", "seen": "started"}],
        )

    def test_a_log_that_stops_after_a_running_line_names_that_binary(self):
        _, failures, stop = read(at("17:47:00.100", *COMPILE_FAIL_SESSION, SAMPLE_HEADER))
        self.assertEqual(failures, [])
        self.assertEqual(
            stop,
            {
                "line": 8,
                "stamp": "17:47:00.100",
                "in": "sample::lib",
                "running": None,
                "finished": None,
                "unfinished": [],
            },
        )

    def test_a_binary_that_crashed_is_no_time_out_and_still_fails(self):
        # cargo writes `error: test failed, to rerun pass …` after a binary that crashed: cargo
        # finished this log, and a session without its `test result:` stays a parse failure.
        _, failures, stop = read([
            SAMPLE_HEADER,
            "",
            "running 2 tests",
            "test tests::first ... ok",
            "error: test failed, to rerun pass `-p sample --lib`",
            "",
            "Caused by:",
            "  process didn't exit successfully: `target\\debug\\deps\\sample-0123456789abcdef.exe`"
            " (exit code: 0xc00000fd, STATUS_STACK_OVERFLOW)",
        ])
        self.assertIsNone(stop)
        self.assertEqual(len(failures), 1, failures)
        self.assertIn("no `running N tests` or no `test result:` line", failures[0])

    def test_attribute_writes_the_time_out_and_check_is_red_naming_the_unfinished_test(self):
        # #71 FLK-13: the check `rust` is red for a job whose Test step ran out of time, with a
        # message that names the job as timed out and the tests that had started and not finished.
        with tempfile.TemporaryDirectory() as folder:
            results = Path(folder) / "test-results-host-lab"
            results.mkdir()
            build = Path(folder) / "build.jsonl"
            build.write_text(
                json.dumps({
                    "reason": "compiler-artifact",
                    "package_id": "path+file:///C:/repo/crates/sample#0.1.0",
                    "target": {"kind": ["lib"], "name": "sample"},
                    "profile": {"test": True},
                    "executable": "C:\\repo\\target\\debug\\deps\\sample-0123456789abcdef.exe",
                })
                + "\n",
                encoding="utf-8",
            )
            log = results / "host-lab.log"
            log.write_text(
                "\n".join(
                    at("17:47:00.100", SAMPLE_HEADER, "", "running 2 tests")
                    + at("17:48:01.000", "test tests::hangs has been running for over 60 seconds")
                    + at("17:48:02.000", "test tests::quick ... ok")
                )
                + "\n",
                encoding="utf-8",
            )
            args = argparse.Namespace(
                job="host-lab", build=[str(build)], log=str(log), configuration=""
            )
            printed = io.StringIO()
            with contextlib.redirect_stdout(printed):
                gate_check.cmd_attribute(args)
            document = json.loads((results / "host-lab.results.json").read_text(encoding="utf-8"))
            violations = []
            index = gate_check.load_results(Path(folder), {"host-lab": {}}, violations)
        expected = (
            "job host-lab timed out: host-lab.log stops in the middle at line 5 (17:48:02.000), in "
            "sample::lib, where 1 of its 2 tests had finished; started and not finished: "
            "sample::lib::tests::hangs (running for over 60 seconds, line 4 at 17:48:01.000)"
        )
        self.assertIn(f"::error::{expected}", printed.getvalue().splitlines())
        self.assertEqual(document["timed_out"]["in"], "sample::lib")
        self.assertEqual(violations, [expected])
        self.assertEqual(index["host-lab"], {("sample", "lib", "tests::quick", ""): ["ok"]})


def gate_row(*tests):
    return {"key": "A3", "coverage": "Y", "incident": [], "tests": list(tests)}


RETIRED_ONE = """
[[retired]]
fragment = "contract"
test = "sample::lib::tests::retired_guard"
ruling = "a ruling"
reason = "why it goes"
covered_by = "what covers its risk"
"""


class RetirementTests(unittest.TestCase):
    def test_a_name_leaves_the_gate_map_only_when_the_retirement_list_holds_it(self):
        # Alice 10-09: a retirement needs its mapping table (PR #1322, Workflow #420 S5).
        base = {"contract": [gate_row("sample::lib::tests::kept", "sample::lib::tests::retired_guard")]}
        head = {"contract": [gate_row("sample::lib::tests::kept")]}

        problems = []
        listed = gate_check.load_retired(tomllib, RETIRED_ONE, "retired-gates.toml", problems)
        self.assertEqual(problems, [])
        self.assertEqual(gate_check.retirement_violations(base, head, listed, "base"), [])

        problems = []
        unlisted = gate_check.load_retired(tomllib, "", "retired-gates.toml", problems)
        self.assertEqual(problems, [])
        violations = gate_check.retirement_violations(base, head, unlisted, "base")
        self.assertEqual(len(violations), 1, violations)
        self.assertIn("contract.toml: test sample::lib::tests::retired_guard is at base", violations[0])


if __name__ == "__main__":
    unittest.main()
