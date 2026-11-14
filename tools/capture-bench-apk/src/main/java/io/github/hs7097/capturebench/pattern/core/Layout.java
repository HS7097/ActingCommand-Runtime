package io.github.hs7097.capturebench.pattern.core;

/**
 * The screen geometry of PROTOCOL.md ("Layout"), in view pixels, for a view of width x height and
 * a cell edge of {@code cell} pixels (at least {@link #MIN_CELL}). It depends only on those three
 * numbers, never on the mode, so a decoder that knows them knows where every block is.
 */
public final class Layout {
    /** Plan v2.1 §3.2 item 5: timecode blocks are at least 24 px. */
    public static final int MIN_CELL = 24;
    public static final int MAX_CELL = 256;
    /** The tap grid of the flip mode is GRID x GRID. */
    public static final int GRID = 4;
    /** The flip block is FLIP_CELLS cells square. */
    public static final int FLIP_CELLS = 8;
    /** The track is TRACK_CELLS cells high and the moving block TRACK_CELLS cells square. */
    public static final int TRACK_CELLS = 4;

    /** A rectangle; right and bottom are exclusive. */
    public static final class Rect {
        public final int left;
        public final int top;
        public final int right;
        public final int bottom;

        public Rect(int left, int top, int right, int bottom) {
            if (right < left || bottom < top) {
                throw new IllegalArgumentException("empty rectangle " + left + "," + top + "," + right + "," + bottom);
            }
            this.left = left;
            this.top = top;
            this.right = right;
            this.bottom = bottom;
        }

        public int width() {
            return right - left;
        }

        public int height() {
            return bottom - top;
        }

        public java.util.List<Object> toJson() {
            return Json.list(left, top, right, bottom);
        }

        @Override
        public boolean equals(Object other) {
            if (!(other instanceof Rect)) {
                return false;
            }
            Rect o = (Rect) other;
            return left == o.left && top == o.top && right == o.right && bottom == o.bottom;
        }

        @Override
        public int hashCode() {
            return ((left * 31 + top) * 31 + right) * 31 + bottom;
        }

        @Override
        public String toString() {
            return "[" + left + "," + top + "," + right + "," + bottom + "]";
        }
    }

    public final int width;
    public final int height;
    public final int cell;
    /** The timecode band: COLUMNS x ROWS cells at (cell, cell). */
    public final Rect band;
    /** The flip block, beside the band when it fits there, else below it. */
    public final Rect flipBlock;
    public final boolean flipBesideBand;
    /** The track the moving block runs along, below the band and the flip block. */
    public final Rect track;

    private Layout(int width, int height, int cell, Rect band, Rect flipBlock, boolean flipBesideBand, Rect track) {
        this.width = width;
        this.height = height;
        this.cell = cell;
        this.band = band;
        this.flipBlock = flipBlock;
        this.flipBesideBand = flipBesideBand;
        this.track = track;
    }

    /** Throws IllegalArgumentException when the cell is out of range or the blocks do not fit. */
    public static Layout compute(int width, int height, int cell) {
        if (cell < MIN_CELL || cell > MAX_CELL) {
            throw new IllegalArgumentException("cell must be " + MIN_CELL + ".." + MAX_CELL + " px: " + cell);
        }
        if (width <= 0 || height <= 0) {
            throw new IllegalArgumentException("the view has no size: " + width + "x" + height);
        }
        Rect band = new Rect(cell, cell, cell + Timecode.COLUMNS * cell, cell + Timecode.ROWS * cell);
        if (band.right + cell > width) {
            throw new IllegalArgumentException("layout does not fit: the band needs a width of "
                    + (band.right + cell) + " px, the view is " + width + " px wide");
        }
        int side = FLIP_CELLS * cell;
        Rect flip;
        boolean beside;
        if (band.right + cell + side + cell <= width) {
            flip = new Rect(band.right + cell, band.top, band.right + cell + side, band.top + side);
            beside = true;
        } else {
            flip = new Rect(cell, band.bottom + cell, cell + side, band.bottom + cell + side);
            beside = false;
        }
        int trackTop = Math.max(band.bottom, flip.bottom) + cell;
        Rect track = new Rect(cell, trackTop, width - cell, trackTop + TRACK_CELLS * cell);
        if (track.bottom + cell > height) {
            throw new IllegalArgumentException("layout does not fit: the track needs a height of "
                    + (track.bottom + cell) + " px, the view is " + height + " px high");
        }
        if (track.width() < 2 * TRACK_CELLS * cell) {
            throw new IllegalArgumentException("layout does not fit: the track is " + track.width()
                    + " px long, at least " + (2 * TRACK_CELLS * cell) + " px are needed");
        }
        return new Layout(width, height, cell, band, flip, beside, track);
    }

    /** Code cell (row, col) of the band. */
    public Rect codeCell(int row, int col) {
        if (row < 0 || row >= Timecode.ROWS || col < 0 || col >= Timecode.COLUMNS) {
            throw new IllegalArgumentException("code cell out of range: " + row + "," + col);
        }
        int left = band.left + col * cell;
        int top = band.top + row * cell;
        return new Rect(left, top, left + cell, top + cell);
    }

    /** The inset of the sample square on each side of a code cell. */
    public int sampleInset() {
        return cell / 4;
    }

    /** The square a decoder averages for code cell (row, col): the cell inset by {@link #sampleInset()}. */
    public Rect sampleRect(int row, int col) {
        Rect c = codeCell(row, col);
        int inset = sampleInset();
        return new Rect(c.left + inset, c.top + inset, c.right - inset, c.bottom - inset);
    }

    /** The edge k (0..GRID) of the tap grid along a side of {@code length} px: ceil(k * length / GRID). */
    static int gridEdge(int k, int length) {
        return (int) (((long) k * length + GRID - 1) / GRID);
    }

    /** Tap-grid cell j (0..15, row-major from the top left). */
    public Rect gridCell(int j) {
        if (j < 0 || j >= GRID * GRID) {
            throw new IllegalArgumentException("grid cell out of range: " + j);
        }
        int col = j % GRID;
        int row = j / GRID;
        return new Rect(gridEdge(col, width), gridEdge(row, height), gridEdge(col + 1, width), gridEdge(row + 1, height));
    }

    /** The tap-grid cell of view point (x, y), or -1 outside the view. */
    public int tapCell(float x, float y) {
        if (!(x >= 0 && y >= 0 && x < width && y < height)) {
            return -1;
        }
        int col = Math.min(GRID - 1, (int) Math.floor((double) x * GRID / width));
        int row = Math.min(GRID - 1, (int) Math.floor((double) y * GRID / height));
        return row * GRID + col;
    }

    public int movingSize() {
        return TRACK_CELLS * cell;
    }

    /** The left edge of the moving block in frame {@code frameSeq}: it advances {@code speed} px a frame and wraps. */
    public int movingLeft(long frameSeq, int speed) {
        int range = track.width() - movingSize();
        return track.left + (int) Math.floorMod(frameSeq * speed, (long) range);
    }

    public Json.Obj toJson(int speed) {
        return Json.obj()
                .put("width", width)
                .put("height", height)
                .put("cell", cell)
                .put("band", band.toJson())
                .put("code_columns", Timecode.COLUMNS)
                .put("code_rows", Timecode.ROWS)
                .put("sample_inset", sampleInset())
                .put("flip_block", flipBlock.toJson())
                .put("flip_beside_band", flipBesideBand)
                .put("track", track.toJson())
                .put("moving_size", movingSize())
                .put("speed", speed)
                .put("grid", GRID);
    }
}
