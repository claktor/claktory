package com.claktory.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

class RecorderCoreTest {
    @get:Rule
    val folder = TemporaryFolder()

    private class FakeMic(private val failStart: Boolean = false, private val failStop: Boolean = false) : Mic {
        var path: String? = null
        var maxMs = 0
        var onLimit: () -> Unit = {}
        var onError: () -> Unit = {}
        var amplitude = 0
        var paused = false
        var stopped = 0
        var released = false

        override fun start(path: String, maxMs: Int, onLimit: () -> Unit, onError: () -> Unit) {
            if (failStart) throw IllegalStateException("no mic")
            this.path = path
            this.maxMs = maxMs
            this.onLimit = onLimit
            this.onError = onError
            File(path).writeText("audio")
        }

        override fun amplitude() = amplitude

        override fun pause() {
            paused = true
        }

        override fun stop() {
            stopped++
            if (failStop) throw RuntimeException("stop failed")
        }

        override fun release() {
            released = true
        }
    }

    private class FakeTicker : Ticker {
        var tick: (() -> Unit)? = null
        var every = 0L

        override fun start(every: Long, tick: () -> Unit) {
            this.every = every
            this.tick = tick
        }

        override fun stop() {
            tick = null
        }
    }

    private var now = 1_000L
    private var permitted = true
    private val events = mutableListOf<Map<String, Any>>()
    private val ticker = FakeTicker()
    private val mics = mutableListOf<FakeMic>()
    private var nextMic: () -> FakeMic = { FakeMic() }

    private val core = RecorderCore(
        newMic = { nextMic().also { mics += it } },
        permitted = { permitted },
        ticker = ticker,
        emit = { events += it },
        clock = { now },
    )

    private fun path() = File(folder.root, "voice/1/Voice.m4a").path

    private fun refused(code: String, body: () -> Unit) {
        try {
            body()
            fail("expected $code")
        } catch (refused: RecorderCore.Refused) {
            assertEquals(code, refused.code)
        }
    }

    @Test
    fun `a recording runs ten minutes at most, ticks every 50 ms and stops with its length`() {
        core.start(path())
        assertEquals(RecorderCore.MAX_MS, mics[0].maxMs)
        assertEquals(50L, ticker.every)
        assertTrue("its folder made", File(path()).exists())
        now += 1_234
        mics[0].amplitude = 32767
        ticker.tick!!()
        assertEquals(mapOf("level" to 1.0, "ms" to 1234), events.last())
        val stopped = core.stop()!!
        assertEquals(mapOf("path" to path(), "durationMs" to 1234), stopped)
        assertEquals(1, mics[0].stopped)
        assertTrue(mics[0].released)
        assertNull("no ticks after", ticker.tick)
        assertNull("nothing recording now", core.stop())
    }

    @Test
    fun `refusals say why, as the iPhone does`() {
        permitted = false
        refused("denied") { core.start(path()) }
        assertTrue("no mic touched", mics.isEmpty())
        permitted = true
        core.start(path())
        refused("busy") { core.start(path()) }
        core.cancel()
        nextMic = { FakeMic(failStart = true) }
        refused("failed") { core.start(path()) }
        assertTrue(mics.last().released)
        assertFalse(core.recording)
    }

    @Test
    fun `cancel stops and deletes the file`() {
        core.start(path())
        core.cancel()
        assertFalse(File(path()).exists())
        assertEquals(1, mics[0].stopped)
        assertTrue(mics[0].released)
        assertNull(core.stop())
        core.cancel()
    }

    @Test
    fun `at the limit it says so once and keeps the file for stop, which doesn't stop it again`() {
        core.start(path())
        now += RecorderCore.MAX_MS + 30
        mics[0].onLimit()
        mics[0].onLimit()
        assertEquals(listOf(mapOf<String, Any>("ended" to "limit")), events)
        assertNull("no ticks after", ticker.tick)
        assertEquals(mapOf("path" to path(), "durationMs" to RecorderCore.MAX_MS), core.stop())
        assertEquals("the mic finished its file itself", 0, mics[0].stopped)
        assertTrue(File(path()).exists())
    }

    @Test
    fun `interrupted, it pauses and keeps the file and its length for stop`() {
        core.start(path())
        now += 4_000
        core.interrupt()
        now += 60_000
        core.interrupt()
        assertEquals(listOf(mapOf<String, Any>("ended" to "interrupted")), events)
        assertTrue(mics[0].paused)
        assertEquals(mapOf("path" to path(), "durationMs" to 4_000), core.stop())
        assertEquals(1, mics[0].stopped)
        core.interrupt()
        assertEquals("nothing recording, nothing said", 1, events.size)
    }

    @Test
    fun `a failure deletes the file and says so`() {
        core.start(path())
        mics[0].onError()
        assertEquals(listOf(mapOf<String, Any>("ended" to "error")), events)
        assertFalse(File(path()).exists())
        assertNull(core.stop())
        // A later recording is its own; the old mic's word is ignored.
        core.start(path())
        mics[0].onError()
        mics[0].onLimit()
        assertEquals(1, events.size)
        assertTrue(core.recording)
    }

    @Test
    fun `a recording too short to make a file of stops with no length`() {
        nextMic = { FakeMic(failStop = true) }
        core.start(path())
        now += 120
        assertEquals(mapOf("path" to path(), "durationMs" to 0), core.stop())
        assertTrue(mics[0].released)
    }

    @Test
    fun `loudness runs from silence at -50 dB to 1 at full scale`() {
        assertEquals(0.0, RecorderCore.level(0), 0.0)
        assertEquals(0.0, RecorderCore.level(10), 0.0)
        assertEquals(1.0, RecorderCore.level(32767), 1e-9)
        assertEquals(0.6, RecorderCore.level((32767 * 0.1).toInt()), 0.01)
    }
}
