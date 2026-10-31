package io.github.hs7097.capturebench.pattern.core;

import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.Set;
import java.util.regex.Pattern;

/**
 * One request line of the time service, parsed and validated (PROTOCOL.md, "Commands"). A line
 * is printable ASCII tokens separated by single spaces; the verb is upper case. Every value is
 * checked here, so a handler never sees an out-of-range argument.
 */
public final class Command {
    public static final int MAX_LINE_BYTES = 4096;
    public static final long MAX_PUT_BYTES = 32L * 1024 * 1024;
    public static final int DEFAULT_RECORDS_MAX = 1000;
    public static final int MAX_RECORDS_MAX = 5000;
    public static final int DEFAULT_REPLAY_FPS = 30;
    public static final int MAX_REPLAY_FPS = 120;
    public static final int MAX_REPLAY_LOOPS = 1000000;
    public static final int MAX_SPEED = 256;

    public static final Set<String> MODES = set("pattern", "flip", "step", "static", "replay");
    public static final Set<String> ORIENTATIONS =
            set("landscape", "portrait", "reverse_landscape", "reverse_portrait", "unspecified");
    public static final Set<String> STREAMS = set(RecordBook.VSYNC, RecordBook.EVENT);

    private static final Pattern TOKEN = Pattern.compile("[A-Za-z0-9_-]{1,32}");
    private static final Pattern NAME = Pattern.compile("[A-Za-z0-9_-][A-Za-z0-9_.-]{0,63}");
    private static final Pattern NUMBER = Pattern.compile("[0-9]{1,18}");

    public final String verb;
    /** PING. */
    public final String token;
    /** MODE. */
    public final String mode;
    /** MODE step and STEP: 0 or 1, null when absent. */
    public final Integer level;
    /** MODE static (null = the built-in still card) and PUT. */
    public final String slot;
    /** MODE replay. */
    public final String prefix;
    public final int fps;
    public final int loops;
    /** ORIENT. */
    public final String orientation;
    /** PUT. */
    public final long length;
    /** RECORDS. */
    public final String stream;
    public final long after;
    public final int max;
    /** SET. */
    public final String key;
    public final int value;

    private Command(Builder b) {
        verb = b.verb;
        token = b.token;
        mode = b.mode;
        level = b.level;
        slot = b.slot;
        prefix = b.prefix;
        fps = b.fps;
        loops = b.loops;
        orientation = b.orientation;
        length = b.length;
        stream = b.stream;
        after = b.after;
        max = b.max;
        key = b.key;
        value = b.value;
    }

    private static final class Builder {
        String verb;
        String token;
        String mode;
        Integer level;
        String slot;
        String prefix;
        int fps = DEFAULT_REPLAY_FPS;
        int loops;
        String orientation;
        long length;
        String stream;
        long after;
        int max = DEFAULT_RECORDS_MAX;
        String key;
        int value;

        Builder(String verb) {
            this.verb = verb;
        }

        Command build() {
            return new Command(this);
        }
    }

    /** The verb of a line as far as it can be read, for the "cmd" of an error response; null if none. */
    public static String verbOf(String line) {
        if (line == null) {
            return null;
        }
        int space = line.indexOf(' ');
        String verb = space < 0 ? line : line.substring(0, space);
        return TOKEN.matcher(verb).matches() ? verb : null;
    }

    public static Command parse(String line) throws ProtocolError {
        if (line == null || line.isEmpty()) {
            throw new ProtocolError("bad_arguments", "empty request line");
        }
        for (int i = 0; i < line.length(); i++) {
            char c = line.charAt(i);
            if (c < 0x20 || c > 0x7E) {
                throw new ProtocolError("bad_arguments", "request lines are printable ASCII; byte " + (int) c + " at " + i);
            }
        }
        String[] tokens = line.split(" ", -1);
        for (String t : tokens) {
            if (t.isEmpty()) {
                throw new ProtocolError("bad_arguments", "tokens are separated by exactly one space");
            }
        }
        String verb = tokens[0];
        String[] args = Arrays.copyOfRange(tokens, 1, tokens.length);
        Builder b = new Builder(verb);
        if ("HELLO".equals(verb) || "INFO".equals(verb) || "SLOTS".equals(verb) || "BYE".equals(verb)) {
            arity(verb, args, 0, 0);
        } else if ("PING".equals(verb)) {
            arity(verb, args, 1, 1);
            if (!TOKEN.matcher(args[0]).matches()) {
                throw new ProtocolError("bad_arguments", "PING token must match [A-Za-z0-9_-]{1,32}");
            }
            b.token = args[0];
        } else if ("MODE".equals(verb)) {
            parseMode(b, args);
        } else if ("STEP".equals(verb)) {
            arity(verb, args, 0, 1);
            Map<String, String> options = options(verb, args, 0, set("level"));
            b.level = levelOption(options);
        } else if ("ORIENT".equals(verb)) {
            arity(verb, args, 1, 1);
            if (!ORIENTATIONS.contains(args[0])) {
                throw new ProtocolError("bad_arguments", "ORIENT takes one of " + ORIENTATIONS + ": " + args[0]);
            }
            b.orientation = args[0];
        } else if ("PUT".equals(verb)) {
            parsePut(b, args);
        } else if ("RECORDS".equals(verb)) {
            arity(verb, args, 2, 3);
            if (!STREAMS.contains(args[0])) {
                throw new ProtocolError("bad_arguments", "RECORDS takes the stream vsync or event: " + args[0]);
            }
            b.stream = args[0];
            b.after = number(verb, "after", args[1]);
            if (args.length == 3) {
                long max = number(verb, "max", args[2]);
                if (max < 1 || max > MAX_RECORDS_MAX) {
                    throw new ProtocolError("bad_arguments", "RECORDS max must be 1.." + MAX_RECORDS_MAX + ": " + max);
                }
                b.max = (int) max;
            }
        } else if ("SET".equals(verb)) {
            arity(verb, args, 2, 2);
            long v = number(verb, args[0], args[1]);
            if ("cell".equals(args[0])) {
                if (v < Layout.MIN_CELL || v > Layout.MAX_CELL) {
                    throw new ProtocolError("bad_arguments", "SET cell must be " + Layout.MIN_CELL + ".." + Layout.MAX_CELL + ": " + v);
                }
            } else if ("speed".equals(args[0])) {
                if (v > MAX_SPEED) {
                    throw new ProtocolError("bad_arguments", "SET speed must be 0.." + MAX_SPEED + ": " + v);
                }
            } else {
                throw new ProtocolError("bad_arguments", "SET takes the key cell or speed: " + args[0]);
            }
            b.key = args[0];
            b.value = (int) v;
        } else {
            throw new ProtocolError("unknown_command", "unknown verb: " + (verbOf(line) == null ? "(unreadable)" : verb));
        }
        return b.build();
    }

    private static void parseMode(Builder b, String[] args) throws ProtocolError {
        if (args.length < 1) {
            throw new ProtocolError("bad_arguments", "MODE needs a mode, one of " + MODES);
        }
        String mode = args[0];
        if (!MODES.contains(mode)) {
            throw new ProtocolError("bad_arguments", "MODE takes one of " + MODES + ": " + mode);
        }
        b.mode = mode;
        Map<String, String> options;
        if ("step".equals(mode)) {
            options = options("MODE", args, 1, set("level"));
            b.level = levelOption(options);
        } else if ("static".equals(mode)) {
            options = options("MODE", args, 1, set("slot"));
            String slot = options.get("slot");
            if (slot != null) {
                b.slot = name("MODE", "slot", slot);
            }
        } else if ("replay".equals(mode)) {
            options = options("MODE", args, 1, set("prefix", "fps", "loops"));
            String prefix = options.get("prefix");
            if (prefix == null) {
                throw new ProtocolError("bad_arguments", "MODE replay needs prefix=<slot prefix>");
            }
            b.prefix = name("MODE", "prefix", prefix);
            if (options.containsKey("fps")) {
                long fps = number("MODE", "fps", options.get("fps"));
                if (fps < 1 || fps > MAX_REPLAY_FPS) {
                    throw new ProtocolError("bad_arguments", "MODE replay fps must be 1.." + MAX_REPLAY_FPS + ": " + fps);
                }
                b.fps = (int) fps;
            }
            if (options.containsKey("loops")) {
                long loops = number("MODE", "loops", options.get("loops"));
                if (loops > MAX_REPLAY_LOOPS) {
                    throw new ProtocolError("bad_arguments", "MODE replay loops must be 0.." + MAX_REPLAY_LOOPS + ": " + loops);
                }
                b.loops = (int) loops;
            }
        } else {
            options("MODE", args, 1, Collections.<String>emptySet());
        }
    }

    private static void parsePut(Builder b, String[] args) throws ProtocolError {
        // A refused PUT header closes the connection: its payload would otherwise be read as requests.
        if (args.length != 2) {
            throw new ProtocolError("bad_arguments", "PUT takes <slot> <length>", true);
        }
        if (!NAME.matcher(args[0]).matches()) {
            throw new ProtocolError("bad_arguments", "PUT slot must match " + NAME.pattern() + ": " + args[0], true);
        }
        if (!NUMBER.matcher(args[1]).matches()) {
            throw new ProtocolError("bad_arguments", "PUT length must be a decimal number: " + args[1], true);
        }
        long length = Long.parseLong(args[1]);
        if (length < 1) {
            throw new ProtocolError("bad_arguments", "PUT length must be at least 1", true);
        }
        if (length > MAX_PUT_BYTES) {
            throw new ProtocolError("payload_too_large", "PUT length " + length + " exceeds " + MAX_PUT_BYTES, true);
        }
        b.slot = args[0];
        b.length = length;
    }

    private static void arity(String verb, String[] args, int min, int max) throws ProtocolError {
        if (args.length < min || args.length > max) {
            throw new ProtocolError("bad_arguments", verb + " takes " + (min == max ? String.valueOf(min) : min + ".." + max)
                    + " arguments, got " + args.length);
        }
    }

    private static Map<String, String> options(String verb, String[] args, int from, Set<String> allowed)
            throws ProtocolError {
        Map<String, String> options = new LinkedHashMap<String, String>();
        for (int i = from; i < args.length; i++) {
            String arg = args[i];
            int eq = arg.indexOf('=');
            if (eq <= 0 || eq == arg.length() - 1) {
                throw new ProtocolError("bad_arguments", verb + " options are key=value: " + arg);
            }
            String key = arg.substring(0, eq);
            if (!allowed.contains(key)) {
                throw new ProtocolError("bad_arguments", verb + " does not take the option " + key
                        + (allowed.isEmpty() ? "" : "; it takes " + allowed));
            }
            if (options.put(key, arg.substring(eq + 1)) != null) {
                throw new ProtocolError("bad_arguments", verb + " option " + key + " is given twice");
            }
        }
        return options;
    }

    private static Integer levelOption(Map<String, String> options) throws ProtocolError {
        String level = options.get("level");
        if (level == null) {
            return null;
        }
        if ("0".equals(level)) {
            return Integer.valueOf(0);
        }
        if ("1".equals(level)) {
            return Integer.valueOf(1);
        }
        throw new ProtocolError("bad_arguments", "level must be 0 or 1: " + level);
    }

    private static String name(String verb, String what, String value) throws ProtocolError {
        if (!NAME.matcher(value).matches()) {
            throw new ProtocolError("bad_arguments", verb + " " + what + " must match " + NAME.pattern() + ": " + value);
        }
        return value;
    }

    private static long number(String verb, String what, String value) throws ProtocolError {
        if (!NUMBER.matcher(value).matches()) {
            throw new ProtocolError("bad_arguments", verb + " " + what + " must be a decimal number: " + value);
        }
        return Long.parseLong(value);
    }

    private static Set<String> set(String... values) {
        return Collections.unmodifiableSet(new HashSet<String>(Arrays.asList(values)));
    }
}
