package io.github.hs7097.capturebench.pattern.app;

import android.graphics.Bitmap;
import android.graphics.BitmapFactory;

import io.github.hs7097.capturebench.pattern.core.Json;
import io.github.hs7097.capturebench.pattern.core.LineReader;
import io.github.hs7097.capturebench.pattern.core.ProtocolError;
import io.github.hs7097.capturebench.pattern.core.RecordBook;
import io.github.hs7097.capturebench.pattern.core.Timecode;

import java.io.BufferedOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.security.DigestOutputStream;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * The images the bench pushes with PUT (static and replay modes), kept in the app's private files
 * directory, one file per slot. Nothing here deletes a file: a PUT of an existing slot replaces
 * it; a payload that is not an image stays in the slot's ".part" file until the next PUT of that
 * slot overwrites it.
 */
final class SlotStore {
    private final File dir;
    private final Map<String, Json.Obj> meta = new HashMap<String, Json.Obj>();

    SlotStore(File dir) {
        this.dir = dir;
    }

    synchronized Json.Obj put(String slot, long length, LineReader in, RecordBook book) throws IOException, ProtocolError {
        if (!dir.isDirectory() && !dir.mkdirs()) {
            // The payload cannot be stored and cannot be skipped reliably: close the connection.
            throw new ProtocolError("io_error", "cannot create the slot directory " + dir, true);
        }
        File part = new File(dir, "." + slot + ".part");
        MessageDigest sha = sha256();
        FileOutputStream file = new FileOutputStream(part);
        try {
            DigestOutputStream out = new DigestOutputStream(new BufferedOutputStream(file, 65536), sha);
            in.copy(out, length);
            out.flush();
        } finally {
            file.close();
        }
        BitmapFactory.Options bounds = new BitmapFactory.Options();
        bounds.inJustDecodeBounds = true;
        BitmapFactory.decodeFile(part.getPath(), bounds);
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) {
            throw new ProtocolError("image_decode_failed", "slot " + slot + ": the " + length
                    + "-byte payload is not an image this device decodes");
        }
        File target = new File(dir, slot);
        if (!part.renameTo(target)) {
            throw new ProtocolError("io_error", "cannot move the payload of slot " + slot + " into place", true);
        }
        Json.Obj m = Json.obj()
                .put("slot", slot)
                .put("bytes", length)
                .put("width", bounds.outWidth)
                .put("height", bounds.outHeight)
                .put("mime", bounds.outMimeType)
                .put("sha256", Timecode.hex(sha.digest()));
        meta.put(slot, m);
        Json.Obj event = Json.obj().put("type", "put");
        Json.Obj response = Bench.ok("PUT");
        for (String key : m.keys()) {
            event.put(key, m.get(key));
            response.put(key, m.get(key));
        }
        book.event(event.put("t_done_dev_ns", System.nanoTime()));
        return response;
    }

    /** Decodes a slot for the static mode, 1:1 (no density scaling), ARGB_8888. */
    synchronized Bitmap decode(String slot) throws ProtocolError {
        File f = existing(slot);
        BitmapFactory.Options options = new BitmapFactory.Options();
        options.inScaled = false;
        options.inPreferredConfig = Bitmap.Config.ARGB_8888;
        Bitmap b = BitmapFactory.decodeFile(f.getPath(), options);
        if (b == null) {
            throw new ProtocolError("image_decode_failed", "slot " + slot + " is not a decodable image");
        }
        return b;
    }

    /** The slots whose names start with {@code prefix}, in name order. */
    synchronized List<String> namesWithPrefix(String prefix) {
        List<String> out = new ArrayList<String>();
        for (String name : names()) {
            if (name.startsWith(prefix)) {
                out.add(name);
            }
        }
        return out;
    }

    synchronized List<File> files(List<String> names) throws ProtocolError {
        List<File> out = new ArrayList<File>();
        for (String name : names) {
            out.add(existing(name));
        }
        return out;
    }

    synchronized Json.Obj list() throws IOException, ProtocolError {
        List<Object> slots = new ArrayList<Object>();
        for (String name : names()) {
            Json.Obj m = meta.get(name);
            if (m == null) {
                m = describe(name);
                meta.put(name, m);
            }
            slots.add(m);
        }
        return Bench.ok("SLOTS").put("count", slots.size()).put("slots", slots);
    }

    private Json.Obj describe(String name) throws IOException, ProtocolError {
        File f = existing(name);
        BitmapFactory.Options bounds = new BitmapFactory.Options();
        bounds.inJustDecodeBounds = true;
        BitmapFactory.decodeFile(f.getPath(), bounds);
        MessageDigest sha = sha256();
        InputStream in = new FileInputStream(f);
        try {
            byte[] buffer = new byte[65536];
            int n;
            while ((n = in.read(buffer)) >= 0) {
                sha.update(buffer, 0, n);
            }
        } finally {
            in.close();
        }
        return Json.obj()
                .put("slot", name)
                .put("bytes", f.length())
                .put("width", bounds.outWidth > 0 ? Integer.valueOf(bounds.outWidth) : null)
                .put("height", bounds.outHeight > 0 ? Integer.valueOf(bounds.outHeight) : null)
                .put("mime", bounds.outMimeType)
                .put("sha256", Timecode.hex(sha.digest()));
    }

    private List<String> names() {
        String[] listed = dir.list();
        List<String> out = new ArrayList<String>();
        if (listed == null) {
            return out;
        }
        Arrays.sort(listed);
        for (String name : listed) {
            if (!name.startsWith(".")) {
                out.add(name);
            }
        }
        return out;
    }

    private File existing(String slot) throws ProtocolError {
        File f = new File(dir, slot);
        if (!f.isFile()) {
            throw new ProtocolError("no_such_slot", "no slot " + slot);
        }
        return f;
    }

    private static MessageDigest sha256() throws ProtocolError {
        try {
            return MessageDigest.getInstance("SHA-256");
        } catch (NoSuchAlgorithmException e) {
            throw new ProtocolError("internal_error", "SHA-256 is unavailable: " + e, true);
        }
    }
}
