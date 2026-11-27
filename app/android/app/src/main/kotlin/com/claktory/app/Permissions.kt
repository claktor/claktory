package com.claktory.app

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.Settings

/**
 * Android's runtime permissions as the app's permission sheet reads them
 * (ux/permissions.md, area 5a), the same for the microphone and the
 * camera:
 *
 * - "granted";
 * - "undetermined" while asking can still show Android's question: never
 *   asked, refused once (Android asks a second time, and says so with
 *   shouldShowRequestPermissionRationale), or its question set aside with
 *   Back or a tap beside it (Android asks again);
 * - "denied" once the person said no in this run of the app and Android
 *   won't ask again, so only Settings changes it.
 *
 * Beside it, [refusedOnce]: undetermined after a first no, which the
 * Settings row tells apart (factory/recorder refusedOnce, factory/device
 * cameraRefusedOnce; a missing method reads as false).
 *
 * Before asking, Android doesn't tell "never asked" from "won't ask
 * again", and the app keeps no record of asking on the device (the
 * device-local rule): after a permanent no in an earlier run it reads
 * undetermined, the sheet's Continue gets Android's no at once, and
 * Declined follows (one wasted tap, accepted in the spec's rule 3).
 */
object PermissionState {
    /**
     * Android's no came sooner than this after asking: no question showed
     * (it won't ask again). Its instant no takes some 60 ms on an emulator;
     * a person reading the question and setting it aside takes seconds.
     * Read wrong either way, slow phones err to Declined, whose Open
     * Settings still works, never to Before again, whose Continue would
     * only get the same instant no.
     */
    const val UNSEEN_MS = 1000L

    fun of(granted: Boolean, refusedForGood: Boolean, androidWouldExplain: Boolean): String = when {
        granted -> "granted"
        refusedForGood && !androidWouldExplain -> "denied"
        else -> "undetermined"
    }

    /**
     * Refused once, and Android will still ask (it explains): the Settings
     * row reads Off with Turn on… (Lucy's ruling, permissions.md §2.1),
     * while [of] still says undetermined, so the sheet's Continue asks.
     */
    fun refusedOnce(granted: Boolean, androidWouldExplain: Boolean): Boolean = !granted && androidWouldExplain

    /**
     * Whether a no means Android won't ask again: a second no (it would
     * explain before asking, and no longer does), or a no that came with
     * no question shown. A first no leaves it explaining; a question set
     * aside leaves it as it was, after the person saw it.
     */
    fun refusedForGood(explainedBefore: Boolean, explainsAfter: Boolean, elapsedMs: Long): Boolean =
        !explainsAfter && (explainedBefore || elapsedMs < UNSEEN_MS)
}

/**
 * What was refused for good in this run of the app, for
 * [PermissionState]: in memory only, process-wide so a recreated activity
 * (a turn of the phone, dark mode) still knows. Nothing is written
 * anywhere; the process ending forgets it, as Android ending it does when
 * a permission is taken away in Settings.
 */
object Refusals {
    private class Ask(val explained: Boolean, val at: Long)

    private val asks = mutableMapOf<String, Ask>()
    private val forGood = mutableSetOf<String>()

    /** Android's question is about to be asked, [explains] as it reads now. */
    @Synchronized
    fun asking(permission: String, explains: Boolean, at: Long) {
        asks[permission] = Ask(explains, at)
    }

    /** Android answered, [explains] as it reads after. */
    @Synchronized
    fun answered(permission: String, allowed: Boolean, explains: Boolean, at: Long) {
        val ask = asks.remove(permission)
        val refused = !allowed && PermissionState.refusedForGood(
            explainedBefore = ask?.explained ?: false,
            explainsAfter = explains,
            elapsedMs = if (ask == null) 0 else at - ask.at,
        )
        if (refused) forGood += permission else forGood -= permission
    }

    @Synchronized
    fun refusedForGood(permission: String): Boolean = permission in forGood

    /** For tests that set Android's own flags, run in one process. */
    @Synchronized
    fun forget() {
        asks.clear()
        forGood.clear()
    }
}

/** The app's page in Android's Settings, where all its permissions are. */
object AppSettings {
    fun intent(packageName: String): Intent =
        Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", packageName, null))

    /** Over the app from an activity; in a task of its own from anywhere else. */
    fun open(context: Context) {
        val intent = intent(context.packageName)
        if (context !is Activity) intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        runCatching { context.startActivity(intent) }
    }
}
