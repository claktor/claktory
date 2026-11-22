package com.claktory.app

import android.app.Activity
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.Build
import android.os.Handler
import android.os.Looper
import androidx.core.content.ContextCompat
import io.flutter.plugin.common.MethodChannel

/**
 * factory/device's share, as on the iPhone (ios/Runner/AppDelegate.swift,
 * lib/mobile/share.dart): {text, rect} opens the system's share sheet for
 * the text and answers true once the person sent it somewhere, false if
 * they closed it or nothing could show it. Android has no popover, so
 * rect is unused. Pure, over [open] (shows the sheet) and [later] (runs
 * something after a moment), so it's tested on the JVM.
 *
 * Android says which app was chosen ([chosen]) as the person picks it; it
 * says nothing when the sheet is closed, so coming back to the app with
 * nothing chosen, after a moment for a late word, answers false.
 */
class ShareCore(
    private val open: (text: String) -> Boolean,
    private val later: (Long, () -> Unit) -> Unit,
) {
    private var waiting: MethodChannel.Result? = null
    private var shown = false

    /** Which share this is, so a late answer can't end the next one. */
    private var turn = 0

    fun share(text: String?, result: MethodChannel.Result) {
        if (text.isNullOrEmpty()) return result.success(false)
        finish(false)
        waiting = result
        shown = false
        turn++
        if (!open(text)) finish(false)
    }

    /** The person picked where to send it. */
    fun chosen() = finish(true)

    fun paused() {
        if (waiting != null) shown = true
    }

    fun resumed() {
        if (waiting == null || !shown) return
        val now = turn
        later(LATE_MS) { if (turn == now) finish(false) }
    }

    fun cancel() = finish(false)

    private fun finish(sent: Boolean) {
        val result = waiting ?: return
        waiting = null
        shown = false
        result.success(sent)
    }

    companion object {
        /** How long a word of the choice may lag behind the app coming back. */
        const val LATE_MS = 600L
    }
}

/**
 * [ShareCore] in the app: ACTION_SEND of plain text through the system's
 * chooser, which reports the app picked to a receiver only this app can
 * reach. Text only, as the iPhone's contract is: no files, no permission.
 */
class ShareSheet(private val activity: Activity) : HostLifecycle, AutoCloseable {
    private val main = Handler(Looper.getMainLooper())
    private val action = "${activity.packageName}.SHARED"
    private var listening = false

    private val picked = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (intent.action == action) core.chosen()
        }
    }

    val core = ShareCore(
        open = { text -> show(text) },
        later = { ms, run -> main.postDelayed(run, ms) },
    )

    private fun show(text: String): Boolean {
        listen()
        val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, text)
        // Mutable, so the chooser can add which app was picked; explicit, to this app only.
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or
            (if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) PendingIntent.FLAG_MUTABLE else 0)
        val report = PendingIntent.getBroadcast(activity, REQUEST, Intent(action).setPackage(activity.packageName), flags)
        return try {
            activity.startActivity(Intent.createChooser(send, null, report.intentSender))
            true
        } catch (error: Exception) {
            false
        }
    }

    private fun listen() {
        if (listening) return
        val filter = IntentFilter(action)
        // Only this app can reach it, on every Android: from 13 by the flag,
        // before that by androidx's permission only this app holds, so no
        // other app can claim a share was picked. (androidx.core comes with
        // Flutter's embedding; declaring it here would change what Gradle
        // resolves and verifies.)
        ContextCompat.registerReceiver(activity, picked, filter, ContextCompat.RECEIVER_NOT_EXPORTED)
        listening = true
    }

    override fun onPause() = core.paused()

    override fun onResume() = core.resumed()

    override fun close() {
        core.cancel()
        if (listening) runCatching { activity.unregisterReceiver(picked) }
        listening = false
    }

    companion object {
        const val REQUEST = 0x4177
    }
}
