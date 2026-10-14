// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #398 RD-2 (model-398 v1.1 §4.1, §4.3): the wired device link of a handset, watched
//! through one `adb track-devices` process of the installation's adb.
//!
//! - **Protocol.** `adb track-devices` writes one frame each time the adb server's device list
//!   changes: four hexadecimal digits giving the payload's length in bytes, then the payload, one
//!   line per device, `<serial>\t<state>\n`; an empty list is the frame `0000`. A
//!   [`TrackDevicesDecoder`] takes the output as it arrives and yields every whole frame as a
//!   [`TrackedDevices`] list; a frame cut by a read waits for its rest. A length that is not four
//!   hexadecimal digits, a payload that is not text, or a line that is not a serial, a tab and a
//!   state is [`TrackDevicesProtocolInvalid`]: nothing is guessed (§4.1).
//! - **States.** Only adb's state word decides, never a sentence (spec §7): `device`, `offline`,
//!   `unauthorized` and `no permissions…` have their own link state, every other word is `other`.
//!   A bound serial the list does not name is `absent`.
//! - **Serials.** A listed serial stays inside [`TrackedDevices`]: it is only compared with a
//!   bound [`DeviceSerial`], and a list's `Debug` shows its states, never a serial.
//! - **Process.** [`AdbTrackDevices`] spawns `adb track-devices`, reads its standard output and
//!   discards its standard error; [`TrackDevicesStopper`] ends it from another thread by killing
//!   the process, whose output then closes. It sends nothing to any device. The one command that
//!   acts on a link, `adb -s <serial> reconnect` of one transport, is [`Adb::reconnect`];
//!   nothing here runs `kill-server`, `reboot`, `usb` or `tcpip` (§4.3).

use crate::adb::{installation_adb_command, stop_child};
use crate::{Adb, DeviceError, DeviceResult, DeviceSerial};
use actingcommand_contract::DeviceLinkState;
use std::collections::VecDeque;
use std::fmt;
use std::io::{ErrorKind, Read};
use std::process::{Child, ChildStdout, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// The length prefix of one frame: four hexadecimal digits.
const LENGTH_DIGITS: usize = 4;
/// How long the stop waits for the killed process to exit.
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
/// The bytes one read of the process's output takes at most.
const READ_CHUNK: usize = 4096;

/// One device of a `track-devices` list: its serial (compared, never shown) and its state.
#[derive(Clone, PartialEq, Eq)]
struct TrackedDevice {
    serial: String,
    state: DeviceLinkState,
}

/// One whole `track-devices` list.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct TrackedDevices {
    devices: Vec<TrackedDevice>,
}

impl TrackedDevices {
    /// The link state of the bound `serial`: its listed state, or `absent` when the list does not
    /// name it. A serial without a raw value (an unbound handset's stand-in) is always `absent`.
    pub fn state_of(&self, serial: &DeviceSerial) -> DeviceLinkState {
        let raw = serial.adb_arg();
        if raw.is_empty() {
            return DeviceLinkState::Absent;
        }
        self.devices
            .iter()
            .find(|device| device.serial == raw)
            .map_or(DeviceLinkState::Absent, |device| device.state)
    }

    /// The number of listed devices.
    pub fn len(&self) -> usize {
        self.devices.len()
    }

    /// Whether the list names no device.
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    /// The listed states, in list order.
    pub fn states(&self) -> Vec<DeviceLinkState> {
        self.devices.iter().map(|device| device.state).collect()
    }
}

impl fmt::Debug for TrackedDevices {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TrackedDevices")
            .field("states", &self.states())
            .finish()
    }
}

/// The link state of adb's state word for one listed device (§4.2).
pub fn link_state_of_adb_state(word: &str) -> DeviceLinkState {
    match word {
        "device" => DeviceLinkState::Device,
        "offline" => DeviceLinkState::Offline,
        "unauthorized" => DeviceLinkState::Unauthorized,
        word if word.starts_with("no permissions") => DeviceLinkState::NoPermissions,
        _ => DeviceLinkState::Other,
    }
}

/// A `track-devices` output whose shape the decoder does not know (§4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackDevicesProtocolInvalid;

/// The incremental decoder of `adb track-devices` output.
#[derive(Debug, Default)]
pub struct TrackDevicesDecoder {
    pending: Vec<u8>,
}

impl TrackDevicesDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes the next bytes of the output and returns every whole list they complete, in
    /// order. Bytes of a frame not yet whole are kept for the next call. After an error the
    /// decoder is not used again.
    pub fn push(
        &mut self,
        bytes: &[u8],
    ) -> Result<Vec<TrackedDevices>, TrackDevicesProtocolInvalid> {
        self.pending.extend_from_slice(bytes);
        let mut lists = Vec::new();
        while self.pending.len() >= LENGTH_DIGITS {
            let length = frame_length(&self.pending[..LENGTH_DIGITS])?;
            let end = LENGTH_DIGITS
                .checked_add(length)
                .ok_or(TrackDevicesProtocolInvalid)?;
            if self.pending.len() < end {
                break;
            }
            let list = parse_list(&self.pending[LENGTH_DIGITS..end])?;
            self.pending.drain(..end);
            lists.push(list);
        }
        Ok(lists)
    }
}

/// The payload length of a frame: exactly four hexadecimal digits.
fn frame_length(digits: &[u8]) -> Result<usize, TrackDevicesProtocolInvalid> {
    if !digits.iter().all(u8::is_ascii_hexdigit) {
        return Err(TrackDevicesProtocolInvalid);
    }
    let text = std::str::from_utf8(digits).map_err(|_| TrackDevicesProtocolInvalid)?;
    usize::from_str_radix(text, 16).map_err(|_| TrackDevicesProtocolInvalid)
}

/// One frame's payload: lines `<serial>\t<state>\n`, the serial printable without white
/// space, the state printable (`no permissions` has spaces); nothing else.
fn parse_list(payload: &[u8]) -> Result<TrackedDevices, TrackDevicesProtocolInvalid> {
    let text = std::str::from_utf8(payload).map_err(|_| TrackDevicesProtocolInvalid)?;
    if text.is_empty() {
        return Ok(TrackedDevices::default());
    }
    let body = text.strip_suffix('\n').ok_or(TrackDevicesProtocolInvalid)?;
    let mut devices = Vec::new();
    for line in body.split('\n') {
        let (serial, state) = line.split_once('\t').ok_or(TrackDevicesProtocolInvalid)?;
        let serial_valid = !serial.is_empty() && serial.bytes().all(|byte| byte.is_ascii_graphic());
        let state_valid = !state.is_empty()
            && !state.starts_with(' ')
            && state
                .bytes()
                .all(|byte| byte.is_ascii_graphic() || byte == b' ');
        if !serial_valid || !state_valid {
            return Err(TrackDevicesProtocolInvalid);
        }
        devices.push(TrackedDevice {
            serial: serial.to_owned(),
            state: link_state_of_adb_state(state),
        });
    }
    Ok(TrackedDevices { devices })
}

/// One event of a running `adb track-devices` process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackDevicesEvent {
    /// The adb server's device list, whole.
    List(TrackedDevices),
    /// The process ended, or its output closed or could not be read.
    Ended,
    /// The output has a shape the decoder does not know.
    ProtocolInvalid,
}

/// Ends a running `adb track-devices` process from another thread.
#[derive(Debug)]
pub struct TrackDevicesStopper {
    child: Mutex<Option<Child>>,
}

impl TrackDevicesStopper {
    /// Kills the process and waits for it to exit; once only, later calls do nothing. Its
    /// output then closes, so a blocked [`AdbTrackDevices::next_event`] returns `Ended`. A kill
    /// that cannot be confirmed is the error.
    pub fn stop(&self) -> DeviceResult<()> {
        let child = self
            .child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        match child {
            Some(mut child) => {
                stop_child(&mut child, STOP_TIMEOUT, "adb_track_devices").map(|_| ())
            }
            None => Ok(()),
        }
    }
}

/// One running `adb track-devices` process and the decoder of its output.
pub struct AdbTrackDevices {
    stdout: ChildStdout,
    decoder: TrackDevicesDecoder,
    ready: VecDeque<TrackedDevices>,
    stopper: Arc<TrackDevicesStopper>,
}

impl AdbTrackDevices {
    /// Spawns `adb track-devices` with the adb at `adb_path`.
    pub fn spawn(adb_path: &str) -> DeviceResult<Self> {
        if adb_path.trim().is_empty() {
            return Err(DeviceError::fatal(
                "ADB path is unresolved; the device-link watch cannot start",
            ));
        }
        let mut child = installation_adb_command(adb_path)?
            .arg("track-devices")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                DeviceError::fatal(format!("failed to spawn adb track-devices: {error}"))
            })?;
        let Some(stdout) = child.stdout.take() else {
            let stopper = TrackDevicesStopper {
                child: Mutex::new(Some(child)),
            };
            stopper.stop()?;
            return Err(DeviceError::fatal(
                "failed to open adb track-devices stdout",
            ));
        };
        Ok(Self {
            stdout,
            decoder: TrackDevicesDecoder::new(),
            ready: VecDeque::new(),
            stopper: Arc::new(TrackDevicesStopper {
                child: Mutex::new(Some(child)),
            }),
        })
    }

    /// What ends this process from another thread.
    pub fn stopper(&self) -> Arc<TrackDevicesStopper> {
        Arc::clone(&self.stopper)
    }

    /// Blocks until the next whole list, the end of the output, or an output the decoder does
    /// not know. After `Ended` or `ProtocolInvalid` the process is over for its reader.
    pub fn next_event(&mut self) -> TrackDevicesEvent {
        let mut buffer = [0_u8; READ_CHUNK];
        loop {
            if let Some(list) = self.ready.pop_front() {
                return TrackDevicesEvent::List(list);
            }
            match self.stdout.read(&mut buffer) {
                Ok(0) => return TrackDevicesEvent::Ended,
                Ok(read) => match self.decoder.push(&buffer[..read]) {
                    Ok(lists) => self.ready.extend(lists),
                    Err(TrackDevicesProtocolInvalid) => return TrackDevicesEvent::ProtocolInvalid,
                },
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => return TrackDevicesEvent::Ended,
            }
        }
    }
}

impl Adb {
    /// Workflow #398 RD-2 (model-398 §4.3, `device_link.offline_action = kick_once`): one
    /// `adb -s <serial> reconnect`, which kicks only that device's transport from the host side.
    /// An open-phase command like `connect` (read-session-resource-close.md): a host-side
    /// transport kick that sends nothing to the device, touches no other device and changes no
    /// setting.
    pub fn reconnect(&self, serial: &str) -> DeviceResult<()> {
        self.run(&["-s", serial, "reconnect"]).map(|_| ())
    }
}
