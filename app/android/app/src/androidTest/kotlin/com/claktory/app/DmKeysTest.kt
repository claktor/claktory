package com.claktory.app

import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import io.flutter.plugin.common.StandardMethodCodec
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.security.KeyStore
import java.security.SecureRandom
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * factory/dm-keys with the real Android Keystore (dms-e2ee.md §5.4, E6):
 * the store under a key alias and folder of its own, and the running
 * app's channel, through the codec Dart's calls come in by.
 */
@RunWith(AndroidJUnit4::class)
class DmKeysTest {
    private val context = InstrumentationRegistry.getInstrumentation().targetContext
    private val alias = "claktory.dm.instrumentation"
    private val connectionsAlias = "claktory.device.dm-instrumentation"
    private lateinit var folder: File

    private val identity = ByteArray(32).also { SecureRandom().nextBytes(it) }
    private val leaf = ByteArray(32).also { SecureRandom().nextBytes(it) }

    private fun log(message: String) {
        Log.i("Claktory", "dm keys: $message")
    }

    private fun store(appId: String = context.packageName) =
        DmKeyStore(File(folder, DmKeyStore.FOLDER), appId, KeystoreSealer(context, alias, ::log), ::log)

    private fun keystore() = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    private fun failure(body: () -> Unit): DmKeyFailure {
        try {
            body()
        } catch (failure: DmKeyFailure) {
            return failure
        }
        fail("expected a DmKeyFailure")
        throw AssertionError()
    }

    private fun contains(haystack: ByteArray, needle: ByteArray) =
        (0..haystack.size - needle.size).any { start -> needle.indices.all { haystack[start + it] == needle[it] } }

    @Before
    fun setUp() {
        folder = File(context.noBackupFilesDir, "dm-keys-instrumentation")
        folder.deleteRecursively()
        clean()
    }

    @After
    fun tearDown() {
        folder.deleteRecursively()
        clean()
    }

    private fun clean() {
        val store = keystore()
        for (name in listOf(alias, connectionsAlias)) if (store.containsAlias(name)) store.deleteEntry(name)
    }

    @Test
    fun keysAreSealedByAKeystoreKeyThatNeverLeavesIt() {
        val store = store()
        assertTrue(store.available())
        assertTrue(keystore().containsAlias(alias))
        // A Keystore key's material can't be read out of it.
        assertNull((keystore().getKey(alias, null) as javax.crypto.SecretKey).encoded)

        assertNull(store.get("identity"))
        store.put("identity", identity)
        store.put("leaf", leaf)
        assertArrayEquals(identity, store().get("identity"))
        assertArrayEquals(leaf, store().get("leaf"))
        val file = File(folder, "${DmKeyStore.FOLDER}/identity.key")
        assertFalse(contains(file.readBytes(), identity))

        store.delete("leaf")
        assertNull(store.get("leaf"))
        assertArrayEquals(identity, store.get("identity"))
    }

    @Test
    fun theKeyGoneIsUnreadableNeverAbsent() {
        store().put("identity", identity)
        keystore().deleteEntry(alias)
        assertEquals(DmKeyFailure.UNREADABLE, failure { store().get("identity") }.code)
        // A later put makes a new key; the old file stays unreadable.
        store().put("leaf", leaf)
        assertEquals(DmKeyFailure.UNREADABLE, failure { store().get("identity") }.code)
        assertArrayEquals(leaf, store().get("leaf"))
    }

    @Test
    fun anotherAppsFileAndAMovedFileAreUnreadable() {
        store("com.claktory.app").put("identity", identity)
        assertEquals(DmKeyFailure.UNREADABLE, failure { store("com.claktory.app.dev").get("identity") }.code)
        val keys = File(folder, DmKeyStore.FOLDER)
        File(keys, "identity.key").copyTo(File(keys, "store.key"))
        assertEquals(DmKeyFailure.UNREADABLE, failure { store("com.claktory.app").get("store") }.code)
    }

    @Test
    fun startingOverOnTheConnectionsLeavesTheDmKeys() {
        assertNotEquals(KeystoreSealer.ALIAS, DeviceStores.DM_KEY_ALIAS)
        val connections = CredentialStore(
            File(folder, CredentialStore.FILE_NAME),
            context.packageName,
            KeystoreSealer(context, connectionsAlias, ::log),
            ::log,
        )
        connections.read()
        connections.write(mapOf("current" to "claktory://add?code=DM-INSTRUMENTATION"))
        store().put("identity", identity)
        connections.reset()
        assertFalse(keystore().containsAlias(connectionsAlias))
        assertArrayEquals(identity, store().get("identity"))
    }

    /** What the channel answered: a value, or "error CODE", or "notImplemented". */
    private fun call(channel: DmKeysChannel, method: String, arguments: Any? = null): Any? {
        // Through the codec, as Dart's Uint8List and String arrive.
        val codec = StandardMethodCodec.INSTANCE
        val encoded = codec.encodeMethodCall(MethodCall(method, arguments))
        encoded.flip()
        val decoded = codec.decodeMethodCall(encoded)
        val done = CountDownLatch(1)
        var answer: Any? = null
        val result = object : MethodChannel.Result {
            override fun success(result: Any?) {
                answer = result
                done.countDown()
            }

            override fun error(errorCode: String, errorMessage: String?, errorDetails: Any?) {
                answer = "error $errorCode"
                done.countDown()
            }

            override fun notImplemented() {
                answer = "notImplemented"
                done.countDown()
            }
        }
        InstrumentationRegistry.getInstrumentation().runOnMainSync { channel.onMethodCall(decoded, result) }
        assertTrue("$method answered", done.await(10, TimeUnit.SECONDS))
        return answer
    }

    @Test
    fun theAppAnswersOnFactoryDmKeys() {
        val name = "instrumentation"
        val kept = File(context.noBackupFilesDir, "${DmKeyStore.FOLDER}/$name.key")
        Screen.launch().use { screen ->
            var channel: DmKeysChannel? = null
            screen.onActivity { channel = it.adapter(DmKeysChannel::class.java) }
            val keys = channel ?: throw AssertionError("factory/dm-keys isn't registered")
            try {
                assertEquals(true, call(keys, "available"))
                assertTrue(keystore().containsAlias(DeviceStores.DM_KEY_ALIAS))
                assertNull(call(keys, "get", mapOf("name" to name)))
                assertNull(call(keys, "put", mapOf("name" to name, "bytes" to identity)))
                assertTrue(kept.exists())
                assertArrayEquals(identity, call(keys, "get", mapOf("name" to name)) as ByteArray)
                assertNull(call(keys, "delete", mapOf("name" to name)))
                assertFalse(kept.exists())
                assertNull(call(keys, "get", mapOf("name" to name)))
                assertNull(call(keys, "delete", mapOf("name" to name)))

                assertEquals("error failed", call(keys, "get", mapOf("name" to "../$name")))
                assertEquals("error failed", call(keys, "put", mapOf("name" to name)))
                assertEquals("error failed", call(keys, "get", emptyMap<String, Any>()))
                assertEquals("notImplemented", call(keys, "list"))

                // The DM store's folder: no_backup/dm, the same each time.
                val dm = File(context.noBackupFilesDir, DmKeysChannel.STORE_FOLDER)
                assertEquals(dm.absolutePath, call(keys, "folder"))
                assertTrue(dm.isDirectory)
                assertEquals(dm.absolutePath, call(keys, "folder"))

                // Damaged, it answers unreadable, never null.
                assertNull(call(keys, "put", mapOf("name" to name, "bytes" to identity)))
                kept.writeBytes(kept.readBytes().also { it[it.size - 1] = (it[it.size - 1].toInt() xor 1).toByte() })
                assertEquals("error unreadable", call(keys, "get", mapOf("name" to name)))
            } finally {
                kept.delete()
            }
        }
    }
}
