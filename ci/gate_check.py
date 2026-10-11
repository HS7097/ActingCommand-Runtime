#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Gate tooling of .github/workflows/ci.yml (test plan v2.2 §2.1, Batch 0b) and of the boot
smoke of .github/workflows/windows-remote-build.yml (REG-0, BOOT-SMOKE).

attribute (every module job, on the Windows runner)
    Attributes every libtest result line of one tee'd test log to (package, target,
    configuration) through the exe table of the job's own test build
    (`cargo test --no-run --message-format=json-render-diagnostics`), checks every test binary's
    count against libtest's `running N tests` and `test result:` lines, and writes
    `<log stem>.results.json` next to the log. Any parse failure exits 1 and writes nothing.

check (the `gate` job, check name `rust`, on ubuntu)
    Holds the gate map contracts/test-gates/<module>.toml against the attributed results of every
    module job: each map name runs exactly once per configuration its job runs it in, and passes.
    A name of ci/flaky-tests.toml is reported as "flaky, skipped". On pull requests the key count
    of each fragment must not fall below the base branch's. Exits 1 on any violation.

known-unbootable <path> (the `Boot smoke` step of the exact-SHA build, on the Windows runner)
    Validates ci/known-unbootable.toml, the machine-readable copy of Workflow #390 REG-0, and
    prints its rows on stdout as one JSON array. Any violation exits 1, naming the row and rule.

Standard library only; `check` and `known-unbootable` need Python 3.11+ (tomllib).
"""

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

RESULTS_FORMAT = "actingcommand-gate-results/1"
CI_WORKFLOW = Path(".github/workflows/ci.yml")
FLAKY_MIRROR = Path("ci/flaky-tests.toml")
GATE_MAP = Path("contracts/test-gates")
GATE_FRAGMENTS = ("contract", "ledger", "host", "policy", "exec", "apps")

# cargo colours its status lines (CARGO_TERM_COLOR=always); the job log shows ESC as `^[`.
ANSI = re.compile(r"(?:\x1b|\^\[)\[[0-9;]*[A-Za-z]")
# The Test step prefixes every line with `HH:mm:ss.fff `.
STAMP = re.compile(r"^\d{2}:\d{2}:\d{2}\.\d{3} ")
RUNNING = re.compile(r"^\s*Running (?:unittests )?\S.*\((?P<exe>[^()]+)\)\s*$")
DOCTESTS = re.compile(r"^\s*Doc-tests \S+\s*$")
COUNT = re.compile(r"^running (?P<count>\d+) tests?$")
RESULT = re.compile(r"^test (?P<name>.+?) \.\.\. (?P<result>ok|FAILED|ignored)(?:, .*)?$")
STARTED = re.compile(r"^test (?P<name>\S+(?: - should panic)?) \.\.\. ?(?P<rest>.*)$")
TOKEN = re.compile(r"^(?P<result>ok|FAILED|ignored)(?:, .*)?$")
SUMMARY = re.compile(
    r"^test result: (?:ok|FAILED)\. (?P<passed>\d+) passed; (?P<failed>\d+) failed; "
    r"(?P<ignored>\d+) ignored; (?P<measured>\d+) measured; (?P<filtered>\d+) filtered out;"
)
LIBTEST_PATH = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*$")
SHOULD_PANIC = " - should panic"
LIB_KINDS = {"lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"}


def fail(message):
    print(f"gate_check: {message}", file=sys.stderr)
    sys.exit(1)


# ---------------------------------------------------------------------------------------------
# attribute


def package_name(package_id):
    """The package name of a cargo package id, old (`name ver (source)`) or spec form."""
    if " " in package_id and not package_id.startswith(("path+", "registry+", "git+", "sparse+")):
        return package_id.split(" ", 1)[0]
    url, separator, fragment = package_id.rpartition("#")
    if not separator:
        raise ValueError(f"unreadable package id {package_id!r}")
    if "@" in fragment:
        return fragment.split("@", 1)[0]
    # `…/<name>#<version>`: the name is the last path segment when it equals the package name.
    return url.rstrip("/").rsplit("/", 1)[-1]


def target_label(target):
    kinds = target["kind"]
    if "bin" in kinds:
        return f"bin:{target['name']}"
    if "test" in kinds:
        return target["name"]
    if LIB_KINDS.intersection(kinds):
        return "lib"
    return f"{kinds[0]}:{target['name']}"


def exe_name(path):
    return re.split(r"[\\/]", path)[-1]


def exe_table(paths):
    """Test exe file name → (package, target), from the test builds' compiler-artifact lines."""
    table = {}
    for path in paths:
        with open(path, encoding="utf-8") as stream:
            for number, line in enumerate(stream, 1):
                line = line.strip()
                if not line:
                    continue
                try:
                    message = json.loads(line)
                except json.JSONDecodeError as error:
                    fail(f"{path}:{number} is not a cargo JSON message ({error}): {line[:200]}")
                if message.get("reason") != "compiler-artifact":
                    continue
                if not message.get("profile", {}).get("test") or not message.get("executable"):
                    continue
                try:
                    entry = (package_name(message["package_id"]), target_label(message["target"]))
                except (KeyError, ValueError) as error:
                    fail(f"{path}:{number}: {error}")
                name = exe_name(message["executable"])
                if table.setdefault(name, entry) != entry:
                    fail(f"{path}:{number}: {name} is both {table[name]} and {entry}")
    if not table:
        fail(f"no test executable in {', '.join(map(str, paths))}")
    return table


class Binary:
    def __init__(self, package, target, exe, line):
        self.package, self.target, self.exe, self.line = package, target, exe, line
        self.running = None
        self.summary = None
        self.results = {}
        self.pending = None
        self.foreign = 0
        self.nested = 0

    def label(self):
        if self.package is None:
            return f"the libtest session at line {self.line}"
        return f"{self.package}::{self.target} ({self.exe}, session at line {self.line})"


def libtest_name(name):
    if name.endswith(SHOULD_PANIC):
        name = name[: -len(SHOULD_PANIC)]
    return name if LIBTEST_PATH.match(name) else None


def parse_log(log, table):
    """Every test binary of one log with its attributed results; a list of failures.

    cargo prints its `Running` and `Doc-tests` headers on stderr and libtest prints each
    binary's session (`running N tests` … `test result:`) on stdout. The Test step merges the two
    streams, so a header can land among the lines of the session before it (run 38082835971).
    Each stream keeps its own order, and cargo runs every test binary before any doc test, so
    the k-th session belongs to the k-th `Running` header; the sessions after them are doc tests
    (one or two per `Doc-tests` header: rustdoc runs standalone and merged doc tests apart).
    """
    headers, sessions, failures = [], [], []
    session = None  # the open session, from its `running N tests` to its `test result:`
    lines = log.read_text(encoding="utf-8", errors="replace").splitlines()
    for number, raw in enumerate(lines, 1):
        line = STAMP.sub("", ANSI.sub("", raw), count=1).rstrip()
        running = RUNNING.match(line)
        if running:
            headers.append((exe_name(running["exe"].strip()), number))
            continue
        if DOCTESTS.match(line):
            headers.append((None, number))  # doc tests are no #[test] fn: not attributed
            continue
        count = COUNT.match(line)
        result = RESULT.match(line)
        summary = SUMMARY.match(line)
        started = None if result else STARTED.match(line)
        if session is None:
            if count:
                session = Binary(None, None, None, number)
                session.running = int(count["count"])
                sessions.append(session)
            elif result or summary or started:
                failures.append(f"line {number}: a libtest line outside any session: {line}")
            continue
        if session.pending is not None:
            # libtest wrote `test <path> ... ` and its result word follows as its next output:
            # single-threaded it runs the test in between, multi-threaded it writes both at once.
            # A child process sharing stdout can still write lines in between; they are not
            # libtest's own.
            token = TOKEN.match(line)
            if token:
                record(session, session.pending, token["result"], number, failures)
                session.pending = None
            elif count or result or summary or started:
                session.nested += 1
            continue
        if count:
            session.nested += 1  # a child run of the same exe (`*_child` entries)
        elif summary:
            session.summary = {key: int(value) for key, value in summary.groupdict().items()}
            session = None
        elif result:
            name = libtest_name(result["name"])
            if name is None:
                session.foreign += 1  # not a libtest path, e.g. trybuild's `test tests/ui/x.rs ... ok`
            else:
                record(session, name, result["result"], number, failures)
        elif started:
            name = libtest_name(started["name"])
            if name is None:
                session.foreign += 1
            else:
                session.pending = name
    tests = [header for header in headers if header[0] is not None]
    docs = len(headers) - len(tests)
    if any(exe is None for exe, _ in headers[: len(tests)]):
        failures.append("a Doc-tests header comes before a Running header")
        return [], failures
    if len(sessions) < len(tests) + docs or (docs == 0 and len(sessions) != len(tests)):
        failures.append(
            f"{len(tests)} Running and {docs} Doc-tests headers but {len(sessions)} libtest sessions"
        )
        return [], failures
    binaries = []
    for (exe, header_line), binary in zip(tests, sessions):
        if exe not in table:
            failures.append(f"line {header_line}: {exe} is missing from the test build table")
            continue
        binary.package, binary.target = table[exe]
        binary.exe = exe
        binaries.append(binary)
        failures.extend(check_binary(binary))
    return binaries, failures


def record(binary, name, result, number, failures):
    if name in binary.results:
        failures.append(f"line {number}: {name} has a second result in {binary.label()}")
        return
    binary.results[name] = result


def check_binary(binary):
    problems = []
    if binary.pending is not None:
        problems.append(f"{binary.label()}: {binary.pending} started and has no result")
    if binary.running is None or binary.summary is None:
        problems.append(f"{binary.label()}: no `running N tests` or no `test result:` line")
        return problems
    attributed = len(binary.results)
    summary = binary.summary
    total = summary["passed"] + summary["failed"] + summary["ignored"] + summary["measured"]
    if attributed != binary.running or attributed != total:
        problems.append(
            f"{binary.label()}: {attributed} attributed result lines, but libtest says "
            f"running {binary.running} and test result {total} (net of {summary['filtered']} "
            "filtered out)"
        )
    for status, key in (("ok", "passed"), ("FAILED", "failed"), ("ignored", "ignored")):
        seen = sum(1 for value in binary.results.values() if value == status)
        if seen != summary[key]:
            problems.append(
                f"{binary.label()}: {seen} `{status}` lines, but test result says {summary[key]} {key}"
            )
    return problems


def cmd_attribute(args):
    log = Path(args.log)
    table = exe_table([Path(path) for path in args.build])
    binaries, failures = parse_log(log, table)
    if not binaries:
        failures.append("the log has no Running line")
    print(f"{log.name} (configuration {args.configuration!r}):")
    for binary in binaries:
        summary = binary.summary or {}
        print(
            f"  {binary.package}::{binary.target}: attributed {len(binary.results)}, running "
            f"{binary.running}, passed {summary.get('passed')} failed {summary.get('failed')} "
            f"ignored {summary.get('ignored')} filtered out {summary.get('filtered')}; "
            f"foreign lines {binary.foreign}, child libtest lines {binary.nested}"
        )
    if failures:
        for failure in failures:
            print(f"parse failure: {failure}", file=sys.stderr)
        fail(f"{log.name}: {len(failures)} parse failure(s); no results written")
    out = log.with_name(log.stem + ".results.json")
    document = {
        "format": RESULTS_FORMAT,
        "job": args.job,
        "log": log.name,
        "configuration": args.configuration,
        "binaries": [
            {
                "package": binary.package,
                "target": binary.target,
                "exe": binary.exe,
                "running": binary.running,
                "summary": binary.summary,
                "tests": binary.results,
            }
            for binary in binaries
        ],
    }
    out.write_text(json.dumps(document, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {out.name}")


# ---------------------------------------------------------------------------------------------
# check


def ci_jobs(text):
    """Job id → one-line key → value, read like guard 1 (`gate_ci_jobs_cover_every_member`)."""
    jobs, in_jobs, current = {}, False, None
    for line in text.splitlines():
        content = line.lstrip()
        if not content or content.startswith("#"):
            continue
        indent = len(line) - len(content)
        if indent == 0:
            in_jobs, current = line.rstrip() == "jobs:", None
            continue
        if not in_jobs:
            continue
        if indent == 2:
            current = content.rstrip().removesuffix(":")
            jobs[current] = {}
        elif indent == 6 and current is not None and ":" in content:
            key, value = content.split(":", 1)
            value = value.strip()
            if len(value) >= 2 and value[0] == value[-1] and value[0] in "'\"":
                value = value[1:-1]
            jobs[current][key] = value
    return jobs


def module_jobs(jobs):
    modules = {}
    for job, keys in jobs.items():
        if "CI_PACKAGES" not in keys:
            continue
        tokens = keys["CI_PACKAGES"].split()
        if len(tokens) % 2 or any(flag != "-p" for flag in tokens[0::2]):
            fail(f"job {job}: CI_PACKAGES is not `-p <package> …`")
        features = keys.get("CI_FEATURES", "").split()
        if features and (len(features) != 2 or features[0] != "--features"):
            fail(f"job {job}: CI_FEATURES is not `--features <features>`")
        extra = None
        if "CI_EXTRA_FEATURES" in keys:
            extra = (keys["CI_EXTRA_FEATURES"].strip(), keys.get("CI_EXTRA_FILTER", "").split())
        modules[job] = {
            "packages": tokens[1::2],
            "features": features[1] if features else "",
            "extra": extra,
        }
    if not modules:
        fail(f"{CI_WORKFLOW} has no module job (no CI_PACKAGES line)")
    return modules


def expected_configurations(module, target, path):
    """D2: the job's features, plus its extra features for a lib test its filter selects."""
    runs = [module["features"]]
    extra = module["extra"]
    if extra and target == "lib" and any(token in path for token in extra[1]):
        runs.append(extra[0])
    return runs


def split_name(name):
    parts = name.split("::", 2)
    return tuple(parts) if len(parts) == 3 and all(parts) else None


def load_fragment(tomllib, text, label, violations):
    try:
        document = tomllib.loads(text)
    except tomllib.TOMLDecodeError as error:
        violations.append(f"{label} does not parse: {error}")
        return None
    rows = document.get("gate")
    if not isinstance(rows, list) or not all(isinstance(row, dict) for row in rows):
        violations.append(f"{label} holds no `[[gate]]` rows")
        return None
    return rows


def base_rows(tomllib, base, module, violations):
    path = f"{GATE_MAP.as_posix()}/{module}.toml"
    shown = subprocess.run(
        ["git", "show", f"{base}:{path}"], capture_output=True, text=True, check=False
    )
    if shown.returncode != 0:
        if "does not exist in" in shown.stderr or "exists on disk, but not in" in shown.stderr:
            return 0
        fail(f"git show {base}:{path} failed: {shown.stderr.strip()}")
    rows = load_fragment(tomllib, shown.stdout, f"{base}:{path}", violations)
    return 0 if rows is None else len(rows)


def load_results(results_dir, modules, violations):
    """Job → {(package, target, path, configuration): [result, …]} from the downloaded artifacts."""
    index = {}
    for job in sorted(modules):
        folder = results_dir / f"test-results-{job}"
        index[job] = {}
        if not folder.is_dir():
            violations.append(f"job {job}: no artifact test-results-{job}")
            continue
        logs = sorted(folder.glob("*.log"))
        documents = sorted(folder.glob("*.results.json"))
        if not documents:
            violations.append(f"job {job}: test-results-{job} holds no attributed results")
        for log in logs:
            if not (folder / (log.stem + ".results.json")).is_file():
                violations.append(f"job {job}: {log.name} has no attributed results")
        for document_path in documents:
            document = json.loads(document_path.read_text(encoding="utf-8"))
            if document.get("format") != RESULTS_FORMAT or document.get("job") != job:
                violations.append(f"job {job}: {document_path.name} is not {RESULTS_FORMAT} of {job}")
                continue
            configuration = document["configuration"]
            for binary in document["binaries"]:
                for path, result in binary["tests"].items():
                    key = (binary["package"], binary["target"], path, configuration)
                    index[job].setdefault(key, []).append(result)
    return index


def cmd_check(args):
    print(f"Python {sys.version.split()[0]}")
    try:
        import tomllib
    except ModuleNotFoundError:
        fail("tomllib is missing (Python 3.11+ is required)")
    violations = []

    # 1. Every fragment parses.
    fragments = {}
    for path in sorted(GATE_MAP.glob("*.toml")):
        if path.stem not in GATE_FRAGMENTS:
            violations.append(f"{path.as_posix()} is not one of the fragments {GATE_FRAGMENTS}")
            continue
        rows = load_fragment(tomllib, path.read_text(encoding="utf-8"), path.as_posix(), violations)
        if rows is not None:
            fragments[path.stem] = rows
    if not fragments:
        violations.append(f"{GATE_MAP.as_posix()} holds no fragment")

    modules = module_jobs(ci_jobs(CI_WORKFLOW.read_text(encoding="utf-8")))
    owner = {package: job for job, module in modules.items() for package in module["packages"]}
    mirror = {entry["name"] for entry in tomllib.loads(FLAKY_MIRROR.read_text(encoding="utf-8")).get("test", [])}

    # 3. Every module job uploaded attributed results.
    index = load_results(Path(args.results), modules, violations)

    # 2, 4, 5. Each map name: its job, its configurations, exactly one `ok` in each.
    checked = {job: [] for job in modules}
    flaky = []
    for module, rows in fragments.items():
        for row in rows:
            key = row.get("key")
            names = row.get("tests")
            if not isinstance(key, str) or not isinstance(names, list):
                violations.append(f"{module}.toml: a row has no string `key` or no `tests` list")
                continue
            for name in names:
                parts = split_name(name) if isinstance(name, str) else None
                if parts is None:
                    violations.append(f"{module} key {key}: {name!r} is not <package>::<target>::<path>")
                    continue
                package, target, path = parts
                job = owner.get(package)
                if job is None:
                    violations.append(
                        f"{module} key {key}: {name} belongs to no module job (`{package}` is in "
                        "no CI_PACKAGES line)"
                    )
                    continue
                if name in mirror:
                    flaky.append(f"{name} ({module} key {key}: effective coverage P this run)")
                    continue
                for configuration in expected_configurations(modules[job], target, path):
                    results = index[job].get((package, target, path, configuration), [])
                    shown = f"{name} [{configuration or 'default features'}]"
                    if results == ["ok"]:
                        checked[job].append(f"{shown} ok")
                    elif not results:
                        violations.append(
                            f"{module} key {key}: {shown} is absent from job {job} (cfg'd out, not "
                            "compiled or not selected)"
                        )
                    else:
                        violations.append(
                            f"{module} key {key}: {shown} in job {job} is {', '.join(results)}"
                        )

    # 6. On pull requests the key count of every fragment does not fall below the base's.
    counts = []
    if args.base:
        verified = subprocess.run(
            ["git", "rev-parse", "--verify", "--quiet", f"{args.base}^{{commit}}"],
            capture_output=True, text=True, check=False,
        )
        if verified.returncode != 0:
            fail(f"base {args.base} is not a commit in this checkout (fetch-depth: 0?)")
        for module in GATE_FRAGMENTS:
            head = len(fragments.get(module, []))
            base = base_rows(tomllib, args.base, module, violations)
            counts.append(f"{module}: head {head} keys, {args.base} {base} keys")
            if head < base:
                violations.append(
                    f"{module}.toml: {head} keys at the head, {base} at {args.base}; a key row "
                    "leaves only when its feature retires (test plan §2.1)"
                )

    for job, names in checked.items():
        print(f"::group::{job}: {len(names)} map names checked")
        for name in names:
            print(f"  {name}")
        print("::endgroup::")
    print(f"flaky, skipped ({len(flaky)}):")
    for name in flaky:
        print(f"  {name}")
    if counts:
        print("key counts, head vs base:")
        for line in counts:
            print(f"  {line}")
    if violations:
        print(f"{len(violations)} gate violation(s):", file=sys.stderr)
        for violation in violations:
            print(f"  {violation}", file=sys.stderr)
        sys.exit(1)
    print("gate map: every map name ran once per configuration and passed")


# ---------------------------------------------------------------------------------------------
# known-unbootable

UNBOOTABLE_KEYS = ("expected_code", "restored_by", "slice", "stage")
UNBOOTABLE_STAGES = ("config_read", "ledger_open", "takeover", "ready")
UNBOOTABLE_CODE = re.compile(r"\A[a-z][a-z0-9_]*\Z")


def cmd_known_unbootable(args):
    try:
        import tomllib
    except ModuleNotFoundError:
        fail("tomllib is missing (Python 3.11+ is required)")
    path = Path(args.path)
    try:
        document = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail(f"{path.as_posix()} cannot be read as TOML: {error}")
    violations = []
    for key in sorted(set(document) - {"unbootable"}):
        violations.append(f"{path.as_posix()}: `{key}`: the file holds only `[[unbootable]]` rows")
    rows = document.get("unbootable", [])
    if not isinstance(rows, list):
        violations.append(f"{path.as_posix()}: `unbootable` is not an array of tables")
        rows = []
    codes = {}
    for index, row in enumerate(rows, 1):
        at = f"{path.as_posix()} row {index}"
        if not isinstance(row, dict):
            violations.append(f"{at}: not a table")
            continue
        if tuple(sorted(row)) != UNBOOTABLE_KEYS:
            violations.append(f"{at}: keys {sorted(row)}, not exactly {list(UNBOOTABLE_KEYS)}")
            continue
        for key in UNBOOTABLE_KEYS:
            if not isinstance(row[key], str) or not row[key].strip():
                violations.append(f"{at}: `{key}` is not a non-empty string")
        if row["stage"] not in UNBOOTABLE_STAGES:
            violations.append(f"{at}: `stage` {row['stage']!r} is not one of {list(UNBOOTABLE_STAGES)}")
        code = row["expected_code"]
        if not isinstance(code, str) or not UNBOOTABLE_CODE.match(code):
            violations.append(f"{at}: `expected_code` {code!r} does not match ^[a-z][a-z0-9_]*$")
        elif code in codes:
            violations.append(f"{at}: `expected_code` {code!r} is also row {codes[code]}'s")
        else:
            codes[code] = index
    if violations:
        for violation in violations:
            print(violation, file=sys.stderr)
        fail(f"{path.as_posix()}: {len(violations)} violation(s)")
    print(json.dumps(rows, ensure_ascii=False))


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    commands = parser.add_subparsers(dest="command", required=True)
    attribute = commands.add_parser("attribute", help="attribute one tee'd test log")
    attribute.add_argument("--job", required=True)
    attribute.add_argument("--build", action="append", required=True, help="test build JSON")
    attribute.add_argument("--log", required=True)
    attribute.add_argument("--configuration", required=True, help="--features value; '' default")
    check = commands.add_parser("check", help="hold the gate map against the results")
    check.add_argument("--results", required=True, help="folder of test-results-<job> artifacts")
    check.add_argument("--base", help="origin/<base branch> on pull requests")
    unbootable = commands.add_parser("known-unbootable", help="validate and print the REG-0 copy")
    unbootable.add_argument("path")
    args = parser.parse_args()
    {
        "attribute": cmd_attribute,
        "check": cmd_check,
        "known-unbootable": cmd_known_unbootable,
    }[args.command](args)


if __name__ == "__main__":
    main()
