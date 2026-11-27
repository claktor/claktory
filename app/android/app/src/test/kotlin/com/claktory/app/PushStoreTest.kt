package com.claktory.app

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

/** The push keys' sealed file (PushStore.kt). */
class PushStoreTest {
    @get:Rule
    val folder = TemporaryFolder()

    private val sealer = SoftwareSealer()

    private fun store(file: File = File(folder.root, PushStore.FILE_NAME), appId: String = "com.claktory.app") =
        PushStore(file, appId, sealer)

    private val registration = PushRegistration("handle", "token", "", "fcm", 7)

    @Test
    fun keepsKeysAndRegistrationsPerClaktoryAcrossStores() {
        val key = PushKey.random()
        assertTrue(store().save("e1", key, registration))
        val again = store()
        assertArrayEquals(key.key, again.keys().getValue("e1").key)
        assertEquals(registration, again.registration("e1"))
        assertNull(again.registration("e2"))
    }

    @Test
    fun aShownPushKeepsTheRegistration() {
        val key = PushKey.random()
        store().save("e1", key, registration)
        assertTrue(store().saveKey("e1", key.show(3, "sealed")))
        val kept = store()
        assertEquals(3L, kept.keys().getValue("e1").ctr)
        assertEquals(registration, kept.registration("e1"))
    }

    @Test
    fun forgettingOneLeavesTheOthers() {
        store().save("e1", PushKey.random(), registration)
        store().save("e2", PushKey.random(), registration)
        store().remove("e1")
        assertEquals(setOf("e2"), store().keys().keys)
    }

    @Test
    fun theFileIsSealedAndAnotherAppsReadsAsNothing() {
        store().save("e1", PushKey.random(), registration)
        val bytes = File(folder.root, PushStore.FILE_NAME).readBytes()
        assertEquals("CLPN", String(bytes.copyOfRange(0, 4)))
        assertFalse(String(bytes).contains("handle"))
        // Claktory Dev's file, read as Claktory's: the additional data differs.
        assertTrue(store(appId = "com.claktory.app.dev").keys().isEmpty())
    }

    @Test
    fun aDamagedFileReadsAsNothingKept() {
        val file = File(folder.root, PushStore.FILE_NAME)
        store(file).save("e1", PushKey.random(), registration)
        file.writeBytes(file.readBytes().also { it[it.size - 1] = (it[it.size - 1] + 1).toByte() })
        assertTrue(store(file).keys().isEmpty())
        // And it can be written afresh.
        assertTrue(store(file).save("e1", PushKey.random(), registration))
        assertEquals(setOf("e1"), store(file).keys().keys)
    }

    @Test
    fun aKeyThatCantSealSaysSo() {
        sealer.fails = StoreFailure(StoreFailure.UNAVAILABLE, "no keystore")
        assertFalse(store().save("e1", PushKey.random(), registration))
    }
}
