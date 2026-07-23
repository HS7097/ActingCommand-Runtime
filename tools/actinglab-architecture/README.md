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

The sole v1 machine registry is `generic-domain-v1.toml`; it contains both approved concepts and
every `[[surface]]` mapping. Rust public and wire items, `closed_code!` variants and wire values,
CLI/serde attributes, structured schema values, and protected text records receive stable surface
IDs. Snapshot output is inventory evidence only. It does not approve new concepts or surfaces, and
refreshing the registry without a matching approved mapping is not an acceptance path. External or
hybrid surface tables are rejected.
Identity allowances bind one exact file and AST/structured selector to a fragment hash, purpose,
and scope. They cannot authorize another function or field in the same file and do not represent
Workflow approval or acceptance authority. Raw strings on the nine identity axes are rejected, including identity values
propagated into private helper parameters; typed identity comparisons remain valid. Cargo metadata is
the workspace membership authority. Every tracked member file and repository-wide structured product
file is classified automatically; tests, fixtures, goldens, root files, `.github/`, and scripts remain
inside the same itemized surface inventory. An undeclared package, shadow workspace, dynamic compile
input, or unknown protected file type fails the guard instead of shrinking the scanned surface.

Validate exact files, hashes, provenance, and allowed scopes in the isolated compatibility zone:

```text
cargo run -p actingcommand-actinglab-architecture --bin external-compat-guard -- --check
```

The external-compat manifest does not authorize identities outside its registered data files.
Upstream provenance is checked offline against a normalized repository URL, full commit SHA,
normalized upstream path, exact purpose/scope, and the registered raw-byte hash; it never fetches a
network object. Generated provenance records a fixed generator path, generator byte hash, full Git revision, stable
entrypoint, exact deterministic command, typed parameters, exact hashed inputs, and output hash.
Consumers receive bytes only through `ExternalCompatReader`, which checks a typed capability before
I/O and hashes the bytes returned by the verified file handle.
