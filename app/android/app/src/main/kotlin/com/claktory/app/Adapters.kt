package com.claktory.app

import android.app.Activity
import android.content.Intent
import android.content.res.Configuration
import io.flutter.embedding.engine.FlutterEngine

/**
 * What an adapter hears of the activity's life, when it needs to: the
 * activity forwards each to every adapter that implements this.
 */
interface HostLifecycle {
    fun onResume() {}

    /** After the activity and Flutter have both resumed (Flutter resets its system UI flags then). */
    fun onPostResume() {}

    fun onPause() {}

    fun onFocus(focused: Boolean) {}

    /** A configuration change the activity handles itself (configChanges), such as light to dark. */
    fun onConfiguration(config: Configuration) {}

    /** True when it asked for these permissions and has handled the answer. */
    fun onPermissionResult(requestCode: Int, permissions: Array<out String>, grants: IntArray): Boolean = false

    /** True when it started that activity and has handled what it answered. */
    fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?): Boolean = false

    /** True when the intent the activity was brought back with was for it, and it took it. */
    fun onNewIntent(intent: Intent): Boolean = false
}

/**
 * Which adapter answers which channel. Each owns its channel and its
 * Android resources; [close] releases them when the engine goes.
 */
class Adapters private constructor(private val all: List<AutoCloseable>) : AutoCloseable {
    override fun close() = all.forEach { it.close() }

    private val lifecycles get() = all.filterIsInstance<HostLifecycle>()

    fun onResume() = lifecycles.forEach { it.onResume() }

    fun onPostResume() = lifecycles.forEach { it.onPostResume() }

    fun onPause() = lifecycles.forEach { it.onPause() }

    fun onFocus(focused: Boolean) = lifecycles.forEach { it.onFocus(focused) }

    fun onConfiguration(config: Configuration) = lifecycles.forEach { it.onConfiguration(config) }

    fun onPermissionResult(requestCode: Int, permissions: Array<out String>, grants: IntArray): Boolean =
        lifecycles.any { it.onPermissionResult(requestCode, permissions, grants) }

    fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?): Boolean =
        lifecycles.any { it.onActivityResult(requestCode, resultCode, data) }

    fun onNewIntent(intent: Intent): Boolean = lifecycles.any { it.onNewIntent(intent) }

    /** The adapter of this kind, for tests that drive one inside the running app. */
    fun <T> find(kind: Class<T>): T? = all.filterIsInstance(kind).firstOrNull()

    companion object {
        /** Only adapters that need the activity itself get it; the rest the application's context. */
        fun register(engine: FlutterEngine, activity: Activity): Adapters {
            val messenger = engine.dartExecutor.binaryMessenger
            val context = activity.applicationContext
            // The phone's camera app for factory/device's takePhoto, and the
            // system's pickers for its pickAttachments.
            val photos = PhotoCapture(activity)
            val attachments = AttachmentPicker(activity, camera = photos)
            // The camera's permission, and the pairing screen's camera views.
            val camera = CameraAccess(activity)
            val scanner = ScannerFactory(messenger)
            engine.platformViewsController.registry.registerViewFactory("claktory/scanner", scanner)
            // Web sign-in: the service's page, and its way back to the app.
            val webAuth = WebAuth(activity)
            // The system's share sheet, for text.
            val share = ShareSheet(activity)
            return Adapters(
                listOf(
                    // No channel: the system bars' icons, dark on light and light on dark.
                    SystemBars(activity),
                    // factory/network: iroh can't see network moves on Android.
                    NetworkPath(messenger, context),
                    // factory/device: what this phone keeps and what it is.
                    DeviceChannel(messenger, context, attachments, camera, webAuth, share, photos),
                    // factory/dm-keys: the keys of end-to-end direct messages.
                    DmKeysChannel(messenger, context),
                    // factory/secret-clipboard: Copy code's secret, cleared only if still there.
                    SecretClipboardChannel(messenger, context),
                    webAuth,
                    share,
                    // factory/back-gesture: where the system's Back swipe doesn't start (the mic).
                    BackGestureExclusion(messenger, activity),
                    // factory/links: claktory://add?code=… opens the app.
                    LinksChannel(messenger, activity),
                    // factory/share-browser: a friend's shared website, in a Custom Tab kept alive.
                    ShareBrowser(messenger, activity),
                    // factory/shared: what other apps share to Claktory.
                    SharedChannel(messenger, activity),
                    attachments,
                    photos,
                    camera,
                    scanner,
                    // factory/recorder: voice messages.
                    RecorderChannel(messenger, activity),
                    // factory/audio: voice messages played back.
                    AudioChannel(messenger, context),
                    // claktory/video: the video viewer's players, on the engine's textures.
                    VideoChannel(messenger, engine.renderer),
                ),
            )
        }
    }
}
