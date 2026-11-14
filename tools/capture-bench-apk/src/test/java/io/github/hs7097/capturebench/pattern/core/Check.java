package io.github.hs7097.capturebench.pattern.core;

import java.util.Arrays;

/** Assertions for the plain-JVM tests (no test framework is downloaded). */
final class Check {
    private Check() {}

    interface Body {
        void run() throws Exception;
    }

    static void equal(Object expected, Object actual, String what) {
        boolean same = expected == null ? actual == null : expected.equals(actual);
        if (!same) {
            throw new AssertionError(what + ": expected <" + expected + "> but was <" + actual + ">");
        }
    }

    static void equalLong(long expected, long actual, String what) {
        if (expected != actual) {
            throw new AssertionError(what + ": expected <" + expected + "> but was <" + actual + ">");
        }
    }

    static void isTrue(boolean condition, String what) {
        if (!condition) {
            throw new AssertionError(what);
        }
    }

    static void cells(boolean[][] expected, boolean[][] actual, String what) {
        if (!Arrays.deepEquals(expected, actual)) {
            throw new AssertionError(what + ": cell matrices differ");
        }
    }

    static <T extends Throwable> T fails(Class<T> type, Body body, String what) {
        try {
            body.run();
        } catch (Throwable t) {
            if (type.isInstance(t)) {
                return type.cast(t);
            }
            throw new AssertionError(what + ": expected " + type.getSimpleName() + " but got " + t, t);
        }
        throw new AssertionError(what + ": expected " + type.getSimpleName() + " but nothing was thrown");
    }

    /** The request must be refused with {@code code}, and close the connection exactly when {@code closes}. */
    static ProtocolError refused(String code, boolean closes, Body body, String what) {
        ProtocolError e = fails(ProtocolError.class, body, what);
        equal(code, e.code, what + " (code)");
        equal(Boolean.valueOf(closes), Boolean.valueOf(e.closeConnection), what + " (closes the connection)");
        return e;
    }
}
