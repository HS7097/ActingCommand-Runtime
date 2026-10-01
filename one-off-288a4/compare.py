# One-off (to be reverted), Workflow #288 A4 evidence: control (exact merge-base) against head.
import sys

CONTROL, HEAD, CONTROL_CLI, HEAD_CLI = sys.argv[1:5]
RETIRED = 'SourceTree { code: "source_tree_loader_retired" }'
GIT_REF = '"schema_version":"actingcommand.package.git-source-tree.v1"'


def lines(path, prefix):
    with open(path, encoding="utf-8") as handle:
        return [line.rstrip("\n") for line in handle if line.startswith(prefix)]


results = []


def check(name, passed, detail):
    results.append(passed)
    print(f"RESULT {name}: {'PASS' if passed else 'FAIL'} {detail}")


def same(prefix):
    control, head = lines(CONTROL, prefix), lines(HEAD, prefix)
    different = sum(1 for left, right in zip(control, head) if left != right)
    different += abs(len(control) - len(head))
    for left, right in zip(control, head):
        if left != right:
            print(f"DIFF control: {left}")
            print(f"DIFF head:    {right}")
    return control, head, different


# (a) recorded GitSourceTree references and PackageAdmitted facts, both builds.
control, head, different = same("A|")
for line in head:
    print(f"(a) head {line}")
equal = [line for line in control + head if "bytes_equal=true" in line]
check(
    "(a) GitSourceTree reference and fact decode/re-encode",
    different == 0 and len(control) == 20 and len(equal) == 40
    and all("variant=GitSourceTree validate_ok=true parse_argument_equal=true" in line
            for line in control + head if "|reference " in line),
    f"control_lines={len(control)} head_lines={len(head)} differing={different} byte_equal_lines={len(equal)}/40",
)

# (b) loading the GitSourceTree references.
control_git = [line for line in lines(CONTROL, "B|") if "|git|" in line and ("|admitted" in line or "|refused" in line)]
head_git = [line for line in lines(HEAD, "B|") if "|git|" in line and ("|admitted" in line or "|refused" in line)]
for line in control_git:
    print(f"(b) control {line}")
for line in head_git:
    print(f"(b) head {line}")
for line in lines(CONTROL, "B|") + lines(HEAD, "B|"):
    if "git_vs_content_directory" in line:
        print(f"(b) {line}")
control_compare = [line for line in lines(CONTROL, "B|") if "git_vs_content_directory" in line]
check(
    "(b) merge-base admits through the Git path",
    len(control_git) == 10 and all("|admitted " in line and GIT_REF in line for line in control_git)
    and len(control_compare) == 10 and all(line.endswith("equal=true") for line in control_compare),
    f"admitted_with_git_reference={sum(1 for line in control_git if '|admitted ' in line and GIT_REF in line)}/10 "
    f"git_equals_content_directory={sum(1 for line in control_compare if line.endswith('equal=true'))}/10",
)
check(
    "(b) head refuses with source_tree_loader_retired",
    len(head_git) == 10 and all(f"|refused {RETIRED}" in line for line in head_git),
    f"refused_retired={sum(1 for line in head_git if RETIRED in line)}/10",
)
control_cli = lines(CONTROL_CLI, "B|")
head_cli = lines(HEAD_CLI, "B|")
for line in control_cli + head_cli:
    print(f"(b) {line}")
head_exit = next((line for line in head_cli if " exit=" in line), "")
check(
    "(b) head actinglab observe exits non-zero with source_tree_loader_retired",
    bool(head_exit) and not head_exit.endswith("exit=0")
    and any("source_tree_loader_retired" in line for line in head_cli),
    head_exit,
)

# (c) ZIP and content-directory admission of the ten BA packs.
control, head, different = same("C|")
admitted = [line for line in head if "|admitted " in line]
documents = [line for line in head if "|doc " in line]
for line in head:
    if "|admitted " in line or "|dir reference=" in line:
        print(f"(c) head {line}")
check(
    "(c) ZIP and content-directory admission and document SHA-256 unchanged",
    different == 0 and len(control) == len(head) and len(admitted) == 20
    and not any("|refused" in line for line in control + head),
    f"lines control={len(control)} head={len(head)} differing={different} admitted={len(admitted)}/20 document_hashes={len(documents)}",
)

# (d) capabilities.
for line in lines(CONTROL_CLI, "D|") + lines(HEAD_CLI, "D|"):
    print(f"(d) {line}")
control_listed = any(line.endswith("git_source_tree_listed=True") for line in lines(CONTROL_CLI, "D|"))
head_listed = any(line.endswith("git_source_tree_listed=False") for line in lines(HEAD_CLI, "D|"))
head_schema = [line for line in lines(HEAD_CLI, "D|") if "schema package package_reference=" in line]
check(
    "(d) capabilities no longer list git_source_tree",
    control_listed and head_listed and head_schema and "git_source_tree" not in head_schema[0],
    f"control_listed={control_listed} head_not_listed={head_listed}",
)

if not all(results):
    print("RESULT overall: FAIL")
    sys.exit(1)
print("RESULT overall: PASS")
