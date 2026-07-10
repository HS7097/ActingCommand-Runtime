// SPDX-License-Identifier: AGPL-3.0-only

#[cfg(test)]
struct TestClock;

#[cfg(test)]
impl Clock for TestClock {
    fn now_unix_ms(&self) -> CliOutcome<u64> {
        Ok(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| CliError::device(format!("test clock failed: {error}")))?
            .as_millis() as u64)
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

#[cfg(test)]
static TEST_CLOCK: TestClock = TestClock;

#[cfg(test)]
impl LabRunContext<'static> {
    fn create(run_root: &Path, input_zip: &Path) -> CliOutcome<Self> {
        Self::create_with_context(
            run_root,
            input_zip,
            crate::LabRunProcessContext {
                current_dir: None,
                lease_root: run_root.join("locks"),
                os: "test".to_string(),
                runtime_commit: None,
                memory_source: crate::MemorySampleSource::fixed(crate::MemorySample {
                    total_bytes: 8 * 1024 * 1024 * 1024,
                    available_bytes: 4 * 1024 * 1024 * 1024,
                }),
            },
            &TEST_CLOCK,
        )
    }
}
