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
