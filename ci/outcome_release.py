#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Writes the release file contracts/outcome-codes/released/<X.Y.Z>.json of the outcome catalog
(Workflow #378 A8b, G10; compact form of Workflow #420 S3). The release PR runs it on the commit
it releases, from the workspace root:

    py -3 -I -B ci/outcome_release.py <X.Y.Z> [<workspace root>]

A release file holds only what binds every later catalog (outcome guard G9): the entries of the
release that are settled or retired, each with its signature (a code's category, ladder_class,
review, status and values {key: {type, required}}, `type` from the key table; a location's review
and status), and of those only the ones whose signature no earlier release file holds. The
release files together therefore hold every signature that any release bound, and G9 checks each
file as it checked a full snapshot. Its members are schema_version, release, codes and locations,
one entry per line, in name order.

It reads the header contracts/outcome-codes/catalog.json and the owner fragments it names, never
a merged file (none is committed, #420 S2). It never overwrites a release file and refuses a
release file of another schema. Standard library only. Exits 1 on any failure.
"""

import json
import re
import sys
from pathlib import Path

SCHEMA_VERSION = "actingcommand.outcome-codes.release.v2"
FRAGMENT_DIR = Path("contracts/outcome-codes")
HEADER_FILE = FRAGMENT_DIR / "catalog.json"
RELEASED_DIR = FRAGMENT_DIR / "released"
RELEASE = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")


def fail(message):
    print(f"ERROR outcome_release: {message}", file=sys.stderr)
    sys.exit(1)


def read_json(path):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        fail(f"{path}: {error}")


def code_signature(entry, keys):
    """A code's signature as a release file holds it."""
    values = {}
    for key, presence in entry["values"].items():
        if key not in keys:
            fail(f"value key {key} is not in the key table")
        values[key] = {"type": keys[key]["type"], "required": presence == "required"}
    return {
        "category": entry["category"],
        "ladder_class": entry["ladder_class"],
        "review": entry["review"],
        "status": entry["status"],
        "values": values,
    }


def location_signature(entry):
    """A location's signature as a release file holds it."""
    return {"review": entry["review"], "status": entry["status"]}


def binds(signature):
    """Whether G9 holds later catalogs to this signature: a settled or a retired entry."""
    return signature["review"] == "settled" or signature["status"] == "retired"


def catalog_signatures(root):
    """Every code's and location's signature in the catalog the fragments hold now."""
    header = read_json(root / HEADER_FILE)
    keys = header["keys"]
    codes, locations = {}, {}
    for owner in header["fragments"]:
        path = root / FRAGMENT_DIR / f"{owner}.json"
        if not path.is_file():
            continue
        fragment = read_json(path)
        for section, table, signature in (
            ("codes", codes, lambda entry: code_signature(entry, keys)),
            ("locations", locations, location_signature),
        ):
            for name, entry in fragment.get(section, {}).items():
                if name in codes or name in locations:
                    fail(f"{name} is registered twice")
                table[name] = signature(entry)
    return codes, locations


def held_signatures(root):
    """Every (section, name) signature that the existing release files hold."""
    held = {"codes": {}, "locations": {}}
    directory = root / RELEASED_DIR
    if not directory.is_dir():
        return held
    for path in sorted(directory.glob("*.json")):
        document = read_json(path)
        if document.get("schema_version") != SCHEMA_VERSION:
            fail(f"{path}: schema_version is not {SCHEMA_VERSION}")
        for section in ("codes", "locations"):
            for name, signature in document.get(section, {}).items():
                held[section].setdefault(name, []).append(signature)
    return held


def release_entries(codes, locations, held):
    """The binding entries whose signature no earlier release file holds, in name order."""
    sections = {}
    for section, table in (("codes", codes), ("locations", locations)):
        sections[section] = {
            name: signature
            for name, signature in sorted(table.items())
            if binds(signature) and signature not in held[section].get(name, [])
        }
    return sections


def render(release, sections):
    """The release file's text: one entry per line, LF line ends."""
    lines = [
        "{",
        f'  "schema_version": {json.dumps(SCHEMA_VERSION)},',
        f'  "release": {json.dumps(release)},',
    ]
    for index, section in enumerate(("codes", "locations")):
        comma = "," if index == 0 else ""
        entries = sections[section]
        if not entries:
            lines.append(f'  "{section}": {{}}{comma}')
            continue
        lines.append(f'  "{section}": {{')
        names = list(entries)
        for position, name in enumerate(names):
            entry = json.dumps(entries[name], ensure_ascii=False, separators=(", ", ": "))
            last = position == len(names) - 1
            lines.append(f"    {json.dumps(name, ensure_ascii=False)}: {entry}{'' if last else ','}")
        lines.append(f"  }}{comma}")
    lines.append("}")
    return "\n".join(lines) + "\n"


def main(arguments):
    if len(arguments) not in (1, 2) or not RELEASE.match(arguments[0]):
        fail("usage: py -3 -I -B ci/outcome_release.py <X.Y.Z> [<workspace root>]")
    release = arguments[0]
    root = Path(arguments[1]) if len(arguments) == 2 else Path(".")
    target = root / RELEASED_DIR / f"{release}.json"
    if target.exists():
        fail(f"{target} exists; a release file is never overwritten")
    codes, locations = catalog_signatures(root)
    sections = release_entries(codes, locations, held_signatures(root))
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(render(release, sections).encode("utf-8"))
    print(
        f"outcome_release: wrote {target}: {len(sections['codes'])} codes and "
        f"{len(sections['locations'])} locations new or changed among the settled and retired "
        f"entries ({len(codes)} codes, {len(locations)} locations in the catalog)"
    )


if __name__ == "__main__":
    main(sys.argv[1:])
