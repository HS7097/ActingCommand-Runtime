// SPDX-License-Identifier: AGPL-3.0-only

//! What the phone sent on one connection, as `acib probe` tells it (board #479, ACIB-3b, second
//! round).
//!
//! [`Tapped`] wraps the stream of one connection inside the tunnel: every byte the library writes
//! and reads passes through it unchanged, and of the bytes the library reads from the phone a
//! [`Seen`] counts the HTTP/2 frames by their type (a 9-byte header: a 24-bit length, a type, a
//! flags byte and a 31-bit stream, as `xpc/http2/frame.rs` of idevice 0.1.68 reads them) and, of
//! the DATA on the root channel (stream 1) and on the reply channel (stream 3), the XPC messages
//! the library reads there (a 24-byte wrapper, `0x29b00b92`, its flags, its body's length and
//! its id, then the body, decoded by the library's own `XPCMessage::decode`): those with a body
//! the library returns, those it skips (an empty dictionary, or no body at all: `xpc/mod.rs`
//! 156-166), those it cannot read, those of the skipped ones flagged as a reply (`0x00020000`),
//! and the bytes of a message not whole yet. A message is kept only until it is whole and counted:
//! nothing of its content leaves this module, only counts, and a count names no application, no
//! process and no identifier of the phone. The library reads only while it waits for an answer
//! (its handshake only writes), so what is counted is everything the phone sent on the
//! connection up to the end of the wait, its answers to the handshake included: the `listen` ways
//! of the probe count those alone, and [`unanswered`] counts a way's messages beyond them.
//! (ACIB-3b, third round) `running`, `launch` and `stop` read their connections to the
//! application service through a [`Tapped`] stream too, so that an error after the phone ended
//! the connection is told by a word of its own ([`kind_after`]).

use crate::answer::{ErrorKind, ProbeOutcome};
use idevice::ReadWrite;
use idevice::xpc::XPCMessage;
use serde::Serialize;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The length of an HTTP/2 frame's header.
const FRAME_HEADER: usize = 9;
/// The HTTP/2 frame types counted by name; any other is `other`.
const DATA: u8 = 0x0;
const HEADERS: u8 = 0x1;
const RST_STREAM: u8 = 0x3;
const SETTINGS: u8 = 0x4;
const GOAWAY: u8 = 0x7;
const WINDOW_UPDATE: u8 = 0x8;
/// The stream identifier's 31 bits (the first bit is reserved).
const STREAM_BITS: u32 = 0x7fff_ffff;
/// The root channel and the reply channel of the library's RemoteXPC client (`xpc/mod.rs`
/// 18-19).
const ROOT_STREAM: u32 = 1;
const REPLY_STREAM: u32 = 3;
/// An XPC message's wrapper: its magic, flags, body length and message id.
const XPC_WRAPPER: usize = 24;
const XPC_MAGIC: u32 = 0x29b0_0b92;
/// The flag of an XPC message that answers one that wanted a reply (`xpc/format.rs` 35).
const REPLY_FLAG: u32 = 0x0002_0000;
/// The longest body counted: a wrapper that announces more is not read as a message.
const LONGEST_BODY: u64 = 64 * 1024 * 1024;

/// The HTTP/2 frames the phone sent, by type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Frames {
    pub settings: u64,
    pub window_update: u64,
    pub headers: u64,
    pub data: u64,
    pub rst_stream: u64,
    pub goaway: u64,
    /// Any other type (a PING, say), which the library does not read and fails on.
    pub other: u64,
}

/// The XPC messages the phone sent on one channel, by what the library does with them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ChannelCounts {
    /// Messages with a body the library returns.
    pub content: u64,
    /// Messages whose body is an empty dictionary, which the library skips.
    pub empty: u64,
    /// Messages without a body, which the library skips.
    pub bodyless: u64,
    /// Messages the library's decoder cannot read, or bytes that are no XPC message (the rest
    /// of the channel is then not counted).
    pub unreadable: u64,
    /// Of the empty and the bodyless messages, those flagged as a reply.
    pub replies_without_body: u64,
    /// The bytes of a message not whole yet.
    pub pending_bytes: u64,
}

impl ChannelCounts {
    /// These counts beyond `baseline`'s (each never below zero); the pending bytes as they are.
    fn beyond(self, baseline: Self) -> Self {
        Self {
            content: self.content.saturating_sub(baseline.content),
            empty: self.empty.saturating_sub(baseline.empty),
            bodyless: self.bodyless.saturating_sub(baseline.bodyless),
            unreadable: self.unreadable.saturating_sub(baseline.unreadable),
            replies_without_body: self
                .replies_without_body
                .saturating_sub(baseline.replies_without_body),
            pending_bytes: self.pending_bytes,
        }
    }

    /// One whole message, its wrapper and its body.
    fn count(&mut self, message: &[u8]) {
        let flags = u32::from_le_bytes([message[4], message[5], message[6], message[7]]);
        let skipped = match XPCMessage::decode(message) {
            Ok(decoded) => match decoded.message {
                None => {
                    self.bodyless += 1;
                    true
                }
                Some(body)
                    if body
                        .as_dictionary()
                        .is_some_and(|members| members.is_empty()) =>
                {
                    self.empty += 1;
                    true
                }
                Some(_) => {
                    self.content += 1;
                    false
                }
            },
            Err(_) => {
                self.unreadable += 1;
                false
            }
        };
        if skipped && flags & REPLY_FLAG != 0 {
            self.replies_without_body += 1;
        }
    }
}

/// What the phone sent on one connection, as far as the library read it: counts only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PhoneSent {
    /// Every byte read.
    pub bytes: u64,
    /// A read found the connection ended by the phone.
    pub closed: bool,
    /// A read failed.
    pub read_failed: bool,
    pub frames: Frames,
    /// Stream 1, the root channel.
    pub root: ChannelCounts,
    /// Stream 3, the reply channel.
    pub reply: ChannelCounts,
    /// The DATA bytes on any other stream.
    pub other_stream_bytes: u64,
    /// The bytes of a frame not whole yet.
    pub pending_frame_bytes: u64,
}

/// One channel's messages: their counts and the bytes of the one not whole yet.
#[derive(Debug, Default)]
struct Channel {
    counts: ChannelCounts,
    buffer: Vec<u8>,
    /// Its bytes stopped being XPC messages: the rest is not counted.
    lost: bool,
}

/// Where the first message of a channel's bytes ends.
enum Framed {
    /// It is whole, this long.
    Whole(usize),
    /// It is not whole yet.
    Partial,
    /// The bytes are no XPC message the library reads.
    Unframed,
}

/// Where the XPC message `bytes` starts with ends.
fn message_at(bytes: &[u8]) -> Framed {
    if bytes.len() >= 4 && u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) != XPC_MAGIC
    {
        return Framed::Unframed;
    }
    if bytes.len() < XPC_WRAPPER {
        return Framed::Partial;
    }
    let body = u64::from_le_bytes([
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    ]);
    if body > LONGEST_BODY {
        return Framed::Unframed;
    }
    let whole = XPC_WRAPPER + body as usize;
    if bytes.len() >= whole {
        Framed::Whole(whole)
    } else {
        Framed::Partial
    }
}

impl Channel {
    /// A DATA frame's payload on this channel.
    fn data(&mut self, payload: &[u8]) {
        if self.lost {
            return;
        }
        self.buffer.extend_from_slice(payload);
        loop {
            match message_at(&self.buffer) {
                Framed::Whole(length) => {
                    self.counts.count(&self.buffer[..length]);
                    self.buffer.drain(..length);
                }
                Framed::Partial => break,
                Framed::Unframed => {
                    self.counts.unreadable += 1;
                    self.lost = true;
                    self.buffer.clear();
                    break;
                }
            }
        }
    }

    fn summary(&self) -> ChannelCounts {
        ChannelCounts {
            pending_bytes: self.buffer.len() as u64,
            ..self.counts
        }
    }
}

/// The type, stream and whole length of the frame `bytes` starts with, when it is whole.
fn frame_at(bytes: &[u8]) -> Option<(u8, u32, usize)> {
    let header = bytes.get(..FRAME_HEADER)?;
    let length = u32::from_be_bytes([0, header[0], header[1], header[2]]) as usize;
    let stream = u32::from_be_bytes([header[5], header[6], header[7], header[8]]) & STREAM_BITS;
    let total = FRAME_HEADER + length;
    (bytes.len() >= total).then_some((header[3], stream, total))
}

/// What the library has read from the phone on one connection, counted as it is read.
#[derive(Debug, Default)]
pub struct Seen {
    bytes: u64,
    closed: bool,
    read_failed: bool,
    frames: Frames,
    /// The bytes of a frame not whole yet.
    pending: Vec<u8>,
    root: Channel,
    reply: Channel,
    other_stream_bytes: u64,
}

impl Seen {
    /// `bytes` the library read from the phone, in the order read.
    pub fn read(&mut self, bytes: &[u8]) {
        self.bytes += bytes.len() as u64;
        let mut pending = std::mem::take(&mut self.pending);
        pending.extend_from_slice(bytes);
        let mut start = 0;
        while let Some((kind, stream, total)) = frame_at(&pending[start..]) {
            self.frame(kind, stream, &pending[start + FRAME_HEADER..start + total]);
            start += total;
        }
        pending.drain(..start);
        self.pending = pending;
    }

    /// A read found the connection ended by the phone.
    pub fn note_end(&mut self) {
        self.closed = true;
    }

    /// A read failed.
    pub fn note_failure(&mut self) {
        self.read_failed = true;
    }

    /// One whole frame.
    fn frame(&mut self, kind: u8, stream: u32, payload: &[u8]) {
        match kind {
            DATA => {
                self.frames.data += 1;
                match stream {
                    ROOT_STREAM => self.root.data(payload),
                    REPLY_STREAM => self.reply.data(payload),
                    _ => self.other_stream_bytes += payload.len() as u64,
                }
            }
            HEADERS => self.frames.headers += 1,
            RST_STREAM => self.frames.rst_stream += 1,
            SETTINGS => self.frames.settings += 1,
            GOAWAY => self.frames.goaway += 1,
            WINDOW_UPDATE => self.frames.window_update += 1,
            _ => self.frames.other += 1,
        }
    }

    /// The counts so far.
    pub fn summary(&self) -> PhoneSent {
        PhoneSent {
            bytes: self.bytes,
            closed: self.closed,
            read_failed: self.read_failed,
            frames: self.frames,
            root: self.root.summary(),
            reply: self.reply.summary(),
            other_stream_bytes: self.other_stream_bytes,
            pending_frame_bytes: self.pending.len() as u64,
        }
    }
}

/// The counts of `seen` so far.
pub fn summary_of(seen: &Mutex<Seen>) -> PhoneSent {
    seen.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .summary()
}

/// A connection's stream, every byte passed through as it is, the bytes read counted in `seen`.
#[derive(Debug)]
pub struct Tapped {
    inner: Box<dyn ReadWrite>,
    seen: Arc<Mutex<Seen>>,
}

impl Tapped {
    /// `inner`, its reads counted in `seen`.
    pub fn new(inner: Box<dyn ReadWrite>, seen: Arc<Mutex<Seen>>) -> Self {
        Self { inner, seen }
    }
}

impl AsyncRead for Tapped {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let (before, room) = (buf.filled().len(), buf.remaining());
        let polled = Pin::new(&mut this.inner).poll_read(cx, buf);
        if let Poll::Ready(result) = &polled {
            let mut seen = this.seen.lock().unwrap_or_else(PoisonError::into_inner);
            match result {
                Ok(()) if buf.filled().len() == before && room > 0 => seen.note_end(),
                Ok(()) => seen.read(&buf.filled()[before..]),
                Err(_) => seen.note_failure(),
            }
        }
        polled
    }
}

impl AsyncWrite for Tapped {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// The channels the library reads a way's answer on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reads {
    /// The root channel alone (`recv_root`: the streamed list).
    Root,
    /// The reply channel alone (`recv`: `list_apps`, `list_processes`).
    Reply,
    /// Both (`recv_any`: the `listen` ways).
    Both,
}

/// The phone ended the connection or reset a stream.
fn ended(sent: &PhoneSent) -> bool {
    sent.closed || sent.frames.goaway > 0 || sent.frames.rst_stream > 0
}

/// The word of a way whose wait passed before the library returned, from what the phone sent on
/// its connection (`sent`), the messages counted beyond those of `baseline` (the `listen` of the
/// same service, which asked nothing; none for a `listen`), the way's answer read on `reads`. In
/// this order: the phone ended the connection; a message with a body came on a channel the way
/// does not read; a frame or a message is not whole, or messages with a body came on the channel
/// read without the library returning (a streamed list without its end); a message flagged as a
/// reply came without a body on the channel read; not one byte came; else no answer.
pub fn unanswered(sent: &PhoneSent, baseline: Option<&PhoneSent>, reads: Reads) -> ProbeOutcome {
    if ended(sent) {
        return ProbeOutcome::ConnectionClosed;
    }
    let unasked = baseline.copied().unwrap_or_default();
    let (root, reply) = (
        sent.root.beyond(unasked.root),
        sent.reply.beyond(unasked.reply),
    );
    let channels = [root, reply];
    let (read, unread): (&[ChannelCounts], &[ChannelCounts]) = match reads {
        Reads::Root => (&channels[..1], &channels[1..]),
        Reads::Reply => (&channels[1..], &channels[..1]),
        Reads::Both => (&channels, &[]),
    };
    if unread.iter().any(|channel| channel.content > 0) {
        return ProbeOutcome::AnswerOnOtherChannel;
    }
    let pending = sent.pending_frame_bytes > 0 || root.pending_bytes > 0 || reply.pending_bytes > 0;
    if pending || read.iter().any(|channel| channel.content > 0) {
        return ProbeOutcome::AnswerIncomplete;
    }
    if read.iter().any(|channel| channel.replies_without_body > 0) {
        return ProbeOutcome::AnswerWithoutBody;
    }
    if sent.bytes == 0 {
        return ProbeOutcome::Silent;
    }
    ProbeOutcome::NoAnswer
}

/// The word of a way the library returned an error of `kind` for, and the kind it names: the
/// phone ended the connection (whatever the library made of it); the service answered with its
/// error (`refused`); any other error (`failed`, with its kind).
pub fn failed(sent: &PhoneSent, kind: ErrorKind) -> (ProbeOutcome, Option<ErrorKind>) {
    if ended(sent) {
        (ProbeOutcome::ConnectionClosed, Some(kind))
    } else if kind == ErrorKind::DeviceRefused {
        (ProbeOutcome::Refused, None)
    } else {
        (ProbeOutcome::Failed, Some(kind))
    }
}

/// (ACIB-3b, third round) The kind of an error of `running`, `launch` or `stop` on a connection
/// the phone sent `sent` on, the library's error being of `kind`: `connection_closed` when the
/// phone had ended the connection or reset a stream of it (as [`failed`] tells it of a way of
/// `probe`), whatever the library made of that (it reports the end as an unexpected response,
/// `answer_unreadable`, and the GOAWAY and the reset as XPC errors, `xpc_failed`); else `kind`.
pub fn kind_after(sent: &PhoneSent, kind: ErrorKind) -> ErrorKind {
    if ended(sent) {
        ErrorKind::ConnectionClosed
    } else {
        kind
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use idevice::xpc::{Dictionary, XPCObject};

    /// An HTTP/2 frame of `kind` on `stream` with `payload`.
    fn frame(kind: u8, flags: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
        let length = u32::try_from(payload.len())
            .expect("a short payload")
            .to_be_bytes();
        let mut bytes = vec![length[1], length[2], length[3], kind, flags];
        bytes.extend_from_slice(&stream.to_be_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    /// An XPC message flagged `flags`, with `body`, as the library encodes one.
    fn message(flags: u32, body: Option<XPCObject>) -> Vec<u8> {
        XPCMessage {
            flags,
            message: body,
            message_id: Some(1),
        }
        .encode(1)
        .expect("a message encodes")
    }

    /// A dictionary with one member: a body the library returns.
    fn content() -> XPCObject {
        let mut members = Dictionary::new();
        members.insert("answer".to_owned(), XPCObject::Bool(true));
        XPCObject::Dictionary(members)
    }

    /// The frames of the phone's settings, their acknowledgement, a window update and a header,
    /// an empty dictionary on the root channel, a bodyless reply and a message with a body split
    /// over two frames on the reply channel, data on another stream, the start of a message on
    /// the root channel, and the start of a frame: each counted by its kind, read in pieces of
    /// five bytes, the pieces not whole yet counted as pending; then the end of the connection.
    #[test]
    fn the_tap_counts_the_frames_and_messages_the_phone_sent() {
        let with_body = message(0x0002_0001, Some(content()));
        let (first, second) = with_body.split_at(10);
        let one = message(1, Some(content()));
        let start_of_one = &one[..30];
        let mut bytes = Vec::new();
        bytes.extend(frame(SETTINGS, 0, 0, &[0, 3, 0, 0, 0, 100]));
        bytes.extend(frame(SETTINGS, 1, 0, &[]));
        bytes.extend(frame(WINDOW_UPDATE, 0, 0, &[0, 0, 0, 255]));
        bytes.extend(frame(HEADERS, 4, 1, &[]));
        let empty = message(1, Some(XPCObject::Dictionary(Dictionary::new())));
        bytes.extend(frame(DATA, 0, 1, &empty));
        bytes.extend(frame(DATA, 0, 3, &message(0x0002_0001, None)));
        bytes.extend(frame(DATA, 0, 3, first));
        bytes.extend(frame(DATA, 0, 3, second));
        bytes.extend(frame(DATA, 0, 5, &[7; 7]));
        bytes.extend(frame(DATA, 0, 1, start_of_one));
        let cut = frame(DATA, 0, 3, &[0; 100]);
        bytes.extend(&cut[..29]);
        let mut seen = Seen::default();
        for piece in bytes.chunks(5) {
            seen.read(piece);
        }
        let sent = seen.summary();
        assert_eq!(sent.bytes, bytes.len() as u64);
        assert!(!sent.closed && !sent.read_failed);
        assert_eq!(
            sent.frames,
            Frames {
                settings: 2,
                window_update: 1,
                headers: 1,
                data: 6,
                ..Frames::default()
            }
        );
        assert_eq!(
            sent.root,
            ChannelCounts {
                empty: 1,
                pending_bytes: 30,
                ..ChannelCounts::default()
            }
        );
        assert_eq!(
            sent.reply,
            ChannelCounts {
                content: 1,
                bodyless: 1,
                replies_without_body: 1,
                ..ChannelCounts::default()
            }
        );
        assert_eq!(sent.other_stream_bytes, 7);
        assert_eq!(sent.pending_frame_bytes, 29);
        seen.note_end();
        assert!(seen.summary().closed);

        // A reset, a goaway, a frame of another type, and bytes on the reply channel that are no
        // XPC message: the channel's rest is not counted.
        let mut seen = Seen::default();
        let mut bytes = frame(RST_STREAM, 0, 3, &[0, 0, 0, 8]);
        bytes.extend(frame(GOAWAY, 0, 0, &[0; 8]));
        bytes.extend(frame(0x6, 0, 0, &[0; 8]));
        bytes.extend(frame(DATA, 0, 3, &[0xAB; 24]));
        bytes.extend(frame(DATA, 0, 3, &message(1, Some(content()))));
        seen.read(&bytes);
        seen.note_failure();
        let sent = seen.summary();
        assert_eq!(
            (
                sent.frames.rst_stream,
                sent.frames.goaway,
                sent.frames.other,
                sent.frames.data
            ),
            (1, 1, 1, 2)
        );
        assert_eq!(
            sent.reply,
            ChannelCounts {
                unreadable: 1,
                ..ChannelCounts::default()
            }
        );
        assert!(sent.read_failed);
    }

    /// Each word of a way follows from what the phone sent, in the order `unanswered` and
    /// `failed` decide, messages counted beyond the `listen` of the same service.
    #[test]
    fn each_word_of_a_way_follows_from_what_the_phone_sent() {
        let counts = |content, replies_without_body, pending_bytes| ChannelCounts {
            content,
            replies_without_body,
            pending_bytes,
            ..ChannelCounts::default()
        };
        let alive = PhoneSent {
            bytes: 40,
            frames: Frames {
                settings: 1,
                ..Frames::default()
            },
            ..PhoneSent::default()
        };
        let with = |root: ChannelCounts, reply: ChannelCounts| PhoneSent {
            root,
            reply,
            ..alive
        };
        let none = ChannelCounts::default();
        let baseline = with(counts(1, 0, 0), counts(0, 1, 0));
        let cases = [
            (
                PhoneSent::default(),
                None,
                Reads::Reply,
                ProbeOutcome::Silent,
            ),
            (alive, None, Reads::Reply, ProbeOutcome::NoAnswer),
            (alive, None, Reads::Both, ProbeOutcome::NoAnswer),
            (
                PhoneSent {
                    closed: true,
                    ..alive
                },
                None,
                Reads::Reply,
                ProbeOutcome::ConnectionClosed,
            ),
            (
                PhoneSent {
                    frames: Frames {
                        goaway: 1,
                        ..Frames::default()
                    },
                    ..alive
                },
                None,
                Reads::Root,
                ProbeOutcome::ConnectionClosed,
            ),
            (
                with(counts(1, 0, 0), none),
                None,
                Reads::Reply,
                ProbeOutcome::AnswerOnOtherChannel,
            ),
            (
                with(none, counts(1, 0, 0)),
                None,
                Reads::Root,
                ProbeOutcome::AnswerOnOtherChannel,
            ),
            (
                with(counts(1, 0, 0), none),
                Some(baseline),
                Reads::Reply,
                ProbeOutcome::NoAnswer,
            ),
            (
                with(counts(3, 0, 0), none),
                None,
                Reads::Root,
                ProbeOutcome::AnswerIncomplete,
            ),
            (
                with(none, counts(0, 0, 12)),
                None,
                Reads::Reply,
                ProbeOutcome::AnswerIncomplete,
            ),
            (
                with(none, counts(0, 1, 0)),
                None,
                Reads::Reply,
                ProbeOutcome::AnswerWithoutBody,
            ),
            (
                with(none, counts(0, 2, 0)),
                Some(baseline),
                Reads::Reply,
                ProbeOutcome::AnswerWithoutBody,
            ),
            (
                with(none, counts(0, 1, 0)),
                Some(baseline),
                Reads::Reply,
                ProbeOutcome::NoAnswer,
            ),
            (
                with(none, counts(0, 1, 0)),
                None,
                Reads::Root,
                ProbeOutcome::NoAnswer,
            ),
        ];
        for (sent, compared, reads, word) in cases {
            assert_eq!(
                unanswered(&sent, compared.as_ref(), reads),
                word,
                "{sent:?} {reads:?}"
            );
        }
        let reset = PhoneSent {
            frames: Frames {
                rst_stream: 1,
                ..Frames::default()
            },
            ..alive
        };
        let failures = [
            (alive, ErrorKind::DeviceRefused, ProbeOutcome::Refused, None),
            (
                alive,
                ErrorKind::XpcFailed,
                ProbeOutcome::Failed,
                Some(ErrorKind::XpcFailed),
            ),
            (
                PhoneSent {
                    closed: true,
                    ..alive
                },
                ErrorKind::AnswerUnreadable,
                ProbeOutcome::ConnectionClosed,
                Some(ErrorKind::AnswerUnreadable),
            ),
            (
                reset,
                ErrorKind::XpcFailed,
                ProbeOutcome::ConnectionClosed,
                Some(ErrorKind::XpcFailed),
            ),
        ];
        for (sent, kind, word, named) in failures {
            assert_eq!(failed(&sent, kind), (word, named), "{sent:?} {kind:?}");
        }

        // (ACIB-3b, third round) The kind of an error of `running`, `launch` or `stop`: on a
        // connection the phone ended (a read found its end, which the library reports as an
        // unexpected response) or on which it sent a GOAWAY or reset a stream (which it reports as
        // XPC errors), `connection_closed`; on one it did not, the library's own kind, each of them.
        let closed = PhoneSent {
            closed: true,
            ..alive
        };
        let gone_away = PhoneSent {
            frames: Frames {
                goaway: 1,
                ..Frames::default()
            },
            ..alive
        };
        let kinds = [
            ErrorKind::DeviceRefused,
            ErrorKind::AnswerUnreadable,
            ErrorKind::ConnectionFailed,
            ErrorKind::XpcFailed,
            ErrorKind::Other,
        ];
        for kind in kinds {
            assert_eq!(kind_after(&alive, kind), kind, "{kind:?}");
            assert_eq!(kind_after(&PhoneSent::default(), kind), kind, "{kind:?}");
        }
        for (sent, kind) in [
            (closed, ErrorKind::AnswerUnreadable),
            (gone_away, ErrorKind::XpcFailed),
            (reset, ErrorKind::XpcFailed),
        ] {
            assert_eq!(
                kind_after(&sent, kind),
                ErrorKind::ConnectionClosed,
                "{sent:?} {kind:?}"
            );
        }
    }
}
