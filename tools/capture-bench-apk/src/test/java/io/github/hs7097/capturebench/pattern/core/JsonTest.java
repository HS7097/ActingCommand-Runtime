package io.github.hs7097.capturebench.pattern.core;

/** The JSON every response and record is written in. */
final class JsonTest {
    public static void testKeysKeepInsertionOrder() {
        String json = Json.obj().put("b", 1).put("a", 2L).put("c", true).put("d", null).put("e", 1.5).toJson();
        Check.equal("{\"b\":1,\"a\":2,\"c\":true,\"d\":null,\"e\":1.5}", json, "object");
    }

    public static void testEscapes() {
        String text = "q\"b\\n" + '\n' + "t" + '\t' + "c" + (char) 1 + "e" + (char) 0xE9;
        String json = Json.obj().put("s", text).toJson();
        String expected = "{\"s\":\"q\\\"b\\\\n\\nt\\tc\\" + "u0001e" + (char) 0xE9 + "\"}";
        Check.equal(expected, json, "escaped string");
        Check.equal("\"\\r\\b\\f\"", Json.quote("\r\b\f"), "the other short escapes");
    }

    public static void testNestedListsAndRaw() {
        String json = Json.obj()
                .put("l", Json.list(1, "two", null, Json.obj().put("x", -3L)))
                .put("r", new Json.Raw("{\"k\":[1]}"))
                .toJson();
        Check.equal("{\"l\":[1,\"two\",null,{\"x\":-3}],\"r\":{\"k\":[1]}}", json, "nested");
    }

    public static void testLongExtremes() {
        String json = Json.obj().put("min", Long.MIN_VALUE).put("max", Long.MAX_VALUE).toJson();
        Check.equal("{\"min\":-9223372036854775808,\"max\":9223372036854775807}", json, "int64 extremes");
    }

    public static void testRefusesUnsupportedValues() {
        Check.fails(IllegalArgumentException.class, () -> Json.obj().put("n", Double.NaN), "NaN");
        Check.fails(IllegalArgumentException.class, () -> Json.obj().put("i", Double.POSITIVE_INFINITY), "infinity");
        Check.fails(IllegalArgumentException.class, () -> Json.obj().put("f", Float.valueOf(1.5f)), "a Float");
        Check.fails(IllegalArgumentException.class, () -> Json.obj().put("o", new Object()), "an Object");
        Check.fails(IllegalArgumentException.class, () -> Json.obj().put(null, 1), "a null key");
        Check.fails(IllegalArgumentException.class, () -> Json.list(new Object()), "an Object in a list");
    }
}
