package io.github.hs7097.capturebench.pattern.core;

/** Which replay image is due at a vsync (PROTOCOL.md, "Replay mode"). */
final class ReplayClockTest {
    public static void testDueIndexAt30Fps() {
        ReplayClock c = new ReplayClock(1000L, 30, 3, 0);
        Check.equalLong(0, c.dueIndex(500L), "before the start");
        Check.equalLong(0, c.dueIndex(1000L), "at the start");
        Check.equalLong(0, c.dueIndex(1000L + 33333333L), "just before 1/30 s");
        Check.equalLong(1, c.dueIndex(1000L + 33333334L), "at 1/30 s");
        Check.equalLong(30, c.dueIndex(1000L + 1000000000L), "one second");
        Check.equalLong(108000, c.dueIndex(1000L + 3600L * 1000000000L), "one hour");
    }

    public static void testImagesAndLoops() {
        ReplayClock c = new ReplayClock(0L, 30, 3, 2);
        Check.equalLong(0, c.imageOf(0), "image of index 0");
        Check.equalLong(1, c.imageOf(4), "image of index 4");
        Check.equalLong(1, c.loopOf(4), "loop of index 4");
        Check.equalLong(5, c.lastIndex(), "last index of two loops of three");
        Check.isTrue(!c.finished(5), "index 5 is shown");
        Check.isTrue(c.finished(6), "index 6 is past the end");
        ReplayClock forever = new ReplayClock(0L, 30, 3, 0);
        Check.isTrue(!forever.finished(1000000L), "loops 0 never ends");
        Check.equalLong(Long.MAX_VALUE, forever.lastIndex(), "no last index");
    }

    public static void testRefusesBadArguments() {
        Check.fails(IllegalArgumentException.class, () -> new ReplayClock(0L, 0, 3, 0), "fps 0");
        Check.fails(IllegalArgumentException.class, () -> new ReplayClock(0L, 121, 3, 0), "fps 121");
        Check.fails(IllegalArgumentException.class, () -> new ReplayClock(0L, 30, 0, 0), "no images");
        Check.fails(IllegalArgumentException.class, () -> new ReplayClock(0L, 30, 3, -1), "loops -1");
    }
}
