#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Logic tests of ci/outcome_codes_index.py (Workflow #439 CODES-IX), run by the `gate` job of
.github/workflows/ci.yml. They build the index of a small synthetic catalog in a temporary
folder.

    python3 ci/test_outcome_codes_index.py -v
"""

import contextlib
import importlib.util
import io
import json
import tempfile
import unittest
from pathlib import Path


def load(name):
    """A script of ci/ by its file, also when Python runs isolated (`-I`)."""
    path = Path(__file__).resolve().with_name(f"{name}.py")
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


index = load("outcome_codes_index")

LONG = "The " + " ".join(f"part{number:02d}" for number in range(1, 40)) + "."


def code(category, ladder_class, status, review, since, description, causes, handling, **rest):
    return {
        "category": category,
        "ladder_class": ladder_class,
        "owner": rest.pop("owner"),
        "layer": "runtime",
        "uncertain": False,
        "values": {},
        "review": review,
        "status": status,
        "since": since,
        **rest,
        "description": description,
        "common_causes": causes,
        "handling": handling,
    }


# The header names the owners out of order and a third owner without a fragment file; each
# fragment lists its codes out of order.
HEADER = {
    "schema_version": "actingcommand.outcome-codes.v1",
    "keys": {},
    "vocabularies": {},
    "domains": {},
    "fragments": {
        "second-owner": "crates/second-owner",
        "first-owner": "crates/first-owner",
        "third-owner": "crates/third-owner",
    },
}
FRAGMENTS = {
    "first-owner": {
        "owner": "first-owner",
        "codes": {
            "first_write_failed": code(
                "error", "environment_fault", "active", "settled", "0.12.3",
                "Writes `a|b <c>` to the file and stops. Then nothing more.",
                "The disk is full.", "Free some space.", owner="first-owner",
            ),
            "first_alpha_ready": code(
                "success", "none", "active", "pending", "0.12.1", LONG, "TBD", "TBD",
                owner="first-owner",
            ),
            "first_old_failed": code(
                "error", "none", "retired", "pending", "0.12.0", "Was: the old\nform. Gone.",
                "TBD", "TBD", owner="first-owner", retired_in="0.12.2",
                replaced_by="first_write_failed",
            ),
        },
        "locations": {
            "first_stage": {
                "owner": "first-owner",
                "layer": "runtime",
                "review": "pending",
                "status": "active",
                "since": "0.12.0",
                "description": "The first owner's stage.",
            },
        },
    },
    "second-owner": {
        "owner": "second-owner",
        "codes": {
            "second_value_invalid": code(
                "warning", "declaration_error", "active", "pending", "0.12.4",
                "A value breaks | its rule\n<name> here.", "A typing error.", "  TBD ",
                owner="second-owner",
            ),
        },
    },
}


def write_catalog(root):
    folder = root / "contracts" / "outcome-codes"
    folder.mkdir(parents=True)
    (folder / "catalog.json").write_text(json.dumps(HEADER, indent=2), encoding="utf-8")
    for owner, fragment in FRAGMENTS.items():
        (folder / f"{owner}.json").write_text(json.dumps(fragment, indent=2), encoding="utf-8")


def run(*arguments):
    """`main(arguments)`: (exit code, stdout, stderr)."""
    out, err = io.StringIO(), io.StringIO()
    status = 0
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        try:
            index.main(list(arguments))
        except SystemExit as stop:
            status = stop.code
    return status, out.getvalue(), err.getvalue()


class IndexTests(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.root = Path(self.folder.name)
        write_catalog(self.root)
        self.files = index.generate(self.root)

    def tearDown(self):
        self.folder.cleanup()

    def rows(self, name):
        return [line for line in self.files[name].splitlines() if line.startswith("| `")]

    def test_the_summary_counts_by_status_category_text_review_and_owner(self):
        self.assertEqual(sorted(self.files), ["README.md", "first-owner.md", "second-owner.md"])
        summary = self.files["README.md"]
        for line in [
            "| active | 3 |",
            "| retired | 1 |",
            "| all | 4 |",
            "| success | 1 |",
            "| info | 0 |",
            "| warning | 1 |",
            "| error | 1 |",
            "| fatal | 0 |",
            "| `common_causes` and `handling` both written | 1 |",
            "| one of the two written, the other `TBD` or empty | 1 |",
            "| neither written (`TBD` or empty) | 1 |",
            "| `settled` | 1 |",
            "| `pending` | 2 |",
            "| all active codes | 3 |",
            "| [first-owner](first-owner.md) | 3 | 2 | 1 | 1 |",
            "| [second-owner](second-owner.md) | 1 | 1 | 0 | 0 |",
            "| all | 4 | 3 | 1 | 1 |",
        ]:
            self.assertIn(line + "\n", summary)
        self.assertIn("1 location is not in it", " ".join(summary.split()))
        self.assertIn("1 of the header's 3 owners has no fragment file", " ".join(summary.split()))

    def test_owners_and_codes_are_sorted_and_a_retired_code_names_its_replacement(self):
        summary = self.files["README.md"]
        self.assertLess(summary.index("[first-owner]"), summary.index("[second-owner]"))
        self.assertEqual(
            [row.split(" | ")[0] for row in self.rows("first-owner.md")],
            ["| `first_alpha_ready`", "| `first_old_failed`", "| `first_write_failed`"],
        )
        self.assertIn(
            "| `first_old_failed` | error | none | 0.12.0 | retired, replaced by "
            "`first_write_failed` | pending | TBD | Was: the old form. |",
            self.rows("first-owner.md"),
        )

    def test_a_pipe_a_newline_and_a_tag_are_safe_in_a_table_cell(self):
        self.assertEqual(
            self.rows("second-owner.md"),
            [
                "| `second_value_invalid` | warning | declaration_error | 0.12.4 | active | pending "
                "| TBD | A value breaks \\| its rule \\<name> here. |"
            ],
        )
        written = [
            row for row in self.rows("first-owner.md") if row.startswith("| `first_write_failed` |")
        ]
        self.assertEqual(
            written,
            [
                "| `first_write_failed` | error | environment_fault | 0.12.3 | active | settled "
                "| written | Writes `a\\|b <c>` to the file and stops. |"
            ],
        )

    def test_a_description_is_cut_at_a_space_within_160_characters(self):
        kept = "The " + " ".join(f"part{number:02d}" for number in range(1, 23))
        self.assertEqual(len(kept), 157)
        self.assertEqual(index.first_sentence(LONG), kept + "…")
        self.assertTrue(self.rows("first-owner.md")[0].endswith(f"| {kept}… |"))
        exact = "y" * 150 + " " + "z" * 9
        self.assertEqual(len(exact), 160)
        self.assertEqual(index.first_sentence(exact), exact)
        self.assertEqual(index.first_sentence(exact + "z"), "y" * 150 + "…")
        self.assertEqual(index.first_sentence("q" * 200), "q" * 160 + "…")
        self.assertEqual(index.first_sentence("Stops here.Not here. Next."), "Stops here.Not here.")

    def test_check_fails_on_a_stale_index_and_passes_on_a_fresh_one(self):
        folder = self.root / "contracts" / "outcome-codes" / "index"

        status, _, err = run("--check", str(self.root))
        self.assertEqual(status, 1)
        self.assertIn("contracts/outcome-codes/index/README.md is missing (and 2 other files)", err)

        self.assertEqual(run(str(self.root))[0], 0)
        self.assertEqual(sorted(path.name for path in folder.iterdir()), sorted(self.files))
        for path in folder.iterdir():
            data = path.read_bytes()
            self.assertFalse(data.startswith(b"\xef\xbb\xbf"), path.name)
            self.assertNotIn(b"\r", data, path.name)
            self.assertEqual(data, self.files[path.name].encode("utf-8"), path.name)
        status, out, _ = run("--check", str(self.root))
        self.assertEqual(status, 0)
        self.assertIn("contracts/outcome-codes/index is current (3 files)", out)

        with (folder / "second-owner.md").open("a", encoding="utf-8", newline="\n") as file:
            file.write("| `hand_written` | error |\n")
        status, _, err = run("--check", str(self.root))
        self.assertEqual(status, 1)
        self.assertIn(
            "contracts/outcome-codes/index/second-owner.md differs from what the script "
            "generates from the catalog: the committed index is stale",
            err,
        )

        self.assertEqual(run(str(self.root))[0], 0)
        self.assertEqual(run("--check", str(self.root))[0], 0)
        (folder / "zeta-owner.md").write_bytes(b"# stale\n")
        status, _, err = run("--check", str(self.root))
        self.assertEqual(status, 1)
        self.assertIn("contracts/outcome-codes/index/zeta-owner.md is no file of the index", err)


if __name__ == "__main__":
    unittest.main()
