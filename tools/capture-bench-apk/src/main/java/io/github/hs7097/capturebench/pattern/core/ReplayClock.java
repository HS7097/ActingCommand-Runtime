package io.github.hs7097.capturebench.pattern.core;

/**
 * Which replay image is due at a vsync (PROTOCOL.md, "Replay mode"). Image index i (counting
 * across loops) is due from {@code start + i / fps} seconds, where start is the {@code T_vsync}
 * of the frame that first showed image 0. The schedule follows the vsync clock, not the
 * decoder: an image that is not ready when it is due is reported late, and a later image that is
 * ready replaces it.
 */
public final class ReplayClock {
    private static final long SECOND_NS = 1000000000L;

    public final long startNs;
    public final int fps;
    public final int count;
    /** 0 = loop forever. */
    public final int loops;

    public ReplayClock(long startNs, int fps, int count, int loops) {
        if (fps < 1 || fps > Command.MAX_REPLAY_FPS) {
            throw new IllegalArgumentException("fps must be 1.." + Command.MAX_REPLAY_FPS + ": " + fps);
        }
        if (count < 1) {
            throw new IllegalArgumentException("a replay needs at least one image: " + count);
        }
        if (loops < 0) {
            throw new IllegalArgumentException("loops must be >= 0: " + loops);
        }
        this.startNs = startNs;
        this.fps = fps;
        this.count = count;
        this.loops = loops;
    }

    /** floor((frameTimeNs - start) * fps / 1 s), and 0 at or before the start. */
    public long dueIndex(long frameTimeNs) {
        if (frameTimeNs <= startNs) {
            return 0;
        }
        long elapsed = frameTimeNs - startNs;
        return (elapsed / SECOND_NS) * fps + (elapsed % SECOND_NS) * fps / SECOND_NS;
    }

    /** True once {@code index} is past the last image of a bounded replay. */
    public boolean finished(long index) {
        return loops > 0 && index >= (long) count * loops;
    }

    /** The last index a bounded replay shows; Long.MAX_VALUE when it loops forever. */
    public long lastIndex() {
        return loops > 0 ? (long) count * loops - 1 : Long.MAX_VALUE;
    }

    /** The image (0..count-1) of index {@code index}. */
    public int imageOf(long index) {
        return (int) (index % count);
    }

    /** The loop (0-based) of index {@code index}. */
    public long loopOf(long index) {
        return index / count;
    }
}
