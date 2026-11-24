package com.claktory.app

import android.app.Activity
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.ClipData
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.webkit.MimeTypeMap
import androidx.core.content.ContextCompat
import androidx.core.content.FileProvider
import io.flutter.plugin.common.MethodChannel
import java.io.File

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
    private val openFiles: (FileShare.Plan) -> Boolean = { false },
) {
    private var waiting: MethodChannel.Result? = null
    private var shown = false

    /** Which share this is, so a late answer can't end the next one. */
    private var turn = 0

    fun share(text: String?, result: MethodChannel.Result) {
        if (text.isNullOrEmpty()) return result.success(false)
        start(result) { open(text) }
    }

    /** Files the app wrote to share (#418), answered as text is; null (refused) answers false. */
    fun shareFiles(plan: FileShare.Plan?, result: MethodChannel.Result) {
        if (plan == null) return result.success(false)
        start(result) { openFiles(plan) }
    }

    private fun start(result: MethodChannel.Result, show: () -> Boolean) {
        finish(false)
        waiting = result
        shown = false
        turn++
        if (!show()) finish(false)
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
 * [ShareCore] in the app: ACTION_SEND of plain text, or ACTION_SEND or
 * ACTION_SEND_MULTIPLE of the files the app wrote to share (#418), through
 * the system's chooser, which reports the app picked to a receiver only
 * this app can reach. Files go as [ShareFiles] content addresses, with a
 * read grant for them alone; no permission. Claktory itself isn't offered:
 * sharing into itself would only go round.
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
        open = { text -> show(Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, text)) },
        later = { ms, run -> main.postDelayed(run, ms) },
        openFiles = { plan -> runCatching { files(plan) }.getOrNull()?.let(::show) ?: false },
    )

    /** The files' own folder, the one [ShareFiles] serves. */
    val root: File get() = File(activity.cacheDir, FileShare.FOLDER)

    /** factory/device's shareFiles {paths, mimes}: checked, then shown. */
    fun shareFiles(arguments: Map<*, *>?, result: MethodChannel.Result) {
        val guess = { extension: String -> MimeTypeMap.getSingleton().getMimeTypeFromExtension(extension) }
        val plan = FileShare.plan(arguments?.get("paths") as? List<*>, arguments?.get("mimes") as? List<*>, root, guess)
        // Older shares go now, off the main thread; this one's folder stays.
        val keep = plan?.items?.map { it.file }?.toSet() ?: emptySet()
        val now = System.currentTimeMillis()
        Thread({ runCatching { FileShare.sweep(root, now, keep) } }, "claktory-share-sweep").start()
        core.shareFiles(plan, result)
    }

    /** The send for [plan]: one file or several, read-only, nothing else. */
    internal fun files(plan: FileShare.Plan): Intent {
        val authority = ShareFiles.authority(activity)
        val uris = plan.items.map { FileProvider.getUriForFile(activity, authority, it.file) }
        val send = if (uris.size == 1) {
            Intent(Intent.ACTION_SEND).putExtra(Intent.EXTRA_STREAM, uris.single())
        } else {
            Intent(Intent.ACTION_SEND_MULTIPLE).putParcelableArrayListExtra(Intent.EXTRA_STREAM, ArrayList(uris))
        }
        // The grant goes with ClipData through the chooser to the app picked.
        val clip = ClipData(null, plan.items.map { it.mime }.distinct().toTypedArray(), ClipData.Item(uris.first()))
        uris.drop(1).forEach { clip.addItem(ClipData.Item(it)) }
        send.clipData = clip
        return send.setType(plan.type).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    }

    private fun show(send: Intent): Boolean {
        listen()
        // Mutable, so the chooser can add which app was picked; explicit, to this app only.
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or
            (if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) PendingIntent.FLAG_MUTABLE else 0)
        val report = PendingIntent.getBroadcast(activity, REQUEST, Intent(action).setPackage(activity.packageName), flags)
        val chooser = Intent.createChooser(send, null, report.intentSender)
            .putExtra(Intent.EXTRA_EXCLUDE_COMPONENTS, arrayOf(ComponentName(activity, ShareActivity::class.java)))
        return try {
            activity.startActivity(chooser)
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

/**
 * The content provider for shared files (#418): read-only, not exported,
 * reachable only through a grant for one address, and only for files in
 * cache/claktory-share (res/xml/share_paths.xml). Its own class, so it
 * can't clash with another FileProvider in the merged manifest.
 */
class ShareFiles : FileProvider() {
    companion object {
        fun authority(context: Context) = "${context.packageName}.files"
    }
}
