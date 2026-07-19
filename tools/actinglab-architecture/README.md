# ActingLab architecture guards

This development-only workspace package enforces issue #33 architecture rules. It is not linked into the ActingLab runtime binary.

Run the focused guard suite:

```text
cargo test -p actingcommand-actinglab-architecture
```

Print the command inventory derived from the real ActingLab dispatch source:

```text
scripts/actinglab/command-inventory.ps1
```

Check the derived inventory against `ratchet/actinglab_commands.json`:

```text
scripts/actinglab/command-inventory.ps1 -Check
```

The snapshot defines the A0 completion denominator. Its pipeline exemption table may only shrink after A0.

Verify an immutable exact-head marker and every changed Git object from a trusted checkout:

```text
cargo run -p actingcommand-actinglab-architecture --bin trusted-provenance-guard -- --repository HS7097/ActingCommand-Runtime --base-ref main --base-protected true --base <full-base-sha> --head <full-head-sha> --pull-request <number> --trusted-verifier-sha <full-trusted-sha> --workflow-issue <number>
```

The marker binds the repository, pull request, protected base, monotonic sequence, and exact final
head. Only the highest matching sequence is parsed in full. Unrelated malformed historical markers
are isolated, while a malformed, edited, missing, or conflicting selected marker fails the gate.
Every changed path must resolve to a `100644` blob; deletions, renames, symlinks, gitlinks,
executable files, and mode/type changes are rejected.

The protected `pull_request_target` workflow builds the verifier before fetching candidate objects.
It never checks out, builds, or executes candidate code. Configure
`ACTINGCOMMAND_WORKFLOW_READ_TOKEN` as a fine-grained read-only repository secret for the private
Workflow repository; an absent or unusable credential fails the job. Each pull request body must
contain exactly one `Workflow-Issue: <number>` line. That untrusted value only selects the issue to
query; the trusted marker still binds the canonical repository, pull request, protected base, and
exact final head. The marker proves an action by the configured GitHub account, not which person
controlled that account. Independent human approval belongs to protected merge or environment
controls outside this verifier.
