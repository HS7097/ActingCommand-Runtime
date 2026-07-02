// SPDX-License-Identifier: AGPL-3.0-only

//! JSON-line recording and replay for device input actions.
//!
//! Replay executes against an explicit `InputBackend`; it does not select a
//! fallback backend or retry failed actions.

use serde::{Deserialize, Serialize};

use crate::{DeviceError, DeviceResult, InputBackend};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedInputEvent {
    pub action: RecordedInputAction,
}

impl RecordedInputEvent {
    pub fn new(action: RecordedInputAction) -> Self {
        Self { action }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RecordedInputAction {
    Tap {
        x: i32,
        y: i32,
    },
    LongTap {
        x: i32,
        y: i32,
        duration_ms: u64,
    },
    Swipe {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        duration_ms: u64,
    },
    Key {
        key: String,
    },
    Text {
        text: String,
    },
    Reset,
}

impl RecordedInputAction {
    pub fn action_type(&self) -> &'static str {
        match self {
            Self::Tap { .. } => "tap",
            Self::LongTap { .. } => "long_tap",
            Self::Swipe { .. } => "swipe",
            Self::Key { .. } => "key",
            Self::Text { .. } => "text",
            Self::Reset => "reset",
        }
    }

    fn execute(&self, backend: &mut impl InputBackend) -> DeviceResult<()> {
        match self {
            Self::Tap { x, y } => backend.tap(*x, *y),
            Self::LongTap { x, y, duration_ms } => backend.long_tap(*x, *y, *duration_ms),
            Self::Swipe {
                x1,
                y1,
                x2,
                y2,
                duration_ms,
            } => backend.swipe(*x1, *y1, *x2, *y2, *duration_ms),
            Self::Key { key } => backend.key(key),
            Self::Text { text } => backend.text(text),
            Self::Reset => backend.reset(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayReport {
    pub actions_replayed: usize,
    pub action_types: Vec<String>,
}

pub fn parse_replay_json_lines(input: &str) -> DeviceResult<Vec<RecordedInputEvent>> {
    let mut records = Vec::new();

    for (index, line) in input.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let record = serde_json::from_str::<RecordedInputEvent>(trimmed).map_err(|err| {
            DeviceError::fatal(format!("invalid replay JSON line {}: {err}", index + 1))
        })?;
        records.push(record);
    }

    if records.is_empty() {
        return Err(DeviceError::fatal("replay JSON-lines stream is empty"));
    }

    Ok(records)
}

pub fn write_replay_json_lines(records: &[RecordedInputEvent]) -> DeviceResult<String> {
    if records.is_empty() {
        return Err(DeviceError::fatal(
            "cannot write an empty replay action stream",
        ));
    }

    let mut output = String::new();
    for record in records {
        let line = serde_json::to_string(record)
            .map_err(|err| DeviceError::fatal(format!("failed to encode replay event: {err}")))?;
        output.push_str(&line);
        output.push('\n');
    }
    Ok(output)
}

pub fn replay_input_records(
    records: &[RecordedInputEvent],
    backend: &mut impl InputBackend,
) -> DeviceResult<ReplayReport> {
    if records.is_empty() {
        return Err(DeviceError::fatal("cannot replay an empty action stream"));
    }

    let mut action_types = Vec::with_capacity(records.len());
    for (index, record) in records.iter().enumerate() {
        let action_type = record.action.action_type();
        record.action.execute(backend).map_err(|err| {
            DeviceError::fatal(format!(
                "replay action {} ({action_type}) failed: {err}",
                index + 1
            ))
        })?;
        action_types.push(action_type.to_owned());
    }

    Ok(ReplayReport {
        actions_replayed: action_types.len(),
        action_types,
    })
}

pub struct RecordingInputBackend<B> {
    inner: B,
    records: Vec<RecordedInputEvent>,
}

impl<B> RecordingInputBackend<B> {
    pub fn new(inner: B) -> Self {
        Self {
            inner,
            records: Vec::new(),
        }
    }

    pub fn records(&self) -> &[RecordedInputEvent] {
        &self.records
    }

    pub fn into_inner(self) -> B {
        self.inner
    }

    pub fn to_json_lines(&self) -> DeviceResult<String> {
        write_replay_json_lines(&self.records)
    }

    fn record(&mut self, action: RecordedInputAction) {
        self.records.push(RecordedInputEvent::new(action));
    }
}

impl<B: InputBackend> InputBackend for RecordingInputBackend<B> {
    fn tap(&mut self, x: i32, y: i32) -> DeviceResult<()> {
        self.inner.tap(x, y)?;
        self.record(RecordedInputAction::Tap { x, y });
        Ok(())
    }

    fn long_tap(&mut self, x: i32, y: i32, duration_ms: u64) -> DeviceResult<()> {
        self.inner.long_tap(x, y, duration_ms)?;
        self.record(RecordedInputAction::LongTap { x, y, duration_ms });
        Ok(())
    }

    fn swipe(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, duration_ms: u64) -> DeviceResult<()> {
        self.inner.swipe(x1, y1, x2, y2, duration_ms)?;
        self.record(RecordedInputAction::Swipe {
            x1,
            y1,
            x2,
            y2,
            duration_ms,
        });
        Ok(())
    }

    fn key(&mut self, key: &str) -> DeviceResult<()> {
        self.inner.key(key)?;
        self.record(RecordedInputAction::Key {
            key: key.to_owned(),
        });
        Ok(())
    }

    fn text(&mut self, text: &str) -> DeviceResult<()> {
        self.inner.text(text)?;
        self.record(RecordedInputAction::Text {
            text: text.to_owned(),
        });
        Ok(())
    }

    fn reset(&mut self) -> DeviceResult<()> {
        self.inner.reset()?;
        self.record(RecordedInputAction::Reset);
        Ok(())
    }

    fn close(&mut self) -> DeviceResult<()> {
        self.inner.close()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DeviceErrorSeverity;

    #[derive(Default)]
    struct FakeInputBackend {
        action_types: Vec<String>,
        fail_on: Option<&'static str>,
    }

    impl FakeInputBackend {
        fn record(&mut self, action_type: &'static str) -> DeviceResult<()> {
            if self.fail_on == Some(action_type) {
                return Err(DeviceError::transient(format!(
                    "{action_type} transport failed"
                )));
            }
            self.action_types.push(action_type.to_owned());
            Ok(())
        }
    }

    impl InputBackend for FakeInputBackend {
        fn tap(&mut self, _x: i32, _y: i32) -> DeviceResult<()> {
            self.record("tap")
        }

        fn long_tap(&mut self, _x: i32, _y: i32, _duration_ms: u64) -> DeviceResult<()> {
            self.record("long_tap")
        }

        fn swipe(
            &mut self,
            _x1: i32,
            _y1: i32,
            _x2: i32,
            _y2: i32,
            _duration_ms: u64,
        ) -> DeviceResult<()> {
            self.record("swipe")
        }

        fn key(&mut self, _key: &str) -> DeviceResult<()> {
            self.record("key")
        }

        fn text(&mut self, _text: &str) -> DeviceResult<()> {
            self.record("text")
        }

        fn reset(&mut self) -> DeviceResult<()> {
            self.record("reset")
        }

        fn close(&mut self) -> DeviceResult<()> {
            self.record("close")
        }
    }

    fn sample_json_lines() -> String {
        [
            r#"{"action":{"type":"tap","x":10,"y":20}}"#,
            r#"{"action":{"type":"long_tap","x":30,"y":40,"duration_ms":500}}"#,
            r#"{"action":{"type":"swipe","x1":1,"y1":2,"x2":3,"y2":4,"duration_ms":250}}"#,
            r#"{"action":{"type":"key","key":"BACK"}}"#,
            r#"{"action":{"type":"text","text":"hello"}}"#,
            r#"{"action":{"type":"reset"}}"#,
        ]
        .join("\n")
    }

    #[test]
    fn replay_reproduces_recorded_action_types() {
        let records = parse_replay_json_lines(&sample_json_lines()).expect("valid replay stream");
        let mut backend = FakeInputBackend::default();

        let report = replay_input_records(&records, &mut backend).expect("replay succeeds");

        let expected = vec![
            "tap".to_owned(),
            "long_tap".to_owned(),
            "swipe".to_owned(),
            "key".to_owned(),
            "text".to_owned(),
            "reset".to_owned(),
        ];
        assert_eq!(report.actions_replayed, expected.len());
        assert_eq!(report.action_types, expected);
        assert_eq!(backend.action_types, expected);
    }

    #[test]
    fn recording_backend_writes_json_lines() {
        let backend = FakeInputBackend::default();
        let mut recorder = RecordingInputBackend::new(backend);

        recorder.tap(10, 20).expect("tap records");
        recorder.swipe(1, 2, 3, 4, 250).expect("swipe records");

        let encoded = recorder.to_json_lines().expect("JSONL encoding succeeds");
        let decoded = parse_replay_json_lines(&encoded).expect("JSONL decoding succeeds");

        assert_eq!(
            decoded,
            vec![
                RecordedInputEvent::new(RecordedInputAction::Tap { x: 10, y: 20 }),
                RecordedInputEvent::new(RecordedInputAction::Swipe {
                    x1: 1,
                    y1: 2,
                    x2: 3,
                    y2: 4,
                    duration_ms: 250,
                }),
            ]
        );
    }

    #[test]
    fn replay_rejects_malformed_json_line() {
        let valid = parse_replay_json_lines(r#"{"action":{"type":"tap","x":10,"y":20}}"#)
            .expect("first line is valid");
        assert_eq!(valid.len(), 1);

        let bad = parse_replay_json_lines(
            &[
                r#"{"action":{"type":"tap","x":10,"y":20}}"#,
                r#"{"action":{"type":"tap","x":10}}"#,
            ]
            .join("\n"),
        )
        .expect_err("second line must fail");

        assert_eq!(bad.severity(), DeviceErrorSeverity::Fatal);
        assert!(bad.message().contains("line 2"));
    }

    #[test]
    fn replay_backend_failure_is_fatal() {
        let records = parse_replay_json_lines(r#"{"action":{"type":"tap","x":10,"y":20}}"#)
            .expect("valid replay stream");
        let mut backend = FakeInputBackend {
            action_types: Vec::new(),
            fail_on: Some("tap"),
        };

        let err = replay_input_records(&records, &mut backend).expect_err("tap must fail");

        assert_eq!(err.severity(), DeviceErrorSeverity::Fatal);
        assert!(err.message().contains("tap"));
        assert!(err.message().contains("transport failed"));
    }
}
