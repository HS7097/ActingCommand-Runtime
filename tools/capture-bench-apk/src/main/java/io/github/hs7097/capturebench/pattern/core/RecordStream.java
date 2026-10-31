package io.github.hs7097.capturebench.pattern.core;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * One record stream (PROTOCOL.md, "Records"): a ring of the newest {@code capacity} records,
 * numbered 1, 2, 3... in append order. A record is immutable once appended. A reader pages
 * through with a cursor: {@code read(after, max)} returns the records with seq > after; records
 * the ring has already overwritten are counted in {@code lost}, never silently skipped.
 */
public final class RecordStream {
    private final String name;
    private final String[] ring;
    private long newest;

    public RecordStream(String name, int capacity) {
        if (name == null || name.isEmpty()) {
            throw new IllegalArgumentException("a stream needs a name");
        }
        if (capacity < 1) {
            throw new IllegalArgumentException("capacity must be at least 1: " + capacity);
        }
        this.name = name;
        this.ring = new String[capacity];
    }

    public String name() {
        return name;
    }

    public int capacity() {
        return ring.length;
    }

    /** Appends a record: "stream" and "seq" come first, then the body's keys in order. Returns the seq. */
    public synchronized long append(Json.Obj body) {
        long seq = newest + 1;
        Json.Obj record = Json.obj().put("stream", name).put("seq", seq);
        for (String key : body.keys()) {
            if ("stream".equals(key) || "seq".equals(key)) {
                throw new IllegalArgumentException("a record body must not set " + key);
            }
            record.put(key, body.get(key));
        }
        ring[slot(seq)] = record.toJson();
        newest = seq;
        return seq;
    }

    public synchronized long newest() {
        return newest;
    }

    /** The oldest seq still held, 0 while the stream is empty. */
    public synchronized long oldest() {
        return newest == 0 ? 0 : Math.max(1, newest - ring.length + 1);
    }

    /** A page of records after a cursor. */
    public static final class Page {
        public final String stream;
        public final long after;
        public final long next;
        public final long oldest;
        public final long newest;
        public final long lost;
        public final List<String> records;

        Page(String stream, long after, long next, long oldest, long newest, long lost, List<String> records) {
            this.stream = stream;
            this.after = after;
            this.next = next;
            this.oldest = oldest;
            this.newest = newest;
            this.lost = lost;
            this.records = Collections.unmodifiableList(records);
        }

        /** The RECORDS response line. */
        public Json.Obj toJson() {
            List<Object> raw = new ArrayList<Object>();
            for (String record : records) {
                raw.add(new Json.Raw(record));
            }
            return Json.obj()
                    .put("ok", Boolean.TRUE)
                    .put("cmd", "RECORDS")
                    .put("stream", stream)
                    .put("after", after)
                    .put("next", next)
                    .put("oldest", oldest)
                    .put("newest", newest)
                    .put("lost", lost)
                    .put("count", records.size())
                    .put("records", raw);
        }
    }

    /**
     * The records with seq > after, at most {@code max}. A cursor beyond the newest record is
     * refused with {@code cursor_ahead} (the reader holds a cursor of another process instance).
     */
    public synchronized Page read(long after, int max) throws ProtocolError {
        if (after < 0) {
            throw new ProtocolError("bad_arguments", "after must be >= 0: " + after);
        }
        if (max < 1) {
            throw new ProtocolError("bad_arguments", "max must be >= 1: " + max);
        }
        if (after > newest) {
            throw new ProtocolError("cursor_ahead", "stream " + name + " has records up to " + newest
                    + ", the cursor is " + after);
        }
        long oldest = oldest();
        long first = after + 1;
        long lost = 0;
        if (newest > 0 && first < oldest) {
            lost = oldest - first;
            first = oldest;
        }
        List<String> out = new ArrayList<String>();
        long seq = first;
        while (seq <= newest && out.size() < max) {
            out.add(ring[slot(seq)]);
            seq++;
        }
        long next = out.isEmpty() ? after : seq - 1;
        return new Page(name, after, next, oldest, newest, lost, out);
    }

    private int slot(long seq) {
        return (int) ((seq - 1) % ring.length);
    }
}
