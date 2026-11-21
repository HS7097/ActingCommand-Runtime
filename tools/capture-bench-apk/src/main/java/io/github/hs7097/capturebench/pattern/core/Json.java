package io.github.hs7097.capturebench.pattern.core;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;

/**
 * The JSON of PROTOCOL.md: objects keep their insertion order, so every response and record has
 * one fixed key order. Values are null, Boolean, Integer, Long, Double, String, {@link Obj},
 * {@link Raw} or a List of those; anything else is refused when it is put.
 */
public final class Json {
    private Json() {}

    /** A JSON text inserted verbatim, such as a record that {@link RecordStream} already serialized. */
    public static final class Raw {
        final String text;

        public Raw(String text) {
            if (text == null) {
                throw new IllegalArgumentException("raw JSON text is null");
            }
            this.text = text;
        }
    }

    /** An object whose keys keep their insertion order. */
    public static final class Obj {
        private final LinkedHashMap<String, Object> fields = new LinkedHashMap<String, Object>();

        public Obj put(String key, Object value) {
            if (key == null) {
                throw new IllegalArgumentException("JSON key is null");
            }
            check(value);
            fields.put(key, value);
            return this;
        }

        public Object get(String key) {
            return fields.get(key);
        }

        public boolean has(String key) {
            return fields.containsKey(key);
        }

        public List<String> keys() {
            return new ArrayList<String>(fields.keySet());
        }

        public String toJson() {
            StringBuilder out = new StringBuilder();
            write(out, this);
            return out.toString();
        }

        @Override
        public String toString() {
            return toJson();
        }
    }

    public static Obj obj() {
        return new Obj();
    }

    public static List<Object> list(Object... values) {
        List<Object> out = new ArrayList<Object>(Arrays.asList(values));
        for (Object value : out) {
            check(value);
        }
        return Collections.unmodifiableList(out);
    }

    static void check(Object value) {
        if (value == null
                || value instanceof String
                || value instanceof Boolean
                || value instanceof Integer
                || value instanceof Long
                || value instanceof Obj
                || value instanceof Raw) {
            return;
        }
        if (value instanceof Double) {
            double d = ((Double) value).doubleValue();
            if (Double.isNaN(d) || Double.isInfinite(d)) {
                throw new IllegalArgumentException("JSON has no NaN or infinity: " + d);
            }
            return;
        }
        if (value instanceof List) {
            for (Object element : (List<?>) value) {
                check(element);
            }
            return;
        }
        throw new IllegalArgumentException("unsupported JSON value type " + value.getClass().getName());
    }

    static void write(StringBuilder out, Object value) {
        if (value == null) {
            out.append("null");
        } else if (value instanceof String) {
            quote(out, (String) value);
        } else if (value instanceof Boolean) {
            out.append(((Boolean) value).booleanValue() ? "true" : "false");
        } else if (value instanceof Integer || value instanceof Long) {
            out.append(value.toString());
        } else if (value instanceof Double) {
            out.append(Double.toString(((Double) value).doubleValue()));
        } else if (value instanceof Raw) {
            out.append(((Raw) value).text);
        } else if (value instanceof Obj) {
            out.append('{');
            boolean first = true;
            for (Map.Entry<String, Object> entry : ((Obj) value).fields.entrySet()) {
                if (!first) {
                    out.append(',');
                }
                first = false;
                quote(out, entry.getKey());
                out.append(':');
                write(out, entry.getValue());
            }
            out.append('}');
        } else if (value instanceof List) {
            out.append('[');
            boolean first = true;
            for (Object element : (List<?>) value) {
                if (!first) {
                    out.append(',');
                }
                first = false;
                write(out, element);
            }
            out.append(']');
        } else {
            throw new IllegalArgumentException("unsupported JSON value type " + value.getClass().getName());
        }
    }

    /** Appends {@code text} as a JSON string. */
    public static void quote(StringBuilder out, String text) {
        out.append('"');
        for (int i = 0; i < text.length(); i++) {
            char c = text.charAt(i);
            switch (c) {
                case '"':
                    out.append("\\\"");
                    break;
                case '\\':
                    out.append("\\\\");
                    break;
                case '\n':
                    out.append("\\n");
                    break;
                case '\r':
                    out.append("\\r");
                    break;
                case '\t':
                    out.append("\\t");
                    break;
                case '\b':
                    out.append("\\b");
                    break;
                case '\f':
                    out.append("\\f");
                    break;
                default:
                    if (c < 0x20) {
                        out.append(String.format(Locale.ROOT, "\\u%04x", (int) c));
                    } else {
                        out.append(c);
                    }
            }
        }
        out.append('"');
    }

    public static String quote(String text) {
        StringBuilder out = new StringBuilder();
        quote(out, text);
        return out.toString();
    }
}
