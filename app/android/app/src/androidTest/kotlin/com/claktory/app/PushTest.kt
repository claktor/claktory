package com.claktory.app

import android.Manifest
import android.app.Notification
import android.app.NotificationManager
import android.content.Context
import android.content.ContextWrapper
import android.content.Intent
import android.os.Build
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.google.firebase.FirebaseApp
import com.google.firebase.messaging.FirebaseMessaging
import com.google.firebase.messaging.FirebaseMessagingService
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import org.json.JSONObject
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.security.SecureRandom
import java.util.Collections
import javax.crypto.Cipher
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec

/**
 * Pushes on the phone (PushChannel.kt, ClaktoryMessagingService.kt): a
 * Claktory's key kept through factory/push, a push sealed with it shown as
 * what it says in its channel, a replay as the generic line, and a tap
 * opening what it's about only for a push that showed; a forged intent opens
 * nothing; a notification payload never shows in its sender's words.
 * Firebase starts only in a build given a config
 * (-PclaktoryFirebaseConfig=app/src/androidTest/firebase-test-config.json:
 * a fake project, enough for Firebase to start, never to reach Google).
 */
@RunWith(AndroidJUnit4::class)
class PushTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext
    private val notifications = context.getSystemService(NotificationManager::class.java)

    @Before
    fun setUp() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) grant(Manifest.permission.POST_NOTIFICATIONS)
        notifications.cancelAll()
        DeviceStores.push(context).remove(CLAKTORY)
    }

    @After
    fun tearDown() {
        notifications.cancelAll()
        DeviceStores.push(context).remove(CLAKTORY)
    }

    private class Answer : MethodChannel.Result {
        val values: MutableList<Any?> = Collections.synchronizedList(mutableListOf())

        override fun success(result: Any?) {
            values += result
        }

        override fun error(errorCode: String, errorMessage: String?, errorDetails: Any?) {
            values += "error $errorCode"
        }

        override fun notImplemented() {
            values += "notImplemented"
        }
    }

    private fun call(screen: Screen, method: String, arguments: Any? = null): Any? {
        val answer = Answer()
        lateinit var push: PushChannel
        screen.onActivity { push = it.adapter(PushChannel::class.java)!! }
        screen.onActivity { push.onMethodCall(MethodCall(method, arguments), answer) }
        val until = System.currentTimeMillis() + 10_000
        while (answer.values.isEmpty() && System.currentTimeMillis() < until) Thread.sleep(50)
        if (answer.values.size != 1) throw AssertionError("${answer.values.size} answers to $method")
        return answer.values.single()
    }

    private fun seal(key: ByteArray, ctr: Long, sentAt: Long = System.currentTimeMillis()): String {
        val json = JSONObject()
            .put("v", 1).put("id", "notify-$ctr").put("ctr", ctr).put("sent_at", sentAt).put("ttl", 3600)
            .put("claktory", "Studio").put("kind", "needs_you").put("title", "Pericolo")
            .put("body", "Deploy tonight?").put("badge", 2).put("urgent", false)
            .put("target", JSONObject().put("agent", "devops").put("pipe", "").put("chat", "").put("seq", 42))
        val plain = json.toString().toByteArray().let { it + ByteArray(512 - it.size) { ' '.code.toByte() } }
        val nonce = ByteArray(12).also(SecureRandom()::nextBytes)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, SecretKeySpec(key, "AES"), GCMParameterSpec(128, nonce))
        return Base64Codec.encode(nonce + cipher.doFinal(plain))
    }

    /** What Android shows that [matches]: it posts on its own thread, so waited for a little. */
    private fun waitShown(matches: (android.service.notification.StatusBarNotification) -> Boolean) =
        run {
            val until = System.currentTimeMillis() + 5_000
            var found = notifications.activeNotifications.firstOrNull(matches)
            while (found == null && System.currentTimeMillis() < until) {
                Thread.sleep(100)
                found = notifications.activeNotifications.firstOrNull(matches)
            }
            found
        }

    private fun shown(tag: String) = waitShown { it.tag == tag }

    @Test
    fun aKeptKeyOpensItsPushAndATapOpensWhatItsAbout() {
        Screen.launch().use { screen ->
            val key = call(
                screen,
                "register",
                mapOf("claktory" to CLAKTORY, "handle" to "h1", "token" to "t1", "relay" to "", "platform" to "fcm"),
            ) as ByteArray
            assertEquals(32, key.size)
            // The same key again; a new one when asked.
            assertTrue(key.contentEquals(call(screen, "register", mapOf("claktory" to CLAKTORY, "handle" to "h1", "token" to "t1", "platform" to "fcm")) as ByteArray))
            val registration = call(screen, "registration", mapOf("claktory" to CLAKTORY)) as Map<*, *>
            assertEquals("h1", registration["handle"])
            assertEquals("fcm", registration["platform"])
            assertTrue(key.contentEquals(registration["key"] as ByteArray))
            assertEquals(listOf(CLAKTORY), call(screen, "kept"))

            // As if behind another app: a push shows.
            Foreground.resumed = false
            val sealed = seal(key, ctr = 1)
            assertEquals(PushReceiver.Shown.NOTICE, PushReceiver.receive(context, mapOf("c" to sealed, "t" to "thread")))
            val notice = shown("$CLAKTORY:notify-1") ?: throw AssertionError("not shown")
            assertEquals("Pericolo", notice.notification.extras.getString("android.title"))
            assertEquals("Deploy tonight?", notice.notification.extras.getString("android.text"))
            assertEquals("Studio", notice.notification.extras.getString("android.subText"))
            assertEquals("thread", notice.notification.group)
            // A lock screen that hides what notifications say shows the generic line.
            assertEquals(Notification.VISIBILITY_PRIVATE, notice.notification.visibility)
            val public = notice.notification.publicVersion ?: throw AssertionError("no public version")
            assertEquals(context.getString(R.string.push_generic), public.extras.getString("android.text"))
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                assertEquals(PushNotifications.NEEDS_YOU, notice.notification.channelId)
            }

            // The same push again is a replay: the generic line.
            notifications.cancelAll()
            assertEquals(PushReceiver.Shown.GENERIC, PushReceiver.receive(context, mapOf("c" to sealed)))

            // A tap on the one that showed opens its agent at its item.
            lateinit var channel: NotificationsChannel
            screen.onActivity { channel = it.adapter(NotificationsChannel::class.java)!! }
            channel.lastClickedForTest = null
            notice.notification.contentIntent.send()
            val until = System.currentTimeMillis() + 10_000
            while (channel.lastClickedForTest == null && System.currentTimeMillis() < until) Thread.sleep(50)
            val target = channel.lastClickedForTest ?: throw AssertionError("the tap opened nothing")
            assertEquals(CLAKTORY, target["claktory"])
            assertEquals("devops", target["agentId"])
            assertEquals(42L, target["seq"])

            // An intent from anywhere else, with a push that never showed, opens nothing.
            channel.lastClickedForTest = null
            val forged = Intent(context, MainActivity::class.java)
                .setAction(PushNotifications.ACTION_OPEN)
                .putExtra(PushNotifications.EXTRA_SEALED, seal(key, ctr = 9))
                .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_NEW_TASK)
            context.startActivity(forged)
            context.startActivity(
                Intent(context, MainActivity::class.java).setAction(PushNotifications.ACTION_OPEN)
                    .putExtra(PushNotifications.EXTRA_POSTED, "made-up")
                    .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_NEW_TASK),
            )
            Thread.sleep(2_000)
            assertNull(channel.lastClickedForTest)

            // In front, a push shows nothing: the app has it already.
            Foreground.resumed = true
            assertEquals(PushReceiver.Shown.NOTHING, PushReceiver.receive(context, mapOf("c" to seal(key, ctr = 2))))

            call(screen, "forget", mapOf("claktory" to CLAKTORY))
            assertEquals(emptyList<String>(), call(screen, "kept"))
        }
    }

    @Test
    fun aPushNoKeyOpensShowsTheGenericLine() {
        Foreground.resumed = false
        assertEquals(PushReceiver.Shown.GENERIC, PushReceiver.receive(context, mapOf("c" to seal(ByteArray(32), ctr = 1))))
        val generic = waitShown { true } ?: throw AssertionError("not shown")
        assertEquals("Claktory", generic.notification.extras.getString("android.title"))
        assertEquals(context.getString(R.string.push_generic), generic.notification.extras.getString("android.text"))
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            assertEquals(PushNotifications.OTHER, generic.notification.channelId)
        }
    }

    /** [service] as Android would have made it, to hand an intent to. */
    private fun <T : ContextWrapper> attached(service: T): T = service.also {
        ContextWrapper::class.java.getDeclaredMethod("attachBaseContext", Context::class.java)
            .apply { isAccessible = true }.invoke(it, context)
    }

    /** A message as Firebase's receiver hands it on: a notification payload in [title]'s words, nothing sealed. */
    private fun notificationMessage(title: String) = Intent("com.google.android.c2dm.intent.RECEIVE")
        .putExtra("google.message_id", "m-${System.nanoTime()}")
        .putExtra("from", "1234567890")
        .putExtra("gcm.n.e", "1")
        .putExtra("gcm.notification.e", "1")
        .putExtra("gcm.notification.title", title)
        .putExtra("gcm.notification.body", "Sign in again at example.com")
        .putExtra("gcm.n.title", title)
        .putExtra("gcm.n.body", "Sign in again at example.com")

    private fun shownWith(title: String) = notifications.activeNotifications.any {
        it.notification.extras.getCharSequence("android.title")?.toString() == title
    }

    /**
     * Fugo 50974: Firebase never shows a notification payload in the
     * sender's words. Pinned against firebase-messaging's own handling: the
     * same message to Firebase's plain service shows (the control), to
     * Claktory's it shows only the generic line.
     */
    @Test
    fun firebaseNeverShowsANotificationPayloadByItself() {
        assumeTrue("built with Firebase's config", FirebaseApp.getApps(context).isNotEmpty())
        Foreground.resumed = false
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            FirebaseMessaging.getInstance()
            Thread.sleep(1_000)
            assertNull("Play services posts for the app", notifications.notificationDelegate)
        }

        attached(FirebaseMessagingService()).handleIntent(notificationMessage("Control"))
        val until = System.currentTimeMillis() + 5_000
        while (!shownWith("Control") && System.currentTimeMillis() < until) Thread.sleep(100)
        // A failure, never a skip (Fugo 51399): if Firebase's own service
        // stops showing it, this test no longer proves anything, so say so.
        assertTrue(
            "Firebase's own service didn't show the control: it took the app as in front, or reads payloads " +
                "another way now; run with no Claktory screen open, then look again at handleIntent",
            shownWith("Control"),
        )
        notifications.cancelAll()

        attached(ClaktoryMessagingService()).handleIntent(notificationMessage("Claktory: sign in again"))
        Thread.sleep(2_000)
        assertTrue("its words shown", !shownWith("Claktory: sign in again"))
        val generic = waitShown { true } ?: throw AssertionError("nothing shown")
        assertEquals(1, notifications.activeNotifications.size)
        assertEquals(context.getString(R.string.push_generic), generic.notification.extras.getString("android.text"))
    }

    @Test
    fun theStateSaysWhetherPushesCanReachThisPhone() {
        Screen.launch().use { screen ->
            val state = call(screen, "state") as Map<*, *>
            assertEquals("fcm", state["platform"])
            val started = FirebaseApp.getApps(context).isNotEmpty()
            if (!started) {
                assertEquals("built without Firebase's config", "unavailable", state["permission"])
                return
            }
            assumeTrue("Google Play services here", PushChannel.available(context))
            // Allowed in setUp.
            assertEquals("allowed", state["permission"])
            assertNotNull(state["token"])
        }
    }

    companion object {
        const val CLAKTORY = "e1device-test"
    }
}
