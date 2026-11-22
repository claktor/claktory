package com.claktory.app

import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject
import org.json.JSONTokener
import java.io.File
import java.io.FileOutputStream
import java.io.IOException

/**
 * Why the connections couldn't be read, written or removed: [code] is
 * `factory/device`'s name for it, the same on every platform
 * (ConnectionStorageFailure in app/lib/connections.dart, StoreFailure in
 * app/macos/Runner/ConnectionStore.swift). The message says only what
 * kind of failure it was, never anything that was kept (DESIGN.md §5).
 */
class StoreFailure(val code: String, message: String) : Exception(message) {
    override fun toString() = "$code: $message"

    companion object {
        const val UNAVAILABLE = "credentials_unavailable"
        const val UNREADABLE = "credentials_unreadable"
        const val KEY_MISSING = "credentials_key_missing"
        const val KEY_INVALIDATED = "credentials_key_invalidated"
        const val NEWER_VERSION = "credentials_newer_version"
        const val READ_FAILED = "credentials_read_failed"
        const val WRITE_FAILED = "credentials_write_failed"
        const val RESET_FAILED = "credentials_reset_failed"
    }
}

/** What [Sealer.seal] made: the nonce the key chose, and the ciphertext with its tag. */
class Sealed(val nonce: ByteArray, val bytes: ByteArray)

/**
 * The key that protects the file: AES-256-GCM in Android Keystore
 * ([KeystoreSealer]), a software key in tests. Every failure is a
 * [StoreFailure] with its code.
 */
interface Sealer {
    /** Whether the key is there. */
    fun hasKey(): Boolean

    /** [plain] sealed with [aad], under a fresh nonce; makes the key when there's none. */
    fun seal(plain: ByteArray, aad: ByteArray): Sealed

    /** What [seal] sealed; credentials_unreadable when it isn't authentic. */
    fun open(nonce: ByteArray, sealed: ByteArray, aad: ByteArray): ByteArray

    /** Removes the key; none there is no failure. */
    fun deleteKey()
}

/**
 * This phone's connections (app/lib/connections.dart: `{current, saved,
 * keys, account}`, every value as the app sent it), in one file sealed by
 * a [Sealer] (DESIGN.md §5):
 *
 * ```
 * "CLKD"  format u8 = 1  key version u8 = 1  nonce length u8 = 12
 * nonce (12)  ciphertext + GCM tag (16)
 * ```
 *
 * The header and [appId] are the additional data, so a changed header or
 * Claktory Dev's file read by Claktory fails. Every read ends in one state:
 * what's kept, nothing (no file yet), or a [StoreFailure] whose code says
 * why, never an empty map in its place. After a failed read nothing is
 * written until the person starts over ([reset]), so a file that couldn't
 * be read is never written over with nothing. The store never makes an
 * identity of its own.
 */
class CredentialStore(
    private val file: File,
    private val appId: String,
    private val sealer: Sealer,
    private val log: (String) -> Unit = {},
) {
    /**
     * Whether the last read worked (or the person started over): writing
     * is allowed only then. Null before the first read.
     */
    private var readable: Boolean? = null

    /** What's kept; empty when nothing ever was. */
    @Synchronized
    fun read(): Map<String, Any?> {
        readable = false
        val kept = try {
            readKept()
        } catch (failure: StoreFailure) {
            log("couldn't read them (${failure.code})")
            throw failure
        }
        readable = true
        return kept
    }

    /**
     * Keeps [connections]; refused (credentials_read_failed) after a read
     * that failed, until one works: what the app holds then isn't what's
     * kept, and writing it would put nothing over the person's identity.
     */
    @Synchronized
    fun write(connections: Map<*, *>) {
        if (readable == null) {
            // The app always reads first; a write before any read reads now.
            try {
                read()
            } catch (failure: StoreFailure) {
                // Refused below.
            }
        }
        if (readable != true) {
            throw StoreFailure(
                StoreFailure.READ_FAILED,
                "They couldn't be read, so they aren't written over.",
            )
        }
        val plain = encode(connections)
        try {
            val sealed = sealer.seal(plain, aad())
            if (sealed.nonce.size != NONCE_LENGTH) {
                throw StoreFailure(StoreFailure.WRITE_FAILED, "The key chose a nonce of an unexpected length.")
            }
            writeAtomically(file, header() + sealed.nonce + sealed.bytes)
        } catch (failure: StoreFailure) {
            log("couldn't write them (${failure.code})")
            throw if (failure.code == StoreFailure.WRITE_FAILED) failure
            else StoreFailure(StoreFailure.WRITE_FAILED, "The key couldn't seal them (${failure.code}).")
        } catch (error: IOException) {
            log("couldn't write them (${error.javaClass.simpleName})")
            throw StoreFailure(StoreFailure.WRITE_FAILED, "The file couldn't be written (${error.javaClass.simpleName}).")
        } finally {
            plain.fill(0)
        }
    }

    /**
     * Starts over, at the person's word: the file moves aside (one copy,
     * `.unreadable`, replaced by the next), the key goes, and reads find
     * nothing until something is written. Clearing this phone's copy
     * isn't revoking it: the person forgets the device on their Claktory.
     */
    @Synchronized
    fun reset() {
        try {
            File(file.path + NEW_SUFFIX).delete()
            if (file.exists()) {
                val aside = File(file.path + UNREADABLE_SUFFIX)
                aside.delete()
                if (!file.renameTo(aside)) throw IOException("couldn't move the file aside")
            }
            sealer.deleteKey()
        } catch (failure: StoreFailure) {
            log("couldn't start over (${failure.code})")
            throw StoreFailure(StoreFailure.RESET_FAILED, "The key couldn't be removed (${failure.code}).")
        } catch (error: Exception) {
            log("couldn't start over (${error.javaClass.simpleName})")
            throw StoreFailure(StoreFailure.RESET_FAILED, "They couldn't be removed (${error.javaClass.simpleName}).")
        }
        readable = true
        log("started over")
    }

    private fun readKept(): Map<String, Any?> {
        if (!file.exists()) return emptyMap()
        val bytes = try {
            file.readBytes()
        } catch (error: IOException) {
            throw StoreFailure(StoreFailure.UNAVAILABLE, "The file couldn't be read (${error.javaClass.simpleName}).")
        }
        if (bytes.size < MAGIC.size + 1 || !bytes.copyOfRange(0, MAGIC.size).contentEquals(MAGIC)) {
            throw StoreFailure(StoreFailure.UNREADABLE, "What's kept isn't a connections file.")
        }
        val format = bytes[MAGIC.size].toInt() and 0xff
        if (format > FORMAT) {
            throw StoreFailure(StoreFailure.NEWER_VERSION, "A newer version of Claktory wrote them.")
        }
        if (format != FORMAT || bytes.size < HEADER_LENGTH + NONCE_LENGTH + TAG_LENGTH) {
            throw StoreFailure(StoreFailure.UNREADABLE, "What's kept isn't a connections file.")
        }
        val keyVersion = bytes[MAGIC.size + 1].toInt() and 0xff
        if (keyVersion > KEY_VERSION) {
            throw StoreFailure(StoreFailure.NEWER_VERSION, "A newer version of Claktory wrote them.")
        }
        if (keyVersion != KEY_VERSION || (bytes[MAGIC.size + 2].toInt() and 0xff) != NONCE_LENGTH) {
            throw StoreFailure(StoreFailure.UNREADABLE, "What's kept isn't a connections file.")
        }
        if (!sealer.hasKey()) {
            throw StoreFailure(StoreFailure.KEY_MISSING, "The key that protects them is gone from this phone.")
        }
        val nonce = bytes.copyOfRange(HEADER_LENGTH, HEADER_LENGTH + NONCE_LENGTH)
        val sealed = bytes.copyOfRange(HEADER_LENGTH + NONCE_LENGTH, bytes.size)
        val plain = sealer.open(nonce, sealed, aad())
        try {
            return decode(plain)
        } finally {
            plain.fill(0)
        }
    }

    private fun header() = MAGIC + byteArrayOf(FORMAT.toByte(), KEY_VERSION.toByte(), NONCE_LENGTH.toByte())

    private fun aad() = header() + appId.toByteArray(Charsets.UTF_8)

    companion object {
        val MAGIC = "CLKD".toByteArray(Charsets.US_ASCII)
        const val FORMAT = 1
        const val KEY_VERSION = 1
        const val NONCE_LENGTH = 12
        const val TAG_LENGTH = 16
        const val HEADER_LENGTH = 7
        const val SCHEMA = 1
        const val FILE_NAME = "connections.v1"
        const val NEW_SUFFIX = ".new"
        const val UNREADABLE_SUFFIX = ".unreadable"

        /** The map as UTF-8 JSON, with its schema. */
        fun encode(connections: Map<*, *>): ByteArray {
            val json = JSONObject()
            for ((key, value) in connections) {
                if (key is String && key != "schema") json.put(key, toJson(value))
            }
            json.put("schema", SCHEMA)
            return json.toString().toByteArray(Charsets.UTF_8)
        }

        /** What [encode] wrote, without its schema. */
        fun decode(plain: ByteArray): Map<String, Any?> {
            val json = try {
                JSONTokener(String(plain, Charsets.UTF_8)).nextValue() as? JSONObject
            } catch (error: JSONException) {
                null
            } ?: throw StoreFailure(StoreFailure.UNREADABLE, "What's kept isn't readable as connections.")
            val schema = json.opt("schema")
            if (schema !is Int) {
                throw StoreFailure(StoreFailure.UNREADABLE, "What's kept isn't readable as connections.")
            }
            if (schema > SCHEMA) {
                throw StoreFailure(StoreFailure.NEWER_VERSION, "A newer version of Claktory wrote them.")
            }
            json.remove("schema")
            @Suppress("UNCHECKED_CAST")
            return fromJson(json) as Map<String, Any?>
        }

        private fun toJson(value: Any?): Any = when (value) {
            null -> JSONObject.NULL
            is Map<*, *> -> JSONObject().also { json ->
                for ((key, item) in value) if (key is String) json.put(key, toJson(item))
            }
            is List<*> -> JSONArray().also { json -> value.forEach { json.put(toJson(it)) } }
            is String, is Boolean, is Int, is Long, is Double -> value
            else -> throw StoreFailure(StoreFailure.WRITE_FAILED, "They hold a value of a kind that can't be kept.")
        }

        private fun fromJson(value: Any?): Any? = when (value) {
            null, JSONObject.NULL -> null
            is JSONObject -> HashMap<String, Any?>().also { map ->
                for (key in value.keys()) map[key] = fromJson(value.get(key))
            }
            is JSONArray -> ArrayList<Any?>().also { list ->
                for (index in 0 until value.length()) list.add(fromJson(value.get(index)))
            }
            // The reference org.json (the unit tests') reads decimals as
            // BigDecimal; Android's as Double, as Dart sends them.
            is java.math.BigDecimal -> value.toDouble()
            is java.math.BigInteger -> value.toLong()
            else -> value
        }
    }
}

/**
 * [bytes] in place of [file], all or nothing: written beside it, synced,
 * then renamed over it, so the last good file stays until the new one is
 * complete.
 */
fun writeAtomically(file: File, bytes: ByteArray) {
    file.parentFile?.mkdirs()
    val next = File(file.path + CredentialStore.NEW_SUFFIX)
    FileOutputStream(next).use { out ->
        out.write(bytes)
        out.fd.sync()
    }
    if (!next.renameTo(file)) {
        next.delete()
        throw IOException("couldn't replace the file")
    }
}
