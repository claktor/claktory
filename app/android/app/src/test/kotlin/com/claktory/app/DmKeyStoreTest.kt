package com.claktory.app

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

/** factory/dm-keys' store (dm_keys.dart's contract) over a software key. */
class DmKeyStoreTest {
    @get:Rule val folder = TemporaryFolder()

    private lateinit var keys: File
    private lateinit var sealer: SoftwareSealer
    private val logs = mutableListOf<String>()
    private val appId = "com.claktory.app"

    // 32 bytes each, as the app's Rust makes them; recognisable in a file.
    private val identity = ByteArray(32) { (0xA0 + it).toByte() }
    private val leaf = ByteArray(32) { (0x40 + it).toByte() }

    @Before
    fun setUp() {
        keys = File(folder.root, DmKeyStore.FOLDER)
        sealer = SoftwareSealer()
    }

    private fun store(id: String = appId, sealer: Sealer = this.sealer) = DmKeyStore(keys, id, sealer) { logs += it }

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

    @Test
    fun nothingKeptIsNullAndWhatIsKeptComesBack() {
        val store = store()
        assertNull(store.get("identity"))
        store.put("identity", identity)
        store.put("leaf", leaf)
        assertArrayEquals(identity, store.get("identity"))
        assertArrayEquals(leaf, store.get("leaf"))
        // Another store on the same folder and key: what the next launch finds.
        assertArrayEquals(identity, store().get("identity"))
        assertNull(store.get("store"))
    }

    @Test
    fun putReplacesAndDeleteForgets() {
        val store = store()
        store.put("identity", identity)
        store.put("identity", leaf)
        assertArrayEquals(leaf, store.get("identity"))
        store.delete("identity")
        assertNull(store.get("identity"))
        store.delete("identity")
        store.delete("leaf")
        assertTrue(sealer.hasKey())
    }

    @Test
    fun theFileHoldsTheHeaderAndNeverTheKey() {
        store().put("identity", identity)
        val bytes = File(keys, "identity.key").readBytes()
        assertArrayEquals(DmKeyStore.HEADER, bytes.copyOfRange(0, 7))
        assertEquals(7 + 12 + 32 + 16, bytes.size)
        assertFalse(contains(bytes, identity))
        assertEquals(listOf("identity.key"), keys.list()!!.toList())
    }

    @Test
    fun aFileMovedToAnotherNameIsUnreadableNotAbsent() {
        val store = store()
        store.put("identity", identity)
        File(keys, "identity.key").copyTo(File(keys, "leaf.key"))
        assertEquals(DmKeyFailure.UNREADABLE, failure { store.get("leaf") }.code)
        assertArrayEquals(identity, store.get("identity"))
    }

    @Test
    fun anotherAppsFileIsUnreadable() {
        store("com.claktory.app.dev").put("identity", identity)
        assertEquals(DmKeyFailure.UNREADABLE, failure { store().get("identity") }.code)
    }

    @Test
    fun theKeyGoneIsUnreadableNotAbsent() {
        store().put("identity", identity)
        sealer.deleteKey()
        val failure = failure { store().get("identity") }
        assertEquals(DmKeyFailure.UNREADABLE, failure.code)
        assertTrue(failure.message!!.contains(StoreFailure.KEY_MISSING))
        // A new key made by a later put leaves the old file unreadable still.
        store().put("leaf", leaf)
        assertEquals(DmKeyFailure.UNREADABLE, failure { store().get("identity") }.code)
        assertArrayEquals(leaf, store().get("leaf"))
    }

    @Test
    fun keystoreRefusingIsUnreadableOnGetAndFailedOnPutAndDelete() {
        store().put("identity", identity)
        sealer.fails = StoreFailure(StoreFailure.UNAVAILABLE, "no answer")
        assertEquals(DmKeyFailure.UNREADABLE, failure { store().get("identity") }.code)
        assertEquals(DmKeyFailure.FAILED, failure { store().put("leaf", leaf) }.code)
        // Absent stays absent: Keystore isn't asked when there's no file.
        assertNull(store().get("leaf"))
        assertFalse(File(keys, "leaf.key").exists())
        sealer.fails = null
        assertArrayEquals(identity, store().get("identity"))
    }

    @Test
    fun damageIsUnreadable() {
        val store = store()
        val file = File(keys, "identity.key")
        store.put("identity", identity)
        val good = file.readBytes()
        for (index in good.indices) {
            val bad = good.copyOf().also { it[index] = (it[index].toInt() xor 0x01).toByte() }
            file.writeBytes(bad)
            assertEquals("byte $index", DmKeyFailure.UNREADABLE, failure { store.get("identity") }.code)
        }
        for (length in listOf(0, 3, 7, 7 + 12 + 15)) {
            file.writeBytes(good.copyOf(length))
            assertEquals("$length bytes", DmKeyFailure.UNREADABLE, failure { store.get("identity") }.code)
        }
        file.writeBytes(good)
        assertArrayEquals(identity, store.get("identity"))
    }

    @Test
    fun aNewerFormatIsUnreadable() {
        val store = store()
        val file = File(keys, "identity.key")
        store.put("identity", identity)
        file.writeBytes(file.readBytes().also { it[4] = 2 })
        val failure = failure { store.get("identity") }
        assertEquals(DmKeyFailure.UNREADABLE, failure.code)
        assertTrue(failure.message!!.contains(StoreFailure.NEWER_VERSION))
    }

    @Test
    fun anUnfinishedWriteLeavesTheLastOne() {
        val store = store()
        store.put("identity", identity)
        // A write that died before its rename: only the old file counts.
        File(keys, "identity.key.new").writeBytes(byteArrayOf(1, 2, 3))
        assertArrayEquals(identity, store.get("identity"))
        File(keys, "leaf.key.new").writeBytes(byteArrayOf(1, 2, 3))
        assertNull(store.get("leaf"))
        store.delete("leaf")
        assertFalse(File(keys, "leaf.key.new").exists())
    }

    @Test
    fun onlyKeyNamesAreTaken() {
        val store = store()
        for (name in listOf("", "Identity", "../identity", "a/b", "a.b", "identity.key", "é", "x".repeat(33))) {
            assertEquals(name, DmKeyFailure.FAILED, failure { store.put(name, identity) }.code)
            assertEquals(name, DmKeyFailure.FAILED, failure { store.get(name) }.code)
            assertEquals(name, DmKeyFailure.FAILED, failure { store.delete(name) }.code)
        }
        store.put("x".repeat(32), identity)
        assertArrayEquals(identity, store.get("x".repeat(32)))
        assertFalse(File(folder.root, "identity.key").exists())
    }

    @Test
    fun aValueFarLongerThanAKeyIsRefused() {
        assertEquals(DmKeyFailure.FAILED, failure { store().put("identity", ByteArray(4097)) }.code)
        store().put("identity", ByteArray(0))
        assertArrayEquals(ByteArray(0), store().get("identity"))
    }

    @Test
    fun availableIsARoundTripThatLeavesNothing() {
        assertTrue(store().available())
        assertTrue(sealer.hasKey())
        assertTrue(keys.list()!!.isEmpty())
        sealer.fails = StoreFailure(StoreFailure.UNAVAILABLE, "no answer")
        assertFalse(store().available())
        assertTrue(keys.list()!!.isEmpty())
    }

    @Test
    fun availableIsFalseWhenTheFolderCantBeWritten() {
        keys.parentFile!!.mkdirs()
        keys.writeText("a file where the folder should be")
        assertFalse(store().available())
    }

    @Test
    fun noLogOrMessageCarriesAKey() {
        val store = store()
        store.put("identity", identity)
        sealer.deleteKey()
        val messages = listOf(
            failure { store.get("identity") }.message!!,
            failure { store.put("Bad", identity) }.message!!,
        ) + logs
        val hex = identity.joinToString("") { "%02x".format(it) }
        for (message in messages) {
            assertFalse(message, message.contains(hex))
        }
        assertTrue(logs.any { it.startsWith("couldn't read identity") })
    }

    @Test
    fun theStoresFolderIsMadeAndAnsweredTheSameEachTime() {
        val dm = File(folder.root, "no_backup/${DmKeysChannel.STORE_FOLDER}")
        val path = DmKeysChannel.madeFolder(dm)
        assertEquals(dm.absolutePath, path)
        assertTrue(dm.isDirectory)
        File(dm, "store.db").writeText("kept")
        assertEquals(path, DmKeysChannel.madeFolder(dm))
        assertEquals("kept", File(dm, "store.db").readText())
    }

    @Test
    fun aFolderThatCantBeMadeFails() {
        val blocked = File(folder.root, "dm").also { it.writeText("a file where the folder should be") }
        val failure = failure { DmKeysChannel.madeFolder(blocked) }
        assertEquals(DmKeyFailure.FAILED, failure.code)
    }
}
