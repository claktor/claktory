package com.claktory.app

import android.os.Process
import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.security.KeyStore

/**
 * The connections' store with the real Android Keystore (DESIGN.md §5),
 * under a key alias and folder of its own so the app's are never touched.
 */
@RunWith(AndroidJUnit4::class)
class KeystoreStoreTest {
    private val context = InstrumentationRegistry.getInstrumentation().targetContext
    private val alias = "claktory.device.instrumentation"
    private lateinit var folder: File
    private lateinit var file: File

    private val seed = "c2VlZC1pbnN0cnVtZW50YXRpb24tc2VjcmV0LXZhbHVl"
    private val token = "INSTRUMENTATION-CAPABILITY-TOKEN"
    private val code = "claktory://add?code=$token&name=Dev"
    private val connections = mapOf(
        "current" to code,
        "saved" to listOf(code),
        "keys" to mapOf(code to "aXJvaC1pbnN0cnVtZW50YXRpb24="),
        "account" to mapOf("key" to seed, "service" to "https://claktory.com", "email" to "friend@example.com"),
    )

    private fun log(message: String) {
        Log.i("Claktory", "connections: $message")
    }

    private fun sealer() = KeystoreSealer(context, alias, ::log)

    private fun store(appId: String = context.packageName) = CredentialStore(file, appId, sealer(), ::log)

    private fun keystore() = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    private fun failure(body: () -> Unit): StoreFailure {
        try {
            body()
        } catch (failure: StoreFailure) {
            return failure
        }
        fail("expected a StoreFailure")
        throw AssertionError()
    }

    @Before
    fun setUp() {
        folder = File(context.noBackupFilesDir, "keystore-store-test").apply { deleteRecursively(); mkdirs() }
        file = File(folder, CredentialStore.FILE_NAME)
        keystore().deleteEntry(alias)
    }

    @After
    fun tearDown() {
        folder.deleteRecursively()
        keystore().deleteEntry(alias)
    }

    @Test
    fun roundTripsWithAKeystoreKeyThatNeverLeavesIt() {
        store().apply { read(); write(connections) }
        assertEquals(connections, store().read())
        val entry = keystore().getEntry(alias, null) as KeyStore.SecretKeyEntry
        // Not exportable: Keystore never hands out the key's bytes.
        assertEquals(null, entry.secretKey.encoded)
    }

    @Test
    fun keystoreChoosesAFreshNonceForEveryWrite() {
        val store = store().apply { read() }
        val nonces = (1..3).map {
            store.write(connections)
            file.readBytes().copyOfRange(7, 19).toList()
        }
        assertEquals(3, nonces.toSet().size)
    }

    @Test
    fun aDeletedKeyIsKeyMissingAndTheFileKept() {
        store().apply { read(); write(connections) }
        val kept = file.readBytes()
        keystore().deleteEntry(alias)
        val store = store()
        assertEquals(StoreFailure.KEY_MISSING, failure { store.read() }.code)
        assertEquals(StoreFailure.READ_FAILED, failure { store.write(emptyMap<String, Any>()) }.code)
        assertArrayEquals(kept, file.readBytes())
        assertFalse(keystore().containsAlias(alias))
    }

    @Test
    fun aFlippedByteOrTheOtherFlavorsFileIsUnreadable() {
        store().apply { read(); write(connections) }
        val bytes = file.readBytes()
        bytes[bytes.size - 3] = (bytes[bytes.size - 3].toInt() xor 1).toByte()
        file.writeBytes(bytes)
        assertEquals(StoreFailure.UNREADABLE, failure { store().read() }.code)
        file.delete()
        store("com.claktory.app.other").apply { read(); write(connections) }
        assertEquals(StoreFailure.UNREADABLE, failure { store().read() }.code)
    }

    @Test
    fun startingOverClearsAndAllowsANewWrite() {
        store().apply { read(); write(connections) }
        assertTrue(keystore().containsAlias(alias))
        val store = store()
        store.reset()
        assertFalse(keystore().containsAlias(alias))
        assertFalse(file.exists())
        assertEquals(emptyMap<String, Any?>(), store.read())
        store.write(connections)
        assertEquals(connections, store().read())
    }

    @Test
    fun nothingKeptReachesLogcatOrAnError() {
        val errors = mutableListOf<StoreFailure>()
        store().apply { read(); write(connections) }
        keystore().deleteEntry(alias)
        errors += failure { store().read() }
        errors += failure { store().apply { runCatching { read() } }.write(connections) }
        val log = Runtime.getRuntime()
            .exec(arrayOf("logcat", "-d", "-v", "raw", "--pid=${Process.myPid()}"))
            .inputStream.bufferedReader().readText()
        assertTrue("the store logs its failures", log.contains("connections: couldn't read them"))
        val said = errors.joinToString("\n") { "$it" } + log
        for (secret in listOf(seed, token, "aXJvaC1pbnN0cnVtZW50YXRpb24=", "friend@example.com")) {
            assertFalse("something kept was said", said.contains(secret))
        }
    }
}
