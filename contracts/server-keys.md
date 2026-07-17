# Server Variant Keys

`server` is an opaque deployment-variant key supplied by an external project
definition, not a UI display suffix or Runtime enum. For example, a resource
catalog may provide `engine-a.region-1` without requiring a Runtime code change.

## Rules

- New keys should be lowercase ASCII.
- Prefer `<upstream>.<variant>` unless a future backend needs a more specific namespace.
- Do not persist display-only suffixes as server keys.
- Do not add narrow database `CHECK` constraints for server keys. Use `server_variants` as the catalog.
- Keep user-facing display labels separate from persisted server keys.
- Keep all project-specific keys in external resource data rather than Runtime defaults.
