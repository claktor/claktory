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
 * from Photos or Files. With "mixed", it shares two but grants only the
 * first, as a careless or hostile app might. It shows nothing and closes at
 * once.
 */
public final class ShareSender extends Activity {
    public static final String PRIVATE_AUTHORITY = "com.claktory.app.sharetest.private";

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        String target = getIntent().getStringExtra("target");
        Uri uri = Uri.parse("content://" + PRIVATE_AUTHORITY + "/granted.png");
        Intent share;
        if (getIntent().getBooleanExtra("mixed", false)) {
            Uri ungranted = Uri.parse("content://" + PRIVATE_AUTHORITY + "/ungranted.png");
            java.util.ArrayList<Uri> both = new java.util.ArrayList<>();
            both.add(uri);
            both.add(ungranted);
            share = new Intent(Intent.ACTION_SEND_MULTIPLE)
                    .setType("image/png")
                    .putParcelableArrayListExtra(Intent.EXTRA_STREAM, both)
                    .putExtra(Intent.EXTRA_TEXT, "mixed");
            // The grant covers only what's in the clip: the first.
            share.setClipData(android.content.ClipData.newRawUri("", uri));
        } else {
            share = new Intent(Intent.ACTION_SEND)
                    .setType("image/png")
                    .putExtra(Intent.EXTRA_STREAM, uri)
                    .putExtra(Intent.EXTRA_TEXT, "granted");
        }
        share.setComponent(ComponentName.unflattenFromString(target))
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_ACTIVITY_NEW_TASK);
        startActivity(share);
        finish();
    }

    /** The same files as ShareTestProvider, but only through a grant. */
    public static final class Private extends ShareTestProvider {}
}
