package io.github.hs7097.capturebench.pattern.core;

/** The geometry of PROTOCOL.md ("Layout") on the two experiment screens and at the edges. */
final class LayoutTest {
    public static void testLandscape1280x720() {
        Layout l = Layout.compute(1280, 720, 24);
        Check.equal(new Layout.Rect(24, 24, 600, 216), l.band, "band");
        Check.equal(new Layout.Rect(624, 24, 816, 216), l.flipBlock, "flip block");
        Check.isTrue(l.flipBesideBand, "the flip block is beside the band");
        Check.equal(new Layout.Rect(24, 240, 1256, 336), l.track, "track");
        Check.equal("{\"width\":1280,\"height\":720,\"cell\":24,\"band\":[24,24,600,216],\"code_columns\":24,"
                + "\"code_rows\":8,\"sample_inset\":6,\"flip_block\":[624,24,816,216],\"flip_beside_band\":true,"
                + "\"track\":[24,240,1256,336],\"moving_size\":96,\"speed\":8,\"grid\":4}",
                l.toJson(8).toJson(), "INFO layout");
    }

    public static void testPortrait720x1280() {
        Layout l = Layout.compute(720, 1280, 24);
        Check.equal(new Layout.Rect(24, 24, 600, 216), l.band, "band");
        Check.equal(new Layout.Rect(24, 240, 216, 432), l.flipBlock, "flip block");
        Check.isTrue(!l.flipBesideBand, "the flip block is below the band");
        Check.equal(new Layout.Rect(24, 456, 696, 552), l.track, "track");
    }

    public static void testCellAndFitBounds() {
        Check.fails(IllegalArgumentException.class, () -> Layout.compute(1280, 720, 23), "cell 23 < 24");
        Check.fails(IllegalArgumentException.class, () -> Layout.compute(1280, 720, 257), "cell 257");
        Check.fails(IllegalArgumentException.class, () -> Layout.compute(720, 1280, 32), "band of 32 px cells on 720 px");
        Check.fails(IllegalArgumentException.class, () -> Layout.compute(1280, 300, 24), "track below the bottom");
        Check.fails(IllegalArgumentException.class, () -> Layout.compute(0, 720, 24), "no width");
        Layout l = Layout.compute(1280, 720, 32);
        Check.equal(new Layout.Rect(32, 32, 800, 288), l.band, "band of 32 px cells");
        Check.equal(new Layout.Rect(832, 32, 1088, 288), l.flipBlock, "flip block of 32 px cells");
    }

    public static void testCodeAndSampleCells() {
        Layout l = Layout.compute(1280, 720, 24);
        Check.equal(new Layout.Rect(24, 24, 48, 48), l.codeCell(0, 0), "first code cell");
        Check.equal(new Layout.Rect(576, 192, 600, 216), l.codeCell(7, 23), "last code cell");
        Check.equal(new Layout.Rect(30, 30, 42, 42), l.sampleRect(0, 0), "first sample square");
        Check.equalLong(6, l.sampleInset(), "sample inset");
        Check.fails(IllegalArgumentException.class, () -> l.codeCell(8, 0), "row 8");
        Check.fails(IllegalArgumentException.class, () -> l.codeCell(0, 24), "column 24");
    }

    public static void testTapGridRoundTrip() {
        int[][] sizes = {{1280, 720}, {720, 1280}, {1281, 721}};
        for (int[] size : sizes) {
            Layout l = Layout.compute(size[0], size[1], 24);
            for (int j = 0; j < Layout.GRID * Layout.GRID; j++) {
                Layout.Rect g = l.gridCell(j);
                String where = size[0] + "x" + size[1] + " cell " + j;
                Check.equalLong(j, l.tapCell(g.left, g.top), where + " top left");
                Check.equalLong(j, l.tapCell(g.right - 1, g.bottom - 1), where + " bottom right");
                Check.equalLong(j, l.tapCell((g.left + g.right) / 2f, (g.top + g.bottom) / 2f), where + " center");
            }
        }
    }

    public static void testTapGridValues() {
        Layout l = Layout.compute(1280, 720, 24);
        Check.equalLong(0, l.tapCell(0f, 0f), "origin");
        Check.equalLong(15, l.tapCell(1279f, 719f), "last pixel");
        Check.equalLong(10, l.tapCell(640f, 360f), "center");
        Check.equalLong(3, l.tapCell(960f, 0f), "top right cell");
        Check.equalLong(-1, l.tapCell(-1f, 0f), "left of the view");
        Check.equalLong(-1, l.tapCell(1280f, 0f), "right of the view");
        Check.equalLong(-1, l.tapCell(Float.NaN, 0f), "NaN");
        Layout odd = Layout.compute(1281, 721, 24);
        Check.equal(new Layout.Rect(0, 0, 321, 181), odd.gridCell(0), "odd-size grid cell 0");
        Check.equalLong(0, odd.tapCell(320f, 0f), "x 320 of 1281");
        Check.equalLong(1, odd.tapCell(321f, 0f), "x 321 of 1281");
    }

    public static void testMovingBlockWraps() {
        Layout l = Layout.compute(1280, 720, 24);
        Check.equalLong(96, l.movingSize(), "moving block size");
        Check.equalLong(24, l.movingLeft(0, 8), "frame 0");
        Check.equalLong(32, l.movingLeft(1, 8), "frame 1");
        Check.equalLong(1152, l.movingLeft(141, 8), "frame 141");
        Check.equalLong(24, l.movingLeft(142, 8), "frame 142 wraps");
        Check.equalLong(24, l.movingLeft(1000, 0), "speed 0");
    }
}
