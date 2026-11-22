package com.claktory.app

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.security.SecureRandom
import javax.crypto.AEADBadTagException
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** A software AES-GCM key standing in for Keystore's, with its failures on demand. */
class SoftwareSealer : Sealer {
    var key: SecretKey? = null
    var fails: StoreFailure? = null
    val nonces = mutableListOf<ByteArray>()

    override fun hasKey(): Boolean {
        fails?.let { throw it }
        return key != null
    }

    override fun seal(plain: ByteArray, aad: ByteArray): Sealed {
        fails?.let { throw it }
        val key = key ?: KeyGenerator.getInstance("AES").apply { init(256) }.generateKey().also { key = it }
        val nonce = ByteArray(12).also { SecureRandom().nextBytes(it) }
        nonces += nonce
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key, GCMParameterSpec(128, nonce))
        cipher.updateAAD(aad)
        return Sealed(nonce, cipher.doFinal(plain))
    }

    override fun open(nonce: ByteArray, sealed: ByteArray, aad: ByteArray): ByteArray {
        fails?.let { throw it }
        val key = key ?: throw StoreFailure(StoreFailure.KEY_MISSING, "gone")
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, nonce))
        cipher.updateAAD(aad)
        return try {
            cipher.doFinal(sealed)
        } catch (error: AEADBadTagException) {
            throw StoreFailure(StoreFailure.UNREADABLE, "doesn't authenticate")
        }
    }

    override fun deleteKey() {
        fails?.let { throw it }
        key = null
    }
}

class CredentialStoreTest {
    @get:Rule val folder = TemporaryFolder()

    private lateinit var file: File
    private lateinit var sealer: SoftwareSealer
    private val logs = mutableListOf<String>()

    // What app/lib/connections.dart sends: pairing codes, an iroh key for
    // each, the account map (DeviceAccount.toKept) with its raw seed.
    private val seed = "c2VlZC1zZWNyZXQtYWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4"
    private val code = "claktory://add?code=SECRET-CAPABILITY-TOKEN&name=Dev"
    private val connections = mapOf(
        "current" to code,
        "saved" to listOf(code, "claktory://add?code=OTHER"),
        "keys" to mapOf(code to "aXJvaC1rZXktc2VjcmV0"),
        "account" to mapOf("key" to seed, "service" to "https://claktory.com", "email" to "matt@example.com"),
    )

    @Before
    fun setUp() {
        file = File(folder.root, CredentialStore.FILE_NAME)
        sealer = SoftwareSealer()
    }

    private fun store(appId: String = "com.claktory.app") = CredentialStore(file, appId, sealer) { logs += it }

    private fun failure(body: () -> Unit): StoreFailure {
        try {
            body()
        } catch (failure: StoreFailure) {
            return failure
        }
        fail("expected a StoreFailure")
        throw AssertionError()
    }

    /** No seed, capability token or pairing code in any log or error. */
    private fun assertNothingKeptSaid(vararg errors: StoreFailure) {
        val said = logs + errors.map { "${it.code} ${it.message} $it" }
        for (secret in listOf(seed, "SECRET-CAPABILITY-TOKEN", "aXJvaC1rZXktc2VjcmV0", "matt@example.com")) {
            for (line in said) assertFalse("'$line' says something kept", line.contains(secret))
        }
    }

    @Test
    fun freshReadsNothingAndMakesNoKey() {
        assertEquals(emptyMap<String, Any?>(), store().read())
        assertFalse(file.exists())
        assertEquals(null, sealer.key)
    }

    @Test
    fun roundTripsTheWholeMapAcrossARelaunch() {
        val first = store()
        first.read()
        first.write(connections)
        assertEquals(connections, store().read())
    }

    @Test
    fun theFileIsTheDesignedEnvelopeAndHoldsNothingInTheClear() {
        store().apply { read(); write(connections) }
        val bytes = file.readBytes()
        assertArrayEquals("CLKD".toByteArray(), bytes.copyOfRange(0, 4))
        assertEquals(1, bytes[4].toInt())
        assertEquals(1, bytes[5].toInt())
        assertEquals(12, bytes[6].toInt())
        assertArrayEquals(sealer.nonces.last(), bytes.copyOfRange(7, 19))
        val text = String(bytes, Charsets.ISO_8859_1)
        for (secret in listOf(seed, "SECRET-CAPABILITY-TOKEN", "matt@example.com", "schema")) {
            assertFalse(text.contains(secret))
        }
        assertFalse(File(file.path + ".new").exists())
    }

    @Test
    fun everyWriteHasAFreshNonce() {
        store().apply { read(); write(connections); write(connections) }
        assertEquals(2, sealer.nonces.size)
        assertFalse(sealer.nonces[0].contentEquals(sealer.nonces[1]))
    }

    @Test
    fun aMissingKeyIsSaidAndTheFileKept() {
        store().apply { read(); write(connections) }
        val kept = file.readBytes()
        sealer.key = null
        val store = store()
        val error = failure { store.read() }
        assertEquals(StoreFailure.KEY_MISSING, error.code)
        assertArrayEquals(kept, file.readBytes())
        assertNothingKeptSaid(error)
    }

    @Test
    fun anInvalidatedKeyIsSaid() {
        store().apply { read(); write(connections) }
        sealer.fails = StoreFailure(StoreFailure.KEY_INVALIDATED, "invalidated")
        assertEquals(StoreFailure.KEY_INVALIDATED, failure { store().read() }.code)
        assertTrue(file.exists())
    }

    @Test
    fun aFlippedByteIsUnreadable() {
        store().apply { read(); write(connections) }
        for (at in listOf(10, 30, file.length().toInt() - 1)) {
            val bytes = file.readBytes()
            val good = bytes.copyOf()
            bytes[at] = (bytes[at].toInt() xor 1).toByte()
            file.writeBytes(bytes)
            val error = failure { store().read() }
            assertEquals("byte $at", StoreFailure.UNREADABLE, error.code)
            assertArrayEquals(bytes, file.readBytes())
            file.writeBytes(good)
        }
    }

    @Test
    fun aChangedHeaderIsRefused() {
        store().apply { read(); write(connections) }
        val bytes = file.readBytes()
        bytes[0] = 'c'.code.toByte()
        file.writeBytes(bytes)
        assertEquals(StoreFailure.UNREADABLE, failure { store().read() }.code)
    }

    @Test
    fun theOtherFlavorsFileIsUnreadable() {
        store("com.claktory.app.dev").apply { read(); write(connections) }
        assertEquals(StoreFailure.UNREADABLE, failure { store("com.claktory.app").read() }.code)
    }

    @Test
    fun truncatedOrForeignFilesAreUnreadable() {
        for (bytes in listOf(ByteArray(0), "CLKD".toByteArray(), "CLKD".toByteArray() + byteArrayOf(1, 1, 12, 0), "{\"current\":\"x\"}".toByteArray())) {
            file.writeBytes(bytes)
            assertEquals(StoreFailure.UNREADABLE, failure { store().read() }.code)
        }
    }

    @Test
    fun aNewerFormatOrKeyVersionIsLeftForTheNewerApp() {
        store().apply { read(); write(connections) }
        for (at in listOf(4, 5)) {
            val bytes = file.readBytes()
            val good = bytes.copyOf()
            bytes[at] = 2
            file.writeBytes(bytes)
            val store = store()
            assertEquals(StoreFailure.NEWER_VERSION, failure { store.read() }.code)
            assertEquals(StoreFailure.READ_FAILED, failure { store.write(connections) }.code)
            assertArrayEquals(bytes, file.readBytes())
            file.writeBytes(good)
        }
    }

    @Test
    fun aNewerSchemaIsLeftForTheNewerApp() {
        assertEquals(StoreFailure.NEWER_VERSION, failure { CredentialStore.decode("{\"schema\":2}".toByteArray()) }.code)
        assertEquals(StoreFailure.UNREADABLE, failure { CredentialStore.decode("{\"current\":\"x\"}".toByteArray()) }.code)
        assertEquals(StoreFailure.UNREADABLE, failure { CredentialStore.decode("[1]".toByteArray()) }.code)
        assertEquals(StoreFailure.UNREADABLE, failure { CredentialStore.decode("{\"sch".toByteArray()) }.code)
    }

    @Test
    fun keystoreTroubleIsUnavailableAndBlocksWrites() {
        store().apply { read(); write(connections) }
        val kept = file.readBytes()
        sealer.fails = StoreFailure(StoreFailure.UNAVAILABLE, "busy")
        val store = store()
        assertEquals(StoreFailure.UNAVAILABLE, failure { store.read() }.code)
        sealer.fails = null
        // Still refused: only a read that works (or starting over) lets writes in.
        assertEquals(StoreFailure.READ_FAILED, failure { store.write(emptyMap<String, Any>()) }.code)
        assertArrayEquals(kept, file.readBytes())
        assertEquals(connections, store.read())
        store.write(mapOf("current" to "", "saved" to emptyList<String>()))
        assertFalse(kept.contentEquals(file.readBytes()))
    }

    @Test
    fun aWriteAfterAFailedReadIsRefusedAndTouchesNothing() {
        store().apply { read(); write(connections) }
        sealer.key = null
        val store = store()
        failure { store.read() }
        val kept = file.readBytes()
        val error = failure { store.write(emptyMap<String, Any>()) }
        assertEquals(StoreFailure.READ_FAILED, error.code)
        assertArrayEquals(kept, file.readBytes())
        assertEquals(null, sealer.key)
    }

    @Test
    fun aWriteBeforeAnyReadReadsFirst() {
        store().apply { read(); write(connections) }
        sealer.key = null
        val error = failure { store().write(emptyMap<String, Any>()) }
        assertEquals(StoreFailure.READ_FAILED, error.code)
        assertEquals(null, sealer.key)
    }

    @Test
    fun startingOverMovesTheFileAsideRemovesTheKeyAndLetsWritesIn() {
        store().apply { read(); write(connections) }
        sealer.key = null
        val store = store()
        failure { store.read() }
        val kept = file.readBytes()
        store.reset()
        assertFalse(file.exists())
        assertArrayEquals(kept, File(file.path + ".unreadable").readBytes())
        assertEquals(emptyMap<String, Any?>(), store.read())
        store.write(connections)
        assertEquals(connections, store().read())
        // A second start over replaces the one copy kept aside.
        store.reset()
        assertFalse(kept.contentEquals(File(file.path + ".unreadable").readBytes()))
        assertEquals(null, sealer.key)
    }

    @Test
    fun aResetTheKeystoreRefusesIsSaid() {
        store().apply { read(); write(connections) }
        sealer.fails = StoreFailure(StoreFailure.UNAVAILABLE, "busy")
        assertEquals(StoreFailure.RESET_FAILED, failure { store().reset() }.code)
    }

    @Test
    fun aWriteTheKeystoreRefusesIsSaidAndKeepsTheLastFile() {
        val store = store()
        store.read()
        store.write(connections)
        val kept = file.readBytes()
        sealer.fails = StoreFailure(StoreFailure.UNAVAILABLE, "busy")
        val error = failure { store.write(mapOf("current" to "")) }
        assertEquals(StoreFailure.WRITE_FAILED, error.code)
        assertArrayEquals(kept, file.readBytes())
    }

    @Test
    fun anUnwritableFolderIsAWriteFailure() {
        file = File(folder.newFile("not-a-folder"), CredentialStore.FILE_NAME)
        val store = store()
        store.read()
        assertEquals(StoreFailure.WRITE_FAILED, failure { store.write(connections) }.code)
    }

    @Test
    fun nothingKeptIsEverLoggedOrInAnError() {
        val errors = mutableListOf<StoreFailure>()
        store().apply { read(); write(connections) }
        sealer.key = null
        errors += failure { store().read() }
        errors += failure { store().write(connections) }
        file.writeBytes(file.readBytes().also { it[20] = (it[20].toInt() xor 1).toByte() })
        sealer.key = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
        errors += failure { store().read() }
        assertTrue(logs.isNotEmpty())
        assertNothingKeptSaid(*errors.toTypedArray())
    }

    @Test
    fun valuesOfOtherKindsSurvive() {
        val more = connections + mapOf("later" to mapOf("n" to 3, "on" to true, "none" to null, "list" to listOf(1.5)))
        store().apply { read(); write(more) }
        assertEquals(more, store().read())
    }
}

class DraftStoreTest {
    @get:Rule val folder = TemporaryFolder()

    @Test
    fun roundTripsAndForgivesABrokenFile() {
        val file = File(folder.root, DraftStore.FILE_NAME)
        val logs = mutableListOf<String>()
        assertEquals(emptyMap<String, String>(), DraftStore(file).read())
        DraftStore(file).write(mapOf("builder" to "half a thought", "mike" to "ünïcödé ✓"))
        assertEquals(mapOf("builder" to "half a thought", "mike" to "ünïcödé ✓"), DraftStore(file).read())
        file.writeText("{\"builder\":")
        assertEquals(emptyMap<String, String>(), DraftStore(file) { logs += it }.read())
        assertEquals(1, logs.size)
    }
}
