package io.github.hs7097.capturebench.pattern.core;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;

/**
 * Reads the request stream of one connection: LF-terminated request lines, and the exact-length
 * payload that follows a PUT header. Both share one buffer, so a payload that arrives in the same
 * read as its header is not lost. Every malformed end of the stream is a {@link ProtocolError}
 * that closes the connection.
 */
public final class LineReader {
    private final InputStream in;
    private final byte[] buffer;
    private int position;
    private int end;

    public LineReader(InputStream in) {
        this(in, 8192);
    }

    public LineReader(InputStream in, int bufferSize) {
        if (in == null) {
            throw new IllegalArgumentException("no input stream");
        }
        if (bufferSize < 1) {
            throw new IllegalArgumentException("buffer size must be at least 1: " + bufferSize);
        }
        this.in = in;
        this.buffer = new byte[bufferSize];
    }

    /**
     * The next line without its LF, or null when the stream ends cleanly before a new line starts.
     * Bytes are taken as ISO-8859-1 characters; {@link Command#parse} refuses anything that is not
     * printable ASCII.
     */
    public String readLine(int maxBytes) throws IOException, ProtocolError {
        StringBuilder line = new StringBuilder();
        boolean started = false;
        while (true) {
            if (position == end && !fill()) {
                if (!started) {
                    return null;
                }
                throw new ProtocolError("bad_arguments", "the connection ended inside a request line", true);
            }
            started = true;
            byte b = buffer[position++];
            if (b == '\n') {
                return line.toString();
            }
            if (line.length() >= maxBytes) {
                throw new ProtocolError("line_too_long", "a request line is at most " + maxBytes + " bytes", true);
            }
            line.append((char) (b & 0xFF));
        }
    }

    /** Copies exactly {@code length} bytes to {@code out}. */
    public void copy(OutputStream out, long length) throws IOException, ProtocolError {
        long left = length;
        while (left > 0) {
            if (position == end && !fill()) {
                throw new ProtocolError("payload_short", "the connection ended " + left + " bytes before the end of a "
                        + length + "-byte payload", true);
            }
            int n = (int) Math.min(left, (long) (end - position));
            out.write(buffer, position, n);
            position += n;
            left -= n;
        }
    }

    private boolean fill() throws IOException {
        while (true) {
            int n = in.read(buffer, 0, buffer.length);
            if (n < 0) {
                return false;
            }
            if (n > 0) {
                position = 0;
                end = n;
                return true;
            }
        }
    }
}
