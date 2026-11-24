package com.claktory.app;

import android.app.Activity;
import android.content.ComponentName;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;

/**
 * Another app sharing to Claktory, for SharedContentTest: started from the
 * shell, it shares a file of the test APK's private provider (not
 * exported), so the app can read it only through this share's grant, as
 * from Photos or Files. It shows nothing and closes at once.
 */
public final class ShareSender extends Activity {
    public static final String PRIVATE_AUTHORITY = "com.claktory.app.sharetest.private";

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        String target = getIntent().getStringExtra("target");
        Uri uri = Uri.parse("content://" + PRIVATE_AUTHORITY + "/granted.png");
        Intent share = new Intent(Intent.ACTION_SEND)
                .setType("image/png")
                .putExtra(Intent.EXTRA_STREAM, uri)
                .putExtra(Intent.EXTRA_TEXT, "granted")
                .setComponent(ComponentName.unflattenFromString(target))
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_ACTIVITY_NEW_TASK);
        startActivity(share);
        finish();
    }

    /** The same files as ShareTestProvider, but only through a grant. */
    public static final class Private extends ShareTestProvider {}
}
