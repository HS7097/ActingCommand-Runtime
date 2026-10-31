package io.github.hs7097.capturebench.pattern.core;

import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Comparator;
import java.util.List;

/**
 * Runs every public static no-argument method named test* of the test classes below, on the
 * plain JVM (build.sh test). Exits 1 when a test fails or when no test ran.
 */
public final class TestMain {
    private static final Class<?>[] TESTS = {
        TimecodeTest.class,
        LayoutTest.class,
        JsonTest.class,
        RecordStreamTest.class,
        RecordBookTest.class,
        CommandTest.class,
        LineReaderTest.class,
        ReplayClockTest.class,
    };

    public static void main(String[] args) {
        int passed = 0;
        List<String> failed = new ArrayList<String>();
        for (Class<?> type : TESTS) {
            Method[] methods = type.getDeclaredMethods();
            Arrays.sort(methods, new Comparator<Method>() {
                @Override
                public int compare(Method a, Method b) {
                    return a.getName().compareTo(b.getName());
                }
            });
            for (Method m : methods) {
                int mod = m.getModifiers();
                if (!m.getName().startsWith("test") || !Modifier.isStatic(mod) || !Modifier.isPublic(mod)
                        || m.getParameterTypes().length != 0) {
                    continue;
                }
                String name = type.getSimpleName() + "." + m.getName();
                try {
                    m.invoke(null);
                    passed++;
                    System.out.println("ok      " + name);
                } catch (InvocationTargetException e) {
                    failed.add(name);
                    System.out.println("FAILED  " + name);
                    e.getCause().printStackTrace(System.out);
                } catch (IllegalAccessException e) {
                    failed.add(name);
                    System.out.println("FAILED  " + name + " (" + e + ")");
                }
            }
        }
        System.out.println(passed + " passed, " + failed.size() + " failed");
        if (!failed.isEmpty()) {
            System.out.println("failed: " + failed);
            System.exit(1);
        }
        if (passed == 0) {
            System.out.println("no test ran");
            System.exit(1);
        }
    }
}
