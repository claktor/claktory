package com.claktory.app

import org.json.JSONObject
import java.io.File

/**
 * What this phone keeps for pushes, per Claktory (its id in notifications,
 * notifications.dart's claktoryId): the 32-byte key that Claktory seals
 * with, the counter and digests of what was shown ([PushKey]), and what
 * the push relay registered ([PushRegistration]). One file in no_backup/
 * (no backup or device transfer takes it), sealed by a Keystore key of its
 * own ([ALIAS]), the connections' file format with its own magic:
 *
 * ```
 * "CLPN"  format u8 = 1  key version u8 = 1  nonce length u8 = 12
 * nonce (12)  ciphertext + GCM tag (16)
 * ```
 *
 * The header and the app's id are the additional data. The keys stay on
 * this phone; the Claktory has its own copy. A file that can't be read
 * reads as nothing kept: every connect registers again with a new key, so
 * losing them costs one generic notification at most, never a connection.
 * Read and written from the app (factory/push) and from the messaging
 * service on its own thread, one at a time.
 */
class PushStore(
    private val file: File,
    private val appId: String,
    private val sealer: Sealer,
    private val log: (String) -> Unit = {},
) {
    private class Entry(val key: PushKey?, val registration: PushRegistration?)

    /** The keys kept, by Claktory. */
    @Synchronized
    fun keys(): Map<String, PushKey> = read().mapNotNull { (claktory, entry) -> entry.key?.let { claktory to it } }.toMap()

    @Synchronized
    fun registration(claktory: String): PushRegistration? = read()[claktory]?.registration

    /** Keeps [key] for [claktory], with its registration as it was; false when it couldn't be written. */
    @Synchronized
    fun saveKey(claktory: String, key: PushKey): Boolean {
        val all = read().toMutableMap()
        all[claktory] = Entry(key, all[claktory]?.registration)
        return write(all)
    }

    /** Keeps [key] and [registration] for [claktory] together; false when they couldn't be written. */
    @Synchronized
    fun save(claktory: String, key: PushKey, registration: PushRegistration): Boolean {
        val all = read().toMutableMap()
        all[claktory] = Entry(key, registration)
        return write(all)
    }

    @Synchronized
    fun remove(claktory: String) {
        val all = read().toMutableMap()
        if (all.remove(claktory) != null) write(all)
    }

    private fun read(): Map<String, Entry> {
        if (!file.exists()) return emptyMap()
        return try {
            val bytes = file.readBytes()
            if (bytes.size < HEADER.size + NONCE_LENGTH + 16 || !bytes.copyOfRange(0, HEADER.size).contentEquals(HEADER)) {
                throw StoreFailure(StoreFailure.UNREADABLE, "the file isn't the push keys'")
            }
            val nonce = bytes.copyOfRange(HEADER.size, HEADER.size + NONCE_LENGTH)
            val plain = sealer.open(nonce, bytes.copyOfRange(HEADER.size + NONCE_LENGTH, bytes.size), aad())
            val json = JSONObject(String(plain, Charsets.UTF_8))
            plain.fill(0)
            json.keys().asSequence().associateWith { claktory ->
                val entry = json.optJSONObject(claktory) ?: JSONObject()
                Entry(
                    entry.optJSONObject("key")?.let(PushKey::from),
                    entry.optJSONObject("registration")?.let(PushRegistration::from),
                )
            }
        } catch (error: Exception) {
            log("push keys unreadable (${if (error is StoreFailure) error.code else error.javaClass.simpleName})")
            emptyMap()
        }
    }

    private fun write(all: Map<String, Entry>): Boolean {
        val json = JSONObject()
        for ((claktory, entry) in all) {
            json.put(
                claktory,
                JSONObject().apply {
                    entry.key?.let { put("key", it.json()) }
                    entry.registration?.let { put("registration", it.json()) }
                },
            )
        }
        val plain = json.toString().toByteArray(Charsets.UTF_8)
        return try {
            val sealed = sealer.seal(plain, aad())
            if (sealed.nonce.size != NONCE_LENGTH) throw StoreFailure(StoreFailure.WRITE_FAILED, "unexpected nonce")
            writeAtomically(file, HEADER + sealed.nonce + sealed.bytes)
            true
        } catch (error: Exception) {
            log("push keys not written (${if (error is StoreFailure) error.code else error.javaClass.simpleName})")
            false
        } finally {
            plain.fill(0)
        }
    }

    private fun aad() = HEADER + appId.toByteArray(Charsets.UTF_8)

    companion object {
        const val FILE_NAME = "push-keys.bin"

        /** The push keys' own Keystore key, apart from the connections' and the DM keys'. */
        const val ALIAS = "claktory.push.v1"

        private val MAGIC = "CLPN".toByteArray(Charsets.US_ASCII)
        private const val NONCE_LENGTH = 12
        private val HEADER = MAGIC + byteArrayOf(1, 1, NONCE_LENGTH.toByte())
    }
}
