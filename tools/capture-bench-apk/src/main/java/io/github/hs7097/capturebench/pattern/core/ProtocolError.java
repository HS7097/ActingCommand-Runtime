package io.github.hs7097.capturebench.pattern.core;

/**
 * A refused request: the {@code error} code and {@code detail} of an error response
 * (PROTOCOL.md, "Errors"). When {@link #closeConnection} is set the server writes the error and
 * closes the connection, because it can no longer tell where the next request starts.
 */
public final class ProtocolError extends Exception {
    private static final long serialVersionUID = 1L;

    public final String code;
    public final boolean closeConnection;

    public ProtocolError(String code, String detail) {
        this(code, detail, false);
    }

    public ProtocolError(String code, String detail, boolean closeConnection) {
        super(detail);
        if (code == null || code.isEmpty()) {
            throw new IllegalArgumentException("an error code is required");
        }
        this.code = code;
        this.closeConnection = closeConnection;
    }

    /** The error response line for a request whose verb was {@code verb} (null when unknown). */
    public Json.Obj toJson(String verb) {
        return Json.obj()
                .put("ok", Boolean.FALSE)
                .put("cmd", verb)
                .put("error", code)
                .put("detail", getMessage());
    }
}
