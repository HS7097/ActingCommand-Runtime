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
struct EmptyRuntimeCommitSource;

#[cfg(test)]
impl crate::RuntimeCommitSource for EmptyRuntimeCommitSource {
    fn sample(&self) -> Option<String> {
        None
    }
}
