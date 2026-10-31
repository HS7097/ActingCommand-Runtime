package io.github.hs7097.capturebench.pattern.core;

/** Request parsing and validation (PROTOCOL.md, "Commands"). */
final class CommandTest {
    private static void refused(String code, boolean closes, final String line) {
        Check.refused(code, closes, () -> Command.parse(line), "\"" + line + "\"");
    }

    public static void testVerbsWithoutArguments() throws Exception {
        for (String verb : new String[] {"HELLO", "INFO", "SLOTS", "BYE"}) {
            Check.equal(verb, Command.parse(verb).verb, verb);
            refused("bad_arguments", false, verb + " x");
        }
    }

    public static void testPing() throws Exception {
        Check.equal("abc-1_Z", Command.parse("PING abc-1_Z").token, "token");
        refused("bad_arguments", false, "PING");
        refused("bad_arguments", false, "PING a b");
        refused("bad_arguments", false, "PING a.b");
        refused("bad_arguments", false, "PING 123456789012345678901234567890123");
    }

    public static void testModes() throws Exception {
        Check.equal("pattern", Command.parse("MODE pattern").mode, "pattern");
        Check.equal("flip", Command.parse("MODE flip").mode, "flip");
        Check.equal(null, Command.parse("MODE step").level, "step without a level");
        Check.equal(Integer.valueOf(1), Command.parse("MODE step level=1").level, "step level 1");
        Check.equal(null, Command.parse("MODE static").slot, "the built-in still card");
        Check.equal("card_01.png", Command.parse("MODE static slot=card_01.png").slot, "static slot");
        Command replay = Command.parse("MODE replay prefix=r_");
        Check.equal("r_", replay.prefix, "replay prefix");
        Check.equalLong(30, replay.fps, "default replay fps");
        Check.equalLong(0, replay.loops, "default loops: forever");
        Command bounded = Command.parse("MODE replay loops=2 fps=60 prefix=r_");
        Check.equalLong(60, bounded.fps, "replay fps");
        Check.equalLong(2, bounded.loops, "replay loops");
    }

    public static void testModeRefusals() {
        String[] lines = {
            "MODE",
            "MODE dance",
            "MODE pattern level=1",
            "MODE flip slot=a",
            "MODE step level=2",
            "MODE step level",
            "MODE step =1",
            "MODE step level=",
            "MODE replay",
            "MODE replay prefix=r_ fps=0",
            "MODE replay prefix=r_ fps=121",
            "MODE replay prefix=r_ loops=1000001",
            "MODE replay prefix=a prefix=b",
            "MODE static slot=../x",
            "MODE static slot=.hidden",
        };
        for (String line : lines) {
            refused("bad_arguments", false, line);
        }
    }

    public static void testStepAndOrient() throws Exception {
        Check.equal(null, Command.parse("STEP").level, "toggle");
        Check.equal(Integer.valueOf(0), Command.parse("STEP level=0").level, "level 0");
        refused("bad_arguments", false, "STEP 1");
        refused("bad_arguments", false, "STEP level=0 level=1");
        for (String o : new String[] {"landscape", "portrait", "reverse_landscape", "reverse_portrait", "unspecified"}) {
            Check.equal(o, Command.parse("ORIENT " + o).orientation, o);
        }
        refused("bad_arguments", false, "ORIENT sideways");
        refused("bad_arguments", false, "ORIENT");
    }

    public static void testPutHeaderAndItsRefusalsCloseTheConnection() throws Exception {
        Command c = Command.parse("PUT img_01.png 1024");
        Check.equal("img_01.png", c.slot, "slot");
        Check.equalLong(1024, c.length, "length");
        Check.equalLong(Command.MAX_PUT_BYTES, Command.parse("PUT a 33554432").length, "the largest payload");
        refused("bad_arguments", true, "PUT ../x 10");
        refused("bad_arguments", true, "PUT .hidden 10");
        refused("bad_arguments", true, "PUT a 0");
        refused("bad_arguments", true, "PUT a -1");
        refused("bad_arguments", true, "PUT a 12x");
        refused("bad_arguments", true, "PUT a");
        refused("bad_arguments", true, "PUT a 1 2");
        refused("payload_too_large", true, "PUT a 33554433");
    }

    public static void testRecords() throws Exception {
        Command c = Command.parse("RECORDS event 0");
        Check.equal("event", c.stream, "stream");
        Check.equalLong(0, c.after, "after");
        Check.equalLong(Command.DEFAULT_RECORDS_MAX, c.max, "default max");
        Check.equalLong(5000, Command.parse("RECORDS vsync 10 5000").max, "largest max");
        refused("bad_arguments", false, "RECORDS vsync 10 5001");
        refused("bad_arguments", false, "RECORDS vsync 10 0");
        refused("bad_arguments", false, "RECORDS other 0");
        refused("bad_arguments", false, "RECORDS frame 0");
        refused("bad_arguments", false, "RECORDS event -1");
        refused("bad_arguments", false, "RECORDS event");
        refused("bad_arguments", false, "RECORDS event 1 2 3");
    }

    public static void testSet() throws Exception {
        Command c = Command.parse("SET cell 32");
        Check.equal("cell", c.key, "key");
        Check.equalLong(32, c.value, "value");
        Check.equalLong(0, Command.parse("SET speed 0").value, "speed 0");
        refused("bad_arguments", false, "SET cell 23");
        refused("bad_arguments", false, "SET cell 257");
        refused("bad_arguments", false, "SET speed 257");
        refused("bad_arguments", false, "SET color 1");
        refused("bad_arguments", false, "SET cell");
        refused("bad_arguments", false, "SET cell x");
    }

    public static void testLineShape() {
        refused("bad_arguments", false, "");
        refused("bad_arguments", false, "HELLO ");
        refused("bad_arguments", false, " HELLO");
        refused("bad_arguments", false, "PING  a");
        refused("bad_arguments", false, "HELLO" + '\t');
        refused("bad_arguments", false, "HELLO" + '\r');
        refused("bad_arguments", false, "H" + (char) 0xC9 + "LLO");
        refused("unknown_command", false, "hello");
        refused("unknown_command", false, "FOO bar");
        Check.equal("PING", Command.verbOf("PING x"), "verb of a line");
        Check.equal(null, Command.verbOf(""), "no verb");
        Check.equal(null, Command.verbOf("!! x"), "unreadable verb");
    }

    public static void testErrorResponseShape() {
        ProtocolError e = new ProtocolError("bad_arguments", "x");
        Check.equal("{\"ok\":false,\"cmd\":\"PING\",\"error\":\"bad_arguments\",\"detail\":\"x\"}", e.toJson("PING").toJson(),
                "error response");
        Check.equal("{\"ok\":false,\"cmd\":null,\"error\":\"bad_arguments\",\"detail\":\"x\"}", e.toJson(null).toJson(),
                "error response without a verb");
    }
}
