# ActingLab CLI Exit-Code Baseline

Source baseline: `f0a5b8536ac19f3df43dcae823c13466ad4d3a13`; since Workflow #434 LAB-2b (0.12.49,
register §1.287, "Slice LAB-2b"; `contracts/lab-two-entries.md` §9.3) two tables apply.

## actinglab's command line (every line without `--via`)

Since LAB-2b actinglab's own commands print the CLI module's line (`src/cli_module/own_line.rs`),
and exit by the line's outcome category, as every operation of the CLI module does:

| Outcome category | Exit code | Lines |
| --- | ---: | --- |
| success, info, warning | 0 | every own command that succeeded (`lab_operation_completed`); a result's own warnings stay its member and do not change the category |
| error | 1 | every own command's refusal or failure, whatever its error class (`validation_failed`, `package_invalid`, `target_not_visible`, `device_error`, `runtime_not_running`, `lab_operation_check_failed`, the retired and not implemented paths) |
| fatal | 2 | the installation's refusal before the command runs, when its code is fatal (a refusal is never 0) |

`expected.json` and the `line` and `text` cases of `consumers.json` record these exit codes.

## MCP's actinglab child (a command line with `--via`) until LAB-5

A command line with `--via` (only MCP's actinglab child, `--json --via mcp …`) keeps the envelope
and the CLI adapter's mapping of the semantic error classes to exit codes:

| Error class | Exit code | Current stable codes include |
| --- | ---: | --- |
| success | 0 | successful envelope |
| usage or validation | 2 | `validation_failed`, `package_invalid` |
| safety blocked | 3 | `target_not_visible`, lease and path safety errors |
| device or instance | 4 | `device_error`, `instance_not_found` |
| runtime unavailable | 5 | `runtime_not_running`, `lab_operation_check_failed` |
| reserved or not implemented | 6 | reserved scheduler and operation paths |

The `envelope` cases of `consumers.json` record these exit codes. This table belongs to the CLI
adapter and is not part of the semantic contract crate; LAB-5 replaces the MCP child and with it
this table.
