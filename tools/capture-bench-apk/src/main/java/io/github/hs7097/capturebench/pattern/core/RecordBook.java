package io.github.hs7097.capturebench.pattern.core;

import java.util.ArrayList;
import java.util.Iterator;
import java.util.LinkedHashMap;
import java.util.List;

/**
 * The record book behind RECORDS (PROTOCOL.md, "Records"). It owns the two streams and decides
 * when a record is final:
 * <ul>
 *   <li>a {@code vsync} record (one drawn frame) is appended when FrameMetrics reports the frame
 *       that has its {@code T_vsync} (then {@code t_drawn_dev_ns} is set), or when no report came
 *       within the metrics timeout, or at once when FrameMetrics is unavailable (then
 *       {@code t_drawn_dev_ns} is null and {@code null_reasons} says why);</li>
 *   <li>a {@code flip}, {@code step}, {@code mode} or {@code config} record waits for the first
 *       drawn frame that shows it and is appended right after that frame's record, with the
 *       frame's counter, {@code T_vsync} and {@code T_drawn}; one that is never shown is appended
 *       with those three null and the reason in {@code null_reasons};</li>
 *   <li>every other event is appended at once.</li>
 * </ul>
 * Field names follow the C4 schema: the discriminator is {@code type}, device times end in
 * {@code _dev_ns}, a null measurement has its reason in {@code null_reasons} under its key.
 * Times are whatever the caller passes; the app passes System.nanoTime (CLOCK_MONOTONIC, ns).
 */
public final class RecordBook {
    /** Stream names of RECORDS. */
    public static final String VSYNC = "vsync";
    public static final String EVENT = "event";

    public static final int VSYNC_CAPACITY = 32768;
    public static final int EVENT_CAPACITY = 16384;
    public static final long METRICS_TIMEOUT_NS = 1000000000L;
    public static final long DRAW_TIMEOUT_NS = 5000000000L;

    public static final String TYPE_FLIP = "flip";
    public static final String TYPE_STEP = "step";
    public static final String TYPE_MODE = "mode";
    public static final String TYPE_CONFIG = "config";

    /** What a drawn frame shows: its mode and the ordinals of the state it draws. */
    public static final class Shown {
        public final String mode;
        public final long flipN;
        public final long stepK;
        public final long modeSeq;
        public final long configSeq;

        public Shown(String mode, long flipN, long stepK, long modeSeq, long configSeq) {
            this.mode = mode;
            this.flipN = flipN;
            this.stepK = stepK;
            this.modeSeq = modeSeq;
            this.configSeq = configSeq;
        }
    }

    private static final class PendingFrame {
        final long counter;
        final long tVsync;
        final long tCallback;
        final long tDraw;
        final String mode;
        final Json.Obj content;
        final List<Awaiting> linked = new ArrayList<Awaiting>();

        PendingFrame(long counter, long tVsync, long tCallback, long tDraw, String mode, Json.Obj content) {
            this.counter = counter;
            this.tVsync = tVsync;
            this.tCallback = tCallback;
            this.tDraw = tDraw;
            this.mode = mode;
            this.content = content;
        }
    }

    private static final class Awaiting {
        final String type;
        final long ordinal;
        final Json.Obj fields;
        final long tCreated;

        Awaiting(String type, long ordinal, Json.Obj fields, long tCreated) {
            this.type = type;
            this.ordinal = ordinal;
            this.fields = fields;
            this.tCreated = tCreated;
        }
    }

    private final RecordStream vsyncs;
    private final RecordStream events;
    private final long metricsTimeoutNs;
    private final long drawTimeoutNs;
    private boolean metricsAvailable;
    private final LinkedHashMap<Long, PendingFrame> pending = new LinkedHashMap<Long, PendingFrame>();
    private final List<Awaiting> awaiting = new ArrayList<Awaiting>();
    private long metricsMatched;
    private long metricsUnmatched;
    private long metricsDroppedReported;
    private long framesWithoutMetrics;
    private long recordsNotDrawn;

    public RecordBook(int vsyncCapacity, int eventCapacity, long metricsTimeoutNs, long drawTimeoutNs,
            boolean metricsAvailable) {
        if (metricsTimeoutNs <= 0 || drawTimeoutNs <= 0) {
            throw new IllegalArgumentException("timeouts must be positive");
        }
        this.vsyncs = new RecordStream(VSYNC, vsyncCapacity);
        this.events = new RecordStream(EVENT, eventCapacity);
        this.metricsTimeoutNs = metricsTimeoutNs;
        this.drawTimeoutNs = drawTimeoutNs;
        this.metricsAvailable = metricsAvailable;
    }

    /** The name of the first-draw vsync field: the C4 schema calls it differently for flips. */
    public static String firstDrawVsyncField(String type) {
        return TYPE_FLIP.equals(type) ? "t_vsync_first_draw_dev_ns" : "t_vsync_dev_ns";
    }

    public synchronized boolean metricsAvailable() {
        return metricsAvailable;
    }

    /** Switching FrameMetrics off finalizes every waiting frame with frame_metrics_unavailable. */
    public synchronized void setMetricsAvailable(boolean available) {
        metricsAvailable = available;
        if (!available) {
            List<PendingFrame> waiting = new ArrayList<PendingFrame>(pending.values());
            pending.clear();
            for (PendingFrame pf : waiting) {
                finalizeFrame(pf, null, null, "frame_metrics_unavailable");
            }
        }
    }

    /** Appends an event that is final at once; the body starts with "type". */
    public synchronized long event(Json.Obj body) {
        if (!body.has("type")) {
            throw new IllegalArgumentException("an event needs a type");
        }
        return events.append(body);
    }

    /** Registers a record that waits for the first drawn frame showing {@code ordinal} of {@code type}. */
    public synchronized void await(String type, long ordinal, Json.Obj fields, long tCreatedNs) {
        if (!TYPE_FLIP.equals(type) && !TYPE_STEP.equals(type) && !TYPE_MODE.equals(type) && !TYPE_CONFIG.equals(type)) {
            throw new IllegalArgumentException("no record waits for a draw of type " + type);
        }
        if (fields.has("type")) {
            throw new IllegalArgumentException("the type is given separately");
        }
        awaiting.add(new Awaiting(type, ordinal, fields, tCreatedNs));
    }

    /** A frame was drawn. {@code content} describes what it shows (PROTOCOL.md, vsync records). */
    public synchronized void frameDrawn(long counter, long tVsync, long tCallback, long tDraw, Shown shown,
            Json.Obj content) {
        PendingFrame pf = new PendingFrame(counter, tVsync, tCallback, tDraw, shown.mode, content);
        Iterator<Awaiting> it = awaiting.iterator();
        List<Awaiting> notShown = new ArrayList<Awaiting>();
        List<Long> supersededBy = new ArrayList<Long>();
        List<String> reasons = new ArrayList<String>();
        while (it.hasNext()) {
            Awaiting a = it.next();
            long current;
            String needs;
            if (TYPE_FLIP.equals(a.type)) {
                current = shown.flipN;
                needs = "flip";
            } else if (TYPE_STEP.equals(a.type)) {
                current = shown.stepK;
                needs = "step";
            } else if (TYPE_MODE.equals(a.type)) {
                current = shown.modeSeq;
                needs = null;
            } else {
                current = shown.configSeq;
                needs = null;
            }
            if (a.ordinal > current) {
                continue;
            }
            it.remove();
            if (needs != null && !needs.equals(shown.mode)) {
                notShown.add(a);
                supersededBy.add(null);
                reasons.add("mode_changed");
            } else if (a.ordinal < current) {
                notShown.add(a);
                supersededBy.add(Long.valueOf(current));
                reasons.add("superseded");
            } else {
                pf.linked.add(a);
            }
        }
        for (int i = 0; i < notShown.size(); i++) {
            appendNotDrawn(notShown.get(i), reasons.get(i), supersededBy.get(i));
        }
        if (!metricsAvailable) {
            finalizeFrame(pf, null, null, "frame_metrics_unavailable");
            return;
        }
        PendingFrame previous = pending.remove(Long.valueOf(tVsync));
        if (previous != null) {
            finalizeFrame(previous, null, null, "vsync_reused");
        }
        pending.put(Long.valueOf(tVsync), pf);
    }

    /**
     * One FrameMetrics report: {@code vsync} is VSYNC_TIMESTAMP; {@code t_drawn_dev_ns} becomes
     * INTENDED_VSYNC_TIMESTAMP + TOTAL_DURATION (HWUI's FRAME_COMPLETED). {@code gpuDuration} < 0
     * means not reported (API < 31).
     */
    public synchronized void metrics(long vsync, long intendedVsync, long totalDuration, long gpuDuration,
            boolean firstDrawFrame, int droppedSinceLast) {
        if (droppedSinceLast > 0) {
            metricsDroppedReported += droppedSinceLast;
        }
        PendingFrame pf = pending.remove(Long.valueOf(vsync));
        if (pf == null) {
            metricsUnmatched++;
            return;
        }
        metricsMatched++;
        Json.Obj m = Json.obj()
                .put("intended_vsync_dev_ns", intendedVsync)
                .put("total_duration_ns", totalDuration)
                .put("gpu_duration_ns", gpuDuration >= 0 ? Long.valueOf(gpuDuration) : null)
                .put("first_draw_frame", firstDrawFrame);
        finalizeFrame(pf, Long.valueOf(intendedVsync + totalDuration), m, null);
    }

    /** Finalizes frames without a report after the metrics timeout and records never drawn after the draw timeout. */
    public synchronized void sweep(long nowNs) {
        List<PendingFrame> expired = new ArrayList<PendingFrame>();
        Iterator<PendingFrame> frameIt = pending.values().iterator();
        while (frameIt.hasNext()) {
            PendingFrame pf = frameIt.next();
            if (nowNs - pf.tDraw >= metricsTimeoutNs) {
                frameIt.remove();
                expired.add(pf);
            }
        }
        for (PendingFrame pf : expired) {
            framesWithoutMetrics++;
            finalizeFrame(pf, null, null, "metrics_not_reported");
        }
        List<Awaiting> late = new ArrayList<Awaiting>();
        Iterator<Awaiting> awaitIt = awaiting.iterator();
        while (awaitIt.hasNext()) {
            Awaiting a = awaitIt.next();
            if (nowNs - a.tCreated >= drawTimeoutNs) {
                awaitIt.remove();
                late.add(a);
            }
        }
        for (Awaiting a : late) {
            appendNotDrawn(a, "not_drawn_within_timeout", null);
        }
    }

    public RecordStream.Page read(String stream, long after, int max) throws ProtocolError {
        if (VSYNC.equals(stream)) {
            return vsyncs.read(after, max);
        }
        if (EVENT.equals(stream)) {
            return events.read(after, max);
        }
        throw new ProtocolError("bad_arguments", "no stream " + stream);
    }

    public synchronized Json.Obj stats() {
        return Json.obj()
                .put("vsync_newest", vsyncs.newest())
                .put("vsync_oldest", vsyncs.oldest())
                .put("vsync_capacity", vsyncs.capacity())
                .put("event_newest", events.newest())
                .put("event_oldest", events.oldest())
                .put("event_capacity", events.capacity())
                .put("frames_pending", pending.size())
                .put("records_awaiting_draw", awaiting.size())
                .put("frame_metrics", metricsAvailable)
                .put("metrics_matched", metricsMatched)
                .put("metrics_unmatched", metricsUnmatched)
                .put("metrics_dropped_reported", metricsDroppedReported)
                .put("frames_without_metrics", framesWithoutMetrics)
                .put("records_not_drawn", recordsNotDrawn);
    }

    private void finalizeFrame(PendingFrame pf, Long tDrawn, Json.Obj metrics, String nullReason) {
        Json.Obj reasons = Json.obj();
        if (tDrawn == null) {
            reasons.put("t_drawn_dev_ns", nullReason).put("metrics", nullReason);
        } else if (metrics != null && metrics.get("gpu_duration_ns") == null) {
            reasons.put("metrics.gpu_duration_ns", "not_reported_below_api_31");
        }
        Json.Obj record = Json.obj()
                .put("type", VSYNC)
                .put("counter", pf.counter)
                .put("mode", pf.mode)
                .put("t_vsync_dev_ns", pf.tVsync)
                .put("t_callback_dev_ns", pf.tCallback)
                .put("t_draw_dev_ns", pf.tDraw)
                .put("t_drawn_dev_ns", tDrawn)
                .put("metrics", metrics)
                .put("content", pf.content)
                .put("null_reasons", reasons);
        vsyncs.append(record);
        for (Awaiting a : pf.linked) {
            Json.Obj linkedReasons = Json.obj();
            if (tDrawn == null) {
                linkedReasons.put("t_drawn_dev_ns", nullReason);
            }
            Json.Obj body = start(a)
                    .put("counter", pf.counter)
                    .put(firstDrawVsyncField(a.type), pf.tVsync)
                    .put("t_drawn_dev_ns", tDrawn)
                    .put("superseded_by", null)
                    .put("null_reasons", linkedReasons);
            events.append(body);
        }
    }

    private void appendNotDrawn(Awaiting a, String reason, Long supersededBy) {
        recordsNotDrawn++;
        String vsyncField = firstDrawVsyncField(a.type);
        Json.Obj body = start(a)
                .put("counter", null)
                .put(vsyncField, null)
                .put("t_drawn_dev_ns", null)
                .put("superseded_by", supersededBy)
                .put("null_reasons", Json.obj().put("counter", reason).put(vsyncField, reason).put("t_drawn_dev_ns", reason));
        events.append(body);
    }

    private static Json.Obj start(Awaiting a) {
        Json.Obj body = Json.obj().put("type", a.type);
        for (String key : a.fields.keys()) {
            body.put(key, a.fields.get(key));
        }
        return body;
    }
}
