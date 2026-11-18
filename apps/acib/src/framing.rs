// SPDX-License-Identifier: AGPL-3.0-only

//! What `acib` prints on its standard output (board #479, ACIB-2): its one JSON line and a line
//! feed, and, after the line of a picture `screenshot --out -` took, exactly the picture's
//! `bytes` raw bytes as the service gave them, and nothing after them. Bytes follow a line
//! exactly when it is a screenshot's line with `"out": "-"` (a line that names `bytes`); a
//! refused line is followed by none.
//!
//! The standard output is written as bytes: Rust's standard library hands them to the
//! operating system as they are (on Windows to `WriteFile` on the handle, with no C runtime
//! text mode in between), so no line end is translated; a console refuses bytes that are not
//! UTF-8, which then fails the run (status 1). The caller redirects the output to a file or a
//! pipe.

use crate::answer::{Elapsed, Line, Screenshot, Way};
use crate::args::STANDARD_OUTPUT;
use std::io::{self, Write};

/// What a command prints: its line and, after the line of a picture asked for on the standard
/// output, the picture's bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Printed {
    pub line: Line,
    pub picture: Option<Vec<u8>>,
}

impl Printed {
    /// A line with nothing after it.
    pub fn line(line: Line) -> Self {
        Self {
            line,
            picture: None,
        }
    }

    /// The line of `picture`, taken from device `device` the way `via` and asked for on the
    /// standard output (`"out": "-"`), and the picture's bytes after it.
    pub fn picture_on_stdout(device: u32, via: Way, picture: Vec<u8>, elapsed: Elapsed) -> Self {
        let line = Line::Screenshot(Screenshot::of(
            device,
            via,
            &picture,
            elapsed,
            STANDARD_OUTPUT,
        ));
        Self {
            line,
            picture: Some(picture),
        }
    }
}

/// Writes the line `text`, a line feed and then `picture`'s bytes as they are, and nothing
/// after them, into `out`, and flushes it.
pub fn write_framed<W: Write>(mut out: W, text: &str, picture: Option<&[u8]>) -> io::Result<()> {
    out.write_all(text.as_bytes())?;
    out.write_all(b"\n")?;
    if let Some(bytes) = picture {
        out.write_all(bytes)?;
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::answer::{CommandName, Failure, Problem};
    use crate::picture::tests::png_file;

    const ELAPSED: Elapsed = Elapsed {
        request: 410,
        command: 812,
    };

    /// What `printed` writes: its line, serialized, then its bytes.
    fn written(printed: &Printed) -> Vec<u8> {
        let text = serde_json::to_string(&printed.line).expect("a line serializes");
        let mut out = Vec::new();
        write_framed(&mut out, &text, printed.picture.as_deref()).expect("written");
        out
    }

    /// A picture on the standard output is its header line (`"out": "-"`), one line feed and
    /// exactly `bytes` raw bytes as the service gave them, line ends and every other byte kept,
    /// and nothing after them.
    #[test]
    fn a_picture_on_stdout_is_its_line_a_line_feed_and_exactly_its_bytes() {
        let mut rest = b"\0\0\0\x0cIDAT\r\n\x1a\n\n\r\x00\xff\xfe\r".to_vec();
        rest.extend(0..=u8::MAX);
        let picture = png_file(750, 1334, &rest);
        let printed = Printed::picture_on_stdout(3, Way::ScreenCapture, picture.clone(), ELAPSED);
        let out = written(&printed);

        let end = out
            .iter()
            .position(|byte| *byte == b'\n')
            .expect("a line feed");
        let header: serde_json::Value =
            serde_json::from_slice(&out[..end]).expect("the header is one JSON line");
        assert_eq!(header["command"], "screenshot");
        assert_eq!(header["out"], "-");
        assert_eq!(header["format"], "png");
        assert_eq!(
            (header["width"].as_u64(), header["height"].as_u64()),
            (Some(750), Some(1334))
        );
        assert_eq!(header["bytes"].as_u64(), Some(picture.len() as u64));
        assert_eq!(&out[end + 1..], picture.as_slice());

        let text = serde_json::to_string(&printed.line).expect("a line serializes");
        let mut expected = text.into_bytes();
        expected.push(b'\n');
        expected.extend_from_slice(&picture);
        assert_eq!(out, expected);
    }

    /// A picture whose header cannot be read still follows its line, which names its length
    /// and the header's problem (status 1): the bytes are the service's, as a file would hold
    /// them.
    #[test]
    fn a_picture_whose_header_is_unknown_still_follows_its_line() {
        let picture = b"GIF89a-not-read-here".to_vec();
        let printed = Printed::picture_on_stdout(3, Way::Instruments, picture.clone(), ELAPSED);
        assert!(!printed.line.answered());
        let out = written(&printed);
        let end = out
            .iter()
            .position(|byte| *byte == b'\n')
            .expect("a line feed");
        let header: serde_json::Value = serde_json::from_slice(&out[..end]).expect("one line");
        assert_eq!(header["problem"], "picture_header_unknown");
        assert_eq!(header["bytes"].as_u64(), Some(picture.len() as u64));
        assert_eq!(&out[end + 1..], picture.as_slice());
    }

    /// A refused line, a screenshot's included, is its line and a line feed, and no byte after
    /// them.
    #[test]
    fn a_refused_line_is_followed_by_no_byte() {
        let printed = Printed::line(Line::refused_on(
            Some(CommandName::Screenshot),
            Some(3),
            Some(Way::ScreenCapture),
            Failure::own(Problem::ScreenshotFailed),
        ));
        assert_eq!(printed.picture, None);
        let out = written(&printed);
        assert_eq!(
            out,
            b"{\"command\":\"screenshot\",\"device\":3,\"via\":\"screen_capture\",\"problem\":\"screenshot_failed\"}\n"
        );
    }
}
