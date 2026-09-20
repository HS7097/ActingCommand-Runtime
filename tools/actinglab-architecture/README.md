# ActingLab architecture guards

This development-only workspace library enforces architecture rules. It is not linked into the ActingLab runtime binary.

Run the existing guard suite:

```text
cargo test -p actingcommand-actinglab-architecture
```

The library derives the command inventory from the real ActingLab dispatch source. The existing workspace guards check it against `ratchet/actinglab_commands.json`, including the current pipeline exemptions.
