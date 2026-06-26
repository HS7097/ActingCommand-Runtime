// SPDX-License-Identifier: AGPL-3.0-only

use crate::DeviceResult;

pub trait InputBackend {
    fn tap(&mut self, x: i32, y: i32) -> DeviceResult<()>;

    fn long_tap(&mut self, x: i32, y: i32, duration_ms: u64) -> DeviceResult<()>;

    fn swipe(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, duration_ms: u64) -> DeviceResult<()>;

    fn key(&mut self, key: &str) -> DeviceResult<()>;

    fn text(&mut self, text: &str) -> DeviceResult<()>;

    fn reset(&mut self) -> DeviceResult<()>;

    fn close(&mut self) -> DeviceResult<()>;
}
