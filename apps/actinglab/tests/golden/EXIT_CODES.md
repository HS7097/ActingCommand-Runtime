# ActingLab CLI Exit-Code Baseline

Source baseline: `f0a5b8536ac19f3df43dcae823c13466ad4d3a13`.

The current CLI adapter maps semantic error classes to process exit codes as follows:

| Error class | Exit code | Current stable codes include |
| --- | ---: | --- |
| success | 0 | successful envelope |
| usage or validation | 2 | `validation_failed`, `package_invalid` |
| safety blocked | 3 | `target_not_visible`, lease and path safety errors |
| device or instance | 4 | `device_error`, `instance_not_found` |
| runtime unavailable | 5 | `runtime_not_running` |
| reserved or not implemented | 6 | reserved scheduler and operation paths |

The golden matrix records the concrete exit code for every case. This table belongs to the CLI adapter and is not part of the semantic contract crate.
