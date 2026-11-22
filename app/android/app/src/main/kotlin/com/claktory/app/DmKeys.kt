package com.claktory.app

import android.content.Context
import android.os.Handler
import android.os.Looper
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.io.File
import java.security.SecureRandom

/**
 * Why a DM key couldn't be kept, read or removed: [code] is
 * `factory/dm-keys`' (app/lib/people/dm_keys.dart), [UNREADABLE] only from
 * [DmKeyStore.get]. The message names the kind of failure, never the key.
 */
class DmKeyFailure(val code: String, message: String) : Exception(message) {
    override fun toString() = "$code: $message"

    companion object {
        /** Something is kept under the name but can't be read: never "absent". */
        const val UNREADABLE = "unreadable"
        const val FAILED = "failed"
    }
}

/**
 * The keys of end-to-end direct messages (dms-e2ee.md §5.4, E6): opaque
 * bytes the app's Rust made, kept under a name, one file each in
 * [folder] (no_backup/dm-keys, which no backup or device transfer takes),
 * sealed by a Keystore key of their own ([DeviceStores.DM_KEY_ALIAS]), so
 * starting over on the connections never takes them:
 *
 * ```
 * "CLDM"  format u8 = 1  key version u8 = 1  nonce length u8 = 12
 * nonce (12)  ciphertext + GCM tag (16)
 * ```
 *
 * The header, [appId] and the name are the additional data, so a file
 * moved to another name, or Claktory Dev's read by Claktory, fails. A
 * read ends in the bytes, null when no file was ever completed under the
 * name, or [DmKeyFailure.UNREADABLE] when one is there but can't be read
 * for any reason (its key gone, the file damaged, Keystore refusing),
 * never null for that: absent means "set up again", unreadable must not.
 */
class DmKeyStore(
    private val folder: File,
    private val appId: String,
    private val sealer: Sealer,
    private val log: (String) -> Unit = {},
) {
    /**
     * Whether keys can be kept here at all: a value sealed, written,
     * read back and opened through the real key and folder just now.
     * No fallback: false turns People's messages off on this device.
     */
    @Synchronized
    fun available(): Boolean {
        val probe = ByteArray(VALUE_LIMIT_PROBE).also { SecureRandom().nextBytes(it) }
        val file = File(folder, PROBE_FILE)
        return try {
            writeSealed(file, PROBE_FILE, probe)
            val back = openSealed(file, PROBE_FILE)
            val same = back.contentEquals(probe)
            back.fill(0)
            if (!same) log("can't keep them: what was read back differs")
            same
        } catch (error: Exception) {
            log("can't keep them (${describe(error)})")
            false
        } finally {
            probe.fill(0)
            file.delete()
            File(file.path + CredentialStore.NEW_SUFFIX).delete()
        }
    }

    /** Keeps [bytes] under [name], replacing what was there. */
    @Synchronized
    fun put(name: String, bytes: ByteArray) {
        check(name)
        if (bytes.size > VALUE_LIMIT) {
            throw DmKeyFailure(DmKeyFailure.FAILED, "The value is longer than a key can be.")
        }
        try {
            writeSealed(file(name), name, bytes)
        } catch (error: Exception) {
            log("couldn't keep $name (${describe(error)})")
            throw DmKeyFailure(DmKeyFailure.FAILED, "It couldn't be kept (${describe(error)}).")
        }
    }

    /** What's kept under [name], or null when nothing is. */
    @Synchronized
    fun get(name: String): ByteArray? {
        check(name)
        val file = file(name)
        if (!file.exists()) return null
        return try {
            openSealed(file, name)
        } catch (error: Exception) {
            log("couldn't read $name (${describe(error)})")
            throw DmKeyFailure(DmKeyFailure.UNREADABLE, "It's kept but can't be read (${describe(error)}).")
        }
    }

    /** Forgets [name]; nothing kept is fine. The key stays for the other names. */
    @Synchronized
    fun delete(name: String) {
        check(name)
        val file = file(name)
        File(file.path + CredentialStore.NEW_SUFFIX).delete()
        if (file.exists() && !file.delete()) {
            log("couldn't forget $name")
            throw DmKeyFailure(DmKeyFailure.FAILED, "The file couldn't be removed.")
        }
    }

    private fun file(name: String) = File(folder, name + SUFFIX)

    private fun check(name: String) {
        if (!NAME.matches(name)) throw DmKeyFailure(DmKeyFailure.FAILED, "That isn't a key's name.")
    }

    private fun writeSealed(file: File, name: String, plain: ByteArray) {
        val sealed = sealer.seal(plain, aad(name))
        if (sealed.nonce.size != NONCE_LENGTH) {
            throw StoreFailure(StoreFailure.WRITE_FAILED, "The key chose a nonce of an unexpected length.")
        }
        writeAtomically(file, HEADER + sealed.nonce + sealed.bytes)
    }

    private fun openSealed(file: File, name: String): ByteArray {
        val bytes = file.readBytes()
        if (bytes.size < HEADER.size + NONCE_LENGTH + TAG_LENGTH) {
            throw StoreFailure(StoreFailure.UNREADABLE, "the file is too short")
        }
        val header = bytes.copyOfRange(0, HEADER.size)
        if (!header.copyOfRange(0, MAGIC.size).contentEquals(MAGIC)) {
            throw StoreFailure(StoreFailure.UNREADABLE, "the file isn't a key's")
        }
        if (!header.contentEquals(HEADER)) {
            // A newer format or key version is as unreadable here as damage.
            throw StoreFailure(StoreFailure.NEWER_VERSION, "the file's format is one this version doesn't read")
        }
        if (!sealer.hasKey()) {
            throw StoreFailure(StoreFailure.KEY_MISSING, "the key that protects it is gone")
        }
        val nonce = bytes.copyOfRange(HEADER.size, HEADER.size + NONCE_LENGTH)
        val sealed = bytes.copyOfRange(HEADER.size + NONCE_LENGTH, bytes.size)
        return sealer.open(nonce, sealed, aad(name))
    }

    private fun aad(name: String) =
        HEADER + appId.toByteArray(Charsets.UTF_8) + byteArrayOf(0) + name.toByteArray(Charsets.US_ASCII)

    /** A failure's kind for the log and messages: a store code or a class name, never content. */
    private fun describe(error: Exception) = if (error is StoreFailure) error.code else error.javaClass.simpleName

    companion object {
        val MAGIC = "CLDM".toByteArray(Charsets.US_ASCII)
        const val FORMAT = 1
        const val KEY_VERSION = 1
        const val NONCE_LENGTH = 12
        const val TAG_LENGTH = 16
        val HEADER = MAGIC + byteArrayOf(FORMAT.toByte(), KEY_VERSION.toByte(), NONCE_LENGTH.toByte())
        const val FOLDER = "dm-keys"
        const val SUFFIX = ".key"

        /** Keys are 32 bytes; anything far longer isn't one. */
        const val VALUE_LIMIT = 4096
        private const val VALUE_LIMIT_PROBE = 32

        /** Not a name a key can have (they have no dots), so the probe never meets one. */
        const val PROBE_FILE = "available.probe"

        /** dm_keys.dart's DmKeyName: lowercase ASCII, at most 32 characters. */
        private val NAME = Regex("[a-z0-9_-]{1,32}")
    }
}

/**
 * factory/dm-keys (app/lib/people/dm_keys.dart): [DmKeyStore]'s calls on
 * the stores' thread, one at a time, answered on the main one.
 */
class DmKeysChannel(
    messenger: BinaryMessenger,
    private val store: () -> DmKeyStore,
    private val storeFolder: () -> File,
) : MethodChannel.MethodCallHandler, AutoCloseable {
    constructor(messenger: BinaryMessenger, context: Context) :
        this(messenger, { DeviceStores.dmKeys(context) }, { File(context.noBackupFilesDir, STORE_FOLDER) })

    private val channel = MethodChannel(messenger, "factory/dm-keys")
    private val main = Handler(Looper.getMainLooper())

    init {
        channel.setMethodCallHandler(this)
    }

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            "available" -> stored(result) { store().available() }
            "put" -> stored(result) {
                val bytes = call.argument<ByteArray>("bytes")
                    ?: throw DmKeyFailure(DmKeyFailure.FAILED, "There are no bytes to keep.")
                store().put(name(call), bytes)
                null
            }
            "get" -> stored(result) { store().get(name(call)) }
            "delete" -> stored(result) {
                store().delete(name(call))
                null
            }
            "folder" -> stored(result) { madeFolder(storeFolder()) }
            else -> result.notImplemented()
        }
    }

    override fun close() = channel.setMethodCallHandler(null)

    companion object {
        /** The DM store's folder (app/rust dm/store.rs), beside the keys in no_backup/. */
        const val STORE_FOLDER = "dm"

        /**
         * [folder]'s absolute path, made when missing (only this app can
         * reach it, as everything in its private storage); `failed` when
         * there's no folder there afterwards.
         */
        fun madeFolder(folder: File): String {
            folder.mkdirs()
            if (!folder.isDirectory) throw DmKeyFailure(DmKeyFailure.FAILED, "The folder for the messages couldn't be made.")
            return folder.absolutePath
        }
    }

    private fun name(call: MethodCall): String =
        call.argument<String>("name") ?: throw DmKeyFailure(DmKeyFailure.FAILED, "That isn't a key's name.")

    private fun stored(result: MethodChannel.Result, body: () -> Any?) {
        DeviceStores.thread.execute {
            val answer = try {
                Result.success(body())
            } catch (error: Throwable) {
                Result.failure(error)
            }
            main.post {
                answer.fold(
                    onSuccess = { result.success(it) },
                    onFailure = { error ->
                        if (error is DmKeyFailure) result.error(error.code, error.message, null)
                        else result.error(DmKeyFailure.FAILED, error.javaClass.simpleName, null)
                    },
                )
            }
        }
    }
}
