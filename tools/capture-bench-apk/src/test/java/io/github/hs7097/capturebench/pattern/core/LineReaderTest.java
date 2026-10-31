package io.github.hs7097.capturebench.pattern.core;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.Random;

/** Request lines and PUT payloads read from one connection stream. */
final class LineReaderTest {
    /** Hands out at most {@code chunk} bytes a read, as a socket may. */
    private static final class Chunked extends InputStream {
        private final byte[] data;
        private final int chunk;
        private int position;

        Chunked(byte[] data, int chunk) {
            this.data = data;
            this.chunk = chunk;
        }

        @Override
        public int read() {
            return position < data.length ? data[position++] & 0xFF : -1;
        }

        @Override
        public int read(byte[] b, int off, int len) {
            if (position >= data.length) {
                return -1;
            }
            int n = Math.min(Math.min(len, chunk), data.length - position);
            System.arraycopy(data, position, b, off, n);
            position += n;
            return n;
        }
    }

    private static LineReader reader(String text, int buffer) {
        return new LineReader(new ByteArrayInputStream(text.getBytes(StandardCharsets.ISO_8859_1)), buffer);
    }

    public static void testLinesAndPayloadShareTheBuffer() throws Exception {
        for (int buffer : new int[] {1, 3, 8192}) {
            LineReader r = reader("HELLO\nPUT a 5\nab\ncdINFO\n", buffer);
            Check.equal("HELLO", r.readLine(4096), "first line, buffer " + buffer);
            Check.equal("PUT a 5", r.readLine(4096), "PUT header, buffer " + buffer);
            ByteArrayOutputStream out = new ByteArrayOutputStream();
            r.copy(out, 5);
            Check.equal("ab\ncd", new String(out.toByteArray(), StandardCharsets.ISO_8859_1), "payload, buffer " + buffer);
            Check.equal("INFO", r.readLine(4096), "line after the payload, buffer " + buffer);
            Check.equal(null, r.readLine(4096), "clean end, buffer " + buffer);
        }
    }

    public static void testLargePayloadInSmallReads() throws Exception {
        byte[] payload = new byte[20000];
        new Random(5).nextBytes(payload);
        byte[] head = "PUT big 20000\n".getBytes(StandardCharsets.US_ASCII);
        byte[] tail = "BYE\n".getBytes(StandardCharsets.US_ASCII);
        byte[] all = new byte[head.length + payload.length + tail.length];
        System.arraycopy(head, 0, all, 0, head.length);
        System.arraycopy(payload, 0, all, head.length, payload.length);
        System.arraycopy(tail, 0, all, head.length + payload.length, tail.length);
        LineReader r = new LineReader(new Chunked(all, 7), 4096);
        Check.equal("PUT big 20000", r.readLine(4096), "header");
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        r.copy(out, payload.length);
        Check.isTrue(Arrays.equals(payload, out.toByteArray()), "the payload arrives intact");
        Check.equal("BYE", r.readLine(4096), "the next request");
    }

    public static void testLineTooLongClosesTheConnection() throws Exception {
        final LineReader r = reader("ABCD\nABCDE\n", 8192);
        Check.equal("ABCD", r.readLine(4), "a line of exactly max bytes");
        Check.refused("line_too_long", true, () -> r.readLine(4), "a line of max + 1 bytes");
    }

    public static void testEndInsideALineOrPayload() {
        final LineReader line = reader("HEL", 8192);
        Check.refused("bad_arguments", true, () -> line.readLine(4096), "end inside a line");
        final LineReader payload = reader("x", 8192);
        Check.refused("payload_short", true, () -> payload.copy(new ByteArrayOutputStream(), 3), "end inside a payload");
    }

    public static void testEmptyStreamIsACleanEnd() throws Exception {
        Check.equal(null, reader("", 8192).readLine(4096), "no request at all");
    }
}
