package io.github.hs7097.capturebench.pattern.core;

import java.util.List;

/**
 * The record format and when records become final (PROTOCOL.md, "Records"), on a fake clock:
 * times are plain numbers, metrics timeout 1000, draw timeout 5000.
 */
final class RecordBookTest {
    private static RecordBook book(boolean metrics) {
        return new RecordBook(16, 16, 1000L, 5000L, metrics);
    }

    private static List<String> records(RecordBook b, String stream) throws ProtocolError {
        return b.read(stream, 0, 100).records;
    }

    private static Json.Obj code(int mode, long counter, int event, int aux) {
        return Json.obj().put("code", true).put("code_mode", mode).put("code_counter", counter).put("event", event)
                .put("aux", aux);
    }

    public static void testVsyncRecordTakesTDrawnFromFrameMetrics() throws Exception {
        RecordBook b = book(true);
        b.frameDrawn(7, 1000, 1002, 1005, new RecordBook.Shown("pattern", 0, 0, 1, 1), code(1, 7, 0, 0));
        Check.equalLong(0, records(b, RecordBook.VSYNC).size(), "a frame waits for its FrameMetrics report");
        b.metrics(1000, 990, 40, 3, false, 0);
        List<String> vsyncs = records(b, RecordBook.VSYNC);
        Check.equalLong(1, vsyncs.size(), "one vsync record");
        Check.equal("{\"stream\":\"vsync\",\"seq\":1,\"type\":\"vsync\",\"counter\":7,\"mode\":\"pattern\","
                + "\"t_vsync_dev_ns\":1000,\"t_callback_dev_ns\":1002,\"t_draw_dev_ns\":1005,\"t_drawn_dev_ns\":1030,"
                + "\"metrics\":{\"intended_vsync_dev_ns\":990,\"total_duration_ns\":40,\"gpu_duration_ns\":3,"
                + "\"first_draw_frame\":false},"
                + "\"content\":{\"code\":true,\"code_mode\":1,\"code_counter\":7,\"event\":0,\"aux\":0},"
                + "\"null_reasons\":{}}",
                vsyncs.get(0), "vsync record");
    }

    public static void testFlipRecordTakesItsFirstDrawAndReport() throws Exception {
        RecordBook b = book(true);
        b.await(RecordBook.TYPE_FLIP, 1, Json.obj().put("n", 1L).put("j", 5).put("x", 400.5).put("y", 200.25)
                .put("t_evt_dev_ns", 900L).put("t_evt_resolution", "ns").put("t_handled_dev_ns", 950L), 950);
        b.frameDrawn(7, 1000, 1002, 1005, new RecordBook.Shown("flip", 1, 0, 1, 1), code(2, 7, 1, 0x58));
        Check.equalLong(0, records(b, RecordBook.EVENT).size(), "the flip waits for the frame's report");
        b.metrics(1000, 990, 40, -1, true, 0);
        List<String> events = records(b, RecordBook.EVENT);
        Check.equalLong(1, events.size(), "one flip record");
        Check.equal("{\"stream\":\"event\",\"seq\":1,\"type\":\"flip\",\"n\":1,\"j\":5,\"x\":400.5,\"y\":200.25,"
                + "\"t_evt_dev_ns\":900,\"t_evt_resolution\":\"ns\",\"t_handled_dev_ns\":950,\"counter\":7,"
                + "\"t_vsync_first_draw_dev_ns\":1000,\"t_drawn_dev_ns\":1030,\"superseded_by\":null,\"null_reasons\":{}}",
                events.get(0), "flip record");
        Check.isTrue(records(b, RecordBook.VSYNC).get(0).contains("\"metrics\":{\"intended_vsync_dev_ns\":990,"
                + "\"total_duration_ns\":40,\"gpu_duration_ns\":null,\"first_draw_frame\":true},"),
                "metrics without a GPU duration");
        Check.isTrue(records(b, RecordBook.VSYNC).get(0).endsWith(
                "\"null_reasons\":{\"metrics.gpu_duration_ns\":\"not_reported_below_api_31\"}}"), "its null reason");
    }

    public static void testTwoTapsInOneFrameSupersedeTheFirst() throws Exception {
        RecordBook b = book(true);
        b.await(RecordBook.TYPE_FLIP, 1, Json.obj().put("n", 1L), 100);
        b.await(RecordBook.TYPE_FLIP, 2, Json.obj().put("n", 2L), 110);
        b.frameDrawn(8, 2000, 2001, 2003, new RecordBook.Shown("flip", 2, 0, 1, 1), code(2, 8, 2, 0x30));
        List<String> events = records(b, RecordBook.EVENT);
        Check.equalLong(1, events.size(), "the superseded flip is final at once");
        Check.equal("{\"stream\":\"event\",\"seq\":1,\"type\":\"flip\",\"n\":1,\"counter\":null,"
                + "\"t_vsync_first_draw_dev_ns\":null,\"t_drawn_dev_ns\":null,\"superseded_by\":2,"
                + "\"null_reasons\":{\"counter\":\"superseded\",\"t_vsync_first_draw_dev_ns\":\"superseded\","
                + "\"t_drawn_dev_ns\":\"superseded\"}}", events.get(0), "superseded flip");
        b.metrics(2000, 1990, 30, 2, false, 0);
        events = records(b, RecordBook.EVENT);
        Check.equal("{\"stream\":\"event\",\"seq\":2,\"type\":\"flip\",\"n\":2,\"counter\":8,"
                + "\"t_vsync_first_draw_dev_ns\":2000,\"t_drawn_dev_ns\":2020,\"superseded_by\":null,\"null_reasons\":{}}",
                events.get(1), "drawn flip");
    }

    public static void testAFlipShownInAnotherModeIsNotDrawn() throws Exception {
        RecordBook b = book(true);
        b.await(RecordBook.TYPE_FLIP, 3, Json.obj().put("n", 3L), 100);
        b.frameDrawn(9, 3000, 3001, 3002, new RecordBook.Shown("pattern", 3, 0, 2, 1), code(1, 9, 0, 0));
        List<String> events = records(b, RecordBook.EVENT);
        Check.equalLong(1, events.size(), "final at once");
        Check.isTrue(events.get(0).endsWith("\"superseded_by\":null,\"null_reasons\":{\"counter\":\"mode_changed\","
                + "\"t_vsync_first_draw_dev_ns\":\"mode_changed\",\"t_drawn_dev_ns\":\"mode_changed\"}}"),
                "mode_changed: " + events.get(0));
    }

    public static void testStepModeAndConfigLinkInOrder() throws Exception {
        RecordBook b = book(true);
        b.await(RecordBook.TYPE_STEP, 1, Json.obj().put("k", 1L), 100);
        b.await(RecordBook.TYPE_MODE, 2, Json.obj().put("mode_seq", 2L), 101);
        b.await(RecordBook.TYPE_CONFIG, 3, Json.obj().put("config_seq", 3L), 102);
        b.await(RecordBook.TYPE_STEP, 2, Json.obj().put("k", 2L), 103);
        b.frameDrawn(9, 3000, 3001, 3002, new RecordBook.Shown("step", 0, 1, 2, 3), code(3, 9, 1, 8));
        b.metrics(3000, 2990, 20, -1, false, 0);
        List<String> events = records(b, RecordBook.EVENT);
        Check.equalLong(3, events.size(), "step 1, mode 2 and config 3 are drawn");
        Check.equal("{\"stream\":\"event\",\"seq\":1,\"type\":\"step\",\"k\":1,\"counter\":9,\"t_vsync_dev_ns\":3000,"
                + "\"t_drawn_dev_ns\":3010,\"superseded_by\":null,\"null_reasons\":{}}", events.get(0), "step record");
        Check.isTrue(events.get(1).contains("\"type\":\"mode\",\"mode_seq\":2,\"counter\":9,\"t_vsync_dev_ns\":3000,"),
                "mode: " + events.get(1));
        Check.isTrue(events.get(2).contains("\"type\":\"config\",\"config_seq\":3,\"counter\":9,\"t_vsync_dev_ns\":3000,"),
                "config: " + events.get(2));
        Check.equal(Integer.valueOf(1), b.stats().get("records_awaiting_draw"), "step 2 still waits");
    }

    public static void testAMissingReportTimesOut() throws Exception {
        RecordBook b = book(true);
        b.frameDrawn(7, 1000, 1002, 1005, new RecordBook.Shown("pattern", 0, 0, 1, 1), code(1, 7, 0, 0));
        b.sweep(2004);
        Check.equalLong(0, records(b, RecordBook.VSYNC).size(), "before the metrics timeout");
        b.sweep(2005);
        List<String> vsyncs = records(b, RecordBook.VSYNC);
        Check.equalLong(1, vsyncs.size(), "after the metrics timeout");
        Check.isTrue(vsyncs.get(0).contains("\"t_drawn_dev_ns\":null,\"metrics\":null,"), "timed out: " + vsyncs.get(0));
        Check.isTrue(vsyncs.get(0).endsWith("\"null_reasons\":{\"t_drawn_dev_ns\":\"metrics_not_reported\","
                + "\"metrics\":\"metrics_not_reported\"}}"), "reasons: " + vsyncs.get(0));
        Check.equal(Long.valueOf(1), b.stats().get("frames_without_metrics"), "counted");
        b.metrics(1000, 990, 40, -1, false, 2);
        Check.equal(Long.valueOf(1), b.stats().get("metrics_unmatched"), "a late report matches nothing");
        Check.equal(Long.valueOf(2), b.stats().get("metrics_dropped_reported"), "dropped reports are counted");
    }

    public static void testAnUndrawnRecordTimesOut() throws Exception {
        RecordBook b = book(true);
        b.await(RecordBook.TYPE_MODE, 1, Json.obj().put("mode_seq", 1L), 100);
        b.sweep(5099);
        Check.equalLong(0, records(b, RecordBook.EVENT).size(), "before the draw timeout");
        b.sweep(5100);
        List<String> events = records(b, RecordBook.EVENT);
        Check.equal("{\"stream\":\"event\",\"seq\":1,\"type\":\"mode\",\"mode_seq\":1,\"counter\":null,"
                + "\"t_vsync_dev_ns\":null,\"t_drawn_dev_ns\":null,\"superseded_by\":null,\"null_reasons\":{"
                + "\"counter\":\"not_drawn_within_timeout\",\"t_vsync_dev_ns\":\"not_drawn_within_timeout\","
                + "\"t_drawn_dev_ns\":\"not_drawn_within_timeout\"}}", events.get(0), "timed out");
        Check.equal(Long.valueOf(1), b.stats().get("records_not_drawn"), "counted");
    }

    public static void testWithoutFrameMetricsRecordsAreFinalAtTheDraw() throws Exception {
        RecordBook b = book(false);
        b.await(RecordBook.TYPE_FLIP, 1, Json.obj().put("n", 1L), 100);
        b.frameDrawn(7, 1000, 1002, 1005, new RecordBook.Shown("flip", 1, 0, 1, 1), code(2, 7, 1, 0x08));
        Check.isTrue(records(b, RecordBook.VSYNC).get(0).endsWith("\"null_reasons\":{\"t_drawn_dev_ns\":"
                + "\"frame_metrics_unavailable\",\"metrics\":\"frame_metrics_unavailable\"}}"), "vsync record");
        Check.equal("{\"stream\":\"event\",\"seq\":1,\"type\":\"flip\",\"n\":1,\"counter\":7,"
                + "\"t_vsync_first_draw_dev_ns\":1000,\"t_drawn_dev_ns\":null,\"superseded_by\":null,"
                + "\"null_reasons\":{\"t_drawn_dev_ns\":\"frame_metrics_unavailable\"}}",
                records(b, RecordBook.EVENT).get(0), "flip record");
    }

    public static void testSwitchingFrameMetricsOffFinalizesWaitingFrames() throws Exception {
        RecordBook b = book(true);
        b.frameDrawn(7, 1000, 1002, 1005, new RecordBook.Shown("pattern", 0, 0, 1, 1), code(1, 7, 0, 0));
        b.setMetricsAvailable(false);
        Check.isTrue(records(b, RecordBook.VSYNC).get(0).contains("\"t_drawn_dev_ns\":\"frame_metrics_unavailable\""),
                "finalized");
        Check.equal(Boolean.FALSE, b.stats().get("frame_metrics"), "reported off");
    }

    public static void testImmediateEventsAndRefusals() throws Exception {
        final RecordBook b = book(true);
        b.event(Json.obj().put("type", "put").put("slot", "a"));
        Check.equal("{\"stream\":\"event\",\"seq\":1,\"type\":\"put\",\"slot\":\"a\"}", records(b, RecordBook.EVENT).get(0),
                "an immediate event");
        Check.fails(IllegalArgumentException.class, () -> b.event(Json.obj().put("slot", "a")), "an event without a type");
        Check.fails(IllegalArgumentException.class, () -> b.await("put", 1, Json.obj(), 0), "a type that never waits");
        Check.fails(IllegalArgumentException.class,
                () -> b.await(RecordBook.TYPE_FLIP, 1, Json.obj().put("type", "flip"), 0), "type given twice");
        Check.refused("bad_arguments", false, () -> b.read("frame", 0, 1), "an unknown stream");
        Check.equal("t_vsync_first_draw_dev_ns", RecordBook.firstDrawVsyncField(RecordBook.TYPE_FLIP), "flip field");
        Check.equal("t_vsync_dev_ns", RecordBook.firstDrawVsyncField(RecordBook.TYPE_STEP), "step field");
    }
}
