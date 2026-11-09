# ActingLab Protocol Goldens

`expected.json` is a static protocol baseline. Normal tests execute the production `actinglab` binary against sealed synthetic fixtures and compare normalized canonical JSON plus the process exit code.

To intentionally re-record after a separately approved protocol change:

```powershell
scripts/actinglab/record-goldens.ps1
```

The recorder is not part of normal test execution. Review the complete diff in `expected.json` before committing it.

`consumers.json` (Workflow #434 LAB-2 fix 2, `../consumer_goldens.rs`) holds the command lines actinglab's current consumers send, each with the exit code and envelope main `c76ce040` printed for it. It was written from main's code, not recorded; it has no recorder. `<ROOT>` is the fixture's folder, and `"<ANY>"` only requires the member to be there.
