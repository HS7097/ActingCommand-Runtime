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

Validate approved generic-domain concepts and every registered Runtime surface item:

```text
cargo run -p actingcommand-actinglab-architecture --bin generic-domain-guard -- --check
```

Print a pending item-level surface inventory for registry review:

```text
cargo run -p actingcommand-actinglab-architecture --bin generic-domain-guard -- --snapshot
```

Print exact AST and structured fragments that require an identity allowance review:

```text
cargo run -p actingcommand-actinglab-architecture --bin generic-domain-guard -- --identity-allowance-candidates
```

The v2 registry lives in `generic-domain-v2.toml`; its content-addressed item manifest lives in
`generic-domain-surfaces-v2.jsonl`. Rust public and wire items, `closed_code!` variants and wire
values, CLI/serde attributes, structured schema values, and protected text records receive stable
surface IDs. Snapshot output is inventory evidence only. It does not approve new concepts or
surfaces, and refreshing the manifest without a matching approved mapping is not an acceptance path.
Identity allowances bind one exact file and AST/structured selector to a fragment hash, purpose,
scope, and content-addressed approval. They cannot authorize another function or field in the same
file. Raw strings on the nine identity axes are rejected; typed identity comparisons remain valid.
Workspace package discovery reconciles every Cargo manifest under `apps/`, `benchmarks/`, `crates/`,
`providers/`, and `tools/` with the workspace member/exclude lists. Root files, `.github/`, scripts,
declared members, and protected data roots are itemized. An undeclared package or unknown protected
file type fails the guard instead of shrinking the scanned surface.

Validate exact files, hashes, provenance, and allowed scopes in the isolated compatibility zone:

```text
cargo run -p actingcommand-actinglab-architecture --bin external-compat-guard -- --check
```

The external-compat manifest does not authorize identities outside its registered data files.
Generated provenance records a fixed generator path, revision, file hash, typed parameters, and an
exact hashed input set; free-form shell commands are not part of the schema. Consumers receive bytes
only through `ExternalCompatReader`, which checks a typed capability before I/O and hashes the bytes
returned by the read.

The snapshot defines the A0 completion denominator. Its pipeline exemption table may only shrink after A0.

Verify approval records against the private Workflow repository and independently derive the
protected base-to-head surface delta from exact commits:

```text
cargo run -p actingcommand-actinglab-architecture --bin approval-provenance-guard -- --base <full-base-sha> --head <full-head-sha>
```

The command requires an authenticated `gh` session with read access to
`HS7097/ActingCommand-Workflow`. Missing authentication, API/network failure, a false comment ID,
author or timestamp drift, body-hash drift, and an out-of-scope surface delta all fail the gate.
The base and head must be full commit SHAs in one ancestry chain; both are inspected through
temporary detached worktrees so uncommitted candidate state cannot become evidence.

Approval lifecycle records are versioned. New approvals carry an immutable repository, pull
request, base, and subject binding while active and retain that binding after retirement.
Retirement only revokes authority: retired records cannot authorize later surface or allowance
changes and cannot be reactivated. The fixed historical comment/scope table is accepted only for
explicit one-time legacy retirement migration.

Verify an immutable exact-head marker and every changed Git object with the trusted PR gate:
Run the verifier from a trusted checkout with the candidate present only as Git objects:
```text
cargo run -p actingcommand-actinglab-architecture --bin trusted-provenance-guard -- --repository HS7097/ActingCommand-Runtime --base-ref main --base-protected true --base <full-base-sha> --head <full-head-sha> --pull-request <number> --trusted-verifier-sha <full-trusted-sha> --workflow-issue <number>
```

The marker binds repository, pull request, protected base, monotonic sequence, and exact final head.
Only the highest matching sequence is parsed in full. An unrelated malformed historical marker is
isolated, while a malformed, edited, missing, or conflicting selected marker fails the gate. Every
changed path must resolve to a `100644` blob in the approved head; deletions, renames, symlinks,
gitlinks, executable files, and mode/type changes are rejected.

The protected `pull_request_target` workflow builds the trusted verifier before fetching candidate
objects. It never checks out, builds, or executes candidate code. Configure
`ACTINGCOMMAND_WORKFLOW_READ_TOKEN` as a fine-grained read-only repository secret for the private
Workflow repository; an absent or unusable credential fails the job instead of skipping
verification. The marker proves an action by the configured GitHub account. It does not prove which
person controlled that account; independent human approval belongs to protected merge or
environment controls outside this verifier.
