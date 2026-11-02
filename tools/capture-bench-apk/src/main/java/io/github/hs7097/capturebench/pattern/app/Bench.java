package io.github.hs7097.capturebench.pattern.app;

import android.content.Context;
import android.graphics.Bitmap;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.os.Process;
import android.util.Log;

import io.github.hs7097.capturebench.pattern.core.Command;
import io.github.hs7097.capturebench.pattern.core.Json;
import io.github.hs7097.capturebench.pattern.core.LineReader;
import io.github.hs7097.capturebench.pattern.core.ProtocolError;
import io.github.hs7097.capturebench.pattern.core.ProtocolInfo;
import io.github.hs7097.capturebench.pattern.core.RecordBook;
import io.github.hs7097.capturebench.pattern.core.Timecode;

import java.io.File;
import java.io.IOException;
import java.security.SecureRandom;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * The process-wide part of the app: the record book, the slot store, the time service and the
 * dispatch of every request. It outlives the activity, so a recreated activity keeps the records.
 */
final class Bench {
    static final String TAG = "CaptureBenchPattern";
    static final long UI_TIMEOUT_MS = 2000;

    private static Bench instance;

    final RecordBook book;
    final SlotStore slots;
    final String instanceId;
    final long processStartNs;
    private final Handler main;
    private final TimeService service;
    private volatile PatternActivity activity;

    static synchronized Bench get(Context context) {
        if (instance == null) {
            instance = new Bench(context.getApplicationContext());
        }
        return instance;
    }

    private Bench(Context app) {
        processStartNs = System.nanoTime();
        main = new Handler(Looper.getMainLooper());
        book = new RecordBook(RecordBook.VSYNC_CAPACITY, RecordBook.EVENT_CAPACITY, RecordBook.METRICS_TIMEOUT_NS,
                RecordBook.DRAW_TIMEOUT_NS, true);
        slots = new SlotStore(new File(app.getFilesDir(), "slots"));
        byte[] id = new byte[8];
        new SecureRandom().nextBytes(id);
        instanceId = Timecode.hex(id);
        book.event(Json.obj().put("type", "server").put("event", "process_start").put("instance", instanceId)
                .put("pid", Process.myPid()).put("t_dev_ns", processStartNs));
        service = new TimeService(this);
        service.start();
    }

    void attach(PatternActivity a) {
        activity = a;
        book.event(Json.obj().put("type", "server").put("event", "activity_attached").put("t_dev_ns", System.nanoTime()));
    }

    void detach(PatternActivity a) {
        if (activity == a) {
            activity = null;
            book.event(Json.obj().put("type", "server").put("event", "activity_detached").put("t_dev_ns", System.nanoTime()));
        }
    }

    /** Records a failure in the event stream and in logcat. */
    void error(String code, String detail) {
        Log.e(TAG, code + ": " + detail);
        book.event(Json.obj().put("type", "error").put("code", code).put("detail", detail).put("t_dev_ns", System.nanoTime()));
    }

    static Json.Obj ok(String verb) {
        return Json.obj().put("ok", Boolean.TRUE).put("cmd", verb);
    }

    /** Handles one request on a connection thread; PING and BYE are answered by the connection itself. */
    Json.Obj handle(final Command c, final long tRx, LineReader in) throws ProtocolError, IOException {
        if ("HELLO".equals(c.verb)) {
            return hello();
        }
        if ("INFO".equals(c.verb)) {
            return onUi(c.verb, new UiCall() {
                @Override
                public Json.Obj run(PatternActivity a) {
                    return a.info();
                }
            });
        }
        if ("MODE".equals(c.verb)) {
            return mode(c, tRx);
        }
        if ("STEP".equals(c.verb)) {
            return onUi(c.verb, new UiCall() {
                @Override
                public Json.Obj run(PatternActivity a) throws ProtocolError {
                    return a.view().requestStep(c, tRx);
                }
            });
        }
        if ("ORIENT".equals(c.verb)) {
            return onUi(c.verb, new UiCall() {
                @Override
                public Json.Obj run(PatternActivity a) throws ProtocolError {
                    return a.requestOrientation(c.orientation, tRx);
                }
            });
        }
        if ("SET".equals(c.verb)) {
            return onUi(c.verb, new UiCall() {
                @Override
                public Json.Obj run(PatternActivity a) throws ProtocolError {
                    return a.view().set(c);
                }
            });
        }
        if ("PUT".equals(c.verb)) {
            return slots.put(c.slot, c.length, in, book);
        }
        if ("SLOTS".equals(c.verb)) {
            return slots.list();
        }
        if ("RECORDS".equals(c.verb)) {
            return book.read(c.stream, c.after, c.max).toJson();
        }
        throw new ProtocolError("internal_error", "no handler for " + c.verb);
    }

    private Json.Obj hello() {
        return ok("HELLO")
                .put("protocol", ProtocolInfo.PROTOCOL_VERSION)
                .put("record_format", ProtocolInfo.RECORD_FORMAT)
                .put("timecode_version", Timecode.VERSION)
                .put("package", ProtocolInfo.PACKAGE)
                .put("version_name", BuildInfo.VERSION_NAME)
                .put("source_sha", BuildInfo.SOURCE_SHA)
                .put("sdk_int", Build.VERSION.SDK_INT)
                .put("release", Build.VERSION.RELEASE)
                .put("instance", instanceId)
                .put("pid", Process.myPid())
                .put("uid", Process.myUid())
                .put("process_start_dev_ns", processStartNs)
                .put("clock", "CLOCK_MONOTONIC")
                .put("t_evt_resolution", Build.VERSION.SDK_INT >= 34 ? "ns" : "ms")
                .put("frame_metrics", book.metricsAvailable())
                .put("server_error", service.error())
                .put("t_now_dev_ns", System.nanoTime());
    }

    private Json.Obj mode(final Command c, final long tRx) throws ProtocolError {
        Bitmap bitmap = null;
        ReplayPlayer player = null;
        if ("static".equals(c.mode) && c.slot != null) {
            bitmap = slots.decode(c.slot);
        } else if ("replay".equals(c.mode)) {
            List<String> names = slots.namesWithPrefix(c.prefix);
            if (names.isEmpty()) {
                throw new ProtocolError("no_such_slots", "no slot name starts with " + c.prefix);
            }
            player = new ReplayPlayer(names, slots.files(names));
            try {
                player.decodeFirst();
            } catch (ProtocolError e) {
                player.close();
                throw e;
            }
        }
        final Bitmap fb = bitmap;
        final ReplayPlayer fp = player;
        try {
            return onUi(c.verb, new UiCall() {
                @Override
                public Json.Obj run(PatternActivity a) throws ProtocolError {
                    return a.view().requestMode(c, tRx, fb, fp);
                }
            });
        } catch (ProtocolError e) {
            if (fp != null) {
                fp.close();
            }
            throw e;
        }
    }

    interface UiCall {
        Json.Obj run(PatternActivity activity) throws ProtocolError;
    }

    /**
     * Runs {@code call} on the UI thread and waits for it. If the UI thread does not start it within
     * UI_TIMEOUT_MS the request fails with ui_timeout and the call is abandoned: it never runs later.
     */
    Json.Obj onUi(final String verb, final UiCall call) throws ProtocolError {
        final AtomicInteger state = new AtomicInteger(0); // 0 waiting, 1 started, 2 abandoned
        final Json.Obj[] result = new Json.Obj[1];
        final ProtocolError[] failure = new ProtocolError[1];
        final CountDownLatch done = new CountDownLatch(1);
        main.post(new Runnable() {
            @Override
            public void run() {
                if (!state.compareAndSet(0, 1)) {
                    error("ui_call_abandoned", verb + " reached the UI thread after its ui_timeout and was not run");
                    return;
                }
                try {
                    PatternActivity a = activity;
                    if (a == null) {
                        throw new ProtocolError("no_view", "the pattern activity is not running");
                    }
                    result[0] = call.run(a);
                } catch (ProtocolError e) {
                    failure[0] = e;
                } catch (RuntimeException e) {
                    Log.e(TAG, verb + " failed on the UI thread", e);
                    failure[0] = new ProtocolError("internal_error", verb + ": " + e);
                } finally {
                    done.countDown();
                }
            }
        });
        boolean finished = await(done, UI_TIMEOUT_MS);
        if (!finished) {
            if (state.compareAndSet(0, 2)) {
                throw new ProtocolError("ui_timeout", verb + ": the UI thread did not take the request within "
                        + UI_TIMEOUT_MS + " ms; it was not applied");
            }
            // The call started just in time: its outcome is the answer.
            await(done, Long.MAX_VALUE);
        }
        if (failure[0] != null) {
            throw failure[0];
        }
        return result[0];
    }

    private static boolean await(CountDownLatch latch, long ms) throws ProtocolError {
        try {
            return latch.await(ms, TimeUnit.MILLISECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ProtocolError("internal_error", "interrupted while waiting for the UI thread", true);
        }
    }

    /** True for the peers allowed to connect: root and shell (adbd runs as one of them) and this app. */
    static boolean peerAllowed(int uid) {
        return uid == 0 || uid == 2000 || uid == Process.myUid();
    }
}
