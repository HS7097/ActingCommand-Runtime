package io.github.hs7097.capturebench.pattern.core;

import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.Random;

/**
 * Timecode v1 encoder and decoder. The vectors were computed by an independent Python
 * implementation (zlib.crc32) and are the ones PROTOCOL.md lists.
 */
final class TimecodeTest {
    private static final String[] ROWS_A = {
        "101100111000111100001111",
        "000100100000000100000010",
        "000000110000010000010001",
        "001000100011001101000100",
        "010101010110011001110111",
        "100010001010101111001101",
        "111011110101101011001010",
        "000101111110010101110110",
    };

    static Timecode.Fields vectorA() {
        return new Timecode.Fields(Timecode.MODE_FLIP, 0x01020304L, 0x1122334455667788L, 0xABCDEF, 0x5A);
    }

    static Timecode.Fields vectorB() {
        return new Timecode.Fields(Timecode.MODE_PATTERN, 1L, 123456789012345L, 0, 0);
    }

    static Timecode.Fields vectorC() {
        return new Timecode.Fields(Timecode.MODE_STEP, 0xFFFFFFFFL, Long.MAX_VALUE, 0xFFFFFF, Timecode.AUX_LEVEL);
    }

    public static void testCrc32IsTheIeeeCrc() {
        byte[] data = "123456789".getBytes(StandardCharsets.US_ASCII);
        Check.equalLong(0xCBF43926L, Timecode.crc32(data, 0, data.length), "CRC-32 check value of 123456789");
    }

    public static void testVectorA() {
        Check.equal("12010203041122334455667788abcdef5a", Timecode.hex(Timecode.payload(vectorA())), "payload A");
        Check.equal("12010203041122334455667788abcdef5aca17e576", Timecode.hex(Timecode.codeword(vectorA())),
                "codeword A");
        boolean[][] cells = Timecode.cells(vectorA());
        for (int r = 0; r < Timecode.ROWS; r++) {
            StringBuilder row = new StringBuilder();
            for (int c = 0; c < Timecode.COLUMNS; c++) {
                row.append(cells[r][c] ? '1' : '0');
            }
            Check.equal(ROWS_A[r], row.toString(), "row " + r + " of vector A");
        }
    }

    public static void testVectorsBAndC() {
        Check.equal("110000000100007048860ddf7900000000f4a49f58", Timecode.hex(Timecode.codeword(vectorB())),
                "codeword B");
        Check.equal("13ffffffff7fffffffffffffffffffff081d32b654", Timecode.hex(Timecode.codeword(vectorC())),
                "codeword C");
    }

    public static void testRoundTripThroughCells() {
        Random random = new Random(402);
        Timecode.Fields[] fixed = {vectorA(), vectorB(), vectorC()};
        for (int i = 0; i < 203; i++) {
            Timecode.Fields f = i < fixed.length ? fixed[i] : new Timecode.Fields(
                    1 + random.nextInt(3),
                    random.nextLong() & 0xFFFFFFFFL,
                    random.nextLong(),
                    random.nextInt(1 << 24),
                    random.nextInt(256));
            Timecode.Decoded d = Timecode.decodeBits(Timecode.cells(f));
            Check.equal(Timecode.Status.OK, d.status, "status of " + f);
            Check.equal(f, d.fields, "fields of " + f);
            Check.isTrue(d.margin == 127.5, "a clean band has the full margin: " + d.margin);
        }
    }

    public static void testEverySingleCellErrorFailsTheCrc() {
        boolean[][] clean = Timecode.cells(vectorA());
        for (int bit = 0; bit < Timecode.DATA_ROWS * Timecode.COLUMNS; bit++) {
            boolean[][] cells = copy(clean);
            int r = 1 + bit / Timecode.COLUMNS;
            int c = bit % Timecode.COLUMNS;
            cells[r][c] = !cells[r][c];
            Check.equal(Timecode.Status.CRC_MISMATCH, Timecode.decodeBits(cells).status, "flipped data cell " + r + "," + c);
        }
    }

    public static void testABrokenSyncRowIsNoSync() {
        boolean[][] cells = copy(Timecode.cells(vectorA()));
        cells[0][5] = !cells[0][5];
        Timecode.Decoded d = Timecode.decodeBits(cells);
        Check.equal(Timecode.Status.NO_SYNC, d.status, "sync cell 5 flipped");
        Check.equal(null, d.fields, "no fields without sync");
    }

    public static void testLowContrastIsNoSync() {
        final boolean[][] cells = Timecode.cells(vectorA());
        Timecode.Decoded d = Timecode.decode(new Timecode.LumaSampler() {
            @Override
            public double meanLuma(int row, int col) {
                return cells[row][col] ? 140.0 : 100.0;
            }
        });
        Check.equal(Timecode.Status.NO_SYNC, d.status, "contrast 40 < 64");
        Check.isTrue(d.contrast == 40.0, "contrast reported: " + d.contrast);
    }

    public static void testForgedVersionAndMode() {
        byte[] cw = Timecode.codeword(vectorA());
        cw[0] = 0x22;
        Timecode.writeBigEndian(cw, Timecode.PAYLOAD_BYTES, 4, Timecode.crc32(cw, 0, Timecode.PAYLOAD_BYTES));
        Check.equal(Timecode.Status.UNSUPPORTED_VERSION, Timecode.decodeCodeword(cw).status, "version 2");
        for (byte head : new byte[] {0x10, 0x14, 0x1F}) {
            cw[0] = head;
            Timecode.writeBigEndian(cw, Timecode.PAYLOAD_BYTES, 4, Timecode.crc32(cw, 0, Timecode.PAYLOAD_BYTES));
            Check.equal(Timecode.Status.BAD_MODE, Timecode.decodeCodeword(cw).status, "mode nibble of " + head);
        }
    }

    public static void testFieldRanges() {
        Check.fails(IllegalArgumentException.class, () -> new Timecode.Fields(0, 0, 0, 0, 0), "mode 0");
        Check.fails(IllegalArgumentException.class, () -> new Timecode.Fields(4, 0, 0, 0, 0), "mode 4");
        Check.fails(IllegalArgumentException.class, () -> new Timecode.Fields(1, -1, 0, 0, 0), "seq -1");
        Check.fails(IllegalArgumentException.class, () -> new Timecode.Fields(1, 1L << 32, 0, 0, 0), "seq 2^32");
        Check.fails(IllegalArgumentException.class, () -> new Timecode.Fields(1, 0, 0, -1, 0), "event -1");
        Check.fails(IllegalArgumentException.class, () -> new Timecode.Fields(1, 0, 0, 1 << 24, 0), "event 2^24");
        Check.fails(IllegalArgumentException.class, () -> new Timecode.Fields(1, 0, 0, 0, 256), "aux 256");
    }

    public static void testDecodesARenderedDegradedBandLandscape() {
        renderDegradeDecode(1280, 720);
    }

    public static void testDecodesARenderedDegradedBandPortrait() {
        renderDegradeDecode(720, 1280);
    }

    /**
     * Draws the band of vector A into a luma image, misregistered by (3, -2) px, then blurs it with
     * a 9x9 box, adds +-40 noise and squeezes the range to 30..225, as a stand-in for scaling and
     * H.264. The decoder samples the inner squares of the layout and must still read the fields.
     */
    private static void renderDegradeDecode(int width, int height) {
        final Layout layout = Layout.compute(width, height, Layout.MIN_CELL);
        Timecode.Fields f = vectorA();
        boolean[][] cells = Timecode.cells(f);
        int[] luma = new int[width * height];
        Arrays.fill(luma, 128);
        int dx = 3;
        int dy = -2;
        for (int r = 0; r < Timecode.ROWS; r++) {
            for (int c = 0; c < Timecode.COLUMNS; c++) {
                Layout.Rect cell = layout.codeCell(r, c);
                fill(luma, width, cell.left + dx, cell.top + dy, cell.right + dx, cell.bottom + dy, cells[r][c] ? 255 : 0);
            }
        }
        int margin = layout.cell;
        int x0 = layout.band.left - margin;
        int y0 = layout.band.top - margin;
        int x1 = layout.band.right + margin;
        int y1 = layout.band.bottom + margin;
        final int[] degraded = luma.clone();
        Random noise = new Random(7);
        int radius = 4;
        for (int y = y0; y < y1; y++) {
            for (int x = x0; x < x1; x++) {
                long sum = 0;
                int count = 0;
                for (int yy = Math.max(0, y - radius); yy <= Math.min(height - 1, y + radius); yy++) {
                    for (int xx = Math.max(0, x - radius); xx <= Math.min(width - 1, x + radius); xx++) {
                        sum += luma[yy * width + xx];
                        count++;
                    }
                }
                int v = (int) (sum / count) + noise.nextInt(81) - 40;
                v = 30 + v * 195 / 255;
                degraded[y * width + x] = Math.max(0, Math.min(255, v));
            }
        }
        final int w = width;
        Timecode.Decoded d = Timecode.decode(new Timecode.LumaSampler() {
            @Override
            public double meanLuma(int row, int col) {
                Layout.Rect s = layout.sampleRect(row, col);
                long sum = 0;
                for (int y = s.top; y < s.bottom; y++) {
                    for (int x = s.left; x < s.right; x++) {
                        sum += degraded[y * w + x];
                    }
                }
                return (double) sum / (s.width() * s.height());
            }
        });
        Check.equal(Timecode.Status.OK, d.status, width + "x" + height + " degraded band");
        Check.equal(f, d.fields, width + "x" + height + " fields");
        Check.isTrue(d.contrast > 150, "contrast after degradation: " + d.contrast);
        Check.isTrue(d.margin > 40, "margin after degradation: " + d.margin);
    }

    private static void fill(int[] luma, int width, int left, int top, int right, int bottom, int value) {
        for (int y = top; y < bottom; y++) {
            for (int x = left; x < right; x++) {
                luma[y * width + x] = value;
            }
        }
    }

    private static boolean[][] copy(boolean[][] cells) {
        boolean[][] out = new boolean[cells.length][];
        for (int r = 0; r < cells.length; r++) {
            out[r] = cells[r].clone();
        }
        return out;
    }
}
