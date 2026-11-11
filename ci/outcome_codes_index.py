#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Writes, or checks, the index of the outcome code catalog (Workflow #439 CODES-IX): under
contracts/outcome-codes/index/, a summary README.md and one <owner>.md per owner fragment, each a
table of that owner's codes. The index is generated, never edited by hand. From the workspace
root:

    py -3 -I -B ci/outcome_codes_index.py [--check] [<workspace root>]

It reads the catalog as ci/outcome_release.py reads it, with that script's `owner_fragments`: the
header contracts/outcome-codes/catalog.json and the owner fragments it names, never a merged
file. Without --check it writes the index. With --check, which the `rust` check of
.github/workflows/ci.yml runs on every pull request, it writes nothing and exits 1 naming the
first file of the committed index that is not what it would write. The text depends on the
catalog alone: sorted, LF line ends, UTF-8 without BOM, no time and no commit. An entry it
cannot count (a status, review or category outside its closed set, a text member that is no
text) fails; other inconsistencies it notices are printed as NOTE lines on stderr. Standard
library only. Exits 1 on any failure.
"""

import importlib.util
import sys
import textwrap
from pathlib import Path


def _load_outcome_release():
    """ci/outcome_release.py beside this file, also when Python runs isolated (`-I` keeps the
    script's folder off sys.path)."""
    path = Path(__file__).resolve().with_name("outcome_release.py")
    spec = importlib.util.spec_from_file_location("outcome_release", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


outcome_release = _load_outcome_release()

INDEX_DIR = outcome_release.FRAGMENT_DIR / "index"
SUMMARY_FILE = "README.md"
SCRIPT = "ci/outcome_codes_index.py"
# What a pending entry holds in place of a text it has not written yet (the outcome guard's
# UNWRITTEN, tools/outcome-guard/src/catalog.rs).
UNWRITTEN = "TBD"
# The closed sets the index counts by, as the outcome guard's loader admits them.
STATUSES = ("active", "retired")
REVIEWS = ("settled", "pending")
CATEGORIES = ("success", "info", "warning", "error", "fatal")
DESCRIPTION_LIMIT = 160
ELLIPSIS = "…"
WIDTH = 100


def fail(message):
    print(f"ERROR outcome_codes_index: {message}", file=sys.stderr)
    sys.exit(1)


def note(message):
    print(f"NOTE outcome_codes_index: {message}", file=sys.stderr)


def is_written(entry, member):
    """A text as the outcome guard's G10 reads it: written unless missing, empty or TBD."""
    value = entry.get(member)
    return isinstance(value, str) and value.strip() not in ("", UNWRITTEN)


def text_written(entry):
    """Whether a code has both `common_causes` and `handling` written."""
    return is_written(entry, "common_causes") and is_written(entry, "handling")


def first_sentence(description):
    """The description's first sentence on one line. White space, newlines included, reads as
    one space; the sentence ends at the first `.`, `!` or `?` followed by a space or the end.
    Longer than DESCRIPTION_LIMIT characters, it is cut at the last space within the limit (at
    the limit when it has none) and ends in the ellipsis: a cut splits no word, so the index
    holds only words the catalog holds."""
    text = " ".join(description.split())
    for position, character in enumerate(text):
        if character in ".!?" and text[position + 1 : position + 2] in ("", " "):
            text = text[: position + 1]
            break
    if len(text) <= DESCRIPTION_LIMIT:
        return text
    cut = text[:DESCRIPTION_LIMIT]
    if text[DESCRIPTION_LIMIT] != " " and " " in cut:
        cut = cut[: cut.rindex(" ")]
    return cut.rstrip() + ELLIPSIS


def table_cell(text):
    """`text`, on one line, safe in a Markdown table cell: every `|` escaped, and a `<` outside
    a code span escaped, so that a renderer shows it instead of reading a tag."""
    parts = " ".join(text.split()).split("`")
    for index, part in enumerate(parts):
        in_code = index % 2 == 1 and index < len(parts) - 1
        part = part.replace("|", "\\|")
        parts[index] = part if in_code else part.replace("<", "\\<")
    return "`".join(parts)


def text_member(entry, member, place):
    value = entry.get(member)
    if not isinstance(value, str):
        fail(f"{place}: {member} is not a text")
    return value


def read_codes(root):
    """`(owners named by the header, {owner: {code: entry}}, number of locations)` for the
    owners that have a fragment file. Fails on an entry the index cannot count; notes the other
    inconsistencies it sees."""
    header, owned = outcome_release.owner_fragments(root)
    owners, locations = {}, 0
    for owner, fragment in owned:
        source = f"{outcome_release.FRAGMENT_DIR.as_posix()}/{owner}.json"
        if f"{owner}.md".lower() == SUMMARY_FILE.lower():
            fail(f"{source}: the owner's index file would be the summary {SUMMARY_FILE}")
        if fragment.get("owner") != owner:
            note(f"{source}: its owner member is {fragment.get('owner')!r}, not {owner}")
        codes = fragment.get("codes", {})
        if not isinstance(codes, dict):
            fail(f"{source}: codes is not an object")
        for name, entry in codes.items():
            place = f"{source}: code {name}"
            if not isinstance(entry, dict):
                fail(f"{place}: is not an object")
            for member, allowed in (
                ("status", STATUSES),
                ("review", REVIEWS),
                ("category", CATEGORIES),
            ):
                if text_member(entry, member, place) not in allowed:
                    fail(f"{place}: {member} {entry[member]} is none of {', '.join(allowed)}")
            for member in ("ladder_class", "since", "description"):
                text_member(entry, member, place)
            if entry.get("owner") != owner:
                note(f"{place}: its owner is {entry.get('owner')!r}, not {owner}")
        owners[owner] = codes
        locations += len(fragment.get("locations", {}))
    names = {name for codes in owners.values() for name in codes}
    for owner in sorted(owners):
        for name, entry in sorted(owners[owner].items()):
            replacement = entry.get("replaced_by")
            if replacement is not None and replacement not in names:
                note(f"code {name} of {owner}: replaced_by names no code: {replacement}")
            if entry["review"] == "settled" and not text_written(entry):
                note(
                    f"code {name} of {owner}: settled, but common_causes or handling is TBD or "
                    "empty"
                )
    return len(header["fragments"]), owners, locations


def plural(count, word):
    return f"{count} {word}" if count == 1 else f"{count} {word}s"


def paragraph(text):
    """`text` wrapped at WIDTH columns, never inside a code span."""
    parts = text.split("`")
    for index in range(1, len(parts), 2):
        parts[index] = parts[index].replace(" ", "\0")
    wrapped = textwrap.fill(
        "`".join(parts), WIDTH, break_long_words=False, break_on_hyphens=False
    )
    return wrapped.replace("\0", " ")


def table(heading, rows):
    """A Markdown table: `heading` the column names, the first column left and the others right
    aligned."""
    lines = [
        "| " + " | ".join(heading) + " |",
        "|---" + "|---:" * (len(heading) - 1) + "|",
    ]
    lines.extend("| " + " | ".join(str(cell) for cell in row) + " |" for row in rows)
    return "\n".join(lines)


def owner_counts(codes):
    """`(codes, active, active with text written, active and settled)` of one owner."""
    active = [entry for entry in codes.values() if entry["status"] == "active"]
    return (
        len(codes),
        len(active),
        sum(1 for entry in active if text_written(entry)),
        sum(1 for entry in active if entry["review"] == "settled"),
    )


def summary_page(owner_count, owners, locations):
    every = [entry for codes in owners.values() for entry in codes.values()]
    active = [entry for entry in every if entry["status"] == "active"]
    both = sum(1 for entry in active if text_written(entry))
    one = sum(
        1
        for entry in active
        if is_written(entry, "common_causes") != is_written(entry, "handling")
    )
    index = INDEX_DIR.as_posix()
    missing = owner_count - len(owners)
    about = paragraph(
        f"Generated by `{SCRIPT}` from the catalog header [`catalog.json`](../catalog.json) and "
        "the owner fragments it names; nobody edits it by hand. After a change to a fragment, run "
        f"`py -3 -I -B {SCRIPT}` from the workspace root and commit `{index}/`; the `rust` check "
        "of CI runs the script with `--check` and fails while the committed index is not what the "
        "script generates. The catalog's contract is "
        "[runtime-outcomes.md](../../runtime-outcomes.md); `actingctl codes` and the MCP tool "
        "`ac_outcome_code` look up one code. In the tables, *Codes* counts every code of a "
        "fragment, active and retired, and *Active* its active codes. *Text written* counts the "
        "active codes whose `common_causes` and `handling` are both written (neither empty nor "
        f"`{UNWRITTEN}`, as the outcome guard's G10 reads a text); the other active codes still "
        f"hold `{UNWRITTEN}`, or nothing, in one or both. *Settled* counts the active codes whose "
        "`review` is `settled`; the others are `pending`. Each owner's file lists every code of "
        "its fragment, retired ones included, sorted by code, with its category, ladder class, "
        "`since`, status (a retired code with the code that replaces it, where it names one), "
        f"review, text (`written` when both texts are written, else `{UNWRITTEN}`) and the first "
        f"sentence of its description, cut at a space within {DESCRIPTION_LIMIT} characters and "
        f"then ending in `{ELLIPSIS}`. The index holds codes only: the catalog's "
        f"{plural(locations, 'location')} {'is' if locations == 1 else 'are'} not in it, and "
        f"{missing} of the header's "
        f"{plural(owner_count, 'owner')} {'has' if missing == 1 else 'have'} no fragment file and "
        "so no file here."
    )
    owner_rows = [
        (f"[{owner}]({owner}.md)", *owner_counts(owners[owner])) for owner in sorted(owners)
    ]
    totals = [sum(row[column] for row in owner_rows) for column in range(1, 5)]
    sections = [
        "# Outcome code index",
        about,
        "## Codes by status",
        table(
            ("Status", "Codes"),
            [
                (status, sum(1 for entry in every if entry["status"] == status))
                for status in STATUSES
            ]
            + [("all", len(every))],
        ),
        "## Active codes by category",
        table(
            ("Category", "Codes"),
            [
                (category, sum(1 for entry in active if entry["category"] == category))
                for category in CATEGORIES
            ],
        ),
        "## Text and review of the active codes",
        table(
            ("Active codes", "Codes"),
            [
                ("`common_causes` and `handling` both written", both),
                (f"one of the two written, the other `{UNWRITTEN}` or empty", one),
                (f"neither written (`{UNWRITTEN}` or empty)", len(active) - both - one),
                ("`settled`", sum(1 for entry in active if entry["review"] == "settled")),
                ("`pending`", sum(1 for entry in active if entry["review"] == "pending")),
                ("all active codes", len(active)),
            ],
        ),
        "## By owner",
        table(
            ("Owner", "Codes", "Active", "Text written", "Settled"),
            owner_rows + [("all", *totals)],
        ),
    ]
    return "\n\n".join(sections) + "\n"


def status_cell(entry):
    if entry["status"] == "retired" and isinstance(entry.get("replaced_by"), str):
        return f"retired, replaced by `{table_cell(entry['replaced_by'])}`"
    return entry["status"]


def owner_page(owner, codes):
    about = paragraph(
        f"Generated by `{SCRIPT}` from the fragment [`{owner}.json`](../{owner}.json), one row per "
        "code, sorted by code; nobody edits it by hand. The [summary](README.md) says what each "
        f"column means. {plural(len(codes), 'code')}."
    )
    rows = [
        (
            f"`{name}`",
            codes[name]["category"],
            table_cell(codes[name]["ladder_class"]),
            table_cell(codes[name]["since"]),
            status_cell(codes[name]),
            codes[name]["review"],
            "written" if text_written(codes[name]) else UNWRITTEN,
            table_cell(first_sentence(codes[name]["description"])),
        )
        for name in sorted(codes)
    ]
    heading = (
        "Code", "Category", "Ladder class", "Since", "Status", "Review", "Text", "Description"
    )
    lines = [
        f"# Outcome codes of `{owner}`",
        "",
        about,
        "",
        "| " + " | ".join(heading) + " |",
        "|---" * len(heading) + "|",
    ]
    lines.extend("| " + " | ".join(row) + " |" for row in rows)
    return "\n".join(lines) + "\n"


def generate(root):
    """The index the catalog under `root` gives, as {file name: text}."""
    owner_count, owners, locations = read_codes(root)
    files = {SUMMARY_FILE: summary_page(owner_count, owners, locations)}
    for owner in sorted(owners):
        files[f"{owner}.md"] = owner_page(owner, owners[owner])
    return files


def differences(root, files):
    """How the committed index under `root` differs from `files`, in file name order, as
    `(file name, what differs)`."""
    directory = root / INDEX_DIR
    present = {path.name: path for path in directory.iterdir()} if directory.is_dir() else {}
    found = []
    for name in sorted(set(files) | set(present)):
        if name not in present:
            found.append((name, "is missing"))
        elif name not in files:
            found.append((name, "is no file of the index (no owner fragment of that name)"))
        elif (
            not present[name].is_file()
            or present[name].read_bytes() != files[name].encode("utf-8")
        ):
            found.append((name, "differs from what the script generates from the catalog"))
    return found


def main(arguments):
    check = "--check" in arguments
    rest = [argument for argument in arguments if argument != "--check"]
    if len(rest) > 1 or any(argument.startswith("-") for argument in rest):
        fail(f"usage: py -3 -I -B {SCRIPT} [--check] [<workspace root>]")
    root = Path(rest[0]) if rest else Path(".")
    files = generate(root)
    index = INDEX_DIR.as_posix()
    if check:
        found = differences(root, files)
        if found:
            name, what = found[0]
            more = f" (and {plural(len(found) - 1, 'other file')})" if len(found) > 1 else ""
            fail(
                f"{index}/{name} {what}{more}: the committed index is stale; run "
                f"py -3 -I -B {SCRIPT} from the workspace root and commit {index}/"
            )
        print(f"outcome_codes_index: {index} is current ({plural(len(files), 'file')})")
        return
    directory = root / INDEX_DIR
    directory.mkdir(parents=True, exist_ok=True)
    for name in sorted(files):
        (directory / name).write_bytes(files[name].encode("utf-8"))
    found = differences(root, files)
    if found:
        name, what = found[0]
        fail(f"{index}/{name} {what}; nothing writes it, remove it by hand")
    print(
        f"outcome_codes_index: wrote {plural(len(files), 'file')} under {index}: {SUMMARY_FILE} "
        f"and one file for each of {plural(len(files) - 1, 'owner fragment')}"
    )


if __name__ == "__main__":
    main(sys.argv[1:])
