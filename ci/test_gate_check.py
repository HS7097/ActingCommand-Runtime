#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Logic tests of ci/gate_check.py, run by the `gate` job of .github/workflows/ci.yml.

    python3 ci/test_gate_check.py -v
"""

import tempfile
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


def parse(lines):
    with tempfile.TemporaryDirectory() as folder:
        log = Path(folder) / "job.log"
        log.write_text("\n".join(lines) + "\n", encoding="utf-8")
        return gate_check.parse_log(log, TABLE)


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


if __name__ == "__main__":
    unittest.main()
