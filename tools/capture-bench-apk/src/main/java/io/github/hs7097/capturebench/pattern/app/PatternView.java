package io.github.hs7097.capturebench.pattern.app;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.os.Build;
import android.view.Choreographer;
import android.view.Display;
import android.view.MotionEvent;
import android.view.View;

import io.github.hs7097.capturebench.pattern.core.Command;
import io.github.hs7097.capturebench.pattern.core.Json;
import io.github.hs7097.capturebench.pattern.core.Layout;
import io.github.hs7097.capturebench.pattern.core.ProtocolError;
import io.github.hs7097.capturebench.pattern.core.RecordBook;
import io.github.hs7097.capturebench.pattern.core.ReplayClock;
import io.github.hs7097.capturebench.pattern.core.Timecode;

import java.util.ArrayList;

/**
 * The test pattern (PROTOCOL.md, "Screen"). Every state change is latched in a Choreographer
 * frame callback and drawn in the same frame's traversal, so the frame that shows a change
 * carries that callback's {@code frameTimeNanos} ({@code T_vsync}) in its timecode. Runs on the
 * UI thread only; the time service reaches it through {@link Bench#onUi}.
 */
final class PatternView extends View implements Choreographer.FrameCallback {
    static final String PATTERN = "pattern";
    static final String FLIP = "flip";
    static final String STEP = "step";
    static final String STATIC = "static";
    static final String REPLAY = "replay";

    private static final int WHITE = 0xFFFFFFFF;
    private static final int BLACK = 0xFF000000;
    private static final int GREY = 0xFF808080;
    private static final int TRACK_GREY = 0xFF606060;
    private static final int GRID_LINE = 0xFF707070;
    private static final int ERROR_RED = 0xFF800000;
    private static final int DEFAULT_SPEED = 8;

    private static final class Tap {
        final long n;
        final int j;

        Tap(long n, int j) {
            this.n = n;
            this.j = j;
        }
    }

    private static final class PendingMode {
        final String mode;
        final long seq;
        final Integer level;
        final String slot;
        final Bitmap bitmap;
        final ReplayPlayer player;
        final String prefix;
        final int fps;
        final int loops;

        PendingMode(String mode, long seq, Integer level, String slot, Bitmap bitmap, ReplayPlayer player, String prefix,
                int fps, int loops) {
            this.mode = mode;
            this.seq = seq;
            this.level = level;
            this.slot = slot;
            this.bitmap = bitmap;
            this.player = player;
            this.prefix = prefix;
            this.fps = fps;
            this.loops = loops;
        }
    }

    private final RecordBook book;
    private final Choreographer choreographer;
    private final Paint fill = new Paint();
    private final Paint image = new Paint();

    private int cell = Layout.MIN_CELL;
    private int speed = DEFAULT_SPEED;
    private Layout layout;
    private String layoutError = "the view has no size yet";

    // Process-wide ordinals (UI thread only): a recreated activity continues them, so a record of
    // the old view can never be matched to a frame of the new one by a repeated number.
    private static long configSeq;
    private static long modeRequests;
    private static long tapCount;
    private static long stepCount;
    private static long frameSeq;

    // Committed state: changed only in doFrame, onSizeChanged and SET.
    private String mode = PATTERN;
    private long modeSeq;
    private long flipN;
    private int flipJ;
    private boolean flipLevel;
    private long stepK;
    private boolean stepLevel;
    private Bitmap staticBitmap;
    private String staticSlot;
    private ReplayPlayer replay;
    private ReplayClock replayClock;
    private String replayPrefix;
    private long replayShown = -1;
    private long replayLateReported = -1;
    private boolean replayDone;
    private Bitmap replayBitmap;

    // Requests that take effect at the next vsync.
    private PendingMode pendingMode;
    private final ArrayList<Tap> pendingTaps = new ArrayList<Tap>();
    private boolean pendingStep;
    private long pendingStepK;
    private boolean pendingStepLevel;
    private boolean pendingConfigDraw;

    private long tapsIgnored;
    private long redrawsWithoutLatch;

    // The latch: set in doFrame, consumed by the next onDraw.
    private boolean callbackPosted;
    private boolean latched;
    private boolean everLatched;
    private long latchedVsync;
    private long latchedCallback;
    private Timecode.Fields codeFields;
    private boolean[][] codeCells;
    private long lastDrawnVsync;

    PatternView(Context context, Bench bench) {
        super(context);
        this.book = bench.book;
        this.choreographer = Choreographer.getInstance();
        fill.setStyle(Paint.Style.FILL);
        fill.setAntiAlias(false);
        image.setAntiAlias(false);
        image.setFilterBitmap(false);
        image.setDither(false);
        setKeepScreenOn(true);
    }

    // ---- requests from the time service (UI thread) ----

    Json.Obj requestMode(Command c, long tRx, Bitmap bitmap, ReplayPlayer player) throws ProtocolError {
        long seq = ++modeRequests;
        if (pendingMode != null && pendingMode.player != null && pendingMode.player != player) {
            pendingMode.player.close();
        }
        pendingMode = new PendingMode(c.mode, seq, c.level, c.slot, bitmap, player, c.prefix, c.fps, c.loops);
        long now = System.nanoTime();
        boolean replayMode = REPLAY.equals(c.mode);
        Json.Obj fields = Json.obj()
                .put("mode_seq", seq)
                .put("mode", c.mode)
                .put("level", c.level)
                .put("slot", c.slot)
                .put("prefix", replayMode ? c.prefix : null)
                .put("fps", replayMode ? Integer.valueOf(c.fps) : null)
                .put("loops", replayMode ? Integer.valueOf(c.loops) : null)
                .put("images", player != null ? Integer.valueOf(player.count()) : null)
                .put("image_width", bitmap != null ? Integer.valueOf(bitmap.getWidth())
                        : player != null ? Integer.valueOf(player.firstWidth()) : null)
                .put("image_height", bitmap != null ? Integer.valueOf(bitmap.getHeight())
                        : player != null ? Integer.valueOf(player.firstHeight()) : null)
                .put("view_width", getWidth())
                .put("view_height", getHeight())
                .put("t_rx_dev_ns", tRx)
                .put("t_accepted_dev_ns", now);
        book.await(RecordBook.TYPE_MODE, seq, fields, now);
        scheduleFrame();
        return Bench.ok("MODE").put("mode_seq", seq).put("mode", c.mode);
    }

    Json.Obj requestStep(Command c, long tRx) throws ProtocolError {
        if (!STEP.equals(mode) || pendingMode != null) {
            throw new ProtocolError("wrong_mode", "STEP needs the committed mode step and no pending MODE; the mode is "
                    + mode + (pendingMode != null ? " with MODE " + pendingMode.mode + " pending" : ""));
        }
        if (pendingStep) {
            throw new ProtocolError("step_pending", "step " + pendingStepK + " is not drawn yet");
        }
        long k = ++stepCount;
        boolean level = c.level == null ? !stepLevel : c.level.intValue() == 1;
        pendingStep = true;
        pendingStepK = k;
        pendingStepLevel = level;
        long now = System.nanoTime();
        Json.Obj fields = Json.obj()
                .put("k", k)
                .put("level", level ? 1 : 0)
                .put("t_rx_dev_ns", tRx)
                .put("t_accepted_dev_ns", now);
        book.await(RecordBook.TYPE_STEP, k, fields, now);
        scheduleFrame();
        return Bench.ok("STEP").put("k", k).put("level", level ? 1 : 0);
    }

    Json.Obj set(Command c) throws ProtocolError {
        if ("cell".equals(c.key)) {
            if (getWidth() > 0 && getHeight() > 0) {
                try {
                    Layout.compute(getWidth(), getHeight(), c.value);
                } catch (IllegalArgumentException e) {
                    throw new ProtocolError("layout_does_not_fit", e.getMessage());
                }
            }
            cell = c.value;
            recomputeLayout("set_cell");
        } else {
            speed = c.value;
            book.event(Json.obj().put("type", "set").put("key", c.key).put("value", c.value).put("t_dev_ns", System.nanoTime()));
        }
        return Bench.ok("SET").put("key", c.key).put("value", c.value);
    }

    // ---- input ----

    @Override
    public boolean onTouchEvent(MotionEvent event) {
        if (event.getActionMasked() != MotionEvent.ACTION_DOWN) {
            return true;
        }
        long tEvt;
        String unit;
        if (Build.VERSION.SDK_INT >= 34) {
            tEvt = event.getEventTimeNanos();
            unit = "ns";
        } else {
            tEvt = event.getEventTime() * 1000000L;
            unit = "ms";
        }
        if (!FLIP.equals(mode) || layout == null) {
            tapsIgnored++;
            return true;
        }
        float x = event.getX();
        float y = event.getY();
        int j = layout.tapCell(x, y);
        long n = ++tapCount;
        long tHandled = System.nanoTime();
        pendingTaps.add(new Tap(n, j < 0 ? 0 : j));
        Json.Obj fields = Json.obj()
                .put("n", n)
                .put("j", j)
                .put("x", Double.valueOf(x))
                .put("y", Double.valueOf(y))
                .put("t_evt_dev_ns", tEvt)
                .put("t_evt_resolution", unit)
                .put("t_handled_dev_ns", tHandled);
        book.await(RecordBook.TYPE_FLIP, n, fields, tHandled);
        scheduleFrame();
        return true;
    }

    // ---- frames ----

    @Override
    protected void onAttachedToWindow() {
        super.onAttachedToWindow();
        scheduleFrame();
    }

    @Override
    protected void onDetachedFromWindow() {
        choreographer.removeFrameCallback(this);
        callbackPosted = false;
        super.onDetachedFromWindow();
    }

    @Override
    protected void onSizeChanged(int w, int h, int oldw, int oldh) {
        super.onSizeChanged(w, h, oldw, oldh);
        recomputeLayout("size");
    }

    private void recomputeLayout(String cause) {
        long seq = ++configSeq;
        try {
            layout = Layout.compute(getWidth(), getHeight(), cell);
            layoutError = null;
        } catch (IllegalArgumentException e) {
            layout = null;
            layoutError = e.getMessage();
            book.event(Json.obj().put("type", "error").put("code", "layout_does_not_fit").put("detail", layoutError)
                    .put("t_dev_ns", System.nanoTime()));
        }
        int[] location = new int[2];
        getLocationOnScreen(location);
        Display display = getDisplay();
        long now = System.nanoTime();
        Json.Obj fields = Json.obj()
                .put("config_seq", seq)
                .put("cause", cause)
                .put("width", getWidth())
                .put("height", getHeight())
                .put("view_x", location[0])
                .put("view_y", location[1])
                .put("rotation", display != null ? Integer.valueOf(display.getRotation()) : null)
                .put("cell", cell)
                .put("layout", layout != null ? layout.toJson(speed) : null)
                .put("layout_error", layoutError)
                .put("t_cfg_dev_ns", now);
        book.await(RecordBook.TYPE_CONFIG, seq, fields, now);
        // The next vsync latches and draws even in the still modes, so the config record gets its frame.
        pendingConfigDraw = true;
        scheduleFrame();
    }

    private void scheduleFrame() {
        if (!callbackPosted) {
            callbackPosted = true;
            choreographer.postFrameCallback(this);
        }
    }

    private boolean animated() {
        return PATTERN.equals(mode) || FLIP.equals(mode) || STEP.equals(mode);
    }

    @Override
    public void doFrame(long frameTimeNanos) {
        callbackPosted = false;
        long now = System.nanoTime();
        boolean draw = pendingConfigDraw;
        pendingConfigDraw = false;
        if (!pendingTaps.isEmpty()) {
            for (Tap t : pendingTaps) {
                flipN = t.n;
                flipJ = t.j;
                flipLevel = !flipLevel;
            }
            pendingTaps.clear();
            draw = true;
        }
        if (pendingStep) {
            pendingStep = false;
            stepK = pendingStepK;
            stepLevel = pendingStepLevel;
            draw = true;
        }
        if (pendingMode != null) {
            applyMode(pendingMode, frameTimeNanos);
            pendingMode = null;
            draw = true;
        } else if (REPLAY.equals(mode) && replay != null && !replayDone) {
            draw |= advanceReplay(frameTimeNanos);
        }
        if (animated()) {
            draw = true;
        }
        if (draw) {
            frameSeq++;
            latched = true;
            everLatched = true;
            latchedVsync = frameTimeNanos;
            latchedCallback = now;
            codeFields = codeFieldsNow();
            codeCells = codeFields != null ? Timecode.cells(codeFields) : null;
            invalidate();
        }
        if (animated() || (REPLAY.equals(mode) && !replayDone)) {
            scheduleFrame();
        }
    }

    private void applyMode(PendingMode pm, long frameTimeNanos) {
        if (replay != null && replay != pm.player) {
            replay.close();
        }
        replay = null;
        replayClock = null;
        replayPrefix = null;
        replayBitmap = null;
        replayShown = -1;
        replayLateReported = -1;
        replayDone = false;
        staticBitmap = null;
        staticSlot = null;
        mode = pm.mode;
        modeSeq = pm.seq;
        if (STEP.equals(mode)) {
            stepLevel = pm.level != null && pm.level.intValue() == 1;
        } else if (STATIC.equals(mode)) {
            staticBitmap = pm.bitmap;
            staticSlot = pm.slot;
        } else if (REPLAY.equals(mode)) {
            replay = pm.player;
            replayPrefix = pm.prefix;
            replayClock = new ReplayClock(frameTimeNanos, pm.fps, replay.count(), pm.loops);
            replayShown = 0;
            replayBitmap = replay.get(0);
            book.event(Json.obj().put("type", "replay").put("event", "start").put("mode_seq", modeSeq)
                    .put("prefix", replayPrefix).put("images", replay.count()).put("fps", pm.fps).put("loops", pm.loops)
                    .put("t_vsync_dev_ns", frameTimeNanos));
            replay.prefetch(1, replayClock.lastIndex());
        }
    }

    /** Moves the replay to the image due at this vsync; true when the shown image changed. */
    private boolean advanceReplay(long frameTimeNanos) {
        long due = replayClock.dueIndex(frameTimeNanos);
        if (replayClock.finished(due)) {
            replayDone = true;
            book.event(Json.obj().put("type", "replay").put("event", "done").put("mode_seq", modeSeq)
                    .put("last_index", replayShown).put("t_vsync_dev_ns", frameTimeNanos));
            return false;
        }
        if (due <= replayShown) {
            return false;
        }
        Bitmap next = replay.get(due);
        if (next == null) {
            String failure = replay.failure(due);
            if (replayLateReported != due) {
                replayLateReported = due;
                book.event(Json.obj().put("type", "replay").put("event", failure != null ? "decode_failed" : "late")
                        .put("mode_seq", modeSeq).put("index", due).put("slot", replay.name(due))
                        .put("detail", failure).put("shown_index", replayShown).put("t_vsync_dev_ns", frameTimeNanos));
            }
            replay.prefetch(due, replayClock.lastIndex());
            return false;
        }
        if (due > replayShown + 1) {
            book.event(Json.obj().put("type", "replay").put("event", "skip").put("mode_seq", modeSeq)
                    .put("from_index", replayShown + 1).put("to_index", due - 1).put("t_vsync_dev_ns", frameTimeNanos));
        }
        replayShown = due;
        replayBitmap = next;
        replay.release(due);
        replay.prefetch(due + 1, replayClock.lastIndex());
        return true;
    }

    private Timecode.Fields codeFieldsNow() {
        long seq = frameSeq & 0xFFFFFFFFL;
        if (PATTERN.equals(mode)) {
            return new Timecode.Fields(Timecode.MODE_PATTERN, seq, latchedVsync, 0, 0);
        }
        if (FLIP.equals(mode)) {
            return new Timecode.Fields(Timecode.MODE_FLIP, seq, latchedVsync, (int) (flipN & 0xFFFFFF),
                    (flipJ << 4) | (flipLevel ? Timecode.AUX_LEVEL : 0));
        }
        if (STEP.equals(mode)) {
            return new Timecode.Fields(Timecode.MODE_STEP, seq, latchedVsync, (int) (stepK & 0xFFFFFF),
                    stepLevel ? Timecode.AUX_LEVEL : 0);
        }
        return null;
    }

    @Override
    protected void onDraw(Canvas canvas) {
        long tDraw = System.nanoTime();
        drawContent(canvas);
        if (!latched) {
            redrawsWithoutLatch++;
            return;
        }
        latched = false;
        lastDrawnVsync = latchedVsync;
        book.frameDrawn(frameSeq, latchedVsync, latchedCallback, tDraw,
                new RecordBook.Shown(mode, flipN, stepK, modeSeq, configSeq), contentRecord());
    }

    private Json.Obj contentRecord() {
        if (STATIC.equals(mode)) {
            return Json.obj().put("code", false).put("slot", staticSlot);
        }
        if (REPLAY.equals(mode)) {
            return Json.obj().put("code", false).put("slot", replay != null ? replay.name(replayShown) : null)
                    .put("replay_index", replayShown)
                    .put("replay_image", replayClock != null ? Integer.valueOf(replayClock.imageOf(replayShown)) : null)
                    .put("replay_loop", replayClock != null ? Long.valueOf(replayClock.loopOf(replayShown)) : null);
        }
        if (layout == null || codeFields == null) {
            return Json.obj().put("code", false).put("layout_error", layoutError);
        }
        return Json.obj()
                .put("code", true)
                .put("code_mode", codeFields.mode)
                .put("code_counter", codeFields.seq)
                .put("event", codeFields.event)
                .put("aux", codeFields.aux);
    }

    private void drawContent(Canvas canvas) {
        if (STATIC.equals(mode)) {
            canvas.drawColor(BLACK);
            if (staticBitmap != null) {
                canvas.drawBitmap(staticBitmap, 0f, 0f, image);
            } else {
                drawStillCard(canvas);
            }
            return;
        }
        if (REPLAY.equals(mode)) {
            canvas.drawColor(BLACK);
            if (replayBitmap != null) {
                canvas.drawBitmap(replayBitmap, 0f, 0f, image);
            }
            return;
        }
        if (layout == null) {
            canvas.drawColor(ERROR_RED);
            return;
        }
        if (STEP.equals(mode)) {
            canvas.drawColor(stepLevel ? WHITE : BLACK);
        } else {
            canvas.drawColor(GREY);
            if (FLIP.equals(mode)) {
                drawGrid(canvas);
                fillRect(canvas, layout.flipBlock, flipLevel ? WHITE : BLACK);
            }
            fillRect(canvas, layout.track, TRACK_GREY);
            drawMoving(canvas);
        }
        if (everLatched && codeCells != null) {
            for (int r = 0; r < Timecode.ROWS; r++) {
                for (int c = 0; c < Timecode.COLUMNS; c++) {
                    fillRect(canvas, layout.codeCell(r, c), codeCells[r][c] ? WHITE : BLACK);
                }
            }
        }
    }

    private void drawGrid(Canvas canvas) {
        fill.setColor(GRID_LINE);
        for (int k = 1; k < Layout.GRID; k++) {
            int x = layout.gridCell(k).left;
            int y = layout.gridCell(k * Layout.GRID).top;
            canvas.drawRect(x, 0, x + 1, layout.height, fill);
            canvas.drawRect(0, y, layout.width, y + 1, fill);
        }
    }

    private void drawMoving(Canvas canvas) {
        int size = layout.movingSize();
        int half = size / 2;
        int left = layout.movingLeft(frameSeq, speed);
        int top = layout.track.top;
        fill.setColor(WHITE);
        canvas.drawRect(left, top, left + half, top + half, fill);
        canvas.drawRect(left + half, top + half, left + size, top + size, fill);
        fill.setColor(BLACK);
        canvas.drawRect(left + half, top, left + size, top + half, fill);
        canvas.drawRect(left, top + half, left + half, top + size, fill);
    }

    /** The built-in still card of MODE static without a slot: eight fixed grey bars, drawn once. */
    private void drawStillCard(Canvas canvas) {
        int w = getWidth();
        int h = getHeight();
        for (int i = 0; i < 8; i++) {
            int level = i * 255 / 7;
            fill.setColor(0xFF000000 | (level << 16) | (level << 8) | level);
            canvas.drawRect(i * w / 8, 0, (i + 1) * w / 8, h, fill);
        }
    }

    private void fillRect(Canvas canvas, Layout.Rect r, int color) {
        fill.setColor(color);
        canvas.drawRect(r.left, r.top, r.right, r.bottom, fill);
    }

    // ---- INFO ----

    Json.Obj info() {
        int[] location = new int[2];
        getLocationOnScreen(location);
        Json.Obj replayInfo = null;
        if (replay != null && replayClock != null) {
            replayInfo = Json.obj()
                    .put("prefix", replayPrefix)
                    .put("images", replay.count())
                    .put("fps", replayClock.fps)
                    .put("loops", replayClock.loops)
                    .put("start_vsync_dev_ns", replayClock.startNs)
                    .put("shown_index", replayShown)
                    .put("done", replayDone);
        }
        return Json.obj()
                .put("mode", mode)
                .put("mode_seq", modeSeq)
                .put("mode_pending", pendingMode != null ? pendingMode.mode : null)
                .put("view", Json.obj().put("width", getWidth()).put("height", getHeight())
                        .put("x", location[0]).put("y", location[1]))
                .put("cell", cell)
                .put("speed", speed)
                .put("layout", layout != null ? layout.toJson(speed) : null)
                .put("layout_error", layoutError)
                .put("config_seq", configSeq)
                .put("counter", frameSeq)
                .put("last_drawn_vsync_dev_ns", lastDrawnVsync)
                .put("flip_n", flipN)
                .put("flip_j", flipJ)
                .put("flip_level", flipLevel ? 1 : 0)
                .put("taps", tapCount)
                .put("taps_ignored", tapsIgnored)
                .put("step_k", stepK)
                .put("step_level", stepLevel ? 1 : 0)
                .put("step_pending", pendingStep)
                .put("static_slot", staticSlot)
                .put("replay", replayInfo)
                .put("redraws_without_latch", redrawsWithoutLatch);
    }
}
