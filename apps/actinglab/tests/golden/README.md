# ActingLab Protocol Goldens

`expected.json` is a static protocol baseline. Normal tests execute the production `actinglab` binary against sealed synthetic fixtures and compare normalized canonical JSON plus the process exit code.

To intentionally re-record after a separately approved protocol change:

```powershell
scripts/actinglab/record-goldens.ps1
```

The recorder is not part of normal test execution. Review the complete diff in `expected.json` before committing it.
