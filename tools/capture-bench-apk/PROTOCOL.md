# capture-bench test pattern: on-device protocol v1

The test-pattern app of Workflow #402 step 1 (plan v2.1 §3.2 item 5, slice C3). The capture
bench (C2) drives it and the analysis (C4) reads what it records. This document is the contract
between them: the screen, the commands and the records. It lives on the experiment branch
`exp/402-test-apk`, which is never merged.

- **Game-neutral.** The app contains no game content. The images of the static and replay modes
  are pushed by the bench at run time.
- **No permission, no network.** The manifest declares no permission; `build.sh` fails a build
  whose APK lists one. The time service is an abstract-namespace local socket, which needs none.
- **Experiment instances only.** The coordinator installs it on the two experiment instances,
  with `adb -s` naming the instance. Nothing in this repository runs adb.

## 1. Identity

| What | Value |
|---|---|
| Package | `io.github.hs7097.capturebench.pattern` |
| Activity | `io.github.hs7097.capturebench.pattern/.app.PatternActivity` |
| Time service socket | `localabstract:capture_bench_pattern` |
| Wire protocol | `1` (HELLO `protocol`) |
| Record format | `capture-bench-pattern-records/1` (HELLO `record_format`) |
| Timecode | version `1` (HELLO `timecode_version`, and in every code) |
| SDK | minSdk 26, targetSdk 35; the experiment instances run Android 15 (API 35) |

## 2. Install and launch

These are the bench's and the coordinator's steps, written here so that C2 can rely on them.

- **Install:** `adb -s <E serial> install capture-bench-pattern.apk`. The CI artifact carries
  `apk-info.txt` (source SHA, APK SHA-256, signer SHA-256) and `SHA256SUMS`.
- **One key per build.** Each CI build is signed with a throw-away debug key made for that
  build. Installing a different build over an installed one fails with
  `INSTALL_FAILED_UPDATE_INCOMPATIBLE`; whether to uninstall the old one (which also removes the
  pushed images) is the coordinator's decision.
- **Launch:** `adb -s <E serial> shell am start -n io.github.hs7097.capturebench.pattern/.app.PatternActivity`.
  Optional extras: `--es mode pattern|flip|step|static` (static without a slot shows the built-in
  still card) and `--es orient landscape|portrait|reverse_landscape|reverse_portrait|unspecified`.
  The activity is `singleTask`: a second `am start` delivers the extras to the running activity.
  A bad extra is recorded as an `error` event (`bad_launch_extra`).
- **Connect:** `adb -s <E serial> forward tcp:<port> localabstract:capture_bench_pattern`, then
  TCP to `127.0.0.1:<port>`. Removing the forward is the bench's cleanup.
- The app runs full screen (system bars hidden, display cutout used, screen kept on). The view
  normally sits at screen (0, 0); INFO reports its actual offset.

## 3. Clocks

Every device time in this protocol is **CLOCK_MONOTONIC in nanoseconds**, the clock of
`System.nanoTime()`, and its name ends in `_dev_ns`. Its zero is the device's boot (the clock
does not advance in deep sleep, which an emulator does not enter while it runs); it restarts when
the instance restarts, so offsets never carry over from one boot to the next. The Choreographer
`frameTimeNanos`, the FrameMetrics timestamps and the MotionEvent event time (uptime) use the same
clock. Durations end in `_ns`.

| Plan v2.1 §5.1 name | Where it is | Meaning |
|---|---|---|
| `T_vsync(f)` | timecode field `t_vsync`; vsync record `t_vsync_dev_ns` | The Choreographer `frameTimeNanos` of the frame callback that latched frame f. It is the vsync the frame's rendering started from, not draw completion, not present. |
| `T_drawn(f)` | vsync record `t_drawn_dev_ns` | FrameMetrics `INTENDED_VSYNC_TIMESTAMP + TOTAL_DURATION` of the HWUI frame whose `VSYNC_TIMESTAMP` equals `T_vsync(f)`. `TOTAL_DURATION` runs from the intended vsync to HWUI's `FRAME_COMPLETED`, "the total time ... this frame took to render and be issued to the display subsystem". It is not present (`T_present` is not measured in step 1). Null with a reason when not available (§6.5). |
| `T_evt(n)` | flip record `t_evt_dev_ns` | `MotionEvent.getEventTimeNanos()` of the n-th tap's ACTION_DOWN (API 34+, `t_evt_resolution` = `ns`); on older APIs `getEventTime()` x 10^6 (`t_evt_resolution` = `ms`). |
| `T_handled(n)` | flip record `t_handled_dev_ns` | The moment the app assigned n to the tap, in the touch handler. |
| `T_vsync` of the first draw of n | flip record `t_vsync_first_draw_dev_ns` | The `T_vsync` of the first frame drawn with flip number n. |
| `T_vsync(k)` | step record `t_vsync_dev_ns` | The `T_vsync` of the first frame drawn with step k. |
| device time of a calibration sample | PING `t_rx_dev_ns`, `t_tx_dev_ns` | The clock read when the request line was complete, and just before the reply was written. |

## 4. Screen

### 4.1 Modes

| Mode | What is drawn | Redraws |
|---|---|---|
| `pattern` (default) | Grey background, the timecode band, the track with the moving block. | Every vsync. |
| `flip` | As `pattern`, plus the 4 x 4 tap-grid lines and the flip block. Each tap toggles the flip block between black and white and increments n. | Every vsync. |
| `step` | The whole screen black or white (the step level), with the timecode band on top. STEP switches the level at the next vsync. | Every vsync. |
| `static` | The pushed image of a slot at (0, 0), 1:1, no scaling, no filtering, black elsewhere; no timecode. Without a slot: the built-in still card (8 vertical grey bars, levels `i * 255 / 7`). | Only when the mode or the geometry changes. |
| `replay` | The pushed images of the slots that share a prefix, in name order, each 1:1 at (0, 0), at `fps` (default 30); no timecode. | When the due image changes. |

Every change (a tap, a step, a mode, a geometry change) is latched in a Choreographer frame
callback and drawn in the traversal of the same frame, so the frame that first shows a change
carries that callback's `frameTimeNanos` in its code. A redraw that the system triggers without a
frame callback shows the previous frame's content unchanged (counted in INFO
`redraws_without_latch`; it produces no vsync record).

The static and replay modes draw no code: the analysis judges those frames with the image
digest (plan §5.4) and against the pushed source (plan §5.8).

### 4.2 Layout

All positions are view pixels; frame pixels = view pixels + (INFO `view.x`, `view.y`). The
layout depends only on the view size W x H and the cell edge C (default 24, `SET cell` 24..256),
never on the mode. Rectangles are `[left, top, right, bottom)`.

| Block | Rectangle |
|---|---|
| Band (timecode) | `[C, C, 25C, 9C)`: 24 columns x 8 rows of C x C cells |
| Code cell (r, c) | `[C + cC, C + rC, 2C + cC, 2C + rC)` |
| Sample square of a code cell | the cell inset by `floor(C / 4)` on every side |
| Flip block (8C square) | beside the band, `[26C, C, 34C, 9C)`, when `35C <= W`; else below it, `[C, 10C, 9C, 18C)` |
| Track (4C high) | `[C, T, W - C, T + 4C)` with `T = max(band bottom, flip bottom) + C` |
| Moving block (4C square) | left edge `track.left + (counter * speed) mod (track width - 4C)`, top = track top; 2 x 2 checker, white top-left and bottom-right; `speed` default 8 px a frame (`SET speed` 0..256) |
| Tap grid cell j (0..15) | column `j % 4`, row `j / 4`; edges at `ceil(k * W / 4)` and `ceil(k * H / 4)`; a tap at (x, y) is in column `floor(4x / W)`, row `floor(4y / H)` |

The layout must fit: `26C <= W`, `T + 5C <= H`, track at least 8C long. Otherwise nothing but a
dark red screen is drawn, an `error` event `layout_does_not_fit` is recorded and INFO `layout` is
null. For the experiment screens, C = 24:

| Screen | Band | Flip block | Track |
|---|---|---|---|
| 1280 x 720 (landscape) | `[24,24,600,216)` | `[624,24,816,216)` | `[24,240,1256,336)` |
| 720 x 1280 (portrait) | `[24,24,600,216)` | `[24,240,216,432)` | `[24,456,696,552)` |

Colours: white `#FFFFFF`, black `#000000`, background `#808080`, track `#606060`, grid lines
`#707070` (1 px). The flip block is white when the flip level is 1. In step mode the background
is white when the step level is 1.

### 4.3 Timecode v1

Drawn in the pattern, flip and step modes, in every frame. Cell value: white = 1, black = 0.

- **Row 0** is the sync row, the 24 bits of `0xB38F0F`, most significant bit at column 0:
  `101100111000111100001111`.
- **Rows 1..7** carry the 21-byte codeword, 168 bits, most significant bit first, row by row,
  left to right (bit i is row `1 + i / 24`, column `i % 24`).
- **Codeword** = the 17-byte payload, then the CRC-32 of the payload (IEEE 802.3, the same as
  `zlib.crc32`; check value of `123456789` is `cbf43926`), big-endian. These 32 bits are the
  check bits of plan §3.2 item 5.

| Payload byte | Field | Meaning |
|---|---|---|
| 0 | version (high nibble), mode (low nibble) | version 1; mode 1 = pattern, 2 = flip, 3 = step |
| 1..4 | `seq` (the frame counter) | the vsync record's `counter` mod 2^32, big-endian |
| 5..12 | `t_vsync` | `T_vsync` in ns, int64 big-endian |
| 13..15 | `event` | flip: `n mod 2^24`; step: `k mod 2^24`; pattern: 0 |
| 16 | `aux` | flip: j in bits 7..4, flip level in bit 3; step: step level in bit 3; others 0 |

Before the first tap, a flip code has n = 0, j = 0, level 0.

**Decoding** (the Java reference is `core/Timecode.decode`; C2 implements the same rules):

1. Take each cell's sample square (§4.2) in the frame, and its mean luma (any luma of R, G, B
   works for black and white; use BT.601 `0.299R + 0.587G + 0.114B` for definiteness).
2. Sync row: W = mean of the sync cells whose bit is 1, B = mean of those whose bit is 0. If
   `W - B < 64`, the result is `no_sync`. Threshold `T = (W + B) / 2`. If any sync cell falls on
   the wrong side of T, the result is `no_sync`.
3. Each data cell is 1 when its mean is `>= T`.
4. CRC mismatch: `crc_mismatch`. Version other than 1: `unsupported_version`. Mode outside 1..3:
   `bad_mode`. Otherwise `ok` with the fields.
5. Report the margin, the smallest `|mean - T|` over all cells, as the decode quality.

A frame whose code does not decode `ok` is "undetermined" for the time classification (plan
§5.3). The sample square leaves a quarter cell on every side, so blur and misregistration of a
few pixels do not reach it (the unit tests decode a band blurred with a 9 x 9 box, shifted by
(3, -2) px, with +-40 noise and the range squeezed to 30..225).

**Test vectors** (computed by an independent Python implementation; the Java tests check them):

| Vector | mode, seq, t_vsync, event, aux | Codeword (hex) |
|---|---|---|
| A | 2, `0x01020304`, `0x1122334455667788`, `0xABCDEF`, `0x5A` | `12010203041122334455667788abcdef5aca17e576` |
| B | 1, 1, 123456789012345, 0, 0 | `110000000100007048860ddf7900000000f4a49f58` |
| C | 3, `0xFFFFFFFF`, `0x7FFFFFFFFFFFFFFF`, `0xFFFFFF`, `0x08` | `13ffffffff7fffffffffffffffffffff081d32b654` |

Cells of vector A, rows 0..7:

```
101100111000111100001111
000100100000000100000010
000000110000010000010001
001000100011001101000100
010101010110011001110111
100010001010101111001101
111011110101101011001010
000101111110010101110110
```

### 4.4 Flip mode

- Only ACTION_DOWN counts; one tap = one n. Taps in other modes are ignored (INFO
  `taps_ignored`); n counts the flip-mode taps since the process started and never resets, so n
  is unique per HELLO `instance`.
- The tap's grid cell is j (§4.2). The bench taps a cell centre in frame pixels; the view
  receives view pixels, which differ by the INFO offset.
- At the next vsync the app latches n and j and toggles the flip block; that frame's code has
  `event = n mod 2^24` and `aux = j << 4 | level << 3`.
- If two taps are latched in the same frame, the frame shows only the later n; the earlier
  record has null first-draw fields with reason `superseded` and `superseded_by: <later n>`.

### 4.5 Step mode

- `MODE step [level=0|1]` enters it (default level 0, black). STEP is refused with `wrong_mode`
  unless the committed mode is step and no MODE is pending: wait for INFO `mode` = `step` first.
- `STEP` toggles the level, `STEP level=0|1` sets it, at the next vsync. k counts STEPs since
  the process started (unique per `instance`). A second STEP before the first is latched is
  refused with `step_pending`.
- The STEP response returns k at once; the step record (§6.4) gives `T_vsync(k)` and
  `t_drawn_dev_ns` when the frame is final. This is the APK side of the `V_hi` trials (plan §5.3).

### 4.6 Static and replay modes

- `PUT` stores an image in a slot (app-private storage; any format the device decodes, normally
  PNG). The image is shown 1:1 at (0, 0): an image larger than the view is cropped, a smaller one
  leaves black. The mode record carries `image_width`, `image_height`, `view_width` and
  `view_height`, so a size mismatch is visible.
- `MODE static slot=<s>` decodes the slot before it answers; `MODE static` shows the still card.
  The screen does not change afterwards except on a geometry change.
- `MODE replay prefix=<p> [fps=<1..120>] [loops=<n>]` plays every slot whose name starts with p,
  in name order (so name them `r_0000.png`, `r_0001.png`, ...). `loops=0` (default) repeats
  forever. Image 0 is decoded before the answer and shown at the first vsync of the mode; index i
  (counting across loops) is due from `start + i / fps` on the vsync clock. Two background
  threads decode up to 4 images ahead. An image that is not ready when due is reported (`replay`
  event `late`, or `decode_failed`) and the previous one stays; when a later index is ready
  first, the skipped indices are reported (`skip`). Each displayed image has a vsync record.

### 4.7 Orientation

`ORIENT <value>` calls `setRequestedOrientation`. The request is an `orient` event; the
resulting size change is a `config` record (cause `size`) whose first-draw fields give the first
frame drawn in the new geometry. The activity handles the change itself (it is not recreated), so
the records, the slots and the time service survive a rotation.

## 5. Wire protocol

### 5.1 Framing

- A request is one line: printable ASCII tokens separated by exactly one space, ended by LF (no
  CR), at most 4096 bytes. Verbs are upper case.
- Each request gets exactly one response line, UTF-8 JSON, ended by LF. Every response starts with
  `"ok"` and `"cmd"` (the verb).
- Requests on one connection are answered strictly in order. Up to 8 connections are served at
  once, each on its own thread (for example one for calibration and one for records); a ninth is
  answered `too_many_connections` and closed.
- Only peers with uid 0 or 2000 (adbd runs as one of them) or the app's own uid are served;
  others get `peer_not_allowed` (with their `uid`) and are closed, and an `error` event is
  recorded.

### 5.2 Commands

| Request | Response fields after `ok`, `cmd` | Notes |
|---|---|---|
| `HELLO` | `protocol`, `record_format`, `timecode_version`, `package`, `version_name`, `source_sha`, `sdk_int`, `release`, `instance`, `pid`, `uid`, `process_start_dev_ns`, `clock` (`CLOCK_MONOTONIC`), `t_evt_resolution`, `frame_metrics` (bool), `server_error` (null while serving), `t_now_dev_ns` | `instance` is 16 random hex digits per process: a new value means the app restarted and the record cursors start over. |
| `PING <token>` | `token`, `t_rx_dev_ns`, `t_tx_dev_ns` | token `[A-Za-z0-9_-]{1,32}`. Answered on the connection thread with nothing between the two clock reads and the write. |
| `INFO` | `instance`, `display` {`rotation`, `real_width`, `real_height`, `refresh_hz`}, `orientation` {`requested`, `configuration`}, `mode`, `mode_seq`, `mode_pending` (mode or null), `view` {`width`, `height`, `x`, `y`}, `cell`, `speed`, `layout` (as §4.2, or null), `layout_error`, `config_seq`, `counter`, `last_drawn_vsync_dev_ns`, `flip_n`, `flip_j`, `flip_level`, `taps`, `taps_ignored`, `step_k`, `step_level`, `step_pending`, `static_slot`, `replay` (null or {`prefix`, `images`, `fps`, `loops`, `start_vsync_dev_ns`, `shown_index`, `done`}), `redraws_without_latch`, `records` {`vsync_newest`, `vsync_oldest`, `vsync_capacity`, `event_newest`, `event_oldest`, `event_capacity`, `frames_pending`, `records_awaiting_draw`, `frame_metrics`, `metrics_matched`, `metrics_unmatched`, `metrics_dropped_reported`, `frames_without_metrics`, `records_not_drawn`} | `layout` = {`width`, `height`, `cell`, `band`, `code_columns`, `code_rows`, `sample_inset`, `flip_block`, `flip_beside_band`, `track`, `moving_size`, `speed`, `grid`}, rectangles as `[l, t, r, b]`. |
| `MODE pattern` / `MODE flip` | `mode_seq`, `mode` | Takes effect at the next vsync; the `mode` record gives the first frame. |
| `MODE step [level=0\|1]` | `mode_seq`, `mode` | |
| `MODE static [slot=<s>]` | `mode_seq`, `mode` | `no_such_slot`, `image_decode_failed` before anything changes. |
| `MODE replay prefix=<p> [fps=<1..120>] [loops=<0..1000000>]` | `mode_seq`, `mode` | `no_such_slots` when no slot matches. |
| `STEP [level=0\|1]` | `k`, `level` | §4.5. |
| `ORIENT landscape\|portrait\|reverse_landscape\|reverse_portrait\|unspecified` | `requested`, `previous` | §4.7. |
| `SET cell <24..256>` / `SET speed <0..256>` | `key`, `value` | A cell size that does not fit is refused with `layout_does_not_fit`; a new cell size is a `config` record with cause `set_cell`. |
| `PUT <slot> <length>` then exactly `length` bytes | `slot`, `bytes`, `width`, `height`, `mime`, `sha256` | slot `[A-Za-z0-9_-][A-Za-z0-9_.-]{0,63}`, length 1..33554432. A refused PUT header closes the connection (its payload would be read as requests). A payload that is not an image is refused with `image_decode_failed` and the slot is unchanged; the connection stays usable. A PUT of an existing slot replaces it. |
| `SLOTS` | `count`, `slots` [{`slot`, `bytes`, `width`, `height`, `mime`, `sha256`}] | In name order. |
| `RECORDS vsync\|event <after> [max]` | `stream`, `after`, `next`, `oldest`, `newest`, `lost`, `count`, `records` [...] | §6.1; max 1..5000, default 1000. |
| `BYE` | (none) | Then the connection is closed. |

`slot`, `prefix`, `level`, `fps` and `loops` are `key=value` options in any order; an unknown,
repeated or malformed option is `bad_arguments`.

### 5.3 Errors

An error response is `{"ok":false,"cmd":<verb or null>,"error":<code>,"detail":<text>}`.

| Code | When | Connection |
|---|---|---|
| `bad_arguments` | malformed line, unknown option, value out of range | stays open, except for a refused PUT header and a stream that ended inside a line |
| `unknown_command` | unknown verb | open |
| `line_too_long` | a request line over 4096 bytes | closed |
| `payload_too_large` | PUT length over 32 MiB | closed |
| `payload_short` | the stream ended inside a PUT payload | closed |
| `peer_not_allowed`, `too_many_connections` | §5.1 | closed |
| `cursor_ahead` | RECORDS `after` beyond `newest` (a cursor from another `instance`) | open |
| `wrong_mode`, `step_pending` | §4.5 | open |
| `no_such_slot`, `no_such_slots`, `image_decode_failed` | §4.6 | open |
| `layout_does_not_fit` | SET cell | open |
| `no_view` | the activity is not running | open |
| `ui_timeout` | the UI thread did not take the request within 2 s; the request was **not** applied and never will be | open |
| `io_error`, `internal_error` | a storage or socket failure, a bug | closed |

## 6. Records

### 6.1 Streams and the cursor

- Two streams: `vsync` (one record per drawn frame, ring of 32 768) and `event` (everything else,
  ring of 16 384). Each record has `stream` and `seq` first, then `type`; seq counts 1, 2, 3...
  per stream and process.
- `RECORDS <stream> <after> [max]` returns the records with `seq > after`, at most max, and
  `next` = the seq of the last one returned (or `after` when none). Page with `after = next`.
- If the ring has overwritten records after the cursor, the page starts at `oldest` and `lost`
  counts the skipped ones. `lost > 0` is data loss, never silent: at 60 frames a second the vsync
  ring holds about 9 minutes, so fetch more often than that.
- A record never changes once it is in a stream. Records appear when they are final (§6.2), so
  the event stream is in finalization order, not request order; every record carries its own
  times.
- A null measurement has its reason in `null_reasons`, an object keyed by the field's dotted path
  (`{}` when nothing is null). Option fields that do not apply to a record (for example `fps` of
  a non-replay mode record) are null without a reason.

### 6.2 When a record is final

- **vsync:** when the FrameMetrics report with the frame's `VSYNC_TIMESTAMP` arrives; after 1 s
  without one (`metrics_not_reported`); at once when FrameMetrics is unavailable.
- **flip, step, mode, config** wait for the first drawn frame that shows them and are appended
  right after that frame's vsync record, with its `counter`, `T_vsync` and `t_drawn_dev_ns`. One
  that is never shown is appended with those three null and the reason: `superseded` (a later one
  of the same type was drawn in the same frame), `mode_changed` (a flip or step whose frame was
  drawn in another mode) or `not_drawn_within_timeout` (no frame showed it within 5 s).
- **Everything else** is appended at once.

### 6.3 The vsync record

```json
{"stream":"vsync","seq":1,"type":"vsync","counter":7,"mode":"pattern",
 "t_vsync_dev_ns":1000,"t_callback_dev_ns":1002,"t_draw_dev_ns":1005,"t_drawn_dev_ns":1030,
 "metrics":{"intended_vsync_dev_ns":990,"total_duration_ns":40,"gpu_duration_ns":3,"first_draw_frame":false},
 "content":{"code":true,"code_mode":1,"code_counter":7,"event":0,"aux":0},
 "null_reasons":{}}
```

(One line on the wire; the numbers are illustrative.)

| Field | Meaning |
|---|---|
| `counter` | Latched frames since the process started (the frame counter of plan §3.2 item 5); a gap means a latched frame was not drawn. The code carries it mod 2^32, as `content.code_counter`. In the animated modes consecutive counters are consecutive vsyncs, so the next record's `t_vsync_dev_ns` is `T_vsync(f+1)`. |
| `mode` | The mode the frame was drawn in. |
| `t_vsync_dev_ns` | `T_vsync`. |
| `t_callback_dev_ns` | The clock at the start of the frame callback. |
| `t_draw_dev_ns` | The clock at the start of the view's draw. |
| `t_drawn_dev_ns` | `T_drawn` (§3), or null. |
| `metrics` | The FrameMetrics report: `intended_vsync_dev_ns`, `total_duration_ns`, `gpu_duration_ns` (API 31+, else null with reason `not_reported_below_api_31`), `first_draw_frame`; null when there is no report. |
| `content` | pattern, flip, step: `{"code":true,"code_mode","code_counter","event","aux"}`, exactly what the code shows. static: `{"code":false,"slot":<slot or null for the still card>}`. replay: `{"code":false,"slot","replay_index","replay_image","replay_loop"}`. No layout: `{"code":false,"layout_error":...}`. |
| `null_reasons` | §6.1. |

### 6.4 Event records

Records that wait for a first draw end with the **first-draw fields**: `counter`, the first-draw
vsync (`t_vsync_first_draw_dev_ns` in flip records, `t_vsync_dev_ns` in the others, the names of
the C4 schema), `t_drawn_dev_ns`, `superseded_by` (null unless superseded) and `null_reasons`.

| type | Fields (in order; * = followed by the first-draw fields) |
|---|---|
| `flip` * | `n`, `j` (0..15), `x`, `y` (view pixels), `t_evt_dev_ns`, `t_evt_resolution` (`ns` or `ms`), `t_handled_dev_ns` |
| `step` * | `k`, `level`, `t_rx_dev_ns` (request line complete), `t_accepted_dev_ns` (on the UI thread) |
| `mode` * | `mode_seq`, `mode`, `level`, `slot`, `prefix`, `fps`, `loops`, `images`, `image_width`, `image_height`, `view_width`, `view_height`, `t_rx_dev_ns`, `t_accepted_dev_ns` |
| `config` * | `config_seq`, `cause` (`size` or `set_cell`), `width`, `height`, `view_x`, `view_y`, `rotation`, `cell`, `layout`, `layout_error`, `t_cfg_dev_ns` |
| `orient` | `requested`, `previous`, `t_rx_dev_ns`, `t_applied_dev_ns` |
| `configuration` | `orientation`, `screen_width_dp`, `screen_height_dp`, `t_dev_ns` |
| `put` | `slot`, `bytes`, `width`, `height`, `mime`, `sha256`, `t_done_dev_ns` |
| `set` | `key`, `value`, `t_dev_ns` (speed; a cell change is a `config` record) |
| `replay` | `event` = `start` (`mode_seq`, `prefix`, `images`, `fps`, `loops`, `t_vsync_dev_ns`), `done` (`mode_seq`, `last_index`, `t_vsync_dev_ns`), `late` or `decode_failed` (`mode_seq`, `index`, `slot`, `detail`, `shown_index`, `t_vsync_dev_ns`), `skip` (`mode_seq`, `from_index`, `to_index`, `t_vsync_dev_ns`) |
| `server` | `event` = `process_start` (`instance`, `pid`, `t_dev_ns`), `listening` (`socket`, `t_dev_ns`), `activity_attached` / `activity_detached` (`t_dev_ns`), `connection_failed` (`detail`, `t_dev_ns`) |
| `error` | `code`, `detail`, `t_dev_ns`; codes: `layout_does_not_fit`, `server_bind_failed`, `server_accept_failed`, `peer_not_allowed`, `io_error`, `internal_error`, `ui_call_abandoned`, `frame_metrics_unavailable`, `system_bars_not_hidden`, `bad_launch_extra` |

Example flip record (one line on the wire):

```json
{"stream":"event","seq":1,"type":"flip","n":1,"j":5,"x":400.5,"y":200.25,
 "t_evt_dev_ns":900,"t_evt_resolution":"ns","t_handled_dev_ns":950,
 "counter":7,"t_vsync_first_draw_dev_ns":1000,"t_drawn_dev_ns":1030,"superseded_by":null,"null_reasons":{}}
```

### 6.5 Null reasons

| Field | Reason | Meaning |
|---|---|---|
| `t_drawn_dev_ns`, `metrics` | `frame_metrics_unavailable` | The window gave no FrameMetrics. |
| | `metrics_not_reported` | No report for this `T_vsync` within 1 s (FrameMetrics drops reports under load). |
| | `vsync_reused` | Two drawn frames had the same `T_vsync`; the first is closed without a report. |
| `metrics.gpu_duration_ns` | `not_reported_below_api_31` | FrameMetrics has no GPU duration below API 31. |
| `counter`, first-draw vsync, `t_drawn_dev_ns` | `superseded`, `mode_changed`, `not_drawn_within_timeout` | The record was never shown (§6.2). |

## 7. Bench recipes

- **Calibration (plan §5.1, 1).** One connection, 200 PINGs in sequence. Host: QPC before the
  send (s) and after the reply (r). Device time of the sample:
  `(t_rx_dev_ns + t_tx_dev_ns) / 2`; `theta = device - (s + r) / 2`, `u = (r - s) / 2` over the
  minimum-round-trip samples, as the plan says. `t_tx_dev_ns - t_rx_dev_ns` (the device's own
  share of the round trip) is there per sample so the analysis can see it is negligible.
  Recalibrate every 10 min.
- **Step trials (plan §5.3, `V_hi`).** `MODE step level=0`; poll INFO until `mode` is `step`;
  then per trial `STEP`, note k, wait a few hundred ms; fetch `event` records until step k is
  there. Its `t_vsync_dev_ns` is `T_vsync(k)`. In the S frames, the first frame whose decoded
  code has `event = k` (or whose background has the new level) is the first that shows the step.
- **Task-style capture (C6).** `MODE flip`; per trial tap the centre of grid cell j; the flip
  record gives n, j, `T_evt(n)`, `T_handled(n)`, the first-draw `T_vsync` and `T_drawn`. In a
  captured frame, the decoded `event` and `aux` give (n mod 2^24, j, level): the event identity of
  plan §5.3.
- **Frame identity in any captured frame.** Decode the code: `t_vsync` identifies the frame and
  `seq` is its counter; `lo(f) = map(T_vsync(f)).lo` (plan §5.3). The vsync record with the same
  `t_vsync_dev_ns` gives `t_drawn_dev_ns`; the vsync record with `counter + 1` gives
  `T_vsync(f+1)` for `hi(f)`.
- **OCR and template truth (C8).** `PUT` each truth image, `MODE static slot=<s>`, wait for the
  mode record (its first-draw fields), then capture.
- **Replay (C8).** `PUT r_0000.png ...`; `MODE replay prefix=r_ fps=30 loops=1`; the vsync
  records list every displayed image with its `T_vsync`; `late`, `skip` and `decode_failed`
  events say where the schedule slipped.
- **Rotation (C10).** `ORIENT portrait` (or back); the `config` record with cause `size` gives
  the new size and the first frame drawn in it. Re-read INFO `layout` before decoding again.
- **Restart detection.** Compare HELLO `instance` with the one the run started with; a change
  means the app process restarted, both cursors start from 0 again, and n, k and the counter
  start over.

## 8. Against the C4 analysis schema (`apk-<instance>.jsonl`)

The C4 analysis reads the APK records from a file the bench writes, `apk-<instance>.jsonl`, with
the record types `flip`, `step` and `vsync` (its §7). The records above use its names, so the
bench can write those three types as they come, one per line; what differs:

| Point | This protocol | The C4 schema | What the bench does |
|---|---|---|---|
| Other record types | The event stream also has `mode`, `config`, `orient`, `configuration`, `put`, `set`, `replay`, `server`, `error`. | Unknown types in `apk-*.jsonl` are input problems. | Write only `flip`, `step` and `vsync` records to `apk-<instance>.jsonl`; keep the rest elsewhere (for example `events.jsonl` or a raw dump). |
| Envelope | Every record starts with `stream`, `seq`. | Not listed. | Extra fields are allowed; leave them. |
| `vsync` records | One per drawn frame, with `counter`, `t_vsync_dev_ns` and more (`t_drawn_dev_ns`, `metrics`, `content`, `null_reasons`). | `counter`, `t_vsync_dev_ns`, optional. | Write them as they are; `counter` equals the decoded `content.frame_counter` while it is below 2^32 (the code carries it mod 2^32). |
| `flip` extras | Also `x`, `y`, `t_evt_resolution`, `counter`, `superseded_by`, `null_reasons`. | `n`, `j`, `t_evt_dev_ns`, `t_handled_dev_ns`, `t_vsync_first_draw_dev_ns`, `t_drawn_dev_ns`. | Same names; extras are allowed. |
| `step` extras | Also `level`, `t_rx_dev_ns`, `t_accepted_dev_ns`, `t_drawn_dev_ns`, `counter`, `superseded_by`, `null_reasons`. | `k`, `t_vsync_dev_ns`. | Same names; extras are allowed. |
| Not drawn | A flip or step never shown has null `counter`, first-draw vsync and `t_drawn_dev_ns`, with the reason. | Only `t_drawn_dev_ns` may be null. | Pass them on; the analysis must treat a null first-draw vsync as "not drawn" (to reconcile with C4). |
| Uniqueness of n and k | Unique per HELLO `instance`; they start over when the app restarts. | n repeated is an input problem; k unique per run and instance. | Start a new `apk-<instance>.jsonl` (or mark the boundary) when `instance` changes. |
| Restarts of the clock | Device times restart with each boot of the instance; an app restart keeps the clock. | Calibration per window (`calib`). | Calibrate again after any restart of the instance. |

## 9. Limits

- Not run on a device in this slice: the first install and run are in the coordinator's window.
- `T_present` is not measured. `t_drawn_dev_ns` is HWUI's frame completion, not the moment pixels
  reach the screen or the encoder.
- The code is drawn only in the pattern, flip and step modes; the static and replay modes show
  the pushed images unchanged, as plan §5.4 and §5.8 need.
- The replay decoder may fall behind on a slow instance; every late or skipped image is reported,
  never hidden.
- A ring overflow is reported as `lost`, not prevented.
