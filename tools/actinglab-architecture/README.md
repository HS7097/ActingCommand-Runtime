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
