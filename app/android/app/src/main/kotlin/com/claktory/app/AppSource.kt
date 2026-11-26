package com.claktory.app

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.net.Uri
import android.os.Build
import android.util.Log

/**
 * Where this app came from, for This phone's version line in Settings
 * (factory/device appSource and openAppSource; Pesci's
 * app/lib/mobile/app_source.dart reads it, as the iPhone answers from
 * StoreKit). "google_play" when Play installed it; "website" for
 * Claktory's own release build installed any other way, the APK from
 * claktory.com/android, whose update action is Doppio's
 * (android_app_updates.dart); null for Claktory Dev, a debuggable build
 * or anything it can't tell. It names a distribution channel, not proof
 * of where the bytes came from: the installer is whatever the system
 * recorded.
 */
object AppSource {
    /** Google Play's package: the installer it records, and its listing's app. */
    const val PLAY = "com.android.vending"

    /** Claktory's own package: Claktory Dev is `com.claktory.app.dev`. */
    const val RELEASE_PACKAGE = "com.claktory.app"

    private const val TAG = "AppSource"

    fun of(context: Context): String? = classify(
        packageName = context.packageName,
        debuggable = context.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0,
        installer = installer(context),
    )

    /** [of]'s rule, apart from the system so it can be tested. */
    fun classify(packageName: String, debuggable: Boolean, installer: String?): String? = when {
        packageName != RELEASE_PACKAGE || debuggable -> null
        installer == PLAY -> "google_play"
        else -> "website"
    }

    /** The package that installed this app, as the system recorded it; null if none or unknown. */
    internal fun installer(context: Context): String? = try {
        val packages = context.packageManager
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            packages.getInstallSourceInfo(context.packageName).installingPackageName
        } else {
            @Suppress("DEPRECATION")
            packages.getInstallerPackageName(context.packageName)
        }
    } catch (error: Exception) {
        Log.w(TAG, "installer unknown: ${error.javaClass.simpleName}")
        null
    }

    /**
     * Opens where [source] updates: Play's listing for "google_play" (the
     * Play app, else its web page). Anything else opens nothing and says
     * false: website's update is Doppio's action, and the rest have none.
     */
    fun open(context: Context, source: String?): Boolean {
        if (source != "google_play") return false
        for (intent in listingIntents(context.packageName)) {
            try {
                context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                return true
            } catch (_: ActivityNotFoundException) {
                // No Play app: its web page next.
            } catch (error: SecurityException) {
                Log.w(TAG, "listing refused: ${error.javaClass.simpleName}")
            }
        }
        return false
    }

    /** Play's listing for [packageName]: in the Play app, then on the web. */
    fun listingIntents(packageName: String): List<Intent> = listOf(
        Intent(Intent.ACTION_VIEW, Uri.parse("market://details?id=$packageName")).setPackage(PLAY),
        Intent(Intent.ACTION_VIEW, Uri.parse("https://play.google.com/store/apps/details?id=$packageName")),
    )
}
