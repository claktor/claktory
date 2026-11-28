package com.claktory.app

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.Settings
import android.util.Log
import com.google.android.gms.common.ConnectionResult
import com.google.android.gms.common.GoogleApiAvailabilityLight
import com.google.firebase.FirebaseApp
import com.google.firebase.messaging.FirebaseMessaging
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.util.UUID

/**
 * factory/push, this phone's registration for pushes, as the iPhone's
 * (app/ios/Runner/Notifications.swift; app/lib/mobile/push.dart drives it):
 * - "state" → {permission, token, platform, refusedOnce}: whether the
 *   person allows notifications ("not_determined", "denied", "allowed"; or
 *   "unavailable" where this build has no Firebase or the phone no Google
 *   Play services), this phone's FCM token (empty until Firebase gives
 *   one), and "fcm". refusedOnce: refused once, and Android will still ask.
 * - "ask" → permission: asks the person (Android 13+); allowed, Firebase is
 *   asked for the token, which comes as "token" {token, platform}.
 * - "registration" {claktory} → {handle, token, relay, platform,
 *   registered, key} or null: what this phone registered for that Claktory.
 * - "register" {claktory, handle, token, relay, platform, newKey} → key:
 *   keeps the relay's handle, and the key (a new one when there's none, or
 *   with newKey), which the app hands to that Claktory.
 * - "kept" → [claktory]: the Claktories a key is kept for (never the keys).
 * - "forget" {claktory}: both gone.
 * - "openSettings": the app's notification settings in Android's Settings.
 *
 * Firebase isn't started on its own (firebase_messaging_auto_init_enabled
 * is false in the manifest): this phone asks it for a token only while
 * notifications are allowed. From Android 13 that's once the person allows
 * them; before 13 they're allowed from install, so the token is asked for
 * at the app's first start. Once asked, Firebase keeps auto-init on, and
 * neither turning notifications off nor forgetting a Claktory deletes
 * its token.
 */
class PushChannel(messenger: BinaryMessenger, private val activity: Activity) :
    MethodChannel.MethodCallHandler, HostLifecycle, AutoCloseable {
    private val channel = MethodChannel(messenger, "factory/push")
    private val main = Handler(Looper.getMainLooper())
    private val context: Context = activity.applicationContext
    private var asking: MethodChannel.Result? = null
    private var token = ""

    init {
        channel.setMethodCallHandler(this)
        current = this
        // A token can change: asked for at every start once allowed.
        if (available(context) && PushNotifications.allowed(context)) fetchToken()
    }

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        val claktory = call.argument<String>("claktory").orEmpty()
        when (call.method) {
            "state" -> result.success(
                mapOf(
                    "permission" to permission(),
                    "token" to token,
                    "platform" to PLATFORM,
                    "refusedOnce" to refusedOnce(),
                ),
            )
            "ask" -> ask(result)
            "registration" -> stored(result) {
                if (claktory.isEmpty()) return@stored null
                val store = DeviceStores.push(context)
                val registration = store.registration(claktory) ?: return@stored null
                val key = store.keys()[claktory] ?: return@stored null
                registration.map() + ("key" to key.key)
            }
            "register" -> stored(result) {
                val arguments = call.arguments as? Map<*, *> ?: emptyMap<String, Any>()
                val registration = PushRegistration.from(arguments, System.currentTimeMillis())
                if (claktory.isEmpty() || registration == null) throw PushFailure("arguments", "No registration.")
                val store = DeviceStores.push(context)
                val kept = store.keys()[claktory]
                val key = if (kept == null || arguments["newKey"] == true) PushKey.random() else kept
                if (!store.save(claktory, key, registration)) throw PushFailure("keychain", "The key couldn't be kept.")
                key.key
            }
            "openSettings" -> {
                openSettings()
                result.success(null)
            }
            "kept" -> stored(result) { DeviceStores.push(context).keys().keys.sorted() }
            "forget" -> stored(result) {
                if (claktory.isNotEmpty()) DeviceStores.push(context).remove(claktory)
                null
            }
            else -> result.notImplemented()
        }
    }

    /** The app's notification settings in Android's Settings (its page before Android 8). */
    private fun openSettings() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val intent = Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                .putExtra(Settings.EXTRA_APP_PACKAGE, activity.packageName)
            if (runCatching { activity.startActivity(intent) }.isSuccess) return
        }
        AppSettings.open(activity)
    }

    private fun permission(): String = when {
        !available(context) -> "unavailable"
        PushNotifications.allowed(context) -> "allowed"
        Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU || granted() -> "denied"
        else -> when (
            PermissionState.of(
                granted = false,
                refusedForGood = Refusals.refusedForGood(NOTIFICATIONS),
                androidWouldExplain = activity.shouldShowRequestPermissionRationale(NOTIFICATIONS),
            )
        ) {
            "denied" -> "denied"
            else -> "not_determined"
        }
    }

    private fun refusedOnce(): Boolean = Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
        available(context) && PermissionState.refusedOnce(granted(), activity.shouldShowRequestPermissionRationale(NOTIFICATIONS))

    private fun granted() = Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
        activity.checkSelfPermission(NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

    private fun ask(result: MethodChannel.Result) {
        when {
            !available(context) -> result.success("denied")
            PushNotifications.allowed(context) -> {
                fetchToken()
                result.success("allowed")
            }
            // Before Android 13 there's no question to ask: the app's switch is in Settings.
            Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU || granted() -> result.success("denied")
            asking != null -> result.success("denied")
            else -> {
                asking = result
                Refusals.asking(
                    NOTIFICATIONS,
                    explains = activity.shouldShowRequestPermissionRationale(NOTIFICATIONS),
                    at = SystemClock.elapsedRealtime(),
                )
                activity.requestPermissions(arrayOf(NOTIFICATIONS), REQUEST)
            }
        }
    }

    override fun onPermissionResult(requestCode: Int, permissions: Array<out String>, grants: IntArray): Boolean {
        if (requestCode != REQUEST) return false
        val allowed = grants.isNotEmpty() && grants.all { it == PackageManager.PERMISSION_GRANTED }
        Refusals.answered(
            NOTIFICATIONS,
            allowed,
            explains = activity.shouldShowRequestPermissionRationale(NOTIFICATIONS),
            at = SystemClock.elapsedRealtime(),
        )
        if (allowed) fetchToken()
        asking?.success(if (allowed) "allowed" else "denied")
        asking = null
        return true
    }

    /** Firebase's token for this phone, once the person allows notifications. */
    private fun fetchToken() {
        try {
            val messaging = FirebaseMessaging.getInstance()
            messaging.isAutoInitEnabled = true
            messaging.token.addOnCompleteListener { task ->
                if (task.isSuccessful) task.result?.let(::tokenChanged)
                else Log.i(TAG, "no push token (${task.exception?.javaClass?.simpleName})")
            }
        } catch (error: IllegalStateException) {
            Log.i(TAG, "no push token (${error.javaClass.simpleName})")
        }
    }

    /** A token from Firebase, now or later ([ClaktoryMessagingService.onNewToken]). */
    fun tokenChanged(new: String) {
        main.post {
            if (new.isEmpty()) return@post
            token = new
            channel.invokeMethod("token", mapOf("token" to new, "platform" to PLATFORM))
        }
    }

    private fun stored(result: MethodChannel.Result, body: () -> Any?) {
        DeviceStores.thread.execute {
            val answer = runCatching(body)
            main.post {
                answer.fold(
                    onSuccess = { result.success(it) },
                    onFailure = { error ->
                        if (error is PushFailure) result.error(error.code, error.message, null)
                        else result.error("failed", error.javaClass.simpleName, null)
                    },
                )
            }
        }
    }

    override fun close() {
        asking?.success("denied")
        asking = null
        if (current === this) current = null
        channel.setMethodCallHandler(null)
    }

    class PushFailure(val code: String, message: String) : Exception(message)

    companion object {
        const val PLATFORM = "fcm"

        /** Ours alone among the activity's permission requests. */
        const val REQUEST = 0x4179

        // POST_NOTIFICATIONS exists from Android 13; the string is the same before, where it's never asked.
        const val NOTIFICATIONS = "android.permission.POST_NOTIFICATIONS"

        private const val TAG = "Claktory"

        /** The one running, for a new token from the messaging service. */
        @Volatile
        var current: PushChannel? = null

        /** Whether pushes can reach this phone: this build has Firebase's config, and the phone Google Play services. */
        fun available(context: Context): Boolean = try {
            FirebaseApp.getApps(context).isNotEmpty() &&
                GoogleApiAvailabilityLight.getInstance().isGooglePlayServicesAvailable(context) == ConnectionResult.SUCCESS
        } catch (_: Exception) {
            false
        }
    }
}

/**
 * factory/notifications, as on the iPhone and the Mac (app/lib/notifications.dart):
 * - "listen" (from the app): answers whether the person is using the app,
 *   and starts the calls back, any tap that came before first.
 * - "post" (from the app) {id, title, body, thread, urgent, kind, target}:
 *   shown, whenever the app's Flutter says it (a Claktory other than the
 *   one on screen, or any while the person isn't using the app). Unlike
 *   the iPhone, Android keeps the app running a while behind other apps,
 *   connected, and a Claktory pushes only to devices that aren't
 *   connected: what it sends meanwhile comes this way, and shows.
 * - "using" (to the app) Bool: the app is in front; sent as it changes.
 * - "clicked" (to the app) target: a notification was tapped.
 */
class NotificationsChannel(messenger: BinaryMessenger, private val activity: Activity) :
    MethodChannel.MethodCallHandler, HostLifecycle, AutoCloseable {
    private val channel = MethodChannel(messenger, "factory/notifications")
    private val main = Handler(Looper.getMainLooper())
    private val context: Context = activity.applicationContext
    private var listening = false
    private val taps = mutableListOf<Map<String, Any>>()

    init {
        channel.setMethodCallHandler(this)
        PushNotifications.ensureChannels(context)
        val launch = activity.intent
        if (launch != null && launch.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY == 0) take(launch)
    }

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            "listen" -> {
                listening = true
                result.success(Foreground.resumed)
                for (tap in taps) channel.invokeMethod("clicked", tap)
                taps.clear()
            }
            "post" -> {
                post(call.arguments as? Map<*, *> ?: emptyMap<String, Any>())
                result.success(null)
            }
            else -> result.notImplemented()
        }
    }

    private fun post(notification: Map<*, *>) {
        val target = (notification["target"] as? Map<*, *>)
            ?.mapNotNull { (key, value) -> (key as? String)?.let { k -> value?.let { k to it } } }?.toMap()
        val posted = PostedTargets.keep(target)
        PushNotifications.showPosted(
            context,
            id = notification["id"] as? String ?: UUID.randomUUID().toString(),
            title = notification["title"] as? String ?: "Claktory",
            body = notification["body"] as? String ?: "",
            thread = notification["thread"] as? String ?: "",
            urgent = notification["urgent"] == true,
            kind = notification["kind"] as? String ?: "",
            posted = posted,
        )
    }

    override fun onResume() = said(true)

    override fun onPause() = said(false)

    private fun said(using: Boolean) {
        if (Foreground.resumed == using) return
        Foreground.resumed = using
        if (listening) channel.invokeMethod("using", using)
    }

    override fun onNewIntent(intent: Intent): Boolean = take(intent)

    /** A tap that brought the app: what it opens, by the rules in [PushNotifications]. */
    private fun take(intent: Intent): Boolean {
        if (intent.action != PushNotifications.ACTION_OPEN) return false
        val sealed = intent.getStringExtra(PushNotifications.EXTRA_SEALED)
        val posted = intent.getStringExtra(PushNotifications.EXTRA_POSTED)
        // Taken once: a recreated activity doesn't open it again.
        intent.removeExtra(PushNotifications.EXTRA_SEALED)
        intent.removeExtra(PushNotifications.EXTRA_POSTED)
        if (posted != null) {
            PostedTargets.take(posted)?.let(::clicked)
            return true
        }
        if (sealed == null) return true
        DeviceStores.thread.execute {
            val target = PushOpener.tapped(sealed, DeviceStores.push(context).keys())
            if (target != null) main.post { clicked(target) }
        }
        return true
    }

    /** The last tap's target, for device tests (it goes to Flutter all the same). */
    @Volatile
    internal var lastClickedForTest: Map<String, Any>? = null

    private fun clicked(target: Map<String, Any>) {
        lastClickedForTest = target
        if (listening) channel.invokeMethod("clicked", target) else taps += target
    }

    override fun close() {
        channel.setMethodCallHandler(null)
    }
}

/**
 * What the app's own notifications open, kept in this process under an id
 * only the tap's intent carries; a tap after the process ended only opens
 * the app. At most [KEPT], the oldest going first.
 */
object PostedTargets {
    private const val KEPT = 64
    private val targets = LinkedHashMap<String, Map<String, Any>>()

    @Synchronized
    fun keep(target: Map<String, Any>?): String {
        val id = UUID.randomUUID().toString()
        if (target != null) {
            targets[id] = target
            while (targets.size > KEPT) targets.remove(targets.keys.first())
        }
        return id
    }

    @Synchronized
    fun take(id: String): Map<String, Any>? = targets.remove(id)
}
