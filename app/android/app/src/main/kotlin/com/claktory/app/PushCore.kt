package com.claktory.app

import org.json.JSONArray
import org.json.JSONObject
import java.security.MessageDigest
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec

/**
 * Notifications pushed to this phone while the app isn't in front (#273,
 * notifications.md §3 in Research), as the iPhone opens them
 * (app/ios/Runner/Push.swift): each Claktory seals what it says with a key
 * only this phone and that Claktory have (AES-256-GCM), the push relay
 * sends it through Firebase as a data message `{c: sealed}`, and
 * [ClaktoryMessagingService] opens it here and shows what it says.
 *
 * What doesn't open, or opens as something already shown or too old, shows
 * the generic line, and a tap on it only opens the app (Fugo's rules).
 * Plain logic, tested on the JVM against the backend's own vectors
 * (crates/factoryd/src/notify/push-vectors.json).
 */

/** What a tap on a push opens, as the app's notifications take it (notifications.dart's NotificationTarget). */
data class PushTarget(
    val agent: String = "",
    val pipe: String = "",
    val chat: String = "",
    val seq: Long = 0,
    val device: String = "",
    /** The push's kind, "needs_you" or another. */
    val kind: String = "",
    /** An alert kept in Needs you, which a tap opens (Notify.alert_id). */
    val alert: String = "",
) {
    fun map(claktory: String): Map<String, Any> = mapOf(
        "claktory" to claktory,
        "agentId" to agent,
        "pipeId" to pipe,
        "chatId" to chat,
        "seq" to seq,
        "deviceId" to device,
        "kind" to kind,
        "alertId" to alert,
    )
}

/** What a push said, opened. */
data class PushNotice(
    /** The Notify's id: a push for one the app already showed replaces it rather than stacking. */
    val id: String,
    /** The Claktory's name, shown under the title. */
    val claktory: String,
    /** "needs_you", "alert", "reply" or "test". */
    val kind: String,
    val title: String,
    val body: String,
    val badge: Int?,
    val urgent: Boolean,
    /** Its counter: each push to this phone from a Claktory counts one up. */
    val ctr: Long,
    val target: PushTarget,
)

/** A Claktory's key as this phone keeps it, with what it has shown. */
class PushKey(
    /** 32 bytes: AES-256-GCM. */
    val key: ByteArray,
    /** The highest counter shown: a push that isn't past it is a replay. */
    val ctr: Long = 0,
    /** Digests of the pushes shown lately, the newest last: only a tap on one of these opens what it's about. */
    val shown: List<String> = emptyList(),
) {
    fun show(ctr: Long, sealed: String): PushKey =
        PushKey(key, maxOf(this.ctr, ctr), (shown + digest(sealed)).takeLast(KEPT))

    fun json(): JSONObject = JSONObject()
        .put("key", Base64Codec.encode(key))
        .put("ctr", ctr)
        .put("shown", JSONArray(shown))

    companion object {
        /** How many shown pushes are kept. */
        const val KEPT = 64

        /** A push's sealed text, in short: half its SHA-256, base64, as the iPhone keeps it. */
        fun digest(sealed: String): String =
            Base64Codec.encode(MessageDigest.getInstance("SHA-256").digest(sealed.toByteArray(Charsets.UTF_8)).copyOf(16))

        fun random(random: SecureRandom = SecureRandom()): PushKey = PushKey(ByteArray(32).also(random::nextBytes))

        fun from(json: JSONObject): PushKey? {
            val key = Base64Codec.decode(json.optString("key")) ?: return null
            if (key.size != 32) return null
            val shown = json.optJSONArray("shown")
            return PushKey(
                key,
                json.optLong("ctr", 0),
                List(shown?.length() ?: 0) { shown!!.optString(it) }.filter { it.isNotEmpty() },
            )
        }
    }
}

/** What this phone registered with the push relay for a Claktory. */
data class PushRegistration(
    /** The relay's handle for this phone, base64url. */
    val handle: String,
    /** The FCM token it was registered with. */
    val token: String,
    /** The relay, as the Claktory's settings named it ("" for Claktory's own). */
    val relay: String,
    /** "fcm". */
    val platform: String,
    /** When, in ms since 1970. */
    val registered: Long,
) {
    fun map(): Map<String, Any> = mapOf(
        "handle" to handle,
        "token" to token,
        "relay" to relay,
        "platform" to platform,
        "registered" to registered,
    )

    fun json(): JSONObject = JSONObject(map())

    companion object {
        /** From the app's arguments, or what was kept; null without a handle, a token or a platform. */
        fun from(values: Map<*, *>, now: Long): PushRegistration? {
            val handle = values["handle"] as? String ?: return null
            val token = values["token"] as? String ?: return null
            val platform = values["platform"] as? String ?: return null
            if (handle.isEmpty()) return null
            return PushRegistration(
                handle,
                token,
                values["relay"] as? String ?: "",
                platform,
                (values["registered"] as? Number)?.toLong() ?: now,
            )
        }

        fun from(json: JSONObject): PushRegistration? =
            from(json.keys().asSequence().associateWith { json.opt(it) }, 0)
    }
}

object PushOpener {
    /** How late past its ttl a push may still show: the clocks of the Claktory and this phone may differ a little. */
    const val SLACK_MS = 5 * 60 * 1000L

    /** The longest thread id kept as a group. */
    const val GROUP_MAX = 64

    /**
     * A push's thread (`t`, unsealed, from the relay) as a notification
     * group: only ever a key Android groups by, never shown and never what
     * a tap opens (Fugo 50974), so only letters, digits, '-' and '_', at
     * most [GROUP_MAX] of them; anything else drops out.
     */
    fun group(thread: String?): String =
        thread.orEmpty().filter { it in 'a'..'z' || it in 'A'..'Z' || it in '0'..'9' || it == '-' || it == '_' }
            .take(GROUP_MAX)

    sealed class Outcome {
        /** It opened with the key kept as [claktory]. */
        data class Shows(val claktory: String, val notice: PushNotice) : Outcome()

        /** It shows the generic line, and why. */
        data class Generic(val why: Why) : Outcome()
    }

    enum class Why {
        /** No ciphertext, or no key of this phone's opens it. */
        UNOPENED,

        /** A version this app doesn't know, or not what a push holds. */
        UNREADABLE,

        /** Its counter isn't past the highest shown. */
        REPLAYED,

        /** Older than its ttl. */
        STALE,
    }

    /** Opens [sealed] (base64: nonce ‖ ciphertext ‖ tag) with the first of [keys] that does, and checks it's new and fresh at [nowMs]. */
    fun open(sealed: String?, keys: Map<String, PushKey>, nowMs: Long): Outcome {
        val (claktory, json) = decrypt(sealed, keys) ?: return Outcome.Generic(Why.UNOPENED)
        val notice = notice(json) ?: return Outcome.Generic(Why.UNREADABLE)
        val key = keys[claktory]
        if (key == null || notice.ctr <= key.ctr) return Outcome.Generic(Why.REPLAYED)
        val sentAt = json.optDouble("sent_at", 0.0)
        val ttl = json.optDouble("ttl", 0.0)
        if (sentAt + ttl * 1000 + SLACK_MS < nowMs) return Outcome.Generic(Why.STALE)
        return Outcome.Shows(claktory, notice)
    }

    /**
     * What a tap on a push with [sealed] opens: its target, only when this
     * phone showed that very push (its digest is kept) and it opens with one
     * of [keys]; null otherwise, and the tap only opens the app.
     */
    fun tapped(sealed: String?, keys: Map<String, PushKey>): Map<String, Any>? {
        if (sealed == null) return null
        val (claktory, json) = decrypt(sealed, keys) ?: return null
        val notice = notice(json) ?: return null
        if (keys[claktory]?.shown?.contains(PushKey.digest(sealed)) != true) return null
        return notice.target.map(claktory)
    }

    /** The plaintext of [sealed] and the name of the key that opened it. */
    fun decrypt(sealed: String?, keys: Map<String, PushKey>): Pair<String, JSONObject>? {
        val data = sealed?.let(Base64Codec::decode) ?: return null
        if (data.size < NONCE + TAG) return null
        for ((claktory, key) in keys.toSortedMap()) {
            if (key.key.size != 32) continue
            val plain = try {
                Cipher.getInstance("AES/GCM/NoPadding").run {
                    init(Cipher.DECRYPT_MODE, SecretKeySpec(key.key, "AES"), GCMParameterSpec(TAG * 8, data, 0, NONCE))
                    doFinal(data, NONCE, data.size - NONCE)
                }
            } catch (_: Exception) {
                continue
            }
            // Padded with spaces, which JSON allows after the value.
            val json = try {
                JSONObject(String(plain, Charsets.UTF_8).trimEnd())
            } catch (_: Exception) {
                return null
            }
            return claktory to json
        }
        return null
    }

    /** The notice in a push's plaintext: version 1 only. */
    fun notice(json: JSONObject): PushNotice? {
        if (json.optInt("v", 0) != 1 || !json.has("ctr")) return null
        val ctr = json.optLong("ctr", -1)
        if (ctr < 0) return null
        val target = json.optJSONObject("target") ?: JSONObject()
        return PushNotice(
            id = json.optString("id"),
            claktory = json.optString("claktory"),
            kind = json.optString("kind"),
            title = json.optString("title"),
            body = json.optString("body"),
            badge = if (json.has("badge")) json.optInt("badge") else null,
            urgent = json.optBoolean("urgent", false),
            ctr = ctr,
            target = PushTarget(
                agent = target.optString("agent"),
                pipe = target.optString("pipe"),
                chat = target.optString("chat"),
                seq = target.optLong("seq", 0),
                // Older Claktories send none: no device.
                device = target.optString("device"),
                kind = json.optString("kind"),
                alert = target.optString("alert"),
            ),
        )
    }

    private const val NONCE = 12
    private const val TAG = 16
}

/**
 * Standard base64 with padding, as the backend writes the sealed text and
 * the iPhone keeps digests: java.util.Base64 needs Android 8, and
 * android.util.Base64 isn't there in JVM tests.
 */
object Base64Codec {
    private const val ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
    private val VALUES = IntArray(128) { -1 }.also { values -> ALPHABET.forEachIndexed { i, c -> values[c.code] = i } }

    fun encode(bytes: ByteArray): String {
        val out = StringBuilder((bytes.size + 2) / 3 * 4)
        var i = 0
        while (i < bytes.size) {
            val b0 = bytes[i].toInt() and 0xff
            val b1 = if (i + 1 < bytes.size) bytes[i + 1].toInt() and 0xff else 0
            val b2 = if (i + 2 < bytes.size) bytes[i + 2].toInt() and 0xff else 0
            out.append(ALPHABET[b0 shr 2])
            out.append(ALPHABET[(b0 and 3) shl 4 or (b1 shr 4)])
            out.append(if (i + 1 < bytes.size) ALPHABET[(b1 and 15) shl 2 or (b2 shr 6)] else '=')
            out.append(if (i + 2 < bytes.size) ALPHABET[b2 and 63] else '=')
            i += 3
        }
        return out.toString()
    }

    /** The bytes of [text], or null when it isn't base64 (whitespace is ignored). */
    fun decode(text: String): ByteArray? {
        val clean = text.filterNot { it.isWhitespace() }.trimEnd('=')
        if (clean.length % 4 == 1) return null
        val out = ByteArray(clean.length * 3 / 4)
        var buffer = 0
        var bits = 0
        var n = 0
        for (c in clean) {
            val value = if (c.code < 128) VALUES[c.code] else -1
            if (value < 0) return null
            buffer = buffer shl 6 or value
            bits += 6
            if (bits >= 8) {
                bits -= 8
                out[n++] = (buffer shr bits and 0xff).toByte()
            }
        }
        return if (n == out.size) out else out.copyOf(n)
    }
}
