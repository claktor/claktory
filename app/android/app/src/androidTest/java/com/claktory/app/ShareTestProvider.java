package com.claktory.app;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.OpenableColumns;
import java.io.IOException;
import java.io.OutputStream;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * Files another app shares, for SharedContentTest, from the test APK (its
 * own package and process, which has no Kotlin, hence Java): /picture.png
 * and any other name answer BYTES; /never opens and then sends nothing, as
 * a provider that hangs.
 */
public class ShareTestProvider extends ContentProvider {
    public static final String AUTHORITY = "com.claktory.app.sharetest";
    public static final byte[] BYTES = bytes();
    private static final List<ParcelFileDescriptor> HANGING = Collections.synchronizedList(new ArrayList<>());

    public static Uri uri(String name) {
        return Uri.parse("content://" + AUTHORITY + "/" + name);
    }

    private static byte[] bytes() {
        byte[] bytes = new byte[70_000];
        for (int i = 0; i < bytes.length; i++) bytes[i] = (byte) (i % 251);
        return bytes;
    }

    @Override
    public boolean onCreate() {
        return true;
    }

    @Override
    public Cursor query(Uri uri, String[] projection, String selection, String[] args, String sort) {
        MatrixCursor cursor = new MatrixCursor(new String[] {OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE});
        boolean never = "never".equals(uri.getLastPathSegment());
        cursor.addRow(new Object[] {uri.getLastPathSegment(), never ? null : (long) BYTES.length});
        return cursor;
    }

    @Override
    public String getType(Uri uri) {
        String name = uri.getLastPathSegment();
        return name != null && name.endsWith(".png") ? "image/png" : "text/plain";
    }

    @Override
    public ParcelFileDescriptor openFile(Uri uri, String mode) throws java.io.FileNotFoundException {
        ParcelFileDescriptor[] pipe;
        try {
            pipe = ParcelFileDescriptor.createPipe();
        } catch (IOException error) {
            throw new java.io.FileNotFoundException(error.getMessage());
        }
        ParcelFileDescriptor write = pipe[1];
        if ("never".equals(uri.getLastPathSegment())) {
            // Held open and never written: a read waits for ever.
            HANGING.add(write);
        } else {
            new Thread(() -> {
                try (OutputStream out = new ParcelFileDescriptor.AutoCloseOutputStream(write)) {
                    out.write(BYTES);
                } catch (IOException ignored) {
                    // The reader went away.
                }
            }).start();
        }
        return pipe[0];
    }

    @Override
    public Uri insert(Uri uri, ContentValues values) {
        return null;
    }

    @Override
    public int delete(Uri uri, String selection, String[] args) {
        return 0;
    }

    @Override
    public int update(Uri uri, ContentValues values, String selection, String[] args) {
        return 0;
    }
}
