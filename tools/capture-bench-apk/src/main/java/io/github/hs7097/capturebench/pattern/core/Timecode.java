package io.github.hs7097.capturebench.pattern.core;

import java.util.Arrays;
import java.util.zip.CRC32;

/**
 * Timecode v1 (PROTOCOL.md, "Timecode v1"): a band of {@link #ROWS} x {@link #COLUMNS} black or
 * white cells. Row 0 is the fixed sync row {@link #SYNC}; rows 1..7 carry the 21-byte codeword,
 * most significant bit first, row by row, left to right. The codeword is the 17-byte payload
 * followed by its CRC-32 (IEEE, as zlib.crc32), big-endian. White is bit 1.
 *
 * <p>Payload: byte 0 = version (high nibble) and mode (low nibble); bytes 1..4 = frame sequence
 * (uint32); bytes 5..12 = {@code T_vsync}, the Choreographer {@code frameTimeNanos} (int64);
 * bytes 13..15 = event (uint24: the flip number n or the step number k); byte 16 = aux (flip:
 * j in the high nibble, level in bit 3; step: level in bit 3).
 */
public final class Timecode {
    private Timecode() {}

    public static final int VERSION = 1;
    public static final int COLUMNS = 24;
    public static final int ROWS = 8;
    public static final int DATA_ROWS = ROWS - 1;
    public static final int PAYLOAD_BYTES = 17;
    public static final int CODEWORD_BYTES = 21;
    /** Row 0, column 0 is the most significant of these 24 bits. */
    public static final int SYNC = 0xB38F0F;

    public static final int MODE_PATTERN = 1;
    public static final int MODE_FLIP = 2;
    public static final int MODE_STEP = 3;

    /** Aux bit 3: the flip block (flip mode) or the background (step mode) is white. */
    public static final int AUX_LEVEL = 0x08;

    /** The sync row's mean white minus mean black, in luma units of 0..255, below which nothing is read. */
    public static final double MIN_CONTRAST = 64.0;

    /** The values one frame carries. */
    public static final class Fields {
        public final int mode;
        public final long seq;
        public final long tVsync;
        public final int event;
        public final int aux;

        public Fields(int mode, long seq, long tVsync, int event, int aux) {
            if (mode < MODE_PATTERN || mode > MODE_STEP) {
                throw new IllegalArgumentException("mode must be 1..3: " + mode);
            }
            if (seq < 0 || seq > 0xFFFFFFFFL) {
                throw new IllegalArgumentException("seq must be a uint32: " + seq);
            }
            if (event < 0 || event > 0xFFFFFF) {
                throw new IllegalArgumentException("event must be a uint24: " + event);
            }
            if (aux < 0 || aux > 0xFF) {
                throw new IllegalArgumentException("aux must be a uint8: " + aux);
            }
            this.mode = mode;
            this.seq = seq;
            this.tVsync = tVsync;
            this.event = event;
            this.aux = aux;
        }

        @Override
        public boolean equals(Object other) {
            if (!(other instanceof Fields)) {
                return false;
            }
            Fields o = (Fields) other;
            return mode == o.mode && seq == o.seq && tVsync == o.tVsync && event == o.event && aux == o.aux;
        }

        @Override
        public int hashCode() {
            return Arrays.hashCode(new long[] {mode, seq, tVsync, event, aux});
        }

        @Override
        public String toString() {
            return "Fields{mode=" + mode + ", seq=" + seq + ", tVsync=" + tVsync + ", event=" + event
                    + ", aux=" + aux + "}";
        }
    }

    /** How a decode ended; the names in PROTOCOL.md are the lower-case forms. */
    public enum Status {
        OK,
        NO_SYNC,
        CRC_MISMATCH,
        UNSUPPORTED_VERSION,
        BAD_MODE
    }

    /** A decode result. {@link #fields} is set only for {@link Status#OK}. */
    public static final class Decoded {
        public final Status status;
        public final Fields fields;
        /** The luma threshold taken from the sync row (NaN when the codeword was given directly). */
        public final double threshold;
        /** Mean white minus mean black of the sync row. */
        public final double contrast;
        /** The smallest distance of any sampled cell from the threshold. */
        public final double margin;

        Decoded(Status status, Fields fields, double threshold, double contrast, double margin) {
            this.status = status;
            this.fields = fields;
            this.threshold = threshold;
            this.contrast = contrast;
            this.margin = margin;
        }
    }

    /** The mean luma (0..255) of the sample square of code cell (row, col); see {@link Layout#sampleRect}. */
    public interface LumaSampler {
        double meanLuma(int row, int col);
    }

    public static boolean syncBit(int col) {
        if (col < 0 || col >= COLUMNS) {
            throw new IllegalArgumentException("column out of range: " + col);
        }
        return ((SYNC >>> (COLUMNS - 1 - col)) & 1) != 0;
    }

    public static byte[] payload(Fields f) {
        byte[] p = new byte[PAYLOAD_BYTES];
        p[0] = (byte) ((VERSION << 4) | f.mode);
        writeBigEndian(p, 1, 4, f.seq);
        writeBigEndian(p, 5, 8, f.tVsync);
        writeBigEndian(p, 13, 3, f.event);
        p[16] = (byte) f.aux;
        return p;
    }

    public static byte[] codeword(Fields f) {
        byte[] p = payload(f);
        byte[] cw = Arrays.copyOf(p, CODEWORD_BYTES);
        writeBigEndian(cw, PAYLOAD_BYTES, 4, crc32(p, 0, PAYLOAD_BYTES));
        return cw;
    }

    public static long crc32(byte[] data, int offset, int length) {
        CRC32 crc = new CRC32();
        crc.update(data, offset, length);
        return crc.getValue();
    }

    /** The band as cells, [row][column], true = white. */
    public static boolean[][] cells(Fields f) {
        byte[] cw = codeword(f);
        boolean[][] cells = new boolean[ROWS][COLUMNS];
        for (int c = 0; c < COLUMNS; c++) {
            cells[0][c] = syncBit(c);
        }
        for (int r = 1; r < ROWS; r++) {
            for (int c = 0; c < COLUMNS; c++) {
                int bit = (r - 1) * COLUMNS + c;
                cells[r][c] = (cw[bit >> 3] & (0x80 >>> (bit & 7))) != 0;
            }
        }
        return cells;
    }

    /** Decodes sampled cells by the rules of PROTOCOL.md ("Decoding"). */
    public static Decoded decode(LumaSampler sampler) {
        double[] sync = new double[COLUMNS];
        double whiteSum = 0;
        double blackSum = 0;
        int whiteCount = 0;
        int blackCount = 0;
        for (int c = 0; c < COLUMNS; c++) {
            double luma = sampler.meanLuma(0, c);
            sync[c] = luma;
            if (syncBit(c)) {
                whiteSum += luma;
                whiteCount++;
            } else {
                blackSum += luma;
                blackCount++;
            }
        }
        double white = whiteSum / whiteCount;
        double black = blackSum / blackCount;
        double contrast = white - black;
        double threshold = (white + black) / 2;
        if (!(contrast >= MIN_CONTRAST)) {
            return new Decoded(Status.NO_SYNC, null, threshold, contrast, 0);
        }
        double margin = Double.POSITIVE_INFINITY;
        for (int c = 0; c < COLUMNS; c++) {
            if ((sync[c] >= threshold) != syncBit(c)) {
                return new Decoded(Status.NO_SYNC, null, threshold, contrast, 0);
            }
            margin = Math.min(margin, Math.abs(sync[c] - threshold));
        }
        byte[] cw = new byte[CODEWORD_BYTES];
        for (int r = 1; r < ROWS; r++) {
            for (int c = 0; c < COLUMNS; c++) {
                double luma = sampler.meanLuma(r, c);
                margin = Math.min(margin, Math.abs(luma - threshold));
                if (luma >= threshold) {
                    int bit = (r - 1) * COLUMNS + c;
                    cw[bit >> 3] |= (byte) (0x80 >>> (bit & 7));
                }
            }
        }
        Decoded checked = decodeCodeword(cw);
        return new Decoded(checked.status, checked.fields, threshold, contrast, margin);
    }

    /** Decodes cells given as bits (true = white), for example the output of {@link #cells}. */
    public static Decoded decodeBits(final boolean[][] cells) {
        if (cells.length != ROWS) {
            throw new IllegalArgumentException("expected " + ROWS + " rows: " + cells.length);
        }
        for (boolean[] row : cells) {
            if (row.length != COLUMNS) {
                throw new IllegalArgumentException("expected " + COLUMNS + " columns: " + row.length);
            }
        }
        return decode(new LumaSampler() {
            @Override
            public double meanLuma(int row, int col) {
                return cells[row][col] ? 255.0 : 0.0;
            }
        });
    }

    /** Checks the CRC, version and mode of a 21-byte codeword and reads its fields. */
    public static Decoded decodeCodeword(byte[] cw) {
        if (cw.length != CODEWORD_BYTES) {
            throw new IllegalArgumentException("a codeword has " + CODEWORD_BYTES + " bytes: " + cw.length);
        }
        long expected = crc32(cw, 0, PAYLOAD_BYTES);
        long actual = readBigEndian(cw, PAYLOAD_BYTES, 4);
        if (expected != actual) {
            return new Decoded(Status.CRC_MISMATCH, null, Double.NaN, Double.NaN, Double.NaN);
        }
        int version = (cw[0] >> 4) & 0xF;
        if (version != VERSION) {
            return new Decoded(Status.UNSUPPORTED_VERSION, null, Double.NaN, Double.NaN, Double.NaN);
        }
        int mode = cw[0] & 0xF;
        if (mode < MODE_PATTERN || mode > MODE_STEP) {
            return new Decoded(Status.BAD_MODE, null, Double.NaN, Double.NaN, Double.NaN);
        }
        Fields fields = new Fields(
                mode,
                readBigEndian(cw, 1, 4),
                readBigEndian(cw, 5, 8),
                (int) readBigEndian(cw, 13, 3),
                cw[16] & 0xFF);
        return new Decoded(Status.OK, fields, Double.NaN, Double.NaN, Double.NaN);
    }

    static void writeBigEndian(byte[] out, int offset, int length, long value) {
        for (int i = 0; i < length; i++) {
            out[offset + i] = (byte) (value >>> (8 * (length - 1 - i)));
        }
    }

    static long readBigEndian(byte[] in, int offset, int length) {
        long value = 0;
        for (int i = 0; i < length; i++) {
            value = (value << 8) | (in[offset + i] & 0xFFL);
        }
        return value;
    }

    /** Lower-case hex of {@code bytes}, as PROTOCOL.md writes the test vectors. */
    public static String hex(byte[] bytes) {
        StringBuilder out = new StringBuilder(bytes.length * 2);
        for (byte b : bytes) {
            out.append(Character.forDigit((b >> 4) & 0xF, 16));
            out.append(Character.forDigit(b & 0xF, 16));
        }
        return out.toString();
    }
}
