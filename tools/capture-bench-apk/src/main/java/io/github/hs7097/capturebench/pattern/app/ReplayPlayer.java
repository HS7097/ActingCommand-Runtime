package io.github.hs7097.capturebench.pattern.app;

import android.graphics.Bitmap;
import android.graphics.BitmapFactory;

import io.github.hs7097.capturebench.pattern.core.ProtocolError;

import java.io.File;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Iterator;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.RejectedExecutionException;

/**
 * Decodes the replay images ahead of the vsync clock on two background threads, a few images
 * ahead, so that at most a handful of decoded images are held at once. Indices count across
 * loops; index i shows image i % count. Decoded bitmaps of images already passed are reused for
 * later decodes. A decode failure is kept and reported by the view as a replay event.
 */
final class ReplayPlayer {
    private static final int AHEAD = 4;
    private static final int KEEP_BEHIND = 2;

    private final List<String> names;
    private final List<File> files;
    private final ExecutorService decoders = Executors.newFixedThreadPool(2);
    private final Map<Long, Bitmap> ready = new HashMap<Long, Bitmap>();
    private final Set<Long> inFlight = new HashSet<Long>();
    private final Map<Long, String> failures = new HashMap<Long, String>();
    private final ArrayDeque<Bitmap> pool = new ArrayDeque<Bitmap>();
    private int firstWidth;
    private int firstHeight;
    private boolean closed;

    ReplayPlayer(List<String> names, List<File> files) {
        if (names.isEmpty() || names.size() != files.size()) {
            throw new IllegalArgumentException("a replay needs one file per name and at least one");
        }
        this.names = new ArrayList<String>(names);
        this.files = new ArrayList<File>(files);
    }

    int count() {
        return names.size();
    }

    String name(long index) {
        return index < 0 ? null : names.get((int) (index % names.size()));
    }

    synchronized int firstWidth() {
        return firstWidth;
    }

    synchronized int firstHeight() {
        return firstHeight;
    }

    /** Decodes image 0 on the caller's thread, so a replay never starts on a missing first image. */
    void decodeFirst() throws ProtocolError {
        Bitmap b = decode(0);
        if (b == null) {
            throw new ProtocolError("image_decode_failed", "replay slot " + names.get(0) + " is not a decodable image");
        }
        synchronized (this) {
            ready.put(Long.valueOf(0), b);
            firstWidth = b.getWidth();
            firstHeight = b.getHeight();
        }
    }

    synchronized Bitmap get(long index) {
        return ready.get(Long.valueOf(index));
    }

    synchronized String failure(long index) {
        return failures.get(Long.valueOf(index));
    }

    /** Starts decodes for the indices from {@code from}, up to AHEAD of them, not beyond {@code last}. */
    synchronized void prefetch(long from, long last) {
        if (closed) {
            return;
        }
        for (long i = from; i < from + AHEAD && i <= last; i++) {
            final Long key = Long.valueOf(i);
            if (ready.containsKey(key) || inFlight.contains(key) || failures.containsKey(key)) {
                continue;
            }
            inFlight.add(key);
            try {
                decoders.execute(new Runnable() {
                    @Override
                    public void run() {
                        decodeInto(key);
                    }
                });
            } catch (RejectedExecutionException e) {
                inFlight.remove(key);
                failures.put(key, "decoder rejected the task: " + e);
            }
        }
    }

    /** Drops the images more than KEEP_BEHIND behind {@code shown}; their bitmaps go to the reuse pool. */
    synchronized void release(long shown) {
        Iterator<Map.Entry<Long, Bitmap>> it = ready.entrySet().iterator();
        while (it.hasNext()) {
            Map.Entry<Long, Bitmap> entry = it.next();
            if (entry.getKey().longValue() < shown - KEEP_BEHIND) {
                it.remove();
                Bitmap b = entry.getValue();
                if (b.isMutable() && pool.size() < AHEAD) {
                    pool.add(b);
                }
            }
        }
    }

    synchronized void close() {
        closed = true;
        decoders.shutdown();
        ready.clear();
        pool.clear();
    }

    private void decodeInto(Long key) {
        Bitmap b;
        String failure = null;
        try {
            b = decode(key.longValue());
            if (b == null) {
                failure = "slot " + name(key.longValue()) + " is not a decodable image";
            }
        } catch (RuntimeException e) {
            b = null;
            failure = "slot " + name(key.longValue()) + ": " + e;
        }
        synchronized (this) {
            inFlight.remove(key);
            if (closed) {
                return;
            }
            if (b != null) {
                ready.put(key, b);
            } else {
                failures.put(key, failure);
            }
        }
    }

    private Bitmap decode(long index) {
        String path = files.get((int) (index % files.size())).getPath();
        BitmapFactory.Options options = new BitmapFactory.Options();
        options.inScaled = false;
        options.inPreferredConfig = Bitmap.Config.ARGB_8888;
        options.inMutable = true;
        Bitmap reuse;
        synchronized (this) {
            reuse = pool.poll();
        }
        if (reuse != null) {
            options.inBitmap = reuse;
            try {
                return BitmapFactory.decodeFile(path, options);
            } catch (IllegalArgumentException e) {
                // The pooled bitmap does not fit this image: decode into a new one.
                options.inBitmap = null;
            }
        }
        return BitmapFactory.decodeFile(path, options);
    }
}
