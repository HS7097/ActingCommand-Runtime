package io.github.hs7097.capturebench.pattern.core;

/** The identities the bench relies on (PROTOCOL.md, "Identity"). */
public final class ProtocolInfo {
    private ProtocolInfo() {}

    /** The wire protocol version that HELLO reports. */
    public static final int PROTOCOL_VERSION = 1;

    /** The record format that HELLO reports; every record of RECORDS has this shape. */
    public static final String RECORD_FORMAT = "capture-bench-pattern-records/1";

    /** The application id. */
    public static final String PACKAGE = "io.github.hs7097.capturebench.pattern";

    /** The activity, relative to the package, for {@code am start -n <package>/<activity>}. */
    public static final String ACTIVITY = ".app.PatternActivity";

    /** The abstract-namespace socket of the time service ({@code localabstract:<name>}). */
    public static final String SOCKET_NAME = "capture_bench_pattern";
}
