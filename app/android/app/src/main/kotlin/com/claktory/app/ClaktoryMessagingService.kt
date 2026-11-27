package com.claktory.app

import android.content.Context
import android.content.Intent
import android.util.Log
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage

/**
 * Firebase's pushes to this phone: a high-priority data message `{c: sealed,
 * t: thread, u: "1" when urgent}` from Claktory's push relay
 * (crates/claktory-push/src/fcm.rs), handed to [PushReceiver] at once,
 * without starting Flutter (Firebase gives a few seconds).
 *
 * Firebase never shows a notification by itself (Fugo 50974): a message
 * with a notification payload would otherwise go straight to the tray in
 * the sender's own words, so anyone holding the relay's FCM credentials
 * could put any text on the lock screen under Claktory's name. Its
 * notification keys are taken off before Firebase reads the message
 * ([NotificationPayloads]), so what's left is a data message, which the
 * receiver opens or shows as the generic line; Play services posting for
 * the app (notification delegation) is off in the manifest. Pinned by
 * PushTest: a future firebase-messaging that reads a notification payload
 * some other way fails it.
 */
class ClaktoryMessagingService : FirebaseMessagingService() {
    override fun handleIntent(intent: Intent) {
        if (NotificationPayloads.strip(intent)) Log.i(TAG, "push with a notification payload: shown generic")
        super.handleIntent(intent)
    }

    override fun onMessageReceived(message: RemoteMessage) {
        PushReceiver.receive(applicationContext, message.data)
    }

    /** Firebase gave this phone a new token: the app registers again with it. */
    override fun onNewToken(token: String) {
        PushChannel.current?.tokenChanged(token)
    }
}

private const val TAG = "Claktory"

/**
 * The keys by which Firebase reads a message as one to show by itself
 * ("gcm.n.e", "gcm.notification.title" and the rest, NotificationParams'
 * prefixes): taken off, the message is plain data.
 */
object NotificationPayloads {
    private val PREFIXES = listOf("gcm.n.", "gcm.notification.")

    fun isNotificationKey(key: String) = PREFIXES.any { key.startsWith(it) } || key == "gcm.notification"

    /** Takes them off [intent]: true when it had any. */
    fun strip(intent: Intent): Boolean {
        val keys = intent.extras?.keySet()?.filter(::isNotificationKey).orEmpty()
        keys.forEach(intent::removeExtra)
        return keys.isNotEmpty()
    }
}

/**
 * What a push shows, as the iPhone's notification extension decides it:
 * what opens, new and fresh, shows what it says, its counter kept so it
 * can't show twice; anything else shows the generic line, so a
 * high-priority push always shows something. In front, nothing shows: the
 * app has it from its Claktory already.
 */
object PushReceiver {
    /** What it did, for tests. */
    enum class Shown { NOTICE, GENERIC, NOTHING }

    fun receive(context: Context, data: Map<String, String>, nowMs: Long = System.currentTimeMillis()): Shown {
        val sealed = data["c"]
        val thread = PushOpener.group(data["t"])
        val store = DeviceStores.push(context)
        val keys = store.keys()
        return when (val outcome = PushOpener.open(sealed, keys, nowMs)) {
            is PushOpener.Outcome.Shows -> {
                val key = keys[outcome.claktory]
                // Not kept, it could show again: the generic line instead.
                val kept = key != null && sealed != null &&
                    store.saveKey(outcome.claktory, key.show(outcome.notice.ctr, sealed))
                when {
                    Foreground.resumed -> Shown.NOTHING
                    kept -> {
                        PushNotifications.showPush(context, outcome.claktory, outcome.notice, sealed!!, thread)
                        Shown.NOTICE
                    }
                    else -> {
                        PushNotifications.showGeneric(context, thread)
                        Shown.GENERIC
                    }
                }
            }
            is PushOpener.Outcome.Generic -> {
                Log.i(TAG, "push shown generic (${outcome.why.name.lowercase()})")
                if (Foreground.resumed) {
                    Shown.NOTHING
                } else {
                    PushNotifications.showGeneric(context, thread)
                    Shown.GENERIC
                }
            }
        }
    }
}
