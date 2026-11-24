package com.claktory.app

import android.Manifest
import android.os.Handler
import android.os.Looper
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.flutter.embedding.engine.FlutterEngine
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.sin

/**
 * Voice messages and videos played by the system's player, as
 * factory/audio and claktory/video use it: real files, the real
 * MediaPlayer, and for videos a real Flutter engine's textures. What
 * comes out of the speaker and the screen is checked by hand on a phone.
 */
@RunWith(AndroidJUnit4::class)
class PlaybackTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext
    private val folder = File(context.cacheDir, "claktory-playback-test")
    private val main = Handler(Looper.getMainLooper())

    @Before
    fun setUp() {
        folder.deleteRecursively()
        folder.mkdirs()
    }

    @After
    fun tearDown() {
        folder.deleteRecursively()
    }

    /** [body] on the main thread, as the channels run it, waited for. */
    private fun <T> onMain(body: () -> T): T {
        var answer: Result<T>? = null
        val done = CountDownLatch(1)
        main.post {
            answer = runCatching(body)
            done.countDown()
        }
        assertTrue(done.await(20, TimeUnit.SECONDS))
        return answer!!.getOrThrow()
    }

    /** Waits until [events] holds one, past the first [after], that [matches]. */
    private fun <T> await(events: List<T>, what: String, after: Int = 0, seconds: Long = 10, matches: (T) -> Boolean): T {
        val until = System.currentTimeMillis() + seconds * 1000
        while (System.currentTimeMillis() < until) {
            synchronized(events) { events.drop(after).lastOrNull(matches) }?.let { return it }
            Thread.sleep(20)
        }
        throw AssertionError("no $what in ${synchronized(events) { events.toList() }}")
    }

    /** A second of a 440 Hz tone, 16 kHz mono 16-bit WAV. */
    private fun tone(name: String, ms: Int = 1_000): File {
        val rate = 16_000
        val samples = rate * ms / 1000
        val data = ByteBuffer.allocate(44 + samples * 2).order(ByteOrder.LITTLE_ENDIAN)
        data.put("RIFF".toByteArray()).putInt(36 + samples * 2).put("WAVE".toByteArray())
        data.put("fmt ".toByteArray()).putInt(16).putShort(1).putShort(1).putInt(rate).putInt(rate * 2).putShort(2).putShort(16)
        data.put("data".toByteArray()).putInt(samples * 2)
        for (i in 0 until samples) data.putShort((sin(2 * PI * 440 * i / rate) * 8_000).toInt().toShort())
        return File(folder, name).apply { writeBytes(data.array()) }
    }

    private fun audio(events: MutableList<Map<String, Any>>) = AudioCore(
        open = { path, onEnd -> MediaSound(path) { main.post(onEnd) } },
        ticker = MainTicker(main),
        emit = { synchronized(events) { events += it } },
    )

    @Test
    fun aRecordingPlaysToItsEndAndSaysSo() {
        val events = mutableListOf<Map<String, Any>>()
        val core = audio(events)
        val path = tone("tone.wav").path
        onMain { core.play(path, 0) }
        val first = synchronized(events) { events.first() }
        assertEquals(path, first["path"])
        assertEquals("playing", first["state"])
        assertTrue("lasts ${first["durationMs"]} ms", abs((first["durationMs"] as Int) - 1_000) < 100)
        val ended = await(events, "end") { it["state"] == "ended" }
        assertEquals(ended["durationMs"], ended["positionMs"])
        val ticks = synchronized(events) { events.count { it["state"] == "playing" } }
        assertTrue("ticked $ticks times", ticks >= 5)
        // Nothing after the end; playing again opens it afresh.
        onMain { core.pause() }
        assertEquals("ended", synchronized(events) { events.last()["state"] })
    }

    @Test
    fun pausingAndPlayingOnResumesFromWhereAsked() {
        val events = mutableListOf<Map<String, Any>>()
        val core = audio(events)
        val path = tone("long.wav", 3_000).path
        onMain { core.play(path, 1_500) }
        assertTrue((synchronized(events) { events.first() }["positionMs"] as Int) >= 1_400)
        Thread.sleep(300)
        onMain { core.pause() }
        val paused = synchronized(events) { events.last() }
        assertEquals("paused", paused["state"])
        val at = paused["positionMs"] as Int
        assertTrue("paused at $at", at in 1_600..2_600)
        Thread.sleep(300)
        onMain { core.seek(500) }
        assertEquals("paused", synchronized(events) { events.last()["state"] })
        val mark = synchronized(events) { events.size }
        onMain { core.play(path, 500) }
        await(events, "playing from 500 ms", after = mark) { it["state"] == "playing" && (it["positionMs"] as Int) in 400..1_200 }
        onMain { core.stop() }
        assertEquals("stopped", synchronized(events) { events.last()["state"] })
    }

    @Test
    fun aVoiceMessageInTheIphonesFormatPlays() {
        grant(Manifest.permission.RECORD_AUDIO)
        val recorder = RecorderCore(newMic = { MediaMic(context) }, permitted = { true }, ticker = MainTicker(main), emit = {})
        val path = File(folder, "Voice.m4a").path
        onMain { recorder.start(path) }
        Thread.sleep(1_200)
        onMain { recorder.stop() }
        val events = mutableListOf<Map<String, Any>>()
        val core = audio(events)
        onMain { core.play(path, 0) }
        assertTrue((synchronized(events) { events.first() }["durationMs"] as Int) > 800)
        await(events, "end") { it["state"] == "ended" }
    }

    @Test
    fun aFileThatIsntSoundIsRefused() {
        val core = audio(mutableListOf())
        val path = File(folder, "not.m4a").apply { writeText("not a recording") }.path
        val refused = onMain { runCatching { core.play(path, 0) }.exceptionOrNull() }
        assertTrue(refused is AudioCore.Refused)
        assertEquals("This recording can't be played.", refused!!.message)
    }

    /**
     * About a second of H.264 video, 320×240, made upright on a phone held
     * upright (turned 90°): a fixed file of the tests' own (assets/clip.mp4,
     * made with ffmpeg), not one recorded here. Android 7's emulator records
     * from a surface with broken timestamps, so a clip recorded there lasted
     * 0 s or weeks, which said nothing of the player.
     */
    private fun video(name: String): File {
        val file = File(folder, name)
        instrumentation.context.assets.open("clip.mp4").use { from -> file.outputStream().use { from.copyTo(it) } }
        return file
    }

    @Test
    fun aVideoOpensOnATexturePlaysSeeksAndEnds() {
        val file = video("clip.mp4")
        val engine = onMain { FlutterEngine(context) }
        val states = mutableListOf<Map<String, Any>>()
        val players = onMain { VideoPlayers(engine.renderer) { synchronized(states) { states += it } } }
        try {
            val opened = CountDownLatch(1)
            var answer: Map<String, Any>? = null
            var error: String? = null
            onMain {
                players.open(file.path) { state, why ->
                    answer = state
                    error = why
                    opened.countDown()
                }
            }
            assertTrue(opened.await(10, TimeUnit.SECONDS))
            assertNull(error)
            val state = answer!!
            val texture = state["texture"] as Long
            val frame = Frame(
                (state["width"] as Double).toInt(),
                (state["height"] as Double).toInt(),
                state["quarterTurns"] as Int,
            )
            assertTrue("the frame is $frame", frame == Frame(320, 240, 1) || frame == Frame(240, 320, 0))
            val duration = state["duration"] as Double
            assertTrue("lasts $duration s", duration in 0.7..2.0)
            assertEquals(false, state["playing"])

            onMain { players.play(texture) }
            await(states, "playing") { it["playing"] == true && it["texture"] == texture }
            val ended = await(states, "end") { it["playing"] == false && (it["position"] as Double) >= duration - 0.1 }
            assertEquals(texture, ended["texture"])

            // Played to its end, it plays from the start again.
            var mark = synchronized(states) { states.size }
            onMain { players.play(texture) }
            await(states, "playing again", after = mark) { it["playing"] == true && (it["position"] as Double) < 0.5 }
            mark = synchronized(states) { states.size }
            onMain { players.pause(texture) }
            await(states, "paused", after = mark) { it["playing"] == false }

            val sought = CountDownLatch(1)
            onMain { players.seek(texture, 0.5) { sought.countDown() } }
            assertTrue(sought.await(5, TimeUnit.SECONDS))
            val at = synchronized(states) { states.last() }["position"] as Double
            assertTrue("sought to $at", abs(at - 0.5) < 0.2)

            onMain { players.close(texture) }
            val count = synchronized(states) { states.size }
            onMain { players.play(texture) }
            Thread.sleep(300)
            assertEquals("a closed player says nothing", count, synchronized(states) { states.size })
        } finally {
            onMain {
                players.close()
                engine.destroy()
            }
        }
    }

    @Test
    fun aFileThatIsntVideoSaysWhy() {
        val engine = onMain { FlutterEngine(context) }
        val players = onMain { VideoPlayers(engine.renderer) {} }
        try {
            val path = File(folder, "not.mp4").apply { writeText("not a video") }.path
            val done = CountDownLatch(1)
            var answer: Map<String, Any>? = mapOf()
            var error: String? = null
            onMain {
                players.open(path) { state, why ->
                    answer = state
                    error = why
                    done.countDown()
                }
            }
            assertTrue(done.await(10, TimeUnit.SECONDS))
            assertNull(answer)
            assertEquals("Claktory can't play this video.", error)
            assertFalse(File(folder, "missing.mp4").exists())
        } finally {
            onMain {
                players.close()
                engine.destroy()
            }
        }
    }
}
