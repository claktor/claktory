package com.claktory.app;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

/**
 * Does nothing. tools/backup_exclusion_check.sh uses the test APK as its
 * positive control (it allows backup), but a freshly installed package is
 * "stopped" and the backup service skips stopped packages; running a test
 * clears that for the app under test, not for the test APK. One explicit
 * broadcast here starts the test APK's own process, which clears it.
 * Guarded by DUMP, so only the shell and the system can send it.
 *
 * Java, not Kotlin: in its own process the test APK has no Kotlin
 * standard library (it borrows the app's only when instrumenting it).
 */
public final class BackupControlWaker extends BroadcastReceiver {
    @Override
    public void onReceive(Context context, Intent intent) {}
}
