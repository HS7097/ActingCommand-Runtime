// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #479 F-3a (register §1.486; contracts/device-binding.md, "An iOS handset"; work order
// rd-1119/brief-479-f3a-ios-capture-backend.md, its tests 1 to 5 and 7, with the coordinator's
// addition of 08:18 on the phone's Display P3 pictures). `acib` is scripted as a child behind the
// launcher (`AcibLauncher`), as F-2 scripted it in the Lab; the scripted child's time is its own
// (`pause` adds to `elapsed`), so no clock is read here. The real child of test 7 is this test
// executable started again on an ignored entry that waits forever. Pictures are written here as
// PNG files by the format's own rules (stored deflate blocks, big-endian 16-bit samples), so the
// samples are exactly the test's. No identifier of a phone exists anywhere here: the key is a
// neutral fake.
//
// Fix round 1 (rd-1119/brief-479-f3a-fix1.md, items 6 and 7): `service_unreachable` is the screen
// capture service's cause; standard error that cannot be read is a cause of its own; 16-bit
// samples whose two bytes differ are read with their high byte first; a picture with an alpha
// channel or a palette is unreadable, as a grey one is.

use super::picture::{self, PictureProblem};
use super::*;
use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

const KEY: &str = "sha256:479f3a000000000000000000000000000000000000000000000000000000f3a1";
const SALT: &str = "neutral-installation-secret-f3a";
const STDERR_MARKER: &str = "NEUTRAL-STDERR-MARKER-F3A";

// ---- PNG files written by the format's rules ----

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1_u32, 0_u32);
    for byte in bytes {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = (data.len() as u32).to_be_bytes().to_vec();
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
    out
}

/// A zlib stream of `raw` in stored deflate blocks.
fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let blocks = raw.chunks(65_535).collect::<Vec<_>>();
    for (index, block) in blocks.iter().enumerate() {
        out.push(u8::from(index + 1 == blocks.len()));
        let length = block.len() as u16;
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&(!length).to_le_bytes());
        out.extend_from_slice(block);
    }
    if blocks.is_empty() {
        out.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

/// The colour chunks a test picture carries.
enum Space {
    Cicp([u8; 4]),
    SrgbChunk,
    None,
}

/// An RGB PNG of `width` x `height` of `bit_depth` (8 or 16) whose samples are `samples`, row by
/// row, with the colour chunk `space` and an `eXIf` chunk, which the reader skips, as the phone's
/// pictures carry one (no `iCCP` chunk is written).
fn png(width: u32, height: u32, bit_depth: u8, samples: &[u16], space: Space) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = width.to_be_bytes().to_vec();
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[bit_depth, 2, 0, 0, 0]);
    out.extend(chunk(b"IHDR", &header));
    match space {
        Space::Cicp(numbers) => out.extend(chunk(b"cICP", &numbers)),
        Space::SrgbChunk => out.extend(chunk(b"sRGB", &[0])),
        Space::None => {}
    }
    out.extend(chunk(b"eXIf", b"MM\0*neutral"));
    let row = width as usize * 3;
    let mut raw = Vec::new();
    for line in samples.chunks(row) {
        raw.push(0);
        for sample in line {
            if bit_depth == 16 {
                raw.extend_from_slice(&sample.to_be_bytes());
            } else {
                raw.push(*sample as u8);
            }
        }
    }
    out.extend(chunk(b"IDAT", &zlib_stored(&raw)));
    out.extend(chunk(b"IEND", &[]));
    out
}

const SRGB: Space = Space::Cicp([1, 13, 0, 1]);
const P3: Space = Space::Cicp([12, 13, 0, 1]);

// ---- the scripted acib ----

/// How a scripted child ends when the runner ends it.
#[derive(Clone, Copy)]
enum End {
    Confirmed,
    Unconfirmed,
}

/// One scripted `acib`: what it prints, how it ends, whether its state and (fix round 1) its
/// errors can be read.
struct Script {
    output: Vec<u8>,
    errors: Vec<u8>,
    errors_fail: bool,
    status: Option<bool>,
    state_fails: bool,
    end: End,
}

impl Script {
    fn answering(output: Vec<u8>, success: bool) -> Self {
        Self {
            output,
            errors: STDERR_MARKER.as_bytes().to_vec(),
            errors_fail: false,
            status: Some(success),
            state_fails: false,
            end: End::Confirmed,
        }
    }
}

/// Fix round 1: a stream whose every read fails, as a broken pipe's.
struct FailingRead;

impl Read for FailingRead {
    fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("scripted read failure"))
    }
}

/// What a scripted launch saw of its command.
struct Launched {
    program: PathBuf,
    arguments: Vec<String>,
    environment: Vec<(&'static str, String)>,
    debug: String,
}

#[derive(Default)]
struct ScriptedLauncher {
    scripts: Mutex<VecDeque<io::Result<Script>>>,
    launched: Mutex<Vec<Launched>>,
    ends: Arc<AtomicUsize>,
}

impl ScriptedLauncher {
    fn with(scripts: Vec<io::Result<Script>>) -> Arc<Self> {
        Arc::new(Self {
            scripts: Mutex::new(scripts.into()),
            ..Self::default()
        })
    }
}

impl AcibLauncher for ScriptedLauncher {
    fn launch(&self, command: &AcibCommand) -> io::Result<Box<dyn AcibProcess>> {
        self.launched.lock().expect("launches").push(Launched {
            program: command.program.clone(),
            arguments: command.arguments.clone(),
            environment: command
                .environment
                .iter()
                .map(|(name, salt)| (*name, salt.expose().to_owned()))
                .collect(),
            debug: format!("{command:?}"),
        });
        let script = self
            .scripts
            .lock()
            .expect("scripts")
            .pop_front()
            .expect("a script for every launch")?;
        Ok(Box::new(ScriptedProcess {
            output: Some(script.output),
            errors: Some(script.errors),
            errors_fail: script.errors_fail,
            status: script.status,
            state_fails: script.state_fails,
            end: script.end,
            ended_by_runtime: false,
            waited: Duration::ZERO,
            ends: Arc::clone(&self.ends),
        }))
    }
}

struct ScriptedProcess {
    output: Option<Vec<u8>>,
    errors: Option<Vec<u8>>,
    errors_fail: bool,
    status: Option<bool>,
    state_fails: bool,
    end: End,
    ended_by_runtime: bool,
    waited: Duration,
    ends: Arc<AtomicUsize>,
}

impl AcibProcess for ScriptedProcess {
    fn output(&mut self) -> Option<Box<dyn Read + Send>> {
        self.output
            .take()
            .map(|bytes| Box::new(Cursor::new(bytes)) as Box<dyn Read + Send>)
    }

    fn errors(&mut self) -> Option<Box<dyn Read + Send>> {
        let errors_fail = self.errors_fail;
        self.errors.take().map(|bytes| {
            if errors_fail {
                Box::new(FailingRead) as Box<dyn Read + Send>
            } else {
                Box::new(Cursor::new(bytes)) as Box<dyn Read + Send>
            }
        })
    }

    fn ended(&mut self) -> io::Result<Option<bool>> {
        if self.state_fails {
            return Err(io::Error::other("scripted state failure"));
        }
        Ok(if self.ended_by_runtime {
            Some(false)
        } else {
            self.status
        })
    }

    fn elapsed(&self) -> Duration {
        self.waited
    }

    fn pause(&mut self, span: Duration) -> io::Result<()> {
        self.waited += span;
        Ok(())
    }

    fn end(&mut self) -> DeviceResult<DeviceResourceCloseOutcome> {
        self.ends.fetch_add(1, Ordering::AcqRel);
        match self.end {
            End::Confirmed => {
                self.ended_by_runtime = true;
                Ok(DeviceResourceCloseOutcome::confirmed(1))
            }
            End::Unconfirmed => Err(DeviceError::fatal("scripted acib child did not exit")
                .with_resource_close_cause(
                    DeviceResourceKind::ExternalChild,
                    DeviceResourceClosePhase::Deadline,
                    "acib",
                    None,
                    None,
                    DeviceResourceQuiescence::Unconfirmed,
                    1,
                )),
        }
    }
}

/// Bounds of a scripted capture: the Runtime's caps, a wait of four looks.
fn bounds() -> AcibBounds {
    AcibBounds {
        wait: Duration::from_millis(100),
        ..AcibBounds::runtime()
    }
}

/// The source of a scripted capture: an existing file as the program (this test executable), the
/// neutral salt, `launcher`.
fn source(launcher: Arc<ScriptedLauncher>) -> AcibCaptureSource {
    AcibCaptureSource::new(
        Some(std::env::current_exe().expect("the test executable")),
        AcibKeySalt::new(SALT),
    )
    .with_bounds(bounds())
    .with_launcher(launcher)
}

fn backend(launcher: &Arc<ScriptedLauncher>) -> Box<dyn CaptureBackend> {
    open_acib_screen_capture(&source(Arc::clone(launcher)), KEY).backend
}

/// The answer of a screenshot of `picture` on the standard output.
fn answer(picture: &[u8]) -> Vec<u8> {
    answer_as(picture, "png", "-")
}

/// The answer of a screenshot of `picture`, its line naming `format` and `out`.
fn answer_as(picture: &[u8], format: &str, out: &str) -> Vec<u8> {
    let mut answer = format!(
        "{{\"command\":\"screenshot\",\"device\":3,\"via\":\"screen_capture\",\"format\":\"{format}\",\"width\":2,\"height\":1,\"bytes\":{},\"elapsed_ms\":{{\"request\":401,\"command\":977}},\"out\":\"{out}\"}}\n",
        picture.len()
    )
    .into_bytes();
    answer.extend_from_slice(picture);
    answer
}

/// A refused screenshot's line with `word` (and `stage`).
fn refused(word: &str, stage: Option<&str>) -> Vec<u8> {
    let stage = stage.map_or(String::new(), |stage| format!(",\"stage\":\"{stage}\""));
    format!(
        "{{\"command\":\"screenshot\",\"device\":3,\"via\":\"screen_capture\",\"problem\":\"{word}\"{stage}}}\n"
    )
    .into_bytes()
}

/// A 2 x 1 8-bit sRGB picture that is not black.
fn small_picture() -> Vec<u8> {
    png(2, 1, 8, &[10, 20, 30, 40, 50, 60], Space::SrgbChunk)
}

#[track_caller]
fn cause(error: &DeviceError) -> IosCaptureFailure {
    error
        .ios_capture_failure()
        .unwrap_or_else(|| panic!("an iOS capture cause: {error:?}"))
}

// ---- 1: the picture ----

/// Test 1: a 16-bit sRGB picture of samples 257·k becomes exactly the 8-bit picture of k; the
/// frame is 8-bit RGB, keeps no original, and the artifact the store writes is an 8-bit PNG.
#[test]
fn f3a_t1_sixteen_bit_samples_of_257k_become_the_eight_bit_picture_of_k() {
    let ks: [u16; 6] = [0, 1, 17, 128, 254, 255];
    let samples = ks.map(|k| k * 257);
    let launcher = ScriptedLauncher::with(vec![Ok(Script::answering(
        answer(&png(2, 1, 16, &samples, SRGB)),
        true,
    ))]);
    let frame = backend(&launcher).capture().expect("a frame");
    assert_eq!((frame.width, frame.height), (2, 1));
    assert_eq!(frame.pixel_format, PixelFormat::Rgb8);
    assert_eq!(frame.pixels, ks.map(|k| k as u8));
    assert_eq!(frame.backend_name, CaptureBackendName::AcibScreenCapture);
    assert!(frame.original_png.is_none(), "no original is kept");
    let stored = frame.png_for_artifact().expect("the artifact's PNG");
    assert_eq!((stored[24], stored[25]), (8, 2), "an 8-bit RGB artifact");
}

/// Test 1: an 8-bit sRGB picture passes as it is, by an `sRGB` chunk and by `cICP` 1/13/0/1.
#[test]
fn f3a_t1_an_eight_bit_srgb_picture_passes_unchanged() {
    let samples = [255, 0, 0, 1, 2, 254];
    for space in [Space::SrgbChunk, SRGB] {
        let converted = picture::to_srgb8(&png(2, 1, 8, &samples, space)).expect("a picture");
        assert_eq!(converted.pixels, samples.map(|sample| sample as u8));
    }
}

/// The coordinator's addition, the derivation: the matrix from P3-D65 to BT.709 derived from the
/// chromaticities of H.273 code points 12 and 1 is the worked one of the contract, each row sums
/// to 1 and the shared blue primary keeps its column.
#[test]
fn f3a_t1_the_derived_p3_matrix_is_the_worked_one() {
    let worked = [
        [1.224_940_176_3, -0.224_940_176_3, 0.0],
        [-0.042_056_954_7, 1.042_056_954_7, 0.0],
        [-0.019_637_554_6, -0.078_636_045_6, 1.098_273_600_1],
    ];
    let derived = picture::p3_d65_to_bt709();
    for (row, expected) in derived.iter().zip(worked) {
        for (cell, expected) in row.iter().zip(expected) {
            assert!((cell - expected).abs() < 1e-9, "{derived:?}");
        }
        assert!((row.iter().sum::<f64>() - 1.0).abs() < 1e-12, "{derived:?}");
    }
}

/// The coordinator's addition: neutral samples 257·k of a P3 picture come out the 8-bit grey k
/// exactly.
#[test]
fn f3a_t1_neutral_p3_samples_come_out_the_grey_exactly() {
    let ks: [u16; 6] = [1, 17, 64, 128, 200, 255];
    let samples = ks
        .iter()
        .flat_map(|k| [k * 257, k * 257, k * 257])
        .collect::<Vec<_>>();
    let converted = picture::to_srgb8(&png(6, 1, 16, &samples, P3)).expect("a picture");
    let expected = ks
        .iter()
        .flat_map(|k| [*k as u8, *k as u8, *k as u8])
        .collect::<Vec<_>>();
    assert_eq!(converted.pixels, expected);
}

/// The coordinator's addition: the three sRGB primaries and three mixed colours, written as 16-bit
/// P3 samples by the forward matrix here, come back within 1 of their 8-bit sRGB values; a P3
/// colour outside sRGB clips to the gamut's edge and is not wrapped.
#[test]
fn f3a_t1_p3_colours_come_back_as_srgb_and_outside_ones_clip() {
    let colours: [[u8; 3]; 6] = [
        [255, 0, 0],
        [0, 255, 0],
        [0, 0, 255],
        [200, 120, 40],
        [30, 180, 220],
        [128, 64, 192],
    ];
    let forward = picture::inverse(&picture::p3_d65_to_bt709());
    let samples = colours
        .iter()
        .flat_map(|colour| {
            let linear = colour.map(|code| picture::srgb_decode(f64::from(code) / 255.0));
            forward.map(|row| {
                let value =
                    (row[0] * linear[0] + row[1] * linear[1] + row[2] * linear[2]).clamp(0.0, 1.0);
                (picture::srgb_encode(value) * 65_535.0).round() as u16
            })
        })
        .collect::<Vec<_>>();
    let converted = picture::to_srgb8(&png(6, 1, 16, &samples, P3)).expect("a picture");
    for (pixel, colour) in converted.pixels.chunks(3).zip(colours) {
        for (got, wanted) in pixel.iter().zip(colour) {
            assert!(got.abs_diff(wanted) <= 1, "{pixel:?} against {colour:?}");
        }
    }
    let outside =
        picture::to_srgb8(&png(2, 1, 16, &[0, 65_535, 0, 65_535, 0, 0], P3)).expect("a picture");
    assert_eq!(
        outside.pixels,
        [0, 255, 0, 255, 0, 0],
        "clipped, not wrapped"
    );
}

/// The coordinator's addition: an sRGB picture is not converted (its samples only rounded); a
/// picture with neither colour chunk, and one with other `cICP` numbers, answer their own cause,
/// through the backend too.
#[test]
fn f3a_t1_an_srgb_picture_is_not_converted_and_an_unknown_space_is_its_own_cause() {
    let samples = [257 * 200, 257 * 10, 257 * 255, 0, 257, 65_535];
    let converted = picture::to_srgb8(&png(2, 1, 16, &samples, SRGB)).expect("a picture");
    assert_eq!(converted.pixels, [200, 10, 255, 0, 1, 255]);
    for space in [
        Space::None,
        Space::Cicp([9, 16, 0, 1]),
        Space::Cicp([12, 1, 0, 1]),
    ] {
        let picture_bytes = png(2, 1, 16, &samples, space);
        assert_eq!(
            picture::to_srgb8(&picture_bytes).err(),
            Some(PictureProblem::ColorSpaceUnsupported)
        );
        let launcher =
            ScriptedLauncher::with(vec![Ok(Script::answering(answer(&picture_bytes), true))]);
        let error = backend(&launcher).capture().expect_err("no frame");
        assert_eq!(
            cause(&error).problem,
            IosCaptureProblem::IosPictureColorSpaceUnsupported
        );
    }
    // A form the conversion does not cover is unreadable, never a guess: grey, and no PNG.
    let mut grey = png(2, 1, 8, &[1, 2, 3, 4, 5, 6], Space::SrgbChunk);
    grey[25] = 0;
    assert_eq!(
        picture::to_srgb8(&grey).err(),
        Some(PictureProblem::Unreadable)
    );
    assert_eq!(
        picture::to_srgb8(b"not a picture").err(),
        Some(PictureProblem::Unreadable)
    );
}

/// The 8-bit sample of the 16-bit sample `v` by the work order's own rule,
/// `(v·255 + 32767) / 65535`.
fn rounded_by_the_rule(v: u16) -> u8 {
    u8::try_from((u32::from(v) * 255 + 32_767) / 65_535).expect("at most 255")
}

/// Fix round 1, item 7: 16-bit samples whose two bytes differ are read as PNG stores them, high
/// byte first: each comes out the rounding of its own value, and each would come out otherwise
/// were its bytes read the other way round (the 257·k samples, whose two bytes are equal, cannot
/// tell).
#[test]
fn f3a_t1_sixteen_bit_samples_are_read_high_byte_first() {
    let samples: [u16; 6] = [0xFF00, 0x00FF, 0x0100, 0x8000, 0x1234, 0xFEDC];
    for sample in samples {
        assert_ne!(
            rounded_by_the_rule(sample),
            rounded_by_the_rule(sample.swap_bytes()),
            "{sample:#06x} tells the two byte orders apart"
        );
    }
    let converted = picture::to_srgb8(&png(2, 1, 16, &samples, SRGB)).expect("a picture");
    assert_eq!(converted.pixels, samples.map(rounded_by_the_rule));
    assert_eq!(converted.pixels, [254, 1, 1, 128, 18, 254]);
}

/// `png` with its header's colour type set to `color_type` and the header's CRC made again.
fn with_color_type(mut png: Vec<u8>, color_type: u8) -> Vec<u8> {
    // The signature (8 bytes), then IHDR's length (4), its name (4) and its 13 bytes, the colour
    // type the tenth of them; its CRC covers the name and the data.
    png[25] = color_type;
    let crc = crc32(&png[12..29]);
    png[29..33].copy_from_slice(&crc.to_be_bytes());
    png
}

/// Fix round 1, item 7: among "anything else is unreadable", colour types other than grey: RGB
/// with an alpha channel, a palette, and grey with an alpha channel, at 8 and 16 bits, each a
/// well-formed header, are unreadable, never a guess.
#[test]
fn f3a_t1_a_picture_with_alpha_or_a_palette_is_unreadable() {
    for (color_type, form) in [
        (6, "RGB with alpha"),
        (3, "a palette"),
        (4, "grey with alpha"),
    ] {
        for bit_depth in [8, 16] {
            let bytes = with_color_type(
                png(2, 1, bit_depth, &[1, 2, 3, 4, 5, 6], Space::SrgbChunk),
                color_type,
            );
            assert_eq!(
                picture::to_srgb8(&bytes).err(),
                Some(PictureProblem::Unreadable),
                "{form}, {bit_depth} bits"
            );
        }
    }
}

// ---- 2: the black picture ----

/// Test 2: an all-zero picture is no frame, `capture_frame_all_black` (`FrameRefusal::AllBlack`),
/// leaves nothing unconfirmed (nothing to fence), and the next capture on the same backend gives
/// its frame. P3 black is sRGB black.
#[test]
fn f3a_t2_an_all_black_picture_is_no_frame_and_the_next_capture_succeeds() {
    let launcher = ScriptedLauncher::with(vec![
        Ok(Script::answering(
            answer(&png(2, 1, 8, &[0; 6], Space::SrgbChunk)),
            true,
        )),
        Ok(Script::answering(answer(&png(2, 1, 16, &[0; 6], P3)), true)),
        Ok(Script::answering(answer(&small_picture()), true)),
    ]);
    let mut backend = backend(&launcher);
    for _ in 0..2 {
        let black = backend.capture().expect_err("a black picture is no frame");
        assert_eq!(black.frame_refused(), Some(FrameRefusal::AllBlack));
        assert_eq!(black.ios_capture_failure(), None);
        assert_eq!(black.resource_quiescence(), None, "nothing to fence");
    }
    let frame = backend.capture().expect("the next capture's frame");
    assert_eq!(frame.pixels, [10, 20, 30, 40, 50, 60]);
    assert_eq!(
        launcher.ends.load(Ordering::Acquire),
        0,
        "no child was ended"
    );
}

// ---- 3: every word and every way of the child ----

/// Test 3: each of `acib`'s words for a screenshot maps to its one cause, the word kept beside it;
/// `deadline_passed` keeps its stage.
#[test]
fn f3a_t3_each_word_of_acib_maps_to_its_one_cause() {
    use IosCaptureProblem as Cause;
    let table = [
        ("command_line_invalid", Cause::AcibCallRefused),
        ("async_runtime_failed", Cause::AcibCallRefused),
        ("key_salt_unusable", Cause::AcibCallRefused),
        ("deadline_passed", Cause::AcibDeadlinePassed),
        ("device_key_unknown", Cause::IosDeviceNotListed),
        ("device_not_listed", Cause::IosDeviceNotListed),
        ("usbmuxd_unreachable", Cause::IosDeviceServiceUnreachable),
        ("usbmuxd_list_failed", Cause::IosDeviceServiceUnreachable),
        ("pair_record_unreadable", Cause::IosDeviceServiceUnreachable),
        ("not_paired", Cause::IosNotPaired),
        ("lockdown_unreachable", Cause::IosTunnelFailed),
        ("lockdown_read_failed", Cause::IosTunnelFailed),
        ("lockdown_value_unexpected", Cause::IosTunnelFailed),
        ("session_refused", Cause::IosTunnelFailed),
        ("service_refused", Cause::IosTunnelFailed),
        ("tunnel_failed", Cause::IosTunnelFailed),
        ("rsd_unreachable", Cause::IosTunnelFailed),
        ("rsd_handshake_failed", Cause::IosTunnelFailed),
        // Fix round 1: a word of the screenshot's own service (`apps/acib/README.md`).
        ("service_unreachable", Cause::IosScreenCaptureFailed),
        ("service_not_listed", Cause::IosScreenCaptureFailed),
        ("service_handshake_failed", Cause::IosScreenCaptureFailed),
        ("channel_refused", Cause::IosScreenCaptureFailed),
        ("screenshot_failed", Cause::IosScreenCaptureFailed),
        ("picture_header_truncated", Cause::IosPictureUnreadable),
        ("picture_header_unknown", Cause::IosPictureUnreadable),
    ];
    assert_eq!(table.len(), AcibProblem::ALL.len(), "every word of acib's");
    for (word, wanted) in table {
        let stage = (word == "deadline_passed").then_some("tunnel");
        let launcher =
            ScriptedLauncher::with(vec![Ok(Script::answering(refused(word, stage), false))]);
        let error = backend(&launcher).capture().expect_err(word);
        let failure = cause(&error);
        assert_eq!(failure.problem, wanted, "{word}");
        assert_eq!(failure.acib_problem.map(AcibProblem::as_str), Some(word));
        assert_eq!(
            failure.acib_stage,
            stage.map(|_| AcibStage::Tunnel),
            "{word}"
        );
        assert!(!error.message().contains(STDERR_MARKER), "{word}");
    }
}

/// Test 3: each way the child can fail is its one cause: no program, a program that does not
/// start, a state that cannot be read, a child ended by another party, fewer bytes than announced,
/// lines that are not an answer, a picture that is not PNG or not decodable.
#[test]
fn f3a_t3_each_way_the_child_fails_is_its_one_cause() {
    use IosCaptureProblem as Cause;
    let missing = AcibCaptureSource::new(None, AcibKeySalt::new(SALT));
    let error = open_acib_screen_capture(&missing, KEY)
        .backend
        .capture()
        .expect_err("no program");
    assert_eq!(cause(&error).problem, Cause::AcibMissing);
    let absent = AcibCaptureSource::new(
        Some(
            std::env::temp_dir()
                .join("neutral-no-acib-here")
                .join("acib.exe"),
        ),
        AcibKeySalt::new(SALT),
    );
    let error = open_acib_screen_capture(&absent, KEY)
        .backend
        .capture()
        .expect_err("no file");
    assert_eq!(cause(&error).problem, Cause::AcibMissing);

    let picture = small_picture();
    let mut cut = answer(&picture);
    cut.truncate(cut.len() - 3);
    let mut state_fails = Script::answering(answer(&picture), true);
    state_fails.status = None;
    state_fails.state_fails = true;
    let mut errors_fail = Script::answering(answer(&picture), true);
    errors_fail.errors_fail = true;
    let cases: Vec<(&str, io::Result<Script>, Cause)> = vec![
        // Fix round 1: its errors could not be read; its answer, whole, is not taken.
        (
            "errors unreadable",
            Ok(errors_fail),
            Cause::AcibErrorsUnreadable,
        ),
        (
            "not started",
            Err(io::Error::new(io::ErrorKind::NotFound, "scripted")),
            Cause::AcibNotStarted,
        ),
        (
            "state unreadable",
            Ok(state_fails),
            Cause::AcibStateUnreadable,
        ),
        (
            "ended by another party",
            Ok(Script::answering(answer(&picture)[..20].to_vec(), false)),
            Cause::AcibAnswerIncomplete,
        ),
        (
            "fewer bytes than announced",
            Ok(Script::answering(cut, true)),
            Cause::AcibAnswerIncomplete,
        ),
        (
            "no line end",
            Ok(Script::answering(
                b"{\"command\":\"screenshot\"".to_vec(),
                true,
            )),
            Cause::AcibAnswerIncomplete,
        ),
        (
            "not JSON",
            Ok(Script::answering(b"running 1 test\n".to_vec(), true)),
            Cause::AcibAnswerUnreadable,
        ),
        (
            "a word of none of its answers",
            Ok(Script::answering(refused("neutral_new_word", None), false)),
            Cause::AcibAnswerUnreadable,
        ),
        (
            "a picture to a file",
            Ok(Script::answering(
                answer_as(&picture, "png", "screen.png"),
                true,
            )),
            Cause::AcibAnswerUnreadable,
        ),
        (
            "a JPEG",
            Ok(Script::answering(answer_as(&picture, "jpeg", "-"), true)),
            Cause::IosPictureUnreadable,
        ),
        (
            "no picture",
            Ok(Script::answering(answer(b"0123456789"), true)),
            Cause::IosPictureUnreadable,
        ),
    ];
    for (case, script, wanted) in cases {
        let launcher = ScriptedLauncher::with(vec![script]);
        let error = backend(&launcher).capture().expect_err(case);
        assert_eq!(cause(&error).problem, wanted, "{case}");
        assert_ne!(
            error.resource_quiescence(),
            Some(DeviceResourceQuiescence::Unconfirmed),
            "{case}: nothing unconfirmed"
        );
    }
}

/// Test 3: a hang ends in the Runtime's own cause with the child ended and its end confirmed; a
/// child that cannot be ended is `acib_not_ended`, its resources unconfirmed (which fences its
/// instance in the host).
#[test]
fn f3a_t3_a_hang_is_ended_and_a_child_that_cannot_be_ended_is_unconfirmed() {
    let mut hangs = Script::answering(Vec::new(), true);
    hangs.status = None;
    let launcher = ScriptedLauncher::with(vec![Ok(hangs)]);
    let error = backend(&launcher).capture().expect_err("past the wait");
    assert_eq!(cause(&error).problem, IosCaptureProblem::AcibNoAnswerInTime);
    assert_ne!(
        error.resource_quiescence(),
        Some(DeviceResourceQuiescence::Unconfirmed)
    );
    assert_eq!(
        launcher.ends.load(Ordering::Acquire),
        1,
        "the child was ended"
    );

    let mut stuck = Script::answering(Vec::new(), true);
    stuck.status = None;
    stuck.end = End::Unconfirmed;
    let launcher = ScriptedLauncher::with(vec![Ok(stuck)]);
    let error = backend(&launcher).capture().expect_err("not ended");
    assert_eq!(cause(&error).problem, IosCaptureProblem::AcibNotEnded);
    assert_eq!(
        error.resource_quiescence(),
        Some(DeviceResourceQuiescence::Unconfirmed)
    );
}

// ---- 4: the caps ----

/// Test 4: an answer cut short and an oversized one are caught by the caps: a line over 4 KiB, a
/// picture announced over 64 MiB, more bytes than announced, and an output over the reader's cap
/// (the child ended).
#[test]
fn f3a_t4_the_caps_catch_an_oversized_answer() {
    use IosCaptureProblem as Cause;
    let picture = small_picture();
    let mut long_line = vec![b' '; ACIB_ANSWER_LINE_LIMIT_BYTES + 1];
    long_line.push(b'\n');
    let announced = format!(
        "{{\"command\":\"screenshot\",\"device\":3,\"via\":\"screen_capture\",\"format\":\"png\",\"bytes\":{},\"out\":\"-\"}}\n",
        ACIB_PICTURE_LIMIT_BYTES + 1
    )
    .into_bytes();
    let mut more = answer(&picture);
    more.extend_from_slice(b"more");
    for (case, output) in [
        ("a long line", long_line),
        ("a picture over the cap", announced),
        ("more than announced", more),
    ] {
        let launcher = ScriptedLauncher::with(vec![Ok(Script::answering(output, true))]);
        let error = backend(&launcher).capture().expect_err(case);
        assert_eq!(cause(&error).problem, Cause::AcibAnswerTooLong, "{case}");
    }
    let launcher = ScriptedLauncher::with(vec![Ok(Script::answering(vec![b'x'; 300], true))]);
    let small = AcibBounds {
        line_limit: 64,
        picture_limit: 64,
        ..bounds()
    };
    let error = open_acib_screen_capture(&source(Arc::clone(&launcher)).with_bounds(small), KEY)
        .backend
        .capture()
        .expect_err("over the reader's cap");
    assert_eq!(cause(&error).problem, Cause::AcibAnswerTooLong);
}

// ---- 5: the salt, the command line and the errors ----

/// Test 5: the salt is in no argument and in the child's environment alone, under the Runtime's
/// name; the command's `Debug` and the source's `Debug` withhold it; the key names the phone; the
/// child's errors (a marker) reach no error text.
#[test]
fn f3a_t5_the_salt_is_in_no_argument_and_the_errors_reach_no_text() {
    let mut failing = Script::answering(refused("screenshot_failed", None), false);
    failing.errors = format!("{STDERR_MARKER} {SALT}").into_bytes();
    let launcher = ScriptedLauncher::with(vec![
        Ok(Script::answering(answer(&small_picture()), true)),
        Ok(failing),
    ]);
    let mut backend = backend(&launcher);
    backend.capture().expect("a frame");
    let error = backend.capture().expect_err("a refusal");
    let launched = launcher.launched.lock().expect("launches");
    for launch in launched.iter() {
        assert_eq!(
            launch.program,
            std::env::current_exe().expect("the program")
        );
        assert!(
            launch
                .arguments
                .iter()
                .all(|argument| !argument.contains(SALT))
        );
        assert_eq!(
            launch.arguments,
            [
                "screenshot",
                "--device-key",
                KEY,
                "--key-salt-env",
                ACIB_KEY_SALT_VARIABLE,
                "--out",
                "-",
                "--via",
                "screen_capture",
                "--timeout-ms",
                "8000",
            ]
        );
        assert_eq!(
            launch.environment,
            [(ACIB_KEY_SALT_VARIABLE, SALT.to_owned())]
        );
        assert!(!launch.debug.contains(SALT), "{}", launch.debug);
    }
    assert!(!format!("{:?}", source(Arc::clone(&launcher))).contains(SALT));
    for text in [
        error.message().to_owned(),
        format!("{error:?}"),
        error.to_string(),
    ] {
        assert!(
            !text.contains(STDERR_MARKER) && !text.contains(SALT),
            "{text}"
        );
    }
}

// ---- 7: a real child ----

/// The environment variable that makes [`f3a_t7_child_entry`] wait forever.
const CHILD_WAITS: &str = "ACTINGCOMMAND_F3A_CHILD_WAITS";

/// Test 7's child: this test executable started again on this entry, which waits forever (without
/// a clock) when [`CHILD_WAITS`] is set, as an `acib` that hangs.
#[test]
#[ignore]
fn f3a_t7_child_entry() {
    if std::env::var_os(CHILD_WAITS).is_some() {
        loop {
            std::thread::park();
        }
    }
}

/// Starts this test executable again on [`f3a_t7_child_entry`] through the system's own start
/// ([`SystemAcibLauncher`]'s), whatever command the runner gives.
struct RealChild {
    ended: Arc<AtomicUsize>,
}

impl AcibLauncher for RealChild {
    fn launch(&self, command: &AcibCommand) -> io::Result<Box<dyn AcibProcess>> {
        // The command's own environment, and the variable that makes the child wait.
        let mut environment = command.environment.clone();
        environment.push((CHILD_WAITS, AcibKeySalt::new("1")));
        let child = AcibCommand {
            program: std::env::current_exe()?,
            arguments: [
                "--ignored",
                "--exact",
                "capture::acib::gate_479_f3a::f3a_t7_child_entry",
                "--nocapture",
            ]
            .map(str::to_owned)
            .to_vec(),
            environment,
        };
        Ok(Box::new(Recorded {
            inner: SystemAcibLauncher.launch(&child)?,
            ended: Arc::clone(&self.ended),
        }))
    }
}

/// A real child whose confirmed ends are counted.
struct Recorded {
    inner: Box<dyn AcibProcess>,
    ended: Arc<AtomicUsize>,
}

impl AcibProcess for Recorded {
    fn output(&mut self) -> Option<Box<dyn Read + Send>> {
        self.inner.output()
    }

    fn errors(&mut self) -> Option<Box<dyn Read + Send>> {
        self.inner.errors()
    }

    fn ended(&mut self) -> io::Result<Option<bool>> {
        self.inner.ended()
    }

    fn elapsed(&self) -> Duration {
        self.inner.elapsed()
    }

    fn pause(&mut self, span: Duration) -> io::Result<()> {
        self.inner.pause(span)
    }

    fn end(&mut self) -> DeviceResult<DeviceResourceCloseOutcome> {
        let ended = self.inner.end()?;
        if ended.quiescence() == DeviceResourceQuiescence::Confirmed {
            self.ended.fetch_add(1, Ordering::AcqRel);
        }
        Ok(ended)
    }
}

/// Test 7 (F-2's leftover): a real child that hangs is ended by the Runtime past its wait and its
/// end is confirmed (the system's kill and its exit seen), with the Runtime's own cause and nothing
/// left unconfirmed.
#[test]
fn f3a_t7_a_real_child_that_hangs_is_ended_and_its_end_confirmed() {
    let ended = Arc::new(AtomicUsize::new(0));
    let source = AcibCaptureSource::new(
        Some(std::env::current_exe().expect("the test executable")),
        AcibKeySalt::new(SALT),
    )
    .with_bounds(AcibBounds {
        wait: Duration::from_millis(500),
        ..AcibBounds::runtime()
    })
    .with_launcher(Arc::new(RealChild {
        ended: Arc::clone(&ended),
    }));
    let error = open_acib_screen_capture(&source, KEY)
        .backend
        .capture()
        .expect_err("a child that hangs gives no frame");
    assert_eq!(cause(&error).problem, IosCaptureProblem::AcibNoAnswerInTime);
    assert_ne!(
        error.resource_quiescence(),
        Some(DeviceResourceQuiescence::Unconfirmed),
        "{error:?}"
    );
    assert_eq!(ended.load(Ordering::Acquire), 1, "ended, its end confirmed");
}
