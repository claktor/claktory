package com.claktory.app;

import android.app.Activity;
import android.content.ClipData;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.Bundle;
import android.provider.OpenableColumns;
import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.ArrayList;

/**
 * Another app a file share goes to (ShareFilesTest): the test APK is its
 * own package, run under its own user. It reads what it was sent, as a
 * mail or chat app would, tries to write it and to read an address it
 * wasn't granted, and reports each to the app. In Java: the test APK's
 * own process has no Kotlin library.
 */
public class ShareReceiver extends Activity {
    public static final String REPORT = "com.claktory.app.sharetest.READ";

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        ArrayList<String> report = new ArrayList<>();
        Intent intent = getIntent();
        report.add("action " + intent.getAction() + " type " + intent.getType());
        ClipData clip = intent.getClipData();
        for (int i = 0; clip != null && i < clip.getItemCount(); i++) {
            Uri uri = clip.getItemAt(i).getUri();
            report.add("read " + name(uri) + " " + read(uri));
            report.add("write " + write(uri));
        }
        String probe = intent.getStringExtra("probe");
        if (probe != null) report.add("probe " + read(Uri.parse(probe)));
        sendBroadcast(new Intent(REPORT).setPackage(intent.getStringExtra("reply")).putStringArrayListExtra("report", report));
        finish();
    }

    private String name(Uri uri) {
        try (Cursor cursor = getContentResolver().query(uri, new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            return cursor != null && cursor.moveToFirst() ? cursor.getString(0) : "?";
        } catch (Exception e) {
            return "denied";
        }
    }

    private String read(Uri uri) {
        try (InputStream in = getContentResolver().openInputStream(uri)) {
            ByteArrayOutputStream out = new ByteArrayOutputStream();
            byte[] buffer = new byte[4096];
            for (int n; (n = in.read(buffer)) > 0; ) out.write(buffer, 0, n);
            return out.toString("UTF-8");
        } catch (Exception e) {
            return "denied";
        }
    }

    private String write(Uri uri) {
        try (OutputStream out = getContentResolver().openOutputStream(uri, "w")) {
            out.write('x');
            return "allowed";
        } catch (Exception e) {
            return "denied";
        }
    }
}
