package com.claktory.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class AudioCoreTest {
    private class FakeSound(val path: String, val onEnd: () -> Unit, private val failPlay: Boolean = false) : Sound {
        override val durationMs = 5_000
        override var positionMs = 0
        override var playing = false
        var released = false

        override fun play() {
            if (failPlay) throw IllegalStateException("can't")
            playing = true
        }

        override fun pause() {
            playing = false
        }

        override fun seekTo(ms: Int) {
            positionMs = ms
        }

        override fun release() {
            released = true
            playing = false
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

    private val sounds = mutableListOf<FakeSound>()
    private val events = mutableListOf<Map<String, Any>>()
    private val active = mutableListOf<Boolean>()
    private val ticker = FakeTicker()
    private var refuse = emptySet<String>()
    private var failPlay = false

    private val core = AudioCore(
        open = { path, onEnd ->
            if (path in refuse) throw java.io.IOException("unreadable")
            FakeSound(path, onEnd, failPlay).also { sounds += it }
        },
        ticker = ticker,
        emit = { events += it },
        active = { active += it },
    )

    private fun states() = events.map { "${it["path"]} ${it["state"]} ${it["positionMs"]}" }

    @Test
    fun playStartsFromWhereAskedAndTicksEvery100ms() {
        core.play("/a.m4a", 1_200)
        assertEquals(1, sounds.size)
        assertTrue(sounds[0].playing)
        assertEquals(listOf("/a.m4a playing 1200"), states())
        assertEquals(5_000, events[0]["durationMs"])
        assertEquals(100L, ticker.every)
        assertEquals(listOf(true), active)
        sounds[0].positionMs = 1_300
        ticker.tick!!()
        assertEquals("/a.m4a playing 1300", states().last())
    }

    @Test
    fun pauseThenPlayResumesTheSameFileFromFromMs() {
        core.play("/a.m4a", 0)
        sounds[0].positionMs = 2_000
        core.pause()
        assertFalse(sounds[0].playing)
        assertEquals(null, ticker.tick)
        assertEquals("/a.m4a paused 2000", states().last())
        core.play("/a.m4a", 2_000)
        assertEquals("the same file isn't opened again", 1, sounds.size)
        assertTrue(sounds[0].playing)
        assertEquals("/a.m4a playing 2000", states().last())
    }

    @Test
    fun anotherFileStopsWhatPlayedFirst() {
        core.play("/a.m4a", 0)
        core.play("/b.m4a", 0)
        assertTrue(sounds[0].released)
        assertEquals(listOf("/a.m4a playing 0", "/a.m4a stopped 0", "/b.m4a playing 0"), states())
        assertEquals(listOf(true, false, true), active)
    }

    @Test
    fun theEndSaysEndedAtItsLengthAndLetsGo() {
        core.play("/a.m4a", 0)
        sounds[0].onEnd()
        assertEquals("/a.m4a ended 5000", states().last())
        assertTrue(sounds[0].released)
        assertEquals(null, ticker.tick)
        assertEquals(listOf(true, false), active)
        // A stale end, from a sound already stopped, says nothing.
        val before = events.size
        sounds[0].onEnd()
        assertEquals(before, events.size)
    }

    @Test
    fun seekMovesWithinTheRecordingAndSaysWhereItIs() {
        core.play("/a.m4a", 0)
        core.seek(9_000)
        assertEquals("/a.m4a playing 5000", states().last())
        core.pause()
        core.seek(-5)
        assertEquals("/a.m4a paused 0", states().last())
    }

    @Test
    fun stopSaysStoppedOnceAndNothingAfter() {
        core.play("/a.m4a", 0)
        core.stop()
        assertEquals("/a.m4a stopped 0", states().last())
        val before = events.size
        core.stop()
        core.pause()
        core.seek(10)
        assertEquals(before, events.size)
        assertEquals(listOf(true, false), active)
    }

    @Test
    fun anInterruptionPausesOnlyWhatPlays() {
        core.play("/a.m4a", 0)
        core.interrupt()
        assertFalse(sounds[0].playing)
        assertEquals("/a.m4a paused 0", states().last())
        val before = events.size
        core.interrupt()
        assertEquals(before, events.size)
    }

    @Test
    fun aFileThatCantBePlayedIsRefused() {
        refuse = setOf("/bad.m4a")
        try {
            core.play("/bad.m4a", 0)
            fail("played")
        } catch (refused: AudioCore.Refused) {
            assertEquals("This recording can't be played.", refused.message)
        }
        failPlay = true
        try {
            core.play("/a.m4a", 0)
            fail("played")
        } catch (refused: AudioCore.Refused) {
            assertEquals("This recording can't be played.", refused.message)
        }
    }
}

class FrameTest {
    @Test
    fun aTextureThatDoesntTurnLeavesTheTurnToTheApp() {
        assertEquals(Frame(1920, 1080, 1), Frame.of(1920, 1080, 90, turned = false))
        assertEquals(Frame(1920, 1080, 3), Frame.of(1920, 1080, 270, turned = false))
        assertEquals(Frame(1920, 1080, 3), Frame.of(1920, 1080, -90, turned = false))
        assertEquals(Frame(640, 480, 0), Frame.of(640, 480, 0, turned = false))
    }

    @Test
    fun aTextureThatTurnsGivesTheTurnedSize() {
        assertEquals(Frame(1080, 1920, 0), Frame.of(1920, 1080, 90, turned = true))
        assertEquals(Frame(1920, 1080, 0), Frame.of(1920, 1080, 180, turned = true))
    }
}
