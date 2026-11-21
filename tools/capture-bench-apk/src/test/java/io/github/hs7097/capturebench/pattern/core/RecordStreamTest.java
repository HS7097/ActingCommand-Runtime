package io.github.hs7097.capturebench.pattern.core;

/** Stream numbering, the cursor and the lost count of RECORDS. */
final class RecordStreamTest {
    private static RecordStream filled(int capacity, int records) {
        RecordStream s = new RecordStream("vsync", capacity);
        for (int i = 1; i <= records; i++) {
            Check.equalLong(i, s.append(Json.obj().put("type", "x").put("i", i)), "seq of record " + i);
        }
        return s;
    }

    public static void testStreamAndSeqComeFirst() throws Exception {
        RecordStream s = filled(4, 1);
        RecordStream.Page p = s.read(0, 10);
        Check.equal("{\"stream\":\"vsync\",\"seq\":1,\"type\":\"x\",\"i\":1}", p.records.get(0), "record text");
    }

    public static void testOverwrittenRecordsAreCountedLost() throws Exception {
        RecordStream s = filled(3, 5);
        Check.equalLong(5, s.newest(), "newest");
        Check.equalLong(3, s.oldest(), "oldest");
        RecordStream.Page p = s.read(0, 10);
        Check.equal("{\"ok\":true,\"cmd\":\"RECORDS\",\"stream\":\"vsync\",\"after\":0,\"next\":5,\"oldest\":3,\"newest\":5,"
                + "\"lost\":2,\"count\":3,\"records\":["
                + "{\"stream\":\"vsync\",\"seq\":3,\"type\":\"x\",\"i\":3},"
                + "{\"stream\":\"vsync\",\"seq\":4,\"type\":\"x\",\"i\":4},"
                + "{\"stream\":\"vsync\",\"seq\":5,\"type\":\"x\",\"i\":5}]}",
                p.toJson().toJson(), "RECORDS response");
        Check.equalLong(0, s.read(2, 10).lost, "a cursor at oldest - 1 loses nothing");
        Check.equalLong(1, s.read(1, 10).lost, "a cursor at 1 lost record 2");
    }

    public static void testPagesWithMax() throws Exception {
        RecordStream s = filled(8, 5);
        RecordStream.Page p = s.read(3, 1);
        Check.equalLong(1, p.records.size(), "one record");
        Check.isTrue(p.records.get(0).contains("\"seq\":4"), "record 4: " + p.records.get(0));
        Check.equalLong(4, p.next, "next");
        RecordStream.Page end = s.read(5, 10);
        Check.equalLong(0, end.records.size(), "nothing after the newest");
        Check.equalLong(5, end.next, "next stays at the cursor");
    }

    public static void testCursorAheadAndBadArguments() {
        final RecordStream s = filled(8, 5);
        Check.refused("cursor_ahead", false, () -> s.read(6, 1), "a cursor beyond the newest");
        Check.refused("bad_arguments", false, () -> s.read(-1, 1), "a negative cursor");
        Check.refused("bad_arguments", false, () -> s.read(0, 0), "max 0");
    }

    public static void testEmptyStream() throws Exception {
        final RecordStream s = new RecordStream("event", 4);
        Check.equalLong(0, s.oldest(), "oldest of an empty stream");
        RecordStream.Page p = s.read(0, 5);
        Check.equalLong(0, p.records.size(), "no records");
        Check.equalLong(0, p.next, "next");
        Check.equalLong(0, p.lost, "lost");
        Check.refused("cursor_ahead", false, () -> s.read(1, 5), "cursor 1 of an empty stream");
    }

    public static void testBodyMustNotSetStreamOrSeq() {
        final RecordStream s = new RecordStream("event", 4);
        Check.fails(IllegalArgumentException.class, () -> s.append(Json.obj().put("seq", 9)), "a body with seq");
        Check.fails(IllegalArgumentException.class, () -> s.append(Json.obj().put("stream", "x")), "a body with stream");
        Check.fails(IllegalArgumentException.class, () -> new RecordStream("event", 0), "capacity 0");
    }
}
