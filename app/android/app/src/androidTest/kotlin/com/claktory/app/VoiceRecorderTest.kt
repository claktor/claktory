package com.claktory.app

import android.Manifest
import android.media.MediaExtractor
import android.media.MediaFormat
import android.media.MediaMetadataRetriever
import android.os.Handler
import android.os.Looper
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * The phone's microphone through [RecorderCore], as factory/recorder uses
 * it: a real recording, in the iPhone's format (AAC in .m4a, 16 kHz mono).
 * The emulator's microphone hears silence, so loudness isn't checked;
 * a call interrupting and the ten-minute limit are [RecorderCoreTest]'s.
 */
@RunWith(AndroidJUnit4::class)
class VoiceRecorderTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext
    private val folder = File(context.cacheDir, "claktory-voice-test")
    private val main = Handler(Looper.getMainLooper())
    private val events = mutableListOf<Map<String, Any>>()

    @Before
    fun setUp() {
        folder.deleteRecursively()
        grant(Manifest.permission.RECORD_AUDIO)
    }

    @After
    fun tearDown() {
        folder.deleteRecursively()
    }

    /** [body] on the main thread, as the channel runs it, waited for. */
    private fun <T> onMain(body: () -> T): T {
        var answer: Result<T>? = null
        val done = CountDownLatch(1)
        main.post {
            answer = runCatching(body)
            done.countDown()
        }
        assertTrue(done.await(10, TimeUnit.SECONDS))
        return answer!!.getOrThrow()
    }

    private fun core() = RecorderCore(
        newMic = { MediaMic(context) },
        permitted = { true },
        ticker = object : Ticker {
            private var running: Runnable? = null

            override fun start(every: Long, tick: () -> Unit) {
                val next = object : Runnable {
                    override fun run() {
                        tick()
                        main.postDelayed(this, every)
                    }
                }
                running = next
                main.postDelayed(next, every)
            }

            override fun stop() {
                running?.let(main::removeCallbacks)
            }
        },
        emit = { synchronized(events) { events += it } },
    )

    @Test
    fun aRecordingIsAacInM4aAt16kHzMonoAndAsLongAsItRan() {
        val core = core()
        val path = File(folder, "1/Voice.m4a").path
        onMain { core.start(path) }
        Thread.sleep(1_500)
        val stopped = onMain { core.stop() }!!
        assertEquals(path, stopped["path"])
        val ms = stopped["durationMs"] as Int
        assertTrue("ran $ms ms", ms in 1_400..3_000)
        val ticks = synchronized(events) { events.count { it.containsKey("ms") } }
        assertTrue("ticked $ticks times", ticks >= 10)

        val extractor = MediaExtractor().apply { setDataSource(path) }
        assertEquals(1, extractor.trackCount)
        val format = extractor.getTrackFormat(0)
        assertEquals(MediaFormat.MIMETYPE_AUDIO_AAC, format.getString(MediaFormat.KEY_MIME))
        assertEquals(16_000, format.getInteger(MediaFormat.KEY_SAMPLE_RATE))
        assertEquals(1, format.getInteger(MediaFormat.KEY_CHANNEL_COUNT))
        extractor.release()
        val retriever = MediaMetadataRetriever().apply { setDataSource(path) }
        val length = retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION)!!.toLong()
        retriever.release()
        assertTrue("the file runs $length ms", length in 1_000..3_000)
    }

    @Test
    fun anInterruptedRecordingKeepsWhatItHad() {
        val core = core()
        val path = File(folder, "2/Voice.m4a").path
        onMain { core.start(path) }
        Thread.sleep(1_200)
        onMain { core.interrupt() }
        Thread.sleep(500)
        val stopped = onMain { core.stop() }!!
        assertTrue(synchronized(events) { events.contains(mapOf<String, Any>("ended" to "interrupted")) })
        assertTrue((stopped["durationMs"] as Int) in 1_100..2_500)
        val retriever = MediaMetadataRetriever().apply { setDataSource(path) }
        assertTrue(retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION)!!.toLong() > 800)
        retriever.release()
    }

    @Test
    fun cancelLeavesNoFile() {
        val core = core()
        val path = File(folder, "3/Voice.m4a").path
        onMain { core.start(path) }
        Thread.sleep(500)
        onMain { core.cancel() }
        assertFalse(File(path).exists())
    }
}
