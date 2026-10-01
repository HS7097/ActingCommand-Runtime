# SPDX-License-Identifier: AGPL-3.0-only
# One-off (to be reverted), Workflow #308 D2 evidence.
#
# usage: compare.py <merge-base actingledger> <head actingledger> <kept state roots>
# The kept roots are base-neutral, base-sealed (merge-base Runtime) and head-neutral,
# head-sealed, head-pack07 (head Runtime).
#   (a) v1 streams written by the merge-base: both readers, task-records (every record page),
#       stability and export, byte for byte.
#   (b) the v2 stream of the 0.7 pack run: color_digest and composite_member rows with their
#       values, and the head reader on all three paths; the merge-base reader as control.
#   (c) the same 0.6 (and the neutral 0.3) package on the merge-base and head Runtime: the task
#       diagnostic artifacts after masking run identities and clock values, line by line.

import difflib
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

BASE, HEAD, KEEP = sys.argv[1], sys.argv[2], Path(sys.argv[3])
V1 = "actingcommand.runtime.task-diagnostic.v1"
V2 = "actingcommand.runtime.task-diagnostic.v2"
STREAM = "actingcommand.runtime.task-diagnostic."
STABILITY_LIMIT = 16 * 1024
CONFIGURATION_LIMIT = 1_048_576
problems = []


def say(text):
    print("D2 " + text, flush=True)


def fail(text):
    problems.append(text)
    say("FAIL " + text)


def ledger(exe, root, args):
    process = subprocess.run([exe, "--state-root", str(root), *args], capture_output=True)
    return process.returncode, process.stdout, process.stderr.decode("utf-8", "replace").strip()


def sha(data):
    return hashlib.sha256(data).hexdigest()


def report_of(out):
    """The `data` of a machine report (`{"command": ..., "data": ...}`)."""
    return json.loads(out)["data"]


def stream_pages(report):
    return [
        page
        for page in report.get("diagnostics", [])
        if (page.get("schema_version") or "").startswith(STREAM)
    ]


def task_record_reads(exe, root):
    """Every record page of `export --task-evidence --include-private`, following the cursor."""
    reads = []
    cursor = None
    while True:
        args = ["export", "--task-evidence", "--include-private", "--record-limit", "64"]
        if cursor is not None:
            args += ["--record-cursor", json.dumps(cursor, separators=(",", ":"))]
        code, out, err = ledger(exe, root, args)
        reads.append((args, code, out, err))
        if code != 0 or not out.strip():
            return reads
        cursors = [page["next_cursor"] for page in stream_pages(report_of(out)) if page.get("next_cursor")]
        if not cursors:
            return reads
        cursor = cursors[0]
        if len(reads) > 64:
            fail(f"{root.name}: task-evidence paging did not end")
            return reads


def compare_readers(name):
    root = KEEP / name
    reads = [(args, (code, out, err)) for args, code, out, err in task_record_reads(BASE, root)]
    reads += [(args, ledger(BASE, root, args)) for args in (
        ["export", "--task-evidence"],
        ["export", "--stability"],
        ["export"],
    )]
    for args, (base_code, base_out, base_err) in reads:
        head_code, head_out, head_err = ledger(HEAD, root, args)
        same = (base_code, base_out, base_err) == (head_code, head_out, head_err)
        label = " ".join(arg if len(arg) < 40 else "<cursor>" for arg in args)
        say(
            f"(a) {name} [{label}]: merge-base exit {base_code} {len(base_out)} bytes sha256 {sha(base_out)}; "
            f"head exit {head_code} {len(head_out)} bytes sha256 {sha(head_out)}; identical={same}"
        )
        if base_err or head_err:
            say(f"(a) {name} [{label}] stderr merge-base: {base_err} | head: {head_err}")
        if args[:2] == ["export", "--task-evidence"] and base_out.strip():
            for page in report_of(base_out).get("diagnostics", []):
                say(
                    f"(a) {name} [{label}] page schema={page.get('schema_version')} state={page.get('state')} "
                    f"total_records={page.get('total_records')} records_in_page={len(page.get('records') or [])} "
                    f"next_cursor={'yes' if page.get('next_cursor') else 'no'}"
                )
        if not same:
            fail(f"(a) {name} [{label}] differs between the merge-base and head readers")


def records_of(reads, root_name):
    records = []
    for args, code, out, err in reads:
        if code != 0:
            fail(f"{root_name}: head task-evidence exit {code}: {err}")
            continue
        for page in stream_pages(report_of(out)):
            if page.get("schema_version") != V2 or page.get("state") != "verified":
                fail(f"{root_name}: stream page schema={page.get('schema_version')} state={page.get('state')}")
            records += page.get("records") or []
    return records


def evidence_v2(name, cells_expected):
    root = KEEP / name
    reads = task_record_reads(HEAD, root)
    first = report_of(reads[0][2]) if reads[0][1] == 0 and reads[0][2].strip() else {}
    pages = stream_pages(first)
    if len(pages) != 1:
        fail(f"(b) {name}: {len(pages)} task stream pages")
        return
    page0 = pages[0]
    artifact = page0["artifact"]
    say(
        f"(b) {name} head task-records: schema={page0['schema_version']} state={page0['state']} "
        f"total_records={page0['total_records']} pages_read={len(reads)} artifact_bytes={artifact['byte_count']} "
        f"(stability limit {STABILITY_LIMIT}, configuration limit {CONFIGURATION_LIMIT}) "
        f"header_schema={(page0.get('header') or {}).get('schema_version')}"
    )
    records = records_of(reads, name)
    by_index = {record["index"]: record for record in records}
    if len(records) != page0["total_records"] or sorted(by_index) != list(range(1, len(records) + 1)):
        fail(f"(b) {name}: read {len(records)} of {page0['total_records']} records")
    kinds = {}
    for record in records:
        kinds[record["kind"]] = kinds.get(record["kind"], 0) + 1
    say(f"(b) {name} rows by kind {json.dumps(kinds, sort_keys=True)}")
    if cells_expected is None:
        for record in records:
            source = record["data"].get("source") if record["kind"] == "target" else None
            if record["kind"] == "color_digest" or (source and "composite_target_id" in source):
                fail(f"(b) {name}: v2-only row in a run without 0.7 targets: {record}")
        return
    if artifact["byte_count"] <= STABILITY_LIMIT:
        fail(f"(b) {name}: artifact {artifact['byte_count']} bytes does not exceed the stability limit")

    def describe(target_record):
        data = target_record["data"]
        return f"#{target_record['index']} target {data['id']} kind={data['kind']} passed={data['passed']} source={json.dumps(data['source'], separators=(',', ':'))}"

    digest_rows = [record for record in records if record["kind"] == "color_digest"]
    member_rows = [
        record
        for record in records
        if record["kind"] == "target" and "composite_target_id" in record["data"]["source"]
    ]
    composites = [
        record
        for record in records
        if record["kind"] == "target" and record["data"]["kind"] == "composite"
    ]
    say(f"(b) {name}: {len(digest_rows)} color_digest rows, {len(composites)} composite target rows, {len(member_rows)} composite_member rows")
    for row in digest_rows:
        parent = by_index.get(row["parent_index"])
        ok = parent is not None and parent["kind"] == "target" and parent["data"]["kind"] == "color_digest"
        say(f"(b) {name} color_digest #{row['index']} parent {describe(parent) if parent else None} data={json.dumps(row['data'], separators=(',', ':'))}")
        if not ok:
            fail(f"(b) {name}: color_digest row #{row['index']} parent is not a color_digest target row")
    for composite in composites:
        page = by_index.get(composite["parent_index"])
        members = [row for row in member_rows if row["parent_index"] == composite["index"]]
        say(
            f"(b) {name} composite {describe(composite)} message={composite['data']['message']!r} "
            f"under page #{page['index'] if page else None} {page['data'].get('page_id') if page else None} "
            f"matched={page['data'].get('matched') if page else None}"
        )
        for member in members:
            source = member["data"]["source"]
            digest = [row for row in digest_rows if row["parent_index"] == member["index"]]
            say(
                f"(b) {name}   member {describe(member)} digest_children="
                + json.dumps([row["data"] for row in digest], separators=(",", ":"))
            )
            if source["composite_target_id"] != composite["data"]["id"]:
                fail(f"(b) {name}: member #{member['index']} names {source['composite_target_id']}")
        if [row["data"]["source"]["member_index"] for row in members] != list(range(len(members))) or [
            row["data"]["id"] for row in members
        ] != ["digest/home_cafe", "page/home"]:
            fail(f"(b) {name}: composite #{composite['index']} members out of declaration order")
        if page and page["data"].get("matched") is True:
            digest_member = [row for row in digest_rows if row["parent_index"] == members[0]["index"]] if members else []
            if not (
                composite["data"]["passed"]
                and all(member["data"]["passed"] for member in members)
                and len(digest_member) == 1
                and digest_member[0]["data"]["mean_milli"] == 0
                and digest_member[0]["data"]["max_cell"] == 0
                and digest_member[0]["data"]["observed_cells"] == cells_expected
            ):
                fail(f"(b) {name}: composite #{composite['index']} on the matched home page lacks the expected values")
    if not any(
        by_index.get(c["parent_index"], {}).get("data", {}).get("matched") is True for c in composites
    ):
        fail(f"(b) {name}: no composite evaluated on a matched home page")
    if not member_rows or not digest_rows:
        fail(f"(b) {name}: missing composite_member or color_digest rows")
    for label, args in (("stability", ["export", "--stability"]), ("export", ["export"])):
        code, out, err = ledger(HEAD, root, args)
        detail = ""
        if label == "stability" and out.strip():
            report = report_of(out)
            mentions = [f for f in report.get("failures", []) if f.get("artifact", {}).get("artifact_id") == artifact["artifact_id"]]
            detail = (
                f" scanned_diagnostic_count={report.get('scanned_diagnostic_count')} failures={len(report.get('failures', []))} "
                f"failures_on_task_stream={len(mentions)} gaps={report.get('gaps')} window_complete={report.get('window_complete')}"
            )
            if mentions:
                fail(f"(b) {name}: stability reports the task stream: {mentions}")
        if label == "export":
            text = out.decode("utf-8", "replace")
            detail = f" effective_configuration_section={'effective_configuration:' in text}"
        say(f"(b) {name} head {label}: exit {code} {len(out)} bytes{detail} stderr={err!r}")
        if code != 0:
            fail(f"(b) {name}: head {label} exit {code}: {err}")
    # Control: the merge-base reader on the same v2 root (contracts/task-diagnostic-stream.md).
    for label, args in (
        ("task-records", ["export", "--task-evidence", "--include-private", "--record-limit", "64"]),
        ("stability", ["export", "--stability"]),
        ("export", ["export"]),
    ):
        code, out, err = ledger(BASE, root, args)
        state = ""
        if label == "task-records" and out.strip():
            report = report_of(out)
            state = " pages=" + json.dumps(
                [[page.get("schema_version"), page.get("state")] for page in report.get("diagnostics", [])]
            ) + f" gaps={report.get('gaps')}"
        if label == "stability" and out.strip():
            report = report_of(out)
            state = " failures=" + json.dumps([[f.get("code"), f.get("artifact", {}).get("byte_count")] for f in report.get("failures", [])])
        say(f"(b) {name} control merge-base reader {label}: exit {code} {len(out)} bytes{state} stderr={err!r}")


ID = re.compile(r"\b([a-z]+)_([0-9a-f]{32})\b")
CLOCK = re.compile(r'"(monotonic_ms|started_monotonic_ms|ended_monotonic_ms|elapsed_ms|created_at_unix_ms)":\d+')
ARTIFACT_KIND = re.compile(r'"artifact":\{[^{}]*?"kind":"([^"]+)"')


def find_stream(root):
    found = []
    for path in sorted(root.rglob("*.json")):
        with open(path, "rb") as handle:
            if handle.read(64).startswith(b'{"schema_version":"' + STREAM.encode()):
                found.append(path)
    if len(found) != 1:
        fail(f"{root.name}: {len(found)} task diagnostic artifacts: {found}")
    return found[0] if found else None


def normalize(data):
    counters = {}
    names = {}

    def identity(match):
        value = match.group(0)
        if value not in names:
            counters[match.group(1)] = counters.get(match.group(1), 0) + 1
            names[value] = f"<{match.group(1)}#{counters[match.group(1)]}>"
        return names[value]

    masked = {}
    lines = []
    for line in data.decode("utf-8").split("\n"):
        line = CLOCK.sub(lambda m: f'"{m.group(1)}":<clock>', ID.sub(identity, line))
        if '"kind":"artifact"' in line:
            kind = ARTIFACT_KIND.search(line)
            kind = kind.group(1) if kind else "?"
            if kind != "capture.frame":
                masked[kind] = masked.get(kind, 0) + 1
                line = re.sub(r'"sha256":"sha256:[0-9a-f]{64}"', '"sha256":<sha256>', line)
                line = re.sub(r'"object_key":"[^"]*"', '"object_key":<key>', line)
                line = re.sub(r'"byte_count":\d+', '"byte_count":<bytes>', line)
        lines.append(line)
    return lines, masked


def compare_streams(base_name, head_name):
    base_path, head_path = find_stream(KEEP / base_name), find_stream(KEEP / head_name)
    if not base_path or not head_path:
        return
    base_bytes, head_bytes = base_path.read_bytes(), head_path.read_bytes()
    base_lines, base_masked = normalize(base_bytes)
    head_lines, head_masked = normalize(head_bytes)
    say(
        f"(c) {base_name} vs {head_name}: merge-base {len(base_bytes)} bytes {len(base_lines)} lines, "
        f"head {len(head_bytes)} bytes {len(head_lines)} lines; masked non-frame artifact rows "
        f"merge-base {json.dumps(base_masked, sort_keys=True)} head {json.dumps(head_masked, sort_keys=True)}"
    )
    differing = [
        index
        for index in range(max(len(base_lines), len(head_lines)))
        if index >= len(base_lines) or index >= len(head_lines) or base_lines[index] != head_lines[index]
    ]
    for line in difflib.unified_diff(base_lines, head_lines, "merge-base", "head", lineterm="", n=0):
        say(f"(c) {base_name} vs {head_name} diff: {line}")
    say(f"(c) {base_name} vs {head_name}: differing line numbers {[index + 1 for index in differing]}")
    expected = (
        differing == [0]
        and base_lines[0].startswith('{"schema_version":"' + V1 + '"')
        and base_lines[0].replace(V1, V2, 1) == head_lines[0]
    )
    if not expected:
        fail(f"(c) {base_name} vs {head_name}: differences beyond the header schema version")
    else:
        say(f"(c) {base_name} vs {head_name}: only the header schema_version differs ({V1} -> {V2})")


for name in ("base-neutral", "base-sealed"):
    compare_readers(name)
cells = (KEEP / "head-pack07" / "d2-declared-cells.txt").read_text().strip()
say(f"(b) declared digest/home_cafe cells {cells}")
evidence_v2("head-pack07", cells)
for name in ("head-sealed", "head-neutral"):
    evidence_v2(name, None)
for base_name, head_name in (("base-neutral", "head-neutral"), ("base-sealed", "head-sealed")):
    compare_streams(base_name, head_name)
say(f"summary: {len(problems)} problem(s)")
for problem in problems:
    say(f"problem: {problem}")
sys.exit(1 if problems else 0)
