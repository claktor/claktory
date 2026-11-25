package com.claktory.app

import android.Manifest
import android.app.Activity
import android.media.MediaMetadataRetriever
import android.view.KeyEvent
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.runner.lifecycle.ActivityLifecycleMonitorRegistry
import androidx.test.runner.lifecycle.Stage
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/**
 * Back at the app's root leaves it as Home does: the screen stops but
 * stays, so a recording in progress pauses and waits for Send or Cancel
 * instead of being thrown away with a finished screen.
 */
@RunWith(AndroidJUnit4::class)
class RootBackTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext
    private val folder = File(context.cacheDir, "claktory-root-back-test")

    @Before
    fun setUp() {
        folder.deleteRecursively()
        grant(Manifest.permission.RECORD_AUDIO)
    }

    @After
    fun tearDown() {
        folder.deleteRecursively()
    }

    private fun stage(activity: Activity): Stage {
        var stage: Stage? = null
        instrumentation.runOnMainSync { stage = ActivityLifecycleMonitorRegistry.getInstance().getLifecycleStageOf(activity) }
        return stage!!
    }

    private fun waitFor(activity: Activity, wanted: Stage): Stage {
        val until = System.currentTimeMillis() + 10_000
        var now = stage(activity)
        while (now != wanted && now != Stage.DESTROYED && System.currentTimeMillis() < until) {
            Thread.sleep(50)
            now = stage(activity)
        }
        return now
    }

    /** What factory/recorder answers [method], as the app would hear it. */
    private fun call(screen: Screen, method: String, arguments: Any? = null): Any? {
        var answer: Any? = null
        var failure: String? = null
        screen.onActivity { activity ->
            activity.adapter(RecorderChannel::class.java)!!.onMethodCall(
                MethodCall(method, arguments),
                object : MethodChannel.Result {
                    override fun success(result: Any?) {
                        answer = result
                    }

                    override fun error(code: String, message: String?, details: Any?) {
                        failure = code
                    }

                    override fun notImplemented() {
                        failure = "notImplemented"
                    }
                },
            )
        }
        assertEquals(null, failure)
        return answer
    }

    @Test
    fun backAtTheRootKeepsARecordingWaiting() {
        Screen.launch().use { screen ->
            val path = File(folder, "1/Voice.m4a").path
            call(screen, "start", path)
            Thread.sleep(1_200)

            var left = false
            screen.onActivity { left = it.popSystemNavigator() }
            assertTrue("the app went behind", left)
            assertEquals(Stage.STOPPED, waitFor(screen.activity, Stage.STOPPED))
            assertFalse("the screen isn't finished", screen.activity.isFinishing)

            // Still there to send: the engine and its recorder lived on.
            Thread.sleep(500)
            @Suppress("UNCHECKED_CAST")
            val stopped = call(screen, "stop") as Map<String, Any>?
            assertNotNull("the recording was still waiting", stopped)
            assertEquals(path, stopped!!["path"])
            val ms = stopped["durationMs"] as Int
            assertTrue("ran $ms ms before it paused", ms in 1_100..2_500)
            val retriever = MediaMetadataRetriever().apply { setDataSource(path) }
            val length = retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION)!!.toLong()
            retriever.release()
            assertTrue("the file runs $length ms", length > 800)
        }
    }

    @Test
    fun theBackKeyAtTheRootLeavesTheScreenStopped() {
        Screen.launch().use { screen ->
            // The app's first screen is drawn and listening.
            val until = System.currentTimeMillis() + 10_000
            var focused = false
            while (!focused && System.currentTimeMillis() < until) {
                screen.onActivity { focused = it.currentFocus != null }
                if (!focused) Thread.sleep(100)
            }
            Thread.sleep(1_000)
            instrumentation.sendKeyDownUpSync(KeyEvent.KEYCODE_BACK)
            assertEquals(Stage.STOPPED, waitFor(screen.activity, Stage.STOPPED))
            assertFalse("the screen isn't finished", screen.activity.isFinishing)
        }
    }
}
