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

Validate the approved generic-domain concepts and protected Runtime surfaces:

```text
cargo run -p actingcommand-actinglab-architecture --bin generic-domain-guard -- --check
```

Print a pending protected-surface inventory for registry review:

```text
cargo run -p actingcommand-actinglab-architecture --bin generic-domain-guard -- --snapshot
```

Snapshot output is inventory evidence only. It does not approve new concepts or surfaces.

Validate exact files, hashes, provenance, and allowed scopes in the isolated compatibility zone:

```text
cargo run -p actingcommand-actinglab-architecture --bin external-compat-guard -- --check
```

The external-compat manifest does not authorize identities outside its registered data files.

The snapshot defines the A0 completion denominator. Its pipeline exemption table may only shrink after A0.
