package com.claktory.app

import android.annotation.SuppressLint
import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec

/**
 * Opening pushes (PushCore.kt) against the backend's own test vectors
 * (crates/factoryd/src/notify/push-vectors.json, made by notify/push.rs),
 * as the iPhone's PushTests do, and Fugo's rules: only what opens, new and
 * fresh shows; a tap opens what it's about only for a push that showed.
 */
class PushCoreTest {
    private fun vectors(): JSONObject {
        var dir: File? = File(System.getProperty("user.dir")).absoluteFile
        while (dir != null) {
            val file = File(dir, "crates/factoryd/src/notify/push-vectors.json")
            if (file.isFile) return JSONObject(file.readText())
            dir = dir.parentFile
        }
        throw AssertionError("push-vectors.json not found above ${System.getProperty("user.dir")}")
    }

    private fun hex(text: String) = ByteArray(text.length / 2) { text.substring(it * 2, it * 2 + 2).toInt(16).toByte() }

    /** Seals [json] as a Claktory does, padded to 512 bytes. */
    private fun seal(json: JSONObject, key: ByteArray): String {
        val plain = json.toString().toByteArray().let { it + ByteArray(512 - it.size) { ' '.code.toByte() } }
        val nonce = ByteArray(12).also(SecureRandom()::nextBytes)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, SecretKeySpec(key, "AES"), GCMParameterSpec(128, nonce))
        return Base64Codec.encode(nonce + cipher.doFinal(plain))
    }

    private fun notice(ctr: Long, sentAt: Long, ttl: Long = 86400) = JSONObject()
        .put("v", 1).put("id", "n$ctr").put("ctr", ctr).put("sent_at", sentAt).put("ttl", ttl)
        .put("claktory", "Studio").put("kind", "alert").put("title", "Pericolo")
        .put("body", "account.claktory.com isn't answering").put("badge", 1).put("urgent", true)
        .put("target", JSONObject().put("agent", "devops").put("pipe", "").put("chat", "").put("seq", 7))

    @Test
    fun opensTheBackendsVectorsInOrder() {
        val vectors = vectors()
        var keys = mapOf("e1" to PushKey(hex(vectors.getString("key_hex"))))
        // Checked at the time they were sealed, as their file says.
        val now = 1_800_000_000_000L
        val list = vectors.getJSONArray("vectors")
        assertEquals(7, list.length())
        for (i in 0 until list.length()) {
            val vector = list.getJSONObject(i)
            val name = vector.getString("name")
            val sealed = vector.getString("c")
            val outcome = PushOpener.open(sealed, keys, now)
            if (vector.getString("expect") == "shows") {
                val plain = vector.getJSONObject("plaintext")
                val shows = outcome as? PushOpener.Outcome.Shows ?: throw AssertionError("$name: $outcome")
                assertEquals(name, "e1", shows.claktory)
                assertEquals(name, plain.getString("title"), shows.notice.title)
                assertEquals(name, plain.getString("body"), shows.notice.body)
                assertEquals(name, plain.getString("claktory"), shows.notice.claktory)
                assertEquals(name, plain.getString("id"), shows.notice.id)
                assertEquals(name, plain.getInt("badge"), shows.notice.badge)
                assertEquals(name, vector.getLong("ctr"), shows.notice.ctr)
                // Shown: its counter and digest are kept, as the service keeps them.
                keys = mapOf("e1" to keys.getValue("e1").show(shows.notice.ctr, sealed))
                val target = PushOpener.tapped(sealed, keys) ?: throw AssertionError("$name: no target")
                assertEquals("e1", target["claktory"])
                assertEquals("devops", target["agentId"])
                assertEquals(42L, target["seq"])
            } else {
                assertTrue("$name: $outcome", outcome is PushOpener.Outcome.Generic)
            }
        }
        // The replay showed the generic line, and its tap opens nothing:
        // only the pushes shown are kept.
        val replay = list.getJSONObject(list.length() - 1).getString("c")
        assertNull(PushOpener.tapped(replay, keys))
        assertEquals(4, keys.getValue("e1").shown.size)
    }

    @Test
    fun aStalePushIsGenericAndATapOnItOpensNothing() {
        val key = PushKey.random()
        val sentAt = 1_000_000L
        val sealed = seal(notice(1, sentAt, ttl = 60), key.key)
        val keys = mapOf("e1" to key)
        // Within its ttl and the slack, it shows.
        assertTrue(PushOpener.open(sealed, keys, sentAt + 60_000 + PushOpener.SLACK_MS) is PushOpener.Outcome.Shows)
        assertEquals(
            PushOpener.Outcome.Generic(PushOpener.Why.STALE),
            PushOpener.open(sealed, keys, sentAt + 60_000 + PushOpener.SLACK_MS + 1),
        )
        // Never shown: a tap only opens the app.
        assertNull(PushOpener.tapped(sealed, keys))
    }

    @Test
    fun anotherClaktorysKeyOrNoneLeavesItUnopened() {
        val sealed = seal(notice(1, 0), PushKey.random().key)
        assertEquals(PushOpener.Outcome.Generic(PushOpener.Why.UNOPENED), PushOpener.open(sealed, mapOf("e1" to PushKey.random()), 0))
        assertEquals(PushOpener.Outcome.Generic(PushOpener.Why.UNOPENED), PushOpener.open(null, emptyMap(), 0))
        assertEquals(PushOpener.Outcome.Generic(PushOpener.Why.UNOPENED), PushOpener.open("not base64!", emptyMap(), 0))
    }

    @Test
    fun theKeyThatOpensItNamesTheClaktory() {
        val mine = PushKey.random()
        val sealed = seal(notice(3, 0), mine.key)
        val outcome = PushOpener.open(sealed, mapOf("a" to PushKey.random(), "b" to mine), 0)
        assertEquals("b", (outcome as PushOpener.Outcome.Shows).claktory)
        assertTrue(outcome.notice.urgent)
        assertEquals("alert", outcome.notice.kind)
    }

    @Test
    fun aWaitingDevicesPushNamesItAndAnOlderOneNone() {
        val json = notice(1, 0).put("target", JSONObject().put("device", "d1"))
        assertEquals("d1", PushOpener.notice(json)!!.target.map("e1")["deviceId"])
        assertEquals("", PushOpener.notice(notice(1, 0))!!.target.map("e1")["deviceId"])
    }

    @Test
    fun aThreadIsOnlyAGroupKey() {
        assertEquals("abc-DEF_123", PushOpener.group("abc-DEF_123"))
        assertEquals("abcdef", PushOpener.group("a b/c.d\ne<f>"))
        assertEquals("", PushOpener.group(null))
        assertEquals(PushOpener.GROUP_MAX, PushOpener.group("x".repeat(500)).length)
    }

    @Test
    fun anAlertsPushNamesTheAlertATapOpens() {
        val json = notice(1, 0).put("target", JSONObject().put("agent", "devops").put("alert", "a7"))
        assertEquals("a7", PushOpener.notice(json)!!.target.map("e1")["alertId"])
        assertEquals("", PushOpener.notice(notice(1, 0))!!.target.map("e1")["alertId"])
    }

    @Test
    fun onlyVersionOneWithACounterIsANotice() {
        assertNull(PushOpener.notice(notice(1, 0).put("v", 2)))
        assertNull(PushOpener.notice(notice(1, 0).apply { remove("ctr") }))
        assertNotNull(PushOpener.notice(notice(1, 0)))
    }

    @Test
    fun keysKeepTheirLatestShownOnlyAndRoundTrip() {
        var key = PushKey.random()
        for (ctr in 1L..70L) key = key.show(ctr, "sealed $ctr")
        assertEquals(PushKey.KEPT, key.shown.size)
        assertEquals(70L, key.ctr)
        assertEquals(PushKey.digest("sealed 70"), key.shown.last())
        val back = PushKey.from(JSONObject(key.json().toString()))!!
        assertArrayEquals(key.key, back.key)
        assertEquals(key.ctr, back.ctr)
        assertEquals(key.shown, back.shown)
        // Not a key: too short.
        assertNull(PushKey.from(JSONObject().put("key", Base64Codec.encode(ByteArray(16)))))
    }

    @Test
    fun registrationsNeedAHandleATokenAndAPlatform() {
        val made = PushRegistration.from(mapOf("handle" to "h", "token" to "t", "platform" to "fcm"), 5)!!
        assertEquals(PushRegistration("h", "t", "", "fcm", 5), made)
        assertEquals(made, PushRegistration.from(JSONObject(made.json().toString())))
        assertNull(PushRegistration.from(mapOf("handle" to "", "token" to "t", "platform" to "fcm"), 5))
        assertNull(PushRegistration.from(mapOf("handle" to "h", "platform" to "fcm"), 5))
    }

    // A JVM test: the host's java.util.Base64 is the reference, never on a phone.
    @SuppressLint("NewApi")
    @Test
    fun base64MatchesTheStandardCodec() {
        val random = SecureRandom()
        for (size in 0..40) {
            val bytes = ByteArray(size).also(random::nextBytes)
            val text = java.util.Base64.getEncoder().encodeToString(bytes)
            assertEquals(text, Base64Codec.encode(bytes))
            assertArrayEquals(bytes, Base64Codec.decode(text))
        }
        assertNull(Base64Codec.decode("a"))
        assertNull(Base64Codec.decode("ab-c"))
    }

    @Test
    fun kindsGoToTheirChannels() {
        assertEquals(PushNotifications.NEEDS_YOU, PushNotifications.channelFor("needs_you"))
        assertEquals(PushNotifications.ALERTS, PushNotifications.channelFor("alert"))
        assertEquals(PushNotifications.REPLIES, PushNotifications.channelFor("reply"))
        assertEquals(PushNotifications.TESTS, PushNotifications.channelFor("test"))
        assertEquals(PushNotifications.OTHER, PushNotifications.channelFor("anything else"))
    }
}
