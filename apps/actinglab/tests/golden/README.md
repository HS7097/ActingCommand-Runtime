# ActingLab Protocol Goldens

`expected.json` is a static protocol baseline. Normal tests execute the production `actinglab` binary against sealed synthetic fixtures and compare normalized canonical JSON plus the process exit code.

To intentionally re-record after a separately approved protocol change:

```powershell
scripts/actinglab/record-goldens.ps1
```

The recorder is not part of normal test execution. Review the complete diff in `expected.json` before committing it.

`consumers.json` (Workflow #434 LAB-2 fix 2, `../consumer_goldens.rs`) holds the command lines actinglab's current consumers send, each with its exit code and what it prints: `envelope` for MCP's `--json --via mcp …` child (main `c76ce040`'s envelope, unchanged until LAB-5), `line` for every other `--json` line and `text` for a line without `--json` into a redirected stdout (Workflow #434 LAB-2b: the CLI module's line and the text for people). It was written from the code, not recorded; it has no recorder. `<ROOT>` is the fixture's folder, and `"<ANY>"` only requires the member to be there.

Workflow #434 LAB-2b rewrote `expected.json` from the code's rule (every case runs without `--via`, so its `envelope` became the CLI module's `line`: the result's members beside `outcome` `lab_operation_completed`, or `{outcome, message, details}` with exit 1), not from a binary; the coordinator approved that rewrite in the slice's work order.
