# A1 Golden Baseline Review

## Source

- Runtime behavior baseline: `f0a5b8536ac19f3df43dcae823c13466ad4d3a13`
- Frozen task specification SHA-256: `efb9e37f10807ce2a615205e3924021ad91eb073a54e4c65cd178e14b0aeab3b`
- Expectations are recorded from the production `actinglab` binary by an explicit maintainer command and checked in as static JSON.
- Normal test execution never regenerates expectations.

## Reviewed Surface

- 15 required command families, each with one success and one failure path.
- Canonical JSON equality and exact process exit code.
- Exactly one complete JSON envelope on stdout.
- No protocol envelope on stderr and no partial JSON on failures.
- Dynamic IDs, timestamps, absolute temporary paths, and environment-instance IDs are normalized.
- `schema_version`, `cli_version`, `runtime_version`, semantic identifiers, and confidence values are retained.
- Fixtures isolate config, app state, Session state, resource roots, run roots, package files, scenes, and the Lab-run ADB process.

## Intentional Failure Coverage

- missing required flags;
- missing scene or page data;
- target not visible;
- route not found;
- package hash mismatch;
- package validation failure;
- missing task or detector;
- missing env result;
- stale env detector version.
